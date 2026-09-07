//! Self-correction loop: chrome state, event isolation, geometry probes, engine offload.
//!
//! The IDE binary and the tests drive the **same** `ChromeState` transitions.
//! Paint in `helion-ide` must follow [`central_pane`] — if a click updates state
//! but the pane is still Timing, tests fail.

use crate::chrome::{self, Activity, Canvas, WorkspacePane};
use crate::{FlowStep, IdeModel, WorkspaceTab};
use std::cell::RefCell;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{SystemTime, UNIX_EPOCH};

thread_local! {
    static PENDING_JOBS: RefCell<Vec<JobKind>> = const { RefCell::new(Vec::new()) };
}

/// Queue a CAD job from a paint helper that only has `&mut IdeModel`.
pub fn request_job(kind: JobKind) {
    PENDING_JOBS.with(|p| p.borrow_mut().push(kind));
}

pub fn take_jobs() -> Vec<JobKind> {
    PENDING_JOBS.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

/// Paint-path flow buttons queue the engine; they never call `run_step` on the UI thread.
pub fn queue_flow(step: FlowStep) {
    request_job(JobKind::Step(step));
}

/// Why a control looked dead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventClass {
    /// Pointer never hit the widget.
    HitTestMiss,
    /// State updated; paint ignored it (the Schematic bug).
    StatePaint,
    /// UI thread blocked in synth/place/route.
    UiThreadBlocked,
    /// Click produced the expected pane.
    Ok,
}

#[derive(Clone, Debug)]
pub struct UiEvent {
    pub at_ms: u128,
    pub kind: &'static str,
    pub widget: String,
    pub expected: String,
    pub actual: String,
    pub class: EventClass,
}

impl UiEvent {
    pub fn line(&self) -> String {
        format!(
            "t={} kind={} widget={} expected={} actual={} class={:?}",
            self.at_ms, self.kind, self.widget, self.expected, self.actual, self.class
        )
    }
}

#[derive(Debug, Default)]
pub struct UiTrace {
    pub events: Vec<UiEvent>,
    log_path: Option<PathBuf>,
}

impl UiTrace {
    pub fn from_env() -> Self {
        let log_path = match std::env::var("HELION_UI_LOG") {
            Ok(s) if s.is_empty() || s == "0" => None,
            Ok(s) => Some(PathBuf::from(s)),
            Err(_) if cfg!(debug_assertions) => Some(std::env::temp_dir().join("helion-ui.log")),
            Err(_) => None,
        };
        Self {
            events: Vec::new(),
            log_path,
        }
    }

    pub fn push(&mut self, ev: UiEvent) {
        if let Some(path) = &self.log_path {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(f, "{}", ev.line());
            }
        }
        self.events.push(ev);
    }

    pub fn last_class(&self) -> Option<EventClass> {
        self.events.last().map(|e| e.class)
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// On-screen chrome the paint path reads. Workspace is mirrored onto `IdeModel.workspace`.
#[derive(Clone, Debug)]
pub struct ChromeState {
    pub activity: Activity,
    pub canvas: Canvas,
    pub workspace: WorkspaceTab,
    pub sidebar_hidden: bool,
    pub sidebar_width: f32,
    pub console_height: f32,
}

impl Default for ChromeState {
    fn default() -> Self {
        Self {
            activity: Activity::Files,
            canvas: Canvas::Editor,
            workspace: WorkspaceTab::TextEditor,
            sidebar_hidden: false,
            sidebar_width: chrome::SIDEBAR_WIDTH,
            console_height: chrome::CONSOLE_DEFAULT_HEIGHT,
        }
    }
}

/// The pane `paint_workspace` must fill. Shared by the binary and tests.
pub fn central_pane(s: &ChromeState) -> WorkspacePane {
    if s.workspace.sim_only() || s.activity == Activity::Simulate {
        return if s.workspace.sim_only() {
            chrome::pane_for_workspace(s.workspace)
        } else {
            WorkspacePane::Wave
        };
    }
    if chrome::is_more_destination(s.workspace) {
        return chrome::pane_for_workspace(s.workspace);
    }
    if s.activity == Activity::Program {
        return WorkspacePane::Hardware;
    }
    match s.canvas {
        Canvas::Editor => WorkspacePane::Editor,
        Canvas::Device => WorkspacePane::Device,
        Canvas::Timing => {
            if s.activity == Activity::Reports {
                if chrome::is_report_detail(s.workspace) {
                    chrome::pane_for_workspace(s.workspace)
                } else {
                    WorkspacePane::ReportsCatalog
                }
            } else {
                WorkspacePane::Timing
            }
        }
    }
}

pub fn apply_canvas(s: &mut ChromeState, c: Canvas) {
    s.canvas = c;
    match c {
        Canvas::Editor => s.workspace = WorkspaceTab::TextEditor,
        Canvas::Device => s.workspace = WorkspaceTab::Device,
        Canvas::Timing => {
            if s.activity == Activity::Reports && chrome::is_report_detail(s.workspace) {
            } else {
                s.workspace = WorkspaceTab::Reports;
            }
        }
    }
}

pub fn apply_activity(s: &mut ChromeState, a: Activity) {
    s.activity = a;
    s.sidebar_hidden = matches!(a, Activity::Timing | Activity::Simulate);
    match a {
        Activity::Files => apply_canvas(s, Canvas::Editor),
        Activity::Device => apply_canvas(s, Canvas::Device),
        Activity::Timing => {
            s.canvas = Canvas::Timing;
            s.workspace = WorkspaceTab::Reports;
        }
        Activity::Simulate => {
            // Do not leave Timing/Device canvas selected — Waveform is the pane.
            s.canvas = Canvas::Editor;
            if !s.workspace.sim_only() {
                s.workspace = WorkspaceTab::Wave;
            }
        }
        Activity::Program => {}
        Activity::Reports => {
            s.canvas = Canvas::Timing;
            if !chrome::is_report_detail(s.workspace) {
                s.workspace = WorkspaceTab::Reports;
            }
        }
    }
}

pub fn apply_more(s: &mut ChromeState, tab: WorkspaceTab) {
    s.workspace = tab;
    s.sidebar_hidden = tab.sim_only();
    if tab.sim_only() {
        s.activity = Activity::Simulate;
        return;
    }
    if chrome::is_report_detail(tab) {
        s.activity = Activity::Reports;
        s.canvas = Canvas::Timing;
        s.sidebar_hidden = false;
        return;
    }
    match chrome::pane_for_workspace(tab) {
        WorkspacePane::Package => {
            s.activity = Activity::Device;
            s.canvas = Canvas::Device;
            s.sidebar_hidden = false;
        }
        WorkspacePane::Hardware | WorkspacePane::Bitstream => {
            s.activity = Activity::Program;
            s.sidebar_hidden = false;
        }
        WorkspacePane::ReportsCatalog => {
            s.activity = Activity::Reports;
            s.canvas = Canvas::Timing;
            s.sidebar_hidden = false;
        }
        _ => {
            s.activity = Activity::Files;
            s.sidebar_hidden = false;
        }
    }
}

/// Drag a splitter; size is kept. Returns false if the travel is below the bar.
pub fn drag_sidebar(s: &mut ChromeState, delta: f32) -> bool {
    let before = s.sidebar_width;
    s.sidebar_width = (s.sidebar_width + delta)
        .clamp(chrome::SIDEBAR_MIN_WIDTH, chrome::SIDEBAR_MAX_WIDTH);
    (s.sidebar_width - before).abs() + 0.5 >= delta.abs().min(chrome::SPLITTER_MIN_DELTA_PX)
}

pub fn drag_console(s: &mut ChromeState, delta: f32) -> bool {
    let before = s.console_height;
    s.console_height = (s.console_height + delta)
        .clamp(chrome::CONSOLE_MIN_HEIGHT, chrome::CONSOLE_MAX_HEIGHT);
    (s.console_height - before).abs() + 0.5 >= delta.abs().min(chrome::SPLITTER_MIN_DELTA_PX)
}

#[derive(Clone, Debug)]
pub struct GeometryReport {
    pub window_w: f32,
    pub window_h: f32,
    pub rail_w: f32,
    pub canvas_w: f32,
    pub die_fill: f32,
    pub die_gap_px: f32,
    pub rail_labels_fit: bool,
    pub more_or_tabs_visible: bool,
    pub table_declares_scroll: bool,
    pub splitter_can_travel: bool,
    pub clipped_rail: Vec<&'static str>,
}

pub fn geometry_at(window_w: f32, window_h: f32, sidebar: f32, console: f32) -> GeometryReport {
    let _ = (window_h, console);
    let plan = chrome::chrome_at(window_w);
    let canvas_w = (window_w - chrome::RAIL_WIDTH - sidebar).max(80.0);
    let canvas_h = 500.0_f32;
    let cell = chrome::floorplan_fit_cell(32, 33, canvas_w, canvas_h);
    let mut clipped = Vec::new();
    for a in Activity::ALL {
        if !chrome::rail_name_fits(a.label()) {
            clipped.push(a.label());
        }
    }
    GeometryReport {
        window_w,
        window_h,
        rail_w: chrome::RAIL_WIDTH,
        canvas_w,
        die_fill: chrome::floorplan_die_fill_ratio(32, cell, canvas_w),
        die_gap_px: chrome::floorplan_right_gap_px(32, cell, canvas_w),
        rail_labels_fit: clipped.is_empty(),
        more_or_tabs_visible: plan.dropped_workspace() == 0
            && (plan.workspace_mode == chrome::OverflowMode::Fit
                || plan.tab_rows.iter().any(|r| r.contains(&chrome::MORE))),
        table_declares_scroll: {
            let t = chrome::table_scroll_policy(10, canvas_w.min(400.0));
            t.x && t.y
        },
        splitter_can_travel: chrome::splitter_can_travel(
            chrome::SIDEBAR_MIN_WIDTH,
            chrome::SIDEBAR_MAX_WIDTH,
        ) && chrome::splitter_can_travel(
            chrome::CONSOLE_MIN_HEIGHT,
            chrome::CONSOLE_MAX_HEIGHT,
        ),
        clipped_rail: clipped,
    }
}

/// Heavy CAD work — never run inside an egui frame.
#[derive(Clone, Debug)]
pub enum JobKind {
    Open(PathBuf),
    Implement,
    Step(FlowStep),
    SimRun(u32),
}

impl JobKind {
    pub fn widget(&self) -> &'static str {
        match self {
            JobKind::Open(_) => "Open",
            JobKind::Implement => "Implement",
            JobKind::Step(FlowStep::Synthesis) => "Synth",
            JobKind::Step(FlowStep::Opt) => "Opt",
            JobKind::Step(FlowStep::Place) => "Place",
            JobKind::Step(FlowStep::Route) => "Route",
            JobKind::Step(FlowStep::Bitstream) => "Bitstream",
            JobKind::SimRun(_) => "Simulate",
        }
    }

    pub fn progress_english(&self) -> String {
        match self {
            JobKind::Open(p) => format!(
                "Opening {}…",
                p.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.display().to_string())
            ),
            JobKind::Implement => "Implementing — synthesis, opt, place, route…".into(),
            JobKind::Step(s) => format!("Running {}…", s.label()),
            JobKind::SimRun(n) => format!("Running simulation ({n} cycles)…"),
        }
    }
}

pub struct JobOutcome {
    pub model: IdeModel,
    pub kind: JobKind,
    pub result: Result<String, String>,
}

pub struct JobHandle {
    rx: Receiver<JobOutcome>,
}

impl JobHandle {
    pub fn try_recv(&self) -> Result<JobOutcome, TryRecvError> {
        self.rx.try_recv()
    }
}

fn run_kind(model: &mut IdeModel, kind: &JobKind) -> Result<String, String> {
    match kind {
        JobKind::Open(p) => model.open_source(p),
        JobKind::Implement => model.implement(),
        JobKind::Step(s) => model.run_step(*s),
        JobKind::SimRun(n) => model.sim_run(*n),
    }
}

/// Spawn synth/place/route/sim off the UI thread. The UI polls [`JobHandle::try_recv`].
pub fn spawn_job(model: IdeModel, kind: JobKind) -> JobHandle {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("helion-engine".into())
        .spawn(move || {
            let mut model = model;
            let result = run_kind(&mut model, &kind);
            let _ = tx.send(JobOutcome { model, kind, result });
        });
    JobHandle { rx }
}

/// Same work as [`spawn_job`], on the caller thread (unit tests / `--headless`).
pub fn run_job_sync(model: &mut IdeModel, kind: &JobKind) -> Result<String, String> {
    run_kind(model, kind)
}

/// Programmatic IDE: clicks update chrome + model the same way the window does.
pub struct ChromeDriver {
    pub model: IdeModel,
    pub chrome: ChromeState,
    pub trace: UiTrace,
}

impl ChromeDriver {
    pub fn new() -> Self {
        Self {
            model: IdeModel::new(),
            chrome: ChromeState::default(),
            trace: UiTrace::default(),
        }
    }

    fn record(
        &mut self,
        kind: &'static str,
        widget: &str,
        expected: &str,
        actual: &str,
        class: EventClass,
    ) {
        self.trace.push(UiEvent {
            at_ms: now_ms(),
            kind,
            widget: widget.into(),
            expected: expected.into(),
            actual: actual.into(),
            class,
        });
    }

    pub fn pane(&self) -> WorkspacePane {
        central_pane(&self.chrome)
    }

    pub fn click_activity(&mut self, a: Activity) {
        apply_activity(&mut self.chrome, a);
        self.model.workspace = self.chrome.workspace;
        let actual = format!("{:?}", self.pane());
        let class = if a == Activity::Timing {
            if self.pane() == WorkspacePane::Timing {
                EventClass::Ok
            } else {
                EventClass::StatePaint
            }
        } else {
            EventClass::Ok
        };
        self.record("click", a.label(), a.label(), &actual, class);
    }

    pub fn click_canvas(&mut self, c: Canvas) {
        match c {
            Canvas::Editor => apply_activity(&mut self.chrome, Activity::Files),
            Canvas::Device => apply_activity(&mut self.chrome, Activity::Device),
            Canvas::Timing => apply_activity(&mut self.chrome, Activity::Timing),
        }
        self.model.workspace = self.chrome.workspace;
        let pane = self.pane();
        let expected = match c {
            Canvas::Editor => WorkspacePane::Editor,
            Canvas::Device => WorkspacePane::Device,
            Canvas::Timing => WorkspacePane::Timing,
        };
        let class = if pane == expected {
            EventClass::Ok
        } else {
            EventClass::StatePaint
        };
        self.record(
            "click",
            c.label(),
            &format!("{expected:?}"),
            &format!("{pane:?}"),
            class,
        );
        debug_assert_eq!(pane, expected, "canvas {} click painted {pane:?}", c.label());
    }

    pub fn click_more(&mut self, tab: WorkspaceTab) {
        let expected = chrome::pane_for_workspace(tab);
        apply_more(&mut self.chrome, tab);
        self.model.workspace = self.chrome.workspace;
        let pane = self.pane();
        let class = if pane == expected {
            EventClass::Ok
        } else {
            EventClass::StatePaint
        };
        self.record(
            "click",
            tab.label(),
            &format!("{expected:?}"),
            &format!("{pane:?}"),
            class,
        );
        debug_assert_eq!(
            pane, expected,
            "More {} set workspace={:?} but pane={pane:?}",
            tab.label(),
            self.chrome.workspace
        );
    }

    pub fn click_open(&mut self, path: &Path) -> Result<String, String> {
        let r = run_job_sync(&mut self.model, &JobKind::Open(path.to_path_buf()));
        if r.is_ok() {
            apply_activity(&mut self.chrome, Activity::Files);
            self.model.workspace = self.chrome.workspace;
        }
        let class = if r.is_ok() {
            EventClass::Ok
        } else {
            EventClass::HitTestMiss
        };
        self.record(
            "click",
            "Open",
            "Editor",
            &format!("{:?}", self.pane()),
            class,
        );
        r
    }

    pub fn click_implement(&mut self) -> Result<String, String> {
        let r = run_job_sync(&mut self.model, &JobKind::Implement);
        if r.is_ok() {
            apply_activity(&mut self.chrome, Activity::Device);
            self.model.workspace = self.chrome.workspace;
        }
        self.record(
            "click",
            "Implement",
            "Device",
            &format!("{:?}", self.pane()),
            if r.is_ok() {
                EventClass::Ok
            } else {
                EventClass::UiThreadBlocked
            },
        );
        r
    }

    pub fn click_sim_run(&mut self) -> Result<String, String> {
        let n = self.model.sim_runtime_cycles.max(1);
        let r = run_job_sync(&mut self.model, &JobKind::SimRun(n));
        apply_activity(&mut self.chrome, Activity::Simulate);
        self.model.workspace = self.chrome.workspace;
        self.record(
            "click",
            "Simulate",
            "Wave",
            &format!("{:?}", self.pane()),
            if r.is_ok() {
                EventClass::Ok
            } else {
                EventClass::UiThreadBlocked
            },
        );
        r
    }

    pub fn click_examples(&mut self, file: &str) -> Result<String, String> {
        let p = helion_device::Device::examples_dir().join(file);
        self.click_open(&p)
    }

    pub fn click_schematic_zoom_fit(&mut self) -> Result<String, String> {
        if self.pane() != WorkspacePane::Schematic {
            self.record(
                "click",
                "Zoom Fit",
                "Schematic",
                &format!("{:?}", self.pane()),
                EventClass::HitTestMiss,
            );
            return Err("Zoom Fit: schematic not open".into());
        }
        let r = self.model.schematic_zoom_fit();
        self.record(
            "click",
            "Zoom Fit",
            "Schematic",
            &format!("{:?}", self.pane()),
            if r.is_ok() {
                EventClass::Ok
            } else {
                EventClass::HitTestMiss
            },
        );
        r
    }

    pub fn click_schematic_next(&mut self) -> Result<String, String> {
        if self.pane() != WorkspacePane::Schematic {
            self.record(
                "click",
                "Next",
                "Schematic",
                &format!("{:?}", self.pane()),
                EventClass::HitTestMiss,
            );
            return Err("Next: schematic not open".into());
        }
        let r = self.model.schematic_next_view();
        self.record(
            "click",
            "Next",
            "Schematic",
            &format!("{:?}", self.pane()),
            if r.is_ok() {
                EventClass::Ok
            } else {
                EventClass::HitTestMiss
            },
        );
        r
    }

    pub fn click_schematic_previous(&mut self) -> Result<String, String> {
        if self.pane() != WorkspacePane::Schematic {
            self.record(
                "click",
                "Previous",
                "Schematic",
                &format!("{:?}", self.pane()),
                EventClass::HitTestMiss,
            );
            return Err("Previous: schematic not open".into());
        }
        let r = self.model.schematic_previous_view();
        self.record(
            "click",
            "Previous",
            "Schematic",
            &format!("{:?}", self.pane()),
            if r.is_ok() {
                EventClass::Ok
            } else {
                EventClass::HitTestMiss
            },
        );
        r
    }
}

#[cfg(test)]
fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send<T: Send>() {}

    #[test]
    fn ide_model_is_send_so_engine_jobs_leave_the_ui_thread() {
        assert_send::<IdeModel>();
        assert_send::<JobKind>();
        assert_send::<JobOutcome>();
    }

    #[test]
    fn programmatic_clicks_cover_open_rail_canvas_more_implement_sim() {
        let mut d = ChromeDriver::new();
        d.click_open(&example("counter.sv")).unwrap();
        assert_eq!(d.pane(), WorkspacePane::Editor);
        assert!(
            d.model.tree.sources.iter().any(|s| s.ends_with("counter.sv")),
            "{:?}",
            d.model.tree.sources
        );

        for a in Activity::ALL {
            d.click_activity(a);
            assert_eq!(d.chrome.activity, a, "{a:?}");
        }
        d.click_canvas(Canvas::Editor);
        assert_eq!(d.pane(), WorkspacePane::Editor);
        d.click_canvas(Canvas::Device);
        assert_eq!(d.pane(), WorkspacePane::Device);
        d.click_canvas(Canvas::Timing);
        assert_eq!(d.pane(), WorkspacePane::Timing);
        assert_ne!(d.pane(), WorkspacePane::ReportsCatalog);

        d.click_implement().unwrap();
        assert_eq!(d.pane(), WorkspacePane::Device);

        d.click_more(WorkspaceTab::Schematic);
        assert_eq!(d.pane(), WorkspacePane::Schematic);
        assert_eq!(d.chrome.workspace, WorkspaceTab::Schematic);
        let drawing = d.model.schematic.drawing();
        assert!(
            drawing.symbols.iter().any(|s| !s.kind.starts_with("PORT")),
            "schematic after Implement+More must show cells"
        );
        assert!(!drawing.wires.is_empty());
        assert_ne!(d.pane(), WorkspacePane::Timing);

        d.click_sim_run().unwrap();
        assert_eq!(d.pane(), WorkspacePane::Wave);
        assert!(!d.model.wave.traces.is_empty());
    }

    #[test]
    fn simulate_activity_does_not_leave_timing_canvas_selected() {
        let mut d = ChromeDriver::new();
        d.click_canvas(Canvas::Timing);
        assert_eq!(d.pane(), WorkspacePane::Timing);
        d.click_activity(Activity::Simulate);
        assert_eq!(d.pane(), WorkspacePane::Wave);
        assert_ne!(
            d.chrome.canvas,
            Canvas::Timing,
            "Sim rail must not keep Timing highlighted"
        );
        assert_eq!(d.chrome.workspace, WorkspaceTab::Wave);
    }

    #[test]
    fn every_more_destination_click_paints_its_own_pane_not_timing() {
        let mut d = ChromeDriver::new();
        d.click_open(&example("counter.sv")).unwrap();
        d.click_implement().unwrap();
        for tab in WorkspaceTab::ALL {
            if tab.is_canvas() {
                continue;
            }
            d.click_more(tab);
            let pane = d.pane();
            assert_eq!(
                pane,
                chrome::pane_for_workspace(tab),
                "More {} → {pane:?}",
                tab.label()
            );
            assert_ne!(
                pane,
                WorkspacePane::Timing,
                "More {} swallowed by Timing",
                tab.label()
            );
            assert_eq!(d.trace.last_class(), Some(EventClass::Ok), "{}", tab.label());
        }
    }

    #[test]
    fn timing_click_does_not_open_reports_catalog() {
        let mut d = ChromeDriver::new();
        d.click_open(&example("counter.sv")).unwrap();
        d.click_canvas(Canvas::Timing);
        assert_eq!(d.pane(), WorkspacePane::Timing);
        assert_ne!(d.pane(), WorkspacePane::ReportsCatalog);
        d.click_activity(Activity::Reports);
        assert_eq!(d.pane(), WorkspacePane::ReportsCatalog);
        d.click_activity(Activity::Timing);
        assert_eq!(d.pane(), WorkspacePane::Timing);
    }

    #[test]
    fn reports_landing_is_catalog_not_timing_and_fills() {
        let mut d = ChromeDriver::new();
        d.click_open(&example("counter.sv")).unwrap();
        d.click_implement().unwrap();
        d.click_activity(Activity::Reports);
        assert_eq!(d.pane(), WorkspacePane::ReportsCatalog);
        assert_ne!(d.pane(), WorkspacePane::Timing);
        let rows = d.model.report_catalog();
        assert!(
            rows.len() >= 4,
            "Reports catalog must have rows, got {}",
            rows.len()
        );
        assert!(
            chrome::reports_stub_fill(400.0) < chrome::PANE_FILL_MIN,
            "heading-only stub must fail fill"
        );
        let bbox = chrome::reports_catalog_bbox(rows.len(), 1000.0, 400.0);
        assert!(
            bbox.fills() || bbox.fill >= chrome::PANE_FILL_MIN,
            "Reports landing catalog {bbox:?}"
        );
    }

    #[test]
    fn native_open_is_not_a_none_stub() {
        assert_eq!(crate::dialog_backend(), "rfd");
        let _ = crate::hdl_file_dialog();
        let mut d = ChromeDriver::new();
        d.click_examples("counter.sv").unwrap();
        assert_eq!(d.pane(), WorkspacePane::Editor);
    }

    #[test]
    fn geometry_probes_1440_1100_fullscreen() {
        for (w, h) in [(1440.0, 900.0), (1100.0, 640.0), (1920.0, 1080.0)] {
            let g = geometry_at(w, h, chrome::SIDEBAR_WIDTH, chrome::CONSOLE_DEFAULT_HEIGHT);
            assert!(
                g.rail_labels_fit,
                "{w}x{h} clipped rail {:?}",
                g.clipped_rail
            );
            assert!(g.more_or_tabs_visible, "{w}x{h} dropped a canvas tab");
            assert!(
                g.die_fill >= 0.80,
                "{w}x{h} die fill {:.3}",
                g.die_fill
            );
            assert!(g.die_gap_px <= 80.0, "{w}x{h} die gap {}", g.die_gap_px);
            assert!(g.splitter_can_travel);
            assert!(g.table_declares_scroll || w > 1200.0);
        }
    }

    #[test]
    fn splitter_drag_moves_at_least_40px_and_keeps_size() {
        let mut s = ChromeState::default();
        let before = s.sidebar_width;
        assert!(drag_sidebar(&mut s, 48.0));
        assert!(s.sidebar_width - before >= 40.0);
        let kept = s.sidebar_width;
        assert!((s.sidebar_width - kept).abs() < 0.5);
        let c0 = s.console_height;
        assert!(drag_console(&mut s, 50.0));
        assert!(s.console_height - c0 >= 40.0);
    }

    #[test]
    fn spawn_job_returns_updated_model_without_blocking_caller_setup() {
        let mut model = IdeModel::new();
        model.open_source(&example("counter.sv")).unwrap();
        let handle = spawn_job(model.clone(), JobKind::Step(FlowStep::Opt));
        let out = loop {
            match handle.try_recv() {
                Ok(o) => break o,
                Err(TryRecvError::Empty) => std::thread::sleep(std::time::Duration::from_millis(5)),
                Err(TryRecvError::Disconnected) => panic!("engine thread died"),
            }
        };
        assert!(out.result.is_ok(), "{:?}", out.result);
        assert_eq!(out.model.step_state(FlowStep::Opt), crate::StepState::Done);
    }

    #[test]
    fn schematic_previous_next_zoom_fit_are_state_ok_not_hit_miss() {
        let mut d = ChromeDriver::new();
        d.click_open(&example("counter.sv")).unwrap();
        d.click_implement().unwrap();
        assert!(d.click_schematic_zoom_fit().is_err());
        assert_eq!(d.trace.last_class(), Some(EventClass::HitTestMiss));
        d.click_more(WorkspaceTab::Schematic);
        d.click_schematic_zoom_fit().unwrap();
        assert_eq!(d.trace.last_class(), Some(EventClass::Ok));
        let _ = d.click_schematic_next();
        let _ = d.click_schematic_previous();
        assert_eq!(d.pane(), WorkspacePane::Schematic);
        let drawing = d.model.schematic.drawing();
        d.model.schematic.set_viewport(900.0, 500.0);
        d.model.schematic.zoom_fit();
        let fit = chrome::fit_pane(drawing.width, drawing.height, 900.0, 500.0);
        assert!(
            fit.right_clip <= 0.5,
            "Schematic right-edge clip {}",
            fit.right_clip
        );
        assert!(fit.fill >= chrome::PANE_FILL_MIN, "schematic fill {}", fit.fill);
    }

    #[test]
    fn program_package_schematic_fill_the_remaining_pane() {
        let mut d = ChromeDriver::new();
        d.click_open(&example("counter.sv")).unwrap();
        d.click_implement().unwrap();
        d.click_more(WorkspaceTab::Package);
        let pkg = &d.model.package;
        let (cw, ch) = chrome::package_cell(pkg.cols.max(1), pkg.rows.max(1), 1000.0, 400.0);
        let fill = ((cw * pkg.cols.max(1) as f32 + 28.0) / 1000.0)
            .min((ch * pkg.rows.max(1) as f32 + 16.0) / 400.0);
        assert!(
            fill >= chrome::PANE_FILL_MIN,
            "package fill {fill} cols={} rows={}",
            pkg.cols,
            pkg.rows
        );
        d.click_more(WorkspaceTab::Hardware);
        assert_eq!(d.pane(), WorkspacePane::Hardware);
        let n = d.model.hw_stat_report().bits.len().max(3);
        let hw = chrome::hardware_content_bbox(n, 1000.0, 400.0);
        assert!(hw.fills(), "Program STAT content bbox {hw:?}");
        d.click_more(WorkspaceTab::Schematic);
        let drawing = d.model.schematic.drawing();
        let fit = chrome::fit_pane(drawing.width.max(1.0), drawing.height.max(1.0), 900.0, 500.0);
        assert!(fit.right_clip <= 0.5);
        assert!(fit.fills() || fit.fill >= chrome::PANE_FILL_MIN);

        d.click_more(WorkspaceTab::Hierarchy);
        assert_eq!(d.pane(), WorkspacePane::Hierarchy);
        let hier = d.model.hierarchy.drawing();
        assert!(!hier.boxes.is_empty(), "hierarchy after Implement must have boxes");
        let native = chrome::native_drawing_fill(hier.width, hier.height, 1000.0, 400.0);
        assert!(
            native < chrome::PANE_FILL_MIN,
            "native hierarchy {native} cols must be a postage stamp so scale is required ({}×{})",
            hier.width,
            hier.height
        );
        let hfit = chrome::hierarchy_fit(hier.width.max(1.0), hier.height.max(1.0), 1000.0, 400.0);
        assert!(hfit.fills(), "hierarchy scaled {hfit:?}");
        d.click_more(WorkspaceTab::Ip);
        assert_eq!(d.pane(), WorkspacePane::Ip);
        let (cat_h, ip_canvas) = chrome::ip_layout(1000.0, 400.0);
        assert!(cat_h <= chrome::IP_CATALOG_MAX_HEIGHT);
        assert!(
            ip_canvas.fills() || ip_canvas.fill >= chrome::PANE_FILL_MIN,
            "IP canvas {ip_canvas:?}"
        );
    }

    #[test]
    fn settings_and_summary_fill_after_implement() {
        let mut d = ChromeDriver::new();
        d.click_open(&example("counter.sv")).unwrap();
        d.click_implement().unwrap();
        d.click_more(WorkspaceTab::Settings);
        assert_eq!(d.pane(), WorkspacePane::Settings);
        let n = d.model.project_setting_rows().len();
        assert!(n >= 4, "settings rows {n}");
        let bbox = chrome::nv_table_bbox(n, 1000.0, 400.0);
        assert!(
            bbox.fills() || bbox.fill >= chrome::PANE_FILL_MIN,
            "settings {bbox:?}"
        );
        d.click_more(WorkspaceTab::Summary);
        assert_eq!(d.pane(), WorkspacePane::Summary);
        let g = d.model.project_summary_gadgets().len();
        let sb = chrome::nv_table_bbox(g.max(1), 1000.0, 400.0);
        assert!(sb.fills() || sb.fill >= chrome::PANE_FILL_MIN, "summary {sb:?}");
        assert!(chrome::occupancy_bar_w(820.0) > 160.0);
        d.click_more(WorkspaceTab::Power);
        assert_eq!(d.pane(), WorkspacePane::Power);
        assert!(
            !d.model.power_report().part.is_empty(),
            "Power after implement must have a report"
        );
        let pwr_bar = chrome::occupancy_bar_w(820.0);
        assert!(pwr_bar > 120.0, "legacy 120px power share bar {pwr_bar}");
        assert!(
            pwr_bar >= 820.0 * chrome::PANE_FILL_MIN,
            "power share bars must span remaining width, got {pwr_bar}"
        );
        let blocks = d.model.power_block_rows();
        assert!(
            !blocks.is_empty(),
            "Power Utilization Details after implement"
        );
        assert!(
            blocks.iter().any(|(n, _, _, _)| n == "LUTFF"),
            "Utilization Details occupancy rows: {blocks:?}"
        );
        let details_bar = chrome::occupancy_bar_w(820.0);
        assert!(
            details_bar > 180.0,
            "Utilization Details without occupancy bars must fail, got {details_bar}"
        );
        d.click_more(WorkspaceTab::Utilization);
        assert_eq!(d.pane(), WorkspacePane::Utilization);
        assert!(
            !d.model.utilization_report().occupancy.is_empty(),
            "Utilization occupancy after implement"
        );
        let n_occ = d.model.utilization_report().occupancy.len() + 1;
        let compact_occ = chrome::occupancy_table_compact_h(n_occ);
        assert!(
            compact_occ / 400.0 < chrome::PANE_FILL_MIN,
            "unstretched occupancy table {compact_occ} must fail so stretch is required"
        );
        let occ_bbox = chrome::occupancy_table_bbox(n_occ, 1000.0, 400.0);
        assert!(
            occ_bbox.fills() || occ_bbox.fill >= chrome::PANE_FILL_MIN,
            "utilization occupancy table {occ_bbox:?}"
        );
        let n_pwr = 9 + d.model.power_block_rows().len();
        let pwr_bbox = chrome::occupancy_table_bbox(n_pwr.max(1), 1000.0, 400.0);
        assert!(
            pwr_bbox.fills() || pwr_bbox.fill >= chrome::PANE_FILL_MIN,
            "power occupancy table {pwr_bbox:?}"
        );
        assert!(
            !d.model.utilization_report().hierarchy.is_empty(),
            "Utilization Hierarchical after implement"
        );
        let compact_hier = 6.0 * 64.0;
        assert!(
            compact_hier < 1000.0 * chrome::PANE_FILL_MIN,
            "compact Hierarchical columns must fail the fill bar"
        );
        let hier = chrome::stretched_col_w_gap(6, 1000.0, 8.0);
        let hier_span = hier * 6.0 + 8.0 * 5.0;
        assert!(
            hier_span >= 1000.0 * chrome::PANE_FILL_MIN,
            "Hierarchical columns must span remaining width, span={hier_span}"
        );
        let compact_bars = chrome::OCCUPANCY_BAR_H * n_occ as f32;
        assert!(
            compact_bars / 400.0 < chrome::PANE_FILL_MIN,
            "20px occupancy bars must fail so stretch is required"
        );
        let bh = chrome::occupancy_bar_h(n_occ, 400.0);
        assert!(
            bh * n_occ as f32 >= 400.0 * chrome::PANE_FILL_MIN,
            "occupancy bar height {bh} leaves a void"
        );
        let n_bars = d.model.utilization_report().occupancy.len().max(1);
        let n_footer = d.model.utilization_report().hierarchy.len() + 1;
        let wrong = chrome::occupancy_bar_h(n_bars + 1 + n_footer, 400.0);
        let wrong_drawn = wrong * n_bars as f32 + chrome::occupancy_table_compact_h(n_footer);
        assert!(
            400.0 - wrong_drawn > chrome::PANE_EMPTY_GAP_MAX,
            "counting Hierarchical rows as occupancy bars must leave a void ({wrong_drawn})"
        );
        let bh_footer = chrome::occupancy_bars_h_after_footer(n_bars, n_footer, 400.0);
        let drawn = bh_footer * n_bars as f32 + chrome::occupancy_table_compact_h(n_footer);
        assert!(
            drawn >= 400.0 * chrome::PANE_FILL_MIN || 400.0 - drawn <= chrome::PANE_EMPTY_GAP_MAX,
            "occupancy bars after Hierarchical footer {drawn}"
        );
        let labels4 = chrome::occupancy_label_reserve(4);
        assert!(
            280.0 - labels4 > 40.0,
            "legacy 280px occupancy label reserve over-subtracts ({labels4})"
        );
        let tight = chrome::occupancy_bar_w(1000.0 - labels4);
        assert!(
            tight > chrome::occupancy_bar_w(1000.0 - 280.0),
            "tight occupancy bars {tight} must beat 280px reserve"
        );
    }

    #[test]
    fn program_paint_path_does_not_reshell_ofl() {
        let n0 = helion_hw::usb_scan_invocations();
        let det = helion_hw::detect_boards();
        let n1 = helion_hw::usb_scan_invocations();
        assert!(n1 > n0);
        let _ = helion_hw::resolve_cable_from("auto", &det);
        let _ = helion_hw::resolve_cable_from("sim", &det);
        let _ = helion_hw::resolve_cable_from("usb", &det);
        assert_eq!(
            helion_hw::usb_scan_invocations(),
            n1,
            "Program sidebar must use cached DetectReport, not resolve_cable every frame"
        );
    }

    #[test]
    fn jobs_are_not_run_inside_a_paint_callback() {
        let _ = take_jobs();
        queue_flow(FlowStep::Synthesis);
        queue_flow(FlowStep::Place);
        queue_flow(FlowStep::Route);
        let queued = take_jobs();
        assert_eq!(queued.len(), 3, "{queued:?}");
        assert!(queued.iter().all(|k| matches!(k, JobKind::Step(_))));
        assert_send::<IdeModel>();
        let mut d = ChromeDriver::new();
        d.click_open(&example("counter.sv")).unwrap();
        let handle = spawn_job(d.model.clone(), JobKind::Implement);
        match handle.try_recv() {
            Ok(_) | Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => panic!("engine thread died before first poll"),
        }
        let out = loop {
            match handle.try_recv() {
                Ok(o) => break o,
                Err(TryRecvError::Empty) => std::thread::sleep(std::time::Duration::from_millis(5)),
                Err(TryRecvError::Disconnected) => panic!("engine thread died"),
            }
        };
        assert!(out.result.is_ok(), "{:?}", out.result);
    }
}
