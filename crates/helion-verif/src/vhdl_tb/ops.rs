//! Operator semantics: predefined operators on scalars, `std_logic_1164`
//! logical operators on `std_ulogic` and vectors, and `numeric_std`
//! arithmetic / comparison on `unsigned` / `signed`.

use std::rc::Rc;

use super::ast::{BinOp, UnOp};
use super::value::*;

type R = Result<Value, String>;

fn arr_parts(v: &Value) -> Option<&ArrVal> {
    match v {
        Value::Arr(a) => Some(a),
        _ => None,
    }
}

fn map_bits(a: &ArrVal, f: impl Fn(u8) -> u8) -> Value {
    Value::Arr(Rc::new(ArrVal {
        kind: a.kind,
        left: a.left,
        asc: a.asc,
        elems: a
            .elems
            .iter()
            .map(|e| match e {
                Value::Ch(c) => Value::Ch(f(*c)),
                other => other.clone(),
            })
            .collect(),
    }))
}

fn zip_bits(op: BinOp, a: &ArrVal, b: &ArrVal, f: impl Fn(u8, u8) -> u8) -> R {
    if a.elems.len() != b.elems.len() {
        return Err(format!(
            "operator {op:?}: length mismatch ({} vs {} elements)",
            a.elems.len(),
            b.elems.len()
        ));
    }
    Ok(Value::Arr(Rc::new(ArrVal {
        kind: a.kind,
        left: a.left,
        asc: a.asc,
        elems: a
            .elems
            .iter()
            .zip(&b.elems)
            .map(|(x, y)| match (x, y) {
                (Value::Ch(p), Value::Ch(q)) => Value::Ch(f(*p, *q)),
                (x, _) => x.clone(),
            })
            .collect(),
    })))
}

fn logic2(op: BinOp, a: u8, b: u8) -> u8 {
    match op {
        BinOp::And => sl_and(a, b),
        BinOp::Or => sl_or(a, b),
        BinOp::Nand => sl_not(sl_and(a, b)),
        BinOp::Nor => sl_not(sl_or(a, b)),
        BinOp::Xor => sl_xor(a, b),
        _ => sl_not(sl_xor(a, b)),
    }
}

fn is_numeric_kind(k: ArrKind) -> bool {
    matches!(k, ArrKind::Unsigned | ArrKind::Signed)
}

/// numeric_std arithmetic result width and operands as integers.
fn numeric_operands(a: &Value, b: &Value) -> Option<(ArrKind, usize, Option<i128>, Option<i128>)> {
    match (a, b) {
        (Value::Arr(x), Value::Arr(y)) if is_numeric_kind(x.kind) || is_numeric_kind(y.kind) => {
            let kind = if is_numeric_kind(x.kind) { x.kind } else { y.kind };
            let s = kind == ArrKind::Signed;
            Some((kind, x.elems.len().max(y.elems.len()), vec_to_int(a, s), vec_to_int(b, s)))
        }
        (Value::Arr(x), Value::Int(i)) if is_numeric_kind(x.kind) => {
            Some((x.kind, x.elems.len(), vec_to_int(a, x.kind == ArrKind::Signed), Some(*i as i128)))
        }
        (Value::Int(i), Value::Arr(y)) if is_numeric_kind(y.kind) => {
            Some((y.kind, y.elems.len(), Some(*i as i128), vec_to_int(b, y.kind == ArrKind::Signed)))
        }
        (Value::Arr(x), Value::Ch(c)) if is_numeric_kind(x.kind) => {
            let bit = match c {
                b'1' | b'H' => Some(1),
                b'0' | b'L' => Some(0),
                _ => None,
            };
            Some((x.kind, x.elems.len(), vec_to_int(a, x.kind == ArrKind::Signed), bit))
        }
        _ => None,
    }
}

fn cmp_result(op: BinOp, ord: std::cmp::Ordering) -> Value {
    use std::cmp::Ordering::*;
    Value::Bool(match op {
        BinOp::Eq => ord == Equal,
        BinOp::Ne => ord != Equal,
        BinOp::Lt => ord == Less,
        BinOp::Le => ord != Greater,
        BinOp::Gt => ord == Greater,
        _ => ord != Less,
    })
}

fn concat(a: &Value, b: &Value) -> R {
    let (kind, left, asc) = match (a, b) {
        (Value::Arr(x), _) => (x.kind, x.left, x.asc),
        (_, Value::Arr(y)) => (y.kind, y.left, y.asc),
        (Value::Ch(_), Value::Ch(_)) => (ArrKind::Slv, 1, false),
        _ => return Err("`&` needs array or element operands".into()),
    };
    let mut elems = Vec::new();
    for v in [a, b] {
        match v {
            Value::Arr(x) => elems.extend(x.elems.iter().cloned()),
            other => elems.push(other.clone()),
        }
    }
    // Result index range: left bound of the left operand, its direction
    // (`downto` vectors re-anchor so the rightmost index is 0).
    let n = elems.len() as i64;
    let left = if kind == ArrKind::Str || asc { left.min(1).max(if kind == ArrKind::Str { 1 } else { 0 }) } else { n - 1 };
    Ok(Value::Arr(Rc::new(ArrVal { kind, left, asc: asc || kind == ArrKind::Str, elems })))
}

fn shift(op: BinOp, a: &ArrVal, n: i64) -> Value {
    let len = a.elems.len();
    let mut out = a.elems.clone();
    if len == 0 {
        return Value::Arr(Rc::new(a.clone()));
    }
    let fill = match op {
        BinOp::Sra => a.elems[0].clone(),
        BinOp::Sla => a.elems[len - 1].clone(),
        _ => Value::Ch(b'0'),
    };
    let (op, n) = if n < 0 {
        (
            match op {
                BinOp::Sll => BinOp::Srl,
                BinOp::Srl => BinOp::Sll,
                BinOp::Sla => BinOp::Sra,
                BinOp::Sra => BinOp::Sla,
                BinOp::Rol => BinOp::Ror,
                _ => BinOp::Rol,
            },
            -n,
        )
    } else {
        (op, n)
    };
    let n = n as usize;
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = match op {
            // Elements move toward the left (index 0 is leftmost).
            BinOp::Sll | BinOp::Sla => a.elems.get(i + n).cloned().unwrap_or_else(|| fill.clone()),
            BinOp::Srl | BinOp::Sra => {
                if i >= n {
                    a.elems[i - n].clone()
                } else {
                    fill.clone()
                }
            }
            BinOp::Rol => a.elems[(i + n) % len].clone(),
            _ => a.elems[(i + len - n % len) % len].clone(),
        };
    }
    Value::Arr(Rc::new(ArrVal { kind: a.kind, left: a.left, asc: a.asc, elems: out }))
}

pub fn binop(op: BinOp, a: &Value, b: &Value) -> R {
    use BinOp::*;
    match op {
        And | Or | Nand | Nor | Xor | Xnor => match (a, b) {
            (Value::Bool(x), Value::Bool(y)) => Ok(Value::Bool(match op {
                And => *x && *y,
                Or => *x || *y,
                Nand => !(*x && *y),
                Nor => !(*x || *y),
                Xor => *x ^ *y,
                _ => *x == *y,
            })),
            (Value::Ch(x), Value::Ch(y)) => Ok(Value::Ch(logic2(op, *x, *y))),
            (Value::Arr(x), Value::Arr(y)) => zip_bits(op, x, y, |p, q| logic2(op, p, q)),
            (Value::Arr(x), Value::Ch(c)) => Ok(map_bits(x, |p| logic2(op, p, *c))),
            (Value::Ch(c), Value::Arr(y)) => Ok(map_bits(y, |q| logic2(op, *c, q))),
            _ => Err(format!("operator {op:?} on incompatible operands")),
        },
        Eq | Ne | Lt | Le | Gt | Ge => compare(op, a, b),
        Sll | Srl | Sla | Sra | Rol | Ror => match (a, b) {
            (Value::Arr(x), Value::Int(n)) => Ok(shift(op, x, *n)),
            _ => Err(format!("shift {op:?} needs an array and an integer")),
        },
        Concat => concat(a, b),
        Add | Sub | Mul | Div | Mod | Rem | Pow => arith(op, a, b),
    }
}

fn compare(op: BinOp, a: &Value, b: &Value) -> R {
    if let Some((_, _, x, y)) = numeric_operands(a, b) {
        return match (x, y) {
            (Some(x), Some(y)) => Ok(cmp_result(op, x.cmp(&y))),
            // numeric_std: comparisons with metavalues are false (`/=` true).
            _ => Ok(Value::Bool(op == BinOp::Ne)),
        };
    }
    let ord = match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Time(x), Value::Time(y)) => x.cmp(y),
        (Value::Real(x), Value::Real(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Enum(x), Value::Enum(y)) => x.cmp(y),
        (Value::Ch(x), Value::Ch(y)) => {
            // std_ulogic ordering is its enumeration position.
            let pos = |c: u8| b"UX01ZWLH-".iter().position(|p| *p == c).map(|p| p as i32).unwrap_or(c as i32 + 16);
            pos(*x).cmp(&pos(*y))
        }
        (Value::Arr(x), Value::Arr(y)) => {
            if matches!(op, BinOp::Eq | BinOp::Ne) {
                let eq = x.elems.len() == y.elems.len() && x.elems == y.elems;
                return Ok(Value::Bool(eq == (op == BinOp::Eq)));
            }
            let xs: Vec<u8> = x.elems.iter().map(|e| if let Value::Ch(c) = e { *c } else { 0 }).collect();
            let ys: Vec<u8> = y.elems.iter().map(|e| if let Value::Ch(c) = e { *c } else { 0 }).collect();
            xs.cmp(&ys)
        }
        (Value::Rec(x), Value::Rec(y)) if matches!(op, BinOp::Eq | BinOp::Ne) => {
            return Ok(Value::Bool((x == y) == (op == BinOp::Eq)));
        }
        _ => return Err("comparison of incompatible operands".into()),
    };
    Ok(cmp_result(op, ord))
}

fn arith(op: BinOp, a: &Value, b: &Value) -> R {
    use BinOp::*;
    if let Some((kind, n, x, y)) = numeric_operands(a, b) {
        let (Some(x), Some(y)) = (x, y) else {
            return Ok(all_x(kind, n));
        };
        let n = if op == Mul {
            match (a, b) {
                (Value::Arr(p), Value::Arr(q)) => p.elems.len() + q.elems.len(),
                _ => 2 * n,
            }
        } else {
            n
        };
        let r = match op {
            Add => x + y,
            Sub => x - y,
            Mul => x * y,
            Div => {
                if y == 0 {
                    return Err("numeric_std division by zero".into());
                }
                x / y
            }
            Rem => {
                if y == 0 {
                    return Err("numeric_std rem by zero".into());
                }
                x % y
            }
            Mod => {
                if y == 0 {
                    return Err("numeric_std mod by zero".into());
                }
                x.rem_euclid(y) * if y < 0 && x.rem_euclid(y) != 0 { 1 } else { 1 } + if y < 0 && x.rem_euclid(y) != 0 { y } else { 0 }
            }
            _ => return Err("`**` is not defined for numeric_std vectors".into()),
        };
        return Ok(int_to_vec(kind, r, n));
    }
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => {
            let (x, y) = (*x, *y);
            Ok(Value::Int(match op {
                Add => x.wrapping_add(y),
                Sub => x.wrapping_sub(y),
                Mul => x.wrapping_mul(y),
                Div => {
                    if y == 0 {
                        return Err("division by zero".into());
                    }
                    x / y
                }
                Rem => {
                    if y == 0 {
                        return Err("rem by zero".into());
                    }
                    x % y
                }
                Mod => {
                    if y == 0 {
                        return Err("mod by zero".into());
                    }
                    let r = x % y;
                    if r != 0 && (r < 0) != (y < 0) { r + y } else { r }
                }
                _ => {
                    if y < 0 {
                        return Err("integer ** with a negative exponent".into());
                    }
                    x.wrapping_pow(y as u32)
                }
            }))
        }
        (Value::Real(x), Value::Real(y)) => Ok(Value::Real(match op {
            Add => x + y,
            Sub => x - y,
            Mul => x * y,
            Div => x / y,
            _ => return Err(format!("operator {op:?} on real")),
        })),
        (Value::Real(x), Value::Int(y)) if op == Pow => Ok(Value::Real(x.powi(*y as i32))),
        (Value::Time(x), Value::Time(y)) => match op {
            Add => Ok(Value::Time(x + y)),
            Sub => Ok(Value::Time(x - y)),
            Div if *y != 0 => Ok(Value::Int(x / y)),
            Mod | Rem if *y != 0 => Ok(Value::Time(x % y)),
            _ => Err(format!("operator {op:?} on time")),
        },
        (Value::Time(x), Value::Int(y)) => match op {
            Mul => Ok(Value::Time(x * y)),
            Div if *y != 0 => Ok(Value::Time(x / y)),
            _ => Err(format!("operator {op:?} on time and integer")),
        },
        (Value::Int(x), Value::Time(y)) if op == Mul => Ok(Value::Time(x * y)),
        (Value::Time(x), Value::Real(y)) => match op {
            Mul => Ok(Value::Time((*x as f64 * y).round() as i64)),
            Div => Ok(Value::Time((*x as f64 / y).round() as i64)),
            _ => Err(format!("operator {op:?} on time and real")),
        },
        (Value::Real(x), Value::Time(y)) if op == Mul => Ok(Value::Time((*x * *y as f64).round() as i64)),
        _ => Err(format!("operator {op:?} on incompatible operands")),
    }
}

pub fn unop(op: UnOp, a: &Value) -> R {
    match (op, a) {
        (UnOp::Plus, v) => Ok(v.clone()),
        (UnOp::Neg, Value::Int(x)) => Ok(Value::Int(x.wrapping_neg())),
        (UnOp::Neg, Value::Real(x)) => Ok(Value::Real(-x)),
        (UnOp::Neg, Value::Time(x)) => Ok(Value::Time(-x)),
        (UnOp::Neg, Value::Arr(x)) if x.kind == ArrKind::Signed => match vec_to_int(a, true) {
            Some(v) => Ok(int_to_vec(ArrKind::Signed, -v, x.elems.len())),
            None => Ok(all_x(ArrKind::Signed, x.elems.len())),
        },
        (UnOp::Abs, Value::Int(x)) => Ok(Value::Int(x.wrapping_abs())),
        (UnOp::Abs, Value::Real(x)) => Ok(Value::Real(x.abs())),
        (UnOp::Abs, Value::Time(x)) => Ok(Value::Time(x.abs())),
        (UnOp::Abs, Value::Arr(x)) if x.kind == ArrKind::Signed => match vec_to_int(a, true) {
            Some(v) => Ok(int_to_vec(ArrKind::Signed, v.abs(), x.elems.len())),
            None => Ok(all_x(ArrKind::Signed, x.elems.len())),
        },
        (UnOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (UnOp::Not, Value::Ch(c)) => Ok(Value::Ch(sl_not(*c))),
        (UnOp::Not, Value::Arr(x)) => Ok(map_bits(x, sl_not)),
        (UnOp::Cond, Value::Ch(c)) => Ok(Value::Bool(matches!(c, b'1' | b'H'))),
        (UnOp::Cond, Value::Bool(b)) => Ok(Value::Bool(*b)),
        (UnOp::RedAnd | UnOp::RedOr | UnOp::RedXor, Value::Arr(x)) => {
            let (init, f): (u8, fn(u8, u8) -> u8) = match op {
                UnOp::RedAnd => (b'1', sl_and),
                UnOp::RedOr => (b'0', sl_or),
                _ => (b'0', sl_xor),
            };
            Ok(Value::Ch(x.elems.iter().fold(init, |acc, e| match e {
                Value::Ch(c) => f(acc, *c),
                _ => acc,
            })))
        }
        _ => Err(format!("operator {op:?} on an incompatible operand")),
    }
}

/// `resize` (numeric_std): sign-extends `signed`, zero-extends `unsigned`;
/// signed truncation keeps the sign bit (IEEE 1076.3).
pub fn resize(v: &Value, n: usize) -> R {
    let Some(a) = arr_parts(v) else {
        return Err("resize needs an unsigned/signed vector".into());
    };
    let bits = v.bits().unwrap_or_default();
    let len = bits.len();
    let signed = a.kind == ArrKind::Signed;
    let mut out = Vec::with_capacity(n);
    if n >= len {
        let fill = if signed && len > 0 { bits[0] } else { b'0' };
        out.extend(std::iter::repeat_n(fill, n - len));
        out.extend(bits);
    } else if signed && n > 0 {
        out.push(bits[0]);
        out.extend_from_slice(&bits[len - (n - 1)..]);
    } else {
        out.extend_from_slice(&bits[len - n..]);
    }
    Ok(Value::vec(a.kind, &out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(bits: &[u8]) -> Value {
        Value::vec(ArrKind::Unsigned, bits)
    }

    #[test]
    fn numeric_std_arithmetic_and_compare() {
        let a = u(b"0011");
        let r = binop(BinOp::Add, &a, &Value::Int(1)).unwrap();
        assert_eq!(r.bits().unwrap(), b"0100".to_vec());
        let wrap = binop(BinOp::Add, &u(b"1111"), &Value::Int(1)).unwrap();
        assert_eq!(wrap.bits().unwrap(), b"0000".to_vec());
        assert_eq!(binop(BinOp::Lt, &a, &Value::Int(4)).unwrap(), Value::Bool(true));
        let s = Value::vec(ArrKind::Signed, b"1110");
        assert_eq!(binop(BinOp::Lt, &s, &Value::Int(0)).unwrap(), Value::Bool(true));
        let p = binop(BinOp::Mul, &a, &u(b"0010")).unwrap();
        assert_eq!(p.bits().unwrap().len(), 8);
        assert_eq!(vec_to_int(&p, false), Some(6));
        let x = binop(BinOp::Add, &u(b"00X1"), &Value::Int(1)).unwrap();
        assert_eq!(x.bits().unwrap(), b"XXXX".to_vec());
    }

    #[test]
    fn logic_shift_concat_resize() {
        let a = Value::vec(ArrKind::Slv, b"1100");
        let b = Value::vec(ArrKind::Slv, b"1010");
        assert_eq!(binop(BinOp::Xor, &a, &b).unwrap().bits().unwrap(), b"0110".to_vec());
        assert_eq!(binop(BinOp::Sll, &a, &Value::Int(1)).unwrap().bits().unwrap(), b"1000".to_vec());
        assert_eq!(binop(BinOp::Srl, &a, &Value::Int(1)).unwrap().bits().unwrap(), b"0110".to_vec());
        let c = binop(BinOp::Concat, &a, &Value::Ch(b'1')).unwrap();
        assert_eq!(c.bits().unwrap(), b"11001".to_vec());
        let s = Value::vec(ArrKind::Signed, b"1010");
        assert_eq!(resize(&s, 6).unwrap().bits().unwrap(), b"111010".to_vec());
        assert_eq!(binop(BinOp::Mod, &Value::Int(-7), &Value::Int(3)).unwrap(), Value::Int(2));
        assert_eq!(binop(BinOp::Rem, &Value::Int(-7), &Value::Int(3)).unwrap(), Value::Int(-1));
        assert_eq!(binop(BinOp::Div, &Value::Time(10 * FS_PER_NS), &Value::Int(2)).unwrap(), Value::Time(5 * FS_PER_NS));
    }
}
