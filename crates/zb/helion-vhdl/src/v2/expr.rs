//! Expression evaluation: names, literals, aggregates, operators
//! (std_logic_1164 / numeric_std / std_logic_arith semantics), built-in
//! functions, attributes, type conversions and assignment targets.

use std::rc::Rc;

use super::ast::*;
use super::dag::*;
use super::elab::*;

#[derive(Clone, Debug)]
pub enum LBase {
    Net(u32),
    Var(u32),
}

#[derive(Clone, Debug)]
pub enum Off {
    Static(u32),
    /// Mutually exclusive (condition, bit offset) alternatives.
    Dyn(Vec<(N, u32)>),
}

#[derive(Clone, Debug)]
pub struct LVal {
    pub base: LBase,
    pub off: Off,
    pub ty: Ty,
}

const MAX_DYN: usize = 1 << 16;

fn is_untyped_lit(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Char(_)
            | Expr::Str(_)
            | Expr::BitStr(_)
            | Expr::Agg(_)
            | Expr::Int(_)
            | Expr::Real(_)
    )
}

/// A character/string literal containing metavalues other than 0/1/L/H.
fn meta_lit(e: &Expr) -> Option<String> {
    let s = match e {
        Expr::Char(c) => c.to_string(),
        Expr::Str(s) | Expr::BitStr(s) => s.clone(),
        _ => return None,
    };
    s.chars()
        .any(|c| matches!(c.to_ascii_lowercase(), 'u' | 'x' | 'z' | 'w' | '-'))
        .then_some(s)
}

fn bit_of(c: char) -> Option<bool> {
    match c.to_ascii_lowercase() {
        '1' | 'h' => Some(true),
        '0' | 'l' | 'u' | 'x' | 'z' | 'w' | '-' => Some(false),
        _ => None,
    }
}

fn slv_ty(n: u32, kind: ArrKind, elem: Ty) -> Ty {
    Ty::Arr(Rc::new(ArrTy {
        kind,
        elem,
        idx: Some(IdxRange::canonical(n)),
        idx_ty: Ty::Int { lo: 0, hi: INT_MAX },
    }))
}

impl Elab {
    pub fn dead(&mut self) -> N {
        let st = &self.px.last().unwrap().st;
        let (r, b, c) = (st.ret, st.brk, st.cont);
        let x = self.dag.or(r, b);
        self.dag.or(x, c)
    }

    // ------------------------------------------------------------------

    pub fn eval(&mut self, e: &Expr, want: Option<&Ty>, line: Line) -> R<Val> {
        match e {
            Expr::Int(v) => {
                if matches!(want, Some(Ty::Real)) {
                    return Ok(Val::R(*v as f64));
                }
                if *v > i64::MAX as i128 / 2 {
                    return err(line, "integer literal too large");
                }
                Ok(self.int_val(*v as i64))
            }
            Expr::Real(r) => Ok(Val::R(*r)),
            Expr::Phys(..) => Ok(Val::T),
            Expr::Char(c) => self.char_lit(*c, want, line),
            Expr::Str(s) => self.str_lit(s, want, line),
            Expr::BitStr(s) => {
                let w = match want {
                    Some(t @ Ty::Arr(_)) => Some(t),
                    _ => None,
                };
                self.str_lit(s, w, line)
            }
            Expr::Agg(elems) => self.aggregate(elems, want, line),
            Expr::Name(n) => self.eval_name(n, want, line),
            Expr::Bin(op, a, b) => self.binop(*op, a, b, want, line),
            Expr::Un(op, a) => self.unop(*op, a, line),
            Expr::Qual(t, x) => {
                let ty = self.resolve_mark(t, line)?;
                let v = self.eval(x, Some(&ty), line)?;
                let ty2 = self.constrain_from(&ty, &v, line)?;
                self.convert(v, &ty2, line)
            }
            Expr::Open => err(line, "`open` used as a value"),
            Expr::Null => err(line, "`null` used as a value"),
        }
    }

    pub fn eval_bits(&mut self, e: &Expr, want: Option<&Ty>, line: Line) -> R<(Ty, N)> {
        match self.eval(e, want, line)? {
            Val::B { ty, n } => Ok((ty, n)),
            Val::R(_) => err(line, "real value used where hardware is needed"),
            Val::S(_) => err(line, "string value used where hardware is needed"),
            Val::T => err(line, "time value used where hardware is needed"),
        }
    }

    pub fn cond(&mut self, e: &Expr, line: Line) -> R<N> {
        let (t, n) = self.eval_bits(e, Some(&Ty::Bool), line)?;
        match t {
            Ty::Bool | Ty::Logic | Ty::Bit => Ok(n),
            t => err(line, format!("condition has type {}", t.name())),
        }
    }

    fn char_lit(&mut self, c: char, want: Option<&Ty>, line: Line) -> R<Val> {
        let target = match want {
            Some(Ty::Arr(a)) => a.elem.clone(),
            Some(t) => t.clone(),
            None => Ty::Logic,
        };
        match &target {
            Ty::Enum(e) => {
                let key = format!("'{c}'");
                if let Some(i) = e.lits.iter().position(|l| *l == key) {
                    let w = target.width()?;
                    let n = self.dag.kval(i as u128, w);
                    return Ok(Val::B {
                        ty: target.clone(),
                        n,
                    });
                }
                err(line, format!("'{c}' is not a literal of {}", e.name))
            }
            Ty::Char => Ok(Val::S(c.to_string())),
            Ty::Bit => {
                let b = bit_of(c).ok_or_else(|| format!("line {line}: bad bit literal '{c}'"))?;
                let n = self.dag.bit(b);
                Ok(Val::B { ty: Ty::Bit, n })
            }
            _ => {
                let b =
                    bit_of(c).ok_or_else(|| format!("line {line}: bad std_logic literal '{c}'"))?;
                let n = self.dag.bit(b);
                Ok(Val::B { ty: Ty::Logic, n })
            }
        }
    }

    fn str_lit(&mut self, s: &str, want: Option<&Ty>, line: Line) -> R<Val> {
        if matches!(want, Some(Ty::Str)) {
            return Ok(Val::S(s.to_string()));
        }
        let arr = match want {
            Some(Ty::Arr(a)) => Some(a.clone()),
            _ => None,
        };
        let n = s.chars().count() as u32;
        let (kind, elem, idx_ty, given) = match &arr {
            Some(a) => (a.kind.clone(), a.elem.clone(), a.idx_ty.clone(), a.idx),
            None => {
                if !s.chars().all(|c| bit_of(c).is_some()) {
                    return Ok(Val::S(s.to_string()));
                }
                (
                    ArrKind::Slv,
                    Ty::Logic,
                    Ty::Int { lo: 0, hi: INT_MAX },
                    None,
                )
            }
        };
        let mut bits = Vec::with_capacity(n as usize);
        match &elem {
            Ty::Logic | Ty::Bit => {
                for c in s.chars().rev() {
                    bits.push(
                        bit_of(c).ok_or_else(|| {
                            format!("line {line}: bad bit '{c}' in string literal")
                        })?,
                    );
                }
            }
            Ty::Enum(e) => {
                let w = elem.width()?;
                for c in s.chars().rev() {
                    let key = format!("'{c}'");
                    let i = e.lits.iter().position(|l| *l == key).ok_or_else(|| {
                        format!("line {line}: '{c}' is not a literal of {}", e.name)
                    })?;
                    bits.extend(from_u128(i as u128, w));
                }
            }
            Ty::Char => return Ok(Val::S(s.to_string())),
            t => return err(line, format!("string literal for an array of {}", t.name())),
        }
        let idx = match given {
            Some(r) if r.len() == n => r,
            _ => {
                let lo = match &idx_ty {
                    Ty::Int { lo, .. } => *lo,
                    _ => 0,
                };
                IdxRange {
                    left: lo,
                    right: lo + n as i64 - 1,
                    down: false,
                }
            }
        };
        let node = self.dag.konst(bits);
        Ok(Val::B {
            ty: Ty::Arr(Rc::new(ArrTy {
                kind,
                elem,
                idx: Some(idx),
                idx_ty,
            })),
            n: node,
        })
    }

    fn aggregate(&mut self, elems: &[AggElem], want: Option<&Ty>, line: Line) -> R<Val> {
        let want = match want {
            Some(t @ (Ty::Arr(_) | Ty::Rec(_))) => t.clone(),
            _ => {
                // Positional aggregate of bits without context: std_logic_vector.
                if elems.iter().all(|e| e.choices.is_empty()) {
                    let mut parts = Vec::new();
                    for e in elems {
                        let (t, n) = self.eval_bits(&e.val, Some(&Ty::Logic), line)?;
                        if !t.is_bit() {
                            return err(
                                line,
                                "cannot determine the type of this aggregate (qualify it, e.g. `t'(...)`)",
                            );
                        }
                        parts.push(n);
                    }
                    let n = self.dag.concat(parts);
                    let w = self.dag.w(n);
                    let ty = Ty::Arr(Rc::new(ArrTy {
                        kind: ArrKind::Slv,
                        elem: Ty::Logic,
                        idx: Some(IdxRange {
                            left: 0,
                            right: w as i64 - 1,
                            down: false,
                        }),
                        idx_ty: Ty::Int { lo: 0, hi: INT_MAX },
                    }));
                    return Ok(Val::B { ty, n });
                }
                return err(
                    line,
                    "cannot determine the type of this aggregate (qualify it, e.g. `t'(...)`)",
                );
            }
        };
        match &want {
            Ty::Rec(r) => {
                let mut vals: Vec<Option<N>> = vec![None; r.fields.len()];
                let mut pos = 0usize;
                for e in elems {
                    if e.choices.is_empty() {
                        if pos >= r.fields.len() {
                            return err(line, "too many elements in record aggregate");
                        }
                        let ft = r.fields[pos].1.clone();
                        let v = self.eval(&e.val, Some(&ft), line)?;
                        let v = self.convert(v, &ft, line)?;
                        vals[pos] = Some(bits_n(&v, line)?);
                        pos += 1;
                        continue;
                    }
                    for ch in &e.choices {
                        let idxs: Vec<usize> = match ch {
                            Choice::Others => {
                                (0..r.fields.len()).filter(|&k| vals[k].is_none()).collect()
                            }
                            Choice::Expr(Expr::Name(n)) if n.simple().is_some() => {
                                let f = n.simple().unwrap();
                                vec![r.fields.iter().position(|(x, _)| x == f).ok_or_else(
                                    || format!("line {line}: record {} has no field `{f}`", r.name),
                                )?]
                            }
                            _ => return err(line, "bad record aggregate choice"),
                        };
                        for k in idxs {
                            let ft = r.fields[k].1.clone();
                            let v = self.eval(&e.val, Some(&ft), line)?;
                            let v = self.convert(v, &ft, line)?;
                            vals[k] = Some(bits_n(&v, line)?);
                        }
                    }
                }
                let mut parts = Vec::new();
                for (k, v) in vals.into_iter().enumerate() {
                    parts.push(v.ok_or_else(|| {
                        format!(
                            "line {line}: field `{}` missing from record aggregate",
                            r.fields[k].0
                        )
                    })?);
                }
                let n = self.dag.concat(parts);
                Ok(Val::B {
                    ty: want.clone(),
                    n,
                })
            }
            Ty::Arr(a) => {
                let et = a.elem.clone();
                let ew = et.width().map_err(|e| format!("line {line}: {e}"))?;
                let positional = elems.iter().all(|e| {
                    e.choices.is_empty() || matches!(e.choices.as_slice(), [Choice::Others])
                });
                if positional && elems.iter().any(|e| e.choices.is_empty()) {
                    // Positional elements (possibly array slices, VHDL-2008), then optional others.
                    let mut parts: Vec<N> = Vec::new();
                    let mut count = 0u32;
                    let mut others: Option<N> = None;
                    for e in elems {
                        if !e.choices.is_empty() {
                            let v = self.eval(&e.val, Some(&et), line)?;
                            let v = self.convert(v, &et, line)?;
                            others = Some(bits_n(&v, line)?);
                            continue;
                        }
                        let v = self.eval(&e.val, Some(&et), line)?;
                        let vt = v.ty();
                        let is_slice = matches!(&vt, Ty::Arr(va) if va.elem.same_base(&et))
                            && !matches!(et, Ty::Arr(_));
                        if is_slice {
                            let n = bits_n(&v, line)?;
                            count += self.dag.w(n) / ew.max(1);
                            parts.push(n);
                        } else {
                            let v = self.convert(v, &et, line)?;
                            parts.push(bits_n(&v, line)?);
                            count += 1;
                        }
                    }
                    let idx = match a.idx {
                        Some(r) => {
                            if let Some(o) = others {
                                while count < r.len() {
                                    parts.push(o);
                                    count += 1;
                                }
                            }
                            if count != r.len() {
                                return err(
                                    line,
                                    format!(
                                        "aggregate has {count} elements, target has {}",
                                        r.len()
                                    ),
                                );
                            }
                            r
                        }
                        None => {
                            if others.is_some() {
                                return err(
                                    line,
                                    "`others` in an aggregate whose bounds are unknown",
                                );
                            }
                            let lo = match &a.idx_ty {
                                Ty::Int { lo, .. } => *lo,
                                _ => 0,
                            };
                            IdxRange {
                                left: lo,
                                right: lo + count as i64 - 1,
                                down: false,
                            }
                        }
                    };
                    let n = self.dag.concat(parts);
                    return Ok(Val::B {
                        ty: want.with_idx(idx),
                        n,
                    });
                }
                // Named association.
                let r = match a.idx {
                    Some(r) => r,
                    None => {
                        let mut lo = i64::MAX;
                        let mut hi = i64::MIN;
                        for e in elems {
                            for ch in &e.choices {
                                match ch {
                                    Choice::Others => {
                                        return err(
                                            line,
                                            "`others` in an aggregate whose bounds are unknown",
                                        );
                                    }
                                    Choice::Expr(x) => {
                                        let v = self.eval_static(x, Some(&a.idx_ty), line)?;
                                        let i = self.val_to_int(&v).ok_or_else(|| {
                                            format!("line {line}: aggregate choice is not static")
                                        })?;
                                        lo = lo.min(i);
                                        hi = hi.max(i);
                                    }
                                    Choice::Range(rg) => {
                                        let (ir, _) = self.eval_range(rg, line)?;
                                        lo = lo.min(ir.lo());
                                        hi = hi.max(ir.hi());
                                    }
                                }
                            }
                        }
                        IdxRange {
                            left: lo,
                            right: hi,
                            down: false,
                        }
                    }
                };
                let n_el = r.len() as usize;
                let mut slots: Vec<Option<N>> = vec![None; n_el];
                let mut dyn_assign: Vec<(Val, N)> = Vec::new();
                let mut others: Option<N> = None;
                for e in elems {
                    let v = self.eval(&e.val, Some(&et), line)?;
                    let v = self.convert(v, &et, line)?;
                    let vn = bits_n(&v, line)?;
                    for ch in &e.choices {
                        match ch {
                            Choice::Others => others = Some(vn),
                            Choice::Expr(x) => {
                                let iv = self.eval(x, Some(&a.idx_ty), line)?;
                                match self.val_to_int(&iv) {
                                    Some(i) => {
                                        let p = r.pos(i).ok_or_else(|| {
                                            format!(
                                                "line {line}: aggregate index {i} outside {}",
                                                want.name()
                                            )
                                        })?;
                                        slots[p as usize] = Some(vn);
                                    }
                                    None => dyn_assign.push((iv, vn)),
                                }
                            }
                            Choice::Range(rg) => {
                                let (ir, _) = self.eval_range(rg, line)?;
                                for i in ir.iter() {
                                    let p = r.pos(i).ok_or_else(|| {
                                        format!(
                                            "line {line}: aggregate index {i} outside {}",
                                            want.name()
                                        )
                                    })?;
                                    slots[p as usize] = Some(vn);
                                }
                            }
                        }
                    }
                }
                let mut parts = Vec::with_capacity(n_el);
                for (p, s) in slots.iter().enumerate().rev() {
                    let base = match (s, others) {
                        (Some(x), _) => *x,
                        (None, Some(o)) => o,
                        (None, None) if !dyn_assign.is_empty() => {
                            let z = self.dag.zero(ew);
                            z
                        }
                        (None, None) => {
                            return err(
                                line,
                                format!("aggregate does not cover position {p} of {}", want.name()),
                            );
                        }
                    };
                    let mut x = base;
                    let idx_val = r.iter()[n_el - 1 - p];
                    for (iv, vn) in &dyn_assign {
                        let c = self.idx_eq(iv, idx_val, line)?;
                        x = self.dag.mux(c, *vn, x);
                    }
                    parts.push(x);
                }
                let n = self.dag.concat(parts);
                Ok(Val::B {
                    ty: want.with_idx(r),
                    n,
                })
            }
            _ => unreachable!(),
        }
    }

    // ------------------------------------------------------------------
    // Names

    pub fn is_pkg_prefix(&self, p: &Name) -> bool {
        match p {
            Name::Id(x, _) => matches!(self.lookup(x), Some(Sym::Pkg)),
            Name::Sel(q, y) => {
                self.is_pkg_prefix(q) && matches!(self.global.get(y.as_str()), Some(Sym::Pkg))
            }
            _ => false,
        }
    }

    pub fn eval_name(&mut self, n: &Name, want: Option<&Ty>, line: Line) -> R<Val> {
        match n {
            Name::Id(x, _) => self.eval_ident(x, want, line),
            Name::Sel(p, f) => {
                if self.is_pkg_prefix(p) {
                    return self.eval_ident(f, want, line);
                }
                let v = self.eval_name(p, None, line)?;
                self.field(v, f, line)
            }
            Name::Call(p, args) => self.eval_call(p, args, want, line),
            Name::Slice(p, r) => {
                let v = self.eval_name(p, None, line)?;
                self.slice_val(v, r, line)
            }
            Name::Attr(p, a, arg) => self.attr(p, a, arg.as_deref(), line),
            Name::OpSym(s) => err(line, format!("operator {s} used as a value")),
        }
    }

    fn eval_ident(&mut self, x: &str, want: Option<&Ty>, line: Line) -> R<Val> {
        match self.lookup(x).cloned() {
            Some(Sym::Sig(s)) => self.read_sig(&s),
            Some(Sym::Var(slot, ty)) => {
                let n = self
                    .px
                    .last()
                    .ok_or_else(|| format!("line {line}: variable outside a process"))?
                    .st
                    .vars[slot as usize];
                // the value escapes: nobody may change it in place any more
                self.px.last().unwrap().st.fresh.0.borrow_mut().remove(&n);
                Ok(Val::B { ty, n })
            }
            Some(Sym::Val(v)) => Ok(v),
            Some(Sym::Lits(l)) => {
                let pick = match want {
                    Some(w) => l.iter().find(|(t, _)| t.same_base(w)).or_else(|| l.first()),
                    None => l.first(),
                };
                let (t, i) = pick.cloned().unwrap();
                let w = t.width()?;
                let n = self.dag.kval(i as u128, w);
                Ok(Val::B { ty: t, n })
            }
            Some(Sym::Subs(subs)) => self
                .call_user(&subs, &[], want, true, line)?
                .ok_or_else(|| format!("line {line}: procedure `{x}` used as a value")),
            Some(Sym::Ty(_)) => err(line, format!("type `{x}` used as a value")),
            Some(Sym::Comp(_)) => err(line, format!("component `{x}` used as a value")),
            Some(Sym::Pkg) => err(line, format!("package `{x}` used as a value")),
            Some(Sym::Unsupported(w)) => err(line, format!("`{x}`: {w} is not supported")),
            Some(Sym::File(_) | Sym::Line(_)) => err(line, format!("std.textio `{x}` is only supported in readline, read, hread and endfile")),
            None => match x {
                "math_pi" => Ok(Val::R(std::f64::consts::PI)),
                "math_e" => Ok(Val::R(std::f64::consts::E)),
                "math_2_pi" => Ok(Val::R(2.0 * std::f64::consts::PI)),
                _ => {
                    if let Some(v) = self.builtin(x, &[], want, line)? {
                        return Ok(v);
                    }
                    err(line, format!("unknown name `{x}`"))
                }
            },
        }
    }

    fn field(&mut self, v: Val, f: &str, line: Line) -> R<Val> {
        let Val::B { ty: Ty::Rec(r), n } = v else {
            return err(line, format!("`.{f}` on a non-record value"));
        };
        let mut off = 0u32;
        for (name, t) in r.fields.iter().rev() {
            let w = t.width()?;
            if name == f {
                let s = self.dag.slice(n, off, w);
                return Ok(Val::B {
                    ty: t.clone(),
                    n: s,
                });
            }
            off += w;
        }
        err(line, format!("record {} has no field `{f}`", r.name))
    }

    fn arg_is_range(&self, a: &Assoc) -> Option<Range> {
        if a.formal.is_some() {
            return None;
        }
        match &a.actual {
            Expr::Name(n) => match &**n {
                Name::Attr(_, at, _) if at == "range" || at == "reverse_range" => {
                    Some(Range::Name(n.clone()))
                }
                Name::Id(x, _) if matches!(self.lookup(x), Some(Sym::Ty(_))) => {
                    Some(Range::Name(n.clone()))
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn eval_call(&mut self, p: &Name, args: &[Assoc], want: Option<&Ty>, line: Line) -> R<Val> {
        if let Name::OpSym(op) = p {
            return self.op_call(op, args, want, line);
        }
        let (fname, sym) = match p {
            Name::Id(x, _) => (Some(x.clone()), self.lookup(x).cloned()),
            Name::Sel(q, f) if self.is_pkg_prefix(q) => (Some(f.clone()), self.lookup(f).cloned()),
            _ => (None, None),
        };
        match sym {
            Some(Sym::Ty(t)) => {
                if let Some(r) = args.first().and_then(|a| self.arg_is_range(a)) {
                    // `t(range)` is not a conversion; treat as constrained subtype? unsupported
                    let _ = r;
                    return err(line, "subtype indication used as a value");
                }
                if args.len() != 1 {
                    return err(line, "type conversion takes one argument");
                }
                self.type_conv(&t, &args[0].actual, line)
            }
            Some(Sym::Subs(subs)) => self
                .call_user(&subs, args, want, true, line)?
                .ok_or_else(|| format!("line {line}: procedure used as a function")),
            Some(Sym::Unsupported(w)) => err(line, format!("{w} is not supported")),
            None | Some(Sym::Pkg) => {
                let f = fname.ok_or_else(|| format!("line {line}: bad call"))?;
                match self.builtin(&f, args, want, line)? {
                    Some(v) => Ok(v),
                    None => err(line, format!("unknown function `{f}`")),
                }
            }
            _ => {
                let v = self.eval_name(p, None, line)?;
                self.index_val(v, args, line)
            }
        }
    }

    fn index_val(&mut self, v: Val, args: &[Assoc], line: Line) -> R<Val> {
        if args.len() == 1 {
            if let Some(r) = self.arg_is_range(&args[0]) {
                return self.slice_val(v, &r, line);
            }
        }
        let Val::B { ty: Ty::Arr(a), n } = v else {
            if let Val::S(_) = v {
                return err(line, "indexing a string is not supported");
            }
            return err(line, "indexing a non-array value");
        };
        if args.len() != 1 {
            return err(line, "multi-dimensional indexing is not supported");
        }
        let r = a
            .idx
            .ok_or_else(|| format!("line {line}: indexing an unconstrained array"))?;
        let ew = a.elem.width()?;
        let iv = self.eval(&args[0].actual, Some(&a.idx_ty), line)?;
        if let Some(i) = self.val_to_int(&iv) {
            let p = r.pos(i).ok_or_else(|| {
                format!(
                    "line {line}: index {i} outside {}",
                    Ty::Arr(a.clone()).name()
                )
            })?;
            let s = self.dag.slice(n, p * ew, ew);
            return Ok(Val::B {
                ty: a.elem.clone(),
                n: s,
            });
        }
        // An array read by a natural index from 0 is a memory read: the
        // emitter makes a large constant array a ROM (block RAM behind) and
        // anything else a balanced mux tree (G97, G99).
        if let (Ty::Int { lo, .. }, Val::B { n: ix, .. }) = (iv.ty(), &iv) {
            if lo >= 0 && r.lo() == 0 {
                let x = self.dag.index(n, *ix, ew, !r.down);
                return Ok(Val::B { ty: a.elem.clone(), n: x });
            }
        }
        // Dynamic index: priority mux over the indices the index can take.
        let cands = self.index_candidates(&iv, &r);
        if cands.is_empty() {
            let z = self.dag.zero(ew);
            return Ok(Val::B {
                ty: a.elem.clone(),
                n: z,
            });
        }
        if cands.len() > MAX_DYN {
            return err(line, "dynamic index over too many elements");
        }
        let mut acc = {
            let (i, p) = *cands.last().unwrap();
            let _ = i;
            self.dag.slice(n, p * ew, ew)
        };
        for &(i, p) in cands.iter().rev().skip(1) {
            let c = self.idx_eq(&iv, i, line)?;
            let el = self.dag.slice(n, p * ew, ew);
            acc = self.dag.mux(c, el, acc);
        }
        Ok(Val::B {
            ty: a.elem.clone(),
            n: acc,
        })
    }

    /// (index value, element position) pairs reachable by a dynamic index.
    pub fn index_candidates(&self, iv: &Val, r: &IdxRange) -> Vec<(i64, u32)> {
        let (lo, hi) = match iv.ty() {
            Ty::Int { lo, hi } => (lo.max(r.lo()), hi.min(r.hi())),
            Ty::Enum(e) => (0.max(r.lo()), (e.lits.len() as i64 - 1).min(r.hi())),
            Ty::Logic | Ty::Bit | Ty::Bool => (0.max(r.lo()), 1.min(r.hi())),
            _ => (r.lo(), r.hi()),
        };
        let mut v = Vec::new();
        let mut i = lo;
        while i <= hi {
            if let Some(p) = r.pos(i) {
                v.push((i, p));
            }
            i += 1;
            if v.len() > MAX_DYN {
                break;
            }
        }
        v
    }

    /// 1-bit node: dynamic index value == k.
    pub fn idx_eq(&mut self, iv: &Val, k: i64, line: Line) -> R<N> {
        let (ty, n) = match iv {
            Val::B { ty, n } => (ty.clone(), *n),
            _ => return err(line, "non-hardware index"),
        };
        let w = self.dag.w(n);
        let signed = matches!(ty, Ty::Int { lo, .. } if lo < 0);
        if k < 0 && !signed {
            return Ok(self.dag.bit(false));
        }
        let need = if signed { bits_signed(k) } else { int_w(k, k) };
        if need > w {
            return Ok(self.dag.bit(false));
        }
        let kn = self.dag.kval(k as i128 as u128, w);
        Ok(self.dag.eq(n, kn))
    }

    fn slice_val(&mut self, v: Val, r: &Range, line: Line) -> R<Val> {
        let Val::B { ty: Ty::Arr(a), n } = v else {
            return err(line, "slicing a non-array value");
        };
        let ar = a
            .idx
            .ok_or_else(|| format!("line {line}: slicing an unconstrained array"))?;
        let (sr, _) = self.eval_range(r, line).map_err(|e| {
            if e.contains("not static") {
                format!("line {line}: slice bounds must be static")
            } else {
                e
            }
        })?;
        let ew = a.elem.width()?;
        if sr.len() == 0 {
            let z = self.dag.konst(Vec::new());
            return Ok(Val::B {
                ty: Ty::Arr(a.clone()).with_idx(sr),
                n: z,
            });
        }
        if sr.down != ar.down {
            return err(line, "slice direction differs from the array's");
        }
        let pl = ar
            .pos(sr.left)
            .ok_or_else(|| format!("line {line}: slice bound {} outside the array", sr.left))?;
        let pr = ar
            .pos(sr.right)
            .ok_or_else(|| format!("line {line}: slice bound {} outside the array", sr.right))?;
        let lo = pl.min(pr);
        let s = self.dag.slice(n, lo * ew, sr.len() * ew);
        Ok(Val::B {
            ty: Ty::Arr(a.clone()).with_idx(sr),
            n: s,
        })
    }

    pub fn prefix_ty(&mut self, p: &Name, line: Line) -> R<Ty> {
        match p {
            Name::Id(x, _) => {
                if let Some(Sym::Ty(t)) = self.lookup(x) {
                    return Ok(t.clone());
                }
            }
            Name::Sel(q, f) if self.is_pkg_prefix(q) => {
                if let Some(Sym::Ty(t)) = self.lookup(f) {
                    return Ok(t.clone());
                }
            }
            Name::Attr(_, a, _) if matches!(a.as_str(), "subtype" | "base" | "element") => {
                return self.resolve_mark(p, line);
            }
            _ => {}
        }
        let pushed = self.px.is_empty();
        if pushed {
            self.push_px(PxKind::Static, line, "attr".into());
        }
        let v = self.eval_name(p, None, line);
        if pushed {
            self.px.pop();
        }
        Ok(v?.ty())
    }

    fn attr(&mut self, p: &Name, a: &str, arg: Option<&Expr>, line: Line) -> R<Val> {
        match a {
            "length" | "high" | "low" | "left" | "right" | "ascending" => {
                let t = self.prefix_ty(p, line)?;
                let (r, it) = match &t {
                    Ty::Arr(at) => (
                        at.idx.ok_or_else(|| {
                            format!("line {line}: '{a} of an unconstrained array")
                        })?,
                        at.idx_ty.clone(),
                    ),
                    _ => self.type_range(&t, line)?,
                };
                if a == "length" {
                    return Ok(self.int_val(r.len() as i64));
                }
                if a == "ascending" {
                    let n = self.dag.bit(!r.down);
                    return Ok(Val::B { ty: Ty::Bool, n });
                }
                let v = match a {
                    "high" => r.hi(),
                    "low" => r.lo(),
                    "left" => r.left,
                    _ => r.right,
                };
                match &it {
                    Ty::Enum(_) | Ty::Bool | Ty::Bit => {
                        let w = it.width()?;
                        let n = self.dag.kval(v as u128, w);
                        Ok(Val::B { ty: it, n })
                    }
                    _ => Ok(self.int_val(v)),
                }
            }
            "range" | "reverse_range" => err(
                line,
                format!("'{a} is only allowed where a range is expected"),
            ),
            "event" | "stable" | "last_value" | "last_event" | "active" | "transaction"
            | "delayed" | "quiet" => err(
                line,
                format!("'{a} is only supported in a clock edge (`clk'event and clk = '1'`)"),
            ),
            "pos" => {
                let t = self.resolve_mark(p, line)?;
                let x = arg.ok_or_else(|| format!("line {line}: 'pos needs an argument"))?;
                let (vt, n) = self.eval_bits(x, Some(&t), line)?;
                match vt {
                    Ty::Enum(e) => {
                        let hi = e.lits.len() as i64 - 1;
                        let w = int_w(0, hi);
                        let n = self.dag.zext(n, w);
                        Ok(Val::B {
                            ty: Ty::Int { lo: 0, hi },
                            n,
                        })
                    }
                    Ty::Logic | Ty::Bit | Ty::Bool => {
                        // std_ulogic'pos('0') = 2, '1' = 3
                        let off = if matches!(vt, Ty::Logic) { 2 } else { 0 };
                        let w = int_w(off, off + 1);
                        let base = self.dag.kval(off as u128, w);
                        let b = self.dag.zext(n, w);
                        let s = self.dag.add(base, b);
                        Ok(Val::B {
                            ty: Ty::Int {
                                lo: off,
                                hi: off + 1,
                            },
                            n: s,
                        })
                    }
                    Ty::Int { .. } => Ok(Val::B { ty: vt, n }),
                    t => err(line, format!("'pos of {}", t.name())),
                }
            }
            "val" => {
                let t = self.resolve_mark(p, line)?;
                let x = arg.ok_or_else(|| format!("line {line}: 'val needs an argument"))?;
                let v = self.eval(x, None, line)?;
                match &t {
                    Ty::Enum(_) => {
                        let (_, n) = match v {
                            Val::B { ty, n } => (ty, n),
                            _ => return err(line, "bad 'val argument"),
                        };
                        let w = t.width()?;
                        let n = self.dag.zext(n, w);
                        Ok(Val::B { ty: t.clone(), n })
                    }
                    Ty::Int { .. } => self.convert(v, &t, line),
                    _ => err(line, format!("'val of {}", t.name())),
                }
            }
            "succ" | "pred" | "leftof" | "rightof" => {
                let t = self.resolve_mark(p, line)?;
                let x = arg.ok_or_else(|| format!("line {line}: '{a} needs an argument"))?;
                let (vt, n) = self.eval_bits(x, Some(&t), line)?;
                let up = matches!(a, "succ" | "rightof");
                match vt {
                    Ty::Int { lo, hi } => {
                        let one = self.int_val(1);
                        let v = Val::B {
                            ty: Ty::Int { lo, hi },
                            n,
                        };
                        let r =
                            self.int_arith(if up { BinOp::Add } else { BinOp::Sub }, v, one, line)?;
                        self.convert(r, &t, line)
                    }
                    Ty::Enum(_) => {
                        let w = vt.width()?;
                        let one = self.dag.kval(1, w);
                        let r = if up {
                            self.dag.add(n, one)
                        } else {
                            self.dag.sub(n, one)
                        };
                        Ok(Val::B { ty: vt, n: r })
                    }
                    t => err(line, format!("'{a} of {}", t.name())),
                }
            }
            "image" | "instance_name" | "path_name" | "simple_name" => Ok(Val::S(String::new())),
            _ => err(line, format!("attribute '{a} is not supported")),
        }
    }

    // ------------------------------------------------------------------
    // Conversions

    fn type_conv(&mut self, t: &Ty, arg: &Expr, line: Line) -> R<Val> {
        let want = if is_untyped_lit(arg) { Some(t) } else { None };
        let v = self.eval(arg, want, line)?;
        match t {
            Ty::Int { lo, hi } => match v {
                Val::R(r) => Ok(self.int_val(r.round() as i64)),
                Val::B {
                    ty: Ty::Int { lo: vl, hi: vh },
                    n,
                } => {
                    // Keep the value's own (narrower) range when it fits.
                    if vl >= *lo && vh <= *hi {
                        Ok(Val::B {
                            ty: Ty::Int { lo: vl, hi: vh },
                            n,
                        })
                    } else {
                        self.convert(
                            Val::B {
                                ty: Ty::Int { lo: vl, hi: vh },
                                n,
                            },
                            t,
                            line,
                        )
                    }
                }
                other => err(
                    line,
                    format!("cannot convert {} to {}", other.ty().name(), t.name()),
                ),
            },
            Ty::Real => match v {
                Val::R(r) => Ok(Val::R(r)),
                ref iv @ Val::B {
                    ty: Ty::Int { .. }, ..
                } => {
                    let i = self
                        .val_to_int(iv)
                        .ok_or_else(|| format!("line {line}: real() of a non-static integer"))?;
                    Ok(Val::R(i as f64))
                }
                other => err(
                    line,
                    format!("cannot convert {} to real", other.ty().name()),
                ),
            },
            Ty::Arr(ta) => match v {
                Val::B { ty: Ty::Arr(va), n } => {
                    let vt = Ty::Arr(va.clone());
                    let idx = match ta.idx {
                        Some(r) => {
                            if r.len() * ta.elem.width()? != self.dag.w(n) {
                                return err(
                                    line,
                                    format!(
                                        "converting {} to {}: length differs",
                                        vt.name(),
                                        t.name()
                                    ),
                                );
                            }
                            r
                        }
                        None => va.idx.unwrap_or(IdxRange::canonical(0)),
                    };
                    if !(ta.elem.is_bit() && va.elem.is_bit())
                        && ta.elem.width()? != va.elem.width()?
                    {
                        return err(
                            line,
                            format!("cannot convert {} to {}", vt.name(), t.name()),
                        );
                    }
                    Ok(Val::B {
                        ty: t.with_idx(idx),
                        n,
                    })
                }
                other => err(
                    line,
                    format!("cannot convert {} to {}", other.ty().name(), t.name()),
                ),
            },
            Ty::Logic | Ty::Bit => match v {
                Val::B {
                    ty: Ty::Logic | Ty::Bit,
                    n,
                } => Ok(Val::B { ty: t.clone(), n }),
                other => err(
                    line,
                    format!("cannot convert {} to {}", other.ty().name(), t.name()),
                ),
            },
            _ => self.convert(v, t, line),
        }
    }

    /// Implicit conversion for assignment / association to type `t`.
    pub fn convert(&mut self, v: Val, t: &Ty, line: Line) -> R<Val> {
        match (t, v) {
            (Ty::Real, Val::R(r)) => Ok(Val::R(r)),
            (
                Ty::Real,
                iv @ Val::B {
                    ty: Ty::Int { .. }, ..
                },
            ) => {
                let i = self
                    .val_to_int(&iv)
                    .ok_or_else(|| format!("line {line}: integer used as real is not static"))?;
                Ok(Val::R(i as f64))
            }
            (Ty::Str, Val::S(s)) => Ok(Val::S(s)),
            (Ty::Time, Val::T) => Ok(Val::T),
            (Ty::Char, Val::S(s)) => Ok(Val::S(s)),
            (
                Ty::Int { lo, hi },
                Val::B {
                    ty: Ty::Int { lo: vl, hi: vh },
                    n,
                },
            ) => {
                if vl == *lo && vh == *hi {
                    return Ok(Val::B { ty: t.clone(), n });
                }
                if let Some(c) = self.dag.as_const(n) {
                    let x = decode_int(c, vl < 0);
                    if x < *lo || x > *hi {
                        return err(line, format!("value {x} is outside {}", t.name()));
                    }
                }
                let tw = int_w(*lo, *hi);
                let vw = self.dag.w(n);
                let e = self.dag.ext(n, tw.max(vw), vl < 0);
                let s = self.dag.slice(e, 0, tw);
                Ok(Val::B {
                    ty: t.clone(),
                    n: s,
                })
            }
            (Ty::Int { lo, hi }, Val::R(r)) => {
                let x = r.round() as i64;
                if x < *lo || x > *hi {
                    return err(line, format!("value {x} is outside {}", t.name()));
                }
                let w = int_w(*lo, *hi);
                let n = self.dag.kval(x as i128 as u128, w);
                Ok(Val::B { ty: t.clone(), n })
            }
            (
                Ty::Arr(ta),
                Val::B {
                    ty: vt @ Ty::Arr(_),
                    n,
                },
            ) => {
                let w = self.dag.w(n);
                match ta.idx {
                    Some(r) => {
                        let tw = r.len() * ta.elem.width()?;
                        if tw != w {
                            return err(
                                line,
                                format!(
                                    "width mismatch: {} bits assigned to {} ({} bits)",
                                    w,
                                    t.name(),
                                    tw
                                ),
                            );
                        }
                        Ok(Val::B { ty: t.clone(), n })
                    }
                    None => {
                        let idx = vt
                            .arr()
                            .and_then(|a| a.idx)
                            .unwrap_or(IdxRange::canonical(w));
                        Ok(Val::B {
                            ty: t.with_idx(idx),
                            n,
                        })
                    }
                }
            }
            (
                Ty::Logic | Ty::Bit,
                Val::B {
                    ty: Ty::Logic | Ty::Bit,
                    n,
                },
            ) => Ok(Val::B { ty: t.clone(), n }),
            (Ty::Bool, Val::B { ty: Ty::Bool, n }) => Ok(Val::B { ty: t.clone(), n }),
            (Ty::Enum(e), Val::B { ty: Ty::Enum(f), n }) if e.name == f.name => {
                Ok(Val::B { ty: t.clone(), n })
            }
            (Ty::Rec(r), Val::B { ty: Ty::Rec(q), n }) if r.name == q.name => {
                Ok(Val::B { ty: t.clone(), n })
            }
            (t, v) => err(
                line,
                format!(
                    "type mismatch: expected {}, got {}",
                    t.name(),
                    v.ty().name()
                ),
            ),
        }
    }

    // ------------------------------------------------------------------
    // Operators

    fn eval2(&mut self, ea: &Expr, eb: &Expr, line: Line) -> R<(Val, Val)> {
        if is_untyped_lit(ea) && !is_untyped_lit(eb) {
            let b = self.eval(eb, None, line)?;
            let a = self.eval(ea, Some(&b.ty()), line)?;
            Ok((a, b))
        } else {
            let a = self.eval(ea, None, line)?;
            let b = self.eval(eb, Some(&a.ty()), line)?;
            Ok((a, b))
        }
    }

    fn binop(&mut self, op: BinOp, ea: &Expr, eb: &Expr, want: Option<&Ty>, line: Line) -> R<Val> {
        use BinOp::*;
        if op == Cat {
            return self.concat_op(ea, eb, want, line);
        }
        let (a, b) = self.eval2(ea, eb, line)?;
        match op {
            And | Or | Xor | Nand | Nor | Xnor => self.logical(op, a, b, line),
            Eq | Ne => {
                let other_logic = |v: &Val| {
                    matches!(v.ty(), Ty::Logic)
                        || matches!(v.ty(), Ty::Arr(x) if matches!(x.elem, Ty::Logic))
                };
                if (meta_lit(ea).is_some() && other_logic(&b))
                    || (meta_lit(eb).is_some() && other_logic(&a))
                {
                    // A synthesized signal is never 'X', 'Z', 'U' or '-'.
                    let n = self.dag.bit(op == Ne);
                    return Ok(Val::B { ty: Ty::Bool, n });
                }
                self.compare(op, a, b, line)
            }
            Lt | Le | Gt | Ge => self.compare(op, a, b, line),
            MEq | MNe | MLt | MLe | MGt | MGe => {
                let base = match op {
                    MEq => Eq,
                    MNe => Ne,
                    MLt => Lt,
                    MLe => Le,
                    MGt => Gt,
                    _ => Ge,
                };
                let r = if matches!(op, MEq | MNe) {
                    if let Some(m) = meta_lit(eb).or_else(|| meta_lit(ea)) {
                        let other = if meta_lit(eb).is_some() {
                            a.clone()
                        } else {
                            b.clone()
                        };
                        let e = self.masked_eq(&other, &m, line)?;
                        let n = if op == MNe { self.dag.not(e) } else { e };
                        Val::B { ty: Ty::Bool, n }
                    } else {
                        self.compare(base, a, b, line)?
                    }
                } else {
                    self.compare(base, a, b, line)?
                };
                let Val::B { n, .. } = r else { unreachable!() };
                Ok(Val::B { ty: Ty::Logic, n })
            }
            Sll | Srl | Sla | Sra | Rol | Ror => self.shift_op(op, a, b, line),
            Add | Sub | Mul | Div | Mod | Rem => self.arith(op, a, b, line),
            Pow => self.pow(a, b, line),
            Cat => unreachable!(),
        }
    }

    /// Equality against a literal with '-' don't-cares (std_match / `?=`).
    fn masked_eq(&mut self, v: &Val, lit: &str, line: Line) -> R<N> {
        let Val::B { n, .. } = v else {
            return err(line, "bad operand");
        };
        let w = self.dag.w(*n) as usize;
        let chars: Vec<char> = lit.chars().collect();
        if chars.len() != w {
            return Ok(self.dag.bit(false));
        }
        let mut acc = self.dag.bit(true);
        for (k, c) in chars.iter().rev().enumerate() {
            let c = c.to_ascii_lowercase();
            if c == '-' {
                continue;
            }
            let Some(b) = bit_of(c) else { continue };
            if matches!(c, 'x' | 'z' | 'u' | 'w') {
                return Ok(self.dag.bit(false));
            }
            let bit = self.dag.slice(*n, k as u32, 1);
            let e = if b { bit } else { self.dag.not(bit) };
            acc = self.dag.and(acc, e);
        }
        Ok(acc)
    }

    fn logical(&mut self, op: BinOp, a: Val, b: Val, line: Line) -> R<Val> {
        let (Val::B { ty: ta, n: na }, Val::B { ty: tb, n: nb }) = (a, b) else {
            return err(line, "logical operator on non-hardware values");
        };
        let (ty, mut x, mut y) = match (&ta, &tb) {
            (Ty::Bool, Ty::Bool) | (Ty::Logic | Ty::Bit, Ty::Logic | Ty::Bit) => {
                (ta.clone(), na, nb)
            }
            (Ty::Arr(_), Ty::Arr(_)) => {
                if self.dag.w(na) != self.dag.w(nb) {
                    return err(
                        line,
                        format!(
                            "logical operator on vectors of different lengths ({} and {})",
                            ta.name(),
                            tb.name()
                        ),
                    );
                }
                (ta.clone(), na, nb)
            }
            (Ty::Arr(_), Ty::Logic | Ty::Bit) => {
                let w = self.dag.w(na);
                let r = self.dag.concat(vec![nb; w as usize]);
                (ta.clone(), na, r)
            }
            (Ty::Logic | Ty::Bit, Ty::Arr(_)) => {
                let w = self.dag.w(nb);
                let r = self.dag.concat(vec![na; w as usize]);
                (tb.clone(), r, nb)
            }
            _ => {
                return err(
                    line,
                    format!("logical operator on {} and {}", ta.name(), tb.name()),
                );
            }
        };
        if matches!(ty, Ty::Arr(ref a) if !a.elem.is_bit() && !matches!(a.elem, Ty::Bool)) {
            return err(line, format!("logical operator on {}", ty.name()));
        }
        let _ = (&mut x, &mut y);
        let r = match op {
            BinOp::And => self.dag.and(x, y),
            BinOp::Or => self.dag.or(x, y),
            BinOp::Xor => self.dag.xor(x, y),
            BinOp::Nand => {
                let t = self.dag.and(x, y);
                self.dag.not(t)
            }
            BinOp::Nor => {
                let t = self.dag.or(x, y);
                self.dag.not(t)
            }
            _ => {
                let t = self.dag.xor(x, y);
                self.dag.not(t)
            }
        };
        Ok(Val::B { ty, n: r })
    }

    /// Numeric view of a value: (node, signed), if it is a number.
    fn num_of(&self, v: &Val) -> Option<(N, bool)> {
        match v {
            Val::B {
                ty: Ty::Int { lo, .. },
                n,
            } => Some((*n, *lo < 0)),
            Val::B { ty: Ty::Arr(a), n } if a.elem.is_bit() => match a.kind {
                ArrKind::Unsigned => Some((*n, false)),
                ArrKind::Signed => Some((*n, true)),
                ArrKind::Slv | ArrKind::BitVec if self.uses_pkg("std_logic_signed") => {
                    Some((*n, true))
                }
                ArrKind::Slv | ArrKind::BitVec
                    if self.uses_pkg("std_logic_unsigned")
                        || self.uses_pkg("numeric_std_unsigned")
                        || self.uses_pkg("std_logic_arith") =>
                {
                    Some((*n, false))
                }
                _ => None,
            },
            _ => None,
        }
    }

    pub fn compare(&mut self, op: BinOp, a: Val, b: Val, line: Line) -> R<Val> {
        use BinOp::*;
        let bool_val = |n| Val::B { ty: Ty::Bool, n };
        if let (Val::R(x), Val::R(y)) = (&a, &b) {
            let r = match op {
                Eq => x == y,
                Ne => x != y,
                Lt => x < y,
                Le => x <= y,
                Gt => x > y,
                _ => x >= y,
            };
            let n = self.dag.bit(r);
            return Ok(bool_val(n));
        }
        if let (Val::S(x), Val::S(y)) = (&a, &b) {
            let r = match op {
                Eq => x == y,
                Ne => x != y,
                _ => return err(line, "ordering of strings"),
            };
            let n = self.dag.bit(r);
            return Ok(bool_val(n));
        }
        let both_numeric = self.num_of(&a).is_some() && self.num_of(&b).is_some();
        let one_vec_numeric = (self.num_of(&a).is_some()
            && matches!(a.ty(), Ty::Arr(_))
            && matches!(b.ty(), Ty::Int { .. }))
            || (self.num_of(&b).is_some()
                && matches!(b.ty(), Ty::Arr(_))
                && matches!(a.ty(), Ty::Int { .. }));
        if both_numeric || one_vec_numeric {
            let (na, sa) = self.num_of(&a).unwrap();
            let (nb, sb) = self.num_of(&b).unwrap();
            let wa = self.dag.w(na) + u32::from(sb && !sa);
            let wb = self.dag.w(nb) + u32::from(sa && !sb);
            let cw = wa.max(wb).max(1);
            let x = self.dag.ext(na, cw, sa);
            let y = self.dag.ext(nb, cw, sb);
            let signed = sa || sb;
            let lt = |d: &mut Dag, p: N, q: N| if signed { d.slt(p, q) } else { d.ult(p, q) };
            let r = match op {
                Eq => self.dag.eq(x, y),
                Ne => self.dag.ne(x, y),
                Lt => lt(&mut self.dag, x, y),
                Gt => lt(&mut self.dag, y, x),
                Le => {
                    let t = lt(&mut self.dag, y, x);
                    self.dag.not(t)
                }
                _ => {
                    let t = lt(&mut self.dag, x, y);
                    self.dag.not(t)
                }
            };
            return Ok(bool_val(r));
        }
        let (Val::B { ty: ta, n: na }, Val::B { ty: tb, n: nb }) = (a, b) else {
            return err(line, "comparison of incompatible values");
        };
        let wa = self.dag.w(na);
        let wb = self.dag.w(nb);
        if !ta.same_base(&tb) && !(ta.is_vec() && tb.is_vec()) && !(ta.is_bit() && tb.is_bit()) {
            return err(
                line,
                format!("comparison of {} with {}", ta.name(), tb.name()),
            );
        }
        if wa != wb {
            if matches!(op, Eq | Ne) && matches!(ta, Ty::Arr(_)) {
                // Predefined array equality: different lengths are unequal.
                let n = self.dag.bit(op == Ne);
                return Ok(bool_val(n));
            }
            return err(
                line,
                format!(
                    "ordering of {} and {} with different lengths",
                    ta.name(),
                    tb.name()
                ),
            );
        }
        let r = match op {
            Eq => self.dag.eq(na, nb),
            Ne => self.dag.ne(na, nb),
            Lt => self.dag.ult(na, nb),
            Gt => self.dag.ult(nb, na),
            Le => self.dag.ule(na, nb),
            _ => self.dag.ule(nb, na),
        };
        Ok(bool_val(r))
    }

    fn shift_op(&mut self, op: BinOp, a: Val, b: Val, line: Line) -> R<Val> {
        let Val::B { ty, n } = a else {
            return err(line, "shift of a non-vector");
        };
        if !matches!(ty, Ty::Arr(_)) {
            return err(line, format!("shift of {}", ty.name()));
        }
        let arith = matches!(op, BinOp::Sra)
            || (matches!(op, BinOp::Sla | BinOp::Sra)
                && matches!(ty.arr().map(|a| &a.kind), Some(ArrKind::Signed)));
        let r = match op {
            BinOp::Sll | BinOp::Sla => self.shift(n, &b, true, false, line)?,
            BinOp::Srl => self.shift(n, &b, false, false, line)?,
            BinOp::Sra => self.shift(n, &b, false, arith, line)?,
            BinOp::Rol => self.rotate(n, &b, true, line)?,
            _ => self.rotate(n, &b, false, line)?,
        };
        Ok(Val::B { ty, n: r })
    }

    /// Shift toward the MSB (`left`) or LSB, filling with 0 or the sign bit.
    pub fn shift(&mut self, n: N, amt: &Val, left: bool, arith: bool, line: Line) -> R<N> {
        let w = self.dag.w(n);
        if let Some(k) = self.val_to_int(amt) {
            let (left, k) = if k < 0 { (!left, -k) } else { (left, k) };
            let kn = self.dag.kval(k.min(w as i64 + 1) as u128, 32);
            return Ok(if left {
                self.dag.shl(n, kn)
            } else if arith && w > 0 {
                // fill with the sign bit
                let k = (k as u32).min(w);
                let msb = self.dag.slice(n, w - 1, 1);
                let hi = self.dag.slice(n, k.min(w - 1).max(0), w - k.min(w));
                let mut parts = vec![msb; k as usize];
                if k < w {
                    parts.push(hi);
                }
                let c = self.dag.concat(parts);
                self.dag.slice(c, 0, w)
            } else {
                self.dag.lshr(n, kn)
            });
        }
        let an = match amt {
            Val::B {
                ty: Ty::Int { lo, .. },
                ..
            } if *lo < 0 => {
                return err(
                    line,
                    "dynamic shift by a possibly negative amount is not supported",
                );
            }
            Val::B {
                ty: Ty::Int { .. } | Ty::Arr(_),
                n,
            } => *n,
            _ => return err(line, "shift amount must be an integer"),
        };
        Ok(if left {
            self.dag.shl(n, an)
        } else if arith {
            let msb = self.dag.slice(n, w - 1, 1);
            let nn = self.dag.not(n);
            let s1 = self.dag.lshr(nn, an);
            let s1 = self.dag.not(s1);
            let s0 = self.dag.lshr(n, an);
            self.dag.mux(msb, s1, s0)
        } else {
            self.dag.lshr(n, an)
        })
    }

    pub fn rotate(&mut self, n: N, amt: &Val, left: bool, line: Line) -> R<N> {
        let w = self.dag.w(n);
        if w == 0 {
            return Ok(n);
        }
        if let Some(k) = self.val_to_int(amt) {
            let k = k.rem_euclid(w as i64) as u32;
            let k = if left { k } else { (w - k) % w };
            if k == 0 {
                return Ok(n);
            }
            // rotate left by k: {n[w-1-k:0], n[w-1:w-k]}
            let lo = self.dag.slice(n, 0, w - k);
            let hi = self.dag.slice(n, w - k, k);
            return Ok(self.dag.concat(vec![lo, hi]));
        }
        let Val::B {
            ty: Ty::Int { lo, .. },
            n: an,
        } = amt
        else {
            return err(line, "rotate amount must be an integer");
        };
        if *lo < 0 {
            return err(
                line,
                "dynamic rotate by a possibly negative amount is not supported",
            );
        }
        // Rotate via a double-width shift; amount reduced modulo w.
        let aw = self.dag.w(*an);
        let cw = aw.max(bits_for_u(w as u128));
        let a = self.dag.zext(*an, cw);
        let wk = self.dag.kval(w as u128, cw);
        let a = self.dag.urem(a, wk);
        let dbl = self.dag.concat(vec![n, n]);
        Ok(if left {
            let s = self.dag.shl(dbl, a);
            self.dag.slice(s, w, w)
        } else {
            let s = self.dag.lshr(dbl, a);
            self.dag.slice(s, 0, w)
        })
    }

    fn concat_op(&mut self, ea: &Expr, eb: &Expr, want: Option<&Ty>, line: Line) -> R<Val> {
        // Typed operand first so literals take its type.
        let (a, b) = if is_untyped_lit(ea) && !is_untyped_lit(eb) {
            let b = self.eval(eb, want, line)?;
            let a = self.eval(ea, Some(&b.ty()), line)?;
            (a, b)
        } else {
            let a = self.eval(ea, want, line)?;
            let b = self.eval(eb, Some(&a.ty()), line)?;
            (a, b)
        };
        if let (Val::S(x), Val::S(y)) = (&a, &b) {
            return Ok(Val::S(format!("{x}{y}")));
        }
        if matches!(a, Val::S(_)) || matches!(b, Val::S(_)) {
            return Ok(Val::S(String::new()));
        }
        let (Val::B { ty: ta, n: na }, Val::B { ty: tb, n: nb }) = (a, b) else {
            return err(line, "concatenation of non-hardware values");
        };
        let arr = match (&ta, &tb, want) {
            (Ty::Arr(x), _, _) => x.clone(),
            (_, Ty::Arr(x), _) => x.clone(),
            (_, _, Some(Ty::Arr(x))) => x.clone(),
            (Ty::Bit, _, _) => match slv_ty(0, ArrKind::BitVec, Ty::Bit) {
                Ty::Arr(x) => x,
                _ => unreachable!(),
            },
            _ => match slv_ty(0, ArrKind::Slv, Ty::Logic) {
                Ty::Arr(x) => x,
                _ => unreachable!(),
            },
        };
        let ew = arr.elem.width().unwrap_or(1).max(1);
        let n = self.dag.concat(vec![na, nb]);
        let len = self.dag.w(n) / ew;
        let down = match &ta {
            Ty::Arr(x) => x.idx.map(|r| r.down).unwrap_or(true),
            _ => true,
        };
        let idx = if down {
            IdxRange {
                left: len as i64 - 1,
                right: 0,
                down: true,
            }
        } else {
            IdxRange {
                left: 0,
                right: len as i64 - 1,
                down: false,
            }
        };
        Ok(Val::B {
            ty: Ty::Arr(Rc::new(ArrTy {
                kind: arr.kind.clone(),
                elem: arr.elem.clone(),
                idx: Some(idx),
                idx_ty: arr.idx_ty.clone(),
            })),
            n,
        })
    }

    fn real_arith(op: BinOp, x: f64, y: f64) -> Option<f64> {
        Some(match op {
            BinOp::Add => x + y,
            BinOp::Sub => x - y,
            BinOp::Mul => x * y,
            BinOp::Div => x / y,
            BinOp::Pow => x.powf(y),
            _ => return None,
        })
    }

    fn arith(&mut self, op: BinOp, a: Val, b: Val, line: Line) -> R<Val> {
        match (&a, &b) {
            (Val::R(x), Val::R(y)) => {
                return Self::real_arith(op, *x, *y)
                    .map(Val::R)
                    .ok_or_else(|| format!("line {line}: unsupported real operator"));
            }
            (
                Val::R(x),
                iv @ Val::B {
                    ty: Ty::Int { .. }, ..
                },
            )
            | (
                iv @ Val::B {
                    ty: Ty::Int { .. }, ..
                },
                Val::R(x),
            ) if matches!(op, BinOp::Mul | BinOp::Div) => {
                let i = self.val_to_int(iv).ok_or_else(|| {
                    format!("line {line}: real arithmetic with a non-static integer")
                })? as f64;
                let (p, q) = if matches!(a, Val::R(_)) {
                    (*x, i)
                } else {
                    (i, *x)
                };
                return Ok(Val::R(Self::real_arith(op, p, q).unwrap()));
            }
            (Val::T, _) | (_, Val::T) => return Ok(Val::T),
            _ => {}
        }
        let ta = a.ty();
        let tb = b.ty();
        if ta.is_int() && tb.is_int() {
            return self.int_arith(op, a, b, line);
        }
        // Vector arithmetic.
        let vec_side = if matches!(ta, Ty::Arr(_)) { &a } else { &b };
        let (_, signed) = self.num_of(vec_side).ok_or_else(|| {
            format!("line {line}: arithmetic on {} (use numeric_std unsigned/signed, or std_logic_unsigned)", vec_side.ty().name())
        })?;
        let rty = vec_side.ty();
        let vw = match vec_side {
            Val::B { n, .. } => self.dag.w(*n),
            _ => unreachable!(),
        };
        let as_vec = |el: &mut Elab, v: &Val| -> R<(N, bool, bool)> {
            match v {
                Val::B {
                    ty: Ty::Int { lo, .. },
                    n,
                } => {
                    // to_unsigned/to_signed(v, vw)
                    let e = el.dag.ext(*n, vw.max(el.dag.w(*n)), *lo < 0);
                    let s = el.dag.slice(e, 0, vw);
                    Ok((s, signed, true))
                }
                Val::B {
                    ty: Ty::Logic | Ty::Bit,
                    n,
                } => Ok((*n, false, false)),
                Val::B { ty: Ty::Arr(_), .. } => {
                    let (n, s) = el
                        .num_of(v)
                        .ok_or_else(|| format!("line {line}: arithmetic on {}", v.ty().name()))?;
                    Ok((n, s, false))
                }
                _ => err(line, format!("arithmetic on {}", v.ty().name())),
            }
        };
        let (x, sx, xi) = as_vec(self, &a)?;
        let (y, sy, yi) = as_vec(self, &b)?;
        if matches!(ta, Ty::Arr(_)) && matches!(tb, Ty::Arr(_)) && sx != sy {
            return err(line, "arithmetic mixing signed and unsigned");
        }
        let wx = self.dag.w(x);
        let wy = self.dag.w(y);
        let mk = |w: u32| rty.with_idx(IdxRange::canonical(w));
        match op {
            BinOp::Add | BinOp::Sub => {
                let rw = wx.max(wy);
                let p = self.dag.ext(x, rw, sx);
                let q = self.dag.ext(y, rw, sy);
                let r = if op == BinOp::Add {
                    self.dag.add(p, q)
                } else {
                    self.dag.sub(p, q)
                };
                Ok(Val::B { ty: mk(rw), n: r })
            }
            BinOp::Mul => {
                // numeric_std: L*R has L'length + R'length bits; a natural
                // operand counts as the vector operand's length.
                let wx2 = if xi { wy } else { wx };
                let wy2 = if yi { wx } else { wy };
                let rw = wx2 + wy2;
                let p = self.dag.ext(x, rw, sx);
                let q = self.dag.ext(y, rw, sy);
                let r = self.dag.mul(p, q);
                Ok(Val::B { ty: mk(rw), n: r })
            }
            BinOp::Div | BinOp::Rem | BinOp::Mod => {
                let cw = wx.max(wy);
                let signed = sx || sy;
                let p = self.dag.ext(x, cw, sx);
                let q = self.dag.ext(y, cw, sy);
                let r = if signed {
                    self.sdivmod(op, p, q)
                } else if op == BinOp::Div {
                    self.dag.udiv(p, q)
                } else {
                    self.dag.urem(p, q)
                };
                // Result length: `/` -> L'length; rem/mod -> R'length (L'length if R is a natural).
                let rw = if op == BinOp::Div || yi { wx } else { wy };
                let r = self.dag.slice(r, 0, rw.min(cw));
                let r = self.dag.ext(r, rw, signed);
                Ok(Val::B { ty: mk(rw), n: r })
            }
            _ => err(line, "unsupported vector operator"),
        }
    }

    /// Signed divide / rem / mod on equal-width two's complement nodes.
    fn sdivmod(&mut self, op: BinOp, p: N, q: N) -> N {
        let w = self.dag.w(p);
        let z = self.dag.zero(w);
        let sp = self.dag.slice(p, w - 1, 1);
        let sq = self.dag.slice(q, w - 1, 1);
        let np = self.dag.sub(z, p);
        let nq = self.dag.sub(z, q);
        let ap = self.dag.mux(sp, np, p);
        let aq = self.dag.mux(sq, nq, q);
        match op {
            BinOp::Div => {
                let uq = self.dag.udiv(ap, aq);
                let s = self.dag.xor(sp, sq);
                let nuq = self.dag.sub(z, uq);
                self.dag.mux(s, nuq, uq)
            }
            _ => {
                let ur = self.dag.urem(ap, aq);
                let nur = self.dag.sub(z, ur);
                let rem = self.dag.mux(sp, nur, ur);
                if op == BinOp::Rem {
                    return rem;
                }
                // mod: sign of divisor; adjust when signs differ and rem != 0
                let nz = self.dag.red_or(rem);
                let diff = self.dag.xor(sp, sq);
                let adj = self.dag.and(nz, diff);
                let plus = self.dag.add(rem, q);
                self.dag.mux(adj, plus, rem)
            }
        }
    }

    pub fn int_arith(&mut self, op: BinOp, a: Val, b: Val, line: Line) -> R<Val> {
        let (
            Val::B {
                ty: Ty::Int { lo: al, hi: ah },
                n: an,
            },
            Val::B {
                ty: Ty::Int { lo: bl, hi: bh },
                n: bn,
            },
        ) = (&a, &b)
        else {
            return err(line, "integer operator on non-integers");
        };
        let (al, ah, bl, bh, an, bn) = (*al, *ah, *bl, *bh, *an, *bn);
        if let (Some(x), Some(y)) = (self.val_to_int(&a), self.val_to_int(&b)) {
            let r = match op {
                BinOp::Add => x.checked_add(y),
                BinOp::Sub => x.checked_sub(y),
                BinOp::Mul => x.checked_mul(y),
                BinOp::Div => {
                    if y == 0 {
                        return err(line, "division by zero");
                    }
                    Some(x / y)
                }
                BinOp::Rem => {
                    if y == 0 {
                        return err(line, "division by zero");
                    }
                    Some(x % y)
                }
                BinOp::Mod => {
                    if y == 0 {
                        return err(line, "division by zero");
                    }
                    let r = x % y;
                    Some(if r != 0 && ((r < 0) != (y < 0)) {
                        r + y
                    } else {
                        r
                    })
                }
                _ => None,
            };
            return match r {
                Some(v) => Ok(self.int_val(v)),
                None => err(line, "integer overflow in constant expression"),
            };
        }
        let lim = 1i128 << 62;
        let (rlo, rhi): (i128, i128) = match op {
            BinOp::Add => (al as i128 + bl as i128, ah as i128 + bh as i128),
            BinOp::Sub => (al as i128 - bh as i128, ah as i128 - bl as i128),
            BinOp::Mul => {
                let c = [
                    al as i128 * bl as i128,
                    al as i128 * bh as i128,
                    ah as i128 * bl as i128,
                    ah as i128 * bh as i128,
                ];
                (*c.iter().min().unwrap(), *c.iter().max().unwrap())
            }
            BinOp::Div | BinOp::Rem | BinOp::Mod => {
                if bl <= 0 && bh >= 0 && !(bl == 0 && bh == 0) && bl < 0 {
                    // divisor range spans zero on both sides
                }
                let m = (al.unsigned_abs().max(ah.unsigned_abs())) as i128;
                let d = bl.unsigned_abs().max(bh.unsigned_abs()) as i128;
                match op {
                    BinOp::Div => (-m, m),
                    _ => (-(d - 1).max(0), (d - 1).max(0)),
                }
            }
            _ => return err(line, "unsupported integer operator"),
        };
        let (rlo, rhi) =
            if op != BinOp::Add && op != BinOp::Sub && op != BinOp::Mul && al >= 0 && bl >= 0 {
                match op {
                    BinOp::Div => (0, rhi),
                    _ => (0, rhi.min(ah as i128)),
                }
            } else {
                (rlo, rhi)
            };
        let (rlo, rhi) = if rlo < -lim || rhi > lim {
            (INT_MIN as i128, INT_MAX as i128)
        } else {
            (rlo, rhi)
        };
        let (rlo, rhi) = (rlo as i64, rhi as i64);
        let rw = int_w(rlo, rhi);
        let rty = Ty::Int { lo: rlo, hi: rhi };
        let r = match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                let x = self.dag.ext(an, rw, al < 0);
                let y = self.dag.ext(bn, rw, bl < 0);
                match op {
                    BinOp::Add => self.dag.add(x, y),
                    BinOp::Sub => self.dag.sub(x, y),
                    _ => self.dag.mul(x, y),
                }
            }
            _ => {
                let cw = self.dag.w(an).max(self.dag.w(bn)) + 1;
                let x = self.dag.ext(an, cw, al < 0);
                let y = self.dag.ext(bn, cw, bl < 0);
                let r = if al >= 0 && bl >= 0 {
                    if op == BinOp::Div {
                        self.dag.udiv(x, y)
                    } else {
                        self.dag.urem(x, y)
                    }
                } else {
                    self.sdivmod(op, x, y)
                };
                let e = self.dag.ext(r, rw.max(cw), true);
                self.dag.slice(e, 0, rw)
            }
        };
        Ok(Val::B { ty: rty, n: r })
    }

    fn pow(&mut self, a: Val, b: Val, line: Line) -> R<Val> {
        if let (Val::R(x), _) = (&a, &b) {
            let y = match &b {
                Val::R(y) => *y,
                v => self
                    .val_to_int(v)
                    .ok_or_else(|| format!("line {line}: non-static exponent"))?
                    as f64,
            };
            return Ok(Val::R(x.powf(y)));
        }
        let base = self.val_to_int(&a);
        let ex = self.val_to_int(&b);
        match (base, ex) {
            (Some(x), Some(y)) => {
                if y < 0 {
                    return err(line, "negative integer exponent");
                }
                let r = (x as i128)
                    .checked_pow(y as u32)
                    .filter(|v| v.abs() < (1i128 << 62))
                    .ok_or_else(|| format!("line {line}: {x}**{y} overflows"))?;
                Ok(self.int_val(r as i64))
            }
            (Some(2), None) => {
                let Val::B {
                    ty: Ty::Int { lo, hi },
                    n,
                } = b
                else {
                    return err(line, "bad exponent");
                };
                if lo < 0 || hi > 61 {
                    return err(line, "2**n needs 0 <= n <= 61");
                }
                let rlo = 1i64 << lo;
                let rhi = 1i64 << hi;
                let w = int_w(rlo, rhi);
                let one = self.dag.kval(1, w);
                let r = self.dag.shl(one, n);
                Ok(Val::B {
                    ty: Ty::Int { lo: rlo, hi: rhi },
                    n: r,
                })
            }
            _ => err(line, "`**` needs static operands (or 2**n)"),
        }
    }

    fn unop(&mut self, op: UnOp, ea: &Expr, line: Line) -> R<Val> {
        let a = self.eval(ea, None, line)?;
        match op {
            UnOp::Pos => Ok(a),
            UnOp::Neg => match a {
                Val::R(r) => Ok(Val::R(-r)),
                Val::B {
                    ty: Ty::Int { lo, hi },
                    n,
                } => {
                    if let Some(x) = self.val_to_int(&Val::B {
                        ty: Ty::Int { lo, hi },
                        n,
                    }) {
                        return Ok(self.int_val(-x));
                    }
                    let (rlo, rhi) = (-hi, -lo);
                    let rw = int_w(rlo, rhi);
                    let x = self.dag.ext(n, rw, lo < 0);
                    let z = self.dag.zero(rw);
                    let r = self.dag.sub(z, x);
                    Ok(Val::B {
                        ty: Ty::Int { lo: rlo, hi: rhi },
                        n: r,
                    })
                }
                Val::B { ty, n }
                    if matches!(&ty, Ty::Arr(a) if a.kind == ArrKind::Signed)
                        || self
                            .num_of(&Val::B { ty: ty.clone(), n })
                            .map(|x| x.1)
                            .unwrap_or(false) =>
                {
                    let w = self.dag.w(n);
                    let z = self.dag.zero(w);
                    let r = self.dag.sub(z, n);
                    Ok(Val::B { ty, n: r })
                }
                Val::T => Ok(Val::T),
                v => err(line, format!("unary minus on {}", v.ty().name())),
            },
            UnOp::Abs => match a {
                Val::R(r) => Ok(Val::R(r.abs())),
                Val::B {
                    ty: Ty::Int { lo, hi },
                    n,
                } => {
                    if lo >= 0 {
                        return Ok(Val::B {
                            ty: Ty::Int { lo, hi },
                            n,
                        });
                    }
                    let rhi = (-lo).max(hi);
                    let rlo = if hi < 0 { -hi } else { 0 };
                    let rw = int_w(rlo, rhi);
                    let sw = int_w(lo, hi).max(rw + 1);
                    let x = self.dag.sext(n, sw);
                    let z = self.dag.zero(sw);
                    let neg = self.dag.sub(z, x);
                    let msb = self.dag.slice(x, sw - 1, 1);
                    let m = self.dag.mux(msb, neg, x);
                    let r = self.dag.slice(m, 0, rw);
                    Ok(Val::B {
                        ty: Ty::Int { lo: rlo, hi: rhi },
                        n: r,
                    })
                }
                Val::B { ty, n } if matches!(&ty, Ty::Arr(a) if a.kind == ArrKind::Signed) => {
                    let w = self.dag.w(n);
                    let z = self.dag.zero(w);
                    let neg = self.dag.sub(z, n);
                    let msb = self.dag.slice(n, w - 1, 1);
                    let r = self.dag.mux(msb, neg, n);
                    Ok(Val::B { ty, n: r })
                }
                v => err(line, format!("abs of {}", v.ty().name())),
            },
            UnOp::Not => match a {
                Val::B { ty, n }
                    if matches!(ty, Ty::Bool | Ty::Logic | Ty::Bit)
                        || matches!(&ty, Ty::Arr(x) if x.elem.is_bit() || matches!(x.elem, Ty::Bool)) =>
                {
                    let r = self.dag.not(n);
                    Ok(Val::B { ty, n: r })
                }
                v => err(line, format!("`not` on {}", v.ty().name())),
            },
            UnOp::RAnd | UnOp::ROr | UnOp::RXor | UnOp::RNand | UnOp::RNor | UnOp::RXnor => {
                let Val::B { ty, n } = a else {
                    return err(line, "reduction of a non-vector");
                };
                let r = self.reduce(op, n);
                let rt = if matches!(ty, Ty::Arr(ref x) if matches!(x.elem, Ty::Bit)) {
                    Ty::Bit
                } else {
                    Ty::Logic
                };
                Ok(Val::B { ty: rt, n: r })
            }
            UnOp::Cond => match a {
                Val::B {
                    ty: Ty::Logic | Ty::Bit | Ty::Bool,
                    n,
                } => Ok(Val::B { ty: Ty::Bool, n }),
                v => err(line, format!("`??` on {}", v.ty().name())),
            },
        }
    }

    fn reduce(&mut self, op: UnOp, n: N) -> N {
        match op {
            UnOp::RAnd => self.dag.red_and(n),
            UnOp::ROr => self.dag.red_or(n),
            UnOp::RXor => self.dag.red_xor(n),
            UnOp::RNand => {
                let r = self.dag.red_and(n);
                self.dag.not(r)
            }
            UnOp::RNor => {
                let r = self.dag.red_or(n);
                self.dag.not(r)
            }
            _ => {
                let r = self.dag.red_xor(n);
                self.dag.not(r)
            }
        }
    }

    fn op_call(&mut self, op: &str, args: &[Assoc], want: Option<&Ty>, line: Line) -> R<Val> {
        let bop = match op {
            "\"and\"" => BinOp::And,
            "\"or\"" => BinOp::Or,
            "\"xor\"" => BinOp::Xor,
            "\"nand\"" => BinOp::Nand,
            "\"nor\"" => BinOp::Nor,
            "\"xnor\"" => BinOp::Xnor,
            "\"+\"" => BinOp::Add,
            "\"-\"" => BinOp::Sub,
            "\"*\"" => BinOp::Mul,
            "\"=\"" => BinOp::Eq,
            "\"/=\"" => BinOp::Ne,
            "\"<\"" => BinOp::Lt,
            "\">\"" => BinOp::Gt,
            "\"&\"" => BinOp::Cat,
            _ => return err(line, format!("operator call {op} is not supported")),
        };
        if args.len() != 2 {
            return err(line, format!("operator call {op} needs two arguments"));
        }
        self.binop(bop, &args[0].actual, &args[1].actual, want, line)
    }

    // ------------------------------------------------------------------
    // Built-in functions (IEEE packages)

    fn arg(&mut self, args: &[Assoc], k: usize, want: Option<&Ty>, line: Line) -> R<Val> {
        let a = args
            .get(k)
            .ok_or_else(|| format!("line {line}: missing argument {}", k + 1))?;
        self.eval(&a.actual, want, line)
    }

    /// Size argument: a static natural or (VHDL-2008) a vector whose length is used.
    fn size_arg(&mut self, args: &[Assoc], k: usize, line: Line) -> R<u32> {
        let v = self.arg(args, k, None, line)?;
        if let Val::B { ty: Ty::Arr(_), n } = &v {
            return Ok(self.dag.w(*n));
        }
        let s = self
            .val_to_int(&v)
            .ok_or_else(|| format!("line {line}: size argument must be static"))?;
        if s < 0 || s > 1 << 20 {
            return err(line, format!("bad size {s}"));
        }
        Ok(s as u32)
    }

    pub fn builtin(
        &mut self,
        f: &str,
        args: &[Assoc],
        want: Option<&Ty>,
        line: Line,
    ) -> R<Option<Val>> {
        let vec = |kind: ArrKind, n: N, el: &Elab| -> Val {
            let w = el.dag.w(n);
            Val::B {
                ty: slv_ty(w, kind, Ty::Logic),
                n,
            }
        };
        let v = match f {
            "rising_edge" | "falling_edge" => {
                return err(
                    line,
                    format!("{f}() is only supported as the clock edge of a clocked process"),
                );
            }
            "now" => return err(line, "`now` is not synthesizable"),
            "endfile" => {
                let i = match args.first().map(|a| &a.actual) {
                    Some(Expr::Name(n)) => match &**n {
                        Name::Id(x, _) => match self.lookup(x) {
                            Some(Sym::File(i)) => *i as usize,
                            _ => return err(line, "endfile expects a file"),
                        },
                        _ => return err(line, "endfile expects a file"),
                    },
                    _ => return err(line, "endfile expects a file"),
                };
                let (_, lines, pos) = &self.tio_files[i];
                let b = *pos >= lines.len();
                let n = self.dag.bit(b);
                Val::B { ty: Ty::Bool, n }
            }
            "to_unsigned"
            | "to_signed"
            | "conv_unsigned"
            | "conv_signed"
            | "conv_std_logic_vector" => {
                let x = self.arg(args, 0, None, line)?;
                let w = self.size_arg(args, 1, line)?;
                let (n, s) = match &x {
                    Val::B {
                        ty: Ty::Int { lo, .. },
                        n,
                    } => (*n, *lo < 0),
                    Val::B { ty: Ty::Arr(_), .. } => self
                        .num_of(&x)
                        .ok_or_else(|| format!("line {line}: {f} of {}", x.ty().name()))?,
                    Val::B {
                        ty: Ty::Logic | Ty::Bit,
                        n,
                    } => (*n, false),
                    Val::R(r) => {
                        let i = r.round() as i64;
                        let iv = self.int_val(i);
                        match iv {
                            Val::B { n, .. } => (n, i < 0),
                            _ => unreachable!(),
                        }
                    }
                    _ => return err(line, format!("{f} of {}", x.ty().name())),
                };
                let nw = self.dag.w(n);
                let e = self.dag.ext(n, nw.max(w), s);
                let r = self.dag.slice(e, 0, w);
                let kind = match f {
                    "to_unsigned" | "conv_unsigned" => ArrKind::Unsigned,
                    "to_signed" | "conv_signed" => ArrKind::Signed,
                    _ => ArrKind::Slv,
                };
                vec(kind, r, self)
            }
            "to_integer" | "conv_integer" | "to_int" => {
                let x = self.arg(args, 0, None, line)?;
                match &x {
                    Val::B {
                        ty: Ty::Int { .. }, ..
                    } => x,
                    Val::B {
                        ty: Ty::Logic | Ty::Bit,
                        n,
                    } => Val::B {
                        ty: Ty::Int { lo: 0, hi: 1 },
                        n: *n,
                    },
                    Val::B { ty: Ty::Arr(_), n } => {
                        let signed = match self.num_of(&x) {
                            Some((_, s)) => s,
                            None if f == "to_integer" && x.ty().is_vec() => false,
                            None => return err(line, format!("{f} of {}", x.ty().name())),
                        };
                        let w = self.dag.w(*n);
                        if w > 62 {
                            return err(line, format!("{f} of a {w}-bit vector"));
                        }
                        if w == 0 {
                            self.int_val(0)
                        } else if signed {
                            Val::B {
                                ty: Ty::Int {
                                    lo: -(1i64 << (w - 1)),
                                    hi: (1i64 << (w - 1)) - 1,
                                },
                                n: *n,
                            }
                        } else {
                            Val::B {
                                ty: Ty::Int {
                                    lo: 0,
                                    hi: (1i64 << w) - 1,
                                },
                                n: *n,
                            }
                        }
                    }
                    _ => return err(line, format!("{f} of {}", x.ty().name())),
                }
            }
            "resize" | "ext" | "sxt" => {
                let x = self.arg(args, 0, None, line)?;
                let w = self.size_arg(args, 1, line)?;
                let Val::B { ty, n } = &x else {
                    return err(line, format!("{f} of {}", x.ty().name()));
                };
                let signed = match f {
                    "ext" => false,
                    "sxt" => true,
                    _ => self.num_of(&x).map(|p| p.1).unwrap_or(false),
                };
                let nw = self.dag.w(*n);
                let r = if signed && f == "resize" && w < nw && w > 0 {
                    // numeric_std resize(signed) keeps the sign bit.
                    let msb = self.dag.slice(*n, nw - 1, 1);
                    let lo = self.dag.slice(*n, 0, w - 1);
                    self.dag.concat(vec![msb, lo])
                } else {
                    let e = self.dag.ext(*n, nw.max(w), signed);
                    self.dag.slice(e, 0, w)
                };
                Val::B {
                    ty: ty.with_idx(IdxRange::canonical(w)),
                    n: r,
                }
            }
            "shift_left" | "shift_right" | "rotate_left" | "rotate_right" | "shl" | "shr" => {
                let x = self.arg(args, 0, None, line)?;
                let amt = self.arg(args, 1, None, line)?;
                let amt = match &amt {
                    Val::B { ty: Ty::Arr(_), n } => {
                        let w = self.dag.w(*n);
                        if w > 62 {
                            return err(line, "shift amount too wide");
                        }
                        Val::B {
                            ty: Ty::Int {
                                lo: 0,
                                hi: (1i64 << w) - 1,
                            },
                            n: *n,
                        }
                    }
                    _ => amt,
                };
                let Val::B { ty, n } = &x else {
                    return err(line, format!("{f} of {}", x.ty().name()));
                };
                let signed = self.num_of(&x).map(|p| p.1).unwrap_or(false);
                let r = match f {
                    "shift_left" | "shl" => self.shift(*n, &amt, true, false, line)?,
                    "shift_right" | "shr" => self.shift(*n, &amt, false, signed, line)?,
                    "rotate_left" => self.rotate(*n, &amt, true, line)?,
                    _ => self.rotate(*n, &amt, false, line)?,
                };
                Val::B {
                    ty: ty.clone(),
                    n: r,
                }
            }
            "and_reduce" | "or_reduce" | "xor_reduce" | "nand_reduce" | "nor_reduce"
            | "xnor_reduce" => {
                let (_, n) = self.eval_bits(
                    &args
                        .first()
                        .ok_or_else(|| format!("line {line}: {f} needs an argument"))?
                        .actual,
                    None,
                    line,
                )?;
                let op = match f {
                    "and_reduce" => UnOp::RAnd,
                    "or_reduce" => UnOp::ROr,
                    "xor_reduce" => UnOp::RXor,
                    "nand_reduce" => UnOp::RNand,
                    "nor_reduce" => UnOp::RNor,
                    _ => UnOp::RXnor,
                };
                let r = self.reduce(op, n);
                Val::B {
                    ty: Ty::Logic,
                    n: r,
                }
            }
            "std_match" => {
                let ea = &args
                    .first()
                    .ok_or_else(|| format!("line {line}: std_match needs two arguments"))?
                    .actual;
                let eb = &args
                    .get(1)
                    .ok_or_else(|| format!("line {line}: std_match needs two arguments"))?
                    .actual;
                if let Some(m) = meta_lit(eb) {
                    let a = self.eval(ea, None, line)?;
                    let n = self.masked_eq(&a, &m, line)?;
                    Val::B { ty: Ty::Bool, n }
                } else if let Some(m) = meta_lit(ea) {
                    let b = self.eval(eb, None, line)?;
                    let n = self.masked_eq(&b, &m, line)?;
                    Val::B { ty: Ty::Bool, n }
                } else {
                    let (a, b) = self.eval2(ea, eb, line)?;
                    let Val::B { n: na, .. } = a else {
                        return err(line, "bad std_match");
                    };
                    let Val::B { n: nb, .. } = b else {
                        return err(line, "bad std_match");
                    };
                    if self.dag.w(na) != self.dag.w(nb) {
                        let n = self.dag.bit(false);
                        Val::B { ty: Ty::Bool, n }
                    } else {
                        let n = self.dag.eq(na, nb);
                        Val::B { ty: Ty::Bool, n }
                    }
                }
            }
            "maximum" | "minimum" => {
                let a = self.arg(args, 0, None, line)?;
                let b = self.arg(args, 1, Some(&a.ty()), line)?;
                if let (Val::R(x), Val::R(y)) = (&a, &b) {
                    return Ok(Some(Val::R(if f == "maximum" {
                        x.max(*y)
                    } else {
                        x.min(*y)
                    })));
                }
                if let (Some(x), Some(y)) = (self.val_to_int(&a), self.val_to_int(&b)) {
                    if a.ty().is_int() {
                        return Ok(Some(self.int_val(if f == "maximum" {
                            x.max(y)
                        } else {
                            x.min(y)
                        })));
                    }
                }
                let lt = self.compare(BinOp::Lt, a.clone(), b.clone(), line)?;
                let Val::B { n: c, .. } = lt else {
                    unreachable!()
                };
                match (&a, &b) {
                    (
                        Val::B {
                            ty: Ty::Int { lo: al, hi: ah },
                            ..
                        },
                        Val::B {
                            ty: Ty::Int { lo: bl, hi: bh },
                            ..
                        },
                    ) => {
                        let rt = Ty::Int {
                            lo: (*al).min(*bl),
                            hi: (*ah).max(*bh),
                        };
                        let x = self.convert(a.clone(), &rt, line)?;
                        let y = self.convert(b.clone(), &rt, line)?;
                        let (Val::B { n: xn, .. }, Val::B { n: yn, .. }) = (x, y) else {
                            unreachable!()
                        };
                        let r = if f == "maximum" {
                            self.dag.mux(c, yn, xn)
                        } else {
                            self.dag.mux(c, xn, yn)
                        };
                        Val::B { ty: rt, n: r }
                    }
                    (Val::B { ty, n: xn }, Val::B { n: yn, .. })
                        if self.dag.w(*xn) == self.dag.w(*yn) =>
                    {
                        let r = if f == "maximum" {
                            self.dag.mux(c, *yn, *xn)
                        } else {
                            self.dag.mux(c, *xn, *yn)
                        };
                        Val::B {
                            ty: ty.clone(),
                            n: r,
                        }
                    }
                    _ => return err(line, format!("{f} of these operands is not supported")),
                }
            }
            "to_01"
            | "to_x01"
            | "to_ux01"
            | "to_x01z"
            | "to_stdulogic"
            | "to_bit"
            | "to_stdlogicvector"
            | "to_stdulogicvector"
            | "to_bitvector"
            | "to_slv"
            | "to_sulv"
            | "to_std_logic_vector"
            | "to_std_ulogic_vector" => {
                let x = self.arg(args, 0, None, line)?;
                let Val::B { ty, n } = x else {
                    return err(line, format!("{f} of a non-hardware value"));
                };
                let ty = match f {
                    "to_stdulogic" => Ty::Logic,
                    "to_bit" => Ty::Bit,
                    "to_stdlogicvector"
                    | "to_stdulogicvector"
                    | "to_slv"
                    | "to_sulv"
                    | "to_std_logic_vector"
                    | "to_std_ulogic_vector" => {
                        let w = self.dag.w(n);
                        slv_ty(w, ArrKind::Slv, Ty::Logic)
                    }
                    "to_bitvector" => {
                        let w = self.dag.w(n);
                        slv_ty(w, ArrKind::BitVec, Ty::Bit)
                    }
                    _ => ty,
                };
                Val::B { ty, n }
            }
            "is_x" => {
                let n = self.dag.bit(false);
                Val::B { ty: Ty::Bool, n }
            }
            "to_string" | "to_hstring" | "to_ostring" | "to_bstring" | "image" | "justify" => {
                Val::S(String::new())
            }
            "log2" | "ceil" | "floor" | "round" | "trunc" | "sqrt" | "exp" | "log" | "log10"
            | "sign" | "realmax" | "realmin" | "sin" | "cos" | "arctan" => {
                let x = match self.arg(args, 0, Some(&Ty::Real), line)? {
                    Val::R(r) => r,
                    v => self
                        .val_to_int(&v)
                        .ok_or_else(|| format!("line {line}: {f}() needs a static real"))?
                        as f64,
                };
                let r = match f {
                    "log2" => x.log2(),
                    "ceil" => x.ceil(),
                    "floor" => x.floor(),
                    "round" => x.round(),
                    "trunc" => x.trunc(),
                    "sqrt" => x.sqrt(),
                    "exp" => x.exp(),
                    "log" => {
                        if args.len() > 1 {
                            let b = match self.arg(args, 1, Some(&Ty::Real), line)? {
                                Val::R(r) => r,
                                v => self.val_to_int(&v).unwrap_or(10) as f64,
                            };
                            x.ln() / b.ln()
                        } else {
                            x.ln()
                        }
                    }
                    "log10" => x.log10(),
                    "sign" => x.signum(),
                    "sin" => x.sin(),
                    "cos" => x.cos(),
                    "arctan" => x.atan(),
                    _ => {
                        let y = match self.arg(args, 1, Some(&Ty::Real), line)? {
                            Val::R(r) => r,
                            v => self.val_to_int(&v).unwrap_or(0) as f64,
                        };
                        if f == "realmax" { x.max(y) } else { x.min(y) }
                    }
                };
                Val::R(r)
            }
            _ => return Ok(None),
        };
        let _ = want;
        Ok(Some(v))
    }

    // ------------------------------------------------------------------
    // Assignment targets

    pub fn lval(&mut self, e: &Expr, line: Line) -> R<LVal> {
        match e {
            Expr::Name(n) => self.lval_name(n, line),
            Expr::Agg(_) => err(line, "aggregate assignment targets are not supported"),
            _ => err(line, "invalid assignment target"),
        }
    }

    fn lval_name(&mut self, n: &Name, line: Line) -> R<LVal> {
        match n {
            Name::Id(x, _) => match self.lookup(x).cloned() {
                Some(Sym::Sig(s)) => Ok(LVal {
                    base: LBase::Net(s.net),
                    off: Off::Static(s.off),
                    ty: s.ty,
                }),
                Some(Sym::Var(slot, ty)) => Ok(LVal {
                    base: LBase::Var(slot),
                    off: Off::Static(0),
                    ty,
                }),
                Some(_) => err(line, format!("`{x}` is not a signal or variable")),
                None => err(line, format!("unknown name `{x}`")),
            },
            Name::Sel(p, f) => {
                if self.is_pkg_prefix(p) {
                    return self.lval_name(&Name::Id(f.clone(), line), line);
                }
                let b = self.lval_name(p, line)?;
                let Ty::Rec(r) = &b.ty else {
                    return err(line, format!("`.{f}` on a non-record"));
                };
                let mut off = 0u32;
                let mut ft = None;
                for (name, t) in r.fields.iter().rev() {
                    if name == f {
                        ft = Some(t.clone());
                        break;
                    }
                    off += t.width()?;
                }
                let ft =
                    ft.ok_or_else(|| format!("line {line}: record {} has no field `{f}`", r.name))?;
                Ok(LVal {
                    base: b.base,
                    off: add_off(b.off, off),
                    ty: ft,
                })
            }
            Name::Call(p, args) => {
                let b = self.lval_name(p, line)?;
                if args.len() == 1 {
                    if let Some(r) = self.arg_is_range(&args[0]) {
                        return self.lval_slice(b, &r, line);
                    }
                }
                let Ty::Arr(a) = &b.ty else {
                    return err(line, "indexing a non-array target");
                };
                if args.len() != 1 {
                    return err(line, "multi-dimensional indexing is not supported");
                }
                let r = a
                    .idx
                    .ok_or_else(|| format!("line {line}: unconstrained target"))?;
                let ew = a.elem.width()?;
                let iv = self.eval(&args[0].actual, Some(&a.idx_ty), line)?;
                if let Some(i) = self.val_to_int(&iv) {
                    let p = r
                        .pos(i)
                        .ok_or_else(|| format!("line {line}: index {i} outside {}", b.ty.name()))?;
                    return Ok(LVal {
                        base: b.base,
                        off: add_off(b.off, p * ew),
                        ty: a.elem.clone(),
                    });
                }
                let cands = self.index_candidates(&iv, &r);
                let mut alts: Vec<(N, u32)> = Vec::new();
                for (i, p) in cands {
                    let c = self.idx_eq(&iv, i, line)?;
                    alts.push((c, p * ew));
                }
                let off = match b.off {
                    Off::Static(o) => Off::Dyn(alts.into_iter().map(|(c, x)| (c, x + o)).collect()),
                    Off::Dyn(prev) => {
                        let mut v = Vec::new();
                        for (c1, o1) in &prev {
                            for (c2, o2) in &alts {
                                let c = self.dag.and(*c1, *c2);
                                v.push((c, o1 + o2));
                            }
                        }
                        if v.len() > MAX_DYN {
                            return err(line, "dynamic target index over too many elements");
                        }
                        Off::Dyn(v)
                    }
                };
                Ok(LVal {
                    base: b.base,
                    off,
                    ty: a.elem.clone(),
                })
            }
            Name::Slice(p, r) => {
                let b = self.lval_name(p, line)?;
                self.lval_slice(b, r, line)
            }
            _ => err(line, "invalid assignment target"),
        }
    }

    fn lval_slice(&mut self, b: LVal, r: &Range, line: Line) -> R<LVal> {
        let Ty::Arr(a) = &b.ty else {
            return err(line, "slicing a non-array target");
        };
        let ar = a
            .idx
            .ok_or_else(|| format!("line {line}: unconstrained target"))?;
        let (sr, _) = self.eval_range(r, line).map_err(|e| {
            if e.contains("not static") {
                format!("line {line}: slice bounds must be static")
            } else {
                e
            }
        })?;
        let ew = a.elem.width()?;
        if sr.len() == 0 {
            return Ok(LVal {
                base: b.base,
                off: b.off,
                ty: b.ty.with_idx(sr),
            });
        }
        if sr.down != ar.down {
            return err(line, "slice direction differs from the array's");
        }
        let pl = ar
            .pos(sr.left)
            .ok_or_else(|| format!("line {line}: slice bound {} outside the array", sr.left))?;
        let pr = ar
            .pos(sr.right)
            .ok_or_else(|| format!("line {line}: slice bound {} outside the array", sr.right))?;
        Ok(LVal {
            base: b.base,
            off: add_off(b.off, pl.min(pr) * ew),
            ty: b.ty.with_idx(sr),
        })
    }
}

fn add_off(o: Off, k: u32) -> Off {
    match o {
        Off::Static(x) => Off::Static(x + k),
        Off::Dyn(v) => Off::Dyn(v.into_iter().map(|(c, x)| (c, x + k)).collect()),
    }
}

pub fn bits_n(v: &Val, line: Line) -> R<N> {
    match v {
        Val::B { n, .. } => Ok(*n),
        _ => err(
            line,
            format!("{} value used where hardware is needed", v.ty().name()),
        ),
    }
}
