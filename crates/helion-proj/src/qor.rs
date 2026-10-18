//! Multi-run strategy exploration (`create_run` / `launch_runs`) and automated
//! QoR suggestion generation (`report_qor_suggestions` / `report_qor_assessment`,
//! UG906 / UG949 / UG986).

use crate::{ImplStrategy, Mode, ProjectFile, Session};
use helion_device::Device;
use helion_ir::{CellKind, Design, intern, resolve};
use helion_sta::{Constraints, create_clock, report_timing_routed_xdc};

/// Category of an automated QoR suggestion (`report_qor_suggestions`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QorCategory {
    Timing,
    Utilization,
    Congestion,
    Clocking,
    Pipelining,
    Strategy,
}

impl QorCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timing => "Timing",
            Self::Utilization => "Utilization",
            Self::Congestion => "Congestion",
            Self::Clocking => "Clocking",
            Self::Pipelining => "Pipelining",
            Self::Strategy => "Strategy",
        }
    }
}

/// One actionable suggestion produced by `report_qor_suggestions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QorSuggestion {
    /// Suggestion ID (e.g. `RQS_STRAT-1`, `RQS_PHYS-1`, `RQS_FANOUT-1`).
    pub id: String,
    pub category: QorCategory,
    /// `"Critical"` | `"Warning"` | `"Advisory"` | `"Info"`
    pub severity: String,
    /// Applicable stage (`"synth_design"`, `"place_design"`, `"phys_opt_design"`, `"route_design"`).
    pub applicable_for: String,
    /// Executable Vivado TCL command / property setting to apply this suggestion.
    pub tcl_command: String,
    /// Target design object or run name.
    pub target: String,
    /// Human-readable rationale.
    pub description: String,
    /// Estimated WNS improvement in picoseconds.
    pub expected_gain_ps: i64,
}

/// Complete QoR Assessment & Suggestions report (`report_qor_suggestions`).
#[derive(Clone, Debug)]
pub struct QorAssessmentReport {
    /// RQA score 1..=5 (Vivado UG906 scale: 5 = timing clean with margin, 1 = severe violation).
    pub rqa_score: u8,
    pub wns_ps: i64,
    pub tns_ps: i64,
    pub whs_ps: i64,
    pub endpoints: usize,
    pub lut_count: usize,
    pub ff_count: usize,
    pub iob_count: usize,
    pub lut_util_pct: u32,
    pub max_fanout: usize,
    pub high_fanout_net: String,
    pub overused_tiles: u32,
    pub current_strategy: ImplStrategy,
    pub recommended_strategy: ImplStrategy,
    pub suggestions: Vec<QorSuggestion>,
}

impl QorAssessmentReport {
    pub fn text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "report_qor_suggestions RQA_SCORE={}/5 WNS_PS={} TNS_PS={} WHS_PS={} CURRENT_STRATEGY={} RECOMMENDED_STRATEGY={} SUGGESTIONS={}\n",
            self.rqa_score,
            self.wns_ps,
            self.tns_ps,
            self.whs_ps,
            self.current_strategy.label(),
            self.recommended_strategy.label(),
            self.suggestions.len()
        ));
        out.push_str(&format!(
            "QoR Assessment Summary: LUT={} ({}%) FF={} IOB={} MAX_FANOUT={} ({}) OVERUSED_TILES={}\n",
            self.lut_count,
            self.lut_util_pct,
            self.ff_count,
            self.iob_count,
            self.max_fanout,
            if self.high_fanout_net.is_empty() {
                "-"
            } else {
                &self.high_fanout_net
            },
            self.overused_tiles
        ));
        for s in &self.suggestions {
            out.push_str(&format!(
                "  [{}] ({}/{}) target={} gain=+{}ps: {} | tcl: {}\n",
                s.id,
                s.category.as_str(),
                s.severity,
                s.target,
                s.expected_gain_ps,
                s.description,
                s.tcl_command
            ));
        }
        out
    }
}

/// Summary of one executed implementation run in a multi-strategy exploration.
#[derive(Clone, Debug)]
pub struct RunResultSummary {
    pub run_name: String,
    pub strategy: ImplStrategy,
    pub wns_ps: i64,
    pub tns_ps: i64,
    pub hold_slack_ps: i64,
    pub luts: usize,
    pub ffs: usize,
    pub iobs: usize,
    pub brams: usize,
    pub dsps: usize,
    pub route_iters: u32,
    pub overused: u32,
    pub max_hops: u32,
    pub bitstream_hash: Option<u32>,
    pub netlist_sha256: String,
    pub session: Session,
}

/// Multi-strategy exploration report across a set of runs.
#[derive(Clone, Debug)]
pub struct StrategyExplorationReport {
    pub runs: Vec<RunResultSummary>,
    pub best_run_index: usize,
}

impl StrategyExplorationReport {
    pub fn best_run(&self) -> Option<&RunResultSummary> {
        self.runs.get(self.best_run_index)
    }

    pub fn text(&self) -> String {
        let mut out = String::new();
        let best_name = self
            .best_run()
            .map(|r| r.run_name.as_str())
            .unwrap_or("-");
        let best_strat = self
            .best_run()
            .map(|r| r.strategy.label())
            .unwrap_or("-");
        let best_wns = self.best_run().map(|r| r.wns_ps).unwrap_or(0);
        out.push_str(&format!(
            "strategy_explorer runs={} best_run={} best_strategy={} best_wns_ps={}\n",
            self.runs.len(),
            best_name,
            best_strat,
            best_wns
        ));
        for (idx, r) in self.runs.iter().enumerate() {
            let marker = if idx == self.best_run_index { "*" } else { " " };
            out.push_str(&format!(
                "{marker} {:<16} {:<28} WNS_PS={:<7} TNS_PS={:<7} LUT={} FF={} IOB={} ITERS={} HASH={:#x}\n",
                r.run_name,
                r.strategy.label(),
                r.wns_ps,
                r.tns_ps,
                r.luts,
                r.ffs,
                r.iobs,
                r.route_iters,
                r.bitstream_hash.unwrap_or(0)
            ));
        }
        out
    }
}

fn max_non_clock_fanout(design: &Design) -> (usize, String) {
    let clk_pin = intern("CLK");
    let mut best_fanout = 0usize;
    let mut best_net = String::new();
    for net in &design.nets {
        let nname = resolve(net.name);
        let is_clk = nname.eq_ignore_ascii_case("clk")
            || nname.eq_ignore_ascii_case("clock")
            || net.endpoints.iter().any(|e| e.pin == clk_pin);
        if is_clk {
            continue;
        }
        let sinks = net.endpoints.len().saturating_sub(1);
        if sinks > best_fanout || (sinks == best_fanout && !best_net.is_empty() && nname < best_net.as_str()) {
            best_fanout = sinks;
            best_net = nname.to_string();
        } else if best_net.is_empty() && sinks > 0 {
            best_fanout = sinks;
            best_net = nname.to_string();
        }
    }
    (best_fanout, best_net)
}

/// Generate an automated QoR assessment and actionable strategy/constraint/opt suggestions.
pub fn report_qor_suggestions(
    session: &Session,
    dev: &Device,
    xdc: &Constraints,
) -> Result<QorAssessmentReport, String> {
    let design = session
        .design
        .as_ref()
        .ok_or("report_qor_suggestions: no synthesized design")?;

    let mut clks = xdc.clocks.clone();
    if clks.is_empty() {
        create_clock(&mut clks, "clk", 10_000, "clk");
    }

    let (wns_ps, tns_ps, whs_ps, endpoints) = if let Some(routed) = &session.routed {
        let t = report_timing_routed_xdc(design, routed, &clks, xdc)?;
        (t.wns_ps, t.tns_ps, t.hold_slack_ps, t.endpoints)
    } else if let Some(placed) = &session.placed {
        let t = helion_sta::report_timing_placed(design, placed, &clks)?;
        (t.wns_ps, t.tns_ps, t.hold_slack_ps, t.endpoints)
    } else {
        let t = helion_sta::report_timing_xdc(design, &clks, xdc)?;
        (t.wns_ps, t.tns_ps, t.hold_slack_ps, t.endpoints)
    };

    let (lut_count, ff_count, iob_count) = if let Some(pl) = &session.placed {
        let ffs = pl
            .packed
            .lutffs
            .iter()
            .filter(|l| !l.ff_cell.is_empty())
            .count();
        (pl.packed.lutffs.len(), ffs, pl.packed.iobs.len())
    } else if let Some(pk) = &session.packed {
        let ffs = pk.lutffs.iter().filter(|l| !l.ff_cell.is_empty()).count();
        (pk.lutffs.len(), ffs, pk.iobs.len())
    } else {
        let luts = design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count();
        let ffs = design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Hff))
            .count();
        let iobs = design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::IobOut))
            .count();
        (luts, ffs, iobs)
    };

    let cap_luts = dev.lut6_count().max(1) as usize;
    let lut_util_pct = ((lut_count * 100) / cap_luts) as u32;
    let (max_fanout, high_fanout_net) = max_non_clock_fanout(design);
    let overused_tiles = session.routed.as_ref().map(|r| r.overused).unwrap_or(0);
    let max_hops = session
        .routed
        .as_ref()
        .and_then(|r| r.iob_src.iter().map(|i| i.hops).max())
        .unwrap_or(0);

    let rqa_score = if overused_tiles > 0 || wns_ps < -1000 {
        1
    } else if wns_ps < 0 {
        2
    } else if wns_ps < 500 || lut_util_pct > 80 {
        3
    } else if wns_ps < 2000 || max_fanout > 32 {
        4
    } else {
        5
    };

    let mut suggestions = Vec::new();
    let mut recommended_strategy = session.strategy;

    // 1. Timing / Strategy suggestion
    if wns_ps < 0 {
        recommended_strategy = if max_hops > 6 {
            ImplStrategy::PerformanceNetDelayHigh
        } else {
            ImplStrategy::PerformanceExplore
        };
        suggestions.push(QorSuggestion {
            id: "RQS_STRAT-1".into(),
            category: QorCategory::Strategy,
            severity: "Critical".into(),
            applicable_for: "place_design,route_design".into(),
            tcl_command: format!(
                "set_property STRATEGY {} [get_runs impl_1]",
                recommended_strategy.label()
            ),
            target: "impl_1".into(),
            description: format!(
                "Negative WNS ({wns_ps} ps); switch implementation strategy from {} to {} for higher timing weight and PathFinder effort",
                session.strategy.label(),
                recommended_strategy.label()
            ),
            expected_gain_ps: 250,
        });
    } else if matches!(
        session.strategy,
        ImplStrategy::RuntimeOpt | ImplStrategy::FlowQuick
    ) {
        recommended_strategy = ImplStrategy::PerformanceExplore;
        suggestions.push(QorSuggestion {
            id: "RQS_STRAT-2".into(),
            category: QorCategory::Strategy,
            severity: "Advisory".into(),
            applicable_for: "place_design,route_design".into(),
            tcl_command: "set_property STRATEGY Performance_Explore [get_runs impl_1]".into(),
            target: "impl_1".into(),
            description: format!(
                "Run uses fast/non-timing-driven strategy {}; use Performance_Explore to maximize setup slack",
                session.strategy.label()
            ),
            expected_gain_ps: 160,
        });
    }

    // 2. Physical optimization suggestion
    if !matches!(
        session.strategy,
        ImplStrategy::PhysOpt
            | ImplStrategy::PerformanceExplore
            | ImplStrategy::PerformanceNetDelayHigh
    ) && (wns_ps < 2000 || max_fanout >= 4)
    {
        suggestions.push(QorSuggestion {
            id: "RQS_PHYS-1".into(),
            category: QorCategory::Timing,
            severity: if wns_ps < 0 { "Warning" } else { "Advisory" }.into(),
            applicable_for: "phys_opt_design".into(),
            tcl_command: "set_property STEPS.PHYS_OPT_DESIGN.IS_ENABLED true [get_runs impl_1]"
                .into(),
            target: "impl_1".into(),
            description:
                "Enable post-place physical optimization (phys_opt_design) to replicate high-fanout drivers and tighten critical routes"
                    .into(),
            expected_gain_ps: 120,
        });
    }

    // 3. High fanout net suggestion
    if max_fanout >= 8 && !high_fanout_net.is_empty() {
        suggestions.push(QorSuggestion {
            id: "RQS_FANOUT-1".into(),
            category: QorCategory::Pipelining,
            severity: "Warning".into(),
            applicable_for: "synth_design,phys_opt_design".into(),
            tcl_command: format!(
                "set_property MAX_FANOUT 8 [get_nets {}]",
                high_fanout_net
            ),
            target: high_fanout_net.clone(),
            description: format!(
                "Net {} drives {} loads; constrain MAX_FANOUT to force register/LUT replication",
                high_fanout_net, max_fanout
            ),
            expected_gain_ps: 90,
        });
    }

    // 4. Congestion / logic spread suggestion
    if overused_tiles > 0 || lut_util_pct >= 70 {
        recommended_strategy = ImplStrategy::CongestionSpreadLogicHigh;
        suggestions.push(QorSuggestion {
            id: "RQS_CONG-1".into(),
            category: QorCategory::Congestion,
            severity: "Warning".into(),
            applicable_for: "place_design,route_design".into(),
            tcl_command:
                "set_property STRATEGY Congestion_SpreadLogic_high [get_runs impl_1]".into(),
            target: "impl_1".into(),
            description: format!(
                "Routing congestion detected (overused={overused_tiles}, util={lut_util_pct}%); spread CLB placement with Congestion_SpreadLogic_high"
            ),
            expected_gain_ps: 140,
        });
    }

    // 5. Area optimization suggestion when dead/const cells or high LUT count exist
    let const0_luts = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { init: 0 }))
        .count();
    if const0_luts > 0 || lut_util_pct >= 50 {
        suggestions.push(QorSuggestion {
            id: "RQS_AREA-1".into(),
            category: QorCategory::Utilization,
            severity: "Advisory".into(),
            applicable_for: "synth_design,opt_design".into(),
            tcl_command:
                "set_property STEPS.SYNTH_DESIGN.ARGS.DIRECTIVE AreaOptimized_high [get_runs synth_1]"
                    .into(),
            target: "synth_1".into(),
            description: format!(
                "Netlist contains {const0_luts} constant-0 LUT(s) ({lut_count} total LUTs); run opt_design / Area_Explore to trim unused logic"
            ),
            expected_gain_ps: 60,
        });
    }

    // 6. Clock constraint completeness suggestion
    if xdc.clocks.is_empty() {
        suggestions.push(QorSuggestion {
            id: "RQS_CLK-1".into(),
            category: QorCategory::Clocking,
            severity: "Advisory".into(),
            applicable_for: "synth_design,place_design".into(),
            tcl_command: "create_clock -period 10.000 -name clk [get_ports clk]".into(),
            target: "clk".into(),
            description:
                "No explicit create_clock constraint in XDC (falling back to 10.000 ns default); add primary clock constraint for signoff STA"
                    .into(),
            expected_gain_ps: 50,
        });
    }

    // Ensure at least one baseline suggestion is always emitted for QoR exploration
    if suggestions.is_empty() {
        suggestions.push(QorSuggestion {
            id: "RQS_BASE-1".into(),
            category: QorCategory::Strategy,
            severity: "Info".into(),
            applicable_for: "place_design,route_design".into(),
            tcl_command: "set_property STRATEGY Performance_Explore [get_runs impl_1]".into(),
            target: "impl_1".into(),
            description: format!(
                "Design meets timing (WNS={wns_ps} ps); explore Performance_Explore or write_checkpoint for incremental compile baseline"
            ),
            expected_gain_ps: 40,
        });
    }

    Ok(QorAssessmentReport {
        rqa_score,
        wns_ps,
        tns_ps,
        whs_ps,
        endpoints,
        lut_count,
        ff_count,
        iob_count,
        lut_util_pct,
        max_fanout,
        high_fanout_net,
        overused_tiles,
        current_strategy: session.strategy,
        recommended_strategy,
        suggestions,
    })
}

/// Run one implementation strategy on a clone of `design` and return its [`RunResultSummary`].
pub fn execute_strategy_run(
    run_name: &str,
    design: &Design,
    dev: &Device,
    xdc: &Constraints,
    strategy: ImplStrategy,
    pblock: Option<&crate::PrjPblock>,
) -> Result<RunResultSummary, String> {
    let mut s = Session::new(Mode::Project);
    s.part = dev.part.clone();
    let mut d = design.clone();
    let _ = helion_sta::apply_xdc(&mut d, xdc);
    s.synth_design(d);
    s.strategy = strategy;
    if matches!(
        strategy,
        ImplStrategy::PhysOpt
            | ImplStrategy::PerformanceExplore
            | ImplStrategy::PerformanceNetDelayHigh
            | ImplStrategy::AreaExplore
    ) {
        let _ = s.opt_design_step()?;
    }
    if let Some(pb) = pblock.filter(|p| p.ranged) {
        s.place_pblock_in(dev, pb, strategy.place_opts())?;
    } else {
        s.place_design_with(dev, strategy.place_opts())?;
    }
    if matches!(
        strategy,
        ImplStrategy::PhysOpt
            | ImplStrategy::PerformanceExplore
            | ImplStrategy::PerformanceNetDelayHigh
    ) {
        let _ = s.phys_opt_design(dev)?;
    }
    s.route_design_with(dev, strategy.route_opts())?;
    s.write_bitstream(dev)?;

    let mut clks = xdc.clocks.clone();
    if clks.is_empty() {
        create_clock(&mut clks, "clk", 10_000, "clk");
    }
    let d_ref = s.design.as_ref().unwrap();
    let r_ref = s.routed.as_ref().unwrap();
    let timing = report_timing_routed_xdc(d_ref, r_ref, &clks, xdc)?;
    let pk = &r_ref.placed.packed;
    let ffs = pk.lutffs.iter().filter(|l| !l.ff_cell.is_empty()).count();
    let max_hops = r_ref.iob_src.iter().map(|i| i.hops).max().unwrap_or(0);
    let sha = crate::netlist_sha256(d_ref);
    let hash = s.blinky_hash();

    Ok(RunResultSummary {
        run_name: run_name.to_string(),
        strategy,
        wns_ps: timing.wns_ps,
        tns_ps: timing.tns_ps,
        hold_slack_ps: timing.hold_slack_ps,
        luts: pk.lutffs.len(),
        ffs,
        iobs: pk.iobs.len(),
        brams: pk.brams.len(),
        dsps: pk.macs.len(),
        route_iters: r_ref.pathfinder_iters,
        overused: r_ref.overused,
        max_hops,
        bitstream_hash: hash,
        netlist_sha256: sha,
        session: s,
    })
}

/// Explore multiple Vivado implementation strategies and rank by QoR (WNS, TNS, LUTs).
pub fn explore_strategies(
    design: &Design,
    dev: &Device,
    xdc: &Constraints,
    strategies: &[ImplStrategy],
) -> Result<StrategyExplorationReport, String> {
    if strategies.is_empty() {
        return Err("explore_strategies: no strategies specified".into());
    }
    let mut runs = Vec::with_capacity(strategies.len());
    for (idx, &strat) in strategies.iter().enumerate() {
        let name = format!("impl_{}_{}", idx + 1, strat.label());
        runs.push(execute_strategy_run(&name, design, dev, xdc, strat, None)?);
    }
    let best_run_index = select_best_run(&runs);
    Ok(StrategyExplorationReport {
        runs,
        best_run_index,
    })
}

/// Launch named project runs (or all `impl_runs` in `prj` when `run_names` is empty)
/// and update `session` to the best-QoR completed run.
pub fn launch_project_runs(
    session: &mut Session,
    prj: &ProjectFile,
    dev: &Device,
    xdc: &Constraints,
    run_names: &[&str],
) -> Result<Vec<RunResultSummary>, String> {
    let design = session
        .design
        .clone()
        .ok_or("launch_runs: no synthesized design (run synth_1 / synth_design first)")?;
    let pb = prj.pblocks.iter().find(|p| p.ranged);

    let mut targets: Vec<(String, ImplStrategy)> = Vec::new();
    if run_names.is_empty() {
        if prj.impl_runs.is_empty() {
            targets.push(("impl_1".into(), ImplStrategy::Default));
        } else {
            for r in &prj.impl_runs {
                let strat = ImplStrategy::parse(&r.strategy).unwrap_or(ImplStrategy::Default);
                targets.push((r.name.clone(), strat));
            }
        }
    } else {
        for &rn in run_names {
            if rn.eq_ignore_ascii_case("synth_1") {
                continue;
            }
            let strat = prj
                .impl_runs
                .iter()
                .find(|r| r.name.eq_ignore_ascii_case(rn))
                .and_then(|r| ImplStrategy::parse(&r.strategy).ok())
                .unwrap_or(ImplStrategy::Default);
            targets.push((rn.to_string(), strat));
        }
        if targets.is_empty() {
            targets.push(("impl_1".into(), ImplStrategy::Default));
        }
    }

    let mut results = Vec::with_capacity(targets.len());
    for (rname, strat) in targets {
        results.push(execute_strategy_run(
            &rname,
            &design,
            dev,
            xdc,
            strat,
            pb,
        )?);
    }
    if !results.is_empty() {
        let best_idx = select_best_run(&results);
        *session = results[best_idx].session.clone();
    }
    Ok(results)
}

fn select_best_run(runs: &[RunResultSummary]) -> usize {
    let mut best = 0usize;
    for (i, r) in runs.iter().enumerate().skip(1) {
        let b = &runs[best];
        let key_r = (
            r.overused == 0,
            r.wns_ps,
            r.tns_ps,
            std::cmp::Reverse(r.luts),
            std::cmp::Reverse(r.max_hops),
        );
        let key_b = (
            b.overused == 0,
            b.wns_ps,
            b.tns_ps,
            std::cmp::Reverse(b.luts),
            std::cmp::Reverse(b.max_hops),
        );
        if key_r > key_b {
            best = i;
        }
    }
    best
}
