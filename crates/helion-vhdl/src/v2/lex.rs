//! VHDL lexer (IEEE 1076-2008 lexical elements, synthesis subset).

#[derive(Clone, Debug, PartialEq)]
pub enum T {
    /// Identifier or reserved word, lowercased. `Tok::orig` keeps the spelling.
    Id(String),
    Int(i128),
    Real(f64),
    Char(char),
    Str(String),
    /// Bit-string literal expanded to one char per element, leftmost first.
    BitStr(String),
    Sym(&'static str),
}

#[derive(Clone, Debug)]
pub struct Tok {
    pub t: T,
    pub line: u32,
    pub orig: String,
}

const RESERVED: &[&str] = &[
    "abs",
    "access",
    "after",
    "alias",
    "all",
    "and",
    "architecture",
    "array",
    "assert",
    "assume",
    "attribute",
    "begin",
    "block",
    "body",
    "buffer",
    "bus",
    "case",
    "component",
    "configuration",
    "constant",
    "context",
    "cover",
    "default",
    "disconnect",
    "downto",
    "else",
    "elsif",
    "end",
    "entity",
    "exit",
    "fairness",
    "file",
    "for",
    "force",
    "function",
    "generate",
    "generic",
    "group",
    "guarded",
    "if",
    "impure",
    "in",
    "inertial",
    "inout",
    "is",
    "label",
    "library",
    "linkage",
    "literal",
    "loop",
    "map",
    "mod",
    "nand",
    "new",
    "next",
    "nor",
    "not",
    "null",
    "of",
    "on",
    "open",
    "or",
    "others",
    "out",
    "package",
    "parameter",
    "port",
    "postponed",
    "procedure",
    "process",
    "property",
    "protected",
    "pure",
    "range",
    "record",
    "register",
    "reject",
    "release",
    "rem",
    "report",
    "restrict",
    "return",
    "rol",
    "ror",
    "select",
    "sequence",
    "severity",
    "shared",
    "signal",
    "sla",
    "sll",
    "sra",
    "srl",
    "strong",
    "subtype",
    "then",
    "to",
    "transport",
    "type",
    "unaffected",
    "units",
    "until",
    "use",
    "variable",
    "vmode",
    "vprop",
    "vunit",
    "wait",
    "when",
    "while",
    "with",
    "xnor",
    "xor",
];

pub fn is_reserved(s: &str) -> bool {
    RESERVED.binary_search(&s).is_ok()
}

const SYMS: &[&str] = &[
    "?/=", "?<=", "?>=", "=>", "**", ":=", "/=", ">=", "<=", "<>", "??", "?=", "?<", "?>", "<<",
    ">>", "&", "'", "(", ")", "*", "+", ",", "-", ".", "/", ":", ";", "<", "=", ">", "|", "[", "]",
    "?", "@",
];

fn digit_val(c: char) -> Option<u32> {
    c.to_digit(16)
}

/// Expand a bit-string literal body for base `b` (2, 8, 16) or decimal.
fn expand_bitstr(base: char, body: &str, line: u32) -> Result<String, String> {
    let body: String = body.chars().filter(|&c| c != '_').collect();
    let per = match base {
        'b' => 1,
        'o' => 3,
        'x' => 4,
        'd' => {
            let v: u128 = body
                .parse()
                .map_err(|_| format!("line {line}: bad decimal bit string \"{body}\""))?;
            if v == 0 {
                return Ok("0".into());
            }
            return Ok(format!("{v:b}"));
        }
        _ => return Err(format!("line {line}: unknown bit string base '{base}'")),
    };
    let mut out = String::new();
    for c in body.chars() {
        match digit_val(c) {
            Some(d) if d < (1 << per) => {
                for k in (0..per).rev() {
                    out.push(if (d >> k) & 1 == 1 { '1' } else { '0' });
                }
            }
            _ => {
                let lc = c.to_ascii_lowercase();
                if matches!(lc, 'x' | 'z' | '-' | 'u' | 'w' | 'l' | 'h') {
                    for _ in 0..per {
                        out.push(lc);
                    }
                } else {
                    return Err(format!("line {line}: bad digit '{c}' in bit string"));
                }
            }
        }
    }
    Ok(out)
}

pub fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let cs: Vec<char> = src.chars().collect();
    let mut toks: Vec<Tok> = Vec::new();
    let mut i = 0usize;
    let mut line = 1u32;
    let n = cs.len();
    while i < n {
        let c = cs[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '-' && i + 1 < n && cs[i + 1] == '-' {
            while i < n && cs[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && cs[i + 1] == '*' {
            i += 2;
            while i + 1 < n && !(cs[i] == '*' && cs[i + 1] == '/') {
                if cs[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
            continue;
        }
        let start = i;
        // Bit string literal: [len] base " ... "  (base: b o x d, optionally u/s prefixed)
        if c.is_ascii_alphanumeric() {
            let mut j = i;
            let mut len: Option<usize> = None;
            while j < n && cs[j].is_ascii_digit() {
                j += 1;
            }
            if j > i {
                len = cs[i..j].iter().collect::<String>().parse().ok();
            }
            let mut k = j;
            let mut sign = None;
            if k < n
                && matches!(cs[k].to_ascii_lowercase(), 'u' | 's')
                && k + 1 < n
                && matches!(cs[k + 1].to_ascii_lowercase(), 'b' | 'o' | 'x')
            {
                sign = Some(cs[k].to_ascii_lowercase());
                k += 1;
            }
            if k < n
                && matches!(cs[k].to_ascii_lowercase(), 'b' | 'o' | 'x' | 'd')
                && k + 1 < n
                && cs[k + 1] == '"'
                && (j == i || len.is_some())
            {
                let base = cs[k].to_ascii_lowercase();
                let mut m = k + 2;
                while m < n && cs[m] != '"' {
                    m += 1;
                }
                let body: String = cs[k + 2..m].iter().collect();
                let mut bits = expand_bitstr(base, &body, line)?;
                if let Some(l) = len {
                    if bits.len() < l {
                        let pad = if sign == Some('s') {
                            bits.chars().next().unwrap_or('0')
                        } else {
                            '0'
                        };
                        bits = std::iter::repeat(pad)
                            .take(l - bits.len())
                            .chain(bits.chars())
                            .collect();
                    } else if bits.len() > l {
                        bits = bits[bits.len() - l..].to_string();
                    }
                }
                i = m + 1;
                toks.push(Tok {
                    t: T::BitStr(bits),
                    line,
                    orig: cs[start..i].iter().collect(),
                });
                continue;
            }
        }
        if c.is_ascii_alphabetic() {
            let mut j = i;
            while j < n && (cs[j].is_ascii_alphanumeric() || cs[j] == '_') {
                j += 1;
            }
            let orig: String = cs[i..j].iter().collect();
            i = j;
            toks.push(Tok {
                t: T::Id(orig.to_ascii_lowercase()),
                line,
                orig,
            });
            continue;
        }
        if c == '\\' {
            let mut j = i + 1;
            while j < n && cs[j] != '\\' {
                j += 1;
            }
            let orig: String = cs[i + 1..j.min(n)].iter().collect();
            i = j + 1;
            toks.push(Tok {
                t: T::Id(orig.to_ascii_lowercase()),
                line,
                orig,
            });
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = i;
            while j < n && (cs[j].is_ascii_digit() || cs[j] == '_') {
                j += 1;
            }
            let int_part: String = cs[i..j].iter().filter(|&&c| c != '_').collect();
            if j < n && (cs[j] == '#' || cs[j] == ':') {
                let delim = cs[j];
                let base: u32 = int_part
                    .parse()
                    .map_err(|_| format!("line {line}: bad base"))?;
                let mut k = j + 1;
                let mut v: i128 = 0;
                while k < n && cs[k] != delim {
                    if cs[k] != '_' {
                        let d = cs[k]
                            .to_digit(base)
                            .ok_or_else(|| format!("line {line}: bad based literal"))?;
                        v = v * base as i128 + d as i128;
                    }
                    k += 1;
                }
                k += 1;
                let mut exp = 0u32;
                if k < n && (cs[k] == 'e' || cs[k] == 'E') {
                    let mut m = k + 1;
                    if m < n && cs[m] == '+' {
                        m += 1;
                    }
                    let s = m;
                    while m < n && cs[m].is_ascii_digit() {
                        m += 1;
                    }
                    exp = cs[s..m].iter().collect::<String>().parse().unwrap_or(0);
                    k = m;
                }
                i = k;
                toks.push(Tok {
                    t: T::Int(v * (base as i128).pow(exp)),
                    line,
                    orig: cs[start..i].iter().collect(),
                });
                continue;
            }
            let mut is_real = false;
            let mut text = int_part.clone();
            if j + 1 < n && cs[j] == '.' && cs[j + 1].is_ascii_digit() {
                is_real = true;
                let mut k = j + 1;
                text.push('.');
                while k < n && (cs[k].is_ascii_digit() || cs[k] == '_') {
                    if cs[k] != '_' {
                        text.push(cs[k]);
                    }
                    k += 1;
                }
                j = k;
            }
            if j < n
                && (cs[j] == 'e' || cs[j] == 'E')
                && j + 1 < n
                && (cs[j + 1].is_ascii_digit()
                    || ((cs[j + 1] == '+' || cs[j + 1] == '-')
                        && j + 2 < n
                        && cs[j + 2].is_ascii_digit()))
            {
                let mut k = j + 1;
                let mut e = String::from("e");
                if cs[k] == '+' || cs[k] == '-' {
                    e.push(cs[k]);
                    k += 1;
                }
                while k < n && cs[k].is_ascii_digit() {
                    e.push(cs[k]);
                    k += 1;
                }
                text.push_str(&e);
                j = k;
                if !is_real {
                    let ev: u32 = e[1..].trim_start_matches('+').parse().unwrap_or(0);
                    let base: i128 = int_part.parse().unwrap_or(0);
                    i = j;
                    toks.push(Tok {
                        t: T::Int(base * 10i128.pow(ev)),
                        line,
                        orig: cs[start..i].iter().collect(),
                    });
                    continue;
                }
            }
            i = j;
            if is_real {
                let v: f64 = text
                    .parse()
                    .map_err(|_| format!("line {line}: bad real literal {text}"))?;
                toks.push(Tok {
                    t: T::Real(v),
                    line,
                    orig: text,
                });
            } else {
                let v: i128 = int_part
                    .parse()
                    .map_err(|_| format!("line {line}: integer literal too large"))?;
                toks.push(Tok {
                    t: T::Int(v),
                    line,
                    orig: int_part,
                });
            }
            continue;
        }
        if c == '"' {
            let mut j = i + 1;
            let mut s = String::new();
            while j < n {
                if cs[j] == '"' {
                    if j + 1 < n && cs[j + 1] == '"' {
                        s.push('"');
                        j += 2;
                        continue;
                    }
                    break;
                }
                s.push(cs[j]);
                j += 1;
            }
            i = j + 1;
            toks.push(Tok {
                t: T::Str(s),
                line,
                orig: cs[start..i.min(n)].iter().collect(),
            });
            continue;
        }
        if c == '\'' {
            let prev_is_name = match toks.last() {
                Some(Tok { t: T::Id(s), .. }) => !is_reserved(s),
                Some(Tok { t: T::Sym(")"), .. }) | Some(Tok { t: T::Sym("]"), .. }) => true,
                _ => false,
            };
            if !prev_is_name && i + 2 < n && cs[i + 2] == '\'' {
                toks.push(Tok {
                    t: T::Char(cs[i + 1]),
                    line,
                    orig: cs[i..i + 3].iter().collect(),
                });
                i += 3;
                continue;
            }
            toks.push(Tok {
                t: T::Sym("'"),
                line,
                orig: "'".into(),
            });
            i += 1;
            continue;
        }
        let mut matched = false;
        for s in SYMS {
            let sc: Vec<char> = s.chars().collect();
            if i + sc.len() <= n && cs[i..i + sc.len()] == sc[..] {
                toks.push(Tok {
                    t: T::Sym(s),
                    line,
                    orig: s.to_string(),
                });
                i += sc.len();
                matched = true;
                break;
            }
        }
        if !matched {
            return Err(format!("line {line}: unexpected character '{c}'"));
        }
    }
    Ok(toks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_sorted() {
        let mut v = RESERVED.to_vec();
        v.sort();
        assert_eq!(v, RESERVED.to_vec());
    }

    #[test]
    fn lex_basics() {
        let t = lex("a <= x\"0F\" when b='1' else 8ux\"F\"; c'high 16#ff# 1e3 2.5 \"ab\"").unwrap();
        let ts: Vec<T> = t.into_iter().map(|t| t.t).collect();
        assert_eq!(ts[2], T::BitStr("00001111".into()));
        assert_eq!(ts[6], T::Char('1'));
        assert_eq!(ts[8], T::BitStr("00001111".into()));
        assert_eq!(ts[11], T::Sym("'"));
        assert_eq!(ts[13], T::Int(255));
        assert_eq!(ts[14], T::Int(1000));
        assert_eq!(ts[15], T::Real(2.5));
        assert_eq!(ts[16], T::Str("ab".into()));
    }

    #[test]
    fn lex_when_char() {
        let t = lex("when '1' => q'(x)").unwrap();
        assert_eq!(t[1].t, T::Char('1'));
        assert_eq!(t[4].t, T::Sym("'"));
    }
}
