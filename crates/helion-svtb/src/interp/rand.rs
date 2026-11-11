//! Constrained randomization: `randomize()`, `randomize() with {…}`,
//! `std::randomize`, `pre_randomize` / `post_randomize`, on top of the
//! `helion-sim` constraint solver ([`helion_sim::RandConstraintSolver`]).
//!
//! Constraint expressions that only involve state (non-`rand` members,
//! locals, parameters) are folded to constants before solving; `rand`
//! integral members become solver variables. Values are unsigned bit
//! vectors (≤ 64 bits): constraints relying on negative values of signed
//! `rand int` members are not supported. `soft` constraints are applied and
//! dropped only if the problem is unsatisfiable with them.

use std::collections::{HashMap, HashSet};

use helion_sim::{DistItem as SDist, RandClass, RandConstraint as RC, RandConstraintSolver, RandExpr as RE};

use super::call::Target;
use super::*;

struct Conv<'c> {
    /// Rand variable names visible unqualified.
    vars: &'c HashSet<String>,
    /// Object whose members the constraint refers to.
    obj: Option<ObjRef>,
    /// Context for folding non-rand expressions.
    ctx: Ctx,
    soft: Vec<RC>,
}

impl Engine<'_> {
    /// Rand variable referenced by `e`, if any (`x`, `this.x`, `obj.x` for the target object).
    fn rand_ref(&self, cv: &Conv, e: &Expr) -> Option<String> {
        match e {
            Expr::Ident(n) if cv.vars.contains(n) => {
                // A local of the same name shadows the member (inline `with`).
                if cv.ctx.scopes.iter().any(|s| s.borrow().contains_key(n)) && cv.obj.is_none() {
                    return None;
                }
                Some(n.clone())
            }
            Expr::Member(b, n) if matches!(b.as_ref(), Expr::This) && cv.vars.contains(n) => Some(n.clone()),
            _ => None,
        }
    }

    fn has_rand_ref(&self, cv: &Conv, e: &Expr) -> bool {
        if self.rand_ref(cv, e).is_some() {
            return true;
        }
        match e {
            Expr::Unary(_, x) | Expr::Cast(_, x) => self.has_rand_ref(cv, x),
            Expr::Binary(_, a, b) => self.has_rand_ref(cv, a) || self.has_rand_ref(cv, b),
            Expr::Cond(a, b, c) => self.has_rand_ref(cv, a) || self.has_rand_ref(cv, b) || self.has_rand_ref(cv, c),
            Expr::Inside(x, items) => {
                self.has_rand_ref(cv, x)
                    || items.iter().any(|i| match i {
                        InsideItem::One(a) => self.has_rand_ref(cv, a),
                        InsideItem::Range(a, b) => self.has_rand_ref(cv, a) || self.has_rand_ref(cv, b),
                    })
            }
            Expr::Concat(p) => p.iter().any(|x| self.has_rand_ref(cv, x)),
            _ => false,
        }
    }

    fn cexpr(&mut self, cv: &Conv, e: &Expr) -> Result<RE, SvError> {
        if let Some(n) = self.rand_ref(cv, e) {
            return Ok(RE::Var(n));
        }
        if !self.has_rand_ref(cv, e) {
            let v = self.eval(&cv.ctx, e)?;
            return Ok(RE::Const(v.as_u64()));
        }
        let b = |x: RE| Box::new(x);
        Ok(match e {
            Expr::Unary(op, x) => {
                let x = self.cexpr(cv, x)?;
                match op {
                    UnOp::Not => RE::Not(b(x)),
                    UnOp::Neg => RE::Sub(b(RE::Const(0)), b(x)),
                    UnOp::Plus => x,
                    UnOp::BitNot => RE::BitXor(b(x), b(RE::Const(u64::MAX))),
                    UnOp::RedOr => RE::Ne(b(x), b(RE::Const(0))),
                    _ => return Err(self.rt("reduction operators on rand variables are not supported in constraints")),
                }
            }
            Expr::Binary(op, l, r) => {
                let (l, r) = (self.cexpr(cv, l)?, self.cexpr(cv, r)?);
                match op {
                    BinOp::Add => RE::Add(b(l), b(r)),
                    BinOp::Sub => RE::Sub(b(l), b(r)),
                    BinOp::Mul => RE::Mul(b(l), b(r)),
                    BinOp::Div => RE::Div(b(l), b(r)),
                    BinOp::Mod => RE::Mod(b(l), b(r)),
                    BinOp::And => RE::BitAnd(b(l), b(r)),
                    BinOp::Or => RE::BitOr(b(l), b(r)),
                    BinOp::Xor => RE::BitXor(b(l), b(r)),
                    BinOp::Shl | BinOp::AShl => RE::Shl(b(l), b(r)),
                    BinOp::Shr | BinOp::AShr => RE::Shr(b(l), b(r)),
                    BinOp::Eq | BinOp::CaseEq => RE::Eq(b(l), b(r)),
                    BinOp::Ne | BinOp::CaseNe => RE::Ne(b(l), b(r)),
                    BinOp::Lt => RE::Lt(b(l), b(r)),
                    BinOp::Le => RE::Le(b(l), b(r)),
                    BinOp::Gt => RE::Gt(b(l), b(r)),
                    BinOp::Ge => RE::Ge(b(l), b(r)),
                    BinOp::LAnd => RE::And(b(l), b(r)),
                    BinOp::LOr => RE::Or(b(l), b(r)),
                    BinOp::Implies => RE::Or(b(RE::Not(b(l))), b(r)),
                    BinOp::Equiv => RE::Eq(b(RE::Ne(b(l), b(RE::Const(0)))), b(RE::Ne(b(r), b(RE::Const(0))))),
                    other => return Err(self.rt(format!("operator {other:?} on rand variables is not supported in constraints"))),
                }
            }
            Expr::Cond(c, x, y) => {
                let (c, x, y) = (self.cexpr(cv, c)?, self.cexpr(cv, x)?, self.cexpr(cv, y)?);
                // Boolean-valued `c ? x : y`.
                RE::Or(b(RE::And(b(c.clone()), b(x))), b(RE::And(b(RE::Not(b(c))), b(y))))
            }
            Expr::Inside(x, items) => {
                let xv = self.cexpr(cv, x)?;
                let mut acc: Option<RE> = None;
                for it in items {
                    let term = match it {
                        InsideItem::One(a) => RE::Eq(b(xv.clone()), b(self.cexpr(cv, a)?)),
                        InsideItem::Range(lo, hi) => RE::And(b(RE::Ge(b(xv.clone()), b(self.cexpr(cv, lo)?))), b(RE::Le(b(xv.clone()), b(self.cexpr(cv, hi)?)))),
                    };
                    acc = Some(match acc {
                        None => term,
                        Some(a) => RE::Or(b(a), b(term)),
                    });
                }
                acc.unwrap_or(RE::Const(0))
            }
            Expr::Cast(_, x) => self.cexpr(cv, x)?,
            _ => return Err(self.rt("unsupported expression on rand variables in a constraint")),
        })
    }

    fn citems(&mut self, cv: &mut Conv, items: &[CItem], out: &mut Vec<RC>) -> Result<(), SvError> {
        for it in items {
            match it {
                CItem::Expr(e) => {
                    let x = self.cexpr(cv, e)?;
                    out.push(RC::Expr(x));
                }
                CItem::Inside { e, items, neg } => {
                    if let Some(v) = self.rand_ref(cv, e) {
                        // Constant set → domain pruning in the solver.
                        let mut ranges = Vec::new();
                        let mut all_const = true;
                        for i in items {
                            match i {
                                InsideItem::One(a) if !self.has_rand_ref(cv, a) => {
                                    let val = self.eval(&cv.ctx, a)?;
                                    if let Value::Arr(arr) = val {
                                        for x in arr.items {
                                            let u = x.as_u64();
                                            ranges.push((u, u));
                                        }
                                    } else {
                                        let u = val.as_u64();
                                        ranges.push((u, u));
                                    }
                                }
                                InsideItem::Range(a, b) if !self.has_rand_ref(cv, a) && !self.has_rand_ref(cv, b) => {
                                    let lo = self.eval(&cv.ctx, a)?.as_u64();
                                    let hi = self.eval(&cv.ctx, b)?.as_u64();
                                    ranges.push((lo.min(hi), lo.max(hi)));
                                }
                                _ => all_const = false,
                            }
                        }
                        if all_const {
                            out.push(RC::Inside { var: v, ranges, negated: *neg });
                            continue;
                        }
                    }
                    let x = self.cexpr(cv, &Expr::Inside(Box::new(e.clone()), items.clone()))?;
                    out.push(RC::Expr(if *neg { RE::Not(Box::new(x)) } else { x }));
                }
                CItem::Dist { e, items } => {
                    let var = self.rand_ref(cv, e).ok_or_else(|| self.rt("`dist` needs a rand variable on the left"))?;
                    let mut ds = Vec::new();
                    for d in items {
                        let lo = self.eval(&cv.ctx, &d.lo)?.as_u64();
                        let hi = match &d.hi {
                            Some(h) => self.eval(&cv.ctx, h)?.as_u64(),
                            None => lo,
                        };
                        let w = self.eval(&cv.ctx, &d.weight)?.as_u64();
                        ds.push(if d.per_range { SDist::range_per_range(lo, hi, w) } else { SDist::range_per_value(lo, hi, w) });
                    }
                    out.push(RC::Dist { var, items: ds });
                }
                CItem::Implies(c, body) => {
                    if !self.has_rand_ref(cv, c) {
                        if self.eval(&cv.ctx, c)?.truthy() {
                            self.citems(cv, body, out)?;
                        }
                        continue;
                    }
                    let cond = self.cexpr(cv, c)?;
                    let mut inner = Vec::new();
                    self.citems(cv, body, &mut inner)?;
                    out.push(RC::Implies { cond, body: inner });
                }
                CItem::IfElse(c, t, e) => {
                    if !self.has_rand_ref(cv, c) {
                        let pick = if self.eval(&cv.ctx, c)?.truthy() { t } else { e };
                        self.citems(cv, pick, out)?;
                        continue;
                    }
                    let cond = self.cexpr(cv, c)?;
                    let mut ti = Vec::new();
                    self.citems(cv, t, &mut ti)?;
                    out.push(RC::Implies { cond: cond.clone(), body: ti });
                    if !e.is_empty() {
                        let mut ei = Vec::new();
                        self.citems(cv, e, &mut ei)?;
                        out.push(RC::Implies { cond: RE::Not(Box::new(cond)), body: ei });
                    }
                }
                CItem::SolveBefore(a, b) => {
                    for x in a {
                        for y in b {
                            if let (Some(p), Some(q)) = (self.rand_ref(cv, x), self.rand_ref(cv, y)) {
                                out.push(RC::SolveBefore { before: p, after: q });
                            }
                        }
                    }
                }
                CItem::Soft(i) => {
                    let mut tmp = Vec::new();
                    self.citems(cv, std::slice::from_ref(i), &mut tmp)?;
                    cv.soft.extend(tmp);
                }
                CItem::Unique(es) => {
                    let names: Vec<String> = es.iter().filter_map(|e| self.rand_ref(cv, e)).collect();
                    out.push(RC::Unique(names));
                }
                CItem::Foreach { .. } => return Err(self.rt("`foreach` constraints (rand arrays) are not supported")),
            }
        }
        Ok(())
    }

    fn call_hook(&mut self, o: &ObjRef, name: &str) -> Result<(), SvError> {
        let c = o.borrow().class;
        let Some(c) = c else { return Ok(()) };
        let Some((k, _)) = self.find_method(c, name) else { return Ok(()) };
        let (k, sub) = self.find_method(k, name).unwrap();
        let t = Target { ns: self.classes[k as usize].ns, sub, this: Some(o.clone()), class: Some(k) };
        let ctx = Ctx { scopes: vec![], this: Some(o.clone()), class: Some(c), ns: t.ns };
        self.call_sync(&ctx, &t, &[], &[])?;
        Ok(())
    }

    /// `obj.randomize([only]) [with {…}]`.
    pub(crate) fn randomize_obj(&mut self, caller: &Ctx, o: &ObjRef, with: Option<&WithClause>, only: Option<Vec<String>>) -> Result<bool, SvError> {
        let cls = o.borrow().class.ok_or_else(|| self.rt("randomize() on a built-in object"))?;
        self.call_hook(o, "pre_randomize")?;
        // Rand variables (whole hierarchy).
        let mut specs: Vec<(String, u8, RandKind)> = Vec::new();
        let mut nested: Vec<ObjRef> = Vec::new();
        for k in self.class_chain(cls) {
            let fs = self.own_fields(k)?;
            for (n, ty, rk) in fs.iter() {
                let active = match &only {
                    Some(list) => list.contains(n),
                    None => *rk != RandKind::None && !o.borrow().rand_off.contains(n),
                };
                if !active {
                    continue;
                }
                match ty {
                    Ty::Int { w, .. } => specs.push((n.clone(), (*w).min(64) as u8, if only.is_some() { RandKind::Rand } else { *rk })),
                    Ty::Enum(e) => specs.push((n.clone(), e.w as u8, *rk)),
                    Ty::Class(_) => {
                        if let Some(Value::Obj(Some(x))) = o.borrow().fields.get(n).map(|v| v.v.clone()) {
                            nested.push(x);
                        }
                    }
                    other => return Err(self.rt(format!("rand member `{n}` of type {} is not supported (integral rand members only)", self.ty_name(other)))),
                }
            }
        }
        let names: HashSet<String> = specs.iter().map(|s| s.0.clone()).collect();
        let mut clauses = Vec::new();
        let mut soft = Vec::new();
        // Class constraints (a derived block with the same name overrides).
        let mut seen_blocks = HashSet::new();
        for k in self.class_chain(cls) {
            let info = self.classes[k as usize].clone();
            for c in &info.constraints {
                if !seen_blocks.insert(c.name.clone()) || o.borrow().cons_off.contains(&c.name) {
                    continue;
                }
                let mut cv = Conv { vars: &names, obj: Some(o.clone()), ctx: Ctx { scopes: vec![], this: Some(o.clone()), class: Some(k), ns: info.ns }, soft: vec![] };
                let saved = self.cur_loc;
                self.cur_loc = c.loc;
                self.citems(&mut cv, &c.items, &mut clauses)?;
                self.cur_loc = saved;
                soft.extend(cv.soft);
            }
        }
        if let Some(WithClause::Constraints(items)) = with {
            // Names resolve in the object first, then in the caller.
            let mut ctx = caller.clone();
            ctx.this = Some(o.clone());
            ctx.class = Some(cls);
            let mut cv = Conv { vars: &names, obj: Some(o.clone()), ctx, soft: vec![] };
            self.citems(&mut cv, items, &mut clauses)?;
            soft.extend(cv.soft);
        }
        let ok = self.solve_into(o, &specs, clauses, soft)?;
        if ok {
            for n in nested {
                self.randomize_obj(caller, &n, None, None)?;
            }
            self.call_hook(o, "post_randomize")?;
        } else {
            let at = format!("{}:{}", self.label(self.cur_loc.file), self.cur_loc.line);
            self.log(Sev::Warning, format!("{at}: randomize() failed: constraints are unsatisfiable for {}", self.classes[cls as usize].name));
        }
        Ok(ok)
    }

    fn solve_into(&mut self, o: &ObjRef, specs: &[(String, u8, RandKind)], clauses: Vec<RC>, soft: Vec<RC>) -> Result<bool, SvError> {
        let mut rc = RandClass::new("obj");
        for (n, w, k) in specs {
            if *k == RandKind::Randc {
                rc.add_randc(n.clone(), *w);
            } else {
                rc.add_rand(n.clone(), *w);
            }
        }
        let mut all = clauses.clone();
        all.extend(soft);
        rc.add_constraint("all", all);
        let seed = self.next_rand();
        let mut solver = o.borrow_mut().solver.take().unwrap_or_else(|| RandConstraintSolver::new(RandClass::new("obj"), seed));
        solver.class = rc;
        let mut r = solver.randomize();
        if r.is_err() {
            let mut rc2 = solver.class.clone();
            rc2.constraints.clear();
            rc2.add_constraint("hard", clauses);
            solver.class = rc2;
            r = solver.randomize();
        }
        let out = match r {
            Ok(vals) => {
                let mut ob = o.borrow_mut();
                for (n, _, _) in specs {
                    if let (Some(v), Some(var)) = (vals.get(n), ob.fields.get_mut(n)) {
                        var.v = convert(Value::Int(*v, 64, false), &var.ty);
                    }
                }
                true
            }
            Err(_) => false,
        };
        o.borrow_mut().solver = Some(solver);
        Ok(out)
    }

    /// `std::randomize(a, b) with {…}` over local/variable lvalues.
    pub(crate) fn std_randomize(&mut self, ctx: &Ctx, args: &[Option<Expr>], with: Option<&WithClause>) -> Result<bool, SvError> {
        let mut specs = Vec::new();
        let mut paths = HashMap::new();
        for a in args.iter().flatten() {
            let Expr::Ident(n) = a else { return Err(self.rt("std::randomize arguments must be variables")) };
            let p = self.lpath(ctx, a)?;
            let (_, ty) = self.read_path(&p)?;
            let w = match &ty {
                Ty::Int { w, .. } => *w,
                Ty::Enum(e) => e.w,
                _ => return Err(self.rt("std::randomize supports integral variables only")),
            };
            specs.push((n.clone(), w as u8, RandKind::Rand));
            paths.insert(n.clone(), p);
        }
        let names: HashSet<String> = specs.iter().map(|s| s.0.clone()).collect();
        let mut clauses = Vec::new();
        let mut soft = Vec::new();
        if let Some(WithClause::Constraints(items)) = with {
            let mut cv = Conv { vars: &names, obj: Some(Rc::new(RefCell::new(Object::new(0, None, None)))), ctx: ctx.clone(), soft: vec![] };
            self.citems(&mut cv, items, &mut clauses)?;
            soft = cv.soft;
        }
        let tmp = Rc::new(RefCell::new(Object::new(0, None, None)));
        for (n, w, _) in &specs {
            tmp.borrow_mut().fields.insert(n.clone(), Var::new(Ty::int(*w as u32, false)));
        }
        let ok = self.solve_into(&tmp, &specs, clauses, soft)?;
        if ok {
            for (n, p) in paths {
                let v = tmp.borrow().fields[&n].v.clone();
                self.write_path(&p, v)?;
            }
        }
        Ok(ok)
    }
}

use std::cell::RefCell;
use std::rc::Rc;
