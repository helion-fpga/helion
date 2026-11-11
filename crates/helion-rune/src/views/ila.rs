//! Logic Analyzer (simulation ILA): probe/trigger model, background capture,
//! session files, VCD export, and the `ila` Tcl verb.
//!
//! The model ports the old IDE `Ila` (`helion-gui/src/ide.rs`, probe lists,
//! `add_probe`, `group_checked_as_bus`, `set_trigger_from_wave`,
//! `start_capture`, `disarm`, `save_session`/`load_session`) onto the
//! UI-free engine in `helion-verif` (`run_sim_ila_capture`,
//! `spawn_sim_ila_capture`, `IlaViewer`, `suggest_trigger_at`,
//! `export_vcd`, session JSON). Captures run on the engine's `helion-ila`
//! worker; [`poll`] merges the result and keeps the "capturing… N cycles"
//! status live. Rendering lives in [`crate::views::ila_view`].
//!
//! The design under test is the live session design
//! ([`crate::live::live_design`]: the last run, else the Tcl elaboration).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, atomic::Ordering, mpsc};

use helion_verif::ila::{
    self as engine, IlaCapture, IlaConfig, IlaProbeCfg, IlaRadix, IlaRunProgress, IlaTriggerKind,
    IlaViewer,
};
use rune::{FileDialogOptions, FileDialogResult, TextEdit, Tx};

use crate::state::{AppState, CenterPane, ConsoleLine, MsgSev, ToastKind};

/// Default canvas width before the first layout report.
pub const DEFAULT_VIEW_W: f32 = 360.0;
/// Net-list rows shown before the "refine the search" note.
pub const NET_LIST_MAX: usize = 200;

/// Capture lifecycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IlaStatus {
    #[default]
    Idle,
    Capturing,
    Done,
    Error,
}

impl IlaStatus {
    pub fn label(self) -> &'static str {
        match self {
            IlaStatus::Idle => "Idle",
            IlaStatus::Capturing => "Capturing",
            IlaStatus::Done => "Captured",
            IlaStatus::Error => "Error",
        }
    }
}

/// Trigger kinds offered by the settings strip, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrigChoice {
    Immediate,
    Rising,
    Falling,
    Value,
}

impl TrigChoice {
    pub const ALL: [TrigChoice; 4] = [
        TrigChoice::Immediate,
        TrigChoice::Rising,
        TrigChoice::Falling,
        TrigChoice::Value,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TrigChoice::Immediate => "Immediate",
            TrigChoice::Rising => "Rising",
            TrigChoice::Falling => "Falling",
            TrigChoice::Value => "Value",
        }
    }

    pub fn of(kind: &IlaTriggerKind) -> Self {
        match kind {
            IlaTriggerKind::Immediate => TrigChoice::Immediate,
            IlaTriggerKind::Rising => TrigChoice::Rising,
            IlaTriggerKind::Falling => TrigChoice::Falling,
            IlaTriggerKind::ValueMatch { .. } => TrigChoice::Value,
        }
    }
}

#[derive(Clone)]
struct ActiveCapture {
    rx: Arc<Mutex<mpsc::Receiver<Result<IlaCapture, String>>>>,
    progress: Arc<IlaRunProgress>,
}

impl std::fmt::Debug for ActiveCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveCapture")
            .field("cycles", &self.progress.cycles.load(Ordering::Relaxed))
            .finish()
    }
}

/// Logic Analyzer state: configuration, its text fields, the last capture,
/// viewer zoom/cursors, and the at-most-one background capture.
#[derive(Clone, Debug)]
pub struct IlaState {
    pub config: IlaConfig,
    pub capture: Option<IlaCapture>,
    pub status: IlaStatus,
    active: Option<ActiveCapture>,
    /// Net search in the probe picker.
    pub search: TextEdit,
    /// Per-probe "group into bus" checks (parallel to `config.probes`).
    pub group: Vec<bool>,
    pub bus_name: TextEdit,
    /// Value-match fields (hex `0x..` or decimal; empty mask = all bits).
    pub trig_value: TextEdit,
    pub trig_mask: TextEdit,
    pub depth: TextEdit,
    pub pre_pct: TextEdit,
    pub max_cycles: TextEdit,
    pub timescale: TextEdit,
    pub viewer: IlaViewer,
    /// Refit the viewer once the canvas reports its width.
    pub fit_pending: bool,
    /// Probe row selected in the capture viewer.
    pub selected_probe: Option<usize>,
    pub last_error: Option<String>,
    /// Last saved/loaded session file.
    pub session_path: Option<PathBuf>,
}

impl Default for IlaState {
    fn default() -> Self {
        let config = IlaConfig {
            depth: 256,
            ..IlaConfig::default()
        };
        let mut s = Self {
            config,
            capture: None,
            status: IlaStatus::Idle,
            active: None,
            search: TextEdit::new(""),
            group: Vec::new(),
            bus_name: TextEdit::new(""),
            trig_value: TextEdit::new("1"),
            trig_mask: TextEdit::new(""),
            depth: TextEdit::new(""),
            pre_pct: TextEdit::new(""),
            max_cycles: TextEdit::new(""),
            timescale: TextEdit::new(""),
            viewer: IlaViewer::fit(1, DEFAULT_VIEW_W),
            fit_pending: false,
            selected_probe: None,
            last_error: None,
            session_path: None,
        };
        s.sync_fields();
        s
    }
}

impl IlaState {
    /// Mirror the numeric config into its text fields.
    pub fn sync_fields(&mut self) {
        self.depth = TextEdit::new(self.config.depth.to_string());
        self.pre_pct = TextEdit::new(((self.config.pre_ratio * 100.0).round() as u32).to_string());
        self.max_cycles = TextEdit::new(self.config.max_cycles.to_string());
        self.timescale = TextEdit::new(self.config.timescale_ps.to_string());
        if let IlaTriggerKind::ValueMatch { value, mask } = self.config.trigger.kind {
            self.trig_value = TextEdit::new(value.to_string());
            self.trig_mask = TextEdit::new(format!("0x{mask:X}"));
        }
        self.group.resize(self.config.probes.len(), false);
    }

    pub fn is_capturing(&self) -> bool {
        self.active.is_some()
    }

    /// Posedges simulated by the capture in flight.
    pub fn progress_cycles(&self) -> u32 {
        self.active
            .as_ref()
            .map(|a| a.progress.cycles.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    pub fn trigger_probe_name(&self) -> String {
        self.config
            .probes
            .get(self.config.trigger.probe)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "-".into())
    }

    /// Parse the settings fields into the config (all or nothing).
    pub fn apply_fields(&mut self) -> Result<(), String> {
        let depth: usize = self
            .depth
            .text
            .trim()
            .parse()
            .map_err(|_| format!("Depth: not a number: {}", self.depth.text.trim()))?;
        if depth == 0 || depth > engine::MAX_DEPTH {
            return Err(format!("Depth must be 1..={}", engine::MAX_DEPTH));
        }
        let pre: f32 = self
            .pre_pct
            .text
            .trim()
            .trim_end_matches('%')
            .parse()
            .map_err(|_| {
                format!(
                    "Pre-trigger: not a percentage: {}",
                    self.pre_pct.text.trim()
                )
            })?;
        if !(0.0..=100.0).contains(&pre) {
            return Err("Pre-trigger must be 0..=100 %".into());
        }
        let max_cycles: u32 =
            self.max_cycles.text.trim().parse().map_err(|_| {
                format!("Max cycles: not a number: {}", self.max_cycles.text.trim())
            })?;
        let ts = crate::views::sim_settings::parse_ps(&self.timescale.text)
            .map_err(|e| format!("Timescale: {e}"))?;
        let kind = match self.config.trigger.kind {
            IlaTriggerKind::ValueMatch { .. } => self.value_match()?,
            k => k,
        };
        self.config.depth = depth;
        self.config.pre_ratio = pre / 100.0;
        self.config.max_cycles = max_cycles;
        self.config.timescale_ps = ts;
        self.config.trigger.kind = kind;
        Ok(())
    }

    /// Value-match trigger from the value/mask fields; an empty mask means
    /// every bit of the trigger probe.
    pub fn value_match(&self) -> Result<IlaTriggerKind, String> {
        let width = self
            .config
            .probes
            .get(self.config.trigger.probe)
            .map(|p| p.width())
            .unwrap_or(64);
        let value = engine::parse_u64(&self.trig_value.text)?;
        let mask = if self.trig_mask.text.trim().is_empty() {
            engine::width_mask(width)
        } else {
            engine::parse_u64(&self.trig_mask.text)?
        };
        Ok(IlaTriggerKind::ValueMatch { value, mask })
    }

    fn refit(&mut self) {
        let n = self.capture.as_ref().map(|c| c.len()).unwrap_or(1).max(1);
        let w = if self.viewer.view_w > 1.0 {
            self.viewer.view_w
        } else {
            DEFAULT_VIEW_W
        };
        let (c, a, b) = (
            self.viewer.cursor,
            self.viewer.cursor_a,
            self.viewer.cursor_b,
        );
        self.viewer = IlaViewer::fit(n, w);
        self.viewer.cursor = c;
        self.viewer.cursor_a = a;
        self.viewer.cursor_b = b;
        self.viewer.clamp(n);
    }

    /// Fit the whole capture into the canvas.
    pub fn zoom_fit(&mut self) {
        self.refit();
        self.fit_pending = false;
    }

    /// Mount a finished (or loaded) capture: fit, cursor on the trigger.
    fn mount_capture(&mut self, cap: IlaCapture) {
        let trig = cap.trigger_sample;
        self.capture = Some(cap);
        self.viewer.cursor = trig;
        self.viewer.cursor_a = None;
        self.viewer.cursor_b = None;
        self.selected_probe = None;
        self.refit();
        self.fit_pending = true;
        self.status = IlaStatus::Done;
        self.last_error = None;
    }

    /// Canvas width report from layout.
    pub fn set_view_width(&mut self, w: f32) {
        if (self.viewer.view_w - w).abs() < 0.5 {
            return;
        }
        self.viewer.view_w = w.max(1.0);
        if self.fit_pending {
            self.refit();
            self.fit_pending = false;
        }
    }

    /// Visible fractional sample range `[first, last)`.
    pub fn visible(&self) -> (f64, f64) {
        let first = self.viewer.first;
        (first, first + self.viewer.view_w as f64 * self.viewer.spp)
    }
}

fn design(s: &AppState) -> Result<helion_ir::Design, String> {
    crate::live::live_design(s)
        .cloned()
        .ok_or_else(|| "ila: no design (run synthesis or elaborate first)".to_string())
}

/// Nets available as probes for the current search.
pub fn probe_nets(s: &AppState) -> Vec<String> {
    crate::live::live_design(s)
        .map(|d| engine::probe_net_list(d, &s.ila.search.text))
        .unwrap_or_default()
}

/// Clock-domain candidates of the live design.
pub fn clock_nets(s: &AppState) -> Vec<String> {
    crate::live::live_design(s)
        .map(engine::clock_net_list)
        .unwrap_or_default()
}

/// Add net `net` as a scalar probe (validated against the design).
pub fn add_probe(s: &mut AppState, net: &str) -> Result<String, String> {
    let net = net.trim();
    if net.is_empty() {
        return Err("ila add: empty net".into());
    }
    let d = design(s)?;
    if s.ila.config.probes.iter().any(|p| p.nets == [net]) {
        return Ok(format!("ila probe {net} (already)"));
    }
    let mut cfg = s.ila.config.clone();
    cfg.probes.push(IlaProbeCfg::scalar(net, net));
    validate_probes(&cfg, &d)?;
    s.ila.config = cfg;
    s.ila.group.push(false);
    Ok(format!("ila probe {net}"))
}

/// Probe-only validation (trigger/depth checks run at arm time).
fn validate_probes(cfg: &IlaConfig, d: &helion_ir::Design) -> Result<(), String> {
    let mut probe_only = cfg.clone();
    probe_only.trigger.kind = IlaTriggerKind::Immediate;
    probe_only.trigger.probe = 0;
    probe_only.max_cycles = probe_only.max_cycles.max(probe_only.depth as u32);
    match probe_only.validate(d) {
        Err(e) if e.contains("probe") || e.contains("net") => Err(e),
        _ => Ok(()),
    }
}

/// Add a named bus probe over `nets` (MSB first).
pub fn add_bus(s: &mut AppState, name: &str, nets: &[String]) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("ila bus: empty name".into());
    }
    if nets.is_empty() {
        return Err("ila bus: no nets".into());
    }
    let d = design(s)?;
    let mut cfg = s.ila.config.clone();
    cfg.probes.push(IlaProbeCfg {
        name: name.into(),
        nets: nets.to_vec(),
        radix: IlaRadix::Hex,
    });
    validate_probes(&cfg, &d)?;
    s.ila.config = cfg;
    s.ila.group.push(false);
    Ok(format!("ila bus {name} width={}", nets.len()))
}

/// Remove probe `idx` (the trigger falls back to probe 0 when it pointed
/// past the end).
pub fn remove_probe(s: &mut AppState, idx: usize) -> Result<String, String> {
    let ila = &mut s.ila;
    if idx >= ila.config.probes.len() {
        return Err(format!("ila: no probe {idx}"));
    }
    let name = ila.config.probes.remove(idx).name;
    if idx < ila.group.len() {
        ila.group.remove(idx);
    }
    let t = &mut ila.config.trigger.probe;
    if *t == idx {
        *t = 0;
    } else if *t > idx {
        *t -= 1;
    }
    if *t >= ila.config.probes.len() {
        *t = 0;
    }
    Ok(format!("ila probe {name} removed"))
}

/// Merge the checked probes into one bus (MSB = first checked).
pub fn group_checked_as_bus(s: &mut AppState) -> Result<String, String> {
    let picked: Vec<usize> = s
        .ila
        .group
        .iter()
        .enumerate()
        .filter(|(_, g)| **g)
        .map(|(i, _)| i)
        .collect();
    if picked.len() < 2 {
        return Err("ila: check at least 2 probes to group".into());
    }
    let typed = s.ila.bus_name.text.trim().to_string();
    let name = if typed.is_empty() {
        format!("{}_bus", s.ila.config.probes[picked[0]].name)
    } else {
        typed
    };
    let d = design(s)?;
    let mut nets = Vec::new();
    for &i in &picked {
        nets.extend(s.ila.config.probes[i].nets.iter().cloned());
    }
    let mut cfg = s.ila.config.clone();
    for &i in picked.iter().rev() {
        cfg.probes.remove(i);
    }
    cfg.probes.push(IlaProbeCfg {
        name: name.clone(),
        nets,
        radix: IlaRadix::Hex,
    });
    if cfg.trigger.probe >= cfg.probes.len() {
        cfg.trigger.probe = 0;
    }
    validate_probes(&cfg, &d)?;
    s.ila.config = cfg;
    s.ila.group = vec![false; s.ila.config.probes.len()];
    s.ila.bus_name = TextEdit::new("");
    Ok(format!("ila bus {name}"))
}

/// Point the trigger at probe `idx`.
pub fn set_trigger_probe(s: &mut AppState, idx: usize) -> Result<String, String> {
    let name = s
        .ila
        .config
        .probes
        .get(idx)
        .map(|p| p.name.clone())
        .ok_or_else(|| format!("ila: no probe {idx}"))?;
    s.ila.config.trigger.probe = idx;
    Ok(format!("ila trigger probe {name}"))
}

/// Switch the trigger kind (value/mask stay in their fields).
pub fn set_trigger_choice(s: &mut AppState, c: TrigChoice) -> String {
    s.ila.config.trigger.kind = match c {
        TrigChoice::Immediate => IlaTriggerKind::Immediate,
        TrigChoice::Rising => IlaTriggerKind::Rising,
        TrigChoice::Falling => IlaTriggerKind::Falling,
        TrigChoice::Value => s.ila.value_match().unwrap_or(IlaTriggerKind::ValueMatch {
            value: 0,
            mask: u64::MAX,
        }),
    };
    format!("ila trigger {}", c.label().to_lowercase())
}

/// Set the trigger from the capture at `sample` on `probe` (click-an-edge):
/// scalar edges become Rising/Falling, anything else a value match.
pub fn set_trigger_from_wave(
    s: &mut AppState,
    probe: usize,
    sample: usize,
) -> Result<String, String> {
    let cap = s.ila.capture.as_ref().ok_or("ila: no capture")?;
    let kind = engine::edge_kind_at(cap, probe, sample).ok_or("ila: no trigger at the cursor")?;
    let pname = cap
        .probes
        .get(probe)
        .map(|p| p.name.clone())
        .unwrap_or_default();
    apply_trigger(s, probe, kind);
    Ok(format!("ila trigger {} @{pname}", kind.label()))
}

/// Suggest a trigger at `sample` across all probes (first scalar edge,
/// else a bus value match).
pub fn suggest_trigger(s: &mut AppState, sample: usize) -> Result<String, String> {
    let cap = s.ila.capture.as_ref().ok_or("ila: no capture")?;
    let (probe, kind) =
        engine::suggest_trigger_at(cap, sample).ok_or("ila: no edge at the cursor")?;
    let pname = cap
        .probes
        .get(probe)
        .map(|p| p.name.clone())
        .unwrap_or_default();
    apply_trigger(s, probe, kind);
    Ok(format!("ila trigger {} @{pname}", kind.label()))
}

fn apply_trigger(s: &mut AppState, probe: usize, kind: IlaTriggerKind) {
    if let IlaTriggerKind::ValueMatch { value, mask } = kind {
        s.ila.trig_value = TextEdit::new(value.to_string());
        s.ila.trig_mask = TextEdit::new(format!("0x{mask:X}"));
    }
    s.ila.config.trigger.probe = probe;
    s.ila.config.trigger.kind = kind;
}

/// Why Arm is unavailable (`Ok` when it can arm).
pub fn can_arm(s: &AppState) -> Result<(), String> {
    if s.ila.is_capturing() {
        return Err("A capture is already in flight".into());
    }
    if crate::live::live_design(s).is_none() {
        return Err("Run synthesis or elaborate a design first".into());
    }
    if s.ila.config.probes.is_empty() {
        return Err("Add at least one probe net".into());
    }
    Ok(())
}

/// Arm a background capture on the engine's worker thread.
pub fn arm(s: &mut AppState) -> Result<String, String> {
    can_arm(s)?;
    s.ila.apply_fields()?;
    let d = design(s)?;
    s.ila.config.validate(&d)?;
    let handle = engine::spawn_sim_ila_capture(d, s.ila.config.clone());
    s.ila.active = Some(ActiveCapture {
        rx: Arc::new(Mutex::new(handle.rx)),
        progress: handle.progress,
    });
    s.ila.status = IlaStatus::Capturing;
    s.ila.last_error = None;
    Ok(format!(
        "ila arm probes={} trigger={}@{} depth={}",
        s.ila.config.probes.len(),
        s.ila.config.trigger.kind.label(),
        s.ila.trigger_probe_name(),
        s.ila.config.depth
    ))
}

/// Synchronous capture (Tcl `ila capture`, scenes, tests).
pub fn capture_now(s: &mut AppState) -> Result<String, String> {
    if s.ila.is_capturing() {
        return Err("ila: a capture is already in flight".into());
    }
    s.ila.apply_fields()?;
    let d = design(s)?;
    let cap = engine::run_sim_ila_capture(&d, &s.ila.config, &|_| false)?;
    let msg = capture_summary(&cap);
    s.ila.mount_capture(cap);
    Ok(msg)
}

fn capture_summary(cap: &IlaCapture) -> String {
    format!(
        "ila capture probes={} samples={} trigger={} cycles={}",
        cap.probes.len(),
        cap.len(),
        cap.trigger_sample
            .map(|i| i.to_string())
            .unwrap_or_else(|| "-".into()),
        cap.cycles_run
    )
}

/// Stop the capture in flight; the worker returns its partial ring as a
/// missed trigger.
pub fn disarm(s: &mut AppState) -> Result<String, String> {
    match &s.ila.active {
        Some(a) => {
            a.progress.cancel.store(true, Ordering::Relaxed);
            Ok("ila capture stopping…".into())
        }
        None => Err("ila: no capture in flight".into()),
    }
}

/// Merge a finished background capture. Returns true when state changed;
/// keeps returning true while capturing so the cycle counter stays live.
pub fn poll(s: &mut AppState) -> bool {
    let Some(active) = s.ila.active.clone() else {
        return false;
    };
    let next = active
        .rx
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .try_recv();
    match next {
        Ok(r) => {
            s.ila.active = None;
            finish(s, r);
        }
        Err(mpsc::TryRecvError::Empty) => {}
        Err(mpsc::TryRecvError::Disconnected) => {
            s.ila.active = None;
            finish(s, Err("ila: capture worker exited unexpectedly".into()));
        }
    }
    true
}

fn finish(s: &mut AppState, r: Result<IlaCapture, String>) {
    match r {
        Ok(cap) => {
            let msg = capture_summary(&cap);
            let found = cap.found_trigger;
            s.ila.mount_capture(cap);
            s.console.lines.push(ConsoleLine {
                text: msg.clone(),
                sev: MsgSev::Info,
            });
            if found {
                s.toasts.push(ToastKind::Success, "Capture complete", &msg);
            } else {
                s.toasts.push(
                    ToastKind::Warning,
                    "Trigger not found",
                    "Showing the last samples before the capture stopped",
                );
            }
        }
        Err(e) => {
            s.ila.status = IlaStatus::Error;
            s.ila.last_error = Some(e.clone());
            s.console.lines.push(ConsoleLine {
                text: format!("ERROR: {e}"),
                sev: MsgSev::Error,
            });
            s.toasts.push(ToastKind::Error, "Capture failed", &e);
        }
    }
}

/// Default session file: `<source dir>/ila_session.json`.
pub fn default_session_path(s: &AppState) -> PathBuf {
    if let Some(p) = &s.ila.session_path {
        return p.clone();
    }
    crate::views::tb::discovery_dir(s)
        .filter(|d| d.is_dir())
        .unwrap_or_else(std::env::temp_dir)
        .join("ila_session.json")
}

/// Save the configuration (and capture) as a session file.
pub fn save_session(s: &mut AppState, path: &Path) -> Result<String, String> {
    let session = engine::IlaSession::new(s.ila.config.clone(), s.ila.capture.clone());
    engine::save_session(path, &session)?;
    s.ila.session_path = Some(path.to_path_buf());
    Ok(format!("ila session saved {}", path.display()))
}

/// Load a session file. Probes are re-validated against the live design;
/// the capture loads for viewing either way (a mismatch is a warning).
pub fn load_session(s: &mut AppState, path: &Path) -> Result<String, String> {
    let session = engine::load_session(path)?;
    let warn = crate::live::live_design(s).and_then(|d| validate_probes(&session.config, d).err());
    s.ila.config = session.config;
    s.ila.group = vec![false; s.ila.config.probes.len()];
    s.ila.sync_fields();
    s.ila.session_path = Some(path.to_path_buf());
    match session.capture {
        Some(cap) => s.ila.mount_capture(cap),
        None => {
            s.ila.capture = None;
            s.ila.status = IlaStatus::Idle;
        }
    }
    match warn {
        Some(e) => Ok(format!(
            "ila session loaded {} (warning: {e})",
            path.display()
        )),
        None => Ok(format!("ila session loaded {}", path.display())),
    }
}

/// Export the capture as a VCD file.
pub fn export(s: &AppState, path: &Path) -> Result<String, String> {
    let cap = s.ila.capture.as_ref().ok_or("ila: no capture to export")?;
    engine::export_vcd(path, cap)?;
    Ok(format!("ila vcd exported {}", path.display()))
}

fn echo_result(s: &mut AppState, r: Result<String, String>, title: &str) {
    match r {
        Ok(line) => {
            s.ila.last_error = None;
            s.console.lines.push(ConsoleLine {
                text: line,
                sev: MsgSev::Info,
            });
        }
        Err(e) => {
            s.ila.last_error = Some(e.clone());
            s.console.lines.push(ConsoleLine {
                text: format!("ERROR: {e}"),
                sev: MsgSev::Error,
            });
            s.toasts.push(ToastKind::Error, title, &e);
        }
    }
}

/// UI action wrapper: run `f`, echo its line, surface errors inline.
pub fn act(s: &mut AppState, title: &str, f: impl FnOnce(&mut AppState) -> Result<String, String>) {
    let r = f(s);
    echo_result(s, r, title);
}

/// Raise the native Save dialog for a session file.
pub fn pick_save_session(tx: &mut Tx, default: PathBuf) {
    let options = FileDialogOptions {
        title: "Save ILA Session".into(),
        file_name: default
            .file_name()
            .map(|n| n.to_string_lossy().into_owned()),
        ..Default::default()
    }
    .filter("ILA session", &["json"]);
    tx.save_file(options, |s: &mut AppState, result: FileDialogResult, _| {
        if let Ok(Some(p)) = result {
            act(s, "Cannot save session", |s| save_session(s, &p));
        } else if let Err(e) = result {
            echo_result(s, Err(e.to_string()), "Save ILA Session");
        }
    });
}

/// Raise the native Open dialog for a session file.
pub fn pick_load_session(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Load ILA Session".into(),
        ..Default::default()
    }
    .filter("ILA session", &["json"]);
    tx.open_file(options, |s: &mut AppState, result: FileDialogResult, _| {
        if let Ok(Some(p)) = result {
            act(s, "Cannot load session", |s| load_session(s, &p));
        } else if let Err(e) = result {
            echo_result(s, Err(e.to_string()), "Load ILA Session");
        }
    });
}

/// Raise the native Save dialog for a VCD export.
pub fn pick_export_vcd(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Export Capture as VCD".into(),
        file_name: Some("ila_capture.vcd".into()),
        ..Default::default()
    }
    .filter("VCD", &["vcd"]);
    tx.save_file(options, |s: &mut AppState, result: FileDialogResult, _| {
        if let Ok(Some(p)) = result {
            act(s, "Cannot export VCD", |s| export(s, &p));
        } else if let Err(e) = result {
            echo_result(s, Err(e.to_string()), "Export Capture as VCD");
        }
    });
}

/// One-line status (Tcl `ila status`).
pub fn status_text(s: &AppState) -> String {
    let ila = &s.ila;
    let cap = match &ila.capture {
        Some(c) => format!(
            " samples={} trigger={} cycles={}",
            c.len(),
            c.trigger_sample
                .map(|i| i.to_string())
                .unwrap_or_else(|| "-".into()),
            c.cycles_run
        ),
        None => " samples=0".into(),
    };
    let prog = if ila.is_capturing() {
        format!(" progress_cycles={}", ila.progress_cycles())
    } else {
        String::new()
    };
    format!(
        "ila status={} probes={} trigger={}@{} depth={} pre={}% clock={}{cap}{prog}{}",
        ila.status.label(),
        ila.config.probes.len(),
        ila.config.trigger.kind.label(),
        ila.trigger_probe_name(),
        ila.config.depth,
        (ila.config.pre_ratio * 100.0).round() as u32,
        ila.config.clock_net,
        match &ila.last_error {
            Some(e) => format!(" error={e}"),
            None => String::new(),
        }
    )
}

/// Tcl `ila <subcommand> ...` (mirrors the old `exec_ila`). No subcommand
/// opens the Logic Analyzer and reports status; probe/capture/session
/// subcommands open it too.
pub fn exec_ila(s: &mut AppState, args: &[String]) -> Result<String, String> {
    let sub = args.first().map(String::as_str).unwrap_or("");
    let rest = args.get(1..).unwrap_or_default();
    let arg = rest.join(" ");
    let show = |s: &mut AppState| s.navigate(CenterPane::Ila);
    let num = |what: &str, v: Option<&String>| -> Result<usize, String> {
        v.and_then(|x| x.parse().ok())
            .ok_or_else(|| format!("ila {what}: need a number"))
    };
    match sub {
        "" | "show" => {
            show(s);
            Ok(status_text(s))
        }
        "status" => Ok(status_text(s)),
        "add" => {
            let r = add_probe(s, &arg);
            show(s);
            r
        }
        "bus" => {
            let name = rest.first().cloned().unwrap_or_default();
            let nets: Vec<String> = rest.iter().skip(1).cloned().collect();
            let r = add_bus(s, &name, &nets);
            show(s);
            r
        }
        "remove" => remove_probe(s, num("remove", rest.first())?),
        "trigger" => {
            let kind = rest.first().map(String::as_str).unwrap_or("immediate");
            let probe = rest
                .get(1)
                .and_then(|p| p.parse::<usize>().ok())
                .unwrap_or(0);
            set_trigger_probe(s, probe)?;
            let k = IlaTriggerKind::parse(kind)?;
            if let IlaTriggerKind::ValueMatch { .. } = k {
                if let Some(v) = rest.get(2) {
                    s.ila.trig_value = TextEdit::new(v.clone());
                }
                s.ila.trig_mask = TextEdit::new(rest.get(3).cloned().unwrap_or_default());
                s.ila.config.trigger.kind = s.ila.value_match()?;
            } else {
                s.ila.config.trigger.kind = k;
            }
            Ok(format!(
                "ila trigger {} @{}",
                s.ila.config.trigger.kind.label(),
                s.ila.trigger_probe_name()
            ))
        }
        "trigger_at" => {
            let probe = num("trigger_at <probe> <sample>", rest.first())?;
            let sample = num("trigger_at <probe> <sample>", rest.get(1))?;
            set_trigger_from_wave(s, probe, sample)
        }
        "depth" => {
            let n = num("depth", rest.first())?;
            if n == 0 || n > engine::MAX_DEPTH {
                return Err(format!("ila depth: must be 1..={}", engine::MAX_DEPTH));
            }
            s.ila.config.depth = n;
            s.ila.sync_fields();
            Ok(format!("ila depth {n}"))
        }
        "preratio" => {
            let r: f32 = arg
                .trim()
                .parse()
                .map_err(|_| format!("ila preratio: bad ratio {arg}"))?;
            if !(0.0..=1.0).contains(&r) {
                return Err("ila preratio: must be 0.0..=1.0".into());
            }
            s.ila.config.pre_ratio = r;
            s.ila.sync_fields();
            Ok(format!("ila pre_ratio {r}"))
        }
        "clock" => {
            if arg.trim().is_empty() {
                return Err("ila clock: need a net".into());
            }
            s.ila.config.clock_net = arg.trim().to_string();
            Ok(format!("ila clock {}", arg.trim()))
        }
        "radix" => {
            let idx = num("radix <probe> <bin|hex|dec>", rest.first())?;
            let radix = IlaRadix::parse(rest.get(1).map(String::as_str).unwrap_or("bin"))?;
            let p = s
                .ila
                .config
                .probes
                .get_mut(idx)
                .ok_or_else(|| format!("ila radix: no probe {idx}"))?;
            p.radix = radix;
            let name = p.name.clone();
            if let Some(cap) = s.ila.capture.as_mut() {
                if let Some(cp) = cap.probes.get_mut(idx) {
                    cp.radix = radix;
                }
            }
            Ok(format!("ila radix {name} {}", radix.label()))
        }
        "capture" => {
            let r = capture_now(s);
            show(s);
            r
        }
        "arm" => {
            let r = arm(s);
            show(s);
            r
        }
        "disarm" => disarm(s),
        "save" => {
            let path = if arg.trim().is_empty() {
                default_session_path(s)
            } else {
                PathBuf::from(arg.trim())
            };
            save_session(s, &path)
        }
        "load" => {
            let path = if arg.trim().is_empty() {
                default_session_path(s)
            } else {
                PathBuf::from(arg.trim())
            };
            let r = load_session(s, &path);
            show(s);
            r
        }
        "export_vcd" => {
            if arg.trim().is_empty() {
                return Err("ila export_vcd: need a file path".into());
            }
            export(s, Path::new(arg.trim()))
        }
        other => Err(format!(
            "ila: unknown subcommand {other} (add, bus, remove, trigger, trigger_at, depth, preratio, clock, radix, capture, arm, disarm, status, save, load, export_vcd)"
        )),
    }
}

/// Scenes/tests: the counter design with `led` and the counter bits probed
/// and a rising-edge trigger on `led`.
pub fn seed_ila_fixture(s: &mut AppState) -> Result<(), String> {
    crate::views::debug::seed_debug_fixture(s)?;
    let nets = probe_nets(s);
    add_probe(s, "led")?;
    let bits: Vec<String> = nets
        .iter()
        .filter(|n| n.starts_with("cnt"))
        .take(4)
        .cloned()
        .collect();
    for b in &bits {
        add_probe(s, b)?;
    }
    // led first rises after 8 clocks: keep the pre window inside that so the
    // capture fills its whole depth.
    s.ila.config.depth = 48;
    s.ila.config.pre_ratio = 0.125;
    s.ila.config.max_cycles = 512;
    s.ila.config.trigger.probe = 0;
    s.ila.config.trigger.kind = IlaTriggerKind::Rising;
    s.ila.sync_fields();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init;

    fn seeded() -> AppState {
        let mut s = init();
        seed_ila_fixture(&mut s).expect("fixture");
        s
    }

    #[test]
    fn probes_validate_against_the_design() {
        let mut s = init();
        assert!(add_probe(&mut s, "led").is_err(), "no design yet");
        crate::views::debug::seed_debug_fixture(&mut s).unwrap();
        add_probe(&mut s, "led").unwrap();
        assert!(add_probe(&mut s, "no_such_net").is_err());
        assert_eq!(add_probe(&mut s, "led").unwrap(), "ila probe led (already)");
        assert_eq!(s.ila.config.probes.len(), 1);
        assert!(!probe_nets(&s).is_empty());
        assert!(!clock_nets(&s).is_empty());
    }

    #[test]
    fn group_checked_as_bus_merges_msb_first() {
        let mut s = seeded();
        let n = s.ila.config.probes.len();
        assert!(n >= 3, "fixture probes: {:?}", s.ila.config.probes);
        assert!(group_checked_as_bus(&mut s).is_err(), "needs 2 checks");
        s.ila.group[1] = true;
        s.ila.group[2] = true;
        s.ila.bus_name = TextEdit::new("cnt_lo");
        let first = s.ila.config.probes[1].nets[0].clone();
        group_checked_as_bus(&mut s).unwrap();
        assert_eq!(s.ila.config.probes.len(), n - 1);
        let bus = s.ila.config.probes.last().unwrap();
        assert_eq!(bus.name, "cnt_lo");
        assert_eq!(bus.nets.len(), 2);
        assert_eq!(bus.nets[0], first, "first checked is the MSB");
        assert!(s.ila.group.iter().all(|g| !g));
    }

    #[test]
    fn sync_capture_triggers_on_led_and_centers_the_cursor() {
        let mut s = seeded();
        let msg = capture_now(&mut s).unwrap();
        let cap = s.ila.capture.as_ref().unwrap();
        assert!(cap.found_trigger, "{msg}");
        assert_eq!(cap.len(), 48);
        assert_eq!(cap.trigger_sample, Some(6), "12.5% pre-trigger of 48");
        assert_eq!(s.ila.viewer.cursor, Some(6));
        assert_eq!(s.ila.status, IlaStatus::Done);
        let t = cap.trigger_sample.unwrap();
        assert_eq!(cap.value_at(0, t), Some(1), "led is high at the trigger");
        assert_eq!(cap.value_at(0, t - 1), Some(0), "and low just before");
    }

    #[test]
    fn background_arm_finishes_through_poll() {
        let mut s = seeded();
        arm(&mut s).unwrap();
        assert!(s.ila.is_capturing());
        assert_eq!(s.ila.status, IlaStatus::Capturing);
        let start = std::time::Instant::now();
        while s.ila.is_capturing() {
            poll(&mut s);
            assert!(start.elapsed() < std::time::Duration::from_secs(60));
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(s.ila.status, IlaStatus::Done);
        assert!(s.ila.capture.as_ref().unwrap().found_trigger);
        assert!(disarm(&mut s).is_err(), "nothing left to disarm");
    }

    #[test]
    fn value_match_uses_the_probe_width_mask() {
        let mut s = seeded();
        s.ila.trig_value = TextEdit::new("1");
        s.ila.trig_mask = TextEdit::new("");
        set_trigger_choice(&mut s, TrigChoice::Value);
        assert_eq!(
            s.ila.config.trigger.kind,
            IlaTriggerKind::ValueMatch { value: 1, mask: 1 }
        );
        capture_now(&mut s).expect("1-bit value match validates");
    }

    #[test]
    fn trigger_from_wave_and_suggestion_read_the_capture() {
        let mut s = seeded();
        capture_now(&mut s).unwrap();
        let t = s.ila.capture.as_ref().unwrap().trigger_sample.unwrap();
        set_trigger_choice(&mut s, TrigChoice::Immediate);
        let r = set_trigger_from_wave(&mut s, 0, t).unwrap();
        assert_eq!(s.ila.config.trigger.kind, IlaTriggerKind::Rising, "{r}");
        assert!(suggest_trigger(&mut s, t).is_ok());
    }

    #[test]
    fn session_and_vcd_round_trip_through_files() {
        let mut s = seeded();
        capture_now(&mut s).unwrap();
        let dir = std::env::temp_dir().join(format!("helion-rune-ila-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sess = dir.join("sess.json");
        save_session(&mut s, &sess).unwrap();
        let vcd = dir.join("cap.vcd");
        export(&s, &vcd).unwrap();
        let text = std::fs::read_to_string(&vcd).unwrap();
        assert!(text.contains("$enddefinitions"));
        let mut fresh = init();
        crate::views::debug::seed_debug_fixture(&mut fresh).unwrap();
        let out = load_session(&mut fresh, &sess).unwrap();
        assert!(!out.contains("warning"), "{out}");
        assert_eq!(fresh.ila.config.probes, s.ila.config.probes);
        assert_eq!(fresh.ila.capture, s.ila.capture);
        assert_eq!(fresh.ila.depth.text, "48");
    }

    #[test]
    fn tcl_verb_covers_every_subcommand() {
        let mut s = init();
        crate::views::debug::seed_debug_fixture(&mut s).unwrap();
        let run = |s: &mut AppState, a: &[&str]| {
            exec_ila(s, &a.iter().map(|x| x.to_string()).collect::<Vec<_>>())
        };
        assert!(run(&mut s, &[]).unwrap().starts_with("ila status=Idle"));
        assert_eq!(s.center, CenterPane::Ila);
        run(&mut s, &["add", "led"]).unwrap();
        let bits: Vec<String> = probe_nets(&s)
            .into_iter()
            .filter(|n| n.starts_with("cnt"))
            .take(2)
            .collect();
        let mut bus = vec!["bus", "cnt2"];
        bus.extend(bits.iter().map(String::as_str));
        run(&mut s, &bus).unwrap();
        run(&mut s, &["trigger", "rising", "0"]).unwrap();
        run(&mut s, &["depth", "32"]).unwrap();
        run(&mut s, &["preratio", "0.1"]).unwrap();
        run(&mut s, &["clock", "clk"]).unwrap();
        run(&mut s, &["radix", "1", "dec"]).unwrap();
        let cap = run(&mut s, &["capture"]).unwrap();
        assert!(cap.contains("samples=32"), "{cap}");
        assert!(run(&mut s, &["trigger_at", "0", "16"]).is_ok());
        run(&mut s, &["remove", "1"]).unwrap();
        assert!(run(&mut s, &["disarm"]).is_err());
        assert!(run(&mut s, &["bogus"]).is_err());
        let path =
            std::env::temp_dir().join(format!("helion-rune-ila-tcl-{}.vcd", std::process::id()));
        run(&mut s, &["export_vcd", &path.to_string_lossy()]).unwrap();
        assert!(path.is_file());
    }
}
