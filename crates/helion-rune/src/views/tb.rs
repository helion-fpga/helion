//! Testbench runner: discovery, background runs, results, and history.
//!
//! The Sim surface (`CenterPane::Sim`, rendered by [`crate::views::tb_view`])
//! drives the real `helion-verif` testbench engine:
//!
//! - Discovery: [`discovery_dir`] is the directory of `runs.source`, else the
//!   project location; [`rescan`] runs `discover_testbenches` there, parses
//!   every file (`parse_testbench`) for the detail panel, and loads the
//!   persisted run history (`load_history`). [`poll`] rescans whenever the
//!   directory changes, so the list follows the open project.
//! - Runs: [`launch_testbench`] hands `run_testbench` to a worker thread
//!   (cancel flag + live `TbStreamEvent` stream); `$display`, assertion
//!   failures, and the summary stream into [`SimState::log`] (the console
//!   Simulation tab) as they happen. Finished runs keep their outcome
//!   (failures with file:line, the full log, the VCD path), mount the VCD in
//!   the Wave pane, and append to the history file (`record_history`).
//!   [`run_all`] queues every testbench.
//! - With no testbench, [`launch_design`] free-runs the elaborated design for
//!   the configured RUNTIME_CYCLES at TIMESCALE_PS on the `helion-sim` event
//!   kernel (`helion_sim::vcd::dump_vcd`) and imports the dump, honoring the
//!   Simulation Settings (SIM_TOP, PARAM, LOG_ALL_SIGNALS).
//! - Compile/Elaborate ([`crate::views::sim_settings`]) share the one worker
//!   slot through [`spawn_job`], so at most one simulation job runs.
//!
//! Engine citations: `helion-verif/src/tb.rs` (`discover_testbenches`,
//! `parse_testbench`, `run_testbench`, `record_history`, `load_history`),
//! `helion-sim/src/vcd.rs` (`dump_vcd`).

use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use helion_verif::tb::{self as engine, TbError, TbFile, TbHistoryEntry, TbOutcome, TbStreamEvent};
use rune::Tx;

use crate::state::{AppState, CenterPane, ConsoleLine, MsgSev, RunStatus, ToastKind};
use crate::views::wave::{LIVE_MAX_CYCLES, LiveWave, import_vcd_text};

/// Max worker events drained per frame.
const MAX_SIM_DRAIN: usize = 256;
/// Sim log lines kept (oldest drop first) so long runs stay bounded.
pub const SIM_LOG_KEEP: usize = 5000;

/// Per-testbench run status; labels mirror `helion-verif` `TbStatus`, plus
/// an honest Stopped for user-cancelled runs (not a pass or a fail).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TbStatus {
    #[default]
    Idle,
    Running,
    Passed,
    Failed,
    Stopped,
}

impl TbStatus {
    pub fn label(self) -> &'static str {
        match self {
            TbStatus::Idle => "Not run",
            TbStatus::Running => "Running",
            TbStatus::Passed => "Passed",
            TbStatus::Failed => "Failed",
            TbStatus::Stopped => "Stopped",
        }
    }
}

/// Parsed testbench facts shown in the detail panel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TbInfo {
    pub dut_module: String,
    pub dut_instance: String,
    /// `(dut port, tb signal)`.
    pub ports: Vec<(String, String)>,
    /// `(signal, half period in ns)`.
    pub clocks: Vec<(String, u64)>,
    /// `(name, width)`.
    pub decls: Vec<(String, u32)>,
    /// Class-based / UVM testbench facts (`None` for the stimulus subset).
    pub class: Option<helion_verif::classtb::ClassTbInfo>,
}

/// One discovered testbench: the file, its parse, status, and last result.
#[derive(Clone, Debug)]
pub struct TbItem {
    pub file: TbFile,
    /// Parse summary, or the parser's `file:line: message`.
    pub info: Result<TbInfo, String>,
    pub status: TbStatus,
    /// Last run of this session: the outcome, or the engine error.
    pub last: Option<Result<TbOutcome, TbError>>,
}

impl TbItem {
    fn load(file: TbFile) -> Self {
        let label = file.path.to_string_lossy().into_owned();
        let info = std::fs::read_to_string(&file.path)
            .map_err(|e| format!("{label}: cannot read: {e}"))
            .and_then(|src| {
                // VHDL first: the class-based sniffing is SystemVerilog-only.
                if !helion_verif::vhdl_tb::is_vhdl_path(&file.path)
                    && helion_verif::classtb::is_class_testbench(&src)
                {
                    // Class-based / UVM: helion-svtb front end.
                    return helion_verif::classtb::inspect_class_testbench(&file.path)
                        .map(|c| {
                            let (dut_module, dut_instance) = c.duts.first().cloned().unwrap_or_default();
                            TbInfo { dut_module, dut_instance, class: Some(c), ..TbInfo::default() }
                        })
                        .map_err(|e| e.to_string());
                }
                engine::parse_testbench(&label, &src)
                    .map(|p| TbInfo {
                        dut_module: p.dut_module,
                        dut_instance: p.dut_instance,
                        ports: p.port_map,
                        clocks: p
                            .clocks
                            .into_iter()
                            .map(|c| (c.sig, c.half_period_units))
                            .collect(),
                        decls: p.decls.into_iter().map(|d| (d.name, d.width)).collect(),
                        class: None,
                    })
                    .map_err(|e| e.to_string())
            });
        Self {
            file,
            info,
            status: TbStatus::Idle,
            last: None,
        }
    }

    pub fn name(&self) -> &str {
        &self.file.name
    }
}

/// What a worker job is doing (one at a time).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunTarget {
    /// A discovered testbench (index into [`SimState::tbs`]).
    Testbench(usize),
    /// The design free-running for RUNTIME_CYCLES (no testbench).
    Design,
    /// Simulation-settings Compile step.
    Compile,
    /// Simulation-settings Elaborate step.
    Elaborate,
}

/// Free-run completion payload: the imported wave plus its summary.
#[derive(Clone, Debug, Default)]
pub struct SimArtifacts {
    pub live: LiveWave,
    pub summary: String,
}

/// One worker → UI message.
pub enum SimEvent {
    Line { text: String, sev: MsgSev },
    TbDone(Result<TbOutcome, TbError>),
    Wave(Result<SimArtifacts, String>),
    Compiled(Result<Vec<String>, String>),
    Elaborated(Result<(helion_ir::Design, helion_sv::SvElabReport), String>),
    Cancelled,
}

#[derive(Clone)]
struct ActiveSim {
    target: RunTarget,
    label: String,
    rx: Arc<Mutex<mpsc::Receiver<SimEvent>>>,
    stream: Option<Arc<Mutex<mpsc::Receiver<TbStreamEvent>>>>,
    cancel: Arc<AtomicBool>,
}

impl std::fmt::Debug for ActiveSim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveSim")
            .field("target", &self.target)
            .field("label", &self.label)
            .field("cancelled", &self.cancel.load(Ordering::Relaxed))
            .finish()
    }
}

/// Testbench runner state: the discovered list, selection, VCD toggle, the
/// persisted history, the streamed log, and the at-most-one worker job.
#[derive(Clone, Debug)]
pub struct SimState {
    /// Scenes/tests pin discovery here; `None` follows [`discovery_dir`].
    pub dir_override: Option<PathBuf>,
    /// Directory of the last scan (`None` = never scanned).
    pub scanned: Option<PathBuf>,
    /// Whether any scan ran (a scan of "no directory" still counts).
    pub has_scanned: bool,
    pub tbs: Vec<TbItem>,
    pub selected: Option<usize>,
    /// Dump a VCD from testbench runs (feeds the waveform viewer).
    pub vcd: bool,
    /// Persisted run history (`.helion/tb_history.json`), oldest first.
    pub history: Vec<TbHistoryEntry>,
    /// Manually added testbench files kept in the history file.
    pub manual: Vec<PathBuf>,
    /// Streamed sim log (the console Simulation tab renders it).
    pub log: Vec<ConsoleLine>,
    /// Remaining Run-all queue (testbench indices).
    pub queue: Vec<usize>,
    /// Latest simulated time reported by the running testbench.
    pub progress_ns: u64,
    /// Index into [`SimState::log`] where the running job's lines start.
    pub run_log_start: usize,
    /// Chosen UVM test per testbench file (`+UVM_TESTNAME`); absent = the
    /// test named in the testbench's `run_test()` call.
    pub uvm_test: std::collections::BTreeMap<PathBuf, String>,
    last_run: Option<RunTarget>,
    active: Option<ActiveSim>,
}

impl Default for SimState {
    fn default() -> Self {
        Self {
            dir_override: None,
            scanned: None,
            has_scanned: false,
            tbs: Vec::new(),
            selected: None,
            vcd: true,
            history: Vec::new(),
            manual: Vec::new(),
            log: Vec::new(),
            queue: Vec::new(),
            progress_ns: 0,
            run_log_start: 0,
            uvm_test: std::collections::BTreeMap::new(),
            last_run: None,
            active: None,
        }
    }
}

impl SimState {
    pub fn status(&self, i: usize) -> TbStatus {
        self.tbs.get(i).map(|t| t.status).unwrap_or_default()
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn active_target(&self) -> Option<RunTarget> {
        self.active.as_ref().map(|a| a.target)
    }

    /// Testbench index of the running job, if it is a testbench run.
    pub fn active_tb(&self) -> Option<usize> {
        match self.active_target() {
            Some(RunTarget::Testbench(i)) => Some(i),
            _ => None,
        }
    }

    /// Human label of the running job (`counter_tb`, `the design`, ...).
    pub fn active_label(&self) -> String {
        self.active
            .as_ref()
            .map(|a| a.label.clone())
            .unwrap_or_default()
    }

    pub fn cancel_requested(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|a| a.cancel.load(Ordering::Relaxed))
    }

    /// The last launched simulation (testbench or design), for Restart.
    pub fn last_run(&self) -> Option<RunTarget> {
        self.last_run
    }

    /// History newest first (the table order).
    pub fn history_newest_first(&self) -> Vec<&TbHistoryEntry> {
        self.history.iter().rev().collect()
    }

    fn request_cancel(&self) {
        if let Some(a) = &self.active {
            a.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Drop any live job (flagging cancel first) and reset to idle. The
    /// orphaned worker's sends fail silently and it exits on its own.
    pub fn reset(&mut self) {
        self.request_cancel();
        *self = SimState::default();
    }

    fn push_log(&mut self, text: String, sev: MsgSev) {
        self.log.push(ConsoleLine { text, sev });
        if self.log.len() > SIM_LOG_KEEP {
            let drop = self.log.len() - SIM_LOG_KEEP;
            self.log.drain(..drop);
            self.run_log_start = self.run_log_start.saturating_sub(drop);
        }
    }
}

/// Where testbenches are discovered: the scene/test override, else the
/// directory of the run source, else the project location.
pub fn discovery_dir(s: &AppState) -> Option<PathBuf> {
    if let Some(d) = &s.sim.dir_override {
        return Some(d.clone());
    }
    if let Some(dir) = s.runs.source.as_ref().and_then(|p| p.parent()) {
        if !dir.as_os_str().is_empty() {
            return Some(dir.to_path_buf());
        }
    }
    let loc = s.project.location.trim();
    (!loc.is_empty()).then(|| PathBuf::from(loc))
}

/// Re-discover testbenches and reload the history. Keeps the selection on
/// the same file when it is still there. Returns a one-line summary.
pub fn rescan(s: &mut AppState) -> String {
    let dir = discovery_dir(s);
    let prev = s
        .sim
        .selected
        .and_then(|i| s.sim.tbs.get(i))
        .map(|t| t.file.path.clone());
    let found = dir
        .as_deref()
        .map(engine::discover_testbenches)
        .unwrap_or_default();
    s.sim.tbs = found.into_iter().map(TbItem::load).collect();
    s.sim.selected = prev.and_then(|p| s.sim.tbs.iter().position(|t| t.file.path == p));
    let history = dir.as_deref().map(engine::load_history).unwrap_or_default();
    s.sim.history = history.runs;
    s.sim.manual = history.manual;
    s.sim.scanned = dir.clone();
    s.sim.has_scanned = true;
    match dir {
        Some(d) => format!(
            "tb_rescan {} testbench(es) in {}",
            s.sim.tbs.len(),
            d.display()
        ),
        None => "tb_rescan: no source directory (open a project or source)".into(),
    }
}

/// Rescan when the discovery directory changed since the last scan.
pub fn ensure_scanned(s: &mut AppState) {
    let dir = discovery_dir(s);
    if !s.sim.has_scanned || s.sim.scanned != dir {
        rescan(s);
    }
}

/// Testbench a default run uses: the selection, else the first one.
fn pick_tb(s: &AppState) -> Option<usize> {
    if s.sim.tbs.is_empty() {
        return None;
    }
    Some(s.sim.selected.filter(|&i| i < s.sim.tbs.len()).unwrap_or(0))
}

/// Whether the design can free-run: an elaborated design or RTL to stage.
fn design_available(s: &AppState) -> bool {
    s.tcl.design.is_some()
        || s.runs.source.is_some()
        || !s.editor.lines.iter().all(|l| l.trim().is_empty())
}

/// Why a default run cannot start (`Ok` when it can).
pub fn can_launch(s: &AppState) -> Result<(), String> {
    if s.sim.is_active() {
        return Err(format!("{} is already running", s.sim.active_label()));
    }
    if pick_tb(s).is_some() || design_available(s) {
        Ok(())
    } else {
        Err("Open an RTL source or a testbench first".into())
    }
}

fn console_error(s: &mut AppState, title: &str, e: &str) {
    s.console.lines.push(ConsoleLine {
        text: format!("ERROR: {e}"),
        sev: MsgSev::Error,
    });
    s.toasts
        .push(ToastKind::Error, title, &truncate_chars(e, 140));
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Default run: the selected (or first) testbench, else the free-running
/// design. Fails without touching run state when nothing can run.
pub fn launch_default_tb(s: &mut AppState) -> Result<(), String> {
    ensure_scanned(s);
    match pick_tb(s) {
        Some(i) => launch_testbench(s, i),
        None => launch_design(s),
    }
}

/// UI/Tcl `launch_simulation`: echo the verb and start the default run;
/// failures land in the console and a toast.
pub fn launch(s: &mut AppState) {
    s.console.lines.push(ConsoleLine {
        text: "launch_simulation".into(),
        sev: MsgSev::Info,
    });
    if let Err(e) = launch_default_tb(s) {
        console_error(s, "Cannot start simulation", &e);
    }
}

/// Rerun the last simulation (testbench or design) from time zero.
pub fn rerun_last(s: &mut AppState) -> Result<(), String> {
    match s.sim.last_run {
        Some(RunTarget::Testbench(i)) if i < s.sim.tbs.len() => launch_testbench(s, i),
        Some(RunTarget::Design) => launch_design(s),
        _ => Err("restart: no simulation has run yet".into()),
    }
}

/// Spawn a worker job in the single simulation slot. The closure gets the
/// event sender and the cancel flag; it must end with a terminal event.
pub(crate) fn spawn_job(
    s: &mut AppState,
    target: RunTarget,
    label: String,
    stream: Option<mpsc::Receiver<TbStreamEvent>>,
    job: impl FnOnce(mpsc::Sender<SimEvent>, Arc<AtomicBool>) + Send + 'static,
) -> Result<(), String> {
    if s.sim.is_active() {
        return Err(format!("{} is already running", s.sim.active_label()));
    }
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    std::thread::Builder::new()
        .name(format!("helion-sim-{label}"))
        .spawn(move || job(tx, worker_cancel))
        .map_err(|e| format!("failed to spawn sim thread: {e}"))?;
    s.sim.active = Some(ActiveSim {
        target,
        label,
        rx: Arc::new(Mutex::new(rx)),
        stream: stream.map(|r| Arc::new(Mutex::new(r))),
        cancel,
    });
    s.sim.progress_ns = 0;
    s.sim.run_log_start = s.sim.log.len();
    Ok(())
}

/// Run testbench `i` on a worker: `run_testbench` streams `$display` and
/// failures live; the outcome lands through [`poll_sim`].
pub fn launch_testbench(s: &mut AppState, i: usize) -> Result<(), String> {
    let item = s
        .sim
        .tbs
        .get(i)
        .ok_or_else(|| format!("tb_run: no testbench index {i}"))?;
    let name = item.name().to_string();
    let path = item.file.path.clone();
    let vcd = s.sim.vcd;
    let plusargs = crate::views::tb_uvm::plusargs(s, &path);
    let (stx, srx) = mpsc::channel();
    spawn_job(
        s,
        RunTarget::Testbench(i),
        name.clone(),
        Some(srx),
        move |tx, cancel| {
            let cfg = engine::TbRunConfig {
                tb_path: path,
                vcd,
                max_units: engine::TB_DEFAULT_MAX_UNITS,
                seed: 1,
                cancel: Some(cancel),
                plusargs,
            };
            let r = engine::run_testbench(&cfg, Some(&stx));
            drop(stx);
            let _ = tx.send(SimEvent::TbDone(r));
        },
    )?;
    s.sim.tbs[i].status = TbStatus::Running;
    s.sim.selected.get_or_insert(i);
    s.sim.last_run = Some(RunTarget::Testbench(i));
    s.flow.sim = RunStatus::Running;
    let uvm = crate::views::tb_uvm::plusargs(s, &s.sim.tbs[i].file.path).join(" ");
    let line = format!(
        "tb_run {name}{}{}",
        if uvm.is_empty() { String::new() } else { format!(" {uvm}") },
        if s.sim.vcd { " (VCD dump on)" } else { "" }
    );
    s.sim.push_log(line, MsgSev::Info);
    Ok(())
}

/// Queue every discovered testbench and start the first.
pub fn run_all(s: &mut AppState) -> Result<String, String> {
    ensure_scanned(s);
    if s.sim.tbs.is_empty() {
        return Err("tb_run_all: no testbenches discovered".into());
    }
    if s.sim.is_active() {
        return Err(format!("{} is already running", s.sim.active_label()));
    }
    let n = s.sim.tbs.len();
    s.sim.queue = (1..n).collect();
    launch_testbench(s, 0)?;
    Ok(format!("tb_run_all {n} testbench(es)"))
}

/// Stop the running job (the worker stops at its next check); clears the
/// Run-all queue.
pub fn cancel_sim(s: &mut AppState) {
    if !s.sim.is_active() {
        s.toasts
            .push(ToastKind::Info, "No active simulation", "Nothing to stop");
        return;
    }
    s.sim.queue.clear();
    s.sim.request_cancel();
    let line = format!("stop_simulation {}", s.sim.active_label());
    s.console.lines.push(ConsoleLine {
        text: line.clone(),
        sev: MsgSev::Info,
    });
    s.sim.push_log(line, MsgSev::Info);
}

/// Inputs for a free run, gathered on the UI thread.
struct DesignJob {
    origin: String,
    src: String,
    tcl_design: Option<helion_ir::Design>,
    top: Option<String>,
    params: std::collections::HashMap<String, u128>,
    cycles: u32,
    period_ps: u64,
    log_all: bool,
}

/// Free-run the design (no testbench) for RUNTIME_CYCLES at TIMESCALE_PS
/// on the `helion-sim` kernel and import the VCD into the Wave pane.
pub fn launch_design(s: &mut AppState) -> Result<(), String> {
    let params = crate::views::sim_settings::params(s)?;
    let tcl_design = s.tcl.design.clone();
    let (origin, src) = if tcl_design.is_some() {
        (
            s.tcl
                .design_name
                .clone()
                .unwrap_or_else(|| "tcl_design".into()),
            String::new(),
        )
    } else {
        let path = crate::runs::resolve_source(s)
            .map_err(|e| e.replace("launch_runs", "launch_simulation"))?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("launch_simulation: cannot read {}: {e}", path.display()))?;
        (path.to_string_lossy().into_owned(), text)
    };
    let job = DesignJob {
        origin,
        src,
        tcl_design,
        top: s.sim_settings.sim_top.clone(),
        params,
        cycles: s.debug.runtime_cycles.clamp(1, LIVE_MAX_CYCLES),
        period_ps: s.debug.timescale_ps.max(1),
        log_all: s.sim_settings.log_all_signals,
    };
    spawn_job(
        s,
        RunTarget::Design,
        "the design".into(),
        None,
        move |tx, cancel| design_worker(job, tx, cancel),
    )?;
    s.sim.last_run = Some(RunTarget::Design);
    s.flow.sim = RunStatus::Running;
    Ok(())
}

/// Port names (plus the synthetic `clk`/`led`) kept when LOG_ALL_SIGNALS
/// is off.
fn port_filter(design: &helion_ir::Design) -> Vec<String> {
    let mut keep = vec!["clk".to_string(), "led".to_string()];
    keep.extend(
        design
            .ports
            .iter()
            .map(|p| helion_ir::resolve(p.name).to_string()),
    );
    keep
}

fn design_worker(job: DesignJob, tx: mpsc::Sender<SimEvent>, cancel: Arc<AtomicBool>) {
    let send = |text: String, sev: MsgSev| {
        let _ = tx.send(SimEvent::Line { text, sev });
    };
    let cancelled = || cancel.load(Ordering::Relaxed);
    send(format!("elaborate {}", job.origin), MsgSev::Info);
    let design = match job.tcl_design {
        Some(d) => d,
        None => {
            let ext = Path::new(&job.origin)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let r = if matches!(ext.as_str(), "vhd" | "vhdl") {
                helion_vhdl::synth_vhdl(&job.src).map_err(|e| format!("elaborate failed: {e}"))
            } else {
                helion_sv::elaborate_sv(
                    &job.src,
                    &job.origin,
                    job.top.as_deref(),
                    &job.params,
                    &helion_sv::SvCompileOpts::default(),
                )
                .map(|(d, _)| d)
                .map_err(|e| format!("elaborate failed: {e}"))
            };
            match r {
                Ok(d) => d,
                Err(e) => {
                    let _ = tx.send(SimEvent::Wave(Err(e)));
                    return;
                }
            }
        }
    };
    send(
        format!(
            "elaborated {} cells={} luts={}",
            helion_ir::resolve(design.name),
            design.cells.len(),
            design.lut_inits().len()
        ),
        MsgSev::Info,
    );
    if cancelled() {
        let _ = tx.send(SimEvent::Cancelled);
        return;
    }
    send(
        format!(
            "sim_run start cycles={} timescale_ps={}",
            job.cycles, job.period_ps
        ),
        MsgSev::Info,
    );
    let vcd = helion_sim::vcd::dump_vcd(&design, job.cycles, job.period_ps);
    if cancelled() {
        let _ = tx.send(SimEvent::Cancelled);
        return;
    }
    let mut live = match import_vcd_text(&vcd) {
        Ok(l) => l,
        Err(e) => {
            let _ = tx.send(SimEvent::Wave(Err(format!("VCD import failed: {e}"))));
            return;
        }
    };
    if !job.log_all {
        let keep = port_filter(&design);
        live.traces.retain(|t| keep.iter().any(|k| k == t.leaf()));
    }
    let top = helion_ir::resolve(design.name).to_string();
    live.source = format!("{top} ({} cycles, helion-sim)", job.cycles);
    live.cycles = job.cycles;
    send(
        format!(
            "dump_vcd signals={} samples={} timescale_ps={} duration_ps={}{}{}",
            live.traces.len(),
            live.sample_len(),
            live.timescale_ps,
            live.duration_ps,
            if live.truncated { " (truncated)" } else { "" },
            if job.log_all { "" } else { " (ports only)" },
        ),
        MsgSev::Info,
    );
    let ns = u64::from(job.cycles).saturating_mul(job.period_ps) / 1_000;
    let summary = format!("{top} simulated {} cycles ({ns} ns)", job.cycles);
    send(summary.clone(), MsgSev::Info);
    let _ = tx.send(SimEvent::Wave(Ok(SimArtifacts { live, summary })));
}

/// Import a VCD file into the Wave pane (testbench dumps, history rows).
pub fn load_vcd_into_wave(s: &mut AppState, path: &Path, source: &str) -> Result<usize, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read VCD {}: {e}", path.display()))?;
    let mut live = import_vcd_text(&text)
        .map_err(|e| format!("VCD import failed ({}): {e}", path.display()))?;
    live.source = source.to_string();
    let n = live.traces.len();
    s.vcd.timescale_ps = live.timescale_ps.max(1);
    s.vcd.duration_ps = live.duration_ps;
    s.vcd.signals = live
        .traces
        .iter()
        .take(64)
        .map(|t| t.name.clone())
        .collect();
    s.wave.mount_live(live);
    Ok(n)
}

/// Open a VCD in the waveform viewer (detail button, history rows, Tcl).
pub fn open_vcd(s: &mut AppState, path: &Path, source: &str) -> Result<String, String> {
    let n = load_vcd_into_wave(s, path, source)?;
    s.navigate(CenterPane::Wave);
    Ok(format!(
        "open_wave_database {} ({n} signals)",
        path.display()
    ))
}

/// Open the failing line of testbench `tb`'s failure `k` in the editor.
pub fn jump_to_failure(s: &mut AppState, tb: usize, k: usize) -> Result<String, String> {
    let (file, line) = match s.sim.tbs.get(tb).and_then(|t| t.last.as_ref()) {
        Some(Ok(out)) => {
            let f = out
                .failures
                .get(k)
                .ok_or_else(|| format!("tb_jump: no failure {k}"))?;
            (f.file.clone(), f.line)
        }
        Some(Err(e)) if k == 0 => (e.file.clone(), e.line),
        _ => return Err(format!("tb_jump: no failure {k}")),
    };
    open_in_editor(s, Path::new(&file), line)
}

/// Load `path` into an editor tab and park the cursor on 1-based `line`.
pub fn open_in_editor(s: &mut AppState, path: &Path, line: usize) -> Result<String, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    crate::views::editor::open_file_tab(s, name.clone(), text, path.to_path_buf());
    crate::views::editor::move_cursor_to(s, line.saturating_sub(1));
    s.navigate(CenterPane::Editor);
    Ok(format!("open {name}:{line}"))
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Record one finished run in the persisted history (best effort: a write
/// failure is logged, never fatal).
fn record(s: &mut AppState, entry: TbHistoryEntry) {
    let Some(dir) = s.sim.scanned.clone() else {
        s.sim.history.push(entry);
        return;
    };
    match engine::record_history(&dir, entry.clone(), &s.sim.manual) {
        Ok(h) => s.sim.history = h.runs,
        Err(e) => {
            s.sim.history.push(entry);
            s.sim
                .push_log(format!("WARNING: run history not saved: {e}"), MsgSev::Warn);
        }
    }
}

/// Apply a finished testbench run: status, log, toast, VCD mount, history.
fn finish_tb(s: &mut AppState, i: usize, r: Result<TbOutcome, TbError>) {
    let Some(item) = s.sim.tbs.get(i) else { return };
    let name = item.name().to_string();
    let path = item.file.path.clone();
    match &r {
        Ok(out) => {
            let status = if out.stopped {
                TbStatus::Stopped
            } else if out.passed {
                TbStatus::Passed
            } else {
                TbStatus::Failed
            };
            s.sim.tbs[i].status = status;
            s.flow.sim = match status {
                TbStatus::Passed => RunStatus::Complete,
                TbStatus::Stopped => RunStatus::NotStarted,
                _ => RunStatus::Failed,
            };
            s.console.lines.push(ConsoleLine {
                text: out.summary.clone(),
                sev: match status {
                    TbStatus::Passed => MsgSev::Info,
                    TbStatus::Stopped => MsgSev::Warn,
                    _ => MsgSev::Error,
                },
            });
            if let Some(vcd) = &out.vcd_path {
                match load_vcd_into_wave(s, vcd, &format!("{name} (testbench VCD)")) {
                    Ok(_) => {
                        let tb_path = out.coverage.tb_file.clone();
                        crate::views::wave_cfg::auto_restore(s, &tb_path);
                    }
                    Err(e) => s.sim.push_log(format!("WARNING: {e}"), MsgSev::Warn),
                }
            }
            match status {
                TbStatus::Passed => {
                    s.toasts
                        .push(ToastKind::Success, "Testbench passed", &out.summary)
                }
                TbStatus::Stopped => {
                    s.toasts
                        .push(ToastKind::Warning, "Testbench stopped", &out.summary)
                }
                _ => s.toasts.push(
                    ToastKind::Error,
                    "Testbench failed",
                    &format!("{name}: {} error(s)", out.failures.len()),
                ),
            }
            if !out.stopped {
                record(
                    s,
                    TbHistoryEntry {
                        tb_name: name,
                        tb_path: path,
                        started_unix: now_unix().saturating_sub(out.runtime_ms / 1000),
                        passed: out.passed,
                        sim_time_ns: out.sim_time_ns,
                        runtime_ms: out.runtime_ms,
                        errors: out.failures.len() as u32,
                        vcd_path: out.vcd_path.clone(),
                    },
                );
            }
        }
        Err(e) => {
            s.sim.tbs[i].status = TbStatus::Failed;
            s.flow.sim = RunStatus::Failed;
            let line = format!("ERROR: {e}");
            s.console.lines.push(ConsoleLine {
                text: line.clone(),
                sev: MsgSev::Error,
            });
            s.sim.push_log(line, MsgSev::Error);
            s.toasts.push(
                ToastKind::Error,
                "Testbench error",
                &truncate_chars(&e.to_string(), 140),
            );
            record(
                s,
                TbHistoryEntry {
                    tb_name: name,
                    tb_path: path,
                    started_unix: now_unix(),
                    passed: false,
                    sim_time_ns: 0,
                    runtime_ms: 0,
                    errors: 1,
                    vcd_path: None,
                },
            );
        }
    }
    s.sim.tbs[i].last = Some(r);
}

fn finish_design(s: &mut AppState, r: Result<SimArtifacts, String>) {
    match r {
        Ok(art) => {
            let n = art.live.traces.len();
            let samples = art.live.sample_len();
            s.vcd.timescale_ps = art.live.timescale_ps.max(1);
            s.vcd.duration_ps = art.live.duration_ps;
            s.vcd.signals = art
                .live
                .traces
                .iter()
                .take(64)
                .map(|t| t.name.clone())
                .collect();
            s.wave.mount_live(art.live);
            s.flow.sim = RunStatus::Complete;
            let detail = format!("{} · {n} signals, {samples} samples", art.summary);
            s.toasts
                .push(ToastKind::Success, "Simulation complete", &detail);
            let done = format!("sim_run done: {detail}");
            s.console.lines.push(ConsoleLine {
                text: done.clone(),
                sev: MsgSev::Info,
            });
            s.sim.push_log(done, MsgSev::Info);
        }
        Err(e) => {
            s.flow.sim = RunStatus::Failed;
            let line = format!("ERROR: {e}");
            s.console.lines.push(ConsoleLine {
                text: line.clone(),
                sev: MsgSev::Error,
            });
            s.sim.push_log(line, MsgSev::Error);
            s.toasts.push(
                ToastKind::Error,
                "Simulation failed",
                &truncate_chars(&e, 140),
            );
        }
    }
}

/// Apply one worker event. Returns false once the job has finished.
fn deliver(s: &mut AppState, target: RunTarget, event: SimEvent) {
    match event {
        SimEvent::Line { text, sev } => {
            s.console.lines.push(ConsoleLine {
                text: text.clone(),
                sev,
            });
            s.sim.push_log(text, sev);
        }
        SimEvent::TbDone(r) => {
            // Lines the engine streamed before returning are already queued
            // on the stream channel: drain them first so the log stays in
            // order.
            drain_stream(s);
            s.sim.active = None;
            if let RunTarget::Testbench(i) = target {
                finish_tb(s, i, r);
            }
            if !s.sim.queue.is_empty() {
                let next = s.sim.queue.remove(0);
                if let Err(e) = launch_testbench(s, next) {
                    s.sim.queue.clear();
                    console_error(s, "Run all stopped", &e);
                }
            }
        }
        SimEvent::Wave(r) => {
            s.sim.active = None;
            finish_design(s, r);
        }
        SimEvent::Compiled(r) => {
            s.sim.active = None;
            crate::views::sim_settings::finish_compile(s, r);
        }
        SimEvent::Elaborated(r) => {
            s.sim.active = None;
            crate::views::sim_settings::finish_elaborate(s, r);
        }
        SimEvent::Cancelled => {
            s.sim.active = None;
            if let RunTarget::Testbench(i) = target {
                if let Some(t) = s.sim.tbs.get_mut(i) {
                    t.status = TbStatus::Stopped;
                }
            }
            if matches!(target, RunTarget::Testbench(_) | RunTarget::Design) {
                s.flow.sim = RunStatus::NotStarted;
            }
            let line = "simulation stopped by user".to_string();
            s.sim.push_log(line.clone(), MsgSev::Warn);
            s.console.lines.push(ConsoleLine {
                text: line,
                sev: MsgSev::Warn,
            });
            s.toasts
                .push(ToastKind::Warning, "Simulation stopped", "Run cancelled");
        }
    }
}

/// Drain the live testbench stream into the sim log.
fn drain_stream(s: &mut AppState) -> bool {
    let Some(stream) = s.sim.active.as_ref().and_then(|a| a.stream.clone()) else {
        return false;
    };
    let mut changed = false;
    for _ in 0..MAX_SIM_DRAIN {
        let next = stream.lock().unwrap_or_else(|e| e.into_inner()).try_recv();
        let Ok(ev) = next else { break };
        changed = true;
        match ev {
            TbStreamEvent::Log { time_ns, sev, text } => {
                let sev = match sev {
                    engine::TbSev::Info => MsgSev::Info,
                    engine::TbSev::Warning => MsgSev::Warn,
                    engine::TbSev::Error => MsgSev::Error,
                };
                s.sim.progress_ns = s.sim.progress_ns.max(time_ns);
                s.sim.push_log(format!("{time_ns:>7} ns  {text}"), sev);
            }
            TbStreamEvent::Failure {
                file,
                line,
                message,
                time_ns,
                ..
            } => {
                let short = Path::new(&file)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or(file);
                s.sim.push_log(
                    format!("{time_ns:>7} ns  FAIL {short}:{line}: {message}"),
                    MsgSev::Error,
                );
            }
            TbStreamEvent::Progress { time_ns } => {
                s.sim.progress_ns = s.sim.progress_ns.max(time_ns);
            }
        }
    }
    changed
}

/// Drain pending worker events into console/log/wave state. Returns true
/// when anything changed (drives `AppSpec::poll`'s dirty flag).
pub fn poll_sim(s: &mut AppState) -> bool {
    let Some(active) = s.sim.active.clone() else {
        return false;
    };
    let mut changed = drain_stream(s);
    for _ in 0..MAX_SIM_DRAIN {
        let next = active
            .rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_recv();
        match next {
            Ok(event) => {
                deliver(s, active.target, event);
                changed = true;
                // A terminal event ends this job (Run all may already have
                // started the next one on a fresh channel).
                let same_job = s
                    .sim
                    .active
                    .as_ref()
                    .is_some_and(|a| Arc::ptr_eq(&a.rx, &active.rx));
                if !same_job {
                    break;
                }
            }
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                s.sim.active = None;
                finish_design(s, Err("sim worker exited unexpectedly".into()));
                changed = true;
                break;
            }
        }
    }
    changed
}

/// `AppSpec::poll` wiring: follow the discovery directory, pump the worker.
pub fn poll(s: &mut AppState, _t: f64, _tx: &mut Tx) -> bool {
    let dir = discovery_dir(s);
    let rescanned = if !s.sim.has_scanned || s.sim.scanned != dir {
        rescan(s);
        true
    } else {
        false
    };
    poll_sim(s) || rescanned
}

/// Run testbench `i` synchronously and apply the result exactly like a
/// worker run (scenes/tests; the UI uses [`launch_testbench`]).
pub fn run_testbench_now(s: &mut AppState, i: usize) -> Result<(), String> {
    let item = s
        .sim
        .tbs
        .get(i)
        .ok_or_else(|| format!("tb_run: no testbench index {i}"))?;
    let cfg = engine::TbRunConfig {
        tb_path: item.file.path.clone(),
        vcd: s.sim.vcd,
        max_units: engine::TB_DEFAULT_MAX_UNITS,
        seed: 1,
        cancel: None,
        plusargs: crate::views::tb_uvm::plusargs(s, &item.file.path),
    };
    let (stx, srx) = mpsc::channel();
    let r = engine::run_testbench(&cfg, Some(&stx));
    drop(stx);
    s.sim.run_log_start = s.sim.log.len();
    s.sim.active = Some(ActiveSim {
        target: RunTarget::Testbench(i),
        label: s.sim.tbs[i].name().to_string(),
        rx: Arc::new(Mutex::new(mpsc::channel().1)),
        stream: Some(Arc::new(Mutex::new(srx))),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    s.sim.last_run = Some(RunTarget::Testbench(i));
    deliver(s, RunTarget::Testbench(i), SimEvent::TbDone(r));
    Ok(())
}

/// Resolve a Tcl testbench selector: an index or a testbench name.
fn resolve_tb(s: &AppState, sel: &str) -> Result<usize, String> {
    let sel = sel.trim();
    if sel.is_empty() {
        return pick_tb(s).ok_or_else(|| "no testbenches discovered".to_string());
    }
    if let Ok(i) = sel.parse::<usize>() {
        if i < s.sim.tbs.len() {
            return Ok(i);
        }
    }
    s.sim
        .tbs
        .iter()
        .position(|t| t.name() == sel || t.file.path.to_string_lossy() == sel)
        .ok_or_else(|| format!("no testbench {sel}"))
}

/// Tcl testbench verbs (`tb_rescan`, `tb_run`, `tb_run_all`, `tb_stop`,
/// `tb_vcd`, `tb_wave`, `tb_jump`, `tb_select`). Every verb that acts on the
/// runner opens the Sim pane.
pub fn exec_tb(s: &mut AppState, verb: &str, args: &[String]) -> Result<String, String> {
    let arg = args.join(" ");
    match verb {
        "tb_rescan" => {
            s.navigate(CenterPane::Sim);
            Ok(rescan(s))
        }
        "tb_select" => {
            ensure_scanned(s);
            let i = resolve_tb(s, &arg).map_err(|e| format!("tb_select: {e}"))?;
            s.sim.selected = Some(i);
            s.navigate(CenterPane::Sim);
            Ok(format!("tb_select {}", s.sim.tbs[i].name()))
        }
        "tb_run" => {
            ensure_scanned(s);
            let i = resolve_tb(s, &arg).map_err(|e| format!("tb_run: {e}"))?;
            s.sim.selected = Some(i);
            s.navigate(CenterPane::Sim);
            launch_testbench(s, i)?;
            Ok(format!("tb_run {} started", s.sim.tbs[i].name()))
        }
        "tb_run_all" => {
            s.navigate(CenterPane::Sim);
            run_all(s)
        }
        "tb_stop" => {
            if !s.sim.is_active() {
                return Err("tb_stop: no simulation running".into());
            }
            cancel_sim(s);
            Ok("tb_stop".into())
        }
        "tb_vcd" => {
            match arg.trim() {
                "" => {}
                "on" | "1" | "true" | "yes" => s.sim.vcd = true,
                "off" | "0" | "false" | "no" => s.sim.vcd = false,
                other => return Err(format!("tb_vcd: expected on|off, got {other}")),
            }
            Ok(format!("tb_vcd {}", if s.sim.vcd { "on" } else { "off" }))
        }
        "tb_wave" => {
            let path = if !arg.trim().is_empty() {
                PathBuf::from(arg.trim())
            } else {
                last_vcd(s).ok_or_else(|| {
                    "tb_wave: no VCD from the last run (turn on VCD dump and run first)".to_string()
                })?
            };
            open_vcd(s, &path, "testbench VCD")
        }
        "tb_uvm_test" => crate::views::tb_uvm::exec_uvm_test(s, arg.trim()),
        "tb_jump" => {
            let k: usize = arg
                .trim()
                .parse()
                .map_err(|_| "tb_jump: need a failure index".to_string())?;
            let tb = s
                .sim
                .selected
                .ok_or_else(|| "tb_jump: select a testbench first".to_string())?;
            jump_to_failure(s, tb, k)
        }
        other => Err(format!("{other}: unknown testbench verb")),
    }
}

/// VCD of the selected testbench's last run, else the newest history VCD.
pub fn last_vcd(s: &AppState) -> Option<PathBuf> {
    let from_selected = s
        .sim
        .selected
        .and_then(|i| s.sim.tbs.get(i))
        .and_then(|t| t.last.as_ref())
        .and_then(|r| r.as_ref().ok())
        .and_then(|o| o.vcd_path.clone());
    from_selected.or_else(|| {
        s.sim
            .history
            .iter()
            .rev()
            .find_map(|h| h.vcd_path.clone().filter(|p| p.is_file()))
    })
}

/// Scene fixture: run `examples/counter_tb.sv` from a temp copy so the
/// Coverage view shows a real run's coverage (never writes into the repo).
pub fn seed_coverage_fixture(s: &mut AppState) {
    let ex = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let dir = std::env::temp_dir().join("helion-rune-scenes").join("coverage");
    let _ = std::fs::create_dir_all(&dir);
    for f in ["counter.sv", "counter_tb.sv"] {
        let _ = std::fs::copy(ex.join(f), dir.join(f));
    }
    s.sim.dir_override = Some(dir.clone());
    rescan(s);
    let Some(i) = s.sim.tbs.iter().position(|t| t.file.name == "counter_tb") else { return };
    let cfg = helion_verif::tb::TbRunConfig::new(s.sim.tbs[i].file.path.clone());
    s.sim.tbs[i].last = Some(helion_verif::tb::run_testbench(&cfg, None));
    s.sim.selected = Some(i);
}

/// Scenes/tests: discover the repo `examples/` testbenches (read-only).
pub fn seed_examples_fixture(s: &mut AppState) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    s.sim.dir_override = Some(std::fs::canonicalize(&dir).unwrap_or(dir));
    rescan(s);
}

/// A failing counter testbench for the run-results fixture (the shipped
/// example passes; this one checks `led` one clock too early).
const FAILING_TB: &str = "\
// Fixture: expects led after 4 clocks (it rises after 8), so it fails.
module counter_early_tb;
    logic clk = 0;
    logic led;

    counter dut (.clk(clk), .led(led));

    always #5 clk = ~clk;

    initial begin
        $display(\"counter_early_tb: expecting led after 4 clocks\");
        repeat (4) @(posedge clk);
        #1;
        assert (led == 1) else $error(\"led still low after 4 clocks: led=%0d\", led);
        $finish;
    end
endmodule
";

/// Scenes/tests: copy the counter example plus a failing variant into a
/// fresh temp directory, discover there, and run both for real (runs write
/// `.helion/` beside the testbenches, never into the repo).
pub fn seed_run_fixture(s: &mut AppState) -> Result<PathBuf, String> {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let ex = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let dir = std::env::temp_dir().join(format!(
        "helion-rune-tb-fixture-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for f in ["counter.sv", "counter_tb.sv"] {
        std::fs::copy(ex.join(f), dir.join(f)).map_err(|e| format!("{f}: {e}"))?;
    }
    std::fs::write(dir.join("counter_early_tb.sv"), FAILING_TB).map_err(|e| e.to_string())?;
    s.sim.dir_override = Some(dir.clone());
    rescan(s);
    for i in 0..s.sim.tbs.len() {
        run_testbench_now(s, i)?;
    }
    Ok(dir)
}

/// Scene/test fixture: the VHDL `examples/counter_tb.vhd` (SV counter DUT
/// as a netlist) plus a copy that expects `led` one bit early, both run in
/// a temp dir; the failing one is selected.
pub fn seed_vhdl_fixture(s: &mut AppState) -> Result<PathBuf, String> {
    let ex = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let dir = std::env::temp_dir().join(format!("helion-rune-vhdl-fixture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for f in ["counter.sv", "counter_tb.vhd"] {
        std::fs::copy(ex.join(f), dir.join(f)).map_err(|e| format!("{f}: {e}"))?;
    }
    let tb = std::fs::read_to_string(ex.join("counter_tb.vhd")).map_err(|e| e.to_string())?;
    let bad = tb.replace("entity counter_tb is", "entity counter_early_tb is").replace("of counter_tb is", "of counter_early_tb is").replace("expected := model(3);", "expected := model(2);");
    std::fs::write(dir.join("counter_early_tb.vhd"), bad).map_err(|e| e.to_string())?;
    s.sim.dir_override = Some(dir.clone());
    rescan(s);
    for i in 0..s.sim.tbs.len() {
        run_testbench_now(s, i)?;
    }
    s.sim.selected = s.sim.tbs.iter().position(|t| t.file.name == "counter_early_tb");
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init;

    #[test]
    fn vhdl_testbenches_run_from_the_pane_with_failures_and_wave() {
        let mut s = init();
        let dir = seed_vhdl_fixture(&mut s).expect("fixture");
        assert_eq!(s.sim.tbs.len(), 2);
        let ok = s.sim.tbs.iter().find(|t| t.file.name == "counter_tb").expect("counter_tb");
        let info = ok.info.as_ref().expect("VHDL summary parse");
        assert_eq!(info.dut_module, "counter");
        assert_eq!(info.clocks, vec![("clk".to_string(), 5)]);
        assert!(matches!(&ok.last, Some(Ok(o)) if o.passed && o.vcd_path.is_some()));
        let i = s.sim.selected.expect("failing tb selected");
        let Some(Ok(out)) = &s.sim.tbs[i].last else { panic!("no outcome") };
        assert!(!out.passed);
        let f = &out.failures[0];
        assert!(f.file.ends_with("counter_early_tb.vhd"));
        // The failure jumps the editor to the assert line.
        jump_to_failure(&mut s, i, 0).expect("jump");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn pump(s: &mut AppState) {
        let start = std::time::Instant::now();
        while s.sim.is_active() {
            poll_sim(s);
            assert!(
                start.elapsed() < std::time::Duration::from_secs(120),
                "sim job timed out"
            );
            if s.sim.is_active() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }

    fn fixture_copy(s: &mut AppState) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let ex = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let dir = std::env::temp_dir().join(format!(
            "helion-rune-tb-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["counter.sv", "counter_tb.sv"] {
            std::fs::copy(ex.join(f), dir.join(f)).unwrap();
        }
        std::fs::write(dir.join("counter_early_tb.sv"), FAILING_TB).unwrap();
        s.sim.dir_override = Some(dir.clone());
        rescan(s);
        dir
    }

    #[test]
    fn discovery_follows_the_source_directory_and_parses_each_file() {
        let mut s = init();
        s.runs.source =
            Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv"));
        ensure_scanned(&mut s);
        let names: Vec<&str> = s.sim.tbs.iter().map(|t| t.name()).collect();
        // SV and VHDL versions of the counter testbench; same stem, so the
        // list shows file names.
        assert_eq!(names, vec!["counter_tb.sv", "counter_tb.vhd"]);
        let vhdl = s.sim.tbs[1].info.as_ref().expect("counter_tb.vhd parses");
        assert_eq!(vhdl.dut_module, "counter");
        assert_eq!(vhdl.clocks, vec![("clk".to_string(), 5)]);
        let info = s.sim.tbs[0].info.as_ref().expect("counter_tb parses");
        assert_eq!(info.dut_module, "counter");
        assert_eq!(info.dut_instance, "dut");
        assert_eq!(info.clocks, vec![("clk".to_string(), 5)]);
        assert!(info.ports.contains(&("led".to_string(), "led".to_string())));
    }

    #[test]
    fn demo_project_without_a_real_directory_discovers_nothing() {
        let mut s = init();
        let line = rescan(&mut s);
        assert!(s.sim.tbs.is_empty(), "no fake testbenches: {line}");
        assert!(s.sim.has_scanned);
    }

    #[test]
    fn worker_run_streams_display_and_records_history() {
        let mut s = init();
        let dir = fixture_copy(&mut s);
        let i = s
            .sim
            .tbs
            .iter()
            .position(|t| t.name() == "counter_tb")
            .unwrap();
        launch_testbench(&mut s, i).expect("launch");
        assert_eq!(s.sim.status(i), TbStatus::Running);
        assert_eq!(s.flow.sim, RunStatus::Running);
        pump(&mut s);
        assert_eq!(s.sim.status(i), TbStatus::Passed, "log: {:?}", s.sim.log);
        assert_eq!(s.flow.sim, RunStatus::Complete);
        assert!(
            s.sim.log.iter().any(|l| l.text.contains("led=1")),
            "$display must stream: {:?}",
            s.sim.log
        );
        assert_eq!(s.sim.history.len(), 1);
        assert!(s.sim.history[0].passed);
        assert!(engine::history_path(&dir).is_file(), "history persists");
        assert!(s.wave.has_live(), "the testbench VCD mounts in the wave");
        // A fresh scan reloads the persisted history.
        let mut fresh = init();
        fresh.sim.dir_override = Some(dir);
        rescan(&mut fresh);
        assert_eq!(fresh.sim.history.len(), 1);
    }

    #[test]
    fn failing_testbench_reports_line_and_jumps_the_editor() {
        let mut s = init();
        fixture_copy(&mut s);
        let i = s
            .sim
            .tbs
            .iter()
            .position(|t| t.name() == "counter_early_tb")
            .unwrap();
        run_testbench_now(&mut s, i).unwrap();
        assert_eq!(s.sim.status(i), TbStatus::Failed);
        let out = s.sim.tbs[i].last.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(out.failures.len(), 1);
        assert_eq!(out.failures[0].line, 14, "{:?}", out.failures);
        s.sim.selected = Some(i);
        exec_tb(&mut s, "tb_jump", &["0".into()]).expect("jump");
        assert_eq!(s.center, CenterPane::Editor);
        assert_eq!(s.editor.cursor_line, 13, "0-based line of the assert");
        assert_eq!(
            s.editor.tabs[s.editor.active], "counter_early_tb.sv",
            "the failing file opens"
        );
    }

    #[test]
    fn run_all_queues_every_testbench() {
        let mut s = init();
        fixture_copy(&mut s);
        run_all(&mut s).expect("run all");
        pump(&mut s);
        // The queue relaunches from deliver; keep pumping until drained.
        while s.sim.is_active() || !s.sim.queue.is_empty() {
            pump(&mut s);
        }
        assert!(s.sim.tbs.iter().all(|t| t.last.is_some()), "every tb ran");
        assert_eq!(s.sim.history.len(), 2);
    }

    #[test]
    fn free_run_uses_runtime_settings_without_testbenches() {
        let mut s = init();
        s.debug.runtime_cycles = 6;
        launch_default_tb(&mut s).expect("free run");
        assert_eq!(s.sim.active_target(), Some(RunTarget::Design));
        pump(&mut s);
        let live = s.wave.live.as_ref().expect("free run mounts a wave");
        assert_eq!(live.cycles, 6, "RUNTIME_CYCLES drives the run");
        assert_eq!(s.flow.sim, RunStatus::Complete);
        assert_eq!(s.sim.last_run(), Some(RunTarget::Design));
    }

    #[test]
    fn stop_cancels_and_marks_the_testbench_stopped() {
        let mut s = init();
        fixture_copy(&mut s);
        launch_testbench(&mut s, 0).unwrap();
        cancel_sim(&mut s);
        assert!(s.sim.cancel_requested());
        pump(&mut s);
        assert!(
            matches!(
                s.sim.status(0),
                TbStatus::Stopped | TbStatus::Passed | TbStatus::Failed
            ),
            "a stop lands as Stopped (or the run already finished)"
        );
        assert!(!s.sim.is_active());
    }

    #[test]
    fn tcl_verbs_rescan_toggle_vcd_and_reject_unknown_testbenches() {
        let mut s = init();
        fixture_copy(&mut s);
        let out = exec_tb(&mut s, "tb_rescan", &[]).unwrap();
        assert!(out.contains("2 testbench(es)"), "{out}");
        assert_eq!(
            exec_tb(&mut s, "tb_vcd", &["off".into()]).unwrap(),
            "tb_vcd off"
        );
        assert!(!s.sim.vcd);
        assert!(exec_tb(&mut s, "tb_run", &["nope_tb".into()]).is_err());
        assert!(exec_tb(&mut s, "tb_stop", &[]).is_err(), "nothing to stop");
    }
}
