//! On-demand re-analysis behind the report actions and `report_*` verbs.
//!
//! A finished run leaves its design ([`crate::runs::RunsState::live_design`])
//! and an [`AnalysisCtx`] (staged source, part, routed result). The report
//! actions re-run the real engines over exactly that data, re-reading the
//! run's sibling `.sdc`/`.xdc` (or the file the Constraints Wizard wrote) so
//! on-disk constraint edits take effect:
//! - STA: `helion_sta::report_timing_routed_xdc` (implementation) or
//!   `report_timing` (synthesis estimate) folded through
//!   [`crate::runs::timing_state_from`] (summary, clocks, groups, paths,
//!   clock networks, clock interaction, CDC, methodology);
//! - DRC: `helion_drc::check_routed` + `check_bitstream_signoff` on a routed
//!   run, else the pre-route `helion_drc::check_placed` deck on the Tcl
//!   session's `place_design` result;
//! - power: `helion_sta::report_power`;
//! - utilization: the routed packing, else `helion_pack::pack`.
//!
//! Every engine step is split into owned inputs (cloned on the UI thread),
//! a `compute_*` body that only reads them (so [`crate::jobs`] runs it on a
//! worker), and an `apply_*` that folds the result into [`AppState`]. The
//! synchronous `rerun_*` entry points are exactly inputs + compute + apply,
//! so the worker path and the scripted Tcl path produce identical state.
//!
//! The text dumps format the stored report state the same way the old IDE's
//! `report_*` verbs print (`ide.rs` report `text()` helpers), so the console
//! and the views always agree. Nothing here invents numbers: missing inputs
//! are errors with the run that would produce them.

use std::cell::RefCell;

use crate::runs::{AnalysisCtx, RunKind};
use crate::state::{
    AppState, ConsoleLine, DrcState, MsgSev, PowerState, TimingState, UtilState,
};
use crate::views::report_kit::ns3;

/// Engine lines collected while re-analyzing, flushed to the console after.
type Emitted = RefCell<Vec<ConsoleLine>>;

fn emitter(lines: &Emitted) -> impl Fn(String, MsgSev) + '_ {
    move |text, sev| lines.borrow_mut().push(ConsoleLine { text, sev })
}

fn flush(s: &mut AppState, lines: Emitted) {
    s.console.lines.extend(lines.into_inner());
}

/// Owned inputs of a timing/power/utilization re-analysis: the last
/// successful run's context plus its design (both `Send`).
#[derive(Clone)]
pub struct RunInputs {
    pub ctx: AnalysisCtx,
    pub design: helion_ir::Design,
}

/// The last successful run's context plus its design, or why re-analysis
/// cannot run.
pub fn run_inputs(s: &AppState) -> Result<RunInputs, String> {
    let ctx = s
        .runs
        .analysis
        .clone()
        .ok_or_else(|| "no analyzed run (launch_runs synth_1 or impl_1 first)".to_string())?;
    let design = s
        .runs
        .live_design
        .clone()
        .ok_or_else(|| "the last run kept no design (launch_runs synth_1 first)".to_string())?;
    Ok(RunInputs { ctx, design })
}

/// Why on-demand timing re-analysis is unavailable (`None` = available).
pub fn timing_unavailable(s: &AppState) -> Option<&'static str> {
    if s.runs.is_active() {
        Some("A run is in progress")
    } else if s.runs.analysis.is_none() || s.runs.live_design.is_none() {
        Some("Run Synthesis or Implementation first")
    } else {
        None
    }
}

/// Why on-demand DRC is unavailable (`None` = available).
pub fn drc_unavailable(s: &AppState) -> Option<&'static str> {
    if s.runs.is_active() {
        return Some("A run is in progress");
    }
    let routed = s.runs.analysis.as_ref().is_some_and(|a| a.routed.is_some())
        && s.runs.live_design.is_some();
    let placed = s.tcl.session.placed.is_some() && s.tcl.session.design.is_some();
    if routed || placed {
        None
    } else {
        Some("DRC needs a placed design — run Implementation first")
    }
}

// ---- timing + methodology ----

/// Fresh STA over the run's design with its reloaded constraints.
pub fn compute_timing(
    inp: &RunInputs,
    emit: &impl Fn(String, MsgSev),
) -> Result<TimingState, String> {
    let RunInputs { ctx, design } = inp;
    let (xdc, clks) = crate::runs::load_ctx_constraints(ctx, emit);
    match &ctx.routed {
        Some(routed) => {
            let t = helion_sta::report_timing_routed_xdc(design, routed, &clks, &xdc)?;
            let summary = helion_sta::report_timing_summary(&clks, &xdc, Some(&t));
            Ok(crate::runs::timing_state_from(
                &summary,
                Some(&t),
                design,
                Some(&routed.placed),
                &xdc,
            ))
        }
        None => {
            let t = helion_sta::report_timing(design, &clks).ok();
            let summary = helion_sta::report_timing_summary(&clks, &xdc, t.as_ref());
            Ok(crate::runs::timing_state_from(
                &summary,
                t.as_ref(),
                design,
                None,
                &xdc,
            ))
        }
    }
}

/// Replace the timing state, keeping the selected path when it survives.
pub fn apply_timing(s: &mut AppState, fresh: TimingState) {
    if s.timing_view
        .selected_path
        .is_some_and(|i| i >= fresh.paths.len())
    {
        s.timing_view.selected_path = None;
    }
    s.timing = fresh;
}

/// Replace only the methodology rows (or everything when no STA ran yet).
pub fn apply_methodology(s: &mut AppState, fresh: TimingState) {
    if s.timing.has_run {
        s.timing.methodology = fresh.methodology;
    } else {
        s.timing = fresh;
    }
    s.reports_view.selected_method = None;
}

/// Re-run STA (report_timing_summary) and replace the timing state.
pub fn rerun_timing(s: &mut AppState) -> Result<(), String> {
    let lines = Emitted::default();
    let result = run_inputs(s).and_then(|inp| compute_timing(&inp, &emitter(&lines)));
    flush(s, lines);
    apply_timing(s, result?);
    Ok(())
}

/// Re-run the methodology checks (they need the fresh STA result) and
/// replace only the methodology rows.
pub fn rerun_methodology(s: &mut AppState) -> Result<(), String> {
    let lines = Emitted::default();
    let result = run_inputs(s).and_then(|inp| compute_timing(&inp, &emitter(&lines)));
    flush(s, lines);
    apply_methodology(s, result?);
    Ok(())
}

// ---- DRC ----

/// Owned DRC inputs: the routed run, else the Tcl session's placement.
#[derive(Clone)]
pub enum DrcInputs {
    Routed {
        part: String,
        routed: helion_route::Routed,
        design: helion_ir::Design,
        pblocks: Vec<helion_proj::PrjPblock>,
    },
    Placed {
        part: String,
        design: helion_ir::Design,
        placed: helion_place::Placed,
        pblocks: Vec<helion_proj::PrjPblock>,
    },
}

/// `PBLK-1` rows: pblock members placed outside their pblock.
fn pblock_items(
    design: &helion_ir::Design,
    placed: &helion_place::Placed,
    pblocks: &[helion_proj::PrjPblock],
) -> Vec<crate::state::DrcItem> {
    if pblocks.is_empty() {
        return Vec::new();
    }
    let regions = helion_proj::pblock_regions(design, pblocks);
    helion_drc::check_pblocks(placed, &regions)
        .items
        .iter()
        .map(crate::runs::drc_item_from)
        .collect()
}

/// DRC inputs for the current state, or why DRC cannot run.
pub fn drc_inputs(s: &AppState) -> Result<DrcInputs, String> {
    let pblocks: Vec<helion_proj::PrjPblock> =
        s.device.pblocks.iter().filter(|p| p.ranged).cloned().collect();
    let routed_inputs = run_inputs(s)
        .ok()
        .and_then(|RunInputs { ctx, design }| ctx.routed.map(|r| (ctx.part, r, design)));
    if let Some((part, routed, design)) = routed_inputs {
        Ok(DrcInputs::Routed {
            part,
            routed,
            design,
            pblocks,
        })
    } else if let (Some(design), Some(placed)) =
        (s.tcl.session.design.as_ref(), s.tcl.session.placed.as_ref())
    {
        Ok(DrcInputs::Placed {
            part: s.project.part.clone(),
            design: design.clone(),
            placed: placed.clone(),
            pblocks,
        })
    } else {
        Err("no placed design (launch_runs impl_1 or place_design first)".into())
    }
}

/// The routed deck plus bitstream signoff, or the pre-route deck.
pub fn compute_drc(inp: &DrcInputs, emit: &impl Fn(String, MsgSev)) -> Result<DrcState, String> {
    match inp {
        DrcInputs::Routed {
            part,
            routed,
            design,
            pblocks,
        } => crate::runs::resolve_device(part, emit).map(|dev| {
            let drc = helion_drc::check_routed(design, routed, &dev);
            let mut items: Vec<crate::state::DrcItem> =
                drc.items.iter().map(crate::runs::drc_item_from).collect();
            items.extend(pblock_items(design, &routed.placed, pblocks));
            let signoff = helion_drc::check_bitstream_signoff(
                design,
                routed,
                &dev,
                &helion_drc::SignoffConfig::default(),
            );
            DrcState {
                items,
                bitgen_allowed: Some(signoff.bitgen_allowed),
                signoff_summary: signoff.summary_line(),
                unrouted: signoff.unrouted_nets,
                overused: signoff.overused_tiles,
                has_run: true,
                pre_route: false,
            }
        }),
        DrcInputs::Placed {
            part,
            design,
            placed,
            pblocks,
        } => crate::runs::resolve_device(part, emit).map(|dev| {
            let drc = helion_drc::check_placed(design, placed, &dev);
            let mut items: Vec<crate::state::DrcItem> =
                drc.items.iter().map(crate::runs::drc_item_from).collect();
            items.extend(pblock_items(design, placed, pblocks));
            DrcState {
                items,
                has_run: true,
                pre_route: true,
                ..DrcState::default()
            }
        }),
    }
}

pub fn apply_drc(s: &mut AppState, fresh: DrcState) {
    s.drc = fresh;
    s.reports_view.selected_drc = None;
}

/// Re-run DRC: the routed deck plus bitstream signoff on an implemented
/// run, else the pre-route deck on the Tcl session's placed design.
pub fn rerun_drc(s: &mut AppState) -> Result<(), String> {
    let lines = Emitted::default();
    let result = drc_inputs(s).and_then(|inp| compute_drc(&inp, &emitter(&lines)));
    flush(s, lines);
    apply_drc(s, result?);
    Ok(())
}

// ---- power + utilization ----

/// `report_power` over the run's design (placed when the run implemented it).
pub fn compute_power(
    inp: &RunInputs,
    emit: &impl Fn(String, MsgSev),
) -> Result<PowerState, String> {
    let RunInputs { ctx, design } = inp;
    let dev = crate::runs::resolve_device(&ctx.part, emit)?;
    let (xdc, clks) = crate::runs::load_ctx_constraints(ctx, emit);
    let placed = ctx.routed.as_ref().map(|r| &r.placed);
    let pr = helion_sta::report_power(
        &dev,
        Some(design),
        placed,
        &clks,
        &xdc.operating_conditions,
    );
    Ok(crate::runs::power_state_from(&pr, &xdc.operating_conditions))
}

/// Re-run `report_power` over the last run's design (placed when the run
/// implemented it).
pub fn rerun_power(s: &mut AppState) -> Result<(), String> {
    let lines = Emitted::default();
    let result = run_inputs(s).and_then(|inp| compute_power(&inp, &emitter(&lines)));
    flush(s, lines);
    s.power = result?;
    Ok(())
}

/// Utilization: the routed packing on an implemented run, else a fresh
/// `helion_pack::pack` of the synthesized design.
pub fn compute_util(inp: &RunInputs, emit: &impl Fn(String, MsgSev)) -> Result<UtilState, String> {
    let RunInputs { ctx, design } = inp;
    let dev = crate::runs::resolve_device(&ctx.part, emit)?;
    match (&ctx.routed, ctx.kind) {
        (Some(routed), _) => Ok(crate::runs::util_state_from(
            &routed.placed.packed,
            design,
            &dev,
        )),
        (None, RunKind::Synth | RunKind::Impl) => {
            let packed = helion_pack::pack(design, &dev)?;
            Ok(crate::runs::util_state_from(&packed, design, &dev))
        }
    }
}

/// Re-count utilization from the last run.
pub fn rerun_util(s: &mut AppState) -> Result<(), String> {
    let lines = Emitted::default();
    let result = run_inputs(s).and_then(|inp| compute_util(&inp, &emitter(&lines)));
    flush(s, lines);
    s.util = result?;
    Ok(())
}

/// Whether the utilization state holds a real count (device capacity set).
pub fn util_ready(s: &AppState) -> bool {
    s.util.luts_avail > 0 || !s.util.hier.is_empty()
}

/// Whether the power state holds a real report.
pub fn power_ready(s: &AppState) -> bool {
    !s.power.part.is_empty() || s.power.total_mw > 0
}

// ---- text dumps (console output of the report verbs) ----

fn slack_token(ps: Option<i64>) -> String {
    ps.map(|v| format!("{} ns", ns3(v)))
        .unwrap_or_else(|| "n/a".into())
}

/// `report_timing_summary` text: the Setup/Hold summary plus groups.
pub fn timing_summary_text(t: &TimingState) -> String {
    let mut lines = vec![format!(
        "Design Timing Summary: WNS {} TNS {} ns | WHS {} THS {} ns | failing setup {} hold {} | endpoints {}",
        slack_token(t.wns_ps),
        ns3(t.tns_ps),
        slack_token(t.whs_ps),
        ns3(t.ths_ps),
        t.failing_setup,
        t.failing_hold,
        t.endpoints
    )];
    for g in &t.groups {
        lines.push(format!(
            "  group {} ({} -> {}, {}) WNS {} TNS {} ns WHS {} THS {} ns endpoints {}",
            g.name,
            g.from,
            g.to,
            g.kind.label(),
            slack_token(g.wns_ps),
            ns3(g.tns_ps),
            slack_token(g.whs_ps),
            ns3(g.ths_ps),
            g.endpoints
        ));
    }
    let met = t.wns_ps.unwrap_or(0) >= 0
        && t.whs_ps.unwrap_or(0) >= 0
        && t.failing_setup == 0
        && t.failing_hold == 0;
    lines.push(
        if met {
            "All user specified timing constraints are met."
        } else {
            "Timing constraints are not met."
        }
        .into(),
    );
    lines.join("\n")
}

/// `report_clock_interaction` text: one line per From x To cell.
pub fn clock_interaction_text(t: &TimingState) -> String {
    if t.clock_interaction.is_empty() {
        return "no clocks — create_clock / report_clock_interaction".into();
    }
    let mut lines = vec![format!(
        "report_clock_interaction cells={}",
        t.clock_interaction.len()
    )];
    for c in &t.clock_interaction {
        lines.push(format!(
            "  {} -> {}: {} common {} ns requirement {} ns WNS {} paths {}",
            c.from,
            c.to,
            c.relation.label(),
            ns3(c.common_period_ps as i64),
            ns3(c.requirement_ps),
            slack_token(c.wns_ps),
            c.path_count
        ));
    }
    lines.join("\n")
}

/// `report_cdc` text: severity counts plus one line per crossing.
pub fn cdc_text(t: &TimingState) -> String {
    let count = |sev: &str| t.cdc.iter().filter(|v| v.severity == sev).count();
    let mut lines = vec![format!(
        "report_cdc crossings={} critical={} warning={} info={} safe={}",
        t.cdc.len(),
        count("Critical"),
        count("Warning"),
        count("Info"),
        count("Safe")
    )];
    for v in &t.cdc {
        lines.push(format!(
            "  {} -> {}: {} {} sync={} endpoints {} WNS {} ({})",
            v.from,
            v.to,
            v.severity,
            v.check,
            if v.sync { "yes" } else { "no" },
            v.endpoints,
            slack_token(v.wns_ps),
            v.relation
        ));
    }
    lines.join("\n")
}

/// `report_clock_networks` text: totals plus one line per clock tree.
pub fn clock_networks_text(t: &TimingState) -> String {
    if t.clock_networks.is_empty() {
        return "no clocks — create_clock / report_clock_networks".into();
    }
    let loads: usize = t.clock_networks.iter().map(|n| n.loads).sum();
    let buffers: usize = t.clock_networks.iter().map(|n| n.buffers).sum();
    let max_ins = t
        .clock_networks
        .iter()
        .map(|n| n.insertion_ps)
        .max()
        .unwrap_or(0);
    let mut lines = vec![format!(
        "report_clock_networks clocks={} loads={loads} buffers={buffers} max_insertion {} ns",
        t.clock_networks.len(),
        ns3(max_ins)
    )];
    for n in &t.clock_networks {
        lines.push(format!(
            "  {} period {} ns source {} net {} loads {} buffers {} fanout {} insertion {} ns",
            n.name,
            ns3(n.period_ps as i64),
            n.source,
            n.net,
            n.loads,
            n.buffers,
            n.fanout,
            ns3(n.insertion_ps)
        ));
    }
    lines.join("\n")
}

/// `report_power` text: totals, PVT, and the dynamic rails.
pub fn power_text(p: &PowerState) -> String {
    let mut lines = vec![format!(
        "report_power part={} total {:.3} W dynamic {:.3} W static {:.3} W junction {:.1} C confidence {}",
        if p.part.is_empty() { "n/a" } else { &p.part },
        p.total_mw as f64 / 1000.0,
        p.dynamic_mw as f64 / 1000.0,
        p.static_mw as f64 / 1000.0,
        p.junction_deci_c as f64 / 10.0,
        if p.confidence.is_empty() {
            "n/a"
        } else {
            &p.confidence
        }
    )];
    for (name, uw) in [
        ("clocks", p.clocks_uw),
        ("signals", p.signals_uw),
        ("logic", p.logic_uw),
        ("io", p.io_uw),
        ("bram", p.bram_uw),
        ("dsp", p.dsp_uw),
    ] {
        lines.push(format!("  {name} {uw} uW"));
    }
    lines.join("\n")
}

/// `report_methodology` text: one line per check.
pub fn methodology_text(t: &TimingState) -> String {
    if t.methodology.is_empty() {
        return "report_methodology checks=0 (no violations)".into();
    }
    let mut lines = vec![format!("report_methodology checks={}", t.methodology.len())];
    for v in &t.methodology {
        lines.push(format!(
            "  {} {} [{}] {}: {}",
            v.id, v.severity, v.category, v.objects, v.message
        ));
    }
    lines.join("\n")
}

/// `report_drc` text: verdict plus one line per violation.
pub fn drc_text(d: &DrcState) -> String {
    let deck = if d.pre_route {
        "pre-route (placed)"
    } else {
        "routed"
    };
    let mut lines = vec![format!(
        "report_drc deck={deck} violations={}",
        d.items.len()
    )];
    for v in &d.items {
        lines.push(format!(
            "  {} {} {}: {}",
            v.id,
            v.severity.label(),
            v.objects,
            v.message
        ));
    }
    if !d.signoff_summary.is_empty() {
        lines.push(d.signoff_summary.clone());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        CdcRow, ClockInteractionRow, ClockRel, DrcItem, DrcSev, TimingGroupKind, TimingGroupRow,
    };

    #[test]
    fn rerun_without_a_run_is_an_honest_error() {
        let mut s = AppState::demo();
        assert!(timing_unavailable(&s).is_some());
        assert!(drc_unavailable(&s).is_some());
        let err = rerun_timing(&mut s).unwrap_err();
        assert!(err.contains("launch_runs"), "{err}");
        assert!(rerun_drc(&mut s).is_err());
        assert!(rerun_power(&mut s).is_err());
        assert!(rerun_util(&mut s).is_err());
        assert!(!s.timing.has_run, "no numbers may appear without a run");
    }

    #[test]
    fn rerun_over_a_synth_context_runs_real_sta() {
        let mut s = AppState::demo();
        let src = "module c(input logic clk, output logic led);\n\
            logic [3:0] q;\n\
            always_ff @(posedge clk) q <= q + 1;\n\
            assign led = q[3];\nendmodule\n";
        let dir = std::env::temp_dir().join("helion-rune-analysis-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.sv");
        std::fs::write(&path, src).unwrap();
        let design = helion_sv::synth_sv_path(&path).expect("synth");
        s.runs.live_design = Some(design);
        s.runs.analysis = Some(AnalysisCtx {
            kind: RunKind::Synth,
            source: path,
            part: s.project.part.clone(),
            routed: None,
            constraints: None,
        });
        assert!(timing_unavailable(&s).is_none());
        rerun_timing(&mut s).expect("sta");
        assert!(s.timing.has_run);
        assert!(!s.timing.clocks.is_empty(), "implicit clock analyzed");
        assert!(
            !s.timing.clock_interaction.is_empty(),
            "interaction matrix computed"
        );
        rerun_util(&mut s).expect("pack");
        assert!(util_ready(&s));
        rerun_power(&mut s).expect("power");
        assert!(power_ready(&s));
        assert!(
            rerun_drc(&mut s).is_err(),
            "synth-only runs have no placement for DRC"
        );
    }

    #[test]
    fn text_dumps_carry_the_state_rows() {
        let mut t = TimingState {
            wns_ps: Some(-120),
            whs_ps: Some(40),
            endpoints: 3,
            failing_setup: 1,
            has_run: true,
            ..TimingState::default()
        };
        t.groups.push(TimingGroupRow {
            name: "clk".into(),
            from: "clk".into(),
            to: "clk".into(),
            kind: TimingGroupKind::Intra,
            wns_ps: Some(-120),
            tns_ps: -120,
            whs_ps: Some(40),
            ths_ps: 0,
            endpoints: 3,
        });
        t.clock_interaction.push(ClockInteractionRow {
            from: "clk".into(),
            to: "clk".into(),
            relation: ClockRel::Timed,
            common_period_ps: 10_000,
            requirement_ps: 10_000,
            wns_ps: Some(-120),
            path_count: 3,
        });
        t.cdc.push(CdcRow {
            from: "a".into(),
            to: "b".into(),
            severity: "Critical".into(),
            check: "CDC-1".into(),
            endpoints: 2,
            sync: false,
            wns_ps: None,
            relation: "Timed (unsafe)".into(),
        });
        let sum = timing_summary_text(&t);
        assert!(sum.contains("WNS -0.120 ns"), "{sum}");
        assert!(sum.contains("group clk"), "{sum}");
        assert!(sum.contains("not met"), "{sum}");
        assert!(clock_interaction_text(&t).contains("clk -> clk: Timed"));
        assert!(cdc_text(&t).contains("critical=1"));
        assert!(clock_networks_text(&t).starts_with("no clocks"));
        let d = DrcState {
            items: vec![DrcItem {
                id: "PLACE-1".into(),
                severity: DrcSev::Error,
                objects: "u0".into(),
                message: "bad".into(),
            }],
            has_run: true,
            pre_route: true,
            ..DrcState::default()
        };
        let txt = drc_text(&d);
        assert!(txt.contains("deck=pre-route"), "{txt}");
        assert!(txt.contains("PLACE-1 Error u0: bad"), "{txt}");
    }
}
