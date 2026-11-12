//! Expressions, assignments and procedural code.
//!
//! Expression widths and signedness follow IEEE 1800 section 11.6: a
//! context-determined operand is evaluated at the width of the whole
//! expression and extended by its signedness; self-determined operands
//! (comparisons, concatenations, shift amounts, indices) are sized on their
//! own. Procedural code is executed symbolically: every `if` and `case`
//! merges the values of its branches with multiplexers.

use super::ast::*;
use super::dag::{bits_for_u, Op, N};
use super::elab::{Elab, Sym, Ty, R};
use super::lex::Num;
use super::lower::{Drv, Reg};
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Outside a process: signals are read, nothing is assigned.
    #[default]
    Const,
    Comb,
    Seq,
}

#[derive(Clone, Default)]
pub struct Env {
    pub mode: Mode,
    /// Blocking view of signals assigned by this process.
    pub sig: HashMap<u32, N>,
    /// Pending non-blocking values.
    pub nba: HashMap<u32, N>,
    pub var: HashMap<u32, N>,
    /// Control flow that has left the current path: `None` is "no".
    pub ret: Option<N>,
    pub brk: Option<N>,
    pub cont: Option<N>,
    /// Liveness of the enclosing call: `None` is "live".
    pub outer: Option<N>,
}

/// Bits of each signal assigned anywhere in the current process.
#[derive(Default)]
pub struct ProcState {
    pub blk: HashMap<u32, Vec<bool>>,
    pub nba: HashMap<u32, Vec<bool>>,
}

/// A value with its type.
#[derive(Clone)]
pub struct V {
    pub n: N,
    pub ty: Rc<Ty>,
}

#[derive(Clone, Copy)]
enum Root {
    Sig(u32),
    Var(u32),
}

enum LStep {
    /// A fixed sub-range (member, constant index or slice).
    Fixed(u32, u32),
    /// One of `count` elements of `ew` bits, chosen by position `idx`.
    Elem { idx: N, count: u32, ew: u32 },
    /// `w` bits starting at position `off`.
    Slide { off: N, w: u32 },
}

struct Lv {
    root: Root,
    steps: Vec<LStep>,
    ty: Rc<Ty>,
}

fn num_i64(bits: &[bool], signed: bool) -> i64 {
    let mut v: i64 = 0;
    for (i, &b) in bits.iter().enumerate().take(64) {
        if b {
            v |= 1 << i;
        }
    }
    if signed && !bits.is_empty() && bits.len() < 64 && bits[bits.len() - 1] {
        v |= -1i64 << bits.len();
    }
    v
}

impl Elab {
    // ------------------------------------------------------------- helpers

    fn or_opt(&mut self, a: Option<N>, b: Option<N>) -> Option<N> {
        match (a, b) {
            (None, x) | (x, None) => x,
            (Some(x), Some(y)) => Some(self.dag.or(x, y)),
        }
    }

    /// Whether this path is still executing: `None` is "yes".
    fn live(&mut self, env: &Env) -> Option<N> {
        let gone = self.or_opt(env.ret, env.brk);
        let gone = self.or_opt(gone, env.cont);
        let here = gone.map(|g| self.dag.not(g));
        match (env.outer, here) {
            (None, x) | (x, None) => x,
            (Some(a), Some(b)) => Some(self.dag.and(a, b)),
        }
    }

    fn is_dead(&mut self, env: &Env) -> bool {
        match self.live(env) {
            Some(l) => self.dag.as_const(l).map(|c| !c[0]).unwrap_or(false),
            None => false,
        }
    }

    fn truth(&mut self, n: N) -> N {
        self.dag.red_or(n)
    }

    fn neg(&mut self, a: N) -> N {
        let z = self.dag.zero(self.dag.w(a));
        self.dag.sub(z, a)
    }

    fn ashr(&mut self, a: N, amt: N) -> N {
        let w = self.dag.w(a);
        let sign = self.dag.slice(a, w - 1, 1);
        let na = self.dag.not(a);
        let s1 = self.dag.lshr(na, amt);
        let neg = self.dag.not(s1);
        let pos = self.dag.lshr(a, amt);
        self.dag.mux(sign, neg, pos)
    }

    fn sdivrem(&mut self, a: N, b: N, rem: bool) -> N {
        let w = self.dag.w(a);
        let sa = self.dag.slice(a, w - 1, 1);
        let sb = self.dag.slice(b, w - 1, 1);
        let na = self.neg(a);
        let nb = self.neg(b);
        let ua = self.dag.mux(sa, na, a);
        let ub = self.dag.mux(sb, nb, b);
        if rem {
            let r = self.dag.urem(ua, ub);
            let nr = self.neg(r);
            self.dag.mux(sa, nr, r)
        } else {
            let q = self.dag.udiv(ua, ub);
            let nq = self.neg(q);
            let s = self.dag.xor(sa, sb);
            self.dag.mux(s, nq, q)
        }
    }

    fn const_of(&self, n: N, signed: bool) -> Option<i64> {
        self.dag.as_const(n).map(|b| num_i64(b, signed))
    }

    pub fn const_int(&mut self, e: &Expr, sc: usize) -> R<i64> {
        let v = self.eval_self(e, sc, &mut Env::default())?;
        match self.const_of(v.n, v.ty.signed()) {
            Some(x) => Ok(x),
            None => self.err("this expression must be constant"),
        }
    }

    /// Binary select of one of `elems` by `idx` (element 0 for index 0).
    fn select(&mut self, elems: &[N], idx: N) -> N {
        if elems.len() == 1 {
            return elems[0];
        }
        let bit = bits_for_u(elems.len() as u128 - 1) - 1;
        let half = 1usize << bit;
        let iw = self.dag.w(idx);
        let lo = self.select(&elems[..half], idx);
        if bit >= iw {
            return lo;
        }
        let hi = self.select(&elems[half..], idx);
        let b = self.dag.slice(idx, bit, 1);
        self.dag.mux(b, hi, lo)
    }

    /// Position (0 at the LSB end) of index `i` in a vector or array.
    fn position(&mut self, i: &V, ty: &Ty) -> N {
        let (lo, hi, asc) = match ty {
            Ty::Bits { w, lo, asc, .. } => (*lo, lo + *w as i64 - 1, *asc),
            Ty::Array { lo, hi, asc, .. } => (*lo, *hi, *asc),
            Ty::Struct { w, .. } => (0, *w as i64 - 1, false),
        };
        if lo == 0 && !asc {
            return i.n;
        }
        let w = self.dag.w(i.n).max(34) + 1;
        let x = self.dag.ext(i.n, w, i.ty.signed());
        if asc {
            let k = self.kint(hi, w);
            self.dag.sub(k, x)
        } else {
            let k = self.kint(lo, w);
            self.dag.sub(x, k)
        }
    }

    // ---------------------------------------------------------------- sizes

    /// Type of a reference expression (a name with selects), if it is one.
    fn ref_ty(&mut self, e: &Expr, sc: usize) -> R<Option<Rc<Ty>>> {
        Ok(match e {
            Expr::Id(path) => match self.lookup_path(sc, path)? {
                Sym::Sig(s) => Some(self.sigs[s as usize].ty.clone()),
                Sym::Var(v) => Some(self.vars[v as usize].ty.clone()),
                Sym::Const { ty, .. } => Some(ty),
                Sym::Func(..) => None,
                Sym::Type(_) => return self.err(format!("'{}' is a type, not a value", path.join("::"))),
            },
            Expr::Member(b, f) => {
                let Some(bt) = self.ref_ty(b, sc)? else { return self.err("member of something that is not a struct") };
                match &*bt {
                    Ty::Struct { fields, .. } => match fields.iter().find(|x| &x.0 == f) {
                        Some(x) => Some(x.1.clone()),
                        None => return self.err(format!("no member '{f}'")),
                    },
                    _ => return self.err(format!("'.{f}' used on something that is not a struct (hierarchical references are not supported)")),
                }
            }
            Expr::Index(b, _) => {
                let Some(bt) = self.ref_ty(b, sc)? else { return self.err("index of something that is not a signal") };
                Some(bt.elem().0)
            }
            Expr::Range(b, hi, lo) => {
                let Some(bt) = self.ref_ty(b, sc)? else { return self.err("range of something that is not a signal") };
                let (a, c) = (self.const_int(hi, sc)?, self.const_int(lo, sc)?);
                let n = (a - c).unsigned_abs() as u32 + 1;
                Some(Ty::bits(n * bt.elem().0.w(), false))
            }
            Expr::PlusRange(b, _, w, _) => {
                let Some(bt) = self.ref_ty(b, sc)? else { return self.err("range of something that is not a signal") };
                let n = self.const_int(w, sc)?;
                if n <= 0 {
                    return self.err("part-select width must be positive");
                }
                Some(Ty::bits(n as u32 * bt.elem().0.w(), false))
            }
            _ => None,
        })
    }

    /// Self-determined width and signedness.
    pub fn size(&mut self, e: &Expr, sc: usize) -> R<(u32, bool)> {
        Ok(match e {
            Expr::Num(n) => (if n.fill { 1 } else { n.bits.len() as u32 }, n.signed),
            Expr::Str(s) => ((s.len() as u32 * 8).max(8), false),
            Expr::Id(_) | Expr::Member(..) | Expr::Index(..) | Expr::Range(..) | Expr::PlusRange(..) => match self.ref_ty(e, sc)? {
                Some(t) => {
                    let whole = matches!(e, Expr::Id(_) | Expr::Member(..)) || matches!(e, Expr::Index(b, _) if matches!(self.ref_ty(b, sc)?.as_deref(), Some(Ty::Array { .. })));
                    (t.w(), whole && t.signed())
                }
                None => match e {
                    Expr::Id(path) => self.size(&Expr::Call(path.clone(), vec![]), sc)?,
                    _ => return self.err("cannot size this expression"),
                },
            },
            Expr::Unary(op, a) => match *op {
                "+" | "-" | "~" => self.size(a, sc)?,
                _ => (1, false),
            },
            Expr::Binary(op, a, b) => match *op {
                "+" | "-" | "*" | "/" | "%" | "&" | "|" | "^" | "^~" | "~^" => {
                    let (x, y) = (self.size(a, sc)?, self.size(b, sc)?);
                    (x.0.max(y.0), x.1 && y.1)
                }
                "<<" | ">>" | "<<<" | ">>>" | "**" => self.size(a, sc)?,
                _ => (1, false),
            },
            Expr::Cond(_, a, b) => {
                let (x, y) = (self.size(a, sc)?, self.size(b, sc)?);
                (x.0.max(y.0), x.1 && y.1)
            }
            Expr::Concat(items) => {
                let mut w = 0;
                for i in items {
                    w += self.size(i, sc)?.0;
                }
                (w, false)
            }
            Expr::Repl(n, items) => {
                let k = self.const_int(n, sc)?;
                if k < 0 {
                    return self.err("negative replication count");
                }
                let mut w = 0;
                for i in items {
                    w += self.size(i, sc)?.0;
                }
                (w * k as u32, false)
            }
            Expr::Call(path, _) => match self.lookup_path(sc, path)? {
                Sym::Func(f, fsc) => match &f.ret {
                    Some(t) => {
                        let ty = self.resolve_type(t, fsc)?;
                        (ty.w(), ty.signed())
                    }
                    None => return self.err(format!("'{}' returns no value", f.name)),
                },
                _ => return self.err(format!("'{}' is not a function", path.join("::"))),
            },
            Expr::SysCall(name, args) => match name.as_str() {
                "signed" => (self.size(&args[0], sc)?.0, true),
                "unsigned" => (self.size(&args[0], sc)?.0, false),
                "onehot" | "onehot0" | "isunknown" => (1, false),
                "clog2" | "bits" | "size" | "high" | "low" | "left" | "right" | "countones" | "countbits" => (32, true),
                _ => return self.err(format!("system function '${name}' is not synthesizable")),
            },
            Expr::Cast(to, v) => match &**to {
                CastTo::Type(t) => {
                    let ty = self.resolve_type(t, sc)?;
                    (ty.w(), ty.signed())
                }
                CastTo::Width(n) => {
                    let w = self.const_int(n, sc)?;
                    if w <= 0 {
                        return self.err("cast width must be positive");
                    }
                    (w as u32, self.size(v, sc)?.1)
                }
                CastTo::Signed => (self.size(v, sc)?.0, true),
                CastTo::Unsigned => (self.size(v, sc)?.0, false),
            },
            Expr::Inside(..) => (1, false),
            Expr::Pattern(_) => return self.err("an assignment pattern needs a typed target"),
            Expr::Type(_) => return self.err("a type is not a value"),
        })
    }

    // ----------------------------------------------------------- evaluation

    fn num_node(&mut self, n: &Num, w: u32, signed: bool) -> N {
        if n.fill {
            let b = n.bits[0] && !n.xz[0];
            return self.dag.konst(vec![b; w as usize]);
        }
        let bits: Vec<bool> = n.bits.iter().zip(&n.xz).map(|(b, x)| *b && !*x).collect();
        let k = self.dag.konst(bits);
        self.dag.ext(k, w, signed)
    }

    /// Value of a reference expression with its type.
    fn eval_ref(&mut self, e: &Expr, sc: usize, env: &mut Env) -> R<Option<V>> {
        Ok(Some(match e {
            Expr::Id(path) => match self.lookup_path(sc, path)? {
                Sym::Sig(s) => {
                    let ty = self.sigs[s as usize].ty.clone();
                    let n = match env.sig.get(&s) {
                        Some(&n) => n,
                        None => self.dag.read(s, ty.w()),
                    };
                    V { n, ty }
                }
                Sym::Var(v) => {
                    let ty = self.vars[v as usize].ty.clone();
                    let n = match env.var.get(&v) {
                        Some(&n) => n,
                        None => self.dag.zero(ty.w()),
                    };
                    V { n, ty }
                }
                Sym::Const { n, ty } => V { n, ty },
                Sym::Func(..) => return Ok(None),
                Sym::Type(_) => return self.err(format!("'{}' is a type, not a value", path.join("::"))),
            },
            Expr::Member(b, f) => {
                let Some(bv) = self.eval_ref(b, sc, env)? else { return self.err("member of something that is not a struct") };
                match &*bv.ty {
                    Ty::Struct { fields, .. } => match fields.iter().find(|x| &x.0 == f) {
                        Some((_, fty, off)) => V { n: self.dag.slice(bv.n, *off, fty.w()), ty: fty.clone() },
                        None => return self.err(format!("no member '{f}'")),
                    },
                    _ => return self.err(format!("'.{f}' used on something that is not a struct (hierarchical references are not supported)")),
                }
            }
            Expr::Index(b, i) => {
                let Some(bv) = self.eval_ref(b, sc, env)? else { return self.err("index of something that is not a signal") };
                let (ety, count) = bv.ty.elem();
                let ew = ety.w();
                let iv = self.eval_self(i, sc, env)?;
                if let Some(k) = self.const_of(iv.n, iv.ty.signed()) {
                    match bv.ty.offset(k) {
                        Some(off) => V { n: self.dag.slice(bv.n, off, ew), ty: ety },
                        // Out of range reads as 0 (it is X in simulation).
                        None => V { n: self.dag.zero(ew), ty: ety },
                    }
                } else {
                    let pos = self.position(&iv, &bv.ty);
                    let elems: Vec<N> = (0..count).map(|k| self.dag.slice(bv.n, k * ew, ew)).collect();
                    V { n: self.select(&elems, pos), ty: ety }
                }
            }
            Expr::Range(b, hi, lo) => {
                let Some(bv) = self.eval_ref(b, sc, env)? else { return self.err("range of something that is not a signal") };
                let (a, c) = (self.const_int(hi, sc)?, self.const_int(lo, sc)?);
                let ew = bv.ty.elem().0.w();
                let (Some(o1), Some(o2)) = (bv.ty.offset(a), bv.ty.offset(c)) else {
                    return self.err(format!("part-select [{a}:{c}] is outside the declared range"));
                };
                let (lo_off, n) = (o1.min(o2), (o1.max(o2) - o1.min(o2)) / ew + 1);
                V { n: self.dag.slice(bv.n, lo_off, n * ew), ty: Ty::bits(n * ew, false) }
            }
            Expr::PlusRange(b, start, w, up) => {
                let Some(bv) = self.eval_ref(b, sc, env)? else { return self.err("range of something that is not a signal") };
                let n = self.const_int(w, sc)?;
                if n <= 0 {
                    return self.err("part-select width must be positive");
                }
                let n = n as u32;
                let ew = bv.ty.elem().0.w();
                let sv = self.eval_self(start, sc, env)?;
                if let Some(s) = self.const_of(sv.n, sv.ty.signed()) {
                    let (a, c) = if *up { (s, s + n as i64 - 1) } else { (s, s - n as i64 + 1) };
                    let (Some(o1), Some(o2)) = (bv.ty.offset(a), bv.ty.offset(c)) else {
                        return self.err("indexed part-select is outside the declared range");
                    };
                    V { n: self.dag.slice(bv.n, o1.min(o2), n * ew), ty: Ty::bits(n * ew, false) }
                } else {
                    if ew != 1 {
                        return self.err("a variable indexed part-select of an array is not supported");
                    }
                    // Position of the lowest bit of the slice.
                    let asc = matches!(&*bv.ty, Ty::Bits { asc: true, .. });
                    let p = self.position(&sv, &bv.ty);
                    let low_end = *up != asc;
                    let off = if low_end {
                        p
                    } else {
                        let pw = self.dag.w(p).max(34) + 1;
                        let px = self.dag.ext(p, pw, false);
                        let k = self.kint(n as i64 - 1, pw);
                        self.dag.sub(px, k)
                    };
                    let sh = self.dag.lshr(bv.n, off);
                    let tw = self.dag.w(sh);
                    let sh = if tw < n { self.dag.zext(sh, n) } else { sh };
                    V { n: self.dag.slice(sh, 0, n), ty: Ty::bits(n, false) }
                }
            }
            _ => return Ok(None),
        }))
    }

    pub fn eval_self(&mut self, e: &Expr, sc: usize, env: &mut Env) -> R<V> {
        if matches!(e, Expr::Id(_) | Expr::Member(..) | Expr::Index(..)) {
            if let Some(v) = self.eval_ref(e, sc, env)? {
                return Ok(v);
            }
        }
        let (w, s) = self.size(e, sc)?;
        let n = self.eval(e, w, s, sc, env)?;
        Ok(V { n, ty: Ty::bits(w, s) })
    }

    /// Value of `e` assigned to something `w` bits wide.
    pub fn eval_assign(&mut self, e: &Expr, w: u32, sc: usize, env: &mut Env) -> R<N> {
        let (ew, es) = self.size(e, sc)?;
        let v = self.eval(e, w.max(ew), es, sc, env)?;
        Ok(self.dag.slice(v, 0, w))
    }

    /// Value of `e` assigned to something of type `ty` (patterns allowed).
    pub fn eval_to_type(&mut self, e: &Expr, ty: &Rc<Ty>, sc: usize, env: &mut Env) -> R<N> {
        let Expr::Pattern(items) = e else {
            if let Expr::Cond(c, a, b) = e {
                if matches!(**a, Expr::Pattern(_)) || matches!(**b, Expr::Pattern(_)) {
                    let cv = self.eval_self(c, sc, env)?;
                    let c1 = self.truth(cv.n);
                    let x = self.eval_to_type(a, ty, sc, env)?;
                    let y = self.eval_to_type(b, ty, sc, env)?;
                    return Ok(self.dag.mux(c1, x, y));
                }
            }
            return self.eval_assign(e, ty.w(), sc, env);
        };
        // Slots, MSB first: (name, index, type).
        let slots: Vec<(Option<String>, Option<i64>, Rc<Ty>)> = match &**ty {
            Ty::Struct { fields, .. } => fields.iter().map(|f| (Some(f.0.clone()), None, f.1.clone())).collect(),
            Ty::Array { elem, lo, hi, asc, .. } => {
                let idx: Vec<i64> = if *asc { (*lo..=*hi).collect() } else { (*lo..=*hi).rev().collect() };
                idx.into_iter().map(|i| (None, Some(i), elem.clone())).collect()
            }
            Ty::Bits { w, lo, asc, .. } => {
                let idx: Vec<i64> = if *asc { (*lo..*lo + *w as i64).collect() } else { (*lo..*lo + *w as i64).rev().collect() };
                idx.into_iter().map(|i| (None, Some(i), Ty::bits(1, false))).collect()
            }
        };
        let mut vals: Vec<Option<N>> = vec![None; slots.len()];
        let mut default: Option<&Expr> = None;
        let mut pos = 0usize;
        for (key, val) in items {
            match key {
                PatKey::Default => default = Some(val),
                PatKey::None => {
                    if pos >= slots.len() {
                        return self.err("assignment pattern has too many items");
                    }
                    vals[pos] = Some(self.eval_to_type(val, &slots[pos].2.clone(), sc, env)?);
                    pos += 1;
                }
                PatKey::Repl(n) => {
                    let k = self.const_int(n, sc)?;
                    for _ in 0..k {
                        if pos >= slots.len() {
                            return self.err("assignment pattern has too many items");
                        }
                        vals[pos] = Some(self.eval_to_type(val, &slots[pos].2.clone(), sc, env)?);
                        pos += 1;
                    }
                }
                PatKey::Key(k) => {
                    let slot = match k {
                        Expr::Id(p) if p.len() == 1 && slots.iter().any(|s| s.0.as_deref() == Some(p[0].as_str())) => slots.iter().position(|s| s.0.as_deref() == Some(p[0].as_str())),
                        _ => {
                            let i = self.const_int(k, sc)?;
                            slots.iter().position(|s| s.1 == Some(i))
                        }
                    };
                    let Some(slot) = slot else { return self.err("assignment pattern key does not name a member or index") };
                    vals[slot] = Some(self.eval_to_type(val, &slots[slot].2.clone(), sc, env)?);
                }
            }
        }
        let mut parts = Vec::with_capacity(slots.len());
        for (i, v) in vals.into_iter().enumerate() {
            parts.push(match (v, default) {
                (Some(v), _) => v,
                (None, Some(d)) => self.eval_to_type(d, &slots[i].2.clone(), sc, env)?,
                (None, None) => return self.err("assignment pattern does not cover every element"),
            });
        }
        Ok(self.dag.concat(parts))
    }

    /// `==?`-style match of `sel` against `item` (don't-care bits of a
    /// literal are ignored when `wild`).
    fn matches(&mut self, sel: N, w: u32, s: bool, item: &Expr, wild: bool, sc: usize, env: &mut Env) -> R<N> {
        if let Expr::Num(n) = item {
            if n.xz.iter().any(|x| *x) && !n.fill {
                if !wild {
                    return Ok(self.dag.bit(false));
                }
                let mut care: Vec<bool> = n.xz.iter().map(|x| !*x).collect();
                let mut val: Vec<bool> = n.bits.iter().zip(&n.xz).map(|(b, x)| *b && !*x).collect();
                // Extend by the top digit: an x/z top digit keeps "don't care".
                let top_dc = *n.xz.last().unwrap();
                care.resize(w as usize, !top_dc);
                val.resize(w as usize, false);
                let m = self.dag.konst(care);
                let v = self.dag.konst(val);
                let a = self.dag.and(sel, m);
                return Ok(self.dag.eq(a, v));
            }
        }
        let v = self.eval(item, w, s, sc, env)?;
        Ok(self.dag.eq(sel, v))
    }

    fn inside(&mut self, x: &Expr, items: &[InsideItem], sc: usize, env: &mut Env) -> R<N> {
        let (mut w, mut s) = self.size(x, sc)?;
        for it in items {
            let es: Vec<&Expr> = match it {
                InsideItem::Value(e) => vec![e],
                InsideItem::Range(a, b) => vec![a, b],
            };
            for e in es {
                let (ew, sg) = self.size(e, sc)?;
                w = w.max(ew);
                s = s && sg;
            }
        }
        let sel = self.eval(x, w, s, sc, env)?;
        let mut acc = self.dag.bit(false);
        for it in items {
            let hit = match it {
                InsideItem::Value(e) => self.matches(sel, w, s, e, true, sc, env)?,
                InsideItem::Range(a, b) => {
                    let lo = self.eval(a, w, s, sc, env)?;
                    let hi = self.eval(b, w, s, sc, env)?;
                    let (ge, le) = if s {
                        let lt = self.dag.slt(sel, lo);
                        let gt = self.dag.slt(hi, sel);
                        (self.dag.not(lt), self.dag.not(gt))
                    } else {
                        (self.dag.ule(lo, sel), self.dag.ule(sel, hi))
                    };
                    self.dag.and(ge, le)
                }
            };
            acc = self.dag.or(acc, hit);
        }
        Ok(acc)
    }

    /// Value of `e` in a context `w` bits wide of signedness `signed`.
    pub fn eval(&mut self, e: &Expr, w: u32, signed: bool, sc: usize, env: &mut Env) -> R<N> {
        let one_bit = |s: &mut Self, b: N| s.dag.zext(b, w);
        Ok(match e {
            Expr::Num(n) => self.num_node(n, w, signed),
            Expr::Str(st) => {
                let mut bits = Vec::new();
                for ch in st.bytes().rev() {
                    for i in 0..8 {
                        bits.push((ch >> i) & 1 == 1);
                    }
                }
                if bits.is_empty() {
                    bits = vec![false; 8];
                }
                let k = self.dag.konst(bits);
                self.dag.ext(k, w, false)
            }
            Expr::Id(_) | Expr::Member(..) | Expr::Index(..) | Expr::Range(..) | Expr::PlusRange(..) => match self.eval_ref(e, sc, env)? {
                Some(v) => {
                    let cut = if self.dag.w(v.n) > w { self.dag.slice(v.n, 0, w) } else { v.n };
                    self.dag.ext(cut, w, signed)
                }
                None => match e {
                    Expr::Id(path) => self.eval(&Expr::Call(path.clone(), vec![]), w, signed, sc, env)?,
                    _ => return self.err("cannot evaluate this expression"),
                },
            },
            Expr::Unary(op, a) => match *op {
                "+" => self.eval(a, w, signed, sc, env)?,
                "-" => {
                    let x = self.eval(a, w, signed, sc, env)?;
                    self.neg(x)
                }
                "~" => {
                    let x = self.eval(a, w, signed, sc, env)?;
                    self.dag.not(x)
                }
                "!" => {
                    let x = self.eval_self(a, sc, env)?;
                    let t = self.truth(x.n);
                    let r = self.dag.not(t);
                    one_bit(self, r)
                }
                "&" | "~&" | "|" | "~|" | "^" | "~^" | "^~" => {
                    let x = self.eval_self(a, sc, env)?;
                    let r = match *op {
                        "&" | "~&" => self.dag.red_and(x.n),
                        "|" | "~|" => self.dag.red_or(x.n),
                        _ => self.dag.red_xor(x.n),
                    };
                    let r = if op.starts_with('~') || *op == "^~" { self.dag.not(r) } else { r };
                    one_bit(self, r)
                }
                _ => return self.err(format!("operator '{op}' is not supported")),
            },
            Expr::Binary(op, a, b) => match *op {
                "+" | "-" | "*" | "/" | "%" | "&" | "|" | "^" | "^~" | "~^" => {
                    let x = self.eval(a, w, signed, sc, env)?;
                    let y = self.eval(b, w, signed, sc, env)?;
                    match *op {
                        "+" => self.dag.add(x, y),
                        "-" => self.dag.sub(x, y),
                        "*" => self.dag.mul(x, y),
                        "/" => {
                            if signed {
                                self.sdivrem(x, y, false)
                            } else {
                                self.dag.udiv(x, y)
                            }
                        }
                        "%" => {
                            if signed {
                                self.sdivrem(x, y, true)
                            } else {
                                self.dag.urem(x, y)
                            }
                        }
                        "&" => self.dag.and(x, y),
                        "|" => self.dag.or(x, y),
                        "^" => self.dag.xor(x, y),
                        _ => {
                            let r = self.dag.xor(x, y);
                            self.dag.not(r)
                        }
                    }
                }
                "**" => {
                    let x = self.eval(a, w, signed, sc, env)?;
                    let ev = self.eval_self(b, sc, env)?;
                    match self.const_of(ev.n, ev.ty.signed()) {
                        Some(k) if (0..=4096).contains(&k) => {
                            let mut acc = self.kint(1, w);
                            for _ in 0..k {
                                acc = self.dag.mul(acc, x);
                            }
                            acc
                        }
                        Some(_) => return self.err("exponent is negative or too large"),
                        None => {
                            if self.dag.const_u128(x) == Some(2) {
                                let one = self.kint(1, w);
                                self.dag.shl(one, ev.n)
                            } else {
                                return self.err("'**' needs a constant exponent or a base of 2");
                            }
                        }
                    }
                }
                "<<" | "<<<" | ">>" | ">>>" => {
                    let x = self.eval(a, w, signed, sc, env)?;
                    let amt = self.eval_self(b, sc, env)?;
                    match *op {
                        "<<" | "<<<" => self.dag.shl(x, amt.n),
                        ">>" => self.dag.lshr(x, amt.n),
                        _ => {
                            if signed {
                                self.ashr(x, amt.n)
                            } else {
                                self.dag.lshr(x, amt.n)
                            }
                        }
                    }
                }
                "==" | "!=" | "===" | "!==" | "==?" | "!=?" | "<" | "<=" | ">" | ">=" => {
                    let (xa, xb) = (self.size(a, sc)?, self.size(b, sc)?);
                    let (m, s) = (xa.0.max(xb.0), xa.1 && xb.1);
                    let x = self.eval(a, m, s, sc, env)?;
                    let r = match *op {
                        "==?" | "!=?" => {
                            let r = self.matches(x, m, s, b, true, sc, env)?;
                            if *op == "!=?" { self.dag.not(r) } else { r }
                        }
                        _ => {
                            let y = self.eval(b, m, s, sc, env)?;
                            match *op {
                                "==" | "===" => self.dag.eq(x, y),
                                "!=" | "!==" => self.dag.ne(x, y),
                                "<" => {
                                    if s { self.dag.slt(x, y) } else { self.dag.ult(x, y) }
                                }
                                ">" => {
                                    if s { self.dag.slt(y, x) } else { self.dag.ult(y, x) }
                                }
                                "<=" => {
                                    let g = if s { self.dag.slt(y, x) } else { self.dag.ult(y, x) };
                                    self.dag.not(g)
                                }
                                _ => {
                                    let l = if s { self.dag.slt(x, y) } else { self.dag.ult(x, y) };
                                    self.dag.not(l)
                                }
                            }
                        }
                    };
                    one_bit(self, r)
                }
                "&&" | "||" => {
                    let x = self.eval_self(a, sc, env)?;
                    let y = self.eval_self(b, sc, env)?;
                    let (p, q) = (self.truth(x.n), self.truth(y.n));
                    let r = if *op == "&&" { self.dag.and(p, q) } else { self.dag.or(p, q) };
                    one_bit(self, r)
                }
                _ => return self.err(format!("operator '{op}' is not supported")),
            },
            Expr::Cond(c, a, b) => {
                let cv = self.eval_self(c, sc, env)?;
                let c1 = self.truth(cv.n);
                if let Some(k) = self.dag.as_const(c1).map(|k| k[0]) {
                    return self.eval(if k { a } else { b }, w, signed, sc, env);
                }
                let x = self.eval(a, w, signed, sc, env)?;
                let y = self.eval(b, w, signed, sc, env)?;
                self.dag.mux(c1, x, y)
            }
            Expr::Concat(items) => {
                let mut parts = Vec::with_capacity(items.len());
                for i in items {
                    parts.push(self.eval_self(i, sc, env)?.n);
                }
                let c = self.dag.concat(parts);
                let cut = if self.dag.w(c) > w { self.dag.slice(c, 0, w) } else { c };
                self.dag.zext(cut, w)
            }
            Expr::Repl(n, items) => {
                let k = self.const_int(n, sc)?;
                let mut parts = Vec::with_capacity(items.len());
                for i in items {
                    parts.push(self.eval_self(i, sc, env)?.n);
                }
                let unit = self.dag.concat(parts);
                let c = self.dag.concat(vec![unit; k.max(0) as usize]);
                let cut = if self.dag.w(c) > w { self.dag.slice(c, 0, w) } else { c };
                self.dag.zext(cut, w)
            }
            Expr::Call(path, args) => {
                let v = self.call(path, args, sc, env)?;
                let Some(v) = v else { return self.err(format!("'{}' returns no value", path.join("::"))) };
                let cut = if self.dag.w(v.n) > w { self.dag.slice(v.n, 0, w) } else { v.n };
                self.dag.ext(cut, w, signed)
            }
            Expr::SysCall(name, args) => {
                let r: N = match name.as_str() {
                    "signed" | "unsigned" => {
                        let v = self.eval_self(&args[0], sc, env)?;
                        let cut = if self.dag.w(v.n) > w { self.dag.slice(v.n, 0, w) } else { v.n };
                        return Ok(self.dag.ext(cut, w, signed));
                    }
                    "clog2" => {
                        let v = self.const_int(&args[0], sc)?;
                        let r = if v <= 1 { 0 } else { 64 - ((v - 1) as u64).leading_zeros() as i64 };
                        self.kint(r, 32)
                    }
                    "bits" => {
                        let bw = match &args[0] {
                            Expr::Type(t) => self.resolve_type(t, sc)?.w(),
                            Expr::Id(p) if matches!(self.lookup_path(sc, p), Ok(Sym::Type(_))) => match self.lookup_path(sc, p)? {
                                Sym::Type(t) => t.w(),
                                _ => unreachable!(),
                            },
                            other => self.size(other, sc)?.0,
                        };
                        self.kint(bw as i64, 32)
                    }
                    "size" | "high" | "low" | "left" | "right" => {
                        let Some(t) = self.ref_ty(&args[0], sc)? else { return self.err(format!("${name} needs an array or vector")) };
                        let (lo, hi, asc) = match &*t {
                            Ty::Bits { w, lo, asc, .. } => (*lo, lo + *w as i64 - 1, *asc),
                            Ty::Array { lo, hi, asc, .. } => (*lo, *hi, *asc),
                            Ty::Struct { w, .. } => (0, *w as i64 - 1, false),
                        };
                        let v = match name.as_str() {
                            "size" => hi - lo + 1,
                            "high" => hi,
                            "low" => lo,
                            "left" => if asc { lo } else { hi },
                            _ => if asc { hi } else { lo },
                        };
                        self.kint(v, 32)
                    }
                    "countones" => {
                        let v = self.eval_self(&args[0], sc, env)?;
                        let vw = self.dag.w(v.n);
                        let mut terms: Vec<N> = (0..vw).map(|i| self.dag.slice(v.n, i, 1)).collect();
                        // Balanced adder tree over growing widths.
                        while terms.len() > 1 {
                            let mut next = Vec::new();
                            for c in terms.chunks(2) {
                                if c.len() == 2 {
                                    let cw = self.dag.w(c[0]).max(self.dag.w(c[1])) + 1;
                                    let (x, y) = (self.dag.zext(c[0], cw), self.dag.zext(c[1], cw));
                                    next.push(self.dag.add(x, y));
                                } else {
                                    next.push(c[0]);
                                }
                            }
                            terms = next;
                        }
                        let s = terms.pop().unwrap_or_else(|| self.dag.zero(1));
                        let cut = if self.dag.w(s) > 32 { self.dag.slice(s, 0, 32) } else { s };
                        self.dag.zext(cut, 32)
                    }
                    "onehot" | "onehot0" => {
                        let v = self.eval_self(&args[0], sc, env)?;
                        let vw = self.dag.w(v.n);
                        // any: some bit set; two: at least two bits set.
                        let mut any = self.dag.bit(false);
                        let mut two = self.dag.bit(false);
                        for i in 0..vw {
                            let b = self.dag.slice(v.n, i, 1);
                            let t = self.dag.and(any, b);
                            two = self.dag.or(two, t);
                            any = self.dag.or(any, b);
                        }
                        let not_two = self.dag.not(two);
                        let r = if name == "onehot" { self.dag.and(any, not_two) } else { not_two };
                        return Ok(one_bit(self, r));
                    }
                    "isunknown" => return Ok(self.dag.zero(w)),
                    _ => return self.err(format!("system function '${name}' is not synthesizable")),
                };
                let cut = if self.dag.w(r) > w { self.dag.slice(r, 0, w) } else { r };
                self.dag.ext(cut, w, signed)
            }
            Expr::Cast(to, v) => {
                let inner: N = match &**to {
                    CastTo::Type(t) => {
                        let ty = self.resolve_type(t, sc)?;
                        self.eval_to_type(v, &ty, sc, env)?
                    }
                    CastTo::Width(n) => {
                        let cw = self.const_int(n, sc)?;
                        if cw <= 0 {
                            return self.err("cast width must be positive");
                        }
                        self.eval_assign(v, cw as u32, sc, env)?
                    }
                    CastTo::Signed | CastTo::Unsigned => self.eval_self(v, sc, env)?.n,
                };
                let cut = if self.dag.w(inner) > w { self.dag.slice(inner, 0, w) } else { inner };
                self.dag.ext(cut, w, signed)
            }
            Expr::Inside(x, items) => {
                let r = self.inside(x, items, sc, env)?;
                one_bit(self, r)
            }
            Expr::Pattern(_) => return self.err("an assignment pattern needs a typed target"),
            Expr::Type(_) => return self.err("a type is not a value"),
        })
    }

    // ------------------------------------------------------------ functions

    fn declare_locals(&mut self, decls: &[Item], sc: usize, env: &mut Env) -> R<()> {
        for d in decls {
            match d {
                Item::Decl { ty, names, line } => {
                    self.line = *line;
                    let base = self.resolve_type(ty, sc)?;
                    for dn in names {
                        let t = self.with_unpacked(base.clone(), &dn.unpacked, sc)?;
                        let id = self.new_var(sc, &dn.name, t.clone());
                        let v = match &dn.init {
                            Some(e) => self.eval_to_type(e, &t, sc, env)?,
                            None => self.dag.zero(t.w()),
                        };
                        env.var.insert(id, v);
                    }
                }
                other => self.items(std::slice::from_ref(other), sc)?,
            }
        }
        Ok(())
    }

    /// Call a function; `None` for a void function or task.
    fn call(&mut self, path: &[String], args: &[Expr], sc: usize, env: &mut Env) -> R<Option<V>> {
        let (f, fsc) = match self.lookup_path(sc, path)? {
            Sym::Func(f, fsc) => (f, fsc),
            _ => return self.err(format!("'{}' is not a function", path.join("::"))),
        };
        if self.call_depth > 200 {
            return self.err(format!("function '{}' recurses too deeply", f.name));
        }
        if args.len() > f.args.len() {
            return self.err(format!("too many arguments for '{}'", f.name));
        }
        let line = self.line;
        let cs = self.new_scope(fsc, String::new());
        let mut outs: Vec<(u32, &Expr)> = Vec::new();
        for (i, a) in f.args.iter().enumerate() {
            let base = self.resolve_type(&a.ty, cs)?;
            let ty = self.with_unpacked(base, &a.decl.unpacked, cs)?;
            let v = match (a.dir, args.get(i)) {
                (Dir::In, Some(e)) => self.eval_to_type(e, &ty, sc, env)?,
                (Dir::In, None) => match &a.decl.init {
                    Some(d) => self.eval_to_type(d, &ty, cs, env)?,
                    None => return self.err(format!("argument '{}' of '{}' is missing", a.decl.name, f.name)),
                },
                _ => self.dag.zero(ty.w()),
            };
            let id = self.new_var(cs, &a.decl.name, ty);
            env.var.insert(id, v);
            if a.dir == Dir::Out {
                match args.get(i) {
                    Some(e) => outs.push((id, e)),
                    None => return self.err(format!("output argument '{}' of '{}' is missing", a.decl.name, f.name)),
                }
            }
        }
        let ret = match &f.ret {
            Some(t) => {
                let ty = self.resolve_type(t, cs)?;
                let id = self.new_var(cs, &f.name, ty.clone());
                let z = self.dag.zero(ty.w());
                env.var.insert(id, z);
                Some((id, ty))
            }
            None => None,
        };
        let saved = (env.ret, env.brk, env.cont, env.outer);
        env.outer = self.live(env);
        env.ret = None;
        env.brk = None;
        env.cont = None;
        self.call_depth += 1;
        self.ret_stack.push(ret.as_ref().map(|r| r.0));
        self.declare_locals(&f.decls, cs, env)?;
        for s in &f.body {
            self.exec(s, cs, env)?;
        }
        self.call_depth -= 1;
        self.ret_stack.pop();
        (env.ret, env.brk, env.cont, env.outer) = saved;
        self.line = line;
        for (id, target) in outs {
            let ty = self.vars[id as usize].ty.clone();
            let v = env.var[&id];
            self.store(target, V { n: v, ty }, false, sc, env)?;
        }
        Ok(ret.map(|(id, ty)| V { n: env.var[&id], ty }))
    }

    // ------------------------------------------------------------- lvalues

    fn lvalue(&mut self, e: &Expr, sc: usize, env: &mut Env) -> R<Lv> {
        match e {
            Expr::Id(path) => match self.lookup_path(sc, path)? {
                Sym::Sig(s) => Ok(Lv { root: Root::Sig(s), steps: vec![], ty: self.sigs[s as usize].ty.clone() }),
                Sym::Var(v) => Ok(Lv { root: Root::Var(v), steps: vec![], ty: self.vars[v as usize].ty.clone() }),
                _ => self.err(format!("'{}' cannot be assigned", path.join("::"))),
            },
            Expr::Member(b, f) => {
                let mut lv = self.lvalue(b, sc, env)?;
                let Ty::Struct { fields, .. } = &*lv.ty.clone() else { return self.err(format!("'.{f}' used on something that is not a struct")) };
                let Some((_, fty, off)) = fields.iter().find(|x| &x.0 == f) else { return self.err(format!("no member '{f}'")) };
                lv.steps.push(LStep::Fixed(*off, fty.w()));
                lv.ty = fty.clone();
                Ok(lv)
            }
            Expr::Index(b, i) => {
                let mut lv = self.lvalue(b, sc, env)?;
                let (ety, count) = lv.ty.elem();
                let iv = self.eval_self(i, sc, env)?;
                if let Some(k) = self.const_of(iv.n, iv.ty.signed()) {
                    let Some(off) = lv.ty.offset(k) else { return self.err(format!("index {k} is outside the declared range")) };
                    lv.steps.push(LStep::Fixed(off, ety.w()));
                } else {
                    let idx = self.position(&iv, &lv.ty);
                    lv.steps.push(LStep::Elem { idx, count, ew: ety.w() });
                }
                lv.ty = ety;
                Ok(lv)
            }
            Expr::Range(b, hi, lo) => {
                let mut lv = self.lvalue(b, sc, env)?;
                let (a, c) = (self.const_int(hi, sc)?, self.const_int(lo, sc)?);
                let ew = lv.ty.elem().0.w();
                let (Some(o1), Some(o2)) = (lv.ty.offset(a), lv.ty.offset(c)) else {
                    return self.err(format!("part-select [{a}:{c}] is outside the declared range"));
                };
                let w = (o1.max(o2) - o1.min(o2)) + ew;
                lv.steps.push(LStep::Fixed(o1.min(o2), w));
                lv.ty = Ty::bits(w, false);
                Ok(lv)
            }
            Expr::PlusRange(b, start, w, up) => {
                let mut lv = self.lvalue(b, sc, env)?;
                let n = self.const_int(w, sc)?;
                if n <= 0 {
                    return self.err("part-select width must be positive");
                }
                let n = n as u32;
                let ew = lv.ty.elem().0.w();
                let sv = self.eval_self(start, sc, env)?;
                if let Some(s) = self.const_of(sv.n, sv.ty.signed()) {
                    let (a, c) = if *up { (s, s + n as i64 - 1) } else { (s, s - n as i64 + 1) };
                    let (Some(o1), Some(o2)) = (lv.ty.offset(a), lv.ty.offset(c)) else {
                        return self.err("indexed part-select is outside the declared range");
                    };
                    lv.steps.push(LStep::Fixed(o1.min(o2), n * ew));
                } else {
                    if ew != 1 {
                        return self.err("a variable indexed part-select of an array is not supported");
                    }
                    let asc = matches!(&*lv.ty, Ty::Bits { asc: true, .. });
                    let p = self.position(&sv, &lv.ty);
                    let off = if *up != asc {
                        p
                    } else {
                        let pw = self.dag.w(p).max(34) + 1;
                        let px = self.dag.ext(p, pw, false);
                        let k = self.kint(n as i64 - 1, pw);
                        self.dag.sub(px, k)
                    };
                    lv.steps.push(LStep::Slide { off, w: n });
                }
                lv.ty = Ty::bits(n * ew, false);
                Ok(lv)
            }
            _ => self.err("this expression cannot be assigned"),
        }
    }

    fn splice(&mut self, cur: N, lo: u32, sub: N) -> N {
        let (cw, sw) = (self.dag.w(cur), self.dag.w(sub));
        let mut parts = Vec::new();
        if lo + sw < cw {
            parts.push(self.dag.slice(cur, lo + sw, cw - lo - sw));
        }
        parts.push(sub);
        if lo > 0 {
            parts.push(self.dag.slice(cur, 0, lo));
        }
        self.dag.concat(parts)
    }

    /// New value of `cur` after writing `val` through `steps`.
    fn write(&mut self, cur: N, steps: &[LStep], val: N) -> N {
        let Some(step) = steps.first() else { return val };
        match step {
            LStep::Fixed(lo, w) => {
                let sub = self.dag.slice(cur, *lo, *w);
                let ns = self.write(sub, &steps[1..], val);
                self.splice(cur, *lo, ns)
            }
            LStep::Elem { idx, count, ew } => {
                let iw = self.dag.w(*idx);
                let mut parts = Vec::with_capacity(*count as usize);
                for k in (0..*count).rev() {
                    let sub = self.dag.slice(cur, k * ew, *ew);
                    if bits_for_u(k as u128) > iw {
                        parts.push(sub);
                        continue;
                    }
                    let ns = self.write(sub, &steps[1..], val);
                    let kc = self.dag.kval(k as u128, iw);
                    let hit = self.dag.eq(*idx, kc);
                    parts.push(self.dag.mux(hit, ns, sub));
                }
                self.dag.concat(parts)
            }
            LStep::Slide { off, w } => {
                let cw = self.dag.w(cur);
                let ow = self.dag.w(*off);
                let mut acc = cur;
                for p in 0..=(cw.saturating_sub(*w)) {
                    if bits_for_u(p as u128) > ow {
                        break;
                    }
                    let cand = self.splice(cur, p, val);
                    let pc = self.dag.kval(p as u128, ow);
                    let hit = self.dag.eq(*off, pc);
                    acc = self.dag.mux(hit, cand, acc);
                }
                acc
            }
        }
    }

    /// Bit range of the root that an assignment through `steps` may change.
    fn touched(steps: &[LStep], total: u32) -> (u32, u32) {
        let (mut lo, mut w) = (0, total);
        for s in steps {
            match s {
                LStep::Fixed(l, sw) => {
                    lo += l;
                    w = *sw;
                }
                _ => break,
            }
        }
        (lo, w)
    }

    /// Assign `val` (already of the target's type) to `target`.
    fn store(&mut self, target: &Expr, val: V, nonblocking: bool, sc: usize, env: &mut Env) -> R<()> {
        let lv = self.lvalue(target, sc, env)?;
        let w = lv.ty.w();
        let v = if self.dag.w(val.n) == w {
            val.n
        } else {
            let cut = if self.dag.w(val.n) > w { self.dag.slice(val.n, 0, w) } else { val.n };
            self.dag.ext(cut, w, val.ty.signed())
        };
        self.store_lv(&lv, v, nonblocking, env)
    }

    fn store_lv(&mut self, lv: &Lv, v: N, nonblocking: bool, env: &mut Env) -> R<()> {
        let live = self.live(env);
        match lv.root {
            Root::Var(id) => {
                let cur = match env.var.get(&id) {
                    Some(&c) => c,
                    None => self.dag.zero(self.vars[id as usize].ty.w()),
                };
                let mut new = self.write(cur, &lv.steps, v);
                if let Some(l) = live {
                    new = self.dag.mux(l, new, cur);
                }
                env.var.insert(id, new);
            }
            Root::Sig(id) => {
                if env.mode == Mode::Const {
                    let name = self.sigs[id as usize].name.clone();
                    return self.err(format!("'{name}' is assigned outside a process"));
                }
                let total = self.sigs[id as usize].ty.w();
                let base = if env.mode == Mode::Comb { self.dag.hold(id, total) } else { self.dag.read(id, total) };
                let map = if nonblocking { &mut env.nba } else { &mut env.sig };
                let cur = map.get(&id).copied().unwrap_or(base);
                let mut new = self.write(cur, &lv.steps, v);
                if let Some(l) = live {
                    new = self.dag.mux(l, new, cur);
                }
                let map = if nonblocking { &mut env.nba } else { &mut env.sig };
                map.insert(id, new);
                let (lo, w) = Self::touched(&lv.steps, total);
                let mask = if nonblocking { &mut self.ps.nba } else { &mut self.ps.blk };
                let m = mask.entry(id).or_insert_with(|| vec![false; total as usize]);
                for b in lo..lo + w {
                    m[b as usize] = true;
                }
            }
        }
        Ok(())
    }

    fn assign(&mut self, lhs: &Expr, rhs: &Expr, nonblocking: bool, sc: usize, env: &mut Env) -> R<()> {
        if let Expr::Concat(parts) = lhs {
            let mut lvs = Vec::with_capacity(parts.len());
            for p in parts {
                lvs.push(self.lvalue(p, sc, env)?);
            }
            let total: u32 = lvs.iter().map(|l| l.ty.w()).sum();
            let v = self.eval_assign(rhs, total, sc, env)?;
            let mut hi = total;
            for lv in &lvs {
                let w = lv.ty.w();
                hi -= w;
                let part = self.dag.slice(v, hi, w);
                self.store_lv(lv, part, nonblocking, env)?;
            }
            return Ok(());
        }
        let lv = self.lvalue(lhs, sc, env)?;
        let v = self.eval_to_type(rhs, &lv.ty.clone(), sc, env)?;
        self.store_lv(&lv, v, nonblocking, env)
    }

    /// Targets of a continuous assignment, MSB first: (signal, low bit,
    /// width, type when the target is a single whole object).
    pub fn static_targets(&mut self, lhs: &Expr, sc: usize, env: &mut Env) -> R<Vec<(u32, u32, u32, Option<Rc<Ty>>)>> {
        let parts: Vec<&Expr> = match lhs {
            Expr::Concat(p) => p.iter().collect(),
            other => vec![other],
        };
        let mut out = Vec::new();
        for p in parts {
            let lv = self.lvalue(p, sc, env)?;
            let Root::Sig(sig) = lv.root else { return self.err("a continuous assignment needs a signal on its left") };
            let total = self.sigs[sig as usize].ty.w();
            if lv.steps.iter().any(|s| !matches!(s, LStep::Fixed(..))) {
                return self.err("a continuous assignment to a variable index is not supported");
            }
            let (lo, w) = Self::touched(&lv.steps, total);
            out.push((sig, lo, w, Some(lv.ty.clone())));
        }
        Ok(out)
    }

    // ---------------------------------------------------------- statements

    fn base_of(&mut self, id: u32, mode: Mode) -> N {
        let w = self.sigs[id as usize].ty.w();
        if mode == Mode::Comb { self.dag.hold(id, w) } else { self.dag.read(id, w) }
    }

    /// `env` becomes `c ? a : b`.
    fn merge(&mut self, c: N, a: Env, b: Env, env: &mut Env) {
        let mode = a.mode;
        let mut out = Env { mode, outer: a.outer, ..Default::default() };
        for which in 0..3 {
            let (ma, mb) = match which {
                0 => (&a.sig, &b.sig),
                1 => (&a.nba, &b.nba),
                _ => (&a.var, &b.var),
            };
            let mut keys: Vec<u32> = ma.keys().chain(mb.keys()).copied().collect();
            keys.sort_unstable();
            keys.dedup();
            for k in keys {
                let dflt = |s: &mut Self| if which == 2 { s.dag.zero(s.vars[k as usize].ty.w()) } else { s.base_of(k, mode) };
                let x = match ma.get(&k) {
                    Some(&x) => x,
                    None => dflt(self),
                };
                let y = match mb.get(&k) {
                    Some(&y) => y,
                    None => dflt(self),
                };
                let v = if x == y { x } else { self.dag.mux(c, x, y) };
                match which {
                    0 => out.sig.insert(k, v),
                    1 => out.nba.insert(k, v),
                    _ => out.var.insert(k, v),
                };
            }
        }
        let f = self.dag.bit(false);
        let mut flag = |s: &mut Self, x: Option<N>, y: Option<N>| -> Option<N> {
            if x.is_none() && y.is_none() {
                return None;
            }
            Some(s.dag.mux(c, x.unwrap_or(f), y.unwrap_or(f)))
        };
        out.ret = flag(self, a.ret, b.ret);
        out.brk = flag(self, a.brk, b.brk);
        out.cont = flag(self, a.cont, b.cont);
        *env = out;
    }

    /// Run the arms `lo..hi` of a case as a balanced priority tree. Returns
    /// the condition "one of these arms matched" and leaves the merged
    /// result of those arms in the returned environment.
    fn case_tree(&mut self, conds: &[N], arms: &[&Stmt], sc: usize, base: &Env) -> R<(N, Env)> {
        if conds.len() == 1 {
            let mut e = base.clone();
            self.exec(arms[0], sc, &mut e)?;
            return Ok((conds[0], e));
        }
        let mid = conds.len() / 2;
        let (ca, ea) = self.case_tree(&conds[..mid], &arms[..mid], sc, base)?;
        let (cb, eb) = self.case_tree(&conds[mid..], &arms[mid..], sc, base)?;
        let mut out = Env::default();
        self.merge(ca, ea, eb, &mut out);
        Ok((self.dag.or(ca, cb), out))
    }

    pub fn exec(&mut self, s: &Stmt, sc: usize, env: &mut Env) -> R<()> {
        if self.is_dead(env) {
            return Ok(());
        }
        match s {
            Stmt::Null => Ok(()),
            Stmt::Block { decls, stmts } => {
                let bs = if decls.is_empty() { sc } else { self.new_scope(sc, String::new()) };
                self.declare_locals(decls, bs, env)?;
                for st in stmts {
                    self.exec(st, bs, env)?;
                }
                Ok(())
            }
            Stmt::Assign { lhs, rhs, nonblocking, line } => {
                self.line = *line;
                self.assign(lhs, rhs, *nonblocking, sc, env)
            }
            Stmt::If { cond, then, els, line } => {
                self.line = *line;
                let cv = self.eval_self(cond, sc, env)?;
                let c = self.truth(cv.n);
                if let Some(k) = self.dag.as_const(c).map(|k| k[0]) {
                    if k {
                        return self.exec(then, sc, env);
                    }
                    if let Some(e) = els {
                        return self.exec(e, sc, env);
                    }
                    return Ok(());
                }
                let mut a = env.clone();
                self.exec(then, sc, &mut a)?;
                let mut b = env.clone();
                if let Some(e) = els {
                    self.exec(e, sc, &mut b)?;
                }
                self.merge(c, a, b, env);
                Ok(())
            }
            Stmt::Case { kind, expr, arms, line } => {
                self.line = *line;
                let (mut w, mut sg) = self.size(expr, sc)?;
                for (items, _) in arms {
                    for it in items {
                        let es: Vec<&Expr> = match it {
                            InsideItem::Value(e) => vec![e],
                            InsideItem::Range(a, b) => vec![a, b],
                        };
                        for e in es {
                            let (ew, es) = self.size(e, sc)?;
                            w = w.max(ew);
                            sg = sg && es;
                        }
                    }
                }
                let sel = self.eval(expr, w, sg, sc, env)?;
                let wild = !matches!(kind, CaseKind::Case);
                let mut conds = Vec::new();
                let mut bodies: Vec<&Stmt> = Vec::new();
                let mut default: Option<&Stmt> = None;
                for (items, body) in arms {
                    if items.is_empty() {
                        default = Some(body);
                        continue;
                    }
                    let mut c = self.dag.bit(false);
                    for it in items {
                        let hit = match it {
                            InsideItem::Value(e) => self.matches(sel, w, sg, e, wild, sc, env)?,
                            InsideItem::Range(a, b) => {
                                let lo = self.eval(a, w, sg, sc, env)?;
                                let hi = self.eval(b, w, sg, sc, env)?;
                                let ge = self.dag.ule(lo, sel);
                                let le = self.dag.ule(sel, hi);
                                self.dag.and(ge, le)
                            }
                        };
                        c = self.dag.or(c, hit);
                    }
                    // Constant conditions settle at elaboration.
                    match self.dag.as_const(c).map(|k| k[0]) {
                        Some(false) => continue,
                        Some(true) => {
                            conds.push(c);
                            bodies.push(body);
                            default = None;
                            break;
                        }
                        None => {
                            conds.push(c);
                            bodies.push(body);
                        }
                    }
                }
                if conds.is_empty() {
                    if let Some(d) = default {
                        self.exec(d, sc, env)?;
                    }
                    return Ok(());
                }
                if conds.len() == 1 && self.dag.as_const(conds[0]).is_some() {
                    return self.exec(bodies[0], sc, env);
                }
                // A trailing always-true arm is the default.
                let mut else_body = default;
                if let Some(&last) = conds.last() {
                    if self.dag.as_const(last).map(|k| k[0]) == Some(true) {
                        conds.pop();
                        else_body = bodies.pop();
                    }
                }
                let base = env.clone();
                let (any, hit) = self.case_tree(&conds, &bodies, sc, &base)?;
                let mut miss = base;
                if let Some(d) = else_body {
                    self.exec(d, sc, &mut miss)?;
                }
                self.merge(any, hit, miss, env);
                Ok(())
            }
            Stmt::For { init, decls, cond, step, body, line } => {
                self.line = *line;
                let ls = self.new_scope(sc, String::new());
                self.declare_locals(decls, ls, env)?;
                for st in init {
                    self.exec(st, ls, env)?;
                }
                self.run_loop(Some(cond), step, body, None, ls, env, *line)
            }
            Stmt::While { cond, body, line } => self.run_loop(Some(cond), &[], body, None, sc, env, *line),
            Stmt::DoWhile { cond, body, line } => {
                self.exec(body, sc, env)?;
                env.cont = None;
                self.run_loop(Some(cond), &[], body, None, sc, env, *line)
            }
            Stmt::Repeat { count, body, line } => {
                self.line = *line;
                let n = self.const_int(count, sc)?;
                self.run_loop(None, &[], body, Some(n), sc, env, *line)
            }
            Stmt::Return(e, line) => {
                self.line = *line;
                if let Some(e) = e {
                    let Some(Some(id)) = self.ret_stack.last().copied() else {
                        return self.err("'return' with a value outside a function that returns one");
                    };
                    let ty = self.vars[id as usize].ty.clone();
                    let v = self.eval_to_type(e, &ty, sc, env)?;
                    self.store_lv(&Lv { root: Root::Var(id), steps: vec![], ty }, v, false, env)?;
                }
                let l = self.live(env).unwrap_or_else(|| self.dag.bit(true));
                env.ret = self.or_opt(env.ret, Some(l));
                Ok(())
            }
            Stmt::Break(line) => {
                self.line = *line;
                let l = self.live(env).unwrap_or_else(|| self.dag.bit(true));
                env.brk = self.or_opt(env.brk, Some(l));
                Ok(())
            }
            Stmt::Continue(line) => {
                self.line = *line;
                let l = self.live(env).unwrap_or_else(|| self.dag.bit(true));
                env.cont = self.or_opt(env.cont, Some(l));
                Ok(())
            }
            Stmt::Call(e, line) => {
                self.line = *line;
                let Expr::Call(path, args) = e else { return self.err("expected a call") };
                self.call(path, args, sc, env)?;
                Ok(())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_loop(&mut self, cond: Option<&Expr>, step: &[Stmt], body: &Stmt, count: Option<i64>, sc: usize, env: &mut Env, line: u32) -> R<()> {
        let mut iters: i64 = 0;
        loop {
            self.line = line;
            if let Some(n) = count {
                if iters >= n {
                    break;
                }
            }
            if let Some(c) = cond {
                let cv = self.eval_self(c, sc, env)?;
                let t = self.truth(cv.n);
                match self.dag.as_const(t).map(|k| k[0]) {
                    Some(true) => {}
                    Some(false) => break,
                    None => return self.err("the loop condition is not constant, so the loop cannot be unrolled"),
                }
            }
            self.exec(body, sc, env)?;
            env.cont = None;
            if self.is_dead(env) {
                break;
            }
            for st in step {
                self.exec(st, sc, env)?;
            }
            iters += 1;
            if iters > 1 << 16 {
                self.line = line;
                return self.err("loop runs more than 65536 times");
            }
        }
        env.brk = None;
        Ok(())
    }

    // ------------------------------------------------------------ processes

    fn idents(e: &Expr, out: &mut Vec<String>) {
        match e {
            Expr::Id(p) => out.push(p.join("::")),
            Expr::Member(a, _) => Self::idents(a, out),
            Expr::Index(a, b) => {
                Self::idents(a, out);
                Self::idents(b, out);
            }
            Expr::Range(a, ..) | Expr::PlusRange(a, ..) => Self::idents(a, out),
            Expr::Unary(_, a) => Self::idents(a, out),
            Expr::Binary(_, a, b) => {
                Self::idents(a, out);
                Self::idents(b, out);
            }
            Expr::Cond(a, b, c) => {
                Self::idents(a, out);
                Self::idents(b, out);
                Self::idents(c, out);
            }
            Expr::Concat(v) => v.iter().for_each(|x| Self::idents(x, out)),
            Expr::Cast(_, a) => Self::idents(a, out),
            Expr::SysCall(_, v) | Expr::Call(_, v) => v.iter().for_each(|x| Self::idents(x, out)),
            _ => {}
        }
    }

    pub fn always(&mut self, kind: AlwaysKind, sens: &Sens, body: &Stmt, sc: usize) -> R<()> {
        let line = self.line;
        let edges: &[(bool, Expr)] = match sens {
            Sens::Edges(e) => e,
            _ => &[],
        };
        if kind == AlwaysKind::Latch {
            return self.err("latches (always_latch) are not supported");
        }
        if kind == AlwaysKind::Ff && edges.is_empty() {
            return self.err("always_ff needs a clock edge");
        }
        self.ps = ProcState::default();
        if edges.is_empty() {
            // Combinational.
            let mut env = Env { mode: Mode::Comb, ..Default::default() };
            self.exec(body, sc, &mut env)?;
            self.line = line;
            let mut ids: Vec<u32> = env.sig.keys().chain(env.nba.keys()).copied().collect();
            ids.sort_unstable();
            ids.dedup();
            // A bit this process never assigns is whatever drives it elsewhere.
            let masks: HashMap<u32, Vec<bool>> = ids
                .iter()
                .map(|id| {
                    let w = self.sigs[*id as usize].ty.w() as usize;
                    let mut m = vec![false; w];
                    for src in [self.ps.blk.get(id), self.ps.nba.get(id)].into_iter().flatten() {
                        for (i, b) in src.iter().enumerate() {
                            m[i] |= *b;
                        }
                    }
                    (*id, m)
                })
                .collect();
            let mut memo: HashMap<N, N> = HashMap::new();
            let mut fix = |s: &mut super::dag::Dag, n: N| -> Option<N> {
                let Op::Hold(sig) = *s.op(n) else { return None };
                let Some(m) = masks.get(&sig) else { return Some(s.read(sig, s.w(n))) };
                if m.iter().all(|b| *b) {
                    return None;
                }
                let w = m.len() as u32;
                let rd = s.read(sig, w);
                // Runs of assigned / unassigned bits, MSB first.
                let mut parts = Vec::new();
                let mut i = w;
                while i > 0 {
                    let v = m[i as usize - 1];
                    let mut j = i;
                    while j > 0 && m[j as usize - 1] == v {
                        j -= 1;
                    }
                    parts.push(s.slice(if v { n } else { rd }, j, i - j));
                    i = j;
                }
                Some(s.concat(parts))
            };
            for id in ids {
                if env.sig.contains_key(&id) && env.nba.contains_key(&id) {
                    let name = self.sigs[id as usize].name.clone();
                    return self.err(format!("'{name}' is assigned with both '=' and '<=' in one block"));
                }
                let raw = env.sig.get(&id).or(env.nba.get(&id)).copied().unwrap();
                let v = self.dag.subst(raw, &mut fix, &mut memo);
                let mask = &masks[&id];
                let taint = self.dag.tainted_bits(v, &|op| matches!(op, Op::Hold(_)), &mut HashMap::new());
                if let Some(b) = (0..mask.len()).find(|&b| mask[b] && taint[b]) {
                    let name = super::lower::bit_name(&self.sigs[id as usize].name, mask.len() as u32, b as u32);
                    return self.err(format!("'{name}' is not assigned on every path through this combinational block (that would be a latch)"));
                }
                for (b, m) in mask.iter().enumerate() {
                    if *m {
                        self.drive(id, b as u32, Drv::Comb(v, b as u32))?;
                    }
                }
            }
            return Ok(());
        }
        // Clocked. With several edges, the clock is the one the leading
        // `if` does not test; the others are asynchronous controls.
        let clk_ix = if edges.len() == 1 {
            0
        } else {
            let mut first = body;
            while let Stmt::Block { stmts, decls } = first {
                if decls.is_empty() && stmts.len() == 1 {
                    first = &stmts[0];
                } else {
                    break;
                }
            }
            let Stmt::If { cond, .. } = first else {
                return self.err("a block with asynchronous controls must start with 'if' on them");
            };
            let mut used = Vec::new();
            Self::idents(cond, &mut used);
            let free: Vec<usize> = (0..edges.len())
                .filter(|&i| {
                    let mut names = Vec::new();
                    Self::idents(&edges[i].1, &mut names);
                    !names.iter().any(|n| used.contains(n))
                })
                .collect();
            if free.len() != 1 {
                return self.err("cannot tell the clock from the asynchronous controls in this sensitivity list");
            }
            let note = "asynchronous set/reset is built as synchronous (the reset takes effect at the next clock edge)";
            if !self.warnings.iter().any(|w| w.ends_with(note)) {
                self.warnings.push(format!("line {line}: {note}"));
            }
            free[0]
        };
        let (pos, clk_e) = &edges[clk_ix];
        let cv = self.eval_self(clk_e, sc, &mut Env::default())?;
        let clk = self.dag.slice(cv.n, 0, 1);
        let mut env = Env { mode: Mode::Seq, ..Default::default() };
        self.exec(body, sc, &mut env)?;
        self.line = line;
        let mut ids: Vec<u32> = env.sig.keys().chain(env.nba.keys()).copied().collect();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            if env.sig.contains_key(&id) && env.nba.contains_key(&id) {
                let name = self.sigs[id as usize].name.clone();
                return self.err(format!("'{name}' is assigned with both '=' and '<=' in one block"));
            }
            let d = env.sig.get(&id).or(env.nba.get(&id)).copied().unwrap();
            let mask = self.ps.blk.get(&id).or(self.ps.nba.get(&id)).cloned().unwrap();
            let reg = self.regs.len() as u32;
            for (b, m) in mask.iter().enumerate() {
                if *m {
                    self.drive(id, b as u32, Drv::Reg(reg))?;
                }
            }
            self.regs.push(Reg { sig: id, d, clk, negedge: !*pos, mask });
        }
        Ok(())
    }
}
