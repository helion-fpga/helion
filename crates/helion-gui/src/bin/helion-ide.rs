//! Helion IDE — Vivado-class desktop window over the real Session engines.
//!
//! `--version` / `--doctor` never open a window (so they work headless and on CI).
//! The GUI paints [`helion_gui::IdeModel`]; every button and the Tcl box call into
//! that model, which is what the unit tests already prove is not a no-op.

use eframe::egui::{self, Color32, RichText, Sense, Stroke};
use helion_gui::{
    doctor, BottomTab, CdcSeverity, ClockRelation, ConstraintSection, DrcSeverity, FlowStep,
    IdeModel, IlaTrigger, LayoutKind, MethodologySeverity, MsgSeverity, NavSection, PathGroupKind,
    StepState, WaveRadix, WaveStyle, WorkspaceTab,
};
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;

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
        let mut o = String::new();
        if let Some(top) = &ide.tree.top {
            o.push_str(&format!("top={top}"));
        }
        for (c, k) in &ide.tree.cells {
            o.push_str(&format!(" cell={c}:{k}"));
        }
        for n in &ide.tree.nets {
            o.push_str(&format!(" net={n}"));
        }
        return Ok(o);
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
            .with_title("Helion Design Suite"),
        ..Default::default()
    };
    eframe::run_native(
        "helion-ide",
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
}

impl HelionIde {
    fn new() -> Self {
        Self {
            model: IdeModel::new(),
            tree_filter: String::new(),
        }
    }
}

impl eframe::App for HelionIde {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        paint_menu_rail(ctx, &mut self.model);
        paint_bottom(ctx, &mut self.model);
        paint_navigator(ctx, &mut self.model);
        match self.model.layout {
            LayoutKind::Simulation => paint_sim_side(ctx, &mut self.model),
            LayoutKind::Default => paint_sources_netlist(ctx, &mut self.model, &mut self.tree_filter),
        }
        paint_properties(ctx, &mut self.model);
        egui::CentralPanel::default().show(ctx, |ui| {
            paint_workspace(ui, &mut self.model);
        });
    }
}

fn paint_menu_rail(ctx: &egui::Context, model: &mut IdeModel) {
    egui::TopBottomPanel::top("rail")
        .exact_height(56.0)
        .show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Helion Design Suite")
                        .strong()
                        .size(18.0)
                        .color(Color32::from_rgb(0x7e, 0xc8, 0xe3)),
                );
                ui.label(
                    RichText::new(model.part())
                        .monospace()
                        .color(Color32::from_rgb(0xb0, 0xb8, 0xc0)),
                );
                ui.separator();
                ui.label(RichText::new("Layout").weak());
                egui::ComboBox::from_id_salt("layout")
                    .selected_text(model.layout.label())
                    .show_ui(ui, |ui| {
                        for l in LayoutKind::ALL {
                            if ui
                                .selectable_label(model.layout == l, l.label())
                                .clicked()
                            {
                                let _ = model.set_layout(l);
                            }
                        }
                    });
                ui.separator();
                ui.label(RichText::new("Flow").weak());
                for step in FlowStep::ALL {
                    flow_button(ui, model, step);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    if ui.button("Open hier.sv").clicked() {
                        let p = helion_device::Device::examples_dir().join("hier.sv");
                        let _ = model.open_source(&p);
                    }
                    if ui.button("Open blinky.sv").clicked() {
                        let p = helion_device::Device::examples_dir().join("blinky.sv");
                        let _ = model.open_source(&p);
                    }
                    if ui.button("Open counter.sv").clicked() {
                        let p = helion_device::Device::examples_dir().join("counter.sv");
                        let _ = model.open_source(&p);
                    }
                });
            });
        });
}

fn paint_nav_section(ui: &mut egui::Ui, model: &mut IdeModel, sec: NavSection) {
    let on = model.nav == sec;
    let fill = if on {
        Color32::from_rgb(0x1f, 0x4a, 0x38)
    } else {
        Color32::from_rgb(0x2b, 0x32, 0x3a)
    };
    let resp = ui.add(
        egui::Button::new(RichText::new(sec.label()).color(Color32::from_rgb(
            0xdc, 0xe0, 0xe4,
        )))
        .fill(fill)
        .min_size(egui::vec2(ui.available_width(), 22.0)),
    );
    if resp.clicked() {
        let _ = model.set_nav(sec);
    }
    resp.on_hover_text(format!("nav {}", sec.tcl()));
}

fn paint_nav_actions(ui: &mut egui::Ui, model: &mut IdeModel, sec: NavSection) {
    let mut run = None;
    ui.indent(format!("nav_actions_{}", sec.tcl()), |ui| {
        for a in sec.actions() {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(a.label)
                            .small()
                            .color(Color32::from_rgb(0xc8, 0xf0, 0xd8)),
                    )
                    .fill(Color32::from_rgb(0x1a, 0x28, 0x22))
                    .min_size(egui::vec2(ui.available_width(), 18.0)),
                )
                .on_hover_text(a.tcl)
                .clicked()
            {
                run = Some(a.tcl);
            }
        }
    });
    if let Some(tcl) = run {
        let _ = model.exec(tcl);
    }
}

fn paint_navigator(ctx: &egui::Context, model: &mut IdeModel) {
    egui::SidePanel::left("navigator")
        .resizable(true)
        .default_width(200.0)
        .min_width(160.0)
        .show(ctx, |ui| {
            ui.label(RichText::new("Flow Navigator").strong().size(14.0));
            ui.weak("UG949 UltraFast tree — stages on Helion engines");
            ui.add_space(4.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.collapsing("I/O AND DEVICE PLANNING", |ui| {
                    paint_nav_section(ui, model, NavSection::BoardDevice);
                    paint_nav_actions(ui, model, NavSection::BoardDevice);
                });
                ui.collapsing("PROJECT MANAGER", |ui| {
                    paint_nav_section(ui, model, NavSection::ProjectManager);
                    paint_nav_actions(ui, model, NavSection::ProjectManager);
                    paint_nav_section(ui, model, NavSection::IpIntegrator);
                    paint_nav_actions(ui, model, NavSection::IpIntegrator);
                });
                ui.collapsing("SIMULATION", |ui| {
                    paint_nav_section(ui, model, NavSection::Simulation);
                    paint_nav_actions(ui, model, NavSection::Simulation);
                });
                ui.collapsing("RTL ANALYSIS", |ui| {
                    paint_nav_section(ui, model, NavSection::RtlAnalysis);
                    paint_nav_actions(ui, model, NavSection::RtlAnalysis);
                });
                ui.collapsing("SYNTHESIS", |ui| {
                    paint_nav_section(ui, model, NavSection::Synthesis);
                    paint_nav_actions(ui, model, NavSection::Synthesis);
                });
                ui.collapsing("IMPLEMENTATION", |ui| {
                    paint_nav_section(ui, model, NavSection::Implementation);
                    paint_nav_actions(ui, model, NavSection::Implementation);
                    paint_nav_section(ui, model, NavSection::TimingAnalysis);
                    paint_nav_actions(ui, model, NavSection::TimingAnalysis);
                });
                ui.collapsing("PROGRAM AND DEBUG", |ui| {
                    paint_nav_section(ui, model, NavSection::ProgramDebug);
                    paint_nav_actions(ui, model, NavSection::ProgramDebug);
                });
            });
        });
}

fn paint_sources_netlist(ctx: &egui::Context, model: &mut IdeModel, tree_filter: &mut String) {
    egui::SidePanel::left("tree")
        .resizable(true)
        .default_width(240.0)
        .min_width(160.0)
        .show(ctx, |ui| {
            ui.label(RichText::new("Sources").strong());
            if model.tree.sources.is_empty() {
                ui.weak("Open an example, or `read_sv path` in the console.");
            }
            for src in &model.tree.sources {
                ui.monospace(src.rsplit('/').next().unwrap_or(src));
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Netlist").strong());
                if let Some(top) = &model.tree.top {
                    ui.label(RichText::new(top).monospace().italics());
                }
            });
            ui.add(
                egui::TextEdit::singleline(tree_filter)
                    .hint_text("filter cells/nets")
                    .desired_width(f32::INFINITY),
            );
            let filt = tree_filter.to_ascii_lowercase();
            let selected = model.selected.clone();
            let mut pick: Option<String> = None;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.collapsing(format!("Cells ({})", model.tree.cells.len()), |ui| {
                        for (name, kind) in &model.tree.cells {
                            if !filt.is_empty()
                                && !name.to_ascii_lowercase().contains(&filt)
                                && !kind.to_ascii_lowercase().contains(&filt)
                            {
                                continue;
                            }
                            let on = selected.as_deref() == Some(name.as_str());
                            ui.horizontal(|ui| {
                                if ui.selectable_label(on, RichText::new(name).monospace()).clicked()
                                {
                                    pick = Some(name.clone());
                                }
                                ui.label(
                                    RichText::new(kind)
                                        .small()
                                        .color(Color32::from_rgb(0x7e, 0xc8, 0xe3)),
                                );
                            });
                        }
                    });
                    ui.collapsing(format!("Nets ({})", model.tree.nets.len()), |ui| {
                        for n in &model.tree.nets {
                            if !filt.is_empty() && !n.to_ascii_lowercase().contains(&filt) {
                                continue;
                            }
                            let on = selected.as_deref() == Some(n.as_str());
                            if ui.selectable_label(on, RichText::new(n).monospace()).clicked() {
                                pick = Some(n.clone());
                            }
                        }
                    });
                });
            if let Some(id) = pick {
                model.select(&id);
            }
        });
}

fn paint_sim_side(ctx: &egui::Context, model: &mut IdeModel) {
    egui::SidePanel::left("scopes")
        .resizable(true)
        .default_width(220.0)
        .show(ctx, |ui| {
            ui.label(RichText::new("Scopes").strong());
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
            let mut pick_scope = None;
            for s in &model.scopes {
                let on = model.selected_scope.as_deref() == Some(s.name.as_str());
                if ui
                    .selectable_label(on, format!("{} ({})", s.name, s.kind))
                    .clicked()
                {
                    pick_scope = Some(s.name.clone());
                }
            }
            if let Some(name) = pick_scope {
                let _ = model.select_scope(&name);
            }
            ui.separator();
            ui.label(RichText::new("Objects").strong());
            ui.weak("click to add_wave");
            let mut add = None;
            for o in &model.objects {
                if ui
                    .selectable_label(false, format!("{} = {}", o.name, o.value))
                    .clicked()
                {
                    add = Some(o.name.clone());
                }
            }
            if let Some(name) = add {
                let _ = model.add_wave(&name);
            }
        });
}

fn paint_properties(ctx: &egui::Context, model: &mut IdeModel) {
    egui::SidePanel::right("properties")
        .resizable(true)
        .default_width(220.0)
        .show(ctx, |ui| {
            ui.label(RichText::new("Properties").strong());
            if model.properties.is_empty() {
                ui.weak("Select a cell, net, or port.");
            }
            for (k, v) in &model.properties {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(k).weak());
                    ui.monospace(v);
                });
            }
        });
}

fn paint_bottom(ctx: &egui::Context, model: &mut IdeModel) {
    egui::TopBottomPanel::bottom("console")
        .resizable(true)
        .default_height(200.0)
        .min_height(100.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for tab in [BottomTab::Tcl, BottomTab::Messages, BottomTab::Log] {
                    let label = match tab {
                        BottomTab::Tcl => "Tcl Console",
                        BottomTab::Messages => "Messages",
                        BottomTab::Log => "Log",
                    };
                    if ui
                        .selectable_label(model.bottom_tab == tab, label)
                        .clicked()
                    {
                        model.bottom_tab = tab;
                    }
                }
                ui.label(
                    RichText::new(&model.status)
                        .monospace()
                        .color(Color32::from_rgb(0x9a, 0xa4, 0xae)),
                );
            });
            match model.bottom_tab {
                BottomTab::Tcl => {
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
                    let mut pick_line = None;
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .max_height(ui.available_height() - 28.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for (i, line) in model.console.iter().enumerate() {
                                let color = if line.ok {
                                    Color32::from_rgb(0xc8, 0xd0, 0xd8)
                                } else {
                                    Color32::from_rgb(0xe0, 0x6c, 0x75)
                                };
                                let on = selected == Some(i);
                                let hit = hits.contains(&i);
                                let cmd_col = if on {
                                    Color32::from_rgb(0xe5, 0xc0, 0x7b)
                                } else if hit {
                                    Color32::from_rgb(0x7e, 0xc8, 0xe3)
                                } else {
                                    Color32::from_rgb(0x5e, 0xa8, 0xc3)
                                };
                                if ui
                                    .selectable_label(
                                        on,
                                        RichText::new(format!("helion% {}", line.cmd))
                                            .monospace()
                                            .color(cmd_col),
                                    )
                                    .clicked()
                                {
                                    pick_line = Some(i);
                                }
                                if !line.out.is_empty() {
                                    ui.monospace(RichText::new(&line.out).color(color));
                                }
                            }
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
                BottomTab::Messages => {
                    paint_messages(ui, model);
                }
                BottomTab::Log => {
                    paint_log(ui, model);
                }
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
    ui.weak(
        "UG893 Messages — clickable severity table (severity / id / engine text), not a colored dump",
    );
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
    let rows: Vec<(usize, helion_gui::IdeMessage)> = model
        .message_rows()
        .into_iter()
        .map(|(i, m)| (i, m.clone()))
        .collect();
    let mut pick: Option<usize> = None;
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
                    ui.label(RichText::new("Message").strong());
                    ui.end_row();
                    if rows.is_empty() {
                        ui.label("—");
                        ui.label("—");
                        ui.label("—");
                        ui.label("no messages");
                        ui.end_row();
                    } else {
                        for (i, m) in &rows {
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
                            if ui.selectable_label(on, &m.text).clicked() {
                                pick = Some(*i);
                            }
                            ui.end_row();
                        }
                    }
                });
        });
    if let Some(i) = pick {
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

fn paint_log(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.weak(
        "UG893 Log — clickable Tcl transcript (status / cmd / engine out), not a monospace dump",
    );
    let n_err = model.console.iter().filter(|l| !l.ok).count();
    ui.label(format!(
        "log n={} errors={n_err}",
        model.console.len()
    ));
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
                        ui.label("log empty");
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

fn paint_workspace(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.horizontal(|ui| {
        for tab in [
            WorkspaceTab::Reports,
            WorkspaceTab::Schematic,
            WorkspaceTab::Device,
            WorkspaceTab::Wave,
            WorkspaceTab::Hardware,
            WorkspaceTab::Ip,
            WorkspaceTab::Constraints,
            WorkspaceTab::ClockInteraction,
            WorkspaceTab::Cdc,
            WorkspaceTab::ClockNetworks,
            WorkspaceTab::Power,
            WorkspaceTab::Methodology,
            WorkspaceTab::Drc,
            WorkspaceTab::Utilization,
            WorkspaceTab::Hierarchy,
            WorkspaceTab::Find,
            WorkspaceTab::Package,
            WorkspaceTab::Runs,
            WorkspaceTab::Bitstream,
        ] {
            let label = match tab {
                WorkspaceTab::Reports => "Reports",
                WorkspaceTab::Schematic => "Schematic",
                WorkspaceTab::Device => "Device",
                WorkspaceTab::Wave => "Waveform",
                WorkspaceTab::Hardware => "Hardware",
                WorkspaceTab::Ip => "IP / BD",
                WorkspaceTab::Constraints => "Timing Constraints",
                WorkspaceTab::ClockInteraction => "Clock Interaction",
                WorkspaceTab::Cdc => "CDC",
                WorkspaceTab::ClockNetworks => "Clock Networks",
                WorkspaceTab::Power => "Power",
                WorkspaceTab::Methodology => "Methodology",
                WorkspaceTab::Drc => "DRC",
                WorkspaceTab::Utilization => "Utilization",
                WorkspaceTab::Hierarchy => "Hierarchy",
                WorkspaceTab::Find => "Find Results",
                WorkspaceTab::Package => "Package",
                WorkspaceTab::Runs => "Design Runs",
                WorkspaceTab::Bitstream => "Bitstream",
            };
            if ui
                .selectable_label(model.workspace == tab, label)
                .clicked()
            {
                model.workspace = tab;
            }
        }
    });
    ui.separator();
    match model.workspace {
        WorkspaceTab::Reports => paint_reports(ui, model),
        WorkspaceTab::Schematic => paint_schematic(ui, model),
        WorkspaceTab::Device => paint_device(ui, model),
        WorkspaceTab::Wave => paint_wave(ui, model),
        WorkspaceTab::Hardware => paint_hw(ui, model),
        WorkspaceTab::Ip => paint_ip(ui, model),
        WorkspaceTab::Constraints => paint_constraints(ui, model),
        WorkspaceTab::ClockInteraction => paint_clock_interaction(ui, model),
        WorkspaceTab::Cdc => paint_cdc(ui, model),
        WorkspaceTab::ClockNetworks => paint_clock_networks(ui, model),
        WorkspaceTab::Power => paint_power(ui, model),
        WorkspaceTab::Methodology => paint_methodology(ui, model),
        WorkspaceTab::Drc => paint_drc(ui, model),
        WorkspaceTab::Utilization => paint_utilization(ui, model),
        WorkspaceTab::Hierarchy => paint_hierarchy(ui, model),
        WorkspaceTab::Find => paint_find(ui, model),
        WorkspaceTab::Package => paint_package(ui, model),
        WorkspaceTab::Runs => paint_runs(ui, model),
        WorkspaceTab::Bitstream => paint_bitstream(ui, model),
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
    ui.heading("Design Runs");
    ui.weak(
        "UG893/UG986 Design Runs — clickable name/strategy/WNS/runtime/hash grid over Helion engines, not a dump",
    );
    ui.horizontal(|ui| {
        if ui.button("Launch synth_1").clicked() {
            let _ = model.exec("launch_runs synth_1");
        }
        if ui.button("Launch impl_1").clicked() {
            let _ = model.exec("launch_runs impl_1");
        }
        if ui.button("Reset impl_1").clicked() {
            let _ = model.exec("reset_runs impl_1");
        }
        if ui.button("Launch selected").clicked() {
            if let Some(name) = model
                .selected
                .as_deref()
                .map(|s| s.strip_prefix("run:").unwrap_or(s).to_string())
            {
                if model.runs.iter().any(|r| r.name == name) {
                    let _ = model.exec(&format!("launch_runs {name}"));
                }
            }
        }
        if ui.button("Reset selected").clicked() {
            if let Some(name) = model
                .selected
                .as_deref()
                .map(|s| s.strip_prefix("run:").unwrap_or(s).to_string())
            {
                if model.runs.iter().any(|r| r.name == name) {
                    let _ = model.exec(&format!("reset_runs {name}"));
                }
            }
        }
        if ui.button("Create RuntimeOpt").clicked() {
            let _ = model.exec("create_run impl_runtime -strategy RuntimeOpt");
            let _ = model.exec("launch_runs impl_runtime");
        }
        if ui.button("Create PhysOpt").clicked() {
            let _ = model.exec("create_run impl_phys -strategy PhysOpt");
            let _ = model.exec("launch_runs impl_phys");
        }
        if ui.button("Compare Runs").clicked() {
            let _ = model.exec("compare_runs");
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Incremental Impl").clicked() {
            let _ = model.exec("incremental_impl");
        }
        if ui.button("Fix Route +8 hops").clicked() {
            if let Some(net) = model
                .session()
                .placed
                .as_ref()
                .and_then(|p| p.packed.iobs.first())
                .map(|i| i.from_net.clone())
            {
                let _ = model.exec(&format!("fix_route {net} 8"));
            }
        }
        if ui.button("Insert ECO_LUT3").clicked() {
            let _ = model.exec("insert_eco_lut ECO_LUT3 0x8");
        }
        if ui.button("Check ECO").clicked() {
            let _ = model.exec("check_eco");
        }
        if ui.button("Incremental Place+Route").clicked() {
            let _ = model.exec("incremental_place");
            let _ = model.exec("incremental_route");
        }
    });
    ui.add_space(6.0);
    ui.label(format!("runs n={}", model.runs.len()));
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
            ui.weak("no implementation runs");
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
}

fn paint_hierarchy(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Hierarchy");
    ui.weak("UG893 Fig. 61 — nested boxes, area ∝ HNF cell/resource count (not a tree list)");
    ui.monospace(model.hierarchy_drawing_text());
    let drawing = model.hierarchy.drawing();
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
    ui.weak("find <cell|net|port|pin> against HNF/HAD");
    if model.find_results.is_empty() {
        ui.weak("No hits — `find u_lut0` in Tcl.");
    }
    let mut pick = None;
    for h in &model.find_results {
        let on = model.selected.as_deref() == Some(h.name.as_str());
        if ui
            .selectable_label(on, format!("{}  {}", h.kind, h.name))
            .clicked()
        {
            pick = Some(h.name.clone());
        }
    }
    if let Some(id) = pick {
        model.select(&id);
    }
}

fn paint_package(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("I/O Planning");
    ui.weak("UG893 — PACKAGE_PIN re-places; IOSTANDARD/DRIVE/SLEW/PULLTYPE/DIFF_TERM/IN_TERM hit HAD / STA / DRC / bitgen");
    let mut pick_port: Option<String> = None;
    let mut set_iostd: Option<(String, &'static str)> = None;
    let mut set_io: Option<(String, &'static str, &'static str)> = None;
    ui.collapsing("I/O Ports", |ui| {
        if model.io_ports.is_empty() {
            ui.weak("Synth a design to list ports.");
        }
        for p in &model.io_ports {
            let on = model.selected.as_deref() == Some(p.name.as_str());
            let pin = p
                .package_pin
                .as_deref()
                .or(p.site.as_deref())
                .unwrap_or("(unplaced)");
            let std = p.iostandard.as_deref().unwrap_or("-");
            let drv = p.drive.as_deref().unwrap_or("-");
            let slew = p.slew.as_deref().unwrap_or("-");
            let pull = p.pulltype.as_deref().unwrap_or("-");
            let diff = p.diff_term.as_deref().unwrap_or("-");
            let interm = p.in_term.as_deref().unwrap_or("-");
            if ui
                .selectable_label(
                    on,
                    format!(
                        "{}  {}  PACKAGE_PIN={pin}  IOSTANDARD={std}  DRIVE={drv}  SLEW={slew}  PULLTYPE={pull}  DIFF_TERM={diff}  IN_TERM={interm}",
                        p.name, p.dir
                    ),
                )
                .clicked()
            {
                pick_port = Some(p.name.clone());
            }
        }
        ui.weak("Select a port, then click an unassigned pin to loc + re-place.");
        if let Some(port) = model
            .selected
            .as_deref()
            .filter(|s| model.io_ports.iter().any(|p| p.name == *s))
        {
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
    });
    if let Some((port, std)) = set_iostd {
        let _ = model.exec(&format!(
            "set_property IOSTANDARD {std} [get_ports {port}]"
        ));
    }
    if let Some((port, key, val)) = set_io {
        let _ = model.exec(&format!("set_property {key} {val} [get_ports {port}]"));
    }
    if let Some(name) = pick_port {
        model.select(&name);
    }
    ui.separator();
    ui.label(RichText::new("Package").strong());
    ui.weak("UG893 Fig. 53 — HAD IOB pin circles on colored I/O bank regions");
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
    let cell = 18.0_f32;
    let mut pick: Option<String> = None;
    let selected = model.selected.clone();
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(cell * cols as f32 + 36.0, cell * rows as f32 + 20.0),
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
    ui.heading("Timing Constraints");
    ui.weak("UG893 Timing Constraints Editor — clickable clocks / I/O-delay / exception tables from helion-sta XDC, not a dump. Empty XDC keeps gold WNS.");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Read examples/counter.sdc").clicked() {
            let p = helion_device::Device::examples_dir().join("counter.sdc");
            let _ = model.exec(&format!("read_xdc {}", p.display()));
        }
        if ui.button("Apply create_generated_clock ÷2").clicked() {
            let _ = model.exec(
                "create_generated_clock -name clkdiv -source [get_ports clk] -divide_by 2 [get_pins u_ff/Q]",
            );
        }
        if ui.button("Apply create_generated_clock ×2").clicked() {
            let _ = model.exec(
                "create_generated_clock -name clk2x -source [get_ports clk] -multiply_by 2 [get_pins u_ff/Q]",
            );
        }
        if ui.button("Apply create_generated_clock -invert").clicked() {
            let _ = model.exec(
                "create_generated_clock -name clkinv -source [get_ports clk] -divide_by 1 -invert [get_pins u_ff/Q]",
            );
        }
        if ui.button("Apply create_generated_clock -edges {1 3 5}").clicked() {
            let _ = model.exec(
                "create_generated_clock -name clkedg -source [get_ports clk] -edges {1 3 5} [get_pins u_ff/Q]",
            );
        }
        if ui.button("Apply set_input_delay 1.5ns clk").clicked() {
            let _ = model.exec("set_input_delay -clock clk 1.5 [get_ports clk]");
        }
        if ui.button("Apply set_output_delay 2ns led").clicked() {
            let _ = model.exec("set_output_delay -clock clk 2.0 [get_ports led]");
        }
        if ui.button("Apply set_false_path clk→led").clicked() {
            let _ = model.exec("set_false_path -from [get_ports clk] -to [get_ports led]");
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Apply set_multicycle_path 2 clk→led").clicked() {
            let _ = model.exec("set_multicycle_path 2 -from [get_ports clk] -to [get_ports led]");
        }
        if ui.button("Apply set_multicycle_path -hold 1").clicked() {
            let _ = model.exec("set_multicycle_path -hold 1 -from [get_ports clk] -to [get_ports led]");
        }
        if ui.button("Apply set_max_delay 5ns clk→led").clicked() {
            let _ = model.exec("set_max_delay 5.0 -from [get_ports clk] -to [get_ports led]");
        }
        if ui.button("Apply set_min_delay 1ns clk→led").clicked() {
            let _ = model.exec("set_min_delay 1.0 -from [get_ports clk] -to [get_ports led]");
        }
        if ui.button("Apply set_clock_groups async clk/virt").clicked() {
            let _ = model.exec(
                "set_clock_groups -asynchronous -group [get_clocks clk] -group [get_clocks virt]",
            );
        }
        if ui.button("Apply set_clock_uncertainty 0.5ns setup").clicked() {
            let _ = model.exec("set_clock_uncertainty -setup 0.5 [get_clocks clk]");
        }
        if ui.button("Apply set_clock_latency 0.4ns late").clicked() {
            let _ = model.exec("set_clock_latency -late 0.4 [get_clocks clk]");
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Apply set_disable_timing clk→led").clicked() {
            let _ = model.exec("set_disable_timing -from [get_ports clk] -to [get_ports led]");
        }
        if ui.button("Apply set_case_analysis 0 clk").clicked() {
            let _ = model.exec("set_case_analysis 0 [get_ports clk]");
        }
        if ui.button("Apply set_propagated_clock clk").clicked() {
            let _ = model.exec("set_propagated_clock [get_clocks clk]");
        }
        if ui.button("Apply set_clock_sense -negative").clicked() {
            let _ = model.exec("set_clock_sense -negative [get_pins u_lut0/I0]");
        }
        if ui.button("Apply set_clock_sense -stop").clicked() {
            let _ = model.exec("set_clock_sense -stop_propagation [get_pins clk_buf/O]");
        }
        if ui.button("Apply set_input_jitter 0.2ns clk").clicked() {
            let _ = model.exec("set_input_jitter [get_clocks clk] 0.2");
        }
        if ui.button("Apply set_system_jitter 0.1ns").clicked() {
            let _ = model.exec("set_system_jitter 0.1");
        }
        if ui.button("Apply set_timing_derate -late 1.1").clicked() {
            let _ = model.exec("set_timing_derate -late 1.1");
        }
        if ui.button("Apply set_operating_conditions 0.95V 85C").clicked() {
            let _ = model.exec("set_operating_conditions -voltage 0.95 -temperature 85");
        }
        if ui.button("Apply set_bus_skew 0.5ns clk→led").clicked() {
            let _ = model.exec("set_bus_skew -setup 0.5 -from [get_ports clk] -to [get_ports led]");
        }
        if ui.button("Apply group_path -weight 2 clk→led").clicked() {
            let _ = model.exec(
                "group_path -name extra -weight 2 -from [get_ports clk] -to [get_ports led]",
            );
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Apply set_max_time_borrow 1ns").clicked() {
            let _ = model.exec("set_max_time_borrow 1.0 [get_cells u_ff]");
        }
        if ui.button("Apply set_data_check -setup 0.5ns").clicked() {
            let _ = model.exec(
                "set_data_check -setup 0.5 -from [get_ports clk] -to [get_ports led]",
            );
        }
        if ui.button("Apply set_data_check -hold 0.2ns").clicked() {
            let _ = model.exec(
                "set_data_check -hold 0.2 -from [get_ports clk] -to [get_ports led]",
            );
        }
    });
    ui.add_space(6.0);
    paint_constraints_tables(ui, model);
}

fn paint_constraints_tables(ui: &mut egui::Ui, model: &mut IdeModel) {
    let rows = model.constraint_rows();
    if rows.is_empty() {
        ui.label("no timing constraints — create_clock / create_generated_clock / read_xdc");
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
    ui.weak(
        "UG893 Reports — clickable catalog of Helion engine reports, not stacked dumps",
    );
    ui.add_space(6.0);
    paint_report_catalog(ui, model);
    ui.add_space(8.0);
    paint_timing_summary(ui, model);
    ui.add_space(8.0);
    paint_timing_paths(ui, model);
}

fn paint_timing_paths(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.label(
        RichText::new(
            "Timing Paths (report_timing) — UG903 pin-delay table (Name / Type / Incr_ps / Path_ps)",
        )
        .strong(),
    );
    ui.weak("STA arcs from helion-sta, not a selectable path-name list.");
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
        ui.label("no timing paths — report_timing / Route");
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
    ui.label(format!(
        "reports n={} complete={}",
        rows.len(),
        rows.iter().filter(|r| r.status == "Complete").count()
    ));
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
            "Timing Summary (report_timing_summary) — UG903/UG949 intra/inter-clock WNS/TNS/WHS/THS by path group",
        )
        .strong(),
    );
    ui.weak("STA path groups, not a dump. Empty XDC keeps gold WNS.");
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
    let selected = model.selected.clone();
    let mut pick: Option<(String, Option<String>)> = None;
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
                    let on = selected.as_deref() == Some(key.as_str());
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
                    ui.label(&g.from);
                    ui.label(&g.to);
                    ui.label(slack_label(g.wns_ps));
                    ui.label(g.tns_ps.to_string());
                    ui.label(slack_label(g.whs_ps));
                    ui.label(g.ths_ps.to_string());
                    ui.label(g.endpoints.to_string());
                    ui.end_row();
                }
            });
    }
    if let Some((a, b)) = pick {
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
    ui.weak(
        "UG949 report_clock_interaction — STA From×To matrix (Timed / generated / unsafe CDC / async / exclusive / false path), not a dump",
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Report clock interaction").clicked() {
            let _ = model.exec("report_clock_interaction");
        }
        if ui.button("Apply set_clock_groups async clk/virt").clicked() {
            let _ = model.exec(
                "set_clock_groups -asynchronous -group [get_clocks clk] -group [get_clocks virt]",
            );
        }
        if ui.button("Apply set_false_path clk→virt").clicked() {
            let _ = model.exec(
                "set_false_path -from [get_clocks clk] -to [get_clocks virt]",
            );
        }
        if ui.button("Apply set_max_delay -datapath_only 2ns").clicked() {
            let _ = model.exec(
                "set_max_delay -datapath_only 2.0 -from [get_clocks clk] -to [get_clocks virt]",
            );
        }
    });
    ui.add_space(6.0);
    let report = model.clock_interaction();
    if report.clocks.is_empty() {
        ui.label("no clocks — create_clock / report_clock_interaction");
        return;
    }
    let selected = model.selected.clone();
    let mut pick: Option<(String, String)> = None;
    egui::ScrollArea::both().show(ui, |ui| {
        egui::Grid::new("clock_interaction_matrix")
            .spacing([4.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("From \\ To").strong());
                for c in &report.clocks {
                    ui.label(RichText::new(&c.name).strong());
                }
                ui.end_row();
                for from in &report.clocks {
                    ui.label(RichText::new(&from.name).strong());
                    for to in &report.clocks {
                        if let Some(cell) = report.cell(&from.name, &to.name) {
                            let key = format!("{}->{}", cell.from, cell.to);
                            let on = selected.as_deref() == Some(key.as_str());
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
                                "FROM={} TO={} {} COMMON_PS={} REQ_PS={} paths={}",
                                cell.from,
                                cell.to,
                                cell.relation.as_str(),
                                cell.common_period_ps,
                                cell.requirement_ps,
                                cell.path_count
                            ));
                        } else {
                            ui.label("—");
                        }
                    }
                    ui.end_row();
                }
            });
    });
    if let Some((from, to)) = pick {
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
    ui.weak(
        "UG906 report_cdc — STA inter-clock rows (Critical missing sync / Warning datapath / Info async / Safe false path), not a dump",
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Report CDC").clicked() {
            let _ = model.exec("report_cdc");
        }
        if ui.button("Apply set_clock_groups async clk/virt").clicked() {
            let _ = model.exec(
                "set_clock_groups -asynchronous -group [get_clocks clk] -group [get_clocks virt]",
            );
        }
        if ui.button("Apply set_false_path clk→virt").clicked() {
            let _ = model.exec("set_false_path -from [get_clocks clk] -to [get_clocks virt]");
        }
        if ui.button("Apply set_max_delay -datapath_only 2ns").clicked() {
            let _ = model.exec(
                "set_max_delay -datapath_only 2.0 -from [get_clocks clk] -to [get_clocks virt]",
            );
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
    let selected = model.selected.clone();
    let mut pick: Option<(String, String)> = None;
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
                ui.label(RichText::new("WNS_PS").strong());
                ui.label(RichText::new("Relation").strong());
                ui.end_row();
                for v in &report.violations {
                    let key = format!("{}->{}", v.from, v.to);
                    let on = selected.as_deref() == Some(key.as_str());
                    let fill = cdc_severity_color(v.severity);
                    let btn = egui::Button::new(
                        RichText::new(&v.from).color(Color32::BLACK),
                    )
                    .fill(fill)
                    .selected(on);
                    if ui.add(btn).clicked() {
                        pick = Some((v.from.clone(), v.to.clone()));
                    }
                    ui.label(&v.to);
                    ui.label(v.severity.as_str());
                    ui.label(&v.check);
                    ui.label(if v.synchronizer { "1" } else { "0" });
                    ui.label(v.endpoints.to_string());
                    ui.label(slack_label(v.wns_ps));
                    ui.label(v.relation.as_str());
                    ui.end_row();
                }
            });
    });
    if let Some((from, to)) = pick {
        let _ = model.select_cdc(&from, &to);
    }
}

fn paint_clock_networks(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Clock Networks");
    ui.weak(
        "UG903 report_clock_networks — STA clocks, HNF FF loads, HAD CLK-spine buffers, place insertion, not a dump",
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Report clock networks").clicked() {
            let _ = model.exec("report_clock_networks");
        }
        if ui.button("Apply create_generated_clock ÷2").clicked() {
            let _ = model.exec(
                "create_generated_clock -name clkdiv -source [get_ports clk] -divide_by 2 [get_pins u_ff/Q]",
            );
        }
        if ui.button("Apply set_propagated_clock clk").clicked() {
            let _ = model.exec("set_propagated_clock [get_clocks clk]");
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
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
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
                let on = selected.as_deref() == Some(c.name.as_str());
                if ui.selectable_label(on, &c.name).clicked() {
                    pick = Some(c.name.clone());
                }
                ui.label(c.period_ps.to_string());
                ui.label(&c.source);
                ui.label(&c.net);
                ui.label(c.n_loads.to_string());
                ui.label(c.n_buffers.to_string());
                ui.label(c.fanout.to_string());
                ui.label(c.insertion_ps.to_string());
                ui.end_row();
            }
        });
    if let Some(name) = pick {
        let _ = model.select_clock_network(&name);
    }
}

fn paint_power(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Power");
    ui.weak(
        "UG907 report_power — HAD occupancy × STA clocks × set_operating_conditions PVT, not a dump",
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Report power").clicked() {
            let _ = model.exec("report_power");
        }
        if ui.button("Apply set_operating_conditions 0.95V 85C").clicked() {
            let _ = model.exec("set_operating_conditions -voltage 0.95 -temperature 85");
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
                let on = selected.as_deref() == Some(name);
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
    ui.label(format!(
        "LUTFF={}/{} IOB={}/{} BRAM={}/{} DSP={}/{}",
        report.lutff,
        report.lutff_cap,
        report.iob,
        report.iob_cap,
        report.bram,
        report.bram_cap,
        report.dsp,
        report.dsp_cap
    ));
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
    ui.weak(
        "UG949 report_methodology — STA/XDC/HNF checks (TIMING-1/6/7/10/18/24, CDC-1), not a dump",
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Report methodology").clicked() {
            let _ = model.exec("report_methodology");
        }
        if ui.button("Apply create_clock 10ns").clicked() {
            let _ = model.exec("create_clock -period 10.000 [get_ports clk]");
        }
        if ui.button("Apply set_output_delay 0.5ns led").clicked() {
            let _ = model.exec("set_output_delay 0.5 -clock [get_clocks clk] [get_ports led]");
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
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
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
                    let on = selected.as_deref() == Some(v.id.as_str());
                    let fill = methodology_severity_color(v.severity);
                    let btn = egui::Button::new(RichText::new(&v.id).color(Color32::BLACK))
                        .fill(fill)
                        .selected(on);
                    if ui.add(btn).clicked() {
                        pick = Some(v.id.clone());
                    }
                    ui.label(v.severity.as_str());
                    ui.label(&v.category);
                    ui.label(if v.objects.is_empty() {
                        "-"
                    } else {
                        v.objects.as_str()
                    });
                    ui.label(&v.message);
                    ui.end_row();
                }
            });
    });
    if let Some(id) = pick {
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
    ui.weak("UG893 DRC — helion-drc rule rows (severity / id / objects), not a one-line dump");
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
    let selected = model.selected.clone();
    let mut pick: Option<String> = None;
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
                        let on = selected.as_deref() == Some(v.id.as_str());
                        let fill = drc_severity_color(v.severity);
                        let btn = egui::Button::new(RichText::new(&v.id).color(Color32::BLACK))
                            .fill(fill)
                            .selected(on);
                        if ui.add(btn).clicked() {
                            pick = Some(v.id.clone());
                        }
                        ui.label(v.severity.as_str());
                        ui.label(if v.objects.is_empty() {
                            "-"
                        } else {
                            v.objects.as_str()
                        });
                        ui.label(&v.message);
                        ui.end_row();
                    }
                }
            });
    });
    if let Some(id) = pick {
        let _ = model.select_drc(&id);
    }
}

fn paint_utilization(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Utilization");
    ui.weak(
        "UG893 Utilization — HAD occupancy bars (used / available / pct) + HNF hierarchy, not a dump",
    );
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
                let on = selected.as_deref() == Some(row.resource);
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
                    ui.label(&h.name);
                    ui.label(h.lut.to_string());
                    ui.label(h.ff.to_string());
                    ui.label(h.iob.to_string());
                    ui.label(h.bram.to_string());
                    ui.label(h.dsp.to_string());
                    ui.end_row();
                }
            });
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
    ui.weak("UG893 Fig. 55/56/57 — HNF symbols, pin stubs, orthogonal nets (dotted = off-sheet, thick = bus)");
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
        ui.horizontal(|ui| {
            ui.label(RichText::new("Timing paths").small());
            let mut pick_path = None;
            for (i, p) in model.timing_paths.iter().enumerate() {
                let on = model.selected_timing_path == Some(i);
                if ui
                    .selectable_label(on, format!("{} slack={}", p.endpoint, p.slack_ps))
                    .clicked()
                {
                    pick_path = Some(i);
                }
            }
            if let Some(i) = pick_path {
                let _ = model.select_timing_path(&i.to_string());
            }
        });
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
                    // UG893 Fig. 60: ports / IOB as right-pointing triangles; LUTs and FFs as boxes.
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
                        // UG893 Fig. pin stub: a short line inside and outside the symbol.
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
    ui.weak(format!(
        "HAD {}  drawing {}×{} @ X{}Y{}  sites={} occupied={}  (UG893 floorplan, not a site list)",
        model.part(),
        model.device.cols,
        model.device.rows,
        model.device.x0,
        model.device.y0,
        model.device.sites.len(),
        model.device.occupied_count()
    ));
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
    let mut pick_port: Option<String> = None;
    let mut set_iostd: Option<(String, &'static str)> = None;
    let mut set_io: Option<(String, &'static str, &'static str)> = None;
    ui.collapsing("I/O Ports", |ui| {
        if model.io_ports.is_empty() {
            ui.weak("Synth a design to list ports.");
        }
        for p in &model.io_ports {
            let on = model.selected.as_deref() == Some(p.name.as_str());
            let pin = p
                .package_pin
                .as_deref()
                .or(p.site.as_deref())
                .unwrap_or("(unplaced)");
            let std = p.iostandard.as_deref().unwrap_or("-");
            let drv = p.drive.as_deref().unwrap_or("-");
            let slew = p.slew.as_deref().unwrap_or("-");
            let pull = p.pulltype.as_deref().unwrap_or("-");
            let diff = p.diff_term.as_deref().unwrap_or("-");
            let interm = p.in_term.as_deref().unwrap_or("-");
            if ui
                .selectable_label(
                    on,
                    format!(
                        "{}  {}  PACKAGE_PIN={pin}  IOSTANDARD={std}  DRIVE={drv}  SLEW={slew}  PULLTYPE={pull}  DIFF_TERM={diff}  IN_TERM={interm}",
                        p.name, p.dir
                    ),
                )
                .clicked()
            {
                pick_port = Some(p.name.clone());
            }
        }
        if let Some(port) = model
            .selected
            .as_deref()
            .filter(|s| model.io_ports.iter().any(|p| p.name == *s))
        {
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
    });
    if let Some((port, std)) = set_iostd {
        let _ = model.exec(&format!(
            "set_property IOSTANDARD {std} [get_ports {port}]"
        ));
    }
    if let Some((port, key, val)) = set_io {
        let _ = model.exec(&format!("set_property {key} {val} [get_ports {port}]"));
    }
    let mut pick_pblock: Option<String> = None;
    ui.collapsing("Pblocks", |ui| {
        ui.weak("UG893 Floorplanning — create_pblock / resize_pblock hits place + bitgen_pblock");
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
        if model.pblocks.is_empty() {
            ui.weak("No pblocks — create_pblock then resize_pblock -add {CLB_X5Y1:CLB_X8Y8}.");
        }
        for p in &model.pblocks {
            let on = model.selected.as_deref() == Some(p.name.as_str());
            if ui
                .selectable_label(
                    on,
                    format!(
                        "{}  {}  cells={} frames={}",
                        p.name,
                        p.range_text(),
                        p.cells.len(),
                        p.frames
                    ),
                )
                .clicked()
            {
                pick_pblock = Some(p.name.clone());
            }
        }
    });
    ui.separator();
    let cols = model.device.cols.max(1);
    let rows = model.device.rows.max(1);
    let x0 = model.device.x0;
    let y0 = model.device.y0;
    let cell = 12.0_f32;
    let grid_w = cell * cols as f32;
    let grid_h = cell * rows as f32;
    let mut pick_site: Option<(u32, u32)> = None;
    let mut pick_region: Option<String> = None;
    let mut click_pblock: Option<String> = None;
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(grid_w + 28.0, grid_h + 16.0),
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
                // UG893 Floorplanning: Pblock rectangles (create_pblock / resize_pblock).
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
                // PathFinder IOB nets over the die (UG893 Device routing, not occupancy restyle).
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
        });
    if let Some(name) = pick_port {
        model.select(&name);
    }
    if let Some(name) = pick_pblock {
        let _ = model.select_pblock(&name);
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
    if model.wave.traces.is_empty() {
        ui.weak("Run Simulation (sim_run 16) after Bitstream.");
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
    ui.weak(
        "UG893 Hardware Manager / UG900 debug — helion-hw TAP STAT bits + ILA samples, not a one-liner",
    );
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
    ui.weak("UG900 — trigger/window on helion-debug capture, samples on Wave");
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

fn flow_button(ui: &mut egui::Ui, model: &mut IdeModel, step: FlowStep) {
    let state = model.step_state(step);
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
    let label = format!("  {}  ", step.label());
    let galley = ui.painter().layout_no_wrap(
        label.clone(),
        egui::FontId::proportional(14.0),
        text,
    );
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 8.0, 28.0),
        Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect(rect, 4.0, fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        let pos = egui::pos2(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        );
        ui.painter().galley(pos, galley, text);
    }
    if resp.hovered() {
        ui.painter()
            .rect_stroke(rect, 4.0, Stroke::new(1.5, stroke), egui::StrokeKind::Outside);
    }
    if resp.clicked() {
        let _ = model.run_step(step);
    }
    resp.on_hover_text(step.tcl());
}
