//! Subroutine calls, method dispatch (static / virtual / `super`), object
//! construction (`new` with `super.new` chaining and property initializers).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::*;

#[derive(Clone)]
pub(crate) struct Target {
    pub sub: Rc<SubDecl>,
    pub this: Option<ObjRef>,
    pub class: Option<ClassId>,
    pub ns: NsId,
}

pub(crate) enum Recv {
    Path(LPath),
    Val(Value, Ty),
}

pub(crate) enum Resolved {
    User(Target),
    Builtin { recv: Recv, name: String },
    Randomize(ObjRef),
    StdRandomize,
    RandMode(ObjRef, Option<String>),
    ConstraintMode(ObjRef, Option<String>),
}

impl Engine<'_> {
    pub(crate) fn has_callable(&self, ctx: &Ctx, n: &str) -> bool {
        if let Some(c) = ctx.class {
            if self.find_method(c, n).is_some() {
                return true;
            }
        }
        self.ns_lookup_sub(ctx.ns, n, 0).is_some()
    }

    fn method_target(&self, static_cls: ClassId, obj: Option<ObjRef>, name: &str, dynamic: bool) -> Option<Target> {
        let (mut k, mut sub) = self.find_method(static_cls, name)?;
        if dynamic && !sub.quals.is_static && self.method_is_virtual(static_cls, name) {
            if let Some(o) = &obj {
                let oc = o.borrow().class;
                if let Some(oc) = oc {
                    if let Some((k2, s2)) = self.find_method(oc, name) {
                        k = k2;
                        sub = s2;
                    }
                }
            }
        }
        let this = if sub.quals.is_static { None } else { obj };
        Some(Target { ns: self.classes[k as usize].ns, sub, this, class: Some(k) })
    }

    pub(crate) fn resolve_call(&mut self, ctx: &Ctx, f: &Expr) -> Result<Resolved, SvError> {
        match f {
            Expr::Ident(n) => {
                match n.as_str() {
                    "randomize" => {
                        let o = ctx.this.clone().ok_or_else(|| self.rt("randomize() outside a class method; use std::randomize or obj.randomize()"))?;
                        return Ok(Resolved::Randomize(o));
                    }
                    "std::randomize" => return Ok(Resolved::StdRandomize),
                    "rand_mode" | "constraint_mode" if ctx.this.is_some() => {
                        let o = ctx.this.clone().unwrap();
                        return Ok(if n == "rand_mode" { Resolved::RandMode(o, None) } else { Resolved::ConstraintMode(o, None) });
                    }
                    _ => {}
                }
                if let Some(c) = ctx.class {
                    if let Some(t) = self.method_target(c, ctx.this.clone(), n, true) {
                        if t.this.is_none() && !t.sub.quals.is_static && t.sub.name != "new" {
                            return Err(self.rt(format!("method `{n}` called without an object (from a static method)")));
                        }
                        return Ok(Resolved::User(t));
                    }
                }
                if let Some((ns, sub)) = self.ns_lookup_sub(ctx.ns, n, 0) {
                    return Ok(Resolved::User(Target { sub, this: None, class: None, ns }));
                }
                Err(self.rt(format!("unknown function or task `{n}`")))
            }
            Expr::Member(b, n) if matches!(b.as_ref(), Expr::Super) => {
                let c = ctx.class.ok_or_else(|| self.rt("`super` outside a class"))?;
                let p = self.classes[c as usize].parent.get().ok_or_else(|| self.rt("`super` in a class without a base class"))?;
                let t = self.method_target(p, ctx.this.clone(), n, false).ok_or_else(|| self.rt(format!("no method `{n}` in the base class")))?;
                Ok(Resolved::User(t))
            }
            Expr::Member(b, n) if matches!(b.as_ref(), Expr::This) => {
                let c = ctx.class.ok_or_else(|| self.rt("`this` outside a class"))?;
                if n == "randomize" {
                    return Ok(Resolved::Randomize(ctx.this.clone().ok_or_else(|| self.rt("`this` in a static method"))?));
                }
                let t = self.method_target(c, ctx.this.clone(), n, true).ok_or_else(|| self.rt(format!("no method `{n}` in this class")))?;
                Ok(Resolved::User(t))
            }
            Expr::Member(b, n) => {
                if n == "rand_mode" || n == "constraint_mode" {
                    if let Expr::Member(ob, var) = b.as_ref() {
                        if let Ok(Value::Obj(Some(o))) = self.eval(ctx, ob) {
                            let is_field = o.borrow().fields.contains_key(var);
                            if n == "rand_mode" && is_field {
                                return Ok(Resolved::RandMode(o, Some(var.clone())));
                            }
                            if n == "constraint_mode" && !is_field {
                                return Ok(Resolved::ConstraintMode(o, Some(var.clone())));
                            }
                        }
                    }
                }
                let (val, ty, path) = match self.lpath(ctx, b) {
                    Ok(p) if !matches!(p.base, Place::Const(..)) => {
                        let (v, t) = self.read_path(&p)?;
                        (v, t, Some(p))
                    }
                    Ok(p) => {
                        let (v, t) = self.read_path(&p)?;
                        (v, t, None)
                    }
                    Err(e) => {
                        if matches!(b.as_ref(), Expr::Ident(_) | Expr::Member(..) | Expr::Index(..)) && !matches!(b.as_ref(), Expr::Ident(id) if self.has_callable(ctx, id)) {
                            // A plain reference that does not resolve.
                            if let Expr::Ident(_) = b.as_ref() {
                                return Err(e);
                            }
                        }
                        let v = self.eval(ctx, b)?;
                        (v, Ty::Void, None)
                    }
                };
                match &val {
                    Value::Obj(Some(o)) => {
                        let (cls, bi) = {
                            let ob = o.borrow();
                            (ob.class, ob.builtin)
                        };
                        if bi.is_some() {
                            return Ok(Resolved::Builtin { recv: Recv::Val(val.clone(), ty), name: n.clone() });
                        }
                        if n == "randomize" {
                            return Ok(Resolved::Randomize(o.clone()));
                        }
                        let sc = match ty {
                            Ty::Class(c) => c,
                            _ => cls.unwrap(),
                        };
                        match self.method_target(sc, Some(o.clone()), n, true) {
                            Some(t) => Ok(Resolved::User(t)),
                            None => {
                                // Declared type may be a base: try the dynamic class.
                                if let Some(t) = cls.and_then(|c| self.method_target(c, Some(o.clone()), n, true)) {
                                    return Ok(Resolved::User(t));
                                }
                                Err(self.rt(format!("class `{}` has no method `{n}`", self.classes[sc as usize].name)))
                            }
                        }
                    }
                    Value::Obj(None) => {
                        if let Ty::Class(c) = ty {
                            if let Some(t) = self.method_target(c, None, n, false) {
                                if t.sub.quals.is_static {
                                    return Ok(Resolved::User(t));
                                }
                            }
                        }
                        if matches!(ty, Ty::Builtin(_)) {
                            return Err(self.rt(format!("null object access: `{n}()` on a null mailbox/semaphore handle")));
                        }
                        Err(self.rt(format!("null object access: `{n}()` called on a null handle")))
                    }
                    Value::VIf(_) => Err(self.rt(format!("interface method `{n}()` calls are not supported; drive interface signals directly"))),
                    _ => {
                        let recv = match path {
                            Some(p) => Recv::Path(p),
                            None => Recv::Val(val, ty),
                        };
                        Ok(Resolved::Builtin { recv, name: n.clone() })
                    }
                }
            }
            Expr::Scoped(path) => {
                let (scope, last) = path.split_at(path.len() - 1);
                let name = &last[0].name;
                match self.resolve_scope(ctx.tyscope(), scope)? {
                    ScopeHit::Ns(ns) => match self.nss[ns].subs.get(name).cloned() {
                        Some(sub) => Ok(Resolved::User(Target { sub, this: None, class: None, ns })),
                        None => Err(self.rt(format!("package `{}` has no function `{name}`", self.nss[ns].name))),
                    },
                    ScopeHit::Class(c) => {
                        let this = ctx.this.clone().filter(|o| o.borrow().class.is_some_and(|oc| self.is_subclass(oc, c)));
                        let t = self.method_target(c, this, name, false).ok_or_else(|| self.rt(format!("class `{}` has no method `{name}`", self.classes[c as usize].name)))?;
                        if t.this.is_none() && !t.sub.quals.is_static {
                            return Err(self.rt(format!("`{}::{name}` is not static", self.classes[c as usize].name)));
                        }
                        Ok(Resolved::User(t))
                    }
                }
            }
            _ => Err(self.rt("expression is not callable")),
        }
    }

    pub(crate) fn eval_call(&mut self, ctx: &Ctx, e: &Expr) -> Result<Value, SvError> {
        let Expr::Call { f, args, named, with, .. } = e else { unreachable!() };
        match self.resolve_call(ctx, f)? {
            Resolved::User(t) => self.call_sync(ctx, &t, args, named),
            Resolved::Builtin { recv, name } => self.builtin_method(ctx, recv, &name, args, with.as_ref()),
            Resolved::Randomize(o) => {
                let only: Vec<String> = args
                    .iter()
                    .flatten()
                    .filter_map(|a| match a {
                        Expr::Ident(n) => Some(n.clone()),
                        _ => None,
                    })
                    .collect();
                let ok = self.randomize_obj(ctx, &o, with.as_ref(), if only.is_empty() { None } else { Some(only) })?;
                Ok(Value::int(ok as i64))
            }
            Resolved::StdRandomize => {
                let ok = self.std_randomize(ctx, args, with.as_ref())?;
                Ok(Value::int(ok as i64))
            }
            Resolved::RandMode(o, var) => {
                let on = match args.first() {
                    Some(Some(a)) => Some(self.eval(ctx, a)?.truthy()),
                    _ => None,
                };
                let mut ob = o.borrow_mut();
                match (var, on) {
                    (Some(v), Some(on)) => {
                        if on {
                            ob.rand_off.remove(&v);
                        } else {
                            ob.rand_off.insert(v);
                        }
                        Ok(Value::int(0))
                    }
                    (Some(v), None) => Ok(Value::int(!ob.rand_off.contains(&v) as i64)),
                    (None, Some(on)) => {
                        let names: Vec<String> = ob.fields.keys().cloned().collect();
                        for n in names {
                            if on {
                                ob.rand_off.remove(&n);
                            } else {
                                ob.rand_off.insert(n);
                            }
                        }
                        Ok(Value::int(0))
                    }
                    (None, None) => Ok(Value::int(1)),
                }
            }
            Resolved::ConstraintMode(o, block) => {
                let on = match args.first() {
                    Some(Some(a)) => Some(self.eval(ctx, a)?.truthy()),
                    _ => None,
                };
                let names: Vec<String> = match &block {
                    Some(b) => vec![b.clone()],
                    None => {
                        let c = o.borrow().class;
                        c.map(|c| self.class_chain(c).iter().flat_map(|k| self.classes[*k as usize].constraints.iter().map(|x| x.name.clone()).collect::<Vec<_>>()).collect()).unwrap_or_default()
                    }
                };
                let mut ob = o.borrow_mut();
                match on {
                    Some(on) => {
                        for n in names {
                            if on {
                                ob.cons_off.remove(&n);
                            } else {
                                ob.cons_off.insert(n);
                            }
                        }
                        Ok(Value::int(0))
                    }
                    None => Ok(Value::int(block.map(|b| !ob.cons_off.contains(&b)).unwrap_or(true) as i64)),
                }
            }
        }
    }

    /// Bind actual arguments for a call of `t` from `caller`.
    pub(crate) fn bind_args(
        &mut self,
        caller: &Ctx,
        t: &Target,
        args: &[Option<Expr>],
        named: &[(String, Option<Expr>)],
    ) -> Result<(Ctx, Vec<(LPath, String)>, Option<Ty>), SvError> {
        let sub = &t.sub;
        if sub.quals.is_pure {
            return Err(self.rt(format!("pure virtual method `{}` called (no override in the object's class)", sub.name)));
        }
        if args.len() > sub.args.len() {
            return Err(self.rt(format!("`{}` takes {} argument(s), {} given", sub.name, sub.args.len(), args.len())));
        }
        let scope: ScopeRef = Rc::new(RefCell::new(HashMap::new()));
        let callee = Ctx { scopes: vec![scope.clone()], this: t.this.clone(), class: t.class, ns: t.ns };
        let tsc = callee.tyscope();
        let mut outs = Vec::new();
        for (i, fa) in sub.args.iter().enumerate() {
            let actual = named.iter().find(|(n, _)| *n == fa.name).map(|(_, e)| e.clone()).unwrap_or_else(|| args.get(i).cloned().flatten());
            let ty = self.resolve_type(tsc, &fa.ty, &fa.udims, None)?;
            let v = match (&actual, fa.dir) {
                (Some(e), Dir::Input) => self.eval_to(caller, e, &ty)?,
                (Some(e), Dir::Output) => {
                    outs.push((self.lpath(caller, e)?, fa.name.clone()));
                    ty.default_value()
                }
                (Some(e), Dir::Inout | Dir::Ref) => {
                    let p = self.lpath(caller, e)?;
                    let (v, _) = self.read_path(&p)?;
                    if !matches!(p.base, Place::Const(..)) {
                        outs.push((p, fa.name.clone()));
                    }
                    convert(v, &ty)
                }
                (None, _) => match &fa.default {
                    Some(d) => self.eval_to(&callee, d, &ty)?,
                    None if fa.dir == Dir::Output => ty.default_value(),
                    None => return Err(self.rt(format!("missing argument `{}` in call to `{}`", fa.name, sub.name))),
                },
            };
            scope.borrow_mut().insert(fa.name.clone(), Var { ty, v });
        }
        let ret = match &sub.ret {
            Some(DataType::Void) | None => None,
            Some(rt) => {
                let ty = self.resolve_type(tsc, rt, &[], None)?;
                // Old-style `fname = value;` return variable.
                scope.borrow_mut().entry(sub.name.clone()).or_insert_with(|| Var::new(ty.clone()));
                Some(ty)
            }
        };
        Ok((callee, outs, ret))
    }

    /// Call `t` synchronously (functions in expressions); it must not block.
    pub(crate) fn call_sync(&mut self, caller: &Ctx, t: &Target, args: &[Option<Expr>], named: &[(String, Option<Expr>)]) -> Result<Value, SvError> {
        let (callee, outs, ret) = self.bind_args(caller, t, args, named)?;
        if t.sub.is_task {
            return Err(self.rt(format!("task `{}` called from an expression (tasks may consume time; call it as a statement)", t.sub.name)));
        }
        let mut st = ExecState {
            frames: vec![Frame::Call { outs, ret, func: t.sub.name.clone() }, Frame::Seq { body: t.sub.body.clone(), pc: 0, scoped: false }],
            ctxs: vec![callee],
            retval: None,
        };
        let saved = self.cur_loc;
        self.step(&mut st, true)?;
        self.cur_loc = saved;
        Ok(st.retval.unwrap_or(Value::Int(0, 1, false)))
    }

    /// Push a call of `t` onto `st` (statement level; tasks may block).
    pub(crate) fn push_call(&mut self, st: &mut ExecState, t: &Target, args: &[Option<Expr>], named: &[(String, Option<Expr>)]) -> Result<(), SvError> {
        let caller = st.ctxs.last().cloned().expect("ctx");
        let (callee, outs, ret) = self.bind_args(&caller, t, args, named)?;
        st.ctxs.push(callee);
        st.frames.push(Frame::Call { outs, ret, func: t.sub.name.clone() });
        st.frames.push(Frame::Seq { body: t.sub.body.clone(), pc: 0, scoped: false });
        Ok(())
    }

    // ---- objects ---------------------------------------------------------------

    pub(crate) fn new_object(&mut self, cid: ClassId, args: Vec<Option<Value>>) -> Result<ObjRef, SvError> {
        let info = self.classes[cid as usize].clone();
        if info.decl.is_virtual {
            return Err(self.rt(format!("cannot instantiate abstract (virtual) class `{}`", info.name)));
        }
        let id = self.new_obj_id();
        let obj = Rc::new(RefCell::new(Object::new(id, Some(cid), None)));
        let mut chain = self.class_chain(cid);
        chain.reverse();
        for k in chain {
            let fs = self.own_fields(k)?;
            let mut o = obj.borrow_mut();
            for (n, ty, _) in fs.iter() {
                o.fields.insert(n.clone(), Var::new(ty.clone()));
            }
        }
        self.run_ctor(cid, &obj, args)?;
        Ok(obj)
    }

    fn run_ctor(&mut self, cid: ClassId, obj: &ObjRef, args: Vec<Option<Value>>) -> Result<(), SvError> {
        let info = self.classes[cid as usize].clone();
        let parent = info.parent.get();
        match &info.ctor {
            Some(ctor) => {
                let scope: ScopeRef = Rc::new(RefCell::new(HashMap::new()));
                let ctx = Ctx { scopes: vec![scope.clone()], this: Some(obj.clone()), class: Some(cid), ns: info.ns };
                if args.len() > ctor.sub.args.len() {
                    return Err(self.rt(format!("`{}::new` takes {} argument(s), {} given", info.name, ctor.sub.args.len(), args.len())));
                }
                for (i, fa) in ctor.sub.args.iter().enumerate() {
                    let ty = self.resolve_type(ctx.tyscope(), &fa.ty, &fa.udims, None)?;
                    let v = match args.get(i).cloned().flatten() {
                        Some(v) => convert(v, &ty),
                        None => match &fa.default {
                            Some(d) => self.eval_to(&ctx, d, &ty)?,
                            None => return Err(self.rt(format!("`{}::new` needs argument `{}`", info.name, fa.name))),
                        },
                    };
                    scope.borrow_mut().insert(fa.name.clone(), Var { ty, v });
                }
                if let Some(p) = parent {
                    let pargs = match &ctor.super_args {
                        Some((exprs, loc)) => {
                            let saved = self.cur_loc;
                            self.cur_loc = *loc;
                            let v = self.eval_args_plain(&ctx, exprs)?;
                            self.cur_loc = saved;
                            v
                        }
                        None => vec![],
                    };
                    self.run_ctor(p, obj, pargs)?;
                }
                self.init_fields(cid, obj)?;
                let mut st = ExecState {
                    frames: vec![Frame::Call { outs: vec![], ret: None, func: "new".into() }, Frame::Seq { body: ctor.rest.clone(), pc: 0, scoped: false }],
                    ctxs: vec![ctx],
                    retval: None,
                };
                let saved = self.cur_loc;
                self.step(&mut st, true)?;
                self.cur_loc = saved;
            }
            None => {
                if let Some(p) = parent {
                    self.run_ctor(p, obj, args)?;
                }
                self.init_fields(cid, obj)?;
            }
        }
        Ok(())
    }

    fn init_fields(&mut self, cid: ClassId, obj: &ObjRef) -> Result<(), SvError> {
        let info = self.classes[cid as usize].clone();
        let ctx = Ctx { scopes: vec![], this: Some(obj.clone()), class: Some(cid), ns: info.ns };
        for it in &info.decl.items {
            if let ClassItem::Prop(d) = it {
                if !d.is_static {
                    if let Some(init) = &d.init {
                        let saved = self.cur_loc;
                        self.cur_loc = d.loc;
                        let ty = obj.borrow().fields[&d.name].ty.clone();
                        let v = self.eval_to(&ctx, init, &ty)?;
                        obj.borrow_mut().fields.get_mut(&d.name).unwrap().v = v;
                        self.cur_loc = saved;
                    }
                }
            }
        }
        Ok(())
    }
}
