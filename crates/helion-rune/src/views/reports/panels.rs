//! Report-pane building blocks added on top of the Utilization / Power /
//! DRC / Methodology / QoR bodies: the per-report action bar (real
//! recomputation), gating empty states with one real call-to-action,
//! utilization KPI cards, the power Rails and Block Power & Utilization
//! tables, and the row-detail panels (full message, objects, fix hint).
//!
//! Mirrors the old IDE's Power tab rails/blocks grids (`helion-ide.rs:
//! 7497-7600`), Utilization KPI cards (`helion-ide.rs:8499-8533`), and the
//! DRC/methodology selection panes. Fix hints come from the engine rule
//! catalog (`helion_drc::lookup_rule` / `signoff_rule_info`) only when the
//! rule id resolves; otherwise the row is not given a fabricated hint.

use rune::{El, col, el, row, text};

use super::{QorSnapshot, ReportNav};
use crate::analysis;
use crate::state::{AppState, ConsoleLine, MsgSev, RunStatus};
use crate::theme::VivadoTheme;
use crate::views::report_kit::{self as kit, header_row, num_cell, plain_row, table};
use crate::views::summary::pct;

/// Numeric column widths.
const UW_W: f32 = 96.0;
const SHARE_W: f32 = 56.0;
const COUNT_W: f32 = 80.0;

/// Which run produced the shown numbers.
pub fn stage_label(s: &AppState) -> &'static str {
    match s.runs.analysis.as_ref().map(|a| a.routed.is_some()) {
        Some(true) => "Post-implementation",
        Some(false) => "Post-synthesis estimate",
        None if s.flow.impl_run == RunStatus::Complete => "Post-implementation",
        None if s.flow.synth == RunStatus::Complete => "Post-synthesis estimate",
        None => "Last run",
    }
}

/// Whether the selected report has real data to show.
pub fn report_ready(s: &AppState, nav: ReportNav) -> bool {
    match nav {
        ReportNav::Utilization => analysis::util_ready(s),
        ReportNav::Power => analysis::power_ready(s),
        ReportNav::Drc => s.drc.has_run,
        ReportNav::Methodology => s.timing.has_run,
        ReportNav::Qor => {
            s.reports_view.qor.is_some() || s.tcl.design.is_some() || s.runs.live_design.is_some()
        }
    }
}

fn console_line(s: &mut AppState, text: String, sev: MsgSev) {
    s.console.lines.push(ConsoleLine { text, sev });
}

/// Report Utilization: recount from the last run and log the result.
fn report_util(s: &mut AppState) {
    match analysis::rerun_util(s) {
        Ok(()) => {
            let u = &s.util;
            let line = format!(
                "report_utilization refreshed: LUT {}/{} FF {}/{} IOB {}/{} BRAM {}/{} DSP {}/{}",
                u.luts_used,
                u.luts_avail,
                u.regs_used,
                u.regs_avail,
                u.io_used,
                u.io_avail,
                u.bram_used,
                u.bram_avail,
                u.dsp_used,
                u.dsp_avail
            );
            console_line(s, line, MsgSev::Info);
        }
        Err(e) => console_line(s, format!("ERROR: report_utilization: {e}"), MsgSev::Error),
    }
}

/// Action bar for the selected report: what produced it plus the one real
/// re-run action (disabled with the reason when its inputs are missing).
pub fn action_bar(s: &AppState, nav: ReportNav, t: &VivadoTheme) -> El {
    let design_missing =
        (s.tcl.design.is_none() && s.runs.live_design.is_none()).then_some("Run Synthesis first");
    let (label, key, reason, stage): (&'static str, &'static str, Option<&str>, String) = match nav
    {
        ReportNav::Utilization => (
            "Report Utilization",
            "reports-action-util",
            analysis::timing_unavailable(s),
            stage_label(s).to_string(),
        ),
        ReportNav::Power => (
            "Report Power",
            "reports-action-power",
            analysis::timing_unavailable(s),
            format!("{} · vectorless", stage_label(s)),
        ),
        ReportNav::Drc => (
            "Report DRC",
            "reports-action-drc",
            analysis::drc_unavailable(s),
            if s.drc.pre_route {
                "Pre-route DRC (placed design)".to_string()
            } else {
                "Routed DRC".to_string()
            },
        ),
        ReportNav::Methodology => (
            "Report Methodology",
            "reports-action-method",
            analysis::timing_unavailable(s),
            stage_label(s).to_string(),
        ),
        ReportNav::Qor => (
            "Re-run QoR Analysis",
            "reports-qor-refresh",
            design_missing,
            "Evaluated on the elaborated design".to_string(),
        ),
    };
    let button = kit::action_button(key, label, reason, t, move |s: &mut AppState| match nav {
        ReportNav::Utilization => report_util(s),
        ReportNav::Power => crate::tcl::eval_command(s, "report_power"),
        ReportNav::Drc => crate::tcl::eval_command(s, "report_drc"),
        ReportNav::Methodology => crate::tcl::eval_command(s, "report_methodology"),
        ReportNav::Qor => super::refresh_qor(s),
    });
    row()
        .key("reports-actions")
        .w_full()
        .h(28.0)
        .items_center()
        .gap(8.0)
        .child(
            text(stage)
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .truncate()
                .key("reports-stage"),
        )
        .child(el().grow(1.0))
        .child(button)
}

/// Empty state for a report without data: a run-in-progress notice, or one
/// sentence plus the one call-to-action that launches the run producing it.
pub fn not_ready(s: &AppState, nav: ReportNav, t: &VivadoTheme) -> El {
    let running = s.flow.impl_run == RunStatus::Running || s.flow.synth == RunStatus::Running;
    let body = if running {
        kit::empty_state(
            "reports-run-status",
            "A run is in progress — follow it in the Tcl Console.",
            None,
            t,
        )
    } else {
        let (sentence, cta) = match nav {
            ReportNav::Drc => (
                "DRC needs a placed design.",
                kit::link(
                    "reports-run-impl",
                    "Run Implementation",
                    t,
                    |s: &mut AppState| crate::runs::launch(s, crate::runs::RunKind::Impl),
                ),
            ),
            ReportNav::Utilization => (
                "Utilization appears once the design is synthesized.",
                synth_link(t),
            ),
            ReportNav::Power => (
                "Power appears once the design is synthesized.",
                synth_link(t),
            ),
            ReportNav::Methodology => (
                "Methodology checks run once the design is synthesized.",
                synth_link(t),
            ),
            ReportNav::Qor => ("QoR needs a synthesized design.", synth_link(t)),
        };
        kit::empty_state("reports-cta", sentence, Some(cta), t)
    };
    col()
        .key("reports-no-run")
        .w_full()
        .padding(8.0)
        .child(body)
}

fn synth_link(t: &VivadoTheme) -> El {
    kit::link(
        "reports-run-synth",
        "Run Synthesis",
        t,
        |s: &mut AppState| crate::runs::launch(s, crate::runs::RunKind::Synth),
    )
}

/// Four utilization KPI cards (first four site rows) with occupancy bars.
pub fn kpi_cards(rows: &[(&'static str, u32, u32)], t: &VivadoTheme) -> El {
    let mut cards = row().key("reports-util-kpis").w_full().gap(8.0);
    for (i, (name, used, avail)) in rows.iter().take(4).enumerate() {
        let frac = if *avail == 0 {
            0.0
        } else {
            *used as f32 / *avail as f32
        };
        cards = cards.child(
            col()
                .key(format!("reports-util-kpi-{i}"))
                .grow(1.0)
                .min_w(0.0)
                .gap(4.0)
                .padding(8.0)
                .fill(t.panel_bg)
                .border(1.0, t.border)
                .child(
                    row()
                        .w_full()
                        .items_center()
                        .gap(4.0)
                        .child(
                            text(*name)
                                .font(VivadoTheme::FONT_UI)
                                .color(t.text_dim)
                                .grow(1.0)
                                .min_w(0.0)
                                .truncate(),
                        )
                        .child(
                            text(format!("{}%", pct(*used, *avail)))
                                .font(VivadoTheme::FONT_TITLE)
                                .color(t.accent),
                        ),
                )
                .child(
                    text(format!("{used} / {avail}"))
                        .font(VivadoTheme::FONT_MONO.mono())
                        .color(t.text)
                        .truncate(),
                )
                .child(row().w_full().child(kit::bar(frac, t.accent, t))),
        );
    }
    cards
}

/// Microwatts with a rail fallback to whole milliwatts (fixtures that only
/// carry the mW totals).
fn rail_uw(uw: u32, mw: u32) -> u32 {
    if uw > 0 { uw } else { mw.saturating_mul(1000) }
}

/// Power Rails table: `Rail | Power (µW) | Share` with share-of-total bars.
pub fn rails_table(s: &AppState, t: &VivadoTheme) -> El {
    let p = &s.power;
    let total = rail_uw(p.total_uw, p.total_mw);
    let rails: [(&str, u32); 9] = [
        ("Total", total),
        ("Static", rail_uw(p.static_uw, p.static_mw)),
        ("Dynamic", rail_uw(p.dynamic_uw, p.dynamic_mw)),
        ("Clocks", p.clocks_uw),
        ("Signals", p.signals_uw),
        ("Logic", p.logic_uw),
        ("I/O", p.io_uw),
        ("BRAM", p.bram_uw),
        ("DSP", p.dsp_uw),
    ];
    let head = header_row(t)
        .child(el().w(80.0).child(kit::header_cell("Rail", t)))
        .child(kit::header_num("Power (µW)", UW_W, t))
        .child(kit::header_cell("Share", t).grow(1.0).min_w(0.0))
        .child(kit::header_num("%", SHARE_W, t));
    let mut body = col().w_full();
    for (i, (name, uw)) in rails.into_iter().enumerate() {
        let frac = if total == 0 {
            0.0
        } else {
            uw as f32 / total as f32
        };
        let bar_ink = match i {
            0 => t.text_dim,
            1 => t.text_faint,
            _ => t.accent,
        };
        body = body.child(
            plain_row(format!("reports-power-row-{i}"))
                .child(kit::fixed_cell(
                    name,
                    80.0,
                    if i == 0 { t.text } else { t.text_dim },
                ))
                .child(num_cell(uw.to_string(), UW_W, t.text))
                .child(kit::bar(frac, bar_ink, t))
                .child(num_cell(
                    format!("{}%", pct(uw, total)),
                    SHARE_W,
                    t.text_dim,
                )),
        );
    }
    col()
        .key("reports-power-table")
        .w_full()
        .gap(2.0)
        .child(kit::section_title("Rails (vectorless STA)", t))
        .child(table(head, body, t))
}

/// Block Power & Utilization: `Block | Used | Available | Occupancy |
/// Power (µW)` from the run's utilization and power rails.
pub fn blocks_table(s: &AppState, t: &VivadoTheme) -> El {
    let u = &s.util;
    let p = &s.power;
    let blocks: [(&str, u32, u32, Option<u32>); 6] = [
        ("Slice LUTs", u.luts_used, u.luts_avail, Some(p.logic_uw)),
        ("Slice Registers", u.regs_used, u.regs_avail, None),
        ("Bonded IOB", u.io_used, u.io_avail, Some(p.io_uw)),
        ("Block RAM Tile", u.bram_used, u.bram_avail, Some(p.bram_uw)),
        ("DSPs", u.dsp_used, u.dsp_avail, Some(p.dsp_uw)),
        ("BUFGCTRL", u.bufg_used, u.bufg_avail, Some(p.clocks_uw)),
    ];
    let head = header_row(t)
        .child(kit::header_cell("Block", t).grow(1.0).min_w(0.0))
        .child(kit::header_num("Used", COUNT_W, t))
        .child(kit::header_num("Available", COUNT_W, t))
        .child(kit::header_cell("Occupancy", t).grow(1.0).min_w(0.0))
        .child(kit::header_num("Power (µW)", UW_W, t));
    let mut body = col().w_full();
    for (i, (name, used, avail, uw)) in blocks.into_iter().enumerate() {
        let frac = if avail == 0 {
            0.0
        } else {
            used as f32 / avail as f32
        };
        let power = match uw {
            Some(v) => num_cell(v.to_string(), UW_W, t.text),
            None => {
                num_cell("–", UW_W, t.text_faint).help("Registers share the Slice LUTs logic rail")
            }
        };
        body = body.child(
            plain_row(format!("reports-power-block-{i}"))
                .child(kit::text_cell(name, 1.0, t.text))
                .child(num_cell(used.to_string(), COUNT_W, t.text))
                .child(num_cell(avail.to_string(), COUNT_W, t.text_dim))
                .child(
                    row()
                        .grow(1.0)
                        .min_w(0.0)
                        .items_center()
                        .gap(6.0)
                        .child(kit::bar(frac, t.accent, t))
                        .child(
                            el().w(36.0).child(
                                text(format!("{}%", pct(used, avail)))
                                    .font(VivadoTheme::FONT_MONO.mono())
                                    .color(t.text_dim)
                                    .w_full()
                                    .align(rune::Align::End),
                            ),
                        ),
                )
                .child(power),
        );
    }
    col()
        .key("reports-power-blocks")
        .w_full()
        .gap(2.0)
        .child(kit::section_title("Block Power & Utilization", t))
        .child(table(head, body, t))
}

/// Engine catalog hint for a DRC/methodology rule id:
/// `(title, description, tcl fix, rtl fix)`.
fn rule_hint(id: &str) -> Option<(String, String, String, String)> {
    if let Some(r) = helion_drc::lookup_rule(id) {
        return Some((
            r.title,
            r.description,
            r.tcl_remediation_template,
            r.rtl_remediation_hint,
        ));
    }
    helion_drc::signoff_rule_info(id).map(|r| {
        (
            r.title.to_string(),
            r.description.to_string(),
            r.downgrade_tcl_hint,
            String::new(),
        )
    })
}

/// Detail rows shared by the DRC and methodology panels.
fn violation_rows(
    id: &str,
    severity: (&str, rune::Color),
    category: Option<&str>,
    objects: &str,
    message: &str,
    t: &VivadoTheme,
) -> Vec<(&'static str, El)> {
    let mut rows: Vec<(&'static str, El)> =
        vec![("Severity", kit::wrap_value(severity.0, severity.1))];
    if let Some(c) = category.filter(|c| !c.is_empty()) {
        rows.push(("Category", kit::wrap_value(c, t.text)));
    }
    rows.push((
        "Objects",
        kit::wrap_mono(if objects.is_empty() { "–" } else { objects }, t.text),
    ));
    rows.push(("Message", kit::wrap_value(message, t.text)));
    if let Some((title, desc, tcl, rtl)) = rule_hint(id) {
        if !title.is_empty() {
            rows.push(("Rule", kit::wrap_value(title, t.text)));
        }
        if !desc.is_empty() {
            rows.push(("Description", kit::wrap_value(desc, t.text_dim)));
        }
        if !tcl.is_empty() {
            // Catalog templates name the flagged objects `<OBJ>`; fill them
            // in so the fix reads as a runnable command.
            let tcl = if objects.is_empty() {
                tcl
            } else {
                tcl.replace("<OBJ>", objects)
            };
            rows.push(("Fix (Tcl)", kit::wrap_mono(tcl, t.text)));
        }
        if !rtl.is_empty() {
            rows.push(("Fix (RTL)", kit::wrap_value(rtl, t.text)));
        }
    }
    rows
}

fn select_hint(key: &'static str, content: &'static str, t: &VivadoTheme) -> El {
    text(content)
        .font(VivadoTheme::FONT_UI)
        .color(t.text_faint)
        .key(key)
}

/// Detail panel for the selected DRC violation.
pub fn drc_detail(s: &AppState, t: &VivadoTheme) -> El {
    match s.reports_view.selected_drc.and_then(|i| s.drc.items.get(i)) {
        Some(v) => kit::detail_panel(
            "reports-drc-detail",
            v.id.clone(),
            violation_rows(
                &v.id,
                (v.severity.label(), super::sev_color(v.severity, t)),
                None,
                &v.objects,
                &v.message,
                t,
            ),
            t,
        ),
        None => select_hint(
            "reports-drc-hint",
            "Select a violation to see its full message and fix.",
            t,
        ),
    }
}

/// Detail panel for the selected methodology check.
pub fn method_detail(s: &AppState, t: &VivadoTheme) -> El {
    match s
        .reports_view
        .selected_method
        .and_then(|i| s.timing.methodology.get(i))
    {
        Some(v) => kit::detail_panel(
            "reports-method-detail",
            v.id.clone(),
            violation_rows(
                &v.id,
                (v.severity.as_str(), super::method_sev_color(&v.severity, t)),
                Some(&v.category),
                &v.objects,
                &v.message,
                t,
            ),
            t,
        ),
        None => select_hint(
            "reports-method-hint",
            "Select a check to see its full message and fix.",
            t,
        ),
    }
}

/// Detail panel for the selected QoR suggestion.
pub fn qor_detail(s: &AppState, snap: &QorSnapshot, t: &VivadoTheme) -> El {
    let selected = s
        .reports_view
        .selected_qor
        .as_deref()
        .and_then(|id| snap.suggestions.iter().find(|g| g.id == id));
    match selected {
        Some(g) => {
            let mut rows: Vec<(&'static str, El)> = vec![
                (
                    "Severity",
                    kit::wrap_value(g.severity.clone(), super::qor_sev_color(&g.severity, t)),
                ),
                ("Category", kit::wrap_value(g.category.clone(), t.text)),
                (
                    "Target",
                    kit::wrap_mono(
                        if g.target.is_empty() {
                            "–"
                        } else {
                            &g.target
                        },
                        t.text,
                    ),
                ),
                (
                    "Expected gain",
                    kit::wrap_mono(format!("+{} ps", g.expected_gain_ps), t.text),
                ),
                (
                    "Description",
                    kit::wrap_value(g.description.clone(), t.text),
                ),
            ];
            if !g.applicable_for.is_empty() {
                rows.push((
                    "Applies to",
                    kit::wrap_value(g.applicable_for.clone(), t.text_dim),
                ));
            }
            if !g.tcl_command.is_empty() {
                rows.push(("Fix (Tcl)", kit::wrap_mono(g.tcl_command.clone(), t.text)));
            }
            kit::detail_panel("reports-qor-detail", g.id.clone(), rows, t)
        }
        None => select_hint(
            "reports-qor-hint",
            "Select a suggestion to see its description and fix.",
            t,
        ),
    }
}
