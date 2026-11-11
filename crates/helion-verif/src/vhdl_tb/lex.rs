//! VHDL lexer for the behavioral testbench simulator.
//!
//! Identifiers and keywords are lower-cased (VHDL is case-insensitive);
//! every token carries its 1-based source line so parse errors and run-time
//! failures point at the testbench source.

use crate::tb::TbError;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    /// Identifier or keyword, lower-cased. Extended identifiers (`\x\`) are
    /// kept verbatim.
    Id(String),
    Int(i64),
    Real(f64),
    /// Character literal `'x'`.
    Char(u8),
    /// String literal `"..."`.
    Str(String),
    /// Bit-string literal (`x"0F"`, `b"0101"`, `o"7"`), expanded to `0`/`1`
    /// characters (and `X`/`Z`/`-` where the source spells them).
    BitStr(String),
    /// Delimiter or compound delimiter (`<=`, `:=`, `=>`, `'` ...).
    Sym(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spanned {
    pub tok: Tok,
    pub line: usize,
}

const KEYWORDS: &[&str] = &[
    "abs", "access", "after", "alias", "all", "and", "architecture", "array", "assert", "attribute",
    "begin", "block", "body", "buffer", "bus", "case", "component", "configuration", "constant",
    "context", "disconnect", "downto", "else", "elsif", "end", "entity", "exit", "file", "for",
    "force", "function", "generate", "generic", "group", "guarded", "if", "impure", "in",
    "inertial", "inout", "is", "label", "library", "linkage", "literal", "loop", "map", "mod",
    "nand", "new", "next", "nor", "not", "null", "of", "on", "open", "or", "others", "out",
    "package", "port", "postponed", "procedure", "process", "protected", "pure", "range",
    "record", "register", "reject", "release", "rem", "report", "return", "rol", "ror", "select",
    "severity", "shared", "signal", "sla", "sll", "sra", "srl", "subtype", "then", "to",
    "transport", "type", "unaffected", "units", "until", "use", "variable", "wait", "when",
    "while", "with", "xnor", "xor",
];

pub fn is_keyword(s: &str) -> bool {
    KEYWORDS.contains(&s)
}

const SYMS2: &[&str] = &["<=", ":=", "=>", "/=", ">=", "**", "<>", "??", "?=", "?/"];
const SYMS1: &[&str] = &["(", ")", ",", ";", ":", ".", "'", "&", "+", "-", "*", "/", "<", ">", "=", "|", "[", "]", "@"];

fn sym2(s: &str) -> Option<&'static str> {
    SYMS2.iter().copied().find(|x| *x == s)
}

fn sym1(c: char) -> Option<&'static str> {
    let mut b = [0u8; 4];
    let s = c.encode_utf8(&mut b);
    SYMS1.iter().copied().find(|x| *x == s)
}

/// Expand a bit-string literal body for `base` (`b`, `o`, `x`, `d`).
fn expand_bitstr(file: &str, line: usize, base: char, body: &str, width: Option<usize>) -> Result<String, TbError> {
    let digits: String = body.chars().filter(|c| *c != '_').collect();
    let mut out = String::new();
    match base {
        'b' => {
            for c in digits.chars() {
                out.push(c.to_ascii_uppercase());
            }
        }
        'o' | 'x' => {
            let bits = if base == 'o' { 3 } else { 4 };
            for c in digits.chars() {
                match c.to_digit(if base == 'o' { 8 } else { 16 }) {
                    Some(v) => {
                        for i in (0..bits).rev() {
                            out.push(if (v >> i) & 1 == 1 { '1' } else { '0' });
                        }
                    }
                    None => {
                        let u = c.to_ascii_uppercase();
                        if matches!(u, 'X' | 'Z' | '-' | 'U' | 'W' | 'L' | 'H') {
                            for _ in 0..bits {
                                out.push(u);
                            }
                        } else {
                            return Err(TbError::new(file, line, format!("bad digit `{c}` in bit-string literal")));
                        }
                    }
                }
            }
        }
        'd' => {
            let v: u128 = digits
                .parse()
                .map_err(|_| TbError::new(file, line, format!("bad decimal bit-string `{body}`")))?;
            let w = width.unwrap_or_else(|| (128 - v.leading_zeros()).max(1) as usize);
            for i in (0..w).rev() {
                out.push(if i < 128 && (v >> i) & 1 == 1 { '1' } else { '0' });
            }
            return Ok(out);
        }
        _ => return Err(TbError::new(file, line, format!("unknown bit-string base `{base}`"))),
    }
    if let Some(w) = width {
        if out.len() > w {
            out = out[out.len() - w..].to_string();
        } else {
            while out.len() < w {
                out.insert(0, '0');
            }
        }
    }
    Ok(out)
}

pub fn lex(file: &str, src: &str) -> Result<Vec<Spanned>, TbError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out: Vec<Spanned> = Vec::new();
    let mut i = 0usize;
    let mut line = 1usize;
    let n = chars.len();
    while i < n {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Comments.
        if c == '-' && i + 1 < n && chars[i + 1] == '-' {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < n && !(chars[i] == '*' && chars[i + 1] == '/') {
                if chars[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
            continue;
        }
        // Identifiers, keywords, bit-string literals (incl. sized 8x"FF").
        if c.is_ascii_alphabetic() {
            let start = i;
            while i < n && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect::<String>().to_ascii_lowercase();
            if i < n && chars[i] == '"' && matches!(word.as_str(), "b" | "o" | "x" | "d" | "ub" | "uo" | "ux" | "sb" | "so" | "sx") {
                let base = word.chars().last().unwrap_or('b');
                let (body, ni, nl) = read_string(file, &chars, i, line)?;
                i = ni;
                let bits = expand_bitstr(file, line, base, &body, None)?;
                line = nl;
                out.push(Spanned { tok: Tok::BitStr(bits), line });
                continue;
            }
            out.push(Spanned { tok: Tok::Id(word), line });
            continue;
        }
        if c == '\\' {
            let start = i;
            i += 1;
            while i < n && chars[i] != '\\' {
                i += 1;
            }
            i += 1;
            out.push(Spanned { tok: Tok::Id(chars[start..i.min(n)].iter().collect()), line });
            continue;
        }
        // Numbers: decimal, based (16#FF#), real, exponent, sized bit-strings.
        if c.is_ascii_digit() {
            let start = i;
            while i < n && (chars[i].is_ascii_digit() || chars[i] == '_') {
                i += 1;
            }
            let int_part: String = chars[start..i].iter().filter(|c| **c != '_').collect();
            // Sized bit-string: 8x"FF"
            if i + 1 < n && chars[i].is_ascii_alphabetic() && {
                let mut j = i;
                while j < n && chars[j].is_ascii_alphabetic() {
                    j += 1;
                }
                j < n && chars[j] == '"'
            } {
                let bstart = i;
                while chars[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let base_word: String = chars[bstart..i].iter().collect::<String>().to_ascii_lowercase();
                let base = base_word.chars().last().unwrap_or('b');
                let (body, ni, nl) = read_string(file, &chars, i, line)?;
                i = ni;
                let w: usize = int_part.parse().unwrap_or(0);
                let bits = expand_bitstr(file, line, base, &body, Some(w))?;
                line = nl;
                out.push(Spanned { tok: Tok::BitStr(bits), line });
                continue;
            }
            if i < n && chars[i] == '#' {
                let base: u32 = int_part.parse().unwrap_or(10);
                i += 1;
                let bstart = i;
                while i < n && chars[i] != '#' {
                    i += 1;
                }
                let digits: String = chars[bstart..i].iter().filter(|c| **c != '_').collect();
                i += 1;
                let v = i64::from_str_radix(&digits, base)
                    .map_err(|_| TbError::new(file, line, format!("bad based literal {base}#{digits}#")))?;
                out.push(Spanned { tok: Tok::Int(v), line });
                continue;
            }
            let mut is_real = false;
            let mut text = int_part;
            if i + 1 < n && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                is_real = true;
                text.push('.');
                i += 1;
                while i < n && (chars[i].is_ascii_digit() || chars[i] == '_') {
                    if chars[i] != '_' {
                        text.push(chars[i]);
                    }
                    i += 1;
                }
            }
            let mut exp: i32 = 0;
            if i < n && (chars[i] == 'e' || chars[i] == 'E') {
                let mut j = i + 1;
                let mut sign = 1;
                if j < n && (chars[j] == '+' || chars[j] == '-') {
                    if chars[j] == '-' {
                        sign = -1;
                    }
                    j += 1;
                }
                if j < n && chars[j].is_ascii_digit() {
                    let es = j;
                    while j < n && chars[j].is_ascii_digit() {
                        j += 1;
                    }
                    let e: i32 = chars[es..j].iter().collect::<String>().parse().unwrap_or(0);
                    exp = sign * e;
                    i = j;
                }
            }
            if is_real {
                let v: f64 = text.parse().unwrap_or(0.0);
                out.push(Spanned { tok: Tok::Real(v * 10f64.powi(exp)), line });
            } else {
                let mut v: i64 = text
                    .parse()
                    .map_err(|_| TbError::new(file, line, format!("integer literal {text} out of range")))?;
                for _ in 0..exp.max(0) {
                    v = v.saturating_mul(10);
                }
                out.push(Spanned { tok: Tok::Int(v), line });
            }
            continue;
        }
        if c == '"' {
            let (body, ni, nl) = read_string(file, &chars, i, line)?;
            i = ni;
            line = nl;
            out.push(Spanned { tok: Tok::Str(body), line });
            continue;
        }
        if c == '\'' {
            // Character literal vs attribute tick: after an identifier
            // (not a keyword) or `)` the apostrophe is a tick.
            let tick = match out.last().map(|t| &t.tok) {
                Some(Tok::Id(w)) => !is_keyword(w) || w == "all",
                Some(Tok::Sym(")")) | Some(Tok::Sym("]")) => true,
                _ => false,
            };
            // `'('` after an identifier is a qualified expression tick
            // followed by `(`; `'x'` elsewhere is a character literal.
            if !tick && i + 2 < n && chars[i + 2] == '\'' {
                out.push(Spanned { tok: Tok::Char(chars[i + 1] as u8), line });
                i += 3;
                continue;
            }
            out.push(Spanned { tok: Tok::Sym("'"), line });
            i += 1;
            continue;
        }
        if i + 1 < n {
            let two: String = chars[i..i + 2].iter().collect();
            if let Some(s) = sym2(&two) {
                out.push(Spanned { tok: Tok::Sym(s), line });
                i += 2;
                continue;
            }
        }
        if let Some(s) = sym1(c) {
            out.push(Spanned { tok: Tok::Sym(s), line });
            i += 1;
            continue;
        }
        return Err(TbError::new(file, line, format!("unexpected character `{c}`")));
    }
    Ok(out)
}

/// Read a `"..."` literal starting at `chars[i] == '"'` (`""` escapes a
/// quote). Returns (body, next index, line).
fn read_string(file: &str, chars: &[char], mut i: usize, line: usize) -> Result<(String, usize, usize), TbError> {
    let mut s = String::new();
    i += 1;
    loop {
        if i >= chars.len() || chars[i] == '\n' {
            return Err(TbError::new(file, line, "unterminated string literal"));
        }
        if chars[i] == '"' {
            if i + 1 < chars.len() && chars[i + 1] == '"' {
                s.push('"');
                i += 2;
                continue;
            }
            return Ok((s, i + 1, line));
        }
        s.push(chars[i]);
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex("t.vhd", s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn char_literals_vs_attribute_ticks() {
        assert_eq!(
            toks("x <= '1'; if clk'event then"),
            vec![
                Tok::Id("x".into()),
                Tok::Sym("<="),
                Tok::Char(b'1'),
                Tok::Sym(";"),
                Tok::Id("if".into()),
                Tok::Id("clk".into()),
                Tok::Sym("'"),
                Tok::Id("event".into()),
                Tok::Id("then".into()),
            ]
        );
        assert_eq!(toks("when '0' =>")[1], Tok::Char(b'0'));
        // Qualified expression: std_logic'('1')
        let t = toks("std_logic'('1')");
        assert_eq!(t[1], Tok::Sym("'"));
        assert_eq!(t[3], Tok::Char(b'1'));
    }

    #[test]
    fn literals() {
        assert_eq!(toks("16#FF#"), vec![Tok::Int(255)]);
        assert_eq!(toks("x\"A5\""), vec![Tok::BitStr("10100101".into())]);
        assert_eq!(toks("6x\"F\""), vec![Tok::BitStr("001111".into())]);
        assert_eq!(toks("1_000"), vec![Tok::Int(1000)]);
        assert_eq!(toks("2.5"), vec![Tok::Real(2.5)]);
        assert_eq!(toks("\"a\"\"b\""), vec![Tok::Str("a\"b".into())]);
        assert_eq!(toks("Foo -- c\nBAR"), vec![Tok::Id("foo".into()), Tok::Id("bar".into())]);
    }
}
