//! Event-driven interpreter.
//!
//! Every SystemVerilog process (`initial`, `always`, each `fork` branch) is
//! a [`Proc`] with an explicit frame stack, so any statement can suspend
//! (`#delay`, `@event`, `wait`, mailbox/semaphore waits, `join`) without
//! host threads. Expressions never suspend (functions cannot consume time),
//! so they are evaluated recursively; function calls inside expressions run
//! on a private frame stack that must complete without blocking.
//!
//! Scheduling follows the IEEE 1800 regions in simplified form: active
//! processes run to suspension in FIFO order, then `wait(cond)` waiters are
//! re-checked if anything was written, then `#0` waiters, then nonblocking
//! assignments are applied, and only when all are empty does time advance.

mod builtin;
mod call;
mod dut;
mod elab;
mod eval;
mod exec;
mod format;
mod rand;

use std::cell::RefCell;
use std::collections::{BTreeMap, BinaryHeap, HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::SvError;
use crate::ast::*;
use crate::pp::SourceMap;
use crate::value::*;

pub use dut::{DutInst, IfaceInst};
pub(crate) use elab::ScopeHit;

pub type Pid = usize;
pub type NsId = usize;
pub type SigId = usize;

/// Message severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sev {
    Info,
    Warning,
    Error,
    Fatal,
}

/// Engine output events (streamed while running).
#[derive(Clone, Debug)]
pub enum Event {
    Log { time_ps: u64, sev: Sev, text: String },
    Failure { file: String, line: usize, msg: String, time_ps: u64 },
    Progress { time_ps: u64 },
}

#[derive(Clone, Debug)]
pub struct Failure {
    pub file: String,
    pub line: usize,
    pub msg: String,
    pub time_ps: u64,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub time_ps: u64,
    pub sev: Sev,
    pub text: String,
}

/// Synthesizes a DUT module referenced by the testbench.
pub trait DutProvider {
    /// Return the synthesized design and the RTL source it came from.
    fn synth(&mut self, module: &str) -> Result<(helion_ir::Design, std::path::PathBuf), String>;
}

#[derive(Clone, Debug)]
pub struct RunConfig {
    pub seed: u64,
    /// Watchdog in picoseconds.
    pub max_ps: u64,
    /// `+NAME` / `+NAME=value` plusargs (`+UVM_TESTNAME=…`).
    pub plusargs: Vec<String>,
    pub vcd: bool,
    pub cancel: Option<Arc<AtomicBool>>,
    /// Top module (default: the single uninstantiated module).
    pub top: Option<String>,
    /// Statements one process may run without suspending.
    pub max_slice: u64,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self { seed: 1, max_ps: 10_000_000_000, plusargs: vec![], vcd: false, cancel: None, top: None, max_slice: 20_000_000 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RunResult {
    pub time_ps: u64,
    pub finished: bool,
    pub cancelled: bool,
    pub failures: Vec<Failure>,
    pub log: Vec<LogLine>,
    /// Statement executions per `(file id, line)`.
    pub line_hits: BTreeMap<(u32, u32), u64>,
    /// Executable statement lines per file id (zero-hit lines included).
    pub lines: BTreeMap<(u32, u32), ()>,
    /// `if` / `assert` outcomes per `(file, line)`: (true, false).
    pub branches: BTreeMap<(u32, u32), (u64, u64, &'static str)>,
    pub map: SourceMap,
    pub top_file: u32,
    pub duts: Vec<DutReport>,
    /// VCD traces: (dotted name, width) and samples (time_ps, idx, value).
    pub trace: Vec<(String, u32)>,
    pub samples: Vec<(u64, usize, u64)>,
    pub displays: u32,
}

#[derive(Clone, Debug)]
pub struct DutReport {
    pub instance: String,
    pub module: String,
    pub src: std::path::PathBuf,
    pub cov: helion_sim::FunctionalCoverageReport,
    pub lines: helion_sim::LineCoverageReport,
}

// ---------------------------------------------------------------------------
// Namespaces and classes
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NsKind {
    Unit,
    Package,
    Module,
}

pub struct Namespace {
    pub name: String,
    pub kind: NsKind,
    pub vars: RefCell<HashMap<String, Var>>,
    pub sigs: RefCell<HashMap<String, SigId>>,
    pub ifaces: RefCell<HashMap<String, Rc<IfaceInst>>>,
    pub subs: HashMap<String, Rc<SubDecl>>,
    pub classes: HashMap<String, Rc<ClassDecl>>,
    pub typedefs: HashMap<String, Typedef>,
    pub ty_cache: RefCell<HashMap<String, Ty>>,
    pub imports: RefCell<Vec<NsId>>,
    pub item_imports: RefCell<Vec<(NsId, String)>>,
    pub parent: Option<NsId>,
    /// Out-of-class method bodies: `(class, method)`.
    pub externs: HashMap<(String, String), Rc<SubDecl>>,
}

impl Namespace {
    fn new(name: &str, kind: NsKind, parent: Option<NsId>) -> Self {
        Self {
            name: name.into(),
            kind,
            vars: RefCell::new(HashMap::new()),
            sigs: RefCell::new(HashMap::new()),
            ifaces: RefCell::new(HashMap::new()),
            subs: HashMap::new(),
            classes: HashMap::new(),
            typedefs: HashMap::new(),
            ty_cache: RefCell::new(HashMap::new()),
            imports: RefCell::new(Vec::new()),
            item_imports: RefCell::new(Vec::new()),
            parent,
            externs: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ParamVal {
    Type(Ty),
    Val(Value),
}

pub struct Ctor {
    pub sub: Rc<SubDecl>,
    pub super_args: Option<(Vec<Option<Expr>>, Loc)>,
    pub rest: Body,
}

pub struct ClassInfo {
    pub id: ClassId,
    pub name: String,
    pub decl: Rc<ClassDecl>,
    pub parent: std::cell::Cell<Option<ClassId>>,
    pub params: HashMap<String, ParamVal>,
    pub ns: NsId,
    pub outer: Option<ClassId>,
    pub statics: RefCell<HashMap<String, Var>>,
    pub methods: HashMap<String, Rc<SubDecl>>,
    pub typedefs: RefCell<HashMap<String, Ty>>,
    /// Instance properties: (name, type, rand kind), resolved lazily.
    pub fields: RefCell<Option<Rc<Vec<(String, Ty, RandKind)>>>>,
    pub ctor: Option<Ctor>,
    pub constraints: Vec<ConstraintDecl>,
}

// ---------------------------------------------------------------------------
// Processes
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Ctx {
    pub scopes: Vec<ScopeRef>,
    pub this: Option<ObjRef>,
    pub class: Option<ClassId>,
    pub ns: NsId,
}

#[derive(Clone)]
pub(crate) enum LoopSt {
    While,
    DoWhile { first: bool },
    For { started: bool },
    Repeat { left: u64 },
    Forever,
    Foreach { tuples: Rc<Vec<Vec<Value>>>, idx: usize, vars: Vec<Option<String>> },
}

#[derive(Clone)]
pub(crate) enum Frame {
    Seq { body: Body, pc: usize, scoped: bool },
    /// Loop statement `owner[idx]`.
    Loop { owner: Body, idx: usize, st: LoopSt, scoped: bool },
    /// Subroutine boundary: output arguments copied back on return.
    Call { outs: Vec<(LPath, String)>, ret: Option<Ty>, func: String },
}

#[derive(Clone, Default)]
pub(crate) struct ExecState {
    pub frames: Vec<Frame>,
    pub ctxs: Vec<Ctx>,
    pub retval: Option<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PState {
    Ready,
    Blocked,
    Done,
    Killed,
}

pub(crate) struct Proc {
    pub exec: ExecState,
    pub state: PState,
    pub epoch: u64,
    pub parent: Option<Pid>,
    pub children: Vec<Pid>,
    pub group: Option<usize>,
    pub name: String,
}

pub(crate) struct ForkGroup {
    pub parent: Pid,
    pub remaining: usize,
    pub kind: JoinKind,
    pub done: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum WaitKey {
    Sig(SigId),
    Event(u64),
    Obj(u64),
    Join(usize),
    Children(Pid),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Waiter {
    pub pid: Pid,
    pub epoch: u64,
    pub edge: Option<Edge>,
}

/// Where a value lives.
#[derive(Clone)]
pub(crate) enum Place {
    Local(ScopeRef, String),
    Field(ObjRef, String),
    Static(ClassId, String),
    NsVar(NsId, String),
    Sig(SigId),
    /// Clocking block variable: interface, block, signal.
    Clock(Rc<IfaceInst>, String, String),
    /// A value with no storage (constants, rvalues).
    Const(Value, Ty),
}

#[derive(Clone)]
pub(crate) enum Sel {
    Index(Value),
    Bits(u32, u32),
}

#[derive(Clone)]
pub(crate) struct LPath {
    pub base: Place,
    pub sels: Vec<Sel>,
}

/// Outcome of executing one statement.
pub(crate) enum Step {
    Go,
    Block,
    /// Block and re-execute the same statement when woken.
    Retry,
}

pub struct Signal {
    pub name: String,
    pub ty: Ty,
    pub v: Value,
    pub trace: Option<usize>,
    /// DUT input bindings: (dut index, port, is clock).
    pub dut_in: Vec<(usize, String, bool)>,
}

pub struct Engine<'a> {
    pub(crate) map: SourceMap,
    pub(crate) nss: Vec<Namespace>,
    pub(crate) pkgs: HashMap<String, NsId>,
    pub(crate) classes: Vec<Rc<ClassInfo>>,
    pub(crate) class_keys: HashMap<String, ClassId>,
    pub(crate) modules: HashMap<String, Rc<ModuleDecl>>,
    next_obj: u64,
    pub(crate) procs: Vec<Proc>,
    ready: VecDeque<(Pid, u64)>,
    timed: BinaryHeap<std::cmp::Reverse<(u64, u64, Pid, u64)>>,
    inactive: Vec<(Pid, u64)>,
    seq: u64,
    pub(crate) time: u64,
    pub(crate) waits: HashMap<WaitKey, Vec<Waiter>>,
    cond_waiters: Vec<(Pid, u64, u64)>,
    pub(crate) nba: Vec<(LPath, Value)>,
    pub(crate) forks: Vec<ForkGroup>,
    pub(crate) finished: bool,
    pub(crate) dirty: u64,
    pub(crate) cur: Option<Pid>,
    pub(crate) cur_loc: Loc,
    pub(crate) sigs: Vec<Signal>,
    pub(crate) duts: Vec<DutInst>,
    pub(crate) clock_watch: HashMap<SigId, Vec<(Rc<IfaceInst>, String)>>,
    pub(crate) sink: &'a mut dyn FnMut(Event),
    pub(crate) cfg: RunConfig,
    pub(crate) res: RunResult,
    pub(crate) rng: u64,
    pub(crate) dollar: Vec<i64>,
    pub(crate) slice_budget: u64,
    pub(crate) final_blocks: Vec<(Body, NsId)>,
    pub(crate) unit: Rc<SourceUnit>,
    /// Item variable (`item`) values while evaluating `with (…)` clauses.
    pub(crate) with_item: Vec<(String, Value, Value)>,
}

impl<'a> Engine<'a> {
    pub fn new(unit: Rc<SourceUnit>, map: SourceMap, cfg: RunConfig, sink: &'a mut dyn FnMut(Event)) -> Self {
        let seed = cfg.seed;
        Self {
            map: map.clone(),
            nss: Vec::new(),
            pkgs: HashMap::new(),
            classes: Vec::new(),
            class_keys: HashMap::new(),
            modules: HashMap::new(),
            next_obj: 1,
            procs: Vec::new(),
            ready: VecDeque::new(),
            timed: BinaryHeap::new(),
            inactive: Vec::new(),
            seq: 0,
            time: 0,
            waits: HashMap::new(),
            cond_waiters: Vec::new(),
            nba: Vec::new(),
            forks: Vec::new(),
            finished: false,
            dirty: 0,
            cur: None,
            cur_loc: Loc::default(),
            sigs: Vec::new(),
            duts: Vec::new(),
            clock_watch: HashMap::new(),
            sink,
            cfg,
            res: RunResult { map, ..Default::default() },
            rng: seed ^ 0x9E37_79B9_7F4A_7C15,
            dollar: Vec::new(),
            slice_budget: 0,
            final_blocks: Vec::new(),
            unit,
            with_item: Vec::new(),
        }
    }

    // ---- diagnostics -------------------------------------------------------

    pub(crate) fn label(&self, file: u32) -> String {
        self.map.label(file)
    }

    pub(crate) fn err_at(&self, loc: Loc, m: impl Into<String>) -> SvError {
        SvError::new(&self.label(loc.file), loc.line as usize, m)
    }

    pub(crate) fn rt(&self, m: impl Into<String>) -> SvError {
        self.err_at(self.cur_loc, m)
    }

    pub(crate) fn log(&mut self, sev: Sev, text: String) {
        let t = self.time;
        self.res.log.push(LogLine { time_ps: t, sev, text: text.clone() });
        (self.sink)(Event::Log { time_ps: t, sev, text });
    }

    pub(crate) fn fail_at(&mut self, file: String, line: usize, msg: String) {
        let t = self.time;
        self.res.failures.push(Failure { file: file.clone(), line, msg: msg.clone(), time_ps: t });
        (self.sink)(Event::Failure { file, line, msg, time_ps: t });
    }

    pub(crate) fn fail(&mut self, loc: Loc, msg: String) {
        let f = self.label(loc.file);
        self.fail_at(f, loc.line as usize, msg);
    }

    pub(crate) fn next_rand(&mut self) -> u64 {
        // SplitMix64.
        self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub(crate) fn new_obj_id(&mut self) -> u64 {
        let id = self.next_obj;
        self.next_obj += 1;
        id
    }

    // ---- processes -----------------------------------------------------------

    pub(crate) fn spawn(&mut self, name: String, exec: ExecState, parent: Option<Pid>, group: Option<usize>) -> Pid {
        let pid = self.procs.len();
        self.procs.push(Proc { exec, state: PState::Ready, epoch: 0, parent, children: Vec::new(), group, name });
        if let Some(p) = parent {
            self.procs[p].children.push(pid);
        }
        self.ready.push_back((pid, 0));
        pid
    }

    /// Mark `pid` blocked (bumping its generation) and return the new epoch.
    pub(crate) fn block(&mut self, pid: Pid) -> u64 {
        let p = &mut self.procs[pid];
        p.epoch += 1;
        p.state = PState::Blocked;
        p.epoch
    }

    pub(crate) fn wake(&mut self, pid: Pid, epoch: u64) -> bool {
        let p = &mut self.procs[pid];
        if p.state == PState::Blocked && p.epoch == epoch {
            p.epoch += 1;
            p.state = PState::Ready;
            let g = p.epoch;
            self.ready.push_back((pid, g));
            true
        } else {
            false
        }
    }

    pub(crate) fn wait_on(&mut self, pid: Pid, epoch: u64, key: WaitKey, edge: Option<Edge>) {
        self.waits.entry(key).or_default().push(Waiter { pid, epoch, edge });
    }

    pub(crate) fn wait_time(&mut self, pid: Pid, epoch: u64, dt: u64) {
        if dt == 0 {
            self.inactive.push((pid, epoch));
        } else {
            self.seq += 1;
            let t = self.time.saturating_add(dt);
            self.timed.push(std::cmp::Reverse((t, self.seq, pid, epoch)));
        }
    }

    pub(crate) fn wait_cond(&mut self, pid: Pid, epoch: u64) {
        self.cond_waiters.push((pid, epoch, self.dirty));
    }

    /// Wake every waiter on `key` (events, mailboxes).
    pub(crate) fn notify(&mut self, key: &WaitKey) {
        if let Some(ws) = self.waits.remove(key) {
            for w in ws {
                self.wake(w.pid, w.epoch);
            }
        }
    }

    /// Signal `id` changed from `old` to `new`: wake matching edge waiters.
    pub(crate) fn notify_sig(&mut self, id: SigId, old: u64, new: u64) {
        let key = WaitKey::Sig(id);
        let Some(ws) = self.waits.remove(&key) else { return };
        let mut keep = Vec::new();
        for w in ws {
            let hit = match w.edge {
                Some(Edge::Pos) => old & 1 == 0 && new & 1 == 1,
                Some(Edge::Neg) => old & 1 == 1 && new & 1 == 0,
                Some(Edge::Any) => old & 1 != new & 1,
                None => old != new,
            };
            if hit {
                self.wake(w.pid, w.epoch);
            } else if self.procs[w.pid].state == PState::Blocked && self.procs[w.pid].epoch == w.epoch {
                keep.push(w);
            }
        }
        if !keep.is_empty() {
            self.waits.insert(key, keep);
        }
    }

    /// A process finished or was killed: settle its fork group / waiters.
    fn proc_ended(&mut self, pid: Pid) {
        if let Some(g) = self.procs[pid].group {
            let grp = &mut self.forks[g];
            grp.remaining = grp.remaining.saturating_sub(1);
            let fire = !grp.done && (grp.kind == JoinKind::Any || grp.remaining == 0);
            if fire {
                grp.done = true;
                self.notify(&WaitKey::Join(g));
            }
        }
        if let Some(par) = self.procs[pid].parent {
            self.notify(&WaitKey::Children(par));
        }
    }

    /// Kill `pid` and all its descendants.
    pub(crate) fn kill(&mut self, pid: Pid) {
        let children = self.procs[pid].children.clone();
        for c in children {
            self.kill(c);
        }
        let st = self.procs[pid].state;
        if matches!(st, PState::Ready | PState::Blocked) {
            let p = &mut self.procs[pid];
            p.state = PState::Killed;
            p.epoch += 1;
            p.exec = ExecState::default();
            self.proc_ended(pid);
        }
    }

    /// Live (not finished) descendants of `pid`.
    pub(crate) fn has_live_children(&self, pid: Pid) -> bool {
        self.procs[pid].children.iter().any(|&c| matches!(self.procs[c].state, PState::Ready | PState::Blocked))
    }

    fn cancelled(&self) -> bool {
        self.cfg.cancel.as_deref().is_some_and(|c| c.load(Ordering::Relaxed))
    }

    fn run_proc(&mut self, pid: Pid) -> Result<(), SvError> {
        self.cur = Some(pid);
        self.slice_budget = self.cfg.max_slice;
        let mut st = std::mem::take(&mut self.procs[pid].exec);
        let r = self.step(&mut st, false);
        self.cur = None;
        match r {
            Ok(true) => {
                // Finished (frames empty).
                if matches!(self.procs[pid].state, PState::Ready | PState::Blocked) {
                    self.procs[pid].state = PState::Done;
                    self.proc_ended(pid);
                }
            }
            Ok(false) => {
                if self.procs[pid].state != PState::Killed {
                    self.procs[pid].exec = st;
                }
            }
            Err(e) => return Err(e),
        }
        Ok(())
    }

    /// Run the scheduler until `$finish`, no events, timeout, or cancel.
    pub fn run(&mut self) -> Result<(), SvError> {
        let mut last_progress = 0u64;
        loop {
            if self.finished {
                break;
            }
            if self.cancelled() {
                self.res.cancelled = true;
                break;
            }
            if let Some((pid, epoch)) = self.ready.pop_front() {
                if self.procs[pid].state == PState::Ready && self.procs[pid].epoch == epoch {
                    if let Err(e) = self.run_proc(pid) {
                        // Runtime errors are fatal testbench failures at the
                        // statement that raised them.
                        let line = e.line;
                        let file = e.file.clone();
                        let msg = format!("runtime error: {}", e.msg);
                        self.log(Sev::Fatal, format!("{file}:{line}: {msg}"));
                        self.fail_at(file, line, msg);
                        self.finished = true;
                    }
                }
                continue;
            }
            // wait(cond) waiters: retry when anything was written.
            if !self.cond_waiters.is_empty() {
                let dirty = self.dirty;
                let ws = std::mem::take(&mut self.cond_waiters);
                let mut woke = false;
                for (pid, epoch, seen) in ws {
                    if seen != dirty {
                        woke |= self.wake(pid, epoch);
                    } else if self.procs[pid].state == PState::Blocked && self.procs[pid].epoch == epoch {
                        self.cond_waiters.push((pid, epoch, seen));
                    }
                }
                if woke {
                    continue;
                }
            }
            if !self.inactive.is_empty() {
                for (pid, epoch) in std::mem::take(&mut self.inactive) {
                    self.wake(pid, epoch);
                }
                continue;
            }
            if !self.nba.is_empty() {
                for (path, v) in std::mem::take(&mut self.nba) {
                    self.write_path(&path, v)?;
                }
                continue;
            }
            // Advance time.
            let Some(std::cmp::Reverse((t, _, pid, epoch))) = self.timed.pop() else { break };
            if t > self.cfg.max_ps {
                let msg = format!(
                    "simulation timed out at {} ns (watchdog {} ns); call $finish or drop all UVM objections",
                    t / 1000,
                    self.cfg.max_ps / 1000
                );
                self.log(Sev::Fatal, msg.clone());
                let top = self.res.top_file;
                self.fail(Loc { file: top, line: 0 }, msg);
                break;
            }
            self.time = t;
            self.wake(pid, epoch);
            while let Some(std::cmp::Reverse((t2, _, p2, g2))) = self.timed.peek().cloned() {
                if t2 != t {
                    break;
                }
                self.timed.pop();
                self.wake(p2, g2);
            }
            if self.time / 1000 >= last_progress + 100_000 {
                last_progress = self.time / 1000;
                (self.sink)(Event::Progress { time_ps: self.time });
            }
        }
        // `final` blocks.
        for (body, ns) in std::mem::take(&mut self.final_blocks) {
            let ctx = Ctx { scopes: vec![], this: None, class: None, ns };
            let mut st = ExecState { frames: vec![Frame::Seq { body, pc: 0, scoped: false }], ctxs: vec![ctx], retval: None };
            self.slice_budget = self.cfg.max_slice;
            if let Err(e) = self.step(&mut st, true) {
                self.log(Sev::Error, e.to_string());
            }
        }
        self.res.time_ps = self.time;
        Ok(())
    }

    pub fn into_result(mut self) -> RunResult {
        for d in &self.duts {
            self.res.duts.push(DutReport {
                instance: d.name.clone(),
                module: d.module.clone(),
                src: d.src.clone(),
                cov: d.sim.coverage_report(),
                lines: d.sim.line_coverage_report(),
            });
        }
        let mut trace = Vec::new();
        for s in &self.sigs {
            if s.trace.is_some() {
                trace.push((s.name.clone(), s.ty.width()));
            }
        }
        self.res.trace = trace;
        self.res
    }

    /// Classes deriving (transitively) from `base` by name.
    pub fn classes_deriving(&self, base: &str) -> Vec<String> {
        let mut out = Vec::new();
        for c in &self.classes {
            if c.decl.params.is_empty() || c.params.is_empty() {
                let mut p = c.parent.get();
                while let Some(pid) = p {
                    if self.classes[pid as usize].decl.name == base {
                        out.push(c.decl.name.clone());
                        break;
                    }
                    p = self.classes[pid as usize].parent.get();
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }
}
