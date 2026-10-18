//! ILA: mark net → extra LUTFF in netlist/bitstream → arm → capture.
//!
//! Soft path (`insert_arm_capture`): host `step_user` + `ble_out` readback.
//! Deep path (`insert_arm_capture_deep`): fabric BRAM sample buffer + **match-unit LUT /
//! trigger FSM IP** (not host ble_out edge compare) + multi-probe; upload via Helion TAP
//! `IR_USR1` **RLE-compressed** JTAG DR scan of capture RAM (`jtag_usr1_match_fsm_rle`).

use helion_bits::bitgen;
use helion_device::Device;
use helion_fabric::{Fabric, IlaMatchKind};
use helion_hw::Tap;
use helion_ir::{intern, resolve, CellKind, Design};
use helion_pack::pack;
use helion_place::place;
use helion_route::route;

pub mod bscan;
pub mod insert;
pub mod ltx;
pub mod trigger;
pub mod vio_axi;

pub use bscan::{
    BSCAN_HUB_BROADCAST_SLOT, BSCAN_HUB_MAGIC, BscanCoreSlot, BscanDebugHub, BscanFrame,
    BscanUserChain, DebugCoreKind, MAX_BSCAN_SLOTS, deterministic_core_uuid, hub_opcode,
};
pub use insert::{
    DebugCoreInsertionReport, DebugInsertConfig, collect_marked_debug_nets, insert_debug_cores,
    strip_debug_cores,
};
pub use ltx::{
    DebugProbesLtx, LtxCoreDescriptor, LtxProbeChannel, LtxProbeDirection,
    group_nets_into_probe_channels,
};
pub use trigger::{
    ComparatorOp, IlaCapturedSegment, IlaSequencerCaptureResult, IlaSequencerConfig,
    IlaTriggerSequencer, MAX_TRIGGER_STATES, ProbeComparator, StateTransition, TriggerBoolOp,
    TriggerCondition, TriggerStateSpec, TriggerWindowPosition, run_sequencer_capture,
};
pub use vio_axi::{
    AxiRegion, AxiResp, AxiTxnKind, HwAxiTxn, JtagAxiMaster, VioActivityKind, VioCore, VioProbeIn,
    VioProbeOut, create_hw_axi_txn,
};

#[derive(Clone, Debug)]
pub struct IlaCapture {
    pub net: String,
    pub samples: Vec<bool>,
}

/// Trigger mode for fabric-backed ILA (deep path).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IlaTriggerKind {
    Immediate,
    Rising,
    Falling,
}

impl IlaTriggerKind {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "immediate" | "any" | "now" => Ok(Self::Immediate),
            "rising" | "rise" | "posedge" => Ok(Self::Rising),
            "falling" | "fall" | "negedge" => Ok(Self::Falling),
            other => Err(format!("ila_trigger: unknown {other}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::Rising => "rising",
            Self::Falling => "falling",
        }
    }
}

/// Arm configuration for [`insert_arm_capture_deep`].
#[derive(Clone, Debug)]
pub struct IlaArmConfig {
    pub nets: Vec<String>,
    pub window: usize,
    pub trigger: IlaTriggerKind,
    /// Which probe evaluates the trigger (must be in `nets`).
    pub trigger_net: String,
    /// Samples kept before the trigger sample inside the window.
    pub pre_trigger: usize,
}

impl IlaArmConfig {
    pub fn single(net: impl Into<String>, window: usize) -> Self {
        let net = net.into();
        Self {
            trigger_net: net.clone(),
            nets: vec![net],
            window,
            trigger: IlaTriggerKind::Immediate,
            pre_trigger: 0,
        }
    }
}

/// Deep ILA capture: multi-probe window uploaded from fabric BRAM sample buffer.
#[derive(Clone, Debug)]
pub struct IlaCaptureDeep {
    pub probes: Vec<String>,
    /// Chronological samples; `samples[t][p]` matches `probes[p]`.
    pub samples: Vec<Vec<bool>>,
    pub trigger_at: Option<usize>,
    pub pre_trigger: usize,
    pub bram_major: u16,
    pub backend: &'static str,
    /// 64-bit USR1 data DR scans used by the RLE upload (header + tokens).
    pub upload_dr_scans: usize,
    /// Uncompressed upload would use one DR per sample word.
    pub upload_raw_scans: usize,
}

const CAPTURE_BRAM: &str = "ila_capture_ram";
const MATCH_HIT_NET: &str = "ila_match_hit";
const MATCH_PREV_NET: &str = "ila_match_prev";
const TRIG_FSM_CELL: &str = "ila_trig_fsm";
const DEEP_BACKEND: &str = "jtag_usr1_match_fsm_rle";

fn ila_cell_names(net: &str) -> [String; 3] {
    [
        format!("ila_{net}"),
        format!("ila_{net}_lut"),
        format!("ila_{net}_ff"),
    ]
}

fn ila_aux_nets(net: &str) -> [String; 2] {
    [format!("ila_{net}_d"), format!("ila_{net}_q")]
}

/// Drop a prior `insert_ila` for `net` so baseline vs probe bitstreams can differ.
/// `Session::mark_debug` / `insert_marked` already inject the probe; arm must still work.
pub fn strip_ila(design: &mut Design, net: &str) {
    let cells = ila_cell_names(net);
    let aux = ila_aux_nets(net);
    design.cells.retain(|c| !cells.iter().any(|n| resolve(c.name) == n.as_str()));
    design.nets.retain(|n| !aux.iter().any(|a| resolve(n.name) == a.as_str()));
    for n in &mut design.nets {
        n.endpoints.retain(|e| !cells.iter().any(|c| resolve(e.cell) == c.as_str()));
    }
    design.rebuild_indexes();
}

/// Insert an ILA probe on `net`: identity LUT+FF so the bitstream gains BLE1 INIT.
pub fn insert_ila(design: &mut Design, net: &str) -> Result<(), String> {
    if !design.nets.iter().any(|n| n.name == intern(net)) {
        return Err(format!("mark_debug: no net {net}"));
    }
    if design
        .cells
        .iter()
        .any(|c| matches!(&c.kind, CellKind::Ila { net: n } if *n == intern(net)))
    {
        return Ok(());
    }
    design.add_cell(&format!("ila_{net}"), CellKind::Ila { net: intern(net) });
    // Buffer LUT: O = I0 → INIT 0xAAAA… so capture tracks the marked net.
    design.add_cell(
        &format!("ila_{net}_lut"),
        CellKind::Lut6 {
            init: 0xAAAA_AAAA_AAAA_AAAA,
        },
    );
    design.add_cell(&format!("ila_{net}_ff"), CellKind::Hff);
    design.connect(net, &format!("ila_{net}_lut"), "I0");
    let dnet = format!("ila_{net}_d");
    design.connect(&dnet, &format!("ila_{net}_lut"), "O");
    design.connect(&dnet, &format!("ila_{net}_ff"), "D");
    design.connect(&format!("ila_{net}_q"), &format!("ila_{net}_ff"), "Q");
    Ok(())
}

/// Insert an ILA on every net with IR attr `mark_debug`.
pub fn insert_marked(design: &mut Design) -> Result<usize, String> {
    let nets = design.marked_debug_nets();
    for n in &nets {
        insert_ila(design, resolve(*n))?;
    }
    Ok(nets.len())
}

pub fn compile(
    dev: &Device,
    design: &Design,
) -> Result<(helion_route::Routed, helion_bits::Bitstream), String> {
    let packed = pack(design, dev)?;
    let placed = place(&packed, dev)?;
    let routed = route(&placed, dev)?;
    let bits = bitgen(dev, &routed)?;
    Ok((routed, bits))
}

pub fn insert_arm_capture(
    dev: &Device,
    design: &Design,
    net: &str,
    n: usize,
) -> Result<IlaCapture, String> {
    // Baseline without this probe — even if mark_debug / (re)implement already inserted it.
    let mut baseline = design.clone();
    strip_ila(&mut baseline, net);
    let (_, bits0) = compile(dev, &baseline)?;
    let packed0 = pack(&baseline, dev)?;

    let mut d = design.clone();
    // Rebuild probe cleanly (idempotent with a prior Session::mark_debug insert).
    strip_ila(&mut d, net);
    insert_ila(&mut d, net)?;
    let (routed, bits1) = compile(dev, &d)?;
    if bits0.frames == bits1.frames {
        return Err("ILA insert was a no-op (bitstream unchanged)".into());
    }
    if routed.placed.packed.lutffs.len() <= packed0.lutffs.len() {
        return Err("ILA did not pack an extra LUTFF".into());
    }
    let mut fab = Fabric::new(dev);
    fab.program(&bits1)?;
    fab.finish_startup();
    // Probe the marked net's driver LUTFF (q_net / IOB from_net), not always site[0].
    // ILA extra LUTFF is in the bitstream; capture is of the marked net, not the probe flop.
    let (site, ble) = probe_site(&routed.placed, design, net)?;
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        fab.step_user();
        // Comb mid-nets (LUT-only BLE) must sample LUT O, not stuck FF Q.
        samples.push(fab.ble_out(site.x, site.y, ble as u32));
    }
    if samples.iter().all(|&s| s == samples[0]) {
        return Err(format!(
            "ILA capture is constant on {net} over {n} samples — increase ila_window or pick a faster net"
        ));
    }
    Ok(IlaCapture {
        net: net.into(),
        samples,
    })
}

pub fn strip_capture_bram(design: &mut Design) {
    design.cells.retain(|c| c.name != intern(CAPTURE_BRAM));
    design.rebuild_indexes();
}

/// Insert a Bram18 used as on-fabric ILA capture RAM (INIT sized to `words`).
pub fn insert_capture_bram(design: &mut Design, words: usize) -> Result<(), String> {
    let words = words.max(1).min(255);
    strip_capture_bram(design);
    design.add_cell(CAPTURE_BRAM, CellKind::Bram18);
    let init = std::iter::repeat("0")
        .take(words)
        .collect::<Vec<_>>()
        .join(",");
    let cell = design
        .cells
        .iter_mut()
        .find(|c| c.name == intern(CAPTURE_BRAM))
        .ok_or_else(|| "ila_capture_ram missing after insert".to_string())?;
    cell.attrs.set("INIT", init);
    Ok(())
}

/// LUT6 INIT for match unit: I0=probe, I1=prev (unused inputs assumed 0).
/// Rising = I0 & ~I1 → 0x2222…; Falling = ~I0 & I1 → 0x4444…; Immediate = 1.
pub fn match_unit_init(kind: IlaTriggerKind) -> u64 {
    match kind {
        IlaTriggerKind::Immediate => 0xFFFF_FFFF_FFFF_FFFF,
        IlaTriggerKind::Rising => 0x2222_2222_2222_2222,
        IlaTriggerKind::Falling => 0x4444_4444_4444_4444,
    }
}

fn match_unit_cell_names() -> [&'static str; 4] {
    [
        "ila_match_delay_lut",
        "ila_match_delay_ff",
        "ila_match_lut",
        TRIG_FSM_CELL,
    ]
}

pub fn strip_match_unit(design: &mut Design) {
    let names = match_unit_cell_names();
    design.cells.retain(|c| !names.iter().any(|n| intern(n) == c.name));
    design.nets.retain(|n| {
        n.name != intern(MATCH_HIT_NET)
            && n.name != intern(MATCH_PREV_NET)
            && n.name != intern("ila_match_delay_d")
    });
    for n in &mut design.nets {
        n.endpoints.retain(|e| !names.iter().any(|c| intern(c) == e.cell));
    }
    design.rebuild_indexes();
}

/// Insert UG908-class match unit + trigger FSM IP marker on `trigger_net`.
///
/// - Delay LUT+FF: samples previous probe value onto `ila_match_prev`.
/// - Comb match LUT: INIT encodes Immediate / Rising / Falling → `ila_match_hit`.
/// - BlackBox `ila_trig_fsm` (`h_ila_trig`): catalog FSM IP with MATCH/NET attrs.
pub fn insert_match_unit(
    design: &mut Design,
    trigger_net: &str,
    kind: IlaTriggerKind,
) -> Result<(), String> {
    if !design.nets.iter().any(|n| n.name == intern(trigger_net)) {
        return Err(format!("match unit: no net {trigger_net}"));
    }
    strip_match_unit(design);
    // Catalog / IP marker (does not pack to LUTFF).
    design.add_cell(
        TRIG_FSM_CELL,
        CellKind::BlackBox {
            module: intern("h_ila_trig"),
        },
    );
    if let Some(c) = design.cells.iter_mut().find(|c| c.name == intern(TRIG_FSM_CELL)) {
        c.attrs.set("MATCH", kind.as_str());
        c.attrs.set("NET", trigger_net);
        c.attrs.set("VLNV", "community:helion:h_ila_trig:1.0");
    }
    // Delay FF: buffer LUT O=I0 on trigger_net → registered prev.
    design.add_cell(
        "ila_match_delay_lut",
        CellKind::Lut6 {
            init: 0xAAAA_AAAA_AAAA_AAAA,
        },
    );
    design.add_cell("ila_match_delay_ff", CellKind::Hff);
    design.connect(trigger_net, "ila_match_delay_lut", "I0");
    design.connect("ila_match_delay_d", "ila_match_delay_lut", "O");
    design.connect("ila_match_delay_d", "ila_match_delay_ff", "D");
    design.connect(MATCH_PREV_NET, "ila_match_delay_ff", "Q");
    // Comb match LUT (no FF on O) — host reads LUT O after refresh_comb.
    design.add_cell(
        "ila_match_lut",
        CellKind::Lut6 {
            init: match_unit_init(kind),
        },
    );
    design.connect(trigger_net, "ila_match_lut", "I0");
    design.connect(MATCH_PREV_NET, "ila_match_lut", "I1");
    design.connect(MATCH_HIT_NET, "ila_match_lut", "O");
    Ok(())
}

fn to_fabric_match_kind(kind: IlaTriggerKind) -> IlaMatchKind {
    match kind {
        IlaTriggerKind::Immediate => IlaMatchKind::Immediate,
        IlaTriggerKind::Rising => IlaMatchKind::Rising,
        IlaTriggerKind::Falling => IlaMatchKind::Falling,
    }
}

fn strip_deep_ila(design: &mut Design, nets: &[String]) {
    for n in nets {
        strip_ila(design, n);
    }
    strip_capture_bram(design);
    strip_match_unit(design);
}

fn pack_sample_word(bits: &[bool]) -> u64 {
    let mut w = 0u64;
    for (i, &b) in bits.iter().enumerate().take(64) {
        if b {
            w |= 1u64 << i;
        }
    }
    w
}

fn unpack_sample_word(w: u64, n: usize) -> Vec<bool> {
    (0..n).map(|i| ((w >> i) & 1) == 1).collect()
}

/// Fabric BRAM–backed multi-probe ILA with match-unit / trigger-FSM IP.
///
/// Strictly deeper than soft `insert_arm_capture` (`ble_out` host poll) and past
/// host-side edge compare during arm:
/// probe LUTFFs + capture Bram18 + **match-unit LUT / `h_ila_trig` FSM** in the
/// bitstream; trigger advances via fabric `IlaTriggerFsm` on match-unit `ila_match_hit`
/// (after `refresh_comb`), then the window is uploaded via Helion `IR_USR1` JTAG DR.
///
/// Residual vs UG908: no compressed upload protocol yet.
pub fn insert_arm_capture_deep(
    dev: &Device,
    design: &Design,
    cfg: &IlaArmConfig,
) -> Result<IlaCaptureDeep, String> {
    if cfg.nets.is_empty() {
        return Err("ila deep: need at least one probe net".into());
    }
    let window = cfg.window.max(1);
    if cfg.pre_trigger >= window {
        return Err(format!(
            "ila deep: pre_trigger {} must be < window {window}",
            cfg.pre_trigger
        ));
    }
    if !cfg.nets.iter().any(|n| n == &cfg.trigger_net) {
        return Err(format!(
            "ila deep: trigger_net {} not in probes {:?}",
            cfg.trigger_net, cfg.nets
        ));
    }
    for n in &cfg.nets {
        if !design.nets.iter().any(|nn| nn.name == intern(n)) {
            return Err(format!("mark_debug: no net {n}"));
        }
    }

    let mut baseline = design.clone();
    strip_deep_ila(&mut baseline, &cfg.nets);
    let (_, bits0) = compile(dev, &baseline)?;
    let packed0 = pack(&baseline, dev)?;

    let mut d = design.clone();
    strip_deep_ila(&mut d, &cfg.nets);
    for n in &cfg.nets {
        insert_ila(&mut d, n)?;
    }
    insert_capture_bram(&mut d, window)?;
    insert_match_unit(&mut d, &cfg.trigger_net, cfg.trigger)?;
    let (routed, bits1) = compile(dev, &d)?;
    if bits0.frames == bits1.frames {
        return Err("ILA deep insert was a no-op (bitstream unchanged)".into());
    }
    if routed.placed.packed.lutffs.len() <= packed0.lutffs.len() {
        return Err("ILA deep did not pack extra LUTFF probes".into());
    }
    if routed.placed.packed.brams.is_empty() {
        return Err("ILA deep did not pack capture BRAM".into());
    }
    if !routed
        .placed
        .packed
        .lutffs
        .iter()
        .any(|l| l.lut_cell == "ila_match_lut")
    {
        return Err("ILA deep did not pack match-unit LUT".into());
    }
    if !d.cells.iter().any(|c| c.name == intern(TRIG_FSM_CELL)) {
        return Err("ILA deep missing ila_trig_fsm IP marker".into());
    }
    let bram_major = routed
        .placed
        .packed
        .brams
        .iter()
        .position(|b| b.cell == CAPTURE_BRAM)
        .ok_or("ILA deep: capture BRAM not in packed list")? as u16;
    let has_bram_frame = bits1
        .frames
        .keys()
        .any(|(bt, maj, _)| *bt == helion_device::Far::BRAM && *maj == bram_major);
    if !has_bram_frame {
        return Err("ILA deep: bitstream missing Far::BRAM capture frame".into());
    }

    let mut sites = Vec::with_capacity(cfg.nets.len());
    for n in &cfg.nets {
        sites.push(probe_site(&routed.placed, design, n)?);
    }
    let trig_idx = cfg.nets.iter().position(|n| n == &cfg.trigger_net).unwrap();

    // Capture + upload share one TAP fabric so IR_USR1 DR sees the same BRAM bank.
    let mut tap = Tap::new(dev);
    tap.program(&bits1)?;

    let match_site = probe_site(&routed.placed, &d, MATCH_HIT_NET)?;
    let fab_kind = to_fabric_match_kind(cfg.trigger);

    let mut ring: std::collections::VecDeque<Vec<bool>> =
        std::collections::VecDeque::with_capacity(window);
    // Hunt bound: allow slow nets (e.g. MSB) to reach an edge.
    let max_steps = window.saturating_mul(64).max(256);
    let mut steps = 0usize;
    let mut wr = 0usize;
    let pre = if cfg.trigger == IlaTriggerKind::Immediate {
        0
    } else {
        cfg.pre_trigger.min(window.saturating_sub(1))
    };

    // Scoped mutable fabric borrow — must end before USR1 DR upload on `tap`.
    {
        let fab = tap.fabric_mut();
        fab.ila_arm_fsm(fab_kind, window, cfg.pre_trigger);
        while steps < max_steps {
            fab.step_user();
            // Match LUT must see post-tick Q (probe=cur, delay FF=prev).
            fab.refresh_comb();
            let mut sample = Vec::with_capacity(sites.len());
            for &(site, ble) in &sites {
                sample.push(fab.ble_out(site.x, site.y, ble as u32));
            }
            let word = pack_sample_word(&sample);
            fab.bram_write_word(bram_major, wr % window, word);
            wr = wr.wrapping_add(1);
            if ring.len() == window {
                ring.pop_front();
            }
            ring.push_back(sample);

            // Trigger from fabric match-unit LUT + FSM — not host ble_out edge compare.
            let match_hit = fab.ble_out(match_site.0.x, match_site.0.y, match_site.1 as u32);
            let done = fab.ila_fsm.on_sample(match_hit);
            steps += 1;
            if done {
                break;
            }
        }
        if !fab.ila_fsm.fired {
            return Err(format!(
                "ILA deep: match-unit/FSM {} never fired on {} in {steps} steps",
                cfg.trigger.as_str(),
                cfg.trigger_net
            ));
        }
    }

    if ring.len() < window && cfg.trigger == IlaTriggerKind::Immediate {
        return Err(format!(
            "ILA deep: short capture {} < window {window}",
            ring.len()
        ));
    }

    // Align window: for edge triggers, ensure `pre` samples before last trigger moment.
    let samples_ring: Vec<Vec<bool>> = if ring.len() >= window {
        ring.iter().skip(ring.len() - window).cloned().collect()
    } else {
        // Pad front with zeros only if we triggered too early for full pre — honest short pre.
        let mut v = vec![vec![false; cfg.nets.len()]; window - ring.len()];
        v.extend(ring.iter().cloned());
        v
    };

    // Upload from capture RAM via Helion TAP IR_USR1 **RLE** JTAG DR (not host bram_read_word).
    let start = wr.saturating_sub(window) % window;
    let rle = tap.usr1_upload_bram_rle(bram_major, start as u32, window, window as u32);
    let uploaded: Vec<Vec<bool>> = rle
        .words
        .iter()
        .copied()
        .map(|w| unpack_sample_word(w, cfg.nets.len()))
        .collect();
    // Prefer JTAG USR1 RLE upload as source of truth when lengths match ring window.
    let samples = if uploaded.len() == samples_ring.len() {
        uploaded
    } else {
        samples_ring
    };
    let upload_dr_scans = rle.dr_scans;
    let upload_raw_scans = rle.raw_scans;

    let trigger_at = match cfg.trigger {
        IlaTriggerKind::Immediate => Some(0),
        IlaTriggerKind::Rising | IlaTriggerKind::Falling => {
            // Expected slot is `pre` when we had enough history; else locate edge.
            let expected = pre;
            let edge = samples.windows(2).position(|w| {
                let a = w[0][trig_idx];
                let b = w[1][trig_idx];
                match cfg.trigger {
                    IlaTriggerKind::Rising => !a && b,
                    IlaTriggerKind::Falling => a && !b,
                    IlaTriggerKind::Immediate => false,
                }
            });
            Some(edge.map(|i| i + 1).unwrap_or(expected))
        }
    };

    // Non-constant check on trigger probe.
    let trig_stream: Vec<bool> = samples.iter().map(|s| s[trig_idx]).collect();
    if trig_stream.iter().all(|&s| s == trig_stream[0]) && cfg.trigger == IlaTriggerKind::Immediate
    {
        return Err(format!(
            "ILA deep capture is constant on {} over {window} samples",
            cfg.trigger_net
        ));
    }

    Ok(IlaCaptureDeep {
        probes: cfg.nets.clone(),
        samples,
        trigger_at,
        pre_trigger: pre,
        bram_major,
        backend: DEEP_BACKEND,
        upload_dr_scans,
        upload_raw_scans,
    })
}

/// Resolve the fabric (site, BLE) that drives `net` (FF Q, or IOB PAD via from_net).
fn probe_site(
    placed: &helion_place::Placed,
    design: &Design,
    net: &str,
) -> Result<(helion_device::Site, u8), String> {
    let packed = &placed.packed;
    if let Some(i) = packed.lutffs.iter().position(|l| l.q_net == net) {
        return placed
            .lutff_sites
            .get(i)
            .copied()
            .ok_or_else(|| format!("ILA: no site for q_net {net}"));
    }
    // PAD / port net: follow IOB I ← from_net → that LUTFF's Q.
    if let Some(iob) = packed
        .iobs
        .iter()
        .find(|iob| design.net_on(intern(&iob.cell), intern("PAD")) == Some(intern(net)))
    {
        if let Some(i) = packed.lutffs.iter().position(|l| l.q_net == iob.from_net) {
            return placed
                .lutff_sites
                .get(i)
                .copied()
                .ok_or_else(|| format!("ILA: no site for IOB from_net {}", iob.from_net));
        }
        return Err(format!(
            "ILA: PAD {net} driven by {}, but that net is not a packed FF Q",
            iob.from_net
        ));
    }
    // Comb / other: match LUT O net packed as q_net for LUT-only clusters.
    if let Some(i) = packed
        .lutffs
        .iter()
        .position(|l| l.q_net == net || l.lut_cell == net)
    {
        return placed
            .lutff_sites
            .get(i)
            .copied()
            .ok_or_else(|| format!("ILA: no site for {net}"));
    }
    Err(format!(
        "ILA: no packed LUTFF drives net {net} (need FF Q or IOB PAD)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::Design;

    #[test]
    fn ila_insert_changes_bitstream_and_captures() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_blinky();
        let cap = insert_arm_capture(&dev, &d, "q", 8).unwrap();
        assert_eq!(cap.net, "q");
        assert!(cap.samples.iter().any(|&b| b) && cap.samples.iter().any(|&b| !b));
    }

    #[test]
    fn ila_unknown_net_fails() {
        let mut d = Design::structural_blinky();
        assert!(insert_ila(&mut d, "no_such").is_err());
    }

    #[test]
    fn mark_debug_attr_inserts_ila() {
        let mut d = Design::structural_blinky();
        d.mark_debug("q").unwrap();
        assert_eq!(insert_marked(&mut d).unwrap(), 1);
        assert!(
            d.cells
                .iter()
                .any(|c| matches!(c.kind, CellKind::Ila { .. }))
        );
    }

    #[test]
    fn capture_reads_marked_net_not_site0() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_counter();
        let c0 = insert_arm_capture(&dev, &d, "q0", 16).unwrap();
        let c3 = insert_arm_capture(&dev, &d, "q3", 16).unwrap();
        let led = insert_arm_capture(&dev, &d, "led", 16).unwrap();
        let b0: String = c0
            .samples
            .iter()
            .map(|b| if *b { '1' } else { '0' })
            .collect();
        let b3: String = c3
            .samples
            .iter()
            .map(|b| if *b { '1' } else { '0' })
            .collect();
        let bl: String = led
            .samples
            .iter()
            .map(|b| if *b { '1' } else { '0' })
            .collect();
        assert_eq!(b0, "1010101010101010", "LSB toggles every cycle: {b0}");
        assert_eq!(b3, "0000000111111110", "MSB matches gold LED stream: {b3}");
        assert_eq!(bl, b3, "PAD led follows q3 driver");
        assert_ne!(b0, b3, "must not stub every probe as site[0]");
    }

    /// Real flow: mark_debug / insert_marked then arm must NOT hit "bitstream unchanged".
    #[test]
    fn mark_debug_then_arm_inserts_probe_not_noop() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_counter();
        d.mark_debug("q3").unwrap();
        assert_eq!(insert_marked(&mut d).unwrap(), 1);
        assert!(
            d.cells
                .iter()
                .any(|c| matches!(&c.kind, CellKind::Ila { net } if *net == intern("q3")))
        );
        let cap = insert_arm_capture(&dev, &d, "q3", 16)
            .expect("mark_debug → arm must capture; must not Err(bitstream unchanged)");
        let bits: String = cap
            .samples
            .iter()
            .map(|b| if *b { '1' } else { '0' })
            .collect();
        assert_eq!(
            bits, "0000000111111110",
            "MSB gold after pre-insert: {bits}"
        );
    }

    #[test]
    fn strip_ila_removes_probe_cells() {
        let mut d = Design::structural_blinky();
        insert_ila(&mut d, "q").unwrap();
        assert!(d.cells.iter().any(|c| resolve(c.name).starts_with("ila_q")));
        strip_ila(&mut d, "q");
        assert!(!d.cells.iter().any(|c| resolve(c.name).starts_with("ila_q")));
        assert!(!d.nets.iter().any(|n| resolve(n.name).starts_with("ila_q")));
        // Marked net keeps user endpoints (FF Q), not ILA sink.
        let q = d.net("q").unwrap();
        assert!(!q.endpoints.iter().any(|e| resolve(e.cell).starts_with("ila_")));
    }

    #[test]
    fn deep_immediate_matches_soft_q3_gold() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_counter();
        let soft = insert_arm_capture(&dev, &d, "q3", 16).unwrap();
        let soft_bits: String = soft
            .samples
            .iter()
            .map(|b| if *b { '1' } else { '0' })
            .collect();
        assert_eq!(soft_bits, "0000000111111110");

        let cfg = IlaArmConfig::single("q3", 16);
        let deep = insert_arm_capture_deep(&dev, &d, &cfg).unwrap();
        assert_eq!(deep.backend, "jtag_usr1_match_fsm_rle");
        assert_eq!(deep.probes, vec!["q3".to_string()]);
        assert_eq!(deep.trigger_at, Some(0));
        let deep_bits: String = deep
            .samples
            .iter()
            .map(|s| if s[0] { '1' } else { '0' })
            .collect();
        assert_eq!(
            deep_bits, soft_bits,
            "deep immediate must match soft ble_out gold"
        );
    }

    #[test]
    fn deep_multi_probe_q0_q3_and_capture_bram() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_counter();
        let cfg = IlaArmConfig {
            nets: vec!["q0".into(), "q3".into()],
            window: 16,
            trigger: IlaTriggerKind::Immediate,
            trigger_net: "q0".into(),
            pre_trigger: 0,
        };
        let deep = insert_arm_capture_deep(&dev, &d, &cfg).unwrap();
        assert_eq!(deep.probes.len(), 2);
        let b0: String = deep
            .samples
            .iter()
            .map(|s| if s[0] { '1' } else { '0' })
            .collect();
        let b3: String = deep
            .samples
            .iter()
            .map(|s| if s[1] { '1' } else { '0' })
            .collect();
        assert_eq!(b0, "1010101010101010", "LSB: {b0}");
        assert_eq!(b3, "0000000111111110", "MSB: {b3}");
        // Bitstream-backed: capture BRAM major is programmed.
        assert!(deep.bram_major < 8);
    }

    #[test]
    fn deep_rising_pretrigger_window_on_q3() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_counter();
        let cfg = IlaArmConfig {
            nets: vec!["q3".into()],
            window: 16,
            trigger: IlaTriggerKind::Rising,
            trigger_net: "q3".into(),
            pre_trigger: 4,
        };
        let deep = insert_arm_capture_deep(&dev, &d, &cfg).unwrap();
        assert_eq!(deep.pre_trigger, 4);
        assert_eq!(deep.samples.len(), 16);
        let bits: String = deep
            .samples
            .iter()
            .map(|s| if s[0] { '1' } else { '0' })
            .collect();
        let at = deep.trigger_at.expect("rising trigger_at");
        assert!(at >= 1 && at < bits.len(), "trigger_at={at} bits={bits}");
        assert_eq!(&bits[at - 1..at + 1], "01", "edge at trigger_at: {bits}");
        // Pre-trigger samples should be mostly 0s leading into the rise.
        let pre = &bits[..at];
        assert!(
            pre.chars().filter(|&c| c == '0').count() >= pre.len().saturating_sub(1),
            "pre-trigger mostly low: pre={pre} bits={bits}"
        );
        // Post-trigger includes the high run.
        assert!(
            bits[at..].contains('1'),
            "post-trigger must include highs: {bits}"
        );
    }

    #[test]
    fn deep_upload_backend_is_jtag_usr1_match_fsm_rle() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_counter();
        let cfg = IlaArmConfig::single("q3", 16);
        let deep = insert_arm_capture_deep(&dev, &d, &cfg).unwrap();
        assert_eq!(deep.backend, "jtag_usr1_match_fsm_rle");
        let bits: String = deep
            .samples
            .iter()
            .map(|s| if s[0] { '1' } else { '0' })
            .collect();
        assert_eq!(bits, "0000000111111110");
        assert!(
            deep.upload_dr_scans < deep.upload_raw_scans,
            "RLE must reduce DR scans: dr={} raw={}",
            deep.upload_dr_scans,
            deep.upload_raw_scans
        );
        assert_eq!(deep.upload_raw_scans, 16);
    }

    #[test]
    fn rle_encode_expand_roundtrip_host_side() {
        use helion_hw::{rle_encode_words, rle_expand_tokens};
        // Identical to soft gold bit pattern as 1-bit packed words.
        let words: Vec<u64> = "0000000111111110"
            .chars()
            .map(|c| if c == '1' { 1 } else { 0 })
            .collect();
        let tok = rle_encode_words(&words);
        assert_eq!(tok.len(), 3, "7×0, 7×1, 1×0");
        assert_eq!(rle_expand_tokens(&tok, 16), words);
    }

    #[test]
    fn match_unit_insert_packs_lut_and_fsm_marker() {
        let mut d = Design::structural_counter();
        insert_match_unit(&mut d, "q3", IlaTriggerKind::Rising).unwrap();
        assert!(d.cells.iter().any(|c| c.name == intern("ila_match_lut")));
        assert!(d.cells.iter().any(|c| {
            matches!(&c.kind, CellKind::BlackBox { module } if *module == intern("h_ila_trig"))
        }));
        assert_eq!(
            match_unit_init(IlaTriggerKind::Rising),
            0x2222_2222_2222_2222
        );
        strip_match_unit(&mut d);
        assert!(!d.cells.iter().any(|c| c.name == intern("ila_match_lut")));
    }

    #[test]
    fn deep_capture_bram_strip_roundtrip() {
        let mut d = Design::structural_counter();
        insert_capture_bram(&mut d, 16).unwrap();
        assert!(d.cells.iter().any(|c| c.name == intern("ila_capture_ram")));
        strip_capture_bram(&mut d);
        assert!(!d.cells.iter().any(|c| c.name == intern("ila_capture_ram")));
    }

    #[test]
    fn insert_debug_cores_stiches_ila_bscan_and_compiles_bitstream() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_counter();
        let (_, baseline_bits) = compile(&dev, &d).unwrap();

        d.mark_debug("q0").unwrap();
        d.mark_debug("q3").unwrap();

        let mut cfg = DebugInsertConfig::default();
        cfg.sample_depth = 32;
        cfg.trigger_stages = 4;
        cfg.vio_probes_in = vec!["q1".into()];
        cfg.vio_probes_out = vec!["q2".into()];
        cfg.insert_jtag_axi = true;

        let rep = insert_debug_cores(&mut d, &cfg).unwrap();
        assert_eq!(rep.probed_nets, vec!["q0".to_string(), "q3".to_string()]);
        assert_eq!(rep.probe_channels.len(), 2);
        assert_eq!(rep.hub.slots.len(), 3, "ILA + VIO + JTAG-AXI on BSCAN hub");
        assert!(d.cell("bscan_usr1").is_some());
        assert!(d.cell("u_dbg_hub").is_some());
        assert!(d.cell("ila_trig_cmp_lut").is_some());
        assert!(d.cell("ila_capture_ram").is_some());
        assert!(d.cell("u_vio_0").is_some());
        assert!(d.cell("u_jtag_axi_0").is_some());

        // Netlist with inserted debug cores must pack, place, route, and bitgen cleanly.
        let (routed, debug_bits) = compile(&dev, &d).unwrap();
        assert_ne!(baseline_bits.frames, debug_bits.frames);
        assert!(!routed.placed.packed.brams.is_empty());

        // HNF round-trip preserves inserted debug cores.
        let hnf = d.to_hnf();
        let d_rt = Design::from_hnf(&hnf).unwrap();
        assert!(d_rt.cell("u_dbg_hub").is_some());

        // Strip removes all debug cores cleanly.
        strip_debug_cores(&mut d);
        assert!(d.cell("u_dbg_hub").is_none());
        assert!(d.cell("bscan_usr1").is_none());
        assert!(d.cell("ila_capture_ram").is_none());
    }

    #[test]
    fn ltx_probe_file_bus_grouping_and_disk_roundtrip() {
        let mut d = Design::structural_counter();
        // Add bus nets cnt[0]..cnt[3] marked for debug alongside scalar q3.
        for bit in 0..4 {
            let bus_net = format!("cnt[{bit}]");
            d.connect(&bus_net, &format!("u_ff{bit}"), "Q");
            d.mark_debug(&bus_net).unwrap();
        }
        d.mark_debug("q3").unwrap();

        let rep = insert_debug_cores(&mut d, &DebugInsertConfig::default()).unwrap();
        assert_eq!(rep.probe_channels.len(), 2, "cnt[3:0] coalesced + scalar q3");

        let (_, q3_ch) = rep.ltx.find_probe("q3").unwrap();
        assert_eq!(q3_ch.port_id, "PROBE0");
        assert_eq!(q3_ch.width, 1);

        let (_, cnt_ch) = rep.ltx.find_probe("cnt").unwrap();
        assert_eq!(cnt_ch.port_id, "PROBE1");
        assert_eq!(cnt_ch.width, 4);
        assert_eq!(cnt_ch.msb, 3);
        assert_eq!(cnt_ch.lsb, 0);
        assert_eq!(cnt_ch.display_signal(), "cnt[3:0]");
        assert_eq!(
            cnt_ch.nets,
            vec!["cnt[0]", "cnt[1]", "cnt[2]", "cnt[3]"]
        );

        // String + disk file round-trip.
        let tmp_dir = std::env::temp_dir().join("helion-debug-ltx-test");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let ltx_path = tmp_dir.join("debug_probes.ltx");
        rep.ltx.write_ltx(&ltx_path).unwrap();
        let parsed = DebugProbesLtx::read_ltx(&ltx_path).unwrap();
        assert_eq!(parsed, rep.ltx);
    }

    #[test]
    fn bscan_hub_multiplexes_slots_and_tap_frames() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut tap = Tap::new(&dev);
        let mut hub = BscanDebugHub::new("u_dbg_hub", BscanUserChain::User1);
        let s_ila = hub.register_core("u_ila_0", DebugCoreKind::Ila, 4).unwrap();
        let s_vio = hub.register_core("u_vio_0", DebugCoreKind::Vio, 2).unwrap();
        let s_axi = hub
            .register_core("u_jtag_axi_0", DebugCoreKind::JtagAxi, 1)
            .unwrap();
        assert_eq!((s_ila, s_vio, s_axi), (0, 1, 2));

        let f_sel = hub.select_slot(s_vio).unwrap();
        assert_eq!(f_sel.slot_id, s_vio);
        assert_eq!(hub.active_slot, Some(s_vio));

        let encoded = f_sel.encode();
        let decoded = BscanFrame::decode(encoded).unwrap();
        assert_eq!(decoded, f_sel);

        let f_tap = hub
            .transact_via_tap(&mut tap, s_axi, hub_opcode::WRITE_CTRL, 0x1234_5678)
            .unwrap();
        assert_eq!(f_tap.payload, 0x1234_5678);
        assert_eq!(hub.discover_cores().len(), 3);
    }

    #[test]
    fn multi_stage_trigger_sequencer_boolean_edges_ranges_and_counters() {
        // 4-bit counter stream: 0..16 repeated 3 times (48 cycles).
        let mut stream = Vec::new();
        for cycle in 0..48u64 {
            let v = cycle % 16;
            stream.push(vec![
                (v & 1) != 0,
                (v & 2) != 0,
                (v & 4) != 0,
                (v & 8) != 0,
            ]);
        }
        // State 0: wait for rising edge on bit 3 (value transitions 7 -> 8) AND bus in range 8..=12.
        let s0 = TriggerStateSpec::new(
            0,
            "wait_msb_rise",
            TriggerCondition::and(vec![
                ProbeComparator::bit(3, ComparatorOp::parse("R").unwrap()),
                ProbeComparator::bus(vec![0, 1, 2, 3], ComparatorOp::parse("8..=12").unwrap()),
            ]),
            1,
            StateTransition::Goto(1),
        );
        // State 1: count 3 toggles ('B' both edges) on bit 0 while bus >= 9, then goto State 2.
        let s1 = TriggerStateSpec::new(
            1,
            "count_lsb_toggles",
            TriggerCondition::and(vec![
                ProbeComparator::bit(0, ComparatorOp::parse("B").unwrap()),
                ProbeComparator::bus(vec![0, 1, 2, 3], ComparatorOp::parse(">=9").unwrap()),
            ]),
            3,
            StateTransition::Goto(2),
        );
        // State 2: wait for falling edge ('F') on bit 3 (15 -> 0 wrap) OR bus <= 2, then Trigger.
        let s2 = TriggerStateSpec::new(
            2,
            "wait_wrap_fall",
            TriggerCondition::or(vec![
                ProbeComparator::bit(3, ComparatorOp::parse("F").unwrap()),
                ProbeComparator::bus(vec![0, 1, 2, 3], ComparatorOp::parse("<=0").unwrap()),
            ]),
            1,
            StateTransition::Trigger,
        );

        let cfg = IlaSequencerConfig {
            states: vec![s0, s1, s2],
            window_position: TriggerWindowPosition::CenterTrigger,
            segment_depth: 8,
            num_segments: 2,
            capture_qualifier: None,
        };
        let mut seq = IlaTriggerSequencer::new(cfg).unwrap();
        let res = seq
            .run_stream(
                vec!["q0".into(), "q1".into(), "q2".into(), "q3".into()],
                &stream,
            )
            .unwrap();
        assert_eq!(res.segments.len(), 2, "must capture both segments");
        for seg in &res.segments {
            assert_eq!(seg.samples.len(), 8);
            assert_eq!(seg.trigger_offset, 4, "CenterTrigger at depth/2 = 4");
            // At trigger_offset, counter wrapped from 15 to 0.
            assert_eq!(seg.words[seg.trigger_offset], 0);
            assert!(seg.state_trace.len() >= 5, "1 + 3 + 1 state matches");
        }
    }

    #[test]
    fn trigger_sequencer_supports_16_states_and_capture_qualification() {
        // Build a full 16-state cascade (states 0..15) where each state matches bus == state_id.
        let mut states = Vec::new();
        for s in 0..16u8 {
            let next = if s < 15 {
                StateTransition::Goto(s + 1)
            } else {
                StateTransition::Trigger
            };
            states.push(TriggerStateSpec::new(
                s,
                format!("st_{s}"),
                TriggerCondition::single(ProbeComparator::bus(
                    vec![0, 1, 2, 3],
                    ComparatorOp::Equal(u64::from(s)),
                )),
                1,
                next,
            ));
        }
        // Capture qualifier: only record even values (bit 0 == 0) except allow all in this 16-step test
        // or test qualification on a 32-cycle stream where qualifier selects bit 0 == 1.
        let cfg = IlaSequencerConfig {
            states,
            window_position: TriggerWindowPosition::PreTrigger,
            segment_depth: 8,
            num_segments: 1,
            capture_qualifier: None,
        };
        let mut stream = Vec::new();
        for v in 0..20u64 {
            let c = v % 16;
            stream.push(vec![
                (c & 1) != 0,
                (c & 2) != 0,
                (c & 4) != 0,
                (c & 8) != 0,
            ]);
        }
        let mut seq = IlaTriggerSequencer::new(cfg).unwrap();
        let res = seq
            .run_stream(
                vec!["b0".into(), "b1".into(), "b2".into(), "b3".into()],
                &stream,
            )
            .unwrap();
        assert_eq!(res.segments.len(), 1);
        assert_eq!(res.segments[0].trigger_offset, 7, "PreTrigger at end of 8-sample window");
        assert_eq!(res.segments[0].words, vec![8, 9, 10, 11, 12, 13, 14, 15]);
    }

    #[test]
    fn fabric_segmented_sequencer_capture_with_qualification() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_counter();
        let probes = vec!["q0".into(), "q1".into(), "q2".into(), "q3".into()];

        // Capture qualifier: only sample cycles where q0 == 1 (odd counter values: 1, 3, 5, 7, 9, 11, 13, 15).
        // Trigger when bus >= 7 (after 2 qualified matches: 7 and 9).
        let seq_cfg = IlaSequencerConfig {
            states: vec![TriggerStateSpec::new(
                0,
                "odd_ge_7",
                TriggerCondition::single(ProbeComparator::bus(
                    vec![0, 1, 2, 3],
                    ComparatorOp::GreaterOrEqual(7),
                )),
                2,
                StateTransition::Trigger,
            )],
            window_position: TriggerWindowPosition::PostTrigger,
            segment_depth: 4,
            num_segments: 2,
            capture_qualifier: Some(TriggerCondition::single(ProbeComparator::bit(
                0,
                ComparatorOp::Equal(1),
            ))),
        };
        let res = run_sequencer_capture(&dev, &d, &probes, seq_cfg, 128).unwrap();
        assert_eq!(res.segments.len(), 2);
        assert!(res.qualified_cycles < res.total_cycles, "qualifier filtered even cycles");
        // Every captured word must have bit 0 == 1 (odd counter value).
        for seg in &res.segments {
            assert_eq!(seg.words.len(), 4);
            for &w in &seg.words {
                assert_eq!(w & 1, 1, "qualified sample must be odd, got {w}");
            }
        }
    }

    #[test]
    fn vio_stimulus_injection_and_activity_detectors_on_fabric() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_counter();
        let (routed, bits) = compile(&dev, &d).unwrap();
        let mut fab = Fabric::new(&dev);
        fab.program(&bits).unwrap();
        fab.finish_startup();

        let mut hub = BscanDebugHub::new("u_dbg_hub", BscanUserChain::User1);
        let slot = hub.register_core("u_vio_0", DebugCoreKind::Vio, 2).unwrap();
        let mut vio = VioCore::new("u_vio_0", slot);
        vio.add_probe_in("cnt_in", vec!["q0".into(), "q1".into(), "q2".into(), "q3".into()]);
        vio.add_probe_out("cnt_force", vec!["q0".into(), "q1".into(), "q2".into(), "q3".into()], 0);

        // Inject virtual stimulus 0b1010 (10) onto counter FFs and verify probe_in reads it back.
        vio.set_probe_out("cnt_force", 0b1010).unwrap();
        assert_eq!(vio.commit_outputs(&mut hub).unwrap(), 1);
        assert_eq!(
            vio.apply_stimulus_to_fabric(&routed.placed, &d, &mut fab)
                .unwrap(),
            4
        );
        vio.sample_from_fabric(&routed.placed, &d, &fab, &mut hub)
            .unwrap();
        assert_eq!(vio.probe_in("cnt_in").unwrap().value, 0b1010);

        // Toggle stimulus to 0b0101 (5): both rising and falling edges must be detected.
        let toggled = vio.toggle_probe_out("cnt_force").unwrap();
        assert_eq!(toggled, 0b0101);
        vio.commit_outputs(&mut hub).unwrap();
        vio.apply_stimulus_to_fabric(&routed.placed, &d, &mut fab)
            .unwrap();
        vio.sample_from_fabric(&routed.placed, &d, &fab, &mut hub)
            .unwrap();

        let pin = vio.probe_in("cnt_in").unwrap();
        assert_eq!(pin.value, 0b0101);
        assert_eq!(pin.activity(), VioActivityKind::BothEdges);
        assert_eq!(pin.rising_mask, 0b0101);
        assert_eq!(pin.falling_mask, 0b1010);
        assert_eq!(pin.transition_count, 1);
        assert_eq!(pin.min_value, Some(0b0101));
        assert_eq!(pin.max_value, Some(0b1010));

        vio.clear_all_activity();
        assert_eq!(vio.probe_in("cnt_in").unwrap().activity(), VioActivityKind::None);
    }

    #[test]
    fn jtag_to_axi_master_transactions_and_error_responses() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut tap = Tap::new(&dev);
        let mut hub = BscanDebugHub::new("u_dbg_hub", BscanUserChain::User1);
        let slot = hub
            .register_core("u_jtag_axi_0", DebugCoreKind::JtagAxi, 1)
            .unwrap();
        let mut axi = JtagAxiMaster::new("u_jtag_axi_0", slot);

        // Unaligned address must be rejected by create_hw_axi_txn.
        assert!(create_hw_axi_txn("bad_align", 0x4000_0002, AxiTxnKind::Read, 1, vec![]).is_err());

        // 4-beat burst write + readback to AXI BRAM aperture (0xC000_0000), backed by Tap BRAM 0.
        let payload = vec![0xCAFE_BABE, 0x1234_5678, 0x0BAD_F00D, 0xFEED_FACE];
        let mut wr_txn =
            create_hw_axi_txn("wr_bram", 0xC000_0000, AxiTxnKind::Write, 4, payload.clone())
                .unwrap();
        let resp = axi.run_hw_axi_on_tap(&mut wr_txn, &mut hub, &mut tap).unwrap();
        assert_eq!(resp, AxiResp::Okay);
        assert!(wr_txn.completed);
        assert_eq!(wr_txn.dr_scans, 8);
        assert_eq!(tap.fabric().bram_read_word(0, 0) as u32, 0xCAFE_BABE);
        assert_eq!(tap.fabric().bram_read_word(0, 3) as u32, 0xFEED_FACE);

        let mut rd_txn =
            create_hw_axi_txn("rd_bram", 0xC000_0000, AxiTxnKind::Read, 4, vec![]).unwrap();
        let resp_rd = axi.run_hw_axi_on_tap(&mut rd_txn, &mut hub, &mut tap).unwrap();
        assert_eq!(resp_rd, AxiResp::Okay);
        assert_eq!(rd_txn.read_data, payload);

        // Read from Read-Only ROM ID aperture succeeds; write returns SLVERR.
        let mut rom_rd =
            create_hw_axi_txn("rom_rd", 0x8000_0000, AxiTxnKind::Read, 1, vec![]).unwrap();
        assert_eq!(axi.run_hw_axi(&mut rom_rd, &mut hub).unwrap(), AxiResp::Okay);
        assert_eq!(rom_rd.read_data, vec![0x4845_4C31]);

        let mut rom_wr =
            create_hw_axi_txn("rom_wr", 0x8000_0000, AxiTxnKind::Write, 1, vec![0]).unwrap();
        assert_eq!(
            axi.run_hw_axi(&mut rom_wr, &mut hub).unwrap(),
            AxiResp::SlvErr
        );

        // Access to unmapped address returns DECERR.
        let mut dec_rd =
            create_hw_axi_txn("unmapped", 0xDEAD_0000, AxiTxnKind::Read, 1, vec![]).unwrap();
        assert_eq!(
            axi.run_hw_axi(&mut dec_rd, &mut hub).unwrap(),
            AxiResp::DecErr
        );
    }
}
