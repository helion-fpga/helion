//! Vectored (simulation-activity) power inside the Power report.
//!
//! Engine: `helion_sta::vcd_power` (the old IDE's VCD power analysis, now a
//! backend module): `parse_vcd` extracts per-signal toggles, then
//! `analyze_power` folds them over the last run's design into totals,
//! by-resource and by-hierarchy splits and the top toggling nets. The VCD is
//! either the last testbench run's dump ([`crate::views::tb::last_vcd`]) or
//! a file the user picks. Parsing and analysis run on a [`crate::bg`]
//! worker; a net row cross-probes the schematic.

use std::path::{Path, PathBuf};

use rune::{El, Tx, col, el, row};

use crate::state::{AppState, CenterPane, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::cad;
use crate::views::report_kit::{self as kit, header_row, num_cell, plain_row, select_row, table};
use helion_sta::vcd_power::VcdPowerReport;

const TASK: &str = "report_power_vcd";
const MW_W: f32 = 84.0;
const TOP_NETS: usize = 15;

/// Vectored power slice of the Power report.
#[derive(Clone, Debug, Default)]
pub struct VecPower {
    /// Power report shows the vectored tab.
    pub show: bool,
    pub result: Option<Result<VcdPowerReport, String>>,
    pub selected_net: Option<usize>,
}

/// Fastest constrained clock period (ps) from the last run, else 10 ns.
fn clock_period_ps(s: &AppState) -> u64 {
    s.timing
        .clocks
        .iter()
        .map(|c| c.period_ps)
        .filter(|p| *p > 0)
        .min()
        .unwrap_or(10_000)
}

/// Parse `vcd` and analyze it over `design` (worker body).
pub fn compute(
    design: &helion_ir::Design,
    part: &str,
    period_ps: u64,
    vcd: &Path,
) -> Result<VcdPowerReport, String> {
    let text =
        std::fs::read_to_string(vcd).map_err(|e| format!("read_vcd {}: {e}", vcd.display()))?;
    let act = helion_sta::vcd_power::parse_vcd(&text)?;
    let cfg = helion_sta::vcd_power::PowerConfig::for_part(part, 1000, 25, period_ps);
    Ok(helion_sta::vcd_power::analyze_power(
        design,
        &cfg,
        Some(&act),
        Some(vcd.display().to_string()),
    ))
}

/// Why vectored power cannot run (`None` = ready).
pub fn unavailable(s: &AppState) -> Option<&'static str> {
    if crate::bg::busy(s, TASK) {
        Some("Running…")
    } else if s.runs.live_design.is_none() {
        Some("Run Synthesis or Implementation first")
    } else {
        None
    }
}

/// Start the analysis of `vcd` on a worker.
pub fn run(s: &mut AppState, vcd: PathBuf) -> Result<String, String> {
    let design = s
        .runs
        .live_design
        .clone()
        .ok_or("report_power_vcd: no design (Run Synthesis or Implementation first)")?;
    let part = if s.runs.live_part.is_empty() {
        s.project.part.clone()
    } else {
        s.runs.live_part.clone()
    };
    let period = clock_period_ps(s);
    let started = crate::bg::spawn(
        s,
        TASK,
        move || compute(&design, &part, period, &vcd),
        |s, r: Result<VcdPowerReport, String>| {
            let line = match &r {
                Ok(p) => ConsoleLine {
                    text: format!(
                        "report_power -vcd TOTAL_MW={:.3} DYNAMIC_MW={:.3} coverage={:.1}% cycles={}",
                        p.total_mw,
                        p.dynamic_mw,
                        p.coverage_pct(),
                        p.cycles
                    ),
                    sev: MsgSev::Info,
                },
                Err(e) => ConsoleLine {
                    text: format!("ERROR: report_power_vcd: {e}"),
                    sev: MsgSev::Error,
                },
            };
            s.console.lines.push(line);
            s.reports_view.vpower.selected_net = None;
            s.reports_view.vpower.show = true;
            s.reports_view.vpower.result = Some(r);
        },
    );
    if started {
        Ok(format!("{TASK} running"))
    } else {
        Err(format!("{TASK} is already running"))
    }
}

/// Use the last testbench run's VCD.
pub fn run_last_sim(s: &mut AppState) -> Result<String, String> {
    let vcd = crate::views::tb::last_vcd(s)
        .ok_or("report_power_vcd: no simulation VCD yet (run a testbench with VCD on)")?;
    run(s, vcd)
}

fn pick_vcd(tx: &mut Tx) {
    let options = rune::FileDialogOptions {
        title: "Switching Activity (VCD)".into(),
        ..Default::default()
    }
    .filter("Value Change Dump", &["vcd"]);
    tx.open_file(options, |s: &mut AppState, result, _| match result {
        Ok(Some(p)) => {
            if let Err(e) = run(s, p) {
                s.console.lines.push(ConsoleLine {
                    text: format!("ERROR: {e}"),
                    sev: MsgSev::Error,
                });
            }
        }
        Ok(None) => {}
        Err(e) => s.console.lines.push(ConsoleLine {
            text: format!("ERROR: Load VCD: {e}"),
            sev: MsgSev::Error,
        }),
    });
}

/// `report_power_vcd ?file.vcd?` — vectored power from a VCD (default: the
/// last simulation's dump).
pub fn tcl_verb(s: &mut AppState, args: &[String]) -> Result<String, String> {
    s.navigate(CenterPane::Reports);
    s.reports_view.selected = super::ReportNav::Power;
    s.reports_view.vpower.show = true;
    match args.iter().find(|a| !a.starts_with('-')) {
        Some(p) => run(s, PathBuf::from(p)),
        None => run_last_sim(s),
    }
}

fn mw(v: f64) -> String {
    format!("{v:.3}")
}

/// The vectored section appended to the Power report body.
pub fn section(s: &AppState, t: &VivadoTheme) -> El {
    let reason = unavailable(s);
    let last = crate::views::tb::last_vcd(s);
    let last_reason = reason.or(last.is_none().then_some("No simulation VCD yet — run a testbench"));
    let use_last = kit::action_button(
        "reports-vpower-last",
        "Use Last Simulation",
        last_reason,
        t,
        |s: &mut AppState| {
            let _ = run_last_sim(s);
        },
    );
    let mut load = el()
        .key("reports-vpower-load")
        .role("button")
        .label("Load VCD…")
        .h(kit::ROW_H)
        .px(kit::PAD_X)
        .center_y()
        .radius(3.0)
        .fill(t.panel_bg)
        .border(1.0, t.border);
    load = match reason {
        None => load
            .focusable(true)
            .hover(t.selection)
            .on_press_tx(|_: &mut AppState, tx: &mut Tx| pick_vcd(tx))
            .child(rune::text("Load VCD…").font(VivadoTheme::FONT_UI).color(t.text)),
        Some(r) => load
            .disabled(true)
            .description(r)
            .help(r)
            .child(rune::text("Load VCD…").font(VivadoTheme::FONT_UI).color(t.text_faint)),
    };
    let head = row()
        .key("reports-vpower-actions")
        .w_full()
        .h(28.0)
        .items_center()
        .gap(8.0)
        .child(kit::section_title("Vectored power (simulation activity)", t))
        .child(el().grow(1.0))
        .child(use_last)
        .child(load);
    let mut out = col().key("reports-vpower").w_full().gap(8.0).child(head);
    match &s.reports_view.vpower.result {
        None if crate::bg::busy(s, TASK) => {
            out = out.child(kit::empty_state("reports-vpower-empty", "Analyzing switching activity…", None, t))
        }
        None => {
            out = out.child(kit::empty_state(
                "reports-vpower-empty",
                "Load a VCD (or use the last simulation) to replace default toggle rates with measured activity.",
                None,
                t,
            ))
        }
        Some(Err(e)) => out = out.child(kit::empty_state("reports-vpower-empty", e, None, t)),
        Some(Ok(p)) => out = out.child(report(s, p, t)),
    }
    out
}

fn report(s: &AppState, p: &VcdPowerReport, t: &VivadoTheme) -> El {
    let src = p.vcd_path.clone().unwrap_or_else(|| "vectorless".into());
    let kpis = row()
        .w_full()
        .gap(8.0)
        .child(cad::kpi("reports-vpower-kpi-total", "Total", format!("{} mW", mw(p.total_mw)), t.accent, format!("{} mW static", mw(p.static_mw)), t))
        .child(cad::kpi("reports-vpower-kpi-dynamic", "Dynamic", format!("{} mW", mw(p.dynamic_mw)), t.text, format!("clock α {:.2}", p.clock_alpha), t))
        .child(cad::kpi("reports-vpower-kpi-coverage", "Activity coverage", format!("{:.1}%", p.coverage_pct()), if p.coverage_pct() < 50.0 { t.warn } else { t.pass }, format!("{} / {} nets matched", p.nets_matched, p.nets_total), t))
        .child(cad::kpi("reports-vpower-kpi-cycles", "Cycles", p.cycles.to_string(), t.text, format!("{:.2} ns clock", p.clock_period_ps as f64 / 1000.0), t));
    let total = p.dynamic_mw.max(1e-12);
    let mut res = col().w_full();
    for (i, r) in p.by_resource.iter().enumerate() {
        res = res.child(
            plain_row(format!("reports-vpower-res-{i}"))
                .child(kit::fixed_cell(r.name.clone(), 110.0, t.text))
                .child(kit::bar((r.mw / total).clamp(0.0, 1.0) as f32, t.accent, t))
                .child(num_cell(mw(r.mw), MW_W, t.text)),
        );
    }
    let res_head = header_row(t)
        .child(el().w(110.0).child(kit::header_cell("Resource", t)))
        .child(kit::header_cell("Share", t).grow(1.0).min_w(0.0))
        .child(kit::header_num("Power (mW)", MW_W, t));
    let mut hier = col().w_full();
    for (i, h) in p.by_hierarchy.iter().enumerate() {
        hier = hier.child(
            plain_row(format!("reports-vpower-hier-{i}"))
                .child(kit::text_cell(h.name.clone(), 1.0, t.text))
                .child(num_cell(h.cells.to_string(), 64.0, t.text_dim))
                .child(num_cell(mw(h.mw), MW_W, t.text)),
        );
    }
    let hier_head = header_row(t)
        .child(kit::header_cell("Hierarchy", t).grow(1.0).min_w(0.0))
        .child(kit::header_num("Cells", 64.0, t))
        .child(kit::header_num("Power (mW)", MW_W, t));
    let sel = s.reports_view.vpower.selected_net;
    let mut nets = col().w_full();
    for (i, n) in p.top_nets.iter().take(TOP_NETS).enumerate() {
        let name = n.name.clone();
        nets = nets.child(
            select_row(format!("reports-vpower-net-{i}"), n.name.clone(), sel == Some(i), t, move |s: &mut AppState| {
                s.reports_view.vpower.selected_net = Some(i);
            })
            .child(kit::text_cell(n.name.clone(), 1.0, if n.is_clock { t.accent } else { t.text }))
            .child(num_cell(n.toggles.to_string(), 72.0, t.text))
            .child(num_cell(format!("{:.3}", n.activity), 64.0, t.text_dim))
            .child(num_cell(format!("{:.4}", n.mw), MW_W, t.text))
            .child(kit::link(format!("reports-vpower-probe-{i}"), "Schematic", t, move |s: &mut AppState| {
                s.reports_view.vpower.selected_net = Some(i);
                s.navigate(CenterPane::Schematic);
                crate::views::schematic::select(s, crate::views::schematic::SchemSel::Net(name.clone()));
            })),
        );
    }
    let nets_head = header_row(t)
        .child(kit::header_cell("Net", t).grow(1.0).min_w(0.0))
        .child(kit::header_num("Toggles", 72.0, t))
        .child(kit::header_num("α", 64.0, t))
        .child(kit::header_num("Power (mW)", MW_W, t))
        .child(el().w(64.0));
    let mut out = col()
        .w_full()
        .gap(8.0)
        .child(cad::kv("Activity from", kit::text_cell(src, 1.0, t.text_dim).key("reports-vpower-src"), 96.0, t))
        .child(kpis)
        .child(row().w_full().gap(8.0).items_start()
            .child(col().grow(1.0).min_w(0.0).gap(2.0).child(kit::section_title("By resource", t)).child(table(res_head, res, t)))
            .child(col().grow(1.0).min_w(0.0).gap(2.0).child(kit::section_title("By hierarchy", t)).child(table(hier_head, hier, t))))
        .child(kit::section_title("Top toggling nets", t))
        .child(table(nets_head, nets, t).key("reports-vpower-nets"));
    if p.unmatched_count > 0 {
        out = out.child(cad::dim(
            format!(
                "{} VCD signal(s) matched no design net (e.g. {}).",
                p.unmatched_count,
                p.unmatched_signals.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
            ),
            t,
        ).truncate());
    }
    for (i, w) in p.warnings.iter().take(4).enumerate() {
        let color = match w.severity {
            helion_sta::vcd_power::PowerSeverity::Critical => t.fail,
            helion_sta::vcd_power::PowerSeverity::Warning => t.warn,
            helion_sta::vcd_power::PowerSeverity::Info => t.text_dim,
        };
        out = out.child(kit::text_cell(w.message.clone(), 1.0, color).key(format!("reports-vpower-warn-{i}")));
    }
    out
}

/// Scene fixture: simulate the structural counter, dump a real VCD and run
/// the engine over it.
pub fn scene_fill(s: &mut AppState) {
    let d = helion_ir::Design::structural_counter();
    let vcd = helion_sim::vcd::dump_vcd(&d, 64, 10_000);
    let dir = std::env::temp_dir().join("helion-rune-scenes");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("counter_power.vcd");
    if std::fs::write(&path, vcd).is_ok() {
        s.reports_view.vpower.result = Some(compute(&d, "HL10T-C32-1", 10_000, &path));
        s.reports_view.vpower.selected_net = Some(0);
        s.reports_view.vpower.show = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vcd_power_runs_off_thread_over_the_live_design() {
        let mut s = AppState::demo();
        let d = helion_ir::Design::structural_counter();
        s.runs.live_design = Some(d.clone());
        let dir = std::env::temp_dir().join("helion-rune-test-vpower");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.vcd");
        std::fs::write(&path, helion_sim::vcd::dump_vcd(&d, 32, 10_000)).unwrap();
        tcl_verb(&mut s, &[path.display().to_string()]).unwrap();
        crate::bg::wait_all(&mut s);
        let p = s.reports_view.vpower.result.as_ref().unwrap().as_ref().unwrap();
        assert!(!p.vectorless);
        assert!(p.cycles > 0 && p.dynamic_mw > 0.0);
        assert!(!p.top_nets.is_empty());
        assert_eq!(s.reports_view.selected, super::super::ReportNav::Power);
    }

    #[test]
    fn missing_vcd_and_missing_design_are_reasons() {
        let mut s = AppState::demo();
        assert!(run_last_sim(&mut s).is_err() || s.runs.live_design.is_none());
        assert!(unavailable(&s).is_some());
        s.runs.live_design = Some(helion_ir::Design::structural_counter());
        run(&mut s, PathBuf::from("/nonexistent/x.vcd")).unwrap();
        crate::bg::wait_all(&mut s);
        assert!(matches!(s.reports_view.vpower.result, Some(Err(_))));
    }
}
