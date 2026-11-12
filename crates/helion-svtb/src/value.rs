//! Runtime values and types.
//!
//! Integral values are 2-state, up to 64 bits (`Int(bits, width, signed)`);
//! DUT `x`/`z` read as 0. Unpacked arrays (fixed, dynamic, queues) and
//! associative arrays are value types; class objects, events, mailboxes and
//! semaphores are handles.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::rc::Rc;

pub type ClassId = u32;
pub type ObjRef = Rc<RefCell<Object>>;
pub type ScopeRef = Rc<RefCell<HashMap<String, Var>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrKind {
    Fixed,
    Dynamic,
    Queue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Mailbox,
    Semaphore,
    Process,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumInfo {
    pub name: String,
    pub w: u32,
    pub items: Vec<(String, u64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Int { w: u32, s: bool },
    Real,
    Str,
    Event,
    Void,
    Chandle,
    Class(ClassId),
    Builtin(Builtin),
    VIf(Rc<str>),
    Enum(Rc<EnumInfo>),
    /// Unpacked array: element type, kind, fixed size (for `Fixed`).
    Arr(Rc<Ty>, ArrKind, usize),
    /// Associative array: element type, string keys?
    Assoc(Rc<Ty>, bool),
}

impl Ty {
    pub fn int(w: u32, s: bool) -> Ty {
        Ty::Int { w, s }
    }
    pub fn default_value(&self) -> Value {
        match self {
            Ty::Int { w, s } => Value::Int(0, *w, *s),
            Ty::Enum(e) => Value::Int(e.items.first().map(|i| i.1).unwrap_or(0), e.w, false),
            Ty::Real => Value::Real(0.0),
            Ty::Str => Value::Str(String::new()),
            Ty::Event => Value::Event(Rc::new(EventCell::new())),
            Ty::Void | Ty::Chandle => Value::Int(0, 1, false),
            Ty::Class(_) | Ty::Builtin(_) => Value::Obj(None),
            Ty::VIf(_) => Value::VIf(None),
            Ty::Arr(el, kind, n) => {
                let items = if *kind == ArrKind::Fixed { (0..*n).map(|_| el.default_value()).collect() } else { Vec::new() };
                Value::Arr(Box::new(ArrVal { kind: *kind, items }))
            }
            Ty::Assoc(..) => Value::Assoc(Box::new(BTreeMap::new())),
        }
    }
    pub fn width(&self) -> u32 {
        match self {
            Ty::Int { w, .. } => *w,
            Ty::Enum(e) => e.w,
            Ty::Real => 64,
            _ => 32,
        }
    }
    pub fn is_integral(&self) -> bool {
        matches!(self, Ty::Int { .. } | Ty::Enum(_))
    }
    pub fn elem(&self) -> Option<&Ty> {
        match self {
            Ty::Arr(e, ..) | Ty::Assoc(e, _) => Some(e),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct EventCell {
    pub id: u64,
    /// Time (ps) of the last trigger (`ev.triggered`).
    pub last: std::cell::Cell<Option<u64>>,
}

impl EventCell {
    pub fn new() -> Self {
        thread_local!(static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(1) });
        let id = NEXT.with(|n| {
            let v = n.get();
            n.set(v + 1);
            v
        });
        Self { id, last: std::cell::Cell::new(None) }
    }
}

impl Default for EventCell {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Key {
    I(i64),
    S(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArrVal {
    pub kind: ArrKind,
    pub items: Vec<Value>,
}

#[derive(Clone, Debug)]
pub enum Value {
    Int(u64, u32, bool),
    Real(f64),
    Str(String),
    Obj(Option<ObjRef>),
    Event(Rc<EventCell>),
    VIf(Option<Rc<crate::interp::IfaceInst>>),
    Arr(Box<ArrVal>),
    Assoc(Box<BTreeMap<Key, Value>>),
}

impl PartialEq for Value {
    fn eq(&self, o: &Self) -> bool {
        match (self, o) {
            (Value::Obj(a), Value::Obj(b)) => match (a, b) {
                (None, None) => true,
                (Some(x), Some(y)) => Rc::ptr_eq(x, y),
                _ => false,
            },
            (Value::Event(a), Value::Event(b)) => Rc::ptr_eq(a, b),
            (Value::VIf(a), Value::VIf(b)) => match (a, b) {
                (None, None) => true,
                (Some(x), Some(y)) => Rc::ptr_eq(x, y),
                _ => false,
            },
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Arr(a), Value::Arr(b)) => a.items == b.items,
            (Value::Assoc(a), Value::Assoc(b)) => a == b,
            (Value::Real(a), Value::Real(b)) => a == b,
            (a, b) if a.is_numeric() && b.is_numeric() => a.as_u64() == b.as_u64(),
            _ => false,
        }
    }
}

pub fn mask(v: u64, w: u32) -> u64 {
    if w >= 64 || w == 0 { v } else { v & ((1u64 << w) - 1) }
}

pub fn sext(v: u64, w: u32) -> i64 {
    if w == 0 || w >= 64 {
        v as i64
    } else {
        let sh = 64 - w;
        ((v << sh) as i64) >> sh
    }
}

impl Value {
    pub fn int(v: i64) -> Value {
        Value::Int(mask(v as u64, 32), 32, true)
    }
    pub fn uint(v: u64, w: u32) -> Value {
        Value::Int(mask(v, w), w, false)
    }
    pub fn bit(b: bool) -> Value {
        Value::Int(b as u64, 1, false)
    }
    pub fn is_numeric(&self) -> bool {
        matches!(self, Value::Int(..) | Value::Real(_))
    }
    /// Raw bits (reals truncate; strings pack big-endian bytes).
    pub fn as_u64(&self) -> u64 {
        match self {
            Value::Int(v, _, _) => *v,
            Value::Real(r) => r.round() as i64 as u64,
            Value::Str(s) => s.bytes().fold(0u64, |a, b| (a << 8) | b as u64),
            Value::Obj(o) => o.as_ref().map(|o| o.borrow().id).unwrap_or(0),
            Value::VIf(v) => v.is_some() as u64,
            Value::Event(e) => e.id,
            Value::Arr(a) => a.items.len() as u64,
            Value::Assoc(a) => a.len() as u64,
        }
    }
    pub fn as_i64(&self) -> i64 {
        match self {
            Value::Int(v, w, true) => sext(*v, *w),
            Value::Real(r) => r.round() as i64,
            other => other.as_u64() as i64,
        }
    }
    pub fn as_f64(&self) -> f64 {
        match self {
            Value::Real(r) => *r,
            Value::Int(v, w, true) => sext(*v, *w) as f64,
            other => other.as_u64() as f64,
        }
    }
    pub fn width(&self) -> u32 {
        match self {
            Value::Int(_, w, _) => *w,
            Value::Real(_) => 64,
            Value::Str(s) => (s.len() as u32 * 8).max(8),
            _ => 32,
        }
    }
    pub fn signed(&self) -> bool {
        matches!(self, Value::Int(_, _, true) | Value::Real(_))
    }
    pub fn truthy(&self) -> bool {
        match self {
            Value::Int(v, _, _) => *v != 0,
            Value::Real(r) => *r != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::Obj(o) => o.is_some(),
            Value::VIf(v) => v.is_some(),
            Value::Event(_) => true,
            Value::Arr(a) => !a.items.is_empty(),
            Value::Assoc(a) => !a.is_empty(),
        }
    }
    pub fn as_str(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            Value::Int(v, w, _) => {
                // Integral → string: big-endian bytes, leading NULs dropped.
                let n = w.div_ceil(8).min(8);
                let mut out = String::new();
                for i in (0..n).rev() {
                    let b = ((v >> (8 * i)) & 0xff) as u8;
                    if b != 0 {
                        out.push(b as char);
                    }
                }
                out
            }
            other => format!("{}", other.as_u64()),
        }
    }
    pub fn to_key(&self, string_keys: bool) -> Key {
        if string_keys { Key::S(self.as_str()) } else { Key::I(self.as_i64()) }
    }
    pub fn obj(&self) -> Option<ObjRef> {
        match self {
            Value::Obj(o) => o.clone(),
            _ => None,
        }
    }
}

impl Key {
    pub fn to_value(&self) -> Value {
        match self {
            Key::I(i) => Value::Int(*i as u64, 64, true),
            Key::S(s) => Value::Str(s.clone()),
        }
    }
}

/// Convert `v` for storage in a slot of type `ty` (width truncation, int ↔
/// real, string packing, array element conversion).
pub fn convert(v: Value, ty: &Ty) -> Value {
    match ty {
        Ty::Int { w, s } => match v {
            Value::Int(x, _, _) => Value::Int(mask(x, *w), *w, *s),
            Value::Real(r) => Value::Int(mask(r.round() as i64 as u64, *w), *w, *s),
            Value::Str(st) => Value::Int(mask(Value::Str(st).as_u64(), *w), *w, *s),
            other => Value::Int(mask(other.as_u64(), *w), *w, *s),
        },
        Ty::Enum(e) => Value::Int(mask(v.as_u64(), e.w), e.w, false),
        Ty::Real => Value::Real(v.as_f64()),
        Ty::Str => match v {
            Value::Str(s) => Value::Str(s),
            other => Value::Str(other.as_str()),
        },
        Ty::Arr(el, kind, n) => match v {
            Value::Arr(mut a) => {
                let mut items: Vec<Value> = std::mem::take(&mut a.items).into_iter().map(|x| convert(x, el)).collect();
                if *kind == ArrKind::Fixed {
                    items.resize_with(*n, || el.default_value());
                }
                Value::Arr(Box::new(ArrVal { kind: *kind, items }))
            }
            other => other,
        },
        Ty::Assoc(el, _) => match v {
            Value::Assoc(m) => Value::Assoc(Box::new(m.into_iter().map(|(k, x)| (k, convert(x, el))).collect())),
            other => other,
        },
        Ty::VIf(_) => match v {
            Value::Obj(None) => Value::VIf(None),
            other => other,
        },
        _ => v,
    }
}

#[derive(Clone, Debug)]
pub struct Var {
    pub ty: Ty,
    pub v: Value,
}

impl Var {
    pub fn new(ty: Ty) -> Self {
        let v = ty.default_value();
        Self { ty, v }
    }
}

#[derive(Debug, Default)]
pub enum Native {
    #[default]
    None,
    Mailbox { q: VecDeque<Value>, bound: usize },
    Semaphore { keys: u64 },
    Process { pid: usize },
}

#[derive(Debug)]
pub struct Object {
    pub id: u64,
    pub class: Option<ClassId>,
    pub builtin: Option<Builtin>,
    pub fields: HashMap<String, Var>,
    pub native: Native,
    /// `rand_mode(0)` variables and `constraint_mode(0)` blocks.
    pub rand_off: HashSet<String>,
    pub cons_off: HashSet<String>,
    /// Persistent solver (keeps `randc` permutation state across calls).
    pub solver: Option<helion_sim::RandConstraintSolver>,
}

impl Object {
    pub fn new(id: u64, class: Option<ClassId>, builtin: Option<Builtin>) -> Self {
        Self { id, class, builtin, fields: HashMap::new(), native: Native::None, rand_off: HashSet::new(), cons_off: HashSet::new(), solver: None }
    }
}

/// Short type-agnostic rendering for `%p` and messages.
pub fn show(v: &Value) -> String {
    match v {
        Value::Int(x, w, s) => {
            if *s { sext(*x, *w).to_string() } else { x.to_string() }
        }
        Value::Real(r) => format!("{r}"),
        Value::Str(s) => format!("\"{s}\""),
        Value::Obj(None) | Value::VIf(None) => "null".into(),
        Value::Obj(Some(o)) => format!("@{}", o.borrow().id),
        Value::VIf(Some(i)) => format!("<interface {}>", i.name),
        Value::Event(e) => format!("<event {}>", e.id),
        Value::Arr(a) => format!("'{{{}}}", a.items.iter().map(show).collect::<Vec<_>>().join(", ")),
        Value::Assoc(m) => format!(
            "'{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}:{}", show(&k.to_value()), show(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
