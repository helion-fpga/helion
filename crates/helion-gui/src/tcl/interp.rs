//! Tcl-subset interpreter core: scopes, procedures, evaluation, and the
//! built-in (non-design) command set.
//!
//! Everything is a string, like Tcl. See `docs/tcl-subset.md` for the
//! honestly-supported subset.

use super::design::SelObj;
use super::parse::{parse_script, Command, Seg, Word};
use crate::GpuiShell;
use std::collections::HashMap;

/// Interpreter-raised error with the 1-based script line.
#[derive(Clone, Debug, PartialEq)]
pub struct TclError {
    pub msg: String,
    pub line: usize,
}

/// Internal control flow: values plus proc/loop control and errors.
#[derive(Clone, Debug)]
pub enum Ctl {
    Value(String),
    Return(String),
    Break,
    Continue,
    Error(TclError),
}

impl Ctl {
    fn err(line: usize, msg: impl Into<String>) -> Self {
        Ctl::Error(TclError {
            msg: msg.into(),
            line,
        })
    }
}

#[derive(Clone, Debug)]
struct Param {
    name: String,
    /// Raw default text, substituted in the *caller's* scope at call time.
    default: Option<String>,
}

#[derive(Clone, Debug)]
struct ProcDef {
    params: Vec<Param>,
    body: Vec<Command>,
}

/// Persistent Tcl session: variables, procedures, `create_clock` state and
/// the `select_objects` selection. Lives on [`GpuiShell`] so it survives
/// across console commands.
#[derive(Clone, Debug)]
pub struct TclInterp {
    scopes: Vec<HashMap<String, String>>,
    procs: HashMap<String, ProcDef>,
    depth: u32,
    /// `puts` output buffer, drained by [`crate::tcl_eval`] after each eval.
    pub output: String,
    /// Clocks from `create_clock` (empty = STA falls back to the 10 ns `clk`).
    pub clocks: helion_sta::Constraints,
    /// Objects named by `select_objects` / `highlight_objects`.
    pub selected: Vec<SelObj>,
}

impl TclInterp {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            procs: HashMap::new(),
            depth: 0,
            output: String::new(),
            clocks: helion_sta::Constraints::default(),
            selected: Vec::new(),
        }
    }

    /// Evaluate a script against the loaded design. Returns the last
    /// command's value; errors are `ERROR: <msg> (line <n>)`.
    pub fn eval(&mut self, script: &str, shell: &mut GpuiShell) -> Result<String, String> {
        let cmds = parse_script(script).map_err(|e| format!("ERROR: {e}"))?;
        match self.eval_cmds(&cmds, shell) {
            Ctl::Value(s) | Ctl::Return(s) => Ok(s),
            Ctl::Break => Err("ERROR: invoked \"break\" outside of a loop".into()),
            Ctl::Continue => Err("ERROR: invoked \"continue\" outside of a loop".into()),
            Ctl::Error(e) => Err(format!("ERROR: {} (line {})", e.msg, e.line)),
        }
    }

    fn eval_cmds(&mut self, cmds: &[Command], shell: &mut GpuiShell) -> Ctl {
        if self.depth >= 500 {
            return Ctl::err(
                cmds.first().map(|c| c.line).unwrap_or(1),
                "max nesting depth exceeded (recursive eval?)",
            );
        }
        self.depth += 1;
        let mut last = String::new();
        for cmd in cmds {
            match self.eval_cmd(cmd, shell) {
                Ctl::Value(s) => last = s,
                other => {
                    self.depth -= 1;
                    return other;
                }
            }
        }
        self.depth -= 1;
        Ctl::Value(last)
    }

    /// Evaluate a command body given as source text (if/foreach/proc bodies).
    pub fn eval_body(&mut self, body: &str, line: usize, shell: &mut GpuiShell) -> Ctl {
        match parse_script(body) {
            Ok(cmds) => self.eval_cmds(&cmds, shell),
            Err(e) => Ctl::err(line, format!("body: {e}")),
        }
    }

    fn eval_cmd(&mut self, cmd: &Command, shell: &mut GpuiShell) -> Ctl {
        let mut argv: Vec<String> = Vec::with_capacity(cmd.words.len());
        for w in &cmd.words {
            match self.subst_word(w, cmd.line, shell) {
                Ok(s) => argv.push(s),
                Err(e) => return Ctl::Error(e),
            }
        }
        if argv.is_empty() {
            return Ctl::Value(String::new());
        }
        self.call(&argv, cmd.line, shell)
    }

    fn subst_word(
        &mut self,
        w: &Word,
        line: usize,
        shell: &mut GpuiShell,
    ) -> Result<String, TclError> {
        if w.no_subst {
            let mut out = String::new();
            for seg in &w.segs {
                if let Seg::Lit(t) = seg {
                    out.push_str(t);
                }
            }
            return Ok(out);
        }
        let mut out = String::new();
        for seg in &w.segs {
            match seg {
                Seg::Lit(t) => out.push_str(t),
                Seg::Var(v) => out.push_str(&self.var_subst(v, line, shell)?),
                Seg::Cmd(cmds) => match self.eval_cmds(cmds, shell) {
                    Ctl::Value(s) | Ctl::Return(s) => out.push_str(&s),
                    Ctl::Error(e) => return Err(e),
                    Ctl::Break => {
                        return Err(TclError {
                            msg: "invoked \"break\" outside of a loop".into(),
                            line,
                        })
                    }
                    Ctl::Continue => {
                        return Err(TclError {
                            msg: "invoked \"continue\" outside of a loop".into(),
                            line,
                        })
                    }
                },
            }
        }
        Ok(out)
    }

    fn var_subst(
        &mut self,
        v: &super::parse::VarRef,
        line: usize,
        shell: &mut GpuiShell,
    ) -> Result<String, TclError> {
        if v.name.is_empty() {
            return Ok("$".to_string()); // lone `$` stays literal
        }
        let key = match &v.index {
            Some(idx_src) => {
                let idx = self.subst_str(idx_src, line, shell)?;
                format!("{}({idx})", v.name)
            }
            None => v.name.clone(),
        };
        self.lookup(&key).cloned().ok_or_else(|| TclError {
            msg: format!("can't read \"{}\": no such variable", v.name),
            line,
        })
    }

    /// Substitute a raw string fragment (array indices, proc defaults):
    /// parse it as words, substitute each, rejoin with spaces.
    fn subst_str(
        &mut self,
        src: &str,
        line: usize,
        shell: &mut GpuiShell,
    ) -> Result<String, TclError> {
        if src.trim().is_empty() {
            return Ok(String::new());
        }
        let cmds = parse_script(src).map_err(|e| TclError {
            msg: format!("subst: {e}"),
            line,
        })?;
        let mut parts = Vec::new();
        for cmd in &cmds {
            for w in &cmd.words {
                parts.push(self.subst_word(w, line, shell)?);
            }
        }
        Ok(parts.join(" "))
    }

    pub fn lookup(&self, key: &str) -> Option<&String> {
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.get(key) {
                return Some(v);
            }
        }
        None
    }

    /// Tcl `set` semantics: update the innermost scope that already has the
    /// variable, otherwise create it in the current (innermost) scope.
    pub fn assign(&mut self, key: String, val: String) {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(&key) {
                scope.insert(key, val);
                return;
            }
        }
        if let Some(top) = self.scopes.last_mut() {
            top.insert(key, val);
        }
    }

    fn call(&mut self, argv: &[String], line: usize, shell: &mut GpuiShell) -> Ctl {
        let name = argv[0].as_str();
        match name {
            "set" => self.c_set(argv, line),
            "unset" => self.c_unset(argv, line),
            "puts" => self.c_puts(argv, line),
            "expr" => self.c_expr(argv, line),
            "if" => self.c_if(argv, line, shell),
            "foreach" => self.c_foreach(argv, line, shell),
            "for" => self.c_for(argv, line, shell),
            "while" => self.c_while(argv, line, shell),
            "proc" => self.c_proc(argv, line),
            "return" => self.c_return(argv, line),
            "break" => {
                if argv.len() != 1 {
                    Ctl::err(line, "wrong # args: should be \"break\"")
                } else {
                    Ctl::Break
                }
            }
            "continue" => {
                if argv.len() != 1 {
                    Ctl::err(line, "wrong # args: should be \"continue\"")
                } else {
                    Ctl::Continue
                }
            }
            "list" => Ctl::Value(join_list(&argv[1..])),
            "llength" => self.c_llength(argv, line),
            "lindex" => self.c_lindex(argv, line),
            "lrange" => self.c_lrange(argv, line),
            "lappend" => self.c_lappend(argv, line),
            "concat" => self.c_concat(argv, line),
            "join" => self.c_join(argv, line),
            "split" => self.c_split(argv, line),
            "string" => self.c_string(argv, line),
            "info" => self.c_info(argv, line),
            "catch" => self.c_catch(argv, line, shell),
            "error" => self.c_error(argv, line),
            "eval" => self.c_eval(argv, line, shell),
            "subst" => self.c_subst(argv, line, shell),
            "source" => self.c_source(argv, line, shell),
            "format" => self.c_format(argv, line),
            "incr" => self.c_incr(argv, line),
            "append" => self.c_append(argv, line),
            _ => {
                if let Some(def) = self.procs.get(name).cloned() {
                    self.call_proc(name, def, argv, line, shell)
                } else {
                    super::design::eval_host(self, name, argv, line, shell)
                }
            }
        }
    }

    // ---- core commands ----

    fn c_set(&mut self, argv: &[String], line: usize) -> Ctl {
        match argv.len() {
            2 => match self.lookup(&argv[1]).cloned() {
                Some(v) => Ctl::Value(v),
                None => Ctl::err(line, format!("can't read \"{}\": no such variable", argv[1])),
            },
            3 => {
                self.assign(argv[1].clone(), argv[2].clone());
                Ctl::Value(argv[2].clone())
            }
            _ => Ctl::err(line, "wrong # args: should be \"set varName ?newValue?\""),
        }
    }

    fn c_unset(&mut self, argv: &[String], line: usize) -> Ctl {
        let mut nocomplain = false;
        let mut vars: Vec<&str> = Vec::new();
        for a in &argv[1..] {
            if a == "-nocomplain" {
                nocomplain = true;
            } else if a.starts_with('-') {
                return Ctl::err(line, format!("bad option \"{a}\": must be -nocomplain"));
            } else {
                vars.push(a);
            }
        }
        if vars.is_empty() {
            return Ctl::err(line, "wrong # args: should be \"unset ?-nocomplain? varName ...\"");
        }
        for v in vars {
            let mut found = false;
            for scope in self.scopes.iter_mut().rev() {
                if scope.remove(v).is_some() {
                    found = true;
                    break;
                }
            }
            if !found && !nocomplain {
                return Ctl::err(line, format!("can't unset \"{v}\": no such variable"));
            }
        }
        Ctl::Value(String::new())
    }

    fn c_puts(&mut self, argv: &[String], line: usize) -> Ctl {
        let mut i = 1;
        let mut nonewline = false;
        while i < argv.len() && argv[i].starts_with('-') {
            match argv[i].as_str() {
                "-nonewline" => nonewline = true,
                other => return Ctl::err(line, format!("bad option \"{other}\": must be -nonewline")),
            }
            i += 1;
        }
        let args = &argv[i..];
        let text = match args.len() {
            1 => args[0].clone(),
            2 => {
                if args[0] != "stdout" {
                    return Ctl::err(line, format!("can only puts to stdout, not \"{}\"", args[0]));
                }
                args[1].clone()
            }
            _ => {
                return Ctl::err(
                    line,
                    "wrong # args: should be \"puts ?-nonewline? ?channelId? string\"",
                )
            }
        };
        self.output.push_str(&text);
        if !nonewline {
            self.output.push('\n');
        }
        Ctl::Value(String::new())
    }

    fn c_expr(&mut self, argv: &[String], line: usize) -> Ctl {
        if argv.len() < 2 {
            return Ctl::err(line, "wrong # args: should be \"expr arg ...\"");
        }
        let src = argv[1..].join(" ");
        match self.eval_expr(&src, line) {
            Ok(v) => Ctl::Value(v.to_string()),
            Err(m) => Ctl::err(line, m),
        }
    }

    fn c_if(&mut self, argv: &[String], line: usize, shell: &mut GpuiShell) -> Ctl {
        let mut i = 1;
        loop {
            if i >= argv.len() {
                return Ctl::err(
                    line,
                    "wrong # args: should be \"if expr body ?elseif expr body? ?else body?\"",
                );
            }
            if argv[i] == "else" {
                if i + 1 != argv.len() - 1 {
                    return Ctl::err(line, "wrong # args: \"else\" must take exactly one body");
                }
                return self.eval_body(&argv[i + 1].clone(), line, shell);
            }
            if i + 1 >= argv.len() {
                return Ctl::err(
                    line,
                    "wrong # args: should be \"if expr body ?elseif expr body? ?else body?\"",
                );
            }
            let cond_src = argv[i].clone();
            let body = argv[i + 1].clone();
            let cond = match self.eval_expr(&cond_src, line) {
                Ok(v) => v,
                Err(m) => return Ctl::err(line, m),
            };
            let take = match truth(&cond) {
                Ok(b) => b,
                Err(m) => return Ctl::err(line, m),
            };
            if take {
                return self.eval_body(&body, line, shell);
            }
            i += 2;
            if i < argv.len() && argv[i] == "elseif" {
                i += 1;
            } else if i < argv.len() {
                return Ctl::err(line, format!("unexpected \"{}\" after if body", argv[i]));
            }
        }
    }

    fn c_foreach(&mut self, argv: &[String], line: usize, shell: &mut GpuiShell) -> Ctl {
        if argv.len() < 4 || argv.len() % 2 != 0 {
            return Ctl::err(
                line,
                "wrong # args: should be \"foreach varList list ?varList list ...? body\"",
            );
        }
        let body = argv[argv.len() - 1].clone();
        let mut pairs: Vec<(Vec<String>, Vec<String>)> = Vec::new();
        for chunk in argv[1..argv.len() - 1].chunks(2) {
            let vars = match split_list(&chunk[0]) {
                Ok(v) => v,
                Err(m) => return Ctl::err(line, format!("foreach varList: {m}")),
            };
            let list = match split_list(&chunk[1]) {
                Ok(v) => v,
                Err(m) => return Ctl::err(line, format!("foreach list: {m}")),
            };
            if vars.is_empty() {
                return Ctl::err(line, "foreach: empty varList");
            }
            pairs.push((vars, list));
        }
        let mut idx = 0usize;
        loop {
            let mut any = false;
            for (vars, list) in &pairs {
                for (vi, var) in vars.iter().enumerate() {
                    let pos = idx * vars.len() + vi;
                    let val = list.get(pos).cloned().unwrap_or_default();
                    if pos < list.len() {
                        any = true;
                    }
                    self.assign(var.clone(), val);
                }
            }
            if !any {
                break;
            }
            match self.eval_body(&body.clone(), line, shell) {
                Ctl::Value(_) => {}
                Ctl::Break => break,
                Ctl::Continue => {}
                other => return other,
            }
            idx += 1;
        }
        Ctl::Value(String::new())
    }

    fn c_for(&mut self, argv: &[String], line: usize, shell: &mut GpuiShell) -> Ctl {
        if argv.len() != 5 {
            return Ctl::err(line, "wrong # args: should be \"for start test next body\"");
        }
        let (start, test, next, body) = (
            argv[1].clone(),
            argv[2].clone(),
            argv[3].clone(),
            argv[4].clone(),
        );
        if let Ctl::Error(e) = self.eval_body(&start, line, shell) {
            return Ctl::Error(e);
        }
        loop {
            let cond = match self.eval_expr(&test, line) {
                Ok(v) => v,
                Err(m) => return Ctl::err(line, m),
            };
            match truth(&cond) {
                Ok(true) => {}
                Ok(false) => break,
                Err(m) => return Ctl::err(line, m),
            }
            match self.eval_body(&body.clone(), line, shell) {
                Ctl::Value(_) => {}
                Ctl::Break => break,
                Ctl::Continue => {}
                other => return other,
            }
            if let Ctl::Error(e) = self.eval_body(&next, line, shell) {
                return Ctl::Error(e);
            }
        }
        Ctl::Value(String::new())
    }

    fn c_while(&mut self, argv: &[String], line: usize, shell: &mut GpuiShell) -> Ctl {
        if argv.len() != 3 {
            return Ctl::err(line, "wrong # args: should be \"while test body\"");
        }
        let (test, body) = (argv[1].clone(), argv[2].clone());
        loop {
            let cond = match self.eval_expr(&test, line) {
                Ok(v) => v,
                Err(m) => return Ctl::err(line, m),
            };
            match truth(&cond) {
                Ok(true) => {}
                Ok(false) => break,
                Err(m) => return Ctl::err(line, m),
            }
            match self.eval_body(&body.clone(), line, shell) {
                Ctl::Value(_) => {}
                Ctl::Break => break,
                Ctl::Continue => {}
                other => return other,
            }
        }
        Ctl::Value(String::new())
    }

    fn c_proc(&mut self, argv: &[String], line: usize) -> Ctl {
        if argv.len() != 4 {
            return Ctl::err(line, "wrong # args: should be \"proc name args body\"");
        }
        let name = argv[1].clone();
        if name.is_empty() {
            return Ctl::err(line, "proc: empty name");
        }
        let arglist = match split_list(&argv[2]) {
            Ok(v) => v,
            Err(m) => return Ctl::err(line, format!("proc argList: {m}")),
        };
        let mut params = Vec::new();
        for a in arglist {
            let a = a.trim().to_string();
            if let Some(inner) = a.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                // `{name default...}` — keep the default's raw text so it
                // substitutes in the caller's scope at call time.
                let inner = inner.trim_start();
                match split_first_token(inner) {
                    (name, "") => params.push(Param {
                        name: name.to_string(),
                        default: None,
                    }),
                    (name, rest) => params.push(Param {
                        name: name.to_string(),
                        default: Some(rest.trim().to_string()),
                    }),
                }
            } else {
                params.push(Param {
                    name: a,
                    default: None,
                });
            }
        }
        let body = match parse_script(&argv[3]) {
            Ok(b) => b,
            Err(e) => return Ctl::err(line, format!("proc body: {e}")),
        };
        self.procs.insert(name, ProcDef { params, body });
        Ctl::Value(String::new())
    }

    fn call_proc(
        &mut self,
        name: &str,
        def: ProcDef,
        argv: &[String],
        line: usize,
        shell: &mut GpuiShell,
    ) -> Ctl {
        let given = &argv[1..];
        let required = def.params.iter().filter(|p| p.default.is_none()).count();
        if given.len() < required || given.len() > def.params.len() {
            return Ctl::err(
                line,
                format!(
                    "wrong # args: should be \"{name} {}\", got {}",
                    def.params
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                    argv[1..].join(" "),
                ),
            );
        }
        // Defaults substitute in the caller's scope, before pushing the frame.
        let mut bound: Vec<(String, String)> = Vec::new();
        for (pi, p) in def.params.iter().enumerate() {
            if let Some(v) = given.get(pi) {
                bound.push((p.name.clone(), v.clone()));
            } else if let Some(d) = &p.default {
                match self.subst_str(d, line, shell) {
                    Ok(v) => bound.push((p.name.clone(), v)),
                    Err(e) => return Ctl::Error(e),
                }
            }
        }
        self.scopes.push(HashMap::new());
        for (k, v) in bound {
            if let Some(top) = self.scopes.last_mut() {
                top.insert(k, v);
            }
        }
        let r = self.eval_cmds(&def.body, shell);
        self.scopes.pop();
        match r {
            Ctl::Value(s) | Ctl::Return(s) => Ctl::Value(s),
            Ctl::Break => Ctl::err(line, "invoked \"break\" outside of a loop"),
            Ctl::Continue => Ctl::err(line, "invoked \"continue\" outside of a loop"),
            Ctl::Error(e) => Ctl::Error(e),
        }
    }

    fn c_return(&mut self, argv: &[String], line: usize) -> Ctl {
        match argv.len() {
            1 => Ctl::Return(String::new()),
            2 => Ctl::Return(argv[1].clone()),
            _ => Ctl::err(line, "wrong # args: should be \"return ?value?\""),
        }
    }

    fn c_llength(&mut self, argv: &[String], line: usize) -> Ctl {
        if argv.len() != 2 {
            return Ctl::err(line, "wrong # args: should be \"llength list\"");
        }
        match split_list(&argv[1]) {
            Ok(v) => Ctl::Value(v.len().to_string()),
            Err(m) => Ctl::err(line, m),
        }
    }

    fn c_lindex(&mut self, argv: &[String], line: usize) -> Ctl {
        if argv.len() != 3 {
            return Ctl::err(line, "wrong # args: should be \"lindex list index\"");
        }
        let list = match split_list(&argv[1]) {
            Ok(v) => v,
            Err(m) => return Ctl::err(line, m),
        };
        match list_index(&argv[2], list.len(), line) {
            Ok(Some(i)) => Ctl::Value(list[i].clone()),
            Ok(None) => Ctl::Value(String::new()),
            Err(e) => Ctl::Error(e),
        }
    }

    fn c_lrange(&mut self, argv: &[String], line: usize) -> Ctl {
        if argv.len() != 4 {
            return Ctl::err(line, "wrong # args: should be \"lrange list first last\"");
        }
        let list = match split_list(&argv[1]) {
            Ok(v) => v,
            Err(m) => return Ctl::err(line, m),
        };
        let n = list.len();
        let first = match list_index(&argv[2], n, line) {
            Ok(Some(i)) => i,
            Ok(None) => return Ctl::Value(String::new()),
            Err(e) => return Ctl::Error(e),
        };
        let last = match list_index(&argv[3], n, line) {
            Ok(Some(i)) => i,
            Ok(None) => n.saturating_sub(1),
            Err(e) => return Ctl::Error(e),
        };
        if last < first {
            return Ctl::Value(String::new());
        }
        Ctl::Value(join_list(&list[first..=last.min(n - 1)]))
    }

    fn c_lappend(&mut self, argv: &[String], line: usize) -> Ctl {
        if argv.len() < 2 {
            return Ctl::err(line, "wrong # args: should be \"lappend varName ?value ...?\"");
        }
        let cur = self.lookup(&argv[1]).cloned().unwrap_or_default();
        let mut list = match split_list(&cur) {
            Ok(v) => v,
            Err(m) => return Ctl::err(line, m),
        };
        list.extend(argv[2..].iter().cloned());
        let joined = join_list(&list);
        self.assign(argv[1].clone(), joined.clone());
        Ctl::Value(joined)
    }

    fn c_concat(&mut self, argv: &[String], line: usize) -> Ctl {
        let mut out: Vec<String> = Vec::new();
        for a in &argv[1..] {
            match split_list(a) {
                Ok(v) => out.extend(v),
                Err(m) => return Ctl::err(line, m),
            }
        }
        Ctl::Value(join_list(&out))
    }

    fn c_join(&mut self, argv: &[String], line: usize) -> Ctl {
        if argv.len() < 2 || argv.len() > 3 {
            return Ctl::err(line, "wrong # args: should be \"join list ?joinString?\"");
        }
        let list = match split_list(&argv[1]) {
            Ok(v) => v,
            Err(m) => return Ctl::err(line, m),
        };
        let sep = argv.get(2).map(|s| s.as_str()).unwrap_or(" ");
        Ctl::Value(list.join(sep))
    }

    fn c_split(&mut self, argv: &[String], line: usize) -> Ctl {
        if argv.len() < 2 || argv.len() > 3 {
            return Ctl::err(line, "wrong # args: should be \"split string ?splitChars?\"");
        }
        let s = &argv[1];
        let parts: Vec<String> = match argv.get(2) {
            None => s.split_whitespace().map(|x| x.to_string()).collect(),
            Some(chars) if chars.is_empty() => s.chars().map(|c| c.to_string()).collect(),
            Some(chars) => {
                let set: Vec<char> = chars.chars().collect();
                s.split(|c| set.contains(&c)).map(|x| x.to_string()).collect()
            }
        };
        Ctl::Value(join_list(&parts))
    }

    fn c_string(&mut self, argv: &[String], line: usize) -> Ctl {
        if argv.len() < 2 {
            return Ctl::err(line, "wrong # args: should be \"string subcommand ...\"");
        }
        match argv[1].as_str() {
            "length" => {
                if argv.len() != 3 {
                    return Ctl::err(line, "string length: wants 1 arg");
                }
                Ctl::Value(argv[2].chars().count().to_string())
            }
            "range" => {
                if argv.len() != 5 {
                    return Ctl::err(line, "wrong # args: should be \"string range string first last\"");
                }
                let ch: Vec<char> = argv[2].chars().collect();
                let n = ch.len();
                let first = str_index(&argv[3], n).unwrap_or(0);
                let last = str_index(&argv[4], n).unwrap_or(n.saturating_sub(1));
                if last < first || n == 0 {
                    return Ctl::Value(String::new());
                }
                Ctl::Value(ch[first..=last.min(n - 1)].iter().collect())
            }
            "index" => {
                if argv.len() != 4 {
                    return Ctl::err(line, "wrong # args: should be \"string index string index\"");
                }
                let ch: Vec<char> = argv[2].chars().collect();
                match str_index(&argv[3], ch.len()) {
                    Some(i) => Ctl::Value(ch[i].to_string()),
                    None => Ctl::Value(String::new()),
                }
            }
            "match" => {
                if argv.len() != 4 {
                    return Ctl::err(line, "wrong # args: should be \"string match pattern string\"");
                }
                Ctl::Value(
                    if super::design::glob_match(&argv[2], &argv[3]) {
                        "1"
                    } else {
                        "0"
                    }
                    .to_string(),
                )
            }
            "tolower" => {
                if argv.len() != 3 {
                    return Ctl::err(line, "string tolower: wants 1 arg");
                }
                Ctl::Value(argv[2].to_lowercase())
            }
            "toupper" => {
                if argv.len() != 3 {
                    return Ctl::err(line, "string toupper: wants 1 arg");
                }
                Ctl::Value(argv[2].to_uppercase())
            }
            "trim" => {
                if argv.len() < 3 || argv.len() > 4 {
                    return Ctl::err(line, "wrong # args: should be \"string trim string ?chars?\"");
                }
                let chars: Vec<char> = argv
                    .get(3)
                    .map(|s| s.chars().collect())
                    .unwrap_or_else(|| vec![' ', '\t', '\n', '\r']);
                Ctl::Value(
                    argv[2]
                        .trim_matches(|c| chars.contains(&c))
                        .to_string(),
                )
            }
            "first" => {
                if argv.len() < 4 || argv.len() > 5 {
                    return Ctl::err(
                        line,
                        "wrong # args: should be \"string first needle haystack ?start?\"",
                    );
                }
                let start: usize = argv
                    .get(4)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let hay = &argv[3];
                let sub = &argv[2];
                let pos = hay
                    .char_indices()
                    .enumerate()
                    .skip_while(|(ci, _)| *ci < start)
                    .find_map(|(ci, (bi, _))| hay[bi..].starts_with(sub).then_some(ci));
                Ctl::Value(pos.map(|p| p.to_string()).unwrap_or_else(|| "-1".into()))
            }
            "equal" => {
                let mut i = 2;
                let mut nocase = false;
                if argv.get(i).is_some_and(|s| s == "-nocase") {
                    nocase = true;
                    i += 1;
                }
                if argv.len() != i + 2 {
                    return Ctl::err(line, "wrong # args: should be \"string equal ?-nocase? s1 s2\"");
                }
                let (a, b) = (&argv[i], &argv[i + 1]);
                let eq = if nocase {
                    a.eq_ignore_ascii_case(b)
                } else {
                    a == b
                };
                Ctl::Value(if eq { "1" } else { "0" }.to_string())
            }
            other => Ctl::err(line, format!("bad string subcommand \"{other}\": must be length, range, index, match, tolower, toupper, trim, first, or equal")),
        }
    }

    fn c_info(&mut self, argv: &[String], line: usize) -> Ctl {
        if argv.len() < 2 {
            return Ctl::err(line, "wrong # args: should be \"info subcommand ...\"");
        }
        match argv[1].as_str() {
            "exists" => {
                if argv.len() != 3 {
                    return Ctl::err(line, "info exists: wants 1 arg");
                }
                Ctl::Value(if self.lookup(&argv[2]).is_some() { "1" } else { "0" }.to_string())
            }
            "vars" => {
                let pat = argv.get(2).map(|s| s.as_str());
                let mut names: Vec<String> = self.scopes[0]
                    .keys()
                    .filter(|k| pat.is_none_or(|p| super::design::glob_match(p, k)))
                    .cloned()
                    .collect();
                names.sort();
                Ctl::Value(join_list(&names))
            }
            "procs" => {
                let pat = argv.get(2).map(|s| s.as_str());
                let mut names: Vec<String> = self
                    .procs
                    .keys()
                    .filter(|k| pat.is_none_or(|p| super::design::glob_match(p, k)))
                    .cloned()
                    .collect();
                names.sort();
                Ctl::Value(join_list(&names))
            }
            "commands" => {
                let mut names = vec![
                    "set", "unset", "puts", "expr", "if", "foreach", "for", "while", "proc",
                    "return", "break", "continue", "list", "llength", "lindex", "lrange",
                    "lappend", "concat", "join", "split", "string", "info", "catch", "error",
                    "eval", "subst", "source", "format", "incr", "append", "help", "get_cells",
                    "get_nets", "get_pins", "get_ports", "get_clocks", "get_timing_paths",
                    "get_property", "set_property", "report_property", "list_property",
                    "select_objects", "highlight_objects", "get_selected_objects",
                    "current_design", "report_timing", "report_utilization", "synth_design",
                    "opt_design", "place_design", "route_design", "write_bitstream",
                    "write_checkpoint", "create_clock", "mark_debug", "eco", "clear",
                ];
                let pat = argv.get(2).map(|s| s.as_str());
                names.retain(|n| pat.is_none_or(|p| super::design::glob_match(p, n)));
                names.sort();
                Ctl::Value(join_list(&names.iter().map(|s| s.to_string()).collect::<Vec<_>>()))
            }
            "args" => {
                if argv.len() != 3 {
                    return Ctl::err(line, "info args: wants 1 proc name");
                }
                match self.procs.get(&argv[2]).cloned() {
                    Some(d) => Ctl::Value(
                        d.params.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(" "),
                    ),
                    None => Ctl::err(line, format!("\"{}\" isn't a procedure", argv[2])),
                }
            }
            "body" => {
                if argv.len() != 3 {
                    return Ctl::err(line, "info body: wants 1 proc name");
                }
                // Bodies are stored parsed; report a placeholder (documented).
                if self.procs.contains_key(&argv[2]) {
                    Ctl::Value("<parsed body>".into())
                } else {
                    Ctl::err(line, format!("\"{}\" isn't a procedure", argv[2]))
                }
            }
            other => Ctl::err(
                line,
                format!("bad info subcommand \"{other}\": must be exists, vars, procs, commands, args, or body"),
            ),
        }
    }

    fn c_catch(&mut self, argv: &[String], line: usize, shell: &mut GpuiShell) -> Ctl {
        if argv.len() < 2 || argv.len() > 3 {
            return Ctl::err(line, "wrong # args: should be \"catch script ?varName?\"");
        }
        let script = argv[1].clone();
        let var = argv.get(2).cloned();
        match self.eval_body(&script, line, shell) {
            Ctl::Value(s) => {
                if let Some(v) = var {
                    self.assign(v, s);
                }
                Ctl::Value("0".into())
            }
            Ctl::Return(s) => {
                if let Some(v) = var {
                    self.assign(v, s);
                }
                Ctl::Value("0".into())
            }
            Ctl::Error(e) => {
                if let Some(v) = var {
                    self.assign(v, e.msg.clone());
                    self.assign(
                        "errorInfo".into(),
                        format!("{} (line {})", e.msg, e.line),
                    );
                }
                Ctl::Value("1".into())
            }
            Ctl::Break => Ctl::err(line, "invoked \"break\" outside of a loop"),
            Ctl::Continue => Ctl::err(line, "invoked \"continue\" outside of a loop"),
        }
    }

    fn c_error(&mut self, argv: &[String], line: usize) -> Ctl {
        if argv.len() < 2 || argv.len() > 3 {
            return Ctl::err(line, "wrong # args: should be \"error message ?info?\"");
        }
        Ctl::err(line, argv[1].clone())
    }

    fn c_eval(&mut self, argv: &[String], line: usize, shell: &mut GpuiShell) -> Ctl {
        if argv.len() < 2 {
            return Ctl::err(line, "wrong # args: should be \"eval arg ...\"");
        }
        let script = argv[1..].join(" ");
        self.eval_body(&script, line, shell)
    }

    fn c_subst(&mut self, argv: &[String], line: usize, shell: &mut GpuiShell) -> Ctl {
        let mut i = 1;
        let mut no_cmd = false;
        let mut no_var = false;
        let mut no_bs = false;
        while i < argv.len() && argv[i].starts_with('-') {
            match argv[i].as_str() {
                "-nocommands" => no_cmd = true,
                "-novariables" => no_var = true,
                "-nobackslashes" => no_bs = true,
                other => return Ctl::err(line, format!("bad option \"{other}\"")),
            }
            i += 1;
        }
        if argv.len() != i + 1 {
            return Ctl::err(
                line,
                "wrong # args: should be \"subst ?-nocommands? ?-novariables? ?-nobackslashes? string\"",
            );
        }
        let src: Vec<char> = argv[i].chars().collect();
        let mut out = String::new();
        let mut k = 0;
        while k < src.len() {
            let c = src[k];
            if c == '\\' && !no_bs && k + 1 < src.len() {
                match src[k + 1] {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    other => out.push(other),
                }
                k += 2;
            } else if c == '$' && !no_var {
                let vlen = var_source_len(&src[k..]);
                let vsrc: String = src[k..k + vlen].iter().collect();
                let vref = parse_script(&vsrc).ok().and_then(|cmds| {
                    cmds.into_iter().next().and_then(|cm| {
                        cm.words.into_iter().next().and_then(|w| {
                            w.segs.into_iter().find_map(|s| match s {
                                Seg::Var(v) => Some(v),
                                _ => None,
                            })
                        })
                    })
                });
                match vref {
                    Some(vref) => match self.var_subst(&vref, line, shell) {
                        Ok(s) => {
                            out.push_str(&s);
                            k += vlen;
                            continue;
                        }
                        Err(e) => return Ctl::Error(e),
                    },
                    None => {
                        out.push(c);
                        k += 1;
                    }
                }
            } else if c == '[' && !no_cmd {
                let clen = cmd_source_len(&src[k..]);
                let csrc: String = src[k..k + clen].iter().collect();
                let nested = parse_script(&csrc).ok().and_then(|cmds| {
                    cmds.into_iter().next().and_then(|cm| {
                        cm.words.into_iter().next().and_then(|w| {
                            w.segs.into_iter().find_map(|s| match s {
                                Seg::Cmd(n) => Some(n),
                                _ => None,
                            })
                        })
                    })
                });
                match nested {
                    Some(nested) => match self.eval_cmds(&nested, shell) {
                        Ctl::Value(s) | Ctl::Return(s) => {
                            out.push_str(&s);
                            k += clen;
                            continue;
                        }
                        Ctl::Error(e) => return Ctl::Error(e),
                        Ctl::Break => {
                            return Ctl::err(line, "invoked \"break\" outside of a loop")
                        }
                        Ctl::Continue => {
                            return Ctl::err(line, "invoked \"continue\" outside of a loop")
                        }
                    },
                    None => {
                        out.push(c);
                        k += 1;
                    }
                }
            } else {
                out.push(c);
                k += 1;
            }
        }
        Ctl::Value(out)
    }

    fn c_source(&mut self, argv: &[String], line: usize, shell: &mut GpuiShell) -> Ctl {
        if argv.len() != 2 {
            return Ctl::err(line, "wrong # args: should be \"source fileName\"");
        }
        match std::fs::read_to_string(&argv[1]) {
            Ok(text) => self.eval_body(&text, line, shell),
            Err(e) => Ctl::err(line, format!("couldn't read file \"{}\": {e}", argv[1])),
        }
    }

    fn c_format(&mut self, argv: &[String], line: usize) -> Ctl {
        if argv.len() < 2 {
            return Ctl::err(line, "wrong # args: should be \"format formatString ...\"");
        }
        match do_format(&argv[1], &argv[2..]) {
            Ok(s) => Ctl::Value(s),
            Err(m) => Ctl::err(line, m),
        }
    }

    fn c_incr(&mut self, argv: &[String], line: usize) -> Ctl {
        if argv.len() < 2 || argv.len() > 3 {
            return Ctl::err(line, "wrong # args: should be \"incr varName ?increment?\"");
        }
        let cur = self.lookup(&argv[1]).cloned().unwrap_or_else(|| "0".into());
        let cur_i: i64 = cur.trim().parse().map_err(|_| {
            TclError {
                msg: format!("expected integer but got \"{cur}\""),
                line,
            }
        })?;
        let inc: i64 = argv
            .get(2)
            .map(|s| {
                s.trim().parse().map_err(|_| TclError {
                    msg: format!("expected integer but got \"{s}\""),
                    line,
                })
            })
            .transpose()?
            .unwrap_or(1);
        let v = cur_i.wrapping_add(inc).to_string();
        self.assign(argv[1].clone(), v.clone());
        Ctl::Value(v)
    }

    fn c_append(&mut self, argv: &[String], line: usize) -> Ctl {
        if argv.len() < 2 {
            return Ctl::err(line, "wrong # args: should be \"append varName ?value ...?\"");
        }
        let mut cur = self.lookup(&argv[1]).cloned().unwrap_or_default();
        for a in &argv[2..] {
            cur.push_str(a);
        }
        self.assign(argv[1].clone(), cur.clone());
        Ctl::Value(cur)
    }

    // ---- expression evaluation ----

    /// Evaluate an `expr`/`if`/`while` condition string.
    pub fn eval_expr(&mut self, src: &str, line: usize) -> Result<XVal, String> {
        let mut xp = ExprParser {
            chars: src.chars().collect(),
            pos: 0,
            line,
            interp: self,
        };
        let v = xp.parse_ternary()?;
        xp.skip_ws();
        if xp.pos != xp.chars.len() {
            return Err(format!("syntax error in expression \"{src}\""));
        }
        Ok(v)
    }
}

/// Source-text length of a `$var` reference (for `subst` scanning).
fn var_source_len(src: &[char]) -> usize {
    // src[0] == '$'
    if src.get(1) == Some(&'{') {
        let mut k = 2;
        while k < src.len() && src[k] != '}' {
            k += 1;
        }
        return (k + 1).min(src.len());
    }
    let mut k = 1;
    while k < src.len() && (src[k].is_ascii_alphanumeric() || src[k] == '_') {
        k += 1;
    }
    if k < src.len() && src[k] == '(' {
        let mut depth = 1;
        k += 1;
        while k < src.len() && depth > 0 {
            if src[k] == '(' {
                depth += 1;
            } else if src[k] == ')' {
                depth -= 1;
            }
            k += 1;
        }
    }
    k
}

/// Source-text length of a `[...]` substitution (for `subst` scanning).
fn cmd_source_len(src: &[char]) -> usize {
    let mut k = 1;
    let mut depth = 1;
    while k < src.len() && depth > 0 {
        match src[k] {
            '[' => depth += 1,
            ']' => depth -= 1,
            '\\' => k += 1,
            _ => {}
        }
        k += 1;
    }
    k
}

/// Split off the first whitespace-delimited token: `(token, rest)`.
fn split_first_token(s: &str) -> (&str, &str) {
    match s.find(|c: char| c.is_whitespace()) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

// ---- Tcl lists ----

/// Split a Tcl list string into elements (braces, quotes, backslashes).
pub fn split_list(s: &str) -> Result<Vec<String>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut k = 0;
    while k < chars.len() {
        while k < chars.len() && chars[k].is_whitespace() {
            k += 1;
        }
        if k >= chars.len() {
            break;
        }
        let mut elem = String::new();
        if chars[k] == '{' {
            k += 1;
            let mut depth = 1;
            while k < chars.len() && depth > 0 {
                match chars[k] {
                    '\\' if k + 1 < chars.len()
                        && matches!(chars[k + 1], '{' | '}' | '\\') =>
                    {
                        elem.push(chars[k + 1]);
                        k += 2;
                    }
                    '{' => {
                        depth += 1;
                        elem.push('{');
                        k += 1;
                    }
                    '}' => {
                        depth -= 1;
                        if depth > 0 {
                            elem.push('}');
                        }
                        k += 1;
                    }
                    c => {
                        elem.push(c);
                        k += 1;
                    }
                }
            }
            if depth != 0 {
                return Err("list: missing close-brace".into());
            }
        } else if chars[k] == '"' {
            k += 1;
            while k < chars.len() && chars[k] != '"' {
                if chars[k] == '\\' && k + 1 < chars.len() {
                    elem.push(chars[k + 1]);
                    k += 2;
                } else {
                    elem.push(chars[k]);
                    k += 1;
                }
            }
            if k >= chars.len() {
                return Err("list: missing close-quote".into());
            }
            k += 1;
        } else {
            while k < chars.len() && !chars[k].is_whitespace() {
                if chars[k] == '\\' && k + 1 < chars.len() {
                    elem.push(chars[k + 1]);
                    k += 2;
                } else {
                    elem.push(chars[k]);
                    k += 1;
                }
            }
        }
        out.push(elem);
    }
    Ok(out)
}

/// Join elements into a canonical Tcl list string (brace-quoting as needed).
pub fn join_list(items: &[String]) -> String {
    items.iter().map(|s| quote_list_elem(s)).collect::<Vec<_>>().join(" ")
}

fn quote_list_elem(s: &str) -> String {
    if s.is_empty() {
        return "{}".to_string();
    }
    let needs = s.chars().any(|c| {
        c.is_whitespace() || matches!(c, '{' | '}' | '"' | '[' | ']' | '$' | ';' | '#' | '\\')
    });
    if !needs {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('{');
    for c in s.chars() {
        if c == '{' || c == '}' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('}');
    out
}

/// Resolve `lindex`-style index: integer, `end`, `end-N`. None = out of range.
fn list_index(spec: &str, len: usize, line: usize) -> Result<Option<usize>, TclError> {
    let spec = spec.trim();
    if spec == "end" {
        return Ok(if len == 0 { None } else { Some(len - 1) });
    }
    if let Some(rest) = spec.strip_prefix("end-") {
        match rest.parse::<usize>() {
            Ok(n) if n < len => return Ok(Some(len - 1 - n)),
            _ => return Ok(None),
        }
    }
    if let Some(rest) = spec.strip_prefix("end+") {
        match rest.parse::<usize>() {
            Ok(n) => return Ok(Some(len - 1 + n).filter(|&i| i < len)),
            Err(_) => {
                return Err(TclError {
                    msg: format!("bad index \"{spec}\": must be integer or end?-?integer?"),
                    line,
                })
            }
        }
    }
    match spec.parse::<i64>() {
        Ok(i) if i >= 0 => Ok(usize::try_from(i).ok().filter(|&i| i < len)),
        Ok(_) => Ok(None),
        Err(_) => Err(TclError {
            msg: format!("bad index \"{spec}\": must be integer or end?-?integer?"),
            line,
        }),
    }
}

/// Resolve `string range` index: integer or `end`/`end-N`. Clamps into range.
fn str_index(spec: &str, len: usize) -> Option<usize> {
    let spec = spec.trim();
    if len == 0 {
        return None;
    }
    if spec == "end" {
        return Some(len - 1);
    }
    if let Some(rest) = spec.strip_prefix("end-") {
        return rest.parse::<usize>().ok().and_then(|n| len.checked_sub(1 + n));
    }
    spec.parse::<i64>().ok().and_then(|i| {
        if i < 0 {
            Some(0)
        } else {
            Some((i as usize).min(len - 1))
        }
    })
}

// ---- expr ----

/// An `expr` value: integer, double, or string.
#[derive(Clone, Debug)]
pub enum XVal {
    I(i64),
    F(f64),
    S(String),
}

impl XVal {
    fn to_string(&self) -> String {
        match self {
            XVal::I(i) => i.to_string(),
            XVal::F(f) => format!("{f:?}"),
            XVal::S(s) => s.clone(),
        }
    }
    fn as_f64(&self) -> Result<f64, String> {
        match self {
            XVal::I(i) => Ok(*i as f64),
            XVal::F(f) => Ok(*f),
            XVal::S(s) => s
                .trim()
                .parse::<f64>()
                .map_err(|_| format!("expected floating-point number but got \"{s}\"")),
        }
    }
    fn as_i64(&self) -> Result<i64, String> {
        match self {
            XVal::I(i) => Ok(*i),
            XVal::F(f) => Ok(*f as i64),
            XVal::S(s) => s
                .trim()
                .parse::<i64>()
                .map_err(|_| format!("expected integer but got \"{s}\"")),
        }
    }
    fn is_num_str(s: &str) -> bool {
        let t = s.trim();
        t.parse::<i64>().is_ok() || t.parse::<f64>().is_ok()
    }
}

/// Tcl truthiness: 0/""/false/no/off -> false; nonzero/true/yes/on -> true.
fn truth(v: &XVal) -> Result<bool, String> {
    match v {
        XVal::I(i) => Ok(*i != 0),
        XVal::F(f) => Ok(*f != 0.0),
        XVal::S(s) => {
            let t = s.trim();
            if t.is_empty() {
                return Ok(false);
            }
            if let Ok(i) = t.parse::<i64>() {
                return Ok(i != 0);
            }
            if let Ok(f) = t.parse::<f64>() {
                return Ok(f != 0.0);
            }
            match t.to_ascii_lowercase().as_str() {
                "true" | "yes" | "on" => Ok(true),
                "false" | "no" | "off" => Ok(false),
                _ => Err(format!("expected boolean value but got \"{s}\"")),
            }
        }
    }
}

struct ExprParser<'a> {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    interp: &'a mut TclInterp,
}

impl<'a> ExprParser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn eat(&mut self, s: &str) -> bool {
        self.skip_ws();
        if self.chars[self.pos..].starts_with(&s.chars().collect::<Vec<_>>()[..]) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }
    fn err(&self, msg: impl Into<String>) -> String {
        msg.into()
    }

    fn parse_ternary(&mut self) -> Result<XVal, String> {
        let c = self.parse_or()?;
        self.skip_ws();
        if self.peek() == Some('?') {
            self.pos += 1;
            let t = self.parse_ternary()?;
            self.skip_ws();
            if self.peek() != Some(':') {
                return Err(self.err("expr: expected ':' in ternary"));
            }
            self.pos += 1;
            let e = self.parse_ternary()?;
            Ok(if truth(&c)? { t } else { e })
        } else {
            Ok(c)
        }
    }

    fn parse_or(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_and()?;
        while self.eat("||") {
            let r = self.parse_and()?;
            l = XVal::I(i64::from(truth(&l)? || truth(&r)?));
        }
        Ok(l)
    }
    fn parse_and(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_bitor()?;
        while self.eat("&&") {
            let r = self.parse_bitor()?;
            l = XVal::I(i64::from(truth(&l)? && truth(&r)?));
        }
        Ok(l)
    }
    fn parse_bitor(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_bitxor()?;
        loop {
            self.skip_ws();
            if self.peek() == Some('|') && self.chars.get(self.pos + 1) != Some(&'|') {
                self.pos += 1;
                let r = self.parse_bitxor()?;
                l = XVal::I(l.as_i64()? | r.as_i64()?);
            } else {
                break;
            }
        }
        Ok(l)
    }
    fn parse_bitxor(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_bitand()?;
        while self.eat("^") {
            // careful: `^` also needs to not eat `^^`? `^^` isn't an op; fine.
            let r = self.parse_bitand()?;
            l = XVal::I(l.as_i64()? ^ r.as_i64()?);
        }
        Ok(l)
    }
    fn parse_bitand(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_eq()?;
        loop {
            self.skip_ws();
            if self.peek() == Some('&') && self.chars.get(self.pos + 1) != Some(&'&') {
                self.pos += 1;
                let r = self.parse_eq()?;
                l = XVal::I(l.as_i64()? & r.as_i64()?);
            } else {
                break;
            }
        }
        Ok(l)
    }
    fn parse_eq(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_cmp()?;
        loop {
            if self.eat("==") {
                let r = self.parse_cmp()?;
                l = XVal::I(i64::from(xeq(&l, &r)));
            } else if self.eat("!=") {
                let r = self.parse_cmp()?;
                l = XVal::I(i64::from(!xeq(&l, &r)));
            } else if self.eat_word_op("eq") {
                let r = self.parse_cmp()?;
                l = XVal::I(i64::from(l.to_string() == r.to_string()));
            } else if self.eat_word_op("ne") {
                let r = self.parse_cmp()?;
                l = XVal::I(i64::from(l.to_string() != r.to_string()));
            } else {
                break;
            }
        }
        Ok(l)
    }
    /// `eq`/`ne`/`in`/`ni` only match as whole words.
    fn eat_word_op(&mut self, op: &str) -> bool {
        self.skip_ws();
        let end = self.pos + op.len();
        if end <= self.chars.len()
            && self.chars[self.pos..end].iter().collect::<String>() == op
            && self.chars.get(end).is_none_or(|c| !c.is_ascii_alphanumeric() && *c != '_')
        {
            // Also require the char before to be an operator boundary — skip_ws
            // already guarantees we are at a token start.
            self.pos = end;
            true
        } else {
            false
        }
    }
    fn parse_cmp(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_shift()?;
        loop {
            // `<=` / `>=` first; a lone `<` / `>` is unambiguous here because
            // `<<` / `>>` are consumed by parse_shift below.
            let op = if self.eat("<=") {
                "<="
            } else if self.eat(">=") {
                ">="
            } else if self.eat("<") {
                "<"
            } else if self.eat(">") {
                ">"
            } else {
                break;
            };
            let r = self.parse_shift()?;
            l = XVal::I(i64::from(xcmp(&l, &r, op)?));
        }
        Ok(l)
    }
    fn parse_shift(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_add()?;
        loop {
            if self.eat("<<") {
                let r = self.parse_add()?;
                l = XVal::I(l.as_i64()?.wrapping_shl(r.as_i64()? as u32));
            } else if self.eat(">>") {
                let r = self.parse_add()?;
                l = XVal::I(l.as_i64()?.wrapping_shr(r.as_i64()? as u32));
            } else {
                break;
            }
        }
        Ok(l)
    }
    fn parse_add(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_mul()?;
        loop {
            if self.eat("+") {
                let r = self.parse_mul()?;
                l = xadd(&l, &r);
            } else if self.eat("-") {
                let r = self.parse_mul()?;
                l = xsub(&l, &r);
            } else {
                break;
            }
        }
        Ok(l)
    }
    fn parse_mul(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_pow()?;
        loop {
            if self.eat("*") {
                // `**` handled in parse_pow; a second `*` here would be a
                // syntax error in real Tcl too — treat as multiply and let the
                // next parse fail naturally.
                if self.peek() == Some('*') {
                    return Err(self.err("expr: unexpected **"));
                }
                let r = self.parse_pow()?;
                l = xmul(&l, &r);
            } else if self.eat("/") {
                let r = self.parse_pow()?;
                l = xdiv(&l, &r)?;
            } else if self.eat("%") {
                let r = self.parse_pow()?;
                let (a, b) = (l.as_i64()?, r.as_i64()?);
                if b == 0 {
                    return Err(self.err("expr: divide by zero"));
                }
                l = XVal::I(a % b);
            } else {
                break;
            }
        }
        Ok(l)
    }
    fn parse_pow(&mut self) -> Result<XVal, String> {
        let base = self.parse_unary()?;
        if self.eat("**") {
            let exp = self.parse_pow()?; // right-associative
            return xpow(&base, &exp);
        }
        Ok(base)
    }
    fn parse_unary(&mut self) -> Result<XVal, String> {
        self.skip_ws();
        match self.peek() {
            Some('-') => {
                self.pos += 1;
                let v = self.parse_unary()?;
                Ok(xneg(&v))
            }
            Some('+') => {
                self.pos += 1;
                self.parse_unary()
            }
            Some('!') if self.chars.get(self.pos + 1) != Some(&'=') => {
                self.pos += 1;
                let v = self.parse_unary()?;
                Ok(XVal::I(i64::from(!truth(&v)?)))
            }
            Some('~') => {
                self.pos += 1;
                let v = self.parse_unary()?;
                Ok(XVal::I(!v.as_i64()?))
            }
            _ => self.parse_primary(),
        }
    }
    fn parse_primary(&mut self) -> Result<XVal, String> {
        self.skip_ws();
        match self.peek() {
            None => Err(self.err("expr: unexpected end")),
            Some('(') => {
                self.pos += 1;
                let v = self.parse_ternary()?;
                self.skip_ws();
                if self.peek() != Some(')') {
                    return Err(self.err("expr: expected ')'"));
                }
                self.pos += 1;
                Ok(v)
            }
            Some('"') => {
                self.pos += 1;
                let mut s = String::new();
                while let Some(c) = self.peek() {
                    if c == '"' {
                        self.pos += 1;
                        break;
                    }
                    if c == '\\' {
                        self.pos += 1;
                        match self.peek() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(c2) => s.push(c2),
                            None => break,
                        }
                        self.pos += 1;
                    } else {
                        s.push(c);
                        self.pos += 1;
                    }
                }
                Ok(XVal::S(s))
            }
            Some('{') => {
                self.pos += 1;
                let mut s = String::new();
                let mut depth = 1;
                while let Some(c) = self.peek() {
                    self.pos += 1;
                    match c {
                        '{' => {
                            depth += 1;
                            s.push(c);
                        }
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            s.push(c);
                        }
                        _ => s.push(c),
                    }
                }
                Ok(XVal::S(s))
            }
            Some('$') => {
                self.pos += 1;
                let name = self.parse_ident();
                let key = if self.peek() == Some('(') {
                    self.pos += 1;
                    let mut idx = String::new();
                    while let Some(c) = self.peek() {
                        if c == ')' {
                            self.pos += 1;
                            break;
                        }
                        idx.push(c);
                        self.pos += 1;
                    }
                    format!("{name}({idx})")
                } else {
                    name.clone()
                };
                match self.interp.lookup(&key) {
                    Some(v) => Ok(XVal::S(v.clone())),
                    None => Err(format!("can't read \"{name}\": no such variable")),
                }
            }
            Some(c) if c.is_ascii_digit() || c == '.' => self.parse_number(),
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                let name = self.parse_ident()?;
                self.skip_ws();
                if self.peek() == Some('(') {
                    // function call
                    self.pos += 1;
                    let mut args = Vec::new();
                    self.skip_ws();
                    if self.peek() != Some(')') {
                        loop {
                            args.push(self.parse_ternary()?);
                            self.skip_ws();
                            if self.peek() == Some(',') {
                                self.pos += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    self.skip_ws();
                    if self.peek() != Some(')') {
                        return Err(self.err("expr: expected ')' after function args"));
                    }
                    self.pos += 1;
                    xfunc(&name, &args)
                } else {
                    // bare word: variable reference
                    match self.interp.lookup(&name) {
                        Some(v) => Ok(XVal::S(v.clone())),
                        None => Err(format!("can't read \"{name}\": no such variable")),
                    }
                }
            }
            Some(c) => Err(self.err(format!("expr: unexpected character '{c}'"))),
        }
    }
    fn parse_ident(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                s.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        s
    }
    fn parse_number(&mut self) -> Result<XVal, String> {
        let start = self.pos;
        let mut is_float = false;
        // hex / octal / binary integers like Tcl
        if self.chars.get(self.pos) == Some(&'0')
            && matches!(self.chars.get(self.pos + 1), Some('x') | Some('X'))
        {
            self.pos += 2;
            let h: String = self.take_while(|c| c.is_ascii_hexdigit());
            let v = i64::from_str_radix(&h, 16)
                .map_err(|_| self.err(format!("bad number \"{}\"", &self.src_slice(start))))?;
            return Ok(XVal::I(v));
        }
        let int_part: String = self.take_while(|c| c.is_ascii_digit());
        if self.peek() == Some('.') {
            is_float = true;
            self.pos += 1;
        }
        let frac_part: String = self.take_while(|c| c.is_ascii_digit());
        if matches!(self.peek(), Some('e') | Some('E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some('+') | Some('-')) {
                self.pos += 1;
            }
            let _e: String = self.take_while(|c| c.is_ascii_digit());
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        let _ = (int_part, frac_part);
        if is_float {
            text.parse::<f64>()
                .map(XVal::F)
                .map_err(|_| self.err(format!("bad number \"{text}\"")))
        } else {
            text.parse::<i64>()
                .map(XVal::I)
                .map_err(|_| self.err(format!("bad number \"{text}\"")))
        }
    }
    fn take_while(&mut self, f: impl Fn(char) -> bool) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if f(c) {
                s.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        s
    }
    fn src_slice(&self, start: usize) -> String {
        self.chars[start..self.pos.min(self.chars.len())]
            .iter()
            .collect()
    }
}

fn xeq(a: &XVal, b: &XVal) -> bool {
    match (a, b) {
        (XVal::I(x), XVal::I(y)) => x == y,
        (XVal::S(x), XVal::S(y)) if !XVal::is_num_str(x) || !XVal::is_num_str(y) => x == y,
        _ => a.as_f64().ok() == b.as_f64().ok() && a.as_f64().is_ok(),
    }
}

fn xcmp(a: &XVal, b: &XVal, op: &str) -> Result<bool, String> {
    let ord = match (a, b) {
        (XVal::S(x), XVal::S(y)) if !XVal::is_num_str(x) || !XVal::is_num_str(y) => {
            x.cmp(y)
        }
        _ => {
            let (x, y) = (a.as_f64()?, b.as_f64()?);
            x.partial_cmp(&y).ok_or("expr: unordered comparison")?
        }
    };
    Ok(match op {
        "<" => ord.is_lt(),
        "<=" => ord.is_le(),
        ">" => ord.is_gt(),
        ">=" => ord.is_ge(),
        _ => false,
    })
}

fn xadd(a: &XVal, b: &XVal) -> XVal {
    match (a, b) {
        (XVal::I(x), XVal::I(y)) => match x.checked_add(*y) {
            Some(v) => XVal::I(v),
            None => XVal::F(*x as f64 + *y as f64),
        },
        _ => XVal::F(a.as_f64().unwrap_or(f64::NAN) + b.as_f64().unwrap_or(f64::NAN)),
    }
}
fn xsub(a: &XVal, b: &XVal) -> XVal {
    match (a, b) {
        (XVal::I(x), XVal::I(y)) => match x.checked_sub(*y) {
            Some(v) => XVal::I(v),
            None => XVal::F(*x as f64 - *y as f64),
        },
        _ => XVal::F(a.as_f64().unwrap_or(f64::NAN) - b.as_f64().unwrap_or(f64::NAN)),
    }
}
fn xmul(a: &XVal, b: &XVal) -> XVal {
    match (a, b) {
        (XVal::I(x), XVal::I(y)) => match x.checked_mul(*y) {
            Some(v) => XVal::I(v),
            None => XVal::F(*x as f64 * *y as f64),
        },
        _ => XVal::F(a.as_f64().unwrap_or(f64::NAN) * b.as_f64().unwrap_or(f64::NAN)),
    }
}
fn xdiv(a: &XVal, b: &XVal) -> Result<XVal, String> {
    match (a, b) {
        (XVal::I(x), XVal::I(y)) => {
            if *y == 0 {
                return Err("expr: divide by zero".into());
            }
            Ok(XVal::I(x / y))
        }
        _ => {
            let (x, y) = (a.as_f64()?, b.as_f64()?);
            Ok(XVal::F(x / y))
        }
    }
}
fn xpow(a: &XVal, b: &XVal) -> Result<XVal, String> {
    match (a, b) {
        (XVal::I(x), XVal::I(y)) if *y >= 0 => {
            let mut r: i64 = 1;
            for _ in 0..*y {
                r = match r.checked_mul(*x) {
                    Some(v) => v,
                    None => return Ok(XVal::F((*x as f64).powi(*y as i32))),
                };
            }
            Ok(XVal::I(r))
        }
        _ => Ok(XVal::F(a.as_f64()?.powf(b.as_f64()?))),
    }
}
fn xneg(a: &XVal) -> XVal {
    match a {
        XVal::I(i) => XVal::I(i.wrapping_neg()),
        XVal::F(f) => XVal::F(-f),
        XVal::S(s) => s
            .trim()
            .parse::<i64>()
            .map(|i| XVal::I(i.wrapping_neg()))
            .unwrap_or_else(|_| {
                XVal::F(-s.trim().parse::<f64>().unwrap_or(f64::NAN))
            }),
    }
}

fn xfunc(name: &str, args: &[XVal]) -> Result<XVal, String> {
    let arity = |n: usize| {
        if args.len() == n {
            Ok(())
        } else {
            Err(format!("expr: {name} wants {n} args, got {}", args.len()))
        }
    };
    match name {
        "abs" => {
            arity(1)?;
            Ok(match &args[0] {
                XVal::I(i) => XVal::I(i.wrapping_abs()),
                XVal::F(f) => XVal::F(f.abs()),
                XVal::S(s) => {
                    if let Ok(i) = s.trim().parse::<i64>() {
                        XVal::I(i.wrapping_abs())
                    } else {
                        XVal::F(s.trim().parse::<f64>().map_err(|_| {
                            format!("expected number but got \"{s}\"")
                        })?.abs())
                    }
                }
            })
        }
        "int" => {
            arity(1)?;
            Ok(XVal::I(args[0].as_i64()?))
        }
        "double" => {
            arity(1)?;
            Ok(XVal::F(args[0].as_f64()?))
        }
        "round" => {
            arity(1)?;
            Ok(XVal::I(args[0].as_f64()?.round() as i64))
        }
        "sqrt" => {
            arity(1)?;
            let f = args[0].as_f64()?;
            if f < 0.0 {
                return Err("expr: sqrt of negative number".into());
            }
            Ok(XVal::F(f.sqrt()))
        }
        "pow" => {
            arity(2)?;
            Ok(XVal::F(args[0].as_f64()?.powf(args[1].as_f64()?)))
        }
        _ => Err(format!("expr: unknown function \"{name}\"")),
    }
}

// ---- format ----

fn do_format(fmt: &str, args: &[String]) -> Result<String, String> {
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut ai = 0;
    let mut k = 0;
    while k < chars.len() {
        if chars[k] != '%' {
            out.push(chars[k]);
            k += 1;
            continue;
        }
        k += 1;
        if k < chars.len() && chars[k] == '%' {
            out.push('%');
            k += 1;
            continue;
        }
        // flags / width / precision (parsed, partially honored)
        while k < chars.len() && matches!(chars[k], '-' | '+' | ' ' | '0' | '#') {
            k += 1;
        }
        let mut width = String::new();
        while k < chars.len() && chars[k].is_ascii_digit() {
            width.push(chars[k]);
            k += 1;
        }
        let mut prec: Option<usize> = None;
        if k < chars.len() && chars[k] == '.' {
            k += 1;
            let mut p = String::new();
            while k < chars.len() && chars[k].is_ascii_digit() {
                p.push(chars[k]);
                k += 1;
            }
            prec = p.parse().ok();
        }
        if k >= chars.len() {
            return Err("format: trailing %".into());
        }
        let conv = chars[k];
        k += 1;
        if ai >= args.len() {
            return Err(format!("format: not enough args for %{conv}"));
        }
        let a = &args[ai];
        ai += 1;
        let w: usize = width.parse().unwrap_or(0);
        let pad = |s: String| {
            if s.len() >= w {
                s
            } else {
                format!("{}{s}", " ".repeat(w - s.len()))
            }
        };
        match conv {
            's' => out.push_str(&pad(a.clone())),
            'd' | 'i' => {
                let v: i64 = a.trim().parse().map_err(|_| {
                    format!("format: expected integer for %d, got \"{a}\"")
                })?;
                out.push_str(&pad(v.to_string()));
            }
            'f' => {
                let v: f64 = a.trim().parse().map_err(|_| {
                    format!("format: expected number for %f, got \"{a}\"")
                })?;
                let s = match prec {
                    Some(p) => format!("{v:.p$}", p = p),
                    None => format!("{v:.6}"),
                };
                out.push_str(&pad(s));
            }
            'x' => {
                let v: i64 = a.trim().parse().map_err(|_| {
                    format!("format: expected integer for %x, got \"{a}\"")
                })?;
                out.push_str(&pad(format!("{v:x}")));
            }
            'X' => {
                let v: i64 = a.trim().parse().map_err(|_| {
                    format!("format: expected integer for %X, got \"{a}\"")
                })?;
                out.push_str(&pad(format!("{v:X}")));
            }
            'o' => {
                let v: i64 = a.trim().parse().map_err(|_| {
                    format!("format: expected integer for %o, got \"{a}\"")
                })?;
                out.push_str(&pad(format!("{v:o}")));
            }
            'c' => {
                let v: i64 = a.trim().parse().map_err(|_| {
                    format!("format: expected integer for %c, got \"{a}\"")
                })?;
                out.push_str(&pad(
                    char::from_u32(v as u32).unwrap_or('?').to_string(),
                ));
            }
            _ => return Err(format!("format: unsupported conversion %{conv}")),
        }
    }
    Ok(out)
}

// ---- -filter expression language ----

/// Evaluate a Vivado-style `-filter` expression (`==`, `!=`, `=~`, `!~`,
/// `<`, `<=`, `>`, `>=`, `&&`, `||`, `!`, parens) against a property map.
/// Property names are case-insensitive.
pub fn eval_filter(expr: &str, props: &HashMap<String, String>) -> Result<bool, String> {
    let mut fp = FilterParser {
        chars: expr.chars().collect(),
        pos: 0,
    };
    let v = fp.parse_or()?;
    fp.skip_ws();
    if fp.pos != fp.chars.len() {
        let tail: String = fp.chars[fp.pos..].iter().collect();
        let tail: String = tail.chars().take(24).collect();
        return Err(format!("filter: syntax error near \"{tail}\""));
    }
    v.eval(props)
}

#[derive(Clone, Debug)]
enum FExpr {
    And(Box<FExpr>, Box<FExpr>),
    Or(Box<FExpr>, Box<FExpr>),
    Not(Box<FExpr>),
    Cmp { prop: String, op: String, lit: String },
}

impl FExpr {
    fn eval(&self, props: &HashMap<String, String>) -> Result<bool, String> {
        match self {
            FExpr::And(a, b) => Ok(a.eval(props)? && b.eval(props)?),
            FExpr::Or(a, b) => Ok(a.eval(props)? || b.eval(props)?),
            FExpr::Not(a) => Ok(!a.eval(props)?),
            FExpr::Cmp { prop, op, lit } => {
                let val = props
                    .get(&prop.to_uppercase())
                    .map(|s| s.as_str())
                    .unwrap_or("");
                Ok(match op.as_str() {
                    "==" => cmp_eq(val, lit),
                    "!=" => !cmp_eq(val, lit),
                    "=~" => super::design::glob_match(lit, val),
                    "!~" => !super::design::glob_match(lit, val),
                    "<" | "<=" | ">" | ">=" => {
                        let r = cmp_ord(val, lit);
                        match op.as_str() {
                            "<" => r.is_lt(),
                            "<=" => r.is_le(),
                            ">" => r.is_gt(),
                            _ => r.is_ge(),
                        }
                    }
                    _ => false,
                })
            }
        }
    }
}

fn cmp_eq(a: &str, b: &str) -> bool {
    if let (Ok(x), Ok(y)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        x == y
    } else {
        a == b
    }
}

fn cmp_ord(a: &str, b: &str) -> std::cmp::Ordering {
    if let (Ok(x), Ok(y)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
    } else {
        a.cmp(b)
    }
}

struct FilterParser {
    chars: Vec<char>,
    pos: usize,
}

impl FilterParser {
    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }
    fn eat(&mut self, s: &str) -> bool {
        self.skip_ws();
        let pat: Vec<char> = s.chars().collect();
        if self.chars[self.pos..].starts_with(&pat[..]) {
            self.pos += pat.len();
            true
        } else {
            false
        }
    }
    fn parse_or(&mut self) -> Result<FExpr, String> {
        let mut l = self.parse_and()?;
        while self.eat("||") {
            let r = self.parse_and()?;
            l = FExpr::Or(Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn parse_and(&mut self) -> Result<FExpr, String> {
        let mut l = self.parse_unary()?;
        while self.eat("&&") {
            let r = self.parse_unary()?;
            l = FExpr::And(Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn parse_unary(&mut self) -> Result<FExpr, String> {
        self.skip_ws();
        if self.eat("!") {
            return Ok(FExpr::Not(Box::new(self.parse_unary()?)));
        }
        if self.eat("(") {
            let e = self.parse_or()?;
            if !self.eat(")") {
                return Err("filter: expected ')'".into());
            }
            return Ok(e);
        }
        let prop = self.parse_operand()?;
        self.skip_ws();
        let op = if self.eat("==") {
            "=="
        } else if self.eat("!=") {
            "!="
        } else if self.eat("=~") {
            "=~"
        } else if self.eat("!~") {
            "!~"
        } else if self.eat("<=") {
            "<="
        } else if self.eat(">=") {
            ">="
        } else if self.eat("<") {
            "<"
        } else if self.eat(">") {
            ">"
        } else {
            return Err(format!("filter: expected comparison operator after \"{prop}\""));
        };
        let lit = self.parse_operand()?;
        Ok(FExpr::Cmp {
            prop,
            op: op.to_string(),
            lit,
        })
    }
    fn parse_operand(&mut self) -> Result<String, String> {
        self.skip_ws();
        match self.chars.get(self.pos) {
            Some('"') => {
                self.pos += 1;
                let mut s = String::new();
                while let Some(&c) = self.chars.get(self.pos) {
                    self.pos += 1;
                    if c == '"' {
                        return Ok(s);
                    }
                    if c == '\\' {
                        if let Some(&n) = self.chars.get(self.pos) {
                            s.push(n);
                            self.pos += 1;
                        }
                    } else {
                        s.push(c);
                    }
                }
                Err("filter: missing close-quote".into())
            }
            Some(_) => {
                let mut s = String::new();
                while let Some(&c) = self.chars.get(self.pos) {
                    if c.is_whitespace() || "()!&|".contains(c) {
                        break;
                    }
                    if (c == '=' || c == '<' || c == '>' || c == '~') && !s.is_empty() {
                        break;
                    }
                    s.push(c);
                    self.pos += 1;
                }
                if s.is_empty() {
                    return Err("filter: expected operand".into());
                }
                Ok(s)
            }
            None => Err("filter: expected operand".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interp() -> (TclInterp, GpuiShell) {
        (TclInterp::new(), GpuiShell::default())
    }

    fn ev(i: &mut TclInterp, sh: &mut GpuiShell, script: &str) -> String {
        i.eval(script, sh)
            .unwrap_or_else(|e| panic!("eval {script:?} failed: {e}"))
    }

    fn ev_err(i: &mut TclInterp, sh: &mut GpuiShell, script: &str) -> String {
        i.eval(script, sh).expect_err("expected error")
    }

    #[test]
    fn set_and_substitution() {
        let (mut i, mut sh) = interp();
        assert_eq!(ev(&mut i, &mut sh, "set x 42"), "42");
        assert_eq!(ev(&mut i, &mut sh, "set y $x"), "42");
        assert_eq!(ev(&mut i, &mut sh, r#"set z "val=$x!""#), "val=42!");
        assert_eq!(ev(&mut i, &mut sh, r"set w {val=$x!}"), "val=$x!");
        assert_eq!(ev(&mut i, &mut sh, "set x"), "42");
        let e = ev_err(&mut i, &mut sh, "set nosuchvar");
        assert!(e.contains("no such variable"), "{e}");
    }

    #[test]
    fn arrays() {
        let (mut i, mut sh) = interp();
        ev(&mut i, &mut sh, "set a(0) first");
        ev(&mut i, &mut sh, "set a(1) second");
        assert_eq!(ev(&mut i, &mut sh, "set a(1)"), "second");
        assert_eq!(ev(&mut i, &mut sh, "set i 0; set a($i)"), "first");
    }

    #[test]
    fn expr_arithmetic() {
        let (mut i, mut sh) = interp();
        assert_eq!(ev(&mut i, &mut sh, "expr {1 + 2 * 3}"), "7");
        assert_eq!(ev(&mut i, &mut sh, "expr {7 / 2}"), "3"); // integer division
        assert_eq!(ev(&mut i, &mut sh, "expr {7.0 / 2}"), "3.5");
        assert_eq!(ev(&mut i, &mut sh, "expr {2 ** 10}"), "1024");
        assert_eq!(ev(&mut i, &mut sh, "expr {(1 + 2) * 3}"), "9");
        assert_eq!(ev(&mut i, &mut sh, "expr {-2 ** 2}"), "-4");
        assert_eq!(ev(&mut i, &mut sh, "expr {0x10 + 1}"), "17");
        assert_eq!(ev(&mut i, &mut sh, "expr {1 < 2 && 3 > 4}"), "0");
        assert_eq!(ev(&mut i, &mut sh, r#"expr {"abc" eq "abc"}"#), "1");
        assert_eq!(ev(&mut i, &mut sh, "expr {5 % 3}"), "2");
        assert_eq!(ev(&mut i, &mut sh, "expr {1 ? 10 : 20}"), "10");
        assert_eq!(ev(&mut i, &mut sh, "expr {abs(-5) + int(2.9) + round(2.5)}"), "9");
        assert_eq!(ev(&mut i, &mut sh, "set n 6; expr {$n * 7}"), "42");
        let e = ev_err(&mut i, &mut sh, "expr {1 / 0}");
        assert!(e.contains("divide by zero"), "{e}");
    }

    #[test]
    fn if_elseif_else() {
        let (mut i, mut sh) = interp();
        assert_eq!(ev(&mut i, &mut sh, "if {1 < 2} {set r yes} else {set r no}"), "yes");
        assert_eq!(
            ev(&mut i, &mut sh, "if {2 < 1} {set r a} elseif {2 < 3} {set r b} else {set r c}"),
            "b"
        );
        assert_eq!(ev(&mut i, &mut sh, "if 0 {set r a}"), "");
    }

    #[test]
    fn foreach_for_while() {
        let (mut i, mut sh) = interp();
        assert_eq!(
            ev(&mut i, &mut sh, "set s 0; foreach x {1 2 3} {set s [expr {$s + $x}]}; set s"),
            "6"
        );
        assert_eq!(
            ev(&mut i, &mut sh, "set s \"\"; foreach {a b} {1 2 3 4} {append s $a$b.}; set s"),
            "12.34."
        );
        assert_eq!(
            ev(&mut i, &mut sh, "set s 0; for {set i 0} {$i < 5} {incr i} {set s [expr {$s+$i}]}; set s"),
            "10"
        );
        assert_eq!(
            ev(&mut i, &mut sh, "set i 0; while {$i < 3} {incr i}; set i"),
            "3"
        );
        // break / continue
        assert_eq!(
            ev(&mut i, &mut sh, "set s 0; foreach x {1 2 3 4} {if {$x > 2} break; set s [expr {$s+$x}]}; set s"),
            "3"
        );
        assert_eq!(
            ev(&mut i, &mut sh, "set s \"\"; foreach x {1 2 3} {if {$x == 2} continue; append s $x}; set s"),
            "13"
        );
    }

    #[test]
    fn proc_and_return() {
        let (mut i, mut sh) = interp();
        ev(&mut i, &mut sh, "proc add {a b} {expr {$a + $b}}");
        assert_eq!(ev(&mut i, &mut sh, "add 3 4"), "7");
        ev(&mut i, &mut sh, "proc greet {{name World}} {return \"hi $name\"}");
        assert_eq!(ev(&mut i, &mut sh, "greet"), "hi World");
        assert_eq!(ev(&mut i, &mut sh, "greet Tcl"), "hi Tcl");
        // locals do not leak
        ev(&mut i, &mut sh, "proc f {} {set tmp 1; return $tmp}");
        ev(&mut i, &mut sh, "f");
        let e = ev_err(&mut i, &mut sh, "set tmp");
        assert!(e.contains("no such variable"), "{e}");
        // early return
        ev(&mut i, &mut sh, "proc g {x} {if {$x > 0} {return pos}; return neg}");
        assert_eq!(ev(&mut i, &mut sh, "g 5"), "pos");
        assert_eq!(ev(&mut i, &mut sh, "g -5"), "neg");
        let e = ev_err(&mut i, &mut sh, "add 1");
        assert!(e.contains("wrong # args"), "{e}");
    }

    #[test]
    fn lists() {
        let (mut i, mut sh) = interp();
        assert_eq!(ev(&mut i, &mut sh, r#"list a "b c" d"#), "a {b c} d");
        assert_eq!(ev(&mut i, &mut sh, "llength {a {b c} d}"), "3");
        assert_eq!(ev(&mut i, &mut sh, "lindex {a b c} 1"), "b");
        assert_eq!(ev(&mut i, &mut sh, "lindex {a b c} end"), "c");
        assert_eq!(ev(&mut i, &mut sh, "lindex {a b c} end-1"), "b");
        assert_eq!(ev(&mut i, &mut sh, "lindex {a b c} 9"), "");
        assert_eq!(ev(&mut i, &mut sh, "lrange {a b c d} 1 2"), "b c");
        assert_eq!(ev(&mut i, &mut sh, r#"join {a b c} ,"#), "a,b,c");
        assert_eq!(ev(&mut i, &mut sh, r#"split "a,b,c" ,"#), "a b c");
        assert_eq!(ev(&mut i, &mut sh, "lappend mylist a b; lappend mylist c"), "a b c");
        assert_eq!(ev(&mut i, &mut sh, r#"concat {a b} {c d}"#), "a b c d");
    }

    #[test]
    fn string_cmd() {
        let (mut i, mut sh) = interp();
        assert_eq!(ev(&mut i, &mut sh, "string length hello"), "5");
        assert_eq!(ev(&mut i, &mut sh, "string range hello 1 3"), "ell");
        assert_eq!(ev(&mut i, &mut sh, "string index hello end"), "o");
        assert_eq!(ev(&mut i, &mut sh, r#"string match "h*o" hello"#), "1");
        assert_eq!(ev(&mut i, &mut sh, "string toupper hello"), "HELLO");
        assert_eq!(ev(&mut i, &mut sh, r#"string trim "  hi  ""#), "hi");
        assert_eq!(ev(&mut i, &mut sh, "string first l hello"), "2");
        assert_eq!(ev(&mut i, &mut sh, r#"string equal -nocase ABC abc"#), "1");
    }

    #[test]
    fn catch_and_error() {
        let (mut i, mut sh) = interp();
        assert_eq!(ev(&mut i, &mut sh, "catch {set x 1}"), "0");
        assert_eq!(ev(&mut i, &mut sh, "catch {error boom} msg"), "1");
        assert_eq!(ev(&mut i, &mut sh, "set msg"), "boom");
        let e = ev_err(&mut i, &mut sh, "error {custom fail}");
        assert!(e.contains("custom fail"), "{e}");
    }

    #[test]
    fn misc_core() {
        let (mut i, mut sh) = interp();
        assert_eq!(ev(&mut i, &mut sh, "incr c"), "1");
        assert_eq!(ev(&mut i, &mut sh, "incr c 4"), "5");
        assert_eq!(ev(&mut i, &mut sh, "append s a b; set s"), "ab");
        assert_eq!(ev(&mut i, &mut sh, r#"format "x=%d s=%s" 42 hi"#), "x=42 s=hi");
        assert_eq!(ev(&mut i, &mut sh, "info exists c"), "1");
        assert_eq!(ev(&mut i, &mut sh, "info exists nope"), "0");
        ev(&mut i, &mut sh, r#"puts "hello puts""#);
        assert!(i.output.contains("hello puts"), "puts buffer: {:?}", i.output);
        assert_eq!(ev(&mut i, &mut sh, "set v 3; subst {a$v}"), "a3");
        assert_eq!(ev(&mut i, &mut sh, "eval {set e 9}; set e"), "9");
        let e = ev_err(&mut i, &mut sh, "nosuchcommand");
        assert!(e.contains("invalid command name"), "{e}");
        assert!(e.starts_with("ERROR:"), "{e}");
    }

    #[test]
    fn filter_lang() {
        let mut props = HashMap::new();
        props.insert("TYPE".into(), "lut".into());
        props.insert("FANOUT".into(), "12".into());
        props.insert("NAME".into(), "u_lut3".into());
        assert!(eval_filter(r#"TYPE == lut"#, &props).unwrap());
        assert!(eval_filter(r#"TYPE == ff"#, &props).unwrap() == false);
        assert!(eval_filter(r#"FANOUT > 10 && NAME =~ u_*"#, &props).unwrap());
        assert!(eval_filter(r#"!(TYPE == ff || FANOUT < 3)"#, &props).unwrap());
        assert!(eval_filter(r#"NAME !~ "*ff*""#, &props).unwrap());
        assert!(eval_filter(r#"MISSING == """#, &props).unwrap());
        assert!(eval_filter(r#"type == LUT"#, &props).unwrap() == false); // case-sensitive value
        assert!(eval_filter(r#"BAD SYNTAX =="#, &props).is_err());
    }

    #[test]
    fn glob_match() {
        use super::super::design::glob_match;
        assert!(glob_match("u_*", "u_lut3"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("[a-z]_lut[0-9]", "u_lut3"));
        assert!(!glob_match("[a-z]_lut[0-9]", "1_lut3"));
        assert!(glob_match(r"a\*b", "a*b"));
        assert!(!glob_match(r"a\*b", "axb"));
    }
}
