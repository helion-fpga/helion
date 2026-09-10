//! ILA: mark net → extra LUTFF in netlist/bitstream → arm → capture.
//!
//! Soft path (`insert_arm_capture`): host `step_user` + `ble_out` readback.
//! Deep path (`insert_arm_capture_deep`): fabric BRAM sample buffer + trigger-before-fill
//! + multi-probe; upload via Helion TAP `IR_USR1` JTAG DR scan of capture RAM.
//! Residual: not full UG908 user-defined trigger FSM IP / match units / compressed upload.

use helion_bits::bitgen;
use helion_device::Device;
use helion_fabric::Fabric;
use helion_hw::Tap;
use helion_ir::{CellKind, Design};
use helion_pack::pack;
use helion_place::place;
use helion_route::route;

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
}

const CAPTURE_BRAM: &str = "ila_capture_ram";
const DEEP_BACKEND: &str = "jtag_usr1_bram_dr";

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
    design.cells.retain(|c| !cells.iter().any(|n| n == &c.name));
    design.nets.retain(|n| !aux.iter().any(|a| a == &n.name));
    for n in &mut design.nets {
        n.endpoints
            .retain(|e| !cells.iter().any(|c| c == &e.cell));
    }
}

/// Insert an ILA probe on `net`: identity LUT+FF so the bitstream gains BLE1 INIT.
pub fn insert_ila(design: &mut Design, net: &str) -> Result<(), String> {
    if !design.nets.iter().any(|n| n.name == net) {
        return Err(format!("mark_debug: no net {net}"));
    }
    if design
        .cells
        .iter()
        .any(|c| matches!(&c.kind, CellKind::Ila { net: n } if n == net))
    {
        return Ok(());
    }
    design.add_cell(
        format!("ila_{net}"),
        CellKind::Ila { net: net.into() },
    );
    // Buffer LUT: O = I0 → INIT 0xAAAA… so capture tracks the marked net.
    design.add_cell(
        format!("ila_{net}_lut"),
        CellKind::Lut6 {
            init: 0xAAAA_AAAA_AAAA_AAAA,
        },
    );
    design.add_cell(format!("ila_{net}_ff"), CellKind::Hff);
    design.connect(net, format!("ila_{net}_lut"), "I0");
    let dnet = format!("ila_{net}_d");
    design.connect(&dnet, format!("ila_{net}_lut"), "O");
    design.connect(&dnet, format!("ila_{net}_ff"), "D");
    design.connect(format!("ila_{net}_q"), format!("ila_{net}_ff"), "Q");
    Ok(())
}

/// Insert an ILA on every net with IR attr `mark_debug`.
pub fn insert_marked(design: &mut Design) -> Result<usize, String> {
    let nets = design.marked_debug_nets();
    for n in &nets {
        insert_ila(design, n)?;
    }
    Ok(nets.len())
}

pub fn compile(dev: &Device, design: &Design) -> Result<(helion_route::Routed, helion_bits::Bitstream), String> {
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
    design.cells.retain(|c| c.name != CAPTURE_BRAM);
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
        .find(|c| c.name == CAPTURE_BRAM)
        .ok_or_else(|| "ila_capture_ram missing after insert".to_string())?;
    cell.attrs.set("INIT", init);
    Ok(())
}

fn strip_deep_ila(design: &mut Design, nets: &[String]) {
    for n in nets {
        strip_ila(design, n);
    }
    strip_capture_bram(design);
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

fn trigger_fires(kind: IlaTriggerKind, prev: Option<bool>, cur: bool, first: bool) -> bool {
    match kind {
        IlaTriggerKind::Immediate => first,
        IlaTriggerKind::Rising => prev == Some(false) && cur,
        IlaTriggerKind::Falling => prev == Some(true) && !cur,
    }
}

/// Fabric BRAM–backed multi-probe ILA with real trigger-before-fill window.
///
/// Strictly deeper than soft `insert_arm_capture` (`ble_out` host poll):
/// probe LUTFFs + capture Bram18 in the bitstream, samples written into the TAP
/// fabric BRAM during the arm, then uploaded via Helion `IR_USR1` JTAG DR scans
/// (`Tap::usr1_upload_bram`) — not a host `bram_read_word` backdoor.
///
/// Residual vs UG908: no user-defined trigger FSM IP / match units, no compressed upload.
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
        if !design.nets.iter().any(|nn| nn.name == *n) {
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
    let trig_idx = cfg
        .nets
        .iter()
        .position(|n| n == &cfg.trigger_net)
        .unwrap();

    // Capture + upload share one TAP fabric so IR_USR1 DR sees the same BRAM bank.
    let mut tap = Tap::new(dev);
    tap.program(&bits1)?;

    let pre = if cfg.trigger == IlaTriggerKind::Immediate {
        0
    } else {
        cfg.pre_trigger
    };
    let post_after_trig = window - pre; // includes trigger sample
    let mut ring: std::collections::VecDeque<Vec<bool>> =
        std::collections::VecDeque::with_capacity(window);
    let mut prev_trig: Option<bool> = None;
    let mut triggered = false;
    let mut post_left = 0usize;
    let mut first = true;
    // Hunt bound: allow slow nets (e.g. MSB) to reach an edge.
    let max_steps = window.saturating_mul(64).max(256);
    let mut steps = 0usize;
    let mut wr = 0usize;

    // Scoped mutable fabric borrow — must end before USR1 DR upload on `tap`.
    {
        let fab = tap.fabric_mut();
        while steps < max_steps {
            fab.step_user();
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
            ring.push_back(sample.clone());

            let cur_trig = sample[trig_idx];
            if !triggered {
                if trigger_fires(cfg.trigger, prev_trig, cur_trig, first) {
                    triggered = true;
                    post_left = post_after_trig.saturating_sub(1);
                    if post_left == 0 && ring.len() >= window.min(pre + 1) {
                        break;
                    }
                }
            } else {
                if post_left == 0 {
                    break;
                }
                post_left -= 1;
                if post_left == 0 {
                    break;
                }
            }
            prev_trig = Some(cur_trig);
            first = false;
            steps += 1;
        }
    }

    if !triggered {
        return Err(format!(
            "ILA deep: trigger {} never fired on {} in {steps} steps",
            cfg.trigger.as_str(),
            cfg.trigger_net
        ));
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

    // Upload from capture RAM via Helion TAP IR_USR1 JTAG DR scans (not host bram_read_word).
    let start = wr.saturating_sub(window) % window;
    let uploaded_words = tap.usr1_upload_bram(bram_major, start as u32, window, window as u32);
    let uploaded: Vec<Vec<bool>> = uploaded_words
        .into_iter()
        .map(|w| unpack_sample_word(w, cfg.nets.len()))
        .collect();
    // Prefer JTAG USR1 upload as source of truth when lengths match ring window.
    let samples = if uploaded.len() == samples_ring.len() {
        uploaded
    } else {
        samples_ring
    };

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
    if let Some(iob) = packed.iobs.iter().find(|iob| {
        design.net_on(&iob.cell, "PAD") == Some(net)
    }) {
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
        assert!(d.cells.iter().any(|c| matches!(c.kind, CellKind::Ila { .. })));
    }

    #[test]
    fn capture_reads_marked_net_not_site0() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_counter();
        let c0 = insert_arm_capture(&dev, &d, "q0", 16).unwrap();
        let c3 = insert_arm_capture(&dev, &d, "q3", 16).unwrap();
        let led = insert_arm_capture(&dev, &d, "led", 16).unwrap();
        let b0: String = c0.samples.iter().map(|b| if *b { '1' } else { '0' }).collect();
        let b3: String = c3.samples.iter().map(|b| if *b { '1' } else { '0' }).collect();
        let bl: String = led.samples.iter().map(|b| if *b { '1' } else { '0' }).collect();
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
                .any(|c| matches!(&c.kind, CellKind::Ila { net } if net == "q3"))
        );
        let cap = insert_arm_capture(&dev, &d, "q3", 16).expect(
            "mark_debug → arm must capture; must not Err(bitstream unchanged)",
        );
        let bits: String = cap.samples.iter().map(|b| if *b { '1' } else { '0' }).collect();
        assert_eq!(bits, "0000000111111110", "MSB gold after pre-insert: {bits}");
    }

    #[test]
    fn strip_ila_removes_probe_cells() {
        let mut d = Design::structural_blinky();
        insert_ila(&mut d, "q").unwrap();
        assert!(d.cells.iter().any(|c| c.name.starts_with("ila_q")));
        strip_ila(&mut d, "q");
        assert!(!d.cells.iter().any(|c| c.name.starts_with("ila_q")));
        assert!(!d.nets.iter().any(|n| n.name.starts_with("ila_q")));
        // Marked net keeps user endpoints (FF Q), not ILA sink.
        let q = d.net("q").unwrap();
        assert!(!q.endpoints.iter().any(|e| e.cell.starts_with("ila_")));
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
        assert_eq!(deep.backend, "jtag_usr1_bram_dr");
        assert_eq!(deep.probes, vec!["q3".to_string()]);
        assert_eq!(deep.trigger_at, Some(0));
        let deep_bits: String = deep.samples.iter().map(|s| if s[0] { '1' } else { '0' }).collect();
        assert_eq!(deep_bits, soft_bits, "deep immediate must match soft ble_out gold");
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
        let b0: String = deep.samples.iter().map(|s| if s[0] { '1' } else { '0' }).collect();
        let b3: String = deep.samples.iter().map(|s| if s[1] { '1' } else { '0' }).collect();
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
        let bits: String = deep.samples.iter().map(|s| if s[0] { '1' } else { '0' }).collect();
        let at = deep.trigger_at.expect("rising trigger_at");
        assert!(
            at >= 1 && at < bits.len(),
            "trigger_at={at} bits={bits}"
        );
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
    fn deep_upload_backend_is_jtag_usr1_dr() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_counter();
        let cfg = IlaArmConfig::single("q3", 16);
        let deep = insert_arm_capture_deep(&dev, &d, &cfg).unwrap();
        assert_eq!(deep.backend, "jtag_usr1_bram_dr");
        let bits: String = deep.samples.iter().map(|s| if s[0] { '1' } else { '0' }).collect();
        assert_eq!(bits, "0000000111111110");
    }

    #[test]
    fn deep_capture_bram_strip_roundtrip() {
        let mut d = Design::structural_counter();
        insert_capture_bram(&mut d, 16).unwrap();
        assert!(d.cells.iter().any(|c| c.name == "ila_capture_ram"));
        strip_capture_bram(&mut d);
        assert!(!d.cells.iter().any(|c| c.name == "ila_capture_ram"));
    }
}
