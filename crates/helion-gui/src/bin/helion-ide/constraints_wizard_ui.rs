//! Constraints Wizard presentation (egui).
//!
//! Pure data model, generation, and validation live in
//! `helion_gui::constraints_wizard`; this module owns the draft buffers,
//! step navigation, and the write + re-run-timing wiring.

use super::HelionIde;
use eframe::egui::{self};
use helion_gui::constraints_wizard as wiz;
use helion_gui::{IdeModel, JobKind};
use helion_ir::PortDir;
use std::path::PathBuf;

/// Screenshot affordance: `HELION_CONSTRAINTS_WIZARD=1` auto-opens the wizard,
/// `HELION_WIZARD_STEP=n` jumps straight to a step (0-4), and
/// `HELION_WIZARD_DEMO=1` prefills representative rows. Same spirit as the
/// existing `HELION_SHOT*` / `HELION_WORKSPACE` launch variables.
pub fn auto_open_from_env() -> Option<usize> {
    if std::env::var("HELION_CONSTRAINTS_WIZARD").as_deref() == Ok("1") {
        let step = std::env::var("HELION_WIZARD_STEP")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0)
            .min(4);
        Some(step)
    } else {
        None
    }
}

#[derive(Clone, Debug)]
pub struct ClockDraft {
    pub name: String,
    pub port: String,
    pub period_ns: String,
    pub duty_pct: String,
    pub generated: bool,
    pub master: String,
    pub divide_by: String,
    pub multiply_by: String,
    pub invert: bool,
}

#[derive(Clone, Debug)]
pub struct IoDelayDraft {
    pub port: String,
    pub clock: String,
    pub min_ns: String,
    pub max_ns: String,
}

#[derive(Clone, Debug)]
pub enum ExceptionDraft {
    FalsePath {
        from: String,
        from_is_clock: bool,
        to: String,
        to_is_clock: bool,
    },
    MaxDelay {
        from: String,
        to: String,
        delay_ns: String,
        datapath_only: bool,
    },
}

#[derive(Clone, Debug)]
pub enum TimingOutcome {
    Running,
    Done { report: String, wns: Option<i64> },
    Failed(String),
}

/// Draft state for the whole wizard. Lives on [`HelionIde`], not the model,
/// so engine-thread jobs (which clone the model) never disturb it.
#[derive(Clone, Debug)]
pub struct WizardUi {
    pub step: wiz::WizardStep,
    visited: Vec<wiz::WizardStep>,
    pub clocks: Vec<ClockDraft>,
    pub input_delays: Vec<IoDelayDraft>,
    pub output_delays: Vec<IoDelayDraft>,
    pub exceptions: Vec<ExceptionDraft>,
    pub errors: Vec<wiz::WizardError>,
    pub preview: Option<String>,
    pub written_path: Option<PathBuf>,
    pub timing: Option<TimingOutcome>,
}

impl WizardUi {
    pub fn new(model: &IdeModel) -> Self {
        let ports: Vec<String> = model
            .design()
            .map(|d| d.ports.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default();
        let default_port = ports
            .iter()
            .find(|p| p.as_str() == "clk")
            .or_else(|| ports.first())
            .cloned()
            .unwrap_or_default();
        let known: Vec<String> = model.constraints.clocks.iter().map(|c| c.name.clone()).collect();
        let mut w = Self {
            step: wiz::WizardStep::Clocks,
            visited: vec![wiz::WizardStep::Clocks],
            clocks: vec![ClockDraft {
                name: default_port.clone(),
                port: default_port,
                period_ns: "10.0".into(),
                duty_pct: "50".into(),
                generated: false,
                master: known.first().cloned().unwrap_or_default(),
                divide_by: "2".into(),
                multiply_by: "1".into(),
                invert: false,
            }],
            input_delays: Vec::new(),
            output_delays: Vec::new(),
            exceptions: Vec::new(),
            errors: Vec::new(),
            preview: None,
            written_path: None,
            timing: None,
        };
        // Screenshot/demo hook: prefill representative rows.
        if std::env::var("HELION_WIZARD_DEMO").as_deref() == Ok("1") {
            let in_ports: Vec<String> = model
                .design()
                .map(|d| {
                    d.ports
                        .iter()
                        .filter(|p| matches!(p.dir, PortDir::In | PortDir::Inout))
                        .map(|p| p.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            let out_ports: Vec<String> = model
                .design()
                .map(|d| {
                    d.ports
                        .iter()
                        .filter(|p| matches!(p.dir, PortDir::Out | PortDir::Inout))
                        .map(|p| p.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            let clk_name = w.clocks[0].name.clone();
            if let Some(p) = in_ports.first() {
                w.input_delays.push(IoDelayDraft {
                    port: p.clone(),
                    clock: clk_name.clone(),
                    min_ns: "0.5".into(),
                    max_ns: "1.5".into(),
                });
            }
            if let Some(p) = out_ports.first() {
                w.output_delays.push(IoDelayDraft {
                    port: p.clone(),
                    clock: clk_name.clone(),
                    min_ns: String::new(),
                    max_ns: "2.0".into(),
                });
            }
            if let (Some(a), Some(b)) = (in_ports.first(), out_ports.first()) {
                w.exceptions.push(ExceptionDraft::FalsePath {
                    from: a.clone(),
                    from_is_clock: false,
                    to: clk_name.clone(),
                    to_is_clock: true,
                });
                w.exceptions.push(ExceptionDraft::MaxDelay {
                    from: a.clone(),
                    to: b.clone(),
                    delay_ns: "8.0".into(),
                    datapath_only: true,
                });
            }
        }
        w
    }

    fn known_clocks(&self, model: &IdeModel) -> Vec<String> {
        let mut names: Vec<String> = model.constraints.clocks.iter().map(|c| c.name.clone()).collect();
        for c in &self.clocks {
            if !c.name.trim().is_empty() && !names.contains(&c.name) {
                names.push(c.name.clone());
            }
        }
        names
    }

    fn ports_of(&self, model: &IdeModel, dir: Option<PortDir>) -> Vec<String> {
        model
            .design()
            .map(|d| {
                d.ports
                    .iter()
                    .filter(|p| match dir {
                        None => true,
                        Some(PortDir::In) => matches!(p.dir, PortDir::In | PortDir::Inout),
                        Some(PortDir::Out) => matches!(p.dir, PortDir::Out | PortDir::Inout),
                        Some(PortDir::Inout) => matches!(p.dir, PortDir::Inout),
                    })
                    .map(|p| p.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Parse drafts into a [`wiz::WizardInput`], collecting number-format
    /// errors alongside (validation errors come from `wiz::validate`).
    fn build_input(&self) -> (wiz::WizardInput, Vec<wiz::WizardError>) {
        let mut errs = Vec::new();
        let req_f64 = |s: &str, what: &str, tag: &str, step: wiz::WizardStep, errs: &mut Vec<wiz::WizardError>| -> Option<f64> {
            let t = s.trim();
            if t.is_empty() {
                errs.push(wiz::WizardError { step, message: format!("{tag}{what} is required") });
                return None;
            }
            match t.parse::<f64>() {
                Ok(v) => Some(v),
                Err(_) => {
                    errs.push(wiz::WizardError { step, message: format!("{tag}{what} '{t}' is not a number") });
                    None
                }
            }
        };
        let opt_f64 = |s: &str, what: &str, tag: &str, step: wiz::WizardStep, errs: &mut Vec<wiz::WizardError>| -> Option<f64> {
            let t = s.trim();
            if t.is_empty() {
                return None;
            }
            match t.parse::<f64>() {
                Ok(v) => Some(v),
                Err(_) => {
                    errs.push(wiz::WizardError { step, message: format!("{tag}{what} '{t}' is not a number") });
                    None
                }
            }
        };

        let mut clocks = Vec::new();
        for (i, c) in self.clocks.iter().enumerate() {
            let tag = format!("Clock #{}: ", i + 1);
            let period_ns = req_f64(&c.period_ns, "period (ns)", &tag, wiz::WizardStep::Clocks, &mut errs).unwrap_or(-1.0);
            let duty_pct = req_f64(&c.duty_pct, "duty cycle (%)", &tag, wiz::WizardStep::Clocks, &mut errs).unwrap_or(-1.0);
            let mut parse_u32 = |s: &str, what: &str| -> u32 {
                let t = s.trim();
                match t.parse::<u32>() {
                    Ok(v) => v,
                    Err(_) => {
                        errs.push(wiz::WizardError { step: wiz::WizardStep::Clocks, message: format!("{tag}{what} '{t}' is not a positive integer") });
                        0
                    }
                }
            };
            clocks.push(wiz::WizardClock {
                name: c.name.trim().to_string(),
                port: c.port.trim().to_string(),
                period_ns,
                duty_pct,
                generated: c.generated,
                master: c.master.trim().to_string(),
                divide_by: parse_u32(&c.divide_by, "divide_by"),
                multiply_by: parse_u32(&c.multiply_by, "multiply_by"),
                invert: c.invert,
            });
        }

        let build_io = |list: &[IoDelayDraft], step: wiz::WizardStep, errs: &mut Vec<wiz::WizardError>| -> Vec<wiz::WizardIoDelay> {
            list.iter().enumerate().map(|(i, d)| {
                let tag = format!("{} #{}: ", step.title(), i + 1);
                wiz::WizardIoDelay {
                    port: d.port.trim().to_string(),
                    clock: d.clock.trim().to_string(),
                    min_ns: opt_f64(&d.min_ns, "min delay (ns)", &tag, step, errs),
                    max_ns: opt_f64(&d.max_ns, "max delay (ns)", &tag, step, errs),
                }
            }).collect()
        };

        let exceptions = self.exceptions.iter().map(|e| match e {
            ExceptionDraft::FalsePath { from, from_is_clock, to, to_is_clock } => {
                wiz::WizardException::FalsePath {
                    from: from.trim().to_string(),
                    from_is_clock: *from_is_clock,
                    to: to.trim().to_string(),
                    to_is_clock: *to_is_clock,
                }
            }
            ExceptionDraft::MaxDelay { from, to, delay_ns, datapath_only } => {
                let tag = "Max delay: ".to_string();
                let v = req_f64(delay_ns, "delay (ns)", &tag, wiz::WizardStep::Exceptions, &mut errs).unwrap_or(-1.0);
                wiz::WizardException::MaxDelay {
                    from: from.trim().to_string(),
                    to: to.trim().to_string(),
                    delay_ns: v,
                    datapath_only: *datapath_only,
                }
            }
        }).collect();

        (
            wiz::WizardInput {
                clocks,
                input_delays: build_io(&self.input_delays, wiz::WizardStep::InputDelays, &mut errs),
                output_delays: build_io(&self.output_delays, wiz::WizardStep::OutputDelays, &mut errs),
                exceptions,
            },
            errs,
        )
    }

    /// Full validate: parse errors + semantic validation. Returns true when
    /// the given step (or all steps for Review) is clean.
    fn revalidate(&mut self, model: &IdeModel, only: Option<wiz::WizardStep>) -> bool {
        // Drafts changed and re-validated: previously written file / timing
        // results no longer correspond to this input.
        if only.is_some() {
            self.written_path = None;
            self.timing = None;
            self.preview = None;
        }
        let (input, mut errs) = self.build_input();
        // Only externally-defined clocks here: validate() adds the wizard's
        // own clock names on top for reference checks (passing them in would
        // flag every wizard clock as a duplicate).
        let known: Vec<String> =
            model.constraints.clocks.iter().map(|c| c.name.clone()).collect();
        let design = match model.design() {
            Some(d) => d,
            None => {
                errs.push(wiz::WizardError { step: wiz::WizardStep::Clocks, message: "No design loaded — open RTL first.".into() });
                self.errors = errs;
                self.preview = None;
                return false;
            }
        };
        errs.extend(wiz::validate(&input, design, &known));
        let keep: Vec<wiz::WizardError> = match only {
            Some(s) => errs.into_iter().filter(|e| e.step == s).collect(),
            None => errs,
        };
        let ok = keep.is_empty();
        self.errors = keep;
        if only.is_none() && ok {
            let text = wiz::generate_sdc(&input, &design.name);
            match wiz::validate_sdc(&text) {
                Ok(()) => self.preview = Some(text),
                Err(e) => {
                    self.errors.push(wiz::WizardError { step: wiz::WizardStep::Review, message: format!("Generated SDC failed to parse: {e}") });
                    self.preview = None;
                }
            }
        } else if only.is_none() {
            self.preview = None;
        }
        match only {
            Some(_) => ok,
            None => ok && self.preview.is_some(),
        }
    }

    fn goto(&mut self, step: wiz::WizardStep) {
        self.step = step;
        if !self.visited.contains(&step) {
            self.visited.push(step);
        }
        self.errors.clear();
    }

    /// Jump to a step by index (screenshot/demo hook).
    pub fn set_step_idx(&mut self, idx: usize) {
        let idx = idx.min(wiz::WizardStep::ALL.len() - 1);
        for s in wiz::WizardStep::ALL.iter().take(idx + 1) {
            if !self.visited.contains(s) {
                self.visited.push(*s);
            }
        }
        self.step = wiz::WizardStep::ALL[idx];
        self.errors.clear();
    }
}

/// Where the wizard writes: the SDC already open in the Constraints tab, else
/// a sibling .sdc/.xdc of the sources, else `<source-stem>.sdc` next to the
/// first source, else `./constraints.sdc`.
fn target_path(model: &IdeModel) -> PathBuf {
    if let Some(p) = &model.sdc_editor_path {
        return p.clone();
    }
    for src in &model.tree.sources {
        let q = PathBuf::from(src);
        let ext = q.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if (ext == "sdc" || ext == "xdc") && q.is_file() {
            return q;
        }
    }
    if let Some(src) = model.tree.sources.first() {
        let q = PathBuf::from(src);
        if let Some(parent) = q.parent() {
            let stem = q.file_stem().and_then(|s| s.to_str()).unwrap_or("constraints");
            return parent.join(format!("{stem}.sdc"));
        }
    }
    PathBuf::from("constraints.sdc")
}

/// Append (or create) the generated SDC and reload it through the same path
/// the Constraints tab's Save button uses, so STA picks it up.
fn write_sdc(app: &mut HelionIde, text: &str) -> Result<PathBuf, String> {
    let target = target_path(&app.model);
    if target.is_file() {
        app.model.open_sdc_editor(&target)?;
        if !app.model.sdc_editor_text.ends_with('\n') {
            app.model.sdc_editor_text.push('\n');
        }
        app.model.sdc_editor_text.push_str(text);
        app.model.sdc_editor_dirty = true;
        app.model.save_sdc_editor()?;
    } else {
        std::fs::write(&target, text)
            .map_err(|e| format!("write {}: {e}", target.display()))?;
        app.model.open_sdc_editor(&target)?;
    }
    Ok(target)
}

/// Called from `poll_job` when a `JobKind::ReportTiming` finishes.
pub fn on_timing_done(app: &mut HelionIde, result: Result<String, String>) {
    if let Some(w) = app.constraints_wizard.as_mut() {
        w.timing = Some(match result {
            Ok(report) => {
                let wns = wiz::parse_wns(&report);
                TimingOutcome::Done { report, wns }
            }
            Err(e) => TimingOutcome::Failed(e),
        });
    }
}

fn error_box(ui: &mut egui::Ui, errors: &[wiz::WizardError]) {
    if errors.is_empty() {
        return;
    }
    ui.add_space(6.0);
    egui::Frame::NONE
        .fill(egui::Color32::from_rgb(64, 24, 24))
        .inner_margin(8.0)
        .corner_radius(4.0)
        .show(ui, |ui| {
            ui.label(egui::RichText::new(format!("{} problem(s) to fix:", errors.len())).strong().color(egui::Color32::from_rgb(255, 150, 150)));
            for e in errors {
                let msg = format!("• [{}] {}", e.step.title(), e.message);
                ui.label(
                    egui::RichText::new(msg).color(egui::Color32::from_rgb(255, 190, 190)),
                );
            }
        });
}

fn paint_clocks(ui: &mut egui::Ui, app: &mut HelionIde) {
    let ports = app.constraints_wizard.as_ref().map(|w| w.ports_of(&app.model, None)).unwrap_or_default();
    let mut remove: Option<usize> = None;
    let n = app.constraints_wizard.as_ref().map(|w| w.clocks.len()).unwrap_or(0);
    for i in 0..n {
        // Owned master-clock candidates: model clocks + other wizard clocks.
        // Computed before the mutable row borrow below.
        let master_opts: Vec<String> = {
            let w = app.constraints_wizard.as_ref().unwrap();
            let cur = w.clocks[i].name.clone();
            let mut opts: Vec<String> =
                app.model.constraints.clocks.iter().map(|c| c.name.clone()).collect();
            for cc in &w.clocks {
                if !cc.generated && cc.name != cur && !opts.contains(&cc.name) {
                    opts.push(cc.name.clone());
                }
            }
            opts
        };
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("Clock {}", i + 1)).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if n > 1 && ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            });
            let w = app.constraints_wizard.as_mut().unwrap();
            let c = &mut w.clocks[i];
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut c.name);
                ui.label("Port");
                egui::ComboBox::from_id_salt(("wclk_port", i))
                    .selected_text(c.port.as_str())
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for p in &ports {
                            ui.selectable_value(&mut c.port, p.clone(), p.as_str());
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Period (ns)");
                ui.add(egui::TextEdit::singleline(&mut c.period_ns).desired_width(80.0));
                ui.label("Duty (%)");
                ui.add(egui::TextEdit::singleline(&mut c.duty_pct).desired_width(60.0));
                ui.weak("waveform recorded for export; STA keys off period");
            });
            ui.checkbox(&mut c.generated, "Generated clock (create_generated_clock)");
            if c.generated {
                ui.horizontal(|ui| {
                    ui.label("Master");
                    egui::ComboBox::from_id_salt(("wclk_master", i))
                        .selected_text(c.master.as_str())
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            for m in &master_opts {
                                ui.selectable_value(&mut c.master, m.clone(), m.as_str());
                            }
                        });
                    ui.label("÷");
                    ui.add(egui::TextEdit::singleline(&mut c.divide_by).desired_width(50.0));
                    ui.label("×");
                    ui.add(egui::TextEdit::singleline(&mut c.multiply_by).desired_width(50.0));
                    ui.checkbox(&mut c.invert, "Invert");
                });
            }
        });
        ui.add_space(6.0);
    }
    if let Some(i) = remove {
        app.constraints_wizard.as_mut().unwrap().clocks.remove(i);
    }
    if ui.button("+ Add clock").clicked() {
        let w = app.constraints_wizard.as_mut().unwrap();
        let port = ports.first().cloned().unwrap_or_default();
        let known_first = w.known_clocks(&app.model).first().cloned().unwrap_or_default();
        w.clocks.push(ClockDraft {
            name: String::new(),
            port: port.clone(),
            period_ns: "10.0".into(),
            duty_pct: "50".into(),
            generated: false,
            master: known_first,
            divide_by: "2".into(),
            multiply_by: "1".into(),
            invert: false,
        });
    }
}

fn paint_io_delays(ui: &mut egui::Ui, app: &mut HelionIde, is_input: bool) {
    let step = if is_input { wiz::WizardStep::InputDelays } else { wiz::WizardStep::OutputDelays };
    let dir = if is_input { PortDir::In } else { PortDir::Out };
    let ports = app.constraints_wizard.as_ref().map(|w| w.ports_of(&app.model, Some(dir))).unwrap_or_default();
    let mut remove: Option<usize> = None;
    let n = app.constraints_wizard.as_ref().map(|w| if is_input { w.input_delays.len() } else { w.output_delays.len() }).unwrap_or(0);
    for i in 0..n {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{} {}", step.title(), i + 1)).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            });
            // Borrow dance: read option lists first, then mutate the draft.
            let w = app.constraints_wizard.as_mut().unwrap();
            let list = if is_input { &mut w.input_delays } else { &mut w.output_delays };
            let d = &mut list[i];
            ui.horizontal(|ui| {
                ui.label("Port");
                egui::ComboBox::from_id_salt(("wio_port", is_input, i))
                    .selected_text(d.port.as_str())
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for p in &ports {
                            ui.selectable_value(&mut d.port, p.clone(), p.as_str());
                        }
                    });
            });
            // Clock list needs the wizard; re-borrow after the combo.
            let w = app.constraints_wizard.as_mut().unwrap();
            let clocks = w.known_clocks(&app.model);
            let list = if is_input { &mut w.input_delays } else { &mut w.output_delays };
            let d = &mut list[i];
            ui.horizontal(|ui| {
                ui.label("Clock");
                egui::ComboBox::from_id_salt(("wio_clk", is_input, i))
                    .selected_text(d.clock.as_str())
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for c in &clocks {
                            ui.selectable_value(&mut d.clock, c.clone(), c.as_str());
                        }
                    });
                ui.label("Min (ns)");
                ui.add(egui::TextEdit::singleline(&mut d.min_ns).desired_width(70.0).hint_text("hold"));
                ui.label("Max (ns)");
                ui.add(egui::TextEdit::singleline(&mut d.max_ns).desired_width(70.0).hint_text("setup"));
            });
        });
        ui.add_space(6.0);
    }
    if let Some(i) = remove {
        let w = app.constraints_wizard.as_mut().unwrap();
        if is_input { w.input_delays.remove(i); } else { w.output_delays.remove(i); };
    }
    if ui.button(if is_input { "+ Add input delay" } else { "+ Add output delay" }).clicked() {
        let w = app.constraints_wizard.as_mut().unwrap();
        let port = ports.first().cloned().unwrap_or_default();
        let clock = w.known_clocks(&app.model).first().cloned().unwrap_or_default();
        let draft = IoDelayDraft { port, clock, min_ns: String::new(), max_ns: String::new() };
        if is_input { w.input_delays.push(draft); } else { w.output_delays.push(draft); };
    }
}

fn paint_exceptions(ui: &mut egui::Ui, app: &mut HelionIde) {
    let ports = app.constraints_wizard.as_ref().map(|w| w.ports_of(&app.model, None)).unwrap_or_default();
    let mut remove: Option<usize> = None;
    let mut new_kind: Option<bool> = None; // true = false-path, false = max-delay
    let n = app.constraints_wizard.as_ref().map(|w| w.exceptions.len()).unwrap_or(0);
    for i in 0..n {
        ui.group(|ui| {
            // Read kind first for the header without holding the borrow.
            let is_fp = matches!(app.constraints_wizard.as_ref().unwrap().exceptions[i], ExceptionDraft::FalsePath { .. });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(if is_fp { format!("False path {}", i + 1) } else { format!("Max delay {}", i + 1) }).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            });
            let w = app.constraints_wizard.as_mut().unwrap();
            let clocks = w.known_clocks(&app.model);
            match &mut w.exceptions[i] {
                ExceptionDraft::FalsePath { from, from_is_clock, to, to_is_clock } => {
                    let sides: [(&str, &mut String, &mut bool); 2] = [
                        ("From", &mut *from, &mut *from_is_clock),
                        ("To", &mut *to, &mut *to_is_clock),
                    ];
                    for (lbl, val, is_clk) in sides {
                        ui.horizontal(|ui| {
                            ui.label(lbl);
                            let opts: &[String] = if *is_clk { &clocks } else { &ports };
                            egui::ComboBox::from_id_salt(("wfp", i, lbl))
                                .selected_text(val.as_str())
                                .width(150.0)
                                .show_ui(ui, |ui| {
                                    for o in opts {
                                        ui.selectable_value(&mut *val, o.clone(), o.as_str());
                                    }
                                });
                            ui.radio_value(&mut *is_clk, false, "Port");
                            ui.radio_value(&mut *is_clk, true, "Clock");
                        });
                    }
                }
                ExceptionDraft::MaxDelay { from, to, delay_ns, datapath_only } => {
                    ui.horizontal(|ui| {
                        ui.label("From");
                        egui::ComboBox::from_id_salt(("wmd_from", i))
                            .selected_text(from.as_str())
                            .width(140.0)
                            .show_ui(ui, |ui| {
                                for p in &ports {
                                    ui.selectable_value(&mut *from, p.clone(), p.as_str());
                                }
                            });
                        ui.label("To");
                        egui::ComboBox::from_id_salt(("wmd_to", i))
                            .selected_text(to.as_str())
                            .width(140.0)
                            .show_ui(ui, |ui| {
                                for p in &ports {
                                    ui.selectable_value(&mut *to, p.clone(), p.as_str());
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Delay (ns)");
                        ui.add(egui::TextEdit::singleline(delay_ns).desired_width(80.0));
                        ui.checkbox(datapath_only, "-datapath_only");
                    });
                }
            }
        });
        ui.add_space(6.0);
    }
    if let Some(i) = remove {
        app.constraints_wizard.as_mut().unwrap().exceptions.remove(i);
    }
    ui.horizontal(|ui| {
        if ui.button("+ False path").clicked() {
            new_kind = Some(true);
        }
        if ui.button("+ Max delay").clicked() {
            new_kind = Some(false);
        }
    });
    if let Some(is_fp) = new_kind {
        let w = app.constraints_wizard.as_mut().unwrap();
        let port = w.ports_of(&app.model, None).first().cloned().unwrap_or_default();
        if is_fp {
            w.exceptions.push(ExceptionDraft::FalsePath {
                from: port.clone(), from_is_clock: false, to: port, to_is_clock: false,
            });
        } else {
            w.exceptions.push(ExceptionDraft::MaxDelay {
                from: port.clone(), to: port, delay_ns: String::new(), datapath_only: true,
            });
        }
    }
}

fn paint_review(ui: &mut egui::Ui, app: &mut HelionIde) {
    // Lazily build the preview (covers the HELION_WIZARD_STEP jump-in, which
    // skips the Next-button validation that normally builds it).
    {
        let w = app.constraints_wizard.as_mut().unwrap();
        if w.preview.is_none() && w.errors.is_empty() {
            w.revalidate(&app.model, None);
        }
    }
    let target = target_path(&app.model);
    let target_name = target.file_name().and_then(|n| n.to_str()).unwrap_or("constraints.sdc").to_string();

    // Snapshot preview/written/timing for display without holding borrows.
    let (preview, written, timing) = {
        let w = app.constraints_wizard.as_ref().unwrap();
        (w.preview.clone(), w.written_path.clone(), w.timing.clone())
    };

    if let Some(text) = &preview {
        ui.label(egui::RichText::new("Generated SDC — parses clean against the repo SDC frontend:").strong());
        ui.add(
            egui::TextEdit::multiline(&mut text.clone())
                .code_editor()
                .desired_rows(12)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Target file:");
            ui.monospace(target.display().to_string());
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button(format!("Write to {target_name}")).clicked() {
                match write_sdc(app, text) {
                    Ok(p) => {
                        app.constraints_wizard.as_mut().unwrap().written_path = Some(p);
                        app.toasts.push(
                            super::toasts::ToastKind::Ok,
                            format!("Constraints written to {target_name}"),
                        );
                    }
                    Err(e) => {
                        app.constraints_wizard.as_mut().unwrap().errors = vec![wiz::WizardError {
                            step: wiz::WizardStep::Review,
                            message: e,
                        }];
                    }
                }
            }
            let can_rerun = written.is_some() && !app.busy;
            if ui.add_enabled(can_rerun, egui::Button::new("Re-run timing")).clicked() {
                app.constraints_wizard.as_mut().unwrap().timing = Some(TimingOutcome::Running);
                app.submit_job(JobKind::ReportTiming);
            }
            if app.busy {
                ui.spinner();
                ui.weak(&app.progress);
            }
        });
        if written.is_none() {
            ui.weak("Write the file first — re-run timing needs the new constraints applied.");
        }
    }

    match timing {
        Some(TimingOutcome::Running) => {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Timing analysis running on the engine thread…");
            });
        }
        Some(TimingOutcome::Done { report, wns }) => {
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Timing after new constraints:").strong());
            if let Some(w) = wns {
                let (txt, color) = if w < 0 {
                    (format!("WNS = {w} ps — VIOLATED"), egui::Color32::from_rgb(255, 120, 120))
                } else {
                    (format!("WNS = {w} ps — met"), egui::Color32::from_rgb(140, 230, 140))
                };
                ui.label(egui::RichText::new(txt).strong().color(color).monospace());
            } else {
                ui.weak("WNS not found in report.");
            }
            ui.collapsing("Full report", |ui| {
                ui.monospace(&report);
            });
        }
        Some(TimingOutcome::Failed(e)) => {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(format!("Timing run failed: {e}")).color(egui::Color32::from_rgb(255, 120, 120)));
        }
        None => {}
    }
}

/// Paint the wizard, replacing the Constraints pane while open.
pub fn paint(ui: &mut egui::Ui, app: &mut HelionIde) {
    let has_design = app.model.design().is_some();

    // Header: title + stepper + close.
    ui.horizontal(|ui| {
        ui.heading("Constraints Wizard");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("✕ Close").clicked() {
                app.constraints_wizard = None;
            }
        });
    });
    if app.constraints_wizard.is_none() {
        return;
    }
    ui.add_space(2.0);

    // Stepper.
    let (step, visited) = {
        let w = app.constraints_wizard.as_ref().unwrap();
        (w.step, w.visited.clone())
    };
    ui.horizontal(|ui| {
        for (i, s) in wiz::WizardStep::ALL.iter().enumerate() {
            let done = visited.contains(s) && *s != step;
            let label = if done {
                format!("✓ {}", s.title())
            } else {
                format!("{} {}", i + 1, s.title())
            };
            let mut text = egui::RichText::new(label);
            if *s == step {
                text = text.strong().underline();
            } else if done {
                text = text.weak();
            }
            let enabled = visited.contains(s);
            if ui.add_enabled(enabled, egui::Button::new(text).frame(false)).clicked() {
                app.constraints_wizard.as_mut().unwrap().goto(*s);
                return;
            }
            if i + 1 < wiz::WizardStep::ALL.len() {
                ui.weak("→");
            }
        }
    });
    ui.label(egui::RichText::new(step.hint()).weak().italics());
    ui.separator();

    if !has_design {
        ui.add_space(12.0);
        ui.label("No design loaded. Open an HDL file first, then run the wizard.");
        return;
    }

    // No inner ScrollArea here: the wizard is page flow content, always painted
    // inside the Timing page scroll (timing_canvas_v6), which owns scrolling.
    // A nested area would shrink to its content and never trap the wheel, so
    // wheeling the form would scroll the page instead (nested-scroll fix).
    // (The editor uses the same one-outer-scroll architecture.)
    match step {
        wiz::WizardStep::Clocks => paint_clocks(ui, app),
        wiz::WizardStep::InputDelays => paint_io_delays(ui, app, true),
        wiz::WizardStep::OutputDelays => paint_io_delays(ui, app, false),
        wiz::WizardStep::Exceptions => paint_exceptions(ui, app),
        wiz::WizardStep::Review => paint_review(ui, app),
    }
    let errors = app.constraints_wizard.as_ref().unwrap().errors.clone();
    error_box(ui, &errors);

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let idx = wiz::WizardStep::ALL.iter().position(|s| *s == step).unwrap_or(0);
        if idx > 0 && ui.button("← Back").clicked() {
            app.constraints_wizard.as_mut().unwrap().goto(wiz::WizardStep::ALL[idx - 1]);
            return;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if idx + 1 < wiz::WizardStep::ALL.len() {
                if ui.button("Next →").clicked() {
                    let next = wiz::WizardStep::ALL[idx + 1];
                    // Validate the current step before advancing.
                    let ok = {
                        let w = app.constraints_wizard.as_mut().unwrap();
                        w.revalidate(&app.model, Some(step))
                    };
                    if ok {
                        app.constraints_wizard.as_mut().unwrap().goto(next);
                        // Pre-build the review preview the moment we land on it.
                        if next == wiz::WizardStep::Review {
                            let w = app.constraints_wizard.as_mut().unwrap();
                            w.revalidate(&app.model, None);
                        }
                    }
                }
            } else {
                ui.weak("Edit the SDC any time in the Constraints tab.");
            }
        });
    });
}
