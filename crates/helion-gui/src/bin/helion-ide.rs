//! Helion IDE — three canvases, one activity rail, one Implement.
//!
//! `--version` / `--doctor` never open a window (so they work headless and on CI).
//! The GUI paints [`helion_gui::IdeModel`]; every button and the Tcl box call into
//! that model, which is what the unit tests already prove is not a no-op.

use eframe::egui::{self, Color32, RichText, Sense, Stroke};
use helion_gui::chrome::{self, Activity, Canvas, RAIL_OPEN_SOURCES};
use helion_gui::surface::{
    self, apply_activity, apply_canvas, apply_more, central_pane, queue_flow, spawn_job, ChromeState,
    EventClass, JobHandle, JobKind, UiEvent, UiTrace,
};
use helion_gui::{
    doctor, open_hdl_dialog, pane_for_workspace, BottomTab, CdcSeverity, ClockRelation,
    ConstraintSection, DrcSeverity, FlowStep, IdeModel, IlaTrigger, LayoutKind,
    MethodologySeverity, MsgSeverity, PathGroupKind, StepState, WaveRadix, WaveStyle,
    WorkspacePane, WorkspaceTab,
};
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

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
            .with_min_inner_size([720.0, 400.0])
            .with_title("Helion"),
        ..Default::default()
    };
    eframe::run_native(
        "Helion",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
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
    show_palette: bool,
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
    /// Cached USB/OFL scan — detect_boards shells openFPGALoader; never call it every frame.
    board_detect: Option<helion_hw::DetectReport>,
    trace: UiTrace,
    job: Option<JobHandle>,
    busy: bool,
    progress: String,
    /// Last window size we shared chrome against.
    last_share: [f32; 2],
    force_share: bool,
}

impl HelionIde {
    fn new() -> Self {
        let mut model = IdeModel::new();
        let has_rtl = !model.tree.sources.is_empty();
        // Floorplan is already preloaded on IdeModel::new (HAD die, no sources).
        let canvas = if has_rtl { Canvas::Device } else { Canvas::Editor };
        let activity = if has_rtl { Activity::Device } else { Activity::Files };
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
            show_palette: false,
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
            busy: false,
            progress: String::new(),
            last_share: [0.0, 0.0],
            force_share: true,
        };
        if let Ok(w) = std::env::var("HELION_SIDEBAR_WIDTH") {
            if let Ok(v) = w.parse::<f32>() {
                app.sidebar_width = v.clamp(chrome::SIDEBAR_MIN_WIDTH, chrome::SIDEBAR_MAX_WIDTH);
            }
        }
        if let Ok(h) = std::env::var("HELION_CONSOLE_HEIGHT") {
            if let Ok(v) = h.parse::<f32>() {
                app.console_height = v.clamp(chrome::CONSOLE_MIN_HEIGHT, chrome::CONSOLE_MAX_HEIGHT);
            }
        }
        // Optional launch hooks for Mac shots / demos (does not remove Open…).
        if let Ok(path) = std::env::var("HELION_OPEN") {
            let pb = PathBuf::from(path.trim());
            if pb.is_file() {
                app.open_path(&pb);
            }
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
                open_more_workspace(&mut app, tab);
            }
        }
        if let Ok(name) = std::env::var("HELION_ACTIVITY") {
            if let Some(act) = Activity::ALL.iter().copied().find(|a| a.label().eq_ignore_ascii_case(name.trim())) {
                app.set_activity(act);
            }
        }
        if let Ok(cmds) = std::env::var("HELION_EXEC") {
            for cmd in cmds.split(';') {
                let t = cmd.trim();
                if !t.is_empty() {
                    let _ = app.model.exec(t);
                }
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
        self.log_click(c.label(), &format!("{:?}", match c {
            Canvas::Editor => WorkspacePane::Editor,
            Canvas::Device => WorkspacePane::Device,
            Canvas::Timing => WorkspacePane::Timing,
        }));
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
            self.progress = "Busy — wait for the current run to finish.".into();
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
        self.job = Some(spawn_job(model, kind));
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = self.job.as_ref() else {
            return;
        };
        match job.try_recv() {
            Ok(out) => {
                self.model = out.model;
                self.busy = false;
                self.progress = match &out.result {
                    Ok(s) if s.is_empty() => format!("{} finished.", out.kind.widget()),
                    Ok(s) => s.clone(),
                    Err(e) => e.clone(),
                };
                if matches!(out.kind, JobKind::Implement) && out.result.is_ok() {
                    self.set_canvas(Canvas::Device);
                }
                if matches!(out.kind, JobKind::Open(_)) && out.result.is_ok() {
                    self.set_activity(Activity::Files);
                }
                if matches!(out.kind, JobKind::SimRun(_)) && out.result.is_ok() {
                    self.set_activity(Activity::Simulate);
                }
                self.job = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint();
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.busy = false;
                self.progress = "Engine thread stopped.".into();
                self.job = None;
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
            Err(_) => {}
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
    /// Idle policy: reactive eframe only — never `request_repaint` / Continuous here.
    /// Synth/implement run on the engine thread via submit_job / queue_flow.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        handle_shortcuts(ctx, self);
        for kind in surface::take_jobs() {
            self.submit_job(kind);
        }
        self.poll_job(ctx);
        paint_toolbar(ctx, self);
        apply_shared_chrome(ctx, self);
        let board_soft_hold = self.activity == Activity::Program
            && self
                .board_detect
                .as_ref()
                .map(|d| !d.physical_had)
                .unwrap_or(true);
        paint_status_bar(
            ctx,
            self.activity,
            self.canvas,
            &self.model,
            board_soft_hold,
            &self.progress,
        );
        paint_bottom(ctx, self);
        paint_activity_rail(ctx, self);
        if !self.sidebar_hidden {
            paint_sidebar(ctx, self);
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            paint_workspace(ui, self);
        });
        paint_tcl_window(ctx, self);
        paint_palette(ctx, self);
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
    let name = app
        .shot_name
        .clone()
        .unwrap_or_else(|| "helion".into());
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
        app.show_palette = !app.show_palette;
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
fn paint_remaining_cta(ui: &mut egui::Ui, message: &str, action: &str) -> bool {
    let avail = ui.available_size().max(egui::vec2(160.0, 120.0));
    let fit = chrome::empty_cta_occupies_pane(48.0, avail.y);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(avail.x, fit.drawn_h.max(avail.y)),
        Sense::hover(),
    );
    let mut clicked = false;
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 4.0, Color32::from_rgb(0x16, 0x1c, 0x22));
        ui.painter().rect_stroke(
            rect,
            4.0,
            Stroke::new(1.0_f32, Color32::from_rgb(0x3a, 0x42, 0x4a)),
            egui::StrokeKind::Inside,
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((rect.height() * 0.38).max(24.0));
                ui.label(
                    RichText::new(message)
                        .size(14.0)
                        .color(Color32::from_rgb(0xa0, 0xa8, 0xb0)),
                );
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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    let synth_blocked = app.model.step_blocked(FlowStep::Synthesis);
                    let busy = app.busy;
                    ui.add_enabled_ui(synth_blocked.is_none(), |ui| {
                        let impl_label = if busy { "Implementing…" } else { "Implement" };
                        let impl_btn = ui
                            .add_sized(
                                chrome::toolbar_ctrl_size(impl_label),
                                egui::Button::new(RichText::new(impl_label).strong()),
                            )
                            .on_hover_text(match synth_blocked {
                                Some(why) => why.to_string(),
                                None if busy => "Busy — wait for the current run.".into(),
                                None => tip("Implement", "⌘↩", "impl_design"),
                            });
                        if impl_btn.clicked() {
                            run_implement(app);
                        }
                    });
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
                });
            });
        });
}

fn paint_progress_strip(ui: &mut egui::Ui, app: &mut HelionIde) {
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
                StepState::Pending => (
                    Color32::from_rgb(0x2b, 0x32, 0x3a),
                    Color32::from_rgb(0x5a, 0x64, 0x6e),
                    Color32::from_rgb(0xdc, 0xe0, 0xe4),
                ),
                StepState::Done => (
                    Color32::from_rgb(0x1f, 0x4a, 0x38),
                    Color32::from_rgb(0x3d, 0xb8, 0x7a),
                    Color32::from_rgb(0xc8, 0xf0, 0xd8),
                ),
                StepState::Failed => (
                    Color32::from_rgb(0x4a, 0x22, 0x28),
                    Color32::from_rgb(0xe0, 0x6c, 0x75),
                    Color32::from_rgb(0xff, 0xd0, 0xd4),
                ),
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
            let mut pick = None;
            for act in Activity::ALL {
                let on = app.activity == act;
                let fill = if on {
                    Color32::from_rgb(0x1f, 0x4a, 0x38)
                } else {
                    Color32::TRANSPARENT
                };
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(chrome::RAIL_WIDTH - 4.0, chrome::HIT_RAIL),
                    Sense::click(),
                );
                if ui.is_rect_visible(rect) {
                    ui.painter().rect(
                        rect,
                        3.0,
                        fill,
                        Stroke::new(
                            if on { 1.0_f32 } else { 0.0_f32 },
                            Color32::from_rgb(0x3d, 0xb8, 0x7a),
                        ),
                        egui::StrokeKind::Inside,
                    );
                    let c = rect.center();
                    // Word is the primary label (MUST 3). Letter is a small index, not the name.
                    ui.painter().text(
                        egui::pos2(c.x, c.y - 12.0),
                        egui::Align2::CENTER_CENTER,
                        act.icon(),
                        egui::FontId::proportional(10.0),
                        if on {
                            Color32::from_rgb(0x8a, 0xc4, 0xa4)
                        } else {
                            Color32::from_rgb(0x7a, 0x84, 0x8c)
                        },
                    );
                    ui.painter().text(
                        egui::pos2(c.x, c.y + 8.0),
                        egui::Align2::CENTER_CENTER,
                        act.short_label(),
                        egui::FontId::proportional(12.0),
                        if on {
                            Color32::from_rgb(0xc8, 0xf0, 0xd8)
                        } else {
                            Color32::from_rgb(0xdc, 0xe0, 0xe4)
                        },
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
        Activity::Timing => {} // WNS + paths live in the Timing canvas; no catalog twin
        Activity::Files | Activity::Device | Activity::Program | Activity::Reports => {
            paint_files_side(ctx, app);
        }
    }
}

fn paint_files_side(ctx: &egui::Context, app: &mut HelionIde) {
    let share = chrome::share_available(ctx.screen_rect().width(), ctx.screen_rect().height());
    let mut width = app.sidebar_width.clamp(share.sidebar_floor, share.sidebar_cap);
    let mut panel = egui::SidePanel::left("sidebar_v3")
        .resizable(true)
        .default_width(width);
    if app.force_share {
        panel = panel.exact_width(width);
        app.force_share = false;
    }
    let inner = panel
        .width_range({
            let share = chrome::share_available(ctx.screen_rect().width(), ctx.screen_rect().height());
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
                Activity::Files => paint_files_tree(ui, app),
                Activity::Device => {
                    paint_io_ports_table(ui, &mut app.model, "sidebar_io");
                }
                Activity::Timing => {
                    paint_timing_paths(ui, &mut app.model);
                }
                Activity::Reports => {
                    paint_report_catalog(ui, &mut app.model, "sidebar_reports_catalog");
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
    app.sidebar_width = inner.response.rect.width().clamp(share.sidebar_floor, share.sidebar_cap);
    let _ = width;
}


fn paint_program_side(ui: &mut egui::Ui, app: &mut HelionIde) {
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
        ui.label(RichText::new(c.detail.as_str()).small().color(Color32::from_rgb(0xa0, 0xa8, 0xb0)));
    }
    // Soft-hold banner while no physical FTDI/HAD — never claim board DONE from detect alone.
    if det.physical_had {
        ui.label(
            RichText::new("Physical USB programmer detected (openFPGALoader / native FTDI).")
                .color(Color32::from_rgb(0x3d, 0xb8, 0x7a))
                .small(),
        );
    } else {
        ui.label(
            RichText::new(
                "Physical board soft-hold — no USB programmer detected. Use sim cable or attach FTDI/HAD.",
            )
            .color(Color32::from_rgb(0xe0, 0xa0, 0x40))
            .small(),
        );
    }
    ui.add_space(6.0);

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
        ui.label(
            RichText::new("No bitstream yet.")
                .color(Color32::from_rgb(0xe0, 0xa0, 0x40)),
        );
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
            if hw.programmed {
                "programmed"
            } else {
                "idle"
            }
        ));
    } else {
        ui.label(
            RichText::new("Hardware Manager closed — Program opens sim cable.")
                .small()
                .color(Color32::from_rgb(0xa0, 0xa8, 0xb0)),
        );
    }
    ui.add_space(8.0);

    let cable_needs_phys = matches!(
        app.program_cable.as_str(),
        "usb" | "ofl" | "native" | "ftdi" | "libusb" | "openfpgaloader"
    );
    let phys_blocked = cable_needs_phys && !det.physical_had;
    let phys_block_msg =
        "No USB programmer detected (physical soft-hold). Switch cable to sim, or attach FTDI/HAD and Detect.";

    ui.horizontal(|ui| {
        if sidebar_button(ui, "Detect")
            .on_hover_text("Detect cables — honest USB/OFL scan; never fakes a probe")
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
            // Honest scan summary only — never invent a probe / board DONE.
            app.program_status = Some((fresh.physical_had, summary));
            let _ = app.model.exec("open_hw_manager");
        }
        let prog_hover = if phys_blocked {
            phys_block_msg.to_string()
        } else if bits_done {
            tip("Program", "", "program_hw")
        } else {
            "No bitstream yet — Generate Bitstream first".into()
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
            Color32::from_rgb(0xa0, 0xa8, 0xb0)
        } else if *ok {
            Color32::from_rgb(0x3d, 0xb8, 0x7a)
        } else {
            Color32::from_rgb(0xe0, 0x50, 0x50)
        };
        ui.label(RichText::new(heading).strong().color(heading_color));
        ui.label(
            RichText::new(msg.as_str()).color(if is_scan || *ok {
                Color32::from_rgb(0xc0, 0xc8, 0xd0)
            } else {
                Color32::from_rgb(0xe0, 0x80, 0x80)
            }),
        );
    }
}

fn paint_files_tree(ui: &mut egui::Ui, app: &mut HelionIde) {
    let src_rows = app.model.source_rows();
    if src_rows.is_empty() {
        ui.label("No sources.");
        ui.label(
            RichText::new("Use Open… in the toolbar.")
                .small()
                .color(Color32::from_rgb(0xa0, 0xa8, 0xb0)),
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

fn paint_status_bar(
    ctx: &egui::Context,
    activity: Activity,
    canvas: Canvas,
    model: &IdeModel,
    board_soft_hold: bool,
    progress: &str,
) {
    egui::TopBottomPanel::bottom("status")
        .exact_height(chrome::STATUS_HEIGHT)
        .show_separator_line(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let honest = model.timing_honesty_label();
                let wns = if model.timing_closed_wns() {
                    honest
                } else if honest.starts_with("no_body") {
                    "no_body".into()
                } else if honest.starts_with("no_clock_path") {
                    "no_clock_path".into()
                } else if honest.contains("default period") {
                    honest
                } else {
                    honest
                };
                let lutff = model
                    .utilization
                    .as_ref()
                    .map(|u| format!("{}/{}", u.lutff, u.lutff_cap))
                    .unwrap_or_else(|| "—".into());
                let run = model
                    .runs
                    .iter()
                    .rev()
                    .find(|r| r.status != "Not started")
                    .map(|r| r.name.as_str())
                    .unwrap_or("idle");
                // Cheap CLI breadcrumb: Activity › Canvas (or workspace canvas label).
                let where_label = match model.workspace.canvas() {
                    WorkspaceTab::TextEditor | WorkspaceTab::Device | WorkspaceTab::Reports => {
                        canvas.label()
                    }
                    _ => model.workspace.canvas_label(),
                };
                let crumb = format!("{} › {}", activity.label(), where_label);
                // Soft-hold crumb on Program rail — visible without opening Program side.
                // Detect only; never claims board DONE. Sim Program path unchanged.
                let board_crumb = if board_soft_hold {
                    " · board:soft-hold"
                } else {
                    ""
                };
                let progress_bit = if progress.is_empty() {
                    String::new()
                } else {
                    format!(" · {progress}")
                };
                ui.label(
                    RichText::new(format!(
                        "{} · {} · {} · LUTFF {} · {}{}{}",
                        crumb,
                        model.part(),
                        wns,
                        lutff,
                        run,
                        board_crumb,
                        progress_bit
                    ))
                    .monospace()
                    .size(12.0)
                    .color(Color32::from_rgb(0x9a, 0xa4, 0xae)),
                );
            });
        });
}

fn paint_properties(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.separator();
    ui.label(RichText::new("Properties").strong());
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
    if (app.last_share[0] - size[0]).abs() > 1.0 || (app.last_share[1] - size[1]).abs() > 1.0 {
        app.sidebar_width = share.sidebar_w;
        app.console_height = share.console_h;
        app.last_share = size;
        app.force_share = true;
    } else {
        app.sidebar_width = app.sidebar_width.clamp(share.sidebar_floor, share.sidebar_cap);
        app.console_height = app.console_height.clamp(share.console_floor, share.console_cap);
    }
}

fn paint_bottom(ctx: &egui::Context, app: &mut HelionIde) {
    let share = chrome::share_available(ctx.screen_rect().width(), ctx.screen_rect().height());
    let mut console = egui::TopBottomPanel::bottom("console")
        .resizable(true)
        .default_height(app.console_height)
        .min_height(share.console_floor)
        .max_height(share.console_cap);
    if app.force_share || ctx.screen_rect().height() < 540.0 {
        console = console.exact_height(app.console_height.min(share.console_h));
    }
    let inner = console
        .show_separator_line(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let console_on = app.model.bottom_tab == BottomTab::Tcl
                    || app.model.bottom_tab == BottomTab::Log;
                let c = ui.add_sized(
                    [88.0, chrome::HIT_SIDEBAR],
                    egui::Button::selectable(console_on, "Console"),
                );
                if c.clicked() {
                    app.model.bottom_tab = BottomTab::Tcl;
                }
                let m = ui.add_sized(
                    [96.0, chrome::HIT_SIDEBAR],
                    egui::Button::selectable(
                        app.model.bottom_tab == BottomTab::Messages,
                        "Messages",
                    ),
                );
                if m.clicked() {
                    app.model.bottom_tab = BottomTab::Messages;
                }
                let tcl = ui.add_sized(
                    [56.0, chrome::HIT_SIDEBAR],
                    egui::Button::selectable(app.show_tcl, "Tcl"),
                );
                if tcl.clicked() {
                    app.show_tcl = true;
                    app.tcl_focus = true;
                    app.model.bottom_tab = BottomTab::Tcl;
                }
                if app.activity == Activity::Simulate {
                    let s = ui.add_sized(
                        [88.0, chrome::HIT_SIDEBAR],
                        egui::Button::selectable(
                            app.model.bottom_tab == BottomTab::SimLog,
                            "Sim log",
                        ),
                    );
                    if s.clicked() {
                        app.model.bottom_tab = BottomTab::SimLog;
                    }
                }
            });
            match app.model.bottom_tab {
                BottomTab::Tcl | BottomTab::Log => paint_tcl_console(ui, app),
                BottomTab::Messages => paint_messages(ui, &mut app.model),
                BottomTab::SimLog => paint_sim_log(ui, &mut app.model),
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

fn paint_palette(ctx: &egui::Context, app: &mut HelionIde) {
    if !app.show_palette {
        return;
    }
    let mut open = app.show_palette;
    let mut run = None;
    egui::Window::new("Commands")
        .open(&mut open)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.label("Recipes — run from here or the console.");
            let recipes: &[(&str, &str)] = &[
                ("Implement", "impl_design"),
                ("Report timing", "report_timing"),
                ("Report utilization", "report_utilization"),
                ("Launch selected run", "launch_runs"),
                ("New run", "create_run impl_2 -strategy Default"),
                ("create_clock 10ns", "create_clock -period 10 clk"),
                ("read_xdc examples/counter.sdc", "read_xdc"),
            ];
            for (name, tcl) in recipes {
                if ui
                    .add_sized(
                        [ui.available_width(), chrome::HIT_SIDEBAR],
                        egui::Button::new(*name),
                    )
                    .on_hover_text(*tcl)
                    .clicked()
                {
                    run = Some(*tcl);
                }
            }
        });
    app.show_palette = open;
    if let Some(tcl) = run {
        if tcl == "impl_design" {
            run_implement(app);
        } else if tcl == "launch_runs" {
            if let Some(name) = app
                .model
                .selected
                .as_deref()
                .map(|s| s.strip_prefix("run:").unwrap_or(s).to_string())
            {
                let _ = app.model.exec(&format!("launch_runs {name}"));
            }
        } else if tcl == "read_xdc" {
            let p = helion_device::Device::examples_dir().join("counter.sdc");
            let _ = app.model.exec(&format!("read_xdc {}", p.display()));
        } else {
            let _ = app.model.exec(tcl);
        }
        app.show_palette = false;
    }
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
                            .color(Color32::from_rgb(0xa0, 0xa8, 0xb0)),
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
                            .color(Color32::from_rgb(0xa0, 0xa8, 0xb0)),
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
                            .color(Color32::from_rgb(0xa0, 0xa8, 0xb0)),
                    );
                }
            }

            if let Some(err) = &app.create_wizard.error {
                ui.colored_label(Color32::from_rgb(0xe0, 0x60, 0x60), err);
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
    // Always: Editor | Device | Timing | More ⋯ (overflow keeps prior WorkspaceTab destinations).
    ui.horizontal(|ui| {
        for c in Canvas::ALL {
            // Reports rail owns the catalog view — don't paint it as "Timing" selected (Timing ≠ Reports).
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
                paint_text_editor(ui, &mut app.model);
            }
        }
        Canvas::Device => {
            paint_device(ui, &mut app.model);
        }
        Canvas::Timing => {
            // Full-width vertical stack. No SidePanel twin, no set_min_size (that created a tall black hole).
            egui::ScrollArea::vertical()
                .id_salt("timing_canvas_v6")
                .auto_shrink([false, true])
                .hscroll(false)
                .show(ui, |ui| {
                    match app.activity {
                        Activity::Reports => {
                            paint_reports_detail(ui, app);
                        }
                        _ => paint_timing_only(ui, &mut app.model),
                    }
                });
        }
    }
}

fn paint_sim_workspace(ui: &mut egui::Ui, app: &mut HelionIde) {
    // Absolute rect split — clip both sides so long HNF names cannot paint over Memory.
    let full = ui.available_rect_before_wrap();
    let h = full.height().max(200.0);
    let nav_w = chrome::SIDEBAR_WIDTH;
    let rule = chrome::SPLITTER_GRAB_PX;
    let _ = ui.allocate_rect(full, Sense::hover());
    let nav_rect = egui::Rect::from_min_size(full.min, egui::vec2(nav_w, h));
    let sep_rect = egui::Rect::from_min_size(
        egui::pos2(full.min.x + nav_w, full.min.y),
        egui::vec2(rule, h),
    );
    let wave_rect = egui::Rect::from_min_max(
        egui::pos2(full.min.x + nav_w + rule, full.min.y),
        egui::pos2(full.max.x, full.min.y + h),
    );
    ui.painter().rect_filled(nav_rect, 0.0, Color32::from_rgb(0x22, 0x28, 0x30));
    ui.painter().rect_filled(sep_rect, 0.0, Color32::from_rgb(0x3a, 0x42, 0x4a));
    ui.painter().rect_filled(wave_rect, 0.0, Color32::from_rgb(0x1a, 0x1e, 0x24));
    ui.scope_builder(egui::UiBuilder::new().max_rect(nav_rect), |ui| {
        ui.set_clip_rect(nav_rect);
        ui.set_min_size(nav_rect.size());
        ui.set_max_width(nav_rect.width());
        paint_sim_nav_body(ui, &mut app.model);
    });
    ui.scope_builder(egui::UiBuilder::new().max_rect(wave_rect), |ui| {
        ui.set_clip_rect(wave_rect);
        ui.set_min_size(wave_rect.size());
        match pane_for_workspace(app.model.workspace) {
            WorkspacePane::Wave => paint_wave(ui, &mut app.model),
            WorkspacePane::Memory => paint_memory(ui, &mut app.model),
            WorkspacePane::Breakpoints => paint_breakpoints(ui, &mut app.model),
            WorkspacePane::Locals => paint_locals(ui, &mut app.model),
            WorkspacePane::Forces => paint_forces(ui, &mut app.model),
            WorkspacePane::SimSettings => paint_sim_settings(ui, &mut app.model),
            WorkspacePane::Source => paint_source(ui, &mut app.model),
            _ => paint_wave(ui, &mut app.model),
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
        pane, expected,
        "More {} updated workspace but painted {pane:?}",
        tab.label()
    );
}

/// More ⋯ destinations fill the central pane with their real UI — never Timing-only, never a stub heading.
fn paint_more_pane(ui: &mut egui::Ui, app: &mut HelionIde) {
    match pane_for_workspace(app.model.workspace) {
        WorkspacePane::Schematic => paint_schematic(ui, &mut app.model),
        WorkspacePane::Package => paint_package(ui, &mut app.model),
        WorkspacePane::Hierarchy => paint_hierarchy(ui, &mut app.model),
        WorkspacePane::Bitstream => paint_bitstream(ui, &mut app.model),
        WorkspacePane::Hardware => paint_hw(ui, app),
        WorkspacePane::Ip => paint_ip(ui, &mut app.model),
        WorkspacePane::Find => paint_find(ui, &mut app.model),
        WorkspacePane::Settings => paint_project_settings(ui, &mut app.model),
        WorkspacePane::Summary => paint_project_summary(ui, &mut app.model),
        WorkspacePane::Constraints => paint_constraints(ui, &mut app.model),
        WorkspacePane::ClockInteraction => paint_clock_interaction(ui, &mut app.model),
        WorkspacePane::Cdc => paint_cdc(ui, &mut app.model),
        WorkspacePane::ClockNetworks => paint_clock_networks(ui, &mut app.model),
        WorkspacePane::Power => paint_power(ui, &mut app.model),
        WorkspacePane::Methodology => paint_methodology(ui, &mut app.model),
        WorkspacePane::Drc => paint_drc(ui, &mut app.model),
        WorkspacePane::Utilization => paint_utilization(ui, &mut app.model),
        WorkspacePane::Runs => paint_runs(ui, &mut app.model),
        WorkspacePane::Wave
        | WorkspacePane::Source
        | WorkspacePane::Memory
        | WorkspacePane::Breakpoints
        | WorkspacePane::Locals
        | WorkspacePane::Forces
        | WorkspacePane::SimSettings => paint_sim_workspace(ui, app),
        WorkspacePane::ReportsCatalog => paint_reports_detail(ui, app),
        WorkspacePane::Editor => paint_text_editor(ui, &mut app.model),
        WorkspacePane::Device => paint_device(ui, &mut app.model),
        WorkspacePane::Timing => paint_timing_only(ui, &mut app.model),
    }
}


/// Timing ⌘3 / Timing rail: WNS + paths only — never Reports catalog.
fn paint_timing_only(ui: &mut egui::Ui, model: &mut IdeModel) {
    paint_timing_summary(ui, model);
    ui.add_space(8.0);
    paint_timing_paths(ui, model);
}

/// Reports rail: selected report body only (catalog is in the SidePanel). No Timing twin chrome.
fn paint_reports_detail(ui: &mut egui::Ui, app: &mut HelionIde) {
    match app.model.workspace {
        WorkspaceTab::Constraints => paint_constraints(ui, &mut app.model),
        WorkspaceTab::ClockInteraction => paint_clock_interaction(ui, &mut app.model),
        WorkspaceTab::Cdc => paint_cdc(ui, &mut app.model),
        WorkspaceTab::ClockNetworks => paint_clock_networks(ui, &mut app.model),
        WorkspaceTab::Power => paint_power(ui, &mut app.model),
        WorkspaceTab::Methodology => paint_methodology(ui, &mut app.model),
        WorkspaceTab::Drc => paint_drc(ui, &mut app.model),
        WorkspaceTab::Utilization => paint_utilization(ui, &mut app.model),
        WorkspaceTab::Runs => paint_runs(ui, &mut app.model),
        WorkspaceTab::Summary => paint_project_summary(ui, &mut app.model),
        WorkspaceTab::Reports => {
            // Sidebar owns the catalog (MUST: no dual full tables). Center is the first report.
            paint_timing_summary(ui, &mut app.model);
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
        WorkspaceTab::Constraints => paint_constraints(ui, &mut app.model),
        WorkspaceTab::ClockInteraction => paint_clock_interaction(ui, &mut app.model),
        WorkspaceTab::Cdc => paint_cdc(ui, &mut app.model),
        WorkspaceTab::ClockNetworks => paint_clock_networks(ui, &mut app.model),
        WorkspaceTab::Power => paint_power(ui, &mut app.model),
        WorkspaceTab::Methodology => paint_methodology(ui, &mut app.model),
        WorkspaceTab::Drc => paint_drc(ui, &mut app.model),
        WorkspaceTab::Utilization => paint_utilization(ui, &mut app.model),
        WorkspaceTab::Runs => paint_runs(ui, &mut app.model),
        WorkspaceTab::Schematic => paint_schematic(ui, &mut app.model),
        WorkspaceTab::Package => paint_package(ui, &mut app.model),
        WorkspaceTab::Hierarchy => paint_hierarchy(ui, &mut app.model),
        WorkspaceTab::Bitstream => paint_bitstream(ui, &mut app.model),
        WorkspaceTab::Hardware => paint_hw(ui, app),
        WorkspaceTab::Ip => paint_ip(ui, &mut app.model),
        WorkspaceTab::Find => paint_find(ui, &mut app.model),
        WorkspaceTab::Settings => paint_project_settings(ui, &mut app.model),
        WorkspaceTab::Summary => paint_project_summary(ui, &mut app.model),
        _ => {
            paint_timing_summary(ui, &mut app.model);
            ui.add_space(8.0);
            // Paths stay reachable: Timing nav lists them; detail still shows pin delay table.
            paint_timing_paths(ui, &mut app.model);
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


fn paint_clipped_select(ui: &mut egui::Ui, on: bool, shown: &str, tip: &str) -> bool {
    let w = ui.available_width().max(16.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 20.0), Sense::click());
    let resp = resp.on_hover_text(tip);
    if ui.is_rect_visible(rect) {
        let p = ui.painter().with_clip_rect(rect);
        if on {
            p.rect_filled(rect, 2.0, Color32::from_rgb(0x2a, 0x4a, 0x6a));
        }
        p.text(
            egui::pos2(rect.left() + 4.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            shown,
            egui::FontId::monospace(12.0),
            Color32::from_rgb(0xdc, 0xe0, 0xe4),
        );
    }
    resp.clicked()
}

fn paint_sim_nav_body(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.add_space(4.0);
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
        if ui.button("Settings").clicked() {
            let _ = model.exec("simulation_settings");
        }
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
                if paint_clipped_select(ui, on, &shown, &s.name) {
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
                if paint_clipped_select(ui, on, &shown, &o.name) {
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


fn msg_severity_color(sev: MsgSeverity) -> Color32 {
    match sev {
        MsgSeverity::Error => Color32::from_rgb(0xe0, 0x50, 0x50),
        MsgSeverity::Warning => Color32::from_rgb(0xf0, 0xc0, 0x40),
        MsgSeverity::Info => Color32::from_rgb(0x6a, 0xb0, 0xd8),
    }
}

fn paint_messages(ui: &mut egui::Ui, model: &mut IdeModel) {
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
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("No messages.");
                        ui.end_row();
                    } else {
                        for (i, m, obj) in &rows {
                            let on = selected == Some(*i);
                            let fill = msg_severity_color(m.severity);
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

fn log_status_color(ok: bool) -> Color32 {
    if ok {
        Color32::from_rgb(0x50, 0xc0, 0x70)
    } else {
        Color32::from_rgb(0xe0, 0x50, 0x50)
    }
}

fn paint_tcl_console(ui: &mut egui::Ui, app: &mut HelionIde) {
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
        // selection highlighted in the journal grid — no sel= dump crumb
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
        .stick_to_bottom(true)
        .max_height(ui.available_height() - 28.0)
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
                        ui.label("—");
                        ui.label("—");
                        ui.label("No commands yet.");
                        ui.label("—");
                        ui.end_row();
                    } else {
                        for (i, line) in &rows {
                            let on = selected == Some(*i);
                            let hit = hits.contains(i);
                            ui.monospace(i.to_string());
                            let btn = egui::Button::new(
                                RichText::new(line.status()).color(Color32::BLACK),
                            )
                            .fill(log_status_color(line.ok))
                            .selected(on);
                            if ui.add(btn).clicked() {
                                pick_line = Some(*i);
                            }
                            let cmd_col = if on {
                                Color32::from_rgb(0xe5, 0xc0, 0x7b)
                            } else if hit {
                                Color32::from_rgb(0x7e, 0xc8, 0xe3)
                            } else {
                                Color32::from_rgb(0xc8, 0xd0, 0xd8)
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
                                "—".into()
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
        let edit = egui::TextEdit::singleline(&mut model.input)
            .desired_width(f32::INFINITY)
            .hint_text("synth_design / sim_run 16 / nav simulation / report_drc …")
            .font(egui::TextStyle::Monospace);
        let resp = ui.add(edit);
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let _ = model.submit_input();
            resp.request_focus();
        }
    });
}

#[allow(dead_code)] // intentional: WIP panel kept for upcoming canvas wiring
fn paint_log(ui: &mut egui::Ui, model: &mut IdeModel) {
    let selected = model.selected_log;
    let rows: Vec<(usize, helion_gui::ConsoleLine)> = model
        .log_rows()
        .into_iter()
        .map(|(i, l)| (i, l.clone()))
        .collect();
    let mut pick: Option<usize> = None;
    egui::ScrollArea::both()
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
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("No log yet — run a flow step or a console command.");
                        ui.end_row();
                    } else {
                        for (i, line) in &rows {
                            let on = selected == Some(*i);
                            ui.monospace(i.to_string());
                            let status = if line.ok { "ok" } else { "error" };
                            let btn = egui::Button::new(
                                RichText::new(status).color(Color32::BLACK),
                            )
                            .fill(log_status_color(line.ok))
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
                                "—".into()
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

fn paint_sim_log(ui: &mut egui::Ui, model: &mut IdeModel) {
    let _n_err = model
        .sim_log
        .iter()
        .filter(|r| r.severity == MsgSeverity::Error)
        .count();
    let selected = model.selected_sim_log;
    let rows = model.sim_log.clone();
    let mut pick: Option<usize> = None;
    egui::ScrollArea::both()
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
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
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
                            .fill(msg_severity_color(row.severity))
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


fn paint_project_summary(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Project Summary");
    let rows = model.project_summary_gadgets();
    let _wns = model
        .wns_ps()
        .map(|w| w.to_string())
        .unwrap_or_else(|| "-".into());
    let _lutff = rows
        .iter()
        .find(|r| r.id == "utilization")
        .map(|r| r.value.as_str())
        .unwrap_or("-");
    let _run = rows
        .iter()
        .find(|r| r.id == "run")
        .map(|r| r.status.as_str())
        .unwrap_or("-");
    let _hash = rows
        .iter()
        .find(|r| r.id == "bitstream")
        .map(|r| r.value.as_str())
        .unwrap_or("-");
    ui.add_space(6.0);
    let selected = model.selected_summary.clone();
    let mut pick: Option<String> = None;
    let remain = ui.available_size();
    let bbox = chrome::nv_table_bbox(rows.len().max(1), remain.x.max(80.0), remain.y.max(120.0));
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(bbox.drawn_w.max(remain.x), bbox.drawn_h.max(remain.y)),
        Sense::hover(),
    );
    ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(0x1a, 0x1e, 0x24));
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(8.0)), |ui| {
    let n = rows.len().max(1) as f32;
    let row_gap = ((ui.available_height() - 80.0) / n - 18.0).clamp(4.0, 22.0);
    let col_w = chrome::stretched_col_w(3, ui.available_width());
    egui::ScrollArea::both()
        .id_salt("ug893_project_summary")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("project_summary_gadgets")
                .spacing([12.0, row_gap])
                .min_col_width(col_w)
                .show(ui, |ui| {
                    ui.label(RichText::new("Gadget").strong());
                    ui.label(RichText::new("Status").strong());
                    ui.label(RichText::new("Value").strong());
                    ui.end_row();
                    for (i, r) in rows.iter().enumerate() {
                        let on = selected.as_deref() == Some(r.id.as_str());
                        if ui.selectable_label(on, &r.name).clicked() {
                            pick = Some(i.to_string());
                        }
                        let fill = run_status_color(&r.status);
                        let btn = egui::Button::new(RichText::new(&r.status).color(Color32::BLACK))
                            .fill(fill)
                            .selected(on);
                        if ui.add(btn).clicked() {
                            pick = Some(i.to_string());
                        }
                        if ui.selectable_label(on, &r.value).clicked() {
                            pick = Some(i.to_string());
                        }
                        ui.end_row();
                    }
                });
            let report = model.utilization_report();
            if !report.occupancy.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new("Occupancy").strong());
                let bar_span = chrome::occupancy_bar_w(
                    (ui.available_width() - chrome::occupancy_label_reserve(2)).max(80.0),
                );
                let bar_h = chrome::occupancy_bar_h(
                    report.occupancy.len().max(1),
                    (ui.available_height() - 8.0).max(chrome::OCCUPANCY_BAR_H),
                );
                egui::Grid::new("project_summary_occupancy")
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        for row in &report.occupancy {
                            ui.label(row.resource);
                            ui.label(format!("{}/{}", row.used, row.available));
                            let frac = if row.available == 0 {
                                0.0
                            } else {
                                row.used as f32 / row.available as f32
                            };
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(bar_span, bar_h), Sense::hover());
                            ui.painter().rect_filled(
                                rect,
                                2.0,
                                Color32::from_rgb(0x2b, 0x32, 0x3a),
                            );
                            let fill =
                                rect.with_max_x(rect.left() + rect.width() * frac.clamp(0.0, 1.0));
                            ui.painter()
                                .rect_filled(fill, 2.0, Color32::from_rgb(0x7e, 0xc8, 0xe3));
                            ui.end_row();
                        }
                    });
            }
        });
    });
    if let Some(spec) = pick {
        let _ = model.select_project_summary(&spec);
    }
}

fn paint_project_settings(ui: &mut egui::Ui, model: &mut IdeModel) {
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
    ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(0x1a, 0x1e, 0x24));
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

fn run_status_color(status: &str) -> Color32 {
    match status {
        "Complete" => Color32::from_rgb(0x50, 0xc0, 0x70),
        "Running" => Color32::from_rgb(0xf0, 0xc0, 0x40),
        "Failed" => Color32::from_rgb(0xe0, 0x50, 0x50),
        _ => Color32::from_rgb(0x6a, 0x70, 0x78),
    }
}

fn paint_runs(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Runs");
    ui.horizontal(|ui| {
        if ui
            .add_sized([148.0, chrome::HIT_COMFORT], egui::Button::new("Launch selected"))
            .clicked()
        {
            if let Some(name) = model
                .selected
                .as_deref()
                .map(|s| s.strip_prefix("run:").unwrap_or(s).to_string())
            {
                if model.runs.iter().any(|r| r.name == name) {
                    let _ = model.exec(&format!("launch_runs {name}"));
                }
            } else if let Some(r) = model.runs.first() {
                let _ = model.exec(&format!("launch_runs {}", r.name));
            }
        }
        ui.menu_button("New run…", |ui| {
            if ui.button("RuntimeOpt").clicked() {
                let _ = model.exec("create_run impl_runtime -strategy RuntimeOpt");
                ui.close();
            }
            if ui.button("PhysOpt").clicked() {
                let _ = model.exec("create_run impl_phys -strategy PhysOpt");
                ui.close();
            }
        });
    });
    ui.add_space(6.0);
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    egui::ScrollArea::both()
        .max_height(220.0)
        .show(ui, |ui| {
            egui::Grid::new("design_runs_table")
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Name").strong());
                    ui.label(RichText::new("Strategy").strong());
                    ui.label(RichText::new("Status").strong());
                    ui.label(RichText::new("LUTFF").strong());
                    ui.label(RichText::new("WNS_PS").strong());
                    ui.label(RichText::new("Runtime").strong());
                    ui.label(RichText::new("Reuse").strong());
                    ui.label(RichText::new("Hash").strong());
                    ui.end_row();
                    for r in &model.runs {
                        let id = format!("run:{}", r.name);
                        let on = selected.as_deref() == Some(id.as_str())
                            || selected.as_deref() == Some(r.name.as_str());
                        if ui.selectable_label(on, &r.name).clicked() {
                            pick = Some(r.name.clone());
                        }
                        ui.label(r.strategy_cell());
                        let fill = run_status_color(&r.status);
                        let btn = egui::Button::new(
                            RichText::new(&r.status).color(Color32::BLACK),
                        )
                        .fill(fill)
                        .selected(on);
                        if ui.add(btn).clicked() {
                            pick = Some(r.name.clone());
                        }
                        ui.label(r.lutff_cell());
                        ui.label(r.wns_cell());
                        ui.label(r.runtime_cell());
                        ui.label(r.reuse_cell());
                        ui.label(r.hash_cell());
                        ui.end_row();
                    }
                });
        });
    ui.add_space(8.0);
    ui.label(RichText::new("Compare Runs (name / strategy / WNS / runtime / hash)").strong());
    {
        let cmp: Vec<_> = model.compare_run_rows().into_iter().cloned().collect();
        if cmp.is_empty() {
            ui.label("No runs yet. Use New run… or Launch selected.");
        } else {
            egui::Grid::new("compare_runs_table")
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Name").strong());
                    ui.label(RichText::new("Strategy").strong());
                    ui.label(RichText::new("WNS_PS").strong());
                    ui.label(RichText::new("Runtime").strong());
                    ui.label(RichText::new("Hash").strong());
                    ui.end_row();
                    for r in &cmp {
                        let id = format!("run:{}", r.name);
                        let on = selected.as_deref() == Some(id.as_str())
                            || selected.as_deref() == Some(r.name.as_str());
                        if ui.selectable_label(on, r.name.as_str()).clicked() {
                            pick = Some(r.name.clone());
                        }
                        ui.label(r.strategy_cell());
                        ui.label(r.wns_cell());
                        ui.label(r.runtime_cell());
                        ui.label(r.hash_cell());
                        ui.end_row();
                    }
                });
        }
    }
    if let Some(name) = pick {
        let _ = model.select_run(&name);
    }
    ui.add_space(8.0);
    paint_incremental_report(ui, model);
    ui.add_space(8.0);
    paint_eco_changes(ui, model);
}

fn incremental_status_color(status: &str) -> Color32 {
    match status {
        "Reused" => Color32::from_rgb(0x50, 0xc0, 0x70),
        "New" => Color32::from_rgb(0x6a, 0xb0, 0xd8),
        "Partial" => Color32::from_rgb(0xf0, 0xc0, 0x40),
        _ => Color32::from_rgb(0x6a, 0x70, 0x78),
    }
}

fn paint_incremental_report(ui: &mut egui::Ui, model: &mut IdeModel) {
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
                        let fill = incremental_status_color(&r.status);
                        let btn = egui::Button::new(
                            RichText::new(r.status_cell()).color(Color32::BLACK),
                        )
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

fn eco_status_color(status: &str) -> Color32 {
    match status {
        "Placed" => Color32::from_rgb(0x50, 0xc0, 0x70),
        "Missing" => Color32::from_rgb(0xf0, 0xc0, 0x40),
        _ => Color32::from_rgb(0x6a, 0x70, 0x78),
    }
}

fn paint_eco_changes(ui: &mut egui::Ui, model: &mut IdeModel) {
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
                        let fill = eco_status_color(&r.status);
                        let btn = egui::Button::new(
                            RichText::new(r.status_cell()).color(Color32::BLACK),
                        )
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

#[allow(dead_code)] // intentional: WIP panel kept for upcoming canvas wiring
fn paint_hierarchy(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.horizontal(|ui| {
        ui.heading("Hierarchy");
        ui.add_space(12.0);
        if ui.button("Show in Schematic").clicked() {
            let _ = model.exec("open_hierarchy_sheet");
        }
    });
    let drawing = model.hierarchy.drawing();
    ui.label(
        RichText::new(format!(
            "{} blocks · {}×{}",
            drawing.boxes.len(),
            drawing.width as i32,
            drawing.height as i32
        ))
        .small()
        .weak(),
    );
    let selected = model.selected.clone();
    let mut pick = None;
    let avail = ui.available_size();
    let view_w = avail.x.max(80.0);
    let view_h = chrome::pane_view_h(avail.y);
    let fit = chrome::hierarchy_fit(drawing.width, drawing.height, view_w, view_h);
    egui::ScrollArea::both()
        .id_salt("hierarchy_die_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let size = egui::vec2(fit.drawn_w.max(view_w), fit.drawn_h.max(view_h));
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
            if ui.is_rect_visible(rect) {
                let p = ui.painter();
                p.rect_filled(rect, 0.0, Color32::from_rgb(0x12, 0x16, 0x1a));
                let o = rect.min;
                let sx = fit.scale_x;
                let sy = fit.scale_y;
                // Outer boxes first so nested leaves paint on top.
                let mut ordered: Vec<_> = drawing.boxes.iter().collect();
                ordered.sort_by(|a, b| (b.w * b.h).partial_cmp(&(a.w * a.h)).unwrap());
                for b in ordered {
                    let r = egui::Rect::from_min_size(
                        egui::pos2(o.x + b.x * sx, o.y + b.y * sy),
                        egui::vec2((b.w * sx).max(8.0), (b.h * sy).max(8.0)),
                    );
                    let on = selected.as_deref() == Some(b.name.as_str());
                    let fill = if b.kind == "module" {
                        Color32::from_rgb(0x1a, 0x22, 0x1c)
                    } else if b.kind.starts_with("instance:") || b.kind == "leaves" {
                        Color32::from_rgb(0x2a, 0x32, 0x24)
                    } else {
                        Color32::from_rgb(0x3a, 0x42, 0x28)
                    };
                    p.rect_filled(r, 2.0, fill);
                    p.rect_stroke(
                        r,
                        2.0,
                        Stroke::new(
                            if on { 2.0_f32 } else { 1.0_f32 },
                            if on {
                                Color32::from_rgb(0xe5, 0xc0, 0x7b)
                            } else {
                                Color32::from_rgb(0x7a, 0x84, 0x8e)
                            },
                        ),
                        egui::StrokeKind::Inside,
                    );
                    p.text(
                        egui::pos2(r.left() + 6.0, r.top() + 3.0),
                        egui::Align2::LEFT_TOP,
                        &b.name,
                        egui::FontId::monospace(10.0),
                        Color32::from_rgb(0xdc, 0xe0, 0xe4),
                    );
                    p.text(
                        egui::pos2(r.right() - 6.0, r.top() + 3.0),
                        egui::Align2::RIGHT_TOP,
                        format!("{}", b.cells),
                        egui::FontId::monospace(10.0),
                        Color32::from_rgb(0x9a, 0xa4, 0xae),
                    );
                    if !b.kind.starts_with("instance:") && b.kind != "module" && b.kind != "leaves"
                    {
                        p.text(
                            r.center() + egui::vec2(0.0, 6.0),
                            egui::Align2::CENTER_CENTER,
                            &b.kind,
                            egui::FontId::monospace(9.0),
                            Color32::from_rgb(0x7e, 0xc8, 0xe3),
                        );
                    }
                }
            }
            if resp.clicked() || resp.double_clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let lx = (pos.x - rect.left()) / fit.scale_x.max(0.01);
                    let ly = (pos.y - rect.top()) / fit.scale_y.max(0.01);
                    // Prefer the smallest containing box (leaf over parent).
                    let mut hit: Option<&helion_gui::HierBox> = None;
                    for b in &drawing.boxes {
                        if lx >= b.x && lx <= b.x + b.w && ly >= b.y && ly <= b.y + b.h {
                            match hit {
                                None => hit = Some(b),
                                Some(h) if b.w * b.h < h.w * h.h => hit = Some(b),
                                _ => {}
                            }
                        }
                    }
                    if let Some(b) = hit {
                        pick = Some(b.name.clone());
                        if resp.double_clicked() {
                            let _ = model.exec(&format!("open_hierarchy_sheet {}", b.name));
                        }
                    }
                }
            }
        });
    if let Some(id) = pick {
        model.select(&id);
    }
}

fn paint_find(ui: &mut egui::Ui, model: &mut IdeModel) {
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
        if paint_remaining_cta(ui, "No hits yet.", "Find cells") {
            let _ = model.exec("sheet_find cells");
        }
        return;
    }
    egui::ScrollArea::both()
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
    data_scroll("io_ports_table_scroll")
        .show(ui, |ui| {
            egui::Grid::new(grid_id)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
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
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("Synth a design to list ports.");
                        ui.label("—");
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
                                    "— set site —"
                                } else {
                                    cur_pin.as_str()
                                })
                                .width(118.0)
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_label(cur_pin == "-", "— (select port) —")
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
                                    "— Set —"
                                } else {
                                    cur_std.as_str()
                                })
                                .width(96.0)
                                .show_ui(ui, |ui| {
                                    for std in ["LVCMOS18", "LVCMOS33", "LVCMOS12", "LVCMOS25", "SSTL15"] {
                                        if ui.selectable_label(cur_std == std, std).clicked() {
                                            set_iostd = Some((p.name.clone(), std));
                                        }
                                    }
                                });
                            let cur_drv = p.drive_cell().to_string();
                            egui::ComboBox::from_id_salt(("io_drive", p.name.as_str()))
                                .selected_text(if cur_drv == "-" {
                                    "— Set —"
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
                                    "— Set —"
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
    let selected_port = model
        .selected_io_port
        .clone()
        .or_else(|| {
            model.selected.clone().filter(|s| model.io_ports.iter().any(|p| p.name == *s))
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
                if ui.add_sized([64.0, chrome::HIT_SIDEBAR], egui::Button::new(std)).clicked() {
                    set_iostd = Some((port.clone(), std));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("DRIVE");
            for ma in ["4", "8", "12", "16"] {
                if ui.add_sized([40.0, chrome::HIT_SIDEBAR], egui::Button::new(ma)).clicked() {
                    set_io = Some((port.clone(), "DRIVE", ma));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("SLEW");
            for s in ["SLOW", "FAST"] {
                if ui.add_sized([ui.spacing().interact_size.x.max(56.0), chrome::HIT_SIDEBAR], egui::Button::new(s)).clicked() {
                    set_io = Some((port.clone(), "SLEW", s));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("PULLTYPE");
            for s in ["NONE", "PULLUP", "PULLDOWN", "KEEPER"] {
                if ui.add_sized([ui.spacing().interact_size.x.max(56.0), chrome::HIT_SIDEBAR], egui::Button::new(s)).clicked() {
                    set_io = Some((port.clone(), "PULLTYPE", s));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("DIFF_TERM");
            for s in ["FALSE", "TRUE"] {
                if ui.add_sized([ui.spacing().interact_size.x.max(56.0), chrome::HIT_SIDEBAR], egui::Button::new(s)).clicked() {
                    set_io = Some((port.clone(), "DIFF_TERM", s));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("IN_TERM");
            for s in ["NONE", "UNTUNED_SPLIT_40", "UNTUNED_SPLIT_50", "UNTUNED_SPLIT_60"] {
                if ui.add_sized([ui.spacing().interact_size.x.max(56.0), chrome::HIT_SIDEBAR], egui::Button::new(s)).clicked() {
                    set_io = Some((port.clone(), "IN_TERM", s));
                }
            }
        });
    }
    if let Some((port, std)) = set_iostd {
        // ComboBox / button Set → set_iostandard (HNF + constraints); exec syncs the table.
        let _ = model.exec(&format!(
            "set_property IOSTANDARD {std} [get_ports {port}]"
        ));
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
            .on_hover_text("add_cells_to_pblock — assign all design LUT cells")
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
            .on_hover_text("add_cells_to_pblock — assign the netlist selection")
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
                ui.label("—");
                ui.label("—");
                ui.label("—");
                ui.label("—");
                ui.label("—");
                ui.label("—");
                ui.label("—");
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
                    if ui.selectable_label(on, p.english_range().as_str()).clicked() {
                        if p.ranged {
                            pick_obj = Some(format!("CLB_X{}Y{}", p.x0, p.y0));
                        } else {
                            pick_pblock = Some(p.name.clone());
                        }
                    }
                    if ui
                        .selectable_label(on, p.cells.len().to_string())
                        .clicked()
                    {
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

fn paint_package(ui: &mut egui::Ui, model: &mut IdeModel) {
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
        model.package_pins.iter().filter(|p| p.port.is_some()).count()
    ));
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("unassigned")
                .small()
                .color(Color32::from_rgb(0x5a, 0x64, 0x6e)),
        );
        ui.label(
            RichText::new("placed port")
                .small()
                .color(Color32::from_rgb(0x7e, 0xc8, 0xe3)),
        );
        let mut seen = std::collections::HashSet::new();
        for p in &model.package_pins {
            if seen.insert(p.bank) {
                let (r, g, b) = p.bank_rgb();
                ui.label(
                    RichText::new(format!("BANK{}", p.bank))
                        .small()
                        .color(Color32::from_rgb(r.saturating_add(40), g.saturating_add(40), b.saturating_add(40))),
                );
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
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(draw_w, draw_h),
                Sense::click(),
            );
            if ui.is_rect_visible(rect) {
                let origin = egui::pos2(rect.left() + 28.0, rect.top() + 4.0);
                let p = ui.painter();
                p.rect_filled(rect, 0.0, Color32::from_rgb(0x0d, 0x10, 0x12));
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
                    p.rect_filled(
                        cell_rect,
                        3.0,
                        Color32::from_rgba_unmultiplied(br, bg, bb, 70),
                    );
                    let on = selected.as_deref() == Some(pin.pin.as_str())
                        || pin.port.as_deref() == selected.as_deref();
                    let fill = if pin.port.is_some() {
                        Color32::from_rgb(0x7e, 0xc8, 0xe3)
                    } else {
                        Color32::from_rgb(0x3a, 0x44, 0x4e)
                    };
                    p.circle_filled(center, rad, fill);
                    if on {
                        p.circle_stroke(
                            center,
                            rad + 2.0,
                            Stroke::new(1.6_f32, Color32::from_rgb(0xe5, 0xc0, 0x7b)),
                        );
                    }
                    p.text(
                        egui::pos2(center.x, cell_rect.bottom() - 2.0),
                        egui::Align2::CENTER_BOTTOM,
                        &pin.pin,
                        egui::FontId::monospace(8.0),
                        Color32::from_rgb(0xdc, 0xe0, 0xe4),
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
        let selected_port = model.selected.clone().filter(|s| {
            model.io_ports.iter().any(|p| p.name == *s)
        });
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

fn paint_constraints(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Constraints");
    // Sibling/user SDC may already be loaded before the editor was painted.
    model.ensure_sdc_editor_populated();
    ui.horizontal(|ui| {
        ui.menu_button("Add…", |ui| {
            let recipes: &[(&str, &str)] = &[
                ("Clock 10 ns", "create_clock -period 10 clk"),
                ("Input delay 1.5 ns", "set_input_delay -clock clk 1.5 [get_ports clk]"),
                ("Output delay 2 ns", "set_output_delay -clock clk 2.0 [get_ports led]"),
                ("False path", "set_false_path -from [get_ports clk] -to [get_ports led]"),
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
        if ui.button("Open counter.sdc").clicked() {
            let p = helion_device::Device::examples_dir().join("counter.sdc");
            let _ = model.open_sdc_editor(&p);
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
    ];
    for (title, section, grid) in sections {
        let sect: Vec<_> = rows.iter().filter(|r| r.section == section).collect();
        ui.add_space(6.0);
        ui.label(RichText::new(title).strong());
        if sect.is_empty() {
            ui.weak("—");
            continue;
        }
        egui::Grid::new(grid)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Kind").strong());
                ui.label(RichText::new("From").strong());
                ui.label(RichText::new("To").strong());
                ui.label(RichText::new("Value").strong());
                ui.end_row();
                for r in sect {
                    let on = selected.as_deref() == Some(r.id.as_str());
                    let name = if r.name.is_empty() { "-" } else { r.name.as_str() };
                    if ui.selectable_label(on, name).clicked() {
                        pick = Some(r.id.clone());
                    }
                    ui.label(&r.kind);
                    let from = if r.from.is_empty() { "-" } else { r.from.as_str() };
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

fn paint_timing_paths(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.label(RichText::new("Timing Paths").strong());
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("Report timing").clicked() {
            let _ = model.exec("report_timing");
        }
        if ui.button("Show in Schematic").clicked() {
            let spec = model
                .selected_timing_path
                .map(|i| i.to_string())
                .unwrap_or_else(|| "0".into());
            let _ = model.exec(&format!("select_timing_path {spec}"));
        }
    });
    if model.timing_paths.is_empty() {
        ui.label("No timing paths yet.");
        if primary_button(ui, "Report timing").clicked() {
            let _ = model.exec("report_timing");
        }
        return;
    }
    ui.add_space(4.0);
    ui.label(RichText::new("Path Summary").strong());
    let mut pick_path = None;
    let selected_path = model.selected_timing_path;
    let path_sum_col = chrome::stretched_col_w_gap(8, ui.available_width(), 8.0);
    egui::Grid::new("timing_path_summary")
        .spacing([8.0, 4.0])
        .min_col_width(path_sum_col)
        .show(ui, |ui| {
            ui.label(RichText::new("Name").strong());
            ui.label(RichText::new("From").strong());
            ui.label(RichText::new("To").strong());
            ui.label(RichText::new("Slack_ps").strong());
            ui.label(RichText::new("Delay_ps").strong());
            ui.label(RichText::new("Logic_ps").strong());
            ui.label(RichText::new("Net_ps").strong());
            ui.label(RichText::new("Pins").strong());
            ui.end_row();
            for (i, p) in model.timing_paths.iter().enumerate() {
                let on = selected_path == Some(i);
                if ui.selectable_label(on, &p.name).clicked() {
                    pick_path = Some(i);
                }
                ui.label(&p.startpoint);
                ui.label(&p.endpoint);
                ui.label(p.slack_ps.to_string());
                ui.label(p.delay_ps.to_string());
                ui.label(p.logic_ps().to_string());
                ui.label(p.net_ps().to_string());
                ui.label(p.pins.len().to_string());
                ui.end_row();
            }
        });
    if let Some(i) = pick_path {
        let _ = model.select_timing_path_report(&i.to_string());
    }
    let path = model
        .selected_timing_path
        .and_then(|i| model.timing_paths.get(i))
        .or_else(|| model.timing_paths.first())
        .cloned();
    let Some(path) = path else {
        return;
    };
    ui.add_space(6.0);
    ui.label(RichText::new("Pin Delay").strong());
    ui.label(format!(
        "{}  slack_ps={} delay_ps={} logic_ps={} net_ps={}",
        path.name,
        path.slack_ps,
        path.delay_ps,
        path.logic_ps(),
        path.net_ps()
    ));
    let selected_pin = model.selected_timing_pin.clone();
    let mut pick_pin: Option<String> = None;
    egui::ScrollArea::vertical().max_height(280.0).hscroll(true).show(ui, |ui| {
        let pin_col = chrome::stretched_col_w_gap(7, ui.available_width(), 8.0);
        egui::Grid::new("timing_pin_delay")
            .spacing([8.0, 4.0])
            .min_col_width(pin_col)
            .show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Type").strong());
                ui.label(RichText::new("Incr_ps").strong());
                ui.label(RichText::new("Path_ps").strong());
                ui.label(RichText::new("Net").strong());
                ui.label(RichText::new("Fanout").strong());
                ui.label(RichText::new("Location").strong());
                ui.end_row();
                for pin in &path.pins {
                    let on = selected_pin.as_deref() == Some(pin.pin.as_str());
                    if ui.selectable_label(on, &pin.pin).clicked() {
                        pick_pin = Some(pin.pin.clone());
                    }
                    ui.label(&pin.delay_type);
                    ui.label(pin.incr_ps.to_string());
                    ui.label(pin.path_ps.to_string());
                    ui.label(if pin.net.is_empty() {
                        "-"
                    } else {
                        pin.net.as_str()
                    });
                    ui.label(pin.fanout.to_string());
                    ui.label(if pin.location.is_empty() {
                        "-"
                    } else {
                        pin.location.as_str()
                    });
                    ui.end_row();
                }
            });
    });
    if let Some(pin) = pick_pin {
        let _ = model.select_timing_pin(&pin);
    }
}

fn paint_report_catalog(ui: &mut egui::Ui, model: &mut IdeModel, salt: &'static str) {
    let rows = model.report_catalog();
    let selected = model.selected_report.clone();
    let mut pick: Option<String> = None;
    egui::ScrollArea::vertical()
        .id_salt(salt)
        .auto_shrink([false, false])
        .max_height(ui.available_height().max(1.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Name").strong());
            for r in &rows {
                let on = selected.as_deref() == Some(r.id.as_str());
                // Full name, wrapped to the sidebar — never "Timing Summ…".
                let name = egui::Button::selectable(on, RichText::new(&r.name).strong())
                    .wrap()
                    .min_size(egui::vec2(ui.available_width(), 22.0));
                if ui.add(name).clicked() {
                    pick = Some(r.id.clone());
                }
                let status_fill = run_status_color(&r.status);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&r.category).small());
                    let btn = egui::Button::new(RichText::new(&r.status).color(Color32::BLACK))
                        .fill(status_fill)
                        .selected(on);
                    if ui.add(btn).clicked() {
                        pick = Some(r.id.clone());
                    }
                });
                ui.add(
                    egui::Label::new(RichText::new(&r.summary).small().weak()).wrap(),
                );
                ui.add_space(6.0);
            }
        });
    if let Some(id) = pick {
        let _ = model.select_report(&id);
    }
}

fn slack_label(v: Option<i64>) -> String {
    match v {
        Some(w) => w.to_string(),
        None => "n/a".into(),
    }
}

fn paint_timing_summary(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.label(
        RichText::new(
            "Timing Summary",
        )
        .strong(),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("Report timing summary").clicked() {
            let _ = model.exec("report_timing_summary");
        }
    });
    // CLI-honest label. Closed WNS_PS= only with cells>0 and an Hff clock path.
    let honest = model.timing_honesty_label();
    let closed = model.timing_closed_wns();
    ui.label(
        RichText::new(honest.clone())
            .monospace()
            .size(16.0)
            .color(if closed {
                Color32::from_rgb(0xc8, 0xf0, 0xd8)
            } else {
                Color32::from_rgb(0xe5, 0xc0, 0x7b)
            }),
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
    ui.label(RichText::new("Design Timing Summary").strong());
    let col_w = chrome::stretched_col_w(7, ui.available_width());
    egui::Grid::new("timing_summary_design")
        .spacing([12.0, 4.0])
        .min_col_width(col_w)
        .show(ui, |ui| {
            ui.label(RichText::new("WNS_PS").strong());
            ui.label(RichText::new("TNS_PS").strong());
            ui.label(RichText::new("WHS_PS").strong());
            ui.label(RichText::new("THS_PS").strong());
            ui.label(RichText::new("FAILING_SETUP").strong());
            ui.label(RichText::new("FAILING_HOLD").strong());
            ui.label(RichText::new("ENDPOINTS").strong());
            ui.end_row();
            ui.label(slack_label(report.wns_ps));
            ui.label(report.tns_ps.to_string());
            ui.label(slack_label(report.whs_ps));
            ui.label(report.ths_ps.to_string());
            ui.label(report.failing_setup.to_string());
            ui.label(report.failing_hold.to_string());
            ui.label(report.endpoints.to_string());
            ui.end_row();
        });
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
                ui.label(RichText::new("WNS_PS").strong());
                ui.label(RichText::new("TNS_PS").strong());
                ui.label(RichText::new("WHS_PS").strong());
                ui.label(RichText::new("THS_PS").strong());
                ui.label(RichText::new("ENDPOINTS").strong());
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

fn clock_relation_color(rel: ClockRelation) -> Color32 {
    match rel {
        ClockRelation::Timed => Color32::from_rgb(0x3c, 0xb3, 0x71),
        ClockRelation::TimedGenerated => Color32::from_rgb(0x6b, 0xc9, 0x6b),
        ClockRelation::TimedUnsafe => Color32::from_rgb(0xf0, 0xc0, 0x40),
        ClockRelation::TimedDatapath => Color32::from_rgb(0xf0, 0x80, 0x40),
        ClockRelation::FalsePath => Color32::from_rgb(0x90, 0x90, 0x90),
        ClockRelation::PartialFalsePath => Color32::from_rgb(0xc0, 0x70, 0x40),
        ClockRelation::Asynchronous => Color32::from_rgb(0x90, 0x60, 0xc0),
        ClockRelation::Exclusive => Color32::from_rgb(0x50, 0x80, 0xc0),
        ClockRelation::NoPaths => Color32::from_rgb(0x40, 0x40, 0x40),
    }
}

fn paint_clock_interaction(ui: &mut egui::Ui, model: &mut IdeModel) {
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
    egui::ScrollArea::both().show(ui, |ui| {
        egui::Grid::new("clock_interaction_matrix")
            .spacing([4.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("From \\ To").strong());
                for c in &report.clocks {
                    let src = c.source.split('/').next().unwrap_or(c.source.as_str());
                    let on_obj = selected.as_deref() == Some(src) || selected.as_deref() == Some(c.name.as_str());
                    if ui
                        .selectable_label(on_obj, RichText::new(&c.name).strong())
                        .clicked()
                    {
                        pick_obj = Some(c.name.clone());
                    }
                }
                ui.end_row();
                for from in &report.clocks {
                    let src = from.source.split('/').next().unwrap_or(from.source.as_str());
                    let on_obj = selected.as_deref() == Some(src)
                        || selected.as_deref() == Some(from.name.as_str());
                    if ui
                        .selectable_label(on_obj, RichText::new(&from.name).strong())
                        .clicked()
                    {
                        pick_obj = Some(from.name.clone());
                    }
                    for to in &report.clocks {
                        if let Some(cell) = report.cell(&from.name, &to.name) {
                            let key = format!("{}->{}", cell.from, cell.to);
                            let on = selected_ci.as_deref() == Some(key.as_str());
                            let fill = clock_relation_color(cell.relation);
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
                            ui.label("—");
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

fn cdc_severity_color(sev: CdcSeverity) -> Color32 {
    match sev {
        CdcSeverity::Critical => Color32::from_rgb(0xe0, 0x50, 0x50),
        CdcSeverity::Warning => Color32::from_rgb(0xf0, 0xc0, 0x40),
        CdcSeverity::Info => Color32::from_rgb(0x90, 0x60, 0xc0),
        CdcSeverity::Safe => Color32::from_rgb(0x3c, 0xb3, 0x71),
    }
}

fn paint_cdc(ui: &mut egui::Ui, model: &mut IdeModel) {
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
    ui.label(format!(
        "critical={} warning={} info={} safe={}",
        report.critical_count(),
        report.warning_count(),
        report.info_count(),
        report.safe_count()
    ));
    let selected_cdc = model.selected_cdc.clone();
    let selected = model.selected.clone();
    let mut pick: Option<(String, String)> = None;
    let mut pick_obj: Option<(String, String)> = None;
    egui::ScrollArea::both().show(ui, |ui| {
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
                ui.label(RichText::new("WNS_PS").strong());
                ui.label(RichText::new("Relation").strong());
                ui.end_row();
                for v in &report.violations {
                    let key = format!("{}->{}", v.from, v.to);
                    let on = selected_cdc.as_deref() == Some(key.as_str());
                    let fill = cdc_severity_color(v.severity);
                    let btn = egui::Button::new(
                        RichText::new(&v.from).color(Color32::BLACK),
                    )
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

fn paint_power(ui: &mut egui::Ui, model: &mut IdeModel) {
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
        if paint_remaining_cta(ui, "No design yet.", "Run Synthesis") {
            queue_flow(FlowStep::Synthesis);
        }
        return;
    }
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
    ui.painter()
        .rect_filled(rect, 0.0, Color32::from_rgb(0x1a, 0x1e, 0x24));
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
                            let (bar, _) = ui.allocate_exact_size(
                                egui::vec2(bar_span, bar_h),
                                Sense::hover(),
                            );
                            ui.painter()
                                .rect_filled(bar, 2.0, Color32::from_rgb(0x2b, 0x32, 0x3a));
                            let fill =
                                bar.with_max_x(bar.left() + bar.width() * frac.clamp(0.0, 1.0));
                            ui.painter()
                                .rect_filled(fill, 2.0, Color32::from_rgb(0x7e, 0xc8, 0xe3));
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
                            ui.painter()
                                .rect_filled(bar, 2.0, Color32::from_rgb(0x2b, 0x32, 0x3a));
                            let fill =
                                bar.with_max_x(bar.left() + bar.width() * frac.clamp(0.0, 1.0));
                            ui.painter()
                                .rect_filled(fill, 2.0, Color32::from_rgb(0x7e, 0xc8, 0xe3));
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

fn methodology_severity_color(sev: MethodologySeverity) -> Color32 {
    match sev {
        MethodologySeverity::Error => Color32::from_rgb(0xe0, 0x50, 0x50),
        MethodologySeverity::CriticalWarning => Color32::from_rgb(0xf0, 0x80, 0x40),
        MethodologySeverity::Warning => Color32::from_rgb(0xf0, 0xc0, 0x40),
        MethodologySeverity::Advisory => Color32::from_rgb(0x90, 0x60, 0xc0),
    }
}

fn paint_methodology(ui: &mut egui::Ui, model: &mut IdeModel) {
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
        if paint_remaining_cta(ui, "No design yet.", "Run Synthesis") {
            queue_flow(FlowStep::Synthesis);
        }
        return;
    }
    let report = model.methodology_report();
    ui.label(format!(
        "checks={} errors={} critical={} warning={} advisory={}",
        report.checks.len(),
        report.error_count(),
        report.critical_count(),
        report.warning_count(),
        report.advisory_count()
    ));
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
    let col_w = chrome::stretched_col_w_gap(5, ui.available_width(), 8.0);
    egui::ScrollArea::both().show(ui, |ui| {
        egui::Grid::new("methodology_table")
            .spacing([8.0, row_gap])
            .min_col_width(col_w)
            .show(ui, |ui| {
                ui.label(RichText::new("ID").strong());
                ui.label(RichText::new("Severity").strong());
                ui.label(RichText::new("Category").strong());
                ui.label(RichText::new("Objects").strong());
                ui.label(RichText::new("Message").strong());
                ui.end_row();
                for v in &report.checks {
                    let on = selected_meth.as_deref() == Some(v.id.as_str());
                    let fill = methodology_severity_color(v.severity);
                    let btn = egui::Button::new(RichText::new(&v.id).color(Color32::BLACK))
                        .fill(fill)
                        .selected(on);
                    if ui.add(btn).clicked() {
                        pick = Some(v.id.clone());
                    }
                    ui.label(v.severity.as_str());
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

fn drc_severity_color(sev: DrcSeverity) -> Color32 {
    match sev {
        DrcSeverity::Error => Color32::from_rgb(0xe0, 0x50, 0x50),
        DrcSeverity::Warning => Color32::from_rgb(0xf0, 0xc0, 0x40),
        DrcSeverity::Advisory => Color32::from_rgb(0x90, 0x60, 0xc0),
    }
}

#[allow(dead_code)] // intentional: helper for WIP bitstream panel
fn bitstream_block_color(block: &str) -> Color32 {
    match block {
        "CLB_IO_CLK" => Color32::from_rgb(0x3d, 0xb8, 0x7a),
        "DSP" => Color32::from_rgb(0x90, 0x60, 0xc0),
        "BRAM" => Color32::from_rgb(0xf0, 0xc0, 0x40),
        "IOB" => Color32::from_rgb(0x5a, 0xb0, 0xe0),
        _ => Color32::from_rgb(0x9a, 0xa4, 0xae),
    }
}

#[allow(dead_code)] // intentional: WIP panel kept for upcoming canvas wiring
fn paint_bitstream(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Bitstream");
    ui.add_space(6.0);
    let report = model.bitstream_report();
    if report.frames == 0 && report.bytes == 0 {
        if paint_remaining_cta(ui, "No bitstream yet.", "Generate Bitstream") {
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
        if report.configured != 0 { "Configured" } else { "Not configured" },
        report.frames,
        report.bytes,
        report.idcode
    ));
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    egui::ScrollArea::both().max_height(280.0).show(ui, |ui| {
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
                    let fill = bitstream_block_color(row.block_name());
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

fn paint_drc(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("DRC");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Report DRC").clicked() {
            let _ = model.exec("report_drc");
        }
    });
    ui.add_space(6.0);
    if model.utilization.is_none() && model.drc.is_none() {
        if paint_remaining_cta(ui, "No DRC results yet.", "Report DRC") {
            let _ = model.exec("report_drc");
        }
        return;
    }
    let report = model.drc.clone().unwrap_or_else(|| model.drc_report());
    ui.label(format!(
        "{} violations · {} errors",
        report.violations.len(),
        report.error_count()
    ));
    let selected_drc = model.selected_drc.clone();
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    let mut pick_obj: Option<String> = None;
    let n = report.items.len().max(1);
    let remain = ui.available_size();
    let bbox = chrome::nv_table_bbox(n, remain.x.max(80.0), remain.y.max(120.0));
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(bbox.drawn_w.max(remain.x), bbox.drawn_h.max(remain.y)),
        Sense::hover(),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(8.0)), |ui| {
    let n_f = n as f32;
    let row_gap = ((ui.available_height() - 28.0) / n_f - 18.0).clamp(4.0, 28.0);
    let col_w = chrome::stretched_col_w_gap(4, ui.available_width(), 8.0);
    egui::ScrollArea::both().show(ui, |ui| {
        egui::Grid::new("drc_table")
            .spacing([8.0, row_gap])
            .min_col_width(col_w)
            .show(ui, |ui| {
                ui.label(RichText::new("ID").strong());
                ui.label(RichText::new("Severity").strong());
                ui.label(RichText::new("Objects").strong());
                ui.label(RichText::new("Message").strong());
                ui.end_row();
                if report.ok() {
                    ui.label("—");
                    ui.label("ok");
                    ui.label("-");
                    ui.label("No violations.");
                    ui.end_row();
                } else {
                    for v in &report.items {
                        let on = selected_drc.as_deref() == Some(v.id.as_str());
                        let fill = drc_severity_color(v.severity);
                        let btn = egui::Button::new(RichText::new(&v.id).color(Color32::BLACK))
                            .fill(fill)
                            .selected(on);
                        if ui.add(btn).clicked() {
                            pick = Some(v.id.clone());
                        }
                        ui.label(v.severity.as_str());
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
                }
            });
    });
    });
    if let Some(id) = pick_obj {
        let _ = model.select_drc(&id);
        let _ = model.select_drc_object(&id);
    } else if let Some(id) = pick {
        let _ = model.select_drc(&id);
    }
}

fn paint_utilization(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Utilization");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Report utilization").clicked() {
            let _ = model.exec("report_utilization");
        }
    });
    ui.add_space(6.0);
    let report = model.utilization_report();
    if report.part.is_empty() {
        if paint_remaining_cta(ui, "No placed design yet.", "Place") {
            queue_flow(FlowStep::Place);
        }
        return;
    }
    ui.label(format!("part={}", report.part));
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
    ui.painter()
        .rect_filled(rect, 0.0, Color32::from_rgb(0x1a, 0x1e, 0x24));
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
                            ui.label(row.used.to_string());
                            ui.label(row.available.to_string());
                            ui.label(format!("{}%", row.pct()));
                            let frac = if row.available == 0 {
                                0.0
                            } else {
                                row.used as f32 / row.available as f32
                            };
                            let (bar, _) = ui.allocate_exact_size(
                                egui::vec2(bar_span, bar_h),
                                Sense::hover(),
                            );
                            ui.painter()
                                .rect_filled(bar, 2.0, Color32::from_rgb(0x2b, 0x32, 0x3a));
                            let fill =
                                bar.with_max_x(bar.left() + bar.width() * frac.clamp(0.0, 1.0));
                            ui.painter()
                                .rect_filled(fill, 2.0, Color32::from_rgb(0x7e, 0xc8, 0xe3));
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
                                ui.label(h.lut.to_string());
                                ui.label(h.ff.to_string());
                                ui.label(h.iob.to_string());
                                ui.label(h.bram.to_string());
                                ui.label(h.dsp.to_string());
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

fn paint_schematic(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Schematic");
    // Stay on the schematic pane — do not fall back to Reports after More / Zoom.
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
    ui.horizontal(|ui| {
        if ui.button("Previous").clicked() {
            let _ = model.schematic_previous_view();
        }
        if ui.button("Next").clicked() {
            let _ = model.schematic_next_view();
        }
        if ui.button("Zoom In").clicked() {
            zoom_in = true;
        }
        if ui.button("Zoom Out").clicked() {
            zoom_out = true;
        }
        if ui.button("Zoom Fit").clicked() {
            zoom_fit = true;
        }
        if ui.button("Expand Cone").clicked() {
            let _ = model.exec("expand_cone");
        }
        if ui.button("Collapse Cone").clicked() {
            let _ = model.exec("collapse_cone");
        }
        if ui.button("Expand Inside").clicked() {
            let _ = model.exec("expand_inside");
        }
        if ui.button("Collapse Inside").clicked() {
            let _ = model.exec("collapse_inside");
        }
        // Fig. 55 sheet links: Cells / I/O Ports / Nets open Find Results.
        if ui.link(format!("{n_cells} Cells")).clicked() {
            let _ = model.exec("sheet_find cells");
        }
        if ui.link(format!("{n_ports} I/O Ports")).clicked() {
            let _ = model.exec("sheet_find ports");
        }
        if ui.link(format!("{n_nets} Nets")).clicked() {
            let _ = model.exec("sheet_find nets");
        }
        if let Some(root) = &model.schematic.cone_root {
            ui.label(RichText::new(format!("Cone {root}")).small().weak());
        }
        if let Some(inst) = &model.schematic.expand_inside {
            ui.label(RichText::new(format!("Inside {inst}")).small().weak());
        }
        ui.label(RichText::new(format!("Zoom {:.0}%", model.schematic.camera.zoom * 100.0)).small().weak());
    });
    // Full hint on hover. Visible words are complete — never "pinch or …".
    let pinch_hint = "pinch or ⌘-scroll to zoom · drag to pan";
    ui.add(
        egui::Label::new(RichText::new("pinch to zoom · drag to pan").small().weak()).wrap(),
    )
    .on_hover_text(pinch_hint);
    if !model.timing_paths.is_empty() {
        // Results strip belongs under the drawing (UG893). Keep it collapsed so Zoom Fit owns the pane.
        egui::CollapsingHeader::new("Timing paths")
            .default_open(false)
            .show(ui, |ui| {
                let mut pick_path = None;
                let selected_path = model.selected_timing_path;
                egui::ScrollArea::vertical()
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
                                    if ui.selectable_label(on, &p.name).clicked() {
                                        pick_path = Some(i);
                                    }
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
    // Canvas is the leftover pane — Zoom Fit fills this, not the heading/toolbar above.
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
        model.schematic.zoom_at(1.35, canvas.x * 0.5, canvas.y * 0.5);
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
                model.schematic.zoom_at(zoom_delta, vx, vy);
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
    let cam = model.schematic.camera;
    let z = cam.zoom.max(0.05);
    if ui.is_rect_visible(rect) {
        let p = ui.painter().with_clip_rect(rect);
        p.rect_filled(rect, 0.0, Color32::from_rgb(0x12, 0x16, 0x1a));
        let o = egui::pos2(rect.min.x + cam.pan_x, rect.min.y + cam.pan_y);
        let net_col = Color32::from_rgb(0x3d, 0xb8, 0x7a);
        let gold = Color32::from_rgb(0xe5, 0xc0, 0x7b);
        let path_col = Color32::from_rgb(0xe0, 0x6c, 0x75);
        let name_fs = (10.0 * z).clamp(8.0, 13.0);
        let pin_fs = 9.0 * z;
        let show_names = true;
        let show_pins = pin_fs >= 6.5;
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
                        Color32::from_rgb(0x7e, 0xc8, 0xe3),
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
                Color32::from_rgb(0x5c, 0x2e, 0x1e)
            } else if on {
                Color32::from_rgb(0x3d, 0x4a, 0x28)
            } else if port || sy.kind == "IOB_OUT" {
                Color32::from_rgb(0x1e, 0x3a, 0x55)
            } else {
                Color32::from_rgb(0x2a, 0x32, 0x24)
            };
            let stroke = Stroke::new(
                if on || sy.highlighted { 2.0_f32 } else { 1.0_f32 },
                if sy.highlighted {
                    path_col
                } else if on {
                    gold
                } else {
                    Color32::from_rgb(0x7a, 0x84, 0x8e)
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
                let anchor = if inward { r.left() + 6.0 } else { r.right() - 4.0 };
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
                    Color32::from_rgb(0x7e, 0xc8, 0xe3),
                );
                p.text(
                    egui::pos2(anchor, r.center().y + 8.0),
                    align,
                    &sy.name,
                    egui::FontId::monospace(fs),
                    Color32::from_rgb(0xdc, 0xe0, 0xe4),
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
                    Color32::from_rgb(0x7e, 0xc8, 0xe3),
                );
                clip.text(
                    egui::pos2(r.center().x, r.bottom() - 2.0 * z),
                    egui::Align2::CENTER_BOTTOM,
                    &sy.name,
                    egui::FontId::monospace(fs),
                    Color32::from_rgb(0xdc, 0xe0, 0xe4),
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
                let stub = Color32::from_rgb(0xdc, 0xe0, 0xe4);
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
                        if nc {
                            Color32::from_rgb(0x6a, 0x72, 0x78)
                        } else {
                            Color32::from_rgb(0x9a, 0xa4, 0xae)
                        },
                    );
                }
            }
        }
        if resp.clicked() || resp.double_clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
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
            }
        }
    }
    if let Some(id) = pick {
        model.select(&id);
    }
    if let Some(id) = expand {
        if model.schematic.is_instance(&id) {
            let _ = model.expand_inside(&id);
        } else {
            let _ = model.expand_cone(&id);
        }
    }
}


fn paint_device_legend(ui: &mut egui::Ui, model: &mut IdeModel, floor_scroll: bool) {
    let tools = |ui: &mut egui::Ui, model: &mut IdeModel| {
        ui.horizontal(|ui| {
            ui.set_min_width(360.0);
            if ui.button("Zoom In").clicked() {
                let _ = model.device_zoom_in();
            }
            if ui.button("Zoom Out").clicked() {
                let _ = model.device_zoom_out();
            }
            if ui.button("Zoom Fit").clicked() {
                let _ = model.device_zoom_fit();
            }
            ui.label(
                RichText::new(format!("Zoom {:.0}%", model.device_zoom * 100.0))
                    .small()
                    .weak(),
            );
        });
    };
    if floor_scroll {
        // One tool row. Narrow windows scroll it; wrapping would steal die height.
        egui::ScrollArea::horizontal()
            .id_salt("device_legend_tools")
            .auto_shrink([false, true])
            .max_height(36.0)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| tools(ui, model));
    } else {
        tools(ui, model);
    }
    // Complete chips only. Narrow: one row, scroll sideways so names stay fully visible.
    let chips: [(&str, Color32); 8] = [
        ("CLB", Color32::from_rgb(0x3d, 0xb8, 0x7a)),
        ("IOB", Color32::from_rgb(0x5b, 0x9b, 0xd5)),
        ("BRAM", Color32::from_rgb(0xb0, 0x7c, 0xe8)),
        ("placed LUT", Color32::from_rgb(0xc8, 0xf0, 0xd8)),
        ("placed I/O", Color32::from_rgb(0x7e, 0xc8, 0xe3)),
        ("clock region", Color32::from_rgb(0xb0, 0x7c, 0xe8)),
        ("route", Color32::from_rgb(0x3d, 0xb8, 0x7a)),
        ("pblock", Color32::from_rgb(0xe5, 0x9a, 0x3c)),
    ];
    if floor_scroll {
        egui::ScrollArea::horizontal()
            .id_salt("device_legend_chips")
            .auto_shrink([false, true])
            .max_height(chrome::DEVICE_LEGEND_ROW_H + 4.0)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    let mut used = 0.0_f32;
                    for (text, col) in chips {
                        used += text.chars().count() as f32 * 7.2 + 14.0;
                        ui.label(RichText::new(text).small().color(col));
                    }
                    ui.set_min_width(used);
                });
            });
        return;
    }
    let budget = ui
        .available_width()
        .min(ui.clip_rect().width())
        .max(96.0)
        - 12.0;
    let mut rows: Vec<Vec<(&str, Color32)>> = vec![Vec::new()];
    let mut used = 0.0_f32;
    for chip in chips {
        let w = chip.0.chars().count() as f32 * 7.2 + 14.0;
        if !rows.last().unwrap().is_empty() && used + w > budget {
            rows.push(Vec::new());
            used = 0.0;
        }
        rows.last_mut().unwrap().push(chip);
        used += w;
    }
    for row in rows {
        ui.horizontal(|ui| {
            for (text, col) in row {
                ui.label(RichText::new(text).small().color(col));
            }
        });
    }
}

fn paint_device(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Device");
    let screen = ui.ctx().screen_rect();
    let narrow = chrome::device_window_narrow(screen.width());
    let short = chrome::device_window_short(screen.height());
    paint_device_legend(ui, model, narrow || short);
    let after_legend = ui.available_height().max(1.0);
    let band = chrome::device_band_share(screen.width(), screen.height(), after_legend);
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
    let cols = model.device.cols.max(1);
    let rows = model.device.rows.max(1);
    let x0 = model.device.x0;
    let y0 = model.device.y0;
    let avail = ui.available_size();
    // Real leftover pane only — do not inflate past it (that jammed an oversized die).
    let view_h = avail.y.max(1.0);
    let view_w = avail.x.max(1.0);
    // Short leftover: Zoom Fit into this rect (zoom 1.0) so Y=0 is not a clipped 100% band.
    if model.device_zoom <= 1.001 {
        model.device_zoom = 1.0;
    }
    let (mut cell_w, mut cell_h) = chrome::floorplan_zoom_cells(cols, rows, view_w, view_h, model.device_zoom);
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
    let draw_w = if overflow { die_w.max(view_w + 8.0) } else { view_w.max(die_w) };
    let draw_h = if overflow { die_h.max(view_h + 8.0) } else { view_h.max(die_h) };
    let origin_dx = if model.device_zoom < 0.999 {
        (view_w - die_w).max(0.0) * 0.5
    } else {
        0.0
    };
    let origin_dy = if model.device_zoom < 0.999 {
        (view_h - die_h).max(0.0) * 0.5
    } else {
        0.0
    };
    let mut pick_site: Option<(u32, u32)> = None;
    let mut pick_region: Option<String> = None;
    let mut click_pblock: Option<String> = None;
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
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(draw_w, draw_h),
                Sense::click(),
            );
            if ui.is_rect_visible(rect) {
                let origin = egui::pos2(
                    rect.left() + chrome::FLOORPLAN_LEFT + origin_dx,
                    rect.top() + chrome::FLOORPLAN_TOP + origin_dy,
                );
                let p = ui.painter();
                p.rect_filled(rect, 0.0, Color32::from_rgb(0x12, 0x16, 0x1a));
                for dx in 0..cols {
                    let x = x0 + dx;
                    let px = origin.x + dx as f32 * cell_w;
                    if dx % 4 == 0 {
                        p.text(
                            egui::pos2(px + 1.0, rect.bottom() - 12.0),
                            egui::Align2::LEFT_BOTTOM,
                            format!("{x}"),
                            egui::FontId::monospace(8.0),
                            Color32::from_rgb(0x7a, 0x84, 0x8e),
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
                            Color32::from_rgb(0x7a, 0x84, 0x8e),
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
                        let fill = match site {
                            Some(s) if s.occupant.is_some() => match s.occupancy_char() {
                                'O' => Color32::from_rgb(0x7e, 0xc8, 0xe3),
                                'L' | 'C' => Color32::from_rgb(0x3d, 0xb8, 0x7a),
                                _ => Color32::from_rgb(0xe5, 0xc0, 0x7b),
                            },
                            Some(s) => match s.kind {
                                helion_device::SiteKind::Iob => Color32::from_rgb(0x1e, 0x3a, 0x55),
                                helion_device::SiteKind::Bram => Color32::from_rgb(0x3a, 0x24, 0x52),
                                helion_device::SiteKind::Dsp => Color32::from_rgb(0x52, 0x3a, 0x1e),
                                helion_device::SiteKind::Clk => Color32::from_rgb(0x3a, 0x3a, 0x1e),
                                helion_device::SiteKind::Clb => Color32::from_rgb(0x1a, 0x2e, 0x24),
                            },
                            None => Color32::from_rgb(0x0d, 0x10, 0x12),
                        };
                        p.rect_filled(tile, 1.0, fill);
                        let selected = site.is_some_and(|s| {
                            let id = model.selected.as_deref();
                            id == s.occupant.as_deref()
                                || id == Some(s.site_name().as_str())
                                || s.bels.iter().any(|b| Some(b.as_str()) == id)
                        });
                        if selected {
                            p.rect_stroke(
                                tile,
                                1.0,
                                Stroke::new(1.5_f32, Color32::from_rgb(0xe5, 0xc0, 0x7b)),
                                egui::StrokeKind::Outside,
                            );
                        }
                    }
                }
                // Fig. 49: clock-region outlines over the die.
                let purple = Color32::from_rgb(0xb0, 0x7c, 0xe8);
                let gold = Color32::from_rgb(0xe5, 0xc0, 0x7b);
                let amber = Color32::from_rgb(0xe5, 0x9a, 0x3c);
                for cr in &model.device.clock_regions {
                    let px = origin.x + (cr.x0 - x0) as f32 * cell_w;
                    let py = origin.y + (rows - 1 - (cr.y1 - y0)) as f32 * cell_h;
                    let pw = cr.cols() as f32 * cell_w;
                    let ph = cr.rows() as f32 * cell_h;
                    let rr = egui::Rect::from_min_size(egui::pos2(px, py), egui::vec2(pw, ph));
                    let on = model.selected.as_deref() == Some(cr.name.as_str());
                    p.rect_stroke(
                        rr,
                        0.0,
                        Stroke::new(if on { 3.0_f32 } else { 2.0_f32 }, if on { gold } else { purple }),
                        egui::StrokeKind::Inside,
                    );
                    p.text(
                        egui::pos2(rr.left() + 3.0, rr.top() + 2.0),
                        egui::Align2::LEFT_TOP,
                        &cr.name,
                        egui::FontId::monospace(9.0),
                        if on { gold } else { purple },
                    );
                }
                // Pblock rectangles (create_pblock / resize_pblock).
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
                    p.rect_stroke(
                        rr,
                        0.0,
                        Stroke::new(if on { 3.0_f32 } else { 2.0_f32 }, if on { gold } else { amber }),
                        egui::StrokeKind::Inside,
                    );
                    p.text(
                        egui::pos2(rr.left() + 3.0, rr.top() + 2.0),
                        egui::Align2::LEFT_TOP,
                        &pb.name,
                        egui::FontId::monospace(9.0),
                        if on { gold } else { amber },
                    );
                }
                // PathFinder IOB nets over the die.
                let route_col = Color32::from_rgb(0x3d, 0xb8, 0x7a);
                let unroute_col = Color32::from_rgb(0x5a, 0x64, 0x6e);
                for rt in &model.device.routes {
                    if rt.tiles.len() < 2 {
                        continue;
                    }
                    let col = if rt.highlighted {
                        gold
                    } else if rt.hops == 0 {
                        unroute_col
                    } else {
                        route_col
                    };
                    let thick = if rt.highlighted { 2.6_f32 } else { 1.7_f32 };
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
                            if s.bels.is_empty() {
                                tip.push_str(&s.site_name());
                            } else {
                                tip.push_str(&format!("{}  {}", s.site_name(), s.bels.join(",")));
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
            }
            if resp.clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let origin = egui::pos2(rect.left() + chrome::FLOORPLAN_LEFT + origin_dx, rect.top() + chrome::FLOORPLAN_TOP + origin_dy);
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
                            pick_site = Some((x, y));
                        }
                    }
                }
            }
    });
    if let Some(name) = click_pblock {
        let _ = model.select_pblock(&name);
    }
    if let Some(name) = pick_region {
        let _ = model.select_clock_region(&name);
    }
    if let Some((x, y)) = pick_site {
        let _ = model.select_device_site(&format!("X{x}Y{y}"));
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
    let pane_w = ui.clip_rect().width().min(ui.max_rect().width()).min(ui.available_width());
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
                            egui::SelectableLabel::new(on, line),
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
                if ui.selectable_label(on, cr.x0.to_string()).clicked() {
                    pick = Some(i.to_string());
                }
                if ui.selectable_label(on, cr.y0.to_string()).clicked() {
                    pick = Some(i.to_string());
                }
                if ui.selectable_label(on, cr.x1.to_string()).clicked() {
                    pick = Some(i.to_string());
                }
                if ui.selectable_label(on, cr.y1.to_string()).clicked() {
                    pick = Some(i.to_string());
                }
                if ui.selectable_label(on, sites.to_string()).clicked() {
                    pick = Some(i.to_string());
                }
                if ui.selectable_label(on, occ.to_string()).clicked() {
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

fn paint_device_routes(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.label(RichText::new("Device Routing").strong());
    let routes = model.device.routes.clone();
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    if routes.is_empty() {
        ui.label("No routes yet.");
        if primary_button(ui, "Route").clicked() {
            queue_flow(FlowStep::Route);
        }
        return;
    }
    data_scroll("ug893_device_routes_scroll").show(ui, |ui| {
    egui::Grid::new("ug893_device_routes")
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(RichText::new("Net").strong());
            ui.label(RichText::new("Hops").strong());
            ui.label(RichText::new("Delay_ps").strong());
            ui.label(RichText::new("Tiles").strong());
            ui.end_row();
            for (i, r) in routes.iter().enumerate() {
                let on = selected.as_deref() == Some(r.net.as_str()) || r.highlighted;
                if ui.selectable_label(on, &r.net).clicked() {
                    pick = Some(i.to_string());
                }
                if ui.selectable_label(on, r.hops.to_string()).clicked() {
                    pick = Some(i.to_string());
                }
                if ui.selectable_label(on, r.delay_ps.to_string()).clicked() {
                    pick = Some(i.to_string());
                }
                if ui.selectable_label(on, r.tiles.len().to_string()).clicked() {
                    pick = Some(i.to_string());
                }
                ui.end_row();
            }
        });
    });
    if let Some(spec) = pick {
        let _ = model.select_device_route(&spec);
    }
}

fn paint_source(ui: &mut egui::Ui, model: &mut IdeModel) {
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
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("No sources yet.");
                        ui.end_row();
                    } else {
                        for r in &rows {
                            let on = selected == Some(r.line);
                            let bp_on = armed.contains(&r.line);
                            let bp_label = if r.executable {
                                r.bp_cell(bp_on)
                            } else {
                                " "
                            };
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
                            if ui
                                .selectable_label(on, RichText::new(r.text.trim()).monospace())
                                .clicked()
                            {
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

fn paint_text_editor(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Text Editor");
    ui.horizontal(|ui| {
        if ui.button("Open").clicked() {
            let _ = model.open_text_editor();
        }
        if ui.button("Goto Source").clicked() {
            let _ = model.goto_editor("");
        }
    });
    let selected = model.selected_source_line;
    let markers = model.editor_markers();
    let n = model.source_line_rows().len();
    let mut pick_line: Option<String> = None;
    let mut pick_mark: Option<String> = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Marker").strong());
        ui.add_space(12.0);
        ui.label(RichText::new("Line").strong());
        ui.add_space(12.0);
        ui.label(RichText::new("Kind").strong());
        ui.add_space(12.0);
        ui.label(RichText::new("Text").strong());
    });
    let row_h = 22.0;
    egui::ScrollArea::both()
        .id_salt("ug893_text_editor")
        .show_rows(ui, row_h, n.max(1), |ui, range| {
            if n == 0 {
                ui.label("No sources yet.");
                return;
            }
            let rows = model.source_line_rows();
            let end = range.end.min(n);
            for r in &rows[range.start..end] {
                ui.horizontal(|ui| {
                    ui.set_min_height(row_h);
                    let on = selected == Some(r.line);
                    let mk = markers
                        .iter()
                        .filter(|m| m.line == r.line)
                        .min_by_key(|m| match m.kind.as_str() {
                            "error" => 0u8,
                            "warning" => 1,
                            "advisory" => 2,
                            "probe" => 3,
                            "bookmark" => 4,
                            _ => 9,
                        });
                    let mark = mk.map(|m| m.marker_cell()).unwrap_or("-");
                    if ui.selectable_label(on && mk.is_some(), mark).clicked() {
                        if mk.map(|m| m.kind.as_str()) == Some("bookmark") || mk.is_none() {
                            pick_mark = Some(r.line.to_string());
                        } else {
                            pick_line = Some(r.line.to_string());
                        }
                    }
                    if ui
                        .selectable_label(on, RichText::new(r.line.to_string()).monospace())
                        .clicked()
                    {
                        pick_line = Some(r.line.to_string());
                    }
                    if ui.selectable_label(on, r.type_cell()).clicked() {
                        pick_line = Some(r.line.to_string());
                    }
                    if ui
                        .selectable_label(on, RichText::new(r.text.trim()).monospace())
                        .clicked()
                    {
                        pick_line = Some(r.line.to_string());
                    }
                });
            }
        });
    if let Some(spec) = pick_mark {
        let _ = model.toggle_editor_bookmark(&spec);
    }
    if let Some(spec) = pick_line {
        let _ = model.select_editor_line(&spec);
    }
}

fn paint_memory(ui: &mut egui::Ui, model: &mut IdeModel) {
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
                if paint_clipped_select(ui, on, &shown, &m.name) {
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
                    if paint_clipped_select(ui, on, &shown, &r.data) {
                        pick_addr = Some(r.addr);
                    }
                }
            });
    }
    if let Some(addr) = pick_addr {
        let _ = model.select_memory_word(&addr.to_string());
    }
}

fn paint_breakpoints(ui: &mut egui::Ui, model: &mut IdeModel) {
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
        if ui.button("Disable").clicked() {
            let _ = model.set_breakpoint_enabled("", false);
        }
        if ui.button("Enable").clicked() {
            let _ = model.set_breakpoint_enabled("", true);
        }
        if ui.button("Delete").clicked() {
            let _ = model.delete_breakpoint("");
        }
    });
    let rows = model.breakpoint_rows().to_vec();
    let selected = model.selected_breakpoint;
    let mut pick: Option<String> = None;
    data_scroll("ug900_breakpoints")
        .show(ui, |ui| {
            egui::Grid::new("ug900_breakpoints_table")
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Id").strong());
                    ui.label(RichText::new("Enabled").strong());
                    ui.label(RichText::new("Signal").strong());
                    ui.label(RichText::new("Condition").strong());
                    ui.label(RichText::new("Hits").strong());
                    ui.end_row();
                    if rows.is_empty() {
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("No breakpoints yet. Set one from the Source view.");
                        ui.end_row();
                    } else {
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

fn paint_forces(ui: &mut egui::Ui, model: &mut IdeModel) {
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
        if ui.button("Remove").clicked() {
            let _ = model.remove_force("");
        }
    });
    let rows = model.force_rows().to_vec();
    let selected = model.selected_force.clone();
    let mut pick: Option<String> = None;
    data_scroll("ug900_forces")
        .show(ui, |ui| {
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
                    if rows.is_empty() {
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("No force constants yet.");
                        ui.end_row();
                    } else {
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
    data_scroll("ug900_locals_ws")
        .show(ui, |ui| {
            egui::Grid::new("ug900_locals_ws_table")
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Name").strong());
                    ui.label(RichText::new("Type").strong());
                    ui.label(RichText::new("Value").strong());
                    ui.label(RichText::new("Scope").strong());
                    ui.end_row();
                    if rows.is_empty() {
                        ui.label("—");
                        ui.label("—");
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

fn paint_wave(ui: &mut egui::Ui, model: &mut IdeModel) {
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
    });
    ui.label(RichText::new("click cursor · Shift A · Alt B").small().weak());
    paint_wave_markers(ui, model);
    paint_wave_cursors(ui, model);
    paint_virtual_buses(ui, model);
    if model.wave.traces.is_empty() {
        ui.label("No waveform yet.");
        if primary_button(ui, "Run Simulation").clicked() {
            surface::request_job(JobKind::SimRun(model.sim_runtime_cycles.max(1)));
        }
        // Own the Wave pane — no thick empty black slab beside scopes.
        let fill = ui.available_size().max(egui::vec2(120.0, 160.0));
        let (rect, _) = ui.allocate_exact_size(fill, Sense::hover());
        ui.painter().rect_filled(
            rect,
            4.0,
            Color32::from_rgb(0x1a, 0x1e, 0x24),
        );
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.0_f32, Color32::from_rgb(0x3a, 0x42, 0x4a)),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Wave · run simulation to fill",
            egui::FontId::proportional(14.0),
            Color32::from_rgb(0xa0, 0xa8, 0xb0),
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
            ui.label(RichText::new("A").strong().monospace().color(Color32::from_rgb(0xe0, 0x6c, 0x75)));
        }
        if model.wave.cursor_b.is_some() {
            ui.add_space(12.0);
            ui.label(RichText::new("B").strong().monospace().color(Color32::from_rgb(0x56, 0xb6, 0xc2)));
        }
        ui.add_space(40.0);
        ui.label(RichText::new("Waveform").strong().monospace());
    });
    // Timescale ruler
    let ruler_h = 18.0;
    let (ruler, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ruler_h),
        Sense::click(),
    );
    if ui.is_rect_visible(ruler) {
        let p = ui.painter();
        p.rect_filled(ruler, 0.0, Color32::from_rgb(0x1a, 0x1e, 0x22));
        let wave_x0 = ruler.left() + 220.0;
        let wave_w = (ruler.right() - wave_x0).max(8.0);
        for i in 0..=n.min(32) {
            let x = wave_x0 + wave_w * (i as f32) / (n as f32);
            p.line_segment(
                [egui::pos2(x, ruler.top() + 10.0), egui::pos2(x, ruler.bottom())],
                Stroke::new(1.0_f32, Color32::from_rgb(0x5a, 0x64, 0x6e)),
            );
            if i % 2 == 0 {
                p.text(
                    egui::pos2(x + 2.0, ruler.top()),
                    egui::Align2::LEFT_TOP,
                    format!("{}", model.wave.time_ps(i) / 1000),
                    egui::FontId::monospace(10.0),
                    Color32::from_rgb(0x9a, 0xa4, 0xae),
                );
            }
        }
        for m in &model.wave.markers {
            let x = wave_x0 + wave_w * (m.sample as f32 + 0.5) / (n as f32);
            p.line_segment(
                [egui::pos2(x, ruler.top()), egui::pos2(x, ruler.bottom())],
                Stroke::new(1.2_f32, Color32::from_rgb(0xc0, 0x78, 0xc8)),
            );
            p.text(
                egui::pos2(x + 2.0, ruler.top()),
                egui::Align2::LEFT_TOP,
                &m.name,
                egui::FontId::monospace(9.0),
                Color32::from_rgb(0xd8, 0xa0, 0xe0),
            );
        }
        if let (Some(a), Some(b)) = (model.wave.cursor_a, model.wave.cursor_b) {
            let xa = wave_x0 + wave_w * (a as f32 + 0.5) / (n as f32);
            let xb = wave_x0 + wave_w * (b as f32 + 0.5) / (n as f32);
            let (l, r) = if xa <= xb { (xa, xb) } else { (xb, xa) };
            p.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(l, ruler.top()),
                    egui::pos2(r, ruler.bottom()),
                ),
                0.0,
                Color32::from_rgba_unmultiplied(0x56, 0xb6, 0xc2, 40),
            );
        }
        if let Some(a) = model.wave.cursor_a {
            let x = wave_x0 + wave_w * (a as f32 + 0.5) / (n as f32);
            p.line_segment(
                [egui::pos2(x, ruler.top()), egui::pos2(x, ruler.bottom())],
                Stroke::new(1.4_f32, Color32::from_rgb(0xe0, 0x6c, 0x75)),
            );
            p.text(
                egui::pos2(x + 2.0, ruler.top()),
                egui::Align2::LEFT_TOP,
                "A",
                egui::FontId::monospace(9.0),
                Color32::from_rgb(0xe0, 0x6c, 0x75),
            );
        }
        if let Some(b) = model.wave.cursor_b {
            let x = wave_x0 + wave_w * (b as f32 + 0.5) / (n as f32);
            p.line_segment(
                [egui::pos2(x, ruler.top()), egui::pos2(x, ruler.bottom())],
                Stroke::new(1.4_f32, Color32::from_rgb(0x56, 0xb6, 0xc2)),
            );
            p.text(
                egui::pos2(x + 2.0, ruler.top()),
                egui::Align2::LEFT_TOP,
                "B",
                egui::FontId::monospace(9.0),
                Color32::from_rgb(0x56, 0xb6, 0xc2),
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
    let row_h = chrome::wave_trace_row_h(model.wave.traces.len(), ui.available_height() - 8.0);

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
                        .color(Color32::from_rgb(0xc8, 0xf0, 0xd8)),
                ),
            );
            if let Some(a) = cursor_a {
                ui.add_sized(
                    [56.0, 28.0],
                    egui::Label::new(
                        RichText::new(t.value_at(a))
                            .monospace()
                            .color(Color32::from_rgb(0xe0, 0x6c, 0x75)),
                    ),
                );
            }
            if let Some(b) = cursor_b {
                ui.add_sized(
                    [56.0, 28.0],
                    egui::Label::new(
                        RichText::new(t.value_at(b))
                            .monospace()
                            .color(Color32::from_rgb(0x56, 0xb6, 0xc2)),
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

fn paint_wave_markers(ui: &mut egui::Ui, model: &mut IdeModel) {
    let markers = model.wave.markers.clone();
    let selected = model.selected_wave_marker.clone();
    let mut pick: Option<String> = None;
    if markers.is_empty() {
        // Toolbar already has Add marker — don't steal pane height for an empty table.
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
) {
    let p = ui.painter();
    p.rect_filled(rect, 0.0, Color32::from_rgb(0x0d, 0x10, 0x12));
    let green = Color32::from_rgb(0x3d, 0xb8, 0x7a);
    let dim = Color32::from_rgb(0x2a, 0x6a, 0x48);
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
                        egui::Rect::from_min_max(
                            egui::pos2(x0, y1),
                            egui::pos2(x1, y0),
                        ),
                        0.0,
                        Color32::from_rgba_unmultiplied(0x3d, 0xb8, 0x7a, 28),
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
            egui::Rect::from_min_max(
                egui::pos2(l, rect.top()),
                egui::pos2(r, rect.bottom()),
            ),
            0.0,
            Color32::from_rgba_unmultiplied(0x56, 0xb6, 0xc2, 28),
        );
    }
    let cx = rect.left() + dx * (cursor as f32 + 0.5);
    p.line_segment(
        [egui::pos2(cx, rect.top()), egui::pos2(cx, rect.bottom())],
        Stroke::new(1.0_f32, Color32::from_rgb(0xe5, 0xc0, 0x7b)),
    );
    if let Some(a) = cursor_a {
        let ax = rect.left() + dx * (a as f32 + 0.5);
        p.line_segment(
            [egui::pos2(ax, rect.top()), egui::pos2(ax, rect.bottom())],
            Stroke::new(1.2_f32, Color32::from_rgb(0xe0, 0x6c, 0x75)),
        );
    }
    if let Some(b) = cursor_b {
        let bx = rect.left() + dx * (b as f32 + 0.5);
        p.line_segment(
            [egui::pos2(bx, rect.top()), egui::pos2(bx, rect.bottom())],
            Stroke::new(1.2_f32, Color32::from_rgb(0x56, 0xb6, 0xc2)),
        );
    }
    for m in markers {
        let mx = rect.left() + dx * (m.sample as f32 + 0.5);
        p.line_segment(
            [egui::pos2(mx, rect.top()), egui::pos2(mx, rect.bottom())],
            Stroke::new(1.0_f32, Color32::from_rgb(0xc0, 0x78, 0xc8)),
        );
    }
    let _ = ns;
}

fn hw_stat_bit_color(name: &str, value: bool) -> Color32 {
    if !value {
        return Color32::from_rgb(0x6a, 0x74, 0x7e);
    }
    match name {
        "CRC_ERR" => Color32::from_rgb(0xe0, 0x50, 0x50),
        "GTS" | "GSR" => Color32::from_rgb(0xf0, 0xc0, 0x40),
        _ => Color32::from_rgb(0x3d, 0xb8, 0x7a),
    }
}

fn paint_hw(ui: &mut egui::Ui, app: &mut HelionIde) {
    ui.heading("Hardware Manager");
    let det = app.boards(false).clone();
    let model = &mut app.model;
    if !det.physical_had {
        ui.label(
            RichText::new(
                "Physical board soft-hold — no USB programmer detected. Use sim cable or attach FTDI/HAD.",
            )
            .color(Color32::from_rgb(0xe0, 0xa0, 0x40))
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
            .color(Color32::from_rgb(0xa0, 0xa8, 0xb0)),
        );
    } else {
        ui.label(
            RichText::new("Physical USB programmer detected (detect only — not board DONE).")
                .color(Color32::from_rgb(0x3d, 0xb8, 0x7a))
                .small(),
        );
    }
    ui.horizontal(|ui| {
        if ui.button("Open Hardware Manager").clicked() {
            let _ = model.exec("open_hw_manager");
        }
        if ui.button("Program Device (sim)").clicked() {
            let _ = model.exec("program_hw");
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
    let (dash, dash_resp) = ui.allocate_exact_size(egui::vec2(remain.x.max(80.0), tile_band), Sense::click());
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
                        hw_stat_bit_color(&b.name, b.value),
                    )
                })
                .collect()
        } else {
            vec![
                ("Sim cable".into(), true, Color32::from_rgb(0x3d, 0xb8, 0x7a)),
                ("USB / OFL".into(), det.physical_had, Color32::from_rgb(0xe0, 0xa0, 0x40)),
                ("Native FTDI".into(), false, Color32::from_rgb(0x5a, 0x64, 0x6e)),
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
                        Color32::from_rgb(0xe5, 0xc0, 0x7b)
                    } else {
                        Color32::from_rgb(0x3a, 0x42, 0x4a)
                    },
                ),
                egui::StrokeKind::Inside,
            );
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("{} {}", name, if *on { "1" } else { "0" }),
                egui::FontId::proportional(13.0),
                Color32::from_rgb(0x12, 0x14, 0x18),
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
    let (ila_rect, _) = ui.allocate_exact_size(egui::vec2(remain.x.max(80.0), ila_h.max(80.0)), Sense::hover());
    ui.scope_builder(egui::UiBuilder::new().max_rect(ila_rect.shrink(4.0)), |ui| {
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
        // not a hardcoded cnt_3 dump — capture is fabric-backed via helion-debug.
        let probe = model.default_ila_probe();
        if ui
            .button(format!("Arm / Capture {probe}"))
            .on_hover_text("insert_arm_capture → fabric ble_q readback")
            .clicked()
        {
            let _ = model.exec(&format!("ila_arm {probe}"));
        }
        if ui.button("Capture cnt_3").on_hover_text("counter MSB probe").clicked() {
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
            p.rect_filled(cap, 4.0, Color32::from_rgb(0x16, 0x1c, 0x22));
            p.rect_stroke(
                cap,
                4.0,
                Stroke::new(1.0_f32, Color32::from_rgb(0x3a, 0x42, 0x4a)),
                egui::StrokeKind::Inside,
            );
            let lanes = 6;
            for i in 0..lanes {
                let y = cap.top() + cap.height() * (i as f32 + 0.5) / lanes as f32;
                p.line_segment(
                    [egui::pos2(cap.left() + 12.0, y), egui::pos2(cap.right() - 12.0, y)],
                    Stroke::new(1.0_f32, Color32::from_rgb(0x2a, 0x32, 0x3a)),
                );
            }
            p.text(
                cap.center(),
                egui::Align2::CENTER_CENTER,
                "ILA capture · arm a probe to fill",
                egui::FontId::proportional(14.0),
                Color32::from_rgb(0xa0, 0xa8, 0xb0),
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
        egui::ScrollArea::both().max_height(220.0).show(ui, |ui| {
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
    });
}

#[allow(dead_code)] // intentional: WIP panel kept for upcoming canvas wiring
fn paint_ip(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("IP Integrator");
    ui.weak("Helion-MM block design canvas — IP boxes and interface wires (not AXI, not a catalog dump)");
    ui.horizontal(|ui| {
        if ui.button("Refresh catalog").clicked() {
            let _ = model.exec("ip_catalog");
        }
        if ui.button("Create Block Design").clicked() {
            let _ = model.exec("create_bd");
        }
        if ui.button("Generate Output Products").clicked() {
            let spec = model.selected_ip.clone().unwrap_or_default();
            let _ = model.exec(&format!("generate_ip {spec}"));
        }
        if ui.button("Add to Block Design").clicked() {
            let spec = model.selected_ip.clone().unwrap_or_default();
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
            model.block_design.as_ref().map(|b| b.name.as_str()).unwrap_or("-"),
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
        egui::vec2(canvas.drawn_w.max(remain.x), canvas.drawn_h.max(chrome::DRAWING_MIN_HEIGHT * 0.5)),
        Sense::hover(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }
    let p = ui.painter();
    p.rect_filled(rect, 0.0, Color32::from_rgb(0x12, 0x16, 0x1a));
    let Some(drawing) = drawing else {
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Create Block Design to place Helion-MM IP on the canvas.",
            egui::FontId::proportional(14.0),
            Color32::from_rgb(0x9a, 0xa4, 0xae),
        );
        return;
    };
    let fit = chrome::hierarchy_fit(drawing.width, drawing.height, rect.width(), rect.height());
    let o = rect.min;
    let sx = fit.scale_x;
    let sy = fit.scale_y;
    let net = Color32::from_rgb(0x3d, 0xb8, 0x7a);
    let mm = Color32::from_rgb(0x7e, 0xc8, 0xe3);
    for w in &drawing.wires {
        let col = if w.net == "Helion-MM" { mm } else { net };
        let thick = if w.net == "Helion-MM" { 3.4_f32 } else { 1.6_f32 };
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
                Color32::from_rgb(0x1e, 0x3a, 0x55),
                Stroke::new(1.0_f32, Color32::from_rgb(0x7a, 0x84, 0x8e)),
            ));
        } else {
            let fill = if syb.kind == "INTERCONNECT" {
                Color32::from_rgb(0x24, 0x2e, 0x3a)
            } else {
                Color32::from_rgb(0x2a, 0x32, 0x24)
            };
            p.rect_filled(r, 3.0, fill);
            p.rect_stroke(
                r,
                3.0,
                Stroke::new(1.0_f32, Color32::from_rgb(0x7a, 0x84, 0x8e)),
                egui::StrokeKind::Inside,
            );
        }
        p.text(
            egui::pos2(r.center().x, r.top() + 4.0),
            egui::Align2::CENTER_TOP,
            &syb.kind,
            egui::FontId::monospace(10.0),
            Color32::from_rgb(0x7e, 0xc8, 0xe3),
        );
        p.text(
            egui::pos2(r.center().x, r.bottom() - 4.0),
            egui::Align2::CENTER_BOTTOM,
            &syb.name,
            egui::FontId::monospace(10.0),
            Color32::from_rgb(0xdc, 0xe0, 0xe4),
        );
        if !syb.bus.is_empty() {
            p.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                &syb.bus,
                egui::FontId::monospace(9.0),
                Color32::from_rgb(0x9a, 0xa4, 0xae),
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
                    Stroke::new(1.0_f32, Color32::from_rgb(0xdc, 0xe0, 0xe4)),
                    egui::StrokeKind::Outside,
                );
                p.line_segment([edge, tip], Stroke::new(3.4_f32, mm));
            } else {
                p.line_segment([edge, tip], Stroke::new(1.6_f32, net));
                p.circle_filled(tip, 2.0, Color32::from_rgb(0xdc, 0xe0, 0xe4));
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
                Color32::from_rgb(0x9a, 0xa4, 0xae),
            );
        }
    }
}

#[allow(dead_code)] // intentional: WIP panel kept for upcoming canvas wiring
fn paint_ip_catalog(ui: &mut egui::Ui, model: &mut IdeModel) {
    let rows = model.ip_catalog_rows();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("IP Catalog").strong());
        ui.label(RichText::new(format!("cores={}", rows.len())).small().weak());
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
                    egui::SelectableLabel::new(on, &r.name),
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

#[allow(dead_code)] // intentional: WIP panel kept for upcoming canvas wiring
fn paint_bd_hdl(ui: &mut egui::Ui, model: &mut IdeModel) {
    let rows = model.bd_hdl_rows();
    if rows.is_empty() {
        return;
    }
    ui.add_space(6.0);
    ui.label(
        RichText::new("Generated HDL — instance table from helion-bd emit_sv, not a source dump")
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
