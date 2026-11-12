//! Event-driven VHDL simulation kernel (IEEE 1076 simulation cycle):
//! drivers with projected waveforms, delta cycles, resolved `std_logic`,
//! process suspension on `wait`, and netlist DUT bridges into helion-sim.

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use super::elab::{BridgeDef, Elab};
use super::eval::{Flow, Host, Mach, PathSel, coerce, cond_true, fit_like, read_path, write_path};
use super::ir::*;
use super::value::*;

/// Delta cycles allowed at one time point before the run is aborted.
pub const DELTA_LIMIT: u32 = 10_000;

#[derive(Clone, Debug)]
struct Txn {
    time: i64,
    path: Vec<PathSel>,
    value: Value,
}

#[derive(Clone, Debug)]
struct Driver {
    owner: usize,
    value: Value,
    pending: Vec<Txn>,
}

#[derive(Clone, Debug)]
pub struct SigRt {
    pub cur: Value,
    pub last: Value,
    pub last_event: Option<i64>,
    pub event: bool,
    drivers: Vec<Driver>,
    pub ty: Ty,
    pub trace: Option<(usize, u32)>,
}

#[derive(Clone, Debug)]
struct WaitRt {
    on: Vec<SigId>,
    until: Option<RExpr>,
    timeout_at: Option<i64>,
}

struct ProcRt {
    pc: usize,
    vars: Vec<Value>,
    wait: Option<WaitRt>,
}

pub struct BridgeRt {
    pub label: String,
    pub path: std::path::PathBuf,
    sim: helion_sim::Sim,
    /// `(sim port, signal, width)`.
    inputs: Vec<(String, SigId, u32)>,
    outputs: Vec<(String, SigId, u32)>,
    clock: Option<SigId>,
    last_edge_fs: i64,
    owner: usize,
}

/// One reported event (log line or failure) produced by the kernel.
#[derive(Clone, Debug)]
pub enum KEvent {
    Log { time_fs: i64, sev: u32, text: String },
    Failure { time_fs: i64, loc: Loc, message: String, signals: Vec<(String, u64)> },
}

pub struct Kernel<'a> {
    pub elab: &'a Elab,
    pub sigs: Vec<SigRt>,
    procs: Vec<ProcRt>,
    pub bridges: Vec<BridgeRt>,
    pub now: i64,
    times: BTreeSet<i64>,
    pending: BTreeSet<SigId>,
    evented: Vec<SigId>,
    pub stopped: bool,
    pub events: Vec<KEvent>,
    /// `(time_ps, trace index, value, has_x)`.
    pub vcd: Vec<(u64, usize, u64, bool)>,
    pub trace_names: Vec<(String, u32)>,
    /// Per (file, line) execution counts.
    pub line_hits: HashMap<(u16, u32), u64>,
    /// Per (file, line) branch outcomes of `if`/`while`/`assert`.
    pub branch_hits: HashMap<(u16, u32), (u64, u64)>,
    pub max_fs: i64,
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub cancelled: bool,
    pub on_event: Option<Box<dyn FnMut(&KEvent) + 'a>>,
    /// Signals snapshotted into each failure (`(label, signal)`).
    pub watch: Vec<(String, SigId)>,
}

impl BridgeRt {
    pub fn sim_coverage(&self) -> helion_sim::FunctionalCoverageReport {
        self.sim.coverage_report()
    }
}

struct View<'s> {
    sigs: &'s [SigRt],
    now: i64,
    reports: Vec<(u32, String, Loc)>,
}

impl Host for View<'_> {
    fn sig(&self, id: SigId) -> &Value {
        &self.sigs[id].cur
    }
    fn event(&self, id: SigId) -> bool {
        self.sigs[id].event
    }
    fn last_value(&self, id: SigId) -> &Value {
        &self.sigs[id].last
    }
    fn last_event(&self, id: SigId) -> Option<i64> {
        self.sigs[id].last_event
    }
    fn now(&self) -> i64 {
        self.now
    }
    fn report(&mut self, sev: u32, msg: String, loc: Loc) {
        self.reports.push((sev, msg, loc));
    }
}

fn logic_char(v: &Value) -> u8 {
    match v {
        Value::Ch(c) => *c,
        Value::Bool(b) => if *b { b'1' } else { b'0' },
        _ => b'X',
    }
}

/// Resolve several drivers of a `std_logic`-based signal elementwise.
fn resolve_values(vals: &[&Value]) -> Value {
    match vals[0] {
        Value::Ch(_) => Value::Ch(vals.iter().map(|v| logic_char(v)).reduce(resolve2).unwrap_or(b'U')),
        Value::Arr(a) if a.elems.iter().all(|e| matches!(e, Value::Ch(_))) => {
            let mut elems = a.elems.clone();
            for (k, e) in elems.iter_mut().enumerate() {
                let mut acc = logic_char(e);
                for v in &vals[1..] {
                    if let Value::Arr(b) = v {
                        acc = resolve2(acc, b.elems.get(k).map(logic_char).unwrap_or(b'U'));
                    }
                }
                *e = Value::Ch(acc);
            }
            Value::Arr(Rc::new(ArrVal { kind: a.kind, left: a.left, asc: a.asc, elems }))
        }
        // Unresolved types: the last active driver wins.
        _ => vals[vals.len() - 1].clone(),
    }
}

fn width_of(v: &Value) -> u32 {
    match v {
        Value::Arr(a) => a.elems.len() as u32,
        _ => 1,
    }
}

impl<'a> Kernel<'a> {
    pub fn new(elab: &'a Elab, max_fs: i64) -> Result<Self, String> {
        let mut trace_names = Vec::new();
        let sigs: Vec<SigRt> = elab
            .signals
            .iter()
            .map(|s| {
                let trace = s.ty.trace_width().map(|w| {
                    trace_names.push((s.name.clone(), w));
                    (trace_names.len() - 1, w)
                });
                SigRt { cur: s.init.clone(), last: s.init.clone(), last_event: None, event: false, drivers: Vec::new(), ty: s.ty.clone(), trace }
            })
            .collect();
        let procs = elab
            .procs
            .iter()
            .map(|p| ProcRt {
                pc: 0,
                vars: p.code.var_tys.iter().map(|t| t.as_ref().map(Ty::default_value).unwrap_or(Value::Int(0))).collect(),
                wait: None,
            })
            .collect();
        let mut k = Kernel {
            elab,
            sigs,
            procs,
            bridges: Vec::new(),
            now: 0,
            times: BTreeSet::new(),
            pending: BTreeSet::new(),
            evented: Vec::new(),
            stopped: false,
            events: Vec::new(),
            vcd: Vec::new(),
            trace_names,
            line_hits: HashMap::new(),
            branch_hits: HashMap::new(),
            max_fs,
            cancel: None,
            cancelled: false,
            on_event: None,
            watch: Vec::new(),
        };
        for (bi, b) in elab.bridges.iter().enumerate() {
            let rt = k.make_bridge(b, elab.procs.len() + bi)?;
            k.bridges.push(rt);
        }
        Ok(k)
    }

    fn make_bridge(&self, b: &BridgeDef, owner: usize) -> Result<BridgeRt, String> {
        let vhdl = b.path.extension().is_some_and(|e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "vhd" | "vhdl"));
        let design = if vhdl { helion_vhdl::synth_vhdl_path(&b.path) } else { helion_sv::synth_sv_path(&b.path) }
            .map_err(|e| format!("{}: synthesis failed: {e}", b.path.display()))?;
        let sim = helion_sim::Sim::new(&design);
        let ins = sim.input_ports();
        let outs = sim.output_ports();
        let find = |list: &[String], n: &str| list.iter().find(|p| p.eq_ignore_ascii_case(n)).cloned();
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let mut clock = None;
        for (formal, sig, _) in &b.ports {
            let w = width_of(&self.sigs[*sig].cur);
            if let Some(p) = find(&ins, formal) {
                let l = p.to_ascii_lowercase();
                if l == "clk" || l == "clock" || l.ends_with("_clk") || l.starts_with("clk_") || l == "aclk" {
                    clock = Some(*sig);
                }
                inputs.push((p, *sig, w));
            } else if let Some(p) = find(&outs, formal) {
                outputs.push((p, *sig, w));
            } else {
                let mut known: Vec<String> = ins.iter().chain(outs.iter()).cloned().collect();
                known.sort();
                return Err(format!("DUT `{}` has no port `{formal}` (ports: {})", b.unit, known.join(", ")));
            }
        }
        Ok(BridgeRt { label: b.label.clone(), path: b.path.clone(), sim, inputs, outputs, clock, last_edge_fs: 0, owner })
    }

    fn emit(&mut self, e: KEvent) {
        if let Some(cb) = self.on_event.as_mut() {
            cb(&e);
        }
        self.events.push(e);
    }

    fn handle_reports(&mut self, reports: Vec<(u32, String, Loc)>) {
        for (sev, msg, loc) in reports {
            let file = self.elab.files.get(loc.file as usize).cloned().unwrap_or_default();
            let name = ["note", "warning", "error", "failure"].get(sev as usize).copied().unwrap_or("note");
            let text = format!("{file}:{}: report {name}: {msg}", loc.line);
            self.emit(KEvent::Log { time_fs: self.now, sev: sev.min(3), text });
            if sev >= 2 {
                let signals = self.snapshot();
                self.emit(KEvent::Failure { time_fs: self.now, loc, message: msg, signals });
            }
            if sev >= 3 {
                self.stopped = true;
            }
        }
    }

    pub fn snapshot(&self) -> Vec<(String, u64)> {
        self.watch
            .iter()
            .take(8)
            .map(|(n, s)| (n.clone(), self.sigs[*s].cur.trace(64).0))
            .collect()
    }

    fn sample_vcd(&mut self, id: SigId) {
        if let Some((idx, w)) = self.sigs[id].trace {
            let (v, x) = self.sigs[id].cur.trace(w);
            self.vcd.push(((self.now / 1000).max(0) as u64, idx, v, x));
        }
    }

    // ------------------------------------------------------------------
    // Scheduling
    // ------------------------------------------------------------------

    fn driver_mut(&mut self, sig: SigId, owner: usize) -> &mut Driver {
        let s = &mut self.sigs[sig];
        if let Some(i) = s.drivers.iter().position(|d| d.owner == owner) {
            return &mut s.drivers[i];
        }
        let init = s.cur.clone();
        s.drivers.push(Driver { owner, value: init, pending: Vec::new() });
        s.drivers.last_mut().expect("driver")
    }

    fn schedule(&mut self, owner: usize, sig: SigId, path: Vec<PathSel>, wave: Vec<(Value, i64)>, transport: bool) {
        let now = self.now;
        let drv = self.driver_mut(sig, owner);
        let first_t = wave.first().map(|w| now + w.1).unwrap_or(now);
        drv.pending.retain(|t| {
            let same = paths_overlap(&t.path, &path);
            // Transport: drop later transactions; inertial (default): drop all.
            !(same && (!transport || t.time >= first_t))
        });
        for (v, d) in wave {
            drv.pending.push(Txn { time: now + d, path: path.clone(), value: v });
        }
        drv.pending.sort_by_key(|t| t.time);
        for t in &self.sigs[sig].drivers.iter().find(|d| d.owner == owner).expect("driver").pending {
            self.times.insert(t.time);
        }
        self.pending.insert(sig);
    }

    /// Apply due transactions; returns the signals that changed.
    fn update_signals(&mut self) -> Vec<SigId> {
        let now = self.now;
        let mut changed = Vec::new();
        let ids: Vec<SigId> = self.pending.iter().copied().collect();
        for id in ids {
            let s = &mut self.sigs[id];
            let mut active = false;
            let mut still = false;
            for d in &mut s.drivers {
                while d.pending.first().is_some_and(|t| t.time <= now) {
                    let t = d.pending.remove(0);
                    let _ = write_path(&mut d.value, &t.path, t.value);
                    active = true;
                }
                still |= !d.pending.is_empty();
            }
            if !still {
                self.pending.remove(&id);
            }
            if !active {
                continue;
            }
            let new = if s.drivers.len() == 1 {
                s.drivers[0].value.clone()
            } else {
                let vals: Vec<&Value> = s.drivers.iter().map(|d| &d.value).collect();
                resolve_values(&vals)
            };
            if new != s.cur {
                s.last = std::mem::replace(&mut s.cur, new);
                s.last_event = Some(now);
                s.event = true;
                changed.push(id);
            }
        }
        for &id in &changed {
            self.sample_vcd(id);
        }
        changed
    }

    // ------------------------------------------------------------------
    // Bridges
    // ------------------------------------------------------------------

    fn bridge_outputs(&mut self, bi: usize, initial: bool) {
        let outs = self.bridges[bi].outputs.clone();
        let owner = self.bridges[bi].owner;
        for (port, sig, w) in outs {
            let b = &self.bridges[bi];
            let bits: Vec<u8> = (0..w)
                .rev()
                .map(|i| {
                    let net = if w == 1 { port.clone() } else { format!("{port}_{i}") };
                    match b.sim.sample(&net) {
                        Some(1) => b'1',
                        Some(_) => b'0',
                        None => b'X',
                    }
                })
                .collect();
            let v = if w == 1 && !matches!(self.sigs[sig].cur, Value::Arr(_)) {
                Value::Ch(bits[0])
            } else {
                match coerce(Value::vec(ArrKind::Slv, &bits), &self.sigs[sig].ty) {
                    Ok(v) => v,
                    Err(_) => continue,
                }
            };
            let cur_drv = self.sigs[sig].drivers.iter().find(|d| d.owner == owner).map(|d| d.value.clone());
            if initial || cur_drv.as_ref() != Some(&v) {
                self.schedule(owner, sig, Vec::new(), vec![(v, 0)], true);
            }
        }
    }

    fn bridge_inputs_changed(&mut self, bi: usize) -> bool {
        let b = &self.bridges[bi];
        b.inputs.iter().any(|(_, s, _)| self.sigs[*s].event)
    }

    fn bridge_step(&mut self, bi: usize) {
        let now = self.now;
        let inputs = self.bridges[bi].inputs.clone();
        let clock = self.bridges[bi].clock;
        for (port, sig, w) in &inputs {
            if Some(*sig) == clock {
                continue;
            }
            let bits = self.sigs[*sig].cur.bits().unwrap_or_default();
            for i in 0..*w {
                let c = bits.get(bits.len().saturating_sub(1 + i as usize)).copied().unwrap_or(b'0');
                let net = if *w == 1 { port.clone() } else { format!("{port}_{i}") };
                let _ = self.bridges[bi].sim.drive_input(&net, matches!(c, b'1' | b'H'));
            }
        }
        if let Some(c) = clock {
            let s = &self.sigs[c];
            let rise = s.event && to_x01(logic_char(&s.cur)) == b'1' && to_x01(logic_char(&s.last)) == b'0';
            if rise {
                let b = &mut self.bridges[bi];
                let dt_ps = ((now - b.last_edge_fs) / 1000).max(1) as u64;
                b.last_edge_fs = now;
                b.sim.step_posedge(dt_ps);
            }
        }
        self.bridge_outputs(bi, false);
    }

    // ------------------------------------------------------------------
    // Processes
    // ------------------------------------------------------------------

    fn run_process(&mut self, pi: usize) -> Result<(), (Loc, String)> {
        let code = self.elab.procs[pi].code.clone();
        let mut vars = std::mem::take(&mut self.procs[pi].vars);
        let mut pc = self.procs[pi].pc;
        let mut steps: u64 = 0;
        let result = loop {
            if pc >= code.ops.len() {
                pc = code.body_start;
            }
            steps += 1;
            if steps > 20_000_000 {
                break Err((self.elab.procs[pi].loc, format!("process {} ran 20M steps without suspending", self.elab.procs[pi].name)));
            }
            let op = &code.ops[pc];
            if let Some(l) = op_loc(op) {
                *self.line_hits.entry((l.file, l.line)).or_insert(0) += 1;
            }
            match self.exec_op(pi, op, pc, &mut vars) {
                Ok(Step::Next) => pc += 1,
                Ok(Step::Jump(t)) => pc = t,
                Ok(Step::Suspend(w)) => {
                    if let Some(t) = w.timeout_at {
                        self.times.insert(t);
                    }
                    self.procs[pi].wait = Some(w);
                    pc += 1;
                    break Ok(());
                }
                Ok(Step::Stop) => {
                    self.stopped = true;
                    pc += 1;
                    break Ok(());
                }
                Err(e) => break Err((op_loc(op).unwrap_or(self.elab.procs[pi].loc), e)),
            }
            if self.stopped {
                break Ok(());
            }
        };
        self.procs[pi].pc = pc;
        self.procs[pi].vars = vars;
        result
    }

    fn exec_op(&mut self, pi: usize, op: &Op, pc: usize, vars: &mut Vec<Value>) -> Result<Step, String> {
        let funcs = &self.elab.funcs;
        match op {
            Op::SigAssign { target, wave, transport, ty, .. } => {
                let mut view = View { sigs: &self.sigs, now: self.now, reports: Vec::new() };
                let mut m = Mach { funcs, host: &mut view, depth: 0 };
                let mut assigns: Vec<(SigId, Vec<PathSel>, Vec<(Value, i64)>)> = Vec::new();
                let mut vals = Vec::new();
                for (v, after) in wave {
                    let x = m.eval_want(v, vars, ty.as_ref())?;
                    let d = match after {
                        Some(a) => match m.eval(a, vars)? {
                            Value::Time(t) => t,
                            _ => return Err("`after` needs a time".into()),
                        },
                        None => 0,
                    };
                    if d < 0 {
                        return Err("negative `after` delay".into());
                    }
                    vals.push((x, d));
                }
                collect_assigns(&mut m, target, vals, ty.as_ref(), vars, &mut assigns)?;
                let reports = std::mem::take(&mut view.reports);
                self.handle_reports(reports);
                for (sig, path, wave) in assigns {
                    self.schedule(pi, sig, path, wave, *transport);
                }
                Ok(Step::Next)
            }
            Op::Wait { on, until, timeout, .. } => {
                let mut view = View { sigs: &self.sigs, now: self.now, reports: Vec::new() };
                let mut m = Mach { funcs, host: &mut view, depth: 0 };
                let timeout_at = match timeout {
                    Some(t) => match m.eval(t, vars)? {
                        Value::Time(t) if t >= 0 => Some(self.now + t),
                        _ => return Err("`wait for` needs a non-negative time".into()),
                    },
                    None => None,
                };
                Ok(Step::Suspend(WaitRt { on: on.clone(), until: until.clone(), timeout_at }))
            }
            Op::Builtin { proc_: BuiltinProc::Stop, loc, .. } => {
                let file = self.elab.files.get(loc.file as usize).cloned().unwrap_or_default();
                self.emit(KEvent::Log { time_fs: self.now, sev: 0, text: format!("{file}:{}: simulation stopped (std.env.stop)", loc.line) });
                Ok(Step::Stop)
            }
            Op::Builtin { proc_: BuiltinProc::WriteLine, outs, .. } => {
                let mut view = View { sigs: &self.sigs, now: self.now, reports: Vec::new() };
                let mut m = Mach { funcs, host: &mut view, depth: 0 };
                let text = m.read_target(&outs[0], vars)?.as_text().unwrap_or_default();
                m.assign_var(&outs[0], Value::str(""), Some(&Ty::Line), vars)?;
                self.emit(KEvent::Log { time_fs: self.now, sev: 0, text });
                Ok(Step::Next)
            }
            _ => {
                let mut view = View { sigs: &self.sigs, now: self.now, reports: Vec::new() };
                let mut m = Mach { funcs, host: &mut view, depth: 0 };
                // Branch coverage for conditional jumps.
                if let Op::JumpIfNot(c, _, loc) = op {
                    let v = m.eval(c, vars)?;
                    let t = cond_true(&v)?;
                    let e = self.branch_hits.entry((loc.file, loc.line)).or_insert((0, 0));
                    if t { e.0 += 1 } else { e.1 += 1 }
                    let reports = std::mem::take(&mut view.reports);
                    self.handle_reports(reports);
                    return Ok(match op {
                        Op::JumpIfNot(_, target, _) if !t => Step::Jump(*target),
                        _ => Step::Next,
                    });
                }
                let flow = m.step_common(op, pc, vars)?;
                let reports = std::mem::take(&mut view.reports);
                self.handle_reports(reports);
                match flow {
                    Flow::Next => Ok(Step::Next),
                    Flow::Jump(t) => Ok(Step::Jump(t)),
                    Flow::Return(_) => Err("`return` outside a subprogram".into()),
                    Flow::Unhandled => Err("unsupported statement".into()),
                }
            }
        }
    }

    fn should_wake(&mut self, pi: usize) -> Result<bool, String> {
        let Some(w) = self.procs[pi].wait.clone() else {
            return Ok(false);
        };
        let timed_out = w.timeout_at == Some(self.now);
        let sens = w.on.iter().any(|s| self.sigs[*s].event);
        if timed_out {
            return Ok(true);
        }
        if !sens {
            return Ok(false);
        }
        match &w.until {
            None => Ok(true),
            Some(c) => {
                let mut vars = std::mem::take(&mut self.procs[pi].vars);
                let mut view = View { sigs: &self.sigs, now: self.now, reports: Vec::new() };
                let mut m = Mach { funcs: &self.elab.funcs, host: &mut view, depth: 0 };
                let r = m.eval(c, &mut vars).and_then(|v| cond_true(&v));
                self.procs[pi].vars = vars;
                r
            }
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.as_deref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Run the simulation to completion (stop, failure, quiescence, or the
    /// time limit). `Err` carries a run-time error with its location.
    pub fn run(&mut self) -> Result<(), (Loc, String)> {
        // Time 0: initial values into the trace, bridges' reset outputs,
        // then every process runs until its first wait.
        for id in 0..self.sigs.len() {
            self.sample_vcd(id);
        }
        for bi in 0..self.bridges.len() {
            self.bridge_step(bi);
            self.bridge_outputs(bi, true);
        }
        for pi in 0..self.procs.len() {
            self.run_process(pi)?;
            if self.stopped {
                return Ok(());
            }
        }
        let mut delta: u32 = 0;
        let mut last_progress = 0i64;
        loop {
            if self.stopped {
                return Ok(());
            }
            if self.is_cancelled() {
                self.cancelled = true;
                return Ok(());
            }
            let Some(&t) = self.times.iter().next() else {
                return Ok(()); // quiescent: nothing left to do
            };
            if t > self.now {
                if t > self.max_fs {
                    return Err((Loc::default(), format!(
                        "simulation timed out at {} (limit {}); end the testbench with std.env.stop or stop its clock",
                        time_image(self.now),
                        time_image(self.max_fs)
                    )));
                }
                self.now = t;
                delta = 0;
                if self.now - last_progress >= 1_000_000_000 {
                    last_progress = self.now;
                    self.emit(KEvent::Log { time_fs: self.now, sev: 4, text: String::new() });
                }
            } else {
                delta += 1;
                if delta > DELTA_LIMIT {
                    return Err((Loc::default(), format!("more than {DELTA_LIMIT} delta cycles at {} (combinational loop?)", time_image(self.now))));
                }
            }
            while self.times.iter().next().is_some_and(|x| *x <= self.now) {
                self.times.pop_first();
            }
            for id in std::mem::take(&mut self.evented) {
                self.sigs[id].event = false;
            }
            let changed = self.update_signals();
            self.evented = changed.clone();
            for bi in 0..self.bridges.len() {
                if self.bridge_inputs_changed(bi) {
                    self.bridge_step(bi);
                }
            }
            for pi in 0..self.procs.len() {
                let wake = self.should_wake(pi).map_err(|e| (self.elab.procs[pi].loc, e))?;
                if wake {
                    self.procs[pi].wait = None;
                    self.run_process(pi)?;
                    if self.stopped {
                        return Ok(());
                    }
                }
            }
            // A process waiting with a timeout at a past time cannot wake;
            // drop stale times so they do not spin.
            let now = self.now;
            for p in &mut self.procs {
                if let Some(w) = &mut p.wait {
                    if w.timeout_at.is_some_and(|t| t < now) {
                        w.timeout_at = None;
                    }
                }
            }
        }
    }
}

enum Step {
    Next,
    Jump(usize),
    Suspend(WaitRt),
    Stop,
}

fn op_loc(op: &Op) -> Option<Loc> {
    match op {
        Op::SigAssign { loc, .. }
        | Op::VarAssign { loc, .. }
        | Op::Case { loc, .. }
        | Op::Wait { loc, .. }
        | Op::Report { loc, .. }
        | Op::Assert { loc, .. }
        | Op::ForInit { loc, .. }
        | Op::Builtin { loc, .. }
        | Op::Return(_, loc)
        | Op::JumpIfNot(_, _, loc) => Some(*loc),
        _ => None,
    }
}

fn paths_overlap(a: &[PathSel], b: &[PathSel]) -> bool {
    for (x, y) in a.iter().zip(b.iter()) {
        let disjoint = match (x, y) {
            (PathSel::Pos(p), PathSel::Pos(q)) => p != q,
            (PathSel::Field(p), PathSel::Field(q)) => p != q,
            (PathSel::Pos(p), PathSel::Range(s, n)) | (PathSel::Range(s, n), PathSel::Pos(p)) => *p < *s || *p >= s + n,
            (PathSel::Range(s1, n1), PathSel::Range(s2, n2)) => s1 + n1 <= *s2 || s2 + n2 <= *s1,
            _ => false,
        };
        if disjoint {
            return false;
        }
    }
    true
}

/// Resolve a signal-assignment target into per-signal path + waveform.
fn collect_assigns(
    m: &mut Mach<'_>,
    target: &RTarget,
    wave: Vec<(Value, i64)>,
    ty: Option<&Ty>,
    vars: &mut Vec<Value>,
    out: &mut Vec<(SigId, Vec<PathSel>, Vec<(Value, i64)>)>,
) -> Result<(), String> {
    match target {
        RTarget::Sig(id, sels) => {
            let base = m.host.sig(*id).clone();
            let path = m.resolve_path(&base, sels, vars)?;
            let old = read_path(&base, &path)?;
            let mut w = Vec::with_capacity(wave.len());
            for (v, d) in wave {
                let v = match (ty, path.is_empty()) {
                    (Some(t), true) => coerce(v, t)?,
                    _ => fit_like(v, &old)?,
                };
                w.push((v, d));
            }
            out.push((*id, path, w));
            Ok(())
        }
        RTarget::Agg(parts) => {
            for (k, p) in parts.iter().enumerate() {
                let mut w = Vec::new();
                for (v, d) in &wave {
                    let Value::Arr(a) = v else {
                        return Err("aggregate target needs an array value".into());
                    };
                    w.push((a.elems.get(k).cloned().ok_or("aggregate target length mismatch")?, *d));
                }
                collect_assigns(m, p, w, None, vars, out)?;
            }
            Ok(())
        }
        RTarget::Var(..) => Err("variable target in a signal assignment".into()),
    }
}
