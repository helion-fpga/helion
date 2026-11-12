//! Design hierarchy: packages, the top testbench module, interface
//! instances (virtual interfaces, clocking blocks), and DUT instances
//! simulated on the `helion-sim` netlist kernel.
//!
//! A DUT is any instantiated module that is not defined in the testbench
//! sources; the [`DutProvider`] synthesizes it. Its ports bind to
//! testbench signals (module variables or interface members) by name:
//! writes to a signal bound to a DUT input drive the netlist, a rising
//! edge on the signal bound to the DUT clock port steps it one cycle, and
//! signals bound to DUT outputs are refreshed after every DUT update (so
//! `@(posedge out)` works). Clocking-block inputs are sampled just before
//! the clock edge is applied to the DUT; clocking-block outputs drive in
//! the NBA region.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use super::*;

#[derive(Debug)]
pub struct ClockingInst {
    pub clock: SigId,
    pub edge: Edge,
    pub sigs: HashMap<String, SigId>,
    pub inputs: Vec<String>,
    pub sampled: RefCell<HashMap<String, Value>>,
}

#[derive(Debug)]
pub struct IfaceInst {
    pub name: String,
    pub decl: Rc<ModuleDecl>,
    pub sigs: HashMap<String, SigId>,
    pub clocking: HashMap<String, ClockingInst>,
}

pub struct DutInst {
    pub name: String,
    pub module: String,
    pub src: PathBuf,
    pub sim: helion_sim::Sim,
    pub inputs: Vec<(String, SigId, u32)>,
    pub outputs: Vec<(String, SigId, u32)>,
    pub last_edge_ps: u64,
}

fn is_clock_port(p: &str) -> bool {
    let l = p.to_ascii_lowercase();
    matches!(l.as_str(), "clk" | "clock" | "clk_i" | "i_clk" | "aclk" | "clk_in" | "sys_clk")
}

impl Engine<'_> {
    pub(crate) fn add_signal(&mut self, name: String, ty: Ty, v: Value) -> SigId {
        let id = self.sigs.len();
        let trace = if ty.is_integral() && self.cfg.vcd {
            let n = self.sigs.iter().filter(|s| s.trace.is_some()).count();
            Some(n)
        } else {
            None
        };
        self.sigs.push(Signal { name, ty, v, trace, dut_in: Vec::new() });
        id
    }

    /// Store a new signal value: clocking samples, VCD, DUT drive/step, waiters.
    pub(crate) fn write_signal(&mut self, id: SigId, newv: Value) -> Result<(), SvError> {
        let old = self.sigs[id].v.clone();
        if old == newv && old.as_u64() == newv.as_u64() {
            return Ok(());
        }
        let (ob, nb) = (old.as_u64(), newv.as_u64());
        if let Some(cbs) = self.clock_watch.get(&id).cloned() {
            for (iface, cb) in cbs {
                let c = &iface.clocking[&cb];
                let edge = match c.edge {
                    Edge::Pos => ob & 1 == 0 && nb & 1 == 1,
                    Edge::Neg => ob & 1 == 1 && nb & 1 == 0,
                    Edge::Any => ob & 1 != nb & 1,
                };
                if edge {
                    let mut s = c.sampled.borrow_mut();
                    for n in &c.inputs {
                        if let Some(&sid) = c.sigs.get(n) {
                            s.insert(n.clone(), self.sigs[sid].v.clone());
                        }
                    }
                }
            }
        }
        self.sigs[id].v = newv;
        self.dirty += 1;
        if let Some(t) = self.sigs[id].trace {
            self.res.samples.push((self.time, t, nb));
        }
        let binds = self.sigs[id].dut_in.clone();
        for (d, port, is_clk) in binds {
            if is_clk {
                if ob & 1 == 0 && nb & 1 == 1 {
                    let dt = self.time.saturating_sub(self.duts[d].last_edge_ps).max(1);
                    self.duts[d].last_edge_ps = self.time;
                    self.duts[d].sim.step_posedge(dt);
                    self.refresh_outputs(d)?;
                }
            } else {
                let w = self.sigs[id].ty.width();
                self.drive_dut(d, &port, nb, w)?;
                self.refresh_outputs(d)?;
            }
        }
        self.notify_sig(id, ob, nb);
        Ok(())
    }

    fn drive_dut(&mut self, d: usize, port: &str, v: u64, w: u32) -> Result<(), SvError> {
        for i in 0..w {
            let net = if w == 1 { port.to_string() } else { format!("{port}_{i}") };
            self.duts[d].sim.drive_input(&net, (v >> i) & 1 == 1).map_err(|e| self.rt(format!("driving DUT port `{port}`: {e}")))?;
        }
        Ok(())
    }

    fn sample_dut(&self, d: usize, port: &str, w: u32) -> u64 {
        let mut v = 0u64;
        for i in 0..w {
            let net = if w == 1 { port.to_string() } else { format!("{port}_{i}") };
            if let Some(b) = self.duts[d].sim.sample(&net) {
                v |= (b & 1) << i;
            }
        }
        v
    }

    fn refresh_outputs(&mut self, d: usize) -> Result<(), SvError> {
        let outs = self.duts[d].outputs.clone();
        for (port, sid, w) in outs {
            let v = self.sample_dut(d, &port, w);
            let ty = self.sigs[sid].ty.clone();
            self.write_signal(sid, convert(Value::Int(v, w, false), &ty))?;
        }
        Ok(())
    }

    /// Signal referenced by a port-connection expression.
    fn sig_of(&mut self, ctx: &Ctx, e: &Expr) -> Option<SigId> {
        match self.lpath(ctx, e) {
            Ok(LPath { base: Place::Sig(id), sels }) if sels.is_empty() => Some(id),
            _ => None,
        }
    }

    /// Elaborate packages, `$unit`, and the top module; start processes.
    pub fn elaborate(&mut self, provider: &mut dyn DutProvider) -> Result<(), SvError> {
        let unit = self.unit.clone();
        let u = self.add_ns("$unit", NsKind::Unit, None);
        for m in &unit.modules {
            self.modules.insert(m.name.clone(), m.clone());
        }
        let mut pkg_ids = Vec::new();
        for p in &unit.packages {
            let id = self.add_ns(&p.name, NsKind::Package, Some(u));
            self.pkgs.insert(p.name.clone(), id);
            pkg_ids.push(id);
        }
        for (p, &id) in unit.packages.iter().zip(&pkg_ids) {
            self.register_items(id, &p.items);
        }
        self.register_items(u, &unit.unit);
        for (p, &id) in unit.packages.iter().zip(&pkg_ids) {
            self.cur_loc = p.loc;
            self.init_ns_items(id, &p.items)?;
            self.elab_classes(id)?;
        }
        self.init_ns_items(u, &unit.unit)?;
        self.elab_classes(u)?;
        // Top module.
        let top = match &self.cfg.top {
            Some(t) => self.modules.get(t).cloned().ok_or_else(|| SvError::new("", 0, format!("top module `{t}` not found")))?,
            None => {
                let mut inst = std::collections::HashSet::new();
                for m in &unit.modules {
                    for it in &m.items {
                        if let ModItem::Inst(i) = it {
                            inst.insert(i.module.clone());
                        }
                    }
                }
                let tops: Vec<Rc<ModuleDecl>> = unit.modules.iter().filter(|m| !m.is_interface && !inst.contains(&m.name)).cloned().collect();
                match tops.len() {
                    1 => tops[0].clone(),
                    0 => return Err(SvError::new("", 0, "no top-level testbench module found")),
                    _ => {
                        let names: Vec<&str> = tops.iter().map(|m| m.name.as_str()).collect();
                        return Err(SvError::new("", 0, format!("several top-level modules ({}); pick one with the top option", names.join(", "))));
                    }
                }
            }
        };
        self.res.top_file = top.loc.file;
        self.elab_module(&top, u, provider)?;
        // Time-0 VCD samples.
        for s in &self.sigs {
            if let Some(t) = s.trace {
                self.res.samples.push((0, t, s.v.as_u64()));
            }
        }
        Ok(())
    }

    fn elab_module(&mut self, m: &Rc<ModuleDecl>, parent: NsId, provider: &mut dyn DutProvider) -> Result<(), SvError> {
        let mut ns = Namespace::new(&m.name, NsKind::Module, Some(parent));
        for it in &m.items {
            match it {
                ModItem::Sub(s) => {
                    ns.subs.insert(s.name.clone(), s.clone());
                }
                ModItem::Class(c) => {
                    ns.classes.insert(c.name.clone(), c.clone());
                }
                ModItem::Typedef(t) if t.ty.is_some() => {
                    ns.typedefs.insert(t.name.clone(), t.clone());
                }
                ModItem::Import(p, item) => {
                    let pid = *self.pkgs.get(p).ok_or_else(|| self.err_at(m.loc, format!("unknown package `{p}`")))?;
                    match item {
                        None => ns.imports.borrow_mut().push(pid),
                        Some(i) => ns.item_imports.borrow_mut().push((pid, i.clone())),
                    }
                }
                _ => {}
            }
        }
        self.nss.push(ns);
        let mid = self.nss.len() - 1;
        let ctx = Ctx { scopes: vec![], this: None, class: None, ns: mid };
        let prefix = m.name.clone();
        // Parameters, enum typedef constants.
        for it in &m.items {
            match it {
                ModItem::Param(p) => {
                    let (ty, v) = self.param_value(&ctx, p)?;
                    match v {
                        Some(v) => {
                            self.nss[mid].vars.borrow_mut().insert(p.name.clone(), Var { ty, v });
                        }
                        None => {
                            self.nss[mid].ty_cache.borrow_mut().insert(p.name.clone(), ty);
                        }
                    }
                }
                ModItem::Typedef(t) => {
                    if let Some(DataType::Enum { .. }) = &t.ty {
                        let ty = self.resolve_type(ctx.tyscope(), t.ty.as_ref().unwrap(), &t.udims, Some(&t.name))?;
                        if let Ty::Enum(e) = &ty {
                            for (n, v) in &e.items {
                                self.nss[mid].vars.borrow_mut().insert(n.clone(), Var { ty: ty.clone(), v: Value::Int(*v, e.w, false) });
                            }
                        }
                        self.nss[mid].ty_cache.borrow_mut().insert(t.name.clone(), ty);
                    }
                }
                _ => {}
            }
        }
        // Signals (module variables). Classes / events / non-integral
        // variables live in the namespace instead.
        for it in &m.items {
            if let ModItem::Var(d) = it {
                self.cur_loc = d.loc;
                let ty = self.resolve_type(ctx.tyscope(), &d.ty, &d.udims, None)?;
                let v = match &d.init {
                    Some(e) => self.eval_to(&ctx, e, &ty)?,
                    None => ty.default_value(),
                };
                if ty.is_integral() {
                    let id = self.add_signal(format!("{prefix}.{}", d.name), ty, v);
                    self.nss[mid].sigs.borrow_mut().insert(d.name.clone(), id);
                } else {
                    self.nss[mid].vars.borrow_mut().insert(d.name.clone(), Var { ty, v });
                }
            }
        }
        // Interface instances.
        for it in &m.items {
            if let ModItem::Inst(inst) = it {
                let Some(decl) = self.modules.get(&inst.module).cloned() else { continue };
                if !decl.is_interface {
                    return Err(self.err_at(inst.loc, format!("testbench module `{}` instantiated inside `{}`: nested testbench modules are not supported", inst.module, m.name)));
                }
                self.cur_loc = inst.loc;
                let ii = self.elab_iface(&ctx, &prefix, inst, decl)?;
                self.nss[mid].ifaces.borrow_mut().insert(inst.name.clone(), ii);
            }
        }
        // DUT instances.
        for it in &m.items {
            if let ModItem::Inst(inst) = it {
                if self.modules.contains_key(&inst.module) {
                    continue;
                }
                self.cur_loc = inst.loc;
                self.elab_dut(&ctx, inst, provider)?;
            }
        }
        self.elab_classes(mid)?;
        // Processes.
        for it in &m.items {
            match it {
                ModItem::Initial(body, loc) => {
                    let exec = ExecState {
                        frames: vec![Frame::Seq { body: body.clone(), pc: 0, scoped: false }],
                        ctxs: vec![Ctx { scopes: vec![Rc::new(RefCell::new(HashMap::new()))], this: None, class: None, ns: mid }],
                        retval: None,
                    };
                    self.spawn(format!("{prefix}.initial@{}", loc.line), exec, None, None);
                }
                ModItem::Always(kind, body, loc) => {
                    if matches!(kind, AlwaysKind::Comb | AlwaysKind::Latch) {
                        return Err(self.err_at(*loc, "always_comb/always_latch in a testbench module is not supported; use `assign` or `always @(…)`"));
                    }
                    let fb: Body = Rc::from(vec![S { loc: *loc, k: SK::Forever(body.clone()) }]);
                    let exec = ExecState {
                        frames: vec![Frame::Seq { body: fb, pc: 0, scoped: false }],
                        ctxs: vec![Ctx { scopes: vec![Rc::new(RefCell::new(HashMap::new()))], this: None, class: None, ns: mid }],
                        retval: None,
                    };
                    self.spawn(format!("{prefix}.always@{}", loc.line), exec, None, None);
                }
                ModItem::Assign(l, r, loc) => {
                    let mut terms = Vec::new();
                    self.collect_sig_refs(&ctx, r, &mut terms);
                    let asg = S { loc: *loc, k: SK::Assign { lhs: l.clone(), op: "=", rhs: r.clone(), nb: false } };
                    let body: Body = if terms.is_empty() {
                        Rc::from(vec![asg])
                    } else {
                        let wait = S { loc: *loc, k: SK::Event(terms, Rc::from(vec![])) };
                        Rc::from(vec![S { loc: *loc, k: SK::Forever(Rc::from(vec![asg, wait])) }])
                    };
                    let exec = ExecState {
                        frames: vec![Frame::Seq { body, pc: 0, scoped: false }],
                        ctxs: vec![Ctx { scopes: vec![], this: None, class: None, ns: mid }],
                        retval: None,
                    };
                    self.spawn(format!("{prefix}.assign@{}", loc.line), exec, None, None);
                }
                ModItem::Final(body, _) => self.final_blocks.push((body.clone(), mid)),
                ModItem::Clocking(_) => {
                    return Err(self.err_at(m.loc, "clocking blocks are supported inside interfaces (use a virtual interface)"));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn collect_sig_refs(&mut self, ctx: &Ctx, e: &Expr, out: &mut Vec<EvTerm>) {
        match e {
            Expr::Ident(_) | Expr::Member(..) => {
                if let Some(_id) = self.sig_of(ctx, e) {
                    out.push(EvTerm { edge: None, e: e.clone() });
                    return;
                }
                if let Expr::Member(b, _) = e {
                    self.collect_sig_refs(ctx, b, out);
                }
            }
            Expr::Index(b, i) | Expr::Binary(_, b, i) => {
                self.collect_sig_refs(ctx, b, out);
                self.collect_sig_refs(ctx, i, out);
            }
            Expr::Slice(b, _, _, _) | Expr::Unary(_, b) | Expr::Cast(_, b) => self.collect_sig_refs(ctx, b, out),
            Expr::Cond(a, b, c) => {
                self.collect_sig_refs(ctx, a, out);
                self.collect_sig_refs(ctx, b, out);
                self.collect_sig_refs(ctx, c, out);
            }
            Expr::Concat(p) => {
                for x in p {
                    self.collect_sig_refs(ctx, x, out);
                }
            }
            _ => {}
        }
    }

    fn elab_iface(&mut self, ctx: &Ctx, prefix: &str, inst: &Instance, decl: Rc<ModuleDecl>) -> Result<Rc<IfaceInst>, SvError> {
        let name = format!("{prefix}.{}", inst.name);
        let mut sigs: HashMap<String, SigId> = HashMap::new();
        let ictx = Ctx { scopes: vec![], this: None, class: None, ns: ctx.ns };
        for (k, p) in decl.ports.iter().enumerate() {
            let actual = inst
                .conns
                .iter()
                .find(|(n, _)| n.as_deref() == Some(p.name.as_str()))
                .map(|(_, e)| e.clone())
                .or_else(|| inst.conns.get(k).filter(|(n, _)| n.is_none()).map(|(_, e)| e.clone()))
                .flatten();
            let id = match actual.as_ref().and_then(|e| self.sig_of(ctx, e)) {
                Some(id) => id,
                None => {
                    let ty = self.resolve_type(ictx.tyscope(), &p.ty, &[], None)?;
                    let v = match &actual {
                        Some(e) => self.eval_to(ctx, e, &ty)?,
                        None => ty.default_value(),
                    };
                    self.add_signal(format!("{name}.{}", p.name), ty, v)
                }
            };
            sigs.insert(p.name.clone(), id);
        }
        for it in &decl.items {
            if let ModItem::Var(d) = it {
                self.cur_loc = d.loc;
                let ty = self.resolve_type(ictx.tyscope(), &d.ty, &d.udims, None)?;
                if !ty.is_integral() {
                    return Err(self.err_at(d.loc, "interface members must be integral signals"));
                }
                let v = match &d.init {
                    Some(e) => self.eval_to(&ictx, e, &ty)?,
                    None => ty.default_value(),
                };
                let id = self.add_signal(format!("{name}.{}", d.name), ty, v);
                sigs.insert(d.name.clone(), id);
            }
        }
        let mut clocking = HashMap::new();
        for it in &decl.items {
            if let ModItem::Clocking(c) = it {
                let t = c.event.first().ok_or_else(|| self.err_at(decl.loc, "clocking block without an event"))?;
                let clk_name = match &t.e {
                    Expr::Ident(n) => n.clone(),
                    _ => return Err(self.err_at(decl.loc, "clocking event must be a signal of the interface")),
                };
                let clock = *sigs.get(&clk_name).ok_or_else(|| self.err_at(decl.loc, format!("clocking event signal `{clk_name}` is not an interface signal")))?;
                let mut cs = HashMap::new();
                let mut inputs = Vec::new();
                for (n, is_in, _) in &c.signals {
                    let id = *sigs.get(n).ok_or_else(|| self.err_at(decl.loc, format!("clocking signal `{n}` is not an interface signal")))?;
                    cs.insert(n.clone(), id);
                    if *is_in {
                        inputs.push(n.clone());
                    }
                }
                clocking.insert(c.name.clone(), ClockingInst { clock, edge: t.edge.unwrap_or(Edge::Pos), sigs: cs, inputs, sampled: RefCell::new(HashMap::new()) });
            }
            if let ModItem::Sub(s) = it {
                return Err(self.err_at(s.loc, "tasks/functions inside interfaces are not supported; put them in a class"));
            }
        }
        let ii = Rc::new(IfaceInst { name, decl, sigs, clocking });
        for (cb, c) in &ii.clocking {
            self.clock_watch.entry(c.clock).or_default().push((ii.clone(), cb.clone()));
        }
        Ok(ii)
    }

    fn elab_dut(&mut self, ctx: &Ctx, inst: &Instance, provider: &mut dyn DutProvider) -> Result<(), SvError> {
        let (design, src) = provider.synth(&inst.module).map_err(|e| self.err_at(inst.loc, format!("DUT `{}`: {e}", inst.module)))?;
        let sim = helion_sim::Sim::new(&design);
        let ins = sim.input_ports();
        let outs = sim.output_ports();
        let d = self.duts.len();
        self.duts.push(DutInst { name: inst.name.clone(), module: inst.module.clone(), src, sim, inputs: vec![], outputs: vec![], last_edge_ps: 0 });
        let mut conns: Vec<(String, Option<Expr>)> = Vec::new();
        for (p, e) in &inst.conns {
            match p.as_deref() {
                Some(".*") => {
                    for port in ins.iter().chain(outs.iter()) {
                        if !conns.iter().any(|(n, _)| n == port) {
                            conns.push((port.clone(), Some(Expr::Ident(port.clone()))));
                        }
                    }
                }
                Some(n) => conns.push((n.to_string(), e.clone())),
                None => return Err(self.err_at(inst.loc, format!("DUT `{}`: connect ports by name (`.port(signal)`)", inst.module))),
            }
        }
        let mut clock_bound = false;
        for (port, e) in conns {
            let Some(e) = e else { continue };
            let is_in = ins.contains(&port);
            let is_out = outs.contains(&port);
            if !is_in && !is_out {
                let mut all: Vec<&str> = ins.iter().chain(outs.iter()).map(String::as_str).collect();
                all.sort_unstable();
                return Err(self.err_at(inst.loc, format!("DUT `{}` has no port `{port}` (ports: {})", inst.module, all.join(", "))));
            }
            match self.sig_of(ctx, &e) {
                Some(sid) => {
                    let w = self.sigs[sid].ty.width();
                    if is_in {
                        let clk = is_clock_port(&port) && !clock_bound;
                        clock_bound |= clk;
                        self.sigs[sid].dut_in.push((d, port.clone(), clk));
                        self.duts[d].inputs.push((port.clone(), sid, w));
                        if !clk {
                            let v = self.sigs[sid].v.as_u64();
                            self.drive_dut(d, &port, v, w)?;
                        }
                    } else {
                        self.duts[d].outputs.push((port.clone(), sid, w));
                    }
                }
                None => {
                    if is_out {
                        return Err(self.err_at(inst.loc, format!("DUT output `{port}` must connect to a testbench signal")));
                    }
                    let v = self.eval(ctx, &e)?;
                    let w = v.width().min(64);
                    self.drive_dut(d, &port, v.as_u64(), w)?;
                }
            }
        }
        self.refresh_outputs(d)?;
        Ok(())
    }
}
