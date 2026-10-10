//! Reports dashboard (P1-2): Vivado Project-Summary style dashboard composing
//! the existing UtilizationReport / TimingSummary / DRC structs. Also owns the
//! timing report catalog regroup + endpoint-slack histogram (P1-4).

use eframe::egui;

use super::open_more_workspace;
use super::theme;
use super::HelionIde;
use super::IdeModel;
use super::WorkspaceTab;

/// Vivado Project-Summary style dashboard: design header, QoR cards
/// (WNS/TNS, utilization, DRC/methodology, power), a run-history strip,
/// and drill-down links. Every card header clicks through to its report tab.
pub fn paint_dashboard(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    egui::ScrollArea::vertical()
        .id_salt("dashboard_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            paint_dash_header(ui, &app.model, &theme);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                // Cards paint vertical content: reset the row's horizontal
                // layout per card, or every card's rows flow into one line.
                ui.vertical(|ui| paint_wns_card(ui, app));
                ui.vertical(|ui| paint_util_card(ui, app));
                ui.vertical(|ui| paint_quality_card(ui, app));
                ui.vertical(|ui| paint_power_card(ui, app));
            });
            ui.add_space(8.0);
            paint_run_strip(ui, app);
            ui.add_space(8.0);
            paint_drill_row(ui, app);
        });
}

/// "Timing Analysis" report catalog: regroups the timing-related report tabs
/// (summary, paths, clocks, CDC, power, methodology, DRC, utilization) into one
/// section tree for the Reports sidebar. Replaces the flat report catalog.
pub fn paint_timing_catalog(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    ui.label(
        egui::RichText::new("TIMING ANALYSIS")
            .small()
            .color(theme.faint),
    );
    ui.add_space(4.0);
    let rows = timing_rows(&app.model, &theme);
    for (label, badge, dot, tab) in rows {
        if catalog_row(ui, &theme, &label, badge.as_deref(), dot) {
            open_more_workspace(app, tab);
        }
    }
}

/// Endpoint-slack histogram over `model.timing_paths`: 12 bins across the
/// observed slack range. A bin paints `theme.danger` when it holds any
/// negative-slack endpoint, otherwise `theme.accent`; axis labels use
/// `theme.faint`. Honest empty state when there is no path data.
pub fn paint_slack_histogram(ui: &mut egui::Ui, model: &IdeModel, theme: &theme::Theme) {
    ui.label(
        egui::RichText::new("Endpoint Slack Histogram")
            .font(theme::font_ui_semibold(13.0))
            .color(theme.text),
    );
    if model.timing_paths.is_empty() {
        ui.label(egui::RichText::new("No timing paths — run Implement").color(theme.faint));
        return;
    }
    const BINS: usize = 12;
    let min = model
        .timing_paths
        .iter()
        .map(|p| p.slack_ps)
        .min()
        .unwrap_or(0);
    let max = model
        .timing_paths
        .iter()
        .map(|p| p.slack_ps)
        .max()
        .unwrap_or(0);
    let span = (max - min).max(1) as f64;
    let mut counts = [0usize; BINS];
    let mut bin_min = [i64::MAX; BINS];
    for p in &model.timing_paths {
        let b = (((p.slack_ps - min) as f64 / span) * BINS as f64).floor() as usize;
        let b = b.min(BINS - 1);
        counts[b] += 1;
        bin_min[b] = bin_min[b].min(p.slack_ps);
    }
    let peak = counts.iter().copied().max().unwrap_or(1).max(1);
    let plot_h = 96.0;
    let width = ui.available_width().max(120.0);
    let bar_w = width / BINS as f32;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, plot_h), egui::Sense::hover());
    for (i, count) in counts.iter().enumerate() {
        let h = *count as f32 / peak as f32 * plot_h;
        let x0 = rect.left() + i as f32 * bar_w;
        let bar = egui::Rect::from_min_max(
            egui::pos2(x0 + 1.0, rect.top() + plot_h - h),
            egui::pos2(x0 + bar_w - 1.0, rect.top() + plot_h),
        );
        let color = if bin_min[i] < 0 {
            theme.danger
        } else {
            theme.accent
        };
        ui.painter().rect_filled(bar, 2.0, color);
    }
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{min} ps"))
                .small()
                .color(theme.faint),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{max} ps"))
                    .small()
                    .color(theme.faint),
            );
        });
    });
    ui.label(
        egui::RichText::new(format!("{} paths", model.timing_paths.len()))
            .small()
            .color(theme.faint),
    );
}

/// Card shell: theme.surface fill, 1px theme.line border, 8px radius, 12px padding.
fn card_frame(theme: &theme::Theme) -> egui::Frame {
    egui::Frame::new()
        .fill(theme.surface)
        .stroke(egui::Stroke::new(1.0, theme.line))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(12))
}

/// Run/report status -> theme color (local: the app's helper is mid-migration).
fn status_color(theme: &theme::Theme, status: &str) -> egui::Color32 {
    match status {
        "Complete" => theme.ok,
        "Running" | "Warnings" => theme.warn,
        "Failed" => theme.danger,
        _ => theme.faint,
    }
}

/// Clickable card header. Returns true when the user drills into the card's tab.
fn card_header(ui: &mut egui::Ui, theme: &theme::Theme, title: &str) -> bool {
    let clicked = ui
        .add(
            egui::Label::new(
                egui::RichText::new(title)
                    .font(theme::font_ui_semibold(13.0))
                    .color(theme.text),
            )
            .sense(egui::Sense::click()),
        )
        .clicked();
    ui.add_space(4.0);
    clicked
}

/// Design/top name: explicit top override, else the loaded design, else "-".
fn design_name(model: &IdeModel) -> String {
    model
        .tree
        .top
        .clone()
        .or_else(|| model.shell.session.design.as_ref().map(|d| d.name.clone()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "-".into())
}

/// Header row: design name + part + latest-run status dot.
fn paint_dash_header(ui: &mut egui::Ui, model: &IdeModel, theme: &theme::Theme) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(design_name(model))
                .font(theme::font_ui_semibold(18.0))
                .color(theme.text),
        );
        ui.label(
            egui::RichText::new(format!("part={}", model.part()))
                .monospace()
                .color(theme.faint),
        );
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| match model.runs.last() {
                Some(run) => {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(
                        rect.center(),
                        5.0,
                        status_color(theme, &run.status),
                    );
                    ui.label(format!("{}: {}", run.name, run.status));
                }
                None => {
                    ui.label(egui::RichText::new("no runs yet").color(theme.faint));
                }
            },
        );
    });
}

/// QoR card: WNS/TNS in big mono, ok/danger by closure. Header drills to Reports.
fn paint_wns_card(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    let mut go = false;
    card_frame(&theme).show(ui, |ui| {
        ui.set_min_width(210.0);
        go = card_header(ui, &theme, "Timing QoR");
        match app.model.wns_ps() {
            Some(wns) => {
                let closed = app.model.timing_closed_wns();
                let tns = app.model.timing_summary().tns_ps;
                let color = if closed { theme.ok } else { theme.danger };
                ui.label(
                    egui::RichText::new(format!("WNS {wns} ps"))
                        .font(theme::font_code(22.0))
                        .color(color),
                );
                ui.label(
                    egui::RichText::new(format!("TNS {tns} ps"))
                        .font(theme::font_code(13.0))
                        .color(theme.dim),
                );
                if !closed {
                    ui.label(
                        egui::RichText::new("not a closed WNS")
                            .small()
                            .color(theme.warn),
                    );
                }
            }
            None => {
                ui.label(egui::RichText::new("No timing data").color(theme.faint));
                ui.label(
                    egui::RichText::new("Run Implement, then Report timing")
                        .small()
                        .color(theme.faint),
                );
            }
        }
    });
    if go {
        open_more_workspace(app, WorkspaceTab::Reports);
    }
}

/// Utilization card: horizontal occupancy bars with % labels. Header drills in.
fn paint_util_card(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    let mut go = false;
    card_frame(&theme).show(ui, |ui| {
        ui.set_min_width(230.0);
        go = card_header(ui, &theme, "Utilization");
        let report = app.model.utilization_report();
        if report.part.is_empty() {
            ui.label(egui::RichText::new("No placed design yet").color(theme.faint));
            return;
        }
        for row in &report.occupancy {
            let pct = row.pct();
            let frac = if row.available == 0 {
                0.0
            } else {
                row.used as f32 / row.available as f32
            };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(row.resource).small().color(theme.dim));
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(110.0, 10.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 3.0, theme.panel);
                let fill = rect.with_max_x(rect.left() + rect.width() * frac.clamp(0.0, 1.0));
                let bar = if pct >= 90 { theme.warn } else { theme.accent };
                ui.painter().rect_filled(fill, 3.0, bar);
                ui.label(
                    egui::RichText::new(format!("{pct}%"))
                        .small()
                        .color(theme.text),
                );
            });
        }
    });
    if go {
        open_more_workspace(app, WorkspaceTab::Utilization);
    }
}

/// One quality badge row: severity dot + clickable label. True when clicked.
fn quality_row(ui: &mut egui::Ui, theme: &theme::Theme, dot: egui::Color32, text: &str) -> bool {
    ui.horizontal(|ui| {
        ui.colored_label(dot, "●");
        ui.add(
            egui::Label::new(egui::RichText::new(text).color(theme.text))
                .sense(egui::Sense::click()),
        )
        .clicked()
    })
    .inner
}

/// Quality card: DRC + methodology badge counts. Rows drill to their tabs.
fn paint_quality_card(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    let mut go: Option<WorkspaceTab> = None;
    card_frame(&theme).show(ui, |ui| {
        ui.set_min_width(230.0);
        if card_header(ui, &theme, "Quality") {
            go = Some(WorkspaceTab::Drc);
        }
        match app.model.drc.clone() {
            Some(drc) => {
                let dot = if drc.violations.is_empty() {
                    theme.ok
                } else {
                    theme.danger
                };
                let text = format!("DRC: {} violations", drc.violations.len());
                if quality_row(ui, &theme, dot, &text) {
                    go = Some(WorkspaceTab::Drc);
                }
            }
            None => {
                ui.label(egui::RichText::new("DRC: no results yet").color(theme.faint));
            }
        }
        if app.model.shell.session.design.is_some() {
            let m = app.model.methodology_report();
            let dot = if m.error_count() > 0 {
                theme.danger
            } else if m.critical_count() > 0 || m.warning_count() > 0 {
                theme.warn
            } else {
                theme.ok
            };
            let text = format!(
                "Methodology: {} errors, {} critical, {} warnings",
                m.error_count(),
                m.critical_count(),
                m.warning_count()
            );
            if quality_row(ui, &theme, dot, &text) {
                go = Some(WorkspaceTab::Methodology);
            }
        } else {
            ui.label(egui::RichText::new("Methodology: no design yet").color(theme.faint));
        }
    });
    if let Some(tab) = go {
        open_more_workspace(app, tab);
    }
}

/// Power card: total/static/dynamic in mW. Honest empty state without numbers.
fn paint_power_card(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    let mut go = false;
    card_frame(&theme).show(ui, |ui| {
        ui.set_min_width(210.0);
        go = card_header(ui, &theme, "Power");
        let report = app.model.power_report();
        if report.part.is_empty() {
            ui.label(egui::RichText::new("No power data").color(theme.faint));
            ui.label(
                egui::RichText::new("Run Report power")
                    .small()
                    .color(theme.faint),
            );
            return;
        }
        let mw = |uw: i64| format!("{:.1} mW", uw as f64 / 1000.0);
        ui.label(
            egui::RichText::new(mw(report.total_uw))
                .font(theme::font_code(22.0))
                .color(theme.text),
        );
        ui.label(
            egui::RichText::new(format!(
                "static {} · dynamic {}",
                mw(report.static_uw),
                mw(report.dynamic_uw)
            ))
            .small()
            .color(theme.dim),
        );
    });
    if go {
        open_more_workspace(app, WorkspaceTab::Power);
    }
}

/// Run history strip: recent runs as status dots + names. Header drills to Runs.
fn paint_run_strip(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    let mut go = false;
    card_frame(&theme).show(ui, |ui| {
        go = card_header(ui, &theme, "Run History");
        if app.model.runs.is_empty() {
            ui.label(
                egui::RichText::new("No runs yet — create one from the Runs tab.")
                    .color(theme.faint),
            );
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for run in app.model.runs.iter().rev().take(8) {
                ui.colored_label(status_color(&theme, &run.status), "●");
                ui.label(&run.name);
                ui.label(egui::RichText::new(&run.status).small().color(theme.faint));
                ui.add_space(14.0);
            }
        });
    });
    if go {
        open_more_workspace(app, WorkspaceTab::Runs);
    }
}

/// Explicit drill-down link row; every card header already drills too.
fn paint_drill_row(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    let links = [
        ("Timing", WorkspaceTab::Reports),
        ("Utilization", WorkspaceTab::Utilization),
        ("DRC", WorkspaceTab::Drc),
        ("Methodology", WorkspaceTab::Methodology),
        ("Power", WorkspaceTab::Power),
        ("Runs", WorkspaceTab::Runs),
    ];
    let mut go: Option<WorkspaceTab> = None;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Drill down:").color(theme.faint));
        for (label, tab) in links {
            if ui
                .link(egui::RichText::new(label).color(theme.accent))
                .clicked()
            {
                go = Some(tab);
            }
        }
    });
    if let Some(tab) = go {
        open_more_workspace(app, tab);
    }
}

/// One catalog row: severity dot + label + right-aligned badge. True on click.
fn catalog_row(
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    label: &str,
    badge: Option<&str>,
    dot: egui::Color32,
) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.colored_label(dot, "●");
        if ui
            .add(
                egui::Label::new(egui::RichText::new(label).color(theme.text))
                    .sense(egui::Sense::click()),
            )
            .clicked()
        {
            clicked = true;
        }
        if let Some(b) = badge {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(b).small().color(theme.faint));
            });
        }
    });
    ui.add_space(2.0);
    clicked
}

/// (label, badge, dot, tab) tuple builder for the catalog rows.
fn cat_row(
    label: &str,
    badge: Option<String>,
    dot: egui::Color32,
    tab: WorkspaceTab,
) -> (String, Option<String>, egui::Color32, WorkspaceTab) {
    (label.into(), badge, dot, tab)
}

/// Full catalog row list. Badges come from cheap model fields only.
fn timing_rows(
    model: &IdeModel,
    theme: &theme::Theme,
) -> Vec<(String, Option<String>, egui::Color32, WorkspaceTab)> {
    let mut rows = timing_clock_rows(model, theme);
    rows.extend(timing_quality_rows(model, theme));
    rows
}

/// Timing + clock rows: summary, paths, clock interaction, CDC, clock networks.
fn timing_clock_rows(
    model: &IdeModel,
    theme: &theme::Theme,
) -> Vec<(String, Option<String>, egui::Color32, WorkspaceTab)> {
    let mut rows = Vec::with_capacity(5);
    if model.timing_closed_wns() {
        let r = model.timing_summary();
        let failing = r.failing_setup + r.failing_hold;
        let dot = if failing > 0 { theme.danger } else { theme.ok };
        rows.push(cat_row(
            "Timing Summary",
            Some(format!("{failing} failing")),
            dot,
            WorkspaceTab::Reports,
        ));
    } else {
        rows.push(cat_row(
            "Timing Summary",
            None,
            theme.faint,
            WorkspaceTab::Reports,
        ));
    }
    let n_paths = model.timing_paths.len();
    rows.push(cat_row(
        "Timing Paths",
        (n_paths > 0).then(|| format!("{n_paths} paths")),
        if n_paths > 0 {
            theme.accent
        } else {
            theme.faint
        },
        WorkspaceTab::Reports,
    ));
    let n_clk = model.clock_interaction().clocks.len();
    rows.push(cat_row(
        "Clock Interaction",
        (n_clk > 0).then(|| format!("{n_clk} clocks")),
        if n_clk > 0 { theme.accent } else { theme.faint },
        WorkspaceTab::ClockInteraction,
    ));
    let cdc = model.cdc_report();
    let (cdc_badge, cdc_dot) = if cdc.clocks.is_empty() {
        (None, theme.faint)
    } else {
        let (crit, warn) = (cdc.critical_count(), cdc.warning_count());
        let dot = if crit > 0 {
            theme.danger
        } else if warn > 0 {
            theme.warn
        } else {
            theme.ok
        };
        (Some(format!("{crit} crit · {warn} warn")), dot)
    };
    rows.push(cat_row("CDC", cdc_badge, cdc_dot, WorkspaceTab::Cdc));
    let n_net = model.clock_networks().clocks.len();
    rows.push(cat_row(
        "Clock Networks",
        (n_net > 0).then(|| format!("{n_net} clocks")),
        if n_net > 0 { theme.accent } else { theme.faint },
        WorkspaceTab::ClockNetworks,
    ));
    rows
}

/// Quality + power rows: power, methodology, DRC, utilization.
fn timing_quality_rows(
    model: &IdeModel,
    theme: &theme::Theme,
) -> Vec<(String, Option<String>, egui::Color32, WorkspaceTab)> {
    let mut rows = Vec::with_capacity(4);
    let pwr = model.power_report();
    if pwr.part.is_empty() {
        rows.push(cat_row("Power", None, theme.faint, WorkspaceTab::Power));
    } else {
        rows.push(cat_row(
            "Power",
            Some(format!("{:.1} mW", pwr.total_uw as f64 / 1000.0)),
            theme.accent,
            WorkspaceTab::Power,
        ));
    }
    if model.shell.session.design.is_none() {
        rows.push(cat_row(
            "Methodology",
            None,
            theme.faint,
            WorkspaceTab::Methodology,
        ));
    } else {
        let m = model.methodology_report();
        let (e, c, w) = (m.error_count(), m.critical_count(), m.warning_count());
        let (badge, dot) = if e > 0 {
            (format!("{e} errors"), theme.danger)
        } else if c > 0 {
            (format!("{c} critical"), theme.warn)
        } else if w > 0 {
            (format!("{w} warnings"), theme.warn)
        } else {
            ("clean".into(), theme.ok)
        };
        rows.push(cat_row(
            "Methodology",
            Some(badge),
            dot,
            WorkspaceTab::Methodology,
        ));
    }
    match model.drc.clone() {
        Some(drc) if !drc.violations.is_empty() => {
            let n = drc.violations.len();
            rows.push(cat_row(
                "DRC",
                Some(format!("{n} violations")),
                theme.danger,
                WorkspaceTab::Drc,
            ));
        }
        Some(_) => rows.push(cat_row(
            "DRC",
            Some("clean".into()),
            theme.ok,
            WorkspaceTab::Drc,
        )),
        None => rows.push(cat_row("DRC", None, theme.faint, WorkspaceTab::Drc)),
    }
    let util = model.utilization_report();
    if util.part.is_empty() {
        rows.push(cat_row(
            "Utilization",
            None,
            theme.faint,
            WorkspaceTab::Utilization,
        ));
    } else {
        let peak = util.occupancy.iter().map(|r| r.pct()).max().unwrap_or(0);
        rows.push(cat_row(
            "Utilization",
            Some(format!("{peak}% peak")),
            if peak >= 90 { theme.warn } else { theme.accent },
            WorkspaceTab::Utilization,
        ));
    }
    rows
}
