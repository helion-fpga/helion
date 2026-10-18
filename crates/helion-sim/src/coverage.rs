//! Functional coverage collector & unified SVA property summary reporting.
//!
//! Tracks:
//! - **Net toggle coverage** (`0 -> 1` and `1 -> 0` transitions per net/signal)
//! - **Branch / mux condition coverage** (`true` / `false` arm hits on muxes and enables)
//! - **FSM state & arc transition coverage** (auto-discovered or user-registered multi-bit state registers)
//! - **SVA `assert property` & `cover property` summary** (attempts, non-vacuous passes, vacuous passes, failures, cover hits)

use crate::fast::{CompiledSim, ConeNodeKind};
use crate::sva::SvaReport;
use helion_ir::{resolve, Design};
use std::collections::{BTreeMap, HashMap};

/// Per-net toggle coverage counter (`0 -> 1` and `1 -> 0`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetToggleStats {
    pub net: String,
    pub transitions_0_to_1: u64,
    pub transitions_1_to_0: u64,
    pub last_value: Option<bool>,
}

impl NetToggleStats {
    pub fn new(net: impl Into<String>) -> Self {
        Self {
            net: net.into(),
            transitions_0_to_1: 0,
            transitions_1_to_0: 0,
            last_value: None,
        }
    }

    #[inline]
    pub fn sample(&mut self, value: bool) {
        if let Some(prev) = self.last_value {
            if !prev && value {
                self.transitions_0_to_1 += 1;
            } else if prev && !value {
                self.transitions_1_to_0 += 1;
            }
        }
        self.last_value = Some(value);
    }

    /// True if both `0 -> 1` and `1 -> 0` transitions have occurred at least once.
    #[inline]
    pub fn is_fully_toggled(&self) -> bool {
        self.transitions_0_to_1 > 0 && self.transitions_1_to_0 > 0
    }

    /// True if exactly one of `0 -> 1` or `1 -> 0` has occurred.
    #[inline]
    pub fn is_partially_toggled(&self) -> bool {
        (self.transitions_0_to_1 > 0) ^ (self.transitions_1_to_0 > 0)
    }

    /// Number of covered toggle bins (`0`, `1`, or `2`).
    #[inline]
    pub fn bins_hit(&self) -> usize {
        usize::from(self.transitions_0_to_1 > 0) + usize::from(self.transitions_1_to_0 > 0)
    }
}

/// Per-branch / mux condition coverage counter (`true` arm and `false` arm hits).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchCoverageStats {
    pub name: String,
    pub cond_net: String,
    pub true_hits: u64,
    pub false_hits: u64,
}

impl BranchCoverageStats {
    #[inline]
    pub fn sample(&mut self, cond: bool) {
        if cond {
            self.true_hits += 1;
        } else {
            self.false_hits += 1;
        }
    }

    /// True if both the `true` and `false` arms were taken at least once.
    #[inline]
    pub fn is_both_covered(&self) -> bool {
        self.true_hits > 0 && self.false_hits > 0
    }

    /// Number of covered branch arms (`0`, `1`, or `2`).
    #[inline]
    pub fn arms_hit(&self) -> usize {
        usize::from(self.true_hits > 0) + usize::from(self.false_hits > 0)
    }
}

/// FSM multi-bit state and state-transition coverage tracker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsmCoverageStats {
    pub name: String,
    /// Ordered bit nets `[lsb..msb]`.
    pub bit_nets: Vec<String>,
    /// Number of reachable states expected (defaults to `1 << width` for width <= 8).
    pub expected_states: usize,
    /// State encoding -> number of cycles visited.
    pub visited_states: BTreeMap<u64, u64>,
    /// `(from_state, to_state)` -> number of transitions observed (where `from_state != to_state`).
    pub transitions: BTreeMap<(u64, u64), u64>,
    pub last_state: Option<u64>,
}

impl FsmCoverageStats {
    pub fn new(name: impl Into<String>, bit_nets: Vec<String>, expected_states: Option<usize>) -> Self {
        let width = bit_nets.len();
        let default_expected = if width > 0 && width <= 8 {
            1usize << width
        } else {
            0
        };
        Self {
            name: name.into(),
            bit_nets,
            expected_states: expected_states.unwrap_or(default_expected),
            visited_states: BTreeMap::new(),
            transitions: BTreeMap::new(),
            last_state: None,
        }
    }

    #[inline]
    pub fn record_state(&mut self, state: u64) {
        *self.visited_states.entry(state).or_insert(0) += 1;
        if let Some(prev) = self.last_state {
            if prev != state {
                *self.transitions.entry((prev, state)).or_insert(0) += 1;
            }
        }
        self.last_state = Some(state);
    }

    /// State coverage percentage (`0.0..=100.0`).
    pub fn state_coverage_pct(&self) -> f64 {
        if self.expected_states == 0 {
            return 100.0;
        }
        let ratio = (self.visited_states.len() as f64) / (self.expected_states as f64);
        (ratio * 100.0).min(100.0)
    }
}

/// Complete functional coverage and SVA property summary report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FunctionalCoverageReport {
    pub cycles_sampled: u64,
    pub net_toggles: Vec<NetToggleStats>,
    pub branches: Vec<BranchCoverageStats>,
    pub fsms: Vec<FsmCoverageStats>,
    pub sva: SvaReport,
}

impl FunctionalCoverageReport {
    /// Total number of tracked nets.
    pub fn total_nets(&self) -> usize {
        self.net_toggles.len()
    }

    /// Number of nets with both `0 -> 1` and `1 -> 0` transitions observed.
    pub fn fully_toggled_nets(&self) -> usize {
        self.net_toggles
            .iter()
            .filter(|n| n.is_fully_toggled())
            .count()
    }

    /// Number of nets with only one of `0 -> 1` or `1 -> 0` observed.
    pub fn partially_toggled_nets(&self) -> usize {
        self.net_toggles
            .iter()
            .filter(|n| n.is_partially_toggled())
            .count()
    }

    /// Toggle bin coverage percentage (`(bins_hit / (2 * total_nets)) * 100.0`).
    pub fn toggle_bin_coverage_pct(&self) -> f64 {
        let total_bins = self.net_toggles.len() * 2;
        if total_bins == 0 {
            return 100.0;
        }
        let hit_bins: usize = self.net_toggles.iter().map(NetToggleStats::bins_hit).sum();
        (hit_bins as f64 / total_bins as f64) * 100.0
    }

    /// Full-net toggle coverage percentage (`(fully_toggled_nets / total_nets) * 100.0`).
    pub fn full_toggle_coverage_pct(&self) -> f64 {
        if self.net_toggles.is_empty() {
            return 100.0;
        }
        (self.fully_toggled_nets() as f64 / self.net_toggles.len() as f64) * 100.0
    }

    /// Branch arm coverage percentage (`(arms_hit / (2 * branches.len())) * 100.0`).
    pub fn branch_coverage_pct(&self) -> f64 {
        let total_arms = self.branches.len() * 2;
        if total_arms == 0 {
            return 100.0;
        }
        let hit_arms: usize = self.branches.iter().map(BranchCoverageStats::arms_hit).sum();
        (hit_arms as f64 / total_arms as f64) * 100.0
    }

    /// Average FSM state coverage percentage across all tracked FSMs.
    pub fn fsm_state_coverage_pct(&self) -> f64 {
        if self.fsms.is_empty() {
            return 100.0;
        }
        let sum: f64 = self.fsms.iter().map(FsmCoverageStats::state_coverage_pct).sum();
        sum / (self.fsms.len() as f64)
    }

    /// Single-line machine-readable summary for logs and Tcl `report_coverage`.
    pub fn summary_line(&self) -> String {
        format!(
            "coverage cycles={} toggle_nets={}/{} ({:.1}%) branch_cov={:.1}% fsm_cov={:.1}% {}",
            self.cycles_sampled,
            self.fully_toggled_nets(),
            self.total_nets(),
            self.toggle_bin_coverage_pct(),
            self.branch_coverage_pct(),
            self.fsm_state_coverage_pct(),
            self.sva.summary_line()
        )
    }

    /// Detailed multi-line report lines in Vivado `xsim` / Tcl format.
    pub fn tcl_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(self.summary_line());
        for t in &self.net_toggles {
            let status = if t.is_fully_toggled() {
                "FULL"
            } else if t.is_partially_toggled() {
                "PARTIAL"
            } else {
                "NONE"
            };
            lines.push(format!(
                "toggle {}: {} 0->1={} 1->0={}",
                t.net, status, t.transitions_0_to_1, t.transitions_1_to_0
            ));
        }
        for b in &self.branches {
            let status = if b.is_both_covered() {
                "BOTH"
            } else if b.arms_hit() == 1 {
                "SINGLE"
            } else {
                "NONE"
            };
            lines.push(format!(
                "branch {} ({}): {} true={} false={}",
                b.name, b.cond_net, status, b.true_hits, b.false_hits
            ));
        }
        for fsm in &self.fsms {
            lines.push(format!(
                "fsm {}: states={}/{} ({:.1}%) transitions={}",
                fsm.name,
                fsm.visited_states.len(),
                fsm.expected_states,
                fsm.state_coverage_pct(),
                fsm.transitions.len()
            ));
        }
        lines.extend(self.sva.tcl_lines());
        lines
    }
}

/// Split a bit-indexed net name such as `state_0`, `cnt_3`, `q0`, or `fsm[2]` into `(prefix, bit_index)`.
fn split_bus_bit_name(net: &str) -> Option<(String, usize)> {
    if let Some((base, rest)) = net.rsplit_once('_') {
        if !base.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            let idx: usize = rest.parse().ok()?;
            return Some((base.to_string(), idx));
        }
    }
    if let Some(open) = net.rfind('[') {
        if net.ends_with(']') && open > 0 {
            let base = &net[..open];
            let idx_str = &net[open + 1..net.len() - 1];
            let idx: usize = idx_str.parse().ok()?;
            return Some((base.to_string(), idx));
        }
    }
    // Also recognize compact `q0..q7` style FF Q nets when prefix is alphabetic and suffix is 1 digit.
    let digit_start = net.rfind(|c: char| !c.is_ascii_digit()).map(|i| i + 1)?;
    if digit_start > 0 && digit_start < net.len() {
        let base = &net[..digit_start];
        let suffix = &net[digit_start..];
        if suffix.len() == 1 && (base == "q" || base.contains("state") || base.contains("cnt")) {
            let idx: usize = suffix.parse().ok()?;
            return Some((base.to_string(), idx));
        }
    }
    None
}

/// Runtime collector for toggle coverage, branch coverage, and FSM state/transition coverage.
#[derive(Clone, Debug, Default)]
pub struct CoverageCollector {
    pub cycles_sampled: u64,
    toggles: BTreeMap<String, NetToggleStats>,
    branches: BTreeMap<String, BranchCoverageStats>,
    fsms: BTreeMap<String, FsmCoverageStats>,
}

impl CoverageCollector {
    /// Create a coverage collector pre-populated from a [`Design`] and its [`CompiledSim`] schedule.
    pub fn from_design(design: &Design, compiled: &CompiledSim) -> Self {
        let mut toggles = BTreeMap::new();
        for p in &design.ports {
            let name = resolve(p.name).to_string();
            toggles.insert(name.clone(), NetToggleStats::new(name));
        }
        for n in &design.nets {
            let name = resolve(n.name).to_string();
            if name != "0" && name != "1" && name != "1'b0" && name != "1'b1" {
                toggles.insert(name.clone(), NetToggleStats::new(name));
            }
        }

        // Auto-discover branches from MUXF7/MUXF8 select nets, FF CE/RST nets, and input control ports.
        let mut branches = BTreeMap::new();
        for node in &compiled.schedule.ops {
            match &node.kind {
                ConeNodeKind::Muxf7 { s_net, .. } | ConeNodeKind::Muxf8 { s_net, .. } => {
                    if let Some(cond_net) = compiled.schedule.net_names.get(*s_net as usize) {
                        if *s_net >= 2 {
                            let bname = format!("{}.S", node.cell_name);
                            branches.insert(
                                bname.clone(),
                                BranchCoverageStats {
                                    name: bname,
                                    cond_net: cond_net.clone(),
                                    true_hits: 0,
                                    false_hits: 0,
                                },
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        for ff in &compiled.schedule.ffs {
            if ff.ce_net >= 2 {
                if let Some(cond_net) = compiled.schedule.net_names.get(ff.ce_net as usize) {
                    let bname = format!("{}.CE", ff.name);
                    branches.insert(
                        bname.clone(),
                        BranchCoverageStats {
                            name: bname,
                            cond_net: cond_net.clone(),
                            true_hits: 0,
                            false_hits: 0,
                        },
                    );
                }
            }
        }

        // Auto-discover multi-bit sequential buses (2..=8 bits) from FF Q nets for FSM state tracking.
        let mut bus_groups: BTreeMap<String, BTreeMap<usize, String>> = BTreeMap::new();
        for ff in &compiled.schedule.ffs {
            if let Some(q_name) = compiled.schedule.net_names.get(ff.q_net as usize) {
                if let Some((prefix, bit_idx)) = split_bus_bit_name(q_name) {
                    bus_groups
                        .entry(prefix)
                        .or_default()
                        .insert(bit_idx, q_name.clone());
                }
            }
        }
        let mut fsms = BTreeMap::new();
        for (prefix, bits_map) in bus_groups {
            if (2..=8).contains(&bits_map.len()) {
                // Verify contiguous bits 0..N
                let is_contiguous = (0..bits_map.len()).all(|i| bits_map.contains_key(&i));
                if is_contiguous {
                    let bit_nets: Vec<String> =
                        (0..bits_map.len()).map(|i| bits_map[&i].clone()).collect();
                    fsms.insert(
                        prefix.clone(),
                        FsmCoverageStats::new(prefix, bit_nets, None),
                    );
                }
            }
        }
        for (net_name, entry) in &mut toggles {
            if let Some(v) = compiled.sample(net_name) {
                entry.last_value = Some(v != 0);
            }
        }
        for fsm in fsms.values_mut() {
            let mut state = 0u64;
            let mut valid = true;
            for (bit_idx, net_name) in fsm.bit_nets.iter().enumerate() {
                if let Some(b) = compiled.sample(net_name) {
                    if b != 0 {
                        state |= 1u64 << bit_idx;
                    }
                } else {
                    valid = false;
                    break;
                }
            }
            if valid {
                fsm.last_state = Some(state);
            }
        }

        Self {
            cycles_sampled: 0,
            toggles,
            branches,
            fsms,
        }
    }

    /// Register an explicit branch condition net to track `true` / `false` arm hits.
    pub fn register_branch(&mut self, name: impl Into<String>, cond_net: impl Into<String>) {
        let name = name.into();
        let cond_net = cond_net.into();
        self.toggles
            .entry(cond_net.clone())
            .or_insert_with(|| NetToggleStats::new(cond_net.clone()));
        self.branches.insert(
            name.clone(),
            BranchCoverageStats {
                name,
                cond_net,
                true_hits: 0,
                false_hits: 0,
            },
        );
    }

    /// Register an explicit multi-bit FSM state bus (`bit_nets` ordered LSB-first `[bit0, bit1, ...]`).
    pub fn register_fsm(
        &mut self,
        name: impl Into<String>,
        bit_nets: Vec<String>,
        expected_states: Option<usize>,
    ) {
        let name = name.into();
        for net in &bit_nets {
            self.toggles
                .entry(net.clone())
                .or_insert_with(|| NetToggleStats::new(net.clone()));
        }
        self.fsms.insert(
            name.clone(),
            FsmCoverageStats::new(name, bit_nets, expected_states),
        );
    }

    /// Sample one simulation cycle from a [`CompiledSim`].
    pub fn sample_compiled(&mut self, sim: &CompiledSim) {
        self.cycles_sampled += 1;
        for (net_name, entry) in &mut self.toggles {
            if net_name.eq_ignore_ascii_case("clk") {
                entry.sample(false);
                entry.sample(true);
                entry.sample(false);
            } else if let Some(v) = sim.sample(net_name) {
                entry.sample(v != 0);
            }
        }
        for branch in self.branches.values_mut() {
            if let Some(v) = sim.sample(&branch.cond_net) {
                branch.sample(v != 0);
            }
        }
        for fsm in self.fsms.values_mut() {
            let mut state = 0u64;
            let mut valid = true;
            for (bit_idx, net_name) in fsm.bit_nets.iter().enumerate() {
                match sim.sample(net_name) {
                    Some(b) => {
                        if b != 0 {
                            state |= 1u64 << bit_idx;
                        }
                    }
                    None => {
                        valid = false;
                        break;
                    }
                }
            }
            if valid {
                fsm.record_state(state);
            }
        }
    }

    /// Sample one simulation cycle from a map of `(net_name -> bool)`.
    pub fn sample_map(&mut self, net_values: &HashMap<String, bool>) {
        self.cycles_sampled += 1;
        for (net_name, &val) in net_values {
            self.toggles
                .entry(net_name.clone())
                .or_insert_with(|| NetToggleStats::new(net_name.clone()))
                .sample(val);
        }
        for branch in self.branches.values_mut() {
            if let Some(&val) = net_values.get(&branch.cond_net) {
                branch.sample(val);
            }
        }
        for fsm in self.fsms.values_mut() {
            let mut state = 0u64;
            let mut valid = true;
            for (bit_idx, net_name) in fsm.bit_nets.iter().enumerate() {
                if let Some(&b) = net_values.get(net_name) {
                    if b {
                        state |= 1u64 << bit_idx;
                    }
                } else {
                    valid = false;
                    break;
                }
            }
            if valid {
                fsm.record_state(state);
            }
        }
    }

    /// Build a [`FunctionalCoverageReport`] combining collected coverage and [`SvaReport`].
    pub fn report(&self, sva: SvaReport) -> FunctionalCoverageReport {
        FunctionalCoverageReport {
            cycles_sampled: self.cycles_sampled,
            net_toggles: self.toggles.values().cloned().collect(),
            branches: self.branches.values().cloned().collect(),
            fsms: self.fsms.values().cloned().collect(),
            sva,
        }
    }
}
