//! RTL line and branch coverage of a simulated netlist.
//!
//! The kernel simulates a synthesized netlist, so RTL statements never
//! "execute" here. A design synthesized with line probes
//! (`helion_sv::synth_sv_path_line_cov`) carries one 1-bit probe net per
//! procedural statement, `if` / `case` arm and continuous `assign`, tagged
//! with [`helion_ir::LineProbe`] attributes. Each probe is real logic computed
//! from the design's own signals, so reading it during simulation tells
//! whether control *would* reach that statement / arm in the current state:
//!
//! - **Clocked statements and arms** (inside an edge-triggered block) are
//!   sampled once per rising edge with the *pre-edge* values — the values the
//!   block sees when it runs. A hit is one edge on which the statement ran or
//!   the arm was taken.
//! - **Combinational statements and arms** (`always_comb`, `always @*`) are
//!   sampled once per *settled state*: after stimulus changed, when the
//!   testbench next reads a DUT signal or the next edge arrives. A hit is one
//!   observed state in which the statement was reached / the arm selected.
//!   Glitches inside one settle are not visible (zero-delay netlist).
//! - **Continuous assigns** are sampled like combinational logic; a hit is
//!   an observed state in which the assigned value (up to 32 bits) differs
//!   from the previously observed one ("active", not merely evaluated).
//!
//! Statements at the top of a block (no enclosing condition) have no probe
//! net; they count once per edge (clocked) or per observed state (combinational).
//! Single-clock model: every clocked block is sampled on the kernel's one
//! rising-edge step. Lines the elaborator could not instrument are listed in
//! [`LineCoverageReport::not_instrumented`] instead of being reported as misses.

use std::collections::{BTreeMap, BTreeSet};

use helion_ir::{resolve, Design, LineProbe, LineProbeKind, SrcLoc};

/// What a covered line holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RtlLineKind {
    /// Procedural statement (assignment, `if`, `case`, `for`).
    Statement,
    /// Continuous `assign`.
    Assign,
}

/// Hit count of one RTL source line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtlLineHits {
    pub file: String,
    pub line: u32,
    pub kind: RtlLineKind,
    /// Edges / observed states in which the line ran (statements) or changed
    /// value (assigns). See the module docs.
    pub hits: u64,
}

/// One arm of an RTL branch and how often it was taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtlArmHits {
    /// `true` / `false` for an `if`; the item text or `default` for a `case`.
    pub label: String,
    /// Line of the arm (the `if` line for both `if` arms).
    pub line: u32,
    pub hits: u64,
}

/// One RTL `if` / `case` and its arms (source order).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtlBranchHits {
    pub file: String,
    /// Line of the `if` / `case` keyword.
    pub line: u32,
    /// `if` or `case`.
    pub construct: String,
    pub arms: Vec<RtlArmHits>,
}

impl RtlBranchHits {
    pub fn arms_hit(&self) -> usize {
        self.arms.iter().filter(|a| a.hits > 0).count()
    }
}

/// A source line the elaborator could not instrument, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtlLineGap {
    pub file: String,
    pub line: u32,
    pub reason: String,
}

/// RTL line / branch coverage of one simulation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineCoverageReport {
    /// The design carried line probes (synthesized for line coverage).
    pub instrumented: bool,
    /// Instrumented lines, sorted by file then line.
    pub lines: Vec<RtlLineHits>,
    /// Branches, sorted by file then line.
    pub branches: Vec<RtlBranchHits>,
    pub not_instrumented: Vec<RtlLineGap>,
    /// Rising edges sampled.
    pub edges: u64,
    /// Settled combinational states sampled.
    pub states: u64,
}

impl LineCoverageReport {
    pub fn lines_hit(&self) -> usize {
        self.lines.iter().filter(|l| l.hits > 0).count()
    }

    /// Line coverage percentage (100 when nothing is instrumented).
    pub fn line_pct(&self) -> f64 {
        pct(self.lines_hit(), self.lines.len())
    }

    pub fn arms_total(&self) -> usize {
        self.branches.iter().map(|b| b.arms.len()).sum()
    }

    pub fn arms_hit(&self) -> usize {
        self.branches.iter().map(RtlBranchHits::arms_hit).sum()
    }

    /// Branch-arm coverage percentage (100 when there are no branches).
    pub fn branch_pct(&self) -> f64 {
        pct(self.arms_hit(), self.arms_total())
    }

    /// Hit count of `file:line` (`None` when the line is not instrumented).
    pub fn line_hits(&self, file: &str, line: u32) -> Option<u64> {
        self.lines.iter().find(|l| l.line == line && l.file == file).map(|l| l.hits)
    }

    /// Single-line summary for logs.
    pub fn summary_line(&self) -> String {
        format!(
            "rtl_coverage lines={}/{} ({:.1}%) arms={}/{} ({:.1}%) not_instrumented={} edges={} states={}",
            self.lines_hit(),
            self.lines.len(),
            self.line_pct(),
            self.arms_hit(),
            self.arms_total(),
            self.branch_pct(),
            self.not_instrumented.len(),
            self.edges,
            self.states
        )
    }
}

fn pct(hit: usize, total: usize) -> f64 {
    if total == 0 {
        100.0
    } else {
        100.0 * hit as f64 / total as f64
    }
}

#[derive(Clone, Debug)]
struct Slot {
    /// Probe net names (one per bit; several instances share a probe).
    nets: Vec<String>,
    probe: LineProbe,
    /// Last observed value per net (assign probes).
    last: Vec<Option<bool>>,
}

#[derive(Clone, Debug)]
struct ArmAcc {
    label: String,
    line: u32,
    hits: u64,
}

#[derive(Clone, Debug)]
struct BranchAcc {
    file: String,
    line: u32,
    construct: String,
    arms: BTreeMap<u32, ArmAcc>,
}

/// Runtime collector behind [`LineCoverageReport`] (owned by [`crate::Sim`]).
#[derive(Clone, Debug, Default)]
pub struct LineCoverageCollector {
    slots: Vec<Slot>,
    always: Vec<LineProbe>,
    lines: BTreeMap<(String, u32), (RtlLineKind, u64)>,
    branches: BTreeMap<u64, BranchAcc>,
    gaps: Vec<RtlLineGap>,
    instrumented: bool,
    edges: u64,
    states: u64,
    /// The combinational state changed since it was last observed.
    pub(crate) dirty: bool,
}

/// Group key: probes for the same statement in several instances (or bits
/// of one assign) share their metadata.
fn probe_key(p: &LineProbe) -> String {
    format!("{:?}|{}|{}", p.kind, p.clocked, p.src)
}

impl LineCoverageCollector {
    /// Collect the probes of `d` (none when it was not synthesized for line
    /// coverage).
    pub fn from_design(d: &Design) -> Self {
        let mut by_key: BTreeMap<String, Slot> = BTreeMap::new();
        for n in &d.nets {
            let Some(probe) = LineProbe::read(&n.attrs) else { continue };
            let slot = by_key.entry(probe_key(&probe)).or_insert_with(|| Slot {
                nets: Vec::new(),
                probe: probe.clone(),
                last: Vec::new(),
            });
            slot.nets.push(resolve(n.name).to_string());
            slot.last.push(None);
        }
        let always = LineProbe::always_on(&d.attrs);
        let mut gaps: Vec<RtlLineGap> = d
            .attrs
            .get("lcov_dropped")
            .map(|v| {
                v.split('|')
                    .filter_map(|e| {
                        let (loc, why) = e.split_once('=')?;
                        let loc = SrcLoc::parse(loc)?;
                        Some(RtlLineGap { file: loc.file, line: loc.line, reason: why.to_string() })
                    })
                    .collect()
            })
            .unwrap_or_default();
        gaps.sort_by(|a, b| (&a.file, a.line, &a.reason).cmp(&(&b.file, b.line, &b.reason)));
        gaps.dedup();
        let instrumented = !by_key.is_empty() || !always.is_empty() || d.attrs.get("lcov").is_some();
        let mut c = Self {
            slots: by_key.into_values().collect(),
            always,
            gaps,
            instrumented,
            dirty: true,
            ..Self::default()
        };
        let probes: Vec<LineProbe> =
            c.slots.iter().map(|s| s.probe.clone()).chain(c.always.iter().cloned()).collect();
        for p in &probes {
            c.declare(p);
        }
        c
    }

    fn declare(&mut self, p: &LineProbe) {
        match &p.kind {
            LineProbeKind::Stmt | LineProbeKind::Assign => {
                let kind = if p.kind == LineProbeKind::Stmt { RtlLineKind::Statement } else { RtlLineKind::Assign };
                let e = self.lines.entry((p.src.file.clone(), p.src.line)).or_insert((kind, 0));
                e.0 = e.0.min(kind);
            }
            LineProbeKind::Arm { branch, construct, index, label, site } => {
                let b = self.branches.entry(*branch).or_insert_with(|| BranchAcc {
                    file: p.src.file.clone(),
                    line: *site,
                    construct: construct.clone(),
                    arms: BTreeMap::new(),
                });
                b.arms.entry(*index).or_insert_with(|| ArmAcc { label: label.clone(), line: p.src.line, hits: 0 });
            }
        }
    }

    /// True when the design carried any line probes.
    pub fn is_instrumented(&self) -> bool {
        self.instrumented
    }

    /// Every probe net (the simulator evaluates these per observation).
    pub fn probe_nets(&self) -> impl Iterator<Item = &str> {
        self.slots.iter().flat_map(|s| s.nets.iter().map(String::as_str))
    }

    /// Record one observation. `edge`: a rising edge is about to happen
    /// (clocked probes count); `state`: the settled combinational state is
    /// new (combinational / assign probes count). `value(net)` reads a probe
    /// net's current value.
    pub fn observe(&mut self, edge: bool, state: bool, mut value: impl FnMut(&str) -> bool) {
        if !self.instrumented || (!edge && !state) {
            return;
        }
        if edge {
            self.edges += 1;
        }
        if state {
            self.states += 1;
        }
        let counts = |p: &LineProbe| if p.clocked { edge } else { state };
        let mut lines_hit: BTreeSet<(String, u32)> = BTreeSet::new();
        let mut arms_hit: BTreeSet<(u64, u32)> = BTreeSet::new();
        let mut hit = |p: &LineProbe, lines_hit: &mut BTreeSet<(String, u32)>, arms_hit: &mut BTreeSet<(u64, u32)>| match &p.kind {
            LineProbeKind::Arm { branch, index, .. } => {
                arms_hit.insert((*branch, *index));
            }
            _ => {
                lines_hit.insert((p.src.file.clone(), p.src.line));
            }
        };
        for p in &self.always {
            if counts(p) {
                hit(p, &mut lines_hit, &mut arms_hit);
            }
        }
        for s in &mut self.slots {
            if !counts(&s.probe) {
                continue;
            }
            let mut any = false;
            for (i, net) in s.nets.iter().enumerate() {
                let v = value(net);
                if s.probe.kind == LineProbeKind::Assign {
                    let changed = s.last[i].is_some_and(|l| l != v);
                    s.last[i] = Some(v);
                    any |= changed;
                } else {
                    any |= v;
                }
            }
            if any {
                hit(&s.probe, &mut lines_hit, &mut arms_hit);
            }
        }
        for k in lines_hit {
            if let Some(e) = self.lines.get_mut(&k) {
                e.1 += 1;
            }
        }
        for (b, i) in arms_hit {
            if let Some(a) = self.branches.get_mut(&b).and_then(|b| b.arms.get_mut(&i)) {
                a.hits += 1;
            }
        }
    }

    pub fn report(&self) -> LineCoverageReport {
        let lines = self
            .lines
            .iter()
            .map(|((file, line), (kind, hits))| RtlLineHits { file: file.clone(), line: *line, kind: *kind, hits: *hits })
            .collect();
        let mut branches: Vec<RtlBranchHits> = self
            .branches
            .values()
            .map(|b| RtlBranchHits {
                file: b.file.clone(),
                line: b.line,
                construct: b.construct.clone(),
                arms: b.arms.values().map(|a| RtlArmHits { label: a.label.clone(), line: a.line, hits: a.hits }).collect(),
            })
            .collect();
        branches.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
        LineCoverageReport {
            instrumented: self.instrumented,
            lines,
            branches,
            not_instrumented: self.gaps.clone(),
            edges: self.edges,
            states: self.states,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::{CellKind, PortDir};

    /// `en` → probe net `p_t` (stmt under `if (en)`), plus an always-on stmt
    /// and an `if` with both arms as LUT-driven probe nets.
    fn probe_design() -> Design {
        let mut d = Design::new("t");
        d.add_port("en", PortDir::In);
        let mut add = |net: &str, init: u64, probe: LineProbe| {
            let cell = format!("c_{net}");
            d.add_cell(&cell, CellKind::Lut6 { init });
            d.connect("en", &cell, "I0");
            d.connect(net, &cell, "O");
            probe.write(&mut d.net_mut(net).unwrap().attrs);
        };
        let loc = |l| SrcLoc::new("d.sv", l);
        let arm = |index, label: &str| LineProbeKind::Arm { branch: 7, construct: "if".into(), index, label: label.into(), site: 3 };
        add("p_t", 0xAAAA_AAAA_AAAA_AAAA, LineProbe { src: loc(4), kind: LineProbeKind::Stmt, clocked: true });
        add("p_at", 0xAAAA_AAAA_AAAA_AAAA, LineProbe { src: loc(3), kind: arm(0, "true"), clocked: true });
        add("p_af", 0x5555_5555_5555_5555, LineProbe { src: loc(3), kind: arm(1, "false"), clocked: true });
        add("p_as", 0xAAAA_AAAA_AAAA_AAAA, LineProbe { src: loc(9), kind: LineProbeKind::Assign, clocked: false });
        LineProbe { src: loc(3), kind: LineProbeKind::Stmt, clocked: true }.write_always(&mut d.attrs, 1);
        d.attrs.set("lcov_dropped", "d.sv:12=condition reads a name that is not a module signal");
        d
    }

    #[test]
    fn clocked_probes_count_edges_and_assigns_count_changes() {
        let d = probe_design();
        let mut c = LineCoverageCollector::from_design(&d);
        assert!(c.is_instrumented());
        let mut en = false;
        // Three edges with en = 0, 0, 1; the assign copy changes once.
        for step in 0..3 {
            if step == 2 {
                en = true;
            }
            c.observe(true, true, |_| en);
        }
        // `p_af` inverts en; the closure returns the net's LUT value.
        let r = c.report();
        assert_eq!(r.edges, 3);
        assert_eq!(r.line_hits("d.sv", 3), Some(3), "always-on stmt runs every edge");
        assert_eq!(r.line_hits("d.sv", 4), Some(1));
        assert_eq!(r.line_hits("d.sv", 9), Some(1), "assign changed once");
        assert_eq!(r.branches.len(), 1);
        assert_eq!(r.branches[0].line, 3);
        assert_eq!(r.branches[0].arms.len(), 2);
        assert_eq!(r.not_instrumented.len(), 1);
        assert_eq!(r.not_instrumented[0].line, 12);
        assert!(r.summary_line().starts_with("rtl_coverage lines=3/3"));
    }

    #[test]
    fn combinational_probes_ignore_edges_without_a_new_state() {
        let d = probe_design();
        let mut c = LineCoverageCollector::from_design(&d);
        c.observe(true, false, |_| true);
        c.observe(true, false, |_| false);
        // Assign probe (combinational) never sampled: no change seen.
        assert_eq!(c.report().line_hits("d.sv", 9), Some(0));
        assert_eq!(c.report().states, 0);
    }

    const DUT: &str = "\
module lc_dut (
    input logic clk,
    input logic rst,
    input logic en,
    input logic m1,
    input logic m0,
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
    always_comb begin
        case ({m1, m0})
            2'd0: q = cnt;
            2'd1: q = ~cnt;
            default: q = 4'd0;
        endcase
    end
    assign flag = cnt[3];
endmodule
";

    fn sim_for(tag: &str) -> (crate::Sim, String) {
        let dir = std::env::temp_dir().join(format!("helion-sim-linecov-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lc_dut.sv");
        std::fs::write(&path, DUT).unwrap();
        let d = helion_sv::synth_sv_path_line_cov(&path).expect("synth");
        (crate::Sim::new(&d), path.display().to_string())
    }

    fn drive(sim: &mut crate::Sim, rst: bool, en: bool, mode: u8) {
        sim.drive_input("rst", rst).unwrap();
        sim.drive_input("en", en).unwrap();
        sim.drive_input("m0", mode & 1 == 1).unwrap();
        sim.drive_input("m1", mode & 2 == 2).unwrap();
    }

    #[test]
    fn simulated_netlist_reports_rtl_lines_and_arms() {
        let (mut sim, file) = sim_for("run");
        drive(&mut sim, true, false, 0);
        sim.step_posedge(10);
        drive(&mut sim, false, false, 0);
        sim.step_posedge(10);
        drive(&mut sim, false, true, 0);
        for _ in 0..3 {
            sim.step_posedge(10);
        }
        let _ = sim.sample("q_0");
        let r = sim.line_coverage_report();
        assert!(r.instrumented);
        assert_eq!(r.edges, 5);
        let hits = |l| r.line_hits(&file, l).unwrap_or_else(|| panic!("line {l} not instrumented: {r:#?}"));
        assert_eq!(hits(12), 5, "`if (rst)` runs every edge");
        assert_eq!(hits(13), 1, "reset assignment on the one reset edge");
        assert_eq!(hits(14), 4);
        assert_eq!(hits(15), 3);
        assert!(hits(18) > 0 && hits(19) > 0, "case + 2'd0 arm in every observed state");
        assert_eq!(hits(20), 0, "mode 1 never driven");
        assert_eq!(hits(21), 0, "default never taken");
        assert_eq!(hits(24), 0, "flag (cnt[3]) never changed in 3 counts");
        let arms = |line: u32| -> Vec<(String, u64)> {
            let b = r.branches.iter().find(|b| b.line == line).unwrap_or_else(|| panic!("no branch at {line}"));
            b.arms.iter().map(|a| (a.label.clone(), a.hits)).collect()
        };
        assert_eq!(arms(12), vec![("true".into(), 1), ("false".into(), 4)]);
        assert_eq!(arms(14), vec![("true".into(), 3), ("false".into(), 1)]);
        let case = arms(18);
        assert_eq!(case.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), ["2'd0", "2'd1", "default"]);
        assert!(case[0].1 > 0 && case[1].1 == 0 && case[2].1 == 0, "{case:?}");
        assert_eq!(r.arms_hit(), 5);
        assert_eq!(r.arms_total(), 7);
        assert!(r.not_instrumented.is_empty(), "{:?}", r.not_instrumented);
        // Probe nets are instrumentation, not DUT toggle-coverage nets.
        assert!(sim.coverage_report().net_toggles.iter().all(|t| !t.net.contains("__lcov")));
    }

    #[test]
    fn combinational_lines_follow_settled_states() {
        let (mut sim, file) = sim_for("comb");
        drive(&mut sim, true, false, 1);
        let _ = sim.sample("q_0");
        drive(&mut sim, true, false, 3);
        let _ = sim.sample("q_0");
        let r = sim.line_coverage_report();
        assert_eq!(r.edges, 0);
        assert_eq!(r.line_hits(&file, 20), Some(1), "mode 1 observed once");
        assert_eq!(r.line_hits(&file, 21), Some(1), "mode 3 hits default once");
        assert_eq!(r.line_hits(&file, 19), Some(0));
        assert_eq!(r.line_hits(&file, 13), Some(0), "clocked lines need an edge");
    }

    #[test]
    fn assign_line_counts_observed_value_changes() {
        let (mut sim, file) = sim_for("assign");
        drive(&mut sim, true, false, 0);
        sim.step_posedge(10);
        drive(&mut sim, false, true, 0);
        for _ in 0..9 {
            sim.step_posedge(10);
        }
        // cnt reaches 8 on the 8th count: flag (cnt[3]) rises once.
        assert_eq!(sim.line_coverage_report().line_hits(&file, 24), Some(1));
    }

    #[test]
    fn uninstrumented_design_reports_nothing() {
        let d = Design::new("plain");
        let mut c = LineCoverageCollector::from_design(&d);
        c.observe(true, true, |_| true);
        let r = c.report();
        assert!(!r.instrumented);
        assert!(r.lines.is_empty() && r.branches.is_empty());
        assert_eq!(r.edges, 0);
        assert_eq!(r.line_pct(), 100.0);
    }
}
