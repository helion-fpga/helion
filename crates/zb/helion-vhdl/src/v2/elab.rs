//! Elaboration: types, scopes, declarations, hierarchy and nets.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use super::ast::*;
use super::dag::*;

pub type R<X> = Result<X, String>;

pub const INT_MIN: i64 = -(1 << 31);
pub const INT_MAX: i64 = (1 << 31) - 1;

#[derive(Clone, Debug, PartialEq)]
pub enum ArrKind {
    Slv,
    Unsigned,
    Signed,
    BitVec,
    User(String),
}

#[derive(Debug)]
pub struct EnumDef {
    pub name: String,
    /// Identifier literals lowercased; character literals as `'c'`.
    pub lits: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IdxRange {
    pub left: i64,
    pub right: i64,
    pub down: bool,
}

impl IdxRange {
    pub fn len(&self) -> u32 {
        let n = if self.down {
            self.left - self.right + 1
        } else {
            self.right - self.left + 1
        };
        n.max(0) as u32
    }
    pub fn lo(&self) -> i64 {
        self.left.min(self.right)
    }
    pub fn hi(&self) -> i64 {
        self.left.max(self.right)
    }
    pub fn contains(&self, i: i64) -> bool {
        self.len() > 0 && i >= self.lo() && i <= self.hi()
    }
    /// Position counted from the rightmost element (the LSB end).
    pub fn pos(&self, i: i64) -> Option<u32> {
        if !self.contains(i) {
            return None;
        }
        Some(if self.down {
            (i - self.right) as u32
        } else {
            (self.right - i) as u32
        })
    }
    /// Indices from left to right.
    pub fn iter(&self) -> Vec<i64> {
        let n = self.len() as i64;
        (0..n)
            .map(|k| {
                if self.down {
                    self.left - k
                } else {
                    self.left + k
                }
            })
            .collect()
    }
    pub fn canonical(n: u32) -> IdxRange {
        IdxRange {
            left: n as i64 - 1,
            right: 0,
            down: true,
        }
    }
}

#[derive(Debug)]
pub struct ArrTy {
    pub kind: ArrKind,
    pub elem: Ty,
    pub idx: Option<IdxRange>,
    /// Index subtype (integer range or an enumeration).
    pub idx_ty: Ty,
}

#[derive(Debug)]
pub struct RecTy {
    pub name: String,
    pub fields: Vec<(String, Ty)>,
}

#[derive(Clone, Debug)]
pub enum Ty {
    Logic,
    Bit,
    Bool,
    Char,
    Int { lo: i64, hi: i64 },
    Real,
    Time,
    Str,
    Enum(Rc<EnumDef>),
    Arr(Rc<ArrTy>),
    Rec(Rc<RecTy>),
}

pub fn bits_signed(v: i64) -> u32 {
    // bits needed for v in two's complement
    if v >= 0 {
        bits_for_u(v as u128) + 1
    } else {
        bits_for_u((-(v + 1)) as u128) + 1
    }
}

pub fn int_w(lo: i64, hi: i64) -> u32 {
    if lo >= 0 {
        bits_for_u(hi as u128).max(1)
    } else {
        bits_signed(lo).max(bits_signed(hi))
    }
}

pub fn clog2(n: u64) -> u32 {
    if n <= 1 {
        0
    } else {
        64 - (n - 1).leading_zeros()
    }
}

impl Ty {
    pub fn width(&self) -> R<u32> {
        Ok(match self {
            Ty::Logic | Ty::Bit | Ty::Bool => 1,
            Ty::Int { lo, hi } => int_w(*lo, *hi),
            Ty::Enum(e) => clog2(e.lits.len() as u64).max(1),
            Ty::Arr(a) => {
                let Some(idx) = a.idx else {
                    return Err(format!(
                        "unconstrained array type {} has no width here",
                        self.name()
                    ));
                };
                idx.len() * a.elem.width()?
            }
            Ty::Rec(r) => {
                let mut w = 0;
                for (_, t) in &r.fields {
                    w += t.width()?;
                }
                w
            }
            Ty::Real | Ty::Time | Ty::Str | Ty::Char => {
                return Err(format!("type {} is not synthesizable", self.name()));
            }
        })
    }
    pub fn name(&self) -> String {
        match self {
            Ty::Logic => "std_logic".into(),
            Ty::Bit => "bit".into(),
            Ty::Bool => "boolean".into(),
            Ty::Char => "character".into(),
            Ty::Int { lo, hi } => format!("integer range {lo} to {hi}"),
            Ty::Real => "real".into(),
            Ty::Time => "time".into(),
            Ty::Str => "string".into(),
            Ty::Enum(e) => e.name.clone(),
            Ty::Arr(a) => {
                let k = match &a.kind {
                    ArrKind::Slv => "std_logic_vector".to_string(),
                    ArrKind::Unsigned => "unsigned".into(),
                    ArrKind::Signed => "signed".into(),
                    ArrKind::BitVec => "bit_vector".into(),
                    ArrKind::User(n) => n.clone(),
                };
                match a.idx {
                    Some(r) => format!(
                        "{k}({} {} {})",
                        r.left,
                        if r.down { "downto" } else { "to" },
                        r.right
                    ),
                    None => k,
                }
            }
            Ty::Rec(r) => r.name.clone(),
        }
    }
    pub fn arr(&self) -> Option<&Rc<ArrTy>> {
        match self {
            Ty::Arr(a) => Some(a),
            _ => None,
        }
    }
    pub fn is_bit(&self) -> bool {
        matches!(self, Ty::Logic | Ty::Bit)
    }
    /// One-dimensional array of std_logic/bit.
    pub fn is_vec(&self) -> bool {
        matches!(self, Ty::Arr(a) if a.elem.is_bit())
    }
    pub fn is_int(&self) -> bool {
        matches!(self, Ty::Int { .. })
    }
    pub fn with_idx(&self, idx: IdxRange) -> Ty {
        match self {
            Ty::Arr(a) => Ty::Arr(Rc::new(ArrTy {
                kind: a.kind.clone(),
                elem: a.elem.clone(),
                idx: Some(idx),
                idx_ty: a.idx_ty.clone(),
            })),
            t => t.clone(),
        }
    }
    pub fn same_base(&self, o: &Ty) -> bool {
        match (self, o) {
            (Ty::Logic, Ty::Logic)
            | (Ty::Bit, Ty::Bit)
            | (Ty::Bool, Ty::Bool)
            | (Ty::Real, Ty::Real)
            | (Ty::Str, Ty::Str)
            | (Ty::Char, Ty::Char)
            | (Ty::Time, Ty::Time) => true,
            (Ty::Int { .. }, Ty::Int { .. }) => true,
            (Ty::Enum(a), Ty::Enum(b)) => Rc::ptr_eq(a, b) || a.name == b.name,
            (Ty::Arr(a), Ty::Arr(b)) => {
                a.kind == b.kind
                    && (a.kind != ArrKind::User(String::new()) || a.elem.same_base(&b.elem))
            }
            (Ty::Rec(a), Ty::Rec(b)) => Rc::ptr_eq(a, b) || a.name == b.name,
            _ => false,
        }
    }
    pub fn default_bits(&self) -> R<Vec<bool>> {
        Ok(match self {
            Ty::Int { lo, hi } => {
                let w = int_w(*lo, *hi);
                from_u128(*lo as i128 as u128, w)
            }
            Ty::Arr(a) => {
                let n = a.idx.map(|r| r.len()).unwrap_or(0);
                let e = a.elem.default_bits()?;
                let mut v = Vec::new();
                for _ in 0..n {
                    v.extend(e.iter().copied());
                }
                v
            }
            Ty::Rec(r) => {
                let mut v = Vec::new();
                for (_, t) in r.fields.iter().rev() {
                    v.extend(t.default_bits()?);
                }
                v
            }
            t => vec![false; t.width()? as usize],
        })
    }
}

#[derive(Clone, Debug)]
pub enum Val {
    B { ty: Ty, n: N },
    R(f64),
    S(String),
    T,
}

impl Val {
    pub fn ty(&self) -> Ty {
        match self {
            Val::B { ty, .. } => ty.clone(),
            Val::R(_) => Ty::Real,
            Val::S(_) => Ty::Str,
            Val::T => Ty::Time,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SigRef {
    pub net: u32,
    pub off: u32,
    pub ty: Ty,
}

pub struct SubDef {
    pub sp: Subprog,
    pub depth: usize,
}

#[derive(Clone)]
pub enum Sym {
    Sig(SigRef),
    Var(u32, Ty),
    Val(Val),
    Ty(Ty),
    Lits(Vec<(Ty, u32)>),
    Subs(Vec<Rc<SubDef>>),
    Comp(Rc<(Vec<IfDecl>, Vec<IfDecl>)>),
    Pkg,
    Unsupported(String),
}

#[derive(Clone, Debug)]
pub enum DrvSrc {
    Comb(N),
    Reg(u32),
}

#[derive(Clone, Debug)]
pub struct Drv {
    pub lo: u32,
    pub w: u32,
    pub src: DrvSrc,
    pub line: Line,
}

#[derive(Clone, Debug)]
pub struct Net {
    pub name: String,
    pub w: u32,
    pub init: Vec<bool>,
    pub drivers: Vec<Drv>,
}

#[derive(Clone, Debug)]
pub struct Arst {
    pub sig: N,
    pub active_high: bool,
    pub val: N,
}

#[derive(Clone, Debug)]
pub struct Reg {
    pub name: String,
    pub w: u32,
    pub clk: N,
    pub neg: bool,
    pub d: N,
    pub init: Vec<bool>,
    pub arst: Option<Arst>,
}

#[derive(Clone, Debug)]
pub struct TopPort {
    pub name: String,
    pub dir: Mode,
    pub net: u32,
    /// SV declared range (None for scalars).
    pub range: Option<(i64, i64)>,
}

/// Execution context of a process or subprogram call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PxKind {
    Comb,
    Clocked,
    Static,
}

#[derive(Clone, Debug)]
pub struct St {
    pub vars: Vec<N>,
    pub pend: BTreeMap<u32, N>,
    pub ret: N,
    pub retv: Option<N>,
    pub brk: N,
    pub cont: N,
}

pub struct Px {
    pub kind: PxKind,
    pub st: St,
    pub var_tys: Vec<Ty>,
    pub var_names: Vec<String>,
    /// Register net of each process variable (clocked processes).
    pub var_nets: Vec<Option<u32>>,
    pub driven: HashMap<u32, Vec<bool>>,
    pub loops: Vec<Option<String>>,
    pub line: Line,
    pub label: String,
}

pub struct Elab {
    pub dag: Dag,
    pub nets: Vec<Net>,
    pub regs: Vec<Reg>,
    pub global: HashMap<String, Sym>,
    pub scopes: Vec<HashMap<String, Sym>>,
    pub entities: HashMap<String, (Rc<Entity>, Rc<Vec<String>>)>,
    pub archs: HashMap<String, Vec<(Rc<Arch>, Rc<Vec<String>>)>>,
    pub uses: Rc<Vec<String>>,
    pub px: Vec<Px>,
    pub inst_depth: u32,
    pub call_depth: u32,
    pub top_ports: Vec<TopPort>,
    pub top_name: String,
    pub prefix: String,
    /// Declared return type of each active function call.
    pub ret_tys: Vec<Ty>,
    /// Constrained type of the first value returned by each active call.
    pub ret_vtys: Vec<Option<Ty>>,
    /// (VHDL name, SV name) for top-level names that are SV keywords.
    pub renames: Vec<(String, String)>,
}

pub fn err<X>(line: Line, msg: impl AsRef<str>) -> R<X> {
    Err(format!("line {line}: {}", msg.as_ref()))
}

impl Elab {
    pub fn new() -> Self {
        let mut e = Elab {
            dag: Dag::default(),
            nets: Vec::new(),
            regs: Vec::new(),
            global: HashMap::new(),
            scopes: Vec::new(),
            entities: HashMap::new(),
            archs: HashMap::new(),
            uses: Rc::new(Vec::new()),
            px: Vec::new(),
            inst_depth: 0,
            call_depth: 0,
            top_ports: Vec::new(),
            top_name: String::new(),
            prefix: String::new(),
            ret_tys: Vec::new(),
            ret_vtys: Vec::new(),
            renames: Vec::new(),
        };
        e.std_env();
        e
    }

    fn std_env(&mut self) {
        let g = &mut self.global;
        let natural = Ty::Int { lo: 0, hi: INT_MAX };
        let mk_arr = |kind: ArrKind, elem: Ty, idx_ty: Ty| {
            Ty::Arr(Rc::new(ArrTy {
                kind,
                elem,
                idx: None,
                idx_ty,
            }))
        };
        for n in ["std_ulogic", "std_logic", "x01", "x01z", "ux01", "ux01z"] {
            g.insert(n.into(), Sym::Ty(Ty::Logic));
        }
        g.insert("bit".into(), Sym::Ty(Ty::Bit));
        g.insert("boolean".into(), Sym::Ty(Ty::Bool));
        g.insert("character".into(), Sym::Ty(Ty::Char));
        g.insert(
            "integer".into(),
            Sym::Ty(Ty::Int {
                lo: INT_MIN,
                hi: INT_MAX,
            }),
        );
        g.insert("natural".into(), Sym::Ty(natural.clone()));
        g.insert("positive".into(), Sym::Ty(Ty::Int { lo: 1, hi: INT_MAX }));
        g.insert("real".into(), Sym::Ty(Ty::Real));
        g.insert("time".into(), Sym::Ty(Ty::Time));
        g.insert("delay_length".into(), Sym::Ty(Ty::Time));
        g.insert("string".into(), Sym::Ty(Ty::Str));
        for n in ["std_logic_vector", "std_ulogic_vector"] {
            g.insert(
                n.into(),
                Sym::Ty(mk_arr(ArrKind::Slv, Ty::Logic, natural.clone())),
            );
        }
        for n in ["unsigned", "unresolved_unsigned", "u_unsigned"] {
            g.insert(
                n.into(),
                Sym::Ty(mk_arr(ArrKind::Unsigned, Ty::Logic, natural.clone())),
            );
        }
        for n in ["signed", "unresolved_signed", "u_signed"] {
            g.insert(
                n.into(),
                Sym::Ty(mk_arr(ArrKind::Signed, Ty::Logic, natural.clone())),
            );
        }
        g.insert(
            "bit_vector".into(),
            Sym::Ty(mk_arr(ArrKind::BitVec, Ty::Bit, natural.clone())),
        );
        g.insert(
            "boolean_vector".into(),
            Sym::Ty(mk_arr(
                ArrKind::User("boolean_vector".into()),
                Ty::Bool,
                natural.clone(),
            )),
        );
        g.insert(
            "integer_vector".into(),
            Sym::Ty(mk_arr(
                ArrKind::User("integer_vector".into()),
                Ty::Int {
                    lo: INT_MIN,
                    hi: INT_MAX,
                },
                natural.clone(),
            )),
        );
        g.insert("true".into(), Sym::Lits(vec![(Ty::Bool, 1)]));
        g.insert("false".into(), Sym::Lits(vec![(Ty::Bool, 0)]));
        let sev = Rc::new(EnumDef {
            name: "severity_level".into(),
            lits: vec![
                "note".into(),
                "warning".into(),
                "error".into(),
                "failure".into(),
            ],
        });
        g.insert("severity_level".into(), Sym::Ty(Ty::Enum(sev.clone())));
        for (i, l) in sev.lits.iter().enumerate() {
            g.insert(
                l.clone(),
                Sym::Lits(vec![(Ty::Enum(sev.clone()), i as u32)]),
            );
        }
        for p in [
            "work",
            "ieee",
            "std",
            "std_logic_1164",
            "numeric_std",
            "numeric_std_unsigned",
            "numeric_bit",
            "std_logic_arith",
            "std_logic_unsigned",
            "std_logic_signed",
            "std_logic_misc",
            "math_real",
            "standard",
            "textio",
            "std_logic_textio",
            "env",
            "fixed_pkg",
            "float_pkg",
            "unisim",
            "vcomponents",
            "unimacro",
        ] {
            g.insert(p.into(), Sym::Pkg);
        }
    }

    pub fn lookup(&self, name: &str) -> Option<&Sym> {
        for s in self.scopes.iter().rev() {
            if let Some(x) = s.get(name) {
                return Some(x);
            }
        }
        self.global.get(name)
    }

    pub fn declare(&mut self, name: &str, sym: Sym) {
        let tab = match self.scopes.last_mut() {
            Some(t) => t,
            None => &mut self.global,
        };
        match (&sym, tab.get_mut(name)) {
            (Sym::Lits(new), Some(Sym::Lits(old))) => old.extend(new.iter().cloned()),
            (Sym::Subs(new), Some(Sym::Subs(old))) => {
                for n in new {
                    // A body replaces a matching declaration.
                    if n.sp.body.is_some() {
                        if let Some(k) = old.iter().position(|o| {
                            o.sp.body.is_none()
                                && o.sp.params.iter().map(|p| p.names.len()).sum::<usize>()
                                    == n.sp.params.iter().map(|p| p.names.len()).sum::<usize>()
                        }) {
                            old[k] = n.clone();
                            continue;
                        }
                    }
                    old.push(n.clone());
                }
            }
            _ => {
                tab.insert(name.to_string(), sym);
            }
        }
    }

    pub fn uses_pkg(&self, p: &str) -> bool {
        self.uses.iter().any(|u| u.ends_with(p))
    }

    // ------------------------------------------------------------------
    // Nets

    pub fn new_net(&mut self, name: String, w: u32, init: Vec<bool>) -> u32 {
        assert_eq!(init.len() as u32, w);
        self.nets.push(Net {
            name,
            w,
            init,
            drivers: Vec::new(),
        });
        (self.nets.len() - 1) as u32
    }

    pub fn read_sig(&mut self, s: &SigRef) -> R<Val> {
        let w = s.ty.width()?;
        let nw = self.nets[s.net as usize].w;
        let r = self.dag.read(s.net, nw);
        let n = self.dag.slice(r, s.off, w);
        Ok(Val::B {
            ty: s.ty.clone(),
            n,
        })
    }

    pub fn add_driver(&mut self, net: u32, lo: u32, w: u32, src: DrvSrc, line: Line) {
        if w == 0 {
            return;
        }
        self.nets[net as usize]
            .drivers
            .push(Drv { lo, w, src, line });
    }

    // ------------------------------------------------------------------
    // Static evaluation helpers

    pub fn static_int(&mut self, e: &Expr, line: Line) -> R<i64> {
        let v = self.eval_static(e, None, line)?;
        self.val_to_int(&v)
            .ok_or_else(|| format!("line {line}: expected a static integer expression"))
    }

    pub fn val_to_int(&self, v: &Val) -> Option<i64> {
        match v {
            Val::B {
                ty: Ty::Int { lo, .. },
                n,
            } => {
                let bits = self.dag.as_const(*n)?;
                Some(decode_int(bits, *lo < 0))
            }
            Val::B {
                ty: Ty::Enum(_) | Ty::Bool | Ty::Logic | Ty::Bit,
                n,
            } => {
                let bits = self.dag.as_const(*n)?;
                Some(decode_int(bits, false))
            }
            Val::R(r) => Some(r.round() as i64),
            _ => None,
        }
    }

    pub fn int_val(&mut self, v: i64) -> Val {
        let w = int_w(v, v);
        let n = self.dag.kval(v as i128 as u128, w);
        Val::B {
            ty: Ty::Int { lo: v, hi: v },
            n,
        }
    }

    /// Evaluate an expression that must be static (outside a process).
    pub fn eval_static(&mut self, e: &Expr, want: Option<&Ty>, line: Line) -> R<Val> {
        let pushed = self.px.is_empty();
        if pushed {
            self.push_px(PxKind::Static, line, "static".into());
        }
        let r = self.eval(e, want, line);
        if pushed {
            self.px.pop();
        }
        r
    }

    pub fn push_px(&mut self, kind: PxKind, line: Line, label: String) {
        let z = self.dag.bit(false);
        self.px.push(Px {
            kind,
            st: St {
                vars: Vec::new(),
                pend: BTreeMap::new(),
                ret: z,
                retv: None,
                brk: z,
                cont: z,
            },
            var_tys: Vec::new(),
            var_names: Vec::new(),
            var_nets: Vec::new(),
            driven: HashMap::new(),
            loops: Vec::new(),
            line,
            label,
        });
    }

    // ------------------------------------------------------------------
    // Types

    pub fn eval_range(&mut self, r: &Range, line: Line) -> R<(IdxRange, Ty)> {
        match r {
            Range::Dir(a, down, b) => {
                let av = self.eval_static(a, None, line)?;
                let bv = self.eval_static(b, Some(&av.ty()), line)?;
                let ty = match av.ty() {
                    Ty::Enum(e) => Ty::Enum(e),
                    _ => Ty::Int {
                        lo: INT_MIN,
                        hi: INT_MAX,
                    },
                };
                let l = self
                    .val_to_int(&av)
                    .ok_or_else(|| format!("line {line}: range bound is not static"))?;
                let rr = self
                    .val_to_int(&bv)
                    .ok_or_else(|| format!("line {line}: range bound is not static"))?;
                Ok((
                    IdxRange {
                        left: l,
                        right: rr,
                        down: *down,
                    },
                    ty,
                ))
            }
            Range::Name(n) => {
                if let Name::Attr(p, a, _) = &**n {
                    if a == "range" || a == "reverse_range" {
                        let t = self.prefix_ty(p, line)?;
                        let (r, it) = match &t {
                            Ty::Arr(at) => (
                                at.idx.ok_or_else(|| {
                                    format!("line {line}: 'range of an unconstrained array")
                                })?,
                                at.idx_ty.clone(),
                            ),
                            Ty::Int { lo, hi } => (
                                IdxRange {
                                    left: *lo,
                                    right: *hi,
                                    down: false,
                                },
                                t.clone(),
                            ),
                            Ty::Enum(e) => (
                                IdxRange {
                                    left: 0,
                                    right: e.lits.len() as i64 - 1,
                                    down: false,
                                },
                                t.clone(),
                            ),
                            _ => return err(line, format!("'range of {}", t.name())),
                        };
                        let r = if a == "reverse_range" {
                            IdxRange {
                                left: r.right,
                                right: r.left,
                                down: !r.down,
                            }
                        } else {
                            r
                        };
                        return Ok((r, it));
                    }
                }
                // A (sub)type name used as a range.
                let t = self.resolve_mark(n, line)?;
                self.type_range(&t, line)
            }
        }
    }

    pub fn type_range(&self, t: &Ty, line: Line) -> R<(IdxRange, Ty)> {
        match t {
            Ty::Int { lo, hi } => Ok((
                IdxRange {
                    left: *lo,
                    right: *hi,
                    down: false,
                },
                t.clone(),
            )),
            Ty::Enum(e) => Ok((
                IdxRange {
                    left: 0,
                    right: e.lits.len() as i64 - 1,
                    down: false,
                },
                t.clone(),
            )),
            Ty::Bool | Ty::Bit => Ok((
                IdxRange {
                    left: 0,
                    right: 1,
                    down: false,
                },
                t.clone(),
            )),
            _ => err(line, format!("{} is not a discrete type", t.name())),
        }
    }

    pub fn eval_disc_range(&mut self, d: &DiscRange, line: Line) -> R<(IdxRange, Ty)> {
        match d {
            DiscRange::Range(r) => self.eval_range(r, line),
            DiscRange::Sub(si) => {
                let t = self.resolve_subind(si)?;
                self.type_range(&t, line)
            }
        }
    }

    pub fn resolve_mark(&mut self, n: &Name, line: Line) -> R<Ty> {
        match n {
            Name::Id(x, _) => match self.lookup(x) {
                Some(Sym::Ty(t)) => Ok(t.clone()),
                Some(Sym::Unsupported(w)) => {
                    err(line, format!("`{x}` uses an unsupported construct: {w}"))
                }
                Some(_) => err(line, format!("`{x}` is not a type")),
                None => err(line, format!("unknown type `{x}`")),
            },
            Name::Sel(p, f) => {
                // lib.pkg.type
                let _ = p;
                match self.global.get(f.as_str()).cloned() {
                    Some(Sym::Ty(t)) => Ok(t),
                    _ => self.resolve_mark(&Name::Id(f.clone(), line), line),
                }
            }
            Name::Attr(p, a, _) if a == "subtype" || a == "base" => self.prefix_ty(p, line),
            Name::Attr(p, a, _) if a == "element" => {
                let t = self.prefix_ty(p, line)?;
                match t {
                    Ty::Arr(a) => Ok(a.elem.clone()),
                    _ => err(line, "'element of a non-array"),
                }
            }
            _ => err(line, "expected a type mark"),
        }
    }

    pub fn resolve_subind(&mut self, si: &SubInd) -> R<Ty> {
        let line = si.line;
        let base = self.resolve_mark(&si.mark, line)?;
        let Some(c) = &si.cons else { return Ok(base) };
        match c {
            Constraint::Range(r) => {
                let (ir, _) = self.eval_range(r, line)?;
                match base {
                    Ty::Int { .. } => Ok(Ty::Int {
                        lo: ir.lo(),
                        hi: ir.hi(),
                    }),
                    Ty::Real => Ok(Ty::Real),
                    Ty::Enum(e) => Ok(Ty::Enum(e)),
                    t => err(line, format!("range constraint on {}", t.name())),
                }
            }
            Constraint::Index(dims, elems) => {
                let Ty::Arr(a) = &base else {
                    return err(
                        line,
                        format!("index constraint on non-array type {}", base.name()),
                    );
                };
                if dims.len() != 1 {
                    return err(line, "multi-dimensional arrays are not supported");
                }
                let idx = match &dims[0] {
                    Some(d) => Some(self.eval_disc_range(d, line)?.0),
                    None => a.idx,
                };
                let mut elem = a.elem.clone();
                if let Some(ec) = elems.first() {
                    if let (Some(Some(d)), Ty::Arr(ea)) = (ec.first(), &elem) {
                        let r = self.eval_disc_range(d, line)?.0;
                        elem = Ty::Arr(Rc::new(ArrTy {
                            kind: ea.kind.clone(),
                            elem: ea.elem.clone(),
                            idx: Some(r),
                            idx_ty: ea.idx_ty.clone(),
                        }));
                    }
                }
                Ok(Ty::Arr(Rc::new(ArrTy {
                    kind: a.kind.clone(),
                    elem,
                    idx,
                    idx_ty: a.idx_ty.clone(),
                })))
            }
        }
    }

    fn type_def(&mut self, name: &str, def: &TypeDef, line: Line) -> R<Ty> {
        match def {
            TypeDef::Enum(lits) => {
                let names: Vec<String> = lits
                    .iter()
                    .map(|l| match l {
                        EnumLit::Id(s) => s.clone(),
                        EnumLit::Char(c) => format!("'{c}'"),
                    })
                    .collect();
                let ed = Rc::new(EnumDef {
                    name: name.to_string(),
                    lits: names.clone(),
                });
                for (i, l) in names.iter().enumerate() {
                    self.declare(l, Sym::Lits(vec![(Ty::Enum(ed.clone()), i as u32)]));
                }
                Ok(Ty::Enum(ed))
            }
            TypeDef::Int(r) => {
                let (ir, _) = self.eval_range(r, line)?;
                Ok(Ty::Int {
                    lo: ir.lo(),
                    hi: ir.hi(),
                })
            }
            TypeDef::Real(_) => Ok(Ty::Real),
            TypeDef::Array { idx, elem } => {
                if idx.len() != 1 {
                    return err(
                        line,
                        format!("multi-dimensional array type {name} is not supported"),
                    );
                }
                let et = self.resolve_subind(elem)?;
                let (ir, it) = match &idx[0] {
                    IdxDef::Unbounded(m) => {
                        let t = self.resolve_mark(m, line)?;
                        (None, t)
                    }
                    IdxDef::Cons(d) => {
                        let (r, t) = self.eval_disc_range(d, line)?;
                        (Some(r), t)
                    }
                };
                let kind = ArrKind::User(name.to_string());
                Ok(Ty::Arr(Rc::new(ArrTy {
                    kind,
                    elem: et,
                    idx: ir,
                    idx_ty: it,
                })))
            }
            TypeDef::Record(fields) => {
                let mut fs = Vec::new();
                for (names, si) in fields {
                    let t = self.resolve_subind(si)?;
                    for n in names {
                        fs.push((n.clone(), t.clone()));
                    }
                }
                Ok(Ty::Rec(Rc::new(RecTy {
                    name: name.to_string(),
                    fields: fs,
                })))
            }
            TypeDef::Unsupported(w) => Err(w.clone()),
        }
    }

    /// Give an unconstrained declared type the index range of a value.
    pub fn constrain_from(&self, decl: &Ty, v: &Val, line: Line) -> R<Ty> {
        match (decl, v.ty()) {
            (Ty::Arr(a), Ty::Arr(b)) if a.idx.is_none() => {
                let vr = b
                    .idx
                    .ok_or_else(|| format!("line {line}: cannot infer array bounds"))?;
                // Constant of an unconstrained type: VHDL takes the index
                // subtype's left bound and direction for literal aggregates;
                // for named values it keeps the value's range.
                Ok(Ty::Arr(Rc::new(ArrTy {
                    kind: a.kind.clone(),
                    elem: a.elem.clone(),
                    idx: Some(vr),
                    idx_ty: a.idx_ty.clone(),
                })))
            }
            _ => Ok(decl.clone()),
        }
    }

    // ------------------------------------------------------------------
    // Declarations

    pub fn elab_decls(&mut self, decls: &[Decl]) -> R<()> {
        for d in decls {
            self.elab_decl(d)?;
        }
        Ok(())
    }

    pub fn elab_decl(&mut self, d: &Decl) -> R<()> {
        match d {
            Decl::Signal {
                names,
                ty,
                init,
                line,
            } => {
                let t = self.resolve_subind(ty)?;
                let t = if let (Ty::Arr(a), Some(e)) = (&t, init) {
                    if a.idx.is_none() {
                        let v = self.eval_static(e, Some(&t), *line)?;
                        self.constrain_from(&t, &v, *line)?
                    } else {
                        t
                    }
                } else {
                    t
                };
                let w = t.width().map_err(|e| format!("line {line}: {e}"))?;
                let init_bits = match init {
                    Some(e) => {
                        let v = self.eval_static(e, Some(&t), *line)?;
                        let v = self.convert(v, &t, *line)?;
                        match &v {
                            Val::B { n, .. } => match self.dag.as_const(*n) {
                                Some(b) => b.to_vec(),
                                None => return err(*line, "signal initial value is not static"),
                            },
                            _ => return err(*line, "bad signal initial value"),
                        }
                    }
                    None => t.default_bits()?,
                };
                for (n, _) in names {
                    let net = self.new_net(format!("{}{n}", self.prefix), w, init_bits.clone());
                    self.declare(
                        n,
                        Sym::Sig(SigRef {
                            net,
                            off: 0,
                            ty: t.clone(),
                        }),
                    );
                }
            }
            Decl::Constant {
                names,
                ty,
                val,
                line,
            } => {
                let t = self.resolve_subind(ty)?;
                match val {
                    Some(e) => {
                        let v = self.eval_static(e, Some(&t), *line)?;
                        let t2 = self.constrain_from(&t, &v, *line)?;
                        let v = self.convert(v, &t2, *line)?;
                        for (n, _) in names {
                            self.declare(n, Sym::Val(v.clone()));
                        }
                    }
                    None => {
                        for (n, _) in names {
                            self.declare(
                                n,
                                Sym::Unsupported(format!(
                                    "deferred constant {n} without a value in the package body"
                                )),
                            );
                        }
                    }
                }
            }
            Decl::Variable {
                names,
                ty,
                init,
                shared,
                line,
            } => {
                if *shared {
                    for (n, _) in names {
                        self.declare(n, Sym::Unsupported(format!("shared variable {n}")));
                    }
                    return Ok(());
                }
                if self.px.is_empty() {
                    return err(*line, "variable declared outside a process or subprogram");
                }
                let t = self.resolve_subind(ty)?;
                let t = if let (Ty::Arr(a), Some(e)) = (&t, init) {
                    if a.idx.is_none() {
                        let v = self.eval(e, Some(&t), *line)?;
                        self.constrain_from(&t, &v, *line)?
                    } else {
                        t
                    }
                } else {
                    t
                };
                let w = t.width().map_err(|e| format!("line {line}: {e}"))?;
                for (n, _) in names {
                    let init_n = match init {
                        Some(e) => {
                            let v = self.eval(e, Some(&t), *line)?;
                            let v = self.convert(v, &t, *line)?;
                            match v {
                                Val::B { n, .. } => n,
                                _ => return err(*line, "bad variable initial value"),
                            }
                        }
                        None => {
                            let b = t.default_bits()?;
                            self.dag.konst(b)
                        }
                    };
                    let slot = self.new_var(n, &t, w, init_n, *line)?;
                    self.declare(n, Sym::Var(slot, t.clone()));
                }
            }
            Decl::Type { name, def, line } => match self.type_def(name, def, *line) {
                Ok(t) => self.declare(name, Sym::Ty(t)),
                Err(w) => self.declare(name, Sym::Unsupported(w)),
            },
            Decl::Subtype { name, ind, .. } => {
                let t = self.resolve_subind(ind)?;
                self.declare(name, Sym::Ty(t));
            }
            Decl::Component {
                name,
                generics,
                ports,
                ..
            } => {
                self.declare(name, Sym::Comp(Rc::new((generics.clone(), ports.clone()))));
            }
            Decl::Subprog(sp) => {
                let depth = self.scopes.len();
                self.declare(
                    &sp.name,
                    Sym::Subs(vec![Rc::new(SubDef {
                        sp: sp.clone(),
                        depth,
                    })]),
                );
            }
            Decl::Alias {
                name,
                ty,
                target,
                line,
            } => {
                let sym = self.alias_target(target, *line)?;
                let sym = match (sym, ty) {
                    (Sym::Sig(mut s), Some(si)) => {
                        let t = self.resolve_subind(si)?;
                        let t = if let (Ty::Arr(a), Ty::Arr(b)) = (&t, &s.ty) {
                            if a.idx.is_none() {
                                t.with_idx(b.idx.unwrap_or(IdxRange::canonical(0)))
                            } else {
                                t
                            }
                        } else {
                            t
                        };
                        if t.width()? != s.ty.width()? {
                            return err(
                                *line,
                                format!("alias {name}: subtype width differs from the object"),
                            );
                        }
                        s.ty = t;
                        Sym::Sig(s)
                    }
                    (s, _) => s,
                };
                self.declare(name, sym);
            }
            Decl::Ignored { .. } => {}
            Decl::Unsupported { name, what, .. } => {
                if let Some(n) = name {
                    self.declare(n, Sym::Unsupported(what.clone()));
                }
            }
        }
        Ok(())
    }

    fn alias_target(&mut self, target: &Name, line: Line) -> R<Sym> {
        // Static name of a signal: resolve to a SigRef.
        if let Ok(l) = self.lval(&Expr::Name(Box::new(target.clone())), line) {
            if let super::expr::LBase::Net(net) = l.base {
                if let super::expr::Off::Static(off) = l.off {
                    return Ok(Sym::Sig(SigRef { net, off, ty: l.ty }));
                }
            }
        }
        let v = self.eval(&Expr::Name(Box::new(target.clone())), None, line)?;
        Ok(Sym::Val(v))
    }

    pub fn new_var(&mut self, name: &str, t: &Ty, w: u32, init: N, line: Line) -> R<u32> {
        let kind = self.px.last().map(|p| p.kind).unwrap_or(PxKind::Static);
        let label = self.px.last().map(|p| p.label.clone()).unwrap_or_default();
        let slot = self.px.last().unwrap().st.vars.len() as u32;
        let mut vnet = None;
        let start = match kind {
            // Process variables keep their value between activations: a
            // register in a clocked process (removed if never read before
            // written), a latch (error if observed) in a combinational one.
            PxKind::Clocked if self.call_depth == 0 => {
                let init_bits =
                    self.dag.as_const(init).map(|b| b.to_vec()).ok_or_else(|| {
                        format!("line {line}: variable initial value is not static")
                    })?;
                let net = self.new_net(format!("{label}.{name}"), w, init_bits);
                vnet = Some(net);
                self.dag.read(net, w)
            }
            PxKind::Comb if self.call_depth == 0 => self.dag.vhold(slot, w),
            _ => init,
        };
        let px = self.px.last_mut().unwrap();
        px.var_nets.push(vnet);
        px.st.vars.push(start);
        px.var_tys.push(t.clone());
        px.var_names.push(name.to_string());
        Ok(slot)
    }

    // ------------------------------------------------------------------
    // Design hierarchy

    pub fn load(&mut self, units: Vec<DesignUnit>) -> R<()> {
        // Packages first (in order), then index entities/architectures.
        for du in &units {
            self.uses = Rc::new(du.uses.clone());
            match &du.unit {
                Unit::Package { name, decls } | Unit::PackageBody { name, decls } => {
                    self.global.insert(name.clone(), Sym::Pkg);
                    self.elab_decls(decls)?;
                }
                _ => {}
            }
        }
        let mut last_uses: Rc<Vec<String>> = Rc::new(Vec::new());
        for du in units {
            let uses = Rc::new(du.uses.clone());
            match du.unit {
                Unit::Entity(e) => {
                    last_uses = uses.clone();
                    self.entities.insert(e.name.clone(), (Rc::new(e), uses));
                }
                Unit::Arch(a) => {
                    // Architectures inherit their entity's context clause.
                    let mut u: Vec<String> = self
                        .entities
                        .get(&a.entity)
                        .map(|(_, u)| (**u).clone())
                        .unwrap_or_else(|| (*last_uses).clone());
                    u.extend(uses.iter().cloned());
                    self.archs
                        .entry(a.entity.clone())
                        .or_default()
                        .push((Rc::new(a), Rc::new(u)));
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn pick_top(&self, top: Option<&str>) -> R<String> {
        if let Some(t) = top {
            let t = t.to_ascii_lowercase();
            if !self.entities.contains_key(&t) {
                return Err(format!("top entity `{t}` not found"));
            }
            return Ok(t);
        }
        let mut used: HashSet<String> = HashSet::new();
        fn walk(c: &[Conc], used: &mut HashSet<String>) {
            for s in c {
                match s {
                    Conc::Inst { unit, .. } => {
                        match unit {
                            InstUnit::Entity { name, .. }
                            | InstUnit::Comp(name)
                            | InstUnit::Config(name) => used.insert(name.clone()),
                        };
                    }
                    Conc::ForGen { body, .. } | Conc::Block { body, .. } => walk(body, used),
                    Conc::IfGen { arms, els, .. } => {
                        for (_, _, b) in arms {
                            walk(b, used);
                        }
                        if let Some((_, b)) = els {
                            walk(b, used);
                        }
                    }
                    Conc::CaseGen { arms, .. } => {
                        for (_, _, b) in arms {
                            walk(b, used);
                        }
                    }
                    _ => {}
                }
            }
        }
        for v in self.archs.values() {
            for (a, _) in v {
                walk(&a.body, &mut used);
            }
        }
        let mut roots: Vec<(u32, String)> = self
            .entities
            .iter()
            .filter(|(n, _)| !used.contains(*n) && self.archs.contains_key(*n))
            .map(|(n, (e, _))| (e.line, n.clone()))
            .collect();
        roots.sort();
        match roots.last() {
            Some((_, n)) => Ok(n.clone()),
            None => Err("no top-level entity with an architecture".into()),
        }
    }

    pub fn elab_top(&mut self, top: &str) -> R<()> {
        let (ent, euses) = self
            .entities
            .get(top)
            .cloned()
            .ok_or_else(|| format!("entity `{top}` not found"))?;
        let (arch, auses) = self
            .archs
            .get(top)
            .and_then(|v| v.last().cloned())
            .ok_or_else(|| format!("entity `{top}` has no architecture"))?;
        let _ = euses;
        self.top_name = ent.orig.clone();
        self.uses = auses;
        self.scopes.push(HashMap::new());
        for g in &ent.generics {
            let t = self.resolve_subind(&g.ty)?;
            for (n, _) in &g.names {
                let Some(d) = &g.default else {
                    return err(
                        g.line,
                        format!("top-level generic `{n}` has no default value"),
                    );
                };
                let v = self.eval_static(d, Some(&t), g.line)?;
                let t2 = self.constrain_from(&t, &v, g.line)?;
                let v = self.convert(v, &t2, g.line)?;
                self.declare(n, Sym::Val(v));
            }
        }
        for p in &ent.ports {
            let t = self.resolve_subind(&p.ty)?;
            if matches!(&t, Ty::Arr(a) if a.idx.is_none()) {
                return err(p.line, "top-level port with an unconstrained type");
            }
            let w = t.width().map_err(|e| format!("line {}: {e}", p.line))?;
            for (n, orig) in &p.names {
                if p.mode == Mode::InOut {
                    return err(
                        p.line,
                        format!(
                            "inout port `{orig}`: bidirectional (tri-state) ports are not supported yet"
                        ),
                    );
                }
                let init = match (&p.default, p.mode) {
                    (Some(d), Mode::Out | Mode::Buffer) => {
                        let v = self.eval_static(d, Some(&t), p.line)?;
                        let v = self.convert(v, &t, p.line)?;
                        match v {
                            Val::B { n, .. } => self
                                .dag
                                .as_const(n)
                                .map(|b| b.to_vec())
                                .unwrap_or(t.default_bits()?),
                            _ => t.default_bits()?,
                        }
                    }
                    _ => t.default_bits()?,
                };
                let net = self.new_net(orig.clone(), w, init);
                let range = match &t {
                    Ty::Arr(a) if a.elem.is_bit() => a.idx.map(|r| (r.left, r.right)),
                    Ty::Logic | Ty::Bit | Ty::Bool => None,
                    _ => Some(((w as i64) - 1, 0)),
                };
                self.top_ports.push(TopPort {
                    name: orig.clone(),
                    dir: p.mode,
                    net,
                    range,
                });
                self.declare(
                    n,
                    Sym::Sig(SigRef {
                        net,
                        off: 0,
                        ty: t.clone(),
                    }),
                );
            }
        }
        self.prefix = String::new();
        self.elab_decls(&ent.decls)?;
        self.elab_decls(&arch.decls)?;
        self.elab_concs(&arch.body)?;
        self.scopes.pop();
        Ok(())
    }

    pub fn elab_concs(&mut self, body: &[Conc]) -> R<()> {
        for c in body {
            self.elab_conc(c)?;
        }
        Ok(())
    }

    fn elab_conc(&mut self, c: &Conc) -> R<()> {
        match c {
            Conc::Process {
                label,
                sens,
                decls,
                body,
                line,
            } => {
                let lbl = label.clone().unwrap_or_else(|| format!("proc_l{line}"));
                self.elab_process(&lbl, sens, decls, body, *line)
            }
            Conc::CondAsg { target, arms, line } => {
                let body = vec![Seq::SigAsg {
                    target: target.clone(),
                    rhs: arms.clone(),
                    line: *line,
                }];
                self.elab_process(&format!("asg_l{line}"), &Sens::All, &[], &body, *line)
            }
            Conc::SelAsg {
                sel,
                target,
                arms,
                line,
            } => {
                let body = vec![Seq::SelAsg {
                    sel: sel.clone(),
                    target: target.clone(),
                    var: false,
                    arms: arms.clone(),
                    line: *line,
                }];
                self.elab_process(&format!("sel_l{line}"), &Sens::All, &[], &body, *line)
            }
            Conc::Inst {
                label,
                unit,
                gmap,
                pmap,
                line,
            } => self.elab_inst(label, unit, gmap, pmap, *line),
            Conc::ForGen {
                label,
                var,
                range,
                decls,
                body,
                line,
            } => {
                let (r, it) = self.eval_disc_range(range, *line)?;
                for i in r.iter() {
                    let saved = self.prefix.clone();
                    self.prefix = format!("{saved}{label}({i}).");
                    self.scopes.push(HashMap::new());
                    let v = match &it {
                        Ty::Enum(_) => {
                            let w = it.width()?;
                            let n = self.dag.kval(i as u128, w);
                            Val::B { ty: it.clone(), n }
                        }
                        _ => self.int_val(i),
                    };
                    self.declare(var, Sym::Val(v));
                    let r = self.elab_decls(decls).and_then(|_| self.elab_concs(body));
                    self.scopes.pop();
                    self.prefix = saved;
                    r?;
                }
                Ok(())
            }
            Conc::IfGen {
                label,
                arms,
                els,
                line,
            } => {
                for (cond, decls, body) in arms {
                    let v = self.eval_static(cond, Some(&Ty::Bool), *line)?;
                    let b = self.val_to_int(&v).ok_or_else(|| {
                        format!("line {line}: if-generate condition is not static")
                    })?;
                    if b != 0 {
                        return self.gen_scope(label, decls, body);
                    }
                }
                if let Some((decls, body)) = els {
                    return self.gen_scope(label, decls, body);
                }
                Ok(())
            }
            Conc::CaseGen {
                label,
                sel,
                arms,
                line,
            } => {
                let sv = self.eval_static(sel, None, *line)?;
                let st = sv.ty();
                for (choices, decls, body) in arms {
                    let mut hit = false;
                    for ch in choices {
                        match ch {
                            Choice::Others => hit = true,
                            Choice::Expr(e) => {
                                let cv = self.eval_static(e, Some(&st), *line)?;
                                let eq =
                                    self.compare(super::ast::BinOp::Eq, sv.clone(), cv, *line)?;
                                if self.val_to_int(&eq) == Some(1) {
                                    hit = true;
                                }
                            }
                            Choice::Range(r) => {
                                let (ir, _) = self.eval_range(r, *line)?;
                                if let Some(x) = self.val_to_int(&sv) {
                                    if ir.contains(x) {
                                        hit = true;
                                    }
                                }
                            }
                        }
                    }
                    if hit {
                        return self.gen_scope(label, decls, body);
                    }
                }
                Ok(())
            }
            Conc::Block {
                label, decls, body, ..
            } => self.gen_scope(label, decls, body),
            Conc::Call { line, .. } => err(*line, "concurrent procedure calls are not supported"),
            Conc::Ignored => Ok(()),
        }
    }

    fn gen_scope(&mut self, label: &str, decls: &[Decl], body: &[Conc]) -> R<()> {
        let saved = self.prefix.clone();
        self.prefix = format!("{saved}{label}.");
        self.scopes.push(HashMap::new());
        let r = self.elab_decls(decls).and_then(|_| self.elab_concs(body));
        self.scopes.pop();
        self.prefix = saved;
        r
    }

    fn elab_inst(
        &mut self,
        label: &str,
        unit: &InstUnit,
        gmap: &[Assoc],
        pmap: &[Assoc],
        line: Line,
    ) -> R<()> {
        let (ename, aname, comp) = match unit {
            InstUnit::Entity { name, arch } => (name.clone(), arch.clone(), None),
            InstUnit::Comp(name) => {
                let comp = match self.lookup(name) {
                    Some(Sym::Comp(c)) => Some(c.clone()),
                    _ => None,
                };
                (name.clone(), None, comp)
            }
            InstUnit::Config(name) => {
                return err(
                    line,
                    format!("configuration instance `{name}` is not supported"),
                );
            }
        };
        let Some((ent, _)) = self.entities.get(&ename).cloned() else {
            if is_vendor_primitive(&ename) {
                return err(
                    line,
                    format!(
                        "instance `{label}` of vendor primitive `{ename}`: device primitives are not supported in VHDL yet"
                    ),
                );
            }
            return err(
                line,
                format!("instance `{label}`: no entity `{ename}` in the design sources"),
            );
        };
        let arch = match &aname {
            Some(a) => self
                .archs
                .get(&ename)
                .and_then(|v| v.iter().rev().find(|(x, _)| &x.name == a).cloned())
                .ok_or_else(|| {
                    format!("line {line}: entity `{ename}` has no architecture `{a}`")
                })?,
            None => self
                .archs
                .get(&ename)
                .and_then(|v| v.last().cloned())
                .ok_or_else(|| format!("line {line}: entity `{ename}` has no architecture"))?,
        };
        if self.inst_depth > 64 {
            return err(
                line,
                "instance hierarchy deeper than 64 (recursive instantiation?)",
            );
        }
        // Formal lists: positional association follows the component
        // declaration when instantiating a component.
        let gformals: Vec<String> = match &comp {
            Some(c) => {
                c.0.iter()
                    .flat_map(|g| g.names.iter().map(|n| n.0.clone()))
                    .collect()
            }
            None => ent
                .generics
                .iter()
                .flat_map(|g| g.names.iter().map(|n| n.0.clone()))
                .collect(),
        };
        let pformals: Vec<String> = match &comp {
            Some(c) => {
                c.1.iter()
                    .flat_map(|g| g.names.iter().map(|n| n.0.clone()))
                    .collect()
            }
            None => ent
                .ports
                .iter()
                .flat_map(|g| g.names.iter().map(|n| n.0.clone()))
                .collect(),
        };
        // Generic actuals (static, evaluated in the parent scope).
        let mut gvals: HashMap<String, (Expr, bool)> = HashMap::new();
        for (k, a) in gmap.iter().enumerate() {
            let f = match &a.formal {
                Some(Name::Id(f, _)) => f.clone(),
                Some(_) => return err(line, "unsupported generic formal"),
                None => gformals
                    .get(k)
                    .cloned()
                    .ok_or_else(|| format!("line {line}: too many generic actuals"))?,
            };
            gvals.insert(f, (a.actual.clone(), true));
        }
        // Component default generics apply when the instance omits them.
        if let Some(c) = &comp {
            for g in &c.0 {
                for (n, _) in &g.names {
                    if !gvals.contains_key(n) {
                        if let Some(d) = &g.default {
                            gvals.insert(n.clone(), (d.clone(), true));
                        }
                    }
                }
            }
        }
        let mut gstatic: HashMap<String, Val> = HashMap::new();
        for g in &ent.generics {
            for (n, _) in &g.names {
                if let Some((e, _)) = gvals.get(n) {
                    if matches!(e, Expr::Open) {
                        continue;
                    }
                    // Type of the generic is resolved in the child scope;
                    // evaluate the actual without a target type first.
                    let v = self.eval_static(e, None, line)?;
                    gstatic.insert(n.clone(), v);
                }
            }
        }
        // Port actuals: value (inputs) or lvalue (outputs), from the parent scope.
        let mut pacts: HashMap<String, Vec<(Option<Name>, Expr)>> = HashMap::new();
        for (k, a) in pmap.iter().enumerate() {
            match &a.formal {
                Some(f) => {
                    let base = formal_base(f)
                        .ok_or_else(|| format!("line {line}: unsupported port formal"))?;
                    let sub = if matches!(f, Name::Id(..)) {
                        None
                    } else {
                        Some(f.clone())
                    };
                    pacts.entry(base).or_default().push((sub, a.actual.clone()));
                }
                None => {
                    let f = pformals
                        .get(k)
                        .cloned()
                        .ok_or_else(|| format!("line {line}: too many port actuals"))?;
                    pacts.entry(f).or_default().push((None, a.actual.clone()));
                }
            }
        }
        let child_prefix = format!("{}{label}.", self.prefix);
        // Pre-evaluate input actuals and output lvalues in the parent scope.
        enum Act {
            In(Option<Name>, Val),
            Out(Option<Name>, super::expr::LVal),
            Open,
        }
        let mut acts: HashMap<String, Vec<Act>> = HashMap::new();
        for p in &ent.ports {
            for (n, orig) in &p.names {
                let Some(list) = pacts.get(n) else { continue };
                let mut v = Vec::new();
                for (sub, e) in list {
                    if matches!(e, Expr::Open) {
                        v.push(Act::Open);
                        continue;
                    }
                    match p.mode {
                        Mode::In => {
                            let val = self
                                .eval_static(e, None, line)
                                .map_err(|er| format!("{er} (port `{orig}` of `{label}`)"))?;
                            v.push(Act::In(sub.clone(), val));
                        }
                        Mode::Out | Mode::Buffer => {
                            let lv = self.lval(e, line).map_err(|er| format!("{er} (output port `{orig}` of `{label}` must connect to a signal)"))?;
                            if !matches!(lv.base, super::expr::LBase::Net(_)) {
                                return err(
                                    line,
                                    format!(
                                        "output port `{orig}` of `{label}` must connect to a signal"
                                    ),
                                );
                            }
                            v.push(Act::Out(sub.clone(), lv));
                        }
                        _ => {
                            return err(
                                line,
                                format!(
                                    "port `{orig}` of `{label}`: inout/linkage ports are not supported yet"
                                ),
                            );
                        }
                    }
                }
                acts.insert(n.clone(), v);
            }
        }
        // Enter the child.
        let saved_scopes = std::mem::take(&mut self.scopes);
        let saved_prefix = std::mem::replace(&mut self.prefix, child_prefix.clone());
        let saved_uses = std::mem::replace(&mut self.uses, arch.1.clone());
        let saved_px = std::mem::take(&mut self.px);
        self.inst_depth += 1;
        let r = (|| -> R<()> {
            self.scopes.push(HashMap::new());
            for g in &ent.generics {
                let t = self.resolve_subind(&g.ty)?;
                for (n, _) in &g.names {
                    let v = match gstatic.get(n) {
                        Some(v) => v.clone(),
                        None => match &g.default {
                            Some(d) => self.eval_static(d, Some(&t), g.line)?,
                            None => {
                                return err(
                                    line,
                                    format!("instance `{label}`: generic `{n}` has no value"),
                                );
                            }
                        },
                    };
                    let t2 = self.constrain_from(&t, &v, g.line)?;
                    let v = self.convert(v, &t2, g.line)?;
                    self.declare(n, Sym::Val(v));
                }
            }
            for p in &ent.ports {
                let t0 = self.resolve_subind(&p.ty)?;
                for (n, orig) in &p.names {
                    let list = acts.remove(n).unwrap_or_default();
                    // Unconstrained port: take the range from a whole actual.
                    let t = match (&t0, list.first()) {
                        (Ty::Arr(a), Some(Act::In(None, v))) if a.idx.is_none() => {
                            self.constrain_from(&t0, v, line)?
                        }
                        (Ty::Arr(a), Some(Act::Out(None, lv))) if a.idx.is_none() => self
                            .constrain_from(
                                &t0,
                                &Val::B {
                                    ty: lv.ty.clone(),
                                    n: 0,
                                },
                                line,
                            )?,
                        _ => t0.clone(),
                    };
                    if matches!(&t, Ty::Arr(a) if a.idx.is_none()) {
                        return err(
                            line,
                            format!(
                                "port `{orig}` of `{label}`: cannot infer the bounds of an unconstrained port"
                            ),
                        );
                    }
                    let w = t.width().map_err(|e| format!("line {}: {e}", p.line))?;
                    let init = t.default_bits()?;
                    let net = self.new_net(format!("{child_prefix}{n}"), w, init);
                    let sref = SigRef {
                        net,
                        off: 0,
                        ty: t.clone(),
                    };
                    match p.mode {
                        Mode::In => {
                            if list.is_empty() || list.iter().all(|a| matches!(a, Act::Open)) {
                                // Unconnected input: default expression or zero.
                                let v = match &p.default {
                                    Some(d) => {
                                        let v = self.eval_static(d, Some(&t), p.line)?;
                                        self.convert(v, &t, p.line)?
                                    }
                                    None => {
                                        let z = self.dag.konst(t.default_bits()?);
                                        Val::B {
                                            ty: t.clone(),
                                            n: z,
                                        }
                                    }
                                };
                                if let Val::B { n: vn, .. } = v {
                                    self.add_driver(net, 0, w, DrvSrc::Comb(vn), line);
                                }
                            } else {
                                for a in list {
                                    let Act::In(sub, v) = a else { continue };
                                    match sub {
                                        None => {
                                            let v = self.convert(v, &t, line).map_err(|e| {
                                                format!("{e} (port `{orig}` of `{label}`)")
                                            })?;
                                            if let Val::B { n: vn, .. } = v {
                                                self.add_driver(net, 0, w, DrvSrc::Comb(vn), line);
                                            }
                                        }
                                        Some(f) => {
                                            // Partial association: formal is a slice/element of the port.
                                            self.scopes.push(HashMap::from([(
                                                n.clone(),
                                                Sym::Sig(sref.clone()),
                                            )]));
                                            let lv =
                                                self.lval(&Expr::Name(Box::new(f.clone())), line);
                                            self.scopes.pop();
                                            let lv = lv?;
                                            let super::expr::Off::Static(off) = lv.off else {
                                                return err(line, "dynamic formal");
                                            };
                                            let v = self.convert(v, &lv.ty, line)?;
                                            if let Val::B { n: vn, .. } = v {
                                                let lw = lv.ty.width()?;
                                                self.add_driver(
                                                    net,
                                                    off,
                                                    lw,
                                                    DrvSrc::Comb(vn),
                                                    line,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        _ => {
                            for a in list {
                                let Act::Out(sub, lv) = a else { continue };
                                let super::expr::LBase::Net(pnet) = lv.base else {
                                    unreachable!()
                                };
                                let super::expr::Off::Static(poff) = lv.off else {
                                    return err(
                                        line,
                                        format!(
                                            "output port `{orig}` of `{label}` connects to a dynamically indexed signal"
                                        ),
                                    );
                                };
                                let (coff, cty) = match &sub {
                                    None => (0, t.clone()),
                                    Some(f) => {
                                        self.scopes.push(HashMap::from([(
                                            n.clone(),
                                            Sym::Sig(sref.clone()),
                                        )]));
                                        let l2 = self.lval(&Expr::Name(Box::new(f.clone())), line);
                                        self.scopes.pop();
                                        let l2 = l2?;
                                        let super::expr::Off::Static(o) = l2.off else {
                                            return err(line, "dynamic formal");
                                        };
                                        (o, l2.ty)
                                    }
                                };
                                let cw = cty.width()?;
                                let pw = lv.ty.width()?;
                                if cw != pw {
                                    return err(
                                        line,
                                        format!(
                                            "port `{orig}` of `{label}`: width {cw} connected to a {pw}-bit signal"
                                        ),
                                    );
                                }
                                let rd = self.dag.read(net, w);
                                let s = self.dag.slice(rd, coff, cw);
                                self.add_driver(pnet, poff, pw, DrvSrc::Comb(s), line);
                            }
                        }
                    }
                    self.declare(n, Sym::Sig(sref));
                }
            }
            self.elab_decls(&ent.decls)?;
            self.elab_decls(&arch.0.decls)?;
            self.elab_concs(&arch.0.body)?;
            Ok(())
        })();
        self.inst_depth -= 1;
        self.scopes = saved_scopes;
        self.prefix = saved_prefix;
        self.uses = saved_uses;
        self.px = saved_px;
        r
    }
}

fn formal_base(f: &Name) -> Option<String> {
    match f {
        Name::Id(s, _) => Some(s.clone()),
        Name::Call(p, _) | Name::Slice(p, _) | Name::Sel(p, _) => formal_base(p),
        _ => None,
    }
}

pub fn decode_int(bits: &[bool], signed: bool) -> i64 {
    let mut v: i128 = 0;
    for (i, &b) in bits.iter().enumerate().take(64) {
        if b {
            v |= 1 << i;
        }
    }
    if signed && !bits.is_empty() && bits[bits.len() - 1] && bits.len() < 128 {
        v -= 1i128 << bits.len();
    }
    v as i64
}

fn is_vendor_primitive(n: &str) -> bool {
    let n = n.to_ascii_uppercase();
    n.starts_with("IBUF")
        || n.starts_with("OBUF")
        || n.starts_with("BUFG")
        || n.starts_with("MMCM")
        || n.starts_with("PLLE")
        || n.starts_with("FD")
        || n.starts_with("LUT")
        || n.starts_with("RAMB")
        || n.starts_with("DSP48")
        || n.starts_with("IDDR")
        || n.starts_with("ODDR")
        || n.starts_with("SRL")
        || n.starts_with("CARRY")
}
