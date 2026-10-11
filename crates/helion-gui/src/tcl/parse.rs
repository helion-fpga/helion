//! Tcl script parser.
//!
//! Produces [`Command`]s whose words are segment lists; substitution happens
//! at evaluation time ([`super::interp`]). Supports the real Tcl word rules:
//! `{...}` (no substitution, only special as the first character of a word),
//! `"..."` (substitution inside), `[...]` command substitution, `$var`,
//! `${var}`, `$var(index)`, backslash escapes, `#` comments, `;` and newline
//! command separators, and backslash-newline continuations.

/// One piece of a word: literal text, a `$` variable reference, or a
/// `[...]` nested command substitution.
#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    Lit(String),
    Var(VarRef),
    Cmd(Vec<Command>),
}

/// `$name`, `${name}` or `$name(index)`.
#[derive(Clone, Debug, PartialEq)]
pub struct VarRef {
    pub name: String,
    /// Array index text (substituted again at eval time, like Tcl).
    pub index: Option<String>,
}

/// One Tcl word: a segment list plus whether `{...}` grouping disabled
/// substitution for the whole word.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub segs: Vec<Seg>,
    pub no_subst: bool,
}

/// One parsed command: words plus the 1-based script line it started on.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub words: Vec<Word>,
    pub line: usize,
}

/// Parse failure with a 1-based line number.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (line {})", self.message, self.line)
    }
}

struct P<'a> {
    chars: &'a [char],
    pos: usize,
    line: usize,
}

impl<'a> P<'a> {
    fn new(src: &'a [char]) -> Self {
        Self {
            chars: src,
            pos: 0,
            line: 1,
        }
    }
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn peek2(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
        }
        c
    }
    fn err<T>(&self, message: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError {
            message: message.into(),
            line: self.line,
        })
    }
    /// Skip blanks and backslash-newline continuations (not newlines).
    fn skip_blanks(&mut self) {
        loop {
            match self.peek() {
                Some(' ') | Some('\t') | Some('\r') => {
                    self.bump();
                }
                Some('\\') if matches!(self.peek2(), Some('\n')) => {
                    self.bump();
                    self.bump();
                }
                _ => break,
            }
        }
    }
}

/// Parse a whole script into commands.
pub fn parse_script(src: &str) -> Result<Vec<Command>, ParseError> {
    let chars: Vec<char> = src.chars().collect();
    let mut p = P::new(&chars);
    parse_commands(&mut p, None)
}

/// Parse commands until EOF or `end` (for `[...]` nesting).
fn parse_commands(p: &mut P, end: Option<char>) -> Result<Vec<Command>, ParseError> {
    let mut cmds = Vec::new();
    loop {
        p.skip_blanks();
        match p.peek() {
            None => break,
            Some(c) if Some(c) == end => {
                p.bump();
                break;
            }
            Some(';') => {
                p.bump();
                continue;
            }
            Some('\n') => {
                p.bump();
                continue;
            }
            Some('#') => {
                // Comment: only where a command word could start.
                while let Some(c) = p.bump() {
                    if c == '\n' {
                        break;
                    }
                }
                continue;
            }
            _ => {}
        }
        let line = p.line;
        let mut words = Vec::new();
        loop {
            p.skip_blanks();
            match p.peek() {
                None => break,
                Some(c) if Some(c) == end => break,
                Some(';') | Some('\n') => break,
                _ => words.push(parse_word(p)?),
            }
        }
        if !words.is_empty() {
            cmds.push(Command { words, line });
        }
        // Consume the separator that ended the command (the loop head skips).
        match p.peek() {
            Some(';') | Some('\n') => {
                p.bump();
            }
            _ => {}
        }
    }
    Ok(cmds)
}

fn is_var_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Parse one word. `{`/`"` are only special as the first character.
fn parse_word(p: &mut P) -> Result<Word, ParseError> {
    let mut segs: Vec<Seg> = Vec::new();
    let mut lit = String::new();
    let first = p.peek();
    if first == Some('{') {
        p.bump();
        let mut depth = 1usize;
        let mut inner = String::new();
        loop {
            match p.bump() {
                None => return p.err("missing close-brace"),
                Some('\\') => {
                    // Inside braces only \{, \} and \\ are escapes; keep others raw.
                    match p.peek() {
                        Some('{') | Some('}') | Some('\\') => {
                            inner.push(p.bump().unwrap_or('\\'));
                        }
                        _ => inner.push('\\'),
                    }
                }
                Some('{') => {
                    depth += 1;
                    inner.push('{');
                }
                Some('}') => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    inner.push('}');
                }
                Some(c) => inner.push(c),
            }
        }
        segs.push(Seg::Lit(inner));
        return Ok(Word {
            segs,
            no_subst: true,
        });
    }
    if first == Some('"') {
        p.bump();
        loop {
            match p.peek() {
                None => return p.err("missing close-quote"),
                Some('"') => {
                    p.bump();
                    break;
                }
                _ => parse_word_char(p, &mut lit, &mut segs)?,
            }
        }
        // A quoted section may be followed by more word text: "a"b.
        loop {
            match p.peek() {
                Some('"') => {
                    p.bump();
                    loop {
                        match p.peek() {
                            None => return p.err("missing close-quote"),
                            Some('"') => {
                                p.bump();
                                break;
                            }
                            _ => parse_word_char(p, &mut lit, &mut segs)?,
                        }
                    }
                }
                Some(' ') | Some('\t') | Some('\r') | Some('\n') | Some(';') | None => break,
                _ => parse_word_char(p, &mut lit, &mut segs)?,
            }
        }
    } else {
        loop {
            match p.peek() {
                None => break,
                Some(' ') | Some('\t') | Some('\r') | Some('\n') | Some(';') => break,
                _ => parse_word_char(p, &mut lit, &mut segs)?,
            }
        }
    }
    if !lit.is_empty() {
        segs.push(Seg::Lit(lit));
    }
    Ok(Word {
        segs,
        no_subst: false,
    })
}

/// Parse one character (or substitution) of a word into `lit`/`segs`.
fn parse_word_char(p: &mut P, lit: &mut String, segs: &mut Vec<Seg>) -> Result<(), ParseError> {
    match p.peek() {
        Some('$') => {
            if !lit.is_empty() {
                segs.push(Seg::Lit(std::mem::take(lit)));
            }
            p.bump();
            segs.push(Seg::Var(parse_var(p)?));
        }
        Some('[') => {
            if !lit.is_empty() {
                segs.push(Seg::Lit(std::mem::take(lit)));
            }
            p.bump();
            let nested = parse_commands(p, Some(']'))?;
            segs.push(Seg::Cmd(nested));
        }
        Some('\\') => {
            p.bump();
            match p.bump() {
                None => lit.push('\\'),
                Some('\n') => lit.push(' '), // backslash-newline -> single space
                Some('n') => lit.push('\n'),
                Some('t') => lit.push('\t'),
                Some('r') => lit.push('\r'),
                Some(c) => lit.push(c), // \", \$, \;, \{, \}, \[, \], \ , \\
            }
        }
        Some(c) => {
            lit.push(c);
            p.bump();
        }
        None => {}
    }
    Ok(())
}

fn parse_var(p: &mut P) -> Result<VarRef, ParseError> {
    if p.peek() == Some('{') {
        p.bump();
        let mut name = String::new();
        loop {
            match p.bump() {
                None => return p.err("missing close-brace after ${"),
                Some('}') => break,
                Some(c) => name.push(c),
            }
        }
        if name.is_empty() {
            return p.err("empty ${} variable name");
        }
        return Ok(VarRef {
            name,
            index: None,
        });
    }
    let mut name = String::new();
    while let Some(c) = p.peek() {
        if is_var_char(c) {
            name.push(c);
            p.bump();
        } else {
            break;
        }
    }
    if name.is_empty() {
        // `$` not followed by a name: literal dollar (Tcl keeps it).
        return Ok(VarRef {
            name: String::new(),
            index: None,
        });
    }
    let index = if p.peek() == Some('(') {
        p.bump();
        let mut idx = String::new();
        let mut depth = 1usize;
        loop {
            match p.bump() {
                None => return p.err("missing ) in $var(index)"),
                Some('(') => {
                    depth += 1;
                    idx.push('(');
                }
                Some(')') => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    idx.push(')');
                }
                Some(c) => idx.push(c),
            }
        }
        Some(idx)
    } else {
        None
    };
    Ok(VarRef { name, index })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(cmds: &[Command]) -> Vec<Vec<String>> {
        // Render segment structure compactly for assertions.
        cmds.iter()
            .map(|c| {
                c.words
                    .iter()
                    .map(|w| {
                        let mut s = String::new();
                        if w.no_subst {
                            s.push('B');
                        }
                        for seg in &w.segs {
                            match seg {
                                Seg::Lit(t) => s.push_str(&format!("L[{t}]")),
                                Seg::Var(v) => s.push_str(&format!("V[{}]", v.name)),
                                Seg::Cmd(_) => s.push_str("C"),
                            }
                        }
                        s
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn basic_words_and_separators() {
        let c = parse_script("get_cells u_*; get_nets\nreport_timing").unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].words.len(), 2);
        assert_eq!(c[2].line, 2);
    }

    #[test]
    fn braces_quotes_brackets() {
        let c = parse_script(r#"set a {x $y [cmd]}; puts "v=$a ok""#).unwrap();
        let w = words(&c);
        assert_eq!(w[0][1], "BL[x $y [cmd]]");
        assert_eq!(w[1][1], "L[v=]V[a]L[ ok]");
    }

    #[test]
    fn brace_only_first_char() {
        let c = parse_script("puts a{$x}").unwrap();
        let w = words(&c);
        assert_eq!(w[0][1], "L[a{]V[x]L[}]");
    }

    #[test]
    fn comments_and_continuation() {
        let c = parse_script("# hi\nset a 1 \\\n 2").unwrap();
        assert_eq!(c.len(), 1);
        let w = words(&c);
        assert_eq!(w[0], vec!["L[set]", "L[a]", "L[1]L[ 2]"]);
    }

    #[test]
    fn nested_cmd_subst() {
        let c = parse_script("puts [join [list a b] ,]").unwrap();
        assert!(matches!(c[0].words[1].segs[0], Seg::Cmd(_)));
    }

    #[test]
    fn var_forms() {
        let c = parse_script("set ${a} $b $c(0)").unwrap();
        match &c[0].words[1].segs[0] {
            Seg::Var(v) => assert_eq!(v.name, "a"),
            _ => panic!(),
        }
        match &c[0].words[3].segs[0] {
            Seg::Var(v) => {
                assert_eq!(v.name, "c");
                assert_eq!(v.index.as_deref(), Some("0"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn unclosed() {
        assert!(parse_script("set a {x").is_err());
        assert!(parse_script("puts \"x").is_err());
        assert!(parse_script("puts [join a").is_err());
    }
}
