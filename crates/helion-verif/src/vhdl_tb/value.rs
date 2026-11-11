//! Run-time values, types, and the predefined / `ieee.std_logic_1164` /
//! `ieee.numeric_std` operators of the VHDL simulator.
//!
//! Characters, `bit`, and `std_ulogic` values are all [`Value::Ch`] (their
//! literals are characters); vectors are [`Value::Arr`] with an
//! [`ArrKind`] that selects numeric_std semantics for `unsigned`/`signed`.

use std::rc::Rc;

/// Femtoseconds per nanosecond (VHDL `time` resolution is fs).
pub const FS_PER_NS: i64 = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrKind {
    /// `std_logic_vector` / `std_ulogic_vector` / `bit_vector`.
    Slv,
    Unsigned,
    Signed,
    /// `string`.
    Str,
    /// Any other array type (array of records, of integers, ...).
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    /// `std_ulogic` / `std_logic`.
    Logic,
    Bit,
    Bool,
    Char,
    Int { lo: i64, hi: i64 },
    Real,
    Time,
    Enum(Rc<EnumTy>),
    Array(Rc<ArrTy>),
    Record(Rc<RecTy>),
    /// `std.textio.line` (modelled as a growable string).
    Line,
    /// `std.textio.text` files (`output`, `input`).
    File,
    Severity,
}

#[derive(Debug, PartialEq)]
pub struct EnumTy {
    pub name: String,
    pub lits: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub struct ArrTy {
    pub name: String,
    pub kind: ArrKind,
    pub elem: Ty,
    /// `(left, right, ascending)`; `None` when unconstrained.
    pub range: Option<(i64, i64, bool)>,
}

#[derive(Debug, PartialEq)]
pub struct RecTy {
    pub name: String,
    pub fields: Vec<(String, Ty)>,
}

impl Ty {
    pub fn name(&self) -> String {
        match self {
            Ty::Logic => "std_logic".into(),
            Ty::Bit => "bit".into(),
            Ty::Bool => "boolean".into(),
            Ty::Char => "character".into(),
            Ty::Int { .. } => "integer".into(),
            Ty::Real => "real".into(),
            Ty::Time => "time".into(),
            Ty::Enum(e) => e.name.clone(),
            Ty::Array(a) => a.name.clone(),
            Ty::Record(r) => r.name.clone(),
            Ty::Line => "line".into(),
            Ty::File => "text".into(),
            Ty::Severity => "severity_level".into(),
        }
    }

    /// Default (leftmost) value.
    pub fn default_value(&self) -> Value {
        match self {
            Ty::Logic => Value::Ch(b'U'),
            Ty::Bit => Value::Ch(b'0'),
            Ty::Bool => Value::Bool(false),
            Ty::Char => Value::Ch(0),
            Ty::Int { lo, .. } => Value::Int(*lo),
            Ty::Real => Value::Real(f64::MIN),
            Ty::Time => Value::Time(i64::MIN),
            Ty::Enum(_) => Value::Enum(0),
            Ty::Severity => Value::Enum(0),
            Ty::Array(a) => {
                let (left, right, asc) = a.range.unwrap_or((0, -1, true));
                let n = len_of(left, right, asc);
                Value::Arr(Rc::new(ArrVal {
                    kind: a.kind,
                    left,
                    asc,
                    elems: vec![a.elem.default_value(); n],
                }))
            }
            Ty::Record(r) => Value::Rec(Rc::new(r.fields.iter().map(|(_, t)| t.default_value()).collect())),
            Ty::Line => Value::str(""),
            Ty::File => Value::Int(0),
        }
    }

    /// Width in bits for VCD tracing.
    pub fn trace_width(&self) -> Option<u32> {
        match self {
            Ty::Logic | Ty::Bit | Ty::Bool => Some(1),
            Ty::Int { lo, hi } => {
                if *lo >= 0 {
                    Some((64 - (*hi as u64).leading_zeros()).clamp(1, 32))
                } else {
                    Some(32)
                }
            }
            Ty::Enum(e) => Some((usize::BITS - e.lits.len().saturating_sub(1).leading_zeros()).max(1)),
            Ty::Array(a) => match (&a.elem, a.range) {
                (Ty::Logic | Ty::Bit, Some((l, r, asc))) => Some((len_of(l, r, asc) as u32).clamp(1, 64)),
                _ => None,
            },
            _ => None,
        }
    }
}

pub fn len_of(left: i64, right: i64, asc: bool) -> usize {
    let n = if asc { right - left + 1 } else { left - right + 1 };
    n.max(0) as usize
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArrVal {
    pub kind: ArrKind,
    pub left: i64,
    pub asc: bool,
    pub elems: Vec<Value>,
}

impl ArrVal {
    pub fn right(&self) -> i64 {
        let n = self.elems.len() as i64;
        if self.asc { self.left + n - 1 } else { self.left - n + 1 }
    }

    /// Position (0 = leftmost) of index `i`.
    pub fn pos(&self, i: i64) -> Option<usize> {
        let p = if self.asc { i - self.left } else { self.left - i };
        (p >= 0 && (p as usize) < self.elems.len()).then_some(p as usize)
    }

    pub fn low(&self) -> i64 {
        self.left.min(self.right())
    }

    pub fn high(&self) -> i64 {
        self.left.max(self.right())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Real(f64),
    /// Time in femtoseconds.
    Time(i64),
    Bool(bool),
    /// Character / bit / std_ulogic literal.
    Ch(u8),
    /// Enumeration position (user enums, severity levels).
    Enum(u32),
    Arr(Rc<ArrVal>),
    Rec(Rc<Vec<Value>>),
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Arr(Rc::new(ArrVal {
            kind: ArrKind::Str,
            left: 1,
            asc: true,
            elems: s.bytes().map(Value::Ch).collect(),
        }))
    }

    pub fn vec(kind: ArrKind, bits_msb_first: &[u8]) -> Value {
        let n = bits_msb_first.len() as i64;
        Value::Arr(Rc::new(ArrVal {
            kind,
            left: (n - 1).max(0),
            asc: false,
            elems: bits_msb_first.iter().map(|b| Value::Ch(*b)).collect(),
        }))
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            Value::Enum(v) => Some(*v as i64),
            Value::Bool(b) => Some(*b as i64),
            Value::Time(t) => Some(*t),
            _ => None,
        }
    }

    pub fn truthy(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Ch(c) => match c {
                b'1' | b'H' => Some(true),
                b'0' | b'L' => Some(false),
                _ => Some(false),
            },
            _ => None,
        }
    }

    /// Text of a string-valued array.
    pub fn as_text(&self) -> Option<String> {
        match self {
            Value::Arr(a) => Some(
                a.elems
                    .iter()
                    .map(|e| match e {
                        Value::Ch(c) => *c as char,
                        _ => '?',
                    })
                    .collect(),
            ),
            Value::Ch(c) => Some((*c as char).to_string()),
            _ => None,
        }
    }

    /// Bits of a vector, MSB (leftmost) first, as characters.
    pub fn bits(&self) -> Option<Vec<u8>> {
        match self {
            Value::Arr(a) => a
                .elems
                .iter()
                .map(|e| match e {
                    Value::Ch(c) => Some(*c),
                    _ => None,
                })
                .collect(),
            Value::Ch(c) => Some(vec![*c]),
            _ => None,
        }
    }

    /// Unsigned value of a vector's bits; `None` when any bit is not 0/1/L/H.
    pub fn to_u64(&self) -> Option<u64> {
        let bits = self.bits()?;
        let mut v: u64 = 0;
        for b in bits {
            v = (v << 1)
                | match b {
                    b'1' | b'H' => 1,
                    b'0' | b'L' => 0,
                    _ => return None,
                };
        }
        Some(v)
    }

    /// Value + has-X for VCD tracing (`width` bits, LSB first).
    pub fn trace(&self, width: u32) -> (u64, bool) {
        match self {
            Value::Ch(c) => match c {
                b'1' | b'H' => (1, false),
                b'0' | b'L' => (0, false),
                _ => (0, true),
            },
            Value::Bool(b) => (*b as u64, false),
            Value::Int(v) => ((*v as u64) & mask(width), false),
            Value::Enum(v) => (*v as u64, false),
            Value::Arr(_) => match self.to_u64() {
                Some(v) => (v & mask(width), false),
                None => (0, true),
            },
            _ => (0, true),
        }
    }
}

pub fn mask(width: u32) -> u64 {
    if width >= 64 { u64::MAX } else { (1u64 << width) - 1 }
}

// ---------------------------------------------------------------------------
// std_logic_1164
// ---------------------------------------------------------------------------

fn sl_idx(c: u8) -> usize {
    match c {
        b'U' => 0,
        b'X' => 1,
        b'0' => 2,
        b'1' => 3,
        b'Z' => 4,
        b'W' => 5,
        b'L' => 6,
        b'H' => 7,
        b'-' => 8,
        _ => 1,
    }
}

const SL: &[u8; 9] = b"UX01ZWLH-";

/// IEEE 1164 `resolved` table.
pub fn resolve2(a: u8, b: u8) -> u8 {
    const T: [&[u8; 9]; 9] = [
        b"UUUUUUUUU", b"UXXXXXXXX", b"UX0X0000X", b"UXX11111X", b"UX01ZWLHX", b"UX01WWWWX", b"UX01LWLWX", b"UX01HWWHX",
        b"UXXXXXXXX",
    ];
    T[sl_idx(a)][sl_idx(b)]
}

pub fn sl_and(a: u8, b: u8) -> u8 {
    const T: [&[u8; 9]; 9] = [
        b"UU0UUU0UU", b"UX0XXX0XX", b"000000000", b"UX01XX01X", b"UX0XXX0XX", b"UX0XXX0XX", b"000000000", b"UX01XX01X",
        b"UX0XXX0XX",
    ];
    T[sl_idx(a)][sl_idx(b)]
}

pub fn sl_or(a: u8, b: u8) -> u8 {
    const T: [&[u8; 9]; 9] = [
        b"UUU1UUU1U", b"UXX1XXX1X", b"UX01XX01X", b"111111111", b"UXX1XXX1X", b"UXX1XXX1X", b"UX01XX01X", b"111111111",
        b"UXX1XXX1X",
    ];
    T[sl_idx(a)][sl_idx(b)]
}

pub fn sl_xor(a: u8, b: u8) -> u8 {
    const T: [&[u8; 9]; 9] = [
        b"UUUUUUUUU", b"UXXXXXXXX", b"UX01XX01X", b"UX10XX10X", b"UXXXXXXXX", b"UXXXXXXXX", b"UX01XX01X", b"UX10XX10X",
        b"UXXXXXXXX",
    ];
    T[sl_idx(a)][sl_idx(b)]
}

pub fn sl_not(a: u8) -> u8 {
    b"UX10XX10X"[sl_idx(a)]
}

/// `to_x01`.
pub fn to_x01(a: u8) -> u8 {
    match a {
        b'0' | b'L' => b'0',
        b'1' | b'H' => b'1',
        _ => b'X',
    }
}

pub fn is_sl_char(c: u8) -> bool {
    SL.contains(&c)
}

// ---------------------------------------------------------------------------
// numeric_std helpers
// ---------------------------------------------------------------------------

/// Integer value of an unsigned/signed vector (`None` on metavalues).
pub fn vec_to_int(v: &Value, signed: bool) -> Option<i128> {
    let bits = v.bits()?;
    if bits.is_empty() {
        return Some(0);
    }
    let mut acc: i128 = 0;
    for b in &bits {
        acc = (acc << 1)
            | match *b {
                b'1' | b'H' => 1,
                b'0' | b'L' => 0,
                _ => return None,
            };
    }
    if signed && bits.len() < 128 && matches!(bits[0], b'1' | b'H') {
        acc -= 1i128 << bits.len();
    }
    Some(acc)
}

/// `n`-bit two's-complement vector of `v` (MSB first, `downto 0`).
pub fn int_to_vec(kind: ArrKind, v: i128, n: usize) -> Value {
    let bits: Vec<u8> = (0..n)
        .rev()
        .map(|i| if i < 127 && (v >> i) & 1 == 1 || (i >= 127 && v < 0) { b'1' } else { b'0' })
        .collect();
    Value::vec(kind, &bits)
}

pub fn all_x(kind: ArrKind, n: usize) -> Value {
    Value::vec(kind, &vec![b'X'; n])
}

/// Re-label an array value with a different kind (type conversion between
/// closely related array types keeps the bits and bounds).
pub fn with_kind(v: &Value, kind: ArrKind) -> Value {
    match v {
        Value::Arr(a) if a.kind != kind => Value::Arr(Rc::new(ArrVal { kind, left: a.left, asc: a.asc, elems: a.elems.clone() })),
        other => other.clone(),
    }
}

/// `'image` / `to_string` text of a value.
pub fn image(v: &Value, ty: Option<&Ty>) -> String {
    match v {
        Value::Int(i) => i.to_string(),
        Value::Real(r) => format!("{r:e}"),
        Value::Time(t) => time_image(*t),
        Value::Bool(b) => if *b { "true".into() } else { "false".into() },
        Value::Ch(c) => match ty {
            Some(Ty::Char) => (*c as char).to_string(),
            _ => format!("'{}'", *c as char),
        },
        Value::Enum(p) => match ty {
            Some(Ty::Enum(e)) => e.lits.get(*p as usize).cloned().unwrap_or_else(|| p.to_string()),
            Some(Ty::Severity) => ["note", "warning", "error", "failure"].get(*p as usize).unwrap_or(&"?").to_string(),
            _ => p.to_string(),
        },
        Value::Arr(a) => a
            .elems
            .iter()
            .map(|e| match e {
                Value::Ch(c) => (*c as char).to_string(),
                other => image(other, None),
            })
            .collect(),
        Value::Rec(_) => "(record)".into(),
    }
}

/// VHDL-style time text: the largest unit that divides the value exactly.
pub fn time_image(t: i64) -> String {
    for (u, m) in [("sec", 1_000_000_000_000_000i64), ("ms", 1_000_000_000_000), ("us", 1_000_000_000), ("ns", 1_000_000), ("ps", 1_000)] {
        if t != 0 && t % m == 0 {
            return format!("{} {u}", t / m);
        }
    }
    if t == 0 {
        return "0 ns".into();
    }
    format!("{t} fs")
}

/// Hex text (`to_hstring`): MSB-aligned groups of four, metavalues as `X`.
pub fn hex_string(v: &Value) -> String {
    let bits = v.bits().unwrap_or_default();
    let pad = (4 - bits.len() % 4) % 4;
    let mut padded = vec![b'0'; pad];
    padded.extend(bits);
    padded
        .chunks(4)
        .map(|c| {
            let mut n = 0u32;
            for b in c {
                n = (n << 1)
                    | match b {
                        b'1' | b'H' => 1,
                        b'0' | b'L' => 0,
                        _ => return 'X',
                    };
            }
            std::char::from_digit(n, 16).unwrap_or('X').to_ascii_uppercase()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn std_logic_tables_and_resolution() {
        assert_eq!(sl_and(b'1', b'0'), b'0');
        assert_eq!(sl_and(b'1', b'X'), b'X');
        assert_eq!(sl_or(b'U', b'1'), b'1');
        assert_eq!(sl_not(b'H'), b'0');
        assert_eq!(resolve2(b'Z', b'1'), b'1');
        assert_eq!(resolve2(b'0', b'1'), b'X');
    }

    #[test]
    fn numeric_conversions() {
        let v = int_to_vec(ArrKind::Signed, -3, 4);
        assert_eq!(v.bits().unwrap(), b"1101".to_vec());
        assert_eq!(vec_to_int(&v, true), Some(-3));
        assert_eq!(vec_to_int(&v, false), Some(13));
        assert_eq!(hex_string(&Value::vec(ArrKind::Slv, b"101011")), "2B");
        assert_eq!(time_image(15 * FS_PER_NS), "15 ns");
    }
}
