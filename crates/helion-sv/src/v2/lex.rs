//! Tokens of preprocessed SystemVerilog.

/// A number literal. `bits` and `xz` are LSB first; a bit with `xz` set is
/// `x`, `z` or `?` (its `bits` entry is true for `z`/`?`).
#[derive(Clone, Debug, PartialEq)]
pub struct Num {
    pub bits: Vec<bool>,
    pub xz: Vec<bool>,
    /// The literal has an explicit size (`8'h..`) and that many bits.
    pub sized: bool,
    pub signed: bool,
    /// `'0`, `'1`, `'x`, `'z`: fills the context width.
    pub fill: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum T {
    Id(String),
    /// `$name`
    Sys(String),
    Num(Num),
    Str(String),
    Op(&'static str),
    Eof,
}

#[derive(Clone, Debug)]
pub struct Tok {
    pub t: T,
    pub line: u32,
}

const OPS: &[&str] = &[
    "<<<=", ">>>=", "===", "!==", "==?", "!=?", "<<<", ">>>", "<<=", ">>=", "&&&", "|->", "|=>", "&&", "||", "==", "!=", "<=", ">=", "<<", ">>", "**",
    "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "++", "--", "->", "::", "+:", "-:", "~&", "~|", "~^", "^~", "'{", "(", ")", "[", "]", "{", "}",
    ",", ";", ":", ".", "+", "-", "*", "/", "%", "&", "|", "^", "~", "!", "<", ">", "=", "?", "@", "#", "'", "$",
];

fn digit_val(c: char) -> Option<u32> {
    c.to_digit(16)
}

/// Parse the digits of a based literal into (bits, xz), LSB first.
fn based_bits(digits: &str, base: u32, line: u32) -> Result<(Vec<bool>, Vec<bool>), String> {
    let per = match base {
        2 => 1,
        8 => 3,
        16 => 4,
        _ => 0,
    };
    let clean: Vec<char> = digits.chars().filter(|c| *c != '_').collect();
    if clean.is_empty() {
        return Err(format!("line {line}: number without digits"));
    }
    if per == 0 {
        // Decimal: all x, all z, or digits.
        if clean.len() == 1 && matches!(clean[0], 'x' | 'X' | 'z' | 'Z' | '?') {
            let z = !matches!(clean[0], 'x' | 'X');
            return Ok((vec![z], vec![true]));
        }
        let mut bits: Vec<bool> = vec![false];
        for c in &clean {
            let d = c.to_digit(10).ok_or_else(|| format!("line {line}: bad decimal digit '{c}'"))?;
            // bits = bits * 10 + d
            let mut carry = d;
            for b in bits.iter_mut() {
                let v = (*b as u32) * 10 + carry;
                *b = v & 1 == 1;
                carry = v >> 1;
            }
            while carry > 0 {
                bits.push(carry & 1 == 1);
                carry >>= 1;
            }
        }
        let n = bits.len();
        return Ok((bits, vec![false; n]));
    }
    let mut bits = Vec::new();
    let mut xz = Vec::new();
    for c in clean.iter().rev() {
        match c {
            'x' | 'X' => {
                bits.extend(std::iter::repeat_n(false, per));
                xz.extend(std::iter::repeat_n(true, per));
            }
            'z' | 'Z' | '?' => {
                bits.extend(std::iter::repeat_n(true, per));
                xz.extend(std::iter::repeat_n(true, per));
            }
            _ => {
                let d = digit_val(*c).filter(|d| *d < base).ok_or_else(|| format!("line {line}: bad digit '{c}' for base {base}"))?;
                for i in 0..per {
                    bits.push((d >> i) & 1 == 1);
                    xz.push(false);
                }
            }
        }
    }
    Ok((bits, xz))
}

fn to_u32(bits: &[bool]) -> u32 {
    bits.iter().enumerate().take(32).fold(0, |a, (i, &b)| a | ((b as u32) << i))
}

pub fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut line = 1u32;
    let is_id = |ch: char| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$';
    while i < c.len() {
        let ch = c[i];
        if ch == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if ch == '/' && i + 1 < c.len() && c[i + 1] == '/' {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if ch == '/' && i + 1 < c.len() && c[i + 1] == '*' {
            i += 2;
            while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == '/') {
                if c[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
            continue;
        }
        // Attribute instance `(* ... *)`, but not the `(*)` of `@(*)`.
        if ch == '(' && i + 1 < c.len() && c[i + 1] == '*' {
            let mut j = i + 2;
            while j < c.len() && c[j].is_whitespace() {
                j += 1;
            }
            if j < c.len() && c[j] != ')' {
                i += 2;
                while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == ')') {
                    if c[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i += 2;
                continue;
            }
        }
        if ch == '`' {
            // `line N F` from the preprocessor: the next line is line N of file F.
            let st = i;
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
            let text: String = c[st..i].iter().collect();
            let mut f = text.split_whitespace();
            if f.next() == Some("`line") {
                if let (Some(n), Some(file)) = (f.next().and_then(|x| x.parse::<u32>().ok()), f.next().and_then(|x| x.parse::<u32>().ok())) {
                    line = file * super::pre::FILE_STRIDE + n.saturating_sub(1);
                }
            }
            continue;
        }
        if ch == '"' {
            let mut s = String::new();
            i += 1;
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' && i + 1 < c.len() {
                    i += 1;
                    s.push(match c[i] {
                        'n' => '\n',
                        't' => '\t',
                        x => x,
                    });
                } else {
                    if c[i] == '\n' {
                        line += 1;
                    }
                    s.push(c[i]);
                }
                i += 1;
            }
            i += 1;
            out.push(Tok { t: T::Str(s), line });
            continue;
        }
        if ch == '\\' {
            // Escaped identifier: up to white space.
            let st = i + 1;
            i += 1;
            while i < c.len() && !c[i].is_whitespace() {
                i += 1;
            }
            out.push(Tok { t: T::Id(c[st..i].iter().collect()), line });
            continue;
        }
        if ch == '$' && i + 1 < c.len() && (c[i + 1].is_ascii_alphabetic() || c[i + 1] == '_') {
            let st = i + 1;
            i += 1;
            while i < c.len() && is_id(c[i]) {
                i += 1;
            }
            out.push(Tok { t: T::Sys(c[st..i].iter().collect()), line });
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            let st = i;
            while i < c.len() && is_id(c[i]) {
                i += 1;
            }
            out.push(Tok { t: T::Id(c[st..i].iter().collect()), line });
            continue;
        }
        // Numbers: [size] ' [s] base digits | ' 0/1/x/z | decimal.
        let based = |i: usize| -> bool {
            // at c[i] == '\'' followed by optional s and a base letter
            let mut j = i + 1;
            if j < c.len() && (c[j] == 's' || c[j] == 'S') {
                j += 1;
            }
            j < c.len() && matches!(c[j], 'b' | 'B' | 'o' | 'O' | 'd' | 'D' | 'h' | 'H')
        };
        if ch.is_ascii_digit() || (ch == '\'' && (based(i) || (i + 1 < c.len() && matches!(c[i + 1], '0' | '1' | 'x' | 'X' | 'z' | 'Z')))) {
            let mut size: Option<u32> = None;
            if ch.is_ascii_digit() {
                let st = i;
                while i < c.len() && (c[i].is_ascii_digit() || c[i] == '_') {
                    i += 1;
                }
                let digits: String = c[st..i].iter().collect();
                // Look ahead for a base, allowing spaces before the quote.
                let mut j = i;
                while j < c.len() && (c[j] == ' ' || c[j] == '\t') {
                    j += 1;
                }
                if j < c.len() && c[j] == '\'' && based(j) {
                    let (b, _) = based_bits(&digits, 10, line)?;
                    size = Some(to_u32(&b));
                    i = j;
                } else {
                    if i < c.len() && (c[i] == '.' && i + 1 < c.len() && c[i + 1].is_ascii_digit()) {
                        return Err(format!("line {line}: real number literals are not synthesizable"));
                    }
                    // Time literal suffix (1ns, 10ps) only appears in delays, which are skipped.
                    let (mut bits, _) = based_bits(&digits, 10, line)?;
                    // Unsized decimal: a signed 32-bit integer (wider if it needs it).
                    let w = bits.len().max(31) + 1;
                    bits.resize(w.max(32), false);
                    let n = bits.len();
                    out.push(Tok { t: T::Num(Num { bits, xz: vec![false; n], sized: false, signed: true, fill: false }), line });
                    while i < c.len() && c[i].is_ascii_alphabetic() {
                        i += 1; // time unit
                    }
                    continue;
                }
            }
            // c[i] == '\''
            i += 1;
            if size.is_none() && i < c.len() && matches!(c[i], '0' | '1' | 'x' | 'X' | 'z' | 'Z') && !(i + 1 < c.len() && is_id(c[i + 1])) {
                let (b, x) = match c[i] {
                    '0' => (false, false),
                    '1' => (true, false),
                    'x' | 'X' => (false, true),
                    _ => (true, true),
                };
                i += 1;
                out.push(Tok { t: T::Num(Num { bits: vec![b], xz: vec![x], sized: false, signed: false, fill: true }), line });
                continue;
            }
            let mut signed = false;
            if i < c.len() && (c[i] == 's' || c[i] == 'S') {
                signed = true;
                i += 1;
            }
            let base = match c.get(i) {
                Some('b') | Some('B') => 2,
                Some('o') | Some('O') => 8,
                Some('d') | Some('D') => 10,
                Some('h') | Some('H') => 16,
                _ => return Err(format!("line {line}: bad number base")),
            };
            i += 1;
            while i < c.len() && (c[i] == ' ' || c[i] == '\t') {
                i += 1;
            }
            let st = i;
            while i < c.len() && (c[i].is_ascii_hexdigit() || matches!(c[i], '_' | 'x' | 'X' | 'z' | 'Z' | '?')) {
                i += 1;
            }
            let digits: String = c[st..i].iter().collect();
            let (mut bits, mut xz) = based_bits(&digits, base, line)?;
            let w = size.unwrap_or(32.max(bits.len() as u32)) as usize;
            if w == 0 {
                return Err(format!("line {line}: zero-width literal"));
            }
            // Extend with 0, or with x/z when the top digit is x/z.
            let (fb, fx) = match (bits.last(), xz.last()) {
                (Some(&b), Some(true)) => (b, true),
                _ => (false, false),
            };
            bits.resize(w, fb);
            xz.resize(w, fx);
            out.push(Tok { t: T::Num(Num { bits, xz, sized: size.is_some(), signed, fill: false }), line });
            continue;
        }
        let rest: String = c[i..(i + 4).min(c.len())].iter().collect();
        let Some(op) = OPS.iter().find(|o| rest.starts_with(**o)) else {
            return Err(format!("line {line}: unexpected character '{ch}'"));
        };
        i += op.len();
        out.push(Tok { t: T::Op(op), line });
    }
    out.push(Tok { t: T::Eof, line });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nums(s: &str) -> Vec<Num> {
        lex(s)
            .unwrap()
            .into_iter()
            .filter_map(|t| match t.t {
                T::Num(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn numbers() {
        let n = nums("8'hA5 4'b1x0z 'd10 12 '1 3'sd3 16'h_ff 4 'b101");
        assert_eq!(to_u32(&n[0].bits), 0xA5);
        assert!(n[0].sized && n[0].bits.len() == 8);
        assert_eq!(n[1].xz, vec![true, false, true, false]);
        assert_eq!(n[1].bits, vec![true, false, false, true]);
        assert_eq!((to_u32(&n[2].bits), n[2].bits.len(), n[2].sized), (10, 32, false));
        assert_eq!((to_u32(&n[3].bits), n[3].bits.len(), n[3].signed), (12, 32, true));
        assert!(n[4].fill && n[4].bits == vec![true]);
        assert!(n[5].signed && n[5].bits.len() == 3);
        assert_eq!((to_u32(&n[6].bits), n[6].bits.len()), (0xff, 16));
        assert_eq!((to_u32(&n[7].bits), n[7].bits.len(), n[7].sized), (5, 4, true));
    }

    #[test]
    fn operators_and_attributes() {
        let t = lex("always @(*) (* keep = 1 *) a <<<= b'{c}; x <= y ==? z;").unwrap();
        let ops: Vec<&str> = t
            .iter()
            .filter_map(|t| match &t.t {
                T::Op(o) => Some(*o),
                _ => None,
            })
            .collect();
        assert_eq!(ops, vec!["@", "(", "*", ")", "<<<=", "'{", "}", ";", "<=", "==?", ";"]);
    }
}
