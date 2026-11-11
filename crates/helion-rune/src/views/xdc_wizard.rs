//! Constraints Wizard: a five-step guided editor for timing constraints
//! (Clocks, Input delays, Output delays, Exceptions, Review), ported from
//! `helion-gui` `constraints_wizard.rs` (model, validation, SDC text) and
//! `bin/helion-ide/constraints_wizard_ui.rs` (multi-row drafts, write to
//! disk, re-run timing with WNS before/after).
//!
//! - Every step is a table with add/remove rows: primary clocks, generated
//!   clocks (master, ÷, ×, invert), input/output delays (-min/-max), false
//!   paths (port or clock endpoints) and max delays (`-datapath_only`).
//! - Validation runs live against the design's real top-level ports
//!   ([`crate::live::live_design`]) and the clocks the project's XDC already
//!   defines; each row then round-trips its own SDC through
//!   [`helion_sta::load_xdc`], so a row only reads clean when the engine
//!   loads it. Problems render under the row that owns them.
//! - Review previews the generated SDC, writes it into the project's
//!   constraints file on disk (adding a new file to `constrs_1` through
//!   [`crate::project::commit_add_sources`]), and re-runs STA on a worker
//!   ([`crate::jobs`]) over the last run with exactly that file, showing WNS
//!   before and after.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune-kit/src/dialog.rs`: `Dialog::new/open/width/fit/on_close`.
//! - `crates/rune-kit/src/overlay.rs`: `Popover` for port/clock pickers.
//! - `crates/rune/src/el.rs`: `text_input`, `edit_access` (row-id fields).

use std::path::{Path, PathBuf};

use rune::TextEdit;

use crate::state::{AppState, ConsoleLine, MsgSev, ToastKind};

mod sdc;
mod view;

pub use sdc::{Ctx, Issue, KnownClock, issues, sdc_text, validate_sdc};
use sdc::clock_names;
pub use view::xdc_dialog;


/// Wizard pages, mirroring `constraints_wizard::WizardStep`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XdcStep {
    #[default]
    Clocks,
    InputDelays,
    OutputDelays,
    Exceptions,
    Review,
}

impl XdcStep {
    pub const ALL: [XdcStep; 5] = [
        XdcStep::Clocks,
        XdcStep::InputDelays,
        XdcStep::OutputDelays,
        XdcStep::Exceptions,
        XdcStep::Review,
    ];

    pub fn title(self) -> &'static str {
        match self {
            XdcStep::Clocks => "Clocks",
            XdcStep::InputDelays => "Input delays",
            XdcStep::OutputDelays => "Output delays",
            XdcStep::Exceptions => "Exceptions",
            XdcStep::Review => "Review & generate",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            XdcStep::Clocks => {
                "Define each clock: the port it enters on, its period and duty cycle; derive generated clocks from a master."
            }
            XdcStep::InputDelays => {
                "Constrain input ports relative to a clock (-max for setup, -min for hold)."
            }
            XdcStep::OutputDelays => {
                "Constrain output ports relative to a clock (-max for setup, -min for hold)."
            }
            XdcStep::Exceptions => {
                "False paths and max-delay overrides for paths the clocks don't cover."
            }
            XdcStep::Review => "Check the generated SDC, write it, then re-run timing.",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    fn next(self) -> Option<XdcStep> {
        Self::ALL.get(self.index() + 1).copied()
    }

    fn prev(self) -> Option<XdcStep> {
        self.index()
            .checked_sub(1)
            .and_then(|p| Self::ALL.get(p).copied())
    }

    /// Tables this step owns (Review owns all of them).
    fn lists(self) -> &'static [List] {
        match self {
            XdcStep::Clocks => &[List::Clock, List::Gen],
            XdcStep::InputDelays => &[List::In],
            XdcStep::OutputDelays => &[List::Out],
            XdcStep::Exceptions => &[List::Fp, List::Md],
            XdcStep::Review => &[
                List::Clock,
                List::Gen,
                List::In,
                List::Out,
                List::Fp,
                List::Md,
            ],
        }
    }
}

/// The six row tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum List {
    Clock,
    Gen,
    In,
    Out,
    Fp,
    Md,
}

impl List {
    fn step(self) -> XdcStep {
        match self {
            List::Clock | List::Gen => XdcStep::Clocks,
            List::In => XdcStep::InputDelays,
            List::Out => XdcStep::OutputDelays,
            List::Fp | List::Md => XdcStep::Exceptions,
        }
    }

    fn noun(self) -> &'static str {
        match self {
            List::Clock => "Clock",
            List::Gen => "Generated clock",
            List::In => "Input delay",
            List::Out => "Output delay",
            List::Fp => "False path",
            List::Md => "Max delay",
        }
    }

    fn key(self) -> &'static str {
        match self {
            List::Clock => "clk",
            List::Gen => "gen",
            List::In => "in",
            List::Out => "out",
            List::Fp => "fp",
            List::Md => "md",
        }
    }
}

/// Every editable text cell, addressed with its row id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    ClkName,
    ClkPort,
    ClkPeriod,
    ClkDuty,
    GenName,
    GenMaster,
    GenTarget,
    GenDiv,
    GenMul,
    InPort,
    InClock,
    InMin,
    InMax,
    OutPort,
    OutClock,
    OutMin,
    OutMax,
    FpFrom,
    FpTo,
    MdFrom,
    MdTo,
    MdDelay,
}

/// Open picker popover: which cell of which row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    pub field: Field,
    pub id: u32,
}

/// One `create_clock` row.
#[derive(Clone, Debug)]
pub struct ClockRow {
    pub id: u32,
    pub name: TextEdit,
    pub port: TextEdit,
    pub period_ns: TextEdit,
    pub duty_pct: TextEdit,
}

/// One `create_generated_clock` row.
#[derive(Clone, Debug)]
pub struct GenClockRow {
    pub id: u32,
    pub name: TextEdit,
    pub master: TextEdit,
    /// Pin or port the generated clock is defined on.
    pub target: TextEdit,
    pub divide_by: TextEdit,
    pub multiply_by: TextEdit,
    pub invert: bool,
}

/// One `set_input_delay` / `set_output_delay` row.
#[derive(Clone, Debug)]
pub struct DelayRow {
    pub id: u32,
    pub port: TextEdit,
    pub clock: TextEdit,
    pub min_ns: TextEdit,
    pub max_ns: TextEdit,
}

/// One `set_false_path` row; each endpoint is a port or a clock.
#[derive(Clone, Debug)]
pub struct FalsePathRow {
    pub id: u32,
    pub from: TextEdit,
    pub from_clock: bool,
    pub to: TextEdit,
    pub to_clock: bool,
}

/// One `set_max_delay` row.
#[derive(Clone, Debug)]
pub struct MaxDelayRow {
    pub id: u32,
    pub from: TextEdit,
    pub to: TextEdit,
    pub delay_ns: TextEdit,
    pub datapath_only: bool,
}

/// Re-run timing outcome (old `TimingOutcome`).
#[derive(Clone, Debug, PartialEq)]
pub enum TimingOutcome {
    /// No synthesized design yet: `synth_1` is running and timing re-runs
    /// when it completes ([`on_run_finished`]).
    AwaitingRun,
    Running,
    Done {
        before: Option<i64>,
        after: Option<i64>,
    },
    Failed(String),
}

/// What "Write to" put on disk: the file and the exact SDC appended, so a
/// later draft edit reads as "not written yet".
#[derive(Clone, Debug)]
pub struct Written {
    pub path: PathBuf,
    pub sdc: String,
    /// The file was new and joined `constrs_1`.
    pub added: bool,
}

/// Whole wizard state (lives in `DialogState::xdc`).
#[derive(Clone, Debug, Default)]
pub struct XdcWizard {
    pub step: XdcStep,
    /// Furthest step reached (the rail only jumps to visited steps).
    pub visited: usize,
    pub clocks: Vec<ClockRow>,
    pub generated: Vec<GenClockRow>,
    pub inputs: Vec<DelayRow>,
    pub outputs: Vec<DelayRow>,
    pub false_paths: Vec<FalsePathRow>,
    pub max_delays: Vec<MaxDelayRow>,
    /// Footer message (Next/Finish refused, write failure).
    pub error: Option<String>,
    pub pop: Option<Pick>,
    pub written: Option<Written>,
    pub timing: Option<TimingOutcome>,
    next_id: u32,
    /// Target for a field handler whose row was removed under it.
    scratch: TextEdit,
}

macro_rules! row_field {
    ($w:expr, $list:ident, $id:expr, $f:ident) => {
        $w.$list.iter_mut().find(|r| r.id == $id).map(|r| &mut r.$f)
    };
}

macro_rules! row_ref {
    ($w:expr, $list:ident, $id:expr, $f:ident) => {
        $w.$list.iter().find(|r| r.id == $id).map(|r| &r.$f)
    };
}

impl XdcWizard {
    fn fresh_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    /// The text field behind `(field, id)`; a removed row's handler edits a
    /// scratch field instead of panicking.
    pub fn edit_mut(&mut self, field: Field, id: u32) -> &mut TextEdit {
        let hit = match field {
            Field::ClkName => row_field!(self, clocks, id, name),
            Field::ClkPort => row_field!(self, clocks, id, port),
            Field::ClkPeriod => row_field!(self, clocks, id, period_ns),
            Field::ClkDuty => row_field!(self, clocks, id, duty_pct),
            Field::GenName => row_field!(self, generated, id, name),
            Field::GenMaster => row_field!(self, generated, id, master),
            Field::GenTarget => row_field!(self, generated, id, target),
            Field::GenDiv => row_field!(self, generated, id, divide_by),
            Field::GenMul => row_field!(self, generated, id, multiply_by),
            Field::InPort => row_field!(self, inputs, id, port),
            Field::InClock => row_field!(self, inputs, id, clock),
            Field::InMin => row_field!(self, inputs, id, min_ns),
            Field::InMax => row_field!(self, inputs, id, max_ns),
            Field::OutPort => row_field!(self, outputs, id, port),
            Field::OutClock => row_field!(self, outputs, id, clock),
            Field::OutMin => row_field!(self, outputs, id, min_ns),
            Field::OutMax => row_field!(self, outputs, id, max_ns),
            Field::FpFrom => row_field!(self, false_paths, id, from),
            Field::FpTo => row_field!(self, false_paths, id, to),
            Field::MdFrom => row_field!(self, max_delays, id, from),
            Field::MdTo => row_field!(self, max_delays, id, to),
            Field::MdDelay => row_field!(self, max_delays, id, delay_ns),
        };
        hit.unwrap_or(&mut self.scratch)
    }

    /// Read side of [`Self::edit_mut`].
    pub fn edit_ref(&self, field: Field, id: u32) -> &TextEdit {
        let hit = match field {
            Field::ClkName => row_ref!(self, clocks, id, name),
            Field::ClkPort => row_ref!(self, clocks, id, port),
            Field::ClkPeriod => row_ref!(self, clocks, id, period_ns),
            Field::ClkDuty => row_ref!(self, clocks, id, duty_pct),
            Field::GenName => row_ref!(self, generated, id, name),
            Field::GenMaster => row_ref!(self, generated, id, master),
            Field::GenTarget => row_ref!(self, generated, id, target),
            Field::GenDiv => row_ref!(self, generated, id, divide_by),
            Field::GenMul => row_ref!(self, generated, id, multiply_by),
            Field::InPort => row_ref!(self, inputs, id, port),
            Field::InClock => row_ref!(self, inputs, id, clock),
            Field::InMin => row_ref!(self, inputs, id, min_ns),
            Field::InMax => row_ref!(self, inputs, id, max_ns),
            Field::OutPort => row_ref!(self, outputs, id, port),
            Field::OutClock => row_ref!(self, outputs, id, clock),
            Field::OutMin => row_ref!(self, outputs, id, min_ns),
            Field::OutMax => row_ref!(self, outputs, id, max_ns),
            Field::FpFrom => row_ref!(self, false_paths, id, from),
            Field::FpTo => row_ref!(self, false_paths, id, to),
            Field::MdFrom => row_ref!(self, max_delays, id, from),
            Field::MdTo => row_ref!(self, max_delays, id, to),
            Field::MdDelay => row_ref!(self, max_delays, id, delay_ns),
        };
        hit.unwrap_or(&self.scratch)
    }

    fn text(&self, field: Field, id: u32) -> &str {
        &self.edit_ref(field, id).text
    }

    fn remove(&mut self, list: List, id: u32) {
        match list {
            List::Clock => self.clocks.retain(|r| r.id != id),
            List::Gen => self.generated.retain(|r| r.id != id),
            List::In => self.inputs.retain(|r| r.id != id),
            List::Out => self.outputs.retain(|r| r.id != id),
            List::Fp => self.false_paths.retain(|r| r.id != id),
            List::Md => self.max_delays.retain(|r| r.id != id),
        }
        self.pop = None;
        self.error = None;
    }

    pub fn add_clock(&mut self, name: &str, port: &str, period_ns: &str, duty_pct: &str) -> u32 {
        let id = self.fresh_id();
        self.clocks.push(ClockRow {
            id,
            name: TextEdit::new(name),
            port: TextEdit::new(port),
            period_ns: TextEdit::new(period_ns),
            duty_pct: TextEdit::new(duty_pct),
        });
        id
    }

    pub fn add_generated(&mut self, name: &str, master: &str, target: &str, div: &str) -> u32 {
        let id = self.fresh_id();
        self.generated.push(GenClockRow {
            id,
            name: TextEdit::new(name),
            master: TextEdit::new(master),
            target: TextEdit::new(target),
            divide_by: TextEdit::new(div),
            multiply_by: TextEdit::new("1"),
            invert: false,
        });
        id
    }

    pub fn add_delay(
        &mut self,
        output: bool,
        port: &str,
        clock: &str,
        min: &str,
        max: &str,
    ) -> u32 {
        let id = self.fresh_id();
        let r = DelayRow {
            id,
            port: TextEdit::new(port),
            clock: TextEdit::new(clock),
            min_ns: TextEdit::new(min),
            max_ns: TextEdit::new(max),
        };
        if output {
            self.outputs.push(r);
        } else {
            self.inputs.push(r);
        }
        id
    }

    pub fn add_false_path(
        &mut self,
        from: &str,
        from_clock: bool,
        to: &str,
        to_clock: bool,
    ) -> u32 {
        let id = self.fresh_id();
        self.false_paths.push(FalsePathRow {
            id,
            from: TextEdit::new(from),
            from_clock,
            to: TextEdit::new(to),
            to_clock,
        });
        id
    }

    pub fn add_max_delay(&mut self, from: &str, to: &str, delay: &str, datapath_only: bool) -> u32 {
        let id = self.fresh_id();
        self.max_delays.push(MaxDelayRow {
            id,
            from: TextEdit::new(from),
            to: TextEdit::new(to),
            delay_ns: TextEdit::new(delay),
            datapath_only,
        });
        id
    }

    fn row_count(&self) -> usize {
        self.clocks.len()
            + self.generated.len()
            + self.inputs.len()
            + self.outputs.len()
            + self.false_paths.len()
            + self.max_delays.len()
    }

    /// The write that still matches the current drafts, if any.
    fn current_write(&self, sdc: &str) -> Option<&Written> {
        self.written.as_ref().filter(|w| w.sdc == sdc)
    }
}

// ---- write target + actions ----

/// Where "Write to" writes: the project's first on-disk constraints file;
/// else the sibling `.sdc`/`.xdc` of the open RTL file (the run stages it);
/// else `<project dir>/<project>.xdc`. `None` for an in-memory project.
pub fn target_path(s: &AppState) -> Option<PathBuf> {
    if let Some(p) = s
        .project_files
        .in_set(crate::project::FileSet::Constraints)
        .find_map(|(_, f)| f.path.clone())
    {
        return Some(p);
    }
    let rtl = |name: &String| {
        let n = name.to_ascii_lowercase();
        [".sv", ".v", ".vhd", ".vhdl"]
            .iter()
            .any(|e| n.ends_with(e))
    };
    let tabs = &s.editor.tabs;
    let tab = tabs
        .get(s.editor.active)
        .filter(|t| rtl(t) && s.editor.paths.contains_key(*t))
        .or_else(|| {
            tabs.iter()
                .find(|t| rtl(t) && s.editor.paths.contains_key(*t))
        });
    if let Some(real) = tab.and_then(|t| s.editor.paths.get(t)) {
        for ext in ["sdc", "xdc"] {
            let sib = real.with_extension(ext);
            if sib.is_file() {
                return Some(sib);
            }
        }
        return Some(real.with_extension("xdc"));
    }
    let dir = Path::new(&s.project.location);
    if dir.is_dir() {
        return Some(dir.join(format!("{}.xdc", s.project.name)));
    }
    None
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

fn console(s: &mut AppState, text: String, sev: MsgSev) {
    s.console.lines.push(ConsoleLine { text, sev });
}

/// Append the generated SDC to the target file on disk (creating it and
/// adding it to `constrs_1` when new), refresh an open tab on it, and point
/// re-analysis at it.
pub fn write_to_target(s: &mut AppState) -> Result<PathBuf, String> {
    let ctx = Ctx::from_state(s);
    let w = &s.dialogs.xdc;
    if let Some(first) = issues(w, &ctx).first() {
        return Err(format!("{} problem: {}", first.list.noun(), first.msg));
    }
    if w.row_count() == 0 {
        return Err("Nothing to write: add a clock, delay or exception first.".into());
    }
    let sdc = sdc_text(w, &ctx);
    validate_sdc(&ctx, &sdc).map_err(|e| format!("Generated SDC failed to parse: {e}"))?;
    let path = target_path(s).ok_or("No constraints file on disk for this project.")?;
    crate::views::editor::flush_edit(s);
    let name = file_name(&path);
    let open_tab = s
        .editor
        .tabs
        .iter()
        .position(|t| *t == name && s.editor.paths.get(t) == Some(&path));
    let base = match open_tab {
        Some(i) => crate::views::editor::tab_text(s, i),
        None if path.is_file() => {
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?
        }
        None => String::new(),
    };
    let mut text = base;
    if !text.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
    }
    text.push_str(&sdc);
    std::fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
    let known = s
        .project_files
        .files
        .iter()
        .any(|f| f.path.as_deref() == Some(path.as_path()));
    let mut added = false;
    if !known {
        crate::project::commit_add_sources(
            s,
            std::slice::from_ref(&path),
            crate::project::FileSet::Constraints,
        )?;
        added = true;
    }
    if let Some(i) = open_tab {
        let lines = text.lines().map(str::to_string).collect();
        crate::views::editor::replace_tab_lines(s, i, lines);
    }
    if let Some(ctx) = s.runs.analysis.as_mut() {
        ctx.constraints = Some(path.clone());
    }
    console(s, format!("write_xdc {}", path.display()), MsgSev::Info);
    s.toasts.push(
        ToastKind::Success,
        &format!("Constraints written to {name}"),
        &path.display().to_string(),
    );
    let w = &mut s.dialogs.xdc;
    w.written = Some(Written {
        path: path.clone(),
        sdc,
        added,
    });
    w.timing = None;
    w.error = None;
    Ok(path)
}

/// Why "Write to" is unavailable (`None` = enabled).
fn write_reason(s: &AppState, issue_count: usize, rows: usize) -> Option<String> {
    if issue_count > 0 {
        Some(format!(
            "Fix the {} problem{} first.",
            issue_count,
            if issue_count == 1 { "" } else { "s" }
        ))
    } else if rows == 0 {
        Some("Add a clock, delay or exception first.".into())
    } else if target_path(s).is_none() {
        Some(
            "This project lives in memory: no constraints file on disk. Use Export XDC… or Finish."
                .into(),
        )
    } else {
        None
    }
}

/// What "Re-run timing" does right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rerun {
    /// Re-analyze the last run with the written file.
    Analyze,
    /// No synthesized design: run `synth_1` first, then re-analyze.
    SynthFirst,
    Unavailable(&'static str),
}

/// The Re-run action for the current state (`written`: the reviewed SDC
/// is on disk).
pub fn rerun_action(s: &AppState, written: bool) -> Rerun {
    if !written {
        return Rerun::Unavailable("Write the constraints first: re-analysis reads the file.");
    }
    if s.dialogs.xdc.timing == Some(TimingOutcome::AwaitingRun) {
        return Rerun::Unavailable("Synthesis is running: timing re-runs when it finishes.");
    }
    if s.runs.is_active() {
        return Rerun::Unavailable("A run is in progress");
    }
    if s.runs.analysis.is_none() || s.runs.live_design.is_none() {
        return match crate::runs::resolve_source(s) {
            Ok(_) => Rerun::SynthFirst,
            Err(_) => Rerun::Unavailable("Add a design source: re-analysis needs a synthesized design."),
        };
    }
    if let Some(why) = crate::analysis::timing_unavailable(s).or_else(|| crate::jobs::analysis_busy(s)) {
        return Rerun::Unavailable(why);
    }
    Rerun::Analyze
}

/// Why "Re-run timing" is unavailable (`None` = enabled).
fn rerun_reason(s: &AppState, written: bool) -> Option<&'static str> {
    match rerun_action(s, written) {
        Rerun::Unavailable(why) => Some(why),
        Rerun::Analyze | Rerun::SynthFirst => None,
    }
}

/// The Re-run button: analyze now, or synthesize first and analyze when
/// the run completes.
fn rerun_pressed(s: &mut AppState) {
    match rerun_action(s, s.dialogs.xdc.written.is_some()) {
        Rerun::Analyze => rerun_timing(s),
        Rerun::SynthFirst => synth_then_rerun(s),
        Rerun::Unavailable(_) => {}
    }
}

/// Launch `synth_1` through the run machinery; [`on_run_finished`]
/// re-runs timing once it completes.
fn synth_then_rerun(s: &mut AppState) {
    console(
        s,
        "constraints wizard: no synthesized design; launching synth_1, timing re-runs when it finishes".into(),
        MsgSev::Info,
    );
    crate::runs::launch(s, crate::runs::RunKind::Synth);
    s.dialogs.xdc.timing = Some(if s.runs.is_active() {
        TimingOutcome::AwaitingRun
    } else {
        TimingOutcome::Failed("Synthesis could not start (see the Tcl console).".into())
    });
}

/// Run completion hook ([`crate::runs::deliver`]): finish a pending
/// "synthesize, then re-run timing".
pub fn on_run_finished(s: &mut AppState, ok: bool) {
    if s.dialogs.xdc.timing != Some(TimingOutcome::AwaitingRun) {
        return;
    }
    s.dialogs.xdc.timing = None;
    match (ok, rerun_action(s, s.dialogs.xdc.written.is_some())) {
        (true, Rerun::Analyze) => rerun_timing(s),
        (true, Rerun::Unavailable(why)) => s.dialogs.xdc.timing = Some(TimingOutcome::Failed(why.into())),
        _ => {
            s.dialogs.xdc.timing = Some(TimingOutcome::Failed(
                "Synthesis did not complete: fix the run, then re-run timing.".into(),
            ))
        }
    }
}

/// Re-run STA over the last run with the written file.
fn rerun_timing(s: &mut AppState) {
    let Some(path) = s.dialogs.xdc.written.as_ref().map(|w| w.path.clone()) else {
        return;
    };
    if let Some(ctx) = s.runs.analysis.as_mut() {
        ctx.constraints = Some(path.clone());
    }
    console(
        s,
        format!("report_timing_summary  ;# constraints {}", path.display()),
        MsgSev::Info,
    );
    s.dialogs.xdc.timing = Some(TimingOutcome::Running);
    crate::jobs::submit_wizard_timing(s);
}

/// [`crate::jobs`] completion of a wizard re-run.
pub fn on_timing_done(s: &mut AppState, before: Option<i64>, result: Result<Option<i64>, String>) {
    let outcome = match result {
        Ok(after) => {
            let fmt = |v: Option<i64>| {
                v.map(|ps| format!("{} ns", crate::views::report_kit::ns3(ps)))
                    .unwrap_or_else(|| "n/a".into())
            };
            console(
                s,
                format!("constraints wizard: WNS {} -> {}", fmt(before), fmt(after)),
                MsgSev::Info,
            );
            TimingOutcome::Done { before, after }
        }
        Err(e) => {
            console(
                s,
                format!("ERROR: report_timing_summary: {e}"),
                MsgSev::Error,
            );
            TimingOutcome::Failed(e)
        }
    };
    s.dialogs.xdc.timing = Some(outcome);
}

/// Next: refuse while this step's rows have problems.
fn go_next(s: &mut AppState) {
    let ctx = Ctx::from_state(s);
    let w = &mut s.dialogs.xdc;
    let lists = w.step.lists();
    let n = issues(w, &ctx)
        .iter()
        .filter(|i| lists.contains(&i.list))
        .count();
    if n > 0 {
        w.error = Some(format!(
            "Fix {n} problem{} on this step to continue.",
            if n == 1 { "" } else { "s" }
        ));
        return;
    }
    if let Some(next) = w.step.next() {
        w.step = next;
        w.visited = w.visited.max(next.index());
        w.error = None;
        w.pop = None;
    }
}

fn go_back(s: &mut AppState) {
    let w = &mut s.dialogs.xdc;
    if let Some(p) = w.step.prev() {
        w.step = p;
        w.error = None;
        w.pop = None;
    }
}

/// Finish: open the written file in the editor, or (nothing written yet)
/// append the SDC to the constraints editor buffer, then reset the wizard.
pub fn finish(s: &mut AppState) {
    let ctx = Ctx::from_state(s);
    let (first, rows, sdc) = {
        let w = &s.dialogs.xdc;
        let first = issues(w, &ctx)
            .first()
            .map(|i| format!("{}: {}", i.list.noun(), i.msg));
        (first, w.row_count(), sdc_text(w, &ctx))
    };
    if let Some(msg) = first {
        s.dialogs.xdc.error = Some(msg);
        return;
    }
    if rows == 0 {
        s.dialogs.xdc.error = Some("Add a clock, delay or exception first.".into());
        return;
    }
    let written = s.dialogs.xdc.current_write(&sdc).map(|x| x.path.clone());
    if let Some(path) = written {
        if let Some(i) = s
            .project_files
            .files
            .iter()
            .position(|f| f.path.as_deref() == Some(path.as_path()))
        {
            crate::project::open_index(s, i);
        }
    } else {
        let tab = match s
            .editor
            .tabs
            .iter()
            .position(|t| crate::views::editor::is_constraint_name(t))
        {
            Some(i) => i,
            None => {
                s.editor.tabs.push(format!("{}.xdc", s.project.name));
                s.editor.tabs.len() - 1
            }
        };
        crate::views::editor::select_tab(s, tab);
        crate::views::editor::flush_edit(s);
        s.editor.push_undo_xdc();
        if !s.editor.xdc_lines.is_empty() {
            s.editor.xdc_lines.push(String::new());
        }
        s.editor.xdc_lines.extend(sdc.lines().map(str::to_string));
        s.editor.dirty = true;
        // Re-mirror the live field from the grown buffer.
        s.editor.edit_tab = None;
        crate::views::editor::sync_edit(s);
        let at = s.editor.xdc_lines.len().saturating_sub(1);
        crate::views::editor::move_cursor_to(s, at);
        for line in sdc.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
            console(s, line.to_string(), MsgSev::Info);
        }
    }
    s.navigate(crate::state::CenterPane::Editor);
    s.dialogs.xdc = XdcWizard::default();
    s.dialogs.close();
}

// ---- row defaults (add buttons) ----

fn first_free(candidates: Vec<String>, taken: &[String]) -> String {
    candidates
        .iter()
        .find(|c| !taken.contains(c))
        .or(candidates.first())
        .cloned()
        .unwrap_or_default()
}

fn add_clock_row(s: &mut AppState) {
    let ctx = Ctx::from_state(s);
    let w = &mut s.dialogs.xdc;
    let mut taken: Vec<String> = ctx.known.iter().map(|k| k.source.clone()).collect();
    taken.extend(w.clocks.iter().map(|c| c.port.text.trim().to_string()));
    let mut ins = ctx.inputs();
    ins.sort_by_key(|p| !p.to_ascii_lowercase().contains("clk"));
    let port = first_free(ins, &taken);
    let names = clock_names(w, &ctx);
    let mut name = if port.is_empty() {
        "clk".to_string()
    } else {
        port.clone()
    };
    let mut n = 1;
    while names.contains(&name) {
        name = format!(
            "{}_{n}",
            if port.is_empty() {
                "clk"
            } else {
                port.as_str()
            }
        );
        n += 1;
    }
    w.add_clock(&name, &port, "10.000", "50");
    w.error = None;
}

fn add_gen_row(s: &mut AppState) {
    let ctx = Ctx::from_state(s);
    let w = &mut s.dialogs.xdc;
    let names = clock_names(w, &ctx);
    let master = names.first().cloned().unwrap_or_default();
    let mut n = w.generated.len() + 1;
    let mut name = format!("clk_div{n}");
    while names.contains(&name) {
        n += 1;
        name = format!("clk_div{n}");
    }
    w.add_generated(&name, &master, "", "2");
    w.error = None;
}

fn add_delay_row(s: &mut AppState, output: bool) {
    let ctx = Ctx::from_state(s);
    let w = &mut s.dialogs.xdc;
    let mut taken: Vec<String> = ctx.known.iter().map(|k| k.source.clone()).collect();
    taken.extend(w.clocks.iter().map(|c| c.port.text.trim().to_string()));
    let rows = if output { &w.outputs } else { &w.inputs };
    taken.extend(rows.iter().map(|r| r.port.text.trim().to_string()));
    let port = first_free(if output { ctx.outputs() } else { ctx.inputs() }, &taken);
    let clock = clock_names(w, &ctx).first().cloned().unwrap_or_default();
    w.add_delay(output, &port, &clock, "", "2.000");
    w.error = None;
}

fn add_fp_row(s: &mut AppState) {
    let ctx = Ctx::from_state(s);
    let from = ctx.inputs().first().cloned().unwrap_or_default();
    let to = ctx.outputs().first().cloned().unwrap_or_default();
    s.dialogs.xdc.add_false_path(&from, false, &to, false);
    s.dialogs.xdc.error = None;
}

fn add_md_row(s: &mut AppState) {
    let ctx = Ctx::from_state(s);
    let from = ctx.inputs().first().cloned().unwrap_or_default();
    let to = ctx.outputs().first().cloned().unwrap_or_default();
    s.dialogs.xdc.add_max_delay(&from, &to, "8.000", true);
    s.dialogs.xdc.error = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::dialogs::DialogId;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn design_state() -> AppState {
        let mut s = init();
        crate::scenes::xdc_wizard_design(&mut s);
        s
    }

    fn rt_with(f: impl Fn(&mut AppState)) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        let mut s = design_state();
        s.dialogs.open(DialogId::Constraints);
        f(&mut s);
        let mut rt = Runtime::new(s, crate::view, spec.size, env);
        rt.frame(1.0 / 60.0);
        rt
    }

    thread_local! {
        static CLOCK: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
    }

    /// Advance the virtual clock so entrance animations settle.
    fn settle(rt: &mut Runtime<AppState>) {
        for _ in 0..40 {
            let t = CLOCK.with(|c| {
                c.set(c.get() + 0.05);
                c.get()
            });
            rt.frame(t);
        }
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        settle(rt);
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(rt);
    }

    #[test]
    fn rows_add_and_remove_and_validate_against_real_ports() {
        let mut s = design_state();
        let ctx = Ctx::from_state(&s);
        assert!(
            ctx.ports.as_ref().is_some_and(|p| !p.is_empty()),
            "design ports"
        );
        add_clock_row(&mut s);
        add_delay_row(&mut s, false);
        add_delay_row(&mut s, true);
        let w = &s.dialogs.xdc;
        assert_eq!((w.clocks.len(), w.inputs.len(), w.outputs.len()), (1, 1, 1));
        let ctx = Ctx::from_state(&s);
        assert_eq!(issues(&s.dialogs.xdc, &ctx), vec![], "defaults are valid");
        // A made-up port and an output used as a clock both fail per row.
        let id = s.dialogs.xdc.clocks[0].id;
        *s.dialogs.xdc.edit_mut(Field::ClkPort, id) = TextEdit::new("no_such_port");
        let out_id = s.dialogs.xdc.outputs[0].id;
        let in_port = s.dialogs.xdc.inputs[0].port.text.clone();
        *s.dialogs.xdc.edit_mut(Field::OutPort, out_id) = TextEdit::new(in_port);
        let probs = issues(&s.dialogs.xdc, &ctx);
        assert!(probs.iter().any(|i| i.list == List::Clock
            && i.id == id
            && i.msg.contains("not a top-level port")));
        assert!(
            probs
                .iter()
                .any(|i| i.list == List::Out && i.msg.contains("not an output"))
        );
        s.dialogs.xdc.remove(List::Clock, id);
        assert!(s.dialogs.xdc.clocks.is_empty());
        // Editing a removed row's field never panics.
        *s.dialogs.xdc.edit_mut(Field::ClkName, id) = TextEdit::new("x");
    }

    #[test]
    fn generated_clocks_max_delay_and_false_paths_round_trip_through_the_parser() {
        let mut s = design_state();
        crate::scenes::xdc_wizard_rows(&mut s);
        let ctx = Ctx::from_state(&s);
        assert_eq!(issues(&s.dialogs.xdc, &ctx), vec![]);
        let sdc = sdc_text(&s.dialogs.xdc, &ctx);
        assert!(
            sdc.contains("create_generated_clock -name clk_div2 -master_clock clk_sys"),
            "{sdc}"
        );
        assert!(sdc.contains(" -datapath_only"), "{sdc}");
        assert!(sdc.contains("set_false_path -from [get_ports"), "{sdc}");
        let c = helion_sta::load_xdc(&sdc).expect("generated SDC loads");
        let g = c
            .clocks
            .iter()
            .find(|k| k.name == "clk_div2")
            .expect("generated clock");
        assert!(g.generated && g.divide_by == 2);
        assert_eq!(g.master.as_deref(), Some("clk_sys"));
        assert_eq!(c.max_delays.len(), 1);
        assert!(c.max_delays[0].datapath_only);
        assert_eq!(c.false_paths.len(), 1);
        // A generated clock whose master is missing is an error on its row.
        let gid = s.dialogs.xdc.generated[0].id;
        *s.dialogs.xdc.edit_mut(Field::GenMaster, gid) = TextEdit::new("nope");
        let probs = issues(&s.dialogs.xdc, &ctx);
        assert!(probs.iter().any(|i| i.list == List::Gen && i.id == gid));
    }

    #[test]
    fn next_refuses_a_step_with_problems() {
        let mut rt = rt_with(|s| {
            s.dialogs.xdc.add_clock("clk_sys", "clk", "nope", "50");
        });
        tap(&mut rt, "xdc-next");
        assert_eq!(rt.state.dialogs.xdc.step, XdcStep::Clocks);
        assert!(rt.find("xdc-error").is_some());
        let id = rt.state.dialogs.xdc.clocks[0].id;
        assert!(
            rt.find(&format!("xdc-clk-err-{id}-0")).is_some(),
            "row error renders"
        );
        *rt.state.dialogs.xdc.edit_mut(Field::ClkPeriod, id) = TextEdit::new("8.0");
        rt.invalidate();
        tap(&mut rt, "xdc-next");
        assert_eq!(rt.state.dialogs.xdc.step, XdcStep::InputDelays);
    }

    #[test]
    fn add_buttons_and_port_picker_fill_rows_from_the_design() {
        let mut rt = rt_with(|_| {});
        tap(&mut rt, "xdc-clk-add");
        assert_eq!(rt.state.dialogs.xdc.clocks.len(), 1);
        let id = rt.state.dialogs.xdc.clocks[0].id;
        tap(&mut rt, &format!("xdc-clkport-{id}-pick"));
        assert!(rt.state.dialogs.xdc.pop.is_some(), "picker opens");
        tap(&mut rt, &format!("xdc-clkport-{id}-pick-opt-0"));
        let ctx = Ctx::from_state(&rt.state);
        let first = ctx.inputs()[0].clone();
        assert_eq!(rt.state.dialogs.xdc.clocks[0].port.text, first);
        tap(&mut rt, &format!("xdc-clk-del-{id}"));
        assert!(rt.state.dialogs.xdc.clocks.is_empty());
    }

    #[test]
    fn write_appends_to_disk_adds_the_file_and_rerun_reports_wns() {
        let dir = std::env::temp_dir().join(format!("helion-xdc-wiz-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = design_state();
        crate::scenes::xdc_wizard_rows(&mut s);
        // A disk project: write lands in <location>/<name>.xdc and joins constrs_1.
        s.project.location = dir.display().to_string();
        s.project.name = "wiz".into();
        s.project_files = crate::project::ProjectFiles::default();
        let before = s.project_files.files.len();
        let path = write_to_target(&mut s).expect("write");
        assert_eq!(path, dir.join("wiz.xdc"));
        let disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            disk.contains("create_clock -period 8.000 -name clk_sys"),
            "{disk}"
        );
        helion_sta::load_xdc(&disk).expect("written file loads");
        assert_eq!(
            s.project_files.files.len(),
            before + 1,
            "new file joins constrs_1"
        );
        assert!(s.dialogs.xdc.written.as_ref().is_some_and(|w| w.added));
        // Second write appends rather than replacing.
        write_to_target(&mut s).expect("write again");
        let again = std::fs::read_to_string(&path).unwrap();
        assert_eq!(again.matches("# Helion Constraints Wizard").count(), 2);
        assert_eq!(s.project_files.files.len(), before + 1, "no duplicate file");

        // Re-run timing over a real synthesized run with that file.
        let mut s2 = crate::jobs::test_support::analyzed_state();
        s2.editor.xdc_lines.clear();
        crate::scenes::xdc_wizard_rows_for(&mut s2, "clk", "clk", 4.0);
        s2.project.location = dir.display().to_string();
        s2.project.name = "wiz2".into();
        s2.project_files = crate::project::ProjectFiles::default();
        crate::analysis::rerun_timing(&mut s2).unwrap();
        let before = s2.timing.wns_ps;
        write_to_target(&mut s2).expect("write");
        assert!(rerun_reason(&s2, true).is_none());
        rerun_timing(&mut s2);
        assert_eq!(s2.dialogs.xdc.timing, Some(TimingOutcome::Running));
        assert!(
            crate::jobs::analysis_busy(&s2).is_some(),
            "button busy while it runs"
        );
        crate::jobs::wait_idle(&mut s2);
        match &s2.dialogs.xdc.timing {
            Some(TimingOutcome::Done { before: b, after }) => {
                assert_eq!(*b, before);
                assert_eq!(*after, s2.timing.wns_ps, "timing state refreshed");
            }
            other => panic!("{other:?}"),
        }
        assert!(
            s2.timing
                .clocks
                .iter()
                .any(|c| c.name == "clk" && c.period_ps == 4000),
            "re-analysis used the written 4 ns clock: {:?}",
            s2.timing.clocks
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rerun_without_a_run_synthesizes_first_then_reports_wns() {
        let dir = std::env::temp_dir().join(format!("helion-xdc-wiz-synth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = crate::jobs::test_support::analyzed_state();
        s.editor.xdc_lines.clear();
        crate::scenes::xdc_wizard_rows_for(&mut s, "clk", "clk", 5.0);
        s.project.location = dir.display().to_string();
        s.project.name = "wiz3".into();
        s.project_files = crate::project::ProjectFiles::default();
        write_to_target(&mut s).expect("write");
        // No prior run, but a source the run machinery can synthesize.
        s.runs.source = s.runs.analysis.take().map(|a| a.source);
        s.runs.live_design = None;
        assert_eq!(rerun_action(&s, true), Rerun::SynthFirst);
        s.runs.source = None;
        s.editor.tabs.clear();
        assert!(matches!(rerun_action(&s, true), Rerun::Unavailable(r) if r.contains("design source")));
        let src = crate::jobs::test_support::analyzed_state().runs.analysis.unwrap().source;
        s.runs.source = Some(src);
        rerun_pressed(&mut s);
        assert_eq!(s.dialogs.xdc.timing, Some(TimingOutcome::AwaitingRun));
        assert!(s.runs.is_active(), "synth_1 started through the run machinery");
        assert!(rerun_reason(&s, true).unwrap().contains("Synthesis is running"));
        let start = std::time::Instant::now();
        while s.runs.is_active() {
            crate::runs::poll_runs(&mut s);
            assert!(start.elapsed() < std::time::Duration::from_secs(180), "synth timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(s.dialogs.xdc.timing, Some(TimingOutcome::Running), "timing re-runs on completion");
        crate::jobs::wait_idle(&mut s);
        match &s.dialogs.xdc.timing {
            Some(TimingOutcome::Done { after, .. }) => assert_eq!(*after, s.timing.wns_ps),
            other => panic!("{other:?}"),
        }
        assert!(
            s.timing.clocks.iter().any(|c| c.name == "clk" && c.period_ps == 5000),
            "re-analysis used the written 5 ns clock: {:?}",
            s.timing.clocks
        );
        // A failed or cancelled run ends the wait with a reason.
        s.dialogs.xdc.timing = Some(TimingOutcome::AwaitingRun);
        on_run_finished(&mut s, false);
        assert!(matches!(&s.dialogs.xdc.timing, Some(TimingOutcome::Failed(e)) if e.contains("did not complete")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn in_memory_project_disables_write_with_a_reason_and_finish_fills_the_buffer() {
        let mut rt = rt_with(|s| {
            crate::scenes::xdc_wizard_rows(s);
            s.dialogs.xdc.step = XdcStep::Review;
            s.dialogs.xdc.visited = 4;
        });
        assert!(rt.find("xdc-sdc").is_some());
        assert!(rt.find("xdc-sdc-0").is_some());
        let write = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("xdc-write"))
            .expect("write button");
        assert!(!write.focusable, "in-memory project has no file to write");
        assert!(
            write.help.is_some_and(|h| h.contains("in memory")),
            "the reason shows"
        );
        let before = rt.state.editor.xdc_lines.len();
        tap(&mut rt, "xdc-finish");
        assert_eq!(rt.state.dialogs.open, None);
        let after = &rt.state.editor.xdc_lines;
        assert!(after.len() > before);
        assert!(
            after
                .iter()
                .any(|l| l.starts_with("create_clock -period 8.000 -name clk_sys"))
        );
        assert!(after.iter().any(|l| l.starts_with("set_false_path ")));
        assert_eq!(rt.state.center, crate::state::CenterPane::Editor);
        assert!(rt.state.editor.dirty);
    }

    #[test]
    fn export_writes_the_generated_sdc_to_the_chosen_path() {
        use rune::FileDialogKind;
        let mut rt = rt_with(|s| {
            crate::scenes::xdc_wizard_rows(s);
            s.dialogs.xdc.step = XdcStep::Review;
        });
        tap(&mut rt, "xdc-export");
        let (kind, _) = rt.file_dialog().expect("Export must raise a save dialog");
        assert_eq!(kind, FileDialogKind::Save);
        let path = std::env::temp_dir().join(format!("helion-pick-{}-wiz.xdc", std::process::id()));
        rt.resolve_file_dialog(Some(path.clone())).expect("answer");
        rt.frame(1.0 / 60.0);
        let disk = std::fs::read_to_string(&path).expect("export writes");
        assert!(disk.contains("create_clock"));
        let _ = std::fs::remove_file(&path);
    }
}
