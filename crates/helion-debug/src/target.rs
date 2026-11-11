//! Live hardware debug target (Vivado Hardware Manager `hw_ila` / `hw_vio` /
//! `hw_axi` objects over one programmed device).
//!
//! - [`implement_debug`] is "Set Up Debug + re-implement": it stitches the
//!   ILA (+ optional VIO / JTAG-to-AXI) cores and the BSCAN hub into a copy of
//!   the netlist with [`insert_debug_cores`], then packs, places, routes and
//!   bitgens it, and checks that every probe, the edge match unit, the value
//!   comparator and the capture BRAM landed in the bitstream.
//! - [`HwDebugTarget`] programs that bitstream into a Helion TAP (the sim
//!   cable) and keeps the fabric alive between operations. Every core access
//!   is a BSCAN hub frame shifted through `IR_USR1` on that TAP; the ILA window
//!   is uploaded from the capture BRAM with the RLE `USR1` protocol.
//!
//! Sim-cable semantics (stated, not hidden):
//! - The fabric advances only when an operation runs user clocks
//!   ([`HwDebugTarget::run_cycles`], an ILA arm, a VIO refresh).
//! - Trigger reprogramming patches the match / comparator LUT INITs live
//!   (the runtime comparator registers of a real ILA).
//! - A committed VIO `PROBE_OUT` overrides the FF that drives its net on every
//!   clock until released (HAD netlists have no free input nets to drive).
//! - JTAG-to-AXI reaches only fabric BRAM apertures: HAD netlists have no AXI
//!   interconnect, so there are no other slaves to address.

use helion_bits::Bitstream;
use helion_device::{Device, Site};
use helion_fabric::{IlaMatchKind, Stat};
use helion_hw::Tap;
use helion_ir::{Design, resolve};
use helion_pack::pack;
use helion_route::Routed;

use crate::bscan::{BscanDebugHub, DebugCoreKind, hub_opcode};
use crate::insert::{
    CMP_HIT_NET, CMP_LUT_CELL, DebugCoreInsertionReport, DebugInsertConfig, insert_debug_cores,
    strip_debug_cores,
};
use crate::ltx::{DebugProbesLtx, LtxProbeChannel, LtxProbeDirection};
use crate::vio_axi::{AxiRegion, AxiResp, HwAxiTxn, JtagAxiMaster, VioCore};
use crate::{
    CAPTURE_BRAM, IlaTriggerKind, MATCH_HIT_NET, compile, match_unit_init,
    probe_site,
};

/// Base of the first BRAM aperture on the JTAG-to-AXI master.
pub const AXI_BRAM_BASE: u32 = 0xC000_0000;
/// Address stride between consecutive BRAM apertures.
pub const AXI_BRAM_STRIDE: u32 = 0x1000;
/// Words per BRAM aperture (Bram18 INIT minors 1..=255).
pub const AXI_BRAM_WORDS: u32 = 255;

/// Result of [`implement_debug`]: the debug netlist and its bitstream.
#[derive(Clone, Debug)]
pub struct DebugBuild {
    pub part: String,
    /// Netlist with the debug cores stitched in.
    pub design: Design,
    pub routed: Routed,
    pub bits: Bitstream,
    pub report: DebugCoreInsertionReport,
    /// LUT+FF clusters before and after insertion (debug overhead).
    pub baseline_lutffs: usize,
    pub debug_lutffs: usize,
    /// Packed BRAMs in the debug build (capture RAM included).
    pub brams: Vec<String>,
    /// BRAM major index of the ILA capture RAM.
    pub capture_major: u16,
}

impl DebugBuild {
    pub fn ltx(&self) -> &DebugProbesLtx {
        &self.report.ltx
    }

    /// ILA probe channels in sample-bit order (`samples` bit `i` = net `i`).
    pub fn ila_nets(&self) -> &[String] {
        &self.report.probed_nets
    }

    pub fn core(&self, kind: &str) -> Option<&crate::ltx::LtxCoreDescriptor> {
        self.report.ltx.cores.iter().find(|c| c.core_kind == kind)
    }
}

/// Set Up Debug + re-implement. `cfg.explicit_probes`, when non-empty, is the
/// authoritative probe list: `mark_debug` attributes on other nets are cleared
/// on the copy so an unchecked net is not probed.
pub fn implement_debug(
    dev: &Device,
    design: &Design,
    cfg: &DebugInsertConfig,
) -> Result<DebugBuild, String> {
    let mut base = design.clone();
    strip_debug_cores(&mut base);
    if !cfg.explicit_probes.is_empty() {
        for n in &mut base.nets {
            n.attrs
                .map
                .retain(|k, _| !k.eq_ignore_ascii_case("mark_debug"));
        }
    }
    let baseline_lutffs = pack(&base, dev)?.lutffs.len();
    let mut d = base;
    let report = insert_debug_cores(&mut d, cfg)?;
    let (routed, bits) = compile(dev, &d)?;
    let packed = &routed.placed.packed;
    let capture_major = packed
        .brams
        .iter()
        .position(|b| b.cell == CAPTURE_BRAM)
        .ok_or("set up debug: capture BRAM was not packed")? as u16;
    let has_frame = bits
        .frames
        .keys()
        .any(|(bt, maj, _)| *bt == helion_device::Far::BRAM && *maj == capture_major);
    if !has_frame {
        return Err("set up debug: bitstream has no capture BRAM frame".into());
    }
    for n in &report.probed_nets {
        probe_site(&routed.placed, &d, n)
            .map_err(|e| format!("set up debug: probe {n}: {e}"))?;
    }
    probe_site(&routed.placed, &d, MATCH_HIT_NET)
        .map_err(|e| format!("set up debug: match unit: {e}"))?;
    probe_site(&routed.placed, &d, CMP_HIT_NET)
        .map_err(|e| format!("set up debug: value comparator: {e}"))?;
    Ok(DebugBuild {
        part: dev.part.clone(),
        debug_lutffs: packed.lutffs.len(),
        brams: packed.brams.iter().map(|b| b.cell.clone()).collect(),
        design: d,
        routed,
        bits,
        report,
        baseline_lutffs,
        capture_major,
    })
}

/// Runtime trigger for [`HwDebugTarget::run_hw_ila`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HwTrigger {
    /// Fire on the first sample after the pre-trigger buffer fills.
    Immediate,
    /// Edge on the match-unit net (the trigger net chosen at Set Up Debug).
    Rising,
    Falling,
    /// `(probes[0..6] & mask) == (value & mask)` on the comparator LUT.
    Value { value: u64, mask: u64 },
}

impl HwTrigger {
    pub fn label(self) -> &'static str {
        match self {
            HwTrigger::Immediate => "immediate",
            HwTrigger::Rising => "rising",
            HwTrigger::Falling => "falling",
            HwTrigger::Value { .. } => "value",
        }
    }
}

/// Probe bits the comparator LUT6 sees (`I0..I5`).
pub const VALUE_COMPARE_BITS: usize = 6;

/// LUT6 INIT for the value comparator: `O = 1` iff the six inputs match
/// `value` on the bits set in `mask` (bits past `width` are don't-care).
pub fn value_compare_init(value: u64, mask: u64, width: usize) -> u64 {
    let w = width.min(VALUE_COMPARE_BITS);
    let wmask = if w == 0 { 0 } else { (1u64 << w) - 1 };
    let m = mask & wmask;
    let v = value & m;
    let mut init = 0u64;
    for addr in 0..64u64 {
        if addr & m == v {
            init |= 1 << addr;
        }
    }
    init
}

/// Arm settings for one hardware ILA run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HwIlaRun {
    pub trigger: HwTrigger,
    /// Capture window in samples (`1..=sample_depth`).
    pub window: usize,
    /// Samples kept before the trigger sample (`< window`).
    pub pre_trigger: usize,
    /// Give up after this many user clocks without a trigger.
    pub max_cycles: usize,
}

/// One uploaded hardware ILA window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HwIlaCapture {
    pub core: String,
    /// Probe nets; bit `i` of every sample word is `nets[i]`.
    pub nets: Vec<String>,
    /// Chronological sample words uploaded from the capture BRAM.
    pub words: Vec<u64>,
    /// Index of the trigger sample inside `words`.
    pub trigger_at: usize,
    pub trigger: HwTrigger,
    /// User clocks the fabric ran during this arm.
    pub cycles: usize,
    /// USR1 DR scans of the RLE upload vs an uncompressed upload.
    pub upload_dr_scans: usize,
    pub upload_raw_scans: usize,
    /// BSCAN hub frames for arm/configure/upload.
    pub hub_frames: usize,
}

impl HwIlaCapture {
    pub fn bit(&self, sample: usize, net: usize) -> bool {
        self.words
            .get(sample)
            .is_some_and(|w| (w >> net) & 1 == 1)
    }
}

/// One VIO probe as the dashboard shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HwVioProbe {
    pub name: String,
    pub direction: LtxProbeDirection,
    pub nets: Vec<String>,
    pub width: usize,
    /// `PROBE_IN`: last sampled value; `PROBE_OUT`: committed value.
    pub value: u64,
    /// `PROBE_IN` sticky activity (`NONE`, `RISING`, `FALLING`, `BOTH`).
    pub activity: &'static str,
    pub transitions: u64,
    /// `PROBE_OUT` currently overriding its net.
    pub driving: bool,
}

/// A programmed device with debug cores, reached over the sim cable.
pub struct HwDebugTarget {
    pub part: String,
    tap: Tap,
    pub build: DebugBuild,
    pub hub: BscanDebugHub,
    pub vio: Option<VioCore>,
    /// Per `vio.probes_out` entry: overriding its net right now.
    vio_driving: Vec<bool>,
    pub axi: Option<JtagAxiMaster>,
    /// User clocks run since programming.
    pub cycles: u64,
    pub stat: Stat,
    pub idcode: u32,
}

impl std::fmt::Debug for HwDebugTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HwDebugTarget")
            .field("part", &self.part)
            .field("cores", &self.hub.slots.len())
            .field("cycles", &self.cycles)
            .finish()
    }
}

fn vio_from_ltx(build: &DebugBuild, slot: u8) -> Option<VioCore> {
    let core = build.core("vio")?;
    let mut vio = VioCore::new(core.core_name.clone(), slot);
    for ch in &core.probes {
        let name = channel_name(ch);
        match ch.direction {
            LtxProbeDirection::ProbeIn => vio.add_probe_in(name, ch.nets.clone()),
            LtxProbeDirection::ProbeOut => vio.add_probe_out(name, ch.nets.clone(), 0),
            LtxProbeDirection::Probe => {}
        }
    }
    Some(vio)
}

fn channel_name(ch: &LtxProbeChannel) -> String {
    format!("{} {}", ch.port_id, ch.display_signal())
}

impl HwDebugTarget {
    /// Program `build` into a fresh Helion TAP and enumerate the hub.
    pub fn program_sim(dev: &Device, build: DebugBuild) -> Result<Self, String> {
        if dev.part != build.part {
            return Err(format!(
                "program_hw_devices: debug build is for {}, device is {}",
                build.part, dev.part
            ));
        }
        let mut tap = Tap::new(dev);
        let idcode = tap.read_idcode();
        let stat = tap.program(&build.bits)?;
        if !stat.done {
            return Err("program_hw_devices: DONE stayed low after CFG_W".into());
        }
        let mut hub = build.report.hub.clone();
        let vio = hub
            .slots
            .iter()
            .find(|s| s.kind == DebugCoreKind::Vio)
            .and_then(|s| vio_from_ltx(&build, s.slot_id));
        let axi = hub
            .slots
            .iter()
            .find(|s| s.kind == DebugCoreKind::JtagAxi)
            .map(|s| {
                let mut m = JtagAxiMaster::bare(s.core_name.clone(), s.slot_id);
                for (i, cell) in build.brams.iter().enumerate() {
                    m.map_region(AxiRegion {
                        name: cell.clone(),
                        base_addr: AXI_BRAM_BASE + AXI_BRAM_STRIDE * i as u32,
                        size_bytes: AXI_BRAM_WORDS * 4,
                        read_only: false,
                        bram_aperture_major: Some(i as u16),
                    });
                }
                m
            });
        let n_out = vio.as_ref().map(|v| v.probes_out.len()).unwrap_or(0);
        // Hub discovery over the TAP (`refresh_hw_device`).
        let _ = hub.transact_via_tap(
            &mut tap,
            crate::bscan::BSCAN_HUB_BROADCAST_SLOT,
            hub_opcode::DISCOVER,
            0,
        )?;
        Ok(Self {
            part: dev.part.clone(),
            tap,
            build,
            hub,
            vio,
            vio_driving: vec![false; n_out],
            axi,
            cycles: 0,
            stat,
            idcode,
        })
    }

    /// Re-read IDCODE + STAT over the TAP.
    pub fn refresh_stat(&mut self) -> Stat {
        self.idcode = self.tap.read_idcode();
        self.stat = self.tap.read_stat();
        self.stat.clone()
    }

    /// Debug cores the hub reports: `(slot, name, kind, uuid)`.
    pub fn cores(&self) -> Vec<(u8, String, DebugCoreKind, String)> {
        self.hub
            .slots
            .iter()
            .map(|s| (s.slot_id, s.core_name.clone(), s.kind, s.uuid_hex()))
            .collect()
    }

    fn site(&self, net: &str) -> Result<(Site, u8), String> {
        probe_site(&self.build.routed.placed, &self.build.design, net)
    }

    fn apply_vio_force(&mut self) -> Result<(), String> {
        let Some(vio) = &self.vio else {
            return Ok(());
        };
        let mut pokes = Vec::new();
        for (i, p) in vio.probes_out.iter().enumerate() {
            if !self.vio_driving.get(i).copied().unwrap_or(false) {
                continue;
            }
            for (bit, net) in p.nets.iter().enumerate() {
                let (site, ble) = self.site(net)?;
                pokes.push((site, ble, (p.committed_value >> bit) & 1 == 1));
            }
        }
        if pokes.is_empty() {
            return Ok(());
        }
        let fab = self.tap.fabric_mut();
        for (site, ble, v) in pokes {
            fab.set_ble_q(site.x, site.y, u32::from(ble), v);
        }
        fab.refresh_comb();
        Ok(())
    }

    /// Run `n` user clocks (VIO overrides re-applied after each edge).
    pub fn run_cycles(&mut self, n: usize) -> Result<(), String> {
        for _ in 0..n {
            self.tap.fabric_mut().step_user();
            self.apply_vio_force()?;
            self.cycles += 1;
        }
        Ok(())
    }

    /// `run_hw_ila` + `wait_on_hw_ila` + `upload_hw_ila_data`.
    pub fn run_hw_ila(&mut self, run: &HwIlaRun) -> Result<HwIlaCapture, String> {
        let core = self
            .build
            .core("ila")
            .ok_or("run_hw_ila: no ILA core in the debug build")?
            .clone();
        let slot = self
            .hub
            .slot_by_name(&core.core_name)
            .map(|s| s.slot_id)
            .ok_or("run_hw_ila: ILA core not on the hub")?;
        let depth = core.sample_depth.max(1);
        let window = run.window;
        if window == 0 || window > depth {
            return Err(format!(
                "run_hw_ila: window {window} outside 1..={depth} (C_DATA_DEPTH)"
            ));
        }
        let pre = if run.trigger == HwTrigger::Immediate {
            0
        } else {
            run.pre_trigger
        };
        if pre >= window {
            return Err(format!(
                "run_hw_ila: pre-trigger {pre} must be < window {window}"
            ));
        }
        let nets = self.build.report.probed_nets.clone();
        if nets.len() > 64 {
            return Err("run_hw_ila: more than 64 probe bits per sample word".into());
        }
        let frames0 = self.hub.total_dr_scans;

        // Program the runtime comparators (match-unit / value LUT INITs).
        let (kind, hit_net) = match run.trigger {
            HwTrigger::Immediate => (IlaMatchKind::Immediate, MATCH_HIT_NET),
            HwTrigger::Rising | HwTrigger::Falling => {
                let k = if run.trigger == HwTrigger::Rising {
                    IlaTriggerKind::Rising
                } else {
                    IlaTriggerKind::Falling
                };
                let (site, ble) = self.lut_site("ila_match_lut", MATCH_HIT_NET)?;
                self.tap.fabric_mut().patch_lut_init_live(
                    site.x,
                    site.y,
                    u32::from(ble),
                    match_unit_init(k),
                )?;
                let kind = if k == IlaTriggerKind::Rising {
                    IlaMatchKind::Rising
                } else {
                    IlaMatchKind::Falling
                };
                (kind, MATCH_HIT_NET)
            }
            HwTrigger::Value { value, mask } => {
                let (site, ble) = self.lut_site(CMP_LUT_CELL, CMP_HIT_NET)?;
                let init = value_compare_init(value, mask, nets.len());
                self.tap
                    .fabric_mut()
                    .patch_lut_init_live(site.x, site.y, u32::from(ble), init)?;
                (IlaMatchKind::Value, CMP_HIT_NET)
            }
        };
        let ctrl = ((window as u64) << 16) | ((pre as u64) << 32) | kind as u64;
        self.hub
            .transact_via_tap(&mut self.tap, slot, hub_opcode::WRITE_CTRL, ctrl)?;
        self.hub
            .transact_via_tap(&mut self.tap, slot, hub_opcode::ARM_TRIGGER, kind as u64)?;

        let mut sites = Vec::with_capacity(nets.len());
        for n in &nets {
            sites.push(self.site(n)?);
        }
        let (hit_site, hit_ble) = self.site(hit_net)?;
        let major = self.build.capture_major;
        let mut wr = 0usize;
        let mut cycles = 0usize;
        self.tap.fabric_mut().ila_arm_fsm(kind, window, pre);
        loop {
            if cycles >= run.max_cycles.max(window) {
                return Err(format!(
                    "run_hw_ila: {} trigger did not fire in {cycles} clocks",
                    run.trigger.label()
                ));
            }
            self.tap.fabric_mut().step_user();
            self.apply_vio_force()?;
            self.cycles += 1;
            cycles += 1;
            let fab = self.tap.fabric_mut();
            fab.refresh_comb();
            let mut word = 0u64;
            for (i, (site, ble)) in sites.iter().enumerate() {
                if fab.ble_out(site.x, site.y, u32::from(*ble)) {
                    word |= 1 << i;
                }
            }
            fab.bram_write_word(major, wr % window, word);
            wr += 1;
            // The trigger is only accepted once the pre-trigger buffer holds
            // `pre` samples (Vivado's "pre-trigger" capture status).
            if wr > pre {
                let hit = fab.ble_out(hit_site.x, hit_site.y, u32::from(hit_ble));
                if fab.ila_fsm.on_sample(hit) {
                    break;
                }
            }
        }
        if !self.tap.fabric().ila_fsm.fired {
            return Err("run_hw_ila: trigger FSM finished without firing".into());
        }
        self.hub
            .transact_via_tap(&mut self.tap, slot, hub_opcode::UPLOAD_BRAM, major as u64)?;
        let start = (wr - window) % window;
        let rle = self
            .tap
            .usr1_upload_bram_rle(major, start as u32, window, window as u32);
        if rle.words.len() != window {
            return Err(format!(
                "upload_hw_ila_data: uploaded {} words, expected {window}",
                rle.words.len()
            ));
        }
        Ok(HwIlaCapture {
            core: core.core_name,
            nets,
            words: rle.words,
            trigger_at: pre,
            trigger: run.trigger,
            cycles,
            upload_dr_scans: rle.dr_scans,
            upload_raw_scans: rle.raw_scans,
            hub_frames: self.hub.total_dr_scans - frames0,
        })
    }

    /// Fabric site of a LUT cell, falling back to its output net.
    fn lut_site(&self, lut_cell: &str, out_net: &str) -> Result<(Site, u8), String> {
        let placed = &self.build.routed.placed;
        if let Some(i) = placed
            .packed
            .lutffs
            .iter()
            .position(|l| l.lut_cell == lut_cell)
        {
            if let Some(s) = placed.lutff_sites.get(i) {
                return Ok(*s);
            }
        }
        self.site(out_net)
    }

    /// `refresh_hw_vio`: run `cycles` clocks, then sample every `PROBE_IN`.
    pub fn refresh_hw_vio(&mut self, cycles: usize) -> Result<Vec<HwVioProbe>, String> {
        if self.vio.is_none() {
            return Err("refresh_hw_vio: no VIO core in the debug build".into());
        }
        self.run_cycles(cycles)?;
        let mut vio = self.vio.take().expect("checked");
        let r = {
            let placed = &self.build.routed.placed;
            let design = &self.build.design;
            let fab = self.tap.fabric();
            let mut words = Vec::new();
            for p in &vio.probes_in {
                let mut w = 0u64;
                for (bit, net) in p.nets.iter().enumerate().take(64) {
                    let (site, ble) = probe_site(placed, design, net)?;
                    if fab.ble_out(site.x, site.y, u32::from(ble)) {
                        w |= 1 << bit;
                    }
                }
                words.push(w);
            }
            Ok::<_, String>(words)
        };
        let res = match r {
            Ok(words) => {
                let mut err = None;
                for (i, w) in words.into_iter().enumerate() {
                    let name = vio.probes_in[i].name.clone();
                    let tap = &mut self.tap;
                    vio.probes_in[i].observe(w);
                    if let Err(e) = self.hub.transact_via_tap(
                        tap,
                        vio.slot_id,
                        hub_opcode::PROBE_READ,
                        w,
                    ) {
                        err = Some(format!("{name}: {e}"));
                    }
                }
                match err {
                    Some(e) => Err(e),
                    None => Ok(()),
                }
            }
            Err(e) => Err(e),
        };
        self.vio = Some(vio);
        res?;
        Ok(self.vio_probes())
    }

    /// `set_property OUTPUT_VALUE v [get_hw_probes p]` + `commit_hw_vio`:
    /// stage, commit over the hub, and start overriding the net.
    pub fn set_vio_output(&mut self, probe: &str, value: u64) -> Result<u64, String> {
        let vio = self
            .vio
            .as_mut()
            .ok_or("set_property OUTPUT_VALUE: no VIO core in the debug build")?;
        let idx = vio
            .probes_out
            .iter()
            .position(|p| p.name == probe || p.nets.iter().any(|n| n == probe))
            .ok_or_else(|| format!("set_property OUTPUT_VALUE: no PROBE_OUT {probe}"))?;
        let name = vio.probes_out[idx].name.clone();
        vio.set_probe_out(&name, value)?;
        let mut vio = self.vio.take().expect("checked");
        let r = vio_commit_via_tap(&mut vio, &mut self.hub, &mut self.tap);
        let committed = vio.probes_out[idx].committed_value;
        self.vio = Some(vio);
        r?;
        if let Some(d) = self.vio_driving.get_mut(idx) {
            *d = true;
        }
        self.apply_vio_force()?;
        Ok(committed)
    }

    /// Stop overriding a `PROBE_OUT` net (its logic driver takes over).
    pub fn release_vio_output(&mut self, probe: &str) -> Result<(), String> {
        let vio = self.vio.as_ref().ok_or("no VIO core in the debug build")?;
        let idx = vio
            .probes_out
            .iter()
            .position(|p| p.name == probe || p.nets.iter().any(|n| n == probe))
            .ok_or_else(|| format!("no PROBE_OUT {probe}"))?;
        if let Some(d) = self.vio_driving.get_mut(idx) {
            *d = false;
        }
        Ok(())
    }

    /// Clear sticky activity on every `PROBE_IN`.
    pub fn reset_vio_activity(&mut self) {
        if let Some(v) = self.vio.as_mut() {
            v.clear_all_activity();
        }
    }

    pub fn vio_probes(&self) -> Vec<HwVioProbe> {
        let Some(vio) = &self.vio else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for p in &vio.probes_in {
            out.push(HwVioProbe {
                name: p.name.clone(),
                direction: LtxProbeDirection::ProbeIn,
                nets: p.nets.clone(),
                width: p.width,
                value: p.value,
                activity: p.activity().as_str(),
                transitions: p.transition_count,
                driving: false,
            });
        }
        for (i, p) in vio.probes_out.iter().enumerate() {
            out.push(HwVioProbe {
                name: p.name.clone(),
                direction: LtxProbeDirection::ProbeOut,
                nets: p.nets.clone(),
                width: p.width,
                value: p.committed_value,
                activity: "",
                transitions: p.commit_count,
                driving: self.vio_driving.get(i).copied().unwrap_or(false),
            });
        }
        out
    }

    /// `run_hw_axi`: execute `txn` through the hub on the live TAP.
    pub fn run_hw_axi(&mut self, txn: &mut HwAxiTxn) -> Result<AxiResp, String> {
        let axi = self
            .axi
            .as_mut()
            .ok_or("run_hw_axi: no JTAG-to-AXI core in the debug build")?;
        axi.run_hw_axi_on_tap(txn, &mut self.hub, &mut self.tap)
    }

    /// AXI apertures the master can reach.
    pub fn axi_regions(&self) -> Vec<AxiRegion> {
        self.axi
            .as_ref()
            .map(|a| a.regions.clone())
            .unwrap_or_default()
    }

    /// Debug-hub cell name (for the hardware tree).
    pub fn hub_name(&self) -> String {
        self.hub.hub_name.clone()
    }

    /// Design top name of the debug build.
    pub fn design_name(&self) -> String {
        resolve(self.build.design.name).to_string()
    }
}

fn vio_commit_via_tap(
    vio: &mut VioCore,
    hub: &mut BscanDebugHub,
    tap: &mut Tap,
) -> Result<usize, String> {
    let mut frames = 0usize;
    for (ch_idx, p) in vio.probes_out.iter_mut().enumerate() {
        if p.dirty {
            let payload = ((ch_idx as u64 & 0xFF) << 40) | (p.staged_value & 0x00FF_FFFF_FFFF);
            hub.transact_via_tap(tap, vio.slot_id, hub_opcode::PROBE_WRITE, payload)?;
            p.committed_value = p.staged_value;
            p.dirty = false;
            p.commit_count += 1;
            frames += 1;
        }
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev() -> Device {
        Device::load_part("HL10T-C32-1").unwrap()
    }

    fn cfg(probes: &[&str]) -> DebugInsertConfig {
        let mut c = DebugInsertConfig::with_probes(probes.iter().copied());
        c.sample_depth = 32;
        c
    }

    fn counter_value(cap: &HwIlaCapture, i: usize) -> u64 {
        cap.words[i] & 0xF
    }

    #[test]
    fn value_compare_init_matches_masked_inputs() {
        let init = value_compare_init(0b1010, 0b1111, 4);
        for a in 0..64u64 {
            assert_eq!((init >> a) & 1 == 1, a & 0xF == 0b1010, "addr {a}");
        }
        // Mask bit 0 only: every odd input address fires.
        let odd = value_compare_init(1, 1, 4);
        assert_eq!(odd, 0xAAAA_AAAA_AAAA_AAAA);
    }

    #[test]
    fn implement_debug_builds_and_programs_the_sim_target() {
        let dev = dev();
        let d = Design::structural_counter();
        let build = implement_debug(&dev, &d, &cfg(&["q0", "q1", "q2", "q3"])).unwrap();
        assert!(build.debug_lutffs > build.baseline_lutffs);
        assert!(build.brams.iter().any(|b| b == CAPTURE_BRAM));
        assert_eq!(build.ila_nets().len(), 4);
        let ltx = build.ltx().to_ltx_string();
        assert!(ltx.contains("q3"), "{ltx}");
        let mut t = HwDebugTarget::program_sim(&dev, build).unwrap();
        assert!(t.stat.done);
        assert_eq!(t.cores().len(), 1);
        assert_eq!(t.cores()[0].2, DebugCoreKind::Ila);
        assert!(t.refresh_stat().done);
    }

    #[test]
    fn explicit_probes_override_mark_debug() {
        let dev = dev();
        let mut d = Design::structural_counter();
        d.mark_debug("q3").unwrap();
        let build = implement_debug(&dev, &d, &cfg(&["q0"])).unwrap();
        assert_eq!(build.ila_nets(), &["q0".to_string()]);
    }

    #[test]
    fn hw_ila_immediate_captures_the_counter_sequence() {
        let dev = dev();
        let d = Design::structural_counter();
        let build = implement_debug(&dev, &d, &cfg(&["q0", "q1", "q2", "q3"])).unwrap();
        let mut t = HwDebugTarget::program_sim(&dev, build).unwrap();
        let cap = t
            .run_hw_ila(&HwIlaRun {
                trigger: HwTrigger::Immediate,
                window: 16,
                pre_trigger: 0,
                max_cycles: 256,
            })
            .unwrap();
        assert_eq!(cap.words.len(), 16);
        assert_eq!(cap.trigger_at, 0);
        for i in 1..16 {
            assert_eq!(
                counter_value(&cap, i),
                (counter_value(&cap, i - 1) + 1) & 0xF,
                "counter increments every sample: {:?}",
                cap.words
            );
        }
        assert!(cap.upload_dr_scans > 0 && cap.hub_frames >= 3);
    }

    #[test]
    fn hw_ila_value_trigger_lands_on_the_value_after_pretrigger() {
        let dev = dev();
        let d = Design::structural_counter();
        let build = implement_debug(&dev, &d, &cfg(&["q0", "q1", "q2", "q3"])).unwrap();
        let mut t = HwDebugTarget::program_sim(&dev, build).unwrap();
        for target in [9u64, 3] {
            let cap = t
                .run_hw_ila(&HwIlaRun {
                    trigger: HwTrigger::Value {
                        value: target,
                        mask: 0xF,
                    },
                    window: 8,
                    pre_trigger: 2,
                    max_cycles: 256,
                })
                .unwrap();
            assert_eq!(cap.trigger_at, 2);
            assert_eq!(counter_value(&cap, 2), target, "{:?}", cap.words);
            assert_eq!(counter_value(&cap, 1), (target + 15) & 0xF);
        }
    }

    #[test]
    fn hw_ila_edge_trigger_on_the_chosen_net() {
        let dev = dev();
        let d = Design::structural_counter();
        let mut c = cfg(&["q0", "q1", "q2", "q3"]);
        c.trigger_net = Some("q3".into());
        let build = implement_debug(&dev, &d, &c).unwrap();
        assert_eq!(build.report.trigger_net, "q3");
        let mut t = HwDebugTarget::program_sim(&dev, build).unwrap();
        let cap = t
            .run_hw_ila(&HwIlaRun {
                trigger: HwTrigger::Falling,
                window: 8,
                pre_trigger: 4,
                max_cycles: 256,
            })
            .unwrap();
        // q3 falls when the counter wraps 15 -> 0.
        assert!(cap.bit(3, 3), "{:?}", cap.words);
        assert!(!cap.bit(4, 3), "{:?}", cap.words);
        assert_eq!(counter_value(&cap, 4), 0);
        let rise = t
            .run_hw_ila(&HwIlaRun {
                trigger: HwTrigger::Rising,
                window: 8,
                pre_trigger: 1,
                max_cycles: 256,
            })
            .unwrap();
        assert_eq!(counter_value(&rise, 1), 8, "{:?}", rise.words);
    }

    #[test]
    fn hw_ila_rejects_bad_windows() {
        let dev = dev();
        let d = Design::structural_counter();
        let build = implement_debug(&dev, &d, &cfg(&["q0"])).unwrap();
        let mut t = HwDebugTarget::program_sim(&dev, build).unwrap();
        let mut run = HwIlaRun {
            trigger: HwTrigger::Rising,
            window: 64,
            pre_trigger: 0,
            max_cycles: 64,
        };
        assert!(t.run_hw_ila(&run).is_err(), "window > depth");
        run.window = 8;
        run.pre_trigger = 8;
        assert!(t.run_hw_ila(&run).is_err(), "pre >= window");
    }

    #[test]
    fn vio_reads_inputs_and_drives_outputs() {
        let dev = dev();
        let d = Design::structural_counter();
        let mut c = cfg(&["q0"]);
        c.vio_probes_in = vec!["q2".into(), "q3".into()];
        c.vio_probes_out = vec!["q1".into()];
        let build = implement_debug(&dev, &d, &c).unwrap();
        let mut t = HwDebugTarget::program_sim(&dev, build).unwrap();
        assert_eq!(t.cores().len(), 2);
        let probes = t.refresh_hw_vio(20).unwrap();
        let ins: Vec<_> = probes
            .iter()
            .filter(|p| p.direction == LtxProbeDirection::ProbeIn)
            .collect();
        assert_eq!(ins.len(), 2);
        // q3 toggled at least once over 20 counter clocks.
        let _ = t.refresh_hw_vio(20).unwrap();
        let q3 = t
            .vio_probes()
            .into_iter()
            .find(|p| p.nets == vec!["q3".to_string()])
            .unwrap();
        assert!(q3.transitions > 0, "{q3:?}");
        let out = t
            .vio_probes()
            .into_iter()
            .find(|p| p.direction == LtxProbeDirection::ProbeOut)
            .unwrap();
        assert!(!out.driving);
        assert_eq!(t.set_vio_output(&out.name, 1).unwrap(), 1);
        // Forced q1 stays high across clocks.
        for _ in 0..5 {
            t.run_cycles(1).unwrap();
            let (site, ble) = t.site("q1").unwrap();
            assert!(t.tap.fabric().ble_out(site.x, site.y, u32::from(ble)));
        }
        t.release_vio_output(&out.name).unwrap();
        assert!(!t.vio_probes().iter().any(|p| p.driving));
    }

    #[test]
    fn axi_reaches_only_real_bram_apertures() {
        let dev = dev();
        let d = Design::structural_counter();
        let mut c = cfg(&["q0", "q1"]);
        c.insert_jtag_axi = true;
        let build = implement_debug(&dev, &d, &c).unwrap();
        let mut t = HwDebugTarget::program_sim(&dev, build).unwrap();
        let regions = t.axi_regions();
        assert_eq!(regions.len(), 1, "only the capture BRAM exists: {regions:?}");
        let base = regions[0].base_addr;
        let mut wr = crate::create_hw_axi_txn(
            "wr",
            base + 8,
            crate::AxiTxnKind::Write,
            2,
            vec![0xDEAD_0001, 0xDEAD_0002],
        )
        .unwrap();
        assert_eq!(t.run_hw_axi(&mut wr).unwrap(), AxiResp::Okay);
        let major = t.build.capture_major;
        assert_eq!(t.tap.fabric().bram_read_word(major, 2) as u32, 0xDEAD_0001);
        let mut rd =
            crate::create_hw_axi_txn("rd", base + 8, crate::AxiTxnKind::Read, 2, vec![]).unwrap();
        assert_eq!(t.run_hw_axi(&mut rd).unwrap(), AxiResp::Okay);
        assert_eq!(rd.read_data, vec![0xDEAD_0001, 0xDEAD_0002]);
        // Fabric-side update is visible to the next read (no stale host cache).
        t.tap.fabric_mut().bram_write_word(major, 2, 0x55);
        let mut rd2 =
            crate::create_hw_axi_txn("rd2", base + 8, crate::AxiTxnKind::Read, 1, vec![]).unwrap();
        t.run_hw_axi(&mut rd2).unwrap();
        assert_eq!(rd2.read_data, vec![0x55]);
        let mut dec =
            crate::create_hw_axi_txn("x", 0x4000_0000, crate::AxiTxnKind::Read, 1, vec![]).unwrap();
        assert_eq!(t.run_hw_axi(&mut dec).unwrap(), AxiResp::DecErr);
    }
}
