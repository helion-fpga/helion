//! Helion IDE — Vivado-class desktop window over the real Session engines.
//!
//! `--version` / `--doctor` never open a window (so they work headless and on CI).
//! The GUI paints [`helion_gui::IdeModel`]; every button and the Tcl box call into
//! that model, which is what the unit tests already prove is not a no-op.

use eframe::egui::{self, Color32, RichText, Sense, Stroke};
use helion_gui::{doctor, FlowStep, IdeModel, StepState};

fn main() -> eframe::Result {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--version") | Some("-V") | Some("version") => {
            println!("{}", doctor::version_line());
            return Ok(());
        }
        Some("--doctor") | Some("doctor") => {
            print!("{}", doctor::doctor_report());
            return Ok(());
        }
        Some("--help") | Some("-h") | Some("help") => {
            eprintln!(
                "helion-ide {}
  helion-ide              open the IDE window
  helion-ide --version
  helion-ide --doctor
  helion-ide --help",
                env!("CARGO_PKG_VERSION")
            );
            return Ok(());
        }
        Some(other) => {
            eprintln!("unknown argument {other}; try helion-ide --help");
            std::process::exit(2);
        }
        None => {}
    }
    run_gui()
}

fn run_gui() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 560.0])
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

    fn example(&self, name: &str) -> std::path::PathBuf {
        helion_device::Device::workspace_root()
            .join("examples")
            .join(name)
    }
}

impl eframe::App for HelionIde {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
                        RichText::new(self.model.part())
                            .monospace()
                            .color(Color32::from_rgb(0xb0, 0xb8, 0xc0)),
                    );
                    ui.separator();
                    ui.label(RichText::new("Flow").weak());
                    for step in FlowStep::ALL {
                        flow_button(ui, &mut self.model, step);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        if ui.button("Open hier.sv").clicked() {
                            let p = self.example("hier.sv");
                            let _ = self.model.open_source(&p);
                        }
                        if ui.button("Open blinky.sv").clicked() {
                            let p = self.example("blinky.sv");
                            let _ = self.model.open_source(&p);
                        }
                        if ui.button("Open counter.sv").clicked() {
                            let p = self.example("counter.sv");
                            let _ = self.model.open_source(&p);
                        }
                    });
                });
                ui.add_space(2.0);
            });

        egui::TopBottomPanel::bottom("console")
            .resizable(true)
            .default_height(220.0)
            .min_height(120.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Tcl Console").strong());
                    ui.label(
                        RichText::new(&self.model.status)
                            .monospace()
                            .color(Color32::from_rgb(0x9a, 0xa4, 0xae)),
                    );
                });
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .max_height(ui.available_height() - 28.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for line in &self.model.console {
                            let color = if line.ok {
                                Color32::from_rgb(0xc8, 0xd0, 0xd8)
                            } else {
                                Color32::from_rgb(0xe0, 0x6c, 0x75)
                            };
                            ui.monospace(RichText::new(format!("helion% {}", line.cmd)).color(
                                Color32::from_rgb(0x7e, 0xc8, 0xe3),
                            ));
                            if !line.out.is_empty() {
                                ui.monospace(RichText::new(&line.out).color(color));
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("helion%").monospace());
                    let edit = egui::TextEdit::singleline(&mut self.model.input)
                        .desired_width(f32::INFINITY)
                        .hint_text("synth_design / place_design / route_design / write_bitstream / report_timing …")
                        .font(egui::TextStyle::Monospace);
                    let resp = ui.add(edit);
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let _ = self.model.submit_input();
                        resp.request_focus();
                    }
                });
            });

        egui::SidePanel::left("tree")
            .resizable(true)
            .default_width(260.0)
            .min_width(180.0)
            .show(ctx, |ui| {
                ui.heading("Sources");
                if self.model.tree.sources.is_empty() {
                    ui.weak("Open an example, or `read_sv path` in the console.");
                }
                for src in &self.model.tree.sources {
                    ui.monospace(src.rsplit('/').next().unwrap_or(src));
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.heading("Netlist");
                    if let Some(top) = &self.model.tree.top {
                        ui.label(RichText::new(top).monospace().italics());
                    }
                });
                ui.add(
                    egui::TextEdit::singleline(&mut self.tree_filter)
                        .hint_text("filter cells/nets")
                        .desired_width(f32::INFINITY),
                );
                let filt = self.tree_filter.to_ascii_lowercase();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.collapsing(
                            format!("Cells ({})", self.model.tree.cells.len()),
                            |ui| {
                                for (name, kind) in &self.model.tree.cells {
                                    if !filt.is_empty()
                                        && !name.to_ascii_lowercase().contains(&filt)
                                        && !kind.to_ascii_lowercase().contains(&filt)
                                    {
                                        continue;
                                    }
                                    ui.horizontal(|ui| {
                                        ui.monospace(name);
                                        ui.label(
                                            RichText::new(kind)
                                                .small()
                                                .color(Color32::from_rgb(0x7e, 0xc8, 0xe3)),
                                        );
                                    });
                                }
                            },
                        );
                        ui.collapsing(format!("Nets ({})", self.model.tree.nets.len()), |ui| {
                            for n in &self.model.tree.nets {
                                if !filt.is_empty() && !n.to_ascii_lowercase().contains(&filt) {
                                    continue;
                                }
                                ui.monospace(n);
                            }
                        });
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Reports");
            ui.add_space(6.0);
            report_box(ui, "Timing (report_timing)", &self.model.timing_text());
            ui.add_space(8.0);
            report_box(
                ui,
                "Utilization (report_utilization)",
                &self.model.utilization_text(),
            );
            ui.add_space(8.0);
            let bits = match (self.model.bitstream_hash(), self.model.bitstream_bytes()) {
                (Some(h), Some(b)) => format!(
                    "hash={h:#010x} bytes={b} frames={}",
                    self.model.bitstream_frames().unwrap_or(0)
                ),
                _ => "no bitstream — run Bitstream".into(),
            };
            report_box(ui, "Bitstream", &bits);
            ui.add_space(12.0);
            ui.weak(
                "Every pane is the real Session: the same engines as `helion run` / `helion doctor`.",
            );
        });
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
