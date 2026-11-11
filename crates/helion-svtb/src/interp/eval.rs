//! Expression evaluation, places (lvalues), reads and writes.

use std::rc::Rc;

use super::*;

fn pow_u(mut b: u64, mut e: u64) -> u64 {
    let mut r = 1u64;
    while e > 0 {
        if e & 1 == 1 {
            r = r.wrapping_mul(b);
        }
        b = b.wrapping_mul(b);
        e >>= 1;
    }
    r
}

impl Engine<'_> {
    // ---- reads & writes ------------------------------------------------------

    pub(crate) fn read_place(&self, p: &Place) -> Result<(Value, Ty), SvError> {
        match p {
            Place::Local(s, n) => s.borrow().get(n).map(|v| (v.v.clone(), v.ty.clone())).ok_or_else(|| self.rt(format!("`{n}` vanished"))),
            Place::Field(o, n) => o.borrow().fields.get(n).map(|v| (v.v.clone(), v.ty.clone())).ok_or_else(|| self.rt(format!("no field `{n}`"))),
            Place::Static(c, n) => self.classes[*c as usize].statics.borrow().get(n).map(|v| (v.v.clone(), v.ty.clone())).ok_or_else(|| self.rt(format!("no static `{n}`"))),
            Place::NsVar(ns, n) => self.nss[*ns].vars.borrow().get(n).map(|v| (v.v.clone(), v.ty.clone())).ok_or_else(|| self.rt(format!("no variable `{n}`"))),
            Place::Sig(id) => Ok((self.sigs[*id].v.clone(), self.sigs[*id].ty.clone())),
            Place::Clock(i, cb, n) => {
                let c = i.clocking.get(cb).ok_or_else(|| self.rt(format!("no clocking block `{cb}`")))?;
                let id = *c.sigs.get(n).ok_or_else(|| self.rt(format!("clocking block `{cb}` has no signal `{n}`")))?;
                let v = c.sampled.borrow().get(n).cloned().unwrap_or_else(|| self.sigs[id].v.clone());
                Ok((v, self.sigs[id].ty.clone()))
            }
            Place::Const(v, t) => Ok((v.clone(), t.clone())),
        }
    }

    pub(crate) fn read_path(&mut self, p: &LPath) -> Result<(Value, Ty), SvError> {
        let (mut v, mut ty) = self.read_place(&p.base)?;
        for s in &p.sels {
            let (nv, nt) = self.select(&v, &ty, s)?;
            v = nv;
            ty = nt;
        }
        Ok((v, ty))
    }

    fn select(&self, v: &Value, ty: &Ty, s: &Sel) -> Result<(Value, Ty), SvError> {
        match s {
            Sel::Bits(hi, lo) => {
                let w = hi - lo + 1;
                Ok((Value::Int(mask(v.as_u64() >> lo, w), w, false), Ty::int(w, false)))
            }
            Sel::Index(i) => match v {
                Value::Arr(a) => {
                    let et = ty.elem().cloned().unwrap_or(Ty::int(32, true));
                    let k = i.as_i64();
                    if k < 0 || k as usize >= a.items.len() {
                        return Ok((et.default_value(), et));
                    }
                    Ok((a.items[k as usize].clone(), et))
                }
                Value::Assoc(m) => {
                    let (et, sk) = match ty {
                        Ty::Assoc(e, s) => ((**e).clone(), *s),
                        _ => (Ty::int(32, true), false),
                    };
                    Ok((m.get(&i.to_key(sk)).cloned().unwrap_or_else(|| et.default_value()), et))
                }
                Value::Str(st) => {
                    let k = i.as_i64();
                    let b = if k >= 0 { st.as_bytes().get(k as usize).copied().unwrap_or(0) } else { 0 };
                    Ok((Value::Int(b as u64, 8, false), Ty::int(8, false)))
                }
                other => {
                    let k = i.as_u64() as u32;
                    Ok((Value::Int((other.as_u64() >> k.min(63)) & 1, 1, false), Ty::int(1, false)))
                }
            },
        }
    }

    /// Store `v` into `p` (with conversion to the slot type).
    pub(crate) fn write_path(&mut self, p: &LPath, v: Value) -> Result<(), SvError> {
        self.dirty += 1;
        if let Place::Sig(id) = &p.base {
            let id = *id;
            let mut cur = self.sigs[id].v.clone();
            let ty = self.sigs[id].ty.clone();
            if p.sels.is_empty() {
                cur = convert(v, &ty);
            } else {
                Self::write_sels(&mut cur, &ty, &p.sels, v).map_err(|m| self.rt(m))?;
            }
            return self.write_signal(id, cur);
        }
        if let Place::Clock(i, cb, n) = &p.base {
            // `cb.sig <= v`: drive the underlying signal.
            let c = i.clocking.get(cb).ok_or_else(|| self.rt(format!("no clocking block `{cb}`")))?;
            let id = *c.sigs.get(n).ok_or_else(|| self.rt(format!("clocking block `{cb}` has no signal `{n}`")))?;
            return self.write_path(&LPath { base: Place::Sig(id), sels: p.sels.clone() }, v);
        }
        let r = match &p.base {
            Place::Local(s, n) => {
                let mut b = s.borrow_mut();
                let var = b.get_mut(n).ok_or_else(|| format!("`{n}` vanished"));
                var.and_then(|var| Self::store(var, &p.sels, v))
            }
            Place::Field(o, n) => {
                let mut b = o.borrow_mut();
                let var = b.fields.get_mut(n).ok_or_else(|| format!("no field `{n}`"));
                var.and_then(|var| Self::store(var, &p.sels, v))
            }
            Place::Static(c, n) => {
                let info = self.classes[*c as usize].clone();
                let mut b = info.statics.borrow_mut();
                let var = b.get_mut(n).ok_or_else(|| format!("no static `{n}`"));
                var.and_then(|var| Self::store(var, &p.sels, v))
            }
            Place::NsVar(ns, n) => {
                let mut b = self.nss[*ns].vars.borrow_mut();
                let var = b.get_mut(n).ok_or_else(|| format!("no variable `{n}`"));
                var.and_then(|var| Self::store(var, &p.sels, v))
            }
            Place::Const(..) => Err("cannot assign to a constant or expression".to_string()),
            Place::Sig(_) | Place::Clock(..) => unreachable!(),
        };
        r.map_err(|m| self.rt(m))
    }

    fn store(var: &mut Var, sels: &[Sel], v: Value) -> Result<(), String> {
        if sels.is_empty() {
            var.v = convert(v, &var.ty);
            return Ok(());
        }
        let ty = var.ty.clone();
        Self::write_sels(&mut var.v, &ty, sels, v)
    }

    fn write_sels(cur: &mut Value, ty: &Ty, sels: &[Sel], v: Value) -> Result<(), String> {
        let Some((first, rest)) = sels.split_first() else {
            *cur = convert(v, ty);
            return Ok(());
        };
        match first {
            Sel::Bits(hi, lo) => {
                let w = hi - lo + 1;
                let (old, cw, s) = match &*cur {
                    Value::Int(x, w, s) => (*x, *w, *s),
                    other => (other.as_u64(), ty.width(), false),
                };
                let fm = mask(u64::MAX, w) << lo;
                let nv = (old & !fm) | ((mask(v.as_u64(), w) << lo) & fm);
                *cur = Value::Int(mask(nv, cw), cw, s);
                Ok(())
            }
            Sel::Index(i) => match cur {
                Value::Arr(a) => {
                    let et = ty.elem().cloned().unwrap_or(Ty::int(32, true));
                    let k = i.as_i64();
                    if k < 0 {
                        return Err(format!("array index {k} is negative"));
                    }
                    let k = k as usize;
                    if k == a.items.len() && a.kind == ArrKind::Queue {
                        a.items.push(et.default_value());
                    }
                    if k >= a.items.len() {
                        return Err(format!("array index {k} out of bounds (size {})", a.items.len()));
                    }
                    Self::write_sels(&mut a.items[k], &et, rest, v)
                }
                Value::Assoc(m) => {
                    let (et, sk) = match ty {
                        Ty::Assoc(e, s) => ((**e).clone(), *s),
                        _ => (Ty::int(32, true), false),
                    };
                    let e = m.entry(i.to_key(sk)).or_insert_with(|| et.default_value());
                    Self::write_sels(e, &et, rest, v)
                }
                Value::Str(s) => {
                    let k = i.as_i64();
                    let mut bytes = s.clone().into_bytes();
                    if k >= 0 && (k as usize) < bytes.len() {
                        bytes[k as usize] = v.as_u64() as u8;
                        *s = String::from_utf8_lossy(&bytes).into_owned();
                    }
                    Ok(())
                }
                other => {
                    let k = i.as_u64() as u32;
                    let w = ty.width();
                    if k >= w {
                        return Err(format!("bit [{k}] out of range for a {w}-bit value"));
                    }
                    let s = other.signed();
                    let old = other.as_u64();
                    let nv = if v.as_u64() & 1 == 1 { old | (1 << k) } else { old & !(1 << k) };
                    *other = Value::Int(mask(nv, w), w, s);
                    Ok(())
                }
            },
        }
    }

    // ---- places ----------------------------------------------------------------

    /// Resolve an lvalue / named storage location.
    pub(crate) fn lpath(&mut self, ctx: &Ctx, e: &Expr) -> Result<LPath, SvError> {
        match e {
            Expr::Ident(n) => match self.lookup_var(ctx, n) {
                Some(p) => Ok(LPath { base: p, sels: vec![] }),
                None => Err(self.rt(format!("unknown variable `{n}`"))),
            },
            Expr::Scoped(path) => {
                let (scope, last) = path.split_at(path.len() - 1);
                let hit = self.resolve_scope(ctx.tyscope(), scope)?;
                let name = &last[0].name;
                match hit {
                    ScopeHit::Ns(ns) => self
                        .ns_lookup_var(ns, name, 0)
                        .map(|p| LPath { base: p, sels: vec![] })
                        .ok_or_else(|| self.rt(format!("package `{}` has no `{name}`", self.nss[ns].name))),
                    ScopeHit::Class(c) => {
                        for k in self.class_chain(c) {
                            if self.classes[k as usize].statics.borrow().contains_key(name) {
                                return Ok(LPath { base: Place::Static(k, name.clone()), sels: vec![] });
                            }
                            if let Some(ParamVal::Val(v)) = self.classes[k as usize].params.get(name) {
                                return Ok(LPath { base: Place::Const(v.clone(), Ty::int(32, true)), sels: vec![] });
                            }
                        }
                        Err(self.rt(format!("class `{}` has no static member `{name}`", self.classes[c as usize].name)))
                    }
                }
            }
            Expr::Member(b, n) => {
                // Clocking block variable: `vif.cb.sig`.
                if let Expr::Member(bb, cb) = b.as_ref() {
                    if let Ok(Value::VIf(Some(i))) = self.eval_quiet(ctx, bb) {
                        if i.clocking.contains_key(cb) {
                            return Ok(LPath { base: Place::Clock(i, cb.clone(), n.clone()), sels: vec![] });
                        }
                    }
                }
                if matches!(b.as_ref(), Expr::This | Expr::Super) {
                    let mut c2 = ctx.clone();
                    c2.scopes.clear();
                    return match self.lookup_var(&c2, n) {
                        Some(p) => Ok(LPath { base: p, sels: vec![] }),
                        None => Err(self.rt(format!("no member `{n}` in this class"))),
                    };
                }
                let bp = self.lpath(ctx, b).ok();
                let (bv, bty) = match &bp {
                    Some(p) => self.read_path(p)?,
                    None => {
                        let v = self.eval(ctx, b)?;
                        (v, Ty::Void)
                    }
                };
                match bv {
                    Value::Obj(Some(o)) => {
                        if o.borrow().fields.contains_key(n) {
                            return Ok(LPath { base: Place::Field(o, n.clone()), sels: vec![] });
                        }
                        let cls = o.borrow().class;
                        if let Some(c) = cls {
                            for k in self.class_chain(c) {
                                if self.classes[k as usize].statics.borrow().contains_key(n) {
                                    return Ok(LPath { base: Place::Static(k, n.clone()), sels: vec![] });
                                }
                            }
                            return Err(self.rt(format!("class `{}` has no member `{n}`", self.classes[c as usize].name)));
                        }
                        Err(self.rt(format!("built-in object has no member `{n}`")))
                    }
                    Value::Obj(None) => {
                        if let Ty::Class(c) = bty {
                            for k in self.class_chain(c) {
                                if self.classes[k as usize].statics.borrow().contains_key(n) {
                                    return Ok(LPath { base: Place::Static(k, n.clone()), sels: vec![] });
                                }
                            }
                        }
                        Err(self.rt(format!("null object access: `.{n}` on a null handle")))
                    }
                    Value::VIf(Some(i)) => match i.sigs.get(n) {
                        Some(&id) => Ok(LPath { base: Place::Sig(id), sels: vec![] }),
                        None => Err(self.rt(format!("interface `{}` has no signal `{n}`", i.decl.name))),
                    },
                    Value::VIf(None) => Err(self.rt(format!("null virtual interface access: `.{n}`"))),
                    _ => Err(self.rt(format!("`.{n}` on a value that has no members"))),
                }
            }
            Expr::Index(b, i) => {
                let mut p = self.lpath(ctx, b)?;
                let (cur, ty) = self.read_path(&p)?;
                let len = match &cur {
                    Value::Arr(a) => a.items.len() as i64,
                    _ => 0,
                };
                self.dollar.push(len - 1);
                let iv = self.eval(ctx, i);
                self.dollar.pop();
                let iv = iv?;
                match (&cur, &ty) {
                    (Value::Arr(_), _) | (Value::Assoc(_), _) | (Value::Str(_), _) => p.sels.push(Sel::Index(iv)),
                    _ => {
                        let k = iv.as_u64() as u32;
                        p.sels.push(Sel::Bits(k, k));
                    }
                }
                Ok(p)
            }
            Expr::Slice(b, a, c, kind) => {
                let mut p = self.lpath(ctx, b)?;
                let av = self.eval(ctx, a)?.as_i64();
                let cv = self.eval(ctx, c)?.as_i64();
                let (hi, lo) = match kind {
                    SliceKind::Fixed => (av.max(cv), av.min(cv)),
                    SliceKind::Up => (av + cv - 1, av),
                    SliceKind::Down => (av, av - cv + 1),
                };
                if lo < 0 || hi > 63 {
                    return Err(self.rt(format!("part-select [{hi}:{lo}] out of range")));
                }
                p.sels.push(Sel::Bits(hi as u32, lo as u32));
                Ok(p)
            }
            _ => {
                let v = self.eval(ctx, e)?;
                let t = match &v {
                    Value::Int(_, w, s) => Ty::int(*w, *s),
                    Value::Str(_) => Ty::Str,
                    _ => Ty::Void,
                };
                Ok(LPath { base: Place::Const(v, t), sels: vec![] })
            }
        }
    }

    /// Evaluate without surfacing errors (probing patterns).
    pub(crate) fn eval_quiet(&mut self, ctx: &Ctx, e: &Expr) -> Result<Value, SvError> {
        match e {
            Expr::Ident(n) => match self.lookup_var(ctx, n) {
                Some(p) => self.read_place(&p).map(|x| x.0),
                None => Err(self.rt("not a variable")),
            },
            Expr::Member(..) | Expr::Index(..) => {
                let p = self.lpath(ctx, e)?;
                self.read_path(&p).map(|x| x.0)
            }
            Expr::This => Ok(Value::Obj(ctx.this.clone())),
            _ => Err(self.rt("not a simple reference")),
        }
    }

    /// Assign with `lhs = rhs` semantics (typed `new`, patterns, queues).
    pub(crate) fn assign(&mut self, ctx: &Ctx, lhs: &Expr, rhs: &Expr) -> Result<Value, SvError> {
        if let Expr::Concat(parts) = lhs {
            // `{a, b} = v`: split by widths, MSB first.
            let v = self.eval(ctx, rhs)?.as_u64();
            let mut paths = Vec::new();
            let mut total = 0;
            for p in parts {
                let lp = self.lpath(ctx, p)?;
                let (_, t) = self.read_path(&lp)?;
                total += t.width();
                paths.push((lp, t.width()));
            }
            let mut sh = total;
            for (lp, w) in paths {
                sh -= w;
                self.write_path(&lp, Value::Int(mask(v >> sh, w), w, false))?;
            }
            return Ok(Value::Int(v, total, false));
        }
        let p = self.lpath(ctx, lhs)?;
        let (_, ty) = self.read_path(&p)?;
        let v = self.eval_to(ctx, rhs, &ty)?;
        self.write_path(&p, v.clone())?;
        Ok(v)
    }

    /// Evaluate `e` for storage in a slot of type `ty`.
    pub(crate) fn eval_to(&mut self, ctx: &Ctx, e: &Expr, ty: &Ty) -> Result<Value, SvError> {
        match e {
            Expr::New { args, size: None, src: None, copy: None, loc } => {
                self.cur_loc = *loc;
                match ty {
                    Ty::Class(c) => {
                        let vals = self.eval_args_plain(ctx, args)?;
                        let o = self.new_object(*c, vals)?;
                        Ok(Value::Obj(Some(o)))
                    }
                    Ty::Builtin(b) => {
                        let n = match args.first() {
                            Some(Some(a)) => self.eval(ctx, a)?.as_u64(),
                            _ => 0,
                        };
                        let id = self.new_obj_id();
                        let mut o = Object::new(id, None, Some(*b));
                        o.native = match b {
                            Builtin::Mailbox => Native::Mailbox { q: Default::default(), bound: n as usize },
                            Builtin::Semaphore => Native::Semaphore { keys: n },
                            Builtin::Process => return Err(self.rt("process objects cannot be created with new")),
                        };
                        Ok(Value::Obj(Some(Rc::new(std::cell::RefCell::new(o)))))
                    }
                    Ty::Event => Ok(Value::Event(Rc::new(EventCell::new()))),
                    other => Err(self.rt(format!("`new` needs a class-typed target, not {}", self.ty_name(other)))),
                }
            }
            Expr::New { size: Some(n), src, .. } => {
                let n = self.eval(ctx, n)?.as_i64().max(0) as usize;
                let et = ty.elem().cloned().unwrap_or(Ty::int(32, true));
                let mut items: Vec<Value> = (0..n).map(|_| et.default_value()).collect();
                if let Some(s) = src {
                    if let Value::Arr(a) = self.eval(ctx, s)? {
                        for (i, v) in a.items.into_iter().enumerate().take(n) {
                            items[i] = v;
                        }
                    }
                }
                Ok(Value::Arr(Box::new(ArrVal { kind: ArrKind::Dynamic, items })))
            }
            Expr::New { copy: Some(src), .. } => {
                let v = self.eval(ctx, src)?;
                match v {
                    Value::Obj(Some(o)) => {
                        let id = self.new_obj_id();
                        let b = o.borrow();
                        let mut n = Object::new(id, b.class, b.builtin);
                        n.fields = b.fields.clone();
                        Ok(Value::Obj(Some(Rc::new(std::cell::RefCell::new(n)))))
                    }
                    Value::Obj(None) => Err(self.rt("shallow copy `new h` of a null handle")),
                    _ => Err(self.rt("shallow copy needs a class handle")),
                }
            }
            Expr::Pattern(items, default) if matches!(ty, Ty::Arr(..) | Ty::Assoc(..)) => {
                let et = ty.elem().cloned().unwrap_or(Ty::int(32, true));
                let mut vals = Vec::new();
                for it in items {
                    match it {
                        Expr::Repl(n, inner) => {
                            let n = self.eval(ctx, n)?.as_u64();
                            for _ in 0..n {
                                for x in inner {
                                    vals.push(self.eval_to(ctx, x, &et)?);
                                }
                            }
                        }
                        x => vals.push(self.eval_to(ctx, x, &et)?),
                    }
                }
                match ty {
                    Ty::Arr(_, k, n) => {
                        if let Some(d) = default {
                            let dv = self.eval_to(ctx, d, &et)?;
                            vals.resize(*n, dv);
                        }
                        Ok(convert(Value::Arr(Box::new(ArrVal { kind: *k, items: vals })), ty))
                    }
                    _ => Ok(ty.default_value()),
                }
            }
            Expr::Concat(parts) if matches!(ty, Ty::Arr(..)) => {
                let et = ty.elem().cloned().unwrap_or(Ty::int(32, true));
                let mut vals = Vec::new();
                for p in parts {
                    let v = self.eval(ctx, p)?;
                    match v {
                        Value::Arr(a) => vals.extend(a.items),
                        other => vals.push(convert(other, &et)),
                    }
                }
                let k = match ty {
                    Ty::Arr(_, k, _) => *k,
                    _ => ArrKind::Queue,
                };
                Ok(convert(Value::Arr(Box::new(ArrVal { kind: k, items: vals })), ty))
            }
            Expr::Cond(c, a, b) => {
                if self.eval(ctx, c)?.truthy() { self.eval_to(ctx, a, ty) } else { self.eval_to(ctx, b, ty) }
            }
            Expr::Null => Ok(match ty {
                Ty::VIf(_) => Value::VIf(None),
                _ => Value::Obj(None),
            }),
            _ => {
                let v = self.eval(ctx, e)?;
                Ok(convert(v, ty))
            }
        }
    }

    pub(crate) fn eval_args_plain(&mut self, ctx: &Ctx, args: &[Option<Expr>]) -> Result<Vec<Option<Value>>, SvError> {
        let mut v = Vec::new();
        for a in args {
            v.push(match a {
                Some(e) => Some(self.eval(ctx, e)?),
                None => None,
            });
        }
        Ok(v)
    }

    // ---- expressions --------------------------------------------------------------

    pub(crate) fn eval(&mut self, ctx: &Ctx, e: &Expr) -> Result<Value, SvError> {
        match e {
            Expr::Num { val, width, signed } => {
                let w = width.unwrap_or(32);
                let w = if width.is_none() && *val > u32::MAX as u64 { 64 } else { w };
                Ok(Value::Int(mask(*val, w), w, *signed))
            }
            Expr::Fill(b) => Ok(Value::Int(if *b == 1 { u64::MAX } else { 0 }, 64, false)),
            Expr::Real(r) => Ok(Value::Real(*r)),
            Expr::Time(ps) => Ok(Value::Real(*ps as f64 / 1000.0)),
            Expr::Str(s) => Ok(Value::Str(s.clone())),
            Expr::Null => Ok(Value::Obj(None)),
            Expr::This => Ok(Value::Obj(ctx.this.clone())),
            Expr::Super => Ok(Value::Obj(ctx.this.clone())),
            Expr::Dollar => Ok(Value::int(*self.dollar.last().ok_or_else(|| self.rt("`$` outside a queue index"))?)),
            Expr::Ident(n) => {
                if let Some(p) = self.lookup_var(ctx, n) {
                    return self.read_place(&p).map(|x| x.0);
                }
                // A function called without parentheses.
                let call = Expr::Call { f: Box::new(e.clone()), args: vec![], named: vec![], with: None, loc: self.cur_loc };
                if self.has_callable(ctx, n) {
                    return self.eval_call(ctx, &call);
                }
                Err(self.rt(format!("unknown identifier `{n}`")))
            }
            Expr::Scoped(path) => match self.lpath(ctx, e) {
                Ok(p) => self.read_path(&p).map(|x| x.0),
                Err(err) => {
                    let call = Expr::Call { f: Box::new(Expr::Scoped(path.clone())), args: vec![], named: vec![], with: None, loc: self.cur_loc };
                    self.eval_call(ctx, &call).map_err(|_| err)
                }
            },
            Expr::Member(b, n) => {
                // Event `.triggered`, enum / string / array zero-arg methods.
                if n == "triggered" {
                    if let Value::Event(ev) = self.eval(ctx, b)? {
                        return Ok(Value::bit(ev.last.get() == Some(self.time)));
                    }
                }
                match self.lpath(ctx, e) {
                    Ok(p) => self.read_path(&p).map(|x| x.0),
                    Err(err) => {
                        let call = Expr::Call { f: Box::new(e.clone()), args: vec![], named: vec![], with: None, loc: self.cur_loc };
                        match self.eval_call(ctx, &call) {
                            Ok(v) => Ok(v),
                            Err(e2) if e2.msg.starts_with("no method") || e2.msg.contains("has no method") => Err(err),
                            Err(e2) => Err(e2),
                        }
                    }
                }
            }
            Expr::Index(b, i) => {
                let bv = self.eval(ctx, b)?;
                let len = match &bv {
                    Value::Arr(a) => a.items.len() as i64,
                    Value::Str(s) => s.len() as i64,
                    _ => 0,
                };
                self.dollar.push(len - 1);
                let iv = self.eval(ctx, i);
                self.dollar.pop();
                let iv = iv?;
                let ty = match &bv {
                    Value::Arr(_) | Value::Assoc(_) => match self.lpath(ctx, b) {
                        Ok(p) => self.read_path(&p)?.1,
                        Err(_) => Ty::Void,
                    },
                    _ => Ty::int(bv.width(), false),
                };
                if let (Value::Assoc(m), Ty::Assoc(_, sk)) = (&bv, &ty) {
                    if !m.contains_key(&iv.to_key(*sk)) {
                        self.log(Sev::Warning, format!("{}:{}: read of nonexistent associative array entry {}", self.label(self.cur_loc.file), self.cur_loc.line, show(&iv)));
                    }
                }
                self.select(&bv, &ty, &Sel::Index(iv)).map(|x| x.0)
            }
            Expr::Slice(b, a, c, kind) => {
                let bv = self.eval(ctx, b)?;
                if let Value::Arr(arr) = &bv {
                    self.dollar.push(arr.items.len() as i64 - 1);
                    let av = self.eval(ctx, a);
                    let cv = self.eval(ctx, c);
                    self.dollar.pop();
                    let (av, cv) = (av?.as_i64(), cv?.as_i64());
                    let (lo, hi) = match kind {
                        SliceKind::Fixed => (av, cv),
                        SliceKind::Up => (av, av + cv - 1),
                        SliceKind::Down => (av - cv + 1, av),
                    };
                    let items: Vec<Value> = if lo <= hi && lo >= 0 {
                        arr.items.iter().skip(lo as usize).take((hi - lo + 1) as usize).cloned().collect()
                    } else {
                        vec![]
                    };
                    return Ok(Value::Arr(Box::new(ArrVal { kind: ArrKind::Queue, items })));
                }
                if let Value::Str(s) = &bv {
                    // not standard, but convenient: s[a:b] substring
                    let (a, c) = (self.eval(ctx, a)?.as_i64(), self.eval(ctx, c)?.as_i64());
                    return Ok(Value::Str(s.chars().skip(a.max(0) as usize).take((c - a + 1).max(0) as usize).collect()));
                }
                let av = self.eval(ctx, a)?.as_i64();
                let cv = self.eval(ctx, c)?.as_i64();
                let (hi, lo) = match kind {
                    SliceKind::Fixed => (av.max(cv), av.min(cv)),
                    SliceKind::Up => (av + cv - 1, av),
                    SliceKind::Down => (av, av - cv + 1),
                };
                if lo < 0 || hi > 63 {
                    return Err(self.rt(format!("part-select [{hi}:{lo}] out of range")));
                }
                self.select(&bv, &Ty::Void, &Sel::Bits(hi as u32, lo as u32)).map(|x| x.0)
            }
            Expr::Call { .. } => self.eval_call(ctx, e),
            Expr::Sys { name, args, loc } => {
                let saved = self.cur_loc;
                if loc.line != 0 {
                    self.cur_loc = *loc;
                }
                let r = self.sys_call(ctx, name, args, false);
                self.cur_loc = saved;
                r
            }
            Expr::Unary(op, x) => {
                let v = self.eval(ctx, x)?;
                Ok(self.unary(*op, v))
            }
            Expr::Binary(op, a, b) => {
                if *op == BinOp::LAnd {
                    let l = self.eval(ctx, a)?;
                    if !l.truthy() {
                        return Ok(Value::bit(false));
                    }
                    return Ok(Value::bit(self.eval(ctx, b)?.truthy()));
                }
                if *op == BinOp::LOr {
                    let l = self.eval(ctx, a)?;
                    if l.truthy() {
                        return Ok(Value::bit(true));
                    }
                    return Ok(Value::bit(self.eval(ctx, b)?.truthy()));
                }
                let l = self.eval(ctx, a)?;
                let r = self.eval(ctx, b)?;
                self.binary(*op, l, r)
            }
            Expr::Cond(c, a, b) => {
                if self.eval(ctx, c)?.truthy() { self.eval(ctx, a) } else { self.eval(ctx, b) }
            }
            Expr::Inside(x, items) => {
                let v = self.eval(ctx, x)?;
                for it in items {
                    if self.inside_item(ctx, &v, it)? {
                        return Ok(Value::bit(true));
                    }
                }
                Ok(Value::bit(false))
            }
            Expr::Concat(parts) => {
                let mut vals = Vec::new();
                for p in parts {
                    vals.push(self.eval(ctx, p)?);
                }
                if vals.iter().any(|v| matches!(v, Value::Str(_))) {
                    return Ok(Value::Str(vals.iter().map(|v| v.as_str()).collect()));
                }
                if vals.iter().any(|v| matches!(v, Value::Arr(_))) {
                    let mut items = Vec::new();
                    for v in vals {
                        match v {
                            Value::Arr(a) => items.extend(a.items),
                            o => items.push(o),
                        }
                    }
                    return Ok(Value::Arr(Box::new(ArrVal { kind: ArrKind::Queue, items })));
                }
                let mut acc = 0u64;
                let mut w = 0u32;
                for v in vals {
                    let vw = v.width();
                    acc = if vw >= 64 { v.as_u64() } else { (acc << vw) | mask(v.as_u64(), vw) };
                    w += vw;
                }
                if w > 64 {
                    return Err(self.rt(format!("concatenation is {w} bits wide (testbench values are limited to 64 bits)")));
                }
                Ok(Value::Int(acc, w.max(1), false))
            }
            Expr::Repl(n, parts) => {
                let n = self.eval(ctx, n)?.as_u64();
                let inner = self.eval(ctx, &Expr::Concat(parts.clone()))?;
                if let Value::Str(s) = &inner {
                    return Ok(Value::Str(s.repeat(n as usize)));
                }
                let vw = inner.width();
                let mut acc = 0u64;
                let total = vw as u64 * n;
                if total > 64 {
                    return Err(self.rt(format!("replication is {total} bits wide (testbench values are limited to 64 bits)")));
                }
                for _ in 0..n {
                    acc = (acc << vw) | inner.as_u64();
                }
                Ok(Value::Int(acc, total.max(1) as u32, false))
            }
            Expr::New { loc, .. } => {
                self.cur_loc = *loc;
                Err(self.rt("`new` here has no class-typed target; assign it to a class handle"))
            }
            Expr::Cast(to, x) => match to {
                CastTo::Type(dt) => {
                    let ty = self.resolve_type(ctx.tyscope(), dt, &[], None)?;
                    if matches!(ty, Ty::Void) {
                        self.eval(ctx, x)?;
                        return Ok(Value::Int(0, 1, false));
                    }
                    let v = self.eval(ctx, x)?;
                    if let (Ty::Class(c), Value::Obj(Some(o))) = (&ty, &v) {
                        let oc = o.borrow().class;
                        if !oc.is_some_and(|oc| self.is_subclass(oc, *c)) {
                            return Err(self.rt("static cast to an unrelated class type; use $cast"));
                        }
                    }
                    Ok(convert(v, &ty))
                }
                CastTo::Width(w) => {
                    let w = self.eval(ctx, w)?.as_u64() as u32;
                    let v = self.eval(ctx, x)?;
                    Ok(Value::Int(mask(v.as_u64(), w), w, v.signed()))
                }
                CastTo::Signed(s) => {
                    let v = self.eval(ctx, x)?;
                    Ok(match v {
                        Value::Int(x, w, _) => Value::Int(x, w, *s),
                        o => o,
                    })
                }
            },
            Expr::Pattern(items, _) => {
                let mut v = Vec::new();
                for i in items {
                    v.push(self.eval(ctx, i)?);
                }
                Ok(Value::Arr(Box::new(ArrVal { kind: ArrKind::Queue, items: v })))
            }
            Expr::IncDec { e: x, inc, prefix } => {
                let p = self.lpath(ctx, x)?;
                let (old, _) = self.read_path(&p)?;
                let one = Value::Int(1, 1, false);
                let new = self.binary(if *inc { BinOp::Add } else { BinOp::Sub }, old.clone(), one)?;
                self.write_path(&p, new.clone())?;
                Ok(if *prefix { new } else { old })
            }
            Expr::AssignExpr(l, r) => self.assign(ctx, l, r),
            Expr::Type(_) => Err(self.rt("a type is not a value here")),
        }
    }

    pub(crate) fn inside_item(&mut self, ctx: &Ctx, v: &Value, it: &InsideItem) -> Result<bool, SvError> {
        match it {
            InsideItem::One(x) => {
                let w = self.eval(ctx, x)?;
                if let Value::Arr(a) = &w {
                    return Ok(a.items.iter().any(|i| i == v));
                }
                Ok(self.binary(BinOp::Eq, v.clone(), w)?.truthy())
            }
            InsideItem::Range(a, b) => {
                let lo = self.eval(ctx, a)?;
                let hi = self.eval(ctx, b)?;
                Ok(self.binary(BinOp::Ge, v.clone(), lo)?.truthy() && self.binary(BinOp::Le, v.clone(), hi)?.truthy())
            }
        }
    }

    pub(crate) fn unary(&self, op: UnOp, v: Value) -> Value {
        if let Value::Real(r) = v {
            return match op {
                UnOp::Neg => Value::Real(-r),
                UnOp::Not => Value::bit(r == 0.0),
                _ => Value::Real(r),
            };
        }
        let (x, w, s) = match &v {
            Value::Int(x, w, s) => (*x, *w, *s),
            o => (o.as_u64(), o.width(), false),
        };
        let ones = (x & mask(u64::MAX, w)).count_ones();
        match op {
            UnOp::Neg => Value::Int(mask(x.wrapping_neg(), w), w, s),
            UnOp::Plus => v,
            UnOp::Not => Value::bit(!v.truthy()),
            UnOp::BitNot => Value::Int(mask(!x, w), w, s),
            UnOp::RedAnd => Value::bit(mask(x, w) == mask(u64::MAX, w)),
            UnOp::RedOr => Value::bit(mask(x, w) != 0),
            UnOp::RedXor => Value::bit(ones % 2 == 1),
            UnOp::RedNand => Value::bit(mask(x, w) != mask(u64::MAX, w)),
            UnOp::RedNor => Value::bit(mask(x, w) == 0),
            UnOp::RedXnor => Value::bit(ones % 2 == 0),
        }
    }

    pub(crate) fn binary(&self, op: BinOp, l: Value, r: Value) -> Result<Value, SvError> {
        use BinOp::*;
        // Handles, strings, reals.
        match (&l, &r) {
            (Value::Obj(_), _) | (_, Value::Obj(_)) | (Value::VIf(_), _) | (_, Value::VIf(_)) | (Value::Event(_), _) | (_, Value::Event(_)) => {
                let eq = l == r || (matches!(l, Value::VIf(None)) && matches!(r, Value::Obj(None))) || (matches!(r, Value::VIf(None)) && matches!(l, Value::Obj(None)));
                return match op {
                    Eq | CaseEq => Ok(Value::bit(eq)),
                    Ne | CaseNe => Ok(Value::bit(!eq)),
                    _ => Err(self.rt("only == and != apply to handles")),
                };
            }
            (Value::Str(a), Value::Str(b)) => {
                return Ok(Value::bit(match op {
                    Eq | CaseEq => a == b,
                    Ne | CaseNe => a != b,
                    Lt => a < b,
                    Le => a <= b,
                    Gt => a > b,
                    Ge => a >= b,
                    _ => return Err(self.rt("unsupported operator on strings")),
                }));
            }
            (Value::Str(_), _) | (_, Value::Str(_)) if matches!(op, Eq | Ne | CaseEq | CaseNe) => {
                let eq = l.as_str() == r.as_str() || l.as_u64() == r.as_u64();
                return Ok(Value::bit(if matches!(op, Eq | CaseEq) { eq } else { !eq }));
            }
            (Value::Arr(a), Value::Arr(b)) if matches!(op, Eq | Ne | CaseEq | CaseNe) => {
                let eq = a.items == b.items;
                return Ok(Value::bit(if matches!(op, Eq | CaseEq) { eq } else { !eq }));
            }
            (Value::Real(_), _) | (_, Value::Real(_)) => {
                let (a, b) = (l.as_f64(), r.as_f64());
                return Ok(match op {
                    Add => Value::Real(a + b),
                    Sub => Value::Real(a - b),
                    Mul => Value::Real(a * b),
                    Div => Value::Real(a / b),
                    Pow => Value::Real(a.powf(b)),
                    Eq | CaseEq => Value::bit(a == b),
                    Ne | CaseNe => Value::bit(a != b),
                    Lt => Value::bit(a < b),
                    Le => Value::bit(a <= b),
                    Gt => Value::bit(a > b),
                    Ge => Value::bit(a >= b),
                    _ => Value::Real(((a as i64) % (b as i64).max(1)) as f64),
                });
            }
            _ => {}
        }
        let (a, wa, sa) = match l {
            Value::Int(x, w, s) => (x, w, s),
            ref o => (o.as_u64(), o.width(), false),
        };
        let (b, wb, sb) = match r {
            Value::Int(x, w, s) => (x, w, s),
            ref o => (o.as_u64(), o.width(), false),
        };
        let w = wa.max(wb);
        let s = sa && sb;
        let ia = if s { sext(a, wa) } else { a as i64 };
        let ib = if s { sext(b, wb) } else { b as i64 };
        let res = |v: u64| Value::Int(mask(v, w), w, s);
        Ok(match op {
            Add => res((ia as u64).wrapping_add(ib as u64)),
            Sub => res((ia as u64).wrapping_sub(ib as u64)),
            Mul => res((ia as u64).wrapping_mul(ib as u64)),
            Div => {
                if b == 0 {
                    res(0)
                } else if s {
                    res(ia.wrapping_div(ib) as u64)
                } else {
                    res(mask(a, wa) / mask(b, wb))
                }
            }
            Mod => {
                if b == 0 {
                    res(0)
                } else if s {
                    res(ia.wrapping_rem(ib) as u64)
                } else {
                    res(mask(a, wa) % mask(b, wb))
                }
            }
            Pow => res(pow_u(a, b)),
            And => res(a & b),
            Or => res(a | b),
            Xor => res(a ^ b),
            Xnor => res(!(a ^ b)),
            Shl | AShl => Value::Int(mask(if b >= 64 { 0 } else { a << b }, wa), wa, sa),
            Shr => Value::Int(if b >= 64 { 0 } else { mask(a, wa) >> b }, wa, sa),
            AShr => {
                if sa {
                    Value::Int(mask((sext(a, wa) >> b.min(63)) as u64, wa), wa, sa)
                } else {
                    Value::Int(if b >= 64 { 0 } else { mask(a, wa) >> b }, wa, sa)
                }
            }
            Eq | CaseEq | WildEq => Value::bit(if s { ia == ib } else { mask(a, w) == mask(b, w) }),
            Ne | CaseNe | WildNe => Value::bit(if s { ia != ib } else { mask(a, w) != mask(b, w) }),
            Lt => Value::bit(if s { ia < ib } else { a < b }),
            Le => Value::bit(if s { ia <= ib } else { a <= b }),
            Gt => Value::bit(if s { ia > ib } else { a > b }),
            Ge => Value::bit(if s { ia >= ib } else { a >= b }),
            LAnd => Value::bit(a != 0 && b != 0),
            LOr => Value::bit(a != 0 || b != 0),
            Implies => Value::bit(a == 0 || b != 0),
            Equiv => Value::bit((a != 0) == (b != 0)),
        })
    }
}
