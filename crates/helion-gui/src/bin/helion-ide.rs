//! Helion IDE — three canvases, one activity rail, one Implement.
//!
//! `--version` / `--doctor` never open a window (so they work headless and on CI).
//! The GUI paints [`helion_gui::IdeModel`]; every button and the Tcl box call into
//! that model, which is what the unit tests already prove is not a no-op.

use eframe::egui::{self, Color32, RichText, Sense, Stroke};
use helion_gui::chrome::{self, Activity, Canvas, RAIL_OPEN_SOURCES};
use helion_gui::{
    doctor, BottomTab, CdcSeverity, ClockRelation, ConstraintSection, DrcSeverity,
    FlowStep, IdeModel, IlaTrigger, LayoutKind, MethodologySeverity, MsgSeverity, NavSection,
    PathGroupKind, StepState, WaveRadix, WaveStyle, WorkspaceTab,
};
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

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
            if let Some(wns) = ide.wns_ps() {
                if !out.contains("WNS_PS=") {
                    println!("WNS_PS={wns}");
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
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1100.0, 640.0])
            .with_title("Helion"),
        ..Default::default()
    };
    eframe::run_native(
        "Helion",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(HelionIde::new()))
        }),
    )
}

struct HelionIde {
    model: IdeModel,
    tree_filter: String,
    sidebar_hidden: bool,
    activity: Activity,
    canvas: Canvas,
    show_tcl: bool,
    show_palette: bool,
    show_examples: bool,
    recent: Vec<PathBuf>,
    tcl_focus: bool,
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
        Self {
            model,
            tree_filter: String::new(),
            sidebar_hidden: false,
            activity,
            canvas,
            show_tcl: false,
            show_palette: false,
            show_examples: false,
            recent: Vec::new(),
            tcl_focus: false,
        }
    }

    fn set_canvas(&mut self, c: Canvas) {
        self.canvas = c;
        self.model.workspace = match c {
            Canvas::Editor => WorkspaceTab::TextEditor,
            Canvas::Device => WorkspaceTab::Device,
            Canvas::Timing => WorkspaceTab::Reports,
        };
    }

    fn set_activity(&mut self, a: Activity) {
        self.activity = a;
        match a {
            Activity::Files => {
                self.sidebar_hidden = false;
                self.model.layout = LayoutKind::Default;
                self.set_canvas(Canvas::Editor);
            }
            Activity::Device => {
                self.sidebar_hidden = false;
                self.set_canvas(Canvas::Editor);
            }
            Activity::Timing => {
                self.sidebar_hidden = false;
                self.set_canvas(Canvas::Timing);
            }
            Activity::Simulate => {
                self.sidebar_hidden = false;
                let _ = self.model.set_layout(LayoutKind::Simulation);
                self.model.workspace = WorkspaceTab::Wave;
            }
            Activity::Program => {
                self.sidebar_hidden = false;
                let _ = self.model.set_nav(NavSection::ProgramDebug);
            }
            Activity::Reports => {
                self.sidebar_hidden = false;
                self.set_canvas(Canvas::Timing);
                self.model.workspace = WorkspaceTab::Reports;
            }
        }
    }

    fn remember(&mut self, path: PathBuf) {
        self.recent.retain(|p| p != &path);
        self.recent.insert(0, path);
        self.recent.truncate(8);
    }

    fn open_path(&mut self, path: &Path) {
        match self.model.open_source(path) {
            Ok(_) => {
                self.remember(path.to_path_buf());
                self.set_canvas(Canvas::Editor);
            }
            Err(_) => {}
        }
    }
}

impl eframe::App for HelionIde {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        handle_shortcuts(ctx, self);
        paint_toolbar(ctx, self);
        paint_status_bar(ctx, &self.model);
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
    }
}

fn handle_shortcuts(ctx: &egui::Context, app: &mut HelionIde) {
    let cmd = egui::Modifiers::COMMAND;
    if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::O)) {
        native_open(app);
    }
    if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::Enter)) {
        run_implement(&mut app.model);
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
        app.open_path(&path);
    }
}

fn native_open_dialog() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let script = r#"try
    POSIX path of (choose file with prompt "Open HDL" of type {"sv", "v", "svh", "vhd", "sdc", "xdc"})
on error
    ""
end try"#;
        let out = Command::new("osascript").arg("-e").arg(script).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if p.is_empty() {
            None
        } else {
            Some(PathBuf::from(p))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn run_implement(model: &mut IdeModel) {
    let _ = model.implement();
}


fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add_sized(
        [ui.spacing().interact_size.x.max(96.0), chrome::HIT_PRIMARY],
        egui::Button::new(RichText::new(text).size(14.0)),
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
                let open = ui
                    .add_sized(
                        [72.0, chrome::HIT_PRIMARY],
                        egui::Button::new("Open…"),
                    )
                    .on_hover_text(tip("Open", "⌘O", "open_source"));
                if open.clicked() {
                    native_open(app);
                }
                ui.menu_button("Recent", |ui| {
                    if app.recent.is_empty() {
                        ui.label("No recent files.");
                    } else {
                        let paths: Vec<PathBuf> = app.recent.clone();
                        for p in paths {
                            let name = p
                                .file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| p.display().to_string());
                            if ui.button(name).clicked() {
                                app.open_path(&p);
                                ui.close_menu();
                            }
                        }
                    }
                });
                ui.separator();
                paint_progress_strip(ui, &mut app.model);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    let impl_btn = ui
                        .add_sized(
                            [112.0, chrome::HIT_PRIMARY],
                            egui::Button::new(RichText::new("Implement").strong()),
                        )
                        .on_hover_text(tip("Implement", "⌘↩", "impl_design"));
                    if impl_btn.clicked() {
                        run_implement(&mut app.model);
                    }
                });
            });
        });
}

fn paint_progress_strip(ui: &mut egui::Ui, model: &mut IdeModel) {
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
            let state = model.step_state(step);
            let blocked = model.step_blocked(step);
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
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(64.0, 28.0), Sense::click());
                if ui.is_rect_visible(rect) {
                    ui.painter().rect(
                        rect,
                        2.0,
                        fill,
                        Stroke::new(1.0, stroke),
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
                    let _ = model.run_step(step);
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
            let mut pick = None;
            for act in Activity::ALL {
                let on = app.activity == act;
                let fill = if on {
                    Color32::from_rgb(0x1f, 0x4a, 0x38)
                } else {
                    Color32::TRANSPARENT
                };
                let resp = ui.add_sized(
                    [chrome::RAIL_WIDTH - 4.0, chrome::HIT_PRIMARY],
                    egui::Button::new(RichText::new(act.icon()).size(11.0).strong())
                        .fill(fill),
                );
                let resp = resp.on_hover_text(tip(act.label(), "", act.tcl()));
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
        Activity::Simulate => paint_sim_side(ctx, &mut app.model),
        Activity::Files => paint_files_side(ctx, app),
        Activity::Device => paint_files_side(ctx, app),
        Activity::Timing | Activity::Reports => paint_files_side(ctx, app),
        Activity::Program => paint_files_side(ctx, app),
    }
}

fn paint_files_side(ctx: &egui::Context, app: &mut HelionIde) {
    egui::SidePanel::left("sidebar")
        .resizable(true)
        .default_width(chrome::SIDEBAR_WIDTH)
        .min_width(180.0)
        .max_width(360.0)
        .show(ctx, |ui| {
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
                Activity::Timing | Activity::Reports => {
                    paint_report_catalog(ui, &mut app.model);
                }
                Activity::Program => {
                    ui.label("Hardware and bitstream.");
                    if primary_button(ui, "Program").clicked() {
                        let _ = app.model.exec("program_hw");
                    }
                }
                Activity::Simulate => {}
            }
            if app.model.selected.is_some()
                || app.model.selected_source.is_some()
                || app.model.selected_io_port.is_some()
            {
                paint_properties(ui, &mut app.model);
            }
        });
}

fn paint_files_tree(ui: &mut egui::Ui, app: &mut HelionIde) {
    let src_rows = app.model.source_rows();
    if src_rows.is_empty() {
        ui.label("No sources yet.");
        if primary_button(ui, "Open HDL…")
            .on_hover_text(tip("Open", "⌘O", "open_source"))
            .clicked()
        {
            native_open(app);
        }
        if ui.button("Examples").clicked() {
            app.show_examples = true;
        }
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
                    egui::SelectableLabel::new(on, &r.name),
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
                egui::SelectableLabel::new(on, format!("{}  {}", r.name, r.type_cell())),
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

fn paint_status_bar(ctx: &egui::Context, model: &IdeModel) {
    egui::TopBottomPanel::bottom("status")
        .exact_height(chrome::STATUS_HEIGHT)
        .show_separator_line(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let wns = model
                    .wns_ps()
                    .map(|w| w.to_string())
                    .unwrap_or_else(|| "—".into());
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
                ui.label(
                    RichText::new(format!(
                        "{} · WNS {} · LUTFF {} · {}",
                        model.part(),
                        wns,
                        lutff,
                        run
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
    let obj = model
        .properties_name()
        .or(model.selected.as_deref())
        .or(model.selected_ip.as_deref())
        .unwrap_or("—");
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

fn paint_bottom(ctx: &egui::Context, app: &mut HelionIde) {
    egui::TopBottomPanel::bottom("console")
        .resizable(true)
        .default_height(180.0)
        .min_height(80.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let console_on = app.model.bottom_tab == BottomTab::Tcl
                    || app.model.bottom_tab == BottomTab::Log;
                if ui.selectable_label(console_on, "Console").clicked() {
                    app.model.bottom_tab = BottomTab::Tcl;
                }
                if ui
                    .selectable_label(app.model.bottom_tab == BottomTab::Messages, "Messages")
                    .clicked()
                {
                    app.model.bottom_tab = BottomTab::Messages;
                }
                if app.activity == Activity::Simulate
                    && ui
                        .selectable_label(app.model.bottom_tab == BottomTab::SimLog, "Sim log")
                        .clicked()
                {
                    app.model.bottom_tab = BottomTab::SimLog;
                }
            });
            match app.model.bottom_tab {
                BottomTab::Tcl | BottomTab::Log => paint_tcl_console(ui, app),
                BottomTab::Messages => paint_messages(ui, &mut app.model),
                BottomTab::SimLog => paint_sim_log(ui, &mut app.model),
            }
        });
}

fn paint_tcl_window(ctx: &egui::Context, app: &mut HelionIde) {
    if !app.show_tcl {
        return;
    }
    egui::Window::new("Tcl")
        .open(&mut app.show_tcl)
        .default_width(520.0)
        .default_height(240.0)
        .show(ctx, |ui| {
            paint_tcl_console(ui, app);
        });
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
            run_implement(&mut app.model);
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
        app.open_path(&p);
        app.show_examples = false;
    }
}

fn paint_workspace(ui: &mut egui::Ui, app: &mut HelionIde) {
    let avail = ui.available_width();
    let plan = {
        let mut p = chrome::chrome_at(ui.ctx().screen_rect().width());
        let (row, more) = chrome::fit_or_more(&chrome::workspace_tab_labels(), avail);
        p.tab_rows = vec![row];
        p.more_items = more;
        p.workspace_mode = chrome::workspace_tab_overflow(avail);
        p
    };
    ui.horizontal(|ui| {
        for lab in plan.tab_rows.first().into_iter().flatten().copied() {
            if lab == chrome::MORE || lab == chrome::MORE_LABEL {
                ui.menu_button(chrome::MORE, |ui| {
                    for extra in &plan.more_items {
                        if let Some(c) = Canvas::parse_label(extra) {
                            if ui
                                .selectable_label(app.canvas == c, extra.to_string())
                                .clicked()
                            {
                                app.set_canvas(c);
                                ui.close_menu();
                            }
                        }
                    }
                });
                continue;
            }
            if let Some(c) = Canvas::parse_label(lab) {
                let on = app.canvas == c;
                if ui
                    .selectable_label(on, format!("{}  {}", c.label(), c.shortcut()))
                    .on_hover_text(tip(c.label(), c.shortcut(), ""))
                    .clicked()
                {
                    app.set_canvas(c);
                }
            }
        }
    });
    ui.separator();
    if app.activity == Activity::Program {
        paint_hw(ui, &mut app.model);
        return;
    }
    if app.activity == Activity::Simulate
        && !matches!(
            app.model.workspace,
            WorkspaceTab::Device | WorkspaceTab::TextEditor | WorkspaceTab::Source
        )
    {
        match app.model.workspace {
            WorkspaceTab::Wave => paint_wave(ui, &mut app.model),
            WorkspaceTab::Memory => paint_memory(ui, &mut app.model),
            WorkspaceTab::Breakpoints => paint_breakpoints(ui, &mut app.model),
            WorkspaceTab::Locals => paint_locals(ui, &mut app.model),
            WorkspaceTab::Forces => paint_forces(ui, &mut app.model),
            WorkspaceTab::SimSettings => paint_sim_settings(ui, &mut app.model),
            WorkspaceTab::Source => paint_source(ui, &mut app.model),
            _ => paint_wave(ui, &mut app.model),
        }
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
            egui::CollapsingHeader::new("I/O")
                .default_open(false)
                .show(ui, |ui| {
                    paint_io_ports_table(ui, &mut app.model, "device_io_overlay");
                });
            paint_device(ui, &mut app.model);
        }
        Canvas::Timing => {
            if app.activity == Activity::Reports {
                paint_reports(ui, &mut app.model);
            } else {
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
                    _ => {
                        paint_timing_summary(ui, &mut app.model);
                        paint_timing_paths(ui, &mut app.model);
                    }
                }
            }
        }
    }
}

fn paint_empty_editor(ui: &mut egui::Ui, app: &mut HelionIde) {
    ui.vertical_centered(|ui| {
        ui.add_space(48.0);
        ui.label(RichText::new("No sources yet.").size(16.0));
        ui.add_space(8.0);
        if primary_button(ui, "Open HDL…")
            .on_hover_text(tip("Open", "⌘O", "open_source"))
            .clicked()
        {
            native_open(app);
        }
        ui.add_space(6.0);
        if ui.button("Examples").clicked() {
            app.show_examples = true;
        }
    });
}


fn paint_sim_side(ctx: &egui::Context, model: &mut IdeModel) {
    egui::SidePanel::left("scopes")
        .resizable(true)
        .default_width(chrome::SIDEBAR_WIDTH)
        .show(ctx, |ui| {
            ui.label(RichText::new("Scopes").strong());
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
                if ui.button("Settings").clicked() {
                    let _ = model.exec("simulation_settings");
                }
            });
            let scopes = model.scope_rows().to_vec();
            let selected_scope = model.selected_scope.clone();
            let mut pick_scope = None;
            egui::ScrollArea::vertical()
                .id_salt("ug900_scopes")
                .max_height(180.0)
                .show(ui, |ui| {
                    egui::Grid::new("ug900_scopes_table")
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            ui.label(RichText::new("Name").strong());
                            ui.label(RichText::new("Type").strong());
                            ui.end_row();
                            if scopes.is_empty() {
                                ui.label("—");
                                ui.label("no scopes — sim_run");
                                ui.end_row();
                            } else {
                                for (i, s) in scopes.iter().enumerate() {
                                    let on = selected_scope.as_deref() == Some(s.name.as_str());
                                    if ui.selectable_label(on, &s.name).clicked() {
                                        pick_scope = Some(i.to_string());
                                    }
                                    if ui.selectable_label(on, s.type_cell()).clicked() {
                                        pick_scope = Some(i.to_string());
                                    }
                                    ui.end_row();
                                }
                            }
                        });
                });
            if let Some(spec) = pick_scope {
                let _ = model.select_scope(&spec);
            }
            ui.separator();
            ui.label(RichText::new("Objects").strong());
            let objects = model.object_rows().to_vec();
            let selected_object = model.selected_object.clone();
            let mut pick_obj = None;
            egui::ScrollArea::vertical()
                .id_salt("ug900_objects")
                .show(ui, |ui| {
                    egui::Grid::new("ug900_objects_table")
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            ui.label(RichText::new("Name").strong());
                            ui.label(RichText::new("Type").strong());
                            ui.label(RichText::new("Value").strong());
                            ui.end_row();
                            if objects.is_empty() {
                                ui.label("—");
                                ui.label("—");
                                ui.label("no objects — select a Scope");
                                ui.end_row();
                            } else {
                                for (i, o) in objects.iter().enumerate() {
                                    let on = selected_object.as_deref() == Some(o.name.as_str());
                                    if ui.selectable_label(on, &o.name).clicked() {
                                        pick_obj = Some(i.to_string());
                                    }
                                    if ui.selectable_label(on, o.type_cell()).clicked() {
                                        pick_obj = Some(i.to_string());
                                    }
                                    if ui.selectable_label(on, o.value_cell()).clicked() {
                                        pick_obj = Some(i.to_string());
                                    }
                                    ui.end_row();
                                }
                            }
                        });
                });
            if let Some(spec) = pick_obj {
                let _ = model.select_object(&spec);
            }
            ui.separator();
            ui.label(RichText::new("Locals").strong());
            let locals = model.local_rows().to_vec();
            let selected_local = model.selected_local.clone();
            let mut pick_local = None;
            egui::ScrollArea::vertical()
                .id_salt("ug900_locals")
                .max_height(160.0)
                .show(ui, |ui| {
                    egui::Grid::new("ug900_locals_table")
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            ui.label(RichText::new("Name").strong());
                            ui.label(RichText::new("Type").strong());
                            ui.label(RichText::new("Value").strong());
                            ui.end_row();
                            if locals.is_empty() {
                                ui.label("—");
                                ui.label("—");
                                ui.label("no locals — sim_run");
                                ui.end_row();
                            } else {
                                for (i, l) in locals.iter().enumerate() {
                                    let on = selected_local.as_deref() == Some(l.name.as_str());
                                    if ui.selectable_label(on, &l.name).clicked() {
                                        pick_local = Some(i.to_string());
                                    }
                                    if ui.selectable_label(on, l.type_cell()).clicked() {
                                        pick_local = Some(i.to_string());
                                    }
                                    if ui.selectable_label(on, l.value_cell()).clicked() {
                                        pick_local = Some(i.to_string());
                                    }
                                    ui.end_row();
                                }
                            }
                        });
                });
            if let Some(spec) = pick_local {
                let _ = model.select_local(&spec);
            }
            ui.horizontal(|ui| {
                if ui.button("Source").clicked() {
                    let _ = model.open_source_window();
                }
                if ui.button("Memory").clicked() {
                    let _ = model.open_memory();
                }
                if ui.button("Breakpoints").clicked() {
                    let _ = model.open_breakpoints();
                }
                if ui.button("Force").clicked() {
                    let _ = model.open_forces();
                }
            });
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
                        ui.label("no messages");
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
        if let Some(i) = model.console_selected {
            ui.weak(format!("sel={i}"));
        }
    });
    let selected = model.console_selected;
    let hits = model.console_find_hits.clone();
    let rows: Vec<(usize, helion_gui::ConsoleLine)> = model
        .console_rows()
        .into_iter()
        .map(|(i, l)| (i, l.clone()))
        .collect();
    let mut pick_line = None;
    let mut open_hdl = false;
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
                        if primary_button(ui, "Open HDL…")
                            .on_hover_text(tip("Open", "⌘O", "open_source"))
                            .clicked()
                        {
                            open_hdl = true;
                        }
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
    if open_hdl {
        native_open(app);
        return;
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
                        ui.label("no log — run a flow step or Tcl command");
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
                        ui.label("—");
                        ui.label("No simulation log yet.");
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
    egui::ScrollArea::both()
        .id_salt("ug893_project_summary")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("project_summary_gadgets")
                .spacing([8.0, 4.0])
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
                let max_avail = report
                    .occupancy
                    .iter()
                    .map(|r| r.available.max(1))
                    .max()
                    .unwrap_or(1) as f32;
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
                            let bar_w = 160.0 * (row.available as f32 / max_avail).clamp(0.25, 1.0);
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(bar_w, 12.0), Sense::hover());
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
    if let Some(spec) = pick {
        let _ = model.select_project_summary(&spec);
    }
}

fn paint_project_settings(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Project Settings");
    let rows = model.project_setting_rows();
    let selected = model.selected_setting.clone();
    let mut pick: Option<String> = None;
    egui::ScrollArea::both()
        .id_salt("ug893_project_settings")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("project_settings_table")
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
            .add_sized([140.0, chrome::HIT_PRIMARY], egui::Button::new("Launch selected"))
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
            ui.label("No runs yet.");
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
        ui.weak("no incremental report — incremental_impl / incremental_place");
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
        ui.weak("no ECO cells — synth / insert_eco_lut");
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

fn paint_hierarchy(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Hierarchy");
    let drawing = model.hierarchy.drawing();
    ui.label(format!(
        "boxes={} canvas={}×{}",
        drawing.boxes.len(),
        drawing.width as i32,
        drawing.height as i32
    ));
    let selected = model.selected.clone();
    let mut pick = None;
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let size = egui::vec2(
                drawing.width.max(ui.available_width()).max(280.0),
                drawing.height.max(180.0),
            );
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
            if ui.is_rect_visible(rect) {
                let p = ui.painter();
                p.rect_filled(rect, 0.0, Color32::from_rgb(0x12, 0x16, 0x1a));
                let o = rect.min;
                // Outer boxes first so nested leaves paint on top.
                let mut ordered: Vec<_> = drawing.boxes.iter().collect();
                ordered.sort_by(|a, b| (b.w * b.h).partial_cmp(&(a.w * a.h)).unwrap());
                for b in ordered {
                    let r = egui::Rect::from_min_size(
                        egui::pos2(o.x + b.x, o.y + b.y),
                        egui::vec2(b.w, b.h),
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
                            if on { 2.0 } else { 1.0 },
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
            if resp.clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let lx = pos.x - rect.left();
                    let ly = pos.y - rect.top();
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
                    if rows.is_empty() {
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("no hits — `find u_lut0` in Tcl");
                        ui.label("—");
                        ui.end_row();
                    } else {
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
                            if ui.selectable_label(on_obj, p.package_pin_cell()).clicked() {
                                if p.package_pin_cell() == "-" {
                                    pick_port = Some(p.name.clone());
                                } else {
                                    pick_obj = Some(p.package_pin_cell().to_string());
                                }
                            }
                            if ui.selectable_label(on_obj, p.placed_cell()).clicked() {
                                if p.placed_cell() == "-" {
                                    pick_port = Some(p.name.clone());
                                } else {
                                    pick_obj = Some(p.placed_cell().to_string());
                                }
                            }
                            ui.label(p.iostandard_cell());
                            ui.label(p.drive_cell());
                            ui.label(p.slew_cell());
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
    ui.weak("Select a port, then click an unassigned pin to loc + re-place.");
    if let Some(port) = model.selected_io_port.as_deref().or_else(|| {
        model
            .selected
            .as_deref()
            .filter(|s| model.io_ports.iter().any(|p| p.name == *s))
    }) {
        ui.horizontal(|ui| {
            ui.weak("IOSTANDARD");
            for std in ["LVCMOS18", "LVCMOS33", "LVCMOS12", "SSTL15"] {
                if ui.small_button(std).clicked() {
                    set_iostd = Some((port.to_string(), std));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("DRIVE");
            for ma in ["4", "8", "12", "16"] {
                if ui.small_button(ma).clicked() {
                    set_io = Some((port.to_string(), "DRIVE", ma));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("SLEW");
            for s in ["SLOW", "FAST"] {
                if ui.small_button(s).clicked() {
                    set_io = Some((port.to_string(), "SLEW", s));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("PULLTYPE");
            for s in ["NONE", "PULLUP", "PULLDOWN", "KEEPER"] {
                if ui.small_button(s).clicked() {
                    set_io = Some((port.to_string(), "PULLTYPE", s));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("DIFF_TERM");
            for s in ["FALSE", "TRUE"] {
                if ui.small_button(s).clicked() {
                    set_io = Some((port.to_string(), "DIFF_TERM", s));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.weak("IN_TERM");
            for s in ["NONE", "UNTUNED_SPLIT_40", "UNTUNED_SPLIT_50", "UNTUNED_SPLIT_60"] {
                if ui.small_button(s).clicked() {
                    set_io = Some((port.to_string(), "IN_TERM", s));
                }
            }
        });
    }
    if let Some((port, std)) = set_iostd {
        let _ = model.exec(&format!(
            "set_property IOSTANDARD {std} [get_ports {port}]"
        ));
    }
    if let Some((port, key, val)) = set_io {
        let _ = model.exec(&format!("set_property {key} {val} [get_ports {port}]"));
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
        if ui.button("Resize to CLOCKREGION_X1Y1").clicked() {
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
    });
    let selected = model.selected.clone();
    let selected_pb = model.selected_pblock.clone();
    let rows = model.pblock_rows().to_vec();
    let mut pick_pblock: Option<String> = None;
    let mut pick_obj: Option<String> = None;
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
                ui.label("No pblocks — create_pblock then resize_pblock -add {CLB_X5Y1:CLB_X8Y8}.");
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
                    if ui.selectable_label(on, p.range_text().as_str()).clicked() {
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
    let cols = model.package.cols.max(1);
    let rows = model.package.rows.max(1);
    let x0 = model.package.x0;
    let y0 = model.package.y0;
    let avail = ui.available_size();
    let view_h = avail.y.max(chrome::DRAWING_MIN_HEIGHT);
    let view_w = avail.x.max(80.0);
    let cell = chrome::floorplan_fit_cell(cols, rows, view_w, view_h);
    let mut pick: Option<String> = None;
    let selected = model.selected.clone();
    {
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(view_w, view_h),
                Sense::click(),
            );
            if ui.is_rect_visible(rect) {
                let origin = egui::pos2(rect.left() + 28.0, rect.top() + 4.0);
                let p = ui.painter();
                p.rect_filled(rect, 0.0, Color32::from_rgb(0x0d, 0x10, 0x12));
                // Fig. 53: colored I/O bank regions behind the pin circles.
                let mut banks: std::collections::BTreeMap<u32, (u32, u32, u32, u32, (u8, u8, u8))> =
                    std::collections::BTreeMap::new();
                for pin in &model.package_pins {
                    let e = banks.entry(pin.bank).or_insert((
                        pin.x,
                        pin.x,
                        pin.y,
                        pin.y,
                        pin.bank_rgb(),
                    ));
                    e.0 = e.0.min(pin.x);
                    e.1 = e.1.max(pin.x);
                    e.2 = e.2.min(pin.y);
                    e.3 = e.3.max(pin.y);
                }
                for (bank, (bx0, bx1, by0, by1, (br, bg, bb))) in &banks {
                    let px = origin.x + (*bx0 - x0) as f32 * cell;
                    let py = origin.y + (rows - 1 - (*by1 - y0)) as f32 * cell;
                    let pw = (*bx1 - *bx0 + 1) as f32 * cell;
                    let ph = (*by1 - *by0 + 1) as f32 * cell;
                    let brct = egui::Rect::from_min_size(egui::pos2(px, py), egui::vec2(pw, ph));
                    p.rect_filled(
                        brct.shrink(1.0),
                        2.0,
                        Color32::from_rgba_unmultiplied(*br, *bg, *bb, 90),
                    );
                    p.rect_stroke(
                        brct.shrink(1.0),
                        2.0,
                        Stroke::new(1.2, Color32::from_rgb(*br, *bg, *bb)),
                        egui::StrokeKind::Inside,
                    );
                    p.text(
                        egui::pos2(brct.left() + 3.0, brct.top() + 1.0),
                        egui::Align2::LEFT_TOP,
                        format!("BANK{bank}"),
                        egui::FontId::monospace(8.0),
                        Color32::from_rgb(0xdc, 0xe0, 0xe4),
                    );
                }
                for dx in 0..cols {
                    let x = x0 + dx;
                    let px = origin.x + dx as f32 * cell;
                    if dx % 2 == 0 {
                        p.text(
                            egui::pos2(px + cell * 0.5, rect.bottom() - 2.0),
                            egui::Align2::CENTER_BOTTOM,
                            format!("{x}"),
                            egui::FontId::monospace(8.0),
                            Color32::from_rgb(0x7a, 0x84, 0x8e),
                        );
                    }
                }
                for dy in 0..rows {
                    let y = y0 + (rows - 1 - dy);
                    let py = origin.y + dy as f32 * cell;
                    p.text(
                        egui::pos2(rect.left() + 4.0, py + cell * 0.5),
                        egui::Align2::LEFT_CENTER,
                        format!("Y{y}"),
                        egui::FontId::monospace(8.0),
                        Color32::from_rgb(0x7a, 0x84, 0x8e),
                    );
                    for dx in 0..cols {
                        let x = x0 + dx;
                        let px = origin.x + dx as f32 * cell;
                        let c = egui::pos2(px + cell * 0.5, py + cell * 0.5);
                        if let Some(pin) = model.package.pin_at(&model.package_pins, x, y) {
                            let on = selected.as_deref() == Some(pin.pin.as_str())
                                || pin.port.as_deref() == selected.as_deref();
                            let fill = if pin.port.is_some() {
                                Color32::from_rgb(0x7e, 0xc8, 0xe3)
                            } else {
                                Color32::from_rgb(0x3a, 0x44, 0x4e)
                            };
                            p.circle_filled(c, cell * 0.32, fill);
                            if on {
                                p.circle_stroke(
                                    c,
                                    cell * 0.38,
                                    Stroke::new(1.6, Color32::from_rgb(0xe5, 0xc0, 0x7b)),
                                );
                            }
                        }
                    }
                }
                if let Some(pos) = resp.hover_pos() {
                    let dx = ((pos.x - origin.x) / cell).floor() as i32;
                    let dy = ((pos.y - origin.y) / cell).floor() as i32;
                    if dx >= 0 && dy >= 0 && (dx as u32) < cols && (dy as u32) < rows {
                        let x = x0 + dx as u32;
                        let y = y0 + (rows - 1 - dy as u32);
                        if let Some(pin) = model.package.pin_at(&model.package_pins, x, y) {
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
                    let dx = ((pos.x - origin.x) / cell).floor() as i32;
                    let dy = ((pos.y - origin.y) / cell).floor() as i32;
                    if dx >= 0 && dy >= 0 && (dx as u32) < cols && (dy as u32) < rows {
                        let x = x0 + dx as u32;
                        let y = y0 + (rows - 1 - dy as u32);
                        if let Some(pin) = model.package.pin_at(&model.package_pins, x, y) {
                            pick = Some(pin.pin.clone());
                        }
                    }
                }
            }
    }
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
                        let _ = model.exec(&format!("read_xdc {}", p.display()));
                    } else {
                        let _ = model.exec(tcl);
                    }
                    ui.close();
                }
            }
        });
    });
    ui.add_space(6.0);
    paint_constraints_tables(ui, model);
}

fn paint_constraints_tables(ui: &mut egui::Ui, model: &mut IdeModel) {
    let rows = model.constraint_rows();
    if rows.is_empty() {
        ui.label("No constraints yet.");
        return;
    }
    ui.label(format!(
        "clocks={} io_delay={} exceptions={}",
        rows.iter()
            .filter(|r| r.section == ConstraintSection::Clocks)
            .count(),
        rows.iter()
            .filter(|r| r.section == ConstraintSection::IoDelay)
            .count(),
        rows.iter()
            .filter(|r| r.section == ConstraintSection::Exception)
            .count()
    ));
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

fn paint_reports(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Reports");
    ui.add_space(6.0);
    paint_report_catalog(ui, model);
    ui.add_space(8.0);
    paint_timing_summary(ui, model);
    ui.add_space(8.0);
    paint_timing_paths(ui, model);
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
        return;
    }
    ui.add_space(4.0);
    ui.label(RichText::new("Path Summary").strong());
    let mut pick_path = None;
    let selected_path = model.selected_timing_path;
    egui::Grid::new("timing_path_summary")
        .spacing([8.0, 4.0])
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
    egui::ScrollArea::both().max_height(280.0).show(ui, |ui| {
        egui::Grid::new("timing_pin_delay")
            .spacing([8.0, 4.0])
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

fn paint_report_catalog(ui: &mut egui::Ui, model: &mut IdeModel) {
    let rows = model.report_catalog();
    let selected = model.selected_report.clone();
    let mut pick: Option<String> = None;
    egui::Grid::new("reports_catalog")
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(RichText::new("Name").strong());
            ui.label(RichText::new("Category").strong());
            ui.label(RichText::new("Status").strong());
            ui.label(RichText::new("Summary").strong());
            ui.end_row();
            for r in &rows {
                let on = selected.as_deref() == Some(r.id.as_str());
                if ui.selectable_label(on, &r.name).clicked() {
                    pick = Some(r.id.clone());
                }
                ui.label(&r.category);
                let fill = run_status_color(&r.status);
                let btn = egui::Button::new(RichText::new(&r.status).color(Color32::BLACK))
                    .fill(fill)
                    .selected(on);
                if ui.add(btn).clicked() {
                    pick = Some(r.id.clone());
                }
                if ui.selectable_label(on, &r.summary).clicked() {
                    pick = Some(r.id.clone());
                }
                ui.end_row();
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
        if ui.button("Apply group_path extra weight 2").clicked() {
            let _ = model.exec(
                "group_path -name extra -weight 2 -from [get_ports clk] -to [get_ports led]",
            );
        }
    });
    let report = model.timing_summary();
    if report.clocks.is_empty() {
        ui.label("no clocks — create_clock / report_timing_summary");
        return;
    }
    ui.add_space(4.0);
    ui.label(RichText::new("Design Timing Summary").strong());
    egui::Grid::new("timing_summary_design")
        .spacing([12.0, 4.0])
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
        egui::Grid::new(format!("timing_summary_{}", kind.as_str()))
            .spacing([8.0, 4.0])
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
        ui.label("no clocks — create_clock / report_clock_interaction");
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
        ui.label("no clocks — create_clock / report_cdc");
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
        ui.label("no clocks — create_clock / report_clock_networks");
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
        ui.label("no design — synth / report_power");
        return;
    }
    ui.label(format!(
        "part={} VOLTAGE_MV={} TEMP_C={} F_MHZ={}",
        report.part, report.voltage_mv, report.temperature_c, report.f_mhz
    ));
    let selected_pwr = model.selected_power.clone();
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
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
    let max_uw = report.total_uw.max(1);
    egui::Grid::new("power_rails")
        .spacing([8.0, 4.0])
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
                let (rect, _) = ui.allocate_exact_size(egui::vec2(120.0, 12.0), Sense::hover());
                ui.painter()
                    .rect_filled(rect, 2.0, Color32::from_rgb(0x2b, 0x32, 0x3a));
                let fill = rect.with_max_x(rect.left() + rect.width() * frac.clamp(0.0, 1.0));
                ui.painter()
                    .rect_filled(fill, 2.0, Color32::from_rgb(0x7e, 0xc8, 0xe3));
                ui.end_row();
            }
        });
    if let Some(rail) = pick {
        let _ = model.select_power(&rail);
    }
    ui.add_space(6.0);
    ui.label(RichText::new("Utilization Details").strong());
    let blocks = model.power_block_rows();
    let mut pick_blk: Option<String> = None;
    egui::Grid::new("power_blocks")
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(RichText::new("Block").strong());
            ui.label(RichText::new("Used").strong());
            ui.label(RichText::new("Available").strong());
            ui.end_row();
            for (name, used, avail, rail) in &blocks {
                let on = selected_pwr.as_deref() == Some(*rail)
                    || selected.as_deref() == Some(name.as_str());
                if ui.selectable_label(on, name).clicked() {
                    pick_blk = Some((*rail).into());
                }
                ui.label(used.to_string());
                ui.label(avail.to_string());
                ui.end_row();
            }
        });
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
    });
    ui.add_space(6.0);
    if model.tree.top.is_none() {
        ui.label("no design — synth / report_methodology");
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
    egui::ScrollArea::both().show(ui, |ui| {
        egui::Grid::new("methodology_table")
            .spacing([8.0, 4.0])
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

fn bitstream_block_color(block: &str) -> Color32 {
    match block {
        "CLB_IO_CLK" => Color32::from_rgb(0x3d, 0xb8, 0x7a),
        "DSP" => Color32::from_rgb(0x90, 0x60, 0xc0),
        "BRAM" => Color32::from_rgb(0xf0, 0xc0, 0x40),
        "IOB" => Color32::from_rgb(0x5a, 0xb0, 0xe0),
        _ => Color32::from_rgb(0x9a, 0xa4, 0xae),
    }
}

fn paint_bitstream(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Bitstream");
    ui.weak(
        "helion-bits FAR table — configured frames (block / major / minor / ones), not a hash dump",
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Generate Bitstream").clicked() {
            let _ = model.exec("write_bitstream");
        }
        if ui.button("Report bitstream").clicked() {
            let _ = model.exec("report_bitstream");
        }
    });
    ui.add_space(6.0);
    let report = model.bitstream_report();
    if report.frames == 0 && report.bytes == 0 {
        ui.label("no bitstream — run Bitstream");
        return;
    }
    ui.label(format!(
        "idcode={:#010x} hash={:#010x} bytes={} frames={} configured={}",
        report.idcode, report.hash, report.bytes, report.frames, report.configured
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
        ui.label("no DRC — run Place/Route");
        return;
    }
    let report = model.drc.clone().unwrap_or_else(|| model.drc_report());
    ui.label(format!(
        "violations={} errors={}",
        report.violations.len(),
        report.error_count()
    ));
    let selected_drc = model.selected_drc.clone();
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    let mut pick_obj: Option<String> = None;
    egui::ScrollArea::both().show(ui, |ui| {
        egui::Grid::new("drc_table")
            .spacing([8.0, 4.0])
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
                    ui.label("no violations");
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
        ui.label("no placed design — run Place");
        return;
    }
    ui.label(format!("part={}", report.part));
    let selected_util = model.selected_utilization.clone();
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    let max_avail = report
        .occupancy
        .iter()
        .map(|r| r.available.max(1))
        .max()
        .unwrap_or(1) as f32;
    egui::Grid::new("utilization_occupancy")
        .spacing([8.0, 4.0])
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
                let bar_w = 160.0 * (row.available as f32 / max_avail).clamp(0.25, 1.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(bar_w, 12.0), Sense::hover());
                ui.painter()
                    .rect_filled(rect, 2.0, Color32::from_rgb(0x2b, 0x32, 0x3a));
                let fill = rect.with_max_x(rect.left() + rect.width() * frac.clamp(0.0, 1.0));
                ui.painter()
                    .rect_filled(fill, 2.0, Color32::from_rgb(0x7e, 0xc8, 0xe3));
                ui.end_row();
            }
        });
    if let Some(res) = pick {
        let _ = model.select_utilization(&res);
    }
    if !report.hierarchy.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("Hierarchical").strong());
        let mut pick_hier: Option<String> = None;
        egui::Grid::new("utilization_hierarchy")
            .spacing([8.0, 4.0])
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
        if let Some(name) = pick_hier {
            let _ = model.select_utilization_hier(&name);
        }
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
    model
        .schematic
        .set_viewport(ui.available_width(), ui.available_height().max(240.0));
    let drawing = model.schematic.drawing();
    let n_cells = drawing
        .symbols
        .iter()
        .filter(|s| !s.kind.starts_with("PORT"))
        .count();
    let n_ports = drawing
        .symbols
        .iter()
        .filter(|s| s.kind.starts_with("PORT"))
        .count();
    let n_nets = {
        let mut s = std::collections::HashSet::new();
        for w in &drawing.wires {
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
        if ui.button("Zoom Fit").clicked() {
            let _ = model.schematic_zoom_fit();
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
            ui.weak(format!("cone={root}"));
        }
        if let Some(inst) = &model.schematic.expand_inside {
            ui.weak(format!("inside={inst}"));
        }
        ui.weak(format!("zoom={:.2}", model.schematic.camera.zoom));
    });
    if !model.timing_paths.is_empty() {
        ui.label(RichText::new("Timing paths").small());
        let mut pick_path = None;
        let selected_path = model.selected_timing_path;
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
        if let Some(i) = pick_path {
            let _ = model.select_timing_path(&i.to_string());
        }
    }
    let mut pick = None;
    let mut expand = None;
    let selected = model.selected.clone();
    let cam = model.schematic.camera;
    let z = cam.zoom.max(0.05);
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let size = egui::vec2(
                (drawing.width * z + cam.pan_x.abs() + 8.0).max(ui.available_width()),
                (drawing.height * z + cam.pan_y.abs() + 8.0).max(ui.available_height().max(240.0)),
            );
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
            if ui.is_rect_visible(rect) {
                let p = ui.painter();
                p.rect_filled(rect, 0.0, Color32::from_rgb(0x12, 0x16, 0x1a));
                let o = egui::pos2(rect.min.x + cam.pan_x, rect.min.y + cam.pan_y);
                let net_col = Color32::from_rgb(0x3d, 0xb8, 0x7a);
                let gold = Color32::from_rgb(0xe5, 0xc0, 0x7b);
                let path_col = Color32::from_rgb(0xe0, 0x6c, 0x75);
                for w in &drawing.wires {
                    let pts: Vec<egui::Pos2> = w
                        .points
                        .iter()
                        .map(|(x, y)| egui::pos2(o.x + *x * z, o.y + *y * z))
                        .collect();
                    let thick = if w.highlighted {
                        4.2
                    } else if w.width > 1 {
                        3.6
                    } else {
                        1.4
                    };
                    let col = if w.highlighted { path_col } else { net_col };
                    for pair in pts.windows(2) {
                        if w.off_sheet {
                            paint_dotted(p, pair[0], pair[1], Stroke::new(thick, col));
                        } else {
                            p.line_segment([pair[0], pair[1]], Stroke::new(thick, col));
                        }
                    }
                    if let (Some(&a), Some(&b)) = (pts.first(), pts.get(1).or(pts.last())) {
                        let mid = egui::pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5 - 8.0);
                        p.text(
                            mid,
                            egui::Align2::CENTER_BOTTOM,
                            &w.net,
                            egui::FontId::monospace(9.0),
                            Color32::from_rgb(0x7e, 0xc8, 0xe3),
                        );
                    }
                }
                for sy in &drawing.symbols {
                    let r = egui::Rect::from_min_size(
                        egui::pos2(o.x + sy.x * z, o.y + sy.y * z),
                        egui::vec2(sy.w * z, sy.h * z),
                    );
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
                        if on || sy.highlighted { 2.0 } else { 1.0 },
                        if sy.highlighted {
                            path_col
                        } else if on {
                            gold
                        } else {
                            Color32::from_rgb(0x7a, 0x84, 0x8e)
                        },
                    );
                    // Ports / IOB as right-pointing triangles; LUTs and FFs as boxes.
                    if port || sy.kind == "IOB_OUT" {
                        let pts = vec![r.left_top(), r.left_bottom(), r.right_center()];
                        p.add(egui::Shape::convex_polygon(pts, fill, stroke));
                    } else {
                        p.rect_filled(r, 3.0, fill);
                        p.rect_stroke(r, 3.0, stroke, egui::StrokeKind::Inside);
                    }
                    p.text(
                        egui::pos2(r.center().x, r.top() + 3.0),
                        egui::Align2::CENTER_TOP,
                        &sy.kind,
                        egui::FontId::monospace(10.0),
                        Color32::from_rgb(0x7e, 0xc8, 0xe3),
                    );
                    p.text(
                        egui::pos2(r.center().x, r.bottom() - 3.0),
                        egui::Align2::CENTER_BOTTOM,
                        &sy.name,
                        egui::FontId::monospace(10.0),
                        Color32::from_rgb(0xdc, 0xe0, 0xe4),
                    );
                    for pin in &sy.pins {
                        let tip = egui::pos2(o.x + pin.x * z, o.y + pin.y * z);
                        let edge = if pin.output {
                            egui::pos2(r.right(), tip.y)
                        } else {
                            egui::pos2(r.left(), tip.y)
                        };
                        // Pin stub: a short line inside and outside the symbol.
                        let inner = if pin.output {
                            egui::pos2(r.right() - 10.0, tip.y)
                        } else {
                            egui::pos2(r.left() + 10.0, tip.y)
                        };
                        let stub = Color32::from_rgb(0xdc, 0xe0, 0xe4);
                        p.line_segment([inner, edge], Stroke::new(2.0, stub));
                        p.line_segment([edge, tip], Stroke::new(2.0, stub));
                        p.circle_filled(tip, 2.2, stub);
                        let nc = pin.net.is_empty();
                        let label = if nc {
                            format!("{} n/c", pin.name)
                        } else {
                            pin.name.clone()
                        };
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
                            label,
                            egui::FontId::monospace(9.0),
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
                        if lx >= sy.x
                            && lx <= sy.x + sy.w
                            && ly >= sy.y
                            && ly <= sy.y + sy.h
                        {
                            pick = Some(sy.name.clone());
                            if resp.double_clicked() && !sy.kind.starts_with("PORT") {
                                expand = Some(sy.name.clone());
                            }
                            break;
                        }
                    }
                }
            }
        });
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

fn paint_device(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Device");
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("CLB")
                .small()
                .color(Color32::from_rgb(0x3d, 0xb8, 0x7a)),
        );
        ui.label(
            RichText::new("IOB")
                .small()
                .color(Color32::from_rgb(0x5b, 0x9b, 0xd5)),
        );
        ui.label(
            RichText::new("BRAM")
                .small()
                .color(Color32::from_rgb(0xb0, 0x7c, 0xe8)),
        );
        ui.label(
            RichText::new("placed LUT")
                .small()
                .color(Color32::from_rgb(0xc8, 0xf0, 0xd8)),
        );
        ui.label(
            RichText::new("placed I/O")
                .small()
                .color(Color32::from_rgb(0x7e, 0xc8, 0xe3)),
        );
        ui.label(
            RichText::new("clock region")
                .small()
                .color(Color32::from_rgb(0xb0, 0x7c, 0xe8)),
        );
        ui.label(
            RichText::new("route")
                .small()
                .color(Color32::from_rgb(0x3d, 0xb8, 0x7a)),
        );
        ui.label(
            RichText::new("pblock")
                .small()
                .color(Color32::from_rgb(0xe5, 0x9a, 0x3c)),
        );
    });
    egui::ScrollArea::vertical()
        .id_salt("device_tables")
        .auto_shrink([false, true])
        .max_height(chrome::DEVICE_TABLES_MAX_HEIGHT)
        .show(ui, |ui| {
            paint_pblocks_table(ui, model);
            paint_clock_regions(ui, model);
            paint_device_routes(ui, model);
        });
    ui.separator();
    let cols = model.device.cols.max(1);
    let rows = model.device.rows.max(1);
    let x0 = model.device.x0;
    let y0 = model.device.y0;
    let avail = ui.available_size();
    let view_h = avail.y.max(chrome::DRAWING_MIN_HEIGHT);
    let view_w = avail.x.max(80.0);
    let cell = chrome::floorplan_fit_cell(cols, rows, view_w, view_h);
    let mut pick_site: Option<(u32, u32)> = None;
    let mut pick_region: Option<String> = None;
    let mut click_pblock: Option<String> = None;
    {
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(view_w, view_h),
                Sense::click(),
            );
            if ui.is_rect_visible(rect) {
                let origin = egui::pos2(rect.left() + 24.0, rect.top() + 4.0);
                let p = ui.painter();
                p.rect_filled(rect, 0.0, Color32::from_rgb(0x12, 0x16, 0x1a));
                for dx in 0..cols {
                    let x = x0 + dx;
                    let px = origin.x + dx as f32 * cell;
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
                    let py = origin.y + dy as f32 * cell;
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
                        let px = origin.x + dx as f32 * cell;
                        let tile = egui::Rect::from_min_size(
                            egui::pos2(px + 0.5, py + 0.5),
                            egui::vec2(cell - 1.0, cell - 1.0),
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
                                Stroke::new(1.5, Color32::from_rgb(0xe5, 0xc0, 0x7b)),
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
                    let px = origin.x + (cr.x0 - x0) as f32 * cell;
                    let py = origin.y + (rows - 1 - (cr.y1 - y0)) as f32 * cell;
                    let pw = cr.cols() as f32 * cell;
                    let ph = cr.rows() as f32 * cell;
                    let rr = egui::Rect::from_min_size(egui::pos2(px, py), egui::vec2(pw, ph));
                    let on = model.selected.as_deref() == Some(cr.name.as_str());
                    p.rect_stroke(
                        rr,
                        0.0,
                        Stroke::new(if on { 3.0 } else { 2.0 }, if on { gold } else { purple }),
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
                    let px = origin.x + (pb.x0 - x0) as f32 * cell;
                    let py = origin.y + (rows - 1 - (pb.y1 - y0)) as f32 * cell;
                    let pw = pb.cols() as f32 * cell;
                    let ph = pb.rows() as f32 * cell;
                    let rr = egui::Rect::from_min_size(egui::pos2(px, py), egui::vec2(pw, ph));
                    let on = model.selected.as_deref() == Some(pb.name.as_str());
                    p.rect_stroke(
                        rr,
                        0.0,
                        Stroke::new(if on { 3.0 } else { 2.0 }, if on { gold } else { amber }),
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
                    let thick = if rt.highlighted { 2.6 } else { 1.7 };
                    let mut pts = Vec::new();
                    for &(x, y) in &rt.tiles {
                        let dx = x.saturating_sub(x0);
                        let dy = rows.saturating_sub(1).saturating_sub(y.saturating_sub(y0));
                        let cx = origin.x + dx as f32 * cell + cell / 2.0;
                        let cy = origin.y + dy as f32 * cell + cell / 2.0;
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
                    let dx = ((pos.x - origin.x) / cell).floor() as i32;
                    let dy = ((pos.y - origin.y) / cell).floor() as i32;
                    if dx >= 0 && dy >= 0 && (dx as u32) < cols && (dy as u32) < rows {
                        let x = x0 + dx as u32;
                        let y = y0 + (rows - 1 - dy as u32);
                        let mut tip = String::new();
                        if let Some(pb) = model.pblocks.iter().find(|p| p.contains(x, y)) {
                            tip.push_str(&format!(
                                "{}  {}  frames={}",
                                pb.name,
                                pb.range_text(),
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
                    let origin = egui::pos2(rect.left() + 24.0, rect.top() + 4.0);
                    let dx = ((pos.x - origin.x) / cell).floor() as i32;
                    let dy = ((pos.y - origin.y) / cell).floor() as i32;
                    if dx >= 0 && dy >= 0 && (dx as u32) < cols && (dy as u32) < rows {
                        let x = x0 + dx as u32;
                        let y = y0 + (rows - 1 - dy as u32);
                        let mut header = false;
                        if let Some(pb) = model.pblocks.iter().find(|p| p.contains(x, y)) {
                            let py = origin.y + (rows - 1 - (pb.y1 - y0)) as f32 * cell;
                            if pos.y - py <= 14.0 {
                                click_pblock = Some(pb.name.clone());
                                header = true;
                            }
                        }
                        if !header {
                            if let Some(cr) = model.device.clock_region_at(x, y) {
                                let py = origin.y + (rows - 1 - (cr.y1 - y0)) as f32 * cell;
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
    }
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
    ui.label(RichText::new("Clock Regions").strong());
    let regions = model.device.clock_regions.clone();

    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
    if regions.is_empty() {
        ui.weak("no clock regions — HAD die");
        return;
    }
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
        ui.weak("no routes — Route / device_routes");
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
    let rows = model.source_line_rows().to_vec();
    let selected = model.selected_source_line;
    let markers = model.editor_markers();
    let mut pick_line: Option<String> = None;
    let mut pick_mark: Option<String> = None;
    egui::ScrollArea::both()
        .id_salt("ug893_text_editor")
        .show(ui, |ui| {
            egui::Grid::new("ug893_text_editor_table")
                .spacing([8.0, 2.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Marker").strong());
                    ui.label(RichText::new("Line").strong());
                    ui.label(RichText::new("Kind").strong());
                    ui.label(RichText::new("Text").strong());
                    ui.end_row();
                    if rows.is_empty() {
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("No sources yet.");
                        ui.end_row();
                    } else {
                        for r in &rows {
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
                                if mk.map(|m| m.kind.as_str()) == Some("bookmark")
                                    || mk.is_none()
                                {
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
                            ui.end_row();
                        }
                    }
                });
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
            let _ = model.sim_run(16);
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
    data_scroll("ug900_memory")
        .show(ui, |ui| {
            egui::Grid::new("ug900_memory_table")
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Name").strong());
                    ui.label(RichText::new("Type").strong());
                    ui.label(RichText::new("Addr").strong());
                    ui.label(RichText::new("Data").strong());
                    ui.label(RichText::new("Width").strong());
                    ui.label(RichText::new("Depth").strong());
                    ui.end_row();
                    if blocks.is_empty() {
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("no memories — sim_run");
                        ui.end_row();
                    } else {
                        for (i, m) in blocks.iter().enumerate() {
                            let on = selected.as_deref() == Some(m.name.as_str());
                            if ui.selectable_label(on, &m.name).clicked() {
                                pick = Some(i.to_string());
                            }
                            if ui.selectable_label(on, m.type_cell()).clicked() {
                                pick = Some(i.to_string());
                            }
                            if ui.selectable_label(on, "0").clicked() {
                                pick = Some(i.to_string());
                            }
                            if ui.selectable_label(on, m.data_cell()).clicked() {
                                pick = Some(i.to_string());
                            }
                            ui.label(m.width.to_string());
                            ui.label(m.depth().to_string());
                            ui.end_row();
                        }
                    }
                });
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
        ui.weak("select a memory to view Addr/Data");
    } else {
        data_scroll("ug900_memory_words")
            .show(ui, |ui| {
                egui::Grid::new("ug900_memory_words_table")
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Addr").strong());
                        ui.label(RichText::new("Data").strong());
                        ui.end_row();
                        for r in &words {
                            let on = sel_addr == Some(r.addr);
                            if ui.selectable_label(on, r.addr.to_string()).clicked() {
                                pick_addr = Some(r.addr);
                            }
                            if ui.selectable_label(on, &r.data).clicked() {
                                pick_addr = Some(r.addr);
                            }
                            ui.end_row();
                        }
                    });
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
            let _ = model.sim_run(16);
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
                        ui.label("no breakpoints — add_bp");
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
                        ui.label("no forces — add_force");
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
            let _ = model.sim_run(16);
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
                        ui.label("no locals — sim_run");
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
        ui.label(
            RichText::new(format!(
                "timescale {} ps/cycle  cursor t={} ({} ps)  {a_txt}  {b_txt}  {d_txt}  markers={} virtual_bus={}",
                model.wave.timescale_ps,
                model.wave.cursor,
                model.wave.time_ps(model.wave.cursor),
                model.wave.markers.len(),
                model.wave.virtual_buses.len()
            ))
            .weak()
            .monospace(),
        );
        if ui.small_button("Cursor A").clicked() {
            let _ = model.set_wave_ab_cursor("A");
        }
        if ui.small_button("Cursor B").clicked() {
            let _ = model.set_wave_ab_cursor("B");
        }
        if ui.small_button("Add marker").clicked() {
            let n = model.wave.markers.len() + 1;
            let _ = model.add_wave_marker(&format!("M{n}"));
        }
        if ui.small_button("Virtual bus led+cnt").clicked() {
            let _ = model.add_wave_virtual_bus("vb led cnt");
        }
    });
    paint_wave_markers(ui, model);
    paint_wave_cursors(ui, model);
    paint_virtual_buses(ui, model);
    if model.wave.traces.is_empty() {
        ui.weak("Run Simulation (simulation_settings runtime) after Bitstream.");
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
                Stroke::new(1.0, Color32::from_rgb(0x5a, 0x64, 0x6e)),
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
                Stroke::new(1.2, Color32::from_rgb(0xc0, 0x78, 0xc8)),
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
                Stroke::new(1.4, Color32::from_rgb(0xe0, 0x6c, 0x75)),
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
                Stroke::new(1.4, Color32::from_rgb(0x56, 0xb6, 0xc2)),
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

    for t in &model.wave.traces {
        ui.horizontal(|ui| {
            ui.set_min_height(36.0);
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
                .small_button(if t.style == WaveStyle::Analog {
                    "Analog"
                } else {
                    "Digital"
                })
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
                .small_button(if t.radix == WaveRadix::Hexadecimal {
                    "Hex"
                } else {
                    "Bin"
                })
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
                egui::vec2(ui.available_width(), 32.0),
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
        ui.weak("no markers — add_wave_marker after sim_run");
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
        ui.weak("no virtual bus — add_wave_virtual_bus after sim_run");
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
                        Stroke::new(1.5, green),
                    );
                }
                p.line_segment(
                    [egui::pos2(x0, y), egui::pos2(x1, y)],
                    Stroke::new(1.5, green),
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
                p.line_segment([w[0], w[1]], Stroke::new(1.6, green));
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
        Stroke::new(1.0, Color32::from_rgb(0xe5, 0xc0, 0x7b)),
    );
    if let Some(a) = cursor_a {
        let ax = rect.left() + dx * (a as f32 + 0.5);
        p.line_segment(
            [egui::pos2(ax, rect.top()), egui::pos2(ax, rect.bottom())],
            Stroke::new(1.2, Color32::from_rgb(0xe0, 0x6c, 0x75)),
        );
    }
    if let Some(b) = cursor_b {
        let bx = rect.left() + dx * (b as f32 + 0.5);
        p.line_segment(
            [egui::pos2(bx, rect.top()), egui::pos2(bx, rect.bottom())],
            Stroke::new(1.2, Color32::from_rgb(0x56, 0xb6, 0xc2)),
        );
    }
    for m in markers {
        let mx = rect.left() + dx * (m.sample as f32 + 0.5);
        p.line_segment(
            [egui::pos2(mx, rect.top()), egui::pos2(mx, rect.bottom())],
            Stroke::new(1.0, Color32::from_rgb(0xc0, 0x78, 0xc8)),
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

fn paint_hw(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Hardware Manager");
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
    let report = model.hw_stat_report();
    if !report.open {
        ui.label("no hardware — open_hw_manager");
    } else {
        ui.label(format!(
            "target={} part={} idcode={:#010x} ir={:#04x} programmed={} word={}",
            report.target,
            report.part,
            report.idcode,
            report.ir,
            u8::from(report.programmed),
            report.word_hex()
        ));
        let selected = model.selected.clone();
        let mut pick: Option<String> = None;
        egui::Grid::new("hw_stat_table")
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Bit").strong());
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Value").strong());
                ui.label(RichText::new("Description").strong());
                ui.end_row();
                for b in &report.bits {
                    let on = selected.as_deref() == Some(b.name.as_str());
                    let fill = hw_stat_bit_color(&b.name, b.value);
                    ui.monospace(b.bit.to_string());
                    let btn = egui::Button::new(RichText::new(&b.name).color(Color32::BLACK))
                        .fill(fill)
                        .selected(on);
                    if ui.add(btn).clicked() {
                        pick = Some(b.name.clone());
                    }
                    ui.label(if b.value { "1" } else { "0" });
                    ui.label(&b.description);
                    ui.end_row();
                }
            });
        if let Some(name) = pick {
            let _ = model.select_hw_stat(&name);
        }
    }
    ui.separator();
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
        if ui.button("Arm / Capture cnt_3").clicked() {
            let _ = model.exec("ila_arm cnt_3");
        }
    });
    let samples = model.ila_sample_rows();
    if samples.is_empty() {
        ui.weak("no capture — Arm / Capture a marked net");
    } else {
        ui.label(format!(
            "probe={} window={} trigger={} trigger_at={}",
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
                .unwrap_or_else(|| "-".into())
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
}

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
    paint_ip_catalog(ui, model);
    let drawing = model
        .block_design
        .as_ref()
        .map(|bd| bd.drawing(&model.ip_catalog));
    let Some(drawing) = drawing else {
        ui.weak("Create Block Design to place Helion-MM IP on the canvas.");
        return;
    };
    ui.label(format!(
        "BD {}  {} IP  {} nets  ok={}",
        model.block_design.as_ref().map(|b| b.name.as_str()).unwrap_or("-"),
        drawing
            .symbols
            .iter()
            .filter(|s| s.kind != "PORT_IN" && s.kind != "INTERCONNECT")
            .count(),
        drawing.wires.len(),
        model.block_design.as_ref().map(|b| b.ok).unwrap_or(false)
    ));
    if !drawing.addresses.is_empty() {
        ui.add_space(4.0);
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
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let size = egui::vec2(
                drawing.width.max(ui.available_width()),
                drawing.height.max(ui.available_height().max(200.0)),
            );
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            if !ui.is_rect_visible(rect) {
                return;
            }
            let p = ui.painter();
            p.rect_filled(rect, 0.0, Color32::from_rgb(0x12, 0x16, 0x1a));
            let o = rect.min;
            let net = Color32::from_rgb(0x3d, 0xb8, 0x7a);
            let mm = Color32::from_rgb(0x7e, 0xc8, 0xe3);
            for w in &drawing.wires {
                let col = if w.net == "Helion-MM" { mm } else { net };
                let thick = if w.net == "Helion-MM" { 3.4 } else { 1.6 };
                let pts: Vec<egui::Pos2> = w
                    .points
                    .iter()
                    .map(|(x, y)| egui::pos2(o.x + *x, o.y + *y))
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
            for sy in &drawing.symbols {
                let r = egui::Rect::from_min_size(
                    egui::pos2(o.x + sy.x, o.y + sy.y),
                    egui::vec2(sy.w, sy.h),
                );
                if sy.kind == "PORT_IN" {
                    let pts = vec![r.left_top(), r.left_bottom(), r.right_center()];
                    p.add(egui::Shape::convex_polygon(
                        pts,
                        Color32::from_rgb(0x1e, 0x3a, 0x55),
                        Stroke::new(1.0, Color32::from_rgb(0x7a, 0x84, 0x8e)),
                    ));
                } else {
                    let fill = if sy.kind == "INTERCONNECT" {
                        Color32::from_rgb(0x24, 0x2e, 0x3a)
                    } else {
                        Color32::from_rgb(0x2a, 0x32, 0x24)
                    };
                    p.rect_filled(r, 3.0, fill);
                    p.rect_stroke(
                        r,
                        3.0,
                        Stroke::new(1.0, Color32::from_rgb(0x7a, 0x84, 0x8e)),
                        egui::StrokeKind::Inside,
                    );
                }
                p.text(
                    egui::pos2(r.center().x, r.top() + 4.0),
                    egui::Align2::CENTER_TOP,
                    &sy.kind,
                    egui::FontId::monospace(10.0),
                    Color32::from_rgb(0x7e, 0xc8, 0xe3),
                );
                p.text(
                    egui::pos2(r.center().x, r.bottom() - 4.0),
                    egui::Align2::CENTER_BOTTOM,
                    &sy.name,
                    egui::FontId::monospace(10.0),
                    Color32::from_rgb(0xdc, 0xe0, 0xe4),
                );
                if !sy.bus.is_empty() {
                    p.text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        &sy.bus,
                        egui::FontId::monospace(9.0),
                        Color32::from_rgb(0x9a, 0xa4, 0xae),
                    );
                }
                for pin in &sy.pins {
                    let tip = egui::pos2(o.x + pin.x, o.y + pin.y);
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
                            Stroke::new(1.0, Color32::from_rgb(0xdc, 0xe0, 0xe4)),
                            egui::StrokeKind::Outside,
                        );
                        p.line_segment([edge, tip], Stroke::new(3.4, mm));
                    } else {
                        p.line_segment([edge, tip], Stroke::new(1.6, net));
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
        });
    paint_bd_hdl(ui, model);
}

fn paint_ip_catalog(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.add_space(4.0);
    ui.label(
        RichText::new("IP Catalog — helion-ipxact VLNV table (Helion-MM/ST), not a collapsing dump")
            .strong(),
    );
    let rows = model.ip_catalog_rows();
    ui.label(format!("cores={}", rows.len()));
    let selected = model.selected_ip.clone();
    let mut pick: Option<String> = None;
    egui::Grid::new("ip_catalog_table")
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(RichText::new("Name").strong());
            ui.label(RichText::new("VLNV").strong());
            ui.label(RichText::new("Bus").strong());
            ui.label(RichText::new("Status").strong());
            ui.end_row();
            for r in &rows {
                let on = selected.as_deref() == Some(r.name.as_str());
                if ui.selectable_label(on, &r.name).clicked() {
                    pick = Some(r.name.clone());
                }
                ui.label(&r.vlnv);
                ui.label(&r.bus);
                ui.label(&r.status);
                ui.end_row();
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
