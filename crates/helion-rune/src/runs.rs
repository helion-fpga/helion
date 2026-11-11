//! Real engine runs: synthesis/implementation on a worker thread.
//!
//! New module (func-03-runs-real). `helion-ide --headless` runs
//! `IdeModel::open_source` + `exec("report_timing")`
//! (`crates/helion-gui/src/bin/helion-ide.rs:166 run_headless_oneshot`), which
//! flows through `run_step_inner` (`crates/helion-gui/src/ide.rs:10413`) into
//! the engine crates. `helion-rune` must not depend on the `helion-gui` lib,
//! so this module drives the same engine entry points directly:
//! - `helion_sv::synth_sv_path` / `helion_vhdl::synth_vhdl_path`
//!   (`crates/helion-gui/src/lib.rs:90 synth_hdl_path` dispatches by extension)
//! - `helion_proj::{Mode, Session}` (`synth_design/opt/place/route`)
//! - `helion_sta::{report_timing, report_timing_routed_xdc,
//!   report_timing_summary, report_power, load_xdc, create_clock}`
//!   (as `ide.rs:25127 refresh_reports` composes them)
//! - `helion_pack::pack`, `helion_drc::{check_placed, check_routed}`
//! - `helion_device::Device::load_part` (`HL10T-C32-1` default part, as
//!   `crates/helion-gui/src/lib.rs:222 GpuiShell::default`)
//!
//! The worker thread streams [`RunEvent::Line`] over an `mpsc` channel;
//! [`poll`] (wired as `AppSpec::poll`) drains it every frame into the Tcl
//! console and the per-run log buffers, so runs stay live without blocking
//! the UI. Completion applies [`RunArtifacts`] built ONLY from real engine
//! reports into Timing/Util/Power/DRC state; failures flip Failed with an
//! error toast; [`cancel_run`] stops the run between stages.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/tooling.rs`: `AppSpec::poll`
//!   (`fn(&mut S, f64, &mut Tx) -> bool`, called every frame).

use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use rune::Tx;

use crate::state::{
    AppState, BitstreamState, CdcRow, ClockNetRow, ConsoleLine, DrcItem, DrcSev, HierRow,
    MethodRow, MsgSev, PowerState, RunStatus, StatusVerb, TimingClock, TimingGroupKind,
    TimingGroupRow, TimingPathGroup, TimingPathRow, TimingPinRow, TimingState, ToastKind,
    UtilState,
};

/// Fallback device when the project part has no Helion HAD model (same
/// default part `GpuiShell::default` ships). The substitution is always
/// announced as a warning line, never silent.
const FALLBACK_PART: &str = "HL10T-C32-1";

/// Implicit analysis clock when no user SDC is found (10 ns, mirroring
/// `ide.rs:7283 clock_period_ps: 10_000`).
const IMPLICIT_CLOCK_PS: u64 = 10_000;

/// Max channel events drained per frame (runs emit tens of lines; the cap
/// only bounds pathological floods).
const MAX_DRAIN_PER_FRAME: usize = 256;

/// Which flow a run executes. Typed so dispatch is never a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunKind {
    Synth,
    Impl,
}

impl RunKind {
    pub fn run_name(self) -> &'static str {
        match self {
            RunKind::Synth => "synth_1",
            RunKind::Impl => "impl_1",
        }
    }

    pub fn tcl_verb(self) -> &'static str {
        match self {
            RunKind::Synth => "launch_runs synth_1",
            RunKind::Impl => "launch_runs impl_1",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            RunKind::Synth => "Synthesis",
            RunKind::Impl => "Implementation",
        }
    }
}

/// One worker -> UI message. Lines stream live; the terminal event carries
/// the real outcome.
#[derive(Debug)]
pub enum RunEvent {
    Line { text: String, sev: MsgSev },
    Finished(Result<RunArtifacts, String>),
    Cancelled,
}

/// Bitstream signoff verdict, mirroring `helion_drc::BitgenSignoffReport`
/// (`bitgen_allowed`/summary/unrouted/overused).
#[derive(Clone, Debug, Default)]
pub struct RunSignoff {
    pub bitgen_allowed: bool,
    pub summary: String,
    pub unrouted: usize,
    pub overused: u32,
}

/// Real completion payload. Every `Some` was computed from engine reports
/// on the worker; `None` means that report genuinely could not run (its
/// reason is in the log) and the old state is kept.
#[derive(Clone, Debug, Default)]
pub struct RunArtifacts {
    pub timing: Option<TimingState>,
    pub util: Option<UtilState>,
    pub power: Option<PowerState>,
    pub drc: Option<Vec<DrcItem>>,
    pub signoff: Option<RunSignoff>,
    pub bitstream: Option<BitstreamState>,
    /// Engine design behind the run (synth keeps the optimized netlist, impl
    /// the placed/routed one). Feeds the schematic/device live views.
    pub design: Option<helion_ir::Design>,
    /// Implementation placement (impl runs only). Feeds the device canvas.
    pub placed: Option<helion_place::Placed>,
    /// Device part the run targeted (`""` when no device model resolved).
    pub part: String,
    /// One-line real summary for the toast (`"12 cells, 5 endpoints"`).
    pub summary: String,
    pub cells: usize,
}

/// The live worker handle: its event channel plus the cancel flag the
/// worker checks between stages.
#[derive(Clone)]
struct ActiveRun {
    kind: RunKind,
    rx: Arc<Mutex<mpsc::Receiver<RunEvent>>>,
    cancel: Arc<AtomicBool>,
}

impl std::fmt::Debug for ActiveRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveRun")
            .field("kind", &self.kind)
            .field("cancelled", &self.cancel.load(Ordering::Relaxed))
            .finish()
    }
}

/// Run ownership slice of [`AppState`]: explicit source override, real
/// per-run log buffers, and the at-most-one live worker.
#[derive(Clone, Default)]
pub struct RunsState {
    /// Explicit RTL source for runs (integration tests / future open-project
    /// wiring). `None` materializes the current editor RTL buffer.
    pub source: Option<PathBuf>,
    /// Real buffered synth log lines (the Log tab renders these, never
    /// canned demo text).
    pub synth_log: Vec<ConsoleLine>,
    /// Real buffered implementation log lines.
    pub impl_log: Vec<ConsoleLine>,
    /// Engine design from the last completed run (synth or impl). The
    /// schematic/device views read this first, `tcl.design` second.
    pub live_design: Option<helion_ir::Design>,
    /// Placement from the last completed implementation run (`None` until an
    /// impl run finishes; synth runs leave the previous placement alone).
    pub live_placed: Option<helion_place::Placed>,
    /// Part behind `live_placed` (`""` until an impl run finishes).
    pub live_part: String,
    /// Die geometry cached when the placement lands (views read this; they
    /// only load a HAD model on the no-run-yet path).
    pub live_geom: Option<crate::live::LiveDieGeom>,
    active: Option<ActiveRun>,
}

impl std::fmt::Debug for RunsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunsState")
            .field("source", &self.source)
            .field("synth_log", &self.synth_log)
            .field("impl_log", &self.impl_log)
            .field(
                "live_design_cells",
                &self.live_design.as_ref().map(|d| d.cells.len()),
            )
            .field("live_placed", &self.live_placed.is_some())
            .field("live_part", &self.live_part)
            .field("active", &self.active)
            .finish()
    }
}

impl RunsState {
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn active_kind(&self) -> Option<RunKind> {
        self.active.as_ref().map(|a| a.kind)
    }

    /// True once [`cancel_run`] was requested and the worker has not yet
    /// delivered its terminal event.
    pub fn cancel_requested(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|a| a.cancel.load(Ordering::Relaxed))
    }

    fn active_rx(&self) -> Option<Arc<Mutex<mpsc::Receiver<RunEvent>>>> {
        self.active.as_ref().map(|a| a.rx.clone())
    }

    fn set_active(&mut self, kind: RunKind, rx: mpsc::Receiver<RunEvent>, cancel: Arc<AtomicBool>) {
        self.active = Some(ActiveRun {
            kind,
            rx: Arc::new(Mutex::new(rx)),
            cancel,
        });
    }

    fn clear_active(&mut self) {
        self.active = None;
    }

    fn request_cancel(&self) {
        if let Some(a) = &self.active {
            a.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Drop any live run (flagging cancel first) and reset to idle. The
    /// orphaned worker's sends fail silently and it exits on its own.
    pub fn reset(&mut self) {
        self.request_cancel();
        *self = RunsState::default();
    }

    fn log_for(&self, kind: RunKind) -> &Vec<ConsoleLine> {
        match kind {
            RunKind::Synth => &self.synth_log,
            RunKind::Impl => &self.impl_log,
        }
    }

    fn log_for_mut(&mut self, kind: RunKind) -> &mut Vec<ConsoleLine> {
        match kind {
            RunKind::Synth => &mut self.synth_log,
            RunKind::Impl => &mut self.impl_log,
        }
    }
}

/// UI entry: echo the Tcl verb, resolve the RTL source, and start the run.
/// Resolution/start problems surface as an error line plus an error toast;
/// the run state is only touched once the worker thread is spawned.
pub fn launch(s: &mut AppState, kind: RunKind) {
    launch_verb(s, kind, kind.tcl_verb());
}

/// UI entry with an explicit echo verb: the `-to_step` Tcl form echoes what
/// the user typed instead of the normalized run verb.
pub fn launch_verb(s: &mut AppState, kind: RunKind, verb: &str) {
    s.console.lines.push(ConsoleLine {
        text: verb.into(),
        sev: MsgSev::Info,
    });
    let source = match resolve_source(s) {
        Ok(p) => p,
        Err(e) => {
            s.console.lines.push(ConsoleLine {
                text: format!("ERROR: {e}"),
                sev: MsgSev::Error,
            });
            s.toasts.push(ToastKind::Error, "Cannot start run", &e);
            return;
        }
    };
    let part = s.project.part.clone();
    if let Err(e) = start_run(s, kind, source, part) {
        s.console.lines.push(ConsoleLine {
            text: format!("ERROR: {e}"),
            sev: MsgSev::Error,
        });
        s.toasts.push(ToastKind::Error, "Cannot start run", &e);
    }
}

/// Explicit entry: start `kind` on `source` targeting `part`. Fails when a
/// run is already active or the worker thread cannot spawn; on failure the
/// state is untouched.
/// Pin-constraint keys that survive across runs (old `constraints.*` maps).
const PIN_KEYS: [&str; 7] = [
    "LOC",
    "IOSTANDARD",
    "DRIVE",
    "SLEW",
    "PULLTYPE",
    "DIFF_TERM",
    "IN_TERM",
];

/// Snapshot the live design's per-port pin constraints so the worker can
/// re-apply them to its fresh elaboration (the old session design kept
/// them on the single netlist; runs elaborate from scratch).
fn snapshot_pin_constraints(s: &AppState) -> Vec<(String, String, String)> {
    let Some(design) = crate::live::live_design(s) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for p in &design.ports {
        let port = helion_ir::resolve(p.name).to_string();
        for key in PIN_KEYS {
            if let Some(v) = p.attrs.get(key) {
                out.push((port.clone(), key.to_string(), v.to_string()));
            }
        }
    }
    out
}

pub fn start_run(
    s: &mut AppState,
    kind: RunKind,
    source: PathBuf,
    part: String,
) -> Result<(), String> {
    if let Some(busy) = s.runs.active_kind() {
        return Err(format!("{} is already running", busy.run_name()));
    }
    let pins = snapshot_pin_constraints(s);
    let strategy = impl_strategy_for(&s.flow.impl_strategy);
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    std::thread::Builder::new()
        .name(format!("helion-run-{}", kind.run_name()))
        .spawn(move || run_worker(kind, source, part, pins, strategy, tx, worker_cancel))
        .map_err(|e| format!("failed to spawn run thread: {e}"))?;
    s.runs.set_active(kind, rx, cancel);
    match kind {
        RunKind::Synth => {
            s.flow.synth = RunStatus::Running;
            s.status = StatusVerb::SynthesisRunning;
        }
        RunKind::Impl => {
            s.flow.impl_run = RunStatus::Running;
            s.status = StatusVerb::ImplementationRunning;
        }
    }
    Ok(())
}

/// Flag the active run for cancellation (the worker stops between stages)
/// and echo the `cancel_runs` verb. With no active run, an info toast says
/// so instead of failing silently.
pub fn cancel_run(s: &mut AppState) {
    match s.runs.active_kind() {
        Some(kind) => {
            s.runs.request_cancel();
            s.console.lines.push(ConsoleLine {
                text: format!("cancel_runs {}", kind.run_name()),
                sev: MsgSev::Info,
            });
        }
        None => {
            s.toasts
                .push(ToastKind::Info, "No active run", "Nothing to cancel");
        }
    }
}

/// Drain pending worker events into console/log state. Returns true when
/// anything changed (drives `AppSpec::poll`'s dirty flag).
pub fn poll_runs(s: &mut AppState) -> bool {
    let kind = match s.runs.active_kind() {
        Some(k) => k,
        None => return false,
    };
    let rx = match s.runs.active_rx() {
        Some(rx) => rx,
        None => return false,
    };
    let mut changed = false;
    for _ in 0..MAX_DRAIN_PER_FRAME {
        let next = rx.lock().unwrap_or_else(|e| e.into_inner()).try_recv();
        match next {
            Ok(event) => {
                deliver(s, kind, event);
                changed = true;
                if !s.runs.is_active() {
                    break;
                }
            }
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                deliver(
                    s,
                    kind,
                    RunEvent::Finished(Err("run worker exited unexpectedly".into())),
                );
                changed = true;
                break;
            }
        }
    }
    changed
}

/// `AppSpec::poll` wiring: pump the run channel every frame.
pub fn poll(s: &mut AppState, _t: f64, _tx: &mut Tx) -> bool {
    poll_runs(s)
}

/// Apply one worker event: lines go to the Tcl console AND the run's log
/// buffer; the terminal event fills reports / flips status / toasts.
pub fn deliver(s: &mut AppState, kind: RunKind, event: RunEvent) {
    match event {
        RunEvent::Line { text, sev } => {
            s.console.lines.push(ConsoleLine {
                text: text.clone(),
                sev,
            });
            s.runs.log_for_mut(kind).push(ConsoleLine { text, sev });
        }
        RunEvent::Finished(result) => {
            let errors = s
                .runs
                .log_for(kind)
                .iter()
                .filter(|l| l.sev == MsgSev::Error)
                .count();
            match result {
                Ok(art) => {
                    if let Some(t) = art.timing {
                        s.timing = t;
                    }
                    if let Some(u) = art.util {
                        s.util = u;
                    }
                    if let Some(p) = art.power {
                        s.power = p;
                    }
                    if let Some(d) = art.drc {
                        s.drc.items = d;
                        s.drc.has_run = true;
                    }
                    if let Some(g) = art.signoff {
                        s.drc.bitgen_allowed = Some(g.bitgen_allowed);
                        s.drc.signoff_summary = g.summary;
                        s.drc.unrouted = g.unrouted;
                        s.drc.overused = g.overused;
                        s.drc.has_run = true;
                    }
                    if let Some(b) = art.bitstream {
                        s.bitstream = b;
                    }
                    if let Some(d) = art.design {
                        s.runs.live_design = Some(d);
                    }
                    if let Some(p) = art.placed {
                        s.runs.live_placed = Some(p);
                    }
                    if !art.part.is_empty() {
                        if let Ok(dev) = helion_device::Device::load_part(&art.part) {
                            s.runs.live_geom = Some(crate::live::LiveDieGeom::from_device(&dev));
                        }
                        s.runs.live_part = art.part;
                    }
                    let verb = match kind {
                        RunKind::Synth => {
                            s.flow.synth = RunStatus::Complete;
                            StatusVerb::SynthesisComplete
                        }
                        RunKind::Impl => {
                            s.flow.impl_run = RunStatus::Complete;
                            StatusVerb::ImplementationComplete
                        }
                    };
                    s.status = verb;
                    let detail = if art.summary.is_empty() {
                        format!("{} finished with {} Errors", kind.run_name(), errors)
                    } else {
                        format!(
                            "{} finished with {} Errors · {}",
                            kind.run_name(),
                            errors,
                            art.summary
                        )
                    };
                    s.toasts.push(
                        ToastKind::Success,
                        &format!("{} complete", kind.title()),
                        &detail,
                    );
                }
                Err(e) => {
                    match kind {
                        RunKind::Synth => s.flow.synth = RunStatus::Failed,
                        RunKind::Impl => s.flow.impl_run = RunStatus::Failed,
                    }
                    s.status = StatusVerb::Failed;
                    let line = format!("ERROR: {e}");
                    s.console.lines.push(ConsoleLine {
                        text: line.clone(),
                        sev: MsgSev::Error,
                    });
                    s.runs.log_for_mut(kind).push(ConsoleLine {
                        text: line,
                        sev: MsgSev::Error,
                    });
                    s.toasts.push(
                        ToastKind::Error,
                        &format!("{} failed", kind.title()),
                        &truncate_chars(&e, 140),
                    );
                }
            }
            s.runs.clear_active();
        }
        RunEvent::Cancelled => {
            match kind {
                RunKind::Synth => s.flow.synth = RunStatus::NotStarted,
                RunKind::Impl => s.flow.impl_run = RunStatus::NotStarted,
            }
            s.status = StatusVerb::Ready;
            let line = format!("{} cancelled by user", kind.run_name());
            s.console.lines.push(ConsoleLine {
                text: line.clone(),
                sev: MsgSev::Info,
            });
            s.runs.log_for_mut(kind).push(ConsoleLine {
                text: line,
                sev: MsgSev::Info,
            });
            s.toasts.push(
                ToastKind::Warning,
                "Run cancelled",
                &format!("{} stopped", kind.run_name()),
            );
            s.runs.clear_active();
        }
    }
}

/// Char-boundary-safe truncation for toast details.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Resolve the RTL source: the explicit override when set, else the current
/// editor RTL buffer materialized to a temp file.
fn resolve_source(s: &AppState) -> Result<PathBuf, String> {
    if let Some(p) = &s.runs.source {
        return Ok(p.clone());
    }
    let tabs = &s.editor.tabs;
    let idx = if s.editor.active < tabs.len() && is_rtl_source(&tabs[s.editor.active]) {
        Some(s.editor.active)
    } else {
        tabs.iter().position(|t| is_rtl_source(t))
    };
    let i = idx.ok_or_else(|| "launch_runs: no RTL source (open a source first)".to_string())?;
    if s.editor.lines.iter().all(|l| l.trim().is_empty()) {
        return Err("launch_runs: the RTL buffer is empty".into());
    }
    let file = PathBuf::from(&tabs[i])
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_else(|| std::ffi::OsString::from("design.v"));
    let dir = std::env::temp_dir().join("helion-rune-runs");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("launch_runs: cannot stage RTL ({}): {e}", dir.display()))?;
    let path = dir.join(file);
    std::fs::write(&path, s.editor.lines.join("\n"))
        .map_err(|e| format!("launch_runs: cannot stage RTL ({}): {e}", path.display()))?;
    stage_sibling_constraints(s, &tabs[i], &path);
    Ok(path)
}

/// Copy a sibling `.sdc`/`.xdc` of the tab's real on-disk file next to the
/// staged RTL, so [`load_run_constraints`] finds the user's clocks instead of
/// falling back to the implicit clock. Best-effort: a missing SDC or a failed
/// copy keeps the old behavior (the worker announces the implicit clock).
fn stage_sibling_constraints(s: &AppState, tab: &str, staged: &PathBuf) {
    let Some(real) = s.editor.paths.get(tab) else {
        return;
    };
    for ext in ["sdc", "xdc"] {
        let src = real.with_extension(ext);
        if src.is_file() {
            let _ = std::fs::copy(&src, staged.with_extension(ext));
            return;
        }
    }
}

fn is_rtl_source(name: &str) -> bool {
    let ext = PathBuf::from(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(ext.as_str(), "sv" | "v" | "vhd" | "vhdl")
}

/// The worker body: real engine flow (synth [+ opt/place/route]) streaming
/// real stage lines, then the real reports (or the real error).
/// The implementation strategy a project setting names: a real
/// `helion_proj::RunStrategy` preset (Default, TimingExplore, RuntimeOpt,
/// ...), falling back to Default for legacy labels.
pub fn impl_strategy_for(label: &str) -> helion_proj::RunStrategy {
    helion_proj::RunStrategy::preset(label.trim())
        .or_else(|_| helion_proj::RunStrategy::preset("default"))
        .unwrap_or_else(|_| helion_proj::RunStrategy::fallback())
}

fn run_worker(
    kind: RunKind,
    source: PathBuf,
    part_req: String,
    pins: Vec<(String, String, String)>,
    strategy: helion_proj::RunStrategy,
    tx: mpsc::Sender<RunEvent>,
    cancel: Arc<AtomicBool>,
) {
    let send = |text: String, sev: MsgSev| {
        let _ = tx.send(RunEvent::Line { text, sev });
    };
    let is_cancelled = || cancel.load(Ordering::Relaxed);
    let cancelled = || {
        let _ = tx.send(RunEvent::Cancelled);
    };
    let finish = |result: Result<RunArtifacts, String>| {
        let _ = tx.send(RunEvent::Finished(result));
    };

    send(format!("open_source {}", source.display()), MsgSev::Info);

    let mut design = match synth_source(&source) {
        Ok(d) => {
            send(
                format!(
                    "synth_design cells={} luts={}",
                    d.cells.len(),
                    d.lut_inits().len()
                ),
                MsgSev::Info,
            );
            d
        }
        Err(e) => {
            finish(Err(format!("synth_design failed: {e}")));
            return;
        }
    };
    if is_cancelled() {
        cancelled();
        return;
    }

    if !pins.is_empty() {
        let mut applied = 0usize;
        for (port, key, value) in &pins {
            if design
                .set_port_attr(port, key.as_str(), value.as_str())
                .is_ok()
            {
                applied += 1;
            }
        }
        if applied > 0 {
            send(
                format!("set_property pin constraints applied={applied}"),
                MsgSev::Info,
            );
        }
    }

    let removed = helion_proj::opt_design(&mut design);
    send(format!("opt_design removed={removed}"), MsgSev::Info);
    if is_cancelled() {
        cancelled();
        return;
    }

    let (xdc, clks) = load_run_constraints(&source, &send);
    let mut art = RunArtifacts {
        cells: design.cells.len(),
        ..RunArtifacts::default()
    };

    if kind == RunKind::Synth {
        run_synth_reports(&design, &xdc, &clks, &part_req, &mut art, &send);
        art.design = Some(design);
        art.summary = summarize(&art);
        finish(Ok(art));
        return;
    }

    let dev = match resolve_device(&part_req, &send) {
        Ok(dev) => dev,
        Err(e) => {
            finish(Err(format!("launch_runs impl_1: {e}")));
            return;
        }
    };
    let mut session = helion_proj::Session::new(helion_proj::Mode::Project);
    session.synth_design(design);
    if is_cancelled() {
        cancelled();
        return;
    }
    send(
        format!("Implementation strategy: {}", strategy.preset),
        MsgSev::Info,
    );
    if strategy.opt_level >= 1 {
        match session.opt_design_step() {
            Ok(n) => send(format!("opt_design removed={n}"), MsgSev::Info),
            Err(e) => {
                finish(Err(format!("opt_design failed: {e}")));
                return;
            }
        }
        if is_cancelled() {
            cancelled();
            return;
        }
    }
    if let Err(e) = session.place_design_with(&dev, strategy.place_opts()) {
        finish(Err(format!("place_design failed: {e}")));
        return;
    }
    if let Some(p) = session.placed.as_ref() {
        send(
            format!(
                "place_design lutff_sites={} iob_sites={} cost={:.3}",
                p.lutff_sites.len(),
                p.iob_sites.len(),
                p.cost
            ),
            MsgSev::Info,
        );
    }
    if is_cancelled() {
        cancelled();
        return;
    }
    if let Err(e) = session.route_design_with(&dev, strategy.route_opts()) {
        finish(Err(format!("route_design failed: {e}")));
        return;
    }
    if let Some(r) = session.routed.as_ref() {
        send(
            format!(
                "route_design pathfinder_iters={} overused={}",
                r.pathfinder_iters, r.overused
            ),
            MsgSev::Info,
        );
    }
    if is_cancelled() {
        cancelled();
        return;
    }
    let routed = match session.routed.as_ref() {
        Some(r) => r,
        None => {
            finish(Err("route_design produced no routed design".into()));
            return;
        }
    };
    let design = match session.design.as_ref() {
        Some(d) => d,
        None => {
            finish(Err("implementation lost the design".into()));
            return;
        }
    };
    // Live-view payload, cloned before `write_bitstream` reborrows `session`.
    let live_design = design.clone();
    let live_placed = routed.placed.clone();
    let mut timing_opt: Option<helion_sta::TimingResult> = None;
    match helion_sta::report_timing_routed_xdc(design, routed, &clks, &xdc) {
        Ok(t) => {
            let summary = helion_sta::report_timing_summary(&clks, &xdc, Some(&t));
            send(
                format!(
                    "report_timing WNS_PS={} TNS_PS={} endpoints={}",
                    wns_text(&summary),
                    summary.tns_ps,
                    summary.endpoints
                ),
                MsgSev::Info,
            );
            timing_opt = Some(t);
        }
        Err(e) => send(format!("WARN: report_timing skipped: {e}"), MsgSev::Warn),
    }
    // Full timing family from the same analysis: summary + clocks + groups +
    // paths/pins + clock networks + CDC + methodology (or loud warnings).
    {
        let summary = helion_sta::report_timing_summary(&clks, &xdc, timing_opt.as_ref());
        let placed = &routed.placed;
        let timing = timing_state_from(&summary, timing_opt.as_ref(), design, Some(placed), &xdc);
        send(
            format!(
                "report_timing paths={} groups={} clocks={}",
                timing.paths.len(),
                timing.groups.len(),
                timing.clocks.len()
            ),
            MsgSev::Info,
        );
        send(
            format!(
                "report_cdc violations={} crossings_clock_trees={}",
                timing.cdc.len(),
                timing.clock_networks.len()
            ),
            MsgSev::Info,
        );
        send(
            format!("report_methodology checks={}", timing.methodology.len()),
            MsgSev::Info,
        );
        art.timing = Some(timing);
    }
    let placed = &routed.placed;
    let packed = &placed.packed;
    send(
        format!(
            "report_utilization LUTFF={} IOB={} BRAM={} DSP={}",
            packed.lutffs.len(),
            packed.iobs.len(),
            packed.brams.len(),
            packed.macs.len()
        ),
        MsgSev::Info,
    );
    art.util = Some(util_state_from(packed, design, &dev));
    let pr = helion_sta::report_power(
        &dev,
        Some(design),
        Some(placed),
        &clks,
        &xdc.operating_conditions,
    );
    send(
        format!(
            "report_power TOTAL_UW={} DYNAMIC_UW={}",
            pr.total_uw, pr.dynamic_uw
        ),
        MsgSev::Info,
    );
    art.power = Some(power_state_from(&pr, &xdc.operating_conditions));
    let drc = helion_drc::check_routed(design, routed, &dev);
    send(
        format!("report_drc violations={}", drc.items.len()),
        MsgSev::Info,
    );
    art.drc = Some(drc.items.iter().map(drc_item_from).collect());
    let signoff = helion_drc::check_bitstream_signoff(
        design,
        routed,
        &dev,
        &helion_drc::SignoffConfig::default(),
    );
    send(signoff.summary_line(), MsgSev::Info);
    art.signoff = Some(RunSignoff {
        bitgen_allowed: signoff.bitgen_allowed,
        summary: signoff.summary_line(),
        unrouted: signoff.unrouted_nets,
        overused: signoff.overused_tiles,
    });
    if is_cancelled() {
        cancelled();
        return;
    }
    // Bitstream stage (`ide.rs::run_step_inner` Bitstream): frames/bytes from
    // the packet stream, configured frames, and the session hash.
    match session.write_bitstream(&dev) {
        Ok(b) => {
            let (idcode, frames, bytes) = (b.idcode, b.frames.len(), b.packets.len());
            let configured = b.frames.values().filter(|w| **w != 0).count();
            let hash = session.blinky_hash().unwrap_or(0);
            send(
                format!(
                    "write_bitstream frames={frames} bytes={bytes} hash={hash:#010x} configured={configured}"
                ),
                MsgSev::Info,
            );
            art.bitstream = Some(BitstreamState {
                idcode,
                hash,
                bytes,
                frames,
                configured,
                has_run: true,
            });
        }
        Err(e) => send(format!("WARN: write_bitstream skipped: {e}"), MsgSev::Warn),
    }
    art.design = Some(live_design);
    art.placed = Some(live_placed);
    art.part = dev.part.clone();
    art.summary = summarize(&art);
    finish(Ok(art));
}

/// Synth-run reports: pre-place timing estimate (summary + clocks +
/// groups + clock networks + CDC + methodology, but no placed paths) plus
/// pack/power when a device model loads. Anything skipped is a loud
/// warning, never zeros passed off as results.
fn run_synth_reports(
    design: &helion_ir::Design,
    xdc: &helion_sta::Constraints,
    clks: &[helion_sta::Clock],
    part_req: &str,
    art: &mut RunArtifacts,
    emit: &impl Fn(String, MsgSev),
) {
    let mut timing_opt: Option<helion_sta::TimingResult> = None;
    match helion_sta::report_timing(design, clks) {
        Ok(t) => {
            let summary = helion_sta::report_timing_summary(clks, xdc, Some(&t));
            emit(
                format!(
                    "report_timing WNS_PS={} TNS_PS={} endpoints={}",
                    wns_text(&summary),
                    summary.tns_ps,
                    summary.endpoints
                ),
                MsgSev::Info,
            );
            timing_opt = Some(t);
        }
        Err(e) => emit(format!("WARN: report_timing skipped: {e}"), MsgSev::Warn),
    }
    let summary = helion_sta::report_timing_summary(clks, xdc, timing_opt.as_ref());
    let timing = timing_state_from(&summary, timing_opt.as_ref(), design, None, xdc);
    emit(
        format!(
            "report_timing groups={} clocks={} cdc={} methodology={}",
            timing.groups.len(),
            timing.clocks.len(),
            timing.cdc.len(),
            timing.methodology.len()
        ),
        MsgSev::Info,
    );
    art.timing = Some(timing);
    match resolve_device(part_req, emit) {
        Ok(dev) => {
            match helion_pack::pack(design, &dev) {
                Ok(packed) => {
                    emit(
                        format!(
                            "pack LUTFF={} IOB={} BRAM={} DSP={}",
                            packed.lutffs.len(),
                            packed.iobs.len(),
                            packed.brams.len(),
                            packed.macs.len()
                        ),
                        MsgSev::Info,
                    );
                    art.util = Some(util_state_from(&packed, design, &dev));
                }
                Err(e) => emit(format!("WARN: utilization skipped: {e}"), MsgSev::Warn),
            }
            let pr =
                helion_sta::report_power(&dev, Some(design), None, clks, &xdc.operating_conditions);
            emit(
                format!(
                    "report_power TOTAL_UW={} DYNAMIC_UW={}",
                    pr.total_uw, pr.dynamic_uw
                ),
                MsgSev::Info,
            );
            art.power = Some(power_state_from(&pr, &xdc.operating_conditions));
        }
        Err(e) => emit(
            format!("WARN: utilization/power skipped: {e}"),
            MsgSev::Warn,
        ),
    }
    emit(
        "Timing paths, DRC, signoff, and bitstream run after placement (Run Implementation)".into(),
        MsgSev::Info,
    );
}

/// Sibling `.sdc`/`.xdc` first (mirroring `ide.rs:22753 load_sibling_sdc`),
/// else an implicit 10 ns clock (mirroring `clocks_for_sta`'s fallback).
fn load_run_constraints(
    source: &PathBuf,
    emit: &impl Fn(String, MsgSev),
) -> (helion_sta::Constraints, Vec<helion_sta::Clock>) {
    let mut xdc = helion_sta::Constraints::default();
    for ext in ["sdc", "xdc"] {
        let cand = source.with_extension(ext);
        if cand.is_file() {
            match std::fs::read_to_string(&cand)
                .map_err(|e| e.to_string())
                .and_then(|text| helion_sta::load_xdc(&text))
            {
                Ok(c) => {
                    let n = c.clocks.len();
                    xdc = c;
                    emit(
                        format!("read_xdc {} clocks={n}", cand.display()),
                        MsgSev::Info,
                    );
                    break;
                }
                Err(e) => {
                    emit(
                        format!("WARN: read_xdc {} skipped: {e}", cand.display()),
                        MsgSev::Warn,
                    );
                }
            }
        }
    }
    let mut clks = xdc.clocks.clone();
    if clks.is_empty() {
        helion_sta::create_clock(&mut clks, "clk", IMPLICIT_CLOCK_PS, "clk");
        emit(
            "no user SDC — implicit 10ns clock on clk".into(),
            MsgSev::Info,
        );
    }
    (xdc, clks)
}

/// Project part first, HL10T fallback with a loud warning (never silent).
fn resolve_device(
    part_req: &str,
    emit: &impl Fn(String, MsgSev),
) -> Result<helion_device::Device, String> {
    match helion_device::Device::load_part(part_req) {
        Ok(dev) => Ok(dev),
        Err(first) => {
            if part_req == FALLBACK_PART {
                return Err(format!(
                    "no Helion device model for part {part_req}: {first}"
                ));
            }
            emit(
                format!(
                    "WARN: part {part_req} has no Helion HAD model ({first}); using {FALLBACK_PART}"
                ),
                MsgSev::Warn,
            );
            helion_device::Device::load_part(FALLBACK_PART).map_err(|e| {
                format!("no Helion device model (tried {part_req}, {FALLBACK_PART}): {e}")
            })
        }
    }
}

/// Elaborate by extension, mirroring `synth_hdl_path`
/// (`crates/helion-gui/src/lib.rs:90`): `.prj` multi-file, `.vhd`/`.vhdl`
/// through helion-vhdl, else SystemVerilog.
fn synth_source(source: &PathBuf) -> Result<helion_ir::Design, String> {
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "prj" => synth_prj_source(source),
        "vhd" | "vhdl" => helion_vhdl::synth_vhdl_path(source),
        _ => helion_sv::synth_sv_path(source),
    }
}

/// Multi-file `.prj` elaboration, mirroring `synth_prj_path`
/// (`crates/helion-gui/src/lib.rs:133`).
fn synth_prj_source(path: &PathBuf) -> Result<helion_ir::Design, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut prj = helion_proj::load_prj(&text)?;
    let _ = helion_proj::expand_ip_packages(&mut prj, path)?;
    let mut rtl: Vec<PathBuf> = Vec::new();
    for s in &prj.sources {
        let resolved = helion_proj::resolve_prj_path(path, s);
        if !resolved.exists() {
            return Err(format!(
                "project source {s}: not found (tried {})",
                resolved.display()
            ));
        }
        let is_sv = resolved
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "sv" | "v"));
        if is_sv {
            rtl.push(resolved);
        }
    }
    if rtl.is_empty() {
        return Err("project has no SystemVerilog sources".into());
    }
    if rtl.len() == 1 && prj.top.is_none() {
        return helion_sv::synth_sv_path(&rtl[0]);
    }
    let mut owned: Vec<(String, String)> = Vec::new();
    for p in &rtl {
        let src = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        owned.push((p.display().to_string(), src));
    }
    let refs: Vec<(&str, &str)> = owned
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let params: std::collections::HashMap<String, u128> = std::collections::HashMap::new();
    let opts = helion_sv::SvCompileOpts::default();
    let (d, _) = helion_sv::elaborate_sv_sources(&refs, prj.top.as_deref(), &params, &opts)?;
    Ok(d)
}

fn wns_text(summary: &helion_sta::TimingSummary) -> String {
    summary
        .wns_ps
        .map(|w| w.to_string())
        .unwrap_or_else(|| "n/a".into())
}

fn summarize(art: &RunArtifacts) -> String {
    let mut parts = vec![format!("{} cells", art.cells)];
    if let Some(t) = &art.timing {
        if let Some(wns) = t.wns_ps {
            parts.push(format!("WNS {:.3}ns", wns as f64 / 1000.0));
        }
        parts.push(format!("{} endpoints", t.endpoints));
    }
    parts.join(", ")
}

/// Full timing state from one STA analysis: summary slacks plus clocks,
/// groups, endpoint paths with UG903 pin rows (placed runs only), clock
/// networks, CDC, methodology, and the raw delay split.
fn timing_state_from(
    s: &helion_sta::TimingSummary,
    timing: Option<&helion_sta::TimingResult>,
    design: &helion_ir::Design,
    placed: Option<&helion_place::Placed>,
    xdc: &helion_sta::Constraints,
) -> TimingState {
    let clks = s.clocks.clone();
    let networks = helion_sta::report_clock_networks(&clks, Some(design), placed);
    let cdc = helion_sta::report_cdc(&clks, xdc, timing, Some(design));
    let methodology = helion_sta::report_methodology(&clks, xdc, timing, Some(design));
    let (r2r_ps, iob_ps, setup_ps, hold_ps, hold_slack_ps, route_ps, clk_net_ps) =
        timing.map_or((0, 0, 0, 0, 0, 0, 0), |t| {
            (
                t.r2r_ps,
                t.iob_ps,
                t.setup_ps,
                t.hold_ps,
                t.hold_slack_ps,
                t.route_ps,
                t.clk_net_ps,
            )
        });
    TimingState {
        wns_ps: s.wns_ps,
        tns_ps: s.tns_ps,
        whs_ps: s.whs_ps,
        ths_ps: s.ths_ps,
        failing_endpoints: s.failing_setup + s.failing_hold,
        failing_setup: s.failing_setup,
        failing_hold: s.failing_hold,
        endpoints: s.endpoints,
        has_run: true,
        clocks: clks.iter().map(clock_row_from).collect(),
        groups: s.groups.iter().map(group_row_from).collect(),
        paths: match (timing, placed) {
            (Some(t), Some(p)) => extract_timing_paths(design, t, p),
            _ => Vec::new(),
        },
        clock_networks: networks.clocks.iter().map(clock_net_row_from).collect(),
        cdc: cdc.violations.iter().map(cdc_row_from).collect(),
        methodology: methodology.checks.iter().map(method_row_from).collect(),
        r2r_ps,
        iob_ps,
        setup_ps,
        hold_ps,
        hold_slack_ps,
        route_ps,
        clk_net_ps,
        design: helion_ir::resolve(design.name).to_string(),
        voltage_mv: xdc.operating_conditions.voltage_mv,
        temp_c: xdc.operating_conditions.temperature_c,
    }
}

fn clock_row_from(c: &helion_sta::Clock) -> TimingClock {
    TimingClock {
        name: c.name.clone(),
        period_ps: c.period_ps,
        source: c.source.clone(),
        generated: c.generated,
        master: c.master.clone(),
    }
}

fn group_row_from(g: &helion_sta::TimingSummaryGroup) -> TimingGroupRow {
    TimingGroupRow {
        name: g.name.clone(),
        from: g.from.clone(),
        to: g.to.clone(),
        kind: match g.kind {
            helion_sta::PathGroupKind::IntraClock => TimingGroupKind::Intra,
            helion_sta::PathGroupKind::InterClock => TimingGroupKind::Inter,
            helion_sta::PathGroupKind::Other => TimingGroupKind::Other,
        },
        wns_ps: g.wns_ps,
        tns_ps: g.tns_ps,
        endpoints: g.endpoints,
    }
}

fn clock_net_row_from(c: &helion_sta::ClockNetwork) -> ClockNetRow {
    ClockNetRow {
        name: c.name.clone(),
        period_ps: c.period_ps,
        net: c.net.clone(),
        loads: c.n_loads,
        buffers: c.n_buffers,
        insertion_ps: c.insertion_ps,
    }
}

fn cdc_row_from(v: &helion_sta::CdcViolation) -> CdcRow {
    CdcRow {
        from: v.from.clone(),
        to: v.to.clone(),
        severity: v.severity.as_str().into(),
        check: v.check.clone(),
        endpoints: v.endpoints,
        sync: v.synchronizer,
        wns_ps: v.wns_ps,
    }
}

fn method_row_from(v: &helion_sta::MethodologyViolation) -> MethodRow {
    MethodRow {
        id: v.id.clone(),
        severity: v.severity.as_str().into(),
        category: v.category.clone(),
        objects: v.objects.clone(),
        message: v.message.clone(),
    }
}

/// STA endpoint paths with UG903 pin rows, mirroring `extract_timing_paths`
/// (`crates/helion-gui/src/ide.rs:27308`): one path per HFF (register) and
/// per IOB out (output), cells/nets walked off the HNF, pin delays from the
/// `helion_sta` arc constants (`FF_CKQ_PS`/`LUT_PS`/`PIN_PS`/`SETUP_PS`),
/// locations from the placed site map. Caps match the GUI (256 register +
/// 128 output paths past 2k cells) so Ibex-scale runs stay bounded.
fn extract_timing_paths(
    design: &helion_ir::Design,
    t: &helion_sta::TimingResult,
    placed: &helion_place::Placed,
) -> Vec<TimingPathRow> {
    let site_map = build_site_map(placed);
    let (max_reg, max_iob) = if design.cells.len() > 2_000 {
        (256usize, 128usize)
    } else {
        (usize::MAX, usize::MAX)
    };
    let mut paths = Vec::new();
    let mut reg_count = 0usize;
    for c in &design.cells {
        if !matches!(c.kind, helion_ir::CellKind::Hff) {
            continue;
        }
        if reg_count >= max_reg {
            break;
        }
        reg_count += 1;
        let cname = helion_ir::resolve(c.name);
        let (start, lut) = reg_path_start(design, cname);
        let pins = r2r_pin_rows(design, &site_map, &start, lut.as_deref(), cname);
        let delay_ps = pins.last().map(|p| p.path_ps).unwrap_or(t.r2r_ps);
        paths.push(TimingPathRow {
            name: format!("{start}->{cname}"),
            from: start,
            to: cname.to_string(),
            slack_ps: t.wns_ps,
            delay_ps,
            group: TimingPathGroup::Register,
            pins,
        });
    }
    let mut iob_count = 0usize;
    for c in &design.cells {
        if !matches!(c.kind, helion_ir::CellKind::IobOut) {
            continue;
        }
        if iob_count >= max_iob {
            break;
        }
        iob_count += 1;
        let cname = helion_ir::resolve(c.name);
        let start = iob_path_start(design, cname);
        let pins = iob_pin_rows(design, &site_map, t, &start, cname);
        let delay_ps = pins.last().map(|p| p.path_ps).unwrap_or(t.iob_ps);
        paths.push(TimingPathRow {
            name: format!("{start}->{cname}"),
            from: start,
            to: cname.to_string(),
            slack_ps: t.wns_ps,
            delay_ps,
            group: TimingPathGroup::Output,
            pins,
        });
    }
    // Worst slack first so the detail tables open on the critical path.
    paths.sort_by_key(|p| p.slack_ps);
    paths
}

/// Packed cell name -> placed HAD site id (`CLB_XnYm`), mirroring
/// `helion_sta::build_bel_map` over the same `Placed` vectors.
fn build_site_map(placed: &helion_place::Placed) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for (i, lf) in placed.packed.lutffs.iter().enumerate() {
        let Some((site, _)) = placed.lutff_sites.get(i) else {
            continue;
        };
        let sid = site.id();
        if !lf.lut_cell.is_empty() {
            map.entry(lf.lut_cell.clone())
                .or_insert_with(|| sid.clone());
        }
        if !lf.ff_cell.is_empty() {
            map.entry(lf.ff_cell.clone()).or_insert(sid.clone());
        }
    }
    for (i, iob) in placed.packed.iobs.iter().enumerate() {
        if !iob.cell.is_empty() {
            if let Some(s) = placed.iob_sites.get(i) {
                map.entry(iob.cell.clone()).or_insert_with(|| s.id());
            }
        }
    }
    for (i, mac) in placed.packed.macs.iter().enumerate() {
        if !mac.cell.is_empty() {
            if let Some(s) = placed.mac_sites.get(i) {
                map.entry(mac.cell.clone()).or_insert_with(|| s.id());
            }
        }
    }
    for (i, bram) in placed.packed.brams.iter().enumerate() {
        if !bram.cell.is_empty() {
            if let Some(s) = placed.bram_sites.get(i) {
                map.entry(bram.cell.clone()).or_insert_with(|| s.id());
            }
        }
    }
    map
}

/// Register path startpoint: the driving FF's Q through the D-net LUT (or
/// the endpoint's own clock net when no driver is found), mirroring the
/// GUI walk.
fn reg_path_start(design: &helion_ir::Design, endpoint: &str) -> (String, Option<String>) {
    let mut start = hff_clk_net(design, endpoint).to_string();
    let mut lut = None;
    let dnet = design
        .net_on(helion_ir::intern(endpoint), helion_ir::intern("D"))
        .map(helion_ir::resolve);
    if let Some(dnet) = dnet.map(str::to_string) {
        if let Some(n) = design.net(&dnet) {
            for e in &n.endpoints {
                let ec = helion_ir::resolve(e.cell);
                if e.pin == helion_ir::intern("O") && ec != endpoint {
                    lut = Some(ec.to_string());
                    for i in 0..6u8 {
                        let pin = format!("I{i}");
                        if let Some(inet) = design
                            .net_on(e.cell, helion_ir::intern(&pin))
                            .map(helion_ir::resolve)
                            .map(str::to_string)
                        {
                            if let Some(nn) = design.net(&inet) {
                                for ee in &nn.endpoints {
                                    let eec = helion_ir::resolve(ee.cell);
                                    if ee.pin == helion_ir::intern("Q") && eec != endpoint {
                                        start = eec.to_string();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    (start, lut)
}

/// Output path startpoint: the FF Q driving the IOB I pin.
fn iob_path_start(design: &helion_ir::Design, iob: &str) -> String {
    let mut start = iob.to_string();
    if let Some(inet) = design
        .net_on(helion_ir::intern(iob), helion_ir::intern("I"))
        .map(helion_ir::resolve)
        .map(str::to_string)
    {
        if let Some(n) = design.net(&inet) {
            for e in &n.endpoints {
                let ec = helion_ir::resolve(e.cell);
                if e.pin == helion_ir::intern("Q") && ec != iob {
                    start = ec.to_string();
                }
            }
        }
    }
    start
}

/// HFF clock pin is `CLK` (not UNISIM `C`), mirroring `hff_clk_net`.
fn hff_clk_net(design: &helion_ir::Design, cell: &str) -> &'static str {
    design
        .net_on(helion_ir::intern(cell), helion_ir::intern("CLK"))
        .or_else(|| design.net_on(helion_ir::intern(cell), helion_ir::intern("C")))
        .map(helion_ir::resolve)
        .unwrap_or("clk")
}

fn net_fanout(design: &helion_ir::Design, net: &str) -> usize {
    design
        .net(net)
        .map(|n| n.endpoints.len().saturating_sub(1).max(1))
        .unwrap_or(1)
}

fn is_hff(design: &helion_ir::Design, name: &str) -> bool {
    design
        .cell(name)
        .is_some_and(|c| matches!(c.kind, helion_ir::CellKind::Hff))
}

fn push_pin(
    pins: &mut Vec<TimingPinRow>,
    cell: &str,
    pin: &str,
    delay_type: &str,
    incr_ps: i64,
    net: &str,
    fanout: usize,
    location: String,
) {
    let path_ps = pins.last().map(|p| p.path_ps).unwrap_or(0) + incr_ps;
    pins.push(TimingPinRow {
        cell: cell.into(),
        pin: format!("{cell}/{pin}"),
        delay_type: delay_type.into(),
        incr_ps,
        path_ps,
        net: net.into(),
        fanout,
        location,
    });
}

/// UG903 r2r pin table: CkQ + LUT fanin x PIN + LUT comb + setup,
/// mirroring `r2r_pin_rows_fast`.
fn r2r_pin_rows(
    design: &helion_ir::Design,
    site_map: &std::collections::HashMap<String, String>,
    start: &str,
    lut: Option<&str>,
    endpoint: &str,
) -> Vec<TimingPinRow> {
    let site = |c: &str| site_map.get(c).cloned().unwrap_or_default();
    let mut pins = Vec::new();
    if is_hff(design, start) {
        let clk_net = hff_clk_net(design, start);
        push_pin(
            &mut pins,
            start,
            "CLK",
            "clk",
            0,
            clk_net,
            net_fanout(design, clk_net),
            site(start),
        );
        let qnet = design
            .net_on(helion_ir::intern(start), helion_ir::intern("Q"))
            .map(helion_ir::resolve)
            .unwrap_or("");
        push_pin(
            &mut pins,
            start,
            "Q",
            "cell",
            helion_sta::FF_CKQ_PS,
            qnet,
            net_fanout(design, qnet),
            site(start),
        );
    }
    if let Some(lut) = lut {
        let spur = helion_ir::intern(lut);
        for i in 0..6u8 {
            let pn = format!("I{i}");
            if let Some(inet) = design
                .net_on(spur, helion_ir::intern(&pn))
                .map(helion_ir::resolve)
            {
                push_pin(
                    &mut pins,
                    lut,
                    &pn,
                    "net",
                    helion_sta::PIN_PS,
                    inet,
                    net_fanout(design, inet),
                    site(lut),
                );
            }
        }
        let onet = design
            .net_on(spur, helion_ir::intern("O"))
            .map(helion_ir::resolve)
            .unwrap_or("");
        push_pin(
            &mut pins,
            lut,
            "O",
            "cell",
            helion_sta::LUT_PS,
            onet,
            net_fanout(design, onet),
            site(lut),
        );
    }
    let dnet = design
        .net_on(helion_ir::intern(endpoint), helion_ir::intern("D"))
        .map(helion_ir::resolve)
        .unwrap_or("");
    push_pin(
        &mut pins,
        endpoint,
        "D",
        "setup",
        helion_sta::SETUP_PS,
        dnet,
        net_fanout(design, dnet),
        site(endpoint),
    );
    pins
}

/// UG903 IOB pin table: CkQ + route net + pad, mirroring
/// `iob_pin_rows_fast` (sums to STA `iob_ps`).
fn iob_pin_rows(
    design: &helion_ir::Design,
    site_map: &std::collections::HashMap<String, String>,
    t: &helion_sta::TimingResult,
    start: &str,
    iob: &str,
) -> Vec<TimingPinRow> {
    let site = |c: &str| site_map.get(c).cloned().unwrap_or_default();
    let mut pins = Vec::new();
    let has_ff = is_hff(design, start);
    let ckq = if has_ff && t.iob_ps > 0 {
        helion_sta::FF_CKQ_PS
    } else {
        0
    };
    let route = if t.iob_ps > 0 { t.route_ps } else { 0 };
    let pad_ps = (t.iob_ps - ckq - route).max(0);
    if has_ff {
        let clk_net = hff_clk_net(design, start);
        push_pin(
            &mut pins,
            start,
            "CLK",
            "clk",
            0,
            clk_net,
            net_fanout(design, clk_net),
            site(start),
        );
        let qnet = design
            .net_on(helion_ir::intern(start), helion_ir::intern("Q"))
            .map(helion_ir::resolve)
            .unwrap_or("");
        push_pin(
            &mut pins,
            start,
            "Q",
            "cell",
            ckq,
            qnet,
            net_fanout(design, qnet),
            site(start),
        );
    }
    let inet = design
        .net_on(helion_ir::intern(iob), helion_ir::intern("I"))
        .map(helion_ir::resolve)
        .unwrap_or("");
    push_pin(
        &mut pins,
        iob,
        "I",
        "net",
        route,
        inet,
        net_fanout(design, inet),
        site(iob),
    );
    let pad = design
        .net_on(helion_ir::intern(iob), helion_ir::intern("PAD"))
        .map(helion_ir::resolve)
        .unwrap_or("");
    push_pin(
        &mut pins,
        iob,
        "PAD",
        "iob",
        pad_ps,
        pad,
        net_fanout(design, pad),
        site(iob),
    );
    pins
}

/// Utilization from packed cells + device caps (mirroring
/// `ide.rs:25129 refresh_reports`; one FF per LUT site class as in
/// `ide.rs:20467 vcd_power_report`) plus hierarchy occupancy from the
/// elaborated design (mirroring `hierarchical_occupancy`).
fn util_state_from(
    packed: &helion_pack::Packed,
    design: &helion_ir::Design,
    dev: &helion_device::Device,
) -> UtilState {
    UtilState {
        luts_used: packed
            .lutffs
            .iter()
            .filter(|e| !e.lut_cell.is_empty())
            .count() as u32,
        luts_avail: dev.lut6_count(),
        regs_used: packed
            .lutffs
            .iter()
            .filter(|e| !e.ff_cell.is_empty())
            .count() as u32,
        regs_avail: dev.lut6_count(),
        bram_used: packed.brams.len() as u32,
        bram_avail: dev.n_bram,
        io_used: packed.iobs.len() as u32,
        io_avail: dev.iob_sites().count() as u32,
        dsp_used: packed.macs.len() as u32,
        dsp_avail: dev.n_dsp,
        bufg_used: 0,
        bufg_avail: 0,
        hier: hierarchical_occupancy(design),
        design: helion_ir::resolve(design.name).to_string(),
    }
}

/// Top module plus one row per elaborated instance (cells whose names carry
/// the `instance_` prefix), mirroring `hierarchical_occupancy`
/// (`crates/helion-gui/src/ide.rs:20784`).
fn hierarchical_occupancy(design: &helion_ir::Design) -> Vec<HierRow> {
    let mut top = HierRow {
        name: helion_ir::resolve(design.name).to_string(),
        child: false,
        ..HierRow::default()
    };
    for c in &design.cells {
        bump_hier_cell(&mut top, &c.kind);
    }
    let mut out = vec![top];
    for inst in &design.instances {
        let iname = helion_ir::resolve(inst.name);
        let pfx = format!("{iname}_");
        let mut h = HierRow {
            name: iname.to_string(),
            child: true,
            ..HierRow::default()
        };
        for c in &design.cells {
            let cname = helion_ir::resolve(c.name);
            if cname == iname || cname.starts_with(&pfx) {
                bump_hier_cell(&mut h, &c.kind);
            }
        }
        out.push(h);
    }
    out
}

fn bump_hier_cell(h: &mut HierRow, kind: &helion_ir::CellKind) {
    match kind {
        helion_ir::CellKind::Lut6 { .. } => h.lut += 1,
        helion_ir::CellKind::Hff => h.ff += 1,
        helion_ir::CellKind::IobOut => h.io += 1,
        helion_ir::CellKind::Bram18 | helion_ir::CellKind::Bram18e1 { .. } => h.bram += 1,
        helion_ir::CellKind::Mac27 | helion_ir::CellKind::Dsp48e1 { .. } => h.dsp += 1,
        _ => {}
    }
}

fn power_state_from(
    r: &helion_sta::PowerReport,
    oc: &helion_sta::OperatingConditions,
) -> PowerState {
    PowerState {
        total_mw: (r.total_uw.max(0) / 1000) as u32,
        dynamic_mw: (r.dynamic_uw.max(0) / 1000) as u32,
        static_mw: (r.static_uw.max(0) / 1000) as u32,
        clocks_uw: r.clocks_uw.max(0) as u32,
        logic_uw: r.logic_uw.max(0) as u32,
        signals_uw: r.signals_uw.max(0) as u32,
        io_uw: r.io_uw.max(0) as u32,
        bram_uw: r.bram_uw.max(0) as u32,
        dsp_uw: r.dsp_uw.max(0) as u32,
        junction_deci_c: if oc.temperature_set {
            (oc.temperature_c.max(0) * 10) as u32
        } else {
            250
        },
        part: r.part.clone(),
        voltage_mv: r.voltage_mv,
        temp_c: r.temperature_c,
        f_mhz: r.f_mhz,
        // Runs are vectorless (no VCD switching activity annotates any net),
        // so confidence is Low by the `analyze_power` rule
        // (`helion-device/src/power.rs:2570`: annotated < 25% -> Low).
        confidence: "Low".into(),
        confidence_note: "Vectorless estimate — no VCD switching activity".into(),
    }
}

fn drc_item_from(v: &helion_drc::DrcViolation) -> DrcItem {
    DrcItem {
        id: v.id.clone(),
        severity: match v.severity {
            helion_drc::DrcSeverity::Error => DrcSev::Error,
            helion_drc::DrcSeverity::Warning => DrcSev::Warning,
            helion_drc::DrcSeverity::Advisory => DrcSev::Advisory,
        },
        objects: v.objects.clone(),
        message: v.message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impl_strategy_labels_map_to_real_presets() {
        assert_eq!(impl_strategy_for("TimingExplore").preset, "TimingExplore");
        assert_eq!(impl_strategy_for("RuntimeOpt").opt_level, 0);
        assert_eq!(
            impl_strategy_for("Helion Implementation Defaults").preset,
            "Default",
            "legacy labels fall back to Default"
        );
    }
    use std::time::{Duration, Instant};

    fn counter_source() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv")
    }

    fn pump_until_idle(s: &mut AppState, timeout: Duration) {
        let start = Instant::now();
        while s.runs.is_active() {
            poll_runs(s);
            if !s.runs.is_active() {
                break;
            }
            assert!(
                start.elapsed() < timeout,
                "run did not finish within {timeout:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        poll_runs(s);
    }

    #[test]
    fn launch_flips_running_echoes_verb_and_arms_active() {
        let mut s = AppState::demo();
        s.runs.source = Some(counter_source());
        launch(&mut s, RunKind::Synth);
        assert_eq!(s.flow.synth, RunStatus::Running);
        assert_eq!(s.status, StatusVerb::SynthesisRunning);
        assert!(
            s.console
                .lines
                .iter()
                .any(|l| l.text == "launch_runs synth_1"),
            "launch must echo its Tcl verb"
        );
        assert!(s.runs.is_active(), "worker must be armed");
        assert_eq!(s.runs.active_kind(), Some(RunKind::Synth));
        cancel_run(&mut s);
        pump_until_idle(&mut s, Duration::from_secs(60));
    }

    #[test]
    fn resolve_source_stages_sibling_sdc_next_to_staged_rtl() {
        let mut s = AppState::demo();
        let real = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/complex.sv");
        let text = std::fs::read_to_string(&real).expect("complex.sv fixture");
        s.editor.tabs = vec!["complex.sv".into()];
        s.editor.active = 0;
        s.editor.lines = text.lines().map(str::to_string).collect();
        s.editor.paths.insert("complex.sv".into(), real.clone());
        let staged = resolve_source(&s).expect("staging must succeed");
        assert!(staged.is_file(), "staged RTL must exist");
        let staged_sdc = staged.with_extension("sdc");
        assert!(
            staged_sdc.is_file(),
            "the tab's sibling SDC must stage too, or the run silently loses clocks"
        );
        assert_eq!(
            std::fs::read_to_string(&staged_sdc).unwrap(),
            std::fs::read_to_string(real.with_extension("sdc")).unwrap(),
            "staged SDC must match the on-disk sibling"
        );
        let _ = std::fs::remove_file(&staged);
        let _ = std::fs::remove_file(&staged_sdc);
    }

    #[test]
    fn launch_reapplies_pin_constraints_to_fresh_elaboration() {
        let mut s = AppState::demo();
        s.runs.source = Some(counter_source());
        let mut design = helion_sv::synth_sv_path(&counter_source()).unwrap();
        design.set_loc("led", "IOB_X0Y0").unwrap();
        design.set_iostandard("led", "LVCMOS33").unwrap();
        s.runs.live_design = Some(design);
        launch(&mut s, RunKind::Synth);
        pump_until_idle(&mut s, Duration::from_secs(120));
        assert_eq!(s.flow.synth, RunStatus::Complete);
        let live = s.runs.live_design.as_ref().unwrap();
        let led = live
            .ports
            .iter()
            .find(|p| helion_ir::resolve(p.name) == "led")
            .unwrap();
        assert_eq!(led.attrs.get("LOC"), Some("IOB_X0Y0"));
        assert_eq!(led.attrs.get("IOSTANDARD"), Some("LVCMOS33"));
        assert!(
            s.runs
                .synth_log
                .iter()
                .any(|l| l.text == "set_property pin constraints applied=2"),
            "worker must announce the re-applied constraints"
        );
    }

    #[test]
    fn poll_pumps_worker_lines_into_console_and_log_live() {
        let mut s = AppState::demo();
        let before = s.console.lines.len();
        deliver(
            &mut s,
            RunKind::Synth,
            RunEvent::Line {
                text: "synth_design cells=12".into(),
                sev: MsgSev::Info,
            },
        );
        assert_eq!(
            s.console.lines.len(),
            before + 1,
            "console must gain the line live"
        );
        assert!(
            s.runs
                .synth_log
                .iter()
                .any(|l| l.text == "synth_design cells=12"),
            "synth log buffer must gain the line live"
        );
        assert!(
            s.runs.impl_log.is_empty(),
            "impl log must not gain synth lines"
        );
    }

    #[test]
    fn completion_fills_reports_and_flips_complete_with_toast() {
        let mut s = AppState::demo();
        let art = RunArtifacts {
            timing: Some(TimingState {
                wns_ps: Some(2061),
                tns_ps: 0,
                whs_ps: Some(96),
                ths_ps: 0,
                failing_endpoints: 0,
                failing_setup: 0,
                failing_hold: 0,
                endpoints: 112,
                has_run: true,
                ..TimingState::default()
            }),
            util: Some(UtilState {
                luts_used: 12,
                luts_avail: 100,
                regs_used: 4,
                regs_avail: 100,
                bram_used: 0,
                bram_avail: 8,
                io_used: 2,
                io_avail: 32,
                dsp_used: 0,
                dsp_avail: 4,
                bufg_used: 0,
                bufg_avail: 0,
                ..UtilState::default()
            }),
            power: Some(PowerState {
                total_mw: 3,
                dynamic_mw: 1,
                static_mw: 2,
                clocks_uw: 100,
                logic_uw: 200,
                signals_uw: 300,
                io_uw: 400,
                bram_uw: 0,
                dsp_uw: 0,
                junction_deci_c: 250,
                ..PowerState::default()
            }),
            drc: Some(vec![DrcItem {
                id: "PDRC-190".into(),
                severity: DrcSev::Warning,
                objects: "clk_pin".into(),
                message: "High fanout net".into(),
            }]),
            signoff: Some(RunSignoff {
                bitgen_allowed: true,
                summary: "bitgen_signoff allowed=true errors=0".into(),
                unrouted: 0,
                overused: 0,
            }),
            bitstream: Some(BitstreamState {
                idcode: 0x01EC,
                hash: 0x1234,
                bytes: 512,
                frames: 4,
                configured: 3,
                has_run: true,
            }),
            summary: "12 cells, 5 endpoints".into(),
            cells: 12,
            design: None,
            placed: None,
            part: String::new(),
        };
        deliver(&mut s, RunKind::Synth, RunEvent::Finished(Ok(art)));
        assert_eq!(s.flow.synth, RunStatus::Complete);
        assert_eq!(s.status, StatusVerb::SynthesisComplete);
        assert!(s.timing.has_run, "timing must fill from artifacts");
        assert_eq!(s.timing.wns_ps, Some(2061));
        assert_eq!(s.util.luts_used, 12);
        assert_eq!(s.power.total_mw, 3);
        assert_eq!(s.drc.items.len(), 1);
        assert!(s.drc.has_run, "drc delivery must mark the report evaluated");
        assert_eq!(s.drc.bitgen_allowed, Some(true), "signoff must fill");
        assert!(s.bitstream.has_run, "bitstream must fill");
        assert_eq!(s.bitstream.bytes, 512);
        assert!(!s.runs.is_active(), "terminal event must disarm the worker");
        assert!(
            s.toasts.items.iter().any(|t| t.kind == ToastKind::Success),
            "completion must toast success"
        );
    }

    #[test]
    fn failure_flips_failed_verb_with_error_toast_and_messages() {
        let mut s = AppState::demo();
        start_run(
            &mut s,
            RunKind::Synth,
            PathBuf::from("/nonexistent/missing.sv"),
            "HL10T-C32-1".into(),
        )
        .expect("start must arm the worker");
        pump_until_idle(&mut s, Duration::from_secs(60));
        assert_eq!(s.flow.synth, RunStatus::Failed);
        assert_eq!(s.status, StatusVerb::Failed);
        assert!(
            s.console.lines.iter().any(|l| l.sev == MsgSev::Error),
            "failure must surface an error line (Messages tab reads these)"
        );
        assert!(
            s.toasts.items.iter().any(|t| t.kind == ToastKind::Error),
            "failure must toast the error"
        );
        assert!(!s.timing.has_run, "failed runs must not fill timing");
    }

    #[test]
    fn cancel_request_sets_flag_and_cancelled_event_resets_to_idle() {
        let mut s = AppState::demo();
        s.runs.source = Some(counter_source());
        launch(&mut s, RunKind::Synth);
        assert!(s.runs.is_active());
        cancel_run(&mut s);
        assert!(s.runs.cancel_requested(), "cancel must set the worker flag");
        assert!(
            s.console
                .lines
                .iter()
                .any(|l| l.text == "cancel_runs synth_1"),
            "cancel must echo its verb"
        );
        // The worker answers Cancelled between stages; deliver it as it would.
        deliver(&mut s, RunKind::Synth, RunEvent::Cancelled);
        assert_eq!(s.flow.synth, RunStatus::NotStarted);
        assert_eq!(s.status, StatusVerb::Ready);
        assert!(!s.runs.is_active());
        assert!(
            s.toasts.items.iter().any(|t| t.kind == ToastKind::Warning),
            "cancel must toast a warning"
        );
        pump_until_idle(&mut s, Duration::from_secs(60));
    }

    #[test]
    fn second_start_while_busy_is_rejected() {
        let mut s = AppState::demo();
        start_run(
            &mut s,
            RunKind::Synth,
            counter_source(),
            "HL10T-C32-1".into(),
        )
        .expect("first start must arm");
        let err = start_run(
            &mut s,
            RunKind::Impl,
            counter_source(),
            "HL10T-C32-1".into(),
        )
        .expect_err("second start must be rejected while busy");
        assert!(
            err.contains("already running"),
            "reason must name the busy run: {err}"
        );
        cancel_run(&mut s);
        pump_until_idle(&mut s, Duration::from_secs(60));
    }

    #[test]
    fn poll_with_no_active_run_is_quiet() {
        let mut s = AppState::demo();
        assert!(!poll_runs(&mut s), "idle poll must report no change");
    }

    #[test]
    fn impl_run_on_counter_binds_full_paths_clocks_checks_and_bitstream() {
        // deep-02: every backend report result must land in state — timing
        // paths/clocks/groups/detail, CDC, clock networks, methodology,
        // hierarchy, signoff, and bitstream — from REAL engine artifacts.
        let src = counter_source();
        let mut s = AppState::demo();
        start_run(&mut s, RunKind::Impl, src, "HL10T-C32-1".into()).expect("start");
        pump_until_idle(&mut s, Duration::from_secs(180));
        assert_eq!(
            s.flow.impl_run,
            RunStatus::Complete,
            "counter impl must succeed"
        );
        assert!(s.timing.has_run, "real timing must fill");
        assert!(!s.timing.clocks.is_empty(), "real STA clocks must fill");
        assert!(!s.timing.groups.is_empty(), "real timing groups must fill");
        assert!(!s.timing.paths.is_empty(), "real timing paths must fill");
        assert!(
            !s.timing.paths[0].pins.is_empty(),
            "real pin-delay rows must fill for the critical path"
        );
        assert!(
            !s.timing.clock_networks.is_empty(),
            "real clock networks must fill"
        );
        assert!(
            !s.util.hier.is_empty(),
            "real hierarchy occupancy must fill"
        );
        assert!(s.power.f_mhz > 0, "real power frequency must fill");
        assert!(s.drc.has_run, "real DRC/signoff must fill");
        assert!(s.bitstream.has_run, "real bitstream must fill");
        assert!(s.bitstream.bytes > 0, "real bitstream packets must fill");
        assert!(
            s.runs
                .impl_log
                .iter()
                .any(|l| l.text.starts_with("write_bitstream")),
            "log must carry the real bitstream stage line"
        );
    }

    #[test]
    fn synth_run_on_counter_produces_real_timing_and_util() {
        let src = counter_source();
        assert!(
            src.is_file(),
            "counter example must exist: {}",
            src.display()
        );
        let mut s = AppState::demo();
        start_run(&mut s, RunKind::Synth, src, "HL10T-C32-1".into()).expect("start");
        pump_until_idle(&mut s, Duration::from_secs(120));
        assert_eq!(
            s.flow.synth,
            RunStatus::Complete,
            "counter synth must succeed"
        );
        assert!(s.timing.has_run, "real timing must fill");
        assert!(s.timing.wns_ps.is_some(), "real WNS must fill");
        assert!(s.timing.endpoints > 0, "real endpoints must fill");
        assert!(s.util.luts_used > 0, "real utilization must fill");
        let blocks = s.power.clocks_uw
            + s.power.logic_uw
            + s.power.signals_uw
            + s.power.io_uw
            + s.power.bram_uw
            + s.power.dsp_uw;
        assert!(blocks > 0, "real power blocks must fill");
        assert!(
            s.runs
                .synth_log
                .iter()
                .any(|l| l.text.starts_with("synth_design")),
            "log must carry the real synth stage line"
        );
    }
}
