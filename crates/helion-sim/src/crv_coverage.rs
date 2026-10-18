//! IEEE 1800-2017 SystemVerilog Constrained-Random Stimulus Solver
//! ([`RandClass`], [`RandConstraint`], [`RandConstraintSolver`]) and Functional
//! Cross-Coverage Engine ([`CovergroupEngine`], [`CoverpointSpec`], [`CrossSpec`]).
//!
//! - **Constrained-Random Solver**: Supports `rand` and cyclic-permutation `randc`
//!   variables (guaranteed period without repetition before cycle exhaustion),
//!   interval domain pruning for `inside { [lo:hi], val }`, weighted distribution
//!   sampling (`dist { v1 := w1, [lo:hi] :/ w2 }`), implication (`cond -> body`),
//!   variable ordering (`solve a before b`), `unique { ... }`, and backtracking
//!   bit-vector constraint solving (`randomize_with_seed`).
//! - **Functional Coverage & Cross Engine**: Tracks explicit value bins, range bins,
//!   multi-step transition bins (`(s0 => s1 => s2)`), `ignore_bins`, `illegal_bins`
//!   (emitting violation records), and $N$-way Cartesian `cross` bins with
//!   `at_least` goal thresholds and coverage percentage reporting.

use crate::Sim;
use std::collections::{BTreeMap, HashMap, HashSet};

// ============================================================================
// Part 1: Constrained-Random Stimulus Solver (`RandConstraintSolver`)
// ============================================================================

/// Randomization mode for a class member variable (`rand` vs `randc`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RandMode {
    /// Standard uniform/weighted random variable (`rand`).
    Rand,
    /// Cyclic random variable (`randc`): permutes through all legal values in
    /// its domain without repeating any value until the cycle is exhausted.
    Randc,
}

/// Declaration of a randomizable variable inside a [`RandClass`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RandVarSpec {
    pub name: String,
    pub width: u8,
    pub mode: RandMode,
    pub rand_enabled: bool,
}

impl RandVarSpec {
    pub const fn max_value(&self) -> u64 {
        if self.width == 0 {
            0
        } else if self.width >= 64 {
            u64::MAX
        } else {
            (1u64 << self.width) - 1
        }
    }
}

/// Disjoint sorted closed intervals `[(lo, hi)]` representing a variable's pruned domain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomainIntervals {
    pub ranges: Vec<(u64, u64)>,
}

impl DomainIntervals {
    pub fn full_for_width(width: u8) -> Self {
        let max_v = if width == 0 {
            0
        } else if width >= 64 {
            u64::MAX
        } else {
            (1u64 << width) - 1
        };
        Self {
            ranges: vec![(0, max_v)],
        }
    }

    pub fn from_ranges(raw: &[(u64, u64)]) -> Self {
        let mut sorted: Vec<(u64, u64)> = raw
            .iter()
            .copied()
            .filter(|&(lo, hi)| lo <= hi)
            .collect();
        sorted.sort_unstable_by_key(|&(lo, _)| lo);
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(sorted.len());
        for (lo, hi) in sorted {
            if let Some(last) = merged.last_mut() {
                if lo <= last.1.saturating_add(1) {
                    last.1 = last.1.max(hi);
                    continue;
                }
            }
            merged.push((lo, hi));
        }
        Self { ranges: merged }
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn contains(&self, val: u64) -> bool {
        self.ranges
            .iter()
            .any(|&(lo, hi)| val >= lo && val <= hi)
    }

    pub fn intersect(&mut self, other: &Self) {
        let mut out = Vec::new();
        for &(a_lo, a_hi) in &self.ranges {
            for &(b_lo, b_hi) in &other.ranges {
                let lo = a_lo.max(b_lo);
                let hi = a_hi.min(b_hi);
                if lo <= hi {
                    out.push((lo, hi));
                }
            }
        }
        *self = Self::from_ranges(&out);
    }

    pub fn exclude(&mut self, excluded: &Self) {
        let mut cur = self.ranges.clone();
        for &(ex_lo, ex_hi) in &excluded.ranges {
            let mut next = Vec::new();
            for (lo, hi) in cur {
                if ex_hi < lo || ex_lo > hi {
                    next.push((lo, hi));
                } else {
                    if ex_lo > lo {
                        next.push((lo, ex_lo - 1));
                    }
                    if ex_hi < hi {
                        next.push((ex_hi + 1, hi));
                    }
                }
            }
            cur = next;
        }
        self.ranges = cur;
    }

    /// Total number of discrete integers in this domain.
    pub fn cardinality(&self) -> u128 {
        self.ranges
            .iter()
            .map(|&(lo, hi)| u128::from(hi - lo) + 1)
            .sum()
    }

    /// Return the `idx`-th value (`0..cardinality()`) in this domain.
    pub fn nth(&self, mut idx: u128) -> Option<u64> {
        for &(lo, hi) in &self.ranges {
            let len = u128::from(hi - lo) + 1;
            if idx < len {
                return Some(lo + (idx as u64));
            }
            idx -= len;
        }
        None
    }

    /// Enumerate up to `cap` values from this domain.
    pub fn enumerate_values(&self, cap: usize) -> Vec<u64> {
        let mut out = Vec::new();
        for &(lo, hi) in &self.ranges {
            for v in lo..=hi {
                out.push(v);
                if out.len() >= cap {
                    return out;
                }
            }
        }
        out
    }
}

/// Weight assignment mode for a SystemVerilog `dist` constraint item (`:=` vs `:/`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistWeightKind {
    /// `:= weight`: `weight` is applied to every individual value in `[lo:hi]`.
    PerValue(u64),
    /// `:/ weight`: `weight` is divided across the range `[lo:hi]` as a whole.
    PerRange(u64),
}

/// One weighted range or value entry in a `dist { ... }` constraint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DistItem {
    pub lo: u64,
    pub hi: u64,
    pub weight: DistWeightKind,
}

impl DistItem {
    pub const fn val_per_value(val: u64, weight: u64) -> Self {
        Self {
            lo: val,
            hi: val,
            weight: DistWeightKind::PerValue(weight),
        }
    }

    pub const fn range_per_value(lo: u64, hi: u64, weight: u64) -> Self {
        Self {
            lo,
            hi,
            weight: DistWeightKind::PerValue(weight),
        }
    }

    pub const fn range_per_range(lo: u64, hi: u64, weight: u64) -> Self {
        Self {
            lo,
            hi,
            weight: DistWeightKind::PerRange(weight),
        }
    }
}

/// Relational, arithmetic, bitwise, or boolean expression in a [`RandConstraint`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RandExpr {
    Const(u64),
    Var(String),
    Add(Box<RandExpr>, Box<RandExpr>),
    Sub(Box<RandExpr>, Box<RandExpr>),
    Mul(Box<RandExpr>, Box<RandExpr>),
    Div(Box<RandExpr>, Box<RandExpr>),
    Mod(Box<RandExpr>, Box<RandExpr>),
    BitAnd(Box<RandExpr>, Box<RandExpr>),
    BitOr(Box<RandExpr>, Box<RandExpr>),
    BitXor(Box<RandExpr>, Box<RandExpr>),
    Shl(Box<RandExpr>, Box<RandExpr>),
    Shr(Box<RandExpr>, Box<RandExpr>),
    Eq(Box<RandExpr>, Box<RandExpr>),
    Ne(Box<RandExpr>, Box<RandExpr>),
    Lt(Box<RandExpr>, Box<RandExpr>),
    Le(Box<RandExpr>, Box<RandExpr>),
    Gt(Box<RandExpr>, Box<RandExpr>),
    Ge(Box<RandExpr>, Box<RandExpr>),
    And(Box<RandExpr>, Box<RandExpr>),
    Or(Box<RandExpr>, Box<RandExpr>),
    Not(Box<RandExpr>),
}

impl RandExpr {
    pub fn var(name: impl Into<String>) -> Self {
        Self::Var(name.into())
    }

    pub const fn val(v: u64) -> Self {
        Self::Const(v)
    }

    /// Collect all variable names referenced by this expression.
    pub fn referenced_vars(&self, out: &mut HashSet<String>) {
        match self {
            Self::Const(_) => {}
            Self::Var(n) => {
                out.insert(n.clone());
            }
            Self::Not(x) => x.referenced_vars(out),
            Self::Add(a, b)
            | Self::Sub(a, b)
            | Self::Mul(a, b)
            | Self::Div(a, b)
            | Self::Mod(a, b)
            | Self::BitAnd(a, b)
            | Self::BitOr(a, b)
            | Self::BitXor(a, b)
            | Self::Shl(a, b)
            | Self::Shr(a, b)
            | Self::Eq(a, b)
            | Self::Ne(a, b)
            | Self::Lt(a, b)
            | Self::Le(a, b)
            | Self::Gt(a, b)
            | Self::Ge(a, b)
            | Self::And(a, b)
            | Self::Or(a, b) => {
                a.referenced_vars(out);
                b.referenced_vars(out);
            }
        }
    }

    /// Evaluate this expression under a partial variable assignment `env`.
    /// Returns `None` if a referenced variable is not yet assigned.
    pub fn eval(&self, env: &HashMap<String, u64>) -> Option<u64> {
        match self {
            Self::Const(v) => Some(*v),
            Self::Var(n) => env.get(n).copied(),
            Self::Not(x) => Some(u64::from(x.eval(env)? == 0)),
            Self::Add(a, b) => Some(a.eval(env)?.wrapping_add(b.eval(env)?)),
            Self::Sub(a, b) => Some(a.eval(env)?.wrapping_sub(b.eval(env)?)),
            Self::Mul(a, b) => Some(a.eval(env)?.wrapping_mul(b.eval(env)?)),
            Self::Div(a, b) => {
                let rhs = b.eval(env)?;
                if rhs == 0 {
                    Some(0)
                } else {
                    Some(a.eval(env)? / rhs)
                }
            }
            Self::Mod(a, b) => {
                let rhs = b.eval(env)?;
                if rhs == 0 {
                    Some(0)
                } else {
                    Some(a.eval(env)? % rhs)
                }
            }
            Self::BitAnd(a, b) => Some(a.eval(env)? & b.eval(env)?),
            Self::BitOr(a, b) => Some(a.eval(env)? | b.eval(env)?),
            Self::BitXor(a, b) => Some(a.eval(env)? ^ b.eval(env)?),
            Self::Shl(a, b) => Some(a.eval(env)?.wrapping_shl(b.eval(env)? as u32)),
            Self::Shr(a, b) => Some(a.eval(env)?.wrapping_shr(b.eval(env)? as u32)),
            Self::Eq(a, b) => Some(u64::from(a.eval(env)? == b.eval(env)?)),
            Self::Ne(a, b) => Some(u64::from(a.eval(env)? != b.eval(env)?)),
            Self::Lt(a, b) => Some(u64::from(a.eval(env)? < b.eval(env)?)),
            Self::Le(a, b) => Some(u64::from(a.eval(env)? <= b.eval(env)?)),
            Self::Gt(a, b) => Some(u64::from(a.eval(env)? > b.eval(env)?)),
            Self::Ge(a, b) => Some(u64::from(a.eval(env)? >= b.eval(env)?)),
            Self::And(a, b) => Some(u64::from(a.eval(env)? != 0 && b.eval(env)? != 0)),
            Self::Or(a, b) => Some(u64::from(a.eval(env)? != 0 || b.eval(env)? != 0)),
        }
    }
}

/// One SystemVerilog constraint clause inside a [`RandClass`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RandConstraint {
    /// Set membership: `var inside { [lo:hi], val }` (or `!(var inside { ... })` when `negated = true`).
    Inside {
        var: String,
        ranges: Vec<(u64, u64)>,
        negated: bool,
    },
    /// Weighted distribution: `var dist { v1 := w1, [lo:hi] :/ w2 }`.
    Dist {
        var: String,
        items: Vec<DistItem>,
    },
    /// Relational, arithmetic, or boolean constraint expression.
    Expr(RandExpr),
    /// Implication constraint: `cond -> { body }`.
    Implies {
        cond: RandExpr,
        body: Vec<RandConstraint>,
    },
    /// Variable solve ordering: `solve before_var before after_var;`.
    SolveBefore {
        before: String,
        after: String,
    },
    /// Pairwise distinctness: `unique { v1, v2, ... };`.
    Unique(Vec<String>),
}

/// Named constraint block on a [`RandClass`] (can be toggled via `constraint_mode`).
#[derive(Clone, Debug)]
pub struct NamedConstraintBlock {
    pub name: String,
    pub enabled: bool,
    pub clauses: Vec<RandConstraint>,
}

/// SystemVerilog randomizable class descriptor (`class Packet; rand bit [7:0] addr; ...`).
#[derive(Clone, Debug, Default)]
pub struct RandClass {
    pub name: String,
    pub vars: Vec<RandVarSpec>,
    pub constraints: Vec<NamedConstraintBlock>,
}

impl RandClass {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            vars: Vec::new(),
            constraints: Vec::new(),
        }
    }

    /// Declare a `rand` variable of bit-width `width` (`1..=64`).
    pub fn add_rand(&mut self, name: impl Into<String>, width: u8) -> &mut Self {
        self.vars.push(RandVarSpec {
            name: name.into(),
            width: width.clamp(1, 64),
            mode: RandMode::Rand,
            rand_enabled: true,
        });
        self
    }

    /// Declare a cyclic `randc` variable of bit-width `width` (`1..=16`).
    pub fn add_randc(&mut self, name: impl Into<String>, width: u8) -> &mut Self {
        self.vars.push(RandVarSpec {
            name: name.into(),
            width: width.clamp(1, 64),
            mode: RandMode::Randc,
            rand_enabled: true,
        });
        self
    }

    /// Add a named constraint block (`constraint c_range { ... }`).
    pub fn add_constraint(
        &mut self,
        block_name: impl Into<String>,
        clauses: Vec<RandConstraint>,
    ) -> &mut Self {
        self.constraints.push(NamedConstraintBlock {
            name: block_name.into(),
            enabled: true,
            clauses,
        });
        self
    }

    /// Enable or disable a named constraint block (SystemVerilog `constraint_mode(0|1)`).
    pub fn set_constraint_mode(&mut self, block_name: &str, enabled: bool) -> Result<(), String> {
        let block = self
            .constraints
            .iter_mut()
            .find(|b| b.name == block_name)
            .ok_or_else(|| format!("constraint_mode: unknown constraint block `{block_name}`"))?;
        block.enabled = enabled;
        Ok(())
    }

    /// Enable or disable randomization of a variable (SystemVerilog `rand_mode(0|1)`).
    pub fn set_rand_mode(&mut self, var_name: &str, enabled: bool) -> Result<(), String> {
        let v = self
            .vars
            .iter_mut()
            .find(|v| v.name == var_name)
            .ok_or_else(|| format!("rand_mode: unknown variable `{var_name}`"))?;
        v.rand_enabled = enabled;
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct XorShift64(u64);

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    #[inline]
    fn below(&mut self, n: u64) -> u64 {
        if n <= 1 {
            0
        } else {
            self.next_u64() % n
        }
    }
}

/// Stateful IEEE 1800 constrained-random solver maintaining `randc` cyclic permutation
/// histories and PRNG state across `randomize()` calls.
#[derive(Clone, Debug)]
pub struct RandConstraintSolver {
    pub class: RandClass,
    rng: XorShift64,
    /// Per-`randc`-variable set of values already emitted in the current permutation cycle.
    randc_used: HashMap<String, HashSet<u64>>,
    /// Current state of all variables in the class instance.
    pub state: HashMap<String, u64>,
}

impl RandConstraintSolver {
    pub fn new(class: RandClass, seed: u64) -> Self {
        let mut state = HashMap::new();
        for v in &class.vars {
            state.insert(v.name.clone(), 0);
        }
        Self {
            class,
            rng: XorShift64::new(seed),
            randc_used: HashMap::new(),
            state,
        }
    }

    /// Reseed the solver's PRNG (SystemVerilog `srandom(seed)`).
    pub fn srandom(&mut self, seed: u64) {
        self.rng = XorShift64::new(seed);
    }

    /// Randomize with an explicit seed (`srandom(seed)` followed by `randomize()`).
    pub fn randomize_with_seed(&mut self, seed: u64) -> Result<HashMap<String, u64>, String> {
        self.srandom(seed);
        self.randomize()
    }

    /// Solve all enabled constraints and randomize all `rand_enabled` variables.
    pub fn randomize(&mut self) -> Result<HashMap<String, u64>, String> {
        self.randomize_with(&[])
    }

    /// Randomize with inline extra constraints (`obj.randomize() with { ... }`).
    pub fn randomize_with(
        &mut self,
        inline_constraints: &[RandConstraint],
    ) -> Result<HashMap<String, u64>, String> {
        let mut active_clauses: Vec<RandConstraint> = self
            .class
            .constraints
            .iter()
            .filter(|b| b.enabled)
            .flat_map(|b| b.clauses.iter().cloned())
            .collect();
        active_clauses.extend_from_slice(inline_constraints);

        // 1. Build initial interval domains per variable and apply static unary interval pruning.
        let mut domains: HashMap<String, DomainIntervals> = HashMap::new();
        let mut dist_map: HashMap<String, Vec<DistItem>> = HashMap::new();
        for v in &self.class.vars {
            if v.rand_enabled {
                domains.insert(v.name.clone(), DomainIntervals::full_for_width(v.width));
            } else {
                let held = self.state.get(&v.name).copied().unwrap_or(0);
                domains.insert(
                    v.name.clone(),
                    DomainIntervals::from_ranges(&[(held, held)]),
                );
            }
        }

        Self::prune_static_domains(&active_clauses, &mut domains, &mut dist_map);
        for (vname, dom) in &domains {
            if dom.is_empty() {
                return Err(format!(
                    "randomize: variable `{vname}` has an empty domain after interval pruning"
                ));
            }
        }

        // 2. Determine variable solve order (`solve a before b` topological sort).
        let order = self.compute_solve_order(&active_clauses);

        // 3. Backtracking solver with forward checking, `dist` weighted sampling, and `randc` cyclic permutation.
        let mut env: HashMap<String, u64> = HashMap::new();
        for v in &self.class.vars {
            if !v.rand_enabled {
                env.insert(v.name.clone(), self.state.get(&v.name).copied().unwrap_or(0));
            }
        }

        if !self.solve_recursive(0, &order, &domains, &dist_map, &active_clauses, &mut env) {
            return Err(format!(
                "randomize: constraints on class `{}` are unsatisfiable",
                self.class.name
            ));
        }

        // 4. Record emitted values for `randc` variables and update `self.state`.
        for v in &self.class.vars {
            if let Some(&val) = env.get(&v.name) {
                self.state.insert(v.name.clone(), val);
                if v.rand_enabled && v.mode == RandMode::Randc {
                    self.randc_used
                        .entry(v.name.clone())
                        .or_default()
                        .insert(val);
                }
            }
        }

        Ok(self.state.clone())
    }

    fn prune_static_domains(
        clauses: &[RandConstraint],
        domains: &mut HashMap<String, DomainIntervals>,
        dist_map: &mut HashMap<String, Vec<DistItem>>,
    ) {
        for clause in clauses {
            match clause {
                RandConstraint::Inside {
                    var,
                    ranges,
                    negated,
                } => {
                    if let Some(dom) = domains.get_mut(var) {
                        let mask = DomainIntervals::from_ranges(ranges);
                        if *negated {
                            dom.exclude(&mask);
                        } else {
                            dom.intersect(&mask);
                        }
                    }
                }
                RandConstraint::Dist { var, items } => {
                    let positive_ranges: Vec<(u64, u64)> = items
                        .iter()
                        .filter(|it| match it.weight {
                            DistWeightKind::PerValue(w) | DistWeightKind::PerRange(w) => w > 0,
                        })
                        .map(|it| (it.lo, it.hi))
                        .collect();
                    if !positive_ranges.is_empty() {
                        if let Some(dom) = domains.get_mut(var) {
                            dom.intersect(&DomainIntervals::from_ranges(&positive_ranges));
                        }
                    }
                    dist_map
                        .entry(var.clone())
                        .or_default()
                        .extend(items.iter().copied());
                }
                RandConstraint::Expr(expr) => {
                    Self::prune_unary_expr(expr, domains);
                }
                _ => {}
            }
        }
    }

    fn prune_unary_expr(expr: &RandExpr, domains: &mut HashMap<String, DomainIntervals>) {
        match expr {
            RandExpr::And(a, b) => {
                Self::prune_unary_expr(a, domains);
                Self::prune_unary_expr(b, domains);
            }
            RandExpr::Eq(a, b) => {
                if let (RandExpr::Var(v), RandExpr::Const(c))
                | (RandExpr::Const(c), RandExpr::Var(v)) = (&**a, &**b)
                {
                    if let Some(dom) = domains.get_mut(v) {
                        dom.intersect(&DomainIntervals::from_ranges(&[(*c, *c)]));
                    }
                }
            }
            RandExpr::Ne(a, b) => {
                if let (RandExpr::Var(v), RandExpr::Const(c))
                | (RandExpr::Const(c), RandExpr::Var(v)) = (&**a, &**b)
                {
                    if let Some(dom) = domains.get_mut(v) {
                        dom.exclude(&DomainIntervals::from_ranges(&[(*c, *c)]));
                    }
                }
            }
            RandExpr::Lt(a, b) => match (&**a, &**b) {
                (RandExpr::Var(v), RandExpr::Const(c)) => {
                    if let Some(dom) = domains.get_mut(v) {
                        if *c == 0 {
                            dom.ranges.clear();
                        } else {
                            dom.intersect(&DomainIntervals::from_ranges(&[(0, *c - 1)]));
                        }
                    }
                }
                (RandExpr::Const(c), RandExpr::Var(v)) => {
                    if let Some(dom) = domains.get_mut(v) {
                        if *c == u64::MAX {
                            dom.ranges.clear();
                        } else {
                            dom.intersect(&DomainIntervals::from_ranges(&[(*c + 1, u64::MAX)]));
                        }
                    }
                }
                _ => {}
            },
            RandExpr::Le(a, b) => match (&**a, &**b) {
                (RandExpr::Var(v), RandExpr::Const(c)) => {
                    if let Some(dom) = domains.get_mut(v) {
                        dom.intersect(&DomainIntervals::from_ranges(&[(0, *c)]));
                    }
                }
                (RandExpr::Const(c), RandExpr::Var(v)) => {
                    if let Some(dom) = domains.get_mut(v) {
                        dom.intersect(&DomainIntervals::from_ranges(&[(*c, u64::MAX)]));
                    }
                }
                _ => {}
            },
            RandExpr::Gt(a, b) => {
                Self::prune_unary_expr(&RandExpr::Lt(b.clone(), a.clone()), domains);
            }
            RandExpr::Ge(a, b) => {
                Self::prune_unary_expr(&RandExpr::Le(b.clone(), a.clone()), domains);
            }
            _ => {}
        }
    }

    fn compute_solve_order(&self, clauses: &[RandConstraint]) -> Vec<usize> {
        let n = self.class.vars.len();
        let name_to_idx: HashMap<&str, usize> = self
            .class
            .vars
            .iter()
            .enumerate()
            .map(|(i, v)| (v.name.as_str(), i))
            .collect();
        let mut in_deg = vec![0usize; n];
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        for c in clauses {
            if let RandConstraint::SolveBefore { before, after } = c {
                if let (Some(&u), Some(&v)) =
                    (name_to_idx.get(before.as_str()), name_to_idx.get(after.as_str()))
                {
                    if u != v && !adj[u].contains(&v) {
                        adj[u].push(v);
                        in_deg[v] += 1;
                    }
                }
            }
        }
        // Prioritize `randc` variables when in-degree is 0 (IEEE 1800: randc solved before rand).
        let mut ready: Vec<usize> = (0..n).filter(|&i| in_deg[i] == 0).collect();
        ready.sort_by_key(|&i| (self.class.vars[i].mode != RandMode::Randc, i));
        let mut out = Vec::with_capacity(n);
        let mut visited = vec![false; n];
        while let Some(u) = ready.first().copied() {
            ready.remove(0);
            if visited[u] {
                continue;
            }
            visited[u] = true;
            out.push(u);
            for &v in &adj[u] {
                in_deg[v] = in_deg[v].saturating_sub(1);
                if in_deg[v] == 0 {
                    ready.push(v);
                    ready.sort_by_key(|&i| (self.class.vars[i].mode != RandMode::Randc, i));
                }
            }
        }
        for i in 0..n {
            if !visited[i] {
                out.push(i);
            }
        }
        out
    }

    fn check_constraints_partial(
        clauses: &[RandConstraint],
        env: &HashMap<String, u64>,
    ) -> bool {
        for c in clauses {
            match c {
                RandConstraint::Inside {
                    var,
                    ranges,
                    negated,
                } => {
                    if let Some(&v) = env.get(var) {
                        let in_set = ranges.iter().any(|&(lo, hi)| v >= lo && v <= hi);
                        if (*negated && in_set) || (!*negated && !in_set) {
                            return false;
                        }
                    }
                }
                RandConstraint::Dist { var, items } => {
                    if let Some(&v) = env.get(var) {
                        let allowed = items.iter().any(|it| {
                            let w = match it.weight {
                                DistWeightKind::PerValue(w) | DistWeightKind::PerRange(w) => w,
                            };
                            w > 0 && v >= it.lo && v <= it.hi
                        });
                        if !allowed && !items.is_empty() {
                            return false;
                        }
                    }
                }
                RandConstraint::Expr(expr) => {
                    if let Some(res) = expr.eval(env) {
                        if res == 0 {
                            return false;
                        }
                    }
                }
                RandConstraint::Implies { cond, body } => {
                    if let Some(cval) = cond.eval(env) {
                        if cval != 0 && !Self::check_constraints_partial(body, env) {
                            return false;
                        }
                    }
                }
                RandConstraint::Unique(vars) => {
                    let mut seen = HashSet::new();
                    for vname in vars {
                        if let Some(&val) = env.get(vname) {
                            if !seen.insert(val) {
                                return false;
                            }
                        }
                    }
                }
                RandConstraint::SolveBefore { .. } => {}
            }
        }
        true
    }

    fn candidate_sequence(
        &mut self,
        var: &RandVarSpec,
        dom: &DomainIntervals,
        dist_items: Option<&Vec<DistItem>>,
        clauses: &[RandConstraint],
        env: &HashMap<String, u64>,
    ) -> Vec<u64> {
        let card = dom.cardinality();
        if card == 0 {
            return Vec::new();
        }

        // If domain is small/moderate (<= 4096 values), enumerate all locally consistent values.
        if card <= 4096 {
            let all_vals = dom.enumerate_values(4096);
            let mut valid_vals = Vec::with_capacity(all_vals.len());
            let mut probe_env = env.clone();
            for v in all_vals {
                probe_env.insert(var.name.clone(), v);
                if Self::check_constraints_partial(clauses, &probe_env) {
                    valid_vals.push(v);
                }
            }
            if valid_vals.is_empty() {
                return Vec::new();
            }

            if var.mode == RandMode::Randc {
                let used = self.randc_used.entry(var.name.clone()).or_default();
                let mut unused: Vec<u64> = valid_vals
                    .iter()
                    .copied()
                    .filter(|v| !used.contains(v))
                    .collect();
                if unused.is_empty() {
                    // Full permutation cycle exhausted -> start a new cycle!
                    used.clear();
                    unused = valid_vals.clone();
                }
                self.shuffle_slice(&mut unused);
                // Append already-used values as fallback only if downstream constraints reject all unused.
                for v in valid_vals {
                    if !unused.contains(&v) {
                        unused.push(v);
                    }
                }
                return unused;
            }

            if let Some(items) = dist_items {
                return self.weighted_order(&valid_vals, items);
            }

            self.shuffle_slice(&mut valid_vals);
            return valid_vals;
        }

        // Large domain (> 4096 values): sample up to 128 distinct random points from the intervals,
        // plus boundary values and algebraic equality targets.
        let mut candidates = Vec::with_capacity(140);
        if let Some(items) = dist_items {
            for _ in 0..96 {
                if let Some(v) = self.sample_dist_item(items, dom) {
                    if !candidates.contains(&v) {
                        candidates.push(v);
                    }
                }
            }
        }
        for _ in 0..96 {
            let k = u128::from(self.rng.next_u64()) % card;
            if let Some(v) = dom.nth(k) {
                if !candidates.contains(&v) {
                    candidates.push(v);
                }
            }
        }
        for &(lo, hi) in &dom.ranges {
            if !candidates.contains(&lo) {
                candidates.push(lo);
            }
            if !candidates.contains(&hi) {
                candidates.push(hi);
            }
        }
        candidates
    }

    fn shuffle_slice(&mut self, slice: &mut [u64]) {
        for i in (1..slice.len()).rev() {
            let j = self.rng.below((i + 1) as u64) as usize;
            slice.swap(i, j);
        }
    }

    fn weighted_order(&mut self, vals: &[u64], items: &[DistItem]) -> Vec<u64> {
        let mut weighted: Vec<(u64, f64)> = vals
            .iter()
            .map(|&v| {
                let mut w_sum = 0.0f64;
                for it in items {
                    if v >= it.lo && v <= it.hi {
                        match it.weight {
                            DistWeightKind::PerValue(w) => w_sum += w as f64,
                            DistWeightKind::PerRange(w) => {
                                let span = ((it.hi - it.lo) as f64) + 1.0;
                                w_sum += (w as f64) / span;
                            }
                        }
                    }
                }
                (v, w_sum.max(1e-9))
            })
            .collect();

        let mut ordered = Vec::with_capacity(vals.len());
        while !weighted.is_empty() {
            let total_w: f64 = weighted.iter().map(|(_, w)| *w).sum();
            let r = ((self.rng.next_u64() as f64) / (u64::MAX as f64)) * total_w;
            let mut acc = 0.0;
            let mut chosen_idx = weighted.len() - 1;
            for (idx, &(_, w)) in weighted.iter().enumerate() {
                acc += w;
                if r <= acc {
                    chosen_idx = idx;
                    break;
                }
            }
            ordered.push(weighted.remove(chosen_idx).0);
        }
        ordered
    }

    fn sample_dist_item(&mut self, items: &[DistItem], dom: &DomainIntervals) -> Option<u64> {
        let item_weights: Vec<u128> = items
            .iter()
            .map(|it| match it.weight {
                DistWeightKind::PerValue(w) => u128::from(w) * (u128::from(it.hi - it.lo) + 1),
                DistWeightKind::PerRange(w) => u128::from(w),
            })
            .collect();
        let total: u128 = item_weights.iter().sum();
        if total == 0 {
            return None;
        }
        let mut pick = u128::from(self.rng.next_u64()) % total;
        for (it, &w) in items.iter().zip(item_weights.iter()) {
            if pick < w {
                let span = it.hi - it.lo + 1;
                let v = it.lo + self.rng.below(span);
                if dom.contains(v) {
                    return Some(v);
                }
                break;
            }
            pick -= w;
        }
        None
    }

    fn solve_recursive(
        &mut self,
        step: usize,
        order: &[usize],
        domains: &HashMap<String, DomainIntervals>,
        dist_map: &HashMap<String, Vec<DistItem>>,
        clauses: &[RandConstraint],
        env: &mut HashMap<String, u64>,
    ) -> bool {
        if step >= order.len() {
            return Self::check_constraints_partial(clauses, env);
        }
        let var_idx = order[step];
        let var = self.class.vars[var_idx].clone();
        if !var.rand_enabled {
            return self.solve_recursive(step + 1, order, domains, dist_map, clauses, env);
        }

        // Dynamic domain refinement when implication conditions (`cond -> { var inside ... }`)
        // have just been resolved by earlier variables in `order`.
        let mut local_dom = domains[&var.name].clone();
        let mut local_dist = dist_map.get(&var.name).cloned();
        for c in clauses {
            if let RandConstraint::Implies { cond, body } = c {
                if cond.eval(env) == Some(1) {
                    let mut tmp_doms = HashMap::new();
                    tmp_doms.insert(var.name.clone(), local_dom);
                    let mut tmp_dists = HashMap::new();
                    if let Some(d) = local_dist {
                        tmp_dists.insert(var.name.clone(), d);
                    }
                    Self::prune_static_domains(body, &mut tmp_doms, &mut tmp_dists);
                    local_dom = tmp_doms.remove(&var.name).unwrap();
                    local_dist = tmp_dists.remove(&var.name);
                }
            }
        }

        let candidates = self.candidate_sequence(
            &var,
            &local_dom,
            local_dist.as_ref(),
            clauses,
            env,
        );

        for cand in candidates {
            env.insert(var.name.clone(), cand);
            if Self::check_constraints_partial(clauses, env)
                && self.solve_recursive(step + 1, order, domains, dist_map, clauses, env)
            {
                return true;
            }
            env.remove(&var.name);
        }
        false
    }
}

// ============================================================================
// Part 2: Functional Coverage & Cartesian Cross Engine (`CovergroupEngine`)
// ============================================================================

/// Kind of bin inside a [`CoverpointSpec`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BinKind {
    /// Value / range bin: `bins b = { [lo:hi], v };`.
    Values(Vec<(u64, u64)>),
    /// Multi-step state transition bin: `bins t = (s0 => s1 => s2);`.
    Transition(Vec<u64>),
    /// Ignored value/range bin: `ignore_bins ig = { ... };`.
    IgnoreValues(Vec<(u64, u64)>),
    /// Illegal value/range bin: `illegal_bins bad = { ... };`.
    IllegalValues(Vec<(u64, u64)>),
    /// Illegal transition sequence bin: `illegal_bins bad_t = (a => b);`.
    IllegalTransition(Vec<u64>),
}

/// Specification of one bin in a [`CoverpointSpec`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinSpec {
    pub name: String,
    pub kind: BinKind,
    /// Minimum hit count required to consider this bin covered (`option.at_least`).
    pub at_least: u64,
}

/// Specification of a SystemVerilog `coverpoint`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoverpointSpec {
    pub name: String,
    pub signal: String,
    pub bins: Vec<BinSpec>,
}

impl CoverpointSpec {
    pub fn new(name: impl Into<String>, signal: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            signal: signal.into(),
            bins: Vec::new(),
        }
    }

    /// Add a value/range bin (`bins <name> = { [lo:hi], ... }`) with threshold `at_least`.
    pub fn add_value_bin(
        &mut self,
        name: impl Into<String>,
        ranges: Vec<(u64, u64)>,
        at_least: u64,
    ) -> &mut Self {
        self.bins.push(BinSpec {
            name: name.into(),
            kind: BinKind::Values(ranges),
            at_least: at_least.max(1),
        });
        self
    }

    /// Add a multi-step transition sequence bin (`bins <name> = (s0 => s1 => ...)`).
    pub fn add_transition_bin(
        &mut self,
        name: impl Into<String>,
        sequence: Vec<u64>,
        at_least: u64,
    ) -> &mut Self {
        self.bins.push(BinSpec {
            name: name.into(),
            kind: BinKind::Transition(sequence),
            at_least: at_least.max(1),
        });
        self
    }

    /// Add an `ignore_bins` entry excluded from coverage and cross products.
    pub fn add_ignore_bin(&mut self, name: impl Into<String>, ranges: Vec<(u64, u64)>) -> &mut Self {
        self.bins.push(BinSpec {
            name: name.into(),
            kind: BinKind::IgnoreValues(ranges),
            at_least: 1,
        });
        self
    }

    /// Add an `illegal_bins` value/range entry that records a violation if sampled.
    pub fn add_illegal_bin(
        &mut self,
        name: impl Into<String>,
        ranges: Vec<(u64, u64)>,
    ) -> &mut Self {
        self.bins.push(BinSpec {
            name: name.into(),
            kind: BinKind::IllegalValues(ranges),
            at_least: 1,
        });
        self
    }

    /// Add an `illegal_bins` state-transition entry (`illegal_bins <name> = (s0 => s1)`).
    pub fn add_illegal_transition_bin(
        &mut self,
        name: impl Into<String>,
        sequence: Vec<u64>,
    ) -> &mut Self {
        self.bins.push(BinSpec {
            name: name.into(),
            kind: BinKind::IllegalTransition(sequence),
            at_least: 1,
        });
        self
    }
}

/// Specification of an $N$-way Cartesian `cross` between two or more coverpoints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossSpec {
    pub name: String,
    pub coverpoints: Vec<String>,
    pub at_least: u64,
    /// Optional ignored bin-name tuples (`ignore_bins` on the cross).
    pub ignore_tuples: Vec<Vec<String>>,
}

impl CrossSpec {
    pub fn new(name: impl Into<String>, coverpoints: Vec<String>, at_least: u64) -> Self {
        Self {
            name: name.into(),
            coverpoints,
            at_least: at_least.max(1),
            ignore_tuples: Vec::new(),
        }
    }

    pub fn with_ignore_tuple(mut self, tuple: Vec<String>) -> Self {
        self.ignore_tuples.push(tuple);
        self
    }
}

/// Violation raised when an `illegal_bins` value or transition is sampled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IllegalBinViolation {
    pub sample_index: u64,
    pub coverpoint: String,
    pub bin_name: String,
    pub value: u64,
    pub detail: String,
}

impl std::fmt::Display for IllegalBinViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[COVERGROUP][ILLEGAL_BIN] sample={} cp={} bin={} val={} :: {}",
            self.sample_index, self.coverpoint, self.bin_name, self.value, self.detail
        )
    }
}

/// Per-bin status in a [`CovergroupReport`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinStatus {
    pub name: String,
    pub hits: u64,
    pub at_least: u64,
    pub covered: bool,
}

/// Per-coverpoint status in a [`CovergroupReport`].
#[derive(Clone, Debug, PartialEq)]
pub struct CoverpointReport {
    pub name: String,
    pub signal: String,
    pub bins: Vec<BinStatus>,
    pub covered_bins: usize,
    pub total_active_bins: usize,
    pub coverage_pct: f64,
}

/// Per-cross status in a [`CovergroupReport`].
#[derive(Clone, Debug, PartialEq)]
pub struct CrossReport {
    pub name: String,
    pub coverpoints: Vec<String>,
    pub tuple_hits: BTreeMap<Vec<String>, u64>,
    pub covered_tuples: usize,
    pub total_tuples: usize,
    pub coverage_pct: f64,
}

/// Complete report for a [`CovergroupEngine`].
#[derive(Clone, Debug, PartialEq)]
pub struct CovergroupReport {
    pub name: String,
    pub samples: u64,
    pub coverpoints: Vec<CoverpointReport>,
    pub crosses: Vec<CrossReport>,
    pub illegal_violations: Vec<IllegalBinViolation>,
    pub overall_coverage_pct: f64,
}

impl CovergroupReport {
    pub fn summary_line(&self) -> String {
        format!(
            "covergroup {}: samples={} coverage={:.1}% coverpoints={} crosses={} illegal_violations={}",
            self.name,
            self.samples,
            self.overall_coverage_pct,
            self.coverpoints.len(),
            self.crosses.len(),
            self.illegal_violations.len()
        )
    }
}

/// Runtime IEEE 1800 `covergroup` evaluation engine.
#[derive(Clone, Debug)]
pub struct CovergroupEngine {
    pub name: String,
    pub samples: u64,
    coverpoints: Vec<CoverpointSpec>,
    crosses: Vec<CrossSpec>,
    /// `(cp_idx, bin_idx)` -> hit count.
    bin_hits: Vec<Vec<u64>>,
    /// Per-coverpoint recent sample history for multi-step transition bins (`s0 => s1 => s2`).
    cp_history: Vec<Vec<u64>>,
    /// `cross_idx` -> (`tuple_of_bin_names` -> hit count).
    cross_hits: Vec<BTreeMap<Vec<String>, u64>>,
    /// Recorded `illegal_bins` violations.
    violations: Vec<IllegalBinViolation>,
}

impl CovergroupEngine {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            samples: 0,
            coverpoints: Vec::new(),
            crosses: Vec::new(),
            bin_hits: Vec::new(),
            cp_history: Vec::new(),
            cross_hits: Vec::new(),
            violations: Vec::new(),
        }
    }

    /// Add a [`CoverpointSpec`] to this covergroup.
    pub fn add_coverpoint(&mut self, cp: CoverpointSpec) -> &mut Self {
        let n_bins = cp.bins.len();
        self.coverpoints.push(cp);
        self.bin_hits.push(vec![0u64; n_bins]);
        self.cp_history.push(Vec::new());
        self
    }

    /// Add a Cartesian [`CrossSpec`] over previously added coverpoints.
    pub fn add_cross(&mut self, cross: CrossSpec) -> Result<&mut Self, String> {
        let mut tuple_lists: Vec<Vec<String>> = vec![Vec::new()];
        for cp_name in &cross.coverpoints {
            let cp = self
                .coverpoints
                .iter()
                .find(|c| &c.name == cp_name)
                .ok_or_else(|| format!("cross {}: unknown coverpoint `{cp_name}`", cross.name))?;
            // Only active value/transition bins participate in the Cartesian cross product
            // (`ignore_bins` and `illegal_bins` are excluded per IEEE 1800-2017 Section 19.6).
            let active_bin_names: Vec<String> = cp
                .bins
                .iter()
                .filter(|b| matches!(b.kind, BinKind::Values(_) | BinKind::Transition(_)))
                .map(|b| b.name.clone())
                .collect();
            let mut next_tuples = Vec::new();
            for prefix in &tuple_lists {
                for bname in &active_bin_names {
                    let mut t = prefix.clone();
                    t.push(bname.clone());
                    next_tuples.push(t);
                }
            }
            tuple_lists = next_tuples;
        }

        let mut map = BTreeMap::new();
        for t in tuple_lists {
            if !cross.ignore_tuples.contains(&t) {
                map.insert(t, 0u64);
            }
        }
        self.crosses.push(cross);
        self.cross_hits.push(map);
        Ok(self)
    }

    /// Sample one transaction / cycle from a `(signal_name -> value)` map.
    /// Returns any new [`IllegalBinViolation`]s triggered on this sample.
    pub fn sample(&mut self, values: &HashMap<String, u64>) -> Vec<IllegalBinViolation> {
        self.samples += 1;
        let sample_idx = self.samples;
        let mut fresh_violations = Vec::new();
        // Track which active bin names matched for each coverpoint on this sample (for cross evaluation).
        let mut active_hits_per_cp: HashMap<String, Vec<String>> = HashMap::new();

        for (cp_idx, cp) in self.coverpoints.iter().enumerate() {
            let Some(&val) = values.get(&cp.signal) else {
                continue;
            };

            // Check if `val` falls in an `ignore_bins` range first.
            let is_ignored = cp.bins.iter().any(|b| match &b.kind {
                BinKind::IgnoreValues(ranges) => {
                    ranges.iter().any(|&(lo, hi)| val >= lo && val <= hi)
                }
                _ => false,
            });
            if is_ignored {
                continue;
            }

            self.cp_history[cp_idx].push(val);
            let hist = &self.cp_history[cp_idx];
            let mut matched_bin_names = Vec::new();

            for (bin_idx, bin) in cp.bins.iter().enumerate() {
                match &bin.kind {
                    BinKind::Values(ranges) => {
                        if ranges.iter().any(|&(lo, hi)| val >= lo && val <= hi) {
                            self.bin_hits[cp_idx][bin_idx] += 1;
                            matched_bin_names.push(bin.name.clone());
                        }
                    }
                    BinKind::Transition(seq) => {
                        if !seq.is_empty()
                            && hist.len() >= seq.len()
                            && &hist[hist.len() - seq.len()..] == seq.as_slice()
                        {
                            self.bin_hits[cp_idx][bin_idx] += 1;
                            matched_bin_names.push(bin.name.clone());
                        }
                    }
                    BinKind::IllegalValues(ranges) => {
                        if ranges.iter().any(|&(lo, hi)| val >= lo && val <= hi) {
                            self.bin_hits[cp_idx][bin_idx] += 1;
                            let v = IllegalBinViolation {
                                sample_index: sample_idx,
                                coverpoint: cp.name.clone(),
                                bin_name: bin.name.clone(),
                                value: val,
                                detail: format!(
                                    "illegal value {val} hit on {}.{}",
                                    cp.name, bin.name
                                ),
                            };
                            self.violations.push(v.clone());
                            fresh_violations.push(v);
                        }
                    }
                    BinKind::IllegalTransition(seq) => {
                        if !seq.is_empty()
                            && hist.len() >= seq.len()
                            && &hist[hist.len() - seq.len()..] == seq.as_slice()
                        {
                            self.bin_hits[cp_idx][bin_idx] += 1;
                            let v = IllegalBinViolation {
                                sample_index: sample_idx,
                                coverpoint: cp.name.clone(),
                                bin_name: bin.name.clone(),
                                value: val,
                                detail: format!(
                                    "illegal transition {:?} hit on {}.{}",
                                    seq, cp.name, bin.name
                                ),
                            };
                            self.violations.push(v.clone());
                            fresh_violations.push(v);
                        }
                    }
                    BinKind::IgnoreValues(_) => {}
                }
            }
            active_hits_per_cp.insert(cp.name.clone(), matched_bin_names);
        }

        // Update N-way Cartesian cross bins.
        for (cross_idx, cross) in self.crosses.iter().enumerate() {
            let mut tuples: Vec<Vec<String>> = vec![Vec::new()];
            for cp_name in &cross.coverpoints {
                let Some(bin_names) = active_hits_per_cp.get(cp_name) else {
                    tuples.clear();
                    break;
                };
                if bin_names.is_empty() {
                    tuples.clear();
                    break;
                }
                let mut next = Vec::new();
                for prefix in &tuples {
                    for bname in bin_names {
                        let mut t = prefix.clone();
                        t.push(bname.clone());
                        next.push(t);
                    }
                }
                tuples = next;
            }
            for t in tuples {
                if let Some(cnt) = self.cross_hits[cross_idx].get_mut(&t) {
                    *cnt += 1;
                }
            }
        }

        fresh_violations
    }

    /// Sample directly from a running [`Sim`] instance.
    pub fn sample_sim(&mut self, sim: &Sim) -> Vec<IllegalBinViolation> {
        let mut map = HashMap::new();
        for cp in &self.coverpoints {
            if let Some(v) = sim.sample(&cp.signal) {
                map.insert(cp.signal.clone(), v);
            }
        }
        self.sample(&map)
    }

    /// All recorded `illegal_bins` violations.
    pub fn violations(&self) -> &[IllegalBinViolation] {
        &self.violations
    }

    /// Generate a [`CovergroupReport`] with per-coverpoint, per-cross, and overall coverage percentages.
    pub fn report(&self) -> CovergroupReport {
        let mut cp_reports = Vec::with_capacity(self.coverpoints.len());
        let mut pct_accum = 0.0f64;
        let mut item_count = 0usize;

        for (cp_idx, cp) in self.coverpoints.iter().enumerate() {
            let mut bin_statuses = Vec::new();
            let mut covered = 0usize;
            for (bin_idx, bin) in cp.bins.iter().enumerate() {
                if !matches!(bin.kind, BinKind::Values(_) | BinKind::Transition(_)) {
                    continue;
                }
                let hits = self.bin_hits[cp_idx][bin_idx];
                let is_cov = hits >= bin.at_least;
                if is_cov {
                    covered += 1;
                }
                bin_statuses.push(BinStatus {
                    name: bin.name.clone(),
                    hits,
                    at_least: bin.at_least,
                    covered: is_cov,
                });
            }
            let total = bin_statuses.len();
            let pct = if total == 0 {
                100.0
            } else {
                (covered as f64 / total as f64) * 100.0
            };
            pct_accum += pct;
            item_count += 1;
            cp_reports.push(CoverpointReport {
                name: cp.name.clone(),
                signal: cp.signal.clone(),
                bins: bin_statuses,
                covered_bins: covered,
                total_active_bins: total,
                coverage_pct: pct,
            });
        }

        let mut cross_reports = Vec::with_capacity(self.crosses.len());
        for (cross_idx, cross) in self.crosses.iter().enumerate() {
            let hits_map = &self.cross_hits[cross_idx];
            let total = hits_map.len();
            let covered = hits_map
                .values()
                .filter(|&&c| c >= cross.at_least)
                .count();
            let pct = if total == 0 {
                100.0
            } else {
                (covered as f64 / total as f64) * 100.0
            };
            pct_accum += pct;
            item_count += 1;
            cross_reports.push(CrossReport {
                name: cross.name.clone(),
                coverpoints: cross.coverpoints.clone(),
                tuple_hits: hits_map.clone(),
                covered_tuples: covered,
                total_tuples: total,
                coverage_pct: pct,
            });
        }

        let overall_coverage_pct = if item_count == 0 {
            100.0
        } else {
            pct_accum / (item_count as f64)
        };

        CovergroupReport {
            name: self.name.clone(),
            samples: self.samples,
            coverpoints: cp_reports,
            crosses: cross_reports,
            illegal_violations: self.violations.clone(),
            overall_coverage_pct,
        }
    }
}
