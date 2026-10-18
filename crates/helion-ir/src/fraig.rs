//! ABC `&fraig` SAT-Sweeping Functional Reduction (`fraig_sat_sweep`) and
//! 4-Input Cut DAG-Aware AIG/LUT Rewriting (`rewrite_aig_4cut`).
//!
//! 1. **64-Bit Parallel Simulation (`simulate_parallel_64`)**:
//!    Simulates 64 input patterns simultaneously across the combinational LUT
//!    DAG using bitwise cofactor evaluation (`eval_lut6_parallel_u64`), partitioning
//!    nets into candidate functional equivalence classes up to bitwise complement.
//! 2. **SAT-Guided FRAIG Sweeping (`fraig_sat_sweep`)**:
//!    For each candidate equivalence pair $(u, v)$ (or $u \equiv \neg v$, or constant),
//!    constructs the structural Tseitin CNF miter cone and solves via an embedded
//!    CDCL SAT solver:
//!    - **UNSAT**: proves $v \equiv u$ (or $v \equiv \neg u$) across all $2^N$ inputs,
//!      merging $v \to u$ (or pushing the inversion into fanout LUT `INIT` masks)
//!      in topological order.
//!    - **SAT**: extracts the counterexample primary input vector and injects it into
//!      the 64-bit simulation pattern table to refine all remaining candidate classes.
//! 3. **4-Input Cut DAG-Aware Rewriting (`rewrite_aig_4cut`)**:
//!    Enumerates $K \le 4$ cuts across LUT cones, canonicalizes 4-input truth tables,
//!    shares functionally identical 4-input subgraphs across the DAG, and collapses
//!    redundant multi-level LUT cascades with $\le 4$ combined inputs without
//!    increasing combinational logic depth.

use std::collections::{BTreeMap, HashMap, HashSet};

use lasso::Spur;

use crate::{intern, resolve, CellKind, Design, Endpoint, PortDir};

/// Elementary 64-bit truth tables for LUT6 inputs `I0..I5`.
pub const FRAIG_VAR_INIT: [u64; 6] = [
    0xAAAA_AAAA_AAAA_AAAA,
    0xCCCC_CCCC_CCCC_CCCC,
    0xF0F0_F0F0_F0F0_F0F0,
    0xFF00_FF00_FF00_FF00,
    0xFFFF_0000_FFFF_0000,
    0xFFFF_FFFF_0000_0000,
];

/// Low cofactor bitmasks for each of the 6 LUT input positions.
const COFACTOR_LO_MASK: [u64; 6] = [
    0x5555_5555_5555_5555,
    0x3333_3333_3333_3333,
    0x0F0F_0F0F_0F0F_0F0F,
    0x00FF_00FF_00FF_00FF,
    0x0000_FFFF_0000_FFFF,
    0x0000_0000_FFFF_FFFF,
];

/// Statistics returned by [`fraig_sat_sweep`] / [`Design::fraig_sat_sweep`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FraigSweepStats {
    /// LUTs proven constant and folded/swept.
    pub consts_merged: usize,
    /// Equivalent LUT pairs ($v \equiv u$) merged.
    pub equiv_merged: usize,
    /// Inverted-equivalent LUT pairs ($v \equiv \neg u$) merged via fanout polarity absorption.
    pub inverted_merged: usize,
    /// SAT queries that returned UNSAT (proven equivalences).
    pub sat_unsat_proofs: usize,
    /// SAT queries that returned SAT (counterexamples used to refine simulation classes).
    pub sat_counterexamples: usize,
}

impl FraigSweepStats {
    /// Total LUT cells eliminated by FRAIG SAT sweeping.
    #[inline]
    pub const fn total_merged(&self) -> usize {
        self.consts_merged + self.equiv_merged + self.inverted_merged
    }
}

/// Statistics returned by [`rewrite_aig_4cut`] / [`Design::rewrite_aig_4cut`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rewrite4CutStats {
    pub luts_before: usize,
    pub luts_after: usize,
    pub depth_before: usize,
    pub depth_after: usize,
    pub cones_rewritten: usize,
}

/// Candidate functional equivalence class from 64-bit parallel simulation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateEquivClass {
    /// Canonical 64-bit signature (normalized so bit 0 is `0`).
    pub canonical_signature: u64,
    /// Member nets `(net_spur, is_complemented_relative_to_canonical)` in topological order.
    pub members: Vec<(Spur, bool)>,
}

/// Evaluate a `Lut6` truth table `init` across 64 parallel input bit-vectors `in_words[0..6]`
/// in $O(1)$ bitwise operations using Shannon cofactor reduction.
#[inline]
pub fn eval_lut6_parallel_u64(init: u64, in_words: &[u64; 6]) -> u64 {
    if init == 0 {
        return 0;
    }
    if init == u64::MAX {
        return u64::MAX;
    }
    let mut words = [0u64; 64];
    for idx in 0..64usize {
        words[idx] = if ((init >> idx) & 1) != 0 {
            u64::MAX
        } else {
            0
        };
    }
    for var in 0..6usize {
        let sel = in_words[var];
        let half = 1usize << (5 - var);
        for i in 0..half {
            words[i] = (words[2 * i] & !sel) | (words[2 * i + 1] & sel);
        }
    }
    words[0]
}

/// Generate a deterministic 64-bit simulation pattern for primary input index `idx` and seed `seed`.
///
/// Pattern bit 0 is always `0` (the all-zero input vector), bits 1..7 are structured
/// Walsh-Hadamard basis patterns for fast small-input coverage, and higher bits use
/// SplitMix64 pseudorandom bit-vectors.
pub fn default_pi_pattern(idx: usize, seed: u64) -> u64 {
    let base = match idx % 6 {
        0 => FRAIG_VAR_INIT[0],
        1 => FRAIG_VAR_INIT[1],
        2 => FRAIG_VAR_INIT[2],
        3 => FRAIG_VAR_INIT[3],
        4 => FRAIG_VAR_INIT[4],
        _ => FRAIG_VAR_INIT[5],
    };
    let mut z = seed
        .wrapping_add((idx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    let rand_bits = z ^ (z >> 31);
    // Keep low 8 bits from structured basis (with bit 0 == 0) and upper 56 bits randomized.
    ((base & 0xFF) | (rand_bits & !0xFFu64)) & !1u64
}

/// Push an inversion on `input` into a LUT6 `init` mask (`addr` bit `input` flipped).
#[inline]
fn absorb_init_invert(init: u64, input: usize) -> u64 {
    let mut out = 0u64;
    for addr in 0..64u64 {
        out |= ((init >> (addr ^ (1u64 << input))) & 1) << addr;
    }
    out
}

/// Push a constant value `val` on `input` into a LUT6 `init` mask.
#[inline]
fn fold_init_const(init: u64, input: usize, val: bool) -> u64 {
    let mut out = 0u64;
    for addr in 0..64u64 {
        let fixed = if val {
            addr | (1u64 << input)
        } else {
            addr & !(1u64 << input)
        };
        out |= ((init >> fixed) & 1) << addr;
    }
    out
}

#[inline]
fn lut6_input_dont_care(init: u64, input: usize) -> bool {
    let shift = 1usize << input;
    let mask = COFACTOR_LO_MASK[input];
    (init & mask) == ((init >> shift) & mask)
}

/// Simulate 64 input patterns in parallel across `design` and return the 64-bit signature
/// for every net in the design.
pub fn simulate_parallel_64(
    design: &Design,
    pi_patterns: &HashMap<Spur, u64>,
) -> HashMap<Spur, u64> {
    let mut sigs: HashMap<Spur, u64> = pi_patterns.clone();
    sigs.insert(intern("0"), 0);
    sigs.insert(intern("1"), u64::MAX);
    sigs.insert(intern("1'b0"), 0);
    sigs.insert(intern("1'b1"), u64::MAX);

    let i_pins = [
        intern("I0"),
        intern("I1"),
        intern("I2"),
        intern("I3"),
        intern("I4"),
        intern("I5"),
    ];
    let o_pin = intern("O");
    let pad_pin = intern("PAD");
    let i_pin = intern("I");

    for c in &design.cells {
        if matches!(c.kind, CellKind::IobIn) {
            if let (Some(pad), Some(o)) = (design.net_on(c.name, pad_pin), design.net_on(c.name, o_pin)) {
                let v = sigs.get(&pad).copied().unwrap_or(0);
                sigs.insert(o, v);
            }
        }
    }

    let max_rounds = design.cells.len() + 2;
    for _ in 0..max_rounds {
        let mut changed = false;
        for c in &design.cells {
            let CellKind::Lut6 { init } = c.kind else {
                continue;
            };
            let Some(out_net) = design.net_on(c.name, o_pin) else {
                continue;
            };
            let mut in_words = [0u64; 6];
            for (idx, &p_sp) in i_pins.iter().enumerate() {
                if let Some(in_net) = design.net_on(c.name, p_sp) {
                    in_words[idx] = sigs.get(&in_net).copied().unwrap_or(0);
                }
            }
            let out_word = eval_lut6_parallel_u64(init, &in_words);
            if sigs.insert(out_net, out_word) != Some(out_word) {
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for c in &design.cells {
        if matches!(c.kind, CellKind::IobOut) {
            if let (Some(i_net), Some(pad_net)) =
                (design.net_on(c.name, i_pin), design.net_on(c.name, pad_pin))
            {
                let v = sigs.get(&i_net).copied().unwrap_or(0);
                sigs.insert(pad_net, v);
            }
        }
    }

    sigs
}

/// Partition nets in `ordered_nets` into candidate functional equivalence classes (up to complement)
/// based on their 64-bit parallel simulation signatures.
pub fn partition_candidate_classes(
    ordered_nets: &[Spur],
    sigs: &HashMap<Spur, u64>,
) -> Vec<CandidateEquivClass> {
    let mut groups: BTreeMap<u64, Vec<(Spur, bool)>> = BTreeMap::new();
    for &net in ordered_nets {
        let raw = sigs.get(&net).copied().unwrap_or(0);
        let inv = (raw & 1) != 0;
        let canon = if inv { !raw } else { raw };
        groups.entry(canon).or_default().push((net, inv));
    }
    groups
        .into_iter()
        .filter(|(canon, members)| *canon == 0 || members.len() >= 2)
        .map(|(canonical_signature, members)| CandidateEquivClass {
            canonical_signature,
            members,
        })
        .collect()
}

// ============================================================================
// Embedded CDCL SAT Solver for FRAIG Miter Proof & Counterexample Extraction
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Lit(i32);

impl Lit {
    #[inline]
    const fn from_var(v: usize, pol: bool) -> Self {
        let idx = (v as i32) + 1;
        Self(if pol { idx } else { -idx })
    }
    #[inline]
    const fn var(self) -> usize {
        (self.0.unsigned_abs() as usize) - 1
    }
    #[inline]
    const fn is_pos(self) -> bool {
        self.0 > 0
    }
}

impl std::ops::Not for Lit {
    type Output = Self;
    #[inline]
    fn not(self) -> Self::Output {
        Self(-self.0)
    }
}

#[derive(Clone, Debug)]
struct TrailItem {
    lit: Lit,
    level: usize,
    reason: Option<usize>,
}

#[derive(Clone, Debug, Default)]
struct CdclSolver {
    num_vars: usize,
    clauses: Vec<Vec<Lit>>,
    unsat: bool,
    assigns: Vec<Option<bool>>,
    var_level: Vec<usize>,
    var_reason: Vec<Option<usize>>,
    phase: Vec<bool>,
    activity: Vec<f64>,
    var_inc: f64,
    const_false: Option<Lit>,
    const_true: Option<Lit>,
}

impl CdclSolver {
    fn new() -> Self {
        Self {
            var_inc: 1.0,
            ..Self::default()
        }
    }

    fn new_lit(&mut self) -> Lit {
        let v = self.num_vars;
        self.num_vars += 1;
        self.assigns.push(None);
        self.var_level.push(0);
        self.var_reason.push(None);
        self.phase.push(false);
        self.activity.push(0.0);
        Lit::from_var(v, true)
    }

    fn const_lit(&mut self, val: bool) -> Lit {
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

    fn add_clause(&mut self, lits: &[Lit]) {
        if self.unsat {
            return;
        }
        let mut clean = lits.to_vec();
        clean.sort_unstable_by_key(|l| l.0);
        clean.dedup();
        for &l in &clean {
            if clean.binary_search_by_key(&(-l.0), |x| x.0).is_ok() {
                return;
            }
        }
        if clean.is_empty() {
            self.unsat = true;
            return;
        }
        for &l in &clean {
            self.activity[l.var()] += 1.0;
        }
        self.clauses.push(clean);
    }

    #[inline]
    fn eval_lit(&self, l: Lit) -> Option<bool> {
        self.assigns[l.var()].map(|v| if l.is_pos() { v } else { !v })
    }

    fn assign(&mut self, lit: Lit, level: usize, reason: Option<usize>, trail: &mut Vec<TrailItem>) {
        let v = lit.var();
        let val = lit.is_pos();
        self.assigns[v] = Some(val);
        self.var_level[v] = level;
        self.var_reason[v] = reason;
        self.phase[v] = val;
        trail.push(TrailItem { lit, level, reason });
    }

    fn backtrack(&mut self, target_level: usize, trail: &mut Vec<TrailItem>) {
        while let Some(last) = trail.last() {
            if last.level <= target_level {
                break;
            }
            let item = trail.pop().unwrap();
            let v = item.lit.var();
            self.assigns[v] = None;
            self.var_level[v] = 0;
            self.var_reason[v] = None;
        }
    }

    fn propagate(&mut self, level: usize, trail: &mut Vec<TrailItem>) -> Option<usize> {
        loop {
            let mut any_unit = false;
            for ci in 0..self.clauses.len() {
                let clause = &self.clauses[ci];
                let mut sat = false;
                let mut unass_cnt = 0usize;
                let mut last_unass = Lit(1);
                for &lit in clause {
                    match self.eval_lit(lit) {
                        Some(true) => {
                            sat = true;
                            break;
                        }
                        Some(false) => {}
                        None => {
                            unass_cnt += 1;
                            last_unass = lit;
                            if unass_cnt > 1 {
                                break;
                            }
                        }
                    }
                }
                if sat {
                    continue;
                }
                if unass_cnt == 0 {
                    return Some(ci);
                }
                if unass_cnt == 1 {
                    self.assign(last_unass, level, Some(ci), trail);
                    any_unit = true;
                }
            }
            if !any_unit {
                return None;
            }
        }
    }

    fn analyze_conflict(
        &mut self,
        conflict_ci: usize,
        current_level: usize,
        trail: &[TrailItem],
    ) -> (Vec<Lit>, usize) {
        let mut seen = vec![false; self.num_vars];
        let mut learned: Vec<Lit> = Vec::new();
        let mut path_c = 0usize;
        let mut reason_lits = self.clauses[conflict_ci].clone();
        let mut t_idx = trail.len();

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
            let uip_item = loop {
                t_idx -= 1;
                let it = &trail[t_idx];
                if seen[it.lit.var()] && it.level == current_level {
                    break it.clone();
                }
            };
            path_c -= 1;
            if path_c == 0 {
                learned.insert(0, !uip_item.lit);
                break;
            }
            if let Some(r_ci) = uip_item.reason {
                reason_lits = self.clauses[r_ci]
                    .iter()
                    .copied()
                    .filter(|l| l.var() != uip_item.lit.var())
                    .collect();
            } else {
                learned.insert(0, !uip_item.lit);
                break;
            }
        }
        self.var_inc *= 1.05;
        let bt_level = learned
            .iter()
            .skip(1)
            .map(|l| self.var_level[l.var()])
            .max()
            .unwrap_or(0);
        (learned, bt_level)
    }

    /// Solve under `assumptions`; returns `Some(model)` if SAT, `None` if UNSAT.
    fn solve_under(&mut self, assumptions: &[Lit]) -> Option<Vec<bool>> {
        if self.unsat {
            return None;
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

        let mut trail = Vec::with_capacity(self.num_vars);
        if self.propagate(0, &mut trail).is_some() {
            self.unsat = true;
            return None;
        }

        let mut level = 0usize;
        for &a_lit in assumptions {
            match self.eval_lit(a_lit) {
                Some(true) => {}
                Some(false) => {
                    self.backtrack(0, &mut trail);
                    return None;
                }
                None => {
                    level += 1;
                    self.assign(a_lit, level, None, &mut trail);
                    if self.propagate(level, &mut trail).is_some() {
                        self.backtrack(0, &mut trail);
                        return None;
                    }
                }
            }
        }
        let base_level = level;

        loop {
            if let Some(conf_ci) = self.propagate(level, &mut trail) {
                if level <= base_level {
                    self.backtrack(0, &mut trail);
                    return None;
                }
                let (learned, bt_lvl) = self.analyze_conflict(conf_ci, level, &trail);
                let uip = learned[0];
                let target = bt_lvl.max(base_level);
                self.backtrack(target, &mut trail);
                level = target;
                if learned.len() == 1 {
                    self.backtrack(0, &mut trail);
                    self.add_clause(&learned);
                    return self.solve_under(assumptions);
                } else {
                    let ci = self.clauses.len();
                    self.clauses.push(learned);
                    self.assign(uip, level, Some(ci), &mut trail);
                }
                continue;
            }

            let mut next_var = None;
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
                return Some(model);
            };
            level += 1;
            let dec = Lit::from_var(v, self.phase[v]);
            self.assign(dec, level, None, &mut trail);
        }
    }
}

fn encode_lut6_into_cdcl(solver: &mut CdclSolver, mut init: u64, inputs: [Option<Lit>; 6]) -> Lit {
    for i in 0..6 {
        if inputs[i].is_none() && !lut6_input_dont_care(init, i) {
            init = fold_init_const(init, i, false);
        }
    }
    if init == 0 {
        return solver.const_lit(false);
    }
    if init == u64::MAX {
        return solver.const_lit(true);
    }

    let mut active: Vec<(usize, Lit)> = Vec::with_capacity(6);
    for (i, inp_opt) in inputs.iter().enumerate() {
        if let Some(lit) = *inp_opt {
            if !lut6_input_dont_care(init, i) {
                active.push((i, lit));
            }
        }
    }
    if active.is_empty() {
        return solver.const_lit((init & 1) != 0);
    }
    if active.len() == 1 {
        let (pin, lit) = active[0];
        let v0 = (init & 1) != 0;
        let v1 = ((init >> (1usize << pin)) & 1) != 0;
        return if !v0 && v1 {
            lit
        } else if v0 && !v1 {
            !lit
        } else {
            solver.const_lit(v0)
        };
    }

    let m = active.len();
    let out = solver.new_lit();
    for sub in 0..(1usize << m) {
        let mut full_addr = 0u64;
        let mut clause = Vec::with_capacity(m + 1);
        for (bit_idx, &(orig_pin, lit)) in active.iter().enumerate() {
            let bit_one = ((sub >> bit_idx) & 1) != 0;
            if bit_one {
                full_addr |= 1u64 << orig_pin;
                clause.push(!lit);
            } else {
                clause.push(lit);
            }
        }
        let val_one = ((init >> full_addr) & 1) != 0;
        clause.push(if val_one { out } else { !out });
        solver.add_clause(&clause);
    }
    out
}

/// Run ABC `&fraig`-style SAT-sweeping functional reduction (`fraig_sat_sweep`) on `design`.
pub fn fraig_sat_sweep(design: &mut Design) -> FraigSweepStats {
    design.fraig_sat_sweep()
}

/// Run 4-input cut DAG-aware AIG/LUT rewriting (`rewrite_aig_4cut`) on `design`.
pub fn rewrite_aig_4cut(design: &mut Design) -> Rewrite4CutStats {
    design.rewrite_aig_4cut()
}

impl Design {
    /// SAT-Guided FRAIG Sweeping (`fraig_sat_sweep`).
    ///
    /// 1. Simulates 64 parallel input patterns (`u64` bitwise signatures) across all combinational cones.
    /// 2. Partitions LUT outputs into candidate equivalence classes (up to bitwise complement).
    /// 3. Proves or refutes candidate equivalences ($v \equiv 0/1$, $v \equiv u$, $v \equiv \neg u$)
    ///    using Tseitin CNF miter encoding and CDCL SAT solving:
    ///    - If UNSAT: merges $v \to u$ (or pushes $\neg u$ into fanout LUT `INIT`s) and removes $v$.
    ///    - If SAT: injects the counterexample vector into the 64-bit simulation pattern table
    ///      to immediately refine remaining candidate classes.
    pub fn fraig_sat_sweep(&mut self) -> FraigSweepStats {
        self.rebuild_indexes();
        let mut stats = FraigSweepStats::default();
        let i_pins = [
            intern("I0"),
            intern("I1"),
            intern("I2"),
            intern("I3"),
            intern("I4"),
            intern("I5"),
        ];
        let o_pin = intern("O");

        // Collect primary input / FF Q leaf nets and assign initial 64-bit parallel patterns.
        let leaf_nets = self.collect_comb_leaf_nets();
        let mut pi_patterns: HashMap<Spur, u64> = HashMap::with_capacity(leaf_nets.len());
        for (idx, &net) in leaf_nets.iter().enumerate() {
            pi_patterns.insert(net, default_pi_pattern(idx, 0xCAFE_BABE_1234_5678));
        }
        let mut cex_bit_cursor = 1usize; // bit 0 is reserved for all-zero vector

        for _round in 0..8 {
            self.rebuild_indexes();
            let (topo_luts, lut_out_net, lut_raw_ins) = self.extract_topo_luts();
            if topo_luts.is_empty() {
                break;
            }

            let ila = self.collect_guarded_nets();
            let mut sigs = simulate_parallel_64(self, &pi_patterns);

            // Build incremental CDCL solver over the current topological LUT DAG.
            let mut solver = CdclSolver::new();
            let mut net_lits: HashMap<Spur, Lit> = HashMap::new();
            net_lits.insert(intern("0"), solver.const_lit(false));
            net_lits.insert(intern("1"), solver.const_lit(true));
            net_lits.insert(intern("1'b0"), solver.const_lit(false));
            net_lits.insert(intern("1'b1"), solver.const_lit(true));
            for &lf in &leaf_nets {
                net_lits.entry(lf).or_insert_with(|| solver.new_lit());
            }

            for &ci in &topo_luts {
                let Some(out_net) = lut_out_net[ci] else {
                    continue;
                };
                let CellKind::Lut6 { init } = self.cells[ci].kind else {
                    continue;
                };
                let mut in_lits = [None; 6];
                for k in 0..6 {
                    if let Some(in_net) = lut_raw_ins[ci][k] {
                        let l = *net_lits
                            .entry(in_net)
                            .or_insert_with(|| solver.new_lit());
                        in_lits[k] = Some(l);
                    }
                }
                let out_lit = encode_lut6_into_cdcl(&mut solver, init, in_lits);
                net_lits.insert(out_net, out_lit);
            }

            // Include leaf nets as potential representatives before topological LUT outputs!
            let mut ordered_candidates: Vec<Spur> = Vec::with_capacity(leaf_nets.len() + topo_luts.len());
            for &lf in &leaf_nets {
                ordered_candidates.push(lf);
            }
            let mut net_to_lut_ci: HashMap<Spur, usize> = HashMap::with_capacity(topo_luts.len());
            for &ci in &topo_luts {
                if let Some(out_net) = lut_out_net[ci] {
                    net_to_lut_ci.insert(out_net, ci);
                    if !ordered_candidates.contains(&out_net) {
                        ordered_candidates.push(out_net);
                    }
                }
            }

            let mut did_merge = false;

            // Step A: Check LUT outputs with constant signature (`sig == 0` or `sig == u64::MAX`).
            for &ci in &topo_luts {
                let Some(out_net) = lut_out_net[ci] else {
                    continue;
                };
                if self.cells[ci].attrs.flag("DONT_TOUCH") || ila.contains(&out_net) {
                    continue;
                }
                let sig = sigs.get(&out_net).copied().unwrap_or(0);
                if sig != 0 && sig != u64::MAX {
                    continue;
                }
                let const_val = sig != 0;
                // Only fold constant into fanouts if all loads on `out_net` are movable LUT6 input pins.
                if !self.all_loads_are_movable_lut_inputs(ci, out_net) {
                    continue;
                }
                let out_lit = net_lits[&out_net];
                // Query if `out_net != const_val` is satisfiable:
                let violate_lit = if const_val { !out_lit } else { out_lit };
                match solver.solve_under(&[violate_lit]) {
                    None => {
                        // Proven constant across all 2^N inputs!
                        stats.sat_unsat_proofs += 1;
                        self.fold_const_net_into_lut_fanouts(ci, out_net, const_val);
                        stats.consts_merged += 1;
                        did_merge = true;
                        break;
                    }
                    Some(model) => {
                        stats.sat_counterexamples += 1;
                        inject_cex_into_pi_patterns(
                            &leaf_nets,
                            &net_lits,
                            &model,
                            &mut pi_patterns,
                            &mut cex_bit_cursor,
                        );
                        sigs = simulate_parallel_64(self, &pi_patterns);
                    }
                }
            }
            if did_merge {
                self.sweep_dead_luts();
                continue;
            }

            // Step B: Check candidate equivalence classes `(u, v)` in topological order.
            'class_search: for idx_v in 0..ordered_candidates.len() {
                let net_v = ordered_candidates[idx_v];
                let Some(&ci_v) = net_to_lut_ci.get(&net_v) else {
                    continue; // `net_v` is a primary input / FF Q, not a removable LUT output
                };
                if self.cells[ci_v].attrs.flag("DONT_TOUCH") || ila.contains(&net_v) {
                    continue;
                }
                let sig_v = sigs.get(&net_v).copied().unwrap_or(0);

                for &net_u in &ordered_candidates[..idx_v] {
                    if net_u == net_v {
                        continue;
                    }
                    let sig_u = sigs.get(&net_u).copied().unwrap_or(0);
                    let is_direct_cand = sig_u == sig_v;
                    let is_inv_cand = sig_u == !sig_v;
                    if !is_direct_cand && !is_inv_cand {
                        continue;
                    }
                    if is_inv_cand && !self.all_loads_are_movable_lut_inputs(ci_v, net_v) {
                        continue;
                    }

                    let lit_u = net_lits[&net_u];
                    let lit_v = net_lits[&net_v];
                    // Miter check: can `u` and `v` disagree with the candidate relation?
                    // For direct (`u == v`): test `(u & ~v)` or `(~u & v)`.
                    // For inverted (`v == ~u`): test `(u & v)` or `(~u & ~v)`.
                    let cex_model = if is_direct_cand {
                        solver
                            .solve_under(&[lit_u, !lit_v])
                            .or_else(|| solver.solve_under(&[!lit_u, lit_v]))
                    } else {
                        solver
                            .solve_under(&[lit_u, lit_v])
                            .or_else(|| solver.solve_under(&[!lit_u, !lit_v]))
                    };

                    match cex_model {
                        None => {
                            stats.sat_unsat_proofs += 1;
                            if is_direct_cand {
                                self.replace_lut_output_with_net(ci_v, net_v, net_u);
                                stats.equiv_merged += 1;
                            } else {
                                self.replace_lut_output_with_inverted_net(ci_v, net_v, net_u);
                                stats.inverted_merged += 1;
                            }
                            let _ = (i_pins, o_pin);
                            did_merge = true;
                            break 'class_search;
                        }
                        Some(model) => {
                            stats.sat_counterexamples += 1;
                            inject_cex_into_pi_patterns(
                                &leaf_nets,
                                &net_lits,
                                &model,
                                &mut pi_patterns,
                                &mut cex_bit_cursor,
                            );
                            sigs = simulate_parallel_64(self, &pi_patterns);
                        }
                    }
                }
            }

            if did_merge {
                self.sweep_dead_luts();
            } else {
                break;
            }
        }

        self.sweep_dead_luts();
        stats
    }

    /// 4-Input Cut DAG-Aware AIG/LUT Rewriting (`rewrite_aig_4cut`).
    ///
    /// Enumerates 4-input cuts ($K \le 4$) across LUT cones in topological order:
    /// 1. When a multi-level LUT cone has a 4-feasible cut over $\le 4$ distinct inputs
    ///    that absorbs intermediate single-fanout LUTs (reducing net/LUT count without
    ///    increasing logic depth), rewrites the root LUT directly to the 4-input cut's
    ///    canonical truth table.
    /// 2. Canonicalizes every 4-input LUT's input order and shares duplicate 4-input
    ///    subgraphs across the DAG.
    pub fn rewrite_aig_4cut(&mut self) -> Rewrite4CutStats {
        self.rebuild_indexes();
        let luts_before = self
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count();
        let depth_before = self.fraig_max_lut_depth();
        let mut cones_rewritten = 0usize;

        let i_pins = [
            intern("I0"),
            intern("I1"),
            intern("I2"),
            intern("I3"),
            intern("I4"),
            intern("I5"),
        ];
        let o_pin = intern("O");

        for _pass in 0..8 {
            self.rebuild_indexes();
            let ila = self.collect_guarded_nets();
            let (topo_luts, lut_out_net, lut_raw_ins) = self.extract_topo_luts();
            let mut changed_in_pass = false;

            // Map net -> driving LUT cell index for movable LUTs.
            let mut net_to_lut: HashMap<Spur, usize> = HashMap::with_capacity(topo_luts.len());
            for &ci in &topo_luts {
                if let Some(out_net) = lut_out_net[ci] {
                    if !self.cells[ci].attrs.flag("DONT_TOUCH")
                        && self.cells[ci].attrs.get("PRIM").is_none()
                    {
                        net_to_lut.insert(out_net, ci);
                    }
                }
            }

            // Try 4-cut absorption of upstream LUTs `ci_a -> ci_b` where combined active inputs <= 4.
            for &ci_b in &topo_luts {
                if self.cells[ci_b].attrs.flag("DONT_TOUCH")
                    || self.cells[ci_b].attrs.get("PRIM").is_none() == false
                {
                    continue;
                }
                let CellKind::Lut6 { init: init_b } = self.cells[ci_b].kind else {
                    continue;
                };
                let (c_init_b, c_ins_b, k_b) =
                    canonicalize_small_lut(init_b, lut_raw_ins[ci_b]);

                // Look for any fanin `net_a` of `ci_b` driven by a movable LUT `ci_a` such that
                // merging `ci_a` into `ci_b` yields a <= 4-input cut and either `ci_a` is single-fanout
                // (so LUT count strictly drops) or `ci_b` absorbs multiple fanins!
                let mut best_4cut: Option<(u64, Vec<Spur>)> = None;
                for pin_b in 0..(k_b as usize) {
                    let net_a = c_ins_b[pin_b].unwrap();
                    let Some(&ci_a) = net_to_lut.get(&net_a) else {
                        continue;
                    };
                    if ci_a == ci_b || ila.contains(&net_a) {
                        continue;
                    }
                    let Some(&net_a_ni) = self.net_ix.get(&net_a) else {
                        continue;
                    };
                    // Check that `net_a` only fans out to `ci_b` so replacing `ci_b` with the 4-cut saves a LUT!
                    let cell_a_name = self.cells[ci_a].name;
                    let cell_b_name = self.cells[ci_b].name;
                    let single_fanout_to_b = self.nets[net_a_ni].endpoints.iter().all(|e| {
                        (e.cell == cell_a_name && e.pin == o_pin)
                            || (e.cell == cell_b_name && i_pins.contains(&e.pin))
                    });
                    if !single_fanout_to_b {
                        continue;
                    }

                    let CellKind::Lut6 { init: init_a } = self.cells[ci_a].kind else {
                        continue;
                    };
                    let (c_init_a, c_ins_a, k_a) =
                        canonicalize_small_lut(init_a, lut_raw_ins[ci_a]);

                    let mut union_leaves: Vec<Spur> = Vec::with_capacity(4);
                    let mut fits_4cut = true;
                    for net_opt in c_ins_b[..k_b as usize]
                        .iter()
                        .chain(c_ins_a[..k_a as usize].iter())
                    {
                        let Some(n) = *net_opt else { continue };
                        if n == net_a {
                            continue;
                        }
                        if !union_leaves.contains(&n) {
                            if union_leaves.len() == 4 {
                                fits_4cut = false;
                                break;
                            }
                            union_leaves.push(n);
                        }
                    }
                    if !fits_4cut {
                        continue;
                    }
                    union_leaves.sort_unstable();

                    // Compose 64-bit truth table over `union_leaves` (which has <= 4 leaves).
                    let mut in_words_a = [0u64; 6];
                    for (pa, net_opt) in c_ins_a[..k_a as usize].iter().enumerate() {
                        if let Some(n) = *net_opt {
                            if let Some(pos) = union_leaves.iter().position(|&u| u == n) {
                                in_words_a[pa] = FRAIG_VAR_INIT[pos];
                            }
                        }
                    }
                    let tt_a = eval_lut6_parallel_u64(c_init_a, &in_words_a);

                    let mut in_words_b = [0u64; 6];
                    for (pb, net_opt) in c_ins_b[..k_b as usize].iter().enumerate() {
                        if let Some(n) = *net_opt {
                            if n == net_a {
                                in_words_b[pb] = tt_a;
                            } else if let Some(pos) = union_leaves.iter().position(|&u| u == n) {
                                in_words_b[pb] = FRAIG_VAR_INIT[pos];
                            }
                        }
                    }
                    let composed = eval_lut6_parallel_u64(c_init_b, &in_words_b);
                    best_4cut = Some((composed, union_leaves));
                    break;
                }

                if let Some((composed_init, union_leaves)) = best_4cut {
                    let mut raw_ins = [None; 6];
                    for (i, &sp) in union_leaves.iter().enumerate() {
                        raw_ins[i] = Some(sp);
                    }
                    let (final_init, final_ins, _) = canonicalize_small_lut(composed_init, raw_ins);
                    if let CellKind::Lut6 { init } = &mut self.cells[ci_b].kind {
                        *init = final_init;
                    }
                    self.rewire_lut_inputs(ci_b, &final_ins);
                    self.sweep_dead_luts();
                    cones_rewritten += 1;
                    changed_in_pass = true;
                    break;
                }
            }

            if !changed_in_pass {
                break;
            }
        }

        // Canonicalize all <=4-input LUT cones first, then share identical 4-cuts across the DAG.
        self.rebuild_indexes();
        let (topo_luts, _, lut_raw_ins) = self.extract_topo_luts();
        for &ci in &topo_luts {
            if self.cells[ci].attrs.flag("DONT_TOUCH")
                || self.cells[ci].attrs.get("PRIM").is_some()
            {
                continue;
            }
            let CellKind::Lut6 { init } = self.cells[ci].kind else {
                continue;
            };
            let (c_init, c_ins, k) = canonicalize_small_lut(init, lut_raw_ins[ci]);
            if k <= 4 {
                if let CellKind::Lut6 { init: mut_init } = &mut self.cells[ci].kind {
                    *mut_init = c_init;
                }
                if c_ins != lut_raw_ins[ci] {
                    self.rewire_lut_inputs(ci, &c_ins);
                }
            }
        }

        self.rebuild_indexes();
        let ila = self.collect_guarded_nets();
        let (topo_luts, lut_out_net, lut_raw_ins) = self.extract_topo_luts();
        let mut seen_4cuts: HashMap<(u64, [Option<Spur>; 6]), Spur> = HashMap::new();
        for &ci in &topo_luts {
            let Some(out_net) = lut_out_net[ci] else {
                continue;
            };
            if self.cells[ci].attrs.flag("DONT_TOUCH")
                || self.cells[ci].attrs.get("PRIM").is_some()
            {
                continue;
            }
            let CellKind::Lut6 { init } = self.cells[ci].kind else {
                continue;
            };
            let (c_init, c_ins, k) = canonicalize_small_lut(init, lut_raw_ins[ci]);
            if k <= 4 {
                let key = (c_init, c_ins);
                if let Some(&rep_net) = seen_4cuts.get(&key) {
                    if !ila.contains(&out_net) && rep_net != out_net {
                        self.replace_lut_output_with_net(ci, out_net, rep_net);
                        cones_rewritten += 1;
                    }
                } else {
                    seen_4cuts.insert(key, out_net);
                }
            }
        }
        self.sweep_dead_luts();

        let luts_after = self
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count();
        let depth_after = self.fraig_max_lut_depth();

        Rewrite4CutStats {
            luts_before,
            luts_after,
            depth_before,
            depth_after,
            cones_rewritten,
        }
    }

    fn collect_guarded_nets(&self) -> HashSet<Spur> {
        let mut g: HashSet<Spur> = self
            .cells
            .iter()
            .filter_map(|c| match c.kind {
                CellKind::Ila { net } => Some(net),
                _ => None,
            })
            .collect();
        for p in &self.ports {
            if matches!(p.dir, PortDir::Out | PortDir::Inout) {
                g.insert(p.name);
            }
        }
        for n in &self.nets {
            if n.attrs.flag("mark_debug") {
                g.insert(n.name);
            }
        }
        g
    }

    fn collect_comb_leaf_nets(&self) -> Vec<Spur> {
        let o_pin = intern("O");
        let mut lut_driven: HashSet<Spur> = HashSet::new();
        for net in &self.nets {
            for ep in &net.endpoints {
                if let Some(&ci) = self.cell_ix.get(&ep.cell) {
                    if matches!(self.cells[ci].kind, CellKind::Lut6 { .. }) && ep.pin == o_pin {
                        lut_driven.insert(net.name);
                    }
                }
            }
        }
        let mut leaves: Vec<Spur> = Vec::new();
        for p in &self.ports {
            if matches!(p.dir, PortDir::In | PortDir::Inout) && !lut_driven.contains(&p.name) {
                leaves.push(p.name);
            }
        }
        for net in &self.nets {
            if !lut_driven.contains(&net.name) && !leaves.contains(&net.name) {
                leaves.push(net.name);
            }
        }
        leaves
    }

    fn extract_topo_luts(&self) -> (Vec<usize>, Vec<Option<Spur>>, Vec<[Option<Spur>; 6]>) {
        let i_pins = [
            intern("I0"),
            intern("I1"),
            intern("I2"),
            intern("I3"),
            intern("I4"),
            intern("I5"),
        ];
        let o_pin = intern("O");
        let n = self.cells.len();
        let mut lut_out_net = vec![None; n];
        let mut lut_raw_ins = vec![[None; 6]; n];
        let mut net_drv: HashMap<Spur, usize> = HashMap::new();

        for net in &self.nets {
            for ep in &net.endpoints {
                let Some(&ci) = self.cell_ix.get(&ep.cell) else {
                    continue;
                };
                if !matches!(self.cells[ci].kind, CellKind::Lut6 { .. }) {
                    continue;
                }
                if ep.pin == o_pin {
                    lut_out_net[ci] = Some(net.name);
                    net_drv.insert(net.name, ci);
                } else {
                    for (idx, &p_sp) in i_pins.iter().enumerate() {
                        if ep.pin == p_sp {
                            lut_raw_ins[ci][idx] = Some(net.name);
                            break;
                        }
                    }
                }
            }
        }

        let mut topo = Vec::new();
        let mut state = vec![0u8; n];
        fn dfs(
            ci: usize,
            ins: &[[Option<Spur>; 6]],
            net_drv: &HashMap<Spur, usize>,
            state: &mut [u8],
            topo: &mut Vec<usize>,
        ) {
            if state[ci] != 0 {
                return;
            }
            state[ci] = 1;
            for net_opt in &ins[ci] {
                if let Some(net) = *net_opt {
                    if let Some(&d_ci) = net_drv.get(&net) {
                        dfs(d_ci, ins, net_drv, state, topo);
                    }
                }
            }
            state[ci] = 2;
            topo.push(ci);
        }

        for (ci, c) in self.cells.iter().enumerate() {
            if matches!(c.kind, CellKind::Lut6 { .. }) && lut_out_net[ci].is_some() && state[ci] == 0
            {
                dfs(ci, &lut_raw_ins, &net_drv, &mut state, &mut topo);
            }
        }
        (topo, lut_out_net, lut_raw_ins)
    }

    fn all_loads_are_movable_lut_inputs(&self, driver_ci: usize, net_spur: Spur) -> bool {
        let Some(&ni) = self.net_ix.get(&net_spur) else {
            return false;
        };
        let i_pins = [
            intern("I0"),
            intern("I1"),
            intern("I2"),
            intern("I3"),
            intern("I4"),
            intern("I5"),
        ];
        let o_pin = intern("O");
        let drv_name = self.cells[driver_ci].name;
        let mut has_load = false;
        for ep in &self.nets[ni].endpoints {
            if ep.cell == drv_name && ep.pin == o_pin {
                continue;
            }
            has_load = true;
            let Some(&ci) = self.cell_ix.get(&ep.cell) else {
                return false;
            };
            if ci == driver_ci
                || !matches!(self.cells[ci].kind, CellKind::Lut6 { .. })
                || self.cells[ci].attrs.flag("DONT_TOUCH")
                || !i_pins.contains(&ep.pin)
            {
                return false;
            }
        }
        has_load
    }

    fn fold_const_net_into_lut_fanouts(&mut self, driver_ci: usize, net_spur: Spur, val: bool) {
        let i_pins = [
            intern("I0"),
            intern("I1"),
            intern("I2"),
            intern("I3"),
            intern("I4"),
            intern("I5"),
        ];
        let o_pin = intern("O");
        let drv_name = self.cells[driver_ci].name;
        let Some(&ni) = self.net_ix.get(&net_spur) else {
            return;
        };
        let fanouts: Vec<(usize, usize, Spur, Spur)> = self.nets[ni]
            .endpoints
            .iter()
            .filter(|e| !(e.cell == drv_name && e.pin == o_pin))
            .filter_map(|e| {
                let ci = *self.cell_ix.get(&e.cell)?;
                let pin_i = i_pins.iter().position(|&p| p == e.pin)?;
                Some((ci, pin_i, e.cell, e.pin))
            })
            .collect();

        for &(ci, pin_i, _, _) in &fanouts {
            if let CellKind::Lut6 { init } = &mut self.cells[ci].kind {
                *init = fold_init_const(*init, pin_i, val);
            }
        }
        self.nets[ni]
            .endpoints
            .retain(|e| e.cell == drv_name && e.pin == o_pin);
        self.rebuild_indexes();
    }

    fn replace_lut_output_with_net(&mut self, dup_ci: usize, dup_net: Spur, rep_net: Spur) {
        let o_pin = intern("O");
        let dup_name = self.cells[dup_ci].name;
        let Some(&dup_ni) = self.net_ix.get(&dup_net) else {
            return;
        };
        let Some(&rep_ni) = self.net_ix.get(&rep_net) else {
            return;
        };
        if dup_ni == rep_ni {
            return;
        }
        let loads: Vec<Endpoint> = self.nets[dup_ni]
            .endpoints
            .iter()
            .filter(|e| !(e.cell == dup_name && e.pin == o_pin))
            .cloned()
            .collect();
        self.nets[dup_ni]
            .endpoints
            .retain(|e| e.cell == dup_name && e.pin == o_pin);
        self.nets[rep_ni].endpoints.extend(loads);
        self.rebuild_indexes();
    }

    fn replace_lut_output_with_inverted_net(
        &mut self,
        dup_ci: usize,
        dup_net: Spur,
        rep_net: Spur,
    ) {
        let i_pins = [
            intern("I0"),
            intern("I1"),
            intern("I2"),
            intern("I3"),
            intern("I4"),
            intern("I5"),
        ];
        let o_pin = intern("O");
        let dup_name = self.cells[dup_ci].name;
        let Some(&dup_ni) = self.net_ix.get(&dup_net) else {
            return;
        };
        let Some(&rep_ni) = self.net_ix.get(&rep_net) else {
            return;
        };
        if dup_ni == rep_ni {
            return;
        }
        let loads: Vec<Endpoint> = self.nets[dup_ni]
            .endpoints
            .iter()
            .filter(|e| !(e.cell == dup_name && e.pin == o_pin))
            .cloned()
            .collect();
        for ep in &loads {
            if let Some(&fo_ci) = self.cell_ix.get(&ep.cell) {
                if let Some(pin_i) = i_pins.iter().position(|&p| p == ep.pin) {
                    if let CellKind::Lut6 { init } = &mut self.cells[fo_ci].kind {
                        *init = absorb_init_invert(*init, pin_i);
                    }
                }
            }
        }
        self.nets[dup_ni]
            .endpoints
            .retain(|e| e.cell == dup_name && e.pin == o_pin);
        self.nets[rep_ni].endpoints.extend(loads);
        self.rebuild_indexes();
    }

    fn rewire_lut_inputs(&mut self, ci: usize, new_ins: &[Option<Spur>; 6]) {
        let i_pins = [
            intern("I0"),
            intern("I1"),
            intern("I2"),
            intern("I3"),
            intern("I4"),
            intern("I5"),
        ];
        let cell_name = self.cells[ci].name;
        for net in &mut self.nets {
            net.endpoints
                .retain(|e| !(e.cell == cell_name && i_pins.contains(&e.pin)));
        }
        self.rebuild_indexes();
        let cell_str = resolve(cell_name);
        for (idx, net_opt) in new_ins.iter().enumerate() {
            if let Some(sp) = *net_opt {
                let pin_str = match idx {
                    0 => "I0",
                    1 => "I1",
                    2 => "I2",
                    3 => "I3",
                    4 => "I4",
                    _ => "I5",
                };
                self.connect(resolve(sp), cell_str, pin_str);
            }
        }
    }

    fn fraig_max_lut_depth(&self) -> usize {
        let (topo, lut_out_net, lut_raw_ins) = self.extract_topo_luts();
        let mut net_depth: HashMap<Spur, usize> = HashMap::new();
        let mut max_d = 0usize;
        for ci in topo {
            let mut d_in = 0usize;
            for net_opt in &lut_raw_ins[ci] {
                if let Some(n) = *net_opt {
                    d_in = d_in.max(net_depth.get(&n).copied().unwrap_or(0));
                }
            }
            let d = 1 + d_in;
            if let Some(out_n) = lut_out_net[ci] {
                net_depth.insert(out_n, d);
            }
            max_d = max_d.max(d);
        }
        max_d
    }
}

fn inject_cex_into_pi_patterns(
    leaf_nets: &[Spur],
    net_lits: &HashMap<Spur, Lit>,
    model: &[bool],
    pi_patterns: &mut HashMap<Spur, u64>,
    bit_cursor: &mut usize,
) {
    let bit = (*bit_cursor % 63) + 1; // keep bit 0 == 0
    *bit_cursor += 1;
    let mask = 1u64 << bit;
    for &lf in leaf_nets {
        if let Some(&lit) = net_lits.get(&lf) {
            let val = model.get(lit.var()).copied().unwrap_or(false);
            let entry = pi_patterns.entry(lf).or_insert(0);
            if val {
                *entry |= mask;
            } else {
                *entry &= !mask;
            }
        }
    }
}

fn canonicalize_small_lut(
    mut init: u64,
    mut inputs: [Option<Spur>; 6],
) -> (u64, [Option<Spur>; 6], u8) {
    for i in 0..6 {
        if inputs[i].is_none() && !lut6_input_dont_care(init, i) {
            init = fold_init_const(init, i, false);
        }
    }
    for i in 0..6 {
        let Some(ni) = inputs[i] else { continue };
        for j in (i + 1)..6 {
            if inputs[j] == Some(ni) {
                let mut collapsed = 0u64;
                for addr in 0..64u64 {
                    let bit_i = (addr >> i) & 1;
                    let eff = (addr & !(1u64 << j)) | (bit_i << j);
                    collapsed |= ((init >> eff) & 1) << addr;
                }
                init = collapsed;
                inputs[j] = None;
            }
        }
    }
    let mut active: [(Spur, usize); 6] = [(Spur::default(), 0); 6];
    let mut m = 0usize;
    for (pin, net_opt) in inputs.iter().enumerate() {
        if let Some(n) = *net_opt {
            if !lut6_input_dont_care(init, pin) {
                active[m] = (n, pin);
                m += 1;
            }
        }
    }
    active[..m].sort_unstable_by_key(|(s, _)| *s);

    let mut canon_ins = [None; 6];
    for new_pos in 0..m {
        canon_ins[new_pos] = Some(active[new_pos].0);
    }

    let mut perm_init = 0u64;
    for new_addr in 0..64u64 {
        let mut old_addr = 0u64;
        for new_pos in 0..m {
            if ((new_addr >> new_pos) & 1) != 0 {
                old_addr |= 1u64 << active[new_pos].1;
            }
        }
        perm_init |= ((init >> old_addr) & 1) << new_addr;
    }
    (perm_init, canon_ins, m as u8)
}
