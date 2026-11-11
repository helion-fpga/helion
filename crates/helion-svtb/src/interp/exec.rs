//! Statement execution on an explicit frame stack.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::call::{Recv, Resolved};
use super::*;

impl Engine<'_> {
    fn hit(&mut self, loc: Loc) {
        *self.res.line_hits.entry((loc.file, loc.line)).or_insert(0) += 1;
    }

    fn branch(&mut self, loc: Loc, taken: bool, kind: &'static str) {
        let e = self.res.branches.entry((loc.file, loc.line)).or_insert((0, 0, kind));
        if taken {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
    }

    fn top_ctx(st: &ExecState) -> &Ctx {
        st.ctxs.last().expect("execution context")
    }

    fn push_scope(st: &mut ExecState) {
        st.ctxs.last_mut().expect("ctx").scopes.push(Rc::new(RefCell::new(HashMap::new())));
    }

    fn pop_scope(st: &mut ExecState) {
        st.ctxs.last_mut().expect("ctx").scopes.pop();
    }

    fn cur_pid(&self, sync: bool, what: &str) -> Result<Pid, SvError> {
        if sync {
            return Err(self.rt(format!("{what} in a function (functions cannot consume time; make it a task)")));
        }
        self.cur.ok_or_else(|| self.rt(format!("{what} outside a process")))
    }

    /// Run until the state blocks (`Ok(false)`) or its frames are exhausted
    /// (`Ok(true)`).
    pub(crate) fn step(&mut self, st: &mut ExecState, sync: bool) -> Result<bool, SvError> {
        loop {
            if self.finished {
                return Ok(sync);
            }
            if self.slice_budget == 0 {
                return Err(self.rt(format!(
                    "a process ran {} statements without suspending (zero-delay infinite loop?)",
                    self.cfg.max_slice
                )));
            }
            self.slice_budget -= 1;
            let Some(top) = st.frames.last_mut() else { return Ok(true) };
            match top {
                Frame::Seq { body, pc, scoped } => {
                    if *pc >= body.len() {
                        let sc = *scoped;
                        st.frames.pop();
                        if sc {
                            Self::pop_scope(st);
                        }
                        continue;
                    }
                    let body = body.clone();
                    let i = *pc;
                    *pc += 1;
                    match self.exec(st, &body, i, sync)? {
                        Step::Go => {}
                        Step::Block => return Ok(false),
                        Step::Retry => {
                            if let Some(Frame::Seq { pc, .. }) = st.frames.last_mut() {
                                *pc -= 1;
                            }
                            return Ok(false);
                        }
                    }
                }
                Frame::Loop { .. } => self.loop_step(st)?,
                Frame::Call { .. } => self.finish_call(st, None)?,
            }
        }
    }

    fn finish_call(&mut self, st: &mut ExecState, val: Option<Value>) -> Result<(), SvError> {
        let Some(Frame::Call { outs, ret, func }) = st.frames.pop() else { unreachable!() };
        let ctx = st.ctxs.pop().expect("callee ctx");
        let args = ctx.scopes.first().cloned();
        for (p, name) in outs {
            let v = args.as_ref().and_then(|s| s.borrow().get(&name).map(|x| x.v.clone()));
            if let Some(v) = v {
                self.write_path(&p, v)?;
            }
        }
        st.retval = match ret {
            Some(ty) => {
                let v = match val {
                    Some(v) => v,
                    None => args.as_ref().and_then(|s| s.borrow().get(&func).map(|x| x.v.clone())).unwrap_or_else(|| ty.default_value()),
                };
                Some(convert(v, &ty))
            }
            None => None,
        };
        Ok(())
    }

    fn loop_step(&mut self, st: &mut ExecState) -> Result<(), SvError> {
        let Some(Frame::Loop { owner, idx, st: ls, scoped }) = st.frames.last_mut() else { unreachable!() };
        let owner = owner.clone();
        let idx = *idx;
        let scoped = *scoped;
        let s = &owner[idx];
        self.cur_loc = s.loc;
        let mut ls2 = ls.clone();
        let (go, body): (bool, Option<Body>) = match (&mut ls2, &s.k) {
            (LoopSt::While, SK::While(c, b)) => {
                let ok = self.eval(Self::top_ctx(st), c)?.truthy();
                (ok, Some(b.clone()))
            }
            (LoopSt::DoWhile { first }, SK::DoWhile(b, c)) => {
                let ok = if *first {
                    *first = false;
                    true
                } else {
                    self.eval(Self::top_ctx(st), c)?.truthy()
                };
                (ok, Some(b.clone()))
            }
            (LoopSt::For { started }, SK::For { cond, step, body, .. }) => {
                if *started {
                    for i in 0..step.len() {
                        match self.exec(st, step, i, true)? {
                            Step::Go => {}
                            _ => return Err(self.rt("for-loop step cannot block")),
                        }
                    }
                }
                *started = true;
                let ok = match cond {
                    Some(c) => self.eval(Self::top_ctx(st), c)?.truthy(),
                    None => true,
                };
                (ok, Some(body.clone()))
            }
            (LoopSt::Repeat { left }, SK::Repeat(_, b)) => {
                if *left > 0 {
                    *left -= 1;
                    (true, Some(b.clone()))
                } else {
                    (false, None)
                }
            }
            (LoopSt::Forever, SK::Forever(b)) => (true, Some(b.clone())),
            (LoopSt::Foreach { tuples, idx: k, vars }, SK::Foreach { body, .. }) => {
                if *k < tuples.len() {
                    let t = tuples[*k].clone();
                    *k += 1;
                    let scope = Self::top_ctx(st).scopes.last().cloned().expect("foreach scope");
                    for (v, val) in vars.iter().zip(t) {
                        if let Some(v) = v {
                            let mut sb = scope.borrow_mut();
                            let ty = match &val {
                                Value::Str(_) => Ty::Str,
                                _ => Ty::int(32, true),
                            };
                            sb.insert(v.clone(), Var { ty: ty.clone(), v: convert(val, &ty) });
                        }
                    }
                    (true, Some(body.clone()))
                } else {
                    (false, None)
                }
            }
            _ => return Err(self.rt("internal: loop frame mismatch")),
        };
        if let Some(Frame::Loop { st: ls, .. }) = st.frames.last_mut() {
            *ls = ls2;
        }
        if go {
            st.frames.push(Frame::Seq { body: body.unwrap(), pc: 0, scoped: false });
        } else {
            st.frames.pop();
            if scoped {
                Self::pop_scope(st);
            }
        }
        Ok(())
    }

    /// Pop frames up to the innermost loop (`brk`: also pop the loop).
    fn unwind_loop(&mut self, st: &mut ExecState, brk: bool) -> Result<(), SvError> {
        loop {
            match st.frames.last() {
                Some(Frame::Seq { scoped, .. }) => {
                    let sc = *scoped;
                    st.frames.pop();
                    if sc {
                        Self::pop_scope(st);
                    }
                }
                Some(Frame::Loop { scoped, .. }) => {
                    if brk {
                        let sc = *scoped;
                        st.frames.pop();
                        if sc {
                            Self::pop_scope(st);
                        }
                    }
                    return Ok(());
                }
                _ => return Err(self.rt(if brk { "`break` outside a loop" } else { "`continue` outside a loop" })),
            }
        }
    }

    pub(crate) fn declare_local(&mut self, ctx: &Ctx, d: &VarDecl) -> Result<(), SvError> {
        if let DataType::Enum { .. } = &d.ty {
            let ty = self.resolve_type(ctx.tyscope(), &d.ty, &[], None)?;
            if let Ty::Enum(e) = &ty {
                if let Some(s) = ctx.scopes.last() {
                    for (n, v) in &e.items {
                        s.borrow_mut().insert(n.clone(), Var { ty: ty.clone(), v: Value::Int(*v, e.w, false) });
                    }
                }
            }
        }
        let ty = self.resolve_type(ctx.tyscope(), &d.ty, &d.udims, None)?;
        let v = match &d.init {
            Some(e) => self.eval_to(ctx, e, &ty)?,
            None => ty.default_value(),
        };
        let scope = ctx.scopes.last().ok_or_else(|| self.rt("declaration outside a block"))?;
        scope.borrow_mut().insert(d.name.clone(), Var { ty, v });
        Ok(())
    }

    /// Delay expression in picoseconds (`#5` = 5 ns, `#2.5`, `#10ps`).
    pub(crate) fn delay_ps(&mut self, ctx: &Ctx, e: &Expr) -> Result<u64, SvError> {
        if let Expr::Time(ps) = e {
            return Ok(*ps);
        }
        let v = self.eval(ctx, e)?;
        Ok(match v {
            Value::Real(r) => (r * 1000.0).round().max(0.0) as u64,
            other => (other.as_i64().max(0) as u64).saturating_mul(1000),
        })
    }

    fn exec(&mut self, st: &mut ExecState, body: &Body, i: usize, sync: bool) -> Result<Step, SvError> {
        let s = &body[i];
        self.cur_loc = s.loc;
        self.hit(s.loc);
        match &s.k {
            SK::Null => Ok(Step::Go),
            SK::Block(b) => {
                Self::push_scope(st);
                st.frames.push(Frame::Seq { body: b.clone(), pc: 0, scoped: true });
                Ok(Step::Go)
            }
            SK::Decl(d) => {
                if st.ctxs.last().is_some_and(|c| c.scopes.is_empty()) {
                    Self::push_scope(st);
                }
                let ctx = Self::top_ctx(st).clone();
                self.declare_local(&ctx, d)?;
                Ok(Step::Go)
            }
            SK::Assign { lhs, op, rhs, nb } => {
                let ctx = Self::top_ctx(st);
                if *nb {
                    let p = self.lpath(ctx, lhs)?;
                    let (_, ty) = self.read_path(&p)?;
                    let v = self.eval_to(ctx, rhs, &ty)?;
                    self.nba.push((p, v));
                    return Ok(Step::Go);
                }
                if *op == "=" {
                    // Task-call results are not expressions; plain assign.
                    let ctx = ctx.clone();
                    self.assign(&ctx, lhs, rhs)?;
                } else {
                    let bop = match *op {
                        "+=" => BinOp::Add,
                        "-=" => BinOp::Sub,
                        "*=" => BinOp::Mul,
                        "/=" => BinOp::Div,
                        "%=" => BinOp::Mod,
                        "&=" => BinOp::And,
                        "|=" => BinOp::Or,
                        "^=" => BinOp::Xor,
                        "<<=" => BinOp::Shl,
                        ">>=" => BinOp::Shr,
                        "<<<=" => BinOp::AShl,
                        _ => BinOp::AShr,
                    };
                    let ctx = ctx.clone();
                    let p = self.lpath(&ctx, lhs)?;
                    let (old, _) = self.read_path(&p)?;
                    let r = self.eval(&ctx, rhs)?;
                    let nv = if matches!(old, Value::Str(_)) && bop == BinOp::Add {
                        Value::Str(old.as_str() + &r.as_str())
                    } else {
                        self.binary(bop, old, r)?
                    };
                    self.write_path(&p, nv)?;
                }
                Ok(Step::Go)
            }
            SK::IncDec { e, inc } => {
                let ctx = Self::top_ctx(st).clone();
                self.eval(&ctx, &Expr::IncDec { e: Box::new(e.clone()), inc: *inc, prefix: true })?;
                Ok(Step::Go)
            }
            SK::Expr(e) => self.exec_expr_stmt(st, e, sync),
            SK::If { cond, then, els } => {
                let c = self.eval(Self::top_ctx(st), cond)?.truthy();
                self.branch(s.loc, c, "if");
                if c {
                    st.frames.push(Frame::Seq { body: then.clone(), pc: 0, scoped: false });
                } else if let Some(e) = els {
                    st.frames.push(Frame::Seq { body: e.clone(), pc: 0, scoped: false });
                }
                Ok(Step::Go)
            }
            SK::Case { kind, e, items, default, .. } => {
                let ctx = Self::top_ctx(st).clone();
                let v = self.eval(&ctx, e)?;
                let _ = kind;
                for (labels, b) in items {
                    for l in labels {
                        if self.inside_item(&ctx, &v, l)? {
                            st.frames.push(Frame::Seq { body: b.clone(), pc: 0, scoped: false });
                            return Ok(Step::Go);
                        }
                    }
                }
                if let Some(d) = default {
                    st.frames.push(Frame::Seq { body: d.clone(), pc: 0, scoped: false });
                }
                Ok(Step::Go)
            }
            SK::For { init, .. } => {
                Self::push_scope(st);
                for k in 0..init.len() {
                    self.exec(st, init, k, true)?;
                }
                st.frames.push(Frame::Loop { owner: body.clone(), idx: i, st: LoopSt::For { started: false }, scoped: true });
                Ok(Step::Go)
            }
            SK::Foreach { arr, vars, .. } => {
                let ctx = Self::top_ctx(st).clone();
                let v = self.eval(&ctx, arr)?;
                let mut tuples: Vec<Vec<Value>> = Vec::new();
                Self::foreach_tuples(&v, vars.len(), &mut Vec::new(), &mut tuples);
                Self::push_scope(st);
                st.frames.push(Frame::Loop {
                    owner: body.clone(),
                    idx: i,
                    st: LoopSt::Foreach { tuples: Rc::new(tuples), idx: 0, vars: vars.clone() },
                    scoped: true,
                });
                Ok(Step::Go)
            }
            SK::While(..) => {
                st.frames.push(Frame::Loop { owner: body.clone(), idx: i, st: LoopSt::While, scoped: false });
                Ok(Step::Go)
            }
            SK::DoWhile(..) => {
                st.frames.push(Frame::Loop { owner: body.clone(), idx: i, st: LoopSt::DoWhile { first: true }, scoped: false });
                Ok(Step::Go)
            }
            SK::Repeat(n, _) => {
                let n = self.eval(Self::top_ctx(st), n)?.as_i64().max(0) as u64;
                st.frames.push(Frame::Loop { owner: body.clone(), idx: i, st: LoopSt::Repeat { left: n }, scoped: false });
                Ok(Step::Go)
            }
            SK::Forever(_) => {
                st.frames.push(Frame::Loop { owner: body.clone(), idx: i, st: LoopSt::Forever, scoped: false });
                Ok(Step::Go)
            }
            SK::Fork { kind, decls, branches } => {
                let pid = self.cur_pid(sync && *kind != JoinKind::None, "fork/join")?;
                let _ = pid;
                let pid = self.cur.ok_or_else(|| self.rt("fork outside a process"))?;
                let mut ctx = Self::top_ctx(st).clone();
                let scope: ScopeRef = Rc::new(RefCell::new(HashMap::new()));
                ctx.scopes.push(scope);
                for d in decls.iter() {
                    if let SK::Decl(vd) = &d.k {
                        self.declare_local(&ctx, vd)?;
                    }
                }
                if branches.is_empty() {
                    return Ok(Step::Go);
                }
                let gid = self.forks.len();
                self.forks.push(ForkGroup { parent: pid, remaining: branches.len(), kind: *kind, done: false });
                let pname = self.procs[pid].name.clone();
                for (k, b) in branches.iter().enumerate() {
                    let exec = ExecState { frames: vec![Frame::Seq { body: b.clone(), pc: 0, scoped: false }], ctxs: vec![ctx.clone()], retval: None };
                    self.spawn(format!("{pname}.fork{gid}.{k}"), exec, Some(pid), Some(gid));
                }
                if *kind == JoinKind::None {
                    return Ok(Step::Go);
                }
                let epoch = self.block(pid);
                self.wait_on(pid, epoch, WaitKey::Join(gid), None);
                Ok(Step::Block)
            }
            SK::Delay(d, b) => {
                let pid = self.cur_pid(sync, "#delay")?;
                let dt = self.delay_ps(Self::top_ctx(st), d)?;
                st.frames.push(Frame::Seq { body: b.clone(), pc: 0, scoped: false });
                let epoch = self.block(pid);
                self.wait_time(pid, epoch, dt);
                Ok(Step::Block)
            }
            SK::Event(terms, b) => {
                let pid = self.cur_pid(sync, "@ event control")?;
                let ctx = Self::top_ctx(st).clone();
                let mut keys = Vec::new();
                for t in terms {
                    keys.push(self.event_key(&ctx, t)?);
                }
                st.frames.push(Frame::Seq { body: b.clone(), pc: 0, scoped: false });
                let epoch = self.block(pid);
                for (k, e) in keys {
                    self.wait_on(pid, epoch, k, e);
                }
                Ok(Step::Block)
            }
            SK::Wait(c, b) => {
                if self.eval(Self::top_ctx(st), c)?.truthy() {
                    st.frames.push(Frame::Seq { body: b.clone(), pc: 0, scoped: false });
                    return Ok(Step::Go);
                }
                let pid = self.cur_pid(sync, "wait()")?;
                let epoch = self.block(pid);
                self.wait_cond(pid, epoch);
                Ok(Step::Retry)
            }
            SK::WaitFork => {
                let pid = self.cur_pid(sync, "wait fork")?;
                if !self.has_live_children(pid) {
                    return Ok(Step::Go);
                }
                let epoch = self.block(pid);
                self.wait_on(pid, epoch, WaitKey::Children(pid), None);
                Ok(Step::Retry)
            }
            SK::DisableFork => {
                if let Some(pid) = self.cur {
                    for c in self.procs[pid].children.clone() {
                        self.kill(c);
                    }
                }
                Ok(Step::Go)
            }
            SK::Disable(n) => Err(self.rt(format!("`disable {n}` (named blocks) is not supported; use `disable fork`"))),
            SK::Trigger(e) => {
                let v = self.eval(Self::top_ctx(st), e)?;
                match v {
                    Value::Event(ev) => {
                        ev.last.set(Some(self.time));
                        self.dirty += 1;
                        self.notify(&WaitKey::Event(ev.id));
                        Ok(Step::Go)
                    }
                    _ => Err(self.rt("`->` needs an event")),
                }
            }
            SK::Return(e) => {
                let v = match e {
                    Some(x) => {
                        let ctx = Self::top_ctx(st).clone();
                        let ret_ty = st.frames.iter().rev().find_map(|f| match f {
                            Frame::Call { ret, .. } => Some(ret.clone()),
                            _ => None,
                        });
                        Some(match ret_ty.flatten() {
                            Some(t) => self.eval_to(&ctx, x, &t)?,
                            None => self.eval(&ctx, x)?,
                        })
                    }
                    None => None,
                };
                loop {
                    match st.frames.last() {
                        Some(Frame::Call { .. }) => {
                            self.finish_call(st, v)?;
                            return Ok(Step::Go);
                        }
                        Some(_) => {
                            st.frames.pop();
                        }
                        None => return Ok(Step::Go),
                    }
                }
            }
            SK::Break => {
                self.unwind_loop(st, true)?;
                Ok(Step::Go)
            }
            SK::Continue => {
                self.unwind_loop(st, false)?;
                Ok(Step::Go)
            }
            SK::Assert { cond, pass, fail } => {
                let ok = self.eval(Self::top_ctx(st), cond)?.truthy();
                self.branch(s.loc, ok, "assert");
                if ok {
                    if let Some(p) = pass {
                        st.frames.push(Frame::Seq { body: p.clone(), pc: 0, scoped: false });
                    }
                } else if let Some(f) = fail {
                    st.frames.push(Frame::Seq { body: f.clone(), pc: 0, scoped: false });
                } else {
                    let msg = "assertion failed".to_string();
                    self.log(Sev::Error, format!("{}:{}: {msg}", self.label(s.loc.file), s.loc.line));
                    self.fail(s.loc, msg);
                }
                Ok(Step::Go)
            }
            SK::RandCase(arms) => {
                let ctx = Self::top_ctx(st).clone();
                let mut ws = Vec::new();
                for (w, _) in arms {
                    ws.push(self.eval(&ctx, w)?.as_i64().max(0) as u64);
                }
                let total: u64 = ws.iter().sum();
                if total == 0 {
                    return Ok(Step::Go);
                }
                let mut r = self.next_rand() % total;
                for (k, w) in ws.iter().enumerate() {
                    if r < *w {
                        st.frames.push(Frame::Seq { body: arms[k].1.clone(), pc: 0, scoped: false });
                        break;
                    }
                    r -= w;
                }
                Ok(Step::Go)
            }
        }
    }

    fn foreach_tuples(v: &Value, dims: usize, prefix: &mut Vec<Value>, out: &mut Vec<Vec<Value>>) {
        if prefix.len() == dims {
            out.push(prefix.clone());
            return;
        }
        let idx: Vec<(Value, Option<&Value>)> = match v {
            Value::Arr(a) => a.items.iter().enumerate().map(|(i, x)| (Value::int(i as i64), Some(x))).collect(),
            Value::Assoc(m) => m.iter().map(|(k, x)| (k.to_value(), Some(x))).collect(),
            Value::Str(s) => (0..s.len()).map(|i| (Value::int(i as i64), None)).collect(),
            Value::Int(_, w, _) => (0..*w).rev().map(|i| (Value::int(i as i64), None)).collect(),
            _ => vec![],
        };
        for (k, sub) in idx {
            prefix.push(k);
            match sub {
                Some(s) if prefix.len() < dims => Self::foreach_tuples(s, dims, prefix, out),
                _ if prefix.len() < dims => {}
                _ => out.push(prefix.clone()),
            }
            prefix.pop();
        }
    }

    /// Wait key for one `@(…)` term.
    fn event_key(&mut self, ctx: &Ctx, t: &EvTerm) -> Result<(WaitKey, Option<Edge>), SvError> {
        // Clocking block event `@(vif.cb)` / `@(cb)`.
        if let Expr::Member(b, n) = &t.e {
            if let Ok(Value::VIf(Some(i))) = self.eval_quiet(ctx, b) {
                if let Some(c) = i.clocking.get(n) {
                    return Ok((WaitKey::Sig(c.clock), Some(c.edge)));
                }
            }
        }
        let p = self.lpath(ctx, &t.e)?;
        if let Place::Sig(id) = p.base {
            if p.sels.is_empty() {
                return Ok((WaitKey::Sig(id), t.edge));
            }
        }
        let (v, _) = self.read_path(&p)?;
        match v {
            Value::Event(ev) => Ok((WaitKey::Event(ev.id), None)),
            _ => Err(self.rt("event control on a non-signal: use a module/interface signal, an event, or a clocking block")),
        }
    }

    /// Expression statement: task/function call (may block), built-in
    /// blocking methods, system tasks.
    fn exec_expr_stmt(&mut self, st: &mut ExecState, e: &Expr, sync: bool) -> Result<Step, SvError> {
        let ctx = Self::top_ctx(st).clone();
        match e {
            Expr::Call { f, args, named, with, .. } => {
                let r = self.resolve_call(&ctx, f)?;
                match r {
                    Resolved::User(t) => {
                        if t.sub.is_task && sync {
                            return Err(self.rt(format!("task `{}` called from a function", t.sub.name)));
                        }
                        self.push_call(st, &t, args, named)?;
                        Ok(Step::Go)
                    }
                    Resolved::Builtin { recv: Recv::Val(Value::Obj(Some(o)), _), name } if matches!(name.as_str(), "get" | "put" | "peek") => {
                        match self.blocking_obj_method(&ctx, &o, &name, args)? {
                            true => Ok(Step::Go),
                            false => {
                                let pid = self.cur_pid(sync, "blocking mailbox/semaphore call")?;
                                let epoch = self.block(pid);
                                let id = o.borrow().id;
                                self.wait_on(pid, epoch, WaitKey::Obj(id), None);
                                Ok(Step::Retry)
                            }
                        }
                    }
                    other => {
                        let _ = with;
                        let call = Expr::Call { f: f.clone(), args: args.clone(), named: named.clone(), with: with.clone(), loc: self.cur_loc };
                        drop(other);
                        self.eval_call(&ctx, &call)?;
                        Ok(Step::Go)
                    }
                }
            }
            Expr::Sys { name, args, .. } => {
                self.sys_call(&ctx, name, args, true)?;
                Ok(Step::Go)
            }
            Expr::Ident(_) | Expr::Member(..) | Expr::Scoped(_) => {
                let call = Expr::Call { f: Box::new(e.clone()), args: vec![], named: vec![], with: None, loc: self.cur_loc };
                self.exec_expr_stmt(st, &call, sync)
            }
            other => {
                self.eval(&ctx, other)?;
                Ok(Step::Go)
            }
        }
    }
}
