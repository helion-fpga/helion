//! Helion IDE — Vivado-class desktop window over the real Session engines.
//!
//! `--version` / `--doctor` never open a window (so they work headless and on CI).
//! The GUI paints [`helion_gui::IdeModel`]; every button and the Tcl box call into
//! that model, which is what the unit tests already prove is not a no-op.

use eframe::egui::{self, Color32, RichText, Sense, Stroke};
use helion_gui::{
    doctor, BottomTab, FlowStep, IdeModel, IlaTrigger, LayoutKind, MsgSeverity, NavSection,
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

fn paint_navigator(ctx: &egui::Context, model: &mut IdeModel) {
    egui::SidePanel::left("navigator")
        .resizable(true)
        .default_width(200.0)
        .min_width(160.0)
        .show(ctx, |ui| {
            ui.label(RichText::new("Flow Navigator").strong().size(14.0));
            ui.weak("UltraFast (UG949) stages on Helion engines");
            ui.add_space(4.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                for sec in NavSection::ALL {
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
            for s in &model.scopes {
                ui.monospace(format!("{} ({})", s.name, s.kind));
            }
            ui.separator();
            ui.label(RichText::new("Objects").strong());
            for o in &model.objects {
                ui.monospace(format!("{} = {}", o.name, o.value));
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
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .max_height(ui.available_height() - 28.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for line in &model.console {
                                let color = if line.ok {
                                    Color32::from_rgb(0xc8, 0xd0, 0xd8)
                                } else {
                                    Color32::from_rgb(0xe0, 0x6c, 0x75)
                                };
                                ui.monospace(
                                    RichText::new(format!("helion% {}", line.cmd))
                                        .color(Color32::from_rgb(0x7e, 0xc8, 0xe3)),
                                );
                                if !line.out.is_empty() {
                                    ui.monospace(RichText::new(&line.out).color(color));
                                }
                            }
                        });
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
                    ui.weak(model.messages_text().lines().next().unwrap_or("messages"));
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for m in &model.messages {
                                let c = match m.severity {
                                    MsgSeverity::Error => Color32::from_rgb(0xe0, 0x6c, 0x75),
                                    MsgSeverity::Warning => Color32::from_rgb(0xe5, 0xc0, 0x7b),
                                    MsgSeverity::Info => Color32::from_rgb(0xc8, 0xd0, 0xd8),
                                };
                                ui.monospace(
                                    RichText::new(format!(
                                        "{} [{}] {}",
                                        m.severity.tag(),
                                        m.id,
                                        m.text
                                    ))
                                    .color(c),
                                );
                            }
                        });
                }
                BottomTab::Log => {
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for line in &model.log {
                                ui.monospace(line);
                            }
                        });
                }
            }
        });
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
            WorkspaceTab::Hierarchy,
            WorkspaceTab::Find,
            WorkspaceTab::Package,
            WorkspaceTab::Runs,
        ] {
            let label = match tab {
                WorkspaceTab::Reports => "Reports",
                WorkspaceTab::Schematic => "Schematic",
                WorkspaceTab::Device => "Device",
                WorkspaceTab::Wave => "Waveform",
                WorkspaceTab::Hardware => "Hardware",
                WorkspaceTab::Ip => "IP / BD",
                WorkspaceTab::Constraints => "Timing Constraints",
                WorkspaceTab::Hierarchy => "Hierarchy",
                WorkspaceTab::Find => "Find Results",
                WorkspaceTab::Package => "Package",
                WorkspaceTab::Runs => "Design Runs",
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
        WorkspaceTab::Hierarchy => paint_hierarchy(ui, model),
        WorkspaceTab::Find => paint_find(ui, model),
        WorkspaceTab::Package => paint_package(ui, model),
        WorkspaceTab::Runs => paint_runs(ui, model),
    }
}

fn paint_runs(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Design Runs");
    ui.weak("UG893 synth_1 / impl_1 on Helion engines — launch_runs / reset_runs");
    ui.horizontal(|ui| {
        if ui.button("Launch synth_1").clicked() {
            let _ = model.exec("launch_runs synth_1");
        }
        if ui.button("Launch impl_1").clicked() {
            let _ = model.exec("launch_runs impl_1");
        }
        if ui.button("Reset synth_1").clicked() {
            let _ = model.exec("reset_runs synth_1");
        }
        if ui.button("Reset impl_1").clicked() {
            let _ = model.exec("reset_runs impl_1");
        }
    });
    ui.add_space(6.0);
    ui.monospace("Name            Step              Status        Part           LUTFF  WNS_PS  hash");
    for r in &model.runs {
        ui.monospace(format!(
            "{:<15} {:<17} {:<13} {:<14} {:<6} {:<7} {}",
            r.name,
            r.step,
            r.status,
            r.part,
            r.lutff.map(|n| n.to_string()).unwrap_or_else(|| "-".into()),
            r.wns_ps.map(|n| n.to_string()).unwrap_or_else(|| "-".into()),
            r.bitstream_hash
                .map(|h| format!("{h:#010x}"))
                .unwrap_or_else(|| "-".into()),
        ));
        if let Some(top) = &r.top {
            ui.weak(format!("  top={top}  cells={}", r.cells.unwrap_or(0)));
        }
    }
    ui.add_space(8.0);
    report_box(ui, "Runs (engine)", &model.runs_text());
}

fn paint_hierarchy(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Hierarchy");
    ui.weak("UG893 — HNF top / instances / leaf cells");
    ui.monospace(model.hierarchy_text());
    let mut pick = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (name, kind) in &model.hierarchy.nodes {
            let on = model.selected.as_deref() == Some(name.as_str());
            let label = if kind.starts_with("instance:") {
                format!("  {name}  ({kind})")
            } else if kind == "module" {
                format!("{name}  [top]")
            } else {
                format!("    {name}  {kind}")
            };
            if ui
                .selectable_label(on, RichText::new(label).monospace())
                .clicked()
            {
                pick = Some(name.clone());
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
    ui.heading("Package");
    ui.weak("UG893 HAD IOB package drawing (IOB_XxYy grid — not a BGA, not a pin table)");
    ui.monospace(format!(
        "part={}  {}×{}  pins={}  assigned={}",
        model.package.part,
        model.package.cols,
        model.package.rows,
        model.package_pins.len(),
        model.package_pins.iter().filter(|p| p.port.is_some()).count()
    ));
    let mut pick: Option<String> = None;
    egui::ScrollArea::both().show(ui, |ui| {
        for dy in 0..model.package.rows {
            let y = model.package.y0 + dy;
            ui.horizontal(|ui| {
                ui.weak(format!("Y{y}"));
                for dx in 0..model.package.cols {
                    let x = model.package.x0 + dx;
                    if let Some(p) = model.package.pin_at(&model.package_pins, x, y) {
                        let on = model.selected.as_deref() == Some(p.pin.as_str())
                            || p.port.as_deref() == model.selected.as_deref();
                        let label = match p.port.as_deref() {
                            Some(port) => format!("X{}Y{}\n{port}", p.x, p.y),
                            None => format!("X{}Y{}\n·", p.x, p.y),
                        };
                        if ui.selectable_label(on, RichText::new(label).monospace().small()).clicked()
                        {
                            pick = Some(p.pin.clone());
                        }
                    }
                }
            });
        }
        ui.separator();
        ui.weak("Pin list (same HAD sites)");
        for p in &model.package_pins {
            let on = model.selected.as_deref() == Some(p.pin.as_str())
                || p.port.as_deref() == model.selected.as_deref();
            if ui
                .selectable_label(
                    on,
                    format!(
                        "{}  X{}Y{}  {}",
                        p.pin,
                        p.x,
                        p.y,
                        p.port.as_deref().unwrap_or("-")
                    ),
                )
                .clicked()
            {
                pick = Some(p.pin.clone());
            }
        }
    });
    if let Some(pin) = pick {
        let _ = model.select_package_pin(&pin);
    }
}

fn paint_constraints(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Timing Constraints");
    ui.weak("UG893 SDC/XDC on helion-sta — create_clock / I/O delay / false path Apply");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Read examples/counter.sdc").clicked() {
            let p = helion_device::Device::examples_dir().join("counter.sdc");
            let _ = model.exec(&format!("read_xdc {}", p.display()));
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
    ui.add_space(6.0);
    report_box(ui, "Constraints (create_clock / I/O delay / false path)", &model.constraints_text());
    ui.add_space(8.0);
    report_box(ui, "Timing (report_timing)", &model.timing_text());
}

fn paint_reports(ui: &mut egui::Ui, model: &IdeModel) {
    ui.heading("Reports");
    ui.add_space(6.0);
    report_box(ui, "Timing (report_timing)", &model.timing_text());
    ui.add_space(8.0);
    report_box(ui, "Utilization (report_utilization)", &model.utilization_text());
    ui.add_space(8.0);
    let bits = match (model.bitstream_hash(), model.bitstream_bytes()) {
        (Some(h), Some(b)) => format!(
            "hash={h:#010x} bytes={b} frames={}",
            model.bitstream_frames().unwrap_or(0)
        ),
        _ => "no bitstream — run Bitstream".into(),
    };
    report_box(ui, "Bitstream", &bits);
    ui.add_space(8.0);
    let drc = match &model.drc {
        Some(d) if d.ok() => "report_drc violations=0 ok".into(),
        Some(d) => format!("report_drc {}", d.violations.join("; ")),
        None => "no DRC — run Place/Route".into(),
    };
    report_box(ui, "DRC (helion-drc)", &drc);
}

fn paint_schematic(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Schematic");
    ui.weak("HNF cells and nets — Expand Cone follows net endpoints");
    ui.horizontal(|ui| {
        if ui.button("Expand Cone").clicked() {
            let _ = model.exec("expand_cone");
        }
        if ui.button("Collapse Cone").clicked() {
            let _ = model.exec("collapse_cone");
        }
        ui.monospace(model.schematic_text().lines().next().unwrap_or(""));
    });
    let mut pick = None;
    let nodes: Vec<(String, String)> = model
        .schematic
        .visible_nodes()
        .iter()
        .map(|n| (n.name.clone(), n.kind.clone()))
        .collect();
    let edges: Vec<(String, String, String)> = model
        .schematic
        .visible_edges()
        .iter()
        .map(|e| (e.src.clone(), e.net.clone(), e.dst.clone()))
        .collect();
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (name, kind) in &nodes {
            let on = model.selected.as_deref() == Some(name.as_str());
            if ui
                .selectable_label(on, format!("{name}  {kind}"))
                .clicked()
            {
                pick = Some(name.clone());
            }
        }
        ui.separator();
        for (src, net, dst) in &edges {
            ui.monospace(format!("{src} —{net}→ {dst}"));
        }
    });
    if let Some(id) = pick {
        model.select(&id);
    }
}

fn paint_device(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Device / I/O Planning");
    ui.weak(format!(
        "HAD {}  {}×{}  sites={}  (UltraFast board/device)",
        model.part(),
        model.device.cols,
        model.device.rows,
        model.device.sites.len()
    ));
    ui.label(RichText::new("I/O Ports").strong());
    for p in &model.io_ports {
        ui.monospace(format!(
            "{}  {}  {}",
            p.name,
            p.dir,
            p.site.as_deref().unwrap_or("(unplaced)")
        ));
    }
    ui.separator();
    let mut pick = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        for s in model.device.sites.iter().filter(|s| s.occupant.is_some()) {
            let name = s.occupant.as_deref().unwrap();
            let on = model.selected.as_deref() == Some(name);
            if ui
                .selectable_label(on, format!("X{}Y{:02} {}  {name}", s.x, s.y, format!("{:?}", s.kind)))
                .clicked()
            {
                pick = Some(name.to_string());
            }
        }
    });
    if let Some(id) = pick {
        model.select(&id);
    }
}

fn paint_wave(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.horizontal(|ui| {
        ui.heading("Waveform");
        ui.label(
            RichText::new(format!(
                "timescale {} ps/cycle  cursor t={} ({} ps)",
                model.wave.timescale_ps,
                model.wave.cursor,
                model.wave.time_ps(model.wave.cursor)
            ))
            .weak()
            .monospace(),
        );
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
    }

    let mut style_cmd: Option<(String, WaveStyle)> = None;
    let mut radix_cmd: Option<(String, WaveRadix)> = None;
    let mut new_cursor: Option<usize> = None;
    let cursor = model.wave.cursor;
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
                paint_trace_shape(ui, rect, t, cursor, n, ts);
            }
            if resp.clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let x = ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
                    new_cursor = Some(((x * n as f32) as usize).min(n.saturating_sub(1)));
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
}

fn paint_trace_shape(
    ui: &egui::Ui,
    rect: egui::Rect,
    t: &helion_gui::WaveTrace,
    cursor: usize,
    n: usize,
    _ts: u64,
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
    let cx = rect.left() + dx * (cursor as f32 + 0.5);
    p.line_segment(
        [egui::pos2(cx, rect.top()), egui::pos2(cx, rect.bottom())],
        Stroke::new(1.0, Color32::from_rgb(0xe5, 0xc0, 0x7b)),
    );
    let _ = ns;
}

fn paint_hw(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("Hardware Manager");
    ui.monospace(format!(
        "target={} open={} programmed={}",
        model.hw.target, model.hw.open, model.hw.programmed
    ));
    if ui.button("Open Hardware Manager").clicked() {
        let _ = model.exec("open_hw_manager");
    }
    if ui.button("Program Device (sim)").clicked() {
        let _ = model.exec("program_hw");
    }
    if let Some(st) = &model.hw.stat {
        ui.monospace(format!(
            "STAT INIT={} DONE={} EOS={} GWE={} GSR={} GTS={} CRC_ERR={}",
            st.init as u8,
            st.done as u8,
            st.eos as u8,
            st.gwe as u8,
            st.gsr as u8,
            st.gts as u8,
            st.crc_err as u8
        ));
    }
    ui.separator();
    ui.label(RichText::new("ILA Dashboard").strong());
    ui.weak("UG900 — trigger/window on helion-debug capture, samples on Wave");
    ui.monospace(model.ila_dashboard_text());
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
    let tname = format!("ila:{}", model.ila.net);
    if let Some(t) = model.wave.trace(&tname) {
        ui.weak(format!(
            "wave {}  cursor={}  value={}",
            t.name,
            model.wave.cursor,
            t.value_at(model.wave.cursor)
        ));
    }
}

fn paint_ip(ui: &mut egui::Ui, model: &mut IdeModel) {
    ui.heading("IP Catalog / Block Design");
    if ui.button("Refresh catalog").clicked() {
        let _ = model.exec("ip_catalog");
    }
    if ui.button("Create Block Design").clicked() {
        let _ = model.exec("create_bd");
    }
    for c in &model.ip_catalog {
        ui.monospace(format!(
            "{}  {}/{}  {}",
            c.name, c.vendor, c.library, c.bus
        ));
    }
    if let Some(bd) = &model.block_design {
        ui.separator();
        ui.label(RichText::new(format!("BD {} ok={}", bd.name, bd.ok)).strong());
        ui.monospace(&bd.sv);
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

fn report_box(ui: &mut egui::Ui, title: &str, body: &str) {
    egui::Frame::group(ui.style())
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.label(RichText::new(title).strong());
            ui.add_space(4.0);
            ui.monospace(body);
        });
}
