//! SV frontend: preprocess + helion-sv elab → AIG → FlowMap LUT6+FF.
//!
//! Large cores (Ibex, PicoRV32) are ingested via `` `define ``/`ifdef`
//! preprocess. Packages seed known enum defaults; always_ff / assign / generate
//! / || && / concat-LHS map to LUT/FF. Verilog gate primitives are not a Helion
//! product and are not a closed WNS. `$display`/`$write` and `===`/`!==`
//! against X/Z are a sim model, not a LUT or a closed WNS. Missing child
//! modules flatten to empty stubs (missing source, not an unknown construct).

mod preprocess;
pub use preprocess::{expand_includes, preprocess_sv};

use helion_ir::{CellKind, Design, PortDir};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use sv_parser::{parse_sv_str, Define, DefineText};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Const(bool),
    Var(String),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Xor(Box<Expr>, Box<Expr>),
}

/// AIG: node 0 is const0. Edges are (node, inverted).
#[derive(Clone, Debug)]
pub struct Aig {
    pub pis: Vec<String>,
    pub ands: Vec<(Lit, Lit)>,
    pub output: Lit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lit {
    pub node: u32,
    pub inv: bool,
}

impl Lit {
    pub fn c0() -> Self {
        Self { node: 0, inv: false }
    }
    pub fn c1() -> Self {
        Self { node: 0, inv: true }
    }
    pub fn not(self) -> Self {
        Self {
            node: self.node,
            inv: !self.inv,
        }
    }
}

impl Aig {
    pub fn from_expr(e: &Expr) -> Self {
        let mut a = Self {
            pis: Vec::new(),
            ands: Vec::new(),
            output: Lit::c0(),
        };
        // Allocate every PI first so AND node ids (1+n_pis+i) cannot collide
        // with later PIs — that collision folded `q2 ^ (q0 & q1)` to const 0.
        a.collect_pis(e);
        a.output = a.expr(e);
        a
    }

    fn collect_pis(&mut self, e: &Expr) {
        match e {
            Expr::Const(_) => {}
            Expr::Var(s) => {
                let _ = self.pi(s);
            }
            Expr::Not(x) => self.collect_pis(x),
            Expr::And(a, b) | Expr::Or(a, b) | Expr::Xor(a, b) => {
                self.collect_pis(a);
                self.collect_pis(b);
            }
        }
    }

    fn pi(&mut self, name: &str) -> Lit {
        if let Some(i) = self.pis.iter().position(|p| p == name) {
            return Lit {
                node: 1 + i as u32,
                inv: false,
            };
        }
        self.pis.push(name.into());
        Lit {
            node: self.pis.len() as u32,
            inv: false,
        }
    }

    fn and_lit(&mut self, a: Lit, b: Lit) -> Lit {
        if a.node == 0 && !a.inv {
            return Lit::c0();
        }
        if b.node == 0 && !b.inv {
            return Lit::c0();
        }
        if a.node == 0 && a.inv {
            return b;
        }
        if b.node == 0 && b.inv {
            return a;
        }
        self.ands.push((a, b));
        Lit {
            node: 1 + self.pis.len() as u32 + (self.ands.len() as u32 - 1),
            inv: false,
        }
    }

    fn expr(&mut self, e: &Expr) -> Lit {
        match e {
            Expr::Const(false) => Lit::c0(),
            Expr::Const(true) => Lit::c1(),
            Expr::Var(s) => self.pi(s),
            Expr::Not(x) => self.expr(x).not(),
            Expr::And(a, b) => {
                let la = self.expr(a);
                let lb = self.expr(b);
                self.and_lit(la, lb)
            }
            Expr::Or(a, b) => {
                let la = self.expr(a);
                let lb = self.expr(b);
                self.and_lit(la.not(), lb.not()).not()
            }
            Expr::Xor(a, b) => {
                let la = self.expr(a);
                let lb = self.expr(b);
                let t1 = self.and_lit(la, lb.not());
                let t2 = self.and_lit(la.not(), lb);
                self.and_lit(t1.not(), t2.not()).not()
            }
        }
    }

    fn eval_lit(&self, lit: Lit, pi_bits: u64) -> bool {
        let mut memo: HashMap<u32, bool> = HashMap::new();
        self.eval_lit_memo(lit, pi_bits, &mut memo)
    }

    fn eval_lit_memo(&self, lit: Lit, pi_bits: u64, memo: &mut HashMap<u32, bool>) -> bool {
        let v = if lit.node == 0 {
            false
        } else if (lit.node as usize) <= self.pis.len() {
            let i = (lit.node - 1) as u32;
            (pi_bits >> i) & 1 == 1
        } else if let Some(&cached) = memo.get(&lit.node) {
            cached
        } else {
            let ai = (lit.node as usize) - 1 - self.pis.len();
            let (a, b) = self.ands[ai];
            let computed =
                self.eval_lit_memo(a, pi_bits, memo) && self.eval_lit_memo(b, pi_bits, memo);
            memo.insert(lit.node, computed);
            computed
        };
        v ^ lit.inv
    }

    /// FlowMap for a single cone with ≤6 PIs: truth table in LUT6 INIT (I0 = LSB = pis[0]).
    pub fn flowmap_lut6(&self) -> u64 {
        let k = self.pis.len().min(6);
        let width = 1u32 << k.max(1);
        let mut pat = 0u64;
        for addr in 0..width {
            if self.eval_lit(self.output, addr as u64) {
                pat |= 1u64 << addr;
            }
        }
        if k == 0 {
            return if self.eval_lit(self.output, 0) {
                u64::MAX
            } else {
                0
            };
        }
        let mut acc = 0u64;
        let w = width as u64;
        if w >= 64 {
            return pat;
        }
        let mask = (1u64 << w) - 1;
        let mut sh = 0;
        while sh < 64 {
            acc |= (pat & mask) << sh;
            sh += w;
        }
        acc
    }
}

/// UG900 Compilation tab over helion-sv (include + `define), not xvlog.
#[derive(Clone, Debug, Default)]
pub struct SvCompileOpts {
    /// `define NAME[=VALUE]` pairs for sv-parser.
    pub defines: Vec<(String, String)>,
    /// Verilog include directories for sv-parser.
    pub include_paths: Vec<String>,
}

/// UG900 Elaboration snapshot stats from helion-sv (not xelab).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SvElabReport {
    pub top: String,
    pub cells: usize,
    pub luts: usize,
    pub ffs: usize,
}

pub fn parse_sv(source: &str, origin: &str) -> Result<sv_parser::SyntaxTree, String> {
    parse_sv_opts(source, origin, &SvCompileOpts::default())
}

pub fn parse_sv_opts(
    source: &str,
    origin: &str,
    opts: &SvCompileOpts,
) -> Result<sv_parser::SyntaxTree, String> {
    let mut defines = HashMap::new();
    for (k, v) in &opts.defines {
        let text = if v.is_empty() {
            None
        } else {
            Some(DefineText::new(v.clone(), None))
        };
        defines.insert(k.clone(), Some(Define::new(k.clone(), Vec::new(), text)));
    }
    let inc: Vec<String> = if opts.include_paths.is_empty() {
        vec!["".into()]
    } else {
        opts.include_paths.clone()
    };
    parse_sv_str(source, origin, &defines, &inc, false, false)
        .map(|(tree, _)| tree)
        .map_err(|e| format!("sv-parser: {e}"))
}

/// Recursive-descent Boolean subset used after AST extraction.
pub fn parse_expr(s: &str) -> Result<Expr, String> {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let chars: Vec<char> = t.chars().collect();
    let mut i = 0;
    fn parse_or(chars: &[char], i: &mut usize) -> Result<Expr, String> {
        let mut e = parse_xor(chars, i)?;
        while *i < chars.len() && (chars[*i] == '|' && peek_not_pipe(chars, *i)) {
            *i += 1;
            let r = parse_xor(chars, i)?;
            e = Expr::Or(Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn peek_not_pipe(chars: &[char], i: usize) -> bool {
        chars.get(i + 1) != Some(&'|')
    }
    fn parse_xor(chars: &[char], i: &mut usize) -> Result<Expr, String> {
        let mut e = parse_and(chars, i)?;
        while *i < chars.len() && chars[*i] == '^' {
            *i += 1;
            let r = parse_and(chars, i)?;
            e = Expr::Xor(Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn parse_and(chars: &[char], i: &mut usize) -> Result<Expr, String> {
        let mut e = parse_un(chars, i)?;
        while *i < chars.len() && chars[*i] == '&' && chars.get(*i + 1) != Some(&'&') {
            *i += 1;
            let r = parse_un(chars, i)?;
            e = Expr::And(Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn parse_un(chars: &[char], i: &mut usize) -> Result<Expr, String> {
        if *i < chars.len() && chars[*i] == '~' {
            *i += 1;
            return Ok(Expr::Not(Box::new(parse_un(chars, i)?)));
        }
        if *i < chars.len() && chars[*i] == '!' {
            *i += 1;
            return Ok(Expr::Not(Box::new(parse_un(chars, i)?)));
        }
        parse_atom(chars, i)
    }
    fn parse_atom(chars: &[char], i: &mut usize) -> Result<Expr, String> {
        if *i < chars.len() && chars[*i] == '(' {
            *i += 1;
            let e = parse_or(chars, i)?;
            if *i >= chars.len() || chars[*i] != ')' {
                return Err("missing )".into());
            }
            *i += 1;
            return Ok(e);
        }
        if *i + 3 < chars.len() && chars[*i] == '1' && chars[*i + 1] == '\'' {
            let b = chars[*i + 3];
            *i += 4;
            return Ok(Expr::Const(b == '1'));
        }
        if *i < chars.len()
            && (chars[*i] == '0' || chars[*i] == '1')
            && !chars.get(*i + 1).map(|c| c.is_alphanumeric()).unwrap_or(false)
        {
            let c = chars[*i] == '1';
            *i += 1;
            return Ok(Expr::Const(c));
        }
        if *i < chars.len() && (chars[*i].is_ascii_alphabetic() || chars[*i] == '_') {
            let mut n = String::new();
            while *i < chars.len() && (chars[*i].is_ascii_alphanumeric() || chars[*i] == '_') {
                n.push(chars[*i]);
                *i += 1;
            }
            return Ok(Expr::Var(n));
        }
        Err(format!("expr at {i}: {:?}", chars.get(*i..)))
    }
    let e = parse_or(&chars, &mut i)?;
    Ok(e)
}

#[derive(Clone, Debug)]
struct Signal {
    name: String,
    width: usize,
    depth: usize,
    keep: bool,
    mark_debug: bool,
}

#[derive(Clone, Debug)]
enum RExpr {
    Const { val: u128, width: usize, care: u128 }, // care bits: 1 = specified (casez)
    Ident(String),
    Bit(String, usize),
    /// Part-select `sig[hi:lo]` (inclusive). Bit i of the slice is `sig[lo+i]`.
    Range(String, usize, usize),
    /// Indexed part-select `sig[base +: width]` (ascending) or `sig[base -: width]`.
    IndexPart {
        name: String,
        base: Box<RExpr>,
        width: usize,
        ascending: bool,
    },
    /// Unpacked word write `mem[addr] <= data`. Not a comb cone.
    WordAt { addr: Box<RExpr>, data: Box<RExpr> },
    /// Concatenation `{a,b,...}` (left = MSB). Replication `{N{e}}` may fold to Const.
    Concat(Vec<RExpr>),
    /// Logical right shift `a >> sh` (const shift amount in rexpr_to_bit; zero-fill).
    Shr(Box<RExpr>, Box<RExpr>),
    /// Arithmetic right shift `a >>> sh` (const shift; sign-fill from MSB).
    Ashr(Box<RExpr>, Box<RExpr>),
    /// Reduction XOR `^in` (1-bit result).
    RedXor(Box<RExpr>),
    /// Reduction AND `&in` (1-bit). `~&in` is Not(RedAnd(...)).
    RedAnd(Box<RExpr>),
    /// Reduction OR `|in` (1-bit). `~|in` is Not(RedOr(...)).
    RedOr(Box<RExpr>),
    Not(Box<RExpr>),
    And(Box<RExpr>, Box<RExpr>),
    Or(Box<RExpr>, Box<RExpr>),
    Xor(Box<RExpr>, Box<RExpr>),
    Add(Box<RExpr>, Box<RExpr>),
    Sub(Box<RExpr>, Box<RExpr>),
    Mul(Box<RExpr>, Box<RExpr>),
    Mux(Box<RExpr>, Box<RExpr>, Box<RExpr>),
    Eq(Box<RExpr>, Box<RExpr>),
    Ne(Box<RExpr>, Box<RExpr>),
    Lt(Box<RExpr>, Box<RExpr>),
}

#[derive(Clone, Debug)]
struct Inst {
    module: String,
    name: String,
    /// child port → parent net (ident)
    conns: Vec<(String, String)>,
    params: Vec<(String, u128)>,
}

#[derive(Clone, Debug)]
struct Rtl {
    module: String,
    ports: Vec<(String, PortDir, usize)>,
    signals: Vec<Signal>,
    nbas: Vec<(String, Option<usize>, RExpr)>, // lhs name, optional bit, rhs
    assigns: Vec<(String, Option<usize>, RExpr)>,
    insts: Vec<Inst>,
    params: Vec<(String, u128)>,
    toks: Vec<Tok>,
    mem_inits: HashMap<String, BTreeMap<usize, u128>>,
}


thread_local! {
    static CUR_MOD: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

fn set_cur_mod(name: &str) {
    CUR_MOD.with(|c| *c.borrow_mut() = name.to_string());
}

fn cur_mod() -> String {
    CUR_MOD.with(|c| c.borrow().clone())
}

fn note_skip(msg: String) {
    eprintln!("{msg}");
}

thread_local! {
    /// Function names skipped in the current module parse (not re-entered).
    static SKIPPED_FUNCS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Once per module+function for the process. Re-elaboration must not loop the line.
    static FUNC_NOT_CALLED_SEEN: std::cell::RefCell<HashSet<String>> =
        std::cell::RefCell::new(HashSet::new());
    /// One `width_overflow` line per module. String-param hashes used as a
    /// range used to panic on `diff + 1` (old lib.rs:1052).
    static WIDTH_OVERFLOW_SEEN: std::cell::RefCell<HashSet<String>> =
        std::cell::RefCell::new(HashSet::new());
    /// One `wide_literal` line per process. Sized binaries wider than u128
    /// used to panic on `1u128 << bit` (old lib.rs:959).
    static WIDE_LITERAL_SEEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Posedge always writes that still need an Hff on the always edge.
    static SEQ_WRITES: std::cell::RefCell<Vec<(String, String, String)>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// Clock net from the first posedge/negedge of each module.
    static EDGE_CLK: std::cell::RefCell<HashMap<String, String>> =
        std::cell::RefCell::new(HashMap::new());
    /// One `sequential_not_lowered` line per module.
    static SEQ_NOT_LOWERED_SEEN: std::cell::RefCell<HashSet<String>> =
        std::cell::RefCell::new(HashSet::new());
    /// One `variable_index_read` line per module. Write-side Hffs may still time.
    static INDEX_READ_SEEN: std::cell::RefCell<HashSet<String>> =
        std::cell::RefCell::new(HashSet::new());
    /// One `assign_not_lowered` line per module+signal. A split `wire` assign
    /// that did not lower is not a const 0 and is not a closed WNS.
    static ASSIGN_NOT_LOWERED_SEEN: std::cell::RefCell<HashSet<String>> =
        std::cell::RefCell::new(HashSet::new());
    /// One `generate_not_lowered` line per module. A generate/for body that
    /// did not parse is not a LUT and not a closed WNS.
    static GEN_NOT_LOWERED_SEEN: std::cell::RefCell<HashSet<String>> =
        std::cell::RefCell::new(HashSet::new());
    /// One `clock_mux` line per module+signal. A posedge on a mux of two
    /// clocks is not one user clock and is not a closed WNS.
    static CLOCK_MUX_SEEN: std::cell::RefCell<HashSet<String>> =
        std::cell::RefCell::new(HashSet::new());
    /// One `clock_gate` line per module+signal. A posedge on an AND/OR of a
    /// clock and an enable is not one user clock and is not a closed WNS.
    static CLOCK_GATE_SEEN: std::cell::RefCell<HashSet<String>> =
        std::cell::RefCell::new(HashSet::new());
    /// One `inout_enable_not_lowered` line per module+signal. A read-only
    /// inout used as a load enable that never reaches FF D is not a closed WNS.
    static INOUT_ENABLE_SEEN: std::cell::RefCell<HashSet<String>> =
        std::cell::RefCell::new(HashSet::new());
    /// One `word_pipeline_cap` line per module. cycles>4 or not a constant:
    /// do not invent extra word stages, and do not close WNS.
    static WORD_PIPELINE_CAP_SEEN: std::cell::RefCell<HashSet<String>> =
        std::cell::RefCell::new(HashSet::new());
    /// One `flatten_cap` line per module. Post-flatten assign/NBA cones that
    /// stall after `hang_diag flatten` are not bit-blasted and not a closed WNS.
    static FLATTEN_CAP_SEEN: std::cell::RefCell<HashSet<String>> =
        std::cell::RefCell::new(HashSet::new());
    /// One `gate_primitive` line per module. bufif/notif/and/or/buf/not are
    /// not LUTs and not a closed WNS. Not one line per instance.
    static GATE_PRIM_SEEN: std::cell::RefCell<HashSet<String>> =
        std::cell::RefCell::new(HashSet::new());
    /// One `sim_only` line per module. `$display`/`$write` or `===`/`!==`
    /// against X/Z is a simulation model, not a LUT and not a closed WNS.
    static SIM_ONLY_SEEN: std::cell::RefCell<HashSet<String>> =
        std::cell::RefCell::new(HashSet::new());
    /// Negedge-only always writes. Not mixed into the posedge NBA cone.
    /// (module, clk, signal, bit, optional RHS). None RHS cannot lower.
    static NEGEDGE_WRITES: std::cell::RefCell<
        Vec<(String, String, String, Option<usize>, Option<RExpr>)>,
    > = const { std::cell::RefCell::new(Vec::new()) };
    /// One `negedge_not_lowered` line per module+signal.
    static NEGEDGE_NOT_LOWERED_SEEN: std::cell::RefCell<HashSet<String>> =
        std::cell::RefCell::new(HashSet::new());
}

fn skipped_funcs_clear() {
    SKIPPED_FUNCS.with(|s| s.borrow_mut().clear());
}

fn skipped_funcs_push(name: String) {
    SKIPPED_FUNCS.with(|s| s.borrow_mut().push(name));
}

fn skipped_funcs_take() -> Vec<String> {
    SKIPPED_FUNCS.with(|s| std::mem::take(&mut *s.borrow_mut()))
}

/// Emit `function_not_called` once per function. A second entry (flatten
/// re-parse, generate copy) is not another LUT and must not loop.
fn note_function_not_called(module: &str, function: &str) {
    let key = format!("{module}\0{function}");
    let fresh = FUNC_NOT_CALLED_SEEN.with(|s| s.borrow_mut().insert(key));
    if !fresh {
        return;
    }
    note_skip(format!(
        "diagnostic function_not_called module={module} function={function} (function is not called; not a LUT)"
    ));
}

/// `[msb:lsb]` width. A string parameter is hashed above bit 96; the old
/// `as usize` then `max - min + 1` overflowed. Do not invent a bus.
fn range_width(msb: u128, lsb: u128) -> Result<usize, String> {
    let span = msb.abs_diff(lsb);
    let Some(w) = span.checked_add(1) else {
        return Err("width_overflow".into());
    };
    usize::try_from(w).map_err(|_| "width_overflow".to_string())
}

fn note_width_overflow() {
    note_width_overflow_named(&cur_mod());
}

/// `hi - lo + 1` or a concat of those widths used to panic (old lib.rs:6042 /
/// accum.rs). One line per module. Not a LUT, not a closed WNS.
fn note_width_overflow_named(module: &str) {
    let module = if module.is_empty() {
        cur_mod()
    } else {
        module.to_string()
    };
    let key = if module.is_empty() {
        "width_overflow".to_string()
    } else {
        module.clone()
    };
    let fresh = WIDTH_OVERFLOW_SEEN.with(|s| s.borrow_mut().insert(key));
    if !fresh {
        return;
    }
    let module = if module.is_empty() { "?" } else { module.as_str() };
    note_skip(format!(
        "diagnostic width_overflow module={module} (range does not fit; string or overflowing parameter used as width; not a LUT)"
    ));
}

fn width_overflow_for(module: &str) -> bool {
    WIDTH_OVERFLOW_SEEN.with(|s| s.borrow().contains(module))
}

/// Inclusive `[hi:lo]` span. `hi - lo + 1` overflowed when a parameter
/// underflowed (`W-1` with `W=0`, `AW-3` with `AW=2`). Do not invent a bus.
fn range_span(lo: usize, hi: usize) -> Option<usize> {
    hi.checked_sub(lo)?.checked_add(1)
}

fn clear_seq_notes() {
    SEQ_WRITES.with(|s| s.borrow_mut().clear());
    EDGE_CLK.with(|m| m.borrow_mut().clear());
    NEGEDGE_WRITES.with(|s| s.borrow_mut().clear());
}

fn note_seq_write(module: &str, clk: &str, signal: &str) {
    let module = if module.is_empty() { "?" } else { module };
    let clk = if clk.is_empty() { "clk" } else { clk };
    let signal = if signal.is_empty() { "always" } else { signal };
    EDGE_CLK.with(|m| {
        m.borrow_mut()
            .entry(module.to_string())
            .or_insert_with(|| clk.to_string());
    });
    SEQ_WRITES.with(|s| {
        let mut v = s.borrow_mut();
        if !v.iter().any(|(m, _, sig)| m == module && sig == signal) {
            v.push((module.to_string(), clk.to_string(), signal.to_string()));
        }
    });
}

fn take_seq_writes(module: &str) -> Vec<(String, String)> {
    SEQ_WRITES.with(|s| {
        let mut v = s.borrow_mut();
        let mut keep = Vec::new();
        let mut out = Vec::new();
        for (m, clk, sig) in v.drain(..) {
            if m == module {
                out.push((clk, sig));
            } else {
                keep.push((m, clk, sig));
            }
        }
        *v = keep;
        out
    })
}

fn edge_clk_of(module: &str) -> Option<String> {
    EDGE_CLK.with(|m| m.borrow().get(module).cloned())
}

fn note_sequential_not_lowered(module: &str, signal: &str) {
    let fresh = SEQ_NOT_LOWERED_SEEN.with(|s| s.borrow_mut().insert(module.to_string()));
    if !fresh {
        return;
    }
    note_skip(format!(
        "diagnostic sequential_not_lowered module={module} signal={signal} (posedge always not mapped to an Hff; not a closed WNS)"
    ));
}

/// Variable-index read of a clocked unpacked array. One line. Does not walk a cone.
fn note_variable_index_read(module: &str, signal: &str) {
    let fresh = INDEX_READ_SEEN.with(|s| s.borrow_mut().insert(module.to_string()));
    if !fresh {
        return;
    }
    note_skip(format!(
        "diagnostic variable_index_read module={module} signal={signal} (variable-index read not mapped; not a LUT; write-side Hff still timed)"
    ));
}

/// Continuous assign (including `wire` + newline + `name = expr`) did not lower.
/// One line per signal. Not a LUT, not a const 0, not a closed WNS.
fn note_assign_not_lowered(module: &str, signal: &str) {
    let module = if module.is_empty() { "?" } else { module };
    let signal = if signal.is_empty() { "assign" } else { signal };
    let key = format!("{module}\0{signal}");
    let fresh = ASSIGN_NOT_LOWERED_SEEN.with(|s| s.borrow_mut().insert(key));
    if !fresh {
        return;
    }
    note_skip(format!(
        "diagnostic assign_not_lowered module={module} signal={signal} (assign not lowered; not a LUT; not a closed WNS)"
    ));
}

fn assign_not_lowered_for(module: &str) -> bool {
    let prefix = format!("{module}\0");
    ASSIGN_NOT_LOWERED_SEEN.with(|s| s.borrow().iter().any(|k| k.starts_with(&prefix)))
}

/// Generate or for-generate assign did not parse. One line per module.
/// Not a LUT. Not a closed WNS. Do not invent gates.
fn note_generate_not_lowered(module: &str) {
    let module = if module.is_empty() { "?" } else { module };
    let fresh = GEN_NOT_LOWERED_SEEN.with(|s| s.borrow_mut().insert(module.to_string()));
    if !fresh {
        return;
    }
    note_skip(format!(
        "diagnostic generate_not_lowered module={module} (generate body not mapped; not a LUT; not a closed WNS)"
    ));
}

fn generate_not_lowered_for(module: &str) -> bool {
    GEN_NOT_LOWERED_SEEN.with(|s| s.borrow().contains(module))
}

/// Posedge of a muxed clock. One line. Not a LUT, not a single user clock.
fn note_clock_mux(module: &str, signal: &str) {
    let module = if module.is_empty() { "?" } else { module };
    let signal = if signal.is_empty() { "clk" } else { signal };
    let key = format!("{module}\0{signal}");
    let fresh = CLOCK_MUX_SEEN.with(|s| s.borrow_mut().insert(key));
    if !fresh {
        return;
    }
    note_skip(format!(
        "diagnostic clock_mux module={module} signal={signal} (posedge is a mux of clocks; not a single user clock; not a closed WNS)"
    ));
}

fn clock_mux_for(module: &str) -> bool {
    let prefix = format!("{module}\0");
    CLOCK_MUX_SEEN.with(|s| s.borrow().iter().any(|k| k.starts_with(&prefix)))
}

/// Posedge of a gated clock (AND/OR of a clock and an enable). One line.
/// Not a LUT, not a single user clock. Not a ternary of two clocks.
fn note_clock_gate(module: &str, signal: &str) {
    let module = if module.is_empty() { "?" } else { module };
    let signal = if signal.is_empty() { "clk" } else { signal };
    let key = format!("{module}\0{signal}");
    let fresh = CLOCK_GATE_SEEN.with(|s| s.borrow_mut().insert(key));
    if !fresh {
        return;
    }
    note_skip(format!(
        "diagnostic clock_gate module={module} signal={signal} (posedge is a gated clock; not a single user clock; not a closed WNS)"
    ));
}

fn clock_gate_for(module: &str) -> bool {
    let prefix = format!("{module}\0");
    CLOCK_GATE_SEEN.with(|s| s.borrow().iter().any(|k| k.starts_with(&prefix)))
}

/// Word pipeline longer than 4, or a bound that is not a constant. One line.
/// Extra stages are not invented. Not a closed WNS.
fn note_word_pipeline_cap(module: &str, signal: &str, cycles: &str) {
    let module = if module.is_empty() { "?" } else { module };
    let signal = if signal.is_empty() { "res" } else { signal };
    let cycles = if cycles.is_empty() { "?" } else { cycles };
    let fresh = WORD_PIPELINE_CAP_SEEN.with(|s| s.borrow_mut().insert(module.to_string()));
    if !fresh {
        return;
    }
    note_skip(format!(
        "diagnostic word_pipeline_cap module={module} signal={signal} cycles={cycles} (word pipeline capped at 4; extra stages not invented; not a closed WNS)"
    ));
}

fn word_pipeline_cap_for(module: &str) -> bool {
    WORD_PIPELINE_CAP_SEEN.with(|s| s.borrow().contains(module))
}

/// Post-flatten leftover that hangs after `hang_diag flatten`. One line.
/// Assign cones and wide NBA cones are not bit-blasted. Not a LUT. Not a closed WNS.
fn note_flatten_cap(module: &str, signal: &str) {
    let module = if module.is_empty() { "?" } else { module };
    let signal = if signal.is_empty() { "flatten" } else { signal };
    let fresh = FLATTEN_CAP_SEEN.with(|s| s.borrow_mut().insert(module.to_string()));
    if !fresh {
        return;
    }
    note_skip(format!(
        "diagnostic flatten_cap module={module} signal={signal} (flatten cone not bit-blasted; not a LUT; not a closed WNS)"
    ));
}

fn flatten_cap_for(module: &str) -> bool {
    FLATTEN_CAP_SEEN.with(|s| s.borrow().contains(module))
}

fn rexpr_has_mux(e: &RExpr) -> bool {
    match e {
        RExpr::Mux(_, _, _) => true,
        RExpr::Not(x) | RExpr::RedXor(x) | RExpr::RedAnd(x) | RExpr::RedOr(x) => rexpr_has_mux(x),
        RExpr::Shr(a, b)
        | RExpr::Ashr(a, b)
        | RExpr::And(a, b)
        | RExpr::Or(a, b)
        | RExpr::Xor(a, b)
        | RExpr::Add(a, b)
        | RExpr::Sub(a, b)
        | RExpr::Mul(a, b)
        | RExpr::Eq(a, b)
        | RExpr::Ne(a, b)
        | RExpr::Lt(a, b) => rexpr_has_mux(a) || rexpr_has_mux(b),
        RExpr::Concat(parts) => parts.iter().any(rexpr_has_mux),
        RExpr::IndexPart { base, .. } => rexpr_has_mux(base),
        RExpr::WordAt { addr, data } => rexpr_has_mux(addr) || rexpr_has_mux(data),
        _ => false,
    }
}

/// Name the signal whose post-flatten cone stalls, or None.
/// 15011: nbas=0, assigns>=32, combo case expanded to per-bit mux assigns.
/// 14777: nbas>=128 and a wide unpacked word (width>16) that var-index lower refuses.
fn flatten_leftover_signal(rtl: &Rtl) -> Option<String> {
    // Generated TB (deque nbas=11508) walks into tens of thousands of
    // reg bits after `hang_diag flatten` and never returns. Name one
    // signal and stop. Not a LUT. Not a closed WNS.
    if rtl.nbas.len() >= 2048 {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for (lhs, _, _) in &rtl.nbas {
            *counts.entry(lhs.clone()).or_insert(0) += 1;
        }
        if let Some((name, _)) = counts.into_iter().max_by_key(|(_, n)| *n) {
            return Some(name);
        }
        return Some("flatten".into());
    }
    if rtl.nbas.is_empty() && rtl.assigns.len() >= 32 {
        let per_bit = rtl
            .assigns
            .iter()
            .filter(|(_, bit, _)| bit.is_some())
            .count();
        if per_bit >= 32 && rtl.assigns.iter().any(|(_, _, rhs)| rexpr_has_mux(rhs)) {
            let mut counts: HashMap<String, usize> = HashMap::new();
            for (lhs, bit, rhs) in &rtl.assigns {
                if bit.is_some() && rexpr_has_mux(rhs) {
                    *counts.entry(lhs.clone()).or_insert(0) += 1;
                }
            }
            if let Some((name, _)) = counts.into_iter().max_by_key(|(_, n)| *n) {
                return Some(name);
            }
        }
    }
    if rtl.nbas.len() < 128 {
        return None;
    }
    let mem = rtl
        .signals
        .iter()
        .find(|sig| sig.depth >= 16 && sig.width > 16)?;
    let mentions = rtl.nbas.iter().any(|(lhs, _, rhs)| {
        if lhs == &mem.name {
            return true;
        }
        let mut names = HashSet::new();
        rexpr_names(rhs, &mut names);
        names.contains(&mem.name)
    });
    if mentions {
        Some(mem.name.clone())
    } else {
        None
    }
}

/// Verilog gate primitive (`bufif1`, `and`, `not`, ...). One line per module.
/// Not a LUT, not a closed WNS. Re-parse and sibling instances must not repeat it.
fn note_gate_primitive(module: &str, primitive: &str) {
    let module = if module.is_empty() { "?" } else { module };
    let primitive = if primitive.is_empty() { "gate" } else { primitive };
    let fresh = GATE_PRIM_SEEN.with(|s| s.borrow_mut().insert(module.to_string()));
    if !fresh {
        return;
    }
    note_skip(format!(
        "diagnostic gate_primitive module={module} primitive={primitive} (gate primitive not mapped; not a LUT; not a closed WNS)"
    ));
}

fn gate_primitive_for(module: &str) -> bool {
    GATE_PRIM_SEEN.with(|s| s.borrow().contains(module))
}

fn is_sim_display(name: &str) -> bool {
    name == "$display"
        || name == "$write"
        || name.starts_with("$display")
        || name.starts_with("$write")
}

fn tok_is_xz_lit(t: &Tok) -> bool {
    match t {
        Tok::Pat { .. } => true,
        Tok::Ident(s) if matches!(s.as_str(), "x" | "X" | "z" | "Z") => true,
        _ => false,
    }
}

/// `===` / `!==` are tokenized as `==`/`!=` plus a leftover `=`.
fn is_case_eq_at(toks: &[Tok], i: usize) -> bool {
    matches!(toks.get(i), Some(Tok::Eq) | Some(Tok::Ne))
        && matches!(toks.get(i + 1), Some(Tok::Sym('=')))
}

fn case_eq_against_xz(toks: &[Tok], i: usize) -> bool {
    let mut j = i + 2;
    let mut depth = 0i32;
    let mut n = 0usize;
    while j < toks.len() && n < 12 {
        match &toks[j] {
            Tok::Sym('(') => depth += 1,
            Tok::Sym(')') | Tok::Sym(';') | Tok::Sym(',') if depth == 0 => break,
            Tok::Kw(k) if depth == 0 && matches!(k.as_str(), "begin" | "end" | "else" | "endcase") => {
                break
            }
            t if tok_is_xz_lit(t) => return true,
            _ => {}
        }
        j += 1;
        n += 1;
    }
    let mut k = i;
    let mut looked = 0usize;
    while k > 0 && looked < 8 {
        k -= 1;
        looked += 1;
        match &toks[k] {
            Tok::Sym(')') => {}
            Tok::Sym('(') | Tok::Sym(';') => break,
            t if tok_is_xz_lit(t) => return true,
            _ => {}
        }
    }
    false
}

/// Module (or always) uses `$display`/`$write` or a 4-state compare against X/Z.
fn slice_is_sim_only(toks: &[Tok]) -> bool {
    let mut i = 0;
    while i < toks.len() {
        if matches!(&toks[i], Tok::Kw(k) if k == "endmodule") {
            break;
        }
        if let Tok::Ident(s) = &toks[i] {
            if is_sim_display(s) {
                return true;
            }
        }
        if is_case_eq_at(toks, i) && case_eq_against_xz(toks, i) {
            return true;
        }
        i += 1;
    }
    false
}

/// Simulation model. One line. Not a LUT, not a MAC, not a closed WNS.
fn note_sim_only(module: &str) {
    let module = if module.is_empty() { "?" } else { module };
    let fresh = SIM_ONLY_SEEN.with(|s| s.borrow_mut().insert(module.to_string()));
    if !fresh {
        return;
    }
    note_skip(format!(
        "diagnostic sim_only module={module} (X/Z compare or display; not mapped; not a LUT; not a closed WNS)"
    ));
}

fn sim_only_for(module: &str) -> bool {
    SIM_ONLY_SEEN.with(|s| s.borrow().contains(module))
}

fn skip_procedural_body(p: &mut P) {
    if p.eat_kw("begin") {
        if p.eat_sym(':') {
            let _ = p.ident();
        }
        let _ = skip_begin_end(p);
    } else {
        let _ = skip_item_or_block(p);
    }
}

/// Read-only `inout` used as a load enable did not reach FF D. One line.
/// Not a closed WNS. Do not invent a bus for the port.
fn note_inout_enable_not_lowered(module: &str, signal: &str) {
    let module = if module.is_empty() { "?" } else { module };
    let signal = if signal.is_empty() { "ena" } else { signal };
    let key = format!("{module}\0{signal}");
    let fresh = INOUT_ENABLE_SEEN.with(|s| s.borrow_mut().insert(key));
    if !fresh {
        return;
    }
    note_skip(format!(
        "diagnostic inout_enable_not_lowered module={module} signal={signal} (inout load enable not mapped; not a closed WNS)"
    ));
}

fn note_negedge_write(
    module: &str,
    clk: &str,
    signal: &str,
    bit: Option<usize>,
    rhs: Option<RExpr>,
) {
    let module = if module.is_empty() { "?" } else { module };
    let clk = if clk.is_empty() { "clk" } else { clk };
    let signal = if signal.is_empty() { "always" } else { signal };
    NEGEDGE_WRITES.with(|s| {
        let mut v = s.borrow_mut();
        if !v.iter().any(|(m, _, sig, _, _)| m == module && sig == signal) {
            v.push((
                module.to_string(),
                clk.to_string(),
                signal.to_string(),
                bit,
                rhs,
            ));
        }
    });
}

fn take_negedge_writes(module: &str) -> Vec<(String, String, Option<usize>, Option<RExpr>)> {
    NEGEDGE_WRITES.with(|s| {
        let mut v = s.borrow_mut();
        let mut keep = Vec::new();
        let mut out = Vec::new();
        for (m, clk, sig, bit, rhs) in v.drain(..) {
            if m == module {
                out.push((clk, sig, bit, rhs));
            } else {
                keep.push((m, clk, sig, bit, rhs));
            }
        }
        *v = keep;
        out
    })
}

/// Negedge-only always that did not become an Hff. One line. Not a closed WNS.
fn note_negedge_not_lowered(module: &str, signal: &str) {
    let module = if module.is_empty() { "?" } else { module };
    let signal = if signal.is_empty() { "always" } else { signal };
    let key = format!("{module}\0{signal}");
    let fresh = NEGEDGE_NOT_LOWERED_SEEN.with(|s| s.borrow_mut().insert(key));
    if !fresh {
        return;
    }
    note_skip(format!(
        "diagnostic negedge_not_lowered module={module} signal={signal} (negedge always not mapped to an Hff; not a closed WNS)"
    ));
}

/// D net of a negedge flop: a named bit or a 1-bit ident. No cone walk.
fn negedge_d_net(rtl: &Rtl, signal: &str, bit: Option<usize>, rhs: &RExpr) -> Option<String> {
    let w = sig_width(rtl, signal).max(1);
    if bit.is_none() && w != 1 {
        return None;
    }
    if bit.is_some() && bit != Some(0) && w == 1 {
        return None;
    }
    match rhs {
        RExpr::Ident(s) => {
            let sw = sig_width(rtl, s).max(1);
            if sw == 1 {
                Some(bit_name(s, sw, 0))
            } else {
                None
            }
        }
        RExpr::Bit(s, i) => {
            let sw = sig_width(rtl, s).max(1);
            if *i < sw {
                Some(bit_name(s, sw, *i))
            } else {
                None
            }
        }
        RExpr::IndexPart {
            name,
            base,
            width,
            ..
        } if *width == 1 => {
            let RExpr::Const { val, .. } = base.as_ref() else {
                return None;
            };
            let sw = sig_width(rtl, name).max(1);
            let i = *val as usize;
            if i < sw {
                Some(bit_name(name, sw, i))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn hff_q_is(d: &Design, signal: &str) -> bool {
    d.cells.iter().any(|c| {
        matches!(c.kind, CellKind::Hff) && d.net_on(&c.name, "Q") == Some(signal)
    })
}

/// One Hff per negedge-only always when the write is a 1-bit flop on a named
/// clock. Falling versus rising does not invent a WNS. Otherwise one diagnostic.
fn lower_negedge_hffs(d: &mut Design, rtl: &Rtl) {
    let writes = take_negedge_writes(&rtl.module);
    for (clk, signal, bit, rhs) in writes {
        if hff_q_is(d, &signal) && is_input_port(rtl, &clk) {
            if let Some(ff) = d.cells.iter().find(|c| {
                matches!(c.kind, CellKind::Hff)
                    && d.net_on(&c.name, "Q") == Some(signal.as_str())
                    && d.net_on(&c.name, "CLK") == Some(clk.as_str())
            }) {
                eprintln!(
                    "hff_path cell={} CLK={} Q={}",
                    ff.name, clk, signal
                );
                continue;
            }
        }
        let Some(rhs) = rhs else {
            note_negedge_not_lowered(&rtl.module, &signal);
            continue;
        };
        if !is_input_port(rtl, &clk) {
            note_negedge_not_lowered(&rtl.module, &signal);
            continue;
        }
        let Some(dnet) = negedge_d_net(rtl, &signal, bit, &rhs) else {
            note_negedge_not_lowered(&rtl.module, &signal);
            continue;
        };
        let qnet = if bit.is_none() {
            bit_name(&signal, sig_width(rtl, &signal).max(1), 0)
        } else {
            bit_name(&signal, sig_width(rtl, &signal).max(1), bit.unwrap_or(0))
        };
        let ff = format!("u_nff_{signal}");
        d.add_cell(&ff, CellKind::Hff);
        d.connect(&clk, &ff, "CLK");
        d.connect(&dnet, &ff, "D");
        d.connect(&qnet, &ff, "Q");
        eprintln!("hff_path cell={ff} CLK={clk} D={dnet} Q={qnet}");
    }
}

fn seq_clocks_of(module: &str) -> Vec<String> {
    SEQ_WRITES.with(|s| {
        let mut out = Vec::new();
        for (m, clk, _) in s.borrow().iter() {
            if m == module && !out.iter().any(|c| c == clk) {
                out.push(clk.clone());
            }
        }
        out
    })
}

fn is_input_port(rtl: &Rtl, name: &str) -> bool {
    rtl.ports
        .iter()
        .any(|(n, dir, _)| n == name && *dir == PortDir::In)
}

/// Data arm of a clock mux: a plain input-port ident (a user clock).
fn clock_port_arm<'a>(rtl: &Rtl, e: &'a RExpr) -> Option<&'a str> {
    match e {
        RExpr::Ident(n) if is_input_port(rtl, n) => Some(n.as_str()),
        _ => None,
    }
}

fn is_clock_mux_expr(rtl: &Rtl, e: &RExpr) -> bool {
    let RExpr::Mux(_, t, f) = e else {
        return false;
    };
    let Some(a) = clock_port_arm(rtl, t) else {
        return false;
    };
    let Some(b) = clock_port_arm(rtl, f) else {
        return false;
    };
    a != b
}

/// Posedge/negedge is not a plain port clock, and that net is a ternary of
/// two clock ports. One diagnostic; the mux is not lowered as a data LUT.
fn note_clock_muxes(rtl: &Rtl) -> HashSet<String> {
    let mut sigs = HashSet::new();
    for clk in seq_clocks_of(&rtl.module) {
        if is_input_port(rtl, &clk) {
            continue;
        }
        let muxed = rtl
            .assigns
            .iter()
            .any(|(lhs, _, rhs)| lhs == &clk && is_clock_mux_expr(rtl, rhs));
        if muxed {
            note_clock_mux(&rtl.module, &clk);
            sigs.insert(clk);
        }
    }
    sigs
}

fn peel_not(e: &RExpr) -> &RExpr {
    let mut cur = e;
    while let RExpr::Not(inner) = cur {
        cur = inner;
    }
    cur
}

/// Enable arm of a clock gate: a signal, a bit, or `~` of those. Not a const.
fn is_enable_arm(e: &RExpr) -> bool {
    matches!(peel_not(e), RExpr::Ident(_) | RExpr::Bit(_, _))
}

fn clock_gate_clock_arm<'a>(rtl: &Rtl, e: &'a RExpr) -> Option<&'a str> {
    clock_port_arm(rtl, peel_not(e))
}

/// `assign gclk = clk & en` / `clk & ~en` / `clk | en`. One side is a clock
/// port; the other is an enable. Not a ternary of two clocks (that is clock_mux).
fn is_clock_gate_expr(rtl: &Rtl, e: &RExpr) -> bool {
    let (a, b) = match e {
        RExpr::And(a, b) | RExpr::Or(a, b) => (a.as_ref(), b.as_ref()),
        _ => return false,
    };
    let ca = clock_gate_clock_arm(rtl, a).is_some();
    let cb = clock_gate_clock_arm(rtl, b).is_some();
    let ea = is_enable_arm(a);
    let eb = is_enable_arm(b);
    (ca && eb) || (cb && ea)
}

/// Posedge/negedge is not a plain port clock, and that net is an AND/OR of a
/// clock and an enable. One diagnostic; the gate is not lowered as a data LUT.
fn note_clock_gates(rtl: &Rtl, already: &HashSet<String>) -> HashSet<String> {
    let mut sigs = HashSet::new();
    for clk in seq_clocks_of(&rtl.module) {
        if is_input_port(rtl, &clk) || already.contains(&clk) {
            continue;
        }
        let gated = rtl
            .assigns
            .iter()
            .any(|(lhs, _, rhs)| lhs == &clk && is_clock_gate_expr(rtl, rhs));
        if gated {
            note_clock_gate(&rtl.module, &clk);
            sigs.insert(clk);
        }
    }
    sigs
}

fn lhs_before_assign(toks: &[Tok], nba_only: bool) -> Option<String> {
    let mut i = 0;
    while i < toks.len() {
        let Tok::Ident(name) = &toks[i] else {
            i += 1;
            continue;
        };
        let mut j = i + 1;
        if matches!(toks.get(j), Some(Tok::Sym('['))) {
            let mut d = 0i32;
            while j < toks.len() {
                match &toks[j] {
                    Tok::Sym('[') => d += 1,
                    Tok::Sym(']') => {
                        d -= 1;
                        j += 1;
                        if d == 0 {
                            break;
                        }
                        continue;
                    }
                    _ => {}
                }
                j += 1;
            }
        }
        let hit = if nba_only {
            matches!(toks.get(j), Some(Tok::Le))
        } else {
            matches!(toks.get(j), Some(Tok::Le) | Some(Tok::Sym('=')))
        };
        if hit {
            return Some(name.clone());
        }
        i += 1;
    }
    None
}

/// First procedural LHS in an always body that did not yield an NBA.
/// Prefer `<=` so a `for (i = ...)` header does not steal the signal name.
fn seq_lhs_from_toks(toks: &[Tok]) -> String {
    lhs_before_assign(toks, true)
        .or_else(|| lhs_before_assign(toks, false))
        .unwrap_or_else(|| "always".into())
}

/// `ena` appears in a mux condition (load enable), not only as data.
fn mux_cond_mentions(e: &RExpr, name: &str) -> bool {
    match e {
        RExpr::Mux(c, t, f) => {
            let mut names = HashSet::new();
            rexpr_names(c, &mut names);
            names.contains(name) || mux_cond_mentions(t, name) || mux_cond_mentions(f, name)
        }
        RExpr::Not(x) | RExpr::RedXor(x) | RExpr::RedAnd(x) | RExpr::RedOr(x) => {
            mux_cond_mentions(x, name)
        }
        RExpr::And(a, b)
        | RExpr::Or(a, b)
        | RExpr::Xor(a, b)
        | RExpr::Add(a, b)
        | RExpr::Sub(a, b)
        | RExpr::Eq(a, b)
        | RExpr::Ne(a, b)
        | RExpr::Lt(a, b)
        | RExpr::Shr(a, b)
        | RExpr::Ashr(a, b) => mux_cond_mentions(a, name) || mux_cond_mentions(b, name),
        RExpr::Concat(parts) => parts.iter().any(|p| mux_cond_mentions(p, name)),
        RExpr::IndexPart { base, .. } => mux_cond_mentions(base, name),
        RExpr::WordAt { addr, data } => mux_cond_mentions(addr, name) || mux_cond_mentions(data, name),
        _ => false,
    }
}

fn signal_is_driven(rtl: &Rtl, name: &str) -> bool {
    rtl.nbas.iter().any(|(lhs, _, _)| lhs == name)
        || rtl.assigns.iter().any(|(lhs, _, _)| lhs == name)
}

/// Read-only inout ports that gate a clocked load. Not a driven bidirectional bus.
fn inout_load_enables(rtl: &Rtl) -> Vec<String> {
    let mut out = Vec::new();
    for (n, dir, _) in &rtl.ports {
        if !matches!(dir, PortDir::Inout) || signal_is_driven(rtl, n) {
            continue;
        }
        let used = rtl
            .nbas
            .iter()
            .any(|(_, _, rhs)| mux_cond_mentions(rhs, n));
        if used {
            out.push(n.clone());
        }
    }
    out
}

fn enabled_q_names(rtl: &Rtl, en: &str) -> HashSet<String> {
    let mut q = HashSet::new();
    for (lhs, bit, rhs) in &rtl.nbas {
        if !mux_cond_mentions(rhs, en) {
            continue;
        }
        let w = sig_width(rtl, lhs).max(1);
        if let Some(b) = bit {
            q.insert(bit_name(lhs, w, *b));
        } else {
            for i in 0..w {
                q.insert(bit_name(lhs, w, i));
            }
        }
    }
    q
}

/// Comb cone feeding `ff` D. Returns the cell pin where `want` is connected.
/// Does not walk through Hff Q, and does not invent a net.
fn d_cone_connect(d: &Design, ff: &str, want: &str) -> Option<(String, String)> {
    let start = d.net_on(ff, "D")?;
    if start == want {
        return Some((ff.to_string(), "D".into()));
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut stack = vec![start.to_string()];
    let mut steps = 0usize;
    while let Some(net) = stack.pop() {
        if steps > 256 {
            break;
        }
        steps += 1;
        if !seen.insert(net.clone()) {
            continue;
        }
        let Some(nrec) = d.nets.iter().find(|n| n.name == net) else {
            continue;
        };
        for ep in &nrec.endpoints {
            if ep.pin != "O" {
                continue;
            }
            for i in 0..6 {
                let pin = format!("I{i}");
                let Some(inode) = d.net_on(&ep.cell, &pin) else {
                    continue;
                };
                if inode == want {
                    return Some((ep.cell.clone(), pin));
                }
                stack.push(inode.to_string());
            }
        }
    }
    None
}

/// Reset-then-enable load: every gated FF D must include the inout enable.
/// One `hff_path` plus a connect when that is true. Otherwise one diagnostic
/// and the design is not a closed WNS. Does not invent a bus.
fn prove_inout_load_enables(d: &Design, rtl: &Rtl) -> bool {
    let enables = inout_load_enables(rtl);
    if enables.is_empty() {
        return false;
    }
    let mut dropped = false;
    for en in enables {
        let w = sig_width(rtl, &en).max(1);
        // 1-bit inout stays the port net. Do not widen it into a bus.
        let en_net = if w == 1 {
            en.clone()
        } else {
            bit_name(&en, w, 0)
        };
        let qnames = enabled_q_names(rtl, &en);
        let matched: Vec<String> = d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Hff))
            .filter_map(|c| {
                let q = d.net_on(&c.name, "Q")?;
                if qnames.contains(q) {
                    Some(c.name.clone())
                } else {
                    None
                }
            })
            .collect();
        if matched.is_empty() {
            note_inout_enable_not_lowered(&rtl.module, &en);
            dropped = true;
            continue;
        }
        let mut proof: Option<(String, String, String, String, String, String)> = None;
        for ff in &matched {
            match d_cone_connect(d, ff, &en_net) {
                Some((cell, pin)) => {
                    if proof.is_none() {
                        let clk = d.net_on(ff, "CLK").unwrap_or("clk").to_string();
                        let dnet = d.net_on(ff, "D").unwrap_or("").to_string();
                        let qnet = d.net_on(ff, "Q").unwrap_or("").to_string();
                        proof = Some((ff.clone(), clk, dnet, en_net.clone(), qnet, format!("{cell} {pin}")));
                    }
                }
                None => {
                    note_inout_enable_not_lowered(&rtl.module, &en);
                    dropped = true;
                }
            }
        }
        if dropped {
            continue;
        }
        if let Some((ff, clk, dnet, en_net, qnet, cell_pin)) = proof {
            let mut parts = cell_pin.split_whitespace();
            let cell = parts.next().unwrap_or("");
            let pin = parts.next().unwrap_or("");
            eprintln!("hff_path cell={ff} CLK={clk} D={dnet} EN={en_net} Q={qnet}");
            eprintln!("connect net={en_net} cell={cell} pin={pin}");
        } else {
            note_inout_enable_not_lowered(&rtl.module, &en);
            dropped = true;
        }
    }
    dropped
}

/// Clocked always that did not become an Hff. One line, then finish.
/// A wide_cone already fired for this module — do not add a second diagnostic.
fn rhs_reads_seq_mem(rhs: &RExpr, rtl: &Rtl) -> bool {
    let mut names = HashSet::new();
    rexpr_names(rhs, &mut names);
    names.iter().any(|n| {
        sig_depth(rtl, n) > 0 && rtl.nbas.iter().any(|(lhs, _, _)| lhs == n)
    })
}

/// `mem[index]` / concat of those. Never walk into a cone; depth>0 is a word read.
fn rhs_unpacked_index(rhs: &RExpr, rtl: &Rtl) -> bool {
    fn walk(e: &RExpr, rtl: &Rtl) -> bool {
        match e {
            RExpr::IndexPart { name, base, .. } => {
                sig_depth(rtl, name) > 0 || walk(base, rtl)
            }
            RExpr::Concat(parts) => parts.iter().any(|p| walk(p, rtl)),
            RExpr::WordAt { addr, data } => walk(addr, rtl) || walk(data, rtl),
            RExpr::Mux(c, t, f) => walk(c, rtl) || walk(t, rtl) || walk(f, rtl),
            RExpr::Not(x) | RExpr::RedXor(x) | RExpr::RedAnd(x) | RExpr::RedOr(x) => walk(x, rtl),
            RExpr::Shr(a, b)
            | RExpr::Ashr(a, b)
            | RExpr::And(a, b)
            | RExpr::Or(a, b)
            | RExpr::Xor(a, b)
            | RExpr::Add(a, b)
            | RExpr::Sub(a, b)
            | RExpr::Mul(a, b)
            | RExpr::Eq(a, b)
            | RExpr::Ne(a, b)
            | RExpr::Lt(a, b) => walk(a, rtl) || walk(b, rtl),
            _ => false,
        }
    }
    walk(rhs, rtl)
}

fn finish_seq_honesty(d: &Design, module: &str, wide_capped: bool) {
    let writes = take_seq_writes(module);
    if writes.is_empty() || wide_capped {
        return;
    }
    let hffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    if hffs == 0 {
        let sig = writes
            .first()
            .map(|(_, sig)| sig.clone())
            .unwrap_or_else(|| "always".into());
        note_sequential_not_lowered(module, &sig);
    }
}

fn note_wide_literal(width: usize) {
    let fresh = WIDE_LITERAL_SEEN.with(|c| {
        let seen = c.get();
        if seen {
            false
        } else {
            c.set(true);
            true
        }
    });
    if !fresh {
        return;
    }
    note_skip(format!(
        "diagnostic wide_literal width={width} (sized binary exceeds 128-bit const; high bits not folded; not a LUT)"
    ));
}

/// Set bit `bit` of a u128 accumulator. Shift-left of 128+ used to panic.
fn or_u128_bit(acc: &mut u128, bit: usize) -> bool {
    if bit >= 128 {
        return false;
    }
    *acc |= 1u128 << bit;
    true
}

/// Name after `function` has been eaten. Does not parse statements.
fn function_name_after_kw(p: &mut P) -> String {
    let save = p.i;
    skip_sv_type(p);
    let _ = p.width_opt();
    match p.ident() {
        Ok(n) => n,
        Err(_) => {
            p.i = save;
            String::new()
        }
    }
}

/// Skip a function body without entering for/if/case/assign. Uncalled.
fn skip_function_without_reentry(p: &mut P) -> String {
    let name = function_name_after_kw(p);
    skip_until_kw(p, "endfunction");
    if name.is_empty() {
        "unknown".into()
    } else {
        name
    }
}

struct OwnCache {
    /// module:hash → own-logic Design (instances not included)
    by_key: HashMap<String, Design>,
    log: Vec<String>,
}

fn cache() -> &'static Mutex<OwnCache> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Mutex<OwnCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(OwnCache {
            by_key: HashMap::new(),
            log: Vec::new(),
        })
    })
}

/// Last incremental reused/rebuilt lines (process cache, not a fake flag).
pub fn incremental_log() -> Vec<String> {
    cache()
        .lock()
        .map(|c| c.log.clone())
        .unwrap_or_default()
}

fn hash_own(rtl: &Rtl) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut feed = |s: &str| {
        for b in s.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    };
    feed(&rtl.module);
    for (n, dir, w) in &rtl.ports {
        feed(n);
        feed(&format!("{dir:?}:{w}"));
    }
    for (lhs, bit, rhs) in &rtl.nbas {
        feed(lhs);
        feed(&format!("{bit:?}"));
        feed(&format!("{rhs:?}"));
    }
    for (lhs, bit, rhs) in &rtl.assigns {
        feed(lhs);
        feed(&format!("{bit:?}"));
        feed(&format!("{rhs:?}"));
    }
    h
}

fn lower_own_cached(rtl: &Rtl) -> Result<Design, String> {
    let h = hash_own(rtl);
    let key = format!("{}:{h:x}", rtl.module);
    if let Ok(mut c) = cache().lock() {
        if let Some(hit) = c.by_key.get(&key).cloned() {
            let line = format!("incremental reused module={}", rtl.module);
            eprintln!("{line}");
            c.log.push(line);
            return Ok(hit);
        }
    }
    let line = format!("incremental rebuilt module={}", rtl.module);
    eprintln!("{line}");
    if let Ok(mut c) = cache().lock() {
        c.log.push(line);
    }
    let mut own = rtl.clone();
    own.insts.clear();
    let d = synth_rtl(&own)?;
    if let Ok(mut c) = cache().lock() {
        c.by_key.insert(key, d.clone());
    }
    Ok(d)
}

fn stitch_child(dst: &mut Design, child: &Design, inst: &Inst) {
    let prefix = format!("{}_", inst.name);
    let mut port_map: HashMap<String, String> = HashMap::new();
    for (i, p) in child.ports.iter().enumerate() {
        if let Some((_, net)) = inst
            .conns
            .iter()
            .find(|(pn, _)| pn == &p.name)
            .or_else(|| inst.conns.iter().find(|(pn, _)| pn == &format!("#{i}")))
        {
            port_map.insert(p.name.clone(), net.clone());
        }
    }
    let map_net = |n: &str| -> String {
        if let Some(p) = port_map.get(n) {
            return p.clone();
        }
        format!("{prefix}{n}")
    };
    for c in &child.cells {
        let mut cell = c.clone();
        cell.name = format!("{prefix}{}", c.name);
        dst.cells.push(cell);
    }
    for n in &child.nets {
        let mut net = n.clone();
        net.name = map_net(&n.name);
        for e in &mut net.endpoints {
            e.cell = format!("{prefix}{}", e.cell);
        }
        if let Some(ex) = dst.nets.iter_mut().find(|x| x.name == net.name) {
            ex.endpoints.extend(net.endpoints);
        } else {
            dst.nets.push(net);
        }
    }
}

fn assemble_module(
    mods: &HashMap<String, Rtl>,
    name: &str,
    visiting: &mut HashSet<String>,
) -> Result<Design, String> {
    let proto = mods
        .get(name)
        .ok_or_else(|| format!("unknown module {name}"))?;
    if !visiting.insert(name.to_string()) {
        return lower_own_cached(proto);
    }
    let mut d = lower_own_cached(proto)?;
    d.name = proto.module.clone();
    for inst in &proto.insts {
        if !mods.contains_key(&inst.module) {
            note_skip(format!(
                "diagnostic unknown_instance module={} inst={} child={} (child body absent; not a LUT)",
                name, inst.name, inst.module
            ));
            continue;
        }
        // Ibex-scale trees: keep already-lowered cells, do not copy the rest.
        if d.cells.len() >= 8_000 {
            note_skip(format!(
                "diagnostic assemble_cap module={} inst={} child={} (hierarchy cap; not a LUT)",
                name, inst.name, inst.module
            ));
            continue;
        }
        let child = assemble_module(mods, &inst.module, visiting)?;
        stitch_child(&mut d, &child, inst);
        // A child cone that was not mapped makes this netlist incomplete.
        if child.attrs.get("WIDE_CONE") == Some("1") {
            d.attrs.set("WIDE_CONE", "1");
        }
        if child.attrs.get("ASSIGN_NOT_LOWERED") == Some("1") {
            d.attrs.set("ASSIGN_NOT_LOWERED", "1");
        }
        if child.attrs.get("GENERATE_NOT_LOWERED") == Some("1") {
            d.attrs.set("GENERATE_NOT_LOWERED", "1");
        }
        if child.attrs.get("WIDTH_OVERFLOW") == Some("1") {
            d.attrs.set("WIDTH_OVERFLOW", "1");
        }
        if child.attrs.get("WORD_PIPELINE_CAP") == Some("1") {
            d.attrs.set("WORD_PIPELINE_CAP", "1");
        }
        if child.attrs.get("FLATTEN_CAP") == Some("1") {
            d.attrs.set("FLATTEN_CAP", "1");
        }
        if child.attrs.get("CLOCK_MUX") == Some("1") {
            d.attrs.set("CLOCK_MUX", "1");
        }
        if child.attrs.get("GATE_PRIMITIVE") == Some("1") {
            d.attrs.set("GATE_PRIMITIVE", "1");
        }
        if child.attrs.get("SIM_ONLY") == Some("1") {
            d.attrs.set("SIM_ONLY", "1");
            d.attrs.set("NO_BODY", "1");
        }
    }
    visiting.remove(name);
    Ok(d)
}

fn fnv_module_present(text: &str, name: &str) -> bool {
    let mut i = 0;
    let b = text.as_bytes();
    while i + 6 < b.len() {
        if &b[i..i + 6] == b"module" {
            let prev_ok = i == 0 || !b[i - 1].is_ascii_alphanumeric();
            let next = i + 6;
            if prev_ok && next < b.len() && b[next].is_ascii_whitespace() {
                let mut j = next;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                let start = j;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                if &text[start..j] == name {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

fn sibling_modules(dir: &Path, missing: &HashSet<String>, skip: &Path) -> Result<Vec<Rtl>, String> {
    let mut out = Vec::new();
    if missing.is_empty() || !dir.is_dir() {
        return Ok(out);
    }
    let rd = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    for ent in rd.flatten() {
        let p = ent.path();
        if p == skip {
            continue;
        }
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext != "v" && ext != "sv" {
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        if !missing.iter().any(|m| fnv_module_present(&text, m)) {
            continue;
        }
        let expanded = expand_includes(&text, dir);
        let pre = preprocess_sv(&strip_comments(&expanded));
        out.extend(parse_source(&pre)?);
    }
    Ok(out)
}

fn strip_comments(s: &str) -> String {
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Number(u128, usize),
    /// based number with x/z/? don't-care bits (casez).
    Pat { val: u128, care: u128, width: usize },
    Kw(String),
    Str(String),
    Sym(char),
    Le, // <=
    Ge, // >=
    Eq, // ==
    Ne, // !=
    Lor, // ||
    Land, // &&
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    let kws = [
        "module", "endmodule", "input", "output", "logic", "wire", "reg", "always_ff", "always",
        "begin", "end", "posedge", "negedge", "assign", "inout", "if", "else", "always_comb",
        "case", "casez", "casex", "endcase", "default", "generate", "endgenerate", "genvar",
        "for", "int", "parameter", "localparam", "initial", "package", "endpackage", "typedef",
        "import", "export", "struct", "enum", "packed", "signed", "unsigned", "function",
        "endfunction", "task", "endtask", "return", "always_latch", "unique", "priority",
        "automatic", "void", "const", "var", "ref", "static", "extern", "virtual", "pure",
        "interface", "endinterface", "modport", "clocking", "property", "endproperty",
        "assert", "assume", "cover", "sequence", "endsequence",
    ];
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '"' {
            i += 1;
            let mut s = String::new();
            while i < chars.len() && chars[i] != '"' {
                s.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            out.push(Tok::Str(s));
            continue;
        }
        if c == '<' && chars.get(i + 1) == Some(&'=') {
            out.push(Tok::Le);
            i += 2;
            continue;
        }
        if c == '>' && chars.get(i + 1) == Some(&'=') {
            out.push(Tok::Ge);
            i += 2;
            continue;
        }
        if c == '=' && chars.get(i + 1) == Some(&'=') {
            out.push(Tok::Eq);
            i += 2;
            continue;
        }
        if c == '!' && chars.get(i + 1) == Some(&'=') {
            out.push(Tok::Ne);
            i += 2;
            continue;
        }
        if c == '|' && chars.get(i + 1) == Some(&'|') {
            out.push(Tok::Lor);
            i += 2;
            continue;
        }
        if c == '&' && chars.get(i + 1) == Some(&'&') {
            out.push(Tok::Land);
            i += 2;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(chars.len());
            continue;
        }
        if c == '\\' {
            i += 1;
            if i < chars.len() && chars[i] == '\n' {
                i += 1;
            }
            continue;
        }
        if c == '\'' {
            i += 1;
            let mut base = 2;
            let mut saw_base = false;
            if i < chars.len() {
                match chars[i].to_ascii_lowercase() {
                    'b' => {
                        base = 2;
                        saw_base = true;
                        i += 1;
                    }
                    'h' => {
                        base = 16;
                        saw_base = true;
                        i += 1;
                    }
                    'd' => {
                        base = 10;
                        saw_base = true;
                        i += 1;
                    }
                    'o' => {
                        base = 8;
                        saw_base = true;
                        i += 1;
                    }
                    '0' | '1' => {}
                    _ => {
                        out.push(Tok::Sym('\''));
                        continue;
                    }
                }
            }
            let mut digits = String::new();
            while i < chars.len() {
                let ch = chars[i];
                if ch == '_' {
                    i += 1;
                    continue;
                }
                let ok = ch.is_ascii_hexdigit() || matches!(ch, 'x' | 'X' | 'z' | 'Z' | '?');
                if !ok {
                    break;
                }
                digits.push(ch);
                i += 1;
            }
            if digits.is_empty() {
                out.push(Tok::Sym('\''));
                continue;
            }
            let width = if saw_base { digits.len().max(1) } else { 1 };
            let val = u128::from_str_radix(
                &digits
                    .chars()
                    .map(|c| if matches!(c, 'x' | 'X' | 'z' | 'Z' | '?') { '0' } else { c })
                    .collect::<String>(),
                base,
            )
            .unwrap_or(0);
            out.push(Tok::Number(val, width));
            continue;
        }
        if "@();,:+-~^|&![]'=#?*<>.{}/%".contains(c) {
            out.push(Tok::Sym(c));
            i += 1;
            continue;
        }
        if c == '`' {
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            continue;
        }
        if c == '$' || c.is_ascii_alphabetic() || c == '_' {
            let mut n = String::new();
            if c == '$' {
                n.push(c);
                i += 1;
            }
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                n.push(chars[i]);
                i += 1;
            }
            if kws.contains(&n.as_str()) {
                out.push(Tok::Kw(n));
            } else {
                out.push(Tok::Ident(n));
            }
            continue;
        }
        if c.is_ascii_digit() {
            let mut n = String::new();
            while i < chars.len() && chars[i].is_ascii_digit() {
                n.push(chars[i]);
                i += 1;
            }
            if chars.get(i) == Some(&'\'') {
                i += 1;
                let width: usize = n.parse().unwrap_or(1);
                let mut base = 10;
                if i < chars.len() {
                    match chars[i].to_ascii_lowercase() {
                        'b' => {
                            base = 2;
                            i += 1;
                        }
                        'h' => {
                            base = 16;
                            i += 1;
                        }
                        'd' => {
                            base = 10;
                            i += 1;
                        }
                        'o' => {
                            base = 8;
                            i += 1;
                        }
                        _ => {}
                    }
                }
                let mut digits = String::new();
                while i < chars.len() {
                    let ch = chars[i];
                    if ch == '_' {
                        i += 1;
                        continue;
                    }
                    let ok = ch.is_ascii_hexdigit()
                        || matches!(ch, 'x' | 'X' | 'z' | 'Z' | '?');
                    if !ok {
                        break;
                    }
                    digits.push(ch);
                    i += 1;
                }
                let mut val = 0u128;
                let mut care = 0u128;
                let mut dc = false;
                if base == 2 {
                    let mut wide = width > 128;
                    for (off, ch) in digits.chars().enumerate() {
                        let bit = width.saturating_sub(off + 1);
                        match ch {
                            '1' => {
                                wide |= !or_u128_bit(&mut val, bit);
                                wide |= !or_u128_bit(&mut care, bit);
                            }
                            '0' => {
                                wide |= !or_u128_bit(&mut care, bit);
                            }
                            _ => {
                                dc = true;
                            }
                        }
                    }
                    if wide {
                        note_wide_literal(width);
                    }
                } else {
                    let parsed = u128::from_str_radix(
                        &digits
                            .chars()
                            .map(|c| if matches!(c, 'x' | 'X' | 'z' | 'Z' | '?') { '0' } else { c })
                            .collect::<String>(),
                        base,
                    )
                    .unwrap_or(0);
                    val = parsed;
                    care = if width >= 128 { u128::MAX } else { (1u128 << width.max(1)) - 1 };
                    if digits.chars().any(|c| matches!(c, 'x' | 'X' | 'z' | 'Z' | '?')) {
                        dc = true;
                    }
                }
                if dc {
                    out.push(Tok::Pat { val, care, width });
                } else {
                    out.push(Tok::Number(val, width));
                }
            } else {
                let val: u128 = n.parse().unwrap_or(0);
                out.push(Tok::Number(val, 32));
            }
            continue;
        }
        // Nothing unknown: leftover glyphs become symbols the parser can eat.
        out.push(Tok::Sym(c));
        i += 1;
    }
    Ok(out)
}

struct P<'a> {
    t: &'a [Tok],
    i: usize,
    params: HashMap<String, u128>,
    /// Signal/port widths seen so far (slice assigns and const if).
    widths: HashMap<String, usize>,
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<&'a Tok> {
        self.t.get(self.i)
    }
    fn bump(&mut self) -> Option<&'a Tok> {
        let t = self.t.get(self.i)?;
        self.i += 1;
        Some(t)
    }
    fn eat_kw(&mut self, k: &str) -> bool {
        match self.peek() {
            Some(Tok::Kw(s)) if s == k => {
                self.i += 1;
                true
            }
            _ => false,
        }
    }
    fn eat_sym(&mut self, c: char) -> bool {
        match self.peek() {
            Some(Tok::Sym(s)) if *s == c => {
                self.i += 1;
                true
            }
            _ => false,
        }
    }
    fn ident(&mut self) -> Result<String, String> {
        match self.bump() {
            Some(Tok::Ident(s)) => Ok(s.clone()),
            other => Err(format!("expected ident, got {other:?}")),
        }
    }
    fn width_opt(&mut self) -> Result<usize, String> {
        if !self.eat_sym('[') {
            return Ok(1);
        }
        let msb = const_u(self)?;
        if !self.eat_sym(':') {
            return Err("range :".into());
        }
        let lsb = const_u(self)?;
        if !self.eat_sym(']') {
            return Err("]".into());
        }
        match range_width(msb, lsb) {
            Ok(w) => Ok(w),
            Err(e) => {
                note_width_overflow();
                Err(e)
            }
        }
    }
}

/// Ibex `ibex_pkg::regfile_e` literals — packages are skipped, so seed defaults
/// so `RegFile == RegFileFF` links the FF register file (~992 sequential bits).
fn enum_const_default(name: &str) -> Option<u128> {
    match name {
        "RegFileFF" => Some(0),
        "RegFileFPGA" => Some(1),
        "RegFileLatch" => Some(2),
        // Harmless Ibex param enums (packages skipped).
        "RV32MNone" => Some(0),
        "RV32MSlow" => Some(1),
        "RV32MFast" => Some(2),
        "RV32MSingleCycle" => Some(3),
        "RV32BNone" => Some(0),
        "RV32BBalanced" => Some(1),
        "RV32BOTEarlGrey" => Some(2),
        "RV32BFull" => Some(3),
        _ => None,
    }
}

fn clog2_u(n: u128) -> u128 {
    if n <= 1 {
        0
    } else {
        (u128::BITS - (n - 1).leading_zeros()) as u128
    }
}

fn const_atom(p: &mut P) -> Result<u128, String> {
    // Unary `(-3)+W` / `(-1)+W` are elaboration consts, not a dropped always.
    if p.eat_sym('+') {
        return const_atom(p);
    }
    if p.eat_sym('-') {
        let n = const_atom(p)?;
        return Ok(0u128.wrapping_sub(n));
    }
    if p.eat_sym('(') {
        let v = const_u(p)?;
        if !p.eat_sym(')') {
            return Err("const )".into());
        }
        return Ok(v);
    }
    match p.peek() {
        Some(Tok::Number(v, _)) => {
            let n = *v;
            p.bump();
            Ok(n)
        }
        Some(Tok::Ident(s)) if s == "$clog2" => {
            p.bump();
            if !p.eat_sym('(') {
                return Err("$clog2 (".into());
            }
            let arg = const_u(p)?;
            if !p.eat_sym(')') {
                return Err("$clog2 )".into());
            }
            Ok(clog2_u(arg))
        }
        Some(Tok::Ident(s)) => {
            let mut name = s.clone();
            p.bump();
            // `pkg::EnumLit` after skipped packages — resolve the member name.
            if matches!(p.peek(), Some(Tok::Sym(':')))
                && matches!(p.t.get(p.i + 1), Some(Tok::Sym(':')))
                && matches!(p.t.get(p.i + 2), Some(Tok::Ident(_)))
            {
                p.bump();
                p.bump();
                name = p.ident()?;
            }
            if let Some(v) = p.params.get(&name).copied() {
                return Ok(v);
            }
            enum_const_default(&name).ok_or_else(|| format!("unknown param {name}"))
        }
        other => Err(format!("const atom {other:?}")),
    }
}

fn const_u(p: &mut P) -> Result<u128, String> {
    let mut v = const_atom(p)?;
    loop {
        if p.eat_sym('+') {
            // wrapping so `(-1)+W` is W-1, not a saturated width_overflow.
            v = v.wrapping_add(const_atom(p)?);
        } else if p.eat_sym('-') {
            v = v.wrapping_sub(const_atom(p)?);
        } else if p.eat_sym('*') {
            if p.eat_sym('*') {
                let e = const_atom(p)? as u32;
                v = v.saturating_pow(e.min(63));
            } else {
                v = v.saturating_mul(const_atom(p)?);
            }
        } else if p.eat_sym('/') {
            let d = const_atom(p)?.max(1);
            v /= d;
        } else if p.eat_sym('%') {
            let d = const_atom(p)?.max(1);
            v %= d;
        } else if p.eat_sym('<') {
            // `<<` or arithmetic `<<<` (both logical shifts on const u128).
            if !p.eat_sym('<') {
                // Lone '<' is relational — put it back for const_rel.
                p.i -= 1;
                break;
            }
            let _ = p.eat_sym('<'); // optional third < for <<<
            let sh = const_atom(p)? as u32;
            v = v.checked_shl(sh.min(63)).unwrap_or(0);
        } else {
            break;
        }
    }
    if p.eat_sym('?') {
        let t = const_u(p)?;
        if !p.eat_sym(':') {
            return Err("const ternary :".into());
        }
        let f = const_u(p)?;
        return Ok(if v != 0 { t } else { f });
    }
    Ok(v)
}

fn const_rel(p: &mut P) -> Result<bool, String> {
    let l = const_u(p)?;
    if matches!(p.peek(), Some(Tok::Eq)) {
        p.bump();
        return Ok(l == const_u(p)?);
    }
    if matches!(p.peek(), Some(Tok::Ne)) {
        p.bump();
        return Ok(l != const_u(p)?);
    }
    if matches!(p.peek(), Some(Tok::Le)) {
        p.bump();
        return Ok(l <= const_u(p)?);
    }
    if matches!(p.peek(), Some(Tok::Ge)) {
        p.bump();
        return Ok(l >= const_u(p)?);
    }
    if p.eat_sym('<') {
        return Ok(l < const_u(p)?);
    }
    if p.eat_sym('>') {
        return Ok(l > const_u(p)?);
    }
    Ok(l != 0)
}

/// Generate/parameter const condition: relational, then left-assoc `||` / `&&`.
fn const_cond(p: &mut P) -> Result<bool, String> {
    let mut v = const_rel(p)?;
    loop {
        match p.peek() {
            Some(Tok::Land) => {
                p.bump();
                let r = const_rel(p)?;
                v = v && r;
            }
            Some(Tok::Lor) => {
                p.bump();
                let r = const_rel(p)?;
                v = v || r;
            }
            _ => break,
        }
    }
    Ok(v)
}

fn parse_rexpr(p: &mut P) -> Result<RExpr, String> {
    let e = parse_lor(p)?;
    if p.eat_sym('?') {
        let t = parse_rexpr(p)?;
        if !p.eat_sym(':') {
            return Err("ternary :".into());
        }
        let f = parse_rexpr(p)?;
        return Ok(RExpr::Mux(Box::new(e), Box::new(t), Box::new(f)));
    }
    Ok(e)
}

fn parse_lor(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_land(p)?;
    loop {
        match p.peek() {
            Some(Tok::Lor) => {
                p.bump();
                let r = parse_land(p)?;
                e = RExpr::Or(Box::new(e), Box::new(r));
            }
            _ => break,
        }
    }
    Ok(e)
}

fn parse_land(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_cmp(p)?;
    loop {
        match p.peek() {
            Some(Tok::Land) => {
                p.bump();
                let r = parse_cmp(p)?;
                e = RExpr::And(Box::new(e), Box::new(r));
            }
            _ => break,
        }
    }
    Ok(e)
}

fn parse_cmp(p: &mut P) -> Result<RExpr, String> {
    let e = parse_shift(p)?;
    if matches!(p.peek(), Some(Tok::Eq)) {
        p.bump();
        let r = parse_shift(p)?;
        if let (
            RExpr::Const { val: a, care: ca, .. },
            RExpr::Const { val: b, care: cb, .. },
        ) = (&e, &r)
        {
            if *ca & 1 == 1 && *cb & 1 == 1 {
                return Ok(RExpr::Const {
                    val: if a == b { 1 } else { 0 },
                    width: 1,
                    care: 1,
                });
            }
        }
        return Ok(RExpr::Eq(Box::new(e), Box::new(r)));
    }
    if matches!(p.peek(), Some(Tok::Ne)) {
        p.bump();
        return Ok(RExpr::Ne(Box::new(e), Box::new(parse_shift(p)?)));
    }
    // a <= b  ≡  !(b < a)
    if matches!(p.peek(), Some(Tok::Le)) {
        p.bump();
        let r = parse_shift(p)?;
        return Ok(RExpr::Not(Box::new(RExpr::Lt(Box::new(r), Box::new(e)))));
    }
    // a >= b  ≡  !(a < b)
    if matches!(p.peek(), Some(Tok::Ge)) {
        p.bump();
        let r = parse_shift(p)?;
        return Ok(RExpr::Not(Box::new(RExpr::Lt(Box::new(e), Box::new(r)))));
    }
    if p.eat_sym('<') {
        // Do not steal `<<` (left shift handled in parse_shift).
        if matches!(p.peek(), Some(Tok::Sym('<'))) {
            p.i -= 1;
            return Ok(e);
        }
        return Ok(RExpr::Lt(Box::new(e), Box::new(parse_shift(p)?)));
    }
    if matches!(p.peek(), Some(Tok::Sym('>')))
        && matches!(p.t.get(p.i + 1), Some(Tok::Sym('>')))
    {
        // `>>` / `>>>` belong to parse_shift (already consumed there); leave alone.
        return Ok(e);
    }
    if p.eat_sym('>') {
        // a > b  ≡  b < a
        return Ok(RExpr::Lt(Box::new(parse_shift(p)?), Box::new(e)));
    }
    Ok(e)
}

fn parse_shift(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_add(p)?;
    loop {
        if matches!(p.peek(), Some(Tok::Sym('>')))
            && matches!(p.t.get(p.i + 1), Some(Tok::Sym('>')))
        {
            p.bump();
            p.bump();
            // `>>>` = arithmetic right shift; `>>` = logical.
            let arith = p.eat_sym('>');
            let sh = parse_add(p)?;
            e = if arith {
                RExpr::Ashr(Box::new(e), Box::new(sh))
            } else {
                RExpr::Shr(Box::new(e), Box::new(sh))
            };
            continue;
        }
        // `<<` / `<<<` — const fold when both sides const; else leave as identity*shift via mul-of-power2 not needed for corpus.
        if matches!(p.peek(), Some(Tok::Sym('<')))
            && matches!(p.t.get(p.i + 1), Some(Tok::Sym('<')))
        {
            p.bump();
            p.bump();
            let _ = p.eat_sym('<'); // <<<
            let sh = parse_add(p)?;
            e = match (&e, &sh) {
                (
                    RExpr::Const { val: a, width: wa, care: ca },
                    RExpr::Const { val: b, .. },
                ) => {
                    let shv = (*b).min(63) as u32;
                    RExpr::Const {
                        val: a.checked_shl(shv).unwrap_or(0),
                        width: *wa,
                        care: *ca,
                    }
                }
                _ => e, // non-const shl: keep LHS (rare); prefer not to drop assign
            };
            continue;
        }
        break;
    }
    Ok(e)
}

fn parse_add(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_mul(p)?;
    loop {
        // Do not consume the '+'/'-' of indexed part-select `+:` / `-:`.
        if matches!(p.peek(), Some(Tok::Sym('+')))
            && matches!(p.t.get(p.i + 1), Some(Tok::Sym(':')))
        {
            break;
        }
        if matches!(p.peek(), Some(Tok::Sym('-')))
            && matches!(p.t.get(p.i + 1), Some(Tok::Sym(':')))
        {
            break;
        }
        if p.eat_sym('+') {
            let r = parse_mul(p)?;
            e = match (&e, &r) {
                (
                    RExpr::Const { val: a, width: wa, care: ca },
                    RExpr::Const { val: b, width: wb, care: cb },
                ) => RExpr::Const {
                    val: a.wrapping_add(*b),
                    width: (*wa).max(*wb),
                    care: *ca & *cb,
                },
                _ => RExpr::Add(Box::new(e), Box::new(r)),
            };
            continue;
        }
        if p.eat_sym('-') {
            let r = parse_mul(p)?;
            e = match (&e, &r) {
                (
                    RExpr::Const { val: a, width: wa, care: ca },
                    RExpr::Const { val: b, width: wb, care: cb },
                ) => RExpr::Const {
                    val: a.wrapping_sub(*b),
                    width: (*wa).max(*wb),
                    care: *ca & *cb,
                },
                _ => RExpr::Sub(Box::new(e), Box::new(r)),
            };
            continue;
        }
        break;
    }
    Ok(e)
}

fn parse_mul(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_or_r(p)?;
    loop {
        if p.eat_sym('*') {
            let r = parse_or_r(p)?;
            e = match (&e, &r) {
                (
                    RExpr::Const { val: a, width: wa, care: ca },
                    RExpr::Const { val: b, width: wb, care: cb },
                ) => RExpr::Const {
                    val: a.wrapping_mul(*b),
                    width: (*wa).max(*wb),
                    care: *ca & *cb,
                },
                _ => RExpr::Mul(Box::new(e), Box::new(r)),
            };
            continue;
        }
        if p.eat_sym('/') {
            let r = parse_or_r(p)?;
            e = match (&e, &r) {
                (
                    RExpr::Const { val: a, width: wa, care: ca },
                    RExpr::Const { val: b, width: wb, care: cb },
                ) if *b != 0 => RExpr::Const {
                    val: a / *b,
                    width: (*wa).max(*wb),
                    care: *ca & *cb,
                },
                _ => return Err("div needs const operands".into()),
            };
            continue;
        }
        break;
    }
    Ok(e)
}

fn parse_or_r(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_xor_r(p)?;
    while p.eat_sym('|') {
        let r = parse_xor_r(p)?;
        e = RExpr::Or(Box::new(e), Box::new(r));
    }
    Ok(e)
}

fn parse_xor_r(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_and_r(p)?;
    while p.eat_sym('^') {
        let r = parse_and_r(p)?;
        e = RExpr::Xor(Box::new(e), Box::new(r));
    }
    Ok(e)
}

fn parse_and_r(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_un_r(p)?;
    while p.eat_sym('&') {
        let r = parse_un_r(p)?;
        e = RExpr::And(Box::new(e), Box::new(r));
    }
    Ok(e)
}

fn parse_un_r(p: &mut P) -> Result<RExpr, String> {
    if p.eat_sym('~') || p.eat_sym('!') {
        return Ok(RExpr::Not(Box::new(parse_un_r(p)?)));
    }
    // Unary minus: -a  ≡  0 - a. Fold consts so `(-3)+W` is a range bound.
    if p.eat_sym('-') {
        let x = parse_un_r(p)?;
        if let RExpr::Const { val, width, care } = x {
            return Ok(RExpr::Const {
                val: 0u128.wrapping_sub(val),
                width,
                care,
            });
        }
        return Ok(RExpr::Sub(
            Box::new(RExpr::Const {
                val: 0,
                width: 32,
                care: u128::MAX,
            }),
            Box::new(x),
        ));
    }
    // Unary reductions (distinct from binary &/|/ ^ in parse_*_r).
    if p.eat_sym('^') {
        return Ok(RExpr::RedXor(Box::new(parse_un_r(p)?)));
    }
    if p.eat_sym('&') {
        return Ok(RExpr::RedAnd(Box::new(parse_un_r(p)?)));
    }
    if p.eat_sym('|') {
        return Ok(RExpr::RedOr(Box::new(parse_un_r(p)?)));
    }
    parse_atom_r(p)
}

fn care_mask(width: usize) -> u128 {
    if width >= 128 {
        u128::MAX
    } else {
        (1u128 << width.max(1)) - 1
    }
}

fn parse_atom_r(p: &mut P) -> Result<RExpr, String> {
    if p.eat_sym('(') {
        let e = parse_rexpr(p)?;
        if !p.eat_sym(')') {
            return Err(")".into());
        }
        return Ok(e);
    }
    // Concat `{a,b}` or replication `{N{expr}}` (fold `{N{1'b0}}` to Const zero).
    if p.eat_sym('{') {
        let first = parse_rexpr(p)?;
        if p.eat_sym('{') {
            let inner = parse_rexpr(p)?;
            if !p.eat_sym('}') {
                return Err("replic }".into());
            }
            if !p.eat_sym('}') {
                return Err("replic }}".into());
            }
            let n = match first {
                RExpr::Const { val, .. } => val as usize,
                _ => return Err("replic count not const".into()),
            };
            let n = n.min(256);
            if matches!(inner, RExpr::Const { val: 0, .. }) {
                return Ok(RExpr::Const {
                    val: 0,
                    width: n.max(1),
                    care: care_mask(n.max(1)),
                });
            }
            if let RExpr::Const { val, width, care } = inner {
                let w = width.max(1);
                let mut acc = 0u128;
                let mut tw = 0usize;
                for _ in 0..n {
                    acc |= (val & care_mask(w)) << tw;
                    tw = tw.saturating_add(w);
                    if tw >= 128 {
                        break;
                    }
                }
                return Ok(RExpr::Const {
                    val: acc,
                    width: (n * w).max(1).min(128),
                    care: care_mask((n * w).max(1).min(128)) & if care == 0 { 0 } else { u128::MAX },
                });
            }
            let mut parts = Vec::with_capacity(n.max(1));
            for _ in 0..n.max(1) {
                parts.push(inner.clone());
            }
            return Ok(RExpr::Concat(parts));
        }
        let mut parts = vec![first];
        while p.eat_sym(',') {
            parts.push(parse_rexpr(p)?);
        }
        if !p.eat_sym('}') {
            return Err("concat }".into());
        }
        if parts.len() == 1 {
            return Ok(parts.pop().unwrap());
        }
        return Ok(RExpr::Concat(parts));
    }
    match p.bump() {
        Some(Tok::Str(s)) => {
            let h = str_param_hash(s);
            Ok(RExpr::Const {
                val: h,
                width: 32,
                care: u128::MAX,
            })
        }
        Some(Tok::Number(v, w)) => {
            let care = if *w >= 128 { u128::MAX } else { (1u128 << (*w).max(1)) - 1 };
            Ok(RExpr::Const {
                val: *v,
                width: *w,
                care,
            })
        }
        Some(Tok::Pat { val, care, width }) => Ok(RExpr::Const {
            val: *val,
            width: *width,
            care: *care,
        }),
        Some(Tok::Ident(s)) => {
            let name = s.clone();
            if p.eat_sym('[') {
                // Indexed part-select: sig[base +: W] / sig[base -: W]
                // or range/bit. Prefer +: / -: before treating ':' as range.
                // `lut[addr_b<<1+1]`: shift binds looser than `+` (Verilog), so
                // parse_add stops at `<<` and the index never reaches `]`.
                let base = parse_shift(p)?;
                if p.eat_sym('+') && p.eat_sym(':') {
                    let w = const_u(p)? as usize;
                    if !p.eat_sym(']') {
                        return Err("]".into());
                    }
                    Ok(RExpr::IndexPart {
                        name,
                        base: Box::new(base),
                        width: w.max(1),
                        ascending: true,
                    })
                } else if p.eat_sym('-') && p.eat_sym(':') {
                    let w = const_u(p)? as usize;
                    if !p.eat_sym(']') {
                        return Err("]".into());
                    }
                    Ok(RExpr::IndexPart {
                        name,
                        base: Box::new(base),
                        width: w.max(1),
                        ascending: false,
                    })
                } else if p.eat_sym(':') {
                    let a = match base {
                        RExpr::Const { val, .. } => val as usize,
                        _ => return Err("range msb not const".into()),
                    };
                    let b = const_u(p)? as usize;
                    if !p.eat_sym(']') {
                        return Err("]".into());
                    }
                    let hi = a.max(b);
                    let lo = a.min(b);
                    // Parameter part-select e.g. SEED[STATE_WIDTH-1:0]
                    if let Some(v) = p.params.get(&name).copied() {
                        let w = match range_width(hi as u128, lo as u128) {
                            Ok(w) => w.max(1),
                            Err(e) => {
                                note_width_overflow();
                                return Err(e);
                            }
                        };
                        let mask = care_mask(w.min(128));
                        let val = (v >> lo) & mask;
                        return Ok(RExpr::Const {
                            val,
                            width: w,
                            care: mask,
                        });
                    }
                    Ok(RExpr::Range(name, lo, hi))
                } else {
                    if !p.eat_sym(']') {
                        return Err("]".into());
                    }
                    match base {
                        RExpr::Const { val, .. } => {
                            if let Some(v) = p.params.get(&name).copied() {
                                let bit = val as usize;
                                return Ok(RExpr::Const {
                                    val: (v >> bit) & 1,
                                    width: 1,
                                    care: 1,
                                });
                            }
                            Ok(RExpr::Bit(name, val as usize))
                        }
                        other => Ok(RExpr::IndexPart {
                            name,
                            base: Box::new(other),
                            width: 1,
                            ascending: true,
                        }),
                    }
                }
            } else if let Some(v) = p.params.get(&name).copied() {
                // Parameter used in an expression (e.g. sel*DW +: DW).
                Ok(RExpr::Const {
                    val: v,
                    width: 32,
                    care: u128::MAX,
                })
            } else {
                Ok(RExpr::Ident(name))
            }
        }
        other => Err(format!("atom {other:?}")),
    }
}

fn parse_port_dir(p: &mut P) -> Option<PortDir> {
    if p.eat_kw("input") {
        Some(PortDir::In)
    } else if p.eat_kw("output") {
        Some(PortDir::Out)
    } else if p.eat_kw("inout") {
        // Keep the declaration. Synthesis treats a read-only inout as the
        // load-enable input; it is not rewritten into a bus.
        Some(PortDir::Inout)
    } else {
        None
    }
}

fn skip_logic(p: &mut P) {
    let _ = p.eat_kw("logic");
    let _ = p.eat_kw("wire");
    let _ = p.eat_kw("reg");
    let _ = p.eat_kw("signed");
    let _ = p.eat_kw("unsigned");
    let _ = p.eat_kw("var");
}

fn parse_lhs(p: &mut P) -> Result<(String, Option<usize>), String> {
    let name = p.ident()?;
    if p.eat_sym('[') {
        // Bit, or const range name[hi:lo] (treated as full-vector assign).
        let save = p.i;
        if let Ok(hi) = const_u(p) {
            if p.eat_sym(':') {
                let _lo = const_u(p)?;
                if !p.eat_sym(']') {
                    return Err("]".into());
                }
                let _ = hi;
                return Ok((name, None));
            }
            if p.eat_sym(']') {
                return Ok((name, Some(hi as usize)));
            }
        }
        p.i = save;
        match p.bump() {
            Some(Tok::Number(v, _)) => {
                let idx = *v as usize;
                if !p.eat_sym(']') {
                    return Err("]".into());
                }
                Ok((name, Some(idx)))
            }
            Some(Tok::Ident(_)) => {
                if !p.eat_sym(']') {
                    return Err("]".into());
                }
                Ok((name, None))
            }
            _ => Err("lhs index".into()),
        }
    } else {
        Ok((name, None))
    }
}

type Nba = (String, Option<usize>, RExpr);

fn parse_nba(p: &mut P) -> Result<Nba, String> {
    let (lhs, bit) = parse_lhs(p)?;
    if matches!(p.peek(), Some(Tok::Le)) {
        p.bump();
    } else if !p.eat_sym('=') {
        return Err("nba".into());
    }
    let rhs = parse_rexpr(p)?;
    let _ = p.eat_sym(';');
    Ok((lhs, bit, rhs))
}

fn parse_seq_block(p: &mut P, block: bool) -> Result<Vec<Nba>, String> {
    let mut v = Vec::new();
    loop {
        if block && p.eat_kw("end") {
            break;
        }
        if matches!(p.peek(), Some(Tok::Kw(k)) if k == "end") {
            p.eat_kw("end");
            break;
        }
        if p.peek().is_none() {
            break;
        }
        match parse_seq_item(p) {
            Ok(s) => v.extend(s),
            Err(_) => {
                let _ = skip_item_or_block(p);
            }
        }
        if !block {
            break;
        }
    }
    Ok(v)
}


/// Parse for-loop step: `i++`, `i += N`, or `i = i + N` (default step 1).
fn parse_for_step(p: &mut P) -> Result<usize, String> {
    let _var = p.ident()?;
    // i++
    if p.eat_sym('+') {
        if p.eat_sym('+') {
            return Ok(1);
        }
        // i += N
        if p.eat_sym('=') {
            let n = const_u(p)? as usize;
            return Ok(n.max(1));
        }
        return Err("for step +".into());
    }
    // i = i + N, or i = N + i (SRL `i = 1+i`).
    if p.eat_sym('=') {
        // i = N + i — do not call const_u: the loop var is not a parameter.
        if matches!(p.peek(), Some(Tok::Number(_, _))) {
            let n = match p.bump() {
                Some(Tok::Number(v, _)) => (*v as usize).max(1),
                _ => 1,
            };
            if p.eat_sym('+') {
                let _ = p.ident();
            }
            return Ok(n);
        }
        // i = i + N
        if matches!(p.peek(), Some(Tok::Ident(_))) {
            let _ = p.ident();
            if !p.eat_sym('+') {
                return Err("for step =+".into());
            }
            let n = match p.peek() {
                Some(Tok::Number(v, _)) => {
                    let n = (*v as usize).max(1);
                    p.bump();
                    n
                }
                _ => 1,
            };
            return Ok(n);
        }
        return Err("for step =".into());
    }
    Err("for step".into())
}

/// Procedural `mem[i] <= mem[i-1]` (optional begin/end). Not a generate.
fn body_is_word_shift(body: &[Tok], var: &str) -> Option<String> {
    let mut i = 0;
    if matches!(body.get(i), Some(Tok::Kw(k)) if k == "begin") {
        i += 1;
        if matches!(body.get(i), Some(Tok::Sym(':'))) {
            i += 1;
            if matches!(body.get(i), Some(Tok::Ident(_))) {
                i += 1;
            }
        }
    }
    let mem = match body.get(i) {
        Some(Tok::Ident(s)) => s.clone(),
        _ => return None,
    };
    i += 1;
    if !matches!(body.get(i), Some(Tok::Sym('['))) {
        return None;
    }
    let mut seen_le = false;
    let mut rhs_mem = false;
    let mut lhs_var = false;
    for t in body.iter().skip(i) {
        match t {
            Tok::Le => seen_le = true,
            Tok::Ident(s) if !seen_le && s == var => lhs_var = true,
            Tok::Ident(s) if seen_le && s == &mem => rhs_mem = true,
            Tok::Kw(k) if k == "end" => break,
            Tok::Sym(';') if seen_le && rhs_mem => break,
            _ => {}
        }
    }
    if lhs_var && rhs_mem {
        Some(mem)
    } else {
        None
    }
}

/// Bound is not a constant. Consume the for if the body is a word shift.
/// Caller emits `word_pipeline_cap` and does not invent stages.
fn consume_nonconst_word_pipeline(p: &mut P, var: &str) -> Option<String> {
    let save = p.i;
    let mut depth = 1i32;
    let mut closed = false;
    while p.peek().is_some() {
        if p.eat_sym('(') {
            depth += 1;
            continue;
        }
        if p.eat_sym(')') {
            depth -= 1;
            if depth == 0 {
                closed = true;
                break;
            }
            continue;
        }
        p.bump();
    }
    if !closed {
        p.i = save;
        return None;
    }
    let block = p.eat_kw("begin");
    if p.eat_sym(':') {
        let _ = p.ident();
    }
    let start_i = p.i;
    if block {
        let mut d = 1i32;
        while d > 0 {
            match p.bump() {
                Some(Tok::Kw(k)) if k == "begin" => d += 1,
                Some(Tok::Kw(k)) if k == "end" => d -= 1,
                None => {
                    p.i = save;
                    return None;
                }
                _ => {}
            }
        }
    } else {
        while p.peek().is_some() && !matches!(p.peek(), Some(Tok::Sym(';'))) {
            p.bump();
        }
        let _ = p.eat_sym(';');
    }
    let body = p.t[start_i..p.i].to_vec();
    match body_is_word_shift(&body, var) {
        Some(mem) => Some(mem),
        None => {
            p.i = save;
            None
        }
    }
}

fn parse_for_unroll(p: &mut P) -> Result<Vec<Nba>, String> {
    if !p.eat_sym('(') {
        return Err("for (".into());
    }
    let _ = p.eat_kw("int");
    let _ = p.eat_kw("genvar");
    let _ = p.eat_kw("unsigned");
    let _ = p.eat_kw("signed");
    let var = p.ident()?;
    if !p.eat_sym('=') {
        return Err("for =".into());
    }
    let start = const_u(p)? as usize;
    if !p.eat_sym(';') {
        return Err("for ;".into());
    }
    let _ = p.ident();
    let inclusive = if matches!(p.peek(), Some(Tok::Le)) {
        p.bump();
        true
    } else if p.eat_sym('<') {
        false
    } else {
        return Err("for cmp".into());
    };
    let bound_at = p.i;
    let end = match const_u(p) {
        Ok(v) => v as usize,
        Err(_) => {
            // `for (i = 1; i < cycles; ...)` when cycles is not a constant.
            // Do not invent the shift stages. One word_pipeline_cap.
            p.i = bound_at;
            if let Some(mem) = consume_nonconst_word_pipeline(p, &var) {
                note_word_pipeline_cap(&cur_mod(), &mem, "nonconst");
                return Ok(Vec::new());
            }
            return Err("for bound".into());
        }
    };
    // Inclusive `<=` used to `end + 1` and panic when the bound was usize::MAX.
    let end = if inclusive { end.saturating_add(1) } else { end };
    if !p.eat_sym(';') {
        return Err("for ;2".into());
    }
    let step = parse_for_step(p)?;
    if !p.eat_sym(')') {
        return Err("for )".into());
    }
    let block = p.eat_kw("begin");
    if p.eat_sym(':') {
        let _ = p.ident();
    }
    let start_i = p.i;
    if block {
        let mut depth = 1i32;
        while depth > 0 {
            match p.bump() {
                Some(Tok::Kw(k)) if k == "begin" => depth += 1,
                Some(Tok::Kw(k)) if k == "end" => depth -= 1,
                None => return Err("for body".into()),
                _ => {}
            }
        }
    } else {
        while p.peek().is_some() && !matches!(p.peek(), Some(Tok::Sym(';'))) {
            p.bump();
        }
        let _ = p.eat_sym(';');
    }
    let body = p.t[start_i..p.i].to_vec();
    let mut out = Vec::new();
    let niter = end.saturating_sub(start) / step.max(1);
    // `for (i = 1; i < cycles; ...)` : exclusive end is the word count.
    // Cap at 4 words. Do not unroll a deep chain into invented stages.
    if let Some(mem) = body_is_word_shift(&body, &var) {
        if end > 4 {
            note_word_pipeline_cap(&cur_mod(), &mem, &end.to_string());
        }
    }
    if niter > 4096 {
        return Ok(Vec::new());
    }
    let mut i = start;
    let mut n = 0usize;
    let step = step.max(1);
    while i < end && n < 4096 {
        let toks: Vec<Tok> = body
            .iter()
            .map(|t| match t {
                Tok::Ident(s) if s == &var => Tok::Number(i as u128, 32),
                other => other.clone(),
            })
            .collect();
        let mut sp = P { t: &toks, i: 0, params: p.params.clone(), widths: p.widths.clone() };
        out.extend(parse_seq_block(&mut sp, block)?);
        i = i.saturating_add(step);
        n += 1;
    }
    Ok(out)
}

fn parse_attr(p: &mut P) -> Option<(String, String)> {
    let save = p.i;
    if !p.eat_sym('(') {
        return None;
    }
    if !p.eat_sym('*') {
        p.i = save;
        return None;
    }
    let k = match p.bump() {
        Some(Tok::Ident(s) | Tok::Kw(s)) => s.clone(),
        _ => {
            p.i = save;
            return None;
        }
    };
    let _ = p.eat_sym('=');
    let v = match p.bump() {
        Some(Tok::Str(s) | Tok::Ident(s) | Tok::Kw(s)) => s.clone(),
        Some(Tok::Number(n, _)) => n.to_string(),
        _ => "1".into(),
    };
    let _ = p.eat_sym('*');
    let _ = p.eat_sym(')');
    Some((k, v))
}


/// FNV-1a tag so a string parameter does not compare equal to a small integer.
fn str_param_hash(s: &str) -> u128 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h as u128) | (1u128 << 96)
}

fn bit_extract(e: RExpr, bit: usize) -> RExpr {
    if bit == 0 {
        e
    } else {
        RExpr::Shr(
            Box::new(e),
            Box::new(RExpr::Const {
                val: bit as u128,
                width: 32,
                care: u128::MAX,
            }),
        )
    }
}

fn note_width(p: &mut P, name: &str, w: usize) {
    p.widths.insert(name.to_string(), w.max(1));
}

/// Last procedural write wins. Vector assigns become per-bit so a case arm
/// that writes slices can mux against an arm that writes the whole vector.
fn normalize_nbas(p: &P, stmts: Vec<Nba>) -> Vec<Nba> {
    let mut order: Vec<(String, Option<usize>)> = Vec::new();
    let mut map: HashMap<(String, Option<usize>), RExpr> = HashMap::new();
    let mut put = |order: &mut Vec<(String, Option<usize>)>,
                   map: &mut HashMap<(String, Option<usize>), RExpr>,
                   key: (String, Option<usize>),
                   rhs: RExpr| {
        if !map.contains_key(&key) {
            order.push(key.clone());
        }
        map.insert(key, rhs);
    };
    for (lhs, bit, rhs) in stmts {
        let w = p.widths.get(&lhs).copied().unwrap_or(1).min(256);
        match bit {
            Some(b) => {
                let full = (lhs.clone(), None);
                if map.remove(&full).is_some() {
                    order.retain(|k| k != &full);
                }
                put(&mut order, &mut map, (lhs, Some(b)), rhs);
            }
            None if matches!(rhs, RExpr::WordAt { .. }) => {
                // Whole-word `mem[addr] <= data`. Do not Shr-split into bits.
                put(&mut order, &mut map, (lhs, None), rhs);
            }
            None if w > 1 => {
                order.retain(|(n, _)| n != &lhs);
                map.retain(|(n, _), _| n != &lhs);
                for i in 0..w {
                    let key = (lhs.clone(), Some(i));
                    order.push(key.clone());
                    map.insert(key, bit_extract(rhs.clone(), i));
                }
            }
            None => put(&mut order, &mut map, (lhs, None), rhs),
        }
    }
    order
        .into_iter()
        .filter_map(|k| map.remove(&k).map(|rhs| (k.0, k.1, rhs)))
        .collect()
}

/// Blocking assign, including `name[hi:lo] = expr` as per-bit writes.
fn parse_assign_nbas(p: &mut P) -> Result<Vec<Nba>, String> {
    let name = p.ident()?;
    let mut range: Option<(usize, usize)> = None;
    let mut bit = None;
    let mut word_addr: Option<RExpr> = None;
    if p.eat_sym('[') {
        let save = p.i;
        let const_ok = const_u(p);
        if let Ok(hi) = const_ok {
            if p.eat_sym(':') {
                let lo = const_u(p)? as usize;
                if !p.eat_sym(']') {
                    return Err("]".into());
                }
                range = Some((hi as usize, lo));
            } else if p.eat_sym(']') {
                bit = Some(hi as usize);
            } else {
                p.i = save;
                word_addr = Some(parse_shift(p)?);
                if !p.eat_sym(']') {
                    return Err("]".into());
                }
            }
        } else {
            // `lut[addr_a]` — variable word index, not a bit-blast.
            p.i = save;
            word_addr = Some(parse_shift(p)?);
            if !p.eat_sym(']') {
                return Err("]".into());
            }
        }
    }
    if matches!(p.peek(), Some(Tok::Le)) {
        p.bump();
    } else if !p.eat_sym('=') {
        return Err("nba".into());
    }
    let rhs = parse_rexpr(p)?;
    let _ = p.eat_sym(';');
    if let Some(addr) = word_addr {
        return Ok(vec![(
            name,
            None,
            RExpr::WordAt {
                addr: Box::new(addr),
                data: Box::new(rhs),
            },
        )]);
    }
    if let Some((a, b)) = range {
        let hi = a.max(b);
        let lo = a.min(b);
        // A param that does not fit (rvfindfirst1 SHIFT) used to walk until
        // the kill after one width_overflow line. Name it and stop. Not a LUT.
        let span = match range_width(hi as u128, lo as u128) {
            Ok(w) if w <= 4096 && !width_overflow_for(&cur_mod()) => w,
            _ => {
                note_width_overflow();
                return Ok(Vec::new());
            }
        };
        let mut v = Vec::new();
        let mut k = 0usize;
        let mut i = lo;
        for _ in 0..span {
            v.push((name.clone(), Some(i), bit_extract(rhs.clone(), k)));
            k += 1;
            if i == usize::MAX {
                break;
            }
            i += 1;
        }
        return Ok(v);
    }
    Ok(vec![(name, bit, rhs)])
}

fn parse_seq_item(p: &mut P) -> Result<Vec<Nba>, String> {
    let _ = p.eat_kw("unique");
    let _ = p.eat_kw("priority");
    if p.eat_kw("for") {
        return parse_for_unroll(p);
    }
    if p.eat_kw("if") {
        if !p.eat_sym('(') {
            return Err("if (".into());
        }
        let cond = parse_rexpr(p)?;
        if !p.eat_sym(')') {
            return Err("if )".into());
        }
        let then_b = p.eat_kw("begin");
        let then_raw = parse_seq_block(p, then_b)?;
        let then_s = normalize_nbas(p, then_raw);
        let else_s = if p.eat_kw("else") {
            let eb = p.eat_kw("begin");
            let else_raw = parse_seq_block(p, eb)?;
            normalize_nbas(p, else_raw)
        } else {
            Vec::new()
        };
        // Parameter string compare (e.g. operation_mode == "dynamic") is
        // an elaboration constant — keep the taken branch, do not mux both.
        if let RExpr::Const { val, care, .. } = &cond {
            if *care & 1 == 1 {
                return Ok(if *val != 0 { then_s } else { else_s });
            }
        }
        let mut out = Vec::new();
        for (lhs, bit, rhs) in then_s {
            let other = else_s
                .iter()
                .find(|(l, b, _)| l == &lhs && b == &bit)
                .map(|(_, _, r)| r.clone())
                .unwrap_or_else(|| RExpr::Ident(lhs.clone()));
            out.push((lhs, bit, RExpr::Mux(Box::new(cond.clone()), Box::new(rhs), Box::new(other))));
        }
        for (lhs, bit, rhs) in else_s {
            if out.iter().any(|(l, b, _)| l == &lhs && b == &bit) {
                continue;
            }
            out.push((
                lhs.clone(),
                bit,
                RExpr::Mux(
                    Box::new(cond.clone()),
                    Box::new(RExpr::Ident(lhs.clone())),
                    Box::new(rhs),
                ),
            ));
        }
        return Ok(out);
    }
    if p.eat_kw("case") || p.eat_kw("casez") || p.eat_kw("casex") {
        if !p.eat_sym('(') {
            return Err("case (".into());
        }
        let sel = parse_rexpr(p)?;
        if !p.eat_sym(')') {
            return Err("case )".into());
        }
        let mut arms: Vec<(Option<RExpr>, Vec<Nba>)> = Vec::new();
        let mut def: Vec<Nba> = Vec::new();
        while !p.eat_kw("endcase") {
            if p.peek().is_none() {
                return Err("unterminated case".into());
            }
            if p.eat_kw("default") {
                let _ = p.eat_sym(':');
                let b = p.eat_kw("begin");
                let def_raw = parse_seq_block(p, b)?;
                def = normalize_nbas(p, def_raw);
                continue;
            }
            let item = parse_rexpr(p)?;
            if !p.eat_sym(':') {
                return Err("case :".into());
            }
            let b = p.eat_kw("begin");
            let body_raw = parse_seq_block(p, b)?;
            let body = normalize_nbas(p, body_raw);
            arms.push((Some(item), body));
        }
        let mut out = Vec::new();
        for (item, body) in arms.into_iter().rev() {
            let item = item.unwrap();
            let cond = RExpr::Eq(Box::new(sel.clone()), Box::new(item));
            for (lhs, bit, rhs) in body {
                let other = out
                    .iter()
                    .chain(def.iter())
                    .find(|(l, b, _)| l == &lhs && b == &bit)
                    .map(|(_, _, r)| r.clone())
                    .unwrap_or_else(|| RExpr::Ident(lhs.clone()));
                if let Some(existing) = out.iter_mut().find(|(l, b, _)| l == &lhs && b == &bit) {
                    existing.2 = RExpr::Mux(Box::new(cond.clone()), Box::new(rhs), Box::new(existing.2.clone()));
                } else {
                    out.push((lhs, bit, RExpr::Mux(Box::new(cond.clone()), Box::new(rhs), Box::new(other))));
                }
            }
        }
        for (lhs, bit, rhs) in def {
            if !out.iter().any(|(l, b, _)| l == &lhs && b == &bit) {
                out.push((lhs, bit, rhs));
            }
        }
        return Ok(out);
    }
    parse_assign_nbas(p)
}

fn skip_until_kw(p: &mut P, kw: &str) {
    while p.peek().is_some() {
        if p.eat_kw(kw) {
            return;
        }
        // Never walk past a module boundary looking for endfunction/endtask.
        if kw != "endmodule"
            && matches!(
                p.peek(),
                Some(Tok::Kw(k)) if k == "endmodule" || k == "endpackage" || k == "module"
            )
        {
            return;
        }
        let _ = p.bump();
    }
}

fn parse_source(source: &str) -> Result<Vec<Rtl>, String> {
    clear_seq_notes();
    let s = preprocess_sv(&strip_comments(source));
    let toks = tokenize(&s)?;
    let mut p = P { t: &toks, i: 0, params: HashMap::new(), widths: HashMap::new() };
    let mut mods = Vec::new();
    while p.peek().is_some() {
        if p.eat_sym(';') {
            continue;
        }
        if p.eat_kw("import") || p.eat_kw("export") {
            let _ = skip_item_or_block(&mut p);
            continue;
        }
        if p.eat_kw("package") {
            skip_until_kw(&mut p, "endpackage");
            continue;
        }
        if p.eat_kw("typedef") {
            let _ = skip_item_or_block(&mut p);
            continue;
        }
        if p.eat_kw("interface") {
            skip_until_kw(&mut p, "endinterface");
            continue;
        }
        if p.eat_kw("function") {
            skip_until_kw(&mut p, "endfunction");
            continue;
        }
        if p.eat_kw("task") {
            skip_until_kw(&mut p, "endtask");
            continue;
        }
        match p.peek() {
            Some(Tok::Kw(k)) if k == "module" => match parse_one_module(&mut p) {
                Ok(m) => mods.push(m),
                Err(_) => skip_until_kw(&mut p, "endmodule"),
            },
            _ => {
                // Never skip_item_or_block at file scope — that can walk past `module`.
                let _ = p.bump();
            }
        }
    }
    if mods.is_empty() {
        return Err("no module".into());
    }
    Ok(mods)
}

#[allow(dead_code)] // intentional: kept as direct RTL entry for future callers / tests
fn parse_rtl(source: &str) -> Result<Rtl, String> {
    parse_source(source)?
        .into_iter()
        .next()
        .ok_or_else(|| "no module".into())
}

fn parse_param_assigns(p: &mut P) -> Result<Vec<(String, u128)>, String> {
    let mut out = Vec::new();
    if !p.eat_sym('#') {
        return Ok(out);
    }
    if !p.eat_sym('(') {
        return Ok(out);
    }
    let mut pos = 0usize;
    loop {
        let i0 = p.i;
        if p.eat_sym(')') {
            break;
        }
        if p.peek().is_none() {
            break;
        }
        let _ = p.eat_kw("parameter");
        let _ = p.eat_kw("localparam");
        skip_sv_type(p);
        if p.eat_sym('.') {
            let name = match p.ident() {
                Ok(n) => n,
                Err(_) => {
                    skip_until_arg_end(p);
                    let _ = p.eat_sym(',');
                    continue;
                }
            };
            if p.eat_sym('(') {
                match const_u(p) {
                    Ok(val) => {
                        if p.eat_sym(')') {
                            out.push((name, val));
                        } else {
                            skip_until_arg_end(p);
                            let _ = p.eat_sym(')');
                        }
                    }
                    Err(_) => {
                        skip_until_arg_end(p);
                        let _ = p.eat_sym(')');
                    }
                }
            }
        } else if matches!(p.peek(), Some(Tok::Ident(_))) {
            let name = p.ident().unwrap();
            skip_sv_type(p);
            if p.eat_sym('=') {
                match const_u(p) {
                    Ok(val) => {
                        // Register immediately so later defaults can ref earlier ones
                        // (`OW = 2 * DW`). Existing overrides in p.params win.
                        let effective = p.params.get(&name).copied().unwrap_or(val);
                        p.params.insert(name.clone(), effective);
                        out.push((name, effective));
                    }
                    Err(_) => skip_until_arg_end(p),
                }
            } else {
                skip_until_arg_end(p);
            }
        } else {
            match const_u(p) {
                Ok(val) => {
                    out.push((format!("#{pos}"), val));
                    pos += 1;
                }
                Err(_) => skip_until_arg_end(p),
            }
        }
        let _ = p.eat_sym(',');
        if p.i == i0 {
            p.bump();
        }
    }
    Ok(out)
}

fn apply_params(p: &mut P, assigns: Vec<(String, u128)>, ordered: &mut Vec<(String, u128)>) {
    for (k, v) in assigns {
        let mut key = k.clone();
        let mut val = v;
        if let Some(rest) = k.strip_prefix('#') {
            if let Ok(i) = rest.parse::<usize>() {
                if let Some((name, _)) = ordered.get(i) {
                    key = name.clone();
                }
            }
        }
        // Overrides already in p.params win over defaults.
        if let Some(ov) = p.params.get(&key).copied() {
            val = ov;
        } else {
            p.params.insert(key.clone(), val);
        }
        if let Some(slot) = ordered.iter_mut().find(|(n, _)| n == &key) {
            slot.1 = val;
        } else {
            ordered.push((key, val));
        }
    }
}

fn skip_begin_end(p: &mut P) -> Result<(), String> {
    let mut depth = 1i32;
    while depth > 0 {
        match p.peek() {
            Some(Tok::Kw(k)) if k == "endmodule" || k == "endgenerate" || k == "endpackage" => {
                return Err("unterminated begin".into());
            }
            Some(Tok::Kw(k)) if k == "begin" => {
                p.bump();
                depth += 1;
            }
            Some(Tok::Kw(k)) if k == "end" => {
                p.bump();
                depth -= 1;
            }
            None => return Err("unterminated begin".into()),
            _ => {
                p.bump();
            }
        }
    }
    Ok(())
}

fn skip_item_or_block(p: &mut P) -> Result<(), String> {
    if p.eat_kw("begin") {
        if p.eat_sym(':') {
            let _ = p.ident();
        }
        return skip_begin_end(p);
    }
    let mut depth = 0i32;
    while p.peek().is_some() {
        if depth == 0 && p.eat_sym(';') {
            break;
        }
        match p.peek() {
            Some(Tok::Kw(k)) if k == "begin" => {
                p.bump();
                depth += 1;
            }
            Some(Tok::Kw(k)) if k == "end" => {
                if depth == 0 {
                    break;
                }
                p.bump();
                depth -= 1;
                if depth <= 0 {
                    break;
                }
            }
            Some(Tok::Kw(k)) if k == "endcase" || k == "endgenerate" || k == "endmodule" || k == "else" => {
                if depth == 0 || k == "endmodule" || k == "endgenerate" {
                    break;
                }
                p.bump();
            }
            None => break,
            _ => {
                p.bump();
            }
        }
    }
    Ok(())
}

fn skip_to_semi(p: &mut P) {
    let mut d = 0i32;
    while p.peek().is_some() {
        if d == 0 && p.eat_sym(';') {
            return;
        }
        match p.peek() {
            Some(Tok::Sym('(' | '[' | '{')) => {
                d += 1;
                p.bump();
            }
            Some(Tok::Sym(')' | ']' | '}')) => {
                // Stray closers (e.g. after a failed part-select parse) must not
                // abort before ';' — that desyncs the module scanner and drops
                // later always_ff / instances (Ibex timer, etc.).
                if d == 0 {
                    p.bump();
                    continue;
                }
                d -= 1;
                p.bump();
            }
            Some(Tok::Kw(k)) if d == 0
                && matches!(k.as_str(), "end" | "endmodule" | "endgenerate" | "endcase" | "else") =>
            {
                return;
            }
            None => return,
            _ => {
                p.bump();
            }
        }
    }
}

fn skip_brackets(p: &mut P) {
    if !p.eat_sym('[') {
        return;
    }
    let mut d = 1i32;
    while d > 0 && p.peek().is_some() {
        if p.eat_sym('[') {
            d += 1;
        } else if p.eat_sym(']') {
            d -= 1;
        } else {
            p.bump();
        }
    }
}

fn skip_until_arg_end(p: &mut P) {
    let mut d = 0i32;
    while p.peek().is_some() {
        if d == 0
            && matches!(
                p.peek(),
                Some(Tok::Sym(',')) | Some(Tok::Sym(')')) | Some(Tok::Sym(';'))
            )
        {
            return;
        }
        match p.peek() {
            Some(Tok::Sym('(' | '[' | '{')) => {
                d += 1;
                p.bump();
            }
            Some(Tok::Sym(')' | ']' | '}')) => {
                if d == 0 {
                    return;
                }
                d -= 1;
                p.bump();
            }
            None => return,
            _ => {
                p.bump();
            }
        }
    }
}

/// Skip `@...` sensitivity. Returns `(combo, edge clock, negedge_only)`.
/// Combo is `@*` / `@(*)` (no edge). Async `posedge clk or negedge nreset`
/// is treated like sync; the first edge name is the user's clock.
/// `negedge_only` is a lone falling edge — not mixed with posedge.
fn skip_event_control(p: &mut P) -> (bool, Option<String>, bool) {
    let _ = p.eat_sym('@');
    if p.eat_sym('*') {
        return (true, None, false);
    }
    if p.eat_sym('(') {
        let start = p.i;
        if p.eat_sym('*') && matches!(p.peek(), Some(Tok::Sym(')'))) {
            p.bump();
            return (true, None, false);
        }
        p.i = start;
        let mut d = 1i32;
        let mut has_posedge = false;
        let mut has_negedge = false;
        let mut clk = None;
        while d > 0 && p.peek().is_some() {
            if p.eat_kw("posedge") {
                has_posedge = true;
                if clk.is_none() {
                    if let Some(Tok::Ident(name)) = p.peek() {
                        clk = Some(name.clone());
                        p.bump();
                    }
                }
                continue;
            }
            if p.eat_kw("negedge") {
                has_negedge = true;
                if clk.is_none() {
                    if let Some(Tok::Ident(name)) = p.peek() {
                        clk = Some(name.clone());
                        p.bump();
                    }
                }
                continue;
            } else if p.eat_sym('(') {
                d += 1;
            } else if p.eat_sym(')') {
                d -= 1;
            } else {
                p.bump();
            }
        }
        let has_edge = has_posedge || has_negedge;
        return (!has_edge, clk, has_negedge && !has_posedge);
    }
    (false, None, false)
}

fn skip_for_rest(p: &mut P) {
    if p.eat_sym('(') {
        let mut d = 1i32;
        while d > 0 && p.peek().is_some() {
            if p.eat_sym('(') {
                d += 1;
            } else if p.eat_sym(')') {
                d -= 1;
            } else {
                p.bump();
            }
        }
    }
    if p.eat_kw("begin") {
        if p.eat_sym(':') {
            let _ = p.ident();
        }
        let _ = skip_begin_end(p);
    } else {
        let _ = skip_item_or_block(p);
    }
}

fn skip_sv_type(p: &mut P) {
    loop {
        let start = p.i;
        if p.eat_kw("bit")
            || p.eat_kw("int")
            || p.eat_kw("logic")
            || p.eat_kw("wire")
            || p.eat_kw("reg")
            || p.eat_kw("signed")
            || p.eat_kw("unsigned")
            || p.eat_kw("byte")
            || p.eat_kw("integer")
            || p.eat_kw("automatic")
            || p.eat_kw("const")
            || p.eat_kw("var")
            || p.eat_kw("packed")
            || p.eat_kw("struct")
            || p.eat_kw("enum")
        {
            continue;
        }
        if matches!(p.peek(), Some(Tok::Ident(_))) {
            let next = p.t.get(p.i + 1);
            if matches!(next, Some(Tok::Sym(':'))) {
                let _ = p.ident();
                let _ = p.eat_sym(':');
                let _ = p.eat_sym(':');
                if matches!(p.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
                    p.bump();
                }
                continue;
            }
            if matches!(next, Some(Tok::Ident(_)) | Some(Tok::Kw(_)) | Some(Tok::Sym('['))) {
                p.bump();
                continue;
            }
        }
        if matches!(p.peek(), Some(Tok::Sym('['))) {
            skip_brackets(p);
            continue;
        }
        if p.i == start {
            break;
        }
    }
}

#[allow(dead_code)] // intentional: parser helper retained for richer net-ref forms
fn parse_net_ref(p: &mut P) -> Result<String, String> {
    let name = p.ident()?;
    if p.eat_sym('[') {
        let idx = const_u(p)? as usize;
        if !p.eat_sym(']') {
            return Err("net ]".into());
        }
        return Ok(format!("{name}_{idx}"));
    }
    Ok(name)
}


#[derive(Clone, Debug)]
struct FuncDef {
    name: String,
    ret_w: usize,
    ports: Vec<(String, PortDir, usize)>,
    signals: Vec<(String, usize)>,
    stmts: Vec<Nba>,
}

/// Standing rule: a function is behavioral Verilog. Parse it so a call can
/// inline, or an otherwise empty module can lift it into ports + assigns.
fn parse_function(p: &mut P) -> Result<FuncDef, String> {
    skip_sv_type(p);
    let ret_w = match p.width_opt() {
        Ok(w) => w,
        Err(_) => 1,
    };
    let name = match p.ident() {
        Ok(n) => n,
        Err(_) => {
            skip_until_kw(p, "endfunction");
            return Err("function name".into());
        }
    };
    let _ = p.eat_sym(';');
    note_width(p, &name, ret_w);
    let mut ports = Vec::new();
    let mut signals = Vec::new();
    let mut stmts = Vec::new();
    while !p.eat_kw("endfunction") {
        if p.peek().is_none()
            || matches!(p.peek(), Some(Tok::Kw(k)) if k == "endmodule" || k == "module")
        {
            return Err("unterminated function".into());
        }
        if matches!(p.peek(), Some(Tok::Kw(k)) if k == "input" || k == "output" || k == "inout") {
            let dir = parse_port_dir(p).unwrap_or(PortDir::In);
            skip_sv_type(p);
            let w = p.width_opt().unwrap_or(1);
            if let Ok(n) = p.ident() {
                ports.push((n.clone(), dir, w));
                signals.push((n.clone(), w));
                note_width(p, &n, w);
            }
            let _ = p.eat_sym(';');
            continue;
        }
        if p.eat_kw("integer") {
            if let Ok(n) = p.ident() {
                signals.push((n.clone(), 32));
                note_width(p, &n, 32);
            }
            let _ = p.eat_sym(';');
            continue;
        }
        if p.eat_kw("reg") || p.eat_kw("wire") || p.eat_kw("logic") {
            let w = p.width_opt().unwrap_or(1);
            if let Ok(n) = p.ident() {
                signals.push((n.clone(), w));
                note_width(p, &n, w);
            }
            skip_to_semi(p);
            continue;
        }
        if p.eat_kw("begin") {
            if p.eat_sym(':') {
                let _ = p.ident();
            }
            match parse_seq_block(p, true) {
                Ok(s) => stmts.extend(normalize_nbas(p, s)),
                Err(_) => {
                    let _ = skip_begin_end(p);
                }
            }
            continue;
        }
        let save = p.i;
        match parse_assign_nbas(p) {
            Ok(v) => stmts.extend(normalize_nbas(p, v)),
            Err(_) => {
                p.i = save;
                let _ = skip_item_or_block(p);
            }
        }
        if p.i == save {
            let _ = p.bump();
        }
    }
    Ok(FuncDef {
        name,
        ret_w,
        ports,
        signals,
        stmts,
    })
}

fn rexpr_names(e: &RExpr, out: &mut HashSet<String>) {
    match e {
        RExpr::Ident(s) | RExpr::Bit(s, _) | RExpr::Range(s, _, _) => {
            out.insert(s.clone());
        }
        RExpr::IndexPart { name, base, .. } => {
            out.insert(name.clone());
            rexpr_names(base, out);
        }
        RExpr::WordAt { addr, data } => {
            rexpr_names(addr, out);
            rexpr_names(data, out);
        }
        RExpr::Concat(parts) => {
            for p in parts {
                rexpr_names(p, out);
            }
        }
        RExpr::Shr(a, b)
        | RExpr::Ashr(a, b)
        | RExpr::And(a, b)
        | RExpr::Or(a, b)
        | RExpr::Xor(a, b)
        | RExpr::Add(a, b)
        | RExpr::Sub(a, b)
        | RExpr::Mul(a, b)
        | RExpr::Eq(a, b)
        | RExpr::Ne(a, b)
        | RExpr::Lt(a, b) => {
            rexpr_names(a, out);
            rexpr_names(b, out);
        }
        RExpr::RedXor(x) | RExpr::RedAnd(x) | RExpr::RedOr(x) | RExpr::Not(x) => rexpr_names(x, out),
        RExpr::Mux(c, t, f) => {
            rexpr_names(c, out);
            rexpr_names(t, out);
            rexpr_names(f, out);
        }
        RExpr::Const { .. } => {}
    }
}

/// Empty module whose only Verilog is function(s): lift each function into
/// module ports + comb assigns. Not used when the module already has a body
/// (calls are inlined at the assign). Never invents a vendor architecture.

/// One-shot parse of functions in a function-only module (lift path).
fn parse_functions_in_slice(toks: &[Tok]) -> Vec<FuncDef> {
    let mut p = P {
        t: toks,
        i: 0,
        params: HashMap::new(),
        widths: HashMap::new(),
    };
    let mut funcs = Vec::new();
    while p.peek().is_some() {
        if p.eat_kw("function") {
            match parse_function(&mut p) {
                Ok(f) => funcs.push(f),
                Err(_) => skip_until_kw(&mut p, "endfunction"),
            }
        } else {
            let before = p.i;
            p.bump();
            if p.i == before {
                break;
            }
        }
    }
    funcs
}

fn lift_functions_if_no_body(
    ports: &mut Vec<(String, PortDir, usize)>,
    signals: &mut Vec<Signal>,
    nbas: &[Nba],
    assigns: &mut Vec<Nba>,
    insts: &[Inst],
    funcs: &[FuncDef],
) {
    if funcs.is_empty() || !nbas.is_empty() || !assigns.is_empty() || !insts.is_empty() {
        return;
    }
    let multi = funcs.len() > 1;
    let mut taken: HashSet<String> = HashSet::new();
    let claim = |taken: &mut HashSet<String>, name: String| -> String {
        if !taken.contains(&name) {
            taken.insert(name.clone());
            return name;
        }
        let mut i = 2u32;
        loop {
            let n = format!("{name}_{i}");
            if !taken.contains(&n) {
                taken.insert(n.clone());
                return n;
            }
            i += 1;
        }
    };
    for f in funcs {
        let mut ids: HashSet<String> = HashSet::new();
        ids.insert(f.name.clone());
        for (n, _, _) in &f.ports {
            ids.insert(n.clone());
        }
        for (n, _) in &f.signals {
            ids.insert(n.clone());
        }
        for (lhs, _, rhs) in &f.stmts {
            ids.insert(lhs.clone());
            rexpr_names(rhs, &mut ids);
        }
        let mut subst: HashMap<String, String> = HashMap::new();
        if multi {
            for id in &ids {
                if id == &f.name {
                    continue;
                }
                subst.insert(id.clone(), format!("{}_{id}", f.name));
            }
        }
        let mapped = |subst: &HashMap<String, String>, n: &str| {
            subst.get(n).cloned().unwrap_or_else(|| n.to_string())
        };
        if ports.is_empty() {
            for (n, dir, w) in &f.ports {
                let pn = claim(&mut taken, mapped(&subst, n));
                subst.insert(n.clone(), pn.clone());
                ports.push((pn.clone(), *dir, *w));
                if !signals.iter().any(|s| s.name == pn) {
                    signals.push(Signal {
                        name: pn,
                        width: *w,
                        depth: 0,
                        keep: false,
                        mark_debug: false,
                    });
                }
            }
            let on = claim(&mut taken, f.name.clone());
            subst.insert(f.name.clone(), on.clone());
            ports.push((on.clone(), PortDir::Out, f.ret_w));
            if !signals.iter().any(|s| s.name == on) {
                signals.push(Signal {
                    name: on,
                    width: f.ret_w,
                    depth: 0,
                    keep: false,
                    mark_debug: false,
                });
            }
        }
        for (n, w) in &f.signals {
            let pn = mapped(&subst, n);
            if !signals.iter().any(|s| s.name == pn) {
                signals.push(Signal {
                    name: pn,
                    width: *w,
                    depth: 0,
                    keep: false,
                    mark_debug: false,
                });
            }
        }
        for (lhs, bit, rhs) in &f.stmts {
            let ln = mapped(&subst, lhs);
            if !signals.iter().any(|s| s.name == ln) {
                signals.push(Signal {
                    name: ln.clone(),
                    width: 1,
                    depth: 0,
                    keep: false,
                    mark_debug: false,
                });
            }
            assigns.push((ln, *bit, rewrite_rexpr(rhs, &subst)));
        }
    }
}

fn parse_one_module(mut p: &mut P) -> Result<Rtl, String> {
    let tok_start = p.i;
    if !p.eat_kw("module") {
        return Err("expected module".into());
    }
    let module = p.ident()?;
    set_cur_mod(&module);
    if slice_is_sim_only(&p.t[p.i..]) {
        note_sim_only(&module);
    }
    while p.eat_kw("import") {
        let _ = skip_item_or_block(&mut p);
    }
    let mut param_order: Vec<(String, u128)> = Vec::new();
    let header_params = parse_param_assigns(&mut p)?;
    apply_params(&mut p, header_params, &mut param_order);
    let mut ports = Vec::new();
    let mut signals = Vec::new();
    if p.eat_sym('(') {
        loop {
            if p.eat_sym(')') {
                break;
            }
            if p.peek().is_none() {
                break;
            }
            let dir = parse_port_dir(&mut p).unwrap_or(PortDir::In);
            skip_logic(&mut p);
            // package::typedef or typedef name before the port ident
            while matches!(p.peek(), Some(Tok::Ident(_)))
                && matches!(p.t.get(p.i + 1), Some(Tok::Sym(':')))
            {
                let _ = p.ident();
                let _ = p.eat_sym(':');
                let _ = p.eat_sym(':');
            }
            if matches!(p.peek(), Some(Tok::Ident(_)))
                && matches!(p.t.get(p.i + 1), Some(Tok::Ident(_)) | Some(Tok::Sym('[')))
            {
                let _ = p.ident();
            }
            skip_logic(&mut p);
            let w = p.width_opt().unwrap_or(1);
            match p.ident() {
                Ok(n) => {
                    ports.push((n.clone(), dir, w));
                    signals.push(Signal {
                        name: n,
                        width: w,
                        depth: 0,
                        keep: false,
                        mark_debug: false,
                    });
                    while p.eat_sym('[') {
                        let mut d = 1i32;
                        while d > 0 && p.peek().is_some() {
                            if p.eat_sym('[') {
                                d += 1;
                            } else if p.eat_sym(']') {
                                d -= 1;
                            } else {
                                p.bump();
                            }
                        }
                    }
                }
                Err(_) => {
                    // skip this port to comma or ')'
                    let mut d = 0i32;
                    while p.peek().is_some() {
                        if d == 0 && (p.eat_sym(',') || matches!(p.peek(), Some(Tok::Sym(')')))) {
                            break;
                        }
                        match p.peek() {
                            Some(Tok::Sym('(')) => {
                                d += 1;
                                p.bump();
                            }
                            Some(Tok::Sym(')')) => {
                                if d == 0 {
                                    break;
                                }
                                d -= 1;
                                p.bump();
                            }
                            _ => {
                                p.bump();
                            }
                        }
                    }
                }
            }
            let _ = p.eat_sym(',');
        }
    }
    let _ = p.eat_sym(';');
    let mut nbas = Vec::new();
    let mut assigns = Vec::new();
    let mut insts = Vec::new();
    let mut pending_keep = false;
    let mut pending_md = false;
    let mut mem_inits: HashMap<String, BTreeMap<usize, u128>> = HashMap::new();
    let mut funcs: Vec<FuncDef> = Vec::new();
    skipped_funcs_clear();
    if parse_module_items(
        &mut p,
        &mut ports,
        &mut signals,
        &mut nbas,
        &mut assigns,
        &mut insts,
        &mut param_order,
        &mut pending_keep,
        &mut pending_md,
        &mut mem_inits,
        &mut funcs,
        "endmodule",
    )
    .is_err()
    {
        skip_until_kw(&mut p, "endmodule");
    }
    // Uncalled functions are skipped without re-entering the body.
    // Lift only a function-only module, and parse those functions once.
    let skipped = skipped_funcs_take();
    let will_lift = !skipped.is_empty() && nbas.is_empty() && assigns.is_empty() && insts.is_empty();
    if will_lift {
        funcs = parse_functions_in_slice(&p.t[tok_start..p.i]);
        lift_functions_if_no_body(&mut ports, &mut signals, &nbas, &mut assigns, &insts, &funcs);
    } else {
        let mut seen_fn = HashSet::new();
        for name in skipped {
            if seen_fn.insert(name.clone()) {
                note_function_not_called(&module, &name);
            }
        }
    }
    if sim_only_for(&module) {
        // A sim model is not a counter. Drop the leftover `assign cout = tmp_cout`
        // so it cannot become a LUT, a MAC, or a closed WNS.
        nbas.clear();
        assigns.clear();
        mem_inits.clear();
    }
    let toks = p.t[tok_start..p.i].to_vec();
    Ok(Rtl {
        module,
        ports,
        signals,
        nbas,
        assigns,
        insts,
        params: param_order,
        toks,
        mem_inits,
    })
}

/// `wire`/`logic name = expr` (assignment may start on the next line).
/// A `reg` initializer is not a continuous assign. An unparsed RHS is a named
/// diagnostic, not a folded 0.
fn push_decl_assign(
    p: &mut P,
    assigns: &mut Vec<(String, Option<usize>, RExpr)>,
    name: &str,
    net_assign: bool,
) {
    if !p.eat_sym('=') {
        return;
    }
    if !net_assign {
        skip_until_arg_end(p);
        return;
    }
    match parse_rexpr(p) {
        Ok(rhs) => assigns.push((name.to_string(), None, rhs)),
        Err(_) => {
            note_assign_not_lowered(&cur_mod(), name);
            skip_until_arg_end(p);
        }
    }
}

fn parse_module_items(
    mut p: &mut P,
    ports: &mut Vec<(String, PortDir, usize)>,
    signals: &mut Vec<Signal>,
    nbas: &mut Vec<Nba>,
    assigns: &mut Vec<(String, Option<usize>, RExpr)>,
    insts: &mut Vec<Inst>,
    param_order: &mut Vec<(String, u128)>,
    pending_keep: &mut bool,
    pending_md: &mut bool,
    mem_inits: &mut HashMap<String, BTreeMap<usize, u128>>,
    funcs: &mut Vec<FuncDef>,
    endkw: &str,
) -> Result<(), String> {
    for sig in signals.iter() {
        note_width(p, &sig.name, sig.width);
    }
    while !p.eat_kw(endkw) {
        if p.peek().is_none() {
            return Err(format!("unterminated until {endkw}"));
        }
        if matches!(p.peek(), Some(Tok::Kw(k)) if k == "end" || k == "endmodule" || k == "endgenerate" || k == "module") {
            break;
        }
        while let Some((k, v)) = parse_attr(&mut p) {
            let on = v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes");
            if k.eq_ignore_ascii_case("keep") || k.eq_ignore_ascii_case("dont_touch") {
                *pending_keep = on;
            }
            if k.eq_ignore_ascii_case("mark_debug") {
                *pending_md = on;
            }
        }
        if p.eat_kw("function") {
            // Do not parse the body. Uncalled functions are not LUTs; re-entering
            // for/if/case inside them loops (Ibex mhpmcounter_get / PMP).
            let name = skip_function_without_reentry(p);
            skipped_funcs_push(name);
            let _ = funcs;
            continue;
        }
        if p.eat_kw("task") {
            skip_until_kw(p, "endtask");
            continue;
        }
        if p.eat_kw("typedef") || p.eat_kw("import") || p.eat_kw("export") {
            let _ = skip_item_or_block(p);
            continue;
        }
        if p.eat_kw("generate") {
            parse_module_items(
                p, ports, signals, nbas, assigns, insts, param_order, pending_keep, pending_md, mem_inits, funcs,
                "endgenerate",
            )?;
            continue;
        }
        if p.eat_kw("endgenerate") {
            break;
        }
        if p.eat_kw("genvar") {
            let _ = p.ident();
            let _ = p.eat_sym(';');
            continue;
        }
        if p.eat_kw("parameter") || p.eat_kw("localparam") {
            // `parameter width = 16, cycles = 1` — every name is elaborated.
            // Skipping after the first used to leave `cycles` unknown, so
            // `res[(-1)+cycles]` looked like a signal index.
            loop {
                skip_sv_type(p);
                match p.ident() {
                    Ok(name) => {
                        if p.eat_sym('=') {
                            if let Some(Tok::Str(sval)) = p.peek() {
                                let h = str_param_hash(sval);
                                p.bump();
                                p.params.entry(name.clone()).or_insert(h);
                            } else {
                                match const_u(p) {
                                    Ok(val) => {
                                        p.params.entry(name.clone()).or_insert(val);
                                        if !param_order.iter().any(|(n, _)| n == &name) {
                                            param_order.push((name, val));
                                        }
                                    }
                                    Err(_) => skip_until_arg_end(p),
                                }
                            }
                        }
                        if p.eat_sym(',') {
                            continue;
                        }
                        skip_to_semi(p);
                        break;
                    }
                    Err(_) => {
                        skip_to_semi(p);
                        break;
                    }
                }
            }
            continue;
        }
        if p.eat_kw("if") {
            // Generate-if / else-if / else chain. Unknown const_cond → prefer else.
            let mut taken = false;
            loop {
                if !p.eat_sym('(') {
                    let _ = skip_item_or_block(p);
                    break;
                }
                // Unknown generate-if (e.g. package enums): prefer else/generic branch
                // so prim_flop → prim_generic_flop still maps real FFs.
                let yes = match const_cond(p) {
                    Ok(v) => v,
                    Err(_) => {
                        skip_until_arg_end(p);
                        false
                    }
                };
                if !p.eat_sym(')') {
                    skip_for_rest(p);
                    if p.eat_kw("else") {
                        skip_for_rest(p);
                    }
                    break;
                }
                let then_begin = p.eat_kw("begin");
                if p.eat_sym(':') {
                    let _ = p.ident();
                }
                if yes && !taken {
                    if then_begin {
                        parse_module_items(
                            p, ports, signals, nbas, assigns, insts, param_order, pending_keep, pending_md, mem_inits, funcs,
                            "end",
                        )?;
                    } else {
                        // one module item; require a following else/endgenerate/endmodule delimiter
                        parse_module_items(
                            p, ports, signals, nbas, assigns, insts, param_order, pending_keep, pending_md, mem_inits, funcs,
                            "else",
                        )?;
                        // parse_module_items consumed the else keyword — put it back
                        p.i -= 1;
                    }
                    taken = true;
                } else if then_begin {
                    skip_begin_end(p)?;
                } else {
                    skip_item_or_block(p)?;
                }
                if !p.eat_kw("else") {
                    break;
                }
                // `else if (...)` — recurse as next arm of the chain.
                if p.eat_kw("if") {
                    continue;
                }
                let eb = p.eat_kw("begin");
                if p.eat_sym(':') {
                    let _ = p.ident();
                }
                if !taken {
                    if eb {
                        parse_module_items(
                            p, ports, signals, nbas, assigns, insts, param_order, pending_keep, pending_md, mem_inits, funcs,
                            "end",
                        )?;
                    } else {
                        skip_item_or_block(p)?;
                    }
                } else if eb {
                    skip_begin_end(p)?;
                } else {
                    skip_item_or_block(p)?;
                }
                break;
            }
            continue;
        }
        if p.eat_kw("for") {
            // Unroll: NBA body and/or instantiations. Skip still-unsupported bounds.
            let save = p.i;
            match parse_for_unroll_module(p, signals, nbas, insts, assigns, mem_inits) {
                Ok(()) => continue,
                Err(_) => {
                    p.i = save;
                    match parse_for_unroll(p) {
                        Ok(v) => {
                            nbas.extend(v);
                            continue;
                        }
                        Err(_) => {
                            p.i = save;
                            skip_for_rest(p);
                            continue;
                        }
                    }
                }
            }
        }
        if matches!(p.peek(), Some(Tok::Kw(k)) if k == "input" || k == "output") {
            let dir = parse_port_dir(&mut p).unwrap();
            skip_logic(&mut p);
            let w = p.width_opt()?;
            let n = p.ident()?;
            if let Some(ex) = ports.iter_mut().find(|(pn, _, _)| pn == &n) {
                // Non-ANSI header listed the name with default dir/width 1.
                ex.1 = dir;
                ex.2 = w;
            } else {
                ports.push((n.clone(), dir, w));
            }
            if let Some(sig) = signals.iter_mut().find(|s| s.name == n) {
                sig.width = w;
            } else {
                signals.push(Signal {
                    name: n.clone(),
                    width: w,
                    depth: 0,
                    keep: false,
                    mark_debug: false,
                });
            }
            note_width(p, &n, w);
            // `input signed [W-1:0] a,b` — the same range applies to each name.
            while p.eat_sym(',') {
                let Ok(n2) = p.ident() else {
                    break;
                };
                if let Some(ex) = ports.iter_mut().find(|(pn, _, _)| pn == &n2) {
                    ex.1 = dir;
                    ex.2 = w;
                } else {
                    ports.push((n2.clone(), dir, w));
                }
                if let Some(sig) = signals.iter_mut().find(|s| s.name == n2) {
                    sig.width = w;
                } else {
                    signals.push(Signal {
                        name: n2.clone(),
                        width: w,
                        depth: 0,
                        keep: false,
                        mark_debug: false,
                    });
                }
                note_width(p, &n2, w);
            }
            let _ = p.eat_sym(';');
            continue;
        }
        let decl_kw = if p.eat_kw("logic") {
            "logic"
        } else if p.eat_kw("wire") {
            "wire"
        } else if p.eat_kw("reg") {
            "reg"
        } else {
            ""
        };
        if !decl_kw.is_empty() {
            // `wire` / `logic name = expr` is a net plus continuous assign.
            // Split across a newline (`wire\n name = expr`) is the same form.
            // `reg name = expr` stays an initializer, not a comb assign.
            let net_assign = decl_kw != "reg";
            // `reg signed [W-1:0] mem[N:0]` — signed is a qualifier, not the name.
            skip_logic(p);
            let w = match p.width_opt() {
                Ok(w) => w,
                Err(_) => {
                    skip_to_semi(p);
                    continue;
                }
            };
            let n = match p.ident() {
                Ok(n) => n,
                Err(_) => {
                    skip_to_semi(p);
                    continue;
                }
            };
            let mut depth = 0usize;
            if matches!(p.peek(), Some(Tok::Sym('['))) {
                let save = p.i;
                let _ = p.eat_sym('[');
                if let (Ok(hi), true, Ok(lo), true) =
                    (const_u(p), p.eat_sym(':'), const_u(p), p.eat_sym(']'))
                {
                    match range_width(hi, lo) {
                        Ok(w) => depth = w,
                        Err(_) => {
                            note_width_overflow();
                            depth = 0;
                        }
                    }
                } else {
                    p.i = save;
                    skip_brackets(p);
                }
            }
            if let Some(sig) = signals.iter_mut().find(|s| s.name == n) {
                if w > 1 {
                    sig.width = w;
                }
                if depth > 0 {
                    sig.depth = depth;
                }
            } else {
                signals.push(Signal {
                    name: n.clone(),
                    width: w,
                    depth,
                    keep: *pending_keep,
                    mark_debug: *pending_md,
                });
            }
            note_width(p, &n, w);
            push_decl_assign(p, assigns, &n, net_assign);
            while p.eat_sym(',') {
                if let Ok(n2) = p.ident() {
                    if !signals.iter().any(|s| s.name == n2) {
                        signals.push(Signal {
                            name: n2.clone(),
                            width: w,
                            depth: 0,
                            keep: *pending_keep,
                            mark_debug: *pending_md,
                        });
                    }
                    note_width(p, &n2, w);
                    if matches!(p.peek(), Some(Tok::Sym('['))) {
                        skip_brackets(p);
                    }
                    push_decl_assign(p, assigns, &n2, net_assign);
                } else {
                    break;
                }
            }
            *pending_keep = false;
            *pending_md = false;
            let _ = p.eat_sym(';');
            continue;
        }
        if p.eat_kw("initial") {
            let block = p.eat_kw("begin");
            loop {
                if block && p.eat_kw("end") {
                    break;
                }
                if p.peek().is_none() {
                    break;
                }
                match parse_nba(p) {
                    Ok((lhs, bit, rhs)) => {
                        if let RExpr::Const { val, width, .. } = rhs {
                            // A >128-bit initial does not fit the u128 const
                            // model. Do not invent a BRAM from the low slice.
                            if width > 128 {
                                note_wide_literal(width);
                            } else {
                                mem_inits
                                    .entry(lhs)
                                    .or_default()
                                    .insert(bit.unwrap_or(0), val);
                            }
                        }
                    }
                    Err(_) => {
                        skip_item_or_block(p)?;
                    }
                }
                if !block {
                    break;
                }
            }
            continue;
        }
        if p.eat_kw("assign") {
            if p.eat_sym('{') {
                let mut parts: Vec<(String, Option<usize>)> = Vec::new();
                loop {
                    if p.eat_sym('}') {
                        break;
                    }
                    match parse_lhs(p) {
                        Ok(x) => parts.push(x),
                        Err(_) => break,
                    }
                    let _ = p.eat_sym(',');
                }
                if p.eat_sym('=') {
                    match parse_rexpr(p) {
                        Ok(rhs) => {
                            let _ = p.eat_sym(';');
                            let widths: Vec<usize> = parts
                                .iter()
                                .map(|(n, bit)| {
                                    if bit.is_some() {
                                        1
                                    } else {
                                        signals
                                            .iter()
                                            .find(|s| s.name == *n)
                                            .map(|s| s.width)
                                            .or_else(|| {
                                                ports
                                                    .iter()
                                                    .find(|(pn, _, _)| pn == n)
                                                    .map(|(_, _, w)| *w)
                                            })
                                            .unwrap_or(1)
                                    }
                                })
                                .collect();
                            let total: usize = widths.iter().sum();
                            let mut bit_hi = total;
                            for ((n, bit), w) in parts.iter().zip(widths.iter()) {
                                bit_hi = bit_hi.saturating_sub(*w);
                                for i in 0..*w {
                                    let src_bit = bit_hi + i;
                                    let piece = if src_bit == 0 {
                                        rhs.clone()
                                    } else {
                                        RExpr::Shr(
                                            Box::new(rhs.clone()),
                                            Box::new(RExpr::Const {
                                                val: src_bit as u128,
                                                width: 32,
                                                care: u128::MAX,
                                            }),
                                        )
                                    };
                                    let lhs_bit = bit.or(if *w == 1 { None } else { Some(i) });
                                    assigns.push((n.clone(), lhs_bit, piece));
                                }
                            }
                        }
                        Err(_) => skip_to_semi(p),
                    }
                } else {
                    skip_to_semi(p);
                }
            } else {
                match (parse_lhs(&mut p), p.eat_sym('='), parse_rexpr(&mut p)) {
                    (Ok((lhs, bit)), true, Ok(rhs)) => {
                        let _ = p.eat_sym(';');
                        assigns.push((lhs, bit, rhs));
                    }
                    _ => {
                        note_generate_not_lowered(&cur_mod());
                        skip_to_semi(p);
                    }
                }
            }
            continue;
        }
        if p.eat_kw("always_comb") {
            if sim_only_for(&cur_mod()) {
                skip_procedural_body(p);
                continue;
            }
            let block = p.eat_kw("begin");
            if p.eat_sym(':') {
                let _ = p.ident();
            }
            match parse_seq_block(&mut p, block) {
                Ok(stmts) => assigns.extend(stmts),
                Err(_) => {
                    note_skip(format!(
                        "diagnostic skip_always module={} (always_comb body not parsed; not a LUT)",
                        cur_mod()
                    ));
                    let _ = skip_item_or_block(p);
                }
            }
            continue;
        }
        if p.eat_kw("always_ff") || p.eat_kw("always") || p.eat_kw("always_latch") {
            let (combo, edge_clk, negedge_only) = skip_event_control(p);
            if sim_only_for(&cur_mod()) {
                let _ = (combo, edge_clk, negedge_only);
                skip_procedural_body(p);
                continue;
            }
            let block = p.eat_kw("begin");
            if p.eat_sym(':') {
                let _ = p.ident();
            }
            let body_start = p.i;
            match parse_seq_block(&mut p, block) {
                Ok(stmts) => {
                    if combo {
                        // `always @(*)` next-state etc. → comb assigns, not FFs.
                        assigns.extend(stmts);
                    } else if negedge_only {
                        // Falling-edge always is its own clock. Do not fold it
                        // into the posedge cone (that drops the name).
                        let clk = edge_clk.unwrap_or_else(|| "clk".into());
                        if stmts.is_empty() {
                            let sig = seq_lhs_from_toks(&p.t[body_start..p.i]);
                            note_negedge_write(&cur_mod(), &clk, &sig, None, None);
                        } else {
                            let mut seen = HashSet::new();
                            for (lhs, bit, rhs) in &stmts {
                                if seen.insert(lhs.clone()) {
                                    note_negedge_write(
                                        &cur_mod(),
                                        &clk,
                                        lhs,
                                        *bit,
                                        Some(rhs.clone()),
                                    );
                                }
                            }
                        }
                    } else {
                        // Includes async `posedge clk or negedge rst` (sync-reset mux).
                        let clk = edge_clk.unwrap_or_else(|| "clk".into());
                        if stmts.is_empty() {
                            let sig = seq_lhs_from_toks(&p.t[body_start..p.i]);
                            note_seq_write(&cur_mod(), &clk, &sig);
                        } else {
                            let mut seen = HashSet::new();
                            for (lhs, _, _) in &stmts {
                                if seen.insert(lhs.clone()) {
                                    note_seq_write(&cur_mod(), &clk, lhs);
                                }
                            }
                        }
                        nbas.extend(stmts);
                    }
                }
                Err(_) => {
                    if negedge_only {
                        let clk = edge_clk.unwrap_or_else(|| "clk".into());
                        let end = p.i.min(p.t.len());
                        let sig = seq_lhs_from_toks(&p.t[body_start..end]);
                        note_negedge_write(&cur_mod(), &clk, &sig, None, None);
                    } else {
                        note_skip(format!(
                            "diagnostic skip_always module={} (always body not parsed; not a LUT)",
                            cur_mod()
                        ));
                        if !combo {
                            let clk = edge_clk.unwrap_or_else(|| "clk".into());
                            let end = p.i.min(p.t.len());
                            let sig = seq_lhs_from_toks(&p.t[body_start..end]);
                            note_seq_write(&cur_mod(), &clk, &sig);
                        }
                    }
                    let _ = skip_item_or_block(p);
                }
            }
            continue;
        }
        if p.eat_kw("function") {
            // Do not parse the body. Uncalled functions are not LUTs; re-entering
            // for/if/case inside them loops (Ibex mhpmcounter_get / PMP).
            let name = skip_function_without_reentry(p);
            skipped_funcs_push(name);
            let _ = funcs;
            continue;
        }
        if p.eat_kw("task") {
            skip_until_kw(p, "endtask");
            continue;
        }
        if p.eat_kw("typedef") || p.eat_kw("import") || p.eat_kw("export") {
            let _ = skip_item_or_block(p);
            continue;
        }
        if matches!(p.peek(), Some(Tok::Ident(_))) {
            if parse_gate_prim(&mut p).is_ok() {
                // Consumed. Not an assign, not an unknown child, not a LUT.
                continue;
            }
            if let Ok(inst) = parse_inst(&mut p) {
                insts.push(inst);
                continue;
            }
        }
        let before = p.i;
        let _ = skip_item_or_block(p);
        if p.i == before && p.peek().is_some() {
            let _ = p.bump();
        }
    }
    Ok(())
}

fn parse_for_unroll_module(
    p: &mut P,
    signals: &mut Vec<Signal>,
    nbas: &mut Vec<Nba>,
    insts: &mut Vec<Inst>,
    assigns: &mut Vec<(String, Option<usize>, RExpr)>,
    mem_inits: &mut HashMap<String, BTreeMap<usize, u128>>,
) -> Result<(), String> {
    if !p.eat_sym('(') {
        return Err("for (".into());
    }
    let _ = p.eat_kw("int");
    let _ = p.eat_kw("genvar");
    let _ = p.eat_kw("unsigned");
    let _ = p.eat_kw("signed");
    let var = p.ident()?;
    if !p.eat_sym('=') {
        return Err("for =".into());
    }
    let start = const_u(p)? as usize;
    if !p.eat_sym(';') {
        return Err("for ;".into());
    }
    let _ = p.ident();
    let inclusive = if matches!(p.peek(), Some(Tok::Le)) {
        p.bump();
        true
    } else if p.eat_sym('<') {
        false
    } else {
        return Err("for cmp".into());
    };
    let end = const_u(p)? as usize;
    let end = if inclusive { end.saturating_add(1) } else { end };
    if !p.eat_sym(';') {
        return Err("for ;2".into());
    }
    let step = parse_for_step(p)?;
    if !p.eat_sym(')') {
        return Err("for )".into());
    }
    let block = p.eat_kw("begin");
    if p.eat_sym(':') {
        let _ = p.ident();
    }
    let start_i = p.i;
    if block {
        skip_begin_end(p)?;
    } else {
        while p.peek().is_some() && !matches!(p.peek(), Some(Tok::Sym(';'))) {
            p.bump();
        }
        let _ = p.eat_sym(';');
    }
    let body = p.t[start_i..p.i].to_vec();
    let niter = end.saturating_sub(start) / step.max(1);
    if niter > 4096 {
        return Ok(());
    }
    let mut i = start;
    while i < end {
        let toks: Vec<Tok> = body
            .iter()
            .map(|t| match t {
                Tok::Ident(s) if s == &var => Tok::Number(i as u128, 32),
                other => other.clone(),
            })
            .collect();
        let mut sp = P {
            t: &toks,
            i: 0,
            params: p.params.clone(),
            widths: p.widths.clone(),
        };
        let mut dummy_ports = Vec::new();
        let mut dummy_sigs = Vec::new();
        let mut dummy_params = Vec::new();
        let mut pk = false;
        let mut pmd = false;
        let mut local_nbas = Vec::new();
        let mut local_assigns = Vec::new();
        let mut local_insts = Vec::new();
        let mut dummy_funcs: Vec<FuncDef> = Vec::new();
        if block {
            parse_module_items(
                &mut sp,
                &mut dummy_ports,
                &mut dummy_sigs,
                &mut local_nbas,
                &mut local_assigns,
                &mut local_insts,
                &mut dummy_params,
                &mut pk,
                &mut pmd,
                mem_inits,
                &mut dummy_funcs,
                "end",
            )?;
        } else if parse_gate_prim(&mut sp).is_ok() {
            // Generate-for of a gate primitive is not an instance and not a LUT.
        } else if let Ok(inst) = parse_inst(&mut sp) {
            local_insts.push(inst);
        } else {
            local_nbas.extend(parse_seq_block(&mut sp, false)?);
        }
        // Local decls inside the generate-for must be uniquified per iteration
        // so vector widths survive and each copy maps to its own FF bits.
        let mut subst: HashMap<String, String> = HashMap::new();
        for mut sig in dummy_sigs {
            let fresh = format!("{}_{i}", sig.name);
            subst.insert(sig.name.clone(), fresh.clone());
            sig.name = fresh;
            if !signals.iter().any(|s| s.name == sig.name) {
                signals.push(sig);
            }
        }
        for mut inst in local_insts {
            inst.name = format!("{}_{i}", inst.name);
            for (port, net) in inst.conns.iter_mut() {
                let _ = port;
                if let Some(n) = subst.get(net) {
                    *net = n.clone();
                }
            }
            insts.push(inst);
        }
        for (lhs, bit, rhs) in local_nbas {
            let lhs = subst.get(&lhs).cloned().unwrap_or(lhs);
            nbas.push((lhs, bit, rewrite_rexpr(&rhs, &subst)));
        }
        for (lhs, bit, rhs) in local_assigns {
            let lhs = subst.get(&lhs).cloned().unwrap_or(lhs);
            assigns.push((lhs, bit, rewrite_rexpr(&rhs, &subst)));
        }
        i += step;
    }
    Ok(())
}

fn parse_inst_net(p: &mut P) -> Option<String> {
    while p.eat_sym('!') || p.eat_sym('~') {}
    if matches!(p.peek(), Some(Tok::Sym(')'))) {
        return None;
    }
    if p.eat_sym('{') {
        let mut d = 1i32;
        while d > 0 && p.peek().is_some() {
            if p.eat_sym('{') {
                d += 1;
            } else if p.eat_sym('}') {
                d -= 1;
            } else {
                p.bump();
            }
        }
        return None;
    }
    if matches!(p.peek(), Some(Tok::Number(_, _)) | Some(Tok::Pat { .. })) {
        p.bump();
        skip_until_arg_end(p);
        return None;
    }
    if matches!(p.peek(), Some(Tok::Ident(_))) {
        let mut name = p.ident().ok()?;
        if p.eat_sym(':') {
            let _ = p.eat_sym(':');
            if let Ok(n) = p.ident() {
                name = n;
            }
        }
        if p.eat_sym('[') {
            let mut d = 1i32;
            while d > 0 && p.peek().is_some() {
                if p.eat_sym('[') {
                    d += 1;
                } else if p.eat_sym(']') {
                    d -= 1;
                } else {
                    p.bump();
                }
            }
        }
        skip_until_arg_end(p);
        return Some(name);
    }
    skip_until_arg_end(p);
    None
}

fn is_gate_prim(s: &str) -> bool {
    matches!(
        s,
        "and"
            | "nand"
            | "or"
            | "nor"
            | "xor"
            | "xnor"
            | "not"
            | "buf"
            | "bufif0"
            | "bufif1"
            | "notif0"
            | "notif1"
    )
}

fn skip_balanced_paren(p: &mut P) -> bool {
    if !p.eat_sym('(') {
        return false;
    }
    let mut d = 1i32;
    while d > 0 && p.peek().is_some() {
        if p.eat_sym('(') {
            d += 1;
        } else if p.eat_sym(')') {
            d -= 1;
        } else {
            p.bump();
        }
    }
    d == 0
}

/// Verilog gate primitive. Consume the statement. Do not invent a LUT mux
/// and do not leave a child named `bufif1` for flatten/assemble to reprint.
fn parse_gate_prim(p: &mut P) -> Result<(), String> {
    let start = p.i;
    let kind = match p.peek() {
        Some(Tok::Ident(s)) if is_gate_prim(s) => s.clone(),
        _ => return Err("not a gate".into()),
    };
    p.bump();
    if p.eat_sym('#') {
        if matches!(p.peek(), Some(Tok::Sym('('))) {
            if !skip_balanced_paren(p) {
                p.i = start;
                return Err("gate delay".into());
            }
        } else {
            p.bump();
        }
    }
    let mut saw_list = false;
    if matches!(p.peek(), Some(Tok::Sym('('))) {
        if !skip_balanced_paren(p) {
            p.i = start;
            return Err("gate (".into());
        }
        saw_list = true;
    }
    if matches!(p.peek(), Some(Tok::Ident(_))) {
        let _ = p.ident();
        if p.eat_sym('[') {
            let mut d = 1i32;
            while d > 0 && p.peek().is_some() {
                if p.eat_sym('[') {
                    d += 1;
                } else if p.eat_sym(']') {
                    d -= 1;
                } else {
                    p.bump();
                }
            }
        }
    }
    if matches!(p.peek(), Some(Tok::Sym('('))) {
        if !skip_balanced_paren(p) {
            p.i = start;
            return Err("gate ports".into());
        }
        saw_list = true;
    }
    while p.eat_sym(',') {
        if matches!(p.peek(), Some(Tok::Ident(_))) {
            let _ = p.ident();
        }
        if !skip_balanced_paren(p) {
            p.i = start;
            return Err("gate inst".into());
        }
        saw_list = true;
    }
    if !saw_list || !p.eat_sym(';') {
        p.i = start;
        return Err("gate ;".into());
    }
    note_gate_primitive(&cur_mod(), &kind);
    Ok(())
}

fn parse_inst(p: &mut P) -> Result<Inst, String> {
    let start = p.i;
    let module = match p.bump() {
        Some(Tok::Ident(s)) => s.clone(),
        _ => {
            p.i = start;
            return Err("inst module".into());
        }
    };
    let params = match parse_param_assigns(p) {
        Ok(v) => v,
        Err(_) => {
            p.i = start;
            return Err("inst params".into());
        }
    };
    let name = match p.bump() {
        Some(Tok::Ident(s)) => s.clone(),
        _ => {
            p.i = start;
            return Err("inst name".into());
        }
    };
    if !p.eat_sym('(') {
        p.i = start;
        return Err("inst (".into());
    }
    let mut conns = Vec::new();
    let mut pos = 0usize;
    loop {
        let i0 = p.i;
        if p.eat_sym(')') {
            break;
        }
        if p.peek().is_none() {
            break;
        }
        if p.eat_sym('.') {
            let port = match p.ident() {
                Ok(n) => n,
                Err(_) => {
                    skip_until_arg_end(p);
                    let _ = p.eat_sym(',');
                    continue;
                }
            };
            if p.eat_sym('(') {
                if let Some(net) = parse_inst_net(p) {
                    conns.push((port, net));
                }
                let _ = p.eat_sym(')');
            } else {
                conns.push((port.clone(), port));
            }
        } else if let Some(net) = parse_inst_net(p) {
            conns.push((format!("#{pos}"), net));
            pos += 1;
        }
        let _ = p.eat_sym(',');
        if p.i == i0 {
            p.bump();
        }
    }
    let _ = p.eat_sym(';');
    Ok(Inst {
        module,
        name,
        conns,
        params,
    })
}

fn sig_width(rtl: &Rtl, name: &str) -> usize {
    rtl.signals
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.width)
        .or_else(|| rtl.ports.iter().find(|(n, _, _)| n == name).map(|(_, _, w)| *w))
        .unwrap_or(1)
}

fn bit_name(sig: &str, width: usize, bit: usize) -> String {
    if width == 1 {
        sig.to_string()
    } else {
        format!("{sig}_{bit}")
    }
}

fn inc_bit_expr(sig: &str, width: usize, i: usize) -> Expr {
    let qi = Expr::Var(bit_name(sig, width, i));
    if i == 0 {
        return Expr::Not(Box::new(qi));
    }
    let mut and = Expr::Var(bit_name(sig, width, 0));
    for j in 1..i {
        and = Expr::And(
            Box::new(and),
            Box::new(Expr::Var(bit_name(sig, width, j))),
        );
    }
    Expr::Xor(Box::new(and), Box::new(qi))
}

fn rexpr_is_plus_one(e: &RExpr) -> bool {
    match e {
        RExpr::Add(_, r) => matches!(r.as_ref(), RExpr::Const { val, .. } if *val == 1),
        _ => false,
    }
}

fn rexpr_ident(e: &RExpr) -> Option<String> {
    match e {
        RExpr::Ident(s) => Some(s.clone()),
        RExpr::Add(l, _) => rexpr_ident(l),
        _ => None,
    }
}

fn index_part_bit(
    name: &str,
    base: &RExpr,
    part_w: usize,
    ascending: bool,
    rtl: &Rtl,
    bit: usize,
) -> Result<Expr, String> {
    if bit >= part_w {
        return Ok(Expr::Const(false));
    }
    let wsrc = sig_width(rtl, name);
    if let RExpr::Const { val, .. } = base {
        let idx = if ascending {
            (*val as usize).saturating_add(bit)
        } else {
            (*val as usize).saturating_sub(bit)
        };
        return Ok(Expr::Var(bit_name(
            name,
            wsrc,
            idx.min(wsrc.saturating_sub(1)),
        )));
    }
    // sel * stride  → N-way mux of data[k*stride + bit]
    let (sel, stride) = match base {
        RExpr::Mul(a, b) => match (a.as_ref(), b.as_ref()) {
            (RExpr::Const { val, .. }, other) => (other, (*val as usize).max(1)),
            (other, RExpr::Const { val, .. }) => (other, (*val as usize).max(1)),
            _ => return Err("index mul needs const stride".into()),
        },
        other => {
            // width-1 dynamic bit: mux over possible indices 0..min(wsrc,16)
            let n = wsrc.min(16).max(1);
            let mut acc: Option<Expr> = None;
            for k in 0..n {
                let eq = cmp_eq_bits(
                    other,
                    &RExpr::Const {
                        val: k as u128,
                        width: 32,
                        care: u128::MAX,
                    },
                    rtl,
                    true,
                )?;
                let src = Expr::Var(bit_name(name, wsrc, k));
                let term = Expr::And(Box::new(eq), Box::new(src));
                acc = Some(match acc {
                    None => term,
                    Some(a) => Expr::Or(Box::new(a), Box::new(term)),
                });
            }
            return Ok(acc.unwrap_or(Expr::Const(false)));
        }
    };
    let n = (wsrc / stride).max(1).min(64);
    let mut acc: Option<Expr> = None;
    for k in 0..n {
        let eq = cmp_eq_bits(
            sel,
            &RExpr::Const {
                val: k as u128,
                width: 32,
                care: u128::MAX,
            },
            rtl,
            true,
        )?;
        let src_idx = if ascending {
            k * stride + bit
        } else {
            k * stride + (part_w - 1 - bit)
        };
        let src = Expr::Var(bit_name(name, wsrc, src_idx.min(wsrc.saturating_sub(1))));
        let term = Expr::And(Box::new(eq), Box::new(src));
        acc = Some(match acc {
            None => term,
            Some(a) => Expr::Or(Box::new(a), Box::new(term)),
        });
    }
    Ok(acc.unwrap_or(Expr::Const(false)))
}

thread_local! {
    static REXPR_BIT_BUDGET: std::cell::Cell<u32> = const { std::cell::Cell::new(u32::MAX) };
}

fn rexpr_bit_budget_reset(limit: u32) {
    REXPR_BIT_BUDGET.with(|c| c.set(limit));
}

fn rexpr_bit_budget_take() -> bool {
    REXPR_BIT_BUDGET.with(|c| {
        let v = c.get();
        if v == 0 {
            false
        } else {
            c.set(v - 1);
            true
        }
    })
}

fn rexpr_to_bit(e: &RExpr, rtl: &Rtl, bit: usize) -> Result<Expr, String> {
    if !rexpr_bit_budget_take() {
        return Err("rexpr_to_bit budget exhausted".into());
    }
    match e {
        RExpr::Const { val, width, care } => {
            let _ = width;
            if bit >= 128 {
                return Ok(Expr::Const(false));
            }
            if (care >> bit) & 1 == 0 {
                return Ok(Expr::Const(false));
            }
            Ok(Expr::Const((val >> bit) & 1 == 1))
        }
        RExpr::Ident(s) => {
            let w = sig_width(rtl, s);
            Ok(Expr::Var(bit_name(s, w, bit.min(w.saturating_sub(1)))))
        }
        RExpr::Bit(s, i) => {
            let w = sig_width(rtl, s);
            Ok(Expr::Var(bit_name(s, w, *i)))
        }
        RExpr::Range(s, lo, hi) => {
            if range_span(*lo, *hi).is_none() {
                note_width_overflow();
                return Err("width_overflow".into());
            }
            let w = sig_width(rtl, s);
            let idx = lo.saturating_add(bit).min(*hi).min(w.saturating_sub(1));
            Ok(Expr::Var(bit_name(s, w, idx)))
        }
        RExpr::IndexPart {
            name,
            base,
            width,
            ascending,
        } => index_part_bit(name, base, *width, *ascending, rtl, bit),
        RExpr::WordAt { .. } => Err("word write is not a comb cone".into()),
        RExpr::Concat(parts) => {
            let mut offset = 0usize;
            for part in parts.iter().rev() {
                let Some(w) = rexpr_width_checked(part, rtl) else {
                    note_width_overflow();
                    return Err("width_overflow".into());
                };
                let w = w.max(1);
                if bit < offset.saturating_add(w) {
                    return rexpr_to_bit(part, rtl, bit - offset);
                }
                offset = offset.saturating_add(w);
            }
            Ok(Expr::Const(false))
        }
        RExpr::Shr(a, sh) => {
            let shift = match sh.as_ref() {
                RExpr::Const { val, .. } => *val as usize,
                _ => return Err("shift amount not const".into()),
            };
            let wa = rexpr_width(a, rtl);
            if bit.saturating_add(shift) >= wa {
                Ok(Expr::Const(false))
            } else {
                rexpr_to_bit(a, rtl, bit + shift)
            }
        }
        RExpr::Ashr(a, sh) => {
            let shift = match sh.as_ref() {
                RExpr::Const { val, .. } => *val as usize,
                _ => return Err("ashr amount not const".into()),
            };
            let wa = rexpr_width(a, rtl).max(1);
            let src = bit.saturating_add(shift);
            if src >= wa {
                // Sign-fill from MSB.
                rexpr_to_bit(a, rtl, wa - 1)
            } else {
                rexpr_to_bit(a, rtl, src)
            }
        }
        RExpr::RedXor(x) => {
            if bit != 0 {
                return Ok(Expr::Const(false));
            }
            let Some(w) = rexpr_width_checked(x, rtl) else {
                note_width_overflow();
                return Err("width_overflow".into());
            };
            let w = w.max(1);
            if w > 128 {
                return Err("reduction width too wide".into());
            }
            let mut acc = rexpr_to_bit(x, rtl, 0)?;
            for i in 1..w {
                acc = Expr::Xor(Box::new(acc), Box::new(rexpr_to_bit(x, rtl, i)?));
            }
            Ok(acc)
        }
        RExpr::RedAnd(x) => {
            if bit != 0 {
                return Ok(Expr::Const(false));
            }
            let Some(w) = rexpr_width_checked(x, rtl) else {
                note_width_overflow();
                return Err("width_overflow".into());
            };
            let w = w.max(1);
            if w > 128 {
                return Err("reduction width too wide".into());
            }
            let mut acc = rexpr_to_bit(x, rtl, 0)?;
            for i in 1..w {
                acc = Expr::And(Box::new(acc), Box::new(rexpr_to_bit(x, rtl, i)?));
            }
            Ok(acc)
        }
        RExpr::RedOr(x) => {
            if bit != 0 {
                return Ok(Expr::Const(false));
            }
            let Some(w) = rexpr_width_checked(x, rtl) else {
                note_width_overflow();
                return Err("width_overflow".into());
            };
            let w = w.max(1);
            if w > 128 {
                return Err("reduction width too wide".into());
            }
            let mut acc = rexpr_to_bit(x, rtl, 0)?;
            for i in 1..w {
                acc = Expr::Or(Box::new(acc), Box::new(rexpr_to_bit(x, rtl, i)?));
            }
            Ok(acc)
        }
        RExpr::Not(x) => Ok(Expr::Not(Box::new(rexpr_to_bit(x, rtl, bit)?))),
        RExpr::And(a, b) => Ok(Expr::And(
            Box::new(rexpr_to_bit(a, rtl, bit)?),
            Box::new(rexpr_to_bit(b, rtl, bit)?),
        )),
        RExpr::Or(a, b) => Ok(Expr::Or(
            Box::new(rexpr_to_bit(a, rtl, bit)?),
            Box::new(rexpr_to_bit(b, rtl, bit)?),
        )),
        RExpr::Xor(a, b) => Ok(Expr::Xor(
            Box::new(rexpr_to_bit(a, rtl, bit)?),
            Box::new(rexpr_to_bit(b, rtl, bit)?),
        )),
        RExpr::Add(a, b) => adder_sum_bit(a, b, rtl, bit),
        RExpr::Sub(a, b) => sub_diff_bit(a, b, rtl, bit),
        RExpr::Mul(_, _) => Err("mul is a DSP primitive, not a LUT cone".into()),
        RExpr::Mux(c, t, f) => {
            let cv = rexpr_to_bit(c, rtl, 0)?;
            let tv = rexpr_to_bit(t, rtl, bit)?;
            let fv = rexpr_to_bit(f, rtl, bit)?;
            Ok(Expr::Or(
                Box::new(Expr::And(Box::new(cv.clone()), Box::new(tv))),
                Box::new(Expr::And(Box::new(Expr::Not(Box::new(cv))), Box::new(fv))),
            ))
        }
        RExpr::Eq(a, b) => {
            if bit != 0 {
                return Ok(Expr::Const(false));
            }
            Ok(cmp_eq_bits(a, b, rtl, true)?)
        }
        RExpr::Ne(a, b) => {
            if bit != 0 {
                return Ok(Expr::Const(false));
            }
            Ok(Expr::Not(Box::new(cmp_eq_bits(a, b, rtl, true)?)))
        }
        RExpr::Lt(a, b) => {
            if bit != 0 {
                return Ok(Expr::Const(false));
            }
            // unsigned: MSB-first: (a_msb < b_msb) | (eq_msb & lower)
            lt_bits(a, b, rtl)
        }
    }
}


/// Truth-table a boolean cone with ≤6 PIs. Linear in the AST, so a huge
/// nested assign of a few inputs (Problem4) maps without lifting the Ibex
/// AIG/add hang guard on wide cones.
fn lut6_from_bool_cone(expr: &Expr) -> Option<(u64, Vec<String>)> {
    fn collect(e: &Expr, vars: &mut Vec<String>, budget: &mut usize) -> bool {
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        match e {
            Expr::Const(_) => true,
            Expr::Var(name) => {
                if !vars.iter().any(|v| v == name) {
                    if vars.len() >= 6 {
                        return false;
                    }
                    vars.push(name.clone());
                }
                true
            }
            Expr::Not(x) => collect(x, vars, budget),
            Expr::And(a, b) | Expr::Or(a, b) | Expr::Xor(a, b) => {
                collect(a, vars, budget) && collect(b, vars, budget)
            }
        }
    }
    fn eval(e: &Expr, env: &[(&str, bool)], budget: &mut usize) -> Option<bool> {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        match e {
            Expr::Const(b) => Some(*b),
            Expr::Var(name) => env.iter().find(|(n, _)| *n == name.as_str()).map(|(_, b)| *b),
            Expr::Not(x) => Some(!eval(x, env, budget)?),
            Expr::And(a, b) => Some(eval(a, env, budget)? && eval(b, env, budget)?),
            Expr::Or(a, b) => Some(eval(a, env, budget)? || eval(b, env, budget)?),
            Expr::Xor(a, b) => Some(eval(a, env, budget)? ^ eval(b, env, budget)?),
        }
    }
    let mut vars = Vec::new();
    let mut budget = 64_000usize;
    if !collect(expr, &mut vars, &mut budget) {
        return None;
    }
    let n = vars.len();
    let mut init = 0u64;
    for addr in 0..64u64 {
        let mut env: Vec<(&str, bool)> = Vec::with_capacity(n);
        for (i, v) in vars.iter().enumerate() {
            env.push((v.as_str(), (addr >> i) & 1 == 1));
        }
        let mut b = 80_000usize;
        let bit = eval(expr, &env, &mut b)?;
        if bit {
            init |= 1u64 << addr;
        }
    }
    Some((init, vars))
}

fn expr_node_count(e: &Expr) -> usize {
    fn walk(e: &Expr, budget: &mut usize) -> usize {
        if *budget == 0 {
            return 0;
        }
        *budget -= 1;
        match e {
            Expr::Const(_) | Expr::Var(_) => 1,
            Expr::Not(x) => 1 + walk(x, budget),
            Expr::And(a, b) | Expr::Or(a, b) | Expr::Xor(a, b) => {
                1 + walk(a, budget) + walk(b, budget)
            }
        }
    }
    let mut budget = 16_000usize;
    let n = walk(e, &mut budget);
    if budget == 0 {
        16_001 // treat as over-cap
    } else {
        n
    }
}

fn const_care_of(e: &RExpr) -> u128 {
    match e {
        RExpr::Const { care, .. } => *care,
        _ => u128::MAX,
    }
}

fn adder_sum_bit(a: &RExpr, b: &RExpr, rtl: &Rtl, bit: usize) -> Result<Expr, String> {
    // Bound nested Add expansion: naive per-bit re-entry is exponential in nesting depth
    // (FM-HEL-HANG-1539: Ibex probe hung after flatten in rexpr_to_bit over Add trees).
    if bit > 64 {
        return Err("adder bit too wide".into());
    }
    if rexpr_add_depth(a).saturating_add(rexpr_add_depth(b)) > 4 {
        return Err("adder nesting too deep".into());
    }
    let mut cin = Expr::Const(false);
    let mut sum = Expr::Const(false);
    for i in 0..=bit {
        let ai = rexpr_to_bit(a, rtl, i)?;
        let bi = rexpr_to_bit(b, rtl, i)?;
        let axb = Expr::Xor(Box::new(ai.clone()), Box::new(bi.clone()));
        sum = Expr::Xor(Box::new(axb.clone()), Box::new(cin.clone()));
        let ab = Expr::And(Box::new(ai), Box::new(bi));
        let cin_axb = Expr::And(Box::new(cin), Box::new(axb));
        cin = Expr::Or(Box::new(ab), Box::new(cin_axb));
    }
    Ok(sum)
}

fn sub_diff_bit(a: &RExpr, b: &RExpr, rtl: &Rtl, bit: usize) -> Result<Expr, String> {
    // Bound like adder_sum_bit (Ibex hang caps).
    if bit > 64 {
        return Err("sub bit too wide".into());
    }
    if rexpr_add_depth(a).saturating_add(rexpr_add_depth(b)) > 4 {
        return Err("sub nesting too deep".into());
    }
    let mut borrow = Expr::Const(false);
    let mut diff = Expr::Const(false);
    for i in 0..=bit {
        let ai = rexpr_to_bit(a, rtl, i)?;
        let bi = rexpr_to_bit(b, rtl, i)?;
        let axb = Expr::Xor(Box::new(ai.clone()), Box::new(bi.clone()));
        diff = Expr::Xor(Box::new(axb.clone()), Box::new(borrow.clone()));
        // borrow_out = (~a & b) | (~(a^b) & borrow)
        let nota_b = Expr::And(Box::new(Expr::Not(Box::new(ai))), Box::new(bi));
        let eq_ab = Expr::Not(Box::new(axb));
        let eq_bor = Expr::And(Box::new(eq_ab), Box::new(borrow));
        borrow = Expr::Or(Box::new(nota_b), Box::new(eq_bor));
    }
    Ok(diff)
}

fn rexpr_add_depth(e: &RExpr) -> usize {
    match e {
        RExpr::Add(a, b) | RExpr::Sub(a, b) => 1 + rexpr_add_depth(a).max(rexpr_add_depth(b)),
        RExpr::Not(x) | RExpr::RedXor(x) | RExpr::RedAnd(x) | RExpr::RedOr(x) => {
            rexpr_add_depth(x)
        }
        RExpr::Shr(a, b) | RExpr::Ashr(a, b) => rexpr_add_depth(a).max(rexpr_add_depth(b)),
        RExpr::Concat(parts) => parts.iter().map(rexpr_add_depth).max().unwrap_or(0),
        RExpr::And(a, b)
        | RExpr::Or(a, b)
        | RExpr::Xor(a, b)
        | RExpr::Mul(a, b)
        | RExpr::Eq(a, b)
        | RExpr::Ne(a, b)
        | RExpr::Lt(a, b) => rexpr_add_depth(a).max(rexpr_add_depth(b)),
        RExpr::Mux(c, t, f) => rexpr_add_depth(c)
            .max(rexpr_add_depth(t))
            .max(rexpr_add_depth(f)),
        RExpr::IndexPart { base, .. } => rexpr_add_depth(base),
        RExpr::WordAt { addr, data } => rexpr_add_depth(addr).max(rexpr_add_depth(data)),
        RExpr::Range(_, _, _) | RExpr::Bit(_, _) | RExpr::Ident(_) | RExpr::Const { .. } => 0,
    }
}

fn name_known(rtl: &Rtl, name: &str) -> bool {
    rtl.signals.iter().any(|s| s.name == name)
        || rtl.ports.iter().any(|(n, _, _)| n == name)
}

fn rexpr_unknown_name(e: &RExpr, rtl: &Rtl) -> Option<String> {
    let mut names = HashSet::new();
    rexpr_names(e, &mut names);
    names.into_iter().find(|n| !name_known(rtl, n))
}

/// Relational assign (`<` / `>=` / `==` and `&&`/`||`/`!` of those).
fn rexpr_is_rel(e: &RExpr) -> bool {
    match e {
        RExpr::Lt(_, _) | RExpr::Eq(_, _) | RExpr::Ne(_, _) => true,
        RExpr::Not(x) => rexpr_is_rel(x),
        RExpr::And(a, b) | RExpr::Or(a, b) => rexpr_is_rel(a) || rexpr_is_rel(b),
        _ => false,
    }
}

/// A constant range is a test of a few bus bits, not a full-width cone.
/// LUT6 holds at most this many inputs. Wider compares stay `assign_not_lowered`.
const SMALL_REL_BITS: usize = 6;

struct RelBus {
    name: String,
    width: usize,
    /// Signal bit of vector bit 0 (`ident` is 0; `sig[hi:lo]` is `lo`).
    lo: usize,
}

fn as_rel_bus(e: &RExpr, rtl: &Rtl) -> Option<RelBus> {
    match e {
        RExpr::Ident(s) if name_known(rtl, s) => {
            let w = sig_width(rtl, s);
            if w == 0 || w > 128 {
                return None;
            }
            Some(RelBus {
                name: s.clone(),
                width: w,
                lo: 0,
            })
        }
        RExpr::Range(s, lo, hi) if name_known(rtl, s) && hi >= lo => {
            let pw = sig_width(rtl, s);
            if pw == 0 {
                return None;
            }
            let hi = (*hi).min(pw - 1);
            if *lo > hi {
                return None;
            }
            let w = range_span(*lo, hi)?;
            if w > 128 {
                return None;
            }
            Some(RelBus {
                name: s.clone(),
                width: w,
                lo: *lo,
            })
        }
        _ => None,
    }
}

/// Numeric bound. A sized don't-care pattern is not a constant range.
/// Unsized `'h8000` is tokenized with width = digit count; keep the value.
fn const_rel_value(e: &RExpr) -> Option<u128> {
    match e {
        RExpr::Const { val, width, care } => {
            if *width < 128 {
                let mask = care_mask(*width);
                if *care & mask != mask {
                    return None;
                }
            }
            Some(*val)
        }
        _ => None,
    }
}

/// Signal bits that unsigned `bus < c` depends on. Empty = constant result.
/// `None` if more bits matter than a LUT6 (not a small bit test).
fn lt_relevant_sig_bits(bus: &RelBus, c: u128) -> Option<Vec<usize>> {
    let w = bus.width;
    let full = if w >= 128 {
        u128::MAX
    } else {
        (1u128 << w) - 1
    };
    let local: Vec<usize> = if c == 0 || c > full {
        Vec::new()
    } else {
        let k = c.trailing_zeros() as usize;
        if w - k > SMALL_REL_BITS {
            return None;
        }
        (k..w).collect()
    };
    let pw = bus.lo.saturating_add(w);
    let mut sig_bits = Vec::with_capacity(local.len());
    for i in local {
        let sb = bus.lo + i;
        if sb >= pw {
            return None;
        }
        sig_bits.push(sb);
    }
    Some(sig_bits)
}

fn push_rel_pi(out: &mut Vec<(String, usize)>, name: &str, bit: usize) -> bool {
    if out.iter().any(|(n, b)| n == name && *b == bit) {
        return true;
    }
    if out.len() >= SMALL_REL_BITS {
        return false;
    }
    out.push((name.to_string(), bit));
    true
}

/// `bus < const` / `const < bus` as a small bit set. Other compares are not this form.
fn collect_lt_pis(a: &RExpr, b: &RExpr, rtl: &Rtl, out: &mut Vec<(String, usize)>) -> bool {
    let (bus, c, _bus_on_left) = if let (Some(bus), Some(c)) = (as_rel_bus(a, rtl), const_rel_value(b)) {
        (bus, c, true)
    } else if let (Some(c), Some(bus)) = (const_rel_value(a), as_rel_bus(b, rtl)) {
        // `const < bus` ≡ `bus > const` ≡ `!(bus < const+1)` when that stays in range.
        let pw_ok = bus.width;
        let full = if pw_ok >= 128 {
            u128::MAX
        } else {
            (1u128 << pw_ok) - 1
        };
        let bound = if c >= full { 0 } else { c + 1 };
        // `bus > c` depends on the same bits as `bus < c+1` (or none if always false).
        return match lt_relevant_sig_bits(&bus, bound) {
            Some(bits) => bits.into_iter().all(|sb| push_rel_pi(out, &bus.name, sb)),
            None => false,
        };
    } else {
        return false;
    };
    let _ = _bus_on_left;
    match lt_relevant_sig_bits(&bus, c) {
        Some(bits) => bits.into_iter().all(|sb| push_rel_pi(out, &bus.name, sb)),
        None => false,
    }
}

fn formula_is_const_rel(e: &RExpr, rtl: &Rtl, out: &mut Vec<(String, usize)>, saw_lt: &mut bool) -> bool {
    match e {
        RExpr::Not(x) => formula_is_const_rel(x, rtl, out, saw_lt),
        RExpr::And(a, b) | RExpr::Or(a, b) => {
            formula_is_const_rel(a, rtl, out, saw_lt) && formula_is_const_rel(b, rtl, out, saw_lt)
        }
        RExpr::Lt(a, b) => {
            *saw_lt = true;
            collect_lt_pis(a, b, rtl, out)
        }
        _ => false,
    }
}

fn eval_const_rel(e: &RExpr, rtl: &Rtl, env: &HashMap<String, bool>) -> Option<bool> {
    match e {
        RExpr::Not(x) => Some(!eval_const_rel(x, rtl, env)?),
        RExpr::And(a, b) => Some(eval_const_rel(a, rtl, env)? && eval_const_rel(b, rtl, env)?),
        RExpr::Or(a, b) => Some(eval_const_rel(a, rtl, env)? || eval_const_rel(b, rtl, env)?),
        RExpr::Lt(a, b) => {
            if let (Some(bus), Some(c)) = (as_rel_bus(a, rtl), const_rel_value(b)) {
                Some(rel_bus_val(&bus, rtl, env) < c)
            } else if let (Some(c), Some(bus)) = (const_rel_value(a), as_rel_bus(b, rtl)) {
                Some(c < rel_bus_val(&bus, rtl, env))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn rel_bus_val(bus: &RelBus, rtl: &Rtl, env: &HashMap<String, bool>) -> u128 {
    let pw = sig_width(rtl, &bus.name);
    let mut v = 0u128;
    for i in 0..bus.width {
        let sb = bus.lo + i;
        if sb >= 128 {
            break;
        }
        if env
            .get(&bit_name(&bus.name, pw, sb))
            .copied()
            .unwrap_or(false)
        {
            v |= 1u128 << i;
        }
    }
    v
}

/// Lower `bus < const`, `bus >= const` (`!(bus < const)`), and `&&`/`||` of those
/// to one LUT6 on only the bits that matter. A general compare wider than a LUT6
/// returns `None` (caller keeps one `assign_not_lowered`).
fn const_range_lut(e: &RExpr, rtl: &Rtl) -> Option<(u64, Vec<String>)> {
    let mut pis: Vec<(String, usize)> = Vec::new();
    let mut saw_lt = false;
    if !formula_is_const_rel(e, rtl, &mut pis, &mut saw_lt) || !saw_lt {
        return None;
    }
    let names: Vec<String> = pis
        .iter()
        .map(|(n, b)| bit_name(n, sig_width(rtl, n), *b))
        .collect();
    let n = names.len();
    let mut init = 0u64;
    let addrs = 1u64 << n;
    for addr in 0..addrs {
        let mut env = HashMap::new();
        for (i, name) in names.iter().enumerate() {
            env.insert(name.clone(), (addr >> i) & 1 == 1);
        }
        if eval_const_rel(e, rtl, &env)? {
            init |= 1u64 << addr;
        }
    }
    // Constant 0/1: fill the LUT so unused upper inputs do not matter.
    if n == 0 {
        init = if init & 1 == 1 { u64::MAX } else { 0 };
    }
    Some((init, names))
}

/// Unsized `'h8000` is tokenized with width = digit count, so the value does
/// not fit. Expand only that case; sized literals keep their declared width.
fn const_fit_width(val: u128, width: usize) -> usize {
    let declared = width.max(1);
    if declared >= 128 {
        return declared;
    }
    let mask = care_mask(declared);
    if val & !mask == 0 {
        return declared;
    }
    let need = if val == 0 {
        1
    } else {
        (128 - val.leading_zeros()) as usize
    };
    need.max(declared).min(128)
}

fn cmp_operand_width(e: &RExpr, rtl: &Rtl) -> usize {
    match e {
        RExpr::Const { val, width, .. } => const_fit_width(*val, *width),
        other => rexpr_width(other, rtl).max(1),
    }
}

/// Compare bit. Unknown names are an error, not const 0. Bits above a
/// declared width are zero (unsigned), not a repeated MSB. A const whose
/// value does not fit its token width uses the extra value bits.
fn cmp_src_bit(e: &RExpr, rtl: &Rtl, bit: usize) -> Result<Expr, String> {
    match e {
        RExpr::Const { val, width, care } => {
            let fitted = const_fit_width(*val, *width);
            if bit >= fitted.max(1) {
                return Ok(Expr::Const(false));
            }
            if bit < *width && (*care >> bit) & 1 == 0 && (*val >> bit) & 1 == 0 {
                return Ok(Expr::Const(false));
            }
            Ok(Expr::Const((*val >> bit) & 1 == 1))
        }
        RExpr::Ident(s) => {
            if !name_known(rtl, s) {
                return Err(format!("unknown name {s}"));
            }
            let w = sig_width(rtl, s);
            if bit >= w {
                Ok(Expr::Const(false))
            } else {
                Ok(Expr::Var(bit_name(s, w, bit)))
            }
        }
        RExpr::Bit(s, i) => {
            if !name_known(rtl, s) {
                return Err(format!("unknown name {s}"));
            }
            let w = sig_width(rtl, s);
            Ok(Expr::Var(bit_name(s, w, *i)))
        }
        other => rexpr_to_bit(other, rtl, bit),
    }
}

fn cmp_eq_bits(a: &RExpr, b: &RExpr, rtl: &Rtl, _eq: bool) -> Result<Expr, String> {
    let wa = cmp_operand_width(a, rtl);
    let wb = cmp_operand_width(b, rtl);
    let w = wa.max(wb).max(1);
    // FM-HEL-HANG: wide/nested Add under Eq → huge AIG (Ibex hang after CORPUS cmp).
    // Skip instead of hang; simple Ident/Const compares (corpus hswish/lrelu) still map.
    if w > 32 {
        return Err("cmp width too wide".into());
    }
    if rexpr_add_depth(a).saturating_add(rexpr_add_depth(b)) > 2 {
        return Err("cmp nesting too deep".into());
    }
    let care = const_care_of(a) & const_care_of(b);
    let mut acc: Option<Expr> = None;
    for i in 0..w {
        if (care >> i) & 1 == 0 {
            continue;
        }
        let ai = cmp_src_bit(a, rtl, i)?;
        let bi = cmp_src_bit(b, rtl, i)?;
        let xnor = Expr::Not(Box::new(Expr::Xor(Box::new(ai), Box::new(bi))));
        acc = Some(match acc {
            None => xnor,
            Some(p) => Expr::And(Box::new(p), Box::new(xnor)),
        });
    }
    Ok(acc.unwrap_or(Expr::Const(true)))
}

fn lt_bits(a: &RExpr, b: &RExpr, rtl: &Rtl) -> Result<Expr, String> {
    let w = cmp_operand_width(a, rtl).max(cmp_operand_width(b, rtl)).max(1);
    // FM-HEL-HANG: same bound as cmp_eq_bits — skip huge cones instead of hang.
    if w > 32 {
        return Err("lt width too wide".into());
    }
    if rexpr_add_depth(a).saturating_add(rexpr_add_depth(b)) > 2 {
        return Err("lt nesting too deep".into());
    }
    let mut acc = Expr::Const(false);
    let mut eq_so_far = Expr::Const(true);
    for i in (0..w).rev() {
        let ai = cmp_src_bit(a, rtl, i)?;
        let bi = cmp_src_bit(b, rtl, i)?;
        let a0b1 = Expr::And(Box::new(Expr::Not(Box::new(ai.clone()))), Box::new(bi.clone()));
        acc = Expr::Or(
            Box::new(acc),
            Box::new(Expr::And(Box::new(eq_so_far.clone()), Box::new(a0b1))),
        );
        let xnor = Expr::Not(Box::new(Expr::Xor(Box::new(ai), Box::new(bi))));
        eq_so_far = Expr::And(Box::new(eq_so_far), Box::new(xnor));
    }
    Ok(acc)
}

/// Width, or `None` when a slice/concat does not fit. Probe reductions,
/// compares, and mux conditions so a nested `sig[W-1:0]` with `W=0` is not
/// a 1-bit cone.
fn rexpr_width_checked(e: &RExpr, rtl: &Rtl) -> Option<usize> {
    match e {
        RExpr::Ident(s) | RExpr::Bit(s, _) => Some(sig_width(rtl, s)),
        RExpr::Range(_, lo, hi) => range_span(*lo, *hi),
        RExpr::IndexPart { width, .. } => Some((*width).max(1)),
        RExpr::WordAt { data, .. } => Some(rexpr_width_checked(data, rtl)?.max(1)),
        RExpr::Const { width, .. } => Some((*width).max(1)),
        RExpr::Concat(parts) => {
            let mut acc = 0usize;
            for p in parts {
                acc = acc.checked_add(rexpr_width_checked(p, rtl)?.max(1))?;
            }
            Some(acc)
        }
        RExpr::Shr(a, _) | RExpr::Ashr(a, _) => rexpr_width_checked(a, rtl),
        RExpr::RedXor(x) | RExpr::RedAnd(x) | RExpr::RedOr(x) => {
            rexpr_width_checked(x, rtl)?;
            Some(1)
        }
        RExpr::Eq(a, b) | RExpr::Ne(a, b) | RExpr::Lt(a, b) => {
            rexpr_width_checked(a, rtl)?;
            rexpr_width_checked(b, rtl)?;
            Some(1)
        }
        RExpr::Not(x) => Some(rexpr_width_checked(x, rtl)?.min(1).max(1)),
        RExpr::And(a, b)
        | RExpr::Or(a, b)
        | RExpr::Xor(a, b)
        | RExpr::Add(a, b)
        | RExpr::Sub(a, b)
        | RExpr::Mul(a, b) => {
            Some(rexpr_width_checked(a, rtl)?.max(rexpr_width_checked(b, rtl)?))
        }
        RExpr::Mux(c, t, f) => {
            rexpr_width_checked(c, rtl)?;
            Some(rexpr_width_checked(t, rtl)?.max(rexpr_width_checked(f, rtl)?))
        }
    }
}

fn rexpr_width(e: &RExpr, rtl: &Rtl) -> usize {
    match rexpr_width_checked(e, rtl) {
        Some(w) => w,
        None => {
            note_width_overflow();
            0
        }
    }
}

fn sig_depth(rtl: &Rtl, name: &str) -> usize {
    rtl.signals
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.depth)
        .unwrap_or(0)
}


/// Collapse `sig[hi:lo]` to Ident when used as a Mul operand (full-vector slice).
fn strip_range_to_ident(e: &RExpr) -> RExpr {
    match e {
        RExpr::Range(name, _, _) => RExpr::Ident(name.clone()),
        other => other.clone(),
    }
}

fn normalize_mul_operands(e: &RExpr) -> RExpr {
    match e {
        RExpr::Mul(a, b) => RExpr::Mul(
            Box::new(strip_range_to_ident(a)),
            Box::new(strip_range_to_ident(b)),
        ),
        RExpr::Add(a, b) => RExpr::Add(
            Box::new(normalize_mul_operands(a)),
            Box::new(normalize_mul_operands(b)),
        ),
        RExpr::Mux(c, t, f) => RExpr::Mux(
            Box::new((**c).clone()),
            Box::new(normalize_mul_operands(t)),
            Box::new(normalize_mul_operands(f)),
        ),
        other => other.clone(),
    }
}

fn expr_contains_mul(e: &RExpr) -> bool {
    match e {
        RExpr::Mul(_, _) => true,
        RExpr::Not(x) | RExpr::RedAnd(x) | RExpr::RedOr(x) | RExpr::RedXor(x) => expr_contains_mul(x),
        RExpr::And(a, b)
        | RExpr::Or(a, b)
        | RExpr::Xor(a, b)
        | RExpr::Add(a, b)
        | RExpr::Sub(a, b)
        | RExpr::Eq(a, b)
        | RExpr::Ne(a, b)
        | RExpr::Lt(a, b)
        | RExpr::Shr(a, b)
        | RExpr::Ashr(a, b) => expr_contains_mul(a) || expr_contains_mul(b),
        RExpr::Mux(c, t, f) => {
            expr_contains_mul(c) || expr_contains_mul(t) || expr_contains_mul(f)
        }
        RExpr::Concat(parts) => parts.iter().any(expr_contains_mul),
        // Address math in sig[base*+:W] is not a DSP multiply.
        RExpr::IndexPart { .. } => false,
        _ => false,
    }
}


/// Pull (A, B, optional C-accum) port/signal names from a MAC/mul RHS tree.
fn mac_ab_c_from_rhs(e: &RExpr) -> (Option<String>, Option<String>, Option<String>) {
    let e = normalize_mul_operands(e);
    fn mul_ab(e: &RExpr) -> Option<(String, String)> {
        match e {
            RExpr::Mul(a, b) => Some((
                rexpr_ident(a).or_else(|| match a.as_ref() {
                    RExpr::Range(n, _, _) | RExpr::Bit(n, _) => Some(n.clone()),
                    _ => None,
                })?,
                rexpr_ident(b).or_else(|| match b.as_ref() {
                    RExpr::Range(n, _, _) | RExpr::Bit(n, _) => Some(n.clone()),
                    _ => None,
                })?,
            )),
            RExpr::Add(l, r) => mul_ab(l).or_else(|| mul_ab(r)),
            RExpr::Mux(_, t, f) => mul_ab(t).or_else(|| mul_ab(f)),
            _ => None,
        }
    }
    fn accum_c(e: &RExpr, a: &str, b: &str) -> Option<String> {
        match e {
            RExpr::Add(l, r) => {
                let li = rexpr_ident(l).or_else(|| match l.as_ref() {
                    RExpr::Range(n, _, _) | RExpr::Bit(n, _) => Some(n.clone()),
                    _ => None,
                });
                let ri = rexpr_ident(r).or_else(|| match r.as_ref() {
                    RExpr::Range(n, _, _) | RExpr::Bit(n, _) => Some(n.clone()),
                    _ => None,
                });
                if matches!(r.as_ref(), RExpr::Mul(_, _)) {
                    return li.filter(|n| n != a && n != b);
                }
                if matches!(l.as_ref(), RExpr::Mul(_, _)) {
                    return ri.filter(|n| n != a && n != b);
                }
                accum_c(l, a, b).or_else(|| accum_c(r, a, b))
            }
            RExpr::Mux(_, t, f) => accum_c(t, a, b).or_else(|| accum_c(f, a, b)),
            _ => None,
        }
    }
    match mul_ab(&e) {
        Some((a, b)) => {
            let c = accum_c(&e, &a, &b);
            (Some(a), Some(b), c)
        }
        None => (None, None, None),
    }
}

/// Infer Mac27 and wire A/B/(C)/P so schematic/HNF are not pin-NC islands.
fn emit_mac27(d: &mut Design, cell: String, rhs: &RExpr, lhs: &str) {
    d.add_cell(&cell, CellKind::Mac27);
    let (a, b, c) = mac_ab_c_from_rhs(rhs);
    if let Some(a) = a {
        d.connect(&a, &cell, "A");
    }
    if let Some(b) = b {
        d.connect(&b, &cell, "B");
    }
    if let Some(c) = c {
        d.connect(&c, &cell, "C");
    }
    d.connect(lhs, &cell, "P");
}

fn is_mac_rhs(e: &RExpr) -> bool {
    let e = normalize_mul_operands(e);
    match &e {
        RExpr::Mul(_, _) => true,
        // `a*b`, `a*b+c`, `c+a*b`, or enable/clear mux wrapping a MAC.
        RExpr::Add(l, r) => {
            matches!(l.as_ref(), RExpr::Mul(_, _))
                || matches!(r.as_ref(), RExpr::Mul(_, _))
                || expr_contains_mul(l)
                || expr_contains_mul(r)
        }
        RExpr::Mux(_, t, f) => expr_contains_mul(t) || expr_contains_mul(f),
        _ => expr_contains_mul(&e),
    }
}

fn drive_target(e: &RExpr, rtl: &Rtl) -> Result<(String, usize), String> {
    match e {
        RExpr::Ident(s) => {
            let w = sig_width(rtl, s);
            if w == 1 {
                Ok((bit_name(s, 1, 0), 0))
            } else {
                Err(format!("vector {s} as 1-bit assign"))
            }
        }
        RExpr::Bit(s, i) => {
            let w = sig_width(rtl, s);
            Ok((bit_name(s, w, *i), *i))
        }
        _ => Err("assign rhs not a name/bitsel".into()),
    }
}


/// LUT6 INIT for a 2-input AND of (I0^a_inv) & (I1^b_inv), then XOR o_inv.
/// 4-bit pattern is replicated across 64 bits (unused I2..I5).
fn lut6_and2(a_inv: bool, b_inv: bool, o_inv: bool) -> u64 {
    let mut pat = 0u64;
    for addr in 0..4u32 {
        let i0 = addr & 1 == 1;
        let i1 = addr & 2 == 2;
        let v = ((i0 ^ a_inv) && (i1 ^ b_inv)) ^ o_inv;
        if v {
            pat |= 1u64 << addr;
        }
    }
    let mut acc = 0u64;
    let mut sh = 0;
    while sh < 64 {
        acc |= (pat & 0xf) << sh;
        sh += 4;
    }
    acc
}

fn lut6_xor2() -> u64 {
    0x6666_6666_6666_6666
}

fn lut6_from_i012(f: impl Fn(bool, bool, bool) -> bool) -> u64 {
    let mut pat = 0u64;
    for addr in 0..8u32 {
        let i0 = addr & 1 == 1;
        let i1 = addr & 2 == 2;
        let i2 = addr & 4 == 4;
        if f(i0, i1, i2) {
            pat |= 1u64 << addr;
        }
    }
    let mut acc = 0u64;
    let mut sh = 0;
    while sh < 64 {
        acc |= pat << sh;
        sh += 8;
    }
    acc
}

fn lut6_xor3() -> u64 {
    lut6_from_i012(|a, b, c| a ^ b ^ c)
}

fn lut6_maj3() -> u64 {
    lut6_from_i012(|a, b, c| (a && b) || (b && c) || (a && c))
}

fn emit_lut_pins(d: &mut Design, cell: &str, out: &str, init: u64, pins: &[(&str, &str)]) {
    d.add_cell(cell, CellKind::Lut6 { init });
    d.connect(out, cell, "O");
    for (net, pin) in pins {
        d.connect(*net, cell, *pin);
    }
}

fn lut6_inv() -> u64 {
    0x5555_5555_5555_5555
}

fn lut6_const(one: bool) -> u64 {
    if one {
        u64::MAX
    } else {
        0
    }
}

/// Wide-cone mapping cap (FM-HEL-10m-0854). VGA-style 12-bit compare muxes
/// printed `node_count` once per bit then spun in `map_wide_cone` until the
/// 90s kill. Modest cones still lower; over this, one diagnostic and stop.
const WIDE_CONE_PI_CAP: usize = 16;
const WIDE_CONE_AND_CAP: usize = 96;
const WIDE_CONE_MODULE_LUT_CAP: usize = 128;

fn wide_aig_over_cap(aig: &Aig) -> bool {
    aig.pis.len() > WIDE_CONE_PI_CAP || aig.ands.len() > WIDE_CONE_AND_CAP
}

/// One line per module. Further wide cones are skipped, not re-announced.
fn emit_wide_cone(module: &str, signal: &str, capped: &mut bool) {
    if *capped {
        return;
    }
    *capped = true;
    note_skip(format!(
        "diagnostic wide_cone module={module} signal={signal} (wide_cone_cap pis>{pi} ands>{ands} luts>{luts}; cone not mapped; not a LUT; not a closed WNS)",
        pi = WIDE_CONE_PI_CAP,
        ands = WIDE_CONE_AND_CAP,
        luts = WIDE_CONE_MODULE_LUT_CAP,
    ));
}

/// True if the cone has more than `n` distinct PIs, or the walk budget dies
/// before that can be ruled out. Stops at the (n+1)th variable so a wide
/// compare does not full-eval.
fn cone_pi_exceeds(e: &Expr, n: usize) -> bool {
    fn walk(e: &Expr, vars: &mut Vec<String>, budget: &mut usize, n: usize) -> bool {
        if *budget == 0 {
            return true;
        }
        *budget -= 1;
        match e {
            Expr::Const(_) => false,
            Expr::Var(name) => {
                if !vars.iter().any(|v| v == name) {
                    vars.push(name.clone());
                    if vars.len() > n {
                        return true;
                    }
                }
                false
            }
            Expr::Not(x) => walk(x, vars, budget, n),
            Expr::And(a, b) | Expr::Or(a, b) | Expr::Xor(a, b) => {
                walk(a, vars, budget, n) || walk(b, vars, budget, n)
            }
        }
    }
    let mut vars = Vec::new();
    let mut budget = 8_000usize;
    walk(e, &mut vars, &mut budget, n)
}

/// Map a >6-PI cone to a LUT2 tree. Output net is the function of `aig`.
/// PI<=6 stays on the single-LUT6 path so gold INIT patterns do not move.
/// Caller must refuse `wide_aig_over_cap` before calling; this does not invent
/// a partial cone past the and/PI cap.
fn map_wide_cone(d: &mut Design, aig: &Aig, prefix: &str) -> String {
    use std::collections::HashMap;
    let mut node_net: HashMap<u32, String> = HashMap::new();
    let mut n_lut = 0usize;
    fn emit_lut(d: &mut Design, prefix: &str, n_lut: &mut usize, init: u64, ins: &[(&str, &str)]) -> String {
        let cell = format!("{prefix}l{n_lut}");
        let out = format!("{prefix}n{n_lut}");
        *n_lut += 1;
        d.add_cell(&cell, CellKind::Lut6 { init });
        d.connect(&out, &cell, "O");
        for (net, pin) in ins {
            d.connect(*net, &cell, *pin);
        }
        out
    }
    fn lit_net(
        d: &mut Design,
        aig: &Aig,
        lit: Lit,
        prefix: &str,
        n_lut: &mut usize,
        node_net: &mut HashMap<u32, String>,
    ) -> String {
        let n = node_true(d, aig, lit.node, prefix, n_lut, node_net);
        if !lit.inv {
            return n;
        }
        emit_lut(d, prefix, n_lut, lut6_inv(), &[(&n, "I0")])
    }
    fn node_true(
        d: &mut Design,
        aig: &Aig,
        node: u32,
        prefix: &str,
        n_lut: &mut usize,
        node_net: &mut HashMap<u32, String>,
    ) -> String {
        if node == 0 {
            return emit_lut(d, prefix, n_lut, lut6_const(false), &[]);
        }
        if (node as usize) <= aig.pis.len() {
            return aig.pis[(node as usize) - 1].clone();
        }
        if let Some(n) = node_net.get(&node) {
            return n.clone();
        }
        let ai = (node as usize) - 1 - aig.pis.len();
        let (a, b) = aig.ands[ai];
        let na = lit_net(d, aig, a, prefix, n_lut, node_net);
        let nb = lit_net(d, aig, b, prefix, n_lut, node_net);
        let out = emit_lut(
            d,
            prefix,
            n_lut,
            lut6_and2(false, false, false),
            &[(&na, "I0"), (&nb, "I1")],
        );
        node_net.insert(node, out.clone());
        out
    }
    lit_net(d, aig, aig.output, prefix, &mut n_lut, &mut node_net)
}

/// Q net of one bit of an unpacked word. Distinct from packed `bit_name`.
fn unpacked_word_q(mem: &str, word: usize, width: usize, bit: usize) -> String {
    if width <= 1 {
        format!("{mem}_w{word}")
    } else {
        format!("{mem}_w{word}_{bit}")
    }
}

/// Hold-through CE, or the naked next-state if the always is not gated.
fn peel_word_enable<'a>(rhs: &'a RExpr, mem: &str) -> Option<(Option<RExpr>, &'a RExpr)> {
    match rhs {
        RExpr::Mux(c, t, f) => {
            let hold = match f.as_ref() {
                RExpr::Ident(s) if s == mem => true,
                RExpr::Bit(s, _) if s == mem => true,
                _ => false,
            };
            if !hold {
                return None;
            }
            Some((Some((**c).clone()), t.as_ref()))
        }
        other => Some((None, other)),
    }
}

enum WordShiftSrc {
    Packed(String),
    Word(usize),
}

/// Const-index unpacked write source: another word of the same array, or a packed vector.
fn classify_word_src(rtl: &Rtl, mem: &str, rhs: &RExpr) -> Option<WordShiftSrc> {
    match rhs {
        RExpr::Ident(s) if s != mem && sig_depth(rtl, s) == 0 => {
            Some(WordShiftSrc::Packed(s.clone()))
        }
        RExpr::Bit(s, w) if s == mem && sig_depth(rtl, s) > 0 => Some(WordShiftSrc::Word(*w)),
        _ => None,
    }
}

/// Enable must stay a handful of PIs. A wide enable is not this shift.
fn simple_word_enable(en: &RExpr, rtl: &Rtl) -> Option<Expr> {
    let e = rexpr_to_bit(en, rtl, 0).ok()?;
    if cone_pi_exceeds(&e, 4) {
        return None;
    }
    Some(e)
}

fn word_ce_mux(en: Expr, next: Expr, hold: Expr) -> Expr {
    Expr::Or(
        Box::new(Expr::And(Box::new(en.clone()), Box::new(next))),
        Box::new(Expr::And(Box::new(Expr::Not(Box::new(en))), Box::new(hold))),
    )
}

/// Bounded unpacked clocked shift / load onto the user's clock.
/// 3×12 enable-gated word copies become per-bit Hffs. A variable-index read
/// is not expanded here — that would be a wide mux, not this lower.
/// Returns (reg bits, memory names that lowered). Empty if no safe write.
fn lower_unpacked_clocked_words(rtl: &Rtl) -> (Vec<(String, Expr)>, HashSet<String>) {
    const MAX_DEPTH: usize = 16;
    const MAX_WIDTH: usize = 32;
    const MAX_BITS: usize = 128;
    let mut names: Vec<String> = Vec::new();
    for (lhs, _, _) in &rtl.nbas {
        if sig_depth(rtl, lhs) == 0 {
            continue;
        }
        if !names.iter().any(|n| n == lhs) {
            names.push(lhs.clone());
        }
    }
    let mut out = Vec::new();
    let mut lowered = HashSet::new();
    for mem in names {
        let depth = sig_depth(rtl, &mem);
        let width = sig_width(rtl, &mem).max(1);
        if depth == 0
            || depth > MAX_DEPTH
            || width > MAX_WIDTH
            || depth.saturating_mul(width) > MAX_BITS
        {
            continue;
        }
        let writes: Vec<_> = rtl
            .nbas
            .iter()
            .filter(|(lhs, _, _)| lhs == &mem)
            .collect();
        if writes.is_empty() {
            continue;
        }
        let mut planned: Vec<(usize, Option<Expr>, WordShiftSrc)> = Vec::new();
        let mut ok = true;
        for (_, bit, rhs) in writes {
            let Some(word) = *bit else {
                ok = false;
                break;
            };
            if word >= depth {
                ok = false;
                break;
            }
            let Some((en_r, src_r)) = peel_word_enable(rhs, &mem) else {
                ok = false;
                break;
            };
            let Some(src) = classify_word_src(rtl, &mem, src_r) else {
                ok = false;
                break;
            };
            if let WordShiftSrc::Word(w) = &src {
                if *w >= depth {
                    ok = false;
                    break;
                }
            }
            let en = if let Some(er) = en_r {
                match simple_word_enable(&er, rtl) {
                    Some(e) => Some(e),
                    None => {
                        ok = false;
                        break;
                    }
                }
            } else {
                None
            };
            if let Some(slot) = planned.iter_mut().find(|(w, _, _)| *w == word) {
                *slot = (word, en, src);
            } else {
                planned.push((word, en, src));
            }
        }
        if !ok || planned.is_empty() {
            continue;
        }
        for (word, en, src) in planned {
            for bit in 0..width {
                let next = match &src {
                    WordShiftSrc::Packed(s) => {
                        let sw = sig_width(rtl, s);
                        if bit >= sw {
                            Expr::Const(false)
                        } else {
                            Expr::Var(bit_name(s, sw, bit))
                        }
                    }
                    WordShiftSrc::Word(w) => Expr::Var(unpacked_word_q(&mem, *w, width, bit)),
                };
                let qn = unpacked_word_q(&mem, word, width, bit);
                let d = if let Some(en) = en.clone() {
                    word_ce_mux(en, next, Expr::Var(qn.clone()))
                } else {
                    next
                };
                out.push((qn, d));
            }
        }
        lowered.insert(mem);
    }
    (out, lowered)
}

/// Constant word index of an unpacked array. A signal index is not a constant.
fn const_rexpr_usize(e: &RExpr) -> Option<usize> {
    match e {
        RExpr::Const { val, .. } => usize::try_from(*val).ok(),
        _ => None,
    }
}

/// `{bus, {K{1'b0}}}` — zeros on the low side. `Ok((bus, K))` when K is
/// 1..=8 and the bus is a wire. `Err(())` is that shape but not a wire
/// alignment (not a LUT, caller names `assign_not_lowered`). Other concats
/// are `None` and stay on the existing path.
fn zero_fill_concat(rhs: &RExpr) -> Option<Result<(String, usize), ()>> {
    let RExpr::Concat(parts) = rhs else {
        return None;
    };
    if parts.len() != 2 {
        return None;
    }
    let zeros = match &parts[1] {
        RExpr::Const { val: 0, width, .. } if *width >= 1 => *width,
        _ => return None,
    };
    let bus = match &parts[0] {
        RExpr::Ident(s) => s.clone(),
        _ => return Some(Err(())),
    };
    if (1..=8).contains(&zeros) {
        Some(Ok((bus, zeros)))
    } else {
        Some(Err(()))
    }
}

fn const_unpacked_word(rhs: &RExpr, rtl: &Rtl) -> Option<(String, usize)> {
    match rhs {
        RExpr::Bit(name, idx) if sig_depth(rtl, name) > 0 && *idx < sig_depth(rtl, name) => {
            Some((name.clone(), *idx))
        }
        RExpr::IndexPart { name, base, .. } if sig_depth(rtl, name) > 0 => {
            let idx = const_rexpr_usize(base)?;
            if idx < sig_depth(rtl, name) {
                Some((name.clone(), idx))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn packed_add_pair(rhs: &RExpr, rtl: &Rtl) -> Option<(String, String)> {
    let RExpr::Add(a, b) = rhs else {
        return None;
    };
    if expr_contains_mul(rhs) {
        return None;
    }
    let a_name = match a.as_ref() {
        RExpr::Ident(s) if sig_depth(rtl, s) == 0 => s.clone(),
        _ => return None,
    };
    let b_name = match b.as_ref() {
        RExpr::Ident(s) if sig_depth(rtl, s) == 0 => s.clone(),
        _ => return None,
    };
    Some((a_name, b_name))
}

/// `assign y = bus + K` (either order). K is a small constant 1..=16.
/// Not two named buses, not a multiply, not an unpacked word.
fn packed_add_const(rhs: &RExpr, rtl: &Rtl) -> Option<(String, u128)> {
    let RExpr::Add(a, b) = rhs else {
        return None;
    };
    if expr_contains_mul(rhs) {
        return None;
    }
    let (bus, k) = match (a.as_ref(), b.as_ref()) {
        (RExpr::Ident(s), RExpr::Const { val, .. }) if sig_depth(rtl, s) == 0 => (s.clone(), *val),
        (RExpr::Const { val, .. }, RExpr::Ident(s)) if sig_depth(rtl, s) == 0 => (s.clone(), *val),
        _ => return None,
    };
    if (1u128..=16).contains(&k) {
        Some((bus, k))
    } else {
        None
    }
}

/// `assign y = bus - K`. K is a small constant 1..=16 on the right only.
/// Not `K - bus`, not two named buses, not a multiply, not an unpacked word.
fn packed_sub_const(rhs: &RExpr, rtl: &Rtl) -> Option<(String, u128)> {
    let RExpr::Sub(a, b) = rhs else {
        return None;
    };
    if expr_contains_mul(rhs) {
        return None;
    }
    let (bus, k) = match (a.as_ref(), b.as_ref()) {
        (RExpr::Ident(s), RExpr::Const { val, .. }) if sig_depth(rtl, s) == 0 => (s.clone(), *val),
        _ => return None,
    };
    if (1u128..=16).contains(&k) {
        Some((bus, k))
    } else {
        None
    }
}

fn op_bit_net(rtl: &Rtl, name: &str, bit: usize) -> Option<String> {
    let w = sig_width(rtl, name);
    if bit >= w {
        None
    } else {
        Some(bit_name(name, w, bit))
    }
}

/// Combinational ripple of two named buses into `assign sum = a + b`.
/// Each bit is a 1-bit full adder (a, b, cin), not one 64-PI cone.
/// No clock, no Hff, not a MAC. `width` > 32 is refused by the caller
/// so a shorter bus is not invented. Returns false if any bit is skipped.
fn emit_ripple_add(
    d: &mut Design,
    rtl: &Rtl,
    sum: &str,
    width: usize,
    a: &str,
    b: &str,
) -> bool {
    if width == 0 || width > 32 {
        return false;
    }
    // A missing operand bit is a skip, not a zero-extended invented bus.
    if (0..width).any(|bit| op_bit_net(rtl, a, bit).is_none() || op_bit_net(rtl, b, bit).is_none()) {
        return false;
    }
    let mut cin: Option<String> = None;
    for bit in 0..width {
        let an = op_bit_net(rtl, a, bit);
        let bn = op_bit_net(rtl, b, bit);
        let sum_net = bit_name(sum, width, bit);
        let sum_cell = format!("u_ra_{sum}_{bit}s");
        let cin_now = cin.clone();
        let emitted = match (an.as_deref(), bn.as_deref(), cin_now.as_deref()) {
            (Some(an), Some(bn), None) => {
                emit_lut_pins(d, &sum_cell, &sum_net, lut6_xor2(), &[(an, "I0"), (bn, "I1")]);
                if bit + 1 < width {
                    let cout = format!("n_ra_{sum}_{bit}c");
                    let cry_cell = format!("u_ra_{sum}_{bit}c");
                    emit_lut_pins(
                        d,
                        &cry_cell,
                        &cout,
                        lut6_and2(false, false, false),
                        &[(an, "I0"), (bn, "I1")],
                    );
                    cin = Some(cout);
                }
                true
            }
            (Some(an), Some(bn), Some(cn)) => {
                emit_lut_pins(
                    d,
                    &sum_cell,
                    &sum_net,
                    lut6_xor3(),
                    &[(an, "I0"), (bn, "I1"), (cn, "I2")],
                );
                if bit + 1 < width {
                    let cout = format!("n_ra_{sum}_{bit}c");
                    let cry_cell = format!("u_ra_{sum}_{bit}c");
                    emit_lut_pins(
                        d,
                        &cry_cell,
                        &cout,
                        lut6_maj3(),
                        &[(an, "I0"), (bn, "I1"), (cn, "I2")],
                    );
                    cin = Some(cout);
                }
                true
            }
            _ => false,
        };
        if !emitted {
            return false;
        }
    }
    eprintln!("synth_rtl ripple_add signal={sum} bits={width}");
    true
}

fn lut6_buf() -> u64 {
    // O = I0, independent of I1..I5.
    0xAAAA_AAAA_AAAA_AAAA
}

fn lut6_xnor2() -> u64 {
    0x9999_9999_9999_9999
}

/// Combinational ripple of `assign sum = bus + K`. K is a constant 1..=16,
/// not a second 32-bit addend bus and not a MAC. Carry is injected on the
/// low bits where the constant is 1; zero constant bits with no carry are
/// a copy of that bus bit. No clock, no Hff. `width` > 32 is refused by
/// the caller so a shorter bus is not invented. Returns false if any bit
/// is skipped.
fn emit_ripple_add_const(
    d: &mut Design,
    rtl: &Rtl,
    sum: &str,
    width: usize,
    bus: &str,
    k: u128,
) -> bool {
    if width == 0 || width > 32 || !(1u128..=16).contains(&k) {
        return false;
    }
    // A missing bus bit is a skip, not a zero-extended invented bus.
    if (0..width).any(|bit| op_bit_net(rtl, bus, bit).is_none()) {
        return false;
    }
    let mut cin: Option<String> = None;
    for bit in 0..width {
        let Some(an) = op_bit_net(rtl, bus, bit) else {
            return false;
        };
        let kbit = ((k >> bit) & 1) == 1;
        let sum_net = bit_name(sum, width, bit);
        let sum_cell = format!("u_rac_{sum}_{bit}s");
        let cin_now = cin.clone();
        match (kbit, cin_now.as_deref()) {
            (false, None) => {
                // Constant bit is 0 and no carry yet: this bit is the bus bit.
                emit_lut_pins(d, &sum_cell, &sum_net, lut6_buf(), &[(&an, "I0")]);
            }
            (true, None) => {
                // First 1 in K: sum = ~bus, carry-out is that bus bit. No
                // constant vector is invented for the addend.
                emit_lut_pins(d, &sum_cell, &sum_net, lut6_inv(), &[(&an, "I0")]);
                if bit + 1 < width {
                    cin = Some(an);
                }
            }
            (false, Some(cn)) => {
                emit_lut_pins(
                    d,
                    &sum_cell,
                    &sum_net,
                    lut6_xor2(),
                    &[(&an, "I0"), (cn, "I1")],
                );
                if bit + 1 < width {
                    let cout = format!("n_rac_{sum}_{bit}c");
                    let cry_cell = format!("u_rac_{sum}_{bit}c");
                    emit_lut_pins(
                        d,
                        &cry_cell,
                        &cout,
                        lut6_and2(false, false, false),
                        &[(&an, "I0"), (cn, "I1")],
                    );
                    cin = Some(cout);
                }
            }
            (true, Some(cn)) => {
                emit_lut_pins(
                    d,
                    &sum_cell,
                    &sum_net,
                    lut6_xnor2(),
                    &[(&an, "I0"), (cn, "I1")],
                );
                if bit + 1 < width {
                    let cout = format!("n_rac_{sum}_{bit}c");
                    let cry_cell = format!("u_rac_{sum}_{bit}c");
                    emit_lut_pins(
                        d,
                        &cry_cell,
                        &cout,
                        lut6_and2(true, true, true),
                        &[(&an, "I0"), (cn, "I1")],
                    );
                    cin = Some(cout);
                }
            }
        }
    }
    eprintln!("synth_rtl ripple_add_const signal={sum} bits={width} const={k}");
    true
}

/// Combinational ripple of `assign y = bus - K`. K is a constant 1..=16
/// on the right, not a second operand bus and not a MAC. Borrow is injected
/// where the constant bit is 1; a zero constant bit with no borrow is a
/// copy of that bus bit. The first borrow is the inverted bus bit (the
/// result bit itself). No clock, no Hff. `width` > 32 is refused by the
/// caller so a shorter bus is not invented. Returns false if any bit
/// is skipped.
fn emit_ripple_sub_const(
    d: &mut Design,
    rtl: &Rtl,
    diff: &str,
    width: usize,
    bus: &str,
    k: u128,
) -> bool {
    if width == 0 || width > 32 || !(1u128..=16).contains(&k) {
        return false;
    }
    // A missing bus bit is a skip, not a zero-extended invented bus.
    if (0..width).any(|bit| op_bit_net(rtl, bus, bit).is_none()) {
        return false;
    }
    let mut bin: Option<String> = None;
    for bit in 0..width {
        let Some(an) = op_bit_net(rtl, bus, bit) else {
            return false;
        };
        let kbit = ((k >> bit) & 1) == 1;
        let diff_net = bit_name(diff, width, bit);
        let diff_cell = format!("u_rsc_{diff}_{bit}s");
        let bin_now = bin.clone();
        match (kbit, bin_now.as_deref()) {
            (false, None) => {
                // Constant bit is 0 and no borrow yet: this bit is the bus bit.
                emit_lut_pins(d, &diff_cell, &diff_net, lut6_buf(), &[(&an, "I0")]);
            }
            (true, None) => {
                // First 1 in K: diff = ~bus, borrow-out is that inverted bus
                // bit. No constant vector is invented for the subtrahend.
                emit_lut_pins(d, &diff_cell, &diff_net, lut6_inv(), &[(&an, "I0")]);
                if bit + 1 < width {
                    bin = Some(diff_net);
                }
            }
            (false, Some(bn)) => {
                emit_lut_pins(
                    d,
                    &diff_cell,
                    &diff_net,
                    lut6_xor2(),
                    &[(&an, "I0"), (bn, "I1")],
                );
                if bit + 1 < width {
                    let bout = format!("n_rsc_{diff}_{bit}b");
                    let cry_cell = format!("u_rsc_{diff}_{bit}b");
                    // bout = ~bus & bin
                    emit_lut_pins(
                        d,
                        &cry_cell,
                        &bout,
                        lut6_and2(true, false, false),
                        &[(&an, "I0"), (bn, "I1")],
                    );
                    bin = Some(bout);
                }
            }
            (true, Some(bn)) => {
                emit_lut_pins(
                    d,
                    &diff_cell,
                    &diff_net,
                    lut6_xnor2(),
                    &[(&an, "I0"), (bn, "I1")],
                );
                if bit + 1 < width {
                    let bout = format!("n_rsc_{diff}_{bit}b");
                    let cry_cell = format!("u_rsc_{diff}_{bit}b");
                    // bout = ~bus | bin
                    emit_lut_pins(
                        d,
                        &cry_cell,
                        &bout,
                        lut6_and2(false, true, true),
                        &[(&an, "I0"), (bn, "I1")],
                    );
                    bin = Some(bout);
                }
            }
        }
    }
    eprintln!("synth_rtl ripple_sub_const signal={diff} bits={width} const={k}");
    true
}

/// Ripple add of two packed vectors into one unpacked word, clocked by `clk`.
/// Signed and unsigned agree on the `width` sum bits. Not a MAC.
fn emit_word_add(
    d: &mut Design,
    rtl: &Rtl,
    clk: &str,
    mem: &str,
    word: usize,
    width: usize,
    a: &str,
    b: &str,
) {
    let mut cin: Option<String> = None;
    for bit in 0..width {
        let an = op_bit_net(rtl, a, bit);
        let bn = op_bit_net(rtl, b, bit);
        let sum = format!("u_add{word}_{bit}s");
        let qn = unpacked_word_q(mem, word, width, bit);
        let sum_cell = format!("u_alu{word}_{bit}s");
        let cin_now = cin.clone();
        match (an.as_deref(), bn.as_deref(), cin_now.as_deref()) {
            (Some(an), Some(bn), None) => {
                emit_lut_pins(d, &sum_cell, &sum, lut6_xor2(), &[(an, "I0"), (bn, "I1")]);
                if bit + 1 < width {
                    let cout = format!("u_add{word}_{bit}c");
                    let cry_cell = format!("u_alu{word}_{bit}c");
                    emit_lut_pins(
                        d,
                        &cry_cell,
                        &cout,
                        lut6_and2(false, false, false),
                        &[(an, "I0"), (bn, "I1")],
                    );
                    cin = Some(cout);
                }
            }
            (Some(an), Some(bn), Some(cn)) => {
                emit_lut_pins(
                    d,
                    &sum_cell,
                    &sum,
                    lut6_xor3(),
                    &[(an, "I0"), (bn, "I1"), (cn, "I2")],
                );
                if bit + 1 < width {
                    let cout = format!("u_add{word}_{bit}c");
                    let cry_cell = format!("u_alu{word}_{bit}c");
                    emit_lut_pins(
                        d,
                        &cry_cell,
                        &cout,
                        lut6_maj3(),
                        &[(an, "I0"), (bn, "I1"), (cn, "I2")],
                    );
                    cin = Some(cout);
                }
            }
            _ => {
                let src = an.clone().or(bn.clone());
                if let Some(src) = src.as_deref() {
                    emit_lut_pins(
                        d,
                        &sum_cell,
                        &sum,
                        lut6_and2(false, false, false),
                        &[(src, "I0"), (src, "I1")],
                    );
                } else {
                    emit_lut_pins(d, &sum_cell, &sum, lut6_const(false), &[]);
                }
                if bit + 1 < width {
                    cin = None;
                }
            }
        }
        let ff = format!("u_aff{word}_{bit}");
        d.add_cell(&ff, CellKind::Hff);
        d.connect(clk, &ff, "CLK");
        d.connect(&sum, &ff, "D");
        d.connect(&qn, &ff, "Q");
    }
    eprintln!("synth_rtl word_add mem={mem} word={word} bits={width} clk={clk}");
}

/// Pipeline word: Hff clocked by `clk`, D from the previous word. Not an adder.
fn emit_word_shift(d: &mut Design, clk: &str, mem: &str, word: usize, from: usize, width: usize) {
    for bit in 0..width {
        let dnet = unpacked_word_q(mem, from, width, bit);
        let qn = unpacked_word_q(mem, word, width, bit);
        let ff = format!("u_sff{word}_{bit}");
        d.add_cell(&ff, CellKind::Hff);
        d.connect(clk, &ff, "CLK");
        d.connect(&dnet, &ff, "D");
        d.connect(&qn, &ff, "Q");
    }
    eprintln!("synth_rtl word_shift mem={mem} word={word} from={from} bits={width} clk={clk}");
}

fn lower_const_word_adds(
    d: &mut Design,
    rtl: &Rtl,
    clk: &str,
    already: &HashSet<String>,
) -> HashSet<String> {
    const MAX_WIDTH: usize = 32;
    let mut names: Vec<String> = Vec::new();
    for (lhs, bit, rhs) in &rtl.nbas {
        if already.contains(lhs) || sig_depth(rtl, lhs) == 0 || bit.is_none() {
            continue;
        }
        if packed_add_pair(rhs, rtl).is_none() {
            continue;
        }
        if !names.iter().any(|n| n == lhs) {
            names.push(lhs.clone());
        }
    }
    let mut lowered = HashSet::new();
    for mem in names {
        let depth = sig_depth(rtl, &mem);
        let width = sig_width(rtl, &mem).max(1);
        if depth == 0 || width > MAX_WIDTH {
            continue;
        }
        let mut words: Vec<(usize, String, String)> = Vec::new();
        let mut ok = true;
        for (_, bit, rhs) in rtl.nbas.iter().filter(|(lhs, _, _)| lhs == &mem) {
            let Some(word) = *bit else {
                ok = false;
                break;
            };
            if word >= depth {
                ok = false;
                break;
            }
            let Some((a, b)) = packed_add_pair(rhs, rtl) else {
                ok = false;
                break;
            };
            if let Some(slot) = words.iter_mut().find(|(w, _, _)| *w == word) {
                *slot = (word, a, b);
            } else {
                words.push((word, a, b));
            }
        }
        if !ok || words.is_empty() {
            continue;
        }
        for (word, a, b) in &words {
            emit_word_add(d, rtl, clk, &mem, *word, width, a, b);
        }
        lowered.insert(mem);
    }
    lowered
}

/// `res[0] <= a+b` plus `for (i = 1; i < cycles) res[i] <= res[i-1]`.
/// Constant cycles 2, 3, or 4 unroll as Hff word shifts. q aliases the last
/// word. More than 4 words, or a non-constant bound: one `word_pipeline_cap`,
/// the add stays in word 0, extra stages are not invented, WNS is not closed.
fn lower_word_pipeline(
    d: &mut Design,
    rtl: &Rtl,
    clk: &str,
    already: &HashSet<String>,
) -> HashSet<String> {
    const MAX_WORDS: usize = 4;
    const MAX_WIDTH: usize = 32;
    let mut names: Vec<String> = Vec::new();
    for (lhs, _, _) in &rtl.nbas {
        if already.contains(lhs) || sig_depth(rtl, lhs) == 0 {
            continue;
        }
        if !names.iter().any(|n| n == lhs) {
            names.push(lhs.clone());
        }
    }
    let mut lowered = HashSet::new();
    for mem in names {
        let depth = sig_depth(rtl, &mem);
        let width = sig_width(rtl, &mem).max(1);
        if depth == 0 || width > MAX_WIDTH {
            continue;
        }
        let mut adds: Vec<(usize, String, String)> = Vec::new();
        let mut shifts: Vec<(usize, usize)> = Vec::new();
        let mut ok = true;
        let mut saw_shift = false;
        for (_, bit, rhs) in rtl.nbas.iter().filter(|(lhs, _, _)| lhs == &mem) {
            let Some(word) = *bit else {
                ok = false;
                break;
            };
            if word >= depth {
                ok = false;
                break;
            }
            let Some((en, src)) = peel_word_enable(rhs, &mem) else {
                ok = false;
                break;
            };
            if en.is_some() {
                ok = false;
                break;
            }
            if let Some((a, b)) = packed_add_pair(src, rtl) {
                if let Some(slot) = adds.iter_mut().find(|(w, _, _)| *w == word) {
                    *slot = (word, a, b);
                } else {
                    adds.push((word, a, b));
                }
            } else if let Some(WordShiftSrc::Word(from)) = classify_word_src(rtl, &mem, src) {
                saw_shift = true;
                if let Some(slot) = shifts.iter_mut().find(|(w, _)| *w == word) {
                    *slot = (word, from);
                } else {
                    shifts.push((word, from));
                }
            } else {
                ok = false;
                break;
            }
        }
        if !ok || !saw_shift || adds.len() != 1 || adds[0].0 != 0 {
            continue;
        }
        if shifts
            .iter()
            .any(|(w, from)| *w == 0 || *from + 1 != *w || *from >= depth)
        {
            continue;
        }
        let max_word = shifts.iter().map(|(w, _)| *w).max().unwrap_or(0);
        let words = depth.max(max_word + 1);
        let add_a = adds[0].1.clone();
        let add_b = adds[0].2.clone();
        if words > MAX_WORDS || word_pipeline_cap_for(&rtl.module) {
            note_word_pipeline_cap(&rtl.module, &mem, &words.to_string());
            emit_word_add(d, rtl, clk, &mem, 0, width, &add_a, &add_b);
            eprintln!(
                "synth_rtl word_pipeline_cap mem={mem} words={words} hffs={width} add_word=0 clk={clk}"
            );
            lowered.insert(mem);
            continue;
        }
        emit_word_add(d, rtl, clk, &mem, 0, width, &add_a, &add_b);
        let mut emitted = 1usize;
        let mut shift_ws = shifts.clone();
        shift_ws.sort_by_key(|(w, _)| *w);
        for (word, from) in shift_ws {
            if word >= MAX_WORDS {
                continue;
            }
            emit_word_shift(d, clk, &mem, word, from, width);
            emitted += 1;
        }
        let q_word = words.saturating_sub(1);
        eprintln!(
            "synth_rtl word_pipeline mem={mem} words={words} q_word={q_word} hffs={} add_word=0 clk={clk}",
            emitted * width
        );
        lowered.insert(mem.clone());
    }
    lowered
}

/// `we && (addr == word)` as one LUT6. Address plus enable must stay ≤6 PIs.
fn addr_match_expr(addr: &str, addr_w: usize, word: usize, en: Option<&Expr>) -> Expr {
    let mut acc: Option<Expr> = None;
    for i in 0..addr_w.max(1) {
        let bit = Expr::Var(bit_name(addr, addr_w, i));
        let term = if (word >> i) & 1 == 1 {
            bit
        } else {
            Expr::Not(Box::new(bit))
        };
        acc = Some(match acc {
            None => term,
            Some(a) => Expr::And(Box::new(a), Box::new(term)),
        });
    }
    let eq = acc.unwrap_or(Expr::Const(true));
    if let Some(en) = en {
        Expr::And(Box::new(en.clone()), Box::new(eq))
    } else {
        eq
    }
}

fn emit_small_lut(d: &mut Design, cell: &str, out: &str, expr: &Expr) -> bool {
    if cone_pi_exceeds(expr, 6) || expr_node_count(expr) > 256 {
        return false;
    }
    let aig = Aig::from_expr(expr);
    if aig.pis.len() > 6 {
        return false;
    }
    let init = aig.flowmap_lut6();
    d.add_cell(cell, CellKind::Lut6 { init });
    d.connect(out, cell, "O");
    for (pin, pi) in aig.pis.iter().enumerate() {
        d.connect(pi, cell, format!("I{pin}"));
    }
    true
}

struct VarWordWrite {
    en: Option<Expr>,
    addr: String,
    addr_w: usize,
    data: String,
}

/// Peel `if (we) mem[addr] <= data` (hold the unselected words).
fn peel_var_word_write(rhs: &RExpr, mem: &str, rtl: &Rtl) -> Option<VarWordWrite> {
    let (en_r, payload) = match rhs {
        RExpr::WordAt { .. } => (None, rhs),
        RExpr::Mux(c, t, f) => {
            let hold = match f.as_ref() {
                RExpr::Ident(s) if s == mem => true,
                RExpr::Bit(s, _) if s == mem => true,
                _ => false,
            };
            if !hold {
                return None;
            }
            (Some(c.as_ref()), t.as_ref())
        }
        _ => return None,
    };
    let RExpr::WordAt { addr, data } = payload else {
        return None;
    };
    let RExpr::Ident(addr_name) = addr.as_ref() else {
        return None;
    };
    let RExpr::Ident(data_name) = data.as_ref() else {
        return None;
    };
    if sig_depth(rtl, addr_name) > 0 || sig_depth(rtl, data_name) > 0 {
        return None;
    }
    let addr_w = sig_width(rtl, addr_name).max(1);
    if addr_w > 6 {
        return None;
    }
    let en = if let Some(er) = en_r {
        Some(simple_word_enable(er, rtl)?)
    } else {
        None
    };
    if en.is_some() && addr_w > 5 {
        return None;
    }
    Some(VarWordWrite {
        en,
        addr: addr_name.clone(),
        addr_w,
        data: data_name.clone(),
    })
}

/// Bounded unpacked variable-index write: one Hff per bit, clocked by `clk`.
/// Depth ≤32 and width ≤16. The read is not expanded — address compare is a
/// shared ≤6-PI LUT, then a 3-PI hold mux per bit. Not a >16 PI cone.
fn lower_var_index_words(d: &mut Design, rtl: &Rtl, clk: &str) -> HashSet<String> {
    const MAX_DEPTH: usize = 32;
    const MAX_WIDTH: usize = 16;
    let mut names: Vec<String> = Vec::new();
    for (lhs, _, _) in &rtl.nbas {
        if sig_depth(rtl, lhs) == 0 {
            continue;
        }
        if !names.iter().any(|n| n == lhs) {
            names.push(lhs.clone());
        }
    }
    let mut lowered = HashSet::new();
    for mem in names {
        let depth = sig_depth(rtl, &mem);
        let width = sig_width(rtl, &mem).max(1);
        if depth == 0 || depth > MAX_DEPTH || width > MAX_WIDTH {
            continue;
        }
        let need = if depth <= 1 {
            1
        } else {
            (usize::BITS - (depth - 1).leading_zeros()) as usize
        };
        let writes: Vec<_> = rtl
            .nbas
            .iter()
            .filter(|(lhs, _, _)| lhs == &mem)
            .collect();
        if writes.is_empty() {
            continue;
        }
        let mut planned: Option<VarWordWrite> = None;
        let mut ok = true;
        for (_, bit, rhs) in writes {
            if bit.is_some() {
                ok = false;
                break;
            }
            let Some(w) = peel_var_word_write(rhs, &mem, rtl) else {
                ok = false;
                break;
            };
            if w.addr_w < need || depth > (1usize << w.addr_w.min(16)) {
                ok = false;
                break;
            }
            if let Some(prev) = &planned {
                if prev.addr != w.addr || prev.data != w.data {
                    ok = false;
                    break;
                }
            } else {
                planned = Some(w);
            }
        }
        let Some(plan) = planned else {
            continue;
        };
        if !ok {
            continue;
        }
        let mut matches: Vec<(String, Expr)> = Vec::new();
        for word in 0..depth {
            let expr = addr_match_expr(&plan.addr, plan.addr_w, word, plan.en.as_ref());
            if cone_pi_exceeds(&expr, 6) {
                ok = false;
                break;
            }
            let aig = Aig::from_expr(&expr);
            if aig.pis.len() > 6 {
                ok = false;
                break;
            }
            matches.push((format!("{mem}_we{word}"), expr));
        }
        if !ok || matches.len() != depth {
            continue;
        }
        let data_w = sig_width(rtl, &plan.data);
        for (net, expr) in &matches {
            let cell = format!("u_{net}");
            if !emit_small_lut(d, &cell, net, expr) {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }
        let mut bit_i = 0usize;
        for word in 0..depth {
            let mnet = format!("{mem}_we{word}");
            for bit in 0..width {
                let next = if bit >= data_w {
                    Expr::Const(false)
                } else {
                    Expr::Var(bit_name(&plan.data, data_w, bit))
                };
                let qn = unpacked_word_q(&mem, word, width, bit);
                let mux = word_ce_mux(Expr::Var(mnet.clone()), next, Expr::Var(qn.clone()));
                let lut = format!("u_vwl{bit_i}");
                let dnet = format!("u_vwd{bit_i}");
                if !emit_small_lut(d, &lut, &dnet, &mux) {
                    ok = false;
                    break;
                }
                let ff = format!("u_vwff{bit_i}");
                d.add_cell(&ff, CellKind::Hff);
                d.connect(clk, &ff, "CLK");
                d.connect(&dnet, &ff, "D");
                d.connect(&qn, &ff, "Q");
                bit_i += 1;
            }
            if !ok {
                break;
            }
        }
        if ok && bit_i == depth.saturating_mul(width) {
            lowered.insert(mem);
        }
    }
    lowered
}

fn synth_rtl(rtl: &Rtl) -> Result<Design, String> {
    let mut d = Design::new(&rtl.module);
    for (n, dir, _) in &rtl.ports {
        // A read-only inout is a load-enable input, not an output pad and
        // not a made-up bus. Timing still sees a plain input.
        let dir = if matches!(dir, PortDir::Inout) {
            PortDir::In
        } else {
            *dir
        };
        d.add_port(n, dir);
    }
    // User clock is the posedge/negedge name when that port exists.
    // Otherwise the historical first-`clk`/first-input pick (gold counter).
    let clk_owned = edge_clk_of(&rtl.module)
        .filter(|c| {
            rtl.ports
                .iter()
                .any(|(n, dir, _)| n == c && *dir == PortDir::In)
        })
        .or_else(|| {
            rtl.ports
                .iter()
                .find(|(n, dir, _)| *dir == PortDir::In && n == "clk")
                .or_else(|| rtl.ports.iter().find(|(_, dir, _)| *dir == PortDir::In))
                .map(|(n, _, _)| n.clone())
        })
        .unwrap_or_else(|| "clk".into());
    let clk = clk_owned.as_str();

    // `$display` / X/Z compare is a simulator model. Do not invent LUTs or MACs.
    if sim_only_for(&rtl.module) {
        d.attrs.set("SIM_ONLY", "1");
        d.attrs.set("NO_BODY", "1");
        return Ok(d);
    }

    // FM-HEL-10m-0835: after `hang_diag flatten`, 15011 (assigns, no nbas)
    // and 14777 (wide unpacked nbas) stall in the bit-blast. Name that path
    // and stop. One diagnostic. No invented LUT, no closed WNS.
    if let Some(sig) = flatten_leftover_signal(rtl) {
        note_flatten_cap(&rtl.module, &sig);
        d.attrs.set("FLATTEN_CAP", "1");
        return Ok(d);
    }

    // Flatten NBAs into per-bit (name_bit, expr)
    // FM-HEL-HANG: hard cap bit-blast work (Ibex synth_sv_path hung after CORPUS).
    // Linear visit budget. Add/cmp hang guards stay elsewhere (Ibex).
    rexpr_bit_budget_reset(400_000);
    let mut reg_bits: Vec<(String, Expr)> = Vec::new();
    let mut n_mac = 0usize;
    let mut n_bram = 0usize;
    // Bounded unpacked shift (const word index, small depth×width) → Hffs.
    // A variable-index read is not expanded here.
    let (unpacked_bits, mut lowered_unpacked) = lower_unpacked_clocked_words(rtl);
    reg_bits.extend(unpacked_bits);
    // Const-index word add (`res[0] <= a+b`) → ripple adder into Hffs.
    // Not a MAC, and not the generic cone (that left 2 bits and a wide_cone).
    lowered_unpacked.extend(lower_const_word_adds(&mut d, rtl, clk, &lowered_unpacked));
    // cycles=2..4: later words are Hff shifts of res[i-1], not a new add.
    // Larger or non-constant: word_pipeline_cap, no extra stages, no closed WNS.
    lowered_unpacked.extend(lower_word_pipeline(&mut d, rtl, clk, &lowered_unpacked));
    // Variable-index write (depth≤32, width≤16) → Hffs on the user's clock.
    // The read stays a single variable_index_read; do not walk a wide cone.
    lowered_unpacked.extend(lower_var_index_words(&mut d, rtl, clk));
    for (lhs, bit, rhs) in &rtl.nbas {
        if sig_depth(rtl, lhs) > 0 {
            continue;
        }
        if is_mac_rhs(rhs) {
            emit_mac27(&mut d, format!("u_mac{n_mac}"), rhs, lhs);
            n_mac += 1;
            continue;
        }
        let w = sig_width(rtl, lhs);
        if let Some(b) = bit {
            let e = if rexpr_is_plus_one(rhs) {
                Some(inc_bit_expr(lhs, w, *b))
            } else {
                rexpr_to_bit(rhs, rtl, 0).ok()
            };
            if let Some(e) = e {
                reg_bits.push((bit_name(lhs, w, *b), e));
            }
        } else if rexpr_is_plus_one(rhs) && rexpr_ident(rhs).as_deref() == Some(lhs.as_str()) {
            for i in 0..w {
                reg_bits.push((bit_name(lhs, w, i), inc_bit_expr(lhs, w, i)));
            }
        } else if let RExpr::Mux(cond, t, f) = rhs {
            if rexpr_is_plus_one(f)
                && rexpr_ident(f).as_deref() == Some(lhs.as_str())
                && matches!(t.as_ref(), RExpr::Const { val: 0, .. })
            {
                for i in 0..w {
                    let inc = inc_bit_expr(lhs, w, i);
                    let Ok(c) = rexpr_to_bit(cond, rtl, 0) else { break; };
                    reg_bits.push((
                        bit_name(lhs, w, i),
                        Expr::And(Box::new(Expr::Not(Box::new(c))), Box::new(inc)),
                    ));
                }
            } else if rexpr_is_plus_one(f)
                && rexpr_ident(f).as_deref() == Some(lhs.as_str())
                && matches!(t.as_ref(), RExpr::Ident(s) if s == lhs)
            {
                // saturating / hold: if (cond) cnt <= cnt; else cnt <= cnt+1
                for i in 0..w {
                    let inc = inc_bit_expr(lhs, w, i);
                    let Ok(c) = rexpr_to_bit(cond, rtl, 0) else { break; };
                    let hold = Expr::Var(bit_name(lhs, w, i));
                    reg_bits.push((
                        bit_name(lhs, w, i),
                        Expr::Or(
                            Box::new(Expr::And(Box::new(c.clone()), Box::new(hold))),
                            Box::new(Expr::And(Box::new(Expr::Not(Box::new(c))), Box::new(inc))),
                        ),
                    ));
                }
            } else if rexpr_is_plus_one(t)
                && rexpr_ident(t).as_deref() == Some(lhs.as_str())
                && matches!(f.as_ref(), RExpr::Ident(s) if s == lhs)
            {
                // clock enable: if (en) cnt <= cnt+1
                for i in 0..w {
                    let inc = inc_bit_expr(lhs, w, i);
                    let Ok(c) = rexpr_to_bit(cond, rtl, 0) else { break; };
                    let hold = Expr::Var(bit_name(lhs, w, i));
                    reg_bits.push((
                        bit_name(lhs, w, i),
                        Expr::Or(
                            Box::new(Expr::And(Box::new(c.clone()), Box::new(inc))),
                            Box::new(Expr::And(Box::new(Expr::Not(Box::new(c))), Box::new(hold))),
                        ),
                    ));
                }
            } else {
                let bw = w.min(32);
                for i in 0..bw {
                    if let Ok(e) = rexpr_to_bit(rhs, rtl, i) {
                        reg_bits.push((bit_name(lhs, w, i), e));
                    }
                }
            }
        } else {
            let bw = w.min(32);
            for i in 0..bw {
                if let Ok(e) = rexpr_to_bit(rhs, rtl, i) {
                    reg_bits.push((bit_name(lhs, w, i), e));
                }
            }
        }
    }
    eprintln!("synth_rtl after nbas reg_bits={}", reg_bits.len());
    let mut mem_names: HashSet<String> = HashSet::new();
    for (lhs, _, _) in &rtl.nbas {
        if sig_depth(rtl, lhs) > 0 && !lowered_unpacked.contains(lhs) {
            mem_names.insert(lhs.clone());
        }
    }
    for k in rtl.mem_inits.keys() {
        mem_names.insert(k.clone());
    }
    for name in &mem_names {
        let cell = format!("u_bram{n_bram}");
        d.add_cell(&cell, CellKind::Bram18);
        let depth = sig_depth(rtl, name).max(
            rtl.mem_inits
                .get(name)
                .and_then(|m| m.keys().max().copied())
                .map(|a| a + 1)
                .unwrap_or(0),
        );
        let mut words = vec![0u64; depth.max(1).min(1024)];
        if let Some(init) = rtl.mem_inits.get(name) {
            for (addr, val) in init {
                if *addr < words.len() {
                    words[*addr] = *val as u64;
                }
            }
        }
        let hex = words
            .iter()
            .map(|w| format!("{w:x}"))
            .collect::<Vec<_>>()
            .join(",");
        let _ = d.set_cell_attr(&cell, "INIT", hex);
        n_bram += 1;
    }

    // Continuous assigns → comb LUT cones (pure-comb modules e.g. mux).
    // Ident/Bit/Range stay nets (IOB passthrough) so gold sequential WNS is
    // not broken by buffer LUTs — that is mapping a wire as a wire, not a skip.
    let mut comb_bits: Vec<(String, Expr)> = Vec::new();
    // Relational wire-assigns that cannot map must name themselves. Do not
    // spend the one wide_cone line on them — clocked cones still report that.
    let mut rel_sig: HashMap<String, String> = HashMap::new();
    let mut n_rel = 0usize;
    // A posedge clock that is a mux of two clocks is not a data LUT and not
    // one user clock. Name it; do not close WNS on a leftover input.
    // A posedge clock that is an AND/OR of a clock and an enable is the same
    // kind of honesty: one clock_gate, no data LUT, not a closed WNS.
    let clock_mux_sigs = note_clock_muxes(rtl);
    let clock_gate_sigs = note_clock_gates(rtl, &clock_mux_sigs);
    // `assign q = res[<constant>]` is a net alias to that word, not a
    // variable-index read and not an unmapped assign. A signal index is not
    // recorded here — that stays `variable_index_read`.
    let mut word_alias: Vec<(String, String, usize)> = Vec::new();
    // `{bus, {K{1'b0}}}` K=1..8 is a wire alignment (zeros on the low
    // side), not a 16-PI cone and not a LUT. A nested concat inside an
    // add is not this form — that add stays wide_cone.
    let mut concat_align: Vec<(String, String, usize)> = Vec::new();
    for (lhs, bit, rhs) in &rtl.assigns {
        if clock_mux_sigs.contains(lhs) || clock_gate_sigs.contains(lhs) {
            continue;
        }
        if bit.is_none() {
            if let Some(kind) = zero_fill_concat(rhs) {
                match kind {
                    Ok((bus, zeros)) => {
                        eprintln!("synth_rtl concat_align signal={lhs} zeros={zeros}");
                        concat_align.push((lhs.clone(), bus, zeros));
                    }
                    Err(()) => {
                        note_assign_not_lowered(&rtl.module, lhs);
                    }
                }
                continue;
            }
        }
        if bit.is_none() {
            if let Some((mem, word)) = const_unpacked_word(rhs, rtl) {
                // Cap: do not alias q onto a stage that was not invented.
                if word_pipeline_cap_for(&rtl.module) {
                    continue;
                }
                word_alias.push((lhs.clone(), mem, word));
                continue;
            }
        }
        match rhs {
            RExpr::Ident(_) | RExpr::Bit(_, _) | RExpr::Range(_, _, _) => continue,
            _ => {}
        }
        // width<=32 `assign y = bus + K` (K in 1..=16) is a ripple from that
        // constant: carry on the low bits, not a 32-PI cone and not a second
        // 32-bit addend bus. No clock, no MAC. Wider than 32, or a skipped
        // bit, stays unlowered — do not invent a shorter bus.
        if bit.is_none() {
            if let Some((bus, k)) = packed_add_const(rhs, rtl) {
                if rexpr_unknown_name(rhs, rtl).is_none() {
                    let w = sig_width(rtl, lhs).max(1);
                    if w > 32 || !emit_ripple_add_const(&mut d, rtl, lhs, w, &bus, k) {
                        note_assign_not_lowered(&rtl.module, lhs);
                    }
                    continue;
                }
            }
        }
        // width<=32 `assign y = bus - K` (K in 1..=16, bus on the left) is a
        // borrow ripple from that constant, not a second operand bus and not
        // a 32-PI cone. No clock, no MAC. Wider than 32, or a skipped bit,
        // stays unlowered — do not invent a shorter bus. `K - bus` is not
        // this form.
        if bit.is_none() {
            if let Some((bus, k)) = packed_sub_const(rhs, rtl) {
                if rexpr_unknown_name(rhs, rtl).is_none() {
                    let w = sig_width(rtl, lhs).max(1);
                    if w > 32 || !emit_ripple_sub_const(&mut d, rtl, lhs, w, &bus, k) {
                        note_assign_not_lowered(&rtl.module, lhs);
                    }
                    continue;
                }
            }
        }
        // width<=32 `assign sum = a + b` of two named buses is a ripple of
        // 1-bit full adders, not one 64-PI cone and not a MAC. No clock.
        // Wider than 32 stays unlowered — do not invent a shorter bus, and
        // do not walk the expression tree. A skipped bit stays incomplete.
        if bit.is_none() {
            if let Some((a_name, b_name)) = packed_add_pair(rhs, rtl) {
                if rexpr_unknown_name(rhs, rtl).is_none() {
                    let w = sig_width(rtl, lhs).max(1);
                    if w > 32 || !emit_ripple_add(&mut d, rtl, lhs, w, &a_name, &b_name) {
                        note_assign_not_lowered(&rtl.module, lhs);
                    }
                    continue;
                }
            }
        }
        // Variable-index read of a clocked unpacked array. Do not walk it
        // into a >16 PI / >96 AND cone. Write-side Hffs, if any, still time.
        // A constant word index was already aliased above.
        if rhs_reads_seq_mem(rhs, rtl) || rhs_unpacked_index(rhs, rtl) {
            // word_pipeline_cap is the one diagnostic for an unrolled bound.
            if !lowered_unpacked.is_empty() && !word_pipeline_cap_for(&rtl.module) {
                note_variable_index_read(&rtl.module, lhs);
            }
            continue;
        }
        // Comb multiply → DSP MAC (do not bitblast).
        if expr_contains_mul(rhs) {
            emit_mac27(&mut d, format!("u_mac{n_mac}"), rhs, lhs);
            n_mac += 1;
            continue;
        }
        // Unknown name is not const 0.
        if rexpr_unknown_name(rhs, rtl).is_some() {
            note_assign_not_lowered(&rtl.module, lhs);
            continue;
        }
        let rel = rexpr_is_rel(rhs);
        let w = sig_width(rtl, lhs);
        // `bus < const` / `bus >= const` (and their && / ||) is a test of the
        // bits that differ from the bound, not a 16-PI cone. `'h6000 <= x < 'h8000`
        // is bits [15:13]==011. Wider than a LUT6 stays assign_not_lowered.
        if rel && match bit {
            Some(0) => true,
            None => w <= 1,
            Some(_) => false,
        } {
            if let Some((init, pis)) = const_range_lut(rhs, rtl) {
                let bidx = (*bit).unwrap_or(0);
                let bn = bit_name(lhs, w, bidx);
                let lut = format!("u_rel{n_rel}");
                n_rel += 1;
                d.add_cell(&lut, CellKind::Lut6 { init });
                d.connect(&bn, &lut, "O");
                for (pin, pi) in pis.iter().enumerate() {
                    d.connect(pi, &lut, format!("I{pin}"));
                }
                continue;
            }
        }
        let mut failed = false;
        if let Some(b) = bit {
            match rexpr_to_bit(rhs, rtl, 0) {
                Ok(e) => {
                    let bn = bit_name(lhs, w, *b);
                    if rel {
                        rel_sig.insert(bn.clone(), lhs.clone());
                    }
                    comb_bits.push((bn, e));
                }
                Err(_) => failed = true,
            }
        } else if rexpr_width_checked(rhs, rtl).is_none() {
            // Slice/concat does not fit (`hi - lo + 1` or accum sum). Do not
            // walk a bit and do not invent a LUT. Closed WNS is refused below.
            note_width_overflow_named(&rtl.module);
        } else {
            let rw = rexpr_width(rhs, rtl).min(w).max(1).min(256);
            let before = comb_bits.len();
            for i in 0..rw.min(w) {
                match rexpr_to_bit(rhs, rtl, i) {
                    Ok(e) => {
                        let bn = bit_name(lhs, w, i);
                        if rel {
                            rel_sig.insert(bn.clone(), lhs.clone());
                        }
                        comb_bits.push((bn, e));
                    }
                    Err(err) => {
                        failed = true;
                        if err.contains("width_overflow") {
                            comb_bits.truncate(before);
                            note_width_overflow_named(&rtl.module);
                            break;
                        }
                    }
                }
            }
        }
        if failed && rel {
            note_assign_not_lowered(&rtl.module, lhs);
        }
    }

    eprintln!("synth_rtl after assigns comb_bits={}", comb_bits.len());

    eprintln!(
        "synth_rtl reg_bits={} comb_bits={} mac={} bram={}",
        reg_bits.len(),
        comb_bits.len(),
        n_mac,
        n_bram
    );
    // FM-HEL-TOP: O(1) keep/mark_debug lookup. Prior per-bit scan of all
    // signals*width allocated bit_name strings (Ibex: ~2.3k regs x 1.2k sigs)
    // and dominated synth_rtl (~4.6s of ~5.3s under debug caps path).
    let mut keep_bits: HashSet<String> = HashSet::new();
    let mut md_bits: HashSet<String> = HashSet::new();
    for s in &rtl.signals {
        if !(s.keep || s.mark_debug) {
            continue;
        }
        if s.keep {
            keep_bits.insert(s.name.clone());
        }
        if s.mark_debug {
            md_bits.insert(s.name.clone());
        }
        for b in 0..s.width {
            let bn = bit_name(&s.name, s.width, b);
            if s.keep {
                keep_bits.insert(bn.clone());
            }
            if s.mark_debug {
                md_bits.insert(bn);
            }
        }
    }
    let single_q = reg_bits.len() == 1 && reg_bits[0].0 == "q";
    // FM-HEL-10m-0854: one wide-cone diagnostic per module, then finish.
    // Do not reprint node_count per bit, and do not map cones past the cap.
    let mut wide_capped = false;
    let mut wide_luts = 0usize;
    for (i, (bitn, expr)) in reg_bits.iter().enumerate() {
        // FM-HEL-HANG: exponential Add/cmp Expr trees explode in Aig::from_expr.
        if expr_node_count(expr) > 8_000 {
            emit_wide_cone(&rtl.module, bitn, &mut wide_capped);
            continue;
        }
        if wide_capped && cone_pi_exceeds(expr, 6) {
            continue;
        }
        let aig = Aig::from_expr(expr);
        if aig.pis.len() > 6 {
            if wide_capped
                || wide_aig_over_cap(&aig)
                || wide_luts.saturating_add(aig.ands.len()) > WIDE_CONE_MODULE_LUT_CAP
            {
                emit_wide_cone(&rtl.module, bitn, &mut wide_capped);
                continue;
            }
            let (ff, qnet) = if single_q {
                ("u_ff".to_string(), "q".to_string())
            } else {
                (format!("u_ff{i}"), bitn.clone())
            };
            let wide = map_wide_cone(&mut d, &aig, &format!("u_w{i}_"));
            wide_luts = wide_luts.saturating_add(aig.ands.len());
            d.add_cell(&ff, CellKind::Hff);
            d.connect(clk, &ff, "CLK");
            d.connect(&wide, &ff, "D");
            d.connect(&qnet, &ff, "Q");
            continue;
        }
        let init = aig.flowmap_lut6();
        let (lut, ff, dnet, qnet) = if single_q {
            ("u_lut".into(), "u_ff".into(), "d".into(), "q".into())
        } else {
            (
                format!("u_lut{i}"),
                format!("u_ff{i}"),
                format!("d{i}"),
                bitn.clone(),
            )
        };
        d.add_cell(&lut, CellKind::Lut6 { init });
        d.add_cell(&ff, CellKind::Hff);
        d.connect(clk, &ff, "CLK");
        d.connect(&dnet, &lut, "O");
        d.connect(&dnet, &ff, "D");
        d.connect(&qnet, &ff, "Q");
        for (pin, pi) in aig.pis.iter().enumerate() {
            d.connect(pi, &lut, format!("I{pin}"));
        }
        if keep_bits.contains(bitn) {
            let _ = d.dont_touch(&ff);
        }
        if md_bits.contains(bitn) {
            let _ = d.mark_debug(&qnet);
        }
    }

    for (i, (bitn, expr)) in comb_bits.iter().enumerate() {
        if i >= 256 {
            note_skip(format!(
                "diagnostic assign_cap signal={} (assign-cap 256; remaining assigns not a LUT)",
                bitn
            ));
            break;
        }
        if expr_node_count(expr) > 2_000 {
            // ≤6-PI boolean (Problem4) is still a real LUT6. A wider cone is
            // the VGA node_count storm: one wide_cone line, do not eval.
            if !wide_capped && !cone_pi_exceeds(expr, 6) {
                if let Some((init, pis)) = lut6_from_bool_cone(expr) {
                    let lut = format!("u_clut{i}");
                    d.add_cell(&lut, CellKind::Lut6 { init });
                    d.connect(bitn, &lut, "O");
                    for (pin, pi) in pis.iter().enumerate() {
                        d.connect(pi, &lut, format!("I{pin}"));
                    }
                    continue;
                }
            }
            if let Some(sig) = rel_sig.get(bitn) {
                note_assign_not_lowered(&rtl.module, sig);
                continue;
            }
            emit_wide_cone(&rtl.module, bitn, &mut wide_capped);
            continue;
        }
        if wide_capped && cone_pi_exceeds(expr, 6) {
            if let Some(sig) = rel_sig.get(bitn) {
                note_assign_not_lowered(&rtl.module, sig);
            }
            continue;
        }
        let aig = Aig::from_expr(expr);
        if aig.pis.len() > 6 {
            if wide_capped
                || wide_aig_over_cap(&aig)
                || wide_luts.saturating_add(aig.ands.len()) > WIDE_CONE_MODULE_LUT_CAP
            {
                if let Some(sig) = rel_sig.get(bitn) {
                    note_assign_not_lowered(&rtl.module, sig);
                    continue;
                }
                emit_wide_cone(&rtl.module, bitn, &mut wide_capped);
                continue;
            }
            let wide = map_wide_cone(&mut d, &aig, &format!("u_cw{i}_"));
            wide_luts = wide_luts.saturating_add(aig.ands.len());
            // Alias wide cone output onto the assign net name.
            d.add_cell(format!("u_cbuf{i}"), CellKind::Lut6 { init: 0x2 });
            d.connect(&wide, format!("u_cbuf{i}"), "I0");
            d.connect(bitn, format!("u_cbuf{i}"), "O");
            continue;
        }
        let init = aig.flowmap_lut6();
        let lut = format!("u_clut{i}");
        d.add_cell(&lut, CellKind::Lut6 { init });
        d.connect(bitn, &lut, "O");
        for (pin, pi) in aig.pis.iter().enumerate() {
            d.connect(pi, &lut, format!("I{pin}"));
        }
    }

    // A refused cone is not a closed design, even if leftover FFs remain.
    // variable_index_read does not set this: write-side Hffs may still time.
    if wide_capped {
        d.attrs.set("WIDE_CONE", "1");
    }
    if assign_not_lowered_for(&rtl.module) {
        d.attrs.set("ASSIGN_NOT_LOWERED", "1");
    }
    if generate_not_lowered_for(&rtl.module) {
        d.attrs.set("GENERATE_NOT_LOWERED", "1");
    }
    if width_overflow_for(&rtl.module) {
        d.attrs.set("WIDTH_OVERFLOW", "1");
    }
    if word_pipeline_cap_for(&rtl.module) {
        d.attrs.set("WORD_PIPELINE_CAP", "1");
    }
    if flatten_cap_for(&rtl.module) {
        d.attrs.set("FLATTEN_CAP", "1");
    }
    if clock_mux_for(&rtl.module) {
        d.attrs.set("CLOCK_MUX", "1");
    }
    if clock_gate_for(&rtl.module) {
        d.attrs.set("CLOCK_GATE", "1");
    }
    if gate_primitive_for(&rtl.module) {
        d.attrs.set("GATE_PRIMITIVE", "1");
    }
    if sim_only_for(&rtl.module) {
        d.attrs.set("SIM_ONLY", "1");
        d.attrs.set("NO_BODY", "1");
    }
    // Inout load enable: FF D must depend on it, or one diagnostic and no WNS.
    if prove_inout_load_enables(&mut d, rtl) {
        d.attrs.set("INOUT_ENABLE_NOT_LOWERED", "1");
    }

    // Negedge-only always: one Hff on that clock, or one named diagnostic.
    // Do not walk a cone. Falling versus rising is not a fake WNS.
    lower_negedge_hffs(&mut d, rtl);

    // Clocked always either already became an Hff on the user's clock, or
    // one sequential_not_lowered. wide_cone already finished this module.
    finish_seq_honesty(&d, &rtl.module, wide_capped);

    // Output IOBs from assigns.
    // FM-HEL-TOP: under skip_comb_assigns, AXI/out continuous assigns have no
    // mapped LUT/FF drivers. Emitting them fills the pack then iob_trim → 0,
    // blocking the FF→PAD fallback (bare ysyx_ibex IOB=0). Prefer NBA-
    // registered top outs; else last FF → first Out.
    let mut iob_n = 0usize;
    let emit_iob = |d: &mut Design, iob_n: &mut usize, qnet: &str, pad: &str| {
        let iob = if *iob_n == 0 {
            "u_iob".to_string()
        } else {
            format!("u_iob{iob_n}")
        };
        *iob_n += 1;
        d.add_cell(&iob, CellKind::IobOut);
        d.connect(qnet, &iob, "I");
        d.connect(pad, &iob, "PAD");
    };
    for (lhs, bit, rhs) in &rtl.assigns {
        let is_out = rtl
            .ports
            .iter()
            .any(|(n, dir, _)| n == lhs && *dir == PortDir::Out);
        if !is_out {
            continue;
        }
        // Constant word read: q is that word. Same nets as the Hff Q, no buffer LUT.
        if bit.is_none() {
            if let Some((_, mem, word)) = word_alias.iter().find(|(n, _, _)| n == lhs) {
                let ow = sig_width(rtl, lhs);
                let mw = sig_width(rtl, mem).max(1);
                for i in 0..ow.min(mw).min(256) {
                    let qnet = unpacked_word_q(mem, *word, mw, i);
                    emit_iob(&mut d, &mut iob_n, &qnet, lhs);
                }
                continue;
            }
            // Zero-fill concat: high bits are the bus, low bits are 0.
            // No buffer LUT. Zero bits have no source wire.
            if let Some((_, bus, zeros)) = concat_align.iter().find(|(n, _, _)| n == lhs) {
                let ow = sig_width(rtl, lhs);
                let bw = sig_width(rtl, bus).max(1);
                for i in *zeros..ow.min(256) {
                    let src = i - zeros;
                    if src >= bw {
                        break;
                    }
                    let qnet = bit_name(bus, bw, src);
                    emit_iob(&mut d, &mut iob_n, &qnet, lhs);
                }
                continue;
            }
        }
        let w = sig_width(rtl, lhs);
        if bit.is_none() && w > 1 {
            for i in 0..w.min(256) {
                // Ident/range passthrough: the driver is the RHS bit, not a
                // buffer LUT. Logic assigns drive the LHS bit name.
                let qnet = match rhs {
                    RExpr::Ident(s) => bit_name(s, sig_width(rtl, s), i),
                    RExpr::Range(s, lo, hi) => {
                        let idx = lo + i;
                        if idx <= *hi {
                            bit_name(s, sig_width(rtl, s), idx)
                        } else {
                            bit_name(lhs, w, i)
                        }
                    }
                    _ => bit_name(lhs, w, i),
                };
                emit_iob(&mut d, &mut iob_n, &qnet, lhs);
            }
            continue;
        }
        let qnet = if let Some(b) = bit {
            bit_name(lhs, w, *b)
        } else if let Ok((q, _)) = drive_target(rhs, rtl) {
            q
        } else {
            // 1-bit comb assign (not a wire name): LUT already drives lhs.
            bit_name(lhs, w, 0)
        };
        emit_iob(&mut d, &mut iob_n, &qnet, lhs);
    }
    if iob_n == 0 {
        // default: last register bit to first output
        if let (Some((n, _, _)), Some((qnet, _))) = (
            rtl.ports.iter().find(|(_, dir, _)| *dir == PortDir::Out),
            reg_bits.last(),
        ) {
            d.add_cell("u_iob", CellKind::IobOut);
            d.connect(qnet, "u_iob", "I");
            d.connect(n, "u_iob", "PAD");
        }
    }
    Ok(d)
}

fn rewrite_rexpr(e: &RExpr, subst: &HashMap<String, String>) -> RExpr {
    let id = |s: &str| subst.get(s).cloned().unwrap_or_else(|| s.to_string());
    match e {
        RExpr::Const { val, width, care } => RExpr::Const {
            val: *val,
            width: *width,
            care: *care,
        },
        RExpr::Ident(s) => RExpr::Ident(id(s)),
        RExpr::Bit(s, i) => RExpr::Bit(id(s), *i),
        RExpr::Range(s, lo, hi) => RExpr::Range(id(s), *lo, *hi),
        RExpr::IndexPart {
            name,
            base,
            width,
            ascending,
        } => RExpr::IndexPart {
            name: id(name),
            base: Box::new(rewrite_rexpr(base, subst)),
            width: *width,
            ascending: *ascending,
        },
        RExpr::WordAt { addr, data } => RExpr::WordAt {
            addr: Box::new(rewrite_rexpr(addr, subst)),
            data: Box::new(rewrite_rexpr(data, subst)),
        },
        RExpr::Concat(parts) => RExpr::Concat(
            parts.iter().map(|p| rewrite_rexpr(p, subst)).collect(),
        ),
        RExpr::Shr(a, b) => RExpr::Shr(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Ashr(a, b) => RExpr::Ashr(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::RedXor(x) => RExpr::RedXor(Box::new(rewrite_rexpr(x, subst))),
        RExpr::RedAnd(x) => RExpr::RedAnd(Box::new(rewrite_rexpr(x, subst))),
        RExpr::RedOr(x) => RExpr::RedOr(Box::new(rewrite_rexpr(x, subst))),
        RExpr::Not(x) => RExpr::Not(Box::new(rewrite_rexpr(x, subst))),
        RExpr::And(a, b) => RExpr::And(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Or(a, b) => RExpr::Or(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Xor(a, b) => RExpr::Xor(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Add(a, b) => RExpr::Add(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Sub(a, b) => RExpr::Sub(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Mul(a, b) => RExpr::Mul(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Mux(c, t, f) => RExpr::Mux(
            Box::new(rewrite_rexpr(c, subst)),
            Box::new(rewrite_rexpr(t, subst)),
            Box::new(rewrite_rexpr(f, subst)),
        ),
        RExpr::Eq(a, b) => RExpr::Eq(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Ne(a, b) => RExpr::Ne(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Lt(a, b) => RExpr::Lt(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
    }
}

fn elaborate_rtl(src: &Rtl, overrides: &HashMap<String, u128>) -> Result<Rtl, String> {
    if overrides.is_empty() {
        return Ok(src.clone());
    }
    if src.toks.is_empty() {
        return Err(format!("module {} has no tokens to re-elaborate", src.module));
    }
    let mut p = P {
        t: &src.toks,
        i: 0,
        params: overrides.clone(),
        widths: HashMap::new(),
    };
    parse_one_module(&mut p)
}

fn inst_overrides(inst: &Inst, child: &Rtl) -> HashMap<String, u128> {
    let mut ov = HashMap::new();
    for (k, v) in &inst.params {
        if let Some(rest) = k.strip_prefix('#') {
            if let Ok(i) = rest.parse::<usize>() {
                if let Some((name, _)) = child.params.get(i) {
                    ov.insert(name.clone(), *v);
                    continue;
                }
            }
        }
        ov.insert(k.clone(), *v);
    }
    ov
}

#[allow(dead_code)] // intentional: thin wrapper over flatten_module_ov
fn flatten_module(mods: &HashMap<String, Rtl>, name: &str) -> Result<Rtl, String> {
    flatten_module_ov(mods, name, &HashMap::new())
}

fn flatten_module_ov(
    mods: &HashMap<String, Rtl>,
    name: &str,
    overrides: &HashMap<String, u128>,
) -> Result<Rtl, String> {
    flatten_module_ov_vis(mods, name, overrides, &mut HashSet::new())
}

fn flatten_module_ov_vis(
    mods: &HashMap<String, Rtl>,
    name: &str,
    overrides: &HashMap<String, u128>,
    visiting: &mut HashSet<String>,
) -> Result<Rtl, String> {
    if !visiting.insert(name.to_string()) {
        let proto = mods
            .get(name)
            .ok_or_else(|| format!("unknown module {name}"))?;
        let mut src = elaborate_rtl(proto, overrides)?;
        src.insts.clear();
        return Ok(src);
    }
    let proto = mods
        .get(name)
        .ok_or_else(|| format!("unknown module {name}"))?;
    let src = elaborate_rtl(proto, overrides)?;
    let mut out = Rtl {
        module: src.module.clone(),
        ports: src.ports.clone(),
        signals: src.signals.clone(),
        nbas: src.nbas.clone(),
        assigns: src.assigns.clone(),
        insts: Vec::new(),
        params: src.params.clone(),
        toks: src.toks.clone(),
        mem_inits: src.mem_inits.clone(),
    };
    for inst in &src.insts {
        let Some(child_proto) = mods.get(&inst.module) else {
            note_skip(format!(
                "diagnostic unknown_instance module={} inst={} child={} (child body absent; not a LUT)",
                name, inst.name, inst.module
            ));
            continue;
        };
        let ov = inst_overrides(inst, child_proto);
        let child = flatten_module_ov_vis(mods, &inst.module, &ov, visiting)?;
        let prefix = format!("{}_", inst.name);
        let mut subst: HashMap<String, String> = HashMap::new();
        for s in &child.signals {
            subst.insert(s.name.clone(), format!("{prefix}{}", s.name));
        }
        for (i, (pname, _, _)) in child.ports.iter().enumerate() {
            if let Some((_, net)) = inst.conns.iter().find(|(p, _)| p == pname)
                .or_else(|| inst.conns.iter().find(|(p, _)| p == &format!("#{i}")))
            {
                subst.insert(pname.clone(), net.clone());
            }
        }
        for s in &child.signals {
            let mapped = subst.get(&s.name).cloned().unwrap();
            if !out.signals.iter().any(|x| x.name == mapped) {
                out.signals.push(Signal {
                    name: mapped,
                    width: s.width,
                    depth: s.depth,
                    keep: s.keep,
                    mark_debug: s.mark_debug,
                });
            }
        }
        for (lhs, bit, rhs) in &child.nbas {
            let lhs = subst.get(lhs).cloned().unwrap_or_else(|| format!("{prefix}{lhs}"));
            out.nbas.push((lhs, *bit, rewrite_rexpr(rhs, &subst)));
        }
        for (lhs, bit, rhs) in &child.assigns {
            let lhs = subst.get(lhs).cloned().unwrap_or_else(|| format!("{prefix}{lhs}"));
            out.assigns.push((lhs, *bit, rewrite_rexpr(rhs, &subst)));
        }
    }
    visiting.remove(name);
    Ok(out)
}

fn inst_tree_len(
    mods: &HashMap<String, Rtl>,
    name: &str,
    cap: usize,
    visiting: &mut HashSet<String>,
) -> usize {
    if !visiting.insert(name.to_string()) {
        return 0;
    }
    let Some(rtl) = mods.get(name) else {
        visiting.remove(name);
        return 0;
    };
    let mut n = rtl.insts.len();
    if n >= cap {
        visiting.remove(name);
        return n;
    }
    for inst in &rtl.insts {
        n = n.saturating_add(inst_tree_len(mods, &inst.module, cap, visiting));
        if n >= cap {
            break;
        }
    }
    visiting.remove(name);
    n
}

fn tree_has_rtl_body(
    mods: &HashMap<String, Rtl>,
    name: &str,
    visiting: &mut HashSet<String>,
) -> bool {
    if !visiting.insert(name.to_string()) {
        return false;
    }
    let Some(rtl) = mods.get(name) else {
        return false;
    };
    if !rtl.nbas.is_empty() || !rtl.assigns.is_empty() {
        return true;
    }
    for inst in &rtl.insts {
        if tree_has_rtl_body(mods, &inst.module, visiting) {
            return true;
        }
    }
    false
}

fn synth_from_parsed(mods: Vec<Rtl>) -> Result<Design, String> {
    synth_from_parsed_top(mods, None, &HashMap::new())
}

fn synth_from_parsed_top(
    mods: Vec<Rtl>,
    top: Option<&str>,
    overrides: &HashMap<String, u128>,
) -> Result<Design, String> {
    let map: HashMap<String, Rtl> = mods.iter().map(|m| (m.module.clone(), m.clone())).collect();
    let instantiated: HashMap<String, ()> = mods
        .iter()
        .flat_map(|m| m.insts.iter().map(|i| (i.module.clone(), ())))
        .collect();
    let top_name = if let Some(name) = top {
        if !map.contains_key(name) {
            return Err(format!("unknown module {name}"));
        }
        name.to_string()
    } else {
        mods.iter()
            .rev()
            .find(|m| !instantiated.contains_key(&m.module))
            .or_else(|| mods.last())
            .ok_or_else(|| "no top module".to_string())?
            .module
            .clone()
    };
    let t_syn = std::time::Instant::now();
    let t_flat = std::time::Instant::now();
    // Leaf designs (no instances) still flatten + synth_rtl, so gold counter
    // mapping is the same call. Fat instance trees must not flatten: that
    // re-enters uncalled functions and copies every child into one Rtl.
    let top_rtl = map.get(&top_name).expect("top");
    let fat = !top_rtl.insts.is_empty()
        && inst_tree_len(&map, &top_name, 64, &mut HashSet::new()) >= 64;
    let (flat_nbas, flat_assigns, mut d) = if fat {
        eprintln!(
            "hang_diag assemble module={} insts={} reason=skip_flatten",
            top_name,
            top_rtl.insts.len()
        );
        let d = assemble_module(&map, &top_name, &mut HashSet::new())?;
        let body = tree_has_rtl_body(&map, &top_name, &mut HashSet::new());
        (usize::from(body), 0usize, d)
    } else {
        let flat = flatten_module_ov(&map, &top_name, overrides)?;
        eprintln!(
            "hang_diag flatten nbas={} assigns={} signals={} ms={}",
            flat.nbas.len(),
            flat.assigns.len(),
            flat.signals.len(),
            t_flat.elapsed().as_millis()
        );
        let nbas = flat.nbas.len();
        let assigns = flat.assigns.len();
        let d = if top_rtl.insts.is_empty() {
            lower_own_cached(&flat)?
        } else {
            assemble_module(&map, &top_name, &mut HashSet::new())?
        };
        (nbas, assigns, d)
    };
    d.name = top_name.clone();
    eprintln!(
        "hang_diag synth_rtl cells={} ms={}",
        d.cells.len(),
        t_syn.elapsed().as_millis()
    );
    let n_logic = d
        .cells
        .iter()
        .filter(|c| {
            matches!(
                c.kind,
                CellKind::Lut6 { .. } | CellKind::Hff | CellKind::Mac27 | CellKind::Bram18
            )
        })
        .count();
    // Standing rule: ports-only shells and unknown vendor instances (no body
    // in this file/set) do not invent gates. cells stay 0; timing must not
    // report a closed WNS.
    if gate_primitive_for(&top_name) {
        d.attrs.set("GATE_PRIMITIVE", "1");
    }
    if sim_only_for(&top_name) {
        d.attrs.set("SIM_ONLY", "1");
        d.attrs.set("NO_BODY", "1");
        d.cells.clear();
        d.nets.clear();
    }
    if sim_only_for(&top_name) || (n_logic == 0 && flat_nbas == 0 && flat_assigns == 0) {
        d.attrs.set("NO_BODY", "1");
        eprintln!(
            "diagnostic no_body module={} cells=0 (ports only or unknown vendor instance; no gates invented)",
            d.name
        );
    } else if n_logic == 0
        && d.attrs.get("FLATTEN_CAP") != Some("1")
        && d.attrs.get("GENERATE_NOT_LOWERED") != Some("1")
        && d.attrs.get("ASSIGN_NOT_LOWERED") != Some("1")
    {
        eprintln!(
            "diagnostic no_logic module={} cells={} (behavioral body present; no LUT/FF mapped under hang guards)",
            d.name,
            d.cells.len()
        );
    }
    record_instances(&map, &top_name, &mut d, "");
    let n_luts = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    eprintln!(
        "synth_design {} cells={} luts={}",
        d.name,
        d.cells.len(),
        n_luts
    );
    Ok(d)
}

/// Keep the pre-flatten instance tree on HNF so the Hierarchy pane is not a cell list.
fn record_instances(mods: &HashMap<String, Rtl>, module: &str, d: &mut Design, pfx: &str) {
    record_instances_vis(mods, module, d, pfx, &mut HashSet::new());
}

fn record_instances_vis(
    mods: &HashMap<String, Rtl>,
    module: &str,
    d: &mut Design,
    pfx: &str,
    visiting: &mut HashSet<String>,
) {
    if !visiting.insert(module.to_string()) {
        return;
    }
    let Some(rtl) = mods.get(module) else {
        visiting.remove(module);
        return;
    };
    for inst in &rtl.insts {
        let name = if pfx.is_empty() {
            inst.name.clone()
        } else {
            format!("{pfx}{}", inst.name)
        };
        d.instances.push(helion_ir::Instance {
            name: name.clone(),
            module: inst.module.clone(),
            conns: inst.conns.clone(),
            attrs: helion_ir::Attrs::default(),
        });
        record_instances_vis(mods, &inst.module, d, &format!("{name}_"), visiting);
    }
    visiting.remove(module);
}

pub fn synth_sv(source: &str, origin: &str) -> Result<Design, String> {
    let t_parse = std::time::Instant::now();
    let origin_path = Path::new(origin);
    let base = origin_path.parent().filter(|d| !d.as_os_str().is_empty() && d.exists());
    let expanded = if let Some(dir) = base {
        expand_includes(source, dir)
    } else {
        source.to_string()
    };
    let pre = preprocess_sv(&strip_comments(&expanded));
    let mut mods = parse_source(&pre)?;
    if let Some(dir) = base {
        let have: HashSet<String> = mods.iter().map(|m| m.module.clone()).collect();
        let mut missing: HashSet<String> = HashSet::new();
        for m in &mods {
            for inst in &m.insts {
                if !have.contains(&inst.module) {
                    missing.insert(inst.module.clone());
                }
            }
        }
        if !missing.is_empty() {
            if let Ok(extra) = sibling_modules(dir, &missing, origin_path) {
                mods.extend(extra);
            }
        }
    }
    eprintln!(
        "hang_diag parse mods={} bytes={} ms={}",
        mods.len(),
        source.len(),
        t_parse.elapsed().as_millis()
    );
    let stem = Path::new(origin)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| mods.iter().any(|m| m.module == *s));
    synth_from_parsed_top(mods, stem, &HashMap::new())
}

/// Module names in one SV file (helion-sv parse). UG900 SIM_TOP candidates.
pub fn list_sv_modules(source: &str) -> Result<Vec<String>, String> {
    list_sv_modules_origin(source, "t.sv")
}

/// Module names in one SV file, with origin for sv-parser diagnostics.
pub fn list_sv_modules_origin(source: &str, origin: &str) -> Result<Vec<String>, String> {
    let _ = origin;
    let pre = preprocess_sv(&strip_comments(source));
    Ok(parse_source(&pre)?
        .into_iter()
        .map(|m| m.module)
        .collect())
}

/// Module names from a path on disk (helion-sv parse).
pub fn list_sv_modules_path(path: &Path) -> Result<Vec<String>, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    list_sv_modules_origin(&src, &path.display().to_string())
}

/// UG900 Compilation: sv-parser + helion-sv module list (not xvlog).
pub fn compile_sv(
    source: &str,
    origin: &str,
    opts: &SvCompileOpts,
) -> Result<Vec<String>, String> {
    let _ = (origin, opts);
    Ok(parse_source(source)?
        .into_iter()
        .map(|m| m.module)
        .collect())
}

pub fn compile_sv_path(path: &Path, opts: &SvCompileOpts) -> Result<Vec<String>, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    compile_sv(&src, &path.display().to_string(), opts)
}

fn elab_report(d: &Design) -> SvElabReport {
    SvElabReport {
        top: d.name.clone(),
        cells: d.cells.len(),
        luts: d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count(),
        ffs: d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Hff))
            .count(),
    }
}

/// UG900 Elaboration: helion-sv flatten + FlowMap snapshot (not xelab).
pub fn elaborate_sv(
    source: &str,
    origin: &str,
    top: Option<&str>,
    params: &HashMap<String, u128>,
    opts: &SvCompileOpts,
) -> Result<(Design, SvElabReport), String> {
    let _ = (origin, opts);
    let d = synth_from_parsed_top(parse_source(source)?, top, params)?;
    let report = elab_report(&d);
    Ok((d, report))
}

pub fn elaborate_sv_path(
    path: &Path,
    top: Option<&str>,
    params: &HashMap<String, u128>,
    opts: &SvCompileOpts,
) -> Result<(Design, SvElabReport), String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    elaborate_sv(&src, &path.display().to_string(), top, params, opts)
}

pub fn elaborate_sv_sources(
    files: &[(&str, &str)],
    top: Option<&str>,
    params: &HashMap<String, u128>,
    opts: &SvCompileOpts,
) -> Result<(Design, SvElabReport), String> {
    if files.is_empty() {
        return Err("no sources".into());
    }
    let t_parse = std::time::Instant::now();
    let mut all = String::new();
    for (origin, src) in files {
        let _ = (origin, opts);
        all.push_str(src);
        all.push('\n');
    }
    let mods = parse_source(&all)?;
    eprintln!(
        "hang_diag parse mods={} bytes={} files={} ms={}",
        mods.len(),
        all.len(),
        files.len(),
        t_parse.elapsed().as_millis()
    );
    let d = synth_from_parsed_top(mods, top, params)?;
    let report = elab_report(&d);
    Ok((d, report))
}

/// Elaborate many SV files together. Each file is parsed by sv-parser, then
/// modules are merged so a top in file B can instantiate a child defined in file A.
pub fn synth_sv_sources(files: &[(&str, &str)]) -> Result<Design, String> {
    if files.is_empty() {
        return Err("no sources".into());
    }
    let t_parse = std::time::Instant::now();
    let mut all = String::new();
    for (origin, src) in files {
        let _ = origin;
        all.push_str(src);
        all.push_str("
");
    }
    let mods = parse_source(&all)?;
    eprintln!(
        "hang_diag parse mods={} bytes={} files={} ms={}",
        mods.len(),
        all.len(),
        files.len(),
        t_parse.elapsed().as_millis()
    );
    synth_from_parsed(mods)
}

pub fn synth_sv_files(paths: &[&Path]) -> Result<Design, String> {
    let mut owned: Vec<(String, String)> = Vec::new();
    for p in paths {
        let text = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
        owned.push((p.display().to_string(), text));
    }
    let refs: Vec<(&str, &str)> = owned.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    synth_sv_sources(&refs)
}

pub fn synth_sv_path(path: &Path) -> Result<Design, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    synth_sv(&src, &path.display().to_string())
}

/// Same-directory wrapper: parent file instantiates a child whose body is
/// `child.v` next to it (no `` `include `` required).
pub fn synth_sv_path_with_siblings(path: &Path) -> Result<Design, String> {
    synth_sv_path(path)
}

pub fn lut_init_of(source: &str) -> Result<u64, String> {
    match synth_sv(source, "t.sv")?.cell("u_lut").unwrap().kind {
        CellKind::Lut6 { init } => Ok(init),
        _ => Err("no lut".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::INC4_INIT;

    fn wrap(rhs: &str) -> String {
        format!(
            "module blinky(input logic clk, output logic led);\n  logic q;\n  always_ff @(posedge clk) q <= {rhs};\n  assign led = q;\nendmodule\n"
        )
    }

    #[test]
    fn sv_parser_rejects_garbage() {
        assert!(parse_sv("this is not verilog @@@", "bad.sv").is_err());
    }

    #[test]
    fn inverter_and_buffer_and_zero_differ() {
        let inv = lut_init_of(&wrap("~q")).expect("inv");
        let buf = lut_init_of(&wrap("q")).expect("buf");
        let z = lut_init_of(&wrap("1'b0")).expect("zero");
        assert_eq!(inv, 0x5555_5555_5555_5555);
        assert_eq!(buf, 0xAAAA_AAAA_AAAA_AAAA);
        assert_eq!(z, 0);
        assert_ne!(inv, buf);
        assert_ne!(buf, z);
    }

    #[test]
    fn verilog_gate_primitives_are_not_luts() {
        // bufif/notif/and/or/nand/nor/xor/xnor/buf/not are not a Helion product.
        let src = r#"
module gates(a, b, y, z);
  input a, b;
  output y, z;
  wire n;
  not u1 (n, a);
  nand u2 (y, n, b);
  xor u3 (z, a, b);
endmodule
"#;
        let d = synth_sv(src, "gates.v").expect("gate synth");
        assert!(
            d.cells.is_empty(),
            "gate primitives must not become LUT cells, cells={:?}",
            d.cells
        );
        assert_eq!(d.attrs.get("NO_BODY"), Some("1"));
        assert_eq!(d.attrs.get("GATE_PRIMITIVE"), Some("1"));
    }

    #[test]
    fn sim_only_display_and_xz_compare_is_not_a_lut() {
        // Vendor PLL sim model: $display and === 1'bx. Not a counter, not a MAC.
        let src = r#"
module hardcopyii_n_cntr(clk,reset,cout,modulus);
  input clk;
  input reset;
  input [31:0] modulus;
  output cout;
  integer count;
  reg tmp_cout;
  initial count = 1;
  always @(reset or clk) begin
    if (reset) count = 1;
    else if (clk === 1'bx) $display("Warning : X");
    else if (count < modulus) count = count+1;
    else begin count = 1; tmp_cout = ~tmp_cout; end
    if (clk !== 1'bx) count = count;
  end
  assign cout = tmp_cout;
endmodule
"#;
        let d = synth_sv(src, "n_cntr.v").expect("sim_only");
        assert!(
            d.cells.is_empty(),
            "sim model must not invent LUT/MAC cells, cells={:?}",
            d.cells
        );
        assert_eq!(d.attrs.get("SIM_ONLY"), Some("1"));
        assert_eq!(d.attrs.get("NO_BODY"), Some("1"));
        assert!(
            !d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)),
            "sim model must not invent a MAC"
        );
    }

    fn bufif1_mux_is_one_gate_primitive_not_a_lut() {
        let src = r#"
module sky130_fd_sc_hdll__muxb16to1(Z,D,S);
  output Z;
  input  [15:0] D;
  input  [15:0] S;
  bufif1 bufif10(Z,!D[0],S[0]);
  bufif1 bufif11(Z,!D[1],S[1]);
endmodule
"#;
        let d = synth_sv(src, "muxb.v").expect("bufif1");
        assert!(d.cells.is_empty(), "bufif1 must not invent a LUT mux, cells={:?}", d.cells);
        assert_eq!(d.attrs.get("NO_BODY"), Some("1"));
        assert_eq!(d.attrs.get("GATE_PRIMITIVE"), Some("1"));
        assert!(d.instances.is_empty(), "bufif1 must not stay an unknown instance");
    }

    #[test]
    fn logikbench_mux_maps_luts() {
        let src = r#"
module mux #(parameter DW = 8, parameter N = 4)
(
    input [$clog2(N)-1:0] sel,
    input [N*DW-1:0] data,
    output [DW-1:0] out
);
   assign out[DW-1:0] = data[sel*DW +: DW];
endmodule
"#;
        let d = synth_sv(src, "mux").expect("synth");
        let cells = d.cells.len();
        assert!(cells > 0, "expected comb LUTs for mux, cells={cells}");
    }

    #[test]
    fn clog2_const_works() {
        assert_eq!(clog2_u(1), 0);
        assert_eq!(clog2_u(2), 1);
        assert_eq!(clog2_u(16), 4);
        assert_eq!(clog2_u(17), 5);
    }

    #[test]
    fn fsm_tiny_maps_state_and_out_regs() {
        // Minimal LogikBench parametric FSM shape: async reset, replication
        // concat, >> shift, reduction XOR, $clog2 localparam.
        let src = r#"
module fsm_tiny #(parameter STATES = 4, parameter DW = 4, parameter [31:0] SEED = 32'hA5A5A5A5)
(
    input clk,
    input nreset,
    input [DW-1:0] in,
    output reg [DW-1:0] out
);
   localparam STATE_WIDTH = $clog2(STATES);
   reg [STATE_WIDTH-1:0] current_state;
   reg [STATE_WIDTH-1:0] next_state;
   always @(posedge clk or negedge nreset)
     if (!nreset)
       current_state <= {STATE_WIDTH{1'b0}};
     else
       current_state <= next_state;
   always @(*)
     case (current_state)
       {STATE_WIDTH{1'b0}}: next_state = in[0] ? (STATES - 1) : 1'b1;
       ((STATES/2) - 1): next_state = (^in) ? {STATE_WIDTH{1'b1}} : {STATE_WIDTH{1'b0}};
       (STATES - 1): next_state = in ^ (SEED[STATE_WIDTH-1:0] >> 1);
       default: next_state = (current_state ^ SEED[STATE_WIDTH-1:0]) + in;
     endcase
   always @(posedge clk or negedge nreset)
     if (!nreset)
       out <= {DW{1'b0}};
     else
       out <= (current_state ^ (current_state >> 2)) + in;
endmodule
"#;
        let d = synth_sv(src, "fsm_tiny.sv").expect("fsm_tiny synth");
        let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        let luts = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count();
        eprintln!("fsm_tiny cells={} luts={luts} ffs={ffs}", d.cells.len());
        assert!(ffs >= 6, "expect state+out regs, ffs={ffs}");
        assert!(luts >= 4, "expect next-state/out LUTs, luts={luts}");
        assert!(d.cells.len() > 17, "must exceed pre-fix ~17 cell baseline, got {}", d.cells.len());
    }

    #[test]
    fn unary_reductions_band_bxor_bnand() {
        for (name, body) in [
            ("band", "assign out = &in;"),
            ("bxor", "assign out = ^in;"),
            ("bnand", "assign out = ~&in;"),
        ] {
            let src = format!(
                "module {name} #(parameter DW = 64) (input [DW-1:0] in, output out);\n  {body}\nendmodule\n"
            );
            let d = synth_sv(&src, &format!("{name}.sv")).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(
                !d.cells.is_empty(),
                "{name} must map cells>0, got {}",
                d.cells.len()
            );
        }
    }

    #[test]
    fn aig_flowmap_and() {
        let e = parse_expr("a&b").unwrap();
        let aig = Aig::from_expr(&e);
        assert_eq!(aig.pis.len(), 2);
        let init = aig.flowmap_lut6();
        // I0=a I1=b: AND is 1 only when addr & 3 == 3 → bits 3,7,11,...
        assert_eq!(init & 0b1111, 0b1000);
    }

    #[test]
    fn incrementer_matches_gold_init() {
        let src = r#"
module counter(input logic clk, output logic led);
  logic [3:0] cnt;
  always_ff @(posedge clk) cnt <= cnt + 1;
  assign led = cnt[3];
endmodule
"#;
        let d = synth_sv(src, "c.sv").unwrap();
        let inits = d.lut_inits();
        assert_eq!(inits, INC4_INIT.to_vec(), "synth incrementer INIT {inits:#x?}");
        assert_eq!(d.net_on("u_iob", "I"), Some("cnt_3"));
        assert_eq!(d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count(), 4);
    }

    #[test]
    fn incrementer_plus_one_b1() {
        let src = r#"
module counter(input logic clk, output logic led);
  logic [3:0] cnt;
  always_ff @(posedge clk) begin
    cnt <= cnt + 1'b1;
  end
  assign led = cnt[3];
endmodule
"#;
        let d = synth_sv(src, "c.sv").unwrap();
        assert_eq!(d.lut_inits(), INC4_INIT.to_vec());
    }

    #[test]
    fn reset_if_else_is_not_a_noop() {
        let src = r#"
module counter(input logic clk, input logic rst, output logic led);
  logic [3:0] cnt;
  always_ff @(posedge clk) begin
    if (rst) cnt <= 0;
    else cnt <= cnt + 1;
  end
  assign led = cnt[3];
endmodule
"#;
        let d = synth_sv(src, "r.sv").unwrap();
        let inits = d.lut_inits();
        assert_eq!(inits.len(), 4);
        assert_ne!(
            inits[0], INC4_INIT[0],
            "rst must occupy a LUT pin so INIT is not the bare incrementer"
        );
        assert!(d.ports.iter().any(|p| p.name == "rst"));
    }

    #[test]
    fn logikbench_mac_with_clear_en_infers_mac27() {
        let src = r#"
module mac #(parameter DW = 16, parameter OW = 40) (
  input clk, input clear, input en,
  input [DW-1:0] a, input [DW-1:0] b,
  output reg [OW-1:0] c
);
  always @(posedge clk) begin
    if (clear) c <= 0;
    else if (en) c <= c + a * b;
  end
endmodule
"#;
        let d = synth_sv(src, "mac.sv").expect("synth");
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)),
            "clear/en MAC must map Mac27, cells={}",
            d.cells.len()
        );
        assert_eq!(d.net_on("u_mac0", "A"), Some("a"));
        assert_eq!(d.net_on("u_mac0", "B"), Some("b"));
        assert_eq!(d.net_on("u_mac0", "C"), Some("c"));
        assert_eq!(d.net_on("u_mac0", "P"), Some("c"));
    }

    
    #[test]
    fn dependent_param_ow_equals_two_times_dw() {
        let src = r#"
module mul #(parameter DW = 8, parameter OW = 2 * DW) (
  input [DW-1:0] a, input [DW-1:0] b, output [OW-1:0] out
);
  assign out = a * b;
endmodule
"#;
        let d = synth_sv(src, "mul.sv").expect("synth");
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)),
            "OW=2*DW mul must map Mac27, cells={}",
            d.cells.len()
        );
    }


#[test]
    fn comb_mul_assign_infers_mac27() {
        let src = r#"
module mul #(parameter DW = 8, parameter OW = 2 * DW) (
  input [DW-1:0] a, input [DW-1:0] b, output [OW-1:0] out
);
  assign out[OW-1:0] = a[DW-1:0] * b[DW-1:0];
endmodule
"#;
        let d = synth_sv(src, "mul.sv").expect("synth");
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)),
            "comb a*b must map Mac27, cells={}",
            d.cells.len()
        );
        assert_eq!(d.net_on("u_mac0", "A"), Some("a"));
        assert_eq!(d.net_on("u_mac0", "B"), Some("b"));
        assert_eq!(d.net_on("u_mac0", "P"), Some("out"));
    }

    
    #[test]
    fn logikbench_mul_ranged_assign_infers_mac27() {
        let src = r#"
module mul #(parameter DW = 16, parameter OW = 2 * DW)
   (
    input [DW-1:0]  a,
    input [DW-1:0]  b,
    output [OW-1:0] out
    );
	assign out[OW-1:0] = a[DW-1:0] * b[DW-1:0];
endmodule
"#;
        let d = synth_sv(src, "mul.sv").expect("synth");
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)),
            "ranged comb mul must map Mac27, cells={}",
            d.cells.len()
        );
    }

    #[test]
    fn mul_infers_mac27() {
        let src = r#"
module mac(input logic clk, input logic [26:0] a, input logic [26:0] b, output logic [47:0] p);
  always_ff @(posedge clk) p <= a * b + 0;
endmodule
"#;
        let d = synth_sv(src, "m.sv").unwrap();
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)),
            "a*b must map to MAC27, cells {:?}",
            d.cells.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
        assert!(d.lut_inits().is_empty(), "DSP path must not bitblast a 27x27 mul");
    }

    #[test]
    fn mem_infers_bram18() {
        let src = r#"
module ram(input logic clk, input logic [7:0] din, input logic [8:0] addr);
  logic [7:0] mem [0:511];
  always_ff @(posedge clk) mem[addr] <= din;
endmodule
"#;
        let d = synth_sv(src, "ram.sv").unwrap();
        assert!(d.cells.iter().any(|c| matches!(c.kind, CellKind::Bram18)));
    }

    #[test]
    fn hierarchy_flattens_to_inverter() {
        let src = r#"
module tog(input logic clk, output logic q);
  always_ff @(posedge clk) q <= ~q;
endmodule
module top(input logic clk, output logic led);
  tog u0(.clk(clk), .q(led));
endmodule
"#;
        let d = synth_sv(src, "h.sv").unwrap();
        assert_eq!(d.name, "top");
        let inits = d.lut_inits();
        assert_eq!(inits, vec![0x5555_5555_5555_5555]);
        assert_eq!(d.instances.len(), 1, "{:?}", d.instances);
        assert_eq!(d.instances[0].name, "u0");
        assert_eq!(d.instances[0].module, "tog");
    }

    #[test]
    fn always_comb_and_eq_case() {
        let src = r#"
module m(input logic clk, output logic led);
  logic [1:0] s;
  logic q;
  always_ff @(posedge clk) begin
    case (s)
      2'd0: s <= 2'd1;
      2'd1: s <= 2'd2;
      default: s <= 2'd0;
    endcase
    q <= (s == 2'd2);
  end
  always_comb led = q;
endmodule
"#;
        let d = synth_sv(src, "c.sv").unwrap();
        assert!(d.lut_inits().len() >= 2, "case/eq must produce LUTs {:?}", d.lut_inits());
        assert!(d.cell("u_iob").is_some() || d.cells.iter().any(|c| matches!(c.kind, CellKind::IobOut)));
    }

    #[test]
    fn generate_for_unrolls_four_inverters() {
        let src = r#"
module m(input logic clk, output logic led);
  logic [3:0] q;
  always_ff @(posedge clk) begin
    for (int i = 0; i < 4; i = i + 1) begin
      q[i] <= ~q[i];
    end
  end
  assign led = q[0];
endmodule
"#;
        let d = synth_sv(src, "g.sv").unwrap();
        assert_eq!(d.lut_inits().len(), 4);
        assert!(d.lut_inits().iter().all(|&i| i == 0x5555_5555_5555_5555));
    }

    #[test]
    fn keep_attr_sets_dont_touch() {
        let src = r#"
module m(input logic clk, output logic led);
  (* keep = "true" *) logic q;
  always_ff @(posedge clk) q <= ~q;
  assign led = q;
endmodule
"#;
        let d = synth_sv(src, "k.sv").unwrap();
        assert!(
            d.cells.iter().any(|c| c.attrs.flag("DONT_TOUCH")),
            "keep attribute must land on IR"
        );
    }

    #[test]
    fn mark_debug_attr_on_q() {
        let src = r#"
module m(input logic clk, output logic led);
  (* mark_debug = "true" *) logic q;
  always_ff @(posedge clk) q <= ~q;
  assign led = q;
endmodule
"#;
        let d = synth_sv(src, "md.sv").unwrap();
        assert!(
            d.marked_debug_nets().iter().any(|n| n == "q"),
            "{:?}",
            d.marked_debug_nets()
        );
    }

    #[test]
    fn module_parameter_propagates_to_width() {
        let child = r#"
module tog #(parameter N = 1) (input logic clk, output logic led);
  logic [N-1:0] q;
  always_ff @(posedge clk) begin
    for (int i = 0; i < N; i = i + 1) begin
      q[i] <= ~q[i];
    end
  end
  assign led = q[0];
endmodule
"#;
        let d1 = synth_sv(child, "t1.sv").unwrap();
        assert_eq!(d1.lut_inits().len(), 1, "default N=1 must be one LUT {:?}", d1.lut_inits());
        let top = r#"
module tog #(parameter N = 1) (input logic clk, output logic q);
  logic [N-1:0] r;
  always_ff @(posedge clk) begin
    for (int i = 0; i < N; i = i + 1) r[i] <= ~r[i];
  end
  assign q = r[0];
endmodule
module top(input logic clk, output logic led);
  tog #(.N(4)) u0(.clk(clk), .q(led));
endmodule
"#;
        let d4 = synth_sv(top, "t4.sv").unwrap();
        assert_eq!(
            d4.lut_inits().len(),
            4,
            "N=4 must unroll four inverters, got {:?}",
            d4.lut_inits()
        );
        assert!(d4.lut_inits().iter().all(|&i| i == 0x5555_5555_5555_5555));
    }

    #[test]
    fn generate_if_selects_real_branch() {
        let src = r#"
module m #(parameter USE_INC = 0) (input logic clk, output logic led);
  logic [3:0] q;
  generate
    if (USE_INC == 1) begin
      always_ff @(posedge clk) q <= q + 1;
    end else begin
      always_ff @(posedge clk) begin
        q[0] <= ~q[0];
        q[1] <= ~q[1];
        q[2] <= ~q[2];
        q[3] <= ~q[3];
      end
    end
  endgenerate
  assign led = q[3];
endmodule
"#;
        let inv = synth_sv(src, "g0.sv").unwrap();
        assert_eq!(inv.lut_inits().len(), 4);
        assert!(inv.lut_inits().iter().all(|&i| i == 0x5555_5555_5555_5555), "else branch is four inverters {:?}", inv.lut_inits());
        let inc_src = src.replace("USE_INC = 0", "USE_INC = 1");
        let inc = synth_sv(&inc_src, "g1.sv").unwrap();
        assert_eq!(inc.lut_inits(), INC4_INIT.to_vec(), "if branch must be the incrementer");
        assert_ne!(inv.lut_inits(), inc.lut_inits());
    }

    #[test]
    fn const_cond_or_and_left_assoc() {
        // Ibex-style: RV32M == RV32MFast || RV32M == RV32MSingleCycle
        let toks = tokenize("RV32M == RV32MFast || RV32M == RV32MSingleCycle").unwrap();
        let mut params = HashMap::new();
        params.insert("RV32M".into(), 2u128); // RV32MFast
        let mut p = P {
            t: &toks,
            i: 0,
            params,
            widths: HashMap::new(),
        };
        assert_eq!(
            const_cond(&mut p).unwrap(),
            true,
            "RV32MFast must match Fast||SingleCycle"
        );
        assert!(p.peek().is_none(), "must consume full || expr");

        let toks = tokenize("RV32M == RV32MFast || RV32M == RV32MSingleCycle").unwrap();
        let mut params = HashMap::new();
        params.insert("RV32M".into(), 0u128); // RV32MNone
        let mut p = P {
            t: &toks,
            i: 0,
            params,
            widths: HashMap::new(),
        };
        assert_eq!(
            const_cond(&mut p).unwrap(),
            false,
            "RV32MNone must not match Fast||SingleCycle"
        );

        let toks = tokenize("A==1 && A==2").unwrap();
        let mut params = HashMap::new();
        params.insert("A".into(), 1u128);
        let mut p = P {
            t: &toks,
            i: 0,
            params,
            widths: HashMap::new(),
        };
        assert_eq!(const_cond(&mut p).unwrap(), false, "A==1 && A==2 with A=1");

        let toks = tokenize("A==2 || A==3 && A==2").unwrap();
        let mut params = HashMap::new();
        params.insert("A".into(), 2u128);
        let mut p = P {
            t: &toks,
            i: 0,
            params,
            widths: HashMap::new(),
        };
        // left-assoc: (A==2 || A==3) && A==2 → true
        assert_eq!(const_cond(&mut p).unwrap(), true);
    }

    #[test]
    fn generate_else_if_selects_branch() {
        // Maps FFs only on the Fast||SingleCycle arm (Ibex multdiv generate shape).
        let src = r#"
module m #(parameter int RV32M = RV32MFast) (
  input logic clk, output logic [3:0] q
);
  if (RV32M == RV32MSlow) begin
    assign q = 4'h0;
  end else if (RV32M == RV32MFast || RV32M == RV32MSingleCycle) begin
    logic [3:0] r;
    always_ff @(posedge clk) r <= ~r;
    assign q = r;
  end else begin
    assign q = 4'hF;
  end
endmodule
"#;
        let d = synth_sv(src, "elif.sv").unwrap();
        let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        assert_eq!(ffs, 4, "Fast||SingleCycle else-if must map 4 FFs, got {ffs}");
        assert_eq!(d.lut_inits().len(), 4);
        assert!(
            d.lut_inits().iter().all(|&i| i == 0x5555_5555_5555_5555),
            "Fast branch must be invertors {:?}",
            d.lut_inits()
        );

        let none = src.replace("RV32M = RV32MFast", "RV32M = RV32MNone");
        let d0 = synth_sv(&none, "elif_none.sv").unwrap();
        let ffs0 = d0.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        assert_eq!(ffs0, 0, "RV32MNone must take final else assigns only, got {ffs0}");
    }

    #[test]
    fn multi_file_cross_instantiation_is_real_cells() {
        let child = r#"
module child #(parameter W = 1) (input logic clk, output logic q);
  logic [W-1:0] r;
  always_ff @(posedge clk) begin
    for (int i = 0; i < W; i = i + 1) r[i] <= ~r[i];
  end
  assign q = r[0];
endmodule
"#;
        let top = r#"
module top(input logic clk, output logic led);
  child #(.W(4)) u0(.clk(clk), .q(led));
endmodule
"#;
        let d = synth_sv_sources(&[("child.sv", child), ("top.sv", top)]).unwrap();
        assert_eq!(d.name, "top");
        assert_eq!(
            d.lut_inits().len(),
            4,
            "cross-file child #(.W(4)) must produce 4 LUT cells, got {:?}",
            d.lut_inits()
        );
        // Concatenating in the wrong order (top first) must still find child.
        let d2 = synth_sv_sources(&[("top.sv", top), ("child.sv", child)]).unwrap();
        assert_eq!(d2.lut_inits().len(), 4);
    }

    #[test]
    fn clock_enable_occupies_lut_pin() {
        let bare = wrap("~q");
        let en = r#"
module m(input logic clk, input logic en, output logic led);
  logic q;
  always_ff @(posedge clk) begin
    if (en) q <= ~q;
  end
  assign led = q;
endmodule
"#;
        let a = lut_init_of(&bare).unwrap();
        let d = synth_sv(en, "en.sv").unwrap();
        let b = d.lut_inits()[0];
        assert_eq!(a, 0x5555_5555_5555_5555);
        assert_ne!(a, b, "enable must change INIT vs bare inverter {b:#x}");
        assert!(d.ports.iter().any(|p| p.name == "en"));
    }

    #[test]
    fn saturating_add_holds_at_max_in_fabric() {
        let src = r#"
module sat(input logic clk, output logic led);
  logic [3:0] cnt;
  always_ff @(posedge clk) begin
    if (cnt == 4'hF) cnt <= cnt;
    else cnt <= cnt + 1;
  end
  assign led = cnt[3];
endmodule
"#;
        let d = synth_sv(src, "sat.sv").unwrap();
        assert_ne!(
            d.lut_inits(),
            INC4_INIT.to_vec(),
            "sat compare must occupy LUT pins so INIT != bare incrementer"
        );
        let dev = helion_device::Device::load_part("HL10T-C32-1").unwrap();
        let p = helion_pack::pack(&d, &dev).unwrap();
        let pl = helion_place::place(&p, &dev).unwrap();
        let r = helion_route::route(&pl, &dev).unwrap();
        let bits = helion_bits::bitgen(&dev, &r).unwrap();
        let mut fab = helion_fabric::Fabric::new(&dev);
        fab.program(&bits).unwrap();
        fab.finish_startup();
        let iob = r.iob_src[0].iob;
        let mut w = Vec::new();
        for _ in 0..16 {
            fab.step_user();
            w.push(fab.led_at(iob.0, iob.1));
        }
        let bits: String = w.iter().map(|b| if *b { '1' } else { '0' }).collect();
        assert_eq!(
            bits, "0000000111111111",
            "saturating counter must stick at 15 (LED=1), not wrap: {bits}"
        );
    }

    #[test]
    fn casez_dont_care_differs_from_exact_case() {
        let z = r#"
module m(input logic clk, output logic led);
  logic [1:0] s;
  always_ff @(posedge clk) begin
    casez (s)
      2'b0?: s <= 2'b11;
      default: s <= 2'b00;
    endcase
  end
  assign led = s[1];
endmodule
"#;
        let c = r#"
module m(input logic clk, output logic led);
  logic [1:0] s;
  always_ff @(posedge clk) begin
    case (s)
      2'b00: s <= 2'b11;
      default: s <= 2'b00;
    endcase
  end
  assign led = s[1];
endmodule
"#;
        let dz = synth_sv(z, "z.sv").unwrap();
        let dc = synth_sv(c, "c.sv").unwrap();
        assert_ne!(
            dz.lut_inits(),
            dc.lut_inits(),
            "casez 2'b0? must match 00 and 01, not only 00: z={:?} c={:?}",
            dz.lut_inits(),
            dc.lut_inits()
        );
    }

    #[test]
    fn bram_init_appears_in_fabric_not_only_pack() {
        let src = r#"
module rom(input logic clk, output logic led);
  logic [7:0] mem [0:3];
  logic q;
  initial begin
    mem[0] = 8'hA5;
    mem[1] = 8'h3C;
    mem[2] = 8'h00;
    mem[3] = 8'hFF;
  end
  always_ff @(posedge clk) begin
    mem[0] <= mem[0];
    q <= ~q;
  end
  assign led = q;
endmodule
"#;
        let d = synth_sv(src, "rom.sv").unwrap();
        let bram = d
            .cells
            .iter()
            .find(|c| matches!(c.kind, CellKind::Bram18))
            .expect("must infer BRAM18");
        let init = bram.attrs.get("INIT").unwrap_or("");
        assert!(init.contains("a5"), "INIT attr must carry mem[0]=A5, got {init}");
        assert!(init.contains("3c"), "INIT attr must carry mem[1]=3C, got {init}");
        let dev = helion_device::Device::load_part("HL10T-C32-1").unwrap();
        let p = helion_pack::pack(&d, &dev).unwrap();
        assert_eq!(p.brams.len(), 1);
        assert_eq!(p.brams[0].init.get(0).copied().unwrap_or(0), 0xA5);
        assert_eq!(p.brams[0].init.get(1).copied().unwrap_or(0), 0x3C);
        let pl = helion_place::place(&p, &dev).unwrap();
        let r = helion_route::route(&pl, &dev).unwrap();
        let bits = helion_bits::bitgen(&dev, &r).unwrap();
        let mut fab = helion_fabric::Fabric::new(&dev);
        fab.program(&bits).unwrap();
        assert_eq!(fab.bram_init_word(0, 0), 0xA5, "fabric must see programmed INIT[0]");
        assert_eq!(fab.bram_init_word(0, 1), 0x3C, "fabric must see programmed INIT[1]");
        assert_eq!(fab.bram_init_word(0, 3), 0xFF);
        let src2 = src.replace("8'hA5", "8'h11").replace("8'h3C", "8'h22");
        let d2 = synth_sv(&src2, "rom2.sv").unwrap();
        let p2 = helion_pack::pack(&d2, &dev).unwrap();
        let pl2 = helion_place::place(&p2, &dev).unwrap();
        let r2 = helion_route::route(&pl2, &dev).unwrap();
        let bits2 = helion_bits::bitgen(&dev, &r2).unwrap();
        fab.program(&bits2).unwrap();
        assert_eq!(fab.bram_init_word(0, 0), 0x11);
        assert_eq!(fab.bram_init_word(0, 1), 0x22);
        assert_ne!(bits.frames, bits2.frames, "different ROM contents must change bitstream");
    }

    #[test]
    fn preprocess_skips_package_and_ifdef_macros() {
        let src = r#"
`define USE_Q
package p;
  typedef struct packed { logic a; } t;
endpackage
import p::*;
module m(input logic clk, output logic led);
  `ifdef USE_Q
  logic q;
  always_ff @(posedge clk) q <= ~q;
  assign led = q;
  `else
  assign led = 1'b0;
  `endif
endmodule
"#;
        let d = synth_sv(src, "pkg.sv").unwrap();
        assert_eq!(d.name, "m");
        assert_eq!(d.lut_inits(), vec![0x5555_5555_5555_5555]);
    }


    #[test]
    fn timer_full_maps_ffs() {
        // Slice the in-tree Ibex timer module (mtime is 64b).
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/ysyx_ibex.sv");
        let all = std::fs::read_to_string(&p).expect("ysyx_ibex.sv");
        let start = all.find("module timer #(").expect("timer module");
        let rest = &all[start..];
        let end = rest.find("\nendmodule").expect("timer endmodule") + "\nendmodule".len();
        let src = &rest[..end];
        let d = synth_sv(src, "timer.sv").expect("timer");
        let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        eprintln!("timer_full ffs={ffs} cells={}", d.cells.len());
        assert!(ffs >= 64, "timer must map mtime FFs, got {ffs}");
    }

    #[test]
    fn nested_add_rexpr_is_bounded_not_hang() {
        // Pathological nesting used to explode adder_sum_bit (HANG-1539).
        let mut e = RExpr::Ident("a".into());
        for _ in 0..12 {
            e = RExpr::Add(Box::new(e), Box::new(RExpr::Ident("b".into())));
        }
        let rtl = Rtl {
            module: "t".into(),
            ports: vec![],
            signals: vec![
                Signal {
                    name: "a".into(),
                    width: 8,
                    depth: 0,
                    keep: false,
                    mark_debug: false,
                },
                Signal {
                    name: "b".into(),
                    width: 8,
                    depth: 0,
                    keep: false,
                    mark_debug: false,
                },
            ],
            nbas: vec![],
            assigns: vec![],
            insts: vec![],
            params: vec![],
            toks: vec![],
            mem_inits: Default::default(),
        };
        let r = rexpr_to_bit(&e, &rtl, 7);
        assert!(r.is_err(), "deep Add must Err, not hang: {r:?}");
    }

    #[test]
    fn nested_cmp_add_rexpr_is_bounded_not_hang() {
        // CORPUS cmp/ashr path: nested Add under Eq/Lt must Err-skip, not hang.
        let mut a = RExpr::Ident("a".into());
        let mut b = RExpr::Ident("b".into());
        for _ in 0..12 {
            a = RExpr::Add(Box::new(a), Box::new(RExpr::Ident("a".into())));
            b = RExpr::Add(Box::new(b), Box::new(RExpr::Ident("b".into())));
        }
        let rtl = Rtl {
            module: "t".into(),
            ports: vec![],
            signals: vec![
                Signal {
                    name: "a".into(),
                    width: 64,
                    depth: 0,
                    keep: false,
                    mark_debug: false,
                },
                Signal {
                    name: "b".into(),
                    width: 64,
                    depth: 0,
                    keep: false,
                    mark_debug: false,
                },
            ],
            nbas: vec![],
            assigns: vec![],
            insts: vec![],
            params: vec![],
            toks: vec![],
            mem_inits: Default::default(),
        };
        let eq = rexpr_to_bit(&RExpr::Eq(Box::new(a.clone()), Box::new(b.clone())), &rtl, 0);
        assert!(eq.is_err(), "deep Eq(Add,Add) must Err, not hang: {eq:?}");
        let lt = rexpr_to_bit(&RExpr::Lt(Box::new(a), Box::new(b)), &rtl, 0);
        assert!(lt.is_err(), "deep Lt(Add,Add) must Err, not hang: {lt:?}");
        let wide = Rtl {
            module: "t".into(),
            ports: vec![],
            signals: vec![
                Signal {
                    name: "x".into(),
                    width: 128,
                    depth: 0,
                    keep: false,
                    mark_debug: false,
                },
                Signal {
                    name: "y".into(),
                    width: 128,
                    depth: 0,
                    keep: false,
                    mark_debug: false,
                },
            ],
            nbas: vec![],
            assigns: vec![],
            insts: vec![],
            params: vec![],
            toks: vec![],
            mem_inits: Default::default(),
        };
        let wide_eq = rexpr_to_bit(
            &RExpr::Eq(
                Box::new(RExpr::Ident("x".into())),
                Box::new(RExpr::Ident("y".into())),
            ),
            &wide,
            0,
        );
        assert!(wide_eq.is_err(), "width>32 Eq must Err: {wide_eq:?}");
    }

    #[test]
    fn genvar_plusplus_unrolls_flops() {
        let src = r#"
module rf #(parameter int NUM_WORDS = 4, parameter int W = 8) (
  input logic clk_i, input logic rst_ni,
  input logic [W-1:0] wdata_i, input logic [NUM_WORDS-1:0] we_i,
  output logic [W-1:0] r0
);
  for (genvar i = 1; i < NUM_WORDS; i++) begin : g_rf
    logic [W-1:0] q;
    always_ff @(posedge clk_i) begin
      if (we_i[i]) q <= wdata_i;
    end
  end
  assign r0 = 8'h0;
endmodule
"#;
        let d = synth_sv(src, "rf.sv").expect("synth rf");
        let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        assert!(ffs >= 24, "i++ genvar must map 3×8 FFs, got {ffs}");
    }


    #[test]
    fn regfile_enum_literal_selects_generate_ff_branch() {
        // Packages/enums are skipped; RegFileFF must still const-fold so the
        // generate-if then branch (always_ff bank) is kept, not the else.
        let src = r#"
module rf_sel #(parameter regfile_e RegFile = RegFileFF) (
  input logic clk,
  output logic [31:0] q
);
  logic [31:0] r;
  if (RegFile == RegFileFF) begin
    always_ff @(posedge clk) r <= ~r;
  end else begin
    assign r = 32'h0;
  end
  assign q = r;
endmodule
"#;
        let d = synth_sv(src, "rf_sel.sv").expect("synth rf_sel");
        let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        assert_eq!(ffs, 32, "RegFile==RegFileFF must take then always_ff, got {ffs}");

        let pkg = src.replace("RegFile = RegFileFF", "RegFile = ibex_pkg::RegFileFF")
            .replace("RegFile == RegFileFF", "RegFile == ibex_pkg::RegFileFF");
        let d2 = synth_sv(&pkg, "rf_sel_pkg.sv").expect("synth pkg::RegFileFF");
        let ffs2 = d2.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        assert_eq!(ffs2, 32, "pkg::RegFileFF must const-fold, got {ffs2}");
    }

    #[test]
    fn unknown_generate_if_takes_else_flops() {
        let src = r#"
module wrap (input logic clk_i, input logic rst_ni, input logic d_i, output logic q_o);
  parameter int Impl = 99;
  if (Impl == MissingPkg::Xilinx) begin : gen_x
    assign q_o = d_i;
  end else begin : gen_g
    always_ff @(posedge clk_i) q_o <= d_i;
  end
endmodule
"#;
        let d = synth_sv(src, "wrap.sv").expect("synth wrap");
        let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        assert_eq!(ffs, 1, "unknown generate-if should take else always_ff");
    }

    #[test]
    fn ysyx_ibex_lists_modules_and_synths_top() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/ysyx_ibex.sv");
        assert!(p.exists(), "examples/ysyx_ibex.sv must ship with the crate tests");
        let src = std::fs::read_to_string(&p).unwrap();
        let mods = list_sv_modules_origin(&src, "ysyx_ibex.sv").expect("list ibex modules");
        eprintln!("ibex_modules={} has_top={}", mods.len(), mods.iter().any(|m| m == "ysyx_ibex"));
        assert!(
            mods.iter().any(|m| m == "ysyx_ibex"),
            "top ysyx_ibex must parse: {mods:?}"
        );
        assert!(
            mods.len() > 5,
            "Ibex-scale file must yield many modules, got {}",
            mods.len()
        );
        {
            let pre = preprocess_sv(&strip_comments(&src));
            let mods = parse_source(&pre).expect("parse ibex stats");
            let n_inst: usize = mods.iter().map(|m| m.insts.len()).sum();
            let n_nba: usize = mods.iter().map(|m| m.nbas.len()).sum();
            let n_as: usize = mods.iter().map(|m| m.assigns.len()).sum();
            let with_nba = mods.iter().filter(|m| !m.nbas.is_empty()).count();
            let with_inst = mods.iter().filter(|m| !m.insts.is_empty()).count();
            eprintln!(
                "parse modules={} insts={n_inst} nbas={n_nba} assigns={n_as} mods_with_nba={with_nba} mods_with_inst={with_inst}",
                mods.len()
            );
            let map: std::collections::HashMap<String, Rtl> =
                mods.iter().map(|m| (m.module.clone(), m.clone())).collect();
            let flat = flatten_module_ov(&map, "ysyx_ibex", &std::collections::HashMap::new())
                .expect("flatten ibex");
            eprintln!(
                "flat nbas={} assigns={} signals={}",
                flat.nbas.len(),
                flat.assigns.len(),
                flat.signals.len()
            );
            // Cap the diagnostic bit-probe (FM-HEL-HANG-1539): full per-bit walk can
            // hang on nested Add trees. Bound time-ish work; synth_sv_path is the real gate.
            let mut bit_ok = 0usize;
            let mut bit_fail = 0usize;
            let mut probed = 0usize;
            const PROBE_CAP: usize = 4096;
            'probe: for (lhs, bit, rhs) in &flat.nbas {
                let w = sig_width(&flat, lhs).min(64);
                if let Some(b) = bit {
                    match rexpr_to_bit(rhs, &flat, (*b).min(63)) {
                        Ok(_) => bit_ok += 1,
                        Err(_) => bit_fail += 1,
                    }
                    probed += 1;
                } else {
                    for i in 0..w.max(1) {
                        match rexpr_to_bit(rhs, &flat, i) {
                            Ok(_) => bit_ok += 1,
                            Err(_) => bit_fail += 1,
                        }
                        probed += 1;
                        if probed >= PROBE_CAP {
                            break 'probe;
                        }
                    }
                }
                if probed >= PROBE_CAP {
                    break;
                }
            }
            eprintln!(
                "rexpr_to_bit ok={bit_ok} fail={bit_fail} probed={probed} (cap={PROBE_CAP})"
            );
        }
        let t0 = std::time::Instant::now();
        let d = synth_sv_path(&p).expect("synth ysyx_ibex");
        let luts = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count();
        let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        eprintln!(
            "synth_sv_path Ok name={} ports={} cells={} luts={} ffs={} elapsed_ms={}",
            d.name,
            d.ports.len(),
            d.cells.len(),
            luts,
            ffs,
            t0.elapsed().as_millis()
        );
        assert_eq!(d.name, "ysyx_ibex");
        assert!(
            !d.ports.is_empty(),
            "ysyx_ibex ports from ANSI list"
        );
        assert!(
            !d.cells.is_empty(),
            "ysyx_ibex must map at least one LUT/FF from always_ff/assigns, cells=0"
        );
    }

    #[test]
    fn logikbench_lrelu_ge_ashr_maps_cells() {
        let src = r#"
module lrelu #(parameter DW = 16, parameter ASHIFT = 7)
   (
    input signed [DW-1:0]  in,
    output signed [DW-1:0] out
    );
   assign out = (in >= 0) ? in : (in >>> ASHIFT);
endmodule
"#;
        let d = synth_sv(src, "lrelu.sv").expect("synth lrelu");
        assert!(
            !d.cells.is_empty(),
            "lrelu >= / >>> must map cells, got 0"
        );
        let luts = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count();
        assert!(luts > 0, "lrelu must produce LUTs for mux/ashr, luts={luts}");
    }

    #[test]
    fn logikbench_hswish_region_le_ge_maps_cells() {
        let src = r#"
module hswish #(parameter DW = 16, parameter QW = 8)
   (
    input signed [DW-1:0]  x,
    output signed [DW-1:0] out
    );
   localparam signed [DW:0] THREE = 3 <<< QW;
   localparam signed [DW:0] SIX  = 6 <<< QW;
   wire signed [2*DW:0]     prod;
   wire signed [2*DW:0]     mid;
   assign prod = x * (x + THREE);
   assign mid  = prod / SIX;
   assign out = (x <= -THREE) ? {DW{1'b0}} :
                (x >=  THREE) ? x :
                mid[DW-1:0];
endmodule
"#;
        let d = synth_sv(src, "hswish.sv").expect("synth hswish");
        assert!(
            !d.cells.is_empty(),
            "hswish region <=/>= must map cells, got 0"
        );
        // Region mux + compares should produce LUTs even if /const is skipped.
        let luts = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count();
        assert!(
            luts > 0 || d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)),
            "hswish must map LUTs or Mac27, cells={}",
            d.cells.len()
        );
    }

    #[test]
    fn cmp_le_ge_and_ashr_smoke() {
        let src = r#"
module t(input [7:0] a, b, input signed [15:0] s, output o, output signed [15:0] y);
  assign o = (a <= b) & (a >= b);
  assign y = s >>> 3;
endmodule
"#;
        let d = synth_sv(src, "cmp.sv").expect("synth cmp");
        assert!(!d.cells.is_empty(), "le/ge/ashr smoke must map cells");
    }

    #[test]
    fn logical_or_in_always_maps_ff() {
        let src = r#"
module RefModule (
  input clk,
  input reset,
  output reg [3:0] q
);
  always @(posedge clk)
    if (reset || q == 10)
      q <= 1;
    else
      q <= q+1;
endmodule
"#;
        let d = synth_sv(src, "cnt10.sv").expect("|| always");
        let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        assert!(ffs >= 4, "reset || q==10 counter must map FFs, got {ffs} cells={:?}", d.cells.len());
    }

    #[test]
    fn concat_lhs_bit_reverse_maps_luts() {
        let src = r#"
module RefModule (
  input [7:0] in,
  output [7:0] out
);
  assign {out[0],out[1],out[2],out[3],out[4],out[5],out[6],out[7]} = in;
endmodule
"#;
        let d = synth_sv(src, "rev.sv").expect("concat lhs");
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Lut6 { .. } | CellKind::IobOut)),
            "bit reverse must map LUT/IOB, cells={:?}",
            d.cells
        );
    }

    #[test]
    fn dp_lut_var_index_write_is_hffs_not_skip_assign() {
        let src = r#"
module dp_lut_7x5_14x4(clk,din_a,we_a,addr_a,dout_b,addr_b);
  input  clk;
  input  we_a;
  input  [4:0] addr_a;
  input  [6:0] din_a;
  input  [3:0] addr_b;
  output [13:0] dout_b;
  reg  [6:0] lut[0:31];
  always @(posedge clk)
      begin
        if (we_a)
          begin
            lut[addr_a] <= din_a;
          end
      end
  assign dout_b = {lut[addr_b<<1+1],lut[addr_b<<1]};
endmodule
"#;
        let d = synth_sv(src, "dp_lut.sv").expect("dp_lut");
        let hffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        assert_eq!(hffs, 32 * 7, "32x7 write-side Hffs, got {hffs}");
        assert_ne!(d.attrs.get("NO_BODY"), Some("1"));
        assert_ne!(d.attrs.get("WIDE_CONE"), Some("1"));
        assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Bram18)));
    }

    #[test]
    fn const_range_compare_lowers_small_lut() {
        let src = r#"
module MMC4(input [15:0] prg_ain);
  wire
       prg_is_ram = (prg_ain < 'h8000) && (prg_ain >= 'h6000);
endmodule
"#;
        let d = synth_sv(src, "mmc4_ram.v").expect("range compare");
        assert_ne!(
            d.attrs.get("ASSIGN_NOT_LOWERED"),
            Some("1"),
            "small range must lower, not assign_not_lowered"
        );
        let lut = d
            .cells
            .iter()
            .find(|c| {
                matches!(c.kind, CellKind::Lut6 { .. })
                    && d.nets.iter().any(|n| {
                        n.name == "prg_is_ram"
                            && n.endpoints
                                .iter()
                                .any(|e| e.cell == c.name && e.pin == "O")
                    })
            })
            .expect("prg_is_ram must drive a LUT");
        let init = match lut.kind {
            CellKind::Lut6 { init } => init,
            _ => unreachable!(),
        };
        let mut pins = Vec::new();
        for i in 0..6 {
            if let Some(net) = d.net_on(&lut.name, &format!("I{i}")) {
                pins.push(net.to_string());
            }
        }
        pins.sort();
        assert_eq!(
            pins,
            vec!["prg_ain_13".to_string(), "prg_ain_14".to_string(), "prg_ain_15".to_string()],
            "range is a 3-bit test, not a 16-PI cone: {pins:?}"
        );
        // I0 is the LSB of the LUT address. [15:13]==011.
        let bit_of = |name: &str| -> usize {
            match name {
                "prg_ain_13" => 0,
                "prg_ain_14" => 1,
                "prg_ain_15" => 2,
                _ => panic!("unexpected pin {name}"),
            }
        };
        let mut pin_at = [""; 3];
        for i in 0..6 {
            if let Some(net) = d.net_on(&lut.name, &format!("I{i}")) {
                pin_at[i] = net;
            }
        }
        for addr in 0..8u64 {
            let mut field = 0u64;
            for i in 0..3 {
                if pin_at[i].is_empty() {
                    continue;
                }
                if (addr >> i) & 1 == 1 {
                    field |= 1u64 << bit_of(pin_at[i]);
                }
            }
            let want = field == 0b011;
            let got = (init >> addr) & 1 == 1;
            assert_eq!(got, want, "addr={addr:#b} field={field:#b} init={init:#x}");
        }
    }

    #[test]
    fn clock_mux_posedge_is_not_a_user_clock() {
        let src = r#"
module clk_mux_q(input csr_clk, input csr_ena, input ram_clk, input d, output reg q);
  wire int_clk;
  assign int_clk = ~csr_ena ? csr_clk : ram_clk;
  always @(posedge int_clk) q <= d;
endmodule
"#;
        let d = synth_sv(src, "clk_mux.v").expect("clock mux");
        assert_eq!(
            d.attrs.get("CLOCK_MUX"),
            Some("1"),
            "muxed posedge must not be a single user clock"
        );
        let invented = d.nets.iter().any(|n| {
            n.name == "int_clk"
                && n.endpoints.iter().any(|e| e.pin == "O")
        });
        assert!(
            !invented,
            "clock mux must not be lowered as a data LUT, nets={:?}",
            d.nets.iter().map(|n| n.name.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn clock_gate_posedge_is_not_a_user_clock() {
        // Sibling of clock_mux: AND/OR of a clock and an enable (`~en` too).
        // Not a ternary of two clocks. Not a data LUT. Not a closed WNS.
        let src = r#"
module clk_gate_q(input clk, input en, input d, output reg q);
  wire gclk;
  assign gclk = clk & ~en;
  always @(posedge gclk) q <= d;
endmodule
"#;
        let d = synth_sv(src, "clk_gate.v").expect("clock gate");
        assert_eq!(
            d.attrs.get("CLOCK_GATE"),
            Some("1"),
            "gated posedge must not be a single user clock"
        );
        assert_ne!(
            d.attrs.get("CLOCK_MUX"),
            Some("1"),
            "clk & ~en is a gate, not a mux of two clocks"
        );
        let invented = d.nets.iter().any(|n| {
            n.name == "gclk" && n.endpoints.iter().any(|e| e.pin == "O")
        });
        assert!(
            !invented,
            "clock gate must not be lowered as a data LUT, nets={:?}",
            d.nets.iter().map(|n| n.name.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn inout_load_enable_reaches_ff_d() {
        // reset has priority; read-only inout ena gates the load. Hold otherwise.
        // D must depend on ena. Not a dropped enable, not an invented bus.
        let src = r#"
module latch_EX_MEM(input clk, input reset, inout ena, input din, output reg q);
  always @(posedge clk) begin
    if (reset)
      q <= 0;
    else if (ena == 1'b1)
      q <= din;
  end
endmodule
"#;
        let d = synth_sv(src, "latch_ex_mem.v").expect("inout load enable");
        assert_ne!(
            d.attrs.get("INOUT_ENABLE_NOT_LOWERED"),
            Some("1"),
            "ena must be mapped onto FF D, not dropped"
        );
        assert!(
            d.ports.iter().any(|p| p.name == "ena" && matches!(p.dir, PortDir::In)),
            "read-only inout is the load-enable input, not a bus"
        );
        let ff = d
            .cells
            .iter()
            .find(|c| matches!(c.kind, CellKind::Hff) && d.net_on(&c.name, "Q") == Some("q"))
            .expect("q must be an Hff");
        assert_eq!(d.net_on(&ff.name, "CLK"), Some("clk"));
        let on_ena = d.nets.iter().any(|n| {
            n.name == "ena"
                && n.endpoints
                    .iter()
                    .any(|e| e.pin.starts_with('I') || (e.cell == ff.name && e.pin == "D"))
        });
        assert!(on_ena, "ena must connect into the enable path of D");
    }

    #[test]
    fn negedge_always_lowers_hff_on_named_clock() {
        let src = r#"
module rw_manager_ram_csr #(parameter DATA_WIDTH = 32, ADDR_WIDTH = 1+1, NUM_WORDS = 4)
  (input csr_clk, input csr_ena, input csr_din, input ram_clk, input wren,
   input [DATA_WIDTH-1:0] data, input [ADDR_WIDTH+(-1):0] wraddress,
   input [ADDR_WIDTH+(-1):0] rdaddress, output reg [DATA_WIDTH-1:0] q,
   output reg csr_dout);
  localparam integer DATA_COUNT = NUM_WORDS*DATA_WIDTH;
  reg [DATA_COUNT+(-1):0] all_data;
  wire int_clk;
  assign int_clk = ~csr_ena ? csr_clk : ram_clk;
  always @(posedge int_clk) begin
    q <= data;
  end
  always @(negedge csr_clk) begin
    csr_dout <= all_data[DATA_COUNT+(-1)];
  end
endmodule
"#;
        let d = synth_sv(src, "negedge_csr.v").expect("negedge always");
        assert_eq!(d.attrs.get("CLOCK_MUX"), Some("1"), "muxed posedge still clock_mux");
        let ff = d
            .cells
            .iter()
            .find(|c| matches!(c.kind, CellKind::Hff) && d.net_on(&c.name, "Q") == Some("csr_dout"))
            .expect("csr_dout must be an Hff Q");
        assert_eq!(d.net_on(&ff.name, "CLK"), Some("csr_clk"), "negedge clock is csr_clk");
    }

    #[test]
    fn wide_const_compare_stays_not_lowered() {
        // Lowest set bit of 'h00FF is 0: every bus bit matters. Not a small LUT.
        let src = r#"
module wide_cmp(input [15:0] a, output y);
  assign y = a < 16'h00FF;
endmodule
"#;
        let d = synth_sv(src, "wide_cmp.v").expect("wide cmp");
        assert_eq!(
            d.attrs.get("ASSIGN_NOT_LOWERED"),
            Some("1"),
            "general compare wider than a LUT6 must stay assign_not_lowered"
        );
    }

    #[test]
    fn split_wire_assign_is_net_plus_assign() {
        // Assignment on the next line after `wire` is a net plus assign.
        let src = r#"
module splitw(input a, input b, output y);
  wire
       y = a & b;
endmodule
"#;
        let d = synth_sv(src, "splitw.v").expect("split wire");
        let lut = d.cells.iter().find(|c| matches!(c.kind, CellKind::Lut6 { .. }));
        let lut = lut.expect("split wire assign must map a LUT, not drop the name");
        let init = match lut.kind {
            CellKind::Lut6 { init } => init,
            _ => unreachable!(),
        };
        assert_ne!(init, 0, "unknown/split wire name must not fold to const 0");
        let on_y = d.nets.iter().any(|n| {
            n.name == "y" && n.endpoints.iter().any(|e| e.cell == lut.name && e.pin == "O")
        });
        assert!(on_y, "split wire assign must drive y, nets={:?}", d.nets);
    }

    #[test]
    fn wire_through_assign_emits_iob() {
        let src = r#"
module RefModule (input in, output out);
  assign out = in;
endmodule
"#;
        let d = synth_sv(src, "wire.sv").expect("wire");
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::IobOut)),
            "assign out=in must emit IOB, cells={:?}",
            d.cells
        );
    }

    #[test]
    fn wide_binary_literal_does_not_panic() {
        // 255-bit sized binary used to `1u128 << bit` and panic (old lib.rs:959).
        let bits = "1".repeat(130);
        let src = format!(
            "module data_generator(input wire CLK, input wire CE, output wire D1);\n  reg [129:0] ring1;\n  initial ring1 <= 130'b{bits};\n  always @(posedge CLK)\n    if (CE) ring1 <= {{ring1[0], ring1[129:1]}};\n  assign D1 = ring1[0];\nendmodule\n"
        );
        let d = synth_sv(&src, "wide_lit.sv").expect("wide literal must not panic");
        assert_eq!(d.name, "data_generator");
        assert_ne!(
            d.attrs.get("NO_BODY"),
            Some("1"),
            "rotate is behavioral; do not drop the body, cells={:?}",
            d.cells
        );
        let logic = d
            .cells
            .iter()
            .filter(|c| {
                matches!(
                    c.kind,
                    CellKind::Lut6 { .. } | CellKind::Hff | CellKind::Bram18
                )
            })
            .count();
        assert!(logic > 0, "wide rotate must lower real cells, cells={:?}", d.cells);
        assert!(
            !d.cells.iter().any(|c| matches!(c.kind, CellKind::Bram18)),
            "wide initial must not invent a BRAM, cells={:?}",
            d.cells
        );
    }

    #[test]
    fn zero_param_range_does_not_panic() {
        // `bits_in=0` makes `in[bits_in-1:0]` a wrapped range. `hi - lo + 1`
        // used to panic (cpuv/16677.v, old lib.rs:6042). Named diagnostic,
        // no invented bus, not a closed WNS.
        let src = r#"
module clip #(parameter bits_in=0, parameter bits_out=0)
    (input [bits_in-1:0] in, output [bits_out-1:0] out);
   wire overflow = |in[bits_in-1:bits_out] & ~(&in[bits_in-1:bits_out]);
   assign out = overflow ? in[bits_out-1:0] : in[bits_out-1:0];
endmodule
"#;
        let d = synth_sv(src, "clip0.sv").expect("zero-width range must not panic");
        assert_eq!(d.name, "clip");
        assert_eq!(d.attrs.get("WIDTH_OVERFLOW"), Some("1"));
        let logic = d
            .cells
            .iter()
            .filter(|c| {
                matches!(
                    c.kind,
                    CellKind::Lut6 { .. } | CellKind::Hff | CellKind::Bram18
                )
            })
            .count();
        assert_eq!(logic, 0, "must not invent gates from a range that does not fit, cells={:?}", d.cells);
    }

    fn string_param_width_does_not_panic() {
        // `parameter DATA_WIDTH = ""` hashed past usize and panicked on
        // range `+ 1` (old lib.rs:1052). Named diagnostic, no invented bus.
        let src = r#"
module rw_manager_bitcheck(ck, read_data);
  parameter DATA_WIDTH = "";
  parameter AFI_RATIO = "";
  localparam NUMBER_OF_WORDS = 1<<1*AFI_RATIO;
  localparam DATA_BUS_SIZE = DATA_WIDTH*NUMBER_OF_WORDS;
  input ck;
  input [DATA_BUS_SIZE+(0-1):0] read_data;
  output [DATA_WIDTH-1:0] error_word;
endmodule
"#;
        let d = synth_sv(src, "str_width.sv").expect("string width must not panic");
        assert_eq!(d.name, "rw_manager_bitcheck");
        let logic = d
            .cells
            .iter()
            .filter(|c| {
                matches!(
                    c.kind,
                    CellKind::Lut6 { .. } | CellKind::Hff | CellKind::Bram18
                )
            })
            .count();
        assert_eq!(logic, 0, "must not invent gates from a string width, cells={:?}", d.cells);
        assert_eq!(d.attrs.get("NO_BODY"), Some("1"));
    }

    #[test]
    fn empty_shell_is_no_body_not_invented_gates() {
        let src = r#"
module sky130_fd_sc_hs__tap(VGND,VPWR);
  input VGND;
  input VPWR;
endmodule
"#;
        let d = synth_sv(src, "tap.v").expect("empty shell");
        assert!(d.cells.is_empty(), "empty shell must not invent gates, cells={:?}", d.cells);
        assert_eq!(d.attrs.get("NO_BODY"), Some("1"));
    }

    #[test]
    fn string_param_always_case_maps_luts() {
        let src = r#"
module hardcopyii_reorder_output(datain, operation, dataout);
  parameter operation_mode = "dynamic";
  input [3:0] datain;
  input [1:0] operation;
  output [3:0] dataout;
  reg [3:0] dataout_tmp;
  assign dataout = dataout_tmp;
  always @(datain)
    begin
      if (operation_mode == "dynamic")
        begin
          case (operation)
            2'b00: dataout_tmp = datain;
            2'b01: dataout_tmp = {datain[1:0], datain[3:2]};
            default: dataout_tmp = 4'b0;
          endcase
        end
      else dataout_tmp = datain;
    end
endmodule
"#;
        let d = synth_sv(src, "reord.v").expect("always/case");
        let luts = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count();
        assert!(luts > 0, "string-param always/case must map LUTs, cells={:?}", d.cells);
        assert_ne!(d.attrs.get("NO_BODY"), Some("1"));
    }

    #[test]
    fn function_lifted_into_empty_module_maps_lut() {
        let src = r#"
module CRC_tiny();
  function [3:0] nextCRC;
    input [3:0] Data;
    input [3:0] crc;
    reg [3:0] newcrc;
    begin
      newcrc[0] = Data[0] ^ crc[0];
      newcrc[1] = Data[1] ^ crc[1];
      newcrc[2] = Data[2] & crc[2];
      newcrc[3] = Data[3] | crc[3];
      nextCRC = newcrc;
    end
  endfunction
endmodule
"#;
        let d = synth_sv(src, "crc.v").expect("lift function");
        let luts = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count();
        assert!(luts >= 4, "lifted function must map LUTs, luts={luts} cells={:?}", d.cells);
        assert!(d.ports.iter().any(|p| p.name == "nextCRC" && matches!(p.dir, PortDir::Out)));
    }

    #[test]
    fn unknown_instance_names_itself() {
        let src = r#"
module wrap(input a, output y);
  missing_child u0(.a(a), .y(y));
endmodule
"#;
        let d = synth_sv(src, "wrap.sv").expect("unknown child");
        assert!(d.cells.is_empty(), "absent child must not invent gates {:?}", d.cells);
        assert_eq!(d.attrs.get("NO_BODY"), Some("1"));
    }

    #[test]
    fn incremental_reuses_parent_when_child_changes() {
        let parent = r#"
module inc_parent(input clk, input en, output led);
  wire q;
  inc_child u0(.clk(clk), .q(q));
  assign led = q & en;
endmodule
"#;
        let child1 = r#"
module inc_child(input clk, output reg q);
  always @(posedge clk) q <= ~q;
endmodule
"#;
        let child2 = r#"
module inc_child(input clk, output reg q);
  always @(posedge clk) q <= q;
endmodule
"#;
        let s1 = format!("{parent}\n{child1}");
        let s2 = format!("{parent}\n{child2}");
        let _ = synth_sv(&s1, "inc1.sv").expect("first");
        let before = incremental_log();
        let _ = synth_sv(&s2, "inc2.sv").expect("second");
        let after = incremental_log();
        let fresh: Vec<_> = after.iter().skip(before.len()).cloned().collect();
        let joined = fresh.join("\n");
        assert!(
            joined.contains("incremental reused module=inc_parent"),
            "parent own-logic must be reused, log={joined}"
        );
        assert!(
            joined.contains("incremental rebuilt module=inc_child"),
            "child must rebuild, log={joined}"
        );
    }

    #[test]
    fn const_index_word_add_is_hffs_not_mac_or_leftover() {
        // cycles=1 → q aliases res[0]. The for-loop body does not run.
        // Signed width-16 add is a ripple adder into Hffs, not a MAC and not
        // two leftover bits. A signal index is not aliased this way.
        let src = r#"
module addfxp(a,b,q,clk);
  parameter  width = 16, cycles = 1;
  input  signed  [(-1)+width:0] a,b;
  input  clk;
  output signed  [(-1)+width:0] q;
  reg  signed  [(-1)+width:0] res[(-1)+cycles:0];
  assign q = res[(-1)+cycles];
  integer i;
  always @(posedge clk)
      begin
        res[0] <= a+b;
        for (i = 1; i < cycles; i = 1+i)
            res[i] <= res[i-1];
      end
endmodule
"#;
        let d = synth_sv(src, "addfxp.v").expect("addfxp");
        assert_ne!(d.attrs.get("ASSIGN_NOT_LOWERED"), Some("1"));
        assert_ne!(d.attrs.get("WIDE_CONE"), Some("1"));
        assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)));
        assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Bram18)));
        let hffs: Vec<_> = d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Hff))
            .collect();
        assert_eq!(hffs.len(), 16, "16-bit add, one stage, got {}", hffs.len());
        assert!(hffs.iter().all(|c| d.net_on(&c.name, "CLK") == Some("clk")));
        let luts = d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count();
        assert!(luts >= 30, "16-bit ripple adder LUTs, not leftover bits, luts={luts}");
        let aliased = d.nets.iter().any(|n| {
            n.name == "res_w0_0"
                && n.endpoints.iter().any(|e| e.pin == "Q")
                && n.endpoints.iter().any(|e| e.pin == "I")
        });
        assert!(
            aliased,
            "q aliases res[0], nets={:?}",
            d.nets.iter().map(|n| n.name.as_str()).collect::<Vec<_>>()
        );
        assert!(
            !d.cells.iter().any(|c| {
                matches!(c.kind, CellKind::Hff) && d.net_on(&c.name, "Q") == Some("res_w1_0")
            }),
            "cycles=1 must not invent a second pipeline word"
        );
    }

    #[test]
    fn const_cycles_2_unrolls_word_shift_not_a_second_add() {
        // cycles=2 → q aliases res[1], the last word. 2*width Hffs.
        // The add lands only in word 0. Word 1 is a shift, D from res[0].
        let src = r#"
module addfxp2(a,b,q,clk);
  parameter  width = 8, cycles = 2;
  input  signed  [(-1)+width:0] a,b;
  input  clk;
  output signed  [(-1)+width:0] q;
  reg  signed  [(-1)+width:0] res[(-1)+cycles:0];
  assign q = res[(-1)+cycles];
  integer i;
  always @(posedge clk)
      begin
        res[0] <= a+b;
        for (i = 1; i < cycles; i = 1+i)
            res[i] <= res[i-1];
      end
endmodule
"#;
        let d = synth_sv(src, "addfxp2.v").expect("addfxp2");
        assert_ne!(d.attrs.get("WORD_PIPELINE_CAP"), Some("1"));
        assert_ne!(d.attrs.get("ASSIGN_NOT_LOWERED"), Some("1"));
        assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)));
        assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Bram18)));
        let hffs: Vec<_> = d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Hff))
            .collect();
        assert_eq!(hffs.len(), 16, "2*width Hffs, got {}", hffs.len());
        assert!(hffs.iter().all(|c| d.net_on(&c.name, "CLK") == Some("clk")));
        for bit in 0..8 {
            let q0 = format!("res_w0_{bit}");
            let q1 = format!("res_w1_{bit}");
            let ff0 = hffs
                .iter()
                .find(|c| d.net_on(&c.name, "Q") == Some(q0.as_str()))
                .unwrap_or_else(|| panic!("word 0 bit {bit}"));
            let d0 = d.net_on(&ff0.name, "D").unwrap_or("");
            assert!(
                d0.starts_with("u_add0_"),
                "add only into word 0, bit {bit} D={d0}"
            );
            let ff1 = hffs
                .iter()
                .find(|c| d.net_on(&c.name, "Q") == Some(q1.as_str()))
                .unwrap_or_else(|| panic!("word 1 bit {bit}"));
            assert_eq!(
                d.net_on(&ff1.name, "D"),
                Some(q0.as_str()),
                "word 1 D from res[0]"
            );
        }
        assert!(
            !d.cells.iter().any(|c| {
                matches!(c.kind, CellKind::Hff) && d.net_on(&c.name, "Q") == Some("res_w2_0")
            }),
            "cycles=2 must not invent a third pipeline word"
        );
        assert!(
            !d.nets.iter().any(|n| n.name.starts_with("u_add1_")),
            "no adder into word 1"
        );
        let aliased = d.nets.iter().any(|n| {
            n.name == "res_w1_0"
                && n.endpoints.iter().any(|e| e.pin == "Q")
                && n.endpoints.iter().any(|e| e.pin == "I")
        });
        assert!(
            aliased,
            "q aliases res[1], the last word, nets={:?}",
            d.nets.iter().map(|n| n.name.as_str()).collect::<Vec<_>>()
        );
        let word0_is_q = d.nets.iter().any(|n| {
            n.name == "res_w0_0" && n.endpoints.iter().any(|e| e.pin == "I")
        });
        assert!(!word0_is_q, "q must not alias word 0 when cycles=2");
    }

    #[test]
    fn cycles_above_cap_does_not_invent_stages_or_close() {
        let src = r#"
module addfxp5(a,b,q,clk);
  parameter  width = 8, cycles = 5;
  input  signed  [(-1)+width:0] a,b;
  input  clk;
  output signed  [(-1)+width:0] q;
  reg  signed  [(-1)+width:0] res[(-1)+cycles:0];
  assign q = res[(-1)+cycles];
  integer i;
  always @(posedge clk)
      begin
        res[0] <= a+b;
        for (i = 1; i < cycles; i = 1+i)
            res[i] <= res[i-1];
      end
endmodule
"#;
        let d = synth_sv(src, "addfxp5.v").expect("addfxp5");
        assert_eq!(d.attrs.get("WORD_PIPELINE_CAP"), Some("1"));
        let hffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        assert_eq!(hffs, 8, "add into word 0 only, no extra stages, got {hffs}");
        assert!(
            !d.cells.iter().any(|c| {
                matches!(c.kind, CellKind::Hff)
                    && d.net_on(&c.name, "Q").is_some_and(|q| q.starts_with("res_w1_"))
            }),
            "cycles=5 must not invent word 1"
        );
        let aliased = d.nets.iter().any(|n| {
            n.name.starts_with("res_w4_") && n.endpoints.iter().any(|e| e.pin == "I")
        });
        assert!(!aliased, "cap must not alias q onto a missing last word");
    }

    #[test]
    fn zero_fill_concat_is_wire_align_not_a_cone() {
        let src = r#"
module align(input [7:0] bus, output [9:0] y);
  assign y = {bus, {1<<1{1'b0}}};
endmodule
"#;
        let d = synth_sv(src, "align.sv").expect("concat align");
        assert_ne!(d.attrs.get("WIDE_CONE"), Some("1"));
        assert_ne!(d.attrs.get("ASSIGN_NOT_LOWERED"), Some("1"));
        assert!(
            !d.cells.iter().any(|c| matches!(c.kind, CellKind::Lut6 { .. })),
            "zero-fill concat must not invent a LUT, cells={:?}",
            d.cells
        );
        let aliased = d.nets.iter().any(|n| {
            n.name == "bus_0" && n.endpoints.iter().any(|e| e.pin == "I")
        });
        assert!(aliased, "y[2] aliases bus[0], nets={:?}", d.nets);
    }

    #[test]
    fn zero_fill_concat_k_above_8_is_not_lowered() {
        let src = r#"
module widepad(input [3:0] bus, output [15:0] y);
  assign y = {bus, {9{1'b0}}};
endmodule
"#;
        let d = synth_sv(src, "widepad.sv").expect("wide pad");
        assert_eq!(d.attrs.get("ASSIGN_NOT_LOWERED"), Some("1"));
        assert_ne!(d.attrs.get("WIDE_CONE"), Some("1"));
        assert!(
            !d.cells.iter().any(|c| matches!(c.kind, CellKind::Lut6 { .. })),
            "unlowered zero-fill must not become a LUT"
        );
    }

    #[test]
    fn comb_named_bus_add_is_ripple_not_wide_cone() {
        let src = r#"
module BranchAdder(input  wire [31:0] pc_plus_four,
                   input  wire [31:0] extended_times_four,
                   output wire [31:0] branch_address);
  assign branch_address = pc_plus_four + extended_times_four;
endmodule
"#;
        let d = synth_sv(src, "10162_1.v").expect("BranchAdder");
        assert_ne!(d.attrs.get("WIDE_CONE"), Some("1"));
        assert_ne!(d.attrs.get("ASSIGN_NOT_LOWERED"), Some("1"));
        assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)));
        assert!(
            !d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)),
            "combinational ripple has no clock and no Hff"
        );
        let luts = d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count();
        assert!(luts >= 63, "32-bit ripple of full adders, luts={luts}");
        let driven = d.nets.iter().any(|n| {
            n.name == "branch_address_0"
                && n.endpoints.iter().any(|e| e.pin == "O")
        });
        assert!(driven, "sum bit 0 is a LUT, not a wide cone");
    }

    #[test]
    fn comb_bus_plus_small_const_is_ripple_not_a_second_operand() {
        let src = r#"
module PCPlus4(input  wire [31:0] pc,
               output wire [31:0] pc_plus_four);
  assign pc_plus_four = pc+4;
endmodule
"#;
        let d = synth_sv(src, "10174_1.v").expect("PCPlus4");
        assert_ne!(d.attrs.get("WIDE_CONE"), Some("1"));
        assert_ne!(d.attrs.get("ASSIGN_NOT_LOWERED"), Some("1"));
        assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)));
        assert!(
            !d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)),
            "combinational const add has no clock and no Hff"
        );
        for bit in 0..32 {
            let name = format!("pc_plus_four_{bit}");
            let driven = d.nets.iter().any(|n| {
                n.name == name && n.endpoints.iter().any(|e| e.pin == "O")
            });
            assert!(driven, "bit {bit} must lower, not a skipped cone");
        }
        // Carry uses the bus bit itself. No invented 32-bit addend.
        let addend = d.nets.iter().any(|n| {
            n.name.starts_with("k_")
                || n.name.starts_with("const_")
                || n.name.contains("_addend")
        });
        assert!(!addend, "must not invent a second 32-bit operand bus");
        let lut_ins: Vec<_> = d
            .nets
            .iter()
            .filter(|n| {
                n.endpoints
                    .iter()
                    .any(|e| e.pin.starts_with('I') && e.cell.starts_with("u_rac_"))
            })
            .map(|n| n.name.as_str())
            .collect();
        assert!(
            lut_ins.iter().all(|n| n.starts_with("pc_") || n.starts_with("n_rac_")),
            "ripple inputs are the bus and carry, not a second operand: {lut_ins:?}"
        );
    }

    #[test]
    fn flatten_assign_case_is_one_cap_not_a_closed_cone() {
        // Same leftover as 15011: combo case expanded to per-bit assigns,
        // no NBAs. Do not bit-blast. One flatten_cap. No Hff, no MAC.
        let mut arms = String::new();
        for i in 0..4 {
            arms.push_str(&format!(
                "          2'd{i}: sreg_n = {{data, data}} | sreg;\n"
            ));
        }
        let src = format!(
            r#"
module decode_in(input [1:0] cnt, input [15:0] data, input [31:0] sreg,
                 output [31:0] stream_data);
  reg [31:0] sreg_n;
  always @(cnt or data or sreg) begin
    sreg_n = sreg;
    case (cnt)
{arms}    endcase
  end
  assign stream_data = sreg_n;
endmodule
"#
        );
        let d = synth_sv(&src, "decode_in.v").expect("flatten cap");
        assert_eq!(d.attrs.get("FLATTEN_CAP"), Some("1"));
        assert_ne!(d.attrs.get("WIDE_CONE"), Some("1"));
        assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)));
        assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)));
        assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Lut6 { .. })));
    }

    #[test]
    fn comb_bus_minus_small_const_is_ripple_not_a_second_operand() {
        let src = r#"
module BusMinus4(input  wire [15:0] bus,
                 output wire [15:0] y);
  assign y = bus - 4;
endmodule
"#;
        let d = synth_sv(src, "bus_minus4.v").expect("bus - 4");
        assert_ne!(d.attrs.get("WIDE_CONE"), Some("1"));
        assert_ne!(d.attrs.get("FLATTEN_CAP"), Some("1"));
        assert_ne!(d.attrs.get("ASSIGN_NOT_LOWERED"), Some("1"));
        assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)));
        assert!(
            !d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)),
            "combinational const subtract has no clock and no Hff"
        );
        for bit in 0..16 {
            let name = format!("y_{bit}");
            let driven = d.nets.iter().any(|n| {
                n.name == name && n.endpoints.iter().any(|e| e.pin == "O")
            });
            assert!(driven, "bit {bit} must lower, not a skipped cone");
        }
        // Borrow uses the bus bit or a carry net. No invented subtrahend.
        let addend = d.nets.iter().any(|n| {
            n.name.starts_with("k_")
                || n.name.starts_with("const_")
                || n.name.contains("_addend")
                || n.name.contains("_subtrahend")
        });
        assert!(!addend, "must not invent a second operand bus");
        let lut_ins: Vec<_> = d
            .nets
            .iter()
            .filter(|n| {
                n.endpoints
                    .iter()
                    .any(|e| e.pin.starts_with('I') && e.cell.starts_with("u_rsc_"))
            })
            .map(|n| n.name.as_str())
            .collect();
        assert!(
            lut_ins
                .iter()
                .all(|n| n.starts_with("bus_") || n.starts_with("n_rsc_") || n.starts_with("y_")),
            "ripple inputs are the bus and borrow, not a second operand: {lut_ins:?}"
        );
    }

    #[test]
    fn comb_bus_minus_const_wider_than_32_is_not_invented() {
        let src = r#"
module WideSub(input [33:0] bus, output [33:0] y);
  assign y = bus - 4;
endmodule
"#;
        let d = synth_sv(src, "widesub.v").expect("WideSub");
        assert_eq!(d.attrs.get("ASSIGN_NOT_LOWERED"), Some("1"));
        assert_ne!(d.attrs.get("WIDE_CONE"), Some("1"));
        assert!(
            !d.cells.iter().any(|c| matches!(c.kind, CellKind::Lut6 { .. })),
            "width>32 must not invent a shorter subtractor"
        );
        assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)));
        assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)));
    }

    #[test]
    fn comb_named_bus_add_wider_than_32_is_not_invented() {
        let src = r#"
module WideAdd(input [33:0] a, input [33:0] b, output [33:0] sum);
  assign sum = a + b;
endmodule
"#;
        let d = synth_sv(src, "wideadd.v").expect("WideAdd");
        assert_eq!(d.attrs.get("ASSIGN_NOT_LOWERED"), Some("1"));
        assert_ne!(d.attrs.get("WIDE_CONE"), Some("1"));
        assert!(
            !d.cells.iter().any(|c| matches!(c.kind, CellKind::Lut6 { .. })),
            "width>32 must not invent a shorter adder"
        );
        assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)));
    }

    #[test]
    fn nested_add_concat_stays_wide_cone() {
        let src = r#"
module branch_control #(parameter DATA_WIDTH = 32, PC_WIDTH = 6, PC_OFFSET_WIDTH = 25)
  (input jmp_inst_in, input jmp_use_r_in, input branch_use_r_in, input branch_inst_in,
   input branch_result_in, input [(0-1)+PC_WIDTH:0] pc_in,
   input [DATA_WIDTH-1:0] reg_a_data_in, input [DATA_WIDTH-1:0] reg_b_data_in,
   input [PC_OFFSET_WIDTH+(0-1):0] pc_offset_in,
   output select_new_pc_out, output [(0-1)+PC_WIDTH:0] pc_out);
  wire [DATA_WIDTH-1:0] jmp_val;
  wire [DATA_WIDTH-1:0] branch_val;
  wire [(0-1)+PC_WIDTH:0] pc_jump;
  assign pc_jump = {pc_offset_in,{1<<1{1'b0}}};
  assign select_new_pc_out = (jmp_inst_in | branch_result_in) & (jmp_inst_in | branch_inst_in);
  assign branch_val = branch_use_r_in ? reg_a_data_in : (pc_in+(4+{reg_b_data_in,{1<<1{1'b0}}}));
  assign jmp_val = jmp_use_r_in ? reg_a_data_in : pc_jump;
  assign pc_out = jmp_inst_in ? jmp_val : branch_val;
endmodule
"#;
        let d = synth_sv(src, "branch_control.v").expect("branch_control");
        assert_eq!(d.attrs.get("WIDE_CONE"), Some("1"));
        assert_ne!(d.attrs.get("ASSIGN_NOT_LOWERED"), Some("1"));
        assert!(
            !d.cells.iter().any(|c| {
                matches!(c.kind, CellKind::Lut6 { .. })
                    && d.net_on(&c.name, "O").is_some_and(|o| o.starts_with("pc_jump"))
            }),
            "pc_jump concat must not be a LUT"
        );
    }

    #[test]
    fn signal_index_read_is_not_const_word_alias() {
        let src = r#"
module vidx(input clk, input [1:0] sel, input [3:0] a, input [3:0] b, output [3:0] q);
  reg [3:0] res[0:3];
  assign q = res[sel];
  always @(posedge clk) res[0] <= a+b;
endmodule
"#;
        let d = synth_sv(src, "vidx.v").expect("vidx");
        let aliased = d.nets.iter().any(|n| {
            n.name.starts_with("res_w0_")
                && n.endpoints
                    .iter()
                    .any(|e| e.pin == "I" && e.cell.starts_with("u_iob"))
        });
        assert!(!aliased, "signal index must not alias q onto res[0]");
        assert!(d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)));
    }
}
