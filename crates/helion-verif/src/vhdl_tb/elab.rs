//! Elaboration: binds names, builds types, creates signals, and compiles
//! processes / subprograms to [`Op`] lists. Procedures are inlined at their
//! call sites (so a `wait` inside a procedure suspends the calling process).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::ast::*;
use super::eval::{Host, Mach};
use super::ir::*;
use super::value::*;
use crate::tb::TbError;

/// A design file available to elaboration.
pub struct Source {
    pub label: String,
    pub file: DesignFile,
}

#[derive(Clone)]
pub enum Bind {
    Sig(SigId),
    Var { frame: usize, slot: usize, ty: Ty },
    Const(Value, Ty),
    Type(Ty),
    EnumLit(Ty, u32),
    Subs(Rc<RefCell<Vec<SubDef>>>),
    /// Procedure parameter aliasing an actual (read expression, target).
    Alias { read: RExpr, write: Option<RTarget>, ty: Option<Ty> },
    /// Library / package prefix (`ieee`, `std`, `work`, `env`).
    Unit,
}

#[derive(Clone)]
pub struct SubDef {
    pub ast: Rc<Subprogram>,
    /// Declaring file (for locations).
    pub file: u16,
    pub compiled: Option<usize>,
}

/// A netlist DUT instance (SV module, or a VHDL entity the behavioral
/// elaborator could not take): its ports bound to signals.
#[derive(Clone, Debug)]
pub struct BridgeDef {
    pub label: String,
    pub unit: String,
    pub path: std::path::PathBuf,
    /// `(port name, signal, is_input)`.
    pub ports: Vec<(String, SigId, bool)>,
}

/// Fully elaborated design.
pub struct Elab {
    pub files: Vec<String>,
    pub signals: Vec<SignalDef>,
    pub procs: Vec<ProcDef>,
    pub funcs: Vec<Func>,
    pub bridges: Vec<BridgeDef>,
    pub top: String,
    /// First instance in the top architecture: (unit, label, port map).
    pub dut: Option<(String, String, Vec<(String, String)>)>,
    pub log: Vec<String>,
}

struct Frame {
    var_tys: Vec<Option<Ty>>,
}

struct LoopCtx {
    label: Option<String>,
    next_target: usize,
    exits: Vec<usize>,
}

struct InlineCtx {
    returns: Vec<usize>,
}

pub struct Elaborator<'a> {
    sources: &'a [Source],
    dir: Option<std::path::PathBuf>,
    scopes: Vec<HashMap<String, Bind>>,
    signals: Vec<SignalDef>,
    procs: Vec<ProcDef>,
    funcs: Vec<Func>,
    bridges: Vec<BridgeDef>,
    frames: Vec<Frame>,
    ops: Vec<Op>,
    loops: Vec<LoopCtx>,
    inlines: Vec<InlineCtx>,
    file: u16,
    hier: String,
    packages_done: HashMap<String, HashMap<String, Bind>>,
    inline_depth: u32,
    log: Vec<String>,
    in_function: bool,
}

type R<T> = Result<T, TbError>;

fn std_types() -> HashMap<String, Bind> {
    let mut m = HashMap::new();
    let arr = |name: &str, kind: ArrKind, elem: Ty| {
        Ty::Array(Rc::new(ArrTy { name: name.into(), kind, elem, range: None }))
    };
    let int = Ty::Int { lo: i32::MIN as i64, hi: i32::MAX as i64 };
    for (n, t) in [
        ("std_logic", Ty::Logic),
        ("std_ulogic", Ty::Logic),
        ("bit", Ty::Bit),
        ("boolean", Ty::Bool),
        ("character", Ty::Char),
        ("integer", int.clone()),
        ("natural", Ty::Int { lo: 0, hi: i32::MAX as i64 }),
        ("positive", Ty::Int { lo: 1, hi: i32::MAX as i64 }),
        ("real", Ty::Real),
        ("time", Ty::Time),
        ("delay_length", Ty::Time),
        ("line", Ty::Line),
        ("text", Ty::File),
        ("severity_level", Ty::Severity),
        ("std_logic_vector", arr("std_logic_vector", ArrKind::Slv, Ty::Logic)),
        ("std_ulogic_vector", arr("std_ulogic_vector", ArrKind::Slv, Ty::Logic)),
        ("bit_vector", arr("bit_vector", ArrKind::Slv, Ty::Bit)),
        ("unsigned", arr("unsigned", ArrKind::Unsigned, Ty::Logic)),
        ("signed", arr("signed", ArrKind::Signed, Ty::Logic)),
        ("u_unsigned", arr("unsigned", ArrKind::Unsigned, Ty::Logic)),
        ("u_signed", arr("signed", ArrKind::Signed, Ty::Logic)),
        ("string", arr("string", ArrKind::Str, Ty::Char)),
        ("integer_vector", arr("integer_vector", ArrKind::Other, int)),
    ] {
        m.insert(n.to_string(), Bind::Type(t));
    }
    m.insert("true".into(), Bind::Const(Value::Bool(true), Ty::Bool));
    m.insert("false".into(), Bind::Const(Value::Bool(false), Ty::Bool));
    for (i, s) in ["note", "warning", "error", "failure"].iter().enumerate() {
        m.insert(s.to_string(), Bind::Const(Value::Enum(i as u32), Ty::Severity));
    }
    m.insert("output".into(), Bind::Const(Value::Int(1), Ty::File));
    m.insert("input".into(), Bind::Const(Value::Int(0), Ty::File));
    for u in ["ieee", "std", "work", "env", "textio", "std_logic_1164", "numeric_std", "math_real", "standard"] {
        m.insert(u.into(), Bind::Unit);
    }
    m
}

fn builtin_fn(name: &str) -> Option<Bi> {
    Some(match name {
        "rising_edge" => Bi::RisingEdge,
        "falling_edge" => Bi::FallingEdge,
        "to_unsigned" => Bi::ToUnsigned,
        "to_signed" => Bi::ToSigned,
        "to_integer" | "conv_integer" => Bi::ToInteger,
        "resize" => Bi::Resize,
        "shift_left" => Bi::ShiftLeft,
        "shift_right" => Bi::ShiftRight,
        "rotate_left" => Bi::RotateLeft,
        "rotate_right" => Bi::RotateRight,
        "to_string" => Bi::ToString,
        "to_hstring" | "to_hex_string" => Bi::ToHString,
        "to_bstring" | "to_binary_string" => Bi::ToBString,
        "std_match" => Bi::StdMatch,
        "is_x" => Bi::IsX,
        "to_x01" => Bi::ToX01,
        "minimum" => Bi::Minimum,
        "maximum" => Bi::Maximum,
        "floor" => Bi::Floor,
        "round" => Bi::Round,
        "ceil" => Bi::Ceil,
        "sqrt" => Bi::Sqrt,
        "now" => Bi::Now,
        "to_slv" | "to_stdlogicvector" | "to_std_logic_vector" => Bi::ToSlv,
        _ => return None,
    })
}

fn bi_ret(b: Bi, args: &[Option<Ty>]) -> Option<Ty> {
    let arr = |name: &str, kind| Ty::Array(Rc::new(ArrTy { name: name.into(), kind, elem: Ty::Logic, range: None }));
    match b {
        Bi::RisingEdge | Bi::FallingEdge | Bi::StdMatch | Bi::IsX => Some(Ty::Bool),
        Bi::ToUnsigned => Some(arr("unsigned", ArrKind::Unsigned)),
        Bi::ToSigned => Some(arr("signed", ArrKind::Signed)),
        Bi::ToInteger | Bi::ToIntegerFromReal => Some(Ty::Int { lo: i32::MIN as i64, hi: i32::MAX as i64 }),
        Bi::ToString | Bi::ToHString | Bi::ToBString => Some(arr("string", ArrKind::Str)),
        Bi::Now => Some(Ty::Time),
        Bi::ToReal | Bi::Floor | Bi::Round | Bi::Ceil | Bi::Sqrt => Some(Ty::Real),
        Bi::ToSlv => Some(arr("std_logic_vector", ArrKind::Slv)),
        _ => args.first().cloned().flatten().map(|t| match t {
            Ty::Array(a) => Ty::Array(Rc::new(ArrTy { name: a.name.clone(), kind: a.kind, elem: a.elem.clone(), range: None })),
            other => other,
        }),
    }
}

/// Signals an expression reads (for implicit sensitivity).
pub fn signals_read(e: &RExpr, out: &mut Vec<SigId>) {
    let mut push = |s: SigId| {
        if !out.contains(&s) {
            out.push(s);
        }
    };
    match e {
        RExpr::Sig(s) | RExpr::Event(s) | RExpr::LastValue(s) | RExpr::LastEvent(s) => push(*s),
        RExpr::Index(a, b) | RExpr::Bin(_, a, b) => {
            signals_read(a, out);
            signals_read(b, out);
        }
        RExpr::Slice(a, b, _, c) => {
            signals_read(a, out);
            signals_read(b, out);
            signals_read(c, out);
        }
        RExpr::Field(a, _) | RExpr::Un(_, a) | RExpr::Conv(_, a) | RExpr::Image(_, a) | RExpr::ValueOf(_, a) | RExpr::Pos(a) | RExpr::Val(_, a) | RExpr::ArrAttr(_, a) => {
            signals_read(a, out)
        }
        RExpr::Call(_, args) | RExpr::Bi(_, args) | RExpr::RecAgg(args) => {
            for a in args {
                signals_read(a, out);
            }
        }
        RExpr::ArrAgg { pos, named, others, .. } => {
            for a in pos {
                signals_read(a, out);
            }
            for (_, a) in named {
                signals_read(a, out);
            }
            if let Some(o) = others {
                signals_read(o, out);
            }
        }
        RExpr::Const(_) | RExpr::Var(_) => {}
    }
}

/// Compile-time host: signals at their initial values, no events.
struct ConstHost<'a> {
    signals: &'a [SignalDef],
}

impl Host for ConstHost<'_> {
    fn sig(&self, id: SigId) -> &Value {
        &self.signals[id].init
    }
    fn event(&self, _: SigId) -> bool {
        false
    }
    fn last_value(&self, id: SigId) -> &Value {
        &self.signals[id].init
    }
    fn last_event(&self, _: SigId) -> Option<i64> {
        None
    }
    fn now(&self) -> i64 {
        0
    }
    fn report(&mut self, _: u32, _: String, _: Loc) {}
}

impl<'a> Elaborator<'a> {
    pub fn new(sources: &'a [Source], dir: Option<std::path::PathBuf>) -> Self {
        Self {
            sources,
            dir,
            scopes: vec![std_types()],
            signals: Vec::new(),
            procs: Vec::new(),
            funcs: Vec::new(),
            bridges: Vec::new(),
            frames: Vec::new(),
            ops: Vec::new(),
            loops: Vec::new(),
            inlines: Vec::new(),
            file: 0,
            hier: String::new(),
            packages_done: HashMap::new(),
            inline_depth: 0,
            log: Vec::new(),
            in_function: false,
        }
    }

    fn err<T>(&self, line: usize, msg: impl Into<String>) -> R<T> {
        Err(TbError::new(&self.sources[self.file as usize].label, line, msg))
    }

    fn loc(&self, line: usize) -> Loc {
        Loc { file: self.file, line: line as u32 }
    }

    fn lookup(&self, name: &str) -> Option<&Bind> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    fn bind(&mut self, name: &str, b: Bind) {
        self.scopes.last_mut().expect("scope").insert(name.to_string(), b);
    }

    /// Evaluate a constant expression at elaboration time.
    fn const_eval(&mut self, e: &RExpr, line: usize) -> R<Value> {
        let mut host = ConstHost { signals: &self.signals };
        let mut m = Mach { funcs: &self.funcs, host: &mut host, depth: 0 };
        let mut vars = Vec::new();
        m.eval(e, &mut vars).map_err(|msg| TbError::new(&self.sources[self.file as usize].label, line, msg))
    }

    fn const_int(&mut self, e: &Expr, line: usize) -> R<i64> {
        let r = self.expr(e, None, line)?;
        let v = self.const_eval(&r, line)?;
        v.as_int().or_else(|| if let Value::Ch(c) = v { Some(c as i64) } else { None }).ok_or_else(|| {
            TbError::new(&self.sources[self.file as usize].label, line, "expected a constant integer")
        })
    }

    // ------------------------------------------------------------------
    // Types
    // ------------------------------------------------------------------

    fn subtype(&mut self, s: &SubtypeInd) -> R<Ty> {
        let base = match self.lookup(&s.mark) {
            Some(Bind::Type(t)) => t.clone(),
            _ => return self.err(s.line, format!("unknown type `{}`", s.mark)),
        };
        let Some((l, asc, r)) = &s.range else {
            return Ok(base);
        };
        // `range x'range` style.
        if let Expr::Attr(..) = l {
            let re = self.expr(l, None, s.line)?;
            if let RExpr::Const(Value::Arr(a)) = re {
                if let Ty::Array(at) = &base {
                    return Ok(Ty::Array(Rc::new(ArrTy { name: at.name.clone(), kind: at.kind, elem: at.elem.clone(), range: Some((a.left, a.right(), a.asc)) })));
                }
            }
        }
        let lo = self.const_int(l, s.line)?;
        let hi = self.const_int(r, s.line)?;
        Ok(match base {
            Ty::Array(at) => {
                let mut elem = at.elem.clone();
                if let Some((el, easc, er)) = &s.elem_range {
                    let a = self.const_int(el, s.line)?;
                    let b = self.const_int(er, s.line)?;
                    if let Ty::Array(e) = &elem {
                        elem = Ty::Array(Rc::new(ArrTy { name: e.name.clone(), kind: e.kind, elem: e.elem.clone(), range: Some((a, b, *easc)) }));
                    }
                }
                Ty::Array(Rc::new(ArrTy { name: at.name.clone(), kind: at.kind, elem, range: Some((lo, hi, *asc)) }))
            }
            Ty::Int { .. } => Ty::Int { lo: lo.min(hi), hi: lo.max(hi) },
            other => other,
        })
    }

    fn type_decl(&mut self, name: &str, def: &TypeDef, line: usize) -> R<()> {
        let ty = match def {
            TypeDef::Enum(lits) => {
                let et = Rc::new(EnumTy { name: name.into(), lits: lits.clone() });
                let ty = Ty::Enum(et);
                for (i, l) in lits.iter().enumerate() {
                    if !l.starts_with('\'') {
                        self.bind(l, Bind::EnumLit(ty.clone(), i as u32));
                    }
                }
                ty
            }
            TypeDef::Int(l, _, r) => {
                let a = self.const_int(l, line)?;
                let b = self.const_int(r, line)?;
                Ty::Int { lo: a.min(b), hi: a.max(b) }
            }
            TypeDef::Real => Ty::Real,
            TypeDef::Array { range, index_mark, elem } => {
                let elem = self.subtype(elem)?;
                let kind = match &elem {
                    Ty::Logic | Ty::Bit => ArrKind::Slv,
                    Ty::Char => ArrKind::Str,
                    _ => ArrKind::Other,
                };
                let range = match (range, index_mark) {
                    (Some((l, asc, r)), _) => Some((self.const_int(l, line)?, self.const_int(r, line)?, *asc)),
                    (None, Some(m)) => match self.lookup(m) {
                        Some(Bind::Type(Ty::Enum(e))) => Some((0, e.lits.len() as i64 - 1, true)),
                        _ => None,
                    },
                    _ => None,
                };
                Ty::Array(Rc::new(ArrTy { name: name.into(), kind, elem, range }))
            }
            TypeDef::Record(fields) => {
                let mut fs = Vec::new();
                for (n, st) in fields {
                    fs.push((n.clone(), self.subtype(st)?));
                }
                Ty::Record(Rc::new(RecTy { name: name.into(), fields: fs }))
            }
            TypeDef::Opaque => Ty::Line,
        };
        self.bind(name, Bind::Type(ty));
        Ok(())
    }

    // ------------------------------------------------------------------
    // Declarations
    // ------------------------------------------------------------------

    /// Declarative region. `frame` is the process/function frame for
    /// variables (None at architecture/package level).
    fn decls(&mut self, decls: &[Decl], in_frame: bool) -> R<()> {
        for d in decls {
            match d {
                Decl::Object { class, names, ty, init, line } => {
                    let t = self.subtype(ty)?;
                    match class {
                        ObjClass::Constant => {
                            let Some(init) = init else {
                                // Deferred constant: value comes from the package body.
                                continue;
                            };
                            let re = self.expr(init, Some(&t), *line)?;
                            let v = self.const_eval(&re, *line)?;
                            let v = super::eval::coerce(v, &self.constant_ty(&t, &re, *line)?).or_else(|e| self.err(*line, e))?;
                            let vt = const_ty_from(&t, &v);
                            for n in names {
                                self.bind(n, Bind::Const(v.clone(), vt.clone()));
                            }
                        }
                        ObjClass::Signal => {
                            let v = match init {
                                Some(e) => {
                                    let re = self.expr(e, Some(&t), *line)?;
                                    let v = self.const_eval(&re, *line)?;
                                    super::eval::coerce(v, &t).or_else(|e| self.err(*line, e))?
                                }
                                None => t.default_value(),
                            };
                            for n in names {
                                let id = self.signals.len();
                                self.signals.push(SignalDef { name: format!("{}{n}", self.hier), ty: t.clone(), init: v.clone() });
                                self.bind(n, Bind::Sig(id));
                            }
                        }
                        ObjClass::Variable => {
                            if !in_frame {
                                return self.err(*line, "shared variables are not supported");
                            }
                            for n in names {
                                let slot = self.new_var(Some(t.clone()));
                                let frame = self.frames.len() - 1;
                                self.bind(n, Bind::Var { frame, slot, ty: t.clone() });
                                let value = match init {
                                    Some(e) => self.expr(e, Some(&t), *line)?,
                                    None => RExpr::Const(t.default_value()),
                                };
                                let loc = self.loc(*line);
                                self.ops.push(Op::VarAssign { target: RTarget::Var(slot, Vec::new()), value, ty: Some(t.clone()), loc });
                            }
                        }
                        ObjClass::File => {
                            for n in names {
                                self.bind(n, Bind::Const(Value::Int(1), Ty::File));
                            }
                        }
                    }
                }
                Decl::Type { name, def, line } => self.type_decl(name, def, *line)?,
                Decl::Subtype { name, ty, .. } => {
                    let t = self.subtype(ty)?;
                    self.bind(name, Bind::Type(t));
                }
                Decl::Subprogram(sp) => self.declare_sub(sp.clone()),
                Decl::Alias { name, target, line } => {
                    let read = self.expr(target, None, *line)?;
                    let write = self.target(target, *line).ok();
                    let ty = self.ty_of(&read);
                    self.bind(name, Bind::Alias { read, write, ty });
                }
                Decl::Ignored => {}
            }
        }
        Ok(())
    }

    /// Unconstrained constant types take their value's bounds.
    fn constant_ty(&self, t: &Ty, _re: &RExpr, _line: usize) -> R<Ty> {
        Ok(match t {
            Ty::Array(a) if a.range.is_none() => Ty::Array(a.clone()),
            other => other.clone(),
        })
    }

    fn declare_sub(&mut self, sp: Box<Subprogram>) {
        let name = sp.name.clone();
        let def = SubDef { ast: Rc::from(sp), file: self.file, compiled: None };
        if let Some(Bind::Subs(list)) = self.scopes.last().and_then(|s| s.get(&name)) {
            let mut l = list.borrow_mut();
            // A body replaces its earlier declaration.
            if def.ast.body.is_some() {
                if let Some(slot) = l.iter_mut().find(|d| d.ast.body.is_none() && d.ast.params.len() == def.ast.params.len()) {
                    *slot = def;
                    return;
                }
            }
            l.push(def);
            return;
        }
        // Overloads visible from outer scopes stay visible.
        let mut list = match self.lookup(&name) {
            Some(Bind::Subs(outer)) => outer.borrow().clone(),
            _ => Vec::new(),
        };
        list.push(def);
        self.bind(&name, Bind::Subs(Rc::new(RefCell::new(list))));
    }

    fn new_var(&mut self, ty: Option<Ty>) -> usize {
        let f = self.frames.last_mut().expect("frame");
        f.var_tys.push(ty);
        f.var_tys.len() - 1
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    pub fn ty_of(&self, e: &RExpr) -> Option<Ty> {
        match e {
            RExpr::Const(v) => Some(match v {
                Value::Int(_) => Ty::Int { lo: i32::MIN as i64, hi: i32::MAX as i64 },
                Value::Real(_) => Ty::Real,
                Value::Time(_) => Ty::Time,
                Value::Bool(_) => Ty::Bool,
                Value::Ch(_) => Ty::Logic,
                Value::Arr(a) => Ty::Array(Rc::new(ArrTy {
                    name: "array".into(),
                    kind: a.kind,
                    elem: if a.kind == ArrKind::Str { Ty::Char } else { Ty::Logic },
                    range: Some((a.left, a.right(), a.asc)),
                })),
                _ => return None,
            }),
            RExpr::Sig(s) => Some(self.signals[*s].ty.clone()),
            RExpr::Var(s) => self.frames.last().and_then(|f| f.var_tys.get(*s).cloned().flatten()),
            RExpr::Index(a, _) => match self.ty_of(a)? {
                Ty::Array(at) => Some(at.elem.clone()),
                _ => None,
            },
            RExpr::Slice(a, ..) => match self.ty_of(a)? {
                Ty::Array(at) => Some(Ty::Array(Rc::new(ArrTy { name: at.name.clone(), kind: at.kind, elem: at.elem.clone(), range: None }))),
                other => Some(other),
            },
            RExpr::Field(r, f) => match self.ty_of(r)? {
                Ty::Record(rt) => rt.fields.get(*f).map(|(_, t)| t.clone()),
                _ => None,
            },
            RExpr::Call(f, _) => self.funcs.get(*f).and_then(|f| f.ret.clone()),
            RExpr::Bi(b, args) => bi_ret(*b, &args.iter().map(|a| self.ty_of(a)).collect::<Vec<_>>()),
            RExpr::Event(_) => Some(Ty::Bool),
            RExpr::LastValue(s) => Some(self.signals[*s].ty.clone()),
            RExpr::LastEvent(_) => Some(Ty::Time),
            RExpr::ArrAgg { ty, .. } => ty.clone(),
            RExpr::RecAgg(_) => None,
            RExpr::Conv(t, _) | RExpr::Val(t, _) => Some(t.clone()),
            RExpr::Image(..) => Some(Ty::Array(Rc::new(ArrTy { name: "string".into(), kind: ArrKind::Str, elem: Ty::Char, range: None }))),
            RExpr::ValueOf(t, _) => Some(t.clone()),
            RExpr::Pos(_) | RExpr::ArrAttr(..) => Some(Ty::Int { lo: i32::MIN as i64, hi: i32::MAX as i64 }),
            RExpr::Un(_, a) => self.ty_of(a),
            RExpr::Bin(op, a, b) => match op {
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Some(Ty::Bool),
                _ => {
                    let ta = self.ty_of(a);
                    match (&ta, op) {
                        (Some(Ty::Array(_)), _) => ta,
                        _ => self.ty_of(b).filter(|t| matches!(t, Ty::Array(_))).or(ta),
                    }
                }
            },
        }
    }

    /// Resolve an expression; `want` is the expected type (aggregates,
    /// string literals, overloads).
    pub fn expr(&mut self, e: &Expr, want: Option<&Ty>, line: usize) -> R<RExpr> {
        Ok(match e {
            Expr::Int(i) => RExpr::Const(match want {
                Some(Ty::Real) => Value::Real(*i as f64),
                _ => Value::Int(*i),
            }),
            Expr::Real(r) => RExpr::Const(Value::Real(*r)),
            Expr::Time(t) => RExpr::Const(Value::Time(*t)),
            Expr::Char(c) => match want {
                Some(Ty::Enum(et)) => {
                    let lit = format!("'{}'", *c as char);
                    match et.lits.iter().position(|l| *l == lit) {
                        Some(p) => RExpr::Const(Value::Enum(p as u32)),
                        None => return self.err(line, format!("{lit} is not a {} literal", et.name)),
                    }
                }
                _ => RExpr::Const(Value::Ch(*c)),
            },
            Expr::Str(s) | Expr::BitStr(s) => {
                let kind = match want {
                    Some(Ty::Array(a)) => a.kind,
                    Some(Ty::Line) => ArrKind::Str,
                    _ if matches!(e, Expr::BitStr(_)) => ArrKind::Slv,
                    _ => ArrKind::Str,
                };
                let v = if kind == ArrKind::Str { Value::str(s) } else { Value::vec(kind, s.as_bytes()) };
                RExpr::Const(v)
            }
            Expr::Name(n) => self.name(n, want, line)?,
            Expr::Select(prefix, field) => self.select(prefix, field, want, line)?,
            Expr::Call(prefix, args) => self.call_expr(prefix, args, want, line)?,
            Expr::Attr(prefix, attr, args) => self.attr(prefix, attr, args, line)?,
            Expr::Qualified(mark, inner) => {
                let ty = match self.lookup(mark) {
                    Some(Bind::Type(t)) => t.clone(),
                    _ => return self.err(line, format!("unknown type `{mark}` in a qualified expression")),
                };
                let x = self.expr(inner, Some(&ty), line)?;
                RExpr::Conv(ty, Box::new(x))
            }
            Expr::Unary(op, a) => {
                let x = self.expr(a, want, line)?;
                RExpr::Un(*op, Box::new(x))
            }
            Expr::Binary(op, a, b) => {
                let (wa, wb) = match op {
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => (None, None),
                    BinOp::Concat => (None, None),
                    _ => (want, want),
                };
                // Resolve the side that fixes the type first.
                let lhs_needs_ctx = matches!(**a, Expr::Aggregate(_) | Expr::Str(_) | Expr::BitStr(_) | Expr::Char(_));
                let (x, y) = if lhs_needs_ctx {
                    let y = self.expr(b, wb, line)?;
                    let ty = self.ty_of(&y);
                    let x = self.expr(a, ty.as_ref().or(wa), line)?;
                    (x, y)
                } else {
                    let x = self.expr(a, wa, line)?;
                    let ty = self.ty_of(&x);
                    let ctx = match op {
                        BinOp::Sll | BinOp::Srl | BinOp::Sla | BinOp::Sra | BinOp::Rol | BinOp::Ror | BinOp::Pow => None,
                        BinOp::Concat => None,
                        BinOp::Mul | BinOp::Div if matches!(ty, Some(Ty::Time)) => None,
                        _ => ty.as_ref().or(wb).cloned(),
                    };
                    let y = self.expr(b, ctx.as_ref(), line)?;
                    (x, y)
                };
                fold(RExpr::Bin(*op, Box::new(x), Box::new(y)), &self.funcs)
            }
            Expr::Aggregate(assocs) => self.aggregate(assocs, want, line)?,
            Expr::Range(..) => return self.err(line, "a range is not an expression here"),
            Expr::Open => return self.err(line, "`open` is only valid in a port map"),
        })
    }

    fn aggregate(&mut self, assocs: &[Assoc], want: Option<&Ty>, line: usize) -> R<RExpr> {
        if let Some(Ty::Record(rt)) = want {
            let rt = rt.clone();
            let mut fields: Vec<Option<RExpr>> = vec![None; rt.fields.len()];
            let mut others = None;
            for (k, a) in assocs.iter().enumerate() {
                if a.choices.is_empty() {
                    let fty = rt.fields.get(k).map(|f| f.1.clone());
                    fields[k] = Some(self.expr(&a.value, fty.as_ref(), line)?);
                    continue;
                }
                for c in &a.choices {
                    match c {
                        Choice::Expr(Expr::Name(n)) => {
                            let Some(i) = rt.fields.iter().position(|f| &f.0 == n) else {
                                return self.err(line, format!("record {} has no field `{n}`", rt.name));
                            };
                            let fty = rt.fields[i].1.clone();
                            fields[i] = Some(self.expr(&a.value, Some(&fty), line)?);
                        }
                        Choice::Others => others = Some(a.value.clone()),
                        _ => return self.err(line, "bad record aggregate choice"),
                    }
                }
            }
            let mut out = Vec::new();
            for (i, f) in fields.into_iter().enumerate() {
                out.push(match (f, &others) {
                    (Some(x), _) => x,
                    (None, Some(o)) => {
                        let fty = rt.fields[i].1.clone();
                        self.expr(o, Some(&fty), line)?
                    }
                    (None, None) => return self.err(line, format!("record aggregate misses field `{}`", rt.fields[i].0)),
                });
            }
            return Ok(RExpr::RecAgg(out));
        }
        let elem_ty = match want {
            Some(Ty::Array(a)) => Some(a.elem.clone()),
            _ => None,
        };
        let mut pos = Vec::new();
        let mut named = Vec::new();
        let mut others = None;
        for a in assocs {
            let v = self.expr(&a.value, elem_ty.as_ref(), line)?;
            if a.choices.is_empty() {
                pos.push(v);
                continue;
            }
            let mut cs = Vec::new();
            for c in &a.choices {
                match c {
                    Choice::Others => others = Some(Box::new(v.clone())),
                    Choice::Expr(x) => cs.push(RChoice::Val(self.expr(x, None, line)?)),
                    Choice::Range(l, asc, r) => cs.push(RChoice::Range(self.expr(l, None, line)?, *asc, self.expr(r, None, line)?)),
                }
            }
            if !cs.is_empty() {
                named.push((cs, v));
            }
        }
        Ok(RExpr::ArrAgg { ty: want.filter(|t| matches!(t, Ty::Array(_))).cloned(), pos, named, others })
    }

    fn name(&mut self, n: &str, want: Option<&Ty>, line: usize) -> R<RExpr> {
        let Some(b) = self.lookup(n).cloned() else {
            if let Some(bi) = builtin_fn(n) {
                if bi == Bi::Now {
                    return Ok(RExpr::Bi(Bi::Now, Vec::new()));
                }
            }
            return self.err(line, format!("unknown name `{n}`"));
        };
        Ok(match b {
            Bind::Sig(s) => RExpr::Sig(s),
            Bind::Var { frame, slot, .. } => {
                if frame + 1 != self.frames.len() {
                    return self.err(line, format!("`{n}` is a variable of an enclosing process (not visible in a function here)"));
                }
                RExpr::Var(slot)
            }
            Bind::Const(v, _) => RExpr::Const(v),
            Bind::EnumLit(t, p) => {
                // An enumeration literal may also name a same-named function.
                let _ = t;
                RExpr::Const(Value::Enum(p))
            }
            Bind::Alias { read, .. } => read,
            Bind::Subs(list) => {
                // Parameterless function call.
                let f = self.pick_sub(&list, &[], true, want, line)?;
                RExpr::Call(f, Vec::new())
            }
            Bind::Type(_) => return self.err(line, format!("type `{n}` used as a value")),
            Bind::Unit => return self.err(line, format!("library/package `{n}` used as a value")),
        })
    }

    fn select(&mut self, prefix: &Expr, field: &str, want: Option<&Ty>, line: usize) -> R<RExpr> {
        // Expanded names: `std.env.stop`, `ieee.numeric_std.to_unsigned`, `work.pkg.x`.
        if is_unit_path(prefix, &|n| matches!(self.lookup(n), Some(Bind::Unit)) || self.packages_done.contains_key(n)) {
            if let Some(Bind::Unit) | None = self.lookup(field) {
                if let Some(pkg) = last_name(prefix) {
                    if let Some(scope) = self.packages_done.get(&pkg) {
                        if let Some(b) = scope.get(field).cloned() {
                            self.scopes.push(HashMap::from([(field.to_string(), b)]));
                            let r = self.name(field, want, line);
                            self.scopes.pop();
                            return r;
                        }
                    }
                }
            }
            return self.name(field, want, line);
        }
        let base = self.expr(prefix, None, line)?;
        match self.ty_of(&base) {
            Some(Ty::Record(rt)) => {
                let Some(i) = rt.fields.iter().position(|f| f.0 == field) else {
                    return self.err(line, format!("record {} has no field `{field}`", rt.name));
                };
                Ok(RExpr::Field(Box::new(base), i))
            }
            _ => self.err(line, format!("`.{field}`: prefix is not a record")),
        }
    }

    fn pick_sub(&mut self, list: &Rc<RefCell<Vec<SubDef>>>, args: &[Option<Ty>], function: bool, _want: Option<&Ty>, line: usize) -> R<usize> {
        let cands: Vec<(usize, SubDef)> = list
            .borrow()
            .iter()
            .cloned()
            .enumerate()
            .filter(|(_, d)| d.ast.is_function == function && d.ast.params.len() >= args.len() && d.ast.params.iter().skip(args.len()).all(|p| p.default.is_some()))
            .collect();
        if cands.is_empty() {
            let name = list.borrow().first().map(|d| d.ast.name.clone()).unwrap_or_default();
            return self.err(line, format!("no `{name}` takes {} argument(s)", args.len()));
        }
        // Prefer the overload whose parameter type families match.
        let score = |d: &SubDef, me: &mut Self| -> usize {
            let mut s = 0;
            for (k, a) in args.iter().enumerate() {
                let Some(p) = d.ast.params.get(k) else { continue };
                let pt = me.subtype(&p.ty).ok();
                if same_family(pt.as_ref(), a.as_ref()) {
                    s += 1;
                }
            }
            s
        };
        let mut best = cands[0].0;
        let mut best_s = 0;
        for (i, d) in &cands {
            let s = score(d, self);
            if s > best_s {
                best_s = s;
                best = *i;
            }
        }
        self.compile_function(list, best, line)
    }

    /// Compile function `list[i]` (once) and return its id.
    fn compile_function(&mut self, list: &Rc<RefCell<Vec<SubDef>>>, i: usize, line: usize) -> R<usize> {
        if let Some(id) = list.borrow()[i].compiled {
            return Ok(id);
        }
        let def = list.borrow()[i].clone();
        let sp = def.ast.clone();
        let Some(body) = &sp.body else {
            return self.err(line, format!("function `{}` has no body", sp.name));
        };
        let id = self.funcs.len();
        // Placeholder so recursive calls resolve.
        self.funcs.push(Func { name: sp.name.clone(), nparams: sp.params.len(), param_tys: Vec::new(), ret: None, code: Rc::new(Code::default()) });
        list.borrow_mut()[i].compiled = Some(id);
        let saved_ops = std::mem::take(&mut self.ops);
        let saved_loops = std::mem::take(&mut self.loops);
        let saved_file = self.file;
        let saved_inf = self.in_function;
        self.file = def.file;
        self.in_function = true;
        self.frames.push(Frame { var_tys: Vec::new() });
        self.scopes.push(HashMap::new());
        let mut param_tys = Vec::new();
        for p in &sp.params {
            let t = self.subtype(&p.ty)?;
            let slot = self.new_var(Some(t.clone()));
            let frame = self.frames.len() - 1;
            self.bind(&p.name, Bind::Var { frame, slot, ty: t.clone() });
            param_tys.push(Some(t));
        }
        let ret = match &sp.ret {
            Some(r) => Some(self.subtype(r)?),
            None => None,
        };
        self.funcs[id].param_tys = param_tys.clone();
        self.funcs[id].ret = ret.clone();
        let r = (|| -> R<()> {
            self.decls(&sp.decls, true)?;
            self.stmts(body)?;
            Ok(())
        })();
        self.scopes.pop();
        let frame = self.frames.pop().expect("frame");
        let ops = std::mem::replace(&mut self.ops, saved_ops);
        self.loops = saved_loops;
        self.file = saved_file;
        self.in_function = saved_inf;
        r?;
        self.funcs[id] = Func {
            name: sp.name.clone(),
            nparams: sp.params.len(),
            param_tys,
            ret,
            code: Rc::new(Code { ops, body_start: 0, var_tys: frame.var_tys }),
        };
        Ok(id)
    }

    fn call_expr(&mut self, prefix: &Expr, args: &[Arg], want: Option<&Ty>, line: usize) -> R<RExpr> {
        let pname = match prefix {
            Expr::Name(n) => Some(n.clone()),
            Expr::Select(p, f) if is_unit_path(p, &|n| matches!(self.lookup(n), Some(Bind::Unit)) || self.packages_done.contains_key(n)) => Some(f.clone()),
            _ => None,
        };
        if let Some(n) = &pname {
            match self.lookup(n).cloned() {
                Some(Bind::Type(t)) => {
                    // Type conversion.
                    let a = args.first().ok_or_else(|| TbError::new(&self.sources[self.file as usize].label, line, "conversion needs an argument"))?;
                    let x = self.expr(&a.actual, None, line)?;
                    return Ok(match t {
                        Ty::Int { .. } if matches!(self.ty_of(&x), Some(Ty::Real)) => RExpr::Bi(Bi::ToIntegerFromReal, vec![x]),
                        Ty::Real => RExpr::Bi(Bi::ToReal, vec![x]),
                        t => RExpr::Conv(t, Box::new(x)),
                    });
                }
                Some(Bind::Subs(list)) => {
                    let has_fn = list.borrow().iter().any(|d| d.ast.is_function);
                    if has_fn {
                        let mut ras = Vec::new();
                        let mut tys = Vec::new();
                        for a in args {
                            let x = self.expr(&a.actual, None, line)?;
                            tys.push(self.ty_of(&x));
                            ras.push(x);
                        }
                        let f = self.pick_sub(&list, &tys, true, want, line)?;
                        // Re-resolve with parameter types (aggregates/literals).
                        let ptys = self.funcs[f].param_tys.clone();
                        let mut ras2 = Vec::new();
                        for (k, a) in args.iter().enumerate() {
                            ras2.push(self.expr(&a.actual, ptys.get(k).cloned().flatten().as_ref(), line)?);
                        }
                        // Fill defaults.
                        let sp = list.borrow().iter().find(|d| d.compiled == Some(f)).map(|d| d.ast.clone());
                        if let Some(sp) = sp {
                            for p in sp.params.iter().skip(args.len()) {
                                if let Some(d) = &p.default {
                                    ras2.push(self.expr(d, None, line)?);
                                }
                            }
                        }
                        let _ = ras;
                        return Ok(RExpr::Call(f, ras2));
                    }
                }
                Some(Bind::Sig(_)) | Some(Bind::Var { .. }) | Some(Bind::Const(..)) | Some(Bind::Alias { .. }) | None
                    if builtin_fn(n).is_none() || self.lookup(n).is_some() =>
                {
                    // Index / slice of an object.
                    let base = self.expr(prefix, None, line)?;
                    return self.index_or_slice(base, args, line);
                }
                _ => {}
            }
            if let Some(b) = builtin_fn(n) {
                let mut ras = Vec::new();
                for a in args {
                    if b == Bi::RisingEdge || b == Bi::FallingEdge {
                        let x = self.expr(&a.actual, None, line)?;
                        if !matches!(x, RExpr::Sig(_)) {
                            return self.err(line, format!("{n} needs a signal name"));
                        }
                        ras.push(x);
                        continue;
                    }
                    ras.push(self.expr(&a.actual, None, line)?);
                }
                // `resize(x, n)` on an integer is a no-op width change.
                return Ok(fold(RExpr::Bi(b, ras), &self.funcs));
            }
            if n == "real" || n == "integer" {
                let x = self.expr(&args[0].actual, None, line)?;
                return Ok(if n == "real" { RExpr::Bi(Bi::ToReal, vec![x]) } else { RExpr::Bi(Bi::ToIntegerFromReal, vec![x]) });
            }
        }
        let base = self.expr(prefix, None, line)?;
        self.index_or_slice(base, args, line)
    }

    fn index_or_slice(&mut self, base: RExpr, args: &[Arg], line: usize) -> R<RExpr> {
        let mut cur = base;
        for a in args {
            cur = match &a.actual {
                Expr::Range(l, asc, r) => {
                    let l = self.expr(l, None, line)?;
                    let r = self.expr(r, None, line)?;
                    RExpr::Slice(Box::new(cur), Box::new(l), *asc, Box::new(r))
                }
                Expr::Attr(p, at, _) if at == "range" || at == "reverse_range" => {
                    let pr = self.expr(p, None, line)?;
                    let rev = at == "reverse_range";
                    let l = RExpr::ArrAttr(if rev { ArrAttr::Right } else { ArrAttr::Left }, Box::new(pr.clone()));
                    let r = RExpr::ArrAttr(if rev { ArrAttr::Left } else { ArrAttr::Right }, Box::new(pr.clone()));
                    // Direction follows the prefix; resolved at run time via Slice of equal bounds.
                    let asc = match self.ty_of(&pr) {
                        Some(Ty::Array(a)) => a.range.map(|r| r.2 != rev).unwrap_or(true),
                        _ => true,
                    };
                    RExpr::Slice(Box::new(cur), Box::new(l), asc, Box::new(r))
                }
                x => {
                    let elem_idx_ty = None;
                    let i = self.expr(x, elem_idx_ty, line)?;
                    RExpr::Index(Box::new(cur), Box::new(i))
                }
            };
        }
        Ok(cur)
    }

    fn attr(&mut self, prefix: &Expr, attr: &str, args: &[Expr], line: usize) -> R<RExpr> {
        // Type attributes.
        if let Expr::Name(n) = prefix {
            if let Some(Bind::Type(t)) = self.lookup(n).cloned() {
                let arg = |me: &mut Self, w: Option<&Ty>| -> R<RExpr> {
                    let a = args.first().ok_or_else(|| TbError::new(&me.sources[me.file as usize].label, line, format!("'{attr} needs an argument")))?;
                    me.expr(a, w, line)
                };
                return Ok(match attr {
                    "image" => RExpr::Image(t.clone(), Box::new(arg(self, Some(&t))?)),
                    "value" => RExpr::ValueOf(t.clone(), Box::new(arg(self, None)?)),
                    "pos" => RExpr::Pos(Box::new(arg(self, Some(&t))?)),
                    "val" => RExpr::Val(t.clone(), Box::new(arg(self, None)?)),
                    "succ" | "pred" => {
                        let x = arg(self, Some(&t))?;
                        let d = if attr == "succ" { 1 } else { -1 };
                        RExpr::Val(t.clone(), Box::new(RExpr::Bin(BinOp::Add, Box::new(RExpr::Pos(Box::new(x))), Box::new(RExpr::Const(Value::Int(d))))))
                    }
                    "high" | "low" | "left" | "right" | "length" => match &t {
                        Ty::Int { lo, hi } => RExpr::Const(Value::Int(match attr {
                            "high" | "right" => *hi,
                            "length" => hi - lo + 1,
                            _ => *lo,
                        })),
                        Ty::Enum(e) => RExpr::Const(Value::Enum(if matches!(attr, "high" | "right") { e.lits.len() as u32 - 1 } else { 0 })),
                        Ty::Array(a) => match a.range {
                            Some((l, r, asc)) => RExpr::Const(Value::Int(match attr {
                                "high" => l.max(r),
                                "low" => l.min(r),
                                "left" => l,
                                "right" => r,
                                _ => len_of(l, r, asc) as i64,
                            })),
                            None => return self.err(line, format!("'{attr} of an unconstrained array type")),
                        },
                        _ => return self.err(line, format!("'{attr} of type {}", t.name())),
                    },
                    _ => return self.err(line, format!("unsupported type attribute '{attr}")),
                });
            }
        }
        let base = self.expr(prefix, None, line)?;
        let sig = match &base {
            RExpr::Sig(s) => Some(*s),
            _ => None,
        };
        Ok(match attr {
            "event" => RExpr::Event(sig.ok_or_else(|| TbError::new(&self.sources[self.file as usize].label, line, "'event needs a signal"))?),
            "last_value" => RExpr::LastValue(sig.ok_or_else(|| TbError::new(&self.sources[self.file as usize].label, line, "'last_value needs a signal"))?),
            "last_event" => RExpr::LastEvent(sig.ok_or_else(|| TbError::new(&self.sources[self.file as usize].label, line, "'last_event needs a signal"))?),
            "stable" => RExpr::Un(UnOp::Not, Box::new(RExpr::Event(sig.ok_or_else(|| TbError::new(&self.sources[self.file as usize].label, line, "'stable needs a signal"))?))),
            "length" => RExpr::ArrAttr(ArrAttr::Length, Box::new(base)),
            "high" => RExpr::ArrAttr(ArrAttr::High, Box::new(base)),
            "low" => RExpr::ArrAttr(ArrAttr::Low, Box::new(base)),
            "left" => RExpr::ArrAttr(ArrAttr::Left, Box::new(base)),
            "right" => RExpr::ArrAttr(ArrAttr::Right, Box::new(base)),
            "ascending" => RExpr::ArrAttr(ArrAttr::Ascending, Box::new(base)),
            "image" => {
                let ty = self.ty_of(&base);
                RExpr::Image(ty.unwrap_or(Ty::Int { lo: 0, hi: 0 }), Box::new(base))
            }
            _ => return self.err(line, format!("unsupported attribute '{attr}")),
        })
    }

    /// Assignment target.
    pub fn target(&mut self, e: &Expr, line: usize) -> R<RTarget> {
        match e {
            Expr::Aggregate(parts) => {
                let mut out = Vec::new();
                for p in parts {
                    out.push(self.target(&p.value, line)?);
                }
                Ok(RTarget::Agg(out))
            }
            _ => {
                let mut sels = Vec::new();
                let base = self.target_base(e, &mut sels, line)?;
                Ok(match base {
                    TB::Sig(s) => RTarget::Sig(s, sels),
                    TB::Var(v) => RTarget::Var(v, sels),
                    TB::Alias(t) => match t {
                        RTarget::Sig(s, mut pre) => {
                            pre.extend(sels);
                            RTarget::Sig(s, pre)
                        }
                        RTarget::Var(s, mut pre) => {
                            pre.extend(sels);
                            RTarget::Var(s, pre)
                        }
                        agg => agg,
                    },
                })
            }
        }
    }

    fn target_base(&mut self, e: &Expr, sels: &mut Vec<RSel>, line: usize) -> R<TB> {
        match e {
            Expr::Name(n) => match self.lookup(n).cloned() {
                Some(Bind::Sig(s)) => Ok(TB::Sig(s)),
                Some(Bind::Var { frame, slot, .. }) if frame + 1 == self.frames.len() => Ok(TB::Var(slot)),
                Some(Bind::Alias { write: Some(t), .. }) => Ok(TB::Alias(t)),
                _ => self.err(line, format!("`{n}` is not an assignable signal or variable")),
            },
            Expr::Call(p, args) => {
                let b = self.target_base(p, sels, line)?;
                for a in args {
                    sels.push(match &a.actual {
                        Expr::Range(l, asc, r) => RSel::Slice(self.expr(l, None, line)?, *asc, self.expr(r, None, line)?),
                        x => RSel::Index(self.expr(x, None, line)?),
                    });
                }
                Ok(b)
            }
            Expr::Select(p, f) => {
                let b = self.target_base(p, sels, line)?;
                // Field index from the target's static type.
                let base_expr = match &b {
                    TB::Sig(s) => RExpr::Sig(*s),
                    TB::Var(v) => RExpr::Var(*v),
                    TB::Alias(RTarget::Sig(s, _)) => RExpr::Sig(*s),
                    TB::Alias(RTarget::Var(v, _)) => RExpr::Var(*v),
                    TB::Alias(_) => return self.err(line, "field of an aggregate target"),
                };
                let mut ty = self.ty_of(&base_expr);
                for s in sels.iter() {
                    ty = match (s, ty) {
                        (RSel::Index(_), Some(Ty::Array(a))) => Some(a.elem.clone()),
                        (RSel::Slice(..), t) => t,
                        (RSel::Field(i), Some(Ty::Record(r))) => r.fields.get(*i).map(|x| x.1.clone()),
                        _ => None,
                    };
                }
                match ty {
                    Some(Ty::Record(rt)) => {
                        let i = rt.fields.iter().position(|x| &x.0 == f).ok_or_else(|| {
                            TbError::new(&self.sources[self.file as usize].label, line, format!("record {} has no field `{f}`", rt.name))
                        })?;
                        sels.push(RSel::Field(i));
                        Ok(b)
                    }
                    _ => self.err(line, format!("`.{f}`: target is not a record")),
                }
            }
            _ => self.err(line, "unsupported assignment target"),
        }
    }

    fn target_ty(&self, t: &RTarget) -> Option<Ty> {
        let (base, sels) = match t {
            RTarget::Sig(s, sels) => (Some(self.signals[*s].ty.clone()), sels),
            RTarget::Var(v, sels) => (self.frames.last().and_then(|f| f.var_tys.get(*v).cloned().flatten()), sels),
            RTarget::Agg(_) => return None,
        };
        let mut ty = base;
        for s in sels {
            ty = match (s, ty) {
                (RSel::Index(_), Some(Ty::Array(a))) => Some(a.elem.clone()),
                (RSel::Slice(..), Some(Ty::Array(a))) => Some(Ty::Array(Rc::new(ArrTy { name: a.name.clone(), kind: a.kind, elem: a.elem.clone(), range: None }))),
                (RSel::Field(i), Some(Ty::Record(r))) => r.fields.get(*i).map(|x| x.1.clone()),
                _ => None,
            };
        }
        ty
    }

    // ------------------------------------------------------------------
    // Statements
    // ------------------------------------------------------------------

    fn stmts(&mut self, body: &[Stmt]) -> R<()> {
        for s in body {
            self.stmt(s)?;
        }
        Ok(())
    }

    fn here(&self) -> usize {
        self.ops.len()
    }

    fn patch(&mut self, at: usize, to: usize) {
        match &mut self.ops[at] {
            Op::Jump(t) | Op::JumpIfNot(_, t, _) => *t = to,
            Op::ForInit { exit, .. } => *exit = to,
            Op::Case { end, .. } => *end = to,
            _ => {}
        }
    }

    fn stmt(&mut self, s: &Stmt) -> R<()> {
        match s {
            Stmt::Null => {}
            Stmt::SigAssign { target, arms, transport, line } => {
                if self.in_function {
                    return self.err(*line, "signal assignment inside a function");
                }
                let t = self.target(target, *line)?;
                let ty = self.target_ty(&t);
                self.cond_chain(arms, *line, |me, w| {
                    let mut wave = Vec::new();
                    for (v, after) in &w.elems {
                        let rv = me.expr(v, ty.as_ref(), *line)?;
                        let ra = match after {
                            Some(a) => Some(me.expr(a, Some(&Ty::Time), *line)?),
                            None => None,
                        };
                        wave.push((rv, ra));
                    }
                    if !wave.is_empty() {
                        let loc = me.loc(*line);
                        me.ops.push(Op::SigAssign { target: t.clone(), wave, transport: *transport, ty: ty.clone(), loc });
                    }
                    Ok(())
                })?;
            }
            Stmt::VarAssign { target, arms, line } => {
                let t = self.target(target, *line)?;
                let ty = self.target_ty(&t);
                let warms: Vec<(Expr, Option<Expr>)> = arms.clone();
                self.cond_chain_exprs(&warms, *line, |me, v| {
                    let value = me.expr(v, ty.as_ref(), *line)?;
                    let loc = me.loc(*line);
                    let whole = matches!(&t, RTarget::Var(_, s) if s.is_empty());
                    me.ops.push(Op::VarAssign { target: t.clone(), value, ty: if whole { ty.clone() } else { None }, loc });
                    Ok(())
                })?;
            }
            Stmt::If { arms, else_b, line } => {
                let mut ends = Vec::new();
                for (c, body) in arms {
                    let rc = self.cond(c, *line)?;
                    let j = self.here();
                    let loc = self.loc(*line);
                    self.ops.push(Op::JumpIfNot(rc, 0, loc));
                    self.stmts(body)?;
                    ends.push(self.here());
                    self.ops.push(Op::Jump(0));
                    let next = self.here();
                    self.patch(j, next);
                }
                self.stmts(else_b)?;
                let end = self.here();
                for e in ends {
                    self.patch(e, end);
                }
            }
            Stmt::Case { sel, arms, line } => {
                let rs = self.expr(sel, None, *line)?;
                let sty = self.ty_of(&rs);
                let at = self.here();
                let loc = self.loc(*line);
                self.ops.push(Op::Case { sel: rs, arms: Vec::new(), end: 0, loc });
                let mut rarms = Vec::new();
                let mut ends = Vec::new();
                for (choices, body) in arms {
                    let mut cs = Vec::new();
                    for c in choices {
                        cs.push(match c {
                            Choice::Others => RChoice::Others,
                            Choice::Expr(x) => RChoice::Val(self.expr(x, sty.as_ref(), *line)?),
                            Choice::Range(a, asc, b) => RChoice::Range(self.expr(a, sty.as_ref(), *line)?, *asc, self.expr(b, sty.as_ref(), *line)?),
                        });
                    }
                    rarms.push((cs, self.here()));
                    self.stmts(body)?;
                    ends.push(self.here());
                    self.ops.push(Op::Jump(0));
                }
                let end = self.here();
                for e in ends {
                    self.patch(e, end);
                }
                if let Op::Case { arms, end: e, .. } = &mut self.ops[at] {
                    *arms = rarms;
                    *e = end;
                }
            }
            Stmt::Loop { label, kind, body, line } => self.loop_stmt(label, kind, body, *line)?,
            Stmt::Exit { label, cond, line } | Stmt::Next { label, cond, line } => {
                let is_exit = matches!(s, Stmt::Exit { .. });
                let idx = match label {
                    Some(l) => self.loops.iter().rposition(|c| c.label.as_deref() == Some(l.as_str())),
                    None => self.loops.len().checked_sub(1),
                };
                let Some(idx) = idx else {
                    return self.err(*line, "exit/next outside a loop");
                };
                let jump_at = if let Some(c) = cond {
                    // if not c then skip
                    let rc = self.cond(c, *line)?;
                    let skip = self.here();
                    let loc = self.loc(*line);
                    self.ops.push(Op::JumpIfNot(rc, 0, loc));
                    let j = self.here();
                    self.ops.push(Op::Jump(0));
                    let after = self.here();
                    self.patch(skip, after);
                    j
                } else {
                    let j = self.here();
                    self.ops.push(Op::Jump(0));
                    j
                };
                if is_exit {
                    self.loops[idx].exits.push(jump_at);
                } else {
                    let t = self.loops[idx].next_target;
                    if t == usize::MAX {
                        self.loops[idx].exits.push(usize::MAX - jump_at);
                    } else {
                        self.patch(jump_at, t);
                    }
                }
            }
            Stmt::Wait { on, until, timeout, line } => {
                if self.in_function {
                    return self.err(*line, "wait inside a function");
                }
                let mut sens = Vec::new();
                for o in on {
                    match self.expr(o, None, *line)? {
                        RExpr::Sig(s) => sens.push(s),
                        other => signals_read(&other, &mut sens),
                    }
                }
                let ru = match until {
                    Some(c) => {
                        let rc = self.cond(c, *line)?;
                        if on.is_empty() {
                            signals_read(&rc, &mut sens);
                        }
                        Some(rc)
                    }
                    None => None,
                };
                let rt = match timeout {
                    Some(t) => Some(self.expr(t, Some(&Ty::Time), *line)?),
                    None => None,
                };
                let loc = self.loc(*line);
                self.ops.push(Op::Wait { on: sens, until: ru, timeout: rt, loc });
            }
            Stmt::Report { msg, severity, line } => {
                let m = self.expr(msg, Some(&string_ty()), *line)?;
                let sev = match severity {
                    Some(x) => Some(self.expr(x, Some(&Ty::Severity), *line)?),
                    None => None,
                };
                let loc = self.loc(*line);
                self.ops.push(Op::Report { msg: m, sev, loc });
            }
            Stmt::Assert { cond, msg, severity, line } => {
                let c = self.cond(cond, *line)?;
                let m = match msg {
                    Some(x) => Some(self.expr(x, Some(&string_ty()), *line)?),
                    None => None,
                };
                let sev = match severity {
                    Some(x) => Some(self.expr(x, Some(&Ty::Severity), *line)?),
                    None => None,
                };
                let loc = self.loc(*line);
                self.ops.push(Op::Assert { cond: c, msg: m, sev, loc });
            }
            Stmt::Return { value, line } => {
                let loc = self.loc(*line);
                if self.in_function && self.inlines.is_empty() {
                    let ret_ty = None;
                    let v = match value {
                        Some(x) => Some(self.expr(x, ret_ty, *line)?),
                        None => None,
                    };
                    self.ops.push(Op::Return(v, loc));
                } else if let Some(ctx) = self.inlines.last_mut() {
                    let j = self.ops.len();
                    ctx.returns.push(j);
                    self.ops.push(Op::Jump(0));
                } else {
                    return self.err(*line, "`return` outside a subprogram");
                }
            }
            Stmt::Call { name, line } => self.call_stmt(name, *line)?,
        }
        Ok(())
    }

    /// Lower `x when c1 else y when c2 else z` into a jump chain.
    fn cond_chain(&mut self, arms: &[(Waveform, Option<Expr>)], line: usize, mut emit: impl FnMut(&mut Self, &Waveform) -> R<()>) -> R<()> {
        let mut ends = Vec::new();
        for (w, c) in arms {
            match c {
                Some(c) => {
                    let rc = self.cond(c, line)?;
                    let j = self.here();
                    let loc = self.loc(line);
                    self.ops.push(Op::JumpIfNot(rc, 0, loc));
                    emit(self, w)?;
                    ends.push(self.here());
                    self.ops.push(Op::Jump(0));
                    let n = self.here();
                    self.patch(j, n);
                }
                None => {
                    emit(self, w)?;
                    break;
                }
            }
        }
        let end = self.here();
        for e in ends {
            self.patch(e, end);
        }
        Ok(())
    }

    fn cond_chain_exprs(&mut self, arms: &[(Expr, Option<Expr>)], line: usize, mut emit: impl FnMut(&mut Self, &Expr) -> R<()>) -> R<()> {
        let mut ends = Vec::new();
        for (v, c) in arms {
            match c {
                Some(c) => {
                    let rc = self.cond(c, line)?;
                    let j = self.here();
                    let loc = self.loc(line);
                    self.ops.push(Op::JumpIfNot(rc, 0, loc));
                    emit(self, v)?;
                    ends.push(self.here());
                    self.ops.push(Op::Jump(0));
                    let n = self.here();
                    self.patch(j, n);
                }
                None => {
                    emit(self, v)?;
                    break;
                }
            }
        }
        let end = self.here();
        for e in ends {
            self.patch(e, end);
        }
        Ok(())
    }

    /// A condition: boolean, or std_ulogic via the implicit `??` (VHDL-2008).
    fn cond(&mut self, c: &Expr, line: usize) -> R<RExpr> {
        let r = self.expr(c, Some(&Ty::Bool), line)?;
        Ok(match self.ty_of(&r) {
            Some(Ty::Logic) | Some(Ty::Bit) => RExpr::Un(UnOp::Cond, Box::new(r)),
            _ => r,
        })
    }

    fn loop_stmt(&mut self, label: &Option<String>, kind: &LoopKind, body: &[Stmt], line: usize) -> R<()> {
        match kind {
            LoopKind::Forever | LoopKind::While(_) => {
                let head = self.here();
                let mut exit_jump = None;
                if let LoopKind::While(c) = kind {
                    let rc = self.cond(c, line)?;
                    exit_jump = Some(self.here());
                    let loc = self.loc(line);
                    self.ops.push(Op::JumpIfNot(rc, 0, loc));
                }
                self.loops.push(LoopCtx { label: label.clone(), next_target: head, exits: Vec::new() });
                self.stmts(body)?;
                self.ops.push(Op::Jump(head));
                let end = self.here();
                let ctx = self.loops.pop().expect("loop");
                for e in ctx.exits {
                    self.patch(e, end);
                }
                if let Some(j) = exit_jump {
                    self.patch(j, end);
                }
            }
            LoopKind::For(var, range) => {
                let rr = match range {
                    ForRange::Range(a, asc, b) => RRange::Explicit(self.expr(a, None, line)?, *asc, self.expr(b, None, line)?),
                    ForRange::Attr(Expr::Attr(p, at, _)) if at == "range" || at == "reverse_range" => {
                        // `t'range` on a constrained type, or `x'range` on an object.
                        if let Expr::Name(n) = &**p {
                            if let Some(Bind::Type(Ty::Array(a))) = self.lookup(n).cloned() {
                                let (l, r, asc) = a.range.ok_or_else(|| TbError::new(&self.sources[self.file as usize].label, line, "'range of an unconstrained type"))?;
                                let (l, r, asc) = if at == "reverse_range" { (r, l, !asc) } else { (l, r, asc) };
                                RRange::Explicit(RExpr::Const(Value::Int(l)), asc, RExpr::Const(Value::Int(r)))
                            } else {
                                RRange::Of(self.expr(p, None, line)?, at == "reverse_range")
                            }
                        } else {
                            RRange::Of(self.expr(p, None, line)?, at == "reverse_range")
                        }
                    }
                    ForRange::Attr(Expr::Name(t)) => match self.lookup(t).cloned() {
                        Some(Bind::Type(Ty::Int { lo, hi })) => RRange::Explicit(RExpr::Const(Value::Int(lo)), true, RExpr::Const(Value::Int(hi))),
                        Some(Bind::Type(Ty::Enum(e))) => RRange::Explicit(RExpr::Const(Value::Int(0)), true, RExpr::Const(Value::Int(e.lits.len() as i64 - 1))),
                        _ => return self.err(line, format!("`{t}` is not a discrete range")),
                    },
                    ForRange::Attr(_) => return self.err(line, "unsupported for-loop range"),
                };
                // Enumeration loop variables iterate positions.
                let var_ty = match range {
                    ForRange::Attr(Expr::Name(t)) => match self.lookup(t).cloned() {
                        Some(Bind::Type(t @ Ty::Enum(_))) => Some(t),
                        _ => None,
                    },
                    _ => None,
                };
                let slot = self.new_var(Some(Ty::Int { lo: i64::MIN, hi: i64::MAX }));
                let end_slot = self.new_var(None);
                let dir_slot = self.new_var(None);
                self.scopes.push(HashMap::new());
                let frame = self.frames.len() - 1;
                match &var_ty {
                    Some(t) => {
                        // Read the position back as the enumeration value.
                        self.bind(var, Bind::Alias { read: RExpr::Val(t.clone(), Box::new(RExpr::Var(slot))), write: None, ty: Some(t.clone()) });
                    }
                    None => self.bind(var, Bind::Var { frame, slot, ty: Ty::Int { lo: i64::MIN, hi: i64::MAX } }),
                }
                let init = self.here();
                let loc = self.loc(line);
                self.ops.push(Op::ForInit { var: slot, end_slot, dir_slot, range: rr, exit: 0, loc });
                let body_at = self.here();
                self.loops.push(LoopCtx { label: label.clone(), next_target: usize::MAX, exits: Vec::new() });
                self.stmts(body)?;
                let next_at = self.here();
                self.ops.push(Op::ForNext { var: slot, end_slot, dir_slot, body: body_at });
                let end = self.here();
                let ctx = self.loops.pop().expect("loop");
                for e in ctx.exits {
                    if e > usize::MAX / 2 {
                        // `next` recorded before the ForNext existed.
                        self.patch(usize::MAX - e, next_at);
                    } else {
                        self.patch(e, end);
                    }
                }
                self.patch(init, end);
                self.scopes.pop();
            }
        }
        Ok(())
    }

    fn call_stmt(&mut self, name: &Expr, line: usize) -> R<()> {
        let (pname, args): (String, Vec<Arg>) = match name {
            Expr::Name(n) => (n.clone(), Vec::new()),
            Expr::Select(_, f) => (f.clone(), Vec::new()),
            Expr::Call(p, a) => match &**p {
                Expr::Name(n) => (n.clone(), a.clone()),
                Expr::Select(_, f) => (f.clone(), a.clone()),
                _ => return self.err(line, "unsupported procedure call"),
            },
            _ => return self.err(line, "unsupported procedure call"),
        };
        let loc = self.loc(line);
        if let Some(Bind::Subs(list)) = self.lookup(&pname).cloned() {
            let cand = list.borrow().iter().find(|d| !d.ast.is_function && d.ast.params.len() >= args.len()).cloned();
            if let Some(def) = cand {
                return self.inline_procedure(def, &args, line);
            }
        }
        match pname.as_str() {
            "stop" | "finish" => {
                if self.in_function {
                    return self.err(line, "stop inside a function");
                }
                self.ops.push(Op::Builtin { proc_: BuiltinProc::Stop, args: Vec::new(), outs: Vec::new(), loc });
            }
            "write" | "hwrite" | "swrite" | "owrite" => {
                if args.len() < 2 {
                    return self.err(line, "write(L, value) needs two arguments");
                }
                let t = self.target(&args[0].actual, line)?;
                let v = self.expr(&args[1].actual, Some(&string_ty()), line)?;
                let mode = RExpr::Const(Value::Int(if pname == "hwrite" { 1 } else { 0 }));
                self.ops.push(Op::Builtin { proc_: BuiltinProc::Write, args: vec![v, mode], outs: vec![t], loc });
            }
            "writeline" => {
                if self.in_function {
                    return self.err(line, "writeline inside a function");
                }
                let t = self.target(&args.get(1).ok_or_else(|| TbError::new(&self.sources[self.file as usize].label, line, "writeline(file, L)"))?.actual, line)?;
                self.ops.push(Op::Builtin { proc_: BuiltinProc::WriteLine, args: Vec::new(), outs: vec![t], loc });
            }
            "uniform" => {
                if args.len() != 3 {
                    return self.err(line, "uniform(seed1, seed2, x) needs three arguments");
                }
                let outs = args.iter().map(|a| self.target(&a.actual, line)).collect::<R<Vec<_>>>()?;
                self.ops.push(Op::Builtin { proc_: BuiltinProc::Uniform, args: Vec::new(), outs, loc });
            }
            "deallocate" => {}
            "read" | "readline" | "file_open" | "file_close" => {
                return self.err(line, format!("textio `{pname}` (file input) is not supported by the simulator"));
            }
            _ => return self.err(line, format!("unknown procedure `{pname}`")),
        }
        Ok(())
    }

    fn inline_procedure(&mut self, def: SubDef, args: &[Arg], line: usize) -> R<()> {
        if self.inline_depth > 16 {
            return self.err(line, "procedure calls nested deeper than 16 (recursion is not supported)");
        }
        let sp = def.ast.clone();
        let Some(body) = sp.body.clone() else {
            return self.err(line, format!("procedure `{}` has no body", sp.name));
        };
        // Bind parameters (evaluated in the caller's scope).
        let mut binds: Vec<(String, Bind)> = Vec::new();
        let mut inits: Vec<Op> = Vec::new();
        for (k, p) in sp.params.iter().enumerate() {
            let actual = args
                .iter()
                .find(|a| a.formal.as_deref() == Some(p.name.as_str()))
                .or_else(|| args.get(k).filter(|a| a.formal.is_none()))
                .map(|a| a.actual.clone())
                .or_else(|| p.default.clone());
            let Some(actual) = actual else {
                return self.err(line, format!("procedure `{}`: no value for parameter `{}`", sp.name, p.name));
            };
            let pty = self.subtype(&p.ty)?;
            let is_signal = p.class == Some(ObjClass::Signal);
            if is_signal || p.mode != Mode::In {
                let read = self.expr(&actual, Some(&pty), line)?;
                let write = if p.mode != Mode::In { Some(self.target(&actual, line)?) } else { None };
                let ty = self.ty_of(&read).or(Some(pty));
                binds.push((p.name.clone(), Bind::Alias { read, write, ty }));
            } else {
                let value = self.expr(&actual, Some(&pty), line)?;
                let ty = match &pty {
                    Ty::Array(a) if a.range.is_none() => self.ty_of(&value),
                    _ => Some(pty.clone()),
                };
                let slot = self.new_var(ty.clone());
                let frame = self.frames.len() - 1;
                binds.push((p.name.clone(), Bind::Var { frame, slot, ty: ty.clone().unwrap_or(pty.clone()) }));
                let loc = self.loc(line);
                inits.push(Op::VarAssign { target: RTarget::Var(slot, Vec::new()), value, ty: None, loc });
            }
        }
        self.ops.extend(inits);
        let saved_file = self.file;
        let saved_loops = std::mem::take(&mut self.loops);
        self.file = def.file;
        self.scopes.push(binds.into_iter().collect());
        self.inlines.push(InlineCtx { returns: Vec::new() });
        self.inline_depth += 1;
        let saved_fn = self.in_function;
        self.in_function = false;
        let r = (|| -> R<()> {
            self.decls(&sp.decls, true)?;
            self.stmts(&body)
        })();
        self.in_function = saved_fn;
        self.inline_depth -= 1;
        let ctx = self.inlines.pop().expect("inline");
        self.scopes.pop();
        self.loops = saved_loops;
        self.file = saved_file;
        r?;
        let end = self.here();
        for j in ctx.returns {
            self.patch(j, end);
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Design units
    // ------------------------------------------------------------------

    fn find_package(&self, name: &str) -> Option<(u16, Vec<&'a Package>)> {
        let sources: &'a [Source] = self.sources;
        for (i, s) in sources.iter().enumerate() {
            let ps: Vec<&Package> = s.file.packages.iter().filter(|p| p.name == name).collect();
            if !ps.is_empty() {
                return Some((i as u16, ps));
            }
        }
        None
    }

    fn use_package(&mut self, name: &str, line: usize) -> R<()> {
        if !self.packages_done.contains_key(name) {
            let Some((fi, pkgs)) = self.find_package(name) else {
                return self.err(line, format!("package work.{name} not found (put its .vhd next to the testbench)"));
            };
            let saved = self.file;
            let saved_scopes = std::mem::replace(&mut self.scopes, vec![std_types()]);
            self.file = fi;
            self.scopes.push(HashMap::new());
            let decl = pkgs.iter().find(|p| !p.is_body).copied();
            let body = pkgs.iter().find(|p| p.is_body).copied();
            let r = (|| -> R<()> {
                if let Some(d) = decl {
                    self.decls(&d.decls, false)?;
                }
                if let Some(b) = body {
                    self.decls(&b.decls, false)?;
                }
                Ok(())
            })();
            let scope = self.scopes.pop().unwrap_or_default();
            self.scopes = saved_scopes;
            self.file = saved;
            r?;
            self.packages_done.insert(name.to_string(), scope);
        }
        let scope = self.packages_done[name].clone();
        for (k, v) in scope {
            self.bind(&k, v);
        }
        Ok(())
    }

    fn find_entity(&self, name: &str) -> Option<(u16, &'a Entity, Option<&'a Architecture>)> {
        for (i, s) in self.sources.iter().enumerate() {
            if let Some(e) = s.file.entities.iter().find(|e| e.name == name) {
                // Last architecture of the entity in any file wins.
                let arch = self.sources.iter().flat_map(|s| s.file.archs.iter()).filter(|a| a.entity == name).last();
                return Some((i as u16, e, arch));
            }
        }
        None
    }

    /// Elaborate the top entity `name`.
    pub fn elaborate_top(mut self, name: &str) -> R<Elab> {
        let Some((fi, ent, arch)) = self.find_entity(name) else {
            return Err(TbError::new(&self.sources[0].label, 0, format!("entity `{name}` not found")));
        };
        let Some(arch) = arch else {
            return Err(TbError::new(&self.sources[fi as usize].label, ent.line, format!("entity `{name}` has no architecture")));
        };
        self.file = fi;
        if !ent.ports.is_empty() && ent.ports.iter().any(|p| p.default.is_none() && p.mode == Mode::In) {
            return self.err(ent.line, format!("top-level testbench entity `{name}` must not have input ports"));
        }
        let dut = arch.stmts.iter().find_map(|c| match c {
            Concurrent::Instance { label, unit, ports, .. } => Some((
                unit.clone(),
                label.clone(),
                ports
                    .iter()
                    .filter_map(|p| Some((p.formal.clone()?, expr_name(&p.actual).unwrap_or_else(|| "<expr>".into()))))
                    .collect(),
            )),
            _ => None,
        });
        self.hier = format!("{name}.");
        self.instance_body(ent, arch, &HashMap::new(), &HashMap::new(), fi)?;
        Ok(Elab {
            files: self.sources.iter().map(|s| s.label.clone()).collect(),
            signals: self.signals,
            procs: self.procs,
            funcs: self.funcs,
            bridges: self.bridges,
            top: name.to_string(),
            dut,
            log: self.log,
        })
    }

    /// Elaborate an architecture with generic values and port bindings.
    fn instance_body(&mut self, ent: &Entity, arch: &Architecture, generics: &HashMap<String, (Value, Ty)>, ports: &HashMap<String, SigId>, fi: u16) -> R<()> {
        let saved_scopes = std::mem::replace(&mut self.scopes, vec![std_types()]);
        let saved_file = self.file;
        self.file = fi;
        self.scopes.push(HashMap::new());
        let r = (|| -> R<()> {
            for u in &self.sources[fi as usize].file.uses.clone() {
                self.use_package(u, ent.line)?;
            }
            for g in &ent.generics {
                let ty = self.subtype(&g.ty)?;
                let v = match generics.get(&g.name) {
                    Some((v, _)) => v.clone(),
                    None => match &g.default {
                        Some(d) => {
                            let re = self.expr(d, Some(&ty), ent.line)?;
                            self.const_eval(&re, ent.line)?
                        }
                        None => return self.err(ent.line, format!("generic `{}` has no value", g.name)),
                    },
                };
                self.bind(&g.name, Bind::Const(v, ty));
            }
            for p in &ent.ports {
                let ty = self.subtype(&p.ty)?;
                let id = match ports.get(&p.name) {
                    Some(id) => *id,
                    None => {
                        let init = match &p.default {
                            Some(d) => {
                                let re = self.expr(d, Some(&ty), ent.line)?;
                                self.const_eval(&re, ent.line)?
                            }
                            None => ty.default_value(),
                        };
                        let id = self.signals.len();
                        self.signals.push(SignalDef { name: format!("{}{}", self.hier, p.name), ty: ty.clone(), init });
                        id
                    }
                };
                self.bind(&p.name, Bind::Sig(id));
            }
            self.decls(&arch.decls, false)?;
            for c in &arch.stmts {
                self.concurrent(c)?;
            }
            Ok(())
        })();
        self.scopes = saved_scopes;
        self.file = saved_file;
        r
    }

    fn start_frame(&mut self) {
        self.frames.push(Frame { var_tys: Vec::new() });
        self.ops.clear();
        self.loops.clear();
    }

    fn finish_frame(&mut self, name: String, body_start: usize, line: usize) {
        let frame = self.frames.pop().expect("frame");
        let ops = std::mem::take(&mut self.ops);
        let loc = self.loc(line);
        self.procs.push(ProcDef { name, code: Rc::new(Code { ops, body_start, var_tys: frame.var_tys }), loc });
    }

    fn concurrent(&mut self, c: &Concurrent) -> R<()> {
        match c {
            Concurrent::Process { label, sens, sens_all, decls, body, line } => {
                self.start_frame();
                self.scopes.push(HashMap::new());
                let r = (|| -> R<usize> {
                    self.decls(decls, true)?;
                    let start = self.here();
                    self.stmts(body)?;
                    // Sensitivity list: implicit `wait on` at the end.
                    let mut sigs = Vec::new();
                    if *sens_all {
                        for op in &self.ops[start..] {
                            op_signals(op, &mut sigs);
                        }
                    } else if let Some(list) = sens {
                        for s in list {
                            match self.expr(s, None, *line)? {
                                RExpr::Sig(id) => sigs.push(id),
                                other => signals_read(&other, &mut sigs),
                            }
                        }
                    }
                    if sens.is_some() {
                        if self.ops[start..].iter().any(|o| matches!(o, Op::Wait { .. })) {
                            return self.err(*line, "a process with a sensitivity list cannot contain `wait`");
                        }
                        let loc = self.loc(*line);
                        self.ops.push(Op::Wait { on: sigs, until: None, timeout: None, loc });
                    } else if !self.ops[start..].iter().any(|o| matches!(o, Op::Wait { .. })) {
                        return self.err(*line, "process has neither a sensitivity list nor a `wait` (it would loop forever at time 0)");
                    }
                    self.ops.push(Op::Jump(start));
                    Ok(start)
                })();
                self.scopes.pop();
                let start = match r {
                    Ok(s) => s,
                    Err(e) => {
                        self.frames.pop();
                        return Err(e);
                    }
                };
                let name = format!("{}{}", self.hier, label.clone().unwrap_or_else(|| format!("process@{line}")));
                self.finish_frame(name, start, *line);
            }
            Concurrent::Assign { target, arms, line } => {
                self.start_frame();
                let s = Stmt::SigAssign { target: target.clone(), arms: arms.clone(), transport: false, line: *line };
                let r = self.stmt(&s);
                if let Err(e) = r {
                    self.frames.pop();
                    return Err(e);
                }
                let mut sigs = Vec::new();
                for op in &self.ops {
                    op_signals_reads(op, &mut sigs);
                }
                let loc = self.loc(*line);
                self.ops.push(Op::Wait { on: sigs, until: None, timeout: None, loc });
                self.ops.push(Op::Jump(0));
                let name = format!("{}assign@{line}", self.hier);
                self.finish_frame(name, 0, *line);
            }
            Concurrent::Select { sel, target, arms, line } => {
                self.start_frame();
                let r = (|| -> R<()> {
                    let s = Stmt::Case {
                        sel: sel.clone(),
                        arms: arms
                            .iter()
                            .map(|(w, ch)| (ch.clone(), vec![Stmt::SigAssign { target: target.clone(), arms: vec![(w.clone(), None)], transport: false, line: *line }]))
                            .collect(),
                        line: *line,
                    };
                    self.stmt(&s)
                })();
                if let Err(e) = r {
                    self.frames.pop();
                    return Err(e);
                }
                let mut sigs = Vec::new();
                for op in &self.ops {
                    op_signals_reads(op, &mut sigs);
                }
                let loc = self.loc(*line);
                self.ops.push(Op::Wait { on: sigs, until: None, timeout: None, loc });
                self.ops.push(Op::Jump(0));
                let name = format!("{}select@{line}", self.hier);
                self.finish_frame(name, 0, *line);
            }
            Concurrent::Assert { cond, msg, severity, line } => {
                self.start_frame();
                let s = Stmt::Assert { cond: cond.clone(), msg: msg.clone(), severity: severity.clone(), line: *line };
                if let Err(e) = self.stmt(&s) {
                    self.frames.pop();
                    return Err(e);
                }
                let mut sigs = Vec::new();
                for op in &self.ops {
                    op_signals_reads(op, &mut sigs);
                }
                let loc = self.loc(*line);
                self.ops.push(Op::Wait { on: sigs, until: None, timeout: None, loc });
                self.ops.push(Op::Jump(0));
                let name = format!("{}assert@{line}", self.hier);
                self.finish_frame(name, 0, *line);
            }
            Concurrent::Call { name, line } => {
                self.start_frame();
                if let Err(e) = self.call_stmt(name, *line) {
                    self.frames.pop();
                    return Err(e);
                }
                if !self.ops.iter().any(|o| matches!(o, Op::Wait { .. })) {
                    let mut sigs = Vec::new();
                    for op in &self.ops {
                        op_signals_reads(op, &mut sigs);
                    }
                    let loc = self.loc(*line);
                    self.ops.push(Op::Wait { on: sigs, until: None, timeout: None, loc });
                }
                self.ops.push(Op::Jump(0));
                let name = format!("{}call@{line}", self.hier);
                self.finish_frame(name, 0, *line);
            }
            Concurrent::Instance { label, unit, generics, ports, line } => self.instance(label, unit, generics, ports, *line)?,
        }
        Ok(())
    }

    fn instance(&mut self, label: &str, unit: &str, generics: &[Arg], ports: &[Arg], line: usize) -> R<()> {
        // Port actuals: signals, constants (fresh constant signal), or open.
        let mut bound: Vec<(Option<String>, Option<SigId>, Option<RExpr>)> = Vec::new();
        for p in ports {
            match &p.actual {
                Expr::Open => bound.push((p.formal.clone(), None, None)),
                e => {
                    let re = self.expr(e, None, line)?;
                    match re {
                        RExpr::Sig(s) => bound.push((p.formal.clone(), Some(s), None)),
                        RExpr::Const(_) => bound.push((p.formal.clone(), None, Some(re))),
                        _ => return self.err(line, format!("port map actual for `{}` must be a signal name, a constant, or open", p.formal.clone().unwrap_or_default())),
                    }
                }
            }
        }
        if let Some((fi, ent, arch)) = self.find_entity(unit) {
            if let Some(arch) = arch {
                // Behavioral VHDL instance.
                let mut gvals = HashMap::new();
                for (k, g) in generics.iter().enumerate() {
                    let name = match &g.formal {
                        Some(f) => f.clone(),
                        None => ent.generics.get(k).map(|d| d.name.clone()).unwrap_or_default(),
                    };
                    let decl = ent.generics.iter().find(|d| d.name == name);
                    let ty = match decl {
                        Some(d) => {
                            let saved = self.file;
                            self.file = fi;
                            let t = self.subtype(&d.ty);
                            self.file = saved;
                            t?
                        }
                        None => return self.err(line, format!("entity `{unit}` has no generic `{name}`")),
                    };
                    let re = self.expr(&g.actual, Some(&ty), line)?;
                    let v = self.const_eval(&re, line)?;
                    gvals.insert(name, (v, ty));
                }
                let mut pmap = HashMap::new();
                for (k, (formal, sig, konst)) in bound.iter().enumerate() {
                    let name = match formal {
                        Some(f) => f.clone(),
                        None => ent.ports.get(k).map(|p| p.name.clone()).unwrap_or_default(),
                    };
                    if !ent.ports.iter().any(|p| p.name == name) {
                        return self.err(line, format!("entity `{unit}` has no port `{name}`"));
                    }
                    if let Some(s) = sig {
                        pmap.insert(name, *s);
                    } else if let Some(RExpr::Const(v)) = konst {
                        let id = self.signals.len();
                        let ty = self.ty_of(&RExpr::Const(v.clone())).unwrap_or(Ty::Logic);
                        self.signals.push(SignalDef { name: format!("{}{label}.{name}", self.hier), ty, init: v.clone() });
                        pmap.insert(name, id);
                    }
                }
                let child_hier = format!("{}{label}.", self.hier);
                let saved_hier = std::mem::replace(&mut self.hier, child_hier);
                let r = self.instance_body(ent, arch, &gvals, &pmap, fi);
                self.hier = saved_hier;
                return r;
            }
        }
        // Netlist bridge: an SV module (or VHDL entity without architecture
        // here) found next to the testbench.
        let Some(dir) = self.dir.clone() else {
            return self.err(line, format!("unit `{unit}` not found"));
        };
        let path = crate::tb::find_dut_source(&dir, unit).map_err(|e| TbError::new(&self.sources[self.file as usize].label, line, e.msg))?;
        if !generics.is_empty() {
            self.log.push(format!("{label}: generic map ignored for netlist DUT `{unit}` (synthesized with its defaults)"));
        }
        let mut bports = Vec::new();
        for (formal, sig, konst) in bound {
            let Some(formal) = formal else {
                return self.err(line, format!("netlist DUT `{unit}` needs named port associations (`port => signal`)"));
            };
            let id = match (sig, konst) {
                (Some(s), _) => s,
                (None, Some(RExpr::Const(v))) => {
                    let id = self.signals.len();
                    let ty = self.ty_of(&RExpr::Const(v.clone())).unwrap_or(Ty::Logic);
                    self.signals.push(SignalDef { name: format!("{}{label}.{formal}", self.hier), ty, init: v });
                    id
                }
                _ => continue,
            };
            bports.push((formal, id, false));
        }
        self.bridges.push(BridgeDef { label: format!("{}{label}", self.hier), unit: unit.to_string(), path, ports: bports });
        Ok(())
    }
}

enum TB {
    Sig(SigId),
    Var(usize),
    Alias(RTarget),
}

fn string_ty() -> Ty {
    Ty::Array(Rc::new(ArrTy { name: "string".into(), kind: ArrKind::Str, elem: Ty::Char, range: None }))
}

fn const_ty_from(t: &Ty, v: &Value) -> Ty {
    match (t, v) {
        (Ty::Array(a), Value::Arr(x)) if a.range.is_none() => {
            Ty::Array(Rc::new(ArrTy { name: a.name.clone(), kind: a.kind, elem: a.elem.clone(), range: Some((x.left, x.right(), x.asc)) }))
        }
        _ => t.clone(),
    }
}

fn same_family(a: Option<&Ty>, b: Option<&Ty>) -> bool {
    match (a, b) {
        (Some(Ty::Array(x)), Some(Ty::Array(y))) => x.kind == y.kind || (x.kind == ArrKind::Slv && y.kind == ArrKind::Slv),
        (Some(Ty::Int { .. }), Some(Ty::Int { .. })) => true,
        (Some(Ty::Logic | Ty::Bit), Some(Ty::Logic | Ty::Bit)) => true,
        (Some(x), Some(y)) => std::mem::discriminant(x) == std::mem::discriminant(y),
        _ => false,
    }
}

fn is_unit_path(e: &Expr, is_unit: &dyn Fn(&str) -> bool) -> bool {
    match e {
        Expr::Name(n) => is_unit(n),
        Expr::Select(p, f) => is_unit_path(p, is_unit) && (is_unit(f) || true),
        _ => false,
    }
}

fn last_name(e: &Expr) -> Option<String> {
    match e {
        Expr::Name(n) => Some(n.clone()),
        Expr::Select(_, f) => Some(f.clone()),
        _ => None,
    }
}

fn expr_name(e: &Expr) -> Option<String> {
    match e {
        Expr::Name(n) => Some(n.clone()),
        Expr::Call(p, _) | Expr::Select(p, _) => expr_name(p),
        _ => None,
    }
}

/// Constant-fold pure expressions over constants (not function calls).
fn fold(e: RExpr, _funcs: &[Func]) -> RExpr {
    match &e {
        RExpr::Bin(op, a, b) => {
            if let (RExpr::Const(x), RExpr::Const(y)) = (&**a, &**b) {
                if let Ok(v) = super::ops::binop(*op, x, y) {
                    return RExpr::Const(v);
                }
            }
            e
        }
        _ => e,
    }
}

fn op_signals_reads(op: &Op, out: &mut Vec<SigId>) {
    match op {
        Op::SigAssign { wave, target, .. } => {
            for (v, a) in wave {
                signals_read(v, out);
                if let Some(a) = a {
                    signals_read(a, out);
                }
            }
            target_index_reads(target, out);
        }
        Op::VarAssign { value, .. } => signals_read(value, out),
        Op::JumpIfNot(c, ..) => signals_read(c, out),
        Op::Case { sel, .. } => signals_read(sel, out),
        Op::Assert { cond, msg, .. } => {
            signals_read(cond, out);
            if let Some(m) = msg {
                signals_read(m, out);
            }
        }
        Op::Report { msg, .. } => signals_read(msg, out),
        _ => {}
    }
}

fn target_index_reads(t: &RTarget, out: &mut Vec<SigId>) {
    match t {
        RTarget::Sig(_, sels) | RTarget::Var(_, sels) => {
            for s in sels {
                match s {
                    RSel::Index(e) => signals_read(e, out),
                    RSel::Slice(a, _, b) => {
                        signals_read(a, out);
                        signals_read(b, out);
                    }
                    RSel::Field(_) => {}
                }
            }
        }
        RTarget::Agg(ts) => {
            for x in ts {
                target_index_reads(x, out);
            }
        }
    }
}

/// `process (all)`: every signal the body reads.
fn op_signals(op: &Op, out: &mut Vec<SigId>) {
    op_signals_reads(op, out);
}
