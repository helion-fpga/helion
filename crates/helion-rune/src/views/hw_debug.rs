//! Hardware debug model: Set Up Debug, the programmed debug target, and its
//! ILA / VIO / JTAG-to-AXI cores (the views live in
//! [`crate::views::hw_debug_view`]).
//!
//! Every engine step is `helion-debug`:
//! - Set Up Debug runs [`helion_debug::implement_debug`] (insert ILA/VIO/AXI
//!   cores + BSCAN hub, pack/place/route/bitgen) on a worker and writes the
//!   `.ltx` probes file and the debug `.hbits` next to the run checkpoints.
//! - Program runs [`helion_debug::HwDebugTarget::program_on`] over the
//!   Simulation cable (in-process Helion TAP) or the Native FTDI cable
//!   (`helion-hw`'s MPSSE JTAG engine, opened by
//!   [`crate::views::hw_native::open_cable`]) and enumerates the hub.
//! - ILA arm/upload, VIO refresh/drive and AXI transactions are BSCAN hub
//!   frames on that cable, each on a worker; the UI keeps a snapshot that
//!   names the cable (`native FTDI · USB (chain scan verified on hardware; programming unverified)`).
//!
//! openFPGALoader cables are refused with the reason: openFPGALoader cannot
//! shift `USER1` data registers.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

use helion_debug::{
    AxiRegion, AxiResp, AxiTxnKind, DebugBuild, DebugCoreKind, DebugInsertConfig, HwDebugTarget,
    HwIlaCapture, HwIlaRun, HwTrigger, HwVioProbe, LtxProbeDirection, create_hw_axi_txn,
};
use helion_hw::CableKind;
use rune::TextEdit;

use crate::state::{AppState, CenterPane, ConsoleLine, MsgSev, ToastKind};

/// Capture depths offered by Set Up Debug (Bram18 INIT holds 255 words).
pub const DEPTHS: [usize; 5] = [16, 32, 64, 128, 255];
/// Transaction log rows kept.
pub const AXI_LOG_MAX: usize = 64;

/// Which list a picked net joins in Set Up Debug.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProbeRole {
    #[default]
    Ila,
    VioIn,
    VioOut,
}

impl ProbeRole {
    pub const ALL: [ProbeRole; 3] = [ProbeRole::Ila, ProbeRole::VioIn, ProbeRole::VioOut];

    pub fn label(self) -> &'static str {
        match self {
            ProbeRole::Ila => "ILA probe",
            ProbeRole::VioIn => "VIO input",
            ProbeRole::VioOut => "VIO output",
        }
    }
}

/// Trigger choice in the ILA panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrigMode {
    #[default]
    Immediate,
    Rising,
    Falling,
    Value,
}

impl TrigMode {
    pub const ALL: [TrigMode; 4] = [
        TrigMode::Immediate,
        TrigMode::Rising,
        TrigMode::Falling,
        TrigMode::Value,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TrigMode::Immediate => "Immediate",
            TrigMode::Rising => "Rising",
            TrigMode::Falling => "Falling",
            TrigMode::Value => "Value",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "immediate" | "now" => Some(TrigMode::Immediate),
            "rising" | "r" => Some(TrigMode::Rising),
            "falling" | "f" => Some(TrigMode::Falling),
            "value" | "eq" => Some(TrigMode::Value),
            _ => None,
        }
    }
}

/// Worker job kinds (one at a time).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbgJobKind {
    Build,
    Program,
    Ila,
    Vio,
    Axi,
}

impl DbgJobKind {
    pub fn busy_label(self) -> &'static str {
        match self {
            DbgJobKind::Build => "Implementing debug cores…",
            DbgJobKind::Program => "Programming debug target…",
            DbgJobKind::Ila => "ILA armed — waiting for trigger…",
            DbgJobKind::Vio => "Refreshing VIO…",
            DbgJobKind::Axi => "Running AXI transaction…",
        }
    }
}

/// Files written by Set Up Debug.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugFiles {
    pub ltx: PathBuf,
    pub hbits: PathBuf,
}

/// One debug core on the hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreRow {
    pub slot: u8,
    pub name: String,
    pub kind: DebugCoreKind,
    pub uuid: String,
}

/// UI copy of the target after each operation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TargetSnap {
    pub part: String,
    pub cores: Vec<CoreRow>,
    pub vio: Vec<HwVioProbe>,
    pub axi_regions: Vec<(String, u32, u32)>,
    pub cycles: u64,
    pub done: bool,
    pub idcode: u32,
    pub hub_frames: usize,
    /// Cable the target is on and its description (port, TCK, chain slot).
    pub cable: CableKind,
    pub cable_detail: String,
    /// (IR, DR) scans the native engine has shifted (0 on the sim TAP).
    pub scans: (usize, usize),
}

impl TargetSnap {
    fn of(t: &HwDebugTarget) -> Self {
        Self {
            part: t.part.clone(),
            cores: t
                .cores()
                .into_iter()
                .map(|(slot, name, kind, uuid)| CoreRow {
                    slot,
                    name,
                    kind,
                    uuid,
                })
                .collect(),
            vio: t.vio_probes(),
            axi_regions: t
                .axi_regions()
                .into_iter()
                .map(|r: AxiRegion| (r.name, r.base_addr, r.size_bytes))
                .collect(),
            cycles: t.cycles,
            done: t.stat.done,
            idcode: t.idcode,
            hub_frames: t.hub.total_dr_scans,
            cable: t.cable_kind(),
            cable_detail: t.cable_detail(),
            scans: t.cable_scans(),
        }
    }

    pub fn has(&self, kind: DebugCoreKind) -> bool {
        self.cores.iter().any(|c| c.kind == kind)
    }
}

/// One AXI transaction-log row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxiLogRow {
    pub name: String,
    pub kind: AxiTxnKind,
    pub addr: u32,
    pub len: usize,
    pub data: Vec<u32>,
    pub resp: Result<AxiResp, String>,
    pub dr_scans: usize,
}

/// Worker -> UI message.
#[derive(Debug)]
pub enum DbgEvent {
    Built(Result<(Arc<DebugBuild>, DebugFiles), String>),
    Programmed(Result<(Arc<Mutex<HwDebugTarget>>, TargetSnap), String>),
    Ila(Result<(HwIlaCapture, TargetSnap), String>),
    Vio(Result<TargetSnap, String>),
    Axi(Box<AxiLogRow>, Option<TargetSnap>),
}

#[derive(Clone)]
struct DbgJob {
    kind: DbgJobKind,
    rx: Arc<Mutex<mpsc::Receiver<DbgEvent>>>,
}

/// Set Up Debug inputs, the debug build, the live target and its panels.
#[derive(Clone)]
pub struct HwDebugState {
    pub ila_nets: Vec<String>,
    pub vio_in: Vec<String>,
    pub vio_out: Vec<String>,
    /// Net the edge match unit watches (`None` = first ILA probe).
    pub trigger_net: Option<String>,
    pub depth: usize,
    pub jtag_axi: bool,
    pub search: TextEdit,
    pub role: ProbeRole,
    pub build: Option<Arc<DebugBuild>>,
    pub files: Option<DebugFiles>,
    pub target: Option<Arc<Mutex<HwDebugTarget>>>,
    pub snap: Option<TargetSnap>,
    // ILA panel
    pub trig: TrigMode,
    pub trig_value: TextEdit,
    pub trig_mask: TextEdit,
    pub window: TextEdit,
    pub pre: TextEdit,
    pub max_cycles: TextEdit,
    pub last_capture: Option<HwIlaCapture>,
    // VIO panel
    pub vio_cycles: TextEdit,
    /// Per `PROBE_OUT` name: the value field.
    pub vio_edits: Vec<(String, TextEdit)>,
    // AXI panel
    pub axi_kind: AxiTxnKind,
    pub axi_addr: TextEdit,
    pub axi_len: TextEdit,
    pub axi_data: TextEdit,
    pub axi_log: Vec<AxiLogRow>,
    pub axi_selected: Option<usize>,
    /// Named transactions from Tcl `create_hw_axi_txn`.
    pub axi_txns: Vec<helion_debug::HwAxiTxn>,
    pub last_error: Option<String>,
    job: Option<DbgJob>,
}

impl Default for HwDebugState {
    fn default() -> Self {
        Self {
            ila_nets: Vec::new(),
            vio_in: Vec::new(),
            vio_out: Vec::new(),
            trigger_net: None,
            depth: 64,
            jtag_axi: false,
            search: TextEdit::new(""),
            role: ProbeRole::Ila,
            build: None,
            files: None,
            target: None,
            snap: None,
            trig: TrigMode::Immediate,
            trig_value: TextEdit::new("0x0"),
            trig_mask: TextEdit::new(""),
            window: TextEdit::new("32"),
            pre: TextEdit::new("8"),
            max_cycles: TextEdit::new("4096"),
            last_capture: None,
            vio_cycles: TextEdit::new("16"),
            vio_edits: Vec::new(),
            axi_kind: AxiTxnKind::Read,
            axi_addr: TextEdit::new("0xC0000000"),
            axi_len: TextEdit::new("1"),
            axi_data: TextEdit::new(""),
            axi_log: Vec::new(),
            axi_selected: None,
            axi_txns: Vec::new(),
            last_error: None,
            job: None,
        }
    }
}

impl std::fmt::Debug for HwDebugState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HwDebugState")
            .field("ila_nets", &self.ila_nets)
            .field("built", &self.build.is_some())
            .field("programmed", &self.target.is_some())
            .field("job", &self.job.as_ref().map(|j| j.kind))
            .finish()
    }
}

impl HwDebugState {
    pub fn busy(&self) -> Option<DbgJobKind> {
        self.job.as_ref().map(|j| j.kind)
    }

    pub fn programmed(&self) -> bool {
        self.target.is_some()
    }

    pub fn role_of(&self, net: &str) -> Option<ProbeRole> {
        if self.ila_nets.iter().any(|n| n == net) {
            Some(ProbeRole::Ila)
        } else if self.vio_in.iter().any(|n| n == net) {
            Some(ProbeRole::VioIn)
        } else if self.vio_out.iter().any(|n| n == net) {
            Some(ProbeRole::VioOut)
        } else {
            None
        }
    }

    fn list_mut(&mut self, role: ProbeRole) -> &mut Vec<String> {
        match role {
            ProbeRole::Ila => &mut self.ila_nets,
            ProbeRole::VioIn => &mut self.vio_in,
            ProbeRole::VioOut => &mut self.vio_out,
        }
    }

    /// Add `net` to the current role (moving it from another list), or
    /// remove it when it already has that role.
    pub fn toggle(&mut self, net: &str) {
        let role = self.role;
        if self.role_of(net) == Some(role) {
            self.list_mut(role).retain(|n| n != net);
        } else {
            for r in ProbeRole::ALL {
                self.list_mut(r).retain(|n| n != net);
            }
            self.list_mut(role).push(net.to_string());
        }
        if self
            .trigger_net
            .as_ref()
            .is_some_and(|t| !self.ila_nets.contains(t))
        {
            self.trigger_net = None;
        }
    }

    /// Effective edge-trigger net.
    pub fn trigger_net(&self) -> Option<&str> {
        self.trigger_net
            .as_deref()
            .or(self.ila_nets.first().map(|s| s.as_str()))
    }

    fn sync_vio_edits(&mut self) {
        let outs: Vec<(String, u64)> = self
            .snap
            .as_ref()
            .map(|s| {
                s.vio
                    .iter()
                    .filter(|p| p.direction == LtxProbeDirection::ProbeOut)
                    .map(|p| (p.name.clone(), p.value))
                    .collect()
            })
            .unwrap_or_default();
        self.vio_edits.retain(|(n, _)| outs.iter().any(|(o, _)| o == n));
        for (name, v) in outs {
            if !self.vio_edits.iter().any(|(n, _)| *n == name) {
                self.vio_edits.push((name, TextEdit::new(format!("0x{v:X}"))));
            }
        }
    }

    pub fn vio_edit_mut(&mut self, name: &str) -> &mut TextEdit {
        if let Some(i) = self.vio_edits.iter().position(|(n, _)| n == name) {
            return &mut self.vio_edits[i].1;
        }
        self.vio_edits.push((name.to_string(), TextEdit::new("0x0")));
        let last = self.vio_edits.len() - 1;
        &mut self.vio_edits[last].1
    }

    /// Drop the build and target (project change).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

fn console(s: &mut AppState, text: impl Into<String>, sev: MsgSev) {
    s.console.lines.push(ConsoleLine {
        text: text.into(),
        sev,
    });
}

/// Parse `0x..`, `0b..` or decimal.
pub fn parse_num(text: &str) -> Result<u64, String> {
    let t = text.trim().replace('_', "");
    let r = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u64::from_str_radix(h, 16)
    } else if let Some(b) = t.strip_prefix("0b") {
        u64::from_str_radix(b, 2)
    } else if let Some(h) = t.strip_prefix("'h") {
        u64::from_str_radix(h, 16)
    } else {
        t.parse::<u64>()
    };
    r.map_err(|_| format!("not a number: {text:?}"))
}

// ---- candidates & gating ----

/// Nets a hardware probe can sample: FF `Q` outputs of the live design
/// (`mark_debug` nets first). The sim fabric reads probes at their driver
/// flop, so comb-only nets are not offered.
pub fn candidate_nets(s: &AppState) -> Vec<String> {
    let Some(d) = crate::live::live_design(s) else {
        return Vec::new();
    };
    let q = helion_ir::intern("Q");
    let mut marked = Vec::new();
    let mut rest = Vec::new();
    for c in &d.cells {
        let cname = helion_ir::resolve(c.name);
        if cname.starts_with("ila_") || cname.starts_with("u_ila") {
            continue;
        }
        if let Some(n) = d.net_on(c.name, q) {
            let name = helion_ir::resolve(n).to_string();
            let is_marked = d.net(&name).is_some_and(|net| net.attrs.flag("mark_debug"));
            let list = if is_marked { &mut marked } else { &mut rest };
            if !list.contains(&name) {
                list.push(name);
            }
        }
    }
    rest.sort();
    marked.sort();
    marked.extend(rest);
    marked
}

/// Nets carrying `mark_debug` in the live design.
pub fn marked_nets(s: &AppState) -> Vec<String> {
    crate::live::live_design(s)
        .map(|d| {
            d.marked_debug_nets()
                .into_iter()
                .map(|n| helion_ir::resolve(n).to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Why debug cores cannot run on the current cable (`None` = Simulation or
/// Native FTDI).
pub fn cable_block(s: &AppState) -> Option<String> {
    use crate::views::hw::CableChoice;
    match s.program.cable {
        CableChoice::Sim | CableChoice::Native => None,
        _ => Some(
            "Debug cores need USER1 data-register scans, which openFPGALoader cannot shift. \
             Select the Native FTDI or Simulation cable."
                .into(),
        ),
    }
}

/// Cable the next Program would use, as the panels label it.
pub fn planned_cable(s: &AppState) -> CableKind {
    if s.program.cable == crate::views::hw::CableChoice::Native {
        s.program.native.kind()
    } else {
        CableKind::Sim
    }
}

fn busy_block(s: &AppState) -> Option<String> {
    s.program
        .debug
        .busy()
        .map(|k| format!("Busy: {}", k.busy_label()))
}

pub fn implement_block(s: &AppState) -> Option<String> {
    if let Some(b) = busy_block(s) {
        return Some(b);
    }
    if crate::live::live_design(s).is_none() {
        return Some("No design yet. Run Synthesis first.".into());
    }
    if s.program.debug.ila_nets.is_empty() {
        return Some("Pick at least one ILA probe net.".into());
    }
    None
}

pub fn program_block(s: &AppState) -> Option<String> {
    if let Some(b) = busy_block(s) {
        return Some(b);
    }
    if s.program.debug.build.is_none() {
        return Some("Implement the debug cores first.".into());
    }
    cable_block(s)
}

pub fn target_block(s: &AppState) -> Option<String> {
    if let Some(b) = busy_block(s) {
        return Some(b);
    }
    if let Some(c) = cable_block(s) {
        return Some(c);
    }
    if s.program.debug.target.is_none() {
        return Some("Program the device with the debug bitstream first.".into());
    }
    None
}

fn core_block(s: &AppState, kind: DebugCoreKind) -> Option<String> {
    if let Some(b) = target_block(s) {
        return Some(b);
    }
    let has = s
        .program
        .debug
        .snap
        .as_ref()
        .is_some_and(|snap| snap.has(kind));
    if !has {
        return Some(format!(
            "The debug build has no {} core. Add it in Set Up Debug.",
            kind.as_str().to_ascii_uppercase()
        ));
    }
    None
}

pub fn ila_block(s: &AppState) -> Option<String> {
    core_block(s, DebugCoreKind::Ila)
}

pub fn vio_block(s: &AppState) -> Option<String> {
    core_block(s, DebugCoreKind::Vio)
}

pub fn axi_block(s: &AppState) -> Option<String> {
    core_block(s, DebugCoreKind::JtagAxi)
}

// ---- worker ----

fn spawn(
    s: &mut AppState,
    kind: DbgJobKind,
    body: impl FnOnce() -> DbgEvent + Send + 'static,
) -> bool {
    if s.program.debug.job.is_some() {
        return false;
    }
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name(format!("helion-hwdbg-{kind:?}").to_lowercase())
        .spawn(move || {
            let _ = tx.send(body());
        });
    match spawned {
        Ok(_) => {
            s.program.debug.job = Some(DbgJob {
                kind,
                rx: Arc::new(Mutex::new(rx)),
            });
            true
        }
        Err(e) => {
            console(s, format!("ERROR: debug worker: {e}"), MsgSev::Error);
            false
        }
    }
}

fn debug_dir(s: &AppState) -> PathBuf {
    crate::runs::checkpoint_root(s).join("debug_1")
}

/// Insert config from the Set Up Debug picks.
pub fn insert_config(s: &AppState) -> DebugInsertConfig {
    let d = &s.program.debug;
    let mut cfg = DebugInsertConfig::with_probes(d.ila_nets.clone());
    cfg.sample_depth = d.depth;
    cfg.trigger_net = d.trigger_net().map(str::to_string);
    cfg.vio_probes_in = d.vio_in.clone();
    cfg.vio_probes_out = d.vio_out.clone();
    cfg.insert_jtag_axi = d.jtag_axi;
    cfg
}

/// Set Up Debug → implement: insert cores, re-implement, write `.ltx` +
/// `.hbits` (worker).
pub fn implement(s: &mut AppState) -> Result<String, String> {
    if let Some(why) = implement_block(s) {
        return Err(format!("implement_debug_core: {why}"));
    }
    let design = crate::live::live_design(s).cloned().expect("checked");
    let (dev, part) = crate::live::live_device(s);
    let cfg = insert_config(s);
    let dir = debug_dir(s);
    let msg = format!(
        "implement_debug_core part={part} ila={} vio_in={} vio_out={} axi={} depth={}",
        cfg.explicit_probes.len(),
        cfg.vio_probes_in.len(),
        cfg.vio_probes_out.len(),
        u8::from(cfg.insert_jtag_axi),
        cfg.sample_depth
    );
    console(s, msg.clone(), MsgSev::Info);
    spawn(s, DbgJobKind::Build, move || {
        let r = helion_debug::implement_debug(&dev, &design, &cfg).and_then(|b| {
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("debug_1: create {}: {e}", dir.display()))?;
            let top = helion_ir::resolve(b.design.name).to_string();
            let ltx = dir.join(format!("{top}.ltx"));
            b.ltx().write_ltx(&ltx)?;
            let hbits = dir.join(format!("{top}_debug.hbits"));
            std::fs::write(&hbits, &b.bits.packets)
                .map_err(|e| format!("debug_1: write {}: {e}", hbits.display()))?;
            Ok((Arc::new(b), DebugFiles { ltx, hbits }))
        });
        DbgEvent::Built(r)
    });
    Ok(msg)
}

/// Program the debug bitstream over the selected cable and enumerate the hub.
pub fn program(s: &mut AppState) -> Result<String, String> {
    if let Some(why) = program_block(s) {
        return Err(format!("program_hw_devices: {why}"));
    }
    let build = s.program.debug.build.clone().expect("checked");
    let part = build.part.clone();
    let native = (s.program.cable == crate::views::hw::CableChoice::Native)
        .then(|| s.program.native.open_cfg());
    let cable = if native.is_some() { "native" } else { "sim" };
    console(
        s,
        format!("program_hw_devices debug bitstream part={part} cable={cable}"),
        MsgSev::Info,
    );
    // Re-programming releases the cable the old target held.
    s.program.debug.target = None;
    let msg = format!("program_hw_devices part={part} cable={cable}");
    spawn(s, DbgJobKind::Program, move || {
        let r = helion_device::Device::load_part(&part).and_then(|dev| {
            let t = match native {
                Some(cfg) => {
                    let (cable, _) = crate::views::hw_native::open_cable(&cfg)?;
                    HwDebugTarget::program_on(&dev, (*build).clone(), cable)?
                }
                None => HwDebugTarget::program_sim(&dev, (*build).clone())?,
            };
            let snap = TargetSnap::of(&t);
            Ok((Arc::new(Mutex::new(t)), snap))
        });
        DbgEvent::Programmed(r)
    });
    Ok(msg)
}

/// ILA panel fields → a run.
pub fn ila_run(s: &AppState) -> Result<HwIlaRun, String> {
    let d = &s.program.debug;
    let window = parse_num(&d.window.text)? as usize;
    let pre = parse_num(&d.pre.text)? as usize;
    let max_cycles = parse_num(&d.max_cycles.text)? as usize;
    let trigger = match d.trig {
        TrigMode::Immediate => HwTrigger::Immediate,
        TrigMode::Rising => HwTrigger::Rising,
        TrigMode::Falling => HwTrigger::Falling,
        TrigMode::Value => {
            let value = parse_num(&d.trig_value.text)?;
            let mask = if d.trig_mask.text.trim().is_empty() {
                u64::MAX
            } else {
                parse_num(&d.trig_mask.text)?
            };
            HwTrigger::Value { value, mask }
        }
    };
    if max_cycles == 0 || max_cycles > 1_000_000 {
        return Err("max cycles must be 1..=1000000".into());
    }
    Ok(HwIlaRun {
        trigger,
        window,
        pre_trigger: pre,
        max_cycles,
    })
}

/// `run_hw_ila`: arm, wait for the trigger, upload (worker).
pub fn run_ila(s: &mut AppState) -> Result<String, String> {
    if let Some(why) = ila_block(s) {
        return Err(format!("run_hw_ila: {why}"));
    }
    let run = ila_run(s)?;
    let target = s.program.debug.target.clone().expect("checked");
    let msg = format!(
        "run_hw_ila trigger={} window={} pre={}",
        run.trigger.label(),
        run.window,
        run.pre_trigger
    );
    console(s, msg.clone(), MsgSev::Info);
    spawn(s, DbgJobKind::Ila, move || {
        let mut t = target.lock().unwrap_or_else(|e| e.into_inner());
        let r = t.run_hw_ila(&run).map(|cap| (cap, TargetSnap::of(&t)));
        DbgEvent::Ila(r)
    });
    Ok(msg)
}

/// `refresh_hw_vio`: run the VIO refresh clocks and sample inputs (worker).
pub fn refresh_vio(s: &mut AppState) -> Result<String, String> {
    if let Some(why) = vio_block(s) {
        return Err(format!("refresh_hw_vio: {why}"));
    }
    let cycles = parse_num(&s.program.debug.vio_cycles.text)? as usize;
    if cycles > 1_000_000 {
        return Err("refresh_hw_vio: at most 1000000 clocks per refresh".into());
    }
    let target = s.program.debug.target.clone().expect("checked");
    spawn(s, DbgJobKind::Vio, move || {
        let mut t = target.lock().unwrap_or_else(|e| e.into_inner());
        DbgEvent::Vio(t.refresh_hw_vio(cycles).map(|_| TargetSnap::of(&t)))
    });
    Ok(format!("refresh_hw_vio cycles={cycles}"))
}

/// `set_property OUTPUT_VALUE v [get_hw_probes p]` + commit (worker).
pub fn set_output(s: &mut AppState, probe: &str, value: u64) -> Result<String, String> {
    if let Some(why) = vio_block(s) {
        return Err(format!("set_property OUTPUT_VALUE: {why}"));
    }
    let target = s.program.debug.target.clone().expect("checked");
    let p = probe.to_string();
    // Only a known probe gets its field rewritten (Tcl may name a net).
    if s.program.debug.vio_edits.iter().any(|(n, _)| n == probe) {
        *s.program.debug.vio_edit_mut(probe) = TextEdit::new(format!("{value:#X}").replace("0X", "0x"));
    }
    spawn(s, DbgJobKind::Vio, move || {
        let mut t = target.lock().unwrap_or_else(|e| e.into_inner());
        DbgEvent::Vio(t.set_vio_output(&p, value).map(|_| TargetSnap::of(&t)))
    });
    Ok(format!("set_property OUTPUT_VALUE {value:#x} {probe}"))
}

/// Stop overriding a `PROBE_OUT` net.
pub fn release_output(s: &mut AppState, probe: &str) -> Result<String, String> {
    if let Some(why) = vio_block(s) {
        return Err(why);
    }
    let target = s.program.debug.target.clone().expect("checked");
    let p = probe.to_string();
    spawn(s, DbgJobKind::Vio, move || {
        let mut t = target.lock().unwrap_or_else(|e| e.into_inner());
        DbgEvent::Vio(t.release_vio_output(&p).map(|_| TargetSnap::of(&t)))
    });
    Ok(format!("release {probe}"))
}

/// Clear sticky activity on the VIO inputs.
pub fn reset_activity(s: &mut AppState) -> Result<String, String> {
    if let Some(why) = vio_block(s) {
        return Err(why);
    }
    let target = s.program.debug.target.clone().expect("checked");
    spawn(s, DbgJobKind::Vio, move || {
        let mut t = target.lock().unwrap_or_else(|e| e.into_inner());
        t.reset_vio_activity();
        DbgEvent::Vio(Ok(TargetSnap::of(&t)))
    });
    Ok("reset_hw_vio_activity".into())
}

pub fn parse_words(text: &str) -> Result<Vec<u32>, String> {
    text.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let v = parse_num(w)?;
            u32::try_from(v).map_err(|_| format!("{w} does not fit 32 bits"))
        })
        .collect()
}

/// Build a transaction from the AXI console fields.
pub fn console_txn(s: &AppState) -> Result<helion_debug::HwAxiTxn, String> {
    let d = &s.program.debug;
    let addr = u32::try_from(parse_num(&d.axi_addr.text)?)
        .map_err(|_| "address must fit 32 bits".to_string())?;
    let len = parse_num(&d.axi_len.text)? as usize;
    let data = if d.axi_kind == AxiTxnKind::Write {
        let w = parse_words(&d.axi_data.text)?;
        if w.len() == 1 && len > 1 {
            vec![w[0]; len]
        } else {
            w
        }
    } else {
        Vec::new()
    };
    let n = d.axi_log.len() + 1;
    create_hw_axi_txn(format!("txn_{n}"), addr, d.axi_kind, len, data)
}

/// `run_hw_axi` (worker); the result lands in the transaction log.
pub fn run_axi(s: &mut AppState, mut txn: helion_debug::HwAxiTxn) -> Result<String, String> {
    if let Some(why) = axi_block(s) {
        return Err(format!("run_hw_axi: {why}"));
    }
    let target = s.program.debug.target.clone().expect("checked");
    let msg = format!(
        "run_hw_axi {} {} {:#010x} len={}",
        txn.name,
        if txn.kind == AxiTxnKind::Read {
            "read"
        } else {
            "write"
        },
        txn.addr,
        txn.len
    );
    console(s, msg.clone(), MsgSev::Info);
    spawn(s, DbgJobKind::Axi, move || {
        let mut t = target.lock().unwrap_or_else(|e| e.into_inner());
        let resp = t.run_hw_axi(&mut txn);
        let data = if txn.kind == AxiTxnKind::Read {
            txn.read_data.clone()
        } else {
            txn.write_data.clone()
        };
        let row = AxiLogRow {
            name: txn.name.clone(),
            kind: txn.kind,
            addr: txn.addr,
            len: txn.len,
            data,
            resp,
            dr_scans: txn.dr_scans,
        };
        DbgEvent::Axi(Box::new(row), Some(TargetSnap::of(&t)))
    });
    Ok(msg)
}

/// Console-field transaction, reporting field errors in the log.
pub fn run_console_txn(s: &mut AppState) {
    match console_txn(s) {
        Ok(txn) => {
            if let Err(e) = run_axi(s, txn) {
                console(s, format!("ERROR: {e}"), MsgSev::Error);
            }
        }
        Err(e) => {
            console(s, format!("ERROR: create_hw_axi_txn: {e}"), MsgSev::Error);
            s.program.debug.last_error = Some(e);
        }
    }
}

// ---- hardware capture → the ILA viewer ----

/// Convert an uploaded window into the viewer's capture, one probe per
/// `.ltx` channel (buses grouped, MSB-first like the simulation ILA).
pub fn to_viewer(cap: &HwIlaCapture, build: &DebugBuild) -> helion_verif::ila::IlaCapture {
    use helion_verif::ila::{IlaCapture, IlaProbeCfg, IlaRadix};
    let channels = build
        .core("ila")
        .map(|c| c.probes.clone())
        .unwrap_or_default();
    let mut probes = Vec::new();
    let mut samples = Vec::new();
    for ch in &channels {
        let idx: Vec<usize> = ch
            .nets
            .iter()
            .filter_map(|n| cap.nets.iter().position(|m| m == n))
            .collect();
        let mut msb_first: Vec<String> = ch.nets.clone();
        msb_first.reverse();
        probes.push(IlaProbeCfg {
            name: ch.display_signal(),
            nets: msb_first,
            radix: if ch.width > 1 {
                IlaRadix::Hex
            } else {
                IlaRadix::Bin
            },
        });
        samples.push(
            cap.words
                .iter()
                .map(|w| {
                    idx.iter()
                        .enumerate()
                        .fold(0u64, |acc, (bit, &i)| acc | (((w >> i) & 1) << bit))
                })
                .collect::<Vec<u64>>(),
        );
    }
    let n = cap.words.len();
    IlaCapture {
        valid: vec![vec![true; n]; probes.len()],
        probes,
        samples,
        timescale_ps: 10_000,
        clock_net: build.report.clock_net.clone(),
        trigger_sample: Some(cap.trigger_at),
        cycles_run: cap.cycles as u32,
        found_trigger: true,
    }
}

/// `upload_hw_ila_data` + `display_hw_ila_data`: show the last hardware
/// window in the Logic Analyzer, labelled as a hardware capture.
pub fn show_in_viewer(s: &mut AppState) -> Result<String, String> {
    let cap = s
        .program
        .debug
        .last_capture
        .clone()
        .ok_or("display_hw_ila_data: no hardware capture yet (run_hw_ila first)")?;
    let build = s
        .program
        .debug
        .build
        .clone()
        .ok_or("display_hw_ila_data: no debug build")?;
    let view = to_viewer(&cap, &build);
    let cable = s
        .program
        .debug
        .snap
        .as_ref()
        .map(|t| t.cable.label())
        .unwrap_or(CableKind::Sim.label());
    let label = format!("{} · {} · {cable}", cap.core, build.part);
    let trigger = match cap.trigger {
        HwTrigger::Value { value, mask } => format!("value {value:#x} & {mask:#x}"),
        t => t.label().to_string(),
    };
    let trigger = format!("{trigger} @ sample {}", cap.trigger_at);
    s.ila.mount_hardware_capture(view, label, trigger);
    s.center = CenterPane::Ila;
    Ok(format!(
        "display_hw_ila_data {} samples={} trigger@{}",
        cap.core,
        cap.words.len(),
        cap.trigger_at
    ))
}

// ---- events ----

fn fail(s: &mut AppState, title: &str, e: String) {
    console(s, format!("ERROR: {e}"), MsgSev::Error);
    let detail: String = e.chars().take(140).collect();
    s.toasts.push(ToastKind::Error, title, &detail);
    s.program.debug.last_error = Some(e);
}

pub fn deliver(s: &mut AppState, ev: DbgEvent) {
    s.program.debug.job = None;
    match ev {
        DbgEvent::Built(Ok((b, files))) => {
            let line = format!(
                "debug cores implemented: {} probe(s), {} core(s), LUTFF {}→{}, ltx {}",
                b.ila_nets().len(),
                b.report.hub.slots.len(),
                b.baseline_lutffs,
                b.debug_lutffs,
                files.ltx.display()
            );
            console(s, line, MsgSev::Info);
            let d = &mut s.program.debug;
            d.build = Some(b);
            d.files = Some(files);
            // A new bitstream invalidates the programmed target.
            d.target = None;
            d.snap = None;
            d.last_capture = None;
            d.last_error = None;
            s.toasts
                .push(ToastKind::Success, "Set Up Debug", "Debug cores implemented");
        }
        DbgEvent::Built(Err(e)) => fail(s, "Set Up Debug failed", e),
        DbgEvent::Programmed(Ok((t, snap))) => {
            console(
                s,
                format!(
                    "program_hw_devices: DONE={} · {} debug core(s) on the hub · {}",
                    u8::from(snap.done),
                    snap.cores.len(),
                    snap.cable.label()
                ),
                MsgSev::Info,
            );
            let d = &mut s.program.debug;
            d.target = Some(t);
            d.snap = Some(snap);
            d.sync_vio_edits();
            d.last_error = None;
            s.program.node = crate::views::hw_tree::HwNode::Core(DebugCoreKind::Ila);
        }
        DbgEvent::Programmed(Err(e)) => fail(s, "Program debug target failed", e),
        DbgEvent::Ila(Ok((cap, snap))) => {
            console(
                s,
                format!(
                    "upload_hw_ila_data {}: {} samples, trigger@{} after {} clocks · {} USR1 scans (raw {})",
                    cap.core,
                    cap.words.len(),
                    cap.trigger_at,
                    cap.cycles,
                    cap.upload_dr_scans,
                    cap.upload_raw_scans
                ),
                MsgSev::Info,
            );
            s.program.debug.snap = Some(snap);
            s.program.debug.last_capture = Some(cap);
            s.program.debug.last_error = None;
        }
        DbgEvent::Ila(Err(e)) => fail(s, "ILA capture failed", e),
        DbgEvent::Vio(Ok(snap)) => {
            s.program.debug.snap = Some(snap);
            s.program.debug.sync_vio_edits();
            s.program.debug.last_error = None;
        }
        DbgEvent::Vio(Err(e)) => fail(s, "VIO failed", e),
        DbgEvent::Axi(row, snap) => {
            let line = match &row.resp {
                Ok(r) => format!(
                    "run_hw_axi {}: {} · {} DR scans",
                    row.name,
                    r.as_str(),
                    row.dr_scans
                ),
                Err(e) => format!("ERROR: run_hw_axi {}: {e}", row.name),
            };
            let sev = match &row.resp {
                Ok(AxiResp::Okay) => MsgSev::Info,
                Ok(_) => MsgSev::Warn,
                Err(_) => MsgSev::Error,
            };
            console(s, line, sev);
            let d = &mut s.program.debug;
            if let Some(snap) = snap {
                d.snap = Some(snap);
            }
            d.axi_log.insert(0, *row);
            d.axi_log.truncate(AXI_LOG_MAX);
            d.axi_selected = Some(0);
        }
    }
}

/// Drain the debug worker (called from [`crate::views::hw::poll`]).
pub fn poll(s: &mut AppState) -> bool {
    let Some(job) = s.program.debug.job.clone() else {
        return false;
    };
    let next = job.rx.lock().unwrap_or_else(|e| e.into_inner()).try_recv();
    match next {
        Ok(ev) => {
            deliver(s, ev);
            true
        }
        Err(mpsc::TryRecvError::Empty) => false,
        Err(mpsc::TryRecvError::Disconnected) => {
            s.program.debug.job = None;
            fail(s, "Debug worker", "debug worker exited unexpectedly".into());
            true
        }
    }
}

/// Block until the debug worker is idle (Tcl `wait_on_hw_ila`, tests).
pub fn wait_idle(s: &mut AppState, timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    while s.program.debug.job.is_some() {
        if start.elapsed() > timeout {
            return false;
        }
        if !poll(s) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    true
}

/// How far [`seed_debug_fixture`] takes the fixture (scenes and tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FixtureStage {
    /// Probes picked, nothing built.
    Picked,
    /// Built, programmed, one ILA window, VIO refreshed + one output
    /// driven, two AXI transactions.
    Live,
}

/// Seed the real fixture design and run the real debug flow up to `stage`
/// (each step through the worker, waited on). Scenes/tests only.
pub fn seed_debug_fixture(s: &mut AppState, stage: FixtureStage) -> Result<(), String> {
    seed_debug_fixture_on(s, stage, false)
}

/// [`seed_debug_fixture`] on the Native FTDI cable wired to the in-process
/// mock FTDI board (every TAP access is real MPSSE traffic). Scenes/tests.
pub fn seed_debug_fixture_native(s: &mut AppState, stage: FixtureStage) -> Result<(), String> {
    seed_debug_fixture_on(s, stage, true)
}

fn seed_debug_fixture_on(
    s: &mut AppState,
    stage: FixtureStage,
    native: bool,
) -> Result<(), String> {
    use std::time::Duration;
    crate::live::seed_live_fixture(s)?;
    if native {
        let (_, part) = crate::live::live_device(s);
        crate::views::hw_native::use_mock_board(
            s,
            crate::views::hw_native::MockBoard::Helion { part, flash: None },
        )?;
    } else {
        s.program.cable = crate::views::hw::CableChoice::Sim;
    }
    let nets = candidate_nets(s);
    if nets.len() < 6 {
        return Err(format!("fixture: too few flop nets {nets:?}"));
    }
    let d = &mut s.program.debug;
    *d = HwDebugState::default();
    for n in nets.iter().take(4) {
        d.toggle(n);
    }
    d.role = ProbeRole::VioIn;
    d.toggle(&nets[4]);
    d.role = ProbeRole::VioOut;
    d.toggle(&nets[5]);
    d.role = ProbeRole::Ila;
    d.jtag_axi = true;
    d.depth = 32;
    if stage == FixtureStage::Picked {
        return Ok(());
    }
    let wait = |s: &mut AppState| -> Result<(), String> {
        if !wait_idle(s, Duration::from_secs(120)) {
            return Err("fixture: debug worker timed out".into());
        }
        match s.program.debug.last_error.clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    };
    implement(s)?;
    wait(s)?;
    program(s)?;
    wait(s)?;
    s.program.debug.trig = TrigMode::Value;
    s.program.debug.trig_value = TextEdit::new("0x5");
    s.program.debug.trig_mask = TextEdit::new("0xF");
    s.program.debug.window = TextEdit::new("24");
    s.program.debug.pre = TextEdit::new("6");
    run_ila(s)?;
    wait(s)?;
    s.program.debug.vio_cycles = TextEdit::new("5");
    refresh_vio(s)?;
    wait(s)?;
    let out = s
        .program
        .debug
        .snap
        .as_ref()
        .and_then(|sn| {
            sn.vio
                .iter()
                .find(|p| p.direction == LtxProbeDirection::ProbeOut)
                .map(|p| p.name.clone())
        })
        .ok_or("fixture: no VIO output")?;
    set_output(s, &out, 1)?;
    wait(s)?;
    refresh_vio(s)?;
    wait(s)?;
    let base = s
        .program
        .debug
        .snap
        .as_ref()
        .and_then(|sn| sn.axi_regions.first().map(|r| r.1))
        .ok_or("fixture: no AXI aperture")?;
    let wr = create_hw_axi_txn("txn_1", base + 0x40, AxiTxnKind::Write, 4, vec![
        0xC0DE_0001,
        0xC0DE_0002,
        0xC0DE_0003,
        0xC0DE_0004,
    ])?;
    run_axi(s, wr)?;
    wait(s)?;
    let rd = create_hw_axi_txn("txn_2", base + 0x40, AxiTxnKind::Read, 4, Vec::new())?;
    run_axi(s, rd)?;
    wait(s)?;
    let bad = create_hw_axi_txn("txn_3", 0x4000_0000, AxiTxnKind::Read, 1, Vec::new())?;
    run_axi(s, bad)?;
    wait(s)?;
    s.program.debug.axi_addr = TextEdit::new(format!("{base:#010x}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    pub fn debug_state() -> AppState {
        let mut s = AppState::demo();
        crate::live::seed_live_fixture(&mut s).expect("fixture");
        s.program.cable = crate::views::hw::CableChoice::Sim;
        s.runs.checkpoint_root = Some(
            std::env::temp_dir().join(format!("helion-rune-hwdbg-{}", std::process::id())),
        );
        s
    }

    fn q_nets(s: &AppState) -> Vec<String> {
        let c = candidate_nets(s);
        assert!(c.len() >= 4, "fixture has counter flops: {c:?}");
        c
    }

    #[test]
    fn candidates_are_flop_outputs() {
        let s = debug_state();
        let c = q_nets(&s);
        let d = crate::live::live_design(&s).unwrap();
        for n in &c {
            assert!(d.net(n).is_some(), "{n}");
        }
    }

    #[test]
    fn toggle_moves_nets_between_roles() {
        let mut d = HwDebugState::default();
        d.toggle("a");
        assert_eq!(d.ila_nets, vec!["a"]);
        d.role = ProbeRole::VioIn;
        d.toggle("a");
        assert!(d.ila_nets.is_empty());
        assert_eq!(d.vio_in, vec!["a"]);
        d.toggle("a");
        assert!(d.vio_in.is_empty());
    }

    #[test]
    fn ofl_cables_block_debug_with_a_reason() {
        let mut s = debug_state();
        s.program.cable = crate::views::hw::CableChoice::Usb;
        s.program.debug.build = None;
        assert!(program_block(&s).is_some());
        assert!(cable_block(&s).unwrap().contains("USER1"));
        s.program.cable = crate::views::hw::CableChoice::Native;
        assert!(cable_block(&s).is_none(), "native FTDI is allowed");
        assert_eq!(planned_cable(&s), CableKind::NativeUsb);
    }

    #[test]
    fn native_cable_full_flow_over_mock_ftdi() {
        let mut s = debug_state();
        seed_debug_fixture_native(&mut s, FixtureStage::Live).unwrap();
        let snap = s.program.debug.snap.clone().unwrap();
        assert_eq!(snap.cable, CableKind::NativeMock);
        assert!(snap.done && snap.scans.1 > 0, "{snap:?}");
        let cap = s.program.debug.last_capture.clone().unwrap();
        assert_eq!(cap.words.len(), 24);
        assert_eq!(s.program.debug.axi_log.len(), 3);
        assert!(crate::views::hw_native::held_by_debug(&s));
        assert!(crate::views::hw_native::scan_block(&s).unwrap().contains("Close Target"));
        crate::views::hw_native::close_target(&mut s).unwrap();
        assert!(crate::views::hw_native::scan_block(&s).is_none());
        // The released board answers a fresh chain scan.
        crate::views::hw_native::tcl_verb(&mut s, "refresh_hw_target", &[]).unwrap();
        assert_eq!(s.program.native.chain.as_ref().unwrap().devices.len(), 1);
    }

    #[test]
    fn native_program_without_a_cable_fails_honestly() {
        let mut s = debug_state();
        seed_debug_fixture(&mut s, FixtureStage::Picked).unwrap();
        implement(&mut s).unwrap();
        assert!(wait_idle(&mut s, Duration::from_secs(120)));
        s.program.cable = crate::views::hw::CableChoice::Native;
        program(&mut s).unwrap();
        assert!(wait_idle(&mut s, Duration::from_secs(30)));
        // No FTDI on this host: no target, and the error says why.
        if s.program.debug.target.is_none() {
            let e = s.program.debug.last_error.clone().unwrap();
            assert!(e.contains("FTDI") || e.contains("usb"), "{e}");
        }
    }

    #[test]
    fn full_flow_implement_program_capture_vio_axi() {
        let mut s = debug_state();
        let nets = q_nets(&s);
        for n in nets.iter().take(4) {
            s.program.debug.toggle(n);
        }
        s.program.debug.role = ProbeRole::VioIn;
        let vio_in = nets[4 % nets.len()].clone();
        if s.program.debug.role_of(&vio_in).is_none() {
            s.program.debug.toggle(&vio_in);
        }
        s.program.debug.jtag_axi = true;
        s.program.debug.depth = 32;
        implement(&mut s).unwrap();
        assert!(wait_idle(&mut s, Duration::from_secs(120)));
        assert!(s.program.debug.last_error.is_none(), "{:?}", s.program.debug.last_error);
        let files = s.program.debug.files.clone().unwrap();
        assert!(files.ltx.is_file() && files.hbits.is_file());
        assert!(files.ltx.starts_with(std::env::temp_dir()));
        program(&mut s).unwrap();
        assert!(wait_idle(&mut s, Duration::from_secs(60)));
        let snap = s.program.debug.snap.clone().unwrap();
        assert!(snap.done);
        assert!(snap.has(DebugCoreKind::Ila));
        assert!(snap.has(DebugCoreKind::JtagAxi));

        s.program.debug.window = TextEdit::new("16");
        run_ila(&mut s).unwrap();
        assert!(wait_idle(&mut s, Duration::from_secs(60)));
        let cap = s.program.debug.last_capture.clone().expect("capture");
        assert_eq!(cap.words.len(), 16);
        show_in_viewer(&mut s).unwrap();
        assert_eq!(s.center, CenterPane::Ila);
        assert!(s.ila.hardware_label().is_some());
        assert_eq!(s.ila.capture.as_ref().unwrap().len(), 16);

        refresh_vio(&mut s).unwrap();
        assert!(wait_idle(&mut s, Duration::from_secs(60)));
        assert!(s.program.debug.last_error.is_none());

        let base = s.program.debug.snap.as_ref().unwrap().axi_regions[0].1;
        s.program.debug.axi_kind = AxiTxnKind::Write;
        s.program.debug.axi_addr = TextEdit::new(format!("{base:#x}"));
        s.program.debug.axi_len = TextEdit::new("2");
        s.program.debug.axi_data = TextEdit::new("0x1234 0x5678");
        run_console_txn(&mut s);
        assert!(wait_idle(&mut s, Duration::from_secs(30)));
        s.program.debug.axi_kind = AxiTxnKind::Read;
        run_console_txn(&mut s);
        assert!(wait_idle(&mut s, Duration::from_secs(30)));
        let row = &s.program.debug.axi_log[0];
        assert_eq!(row.resp, Ok(AxiResp::Okay));
        assert_eq!(row.data, vec![0x1234, 0x5678]);
    }
}
