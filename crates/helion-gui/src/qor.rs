//! QoR suggestions engine — the Vivado `report_qor_suggestions` equivalent.
//!
//! Rule-based analysis over the post-route design and STA results. Each rule
//! is an entry in the [`qor_rules()`] registry: the IDE panel renders whatever
//! the registry returns, so adding a rule never touches UI code.
//!
//! Every suggestion cites real measured numbers (logic levels, fanout counts,
//! slack, router overuse) taken from the analysis inputs — never generic
//! advice. Estimated WNS gain is attached only where it is computable from
//! those numbers; otherwise it is `None` rather than invented.
//!
//! This module is pure logic (no egui). The IDE adapts its model into
//! [`QorInputs`] and paints the resulting [`QorReport`].

use crate::ide::{TimingPath, Utilization};
use helion_ir::{CellKind, Design};
use helion_route::Routed;
use helion_sta::{Clock, Constraints, TimingResult};

/// Severity bucket of a QoR suggestion, mirroring Vivado's report buckets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum QorSeverity {
    /// Blocks or threatens timing closure.
    Critical,
    /// Worth fixing before signoff.
    Warning,
    /// Improvement opportunity.
    Info,
}

impl QorSeverity {
    /// Short label used by the panel, Tcl text, and tests.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "Critical",
            Self::Warning => "Warning",
            Self::Info => "Info",
        }
    }
}

/// One-click action the IDE panel offers on a suggestion, beyond Locate.
/// Only attached where the IDE can actually perform the step — otherwise the
/// suggestion carries a `fix_hint` string and `action` stays `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QorAction {
    /// Navigate to the Design Runs tab so the user can launch a sweep.
    OpenRuns,
}

/// One actionable QoR suggestion: what is wrong, where, by how much, and how
/// to fix it. `objects` holds real design object names (cells, nets, paths)
/// the IDE can cross-probe; `gain_ps` is an estimated WNS improvement in
/// picoseconds, present only when computable from measured data.
#[derive(Clone, Debug)]
pub struct QorSuggestion {
    /// Rule id, e.g. `"QOR-LOGIC-1"`. Stable across runs; the panel and Tcl
    /// select suggestions by it.
    pub id: &'static str,
    pub severity: QorSeverity,
    /// Rule family, e.g. `"Logic Levels"`.
    pub category: &'static str,
    /// One-line summary with the key measured number.
    pub title: String,
    /// Plain-language explanation citing the measured numbers.
    pub explanation: String,
    /// Affected design objects (cell / net / path names) for Locate.
    pub objects: Vec<String>,
    /// Estimated WNS gain in ps, when computable. `None` means "not estimated",
    /// never a guess.
    pub gain_ps: Option<i64>,
    /// Concrete next step (constraint, option, or RTL change).
    pub fix_hint: String,
    /// One-click IDE action for the panel (e.g. jump to Design Runs), or
    /// `None` when the fix is manual work described by `fix_hint`.
    pub action: Option<QorAction>,
}

/// The full suggestion set from one analysis pass.
#[derive(Clone, Debug, Default)]
pub struct QorReport {
    pub suggestions: Vec<QorSuggestion>,
}

impl QorReport {
    /// Look up a suggestion by rule id.
    pub fn suggestion(&self, id: &str) -> Option<&QorSuggestion> {
        self.suggestions.iter().find(|s| s.id == id)
    }

    /// Suggestions at exactly this severity.
    pub fn at(&self, severity: QorSeverity) -> Vec<&QorSuggestion> {
        self.suggestions
            .iter()
            .filter(|s| s.severity == severity)
            .collect()
    }

    pub fn critical_count(&self) -> usize {
        self.at(QorSeverity::Critical).len()
    }

    pub fn warning_count(&self) -> usize {
        self.at(QorSeverity::Warning).len()
    }

    pub fn info_count(&self) -> usize {
        self.at(QorSeverity::Info).len()
    }

    /// `report_qor_suggestions` Tcl text: one header line plus one row per
    /// suggestion, mirroring the `report_methodology` text shape.
    pub fn text(&self) -> String {
        if self.suggestions.is_empty() {
            return "report_qor_suggestions suggestions=0 ok".into();
        }
        let mut lines = vec![format!(
            "report_qor_suggestions suggestions={} critical={} warning={} info={}",
            self.suggestions.len(),
            self.critical_count(),
            self.warning_count(),
            self.info_count()
        )];
        for s in &self.suggestions {
            let objs = if s.objects.is_empty() {
                "-".to_string()
            } else {
                s.objects.join(",")
            };
            let gain = s
                .gain_ps
                .map(|g| format!(" gain_ps={g}"))
                .unwrap_or_default();
            lines.push(format!(
                "{} {} {} {objs}{gain} :: {}",
                s.id,
                s.severity.as_str(),
                s.category,
                s.title
            ));
        }
        lines.join("\n")
    }
}

/// Everything a rule may inspect. Plain borrows of engine results — the IDE
/// fills this from its model; tests fill it from fixtures.
pub struct QorInputs<'a> {
    pub design: &'a Design,
    pub timing: Option<&'a TimingResult>,
    pub paths: &'a [TimingPath],
    pub routed: Option<&'a Routed>,
    pub constraints: &'a Constraints,
    /// Effective clocks (user SDC, else the built-in analysis clock).
    pub clocks: &'a [Clock],
    /// True once the user loads an SDC/XDC with a clock (not the built-in one).
    pub user_sdc: bool,
    pub utilization: Option<&'a Utilization>,
    /// Active clock period in ps (from [`QorInputs::clocks`] or the IDE default).
    pub period_ps: u64,
}

/// One QoR rule: a stable id plus a pure function over [`QorInputs`].
///
/// Rules are data — [`qor_rules()`] is the registry and [`analyze_qor`] runs
/// it. Adding a rule means appending one entry; the panel, Tcl, and tests
/// pick it up untouched.
pub struct QorRule {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub run: fn(&QorInputs) -> Vec<QorSuggestion>,
}

fn push(
    out: &mut Vec<QorSuggestion>,
    id: &'static str,
    severity: QorSeverity,
    category: &'static str,
    title: String,
    explanation: String,
    objects: Vec<String>,
    gain_ps: Option<i64>,
    fix_hint: &str,
    action: Option<QorAction>,
) {
    out.push(QorSuggestion {
        id,
        severity,
        category,
        title,
        explanation,
        objects,
        gain_ps,
        fix_hint: fix_hint.into(),
        action,
    });
}

/// Combinational cell arcs on a path = logic levels (Vivado's "logic levels").
fn logic_levels(path: &TimingPath) -> usize {
    path.pins
        .iter()
        .filter(|p| p.delay_type == "cell")
        .count()
}

/// QOR-LOGIC-1: deep combinational paths on (near-)critical timing paths.
/// Suggests pipelining; the gain estimate prices each removed LUT level at the
/// path's own measured average LUT delay.
fn rule_logic_levels(inp: &QorInputs) -> Vec<QorSuggestion> {
    const LEVEL_BUDGET: usize = 6;
    const LEVEL_TARGET: i64 = 4;
    let period = inp.period_ps.max(1) as i64;
    let mut out = Vec::new();
    for p in inp.paths {
        let levels = logic_levels(p);
        if levels < LEVEL_BUDGET {
            continue;
        }
        let severity = if p.slack_ps < 0 {
            QorSeverity::Critical
        } else if p.slack_ps < period / 10 {
            QorSeverity::Warning
        } else {
            continue;
        };
        let avg_lut_ps = p.logic_ps().max(1) / levels.max(1) as i64;
        let cut = (levels as i64 - LEVEL_TARGET).max(1);
        let gain = cut * avg_lut_ps;
        let mut objects = vec![p.name.clone()];
        objects.extend(p.cells.iter().take(8).cloned());
        push(
            &mut out,
            "QOR-LOGIC-1",
            severity,
            "Logic Levels",
            format!(
                "{}: {} logic levels, slack {} ps",
                p.name, levels, p.slack_ps
            ),
            format!(
                "Path {} -> {} spans {} LUT levels with {} ps of logic delay \
                 and {} ps of slack. Each LUT level costs ~{} ps on this path, \
                 so removing {} levels (≈ +{} ps) would close timing.",
                p.startpoint,
                p.endpoint,
                levels,
                p.logic_ps(),
                p.slack_ps,
                avg_lut_ps,
                cut,
                gain
            ),
            objects,
            Some(gain),
            "Pipeline the cone: register the path at a natural boundary \
             (state bit, bus slice, or arithmetic stage) to cut logic levels.",
            None,
        );
    }
    // Loudest first, deterministic: worst slack wins, then path name.
    out.sort_by(|a, b| {
        let sa = inp
            .paths
            .iter()
            .find(|p| p.name == a.objects[0])
            .map(|p| p.slack_ps)
            .unwrap_or(i64::MAX);
        let sb = inp
            .paths
            .iter()
            .find(|p| p.name == b.objects[0])
            .map(|p| p.slack_ps)
            .unwrap_or(i64::MAX);
        sa.cmp(&sb).then(a.title.cmp(&b.title))
    });
    out.truncate(4);
    out
}

/// Fanout of a net: load endpoints beyond the single driver.
fn net_fanout(net: &helion_ir::Net) -> usize {
    net.endpoints.len().saturating_sub(1)
}

/// QOR-FANOUT-1: high-fanout nets. Suggests replication / buffering; the gain
/// is load-dependent, so no ps estimate is attached.
fn rule_fanout(inp: &QorInputs) -> Vec<QorSuggestion> {
    const WARN_FANOUT: usize = 128;
    const INFO_FANOUT: usize = 32;
    let mut hits: Vec<(&str, usize)> = inp
        .design
        .nets
        .iter()
        .map(|n| (n.name.as_str(), net_fanout(n)))
        .filter(|(_, f)| *f >= INFO_FANOUT)
        .collect();
    hits.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    hits.truncate(8);
    let mut out = Vec::new();
    for (name, fanout) in hits {
        let severity = if fanout >= WARN_FANOUT {
            QorSeverity::Warning
        } else {
            QorSeverity::Info
        };
        push(
            &mut out,
            "QOR-FANOUT-1",
            severity,
            "High Fanout",
            format!("Net {name}: fanout {fanout}"),
            format!(
                "Net {name} drives {fanout} loads. High-fanout nets stretch \
                 across the die and pick up large route delay on every sink; \
                 they also serialize the router. Replicating the driver (or \
                 the driving register) per sink cluster shortens each branch."
            ),
            vec![name.to_string()],
            None,
            "Replicate the driver: duplicate the source register per load \
             cluster (max_fanout / manual replication), or buffer the net.",
            None,
        );
    }
    out
}

/// QOR-ROUTE-1: the router left overused nodes — real congestion, measured by
/// PathFinder, not a heuristic.
fn rule_congestion(inp: &QorInputs) -> Vec<QorSuggestion> {
    let mut out = Vec::new();
    if let Some(r) = inp.routed {
        if r.overused > 0 {
            push(
                &mut out,
                "QOR-ROUTE-1",
                QorSeverity::Critical,
                "Congestion",
                format!(
                    "Router left {} overused nodes ({} PathFinder iterations)",
                    r.overused, r.pathfinder_iters
                ),
                format!(
                    "PathFinder finished with {} overused routing nodes after {} \
                     iterations: demand exceeds supply in at least one region. \
                     Congested routes detour, and detours cost delay on exactly \
                     the nets timing cares about.",
                    r.overused, r.pathfinder_iters
                ),
                Vec::new(),
                None,
                "Relieve congestion: spread logic with placement pblock \
                 guidance, reduce LUT packing density, or rerun with a \
                 routability-focused strategy.",
                None,
            );
        }
    }
    out
}

/// QOR-CLOCK-1: sequential design with no user clock constraint. Fires only
/// when the analysis runs on the built-in clock rather than an SDC/XDC.
fn rule_unconstrained_clock(inp: &QorInputs) -> Vec<QorSuggestion> {
    let mut out = Vec::new();
    if inp.user_sdc {
        return out;
    }
    let pins = inp.design.pin_index();
    let mut clk_nets: Vec<String> = Vec::new();
    for ff in inp
        .design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
    {
        if let Some(net) = pins.net_on(&ff.name, "CLK") {
            if !clk_nets.iter().any(|n| n == net) {
                clk_nets.push(net.to_string());
            }
        }
    }
    if clk_nets.is_empty() {
        return out;
    }
    let period = inp.period_ps;
    push(
        &mut out,
        "QOR-CLOCK-1",
        QorSeverity::Warning,
        "Constraints",
        format!(
            "{} clock net{} without a create_clock constraint",
            clk_nets.len(),
            if clk_nets.len() == 1 { "" } else { "s" }
        ),
        format!(
            "The design has {} sequential clock net{} ({}) but no user clock \
             constraint: timing was analyzed against the built-in {} ps clock. \
             Reported slack is meaningless until the real clock is constrained.",
            clk_nets.len(),
            if clk_nets.len() == 1 { "" } else { "s" },
            clk_nets.join(", "),
            period
        ),
        clk_nets,
        None,
        "Add create_clock with the real period to the SDC/XDC, e.g. \
         `create_clock -period <ns> [get_ports clk]`, then rerun timing.",
        None,
    );
    out
}

/// QOR-MACRO-1: BRAM/DSP cascade dominating a timing path. Suggests
/// registering the macro boundary.
fn rule_macro_cascade(inp: &QorInputs) -> Vec<QorSuggestion> {
    let period = inp.period_ps.max(1) as i64;
    let mut out = Vec::new();
    for p in inp.paths {
        if p.delay_ps <= 0 || p.slack_ps >= period / 5 {
            continue;
        }
        let macro_cells: Vec<&str> = p
            .cells
            .iter()
            .filter(|c| {
                inp.design
                    .cells
                    .iter()
                    .find(|cell| cell.name == **c)
                    .is_some_and(|cell| {
                        matches!(cell.kind, CellKind::Bram18 | CellKind::Mac27 { .. })
                    })
            })
            .map(String::as_str)
            .collect();
        if macro_cells.is_empty() {
            continue;
        }
        let macro_ps: i64 = p
            .pins
            .iter()
            .filter(|pin| macro_cells.contains(&pin.cell.as_str()))
            .map(|pin| pin.incr_ps)
            .sum();
        if macro_ps * 10 < p.delay_ps * 3 {
            continue; // macro is under 30% of the path
        }
        let share = macro_ps * 100 / p.delay_ps;
        let mut objects = vec![p.name.clone()];
        objects.extend(macro_cells.iter().map(|s| (*s).to_string()));
        push(
            &mut out,
            "QOR-MACRO-1",
            QorSeverity::Warning,
            "Macro Cascade",
            format!(
                "{}: BRAM/DSP is {share}% of path delay ({} ps of {} ps)",
                p.name, macro_ps, p.delay_ps
            ),
            format!(
                "Path {} -> {} spends {} ps of its {} ps inside BRAM/DSP \
                 cascade ({}%). Macro delay is fixed silicon: the only way to \
                 buy it back is to register the macro boundary so the cascade \
                 sits in its own pipeline stage.",
                p.startpoint, p.endpoint, macro_ps, p.delay_ps, share
            ),
            objects,
            None,
            "Register the macro I/O: enable the BRAM output register / DSP \
             pipeline registers, or add a fabric register stage around the macro.",
            None,
        );
    }
    out.truncate(4);
    out
}

/// QOR-HOLD-1: hold violations from STA. Fix hints point at clock skew, the
/// usual culprit.
fn rule_hold(inp: &QorInputs) -> Vec<QorSuggestion> {
    let mut out = Vec::new();
    if let Some(t) = inp.timing {
        if t.hold_slack_ps < 0 {
            push(
                &mut out,
                "QOR-HOLD-1",
                QorSeverity::Critical,
                "Hold",
                format!("Hold violation: hold slack {} ps", t.hold_slack_ps),
                format!(
                    "STA reports {} ps of hold slack across {} timing endpoints: \
                     at least one short path arrives too early. Hold failures \
                     are silicon failures, not margins — they cannot be waived.",
                    t.hold_slack_ps, t.endpoints
                ),
                Vec::new(),
                None,
                "Check clock skew first (unbalanced clock nets, LUTs on clock \
                 paths), then let the router's hold-fix pass add delay to the \
                 short paths.",
                None,
            );
        }
    }
    out
}

/// QOR-IO-1: outputs driven by combinational fabric instead of a packed IOB
/// register — measured off the real router result.
fn rule_iob_packing(inp: &QorInputs) -> Vec<QorSuggestion> {
    let mut out = Vec::new();
    if let Some(r) = inp.routed {
        for io in r.iob_src.iter().filter(|io| !io.from_ff).take(8) {
            push(
                &mut out,
                "QOR-IOB-1",
                QorSeverity::Info,
                "IOB Packing",
                format!("Output net {} driven by combinational logic", io.net),
                format!(
                    "Output {} is driven by a combinational LUT {} hops from \
                     the pad, with no register packed into the IOB. A packed \
                     output register removes the fabric-to-pad route from the \
                     timing path.",
                    io.net, io.hops
                ),
                vec![io.net.clone()],
                None,
                "Pack the driving register into the IOB (IOB=TRUE on the \
                 output FF) so the pad is driven from the IOB register.",
                None,
            );
        }
    }
    out
}

/// QOR-NET-1: the worst setup path is dominated by route delay, not logic.
fn rule_net_dominated(inp: &QorInputs) -> Vec<QorSuggestion> {
    let mut out = Vec::new();
    let Some(p) = inp.paths.iter().min_by_key(|p| p.slack_ps) else {
        return out;
    };
    if p.delay_ps <= 0 {
        return out;
    }
    let net_ps = p.net_ps();
    let share = net_ps * 100 / p.delay_ps;
    if share <= 55 {
        return out;
    }
    // Halving the route delay is the computable, honest estimate.
    let gain = net_ps / 2;
    let mut top_nets: Vec<(&str, i64)> = Vec::new();
    for pin in p.pins.iter().filter(|p| p.delay_type == "net") {
        if let Some(e) = top_nets.iter_mut().find(|(n, _)| *n == pin.net) {
            e.1 += pin.incr_ps;
        } else {
            top_nets.push((pin.net.as_str(), pin.incr_ps));
        }
    }
    top_nets.sort_by(|a, b| b.1.cmp(&a.1));
    let mut objects = vec![p.name.clone()];
    objects.extend(top_nets.iter().take(4).map(|(n, _)| (*n).to_string()));
    let top_list = top_nets
        .iter()
        .take(3)
        .map(|(n, d)| format!("{n} ({d} ps)"))
        .collect::<Vec<_>>()
        .join(", ");
    push(
        &mut out,
        "QOR-NET-1",
        QorSeverity::Warning,
        "Route Delay",
        format!(
            "{}: route delay is {share}% of the critical path",
            p.name
        ),
        format!(
            "The worst setup path ({} -> {}, slack {} ps) spends {} ps of its \
             {} ps in routing ({}%). The logic is fine; the placement put the \
             endpoints far apart. Worst nets: {}.",
            p.startpoint, p.endpoint, p.slack_ps, net_ps, p.delay_ps, share, top_list
        ),
        objects,
        Some(gain),
        "Pull the endpoints together: floorplan the communicating logic into \
         one region, or ease the congestion the router is detouring around.",
        None,
    );
    out
}

/// QOR-TNS-1: many failing endpoints — a breadth problem, not one bad path.
fn rule_tns(inp: &QorInputs) -> Vec<QorSuggestion> {
    let mut out = Vec::new();
    if let Some(t) = inp.timing {
        if t.tns_ps < 0 && t.endpoints >= 8 {
            push(
                &mut out,
                "QOR-TNS-1",
                QorSeverity::Warning,
                "Failing Endpoints",
                format!(
                    "{} failing endpoints, TNS {} ps",
                    t.endpoints, t.tns_ps
                ),
                format!(
                    "{} timing endpoints fail with a total negative slack of \
                     {} ps. This is a breadth problem — no single path fix \
                     closes it; the implementation strategy needs to change.",
                    t.endpoints, t.tns_ps
                ),
                Vec::new(),
                None,
                "Sweep implementation strategies (design runs): try a \
                 timing-driven placement directive and compare WNS/TNS across \
                 runs before touching RTL.",
                None,
            );
        }
    }
    out
}

/// QOR-WNS-1: negative WNS on a narrow failure — the cheap move is a strategy
/// sweep, not RTL surgery. QOR-TNS-1 already covers the broad case (≥8 failing
/// endpoints), so this rule stays silent there to avoid double advice. The
/// panel renders a one-click "Open Design Runs" action on this suggestion.
fn rule_wns_strategy(inp: &QorInputs) -> Vec<QorSuggestion> {
    let mut out = Vec::new();
    let Some(t) = inp.timing else {
        return out;
    };
    if t.wns_ps >= 0 || t.endpoints >= 8 {
        return out;
    }
    let worst = inp.paths.iter().min_by_key(|p| p.slack_ps);
    let mut objects = Vec::new();
    let worst_txt = match worst {
        Some(p) => {
            objects.push(p.name.clone());
            format!("; worst path {} at {} ps", p.name, p.slack_ps)
        }
        None => String::new(),
    };
    push(
        &mut out,
        "QOR-WNS-1",
        QorSeverity::Critical,
        "Strategy",
        format!(
            "WNS {} ps across {} endpoint{} — try strategies before RTL",
            t.wns_ps,
            t.endpoints,
            if t.endpoints == 1 { "" } else { "s" }
        ),
        format!(
            "Timing fails with a worst negative slack of {} ps across only {} \
             failing endpoint{}. A narrow failure like this is often an \
             implementation-strategy problem (placement seed, packing, \
             routing effort), not an RTL problem{}. A strategy sweep \
             explores those axes in parallel before any RTL rewrite.",
            t.wns_ps,
            t.endpoints,
            if t.endpoints == 1 { "" } else { "s" },
            worst_txt
        ),
        objects,
        None,
        "Open Design Runs and launch a timing-driven strategy sweep; compare \
         WNS across seeds and directives before touching RTL.",
        Some(QorAction::OpenRuns),
    );
    out
}

/// QOR-UTIL-1: LUTFF occupancy high enough that QoR degrades.
fn rule_utilization(inp: &QorInputs) -> Vec<QorSuggestion> {
    let mut out = Vec::new();
    if let Some(u) = inp.utilization {
        let cap = u.lutff_cap.max(1) as usize;
        let pct = u.lutff * 100 / cap;
        if pct >= 80 {
            push(
                &mut out,
                "QOR-UTIL-1",
                QorSeverity::Info,
                "Utilization",
                format!("LUTFF utilization {pct}% ({}/{})", u.lutff, u.lutff_cap),
                format!(
                    "LUTFF utilization is {pct}% ({}/{}). Past ~80% the \
                     placer loses freedom and route delay climbs on every \
                     path; QoR work should start with area.",
                    u.lutff, u.lutff_cap
                ),
                Vec::new(),
                None,
                "Recover area first: run opt_design, map wide multiplexers and \
                 shift registers to BRAM, and re-check utilization before \
                 chasing picoseconds; or sweep area-oriented strategies in \
                 Design Runs.",
                Some(QorAction::OpenRuns),
            );
        }
    }
    out
}

/// The rule registry, in panel order. Add a rule here — nothing else changes.
pub fn qor_rules() -> Vec<QorRule> {
    vec![
        QorRule {
            id: "QOR-LOGIC-1",
            name: "High logic levels",
            category: "Logic Levels",
            run: rule_logic_levels,
        },
        QorRule {
            id: "QOR-FANOUT-1",
            name: "High-fanout nets",
            category: "High Fanout",
            run: rule_fanout,
        },
        QorRule {
            id: "QOR-ROUTE-1",
            name: "Routing congestion",
            category: "Congestion",
            run: rule_congestion,
        },
        QorRule {
            id: "QOR-CLOCK-1",
            name: "Unconstrained clock",
            category: "Constraints",
            run: rule_unconstrained_clock,
        },
        QorRule {
            id: "QOR-MACRO-1",
            name: "Macro cascade",
            category: "Macro Cascade",
            run: rule_macro_cascade,
        },
        QorRule {
            id: "QOR-HOLD-1",
            name: "Hold violations",
            category: "Hold",
            run: rule_hold,
        },
        QorRule {
            id: "QOR-IOB-1",
            name: "IOB packing",
            category: "IOB Packing",
            run: rule_iob_packing,
        },
        QorRule {
            id: "QOR-NET-1",
            name: "Net-dominated critical path",
            category: "Route Delay",
            run: rule_net_dominated,
        },
        QorRule {
            id: "QOR-TNS-1",
            name: "Many failing endpoints",
            category: "Failing Endpoints",
            run: rule_tns,
        },
        QorRule {
            id: "QOR-WNS-1",
            name: "Negative WNS: sweep strategies",
            category: "Strategy",
            run: rule_wns_strategy,
        },
        QorRule {
            id: "QOR-UTIL-1",
            name: "High utilization",
            category: "Utilization",
            run: rule_utilization,
        },
    ]
}

/// Run every registered rule over the inputs. Output is deterministic:
/// Critical first, then registry order.
pub fn analyze_qor(inp: &QorInputs) -> QorReport {
    let rules = qor_rules();
    let order: std::collections::HashMap<&str, usize> = rules
        .iter()
        .enumerate()
        .map(|(i, r)| (r.id, i))
        .collect();
    let mut suggestions = Vec::new();
    for rule in &rules {
        suggestions.extend((rule.run)(inp));
    }
    suggestions.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then(
                order
                    .get(a.id)
                    .copied()
                    .unwrap_or(usize::MAX)
                    .cmp(&order.get(b.id).copied().unwrap_or(usize::MAX)),
            )
            .then(a.title.cmp(&b.title))
    });
    QorReport { suggestions }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ide::TimingPathPin;
    use helion_ir::{CellKind, PortDir};

    fn empty_constraints() -> Constraints {
        Constraints::default()
    }

    fn empty_timing() -> TimingResult {
        TimingResult {
            clocks: Vec::new(),
            wns_ps: 1000,
            tns_ps: 0,
            endpoints: 4,
            r2r_ps: 2000,
            iob_ps: 3000,
            setup_ps: 100,
            hold_ps: 50,
            hold_slack_ps: 200,
            route_ps: 500,
            clk_net_ps: 0,
        }
    }

    /// Minimal sequential design: clk -> 2 FFs, one LUT between them.
    fn ff_design() -> Design {
        let mut d = Design::new("fx");
        d.add_port("clk", PortDir::In);
        d.add_cell("ff0", CellKind::Hff);
        d.add_cell("ff1", CellKind::Hff);
        d.add_cell("lut0", CellKind::Lut6 { init: 0 });
        d.connect("clk", "ff0", "CLK");
        d.connect("clk", "ff1", "CLK");
        d.connect("q0", "ff0", "Q");
        d.connect("q0", "lut0", "I0");
        d.connect("w0", "lut0", "O");
        d.connect("w0", "ff1", "D");
        d
    }

    fn pin(cell: &str, delay_type: &str, incr_ps: i64, net: &str, fanout: usize) -> TimingPathPin {
        TimingPathPin {
            cell: cell.into(),
            pin: format!("{cell}/O"),
            delay_type: delay_type.into(),
            incr_ps,
            path_ps: incr_ps,
            net: net.into(),
            fanout,
            location: String::new(),
        }
    }

    fn path(name: &str, slack_ps: i64, pins: Vec<TimingPathPin>) -> TimingPath {
        let delay_ps = pins.iter().map(|p| p.incr_ps).sum();
        TimingPath {
            name: name.into(),
            startpoint: "ff0".into(),
            endpoint: "ff1".into(),
            cells: vec!["ff0".into(), "lut0".into(), "ff1".into()],
            nets: vec!["q0".into(), "w0".into()],
            delay_ps,
            slack_ps,
            pins,
            group: "register".into(),
        }
    }

    fn inputs<'a>(
        design: &'a Design,
        timing: Option<&'a TimingResult>,
        paths: &'a [TimingPath],
        constraints: &'a Constraints,
    ) -> QorInputs<'a> {
        QorInputs {
            design,
            timing,
            paths,
            routed: None,
            constraints,
            clocks: &[],
            user_sdc: false,
            utilization: None,
            period_ps: 10_000,
        }
    }

    #[test]
    fn logic_levels_fires_on_deep_critical_path() {
        let d = ff_design();
        let c = empty_constraints();
        let t = empty_timing();
        // 7 LUT levels, negative slack.
        let mut pins = vec![pin("ff0", "clk", 100, "clk", 2)];
        for i in 0..7 {
            pins.push(pin(&format!("lut{i}"), "cell", 300, &format!("w{i}"), 1));
            pins.push(pin(&format!("lut{i}"), "net", 120, &format!("w{i}"), 1));
        }
        pins.push(pin("ff1", "setup", 80, "", 0));
        let paths = vec![path("ff0->ff1", -150, pins)];
        let inp = inputs(&d, Some(&t), &paths, &c);
        let s = rule_logic_levels(&inp);
        assert_eq!(s.len(), 1, "deep failing path must fire");
        assert_eq!(s[0].severity, QorSeverity::Critical);
        assert!(s[0].title.contains("7 logic levels"), "{}", s[0].title);
        assert!(s[0].explanation.contains("-150"), "{}", s[0].explanation);
        // logic_ps = 7*300 + 80 (setup) = 2180; avg = 2180/7 = 311;
        // (7-4) levels * 311ps = 933ps gain.
        assert_eq!(s[0].gain_ps, Some(933));
        assert!(s[0].objects.contains(&"ff0->ff1".to_string()));
    }

    #[test]
    fn logic_levels_silent_on_shallow_or_healthy_paths() {
        let d = ff_design();
        let c = empty_constraints();
        let t = empty_timing();
        // Only 2 levels, negative slack: silent (not a logic-depth problem).
        let pins = vec![
            pin("ff0", "clk", 100, "clk", 2),
            pin("lut0", "cell", 300, "w0", 1),
            pin("lut1", "cell", 300, "w1", 1),
            pin("ff1", "setup", 80, "", 0),
        ];
        let paths = vec![path("ff0->ff1", -50, pins)];
        let inp = inputs(&d, Some(&t), &paths, &c);
        assert!(rule_logic_levels(&inp).is_empty());

        // 7 levels but healthy slack: silent.
        let mut pins = vec![pin("ff0", "clk", 100, "clk", 2)];
        for i in 0..7 {
            pins.push(pin(&format!("lut{i}"), "cell", 300, &format!("w{i}"), 1));
        }
        pins.push(pin("ff1", "setup", 80, "", 0));
        let paths = vec![path("ff0->ff1", 5000, pins)];
        let inp = inputs(&d, Some(&t), &paths, &c);
        assert!(rule_logic_levels(&inp).is_empty());
    }

    #[test]
    fn fanout_fires_on_hot_net() {
        let mut d = Design::new("fan");
        d.add_cell("src", CellKind::Hff);
        d.connect("hot", "src", "Q");
        for i in 0..40 {
            d.add_cell(format!("sink{i}"), CellKind::Hff);
            d.connect("hot", format!("sink{i}"), "D");
        }
        let c = empty_constraints();
        let paths: Vec<TimingPath> = Vec::new();
        let inp = inputs(&d, None, &paths, &c);
        let s = rule_fanout(&inp);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].severity, QorSeverity::Info);
        assert!(s[0].title.contains("hot"), "{}", s[0].title);
        assert!(s[0].title.contains("40"), "{}", s[0].title);
        assert_eq!(s[0].gain_ps, None, "fanout gain is not estimated");
    }

    #[test]
    fn fanout_silent_on_small_design() {
        let d = ff_design();
        let c = empty_constraints();
        let paths: Vec<TimingPath> = Vec::new();
        let inp = inputs(&d, None, &paths, &c);
        assert!(rule_fanout(&inp).is_empty());
    }

    #[test]
    fn unconstrained_clock_fires_without_sdc() {
        let d = ff_design();
        let c = empty_constraints();
        let paths: Vec<TimingPath> = Vec::new();
        let inp = inputs(&d, None, &paths, &c);
        let s = rule_unconstrained_clock(&inp);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].severity, QorSeverity::Warning);
        assert!(s[0].objects.contains(&"clk".to_string()), "{:?}", s[0].objects);
        assert!(s[0].fix_hint.contains("create_clock"));
    }

    #[test]
    fn unconstrained_clock_silent_with_sdc_or_no_ffs() {
        let d = ff_design();
        let c = empty_constraints();
        let paths: Vec<TimingPath> = Vec::new();
        // user_sdc = true silences the rule.
        let inp = QorInputs {
            user_sdc: true,
            ..inputs(&d, None, &paths, &c)
        };
        assert!(rule_unconstrained_clock(&inp).is_empty());

        // No FFs, no clocks: silent.
        let mut comb = Design::new("comb");
        comb.add_cell("lut0", CellKind::Lut6 { init: 0 });
        let inp = inputs(&comb, None, &paths, &c);
        assert!(rule_unconstrained_clock(&inp).is_empty());
    }

    #[test]
    fn hold_violation_fires() {
        let d = ff_design();
        let c = empty_constraints();
        let mut t = empty_timing();
        t.hold_slack_ps = -45;
        let paths: Vec<TimingPath> = Vec::new();
        let inp = inputs(&d, Some(&t), &paths, &c);
        let s = rule_hold(&inp);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].severity, QorSeverity::Critical);
        assert!(s[0].explanation.contains("-45"), "{}", s[0].explanation);

        t.hold_slack_ps = 10;
        let inp = inputs(&d, Some(&t), &paths, &c);
        assert!(rule_hold(&inp).is_empty());
    }

    #[test]
    fn net_dominated_fires_on_routing_heavy_path() {
        let d = ff_design();
        let c = empty_constraints();
        let t = empty_timing();
        // 200ps logic, 1800ps net = 90% route.
        let pins = vec![
            pin("ff0", "clk", 100, "clk", 2),
            pin("lut0", "cell", 100, "w0", 1),
            pin("lut0", "net", 1800, "w0", 1),
            pin("ff1", "setup", 80, "", 0),
        ];
        let paths = vec![path("ff0->ff1", -200, pins)];
        let inp = inputs(&d, Some(&t), &paths, &c);
        let s = rule_net_dominated(&inp);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].severity, QorSeverity::Warning);
        assert!(s[0].title.contains("86%"), "{}", s[0].title);
        assert_eq!(s[0].gain_ps, Some(900), "halving 1800ps net delay");
        assert!(s[0].objects.contains(&"w0".to_string()));
    }

    #[test]
    fn macro_cascade_fires_on_bram_heavy_path() {
        let mut d = Design::new("m");
        d.add_cell("ff0", CellKind::Hff);
        d.add_cell("ram0", CellKind::Bram18);
        d.add_cell("ff1", CellKind::Hff);
        d.connect("clk", "ff0", "CLK");
        d.connect("clk", "ff1", "CLK");
        let c = empty_constraints();
        let t = empty_timing();
        let pins = vec![
            pin("ff0", "clk", 100, "clk", 2),
            pin("ram0", "cell", 2500, "dout", 1),
            pin("ram0", "net", 300, "dout", 1),
            pin("ff1", "setup", 80, "", 0),
        ];
        let mut p = path("ff0->ff1", -400, pins);
        p.cells = vec!["ff0".into(), "ram0".into(), "ff1".into()];
        let paths = vec![p];
        let inp = inputs(&d, Some(&t), &paths, &c);
        let s = rule_macro_cascade(&inp);
        assert_eq!(s.len(), 1);
        assert!(s[0].objects.contains(&"ram0".to_string()));
        assert!(s[0].fix_hint.contains("BRAM output register"));
    }

    #[test]
    fn tns_rule_fires_on_broad_failure() {
        let d = ff_design();
        let c = empty_constraints();
        let mut t = empty_timing();
        t.tns_ps = -5000;
        t.endpoints = 12;
        let paths: Vec<TimingPath> = Vec::new();
        let inp = inputs(&d, Some(&t), &paths, &c);
        let s = rule_tns(&inp);
        assert_eq!(s.len(), 1);
        assert!(s[0].title.contains("12 failing endpoints"), "{}", s[0].title);

        t.endpoints = 3;
        let inp = inputs(&d, Some(&t), &paths, &c);
        assert!(rule_tns(&inp).is_empty(), "narrow failure is not a TNS rule");
    }

    #[test]
    fn utilization_rule_fires_when_dense() {
        let d = ff_design();
        let c = empty_constraints();
        let paths: Vec<TimingPath> = Vec::new();
        let u = Utilization {
            lutff: 7000,
            lutff_cap: 8192,
            iob: 8,
            iob_cap: 64,
            bram: 0,
            bram_cap: 16,
            dsp: 0,
            dsp_cap: 8,
        };
        let inp = QorInputs {
            utilization: Some(&u),
            ..inputs(&d, None, &paths, &c)
        };
        let s = rule_utilization(&inp);
        assert_eq!(s.len(), 1);
        assert!(s[0].title.contains("85%"), "{}", s[0].title);

        let sparse = Utilization {
            lutff: 4,
            ..u
        };
        let inp = QorInputs {
            utilization: Some(&sparse),
            ..inputs(&d, None, &paths, &c)
        };
        assert!(rule_utilization(&inp).is_empty());
    }

    #[test]
    fn wns_strategy_fires_on_narrow_failure() {
        let d = ff_design();
        let c = empty_constraints();
        let mut t = empty_timing();
        t.wns_ps = -320;
        t.endpoints = 3;
        let paths: Vec<TimingPath> = Vec::new();
        let inp = inputs(&d, Some(&t), &paths, &c);
        let s = rule_wns_strategy(&inp);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].id, "QOR-WNS-1");
        assert_eq!(s[0].severity, QorSeverity::Critical);
        assert_eq!(s[0].action, Some(QorAction::OpenRuns));
        assert!(s[0].explanation.contains("-320"), "{}", s[0].explanation);
        assert!(s[0].explanation.chars().any(|c| c.is_ascii_digit()));
    }

    #[test]
    fn wns_strategy_silent_when_healthy_or_broad() {
        let d = ff_design();
        let c = empty_constraints();
        let mut t = empty_timing();
        let paths: Vec<TimingPath> = Vec::new();

        // Healthy WNS: silent.
        t.wns_ps = 120;
        t.endpoints = 0;
        let inp = inputs(&d, Some(&t), &paths, &c);
        assert!(rule_wns_strategy(&inp).is_empty());

        // Broad failure belongs to QOR-TNS-1, not this rule.
        t.wns_ps = -5000;
        t.endpoints = 12;
        let inp = inputs(&d, Some(&t), &paths, &c);
        assert!(rule_wns_strategy(&inp).is_empty());

        // No timing at all: silent.
        let inp = inputs(&d, None, &paths, &c);
        assert!(rule_wns_strategy(&inp).is_empty());
    }

    #[test]
    fn registry_is_stable_and_report_sorts_critical_first() {
        let ids: Vec<&str> = qor_rules().iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            vec![
                "QOR-LOGIC-1",
                "QOR-FANOUT-1",
                "QOR-ROUTE-1",
                "QOR-CLOCK-1",
                "QOR-MACRO-1",
                "QOR-HOLD-1",
                "QOR-IOB-1",
                "QOR-NET-1",
                "QOR-TNS-1",
                "QOR-WNS-1",
                "QOR-UTIL-1",
            ]
        );
        // Second call returns the same registry: rules are data.
        let ids2: Vec<&str> = qor_rules().iter().map(|r| r.id).collect();
        assert_eq!(ids, ids2);

        let d = ff_design();
        let c = empty_constraints();
        let mut t = empty_timing();
        t.hold_slack_ps = -10; // Critical
        let paths: Vec<TimingPath> = Vec::new();
        let inp = inputs(&d, Some(&t), &paths, &c);
        let report = analyze_qor(&inp);
        // hold (Critical) sorts before unconstrained-clock (Warning).
        assert_eq!(report.suggestions[0].id, "QOR-HOLD-1");
        assert!(report.text().starts_with("report_qor_suggestions suggestions=2"));
        assert!(report.text().contains("QOR-CLOCK-1"));
    }
}
