//! Token-level preprocessor: `` `define`` (with arguments and defaults,
//! `` `" `` stringification, ``` `` ``` pasting), `` `undef``, `` `include``,
//! `` `ifdef`` / `` `ifndef`` / `` `elsif`` / `` `else`` / `` `endif``,
//! `` `__FILE__`` / `` `__LINE__``. Expanded tokens keep the line of the
//! macro use so diagnostics point at the call site.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::lex::{Tok, Token, lex};
use crate::SvError;

#[derive(Clone, Debug)]
pub struct Macro {
    pub params: Option<Vec<(String, Option<String>)>>,
    pub body: String,
}

/// Source files known to the preprocessor (id = index).
#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    pub files: Vec<PathBuf>,
}

impl SourceMap {
    pub fn label(&self, id: u32) -> String {
        self.files
            .get(id as usize)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "<unknown>".into())
    }
    fn add(&mut self, p: PathBuf) -> u32 {
        if let Some(i) = self.files.iter().position(|f| *f == p) {
            return i as u32;
        }
        self.files.push(p);
        (self.files.len() - 1) as u32
    }
}

pub struct Preprocessor {
    pub defines: HashMap<String, Macro>,
    pub incdirs: Vec<PathBuf>,
    pub map: SourceMap,
    /// Built-in headers resolved before the file system (`uvm_macros.svh`).
    pub builtin: HashMap<&'static str, &'static str>,
    depth: usize,
}

struct Cond {
    active: bool,
    taken: bool,
    parent_active: bool,
}

impl Preprocessor {
    pub fn new() -> Self {
        let mut builtin = HashMap::new();
        builtin.insert("uvm_macros.svh", crate::uvm::UVM_MACROS);
        builtin.insert("uvm_pkg.sv", crate::uvm::UVM_PKG);
        builtin.insert("uvm_pkg.svh", crate::uvm::UVM_PKG);
        let mut defines = HashMap::new();
        defines.insert("HELION".into(), Macro { params: None, body: "1".into() });
        defines.insert("HELION_SVTB".into(), Macro { params: None, body: "1".into() });
        Self { defines, incdirs: Vec::new(), map: SourceMap::default(), builtin, depth: 0 }
    }

    pub fn define(&mut self, name: &str, value: &str) {
        self.defines.insert(name.into(), Macro { params: None, body: value.into() });
    }

    /// Preprocess a file from disk.
    pub fn file(&mut self, path: &Path) -> Result<Vec<Token>, SvError> {
        let label = path.to_string_lossy().into_owned();
        let src = std::fs::read_to_string(path).map_err(|e| SvError::new(&label, 0, format!("cannot read: {e}")))?;
        self.text(path.to_path_buf(), &src)
    }

    /// Preprocess source text attributed to `path`.
    pub fn text(&mut self, path: PathBuf, src: &str) -> Result<Vec<Token>, SvError> {
        let id = self.map.add(path.clone());
        let label = path.to_string_lossy().into_owned();
        let toks = lex(id, &label, src)?;
        let mut out = Vec::new();
        self.run(toks, &mut out, Some(&path))?;
        Ok(out)
    }

    fn err(&self, t: &Token, m: impl Into<String>) -> SvError {
        SvError::new(&self.map.label(t.file), t.line as usize, m)
    }

    fn run(&mut self, toks: Vec<Token>, out: &mut Vec<Token>, cur: Option<&Path>) -> Result<(), SvError> {
        self.depth += 1;
        if self.depth > 64 {
            self.depth -= 1;
            let t = toks.first().cloned().unwrap_or(Token { tok: Tok::Op(";"), file: 0, line: 0 });
            return Err(self.err(&t, "macro/include nesting deeper than 64 (recursive macro?)"));
        }
        let r = self.run_inner(toks, out, cur);
        self.depth -= 1;
        r
    }

    fn run_inner(&mut self, toks: Vec<Token>, out: &mut Vec<Token>, cur: Option<&Path>) -> Result<(), SvError> {
        let mut conds: Vec<Cond> = Vec::new();
        let mut i = 0;
        let active = |conds: &Vec<Cond>| conds.last().is_none_or(|c| c.active);
        while i < toks.len() {
            let t = &toks[i];
            match &t.tok {
                Tok::Directive(d) if matches!(d.as_str(), "ifdef" | "ifndef" | "elsif") => {
                    let name = match toks.get(i + 1).map(|x| &x.tok) {
                        Some(Tok::Id(n)) => n.clone(),
                        _ => return Err(self.err(t, format!("`{d} needs a macro name"))),
                    };
                    let def = self.defines.contains_key(&name);
                    if d == "elsif" {
                        let c = conds.last_mut().ok_or_else(|| self.err(t, "`elsif without `ifdef"))?;
                        if c.taken {
                            c.active = false;
                        } else {
                            c.active = c.parent_active && def;
                            c.taken = c.active;
                        }
                    } else {
                        let pa = active(&conds);
                        let on = pa && (def == (d == "ifdef"));
                        conds.push(Cond { active: on, taken: on, parent_active: pa });
                    }
                    i += 2;
                    continue;
                }
                Tok::Directive(d) if d == "else" => {
                    let c = conds.last_mut().ok_or_else(|| self.err(t, "`else without `ifdef"))?;
                    c.active = c.parent_active && !c.taken;
                    c.taken = true;
                    i += 1;
                    continue;
                }
                Tok::Directive(d) if d == "endif" => {
                    conds.pop().ok_or_else(|| self.err(t, "`endif without `ifdef"))?;
                    i += 1;
                    continue;
                }
                _ => {}
            }
            if !active(&conds) {
                i += 1;
                continue;
            }
            match &t.tok {
                Tok::LineDirective(d, raw) => {
                    if d == "define" {
                        self.parse_define(t, raw)?;
                    }
                    i += 1;
                }
                Tok::Directive(d) if d == "undef" => {
                    if let Some(Tok::Id(n)) = toks.get(i + 1).map(|x| &x.tok) {
                        self.defines.remove(n);
                    }
                    i += 2;
                }
                Tok::Directive(d) if d == "include" => {
                    let (name, skip) = match toks.get(i + 1).map(|x| &x.tok) {
                        Some(Tok::Str(s)) => (s.clone(), 2),
                        Some(Tok::Op("<")) => {
                            let mut j = i + 2;
                            let mut s = String::new();
                            while j < toks.len() && toks[j].tok != Tok::Op(">") {
                                s.push_str(&toks[j].src());
                                j += 1;
                            }
                            (s, j - i + 1)
                        }
                        _ => return Err(self.err(t, "`include needs a \"file\" name")),
                    };
                    i += skip;
                    self.include(t, &name, cur, out)?;
                }
                Tok::Directive(d) if d == "__FILE__" => {
                    out.push(Token { tok: Tok::Str(self.map.label(t.file)), file: t.file, line: t.line });
                    i += 1;
                }
                Tok::Directive(d) if d == "__LINE__" => {
                    out.push(Token {
                        tok: Tok::Num { val: t.line as u64, width: None, signed: true, text: t.line.to_string() },
                        file: t.file,
                        line: t.line,
                    });
                    i += 1;
                }
                Tok::Directive(d) => {
                    let m = match self.defines.get(d) {
                        Some(m) => m.clone(),
                        None => return Err(self.err(t, format!("undefined macro `{d}"))),
                    };
                    let use_tok = t.clone();
                    i += 1;
                    let mut args: Vec<String> = Vec::new();
                    if let Some(params) = &m.params {
                        if toks.get(i).map(|x| &x.tok) != Some(&Tok::Op("(")) {
                            if params.iter().all(|p| p.1.is_some()) && !params.is_empty() {
                                // all-default macro used without parens
                            } else if !params.is_empty() {
                                return Err(self.err(&use_tok, format!("macro `{d} expects arguments")));
                            }
                        } else {
                            i += 1;
                            let mut depth = 0i32;
                            let mut curarg = String::new();
                            loop {
                                let a = toks.get(i).ok_or_else(|| self.err(&use_tok, format!("unterminated arguments of `{d}")))?;
                                match &a.tok {
                                    Tok::Op("(") | Tok::Op("[") | Tok::Op("{") => depth += 1,
                                    Tok::Op(")") if depth == 0 => {
                                        args.push(curarg.trim().to_string());
                                        i += 1;
                                        break;
                                    }
                                    Tok::Op(")") | Tok::Op("]") | Tok::Op("}") => depth -= 1,
                                    Tok::Op(",") if depth == 0 => {
                                        args.push(std::mem::take(&mut curarg).trim().to_string());
                                        i += 1;
                                        continue;
                                    }
                                    _ => {}
                                }
                                if !curarg.is_empty() {
                                    curarg.push(' ');
                                }
                                curarg.push_str(&a.src());
                                i += 1;
                            }
                            if args.len() == 1 && args[0].is_empty() && params.is_empty() {
                                args.clear();
                            }
                        }
                    }
                    let text = self.expand(&use_tok, d, &m, &args)?;
                    let label = self.map.label(use_tok.file);
                    let mut etoks = lex(use_tok.file, &label, &text)?;
                    for et in &mut etoks {
                        et.file = use_tok.file;
                        et.line = use_tok.line;
                    }
                    self.run(etoks, out, cur)?;
                }
                _ => {
                    out.push(t.clone());
                    i += 1;
                }
            }
        }
        if !conds.is_empty() {
            if let Some(t) = toks.last() {
                return Err(self.err(t, "missing `endif"));
            }
        }
        Ok(())
    }

    fn include(&mut self, t: &Token, name: &str, cur: Option<&Path>, out: &mut Vec<Token>) -> Result<(), SvError> {
        let base = Path::new(name).file_name().and_then(|s| s.to_str()).unwrap_or(name);
        if let Some(src) = self.builtin.get(base).copied() {
            if base.starts_with("uvm_pkg") && self.defines.contains_key("__HELION_UVM_PKG_INCLUDED") {
                return Ok(());
            }
            let p = PathBuf::from(format!("<helion>/{base}"));
            let toks = self.text(p, src)?;
            out.extend(toks);
            return Ok(());
        }
        let mut cands: Vec<PathBuf> = Vec::new();
        let p = Path::new(name);
        if p.is_absolute() {
            cands.push(p.to_path_buf());
        } else {
            if let Some(dir) = cur.and_then(|c| c.parent()) {
                cands.push(dir.join(name));
            }
            for d in &self.incdirs {
                cands.push(d.join(name));
            }
        }
        let found = cands.into_iter().find(|c| c.is_file()).ok_or_else(|| self.err(t, format!("`include file \"{name}\" not found")))?;
        let label = found.to_string_lossy().into_owned();
        let src = std::fs::read_to_string(&found).map_err(|e| SvError::new(&label, 0, format!("cannot read: {e}")))?;
        let id = self.map.add(found.clone());
        let toks = lex(id, &label, &src)?;
        self.run(toks, out, Some(&found))
    }

    fn parse_define(&mut self, t: &Token, raw: &str) -> Result<(), SvError> {
        let s = raw.trim_start();
        let name: String = s.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if name.is_empty() {
            return Err(self.err(t, "`define needs a name"));
        }
        let rest = &s[name.len()..];
        let (params, body) = if let Some(r) = rest.strip_prefix('(') {
            // Formal arguments up to the matching ')'.
            let mut depth = 0;
            let mut end = None;
            for (k, c) in r.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' if depth == 0 => {
                        end = Some(k);
                        break;
                    }
                    ')' => depth -= 1,
                    _ => {}
                }
            }
            let end = end.ok_or_else(|| self.err(t, format!("`define {name}: unterminated argument list")))?;
            let mut ps = Vec::new();
            for a in split_top(&r[..end]) {
                let a = a.trim();
                if a.is_empty() {
                    continue;
                }
                match a.split_once('=') {
                    Some((n, d)) => ps.push((n.trim().to_string(), Some(d.trim().to_string()))),
                    None => ps.push((a.to_string(), None)),
                }
            }
            (Some(ps), r[end + 1..].to_string())
        } else {
            (None, rest.to_string())
        };
        self.defines.insert(name, Macro { params, body: body.trim().to_string() });
        Ok(())
    }

    fn expand(&self, t: &Token, name: &str, m: &Macro, args: &[String]) -> Result<String, SvError> {
        let mut map: HashMap<&str, String> = HashMap::new();
        if let Some(ps) = &m.params {
            if args.len() > ps.len() {
                return Err(self.err(t, format!("macro `{name} takes {} argument(s), got {}", ps.len(), args.len())));
            }
            for (k, (p, def)) in ps.iter().enumerate() {
                let v = match args.get(k) {
                    Some(a) if !a.is_empty() => a.clone(),
                    _ => match def {
                        Some(d) => d.clone(),
                        None if args.get(k).is_some() => String::new(),
                        None => return Err(self.err(t, format!("macro `{name}: missing argument `{p}`"))),
                    },
                };
                map.insert(p.as_str(), v);
            }
        }
        let b: Vec<char> = m.body.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        let mut in_mstr = false;
        while i < b.len() {
            let c = b[i];
            if c == '`' && i + 1 < b.len() && b[i + 1] == '"' {
                out.push('"');
                in_mstr = !in_mstr;
                i += 2;
                continue;
            }
            if c == '`' && i + 3 < b.len() && b[i + 1] == '\\' && b[i + 2] == '`' && b[i + 3] == '"' {
                out.push_str("\\\"");
                i += 4;
                continue;
            }
            if c == '`' && i + 1 < b.len() && b[i + 1] == '`' {
                i += 2;
                continue;
            }
            if c == '"' && !in_mstr {
                out.push('"');
                i += 1;
                while i < b.len() && b[i] != '"' {
                    if b[i] == '\\' && i + 1 < b.len() {
                        out.push(b[i]);
                        i += 1;
                    }
                    out.push(b[i]);
                    i += 1;
                }
                if i < b.len() {
                    out.push('"');
                    i += 1;
                }
                continue;
            }
            if c.is_alphabetic() || c == '_' {
                let mut j = i;
                while j < b.len() && (b[j].is_alphanumeric() || b[j] == '_' || b[j] == '$') {
                    j += 1;
                }
                let id: String = b[i..j].iter().collect();
                let prev_tick = i > 0 && b[i - 1] == '`' && !(i > 1 && b[i - 2] == '`');
                match map.get(id.as_str()) {
                    Some(v) if !prev_tick => {
                        if in_mstr {
                            out.push_str(&v.replace('"', "\\\""));
                        } else {
                            out.push_str(v);
                        }
                    }
                    _ => out.push_str(&id),
                }
                i = j;
                continue;
            }
            if c == '\n' {
                out.push(' ');
                i += 1;
                continue;
            }
            out.push(c);
            i += 1;
        }
        Ok(out)
    }
}

impl Default for Preprocessor {
    fn default() -> Self {
        Self::new()
    }
}

fn split_top(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    let mut in_str = false;
    for c in s.chars() {
        match c {
            '"' => in_str = !in_str,
            '(' | '[' | '{' if !in_str => depth += 1,
            ')' | ']' | '}' if !in_str => depth -= 1,
            ',' if depth == 0 && !in_str => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pp(src: &str) -> String {
        let mut p = Preprocessor::new();
        p.text("t.sv".into(), src).unwrap().iter().map(|t| t.src()).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn define_args_defaults_stringify_and_paste() {
        assert_eq!(pp("`define ADD(a,b=1) (a+b)\nx = `ADD(2) + `ADD(3,4);"), "x = ( 2 + 1 ) + ( 3 + 4 ) ;");
        assert_eq!(pp("`define S(t) `\"t`\"\ns = `S(foo);"), "s = \"foo\" ;");
        assert_eq!(pp("`define P(s) imp``s\nclass `P(_a);"), "class imp_a ;");
    }

    #[test]
    fn ifdef_else_and_nesting() {
        let s = "`define A\n`ifdef A a `ifdef B b `else nb `endif `else na `endif `ifndef C c `endif";
        assert_eq!(pp(s), "a nb c");
    }

    #[test]
    fn expansion_keeps_use_line_and_file_line_macros() {
        let mut p = Preprocessor::new();
        let t = p.text("t.sv".into(), "`define M(x) foo(x, `__LINE__)\n\n`M(1)").unwrap();
        assert!(t.iter().all(|t| t.line == 3));
        assert_eq!(t.iter().map(|t| t.src()).collect::<Vec<_>>().join(""), "foo(1,3)");
    }

    #[test]
    fn undefined_macro_is_an_error_with_line() {
        let mut p = Preprocessor::new();
        let e = p.text("t.sv".into(), "\n`NOPE").unwrap_err();
        assert_eq!(e.line, 2);
    }
}
