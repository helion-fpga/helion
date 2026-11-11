//! RTL line / branch coverage instrumentation ("line probes").
//!
//! The DUT of a testbench run is simulated as a synthesized netlist, so the
//! simulator never executes RTL statements. To still report which RTL lines
//! and branch arms a run exercised, an instrumented synthesis
//! ([`crate::synth_sv_path_line_cov`]) adds one 1-bit *probe signal* per
//! procedural statement and per `if` / `case` arm:
//!
//! - a statement's probe is the conjunction of every enclosing branch
//!   condition (its *path guard*): it is 1 exactly when control would reach
//!   the statement if the block ran now;
//! - an `if` has a `true` arm probe (`guard && cond`) and a `false` arm probe
//!   (`guard && !cond`, also when there is no `else`);
//! - a `case` item arm is selected when its item matches and no earlier item
//!   did (`guard && match_i && !match_0 && ...`); an explicit `default` arm
//!   when no item matches;
//! - a continuous `assign` gets a copy of (up to 32 bits of) its target.
//!
//! The probes are ordinary continuous assigns into fresh `__lcov<uid>`
//! signals, lowered by the normal synthesis path from the real design
//! signals, so their values come from the same simulated netlist as the
//! DUT's outputs. Before LUT optimization each probe net is marked
//! `mark_debug` (kept by the dead-LUT sweep) and gets its metadata as net
//! attributes ([`helion_ir::LineProbe`]); its driving cell gets `src`.
//!
//! Not instrumented (reported via the design attribute `lcov_dropped`):
//! probes whose condition reads a name that is not a module port/signal
//! (function locals, loop variables that were not folded), conditions in an
//! `always_comb` that read a variable assigned in the same block (the probe
//! would see the block's final value, not the intermediate one), statements
//! in falling-edge-only blocks (the simulator steps rising edges), and the
//! arms of a `case` whose `default` is not the last item. Function / task
//! bodies are not instrumented.
//!
//! Instrumentation is opt-in per thread ([`Enable`]) and costs nothing when off.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};

use bumpalo::Bump;
use helion_ir::{intern, resolve, CellKind, Design, LineProbe, LineProbeKind, SrcLoc};

use crate::{RExpr, Tok};

/// Maps a line of the parsed (include-expanded) text back to a source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Seg {
    /// 0-based line in the expanded text where this segment starts.
    pub out_line: u32,
    pub file: String,
    /// 1-based line in `file` that `out_line` corresponds to.
    pub file_line: u32,
}

/// Map an expanded-text line (0-based) to `file:line`.
pub(crate) fn map_line(segs: &[Seg], out_line: u32) -> Option<SrcLoc> {
    let s = segs.iter().rev().find(|s| s.out_line <= out_line)?;
    Some(SrcLoc::new(s.file.clone(), s.file_line + (out_line - s.out_line)))
}

struct Source {
    src_base: usize,
    src_len: usize,
    tok_base: usize,
    tok_len: usize,
    tok_offsets: Vec<u32>,
    line_starts: Vec<usize>,
    segs: Vec<Seg>,
}

impl Source {
    fn out_line(&self, off: usize) -> u32 {
        (self.line_starts.partition_point(|&s| s <= off).saturating_sub(1)) as u32
    }
    fn line_text(&self, out_line: u32) -> Option<&'static str> {
        let start = *self.line_starts.get(out_line as usize)?;
        let end = self.line_starts.get(out_line as usize + 1).map(|e| e - 1).unwrap_or(self.src_len);
        // SAFETY: `src_base..src_base+src_len` is a leaked `&'static str`
        // registered by `register_source`; line starts sit after `\n` bytes,
        // so both ends are char boundaries.
        let all: &'static str = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.src_base as *const u8, self.src_len))
        };
        all.get(start..end.max(start))
    }
}

/// Metadata of one probe, keyed by uid until the netlist exists.
#[derive(Clone, Debug)]
struct Meta {
    src: SrcLoc,
    kind: LineProbeKind,
    clocked: bool,
}

/// A probe waiting for its procedural block to finish parsing.
struct Pending {
    uid: u64,
    guard: RExpr<'static>,
    width: usize,
    meta: Meta,
    dropped: Option<&'static str>,
}

#[derive(Default)]
struct ProcCtx {
    active: bool,
    clocked: bool,
    dead: u32,
    /// Path-guard stack (each entry a 1-bit condition).
    guards: Vec<RExpr<'static>>,
    probes: Vec<Pending>,
}

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static PENDING_SEGS: RefCell<Option<Vec<Seg>>> = const { RefCell::new(None) };
    static SOURCES: RefCell<Vec<Source>> = const { RefCell::new(Vec::new()) };
    static CTX: RefCell<ProcCtx> = RefCell::new(ProcCtx::default());
    static REGISTRY: RefCell<HashMap<u64, Meta>> = RefCell::new(HashMap::new());
    static DROPPED: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
    static ALWAYS: RefCell<HashMap<u64, Meta>> = RefCell::new(HashMap::new());
}

/// Process-wide so probe names never repeat (the own-logic cache keys on them).
static NEXT_UID: AtomicU64 = AtomicU64::new(1);

/// Name prefix of every probe signal.
pub(crate) const PROBE_PREFIX: &str = "__lcov";

/// Turns instrumentation on for this thread until dropped.
pub(crate) struct Enable {
    was: bool,
}

impl Enable {
    pub(crate) fn new() -> Self {
        let was = ENABLED.with(|e| e.replace(true));
        if !was {
            SOURCES.with(|s| s.borrow_mut().clear());
            REGISTRY.with(|r| r.borrow_mut().clear());
            DROPPED.with(|d| d.borrow_mut().clear());
            ALWAYS.with(|a| a.borrow_mut().clear());
            CTX.with(|c| *c.borrow_mut() = ProcCtx::default());
        }
        Self { was }
    }
}

impl Drop for Enable {
    fn drop(&mut self) {
        ENABLED.with(|e| e.set(self.was));
        if !self.was {
            SOURCES.with(|s| s.borrow_mut().clear());
            PENDING_SEGS.with(|p| *p.borrow_mut() = None);
            CTX.with(|c| *c.borrow_mut() = ProcCtx::default());
        }
    }
}

#[inline]
pub(crate) fn enabled() -> bool {
    ENABLED.with(Cell::get)
}

/// File segments for the next [`register_source`] (the text about to be parsed).
pub(crate) fn set_pending_segs(segs: Vec<Seg>) {
    if enabled() {
        PENDING_SEGS.with(|p| *p.borrow_mut() = Some(segs));
    }
}

/// The whole text comes from `file`, line for line.
pub(crate) fn single_file(file: &str) -> Vec<Seg> {
    vec![Seg { out_line: 0, file: file.to_string(), file_line: 1 }]
}

/// Remember a leaked source text and its token slice so token positions map
/// back to `file:line`. No-op unless enabled and the caller set the
/// segments for this text.
pub(crate) fn register_source(src: &'static str, toks: &'static [Tok<'static>], tok_offsets: Vec<u32>) {
    if !enabled() {
        return;
    }
    let Some(segs) = PENDING_SEGS.with(|p| p.borrow_mut().take()) else { return };
    let mut line_starts = vec![0usize];
    line_starts.extend(src.bytes().enumerate().filter(|(_, b)| *b == b'\n').map(|(i, _)| i + 1));
    SOURCES.with(|s| {
        s.borrow_mut().push(Source {
            src_base: src.as_ptr() as usize,
            src_len: src.len(),
            tok_base: toks.as_ptr() as usize,
            tok_len: toks.len(),
            tok_offsets,
            line_starts,
            segs,
        })
    });
}

fn span_of(t: &Tok) -> Option<usize> {
    match t {
        Tok::Ident(s) | Tok::Kw(s) | Tok::Str(s) => Some(s.as_ptr() as usize),
        _ => None,
    }
}

/// `(source, out_line)` of token `i` of `t`: exact via the registered token
/// offsets when `t` is (a sub-slice of) a registered token slice, else via
/// the nearest token that borrows the source text (copied token streams of
/// unrolled loops / generates).
fn locate(t: &[Tok], i: usize) -> Option<(usize, u32)> {
    if i >= t.len() {
        return None;
    }
    SOURCES.with(|s| {
        let s = s.borrow();
        let addr = &t[i] as *const Tok as usize;
        let sz = std::mem::size_of::<Tok>();
        for (k, src) in s.iter().enumerate() {
            if addr >= src.tok_base && addr < src.tok_base + src.tok_len * sz {
                let ix = (addr - src.tok_base) / sz;
                let off = *src.tok_offsets.get(ix)? as usize;
                return Some((k, src.out_line(off)));
            }
        }
        let near = (0..=i).rev().chain(i + 1..t.len()).find_map(|j| span_of(&t[j]))?;
        let (k, src) = s.iter().enumerate().find(|(_, src)| near >= src.src_base && near < src.src_base + src.src_len)?;
        Some((k, src.out_line(near - src.src_base)))
    })
}

/// Source location of token `i` (`None` when instrumentation is off or the
/// token is synthetic).
pub(crate) fn tok_loc(t: &[Tok], i: usize) -> Option<SrcLoc> {
    if !enabled() {
        return None;
    }
    let (k, line) = locate(t, i)?;
    SOURCES.with(|s| map_line(&s.borrow()[k].segs, line))
}

/// Case-item label: the source text of the item's line up to its `:`.
pub(crate) fn arm_label(t: &[Tok], i: usize) -> String {
    let text = locate(t, i).and_then(|(k, line)| SOURCES.with(|s| s.borrow()[k].line_text(line)));
    let Some(text) = text else { return "item".into() };
    let b = text.as_bytes();
    let mut end = text.len();
    let mut j = 0;
    while j < b.len() {
        if b[j] == b':' {
            if b.get(j + 1) == Some(&b':') {
                j += 2;
                continue;
            }
            end = j;
            break;
        }
        j += 1;
    }
    let label = text[..end].trim();
    let label: String = label.chars().take(48).collect();
    if label.is_empty() { "item".into() } else { label }
}

/// Start collecting probes for one procedural block.
pub(crate) fn begin_proc(clocked: bool) {
    if !enabled() {
        return;
    }
    CTX.with(|c| *c.borrow_mut() = ProcCtx { active: true, clocked, ..ProcCtx::default() });
}

fn active() -> bool {
    enabled() && CTX.with(|c| c.borrow().active)
}

/// A finished probe: its uid, signal width and RHS.
pub(crate) struct Probe<'a> {
    pub uid: u64,
    pub width: usize,
    pub rhs: RExpr<'a>,
}

impl Probe<'_> {
    pub(crate) fn signal_name<'b>(&self, arena: &'b Bump) -> &'b str {
        arena.alloc_str(&format!("{PROBE_PREFIX}{}", self.uid))
    }
}

/// Stop collecting; returns the block's probes. `keep` filters them (a
/// rejected probe is reported as not instrumented with the given reason).
pub(crate) fn end_proc<'a>(keep: impl Fn(&RExpr<'a>) -> Result<(), &'static str>) -> Vec<Probe<'a>> {
    if !enabled() {
        return Vec::new();
    }
    let probes = CTX.with(|c| std::mem::take(&mut *c.borrow_mut()).probes);
    finish(probes, keep)
}

/// Abandon the current block (parse error).
pub(crate) fn abort_proc() {
    if enabled() {
        CTX.with(|c| *c.borrow_mut() = ProcCtx::default());
    }
}

fn finish<'a>(probes: Vec<Pending>, keep: impl Fn(&RExpr<'a>) -> Result<(), &'static str>) -> Vec<Probe<'a>> {
    let mut out = Vec::new();
    for p in probes {
        // SAFETY: `guard` was stored by `stash` from an `RExpr<'a>` of the
        // same parse (same arena / source, which outlive this call); RExpr
        // is covariant, so this only restores the original lifetime.
        let rhs: RExpr<'a> = unsafe { std::mem::transmute::<RExpr<'static>, RExpr<'a>>(p.guard) };
        let verdict = p.dropped.map(Err).unwrap_or_else(|| keep(&rhs));
        match verdict {
            Ok(()) if matches!(rhs, RExpr::Const { val: 1, width: 1, care: 1 }) => {
                // Unconditional: no net needed (see `LineProbe::write_always`).
                ALWAYS.with(|a| a.borrow_mut().insert(p.uid, p.meta));
            }
            Ok(()) => {
                REGISTRY.with(|r| r.borrow_mut().insert(p.uid, p.meta));
                out.push(Probe { uid: p.uid, width: p.width, rhs });
            }
            Err(why) => {
                DROPPED.with(|d| d.borrow_mut().insert(format!("{}={why}", p.meta.src)));
            }
        }
    }
    out
}

fn stash<'a>(e: RExpr<'a>) -> RExpr<'static> {
    // SAFETY: see `finish`; the value is restored to `'a` before the parse
    // that owns its arena / source ends (`end_proc` runs in the frame that
    // called `begin_proc`, and `begin_proc` / `abort_proc` discard leftovers).
    unsafe { std::mem::transmute::<RExpr<'a>, RExpr<'static>>(e) }
}

const fn one() -> RExpr<'static> {
    RExpr::Const { val: 1, width: 1, care: 1 }
}

/// 1-bit truth value of a condition (`|cond`).
pub(crate) fn truth<'a>(cond: &RExpr<'a>, arena: &'a Bump) -> RExpr<'a> {
    RExpr::RedOr(arena.alloc(cond.clone()))
}

pub(crate) fn not<'a>(b: RExpr<'a>, arena: &'a Bump) -> RExpr<'a> {
    RExpr::Not(arena.alloc(b))
}

pub(crate) fn and<'a>(a: RExpr<'a>, b: RExpr<'a>, arena: &'a Bump) -> RExpr<'a> {
    if matches!(a, RExpr::Const { val: 1, width: 1, care: 1 }) {
        return b;
    }
    RExpr::And(arena.alloc(a), arena.alloc(b))
}

pub(crate) fn or<'a>(a: RExpr<'a>, b: RExpr<'a>, arena: &'a Bump) -> RExpr<'a> {
    RExpr::Or(arena.alloc(a), arena.alloc(b))
}

fn path_guard<'a>(ctx: &ProcCtx, arena: &'a Bump) -> RExpr<'a> {
    let mut g: RExpr<'a> = one();
    for e in &ctx.guards {
        // Restore `'a` (see `finish`).
        let e: RExpr<'a> = unsafe { std::mem::transmute::<RExpr<'static>, RExpr<'a>>(e.clone()) };
        g = and(g, e, arena);
    }
    g
}

/// Pops the path guard pushed by [`GuardScope::push`] when dropped (also on
/// an early `?` return from a branch parse).
pub(crate) struct GuardScope {
    kind: u8,
}

impl GuardScope {
    /// Enter a branch whose statements run only when `cond` (1-bit) holds.
    pub(crate) fn push<'a>(cond: RExpr<'a>) -> Self {
        if !active() {
            return Self { kind: 0 };
        }
        CTX.with(|c| c.borrow_mut().guards.push(stash(cond)));
        Self { kind: 1 }
    }

    /// No guard change (instrumentation off, or an always-taken branch).
    pub(crate) fn none() -> Self {
        Self { kind: 0 }
    }

    /// Scope for the then (`then = true`) or else arm of an `if` whose
    /// condition is elaboration-`known` or has the 1-bit truth value `cond`.
    pub(crate) fn branch<'a>(known: Option<bool>, cond: Option<&RExpr<'a>>, then: bool, arena: &'a Bump) -> Self {
        match (known, cond) {
            (Some(k), _) if k == then => Self::none(),
            (Some(_), _) => Self::dead(),
            (None, Some(c)) => Self::push(if then { c.clone() } else { not(c.clone(), arena) }),
            (None, None) => Self::none(),
        }
    }

    /// Enter a branch that elaboration removed (constant condition): its
    /// statements are not hardware and get no probes.
    pub(crate) fn dead() -> Self {
        if !active() {
            return Self { kind: 0 };
        }
        CTX.with(|c| c.borrow_mut().dead += 1);
        Self { kind: 2 }
    }
}

impl Drop for GuardScope {
    fn drop(&mut self) {
        match self.kind {
            1 => CTX.with(|c| {
                c.borrow_mut().guards.pop();
            }),
            2 => CTX.with(|c| {
                let mut c = c.borrow_mut();
                c.dead = c.dead.saturating_sub(1);
            }),
            _ => {}
        }
    }
}

fn push_probe<'a>(loc: Option<SrcLoc>, kind: LineProbeKind, extra: Option<RExpr<'a>>, arena: &'a Bump) {
    let Some(src) = loc else { return };
    CTX.with(|c| {
        let mut c = c.borrow_mut();
        if !c.active || c.dead > 0 {
            return;
        }
        let mut g = path_guard(&c, arena);
        if let Some(e) = extra {
            g = and(g, e, arena);
        }
        let uid = NEXT_UID.fetch_add(1, Ordering::Relaxed);
        let clocked = c.clocked;
        c.probes.push(Pending { uid, guard: stash(g), width: 1, meta: Meta { src, kind, clocked }, dropped: None });
    });
}

/// A procedural statement at `loc` (reached under the current path guard).
pub(crate) fn stmt<'a>(loc: Option<SrcLoc>, arena: &'a Bump) {
    if active() {
        push_probe(loc, LineProbeKind::Stmt, None, arena);
    }
}

/// A fresh branch-site id.
pub(crate) fn new_branch() -> u64 {
    NEXT_UID.fetch_add(1, Ordering::Relaxed)
}

/// Where an arm sits: its branch site id, construct, position, label and
/// the line of the `if` / `case` keyword.
pub(crate) struct ArmAt<'l> {
    pub branch: u64,
    pub construct: &'static str,
    pub index: u32,
    pub label: &'l str,
    pub site: Option<&'l SrcLoc>,
}

/// One arm at `loc`, selected when `sel` (1-bit) holds under the current
/// path guard.
pub(crate) fn arm<'a>(loc: Option<SrcLoc>, at: ArmAt<'_>, sel: RExpr<'a>, arena: &'a Bump) {
    if active() {
        let Some(loc) = loc else { return };
        let site = at.site.map(|s| s.line).unwrap_or(loc.line);
        let kind = LineProbeKind::Arm {
            branch: at.branch,
            construct: at.construct.into(),
            index: at.index,
            label: at.label.into(),
            site,
        };
        push_probe(Some(loc), kind, Some(sel), arena);
    }
}

/// Number of probes recorded so far in this block (see [`drop_range`]).
pub(crate) fn mark() -> usize {
    if !active() {
        return 0;
    }
    CTX.with(|c| c.borrow().probes.len())
}

/// Mark probes `from..to` (from [`mark`]) as not instrumented.
pub(crate) fn drop_range(from: usize, to: usize, why: &'static str) {
    if !active() {
        return;
    }
    CTX.with(|c| {
        for p in c.borrow_mut().probes.iter_mut().take(to).skip(from) {
            p.dropped = Some(why);
        }
    });
}

/// Probe for a continuous `assign` at `loc`: a copy of `target` (`width` bits).
pub(crate) fn assign_probe<'a>(loc: Option<SrcLoc>, target: RExpr<'a>, width: usize, keep: impl Fn(&RExpr<'a>) -> Result<(), &'static str>) -> Option<Probe<'a>> {
    if !enabled() {
        return None;
    }
    let src = loc?;
    let uid = NEXT_UID.fetch_add(1, Ordering::Relaxed);
    let p = Pending {
        uid,
        guard: stash(target),
        width: width.clamp(1, 32),
        meta: Meta { src, kind: LineProbeKind::Assign, clocked: false },
        dropped: None,
    };
    finish(vec![p], keep).pop()
}

/// Every identifier an expression reads.
pub(crate) fn idents<'a>(e: &RExpr<'a>, out: &mut Vec<&'a str>) {
    let mut stack: Vec<&RExpr<'a>> = vec![e];
    while let Some(e) = stack.pop() {
        match e {
            RExpr::Const { .. } => {}
            RExpr::Ident(n) | RExpr::Bit(n, _) | RExpr::Range(n, _, _) => out.push(n),
            RExpr::IndexPart { name, base, .. } => {
                out.push(name);
                stack.push(base);
            }
            RExpr::WordAt { addr, data } => {
                stack.push(addr);
                stack.push(data);
            }
            RExpr::Concat(v) => stack.extend(v.iter()),
            RExpr::Call { args, .. } => stack.extend(args.iter()),
            RExpr::RedXor(a) | RExpr::RedAnd(a) | RExpr::RedOr(a) | RExpr::Not(a) => stack.push(a),
            RExpr::Shr(a, b)
            | RExpr::Ashr(a, b)
            | RExpr::Shl(a, b)
            | RExpr::And(a, b)
            | RExpr::Or(a, b)
            | RExpr::Xor(a, b)
            | RExpr::Add(a, b)
            | RExpr::Sub(a, b)
            | RExpr::Mul(a, b)
            | RExpr::Eq(a, b)
            | RExpr::Ne(a, b)
            | RExpr::Lt(a, b) => {
                stack.push(a);
                stack.push(b);
            }
            RExpr::Mux(c, a, b) => {
                stack.push(c);
                stack.push(a);
                stack.push(b);
            }
        }
    }
}

/// Split `name` = `[prefix]__lcov<uid>[_<bit>]` into the uid.
fn probe_uid(name: &str) -> Option<u64> {
    let at = name.rfind(PROBE_PREFIX)?;
    let rest = &name[at + PROBE_PREFIX.len()..];
    let digits = rest.split('_').next()?;
    let tail = &rest[digits.len()..];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !(tail.is_empty() || (tail.len() > 1 && tail[1..].bytes().all(|b| b.is_ascii_digit()))) {
        return None;
    }
    digits.parse().ok()
}

/// Attach probe metadata to the synthesized netlist (before LUT
/// optimization): each probe net gets `mark_debug` + [`LineProbe`]
/// attributes, its driving cell gets `src`. Probes that did not survive
/// lowering, and the ones rejected while parsing, are listed in the design
/// attribute `lcov_dropped` (`file:line=reason` joined by `|`).
pub(crate) fn mark_probe_nets(d: &mut Design) {
    if !enabled() {
        return;
    }
    let reg = REGISTRY.with(|r| r.borrow().clone());
    let mut seen: BTreeSet<u64> = BTreeSet::new();
    let mut driver_src: Vec<(helion_ir::Spur, SrcLoc)> = Vec::new();
    let out_pins = [intern("O"), intern("Q")];
    for n in d.nets.iter_mut() {
        let Some(uid) = probe_uid(resolve(n.name)) else { continue };
        let Some(m) = reg.get(&uid) else { continue };
        seen.insert(uid);
        let probe = LineProbe { src: m.src.clone(), kind: m.kind.clone(), clocked: m.clocked };
        probe.write(&mut n.attrs);
        n.attrs.set("mark_debug", "1");
        if let Some(e) = n.endpoints.iter().find(|e| out_pins.contains(&e.pin)) {
            driver_src.push((e.cell, m.src.clone()));
        }
    }
    for (cell, loc) in driver_src {
        if let Some(c) = d.cell_mut(resolve(cell)) {
            if matches!(c.kind, CellKind::Lut6 { .. } | CellKind::Hff) {
                c.attrs.add_src(&loc);
            }
        }
    }
    let mut dropped = DROPPED.with(|d| d.borrow().clone());
    for (uid, m) in &reg {
        if !seen.contains(uid) {
            dropped.insert(format!("{}=not lowered", m.src));
        }
    }
    if !dropped.is_empty() {
        let v: Vec<String> = dropped.into_iter().collect();
        d.attrs.set("lcov_dropped", v.join("|"));
    }
    for (uid, m) in ALWAYS.with(|a| a.borrow().clone()) {
        let probe = LineProbe { src: m.src, kind: m.kind, clocked: m.clocked };
        probe.write_always(&mut d.attrs, uid);
    }
    d.attrs.set("lcov", "1");
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const DUT: &str = "\
module lc_dut (
    input logic clk,
    input logic rst,
    input logic en,
    input logic [1:0] mode,
    output logic [3:0] q,
    output logic flag
);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin
        if (rst)
            cnt <= 4'd0;
        else if (en)
            cnt <= cnt + 4'd1;
    end
    /* a block comment
       spanning lines */
    always_comb begin
        case (mode)
            2'd0: q = cnt;
            2'd1: q = ~cnt;
            default: q = 4'd0;
        endcase
    end
    assign flag = cnt[3];
endmodule
";

    fn write_dut(tag: &str, text: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("helion-sv-linecov-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("lc_dut.sv");
        std::fs::write(&p, text).unwrap();
        p
    }

    fn probes(d: &Design) -> Vec<LineProbe> {
        let mut v: Vec<LineProbe> = d.nets.iter().filter_map(|n| LineProbe::read(&n.attrs)).collect();
        v.sort_by(|a, b| (a.src.line, format!("{:?}", a.kind)).cmp(&(b.src.line, format!("{:?}", b.kind))));
        v
    }

    #[test]
    fn instrumented_synth_tags_statements_branches_and_assigns() {
        let path = write_dut("tags", DUT);
        let file = path.display().to_string();
        let d = crate::synth_sv_path_line_cov(&path).expect("synth");
        let mut ps = probes(&d);
        ps.extend(LineProbe::always_on(&d.attrs));
        assert!(ps.iter().all(|p| p.src.file == file), "{ps:#?}");
        let stmt_lines: BTreeSet<u32> =
            ps.iter().filter(|p| p.kind == LineProbeKind::Stmt).map(|p| p.src.line).collect();
        assert_eq!(stmt_lines, [11, 12, 13, 14, 19, 20, 21, 22].into_iter().collect(), "{ps:#?}");
        let arms: Vec<(u32, String, u32, bool)> = ps
            .iter()
            .filter_map(|p| match &p.kind {
                LineProbeKind::Arm { construct, label, index, .. } => {
                    Some((p.src.line, format!("{construct}:{label}"), *index, p.clocked))
                }
                _ => None,
            })
            .collect();
        for want in [
            (11, "if:true", 0, true),
            (11, "if:false", 1, true),
            (13, "if:true", 0, true),
            (13, "if:false", 1, true),
            (20, "case:2'd0", 0, false),
            (21, "case:2'd1", 1, false),
            (22, "case:default", 2, false),
        ] {
            assert!(arms.contains(&(want.0, want.1.to_string(), want.2, want.3)), "missing {want:?} in {arms:?}");
        }
        assert!(ps.iter().any(|p| p.kind == LineProbeKind::Assign && p.src.line == 25), "{ps:#?}");
        assert!(d.attrs.get("lcov_dropped").is_none(), "{:?}", d.attrs.get("lcov_dropped"));
        // Probe drivers carry `src` as cell attributes.
        assert!(d.cells.iter().any(|c| !c.attrs.src_locs().is_empty()));
        // Plain synthesis is untouched: no probes, no attrs.
        let plain = crate::synth_sv_path(&path).expect("plain");
        assert!(plain.nets.iter().all(|n| !LineProbe::is_probe(&n.attrs)));
        assert!(plain.nets.iter().all(|n| !resolve(n.name).contains(PROBE_PREFIX)));
        assert!(plain.cells.len() < d.cells.len());
    }

    #[test]
    fn child_module_probes_keep_their_lines_under_hierarchy() {
        let text = "\
module lc_leaf (input logic clk, input logic a, output logic y);
    logic r;
    always_ff @(posedge clk)
        if (a) r <= 1'b1;
        else r <= 1'b0;
    assign y = r;
endmodule
module lc_top (input logic clk, input logic a, output logic y);
    lc_leaf u_l (.clk(clk), .a(a), .y(y));
endmodule
";
        let path = write_dut("hier", text);
        let d = crate::synth_sv_path_line_cov(&path).expect("synth");
        let arms: Vec<(String, u32)> = d
            .nets
            .iter()
            .filter_map(|n| LineProbe::read(&n.attrs).map(|p| (resolve(n.name).to_string(), p)))
            .filter(|(_, p)| matches!(p.kind, LineProbeKind::Arm { .. }))
            .map(|(n, p)| (n, p.src.line))
            .collect();
        assert!(arms.iter().any(|(n, l)| n.starts_with("u_l_") && *l == 4), "{arms:?}");
        let stmts: BTreeSet<u32> = d
            .nets
            .iter()
            .filter_map(|n| LineProbe::read(&n.attrs))
            .filter(|p| p.kind == LineProbeKind::Stmt)
            .map(|p| p.src.line)
            .collect();
        assert_eq!(stmts, [4, 5].into_iter().collect());
    }

    #[test]
    fn unsound_probes_are_reported_not_instrumented() {
        let text = "\
module lc_drop (input logic clk, input logic a, output logic y, output logic z);
    logic t;
    always_comb begin
        t = a;
        if (t) y = 1'b1;
        else y = 1'b0;
    end
    always_ff @(negedge clk)
        if (a) z <= 1'b1;
endmodule
";
        let path = write_dut("drop", text);
        let d = crate::synth_sv_path_line_cov(&path).expect("synth");
        let dropped = d.attrs.get("lcov_dropped").unwrap_or_default().to_string();
        assert!(dropped.contains(":5=condition reads a value assigned in the same always_comb"), "{dropped}");
        assert!(dropped.contains(":9=falling-edge block"), "{dropped}");
        // Line 4 (`t = a;`) is unconditional, so it is still instrumented.
        assert!(LineProbe::always_on(&d.attrs).iter().any(|p| p.src.line == 4));
    }

    #[test]
    fn probe_uid_parses_prefixed_and_bit_names() {
        assert_eq!(probe_uid("__lcov12"), Some(12));
        assert_eq!(probe_uid("__lcov12_3"), Some(12));
        assert_eq!(probe_uid("u_a___lcov7"), Some(7));
        assert_eq!(probe_uid("u_a___lcov7_0"), Some(7));
        assert_eq!(probe_uid("cnt_3"), None);
        assert_eq!(probe_uid("__lcov"), None);
        assert_eq!(probe_uid("__lcov5x"), None);
    }

    #[test]
    fn segments_map_lines_through_includes() {
        let segs = vec![
            Seg { out_line: 0, file: "top.sv".into(), file_line: 1 },
            Seg { out_line: 3, file: "inc.svh".into(), file_line: 1 },
            Seg { out_line: 6, file: "top.sv".into(), file_line: 4 },
        ];
        assert_eq!(map_line(&segs, 0), Some(SrcLoc::new("top.sv", 1)));
        assert_eq!(map_line(&segs, 4), Some(SrcLoc::new("inc.svh", 2)));
        assert_eq!(map_line(&segs, 8), Some(SrcLoc::new("top.sv", 6)));
    }
}
