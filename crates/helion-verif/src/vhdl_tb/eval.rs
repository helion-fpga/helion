//! Expression evaluation and function execution (functions never suspend).

use std::rc::Rc;

use super::ast::BinOp;
use super::ir::*;
use super::ops;
use super::value::*;

/// What the evaluator needs from the simulation kernel.
pub trait Host {
    fn sig(&self, id: SigId) -> &Value;
    fn event(&self, id: SigId) -> bool;
    fn last_value(&self, id: SigId) -> &Value;
    fn last_event(&self, id: SigId) -> Option<i64>;
    fn now(&self) -> i64;
    /// `report` / failed `assert` (severity 0..=3) from inside a function.
    fn report(&mut self, sev: u32, msg: String, loc: Loc);
}

pub struct Mach<'a> {
    pub funcs: &'a [Func],
    pub host: &'a mut dyn Host,
    pub depth: u32,
}

/// One resolved step of a target path.
#[derive(Clone, Copy, Debug)]
pub enum PathSel {
    Pos(usize),
    Range(usize, usize),
    Field(usize),
}

pub fn read_path(v: &Value, path: &[PathSel]) -> Result<Value, String> {
    let mut cur = v.clone();
    for s in path {
        cur = match (s, &cur) {
            (PathSel::Pos(p), Value::Arr(a)) => a.elems.get(*p).cloned().ok_or("index out of range")?,
            (PathSel::Range(p, n), Value::Arr(a)) => {
                let elems = a.elems.get(*p..p + n).ok_or("slice out of range")?.to_vec();
                let left = if a.asc { a.left + *p as i64 } else { a.left - *p as i64 };
                Value::Arr(Rc::new(ArrVal { kind: a.kind, left, asc: a.asc, elems }))
            }
            (PathSel::Field(f), Value::Rec(r)) => r.get(*f).cloned().ok_or("bad record field")?,
            _ => return Err("selection of a non-composite value".into()),
        };
    }
    Ok(cur)
}

/// Replace the part of `v` at `path` with `new`.
pub fn write_path(v: &mut Value, path: &[PathSel], new: Value) -> Result<(), String> {
    let Some((first, rest)) = path.split_first() else {
        *v = new;
        return Ok(());
    };
    match (first, v) {
        (PathSel::Pos(p), Value::Arr(a)) => {
            let a = Rc::make_mut(a);
            let slot = a.elems.get_mut(*p).ok_or("index out of range")?;
            write_path(slot, rest, new)
        }
        (PathSel::Range(p, n), Value::Arr(a)) => {
            let a = Rc::make_mut(a);
            let src = match &new {
                Value::Arr(b) => b.elems.clone(),
                other => vec![other.clone()],
            };
            if src.len() != *n {
                return Err(format!("slice assignment length mismatch ({} vs {})", n, src.len()));
            }
            for (k, e) in src.into_iter().enumerate() {
                *a.elems.get_mut(p + k).ok_or("slice out of range")? = e;
            }
            Ok(())
        }
        (PathSel::Field(f), Value::Rec(r)) => {
            let r = Rc::make_mut(r);
            let slot = r.get_mut(*f).ok_or("bad record field")?;
            write_path(slot, rest, new)
        }
        _ => Err("selection of a non-composite value".into()),
    }
}

/// Fit `v` to type `ty` for an assignment: array values take the target's
/// index range (lengths must match), integers are range-checked.
pub fn coerce(v: Value, ty: &Ty) -> Result<Value, String> {
    match (ty, v) {
        (Ty::Array(at), Value::Arr(a)) => {
            if let Some((l, r, asc)) = at.range {
                let n = len_of(l, r, asc);
                if n != a.elems.len() {
                    return Err(format!(
                        "length mismatch: {} has {n} elements, value has {}",
                        at.name,
                        a.elems.len()
                    ));
                }
                if a.left == l && a.asc == asc && a.kind == at.kind {
                    return Ok(Value::Arr(a));
                }
                return Ok(Value::Arr(Rc::new(ArrVal { kind: at.kind, left: l, asc, elems: a.elems.clone() })));
            }
            Ok(with_kind(&Value::Arr(a), at.kind))
        }
        (Ty::Int { lo, hi }, Value::Int(i)) => {
            if i < *lo || i > *hi {
                return Err(format!("value {i} out of range {lo} to {hi}"));
            }
            Ok(Value::Int(i))
        }
        (Ty::Line, Value::Arr(a)) => Ok(with_kind(&Value::Arr(a), ArrKind::Str)),
        (_, v) => Ok(v),
    }
}

pub fn cond_true(v: &Value) -> Result<bool, String> {
    match v {
        Value::Bool(b) => Ok(*b),
        Value::Ch(c) => Ok(matches!(c, b'1' | b'H')),
        _ => Err("condition is not boolean".into()),
    }
}

impl Mach<'_> {
    pub fn eval(&mut self, e: &RExpr, vars: &mut Vec<Value>) -> Result<Value, String> {
        self.eval_want(e, vars, None)
    }

    /// Evaluate with an expected type (gives `others` aggregates a length).
    pub fn eval_want(&mut self, e: &RExpr, vars: &mut Vec<Value>, want: Option<&Ty>) -> Result<Value, String> {
        Ok(match e {
            RExpr::Const(v) => v.clone(),
            RExpr::Sig(id) => self.host.sig(*id).clone(),
            RExpr::Var(s) => vars.get(*s).cloned().ok_or("variable slot out of range")?,
            RExpr::Index(a, i) => {
                let av = self.eval(a, vars)?;
                let iv = self.eval(i, vars)?;
                let Value::Arr(arr) = &av else {
                    return Err("indexing a non-array value".into());
                };
                let idx = iv.as_int().or_else(|| match &iv {
                    Value::Ch(c) => Some(*c as i64),
                    _ => None,
                }).ok_or("array index is not discrete")?;
                let p = arr.pos(idx).ok_or_else(|| {
                    format!("index {idx} out of range {} {} {}", arr.left, if arr.asc { "to" } else { "downto" }, arr.right())
                })?;
                arr.elems[p].clone()
            }
            RExpr::Slice(a, l, asc, r) => {
                let av = self.eval(a, vars)?;
                let l = self.eval(l, vars)?.as_int().ok_or("slice bound is not an integer")?;
                let r = self.eval(r, vars)?.as_int().ok_or("slice bound is not an integer")?;
                let Value::Arr(arr) = &av else {
                    return Err("slicing a non-array value".into());
                };
                let n = len_of(l, r, *asc);
                if n == 0 {
                    return Ok(Value::Arr(Rc::new(ArrVal { kind: arr.kind, left: l, asc: *asc, elems: Vec::new() })));
                }
                let (p0, p1) = (arr.pos(l), arr.pos(r));
                let (Some(p0), Some(p1)) = (p0, p1) else {
                    return Err(format!("slice ({l} {} {r}) out of range", if *asc { "to" } else { "downto" }));
                };
                let (s, e) = (p0.min(p1), p0.max(p1));
                Value::Arr(Rc::new(ArrVal { kind: arr.kind, left: l, asc: *asc, elems: arr.elems[s..=e].to_vec() }))
            }
            RExpr::Field(r, f) => match self.eval(r, vars)? {
                Value::Rec(fields) => fields.get(*f).cloned().ok_or("bad record field")?,
                _ => return Err("field of a non-record value".into()),
            },
            RExpr::Un(op, a) => {
                let v = self.eval(a, vars)?;
                ops::unop(*op, &v)?
            }
            RExpr::Bin(op, a, b) => {
                // Short-circuit boolean and/or.
                if matches!(op, BinOp::And | BinOp::Or) {
                    let av = self.eval(a, vars)?;
                    if let Value::Bool(x) = av {
                        if (*op == BinOp::And && !x) || (*op == BinOp::Or && x) {
                            return Ok(Value::Bool(x));
                        }
                    }
                    let bv = self.eval(b, vars)?;
                    return ops::binop(*op, &av, &bv);
                }
                let av = self.eval(a, vars)?;
                let bv = self.eval(b, vars)?;
                ops::binop(*op, &av, &bv)?
            }
            RExpr::Call(f, args) => {
                let mut vals = Vec::with_capacity(args.len());
                let func = self.funcs.get(*f).ok_or("unknown function")?;
                for (k, a) in args.iter().enumerate() {
                    let want = func.param_tys.get(k).cloned().flatten();
                    vals.push(self.eval_want(a, vars, want.as_ref())?);
                }
                self.call(*f, vals)?
            }
            RExpr::Bi(b, args) => self.builtin(*b, args, vars)?,
            RExpr::Event(s) => Value::Bool(self.host.event(*s)),
            RExpr::LastValue(s) => self.host.last_value(*s).clone(),
            RExpr::LastEvent(s) => Value::Time(self.host.last_event(*s).map(|t| self.host.now() - t).unwrap_or(i64::MAX)),
            RExpr::ArrAgg { ty, pos, named, others } => {
                let ty = ty.as_ref().or(want);
                self.array_aggregate(ty, pos, named, others.as_deref(), vars)?
            }
            RExpr::RecAgg(fields) => {
                let mut out = Vec::with_capacity(fields.len());
                for f in fields {
                    out.push(self.eval(f, vars)?);
                }
                Value::Rec(Rc::new(out))
            }
            RExpr::Conv(ty, x) => {
                let v = self.eval_want(x, vars, Some(ty))?;
                convert(v, ty)?
            }
            RExpr::Image(ty, x) => {
                let v = self.eval(x, vars)?;
                Value::str(&image(&v, Some(ty)))
            }
            RExpr::ValueOf(ty, x) => {
                let s = self.eval(x, vars)?.as_text().unwrap_or_default();
                let t = s.trim();
                match ty {
                    Ty::Enum(e) => Value::Enum(
                        e.lits.iter().position(|l| l.eq_ignore_ascii_case(t)).ok_or_else(|| format!("'value: `{t}` is not a {} literal", e.name))? as u32,
                    ),
                    Ty::Bool => Value::Bool(t.eq_ignore_ascii_case("true")),
                    _ => Value::Int(t.parse().map_err(|_| format!("'value: `{t}` is not an integer"))?),
                }
            }
            RExpr::Pos(x) => match self.eval(x, vars)? {
                Value::Enum(p) => Value::Int(p as i64),
                Value::Ch(c) => Value::Int(match b"UX01ZWLH-".iter().position(|x| *x == c) {
                    Some(p) => p as i64,
                    None => c as i64,
                }),
                Value::Bool(b) => Value::Int(b as i64),
                Value::Int(i) => Value::Int(i),
                _ => return Err("'pos of a non-discrete value".into()),
            },
            RExpr::Val(ty, x) => {
                let n = self.eval(x, vars)?.as_int().ok_or("'val needs an integer")?;
                match ty {
                    Ty::Enum(_) | Ty::Severity => Value::Enum(n as u32),
                    Ty::Logic => Value::Ch(*b"UX01ZWLH-".get(n as usize).ok_or("'val out of range")?),
                    Ty::Bit => Value::Ch(if n == 0 { b'0' } else { b'1' }),
                    Ty::Char => Value::Ch(n as u8),
                    Ty::Bool => Value::Bool(n != 0),
                    _ => Value::Int(n),
                }
            }
            RExpr::ArrAttr(a, x) => {
                let v = self.eval(x, vars)?;
                let Value::Arr(arr) = &v else {
                    return Err("array attribute of a non-array value".into());
                };
                match a {
                    ArrAttr::Length => Value::Int(arr.elems.len() as i64),
                    ArrAttr::High => Value::Int(arr.high()),
                    ArrAttr::Low => Value::Int(arr.low()),
                    ArrAttr::Left => Value::Int(arr.left),
                    ArrAttr::Right => Value::Int(arr.right()),
                    ArrAttr::Ascending => Value::Bool(arr.asc),
                }
            }
        })
    }

    fn array_aggregate(
        &mut self,
        ty: Option<&Ty>,
        pos: &[RExpr],
        named: &[(Vec<RChoice>, RExpr)],
        others: Option<&RExpr>,
        vars: &mut Vec<Value>,
    ) -> Result<Value, String> {
        let at = match ty {
            Some(Ty::Array(a)) => Some(a.clone()),
            Some(Ty::Line) => None,
            _ => None,
        };
        let kind = at.as_ref().map(|a| a.kind).unwrap_or(ArrKind::Other);
        let elem_ty = at.as_ref().map(|a| a.elem.clone());
        let ew = elem_ty.as_ref();
        if named.is_empty() && others.is_none() {
            let mut elems = Vec::with_capacity(pos.len());
            for p in pos {
                elems.push(self.eval_want(p, vars, ew)?);
            }
            let (left, asc) = match at.as_ref().and_then(|a| a.range) {
                Some((l, _, asc)) => (l, asc),
                None => (if kind == ArrKind::Str { 1 } else { (elems.len() as i64 - 1).max(0) }, kind == ArrKind::Str),
            };
            let (left, asc) = if kind == ArrKind::Other && at.as_ref().and_then(|a| a.range).is_none() { (0, true) } else { (left, asc) };
            return Ok(Value::Arr(Rc::new(ArrVal { kind, left, asc, elems })));
        }
        // Named / others: need the index range.
        let mut named_vals: Vec<(Vec<(i64, i64)>, Value)> = Vec::new();
        let mut lo_hi: Option<(i64, i64)> = None;
        for (choices, v) in named {
            let val = self.eval_want(v, vars, ew)?;
            let mut spans = Vec::new();
            for c in choices {
                match c {
                    RChoice::Val(x) => {
                        let i = self.eval(x, vars)?.as_int().ok_or("aggregate choice is not discrete")?;
                        spans.push((i, i));
                    }
                    RChoice::Range(a, _, b) => {
                        let a = self.eval(a, vars)?.as_int().ok_or("aggregate range bound")?;
                        let b = self.eval(b, vars)?.as_int().ok_or("aggregate range bound")?;
                        spans.push((a.min(b), a.max(b)));
                    }
                    RChoice::Others => {}
                }
            }
            for (a, b) in &spans {
                lo_hi = Some(match lo_hi {
                    Some((l, h)) => (l.min(*a), h.max(*b)),
                    None => (*a, *b),
                });
            }
            named_vals.push((spans, val));
        }
        let (left, right, asc) = match at.as_ref().and_then(|a| a.range) {
            Some(r) => r,
            None => match (lo_hi, others) {
                (Some((l, h)), None) => (h, l, false),
                _ => return Err("aggregate with `others` needs a constrained target type".into()),
            },
        };
        let n = len_of(left, right, asc);
        let fill = match others {
            Some(o) => self.eval_want(o, vars, ew)?,
            None => elem_ty.as_ref().map(Ty::default_value).unwrap_or(Value::Ch(b'U')),
        };
        let mut elems = vec![fill; n];
        let arr = ArrVal { kind, left, asc, elems: Vec::new() };
        for (k, p) in pos.iter().enumerate() {
            if k < n {
                elems[k] = self.eval_want(p, vars, ew)?;
            }
        }
        for (spans, val) in named_vals {
            for (a, b) in spans {
                for i in a..=b {
                    if let Some(p) = arr.pos(i).filter(|p| *p < n) {
                        elems[p] = val.clone();
                    }
                }
            }
        }
        Ok(Value::Arr(Rc::new(ArrVal { kind, left, asc, elems })))
    }

    fn builtin(&mut self, b: Bi, args: &[RExpr], vars: &mut Vec<Value>) -> Result<Value, String> {
        if matches!(b, Bi::RisingEdge | Bi::FallingEdge) {
            let Some(RExpr::Sig(s)) = args.first() else {
                return Err("rising_edge/falling_edge need a signal".into());
            };
            let cur = to_x01(match self.host.sig(*s) {
                Value::Ch(c) => *c,
                _ => b'X',
            });
            let last = to_x01(match self.host.last_value(*s) {
                Value::Ch(c) => *c,
                _ => b'X',
            });
            let ev = self.host.event(*s);
            return Ok(Value::Bool(if b == Bi::RisingEdge {
                ev && cur == b'1' && last == b'0'
            } else {
                ev && cur == b'0' && last == b'1'
            }));
        }
        if b == Bi::Now {
            return Ok(Value::Time(self.host.now()));
        }
        let mut v = Vec::with_capacity(args.len());
        for a in args {
            v.push(self.eval(a, vars)?);
        }
        let int = |x: &Value| x.as_int().ok_or_else(|| "expected an integer argument".to_string());
        Ok(match b {
            Bi::ToUnsigned | Bi::ToSigned => {
                let i = int(&v[0])?;
                let n = int(v.get(1).ok_or("to_unsigned needs a size")?)?.max(0) as usize;
                if b == Bi::ToUnsigned && i < 0 {
                    return Err(format!("to_unsigned of a negative value ({i})"));
                }
                int_to_vec(if b == Bi::ToUnsigned { ArrKind::Unsigned } else { ArrKind::Signed }, i as i128, n)
            }
            Bi::ToInteger => match &v[0] {
                Value::Arr(a) => {
                    let s = a.kind == ArrKind::Signed;
                    // numeric_std returns 0 (with a warning) for metavalues.
                    Value::Int(vec_to_int(&v[0], s).unwrap_or(0) as i64)
                }
                Value::Int(i) => Value::Int(*i),
                Value::Real(r) => Value::Int(r.round() as i64),
                Value::Ch(c) => Value::Int(i64::from(matches!(c, b'1' | b'H'))),
                _ => return Err("to_integer of a non-numeric value".into()),
            },
            Bi::Resize => ops::resize(&v[0], int(&v[1])?.max(0) as usize)?,
            Bi::ShiftLeft | Bi::ShiftRight | Bi::RotateLeft | Bi::RotateRight => {
                let Value::Arr(a) = &v[0] else {
                    return Err("shift of a non-vector".into());
                };
                let n = int(&v[1])?;
                let op = match (b, a.kind == ArrKind::Signed) {
                    (Bi::ShiftLeft, _) => BinOp::Sll,
                    (Bi::ShiftRight, true) => BinOp::Sra,
                    (Bi::ShiftRight, false) => BinOp::Srl,
                    (Bi::RotateLeft, _) => BinOp::Rol,
                    _ => BinOp::Ror,
                };
                ops::binop(op, &v[0], &Value::Int(n))?
            }
            Bi::ToString => Value::str(&image(&v[0], None)),
            Bi::ToHString => Value::str(&hex_string(&v[0])),
            Bi::ToBString => Value::str(&image(&v[0], None)),
            Bi::StdMatch => {
                let (a, c) = (v[0].bits().unwrap_or_default(), v[1].bits().unwrap_or_default());
                Value::Bool(
                    a.len() == c.len()
                        && a.iter().zip(&c).all(|(x, y)| *x == b'-' || *y == b'-' || (to_x01(*x) == to_x01(*y) && to_x01(*x) != b'X')),
                )
            }
            Bi::IsX => Value::Bool(v[0].bits().unwrap_or_default().iter().any(|c| to_x01(*c) == b'X')),
            Bi::ToX01 => match &v[0] {
                Value::Ch(c) => Value::Ch(to_x01(*c)),
                Value::Arr(a) => Value::Arr(Rc::new(ArrVal {
                    kind: a.kind,
                    left: a.left,
                    asc: a.asc,
                    elems: a.elems.iter().map(|e| if let Value::Ch(c) = e { Value::Ch(to_x01(*c)) } else { e.clone() }).collect(),
                })),
                other => other.clone(),
            },
            Bi::Minimum | Bi::Maximum => {
                let lt = ops::binop(BinOp::Lt, &v[0], &v[1])?;
                let a_smaller = lt == Value::Bool(true);
                if (b == Bi::Minimum) == a_smaller { v[0].clone() } else { v[1].clone() }
            }
            Bi::ToReal => Value::Real(match &v[0] {
                Value::Int(i) => *i as f64,
                Value::Real(r) => *r,
                _ => return Err("real() of a non-numeric value".into()),
            }),
            Bi::ToIntegerFromReal => Value::Int(match &v[0] {
                Value::Real(r) => r.round() as i64,
                Value::Int(i) => *i,
                _ => return Err("integer() of a non-numeric value".into()),
            }),
            Bi::Floor | Bi::Round | Bi::Ceil | Bi::Sqrt => {
                let Value::Real(r) = v[0] else {
                    return Err("math_real function of a non-real value".into());
                };
                Value::Real(match b {
                    Bi::Floor => r.floor(),
                    Bi::Round => r.round(),
                    Bi::Ceil => r.ceil(),
                    _ => r.sqrt(),
                })
            }
            Bi::ToSlv => with_kind(&v[0], ArrKind::Slv),
            Bi::RisingEdge | Bi::FallingEdge | Bi::Now => unreachable!(),
        })
    }

    /// Call user function `f` with evaluated arguments.
    pub fn call(&mut self, f: usize, args: Vec<Value>) -> Result<Value, String> {
        if self.depth > 200 {
            return Err("function recursion deeper than 200".into());
        }
        let func = self.funcs.get(f).ok_or("unknown function")?;
        let code = func.code.clone();
        let mut vars: Vec<Value> = code.var_tys.iter().map(|t| t.as_ref().map(Ty::default_value).unwrap_or(Value::Int(0))).collect();
        for (k, a) in args.into_iter().enumerate().take(func.nparams) {
            vars[k] = a;
        }
        self.depth += 1;
        let r = self.run_function(&code, &mut vars);
        self.depth -= 1;
        let name = &self.funcs[f].name;
        r.map_err(|e| format!("in function {name}: {e}"))?
            .ok_or_else(|| format!("function {name} ended without `return`"))
    }

    fn run_function(&mut self, code: &Code, vars: &mut Vec<Value>) -> Result<Option<Value>, String> {
        let mut pc = 0usize;
        let mut steps = 0u64;
        while pc < code.ops.len() {
            steps += 1;
            if steps > 50_000_000 {
                return Err("function did not terminate (50M steps)".into());
            }
            match self.step_common(&code.ops[pc], pc, vars)? {
                Flow::Next => pc += 1,
                Flow::Jump(t) => pc = t,
                Flow::Return(v) => return Ok(v),
                Flow::Unhandled => return Err("signal assignment / wait inside a function".into()),
            }
        }
        Ok(None)
    }

    /// Execute the ops shared by processes and functions.
    pub fn step_common(&mut self, op: &Op, pc: usize, vars: &mut Vec<Value>) -> Result<Flow, String> {
        match op {
            Op::VarAssign { target, value, ty, .. } => {
                let v = self.eval_want(value, vars, ty.as_ref())?;
                self.assign_var(target, v, ty.as_ref(), vars)?;
                Ok(Flow::Next)
            }
            Op::Jump(t) => Ok(Flow::Jump(*t)),
            Op::JumpIfNot(c, t, _) => {
                let v = self.eval(c, vars)?;
                Ok(if cond_true(&v)? { Flow::Next } else { Flow::Jump(*t) })
            }
            Op::Case { sel, arms, end, .. } => {
                let s = self.eval(sel, vars)?;
                for (choices, target) in arms {
                    for c in choices {
                        let hit = match c {
                            RChoice::Others => true,
                            RChoice::Val(x) => {
                                let w = self.eval(x, vars)?;
                                ops::binop(BinOp::Eq, &s, &w).map(|r| r == Value::Bool(true)).unwrap_or(false)
                            }
                            RChoice::Range(a, _, b) => {
                                let a = self.eval(a, vars)?;
                                let b = self.eval(b, vars)?;
                                let ge = |x: &Value, y: &Value| ops::binop(BinOp::Ge, x, y).map(|r| r == Value::Bool(true)).unwrap_or(false);
                                (ge(&s, &a) && ge(&b, &s)) || (ge(&s, &b) && ge(&a, &s))
                            }
                        };
                        if hit {
                            return Ok(Flow::Jump(*target));
                        }
                    }
                }
                Ok(Flow::Jump(*end))
            }
            Op::ForInit { var, end_slot, dir_slot, range, exit, .. } => {
                let (start, end, asc) = self.range(range, vars)?;
                vars[*var] = Value::Int(start);
                vars[*end_slot] = Value::Int(end);
                vars[*dir_slot] = Value::Bool(asc);
                let empty = if asc { start > end } else { start < end };
                Ok(if empty { Flow::Jump(*exit) } else { Flow::Next })
            }
            Op::ForNext { var, end_slot, dir_slot, body } => {
                let cur = vars[*var].as_int().unwrap_or(0);
                let end = vars[*end_slot].as_int().unwrap_or(0);
                let asc = vars[*dir_slot] == Value::Bool(true);
                if cur == end {
                    return Ok(Flow::Next);
                }
                vars[*var] = Value::Int(if asc { cur + 1 } else { cur - 1 });
                Ok(Flow::Jump(*body))
            }
            Op::Report { msg, sev, loc } => {
                let m = self.eval(msg, vars)?.as_text().unwrap_or_default();
                let s = match sev {
                    Some(x) => self.eval(x, vars)?.as_int().unwrap_or(0) as u32,
                    None => 0,
                };
                self.host.report(s, m, *loc);
                Ok(Flow::Next)
            }
            Op::Assert { cond, msg, sev, loc } => {
                let c = self.eval(cond, vars)?;
                if !cond_true(&c)? {
                    let m = match msg {
                        Some(x) => self.eval(x, vars)?.as_text().unwrap_or_default(),
                        None => "Assertion violation.".into(),
                    };
                    let s = match sev {
                        Some(x) => self.eval(x, vars)?.as_int().unwrap_or(2) as u32,
                        None => 2,
                    };
                    self.host.report(s, m, *loc);
                }
                Ok(Flow::Next)
            }
            Op::Return(v, _) => Ok(Flow::Return(match v {
                Some(x) => Some(self.eval(x, vars)?),
                None => None,
            })),
            Op::Builtin { proc_, args, outs, .. } if *proc_ != BuiltinProc::Stop && *proc_ != BuiltinProc::WriteLine => {
                self.builtin_proc(*proc_, args, outs, vars)?;
                Ok(Flow::Next)
            }
            Op::Nop => Ok(Flow::Next),
            _ => {
                let _ = pc;
                Ok(Flow::Unhandled)
            }
        }
    }

    pub fn builtin_proc(&mut self, p: BuiltinProc, args: &[RExpr], outs: &[RTarget], vars: &mut Vec<Value>) -> Result<(), String> {
        match p {
            BuiltinProc::Write => {
                let v = self.eval(&args[0], vars)?;
                let mode = args.get(1).map(|m| self.eval(m, vars)).transpose()?.and_then(|m| m.as_int()).unwrap_or(0);
                let text = match (mode, &v) {
                    (1, _) => hex_string(&v),
                    (_, Value::Bool(b)) => if *b { "TRUE".into() } else { "FALSE".into() },
                    (_, Value::Ch(c)) => (*c as char).to_string(),
                    _ => image(&v, None),
                };
                let target = &outs[0];
                let cur = self.read_target(target, vars)?.as_text().unwrap_or_default();
                self.assign_var(target, Value::str(&format!("{cur}{text}")), Some(&Ty::Line), vars)
            }
            BuiltinProc::Uniform => {
                let mut s1 = self.read_target(&outs[0], vars)?.as_int().unwrap_or(1);
                let mut s2 = self.read_target(&outs[1], vars)?.as_int().unwrap_or(1);
                let k = s1 / 53668;
                s1 = 40014 * (s1 - k * 53668) - k * 12211;
                if s1 < 0 {
                    s1 += 2147483563;
                }
                let k = s2 / 52774;
                s2 = 40692 * (s2 - k * 52774) - k * 3791;
                if s2 < 0 {
                    s2 += 2147483399;
                }
                let mut z = s1 - s2;
                if z < 1 {
                    z += 2147483562;
                }
                self.assign_var(&outs[0], Value::Int(s1), None, vars)?;
                self.assign_var(&outs[1], Value::Int(s2), None, vars)?;
                self.assign_var(&outs[2], Value::Real(z as f64 * 4.656613e-10), None, vars)
            }
            BuiltinProc::Deallocate => Ok(()),
            BuiltinProc::Stop | BuiltinProc::WriteLine => Err("process-only builtin".into()),
        }
    }

    pub fn range(&mut self, r: &RRange, vars: &mut Vec<Value>) -> Result<(i64, i64, bool), String> {
        match r {
            RRange::Explicit(a, asc, b) => {
                let a = self.eval(a, vars)?;
                let b = self.eval(b, vars)?;
                let ai = a.as_int().or_else(|| if let Value::Ch(c) = a { Some(c as i64) } else { None }).ok_or("range bound is not discrete")?;
                let bi = b.as_int().or_else(|| if let Value::Ch(c) = b { Some(c as i64) } else { None }).ok_or("range bound is not discrete")?;
                Ok((ai, bi, *asc))
            }
            RRange::Of(x, rev) => {
                let v = self.eval(x, vars)?;
                let Value::Arr(a) = &v else {
                    return Err("'range of a non-array value".into());
                };
                if a.elems.is_empty() {
                    return Ok((0, -1, true));
                }
                Ok(if *rev { (a.right(), a.left, !a.asc) } else { (a.left, a.right(), a.asc) })
            }
        }
    }

    /// Resolve a target path's selectors against the current value.
    pub fn resolve_path(&mut self, base: &Value, sels: &[RSel], vars: &mut Vec<Value>) -> Result<Vec<PathSel>, String> {
        let mut out = Vec::with_capacity(sels.len());
        let mut cur = base.clone();
        for s in sels {
            let ps = match (s, &cur) {
                (RSel::Index(i), Value::Arr(a)) => {
                    let iv = self.eval(i, vars)?;
                    let idx = iv.as_int().or_else(|| if let Value::Ch(c) = iv { Some(c as i64) } else { None }).ok_or("index is not discrete")?;
                    PathSel::Pos(a.pos(idx).ok_or_else(|| format!("index {idx} out of range {} {} {}", a.left, if a.asc { "to" } else { "downto" }, a.right()))?)
                }
                (RSel::Slice(l, _, r), Value::Arr(a)) => {
                    let l = self.eval(l, vars)?.as_int().ok_or("slice bound")?;
                    let r = self.eval(r, vars)?.as_int().ok_or("slice bound")?;
                    let (Some(p0), Some(p1)) = (a.pos(l), a.pos(r)) else {
                        return Err(format!("slice ({l}, {r}) out of range"));
                    };
                    PathSel::Range(p0.min(p1), p0.max(p1) - p0.min(p1) + 1)
                }
                (RSel::Field(f), Value::Rec(_)) => PathSel::Field(*f),
                _ => return Err("selection of a non-composite value".into()),
            };
            cur = read_path(&cur, &[ps])?;
            out.push(ps);
        }
        Ok(out)
    }

    pub fn read_target(&mut self, t: &RTarget, vars: &mut Vec<Value>) -> Result<Value, String> {
        match t {
            RTarget::Var(s, sels) => {
                let base = vars[*s].clone();
                let p = self.resolve_path(&base, sels, vars)?;
                read_path(&base, &p)
            }
            RTarget::Sig(id, sels) => {
                let base = self.host.sig(*id).clone();
                let p = self.resolve_path(&base, sels, vars)?;
                read_path(&base, &p)
            }
            RTarget::Agg(_) => Err("aggregate target read".into()),
        }
    }

    pub fn assign_var(&mut self, t: &RTarget, v: Value, ty: Option<&Ty>, vars: &mut Vec<Value>) -> Result<(), String> {
        match t {
            RTarget::Var(s, sels) => {
                let base = vars.get(*s).cloned().ok_or("variable slot out of range")?;
                let path = self.resolve_path(&base, sels, vars)?;
                let v = match (ty, path.is_empty()) {
                    (Some(ty), true) => coerce(v, ty)?,
                    _ => fit_like(v, &read_path(&base, &path)?)?,
                };
                write_path(&mut vars[*s], &path, v)
            }
            RTarget::Agg(parts) => {
                let Value::Arr(a) = v else {
                    return Err("aggregate target needs an array value".into());
                };
                if a.elems.len() != parts.len() {
                    return Err("aggregate target length mismatch".into());
                }
                for (p, e) in parts.iter().zip(a.elems.iter()) {
                    self.assign_var(p, e.clone(), None, vars)?;
                }
                Ok(())
            }
            RTarget::Sig(..) => Err("signal target in a variable assignment".into()),
        }
    }
}

/// Fit an assigned part to the shape of the part it replaces (bounds of
/// arrays, kind of vectors).
pub fn fit_like(v: Value, old: &Value) -> Result<Value, String> {
    match (old, v) {
        (Value::Arr(o), Value::Arr(n)) => {
            if o.elems.len() != n.elems.len() {
                return Err(format!("length mismatch: target has {} elements, value has {}", o.elems.len(), n.elems.len()));
            }
            Ok(Value::Arr(Rc::new(ArrVal { kind: o.kind, left: o.left, asc: o.asc, elems: n.elems.clone() })))
        }
        (_, v) => Ok(v),
    }
}

/// Type conversion / qualification.
pub fn convert(v: Value, ty: &Ty) -> Result<Value, String> {
    Ok(match (ty, v) {
        (Ty::Array(at), Value::Arr(a)) => with_kind(&Value::Arr(a), at.kind),
        (Ty::Int { .. }, Value::Real(r)) => Value::Int(r.round() as i64),
        (Ty::Real, Value::Int(i)) => Value::Real(i as f64),
        (_, v) => v,
    })
}

pub enum Flow {
    Next,
    Jump(usize),
    Return(Option<Value>),
    /// The op is process-only (signal assignment, wait, stop, writeline).
    Unhandled,
}
