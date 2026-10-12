//! SystemVerilog Assertions, v3.0 subset (verification track, Part B).
//!
//! Tight subset: immediate `assert` / `assert final` in procedural blocks,
//! concurrent `assert property` with `|->`, `|=>`, fixed `##N`, and
//! `$rose` / `$fell` / `$stable`. Everything else is rejected with a named
//! "not in v3.0 subset" error at parse time, never silently skipped
//! (`assume` / `cover` keep their historical silent skip).
//!
//! Checkers are software sidecars in `helion-sim`, never synthesized
//! hardware. This module parses assertions, elaborates them to owned,
//! comb-inlined, bit-flattened boolean expressions over net-name probes
//! ([`SvaProperty`] / [`SvaImmProp`]), and evaluates them purely via
//! [`eval`]. `helion-sim` resolves the probes at attach time and samples
//! each posedge.

use helion_ir::{CellKind, Design, PortDir};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::{
    bit_name, clear_function_notes, expand_includes, parse_preprocessed, pkg_enums_clear,
    preprocess_sv, set_readmem_origin, sibling_modules, sig_width, strip_comments,
    synth_from_parsed_top, take_sva_error, top_module_name, RExpr, Rtl,
};

/// Severity of an assertion, from the optional `else` action.
/// No `else` defaults to `Error` (a failed assert without an action is an error).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvaSeverity {
    Error,
    Warning,
    Info,
}

impl SvaSeverity {
    /// Short tag used in the canonical failure line: `[SVA][ERROR] ...`.
    pub fn tag(self) -> &'static str {
        match self {
            SvaSeverity::Error => "ERROR",
            SvaSeverity::Warning => "WARNING",
            SvaSeverity::Info => "INFO",
        }
    }
}

/// `$rose` / `$fell` / `$stable` sampled-value functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvaSampleFn {
    Rose,
    Fell,
    Stable,
}

impl SvaSampleFn {
    pub fn name(self) -> &'static str {
        match self {
            SvaSampleFn::Rose => "$rose",
            SvaSampleFn::Fell => "$fell",
            SvaSampleFn::Stable => "$stable",
        }
    }
}

/// Parse-output boolean expression for one side of a property.
/// Borrows the parse arena like [`RExpr`]. Boolean connectives are hoisted
/// into [`SvaBool`] nodes so sampled-function calls become explicit
/// [`SvaBool::Sampled`] nodes; every [`SvaBool::Expr`] leaf is call-free.
#[derive(Clone, Debug)]
pub(crate) enum SvaBool<'a> {
    /// Call-free [`RExpr`] leaf.
    Expr(RExpr<'a>),
    Not(Box<SvaBool<'a>>),
    And(Box<SvaBool<'a>>, Box<SvaBool<'a>>),
    Or(Box<SvaBool<'a>>, Box<SvaBool<'a>>),
    Sampled {
        func: SvaSampleFn,
        arg: Box<SvaBool<'a>>,
    },
}

/// Which overlapping implication was written (kept for the canonical text;
/// [`ConcPropAst::delay`] is the normalized cycle delay).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvaImplKind {
    /// `|->`: consequent checked `delay` cycles after the antecedent.
    Overlapped,
    /// `|=>`: like `|-> ##1`, so `a |=> ##N b` has delay N+1.
    NonOverlapped,
}

/// Parse output of one concurrent `assert property`.
/// Borrows the parse arena.
#[derive(Clone, Debug)]
pub struct ConcPropAst<'a> {
    pub label: Option<String>,
    pub clk: String,
    /// True when the clocking event was `@(negedge ...)`: parses, but
    /// elaboration rejects it (the kernel is posedge-only).
    pub negedge: bool,
    pub(crate) antecedent: SvaBool<'a>,
    /// Normalized delay: `|-> ##N` gives N, `|=> ##N` gives N+1.
    pub delay: u32,
    pub(crate) consequent: SvaBool<'a>,
    pub severity: SvaSeverity,
    pub else_msg: Option<String>,
    /// Canonical reconstruction, used verbatim in failure reports.
    pub text: String,
}

/// Parse output of one immediate `assert` / `assert final`.
/// Borrows the parse arena.
#[derive(Clone, Debug)]
pub struct ImmAssertAst<'a> {
    pub label: Option<String>,
    pub is_final: bool,
    pub(crate) expr: SvaBool<'a>,
    pub severity: SvaSeverity,
    pub else_msg: Option<String>,
    /// Canonical reconstruction, used verbatim in failure reports.
    pub text: String,
}

/// Rewrite a parsed [`RExpr`] into a [`SvaBool`]: `$rose` / `$fell` /
/// `$stable` calls become [`SvaBool::Sampled`] nodes, boolean connectives are
/// hoisted, and anything else becomes a call-free [`SvaBool::Expr`] leaf.
/// Any other call (user function, `$past`, ...) is a named out-of-subset error.
pub(crate) fn rewrite_sva_bool<'a>(e: &RExpr<'a>) -> Result<SvaBool<'a>, String> {
    match e {
        RExpr::Call { name, args } => {
            let func = match *name {
                "$rose" => SvaSampleFn::Rose,
                "$fell" => SvaSampleFn::Fell,
                "$stable" => SvaSampleFn::Stable,
                "$past" | "$sampled" | "$isunknown" | "$onehot" | "$onehot0" | "$countones" => {
                    return Err(super::sva_err(format!(
                        "`{name}` is not in the v3.0 SVA subset"
                    )));
                }
                _ => {
                    return Err(super::sva_err(format!(
                        "function call `{name}` in a property is not in the v3.0 SVA subset"
                    )));
                }
            };
            if args.len() != 1 {
                return Err(super::sva_err(format!(
                    "`{name}` takes exactly one argument"
                )));
            }
            Ok(SvaBool::Sampled {
                func,
                arg: Box::new(rewrite_sva_bool(&args[0])?),
            })
        }
        RExpr::Not(x) => Ok(SvaBool::Not(Box::new(rewrite_sva_bool(x)?))),
        RExpr::And(a, b) => Ok(SvaBool::And(
            Box::new(rewrite_sva_bool(a)?),
            Box::new(rewrite_sva_bool(b)?),
        )),
        RExpr::Or(a, b) => Ok(SvaBool::Or(
            Box::new(rewrite_sva_bool(a)?),
            Box::new(rewrite_sva_bool(b)?),
        )),
        _ => Ok(SvaBool::Expr(e.clone())),
    }
}
/// Elaborated probe: one bit of signal state the sim checker samples.
/// `FfBit` reads a live FF Q; `InputBit` reads the SVA stimulus map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SvaProbe {
    /// Net driven by an `Hff` Q pin.
    FfBit { net: String },
    /// Top-level input port net (sampled from the checker's stimulus map).
    InputBit { name: String },
}

/// Elaborated, owned, sim-ready boolean expression over probe slots.
/// Pure: [`eval`] needs only the probe bit vector and the sampled-value
/// history, never the [`Design`] or the [`Sim`](helion_sim).
///
/// Width rules (documented simplifications): bitwise/arithmetic operators
/// take the max operand width and mask to it; comparisons, reductions, and
/// sampled functions are 1 bit; `Mux` takes the max branch width. Operands
/// are always masked to their own width before use, so multi-bit values
/// never leak bits above their width.
#[derive(Clone, Debug, PartialEq)]
pub enum SvaExpr {
    /// Constant value + width (width in 1..=64).
    Const(u64, u8),
    /// One probe slot (1 bit).
    Bit(usize),
    /// LSB-first probe slots (multi-bit signal value, packed).
    Bus(Vec<usize>),
    Not(Box<SvaExpr>),
    And(Box<SvaExpr>, Box<SvaExpr>),
    Or(Box<SvaExpr>, Box<SvaExpr>),
    Xor(Box<SvaExpr>, Box<SvaExpr>),
    Add(Box<SvaExpr>, Box<SvaExpr>),
    Sub(Box<SvaExpr>, Box<SvaExpr>),
    Mul(Box<SvaExpr>, Box<SvaExpr>),
    Shl(Box<SvaExpr>, Box<SvaExpr>),
    Shr(Box<SvaExpr>, Box<SvaExpr>),
    Eq(Box<SvaExpr>, Box<SvaExpr>),
    Ne(Box<SvaExpr>, Box<SvaExpr>),
    Lt(Box<SvaExpr>, Box<SvaExpr>),
    Mux(Box<SvaExpr>, Box<SvaExpr>, Box<SvaExpr>),
    /// Concatenation, MSB first.
    Concat(Vec<SvaExpr>),
    /// Bit slice: `arg >> lo`, masked to `width` bits.
    Slice { arg: Box<SvaExpr>, lo: u8, width: u8 },
    RedAnd(Box<SvaExpr>),
    RedOr(Box<SvaExpr>),
    RedXor(Box<SvaExpr>),
    /// Sampled-value function. `slot` indexes the checker's history vector,
    /// which holds the previous-cycle value of `arg`.
    Sampled {
        func: SvaSampleFn,
        arg: Box<SvaExpr>,
        slot: usize,
    },
}

fn mask(v: u64, w: u8) -> u64 {
    if w >= 64 {
        v
    } else {
        v & ((1u64 << w) - 1)
    }
}

fn eval_bin<F>(a: &SvaExpr, b: &SvaExpr, probes: &[bool], hist: &[u64], f: F) -> (u64, u8)
where
    F: Fn(u64, u64) -> u64,
{
    let (x, wx) = eval(a, probes, hist);
    let (y, wy) = eval(b, probes, hist);
    let w = wx.max(wy);
    (mask(f(mask(x, wx), mask(y, wy)), w), w)
}

/// Pure expression evaluation: `(value, width)`. No `Sim` borrow, no
/// [`Design`] access; trivially unit-testable.
///
/// `$rose` / `$fell` / `$stable` read the previous-cycle value from `hist`
/// at their `slot`. A missing history entry behaves as "previous = current"
/// (`$stable` = 1, `$rose` = `$fell` = 0); checkers additionally pre-fill
/// history on the first sample so there are no spurious time-0 failures.
pub fn eval(e: &SvaExpr, probes: &[bool], hist: &[u64]) -> (u64, u8) {
    match e {
        SvaExpr::Const(v, w) => (mask(*v, *w), *w),
        SvaExpr::Bit(s) => (u64::from(probes.get(*s).copied().unwrap_or(false)), 1),
        SvaExpr::Bus(slots) => {
            let mut v = 0u64;
            for (i, s) in slots.iter().enumerate().take(64) {
                if probes.get(*s).copied().unwrap_or(false) {
                    v |= 1 << i;
                }
            }
            (v, slots.len().min(64) as u8)
        }
        SvaExpr::Not(x) => {
            let (v, w) = eval(x, probes, hist);
            (mask(!v, w), w)
        }
        SvaExpr::And(a, b) => eval_bin(a, b, probes, hist, |x, y| x & y),
        SvaExpr::Or(a, b) => eval_bin(a, b, probes, hist, |x, y| x | y),
        SvaExpr::Xor(a, b) => eval_bin(a, b, probes, hist, |x, y| x ^ y),
        SvaExpr::Add(a, b) => eval_bin(a, b, probes, hist, |x, y| x.wrapping_add(y)),
        SvaExpr::Sub(a, b) => eval_bin(a, b, probes, hist, |x, y| x.wrapping_sub(y)),
        SvaExpr::Mul(a, b) => eval_bin(a, b, probes, hist, |x, y| x.wrapping_mul(y)),
        SvaExpr::Shl(a, b) => {
            let (x, wx) = eval(a, probes, hist);
            let (y, _) = eval(b, probes, hist);
            (mask(x << y.min(63), wx), wx)
        }
        SvaExpr::Shr(a, b) => {
            let (x, wx) = eval(a, probes, hist);
            let (y, _) = eval(b, probes, hist);
            (mask(x >> y.min(63), wx), wx)
        }
        SvaExpr::Eq(a, b) => {
            let (x, wx) = eval(a, probes, hist);
            let (y, wy) = eval(b, probes, hist);
            (u64::from(mask(x, wx) == mask(y, wy)), 1)
        }
        SvaExpr::Ne(a, b) => {
            let (x, wx) = eval(a, probes, hist);
            let (y, wy) = eval(b, probes, hist);
            (u64::from(mask(x, wx) != mask(y, wy)), 1)
        }
        SvaExpr::Lt(a, b) => {
            let (x, wx) = eval(a, probes, hist);
            let (y, wy) = eval(b, probes, hist);
            (u64::from(mask(x, wx) < mask(y, wy)), 1)
        }
        SvaExpr::Mux(c, t, f) => {
            let (cv, _) = eval(c, probes, hist);
            let (tv, tw) = eval(t, probes, hist);
            let (fv, fw) = eval(f, probes, hist);
            let w = tw.max(fw);
            (mask(if cv != 0 { tv } else { fv }, w), w)
        }
        SvaExpr::Concat(parts) => {
            let mut v = 0u64;
            let mut w = 0u8;
            for p in parts {
                let (pv, pw) = eval(p, probes, hist);
                v = (v << pw) | mask(pv, pw);
                w = w.saturating_add(pw).min(64);
            }
            (v, w)
        }
        SvaExpr::Slice { arg, lo, width } => {
            let (v, _) = eval(arg, probes, hist);
            (mask(v >> lo, *width), *width)
        }
        SvaExpr::RedAnd(x) => {
            let (v, w) = eval(x, probes, hist);
            (u64::from(w > 0 && mask(v, w) == mask(u64::MAX, w)), 1)
        }
        SvaExpr::RedOr(x) => {
            let (v, w) = eval(x, probes, hist);
            (u64::from(mask(v, w) != 0), 1)
        }
        SvaExpr::RedXor(x) => {
            let (v, w) = eval(x, probes, hist);
            (u64::from(mask(v, w).count_ones() % 2 == 1), 1)
        }
        SvaExpr::Sampled { func, arg, slot } => {
            let (cur, w) = eval(arg, probes, hist);
            let prev = hist.get(*slot).copied().unwrap_or(cur);
            let r = match func {
                // Whole-value equality with the previous cycle, masked.
                SvaSampleFn::Stable => mask(cur, w) == mask(prev, w),
                // LSB transitions.
                SvaSampleFn::Rose => cur & 1 == 1 && prev & 1 == 0,
                SvaSampleFn::Fell => cur & 1 == 0 && prev & 1 == 1,
            };
            (u64::from(r), 1)
        }
    }
}

/// Elaborated concurrent property, ready for the sim checker sidecar.
#[derive(Clone, Debug)]
pub struct SvaProperty {
    pub label: Option<String>,
    /// Canonical text, used verbatim in failure reports.
    pub text: String,
    pub severity: SvaSeverity,
    pub else_msg: Option<String>,
    /// Normalized consequent delay in cycles (`|-> ##N` = N, `|=> ##N` = N+1).
    pub delay: u32,
    pub antecedent: SvaExpr,
    pub consequent: SvaExpr,
    /// Probe slots referenced by [`SvaExpr::Bit`] / [`SvaExpr::Bus`].
    pub probes: Vec<SvaProbe>,
    /// History slots for `$rose` / `$fell` / `$stable` (history vector length).
    pub slots: usize,
}

/// Elaborated immediate assertion, ready for the sim checker sidecar.
#[derive(Clone, Debug)]
pub struct SvaImmProp {
    pub label: Option<String>,
    /// Canonical text, used verbatim in failure reports.
    pub text: String,
    /// `assert final`: evaluated in the postponed region (after FF commit).
    pub is_final: bool,
    pub severity: SvaSeverity,
    pub else_msg: Option<String>,
    pub expr: SvaExpr,
    pub probes: Vec<SvaProbe>,
    pub slots: usize,
}

/// Output of [`synth_sv_with_sva`]: the synthesized design plus the
/// elaborated SVA checkers for it.
#[derive(Debug)]
pub struct SvaSynthOut {
    pub design: Design,
    pub properties: Vec<SvaProperty>,
    pub immediates: Vec<SvaImmProp>,
}

/// Render a call-free [`RExpr`] leaf back to canonical source text.
fn rexpr_text(e: &RExpr) -> String {
    match e {
        RExpr::Const { val, width, .. } => {
            if *width <= 1 {
                format!("{}", val & 1)
            } else {
                format!("{width}'d{val}")
            }
        }
        RExpr::Ident(n) => n.to_string(),
        RExpr::Bit(n, b) => format!("{n}[{b}]"),
        RExpr::Range(n, hi, lo) => format!("{n}[{hi}:{lo}]"),
        RExpr::IndexPart {
            name,
            base,
            width,
            ascending,
        } => format!(
            "{name}[{} {}: {}]",
            rexpr_text(base),
            if *ascending { "+" } else { "-" },
            width
        ),
        RExpr::WordAt { .. } => "<mem>".to_string(),
        RExpr::Concat(parts) => format!(
            "{{{}}}",
            parts
                .iter()
                .map(rexpr_text)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        RExpr::Shr(a, b) => format!("({} >> {})", rexpr_text(a), rexpr_text(b)),
        RExpr::Shl(a, b) => format!("({} << {})", rexpr_text(a), rexpr_text(b)),
        RExpr::Ashr(a, b) => format!("({} >>> {})", rexpr_text(a), rexpr_text(b)),
        RExpr::RedXor(x) => format!("(^{})", rexpr_text(x)),
        RExpr::RedAnd(x) => format!("(&{})", rexpr_text(x)),
        RExpr::RedOr(x) => format!("(|{})", rexpr_text(x)),
        RExpr::Not(x) => format!("(!{})", rexpr_text(x)),
        RExpr::And(a, b) => format!("({} & {})", rexpr_text(a), rexpr_text(b)),
        RExpr::Or(a, b) => format!("({} | {})", rexpr_text(a), rexpr_text(b)),
        RExpr::Xor(a, b) => format!("({} ^ {})", rexpr_text(a), rexpr_text(b)),
        RExpr::Add(a, b) => format!("({} + {})", rexpr_text(a), rexpr_text(b)),
        RExpr::Sub(a, b) => format!("({} - {})", rexpr_text(a), rexpr_text(b)),
        RExpr::Mul(a, b) => format!("({} * {})", rexpr_text(a), rexpr_text(b)),
        RExpr::Mux(c, t, f) => format!(
            "({} ? {} : {})",
            rexpr_text(c),
            rexpr_text(t),
            rexpr_text(f)
        ),
        RExpr::Eq(a, b) => format!("({} == {})", rexpr_text(a), rexpr_text(b)),
        RExpr::Ne(a, b) => format!("({} != {})", rexpr_text(a), rexpr_text(b)),
        RExpr::Lt(a, b) => format!("({} < {})", rexpr_text(a), rexpr_text(b)),
        RExpr::Call { name, args } => format!(
            "{name}({})",
            args.iter().map(rexpr_text).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// Parenthesize a [`SvaBool`] child unless it is a leaf or a sampled call.
fn svabool_child(b: &SvaBool) -> String {
    match b {
        SvaBool::Expr(_) | SvaBool::Sampled { .. } => svabool_text(b),
        _ => format!("({})", svabool_text(b)),
    }
}

/// Render a [`SvaBool`] to canonical source text.
fn svabool_text(b: &SvaBool) -> String {
    match b {
        SvaBool::Expr(e) => rexpr_text(e),
        SvaBool::Not(x) => format!("!{}", svabool_child(x)),
        SvaBool::And(a, b) => format!("{} && {}", svabool_child(a), svabool_child(b)),
        SvaBool::Or(a, b) => format!("{} || {}", svabool_child(a), svabool_child(b)),
        SvaBool::Sampled { func, arg } => format!("{}({})", func.name(), svabool_child(arg)),
    }
}

fn else_text(severity: SvaSeverity, else_msg: &Option<String>) -> String {
    let action = match severity {
        SvaSeverity::Error => "$error",
        SvaSeverity::Warning => "$warning",
        SvaSeverity::Info => "$info",
    };
    match else_msg {
        Some(m) => format!(" else {action}(\"{m}\")"),
        None => String::new(),
    }
}

/// Canonical text for a concurrent assertion, used verbatim in reports:
/// `assert property (@(posedge clk) en |-> ##1 $stable(cnt))`.
/// The statement label (if any) is kept on the AST, not in the text.
#[allow(clippy::too_many_arguments)]
pub(crate) fn conc_text(
    clk: &str,
    negedge: bool,
    antecedent: &SvaBool,
    impl_kind: SvaImplKind,
    delay_lit: Option<u32>,
    consequent: &SvaBool,
    severity: SvaSeverity,
    else_msg: &Option<String>,
) -> String {
    let edge = if negedge { "negedge" } else { "posedge" };
    let op = match impl_kind {
        SvaImplKind::Overlapped => "|->",
        SvaImplKind::NonOverlapped => "|=>",
    };
    let dl = delay_lit
        .map(|n| format!(" ##{n}"))
        .unwrap_or_default();
    // An explicit `else` is only rendered when the user wrote one; the
    // default severity (Error) carries no action text.
    let el = if else_msg.is_some() {
        else_text(severity, else_msg)
    } else {
        String::new()
    };
    format!(
        "assert property (@({edge} {clk}) {} {op}{dl} {}){el}",
        strip_outer_parens(&svabool_text(antecedent)),
        strip_outer_parens(&svabool_text(consequent)),
    )
}

/// Strip one balanced outer paren pair: `(a < b)` -> `a < b`.
fn strip_outer_parens(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2 && b[0] == b'(' && b[b.len() - 1] == b')' {
        let mut d = 0i32;
        for (i, &c) in b.iter().enumerate() {
            if c == b'(' {
                d += 1;
            } else if c == b')' {
                d -= 1;
            }
            if d == 0 && i < b.len() - 1 {
                return s;
            }
        }
        return &s[1..s.len() - 1];
    }
    s
}

/// Canonical text for an immediate assertion:
/// `assert (cnt < 4'd8)` / `assert final (e) else $error("msg")`.
/// The statement label (if any) is kept on the AST, not in the text.
pub(crate) fn imm_text(
    is_final: bool,
    expr: &SvaBool,
    severity: SvaSeverity,
    else_msg: &Option<String>,
) -> String {
    let fin = if is_final { "final " } else { "" };
    let el = if else_msg.is_some() {
        else_text(severity, else_msg)
    } else {
        String::new()
    };
    format!(
        "assert {fin}({}){el}",
        strip_outer_parens(&svabool_text(expr))
    )
}

/// Elaboration context: lowers borrowed [`SvaBool`] parse trees to owned
/// [`SvaExpr`] over interned [`SvaProbe`] slots.
struct Elab<'r> {
    rtl: &'r Rtl<'r>,
    design: &'r Design,
    /// Whole-signal, single-driver continuous assigns: inlined into properties.
    subst: HashMap<&'r str, &'r RExpr<'r>>,
    probes: Vec<SvaProbe>,
    /// Sampled-function history slots allocated so far.
    slots: usize,
    /// Names currently being inlined (combinational-loop guard).
    inline_stack: Vec<&'r str>,
    /// Canonical property text, for fail-loud error messages.
    prop_text: String,
}

/// Classify one net-name probe: an `Hff` Q endpoint becomes [`SvaProbe::FfBit`],
/// a top-level input port becomes [`SvaProbe::InputBit`]. Anything else is a
/// hard error naming the property and the signal: a silently dropped
/// assertion is worse than a loud one.
fn classify_probe(
    design: &Design,
    net: &str,
    prop_text: &str,
    sig: &str,
) -> Result<SvaProbe, String> {
    if let Some(n) = design.net(net) {
        if let Some(q) = n.endpoints.iter().find(|e| e.pin == "Q") {
            let is_ff = design
                .cell(&q.cell)
                .map(|c| matches!(c.kind, CellKind::Hff))
                .unwrap_or(false);
            if is_ff {
                return Ok(SvaProbe::FfBit {
                    net: net.to_string(),
                });
            }
        }
    }
    if design
        .ports
        .iter()
        .any(|p| p.name == net && matches!(p.dir, PortDir::In | PortDir::Inout))
    {
        return Ok(SvaProbe::InputBit {
            name: net.to_string(),
        });
    }
    Err(super::sva_err(format!(
        "property `{prop_text}` references signal `{sig}` (net `{net}`): \
         not a register bit or an input port; the v3.0 subset only probes FF Q and input ports"
    )))
}

impl<'r> Elab<'r> {
    /// Reject `a.b` hierarchical references with a named error.
    fn check_hier(&self, name: &str) -> Result<(), String> {
        if name.contains('.') {
            return Err(super::sva_err(format!(
                "property `{}`: hierarchical reference `{name}` is not in the v3.0 subset",
                self.prop_text
            )));
        }
        Ok(())
    }

    /// Intern one bit probe, returning its slot.
    fn probe_bit(&mut self, net: String, sig: &str) -> Result<usize, String> {
        let probe = classify_probe(self.design, &net, &self.prop_text, sig)?;
        if let Some(ix) = self.probes.iter().position(|p| p == &probe) {
            return Ok(ix);
        }
        self.probes.push(probe);
        Ok(self.probes.len() - 1)
    }

    /// Lower one signal name to [`SvaExpr::Bit`] / [`SvaExpr::Bus`] over
    /// interned probe slots. Net names follow the [`bit_name`] convention
    /// (`sig` / `sig_{i}`, LSB first), matching the synthesized netlist.
    fn lower_signal(&mut self, name: &'r str, width: usize) -> Result<SvaExpr, String> {
        if width > 64 {
            return Err(super::sva_err(format!(
                "property `{}`: signal `{name}` is {width} bits wide; the v3.0 subset caps expressions at 64 bits",
                self.prop_text
            )));
        }
        let mut slots = Vec::with_capacity(width);
        for i in 0..width {
            slots.push(self.probe_bit(bit_name(name, width, i), name)?);
        }
        if slots.len() == 1 {
            Ok(SvaExpr::Bit(slots[0]))
        } else {
            Ok(SvaExpr::Bus(slots))
        }
    }

    /// Constant-fold a part-select base; only literals are supported.
    fn const_base(&self, base: &'r RExpr<'r>) -> Result<usize, String> {
        match base {
            RExpr::Const { val, .. } => Ok((*val).min(usize::MAX as u128) as usize),
            _ => Err(super::sva_err(format!(
                "property `{}`: non-constant part-select base is not in the v3.0 subset",
                self.prop_text
            ))),
        }
    }

    fn lower_rexpr(&mut self, e: &'r RExpr<'r>, depth: usize) -> Result<SvaExpr, String> {
        if depth > 64 {
            return Err(super::sva_err(format!(
                "property `{}`: expression nesting exceeds 64",
                self.prop_text
            )));
        }
        let d1 = depth + 1;
        match e {
            RExpr::Const { val, width, .. } => {
                if *width > 64 {
                    return Err(super::sva_err(format!(
                        "property `{}`: constant wider than 64 bits is not in the v3.0 subset",
                        self.prop_text
                    )));
                }
                Ok(SvaExpr::Const(*val as u64, (*width).clamp(1, 64) as u8))
            }
            RExpr::Ident(name) => {
                self.check_hier(name)?;
                // Inline whole-signal, single-driver continuous assigns.
                if let Some(rhs) = self.subst.get(name) {
                    if self.inline_stack.contains(name) {
                        return Err(super::sva_err(format!(
                            "property `{}`: combinational loop while inlining `{name}`",
                            self.prop_text
                        )));
                    }
                    if self.inline_stack.len() >= 32 {
                        return Err(super::sva_err(format!(
                            "property `{}`: substitution depth exceeded while inlining `{name}`",
                            self.prop_text
                        )));
                    }
                    self.inline_stack.push(name);
                    let r = self.lower_rexpr(rhs, d1);
                    self.inline_stack.pop();
                    return r;
                }
                let w = sig_width(self.rtl, name).max(1);
                self.lower_signal(name, w)
            }
            RExpr::Bit(name, b) => {
                self.check_hier(name)?;
                let sw = sig_width(self.rtl, name).max(1);
                let bit = (*b).min(sw.saturating_sub(1));
                let slot = self.probe_bit(bit_name(name, sw, bit), name)?;
                Ok(SvaExpr::Bit(slot))
            }
            RExpr::Range(name, hi, lo) => {
                self.check_hier(name)?;
                let sw = sig_width(self.rtl, name).max(1);
                let (lo, hi) = ((*lo).min(*hi), (*lo).max(*hi));
                let w = hi - lo + 1;
                if w > 64 {
                    return Err(super::sva_err(format!(
                        "property `{}`: part-select wider than 64 bits is not in the v3.0 subset",
                        self.prop_text
                    )));
                }
                let mut slots = Vec::with_capacity(w);
                for i in 0..w {
                    let bit = (lo + i).min(sw.saturating_sub(1));
                    slots.push(self.probe_bit(bit_name(name, sw, bit), name)?);
                }
                if slots.len() == 1 {
                    Ok(SvaExpr::Bit(slots[0]))
                } else {
                    Ok(SvaExpr::Bus(slots))
                }
            }
            RExpr::IndexPart {
                name,
                base,
                width,
                ascending,
            } => {
                self.check_hier(name)?;
                let sw = sig_width(self.rtl, name).max(1);
                if *width > 64 {
                    return Err(super::sva_err(format!(
                        "property `{}`: part-select wider than 64 bits is not in the v3.0 subset",
                        self.prop_text
                    )));
                }
                let b = self.const_base(base)?;
                let mut slots = Vec::with_capacity(*width);
                for i in 0..*width {
                    let bit = if *ascending {
                        b.saturating_add(i)
                    } else {
                        b.saturating_sub(i)
                    }
                    .min(sw.saturating_sub(1));
                    slots.push(self.probe_bit(bit_name(name, sw, bit), name)?);
                }
                if slots.len() == 1 {
                    Ok(SvaExpr::Bit(slots[0]))
                } else {
                    Ok(SvaExpr::Bus(slots))
                }
            }
            RExpr::WordAt { .. } => Err(super::sva_err(format!(
                "property `{}`: memory reads are not in the v3.0 subset",
                self.prop_text
            ))),
            RExpr::Concat(parts) => {
                let mut out = Vec::with_capacity(parts.len());
                for p in parts {
                    out.push(self.lower_rexpr(p, d1)?);
                }
                Ok(SvaExpr::Concat(out))
            }
            RExpr::Shr(a, b) => Ok(SvaExpr::Shr(
                Box::new(self.lower_rexpr(a, d1)?),
                Box::new(self.lower_rexpr(b, d1)?),
            )),
            RExpr::Shl(a, b) => Ok(SvaExpr::Shl(
                Box::new(self.lower_rexpr(a, d1)?),
                Box::new(self.lower_rexpr(b, d1)?),
            )),
            RExpr::Ashr(..) => Err(super::sva_err(format!(
                "property `{}`: `>>>` is not in the v3.0 subset",
                self.prop_text
            ))),
            RExpr::RedXor(x) => Ok(SvaExpr::RedXor(Box::new(self.lower_rexpr(x, d1)?))),
            RExpr::RedAnd(x) => Ok(SvaExpr::RedAnd(Box::new(self.lower_rexpr(x, d1)?))),
            RExpr::RedOr(x) => Ok(SvaExpr::RedOr(Box::new(self.lower_rexpr(x, d1)?))),
            RExpr::Not(x) => Ok(SvaExpr::Not(Box::new(self.lower_rexpr(x, d1)?))),
            RExpr::And(a, b) => self.lower_bin(a, b, d1, SvaExpr::And),
            RExpr::Or(a, b) => self.lower_bin(a, b, d1, SvaExpr::Or),
            RExpr::Xor(a, b) => self.lower_bin(a, b, d1, SvaExpr::Xor),
            RExpr::Add(a, b) => self.lower_bin(a, b, d1, SvaExpr::Add),
            RExpr::Sub(a, b) => self.lower_bin(a, b, d1, SvaExpr::Sub),
            RExpr::Mul(a, b) => self.lower_bin(a, b, d1, SvaExpr::Mul),
            RExpr::Mux(c, t, f) => Ok(SvaExpr::Mux(
                Box::new(self.lower_rexpr(c, d1)?),
                Box::new(self.lower_rexpr(t, d1)?),
                Box::new(self.lower_rexpr(f, d1)?),
            )),
            RExpr::Eq(a, b) => self.lower_bin(a, b, d1, SvaExpr::Eq),
            RExpr::Ne(a, b) => self.lower_bin(a, b, d1, SvaExpr::Ne),
            RExpr::Lt(a, b) => self.lower_bin(a, b, d1, SvaExpr::Lt),
            RExpr::Call { name, .. } => Err(super::sva_err(format!(
                "property `{}`: function call `{name}` is not in the v3.0 subset",
                self.prop_text
            ))),
        }
    }

    fn lower_bin(
        &mut self,
        a: &'r RExpr<'r>,
        b: &'r RExpr<'r>,
        depth: usize,
        mk: fn(Box<SvaExpr>, Box<SvaExpr>) -> SvaExpr,
    ) -> Result<SvaExpr, String> {
        Ok(mk(
            Box::new(self.lower_rexpr(a, depth)?),
            Box::new(self.lower_rexpr(b, depth)?),
        ))
    }

    fn lower_bool(&mut self, b: &'r SvaBool<'r>, depth: usize) -> Result<SvaExpr, String> {
        if depth > 64 {
            return Err(super::sva_err(format!(
                "property `{}`: expression nesting exceeds 64",
                self.prop_text
            )));
        }
        let d1 = depth + 1;
        match b {
            SvaBool::Expr(e) => self.lower_rexpr(e, d1),
            SvaBool::Not(x) => Ok(SvaExpr::Not(Box::new(self.lower_bool(x, d1)?))),
            SvaBool::And(a, b) => Ok(SvaExpr::And(
                Box::new(self.lower_bool(a, d1)?),
                Box::new(self.lower_bool(b, d1)?),
            )),
            SvaBool::Or(a, b) => Ok(SvaExpr::Or(
                Box::new(self.lower_bool(a, d1)?),
                Box::new(self.lower_bool(b, d1)?),
            )),
            SvaBool::Sampled { func, arg } => {
                let slot = self.slots;
                self.slots += 1;
                let lowered = self.lower_bool(arg, d1)?;
                Ok(SvaExpr::Sampled {
                    func: *func,
                    arg: Box::new(lowered),
                    slot,
                })
            }
        }
    }
}

/// Build the comb-substitution map: whole-signal, single-driver continuous
/// assigns from the top module's [`Rtl::assigns`].
fn build_subst<'r>(rtl: &'r Rtl<'r>) -> HashMap<&'r str, &'r RExpr<'r>> {
    let mut counts: HashMap<&'r str, usize> = HashMap::new();
    for (lhs, bit, _) in &rtl.assigns {
        if bit.is_none() {
            *counts.entry(*lhs).or_default() += 1;
        }
    }
    let mut map = HashMap::new();
    for (lhs, bit, rhs) in &rtl.assigns {
        if bit.is_none() && counts.get(lhs).copied().unwrap_or(0) == 1 {
            map.insert(*lhs, rhs);
        }
    }
    map
}

/// Elaborate the top module's parsed assertions against the synthesized
/// design: inline comb assigns, flatten to bit probes, allocate sampled-value
/// history slots. Unresolvable probes are hard errors naming the property
/// and the signal.
pub(crate) fn elaborate<'r>(
    top: &'r Rtl<'r>,
    design: &Design,
) -> Result<(Vec<SvaProperty>, Vec<SvaImmProp>), String> {
    let subst = build_subst(top);
    let mut properties = Vec::with_capacity(top.sva_conc.len());
    for ast in &top.sva_conc {
        if ast.negedge {
            return Err(super::sva_err(format!(
                "property `{}` uses `@(negedge {})`: negedge sampling is not supported (the kernel is posedge-only)",
                ast.text, ast.clk
            )));
        }
        let mut elab = Elab {
            rtl: top,
            design,
            subst: subst.clone(),
            probes: Vec::new(),
            slots: 0,
            inline_stack: Vec::new(),
            prop_text: ast.text.clone(),
        };
        let antecedent = elab.lower_bool(&ast.antecedent, 0)?;
        let consequent = elab.lower_bool(&ast.consequent, 0)?;
        properties.push(SvaProperty {
            label: ast.label.clone(),
            text: ast.text.clone(),
            severity: ast.severity,
            else_msg: ast.else_msg.clone(),
            delay: ast.delay,
            antecedent,
            consequent,
            probes: elab.probes,
            slots: elab.slots,
        });
    }
    let mut immediates = Vec::with_capacity(top.sva_imm.len());
    for ast in &top.sva_imm {
        let mut elab = Elab {
            rtl: top,
            design,
            subst: subst.clone(),
            probes: Vec::new(),
            slots: 0,
            inline_stack: Vec::new(),
            prop_text: ast.text.clone(),
        };
        let expr = elab.lower_bool(&ast.expr, 0)?;
        immediates.push(SvaImmProp {
            label: ast.label.clone(),
            text: ast.text.clone(),
            is_final: ast.is_final,
            severity: ast.severity,
            else_msg: ast.else_msg.clone(),
            expr,
            probes: elab.probes,
            slots: elab.slots,
        });
    }
    Ok((properties, immediates))
}

/// Synthesize SystemVerilog like [`super::synth_sv`], and additionally parse
/// and elaborate the v3.0 SVA subset found in the top module.
///
/// Named SVA subset rejections surface as `Err` even though the failing
/// module is skipped by the parser: they are recorded on the thread during
/// parsing and re-raised here. Assertions in non-top modules are skipped
/// with a `sva_skip_non_top` warning.
pub fn synth_sv_with_sva(source: &str, origin: &str) -> Result<SvaSynthOut, String> {
    clear_function_notes();
    pkg_enums_clear();
    set_readmem_origin(origin);
    // Drop any stale SVA rejection left by an earlier parse on this thread.
    let _ = take_sva_error();
    let t_parse = std::time::Instant::now();
    let origin_path = Path::new(origin);
    let base = origin_path
        .parent()
        .filter(|d| !d.as_os_str().is_empty() && d.exists());
    let expanded = if let Some(dir) = base {
        expand_includes(source, dir)
    } else {
        source.to_string()
    };
    let pre = preprocess_sv(&strip_comments(&expanded));
    let mut mods = match parse_preprocessed(pre) {
        Ok(m) => m,
        Err(e) => return Err(take_sva_error().unwrap_or(e)),
    };
    if let Some(dir) = base {
        let have: HashSet<String> = mods.iter().map(|m| m.module.to_string()).collect();
        let mut missing: HashSet<String> = HashSet::new();
        for m in &mods {
            for inst in &m.insts {
                if !have.contains(inst.module) {
                    missing.insert(inst.module.to_string());
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
    // A named SVA rejection aborts its module inside the parser; re-raise the
    // recorded message instead of a generic downstream error.
    if let Some(se) = take_sva_error() {
        return Err(se);
    }
    let stem = Path::new(origin)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| mods.iter().any(|m| m.module == *s));
    let top_name = top_module_name(&mods, stem)?;
    // Keep the top module's parse tree for SVA elaboration; the synth call
    // below consumes `mods`.
    let top_rtl: Rtl<'static> = mods
        .iter()
        .find(|m| m.module == top_name.as_str())
        .cloned()
        .ok_or_else(|| "no top module".to_string())?;
    for m in &mods {
        if m.module != top_name.as_str()
            && (!m.sva_conc.is_empty() || !m.sva_imm.is_empty())
        {
            eprintln!(
                "sva_skip_non_top module={} concurrent={} immediate={} \
                 (assertions in non-top modules are not checked in v3.0)",
                m.module,
                m.sva_conc.len(),
                m.sva_imm.len()
            );
        }
    }
    let design = synth_from_parsed_top(mods, Some(&top_name), &HashMap::new())?;
    let (properties, immediates) = elaborate(&top_rtl, &design)?;
    Ok(SvaSynthOut {
        design,
        properties,
        immediates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_concurrent_assert, parse_immediate_assert, tokenize, P, Tok};
    use bumpalo::Bump;
    use std::collections::{HashMap, HashSet};

    /// Build a throwaway parser over one assertion statement. The source is
    /// leaked so token spans are `'static`, mirroring `parse_preprocessed`.
    fn make_p(src: &str) -> P<'static> {
        let leaked_src: &'static str = Box::leak(src.to_string().into_boxed_str());
        let toks: &'static [Tok<'static>] =
            Box::leak(tokenize(leaked_src).expect("tokenize").into_boxed_slice());
        let arena: &'static Bump = Box::leak(Box::new(Bump::new()));
        P {
            t: toks,
            i: 0,
            arena,
            params: HashMap::new(),
            widths: HashMap::new(),
            signed: HashSet::new(),
        }
    }

    fn parse_conc(src: &str) -> ConcPropAst<'static> {
        parse_concurrent_assert(&mut make_p(src)).expect("parse_concurrent_assert")
    }

    fn parse_imm(src: &str) -> ImmAssertAst<'static> {
        parse_immediate_assert(&mut make_p(src)).expect("parse_immediate_assert")
    }

    #[test]
    fn conc_delay_normalization() {
        let a = parse_conc("assert property (@(posedge clk) a |-> b);");
        assert_eq!(a.delay, 0);
        assert_eq!(a.text, "assert property (@(posedge clk) a |-> b)");

        let a = parse_conc("assert property (@(posedge clk) a |=> b);");
        assert_eq!(a.delay, 1);
        assert_eq!(a.text, "assert property (@(posedge clk) a |=> b)");

        let a = parse_conc("assert property (@(posedge clk) a |-> ##3 b);");
        assert_eq!(a.delay, 3);
        assert_eq!(a.text, "assert property (@(posedge clk) a |-> ##3 b)");

        // `|=> ##N` is `|-> ##(N+1)`.
        let a = parse_conc("assert property (@(posedge clk) a |=> ##2 b);");
        assert_eq!(a.delay, 3);
        assert_eq!(a.text, "assert property (@(posedge clk) a |=> ##2 b)");
    }

    #[test]
    fn conc_label_and_else_action() {
        let a = parse_conc(
            "p1: assert property (@(posedge clk) en |-> ##1 $stable(cnt)) else $error(\"boom\");",
        );
        assert_eq!(a.label.as_deref(), Some("p1"));
        assert_eq!(a.severity, SvaSeverity::Error);
        assert_eq!(a.else_msg.as_deref(), Some("boom"));
        assert_eq!(
            a.text,
            "assert property (@(posedge clk) en |-> ##1 $stable(cnt)) else $error(\"boom\")"
        );

        let a = parse_conc("assert property (@(posedge clk) a |-> b) else $warning(\"w\");");
        assert_eq!(a.severity, SvaSeverity::Warning);
        assert_eq!(a.else_msg.as_deref(), Some("w"));

        let a = parse_conc("assert property (@(posedge clk) a |-> b) else $info;");
        assert_eq!(a.severity, SvaSeverity::Info);
        assert!(a.else_msg.is_none());
    }

    #[test]
    fn conc_sampled_shapes() {
        let a = parse_conc("assert property (@(posedge clk) $rose(en) |-> ##1 $stable(cnt));");
        assert!(
            matches!(a.antecedent, SvaBool::Sampled { func: SvaSampleFn::Rose, .. }),
            "{:?}",
            a.antecedent
        );
        assert!(
            matches!(a.consequent, SvaBool::Sampled { func: SvaSampleFn::Stable, .. }),
            "{:?}",
            a.consequent
        );
        assert_eq!(
            a.text,
            "assert property (@(posedge clk) $rose(en) |-> ##1 $stable(cnt))"
        );

        let a = parse_conc("assert property (@(posedge clk) $fell(en) |-> !$stable(cnt));");
        assert!(matches!(
            a.antecedent,
            SvaBool::Sampled { func: SvaSampleFn::Fell, .. }
        ));
        assert!(matches!(a.consequent, SvaBool::Not(_)));
        assert_eq!(
            a.text,
            "assert property (@(posedge clk) $fell(en) |-> !$stable(cnt))"
        );
    }

    #[test]
    fn conc_rejections_are_named() {
        let cases: &[(&str, &str)] = &[
            ("assert property (@(posedge clk) a[*3] |-> b);", "[*N]"),
            ("assert property (@(posedge clk) a[=2] |-> b);", "[=N]"),
            ("assert property (@(posedge clk) a[->1] |-> b);", "[->N]"),
            (
                "assert property (@(posedge clk) a throughout b |-> c);",
                "throughout",
            ),
            (
                "assert property (@(posedge clk) disable iff (!rst) a |-> b);",
                "disable iff",
            ),
            (
                "assert property (@(posedge clk) a |-> ##[1:3] b);",
                "##[N:M]",
            ),
            ("assert property (@(posedge clk) a |-> ##* b);", "##*"),
            (
                "assert property (@(posedge clk) a |-> ##(1+1) b);",
                "##(expr)",
            ),
            (
                "assert property (@(posedge clk) a |-> ##300 b);",
                "##N",
            ),
            ("assert property (@(posedge clk) $past(a) |-> b);", "$past"),
            (
                "assert property (@(posedge clk) myfunc(a) |-> b);",
                "myfunc",
            ),
            ("assert #0 property (@(posedge clk) a |-> b);", "assert #0"),
            (
                "assert property (@(posedge clk) a |-> b |-> c);",
                "chained",
            ),
            (
                "assert property (@(posedge clk) a ##1 b |-> c);",
                "##",
            ),
        ];
        for (src, needle) in cases {
            let e = parse_concurrent_assert(&mut make_p(src)).expect_err("must reject");
            assert!(
                e.contains(needle),
                "src={src} expected `{needle}` in `{e}`"
            );
            assert!(e.starts_with("sva:"), "named sva error: {e}");
        }
    }

    #[test]
    fn conc_negedge_parses_but_elaboration_rejects() {
        let a = parse_conc("assert property (@(negedge clk) a |-> b);");
        assert!(a.negedge);
        let src = r#"
module top (
    input logic clk,
    input logic a,
    input logic b
);
    assert property (@(negedge clk) a |-> b);
endmodule
"#;
        let e = synth_sv_with_sva(src, "top.sv").expect_err("negedge must fail");
        assert!(e.contains("negedge"), "{e}");
    }

    #[test]
    fn imm_parse_shapes() {
        let a = parse_imm("assert (cnt < 4'h8);");
        assert!(!a.is_final);
        assert_eq!(a.severity, SvaSeverity::Error);
        assert!(a.else_msg.is_none());
        assert_eq!(a.text, "assert (cnt < 4'd8)");

        let a = parse_imm("assert final (done) else $error(\"not done\");");
        assert!(a.is_final);
        assert_eq!(a.else_msg.as_deref(), Some("not done"));
        assert_eq!(a.text, "assert final (done) else $error(\"not done\")");

        let a = parse_imm("chk: assert ($stable(cnt) || en) else $warning(\"w\");");
        assert_eq!(a.label.as_deref(), Some("chk"));
        assert_eq!(a.severity, SvaSeverity::Warning);
        assert_eq!(a.text, "assert ($stable(cnt) || en) else $warning(\"w\")");
    }

    #[test]
    fn imm_rejections_are_named() {
        for (src, needle) in [
            ("assert #0 (a);", "assert #0"),
            ("assert ($fatal(1));", "$fatal"),
            ("assert (a) else b = 1;", "$error/$warning/$info"),
        ] {
            let e = parse_immediate_assert(&mut make_p(src)).expect_err("must reject");
            assert!(e.contains(needle), "src={src} expected `{needle}` in `{e}`");
        }
    }

    #[test]
    fn assume_cover_still_skipped() {
        let src = r#"
module top (
    input logic clk,
    input logic a
);
    assume property (@(posedge clk) a |-> ##1 a);
    cover property (@(posedge clk) a);
endmodule
"#;
        let out = synth_sv_with_sva(src, "top.sv").expect("synth");
        assert!(out.properties.is_empty(), "assume/cover stay skipped");
        assert!(out.immediates.is_empty());
    }

    #[test]
    fn eval_sampled_first_sample_semantics() {
        // No history: prev := current, so $stable=1, $rose=$fell=0.
        let st = SvaExpr::Sampled {
            func: SvaSampleFn::Stable,
            arg: Box::new(SvaExpr::Bit(0)),
            slot: 0,
        };
        assert_eq!(eval(&st, &[true], &[]), (1, 1));
        assert_eq!(eval(&st, &[true], &[0]), (0, 1));
        assert_eq!(eval(&st, &[true], &[1]), (1, 1));
        // Whole-value equality is masked to the arg width.
        let st4 = SvaExpr::Sampled {
            func: SvaSampleFn::Stable,
            arg: Box::new(SvaExpr::Bus(vec![0, 1, 2, 3])),
            slot: 0,
        };
        assert_eq!(
            eval(&st4, &[true, true, true, true], &[0b01111]),
            (1, 1),
            "masked to 4 bits: 0b1111 == 0b1111"
        );

        let ro = SvaExpr::Sampled {
            func: SvaSampleFn::Rose,
            arg: Box::new(SvaExpr::Bit(0)),
            slot: 0,
        };
        assert_eq!(eval(&ro, &[false], &[]), (0, 1));
        assert_eq!(eval(&ro, &[true], &[0]), (1, 1));
        assert_eq!(eval(&ro, &[true], &[1]), (0, 1));

        let fe = SvaExpr::Sampled {
            func: SvaSampleFn::Fell,
            arg: Box::new(SvaExpr::Bit(0)),
            slot: 0,
        };
        assert_eq!(eval(&fe, &[true], &[]), (0, 1));
        assert_eq!(eval(&fe, &[false], &[1]), (1, 1));
        assert_eq!(eval(&fe, &[false], &[0]), (0, 1));
    }

    #[test]
    fn eval_bus_and_operators() {
        // Bus packs LSB-first.
        let b = SvaExpr::Bus(vec![0, 1, 2, 3]);
        assert_eq!(eval(&b, &[true, false, true, false], &[]), (0b0101, 4));
        // (cnt < 4'd8): 7 < 8 true, 8 < 8 false.
        let lt = SvaExpr::Lt(
            Box::new(SvaExpr::Bus(vec![0, 1, 2, 3])),
            Box::new(SvaExpr::Const(8, 4)),
        );
        assert_eq!(eval(&lt, &[true, true, true, false], &[]), (1, 1));
        assert_eq!(eval(&lt, &[false, false, false, true], &[]), (0, 1));
        // Masking: 4-bit not of 0b0011 is 0b1100.
        let n = SvaExpr::Not(Box::new(SvaExpr::Bus(vec![0, 1, 2, 3])));
        assert_eq!(eval(&n, &[true, true, false, false], &[]), (0b1100, 4));
        // Concat is MSB-first: {2'b10, 2'b01} = 0b1001.
        let c = SvaExpr::Concat(vec![
            SvaExpr::Bus(vec![0, 1]),
            SvaExpr::Bus(vec![2, 3]),
        ]);
        assert_eq!(
            eval(&c, &[false, true, true, false], &[]),
            (0b1001, 4)
        );
        // Slice: bits [2:1] of 0b1101 = 0b10.
        let s = SvaExpr::Slice {
            arg: Box::new(SvaExpr::Bus(vec![0, 1, 2, 3])),
            lo: 1,
            width: 2,
        };
        assert_eq!(eval(&s, &[true, false, true, true], &[]), (0b10, 2));
    }

    const EN_CNT: &str = r#"
module top (
    input  logic clk,
    input  logic en,
    output logic [3:0] cnt_o
);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin
        if (en)
            cnt <= cnt + 1;
    end
    assign cnt_o = cnt;
    p1: assert property (@(posedge clk) en |-> ##1 $stable(cnt));
endmodule
"#;

    #[test]
    fn elaborate_counter_probes() {
        let out = synth_sv_with_sva(EN_CNT, "top.sv").expect("synth");
        assert_eq!(out.properties.len(), 1);
        let p = &out.properties[0];
        assert_eq!(p.label.as_deref(), Some("p1"));
        assert_eq!(p.delay, 1);
        assert_eq!(
            p.text,
            "assert property (@(posedge clk) en |-> ##1 $stable(cnt))"
        );
        // Probes: en (input) + cnt_0..cnt_3 (FF Q nets), interned once each.
        assert_eq!(p.probes.len(), 5);
        assert!(p.probes.contains(&SvaProbe::InputBit {
            name: "en".to_string()
        }));
        for i in 0..4 {
            assert!(
                p.probes.contains(&SvaProbe::FfBit {
                    net: format!("cnt_{i}")
                }),
                "{:?}",
                p.probes
            );
        }
        assert_eq!(p.slots, 1, "one $stable history slot");
    }

    #[test]
    fn elaborate_rejects_unresolvable_probe() {
        let src = r#"
module top (
    input  logic clk,
    input  logic en,
    output logic [3:0] cnt_o
);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin
        if (en)
            cnt <= cnt + 1;
    end
    assign cnt_o = cnt;
    assert property (@(posedge clk) en |-> ##1 $stable(nosuch));
endmodule
"#;
        // `nosuch` is not a register bit or an input port: fail loud,
        // naming the property and the signal.
        let e = synth_sv_with_sva(src, "top.sv").expect_err("must reject");
        assert!(e.contains("nosuch"), "{e}");
        assert!(e.contains("$stable(nosuch)"), "{e}");
    }

    #[test]
    fn elaborate_rejects_hierarchical_ref() {
        let src = r#"
module top (
    input  logic clk,
    input  logic en,
    output logic [3:0] cnt_o
);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin
        if (en)
            cnt <= cnt + 1;
    end
    assign cnt_o = cnt;
    assert property (@(posedge clk) top.cnt |-> ##1 $stable(cnt));
endmodule
"#;
        // Hierarchical references parse but are rejected at elaboration
        // with a named error (the kernel is single-module in v3.0).
        let e = synth_sv_with_sva(src, "top.sv").expect_err("must reject");
        assert!(e.contains("sva:"), "{e}");
        assert!(e.contains("top.cnt"), "{e}");
    }

    #[test]
    fn elaborate_inlines_comb_assign() {
        let src = r#"
module top (
    input  logic clk,
    input  logic en,
    output logic [3:0] cnt_o
);
    logic [3:0] cnt;
    logic [3:0] cnt_alias;
    always_ff @(posedge clk) begin
        if (en)
            cnt <= cnt + 1;
    end
    assign cnt_alias = cnt;
    assign cnt_o = cnt;
    assert property (@(posedge clk) en |-> ##1 $stable(cnt_alias));
endmodule
"#;
        // `cnt_alias` is a whole-signal single-driver comb alias of the
        // counter: it inlines to the FF Q probes, no new probe nets.
        let out = synth_sv_with_sva(src, "top.sv").expect("synth");
        assert_eq!(out.properties.len(), 1);
        let p = &out.properties[0];
        assert!(
            p.probes
                .iter()
                .all(|pr| !matches!(pr, SvaProbe::FfBit { net } if net.starts_with("cnt_alias"))),
            "{:?}",
            p.probes
        );
        assert_eq!(p.probes.len(), 5, "{:?}", p.probes);
    }
}


