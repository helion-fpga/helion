//! Integrated testbench runner core: discovery, parsing, interpretation,
//! VCD writing, and run-history persistence.
//!
//! A testbench (`*_tb.sv` / `tb_*.sv`) is a *stimulus program* for the
//! synthesizable DUT it instantiates. The runner elaborates the DUT with
//! `helion-sv`, drives the `helion-sim` event kernel from the testbench's
//! `initial` block, and reports `$display` output plus assertion failures
//! with file:line.
//!
//! ## Supported subset
//!
//! * `module tb; … endmodule` (no ports)
//! * `logic` / `reg` / `wire` declarations with optional `[msb:lsb]` and
//!   optional `= const` initializers
//! * one DUT instantiation with **named** port connections: `dut u (.a(x))`
//! * `initial begin … end` (several `initial` blocks run in source order)
//! * `always #N sig = ~sig;` clock generators (`#N` = N ns half period)
//! * `#N;` delays (`#1` = 1 ns), `@(posedge sig)` / `@(negedge sig)`
//! * blocking assignments, bit-selects (`s[i]`, `s[msb:lsb]`)
//! * `if (…) … else …`, `repeat (n) …`, `begin … end`
//! * `assert (expr) else $error("…");`, `$display`, `$write`, `$error`,
//!   `$fatal`, `$warning` with `%d %0d %b %h %t %%` and `$time`
//! * `$finish`
//!
//! Anything else is a parse error naming the file, line, and construct.
//! The DUT must be a single synthesizable `.sv`/`.v` file in the same
//! directory (its `module <name>` header is matched against the instance).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

/// `#1` in a testbench is one nanosecond.
pub const TB_PS_PER_UNIT: u64 = 1000;
/// Watchdog: abort a runaway testbench past this many time units.
pub const TB_DEFAULT_MAX_UNITS: u64 = 10_000_000;
/// Kept run-history entries per project.
pub const TB_HISTORY_KEEP: usize = 20;

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// One discovered testbench file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TbFile {
    pub path: PathBuf,
    /// File stem (`counter_tb`), used as the run name.
    pub name: String,
}

/// Per-testbench run status badge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TbStatus {
    Idle,
    Running,
    Passed,
    Failed,
}

impl TbStatus {
    pub fn label(self) -> &'static str {
        match self {
            TbStatus::Idle => "Not run",
            TbStatus::Running => "Running",
            TbStatus::Passed => "Passed",
            TbStatus::Failed => "Failed",
        }
    }
}

/// True for `*_tb.sv`, `tb_*.sv` (also `.v`), case-insensitive.
pub fn is_testbench_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "sv" && ext != "v" {
        return false;
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    stem.ends_with("_tb") || stem.starts_with("tb_")
}

/// Non-recursive discovery of testbench files in `dir`.
pub fn discover_testbenches(dir: &Path) -> Vec<TbFile> {
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_file() && is_testbench_file(&p) {
            out.push(TbFile {
                name: p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("tb")
                    .to_string(),
                path: p,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
    out
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Parse / elaboration / run error. Always carries the testbench line when
/// the problem is in the testbench source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TbError {
    pub file: String,
    pub line: usize,
    pub msg: String,
}

impl TbError {
    pub fn new(file: &str, line: usize, msg: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            line,
            msg: msg.into(),
        }
    }
}

impl std::fmt::Display for TbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 {
            write!(f, "{}:{}: {}", self.file, self.line, self.msg)
        } else {
            write!(f, "{}: {}", self.file, self.msg)
        }
    }
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Num(u64, u32), // value, width (0 = unsized)
    Str(String),
    Hash,     // #
    At,       // @
    Semi,     // ;
    Comma,    // ,
    Dot,      // .
    Colon,    // :
    LParen,   // (
    RParen,   // )
    LBrack,   // [
    RBrack,   // ]
    Eq,       // =
    EqEq,     // ==
    EqEqEq,   // ===
    BangEq,   // !=
    BangEqEq, // !==
    Bang,     // !
    Tilde,    // ~
    Amp,      // &
    AmpAmp,   // &&
    Pipe,     // |
    PipePipe, // ||
    Caret,    // ^
    Lt,       // <
    Gt,       // >
    LtEq,     // <=
    GtEq,     // >=
    LtLt,     // <<
    GtGt,     // >>
    Plus,     // +
    Minus,    // -
}

#[derive(Clone, Debug)]
struct Spanned {
    tok: Tok,
    line: usize,
}

fn is_kw(s: &str) -> bool {
    matches!(
        s,
        "module"
            | "endmodule"
            | "logic"
            | "reg"
            | "wire"
            | "input"
            | "output"
            | "inout"
            | "initial"
            | "always"
            | "begin"
            | "end"
            | "if"
            | "else"
            | "repeat"
            | "assert"
            | "posedge"
            | "negedge"
    )
}

fn lex(file: &str, src: &str) -> Result<Vec<Spanned>, TbError> {
    let mut toks = Vec::new();
    let mut line = 1usize;
    let b = src.as_bytes();
    let mut i = 0;
    let err = |line: usize, msg: String| TbError::new(file, line, msg);
    while i < b.len() {
        let c = b[i] as char;
        match c {
            '\n' => {
                line += 1;
                i += 1;
            }
            c if c.is_whitespace() => i += 1,
            '/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            '/' if i + 1 < b.len() && b[i + 1] == b'*' => {
                i += 2;
                loop {
                    if i + 1 >= b.len() {
                        return Err(err(line, "unterminated /* comment".into()));
                    }
                    if b[i] == b'\n' {
                        line += 1;
                    }
                    if b[i] == b'*' && b[i + 1] == b'/' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            '"' => {
                i += 1;
                let mut s = String::new();
                loop {
                    if i >= b.len() {
                        return Err(err(line, "unterminated string".into()));
                    }
                    let ch = b[i] as char;
                    if ch == '\n' {
                        return Err(err(line, "newline in string".into()));
                    }
                    if ch == '"' {
                        i += 1;
                        break;
                    }
                    if ch == '\\' && i + 1 < b.len() {
                        i += 1;
                        match b[i] as char {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            '"' => s.push('"'),
                            '\\' => s.push('\\'),
                            '%' => s.push('%'),
                            other => {
                                s.push('\\');
                                s.push(other);
                            }
                        }
                        i += 1;
                    } else {
                        s.push(ch);
                        i += 1;
                    }
                }
                toks.push(Spanned {
                    tok: Tok::Str(s),
                    line,
                });
            }
            '$' => {
                i += 1;
                let start = i;
                while i < b.len() && ((b[i] as char).is_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                toks.push(Spanned {
                    tok: Tok::Ident(format!("${}", &src[start..i])),
                    line,
                });
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < b.len() && ((b[i] as char).is_alphanumeric() || b[i] == b'_' || b[i] == b'$')
                {
                    i += 1;
                }
                let word = src[start..i].to_string();
                // sized number like 4'hff attached to a leading width
                if i < b.len() && b[i] == b'\'' {
                    let width: u32 = word.parse().map_err(|_| {
                        err(line, format!("bad number width `{word}`"))
                    })?;
                    i += 1; // '
                    if i >= b.len() {
                        return Err(err(line, "truncated sized number".into()));
                    }
                    let base = (b[i] as char).to_ascii_lowercase();
                    i += 1;
                    let start_d = i;
                    while i < b.len()
                        && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'?')
                    {
                        i += 1;
                    }
                    let digits: String = src[start_d..i]
                        .chars()
                        .filter(|&ch| ch != '_')
                        .collect();
                    let radix = match base {
                        'b' => 2,
                        'o' => 8,
                        'd' => 10,
                        'h' => 16,
                        _ => return Err(err(line, format!("bad number base '{base}'"))),
                    };
                    let mut val = 0u64;
                    for ch in digits.chars() {
                        // x/z/? bits read as 0 in the runner.
                        let d = match ch {
                            'x' | 'X' | 'z' | 'Z' | '?' => 0,
                            _ => ch.to_digit(radix).ok_or_else(|| {
                                err(line, format!("bad digit `{ch}` for base {radix}"))
                            })? as u64,
                        };
                        val = (val * radix as u64 + d) & u64::MAX;
                    }
                    let mask = if width >= 64 { u64::MAX } else { (1u64 << width) - 1 };
                    toks.push(Spanned {
                        tok: Tok::Num(val & mask, width),
                        line,
                    });
                } else {
                    toks.push(Spanned {
                        tok: Tok::Ident(word),
                        line,
                    });
                }
            }
            c if c.is_ascii_digit() => {
                let start = i;
                while i < b.len() && (b[i] as char).is_ascii_digit() {
                    i += 1;
                }
                let word = src[start..i].to_string();
                if i < b.len() && b[i] == b'\'' {
                    // sized number, e.g. 8'hff
                    let width: u32 =
                        word.parse().map_err(|_| err(line, format!("bad width `{word}`")))?;
                    i += 1;
                    if i >= b.len() {
                        return Err(err(line, "truncated sized number".into()));
                    }
                    let base = (b[i] as char).to_ascii_lowercase();
                    i += 1;
                    let start_d = i;
                    while i < b.len()
                        && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'?')
                    {
                        i += 1;
                    }
                    let digits: String =
                        src[start_d..i].chars().filter(|&ch| ch != '_').collect();
                    let radix = match base {
                        'b' => 2,
                        'o' => 8,
                        'd' => 10,
                        'h' => 16,
                        _ => return Err(err(line, format!("bad number base '{base}'"))),
                    };
                    let mut val = 0u64;
                    for ch in digits.chars() {
                        let d = match ch {
                            'x' | 'X' | 'z' | 'Z' | '?' => 0,
                            _ => ch.to_digit(radix).ok_or_else(|| {
                                err(line, format!("bad digit `{ch}` for base {radix}"))
                            })? as u64,
                        };
                        val = val.wrapping_mul(radix as u64).wrapping_add(d);
                    }
                    let mask = if width >= 64 {
                        u64::MAX
                    } else if width == 0 {
                        u64::MAX
                    } else {
                        (1u64 << width) - 1
                    };
                    toks.push(Spanned {
                        tok: Tok::Num(val & mask, width),
                        line,
                    });
                } else {
                    let val: u64 = word.parse().map_err(|_| err(line, "bad number".into()))?;
                    toks.push(Spanned {
                        tok: Tok::Num(val, 0),
                        line,
                    });
                }
            }
            '\'' => {
                // unsized base literal: 'hff
                i += 1;
                if i >= b.len() {
                    return Err(err(line, "truncated `' literal".into()));
                }
                let base = (b[i] as char).to_ascii_lowercase();
                i += 1;
                let start_d = i;
                while i < b.len()
                    && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'?')
                {
                    i += 1;
                }
                let digits: String = src[start_d..i].chars().filter(|&ch| ch != '_').collect();
                let radix = match base {
                    'b' => 2,
                    'o' => 8,
                    'd' => 10,
                    'h' => 16,
                    _ => return Err(err(line, format!("bad `' base '{base}'"))),
                };
                let mut val = 0u64;
                for ch in digits.chars() {
                    let d = match ch {
                        'x' | 'X' | 'z' | 'Z' | '?' => 0,
                        _ => ch
                            .to_digit(radix)
                            .ok_or_else(|| err(line, format!("bad digit `{ch}`")))?
                            as u64,
                    };
                    val = val.wrapping_mul(radix as u64).wrapping_add(d);
                }
                toks.push(Spanned {
                    tok: Tok::Num(val, 32),
                    line,
                });
            }
            '#' => {
                toks.push(Spanned { tok: Tok::Hash, line });
                i += 1;
            }
            '@' => {
                toks.push(Spanned { tok: Tok::At, line });
                i += 1;
            }
            ';' => {
                toks.push(Spanned { tok: Tok::Semi, line });
                i += 1;
            }
            ',' => {
                toks.push(Spanned { tok: Tok::Comma, line });
                i += 1;
            }
            '.' => {
                toks.push(Spanned { tok: Tok::Dot, line });
                i += 1;
            }
            ':' => {
                toks.push(Spanned { tok: Tok::Colon, line });
                i += 1;
            }
            '(' => {
                toks.push(Spanned { tok: Tok::LParen, line });
                i += 1;
            }
            ')' => {
                toks.push(Spanned { tok: Tok::RParen, line });
                i += 1;
            }
            '[' => {
                toks.push(Spanned { tok: Tok::LBrack, line });
                i += 1;
            }
            ']' => {
                toks.push(Spanned { tok: Tok::RBrack, line });
                i += 1;
            }
            '=' => {
                if i + 2 < b.len() && b[i + 1] == b'=' && b[i + 2] == b'=' {
                    toks.push(Spanned { tok: Tok::EqEqEq, line });
                    i += 3;
                } else if i + 1 < b.len() && b[i + 1] == b'=' {
                    toks.push(Spanned { tok: Tok::EqEq, line });
                    i += 2;
                } else {
                    toks.push(Spanned { tok: Tok::Eq, line });
                    i += 1;
                }
            }
            '!' => {
                if i + 2 < b.len() && b[i + 1] == b'=' && b[i + 2] == b'=' {
                    toks.push(Spanned { tok: Tok::BangEqEq, line });
                    i += 3;
                } else if i + 1 < b.len() && b[i + 1] == b'=' {
                    toks.push(Spanned { tok: Tok::BangEq, line });
                    i += 2;
                } else {
                    toks.push(Spanned { tok: Tok::Bang, line });
                    i += 1;
                }
            }
            '~' => {
                toks.push(Spanned { tok: Tok::Tilde, line });
                i += 1;
            }
            '&' => {
                if i + 1 < b.len() && b[i + 1] == b'&' {
                    toks.push(Spanned { tok: Tok::AmpAmp, line });
                    i += 2;
                } else {
                    toks.push(Spanned { tok: Tok::Amp, line });
                    i += 1;
                }
            }
            '|' => {
                if i + 1 < b.len() && b[i + 1] == b'|' {
                    toks.push(Spanned { tok: Tok::PipePipe, line });
                    i += 2;
                } else {
                    toks.push(Spanned { tok: Tok::Pipe, line });
                    i += 1;
                }
            }
            '^' => {
                toks.push(Spanned { tok: Tok::Caret, line });
                i += 1;
            }
            '<' => {
                if i + 1 < b.len() && b[i + 1] == b'=' {
                    toks.push(Spanned { tok: Tok::LtEq, line });
                    i += 2;
                } else if i + 1 < b.len() && b[i + 1] == b'<' {
                    toks.push(Spanned { tok: Tok::LtLt, line });
                    i += 2;
                } else {
                    toks.push(Spanned { tok: Tok::Lt, line });
                    i += 1;
                }
            }
            '>' => {
                if i + 1 < b.len() && b[i + 1] == b'=' {
                    toks.push(Spanned { tok: Tok::GtEq, line });
                    i += 2;
                } else if i + 1 < b.len() && b[i + 1] == b'>' {
                    toks.push(Spanned { tok: Tok::GtGt, line });
                    i += 2;
                } else {
                    toks.push(Spanned { tok: Tok::Gt, line });
                    i += 1;
                }
            }
            '+' => {
                toks.push(Spanned { tok: Tok::Plus, line });
                i += 1;
            }
            '-' => {
                toks.push(Spanned { tok: Tok::Minus, line });
                i += 1;
            }
            other => return Err(err(line, format!("unexpected character `{other}`"))),
        }
    }
    Ok(toks)
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Register/wire declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TbDecl {
    pub name: String,
    pub width: u32,
    pub init: Option<Expr>,
    pub line: usize,
}

/// `always #N sig = ~sig;` clock generator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockGen {
    pub sig: String,
    pub half_period_units: u64,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Sig(String),
    Bit(String, Expr),
    Slice(String, Expr, Expr), // [msb:lsb]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Pos,
    Neg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Not,    // ~
    LogNot, // !
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,    // &
    Or,     // |
    Xor,    // ^
    LogAnd, // &&
    LogOr,  // ||
    Shl,
    Shr,
    Add,
    Sub,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Num(u64, u32),
    Sig(String),
    Bit(String, Box<Expr>),
    Slice(String, Box<Expr>, Box<Expr>),
    Un(UnOp, Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Time,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SysTask {
    Display,
    Write,
    Error,
    Fatal,
    Warning,
}

impl SysTask {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "$display" => Some(SysTask::Display),
            "$write" => Some(SysTask::Write),
            "$error" => Some(SysTask::Error),
            "$fatal" => Some(SysTask::Fatal),
            "$warning" => Some(SysTask::Warning),
            _ => None,
        }
    }

    pub fn is_failure(self) -> bool {
        matches!(self, SysTask::Error | SysTask::Fatal)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stmt {
    Assign { target: Target, expr: Expr, line: usize },
    Delay { units: u64, line: usize },
    WaitEdge { edge: Edge, sig: String, line: usize },
    Sys { task: SysTask, fmt: String, args: Vec<Expr>, line: usize },
    Assert { cond: Expr, task: SysTask, fmt: String, args: Vec<Expr>, line: usize },
    If { cond: Expr, then_b: Vec<Stmt>, else_b: Vec<Stmt>, line: usize },
    Repeat { count: Expr, body: Vec<Stmt>, line: usize },
    Finish { line: usize },
}

/// Parsed testbench program.
#[derive(Clone, Debug)]
pub struct TbProgram {
    pub module: String,
    pub dut_module: String,
    pub dut_instance: String,
    /// (dut port, tb signal)
    pub port_map: Vec<(String, String)>,
    pub decls: Vec<TbDecl>,
    pub clocks: Vec<ClockGen>,
    /// Concatenated `initial` statements in source order.
    pub initial: Vec<Stmt>,
}

struct Parser<'a> {
    file: &'a str,
    toks: Vec<Spanned>,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(file: &'a str, toks: Vec<Spanned>) -> Self {
        Self { file, toks, pos: 0 }
    }

    fn err(&self, msg: impl Into<String>) -> TbError {
        let line = self.toks.get(self.pos).map(|t| t.line).unwrap_or(0);
        TbError::new(self.file, line, msg)
    }

    fn err_at(&self, idx: usize, msg: impl Into<String>) -> TbError {
        let line = self.toks.get(idx).map(|t| t.line).unwrap_or(0);
        TbError::new(self.file, line, msg)
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|t| &t.tok)
    }

    fn peek_line(&self) -> usize {
        self.toks.get(self.pos).map(|t| t.line).unwrap_or(0)
    }

    fn next(&mut self) -> Option<Spanned> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, want: &Tok) -> Result<Spanned, TbError> {
        match self.next() {
            Some(t) if &t.tok == want => Ok(t),
            Some(t) => Err(self.err_at(
                self.pos - 1,
                format!("expected {want:?}, found {:?}", t.tok),
            )),
            None => Err(self.err(format!("expected {want:?}, found end of file"))),
        }
    }

    fn eat_ident(&mut self) -> Result<(String, usize), TbError> {
        match self.next() {
            Some(t) => match t.tok {
                Tok::Ident(s) => Ok((s, t.line)),
                other => Err(self.err_at(self.pos - 1, format!("expected identifier, found {other:?}"))),
            },
            None => Err(self.err("expected identifier, found end of file")),
        }
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        match self.peek() {
            Some(Tok::Ident(s)) if s == kw => {
                self.pos += 1;
                true
            }
            _ => false,
        }
    }

    fn peek_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if s == kw)
    }

    fn at_end(&self) -> bool {
        self.pos >= self.toks.len()
    }

    fn parse(mut self) -> Result<TbProgram, TbError> {
        if !self.eat_kw("module") {
            return Err(self.err("testbench must start with `module <name>`"));
        }
        let (module, _) = self.eat_ident()?;
        // Tolerate an empty port list `module tb();`
        if self.peek() == Some(&Tok::LParen) {
            self.pos += 1;
            self.expect(&Tok::RParen)?;
        }
        self.expect(&Tok::Semi)?;
        let mut prog = TbProgram {
            module,
            dut_module: String::new(),
            dut_instance: String::new(),
            port_map: Vec::new(),
            decls: Vec::new(),
            clocks: Vec::new(),
            initial: Vec::new(),
        };
        let mut saw_instance = false;
        // Module-level `assign`s elaborate once at time 0, before initials.
        let mut assigns: Vec<Stmt> = Vec::new();
        while !self.at_end() {
            if self.eat_kw("endmodule") {
                break;
            } else if self.peek_kw("logic") || self.peek_kw("reg") || self.peek_kw("wire") {
                self.parse_decl(&mut prog)?;
            } else if self.peek_kw("input") || self.peek_kw("output") || self.peek_kw("inout") {
                // Tolerated: testbench port directions are ignored.
                self.pos += 1;
                self.parse_decl(&mut prog)?;
            } else if self.peek_kw("initial") {
                self.pos += 1;
                let stmts = self.parse_stmt()?;
                // `@(posedge clk);` as the whole body leaves its `;`.
                if self.peek() == Some(&Tok::Semi) {
                    self.pos += 1;
                }
                prog.initial.push(stmts);
            } else if self.peek_kw("always") {
                self.pos += 1;
                prog.clocks.push(self.parse_clock_gen()?);
            } else if self.peek_kw("assign") {
                // `assign s = expr;` elaborates once at time 0.
                let line = self.peek_line();
                self.pos += 1;
                let target = self.parse_target()?;
                self.expect(&Tok::Eq)?;
                let expr = self.parse_expr()?;
                self.expect(&Tok::Semi)?;
                assigns.push(Stmt::Assign { target, expr, line });
            } else if matches!(self.peek(), Some(Tok::Ident(_))) {
                if saw_instance {
                    return Err(self.err(
                        "only one DUT instantiation is supported (first one wins); \
                         remove the extra instance",
                    ));
                }
                let (dut_module, _) = self.eat_ident()?;
                let (dut_instance, _) = self.eat_ident()?;
                self.expect(&Tok::LParen)?;
                let mut port_map = Vec::new();
                loop {
                    if !matches!(self.peek(), Some(Tok::Dot)) {
                        return Err(self.err(
                            "port connections must be named `.port(sig)`; \
                             positional `(sig)` is not supported",
                        ));
                    }
                    self.expect(&Tok::Dot)?;
                    let (port, _) = self.eat_ident()?;
                    self.expect(&Tok::LParen)?;
                    let (sig, _) = self.eat_ident()?;
                    self.expect(&Tok::RParen)?;
                    port_map.push((port, sig));
                    match self.peek() {
                        Some(Tok::Comma) => {
                            self.pos += 1;
                        }
                        Some(Tok::RParen) => {
                            self.pos += 1;
                            break;
                        }
                        _ => {
                            return Err(self.err(
                                "expected `,` or `)` in port map (named connections \
                                 `.port(sig)` only)",
                            ));
                        }
                    }
                }
                self.expect(&Tok::Semi)?;
                prog.dut_module = dut_module;
                prog.dut_instance = dut_instance;
                prog.port_map = port_map;
                saw_instance = true;
            } else {
                return Err(self.err(
                    "expected declaration, DUT instantiation, initial/always block, \
                     or endmodule (unsupported construct?)",
                ));
            }
        }
        if prog.dut_module.is_empty() {
            return Err(TbError::new(
                self.file,
                0,
                "no DUT instantiation found: add `<dut_module> <inst> (.port(sig), …);`",
            ));
        }
        // Time-0 assigns run before every initial block, in source order.
        if !assigns.is_empty() {
            let mut initial = std::mem::take(&mut assigns);
            initial.extend(prog.initial);
            prog.initial = initial;
        }
        Ok(prog)
    }

    /// `[msb:lsb]` → width, or 1.
    fn parse_range(&mut self) -> Result<u32, TbError> {
        if self.peek() != Some(&Tok::LBrack) {
            return Ok(1);
        }
        self.pos += 1;
        let msb = self.parse_const_u64()?;
        self.expect(&Tok::Colon)?;
        let lsb = self.parse_const_u64()?;
        self.expect(&Tok::RBrack)?;
        if msb < lsb {
            return Err(self.err("inverted range [msb:lsb]"));
        }
        let w = msb - lsb + 1;
        if w > 64 {
            return Err(self.err("widths over 64 bits are not supported"));
        }
        Ok(w as u32)
    }

    fn parse_const_u64(&mut self) -> Result<u64, TbError> {
        match self.next() {
            Some(t) => match t.tok {
                Tok::Num(v, _) => Ok(v),
                other => Err(self.err_at(self.pos - 1, format!("expected constant, found {other:?}"))),
            },
            None => Err(self.err("expected constant, found end of file")),
        }
    }

    fn parse_decl(&mut self, prog: &mut TbProgram) -> Result<(), TbError> {
        // [logic|reg|wire] [[msb:lsb]] name [= init] [, name [= init] …] ;
        // A bare `input clk;` (no type keyword) is also accepted.
        let mut width = 1u32;
        if let Some(Tok::Ident(s)) = self.peek() {
            if s == "logic" || s == "reg" || s == "wire" {
                self.pos += 1;
                width = self.parse_range()?;
            }
        }
        let base_line = self.peek_line();
        loop {
            let (name, line) = self.eat_ident()?;
            if is_kw(&name) {
                return Err(self.err_at(self.pos - 1, format!("`{name}` is a keyword")));
            }
            let init = if self.peek() == Some(&Tok::Eq) {
                self.pos += 1;
                Some(self.parse_expr()?)
            } else {
                None
            };
            prog.decls.push(TbDecl {
                name,
                width,
                init,
                line: base_line.max(line),
            });
            match self.peek() {
                Some(Tok::Comma) => {
                    self.pos += 1;
                }
                Some(Tok::Semi) => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(self.err("expected `,` or `;` in declaration")),
            }
        }
        Ok(())
    }

    fn parse_clock_gen(&mut self) -> Result<ClockGen, TbError> {
        let line = self.peek_line();
        if !matches!(self.peek(), Some(Tok::Hash)) {
            return Err(self.err(
                "only `always #N sig = ~sig;` clock generators are supported",
            ));
        }
        self.expect(&Tok::Hash)?;
        let half = self.parse_const_u64()?;
        if half == 0 {
            return Err(self.err("clock half period must be > 0"));
        }
        let (sig, _) = self.eat_ident()?;
        self.expect(&Tok::Eq)?;
        self.expect(&Tok::Tilde)?;
        let (sig2, _) = self.eat_ident()?;
        if sig != sig2 {
            return Err(self.err("clock generator must be `sig = ~sig`"));
        }
        self.expect(&Tok::Semi)?;
        Ok(ClockGen { sig, half_period_units: half, line })
    }

    fn parse_target(&mut self) -> Result<Target, TbError> {
        let (name, _) = self.eat_ident()?;
        if self.peek() == Some(&Tok::LBrack) {
            self.pos += 1;
            let hi = self.parse_expr()?;
            if self.peek() == Some(&Tok::Colon) {
                self.pos += 1;
                let lo = self.parse_expr()?;
                self.expect(&Tok::RBrack)?;
                Ok(Target::Slice(name, hi, lo))
            } else {
                self.expect(&Tok::RBrack)?;
                Ok(Target::Bit(name, hi))
            }
        } else {
            Ok(Target::Sig(name))
        }
    }

    fn parse_stmt(&mut self) -> Result<Stmt, TbError> {
        let line = self.peek_line();
        if self.eat_kw("begin") {
            // `begin : block_name` labels are accepted and ignored.
            if self.peek() == Some(&Tok::Colon) {
                self.pos += 1;
                let _ = self.eat_ident()?;
            }
            let mut stmts = Vec::new();
            while !self.eat_kw("end") {
                if self.at_end() {
                    return Err(self.err("unterminated begin/end"));
                }
                // Tolerate empty statements (`@(posedge clk);` leaves its `;`).
                if self.peek() == Some(&Tok::Semi) {
                    self.pos += 1;
                    continue;
                }
                stmts.push(self.parse_stmt()?);
            }
            // begin/end as a statement: wrap into a synthetic block via Repeat(1)?
            // Simpler: represent as nested stmts through a dedicated path in the
            // interpreter — here we flatten into a Block pseudo-stmt using Repeat.
            return Ok(Stmt::Repeat {
                count: Expr::Num(1, 32),
                body: stmts,
                line,
            });
        }
        if self.peek() == Some(&Tok::Hash) {
            self.pos += 1;
            let units = self.parse_const_u64()?;
            self.expect(&Tok::Semi)?;
            return Ok(Stmt::Delay { units, line });
        }
        if self.peek() == Some(&Tok::At) {
            self.pos += 1;
            self.expect(&Tok::LParen)?;
            let edge = if self.eat_kw("posedge") {
                Edge::Pos
            } else if self.eat_kw("negedge") {
                Edge::Neg
            } else {
                return Err(self.err("expected posedge/negedge after @("));
            };
            let (sig, _) = self.eat_ident()?;
            self.expect(&Tok::RParen)?;
            // `@(...)` is an event control, not a terminated statement: the
            // caller (or a stray-`;` skip) consumes any following `;`.
            return Ok(Stmt::WaitEdge { edge, sig, line });
        }
        if self.eat_kw("if") {
            self.expect(&Tok::LParen)?;
            let cond = self.parse_expr()?;
            self.expect(&Tok::RParen)?;
            let then_b = vec![self.parse_stmt()?];
            // `@(posedge clk); else …` — the event control leaves its `;`.
            if self.peek() == Some(&Tok::Semi) {
                self.pos += 1;
            }
            let else_b = if self.eat_kw("else") {
                vec![self.parse_stmt()?]
            } else {
                Vec::new()
            };
            return Ok(Stmt::If { cond, then_b, else_b, line });
        }
        if self.eat_kw("repeat") {
            self.expect(&Tok::LParen)?;
            let count = self.parse_expr()?;
            self.expect(&Tok::RParen)?;
            let body = vec![self.parse_stmt()?];
            if self.peek() == Some(&Tok::Semi) {
                self.pos += 1;
            }
            return Ok(Stmt::Repeat { count, body, line });
        }
        match self.peek() {
            Some(Tok::Ident(s)) if s == "$finish" => {
                self.pos += 1;
                self.expect(&Tok::Semi)?;
                return Ok(Stmt::Finish { line });
            }
            Some(Tok::Ident(s)) if SysTask::parse(s).is_some() => {
                let (name, _) = self.eat_ident()?;
                let task = SysTask::parse(&name).expect("checked");
                self.expect(&Tok::LParen)?;
                let (fmt, args) = self.parse_sys_args()?;
                self.expect(&Tok::RParen)?;
                self.expect(&Tok::Semi)?;
                return Ok(Stmt::Sys { task, fmt, args, line });
            }
            Some(Tok::Ident(s)) if s == "assert" => {
                self.pos += 1;
                self.expect(&Tok::LParen)?;
                let cond = self.parse_expr()?;
                self.expect(&Tok::RParen)?;
                let (task, fmt, args) = if self.eat_kw("else") {
                    match self.peek() {
                        Some(Tok::Ident(s)) if SysTask::parse(s).is_some() => {
                            let (name, _) = self.eat_ident()?;
                            let task = SysTask::parse(&name).expect("checked");
                            if !task.is_failure() {
                                return Err(self.err(
                                    "assert else requires $error or $fatal",
                                ));
                            }
                            self.expect(&Tok::LParen)?;
                            let (fmt, args) = self.parse_sys_args()?;
                            self.expect(&Tok::RParen)?;
                            // optional `;` after the else branch
                            if self.peek() == Some(&Tok::Semi) {
                                self.pos += 1;
                            }
                            (task, fmt, args)
                        }
                        _ => {
                            return Err(self.err(
                                "assert else requires $error(\"…\") or $fatal(\"…\")",
                            ));
                        }
                    }
                } else {
                    (SysTask::Error, "assertion failed".to_string(), Vec::new())
                };
                return Ok(Stmt::Assert { cond, task, fmt, args, line });
            }
            _ => {}
        }
        // Assignment: `<target> = <expr>;`
        let target = self.parse_target()?;
        self.expect(&Tok::Eq)?;
        let expr = self.parse_expr()?;
        self.expect(&Tok::Semi)?;
        Ok(Stmt::Assign { target, expr, line })
    }

    fn parse_sys_args(&mut self) -> Result<(String, Vec<Expr>), TbError> {
        let fmt = match self.next() {
            Some(t) => match t.tok {
                Tok::Str(s) => s,
                other => {
                    return Err(self.err_at(
                        self.pos - 1,
                        format!("system task needs a format string, found {other:?}"),
                    ));
                }
            },
            None => return Err(self.err("expected format string")),
        };
        let mut args = Vec::new();
        while self.peek() == Some(&Tok::Comma) {
            self.pos += 1;
            args.push(self.parse_expr()?);
        }
        Ok((fmt, args))
    }

    // ---- expressions (precedence climbing) ----

    fn parse_expr(&mut self) -> Result<Expr, TbError> {
        self.parse_log_or()
    }

    fn parse_log_or(&mut self) -> Result<Expr, TbError> {
        let mut e = self.parse_log_and()?;
        while self.peek() == Some(&Tok::PipePipe) {
            self.pos += 1;
            let r = self.parse_log_and()?;
            e = Expr::Bin(BinOp::LogOr, Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn parse_log_and(&mut self) -> Result<Expr, TbError> {
        let mut e = self.parse_bit_or()?;
        while self.peek() == Some(&Tok::AmpAmp) {
            self.pos += 1;
            let r = self.parse_bit_or()?;
            e = Expr::Bin(BinOp::LogAnd, Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn parse_bit_or(&mut self) -> Result<Expr, TbError> {
        let mut e = self.parse_bit_xor()?;
        while self.peek() == Some(&Tok::Pipe) {
            self.pos += 1;
            let r = self.parse_bit_xor()?;
            e = Expr::Bin(BinOp::Or, Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn parse_bit_xor(&mut self) -> Result<Expr, TbError> {
        let mut e = self.parse_bit_and()?;
        while self.peek() == Some(&Tok::Caret) {
            self.pos += 1;
            let r = self.parse_bit_and()?;
            e = Expr::Bin(BinOp::Xor, Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn parse_bit_and(&mut self) -> Result<Expr, TbError> {
        let mut e = self.parse_equality()?;
        while self.peek() == Some(&Tok::Amp) {
            self.pos += 1;
            let r = self.parse_equality()?;
            e = Expr::Bin(BinOp::And, Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn parse_equality(&mut self) -> Result<Expr, TbError> {
        let mut e = self.parse_relational()?;
        loop {
            let op = match self.peek() {
                Some(Tok::EqEq) | Some(Tok::EqEqEq) => BinOp::Eq,
                Some(Tok::BangEq) | Some(Tok::BangEqEq) => BinOp::Ne,
                _ => break,
            };
            self.pos += 1;
            let r = self.parse_relational()?;
            e = Expr::Bin(op, Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn parse_relational(&mut self) -> Result<Expr, TbError> {
        let mut e = self.parse_shift()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Lt) => BinOp::Lt,
                Some(Tok::Gt) => BinOp::Gt,
                Some(Tok::LtEq) => BinOp::Le,
                Some(Tok::GtEq) => BinOp::Ge,
                _ => break,
            };
            self.pos += 1;
            let r = self.parse_shift()?;
            e = Expr::Bin(op, Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn parse_shift(&mut self) -> Result<Expr, TbError> {
        let mut e = self.parse_add()?;
        loop {
            let op = match self.peek() {
                Some(Tok::LtLt) => BinOp::Shl,
                Some(Tok::GtGt) => BinOp::Shr,
                _ => break,
            };
            self.pos += 1;
            let r = self.parse_add()?;
            e = Expr::Bin(op, Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn parse_add(&mut self) -> Result<Expr, TbError> {
        let mut e = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => BinOp::Add,
                Some(Tok::Minus) => BinOp::Sub,
                _ => break,
            };
            self.pos += 1;
            let r = self.parse_unary()?;
            e = Expr::Bin(op, Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn parse_unary(&mut self) -> Result<Expr, TbError> {
        match self.peek() {
            Some(Tok::Tilde) => {
                self.pos += 1;
                Ok(Expr::Un(UnOp::Not, Box::new(self.parse_unary()?)))
            }
            Some(Tok::Bang) => {
                self.pos += 1;
                Ok(Expr::Un(UnOp::LogNot, Box::new(self.parse_unary()?)))
            }
            Some(Tok::Minus) => {
                // unary minus: 0 - e
                self.pos += 1;
                let e = self.parse_unary()?;
                Ok(Expr::Bin(
                    BinOp::Sub,
                    Box::new(Expr::Num(0, 32)),
                    Box::new(e),
                ))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, TbError> {
        match self.next() {
            Some(t) => {
                let line = t.line;
                match t.tok {
                    Tok::Num(v, w) => Ok(Expr::Num(v, if w == 0 { 32 } else { w })),
                    Tok::Ident(s) => {
                        if s == "$time" {
                            return Ok(Expr::Time);
                        }
                        if is_kw(&s) {
                            return Err(self.err_at(
                                self.pos - 1,
                                format!("`{s}` is a keyword, not a signal"),
                            ));
                        }
                        if SysTask::parse(&s).is_some() {
                            return Err(self.err_at(
                                self.pos - 1,
                                format!("`{s}` is a system task, not an expression"),
                            ));
                        }
                        if self.peek() == Some(&Tok::LBrack) {
                            self.pos += 1;
                            let hi = self.parse_expr()?;
                            if self.peek() == Some(&Tok::Colon) {
                                self.pos += 1;
                                let lo = self.parse_expr()?;
                                self.expect(&Tok::RBrack)?;
                                Ok(Expr::Slice(s, Box::new(hi), Box::new(lo)))
                            } else {
                                self.expect(&Tok::RBrack)?;
                                Ok(Expr::Bit(s, Box::new(hi)))
                            }
                        } else {
                            Ok(Expr::Sig(s))
                        }
                    }
                    Tok::LParen => {
                        let e = self.parse_expr()?;
                        self.expect(&Tok::RParen)?;
                        Ok(e)
                    }
                    other => Err(self.err_at(line, format!("expected expression, found {other:?}"))),
                }
            }
            None => Err(self.err("expected expression, found end of file")),
        }
    }
}

/// Parse testbench source text.
pub fn parse_testbench(file: &str, src: &str) -> Result<TbProgram, TbError> {
    let toks = lex(file, src)?;
    Parser::new(file, toks).parse()
}

/// Find the DUT source file: a `.sv`/`.v` in `dir` whose text contains
/// `module <dut>`. Returns the path or a helpful error.
pub fn find_dut_source(dir: &Path, dut: &str) -> Result<PathBuf, TbError> {
    let rd = std::fs::read_dir(dir)
        .map_err(|e| TbError::new("", 0, format!("cannot read {}: {e}", dir.display())))?;
    let mut cands: Vec<PathBuf> = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let ext = p
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext != "sv" && ext != "v" {
            continue;
        }
        if is_testbench_file(&p) {
            continue;
        }
        cands.push(p);
    }
    cands.sort();
    for p in &cands {
        let text = std::fs::read_to_string(p).unwrap_or_default();
        if module_declared(&text, dut) {
            return Ok(p.clone());
        }
    }
    Err(TbError::new(
        "",
        0,
        format!(
            "DUT module `{dut}` not found: put its synthesizable .sv/.v \
             (with `module {dut}`) in {}",
            dir.display()
        ),
    ))
}

/// True when `text` declares `module <name>` outside comments.
fn module_declared(text: &str, name: &str) -> bool {
    let mut in_block = false;
    for line in text.lines() {
        let mut l = line;
        // strip // comments (crude but fine for a header scan)
        if let Some(i) = l.find("//") {
            l = &l[..i];
        }
        if in_block {
            if let Some(i) = l.find("*/") {
                l = &l[i + 2..];
                in_block = false;
            } else {
                continue;
            }
        }
        if let Some(i) = l.find("/*") {
            if l[i..].find("*/").is_none() {
                in_block = true;
            }
            l = &l[..i];
        }
        let t = l.trim_start();
        if t.starts_with("module") {
            let rest = t["module".len()..].trim_start();
            let ident: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                .collect();
            if ident == name {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Interpreter
// ---------------------------------------------------------------------------

/// Log severity for streamed lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TbSev {
    Info,
    Warning,
    Error,
}

impl TbSev {
    pub fn tag(self) -> &'static str {
        match self {
            TbSev::Info => "INFO",
            TbSev::Warning => "WARNING",
            TbSev::Error => "ERROR",
        }
    }
}

/// Live events streamed from the engine thread to the UI.
#[derive(Clone, Debug)]
pub enum TbStreamEvent {
    Log { time_ns: u64, sev: TbSev, text: String },
    Failure {
        file: String,
        line: usize,
        message: String,
        time_ns: u64,
        /// Relevant signals at failure time: `("dut.count", value)`.
        signals: Vec<(String, u64)>,
    },
    Progress { time_ns: u64 },
}

/// One assertion / `$error` / `$fatal` failure (clickable in the IDE).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TbFailure {
    pub file: String,
    pub line: usize,
    pub message: String,
    pub time_ns: u64,
    /// Relevant signals at failure time: `("dut.count", value)`.
    pub signals: Vec<(String, u64)>,
    /// DUT instance name from the testbench (schematic highlight target).
    pub instance: String,
}

/// One buffered log line (also written to `run.log`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TbLogLine {
    pub time_ns: u64,
    pub sev: TbSev,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct TbRunConfig {
    pub tb_path: PathBuf,
    pub vcd: bool,
    pub max_units: u64,
    /// Set by the UI thread to stop an in-flight run (`tb_stop`).
    pub cancel: Option<Arc<AtomicBool>>,
}

impl TbRunConfig {
    pub fn new(tb_path: PathBuf) -> Self {
        Self {
            tb_path,
            vcd: false,
            max_units: TB_DEFAULT_MAX_UNITS,
            cancel: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TbOutcome {
    pub passed: bool,
    /// The run was stopped by the user (`tb_stop`); not a pass or fail.
    pub stopped: bool,
    pub sim_time_ns: u64,
    pub failures: Vec<TbFailure>,
    pub displays: u32,
    pub vcd_path: Option<PathBuf>,
    pub log_path: PathBuf,
    /// Full log lines (also streamed live and written to `run.log`).
    pub log: Vec<TbLogLine>,
    /// One-line `NAME PASSED/FAILED/STOPPED: …` summary.
    pub summary: String,
    /// Wall-clock runtime in milliseconds.
    pub runtime_ms: u64,
    /// DUT module name from the testbench (for schematic highlight).
    pub dut_module: String,
    /// DUT instance name from the testbench (for schematic highlight).
    pub dut_instance: String,
}

#[derive(Clone, Debug)]
enum Frame {
    Stmts { stmts: Vec<Stmt>, pc: usize },
    Repeat { remaining: u64, body: Vec<Stmt> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum InitState {
    Running,
    WaitingTime(u64),
    WaitingEdge(Edge, String, u64 /* level when the wait started */),
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum EvKind {
    ClockToggle(usize),
    WakeInitial,
}

struct Interp<'a> {
    prog: &'a TbProgram,
    file_label: String,
    sim: helion_sim::Sim,
    dut_inputs: std::collections::HashSet<String>,
    dut_outputs: std::collections::HashSet<String>,
    /// tb signal → dut port
    tb_to_dut: HashMap<String, String>,
    regs: HashMap<String, u64>,
    widths: HashMap<String, u32>,
    time_ps: u64,
    clocks: Vec<ClockState>,
    dut_clock_tb: Option<String>,
    last_posedge_ps: u64,
    events: std::collections::BinaryHeap<Ev>,
    seq: u64,
    stack: Vec<Frame>,
    state: InitState,
    failures: Vec<TbFailure>,
    displays: u32,
    stopped: bool,
    max_ps: u64,
    tx: Option<&'a mpsc::Sender<TbStreamEvent>>,
    /// Stop request from the UI thread (`tb_stop`); checked each event.
    cancel: Option<Arc<AtomicBool>>,
    /// The user cancelled this run (not a pass/fail).
    cancel_hit: bool,
    log: Vec<TbLogLine>,
    /// VCD trace signals: (dotted name, width)
    trace_names: Vec<(String, u32)>,
    /// dotted name → trace idx, for per-write VCD sampling.
    trace_idx: HashMap<String, usize>,
    /// (time_ps, trace idx, value, has_x)
    vcd_samples: Vec<(u64, usize, u64, bool)>,
    vcd_last: Vec<Option<(u64, bool)>>,
    posedge_count: u64,
}

#[derive(Clone, Debug)]
struct ClockState {
    sig: String,
    half_ps: u64,
    next_ps: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Ev {
    time_ps: u64,
    seq: u64,
    kind: EvKind,
}

impl Ord for Ev {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // BinaryHeap is a max-heap: reverse for earliest-first.
        other
            .time_ps
            .cmp(&self.time_ps)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

impl PartialOrd for Ev {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn mask(width: u32, v: u64) -> u64 {
    if width >= 64 {
        v
    } else if width == 0 {
        v
    } else {
        v & ((1u64 << width) - 1)
    }
}

impl<'a> Interp<'a> {
    fn tb_err(&self, line: usize, msg: impl Into<String>) -> TbError {
        TbError::new(&self.file_label, line, msg)
    }

    fn emit(&mut self, ev: TbStreamEvent) {
        match &ev {
            TbStreamEvent::Log { time_ns, sev, text } => {
                self.log.push(TbLogLine { time_ns: *time_ns, sev: *sev, text: text.clone() });
            }
            TbStreamEvent::Failure { file, line, message, time_ns, signals } => {
                self.failures.push(TbFailure {
                    file: file.clone(),
                    line: *line,
                    message: message.clone(),
                    time_ns: *time_ns,
                    signals: signals.clone(),
                    instance: self.prog.dut_instance.clone(),
                });
            }
            TbStreamEvent::Progress { .. } => {}
        }
        if let Some(tx) = self.tx {
            let _ = tx.send(ev);
        }
    }

    fn log_info(&mut self, text: String) {
        let t = self.time_ps / TB_PS_PER_UNIT;
        self.emit(TbStreamEvent::Log { time_ns: t, sev: TbSev::Info, text });
    }

    fn log_warn(&mut self, text: String) {
        let t = self.time_ps / TB_PS_PER_UNIT;
        self.emit(TbStreamEvent::Log { time_ns: t, sev: TbSev::Warning, text });
    }

    fn fail(&mut self, line: usize, message: String) {
        let t = self.time_ps / TB_PS_PER_UNIT;
        let text = format!("$error at {}:{}: {message}", self.file_label, line);
        self.emit(TbStreamEvent::Log { time_ns: t, sev: TbSev::Error, text });
        // Snapshot the bound DUT ports so the failure row can show the
        // relevant signal values (max 8, sorted by port name).
        let mut port_sigs: Vec<(&String, &String)> =
            self.tb_to_dut.iter().collect();
        port_sigs.sort_by(|a, b| a.1.cmp(b.1));
        let mut signals: Vec<(String, u64)> = Vec::new();
        for (sig, port) in port_sigs.iter().take(8) {
            match self.resolve(sig) {
                Ok((v, _, _)) => signals.push((format!("dut.{port}"), v)),
                Err(_) => signals.push((format!("dut.{port}"), 0)),
            }
        }
        self.emit(TbStreamEvent::Failure {
            file: self.file_label.clone(),
            line,
            message,
            time_ns: t,
            signals,
        });
    }

    fn time_ns(&self) -> u64 {
        self.time_ps / TB_PS_PER_UNIT
    }

    /// True when the UI thread requested a stop (`tb_stop`).
    fn cancelled(&self) -> bool {
        self.cancel
            .as_deref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
    }

    /// Resolve a testbench signal to (value, width, has_x).
    fn resolve(&self, name: &str) -> Result<(u64, u32, bool), TbError> {
        // DUT outputs are always sampled live from the kernel — a declared
        // TB reg of the same name is just a placeholder.
        if let Some(port) = self.tb_to_dut.get(name) {
            if self.dut_outputs.contains(port) {
                let w = self.widths.get(name).copied().unwrap_or(1);
                return self.sample_dut_port(port, w);
            }
        }
        if let Some(&v) = self.regs.get(name) {
            let w = self.widths.get(name).copied().unwrap_or(1);
            return Ok((v, w, false));
        }
        if let Some(port) = self.tb_to_dut.get(name).cloned() {
            let w = self.widths.get(name).copied().unwrap_or(1);
            return self.sample_dut_port(&port, w);
        }
        Err(self.tb_err(0, format!("unknown signal `{name}`")))
    }

    /// Sample a DUT port, `w` bits LSB-first. X bits read as 0 with has_x set.
    fn sample_dut_port(&self, port: &str, w: u32) -> Result<(u64, u32, bool), TbError> {
        let mut val = 0u64;
        let mut has_x = false;
        for i in 0..w {
            let bit_net = if w == 1 {
                port.to_string()
            } else {
                format!("{port}_{i}")
            };
            match self.sim.sample(&bit_net) {
                Some(b) => {
                    val |= (b & 1) << i;
                }
                None => {
                    has_x = true;
                }
            }
        }
        Ok((val, w, has_x))
    }

    fn eval(&self, e: &Expr) -> Result<(u64, u32, bool), TbError> {
        match e {
            Expr::Num(v, w) => Ok((*v, *w, false)),
            Expr::Time => Ok((self.time_ns(), 32, false)),
            Expr::Sig(n) => self.resolve(n),
            Expr::Bit(n, idx) => {
                let (v, _, x) = self.resolve(n)?;
                let (i, _, _) = self.eval(idx)?;
                if i >= 64 {
                    return Err(self.tb_err(0, format!("bit select [{i}] out of range")));
                }
                Ok(((v >> i) & 1, 1, x))
            }
            Expr::Slice(n, msb_e, lsb_e) => {
                let (v, w, x) = self.resolve(n)?;
                let (msb, _, _) = self.eval(msb_e)?;
                let (lsb, _, _) = self.eval(lsb_e)?;
                if msb < lsb || msb >= w as u64 {
                    return Err(self.tb_err(
                        0,
                        format!("slice [{msb}:{lsb}] out of range for {w}-bit `{n}`"),
                    ));
                }
                let width = (msb - lsb + 1) as u32;
                Ok((mask(width, v >> lsb), width, x))
            }
            Expr::Un(op, inner) => {
                let (v, w, x) = self.eval(inner)?;
                match op {
                    UnOp::Not => Ok((mask(w, !v), w, x)),
                    UnOp::LogNot => Ok((u64::from(v == 0), 1, x)),
                }
            }
            Expr::Bin(op, l, r) => {
                let (lv, lw, lx) = self.eval(l)?;
                let (rv, rw, rx) = self.eval(r)?;
                let x = lx || rx;
                let w = lw.max(rw).max(1);
                let bit = |b: bool| (u64::from(b), 1, x);
                match op {
                    BinOp::Eq => Ok(bit(mask(w, lv) == mask(w, rv))),
                    BinOp::Ne => Ok(bit(mask(w, lv) != mask(w, rv))),
                    BinOp::Lt => Ok(bit(lv < rv)),
                    BinOp::Gt => Ok(bit(lv > rv)),
                    BinOp::Le => Ok(bit(lv <= rv)),
                    BinOp::Ge => Ok(bit(lv >= rv)),
                    BinOp::And => Ok((mask(w, lv & rv), w, x)),
                    BinOp::Or => Ok((mask(w, lv | rv), w, x)),
                    BinOp::Xor => Ok((mask(w, lv ^ rv), w, x)),
                    BinOp::LogAnd => Ok(bit(lv != 0 && rv != 0)),
                    BinOp::LogOr => Ok(bit(lv != 0 || rv != 0)),
                    BinOp::Shl => Ok((mask(w, lv.wrapping_shl(rv.min(63) as u32)), w, x)),
                    BinOp::Shr => Ok((mask(w, lv.wrapping_shr(rv.min(63) as u32)), w, x)),
                    BinOp::Add => Ok((mask(w, lv.wrapping_add(rv)), w, x)),
                    BinOp::Sub => Ok((mask(w, lv.wrapping_sub(rv)), w, x)),
                }
            }
        }
    }

    /// Drive a testbench signal (DUT input port or TB reg) with a value.
    fn drive(&mut self, name: &str, val: u64, line: usize) -> Result<(), TbError> {
        let w = self.widths.get(name).copied().unwrap_or(1);
        let val = mask(w, val);
        if let Some(port) = self.tb_to_dut.get(name).cloned() {
            if self.dut_inputs.contains(&port) {
                for i in 0..w {
                    let bit_net = if w == 1 {
                        port.clone()
                    } else {
                        format!("{port}_{i}")
                    };
                    self.sim
                        .drive_input(&bit_net, (val >> i) & 1 == 1)
                        .map_err(|e| self.tb_err(line, format!("driving `{name}`: {e}")))?;
                }
                // Keep the TB-side reg in sync so reads see the driven value.
                self.regs.insert(name.to_string(), val);
                self.vcd_changed(name, val);
                // Combinational DUTs settle immediately; sample for the VCD.
                self.sample_all();
                return Ok(());
            }
            return Err(self.tb_err(
                line,
                format!("cannot drive `{name}`: bound to DUT output port `{port}`"),
            ));
        }
        if self.regs.contains_key(name) {
            let old = self.regs.get(name).copied().unwrap_or(0) & 1;
            let new = val & 1;
            self.regs.insert(name.to_string(), val);
            self.vcd_changed(name, val);
            // A procedurally-toggled clock still steps the DUT.
            let is_clk = self.dut_clock_tb.as_deref() == Some(name);
            if is_clk && old == 0 && new == 1 {
                self.dut_posedge();
            }
            self.poll_edge_wait(name);
            return Ok(());
        }
        Err(self.tb_err(line, format!("unknown signal `{name}`")))
    }

    /// Execute an assignment target.
    fn exec_assign(&mut self, target: &Target, expr: &Expr, line: usize) -> Result<(), TbError> {
        let (val, _, _) = self.eval(expr)?;
        match target {
            Target::Sig(name) => self.drive(name, val, line),
            Target::Bit(name, idx_e) => {
                if self.tb_to_dut.contains_key(name) {
                    return Err(self.tb_err(
                        line,
                        "bit-select assignment to a DUT-bound signal is not supported; \
                         assign the whole signal",
                    ));
                }
                let (i, _, _) = self.eval(idx_e)?;
                let w = self.widths.get(name.as_str()).copied().unwrap_or(1);
                if i >= w as u64 {
                    return Err(self.tb_err(line, format!("bit [{i}] out of range for {w}-bit `{name}`")));
                }
                let cur = self.regs.get(name.as_str()).copied().unwrap_or(0);
                let nv = if (val & 1) == 1 { cur | (1 << i) } else { cur & !(1 << i) };
                let nv = mask(w, nv);
                self.regs.insert(name.clone(), nv);
                self.vcd_changed(name, nv);
                Ok(())
            }
            Target::Slice(name, msb_e, lsb_e) => {
                if self.tb_to_dut.contains_key(name) {
                    return Err(self.tb_err(
                        line,
                        "part-select assignment to a DUT-bound signal is not supported; \
                         assign the whole signal",
                    ));
                }
                let (msb, _, _) = self.eval(msb_e)?;
                let (lsb, _, _) = self.eval(lsb_e)?;
                let w = self.widths.get(name.as_str()).copied().unwrap_or(1);
                if msb < lsb || msb >= w as u64 {
                    return Err(self.tb_err(line, format!("slice [{msb}:{lsb}] out of range")));
                }
                let width = (msb - lsb + 1) as u32;
                let cur = self.regs.get(name.as_str()).copied().unwrap_or(0);
                let field_mask = mask(width, u64::MAX) << lsb;
                let nv = mask(w, (cur & !field_mask) | ((mask(width, val) << lsb) & field_mask));
                self.regs.insert(name.clone(), nv);
                self.vcd_changed(name, nv);
                Ok(())
            }
        }
    }

    fn format_sys(&self, fmt: &str, args: &[Expr]) -> Result<String, TbError> {
        let mut out = String::new();
        let mut arg_vals: Vec<(u64, u32, bool)> = Vec::new();
        for a in args {
            arg_vals.push(self.eval(a)?);
        }
        let mut ai = 0usize;
        let mut chars = fmt.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            // optional `0` flag (width padding is not supported; flag ignored)
            if chars.peek() == Some(&'0') {
                chars.next();
            }
            let spec = chars.next().unwrap_or('%');
            match spec {
                '%' => out.push('%'),
                'd' | 'D' => {
                    let (v, _, _) = arg_vals.get(ai).copied().unwrap_or((0, 32, false));
                    ai += 1;
                    out.push_str(&format!("{v}"));
                }
                'b' | 'B' => {
                    let (v, _, _) = arg_vals.get(ai).copied().unwrap_or((0, 32, false));
                    ai += 1;
                    out.push_str(&format!("{v:b}"));
                }
                'h' | 'H' => {
                    let (v, _, _) = arg_vals.get(ai).copied().unwrap_or((0, 32, false));
                    ai += 1;
                    out.push_str(&format!("{v:x}"));
                }
                't' | 'T' => {
                    out.push_str(&format!("{}", self.time_ns()));
                }
                other => {
                    out.push('%');
                    out.push(other);
                }
            }
        }
        Ok(out)
    }

    fn exec_sys(&mut self, task: SysTask, fmt: &str, args: &[Expr], line: usize) -> Result<bool, TbError> {
        // Returns true when the run must stop ($fatal).
        let text = self.format_sys(fmt, args)?;
        match task {
            SysTask::Display | SysTask::Write => {
                self.displays += 1;
                self.log_info(text);
            }
            SysTask::Warning => {
                self.log_warn(format!("$warning at {}:{line}: {text}", self.file_label));
            }
            SysTask::Error => {
                self.fail(line, text);
            }
            SysTask::Fatal => {
                self.fail(line, text);
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Execute one statement; may suspend the initial thread.
    fn step_initial(&mut self) -> Result<(), TbError> {
        let stmt = loop {
            let frame = self.stack.last_mut();
            match frame {
                None => {
                    self.state = InitState::Done;
                    return Ok(());
                }
                Some(Frame::Stmts { stmts, pc }) => {
                    if *pc >= stmts.len() {
                        self.stack.pop();
                        continue;
                    }
                    let s = stmts[*pc].clone();
                    *pc += 1;
                    break s;
                }
                Some(Frame::Repeat { remaining, body }) => {
                    if *remaining == 0 {
                        self.stack.pop();
                        continue;
                    }
                    *remaining -= 1;
                    let body = body.clone();
                    self.stack.push(Frame::Stmts { stmts: body, pc: 0 });
                    continue;
                }
            }
        };
        match stmt {
            Stmt::Assign { target, expr, line } => {
                self.exec_assign(&target, &expr, line)?;
            }
            Stmt::Delay { units, .. } => {
                let wake = self.time_ps.saturating_add(units.saturating_mul(TB_PS_PER_UNIT));
                self.state = InitState::WaitingTime(wake);
                self.push_event(wake, EvKind::WakeInitial);
            }
            Stmt::WaitEdge { edge, sig, line } => {
                if !self.regs.contains_key(&sig) && !self.tb_to_dut.contains_key(&sig) {
                    return Err(self.tb_err(line, format!("unknown signal `{sig}`")));
                }
                let (v, _, _) = self.resolve(&sig)?;
                self.state = InitState::WaitingEdge(edge, sig, v & 1);
            }
            Stmt::Sys { task, fmt, args, line } => {
                if self.exec_sys(task, &fmt, &args, line)? {
                    self.stopped = true;
                }
            }
            Stmt::Assert { cond, task, fmt, args, line } => {
                let (v, _, _) = self.eval(&cond)?;
                if v == 0 {
                    if self.exec_sys(task, &fmt, &args, line)? {
                        self.stopped = true;
                    }
                }
            }
            Stmt::If { cond, then_b, else_b, .. } => {
                let (v, _, _) = self.eval(&cond)?;
                let branch = if v != 0 { then_b } else { else_b };
                self.stack.push(Frame::Stmts { stmts: branch, pc: 0 });
            }
            Stmt::Repeat { count, body, line } => {
                let (n, _, _) = self.eval(&count)?;
                if n > 100_000_000 {
                    return Err(self.tb_err(line, "repeat count absurdly large"));
                }
                self.stack.push(Frame::Repeat { remaining: n, body });
            }
            Stmt::Finish { .. } => {
                self.log_info("$finish".to_string());
                self.stopped = true;
            }
        }
        Ok(())
    }

    fn push_event(&mut self, time_ps: u64, kind: EvKind) {
        let seq = self.seq;
        self.seq += 1;
        self.events.push(Ev { time_ps, seq, kind });
    }

    /// Record one VCD sample when a known value changes.
    fn push_vcd_sample(&mut self, idx: usize, val: u64, has_x: bool) {
        let changed = self.vcd_last[idx]
            .map(|(v, x)| v != val || x != has_x)
            .unwrap_or(true);
        if changed {
            self.vcd_last[idx] = Some((val, has_x));
            self.vcd_samples.push((self.time_ps, idx, val, has_x));
        }
    }

    /// A TB-side signal was just written: sample it into the VCD now so
    /// same-timestamp stimulus is not lost before the next event.
    fn vcd_changed(&mut self, tb_name: &str, val: u64) {
        if let Some(&idx) = self.trace_idx.get(&format!("tb.{tb_name}")) {
            self.push_vcd_sample(idx, val, false);
        }
    }

    /// Sample every traced signal; record VCD changes.
    fn sample_all(&mut self) {
        for idx in 0..self.trace_names.len() {
            let (dotted, width) = self.trace_names[idx].clone();
            let (val, has_x) = if let Some(reg) = dotted.strip_prefix("tb.") {
                (self.regs.get(reg).copied().unwrap_or(0), false)
            } else if let Some(port) = dotted.strip_prefix("dut.") {
                match self.sample_dut_port(port, width) {
                    Ok((v, _, x)) => (v, x),
                    Err(_) => (0, true),
                }
            } else {
                (0, true)
            };
            let changed = self.vcd_last[idx].map(|(v, x)| v != val || x != has_x).unwrap_or(true);
            if changed {
                self.vcd_last[idx] = Some((val, has_x));
                self.vcd_samples.push((self.time_ps, idx, val, has_x));
            }
        }
    }

    fn dut_posedge(&mut self) {
        let dt = self.time_ps.saturating_sub(self.last_posedge_ps);
        self.last_posedge_ps = self.time_ps;
        self.sim.step_posedge(dt.max(1));
        self.posedge_count += 1;
        if self.posedge_count % 4096 == 0 {
            let t = self.time_ns();
            self.emit(TbStreamEvent::Progress { time_ns: t });
        }
        self.sample_all();
        // DUT outputs may have moved; re-check @(posedge/negedge) waits.
        self.poll_waiting_edge();
    }

    /// Re-check a `@(posedge/negedge sig)` wait against `sig`'s current level.
    fn poll_edge_wait(&mut self, sig: &str) {
        let (edge, old) = match &self.state {
            InitState::WaitingEdge(e, want, old) if want == sig => (*e, *old),
            _ => return,
        };
        let new = self.resolve(sig).map(|(v, _, _)| v & 1).unwrap_or(old);
        let hit = match edge {
            Edge::Pos => old == 0 && new == 1,
            Edge::Neg => old == 1 && new == 0,
        };
        if hit {
            self.state = InitState::Running;
        } else if new != old {
            // Track the latest level so a slow-moving signal still triggers.
            self.state = InitState::WaitingEdge(edge, sig.to_string(), new);
        }
    }

    /// Re-check whichever signal the initial thread is waiting on, if any.
    fn poll_waiting_edge(&mut self) {
        let want = match &self.state {
            InitState::WaitingEdge(_, want, _) => want.clone(),
            _ => return,
        };
        self.poll_edge_wait(&want);
    }

    fn run(&mut self) -> Result<(), TbError> {
        // Initial samples at t=0 (before any event) for the VCD dumpvars.
        self.sample_all();
        self.push_event(0, EvKind::WakeInitial);
        for i in 0..self.clocks.len() {
            let t = self.clocks[i].next_ps;
            self.push_event(t, EvKind::ClockToggle(i));
        }
        loop {
            if self.stopped {
                break;
            }
            if self.cancelled() {
                // User hit Stop: unwind cleanly; the outcome reports STOPPED.
                self.cancel_hit = true;
                self.stopped = true;
                break;
            }
            let ev = match self.events.pop() {
                Some(e) => e,
                None => {
                    if self.state != InitState::Done {
                        return Err(self.tb_err(
                            0,
                            "deadlock: initial block is waiting but no events remain \
                             (missing clock generator?)",
                        ));
                    }
                    break;
                }
            };
            if ev.time_ps > self.max_ps {
                return Err(self.tb_err(
                    0,
                    format!(
                        "simulation timed out at {} ns (max {} ns); \
                         add $finish or shorten the stimulus",
                        ev.time_ps / TB_PS_PER_UNIT,
                        self.max_ps / TB_PS_PER_UNIT
                    ),
                ));
            }
            self.time_ps = ev.time_ps;
            match ev.kind {
                EvKind::WakeInitial => {
                    if self.state != InitState::Done {
                        self.state = InitState::Running;
                    }
                }
                EvKind::ClockToggle(i) => {
                    let (sig, half) = (self.clocks[i].sig.clone(), self.clocks[i].half_ps);
                    let old = self.regs.get(&sig).copied().unwrap_or(0) & 1;
                    let new = old ^ 1;
                    self.regs.insert(sig.clone(), new);
                    self.vcd_changed(&sig, new);
                    self.clocks[i].next_ps = self.time_ps.saturating_add(half);
                    let nt = self.clocks[i].next_ps;
                    self.push_event(nt, EvKind::ClockToggle(i));
                    let is_dut_clk = self.dut_clock_tb.as_deref() == Some(sig.as_str());
                    if is_dut_clk && old == 0 && new == 1 {
                        self.dut_posedge();
                    } else {
                        self.sample_all();
                    }
                    self.poll_edge_wait(&sig);
                }
            }
            while self.state == InitState::Running && !self.stopped && !self.cancelled() {
                self.step_initial()?;
            }
            // A #delay that already expired, or an edge that already
            // happened, must not wedge the thread: re-check immediately.
            if let InitState::WaitingTime(w) = self.state {
                if w <= self.time_ps {
                    self.state = InitState::Running;
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Top-level run
// ---------------------------------------------------------------------------

/// Run a testbench file end to end: parse → find DUT → synthesize → drive
/// `helion-sim` → stream events → write `run.log` (+ `run.vcd`).
pub fn run_testbench(
    cfg: &TbRunConfig,
    tx: Option<&mpsc::Sender<TbStreamEvent>>,
) -> Result<TbOutcome, TbError> {
    let started = std::time::Instant::now();
    let tb_dir = cfg.tb_path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let file_label = cfg.tb_path.to_string_lossy().into_owned();
    let src = std::fs::read_to_string(&cfg.tb_path)
        .map_err(|e| TbError::new(&file_label, 0, format!("cannot read: {e}")))?;
    let prog = parse_testbench(&file_label, &src)?;

    let dut_path = find_dut_source(&tb_dir, &prog.dut_module)?;
    let dut_label = dut_path.to_string_lossy().into_owned();
    let design = helion_sv::synth_sv_path(&dut_path)
        .map_err(|e| TbError::new(&dut_label, 0, format!("synthesis failed: {e}")))?;
    let sim = helion_sim::Sim::new(&design);

    let dut_inputs: std::collections::HashSet<String> =
        sim.input_ports().into_iter().collect();
    let dut_outputs: std::collections::HashSet<String> =
        sim.output_ports().into_iter().collect();

    // Bind TB signals to DUT ports; unknown DUT ports are a hard error.
    let mut tb_to_dut: HashMap<String, String> = HashMap::new();
    for (port, sig) in &prog.port_map {
        if !dut_inputs.contains(port) && !dut_outputs.contains(port) {
            let mut known: Vec<&str> = dut_inputs.iter().chain(dut_outputs.iter()).map(String::as_str).collect();
            known.sort_unstable();
            return Err(TbError::new(
                &file_label,
                0,
                format!(
                    "DUT `{}` has no port `{port}` (ports: {})",
                    prog.dut_module,
                    known.join(", ")
                ),
            ));
        }
        tb_to_dut.insert(sig.clone(), port.clone());
    }

    // Registers: declared widths, initializers evaluated at time 0.
    let mut regs: HashMap<String, u64> = HashMap::new();
    let mut widths: HashMap<String, u32> = HashMap::new();
    for d in &prog.decls {
        if widths.contains_key(&d.name) {
            return Err(TbError::new(&file_label, d.line, format!("duplicate declaration of `{}`", d.name)));
        }
        widths.insert(d.name.clone(), d.width);
        regs.insert(d.name.clone(), 0);
    }
    // Implicit 1-bit wires for port-map signals that were never declared.
    for sig in tb_to_dut.keys() {
        if !regs.contains_key(sig) {
            regs.insert(sig.clone(), 0);
            widths.insert(sig.clone(), 1);
        }
    }

    // DUT clock: the TB signal bound to a DUT port named clk/clock.
    let dut_clock_tb = prog
        .port_map
        .iter()
        .find(|(p, _)| p.eq_ignore_ascii_case("clk") || p.eq_ignore_ascii_case("clock"))
        .map(|(_, s)| s.clone());

    let mut clocks: Vec<ClockState> = Vec::new();
    for c in &prog.clocks {
        if !regs.contains_key(&c.sig) {
            return Err(TbError::new(
                &file_label,
                c.line,
                format!("clock generator on unknown signal `{}`", c.sig),
            ));
        }
        clocks.push(ClockState {
            sig: c.sig.clone(),
            half_ps: c.half_period_units.saturating_mul(TB_PS_PER_UNIT).max(1),
            next_ps: c.half_period_units.saturating_mul(TB_PS_PER_UNIT).max(1),
        });
    }

    // VCD trace list: every TB reg plus every bound DUT port.
    let mut trace_names: Vec<(String, u32)> = Vec::new();
    let mut seen_trace = std::collections::HashSet::new();
    let mut reg_names: Vec<String> = regs.keys().cloned().collect();
    reg_names.sort();
    for n in reg_names {
        let w = widths.get(&n).copied().unwrap_or(1);
        if seen_trace.insert(format!("tb.{n}")) {
            trace_names.push((format!("tb.{n}"), w));
        }
    }
    let mut bound_ports: Vec<String> = tb_to_dut.values().cloned().collect();
    bound_ports.sort();
    bound_ports.dedup();
    for p in bound_ports {
        let tb_sig = prog.port_map.iter().find(|(pp, _)| pp == &p).map(|(_, s)| s.clone()).unwrap_or_default();
        let w = widths.get(&tb_sig).copied().unwrap_or(1);
        if seen_trace.insert(format!("dut.{p}")) {
            trace_names.push((format!("dut.{p}"), w));
        }
    }
    let vcd_last: Vec<Option<(u64, bool)>> = vec![None; trace_names.len()];

    let mut ip = Interp {
        prog: &prog,
        file_label: file_label.clone(),
        sim,
        dut_inputs,
        dut_outputs,
        tb_to_dut,
        regs,
        widths,
        time_ps: 0,
        clocks,
        dut_clock_tb,
        last_posedge_ps: 0,
        events: std::collections::BinaryHeap::new(),
        seq: 0,
        stack: vec![Frame::Stmts { stmts: prog.initial.clone(), pc: 0 }],
        state: InitState::Running,
        failures: Vec::new(),
        displays: 0,
        stopped: false,
        max_ps: cfg.max_units.saturating_mul(TB_PS_PER_UNIT),
        tx,
        cancel: cfg.cancel.clone(),
        cancel_hit: false,
        log: Vec::new(),
        trace_names,
        trace_idx: HashMap::new(),
        vcd_samples: Vec::new(),
        vcd_last,
        posedge_count: 0,
    };

    // Time-0 initializers (constant expressions only).
    for d in &prog.decls {
        if let Some(init) = &d.init {
            let (v, _, _) = ip.eval(init).map_err(|_| {
                TbError::new(&file_label, d.line, "initializer must be a constant expression")
            })?;
            ip.regs.insert(d.name.clone(), mask(d.width, v));
        }
    }

    ip.log_info(format!(
        "testbench {}: DUT {} <- {}",
        prog.module, prog.dut_module, dut_label
    ));
    // Trace index lookup for per-write VCD sampling.
    ip.trace_idx = ip
        .trace_names
        .iter()
        .enumerate()
        .map(|(i, (n, _))| (n.clone(), i))
        .collect();
    ip.run()?;
    let sim_time_ns = ip.time_ns();
    let failed = !ip.failures.is_empty();
    let failures = ip.failures.clone();
    let displays = ip.displays;
    let log_lines = ip.log.clone();
    let vcd_samples = std::mem::take(&mut ip.vcd_samples);
    let trace_names = ip.trace_names.clone();

    // Artifacts: <tb_dir>/.helion/tb_runs/<stem>_<unix>/{run.log,run.vcd}
    let stem = cfg
        .tb_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("tb");
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let run_dir = tb_dir
        .join(".helion")
        .join("tb_runs")
        .join(format!("{stem}_{unix}"));
    std::fs::create_dir_all(&run_dir)
        .map_err(|e| TbError::new(&file_label, 0, format!("cannot create {}: {e}", run_dir.display())))?;
    let log_path = run_dir.join("run.log");
    {
        let mut text = String::new();
        for l in &log_lines {
            text.push_str(&format!("[{:>10} ns] {:<7} {}\n", l.time_ns, l.sev.tag(), l.text));
        }
        std::fs::write(&log_path, text)
            .map_err(|e| TbError::new(&file_label, 0, format!("cannot write log: {e}")))?;
    }
    let vcd_path = if cfg.vcd {
        let p = run_dir.join("run.vcd");
        write_vcd(&p, &trace_names, &vcd_samples)
            .map_err(|e| TbError::new(&file_label, 0, format!("cannot write VCD: {e}")))?;
        Some(p)
    } else {
        None
    };

    let runtime_ms = started.elapsed().as_millis() as u64;
    let stopped_by_user = ip.cancel_hit;
    let passed = !failed && !stopped_by_user;
    let summary = if stopped_by_user {
        format!("{stem} STOPPED by user at {sim_time_ns} ns (not a pass/fail)")
    } else if passed {
        format!(
            "{stem} PASSED: {displays} displays, sim_time={sim_time_ns} ns, wall={runtime_ms} ms"
        )
    } else {
        format!(
            "{stem} FAILED: {} error(s), {displays} displays, sim_time={sim_time_ns} ns",
            failures.len()
        )
    };
    // The summary goes through the log both for the file and the stream.
    let summary_line = TbLogLine {
        time_ns: sim_time_ns,
        sev: if passed {
            TbSev::Info
        } else if stopped_by_user {
            TbSev::Warning
        } else {
            TbSev::Error
        },
        text: summary.clone(),
    };
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .map_err(|e| TbError::new(&file_label, 0, format!("cannot append log: {e}")))?;
        use std::io::Write as _;
        let _ = writeln!(
            f,
            "[{:>10} ns] {:<7} {}",
            summary_line.time_ns,
            summary_line.sev.tag(),
            summary_line.text
        );
        if let Some(tx) = tx {
            let _ = tx.send(TbStreamEvent::Log {
                time_ns: summary_line.time_ns,
                sev: summary_line.sev,
                text: summary_line.text.clone(),
            });
        }
    }
    let mut full_log = log_lines;
    full_log.push(summary_line);
    Ok(TbOutcome {
        passed,
        stopped: stopped_by_user,
        sim_time_ns,
        failures,
        displays,
        vcd_path,
        log_path,
        log: full_log,
        summary,
        runtime_ms,
        dut_module: prog.dut_module.clone(),
        dut_instance: prog.dut_instance.clone(),
    })
}

// ---------------------------------------------------------------------------
// VCD writer (feeds the VCD-driven power feature later; standard IEEE format)
// ---------------------------------------------------------------------------

fn vcd_id(mut n: usize) -> String {
    // Printable ASCII ids: ! " # … then two-char combos.
    let mut s = String::new();
    loop {
        let c = (33 + (n % 94)) as u8 as char;
        s.push(c);
        n /= 94;
        if n == 0 {
            break;
        }
    }
    s
}

/// Write a standard VCD file: 1 ns timescale, one `testbench` scope.
pub fn write_vcd(
    path: &Path,
    trace_names: &[(String, u32)],
    samples: &[(u64, usize, u64, bool)],
) -> std::io::Result<()> {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "$date");
    let _ = writeln!(out, "   {}", humantime_now());
    let _ = writeln!(out, "$end");
    let _ = writeln!(out, "$version Helion testbench runner $end");
    let _ = writeln!(out, "$timescale 1ns $end");
    let _ = writeln!(out, "$scope module testbench $end");
    let ids: Vec<String> = (0..trace_names.len()).map(vcd_id).collect();
    for (i, (name, width)) in trace_names.iter().enumerate() {
        let safe: String = name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '_' || c == '.' { c } else { '_' })
            .collect();
        let _ = writeln!(out, "$var wire {width} {} {safe} $end", ids[i]);
    }
    let _ = writeln!(out, "$upscope $end");
    let _ = writeln!(out, "$enddefinitions $end");
    let _ = writeln!(out, "#0");
    let _ = writeln!(out, "$dumpvars");
    // Initial values: first sample per signal (t=0 samples are pushed first).
    let mut init: HashMap<usize, (u64, bool)> = HashMap::new();
    for &(_, idx, v, x) in samples {
        init.entry(idx).or_insert((v, x));
    }
    for i in 0..trace_names.len() {
        let (v, x) = init.get(&i).copied().unwrap_or((0, true));
        write_vcd_value(&mut out, &ids[i], trace_names[i].1, v, x);
    }
    let _ = writeln!(out, "$end");
    let mut last_t = 0u64;
    for &(t_ps, idx, v, x) in samples {
        let t = t_ps / TB_PS_PER_UNIT;
        if t != last_t {
            let _ = writeln!(out, "#{t}");
            last_t = t;
        }
        write_vcd_value(&mut out, &ids[idx], trace_names[idx].1, v, x);
    }
    std::fs::write(path, out)
}

fn write_vcd_value(out: &mut String, id: &str, width: u32, v: u64, has_x: bool) {
    use std::fmt::Write as _;
    if has_x {
        let _ = writeln!(out, "x{id}");
        return;
    }
    if width <= 1 {
        let _ = writeln!(out, "{}{id}", v & 1);
    } else {
        let _ = writeln!(out, "b{:0width$b} {id}", mask(width, v), width = width as usize);
    }
}

fn humantime_now() -> String {
    // No chrono/time deps in helion-gui: format SystemTime crudely.
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => format!("{}s since epoch", d.as_secs()),
        Err(_) => "unknown".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Run history (JSON, hand-rolled; <project>/.helion/tb_history.json)
// ---------------------------------------------------------------------------

/// One persisted run record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TbHistoryEntry {
    pub tb_name: String,
    pub tb_path: PathBuf,
    pub started_unix: u64,
    pub passed: bool,
    pub sim_time_ns: u64,
    pub runtime_ms: u64,
    pub errors: u32,
    pub vcd_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct TbHistory {
    pub runs: Vec<TbHistoryEntry>,
    /// Manually added testbench files (outside discovery or re-added).
    pub manual: Vec<PathBuf>,
}

pub fn history_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".helion").join("tb_history.json")
}

fn json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

fn opt_path_json(p: &Option<PathBuf>) -> String {
    match p {
        Some(x) => format!("\"{}\"", json_escape(&x.to_string_lossy())),
        None => "null".to_string(),
    }
}

/// Append a run, keep the last [`TB_HISTORY_KEEP`], and persist.
pub fn record_history(
    project_dir: &Path,
    entry: TbHistoryEntry,
    manual: &[PathBuf],
) -> Result<TbHistory, String> {
    let mut h = load_history(project_dir);
    h.runs.push(entry);
    if h.runs.len() > TB_HISTORY_KEEP {
        let drop = h.runs.len() - TB_HISTORY_KEEP;
        h.runs.drain(..drop);
    }
    h.manual = manual.to_vec();
    save_history(project_dir, &h)?;
    Ok(h)
}

pub fn save_history(project_dir: &Path, h: &TbHistory) -> Result<(), String> {
    use std::fmt::Write as _;
    let mut o = String::from("{\"runs\":[");
    for (i, r) in h.runs.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let _ = write!(
            o,
            "{{\"tb\":\"{}\",\"path\":\"{}\",\"started\":{},\"passed\":{},\
             \"sim_time_ns\":{},\"runtime_ms\":{},\"errors\":{},\"vcd\":{}}}",
            json_escape(&r.tb_name),
            json_escape(&r.tb_path.to_string_lossy()),
            r.started_unix,
            r.passed,
            r.sim_time_ns,
            r.runtime_ms,
            r.errors,
            opt_path_json(&r.vcd_path),
        );
    }
    o.push_str("],\"manual\":[");
    for (i, m) in h.manual.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let _ = write!(o, "\"{}\"", json_escape(&m.to_string_lossy()));
    }
    o.push_str("]}");
    let p = history_path(project_dir);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, o).map_err(|e| e.to_string())
}

/// Minimal JSON reader for exactly what [`save_history`] writes.
pub fn load_history(project_dir: &Path) -> TbHistory {
    let text = std::fs::read_to_string(history_path(project_dir)).unwrap_or_default();
    parse_history_json(&text).unwrap_or_default()
}

fn parse_history_json(text: &str) -> Option<TbHistory> {
    let mut p = JsonParser { b: text.as_bytes(), i: 0 };
    p.parse_history()
}

struct JsonParser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> JsonParser<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && (self.b[self.i] as char).is_whitespace() {
            self.i += 1;
        }
    }
    fn lit(&mut self, s: &str) -> bool {
        if self.b[self.i..].starts_with(s.as_bytes()) {
            self.i += s.len();
            true
        } else {
            false
        }
    }
    fn string(&mut self) -> Option<String> {
        self.ws();
        if self.b.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        let mut o = String::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    match e {
                        b'"' => o.push('"'),
                        b'\\' => o.push('\\'),
                        b'n' => o.push('\n'),
                        b'r' => o.push('\r'),
                        b't' => o.push('\t'),
                        b'u' => {
                            if self.i + 4 > self.b.len() {
                                return None;
                            }
                            let h = std::str::from_utf8(&self.b[self.i..self.i + 4]).ok()?;
                            let cp = u32::from_str_radix(h, 16).ok()?;
                            o.push(char::from_u32(cp)?);
                            self.i += 4;
                        }
                        _ => return None,
                    }
                }
                _ => o.push(c as char),
            }
        }
        Some(o)
    }
    fn number(&mut self) -> Option<u64> {
        self.ws();
        let start = self.i;
        if self.b.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        while self.i < self.b.len() && (self.b[self.i] as char).is_ascii_digit() {
            self.i += 1;
        }
        std::str::from_utf8(&self.b[start..self.i]).ok()?.parse().ok()
    }
    fn bool(&mut self) -> Option<bool> {
        self.ws();
        if self.lit("true") {
            Some(true)
        } else if self.lit("false") {
            Some(false)
        } else {
            None
        }
    }
    fn key(&mut self, name: &str) -> bool {
        self.ws();
        let s = match self.string() {
            Some(s) => s,
            None => return false,
        };
        if s != name {
            return false;
        }
        self.ws();
        if self.b.get(self.i) != Some(&b':') {
            return false;
        }
        self.i += 1;
        true
    }
    fn parse_history(&mut self) -> Option<TbHistory> {
        self.ws();
        if self.b.get(self.i) != Some(&b'{') {
            return None;
        }
        self.i += 1;
        let mut h = TbHistory::default();
        // "runs": [...]
        if !self.key("runs") {
            return None;
        }
        self.ws();
        if self.b.get(self.i) != Some(&b'[') {
            return None;
        }
        self.i += 1;
        loop {
            self.ws();
            if self.b.get(self.i) == Some(&b']') {
                self.i += 1;
                break;
            }
            let e = self.parse_entry()?;
            h.runs.push(e);
            self.ws();
            if self.b.get(self.i) == Some(&b',') {
                self.i += 1;
            }
        }
        self.ws();
        if self.b.get(self.i) == Some(&b',') {
            self.i += 1;
        }
        // "manual": [...]
        if !self.key("manual") {
            return None;
        }
        self.ws();
        if self.b.get(self.i) != Some(&b'[') {
            return None;
        }
        self.i += 1;
        loop {
            self.ws();
            if self.b.get(self.i) == Some(&b']') {
                self.i += 1;
                break;
            }
            h.manual.push(PathBuf::from(self.string()?));
            self.ws();
            if self.b.get(self.i) == Some(&b',') {
                self.i += 1;
            }
        }
        Some(h)
    }
    fn parse_entry(&mut self) -> Option<TbHistoryEntry> {
        self.ws();
        if self.b.get(self.i) != Some(&b'{') {
            return None;
        }
        self.i += 1;
        if !self.key("tb") {
            return None;
        }
        let tb_name = self.string()?;
        if self.b.get(self.i) == Some(&b',') {
            self.i += 1;
        }
        if !self.key("path") {
            return None;
        }
        let path = PathBuf::from(self.string()?);
        if self.b.get(self.i) == Some(&b',') {
            self.i += 1;
        }
        if !self.key("started") {
            return None;
        }
        let started = self.number()?;
        if self.b.get(self.i) == Some(&b',') {
            self.i += 1;
        }
        if !self.key("passed") {
            return None;
        }
        let passed = self.bool()?;
        if self.b.get(self.i) == Some(&b',') {
            self.i += 1;
        }
        if !self.key("sim_time_ns") {
            return None;
        }
        let sim_time_ns = self.number()?;
        if self.b.get(self.i) == Some(&b',') {
            self.i += 1;
        }
        if !self.key("runtime_ms") {
            return None;
        }
        let runtime_ms = self.number()?;
        if self.b.get(self.i) == Some(&b',') {
            self.i += 1;
        }
        if !self.key("errors") {
            return None;
        }
        let errors = self.number()? as u32;
        if self.b.get(self.i) == Some(&b',') {
            self.i += 1;
        }
        if !self.key("vcd") {
            return None;
        }
        self.ws();
        let vcd = if self.lit("null") {
            None
        } else {
            Some(PathBuf::from(self.string()?))
        };
        self.ws();
        if self.b.get(self.i) != Some(&b'}') {
            return None;
        }
        self.i += 1;
        Some(TbHistoryEntry {
            tb_name,
            tb_path: path,
            started_unix: started,
            passed,
            sim_time_ns,
            runtime_ms,
            errors,
            vcd_path: vcd,
        })
    }
}

// ---------------------------------------------------------------------------
// Engine-thread request (surface.rs JobKind::TbRun)
// ---------------------------------------------------------------------------

/// What the UI hands the engine thread for one testbench run.
pub struct TbRunRequest {
    /// Testbench name or index into the model's discovered list.
    pub sel: String,
    pub vcd: bool,
    pub tx: mpsc::Sender<TbStreamEvent>,
    /// Stop flag: the app holds a clone and `tb_stop` sets it; the
    /// engine-thread interpreter polls it.
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Clone for TbRunRequest {
    fn clone(&self) -> Self {
        Self {
            sel: self.sel.clone(),
            vcd: self.vcd,
            tx: self.tx.clone(),
            cancel: self.cancel.clone(),
        }
    }
}

impl std::fmt::Debug for TbRunRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TbRunRequest")
            .field("sel", &self.sel)
            .field("vcd", &self.vcd)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn tmp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("helion_tb_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        p
    }

    const AND_DUT: &str = r#"
module and_dut (
    input  logic clk,
    input  logic a,
    input  logic b,
    output logic y
);
    logic q;
    always_ff @(posedge clk) q <= a & b;
    assign y = q;
endmodule
"#;

    const AND_TB_PASS: &str = r#"
module and_dut_tb;
    logic clk;
    logic a;
    logic b;
    logic y;

    and_dut dut (.clk(clk), .a(a), .b(b), .y(y));

    always #5 clk = ~clk;

    initial begin
        a = 0; b = 0;
        #10;
        a = 1; b = 1;
        @(posedge clk); @(posedge clk);
        $display("y=%0d (t=%0t)", y, $time);
        assert (y == 1) else $error("y should be 1, got %0d", y);
        a = 0;
        @(posedge clk); @(posedge clk);
        assert (y == 0) else $error("y should be 0");
        $display("done");
        $finish;
    end
endmodule
"#;

    #[test]
    fn discovery_matches_tb_suffix_and_prefix() {
        let d = tmp_dir("discover");
        write_file(&d, "counter_tb.sv", "module x; endmodule");
        write_file(&d, "tb_alu.sv", "module x; endmodule");
        write_file(&d, "TB_TOP.V", "module x; endmodule");
        write_file(&d, "counter.sv", "module x; endmodule");
        write_file(&d, "notes.txt", "x");
        let mut names: Vec<String> =
            discover_testbenches(&d).into_iter().map(|t| t.name).collect();
        names.sort();
        assert_eq!(names, vec!["TB_TOP", "counter_tb", "tb_alu"]);
        assert!(is_testbench_file(Path::new("a_tb.sv")));
        assert!(is_testbench_file(Path::new("tb_a.v")));
        assert!(!is_testbench_file(Path::new("a.sv")));
        assert!(!is_testbench_file(Path::new("atb.sv")));
    }

    #[test]
    fn parse_extracts_dut_clocks_and_stmts() {
        let p = parse_testbench("and_dut_tb.sv", AND_TB_PASS).unwrap();
        assert_eq!(p.module, "and_dut_tb");
        assert_eq!(p.dut_module, "and_dut");
        assert_eq!(p.dut_instance, "dut");
        assert_eq!(p.port_map.len(), 4);
        assert!(p.port_map.contains(&("clk".to_string(), "clk".to_string())));
        assert_eq!(p.clocks.len(), 1);
        assert_eq!(p.clocks[0].sig, "clk");
        assert_eq!(p.clocks[0].half_period_units, 5);
        assert_eq!(p.decls.len(), 4);
        // initial block: 1 begin/end → wrapped as Repeat(1)
        assert_eq!(p.initial.len(), 1);
    }

    #[test]
    fn parse_error_reports_file_and_line() {
        let src = "module tb_x;\n  logic a;\n  bogus construct here;\nendmodule\n";
        let e = parse_testbench("tb_x.sv", src).unwrap_err();
        assert_eq!(e.line, 3, "{e}");
        assert!(e.to_string().contains("tb_x.sv:3"), "{e}");
    }

    #[test]
    fn parse_rejects_unnamed_port_map() {
        let src = "module tb_x;\nlogic clk;\nfoo dut (clk);\nendmodule\n";
        let e = parse_testbench("tb_x.sv", src).unwrap_err();
        assert!(e.msg.contains("named"), "{e}");
    }

    #[test]
    fn parse_rejects_unsupported_always_body() {
        let src = "module tb_x;\nlogic clk;\nalways @(posedge clk) clk = 0;\nendmodule\n";
        let e = parse_testbench("tb_x.sv", src).unwrap_err();
        assert!(e.msg.contains("sig = ~sig"), "{e}");
    }

    #[test]
    fn run_passing_testbench_reports_pass_and_log() {
        let d = tmp_dir("pass");
        write_file(&d, "and_dut.sv", AND_DUT);
        let tb = write_file(&d, "and_dut_tb.sv", AND_TB_PASS);
        let (tx, rx) = mpsc::channel();
        let cfg = TbRunConfig { tb_path: tb, vcd: true, max_units: TB_DEFAULT_MAX_UNITS, cancel: None };
        let out = run_testbench(&cfg, Some(&tx)).unwrap();
        assert!(out.passed, "failures: {:?}", out.failures);
        assert!(out.failures.is_empty());
        assert_eq!(out.displays, 2);
        assert!(out.sim_time_ns > 0);
        assert!(out.log_path.exists());
        let log = std::fs::read_to_string(&out.log_path).unwrap();
        assert!(log.contains("y=1"), "{log}");
        assert!(log.contains("done"), "{log}");
        assert!(log.contains("PASSED"), "{log}");
        // VCD written next to the log.
        let vcd_path = out.vcd_path.expect("vcd");
        assert_eq!(vcd_path.parent(), out.log_path.parent());
        let vcd = std::fs::read_to_string(&vcd_path).unwrap();
        assert!(vcd.contains("$timescale 1ns"), "{vcd}");
        assert!(vcd.contains("tb.clk"), "{vcd}");
        assert!(vcd.contains("dut.y"), "{vcd}");
        // Stream delivered log events live.
        let streamed: Vec<_> = rx.try_iter().collect();
        assert!(
            streamed.iter().any(|e| matches!(e, TbStreamEvent::Log { text, .. } if text.contains("y=1"))),
            "live stream must carry $display lines"
        );
    }

    #[test]
    fn run_failing_testbench_reports_file_line() {
        let d = tmp_dir("fail");
        write_file(&d, "and_dut.sv", AND_DUT);
        let tb_src = r#"
module and_dut_tb;
    logic clk; logic a; logic b; logic y;
    and_dut dut (.clk(clk), .a(a), .b(b), .y(y));
    always #5 clk = ~clk;
    initial begin
        a = 0; b = 0;
        #10;
        @(posedge clk); @(posedge clk);
        assert (y == 1) else $error("boom y=%0d", y);
        $finish;
    end
endmodule
"#;
        let tb = write_file(&d, "and_dut_tb.sv", tb_src);
        let cfg = TbRunConfig::new(tb);
        let out = run_testbench(&cfg, None).unwrap();
        assert!(!out.passed);
        assert_eq!(out.failures.len(), 1);
        let f = &out.failures[0];
        // the assert is on line 10 of the source above
        assert_eq!(f.line, 10, "failure: {f:?}");
        assert!(f.message.contains("boom y=0"), "{f:?}");
        assert!(out.vcd_path.is_none(), "vcd off by default");
        // Failure UX: assertion message + sim time + relevant signals + instance.
        assert!(f.time_ns > 0, "{f:?}");
        assert!(!f.signals.is_empty(), "failure must snapshot DUT ports");
        assert_eq!(f.instance, "dut");
        assert!(
            f.signals.iter().any(|(n, v)| n == "dut.y" && *v == 0),
            "signals: {:?}",
            f.signals
        );
        assert!(out.dut_module == "and_dut" && out.dut_instance == "dut");
    }

    #[test]
    fn run_cancel_before_start_reports_stopped() {
        let d = tmp_dir("cancel");
        write_file(&d, "and_dut.sv", AND_DUT);
        let tb = write_file(&d, "and_dut_tb.sv", AND_TB_PASS);
        let cancel = Arc::new(AtomicBool::new(true)); // stop before it starts
        let cfg = TbRunConfig {
            tb_path: tb,
            vcd: false,
            max_units: TB_DEFAULT_MAX_UNITS,
            cancel: Some(cancel),
        };
        let out = run_testbench(&cfg, None).unwrap();
        assert!(out.stopped, "outcome: {:?}", out.summary);
        assert!(!out.passed);
        assert!(out.summary.contains("STOPPED"), "{:?}", out.summary);
        assert!(out.failures.is_empty(), "stop is not a failure");
    }

    #[test]
    fn run_missing_dut_is_a_clear_error() {
        let d = tmp_dir("nodut");
        let tb = write_file(&d, "ghost_tb.sv", "module ghost_tb;\nfoo dut (.a(a));\nendmodule\n");
        let cfg = TbRunConfig::new(tb);
        let e = run_testbench(&cfg, None).unwrap_err();
        assert!(e.msg.contains("DUT module `foo` not found"), "{e}");
    }

    #[test]
    fn run_timeout_is_a_clear_error() {
        let d = tmp_dir("timeout");
        write_file(&d, "and_dut.sv", AND_DUT);
        let tb = write_file(
            &d,
            "and_dut_tb.sv",
            "module and_dut_tb;\nlogic clk;\nand_dut dut (.clk(clk), .a(clk), .b(clk), .y(clk));\nalways #5 clk = ~clk;\ninitial begin\n#100000000;\n$finish;\nend\nendmodule\n",
        );
        let cfg = TbRunConfig { tb_path: tb, vcd: false, max_units: 100, cancel: None };
        let e = run_testbench(&cfg, None).unwrap_err();
        assert!(e.msg.contains("timed out"), "{e}");
    }

    #[test]
    fn vcd_writer_emits_standard_markers() {
        let d = tmp_dir("vcd");
        let p = d.join("t.vcd");
        let names = vec![("tb.clk".to_string(), 1), ("dut.y".to_string(), 4)];
        let samples = vec![
            (0u64, 0usize, 0u64, false),
            (0, 1, 0, false),
            (5_000, 0, 1, false),
            (10_000, 1, 0b1010, false),
        ];
        write_vcd(&p, &names, &samples).unwrap();
        let v = std::fs::read_to_string(&p).unwrap();
        assert!(v.contains("$var wire 1"), "{v}");
        assert!(v.contains("$var wire 4"), "{v}");
        assert!(v.contains("$enddefinitions $end"), "{v}");
        assert!(v.contains("$dumpvars"), "{v}");
        assert!(v.contains("#5"), "{v}");
        assert!(v.contains("b1010"), "{v}");
    }

    #[test]
    fn history_round_trip_and_keep_limit() {
        let d = tmp_dir("hist");
        // record_history is the only writer: the on-disk history is the
        // source of truth, and it trims to TB_HISTORY_KEEP.
        for i in 0..25 {
            let e = TbHistoryEntry {
                tb_name: format!("tb{i}"),
                tb_path: PathBuf::from(format!("/x/tb{i}.sv")),
                started_unix: 1000 + i as u64,
                passed: i % 2 == 0,
                sim_time_ns: i as u64,
                runtime_ms: 5,
                errors: if i % 2 == 0 { 0 } else { 1 },
                vcd_path: None,
            };
            let manual = vec![PathBuf::from("/y/custom_tb.sv")];
            record_history(&d, e, &manual).unwrap();
        }
        let h2 = load_history(&d);
        assert_eq!(h2.runs.len(), TB_HISTORY_KEEP);
        // Kept the newest 20: tb5..tb24.
        assert_eq!(h2.runs[0].tb_name, "tb5");
        assert_eq!(h2.runs.last().unwrap().tb_name, "tb24");
        assert!(h2.runs.last().unwrap().passed);
        assert_eq!(h2.manual, vec![PathBuf::from("/y/custom_tb.sv")]);
        // corrupt file → empty history, never a panic
        std::fs::write(history_path(&d), "{oops").unwrap();
        assert!(load_history(&d).runs.is_empty());
    }

    #[test]
    fn display_formatting_covers_specifiers() {
        // format_sys needs an Interp; drive it through a tiny passing run's
        // machinery instead: parse and check the formatted log line.
        let d = tmp_dir("fmt");
        write_file(&d, "and_dut.sv", AND_DUT);
        let tb = write_file(
            &d,
            "and_dut_tb.sv",
            "module and_dut_tb;\nlogic clk; logic [3:0] nib;\nand_dut dut (.clk(clk), .a(clk), .b(clk), .y(clk));\ninitial begin\nnib = 4'hA;\n$display(\"d=%0d b=%b h=%h t=%0t %%\", nib, nib, nib, $time);\n$finish;\nend\nendmodule\n",
        );
        let cfg = TbRunConfig::new(tb);
        let out = run_testbench(&cfg, None).unwrap();
        assert!(out.passed);
        let log = std::fs::read_to_string(&out.log_path).unwrap();
        assert!(log.contains("d=10 b=1010 h=a t=0 %"), "{log}");
    }
}
