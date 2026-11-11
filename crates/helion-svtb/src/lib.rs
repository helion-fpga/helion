//! Helion class-based SystemVerilog testbench engine.
//!
//! Front end ([`pp`] preprocessor, [`lex`], [`parse`]) and an event-driven
//! interpreter ([`interp`]) for verification code: classes (inheritance,
//! virtual methods, static members, parameterized classes), dynamic arrays,
//! queues, associative arrays, strings, `fork`/`join`, events, mailboxes,
//! semaphores, virtual interfaces with clocking blocks, constrained random
//! (`rand`/`randc`, constraint blocks, `randomize() with`), macros and
//! `` `include``, plus a UVM subset ([`uvm`]). The DUT is synthesized by
//! the caller ([`DutProvider`]) and simulated on the `helion-sim` netlist
//! kernel.
//!
//! Limits (reported as errors, never silently ignored): 2-state values of
//! at most 64 bits, no `struct`/`union`, no covergroups, no `disable`
//! of named blocks, no interface tasks, no DPI.

pub mod ast;
pub mod cov;
pub mod interp;
pub mod lex;
pub mod parse;
pub mod pp;
pub mod uvm;
pub mod value;

use std::path::{Path, PathBuf};
use std::rc::Rc;

pub use interp::{DutProvider, DutReport, Event, Failure, LogLine, RunConfig, RunResult, Sev};

/// A diagnostic with a source location (`line` 0 = whole file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SvError {
    pub file: String,
    pub line: usize,
    pub msg: String,
}

impl SvError {
    pub fn new(file: &str, line: usize, msg: impl Into<String>) -> Self {
        Self { file: file.into(), line, msg: msg.into() }
    }
}

impl std::fmt::Display for SvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 {
            write!(f, "{}:{}: {}", self.file, self.line, self.msg)
        } else if self.file.is_empty() {
            write!(f, "{}", self.msg)
        } else {
            write!(f, "{}: {}", self.file, self.msg)
        }
    }
}

impl std::error::Error for SvError {}

#[derive(Clone, Debug, Default)]
pub struct CompileOptions {
    pub defines: Vec<(String, String)>,
    pub incdirs: Vec<PathBuf>,
}

/// A parsed testbench (all source files plus the UVM library when used).
#[derive(Clone, Debug)]
pub struct Program {
    pub unit: Rc<ast::SourceUnit>,
    pub map: pp::SourceMap,
    pub uses_uvm: bool,
}

/// Does this source need the class-based engine (vs. the simple
/// stimulus-only runner)? True for classes, packages, interfaces, macros,
/// includes, fork/join, queues and UVM.
pub fn wants_engine(src: &str) -> bool {
    let code: String = src
        .lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    let words = ["class", "package", "interface", "import", "fork", "mailbox", "semaphore", "virtual", "program", "event", "string", "task", "function", "foreach", "forever", "while", "always_ff", "typedef"];
    code.contains('`')
        || code.contains("[$]")
        || code.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|w| words.contains(&w))
}

/// Preprocess and parse `files` (the first is the testbench top).
pub fn compile(files: &[PathBuf], opts: &CompileOptions) -> Result<Program, SvError> {
    let mut pp = pp::Preprocessor::new();
    for (n, v) in &opts.defines {
        pp.define(n, v);
    }
    pp.incdirs = opts.incdirs.clone();
    let mut srcs = Vec::new();
    let mut uses_uvm = false;
    for f in files {
        let label = f.to_string_lossy().into_owned();
        let s = std::fs::read_to_string(f).map_err(|e| SvError::new(&label, 0, format!("cannot read: {e}")))?;
        uses_uvm |= s.contains("uvm_pkg") || s.contains("uvm_macros.svh");
        srcs.push((f.clone(), s));
    }
    let mut unit = ast::SourceUnit::default();
    let mut uid = 0u32;
    if uses_uvm {
        let toks = pp.text(PathBuf::from("<helion>/uvm_pkg.sv"), uvm::UVM_PKG)?;
        let map = pp.map.clone();
        parse::parse_tokens(toks, &map, &mut uid, &mut unit)?;
    }
    for (f, s) in &srcs {
        if let Some(dir) = f.parent() {
            if !pp.incdirs.iter().any(|d| d == dir) {
                pp.incdirs.push(dir.to_path_buf());
            }
        }
        let toks = pp.text(f.clone(), s)?;
        let map = pp.map.clone();
        parse::parse_tokens(toks, &map, &mut uid, &mut unit)?;
    }
    Ok(Program { unit: Rc::new(unit), map: pp.map.clone(), uses_uvm })
}

/// Elaborate and run `prog`, streaming events to `sink`.
pub fn run(prog: &Program, provider: &mut dyn DutProvider, cfg: RunConfig, sink: &mut dyn FnMut(Event)) -> Result<RunResult, SvError> {
    let mut eng = interp::Engine::new(prog.unit.clone(), prog.map.clone(), cfg, sink);
    eng.elaborate(provider)?;
    eng.run()?;
    Ok(eng.into_result())
}

/// Testbench modules and the DUT modules they instantiate (modules not
/// defined in the testbench sources).
pub fn dut_modules(prog: &Program) -> Vec<(String, String)> {
    let defined: std::collections::HashSet<&str> = prog.unit.modules.iter().map(|m| m.name.as_str()).collect();
    let mut out = Vec::new();
    for m in &prog.unit.modules {
        for it in &m.items {
            if let ast::ModItem::Inst(i) = it {
                if !defined.contains(i.module.as_str()) {
                    out.push((i.module.clone(), i.name.clone()));
                }
            }
        }
    }
    out
}

/// Names of classes deriving (transitively) from `uvm_test`, sorted.
pub fn uvm_tests(prog: &Program) -> Vec<String> {
    use std::collections::HashMap;
    let mut parent: HashMap<String, String> = HashMap::new();
    let mut is_virtual: HashMap<String, bool> = HashMap::new();
    let mut add = |c: &ast::ClassDecl| {
        if let Some(ast::DataType::Named { path }) = &c.extends {
            if let Some(last) = path.last() {
                parent.insert(c.name.clone(), last.name.clone());
            }
        }
        is_virtual.insert(c.name.clone(), c.is_virtual);
    };
    for p in &prog.unit.packages {
        for it in &p.items {
            if let ast::PkgItem::Class(c) = it {
                add(c);
            }
        }
    }
    for it in &prog.unit.unit {
        if let ast::PkgItem::Class(c) = it {
            add(c);
        }
    }
    for m in &prog.unit.modules {
        for it in &m.items {
            if let ast::ModItem::Class(c) = it {
                add(c);
            }
        }
    }
    let mut out: Vec<String> = parent
        .keys()
        .filter(|c| {
            let mut cur = c.as_str();
            for _ in 0..64 {
                match parent.get(cur) {
                    Some(p) if p == "uvm_test" => return !is_virtual.get(c.as_str()).copied().unwrap_or(false),
                    Some(p) => cur = p,
                    None => return false,
                }
            }
            false
        })
        .cloned()
        .collect();
    out.sort();
    out
}

/// Convenience: source path label for a file id.
pub fn file_label(prog: &Program, id: u32) -> String {
    prog.map.label(id)
}

/// Directory-relative path helper used by callers searching for DUTs.
pub fn parent_dir(p: &Path) -> PathBuf {
    p.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}
