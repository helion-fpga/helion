//! Off-UI-thread engine jobs: report re-analysis (STA, methodology, DRC,
//! power, utilization) and the Tcl `program_hw` verb.
//!
//! These used to run inside the click or console handler and froze the
//! window for the length of an STA or a board program. A job now clones its
//! inputs on the UI thread ([`crate::analysis::RunInputs`],
//! [`crate::analysis::DrcInputs`], or a copy of the Tcl session), runs the
//! very same `compute_*` / [`crate::tcl::program_hw_session`] body on a
//! worker thread, and [`poll`] folds the result back with the same
//! `apply_*` step the synchronous path uses, so results are identical.
//!
//! One job runs at a time; later requests queue and gather their inputs
//! when they start, so a queue of verbs behaves like typing them in order.
//! Scripted Tcl (`source`, procs, nested `[...]`) keeps the synchronous
//! path: only a plain one-command line from the console, palette, menus or
//! a button is deferred ([`submit_tcl`]).
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/tooling.rs`: `AppSpec::poll` drains the channel.
//! - `crates/rune/src/el/tx.rs`: `Tx::wake_after` keeps polling while busy.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc};

use crate::analysis::{self as an, DrcInputs, RunInputs};
use crate::state::{AppState, ConsoleLine, DrcState, MsgSev, PowerState, TimingState, UtilState};

/// Which engine a job runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Timing,
    Methodology,
    Power,
    Drc,
    Util,
    ProgramHw,
}

impl JobKind {
    /// Busy text for buttons and the status line.
    pub fn busy_label(self) -> &'static str {
        match self {
            JobKind::Timing => "Timing analysis is running",
            JobKind::Methodology => "Methodology checks are running",
            JobKind::Power => "Power analysis is running",
            JobKind::Drc => "DRC is running",
            JobKind::Util => "Utilization is being counted",
            JobKind::ProgramHw => "program_hw is programming the device",
        }
    }

    fn is_analysis(self) -> bool {
        !matches!(self, JobKind::ProgramHw)
    }
}

/// Who asked, which decides what completion prints and updates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobOrigin {
    /// A one-command Tcl line: completion prints the verb's value or error
    /// exactly as `eval_command` would have.
    Tcl { verb: String, argv: Vec<String> },
    /// Reports > Utilization "Report Utilization" button.
    UtilButton,
    /// Constraints Wizard "Re-run timing": WNS before is what the timing
    /// state held when the job was requested.
    Wizard { wns_before: Option<i64> },
}

#[derive(Clone, Debug)]
struct Request {
    kind: JobKind,
    origin: JobOrigin,
}

enum Output {
    Timing(Result<TimingState, String>),
    Power(Result<PowerState, String>),
    Drc(Result<DrcState, String>),
    Util(Result<UtilState, String>),
    Program {
        session: Box<helion_proj::Session>,
        result: Result<Result<String, String>, String>,
    },
}

struct Done {
    lines: Vec<ConsoleLine>,
    out: Output,
}

#[derive(Clone)]
struct Active {
    req: Request,
    rx: Arc<Mutex<mpsc::Receiver<Done>>>,
}

/// Job slice of [`AppState`]: the running job plus the queue behind it.
#[derive(Clone, Default)]
pub struct JobsState {
    active: Option<Active>,
    queue: VecDeque<Request>,
}

impl std::fmt::Debug for JobsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobsState")
            .field("active", &self.active.as_ref().map(|a| a.req.kind))
            .field("queued", &self.queue.len())
            .finish()
    }
}

impl JobsState {
    /// The running job's kind.
    pub fn active(&self) -> Option<JobKind> {
        self.active.as_ref().map(|a| a.req.kind)
    }

    /// True while `kind` runs or waits in the queue.
    pub fn pending(&self, kind: JobKind) -> bool {
        self.active() == Some(kind) || self.queue.iter().any(|r| r.kind == kind)
    }

    pub fn is_idle(&self) -> bool {
        self.active.is_none() && self.queue.is_empty()
    }

    /// True while a wizard-requested re-analysis runs or waits.
    pub fn wizard_pending(&self) -> bool {
        self.active
            .iter()
            .map(|a| &a.req)
            .chain(self.queue.iter())
            .any(|r| matches!(r.origin, JobOrigin::Wizard { .. }))
    }
}

/// Why a button that re-runs an analysis engine is busy (`None` = idle).
pub fn analysis_busy(s: &AppState) -> Option<&'static str> {
    s.jobs
        .active
        .iter()
        .map(|a| a.req.kind)
        .chain(s.jobs.queue.iter().map(|r| r.kind))
        .find(|k| k.is_analysis())
        .map(JobKind::busy_label)
}

/// Why programming is busy (`None` = no `program_hw` job pending).
pub fn program_busy(s: &AppState) -> Option<&'static str> {
    s.jobs
        .pending(JobKind::ProgramHw)
        .then_some(JobKind::ProgramHw.busy_label())
}

/// Characters that make a line real Tcl (substitution, quoting, several
/// commands): such lines keep the synchronous interpreter.
fn is_plain(verb: &str) -> bool {
    !verb.contains(['[', ']', '{', '}', '$', ';', '"', '\\', '\n'])
}

/// Defer a one-command console/palette/button line when it re-runs an
/// engine. Returns false (the caller evaluates it synchronously) for any
/// other line, including report verbs whose inputs are missing, which only
/// print the stored state.
pub fn submit_tcl(s: &mut AppState, verb: &str) -> bool {
    if !is_plain(verb) {
        return false;
    }
    let argv: Vec<String> = verb.split_whitespace().map(str::to_string).collect();
    let Some(name) = argv.first().map(String::as_str) else {
        return false;
    };
    let kind = match name {
        "report_timing_summary" if argv.len() == 1 => JobKind::Timing,
        "report_methodology" if argv.len() == 1 => JobKind::Methodology,
        "report_power" if argv.len() == 1 => JobKind::Power,
        "report_drc" if argv.len() == 1 => JobKind::Drc,
        "program_hw" | "program_hw_devices" => JobKind::ProgramHw,
        _ => return false,
    };
    let ready = match kind {
        JobKind::Drc => an::drc_unavailable(s).is_none(),
        JobKind::ProgramHw => true,
        _ => an::timing_unavailable(s).is_none(),
    };
    if !ready {
        return false;
    }
    if kind.is_analysis() {
        crate::tcl::report_navigate(s, name);
    }
    let origin = JobOrigin::Tcl {
        verb: name.to_string(),
        argv,
    };
    submit(s, kind, origin);
    true
}

/// Report Utilization button: recount on a worker, then log the counts.
pub fn submit_util(s: &mut AppState) {
    submit(s, JobKind::Util, JobOrigin::UtilButton);
}

/// Constraints Wizard re-run: STA over the last run with the constraints
/// the wizard wrote ([`crate::runs::AnalysisCtx::constraints`]).
pub fn submit_wizard_timing(s: &mut AppState) {
    let wns_before = s.timing.has_run.then_some(s.timing.wns_ps).flatten();
    submit(s, JobKind::Timing, JobOrigin::Wizard { wns_before });
}

fn submit(s: &mut AppState, kind: JobKind, origin: JobOrigin) {
    s.jobs.queue.push_back(Request { kind, origin });
    start_next(s);
}

/// Inputs gathered at start time, or why the job cannot start.
enum Input {
    Run(RunInputs),
    Drc(DrcInputs),
    Program {
        session: Box<helion_proj::Session>,
        design: Option<helion_ir::Design>,
        part: String,
        cable: String,
    },
}

fn gather(s: &AppState, req: &Request) -> Result<Input, String> {
    match req.kind {
        JobKind::Drc => an::drc_inputs(s).map(Input::Drc),
        JobKind::ProgramHw => {
            let argv = match &req.origin {
                JobOrigin::Tcl { argv, .. } => argv.clone(),
                _ => Vec::new(),
            };
            Ok(Input::Program {
                session: Box::new(s.tcl.session.clone()),
                design: s.tcl.design.clone(),
                part: s.project.part.clone(),
                cable: crate::tcl::program_cable(&argv).to_string(),
            })
        }
        _ => {
            if s.runs.is_active() {
                return Err("a run is in progress".into());
            }
            an::run_inputs(s).map(Input::Run)
        }
    }
}

fn work(kind: JobKind, input: Input) -> Done {
    let lines = std::cell::RefCell::new(Vec::new());
    let emit = |text: String, sev: MsgSev| lines.borrow_mut().push(ConsoleLine { text, sev });
    let out = match (kind, input) {
        (JobKind::Timing | JobKind::Methodology, Input::Run(inp)) => {
            Output::Timing(an::compute_timing(&inp, &emit))
        }
        (JobKind::Power, Input::Run(inp)) => Output::Power(an::compute_power(&inp, &emit)),
        (JobKind::Util, Input::Run(inp)) => Output::Util(an::compute_util(&inp, &emit)),
        (JobKind::Drc, Input::Drc(inp)) => Output::Drc(an::compute_drc(&inp, &emit)),
        (
            _,
            Input::Program {
                mut session,
                design,
                part,
                cable,
            },
        ) => {
            let result =
                crate::tcl::program_hw_session(&mut session, design.as_ref(), &part, &cable);
            Output::Program { session, result }
        }
        (_, _) => Output::Timing(Err("job inputs do not match the job".into())),
    };
    Done {
        lines: lines.into_inner(),
        out,
    }
}

/// Start the next queued job when idle. Jobs whose inputs vanished since
/// they were queued complete at once with the honest reason.
fn start_next(s: &mut AppState) {
    while s.jobs.active.is_none() {
        let Some(req) = s.jobs.queue.pop_front() else {
            return;
        };
        let input = match gather(s, &req) {
            Ok(i) => i,
            Err(e) => {
                complete_unavailable(s, &req, e);
                continue;
            }
        };
        let (tx, rx) = mpsc::channel();
        let kind = req.kind;
        let spawned = std::thread::Builder::new()
            .name(format!("helion-job-{kind:?}").to_lowercase())
            .spawn(move || {
                let _ = tx.send(work(kind, input));
            });
        match spawned {
            Ok(_) => {
                s.jobs.active = Some(Active {
                    req,
                    rx: Arc::new(Mutex::new(rx)),
                });
            }
            Err(e) => complete_unavailable(s, &req, format!("job worker: {e}")),
        }
    }
}

/// A queued job that could not start: report verbs print the stored state
/// (as the synchronous verb does when it cannot re-run); others fail.
fn complete_unavailable(s: &mut AppState, req: &Request, why: String) {
    match &req.origin {
        JobOrigin::Tcl { verb, .. } if req.kind.is_analysis() => {
            let v = crate::tcl::report_value(s, verb);
            crate::tcl::push_result(s, v);
        }
        JobOrigin::Tcl { verb, .. } => {
            crate::tcl::push_result(s, Err(format!("{verb}: {why}")));
        }
        JobOrigin::UtilButton => s.console.lines.push(ConsoleLine {
            text: format!("ERROR: report_utilization: {why}"),
            sev: MsgSev::Error,
        }),
        JobOrigin::Wizard { wns_before } => {
            crate::views::xdc_wizard::on_timing_done(s, *wns_before, Err(why));
        }
    }
}

/// Drain the active job without a frame context. Returns true when a job
/// finished (state changed).
pub fn pump(s: &mut AppState) -> bool {
    let Some(active) = s.jobs.active.clone() else {
        return false;
    };
    let next = active
        .rx
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .try_recv();
    let done = match next {
        Ok(done) => done,
        Err(mpsc::TryRecvError::Empty) => return false,
        Err(mpsc::TryRecvError::Disconnected) => Done {
            lines: Vec::new(),
            out: Output::Timing(Err("job worker exited unexpectedly".into())),
        },
    };
    s.jobs.active = None;
    s.console.lines.extend(done.lines);
    finish(s, &active.req, done.out);
    start_next(s);
    true
}

/// `AppSpec::poll` wiring: fold finished jobs in, and keep frames coming
/// while one runs so its completion lands without input.
pub fn poll(s: &mut AppState, tx: &mut rune::Tx) -> bool {
    let changed = pump(s);
    if s.jobs.active.is_some() {
        tx.wake_after(std::time::Duration::from_millis(40));
    }
    changed
}

/// Block until every queued job finished (headless callers and tests).
pub fn wait_idle(s: &mut AppState) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    while !s.jobs.is_idle() && std::time::Instant::now() < deadline {
        if !pump(s) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

fn finish(s: &mut AppState, req: &Request, out: Output) {
    match (out, &req.origin) {
        (Output::Program { session, result }, origin) => {
            // Fold back only what `program_hw` touches, so console verbs
            // typed while it ran keep their own session edits.
            let t = &mut s.tcl.session;
            t.design = session.design;
            t.packed = session.packed;
            t.placed = session.placed;
            t.routed = session.routed;
            t.bitstream = session.bitstream;
            t.impl_checkpoint = session.impl_checkpoint;
            t.hw_open = session.hw_open;
            t.programmed = session.programmed;
            let value = match result {
                Err(e) => Err(e),
                Ok(r) => {
                    crate::views::program::note_tcl_program(s, &r);
                    r
                }
            };
            if let JobOrigin::Tcl { .. } = origin {
                crate::tcl::push_result(s, value);
            }
        }
        (Output::Timing(r), JobOrigin::Wizard { wns_before }) => {
            if let Ok(fresh) = &r {
                an::apply_timing(s, fresh.clone());
            }
            crate::views::xdc_wizard::on_timing_done(s, *wns_before, r.map(|t| t.wns_ps));
        }
        (out, JobOrigin::Tcl { verb, .. }) => {
            let applied = match out {
                Output::Timing(r) if req.kind == JobKind::Methodology => {
                    r.map(|t| an::apply_methodology(s, t))
                }
                Output::Timing(r) => r.map(|t| an::apply_timing(s, t)),
                Output::Power(r) => r.map(|p| s.power = p),
                Output::Drc(r) => r.map(|d| an::apply_drc(s, d)),
                Output::Util(r) => r.map(|u| s.util = u),
                Output::Program { .. } => Ok(()),
            };
            let value = match applied {
                Err(e) => Err(format!("{verb}: {e}")),
                Ok(()) => crate::tcl::report_value(s, verb),
            };
            crate::tcl::push_result(s, value);
        }
        (Output::Util(r), JobOrigin::UtilButton) => match r {
            Ok(u) => {
                s.util = u;
                let line = util_line(&s.util);
                s.console.lines.push(ConsoleLine {
                    text: line,
                    sev: MsgSev::Info,
                });
            }
            Err(e) => s.console.lines.push(ConsoleLine {
                text: format!("ERROR: report_utilization: {e}"),
                sev: MsgSev::Error,
            }),
        },
        (_, _) => {}
    }
}

/// The console line the Report Utilization button logs.
pub fn util_line(u: &UtilState) -> String {
    format!(
        "report_utilization refreshed: LUT {}/{} FF {}/{} IOB {}/{} BRAM {}/{} DSP {}/{}",
        u.luts_used,
        u.luts_avail,
        u.regs_used,
        u.regs_avail,
        u.io_used,
        u.io_avail,
        u.bram_used,
        u.bram_avail,
        u.dsp_used,
        u.dsp_avail
    )
}

/// Fixtures shared by job and wizard tests.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::runs::{AnalysisCtx, RunKind};
    use crate::state::AppState;

    /// A synthesized counter with a re-analysis context (real STA input).
    pub fn analyzed_state() -> AppState {
        let mut s = AppState::demo();
        let src = "module c(input logic clk, input logic en, output logic led);\n\
            logic [3:0] q;\n\
            always_ff @(posedge clk) if (en) q <= q + 1;\n\
            assign led = q[3];\nendmodule\n";
        // One file per call: tests run in parallel and must not share it.
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("helion-rune-jobs-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.sv");
        std::fs::write(&path, src).unwrap();
        s.runs.live_design = Some(helion_sv::synth_sv_path(&path).expect("synth"));
        s.runs.analysis = Some(AnalysisCtx {
            kind: RunKind::Synth,
            source: path,
            part: s.project.part.clone(),
            routed: None,
            constraints: None,
        });
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::test_support::analyzed_state;

    fn console_after(s: &AppState, from: usize) -> Vec<String> {
        s.console.lines[from..]
            .iter()
            .map(|l| l.text.clone())
            .collect()
    }

    #[test]
    fn deferred_report_verbs_print_what_the_sync_path_prints() {
        for verb in [
            "report_timing_summary",
            "report_power",
            "report_methodology",
        ] {
            // Synchronous reference: the scripted interpreter path.
            let mut sync = analyzed_state();
            let mut out = Vec::new();
            let mut vals = Vec::new();
            let at = sync.console.lines.len();
            let _ = crate::tcl::eval_script_for_test(&mut sync, verb, &mut out, &mut vals);
            let sync_lines = console_after(&sync, at);
            let sync_value = vals.last().cloned().unwrap_or_default();

            let mut s = analyzed_state();
            let at = s.console.lines.len();
            crate::tcl::eval_command(&mut s, verb);
            assert!(s.jobs.active().is_some(), "{verb} must run on a worker");
            assert_eq!(
                analysis_busy(&s),
                Some(s.jobs.active().unwrap().busy_label())
            );
            wait_idle(&mut s);
            let lines = console_after(&s, at);
            assert_eq!(lines[0], verb, "the verb echoes first");
            let mut want = sync_lines.clone();
            want.extend(sync_value.split('\n').map(str::to_string));
            assert_eq!(
                lines[1..].to_vec(),
                want,
                "{verb}: same lines as the sync path"
            );
            assert_eq!(s.timing.wns_ps, sync.timing.wns_ps);
            assert_eq!(s.power.total_uw, sync.power.total_uw);
            assert!(s.jobs.is_idle());
        }
    }

    #[test]
    fn scripted_and_unready_lines_stay_synchronous() {
        let mut s = AppState::demo();
        crate::tcl::eval_command(&mut s, "report_timing_summary");
        assert!(
            s.jobs.is_idle(),
            "no run: nothing to re-analyze, prints at once"
        );
        assert!(
            s.console
                .lines
                .last()
                .unwrap()
                .text
                .starts_with("ERROR: report_timing_summary"),
            "{:?}",
            s.console.lines.last()
        );
        let mut s = analyzed_state();
        crate::tcl::eval_command(&mut s, "set x [report_timing_summary]");
        assert!(s.jobs.is_idle(), "substitution keeps the interpreter path");
        assert!(s.timing.has_run);
    }

    #[test]
    fn queued_jobs_run_in_order() {
        let mut s = analyzed_state();
        crate::tcl::eval_command(&mut s, "report_timing_summary");
        submit_util(&mut s);
        assert!(s.jobs.pending(JobKind::Util));
        wait_idle(&mut s);
        let texts: Vec<&str> = s.console.lines.iter().map(|l| l.text.as_str()).collect();
        let sum = texts
            .iter()
            .position(|t| t.starts_with("Design Timing Summary:"))
            .expect("summary");
        let util = texts
            .iter()
            .position(|t| t.starts_with("report_utilization refreshed"))
            .expect("util line");
        assert!(sum < util, "jobs complete in submission order");
        assert!(an::util_ready(&s));
    }

    #[test]
    fn program_hw_runs_on_a_worker_and_mirrors_the_pane() {
        let mut sync = AppState::demo();
        sync.project.part = "HL10T-C32-1".into();
        let src =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv");
        crate::tcl::eval_command(&mut sync, &format!("read_sv {}", src.display()));
        assert!(sync.tcl.design.is_some(), "read_sv elaborates");
        let mut s = sync.clone();
        let mut out = Vec::new();
        let mut vals = Vec::new();
        let _ = crate::tcl::eval_script_for_test(
            &mut sync,
            "program_hw cable=sim",
            &mut out,
            &mut vals,
        );
        let at = s.console.lines.len();
        crate::tcl::eval_command(&mut s, "program_hw cable=sim");
        assert_eq!(program_busy(&s), Some(JobKind::ProgramHw.busy_label()));
        wait_idle(&mut s);
        let lines = console_after(&s, at);
        assert_eq!(lines[0], "program_hw cable=sim");
        assert_eq!(lines[1], vals.last().cloned().unwrap_or_default());
        assert!(s.tcl.session.programmed, "the session result folds back");
        assert_eq!(
            s.program.result.as_ref().map(|r| r.ok),
            sync.program.result.as_ref().map(|r| r.ok)
        );
    }
}
