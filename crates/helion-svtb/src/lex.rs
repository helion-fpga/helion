//! Lexer for the testbench language (IEEE 1800 lexical conventions).
//!
//! Produces [`Token`]s tagged with a file id and 1-based line. Compiler
//! directives are lexed as [`Tok::Directive`]; the ones whose argument is
//! the rest of the line (`` `define``, `` `timescale``, …) carry it raw in
//! [`Tok::LineDirective`] so the preprocessor can parse macro bodies.

use crate::SvError;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Id(String),
    /// `$display`, `$urandom`, …
    Sys(String),
    /// Integer literal: value, explicit width, signed, original text.
    Num { val: u64, width: Option<u32>, signed: bool, text: String },
    /// `'0` / `'1` / `'x` / `'z` fill literal (bit value; x/z read as 0).
    Fill(u8),
    Real(f64, String),
    /// Time literal in picoseconds (`10ns`).
    Time(u64, String),
    Str(String),
    Op(&'static str),
    /// `` `name `` (macro use or conditional directive).
    Directive(String),
    /// Directive whose argument is the rest of the line: (name, raw text).
    LineDirective(String, String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub file: u32,
    pub line: u32,
}

impl Token {
    /// Source text of the token (macro argument reconstruction).
    pub fn src(&self) -> String {
        match &self.tok {
            Tok::Id(s) => s.clone(),
            Tok::Sys(s) => s.clone(),
            Tok::Num { text, .. } => text.clone(),
            Tok::Fill(b) => format!("'{b}"),
            Tok::Real(_, t) | Tok::Time(_, t) => t.clone(),
            Tok::Str(s) => {
                let mut o = String::from("\"");
                for c in s.chars() {
                    match c {
                        '"' => o.push_str("\\\""),
                        '\\' => o.push_str("\\\\"),
                        '\n' => o.push_str("\\n"),
                        '\t' => o.push_str("\\t"),
                        c => o.push(c),
                    }
                }
                o.push('"');
                o
            }
            Tok::Op(o) => (*o).to_string(),
            Tok::Directive(d) => format!("`{d}"),
            Tok::LineDirective(d, r) => format!("`{d} {r}"),
        }
    }
}

const OPS: &[&str] = &[
    "<<<=", ">>>=", "===", "!==", "==?", "!=?", "<<<", ">>>", "<<=", ">>=", "->>", "<->", "::", "==",
    "!=", "<=", ">=", "&&", "||", "**", "<<", ">>", "->", "++", "--", "+=", "-=", "*=", "/=", "%=",
    "&=", "|=", "^=", "~&", "~|", "~^", "^~", "+:", "-:", ":=", ":/", "##", ".*", "+", "-", "*", "/",
    "%", "&", "|", "^", "~", "!", "<", ">", "=", "?", ":", ";", ",", ".", "(", ")", "[", "]", "{",
    "}", "#", "@", "'", "$",
];

/// Directives that consume the rest of their line.
fn is_line_directive(name: &str) -> bool {
    matches!(
        name,
        "define"
            | "timescale"
            | "default_nettype"
            | "line"
            | "pragma"
            | "resetall"
            | "celldefine"
            | "endcelldefine"
            | "unconnected_drive"
            | "nounconnected_drive"
            | "undefineall"
            | "begin_keywords"
            | "end_keywords"
    )
}

fn time_unit_ps(u: &str) -> Option<f64> {
    Some(match u {
        "fs" => 0.001,
        "ps" => 1.0,
        "ns" => 1_000.0,
        "us" => 1_000_000.0,
        "ms" => 1_000_000_000.0,
        "s" => 1_000_000_000_000.0,
        _ => return None,
    })
}

pub fn lex(file: u32, label: &str, src: &str) -> Result<Vec<Token>, SvError> {
    let b: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let mut line = 1u32;
    let mut out = Vec::new();
    let err = |line: u32, m: String| SvError::new(label, line as usize, m);
    while i < b.len() {
        let c = b[i];
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
        if c == '/' && i + 1 < b.len() && b[i + 1] == '/' {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < b.len() && b[i + 1] == '*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                if b[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
            continue;
        }
        let start_line = line;
        let push = |out: &mut Vec<Token>, tok: Tok| out.push(Token { tok, file, line: start_line });
        // Directives.
        if c == '`' {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_alphanumeric() || b[j] == '_') {
                j += 1;
            }
            let name: String = b[i + 1..j].iter().collect();
            if name.is_empty() {
                return Err(err(line, "stray ` in source".into()));
            }
            i = j;
            if is_line_directive(&name) {
                // Capture the rest of the line (with `\` continuations for
                // `define), dropping `//` comments outside strings.
                let mut raw = String::new();
                let mut in_str = false;
                while i < b.len() {
                    let ch = b[i];
                    if ch == '\\' && i + 1 < b.len() && b[i + 1] == '\n' {
                        raw.push('\n');
                        line += 1;
                        i += 2;
                        continue;
                    }
                    if ch == '\\' && i + 2 < b.len() && b[i + 1] == '\r' && b[i + 2] == '\n' {
                        raw.push('\n');
                        line += 1;
                        i += 3;
                        continue;
                    }
                    if ch == '\n' {
                        break;
                    }
                    if ch == '"' && (i == 0 || b[i - 1] != '\\') {
                        in_str = !in_str;
                    }
                    if !in_str && ch == '/' && i + 1 < b.len() && b[i + 1] == '/' {
                        while i < b.len() && b[i] != '\n' {
                            i += 1;
                        }
                        break;
                    }
                    if !in_str && ch == '/' && i + 1 < b.len() && b[i + 1] == '*' {
                        i += 2;
                        while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                            if b[i] == '\n' {
                                line += 1;
                            }
                            i += 1;
                        }
                        i += 2;
                        continue;
                    }
                    raw.push(ch);
                    i += 1;
                }
                push(&mut out, Tok::LineDirective(name, raw));
            } else {
                push(&mut out, Tok::Directive(name));
            }
            continue;
        }
        // Strings.
        if c == '"' {
            let mut s = String::new();
            i += 1;
            loop {
                if i >= b.len() {
                    return Err(err(start_line, "unterminated string literal".into()));
                }
                let ch = b[i];
                if ch == '"' {
                    i += 1;
                    break;
                }
                if ch == '\n' {
                    return Err(err(start_line, "unterminated string literal".into()));
                }
                if ch == '\\' && i + 1 < b.len() {
                    let e = b[i + 1];
                    i += 2;
                    match e {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        '\\' => s.push('\\'),
                        '"' => s.push('"'),
                        'a' => s.push('\x07'),
                        'v' => s.push('\x0b'),
                        'f' => s.push('\x0c'),
                        '\n' => line += 1,
                        'x' => {
                            let mut v = 0u32;
                            let mut k = 0;
                            while k < 2 && i < b.len() && b[i].is_ascii_hexdigit() {
                                v = v * 16 + b[i].to_digit(16).unwrap_or(0);
                                i += 1;
                                k += 1;
                            }
                            s.push(char::from_u32(v).unwrap_or('?'));
                        }
                        d if d.is_digit(8) => {
                            let mut v = d.to_digit(8).unwrap_or(0);
                            let mut k = 1;
                            while k < 3 && i < b.len() && b[i].is_digit(8) {
                                v = v * 8 + b[i].to_digit(8).unwrap_or(0);
                                i += 1;
                                k += 1;
                            }
                            s.push(char::from_u32(v).unwrap_or('?'));
                        }
                        other => s.push(other),
                    }
                    continue;
                }
                s.push(ch);
                i += 1;
            }
            push(&mut out, Tok::Str(s));
            continue;
        }
        // Identifiers / keywords / system names.
        if c.is_alphabetic() || c == '_' || (c == '$' && i + 1 < b.len() && (b[i + 1].is_alphabetic() || b[i + 1] == '_')) {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_alphanumeric() || b[j] == '_' || b[j] == '$') {
                j += 1;
            }
            let s: String = b[i..j].iter().collect();
            i = j;
            if s.starts_with('$') {
                push(&mut out, Tok::Sys(s));
            } else {
                push(&mut out, Tok::Id(s));
            }
            continue;
        }
        if c == '\\' {
            // Escaped identifier.
            let mut j = i + 1;
            while j < b.len() && !b[j].is_whitespace() {
                j += 1;
            }
            let s: String = b[i + 1..j].iter().collect();
            i = j;
            push(&mut out, Tok::Id(s));
            continue;
        }
        // Numbers.
        if c.is_ascii_digit() || (c == '\'' && i + 1 < b.len() && is_base_char(b[i + 1], b.get(i + 2).copied())) {
            let (tok, j) = lex_number(&b, i).map_err(|m| err(line, m))?;
            i = j;
            push(&mut out, tok);
            continue;
        }
        if c == '\'' && i + 1 < b.len() && matches!(b[i + 1], '0' | '1' | 'x' | 'X' | 'z' | 'Z') && !b.get(i + 2).is_some_and(|c| c.is_alphanumeric() || *c == '_') {
            let v = if b[i + 1] == '1' { 1 } else { 0 };
            i += 2;
            push(&mut out, Tok::Fill(v));
            continue;
        }
        // Operators (longest match).
        let mut matched = None;
        for op in OPS {
            let n = op.len();
            if i + n <= b.len() && b[i..i + n].iter().copied().eq(op.chars()) {
                matched = Some(*op);
                break;
            }
        }
        match matched {
            Some(op) => {
                i += op.len();
                push(&mut out, Tok::Op(op));
            }
            None => return Err(err(line, format!("unexpected character `{c}`"))),
        }
    }
    Ok(out)
}

fn is_base_char(c: char, next: Option<char>) -> bool {
    match c {
        'b' | 'B' | 'o' | 'O' | 'd' | 'D' | 'h' | 'H' => true,
        's' | 'S' => matches!(next, Some('b' | 'B' | 'o' | 'O' | 'd' | 'D' | 'h' | 'H')),
        _ => false,
    }
}

fn lex_number(b: &[char], start: usize) -> Result<(Tok, usize), String> {
    let mut i = start;
    let mut width: Option<u32> = None;
    // Leading decimal (size or plain value).
    let mut dec = String::new();
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == '_') {
        if b[i] != '_' {
            dec.push(b[i]);
        }
        i += 1;
    }
    // Skip whitespace between size and base: `8 'hFF` is legal.
    let mut k = i;
    while k < b.len() && (b[k] == ' ' || b[k] == '\t') {
        k += 1;
    }
    let based = k < b.len() && b[k] == '\'' && k + 1 < b.len() && is_base_char(b[k + 1], b.get(k + 2).copied());
    if !based {
        // Real?
        if i < b.len() && b[i] == '.' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == '_') {
                j += 1;
            }
            if j < b.len() && (b[j] == 'e' || b[j] == 'E') {
                j += 1;
                if j < b.len() && (b[j] == '+' || b[j] == '-') {
                    j += 1;
                }
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
            }
            let text: String = b[start..j].iter().filter(|c| **c != '_').collect();
            let v: f64 = text.parse().map_err(|_| format!("bad real literal `{text}`"))?;
            return time_suffix(b, j, v, text, true);
        }
        if i < b.len() && (b[i] == 'e' || b[i] == 'E') && i + 1 < b.len() && (b[i + 1].is_ascii_digit() || b[i + 1] == '-' || b[i + 1] == '+') {
            let mut j = i + 1;
            if b[j] == '+' || b[j] == '-' {
                j += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let text: String = b[start..j].iter().collect();
            let v: f64 = text.parse().map_err(|_| format!("bad real literal `{text}`"))?;
            return Ok((Tok::Real(v, text), j));
        }
        let v: u64 = dec.parse().unwrap_or(u64::MAX);
        let text: String = b[start..i].iter().collect();
        return time_suffix(b, i, v as f64, text, false).map(|(t, j)| match t {
            Tok::Real(..) => (Tok::Num { val: v, width: None, signed: true, text: b[start..i].iter().collect() }, i),
            other => (other, j),
        });
    }
    if !dec.is_empty() {
        width = Some(dec.parse().map_err(|_| "bad literal width".to_string())?);
    }
    i = k + 1; // past '
    let mut signed = false;
    if b[i] == 's' || b[i] == 'S' {
        signed = true;
        i += 1;
    }
    let base = match b[i].to_ascii_lowercase() {
        'b' => 2,
        'o' => 8,
        'd' => 10,
        _ => 16,
    };
    i += 1;
    while i < b.len() && (b[i] == ' ' || b[i] == '\t') {
        i += 1;
    }
    let mut val: u64 = 0;
    let mut any = false;
    while i < b.len() && (b[i].is_ascii_hexdigit() || matches!(b[i], '_' | 'x' | 'X' | 'z' | 'Z' | '?')) {
        let ch = b[i];
        if ch != '_' {
            let d = match ch {
                'x' | 'X' | 'z' | 'Z' | '?' => 0,
                _ => ch.to_digit(16).unwrap_or(0) as u64,
            };
            if d >= base {
                break;
            }
            val = val.wrapping_mul(base).wrapping_add(d);
            any = true;
        }
        i += 1;
    }
    if !any {
        return Err("based literal has no digits".into());
    }
    if let Some(w) = width {
        if w < 64 {
            val &= (1u64 << w) - 1;
        }
    }
    let text: String = b[start..i].iter().collect();
    Ok((Tok::Num { val, width: Some(width.unwrap_or(32)), signed, text }, i))
}

fn time_suffix(b: &[char], i: usize, v: f64, text: String, real: bool) -> Result<(Tok, usize), String> {
    let mut j = i;
    while j < b.len() && b[j].is_ascii_alphabetic() {
        j += 1;
    }
    let unit: String = b[i..j].iter().collect();
    if let Some(ps) = time_unit_ps(&unit) {
        let t = (v * ps).round() as u64;
        return Ok((Tok::Time(t, format!("{text}{unit}")), j));
    }
    if real {
        Ok((Tok::Real(v, text), i))
    } else {
        // Caller re-wraps plain integers.
        Ok((Tok::Real(v, text), i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(0, "t.sv", s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn numbers_strings_ops_and_directives() {
        let t = toks("8'hFF 'b1 12 3.5 10ns '1 \"a\\n\" a<=b `define X(a) a+1\n`X(2)");
        assert!(matches!(&t[0], Tok::Num { val: 255, width: Some(8), .. }));
        assert!(matches!(&t[1], Tok::Num { val: 1, width: Some(32), .. }));
        assert!(matches!(&t[2], Tok::Num { val: 12, width: None, .. }));
        assert!(matches!(&t[3], Tok::Real(v, _) if *v == 3.5));
        assert!(matches!(&t[4], Tok::Time(10_000, _)));
        assert_eq!(t[5], Tok::Fill(1));
        assert_eq!(t[6], Tok::Str("a\n".into()));
        assert_eq!(t[8], Tok::Op("<="));
        assert_eq!(t[10], Tok::LineDirective("define".into(), " X(a) a+1".into()));
        assert_eq!(t[11], Tok::Directive("X".into()));
    }

    #[test]
    fn lines_survive_block_comments() {
        let v = lex(0, "t", "a /* x\n y */ b\nc").unwrap();
        assert_eq!(v.iter().map(|t| t.line).collect::<Vec<_>>(), vec![1, 2, 3]);
    }
}
