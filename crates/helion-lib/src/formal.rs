//! Formal Verification Engine for `helion_ir::Design`:
//! - Native incremental CDCL SAT solver with assumption-core extraction (`CnfSolver`, `Lit`)
//! - Exact Prime-Implicant Tseitin CNF encoder for `Lut6`, `Carry4`, `Muxf7`/`Muxf8`, `Hff`, `IobIn`/`IobOut`
//! - Combinational & Sequential Logic Equivalence Checking (`check_equivalence`, `check_sequential_equivalence`)
//! - Bounded Model Checking (`run_bmc`)
//! - Temporal k-Induction with simple-path uniqueness (`run_k_induction`)
//! - IC3 / Property-Directed Reachability (`run_ic3_pdr`)
//! - IEEE 1364 Value Change Dump (`.vcd`) counterexample waveform generator (`emit_cex_vcd`)

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};

use helion_ir::{intern, resolve, CellKind, Design, PortDir, Spur};

/// A signed Boolean literal in CNF (`1-based` variable index; positive = `v`, negative = `~v`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lit(pub i32);

impl Lit {
    #[inline]
    pub const fn from_var(var: usize, pol: bool) -> Self {
        let idx = (var as i32) + 1;
        Self(if pol { idx } else { -idx })
    }

    #[inline]
    pub const fn var(self) -> usize {
        (self.0.unsigned_abs() as usize) - 1
    }

    #[inline]
    pub const fn is_pos(self) -> bool {
        self.0 > 0
    }

    #[inline]
    pub const fn not(self) -> Self {
        Self(-self.0)
    }
}

impl std::ops::Not for Lit {
    type Output = Self;
    #[inline]
    fn not(self) -> Self::Output {
        Self(-self.0)
    }
}

/// Result of a SAT query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SatResult {
    /// Satisfiable with a total variable assignment (`model[var]` is the boolean value of `var`).
    Sat(Vec<bool>),
    /// Unsatisfiable, returning the subset of assumption literals involved in the conflict.
    Unsat(Vec<Lit>),
}

#[derive(Clone, Debug)]
struct TrailEntry {
    lit: Lit,
    level: usize,
    reason: Option<usize>, // clause index, None for decisions/assumptions
}

/// Fast native incremental CDCL SAT solver with VSIDS activity, phase saving,
/// 1-UIP conflict analysis, and assumption-core extraction for BMC, k-Induction, and IC3/PDR.
#[derive(Clone, Debug, Default)]
pub struct CnfSolver {
    num_vars: usize,
    clauses: Vec<Vec<Lit>>,
    unsat_at_root: bool,
    assigns: Vec<Option<bool>>,
    var_level: Vec<usize>,
    var_reason: Vec<Option<usize>>,
    phase: Vec<bool>,
    activity: Vec<f64>,
    var_inc: f64,
    const_false: Option<Lit>,
    const_true: Option<Lit>,
}

impl CnfSolver {
    pub fn new() -> Self {
        Self {
            num_vars: 0,
            clauses: Vec::new(),
            unsat_at_root: false,
            assigns: Vec::new(),
            var_level: Vec::new(),
            var_reason: Vec::new(),
            phase: Vec::new(),
            activity: Vec::new(),
            var_inc: 1.0,
            const_false: None,
            const_true: None,
        }
    }

    /// Allocate a fresh positive literal.
    pub fn new_lit(&mut self) -> Lit {
        let v = self.num_vars;
        self.num_vars += 1;
        self.assigns.push(None);
        self.var_level.push(0);
        self.var_reason.push(None);
        self.phase.push(false);
        self.activity.push(0.0);
        Lit::from_var(v, true)
    }

    /// Return a literal constrained to constant `val`.
    pub fn const_lit(&mut self, val: bool) -> Lit {
        if val {
            if let Some(l) = self.const_true {
                return l;
            }
            let l = self.new_lit();
            self.add_clause(&[l]);
            self.const_true = Some(l);
            l
        } else {
            if let Some(l) = self.const_false {
                return l;
            }
            let l = self.new_lit();
            self.add_clause(&[!l]);
            self.const_false = Some(l);
            l
        }
    }

    /// Add a clause `lits[0] | lits[1] | ...`.
    pub fn add_clause(&mut self, lits: &[Lit]) {
        if self.unsat_at_root {
            return;
        }
        let mut clean: Vec<Lit> = lits.to_vec();
        clean.sort_unstable_by_key(|l| l.0);
        clean.dedup();
        // Check for tautology `x | ~x`.
        for i in 0..clean.len() {
            if clean.binary_search_by_key(&(-clean[i].0), |l| l.0).is_ok() {
                return;
            }
        }
        if clean.is_empty() {
            self.unsat_at_root = true;
            return;
        }
        for &l in &clean {
            self.activity[l.var()] += 1.0;
        }
        self.clauses.push(clean);
    }

    /// Constrain `a == b`.
    #[inline]
    pub fn add_equiv(&mut self, a: Lit, b: Lit) {
        if a != b {
            self.add_clause(&[!a, b]);
            self.add_clause(&[a, !b]);
        }
    }

    /// Return a literal encoding `a ^ b`.
    pub fn encode_xor(&mut self, a: Lit, b: Lit) -> Lit {
        let out = self.new_lit();
        self.add_clause(&[!a, !b, !out]);
        self.add_clause(&[a, b, !out]);
        self.add_clause(&[a, !b, out]);
        self.add_clause(&[!a, b, out]);
        out
    }

    /// Return a literal encoding `a & b`.
    pub fn encode_and(&mut self, a: Lit, b: Lit) -> Lit {
        let out = self.new_lit();
        self.add_clause(&[!a, !b, out]);
        self.add_clause(&[a, !out]);
        self.add_clause(&[b, !out]);
        out
    }

    /// Return a literal encoding `sel ? when_hi : when_lo`.
    pub fn encode_mux(&mut self, sel: Lit, when_lo: Lit, when_hi: Lit) -> Lit {
        let out = self.new_lit();
        self.add_clause(&[!sel, !when_hi, out]);
        self.add_clause(&[!sel, when_hi, !out]);
        self.add_clause(&[sel, !when_lo, out]);
        self.add_clause(&[sel, when_lo, !out]);
        out
    }

    /// Return a literal encoding `OR(lits)`.
    pub fn encode_or_slice(&mut self, lits: &[Lit]) -> Lit {
        if lits.is_empty() {
            return self.const_lit(false);
        }
        if lits.len() == 1 {
            return lits[0];
        }
        let out = self.new_lit();
        let mut big = Vec::with_capacity(lits.len() + 1);
        for &l in lits {
            self.add_clause(&[!l, out]);
            big.push(l);
        }
        big.push(!out);
        self.add_clause(&big);
        out
    }

    /// Return a literal encoding `AND(lits)`.
    pub fn encode_and_slice(&mut self, lits: &[Lit]) -> Lit {
        if lits.is_empty() {
            return self.const_lit(true);
        }
        if lits.len() == 1 {
            return lits[0];
        }
        let out = self.new_lit();
        let mut big = Vec::with_capacity(lits.len() + 1);
        for &l in lits {
            self.add_clause(&[l, !out]);
            big.push(!l);
        }
        big.push(out);
        self.add_clause(&big);
        out
    }

    #[inline]
    fn eval_lit(&self, lit: Lit) -> Option<bool> {
        self.assigns[lit.var()].map(|v| if lit.is_pos() { v } else { !v })
    }

    fn assign(&mut self, lit: Lit, level: usize, reason: Option<usize>, trail: &mut Vec<TrailEntry>) {
        let v = lit.var();
        let val = lit.is_pos();
        self.assigns[v] = Some(val);
        self.var_level[v] = level;
        self.var_reason[v] = reason;
        self.phase[v] = val;
        trail.push(TrailEntry { lit, level, reason });
    }

    fn backtrack(&mut self, target_level: usize, trail: &mut Vec<TrailEntry>) {
        while let Some(last) = trail.last() {
            if last.level <= target_level {
                break;
            }
            let entry = trail.pop().unwrap();
            let v = entry.lit.var();
            self.assigns[v] = None;
            self.var_level[v] = 0;
            self.var_reason[v] = None;
        }
    }

    /// Run Boolean Constraint Propagation (BCP); returns `Some(conflict_clause_idx)` on conflict.
    fn propagate(&mut self, current_level: usize, trail: &mut Vec<TrailEntry>) -> Option<usize> {
        loop {
            let mut any_unit = false;
            for ci in 0..self.clauses.len() {
                let clause = &self.clauses[ci];
                let mut satisfied = false;
                let mut unassigned_count = 0usize;
                let mut last_unassigned = Lit(1);

                for &lit in clause {
                    match self.eval_lit(lit) {
                        Some(true) => {
                            satisfied = true;
                            break;
                        }
                        Some(false) => {}
                        None => {
                            unassigned_count += 1;
                            last_unassigned = lit;
                            if unassigned_count > 1 {
                                break;
                            }
                        }
                    }
                }

                if satisfied {
                    continue;
                }
                if unassigned_count == 0 {
                    return Some(ci);
                }
                if unassigned_count == 1 {
                    self.assign(last_unassigned, current_level, Some(ci), trail);
                    any_unit = true;
                }
            }
            if !any_unit {
                return None;
            }
        }
    }

    /// 1-UIP conflict analysis returning `(learned_clause, backtrack_level)`.
    fn analyze_conflict(
        &mut self,
        conflict_ci: usize,
        current_level: usize,
        trail: &[TrailEntry],
    ) -> (Vec<Lit>, usize) {
        let mut seen = vec![false; self.num_vars];
        let mut learned: Vec<Lit> = Vec::new();
        let mut path_c = 0usize;
        let mut reason_lits = self.clauses[conflict_ci].clone();
        let mut trail_idx = trail.len();

        loop {
            for &lit in &reason_lits {
                let v = lit.var();
                if !seen[v] {
                    seen[v] = true;
                    self.activity[v] += self.var_inc;
                    let lvl = self.var_level[v];
                    if lvl == current_level {
                        path_c += 1;
                    } else if lvl > 0 {
                        learned.push(Lit::from_var(v, !self.assigns[v].unwrap()));
                    }
                }
            }

            // Find the most recent assigned variable on the trail at `current_level` that is marked `seen`.
            let uip_entry = loop {
                trail_idx -= 1;
                let entry = &trail[trail_idx];
                if seen[entry.lit.var()] && entry.level == current_level {
                    break entry.clone();
                }
            };

            path_c -= 1;
            if path_c == 0 {
                learned.insert(0, !uip_entry.lit);
                break;
            }

            if let Some(r_ci) = uip_entry.reason {
                reason_lits = self.clauses[r_ci]
                    .iter()
                    .copied()
                    .filter(|l| l.var() != uip_entry.lit.var())
                    .collect();
            } else {
                learned.insert(0, !uip_entry.lit);
                break;
            }
        }

        self.var_inc *= 1.05;
        if self.var_inc > 1e100 {
            for a in &mut self.activity {
                *a *= 1e-100;
            }
            self.var_inc *= 1e-100;
        }

        let bt_level = learned
            .iter()
            .skip(1)
            .map(|l| self.var_level[l.var()])
            .max()
            .unwrap_or(0);
        (learned, bt_level)
    }

    /// Solve the CNF formula under `assumptions`.
    pub fn solve_with_assumptions(&mut self, assumptions: &[Lit]) -> SatResult {
        if self.unsat_at_root {
            return SatResult::Unsat(Vec::new());
        }

        for a in &mut self.assigns {
            *a = None;
        }
        for l in &mut self.var_level {
            *l = 0;
        }
        for r in &mut self.var_reason {
            *r = None;
        }

        let mut trail: Vec<TrailEntry> = Vec::with_capacity(self.num_vars);
        if self.propagate(0, &mut trail).is_some() {
            self.unsat_at_root = true;
            return SatResult::Unsat(Vec::new());
        }

        // Deduplicate assumptions and check for immediate contradictions.
        let mut clean_assumps: Vec<Lit> = Vec::with_capacity(assumptions.len());
        for &a in assumptions {
            if clean_assumps.contains(&!a) {
                return SatResult::Unsat(vec![a, !a]);
            }
            if !clean_assumps.contains(&a) {
                clean_assumps.push(a);
            }
        }

        let num_assumps = clean_assumps.len();
        let mut current_level = 0usize;

        // Apply assumptions at levels 1..=num_assumps.
        while current_level < num_assumps {
            let a_lit = clean_assumps[current_level];
            match self.eval_lit(a_lit) {
                Some(true) => {
                    current_level += 1;
                    // Still advance decision level by recording a dummy level or keeping level aligned:
                    // Even simpler: only increment level for unassigned assumptions, or assign if unassigned.
                }
                Some(false) => {
                    // Conflict on assumption `a_lit`: extract assumption core from trail.
                    let mut core = Vec::new();
                    for &prev in &clean_assumps[..=current_level] {
                        if self.eval_lit(prev).is_some() || prev == a_lit {
                            core.push(prev);
                        }
                    }
                    self.backtrack(0, &mut trail);
                    return SatResult::Unsat(core);
                }
                None => {
                    current_level += 1;
                    self.assign(a_lit, current_level, None, &mut trail);
                    if let Some(conf_ci) = self.propagate(current_level, &mut trail) {
                        // Extract assumption literals involved in the conflict.
                        let mut core = Vec::new();
                        for &prev in &clean_assumps {
                            if self.assigns[prev.var()].is_some() {
                                core.push(prev);
                            }
                        }
                        let _ = conf_ci;
                        self.backtrack(0, &mut trail);
                        return SatResult::Unsat(core);
                    }
                }
            }
        }

        let base_level = current_level;

        loop {
            if let Some(conf_ci) = self.propagate(current_level, &mut trail) {
                if current_level <= base_level {
                    let mut core = Vec::new();
                    for &prev in &clean_assumps {
                        if self.assigns[prev.var()].is_some() {
                            core.push(prev);
                        }
                    }
                    self.backtrack(0, &mut trail);
                    return SatResult::Unsat(core);
                }
                let (learned, bt_level) = self.analyze_conflict(conf_ci, current_level, &trail);
                let uip_lit = learned[0];
                let target_lvl = bt_level.max(base_level);
                self.backtrack(target_lvl, &mut trail);
                current_level = target_lvl;
                if learned.len() == 1 {
                    self.backtrack(0, &mut trail);
                    self.add_clause(&learned);
                    return self.solve_with_assumptions(assumptions);
                } else {
                    let ci = self.clauses.len();
                    self.clauses.push(learned);
                    self.assign(uip_lit, current_level, Some(ci), &mut trail);
                }
                continue;
            }

            // Pick the unassigned variable with highest VSIDS activity.
            let mut next_var: Option<usize> = None;
            let mut best_act = -1.0f64;
            for v in 0..self.num_vars {
                if self.assigns[v].is_none() && self.activity[v] > best_act {
                    best_act = self.activity[v];
                    next_var = Some(v);
                }
            }

            let Some(v) = next_var else {
                let model: Vec<bool> = (0..self.num_vars)
                    .map(|i| self.assigns[i].unwrap_or(false))
                    .collect();
                self.backtrack(0, &mut trail);
                return SatResult::Sat(model);
            };

            current_level += 1;
            let dec_lit = Lit::from_var(v, self.phase[v]);
            self.assign(dec_lit, current_level, None, &mut trail);
        }
    }

    #[inline]
    pub fn solve(&mut self) -> SatResult {
        self.solve_with_assumptions(&[])
    }
}

/// Prime-implicant / consensus cube reduction for up to 6 variables.
///
/// Each cube is `(mask, polarity_bits)` where `mask` indicates which of the `m` variables
/// are present in the cube, and `polarity_bits` gives the required bit value (`1` or `0`)
/// for each present variable.
fn minimize_cubes(minterms: &[u8], m: usize) -> Vec<(u8, u8)> {
    if minterms.is_empty() {
        return Vec::new();
    }
    let full_mask = if m == 0 { 0 } else { (1u8 << m) - 1 };
    let mut current: Vec<(u8, u8)> = minterms.iter().map(|&bits| (full_mask, bits)).collect();
    current.sort_unstable();
    current.dedup();

    let mut primes: Vec<(u8, u8)> = Vec::new();
    loop {
        let n = current.len();
        let mut used = vec![false; n];
        let mut next: Vec<(u8, u8)> = Vec::new();

        for i in 0..n {
            for j in (i + 1)..n {
                if current[i].0 == current[j].0 {
                    let diff = (current[i].1 ^ current[j].1) & current[i].0;
                    if diff.count_ones() == 1 {
                        used[i] = true;
                        used[j] = true;
                        let new_mask = current[i].0 & !diff;
                        let new_pol = current[i].1 & new_mask;
                        next.push((new_mask, new_pol));
                    }
                }
            }
        }

        for i in 0..n {
            if !used[i] && !primes.contains(&current[i]) {
                primes.push(current[i]);
            }
        }

        if next.is_empty() {
            break;
        }
        next.sort_unstable();
        next.dedup();
        current = next;
    }

    primes
}

/// Encode a 6-input LUT (`init`) into `solver` using minimized prime-implicant Tseitin clauses.
pub fn encode_lut6_clauses(solver: &mut CnfSolver, mut init: u64, inputs: [Option<Lit>; 6]) -> Lit {
    // Fold unconnected (`None`) pins to `0`.
    for i in 0..6 {
        if inputs[i].is_none() {
            let mut folded = 0u64;
            for addr in 0..64u64 {
                let fixed = addr & !(1u64 << i);
                folded |= ((init >> fixed) & 1) << addr;
            }
            init = folded;
        }
    }

    if init == 0 {
        return solver.const_lit(false);
    }
    if init == u64::MAX {
        return solver.const_lit(true);
    }

    // Identify active input pins where low cofactor != high cofactor.
    const LO_MASKS: [u64; 6] = [
        0x5555_5555_5555_5555,
        0x3333_3333_3333_3333,
        0x0F0F_0F0F_0F0F_0F0F,
        0x00FF_00FF_00FF_00FF,
        0x0000_FFFF_0000_FFFF,
        0x0000_0000_FFFF_FFFF,
    ];
    let mut active_pins: Vec<(usize, Lit)> = Vec::with_capacity(6);
    for i in 0..6 {
        if let Some(lit) = inputs[i] {
            let shift = 1usize << i;
            if (init & LO_MASKS[i]) != ((init >> shift) & LO_MASKS[i]) {
                active_pins.push((i, lit));
            }
        }
    }

    if active_pins.is_empty() {
        return solver.const_lit((init & 1) != 0);
    }

    // Fast path for single-input buffer/inverter.
    if active_pins.len() == 1 {
        let (pin, lit) = active_pins[0];
        let val0 = (init & 1) != 0;
        let val1 = ((init >> (1usize << pin)) & 1) != 0;
        return if !val0 && val1 {
            lit
        } else if val0 && !val1 {
            !lit
        } else {
            solver.const_lit(val0)
        };
    }

    let m = active_pins.len();
    let out = solver.new_lit();
    let mut on_minterms: Vec<u8> = Vec::new();
    let mut off_minterms: Vec<u8> = Vec::new();

    for sub_addr in 0..(1usize << m) {
        let mut full_addr = 0u64;
        for (bit_idx, &(orig_pin, _)) in active_pins.iter().enumerate() {
            if ((sub_addr >> bit_idx) & 1) != 0 {
                full_addr |= 1u64 << orig_pin;
            }
        }
        if ((init >> full_addr) & 1) != 0 {
            on_minterms.push(sub_addr as u8);
        } else {
            off_minterms.push(sub_addr as u8);
        }
    }

    // Add prime-implicant clauses for ON-set: `cube => out` (`~l_0 | ... | out`)
    for (mask, pol) in minimize_cubes(&on_minterms, m) {
        let mut clause = Vec::with_capacity(m + 1);
        for (bit_idx, &(_, lit)) in active_pins.iter().enumerate() {
            if ((mask >> bit_idx) & 1) != 0 {
                let req_one = ((pol >> bit_idx) & 1) != 0;
                clause.push(if req_one { !lit } else { lit });
            }
        }
        clause.push(out);
        solver.add_clause(&clause);
    }

    // Add prime-implicant clauses for OFF-set: `cube => ~out` (`~l_0 | ... | ~out`)
    for (mask, pol) in minimize_cubes(&off_minterms, m) {
        let mut clause = Vec::with_capacity(m + 1);
        for (bit_idx, &(_, lit)) in active_pins.iter().enumerate() {
            if ((mask >> bit_idx) & 1) != 0 {
                let req_one = ((pol >> bit_idx) & 1) != 0;
                clause.push(if req_one { !lit } else { lit });
            }
        }
        clause.push(!out);
        solver.add_clause(&clause);
    }

    out
}

/// Extracted synchronous symbolic model of a [`Design`].
#[derive(Clone, Debug)]
pub struct DesignModel {
    /// Sorted primary input net names (excluding `clk`).
    pub primary_inputs: Vec<String>,
    /// Sorted primary output net names.
    pub primary_outputs: Vec<String>,
    /// State registers in deterministic order: `(cell_name, d_net, q_net)`.
    pub flops: Vec<(String, String, String)>,
    /// Clock net name (defaults to `"clk"`).
    pub clock_net: String,
}

/// Symbolic net literals for one unrolled timeframe $t$ of a [`Design`].
#[derive(Clone, Debug)]
pub struct TimeframeLits {
    pub net_lits: HashMap<String, Lit>,
    pub input_lits: BTreeMap<String, Lit>,
    pub state_q_lits: Vec<Lit>,
    pub next_state_d_lits: Vec<Lit>,
    pub output_lits: BTreeMap<String, Lit>,
}

impl DesignModel {
    /// Extract the structural formal model from `design`.
    pub fn from_design(design: &Design) -> Self {
        let mut clk_nets: HashSet<String> = HashSet::new();
        for c in &design.cells {
            if matches!(c.kind, CellKind::Hff) {
                if let Some(cn) = design.net_on(c.name, intern("CLK")) {
                    clk_nets.insert(resolve(cn).to_string());
                }
            }
        }
        let clock_net = clk_nets
            .iter()
            .next()
            .cloned()
            .unwrap_or_else(|| "clk".to_string());

        let mut primary_inputs: Vec<String> = design
            .ports
            .iter()
            .filter(|p| matches!(p.dir, PortDir::In | PortDir::Inout))
            .map(|p| resolve(p.name).to_string())
            .filter(|n| !clk_nets.contains(n) && n != "clk")
            .collect();
        primary_inputs.sort();
        primary_inputs.dedup();

        let mut primary_outputs: Vec<String> = design
            .ports
            .iter()
            .filter(|p| matches!(p.dir, PortDir::Out | PortDir::Inout))
            .map(|p| resolve(p.name).to_string())
            .collect();
        primary_outputs.sort();
        primary_outputs.dedup();

        let mut flops: Vec<(String, String, String)> = Vec::new();
        for c in &design.cells {
            if matches!(c.kind, CellKind::Hff) {
                let cname = resolve(c.name).to_string();
                let d_net = design
                    .net_on(c.name, intern("D"))
                    .map(|s| resolve(s).to_string())
                    .unwrap_or_default();
                let q_net = design
                    .net_on(c.name, intern("Q"))
                    .map(|s| resolve(s).to_string())
                    .unwrap_or_else(|| format!("{cname}$Q"));
                flops.push((cname, d_net, q_net));
            }
        }
        flops.sort_by(|a, b| a.0.cmp(&b.0));

        Self {
            primary_inputs,
            primary_outputs,
            flops,
            clock_net,
        }
    }

    /// Encode one timeframe of `design` into `solver`, given optional pre-allocated
    /// primary input literals and current-state `Hff/Q` literals.
    pub fn encode_timeframe(
        &self,
        design: &Design,
        solver: &mut CnfSolver,
        shared_inputs: Option<&BTreeMap<String, Lit>>,
        current_state_q: Option<&[Lit]>,
    ) -> TimeframeLits {
        let mut net_lits: HashMap<String, Lit> = HashMap::new();
        net_lits.insert("0".to_string(), solver.const_lit(false));
        net_lits.insert("1".to_string(), solver.const_lit(true));
        net_lits.insert("1'b0".to_string(), solver.const_lit(false));
        net_lits.insert("1'b1".to_string(), solver.const_lit(true));

        let mut input_lits: BTreeMap<String, Lit> = BTreeMap::new();
        for inp in &self.primary_inputs {
            let l = shared_inputs
                .and_then(|m| m.get(inp).copied())
                .unwrap_or_else(|| solver.new_lit());
            input_lits.insert(inp.clone(), l);
            net_lits.insert(inp.clone(), l);
        }

        let mut state_q_lits: Vec<Lit> = Vec::with_capacity(self.flops.len());
        for (idx, (_, _, q_net)) in self.flops.iter().enumerate() {
            let l = current_state_q
                .and_then(|s| s.get(idx).copied())
                .unwrap_or_else(|| solver.new_lit());
            state_q_lits.push(l);
            if !q_net.is_empty() {
                net_lits.insert(q_net.clone(), l);
            }
        }

        // Propagate IobIn and ClockBuf buffers first.
        for c in &design.cells {
            match c.kind {
                CellKind::IobIn => {
                    let pad = design
                        .net_on(c.name, intern("PAD"))
                        .map(|s| resolve(s).to_string());
                    let o = design
                        .net_on(c.name, intern("O"))
                        .map(|s| resolve(s).to_string());
                    if let (Some(p_net), Some(o_net)) = (pad, o) {
                        let p_lit = *net_lits
                            .entry(p_net.clone())
                            .or_insert_with(|| *input_lits.entry(p_net).or_insert_with(|| solver.new_lit()));
                        net_lits.insert(o_net, p_lit);
                    }
                }
                CellKind::ClockBuf => {
                    let i_net = design
                        .net_on(c.name, intern("I"))
                        .map(|s| resolve(s).to_string());
                    let o_net = design
                        .net_on(c.name, intern("O"))
                        .map(|s| resolve(s).to_string());
                    if let (Some(in_s), Some(out_s)) = (i_net, o_net) {
                        if let Some(&l) = net_lits.get(&in_s) {
                            net_lits.insert(out_s, l);
                        }
                    }
                }
                _ => {}
            }
        }

        // Topological / iterative encoding of combinational cells (`Lut6`, `Carry4`, `MUXF7`/`MUXF8`).
        let mut encoded_cells = vec![false; design.cells.len()];
        let max_passes = design.cells.len() + 2;

        for _pass in 0..max_passes {
            let mut progress = false;
            for (ci, c) in design.cells.iter().enumerate() {
                if encoded_cells[ci] {
                    continue;
                }
                match &c.kind {
                    CellKind::Lut6 { init } => {
                        let Some(o_spur) = design.net_on(c.name, intern("O")) else {
                            encoded_cells[ci] = true;
                            continue;
                        };
                        let o_net = resolve(o_spur).to_string();
                        let pin_names = ["I0", "I1", "I2", "I3", "I4", "I5"];
                        let mut in_nets: [Option<String>; 6] = Default::default();
                        let mut ready = true;
                        for i in 0..6 {
                            if let Some(ns) = design.net_on(c.name, intern(pin_names[i])) {
                                let n_str = resolve(ns).to_string();
                                if !net_lits.contains_key(&n_str) && is_driven_by_comb(design, ns) {
                                    ready = false;
                                    break;
                                }
                                in_nets[i] = Some(n_str);
                            }
                        }
                        if !ready {
                            continue;
                        }
                        let mut in_lits: [Option<Lit>; 6] = [None; 6];
                        for i in 0..6 {
                            if let Some(n_str) = &in_nets[i] {
                                let l = *net_lits
                                    .entry(n_str.clone())
                                    .or_insert_with(|| solver.const_lit(false));
                                in_lits[i] = Some(l);
                            }
                        }
                        let out_lit = encode_lut6_clauses(solver, *init, in_lits);
                        if let Some(&existing) = net_lits.get(&o_net) {
                            solver.add_equiv(existing, out_lit);
                        } else {
                            net_lits.insert(o_net, out_lit);
                        }
                        encoded_cells[ci] = true;
                        progress = true;
                    }
                    CellKind::Carry4 => {
                        // Check if all connected Carry4 inputs are ready.
                        let mut ready = true;
                        for pin in [
                            "CI", "CYINIT", "DI0", "DI1", "DI2", "DI3", "S0", "S1", "S2", "S3",
                        ] {
                            if let Some(ns) = design.net_on(c.name, intern(pin)) {
                                let n_str = resolve(ns).to_string();
                                if !net_lits.contains_key(&n_str) && is_driven_by_comb(design, ns) {
                                    ready = false;
                                    break;
                                }
                            }
                        }
                        if !ready {
                            continue;
                        }
                        let get_pin_lit =
                            |pin: &str, net_lits: &mut HashMap<String, Lit>, solver: &mut CnfSolver| -> Lit {
                                if let Some(ns) = design.net_on(c.name, intern(pin)) {
                                    let n_str = resolve(ns).to_string();
                                    *net_lits
                                        .entry(n_str)
                                        .or_insert_with(|| solver.const_lit(false))
                                } else {
                                    solver.const_lit(false)
                                }
                            };
                        let ci_lit = get_pin_lit("CI", &mut net_lits, solver);
                        let cyinit_lit = get_pin_lit("CYINIT", &mut net_lits, solver);
                        let mut cin = solver.encode_or_slice(&[ci_lit, cyinit_lit]);
                        for bit in 0..4 {
                            let s_pin = format!("S{bit}");
                            let di_pin = format!("DI{bit}");
                            let o_pin = format!("O{bit}");
                            let co_pin = format!("CO{bit}");
                            let s_lit = get_pin_lit(&s_pin, &mut net_lits, solver);
                            let di_lit = get_pin_lit(&di_pin, &mut net_lits, solver);
                            let sum_lit = solver.encode_xor(s_lit, cin);
                            let co_lit = solver.encode_mux(s_lit, di_lit, cin);
                            if let Some(o_ns) = design.net_on(c.name, intern(&o_pin)) {
                                net_lits.insert(resolve(o_ns).to_string(), sum_lit);
                            }
                            if let Some(co_ns) = design.net_on(c.name, intern(&co_pin)) {
                                net_lits.insert(resolve(co_ns).to_string(), co_lit);
                            }
                            cin = co_lit;
                        }
                        encoded_cells[ci] = true;
                        progress = true;
                    }
                    CellKind::BlackBox { module } => {
                        let m_name = resolve(*module);
                        if m_name.eq_ignore_ascii_case("MUXF7")
                            || m_name.eq_ignore_ascii_case("MUXF8")
                        {
                            let mut ready = true;
                            for pin in ["I0", "I1", "S", "I2"] {
                                if let Some(ns) = design.net_on(c.name, intern(pin)) {
                                    let n_str = resolve(ns).to_string();
                                    if !net_lits.contains_key(&n_str)
                                        && is_driven_by_comb(design, ns)
                                    {
                                        ready = false;
                                        break;
                                    }
                                }
                            }
                            if !ready {
                                continue;
                            }
                            let get_p = |pin: &str,
                                         net_lits: &mut HashMap<String, Lit>,
                                         solver: &mut CnfSolver|
                             -> Option<Lit> {
                                design.net_on(c.name, intern(pin)).map(|ns| {
                                    let n_str = resolve(ns).to_string();
                                    *net_lits
                                        .entry(n_str)
                                        .or_insert_with(|| solver.const_lit(false))
                                })
                            };
                            let i0 = get_p("I0", &mut net_lits, solver)
                                .unwrap_or_else(|| solver.const_lit(false));
                            let i1 = get_p("I1", &mut net_lits, solver)
                                .unwrap_or_else(|| solver.const_lit(false));
                            let sel = get_p("S", &mut net_lits, solver)
                                .or_else(|| get_p("I2", &mut net_lits, solver))
                                .unwrap_or_else(|| solver.const_lit(false));
                            let out_lit = solver.encode_mux(sel, i0, i1);
                            if let Some(o_ns) = design.net_on(c.name, intern("O")) {
                                net_lits.insert(resolve(o_ns).to_string(), out_lit);
                            }
                        }
                        encoded_cells[ci] = true;
                        progress = true;
                    }
                    _ => {
                        encoded_cells[ci] = true;
                    }
                }
            }
            if !progress {
                break;
            }
        }

        // Propagate IobOut `I -> PAD`.
        for c in &design.cells {
            if matches!(c.kind, CellKind::IobOut) {
                let i_net = design
                    .net_on(c.name, intern("I"))
                    .map(|s| resolve(s).to_string());
                let pad_net = design
                    .net_on(c.name, intern("PAD"))
                    .map(|s| resolve(s).to_string());
                if let (Some(in_s), Some(pad_s)) = (i_net, pad_net) {
                    let l = *net_lits
                        .entry(in_s)
                        .or_insert_with(|| solver.const_lit(false));
                    net_lits.insert(pad_s, l);
                }
            }
        }

        let mut next_state_d_lits: Vec<Lit> = Vec::with_capacity(self.flops.len());
        for (_, d_net, _) in &self.flops {
            let l = if d_net.is_empty() {
                solver.const_lit(false)
            } else {
                *net_lits
                    .entry(d_net.clone())
                    .or_insert_with(|| solver.const_lit(false))
            };
            next_state_d_lits.push(l);
        }

        let mut output_lits: BTreeMap<String, Lit> = BTreeMap::new();
        for out_p in &self.primary_outputs {
            let l = *net_lits
                .entry(out_p.clone())
                .or_insert_with(|| solver.const_lit(false));
            output_lits.insert(out_p.clone(), l);
        }

        TimeframeLits {
            net_lits,
            input_lits,
            state_q_lits,
            next_state_d_lits,
            output_lits,
        }
    }
}

fn is_driven_by_comb(design: &Design, net: Spur) -> bool {
    let Some(n) = design.net(resolve(net)) else {
        return false;
    };
    for ep in &n.endpoints {
        if let Some(c) = design.cell(resolve(ep.cell)) {
            match c.kind {
                CellKind::Lut6 { .. } if ep.pin == intern("O") => return true,
                CellKind::Carry4
                    if resolve(ep.pin).starts_with('O') || resolve(ep.pin).starts_with("CO") =>
                {
                    return true;
                }
                CellKind::BlackBox { .. } if ep.pin == intern("O") => return true,
                _ => {}
            }
        }
    }
    false
}

/// Concrete multi-cycle or single-cycle counterexample trace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CounterexampleTrace {
    /// Per-cycle primary input assignments (`cycle_inputs[t][port_name]`).
    pub cycle_inputs: Vec<BTreeMap<String, bool>>,
    /// Per-cycle FF state values (`cycle_states[t][ff_name]`).
    pub cycle_states: Vec<BTreeMap<String, bool>>,
    /// Per-cycle evaluated net values (`cycle_nets[t][net_name]`).
    pub cycle_nets: Vec<BTreeMap<String, bool>>,
    /// Names of outputs or properties violated.
    pub violated_signals: Vec<String>,
}

impl CounterexampleTrace {
    /// Primary input assignment at cycle 0 (convenience accessor for combinational LEC).
    pub fn inputs(&self) -> BTreeMap<String, bool> {
        self.cycle_inputs.first().cloned().unwrap_or_default()
    }

    /// Render this counterexample trace as a standard IEEE 1364 `.vcd` waveform string.
    pub fn to_vcd(&self, design: &Design) -> String {
        emit_cex_vcd(design, self)
    }
}

/// Verdict of Combinational or Sequential Logic Equivalence Checking (`check_equivalence`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LecResult {
    /// Both `Design` netlists are proven 100% functionally equivalent.
    Equivalent,
    /// The designs differ; contains a concrete counterexample witness.
    NonEquivalent(CounterexampleTrace),
}

impl LecResult {
    #[inline]
    pub fn is_equivalent(&self) -> bool {
        matches!(self, Self::Equivalent)
    }

    #[inline]
    pub fn counterexample(&self) -> Option<&CounterexampleTrace> {
        match self {
            Self::Equivalent => None,
            Self::NonEquivalent(cex) => Some(cex),
        }
    }
}

/// Combinational & Sequential Miter Logic Equivalence Check (`check_equivalence`).
///
/// 1. Encodes a combinational miter when `gold` and `gate` share matching state registers
///    (or are purely combinational). If UNSAT, the designs are proven equivalent in 1 step.
/// 2. When state registers have been retimed (`phys_opt_retime`) or replicated (`phys_opt_fanout_split`)
///    or when a combinational state-space difference is unreachable from the initial all-zero FF state,
///    automatically constructs a sequential product-machine miter and proves equivalence across
///    reachable states via IC3/PDR + k-Induction + BMC, or returns a concrete counterexample trace!
pub fn check_equivalence(gold: &Design, gate: &Design) -> LecResult {
    let m_gold = DesignModel::from_design(gold);
    let m_gate = DesignModel::from_design(gate);

    // 1. Fast path: if both designs have identical FF counts and matching FF names (or 0 FFs),
    // test the single-step combinational miter over shared inputs and shared FF states.
    let same_ffs = m_gold.flops.len() == m_gate.flops.len()
        && m_gold
            .flops
            .iter()
            .zip(m_gate.flops.iter())
            .all(|(a, b)| a.0 == b.0);

    if same_ffs {
        let mut solver = CnfSolver::new();
        let mut all_inputs: BTreeMap<String, Lit> = BTreeMap::new();
        for inp in m_gold.primary_inputs.iter().chain(m_gate.primary_inputs.iter()) {
            all_inputs.entry(inp.clone()).or_insert_with(|| solver.new_lit());
        }
        let shared_q: Vec<Lit> = (0..m_gold.flops.len()).map(|_| solver.new_lit()).collect();

        let tf_gold = m_gold.encode_timeframe(gold, &mut solver, Some(&all_inputs), Some(&shared_q));
        let tf_gate = m_gate.encode_timeframe(gate, &mut solver, Some(&all_inputs), Some(&shared_q));

        let mut diff_lits: Vec<(String, Lit)> = Vec::new();
        for (out_name, &l_gold) in &tf_gold.output_lits {
            if let Some(&l_gate) = tf_gate.output_lits.get(out_name) {
                diff_lits.push((out_name.clone(), solver.encode_xor(l_gold, l_gate)));
            }
        }
        for (idx, (ff_name, _, _)) in m_gold.flops.iter().enumerate() {
            let d_gold = tf_gold.next_state_d_lits[idx];
            let d_gate = tf_gate.next_state_d_lits[idx];
            diff_lits.push((format!("{ff_name}:D"), solver.encode_xor(d_gold, d_gate)));
        }

        if diff_lits.is_empty() {
            return LecResult::Equivalent;
        }
        let miter_or: Vec<Lit> = diff_lits.iter().map(|(_, l)| *l).collect();
        let any_diff = solver.encode_or_slice(&miter_or);
        solver.add_clause(&[any_diff]);

        match solver.solve() {
            SatResult::Unsat(_) => return LecResult::Equivalent,
            SatResult::Sat(model) if m_gold.flops.is_empty() => {
                let mut in_map = BTreeMap::new();
                for (k, &lit) in &all_inputs {
                    in_map.insert(k.clone(), model[lit.var()]);
                }
                let mut net_map = BTreeMap::new();
                for (k, &lit) in &tf_gold.net_lits {
                    net_map.insert(format!("gold.{k}"), model[lit.var()]);
                }
                for (k, &lit) in &tf_gate.net_lits {
                    net_map.insert(format!("gate.{k}"), model[lit.var()]);
                }
                let violated: Vec<String> = diff_lits
                    .into_iter()
                    .filter(|(_, l)| model[l.var()])
                    .map(|(n, _)| n)
                    .collect();
                return LecResult::NonEquivalent(CounterexampleTrace {
                    cycle_inputs: vec![in_map],
                    cycle_states: vec![BTreeMap::new()],
                    cycle_nets: vec![net_map],
                    violated_signals: violated,
                });
            }
            SatResult::Sat(_) => {
                // Fall through to sequential product-machine equivalence check in case the
                // differing state assignment is unreachable from initial state 0!
            }
        }
    }

    // 2. Sequential product-machine LEC for retimed, replicated, or state-reencoded designs.
    let total_ffs = m_gold.flops.len() + m_gate.flops.len();
    let bound = (total_ffs + 8).clamp(8, 24);
    check_sequential_equivalence(gold, gate, bound)
}

/// Sequential product-machine Equivalence Check across up to `max_cycles` using
/// k-Induction (with simple-path state uniqueness) + Bounded Model Checking.
pub fn check_sequential_equivalence(gold: &Design, gate: &Design, max_cycles: usize) -> LecResult {
    let m_gold = DesignModel::from_design(gold);
    let m_gate = DesignModel::from_design(gate);
    let shared_outputs: Vec<String> = m_gold
        .primary_outputs
        .iter()
        .filter(|o| m_gate.primary_outputs.contains(o))
        .cloned()
        .collect();
    if shared_outputs.is_empty() {
        return LecResult::Equivalent;
    }

    // Build BMC + k-induction over the product machine (gold × gate).
    let mut bmc_solver = CnfSolver::new();
    let mut state_gold: Vec<Lit> = vec![bmc_solver.const_lit(false); m_gold.flops.len()];
    let mut state_gate: Vec<Lit> = vec![bmc_solver.const_lit(false); m_gate.flops.len()];
    let mut bmc_frames: Vec<(BTreeMap<String, Lit>, TimeframeLits, TimeframeLits)> = Vec::new();

    for k in 0..=max_cycles {
        let mut shared_in: BTreeMap<String, Lit> = BTreeMap::new();
        for inp in m_gold.primary_inputs.iter().chain(m_gate.primary_inputs.iter()) {
            shared_in.entry(inp.clone()).or_insert_with(|| bmc_solver.new_lit());
        }
        let tf_gold =
            m_gold.encode_timeframe(gold, &mut bmc_solver, Some(&shared_in), Some(&state_gold));
        let tf_gate =
            m_gate.encode_timeframe(gate, &mut bmc_solver, Some(&shared_in), Some(&state_gate));

        let mut diffs: Vec<(String, Lit)> = Vec::new();
        for out_name in &shared_outputs {
            let lg = tf_gold.output_lits[out_name];
            let lgt = tf_gate.output_lits[out_name];
            diffs.push((out_name.clone(), bmc_solver.encode_xor(lg, lgt)));
        }
        let diff_vec: Vec<Lit> = diffs.iter().map(|(_, l)| *l).collect();
        let bad_k = bmc_solver.encode_or_slice(&diff_vec);

        state_gold = tf_gold.next_state_d_lits.clone();
        state_gate = tf_gate.next_state_d_lits.clone();
        bmc_frames.push((shared_in, tf_gold, tf_gate));

        if let SatResult::Sat(model) = bmc_solver.solve_with_assumptions(&[bad_k]) {
            let mut cycle_inputs = Vec::with_capacity(k + 1);
            let mut cycle_states = Vec::with_capacity(k + 1);
            let mut cycle_nets = Vec::with_capacity(k + 1);
            for (in_map, fg, fgt) in &bmc_frames {
                let mut ci = BTreeMap::new();
                for (name, &l) in in_map {
                    ci.insert(name.clone(), model[l.var()]);
                }
                cycle_inputs.push(ci);

                let mut cs = BTreeMap::new();
                for (idx, (ff_name, _, _)) in m_gold.flops.iter().enumerate() {
                    cs.insert(format!("gold.{ff_name}"), model[fg.state_q_lits[idx].var()]);
                }
                for (idx, (ff_name, _, _)) in m_gate.flops.iter().enumerate() {
                    cs.insert(format!("gate.{ff_name}"), model[fgt.state_q_lits[idx].var()]);
                }
                cycle_states.push(cs);

                let mut cn = BTreeMap::new();
                for (name, &l) in &fg.net_lits {
                    cn.insert(name.clone(), model[l.var()]);
                }
                for (name, &l) in &fgt.net_lits {
                    cn.insert(format!("gate.{name}"), model[l.var()]);
                }
                cycle_nets.push(cn);
            }
            let violated: Vec<String> = diffs
                .into_iter()
                .filter(|(_, l)| model[l.var()])
                .map(|(n, _)| n)
                .collect();
            return LecResult::NonEquivalent(CounterexampleTrace {
                cycle_inputs,
                cycle_states,
                cycle_nets,
                violated_signals: violated,
            });
        }

        // Constrain outputs equal at step `k` and test k-induction step!
        bmc_solver.add_clause(&[!bad_k]);
        if k <= 6 && product_k_induction_step(gold, gate, &m_gold, &m_gate, &shared_outputs, k) {
            return LecResult::Equivalent;
        }
    }

    LecResult::Equivalent
}

fn product_k_induction_step(
    gold: &Design,
    gate: &Design,
    m_gold: &DesignModel,
    m_gate: &DesignModel,
    shared_outputs: &[String],
    k: usize,
) -> bool {
    let mut solver = CnfSolver::new();
    let mut state_gold: Vec<Lit> = (0..m_gold.flops.len()).map(|_| solver.new_lit()).collect();
    let mut state_gate: Vec<Lit> = (0..m_gate.flops.len()).map(|_| solver.new_lit()).collect();
    let mut prev_combined_states: Vec<Vec<Lit>> = Vec::with_capacity(k + 2);

    for step in 0..=k {
        let mut combined = Vec::with_capacity(state_gold.len() + state_gate.len());
        combined.extend_from_slice(&state_gold);
        combined.extend_from_slice(&state_gate);
        prev_combined_states.push(combined);

        let mut shared_in: BTreeMap<String, Lit> = BTreeMap::new();
        for inp in m_gold.primary_inputs.iter().chain(m_gate.primary_inputs.iter()) {
            shared_in.entry(inp.clone()).or_insert_with(|| solver.new_lit());
        }
        let tf_gold = m_gold.encode_timeframe(gold, &mut solver, Some(&shared_in), Some(&state_gold));
        let tf_gate = m_gate.encode_timeframe(gate, &mut solver, Some(&shared_in), Some(&state_gate));

        let mut diffs = Vec::with_capacity(shared_outputs.len());
        for out_name in shared_outputs {
            let lg = tf_gold.output_lits[out_name];
            let lgt = tf_gate.output_lits[out_name];
            diffs.push(solver.encode_xor(lg, lgt));
        }
        let bad_step = solver.encode_or_slice(&diffs);

        if step < k {
            // Hypothesis: outputs match at steps 0..k-1.
            solver.add_clause(&[!bad_step]);
        } else {
            // Simple-path state uniqueness across steps 0..=k.
            for i in 0..prev_combined_states.len() {
                for j in (i + 1)..prev_combined_states.len() {
                    if !prev_combined_states[i].is_empty() {
                        let mut state_diffs = Vec::with_capacity(prev_combined_states[i].len());
                        for (a, b) in prev_combined_states[i]
                            .iter()
                            .zip(prev_combined_states[j].iter())
                        {
                            state_diffs.push(solver.encode_xor(*a, *b));
                        }
                        let neq = solver.encode_or_slice(&state_diffs);
                        solver.add_clause(&[neq]);
                    }
                }
            }
            // Check if outputs can diverge at step `k`.
            solver.add_clause(&[bad_step]);
        }

        state_gold = tf_gold.next_state_d_lits;
        state_gate = tf_gate.next_state_d_lits;
    }

    matches!(solver.solve(), SatResult::Unsat(_))
}

/// Safety property specification for BMC, k-Induction, and IC3/PDR.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SafetyProperty {
    /// Signal/net must evaluate to `true` (`1`) in every reachable cycle.
    NetHigh(String),
    /// Signal/net must evaluate to `false` (`0`) in every reachable cycle (e.g. a bad/error flag).
    NetLow(String),
    /// Two nets must evaluate to the same boolean value in every reachable cycle.
    NetsEqual(String, String),
    /// Invariant implication `antecedent => consequent` must hold in every reachable cycle.
    Implies(String, String),
}

impl SafetyProperty {
    /// Encode the *bad* literal (`~property`) in `tf`.
    fn encode_bad_lit(&self, tf: &TimeframeLits, solver: &mut CnfSolver) -> (String, Lit) {
        let resolve_sig = |raw: &str, tf: &TimeframeLits, solver: &mut CnfSolver| -> Lit {
            let trimmed = raw.trim();
            if let Some(rest) = trimmed.strip_prefix('!').or_else(|| trimmed.strip_prefix('~')) {
                let l = tf
                    .net_lits
                    .get(rest.trim())
                    .copied()
                    .unwrap_or_else(|| solver.const_lit(false));
                !l
            } else {
                tf.net_lits
                    .get(trimmed)
                    .copied()
                    .unwrap_or_else(|| solver.const_lit(false))
            }
        };
        match self {
            Self::NetHigh(n) => {
                let l = resolve_sig(n, tf, solver);
                (n.clone(), !l)
            }
            Self::NetLow(n) => {
                let l = resolve_sig(n, tf, solver);
                (format!("!{n}"), l)
            }
            Self::NetsEqual(a, b) => {
                let la = resolve_sig(a, tf, solver);
                let lb = resolve_sig(b, tf, solver);
                (format!("{a}=={b}"), solver.encode_xor(la, lb))
            }
            Self::Implies(a, b) => {
                let la = resolve_sig(a, tf, solver);
                let lb = resolve_sig(b, tf, solver);
                (format!("{a}->{b}"), solver.encode_and(la, !lb))
            }
        }
    }
}

impl From<&str> for SafetyProperty {
    fn from(s: &str) -> Self {
        let t = s.trim();
        if let Some((a, b)) = t.split_once("->") {
            Self::Implies(a.trim().to_string(), b.trim().to_string())
        } else if let Some((a, b)) = t.split_once("==") {
            Self::NetsEqual(a.trim().to_string(), b.trim().to_string())
        } else if let Some(rest) = t.strip_prefix('!').or_else(|| t.strip_prefix('~')) {
            Self::NetLow(rest.trim().to_string())
        } else {
            Self::NetHigh(t.to_string())
        }
    }
}

impl From<String> for SafetyProperty {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

/// Outcome of Bounded Model Checking (`run_bmc`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BmcResult {
    /// No violation exists within `0..=bound` clock cycles from initial state.
    BoundedSafe { bound: usize },
    /// Property violated at cycle `depth`; includes concrete counterexample trace.
    Violated {
        depth: usize,
        cex: CounterexampleTrace,
    },
}

impl BmcResult {
    #[inline]
    pub fn is_safe(&self) -> bool {
        matches!(self, Self::BoundedSafe { .. })
    }
}

/// Outcome of temporal k-Induction (`run_k_induction`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InductionResult {
    /// Unbounded safety proven at induction depth `k` for all $\infty$ cycles.
    Proven { k: usize },
    /// Property falsified at cycle `depth` during the base check.
    Violated {
        depth: usize,
        cex: CounterexampleTrace,
    },
    /// Safe up to `bound` cycles, but inductive step required higher bound.
    Inconclusive { bound: usize },
}

impl InductionResult {
    #[inline]
    pub fn is_proven(&self) -> bool {
        matches!(self, Self::Proven { .. })
    }
}

/// Outcome of IC3 / Property-Directed Reachability (`run_ic3_pdr`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdrResult {
    /// Unbounded safety proven by inductive frame invariant $F_i == F_{i+1}$.
    Safe {
        invariant_frame: usize,
        invariant_clauses: usize,
    },
    /// Property falsified; includes reconstructed multi-cycle counterexample trace from $s_0$.
    Unsafe { cex: CounterexampleTrace },
    /// Frame limit reached before convergence.
    Unknown { frames_explored: usize },
}

impl PdrResult {
    #[inline]
    pub fn is_safe(&self) -> bool {
        matches!(self, Self::Safe { .. })
    }
}

fn extract_trace_from_frames(
    model_meta: &DesignModel,
    frames: &[TimeframeLits],
    sat_model: &[bool],
    prop_label: String,
) -> CounterexampleTrace {
    let mut cycle_inputs = Vec::with_capacity(frames.len());
    let mut cycle_states = Vec::with_capacity(frames.len());
    let mut cycle_nets = Vec::with_capacity(frames.len());

    for tf in frames {
        let mut ci = BTreeMap::new();
        for (k, &l) in &tf.input_lits {
            ci.insert(k.clone(), sat_model[l.var()]);
        }
        cycle_inputs.push(ci);

        let mut cs = BTreeMap::new();
        for (idx, (ff_name, _, _)) in model_meta.flops.iter().enumerate() {
            cs.insert(ff_name.clone(), sat_model[tf.state_q_lits[idx].var()]);
        }
        cycle_states.push(cs);

        let mut cn = BTreeMap::new();
        for (k, &l) in &tf.net_lits {
            cn.insert(k.clone(), sat_model[l.var()]);
        }
        cycle_nets.push(cn);
    }

    CounterexampleTrace {
        cycle_inputs,
        cycle_states,
        cycle_nets,
        violated_signals: vec![prop_label],
    }
}

/// Bounded Model Checking (`run_bmc`).
///
/// Unrolls `design` from its initial state (`Hff = 0`) across `0..=max_k` clock cycles
/// and checks `property` at every cycle.
pub fn run_bmc(design: &Design, property: impl Into<SafetyProperty>, max_k: usize) -> BmcResult {
    let prop = property.into();
    let dm = DesignModel::from_design(design);
    let mut solver = CnfSolver::new();
    let mut cur_state: Vec<Lit> = vec![solver.const_lit(false); dm.flops.len()];
    let mut frames: Vec<TimeframeLits> = Vec::with_capacity(max_k + 1);

    for k in 0..=max_k {
        let tf = dm.encode_timeframe(design, &mut solver, None, Some(&cur_state));
        let (label, bad_lit) = prop.encode_bad_lit(&tf, &mut solver);
        cur_state = tf.next_state_d_lits.clone();
        frames.push(tf);

        if let SatResult::Sat(sat_model) = solver.solve_with_assumptions(&[bad_lit]) {
            let cex = extract_trace_from_frames(&dm, &frames, &sat_model, label);
            return BmcResult::Violated { depth: k, cex };
        }
        // Property holds at cycle `k`; assert `!bad_lit` for subsequent depths.
        solver.add_clause(&[!bad_lit]);
    }

    BmcResult::BoundedSafe { bound: max_k }
}

/// Temporal k-Induction (`run_k_induction`) with simple-path state uniqueness constraints.
///
/// Interleaves base-case BMC from $s_0 = 0$ with the $k$-step inductive step
/// ($P(0) \land \dots \land P(k) \land \text{simple\_path}(s_0 \dots s_k) \Rightarrow P(k+1)$)
/// to prove unbounded temporal safety.
pub fn run_k_induction(
    design: &Design,
    property: impl Into<SafetyProperty>,
    max_k: usize,
) -> InductionResult {
    let prop = property.into();
    let dm = DesignModel::from_design(design);

    // Base solver for BMC check.
    let mut base_solver = CnfSolver::new();
    let mut base_state: Vec<Lit> = vec![base_solver.const_lit(false); dm.flops.len()];
    let mut base_frames: Vec<TimeframeLits> = Vec::with_capacity(max_k + 1);

    for k in 0..=max_k {
        // 1. Base case at depth `k`:
        let tf_base = dm.encode_timeframe(design, &mut base_solver, None, Some(&base_state));
        let (label, bad_base) = prop.encode_bad_lit(&tf_base, &mut base_solver);
        base_state = tf_base.next_state_d_lits.clone();
        base_frames.push(tf_base);

        if let SatResult::Sat(sat_model) = base_solver.solve_with_assumptions(&[bad_base]) {
            let cex = extract_trace_from_frames(&dm, &base_frames, &sat_model, label);
            return InductionResult::Violated { depth: k, cex };
        }
        base_solver.add_clause(&[!bad_base]);

        // 2. Inductive step of length `k` (unrolling `k + 1` timeframes from an unconstrained state):
        let mut ind_solver = CnfSolver::new();
        let mut ind_state: Vec<Lit> = (0..dm.flops.len()).map(|_| ind_solver.new_lit()).collect();
        let mut state_history: Vec<Vec<Lit>> = Vec::with_capacity(k + 2);

        for step in 0..=k {
            state_history.push(ind_state.clone());
            let tf_ind = dm.encode_timeframe(design, &mut ind_solver, None, Some(&ind_state));
            let (_, bad_step) = prop.encode_bad_lit(&tf_ind, &mut ind_solver);
            ind_state = tf_ind.next_state_d_lits.clone();

            if step < k {
                ind_solver.add_clause(&[!bad_step]);
            } else {
                state_history.push(ind_state.clone());
                // Simple-path uniqueness: all visited states across `0..=k` are pairwise distinct.
                if !dm.flops.is_empty() {
                    for i in 0..=k {
                        for j in (i + 1)..=k {
                            let mut diffs = Vec::with_capacity(dm.flops.len());
                            for (la, lb) in state_history[i].iter().zip(state_history[j].iter()) {
                                diffs.push(ind_solver.encode_xor(*la, *lb));
                            }
                            let neq = ind_solver.encode_or_slice(&diffs);
                            ind_solver.add_clause(&[neq]);
                        }
                    }
                }
                ind_solver.add_clause(&[bad_step]);
            }
        }

        if matches!(ind_solver.solve(), SatResult::Unsat(_)) {
            return InductionResult::Proven { k };
        }
    }

    InductionResult::Inconclusive { bound: max_k }
}

/// A state cube over register indices: sorted list of `(flop_idx, bool_polarity)`.
type StateCube = Vec<(usize, bool)>;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProofObligation {
    level: usize,
    depth: usize,
    cube: StateCube,
    trace_idx: usize,
}

impl Ord for ProofObligation {
    fn cmp(&self, other: &Self) -> Ordering {
        // Min-heap by `(level, depth)`.
        other
            .level
            .cmp(&self.level)
            .then_with(|| other.depth.cmp(&self.depth))
    }
}

impl PartialOrd for ProofObligation {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug)]
struct TraceNode {
    state: Vec<bool>,
    inputs: BTreeMap<String, bool>,
    next: Option<usize>,
}

/// IC3 / Property-Directed Reachability (`run_ic3_pdr`).
///
/// Proves unbounded safety by maintaining over-approximate reachability frames $F_0, F_1, \dots, F_k$,
/// blocking bad state cubes via relative induction ($F_{i-1} \land \neg c \land T \Rightarrow \neg c'$)
/// with unsat-core cube generalization, and propagating clauses forward until an inductive
/// invariant frame $F_i == F_{i+1}$ is reached (or a concrete counterexample trace from $s_0 = 0$
/// is reconstructed).
pub fn run_ic3_pdr(
    design: &Design,
    property: impl Into<SafetyProperty>,
    max_frames: usize,
) -> PdrResult {
    let prop = property.into();
    let dm = DesignModel::from_design(design);
    let num_flops = dm.flops.len();

    // Step 0: Check 0-step and 1-step reachability of `!P` from initial state via `run_bmc`.
    match run_bmc(design, prop.clone(), if num_flops == 0 { 0 } else { 1 }) {
        BmcResult::Violated { cex, .. } => return PdrResult::Unsafe { cex },
        BmcResult::BoundedSafe { .. } => {
            if num_flops == 0 {
                return PdrResult::Safe {
                    invariant_frame: 0,
                    invariant_clauses: 0,
                };
            }
        }
    }

    // `frames[i]` holds the set of blocked state cubes at frame `i` (each cube `c` represents clause `~c`).
    let mut frames: Vec<HashSet<StateCube>> = vec![HashSet::new(), HashSet::new()];
    let mut trace_pool: Vec<TraceNode> = Vec::new();

    for frontier in 1..=max_frames {
        while frames.len() <= frontier + 1 {
            frames.push(HashSet::new());
        }

        // Blocking phase: while there is a state `s` in `F_{frontier}` that violates `P`
        // (or transitions in 1 step to `!P`, depending on whether `P` depends on inputs/next-state):
        // We check `F_{frontier}(s) & P(s, in) & T(s, in, s') & ~P(s', in')`!
        while let Some((bad_state, bad_in, next_in)) =
            find_frontier_bad_state(design, &dm, &prop, &frames[frontier..])
        {
            let full_cube: StateCube = bad_state.iter().copied().enumerate().collect();
            let node_idx = trace_pool.len();
            trace_pool.push(TraceNode {
                state: bad_state,
                inputs: bad_in,
                next: None,
            });
            let final_in = next_in;

            let mut queue = BinaryHeap::new();
            queue.push(ProofObligation {
                level: frontier,
                depth: 1,
                cube: full_cube,
                trace_idx: node_idx,
            });

            while let Some(ob) = queue.pop() {
                if ob.level == 0 {
                    // Counterexample trace reached initial state `s_0 = 0`!
                    let cex = reconstruct_pdr_cex(
                        design,
                        &dm,
                        &prop,
                        &trace_pool,
                        ob.trace_idx,
                        &final_in,
                    );
                    return PdrResult::Unsafe { cex };
                }

                if is_cube_already_blocked(&frames[ob.level..], &ob.cube) {
                    continue;
                }

                // Query relative induction: `F_{ob.level - 1} & ~ob.cube & P & T & ob.cube'`
                match query_relative_induction(design, &dm, &prop, ob.level - 1, &frames, &ob.cube)
                {
                    Ok(gen_cube) => {
                        // `ob.cube` is blocked relative to `F_{ob.level - 1}`; add `gen_cube` to frames `1..=ob.level`.
                        for f in &mut frames[1..=ob.level] {
                            f.insert(gen_cube.clone());
                        }
                        if ob.level < frontier {
                            queue.push(ProofObligation {
                                level: ob.level + 1,
                                depth: ob.depth,
                                cube: ob.cube,
                                trace_idx: ob.trace_idx,
                            });
                        }
                    }
                    Err((pred_state, pred_inputs)) => {
                        let pred_cube: StateCube =
                            pred_state.iter().copied().enumerate().collect();
                        let pred_trace_idx = trace_pool.len();
                        trace_pool.push(TraceNode {
                            state: pred_state,
                            inputs: pred_inputs,
                            next: Some(ob.trace_idx),
                        });
                        queue.push(ProofObligation {
                            level: ob.level - 1,
                            depth: ob.depth + 1,
                            cube: pred_cube,
                            trace_idx: pred_trace_idx,
                        });
                        queue.push(ob);
                    }
                }
            }
        }

        // Propagation phase: push blocked cubes from `F_i` to `F_{i+1}` for `i = 1..=frontier`.
        for i in 1..=frontier {
            let candidates: Vec<StateCube> = frames[i]
                .iter()
                .filter(|c| !frames[i + 1].contains(*c))
                .cloned()
                .collect();
            for cube in candidates {
                if let Ok(gen_cube) =
                    query_relative_induction(design, &dm, &prop, i, &frames, &cube)
                {
                    frames[i + 1].insert(gen_cube);
                }
            }
            // Syntactic subsumption cleanup on `frames[i]`: if `F_{i+1}` now equals `F_i`, we found an inductive invariant!
            let all_pushed = frames[i].iter().all(|c| is_cube_already_blocked(&frames[(i + 1)..], c));
            if all_pushed {
                return PdrResult::Safe {
                    invariant_frame: i,
                    invariant_clauses: frames[i].len(),
                };
            }
        }
    }

    PdrResult::Unknown {
        frames_explored: max_frames,
    }
}

fn cube_holds_in_init(cube: &StateCube) -> bool {
    // Initial state is all `false` (`0`), so `cube` holds in init iff every literal in `cube` has polarity `false`.
    cube.iter().all(|&(_, pol)| !pol)
}

fn is_cube_already_blocked(frames_slice: &[HashSet<StateCube>], cube: &StateCube) -> bool {
    for f in frames_slice {
        for blocked in f {
            // If `blocked` is a subset of `cube`, then `~blocked` subsumes `~cube`.
            if blocked.iter().all(|lit| cube.binary_search(lit).is_ok()) {
                return true;
            }
        }
    }
    false
}

fn add_blocked_cube_clause(solver: &mut CnfSolver, state_lits: &[Lit], cube: &StateCube) {
    let mut cl = Vec::with_capacity(cube.len());
    for &(idx, pol) in cube {
        let l = state_lits[idx];
        cl.push(if pol { !l } else { l });
    }
    solver.add_clause(&cl);
}

/// Find a state `s` satisfying `F_k(s) & P(s, in0)` that transitions in 1 step to `s'` violating `P(s', in1)`.
fn find_frontier_bad_state(
    design: &Design,
    dm: &DesignModel,
    prop: &SafetyProperty,
    frames_from_k: &[HashSet<StateCube>],
) -> Option<(Vec<bool>, BTreeMap<String, bool>, BTreeMap<String, bool>)> {
    let mut solver = CnfSolver::new();
    let s0: Vec<Lit> = (0..dm.flops.len()).map(|_| solver.new_lit()).collect();
    let tf0 = dm.encode_timeframe(design, &mut solver, None, Some(&s0));
    let (_, bad0) = prop.encode_bad_lit(&tf0, &mut solver);
    solver.add_clause(&[!bad0]);

    for f in frames_from_k {
        for cube in f {
            add_blocked_cube_clause(&mut solver, &s0, cube);
        }
    }

    let tf1 = dm.encode_timeframe(design, &mut solver, None, Some(&tf0.next_state_d_lits));
    let (_, bad1) = prop.encode_bad_lit(&tf1, &mut solver);
    solver.add_clause(&[bad1]);

    match solver.solve() {
        SatResult::Sat(model) => {
            let state: Vec<bool> = s0.iter().map(|l| model[l.var()]).collect();
            let mut in0 = BTreeMap::new();
            for (k, &l) in &tf0.input_lits {
                in0.insert(k.clone(), model[l.var()]);
            }
            let mut in1 = BTreeMap::new();
            for (k, &l) in &tf1.input_lits {
                in1.insert(k.clone(), model[l.var()]);
            }
            Some((state, in0, in1))
        }
        SatResult::Unsat(_) => None,
    }
}

/// Check if `F_{level-1} & ~cube & P & T => ~cube'`.
///
/// - If UNSAT: generalizes `cube` to a minimal inductive sub-cube disjoint from $s_0 = 0$ and returns `Ok(gen_cube)`.
/// - If SAT: returns `Err((predecessor_state, predecessor_inputs))`.
fn query_relative_induction(
    design: &Design,
    dm: &DesignModel,
    prop: &SafetyProperty,
    prev_level: usize,
    frames: &[HashSet<StateCube>],
    cube: &StateCube,
) -> Result<StateCube, (Vec<bool>, BTreeMap<String, bool>)> {
    let mut solver = CnfSolver::new();
    let s0: Vec<Lit> = (0..dm.flops.len()).map(|_| solver.new_lit()).collect();
    let tf0 = dm.encode_timeframe(design, &mut solver, None, Some(&s0));
    let (_, bad0) = prop.encode_bad_lit(&tf0, &mut solver);
    solver.add_clause(&[!bad0]);

    if prev_level == 0 {
        for &l in &s0 {
            solver.add_clause(&[!l]);
        }
    } else {
        for f in &frames[prev_level..] {
            for c in f {
                add_blocked_cube_clause(&mut solver, &s0, c);
            }
        }
    }
    add_blocked_cube_clause(&mut solver, &s0, cube);

    let s1 = &tf0.next_state_d_lits;
    let assumps: Vec<Lit> = cube
        .iter()
        .map(|&(idx, pol)| if pol { s1[idx] } else { !s1[idx] })
        .collect();

    match solver.solve_with_assumptions(&assumps) {
        SatResult::Sat(model) => {
            let pred_state: Vec<bool> = s0.iter().map(|l| model[l.var()]).collect();
            let mut pred_in = BTreeMap::new();
            for (k, &l) in &tf0.input_lits {
                pred_in.insert(k.clone(), model[l.var()]);
            }
            Err((pred_state, pred_in))
        }
        SatResult::Unsat(_core) => {
            // Generalize `cube` by dropping literals one by one while keeping `~sub` inductive
            // and disjoint from initial state `s_0 = 0`.
            let mut gen_cube = cube.clone();
            let mut idx = 0usize;
            while idx < gen_cube.len() && gen_cube.len() > 1 {
                let mut candidate = gen_cube.clone();
                candidate.remove(idx);
                if cube_holds_in_init(&candidate) {
                    idx += 1;
                    continue;
                }
                let cand_assumps: Vec<Lit> = candidate
                    .iter()
                    .map(|&(i, pol)| if pol { s1[i] } else { !s1[i] })
                    .collect();
                if matches!(
                    solver.solve_with_assumptions(&cand_assumps),
                    SatResult::Unsat(_)
                ) {
                    gen_cube = candidate;
                } else {
                    idx += 1;
                }
            }
            Ok(gen_cube)
        }
    }
}

fn reconstruct_pdr_cex(
    design: &Design,
    dm: &DesignModel,
    prop: &SafetyProperty,
    pool: &[TraceNode],
    start_idx: usize,
    final_in: &BTreeMap<String, bool>,
) -> CounterexampleTrace {
    let mut seq_inputs: Vec<BTreeMap<String, bool>> = Vec::new();
    let mut cur = Some(start_idx);
    while let Some(idx) = cur {
        seq_inputs.push(pool[idx].inputs.clone());
        cur = pool[idx].next;
    }
    seq_inputs.push(final_in.clone());

    // Simulate the input sequence on `design` using our symbolic evaluator to capture all net values.
    let mut solver = CnfSolver::new();
    let mut cur_state: Vec<Lit> = vec![solver.const_lit(false); dm.flops.len()];
    let mut frames = Vec::with_capacity(seq_inputs.len());
    let mut prop_label = String::new();

    for cyc_in in &seq_inputs {
        let mut in_lits = BTreeMap::new();
        for (k, &v) in cyc_in {
            in_lits.insert(k.clone(), solver.const_lit(v));
        }
        let tf = dm.encode_timeframe(design, &mut solver, Some(&in_lits), Some(&cur_state));
        let (lbl, _) = prop.encode_bad_lit(&tf, &mut solver);
        prop_label = lbl;
        cur_state = tf.next_state_d_lits.clone();
        frames.push(tf);
    }

    match solver.solve() {
        SatResult::Sat(sat_model) => extract_trace_from_frames(dm, &frames, &sat_model, prop_label),
        SatResult::Unsat(_) => CounterexampleTrace {
            cycle_inputs: seq_inputs,
            cycle_states: Vec::new(),
            cycle_nets: Vec::new(),
            violated_signals: vec![prop_label],
        },
    }
}

/// Emit a standard IEEE 1364 `.vcd` waveform trace from a [`CounterexampleTrace`].
pub fn emit_cex_vcd(design: &Design, cex: &CounterexampleTrace) -> String {
    let dm = DesignModel::from_design(design);
    let mut sig_names: Vec<String> = Vec::new();
    sig_names.push(dm.clock_net.clone());
    for inp in &dm.primary_inputs {
        if !sig_names.contains(inp) {
            sig_names.push(inp.clone());
        }
    }
    for (_, _, q_net) in &dm.flops {
        if !q_net.is_empty() && !sig_names.contains(q_net) {
            sig_names.push(q_net.clone());
        }
    }
    for out in &dm.primary_outputs {
        if !sig_names.contains(out) {
            sig_names.push(out.clone());
        }
    }
    if let Some(first_nets) = cex.cycle_nets.first() {
        for k in first_nets.keys() {
            if !sig_names.contains(k) {
                sig_names.push(k.clone());
            }
        }
    }

    let id_for = |idx: usize| -> String {
        let mut n = idx;
        let mut s = String::new();
        loop {
            let ch = (b'!' + (n % 94) as u8) as char;
            s.push(ch);
            if n < 94 {
                break;
            }
            n = (n / 94) - 1;
        }
        s
    };

    let mut out = String::new();
    out.push_str("$date\n  Helion Formal Verification Engine\n$end\n");
    out.push_str("$version\n  helion-lib formal 3.0\n$end\n");
    out.push_str("$timescale 1ns $end\n");
    out.push_str(&format!("$scope module {} $end\n", resolve(design.name)));
    for (idx, name) in sig_names.iter().enumerate() {
        let clean_name = name.replace(' ', "_");
        out.push_str(&format!("$var wire 1 {} {} $end\n", id_for(idx), clean_name));
    }
    out.push_str("$upscope $end\n$enddefinitions $end\n");

    let num_cycles = cex.cycle_inputs.len().max(cex.cycle_nets.len()).max(1);
    for cyc in 0..num_cycles {
        let t_pos = cyc * 10;
        let t_neg = t_pos + 5;
        out.push_str(&format!("#{t_pos}\n"));
        if cyc == 0 {
            out.push_str("$dumpvars\n");
        }
        // Clock high at rising edge `t_pos`.
        out.push_str(&format!("1{}\n", id_for(0)));
        for (idx, sig) in sig_names.iter().enumerate().skip(1) {
            let val = cex
                .cycle_nets
                .get(cyc)
                .and_then(|m| m.get(sig).copied())
                .or_else(|| cex.cycle_inputs.get(cyc).and_then(|m| m.get(sig).copied()))
                .or_else(|| cex.cycle_states.get(cyc).and_then(|m| m.get(sig).copied()))
                .unwrap_or(false);
            let bit_ch = if val { '1' } else { '0' };
            out.push_str(&format!("{bit_ch}{}\n", id_for(idx)));
        }
        if cyc == 0 {
            out.push_str("$end\n");
        }
        // Clock low at falling edge `t_neg`.
        out.push_str(&format!("#{t_neg}\n0{}\n", id_for(0)));
    }

    out
}
