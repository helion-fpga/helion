//! Helion IDE - three canvases, one activity rail, one Implement.
//!
//! `--version` / `--doctor` never open a window (so they work headless and on CI).
//! The GUI paints [`helion_gui::IdeModel`]; every button and the Tcl box call into
//! that model, which is what the unit tests already prove is not a no-op.

use eframe::egui::{self, Color32, RichText, Sense, Stroke};
use helion_gui::chrome::{self, Activity, Canvas, RAIL_OPEN_SOURCES};
use helion_gui::surface::{
    self, apply_activity, apply_canvas, apply_more, central_pane, queue_flow, spawn_job,
    ChromeState, EventClass, JobHandle, JobKind, UiEvent, UiTrace,
};
use helion_gui::{
    doctor, ila, open_hdl_dialog, open_vcd_dialog, pane_for_workspace, BottomTab, CdcSeverity,
    ClockRelation, ConstraintSection, DrcSeverity, FlowStep, IdeModel, IlaTrigger, LayoutKind,
    MethodologySeverity, MsgSeverity, PathGroupKind, PowerSeverity, ProbeSelection, QorAction,
    QorSeverity, RunCol, SimIlaStatus, SourceLine, StepState, TbFailure, TbLogLine, TbRunRequest,
    TbSev, TbStatus, TbStreamEvent, TimingPathSortCol, WaveRadix, WaveStyle, WorkspacePane,
    WorkspaceTab, best_run_index, sort_run_indices, sweep_wns_stats,
};
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

#[path = "helion-ide/commands.rs"]
mod commands;
#[path = "helion-ide/constraints_wizard_ui.rs"]
mod constraints_wizard_ui;
#[path = "helion-ide/dashboard.rs"]
mod dashboard;
#[path = "helion-ide/editor.rs"]
mod editor;
#[path = "helion-ide/flow_nav.rs"]
mod flow_nav;
#[path = "helion-ide/hierarchy.rs"]
mod hierarchy;
#[path = "helion-ide/highlight.rs"]
mod highlight;
#[path = "helion-ide/icons.rs"]
mod icons;
#[path = "helion-ide/layouts.rs"]
mod layouts;
#[path = "helion-ide/physview_panels.rs"]
mod physview_panels;
#[path = "helion-ide/schem_lod.rs"]
mod schem_lod;
#[path = "helion-ide/theme.rs"]
mod theme;
#[path = "helion-ide/toasts.rs"]
mod toasts;
#[path = "helion-ide/welcome.rs"]
mod welcome;

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--version") | Some("-V") | Some("version") => {
            println!("{}", doctor::version_line());
            return;
        }
        Some("--doctor") | Some("doctor") => {
            print!("{}", doctor::doctor_report());
            return;
        }
        Some("--help") | Some("-h") | Some("help") => {
            eprintln!(
                "helion-ide {}
  helion-ide                         open the IDE window (eframe)
  helion-ide --headless [file.sv]    synth + report_timing, print WNS (no window)
  helion-ide --stdin                 Tcl console + flow rail on stdin (no window)
  helion-ide --version
  helion-ide --doctor
  helion-ide --help

Windowed path is eframe when a display is available. This Linux host
cannot verify the Mac .app; use scripts/build-macos-app.sh on Apple Silicon.",
                env!("CARGO_PKG_VERSION")
            );
            return;
        }
        Some("--stdin") => {
            run_stdin();
            return;
        }
        Some("--headless") => {
            match args.next() {
                Some(path) => run_headless_oneshot(&path),
                None => run_stdin(),
            }
            return;
        }
        Some(other) => {
            eprintln!("unknown argument {other}; try helion-ide --help");
            std::process::exit(2);
        }
        None => {}
    }
    // `open Helion.app` gives /dev/null stdin (not a TTY). That is a GUI launch,
    // not a Tcl pipe. Only steal the window for a real pipe / Linux CI.
    if !io::stdin().is_terminal() && !cfg!(target_os = "macos") {
        run_stdin();
        return;
    }
    if let Err(e) = run_gui() {
        eprintln!("helion-ide: window failed ({e}); falling back to stdin Tcl console");
        eprintln!("hint: pass --stdin, or on macOS run scripts/build-macos-app.sh");
        run_stdin();
    }
}

fn parse_step(s: &str) -> Result<FlowStep, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "synth" | "synthesis" | "synth_design" => Ok(FlowStep::Synthesis),
        "opt" | "opt_design" => Ok(FlowStep::Opt),
        "place" | "place_design" => Ok(FlowStep::Place),
        "route" | "route_design" => Ok(FlowStep::Route),
        "bits" | "bitstream" | "write_bitstream" => Ok(FlowStep::Bitstream),
        other => Err(format!("unknown flow step {other}")),
    }
}

fn handle_line(ide: &mut IdeModel, line: &str) -> Result<String, String> {
    let t = line.trim();
    if t.is_empty() {
        return Ok(String::new());
    }
    if t == "quit" || t == "exit" {
        return Ok("__QUIT__".into());
    }
    if t == "help" {
        return Ok(
            "open <file> | flow synth|opt|place|route|bits | rail | tree | timing | util | <tcl>"
                .into(),
        );
    }
    if t == "rail" {
        let s = FlowStep::ALL
            .iter()
            .map(|st| format!("{}={:?}", st.label(), ide.step_state(*st)))
            .collect::<Vec<_>>()
            .join(" ");
        return Ok(s);
    }
    if t == "tree" {
        return Ok(ide.netlist_text());
    }
    if t == "timing" {
        return Ok(ide.timing_text());
    }
    if t == "util" {
        return Ok(ide.utilization_text());
    }
    if let Some(path) = t.strip_prefix("open ") {
        return ide.open_source(Path::new(path.trim()));
    }
    if let Some(step) = t.strip_prefix("flow ") {
        return ide.run_step(parse_step(step)?);
    }
    ide.exec(t)
}

fn run_headless_oneshot(path: &str) {
    let p = Path::new(path);
    let mut ide = IdeModel::new();
    if let Err(e) = ide.open_source(p) {
        eprintln!("synth {}: {e}", p.display());
        std::process::exit(1);
    }
    match ide.exec("report_timing") {
        Ok(out) => {
            println!("{out}");
            // Do not append a closed WNS after an unfinished timing line.
            let unfinished = out.contains("timing_incomplete")
                || out.contains("no_clock_path")
                || out.contains("no_body")
                || out.contains("no_logic")
                || out.contains("not a closed WNS");
            if !unfinished {
                if let Some(wns) = ide.wns_ps() {
                    if !out.contains("WNS_PS=") {
                        println!("WNS_PS={wns}");
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("report_timing: {e}");
            std::process::exit(1);
        }
    }
}

fn run_stdin() {
    let mut ide = IdeModel::new();
    println!("{}", doctor::version_line());
    println!("target {}", doctor::target_triple());
    println!("part {}", ide.part());
    println!("stdin Tcl console + flow rail. `help` for commands, `quit` to exit.");
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(s) => s,
            Err(e) => {
                eprintln!("stdin: {e}");
                break;
            }
        };
        match handle_line(&mut ide, &line) {
            Ok(out) if out == "__QUIT__" => break,
            Ok(out) => {
                if !out.is_empty() {
                    println!("{out}");
                }
            }
            Err(e) => println!("ERROR {e}"),
        }
        let _ = stdout.flush();
    }
}

fn run_gui() -> eframe::Result {
    let (iw, ih) = std::env::var("HELION_INNER_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((1440.0, 900.0));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([iw, ih])
            .with_min_inner_size([chrome::MIN_WINDOW_W, chrome::MIN_WINDOW_H])
            .with_title("Helion"),
        ..Default::default()
    };
    eframe::run_native(
        "Helion",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            theme::install_fonts(&cc.egui_ctx);
            cc.egui_ctx.style_mut(|s| {
                s.interaction.resize_grab_radius_side = chrome::SPLITTER_GRAB_PX;
                s.interaction.resize_grab_radius_corner = chrome::SPLITTER_GRAB_PX + 4.0;
            });
            Ok(Box::new(HelionIde::new()))
        }),
    )
}

/// Multi-step Create Project wizard (Vivado-shaped; no IP catalog / board DONE).
#[derive(Clone, Debug)]
struct CreateProjectWizard {
    /// 0 name/dir, 1 part, 2 sources, 3 constraints, 4 summary
    step: usize,
    name: String,
    directory: String,
    part: String,
    sources: Vec<String>,
    source_draft: String,
    constraints: Vec<String>,
    constraint_draft: String,
    error: Option<String>,
}

impl Default for CreateProjectWizard {
    fn default() -> Self {
        let dir = default_projects_dir();
        Self {
            step: 0,
            name: "project_1".into(),
            directory: dir.display().to_string(),
            part: "HL10T-C32-1".into(),
            sources: Vec::new(),
            source_draft: String::new(),
            constraints: Vec::new(),
            constraint_draft: String::new(),
            error: None,
        }
    }
}

fn default_projects_dir() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return PathBuf::from(h).join("helion-projects");
        }
    }
    PathBuf::from("/tmp/helion-projects")
}

const CREATE_WIZARD_PARTS: &[&str] = &["HL10T-C32-1", "HL10T-DSP1"];
const CREATE_WIZARD_STEPS: &[&str] = &[
    "Project name",
    "Part",
    "Add sources",
    "Add constraints",
    "Finish",
];

struct HelionIde {
    model: IdeModel,
    tree_filter: String,
    sidebar_hidden: bool,
    sidebar_width: f32,
    console_height: f32,
    activity: Activity,
    canvas: Canvas,
    show_tcl: bool,
    palette: commands::PaletteState,
    show_cheatsheet: bool,
    show_examples: bool,
    show_create_project: bool,
    create_wizard: CreateProjectWizard,
    recent: Vec<PathBuf>,
    tcl_focus: bool,
    /// Last Program rail action: (ok, message) for honest empty/error/progress.
    program_status: Option<(bool, String)>,
    /// Cable picker: auto|sim|usb|ofl|native (wired to helion-hw resolve_cable).
    program_cable: String,
    /// Optional framebuffer dump: HELION_SHOTDIR + HELION_SHOT (then HELION_SHOT_QUIT=1).
    shot_dir: Option<PathBuf>,
    shot_name: Option<String>,
    shot_quit: bool,
    shot_frames: u32,
    shot_done: bool,
    /// Cached USB/OFL scan - detect_boards shells openFPGALoader; never call it every frame.
    board_detect: Option<helion_hw::DetectReport>,
    trace: UiTrace,
    job: Option<JobHandle>,
    /// Simulation-ILA background capture result channel (progress atomics live in the model).
    ila_rx: Option<std::sync::mpsc::Receiver<Result<ila::IlaCapture, String>>>,
    busy: bool,
    progress: String,
    /// Live testbench stream for the in-flight `JobKind::TbRun` (kept on the
    /// app, not the model: the model is cloned onto the engine thread).
    tb_rx: Option<mpsc::Receiver<TbStreamEvent>>,
    /// Remaining testbenches for "Run all" (started one per job completion).
    tb_queue: Vec<String>,
    /// Last window size we shared chrome against.
    last_share: [f32; 2],
    force_share: bool,
    /// Design tokens (P0-4). All paint code reads these; no raw hex in the shell.
    theme: theme::Theme,
    theme_dirty: bool,
    /// Toast notifications (P1-5).
    toasts: toasts::ToastQueue,
    /// Layout preset + persisted chrome (P1-1).
    layout_state: layouts::LayoutState,
    /// Welcome page dismissed this session.
    welcome_dismissed: bool,
    /// Physical View 2.0 state: layer model, docks, rulers, find palette.
    physview: physview_panels::PhysViewState,
    /// Pending zoom-to-selection target (die tile coords), consumed by paint_device.
    device_zoom_target: Option<(u32, u32)>,
    /// Constraints Wizard draft state (None = closed). Lives on the app, not
    /// the model, so engine-thread jobs never disturb it.
    constraints_wizard: Option<constraints_wizard_ui::WizardUi>,
    /// Editable source editor (UX fixes Part 2): buffer + backing file.
    editor_text: String,
    editor_path: Option<PathBuf>,
    editor_dirty: bool,
    /// Soft-wrap toggle (Alt+Z / header button).
    editor_wrap: bool,
    /// Visual rows per logical line (wrap mode), refreshed by the layouter.
    editor_line_rows: Vec<usize>,
    /// Longest laid-out line width: sizes the TextEdit so it never scrolls
    /// internally (the outer ScrollArea owns scrolling).
    editor_content_w: f32,
    /// Pending minimap-driven vertical scroll offset.
    editor_mini_scroll: Option<f32>,
    /// Cached minimap galley: (text hash, font size bits, Arc galley).
    editor_minimap_cache: Option<(u64, u32, std::sync::Arc<egui::Galley>)>,
}

impl HelionIde {
    fn new() -> Self {
        let mut model = IdeModel::new();
        let has_rtl = !model.tree.sources.is_empty();
        // Floorplan is already preloaded on IdeModel::new (HAD die, no sources).
        let canvas = if has_rtl {
            Canvas::Device
        } else {
            Canvas::Editor
        };
        let activity = if has_rtl {
            Activity::Device
        } else {
            Activity::Files
        };
        model.workspace = if has_rtl {
            WorkspaceTab::Device
        } else {
            WorkspaceTab::TextEditor
        };
        model.bottom_tab = BottomTab::Tcl;
        let mut app = Self {
            model,
            tree_filter: String::new(),
            sidebar_hidden: false,
            sidebar_width: chrome::SIDEBAR_WIDTH,
            console_height: chrome::CONSOLE_DEFAULT_HEIGHT,
            activity,
            canvas,
            show_tcl: false,
            palette: commands::PaletteState::default(),
            show_cheatsheet: false,
            show_examples: false,
            show_create_project: false,
            create_wizard: CreateProjectWizard::default(),
            recent: load_recent(),
            tcl_focus: false,
            program_status: None,
            program_cable: "auto".into(),
            shot_dir: std::env::var("HELION_SHOTDIR").ok().map(PathBuf::from),
            shot_name: std::env::var("HELION_SHOT").ok(),
            shot_quit: std::env::var("HELION_SHOT_QUIT").ok().as_deref() == Some("1"),
            shot_frames: 0,
            shot_done: false,
            board_detect: None,
            trace: UiTrace::from_env(),
            job: None,
            ila_rx: None,
            busy: false,
            progress: String::new(),
            tb_rx: None,
            tb_queue: Vec::new(),
            last_share: [0.0, 0.0],
            force_share: true,
            theme: theme::Theme::dark(),
            theme_dirty: true,
            toasts: toasts::ToastQueue::default(),
            layout_state: layouts::LayoutState::default(),
            welcome_dismissed: false,
            physview: physview_panels::PhysViewState::default(),
            device_zoom_target: None,
            constraints_wizard: None,
            editor_text: String::new(),
            editor_path: None,
            editor_dirty: false,
            editor_wrap: false,
            editor_line_rows: Vec::new(),
            editor_content_w: 600.0,
            editor_mini_scroll: None,
            editor_minimap_cache: None,
        };
        if let Ok(w) = std::env::var("HELION_SIDEBAR_WIDTH") {
            if let Ok(v) = w.parse::<f32>() {
                app.sidebar_width = v.clamp(chrome::SIDEBAR_MIN_WIDTH, chrome::SIDEBAR_MAX_WIDTH);
            }
        }
        if let Ok(h) = std::env::var("HELION_CONSOLE_HEIGHT") {
            if let Ok(v) = h.parse::<f32>() {
                app.console_height =
                    v.clamp(chrome::CONSOLE_MIN_HEIGHT, chrome::CONSOLE_MAX_HEIGHT);
            }
        }
        // Optional launch hooks for Mac shots / demos (does not remove Open…).
        if let Ok(path) = std::env::var("HELION_OPEN") {
            let pb = PathBuf::from(path.trim());
            if pb.is_file() {
                app.open_path(&pb);
            }
        }
        // Demo/shot hook: auto-save one checkpoint per flow stage.
        if std::env::var("HELION_AUTO_CHECKPOINT").as_deref() == Ok("1") {
            app.model.auto_checkpoint = true;
        }
        match std::env::var("HELION_FLOW").as_deref() {
            Ok("implement") => {
                let _ = app.model.implement();
                let _ = app.model.device_zoom_fit();
                app.set_canvas(Canvas::Device);
                app.set_activity(Activity::Device);
            }
            Ok("synth") => {
                let _ = app.model.run_step(FlowStep::Synthesis);
            }
            Ok("sim") => {
                let _ = app.model.exec("sim_run");
                app.set_activity(Activity::Simulate);
            }
            _ => {}
        }
        if let Ok(name) = std::env::var("HELION_WORKSPACE") {
            if let Some(tab) = WorkspaceTab::parse_label(&name) {
                // Canvas tabs Text Editor / Device switch the canvas; Reports
                // goes through More routing so it lands on the catalog with
                // activity=Reports. Without this, open_more_workspace hits its
                // debug_assert for Device because central_pane follows the
                // canvas for canvas tabs.
                if matches!(tab, WorkspaceTab::TextEditor | WorkspaceTab::Device) {
                    app.set_canvas(match tab {
                        WorkspaceTab::TextEditor => Canvas::Editor,
                        _ => Canvas::Device,
                    });
                } else {
                    open_more_workspace(&mut app, tab);
                }
            }
        }
        if let Ok(name) = std::env::var("HELION_ACTIVITY") {
            if let Some(act) = Activity::ALL
                .iter()
                .copied()
                .find(|a| a.label().eq_ignore_ascii_case(name.trim()))
            {
                app.set_activity(act);
            }
        }
        // Cross-probe demo/screenshot hook: probe a timing path by index
        // after the flow and workspace are set up. Unlike select_timing_path,
        // this does not move the workspace, so it composes with
        // HELION_WORKSPACE (Timing/Device/Schematic views).
        if let Ok(spec) = std::env::var("HELION_PROBE_PATH") {
            if let Ok(idx) = spec.trim().parse::<usize>() {
                let _ = app.model.probe_timing_path(idx);
            }
        }
        if let Ok(cmds) = std::env::var("HELION_EXEC") {
            for cmd in cmds.split(';') {
                let t = cmd.trim();
                if !t.is_empty() {
                    let _ = app.model.exec(t);
                }
            }
            // Save Project As / open_project push the .prj onto Recent.
            if let Some(prj) = app.model.last_project_path.clone() {
                app.remember(prj);
            }
        }
        // Screenshot/demo hook: queue an async testbench run at startup
        // (HELION_EXEC runs `tb_rescan <dir>` first).
        if let Ok(sel) = std::env::var("HELION_TB_AUTORUN") {
            if !sel.trim().is_empty() {
                tb_start_run(&mut app, sel.trim().to_string());
            }
        }
        if let Ok(name) = std::env::var("HELION_BOTTOM") {
            if let Some(tab) = BottomTab::ALL.iter().copied().find(|t| {
                format!("{t:?}").eq_ignore_ascii_case(name.trim())
                    || t.label().eq_ignore_ascii_case(name.trim())
            }) {
                app.model.bottom_tab = tab;
            }
        }
        if let Ok(z) = std::env::var("HELION_DEVICE_ZOOM") {
            if let Ok(v) = z.parse::<f32>() {
                app.model.device_zoom = v.clamp(0.40, 6.0);
            }
        }
        // Screenshot/demo hook: auto-open the Constraints Wizard, optionally
        // on a given step (HELION_WIZARD_STEP=0..4).
        if let Some(step) = constraints_wizard_ui::auto_open_from_env() {
            let mut w = constraints_wizard_ui::WizardUi::new(&app.model);
            w.set_step_idx(step);
            app.constraints_wizard = Some(w);
        }
        layouts::load_chrome_state(&mut app);
        app
    }

    fn snapshot(&self) -> ChromeState {
        ChromeState {
            activity: self.activity,
            canvas: self.canvas,
            workspace: self.model.workspace,
            sidebar_hidden: self.sidebar_hidden,
            sidebar_width: self.sidebar_width,
            console_height: self.console_height,
        }
    }

    fn restore(&mut self, s: ChromeState) {
        self.activity = s.activity;
        self.canvas = s.canvas;
        self.model.workspace = s.workspace;
        self.sidebar_hidden = s.sidebar_hidden;
        self.sidebar_width = s.sidebar_width;
        self.console_height = s.console_height;
    }

    fn log_click(&mut self, widget: &str, expected: &str) {
        let actual = format!("{:?}", central_pane(&self.snapshot()));
        let class = if actual == expected || actual.contains(expected) {
            EventClass::Ok
        } else {
            EventClass::StatePaint
        };
        self.trace.push(UiEvent {
            at_ms: 0,
            kind: "click",
            widget: widget.into(),
            expected: expected.into(),
            actual,
            class,
        });
    }

    fn set_canvas(&mut self, c: Canvas) {
        let mut s = self.snapshot();
        apply_canvas(&mut s, c);
        self.restore(s);
        self.log_click(
            c.label(),
            &format!(
                "{:?}",
                match c {
                    Canvas::Editor => WorkspacePane::Editor,
                    Canvas::Device => WorkspacePane::Device,
                    Canvas::Timing => WorkspacePane::Timing,
                }
            ),
        );
    }

    fn set_activity(&mut self, a: Activity) {
        let mut s = self.snapshot();
        apply_activity(&mut s, a);
        self.restore(s);
        if a == Activity::Files {
            self.model.layout = LayoutKind::Default;
        }
        if a == Activity::Reports && self.model.selected_report.is_none() {
            // Windows layout: unused empty pane is incorrect. Open Timing Summary.
            self.model.selected_report = Some("report_timing_summary".into());
        }
        self.log_click(a.label(), a.label());
    }

    fn submit_job(&mut self, kind: JobKind) {
        if self.job.is_some() {
            // Never drop a design-run queue item: park it back at the front;
            // the completion handler chains the queue.
            if let JobKind::DesignRun { name } = &kind {
                if !self.model.run_queue.iter().any(|q| q == name) {
                    self.model.run_queue.push_front(name.clone());
                }
            }
            self.progress = "Busy - wait for the current run to finish.".into();
            if matches!(kind, JobKind::TbRun(_)) {
                // The run never started: roll back tb_start_run's optimistic
                // Running state so the badge/spinner cannot stick.
                self.tb_rx = None;
                self.model.tb_run_active = false;
                self.model.tb_cancel = None;
                if let Some(i) = self.model.tb_selected {
                    if self.model.tb_status.get(i) == Some(&TbStatus::Running) {
                        self.model.tb_status[i] = TbStatus::Idle;
                    }
                }
            }
            return;
        }
        self.busy = true;
        self.progress = kind.progress_english();
        self.trace.push(UiEvent {
            at_ms: 0,
            kind: "job",
            widget: kind.widget().into(),
            expected: "engine-thread".into(),
            actual: "queued".into(),
            class: EventClass::Ok,
        });
        let model = self.model.clone();
        // No "Started" toast: job progress already lives in the status bar
        // (busy spinner + progress segment). Toasts are for completions only.
        self.job = Some(spawn_job(model, kind));
    }

    /// Drain live testbench events into the panel (log lines, failures).
    fn drain_tb_stream(&mut self) {
        let mut progress_tick: Option<u64> = None;
        if let Some(rx) = self.tb_rx.as_ref() {
            for ev in rx.try_iter() {
                match ev {
                    TbStreamEvent::Log { time_ns, sev, text } => {
                        self.model.tb_log.push(TbLogLine { time_ns, sev, text });
                    }
                    TbStreamEvent::Failure { file, line, message, time_ns, signals } => {
                        let f = TbFailure { file, line, message, time_ns, signals, instance: String::new() };
                        if !self.model.tb_failures.contains(&f) {
                            self.model.tb_failures.push(f);
                        }
                    }
                    TbStreamEvent::Progress { time_ns } => {
                        progress_tick = Some(time_ns);
                    }
                }
            }
        }
        if let Some(t) = progress_tick {
            self.progress = format!("Testbench {} - {t} ns", self.model.tb_run_name);
        }
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        self.drain_tb_stream();
        let Some(job) = self.job.as_ref() else {
            return;
        };
        match job.try_recv() {
            Ok(out) => {
                if matches!(out.kind, JobKind::TbRun(_)) {
                    // Merge - the streamed log is already live; only
                    // status/history/summary merge in.
                    self.drain_tb_stream();
                    self.model.tb_merge_finished(&out.model);
                    self.tb_rx = None;
                    self.busy = false;
                    self.progress = match &out.result {
                        Ok(s) if s.is_empty() => "Testbench finished.".into(),
                        Ok(s) => s.clone(),
                        Err(e) => e.clone(),
                    };
                    self.model.tb_last_summary = self.progress.clone();
                    self.toasts.push(
                        if out.result.is_ok() {
                            toasts::ToastKind::Ok
                        } else {
                            toasts::ToastKind::Error
                        },
                        format!("Testbench - {}", self.progress),
                    );
                    self.job = None;
                    // "Run all": chain the next queued testbench - unless the
                    // user stopped the run (a stop aborts the whole sweep).
                    if !self.tb_queue.is_empty() && !self.progress.contains("STOPPED") {
                        let next = self.tb_queue.remove(0);
                        tb_start_run(self, next);
                    }
                    return;
                }
                // The job ran on a model clone: merge back live design-run
                // state (queue edits, newly created runs) before adopting
                // the clone, so mid-flight panel edits are never lost.
                let live_queue = std::mem::take(&mut self.model.run_queue);
                let live_runs = std::mem::take(&mut self.model.runs);
                self.model = out.model;
                for name in live_queue {
                    if !self.model.run_queue.iter().any(|q| q == &name) {
                        self.model.run_queue.push_back(name);
                    }
                }
                for r in live_runs {
                    if !self.model.runs.iter().any(|e| e.name == r.name) {
                        self.model.runs.push(r);
                    }
                }
                self.busy = false;
                self.progress = match &out.result {
                    Ok(s) if s.is_empty() => format!("{} finished.", out.kind.widget()),
                    Ok(s) => s.clone(),
                    Err(e) => e.clone(),
                };
                // Short toast; the detail string stays in the status bar
                // progress segment and the Messages tab.
                let widget = out.kind.widget();
                self.toasts.push(
                    if out.result.is_ok() {
                        toasts::ToastKind::Ok
                    } else {
                        toasts::ToastKind::Error
                    },
                    if out.result.is_ok() {
                        format!("{widget} finished")
                    } else {
                        format!("{widget} failed - see Messages")
                    },
                );
                if matches!(out.kind, JobKind::Implement) && out.result.is_ok() {
                    self.set_canvas(Canvas::Device);
                }
                // Open/Implement failures: Messages tab (parity; surface_stage_error also sets it).
                if matches!(out.kind, JobKind::Implement | JobKind::Open(_)) {
                    match &out.result {
                        Ok(_) if matches!(out.kind, JobKind::Open(_)) => {
                            self.set_activity(Activity::Files);
                        }
                        Ok(_) => {}
                        Err(_) => {
                            self.model.bottom_tab = BottomTab::Messages;
                        }
                    }
                }
                if matches!(out.kind, JobKind::SimRun(_)) && out.result.is_ok() {
                    self.set_activity(Activity::Simulate);
                }
                if matches!(out.kind, JobKind::ReportTiming) {
                    constraints_wizard_ui::on_timing_done(self, out.result);
                }
                // Design-run queue chaining. `submit_job` refuses while a job
                // is live, so drop the handle first.
                let chained = if matches!(out.kind, JobKind::DesignRun { .. }) {
                    self.model.run_queue.pop_front()
                } else {
                    None
                };
                self.job = None;
                if let Some(name) = chained {
                    self.submit_job(JobKind::DesignRun { name });
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.busy = false;
                self.progress = "Engine thread stopped.".into();
                self.tb_rx = None;
                self.model.tb_run_active = false;
                self.job = None;
            }
        }
    }

    /// Poll the simulation-ILA background capture without blocking the frame.
    fn poll_ila(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.ila_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(r) => {
                let ok = r.is_ok();
                let summary = match &r {
                    Ok(cap) => format!(
                        "ILA capture: {} probes, {} samples, trigger {}",
                        cap.probes.len(),
                        cap.len(),
                        cap.trigger_sample
                            .map(|i| i.to_string())
                            .unwrap_or_else(|| "-".into())
                    ),
                    Err(e) => e.clone(),
                };
                self.model.sim_ila.finish_capture(r);
                self.ila_rx = None;
                self.toasts.push(
                    if ok {
                        toasts::ToastKind::Ok
                    } else {
                        toasts::ToastKind::Error
                    },
                    summary,
                );
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.model.sim_ila.status = SimIlaStatus::Error;
                self.model.sim_ila.last_error = Some("ila: capture thread died".into());
                self.model.sim_ila.progress = None;
                self.ila_rx = None;
            }
        }
    }

    fn boards(&mut self, refresh: bool) -> &helion_hw::DetectReport {
        if refresh || self.board_detect.is_none() {
            self.board_detect = Some(helion_hw::detect_boards());
        }
        self.board_detect.as_ref().expect("board detect cached")
    }

    fn remember(&mut self, path: PathBuf) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        self.recent.retain(|p| p != &path);
        self.recent.insert(0, path);
        self.recent.truncate(8);
        save_recent(&self.recent);
    }

    fn open_path(&mut self, path: &Path) {
        self.remember(path.to_path_buf());
        match self.model.open_source(path) {
            Ok(_) => self.set_activity(Activity::Files),
            Err(e) => {
                self.progress = e;
            }
        }
    }

    fn open_path_async(&mut self, path: &Path) {
        self.remember(path.to_path_buf());
        self.submit_job(JobKind::Open(path.to_path_buf()));
    }
}

/// Recent menu label: prefer clear `.prj` names (`counter.prj  (project)`).
fn recent_menu_label(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "prj" {
        format!("{name}  (project)")
    } else {
        name
    }
}

fn recent_store_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("HELION_RECENT") {
        let pb = PathBuf::from(p.trim());
        if !pb.as_os_str().is_empty() {
            return Some(pb);
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".helion").join("recent.txt"))
}

fn load_recent() -> Vec<PathBuf> {
    let Some(store) = recent_store_path() else {
        return Vec::new();
    };
    load_recent_from(&store)
}

fn load_recent_from(store: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(store) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let p = PathBuf::from(line);
        if p.is_file() && !out.iter().any(|q| q == &p) {
            out.push(p);
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}

fn save_recent(paths: &[PathBuf]) {
    let Some(store) = recent_store_path() else {
        return;
    };
    save_recent_to(&store, paths);
}

fn save_recent_to(store: &Path, paths: &[PathBuf]) {
    if let Some(parent) = store.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut body = String::new();
    for p in paths {
        body.push_str(&p.display().to_string());
        body.push('\n');
    }
    let _ = std::fs::write(store, body);
}

impl eframe::App for HelionIde {
    /// Idle policy: reactive eframe only - never `request_repaint` / Continuous here.
    /// Synth/implement run on the engine thread via submit_job / queue_flow.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        handle_shortcuts(ctx, self);
        if self.theme_dirty {
            self.theme.apply(ctx);
            self.theme_dirty = false;
        }
        for kind in surface::take_jobs() {
            self.submit_job(kind);
        }
        self.poll_job(ctx);
        self.poll_ila(ctx);
        // Menu bar (P1-6): standard File/Edit/View/Flow/Tools/Help, driven by
        // the same command registry as the palette.
        egui::TopBottomPanel::top("menu_bar")
            .show_separator_line(true)
            .show(ctx, |ui| {
                commands::paint_menu_bar(ui, self);
            });
        paint_toolbar(ctx, self);
        apply_shared_chrome(ctx, self);
        paint_status_bar(ctx, self);
        paint_bottom(ctx, self);
        paint_activity_rail(ctx, self);
        if !self.sidebar_hidden {
            paint_sidebar(ctx, self);
        }
        // Consume the re-share flag here - not inside the sidebar painter, which
        // runs after paint_bottom and never runs when the sidebar is hidden.
        // A stuck flag pinned the bottom console every frame and killed dragging.
        self.force_share = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            let theme = self.theme;
            if welcome::should_show(self) {
                welcome::paint_welcome(ui, self, &theme);
            } else {
                paint_workspace(ui, self);
            }
        });
        paint_tcl_window(ctx, self);
        commands::paint_palette(ctx, self);
        commands::paint_shortcut_cheatsheet(ctx, self);
        self.toasts.paint(ctx, &self.theme);
        paint_examples_popup(ctx, self);
        paint_create_project_wizard(ctx, self);
        capture_shot(ctx, self);
        paint_debug_overlay(ctx, self);
    }
}

fn paint_debug_overlay(ctx: &egui::Context, app: &HelionIde) {
    if std::env::var("HELION_DEBUG_OVERLAY").ok().as_deref() != Some("1") {
        return;
    }
    // egui 0.32: Context::set_debug_on_hover is #[cfg(debug_assertions)] only.
    #[cfg(debug_assertions)]
    ctx.set_debug_on_hover(true);
    let s = app.snapshot();
    let pane = central_pane(&s);
    egui::Window::new("surface debug")
        .anchor(egui::Align2::RIGHT_TOP, [-8.0, 48.0])
        .resizable(false)
        .collapsible(true)
        .show(ctx, |ui| {
            ui.monospace(format!(
                "rail={} sidebar={:.0} console={:.0}\nactivity={:?}\ncanvas={:?}\nworkspace={:?}\npane={:?}\nbusy={} {}",
                chrome::RAIL_WIDTH,
                s.sidebar_width,
                s.console_height,
                s.activity,
                s.canvas,
                s.workspace,
                pane,
                app.busy,
                app.progress
            ));
        });
}

fn capture_shot(ctx: &egui::Context, app: &mut HelionIde) {
    let Some(dir) = app.shot_dir.clone() else {
        return;
    };
    if app.shot_done {
        return;
    }
    ctx.request_repaint();
    app.shot_frames = app.shot_frames.saturating_add(1);
    if app.shot_frames == 4 {
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
    }
    let mut image: Option<std::sync::Arc<egui::ColorImage>> = None;
    ctx.input(|i| {
        for ev in &i.raw.events {
            if let egui::Event::Screenshot { image: img, .. } = ev {
                image = Some(img.clone());
            }
        }
    });
    let Some(image) = image else {
        if app.shot_frames > 90 {
            eprintln!("helion-ide: screenshot timed out");
            if app.shot_quit {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            app.shot_done = true;
        }
        return;
    };
    let name = app.shot_name.clone().unwrap_or_else(|| "helion".into());
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.ppm"));
    if let Err(e) = write_ppm(&path, &image) {
        eprintln!("helion-ide: shot {}: {e}", path.display());
    } else {
        eprintln!("helion-ide: wrote {}", path.display());
    }
    app.shot_done = true;
    if app.shot_quit {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn write_ppm(path: &Path, image: &egui::ColorImage) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    write!(f, "P6\n{} {}\n255\n", image.size[0], image.size[1])?;
    for px in &image.pixels {
        f.write_all(&[px.r(), px.g(), px.b()])?;
    }
    Ok(())
}

fn handle_shortcuts(ctx: &egui::Context, app: &mut HelionIde) {
    let cmd = egui::Modifiers::COMMAND;
    if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::O)) {
        native_open(app);
    }
    if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::Enter)) {
        run_implement(app);
    }
    if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::Num1)) {
        app.set_canvas(Canvas::Editor);
    }
    if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::Num2)) {
        app.set_canvas(Canvas::Device);
    }
    if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::Num3)) {
        app.set_canvas(Canvas::Timing);
    }
    if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::J)) {
        app.sidebar_hidden = !app.sidebar_hidden;
    }
    if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::Backtick)) {
        app.show_tcl = !app.show_tcl;
        if app.show_tcl {
            app.tcl_focus = true;
            app.model.bottom_tab = BottomTab::Tcl;
        }
    }
    if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::P)) {
        app.palette.open = !app.palette.open;
        app.palette.mode = commands::PaletteMode::Files;
        app.palette.selected = 0;
        app.palette.query.clear();
    }
    if ctx.input_mut(|i| i.consume_key(cmd | egui::Modifiers::SHIFT, egui::Key::P)) {
        app.palette.open = true;
        app.palette.mode = commands::PaletteMode::Commands;
        app.palette.selected = 0;
        app.palette.query.clear();
    }
    if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::K)) {
        app.show_cheatsheet = !app.show_cheatsheet;
    }
    if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::S)) {
        // Save the open source file (editor canvas only; no project binding
        // uses ⌘S today).
        if app.canvas == Canvas::Editor {
            editor::save_editor(app);
        }
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, egui::Key::Z)) {
        app.editor_wrap = !app.editor_wrap;
    }
}

fn native_close_project(app: &mut HelionIde) {
    match app.model.close_project() {
        Ok(msg) => {
            app.progress = msg;
            app.set_activity(Activity::Files);
        }
        Err(e) => {
            app.progress = e;
        }
    }
}

fn native_save_project_as(app: &mut HelionIde) {
    let default_name = app
        .model
        .design()
        .map(|d| d.name.clone())
        .or_else(|| app.model.tree.top.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "project".into());
    let start = default_projects_dir();
    let start_s = start.to_string_lossy().into_owned();
    let Some(dir) = pick_directory_dialog(&start_s) else {
        return;
    };
    match app.model.save_project_as(&dir, Some(&default_name)) {
        Ok(msg) => {
            if let Some(prj) = app.model.last_project_path.clone() {
                app.remember(prj);
            }
            app.progress = msg;
            app.set_activity(Activity::Files);
        }
        Err(e) => {
            app.progress = e;
        }
    }
}

fn native_open(app: &mut HelionIde) {
    if let Some(path) = native_open_dialog() {
        app.open_path_async(&path);
    }
}

fn native_open_dialog() -> Option<PathBuf> {
    open_hdl_dialog()
}

fn run_implement(app: &mut HelionIde) {
    if app.model.step_blocked(FlowStep::Synthesis).is_some() {
        return;
    }
    app.submit_job(JobKind::Implement);
}

fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add_sized(
        chrome::toolbar_ctrl_size(text),
        egui::Button::new(RichText::new(text).size(14.0)),
    )
}

/// Fill leftover pane with an empty-state CTA (not a heading plus a dark hole).
fn paint_remaining_cta(
    ui: &mut egui::Ui,
    message: &str,
    action: &str,
    theme: &theme::Theme,
) -> bool {
    let avail = ui.available_size().max(egui::vec2(160.0, 120.0));
    let fit = chrome::empty_cta_occupies_pane(48.0, avail.y);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(avail.x, fit.drawn_h.max(avail.y)),
        Sense::hover(),
    );
    let mut clicked = false;
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 4.0, theme.bg);
        ui.painter().rect_stroke(
            rect,
            4.0,
            Stroke::new(1.0_f32, theme.line),
            egui::StrokeKind::Inside,
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((rect.height() * 0.38).max(24.0));
                ui.label(RichText::new(message).size(14.0).color(theme.dim));
                ui.add_space(10.0);
                if primary_button(ui, action).clicked() {
                    clicked = true;
                }
            });
        });
    }
    clicked
}

fn sidebar_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add_sized(
        [ui.spacing().interact_size.x.max(72.0), chrome::HIT_SIDEBAR],
        egui::Button::new(text),
    )
}

fn tip(name: &str, shortcut: &str, tcl: &str) -> String {
    if tcl.is_empty() {
        format!("{name}  {shortcut}")
    } else {
        format!("{name}  {shortcut}\n{tcl}")
    }
}

fn data_scroll(id: &'static str) -> egui::ScrollArea {
    let p = chrome::table_scroll_policy(10, chrome::DESKTOP_WIDTH);
    let sa = if p.x && p.y {
        egui::ScrollArea::both()
    } else if p.x {
        egui::ScrollArea::horizontal()
    } else {
        egui::ScrollArea::vertical()
    };
    sa.id_salt(id)
        .auto_shrink([false, true])
        .max_height(p.max_height)
}

fn paint_toolbar(ctx: &egui::Context, app: &mut HelionIde) {
    egui::TopBottomPanel::top("toolbar")
        .exact_height(chrome::TOOLBAR_HEIGHT)
        .show(ctx, |ui| {
            // Horizontal scroll when the window is too narrow for all controls:
            // buttons are never clipped into unreachability (D7). The
            // right-aligned flow buttons stay at the visible right edge on
            // wide windows and scroll with the content on narrow ones.
            egui::ScrollArea::horizontal()
                .id_salt("toolbar_hscroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                ui.add_space(6.0);
                let new_proj = ui
                    .add_sized(
                        chrome::toolbar_ctrl_size("New Project…"),
                        egui::Button::new("New Project…"),
                    )
                    .on_hover_text(tip("New Project", "", "create_project"));
                if new_proj.clicked() {
                    app.create_wizard = CreateProjectWizard::default();
                    app.show_create_project = true;
                }
                let open = ui
                    .add_sized(
                        chrome::toolbar_ctrl_size("Open…"),
                        egui::Button::new("Open…"),
                    )
                    .on_hover_text(tip("Open", "⌘O", "open_source"));
                if open.clicked() {
                    native_open(app);
                }
                let save_as = ui
                    .add_sized(
                        chrome::toolbar_ctrl_size("Save Project As…"),
                        egui::Button::new("Save Project As…"),
                    )
                    .on_hover_text(tip("Save Project As", "", "save_project_as"));
                if save_as.clicked() {
                    native_save_project_as(app);
                }
                let close_proj = ui
                    .add_sized(
                        chrome::toolbar_ctrl_size("Close Project"),
                        egui::Button::new("Close Project"),
                    )
                    .on_hover_text(tip("Close Project", "", "close_project"));
                if close_proj.clicked() {
                    native_close_project(app);
                }
                let recent_sz = chrome::toolbar_ctrl_size("Recent");
                ui.allocate_ui(egui::vec2(recent_sz[0], recent_sz[1]), |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.menu_button("Recent", |ui| {
                            if app.recent.is_empty() {
                                ui.label("No recent files.");
                            } else {
                                let paths: Vec<PathBuf> = app.recent.clone();
                                for p in paths {
                                    let name = recent_menu_label(&p);
                                    if ui.button(name).clicked() {
                                        app.open_path_async(&p);
                                        ui.close();
                                    }
                                }
                            }
                        });
                    });
                });
                let ex_sz = chrome::toolbar_ctrl_size("Examples");
                ui.allocate_ui(egui::vec2(ex_sz[0], ex_sz[1]), |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.menu_button("Examples", |ui| {
                            let mut pick = None;
                            for (label, file) in RAIL_OPEN_SOURCES {
                                if ui.button(label).clicked() {
                                    pick = Some(file);
                                    ui.close();
                                }
                            }
                            if let Some(file) = pick {
                                let p = helion_device::Device::examples_dir().join(file);
                                app.open_path_async(&p);
                            }
                        });
                    });
                });
                ui.separator();
                paint_progress_strip(ui, app);
                ui.separator();
                // Layout presets (P1-1): task-curated chrome arrangements.
                ui.label(RichText::new("Layout").small().color(app.theme.dim))
                    .on_hover_text("Task-curated window arrangement (persists)");
                layouts::paint_layout_selector(ui, app);
                // Right-pin the flow buttons WITHOUT overlapping the left content:
                // a spacer takes the remaining viewport width minus the buttons'
                // own width (min 16px gap), then buttons lay out left-to-right in
                // visual order. Narrow windows collapse the spacer and the row
                // scrolls (VS Code pattern) instead of painting "BitstreamDesign"
                // text soup.
                let busy = app.busy;
                let impl_label = if busy { "Implementing…" } else { "Implement" };
                let right_w = chrome::toolbar_ctrl_size(impl_label)[0]
                    + chrome::toolbar_ctrl_size("Bitstream")[0]
                    + ui.spacing().item_spacing.x;
                ui.add_space((ui.available_width() - right_w).max(16.0));
                let bits_blocked = app.model.step_blocked(FlowStep::Bitstream);
                ui.add_enabled_ui(bits_blocked.is_none(), |ui| {
                    let b = ui
                        .add_sized(
                            chrome::toolbar_ctrl_size("Bitstream"),
                            egui::Button::new("Bitstream"),
                        )
                        .on_hover_text(match bits_blocked {
                            Some(why) => why.to_string(),
                            None => tip("Bitstream", "", "write_bitstream"),
                        });
                    if b.clicked() {
                        app.submit_job(JobKind::Step(FlowStep::Bitstream));
                    }
                });
                let synth_blocked = app.model.step_blocked(FlowStep::Synthesis);
                ui.add_enabled_ui(synth_blocked.is_none(), |ui| {
                    let impl_btn = ui
                        .add_sized(
                            chrome::toolbar_ctrl_size(impl_label),
                            egui::Button::new(RichText::new(impl_label).strong()),
                        )
                        .on_hover_text(match synth_blocked {
                            Some(why) => why.to_string(),
                            None if busy => "Busy - wait for the current run.".into(),
                            None => tip("Implement", "⌘↩", "impl_design"),
                        });
                    if impl_btn.clicked() {
                        run_implement(app);
                    }
                });
        });
        });
    });
}

fn paint_progress_strip(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    const STEPS: [FlowStep; 4] = [
        FlowStep::Synthesis,
        FlowStep::Opt,
        FlowStep::Place,
        FlowStep::Route,
    ];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let n = STEPS.len();
        for (i, step) in STEPS.iter().copied().enumerate() {
            let state = app.model.step_state(step);
            let blocked = app.model.step_blocked(step);
            let (fill, stroke, text) = match state {
                StepState::Pending => (theme.surface, theme.faint, theme.text),
                StepState::Done => (theme.ok.gamma_multiply(0.28), theme.ok, theme.text),
                StepState::Failed => (theme.danger.gamma_multiply(0.3), theme.danger, theme.text),
            };
            ui.add_enabled_ui(blocked.is_none(), |ui| {
                let chip = chrome::flow_chip_size();
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(chip[0], chip[1]), Sense::click());
                if ui.is_rect_visible(rect) {
                    ui.painter().rect(
                        rect,
                        2.0,
                        fill,
                        Stroke::new(1.0_f32, stroke),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        step.label(),
                        egui::FontId::proportional(11.0),
                        text,
                    );
                }
                let hover = match blocked {
                    Some(why) => why.to_string(),
                    None => tip(step.label(), "", step.tcl()),
                };
                let resp = resp.on_hover_text(hover);
                if resp.clicked() && blocked.is_none() {
                    app.submit_job(JobKind::Step(step));
                }
            });
            if i + 1 < n {
                ui.label(RichText::new("·").weak().size(11.0));
            }
        }
    });
}

fn paint_activity_rail(ctx: &egui::Context, app: &mut HelionIde) {
    egui::SidePanel::left("activity_rail")
        .exact_width(chrome::RAIL_WIDTH)
        .resizable(false)
        .show_separator_line(true)
        .show(ctx, |ui| {
            ui.add_space(4.0);
            ui.spacing_mut().item_spacing.y = 4.0;
            let theme = app.theme;
            let mut pick = None;
            for act in Activity::ALL {
                let on = app.activity == act;
                let fill = if on { theme.sel } else { Color32::TRANSPARENT };
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(chrome::RAIL_WIDTH - 4.0, chrome::HIT_RAIL),
                    Sense::click(),
                );
                if ui.is_rect_visible(rect) {
                    ui.painter().rect(
                        rect,
                        3.0,
                        fill,
                        Stroke::new(if on { 1.0_f32 } else { 0.0_f32 }, theme.accent),
                        egui::StrokeKind::Inside,
                    );
                    let c = rect.center();
                    // Vector glyph (icons.rs) - no letter glyphs, no emoji.
                    let icon_color = if on { theme.accent } else { theme.dim };
                    icons::paint_activity(
                        ui.painter(),
                        egui::Rect::from_center_size(
                            egui::pos2(c.x, c.y - 10.0),
                            egui::vec2(26.0, 26.0),
                        ),
                        act,
                        icon_color,
                    );
                    ui.painter().text(
                        egui::pos2(c.x, c.y + 12.0),
                        egui::Align2::CENTER_CENTER,
                        act.short_label(),
                        theme::font_ui(12.0),
                        if on { theme.text } else { theme.dim },
                    );
                }
                let resp = resp.on_hover_text(act.hover());
                if resp.clicked() {
                    pick = Some(act);
                }
            }
            if let Some(a) = pick {
                app.set_activity(a);
            }
        });
}

fn paint_sidebar(ctx: &egui::Context, app: &mut HelionIde) {
    match app.activity {
        Activity::Simulate => {} // scopes live in-canvas (SidePanel left a thick void beside Wave)
        Activity::Timing => {}   // WNS + paths live in the Timing canvas; no catalog twin
        Activity::Files | Activity::Device | Activity::Program | Activity::Reports => {
            paint_files_side(ctx, app);
        }
    }
}

fn paint_files_side(ctx: &egui::Context, app: &mut HelionIde) {
    let share = chrome::share_available(ctx.screen_rect().width(), ctx.screen_rect().height());
    let mut width = app
        .sidebar_width
        .clamp(share.sidebar_floor, share.sidebar_cap);
    let mut panel = egui::SidePanel::left("sidebar_v3")
        .resizable(true)
        .default_width(width);
    if app.force_share {
        // Re-share frame: snap to the shared width once. The flag itself is
        // cleared at the end of the frame (not here), so a hidden sidebar
        // can't leave it stuck on.
        panel = panel.exact_width(width);
    }
    let inner = panel
        .width_range({
            let share =
                chrome::share_available(ctx.screen_rect().width(), ctx.screen_rect().height());
            share.sidebar_floor..=share.sidebar_cap
        })
        .show_separator_line(true)
        .show(ctx, |ui| {
            ui.set_width(ui.available_width());
            ui.set_max_width(ui.available_width());
            let title = match app.activity {
                Activity::Files => "Files",
                Activity::Device => "Device",
                Activity::Timing => "Timing",
                Activity::Simulate => "Simulate",
                Activity::Program => "Program",
                Activity::Reports => "Reports",
            };
            ui.label(RichText::new(title).strong().size(14.0));
            ui.add_space(4.0);
            match app.activity {
                Activity::Files => {
                    // Flow Navigator first (Vivado mental model), then the file
                    // tree below. Both live in the Files sidebar.
                    flow_nav::paint_flow_nav(ui, app);
                    ui.separator();
                    paint_files_tree(ui, app);
                }
                Activity::Device => {
                    paint_io_ports_table(ui, &mut app.model, "sidebar_io");
                }
                Activity::Timing => {
                    paint_timing_paths(ui, &mut app.model, &app.theme);
                }
                Activity::Reports => {
                    // Timing Analysis report catalog (P1-4): regroups the
                    // timing/power/quality tabs with badges - replaces the
                    // flat report list.
                    dashboard::paint_timing_catalog(ui, app);
                }
                Activity::Program => {
                    paint_program_side(ui, app);
                }
                Activity::Simulate => {}
            }
            if app.model.selected.is_some()
                || app.model.selected_source.is_some()
                || app.model.selected_io_port.is_some()
            {
                paint_properties(ui, &mut app.model);
            }
            width = ui.max_rect().width();
        });
    app.sidebar_width = inner
        .response
        .rect
        .width()
        .clamp(share.sidebar_floor, share.sidebar_cap);
    let _ = width;
}

fn paint_program_side(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    let det = app.boards(false).clone();
    ui.label(RichText::new("Cable").strong());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (id, label) in [
            ("auto", "auto"),
            ("sim", "sim"),
            ("usb", "usb/ofl"),
            ("native", "native→ofl"),
        ] {
            let selected = app.program_cable == id;
            let resp = ui.add_sized(
                [72.0, chrome::HIT_SIDEBAR],
                egui::Button::selectable(selected, label),
            );
            if resp.clicked() {
                app.program_cable = id.to_string();
            }
        }
    });
    let resolved = helion_hw::resolve_cable_from(&app.program_cable, &det).ok();
    if let Some(c) = &resolved {
        ui.label(format!("{} · {}", c.id, c.backend.as_str()));
        ui.label(RichText::new(c.detail.as_str()).small().color(theme.dim));
    }
    // No USB → honest refuse board DONE (never soft-hold success / never invent DONE).
    // The full warning lives in the Hardware Manager main panel (next to the
    // Program button); the sidebar shows only the positive state to avoid
    // showing the same warning twice in Program activity.
    if det.physical_had {
        ui.label(
            RichText::new("Physical USB programmer detected (openFPGALoader / native FTDI).")
                .color(theme.ok)
                .small(),
        );
        ui.add_space(6.0);
    }

    let bits_done = app.model.step_state(FlowStep::Bitstream) == StepState::Done;
    let frames = app
        .model
        .shell
        .session
        .bitstream
        .as_ref()
        .map(|b| b.frames.len())
        .unwrap_or(0);
    let bytes = app
        .model
        .shell
        .session
        .bitstream
        .as_ref()
        .map(|b| b.packets.len())
        .unwrap_or(0);
    ui.label(RichText::new("Bitstream").strong());
    if bits_done {
        ui.label(format!("Last implement · {frames} frames · {bytes} B"));
    } else {
        ui.label(RichText::new("No bitstream yet.").color(theme.warn));
        if primary_button(ui, "Generate Bitstream")
            .on_hover_text(tip("Bitstream", "", "write_bitstream"))
            .clicked()
        {
            app.submit_job(JobKind::Step(FlowStep::Bitstream));
        }
    }
    ui.add_space(6.0);

    let hw = app.model.hw_stat_report();
    if hw.open {
        ui.label(format!(
            "Target {} · {}",
            if hw.target == "sim" {
                "sim"
            } else {
                hw.target.as_str()
            },
            if hw.programmed { "programmed" } else { "idle" }
        ));
    } else {
        ui.label(
            RichText::new("Hardware Manager closed - Program opens sim cable.")
                .small()
                .color(theme.dim),
        );
    }
    ui.add_space(8.0);

    let cable_needs_phys = matches!(
        app.program_cable.as_str(),
        "auto" | "usb" | "ofl" | "native" | "ftdi" | "libusb" | "openfpgaloader"
    );
    let phys_blocked = cable_needs_phys && !det.physical_had;
    let phys_block_msg = "No USB programmer (USB=0) - refusing board DONE. Switch cable to sim for fabric only, or attach FTDI/HAD and Detect.";

    ui.horizontal(|ui| {
        if sidebar_button(ui, "Detect")
            .on_hover_text("Detect cables - honest USB/OFL scan; never fakes a probe")
            .clicked()
        {
            let fresh = app.boards(true).clone();
            let ofl = fresh
                .usb
                .ofl_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(not on PATH)".into());
            let summary = format!(
                "scan USB={} · OFL={} · physical_had={}",
                fresh.usb.probes.len(),
                ofl,
                u8::from(fresh.physical_had),
            );
            // Honest scan summary only - never invent a probe / board DONE.
            app.program_status = Some((fresh.physical_had, summary));
            let _ = app.model.exec("open_hw_manager");
        }
        let prog_hover = if phys_blocked {
            phys_block_msg.to_string()
        } else if bits_done {
            tip("Program", "", "program_hw")
        } else {
            "No bitstream yet - Generate Bitstream first".into()
        };
        ui.add_enabled_ui(!phys_blocked, |ui| {
            let prog = primary_button(ui, "Program").on_hover_text(prog_hover);
            if prog.clicked() {
                if phys_blocked {
                    app.program_status = Some((false, phys_block_msg.to_string()));
                } else if !bits_done {
                    app.program_status = Some((
                        false,
                        "No bitstream yet. Run Implement, then Generate Bitstream.".into(),
                    ));
                } else {
                    let cable = app.program_cable.clone();
                    let _ = app.model.exec("open_hw_manager");
                    match app.model.program_hw_with_cable(&cable) {
                        Ok(s) => {
                            app.program_status =
                                Some((true, format!("{s} · {frames} frames · {bytes} B")));
                        }
                        Err(e) => app.program_status = Some((false, e)),
                    }
                }
            }
        });
    });

    if let Some((ok, msg)) = &app.program_status {
        ui.add_space(8.0);
        ui.separator();
        let is_scan = msg.starts_with("scan USB=");
        let heading = if is_scan {
            "Scan"
        } else if *ok {
            "Result"
        } else {
            "Error"
        };
        let heading_color = if is_scan {
            theme.dim
        } else if *ok {
            theme.ok
        } else {
            theme.danger
        };
        ui.label(RichText::new(heading).strong().color(heading_color));
        ui.label(RichText::new(msg.as_str()).color(if is_scan || *ok {
            theme.dim
        } else {
            theme.danger
        }));
    }
}

fn paint_files_tree(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    let src_rows = app.model.source_rows();
    if src_rows.is_empty() {
        ui.label("No sources.");
        ui.label(
            RichText::new("Use Open… in the toolbar.")
                .small()
                .color(theme.dim),
        );
        return;
    }
    let selected_source = app.model.selected_source.clone();
    let mut pick_src: Option<String> = None;
    data_scroll("files_sources")
        .max_height(200.0)
        .show(ui, |ui| {
            for (i, r) in src_rows.iter().enumerate() {
                let on = selected_source.as_deref() == Some(r.parent.as_str())
                    || selected_source.as_deref() == Some(r.name.as_str());
                let resp = ui.add_sized(
                    [ui.available_width(), chrome::HIT_SIDEBAR],
                    egui::Button::selectable(on, &r.name),
                );
                if resp.clicked() {
                    pick_src = Some(i.to_string());
                }
            }
        });
    if let Some(spec) = pick_src {
        let _ = app.model.select_source(&spec);
        app.set_canvas(Canvas::Editor);
    }
    ui.separator();
    ui.label(RichText::new("Netlist").strong());
    ui.add(
        egui::TextEdit::singleline(&mut app.tree_filter)
            .hint_text("filter")
            .desired_width(f32::INFINITY),
    );
    let filt = app.tree_filter.to_ascii_lowercase();
    let rows: Vec<_> = app
        .model
        .netlist_rows()
        .into_iter()
        .filter(|r| {
            filt.is_empty()
                || r.name.to_ascii_lowercase().contains(&filt)
                || r.type_cell().to_ascii_lowercase().contains(&filt)
                || r.kind.to_ascii_lowercase().contains(&filt)
        })
        .collect();
    if rows.is_empty() {
        ui.label("No netlist yet.");
        if primary_button(ui, "Implement").clicked() {
            app.submit_job(JobKind::Implement);
        }
        return;
    }
    let selected = app.model.selected.clone();
    let selected_netlist = app.model.selected_netlist.clone();
    let mut pick: Option<String> = None;
    let mut pick_obj: Option<String> = None;
    data_scroll("files_netlist").show(ui, |ui| {
        for r in &rows {
            let on = selected_netlist.as_deref() == Some(r.name.as_str())
                || selected.as_deref() == Some(r.name.as_str());
            let resp = ui.add_sized(
                [ui.available_width(), chrome::HIT_SIDEBAR],
                egui::Button::selectable(on, format!("{}  {}", r.name, r.type_cell())),
            );
            if resp.clicked() {
                pick_obj = Some(r.name.clone());
                pick = Some(r.name.clone());
            }
        }
    });
    if let Some(id) = pick_obj {
        let _ = app.model.select_netlist_object(&id);
    } else if let Some(id) = pick {
        let _ = app.model.select_netlist(&id);
    }
}

/// Clickable status bar (P1-5, VS Code style). Every segment is an action:
/// breadcrumb → Files, WNS chip → Timing canvas, error/warning counts →
/// Messages panel, LUTFF → Utilization, run name → Runs tab. Background jobs
/// show a spinner; job completion also toasts (see `toasts.rs`).
/// One status-bar segment (§3.7): 12px mono text, 8px side padding, 1px left
/// divider (except the first segment), hover highlight when clickable.
#[allow(clippy::too_many_arguments)]
fn status_seg(
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    bar: egui::Rect,
    x: &mut f32,
    first: &mut bool,
    idx: usize,
    label: &str,
    color: Color32,
    hover: &str,
    clickable: bool,
) -> egui::Response {
    let font = egui::FontId::new(12.0, egui::FontFamily::Monospace);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, color);
    let sw = galley.size().x + 16.0;
    let rect =
        egui::Rect::from_min_size(egui::pos2(*x, bar.top()), egui::vec2(sw, bar.height()));
    *x += sw;
    if !*first {
        ui.painter().line_segment(
            [
                egui::pos2(rect.left(), bar.top() + 5.0),
                egui::pos2(rect.left(), bar.bottom() - 5.0),
            ],
            egui::Stroke::new(1.0, theme.line),
        );
    }
    *first = false;
    let sense = if clickable {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let resp = ui.interact(rect, egui::Id::new(("statusseg", idx)), sense);
    if clickable && resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, theme.surface_hover);
    }
    ui.painter().galley(
        egui::pos2(
            rect.left() + 8.0,
            bar.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
    if hover.is_empty() {
        resp
    } else {
        resp.on_hover_text(hover)
    }
}

fn paint_status_bar(ctx: &egui::Context, app: &mut HelionIde) {
    let theme = app.theme;
    let model = &app.model;
    let honest = model.timing_honesty_label();
    let provenance = model.constraint_provenance().to_string();
    let wns = if model.timing_closed_wns() {
        honest.clone()
    } else if honest.starts_with("no_body") {
        // Internal token; the status bar shows a human label instead.
        "No design".into()
    } else if honest.starts_with("no_clock_path") {
        "no_clock_path".into()
    } else if honest.contains("default period") {
        honest.clone()
    } else {
        honest.clone()
    };
    // Red when negative (§3.7); green only for genuinely closed timing (status
    // semantics), warn when there is no closed WNS to show.
    let wns_num = model.timing.as_ref().map(|t| t.wns_ps);
    let wns_color = if model.timing_closed_wns() {
        if wns_num.is_some_and(|v| v < 0) {
            theme.danger
        } else {
            theme.ok
        }
    } else {
        theme.warn
    };
    let lutff = model
        .utilization
        .as_ref()
        .map(|u| format!("{}/{}", u.lutff, u.lutff_cap))
        .unwrap_or_else(|| "-".into());
    let run = model
        .runs
        .iter()
        .rev()
        .find(|r| r.status != "Not started")
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "idle".into());
    // Cheap CLI breadcrumb: Activity › Canvas (or workspace canvas label).
    let where_label = match model.workspace.canvas() {
        WorkspaceTab::TextEditor | WorkspaceTab::Device | WorkspaceTab::Reports => {
            app.canvas.label().to_string()
        }
        _ => model.workspace.canvas_label().to_string(),
    };
    let crumb = format!("{} › {}", app.activity.label(), where_label);
    // No-cable crumb on Program rail - never claims board DONE.
    let board_soft_hold = app.activity == Activity::Program
        && app
            .board_detect
            .as_ref()
            .map(|d| !d.physical_had)
            .unwrap_or(true);
    let board_crumb = if board_soft_hold {
        "board:no-cable"
    } else {
        ""
    };
    let errors = model
        .messages
        .iter()
        .filter(|m| m.severity == MsgSeverity::Error)
        .count();
    let warnings = model
        .messages
        .iter()
        .filter(|m| m.severity == MsgSeverity::Warning)
        .count();
    let busy = app.busy;
    let progress = app.progress.clone();
    let status = model.status.clone();

    #[derive(Clone, Copy)]
    enum Go {
        Activity(Activity),
        Tab(WorkspaceTab),
        Bottom(BottomTab),
    }
    let mut go: Option<Go> = None;

    egui::TopBottomPanel::bottom("status")
        .exact_height(chrome::STATUS_HEIGHT)
        .show_separator_line(true)
        .show(ctx, |ui| {
            let w = ui.available_width();
            // Reserve for the right-side job indicator; segments never paint into it.
            let right_w = 190.0;
            let (bar, _) = ui.allocate_exact_size(
                egui::vec2(w, chrome::STATUS_HEIGHT),
                egui::Sense::hover(),
            );
            let x_limit = bar.right() - right_w - 8.0;
            // Segments are spec'd first, then measured, then the visible
            // leading run is painted (chrome::status_visible_count): narrow
            // windows hide low-priority segments instead of overlapping them
            // (VS Code's status bar priority pattern).
            struct SegSpec {
                label: String,
                color: Color32,
                hover: String,
                go: Option<Go>,
            }
            let mut specs: Vec<SegSpec> = Vec::new();
            let mut spec = |label: String, color: Color32, hover: &str, go: Option<Go>| {
                specs.push(SegSpec {
                    label,
                    color,
                    hover: hover.to_string(),
                    go,
                });
            };
            spec(
                crumb.clone(),
                theme.dim,
                "Go to Files (project view)",
                Some(Go::Activity(Activity::Files)),
            );
            // No standalone part picker exists; the Device view is the honest
            // target for the part segment.
            spec(
                model.part().to_string(),
                theme.dim,
                "Open Device view",
                Some(Go::Activity(Activity::Device)),
            );
            spec(
                wns.clone(),
                wns_color,
                "Open Timing canvas (timing summary + paths)",
                Some(Go::Activity(Activity::Timing)),
            );
            spec(
                format!("provenance={provenance}"),
                theme.faint,
                "",
                None,
            );
            let issue_label = if errors + warnings == 0 {
                "0 problems".to_string()
            } else {
                format!("{errors}E {warnings}W")
            };
            let issue_color = if errors > 0 {
                theme.danger
            } else if warnings > 0 {
                theme.warn
            } else {
                theme.dim
            };
            spec(
                issue_label,
                issue_color,
                "Open Messages panel",
                Some(Go::Bottom(BottomTab::Messages)),
            );
            spec(
                format!("LUTFF {lutff}"),
                theme.dim,
                "Open Utilization report",
                Some(Go::Tab(WorkspaceTab::Utilization)),
            );
            spec(
                run.clone(),
                theme.dim,
                "Open Runs tab",
                Some(Go::Tab(WorkspaceTab::Runs)),
            );
            if !board_crumb.is_empty() {
                spec(board_crumb.to_string(), theme.faint, "", None);
            }
            if !progress.is_empty() {
                spec(progress.clone(), theme.accent_dim, "", None);
            } else if status != "idle" && !status.is_empty() && !status.ends_with(": ok") {
                spec(status.clone(), theme.dim, "", None);
            }
            let font = egui::FontId::new(12.0, egui::FontFamily::Monospace);
            let widths: Vec<f32> = specs
                .iter()
                .map(|sp| {
                    ui.painter()
                        .layout_no_wrap(sp.label.clone(), font.clone(), sp.color)
                        .size()
                        .x
                        + 16.0
                })
                .collect();
            let n_visible = chrome::status_visible_count(&widths, x_limit);
            let mut x = bar.left() + 8.0;
            let mut first = true;
            for (idx, sp) in specs.iter().take(n_visible).enumerate() {
                if status_seg(
                    ui,
                    &theme,
                    bar,
                    &mut x,
                    &mut first,
                    idx + 1,
                    &sp.label,
                    sp.color,
                    &sp.hover,
                    sp.go.is_some(),
                )
                .clicked()
                {
                    go = sp.go;
                }
            }
            // Right side: background-job spinner + progress, or idle indicator.
            let right_rect = egui::Rect::from_min_size(
                egui::pos2(bar.right() - right_w - 8.0, bar.top()),
                egui::vec2(right_w, bar.height()),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(right_rect), |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if busy {
                        let ptxt = if progress.is_empty() {
                            "working".to_string()
                        } else {
                            progress.clone()
                        };
                        ui.label(
                            RichText::new(ptxt)
                                .monospace()
                                .size(12.0)
                                .color(theme.accent_dim),
                        );
                        ui.spinner();
                    } else {
                        ui.label(
                            RichText::new("✓ idle")
                                .monospace()
                                .size(12.0)
                                .color(theme.faint),
                        );
                    }
                });
            });
        });

    match go {
        Some(Go::Activity(a)) => app.set_activity(a),
        Some(Go::Tab(t)) => open_more_workspace(app, t),
        Some(Go::Bottom(b)) => app.model.bottom_tab = b,
        None => {}
    }
}
fn paint_properties(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.separator();
    ui.label(RichText::new("Properties").strong());
    if let Some(net) = model.selected_debug_net() {
        ui.horizontal(|ui| {
            if ui
                .button(format!("Mark Debug {net}"))
                .on_hover_text("UG908 Mark Debug")
                .clicked()
            {
                let _ = model.exec("mark_debug");
            }
            if ui
                .button("Add Probe")
                .on_hover_text("UG908 Add Probe → ILA")
                .clicked()
            {
                let _ = model.exec("add_probe");
            }
        });
    }
    let rows = model.property_rows();
    let selected = model.selected_property.clone();
    let mut pick: Option<String> = None;
    data_scroll("properties_table_scroll").show(ui, |ui| {
        egui::Grid::new("properties_table")
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Value").strong());
                ui.end_row();
                if rows.is_empty() {
                    ui.label("No properties.");
                    ui.end_row();
                } else {
                    for (i, r) in rows.iter().enumerate() {
                        let on = selected.as_deref() == Some(r.name.as_str());
                        if ui.selectable_label(on, &r.name).clicked() {
                            pick = Some(i.to_string());
                        }
                        if ui.selectable_label(on, &r.value).clicked() {
                            pick = Some(i.to_string());
                        }
                        ui.end_row();
                    }
                }
            });
    });
    if let Some(spec) = pick {
        let _ = model.select_property(&spec);
    }
}

fn apply_shared_chrome(ctx: &egui::Context, app: &mut HelionIde) {
    let r = ctx.screen_rect();
    let share = chrome::share_available(r.width(), r.height());
    let size = [r.width(), r.height()];
    if app.last_share[0] == 0.0 && app.last_share[1] == 0.0 {
        // First frame: seed from the real window size. This is NOT a resize -
        // the constructor may have set sizes from HELION_* env hooks, which a
        // reset-to-default here would silently clobber.
        app.last_share = size;
        app.sidebar_width = app
            .sidebar_width
            .clamp(share.sidebar_floor, share.sidebar_cap);
        app.console_height = app
            .console_height
            .clamp(share.console_floor, share.console_cap);
    } else if (app.last_share[0] - size[0]).abs() > 1.0 || (app.last_share[1] - size[1]).abs() > 1.0 {
        app.sidebar_width = share.sidebar_w;
        app.console_height = share.console_h;
        app.last_share = size;
        app.force_share = true;
    } else {
        app.sidebar_width = app
            .sidebar_width
            .clamp(share.sidebar_floor, share.sidebar_cap);
        app.console_height = app
            .console_height
            .clamp(share.console_floor, share.console_cap);
    }
}

fn paint_bottom(ctx: &egui::Context, app: &mut HelionIde) {
    let share = chrome::share_available(ctx.screen_rect().width(), ctx.screen_rect().height());
    if !app.force_share && ctx.screen_rect().height() < 540.0 {
        // Short window: clamp toward the default share instead of pinning every
        // frame, so the resize handle keeps working.
        app.console_height = app.console_height.min(share.console_h);
    }
    let mut console = egui::TopBottomPanel::bottom("console")
        .resizable(true)
        .default_height(app.console_height)
        .min_height(share.console_floor)
        .max_height(share.console_cap);
    if app.force_share {
        // Re-share frame (window resized, or first frame): snap to the current
        // console_height once. apply_shared_chrome already reset it to the
        // default share on real resizes, and the first frame honors any
        // HELION_CONSOLE_HEIGHT override. The flag is cleared at the end of
        // the frame, so it can never get stuck on and pin the panel - a stuck
        // flag is what made the user's drag snap back / go dead.
        console = console.exact_height(app.console_height);
    }
    let inner = console.show_separator_line(true).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let console_on =
                app.model.bottom_tab == BottomTab::Tcl || app.model.bottom_tab == BottomTab::Log;
            let c = ui.add_sized(
                [88.0, chrome::HIT_SIDEBAR],
                egui::Button::selectable(console_on, "Console"),
            );
            if c.clicked() {
                app.model.bottom_tab = BottomTab::Tcl;
            }
            let m = ui.add_sized(
                [96.0, chrome::HIT_SIDEBAR],
                egui::Button::selectable(app.model.bottom_tab == BottomTab::Messages, "Messages"),
            );
            if m.clicked() {
                app.model.bottom_tab = BottomTab::Messages;
            }
            // Note: the "Console" tab above IS the inline Tcl console
            // (BottomTab::Tcl). There is deliberately no separate "Tcl" tab:
            // it used to pop a floating dialog, which users did not want. The
            // floating Tcl window remains on Cmd+Backtick.
            if app.activity == Activity::Simulate {
                let s = ui.add_sized(
                    [88.0, chrome::HIT_SIDEBAR],
                    egui::Button::selectable(app.model.bottom_tab == BottomTab::SimLog, "Sim log"),
                );
                if s.clicked() {
                    app.model.bottom_tab = BottomTab::SimLog;
                }
            }
        });
        match app.model.bottom_tab {
            BottomTab::Tcl | BottomTab::Log => paint_tcl_console(ui, app),
            BottomTab::Messages => paint_messages(ui, &mut app.model, &app.theme),
            BottomTab::SimLog => paint_sim_log(ui, &mut app.model, &app.theme),
        }
    });
    app.console_height = inner
        .response
        .rect
        .height()
        .clamp(share.console_floor, share.console_cap);
}

fn paint_tcl_window(ctx: &egui::Context, app: &mut HelionIde) {
    if !app.show_tcl {
        return;
    }
    let mut open = app.show_tcl;
    egui::Window::new("Tcl")
        .open(&mut open)
        .default_width(520.0)
        .default_height(240.0)
        .show(ctx, |ui| {
            paint_tcl_console(ui, app);
        });
    app.show_tcl = open;
}

fn pick_rtl_dialog() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Add Sources")
        .add_filter("RTL", &["sv", "v", "vhd", "vhdl"])
        .pick_file()
}

fn pick_constraint_dialog() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Add Constraints")
        .add_filter("Constraints", &["sdc", "xdc"])
        .pick_file()
}

fn pick_directory_dialog(start: &str) -> Option<PathBuf> {
    let mut d = rfd::FileDialog::new().set_title("Project Directory");
    let start_pb = PathBuf::from(start);
    if start_pb.is_dir() {
        d = d.set_directory(start_pb);
    }
    d.pick_folder()
}

fn paint_create_project_wizard(ctx: &egui::Context, app: &mut HelionIde) {
    let theme = app.theme;
    if !app.show_create_project {
        return;
    }
    let mut open = app.show_create_project;
    let mut finish = false;
    let mut cancel = false;
    egui::Window::new("Create Project")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .show(ctx, |ui| {
            let step = app.create_wizard.step.min(CREATE_WIZARD_STEPS.len() - 1);
            ui.label(
                RichText::new(format!(
                    "Step {} of {}: {}",
                    step + 1,
                    CREATE_WIZARD_STEPS.len(),
                    CREATE_WIZARD_STEPS[step]
                ))
                .strong(),
            );
            ui.separator();

            match step {
                0 => {
                    ui.label("Project name");
                    ui.add(
                        egui::TextEdit::singleline(&mut app.create_wizard.name)
                            .desired_width(360.0)
                            .hint_text("project_1"),
                    );
                    ui.add_space(8.0);
                    ui.label("Project directory");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut app.create_wizard.directory)
                                .desired_width(320.0)
                                .hint_text("/tmp/helion-projects"),
                        );
                        if ui.button("Browse…").clicked() {
                            if let Some(p) = pick_directory_dialog(&app.create_wizard.directory) {
                                app.create_wizard.directory = p.display().to_string();
                            }
                        }
                    });
                    ui.label(
                        RichText::new("Creates <dir>/<name>/<name>.prj")
                            .small()
                            .color(theme.dim),
                    );
                }
                1 => {
                    ui.label("Part");
                    egui::ComboBox::from_id_salt("create_wizard_part")
                        .selected_text(&app.create_wizard.part)
                        .show_ui(ui, |ui| {
                            for p in CREATE_WIZARD_PARTS {
                                ui.selectable_value(
                                    &mut app.create_wizard.part,
                                    (*p).to_string(),
                                    *p,
                                );
                            }
                        });
                    ui.label(
                        RichText::new("Default: HL10T-C32-1")
                            .small()
                            .color(theme.dim),
                    );
                }
                2 => {
                    ui.label("RTL sources (.sv / .v / .vhd)");
                    let mut remove = None;
                    for (i, s) in app.create_wizard.sources.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(s);
                            if ui.small_button("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove {
                        app.create_wizard.sources.remove(i);
                    }
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut app.create_wizard.source_draft)
                                .desired_width(300.0)
                                .hint_text("Absolute path to .sv"),
                        );
                        if ui.button("Add").clicked() {
                            let p = app.create_wizard.source_draft.trim().to_string();
                            if !p.is_empty() && !app.create_wizard.sources.contains(&p) {
                                app.create_wizard.sources.push(p);
                                app.create_wizard.source_draft.clear();
                                app.create_wizard.error = None;
                            }
                        }
                        if ui.button("Browse…").clicked() {
                            if let Some(p) = pick_rtl_dialog() {
                                let s = p.display().to_string();
                                if !app.create_wizard.sources.contains(&s) {
                                    app.create_wizard.sources.push(s);
                                }
                                app.create_wizard.error = None;
                            }
                        }
                    });
                }
                3 => {
                    ui.label("Constraints (.sdc / .xdc)");
                    let mut remove = None;
                    for (i, s) in app.create_wizard.constraints.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(s);
                            if ui.small_button("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove {
                        app.create_wizard.constraints.remove(i);
                    }
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut app.create_wizard.constraint_draft)
                                .desired_width(300.0)
                                .hint_text("Absolute path to .sdc"),
                        );
                        if ui.button("Add").clicked() {
                            let p = app.create_wizard.constraint_draft.trim().to_string();
                            if !p.is_empty() && !app.create_wizard.constraints.contains(&p) {
                                app.create_wizard.constraints.push(p);
                                app.create_wizard.constraint_draft.clear();
                                app.create_wizard.error = None;
                            }
                        }
                        if ui.button("Browse…").clicked() {
                            if let Some(p) = pick_constraint_dialog() {
                                let s = p.display().to_string();
                                if !app.create_wizard.constraints.contains(&s) {
                                    app.create_wizard.constraints.push(s);
                                }
                                app.create_wizard.error = None;
                            }
                        }
                    });
                }
                _ => {
                    ui.label(RichText::new("Summary").strong());
                    ui.monospace(format!("Name: {}", app.create_wizard.name));
                    ui.monospace(format!("Directory: {}", app.create_wizard.directory));
                    ui.monospace(format!("Part: {}", app.create_wizard.part));
                    ui.monospace(format!("Sources: {}", app.create_wizard.sources.len()));
                    for s in &app.create_wizard.sources {
                        ui.label(format!("  • {s}"));
                    }
                    ui.monospace(format!(
                        "Constraints: {}",
                        app.create_wizard.constraints.len()
                    ));
                    for s in &app.create_wizard.constraints {
                        ui.label(format!("  • {s}"));
                    }
                    ui.label(
                        RichText::new("Finish writes the .prj and opens it in the IDE.")
                            .small()
                            .color(theme.dim),
                    );
                }
            }

            if let Some(err) = &app.create_wizard.error {
                ui.colored_label(theme.danger, err);
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let last = step + 1 >= CREATE_WIZARD_STEPS.len();
                    if last {
                        if primary_button(ui, "Finish").clicked() {
                            finish = true;
                        }
                    } else if ui.button("Next").clicked() {
                        // Validate before advancing.
                        let w = &mut app.create_wizard;
                        w.error = None;
                        match step {
                            0 => {
                                if w.name.trim().is_empty() {
                                    w.error = Some("Enter a project name.".into());
                                } else if w.directory.trim().is_empty() {
                                    w.error = Some("Enter a project directory.".into());
                                } else {
                                    w.step += 1;
                                }
                            }
                            1 => {
                                if w.part.trim().is_empty() {
                                    w.part = "HL10T-C32-1".into();
                                }
                                w.step += 1;
                            }
                            2 => {
                                if w.sources.is_empty() {
                                    w.error = Some("Add at least one RTL source.".into());
                                } else {
                                    w.step += 1;
                                }
                            }
                            3 => {
                                w.step += 1;
                            }
                            _ => {}
                        }
                    }
                    if step > 0 && ui.button("Back").clicked() {
                        app.create_wizard.error = None;
                        app.create_wizard.step = step.saturating_sub(1);
                    }
                });
            });
        });

    if cancel || !open {
        app.show_create_project = false;
        return;
    }
    app.show_create_project = open;

    if finish {
        let w = &app.create_wizard;
        let name = w.name.trim().to_string();
        let dir = PathBuf::from(w.directory.trim());
        let part = w.part.clone();
        let sources: Vec<PathBuf> = w.sources.iter().map(PathBuf::from).collect();
        let constraints: Vec<PathBuf> = w.constraints.iter().map(PathBuf::from).collect();
        match app
            .model
            .create_project(&name, &dir, &part, &sources, &constraints)
        {
            Ok(msg) => {
                let prj = dir.join(&name).join(format!("{name}.prj"));
                app.remember(prj);
                app.set_activity(Activity::Files);
                app.show_create_project = false;
                app.create_wizard.error = None;
                let _ = msg;
            }
            Err(e) => {
                app.create_wizard.error = Some(e);
            }
        }
    }
}

fn paint_examples_popup(ctx: &egui::Context, app: &mut HelionIde) {
    if !app.show_examples {
        return;
    }
    let mut open = app.show_examples;
    let mut pick = None;
    egui::Window::new("Examples")
        .open(&mut open)
        .show(ctx, |ui| {
            for (label, file) in RAIL_OPEN_SOURCES {
                if ui.button(label).clicked() {
                    pick = Some(file);
                }
            }
        });
    app.show_examples = open;
    if let Some(file) = pick {
        let p = helion_device::Device::examples_dir().join(file);
        app.open_path_async(&p);
        app.show_examples = false;
    }
}

fn paint_workspace(ui: &mut egui::Ui, app: &mut HelionIde) {
    // Always: Editor | Device | Timing | More ... (overflow keeps prior WorkspaceTab destinations).
    // Scrolls horizontally at narrow widths (VS Code pattern) so tabs never
    // overlap neighboring chrome.
    egui::ScrollArea::horizontal()
        .id_salt("workspace_tabs")
        .auto_shrink([false, true])
        .show(ui, |ui| {
    ui.horizontal(|ui| {
        for c in Canvas::ALL {
            // Reports rail owns the catalog view - don't paint it as "Timing" selected (Timing ≠ Reports).
            let on = app.canvas == c
                && !chrome::is_more_destination(app.model.workspace)
                && !(app.activity == Activity::Reports && c == Canvas::Timing);
            if ui
                .selectable_label(on, format!("{}  {}", c.label(), c.shortcut()))
                .on_hover_text(tip(c.label(), c.shortcut(), ""))
                .clicked()
            {
                match c {
                    Canvas::Editor => app.set_activity(Activity::Files),
                    Canvas::Device => app.set_activity(Activity::Device),
                    Canvas::Timing => app.set_activity(Activity::Timing),
                }
            }
        }
        if app.activity == Activity::Reports {
            let _ = ui.selectable_label(true, "Reports");
        }
        if chrome::is_more_destination(app.model.workspace) {
            let _ = ui.selectable_label(true, app.model.workspace.label());
        }
        ui.menu_button(chrome::MORE, |ui| {
            ui.label(RichText::new("More views").strong().small());
            ui.separator();
            for tab in WorkspaceTab::ALL {
                if tab.is_canvas() {
                    continue;
                }
                let on = app.model.workspace == tab;
                if ui.selectable_label(on, tab.label()).clicked() {
                    open_more_workspace(app, tab);
                    ui.close();
                }
            }
        });
        });
    });
    ui.separator();
    if app.model.workspace.sim_only() || app.activity == Activity::Simulate {
        paint_sim_workspace(ui, app);
        return;
    }
    if chrome::is_more_destination(app.model.workspace) {
        paint_more_pane(ui, app);
        return;
    }
    if app.activity == Activity::Program {
        paint_hw(ui, app);
        return;
    }
    match app.canvas {
        Canvas::Editor => {
            if app.model.tree.sources.is_empty() && app.model.source_line_rows().is_empty() {
                paint_empty_editor(ui, app);
            } else {
                let theme = app.theme;
                editor::paint_editor(ui, app, &theme);
            }
        }
        Canvas::Device => {
            paint_device(
                ui,
                &mut app.model,
                &app.theme,
                &mut app.physview,
                &mut app.device_zoom_target,
            );
        }
        Canvas::Timing => {
            // Full-width vertical stack. No SidePanel twin, no set_min_size (that created a tall black hole).
            egui::ScrollArea::vertical()
                .id_salt("timing_canvas_v6")
                .auto_shrink([false, true])
                .hscroll(false)
                .show(ui, |ui| match app.activity {
                    Activity::Reports => {
                        paint_reports_detail(ui, app);
                    }
                    _ => paint_timing_only(ui, &mut app.model, &app.theme),
                });
        }
    }
}

fn paint_sim_workspace(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    // Absolute rect split - clip both sides so long HNF names cannot paint over Memory.
    let full = ui.available_rect_before_wrap();
    let h = full.height().max(200.0);
    // Proportional nav share (VS Code GridView pattern): a fixed 220px nav
    // left the wave pane with a negative width on narrow windows.
    let (nav_w, sep_w, wave_w) = chrome::sim_split(full.width());
    let _ = ui.allocate_rect(full, Sense::hover());
    let nav_rect = egui::Rect::from_min_size(full.min, egui::vec2(nav_w, h));
    let sep_rect = egui::Rect::from_min_size(
        egui::pos2(full.min.x + nav_w, full.min.y),
        egui::vec2(sep_w, h),
    );
    let wave_left = full.min.x + nav_w + sep_w;
    let wave_rect = egui::Rect::from_min_size(
        egui::pos2(wave_left, full.min.y),
        egui::vec2(wave_w, h),
    );
    ui.painter().rect_filled(nav_rect, 0.0, theme.panel);
    ui.painter().rect_filled(sep_rect, 0.0, theme.line);
    ui.painter().rect_filled(wave_rect, 0.0, theme.die_bg);
    ui.scope_builder(egui::UiBuilder::new().max_rect(nav_rect), |ui| {
        ui.set_clip_rect(nav_rect);
        ui.set_min_size(nav_rect.size());
        ui.set_max_width(nav_rect.width());
        paint_sim_nav_body(ui, &mut app.model, &app.theme);
    });
    ui.scope_builder(egui::UiBuilder::new().max_rect(wave_rect), |ui| {
        ui.set_clip_rect(wave_rect);
        ui.set_min_size(wave_rect.size());
        match pane_for_workspace(app.model.workspace) {
            WorkspacePane::Wave => paint_wave(ui, &mut app.model, &app.theme),
            WorkspacePane::Ila => {
                paint_ila(ui, &mut app.model, &mut app.ila_rx, &mut app.toasts, &app.theme)
            }
            WorkspacePane::Memory => paint_memory(ui, &mut app.model, &app.theme),
            WorkspacePane::Breakpoints => paint_breakpoints(ui, &mut app.model, &app.theme),
            WorkspacePane::Locals => paint_locals(ui, &mut app.model),
            WorkspacePane::Forces => paint_forces(ui, &mut app.model, &app.theme),
            WorkspacePane::SimSettings => paint_sim_settings(ui, &mut app.model),
            WorkspacePane::Source => paint_source(ui, &mut app.model, &app.theme),
            WorkspacePane::Testbench => paint_testbench(ui, app),
            _ => paint_wave(ui, &mut app.model, &app.theme),
        }
    });
}

fn open_more_workspace(app: &mut HelionIde, tab: WorkspaceTab) {
    let mut s = app.snapshot();
    apply_more(&mut s, tab);
    app.restore(s);
    let pane = central_pane(&app.snapshot());
    let expected = pane_for_workspace(tab);
    app.log_click(tab.label(), &format!("{expected:?}"));
    debug_assert_eq!(
        pane,
        expected,
        "More {} updated workspace but painted {pane:?}",
        tab.label()
    );
}

/// More ... destinations fill the central pane with their real UI - never Timing-only, never a stub heading.
fn paint_more_pane(ui: &mut egui::Ui, app: &mut HelionIde) {
    match pane_for_workspace(app.model.workspace) {
        WorkspacePane::Schematic => paint_schematic(ui, &mut app.model, &app.theme),
        WorkspacePane::Package => paint_package(ui, &mut app.model, &app.theme),
        WorkspacePane::Hierarchy => paint_hierarchy(ui, &mut app.model, &app.theme),
        WorkspacePane::Bitstream => paint_bitstream(ui, &mut app.model, &app.theme),
        WorkspacePane::Hardware => paint_hw(ui, app),
        WorkspacePane::Ip => paint_ip(ui, &mut app.model, &app.theme),
        WorkspacePane::Find => paint_find(ui, &mut app.model, &app.theme),
        WorkspacePane::Settings => paint_project_settings(ui, &mut app.model, &app.theme),
        WorkspacePane::Summary => dashboard::paint_dashboard(ui, app),
        WorkspacePane::Constraints => paint_constraints_tab(ui, app),
        WorkspacePane::ClockInteraction => paint_clock_interaction(ui, &mut app.model, &app.theme),
        WorkspacePane::Cdc => paint_cdc(ui, &mut app.model, &app.theme),
        WorkspacePane::ClockNetworks => paint_clock_networks(ui, &mut app.model),
        WorkspacePane::Power => paint_power(ui, &mut app.model, &app.theme),
        WorkspacePane::Methodology => paint_methodology(ui, &mut app.model, &app.theme),
        WorkspacePane::Qor => paint_qor(ui, &mut app.model, &app.theme),
        WorkspacePane::Drc => paint_drc(ui, &mut app.model, &app.theme),
        WorkspacePane::Utilization => paint_utilization(ui, &mut app.model, &app.theme),
        WorkspacePane::Runs => paint_runs(ui, app),
        WorkspacePane::Checkpoints => paint_checkpoints(ui, &mut app.model, &app.theme),
        WorkspacePane::Testbench => paint_testbench(ui, app),
        WorkspacePane::Wave
        | WorkspacePane::Ila
        | WorkspacePane::Source
        | WorkspacePane::Memory
        | WorkspacePane::Breakpoints
        | WorkspacePane::Locals
        | WorkspacePane::Forces
        | WorkspacePane::SimSettings => paint_sim_workspace(ui, app),
        WorkspacePane::ReportsCatalog => paint_reports_detail(ui, app),
        WorkspacePane::Editor => {
            let theme = app.theme;
            editor::paint_editor(ui, app, &theme)
        }
        WorkspacePane::Device => paint_device(
            ui,
            &mut app.model,
            &app.theme,
            &mut app.physview,
            &mut app.device_zoom_target,
        ),
        WorkspacePane::Timing => paint_timing_only(ui, &mut app.model, &app.theme),
    }
}

/// Timing ⌘3 / Timing rail: WNS + paths only - never Reports catalog.
fn paint_timing_only(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    paint_timing_summary(ui, model, theme);
    ui.add_space(8.0);
    paint_timing_paths(ui, model, theme);
}

/// Reports rail: selected report body only (catalog is in the SidePanel). No Timing twin chrome.
fn paint_reports_detail(ui: &mut egui::Ui, app: &mut HelionIde) {
    match app.model.workspace {
        WorkspaceTab::Constraints => paint_constraints_tab(ui, app),
        WorkspaceTab::ClockInteraction => paint_clock_interaction(ui, &mut app.model, &app.theme),
        WorkspaceTab::Cdc => paint_cdc(ui, &mut app.model, &app.theme),
        WorkspaceTab::ClockNetworks => paint_clock_networks(ui, &mut app.model),
        WorkspaceTab::Power => paint_power(ui, &mut app.model, &app.theme),
        WorkspaceTab::Methodology => paint_methodology(ui, &mut app.model, &app.theme),
        WorkspaceTab::Qor => paint_qor(ui, &mut app.model, &app.theme),
        WorkspaceTab::Drc => paint_drc(ui, &mut app.model, &app.theme),
        WorkspaceTab::Utilization => paint_utilization(ui, &mut app.model, &app.theme),
        WorkspaceTab::Runs => paint_runs(ui, app),
        WorkspaceTab::Checkpoints => paint_checkpoints(ui, &mut app.model, &app.theme),
        WorkspaceTab::Summary => dashboard::paint_dashboard(ui, app),
        WorkspaceTab::Reports => {
            // Sidebar owns the catalog (MUST: no dual full tables). Center is the first report.
            paint_timing_summary(ui, &mut app.model, &app.theme);
        }
        other => {
            // Fall through to known panes / timing-only for overflow More picks.
            let _ = other;
            paint_timing_canvas_body(ui, app);
        }
    }
}

fn paint_timing_canvas_body(ui: &mut egui::Ui, app: &mut HelionIde) {
    match app.model.workspace {
        WorkspaceTab::Constraints => paint_constraints_tab(ui, app),
        WorkspaceTab::ClockInteraction => paint_clock_interaction(ui, &mut app.model, &app.theme),
        WorkspaceTab::Cdc => paint_cdc(ui, &mut app.model, &app.theme),
        WorkspaceTab::ClockNetworks => paint_clock_networks(ui, &mut app.model),
        WorkspaceTab::Power => paint_power(ui, &mut app.model, &app.theme),
        WorkspaceTab::Methodology => paint_methodology(ui, &mut app.model, &app.theme),
        WorkspaceTab::Qor => paint_qor(ui, &mut app.model, &app.theme),
        WorkspaceTab::Drc => paint_drc(ui, &mut app.model, &app.theme),
        WorkspaceTab::Utilization => paint_utilization(ui, &mut app.model, &app.theme),
        WorkspaceTab::Runs => paint_runs(ui, app),
        WorkspaceTab::Checkpoints => paint_checkpoints(ui, &mut app.model, &app.theme),
        WorkspaceTab::Schematic => paint_schematic(ui, &mut app.model, &app.theme),
        WorkspaceTab::Package => paint_package(ui, &mut app.model, &app.theme),
        WorkspaceTab::Hierarchy => paint_hierarchy(ui, &mut app.model, &app.theme),
        WorkspaceTab::Bitstream => paint_bitstream(ui, &mut app.model, &app.theme),
        WorkspaceTab::Hardware => paint_hw(ui, app),
        WorkspaceTab::Ip => paint_ip(ui, &mut app.model, &app.theme),
        WorkspaceTab::Find => paint_find(ui, &mut app.model, &app.theme),
        WorkspaceTab::Settings => paint_project_settings(ui, &mut app.model, &app.theme),
        WorkspaceTab::Summary => dashboard::paint_dashboard(ui, app),
        _ => {
            paint_timing_summary(ui, &mut app.model, &app.theme);
            ui.add_space(8.0);
            // Paths stay reachable: Timing nav lists them; detail still shows pin delay table.
            paint_timing_paths(ui, &mut app.model, &app.theme);
        }
    }
}

fn paint_empty_editor(ui: &mut egui::Ui, app: &mut HelionIde) {
    ui.vertical_centered(|ui| {
        ui.add_space(48.0);
        ui.label(RichText::new("No sources yet.").size(16.0));
        ui.add_space(8.0);
        if primary_button(ui, "New Project…")
            .on_hover_text(tip("New Project", "", "create_project"))
            .clicked()
        {
            app.create_wizard = CreateProjectWizard::default();
            app.show_create_project = true;
        }
        ui.add_space(6.0);
        if primary_button(ui, "Open HDL…")
            .on_hover_text(tip("Open", "⌘O", "open_source"))
            .clicked()
        {
            native_open(app);
        }
        ui.add_space(6.0);
        if primary_button(ui, "Examples")
            .on_hover_text("Open an example HDL file")
            .clicked()
        {
            app.show_examples = true;
        }
    });
}

fn paint_clipped_select(
    ui: &mut egui::Ui,
    on: bool,
    shown: &str,
    tip: &str,
    theme: &theme::Theme,
) -> bool {
    let w = ui.available_width().max(16.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 20.0), Sense::click());
    let resp = resp.on_hover_text(tip);
    if ui.is_rect_visible(rect) {
        let p = ui.painter().with_clip_rect(rect);
        if on {
            p.rect_filled(rect, 2.0, theme.sel);
        }
        p.text(
            egui::pos2(rect.left() + 4.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            shown,
            egui::FontId::monospace(12.0),
            theme.text,
        );
    }
    resp.clicked()
}

fn paint_sim_nav_body(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.add_space(4.0);
    let on_tb = model.workspace == WorkspaceTab::Testbench;
    if ui
        .selectable_label(on_tb, RichText::new("Testbenches").strong())
        .on_hover_text("Integrated testbench runner")
        .clicked()
    {
        model.workspace = WorkspaceTab::Testbench;
    }
    ui.separator();
    ui.label(RichText::new("Scopes").strong());
    ui.horizontal_wrapped(|ui| {
        let n = model.sim_runtime_cycles.max(1);
        if ui.button(format!("Run {n}")).clicked() {
            surface::request_job(JobKind::SimRun(n));
        }
        if ui.button("Step").clicked() {
            let _ = model.sim_step();
        }
        if ui.button("Restart").clicked() {
            let _ = model.sim_restart();
        }
        // (Sim settings lives in the Panes row below; a duplicate "Settings"
        // button here used to exec the same destination.)
    });
    let scopes = model.scope_rows().to_vec();
    let selected_scope = model.selected_scope.clone();
    let mut pick_scope = None;
    let row_h = 22.0;
    ui.label(RichText::new("Name").small().weak());
    egui::ScrollArea::vertical()
        .id_salt("ug900_scopes")
        .max_height(160.0)
        .show_rows(ui, row_h, scopes.len().max(1), |ui, range| {
            if scopes.is_empty() {
                ui.label("No scopes yet.");
                return;
            }
            let end = range.end.min(scopes.len());
            for (i, s) in scopes[range.start..end].iter().enumerate() {
                let i = range.start + i;
                let on = selected_scope.as_deref() == Some(s.name.as_str());
                let shown = format!(
                    "{}  {}",
                    helion_gui::schematic_short_name(&s.name),
                    s.type_cell()
                );
                if paint_clipped_select(ui, on, &shown, &s.name, theme) {
                    pick_scope = Some(i.to_string());
                }
            }
        });
    if let Some(spec) = pick_scope {
        let _ = model.select_scope(&spec);
    }
    ui.separator();
    ui.label(RichText::new("Objects").strong());
    let objects = model.object_rows().to_vec();
    let selected_object = model.selected_object.clone();
    let mut pick_obj = None;
    let obj_h = ui.available_height().max(80.0) - 72.0;
    egui::ScrollArea::vertical()
        .id_salt("ug900_objects")
        .max_height(obj_h.max(80.0))
        .show_rows(ui, row_h, objects.len().max(1), |ui, range| {
            if objects.is_empty() {
                ui.label("No objects. Select a scope.");
                return;
            }
            let end = range.end.min(objects.len());
            for (i, o) in objects[range.start..end].iter().enumerate() {
                let i = range.start + i;
                let on = selected_object.as_deref() == Some(o.name.as_str());
                let shown = format!(
                    "{}  {}  {}",
                    helion_gui::schematic_short_name(&o.name),
                    o.type_cell(),
                    o.value_cell()
                );
                if paint_clipped_select(ui, on, &shown, &o.name, theme) {
                    pick_obj = Some(i.to_string());
                }
            }
        });
    if let Some(spec) = pick_obj {
        let _ = model.select_object(&spec);
    }
    ui.separator();
    ui.label(RichText::new("Panes").strong());
    ui.horizontal_wrapped(|ui| {
        if ui.button("Wave").clicked() {
            model.workspace = WorkspaceTab::Wave;
        }
        if ui.button("Source").clicked() {
            let _ = model.open_source_window();
        }
        if ui.button("Memory").clicked() {
            let _ = model.open_memory();
        }
        if ui.button("Breakpoints").clicked() {
            let _ = model.open_breakpoints();
        }
        if ui.button("Locals").clicked() {
            model.workspace = WorkspaceTab::Locals;
        }
        if ui.button("Force").clicked() {
            let _ = model.open_forces();
        }
        if ui.button("Settings").clicked() {
            model.workspace = WorkspaceTab::SimSettings;
        }
    });
}

fn msg_severity_color(theme: &theme::Theme, sev: MsgSeverity) -> Color32 {
    match sev {
        MsgSeverity::Error => theme.danger,
        MsgSeverity::Warning => theme.warn,
        MsgSeverity::Info => theme.info,
    }
}

fn paint_messages(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    let n_err = model
        .messages
        .iter()
        .filter(|m| m.severity == MsgSeverity::Error)
        .count();
    let n_warn = model
        .messages
        .iter()
        .filter(|m| m.severity == MsgSeverity::Warning)
        .count();
    let n_info = model
        .messages
        .iter()
        .filter(|m| m.severity == MsgSeverity::Info)
        .count();
    ui.horizontal(|ui| {
        if ui
            .selectable_label(
                model.message_filter.is_none(),
                format!("All {}", model.messages.len()),
            )
            .clicked()
        {
            let _ = model.filter_messages("all");
        }
        if ui
            .selectable_label(
                model.message_filter == Some(MsgSeverity::Error),
                format!("Errors {n_err}"),
            )
            .clicked()
        {
            let _ = model.filter_messages("error");
        }
        if ui
            .selectable_label(
                model.message_filter == Some(MsgSeverity::Warning),
                format!("Warnings {n_warn}"),
            )
            .clicked()
        {
            let _ = model.filter_messages("warning");
        }
        if ui
            .selectable_label(
                model.message_filter == Some(MsgSeverity::Info),
                format!("Info {n_info}"),
            )
            .clicked()
        {
            let _ = model.filter_messages("info");
        }
    });
    let selected = model.selected_message;
    let rows: Vec<(usize, helion_gui::IdeMessage, String)> = model
        .message_rows()
        .into_iter()
        .map(|(i, m)| {
            let objs = model.extract_design_objects(&m.text);
            let cell = if objs.is_empty() {
                "-".into()
            } else {
                objs.join(",")
            };
            (i, m.clone(), cell)
        })
        .collect();
    let mut pick: Option<usize> = None;
    let mut pick_obj: Option<usize> = None;
    egui::ScrollArea::both()
        .id_salt("messages_scroll")
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("messages_table")
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("#").strong());
                    ui.label(RichText::new("Severity").strong());
                    ui.label(RichText::new("ID").strong());
                    ui.label(RichText::new("Objects").strong());
                    ui.label(RichText::new("Message").strong());
                    ui.end_row();
                    if rows.is_empty() {
                        ui.label("-");
                        ui.label("-");
                        ui.label("-");
                        ui.label("-");
                        ui.label("No messages.");
                        ui.end_row();
                    } else {
                        for (i, m, obj) in &rows {
                            let on = selected == Some(*i);
                            let fill = msg_severity_color(theme, m.severity);
                            ui.monospace(i.to_string());
                            let btn = egui::Button::new(
                                RichText::new(m.severity.tag()).color(Color32::BLACK),
                            )
                            .fill(fill)
                            .selected(on);
                            if ui.add(btn).clicked() {
                                pick = Some(*i);
                            }
                            if ui.selectable_label(on, &m.id).clicked() {
                                pick = Some(*i);
                            }
                            if ui.selectable_label(on, obj).clicked() {
                                if obj.as_str() == "-" {
                                    pick = Some(*i);
                                } else {
                                    pick_obj = Some(*i);
                                }
                            }
                            if ui.selectable_label(on, &m.text).clicked() {
                                pick = Some(*i);
                            }
                            ui.end_row();
                        }
                    }
                });
        });
    if let Some(i) = pick_obj {
        let _ = model.select_message_object(&i.to_string());
    } else if let Some(i) = pick {
        let _ = model.select_message(&i.to_string());
    }
}

fn log_status_color(theme: &theme::Theme, ok: bool) -> Color32 {
    if ok {
        theme.ok
    } else {
        theme.danger
    }
}

fn paint_tcl_console(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    let model = &mut app.model;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Find").small());
        let find = egui::TextEdit::singleline(&mut model.console_find)
            .desired_width(180.0)
            .hint_text("find in journal")
            .font(egui::TextStyle::Monospace);
        let resp = ui.add(find);
        if ui.button("Find").clicked()
            || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
        {
            let q = model.console_find.clone();
            let _ = model.find_console(&q);
        }
        // selection highlighted in the journal grid - no sel= dump crumb
    });
    let selected = model.console_selected;
    let hits = model.console_find_hits.clone();
    let rows: Vec<(usize, helion_gui::ConsoleLine)> = model
        .console_rows()
        .into_iter()
        .map(|(i, l)| (i, l.clone()))
        .collect();
    let mut pick_line = None;
    egui::ScrollArea::both()
        .id_salt("tcl_journal_scroll")
        .stick_to_bottom(true)
        // Short console (floor 36-40px): available_height() - 28 can go
        // negative, which egui treats as an invalid max size.
        .max_height((ui.available_height() - 28.0).max(0.0))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("tcl_console_table")
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("#").strong());
                    ui.label(RichText::new("Status").strong());
                    ui.label(RichText::new("Cmd").strong());
                    ui.label(RichText::new("Out").strong());
                    ui.end_row();
                    if rows.is_empty() {
                        ui.label("-");
                        ui.label("-");
                        ui.label("No commands yet.");
                        ui.label("-");
                        ui.end_row();
                    } else {
                        for (i, line) in &rows {
                            if line.is_session_separator() {
                                // D10: session boundary - dim rule-like row,
                                // not selectable, not clickable.
                                ui.label(RichText::new("-").small().color(theme.faint));
                                ui.label(RichText::new("").small());
                                ui.label(RichText::new("").small());
                                ui.label(
                                    RichText::new("- session started -")
                                        .small()
                                        .italics()
                                        .color(theme.faint),
                                );
                                ui.end_row();
                                continue;
                            }
                            let on = selected == Some(*i);
                            let hit = hits.contains(i);
                            ui.monospace(i.to_string());
                            let btn = egui::Button::new(
                                RichText::new(line.status()).color(Color32::BLACK),
                            )
                            .fill(log_status_color(&app.theme, line.ok))
                            .selected(on);
                            if ui.add(btn).clicked() {
                                pick_line = Some(*i);
                            }
                            let cmd_col = if on {
                                theme.gold
                            } else if hit {
                                theme.info
                            } else {
                                theme.text
                            };
                            if ui
                                .selectable_label(on, RichText::new(&line.cmd).color(cmd_col))
                                .clicked()
                            {
                                pick_line = Some(*i);
                            }
                            let out = if line.out.len() > 80 {
                                format!("{}…", &line.out[..80])
                            } else if line.out.is_empty() {
                                "-".into()
                            } else {
                                line.out.clone()
                            };
                            if ui.selectable_label(on, out).clicked() {
                                pick_line = Some(*i);
                            }
                            ui.end_row();
                        }
                    }
                });
        });
    if let Some(i) = pick_line {
        let _ = model.select_console_line(&i.to_string());
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("helion%").monospace());
        let edit = egui::TextEdit::multiline(&mut model.input)
            .desired_width(f32::INFINITY)
            .desired_rows(1)
            .hint_text("get_cells | get_nets clk* | report_timing | set a [expr {2+2}] … (Enter: run · Shift+Enter: newline · Up/Down: history)")
            .font(egui::TextStyle::Monospace);
        let resp = ui.add(edit);
        if resp.has_focus() {
            let (up, down, enter, shift) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::Enter),
                    i.modifiers.shift,
                )
            });
            // Up/Down walk history only on a single-line edit, so they
            // still move the cursor inside multi-line scripts.
            let single = !model.input.contains('\n');
            if up && single {
                model.history_step(-1);
            } else if down && single {
                model.history_step(1);
            }
            // Enter already inserted a newline; submit only when the
            // command is complete, otherwise keep editing the script.
            if enter && !shift && IdeModel::tcl_input_complete(&model.input) {
                let _ = model.submit_input();
                resp.request_focus();
            }
        }
    });
}

#[allow(dead_code)] // intentional: WIP panel kept for upcoming canvas wiring
fn paint_log(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    let selected = model.selected_log;
    let rows: Vec<(usize, helion_gui::ConsoleLine)> = model
        .log_rows()
        .into_iter()
        .map(|(i, l)| (i, l.clone()))
        .collect();
    let mut pick: Option<usize> = None;
    egui::ScrollArea::both()
        .id_salt("log_scroll")
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("log_table")
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("#").strong());
                    ui.label(RichText::new("Status").strong());
                    ui.label(RichText::new("Command").strong());
                    ui.label(RichText::new("Result").strong());
                    ui.end_row();
                    if rows.is_empty() {
                        ui.label("-");
                        ui.label("-");
                        ui.label("-");
                        ui.label("No log yet - run a flow step or a console command.");
                        ui.end_row();
                    } else {
                        for (i, line) in &rows {
                            if line.is_session_separator() {
                                // D10: session boundary - dim rule-like row,
                                // not selectable, not clickable.
                                ui.label(RichText::new("-").small().color(theme.faint));
                                ui.label(RichText::new("").small());
                                ui.label(RichText::new("").small());
                                ui.label(
                                    RichText::new("- session started -")
                                        .small()
                                        .italics()
                                        .color(theme.faint),
                                );
                                ui.end_row();
                                continue;
                            }
                            let on = selected == Some(*i);
                            ui.monospace(i.to_string());
                            let status = if line.ok { "ok" } else { "error" };
                            let btn =
                                egui::Button::new(RichText::new(status).color(Color32::BLACK))
                                    .fill(log_status_color(theme, line.ok))
                                    .selected(on);
                            if ui.add(btn).clicked() {
                                pick = Some(*i);
                            }
                            if ui.selectable_label(on, &line.cmd).clicked() {
                                pick = Some(*i);
                            }
                            let out = if line.out.len() > 80 {
                                format!("{}…", &line.out[..80])
                            } else if line.out.is_empty() {
                                "-".into()
                            } else {
                                line.out.clone()
                            };
                            if ui.selectable_label(on, out).clicked() {
                                pick = Some(*i);
                            }
                            ui.end_row();
                        }
                    }
                });
        });
    if let Some(i) = pick {
        let _ = model.select_log(&i.to_string());
    }
}

fn paint_sim_log(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    let _n_err = model
        .sim_log
        .iter()
        .filter(|r| r.severity == MsgSeverity::Error)
        .count();
    let selected = model.selected_sim_log;
    let rows = model.sim_log.clone();
    let mut pick: Option<usize> = None;
    egui::ScrollArea::both()
        .id_salt("simlog_scroll")
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("sim_log_table")
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("#").strong());
                    ui.label(RichText::new("Time").strong());
                    ui.label(RichText::new("Severity").strong());
                    ui.label(RichText::new("ID").strong());
                    ui.label(RichText::new("Message").strong());
                    ui.end_row();
                    if rows.is_empty() {
                        ui.label("-");
                        ui.label("-");
                        ui.label("-");
                        ui.label("No simulation log yet.");
                        if primary_button(ui, "Run").clicked() {
                            let _ = model.exec("run_simulation");
                        }
                        ui.end_row();
                    } else {
                        for (i, row) in rows.iter().enumerate() {
                            let on = selected == Some(i);
                            ui.monospace(i.to_string());
                            if ui
                                .selectable_label(on, format!("{} ps", row.time_ps))
                                .clicked()
                            {
                                pick = Some(i);
                            }
                            let btn = egui::Button::new(
                                RichText::new(row.severity.tag()).color(Color32::BLACK),
                            )
                            .fill(msg_severity_color(theme, row.severity))
                            .selected(on);
                            if ui.add(btn).clicked() {
                                pick = Some(i);
                            }
                            if ui.selectable_label(on, &row.id).clicked() {
                                pick = Some(i);
                            }
                            if ui.selectable_label(on, &row.text).clicked() {
                                pick = Some(i);
                            }
                            ui.end_row();
                        }
                    }
                });
        });
    if let Some(i) = pick {
        let _ = model.select_sim_log(&i.to_string());
    }
}

fn paint_project_settings(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("Project Settings");
    let rows = model.project_setting_rows();
    let selected = model.selected_setting.clone();
    let mut pick: Option<String> = None;
    let remain = ui.available_size();
    let bbox = chrome::nv_table_bbox(rows.len().max(1), remain.x.max(80.0), remain.y.max(120.0));
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(bbox.drawn_w.max(remain.x), bbox.drawn_h.max(remain.y)),
        Sense::hover(),
    );
    ui.painter().rect_filled(rect, 0.0, theme.die_bg);
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(8.0)), |ui| {
        let n = rows.len().max(1) as f32;
        let row_gap = ((ui.available_height() - 28.0) / n - 18.0).clamp(4.0, 28.0);
        let col_w = chrome::stretched_col_w(2, ui.available_width());
        egui::ScrollArea::both()
            .id_salt("ug893_project_settings")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("project_settings_table")
                    .spacing([12.0, row_gap])
                    .min_col_width(col_w)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Name").strong());
                        ui.label(RichText::new("Value").strong());
                        ui.end_row();
                        for (i, r) in rows.iter().enumerate() {
                            let on = selected.as_deref() == Some(r.name.as_str());
                            if ui.selectable_label(on, &r.name).clicked() {
                                pick = Some(i.to_string());
                            }
                            if ui.selectable_label(on, &r.value).clicked() {
                                pick = Some(i.to_string());
                            }
                            ui.end_row();
                        }
                    });
            });
    });
    if let Some(spec) = pick {
        let _ = model.select_project_setting(&spec);
    }
}

fn paint_sim_settings(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Simulation Settings");
    ui.horizontal(|ui| {
        if ui.button("Compile").clicked() {
            let _ = model.exec("compile");
        }
        if ui.button("Elaborate").clicked() {
            let _ = model.exec("elaborate");
        }
    });
    let rows = model.sim_setting_rows();
    let selected = model.selected_sim_setting.clone();
    let mut pick: Option<String> = None;
    egui::ScrollArea::both()
        .id_salt("ug900_simulation_settings")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("sim_settings_table")
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Name").strong());
                    ui.label(RichText::new("Value").strong());
                    ui.end_row();
                    for (i, r) in rows.iter().enumerate() {
                        let on = selected.as_deref() == Some(r.name.as_str());
                        if ui.selectable_label(on, &r.name).clicked() {
                            pick = Some(i.to_string());
                        }
                        if ui.selectable_label(on, &r.value).clicked() {
                            pick = Some(i.to_string());
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(spec) = pick {
        let _ = model.select_sim_setting(&spec);
    }
}

fn run_status_color(theme: &theme::Theme, status: &str) -> Color32 {
    match status {
        "Complete" => theme.ok,
        "Running" | "Warnings" => theme.warn,
        "Failed" => theme.danger,
        _ => theme.faint,
    }
}

fn paint_runs(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    // Deferred user feedback: set instead of pushing a toast directly, so the
    // model borrow below does not conflict with the toast queue borrow.
    let mut notice: Option<(toasts::ToastKind, String)> = None;
    let model = &mut app.model;
    ui.heading("Runs");
    ui.add_space(4.0);

    // --- Queue actions -----------------------------------------------------
    let pending = model.run_queue.len();
    let selected_name = model
        .selected
        .as_deref()
        .map(|s| s.strip_prefix("run:").unwrap_or(s).to_string());
    let selected_has_result = selected_name
        .as_deref()
        .and_then(|n| model.runs.iter().find(|r| r.name == *n))
        .is_some_and(|r| r.result_session.is_some());

    ui.horizontal(|ui| {
        if ui
            .add_sized(
                [140.0, chrome::HIT_COMFORT],
                egui::Button::new("Launch all pending"),
            )
            .clicked()
        {
            let names: Vec<String> = model
                .runs
                .iter()
                .filter(|r| r.step == "Implementation" && r.status == "Not started")
                .map(|r| r.name.clone())
                .collect();
            if names.is_empty() {
                notice = Some((
                    toasts::ToastKind::Warn,
                    "No pending runs to launch.".to_string(),
                ));
            } else {
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                if model.enqueue_design_runs(&refs).is_ok() {
                    kick_run_queue(model);
                }
            }
        }
        if ui
            .add_sized(
                [130.0, chrome::HIT_COMFORT],
                egui::Button::new("Launch selected"),
            )
            .clicked()
        {
            if let Some(name) = selected_name.clone() {
                if model.enqueue_design_runs(&[name.as_str()]).is_ok() {
                    kick_run_queue(model);
                }
            }
        }
        if ui
            .add_sized(
                [110.0, chrome::HIT_COMFORT],
                egui::Button::new("Cancel queue"),
            )
            .clicked()
        {
            model.cancel_run_queue();
        }
        // Disabled until the selected run has a result to activate.
        let make_active = ui
            .add_enabled_ui(selected_has_result, |ui| {
                ui.add_sized(
                    [110.0, chrome::HIT_COMFORT],
                    egui::Button::new("Make active"),
                )
            })
            .inner;
        if make_active.clicked() {
            if let Some(name) = selected_name.clone() {
                let _ = model.make_run_active(&name);
            }
        }
    });
    if pending > 0 {
        ui.label(
            RichText::new(format!(
                "Queue: {pending} run{} waiting - results stream in as each finishes.",
                if pending == 1 { "" } else { "s" }
            ))
            .color(theme.info),
        );
    } else {
        ui.label(
            RichText::new("Queue empty. Create runs below, then launch them.")
                .color(theme.dim),
        );
    }
    ui.add_space(8.0);

    // --- New run: strategy editor ------------------------------------------
    egui::CollapsingHeader::new("New run - strategy")
        .default_open(false)
        .show(ui, |ui| {
            let mut create: Option<String> = None;
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.text_edit_singleline(&mut model.run_editor.name);
                ui.label("Preset:");
                let cur = model.run_editor.preset.clone();
                egui::ComboBox::from_id_salt("run_preset")
                    .selected_text(cur.as_str())
                    .show_ui(ui, |ui| {
                        for p in [
                            "Default",
                            "TimingExplore",
                            "RuntimeOpt",
                            "PhysOpt",
                            "Custom",
                        ] {
                            if ui.selectable_label(cur == p, p).clicked() {
                                model.run_editor.apply_preset(p);
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Opt level:");
                if ui
                    .add(
                        egui::DragValue::new(&mut model.run_editor.opt_level)
                            .range(0..=2),
                    )
                    .changed()
                {
                    model.run_editor.touched();
                }
                if ui
                    .checkbox(&mut model.run_editor.aig_rewrite, "AIG rewrite")
                    .changed()
                {
                    model.run_editor.touched();
                }
                if ui
                    .checkbox(&mut model.run_editor.timing_driven, "Timing-driven")
                    .changed()
                {
                    model.run_editor.touched();
                }
                if ui
                    .checkbox(&mut model.run_editor.phys_opt, "Phys-opt")
                    .changed()
                {
                    model.run_editor.touched();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Placer seed:");
                if ui
                    .add(egui::DragValue::new(&mut model.run_editor.placer_seed))
                    .changed()
                {
                    model.run_editor.touched();
                }
                ui.label("Router iters:");
                if ui
                    .add(
                        egui::DragValue::new(&mut model.run_editor.router_iters)
                            .range(1..=64),
                    )
                    .changed()
                {
                    model.run_editor.touched();
                }
            });
            ui.horizontal(|ui| {
                if primary_button(ui, "Create run").clicked() {
                    let name = model.run_editor.name.trim().to_string();
                    if name.is_empty() {
                        notice = Some((
                            toasts::ToastKind::Warn,
                            "Give the run a name first.".to_string(),
                        ));
                    } else if model.runs.iter().any(|r| r.name == name) {
                        notice = Some((
                            toasts::ToastKind::Warn,
                            format!("A run named '{name}' already exists."),
                        ));
                    } else {
                        let spec = model.run_editor.strategy().to_spec();
                        create = Some(format!("{name} -spec {spec}"));
                    }
                }
                ui.label(
                    RichText::new("Seed 0 = deterministic (legacy order).")
                        .color(theme.dim),
                );
            });
            if let Some(cmd) = create {
                let _ = model.create_run(&cmd);
                model.run_editor.name.clear();
            }
        });
    ui.add_space(4.0);

    // --- Seed sweep ----------------------------------------------------------
    egui::CollapsingHeader::new("Seed sweep")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Base name:");
                ui.text_edit_singleline(&mut model.run_editor.sweep_base);
                ui.label("Seeds (comma-separated):");
                ui.text_edit_singleline(&mut model.run_editor.sweep_seeds);
            });
            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut model.run_editor.sweep_spec_from_editor,
                    "Use strategy editor settings (else Default)",
                );
            });
            if primary_button(ui, "Create + queue sweep").clicked() {
                let base = model.run_editor.sweep_base.trim().to_string();
                let seeds: Vec<u64> = model
                    .run_editor
                    .sweep_seeds
                    .split(',')
                    .filter_map(|s| s.trim().parse::<u64>().ok())
                    .collect();
                let spec = if model.run_editor.sweep_spec_from_editor {
                    model.run_editor.strategy().to_spec()
                } else {
                    helion_proj::RunStrategy::preset("Default")
                        .map(|p| p.to_spec())
                        .unwrap_or_default()
                };
                if !base.is_empty() && !spec.is_empty() {
                    if model.create_seed_sweep(&base, &spec, &seeds).is_ok() {
                        kick_run_queue(model);
                    }
                }
            }
            ui.label(
                RichText::new("Each seed becomes its own run (base_s1, base_s2, …) with the placer seed knob set.")
                    .color(theme.dim),
            );
        });
    ui.add_space(8.0);

    // --- Results table (sortable, best-in-column highlighted) ----------------
    ui.label(RichText::new("Results").strong());
    {
        let sort_col = model.run_editor.sort_col;
        let sort_asc = model.run_editor.sort_asc;
        let mut order: Vec<usize> = (0..model.runs.len()).collect();
        sort_run_indices(&mut order, &model.runs, sort_col, sort_asc);
        // Best value per numeric column, over finished runs.
        let best: std::collections::HashMap<RunCol, usize> = RunCol::ALL
            .iter()
            .filter_map(|c| best_run_index(&model.runs, *c).map(|i| (*c, i)))
            .collect();

        let selected = model.selected.clone();
        let mut pick: Option<String> = None;
        let mut sort_click: Option<RunCol> = None;
        egui::ScrollArea::both()
            .id_salt("design_runs_results")
            .max_height(260.0)
            .show(ui, |ui| {
                egui::Grid::new("design_runs_table")
                    .spacing([10.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for col in RunCol::ALL {
                            let arrow = if sort_col == col {
                                if sort_asc { " ▲" } else { " ▼" }
                            } else {
                                ""
                            };
                            if ui
                                .link(format!("{}{}", col.header(), arrow))
                                .clicked()
                            {
                                sort_click = Some(col);
                            }
                        }
                        ui.end_row();
                        for &i in &order {
                            let r = &model.runs[i];
                            let id = format!("run:{}", r.name);
                            let on = selected.as_deref() == Some(id.as_str())
                                || selected.as_deref() == Some(r.name.as_str());
                            if ui.selectable_label(on, &r.name).clicked() {
                                pick = Some(r.name.clone());
                            }
                            ui.label(r.strategy_cell());
                            let fill = run_status_color(&theme, &r.status);
                            let btn = egui::Button::new(
                                RichText::new(&r.status).color(Color32::BLACK),
                            )
                            .fill(fill)
                            .selected(on);
                            if ui.add(btn).clicked() {
                                pick = Some(r.name.clone());
                            }
                            for col in [
                                RunCol::Luts,
                                RunCol::Ffs,
                                RunCol::Bram,
                                RunCol::Dsp,
                                RunCol::Wns,
                                RunCol::Tns,
                                RunCol::Runtime,
                            ] {
                                let txt = match col {
                                    RunCol::Luts => r.luts_cell(),
                                    RunCol::Ffs => r.ffs_cell(),
                                    RunCol::Bram => r.brams_cell(),
                                    RunCol::Dsp => r.dsps_cell(),
                                    RunCol::Wns => r.wns_cell(),
                                    RunCol::Tns => r.tns_cell(),
                                    RunCol::Runtime => r.runtime_cell(),
                                    _ => "-".into(),
                                };
                                let is_best =
                                    best.get(&col).is_some_and(|&bi| bi == i);
                                let label = if is_best {
                                    RichText::new(format!("★ {txt}"))
                                        .color(theme.ok)
                                        .strong()
                                } else {
                                    RichText::new(txt)
                                };
                                ui.label(label);
                            }
                            ui.end_row();
                        }
                    });
            });
        if let Some(col) = sort_click {
            if model.run_editor.sort_col == col {
                model.run_editor.sort_asc = !model.run_editor.sort_asc;
            } else {
                model.run_editor.sort_col = col;
                // Numeric "lower is better" columns default ascending;
                // WNS/TNS default descending (best first).
                model.run_editor.sort_asc = col.lower_is_better()
                    || matches!(col, RunCol::Name | RunCol::Strategy | RunCol::Status);
            }
        }
        if let Some(name) = pick {
            let _ = model.select_run(&name);
        }
    }
    ui.add_space(4.0);

    // --- Sweep summary -------------------------------------------------------
    {
        let sweep_runs: Vec<helion_gui::DesignRun> = model
            .runs
            .iter()
            .filter(|r| r.seed.is_some())
            .cloned()
            .collect();
        if let Some((best, worst, mean)) = sweep_wns_stats(&sweep_runs) {
            ui.label(RichText::new(format!(
                "Seed sweep WNS: best {best} ps · worst {worst} ps · mean {mean} ps ({} runs)",
                sweep_runs.len()
            )));
        }
    }

    ui.add_space(8.0);
    paint_incremental_report(ui, model, &theme);
    ui.add_space(8.0);
    paint_eco_changes(ui, model, &theme);

    // Flush deferred user feedback now that the model borrow has ended.
    if let Some((kind, msg)) = notice {
        app.toasts.push(kind, msg);
    }
}

/// Pop the first queued run and hand it to the engine thread.
/// If a job is already live, `submit_job` re-queues the name at the front -
/// the queue is the source of truth and nothing is ever dropped.
fn kick_run_queue(model: &mut IdeModel) {
    if let Some(name) = model.run_queue.pop_front() {
        surface::request_job(JobKind::DesignRun { name });
    }
}
/// Format unix seconds as `YYYY-MM-DD HH:MM` (UTC) without extra crates.
fn fmt_checkpoint_time(secs: u64) -> String {
    // Howard Hinnant's civil_from_days.
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let hh = (secs % 86_400) / 3_600;
    let mm = (secs % 3_600) / 60;
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
}

fn fmt_checkpoint_bytes(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1_024 {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Design checkpoints (`.hck2`): save / restore / delete / diff snapshots of the
/// design database at post-synth, post-pack, post-place, or post-route.
fn paint_checkpoints(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.horizontal(|ui| {
        ui.heading("Checkpoints");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.checkbox(&mut model.auto_checkpoint, "Auto-checkpoint after each stage")
                .on_hover_text(tip(
                    "Auto-checkpoint",
                    "",
                    "saves one checkpoint per flow stage (post-synth, post-pack, post-place, post-route)",
                ));
        });
    });
    ui.add_space(4.0);

    // Save row.
    ui.horizontal(|ui| {
        ui.label("Save:");
        let resp = ui.add_sized(
            [220.0, chrome::HIT_SIDEBAR],
            egui::TextEdit::singleline(&mut model.checkpoint_name)
                .hint_text("checkpoint name…"),
        );
        let mut do_save = primary_button(ui, "Save checkpoint")
            .on_hover_text(tip("Save checkpoint", "", "checkpoint_save <name>"))
            .clicked();
        do_save |= resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if do_save {
            let name = model.checkpoint_name.trim().to_string();
            let name = if name.is_empty() { "checkpoint".into() } else { name };
            match model.save_checkpoint_named(&name) {
                Ok(_) => model.checkpoint_name.clear(),
                Err(e) => model.status = format!("checkpoint save failed: {e}"),
            }
        }
        ui.label(
            RichText::new(format!("→ {}", model.checkpoint_dir().display()))
                .size(12.0)
                .color(theme.dim),
        );
    });
    ui.add_space(6.0);

    // Table.
    let mut toggle: Option<String> = None;
    egui::ScrollArea::both()
        .id_salt("checkpoints_table")
        .max_height(260.0)
        .show(ui, |ui| {
            egui::Grid::new("checkpoints_grid")
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    for h in [
                        "Name", "Stage", "Created", "LUT", "FF", "IOB", "DSP", "BRAM",
                        "WNS", "TNS", "Size",
                    ] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    if model.checkpoints.is_empty() {
                        ui.label(
                            RichText::new("No checkpoints yet.")
                                .color(theme.dim),
                        );
                        ui.end_row();
                    }
                    for c in &model.checkpoints {
                        let on = model.checkpoint_sel.iter().any(|n| n == &c.name);
                        if ui.selectable_label(on, &c.name).clicked() {
                            toggle = Some(c.name.clone());
                        }
                        ui.label(c.stage.label());
                        ui.label(fmt_checkpoint_time(c.created_unix));
                        ui.label(c.luts.to_string());
                        ui.label(c.ffs.to_string());
                        ui.label(c.iobs.to_string());
                        ui.label(c.dsps.to_string());
                        ui.label(c.brams.to_string());
                        ui.label(if c.has_timing() {
                            format!("{} ps", c.wns_ps)
                        } else {
                            "-".into()
                        });
                        ui.label(if c.has_timing() {
                            format!("{} ps", c.tns_ps)
                        } else {
                            "-".into()
                        });
                        ui.label(fmt_checkpoint_bytes(c.bytes));
                        ui.end_row();
                    }
                });
        });
    if let Some(name) = toggle {
        if let Some(i) = model.checkpoint_sel.iter().position(|n| n == &name) {
            model.checkpoint_sel.remove(i);
        } else {
            model.checkpoint_sel.push(name);
        }
    }
    ui.add_space(6.0);

    // Actions.
    ui.horizontal(|ui| {
        let n_sel = model.checkpoint_sel.len();
        let restore_btn = ui
            .add_enabled(
                n_sel == 1,
                egui::Button::new("Restore selected"),
            )
            .on_hover_text(tip(
                "Restore",
                "",
                "replace the session with the checkpoint (views refresh)",
            ));
        if restore_btn.clicked() {
            if let Some(name) = model.checkpoint_sel.first().cloned() {
                if let Err(e) = model.restore_checkpoint(&name) {
                    model.status = format!("checkpoint restore failed: {e}");
                }
            }
        }
        let delete_btn = ui
            .add_enabled(
                n_sel >= 1,
                egui::Button::new(if n_sel > 1 {
                    format!("Delete {} selected", n_sel)
                } else {
                    "Delete selected".into()
                }),
            )
            .on_hover_text(tip("Delete", "", "checkpoint_delete <name>"));
        if delete_btn.clicked() {
            let names = model.checkpoint_sel.clone();
            for name in names {
                if let Err(e) = model.delete_checkpoint_named(&name) {
                    model.status = format!("checkpoint delete failed: {e}");
                    break;
                }
            }
        }
        if n_sel == 2 {
            ui.label(
                RichText::new("Two selected - diff below.")
                    .color(theme.dim),
            );
        } else if n_sel > 2 {
            ui.label(
                RichText::new("Select exactly two for a diff.")
                    .color(theme.dim),
            );
        }
    });

    // Diff.
    if model.checkpoint_sel.len() == 2 {
        ui.add_space(8.0);
        ui.label(RichText::new("Diff (second minus first)").strong());
        if let Some(text) = model.checkpoint_diff_text() {
            ui.label(RichText::new(text).monospace().size(13.0));
        }
    }
}

fn incremental_status_color(theme: &theme::Theme, status: &str) -> Color32 {
    match status {
        "Reused" => theme.ok,
        "New" => theme.info,
        "Partial" => theme.warn,
        _ => theme.faint,
    }
}

fn paint_incremental_report(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.label(RichText::new("Incremental Compile").strong());
    let rows = model.incremental_rows.clone();
    let _n_new = rows
        .iter()
        .filter(|r| r.kind != "resource" && r.status == "New")
        .count();
    let selected = model.selected_incremental.clone();
    let selected_obj = model.selected.clone();
    let mut pick: Option<String> = None;
    let mut pick_obj: Option<String> = None;
    if rows.is_empty() {
        ui.label("No incremental report yet.");
        if primary_button(ui, "Run incremental").clicked() {
            let _ = model.exec("incremental_impl");
        }
        return;
    }
    egui::ScrollArea::both()
        .id_salt("ug986_incremental_report")
        .max_height(180.0)
        .show(ui, |ui| {
            egui::Grid::new("incremental_report_table")
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Name").strong());
                    ui.label(RichText::new("Kind").strong());
                    ui.label(RichText::new("Status").strong());
                    ui.label(RichText::new("Site").strong());
                    ui.label(RichText::new("Reused").strong());
                    ui.label(RichText::new("Total").strong());
                    ui.label(RichText::new("Pct").strong());
                    ui.label(RichText::new("Objects").strong());
                    ui.end_row();
                    for (i, r) in rows.iter().enumerate() {
                        let obj = model.incremental_object_cell(r);
                        let on = selected.as_deref() == Some(r.name.as_str());
                        let on_obj = selected_obj.as_deref() == Some(obj.as_str())
                            || (obj != "-"
                                && selected_obj
                                    .as_deref()
                                    .is_some_and(|s| obj.split(',').any(|t| t == s)));
                        if ui.selectable_label(on || on_obj, &r.name).clicked() {
                            if obj == "-" {
                                pick = Some(i.to_string());
                            } else {
                                pick_obj = Some(i.to_string());
                            }
                        }
                        if ui.selectable_label(on, r.kind_cell()).clicked() {
                            pick = Some(i.to_string());
                        }
                        let fill = incremental_status_color(theme, &r.status);
                        let btn =
                            egui::Button::new(RichText::new(r.status_cell()).color(Color32::BLACK))
                                .fill(fill)
                                .selected(on);
                        if ui.add(btn).clicked() {
                            pick = Some(i.to_string());
                        }
                        if ui.selectable_label(on, r.site_cell()).clicked() {
                            pick = Some(i.to_string());
                        }
                        if ui.selectable_label(on, r.reused.to_string()).clicked() {
                            pick = Some(i.to_string());
                        }
                        if ui.selectable_label(on, r.total.to_string()).clicked() {
                            pick = Some(i.to_string());
                        }
                        if ui.selectable_label(on, format!("{}%", r.pct)).clicked() {
                            pick = Some(i.to_string());
                        }
                        if ui.selectable_label(on || on_obj, &obj).clicked() {
                            if obj == "-" {
                                pick = Some(i.to_string());
                            } else {
                                pick_obj = Some(i.to_string());
                            }
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(spec) = pick_obj {
        let _ = model.select_incremental_object(&spec);
    } else if let Some(spec) = pick {
        let _ = model.select_incremental(&spec);
    }
}

fn eco_status_color(theme: &theme::Theme, status: &str) -> Color32 {
    match status {
        "Placed" => theme.ok,
        "Missing" => theme.warn,
        _ => theme.faint,
    }
}

fn paint_eco_changes(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.label(RichText::new("ECO Changes").strong());
    let rows = model.eco_rows();
    let _n_missing = rows.iter().filter(|r| r.status == "Missing").count();
    let selected = model.selected_eco.clone();
    let selected_obj = model.selected.clone();
    let mut pick: Option<String> = None;
    let mut pick_obj: Option<String> = None;
    if rows.is_empty() {
        ui.label("No ECO cells yet.");
        if primary_button(ui, "Insert ECO LUT").clicked() {
            let _ = model.exec("insert_eco_lut");
        }
        return;
    }
    egui::ScrollArea::both()
        .id_salt("ug893_eco_changes")
        .max_height(180.0)
        .show(ui, |ui| {
            egui::Grid::new("eco_changes_table")
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Name").strong());
                    ui.label(RichText::new("Kind").strong());
                    ui.label(RichText::new("Status").strong());
                    ui.label(RichText::new("Site").strong());
                    ui.label(RichText::new("Init").strong());
                    ui.label(RichText::new("Objects").strong());
                    ui.end_row();
                    for (i, r) in rows.iter().enumerate() {
                        let obj = model.eco_object_cell(&r);
                        let on = selected.as_deref() == Some(r.name.as_str());
                        let on_obj = selected_obj.as_deref() == Some(obj.as_str());
                        if ui.selectable_label(on || on_obj, &r.name).clicked() {
                            pick_obj = Some(i.to_string());
                        }
                        if ui.selectable_label(on, r.kind_cell()).clicked() {
                            pick = Some(i.to_string());
                        }
                        let fill = eco_status_color(theme, &r.status);
                        let btn =
                            egui::Button::new(RichText::new(r.status_cell()).color(Color32::BLACK))
                                .fill(fill)
                                .selected(on);
                        if ui.add(btn).clicked() {
                            pick = Some(i.to_string());
                        }
                        if ui.selectable_label(on, r.site_cell()).clicked() {
                            pick = Some(i.to_string());
                        }
                        if ui.selectable_label(on, r.init_cell()).clicked() {
                            pick = Some(i.to_string());
                        }
                        if ui.selectable_label(on || on_obj, &obj).clicked() {
                            pick_obj = Some(i.to_string());
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(spec) = pick_obj {
        let _ = model.select_eco_object(&spec);
    } else if let Some(spec) = pick {
        let _ = model.select_eco(&spec);
    }
}

fn paint_hierarchy(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    hierarchy::paint_hierarchy(ui, model, theme);
}

fn paint_find(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("Find Results");
    ui.horizontal(|ui| {
        if ui.button("Find cells").clicked() {
            let _ = model.exec("sheet_find cells");
        }
        if ui.button("Find ports").clicked() {
            let _ = model.exec("sheet_find ports");
        }
        if ui.button("Find nets").clicked() {
            let _ = model.exec("sheet_find nets");
        }
    });
    let selected = model.selected.clone();
    let selected_find = model.selected_find;
    let rows = model.find_rows().to_vec();
    let mut pick: Option<String> = None;
    let mut pick_obj: Option<String> = None;
    if rows.is_empty() {
        if paint_remaining_cta(ui, "No hits yet.", "Find cells", theme) {
            let _ = model.exec("sheet_find cells");
        }
        return;
    }
    egui::ScrollArea::both()
        .id_salt("find_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("find_results_table")
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Name").strong());
                    ui.label(RichText::new("Type").strong());
                    ui.label(RichText::new("Primitive").strong());
                    ui.label(RichText::new("Parent").strong());
                    ui.label(RichText::new("Objects").strong());
                    ui.end_row();
                    {
                        for (i, h) in rows.iter().enumerate() {
                            let on = selected_find == Some(i)
                                || selected.as_deref() == Some(h.name.as_str());
                            if ui.selectable_label(on, &h.name).clicked() {
                                pick_obj = Some(i.to_string());
                            }
                            if ui.selectable_label(on, h.type_cell()).clicked() {
                                pick = Some(i.to_string());
                            }
                            ui.label(h.primitive_cell());
                            ui.label(h.parent_cell());
                            if ui.selectable_label(on, &h.name).clicked() {
                                pick_obj = Some(i.to_string());
                            }
                            ui.end_row();
                        }
                    }
                });
        });
    if let Some(id) = pick_obj {
        let _ = model.select_find_object(&id);
    } else if let Some(id) = pick {
        let _ = model.select_find(&id);
    }
}

fn paint_io_ports_table(ui: &mut egui::Ui, model: &mut IdeModel, grid_id: &'static str) {
    ui.label(RichText::new("I/O Ports").strong());
    let _assigned = model
        .io_ports
        .iter()
        .filter(|p| p.package_pin.is_some() || p.site.is_some())
        .count();
    let selected = model.selected.clone();
    let selected_io = model.selected_io_port.clone();
    let rows = model.io_port_rows().to_vec();
    let mut pick_port: Option<String> = None;
    let mut pick_obj: Option<String> = None;
    let mut set_iostd: Option<(String, &'static str)> = None;
    let mut set_io: Option<(String, &'static str, &'static str)> = None;
    let mut set_pkg_pin: Option<(String, String)> = None;
    if grid_id == "sidebar_io" {
        // Full HAD site names. The wide grid clipped Placed to "IOB_X…".
        ui.label(RichText::new("Name  Dir  Site").small().weak());
        if rows.is_empty() {
            ui.label("Synth a design to list ports.");
        }
        for p in &rows {
            let site = p.placed_cell();
            let line = chrome::device_io_site_line(&p.name, &p.dir, site);
            let on = selected_io.as_deref() == Some(p.name.as_str())
                || selected.as_deref() == Some(p.name.as_str())
                || selected.as_deref() == Some(p.placed_cell());
            let resp = ui.add(
                egui::Button::selectable(on, RichText::new(line).monospace())
                    .wrap_mode(egui::TextWrapMode::Extend),
            );
            if resp.clicked() {
                pick_obj = Some(if site == "-" {
                    p.name.clone()
                } else {
                    p.placed_cell().to_string()
                });
                pick_port = Some(p.name.clone());
            }
        }
        egui::CollapsingHeader::new("Pin properties")
            .default_open(false)
            .id_salt("sidebar_io_props")
            .show(ui, |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("sidebar_io_props_scroll")
                    .show(ui, |ui| {
                        ui.set_min_width(420.0);
                        for p in &rows {
                            ui.label(
                                RichText::new(format!(
                                    "{}  pin {}  {}  DRIVE {}  SLEW {}  {}",
                                    p.name,
                                    p.package_pin_cell(),
                                    p.iostandard_cell(),
                                    p.drive_cell(),
                                    p.slew_cell(),
                                    p.placed_cell(),
                                ))
                                .monospace()
                                .small(),
                            );
                        }
                    });
            });
    } else {
        data_scroll("io_ports_table_scroll").show(ui, |ui| {
            egui::Grid::new(grid_id).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Dir").strong());
                ui.label(RichText::new("Package Pin").strong());
                ui.label(RichText::new("Placed").strong());
                ui.label(RichText::new("IOSTANDARD").strong());
                ui.label(RichText::new("Drive").strong());
                ui.label(RichText::new("Slew").strong());
                ui.label(RichText::new("Pull Type").strong());
                ui.label(RichText::new("Diff Term").strong());
                ui.label(RichText::new("In Term").strong());
                ui.label(RichText::new("Objects").strong());
                ui.end_row();
                if rows.is_empty() {
                    ui.label("-");
                    ui.label("-");
                    ui.label("-");
                    ui.label("-");
                    ui.label("-");
                    ui.label("-");
                    ui.label("-");
                    ui.label("-");
                    ui.label("-");
                    ui.label("Synth a design to list ports.");
                    ui.label("-");
                    ui.end_row();
                } else {
                    for p in &rows {
                        let obj = model.io_port_object_cell(p);
                        let on = selected_io.as_deref() == Some(p.name.as_str())
                            || selected.as_deref() == Some(p.name.as_str());
                        let on_obj = selected.as_deref() == Some(p.name.as_str())
                            || selected.as_deref() == Some(p.package_pin_cell())
                            || selected.as_deref() == Some(p.placed_cell());
                        if ui.selectable_label(on || on_obj, &p.name).clicked() {
                            pick_obj = Some(p.name.clone());
                        }
                        if ui.selectable_label(on, &p.dir).clicked() {
                            pick_port = Some(p.name.clone());
                        }
                        let cur_pin = p.package_pin_cell().to_string();
                        let pin_ports: Vec<(String, Option<String>)> = model
                            .package_pins
                            .iter()
                            .map(|pin| (pin.pin.clone(), pin.port.clone()))
                            .collect();
                        egui::ComboBox::from_id_salt(("io_pkg_pin", p.name.as_str()))
                            .selected_text(if cur_pin == "-" {
                                "- set site -"
                            } else {
                                cur_pin.as_str()
                            })
                            .width(118.0)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(cur_pin == "-", "- (select port) -")
                                    .clicked()
                                {
                                    pick_port = Some(p.name.clone());
                                }
                                for (pin, owner) in &pin_ports {
                                    let label = match owner.as_deref() {
                                        Some(port) if port != p.name => {
                                            format!("{pin} ({port})")
                                        }
                                        _ => pin.clone(),
                                    };
                                    if ui.selectable_label(cur_pin == *pin, label).clicked() {
                                        set_pkg_pin = Some((p.name.clone(), pin.clone()));
                                    }
                                }
                            });
                        if ui.selectable_label(on_obj, p.placed_cell()).clicked() {
                            if p.placed_cell() == "-" {
                                pick_port = Some(p.name.clone());
                            } else {
                                pick_obj = Some(p.placed_cell().to_string());
                            }
                        }
                        // ComboBox Set → model.exec set_property (HNF + table), not labels.
                        let cur_std = p.iostandard_cell().to_string();
                        egui::ComboBox::from_id_salt(("io_iostd", p.name.as_str()))
                            .selected_text(if cur_std == "-" {
                                "- Set -"
                            } else {
                                cur_std.as_str()
                            })
                            .width(96.0)
                            .show_ui(ui, |ui| {
                                for std in
                                    ["LVCMOS18", "LVCMOS33", "LVCMOS12", "LVCMOS25", "SSTL15"]
                                {
                                    if ui.selectable_label(cur_std == std, std).clicked() {
                                        set_iostd = Some((p.name.clone(), std));
                                    }
                                }
                            });
                        let cur_drv = p.drive_cell().to_string();
                        egui::ComboBox::from_id_salt(("io_drive", p.name.as_str()))
                            .selected_text(if cur_drv == "-" {
                                "- Set -"
                            } else {
                                cur_drv.as_str()
                            })
                            .width(64.0)
                            .show_ui(ui, |ui| {
                                for ma in ["4", "8", "12", "16", "24"] {
                                    if ui.selectable_label(cur_drv == ma, ma).clicked() {
                                        set_io = Some((p.name.clone(), "DRIVE", ma));
                                    }
                                }
                            });
                        let cur_slew = p.slew_cell().to_string();
                        egui::ComboBox::from_id_salt(("io_slew", p.name.as_str()))
                            .selected_text(if cur_slew == "-" {
                                "- Set -"
                            } else {
                                cur_slew.as_str()
                            })
                            .width(72.0)
                            .show_ui(ui, |ui| {
                                for s in ["SLOW", "FAST"] {
                                    if ui.selectable_label(cur_slew == s, s).clicked() {
                                        set_io = Some((p.name.clone(), "SLEW", s));
                                    }
                                }
                            });
                        ui.label(p.pulltype_cell());
                        ui.label(p.diff_term_cell());
                        ui.label(p.in_term_cell());
                        if ui.selectable_label(on || on_obj, &obj).clicked() {
                            pick_obj = Some(p.name.clone());
                        }
                        ui.end_row();
                    }
                }
            });
        });
    }
    ui.weak("Package Pin: dropdown on each row, or type a site below / click the package drawing.");
    let selected_port = model.selected_io_port.clone().or_else(|| {
        model
            .selected
            .clone()
            .filter(|s| model.io_ports.iter().any(|p| p.name == *s))
    });
    if let Some(port) = selected_port {
        ui.horizontal(|ui| {
            ui.weak("PACKAGE_PIN");
            let edit = egui::TextEdit::singleline(&mut model.package_pin_draft)
                .desired_width(120.0)
                .hint_text("IOB_X…");
            let resp = ui.add(edit);
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let pin = model.package_pin_draft.trim().to_string();
                if !pin.is_empty() {
                    set_pkg_pin = Some((port.clone(), pin));
                }
            }
            if ui.button("Set pin").clicked() {
                let pin = model.package_pin_draft.trim().to_string();
                if !pin.is_empty() {
                    set_pkg_pin = Some((port.clone(), pin));
                }
            }
            for site in ["IOB_X2Y0", "IOB_X3Y0", "IOB_X0Y0", "IOB_X5Y0"] {
                if ui
                    .add_sized(
                        [ui.spacing().interact_size.x.max(72.0), chrome::HIT_SIDEBAR],
                        egui::Button::new(site),
                    )
                    .clicked()
                {
                    model.package_pin_draft = site.to_string();
                    set_pkg_pin = Some((port.clone(), site.to_string()));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("IOSTANDARD");
            for std in ["LVCMOS18", "LVCMOS33", "LVCMOS12", "SSTL15"] {
                if ui
                    .add_sized([64.0, chrome::HIT_SIDEBAR], egui::Button::new(std))
                    .clicked()
                {
                    set_iostd = Some((port.clone(), std));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("DRIVE");
            for ma in ["4", "8", "12", "16"] {
                if ui
                    .add_sized([40.0, chrome::HIT_SIDEBAR], egui::Button::new(ma))
                    .clicked()
                {
                    set_io = Some((port.clone(), "DRIVE", ma));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("SLEW");
            for s in ["SLOW", "FAST"] {
                if ui
                    .add_sized(
                        [ui.spacing().interact_size.x.max(56.0), chrome::HIT_SIDEBAR],
                        egui::Button::new(s),
                    )
                    .clicked()
                {
                    set_io = Some((port.clone(), "SLEW", s));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("PULLTYPE");
            for s in ["NONE", "PULLUP", "PULLDOWN", "KEEPER"] {
                if ui
                    .add_sized(
                        [ui.spacing().interact_size.x.max(56.0), chrome::HIT_SIDEBAR],
                        egui::Button::new(s),
                    )
                    .clicked()
                {
                    set_io = Some((port.clone(), "PULLTYPE", s));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("DIFF_TERM");
            for s in ["FALSE", "TRUE"] {
                if ui
                    .add_sized(
                        [ui.spacing().interact_size.x.max(56.0), chrome::HIT_SIDEBAR],
                        egui::Button::new(s),
                    )
                    .clicked()
                {
                    set_io = Some((port.clone(), "DIFF_TERM", s));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("IN_TERM");
            for s in [
                "NONE",
                "UNTUNED_SPLIT_40",
                "UNTUNED_SPLIT_50",
                "UNTUNED_SPLIT_60",
            ] {
                if ui
                    .add_sized(
                        [ui.spacing().interact_size.x.max(56.0), chrome::HIT_SIDEBAR],
                        egui::Button::new(s),
                    )
                    .clicked()
                {
                    set_io = Some((port.clone(), "IN_TERM", s));
                }
            }
        });
    }
    if let Some((port, std)) = set_iostd {
        // ComboBox / button Set → set_iostandard (HNF + constraints); exec syncs the table.
        let _ = model.exec(&format!("set_property IOSTANDARD {std} [get_ports {port}]"));
    }
    if let Some((port, key, val)) = set_io {
        // DRIVE / SLEW / … Set → model.set_*; illegal HAD values Err (not silent wallpaper).
        let _ = model.exec(&format!("set_property {key} {val} [get_ports {port}]"));
    }
    if let Some((port, pin)) = set_pkg_pin {
        let _ = model.exec(&format!(
            "set_property PACKAGE_PIN {pin} [get_ports {port}]"
        ));
        model.package_pin_draft = pin;
    }
    if let Some(name) = pick_obj {
        let _ = model.select_io_port_object(&name);
    } else if let Some(name) = pick_port {
        let _ = model.select_io_port(&name);
    }
}

fn paint_pblocks_table(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.label(RichText::new("Pblocks").strong());
    ui.horizontal(|ui| {
        if ui.button("Create pblock").clicked() {
            let _ = model.exec("create_pblock");
        }
        if ui.button("Resize to clock region X1Y1").clicked() {
            let name = model
                .pblocks
                .first()
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "pblock_0".into());
            if model.pblocks.is_empty() {
                let _ = model.exec("create_pblock pblock_0");
            }
            let _ = model.exec(&format!("resize_pblock {name} -add CLOCKREGION_X1Y1"));
        }
        let pb_name = model
            .selected_pblock
            .clone()
            .or_else(|| model.pblocks.first().map(|p| p.name.clone()));
        if ui
            .add_enabled(pb_name.is_some(), egui::Button::new("Add design cells"))
            .on_hover_text("add_cells_to_pblock - assign all design LUT cells")
            .clicked()
        {
            if let Some(name) = pb_name.as_deref() {
                let _ = model.exec(&format!("add_cells_to_pblock {name}"));
            }
        }
        let sel_cell = model
            .selected
            .as_deref()
            .filter(|s| model.tree.has_cell(s))
            .map(|s| s.to_string());
        if ui
            .add_enabled(
                pb_name.is_some() && sel_cell.is_some(),
                egui::Button::new("Add selected cells"),
            )
            .on_hover_text("add_cells_to_pblock - assign the netlist selection")
            .clicked()
        {
            if let (Some(name), Some(cell)) = (pb_name.as_deref(), sel_cell.as_deref()) {
                let _ = model.exec(&format!("add_cells_to_pblock {name} {cell}"));
            }
        }
    });
    let selected = model.selected.clone();
    let selected_pb = model.selected_pblock.clone();
    let rows = model.pblock_rows().to_vec();
    let mut pick_pblock: Option<String> = None;
    let mut pick_obj: Option<String> = None;
    if rows.is_empty() {
        let screen = ui.ctx().screen_rect();
        let floor = chrome::device_window_narrow(screen.width())
            || chrome::device_window_short(screen.height());
        if floor {
            // One complete line. Horizontal scroll keeps the sentence; wrap would crush the die.
            ui.horizontal(|ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.set_min_width(420.0);
                ui.label("No pblocks yet. Create one, then resize it on the die.");
            });
        } else {
            ui.add(
                egui::Label::new("No pblocks yet. Create one, then resize it on the die.").wrap(),
            );
        }
        // Short leftover belongs to the die, not a dashed placeholder grid.
        if chrome::device_window_short(screen.height()) {
            return;
        }
    }
    data_scroll("pblocks_table_scroll").show(ui, |ui| {
        egui::Grid::new("pblocks_table")
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Range").strong());
                ui.label(RichText::new("Cells").strong());
                ui.label(RichText::new("Sites").strong());
                ui.label(RichText::new("Frames").strong());
                ui.label(RichText::new("Bytes").strong());
                ui.label(RichText::new("Objects").strong());
                ui.end_row();
                if rows.is_empty() {
                    ui.label("-");
                    ui.label("-");
                    ui.label("-");
                    ui.label("-");
                    ui.label("-");
                    ui.label("-");
                    ui.label("-");
                    ui.end_row();
                } else {
                    for p in &rows {
                        let obj = model.pblock_object_cell(p);
                        let on = selected_pb.as_deref() == Some(p.name.as_str())
                            || selected.as_deref() == Some(p.name.as_str());
                        let on_obj = selected.as_deref().is_some_and(|s| {
                            obj.split(',').any(|t| t == s) || p.cells.iter().any(|c| c == s)
                        });
                        if ui.selectable_label(on || on_obj, p.name.as_str()).clicked() {
                            if obj == "-" {
                                pick_pblock = Some(p.name.clone());
                            } else {
                                pick_obj = Some(p.name.clone());
                            }
                        }
                        if ui
                            .selectable_label(on, p.english_range().as_str())
                            .clicked()
                        {
                            if p.ranged {
                                pick_obj = Some(format!("CLB_X{}Y{}", p.x0, p.y0));
                            } else {
                                pick_pblock = Some(p.name.clone());
                            }
                        }
                        if ui.selectable_label(on, p.cells.len().to_string()).clicked() {
                            pick_pblock = Some(p.name.clone());
                        }
                        ui.label(p.site_count(&model.device.sites).to_string());
                        ui.label(p.frames.to_string());
                        ui.label(p.bytes.to_string());
                        if ui.selectable_label(on || on_obj, &obj).clicked() {
                            if obj == "-" {
                                pick_pblock = Some(p.name.clone());
                            } else {
                                pick_obj = Some(p.name.clone());
                            }
                        }
                        ui.end_row();
                    }
                }
            });
    });
    if let Some(name) = pick_obj {
        let _ = model.select_pblock_object(&name);
    } else if let Some(name) = pick_pblock {
        let _ = model.select_pblock(&name);
    }
}

fn paint_package(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("I/O Planning");
    egui::ScrollArea::vertical()
        .id_salt("package_tables")
        .auto_shrink([false, true])
        .max_height(chrome::DEVICE_TABLES_MAX_HEIGHT)
        .show(ui, |ui| {
            paint_io_ports_table(ui, model, "io_ports_package");
        });
    ui.separator();
    ui.label(RichText::new("Package").strong());
    ui.monospace(format!(
        "part={}  {}×{}  pins={}  assigned={}",
        model.package.part,
        model.package.cols,
        model.package.rows,
        model.package_pins.len(),
        model
            .package_pins
            .iter()
            .filter(|p| p.port.is_some())
            .count()
    ));
    ui.horizontal(|ui| {
        ui.label(RichText::new("unassigned").small().color(theme.faint));
        ui.label(RichText::new("placed port").small().color(theme.info));
        let mut seen = std::collections::HashSet::new();
        for p in &model.package_pins {
            if seen.insert(p.bank) {
                let (r, g, b) = p.bank_rgb();
                ui.label(RichText::new(format!("BANK{}", p.bank)).small().color(
                    Color32::from_rgb(
                        r.saturating_add(40),
                        g.saturating_add(40),
                        b.saturating_add(40),
                    ),
                ));
            }
        }
    });
    let cols_had = model.package.cols.max(1);
    let rows_had = model.package.rows.max(1);
    let n_pins = model.package_pins.len().max((cols_had * rows_had) as usize);
    let x0 = model.package.x0;
    let y0 = model.package.y0;
    let avail = ui.available_size();
    let view_h = chrome::pane_view_h(avail.y);
    let view_w = avail.x.max(80.0);
    // Wrap a 1-row HAD strip into a filled pin grid (Apple/Windows: content fills, no dead space).
    let (cols, rows, cell_w, cell_h) = chrome::stat_bit_grid(n_pins.max(1), view_w, view_h);
    let die_w = cell_w * cols as f32 + 28.0;
    let die_h = cell_h * rows as f32 + 16.0;
    let draw_w = view_w.max(die_w);
    let draw_h = view_h.max(die_h);
    let _ = (x0, y0);
    let mut pick: Option<String> = None;
    let selected = model.selected.clone();
    egui::ScrollArea::both()
        .id_salt("package_die_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(draw_w, draw_h), Sense::click());
            if ui.is_rect_visible(rect) {
                let origin = egui::pos2(rect.left() + 28.0, rect.top() + 4.0);
                let p = ui.painter();
                p.rect_filled(rect, 0.0, theme.die_bg);
                let rad = cell_w.min(cell_h) * 0.32;
                for (i, pin) in model.package_pins.iter().enumerate() {
                    let c = (i as u32) % cols;
                    let r = (i as u32) / cols;
                    if r >= rows {
                        break;
                    }
                    let px = origin.x + c as f32 * cell_w;
                    let py = origin.y + r as f32 * cell_h;
                    let center = egui::pos2(px + cell_w * 0.5, py + cell_h * 0.5);
                    let (br, bg, bb) = pin.bank_rgb();
                    let cell_rect = egui::Rect::from_min_size(
                        egui::pos2(px + 2.0, py + 2.0),
                        egui::vec2((cell_w - 4.0).max(8.0), (cell_h - 4.0).max(8.0)),
                    );
                    if !cell_rect.intersects(ui.clip_rect()) {
                        continue;
                    }
                    p.rect_filled(
                        cell_rect,
                        3.0,
                        Color32::from_rgba_unmultiplied(br, bg, bb, 70),
                    );
                    let on = selected.as_deref() == Some(pin.pin.as_str())
                        || pin.port.as_deref() == selected.as_deref();
                    let fill = if pin.port.is_some() {
                        theme.info
                    } else {
                        theme.line
                    };
                    p.circle_filled(center, rad, fill);
                    if on {
                        p.circle_stroke(center, rad + 2.0, Stroke::new(1.6_f32, theme.gold));
                    }
                    p.text(
                        egui::pos2(center.x, cell_rect.bottom() - 2.0),
                        egui::Align2::CENTER_BOTTOM,
                        &pin.pin,
                        egui::FontId::monospace(8.0),
                        theme.text,
                    );
                }
                if let Some(pos) = resp.hover_pos() {
                    let dx = ((pos.x - origin.x) / cell_w).floor() as i32;
                    let dy = ((pos.y - origin.y) / cell_h).floor() as i32;
                    if dx >= 0 && dy >= 0 {
                        let i = dy as u32 * cols + dx as u32;
                        if let Some(pin) = model.package_pins.get(i as usize) {
                            let tip = match pin.port.as_deref() {
                                Some(port) => format!("{}  {port}", pin.pin),
                                None => pin.pin.clone(),
                            };
                            resp.clone().on_hover_text(tip);
                        }
                    }
                }
            }
            if resp.clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let origin = egui::pos2(rect.left() + 28.0, rect.top() + 4.0);
                    let dx = ((pos.x - origin.x) / cell_w).floor() as i32;
                    let dy = ((pos.y - origin.y) / cell_h).floor() as i32;
                    if dx >= 0 && dy >= 0 {
                        let i = dy as u32 * cols + dx as u32;
                        if let Some(pin) = model.package_pins.get(i as usize) {
                            pick = Some(pin.pin.clone());
                        }
                    }
                }
            }
        });
    if let Some(pin) = pick {
        let selected_port = model
            .selected
            .clone()
            .filter(|s| model.io_ports.iter().any(|p| p.name == *s));
        let assigned = model
            .package_pins
            .iter()
            .find(|p| p.pin == pin)
            .and_then(|p| p.port.clone());
        if let (Some(port), true) = (selected_port, assigned.is_none()) {
            let _ = model.exec(&format!(
                "set_property PACKAGE_PIN {pin} [get_ports {port}]"
            ));
        } else {
            let _ = model.select_package_pin(&pin);
        }
    }
}

/// Constraints tab: the wizard takes over the pane while open.
fn paint_constraints_tab(ui: &mut egui::Ui, app: &mut HelionIde) {
    if app.constraints_wizard.is_some() {
        constraints_wizard_ui::paint(ui, app);
        return;
    }
    ui.horizontal(|ui| {
        if ui.button("🧭 Constraints Wizard…").clicked() {
            app.constraints_wizard = Some(constraints_wizard_ui::WizardUi::new(&app.model));
        }
        ui.weak("Guided: clocks → I/O delays → exceptions → SDC");
    });
    ui.add_space(4.0);
    paint_constraints(ui, &mut app.model);
}

fn paint_constraints(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Constraints");
    // Sibling/user SDC may already be loaded before the editor was painted.
    model.ensure_sdc_editor_populated();
    ui.horizontal(|ui| {
        ui.menu_button("Add…", |ui| {
            let recipes: &[(&str, &str)] = &[
                ("Clock 10 ns", "create_clock -period 10 clk"),
                (
                    "Input delay 1.5 ns",
                    "set_input_delay -clock clk 1.5 [get_ports clk]",
                ),
                (
                    "Output delay 2 ns",
                    "set_output_delay -clock clk 2.0 [get_ports led]",
                ),
                (
                    "False path",
                    "set_false_path -from [get_ports clk] -to [get_ports led]",
                ),
                ("Read counter.sdc", "__read_sdc__"),
            ];
            for (name, tcl) in recipes {
                if ui.button(*name).clicked() {
                    if *tcl == "__read_sdc__" {
                        let p = helion_device::Device::examples_dir().join("counter.sdc");
                        let _ = model.open_sdc_editor(&p);
                    } else {
                        let _ = model.exec(tcl);
                    }
                    ui.close();
                }
            }
        });
        // Label + target follow the actual file: the editor's current path,
        // else a sibling .sdc/.xdc on the sources list (same fallback as
        // ensure_sdc_editor_populated), else the counter example.
        let sdc_path = model
            .sdc_editor_path
            .clone()
            .or_else(|| {
                model.tree.sources.iter().find_map(|src| {
                    let q = PathBuf::from(src);
                    let ext = q
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    ((ext == "sdc" || ext == "xdc") && q.is_file()).then(|| q)
                })
            })
            .unwrap_or_else(|| helion_device::Device::examples_dir().join("counter.sdc"));
        let sdc_name = sdc_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("counter.sdc");
        if ui.button(format!("Open {sdc_name}")).clicked() {
            let _ = model.open_sdc_editor(&sdc_path);
        }
        if ui.button("Save").clicked() {
            let _ = model.save_sdc_editor();
        }
    });
    ui.add_space(6.0);
    // Vivado-shaped SDC/XDC text editor (monospace). Keep tables below.
    let file_label = model
        .sdc_editor_path
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("counter.sdc")
        .to_string();
    let dirty = model.sdc_editor_dirty;
    ui.horizontal(|ui| {
        ui.label(RichText::new(&file_label).strong().monospace());
        if dirty {
            ui.weak("•");
        }
    });
    let resp = ui.add(
        egui::TextEdit::multiline(&mut model.sdc_editor_text)
            .code_editor()
            .desired_rows(10)
            .desired_width(f32::INFINITY),
    );
    if resp.changed() {
        model.sdc_editor_dirty = true;
    }
    ui.add_space(6.0);
    paint_constraints_tables(ui, model);
}

fn paint_constraints_tables(ui: &mut egui::Ui, model: &mut IdeModel) {
    let rows = model.constraint_rows();
    if rows.is_empty() {
        ui.label("No constraints yet. Use Add… above to create a clock.");
        return;
    }
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    let sections = [
        (
            "Clocks (create_clock / create_generated_clock)",
            ConstraintSection::Clocks,
            "constraints_clocks",
        ),
        (
            "I/O Delay (set_input_delay / set_output_delay)",
            ConstraintSection::IoDelay,
            "constraints_io_delay",
        ),
        (
            "Exceptions (false path / multicycle / max_delay / min_delay / clock_groups / …)",
            ConstraintSection::Exception,
            "constraints_exceptions",
        ),
        (
            "I/O Placement (PACKAGE_PIN / IOSTANDARD / DRIVE / SLEW / …)",
            ConstraintSection::IoPlacement,
            "constraints_io_placement",
        ),
    ];
    for (title, section, grid) in sections {
        let sect: Vec<_> = rows.iter().filter(|r| r.section == section).collect();
        ui.add_space(6.0);
        ui.label(RichText::new(title).strong());
        if sect.is_empty() {
            ui.weak("-");
            continue;
        }
        egui::Grid::new(grid).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label(RichText::new("Name").strong());
            ui.label(RichText::new("Kind").strong());
            ui.label(RichText::new("From").strong());
            ui.label(RichText::new("To").strong());
            ui.label(RichText::new("Value").strong());
            ui.end_row();
            for r in sect {
                let on = selected.as_deref() == Some(r.id.as_str());
                let name = if r.name.is_empty() {
                    "-"
                } else {
                    r.name.as_str()
                };
                if ui.selectable_label(on, name).clicked() {
                    pick = Some(r.id.clone());
                }
                ui.label(&r.kind);
                let from = if r.from.is_empty() {
                    "-"
                } else {
                    r.from.as_str()
                };
                let to = if r.to.is_empty() { "-" } else { r.to.as_str() };
                ui.label(from);
                ui.label(to);
                ui.label(&r.value);
                ui.end_row();
            }
        });
    }
    if let Some(id) = pick {
        let _ = model.select_constraint(&id);
    }
}

// ---------------------------------------------------------------------------
// §3.8 spec tables: 11px dim semibold 28px headers, 27px rows, 1px dividers,
// hover highlight. Cells are painted with `painter.text` at explicit anchors
// because `ui.put`+`Label` re-sizes the label to its text, which defeats
// column alignment (numeric columns must right-align in tabular figures).
// ---------------------------------------------------------------------------

/// One cell of a §3.8 spec table.
struct SpecCell {
    text: String,
    color: Color32,
    right: bool,
    mono: bool,
}

impl SpecCell {
    fn text(s: impl Into<String>, color: Color32) -> Self {
        Self {
            text: s.into(),
            color,
            right: false,
            mono: false,
        }
    }
    fn num(s: impl Into<String>, color: Color32) -> Self {
        Self {
            text: s.into(),
            color,
            right: true,
            mono: true,
        }
    }
}

fn spec_paint_cells(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    widths: &[f32],
    cells: &[SpecCell],
    header: bool,
) {
    let mut x = rect.left();
    for (i, cell) in cells.iter().enumerate() {
        let cw = widths.get(i).copied().unwrap_or(60.0);
        let font = if header {
            theme::font_ui_semibold(11.0)
        } else if cell.mono {
            theme::font_code(13.0)
        } else {
            theme::font_ui(13.0)
        };
        let (anchor, px) = if cell.right {
            (egui::Align2::RIGHT_CENTER, x + cw - 8.0)
        } else {
            (egui::Align2::LEFT_CENTER, x + 8.0)
        };
        ui.painter()
            .text(egui::pos2(px, rect.center().y), anchor, &cell.text, font, cell.color);
        x += cw;
    }
}

/// 1px divider under a row rect.
/// Right-aligned monospace numeric cell for `egui::Grid` tables.
///
/// Vivado right-aligns every numeric column in monospace so values scan
/// vertically; the custom `SpecCell::num` painter already does this, this is
/// the grid-table equivalent. Use for all numeric grid cells.
fn num_cell(ui: &mut egui::Ui, text: &str) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.monospace(text);
    });
}

/// Selectable numeric cell: right-aligned monospace that can be clicked.
/// Returns true if clicked.
fn num_cell_selectable(ui: &mut egui::Ui, selected: bool, text: &str) -> bool {
    let mut clicked = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui
            .selectable_label(selected, egui::RichText::new(text).monospace())
            .clicked()
        {
            clicked = true;
        }
    });
    clicked
}

fn spec_divider(ui: &mut egui::Ui, theme: &theme::Theme, rect: egui::Rect) {
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        egui::Stroke::new(1.0, theme.line),
    );
}

/// §3.3 slack color rule: negative = danger, <10% of the clock period = warn,
/// otherwise plain text. Green is status-only - never for "good slack".
fn slack_color(theme: &theme::Theme, slack_ps: i64, period_ps: u64) -> Color32 {
    if slack_ps < 0 {
        theme.danger
    } else if (slack_ps as f64) < 0.10 * period_ps as f64 {
        theme.warn
    } else {
        theme.text
    }
}

/// Section header bar (§3.1 pattern): 32px tall, 13px semibold title left,
/// actions right-aligned, 1px bottom divider.
fn section_bar(
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    title: &str,
    actions: impl FnOnce(&mut egui::Ui),
) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 32.0), egui::Sense::hover());
    ui.painter().text(
        egui::pos2(rect.left() + 4.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        theme::font_ui_semibold(13.0),
        theme.text,
    );
    let act_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top()),
        egui::pos2(rect.right() - 4.0, rect.bottom()),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(act_rect), |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), actions);
    });
    spec_divider(ui, theme, rect);
}

fn paint_timing_paths(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    let mut do_report = false;
    let mut do_schematic = false;
    section_bar(ui, theme, "Timing Paths", |ui| {
        // Quartus-pattern filter field on dense tables (§3.8.6).
        ui.add(
            egui::TextEdit::singleline(&mut model.timing_path_filter)
                .hint_text("Filter...")
                .desired_width(170.0),
        );
        if ui.button("Report timing").clicked() {
            do_report = true;
        }
        // Explicit about the default: with no path selected, the worst path
        // (index 0) is shown. Disabled when there are no paths at all.
        let has_paths = !model.timing_paths.is_empty();
        let sch_tip = if model.selected_timing_path.is_some() {
            "Show the selected timing path in the schematic"
        } else {
            "Show the worst timing path (index 0) in the schematic"
        };
        if ui
            .add_enabled(has_paths, egui::Button::new("Show in Schematic"))
            .on_hover_text(if has_paths {
                sch_tip
            } else {
                "Run Report timing first"
            })
            .clicked()
        {
            do_schematic = true;
        }
    });
    if do_report {
        let _ = model.exec("report_timing");
    }
    if do_schematic {
        let spec = model
            .selected_timing_path
            .map(|i| i.to_string())
            .unwrap_or_else(|| "0".into());
        let _ = model.exec(&format!("select_timing_path {spec}"));
    }

    // Reverse cross-probe: an object selected in Device/Schematic surfaces
    // the timing paths through it here; click one to probe it.
    if model.selected_timing_path.is_none() {
        if let Some(id) = model.selected.clone() {
            let hits = model.paths_through(&id);
            if !hits.is_empty() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} timing path{} through",
                            hits.len(),
                            if hits.len() == 1 { "" } else { "s" }
                        ))
                        .size(12.0)
                        .color(theme.dim),
                    );
                    ui.label(
                        egui::RichText::new(&id)
                            .size(12.0)
                            .color(theme.text)
                            .strong(),
                    );
                });
                let mut probe_pick: Option<usize> = None;
                for &hi in &hits {
                    let row_label = {
                        let p = &model.timing_paths[hi];
                        format!(
                            "{} → {}   slack {} ps",
                            p.startpoint, p.endpoint, p.slack_ps
                        )
                    };
                    if ui.selectable_label(false, row_label).clicked() {
                        probe_pick = Some(hi);
                    }
                }
                if let Some(hi) = probe_pick {
                    model.set_probe(ProbeSelection::Path(hi));
                }
                ui.add_space(4.0);
            }
        }
    }

    if model.timing_paths.is_empty() {
        // Designed empty state (§3.8): centered dim text + action, never a bare rect.
        ui.add_space(28.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("No timing paths yet.")
                    .size(13.0)
                    .color(theme.dim),
            );
            ui.add_space(6.0);
            if primary_button(ui, "Report timing").clicked() {
                let _ = model.exec("report_timing");
            }
        });
        return;
    }

    // Endpoint slack histogram (§3.3.3) sits above the path table.
    ui.add_space(6.0);
    dashboard::paint_slack_histogram(ui, model, theme);
    ui.add_space(10.0);

    // Filter (From / To / group, case-insensitive) then sort.
    let filt = model.timing_path_filter.trim().to_lowercase();
    let mut order: Vec<usize> = (0..model.timing_paths.len()).collect();
    if !filt.is_empty() {
        order.retain(|&i| {
            let p = &model.timing_paths[i];
            p.startpoint.to_lowercase().contains(&filt)
                || p.endpoint.to_lowercase().contains(&filt)
                || p.group.to_lowercase().contains(&filt)
        });
    }
    if let Some((col, asc)) = model.timing_path_sort {
        order.sort_by(|&a, &b| {
            let (pa, pb) = (&model.timing_paths[a], &model.timing_paths[b]);
            let ord = match col {
                TimingPathSortCol::From => pa.startpoint.cmp(&pb.startpoint),
                TimingPathSortCol::To => pa.endpoint.cmp(&pb.endpoint),
                TimingPathSortCol::Slack => pa.slack_ps.cmp(&pb.slack_ps),
                TimingPathSortCol::Levels => pa.pins.len().cmp(&pb.pins.len()),
            };
            if asc {
                ord
            } else {
                ord.reverse()
            }
        });
    }

    if order.is_empty() {
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("No paths match the current filters.")
                    .size(13.0)
                    .color(theme.dim),
            );
        });
    } else {
        paint_timing_path_table(ui, model, theme, &order);
    }

    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("Double-click a row to cross-probe Device + Source.")
            .size(11.0)
            .color(theme.faint),
    );

    paint_timing_pin_detail(ui, model, theme);
}

/// §3.3 path table: # | From | To | Launch clock | Capture clock | Slack |
/// Levels | Path group. Sortable headers (▲▼), 27px rows, 1px dividers,
/// right-aligned tabular numerics.
#[allow(clippy::too_many_lines)]
fn paint_timing_path_table(
    ui: &mut egui::Ui,
    model: &mut IdeModel,
    theme: &theme::Theme,
    order: &[usize],
) {
    // (title, fixed width | None = fill, numeric, sort column)
    let cols: &[(&str, Option<f32>, bool, Option<TimingPathSortCol>)] = &[
        ("#", Some(40.0), true, None),
        ("From", None, false, Some(TimingPathSortCol::From)),
        ("To", None, false, Some(TimingPathSortCol::To)),
        ("Launch clock", Some(110.0), false, None),
        ("Capture clock", Some(110.0), false, None),
        ("Slack", Some(96.0), true, Some(TimingPathSortCol::Slack)),
        ("Levels", Some(64.0), true, Some(TimingPathSortCol::Levels)),
        ("Path group", Some(96.0), false, None),
    ];
    let w = ui.available_width();
    let fixed: f32 = cols.iter().filter_map(|c| c.1).sum();
    let n_fill = cols.iter().filter(|c| c.1.is_none()).count().max(1) as f32;
    let fill_w = ((w - fixed) / n_fill).max(80.0);
    let widths: Vec<f32> = cols.iter().map(|c| c.1.unwrap_or(fill_w)).collect();

    // Header row: 28px, 11px dim semibold, click to sort.
    let (hrect, _) = ui.allocate_exact_size(egui::vec2(w, 28.0), egui::Sense::hover());
    let mut x = hrect.left();
    let mut sort_click: Option<TimingPathSortCol> = None;
    for (i, (title, _, numeric, scol)) in cols.iter().enumerate() {
        let cw = widths[i];
        let crect =
            egui::Rect::from_min_size(egui::pos2(x, hrect.top()), egui::vec2(cw, 28.0));
        let mut label = (*title).to_string();
        if let Some(sc) = scol {
            if let Some((cur, asc)) = model.timing_path_sort {
                if cur == *sc {
                    label.push_str(if asc { " ▲" } else { " ▼" });
                }
            }
            let resp = ui.interact(crect, egui::Id::new(("tpathh", i)), egui::Sense::click());
            if resp.clicked() {
                sort_click = Some(*sc);
            }
            if resp.hovered() {
                ui.painter().rect_filled(crect, 0.0, theme.surface_hover);
            }
        }
        let (anchor, px) = if *numeric {
            (egui::Align2::RIGHT_CENTER, x + cw - 8.0)
        } else {
            (egui::Align2::LEFT_CENTER, x + 8.0)
        };
        ui.painter().text(
            egui::pos2(px, hrect.center().y),
            anchor,
            label,
            theme::font_ui_semibold(11.0),
            theme.dim,
        );
        x += cw;
    }
    spec_divider(ui, theme, hrect);
    if let Some(sc) = sort_click {
        model.timing_path_sort = Some(match model.timing_path_sort {
            Some((cur, asc)) if cur == sc => (sc, !asc),
            _ => (sc, true),
        });
    }

    // Rows.
    let clk = model
        .sta_clock_names()
        .first()
        .cloned()
        .unwrap_or_else(|| "-".into());
    let period = model.clock_period_ps;
    let selected = model.selected_timing_path;
    let mut pick: Option<usize> = None;
    let mut dbl: Option<usize> = None;
    for (ri, &mi) in order.iter().enumerate() {
        let (from, to, slack, levels, group) = {
            let p = &model.timing_paths[mi];
            (
                p.startpoint.clone(),
                p.endpoint.clone(),
                p.slack_ps,
                p.pins.len(),
                p.group.clone(),
            )
        };
        let (rrect, _) = ui.allocate_exact_size(egui::vec2(w, 27.0), egui::Sense::hover());
        if selected == Some(mi) {
            ui.painter().rect_filled(rrect, 0.0, theme.sel);
        } else if ui.rect_contains_pointer(rrect) {
            ui.painter().rect_filled(rrect, 0.0, theme.surface_hover);
        }
        let resp = ui.interact(rrect, egui::Id::new(("tpathr", mi)), egui::Sense::click());
        if resp.clicked() {
            pick = Some(mi);
        }
        if resp.double_clicked() {
            dbl = Some(mi);
        }
        let cells = vec![
            SpecCell::num((ri + 1).to_string(), theme.faint),
            SpecCell::text(from, theme.text),
            SpecCell::text(to, theme.text),
            SpecCell::text(clk.clone(), theme.dim),
            SpecCell::text(clk.clone(), theme.dim),
            SpecCell::num(slack.to_string(), slack_color(theme, slack, period)),
            SpecCell::num(levels.to_string(), theme.text),
            SpecCell::text(group, theme.dim),
        ];
        spec_paint_cells(ui, rrect, &widths, &cells, false);
        spec_divider(ui, theme, rrect);
    }
    // Empty-space click-to-clear starts below the last row (captured after rows).
    let bg_top = ui.min_rect().max.y;
    let table_id = egui::Id::new("timing_path_table");
    if let Some(mi) = pick {
        // Cross-probe: the path lights up in Device + Schematic, and the
        // pin-delay pane below lists its elements.
        model.set_probe(ProbeSelection::Path(mi));
        ui.ctx().memory_mut(|m| m.request_focus(table_id));
    }
    if let Some(mi) = dbl {
        let _ = model.exec(&format!("select_timing_path {mi}"));
    }
    // Clicking empty table space clears the probe.
    let bg_rect = egui::Rect::from_x_y_ranges(
        ui.min_rect().x_range(),
        bg_top..=ui.min_rect().max.y,
    );
    let bg_resp = ui.interact(bg_rect, egui::Id::new("tpath_bg"), egui::Sense::click());
    if bg_resp.clicked() && pick.is_none() && sort_click.is_none() {
        model.clear_probe();
    }
    // Keyboard: Up/Down walks the (sorted/filtered) path list, driving
    // Device + Schematic live; Esc clears. Gated on table focus so arrows
    // keep scrolling everywhere else.
    if ui.ctx().memory(|m| m.has_focus(table_id)) {
        let dir = if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
            1
        } else if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
            -1
        } else {
            0
        };
        if dir != 0 && !order.is_empty() {
            let cur = model
                .selected_timing_path
                .and_then(|mi| order.iter().position(|&x| x == mi));
            let next = match cur {
                Some(p) => (p as i32 + dir).clamp(0, order.len() as i32 - 1) as usize,
                None => {
                    if dir > 0 {
                        0
                    } else {
                        order.len() - 1
                    }
                }
            };
            model.set_probe(ProbeSelection::Path(order[next]));
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            model.clear_probe();
        }
    }
}

/// Pin-delay detail for the selected path, §3.8 styled.
fn paint_timing_pin_detail(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    let path = model
        .selected_timing_path
        .and_then(|i| model.timing_paths.get(i))
        .or_else(|| model.timing_paths.first())
        .cloned();
    let Some(path) = path else {
        return;
    };
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new("Pin Delay")
            .font(theme::font_ui_semibold(13.0))
            .color(theme.text),
    );
    ui.label(
        egui::RichText::new(format!(
            "{} - slack {} ps, delay {} ps (logic {} ps, net {} ps)",
            path.name,
            path.slack_ps,
            path.delay_ps,
            path.logic_ps(),
            path.net_ps()
        ))
        .size(12.0)
        .color(theme.dim),
    );
    ui.add_space(4.0);

    // Cross-probe element strip: ordered cells/nets with per-element
    // arrival; click one to probe it alone in Device + Schematic.
    let elements = model.probe_elements();
    if !elements.is_empty() {
        ui.label(
            egui::RichText::new("Path elements")
                .size(11.0)
                .color(theme.dim),
        );
        let mut el_pick: Option<String> = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (i, el) in elements.iter().enumerate() {
                if i > 0 {
                    ui.label(egui::RichText::new("→").size(11.0).color(theme.faint));
                }
                let chip = format!("{} {} · {}ps", el.kind, el.id, el.arrival_ps);
                let btn = egui::Button::new(
                    egui::RichText::new(chip).size(11.0).monospace().color(
                        if el.kind == "cell" {
                            theme.text
                        } else {
                            theme.info
                        },
                    ),
                )
                .small();
                if ui.add(btn).clicked() {
                    el_pick = Some(el.id.clone());
                }
            }
        });
        if let Some(id) = el_pick {
            model.set_probe(ProbeSelection::Object(id));
        }
        ui.add_space(4.0);
    }

    // (title, fixed width | None = fill, numeric)
    let cols: &[(&str, Option<f32>, bool)] = &[
        ("Name", None, false),
        ("Type", Some(110.0), false),
        ("Incr (ps)", Some(90.0), true),
        ("Path (ps)", Some(90.0), true),
        ("Net", None, false),
        ("Fanout", Some(70.0), true),
        ("Location", Some(150.0), false),
    ];
    let w = ui.available_width();
    let fixed: f32 = cols.iter().filter_map(|c| c.1).sum();
    let n_fill = cols.iter().filter(|c| c.1.is_none()).count().max(1) as f32;
    let fill_w = ((w - fixed) / n_fill).max(80.0);
    let widths: Vec<f32> = cols.iter().map(|c| c.1.unwrap_or(fill_w)).collect();

    let (hrect, _) = ui.allocate_exact_size(egui::vec2(w, 28.0), egui::Sense::hover());
    let cells: Vec<SpecCell> = cols
        .iter()
        .map(|(t, _, num)| SpecCell {
            text: (*t).to_string(),
            color: theme.dim,
            right: *num,
            mono: false,
        })
        .collect();
    spec_paint_cells(ui, hrect, &widths, &cells, true);
    spec_divider(ui, theme, hrect);

    let selected_pin = model.selected_timing_pin.clone();
    let mut pick_pin: Option<String> = None;
    egui::ScrollArea::vertical()
        .id_salt("timing_pin_details")
        .max_height(280.0)
        .hscroll(true)
        .show(ui, |ui| {
            for pin in &path.pins {
                let on = selected_pin.as_deref() == Some(pin.pin.as_str());
                let (rrect, _) =
                    ui.allocate_exact_size(egui::vec2(w, 27.0), egui::Sense::hover());
                if on {
                    ui.painter().rect_filled(rrect, 0.0, theme.sel);
                } else if ui.rect_contains_pointer(rrect) {
                    ui.painter().rect_filled(rrect, 0.0, theme.surface_hover);
                }
                let resp =
                    ui.interact(rrect, egui::Id::new(("tpathp", &pin.pin)), egui::Sense::click());
                if resp.clicked() {
                    pick_pin = Some(pin.pin.clone());
                }
                let net = if pin.net.is_empty() { "-" } else { pin.net.as_str() };
                let loc = if pin.location.is_empty() {
                    "-"
                } else {
                    pin.location.as_str()
                };
                let cells = vec![
                    SpecCell::text(pin.pin.clone(), theme.text),
                    SpecCell::text(pin.delay_type.clone(), theme.dim),
                    SpecCell::num(pin.incr_ps.to_string(), theme.text),
                    SpecCell::num(pin.path_ps.to_string(), theme.text),
                    SpecCell::text(net, theme.dim),
                    SpecCell::num(pin.fanout.to_string(), theme.text),
                    SpecCell::text(loc, theme.dim),
                ];
                spec_paint_cells(ui, rrect, &widths, &cells, false);
                spec_divider(ui, theme, rrect);
            }
        });
    if let Some(pin) = pick_pin {
        let _ = model.select_timing_pin(&pin);
    }
}

fn slack_label(v: Option<i64>) -> String {
    match v {
        Some(w) => w.to_string(),
        None => "n/a".into(),
    }
}

fn paint_timing_summary(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.label(RichText::new("Timing Summary").strong());
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("Report timing summary").clicked() {
            let _ = model.exec("report_timing_summary");
        }
        if ui.button("Write Report…").clicked() {
            let path = std::env::var("HELION_REPORT_PATH")
                .unwrap_or_else(|_| "/tmp/helion-reports/timing-summary.rpt".into());
            let _ = model.exec(&format!("write_report timing {path}"));
        }
    });
    if let Some(exp) = model.last_report_export.clone() {
        ui.label(RichText::new(exp).monospace().size(12.0).color(theme.dim));
    }
    // CLI-honest label. Closed WNS_PS= only with cells>0 and an Hff clock path.
    let honest = model.timing_honesty_label();
    let closed = model.timing_closed_wns();
    ui.label(
        RichText::new(honest.clone())
            .monospace()
            .size(16.0)
            .color(if closed { theme.ok } else { theme.gold }),
    );
    if !closed {
        ui.label("Not a closed WNS.");
        let pane = model.timing_text();
        if pane != honest {
            ui.label(RichText::new(pane).monospace().size(13.0));
        }
    }
    let report = model.timing_summary();
    if !closed {
        return;
    }
    if report.clocks.is_empty() {
        ui.label("No clocks yet.");
        if primary_button(ui, "Add clock 10 ns").clicked() {
            let _ = model.exec("create_clock -period 10 clk");
        }
        return;
    }
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("Design Timing Summary")
            .font(theme::font_ui_semibold(13.0))
            .color(theme.text),
    );
    // Full-word labels (§3.3.5) - never bare codes like WNS_PS.
    {
        let headers = [
            "Worst Negative Slack (WNS)",
            "Total Negative Slack (TNS)",
            "Worst Hold Slack (WHS)",
            "Total Hold Slack (THS)",
            "Failing Setup",
            "Failing Hold",
            "Endpoints",
        ];
        let widths = [220.0f32, 210.0, 200.0, 190.0, 130.0, 130.0, 110.0];
        let total: f32 = widths.iter().sum();
        let w = ui.available_width().max(total);
        let period = model.clock_period_ps;
        egui::ScrollArea::horizontal().id_salt("timing_summary_hscroll").show(ui, |ui| {
            let (hrect, _) =
                ui.allocate_exact_size(egui::vec2(w, 28.0), egui::Sense::hover());
            let hcells: Vec<SpecCell> = headers
                .iter()
                .map(|t| SpecCell {
                    text: (*t).to_string(),
                    color: theme.dim,
                    right: true,
                    mono: false,
                })
                .collect();
            spec_paint_cells(ui, hrect, &widths, &hcells, true);
            spec_divider(ui, theme, hrect);
            let (rrect, _) =
                ui.allocate_exact_size(egui::vec2(w, 27.0), egui::Sense::hover());
            let wns_c = report
                .wns_ps
                .map(|v| slack_color(theme, v, period))
                .unwrap_or(theme.dim);
            let whs_c = report
                .whs_ps
                .map(|v| slack_color(theme, v, period))
                .unwrap_or(theme.dim);
            let rcells = vec![
                SpecCell::num(slack_label(report.wns_ps), wns_c),
                SpecCell::num(report.tns_ps.to_string(), theme.text),
                SpecCell::num(slack_label(report.whs_ps), whs_c),
                SpecCell::num(report.ths_ps.to_string(), theme.text),
                SpecCell::num(report.failing_setup.to_string(), theme.text),
                SpecCell::num(report.failing_hold.to_string(), theme.text),
                SpecCell::num(report.endpoints.to_string(), theme.text),
            ];
            spec_paint_cells(ui, rrect, &widths, &rcells, false);
            spec_divider(ui, theme, rrect);
        });
    }
    let selected_ts = model.selected_timing_summary.clone();
    let selected = model.selected.clone();
    let mut pick: Option<(String, Option<String>)> = None;
    let mut pick_obj: Option<String> = None;
    let sections = [
        ("Intra-Clock Paths", PathGroupKind::IntraClock),
        ("Inter-Clock Paths", PathGroupKind::InterClock),
        ("Other Path Groups", PathGroupKind::Other),
    ];
    for (title, kind) in sections {
        let rows: Vec<_> = report.groups.iter().filter(|g| g.kind == kind).collect();
        if rows.is_empty() {
            continue;
        }
        ui.add_space(6.0);
        ui.label(RichText::new(title).strong());
        let path_col = chrome::stretched_col_w_gap(8, ui.available_width(), 8.0);
        egui::Grid::new(format!("timing_summary_{}", kind.as_str()))
            .spacing([8.0, 4.0])
            .min_col_width(path_col)
            .show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("From").strong());
                ui.label(RichText::new("To").strong());
                ui.label(RichText::new("Worst Negative Slack (WNS)").strong());
                ui.label(RichText::new("Total Negative Slack (TNS)").strong());
                ui.label(RichText::new("Worst Hold Slack (WHS)").strong());
                ui.label(RichText::new("Total Hold Slack (THS)").strong());
                ui.label(RichText::new("Endpoints").strong());
                ui.end_row();
                for g in rows {
                    let key = if kind == PathGroupKind::Other {
                        g.name.clone()
                    } else {
                        format!("{}->{}", g.from, g.to)
                    };
                    let on = selected_ts.as_deref() == Some(key.as_str());
                    let name_lbl = if on {
                        RichText::new(&g.name).strong()
                    } else {
                        RichText::new(&g.name)
                    };
                    if ui.selectable_label(on, name_lbl).clicked() {
                        pick = Some(if kind == PathGroupKind::Other {
                            (g.name.clone(), None)
                        } else {
                            (g.from.clone(), Some(g.to.clone()))
                        });
                    }
                    let on_from = selected.as_deref() == Some(g.from.as_str());
                    if ui.selectable_label(on || on_from, &g.from).clicked() {
                        pick_obj = Some(g.from.clone());
                    }
                    let on_to = selected.as_deref() == Some(g.to.as_str());
                    if ui.selectable_label(on || on_to, &g.to).clicked() {
                        pick_obj = Some(g.to.clone());
                    }
                    ui.label(slack_label(g.wns_ps));
                    ui.label(g.tns_ps.to_string());
                    ui.label(slack_label(g.whs_ps));
                    ui.label(g.ths_ps.to_string());
                    ui.label(g.endpoints.to_string());
                    ui.end_row();
                }
            });
    }
    if let Some(name) = pick_obj {
        let _ = model.select_timing_summary_object(&name);
    } else if let Some((a, b)) = pick {
        let _ = model.select_timing_summary(&a, b.as_deref());
    }
}

fn clock_relation_color(theme: &theme::Theme, rel: ClockRelation) -> Color32 {
    match rel {
        ClockRelation::Timed => theme.ok,
        ClockRelation::TimedGenerated => theme.info,
        ClockRelation::TimedUnsafe => theme.warn,
        ClockRelation::TimedDatapath => theme.warn,
        ClockRelation::FalsePath => theme.faint,
        ClockRelation::PartialFalsePath => theme.warn,
        ClockRelation::Asynchronous => theme.violet,
        ClockRelation::Exclusive => theme.info,
        ClockRelation::NoPaths => theme.faint,
    }
}

fn paint_clock_interaction(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("Clock Interaction");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Report clock interaction").clicked() {
            let _ = model.exec("report_clock_interaction");
        }
    });
    ui.add_space(6.0);
    let report = model.clock_interaction();
    if report.clocks.is_empty() {
        ui.label("No clocks yet.");
        if primary_button(ui, "Add clock 10 ns").clicked() {
            let _ = model.exec("create_clock -period 10 clk");
        }
        return;
    }
    let selected_ci = model.selected_clock_interaction.clone();
    let selected = model.selected.clone();
    let mut pick: Option<(String, String)> = None;
    let mut pick_obj: Option<String> = None;
    egui::ScrollArea::both().id_salt("clock_interaction_scroll").show(ui, |ui| {
        // Fixed 120px columns (Vivado matrix): headers center over their
        // cells instead of ragged content-sized labels.
        const CELL_W: f32 = 120.0;
        const HEAD_H: f32 = 28.0;
        egui::Grid::new("clock_interaction_matrix")
            .spacing([4.0, 4.0])
            .show(ui, |ui| {
                ui.allocate_ui(egui::vec2(CELL_W, HEAD_H), |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("From \\ To").strong());
                    });
                });
                for c in &report.clocks {
                    let src = c.source.split('/').next().unwrap_or(c.source.as_str());
                    let on_obj = selected.as_deref() == Some(src)
                        || selected.as_deref() == Some(c.name.as_str());
                    ui.allocate_ui(egui::vec2(CELL_W, HEAD_H), |ui| {
                        ui.centered_and_justified(|ui| {
                            if ui
                                .selectable_label(on_obj, RichText::new(&c.name).strong())
                                .clicked()
                            {
                                pick_obj = Some(c.name.clone());
                            }
                        });
                    });
                }
                ui.end_row();
                for from in &report.clocks {
                    let src = from
                        .source
                        .split('/')
                        .next()
                        .unwrap_or(from.source.as_str());
                    let on_obj = selected.as_deref() == Some(src)
                        || selected.as_deref() == Some(from.name.as_str());
                    ui.allocate_ui(egui::vec2(CELL_W, HEAD_H), |ui| {
                        ui.centered_and_justified(|ui| {
                            if ui
                                .selectable_label(on_obj, RichText::new(&from.name).strong())
                                .clicked()
                            {
                                pick_obj = Some(from.name.clone());
                            }
                        });
                    });
                    for to in &report.clocks {
                        if let Some(cell) = report.cell(&from.name, &to.name) {
                            let key = format!("{}->{}", cell.from, cell.to);
                            let on = selected_ci.as_deref() == Some(key.as_str());
                            let fill = clock_relation_color(theme, cell.relation);
                            let wns = cell
                                .wns_ps
                                .map(|w| format!(" WNS_PS={w}"))
                                .unwrap_or_default();
                            let label = format!("{}{wns}", cell.relation.as_str());
                            let btn = egui::Button::new(RichText::new(label).color(Color32::BLACK))
                                .fill(fill)
                                .selected(on);
                            let resp = ui.add_sized([120.0, 40.0], btn);
                            if resp.clicked() {
                                pick = Some((cell.from.clone(), cell.to.clone()));
                            }
                            resp.on_hover_text(format!(
                                "FROM={} TO={} {} COMMON_PS={} REQ_PS={} paths={} OBJECTS={}",
                                cell.from,
                                cell.to,
                                cell.relation.as_str(),
                                cell.common_period_ps,
                                cell.requirement_ps,
                                cell.path_count,
                                src
                            ));
                        } else {
                            ui.label("-");
                        }
                    }
                    ui.end_row();
                }
            });
    });
    if let Some(name) = pick_obj {
        let _ = model.select_clock_interaction_object(&name);
    } else if let Some((from, to)) = pick {
        let _ = model.select_clock_interaction(&from, &to);
    }
}

fn cdc_severity_color(theme: &theme::Theme, sev: CdcSeverity) -> Color32 {
    match sev {
        CdcSeverity::Critical => theme.danger,
        CdcSeverity::Warning => theme.warn,
        CdcSeverity::Info => theme.violet,
        CdcSeverity::Safe => theme.ok,
    }
}

fn paint_cdc(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("CDC");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Report CDC").clicked() {
            let _ = model.exec("report_cdc");
        }
    });
    ui.add_space(6.0);
    let report = model.cdc_report();
    if report.clocks.is_empty() {
        ui.label("No clocks yet.");
        if primary_button(ui, "Add clock 10 ns").clicked() {
            let _ = model.exec("create_clock -period 10 clk");
        }
        return;
    }
    // Labeled summary chips, not a key=value dump (Vivado-style summary block).
    ui.horizontal(|ui| {
        ui.label(RichText::new("Critical").color(theme.faint).small());
        ui.label(
            RichText::new(report.critical_count().to_string())
                .color(theme.danger)
                .strong(),
        );
        ui.label(RichText::new("Warning").color(theme.faint).small());
        ui.label(
            RichText::new(report.warning_count().to_string())
                .color(theme.warn)
                .strong(),
        );
        ui.label(RichText::new("Info").color(theme.faint).small());
        ui.label(
            RichText::new(report.info_count().to_string())
                .color(theme.info)
                .strong(),
        );
        ui.label(RichText::new("Safe").color(theme.faint).small());
        ui.label(
            RichText::new(report.safe_count().to_string())
                .color(theme.ok)
                .strong(),
        );
    });
    let selected_cdc = model.selected_cdc.clone();
    let selected = model.selected.clone();
    let mut pick: Option<(String, String)> = None;
    let mut pick_obj: Option<(String, String)> = None;
    if report.violations.is_empty() {
        if paint_remaining_cta(ui, "No CDC violations.", "Run CDC analysis", theme) {
            let _ = model.exec("report_cdc");
        }
        return;
    }
    egui::ScrollArea::both().id_salt("cdc_scroll").show(ui, |ui| {
        egui::Grid::new("cdc_table")
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("From").strong());
                ui.label(RichText::new("To").strong());
                ui.label(RichText::new("Severity").strong());
                ui.label(RichText::new("Check").strong());
                ui.label(RichText::new("Sync").strong());
                ui.label(RichText::new("Endpoints").strong());
                ui.label(RichText::new("Objects").strong());
                ui.label(RichText::new("Worst Slack (ps)").strong());
                ui.label(RichText::new("Relation").strong());
                ui.end_row();
                for v in &report.violations {
                    let key = format!("{}->{}", v.from, v.to);
                    let on = selected_cdc.as_deref() == Some(key.as_str());
                    let fill = cdc_severity_color(theme, v.severity);
                    let btn = egui::Button::new(RichText::new(&v.from).color(Color32::BLACK))
                        .fill(fill)
                        .selected(on);
                    if ui.add(btn).clicked() {
                        pick = Some((v.from.clone(), v.to.clone()));
                    }
                    if ui.selectable_label(on, &v.to).clicked() {
                        pick = Some((v.from.clone(), v.to.clone()));
                    }
                    ui.label(v.severity.as_str());
                    ui.label(&v.check);
                    ui.label(if v.synchronizer { "1" } else { "0" });
                    ui.label(v.endpoints.to_string());
                    let clk = report.clocks.iter().find(|c| c.name == v.from);
                    let obj = clk
                        .map(|c| {
                            let src = c.source.split('/').next().unwrap_or(c.source.as_str());
                            if src.is_empty() {
                                v.from.as_str()
                            } else {
                                src
                            }
                        })
                        .unwrap_or(v.from.as_str());
                    let on_obj = selected.as_deref() == Some(obj);
                    if ui.selectable_label(on || on_obj, obj).clicked() {
                        pick_obj = Some((v.from.clone(), v.to.clone()));
                    }
                    ui.label(slack_label(v.wns_ps));
                    ui.label(v.relation.as_str());
                    ui.end_row();
                }
            });
    });
    if let Some((from, to)) = pick_obj {
        let _ = model.select_cdc(&from, &to);
        let _ = model.select_cdc_object(&format!("{from} {to}"));
    } else if let Some((from, to)) = pick {
        let _ = model.select_cdc(&from, &to);
    }
}

fn paint_clock_networks(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Clock Networks");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Report clock networks").clicked() {
            let _ = model.exec("report_clock_networks");
        }
    });
    ui.add_space(6.0);
    let report = model.clock_networks();
    if report.clocks.is_empty() {
        ui.label("No clocks yet.");
        if primary_button(ui, "Add clock 10 ns").clicked() {
            let _ = model.exec("create_clock -period 10 clk");
        }
        return;
    }
    ui.label(format!(
        "loads={} buffers={} INSERTION_PS={}",
        report.total_loads, report.total_buffers, report.max_insertion_ps
    ));
    let selected_cn = model.selected_clock_network.clone();
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    let mut pick_obj: Option<String> = None;
    egui::Grid::new("clock_networks_table")
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(RichText::new("Clock").strong());
            ui.label(RichText::new("Period_ps").strong());
            ui.label(RichText::new("Source").strong());
            ui.label(RichText::new("Net").strong());
            ui.label(RichText::new("Loads").strong());
            ui.label(RichText::new("Buffers").strong());
            ui.label(RichText::new("Fanout").strong());
            ui.label(RichText::new("Insertion_ps").strong());
            ui.end_row();
            for c in &report.clocks {
                let on = selected_cn.as_deref() == Some(c.name.as_str());
                if ui.selectable_label(on, &c.name).clicked() {
                    pick = Some(c.name.clone());
                }
                ui.label(c.period_ps.to_string());
                let src = c.source.split('/').next().unwrap_or(c.source.as_str());
                let on_src = selected.as_deref() == Some(src);
                if ui.selectable_label(on || on_src, &c.source).clicked() {
                    pick_obj = Some(c.name.clone());
                }
                let on_net = selected.as_deref() == Some(c.net.as_str());
                if ui.selectable_label(on || on_net, &c.net).clicked() {
                    pick_obj = Some(c.name.clone());
                }
                ui.label(c.n_loads.to_string());
                ui.label(c.n_buffers.to_string());
                ui.label(c.fanout.to_string());
                ui.label(c.insertion_ps.to_string());
                ui.end_row();
            }
        });
    if let Some(name) = pick_obj {
        let _ = model.select_clock_network(&name);
        let _ = model.select_clock_network_object(&name);
    } else if let Some(name) = pick {
        let _ = model.select_clock_network(&name);
    }
}

fn paint_power(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("Power");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Report power").clicked() {
            let _ = model.exec("report_power");
        }
    });
    ui.add_space(6.0);
    let report = model.power_report();
    if report.part.is_empty() {
        if paint_remaining_cta(ui, "No design yet.", "Run Synthesis", theme) {
            queue_flow(FlowStep::Synthesis);
        }
        return;
    }
    paint_vcd_power_section(ui, model, theme);
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);
    ui.label(RichText::new("Rails (vectorless STA)").strong());
    ui.add_space(4.0);
    ui.label(format!(
        "part={} VOLTAGE_MV={} TEMP_C={} F_MHZ={}",
        report.part, report.voltage_mv, report.temperature_c, report.f_mhz
    ));
    let selected_pwr = model.selected_power.clone();
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    let mut pick_blk: Option<String> = None;
    let rails = [
        ("total", report.total_uw),
        ("static", report.static_uw),
        ("dynamic", report.dynamic_uw),
        ("clocks", report.clocks_uw),
        ("logic", report.logic_uw),
        ("signals", report.signals_uw),
        ("io", report.io_uw),
        ("bram", report.bram_uw),
        ("dsp", report.dsp_uw),
    ];
    let blocks = model.power_block_rows();
    let n_rows = rails.len() + 1 + blocks.len() + 1;
    let remain = ui.available_size();
    let bbox = chrome::occupancy_table_bbox(n_rows.max(1), remain.x.max(80.0), remain.y.max(120.0));
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(bbox.drawn_w.max(remain.x), bbox.drawn_h.max(remain.y)),
        Sense::hover(),
    );
    ui.painter().rect_filled(rect, 0.0, theme.die_bg);
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(8.0)), |ui| {
        let remain_h = (ui.available_height() - 40.0).max(chrome::OCCUPANCY_BAR_H);
        let bar_h = chrome::occupancy_bar_h(n_rows.max(1), remain_h);
        let row_gap = 4.0;
        let max_uw = report.total_uw.max(1);
        let bar_span = chrome::occupancy_bar_w(
            (ui.available_width() - chrome::occupancy_label_reserve(2)).max(80.0),
        );
        egui::ScrollArea::both()
            .id_salt("ug907_power")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("power_rails")
                    .spacing([8.0, row_gap])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Rail").strong());
                        ui.label(RichText::new("UW").strong());
                        ui.label(RichText::new("Share").strong());
                        ui.end_row();
                        for (name, uw) in rails {
                            let on = selected_pwr.as_deref() == Some(name);
                            if ui.selectable_label(on, name).clicked() {
                                pick = Some(name.into());
                            }
                            ui.label(uw.to_string());
                            let frac = uw as f32 / max_uw as f32;
                            let (bar, _) =
                                ui.allocate_exact_size(egui::vec2(bar_span, bar_h), Sense::hover());
                            ui.painter().rect_filled(bar, 2.0, theme.surface);
                            let fill =
                                bar.with_max_x(bar.left() + bar.width() * frac.clamp(0.0, 1.0));
                            ui.painter().rect_filled(fill, 2.0, theme.info);
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
                ui.label(RichText::new("Utilization Details").strong());
                let details_bar = chrome::occupancy_bar_w(
                    (ui.available_width() - chrome::occupancy_label_reserve(3)).max(80.0),
                );
                egui::Grid::new("power_blocks")
                    .spacing([8.0, row_gap])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Block").strong());
                        ui.label(RichText::new("Used").strong());
                        ui.label(RichText::new("Available").strong());
                        ui.label(RichText::new("Occupancy").strong());
                        ui.end_row();
                        for (name, used, avail, rail) in &blocks {
                            let on = selected_pwr.as_deref() == Some(*rail)
                                || selected.as_deref() == Some(name.as_str());
                            if ui.selectable_label(on, name).clicked() {
                                pick_blk = Some((*rail).into());
                            }
                            ui.label(used.to_string());
                            ui.label(avail.to_string());
                            let frac = if *avail == 0 {
                                0.0
                            } else {
                                *used as f32 / *avail as f32
                            };
                            let (bar, _) = ui.allocate_exact_size(
                                egui::vec2(details_bar, bar_h),
                                Sense::hover(),
                            );
                            ui.painter().rect_filled(bar, 2.0, theme.surface);
                            let fill =
                                bar.with_max_x(bar.left() + bar.width() * frac.clamp(0.0, 1.0));
                            ui.painter().rect_filled(fill, 2.0, theme.info);
                            ui.end_row();
                        }
                    });
            });
    });
    if let Some(rail) = pick {
        let _ = model.select_power(&rail);
    }
    if let Some(rail) = pick_blk {
        let _ = model.select_power(&rail);
    }
}

/// VCD-driven switching-activity power section of the Power tab.
///
/// Mode badge (VCD-driven vs vectorless estimate), load/dump/clear actions,
/// total/static/dynamic cards, per-resource bars, per-hierarchy table,
/// top-20 toggling nets, and activity-annotated findings. Capacitances are
/// modeled, not silicon-measured - see `helion_gui::power`.
fn paint_vcd_power_section(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.label(RichText::new("Switching activity").strong());
    ui.horizontal_wrapped(|ui| {
        if ui.button("Load VCD…").clicked() {
            if let Some(p) = open_vcd_dialog() {
                let _ = model.read_vcd(&p.to_string_lossy());
            }
        }
        if ui.button("Dump VCD from sim").clicked() {
            surface::request_job(JobKind::DumpVcd(256));
        }
        if model.vcd_activity.is_some() && ui.button("Clear VCD").clicked() {
            let _ = model.exec("clear_vcd");
        }
    });
    ui.add_space(4.0);
    if let Some(err) = model.vcd_error.clone() {
        ui.label(RichText::new(format!("VCD error: {err}")).color(theme.danger));
        ui.add_space(4.0);
    }
    let Some(r) = model.vcd_power_report() else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        if r.vectorless {
            ui.label(
                RichText::new("● vectorless estimate")
                    .color(theme.warn)
                    .strong(),
            );
        } else {
            ui.label(RichText::new("● VCD-driven").color(theme.ok).strong());
        }
        if let Some(p) = &r.vcd_path {
            let short = p.rsplit('/').next().unwrap_or(p);
            ui.label(RichText::new(short).color(theme.dim).monospace())
                .on_hover_text(p);
        }
        ui.label(
            RichText::new(format!("{} cycles", r.cycles))
                .color(theme.dim)
                .small(),
        );
    });
    if r.vectorless {
        ui.label(
            RichText::new(
                "Default toggle rates - load a VCD for measured switching activity. \
                 Capacitances are modeled, not silicon-measured.",
            )
            .color(theme.dim)
            .small(),
        );
    } else {
        ui.label(
            RichText::new(format!(
                "activity coverage: {}/{} nets ({:.0}%) · clock α={:.2}",
                r.nets_matched,
                r.nets_total,
                r.coverage_pct(),
                r.clock_alpha
            ))
            .color(theme.dim)
            .small(),
        );
        if !r.unmatched_signals.is_empty() {
            let mut shown = r
                .unmatched_signals
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            if r.unmatched_count > r.unmatched_signals.len() {
                shown.push_str(&format!(
                    " (+{} more)",
                    r.unmatched_count - r.unmatched_signals.len()
                ));
            }
            ui.label(
                RichText::new(format!("unmatched VCD signals: {shown}"))
                    .color(theme.dim)
                    .small(),
            )
            .on_hover_text(
                "VCD signals that matched no design net - check hierarchical scope or net names",
            );
        }
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        for (label, mw) in [
            ("Total", r.total_mw),
            ("Static", r.static_mw),
            ("Dynamic", r.dynamic_mw),
        ] {
            ui.vertical(|ui| {
                ui.label(RichText::new(format!("{mw:.3} mW")).strong().size(18.0));
                ui.label(RichText::new(label).color(theme.dim).small());
            });
            ui.add_space(28.0);
        }
        ui.vertical(|ui| {
            ui.label(
                RichText::new(format!("{} mV · {} °C", r.voltage_mv, r.temperature_c)).strong(),
            );
            ui.label(
                RichText::new("operating conditions").color(theme.dim).small(),
            );
        });
    });
    ui.add_space(8.0);
    ui.label(RichText::new("By resource").strong());
    let max_mw = r
        .by_resource
        .iter()
        .map(|x| x.mw)
        .fold(0.0f64, f64::max)
        .max(1e-9);
    egui::Grid::new("vcd_power_resources")
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            for res in &r.by_resource {
                ui.label(RichText::new(&res.name).monospace());
                num_cell(ui, &format!("{:.3} mW", res.mw));
                let (bar, _) = ui.allocate_exact_size(egui::vec2(160.0, 10.0), Sense::hover());
                ui.painter().rect_filled(bar, 2.0, theme.surface);
                let frac = (res.mw / max_mw).clamp(0.0, 1.0) as f32;
                let fill = bar.with_max_x(bar.left() + bar.width() * frac);
                ui.painter().rect_filled(fill, 2.0, theme.info);
                ui.end_row();
            }
        });
    if !r.by_hierarchy.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("By hierarchy").strong());
        egui::Grid::new("vcd_power_hier")
            .spacing([8.0, 2.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(RichText::new("Block").strong());
                ui.label(RichText::new("Cells").strong());
                ui.label(RichText::new("mW").strong());
                ui.end_row();
                for h in r.by_hierarchy.iter().take(8) {
                    ui.label(RichText::new(&h.name).monospace());
                    ui.label(h.cells.to_string());
                    ui.label(format!("{:.3}", h.mw));
                    ui.end_row();
                }
            });
    }
    ui.add_space(8.0);
    ui.label(RichText::new("Top toggling nets").strong());
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .id_salt("vcd_top_nets")
        .show(ui, |ui| {
            egui::Grid::new("vcd_top_nets_grid")
                .spacing([8.0, 2.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.label(RichText::new("Net").strong());
                    ui.label(RichText::new("Toggles").strong());
                    ui.label(RichText::new("α").strong());
                    ui.label(RichText::new("mW").strong());
                    ui.end_row();
                    for n in &r.top_nets {
                        let name = if n.is_clock {
                            RichText::new(format!("{} (clock)", n.name))
                                .monospace()
                                .color(theme.info)
                        } else {
                            RichText::new(&n.name).monospace()
                        };
                        // Click a hot net to cross-probe it in the schematic
                        // (device routes highlighted).
                        if ui
                            .selectable_label(false, name)
                            .on_hover_text("Cross-probe in schematic")
                            .clicked()
                        {
                            model.select_vcd_net(&n.name);
                        }
                        ui.label(n.toggles.to_string());
                        ui.label(format!("{:.2}", n.activity));
                        ui.label(format!("{:.4}", n.mw));
                        ui.end_row();
                    }
                });
        });
    if !r.warnings.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("Findings").strong());
        for w in &r.warnings {
            let color = match w.severity {
                PowerSeverity::Critical => theme.danger,
                PowerSeverity::Warning => theme.warn,
                PowerSeverity::Info => theme.dim,
            };
            ui.label(RichText::new(format!("• {}", w.message)).color(color));
        }
    }
}

fn methodology_severity_color(theme: &theme::Theme, sev: MethodologySeverity) -> Color32 {
    match sev {
        MethodologySeverity::Error => theme.danger,
        MethodologySeverity::CriticalWarning => theme.warn,
        MethodologySeverity::Warning => theme.warn,
        MethodologySeverity::Advisory => theme.violet,
    }
}

fn paint_methodology(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("Methodology");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Report methodology").clicked() {
            let _ = model.exec("report_methodology");
        }
        // TIMING-7 (and TIMING-6): Fix applies Vivado-shaped set_*_delay,
        // persists via save_sdc_editor, and opens Constraints; Jump inserts
        // the template ready to Save (disk write is Apply-only).
        if let Some(id) = model.selected_methodology.clone() {
            if model.methodology_fix_template(&id).is_some() {
                let fix_label = if id == "TIMING-7" {
                    "Apply set_output_delay"
                } else if id == "TIMING-6" {
                    "Apply set_input_delay"
                } else {
                    "Fix"
                };
                if ui.button(fix_label).clicked() {
                    let _ = model.fix_methodology(&id);
                }
                if ui.button("Jump to Constraints").clicked() {
                    let _ = model.goto_methodology_constraints(&id);
                }
            }
        }
    });
    ui.add_space(6.0);
    if model.tree.top.is_none() {
        if paint_remaining_cta(ui, "No design yet.", "Run Synthesis", theme) {
            queue_flow(FlowStep::Synthesis);
        }
        return;
    }
    let report = model.methodology_report();
    // Labeled summary chips, not a key=value dump.
    ui.horizontal(|ui| {
        ui.label(RichText::new("Checks").color(theme.faint).small());
        ui.label(RichText::new(report.checks.len().to_string()).strong());
        ui.label(RichText::new("Errors").color(theme.faint).small());
        ui.label(
            RichText::new(report.error_count().to_string())
                .color(theme.danger)
                .strong(),
        );
        ui.label(RichText::new("Critical").color(theme.faint).small());
        ui.label(
            RichText::new(report.critical_count().to_string())
                .color(theme.danger)
                .strong(),
        );
        ui.label(RichText::new("Warning").color(theme.faint).small());
        ui.label(
            RichText::new(report.warning_count().to_string())
                .color(theme.warn)
                .strong(),
        );
        ui.label(RichText::new("Advisory").color(theme.faint).small());
        ui.label(
            RichText::new(report.advisory_count().to_string())
                .color(theme.info)
                .strong(),
        );
    });
    let selected_meth = model.selected_methodology.clone();
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    let mut pick_obj: Option<String> = None;
    let n = report.checks.len().max(1);
    let remain = ui.available_size();
    let bbox = chrome::nv_table_bbox(n, remain.x.max(80.0), remain.y.max(120.0));
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(bbox.drawn_w.max(remain.x), bbox.drawn_h.max(remain.y)),
        Sense::hover(),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(8.0)), |ui| {
        let n_f = n as f32;
        let row_gap = ((ui.available_height() - 28.0) / n_f - 18.0).clamp(4.0, 28.0);
        // ID (severity-colored button) | Category | Objects | Message. The old
        // Severity text column duplicated the button's encoding.
        let col_w = chrome::stretched_col_w_gap(4, ui.available_width(), 8.0);
        egui::ScrollArea::both().id_salt("methodology_scroll").show(ui, |ui| {
            egui::Grid::new("methodology_table")
                .spacing([8.0, row_gap])
                .min_col_width(col_w)
                .show(ui, |ui| {
                    ui.label(RichText::new("ID").strong());
                    ui.label(RichText::new("Category").strong());
                    ui.label(RichText::new("Objects").strong());
                    ui.label(RichText::new("Message").strong());
                    ui.end_row();
                    for v in &report.checks {
                        let on = selected_meth.as_deref() == Some(v.id.as_str());
                        let fill = methodology_severity_color(theme, v.severity);
                        let btn = egui::Button::new(RichText::new(&v.id).color(Color32::BLACK))
                            .fill(fill)
                            .selected(on);
                        if ui.add(btn).clicked() {
                            pick = Some(v.id.clone());
                        }
                        ui.label(&v.category);
                        let obj = if v.objects.is_empty() {
                            "-"
                        } else {
                            v.objects.as_str()
                        };
                        let on_obj = selected.as_deref() == Some(obj)
                            || (!v.objects.is_empty()
                                && selected.as_deref().is_some_and(|s| v.objects.contains(s)));
                        if ui.selectable_label(on || on_obj, obj).clicked() {
                            if obj == "-" {
                                pick = Some(v.id.clone());
                            } else {
                                pick_obj = Some(v.id.clone());
                            }
                        }
                        ui.label(&v.message);
                        ui.end_row();
                    }
                });
        });
    });
    if let Some(id) = pick_obj {
        let _ = model.select_methodology(&id);
        let _ = model.select_methodology_object(&id);
    } else if let Some(id) = pick {
        let _ = model.select_methodology(&id);
    }
}

fn qor_severity_color(theme: &theme::Theme, sev: QorSeverity) -> Color32 {
    match sev {
        QorSeverity::Critical => theme.danger,
        QorSeverity::Warning => theme.warn,
        QorSeverity::Info => theme.info,
    }
}

/// QoR Suggestions panel: the rule-registry report as an actionable table.
/// Selecting a row shows the full explanation + fix; Locate cross-probes the
/// cited objects via the master selection API.
fn paint_qor(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("QoR Suggestions");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Re-run analysis").clicked() {
            let msg = model.refresh_qor();
            model.log.push(msg);
        }
        ui.label(
            RichText::new("rule-based, measured - no generic advice")
                .size(11.0)
                .color(theme.dim),
        );
    });
    ui.add_space(6.0);
    if model.tree.top.is_none() {
        if paint_remaining_cta(ui, "No design yet.", "Run Synthesis", theme) {
            queue_flow(FlowStep::Synthesis);
        }
        return;
    }
    let report = model.qor_report();
    // Labeled summary chips, not a key=value dump.
    ui.horizontal(|ui| {
        ui.label(RichText::new("Suggestions").color(theme.faint).small());
        ui.label(
            RichText::new(report.suggestions.len().to_string()).strong(),
        );
        ui.label(RichText::new("Critical").color(theme.faint).small());
        ui.label(
            RichText::new(report.critical_count().to_string())
                .color(theme.danger)
                .strong(),
        );
        ui.label(RichText::new("Warning").color(theme.faint).small());
        ui.label(
            RichText::new(report.warning_count().to_string())
                .color(theme.warn)
                .strong(),
        );
        ui.label(RichText::new("Info").color(theme.faint).small());
        ui.label(
            RichText::new(report.info_count().to_string())
                .color(theme.info)
                .strong(),
        );
    });
    ui.add_space(4.0);
    if report.suggestions.is_empty() {
        ui.label(
            RichText::new("No suggestions - the design meets every QoR check.")
                .color(theme.ok),
        );
        return;
    }
    let selected_qor = model.selected_qor.clone();
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    let mut locate: Option<String> = None;
    let n = report.suggestions.len().max(1);
    let remain = ui.available_size();
    let bbox = chrome::nv_table_bbox(n, remain.x.max(80.0), remain.y.max(120.0));
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(bbox.drawn_w.max(remain.x), bbox.drawn_h.max(remain.y)),
        Sense::hover(),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(8.0)), |ui| {
        let n_f = n as f32;
        let row_gap = ((ui.available_height() - 28.0) / n_f - 18.0).clamp(4.0, 28.0);
        let col_w = chrome::stretched_col_w_gap(6, ui.available_width(), 8.0);
        egui::ScrollArea::both().show(ui, |ui| {
            egui::Grid::new("qor_table")
                .spacing([8.0, row_gap])
                .min_col_width(col_w)
                .show(ui, |ui| {
                    ui.label(RichText::new("ID").strong());
                    ui.label(RichText::new("Severity").strong());
                    ui.label(RichText::new("Category").strong());
                    ui.label(RichText::new("Suggestion").strong());
                    ui.label(RichText::new("Est. gain").strong());
                    ui.label(RichText::new("Objects").strong());
                    ui.end_row();
                    for s in &report.suggestions {
                        let on = selected_qor.as_deref() == Some(s.id);
                        let fill = qor_severity_color(theme, s.severity);
                        let btn = egui::Button::new(RichText::new(s.id).color(Color32::BLACK))
                            .fill(fill)
                            .selected(on);
                        if ui.add(btn).clicked() {
                            pick = Some(s.id.to_string());
                        }
                        ui.label(s.severity.as_str());
                        ui.label(s.category);
                        ui.label(&s.title);
                        ui.label(
                            s.gain_ps
                                .map(|g| format!("+{g} ps"))
                                .unwrap_or_else(|| "-".into()),
                        );
                        let objs = if s.objects.is_empty() {
                            "-".to_string()
                        } else {
                            s.objects.join(", ")
                        };
                        ui.horizontal(|ui| {
                            let on_obj = selected.as_deref().is_some_and(|sel| {
                                !s.objects.is_empty() && s.objects.iter().any(|o| o == sel)
                            });
                            if ui.selectable_label(on || on_obj, &objs).clicked() {
                                pick = Some(s.id.to_string());
                            }
                            if !s.objects.is_empty() && ui.button("Locate").clicked() {
                                locate = Some(s.objects[0].clone());
                            }
                        });
                        ui.end_row();
                    }
                });
        });
    });
    // Detail of the selected suggestion: full explanation + fix hint.
    if let Some(id) = selected_qor.as_deref().and_then(|id| report.suggestion(id)) {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        ui.label(RichText::new(format!("{} - {}", id.id, id.title)).strong());
        ui.add_space(2.0);
        ui.label(RichText::new(&id.explanation).color(theme.text));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Fix:").strong());
            ui.label(RichText::new(&id.fix_hint).color(theme.dim));
        });
        if id.action == Some(QorAction::OpenRuns) {
            ui.add_space(4.0);
            if ui.button("Open Design Runs").clicked() {
                model.workspace = WorkspaceTab::Runs;
            }
        }
    }
    if let Some(obj) = locate {
        let _ = model.select_qor_object(&obj);
    } else if let Some(id) = pick {
        let _ = model.select_qor(&id);
    }
}

fn drc_severity_color(theme: &theme::Theme, sev: DrcSeverity) -> Color32 {
    match sev {
        DrcSeverity::Error => theme.danger,
        DrcSeverity::Warning => theme.warn,
        DrcSeverity::Advisory => theme.violet,
    }
}

#[allow(dead_code)] // intentional: helper for WIP bitstream panel
fn bitstream_block_color(theme: &theme::Theme, block: &str) -> Color32 {
    match block {
        "CLB_IO_CLK" => theme.ok,
        "DSP" => theme.violet,
        "BRAM" => theme.warn,
        "IOB" => theme.info,
        _ => theme.dim,
    }
}

#[allow(dead_code)] // intentional: WIP panel kept for upcoming canvas wiring
fn paint_bitstream(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("Bitstream");
    ui.add_space(6.0);
    let report = model.bitstream_report();
    if report.frames == 0 && report.bytes == 0 {
        if paint_remaining_cta(ui, "No bitstream yet.", "Generate Bitstream", theme) {
            let _ = model.exec("write_bitstream");
        }
        return;
    }
    ui.horizontal(|ui| {
        if ui.button("Generate Bitstream").clicked() {
            let _ = model.exec("write_bitstream");
        }
        if ui.button("Report bitstream").clicked() {
            let _ = model.exec("report_bitstream");
        }
    });
    ui.add_space(6.0);
    ui.label(format!(
        "{} · {} frames · {} B · id {:#010x}",
        if report.configured != 0 {
            "Configured"
        } else {
            "Not configured"
        },
        report.frames,
        report.bytes,
        report.idcode
    ));
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    egui::ScrollArea::both().id_salt("bitstream_scroll").max_height(280.0).show(ui, |ui| {
        egui::Grid::new("bitstream_far_table")
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("FAR").strong());
                ui.label(RichText::new("Block").strong());
                ui.label(RichText::new("Die").strong());
                ui.label(RichText::new("Major").strong());
                ui.label(RichText::new("Minor").strong());
                ui.label(RichText::new("Ones").strong());
                ui.label(RichText::new("Word").strong());
                ui.end_row();
                for row in &report.rows {
                    let far = row.far_hex();
                    let on = selected.as_deref() == Some(far.as_str());
                    let fill = bitstream_block_color(theme, row.block_name());
                    let btn = egui::Button::new(RichText::new(&far).color(Color32::BLACK))
                        .fill(fill)
                        .selected(on);
                    if ui.add(btn).clicked() {
                        pick = Some(far);
                    }
                    ui.label(row.block_name());
                    ui.label(row.die.to_string());
                    ui.label(row.major.to_string());
                    ui.label(row.minor.to_string());
                    ui.label(row.ones().to_string());
                    ui.label(row.word_hex());
                    ui.end_row();
                }
            });
    });
    if let Some(far) = pick {
        let _ = model.select_bitstream_frame(&far);
    }
}

fn paint_drc(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    let mut do_report = false;
    section_bar(ui, theme, "DRC", |ui| {
        if ui.button("Report DRC").clicked() {
            do_report = true;
        }
    });
    if do_report {
        let _ = model.exec("report_drc");
    }
    if model.utilization.is_none() && model.drc.is_none() {
        // Designed empty state (§3.8): centered dim text + action.
        ui.add_space(28.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("No DRC results yet.")
                    .size(13.0)
                    .color(theme.dim),
            );
            ui.add_space(6.0);
            if primary_button(ui, "Report DRC").clicked() {
                let _ = model.exec("report_drc");
            }
        });
        return;
    }
    let report = model.drc.clone().unwrap_or_else(|| model.drc_report());
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(format!(
            "{} violations · {} errors",
            report.violations.len(),
            report.error_count()
        ))
        .size(12.0)
        .color(theme.dim),
    );
    ui.add_space(4.0);

    if report.ok() {
        ui.add_space(16.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("No violations.")
                    .size(13.0)
                    .color(theme.dim),
            );
        });
        return;
    }

    // ID | Severity | Objects | Message. The message column wraps inside its
    // own column width - it never clips at the right edge. Header and rows
    // live in the same scroll area so their widths always agree.
    let selected_drc = model.selected_drc.clone();
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    let mut pick_obj: Option<String> = None;
    egui::ScrollArea::vertical().id_salt("drc_scroll").show(ui, |ui| {
        let w = ui.available_width();
        // ID (severity-colored badge) | Objects | Message. The old Severity
        // text column duplicated the badge's encoding, so it is gone; its
        // width goes to the message column.
        let obj_w = ((w - 80.0) * 0.35).max(90.0);
        let msg_w = (w - 80.0 - obj_w).max(90.0);
        let widths = [80.0f32, obj_w, msg_w];

        let (hrect, _) =
            ui.allocate_exact_size(egui::vec2(w, 28.0), egui::Sense::hover());
        spec_paint_cells(
            ui,
            hrect,
            &widths,
            &[
                SpecCell::text("ID", theme.dim),
                SpecCell::text("Objects", theme.dim),
                SpecCell::text("Message", theme.dim),
            ],
            true,
        );
        spec_divider(ui, theme, hrect);

        for (i, v) in report.items.iter().enumerate() {
            let on = selected_drc.as_deref() == Some(v.id.as_str());
            let on_obj = selected.as_deref().is_some_and(|s| {
                !v.objects.is_empty() && v.objects.contains(s)
            });
            // Wrap the message inside its column; the row grows to fit it.
            let msg = ui.painter().layout(
                v.message.clone(),
                theme::font_ui(13.0),
                theme.text,
                msg_w - 16.0,
            );
            let rh = (msg.size().y + 12.0).max(27.0);
            let (rrect, _) =
                ui.allocate_exact_size(egui::vec2(w, rh), egui::Sense::hover());
            if on {
                ui.painter().rect_filled(rrect, 0.0, theme.sel);
            } else if ui.rect_contains_pointer(rrect) {
                ui.painter().rect_filled(rrect, 0.0, theme.surface_hover);
            }
            let resp = ui.interact(
                rrect,
                egui::Id::new(("drcrow", i)),
                egui::Sense::click(),
            );
            if resp.clicked() {
                pick = Some(v.id.clone());
                // Clicking the Objects cell also cross-probes the object.
                if let Some(pos) = resp.interact_pointer_pos() {
                    let ox0 = rrect.left() + widths[0];
                    if pos.x >= ox0 && pos.x < ox0 + widths[1] {
                        pick_obj = Some(v.id.clone());
                    }
                }
            }
            let p = ui.painter();
            let cy = rrect.center().y;
            // ID badge in the severity color.
            let badge = egui::Rect::from_min_size(
                egui::pos2(rrect.left() + 8.0, cy - 9.0),
                egui::vec2(widths[0] - 16.0, 18.0),
            );
            p.rect_filled(badge, 4.0, drc_severity_color(theme, v.severity));
            p.text(
                badge.center(),
                egui::Align2::CENTER_CENTER,
                &v.id,
                theme::font_ui_semibold(11.0),
                Color32::BLACK,
            );
            // Objects cell (the severity text column is gone; the ID badge
            // carries the severity color).
            let sx = rrect.left() + widths[0];
            let obj = if v.objects.is_empty() {
                "-"
            } else {
                v.objects.as_str()
            };
            p.text(
                egui::pos2(sx + 8.0, cy),
                egui::Align2::LEFT_CENTER,
                obj,
                theme::font_ui(13.0),
                if on_obj { theme.text } else { theme.dim },
            );
            // Wrapped message, top-aligned within the row.
            let mx = rrect.left() + widths[0] + widths[1];
            p.galley(egui::pos2(mx + 8.0, rrect.top() + 6.0), msg, theme.text);
            spec_divider(ui, theme, rrect);
        }
    });
    if let Some(id) = pick_obj {
        let _ = model.select_drc(&id);
        let _ = model.select_drc_object(&id);
    } else if let Some(id) = pick {
        let _ = model.select_drc(&id);
    }
}
fn paint_utilization(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("Utilization");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Report utilization").clicked() {
            let _ = model.exec("report_utilization");
        }
        if ui.button("Export").clicked() {
            let path = std::env::var("HELION_REPORT_PATH")
                .unwrap_or_else(|_| "/tmp/helion-reports/utilization.rpt".into());
            let _ = model.exec(&format!("write_report utilization {path}"));
        }
    });
    if let Some(exp) = model.last_report_export.clone() {
        ui.label(RichText::new(exp).monospace().size(12.0).color(theme.dim));
    }
    ui.add_space(6.0);
    let report = model.utilization_report();
    if report.part.is_empty() {
        if paint_remaining_cta(ui, "No placed design yet.", "Place", theme) {
            queue_flow(FlowStep::Place);
        }
        return;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Part").color(theme.faint).small());
        ui.label(RichText::new(&report.part).strong());
    });
    let selected_util = model.selected_utilization.clone();
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    let mut pick_hier: Option<String> = None;
    let n_hier = if report.hierarchy.is_empty() {
        0
    } else {
        report.hierarchy.len() + 1
    };
    let n_rows = report.occupancy.len() + 1 + n_hier;
    let remain = ui.available_size();
    let bbox = chrome::occupancy_table_bbox(n_rows.max(1), remain.x.max(80.0), remain.y.max(120.0));
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(bbox.drawn_w.max(remain.x), bbox.drawn_h.max(remain.y)),
        Sense::hover(),
    );
    ui.painter().rect_filled(rect, 0.0, theme.die_bg);
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(8.0)), |ui| {
        let remain_h = (ui.available_height() - 40.0).max(chrome::OCCUPANCY_BAR_H);
        let n_bars = report.occupancy.len().max(1);
        let bar_h = chrome::occupancy_bars_h_after_footer(n_bars, n_hier, remain_h);
        let row_gap = 4.0;
        let bar_span = chrome::occupancy_bar_w(
            (ui.available_width() - chrome::occupancy_label_reserve(4)).max(80.0),
        );
        egui::ScrollArea::both()
            .id_salt("ug893_utilization")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("utilization_occupancy")
                    .spacing([8.0, row_gap])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Resource").strong());
                        ui.label(RichText::new("Used").strong());
                        ui.label(RichText::new("Available").strong());
                        ui.label(RichText::new("Pct").strong());
                        ui.label(RichText::new("Occupancy").strong());
                        ui.end_row();
                        for row in &report.occupancy {
                            let on = selected_util.as_deref() == Some(row.resource);
                            if ui.selectable_label(on, row.resource).clicked() {
                                pick = Some(row.resource.into());
                            }
                            num_cell(ui, &row.used.to_string());
                            num_cell(ui, &row.available.to_string());
                            num_cell(ui, &format!("{}%", row.pct()));
                            let frac = if row.available == 0 {
                                0.0
                            } else {
                                row.used as f32 / row.available as f32
                            };
                            let (bar, _) =
                                ui.allocate_exact_size(egui::vec2(bar_span, bar_h), Sense::hover());
                            ui.painter().rect_filled(bar, 2.0, theme.surface);
                            let fill =
                                bar.with_max_x(bar.left() + bar.width() * frac.clamp(0.0, 1.0));
                            ui.painter().rect_filled(fill, 2.0, theme.info);
                            ui.end_row();
                        }
                    });
                if !report.hierarchy.is_empty() {
                    ui.add_space(8.0);
                    ui.label(RichText::new("Hierarchical").strong());
                    let hier_col = chrome::stretched_col_w_gap(6, ui.available_width(), 8.0);
                    egui::Grid::new("utilization_hierarchy")
                        .spacing([8.0, row_gap])
                        .min_col_width(hier_col)
                        .show(ui, |ui| {
                            ui.label(RichText::new("Instance").strong());
                            ui.label(RichText::new("LUT").strong());
                            ui.label(RichText::new("FF").strong());
                            ui.label(RichText::new("IOB").strong());
                            ui.label(RichText::new("BRAM").strong());
                            ui.label(RichText::new("DSP").strong());
                            ui.end_row();
                            for h in &report.hierarchy {
                                let key = format!("hier:{}", h.name);
                                let on = selected_util.as_deref() == Some(key.as_str())
                                    || selected.as_deref() == Some(h.name.as_str());
                                if ui.selectable_label(on, &h.name).clicked() {
                                    pick_hier = Some(h.name.clone());
                                }
                                num_cell(ui, &h.lut.to_string());
                                num_cell(ui, &h.ff.to_string());
                                num_cell(ui, &h.iob.to_string());
                                num_cell(ui, &h.bram.to_string());
                                num_cell(ui, &h.dsp.to_string());
                                ui.end_row();
                            }
                        });
                }
            });
    });
    if let Some(res) = pick {
        let _ = model.select_utilization(&res);
    }
    if let Some(name) = pick_hier {
        let _ = model.select_utilization_hier(&name);
    }
}

fn paint_dotted(p: &egui::Painter, a: egui::Pos2, b: egui::Pos2, stroke: Stroke) {
    let d = b - a;
    let len = d.length();
    if len < 0.5 {
        return;
    }
    let dir = d / len;
    let mut t = 0.0;
    while t < len {
        let t1 = (t + 4.0).min(len);
        p.line_segment([a + dir * t, a + dir * t1], stroke);
        t += 8.0;
    }
}

fn paint_schematic(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("Schematic");
    // Stay on the schematic pane - do not fall back to Reports after More / Zoom.
    model.workspace = WorkspaceTab::Schematic;
    let mut zoom_fit = false;
    let mut zoom_in = false;
    let mut zoom_out = false;
    let drawing_for_counts = model.schematic.drawing_arc();
    let n_cells = drawing_for_counts
        .symbols
        .iter()
        .filter(|s| !s.kind.starts_with("PORT"))
        .count();
    let n_ports = drawing_for_counts
        .symbols
        .iter()
        .filter(|s| s.kind.starts_with("PORT"))
        .count();
    let n_nets = {
        let mut s = std::collections::HashSet::new();
        for w in &drawing_for_counts.wires {
            s.insert(w.net.as_str());
        }
        s.len()
    };
    // Overflow toolbar: actions that do not fit move into a ">>" menu
    // (VS Code pattern) instead of truncating or needing a scroll strip.
    #[derive(Clone, Copy, PartialEq)]
    enum SchemTool {
        Previous,
        Next,
        ZoomIn,
        ZoomOut,
        ZoomFit,
        ExpandCone,
        CollapseCone,
        ExpandInside,
        CollapseInside,
        FindCells,
        FindPorts,
        FindNets,
        MarkDebug,
        AddProbe,
    }
    {
        let dbg_net = model.selected_debug_net();
        let md_label = match &dbg_net {
            Some(n) => format!("Mark Debug {n}"),
            None => "Mark Debug".into(),
        };
        let items = [
            SchemTool::Previous,
            SchemTool::Next,
            SchemTool::ZoomIn,
            SchemTool::ZoomOut,
            SchemTool::ZoomFit,
            SchemTool::ExpandCone,
            SchemTool::CollapseCone,
            SchemTool::ExpandInside,
            SchemTool::CollapseInside,
            SchemTool::FindCells,
            SchemTool::FindPorts,
            SchemTool::FindNets,
            SchemTool::MarkDebug,
            SchemTool::AddProbe,
        ];
        let label_of = |t: SchemTool| -> String {
            match t {
                SchemTool::Previous => "Previous".into(),
                SchemTool::Next => "Next".into(),
                SchemTool::ZoomIn => "Zoom In".into(),
                SchemTool::ZoomOut => "Zoom Out".into(),
                SchemTool::ZoomFit => "Zoom Fit".into(),
                SchemTool::ExpandCone => "Expand Cone".into(),
                SchemTool::CollapseCone => "Collapse Cone".into(),
                SchemTool::ExpandInside => "Expand Inside".into(),
                SchemTool::CollapseInside => "Collapse Inside".into(),
                SchemTool::FindCells => format!("{n_cells} Cells"),
                SchemTool::FindPorts => format!("{n_ports} I/O Ports"),
                SchemTool::FindNets => format!("{n_nets} Nets"),
                SchemTool::MarkDebug => md_label.clone(),
                SchemTool::AddProbe => "Add Probe".into(),
            }
        };
        let widths: Vec<chrome::OverflowItem> = items
            .iter()
            .map(|t| chrome::OverflowItem {
                width: chrome::text_button_w(&label_of(*t)),
            })
            .collect();
        let m = &mut *model;
        chrome::overflow_toolbar(ui, &widths, |ui, idx, in_menu| {
            let t = items[idx];
            // In the overflow menu everything is a plain button row.
            let clicked = match t {
                SchemTool::Previous => ui.button("Previous").clicked(),
                SchemTool::Next => ui.button("Next").clicked(),
                SchemTool::ZoomIn => ui.button("Zoom In").clicked(),
                SchemTool::ZoomOut => ui.button("Zoom Out").clicked(),
                SchemTool::ZoomFit => ui.button("Zoom Fit").clicked(),
                SchemTool::ExpandCone => ui.button("Expand Cone").clicked(),
                SchemTool::CollapseCone => ui.button("Collapse Cone").clicked(),
                SchemTool::ExpandInside => ui.button("Expand Inside").clicked(),
                SchemTool::CollapseInside => ui.button("Collapse Inside").clicked(),
                SchemTool::FindCells => {
                    if in_menu {
                        ui.button(format!("{n_cells} Cells")).clicked()
                    } else {
                        ui.link(format!("{n_cells} Cells")).clicked()
                    }
                }
                SchemTool::FindPorts => {
                    if in_menu {
                        ui.button(format!("{n_ports} I/O Ports")).clicked()
                    } else {
                        ui.link(format!("{n_ports} I/O Ports")).clicked()
                    }
                }
                SchemTool::FindNets => {
                    if in_menu {
                        ui.button(format!("{n_nets} Nets")).clicked()
                    } else {
                        ui.link(format!("{n_nets} Nets")).clicked()
                    }
                }
                SchemTool::MarkDebug => {
                    ui.add_enabled(
                        dbg_net.is_some(),
                        egui::Button::new(md_label.clone()),
                    )
                    .on_hover_text("UG908 Mark Debug on the selected Schematic/Device net")
                    .clicked()
                }
                SchemTool::AddProbe => {
                    ui.add_enabled(dbg_net.is_some(), egui::Button::new("Add Probe"))
                        .on_hover_text("UG908 Add Probe → ILA dashboard")
                        .clicked()
                }
            };
            if !clicked {
                return;
            }
            match t {
                SchemTool::Previous => {
                    let _ = m.schematic_previous_view();
                }
                SchemTool::Next => {
                    let _ = m.schematic_next_view();
                }
                SchemTool::ZoomIn => zoom_in = true,
                SchemTool::ZoomOut => zoom_out = true,
                SchemTool::ZoomFit => zoom_fit = true,
                SchemTool::ExpandCone => {
                    let _ = m.exec("expand_cone");
                }
                SchemTool::CollapseCone => {
                    let _ = m.exec("collapse_cone");
                }
                SchemTool::ExpandInside => {
                    let _ = m.exec("expand_inside");
                }
                SchemTool::CollapseInside => {
                    let _ = m.exec("collapse_inside");
                }
                SchemTool::FindCells => {
                    let _ = m.exec("sheet_find cells");
                }
                SchemTool::FindPorts => {
                    let _ = m.exec("sheet_find ports");
                }
                SchemTool::FindNets => {
                    let _ = m.exec("sheet_find nets");
                }
                SchemTool::MarkDebug => {
                    let _ = m.exec("mark_debug");
                }
                SchemTool::AddProbe => {
                    let _ = m.exec("add_probe");
                }
            }
        });
        // Auto-fit checkbox + status labels stay inline after the row.
        ui.horizontal(|ui| {
            ui.checkbox(&mut model.schematic.auto_fit_on_probe, "Auto-fit path")
                .on_hover_text("Zoom to fit the probed timing path on selection");
            if let Some(root) = &model.schematic.cone_root {
                ui.label(RichText::new(format!("Cone {root}")).small().weak());
            }
            if let Some(inst) = &model.schematic.expand_inside {
                ui.label(RichText::new(format!("Inside {inst}")).small().weak());
            }
            ui.label(
                RichText::new(format!("Zoom {:.0}%", model.schematic.camera.zoom * 100.0))
                    .small()
                    .weak(),
            );
        });
    }
    // Full hint on hover. Visible words are complete - never "pinch or …".
    let pinch_hint = "pinch or ⌘-scroll to zoom · drag to pan";
    ui.add(egui::Label::new(RichText::new("pinch to zoom · drag to pan").small().weak()).wrap())
        .on_hover_text(pinch_hint);
    if !model.timing_paths.is_empty() {
        // Results strip belongs under the drawing (UG893). Keep it collapsed so Zoom Fit owns the pane.
        egui::CollapsingHeader::new("Timing paths")
            .default_open(false)
            .show(ui, |ui| {
                let mut pick_path = None;
                let selected_path = model.selected_timing_path;
                egui::ScrollArea::vertical()
                    .id_salt("schematic_timing_paths_scroll")
                    .max_height(chrome::DEVICE_TABLES_MAX_HEIGHT)
                    .show(ui, |ui| {
                        egui::Grid::new("schematic_timing_paths")
                            .spacing([8.0, 4.0])
                            .show(ui, |ui| {
                                ui.label(RichText::new("Name").strong());
                                ui.label(RichText::new("From").strong());
                                ui.label(RichText::new("To").strong());
                                ui.label(RichText::new("Slack_ps").strong());
                                ui.end_row();
                                for (i, p) in model.timing_paths.iter().enumerate() {
                                    let on = selected_path == Some(i);
                                    let resp = ui.selectable_label(on, &p.name);
                                    if resp.clicked() {
                                        pick_path = Some(i);
                                    }
                                    // Right-click: Copy row / cross-probe (Vivado pattern).
                                    resp.context_menu(|ui| {
                                        if ui.button("Copy row").clicked() {
                                            ui.ctx().copy_text(format!(
                                                "{}\t{}\t{}\t{}",
                                                p.name, p.startpoint, p.endpoint, p.slack_ps
                                            ));
                                            ui.close();
                                        }
                                        if ui.button("Cross-probe in Device").clicked() {
                                            pick_path = Some(i);
                                            ui.close();
                                        }
                                    });
                                    ui.label(&p.startpoint);
                                    ui.label(&p.endpoint);
                                    ui.label(p.slack_ps.to_string());
                                    ui.end_row();
                                }
                            });
                    });
                if let Some(i) = pick_path {
                    let _ = model.select_timing_path(&i.to_string());
                }
            });
    }
    let mut pick = None;
    let mut expand = None;
    let selected = model.selected.clone();
    let avail = ui.available_size();
    // Canvas is the leftover pane - Zoom Fit fills this, not the heading/toolbar above.
    let canvas = egui::vec2(avail.x.max(1.0), avail.y.max(1.0));
    model.schematic.set_viewport(canvas.x, canvas.y);
    let sheet = model.schematic.drawing_arc();
    let cam0 = model.schematic.camera;
    if zoom_fit
        || chrome::schematic_should_auto_fit(
            cam0.zoom,
            cam0.pan_x,
            cam0.pan_y,
            sheet.width,
            sheet.height,
            canvas.x,
            canvas.y,
        )
        || chrome::schematic_identity_clips_bottom(
            cam0.zoom,
            cam0.pan_x,
            cam0.pan_y,
            sheet.height,
            canvas.y,
        )
    {
        model.workspace = WorkspaceTab::Schematic;
        model.schematic.apply_zoom_fit(sheet.width, sheet.height);
    } else if zoom_in {
        model.workspace = WorkspaceTab::Schematic;
        if cam0.zoom < schem_lod::BLOCK_ZOOM {
            // In the block treemap the camera is at fit zoom; a relative
            // zoom would never cross the LOD threshold, so jump straight
            // to the gate sheet at a readable zoom, centered.
            let z1 = 0.75_f32;
            model.schematic.camera.zoom = z1;
            model.schematic.camera.pan_x = canvas.x * 0.5 - (sheet.width * 0.5) * z1;
            model.schematic.camera.pan_y = canvas.y * 0.5 - (sheet.height * 0.5) * z1;
        } else {
            model
                .schematic
                .zoom_at(1.35, canvas.x * 0.5, canvas.y * 0.5);
        }
    } else if zoom_out {
        model.workspace = WorkspaceTab::Schematic;
        // Reset pan. zoom_at around the pane center clipped PORT_OUT on the right.
        model.schematic.zoom_out_framed(sheet.width, sheet.height);
    }
    let (rect, resp) = ui.allocate_exact_size(canvas, Sense::click_and_drag());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        let (zoom_delta, scroll, mods, pointer) = ui.input(|i| {
            (
                i.zoom_delta(),
                i.smooth_scroll_delta,
                i.modifiers,
                i.pointer.hover_pos(),
            )
        });
        if let Some(pos) = pointer {
            let vx = pos.x - rect.left();
            let vy = pos.y - rect.top();
            if (zoom_delta - 1.0).abs() > 0.001 {
                if model.schematic.camera.zoom < schem_lod::BLOCK_ZOOM && zoom_delta > 1.0 {
                    // Pinching out of the block treemap: jump to gates.
                    let z1 = 0.75_f32;
                    let sheet = model.schematic.drawing_arc();
                    model.schematic.camera.zoom = z1;
                    model.schematic.camera.pan_x = canvas.x * 0.5 - (sheet.width * 0.5) * z1;
                    model.schematic.camera.pan_y = canvas.y * 0.5 - (sheet.height * 0.5) * z1;
                } else {
                    model.schematic.zoom_at(zoom_delta, vx, vy);
                }
            } else if (mods.command || mods.ctrl) && scroll.y.abs() > 0.1 {
                let f = (1.0 + scroll.y * 0.004).clamp(0.5, 1.8);
                model.schematic.zoom_at(f, vx, vy);
            } else if scroll.length_sq() > 0.1 {
                model.schematic.pan_by(scroll.x, scroll.y);
            }
        }
    }
    if resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let d = resp.drag_delta();
        model.schematic.pan_by(d.x, d.y);
    }
    let drawing = model.schematic.drawing_arc();
    // Cross-probe: a freshly probed timing path auto-frames in the viewport
    // (toggle: "Auto-fit path" in the toolbar).
    if model.schematic.zoom_to_path_pending {
        model.schematic.zoom_to_path_pending = false;
        if model.schematic.auto_fit_on_probe {
            let mut bb: Option<(f32, f32, f32, f32)> = None;
            for sy in drawing.symbols.iter() {
                if model.schematic.highlight_cells.contains(&sy.name) {
                    let (x0, y0, x1, y1) = (sy.x, sy.y, sy.x + sy.w, sy.y + sy.h);
                    bb = Some(match bb {
                        Some((ax0, ay0, ax1, ay1)) => {
                            (ax0.min(x0), ay0.min(y0), ax1.max(x1), ay1.max(y1))
                        }
                        None => (x0, y0, x1, y1),
                    });
                }
            }
            if let Some((x0, y0, x1, y1)) = bb {
                // Pad so the path doesn't touch the pane edges.
                let px = (x1 - x0) * 0.15 + 40.0;
                let py = (y1 - y0) * 0.15 + 40.0;
                model
                    .schematic
                    .zoom_to_region(x0 - px, y0 - py, x1 + px, y1 + py);
            }
        }
    }
    let cam = model.schematic.camera;
    let z = cam.zoom.max(0.05);
    // Level-of-detail: below BLOCK_ZOOM the sheet renders a treemap of
    // hierarchical blocks instead of 15k individual gates (wire grid).
    // Block mode needs at least 2 blocks and a large-enough design to be
    // meaningful; small designs stay on the readable gate sheet at fit zoom.
    let mut block_layer: Option<schem_lod::BlockLayer> = None;
    if z < schem_lod::BLOCK_ZOOM && n_cells >= schem_lod::LOD_MIN_CELLS {
        let layer_id = egui::Id::new("schem_block_layer");
        let key = (
            std::sync::Arc::as_ptr(&drawing) as usize,
            model.schematic.instances.len(),
            drawing.symbols.len(),
        );
        let cached: Option<((usize, usize, usize), schem_lod::BlockLayer)> =
            ui.ctx().data_mut(|d| d.get_temp(layer_id));
        let layer = match cached {
            Some((k, l)) if k == key => l,
            _ => {
                let top = model.hierarchy.top.as_deref().unwrap_or("(top)");
                let l = schem_lod::build_block_layer(&drawing, &model.schematic.instances, top);
                ui.ctx().data_mut(|d| d.insert_temp(layer_id, (key, l.clone())));
                l
            }
        };
        if layer.blocks.len() >= 2 {
            block_layer = Some(layer);
        }
    }
    if ui.is_rect_visible(rect) {
        let p = ui.painter().with_clip_rect(rect);
        p.rect_filled(rect, 0.0, theme.die_bg);
        let o = egui::pos2(rect.min.x + cam.pan_x, rect.min.y + cam.pan_y);
        let net_col = theme.ok;
        let gold = theme.gold;
        let path_col = theme.danger;
        let name_fs = (10.0 * z).clamp(8.0, 13.0);
        let pin_fs = 9.0 * z;
        let show_names = true;
        let show_pins = pin_fs >= 6.5;
        if let Some(layer) = &block_layer {
            schem_lod::paint_block_layer(&p, rect, layer, theme, selected.as_deref());
        } else {
        for w in &drawing.wires {
            if w.points.len() < 2 {
                continue;
            }
            let mut minx = f32::MAX;
            let mut miny = f32::MAX;
            let mut maxx = f32::MIN;
            let mut maxy = f32::MIN;
            for &(x, y) in &w.points {
                let px = o.x + x * z;
                let py = o.y + y * z;
                minx = minx.min(px);
                miny = miny.min(py);
                maxx = maxx.max(px);
                maxy = maxy.max(py);
            }
            let bb = egui::Rect::from_min_max(egui::pos2(minx, miny), egui::pos2(maxx, maxy));
            if !bb.expand(8.0).intersects(rect) {
                continue;
            }
            let thick = if w.highlighted {
                4.2_f32
            } else if w.width > 1 {
                3.6_f32
            } else {
                1.4_f32
            };
            let col = if w.highlighted { path_col } else { net_col };
            for pair in w.points.windows(2) {
                let a = egui::pos2(o.x + pair[0].0 * z, o.y + pair[0].1 * z);
                let b = egui::pos2(o.x + pair[1].0 * z, o.y + pair[1].1 * z);
                if w.off_sheet {
                    paint_dotted(&p, a, b, Stroke::new(thick, col));
                } else {
                    p.line_segment([a, b], Stroke::new(thick, col));
                }
            }
            if show_names {
                let a = w.points[0];
                let b = w.points.get(1).copied().unwrap_or(a);
                let pa = egui::pos2(o.x + a.0 * z, o.y + a.1 * z);
                let pb = egui::pos2(o.x + b.0 * z, o.y + b.1 * z);
                let span = (pa.x - pb.x).abs().max((pa.y - pb.y).abs());
                if span > 48.0 && z >= 0.55 {
                    let mid = egui::pos2((pa.x + pb.x) * 0.5, (pa.y + pb.y) * 0.5 - 8.0 * z);
                    p.text(
                        mid,
                        egui::Align2::CENTER_BOTTOM,
                        &w.net,
                        egui::FontId::monospace(pin_fs.clamp(6.5, 11.0)),
                        theme.info,
                    );
                }
            }
        }
        for sy in &drawing.symbols {
            let r = egui::Rect::from_min_size(
                egui::pos2(o.x + sy.x * z, o.y + sy.y * z),
                egui::vec2(sy.w * z, sy.h * z),
            );
            if !r.expand(24.0).intersects(rect) {
                continue;
            }
            let on = selected.as_deref() == Some(sy.name.as_str());
            let port = sy.kind.starts_with("PORT");
            let fill = if sy.highlighted {
                theme.danger.gamma_multiply(0.4)
            } else if on {
                theme.ok.gamma_multiply(0.35)
            } else if port || sy.kind == "IOB_OUT" {
                theme.cat_blue.gamma_multiply(0.4)
            } else {
                theme.surface
            };
            let stroke = Stroke::new(
                if on || sy.highlighted {
                    2.0_f32
                } else {
                    1.0_f32
                },
                if sy.highlighted {
                    path_col
                } else if on {
                    gold
                } else {
                    theme.faint
                },
            );
            if port || sy.kind == "IOB_OUT" {
                let pts = vec![r.left_top(), r.left_bottom(), r.right_center()];
                p.add(egui::Shape::convex_polygon(pts, fill, stroke));
            } else {
                p.rect_filled(r, 3.0, fill);
                p.rect_stroke(r, 3.0, stroke, egui::StrokeKind::Inside);
            }
            if port || sy.kind == "IOB_OUT" {
                // Keep PORT_OUT / led readable even when the triangle is small.
                // Text stays inside the canvas, not clipped to the symbol box.
                let fs = name_fs.clamp(8.0, 12.0);
                let kind = helion_gui::schematic_kind_label(&sy.kind);
                let inward = sy.kind == "PORT_IN";
                let anchor = if inward {
                    r.left() + 6.0
                } else {
                    r.right() - 4.0
                };
                let align = if inward {
                    egui::Align2::LEFT_CENTER
                } else {
                    egui::Align2::RIGHT_CENTER
                };
                p.text(
                    egui::pos2(anchor, r.center().y - 7.0),
                    align,
                    kind,
                    egui::FontId::monospace(fs),
                    theme.info,
                );
                p.text(
                    egui::pos2(anchor, r.center().y + 8.0),
                    align,
                    &sy.name,
                    egui::FontId::monospace(fs),
                    theme.text,
                );
            } else if show_names && r.width() >= 28.0 && r.height() >= 16.0 {
                // Do not shrink the bottom: the name band sits under the box and
                // a tightened clip cropped u_lut1.
                let mut name_r = r;
                name_r.min.x += 2.0;
                name_r.max.x -= 2.0;
                name_r.min.y += 2.0;
                let clip = p.with_clip_rect(name_r.intersect(rect));
                let fs = name_fs.clamp(8.0, 13.0);
                clip.text(
                    egui::pos2(r.center().x, r.top() + 3.0 * z),
                    egui::Align2::CENTER_TOP,
                    helion_gui::schematic_kind_label(&sy.kind),
                    egui::FontId::monospace(fs * 0.9),
                    theme.info,
                );
                clip.text(
                    egui::pos2(r.center().x, r.bottom() - 2.0 * z),
                    egui::Align2::CENTER_BOTTOM,
                    &sy.name,
                    egui::FontId::monospace(fs),
                    theme.text,
                );
            }
            for pin in &sy.pins {
                let nc = pin.net.is_empty();
                if !chrome::schematic_pin_visible(nc, on) {
                    continue;
                }
                let tip = egui::pos2(o.x + pin.x * z, o.y + pin.y * z);
                let edge = if pin.output {
                    egui::pos2(r.right(), tip.y)
                } else {
                    egui::pos2(r.left(), tip.y)
                };
                let inner = if pin.output {
                    egui::pos2(r.right() - 10.0 * z, tip.y)
                } else {
                    egui::pos2(r.left() + 10.0 * z, tip.y)
                };
                let stub = theme.text;
                p.line_segment([inner, edge], Stroke::new(2.0_f32, stub));
                p.line_segment([edge, tip], Stroke::new(2.0_f32, stub));
                p.circle_filled(tip, (2.2 * z).clamp(1.2, 3.0), stub);
                if show_pins {
                    let label = if nc {
                        format!("{} n/c", pin.name)
                    } else {
                        pin.name.clone()
                    };
                    let label_pos = if pin.output {
                        egui::pos2(r.right() - 4.0 * z, tip.y)
                    } else {
                        egui::pos2(r.left() + 4.0 * z, tip.y)
                    };
                    p.text(
                        label_pos,
                        if pin.output {
                            egui::Align2::RIGHT_CENTER
                        } else {
                            egui::Align2::LEFT_CENTER
                        },
                        label,
                        egui::FontId::monospace(pin_fs.clamp(6.5, 11.0)),
                        if nc { theme.faint } else { theme.dim },
                    );
                }
            }
        }
        }
        if resp.clicked() || resp.double_clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                if let Some(layer) = &block_layer {
                    // Block treemap: hit-test in view space. Single-click
                    // selects (cross-probes); double-click drills into the
                    // instance via Expand Inside.
                    if let Some(i) = schem_lod::hit_block(layer, rect, pos) {
                        let name = layer.blocks[i].name.clone();
                        pick = Some(name.clone());
                        if resp.double_clicked() && !layer.blocks[i].is_top {
                            if model.schematic.is_instance(&name) {
                                let _ = model.expand_inside(&name);
                                model.schematic.zoom_fit();
                            }
                        }
                    }
                } else {
                let lx = (pos.x - rect.left() - cam.pan_x) / z;
                let ly = (pos.y - rect.top() - cam.pan_y) / z;
                for sy in drawing.symbols.iter().rev() {
                    if lx >= sy.x && lx <= sy.x + sy.w && ly >= sy.y && ly <= sy.y + sy.h {
                        pick = Some(sy.name.clone());
                        if resp.double_clicked() && !sy.kind.starts_with("PORT") {
                            expand = Some(sy.name.clone());
                        }
                        break;
                    }
                }
                // Vivado Schematic: click a net/wire for Mark Debug / Add Probe.
                if pick.is_none() {
                    let mut best: Option<(f32, String)> = None;
                    let thresh = (8.0 / z).clamp(4.0, 14.0);
                    for w in &drawing.wires {
                        for pair in w.points.windows(2) {
                            let (ax, ay) = pair[0];
                            let (bx, by) = pair[1];
                            let dx = bx - ax;
                            let dy = by - ay;
                            let len2 = dx * dx + dy * dy;
                            let tseg = if len2 < 1e-6 {
                                0.0
                            } else {
                                ((lx - ax) * dx + (ly - ay) * dy) / len2
                            }
                            .clamp(0.0, 1.0);
                            let px = ax + tseg * dx;
                            let py = ay + tseg * dy;
                            let dist = (lx - px).hypot(ly - py);
                            if dist <= thresh {
                                if best.as_ref().map(|(bd, _)| dist < *bd).unwrap_or(true) {
                                    best = Some((dist, w.net.clone()));
                                }
                            }
                        }
                    }
                    if let Some((_, net)) = best {
                        pick = Some(net);
                    }
                }
                }
            }
        }
    }
    if let Some(id) = pick {
        // Cross-probe: schematic selection reflects in Device + Timing.
        model.set_probe(ProbeSelection::Object(id));
    }
    if let Some(id) = expand {
        if model.schematic.is_instance(&id) {
            let _ = model.expand_inside(&id);
        } else {
            let _ = model.expand_cone(&id);
        }
    }
}

/// Die coords of the current selection (site name, occupant cell, or BEL),
/// for zoom-to-selection. `None` when nothing selectable is selected.
fn selected_site_xy(model: &IdeModel) -> Option<(u32, u32)> {
    let id = model.selected.as_deref()?;
    model.device.sites.iter().find_map(|s| {
        let hit = s.site_name() == id
            || s.occupant.as_deref() == Some(id)
            || s.bels.iter().any(|b| b == id);
        hit.then_some((s.x, s.y))
    })
}

/// Clear selection, rulers, solo, and cross-probe highlight (Esc / Clear).
fn clear_physview_selection(model: &mut IdeModel, pv: &mut physview_panels::PhysViewState) {
    model.selected = None;
    model.schematic.highlight_nets.clear();
    model.schematic.highlight_cells.clear();
    model.clear_probe();
    pv.multi_select.clear();
    pv.rulers.clear();
    pv.ruler_anchor = None;
    pv.inspector_ruler = None;
    pv.layers.clear_solo();
    model.highlight_device_routes();
}

fn paint_device_toolbar(
    ui: &mut egui::Ui,
    model: &mut IdeModel,
    _theme: &theme::Theme,
    pv: &mut physview_panels::PhysViewState,
    zoom_target: &mut Option<(u32, u32)>,
) {
    ui.horizontal(|ui| {
        if ui.button("Fit").on_hover_text("Zoom the die to fit (F)").clicked() {
            let _ = model.device_zoom_fit();
        }
        if ui
            .button("Find...")
            .on_hover_text("Find a site, cell, or net (Ctrl+K)")
            .clicked()
        {
            pv.find_open = true;
            pv.find_query.clear();
            pv.find_selected = 0;
            pv.find_needs_focus = true;
        }
        let ruler_text = if pv.ruler_tool { "Ruler [on]" } else { "Ruler" };
        if ui
            .button(ruler_text)
            .on_hover_text("Ruler tool: click-drag on the die to measure (R)")
            .clicked()
        {
            pv.ruler_tool = !pv.ruler_tool;
            pv.ruler_anchor = None;
        }
        if ui
            .button("Clear")
            .on_hover_text("Clear selection, rulers, and solo (Esc)")
            .clicked()
        {
            clear_physview_selection(model, pv);
        }
        ui.separator();
        if ui
            .small_button(if pv.show_display_control {
                "Layers [-]"
            } else {
                "Layers [+]"
            })
            .on_hover_text("Toggle the Display Control dock")
            .clicked()
        {
            pv.show_display_control = !pv.show_display_control;
        }
        if ui
            .small_button(if pv.show_inspector {
                "Inspector [-]"
            } else {
                "Inspector [+]"
            })
            .on_hover_text("Toggle the Inspector dock")
            .clicked()
        {
            pv.show_inspector = !pv.show_inspector;
        }
        ui.separator();
        if ui
            .button("Zoom to selection")
            .on_hover_text("Zoom the die to the selected site/net")
            .clicked()
        {
            if let Some((x, y)) = selected_site_xy(model) {
                model.device_zoom = 2.5;
                *zoom_target = Some((x, y));
            }
        }
        if ui.button("Zoom In").clicked() {
            let _ = model.device_zoom_in();
        }
        if ui.button("Zoom Out").clicked() {
            let _ = model.device_zoom_out();
        }
        if ui.button("Zoom Fit").clicked() {
            let _ = model.device_zoom_fit();
        }
        let dbg_net = model.selected_debug_net();
        ui.add_enabled_ui(dbg_net.is_some(), |ui| {
            let md = match &dbg_net {
                Some(n) => format!("Mark Debug {n}"),
                None => "Mark Debug".into(),
            };
            if ui
                .button(md)
                .on_hover_text("UG908 Mark Debug on the selected Device route/net")
                .clicked()
            {
                let _ = model.exec("mark_debug");
            }
            if ui
                .button("Add Probe")
                .on_hover_text("UG908 Add Probe to ILA dashboard")
                .clicked()
            {
                let _ = model.exec("add_probe");
            }
        });
        ui.label(
            RichText::new(format!("Zoom {:.0}%", model.device_zoom * 100.0))
                .small()
                .weak(),
        );
    });
}

/// Device-view keyboard shortcuts. Guarded to the Device workspace and to
/// "not typing" so editor and Tcl input is never stolen.
fn device_shortcuts(
    ui: &mut egui::Ui,
    model: &mut IdeModel,
    pv: &mut physview_panels::PhysViewState,
) {
    use helion_gui::physview::{HeatMode, LayerId, ALL_LAYERS};
    if model.workspace != WorkspaceTab::Device {
        return;
    }
    let ctx = ui.ctx();
    if ctx.wants_keyboard_input() {
        return;
    }
    let num_keys = [
        egui::Key::Num1,
        egui::Key::Num2,
        egui::Key::Num3,
        egui::Key::Num4,
        egui::Key::Num5,
        egui::Key::Num6,
        egui::Key::Num7,
        egui::Key::Num8,
        egui::Key::Num9,
    ];
    for (i, layer) in ALL_LAYERS.iter().enumerate().take(9) {
        if ctx.input_mut(|inp| inp.consume_key(egui::Modifiers::NONE, num_keys[i])) {
            let on = !pv.layers.is_visible(*layer);
            pv.layers.set_visible(*layer, on);
        }
    }
    if ctx.input_mut(|inp| inp.consume_key(egui::Modifiers::NONE, egui::Key::Num0)) {
        // Heat-map master: toggle congestion; pressing again turns it off.
        let on = pv.layers.heat_mode != HeatMode::Congestion;
        pv.layers.set_visible(LayerId::CongestionHeat, on);
    }
    if ctx.input_mut(|inp| inp.consume_key(egui::Modifiers::NONE, egui::Key::F)) {
        let _ = model.device_zoom_fit();
    }
    if ctx.input_mut(|inp| inp.consume_key(egui::Modifiers::NONE, egui::Key::R)) {
        pv.ruler_tool = !pv.ruler_tool;
        pv.ruler_anchor = None;
    }
    if ctx.input_mut(|inp| inp.consume_key(egui::Modifiers::CTRL, egui::Key::K)) {
        pv.find_open = true;
        pv.find_query.clear();
        pv.find_selected = 0;
        pv.find_needs_focus = true;
    }
    if ctx.input_mut(|inp| inp.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        if pv.find_open {
            pv.find_open = false;
            pv.find_query.clear();
        } else {
            clear_physview_selection(model, pv);
        }
    }
}

fn paint_device(
    ui: &mut egui::Ui,
    model: &mut IdeModel,
    theme: &theme::Theme,
    pv: &mut physview_panels::PhysViewState,
    zoom_target: &mut Option<(u32, u32)>,
) {
    ui.heading("Device");
    paint_device_toolbar(ui, model, theme, pv, zoom_target);
    device_shortcuts(ui, model, pv);
    physview_panels::paint_find_dialog(ui.ctx(), model, pv, theme, zoom_target);

    let screen = ui.ctx().screen_rect();
    let narrow = chrome::device_window_narrow(screen.width());
    let short = chrome::device_window_short(screen.height());
    let docked = !(narrow || short);

    // Tables band (pblocks + clock regions), same shares as before.
    let after_toolbar = ui.available_height().max(1.0);
    let band = chrome::device_band_share(screen.width(), screen.height(), after_toolbar);
    let pane_w = ui.available_width().max(48.0);
    if narrow || short {
        // Tables and legend floor, then scroll horizontally. Die keeps the leftover.
        egui::ScrollArea::both()
            .id_salt("device_tables")
            .auto_shrink([false, true])
            .max_height(band.tables_h)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                ui.set_min_width(pane_w.max(640.0));
                paint_pblocks_table(ui, model);
            });
        ui.add_space(2.0);
        // Clock-region viewport is whole rows only so the last line is not sliced.
        egui::ScrollArea::both()
            .id_salt("device_clock_regions_block")
            .auto_shrink([false, false])
            .max_height(band.clock_h)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                ui.set_min_width(pane_w.max(560.0));
                ui.set_min_height(band.clock_h);
                paint_clock_regions(ui, model);
            });
    } else {
        // Wide: short table caps; the die ScrollArea takes the real leftover height.
        let tables_h = band.tables_h;
        egui::ScrollArea::both()
            .id_salt("device_tables")
            .auto_shrink([false, true])
            .max_height(tables_h)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                ui.set_max_width(pane_w);
                ui.set_width(pane_w);
                paint_pblocks_table(ui, model);
            });
        let cr_h = band.clock_h;
        egui::ScrollArea::vertical()
            .id_salt("device_clock_regions_block")
            .auto_shrink([false, true])
            .max_height(cr_h)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                ui.set_max_width(pane_w);
                ui.set_width(pane_w);
                paint_clock_regions(ui, model);
            });
    }
    ui.separator();

    if docked {
        let band_h = ui.available_height().max(1.0);
        ui.horizontal(|ui| {
            if pv.show_display_control {
                ui.allocate_ui_with_layout(
                    egui::vec2(234.0, band_h),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("pv_display_control")
                            .show(ui, |ui| {
                                ui.set_min_width(216.0);
                                physview_panels::paint_display_control(ui, pv, model, theme);
                            });
                    },
                );
            } else if ui
                .small_button(">")
                .on_hover_text("Show Display Control")
                .clicked()
            {
                pv.show_display_control = true;
            }
            // Die fills the middle.
            {
                let insp_w = if pv.show_inspector { 272.0 } else { 30.0 };
                let w = (ui.available_width() - insp_w).max(220.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(w, band_h),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        paint_device_die(ui, model, theme, pv, zoom_target);
                    },
                );
            }
            if pv.show_inspector {
                ui.allocate_ui_with_layout(
                    egui::vec2(268.0, band_h),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("pv_inspector")
                            .show(ui, |ui| {
                                ui.set_min_width(252.0);
                                physview_panels::paint_inspector(ui, model, pv, theme, zoom_target);
                            });
                    },
                );
            } else if ui
                .small_button("<")
                .on_hover_text("Show Inspector")
                .clicked()
            {
                pv.show_inspector = true;
            }
        });
    } else {
        // Narrow windows: docks float so nothing is lost.
        if pv.show_display_control {
            egui::Window::new("Display Control")
                .collapsible(false)
                .show(ui.ctx(), |ui| {
                    ui.set_min_width(216.0);
                    physview_panels::paint_display_control(ui, pv, model, theme);
                });
        }
        if pv.show_inspector {
            egui::Window::new("Inspector")
                .collapsible(false)
                .show(ui.ctx(), |ui| {
                    ui.set_min_width(252.0);
                    physview_panels::paint_inspector(ui, model, pv, theme, zoom_target);
                });
        }
        paint_device_die(ui, model, theme, pv, zoom_target);
    }
}

/// Die tile under a pointer position, if the pointer is over the die.
fn die_tile_at(
    pos: egui::Pos2,
    origin: egui::Pos2,
    cell_w: f32,
    cell_h: f32,
    x0: u32,
    y0: u32,
    cols: u32,
    rows: u32,
) -> Option<(u32, u32)> {
    let dx = ((pos.x - origin.x) / cell_w).floor() as i32;
    let dy = ((pos.y - origin.y) / cell_h).floor() as i32;
    if dx >= 0 && dy >= 0 && (dx as u32) < cols && (dy as u32) < rows {
        Some((x0 + dx as u32, y0 + (rows - 1 - dy as u32)))
    } else {
        None
    }
}

/// Tile-center pixel for a die tile (Y-up die coords to Y-down pixels).
fn tile_center_px(
    x: u32,
    y: u32,
    origin: egui::Pos2,
    cell_w: f32,
    cell_h: f32,
    x0: u32,
    y0: u32,
    rows: u32,
) -> egui::Pos2 {
    let dx = x.saturating_sub(x0);
    let dy = rows
        .saturating_sub(1)
        .saturating_sub(y.saturating_sub(y0));
    egui::pos2(
        origin.x + dx as f32 * cell_w + cell_w / 2.0,
        origin.y + dy as f32 * cell_h + cell_h / 2.0,
    )
}

fn dist_to_segment(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len2 = abx * abx + aby * aby;
    if len2 < 1e-6 {
        return ((p.x - a.x).powi(2) + (p.y - a.y).powi(2)).sqrt();
    }
    let t = (((p.x - a.x) * abx + (p.y - a.y) * aby) / len2).clamp(0.0, 1.0);
    let qx = a.x + abx * t;
    let qy = a.y + aby * t;
    ((p.x - qx).powi(2) + (p.y - qy).powi(2)).sqrt()
}

/// Net whose route polyline passes within ~4 px of the pointer (nearest wins).
fn route_hit_at(
    model: &IdeModel,
    pos: egui::Pos2,
    origin: egui::Pos2,
    cell_w: f32,
    cell_h: f32,
    x0: u32,
    y0: u32,
    rows: u32,
) -> Option<String> {
    let mut best: Option<(f32, String)> = None;
    for rt in &model.device.routes {
        if rt.tiles.len() < 2 {
            continue;
        }
        let pts: Vec<egui::Pos2> = rt
            .tiles
            .iter()
            .map(|&(x, y)| tile_center_px(x, y, origin, cell_w, cell_h, x0, y0, rows))
            .collect();
        let d = pts
            .windows(2)
            .map(|w| dist_to_segment(pos, w[0], w[1]))
            .fold(f32::INFINITY, f32::min);
        if d <= 4.0 && best.as_ref().map(|(bd, _)| d < *bd).unwrap_or(true) {
            best = Some((d, rt.net.clone()));
        }
    }
    best.map(|(_, n)| n)
}

/// Ruler measurement whose line passes within ~4 px of the pointer.
fn ruler_hit_at(
    rulers: &[helion_gui::physview::RulerMeasurement],
    pos: egui::Pos2,
    to_px: &impl Fn(u32, u32) -> egui::Pos2,
) -> Option<usize> {
    rulers.iter().enumerate().find_map(|(i, m)| {
        let a = to_px(m.x0, m.y0);
        let b = to_px(m.x1, m.y1);
        (dist_to_segment(pos, a, b) <= 4.0).then_some(i)
    })
}

/// The die canvas: tile grid, heat maps, sites, clock regions, pblocks,
/// routes, rulers, hover tooltips, click/drag interaction.
fn paint_device_die(
    ui: &mut egui::Ui,
    model: &mut IdeModel,
    theme: &theme::Theme,
    pv: &mut physview_panels::PhysViewState,
    zoom_target: &mut Option<(u32, u32)>,
) {
    let cols = model.device.cols.max(1);
    let rows = model.device.rows.max(1);
    let x0 = model.device.x0;
    let y0 = model.device.y0;
    let avail = ui.available_size();
    // Real leftover pane only - do not inflate past it (that jammed an oversized die).
    let view_h = avail.y.max(1.0);
    let view_w = avail.x.max(1.0);
    // Short leftover: Zoom Fit into this rect (zoom 1.0) so Y=0 is not a clipped 100% band.
    if model.device_zoom <= 1.001 {
        model.device_zoom = 1.0;
    }
    let (mut cell_w, mut cell_h) =
        chrome::floorplan_zoom_cells(cols, rows, view_w, view_h, model.device_zoom);
    // P1-3 rendering polish: square cells, always. A stretched die (wide flat
    // tiles) reads as a rendering bug next to Vivado; letterbox instead.
    let sq = cell_w.min(cell_h).max(0.5);
    cell_w = sq;
    cell_h = sq;
    let mut die_w = chrome::FLOORPLAN_LEFT + cell_w * cols as f32 + chrome::FLOORPLAN_RIGHT;
    let mut die_h = chrome::FLOORPLAN_TOP + cell_h * rows as f32 + chrome::FLOORPLAN_BOT;
    // Fit must occupy the leftover, never paint a taller band and hide the bottom.
    if model.device_zoom <= 1.001 && (die_w > view_w + 0.5 || die_h > view_h + 0.5) {
        let sx = (view_w - 1.0).max(1.0) / die_w.max(1.0);
        let sy = (view_h - 1.0).max(1.0) / die_h.max(1.0);
        let s = sx.min(sy);
        cell_w *= s;
        cell_h *= s;
        die_w *= s;
        die_h *= s;
    }
    let overflow = model.device_zoom > 1.001 && (die_w > view_w + 1.0 || die_h > view_h + 1.0);
    let zoomed_in = overflow;
    let draw_w = if overflow {
        die_w.max(view_w + 8.0)
    } else {
        view_w.max(die_w)
    };
    let draw_h = if overflow {
        die_h.max(view_h + 8.0)
    } else {
        view_h.max(die_h)
    };
    // Center the die in the viewport whenever it is smaller than it (fit /
    // letterbox); top-left only when zoomed past the viewport.
    let origin_dx = (view_w - die_w).max(0.0) * 0.5;
    let origin_dy = (view_h - die_h).max(0.0) * 0.5;
    let mut pick_site: Option<(u32, u32)> = None;
    let mut pick_region: Option<String> = None;
    let mut click_pblock: Option<String> = None;
    let mut ctx_mark_debug = false;
    let mut ctx_add_probe = false;
    let mut ctx_zoom_fit = false;
    let mut scroller = egui::ScrollArea::both()
        .id_salt("device_die_scroll")
        .auto_shrink([false, false])
        .max_width(view_w)
        .max_height(view_h)
        .scroll_bar_visibility(if zoomed_in {
            egui::scroll_area::ScrollBarVisibility::AlwaysVisible
        } else {
            egui::scroll_area::ScrollBarVisibility::AlwaysHidden
        });
    if zoomed_in && std::env::var("HELION_DEVICE_SCROLL").ok().as_deref() == Some("bottom") {
        scroller = scroller.vertical_scroll_offset(1.0e6);
    }
    scroller.show(ui, |ui| {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(draw_w, draw_h), Sense::click());
        if ui.is_rect_visible(rect) {
            let origin = egui::pos2(
                rect.left() + chrome::FLOORPLAN_LEFT + origin_dx,
                rect.top() + chrome::FLOORPLAN_TOP + origin_dy,
            );
            let p = ui.painter();
            p.rect_filled(rect, 0.0, theme.die_bg);
            // Physical View 2.0: congestion heat map paints under the sites
            // but over the die background.
            if pv.layers.is_visible(helion_gui::physview::LayerId::CongestionHeat) {
                let max_u = model.device.congestion.iter().copied().max().unwrap_or(0);
                if max_u > 0 {
                    let op = pv.layers.heat_opacity
                        * pv.layers.layer_alpha(helion_gui::physview::LayerId::CongestionHeat);
                    let n = model.device.congestion.len();
                    for dyy in 0..rows {
                        for dxx in 0..cols {
                            // Die coords are Y-up: paint row dyy is die row (rows-1-dyy).
                            let idx = (rows - 1 - dyy) as usize * cols as usize + dxx as usize;
                            if idx >= n {
                                continue;
                            }
                            let u = model.device.congestion[idx];
                            if u == 0 {
                                continue;
                            }
                            let rgb = helion_gui::physview::congestion_ramp(
                                helion_gui::physview::congestion_t(u, max_u),
                            );
                            let px = origin.x + dxx as f32 * cell_w;
                            let py = origin.y + dyy as f32 * cell_h;
                            let htile = egui::Rect::from_min_size(
                                egui::pos2(px + 0.5, py + 0.5),
                                egui::vec2((cell_w - 1.0).max(1.0), (cell_h - 1.0).max(1.0)),
                            );
                            p.rect_filled(
                                htile,
                                1.0,
                                Color32::from_rgb(rgb[0], rgb[1], rgb[2]).gamma_multiply(op),
                            );
                        }
                    }
                }
            }
            // Cross-probe: while a timing path is probed, its sites paint
            // gold and everything else dims (Vivado-style focus).
            let path_probe = matches!(model.probe(), ProbeSelection::Path(_));
            for dx in 0..cols {
                let x = x0 + dx;
                let px = origin.x + dx as f32 * cell_w;
                if dx % 4 == 0 {
                    p.text(
                        egui::pos2(px + 1.0, rect.bottom() - 12.0),
                        egui::Align2::LEFT_BOTTOM,
                        format!("{x}"),
                        egui::FontId::monospace(8.0),
                        theme.faint,
                    );
                }
            }
            for dy in 0..rows {
                // HAD y=0 IOB at the bottom of the die (Vivado Y-up).
                let y = y0 + (rows - 1 - dy);
                let py = origin.y + dy as f32 * cell_h;
                if dy % 4 == 0 {
                    p.text(
                        egui::pos2(rect.left() + 2.0, py + 1.0),
                        egui::Align2::LEFT_TOP,
                        format!("{y}"),
                        egui::FontId::monospace(8.0),
                        theme.faint,
                    );
                }
                for dx in 0..cols {
                    let x = x0 + dx;
                    let px = origin.x + dx as f32 * cell_w;
                    let tile = egui::Rect::from_min_size(
                        egui::pos2(px + 0.5, py + 0.5),
                        egui::vec2((cell_w - 1.0).max(1.0), (cell_h - 1.0).max(1.0)),
                    );
                    let site = model.device.site_at(x, y);
                    let path_hl = site.is_some_and(|s| s.highlighted);
                    // Physical View 2.0: per-site-kind layer visibility + solo dimming.
                    let site_layer =
                        site.and_then(|s| helion_gui::physview::LayerId::for_site_kind(s.kind));
                    let site_on = site_layer
                        .map(|l| pv.layers.is_visible(l))
                        .unwrap_or(true);
                    let solo_alpha = site_layer
                        .map(|l| pv.layers.layer_alpha(l))
                        .unwrap_or(1.0);
                    // Cross-probe: a path probe paints gold; a single-object
                    // probe paints the accent outline. Non-path sites dim
                    // while a path is probed (Vivado-style focus).
                    let fill = if !site_on {
                        theme.die_bg
                    } else if path_hl && path_probe {
                        // STA path: gold fill so multi-cell paths read on the die.
                        theme.gold.gamma_multiply(solo_alpha)
                    } else {
                        let base = match site {
                            Some(s) if s.occupant.is_some() || !s.bels.is_empty() => {
                                match s.occupancy_char() {
                                    'O' => theme.info,
                                    'L' | 'C' => theme.ok,
                                    _ => theme.gold,
                                }
                            }
                            Some(s) => match s.kind {
                                helion_device::SiteKind::Iob => theme.cat_blue.gamma_multiply(0.38),
                                helion_device::SiteKind::Bram => theme.violet.gamma_multiply(0.38),
                                helion_device::SiteKind::Dsp => theme.amber.gamma_multiply(0.38),
                                helion_device::SiteKind::Clk => theme.gold.gamma_multiply(0.38),
                                helion_device::SiteKind::Clb => theme.ok.gamma_multiply(0.32),
                            },
                            None => theme.die_bg,
                        };
                        if path_probe {
                            base.gamma_multiply(0.45 * solo_alpha)
                        } else {
                            base.gamma_multiply(solo_alpha)
                        }
                    };
                    p.rect_filled(tile, 1.0, fill);
                    let selected = path_hl
                        || site.is_some_and(|s| {
                            let id = model.selected.as_deref();
                            id == s.occupant.as_deref()
                                || id == Some(s.site_name().as_str())
                                || s.bels.iter().any(|b| Some(b.as_str()) == id)
                        });
                    if selected {
                        let (sw, scol) = if path_hl && path_probe {
                            (2.2_f32, theme.gold)
                        } else {
                            (2.5_f32, theme.accent)
                        };
                        p.rect_stroke(
                            tile,
                            1.0,
                            Stroke::new(sw, scol),
                            egui::StrokeKind::Outside,
                        );
                    }
                }
            }
            // Fig. 49: clock-region outlines over the die.
            // Cross-probe state for the die: path probes paint gold, single
            // selections paint the accent color, everything else dims.
            let path_probe = model.selected_timing_path.is_some();
            let purple = theme.violet;
            let gold = theme.gold;
            let amber = theme.amber;
            if pv.layers.is_visible(helion_gui::physview::LayerId::ClockRegions) {
                let cr_alpha =
                    pv.layers.layer_alpha(helion_gui::physview::LayerId::ClockRegions);
                for cr in &model.device.clock_regions {
                    let px = origin.x + (cr.x0 - x0) as f32 * cell_w;
                    let py = origin.y + (rows - 1 - (cr.y1 - y0)) as f32 * cell_h;
                    let pw = cr.cols() as f32 * cell_w;
                    let ph = cr.rows() as f32 * cell_h;
                    let rr = egui::Rect::from_min_size(egui::pos2(px, py), egui::vec2(pw, ph));
                    let on = model.selected.as_deref() == Some(cr.name.as_str());
                    let stroke_col = if on { gold } else { purple }.gamma_multiply(cr_alpha);
                    let text_col = if on { gold } else { purple }.gamma_multiply(cr_alpha);
                    p.rect_stroke(
                        rr,
                        0.0,
                        Stroke::new(if on { 3.0_f32 } else { 2.0_f32 }, stroke_col),
                        egui::StrokeKind::Inside,
                    );
                    p.text(
                        egui::pos2(rr.left() + 3.0, rr.top() + 2.0),
                        egui::Align2::LEFT_TOP,
                        &cr.name,
                        egui::FontId::monospace(9.0),
                        text_col,
                    );
                }
            }
            // Pblock rectangles (create_pblock / resize_pblock).
            if pv.layers.is_visible(helion_gui::physview::LayerId::Pblocks) {
                let pb_alpha = pv.layers.layer_alpha(helion_gui::physview::LayerId::Pblocks);
                for pb in &model.pblocks {
                    if !pb.ranged {
                        continue;
                    }
                    let px = origin.x + (pb.x0 - x0) as f32 * cell_w;
                    let py = origin.y + (rows - 1 - (pb.y1 - y0)) as f32 * cell_h;
                    let pw = pb.cols() as f32 * cell_w;
                    let ph = pb.rows() as f32 * cell_h;
                    let rr = egui::Rect::from_min_size(egui::pos2(px, py), egui::vec2(pw, ph));
                    let on = model.selected.as_deref() == Some(pb.name.as_str());
                    let stroke_col = if on { gold } else { amber }.gamma_multiply(pb_alpha);
                    let text_col = if on { gold } else { amber }.gamma_multiply(pb_alpha);
                    p.rect_stroke(
                        rr,
                        0.0,
                        Stroke::new(if on { 3.0_f32 } else { 2.0_f32 }, stroke_col),
                        egui::StrokeKind::Inside,
                    );
                    p.text(
                        egui::pos2(rr.left() + 3.0, rr.top() + 2.0),
                        egui::Align2::LEFT_TOP,
                        &pb.name,
                        egui::FontId::monospace(9.0),
                        text_col,
                    );
                }
                // PathFinder IOB nets over the die.
                if pv.layers.is_visible(helion_gui::physview::LayerId::Routes) {
                    let route_col = theme.ok;
                    let unroute_col = theme.faint;
                    let route_alpha =
                        pv.layers.layer_alpha(helion_gui::physview::LayerId::Routes);
                    // Physical View 2.0: criticality heat map colors each
                    // route polyline by its worst-slack bucket.
                    let crit_on = pv.layers.is_visible(
                        helion_gui::physview::LayerId::CriticalityHeat,
                    );
                    for rt in &model.device.routes {
                        if rt.tiles.len() < 2 {
                            continue;
                        }
                        let crit_col = if crit_on {
                            model.device.net_slack.get(&rt.net).map(|slack| {
                                let rgb = helion_gui::physview::slack_color(
                                    helion_gui::physview::slack_bucket(*slack),
                                );
                                Color32::from_rgb(rgb[0], rgb[1], rgb[2])
                            })
                        } else {
                            None
                        };
                        // Path nets paint gold and thick; a singly selected net
                        // paints the accent color; the rest dim while probing.
                        let hl_path_net = path_probe
                            && model.schematic.highlight_nets.contains(&rt.net);
                        let (col, thick) = if hl_path_net {
                            (gold, 2.6_f32)
                        } else if rt.highlighted {
                            (theme.accent, 2.2_f32)
                        } else if let Some(cc) = crit_col {
                            (cc, 2.0_f32)
                        } else if rt.hops == 0 {
                            (
                                if path_probe {
                                    unroute_col.gamma_multiply(0.45)
                                } else {
                                    unroute_col
                                },
                                1.7_f32,
                            )
                        } else {
                            (
                                if path_probe {
                                    route_col.gamma_multiply(0.45)
                                } else {
                                    route_col
                                },
                                1.7_f32,
                            )
                        };
                        let col = col.gamma_multiply(route_alpha);
                        let mut pts = Vec::new();
                        for &(x, y) in &rt.tiles {
                            let dx = x.saturating_sub(x0);
                            let dy = rows.saturating_sub(1).saturating_sub(y.saturating_sub(y0));
                            let cx = origin.x + dx as f32 * cell_w + cell_w / 2.0;
                            let cy = origin.y + dy as f32 * cell_h + cell_h / 2.0;
                            pts.push(egui::pos2(cx, cy));
                        }
                        for w in pts.windows(2) {
                            if rt.hops == 0 {
                                paint_dotted(p, w[0], w[1], Stroke::new(thick, col));
                            } else {
                                p.line_segment([w[0], w[1]], Stroke::new(thick, col));
                            }
                        }
                        if let (Some(&a), Some(&b)) = (pts.first(), pts.last()) {
                            p.circle_filled(a, 2.4, col);
                            p.circle_filled(b, 2.4, col);
                        }
                    }
                } // routes layer
                // Physical View 2.0: ruler measurements over the routes.
                if pv.layers.is_visible(helion_gui::physview::LayerId::Rulers)
                    && (!pv.rulers.is_empty() || pv.ruler_anchor.is_some())
                {
                    let to_px =
                        |x: u32, y: u32| tile_center_px(x, y, origin, cell_w, cell_h, x0, y0, rows);
                    let draft = match (pv.ruler_anchor, resp.hover_pos()) {
                        (Some(a), Some(pos)) => die_tile_at(
                            pos, origin, cell_w, cell_h, x0, y0, cols, rows,
                        )
                        .map(|b| (a, b)),
                        _ => None,
                    };
                    physview_panels::paint_rulers(p, &pv.rulers, draft, &to_px, theme);
                }
            } // layers.pblocks
            if let Some(pos) = resp.hover_pos() {
                let dx = ((pos.x - origin.x) / cell_w).floor() as i32;
                let dy = ((pos.y - origin.y) / cell_h).floor() as i32;
                if dx >= 0 && dy >= 0 && (dx as u32) < cols && (dy as u32) < rows {
                    let x = x0 + dx as u32;
                    let y = y0 + (rows - 1 - dy as u32);
                    let mut tip = String::new();
                    if let Some(pb) = model.pblocks.iter().find(|p| p.contains(x, y)) {
                        tip.push_str(&format!(
                            "{}  {}  frames={}",
                            pb.name,
                            pb.english_range(),
                            pb.frames
                        ));
                    }
                    if let Some(cr) = model.device.clock_region_at(x, y) {
                        if !tip.is_empty() {
                            tip.push(' ');
                        }
                        tip.push_str(&format!(
                            "{}  sites={}",
                            cr.name,
                            cr.site_count(&model.device.sites)
                        ));
                    }
                    if let Some(s) = model.device.site_at(x, y) {
                        if !tip.is_empty() {
                            tip.push(' ');
                        }
                        // Physical View 2.0: occupant + BEL count on the site line.
                        let occ = s.occupant.as_deref().unwrap_or("-");
                        if s.bels.is_empty() {
                            tip.push_str(&format!("{}  occ={}", s.site_name(), occ));
                        } else {
                            tip.push_str(&format!(
                                "{}  occ={}  bels={} ({})",
                                s.site_name(),
                                occ,
                                s.bels.len(),
                                s.bels.join(",")
                            ));
                        }
                    }
                    if let Some(rt) = model.device.route_at(x, y) {
                        if !tip.is_empty() {
                            tip.push(' ');
                        }
                        tip.push_str(&format!(
                            "route {} hops={} delay_ps={}",
                            rt.net, rt.hops, rt.delay_ps
                        ));
                    }
                    if !tip.is_empty() {
                        resp.clone().on_hover_text(tip);
                    }
                }
            }
            // Zoom-to-selection: center the scroll area on the target tile.
            if let Some((tx, ty)) = zoom_target.take() {
                let dx = tx.saturating_sub(x0);
                let dy = rows.saturating_sub(1).saturating_sub(ty.saturating_sub(y0));
                let target = egui::Rect::from_min_size(
                    egui::pos2(origin.x + dx as f32 * cell_w, origin.y + dy as f32 * cell_h),
                    egui::vec2(cell_w * 4.0, cell_h * 4.0),
                );
                ui.scroll_to_rect(target, Some(egui::Align::Center));
            }
        }
        // Physical View 2.0: ruler tool consumes drags; plain clicks select.
        if pv.ruler_tool {
            if resp.drag_started() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let origin = egui::pos2(
                        rect.left() + chrome::FLOORPLAN_LEFT + origin_dx,
                        rect.top() + chrome::FLOORPLAN_TOP + origin_dy,
                    );
                    pv.ruler_anchor =
                        die_tile_at(pos, origin, cell_w, cell_h, x0, y0, cols, rows);
                }
            }
            if resp.drag_stopped() {
                if let (Some(a), Some(pos)) =
                    (pv.ruler_anchor.take(), resp.interact_pointer_pos())
                {
                    let origin = egui::pos2(
                        rect.left() + chrome::FLOORPLAN_LEFT + origin_dx,
                        rect.top() + chrome::FLOORPLAN_TOP + origin_dy,
                    );
                    if let Some((x, y)) =
                        die_tile_at(pos, origin, cell_w, cell_h, x0, y0, cols, rows)
                    {
                        pv.rulers.push(helion_gui::physview::RulerMeasurement {
                            x0: a.0,
                            y0: a.1,
                            x1: x,
                            y1: y,
                        });
                    }
                }
            }
        } else if resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let origin = egui::pos2(
                    rect.left() + chrome::FLOORPLAN_LEFT + origin_dx,
                    rect.top() + chrome::FLOORPLAN_TOP + origin_dy,
                );
                let dx = ((pos.x - origin.x) / cell_w).floor() as i32;
                let dy = ((pos.y - origin.y) / cell_h).floor() as i32;
                if dx >= 0 && dy >= 0 && (dx as u32) < cols && (dy as u32) < rows {
                    let x = x0 + dx as u32;
                    let y = y0 + (rows - 1 - dy as u32);
                    let mut header = false;
                    if let Some(pb) = model.pblocks.iter().find(|p| p.contains(x, y)) {
                        let py = origin.y + (rows - 1 - (pb.y1 - y0)) as f32 * cell_h;
                        if pos.y - py <= 14.0 {
                            click_pblock = Some(pb.name.clone());
                            header = true;
                        }
                    }
                    if !header {
                        if let Some(cr) = model.device.clock_region_at(x, y) {
                            let py = origin.y + (rows - 1 - (cr.y1 - y0)) as f32 * cell_h;
                            if pos.y - py <= 14.0 {
                                pick_region = Some(cr.name.clone());
                                header = true;
                            }
                        }
                    }
                    if !header {
                        // Route polylines (within ~4 px) beat the site tile.
                        if let Some(net) =
                            route_hit_at(model, pos, origin, cell_w, cell_h, x0, y0, rows)
                        {
                            let shift = ui.input(|i| i.modifiers.shift);
                            if shift {
                                if !pv.multi_select.contains(&net) {
                                    pv.multi_select.push(net.clone());
                                }
                            } else {
                                pv.multi_select.clear();
                            }
                            pv.inspector_ruler = None;
                            model.selected = Some(net);
                            model.highlight_device_routes();
                            header = true;
                        }
                    }
                    if !header {
                        // Ruler lines select the measurement for the inspector.
                        let to_px = |rx: u32, ry: u32| {
                            tile_center_px(rx, ry, origin, cell_w, cell_h, x0, y0, rows)
                        };
                        if let Some(i) = ruler_hit_at(&pv.rulers, pos, &to_px) {
                            pv.inspector_ruler = Some(i);
                            pv.multi_select.clear();
                            model.selected = None;
                            model.highlight_device_routes();
                            header = true;
                        }
                    }
                    if !header {
                        let shift = ui.input(|i| i.modifiers.shift);
                        if shift {
                            let id = format!("X{x}Y{y}");
                            if !pv.multi_select.contains(&id) {
                                pv.multi_select.push(id.clone());
                            }
                            pv.inspector_ruler = None;
                            model.selected = Some(id);
                            model.highlight_device_routes();
                        } else {
                            pick_site = Some((x, y));
                        }
                    }
                }
            }
        }
        // Right-click menu (Vivado pattern): debug actions on the die.
        resp.context_menu(|ui| {
            if ui.button("Mark Debug").clicked() {
                ctx_mark_debug = true;
                ui.close();
            }
            if ui.button("Add Probe").clicked() {
                ctx_add_probe = true;
                ui.close();
            }
            ui.separator();
            if ui.button("Zoom Fit").clicked() {
                ctx_zoom_fit = true;
                ui.close();
            }
        });
    });
    if let Some(name) = click_pblock {
        pv.multi_select.clear();
        pv.inspector_ruler = None;
        let _ = model.select_pblock(&name);
    }
    if let Some(name) = pick_region {
        pv.multi_select.clear();
        pv.inspector_ruler = None;
        let _ = model.select_clock_region(&name);
    }
    if let Some((x, y)) = pick_site {
        pv.multi_select.clear();
        pv.inspector_ruler = None;
        let _ = model.select_device_site(&format!("X{x}Y{y}"));
    }
    if ctx_mark_debug {
        let _ = model.exec("mark_debug");
    }
    if ctx_add_probe {
        let _ = model.exec("add_probe");
    }
    if ctx_zoom_fit {
        let _ = model.device_zoom_fit();
    }
}

fn paint_clock_regions(ui: &mut egui::Ui, model: &mut IdeModel) {
    let regions = model.device.clock_regions.clone();

    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    if regions.is_empty() {
        ui.label("No clock regions on this die.");
        return;
    }
    // Narrow or short: one complete line per region, whole-row height, then scroll.
    // Never a two-line block that slices "Occupied" mid-row or crushes the die.
    let screen = ui.ctx().screen_rect();
    let pane_w = ui
        .clip_rect()
        .width()
        .min(ui.max_rect().width())
        .min(ui.available_width());
    let floor = chrome::device_window_narrow(screen.width())
        || chrome::device_window_short(screen.height())
        || pane_w < 720.0;
    if floor {
        ui.spacing_mut().item_spacing.y = 0.0;
        // Left-aligned heading. add_sized(full width) was painting this on the right.
        ui.horizontal(|ui| {
            ui.set_min_height(chrome::DEVICE_CR_HEAD_H);
            ui.label(RichText::new("Clock Regions").strong());
        });
        for (i, cr) in regions.iter().enumerate() {
            let sites = cr.site_count(&model.device.sites);
            let occ = cr.occupied_count(&model.device.sites);
            let on = selected.as_deref() == Some(cr.name.as_str());
            let line = format!(
                "{}   X0={}  Y0={}  X1={}  Y1={}   Sites={}  Occupied={}",
                cr.name, cr.x0, cr.y0, cr.x1, cr.y1, sites, occ
            );
            let row_w = line.chars().count() as f32 * 7.2 + 24.0;
            ui.allocate_ui_with_layout(
                egui::vec2(row_w, chrome::DEVICE_CR_ROW_H),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    if ui
                        .add_sized(
                            [row_w, chrome::DEVICE_CR_ROW_H],
                            egui::Button::selectable(on, line),
                        )
                        .clicked()
                    {
                        pick = Some(i.to_string());
                    }
                },
            );
        }
        if let Some(spec) = pick {
            let _ = model.select_clock_region(&spec);
        }
        return;
    }
    ui.label(RichText::new("Clock Regions").strong());
    data_scroll("ug893_clock_regions_scroll").show(ui, |ui| {
        egui::Grid::new("ug893_clock_regions")
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("X0").strong());
                ui.label(RichText::new("Y0").strong());
                ui.label(RichText::new("X1").strong());
                ui.label(RichText::new("Y1").strong());
                ui.label(RichText::new("Sites").strong());
                ui.label(RichText::new("Occupied").strong());
                ui.end_row();
                for (i, cr) in regions.iter().enumerate() {
                    let on = selected.as_deref() == Some(cr.name.as_str());
                    let sites = cr.site_count(&model.device.sites);
                    let occ = cr.occupied_count(&model.device.sites);
                    if ui.selectable_label(on, &cr.name).clicked() {
                        pick = Some(i.to_string());
                    }
                    // Numeric cells: right-aligned monospace (Vivado-style).
                    // The X0 value was rendering as "?" with the default UI
                    // font; monospace renders the glyph correctly.
                    if num_cell_selectable(ui, on, &cr.x0.to_string()) {
                        pick = Some(i.to_string());
                    }
                    if num_cell_selectable(ui, on, &cr.y0.to_string()) {
                        pick = Some(i.to_string());
                    }
                    if num_cell_selectable(ui, on, &cr.x1.to_string()) {
                        pick = Some(i.to_string());
                    }
                    if num_cell_selectable(ui, on, &cr.y1.to_string()) {
                        pick = Some(i.to_string());
                    }
                    if num_cell_selectable(ui, on, &sites.to_string()) {
                        pick = Some(i.to_string());
                    }
                    if num_cell_selectable(ui, on, &occ.to_string()) {
                        pick = Some(i.to_string());
                    }
                    ui.end_row();
                }
            });
    });
    if let Some(spec) = pick {
        let _ = model.select_clock_region(&spec);
    }
}

#[allow(dead_code)] // intentional: retained for upcoming device routing inspection panel
fn paint_source(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("Source");
    ui.horizontal(|ui| {
        let n = model.sim_runtime_cycles.max(1);
        if ui.button(format!("Run {n}")).clicked() {
            let _ = model.exec("run_simulation");
        }
        if ui.button("Step").clicked() {
            let _ = model.sim_step();
        }
        if ui.button("Restart").clicked() {
            let _ = model.sim_restart();
        }
        if ui.button("Open").clicked() {
            let _ = model.open_source_window();
        }
        if ui.button("Settings").clicked() {
            let _ = model.exec("simulation_settings");
        }
    });
    let rows = model.source_line_rows().to_vec();
    let selected = model.selected_source_line;
    let pc = model.sim_pc_line;
    let armed: Vec<usize> = model
        .breakpoint_rows()
        .iter()
        .filter(|b| b.kind_cell() == "line" && b.enabled)
        .filter_map(|b| b.line)
        .collect();
    let mut pick_line: Option<String> = None;
    let mut pick_bp: Option<String> = None;
    egui::ScrollArea::both()
        .id_salt("ug900_source")
        .show(ui, |ui| {
            egui::Grid::new("ug900_source_table")
                .spacing([8.0, 2.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("BP").strong());
                    ui.label(RichText::new("Line").strong());
                    ui.label(RichText::new("PC").strong());
                    ui.label(RichText::new("Kind").strong());
                    ui.label(RichText::new("Text").strong());
                    ui.end_row();
                    if rows.is_empty() {
                        ui.label("-");
                        ui.label("-");
                        ui.label("-");
                        ui.label("-");
                        ui.label("No sources yet.");
                        ui.end_row();
                    } else {
                        let mut in_block = false;
                        for r in &rows {
                            let on = selected == Some(r.line);
                            let bp_on = armed.contains(&r.line);
                            let bp_label = if r.executable { r.bp_cell(bp_on) } else { " " };
                            if ui.selectable_label(bp_on, bp_label).clicked() {
                                pick_bp = Some(r.line.to_string());
                            }
                            if ui
                                .selectable_label(on, RichText::new(r.line.to_string()).monospace())
                                .clicked()
                            {
                                pick_line = Some(r.line.to_string());
                            }
                            let pc_txt = r.pc_cell(pc);
                            if ui.selectable_label(pc == Some(r.line), pc_txt).clicked() {
                                pick_line = Some(r.line.to_string());
                            }
                            if ui.selectable_label(on, r.type_cell()).clicked() {
                                pick_line = Some(r.line.to_string());
                            }
                            // Syntax-highlighted source text (Editor colors),
                            // selectable and clickable like the old label.
                            let mut job = egui::text::LayoutJob::default();
                            editor::append_highlighted(
                                &mut job,
                                r.text.trim(),
                                highlight::Lang::SystemVerilog,
                                in_block,
                                theme,
                                13.0,
                            );
                            in_block = editor::block_state_after(&r.text, in_block);
                            let galley = ui.fonts(|f| f.layout_job(job));
                            let (cell_rect, _) =
                                ui.allocate_exact_size(galley.size(), egui::Sense::click());
                            if on {
                                ui.painter().rect_filled(cell_rect, 2.0, theme.sel);
                            }
                            let cell_resp = ui.put(
                                cell_rect,
                                egui::Label::new(galley).selectable(true).sense(egui::Sense::click()),
                            );
                            if cell_resp.clicked() {
                                pick_line = Some(r.line.to_string());
                            }
                            ui.end_row();
                        }
                    }
                });
        });
    if let Some(spec) = pick_bp {
        let _ = model.toggle_source_breakpoint(&spec);
    }
    if let Some(spec) = pick_line {
        let _ = model.select_source_line(&spec);
    }
}

/// Colored [`egui::text::LayoutJob`] for one editor line. `starts_in_block`
/// paints through the closing `*/` as a comment first, then tokenizes the
/// rest. All slicing is char-boundary-guarded: the tokenizer is
/// byte-oriented, so a defensive `str::get` keeps non-ASCII text panic-free.
fn highlight_line_job(
    text: &str,
    lang: highlight::Lang,
    starts_in_block: bool,
    theme: &theme::Theme,
) -> egui::text::LayoutJob {
    use egui::text::LayoutJob;
    let mut job = LayoutJob::default();
    let base = egui::TextFormat {
        font_id: theme::font_code(13.0),
        color: theme.text,
        ..Default::default()
    };
    let comment_fmt = egui::TextFormat {
        color: theme.faint,
        italics: true,
        ..base.clone()
    };
    // Block-comment prefix, if the line starts inside one.
    let rest = if starts_in_block {
        match text.find("*/") {
            Some(j) => {
                let end = j + 2;
                if let Some(prefix) = text.get(..end) {
                    job.append(prefix, 0.0, comment_fmt);
                }
                text.get(end..).unwrap_or("")
            }
            None => {
                job.append(text, 0.0, comment_fmt);
                ""
            }
        }
    } else {
        text
    };
    if rest.is_empty() {
        return job;
    }
    let spans = highlight::highlight(lang, rest, theme);
    let mut cursor = 0usize;
    for span in spans {
        let (range, color) = (span.range, span.color);
        if range.start < cursor || range.end > rest.len() {
            continue; // defensive: spans are ordered and non-overlapping
        }
        if range.start > cursor {
            if let Some(gap) = rest.get(cursor..range.start) {
                job.append(gap, 0.0, base.clone());
            }
            cursor = range.start;
        }
        match rest.get(range.clone()) {
            Some(tok) => {
                let mut fmt = egui::TextFormat {
                    color,
                    ..base.clone()
                };
                // Comment spans render dim italic per the editor spec.
                fmt.italics = span.italic;
                job.append(tok, 0.0, fmt);
                cursor = range.end;
            }
            None => break, // non-char-boundary span: tail append below covers it
        }
    }
    if let Some(tail) = rest.get(cursor..) {
        if !tail.is_empty() {
            job.append(tail, 0.0, base);
        }
    }
    job
}

/// Breadcrumb context for the editor: enclosing module/entity and innermost
/// block for a 1-based line. Heuristic upward scan with closed-scope depth
/// tracking (good enough for navigation; not a parser).
struct EditorCrumbs {
    module: Option<(String, usize)>,
    block: Option<(String, usize)>,
}

fn crumb_name(trimmed: &str, kw: &str) -> String {
    let rest = trimmed[kw.len().min(trimmed.len())..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(rest.len());
    let name: String = rest[..end].chars().take(40).collect();
    if name.is_empty() {
        kw.to_string()
    } else {
        name
    }
}

/// Block label: `label : always_ff @(…)` -> `label`; else the keyword itself.
fn block_label(trimmed: &str, kw: &str) -> String {
    if let Some(i) = trimmed.find(" : ") {
        let before = trimmed[..i].trim();
        let after: String = trimmed[i + 3..]
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        // `label : always_ff ...` → `label`.
        if !before.is_empty()
            && before
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        {
            return before.chars().take(32).collect();
        }
        // `always_comb begin : we_a_decoder` → `always_comb : we_a_decoder`.
        if !after.is_empty() {
            return format!("{kw} : {after}");
        }
        if !before.is_empty() {
            return before.chars().take(32).collect();
        }
    }
    kw.to_string()
}

fn kw_at(tl: &str, k: &str) -> bool {
    tl.starts_with(k)
        && tl[k.len()..]
            .chars()
            .next()
            .map_or(true, |c| !c.is_alphanumeric() && c != '_' && c != '$')
}

fn editor_crumbs(rows: &[SourceLine], cursor: usize, lang: highlight::Lang) -> EditorCrumbs {
    let mut out = EditorCrumbs {
        module: None,
        block: None,
    };
    if rows.is_empty() {
        return out;
    }
    let vhdl = lang == highlight::Lang::Vhdl;
    let idx = rows
        .iter()
        .position(|r| r.line == cursor)
        .unwrap_or(0)
        .min(rows.len() - 1);
    let mut closed = 0usize;
    for r in rows[..=idx].iter().rev() {
        let t = r.text.trim();
        if t.is_empty() {
            continue;
        }
        let tl = t.to_ascii_lowercase();
        // Closed scopes seen while scanning upward.
        let is_end = tl.starts_with("endmodule")
            || tl.starts_with("endfunction")
            || tl.starts_with("endtask")
            || tl.starts_with("endgenerate")
            || tl.starts_with("endclass")
            || tl.starts_with("endinterface")
            || tl.starts_with("endpackage")
            || tl.starts_with("endarchitecture")
            || tl == "end"
            || tl.starts_with("end ")
            || tl.starts_with("end;")
            || tl.starts_with("end:");
        if is_end {
            closed += 1;
            continue;
        }
        let mod_kw = if vhdl { "entity" } else { "module" };
        if kw_at(&tl, mod_kw) {
            if closed > 0 {
                closed -= 1;
            } else if out.module.is_none() {
                out.module = Some((crumb_name(t, mod_kw), r.line));
            }
            continue;
        }
        if out.block.is_none() {
            let found: Option<&'static str> = if vhdl {
                if tl.starts_with("process") {
                    Some("process")
                } else if kw_at(&tl, "architecture") {
                    Some("architecture")
                } else {
                    None
                }
            } else {
                // Longest-first so `always` never shadows `always_ff`.
                [
                    "always_ff",
                    "always_comb",
                    "always_latch",
                    "always @",
                    "always",
                    "initial",
                    "function",
                    "task",
                    "generate",
                ]
                .iter()
                .find(|k| {
                    kw_at(&tl, k) || tl.split(" : ").nth(1).map_or(false, |rest| kw_at(rest, k))
                })
                .copied()
            };
            if let Some(k) = found {
                if closed > 0 {
                    closed -= 1;
                } else {
                    out.block = Some((block_label(t, k), r.line));
                }
            }
        }
        if out.module.is_some() && out.block.is_some() {
            break;
        }
    }
    out
}

fn paint_memory(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("Memory");
    ui.horizontal(|ui| {
        if ui.button("Run 16").clicked() {
            surface::request_job(JobKind::SimRun(16));
        }
        if ui.button("Step").clicked() {
            let _ = model.sim_step();
        }
        if ui.button("Restart").clicked() {
            let _ = model.sim_restart();
        }
    });
    let blocks = model.memory_rows().to_vec();
    let selected = model.selected_memory.clone();
    let mut pick: Option<String> = None;
    let remain = ui.available_height().max(160.0);
    let list_h = (remain * 0.55).clamp(120.0, remain - 80.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Name").strong());
        ui.add_space(48.0);
        ui.label(RichText::new("Type").strong());
        ui.add_space(24.0);
        ui.label(RichText::new("Data").strong());
        ui.add_space(24.0);
        ui.label(RichText::new("W").strong());
        ui.label(RichText::new("D").strong());
    });
    let row_h = 22.0;
    egui::ScrollArea::vertical()
        .id_salt("ug900_memory")
        .max_height(list_h)
        .show_rows(ui, row_h, blocks.len().max(1), |ui, range| {
            if blocks.is_empty() {
                ui.label("No memories until you run simulation.");
                return;
            }
            let end = range.end.min(blocks.len());
            for (i, m) in blocks[range.start..end].iter().enumerate() {
                let i = range.start + i;
                let on = selected.as_deref() == Some(m.name.as_str());
                let shown = format!(
                    "{}  {}  {}  {}/{}",
                    helion_gui::schematic_short_name(&m.name),
                    m.type_cell(),
                    m.data_cell(),
                    m.width,
                    m.depth()
                );
                if paint_clipped_select(ui, on, &shown, &m.name, theme) {
                    pick = Some(i.to_string());
                }
            }
        });
    if let Some(spec) = pick {
        let _ = model.select_memory(&spec);
    }
    ui.separator();
    ui.label(RichText::new("Contents").strong());
    let words = model.memory_word_rows();
    let sel_addr = model.selected_memory_addr;
    let mut pick_addr: Option<usize> = None;
    if words.is_empty() {
        ui.label("Select a memory to view address and data.");
    } else {
        let rest = ui.available_height().max(80.0);
        egui::ScrollArea::vertical()
            .id_salt("ug900_memory_words")
            .max_height(rest)
            .show_rows(ui, row_h, words.len().max(1), |ui, range| {
                let end = range.end.min(words.len());
                for r in &words[range.start..end] {
                    let on = sel_addr == Some(r.addr);
                    let shown = format!("{:#x}  {}", r.addr, r.data);
                    if paint_clipped_select(ui, on, &shown, &r.data, theme) {
                        pick_addr = Some(r.addr);
                    }
                }
            });
    }
    if let Some(addr) = pick_addr {
        let _ = model.select_memory_word(&addr.to_string());
    }
}

fn paint_breakpoints(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("Breakpoints");
    ui.horizontal(|ui| {
        if ui.button("Add led == 1").clicked() {
            let _ = model.add_breakpoint("led 1");
        }
        if ui.button("Add led change").clicked() {
            let _ = model.add_breakpoint("led");
        }
        if ui.button("Run 16").clicked() {
            surface::request_job(JobKind::SimRun(16));
        }
        // Selection-gated: disabled with a reason when no breakpoint is selected,
        // instead of silently no-opping on the empty spec.
        let bp_sel = model.selected_breakpoint.is_some();
        let bp_tip = if bp_sel {
            ""
        } else {
            "Select a breakpoint below first"
        };
        if ui
            .add_enabled(bp_sel, egui::Button::new("Disable"))
            .on_hover_text(bp_tip)
            .clicked()
        {
            let _ = model.set_breakpoint_enabled("", false);
        }
        if ui
            .add_enabled(bp_sel, egui::Button::new("Enable"))
            .on_hover_text(bp_tip)
            .clicked()
        {
            let _ = model.set_breakpoint_enabled("", true);
        }
        if ui
            .add_enabled(bp_sel, egui::Button::new("Delete"))
            .on_hover_text(bp_tip)
            .clicked()
        {
            let _ = model.delete_breakpoint("");
        }
    });
    let rows = model.breakpoint_rows().to_vec();
    let selected = model.selected_breakpoint;
    let mut pick: Option<String> = None;
    if rows.is_empty() {
        if paint_remaining_cta(ui, "No breakpoints yet.", "Add led == 1", theme) {
            let _ = model.add_breakpoint("led 1");
        }
        return;
    }
    data_scroll("ug900_breakpoints").show(ui, |ui| {
        egui::Grid::new("ug900_breakpoints_table")
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Id").strong());
                ui.label(RichText::new("Enabled").strong());
                ui.label(RichText::new("Signal").strong());
                ui.label(RichText::new("Condition").strong());
                ui.label(RichText::new("Hits").strong());
                ui.end_row();
                {
                    for b in &rows {
                        let on = selected == Some(b.id);
                        let id = b.id.to_string();
                        if ui.selectable_label(on, &id).clicked() {
                            pick = Some(id.clone());
                        }
                        if ui.selectable_label(on, b.enabled_cell()).clicked() {
                            pick = Some(id.clone());
                        }
                        if ui.selectable_label(on, &b.signal).clicked() {
                            pick = Some(id.clone());
                        }
                        if ui.selectable_label(on, &b.condition).clicked() {
                            pick = Some(id.clone());
                        }
                        if ui.selectable_label(on, b.hits.to_string()).clicked() {
                            pick = Some(id.clone());
                        }
                        ui.end_row();
                    }
                }
            });
    });
    if let Some(spec) = pick {
        let _ = model.select_breakpoint(&spec);
    }
}

fn paint_forces(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("Force Constants");
    ui.horizontal(|ui| {
        if ui.button("Force led 1").clicked() {
            let _ = model.add_force("led 1");
        }
        if ui.button("Deposit led 1").clicked() {
            let _ = model.add_deposit("led 1");
        }
        if ui.button("Run").clicked() {
            let _ = model.exec("run_simulation");
        }
        let force_sel = model.selected_force.is_some();
        if ui
            .add_enabled(force_sel, egui::Button::new("Remove"))
            .on_hover_text(if force_sel {
                ""
            } else {
                "Select a force below first"
            })
            .clicked()
        {
            let _ = model.remove_force("");
        }
    });
    let rows = model.force_rows().to_vec();
    let selected = model.selected_force.clone();
    let mut pick: Option<String> = None;
    if rows.is_empty() {
        if paint_remaining_cta(ui, "No force constants yet.", "Force led 1", theme) {
            let _ = model.add_force("led 1");
        }
        return;
    }
    data_scroll("ug900_forces").show(ui, |ui| {
        egui::Grid::new("ug900_forces_table")
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Kind").strong());
                ui.label(RichText::new("Value").strong());
                ui.label(RichText::new("Radix").strong());
                ui.label(RichText::new("Start_ps").strong());
                ui.label(RichText::new("Cancel_ps").strong());
                ui.label(RichText::new("Status").strong());
                ui.end_row();
                {
                    for (i, r) in rows.iter().enumerate() {
                        let on = selected.as_deref() == Some(r.name.as_str());
                        let key = i.to_string();
                        if ui.selectable_label(on, &r.name).clicked() {
                            pick = Some(key.clone());
                        }
                        if ui.selectable_label(on, r.kind_cell()).clicked() {
                            pick = Some(key.clone());
                        }
                        if ui.selectable_label(on, r.value_cell()).clicked() {
                            pick = Some(key.clone());
                        }
                        if ui.selectable_label(on, r.radix_cell()).clicked() {
                            pick = Some(key.clone());
                        }
                        if ui.selectable_label(on, r.start_ps.to_string()).clicked() {
                            pick = Some(key.clone());
                        }
                        if ui.selectable_label(on, r.cancel_ps.to_string()).clicked() {
                            pick = Some(key.clone());
                        }
                        if ui.selectable_label(on, r.status_cell()).clicked() {
                            pick = Some(key.clone());
                        }
                        ui.end_row();
                    }
                }
            });
    });
    if let Some(spec) = pick {
        let _ = model.select_force(&spec);
    }
}

fn paint_locals(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Locals");
    ui.horizontal(|ui| {
        if ui.button("Run 16").clicked() {
            surface::request_job(JobKind::SimRun(16));
        }
        if ui.button("Step").clicked() {
            let _ = model.sim_step();
        }
        if ui.button("Restart").clicked() {
            let _ = model.sim_restart();
        }
    });
    let rows = model.local_rows().to_vec();
    let selected = model.selected_local.clone();
    let mut pick: Option<String> = None;
    data_scroll("ug900_locals_ws").show(ui, |ui| {
        egui::Grid::new("ug900_locals_ws_table")
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Type").strong());
                ui.label(RichText::new("Value").strong());
                ui.label(RichText::new("Scope").strong());
                ui.end_row();
                if rows.is_empty() {
                    ui.label("-");
                    ui.label("-");
                    ui.label("No locals until you run simulation.");
                    ui.end_row();
                } else {
                    for (i, l) in rows.iter().enumerate() {
                        let on = selected.as_deref() == Some(l.name.as_str());
                        if ui.selectable_label(on, &l.name).clicked() {
                            pick = Some(i.to_string());
                        }
                        if ui.selectable_label(on, l.type_cell()).clicked() {
                            pick = Some(i.to_string());
                        }
                        if ui.selectable_label(on, l.value_cell()).clicked() {
                            pick = Some(i.to_string());
                        }
                        if ui.selectable_label(on, &l.scope).clicked() {
                            pick = Some(i.to_string());
                        }
                        ui.end_row();
                    }
                }
            });
    });
    if let Some(spec) = pick {
        let _ = model.select_local(&spec);
    }
}

fn paint_wave(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.horizontal(|ui| {
        ui.heading("Waveform");
        let a_txt = match model.wave.cursor_a {
            Some(s) => format!("A t={s} ({} ps)", model.wave.time_ps(s)),
            None => "A=-".into(),
        };
        let b_txt = match model.wave.cursor_b {
            Some(s) => format!("B t={s} ({} ps)", model.wave.time_ps(s)),
            None => "B=-".into(),
        };
        let d_txt = match model.wave.time_delta_ps() {
            Some(d) => format!("Δt={d} ps"),
            None => "Δt=n/a".into(),
        };
        let ns = model.wave.timescale_ps as f64 / 1000.0;
        let period = if ns >= 1.0 && (ns - ns.round()).abs() < 1e-9 {
            format!("{} ns per cycle", ns.round() as i64)
        } else {
            format!("{} ps per cycle", model.wave.timescale_ps)
        };
        ui.label(
            RichText::new(format!(
                "Waveform {period}  cursor t={}  {a_txt}  {b_txt}  {d_txt}",
                model.wave.cursor,
            ))
            .weak(),
        );
        if sidebar_button(ui, "Cursor A")
            .on_hover_text("Place cursor A (Shift-click on wave)")
            .clicked()
        {
            let _ = model.set_wave_ab_cursor("A");
        }
        if sidebar_button(ui, "Cursor B")
            .on_hover_text("Place cursor B (Alt-click on wave)")
            .clicked()
        {
            let _ = model.set_wave_ab_cursor("B");
        }
        if sidebar_button(ui, "Add marker").clicked() {
            let n = model.wave.markers.len() + 1;
            let _ = model.add_wave_marker(&format!("M{n}"));
        }
        if sidebar_button(ui, "Virtual bus").clicked() {
            let _ = model.add_wave_virtual_bus("vb led cnt");
        }
        // GTKWave-style lane density.
        ui.label(RichText::new("Density").small().weak());
        let mut dens = model.wave.density;
        ui.radio_value(&mut dens, helion_gui::WaveDensity::Comfortable, "Comfortable")
            .on_hover_text("Roomy lanes for probing");
        ui.radio_value(&mut dens, helion_gui::WaveDensity::Compact, "Compact")
            .on_hover_text("Fit ~3x more signals in the pane");
        model.wave.density = dens;
    });
    ui.label(
        RichText::new("click cursor · Shift A · Alt B")
            .small()
            .weak(),
    );
    paint_wave_markers(ui, model);
    paint_wave_cursors(ui, model);
    paint_virtual_buses(ui, model);
    if model.wave.traces.is_empty() {
        ui.label("No waveform yet.");
        if primary_button(ui, "Run Simulation").clicked() {
            surface::request_job(JobKind::SimRun(model.sim_runtime_cycles.max(1)));
        }
        // Own the Wave pane - no thick empty black slab beside scopes.
        let fill = ui.available_size().max(egui::vec2(120.0, 160.0));
        let (rect, _) = ui.allocate_exact_size(fill, Sense::hover());
        ui.painter().rect_filled(rect, 4.0, theme.die_bg);
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.0_f32, theme.line),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Wave · run simulation to fill",
            egui::FontId::proportional(14.0),
            theme.dim,
        );
        return;
    }
    let n = model.wave.sample_len().max(1);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Name").strong().monospace());
        ui.add_space(80.0);
        ui.label(RichText::new("Value").strong().monospace());
        if model.wave.cursor_a.is_some() {
            ui.add_space(12.0);
            ui.label(RichText::new("A").strong().monospace().color(theme.danger));
        }
        if model.wave.cursor_b.is_some() {
            ui.add_space(12.0);
            ui.label(RichText::new("B").strong().monospace().color(theme.trace_b));
        }
        ui.add_space(40.0);
        ui.label(RichText::new("Waveform").strong().monospace());
    });
    // Timescale ruler
    let ruler_h = 18.0;
    let (ruler, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ruler_h), Sense::click());
    if ui.is_rect_visible(ruler) {
        let p = ui.painter();
        p.rect_filled(ruler, 0.0, theme.die_bg);
        let wave_x0 = ruler.left() + 220.0;
        let wave_w = (ruler.right() - wave_x0).max(8.0);
        for i in 0..=n.min(32) {
            let x = wave_x0 + wave_w * (i as f32) / (n as f32);
            p.line_segment(
                [
                    egui::pos2(x, ruler.top() + 10.0),
                    egui::pos2(x, ruler.bottom()),
                ],
                Stroke::new(1.0_f32, theme.faint),
            );
            if i % 2 == 0 {
                p.text(
                    egui::pos2(x + 2.0, ruler.top()),
                    egui::Align2::LEFT_TOP,
                    format!("{}", model.wave.time_ps(i) / 1000),
                    egui::FontId::monospace(10.0),
                    theme.dim,
                );
            }
        }
        for m in &model.wave.markers {
            let x = wave_x0 + wave_w * (m.sample as f32 + 0.5) / (n as f32);
            p.line_segment(
                [egui::pos2(x, ruler.top()), egui::pos2(x, ruler.bottom())],
                Stroke::new(1.2_f32, theme.violet),
            );
            p.text(
                egui::pos2(x + 2.0, ruler.top()),
                egui::Align2::LEFT_TOP,
                &m.name,
                egui::FontId::monospace(9.0),
                theme.violet,
            );
        }
        if let (Some(a), Some(b)) = (model.wave.cursor_a, model.wave.cursor_b) {
            let xa = wave_x0 + wave_w * (a as f32 + 0.5) / (n as f32);
            let xb = wave_x0 + wave_w * (b as f32 + 0.5) / (n as f32);
            let (l, r) = if xa <= xb { (xa, xb) } else { (xb, xa) };
            p.rect_filled(
                egui::Rect::from_min_max(egui::pos2(l, ruler.top()), egui::pos2(r, ruler.bottom())),
                0.0,
                theme.trace_b.gamma_multiply(0.16),
            );
        }
        if let Some(a) = model.wave.cursor_a {
            let x = wave_x0 + wave_w * (a as f32 + 0.5) / (n as f32);
            p.line_segment(
                [egui::pos2(x, ruler.top()), egui::pos2(x, ruler.bottom())],
                Stroke::new(1.4_f32, theme.danger),
            );
            p.text(
                egui::pos2(x + 2.0, ruler.top()),
                egui::Align2::LEFT_TOP,
                "A",
                egui::FontId::monospace(9.0),
                theme.danger,
            );
        }
        if let Some(b) = model.wave.cursor_b {
            let x = wave_x0 + wave_w * (b as f32 + 0.5) / (n as f32);
            p.line_segment(
                [egui::pos2(x, ruler.top()), egui::pos2(x, ruler.bottom())],
                Stroke::new(1.4_f32, theme.trace_b),
            );
            p.text(
                egui::pos2(x + 2.0, ruler.top()),
                egui::Align2::LEFT_TOP,
                "B",
                egui::FontId::monospace(9.0),
                theme.trace_b,
            );
        }
    }

    let mut style_cmd: Option<(String, WaveStyle)> = None;
    let mut radix_cmd: Option<(String, WaveRadix)> = None;
    let mut new_cursor: Option<usize> = None;
    let mut place_a: Option<usize> = None;
    let mut place_b: Option<usize> = None;
    let cursor = model.wave.cursor;
    let cursor_a = model.wave.cursor_a;
    let cursor_b = model.wave.cursor_b;
    let ts = model.wave.timescale_ps;
    let row_h = chrome::wave_trace_row_h(model.wave.density);

    for t in &model.wave.traces {
        ui.horizontal(|ui| {
            ui.set_min_height(row_h);
            ui.set_max_height(row_h);
            ui.add_sized(
                [110.0, 28.0],
                egui::Label::new(RichText::new(&t.name).monospace().strong()),
            );
            ui.add_sized(
                [72.0, 28.0],
                egui::Label::new(
                    RichText::new(t.value_at(cursor))
                        .monospace()
                        .color(theme.ok),
                ),
            );
            if let Some(a) = cursor_a {
                ui.add_sized(
                    [56.0, 28.0],
                    egui::Label::new(RichText::new(t.value_at(a)).monospace().color(theme.danger)),
                );
            }
            if let Some(b) = cursor_b {
                ui.add_sized(
                    [56.0, 28.0],
                    egui::Label::new(
                        RichText::new(t.value_at(b))
                            .monospace()
                            .color(theme.trace_b),
                    ),
                );
            }
            if ui
                .add_sized(
                    [64.0, chrome::HIT_SIDEBAR],
                    egui::Button::new(if t.style == WaveStyle::Analog {
                        "Analog"
                    } else {
                        "Digital"
                    }),
                )
                .clicked()
            {
                style_cmd = Some((
                    t.name.clone(),
                    if t.style == WaveStyle::Analog {
                        WaveStyle::Digital
                    } else {
                        WaveStyle::Analog
                    },
                ));
            }
            if ui
                .add_sized(
                    [48.0, chrome::HIT_SIDEBAR],
                    egui::Button::new(if t.radix == WaveRadix::Hexadecimal {
                        "Hex"
                    } else {
                        "Bin"
                    }),
                )
                .clicked()
            {
                radix_cmd = Some((
                    t.name.clone(),
                    if t.radix == WaveRadix::Hexadecimal {
                        WaveRadix::Binary
                    } else {
                        WaveRadix::Hexadecimal
                    },
                ));
            }
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), (row_h - 4.0).max(28.0)),
                Sense::click(),
            );
            if ui.is_rect_visible(rect) {
                paint_trace_shape(
                    ui,
                    rect,
                    t,
                    cursor,
                    cursor_a,
                    cursor_b,
                    n,
                    ts,
                    &model.wave.markers,
                    theme,
                );
            }
            if resp.clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let x = ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
                    let sample = ((x * n as f32) as usize).min(n.saturating_sub(1));
                    let mods = ui.input(|i| i.modifiers);
                    if mods.shift {
                        place_a = Some(sample);
                    } else if mods.alt {
                        place_b = Some(sample);
                    } else {
                        new_cursor = Some(sample);
                    }
                }
            }
            // Right-click menu (GTKWave pattern): per-trace radix and style.
            let t_name = t.name.clone();
            let t_style = t.style;
            let t_radix = t.radix;
            resp.context_menu(|ui| {
                let radix_label = if t_radix == WaveRadix::Hexadecimal {
                    "Set radix: Binary"
                } else {
                    "Set radix: Hex"
                };
                if ui.button(radix_label).clicked() {
                    radix_cmd = Some((
                        t_name.clone(),
                        if t_radix == WaveRadix::Hexadecimal {
                            WaveRadix::Binary
                        } else {
                            WaveRadix::Hexadecimal
                        },
                    ));
                    ui.close();
                }
                let style_label = if t_style == WaveStyle::Analog {
                    "Show as Digital"
                } else {
                    "Show as Analog"
                };
                if ui.button(style_label).clicked() {
                    style_cmd = Some((
                        t_name.clone(),
                        if t_style == WaveStyle::Analog {
                            WaveStyle::Digital
                        } else {
                            WaveStyle::Analog
                        },
                    ));
                    ui.close();
                }
            });
        });
    }
    if let Some((name, st)) = style_cmd {
        let _ = model.set_wave_style(&format!(
            "{name} {}",
            if st == WaveStyle::Analog {
                "analog"
            } else {
                "digital"
            }
        ));
    }
    if let Some((name, r)) = radix_cmd {
        let _ = model.set_wave_radix(&format!(
            "{name} {}",
            if r == WaveRadix::Hexadecimal {
                "hex"
            } else {
                "binary"
            }
        ));
    }
    if let Some(c) = new_cursor {
        model.wave.set_cursor(c);
    }
    if let Some(a) = place_a {
        let _ = model.set_wave_ab_cursor(&format!("A {a}"));
    }
    if let Some(b) = place_b {
        let _ = model.set_wave_ab_cursor(&format!("B {b}"));
    }
}

/// Simulation-based ILA (logic analyzer): probe setup, trigger, capture, viewer.
fn paint_ila(
    ui: &mut egui::Ui,
    model: &mut IdeModel,
    ila_rx: &mut Option<std::sync::mpsc::Receiver<Result<ila::IlaCapture, String>>>,
    toasts: &mut toasts::ToastQueue,
    theme: &theme::Theme,
) {
    enum Intent {
        AddProbe(String),
        RemoveProbe(usize),
        SetRadix(usize, ila::IlaRadix),
        GroupBus,
        SetTrigKind(ila::IlaTriggerKind),
        SetTrigProbe(usize),
        Arm,
        Disarm,
        Save,
        Load,
        ExportVcd,
        ClearError,
    }
    let mut intents: Vec<Intent> = Vec::new();
    let has_design = model.shell.session.design.is_some();

    // Header: title, status, progress, arm + session buttons.
    ui.horizontal(|ui| {
        ui.heading("Logic Analyzer");
        // Human-readable status chips instead of the debug dump that
        // `status_text()` produces (kept as-is for the `ila_status` Tcl cmd).
        {
            let ila = &model.sim_ila;
            let trig_name = ila
                .config
                .probes
                .get(ila.config.trigger.probe)
                .map(|p| p.name.as_str())
                .unwrap_or("-");
            ui.horizontal(|ui| {
                ui.label(RichText::new("Status").color(theme.faint).small());
                ui.label(RichText::new(format!("{:?}", ila.status)).strong());
                ui.label(RichText::new("Probes").color(theme.faint).small());
                ui.label(
                    RichText::new(ila.config.probes.len().to_string()).strong(),
                );
                ui.label(RichText::new("Trigger").color(theme.faint).small());
                ui.label(
                    RichText::new(format!(
                        "{} @ {}",
                        ila.config.trigger.kind.label(),
                        trig_name
                    ))
                    .strong(),
                );
                ui.label(RichText::new("Depth").color(theme.faint).small());
                ui.label(
                    RichText::new(ila.config.depth.to_string()).strong(),
                );
            });
        }
        if model.sim_ila.status == SimIlaStatus::Capturing {
            if let Some(p) = model.sim_ila.progress.as_ref() {
                let c = p.cycles.load(std::sync::atomic::Ordering::Relaxed);
                ui.label(RichText::new(format!("capturing… {c} cycles")).color(theme.info));
            }
            ui.ctx().request_repaint();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if sidebar_button(ui, "Export VCD")
                .on_hover_text("Save the current capture as a VCD file (GTKWave)")
                .clicked()
            {
                intents.push(Intent::ExportVcd);
            }
            if sidebar_button(ui, "Load session").clicked() {
                intents.push(Intent::Load);
            }
            if sidebar_button(ui, "Save session").clicked() {
                intents.push(Intent::Save);
            }
            let armed = model.sim_ila.status == SimIlaStatus::Capturing;
            if armed {
                if sidebar_button(ui, "Stop")
                    .on_hover_text("Stop the capture in flight (keeps partial samples)")
                    .clicked()
                {
                    intents.push(Intent::Disarm);
                }
            }
            ui.add_enabled_ui(!armed && has_design, |ui| {
                if primary_button(ui, if armed { "Capturing…" } else { "Arm Capture" })
                    .on_hover_text("Run the event kernel with probes instrumented (helion-sim)")
                    .clicked()
                {
                    intents.push(Intent::Arm);
                }
            });
        });
    });
    if let Some(e) = model.sim_ila.last_error.clone() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("error: {e}")).color(theme.danger));
            if ui.small_button("clear").clicked() {
                intents.push(Intent::ClearError);
            }
        });
    }
    if !has_design {
        ui.label(
            RichText::new("Open HDL and synthesize to use the Logic Analyzer.")
                .weak(),
        );
    }

    // Setup: probes (left) + trigger (right).
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_min_width(340.0);
            ui.set_max_width(340.0);
            ui.label(RichText::new("Probes").strong());
            ui.horizontal(|ui| {
                ui.label("Search");
                ui.text_edit_singleline(&mut model.sim_ila.search);
            });
            let nets: Vec<String> = model
                .shell
                .session
                .design
                .as_ref()
                .map(|d| model.sim_ila.probe_net_list(d))
                .unwrap_or_default();
            egui::ScrollArea::vertical()
                .id_salt("ila_net_list")
                .max_height(180.0)
                .show(ui, |ui| {
                    for net in nets.iter().take(500) {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(net).monospace());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("+").on_hover_text("Add as probe").clicked() {
                                        intents.push(Intent::AddProbe(net.clone()));
                                    }
                                },
                            );
                        });
                    }
                    if nets.len() > 500 {
                        ui.label(
                            RichText::new(format!("…{} more (refine search)", nets.len() - 500))
                                .weak()
                                .small(),
                        );
                    }
                });
            ui.separator();
            let rows: Vec<(String, usize, ila::IlaRadix)> = model
                .sim_ila
                .config
                .probes
                .iter()
                .map(|p| (p.name.clone(), p.width(), p.radix))
                .collect();
            if model.sim_ila.group.len() != rows.len() {
                model.sim_ila.group = vec![false; rows.len()];
            }
            ui.label(RichText::new(format!("Probe list ({})", rows.len())).strong());
            egui::ScrollArea::vertical()
                .id_salt("ila_probe_list")
                .max_height(150.0)
                .show(ui, |ui| {
                    for (i, (name, width, radix)) in rows.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let mut g = model.sim_ila.group[i];
                            if ui.checkbox(&mut g, "").on_hover_text("Check to group into a bus").changed() {
                                model.sim_ila.group[i] = g;
                            }
                            ui.label(RichText::new(name).monospace().strong());
                            ui.label(
                                RichText::new(format!("[{width}]")).weak().monospace().small(),
                            );
                            egui::ComboBox::from_id_salt(format!("ila_radix_{i}"))
                                .selected_text(radix.label())
                                .width(56.0)
                                .show_ui(ui, |ui| {
                                    for r in [ila::IlaRadix::Bin, ila::IlaRadix::Hex, ila::IlaRadix::Dec] {
                                        if ui.selectable_label(*radix == r, r.label()).clicked() {
                                            intents.push(Intent::SetRadix(i, r));
                                        }
                                    }
                                });
                            if ui.small_button("✕").clicked() {
                                intents.push(Intent::RemoveProbe(i));
                            }
                        });
                    }
                    if rows.is_empty() {
                        ui.label(RichText::new("No probes - add nets above.").weak().small());
                    }
                });
            ui.horizontal(|ui| {
                if sidebar_button(ui, "Group checked as bus").clicked() {
                    intents.push(Intent::GroupBus);
                }
                ui.text_edit_singleline(&mut model.sim_ila.bus_name)
                    .on_hover_text("Bus name (empty = first probe's name)");
            });
        });
        ui.separator();
        ui.vertical(|ui| {
            ui.label(RichText::new("Trigger & Capture").strong());
            // Trigger probe picker.
            let names: Vec<String> = model
                .sim_ila
                .config
                .probes
                .iter()
                .map(|p| p.name.clone())
                .collect();
            let tp = model.sim_ila.config.trigger.probe.min(names.len().saturating_sub(1));
            ui.horizontal(|ui| {
                ui.label("Probe");
                egui::ComboBox::from_id_salt("ila_trig_probe")
                    .selected_text(names.get(tp).cloned().unwrap_or_else(|| "-".into()))
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for (i, n) in names.iter().enumerate() {
                            if ui.selectable_label(tp == i, n).clicked() {
                                intents.push(Intent::SetTrigProbe(i));
                            }
                        }
                    });
                ui.label("Kind");
                let kind = model.sim_ila.config.trigger.kind;
                egui::ComboBox::from_id_salt("ila_trig_kind")
                    .selected_text(kind.label())
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        for k in [
                            ila::IlaTriggerKind::Immediate,
                            ila::IlaTriggerKind::Rising,
                            ila::IlaTriggerKind::Falling,
                            ila::IlaTriggerKind::ValueMatch { value: 0, mask: u64::MAX },
                        ] {
                            if ui.selectable_label(kind.label() == k.label(), k.label()).clicked() {
                                intents.push(Intent::SetTrigKind(k));
                            }
                        }
                    });
            });
            if matches!(
                model.sim_ila.config.trigger.kind,
                ila::IlaTriggerKind::ValueMatch { .. }
            ) {
                ui.horizontal(|ui| {
                    ui.label("Value");
                    ui.text_edit_singleline(&mut model.sim_ila.trig_value_text);
                    ui.label("Mask");
                    ui.text_edit_singleline(&mut model.sim_ila.trig_mask_text);
                });
            }
            ui.horizontal(|ui| {
                ui.label("Depth");
                ui.add(
                    egui::DragValue::new(&mut model.sim_ila.config.depth)
                        .range(1.0..=ila::MAX_DEPTH as f64),
                );
                ui.label("Pre-trigger");
                ui.add(
                    egui::Slider::new(&mut model.sim_ila.config.pre_ratio, 0.0..=1.0)
                        .show_value(true),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Clock");
                let clocks: Vec<String> = model
                    .shell
                    .session
                    .design
                    .as_ref()
                    .map(|d| model.sim_ila.clock_net_list(d))
                    .unwrap_or_default();
                let cur = model.sim_ila.config.clock_net.clone();
                egui::ComboBox::from_id_salt("ila_clock")
                    .selected_text(&cur)
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        for c in &clocks {
                            if ui.selectable_label(&cur == c, c).clicked() {
                                model.sim_ila.config.clock_net = c.clone();
                            }
                        }
                    });
                ui.label("Max cycles");
                ui.add(
                    egui::DragValue::new(&mut model.sim_ila.config.max_cycles)
                        .range(1.0..=ila::MAX_CYCLES as f64),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Timescale ps");
                ui.add(
                    egui::DragValue::new(&mut model.sim_ila.config.timescale_ps)
                        .range(1.0..=1_000_000_000.0),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Session");
                ui.text_edit_singleline(&mut model.sim_ila.session_path)
                    .on_hover_text("Session JSON path (empty = project dir / cwd)");
            });
            ui.label(
                RichText::new("helion-sim is single-clock: probes sample once per posedge of the design clock.")
                    .weak()
                    .small(),
            );
        });
    });

    // Apply intents (design borrows end before the &mut model.sim_ila calls).
    for intent in intents {
        match intent {
            Intent::AddProbe(net) => {
                if let Some(d) = model.shell.session.design.as_ref() {
                    if let Err(e) = model.sim_ila.add_probe(d, &net) {
                        model.sim_ila.last_error = Some(e);
                    } else {
                        model.sim_ila.last_error = None;
                    }
                }
            }
            Intent::RemoveProbe(i) => {
                if let Err(e) = model.sim_ila.remove_probe(i) {
                    model.sim_ila.last_error = Some(e);
                }
            }
            Intent::SetRadix(i, r) => {
                if let Some(p) = model.sim_ila.config.probes.get_mut(i) {
                    p.radix = r;
                }
            }
            Intent::GroupBus => {
                let name = model.sim_ila.bus_name.clone();
                if let Some(d) = model.shell.session.design.as_ref() {
                    match model.sim_ila.group_checked_as_bus(d, &name) {
                        Ok(_) => {
                            model.sim_ila.bus_name.clear();
                            model.sim_ila.last_error = None;
                        }
                        Err(e) => model.sim_ila.last_error = Some(e),
                    }
                }
            }
            Intent::SetTrigKind(k) => {
                if let Err(e) = model.sim_ila.set_trigger_kind(k) {
                    model.sim_ila.last_error = Some(e);
                }
            }
            Intent::SetTrigProbe(i) => {
                if let Err(e) = model.sim_ila.set_trigger_probe(i) {
                    model.sim_ila.last_error = Some(e);
                }
            }
            Intent::Arm => {
                // Design moves to the capture thread; never on the egui frame.
                if let Some(d) = model.shell.session.design.clone() {
                    match model.sim_ila.start_capture(&d) {
                        Ok(rx) => {
                            *ila_rx = Some(rx);
                        }
                        Err(e) => {
                            model.sim_ila.last_error = Some(e);
                            model.sim_ila.status = SimIlaStatus::Error;
                        }
                    }
                }
            }
            Intent::Disarm => {
                if let Err(e) = model.sim_ila.disarm() {
                    model.sim_ila.last_error = Some(e);
                }
            }
            Intent::Save => {
                let pp = model.last_project_path.clone();
                let r = model.sim_ila.save_session(pp.as_deref(), "");
                match r {
                    Ok(msg) => {
                        model.sim_ila.last_error = None;
                        toasts.push(toasts::ToastKind::Ok, msg);
                    }
                    Err(e) => model.sim_ila.last_error = Some(e),
                }
            }
            Intent::Load => {
                let pp = model.last_project_path.clone();
                let d = model.shell.session.design.clone();
                let r = model
                    .sim_ila
                    .load_session(d.as_ref(), pp.as_deref(), "");
                match r {
                    Ok(msg) => {
                        model.sim_ila.last_error = None;
                        toasts.push(toasts::ToastKind::Ok, msg);
                    }
                    Err(e) => model.sim_ila.last_error = Some(e),
                }
            }
            Intent::ClearError => model.sim_ila.last_error = None,
            Intent::ExportVcd => {
                // Default next to the session path (or project dir / cwd).
                let pp = model.last_project_path.clone();
                let base = if !model.sim_ila.session_path.trim().is_empty() {
                    std::path::PathBuf::from(model.sim_ila.session_path.trim())
                } else if let Some(p) = pp.as_deref().and_then(|p| p.parent()) {
                    p.join("ila_capture.json")
                } else {
                    std::path::PathBuf::from("ila_capture.json")
                };
                let path = base.with_extension("vcd");
                match model.sim_ila.capture.clone() {
                    Some(cap) => match ila::export_vcd(&path, &cap) {
                        Ok(()) => toasts.push(
                            toasts::ToastKind::Ok,
                            format!("ILA VCD saved {}", path.display()),
                        ),
                        Err(e) => model.sim_ila.last_error = Some(e),
                    },
                    None => {
                        model.sim_ila.last_error = Some("ila: no capture to export".into());
                    }
                }
            }
        }
    }

    ui.separator();
    if let Some(msg) = paint_ila_wave(ui, model, theme) {
        toasts.push(toasts::ToastKind::Ok, msg);
    }
}

fn ila_fmt_time(sample: u64, ts_ps: u64) -> String {
    let ps = sample.saturating_mul(ts_ps.max(1));
    if ps % 1000 == 0 {
        format!("{} ns", ps / 1000)
    } else {
        format!("{ps} ps")
    }
}

/// GTKWave-grade viewer: signal pane (name + value at cursor), time ruler,
/// zoom (wheel) + pan (drag), click cursor, Shift-A / Alt-B, trigger marker.
/// Returns a toast message when the trigger was set from the waveform.
fn paint_ila_wave(
    ui: &mut egui::Ui,
    model: &mut IdeModel,
    theme: &theme::Theme,
) -> Option<String> {
    let n = model.sim_ila.capture.as_ref().map(|c| c.len()).unwrap_or(0);
    if n == 0 {
        ui.label(
            RichText::new("No capture yet - add probes, set the trigger, press Arm Capture.")
                .weak(),
        );
        return None;
    }
    let nprobes = model
        .sim_ila
        .capture
        .as_ref()
        .map(|c| c.probes.len())
        .unwrap_or(0);
    if nprobes == 0 {
        return None;
    }
    let ts = model
        .sim_ila
        .capture
        .as_ref()
        .map(|c| c.timescale_ps)
        .unwrap_or(10_000);

    // Toolbar: zoom, fit, From/To, cursor readouts, trigger-from-cursor.
    let mut trig_msg: Option<String> = None;
    ui.horizontal(|ui| {
        if sidebar_button(ui, "−").on_hover_text("Zoom in").clicked() {
            model.sim_ila.viewer.zoom_center(0.5, n);
        }
        if sidebar_button(ui, "+").on_hover_text("Zoom out").clicked() {
            model.sim_ila.viewer.zoom_center(2.0, n);
        }
        if sidebar_button(ui, "Fit").on_hover_text("Fit capture").clicked() {
            let w = model.sim_ila.viewer.view_w;
            let (c, ca, cb) = (
                model.sim_ila.viewer.cursor,
                model.sim_ila.viewer.cursor_a,
                model.sim_ila.viewer.cursor_b,
            );
            model.sim_ila.viewer = ila::IlaViewer::fit(n, w);
            model.sim_ila.viewer.cursor = c;
            model.sim_ila.viewer.cursor_a = ca;
            model.sim_ila.viewer.cursor_b = cb;
        }
        let mut from = model.sim_ila.viewer.first.max(0.0) as usize;
        ui.label("From");
        if ui
            .add(egui::DragValue::new(&mut from).range(0.0..=n.saturating_sub(1) as f64))
            .changed()
        {
            model.sim_ila.viewer.first = from as f64;
            model.sim_ila.viewer.clamp(n);
        }
        let to = (model.sim_ila.viewer.first
            + model.sim_ila.viewer.view_w as f64 * model.sim_ila.viewer.spp)
            as usize;
        ui.label(format!("To {}", to.min(n.saturating_sub(1))));
        ui.separator();
        let cur_txt = match model.sim_ila.viewer.cursor {
            Some(c) => format!("t={} ({})", c, ila_fmt_time(c as u64, ts)),
            None => "t=-".into(),
        };
        ui.label(RichText::new(cur_txt).monospace().small());
        let (a_txt, b_txt, d_txt) = match model.sim_ila.viewer.delta_samples() {
            Some((a, b, d)) => (
                format!("A={}", ila_fmt_time(a as u64, ts)),
                format!("B={}", ila_fmt_time(b as u64, ts)),
                format!("Δt={d} ({} )", ila_fmt_time(d as u64, ts)),
            ),
            _ => {
                let a = match model.sim_ila.viewer.cursor_a {
                    Some(a) => format!("A={}", ila_fmt_time(a as u64, ts)),
                    None => "A=-".into(),
                };
                let b = match model.sim_ila.viewer.cursor_b {
                    Some(b) => format!("B={}", ila_fmt_time(b as u64, ts)),
                    None => "B=-".into(),
                };
                (a, b, "Δt=n/a".into())
            }
        };
        ui.label(RichText::new(a_txt).monospace().small().color(theme.danger));
        ui.label(RichText::new(b_txt).monospace().small().color(theme.trace_b));
        ui.label(RichText::new(d_txt).monospace().small());
        if let Some(ti) = model.sim_ila.capture.as_ref().and_then(|c| c.trigger_sample) {
            ui.label(
                RichText::new(format!("TRIG @{} ({})", ti, ila_fmt_time(ti as u64, ts)))
                    .monospace()
                    .small()
                    .color(theme.warn),
            );
        } else if model.sim_ila.capture.as_ref().is_some_and(|c| !c.found_trigger) {
            ui.label(
                RichText::new("trigger not found in window")
                    .small()
                    .color(theme.warn),
            );
        }
        ui.separator();
        // Click-an-edge trigger: set the trigger from the cursor sample.
        if sidebar_button(ui, "⚡ Trigger at cursor")
            .on_hover_text("Set the trigger from the edge/value under the cursor")
            .clicked()
        {
            if let Some(c) = model.sim_ila.viewer.cursor {
                match model.sim_ila.suggest_trigger_at_cursor(c) {
                    Ok(msg) => trig_msg = Some(msg),
                    Err(e) => model.sim_ila.last_error = Some(e),
                }
            } else {
                model.sim_ila.last_error = Some("ila: place the cursor first".into());
            }
        }
    });
    ui.label(
        RichText::new("wheel zoom · drag pan · click cursor · Shift-click A · Alt-click B · Ctrl-click edge = trigger")
            .small()
            .weak(),
    );

    let avail_h = ui.available_height().max(120.0);
    let row_h = (avail_h / nprobes as f32).clamp(22.0, 200.0);
    let ruler_h = 20.0;
    let pane_w = 300.0f32;

    ui.horizontal(|ui| {
        // Signal pane: GTKWave two columns - name + value at cursor.
        ui.vertical(|ui| {
            ui.set_min_width(pane_w);
            ui.set_max_width(pane_w);
            ui.horizontal(|ui| {
                ui.set_min_height(ruler_h);
                ui.label(RichText::new("Name").strong().monospace().small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("Value").strong().monospace().small());
                });
            });
            let cursor = model.sim_ila.viewer.cursor.unwrap_or(0).min(n - 1);
            for i in 0..nprobes {
                let (name, val) = {
                    let cap = model.sim_ila.capture.as_ref().expect("ila capture");
                    (cap.probes[i].name.clone(), cap.formatted_at(i, cursor))
                };
                ui.horizontal(|ui| {
                    ui.set_min_height(row_h);
                    ui.set_max_height(row_h);
                    ui.label(RichText::new(&name).monospace());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(val).monospace().color(theme.dim));
                    });
                });
            }
        });
        // Trace canvas.
        let canvas_w = (ui.available_width() - 8.0).max(80.0);
        let canvas_h = ruler_h + row_h * nprobes as f32;
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(canvas_w, canvas_h),
            egui::Sense::click_and_drag(),
        );
        model.sim_ila.viewer.view_w = rect.width();
        // Interactions.
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll.abs() > 0.5 {
            if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                let anchor = model.sim_ila.viewer.sample_at(pos.x, rect.left());
                model
                    .sim_ila
                    .viewer
                    .zoom(2f64.powf(-scroll as f64 / 240.0), anchor, n);
            }
        }
        if resp.dragged() {
            let dx = resp.drag_delta().x as f64 * model.sim_ila.viewer.spp;
            model.sim_ila.viewer.pan_samples(-dx, n);
        }
        if resp.clicked() || resp.double_clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let s = model.sim_ila.viewer.sample_at(pos.x, rect.left()).round() as usize;
                let s = s.min(n - 1);
                let mods = ui.input(|i| i.modifiers);
                if mods.ctrl || mods.command || resp.double_clicked() {
                    // Click-an-edge: set the trigger from the row's probe.
                    let row =
                        ((pos.y - rect.top() - ruler_h) / row_h).floor() as usize;
                    model.sim_ila.viewer.cursor = Some(s);
                    if row < nprobes {
                        match model.sim_ila.set_trigger_from_wave(row, s) {
                            Ok(msg) => trig_msg = Some(msg),
                            Err(e) => model.sim_ila.last_error = Some(e),
                        }
                    }
                } else if mods.shift {
                    model.sim_ila.viewer.cursor_a = Some(s);
                } else if mods.alt {
                    model.sim_ila.viewer.cursor_b = Some(s);
                } else {
                    model.sim_ila.viewer.cursor = Some(s);
                }
            }
        }
        if ui.is_rect_visible(rect) {
            paint_ila_traces(ui, rect, model, n, nprobes, row_h, ruler_h, ts, theme);
        }
    });
    trig_msg
}

#[allow(clippy::too_many_arguments)]
fn paint_ila_traces(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    model: &IdeModel,
    n: usize,
    nprobes: usize,
    row_h: f32,
    ruler_h: f32,
    ts: u64,
    theme: &theme::Theme,
) {
    let p = ui.painter();
    let cap = model.sim_ila.capture.as_ref().expect("ila capture");
    let v = &model.sim_ila.viewer;
    let wave_x0 = rect.left();
    let wave_w = rect.width().max(1.0);
    let top = rect.top();

    // Canvas background.
    p.rect_filled(rect, 4.0, theme.die_bg);

    // Ruler with 1/2/5 ticks.
    let target_px = 80.0f64;
    let mut step = 10f64.powf((v.spp * target_px).log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if step * m / v.spp >= target_px {
            step *= m;
            break;
        }
    }
    let first_tick = (v.first / step).ceil() as usize * step as usize;
    let mut s = first_tick;
    while (s as f64) < v.first + v.spp * wave_w as f64 {
        let x = v.x_of(s as f64, wave_x0);
        p.line_segment(
            [egui::pos2(x, top + ruler_h - 6.0), egui::pos2(x, top + ruler_h)],
            Stroke::new(1.0, theme.faint),
        );
        p.text(
            egui::pos2(x + 2.0, top + 2.0),
            egui::Align2::LEFT_TOP,
            ila_fmt_time(s as u64, ts),
            egui::FontId::monospace(9.0),
            theme.dim,
        );
        s += step as usize;
        if s > n + step as usize {
            break;
        }
    }
    p.line_segment(
        [egui::pos2(rect.left(), top + ruler_h), egui::pos2(rect.right(), top + ruler_h)],
        Stroke::new(1.0, theme.line),
    );

    let vis_first = v.first.max(0.0) as usize;
    let vis_last = (v.first + v.spp * wave_w as f64).ceil() as usize + 1;
    let vis_last = vis_last.min(n.saturating_sub(1)).max(vis_first);

    for i in 0..nprobes {
        let row_top = top + ruler_h + i as f32 * row_h;
        let row = egui::Rect::from_min_size(
            egui::pos2(rect.left(), row_top),
            egui::vec2(rect.width(), row_h),
        );
        if i % 2 == 1 {
            p.rect_filled(row, 0.0, theme.panel.gamma_multiply(0.35));
        }
        let probe = &cap.probes[i];
        let mid = row_top + row_h * 0.5;
        let hi = row_top + row_h * 0.18;
        let lo = row_top + row_h * 0.82;
        if probe.width() == 1 {
            // Digital trace.
            let mut prev_x = v.x_of(vis_first as f64, wave_x0);
            let mut prev_v = cap.value_at(i, vis_first).unwrap_or(0);
            let y_of = |b: u64| if b == 1 { hi } else { lo };
            for s in (vis_first + 1)..=vis_last {
                let x = v.x_of(s as f64, wave_x0);
                let cur = cap.value_at(i, s).unwrap_or(0);
                let valid = cap.valid.get(i).and_then(|vv| vv.get(s)).copied().unwrap_or(false);
                if !valid {
                    // X: red middle band.
                    p.line_segment(
                        [egui::pos2(prev_x, mid), egui::pos2(x, mid)],
                        Stroke::new(1.6, theme.danger),
                    );
                } else {
                    if cur != prev_v {
                        p.line_segment(
                            [egui::pos2(x, y_of(prev_v)), egui::pos2(x, y_of(cur))],
                            Stroke::new(1.6, theme.ok),
                        );
                    }
                    p.line_segment(
                        [egui::pos2(prev_x, y_of(prev_v)), egui::pos2(x, y_of(cur))],
                        Stroke::new(1.6, theme.ok),
                    );
                }
                prev_x = x;
                prev_v = cur;
            }
        } else {
            // Bus trace: segments between transitions, value labels when room.
            let mut seg_start = vis_first;
            let mut idx = vis_first;
            while idx <= vis_last {
                let cur = cap.value_at(i, idx);
                let nxt = if idx + 1 <= vis_last { cap.value_at(i, idx + 1) } else { cur };
                let valid = cap.valid.get(i).and_then(|vv| vv.get(idx)).copied().unwrap_or(false);
                if nxt != cur || idx == vis_last {
                    let x0 = v.x_of(seg_start as f64, wave_x0);
                    let x1 = v.x_of((idx + 1) as f64, wave_x0).min(rect.right());
                    if valid {
                        // GTKWave sausage: top/bottom rails with crossing diagonals.
                        p.line_segment(
                            [egui::pos2(x0, hi), egui::pos2(x1, hi)],
                            Stroke::new(1.4, theme.trace_b),
                        );
                        p.line_segment(
                            [egui::pos2(x0, lo), egui::pos2(x1, lo)],
                            Stroke::new(1.4, theme.trace_b),
                        );
                        if seg_start > vis_first {
                            p.line_segment(
                                [egui::pos2(x0, hi), egui::pos2(x0 + 5.0, mid)],
                                Stroke::new(1.4, theme.trace_b),
                            );
                            p.line_segment(
                                [egui::pos2(x0, lo), egui::pos2(x0 + 5.0, mid)],
                                Stroke::new(1.4, theme.trace_b),
                            );
                        }
                        if x1 - x0 > 34.0 {
                            if let Some(val) = cur {
                                p.text(
                                    egui::pos2((x0 + x1) / 2.0, mid - 6.0),
                                    egui::Align2::CENTER_CENTER,
                                    probe_radix_text(probe, val),
                                    egui::FontId::monospace(10.0),
                                    theme.text,
                                );
                            }
                        }
                    } else {
                        p.rect_filled(
                            egui::Rect::from_min_max(
                                egui::pos2(x0, mid - 2.0),
                                egui::pos2(x1, mid + 2.0),
                            ),
                            1.0,
                            theme.danger.gamma_multiply(0.55),
                        );
                    }
                    seg_start = idx + 1;
                }
                idx += 1;
            }
        }
    }

    // Trigger marker.
    if let Some(ti) = cap.trigger_sample {
        let x = v.x_of(ti as f64, wave_x0);
        if x >= rect.left() && x <= rect.right() {
            p.line_segment(
                [egui::pos2(x, top), egui::pos2(x, rect.bottom())],
                Stroke::new(1.6, theme.warn),
            );
            p.text(
                egui::pos2(x + 3.0, top + 2.0),
                egui::Align2::LEFT_TOP,
                "TRIG",
                egui::FontId::monospace(9.0),
                theme.warn,
            );
        }
    }
    // Cursors span the full canvas height (GTKWave pattern).
    let draw_cursor = |sample: usize, color: egui::Color32| {
        let x = v.x_of(sample as f64, wave_x0);
        if x >= rect.left() && x <= rect.right() {
            p.line_segment(
                [egui::pos2(x, top), egui::pos2(x, rect.bottom())],
                Stroke::new(1.2, color),
            );
        }
    };
    if let Some(c) = v.cursor {
        draw_cursor(c, theme.text);
    }
    if let Some(a) = v.cursor_a {
        draw_cursor(a, theme.danger);
    }
    if let Some(b) = v.cursor_b {
        draw_cursor(b, theme.trace_b);
    }
}

fn probe_radix_text(probe: &ila::IlaProbeCfg, val: u64) -> String {
    ila::format_value(val, probe.width(), probe.radix)
}

fn paint_wave_markers(ui: &mut egui::Ui, model: &mut IdeModel) {
    let markers = model.wave.markers.clone();
    let selected = model.selected_wave_marker.clone();
    let mut pick: Option<String> = None;
    if markers.is_empty() {
        // Toolbar already has Add marker - don't steal pane height for an empty table.
        return;
    }
    data_scroll("ug900_wave_markers_scroll").show(ui, |ui| {
        egui::Grid::new("ug900_wave_markers")
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Sample").strong());
                ui.label(RichText::new("Time_ps").strong());
                ui.end_row();
                for (i, m) in markers.iter().enumerate() {
                    let on = selected.as_deref() == Some(m.name.as_str());
                    if ui.selectable_label(on, &m.name).clicked() {
                        pick = Some(i.to_string());
                    }
                    if ui.selectable_label(on, m.sample.to_string()).clicked() {
                        pick = Some(i.to_string());
                    }
                    if ui
                        .selectable_label(on, model.wave.time_ps(m.sample).to_string())
                        .clicked()
                    {
                        pick = Some(i.to_string());
                    }
                    ui.end_row();
                }
            });
    });
    if let Some(spec) = pick {
        let _ = model.select_wave_marker(&spec);
    }
}

fn paint_wave_cursors(ui: &mut egui::Ui, model: &mut IdeModel) {
    let rows = model.wave_cursor_rows();
    let selected = model.selected_wave_cursor.clone();
    let mut pick: Option<String> = None;
    data_scroll("ug900_wave_cursors_scroll").show(ui, |ui| {
        egui::Grid::new("ug900_wave_cursors")
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Sample").strong());
                ui.label(RichText::new("Time_ps").strong());
                ui.label(RichText::new("Delta_ps").strong());
                ui.label(RichText::new("Value").strong());
                ui.end_row();
                for (i, r) in rows.iter().enumerate() {
                    let on = selected.as_deref() == Some(r.name.as_str());
                    if ui.selectable_label(on, &r.name).clicked() {
                        pick = Some(i.to_string());
                    }
                    if ui.selectable_label(on, r.sample_cell()).clicked() {
                        pick = Some(i.to_string());
                    }
                    if ui.selectable_label(on, r.time_cell()).clicked() {
                        pick = Some(i.to_string());
                    }
                    if ui.selectable_label(on, r.delta_cell()).clicked() {
                        pick = Some(i.to_string());
                    }
                    if ui.selectable_label(on, r.value_cell()).clicked() {
                        pick = Some(i.to_string());
                    }
                    ui.end_row();
                }
            });
    });
    if let Some(spec) = pick {
        let _ = model.select_wave_cursor(&spec);
    }
}

fn paint_virtual_buses(ui: &mut egui::Ui, model: &mut IdeModel) {
    let buses = model.wave.virtual_buses.clone();
    let selected = model.selected_virtual_bus.clone();
    let mut pick: Option<String> = None;
    if buses.is_empty() {
        return;
    }
    data_scroll("ug900_virtual_buses_scroll").show(ui, |ui| {
        egui::Grid::new("ug900_virtual_buses")
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Members").strong());
                ui.label(RichText::new("Width").strong());
                ui.label(RichText::new("Value").strong());
                ui.end_row();
                for (i, vb) in buses.iter().enumerate() {
                    let on = selected.as_deref() == Some(vb.name.as_str());
                    let t = model.wave.trace(&vb.name);
                    let width = t.map(|t| t.width).unwrap_or(0);
                    let value = t
                        .map(|t| t.value_at(model.wave.cursor))
                        .unwrap_or_else(|| "-".into());
                    if ui.selectable_label(on, &vb.name).clicked() {
                        pick = Some(i.to_string());
                    }
                    if ui.selectable_label(on, vb.members_cell()).clicked() {
                        pick = Some(i.to_string());
                    }
                    if ui.selectable_label(on, width.to_string()).clicked() {
                        pick = Some(i.to_string());
                    }
                    if ui.selectable_label(on, &value).clicked() {
                        pick = Some(i.to_string());
                    }
                    ui.end_row();
                }
            });
    });
    if let Some(spec) = pick {
        let _ = model.select_virtual_bus(&spec);
    }
}

fn paint_trace_shape(
    ui: &egui::Ui,
    rect: egui::Rect,
    t: &helion_gui::WaveTrace,
    cursor: usize,
    cursor_a: Option<usize>,
    cursor_b: Option<usize>,
    n: usize,
    _ts: u64,
    markers: &[helion_gui::WaveMarker],
    theme: &theme::Theme,
) {
    let p = ui.painter();
    p.rect_filled(rect, 0.0, theme.die_bg);
    let green = theme.ok;
    let dim = theme.ok.gamma_multiply(0.45);
    let ns = t.samples.len().max(1);
    let dx = rect.width() / n.max(1) as f32;
    match t.style {
        WaveStyle::Digital => {
            let y1 = rect.top() + 6.0;
            let y0 = rect.bottom() - 6.0;
            let mut prev = t.samples.first().copied().unwrap_or(0) & 1;
            let mut x0 = rect.left();
            for (i, v) in t.samples.iter().enumerate() {
                let bit = v & 1;
                let y = if bit == 1 { y1 } else { y0 };
                let x1 = rect.left() + dx * (i as f32 + 1.0);
                if bit != prev {
                    let yp = if prev == 1 { y1 } else { y0 };
                    p.line_segment(
                        [egui::pos2(x0, yp), egui::pos2(x0, y)],
                        Stroke::new(1.5_f32, green),
                    );
                }
                p.line_segment(
                    [egui::pos2(x0, y), egui::pos2(x1, y)],
                    Stroke::new(1.5_f32, green),
                );
                if bit == 1 {
                    p.rect_filled(
                        egui::Rect::from_min_max(egui::pos2(x0, y1), egui::pos2(x1, y0)),
                        0.0,
                        theme.ok.gamma_multiply(0.11),
                    );
                }
                prev = bit;
                x0 = x1;
            }
        }
        WaveStyle::Analog => {
            let ys = t.analog_series();
            let max = ys.iter().cloned().fold(1.0_f64, f64::max).max(1.0);
            let mut pts = Vec::new();
            for (i, y) in ys.iter().enumerate() {
                let x = rect.left() + dx * (i as f32 + 0.5);
                let yn = 1.0 - (*y / max) as f32;
                let py = rect.top() + 4.0 + yn * (rect.height() - 8.0);
                pts.push(egui::pos2(x, py));
            }
            for w in pts.windows(2) {
                p.line_segment([w[0], w[1]], Stroke::new(1.6_f32, green));
            }
            let _ = dim;
        }
    }
    if let (Some(a), Some(b)) = (cursor_a, cursor_b) {
        let xa = rect.left() + dx * (a as f32 + 0.5);
        let xb = rect.left() + dx * (b as f32 + 0.5);
        let (l, r) = if xa <= xb { (xa, xb) } else { (xb, xa) };
        p.rect_filled(
            egui::Rect::from_min_max(egui::pos2(l, rect.top()), egui::pos2(r, rect.bottom())),
            0.0,
            theme.trace_b.gamma_multiply(0.11),
        );
    }
    let cx = rect.left() + dx * (cursor as f32 + 0.5);
    p.line_segment(
        [egui::pos2(cx, rect.top()), egui::pos2(cx, rect.bottom())],
        Stroke::new(1.0_f32, theme.gold),
    );
    if let Some(a) = cursor_a {
        let ax = rect.left() + dx * (a as f32 + 0.5);
        p.line_segment(
            [egui::pos2(ax, rect.top()), egui::pos2(ax, rect.bottom())],
            Stroke::new(1.2_f32, theme.danger),
        );
    }
    if let Some(b) = cursor_b {
        let bx = rect.left() + dx * (b as f32 + 0.5);
        p.line_segment(
            [egui::pos2(bx, rect.top()), egui::pos2(bx, rect.bottom())],
            Stroke::new(1.2_f32, theme.trace_b),
        );
    }
    for m in markers {
        let mx = rect.left() + dx * (m.sample as f32 + 0.5);
        p.line_segment(
            [egui::pos2(mx, rect.top()), egui::pos2(mx, rect.bottom())],
            Stroke::new(1.0_f32, theme.violet),
        );
    }
    let _ = ns;
}

fn hw_stat_bit_color(theme: &theme::Theme, name: &str, value: bool) -> Color32 {
    if !value {
        return theme.faint;
    }
    match name {
        "CRC_ERR" => theme.danger,
        "GTS" | "GSR" => theme.warn,
        _ => theme.ok,
    }
}

fn paint_hw(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    ui.heading("Hardware Manager");
    let det = app.boards(false).clone();
    let model = &mut app.model;
    if !det.physical_had {
        ui.label(
            RichText::new(
                "No USB programmer (USB=0) - board DONE refused. Use Program Device (sim) for fabric only, or attach FTDI/HAD.",
            )
            .color(theme.warn)
            .small(),
        );
        let ofl = det
            .usb
            .ofl_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(not on PATH)".into());
        ui.label(
            RichText::new(format!(
                "Last scan · USB={} · OFL={} · physical_had={}",
                det.usb.probes.len(),
                ofl,
                u8::from(det.physical_had),
            ))
            .small()
            .color(theme.dim),
        );
    } else {
        ui.label(
            RichText::new("Physical USB programmer detected (detect only - not board DONE).")
                .color(theme.ok)
                .small(),
        );
    }
    ui.horizontal(|ui| {
        if ui.button("Open Hardware Manager").clicked() {
            let _ = model.exec("open_hw_manager");
        }
        if ui.button("Program Device (sim)").clicked() {
            // Explicit sim cable - fabric DONE only; never board DONE invent.
            match model.program_hw_with_cable("sim") {
                Ok(s) => {
                    app.toasts.push(toasts::ToastKind::Ok, s);
                }
                Err(e) => {
                    app.toasts.push(
                        toasts::ToastKind::Error,
                        format!("Program Device (sim) failed: {e}"),
                    );
                }
            }
        }
        if ui.button("Refresh STAT").clicked() {
            let _ = model.exec("report_hw_stat");
        }
    });
    let remain = ui.available_size();
    let report = model.hw_stat_report();
    let n_tiles = if report.open {
        report.bits.len().max(1)
    } else {
        3
    };
    let (tile_band, ila_h, cols, rows, cw, ch) =
        chrome::program_layout(n_tiles, remain.x, remain.y.max(200.0));
    let (dash, dash_resp) =
        ui.allocate_exact_size(egui::vec2(remain.x.max(80.0), tile_band), Sense::click());
    let origin = egui::pos2(dash.left() + 8.0, dash.top() + 8.0);
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    {
        let p = ui.painter();
        let labels: Vec<(String, bool, Color32)> = if report.open {
            report
                .bits
                .iter()
                .map(|b| {
                    (
                        b.name.clone(),
                        b.value,
                        hw_stat_bit_color(&app.theme, &b.name, b.value),
                    )
                })
                .collect()
        } else {
            vec![
                ("Sim cable".into(), true, theme.ok),
                ("USB / OFL".into(), det.physical_had, theme.warn),
                ("Native FTDI".into(), false, theme.faint),
            ]
        };
        for (i, (name, on, fill)) in labels.iter().enumerate() {
            let c = (i as u32) % cols;
            let r = (i as u32) / cols;
            if r >= rows {
                break;
            }
            let rect = egui::Rect::from_min_size(
                egui::pos2(origin.x + c as f32 * cw, origin.y + r as f32 * ch),
                egui::vec2((cw - 8.0).max(24.0), (ch - 8.0).max(24.0)),
            );
            p.rect_filled(rect, 4.0, *fill);
            p.rect_stroke(
                rect,
                4.0,
                Stroke::new(
                    1.0,
                    if selected.as_deref() == Some(name.as_str()) {
                        theme.gold
                    } else {
                        theme.line
                    },
                ),
                egui::StrokeKind::Inside,
            );
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("{} {}", name, if *on { "1" } else { "0" }),
                egui::FontId::proportional(13.0),
                theme.die_bg,
            );
        }
        if let Some(pos) = dash_resp.interact_pointer_pos() {
            if dash_resp.clicked() {
                let dx = ((pos.x - origin.x) / cw).floor() as i32;
                let dy = ((pos.y - origin.y) / ch).floor() as i32;
                if dx >= 0 && dy >= 0 {
                    let i = dy as u32 * cols + dx as u32;
                    if let Some((name, _, _)) = labels.get(i as usize) {
                        pick = Some(name.clone());
                    }
                }
            }
        }
    }
    if let Some(name) = pick {
        if report.open {
            let _ = model.select_hw_stat(&name);
        } else if name.starts_with("Sim") {
            let _ = model.exec("open_hw_manager");
        }
    }
    ui.add_space(8.0);
    let (ila_rect, _) = ui.allocate_exact_size(
        egui::vec2(remain.x.max(80.0), ila_h.max(80.0)),
        Sense::hover(),
    );
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(ila_rect.shrink(4.0)),
        |ui| {
            ui.label(RichText::new("ILA Dashboard").strong());
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(model.ila.trigger == IlaTrigger::Rising, "Rising")
                    .clicked()
                {
                    let _ = model.exec("ila_trigger rising");
                }
                if ui
                    .selectable_label(model.ila.trigger == IlaTrigger::Falling, "Falling")
                    .clicked()
                {
                    let _ = model.exec("ila_trigger falling");
                }
                if ui
                    .selectable_label(model.ila.trigger == IlaTrigger::Immediate, "Immediate")
                    .clicked()
                {
                    let _ = model.exec("ila_trigger immediate");
                }
                if ui.button("Window 8").clicked() {
                    let _ = model.exec("ila_window 8");
                }
                if ui.button("Window 16").clicked() {
                    let _ = model.exec("ila_window 16");
                }
                // Default arm uses IdeModel::ila_arm probe resolution (session net / led),
                // not a hardcoded cnt_3 dump - capture is fabric-backed via helion-debug.
                let probe = model.default_ila_probe();
                if ui
                    .button(format!("Arm / Capture {probe}"))
                    .on_hover_text("ila_arm soft ble_out; ila_arm_deep → fabric BRAM sample buffer")
                    .clicked()
                {
                    let _ = model.exec(&format!("ila_arm {probe}"));
                }
                if ui
                    .button("Capture cnt_3")
                    .on_hover_text("counter MSB probe")
                    .clicked()
                {
                    let _ = model.exec("ila_arm cnt_3");
                }
            });
            let samples = model.ila_sample_rows();
            if samples.is_empty() {
                ui.label("No ILA capture yet.");
                ui.weak("Open Hardware Manager, program the sim cable, then Arm / Capture a net.");
                let rest = ui.available_size().max(egui::vec2(120.0, 80.0));
                let (cap, _) = ui.allocate_exact_size(rest, Sense::hover());
                if ui.is_rect_visible(cap) {
                    let p = ui.painter();
                    p.rect_filled(cap, 4.0, theme.bg);
                    p.rect_stroke(
                        cap,
                        4.0,
                        Stroke::new(1.0_f32, theme.line),
                        egui::StrokeKind::Inside,
                    );
                    let lanes = 6;
                    for i in 0..lanes {
                        let y = cap.top() + cap.height() * (i as f32 + 0.5) / lanes as f32;
                        p.line_segment(
                            [
                                egui::pos2(cap.left() + 12.0, y),
                                egui::pos2(cap.right() - 12.0, y),
                            ],
                            Stroke::new(1.0_f32, theme.surface),
                        );
                    }
                    p.text(
                        cap.center(),
                        egui::Align2::CENTER_CENTER,
                        "ILA capture · arm a probe to fill",
                        egui::FontId::proportional(14.0),
                        theme.dim,
                    );
                }
            } else {
                ui.label(format!(
                    "probe={} window={} trigger={} trigger_at={} bits={}",
                    if model.ila.net.is_empty() {
                        "-"
                    } else {
                        model.ila.net.as_str()
                    },
                    model.ila.window,
                    model.ila.trigger.tcl(),
                    model
                        .ila
                        .trigger_at
                        .map(|i| i.to_string())
                        .unwrap_or_else(|| "-".into()),
                    if model.ila.bits.is_empty() {
                        "-"
                    } else {
                        model.ila.bits.as_str()
                    }
                ));
                let cursor = model.wave.cursor;
                let mut pick_s: Option<usize> = None;
                egui::ScrollArea::both().id_salt("hw_ila_scroll").max_height(220.0).show(ui, |ui| {
                    egui::Grid::new("ila_sample_table")
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            ui.label(RichText::new("Sample").strong());
                            ui.label(RichText::new("Time_ps").strong());
                            ui.label(RichText::new("Value").strong());
                            ui.label(RichText::new("Marker").strong());
                            ui.end_row();
                            for r in &samples {
                                let on = cursor == r.sample;
                                if ui.selectable_label(on, r.sample.to_string()).clicked() {
                                    pick_s = Some(r.sample);
                                }
                                ui.label(r.time_ps.to_string());
                                ui.label(r.value.to_string());
                                ui.label(if r.trigger { "TRIGGER" } else { "-" });
                                ui.end_row();
                            }
                        });
                });
                if let Some(i) = pick_s {
                    let _ = model.select_ila_sample(&i.to_string());
                }
            }
        },
    );
}

fn paint_ip(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.heading("IP Integrator");
    ui.weak("Helion-MM block design canvas - IP boxes and interface wires (not AXI, not a catalog dump)");
    ui.horizontal(|ui| {
        if ui.button("Refresh catalog").clicked() {
            let _ = model.exec("ip_catalog");
        }
        if ui.button("Create Block Design").clicked() {
            let _ = model.exec("create_bd");
        }
        // Selection-gated: the model execs against an empty spec when nothing
        // is selected, so disable with a reason instead of failing silently.
        let ip_sel = model.selected_ip.clone();
        let ip_tip = if ip_sel.is_some() {
            ""
        } else {
            "Select an IP core from the catalog below first"
        };
        if ui
            .add_enabled(ip_sel.is_some(), egui::Button::new("Generate Output Products"))
            .on_hover_text(ip_tip)
            .clicked()
        {
            let spec = ip_sel.clone().unwrap_or_default();
            let _ = model.exec(&format!("generate_ip {spec}"));
        }
        if ui
            .add_enabled(ip_sel.is_some(), egui::Button::new("Add to Block Design"))
            .on_hover_text(ip_tip)
            .clicked()
        {
            let spec = ip_sel.unwrap_or_default();
            let _ = model.exec(&format!("create_bd_cell {spec}"));
        }
    });
    let drawing = model
        .block_design
        .as_ref()
        .map(|bd| bd.drawing(&model.ip_catalog));
    let remain = ui.available_size();
    let (catalog_h, canvas) = chrome::ip_layout(remain.x.max(80.0), remain.y.max(200.0));
    // Catalog is a wrapping chip strip so names like h_rv32_hb1 are not clipped in 88px.
    ui.allocate_ui(egui::vec2(remain.x.max(80.0), catalog_h), |ui| {
        paint_ip_catalog(ui, model);
    });
    if let Some(drawing) = drawing.as_ref() {
        egui::CollapsingHeader::new(format!(
            "BD {} · {} IP · {} nets",
            model
                .block_design
                .as_ref()
                .map(|b| b.name.as_str())
                .unwrap_or("-"),
            drawing
                .symbols
                .iter()
                .filter(|s| s.kind != "PORT_IN" && s.kind != "INTERCONNECT")
                .count(),
            drawing.wires.len(),
        ))
        .default_open(false)
        .show(ui, |ui| {
            if !drawing.addresses.is_empty() {
                ui.label(RichText::new("Address Map (Helion-MM)").strong());
                egui::Grid::new("bd_addr_map")
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Slave").strong());
                        ui.label(RichText::new("Offset").strong());
                        ui.label(RichText::new("Range").strong());
                        ui.end_row();
                        for a in &drawing.addresses {
                            ui.label(&a.slave);
                            ui.label(format!("0x{:08x}", a.base));
                            ui.label(format!("0x{:x}", a.range));
                            ui.end_row();
                        }
                    });
            }
            paint_bd_hdl(ui, model);
        });
    }
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(
            canvas.drawn_w.max(remain.x),
            canvas.drawn_h.max(chrome::DRAWING_MIN_HEIGHT * 0.5),
        ),
        Sense::hover(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }
    let p = ui.painter();
    p.rect_filled(rect, 0.0, theme.die_bg);
    let Some(drawing) = drawing else {
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Create Block Design to place Helion-MM IP on the canvas.",
            egui::FontId::proportional(14.0),
            theme.dim,
        );
        return;
    };
    let fit = chrome::hierarchy_fit(drawing.width, drawing.height, rect.width(), rect.height());
    let o = rect.min;
    let sx = fit.scale_x;
    let sy = fit.scale_y;
    let net = theme.ok;
    let mm = theme.info;
    for w in &drawing.wires {
        let col = if w.net == "Helion-MM" { mm } else { net };
        let thick = if w.net == "Helion-MM" {
            3.4_f32
        } else {
            1.6_f32
        };
        let pts: Vec<egui::Pos2> = w
            .points
            .iter()
            .map(|(x, y)| egui::pos2(o.x + *x * sx, o.y + *y * sy))
            .collect();
        for pair in pts.windows(2) {
            p.line_segment([pair[0], pair[1]], Stroke::new(thick, col));
        }
        if let Some(&a) = pts.first() {
            p.text(
                a + egui::vec2(4.0, -8.0),
                egui::Align2::LEFT_BOTTOM,
                &w.net,
                egui::FontId::monospace(9.0),
                col,
            );
        }
    }
    for syb in &drawing.symbols {
        let r = egui::Rect::from_min_size(
            egui::pos2(o.x + syb.x * sx, o.y + syb.y * sy),
            egui::vec2((syb.w * sx).max(8.0), (syb.h * sy).max(8.0)),
        );
        if syb.kind == "PORT_IN" {
            let pts = vec![r.left_top(), r.left_bottom(), r.right_center()];
            p.add(egui::Shape::convex_polygon(
                pts,
                theme.cat_blue,
                Stroke::new(1.0_f32, theme.faint),
            ));
        } else {
            let fill = if syb.kind == "INTERCONNECT" {
                theme.surface
            } else {
                theme.surface
            };
            p.rect_filled(r, 3.0, fill);
            p.rect_stroke(
                r,
                3.0,
                Stroke::new(1.0_f32, theme.faint),
                egui::StrokeKind::Inside,
            );
        }
        p.text(
            egui::pos2(r.center().x, r.top() + 4.0),
            egui::Align2::CENTER_TOP,
            &syb.kind,
            egui::FontId::monospace(10.0),
            theme.info,
        );
        p.text(
            egui::pos2(r.center().x, r.bottom() - 4.0),
            egui::Align2::CENTER_BOTTOM,
            &syb.name,
            egui::FontId::monospace(10.0),
            theme.text,
        );
        if !syb.bus.is_empty() {
            p.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                &syb.bus,
                egui::FontId::monospace(9.0),
                theme.dim,
            );
        }
        for pin in &syb.pins {
            let tip = egui::pos2(o.x + pin.x * sx, o.y + pin.y * sy);
            let edge = if pin.output {
                egui::pos2(r.right(), tip.y)
            } else {
                egui::pos2(r.left(), tip.y)
            };
            if pin.iface {
                let bar = egui::Rect::from_center_size(edge, egui::vec2(10.0, 16.0));
                p.rect_filled(bar, 1.0, mm);
                p.rect_stroke(
                    bar,
                    1.0,
                    Stroke::new(1.0_f32, theme.text),
                    egui::StrokeKind::Outside,
                );
                p.line_segment([edge, tip], Stroke::new(3.4_f32, mm));
            } else {
                p.line_segment([edge, tip], Stroke::new(1.6_f32, net));
                p.circle_filled(tip, 2.0, theme.text);
            }
            let label_pos = if pin.output {
                egui::pos2(r.right() - 4.0, tip.y)
            } else {
                egui::pos2(r.left() + 4.0, tip.y)
            };
            p.text(
                label_pos,
                if pin.output {
                    egui::Align2::RIGHT_CENTER
                } else {
                    egui::Align2::LEFT_CENTER
                },
                &pin.name,
                egui::FontId::monospace(8.0),
                theme.dim,
            );
        }
    }
}

#[allow(dead_code)] // intentional: WIP panel kept for upcoming canvas wiring
fn paint_ip_catalog(ui: &mut egui::Ui, model: &mut IdeModel) {
    let rows = model.ip_catalog_rows();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("IP Catalog").strong());
        ui.label(
            RichText::new(format!("cores={}", rows.len()))
                .small()
                .weak(),
        );
    });
    let selected = model.selected_ip.clone();
    let mut pick: Option<String> = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.spacing_mut().item_spacing.y = 4.0;
        for r in &rows {
            let on = selected.as_deref() == Some(r.name.as_str());
            let w = chrome::ip_catalog_chip_w(&r.name);
            let resp = ui
                .add_sized(
                    [w, chrome::HIT_SIDEBAR],
                    egui::Button::selectable(on, &r.name),
                )
                .on_hover_text(format!("{}\n{}\n{}", r.vlnv, r.bus, r.status));
            if resp.clicked() {
                pick = Some(r.name.clone());
            }
        }
    });
    if let Some(name) = pick {
        let _ = model.select_ip_core(&name);
    }
}

fn paint_bd_hdl(ui: &mut egui::Ui, model: &mut IdeModel) {
    let rows = model.bd_hdl_rows();
    if rows.is_empty() {
        return;
    }
    ui.add_space(6.0);
    ui.label(
        RichText::new("Generated HDL - instance table from helion-bd emit_sv, not a source dump")
            .strong(),
    );
    let selected = model.selected_ip.clone();
    let mut pick: Option<String> = None;
    egui::Grid::new("bd_hdl_table")
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(RichText::new("Instance").strong());
            ui.label(RichText::new("Module").strong());
            ui.label(RichText::new("Bus").strong());
            ui.label(RichText::new("Ports").strong());
            ui.end_row();
            for r in &rows {
                let on = selected.as_deref() == Some(r.module.as_str());
                if ui.selectable_label(on, &r.instance).clicked() {
                    pick = Some(r.module.clone());
                }
                ui.label(&r.module);
                ui.label(&r.bus);
                ui.label(&r.ports);
                ui.end_row();
            }
        });
    if let Some(name) = pick {
        let _ = model.select_ip_core(&name);
    }
}

#[cfg(test)]
mod recent_tests {
    use super::{load_recent_from, recent_menu_label, save_recent_to};
    use std::path::PathBuf;

    #[test]
    fn recent_menu_label_marks_prj() {
        let p = PathBuf::from("/tmp/counter.prj");
        assert_eq!(recent_menu_label(&p), "counter.prj  (project)");
        let sv = PathBuf::from("/tmp/counter.sv");
        assert_eq!(recent_menu_label(&sv), "counter.sv");
    }

    #[test]
    fn recent_persist_round_trip_keeps_prj() {
        let dir = std::env::temp_dir().join(format!("helion-recent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("recent.txt");
        // Use a real file so load_recent_from keeps it.
        let prj = dir.join("counter_wiz.prj");
        std::fs::write(&prj, "part HL10T-C32-1\n").unwrap();
        save_recent_to(&store, &[prj.clone()]);
        let loaded = load_recent_from(&store);
        assert_eq!(loaded, vec![prj]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn tb_status_color(theme: &theme::Theme, s: TbStatus) -> Color32 {
    match s {
        TbStatus::Passed => theme.ok,
        TbStatus::Failed => theme.danger,
        TbStatus::Running => theme.warn,
        TbStatus::Idle => theme.faint,
    }
}

fn tb_sev_color(theme: &theme::Theme, sev: TbSev) -> Color32 {
    match sev {
        TbSev::Info => theme.text,
        TbSev::Warning => theme.warn,
        TbSev::Error => theme.danger,
    }
}

/// Queue one async testbench run on the engine thread with live streaming.
fn tb_start_run(app: &mut HelionIde, sel: String) {
    if app.job.is_some() || app.model.tb_run_active {
        app.progress = "Busy - wait for the current run to finish.".into();
        return;
    }
    let vcd = app.model.tb_vcd_enabled;
    if let Some(i) = app.model.tb_files.iter().position(|f| f.name == sel) {
        app.model.tb_selected = Some(i);
        app.model.tb_status[i] = TbStatus::Running;
        app.model.tb_run_active = true;
        app.model.tb_run_name = sel.clone();
        app.model.tb_log.clear();
        app.model.tb_failures.clear();
    } else {
        app.progress = format!("No testbench `{sel}`.");
        return;
    }
    let (tx, rx) = mpsc::channel();
    app.tb_rx = Some(rx);
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    app.model.tb_cancel = Some(cancel.clone());
    surface::request_job(JobKind::TbRun(TbRunRequest { sel, vcd, tx, cancel: Some(cancel) }));
}

/// Integrated testbench runner panel (VS Code test-explorer layout):
/// testbench list left, live log + failures + history right.
fn paint_testbench(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    // First open: discover immediately so the panel is never an empty stub.
    if app.model.tb_files.is_empty() && app.model.tb_project_dir.is_none() {
        let msg = app.model.tb_rescan(None);
        app.progress = msg;
    }
    let busy = app.job.is_some() || app.model.tb_run_active;

    ui.heading("Testbench");
    ui.horizontal(|ui| {
        if ui
            .add_sized([96.0, chrome::HIT_COMFORT], egui::Button::new("Rescan"))
            .on_hover_text("Re-discover *_tb.sv / tb_*.sv in the project dir")
            .clicked()
        {
            let msg = app.model.tb_rescan(None);
            app.progress = msg;
        }
        let run_all = ui
            .add_enabled(
                !busy && !app.model.tb_files.is_empty(),
                egui::Button::new("Run all"),
            )
            .on_hover_text("Run every testbench in order");
        if run_all.clicked() {
            let names: Vec<String> = app.model.tb_files.iter().map(|f| f.name.clone()).collect();
            app.tb_queue = names;
            if app.tb_queue.first().cloned().is_some() {
                let first = app.tb_queue.remove(0);
                tb_start_run(app, first);
            }
        }
        let mut vcd = app.model.tb_vcd_enabled;
        if ui
            .checkbox(&mut vcd, "VCD dump")
            .on_hover_text("Write run.vcd next to each run's log (feeds VCD power later)")
            .changed()
        {
            app.model.tb_vcd_enabled = vcd;
        }
        if busy {
            ui.spinner();
            ui.label(RichText::new(format!("Running {}…", app.model.tb_run_name)).color(theme.warn));
        }
        // Stop only when this panel owns the in-flight job.
        let tb_running = app.model.tb_run_active;
        if ui
            .add_enabled(
                tb_running,
                egui::Button::new("Stop"),
            )
            .on_hover_text("Stop the in-flight testbench run")
            .clicked()
        {
            let msg = app.model.tb_stop();
            app.tb_queue.clear(); // a stop aborts the whole regression sweep
            app.progress = msg;
        }
    });
    let proj = app
        .model
        .tb_project_dir
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".into());
    ui.label(
        RichText::new(format!("{} · {} testbench(s)", proj, app.model.tb_files.len()))
            .color(theme.faint)
            .small(),
    );
    if !app.model.tb_last_summary.is_empty() {
        ui.label(RichText::new(&app.model.tb_last_summary).color(theme.dim).small());
    }
    ui.separator();

    if app.model.tb_files.is_empty() {
        ui.add_space(12.0);
        ui.label(RichText::new("No testbenches found.").strong());
        ui.label(
            RichText::new(
                "Name a SystemVerilog file *_tb.sv or tb_*.sv and put it next to its DUT \
                 (single synthesizable .sv), then Rescan. Or use tb_add <path>.",
            )
            .color(theme.dim),
        );
        ui.add_space(8.0);
        paint_tb_help(ui, &theme);
        return;
    }

    // Left: testbench list. Right: detail.
    let full_h = ui.available_height().max(200.0);
    ui.columns(2, |cols| {
        let list_w = 280.0f32.min(cols[0].available_width());
        cols[0].set_max_width(list_w);
        paint_tb_list(&mut cols[0], app, busy);
        paint_tb_detail(&mut cols[1], app, busy, full_h);
    });
    ui.separator();
    paint_tb_history(ui, app);
    ui.separator();
    paint_tb_help(ui, &theme);
}

fn paint_tb_list(ui: &mut egui::Ui, app: &mut HelionIde, busy: bool) {
    let theme = app.theme;
    ui.label(RichText::new("Testbenches").strong().small());
    let mut pick: Option<usize> = None;
    let mut run: Option<String> = None;
    egui::ScrollArea::vertical()
        .id_salt("tb_list")
        .show(ui, |ui| {
            for (i, f) in app.model.tb_files.iter().enumerate() {
                let status = app.model.tb_status.get(i).copied().unwrap_or(TbStatus::Idle);
                let on = app.model.tb_selected == Some(i);
                ui.horizontal(|ui| {
                    let resp = ui.selectable_label(on, &f.name);
                    if resp.clicked() {
                        pick = Some(i);
                    }
                    resp.on_hover_text(f.path.display().to_string());
                    ui.add_enabled(
                        false,
                        egui::Button::new(RichText::new(status.label()).color(Color32::BLACK).small())
                            .fill(tb_status_color(&theme, status))
                            .small(),
                    );
                    if ui
                        .add_enabled(!busy, egui::Button::new("Run").small())
                        .on_hover_text(format!("Run {}", f.name))
                        .clicked()
                    {
                        run = Some(f.name.clone());
                    }
                });
            }
        });
    if let Some(i) = pick {
        app.model.tb_selected = Some(i);
    }
    if let Some(name) = run {
        tb_start_run(app, name);
    }
}

fn paint_tb_detail(ui: &mut egui::Ui, app: &mut HelionIde, busy: bool, full_h: f32) {
    let theme = app.theme;
    let sel = app.model.tb_selected.filter(|&i| i < app.model.tb_files.len());
    let Some(i) = sel else {
        ui.label(RichText::new("Select a testbench.").color(theme.faint));
        return;
    };
    let name = app.model.tb_files[i].name.clone();
    let path = app.model.tb_files[i].path.display().to_string();
    let status = app.model.tb_status.get(i).copied().unwrap_or(TbStatus::Idle);
    ui.horizontal(|ui| {
        ui.label(RichText::new(&name).strong());
        ui.label(
            RichText::new(status.label())
                .color(tb_status_color(&theme, status))
                .small(),
        );
        ui.label(RichText::new(&path).color(theme.faint).small());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Run"))
                .on_hover_text(format!("Run {name}"))
                .clicked()
            {
                tb_start_run(app, name.clone());
            }
            if let Some(vcd) = app.model.tb_last_vcd.clone() {
                if ui
                    .button("Open in waveform viewer")
                    .on_hover_text(format!("Open {} in the waveform viewer", vcd.display()))
                    .clicked()
                {
                    app.progress = app.model.tb_open_vcd_in_waveform(&vcd);
                }
            }
        });
    });
    ui.separator();

    // Failures first (clickable → editor), then the live log.
    let mut jump: Option<usize> = None;
    let mut sch: Option<usize> = None;
    if !app.model.tb_failures.is_empty() {
        ui.label(
            RichText::new(format!("Failures ({})", app.model.tb_failures.len()))
                .strong()
                .color(theme.danger)
                .small(),
        );
        egui::ScrollArea::vertical()
            .id_salt("tb_failures")
            .max_height(130.0)
            .show(ui, |ui| {
                for (fi, f) in app.model.tb_failures.iter().enumerate() {
                    let sigs = if f.signals.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " · {}",
                            f.signals
                                .iter()
                                .map(|(n, v)| format!("{n}={v}"))
                                .collect::<Vec<_>>()
                                .join(" ")
                        )
                    };
                    let label = format!(
                        "@{} ns {}:{}: {}{}",
                        f.time_ns,
                        f.file.rsplit('/').next().unwrap_or(&f.file),
                        f.line,
                        f.message,
                        sigs,
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .button(RichText::new(label).color(theme.danger).small())
                            .on_hover_text(format!("Open {}:{}", f.file, f.line))
                            .clicked()
                        {
                            jump = Some(fi);
                        }
                        if ui
                            .small_button("sch")
                            .on_hover_text("Highlight the failing DUT instance in the schematic")
                            .clicked()
                        {
                            sch = Some(fi);
                        }
                    });
                }
            });
        ui.separator();
    }
    if let Some(fi) = jump {
        let msg = app.model.tb_jump_to_failure(fi).unwrap_or_else(|e| e);
        app.progress = msg;
        return;
    }
    if let Some(fi) = sch {
        app.progress = app
            .model
            .tb_highlight_failure_in_schematic(fi)
            .unwrap_or_else(|e| e);
        return;
    }

    ui.label(RichText::new("Log").strong().small());
    let log_h = (full_h - 210.0).clamp(120.0, 420.0);
    egui::ScrollArea::vertical()
        .id_salt("tb_log")
        .max_height(log_h)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if app.model.tb_log.is_empty() {
                ui.label(RichText::new("No output yet - press Run.").color(theme.faint).small());
                return;
            }
            for l in &app.model.tb_log {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("[{:>8} ns]", l.time_ns))
                            .color(theme.faint)
                            .small()
                            .monospace(),
                    );
                    ui.label(
                        RichText::new(l.sev.tag()).color(tb_sev_color(&theme, l.sev)).small().monospace(),
                    );
                    ui.label(RichText::new(&l.text).small().monospace());
                });
            }
        });
}

fn paint_tb_history(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    ui.label(RichText::new("Run history").strong().small());
    if app.model.tb_history.is_empty() {
        ui.label(
            RichText::new("No runs yet - history persists per project.")
                .color(theme.faint)
                .small(),
        );
        return;
    }
    let col_w = chrome::stretched_col_w(7, ui.available_width());
    egui::ScrollArea::vertical()
        .id_salt("tb_history")
        .max_height(150.0)
        .show(ui, |ui| {
            egui::Grid::new("tb_history_grid")
                .spacing([8.0, 4.0])
                .min_col_width(col_w.min(150.0))
                .show(ui, |ui| {
                    for h in ["Testbench", "Result", "Sim time", "Wall", "Errors", "VCD", "Waveform"] {
                        ui.label(RichText::new(h).strong().small());
                    }
                    ui.end_row();
                    let mut open_wave: Option<PathBuf> = None;
                    for e in app.model.tb_history.iter().rev().take(20) {
                        ui.label(RichText::new(&e.tb_name).small());
                        let (txt, col) = if e.passed { ("Passed", theme.ok) } else { ("Failed", theme.danger) };
                        ui.label(RichText::new(txt).color(col).small());
                        ui.label(RichText::new(format!("{} ns", e.sim_time_ns)).small());
                        ui.label(RichText::new(format!("{} ms", e.runtime_ms)).small());
                        ui.label(RichText::new(format!("{}", e.errors)).small());
                        ui.label(RichText::new(if e.vcd_path.is_some() { "yes" } else { "-" }).small());
                        if let Some(vcd) = &e.vcd_path {
                            if ui
                                .small_button("open")
                                .on_hover_text(format!("Open {} in the waveform viewer", vcd.display()))
                                .clicked()
                            {
                                open_wave = Some(vcd.clone());
                            }
                        } else {
                            ui.label(RichText::new("-").color(theme.faint).small());
                        }
                        ui.end_row();
                    }
                    if let Some(vcd) = open_wave {
                        app.progress = app.model.tb_open_vcd_in_waveform(&vcd);
                    }
                });
        });
}

fn paint_tb_help(ui: &mut egui::Ui, theme: &theme::Theme) {
    ui.collapsing(
        RichText::new("Supported testbench subset").color(theme.faint).small(),
        |ui| {
            ui.label(
                RichText::new(
                    "module tb (no ports) · logic/reg/wire with [msb:lsb] · one DUT \
                     instantiation with named .port(sig) connections · initial blocks · \
                     always #N sig = ~sig clocks (#1 = 1 ns) · #delays · @(posedge/negedge sig) · \
                     blocking = assignments · if/else · repeat · assert (e) else $error(\"…\") · \
                     $display/$write/$error/$fatal/$warning with %d %b %h %t %% and $time · $finish.\n\
                     The DUT is synthesized with helion-sv and driven on the helion-sim event \
                     kernel; VCD dumps use a 1 ns timescale. Anything outside the subset is a \
                     parse error naming the file and line.",
                )
                .color(theme.dim)
                .small(),
            );
        },
    );
}
