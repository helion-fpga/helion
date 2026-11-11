//! Behavior-preserving FSMD (finite-state machine with datapath) RTL emitter.
//!
//! This is the RTL the C/RTL co-simulation runs: every C statement becomes
//! one or more FSM states over 32-bit datapath registers, so the generated
//! netlist computes what the C kernel computes. Loops a `PIPELINE` /
//! `UNROLL` directive flattens are unrolled; other loops stay rolled with a
//! counter register. `if`/`else`, early `return`, `?:`, `min`/`max`/`abs`
//! lower to state branches and muxes (if-conversion keeps both arms'
//! reads, so branches never drop work).
//!
//! Interfaces: `ap_ctrl_hs` / `ap_ctrl_none` block control, `ap_none` /
//! `ap_vld` scalars and pointers, and `ap_memory` arrays (one port per
//! partition bank, one-cycle read latency, registered address/enable). C
//! multiplies are bound round-robin to the `binding.dsp_count` shared
//! multiplier units; `/` and `%` use a shared divider.
//!
//! The emitted SystemVerilog sticks to the constructs `helion-sv` lowers
//! faithfully: a one-hot state register, one `r <= qa ? v1 : qb ? v2 : r`
//! update per register (no `case`, no shared if/else trees), sized
//! unsigned literals, three-address code (one operator per
//! state, operands are registers/ports/literals, so no wide expression
//! cones), compares latched into 1-bit registers (signed order via the
//! sign of a difference), no intermediate `assign`s except the divider's
//! arithmetic tree and each multiplier's `w = a * b` (registered into `p`).
//!
//! Kernels outside this subset (streams, AXI interfaces, dataflow, math
//! intrinsics) return `Err(reason)`; the caller then emits the interface
//! shell and co-simulation refuses to run it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

use crate::emit::InterfacePort;
use crate::parser::{BinOp, CExpr, CFunction, CStmt, LValue, UnaryOp};
use crate::pragma::{HlsDirective, InterfaceMode};
use crate::schedule::{ArrayBankConfig, BindingResult, Cdfg, eval_const_i64, subst_expr};

/// Datapath width of every internal register.
const W: usize = 32;
/// Largest array the FSMD keeps in registers / enumerates for dynamic
/// indexing.
const MAX_ARRAY_ELEMS: usize = 256;

pub struct FsmdRtl {
    pub sv: String,
    /// Worst-case cycles from start to done (for `ap_ctrl_none`, where the
    /// harness has no `ap_done` to wait for).
    pub cycle_bound: u64,
    pub states: usize,
    pub mul_units: usize,
}

#[derive(Clone, Copy, Debug)]
enum Hole {
    Goto(usize),
    Then(usize),
    Else(usize),
}

#[derive(Clone, Debug)]
enum Next {
    Goto(usize),
    Branch(String, usize, usize),
}

#[derive(Clone, Debug)]
struct State {
    actions: Vec<String>,
    next: Next,
}

#[derive(Clone, Debug)]
enum ArrStore {
    /// `ap_memory` argument: per-bank ports, bank mapping from `cfg`.
    Port { cfg: ArrayBankConfig, aw: usize },
    /// Local array in registers (`c_k_name_e{j}`); `rom` when `const`.
    Regs { regs: Vec<String>, rom: Option<Vec<i64>> },
}

#[derive(Clone, Debug)]
struct ArrInfo {
    dims: Vec<usize>,
    store: ArrStore,
}

const IDLE: usize = 0;
const DONE: usize = 1;

struct Em<'a> {
    func: &'a CFunction,
    directives: &'a [HlsDirective],
    states: Vec<State>,
    open: Vec<Hole>,
    regs: BTreeMap<String, usize>,
    /// Scalar C name -> SV read expression (port or register).
    scalars: HashMap<String, String>,
    /// Scalars stored in registers (assignable): C name -> register.
    scalar_regs: HashMap<String, String>,
    /// Output pointer ports: C name -> (port, width, has ap_vld).
    outs: HashMap<String, (String, usize, bool)>,
    arrays: HashMap<String, ArrInfo>,
    pulses: BTreeMap<String, Vec<String>>,
    temps: usize,
    units: usize,
    mul_next: usize,
    units_used: BTreeSet<usize>,
    div_used: bool,
    ret_port: Option<(String, usize)>,
    seq: usize,
}

fn lit(n: i64) -> String {
    if (0..=u32::MAX as i64).contains(&n) {
        format!("32'd{n}")
    } else {
        format!("32'h{:08X}", n as u32)
    }
}

fn clog2(n: usize) -> usize {
    let mut b = 0;
    while (1usize << b) < n {
        b += 1;
    }
    b
}

fn flat_coords(mut flat: usize, dims: &[usize]) -> Vec<usize> {
    let mut out = vec![0usize; dims.len()];
    for i in (0..dims.len()).rev() {
        let d = dims[i].max(1);
        out[i] = flat % d;
        flat /= d;
    }
    out
}

impl<'a> Em<'a> {
    fn reg(&mut self, base: &str, width: usize) -> String {
        self.seq += 1;
        let name = format!("c_{}_{}", self.seq, base);
        self.regs.insert(name.clone(), width);
        name
    }

    fn temp(&mut self) -> String {
        self.temps += 1;
        let name = format!("t{}x", self.temps);
        self.regs.insert(name.clone(), W);
        name
    }

    fn patch(&mut self, h: Hole, to: usize) {
        match h {
            Hole::Goto(s) => self.states[s].next = Next::Goto(to),
            Hole::Then(s) => {
                if let Next::Branch(_, t, _) = &mut self.states[s].next {
                    *t = to;
                }
            }
            Hole::Else(s) => {
                if let Next::Branch(_, _, e) = &mut self.states[s].next {
                    *e = to;
                }
            }
        }
    }

    /// Append a state that the currently open holes fall into.
    fn state(&mut self, actions: Vec<String>) -> usize {
        let id = self.states.len();
        self.states.push(State { actions, next: Next::Goto(DONE) });
        for h in std::mem::take(&mut self.open) {
            self.patch(h, id);
        }
        self.open.push(Hole::Goto(id));
        id
    }

    fn pulse(&mut self, sig: &str, state: usize, extra: Option<String>) {
        let c = match extra {
            Some(e) => format!("(q{state}x & {e})"),
            None => format!("q{state}x"),
        };
        self.pulses.entry(sig.to_string()).or_default().push(c);
    }

    // ------------------------------------------------------------------
    // Expressions: three-address code, one operator per state, so every
    // register's next-state function is a single operator over registers,
    // ports and sized literals (the forms helion-sv lowers exactly).
    // ------------------------------------------------------------------

    fn temp1(&mut self) -> String {
        self.temps += 1;
        let name = format!("b{}x", self.temps);
        self.regs.insert(name.clone(), 1);
        name
    }

    /// `t <= rhs;` in a new state; returns `t`.
    fn op(&mut self, rhs: String) -> String {
        let t = self.temp();
        self.state(vec![format!("{t} <= {rhs};")]);
        t
    }

    /// 1-bit `b <= rhs;` in a new state; returns `b`.
    fn op1(&mut self, rhs: String) -> String {
        let b = self.temp1();
        self.state(vec![format!("{b} <= {rhs};")]);
        b
    }

    /// A register/port name the expression can bit-select (latches
    /// literals and composite reads into a temporary).
    fn reg_atom(&mut self, a: &str) -> String {
        if is_ident(a) { a.to_string() } else { self.op(a.to_string()) }
    }

    /// SV atom (register, port or sized literal) holding the value of `e`.
    fn ex(&mut self, e: &CExpr) -> Result<(String, u64), String> {
        if let Some(v) = eval_const_i64(e) {
            return Ok((lit(v), 0));
        }
        Ok(match e {
            CExpr::Num(n) => (lit(*n as i64), 0),
            CExpr::Var(n) | CExpr::Deref(n) => {
                let r = self.read_scalar(n)?;
                if is_ident(&r) { (r, 0) } else { (self.op(r), 1) }
            }
            CExpr::Bit(n, idx) => {
                let x = self.read_scalar(n)?;
                let x = self.reg_atom(&x);
                let (i, c) = self.ex(idx)?;
                (self.op(format!("({x} >> {i}) & 32'd1")), c + 1)
            }
            CExpr::Slice(n, hi, lo) => {
                let x = self.read_scalar(n)?;
                let x = self.reg_atom(&x);
                let w = hi.saturating_sub(*lo) + 1;
                let mask = if w >= 32 { u32::MAX as i64 } else { (1i64 << w) - 1 };
                (self.op(format!("({x} >> {lo}) & {}", lit(mask))), 1)
            }
            CExpr::Index { array, indices } => {
                if !self.arrays.contains_key(array) {
                    // Bit select on a scalar (`cnt[2]`).
                    let x = self.read_scalar(array)?;
                    let x = self.reg_atom(&x);
                    let (i, c) = self.ex(indices.first().ok_or("empty index")?)?;
                    return Ok((self.op(format!("({x} >> {i}) & 32'd1")), c + 1));
                }
                self.array_read(array, indices)?
            }
            CExpr::Unary { op, expr } => {
                let (x, c) = self.ex(expr)?;
                match op {
                    UnaryOp::Neg => (self.op(format!("32'd0 - {x}")), c + 1),
                    UnaryOp::BitNot => (self.op(format!("~{x}")), c + 1),
                    UnaryOp::Not => {
                        let b = self.op1(format!("({x} == 32'd0)"));
                        (self.op(format!("{b} ? 32'd1 : 32'd0")), c + 2)
                    }
                }
            }
            CExpr::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs)?,
            CExpr::Ternary { cond, then_e, else_e } => {
                let (c, c0) = self.cond(cond)?;
                let (t, c1) = self.ex(then_e)?;
                let (f, c2) = self.ex(else_e)?;
                (self.op(format!("{c} ? {t} : {f}")), c0 + c1 + c2 + 1)
            }
            CExpr::Call { func, args } => {
                let name = func.trim_start_matches("hls::").trim_start_matches("std::");
                match (name, args.len()) {
                    ("min", 2) | ("max", 2) => {
                        let (a, c0) = self.ex(&args[0])?;
                        let (b, c1) = self.ex(&args[1])?;
                        let (lt, c2) = self.compare_atoms(BinOp::Lt, &a, &b);
                        let s = if name == "min" { format!("{lt} ? {a} : {b}") } else { format!("{lt} ? {b} : {a}") };
                        (self.op(s), c0 + c1 + c2 + 1)
                    }
                    ("abs", 1) => {
                        let (a, c) = self.ex(&args[0])?;
                        let a = self.reg_atom(&a);
                        (self.op(format!("{a}[31] ? (32'd0 - {a}) : {a}")), c + 2)
                    }
                    _ => return Err(format!("call to `{func}` (no RTL operator for this intrinsic)")),
                }
            }
            CExpr::StreamRead(_) | CExpr::StreamEmpty(_) | CExpr::StreamFull(_) => {
                return Err("hls::stream access".into());
            }
        })
    }

    /// Signed/equality compare of two atoms into a 1-bit register.
    fn compare_atoms(&mut self, op: BinOp, a: &str, b: &str) -> (String, u64) {
        match op {
            BinOp::Eq => (self.op1(format!("({a} == {b})")), 1),
            BinOp::Ne => (self.op1(format!("({a} != {b})")), 1),
            _ => {
                // Signed a < b from the operand signs and the sign of a - b
                // (3-bit cone; a wide `<` comparator is avoided on purpose).
                let (x, y) = match op {
                    BinOp::Lt | BinOp::Ge => (a, b),
                    _ => (b, a),
                };
                let x = self.reg_atom(x);
                let y = self.reg_atom(y);
                let d = self.op(format!("{x} - {y}"));
                let lt = self.op1(format!("({x}[31] & ~{y}[31]) | (~({x}[31] ^ {y}[31]) & {d}[31])"));
                match op {
                    // a >= b  ==  !(a < b);  a <= b  ==  !(b < a)
                    BinOp::Ge | BinOp::Le => (self.op1(format!("~{lt}")), 5),
                    _ => (lt, 4),
                }
            }
        }
    }

    /// 1-bit register (or combination of them) for condition `e`.
    fn cond(&mut self, e: &CExpr) -> Result<(String, u64), String> {
        match e {
            CExpr::Binary { op, lhs, rhs } if matches!(op, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) => {
                let (a, c0) = self.ex(lhs)?;
                let (b, c1) = self.ex(rhs)?;
                let (r, c2) = self.compare_atoms(*op, &a, &b);
                Ok((r, c0 + c1 + c2))
            }
            CExpr::Binary { op: op @ (BinOp::LogAnd | BinOp::LogOr), lhs, rhs } => {
                let (a, c0) = self.cond(lhs)?;
                let (b, c1) = self.cond(rhs)?;
                let sym = if *op == BinOp::LogAnd { "&" } else { "|" };
                Ok((self.op1(format!("{a} {sym} {b}")), c0 + c1 + 1))
            }
            CExpr::Unary { op: UnaryOp::Not, expr } => {
                let (c, k) = self.cond(expr)?;
                Ok((self.op1(format!("~{c}")), k + 1))
            }
            _ => {
                let (x, c) = self.ex(e)?;
                Ok((self.op1(format!("({x} != 32'd0)")), c + 1))
            }
        }
    }

    fn binary(&mut self, op: BinOp, l: &CExpr, r: &CExpr) -> Result<(String, u64), String> {
        if matches!(op, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::LogAnd | BinOp::LogOr) {
            let (c, k) = self.cond(&CExpr::Binary { op, lhs: Box::new(l.clone()), rhs: Box::new(r.clone()) })?;
            return Ok((self.op(format!("{c} ? 32'd1 : 32'd0")), k + 1));
        }
        if op == BinOp::Mul {
            return self.multiply(l, r);
        }
        if matches!(op, BinOp::Div | BinOp::Mod) {
            return self.divide(op, l, r);
        }
        let (a, c0) = self.ex(l)?;
        let (b, c1) = self.ex(r)?;
        let rhs = match op {
            BinOp::Add => format!("{a} + {b}"),
            BinOp::Sub => format!("{a} - {b}"),
            BinOp::And => format!("{a} & {b}"),
            BinOp::Or => format!("{a} | {b}"),
            BinOp::Xor => format!("{a} ^ {b}"),
            BinOp::Shl => format!("{a} << {b}"),
            BinOp::Shr => {
                // C `>>` on signed operands is arithmetic.
                let a = self.reg_atom(&a);
                format!("({a} >> {b}) | ({a}[31] ? (~(32'hFFFFFFFF >> {b})) : 32'd0)")
            }
            _ => unreachable!("handled above"),
        };
        Ok((self.op(rhs), c0 + c1 + 1))
    }

    /// Bind `l * r` to the next shared multiplier unit: load operands,
    /// wait for the registered product, copy it into a temporary.
    fn multiply(&mut self, l: &CExpr, r: &CExpr) -> Result<(String, u64), String> {
        let (a, c0) = self.ex(l)?;
        let (b, c1) = self.ex(r)?;
        let u = self.mul_next % self.units;
        self.mul_next += 1;
        self.units_used.insert(u);
        self.state(vec![format!("mula{u}x <= {a};"), format!("mulb{u}x <= {b};")]);
        self.state(Vec::new());
        let t = self.op(format!("mulp{u}x"));
        Ok((t, c0 + c1 + 3))
    }

    /// C `/` / `%` (truncating, sign of the dividend for `%`, `x/0 == 0`
    /// like the C reference) on the shared unsigned divider.
    fn divide(&mut self, op: BinOp, l: &CExpr, r: &CExpr) -> Result<(String, u64), String> {
        let (a, c0) = self.ex(l)?;
        let (b, c1) = self.ex(r)?;
        let a = self.reg_atom(&a);
        let b = self.reg_atom(&b);
        self.div_used = true;
        let neg = if op == BinOp::Div { format!("{a}[31] ^ {b}[31]") } else { format!("{a}[31]") };
        self.state(vec![
            format!("diva <= {a}[31] ? (32'd0 - {a}) : {a};"),
            format!("divb <= {b}[31] ? (32'd0 - {b}) : {b};"),
            format!("divn <= {neg};"),
        ]);
        let res = if op == BinOp::Div { "divq" } else { "divr" };
        let mag = self.op(res.to_string());
        let z = self.op1("(divb == 32'd0)".into());
        let signed = self.op(format!("divn ? (32'd0 - {mag}) : {mag}"));
        Ok((self.op(format!("{z} ? 32'd0 : {signed}")), c0 + c1 + 5))
    }

    fn read_scalar(&mut self, n: &str) -> Result<String, String> {
        if let Some(s) = self.scalars.get(n) {
            return Ok(s.clone());
        }
        if let Some((port, _, _)) = self.outs.get(n) {
            return Ok(port.clone());
        }
        Err(format!("unknown variable `{n}`"))
    }

    /// Flat element index: a constant, or an atom computed by ops.
    fn flat_index(&mut self, dims: &[usize], indices: &[CExpr]) -> Result<(Option<i64>, String, u64), String> {
        let mut konst: Option<i64> = Some(0);
        for (k, ie) in indices.iter().enumerate() {
            let d = dims.get(k).copied().unwrap_or(1).max(1) as i64;
            konst = match (konst, eval_const_i64(ie)) {
                (Some(a), Some(b)) => Some(a * d + b),
                _ => None,
            };
        }
        if let Some(k) = konst {
            return Ok((Some(k), lit(k), 0));
        }
        let mut cyc = 0;
        let mut acc = String::new();
        for (k, ie) in indices.iter().enumerate() {
            let d = dims.get(k).copied().unwrap_or(1).max(1) as i64;
            let (x, c) = self.ex(ie)?;
            cyc += c;
            acc = if k == 0 {
                x
            } else {
                // acc * d + x via shifts and adds.
                let mut terms = Vec::new();
                for bit in 0..32 {
                    if (d >> bit) & 1 == 1 {
                        terms.push(if bit == 0 { acc.clone() } else { self.op(format!("{acc} << {bit}")) });
                    }
                }
                let mut sum = terms.first().cloned().unwrap_or_else(|| "32'd0".into());
                for t in terms.iter().skip(1) {
                    sum = self.op(format!("{sum} + {t}"));
                }
                cyc += terms.len() as u64 + 1;
                self.op(format!("{sum} + {x}"))
            };
        }
        let acc = self.reg_atom(&acc);
        Ok((None, acc, cyc))
    }

    /// One-hot element selects for a dynamic index: `sel[j] == (fx == j)`.
    fn decode(&mut self, fx: &str, n: usize) -> Vec<String> {
        let sels: Vec<String> = (0..n).map(|_| self.temp1()).collect();
        let acts = sels.iter().enumerate().map(|(j, s)| format!("{s} <= ({fx} == {});", lit(j as i64))).collect();
        self.state(acts);
        sels
    }

    fn array_read(&mut self, array: &str, indices: &[CExpr]) -> Result<(String, u64), String> {
        let info = self.arrays.get(array).cloned().ok_or_else(|| format!("unknown array `{array}`"))?;
        let total: usize = info.dims.iter().product::<usize>().max(1);
        let (konst, fx, c0) = self.flat_index(&info.dims, indices)?;
        match &info.store {
            ArrStore::Regs { regs, rom } => {
                let elem = |j: usize| match rom {
                    Some(v) => lit(v.get(j).copied().unwrap_or(0)),
                    None => regs[j].clone(),
                };
                if let Some(k) = konst {
                    let k = k as usize;
                    return Ok((if k < total { elem(k) } else { "32'd0".into() }, c0));
                }
                let sels = self.decode(&fx, total);
                let terms: Vec<String> = (0..total).map(|j| format!("({} ? {} : 32'd0)", sels[j], elem(j))).collect();
                Ok((self.op(terms.join(" | ")), c0 + 2))
            }
            ArrStore::Port { cfg, aw } => {
                let banks = cfg.num_banks.max(1);
                let elems: Vec<(usize, usize)> =
                    (0..total).map(|e| cfg.map_indices(&flat_coords(e, &cfg.dims))).collect();
                let sels = if konst.is_none() && banks > 1 { self.decode(&fx, total) } else { Vec::new() };
                // Issue: registered address (+ ce pulse) per bank.
                let issue = self.state(Vec::new());
                let mut capture: Vec<String> = Vec::new();
                for b in 0..banks {
                    let port = if banks <= 1 { array.to_string() } else { format!("{array}_{b}") };
                    if let Some(k) = konst {
                        let Some(&(kb, ko)) = elems.get(k as usize) else { continue };
                        if kb != b {
                            continue;
                        }
                        self.states[issue].actions.push(format!("{port}_address0 <= {aw}'d{ko};"));
                        self.pulse(&format!("{port}_ce0"), issue, None);
                        capture.push(format!("{port}_q0"));
                    } else if banks <= 1 {
                        self.states[issue].actions.push(format!("{port}_address0 <= {fx}[{}:0];", aw - 1));
                        self.pulse(&format!("{port}_ce0"), issue, None);
                        capture.push(format!("{port}_q0"));
                    } else {
                        let mine: Vec<usize> = (0..total).filter(|&e| elems[e].0 == b).collect();
                        if mine.is_empty() {
                            continue;
                        }
                        let addr: Vec<String> =
                            mine.iter().map(|&e| format!("({} ? {aw}'d{} : {aw}'d0)", sels[e], elems[e].1)).collect();
                        self.states[issue].actions.push(format!("{port}_address0 <= {};", addr.join(" | ")));
                        let hit: Vec<String> = mine.iter().map(|&e| sels[e].clone()).collect();
                        self.pulse(&format!("{port}_ce0"), issue, Some(format!("({})", hit.join(" | "))));
                        for &e in &mine {
                            capture.push(format!("({} ? {port}_q0 : 32'd0)", sels[e]));
                        }
                    }
                }
                self.state(Vec::new());
                let t = self.op(if capture.is_empty() { "32'd0".into() } else { capture.join(" | ") });
                Ok((t, c0 + 3 + u64::from(!sels.is_empty())))
            }
        }
    }

    // ------------------------------------------------------------------
    // Statements
    // ------------------------------------------------------------------

    fn unroll_here(&self, label: &Option<String>, pragmas: &[HlsDirective], in_pipeline: bool) -> bool {
        if in_pipeline {
            return true;
        }
        let fname = &self.func.name;
        let target = label.as_ref().map(|l| format!("{fname}/{l}"));
        let directive_unrolls = |d: &HlsDirective| match d {
            HlsDirective::Unroll(u) => !u.off && u.factor.is_none(),
            _ => false,
        };
        pragmas.iter().any(directive_unrolls)
            || self
                .directives
                .iter()
                .any(|d| directive_unrolls(d) && target.is_some() && d.target() == target.as_deref())
    }

    fn loop_pipelined(&self, label: &Option<String>, pragmas: &[HlsDirective]) -> bool {
        let fname = &self.func.name;
        let target = label.as_ref().map(|l| format!("{fname}/{l}"));
        let pipe = |d: &HlsDirective| matches!(d, HlsDirective::Pipeline(p) if !p.off);
        pragmas.iter().any(pipe) || self.directives.iter().any(|d| pipe(d) && target.is_some() && d.target() == target.as_deref())
    }

    fn stmts(&mut self, body: &[CStmt], in_pipeline: bool) -> Result<u64, String> {
        let mut cyc = 0;
        for s in body {
            cyc += self.stmt(s, in_pipeline)?;
        }
        Ok(cyc)
    }

    fn branch(&mut self, c: String) -> usize {
        let id = self.state(Vec::new());
        self.states[id].next = Next::Branch(c, DONE, DONE);
        self.open = vec![Hole::Then(id)];
        id
    }

    fn stmt(&mut self, s: &CStmt, in_pipeline: bool) -> Result<u64, String> {
        match s {
            CStmt::Pragma(_) => Ok(0),
            CStmt::Decl(v) => {
                // Block-scoped declaration (`int sum = 0;` inside a loop):
                // one register for the whole kernel.
                if !v.dims.is_empty() {
                    return Err(format!("block-scoped array `{}`", v.name));
                }
                if v.is_static {
                    return Err(format!("block-scoped static `{}`", v.name));
                }
                if !self.scalar_regs.contains_key(&v.name) {
                    let r = self.reg(&v.name, W);
                    self.scalars.insert(v.name.clone(), r.clone());
                    self.scalar_regs.insert(v.name.clone(), r);
                }
                // The parser emits an explicit assignment for a non-zero
                // initializer; a zero / missing one re-zeroes here.
                if !matches!(&v.init, Some(e) if !matches!(e, CExpr::Num(0))) {
                    let r = self.scalar_regs[&v.name].clone();
                    self.state(vec![format!("{r} <= 32'd0;")]);
                    return Ok(1);
                }
                Ok(0)
            }
            CStmt::Assign { lhs, rhs } => self.assign(lhs, rhs),
            CStmt::Return(v) => {
                let mut cyc = 0;
                let mut acts = Vec::new();
                if let Some(e) = v {
                    let (x, c) = self.ex(e)?;
                    cyc += c;
                    if let Some((port, _)) = &self.ret_port {
                        acts.push(format!("{port} <= {x};"));
                    }
                }
                let id = self.state(acts);
                self.states[id].next = Next::Goto(DONE);
                self.open.clear();
                Ok(cyc + 1)
            }
            CStmt::If { cond, then_body, else_body } => {
                let (c, c0) = self.cond(cond)?;
                let id = self.branch(c);
                let ct = self.stmts(then_body, in_pipeline)?;
                let then_open = std::mem::take(&mut self.open);
                self.open = vec![Hole::Else(id)];
                let ce = self.stmts(else_body, in_pipeline)?;
                self.open.extend(then_open);
                Ok(c0 + 1 + ct.max(ce))
            }
            CStmt::For { label, var, start, end, step, inclusive, pragmas, body } => {
                let body_pipelined = in_pipeline
                    || self.loop_pipelined(label, pragmas)
                    || body.iter().any(|b| matches!(b, CStmt::Pragma(HlsDirective::Pipeline(p)) if !p.off));
                if self.unroll_here(label, pragmas, in_pipeline) {
                    let mut cyc = 0;
                    let iters = crate::schedule::loop_iterations(*start, *end, *step, *inclusive);
                    for iv in iters {
                        let mut map = HashMap::new();
                        map.insert(var.clone(), CExpr::Num(i128::from(iv)));
                        let sub: Vec<CStmt> = body.iter().map(|b| subst_stmt(b, &map)).collect();
                        cyc += self.stmts(&sub, body_pipelined)?;
                    }
                    return Ok(cyc);
                }
                if *step == 0 {
                    return Err("for loop with a zero step".into());
                }
                let r = self.reg(var, W);
                let prev = self.scalars.insert(var.clone(), r.clone());
                let prev_reg = self.scalar_regs.insert(var.clone(), r.clone());
                self.state(vec![format!("{r} <= {};", lit(*start))]);
                let head = self.states.len();
                let op = match (*step > 0, *inclusive) {
                    (true, false) => BinOp::Lt,
                    (true, true) => BinOp::Le,
                    (false, false) => BinOp::Gt,
                    (false, true) => BinOp::Ge,
                };
                let (c, cc) = self.compare_atoms(op, &r, &lit(*end));
                let br = self.branch(c);
                let cb = self.stmts(body, body_pipelined)?;
                // `r <= r + k` is a pattern helion-sv special-cases as a
                // counter (and mis-enables inside an FSM); go via a temp.
                let nxt = self.op(format!("{r} + {}", lit(*step)));
                let latch = self.state(vec![format!("{r} <= {nxt};")]);
                self.states[latch].next = Next::Goto(head);
                self.open = vec![Hole::Else(br)];
                match prev {
                    Some(p) => self.scalars.insert(var.clone(), p),
                    None => self.scalars.remove(var),
                };
                match prev_reg {
                    Some(p) => self.scalar_regs.insert(var.clone(), p),
                    None => self.scalar_regs.remove(var),
                };
                let trips = trip_count(*start, *end, *step, *inclusive);
                Ok(2 + cc + trips * (cb + cc + 3))
            }
            CStmt::While { cond, trip_bound, body, .. } => {
                let wc = self.reg("while_count", W);
                self.state(vec![format!("{wc} <= 32'd0;")]);
                let head = self.states.len();
                let (c, c0) = self.cond(cond)?;
                let (more, c1) = self.compare_atoms(BinOp::Lt, &wc, &lit(*trip_bound as i64));
                let go = self.op1(format!("{c} & {more}"));
                let br = self.branch(go);
                let cb = self.stmts(body, in_pipeline)?;
                let nxt = self.op(format!("{wc} + 32'd1"));
                let latch = self.state(vec![format!("{wc} <= {nxt};")]);
                self.states[latch].next = Next::Goto(head);
                self.open = vec![Hole::Else(br)];
                Ok(2 + (*trip_bound as u64 + 1) * (cb + c0 + c1 + 4))
            }
            CStmt::Call { func, .. } => Err(format!("call to `{func}` that was not inlined")),
            CStmt::StreamWrite { .. } => Err("hls::stream write".into()),
        }
    }

    fn assign(&mut self, lhs: &LValue, rhs: &CExpr) -> Result<u64, String> {
        let (v, c) = self.ex(rhs)?;
        match lhs {
            LValue::Var(n) => {
                let r = self.scalar_regs.get(n).cloned().ok_or_else(|| format!("assignment to `{n}` (not a local variable)"))?;
                self.state(vec![format!("{r} <= {v};")]);
            }
            LValue::Deref(n) => {
                if let Some((port, _, vld)) = self.outs.get(n).cloned() {
                    let id = self.state(vec![format!("{port} <= {v};")]);
                    if vld {
                        self.pulse(&format!("{port}_ap_vld"), id, None);
                    }
                } else if let Some(r) = self.scalar_regs.get(n).cloned() {
                    self.state(vec![format!("{r} <= {v};")]);
                } else {
                    return Err(format!("write through `{n}` (not an output pointer)"));
                }
            }
            LValue::Bit(n, b) => return self.bit_write(n, b, &v).map(|k| k + c),
            LValue::Slice(n, hi, lo) => {
                let r = self.scalar_regs.get(n).cloned().ok_or_else(|| format!("slice write to `{n}`"))?;
                let w = hi.saturating_sub(*lo) + 1;
                let mask: i64 = if w >= 32 { u32::MAX as i64 } else { ((1i64 << w) - 1) << lo };
                let ins = self.op(format!("({v} << {lo}) & {}", lit(mask)));
                let kept = self.op(format!("{r} & {}", lit(!mask & u32::MAX as i64)));
                self.state(vec![format!("{r} <= {kept} | {ins};")]);
                return Ok(c + 3);
            }
            LValue::ArrayElem { array, indices } => {
                if !self.arrays.contains_key(array) {
                    return self.bit_write(array, indices.first().ok_or("empty index")?, &v).map(|k| k + c);
                }
                return self.array_write(array, indices, &v).map(|k| k + c);
            }
        }
        Ok(c + 1)
    }

    fn bit_write(&mut self, n: &str, b: &CExpr, v: &str) -> Result<u64, String> {
        let r = self.scalar_regs.get(n).cloned().ok_or_else(|| format!("bit write to `{n}`"))?;
        let (bi, cb) = self.ex(b)?;
        let one = self.op(format!("32'd1 << {bi}"));
        let set = self.op(format!("{r} | {one}"));
        let clr = self.op(format!("{r} & ~{one}"));
        let v = self.reg_atom(v);
        self.state(vec![format!("{r} <= {v}[0] ? {set} : {clr};")]);
        Ok(cb + 5)
    }

    fn array_write(&mut self, array: &str, indices: &[CExpr], v: &str) -> Result<u64, String> {
        let info = self.arrays.get(array).cloned().ok_or_else(|| format!("unknown array `{array}`"))?;
        let total: usize = info.dims.iter().product::<usize>().max(1);
        let (konst, fx, c0) = self.flat_index(&info.dims, indices)?;
        match &info.store {
            ArrStore::Regs { regs, rom } => {
                if rom.is_some() {
                    return Err(format!("write to const array `{array}`"));
                }
                if let Some(k) = konst {
                    if (k as usize) < total {
                        self.state(vec![format!("{} <= {v};", regs[k as usize])]);
                    } else {
                        self.state(Vec::new());
                    }
                    return Ok(c0 + 1);
                }
                let sels = self.decode(&fx, total);
                let acts = regs.iter().enumerate().map(|(j, r)| format!("if ({}) {r} <= {v};", sels[j])).collect();
                self.state(acts);
                Ok(c0 + 2)
            }
            ArrStore::Port { cfg, aw } => {
                let banks = cfg.num_banks.max(1);
                let elems: Vec<(usize, usize)> =
                    (0..total).map(|e| cfg.map_indices(&flat_coords(e, &cfg.dims))).collect();
                let sels = if konst.is_none() && banks > 1 { self.decode(&fx, total) } else { Vec::new() };
                let id = self.state(Vec::new());
                for b in 0..banks {
                    let port = if banks <= 1 { array.to_string() } else { format!("{array}_{b}") };
                    let (addr, hit) = if let Some(k) = konst {
                        let Some(&(kb, ko)) = elems.get(k as usize) else { continue };
                        if kb != b {
                            continue;
                        }
                        (format!("{aw}'d{ko}"), None)
                    } else if banks <= 1 {
                        (format!("{fx}[{}:0]", aw - 1), None)
                    } else {
                        let mine: Vec<usize> = (0..total).filter(|&e| elems[e].0 == b).collect();
                        if mine.is_empty() {
                            continue;
                        }
                        let addr: Vec<String> =
                            mine.iter().map(|&e| format!("({} ? {aw}'d{} : {aw}'d0)", sels[e], elems[e].1)).collect();
                        let hit: Vec<String> = mine.iter().map(|&e| sels[e].clone()).collect();
                        (addr.join(" | "), Some(format!("({})", hit.join(" | "))))
                    };
                    self.states[id].actions.push(format!("{port}_address0 <= {addr};"));
                    self.states[id].actions.push(format!("{port}_d0 <= {v};"));
                    self.pulse(&format!("{port}_ce0"), id, hit.clone());
                    self.pulse(&format!("{port}_we0"), id, hit);
                }
                Ok(c0 + 1 + u64::from(!sels.is_empty()))
            }
        }
    }
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !s.starts_with(|c: char| c.is_ascii_digit())
}

fn assigned_vars(body: &[CStmt], out: &mut std::collections::HashSet<String>) {
    for s in body {
        match s {
            CStmt::Assign { lhs: LValue::Var(n) | LValue::Bit(n, _) | LValue::Slice(n, _, _), .. } => {
                out.insert(n.clone());
            }
            CStmt::If { then_body, else_body, .. } => {
                assigned_vars(then_body, out);
                assigned_vars(else_body, out);
            }
            CStmt::For { body, .. } | CStmt::While { body, .. } => assigned_vars(body, out),
            _ => {}
        }
    }
}

/// Split an action `[if (G) ]LHS <= RHS;` into (guard, lhs, rhs).
fn parse_act(a: &str) -> (Option<String>, String, String) {
    let a = a.trim().trim_end_matches(';');
    let (guard, rest) = match a.strip_prefix("if (") {
        Some(r) => {
            // Guards are register names or `(...)`: find the matching paren.
            let mut depth = 1usize;
            let mut end = 0;
            for (i, c) in r.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            (Some(r[..end].to_string()), r[end + 1..].trim())
        }
        None => (None, a),
    };
    let (lhs, rhs) = rest.split_once(" <= ").unwrap_or((rest, "0"));
    (guard, lhs.trim().to_string(), rhs.trim().to_string())
}

fn trip_count(start: i64, end: i64, step: i64, inclusive: bool) -> u64 {
    let span = if step > 0 { end - start } else { start - end };
    let s = step.abs();
    let n = if inclusive { span / s + 1 } else { (span + s - 1) / s };
    n.max(0) as u64
}

fn subst_stmt(st: &CStmt, map: &HashMap<String, CExpr>) -> CStmt {
    let lv = |l: &LValue| match l {
        LValue::Bit(n, e) => LValue::Bit(n.clone(), subst_expr(e, map)),
        LValue::ArrayElem { array, indices } => LValue::ArrayElem {
            array: array.clone(),
            indices: indices.iter().map(|i| subst_expr(i, map)).collect(),
        },
        other => other.clone(),
    };
    match st {
        CStmt::Assign { lhs, rhs } => CStmt::Assign { lhs: lv(lhs), rhs: subst_expr(rhs, map) },
        CStmt::Return(Some(e)) => CStmt::Return(Some(subst_expr(e, map))),
        CStmt::If { cond, then_body, else_body } => CStmt::If {
            cond: subst_expr(cond, map),
            then_body: then_body.iter().map(|b| subst_stmt(b, map)).collect(),
            else_body: else_body.iter().map(|b| subst_stmt(b, map)).collect(),
        },
        CStmt::For { label, var, start, end, step, inclusive, pragmas, body } => CStmt::For {
            label: label.clone(),
            var: var.clone(),
            start: *start,
            end: *end,
            step: *step,
            inclusive: *inclusive,
            pragmas: pragmas.clone(),
            body: body.iter().map(|b| subst_stmt(b, map)).collect(),
        },
        CStmt::While { label, cond, trip_bound, pragmas, body } => CStmt::While {
            label: label.clone(),
            cond: subst_expr(cond, map),
            trip_bound: *trip_bound,
            pragmas: pragmas.clone(),
            body: body.iter().map(|b| subst_stmt(b, map)).collect(),
        },
        CStmt::StreamWrite { stream, val } => CStmt::StreamWrite { stream: stream.clone(), val: subst_expr(val, map) },
        CStmt::Call { func, args } => CStmt::Call { func: func.clone(), args: args.iter().map(|a| subst_expr(a, map)).collect() },
        other => other.clone(),
    }
}

/// Emit the FSMD RTL for `func`, or `Err(reason)` when the kernel uses a
/// construct this emitter does not implement.
pub fn emit_fsmd_sv(
    func: &CFunction,
    cdfg: &Cdfg,
    binding: &BindingResult,
    interfaces: &[InterfacePort],
    directives: &[HlsDirective],
) -> Result<FsmdRtl, String> {
    if cdfg.is_dataflow {
        return Err("DATAFLOW region".into());
    }
    if func.args.iter().any(|a| a.width > W) || func.vars.iter().any(|v| v.width > W) || func.ret_width > W {
        return Err(format!("operands wider than {W} bits"));
    }
    let iface: HashMap<&str, &InterfacePort> = interfaces.iter().map(|i| (i.c_name.as_str(), i)).collect();
    let ret_mode = iface.get("return").map(|i| i.mode).unwrap_or(InterfaceMode::ApCtrlHs);
    let handshake = match ret_mode {
        InterfaceMode::ApCtrlHs | InterfaceMode::ApCtrlChain => true,
        InterfaceMode::ApCtrlNone => false,
        m => return Err(format!("block-level interface {m:?}")),
    };
    let mut em = Em {
        func,
        directives,
        states: vec![
            State { actions: Vec::new(), next: Next::Goto(DONE) },
            State { actions: Vec::new(), next: Next::Goto(IDLE) },
        ],
        open: Vec::new(),
        regs: BTreeMap::new(),
        scalars: HashMap::new(),
        scalar_regs: HashMap::new(),
        outs: HashMap::new(),
        arrays: HashMap::new(),
        pulses: BTreeMap::new(),
        temps: 0,
        units: binding.dsp_count.max(1),
        mul_next: 0,
        units_used: BTreeSet::new(),
        div_used: false,
        ret_port: None,
        seq: 0,
    };

    let mut ports: Vec<String> = vec!["input logic clk".into(), "input logic rst".into()];
    if handshake {
        for p in ["input logic ap_start", "output logic ap_done", "output logic ap_idle", "output logic ap_ready"] {
            ports.push(p.into());
        }
    }
    if func.ret_width > 0 {
        let w = func.ret_width.clamp(1, W);
        ports.push(format!("output logic [{}:0] ap_return", w - 1));
        em.ret_port = Some(("ap_return".into(), w));
    }
    let mut static_resets: Vec<String> = Vec::new();
    let mut start_inits: Vec<String> = Vec::new();
    let mut assigned = std::collections::HashSet::new();
    assigned_vars(&func.body, &mut assigned);
    for a in &func.args {
        if a.is_stream || a.is_axis_pkt {
            return Err(format!("stream argument `{}`", a.name));
        }
        let mode = iface.get(a.name.as_str()).map(|i| i.mode).unwrap_or(InterfaceMode::ApNone);
        let w = a.width.clamp(1, W);
        let decl = |dir: &str, name: &str, w: usize| {
            if w == 1 { format!("{dir} logic {name}") } else { format!("{dir} logic [{}:0] {name}", w - 1) }
        };
        if !a.dims.is_empty() {
            if mode != InterfaceMode::ApMemory {
                return Err(format!("array `{}` on a {mode:?} interface", a.name));
            }
            let cfg = cdfg.array_configs.get(&a.name).cloned().unwrap_or_else(|| {
                ArrayBankConfig::from_decl(&a.name, a.width, &a.dims, true, a.output, Vec::new(), &[])
            });
            let total: usize = a.dims.iter().product::<usize>().max(1);
            if total > MAX_ARRAY_ELEMS * 16 {
                return Err(format!("array `{}` with {total} elements", a.name));
            }
            let aw = clog2(cfg.bank_depth.max(1)).max(6);
            let banks = cfg.num_banks.max(1);
            for b in 0..banks {
                let p = if banks <= 1 { a.name.clone() } else { format!("{}_{b}", a.name) };
                ports.push(format!("output logic [{}:0] {p}_address0", aw - 1));
                ports.push(format!("output logic {p}_ce0"));
                if a.output {
                    ports.push(format!("output logic {p}_we0"));
                    ports.push(decl("output", &format!("{p}_d0"), w));
                    em.pulses.entry(format!("{p}_we0")).or_default();
                }
                ports.push(decl("input", &format!("{p}_q0"), w));
                em.pulses.entry(format!("{p}_ce0")).or_default();
            }
            em.arrays.insert(a.name.clone(), ArrInfo { dims: a.dims.clone(), store: ArrStore::Port { cfg, aw } });
            continue;
        }
        if a.is_ptr && a.output {
            if !matches!(mode, InterfaceMode::ApNone | InterfaceMode::ApVld | InterfaceMode::ApHs) {
                return Err(format!("pointer `{}` on a {mode:?} interface", a.name));
            }
            let vld = mode != InterfaceMode::ApNone;
            ports.push(decl("output", &a.name, w));
            if vld {
                ports.push(format!("output logic {}_ap_vld", a.name));
                em.pulses.entry(format!("{}_ap_vld", a.name)).or_default();
            }
            em.outs.insert(a.name.clone(), (a.name.clone(), w, vld));
            static_resets.push(format!("{} <= {};", a.name, if w == 1 { "1'b0".to_string() } else { format!("{w}'d0") }));
            continue;
        }
        if !matches!(mode, InterfaceMode::ApNone | InterfaceMode::ApVld | InterfaceMode::ApHs) {
            return Err(format!("argument `{}` on a {mode:?} interface", a.name));
        }
        ports.push(decl("input", &a.name, w));
        // Narrow signed inputs sign-extend into the 32-bit datapath.
        let read = if a.signed && w < W && w > 1 {
            let m = 1i64 << (w - 1);
            format!("(({} ^ {}) - {})", a.name, lit(m), lit(m))
        } else {
            a.name.clone()
        };
        if assigned.contains(&a.name) {
            // C may assign a by-value parameter: give it a register loaded
            // from the port when the kernel starts.
            let r = em.reg(&a.name, W);
            start_inits.push(format!("{r} <= {read};"));
            em.scalars.insert(a.name.clone(), r.clone());
            em.scalar_regs.insert(a.name.clone(), r);
        } else {
            em.scalars.insert(a.name.clone(), read);
        }
    }
    for v in &func.vars {
        if v.is_stream {
            return Err(format!("stream variable `{}`", v.name));
        }
        if v.dims.is_empty() {
            let r = em.reg(&v.name, W);
            em.scalars.insert(v.name.clone(), r.clone());
            em.scalar_regs.insert(v.name.clone(), r.clone());
            let init = v.init.as_ref().and_then(eval_const_i64).unwrap_or(0);
            if v.is_static {
                static_resets.push(format!("{r} <= {};", lit(init)));
            } else {
                start_inits.push(format!("{r} <= 32'd0;"));
            }
            continue;
        }
        let total: usize = v.dims.iter().product::<usize>().max(1);
        if total > MAX_ARRAY_ELEMS {
            return Err(format!("local array `{}` with {total} elements (register-file limit {MAX_ARRAY_ELEMS})", v.name));
        }
        let init: Vec<i64> = (0..total).map(|j| v.init_list.get(j).copied().unwrap_or(0) as i64).collect();
        if v.is_const {
            em.arrays.insert(v.name.clone(), ArrInfo { dims: v.dims.clone(), store: ArrStore::Regs { regs: Vec::new(), rom: Some(init) } });
            continue;
        }
        let base = em.reg(&v.name, W);
        em.regs.remove(&base);
        let regs: Vec<String> = (0..total)
            .map(|j| {
                let n = format!("{base}_e{j}x");
                em.regs.insert(n.clone(), W);
                n
            })
            .collect();
        for (j, r) in regs.iter().enumerate() {
            let line = format!("{r} <= {};", lit(init[j]));
            if v.is_static {
                static_resets.push(line);
            } else {
                start_inits.push(line);
            }
        }
        em.arrays.insert(v.name.clone(), ArrInfo { dims: v.dims.clone(), store: ArrStore::Regs { regs, rom: None } });
    }

    // Function-level PIPELINE flattens every loop (Vivado semantics).
    let fn_pipeline = func.pragmas.iter().any(|d| matches!(d, HlsDirective::Pipeline(p) if !p.off))
        || func.body.iter().any(|s| matches!(s, CStmt::Pragma(HlsDirective::Pipeline(p)) if !p.off))
        || directives.iter().any(|d| matches!(d, HlsDirective::Pipeline(p) if !p.off && d.target() == Some(func.name.as_str())));
    em.open = Vec::new();
    let first = em.states.len();
    let body_cycles = em.stmts(&func.body, fn_pipeline)?;
    // Fall-through end of the body: done.
    if em.states.len() == first {
        em.state(Vec::new());
    }
    for h in std::mem::take(&mut em.open) {
        em.patch(h, DONE);
    }
    if handshake {
        em.pulse("ap_done", DONE, None);
        em.pulse("ap_ready", DONE, None);
        em.pulse("ap_idle", IDLE, None);
    }

    // ------------------------------------------------------------------
    // Text
    // ------------------------------------------------------------------
    let mut sv = String::new();
    let _ = writeln!(
        sv,
        "// Generated by Helion HLS (FSMD datapath): {} states, {} multiplier unit(s){}",
        em.states.len(),
        em.units_used.len(),
        if em.div_used { ", 1 divider" } else { "" }
    );
    let _ = writeln!(sv, "module {}({});", func.name, ports.join(", "));
    // One-hot state register: one flip-flop per state, so every next-state
    // function is the OR of its few predecessors (a binary `st` compared in
    // every state made cones the synthesizer mis-lowered for large FSMs).
    for k in 0..em.states.len() {
        let _ = writeln!(sv, "  logic q{k}x;");
    }
    for (r, w) in &em.regs {
        if *w == 1 {
            let _ = writeln!(sv, "  logic {r};");
        } else {
            let _ = writeln!(sv, "  logic [{}:0] {r};", w - 1);
        }
    }
    for u in &em.units_used {
        let _ = writeln!(sv, "  logic [31:0] mula{u}x;\n  logic [31:0] mulb{u}x;\n  logic [31:0] mulp{u}x;");
    }
    if em.div_used {
        let _ = writeln!(sv, "  logic [31:0] diva;\n  logic [31:0] divb;\n  logic divn;\n  logic [31:0] divq;\n  logic [31:0] divr;");
        let _ = writeln!(sv, "  assign divq = diva / divb;\n  assign divr = diva % divb;");
    }
    // Each multiplier is a combinational DSP over two operand registers with
    // its own product register. (A clocked `p <= a * b` lets the DSP pass
    // absorb the enable-only operand/product registers as AREG/BREG/PREG,
    // which drops the FSM's load enables.)
    for u in &em.units_used {
        let _ = writeln!(sv, "  logic [31:0] mulw{u}x;\n  assign mulw{u}x = mula{u}x * mulb{u}x;\n  always_ff @(posedge clk) mulp{u}x <= mulw{u}x;");
    }
    for (sig, conds) in &em.pulses {
        let rhs = if conds.is_empty() { "1'b0".to_string() } else { conds.join(" | ") };
        let _ = writeln!(sv, "  always_ff @(posedge clk) {sig} <= (~rst) & ({rhs});");
    }
    let q = |k: usize| format!("q{k}x");
    // One register update per register: `r <= rst ? init : qa ? x : qb ? y :
    // r;` (a single linear mux chain per register).
    let first_body = if em.states.len() > first { first } else { DONE };
    let go = if handshake { "ap_start" } else { "1'b1" };
    let mut arms: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let start_guard = format!("({} & {go})", q(IDLE));
    for l in &start_inits {
        let (_, lhs, rhs) = parse_act(l);
        arms.entry(lhs).or_default().push((start_guard.clone(), rhs));
    }
    let mut preds: Vec<Vec<String>> = vec![Vec::new(); em.states.len()];
    preds[first_body].push(start_guard.clone());
    if handshake {
        preds[IDLE].push(format!("({} & ~ap_start)", q(IDLE)));
    }
    for (k, s) in em.states.iter().enumerate().skip(1) {
        let here = q(k);
        for a in &s.actions {
            let (guard, lhs, rhs) = parse_act(a);
            let g = match guard {
                Some(g) => format!("({here} & {g})"),
                None => here.clone(),
            };
            arms.entry(lhs).or_default().push((g, rhs));
        }
        match &s.next {
            Next::Goto(n) => preds[*n].push(here.clone()),
            Next::Branch(c, t, e) => {
                preds[*t].push(format!("({here} & ({c}))"));
                preds[*e].push(format!("({here} & ~({c}))"));
            }
        }
    }
    for (k, p) in preds.iter().enumerate() {
        let next = if p.is_empty() { "1'b0".to_string() } else { p.join(" | ") };
        let init = if k == IDLE { "1'b1" } else { "1'b0" };
        let _ = writeln!(sv, "  always_ff @(posedge clk) {} <= rst ? {init} : ({next});", q(k));
    }
    let resets: HashMap<String, String> = static_resets.iter().map(|l| {
        let (_, lhs, rhs) = parse_act(l);
        (lhs, rhs)
    }).collect();
    let chain = |reg: &str, arms: &[(String, String)]| -> String {
        let mut e = reg.to_string();
        for (g, v) in arms.iter().rev() {
            e = format!("{g} ? {v} : ({e})");
        }
        e
    };
    let mut emit_reg = |sv: &mut String, reg: &str, arms: &[(String, String)], reset: Option<&String>| {
        let body = chain(reg, arms);
        match reset {
            Some(r) => {
                let _ = writeln!(sv, "  always_ff @(posedge clk) {reg} <= rst ? {r} : ({body});");
            }
            None => {
                let _ = writeln!(sv, "  always_ff @(posedge clk) {reg} <= {body};");
            }
        }
    };
    for (reg, a) in &arms {
        emit_reg(&mut sv, reg, a, resets.get(reg));
    }
    // Statics with a reset value but never written by the FSM.
    for (reg, r) in &resets {
        if !arms.contains_key(reg) {
            emit_reg(&mut sv, reg, &[], Some(r));
        }
    }
    let _ = writeln!(sv, "endmodule");
    Ok(FsmdRtl {
        sv,
        cycle_bound: body_cycles + 4,
        states: em.states.len(),
        mul_units: em.units_used.len(),
    })
}
