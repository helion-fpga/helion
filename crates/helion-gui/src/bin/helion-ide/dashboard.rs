//! Reports dashboard: Vivado Project-Summary style dashboard.
//!
//! Layout spec (`~/workspace/helion-research/ide-design-references.md` §3.1):
//! eight sections (Design, Synthesis, Implementation, Timing, Utilization,
//! DRC, Power, Run History) in a two-column grid that fills the window.
//! Every section gets a full-width 32px header bar (--surface bg, 1px --line
//! bottom border, 13px semibold title left, view tabs right where applicable)
//! inside a 12px-radius container. Label:value rows use a fixed ~190px label
//! column with full-word labels; status is always icon + word, never a bare
//! dot; tables follow the global table rules (§3.8). Also owns the timing
//! report catalog regroup + endpoint-slack histogram.

use eframe::egui;

use super::open_more_workspace;
use super::theme;
use super::HelionIde;
use super::IdeModel;
use super::WorkspaceTab;

/// Dashboard: 8 sections in a two-column grid that fills the panel.
/// Collapses to one column on narrow windows. Zero dead void by construction.
pub fn paint_dashboard(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    egui::ScrollArea::vertical()
        .id_salt("dashboard_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let avail = ui.available_width();
            let cols = if avail >= 760.0 { 2 } else { 1 };
            let gap = 16.0;
            let sw = (avail - gap * (cols as f32 - 1.0)) / cols as f32;
            // Vertical fill: when the viewport is taller than the content,
            // distribute the slack across rows so no dead void remains.
            let viewport_h = ui.available_height();
            let row_min: Vec<f32> = SECTIONS
                .chunks(cols)
                .map(|c| c.iter().map(|s| s.2).fold(0.0, f32::max))
                .collect();
            let total: f32 =
                row_min.iter().sum::<f32>() + gap * (row_min.len() as f32 - 1.0).max(0.0);
            let stretch = if viewport_h > total {
                (viewport_h - total) / row_min.len() as f32
            } else {
                0.0
            };
            // (title, view tabs, min height, section index)
            const SECTIONS: [(&str, Option<(&str, &[&str])>, f32, usize); 8] = [
                ("Design", None, 152.0, 0),
                ("Synthesis", None, 174.0, 1),
                ("Implementation", None, 196.0, 2),
                (
                    "Timing",
                    Some(("dash_timing_view", &["Summary", "Paths"])),
                    248.0,
                    3,
                ),
                ("Utilization", None, 232.0, 4),
                ("DRC", None, 162.0, 5),
                ("Power", None, 192.0, 6),
                ("Run History", None, 232.0, 7),
            ];
            let mut go: Option<WorkspaceTab> = None;
            for (ri, chunk) in SECTIONS.chunks(cols).enumerate() {
                if ri > 0 {
                    ui.add_space(gap);
                }
                // Row height = tallest section in the row, plus any vertical
                // fill slack: the grid stays flush and fills the viewport.
                let row_h = row_min[ri] + stretch;
                ui.horizontal_top(|ui| {
                    for (j, (title, tabs, _, idx)) in chunk.iter().enumerate() {
                        if j > 0 {
                            ui.add_space(gap);
                        }
                        if let Some(tab) =
                            paint_section(ui, app, &theme, sw, row_h, title, *tabs, *idx)
                        {
                            go = Some(tab);
                        }
                    }
                });
            }
            if let Some(tab) = go {
                open_more_workspace(app, tab);
            }
        });
}

/// Section container: 12px radius, --surface fill, 1px --line border.
fn section_frame(theme: &theme::Theme) -> egui::Frame {
    egui::Frame::new()
        .fill(theme.surface)
        .stroke(egui::Stroke::new(1.0, theme.line))
        .corner_radius(12.0)
        .inner_margin(egui::Margin::ZERO)
}

/// One dashboard section: header bar + padded body. Returns the tab to drill
/// into when a body link is clicked.
fn paint_section(
    ui: &mut egui::Ui,
    app: &mut HelionIde,
    theme: &theme::Theme,
    w: f32,
    min_h: f32,
    title: &str,
    tabs: Option<(&str, &[&str])>,
    idx: usize,
) -> Option<WorkspaceTab> {
    let mut go: Option<WorkspaceTab> = None;
    let mut view = 0;
    ui.allocate_ui_with_layout(
        egui::vec2(w, min_h),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            section_frame(theme).show(ui, |ui| {
                ui.set_min_size(egui::vec2(w, min_h));
                if let Some(v) = section_header(ui, theme, title, tabs) {
                    view = v;
                }
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.set_min_width(w - 28.0);
                        if let Some(tab) = paint_section_body(idx, view, ui, app, theme) {
                            go = Some(tab);
                        }
                    });
            });
        },
    );
    go
}

/// Section header bar: 32px tall, --surface bg, 1px --line bottom border,
/// 13px semibold title left, view tabs right. Top corners rounded to sit
/// inside the 12px container; bottom edge is a straight divider line.
fn section_header(
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    title: &str,
    tabs: Option<(&str, &[&str])>,
) -> Option<usize> {
    let hw = ui.available_width();
    let (hrect, _) = ui.allocate_exact_size(egui::vec2(hw, 32.0), egui::Sense::hover());
    // Rounded rect + square overlay on the lower half = top-rounded header.
    // The seam is invisible (same fill); the bottom divider is a straight line.
    ui.painter().rect_filled(hrect, 12.0, theme.surface);
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(hrect.left(), hrect.center().y),
            egui::pos2(hrect.right(), hrect.bottom()),
        ),
        0.0,
        theme.surface,
    );
    ui.painter().line_segment(
        [hrect.left_bottom(), hrect.right_bottom()],
        egui::Stroke::new(1.0, theme.line),
    );
    let mut sel_out = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(hrect), |ui| {
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new(title)
                    .font(theme::font_ui_semibold(13.0))
                    .color(theme.text),
            );
            if let Some((id, names)) = tabs {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(14.0);
                    sel_out = Some(section_tabs(ui, theme, id, names));
                });
            }
        });
    });
    sel_out
}

/// View tabs for a section header ("Summary | Paths"). State persists in egui
/// temp data; active tab is accent, inactive dim.
fn section_tabs(ui: &mut egui::Ui, theme: &theme::Theme, id: &str, tabs: &[&str]) -> usize {
    let eid = egui::Id::new(("dash_view_tabs", id));
    let mut sel: usize = ui.ctx().data_mut(|d| d.get_temp(eid).unwrap_or(0));
    if sel >= tabs.len() {
        sel = 0;
    }
    // right_to_left: add last tab first so order reads left-to-right.
    for (i, t) in tabs.iter().enumerate().rev() {
        let active = i == sel;
        let r = ui.add(
            egui::Label::new(
                egui::RichText::new(*t)
                    .size(12.0)
                    .color(if active { theme.accent } else { theme.dim }),
            )
            .sense(egui::Sense::click()),
        );
        if r.clicked() {
            sel = i;
        }
        ui.add_space(10.0);
    }
    ui.ctx().data_mut(|d| d.insert_temp(eid, sel));
    sel
}

/// Dispatch to the per-section body painter.
fn paint_section_body(
    idx: usize,
    view: usize,
    ui: &mut egui::Ui,
    app: &mut HelionIde,
    theme: &theme::Theme,
) -> Option<WorkspaceTab> {
    match idx {
        0 => {
            paint_design_body(ui, app, theme);
            None
        }
        1 => {
            paint_synth_body(ui, app, theme);
            None
        }
        2 => {
            paint_impl_body(ui, app, theme);
            None
        }
        3 => paint_timing_body(view, ui, app, theme),
        4 => {
            paint_util_body(ui, app, theme);
            None
        }
        5 => paint_drc_body(ui, app, theme),
        6 => {
            paint_power_body(ui, app, theme);
            None
        }
        _ => paint_runs_body(ui, app, theme),
    }
}

/// Label:value row: fixed ~190px label column (12px dim), 13px text value.
/// Painted with `painter.text` — `add_sized`+`Label` centers text (egui
/// uses a centered-and-justified layout there), which breaks the column.
fn kv_row(ui: &mut egui::Ui, theme: &theme::Theme, label: &str, value: &str) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 20.0), egui::Sense::hover());
    let cy = rect.center().y;
    let p = ui.painter();
    p.text(
        egui::pos2(rect.left(), cy),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::new(12.0, egui::FontFamily::Proportional),
        theme.dim,
    );
    p.text(
        egui::pos2(rect.left() + 190.0, cy),
        egui::Align2::LEFT_CENTER,
        value,
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
        theme.text,
    );
    ui.add_space(2.0);
}

/// Status row: icon + word, never a bare dot.
/// ✓ Complete · ▶ Running · ✗ Failed · ⚠ Warnings · ✓ Clean · ○ Not started
fn status_row(ui: &mut egui::Ui, theme: &theme::Theme, label: &str, status: &str) {
    let (icon, color) = match status {
        "Complete" | "Clean" => ("✓", theme.ok),
        "Running" => ("▶", theme.accent),
        "Failed" => ("✗", theme.danger),
        "Warnings" => ("⚠", theme.warn),
        _ => ("○", theme.faint),
    };
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 20.0), egui::Sense::hover());
    let cy = rect.center().y;
    let p = ui.painter();
    p.text(
        egui::pos2(rect.left(), cy),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::new(12.0, egui::FontFamily::Proportional),
        theme.dim,
    );
    p.text(
        egui::pos2(rect.left() + 190.0, cy),
        egui::Align2::LEFT_CENTER,
        icon,
        egui::FontId::new(14.0, egui::FontFamily::Proportional),
        color,
    );
    p.text(
        egui::pos2(rect.left() + 214.0, cy),
        egui::Align2::LEFT_CENTER,
        status,
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
        theme.text,
    );
    ui.add_space(2.0);
}

/// 28px bold tabular headline figure with a 12px dim full-word label above.
/// Never a bare code: "Worst Negative Slack (WNS)" / "9640 ps".
fn metric_headline(
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    label: &str,
    value: &str,
    color: egui::Color32,
) {
    ui.label(egui::RichText::new(label).size(12.0).color(theme.dim));
    ui.label(
        egui::RichText::new(value)
            .size(28.0)
            .strong()
            .monospace()
            .color(color),
    );
}

/// Designed empty state: centered 13px dim text + accent action link.
/// Returns true when the action is clicked. No section may render as an
/// empty box or disappear silently.
fn empty_state(ui: &mut egui::Ui, theme: &theme::Theme, text: &str, action: &str) -> bool {
    ui.vertical_centered(|ui| {
        ui.add_space(10.0);
        ui.label(egui::RichText::new(text).size(13.0).color(theme.dim));
        ui.add_space(4.0);
        ui.link(egui::RichText::new(action).size(13.0).color(theme.accent))
            .clicked()
    })
    .inner
}

/// Accent action link on its own line. Returns true when clicked.
fn action_link(ui: &mut egui::Ui, theme: &theme::Theme, text: &str) -> bool {
    ui.add_space(4.0);
    ui.link(egui::RichText::new(text).size(13.0).color(theme.accent))
        .clicked()
}

/// One table cell.
struct Cell {
    text: String,
    color: egui::Color32,
    right: bool,
    mono: bool,
}

impl Cell {
    fn text(t: impl Into<String>, color: egui::Color32) -> Self {
        Self {
            text: t.into(),
            color,
            right: false,
            mono: false,
        }
    }
    fn num(t: impl Into<String>, color: egui::Color32) -> Self {
        Self {
            text: t.into(),
            color,
            right: true,
            mono: true,
        }
    }
}

/// Spec table (§3.8): 11px dim semibold header, 28px tall, 1px --line bottom
/// border; 28px rows, 1px --line dividers, hover highlight; numeric columns
/// right-aligned in tabular figures. `cols`: (header, width px or None=fill,
/// right_align). `bars`: optional per-row 0..1 fractions painted as 2px
/// accent bars under `bar_col`.
fn spec_table(
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    cols: &[(&str, Option<f32>, bool)],
    rows: &[Vec<Cell>],
    bar_col: Option<usize>,
    bars: &[f32],
) {
    let w = ui.available_width();
    let fixed: f32 = cols.iter().filter_map(|c| c.1).sum();
    let n_fill = cols.iter().filter(|c| c.1.is_none()).count().max(1) as f32;
    let fill_w = ((w - fixed) / n_fill).max(48.0);
    let widths: Vec<f32> = cols.iter().map(|c| c.1.unwrap_or(fill_w)).collect();

    let (hrect, _) = ui.allocate_exact_size(egui::vec2(w, 28.0), egui::Sense::hover());
    paint_table_cells(
        ui,
        hrect,
        &widths,
        &cols
            .iter()
            .map(|c| Cell {
                text: c.0.to_string(),
                color: theme.dim,
                right: c.2,
                mono: false,
            })
            .collect::<Vec<_>>(),
        true,
    );
    ui.painter().line_segment(
        [hrect.left_bottom(), hrect.right_bottom()],
        egui::Stroke::new(1.0, theme.line),
    );

    for (ri, row) in rows.iter().enumerate() {
        let (rrect, _) = ui.allocate_exact_size(egui::vec2(w, 28.0), egui::Sense::hover());
        if ui.rect_contains_pointer(rrect) {
            ui.painter().rect_filled(rrect, 0.0, theme.surface_hover);
        }
        paint_table_cells(ui, rrect, &widths, row, false);
        ui.painter().line_segment(
            [rrect.left_bottom(), rrect.right_bottom()],
            egui::Stroke::new(1.0, theme.line),
        );
        if let Some(bc) = bar_col {
            if bc < widths.len() && ri < bars.len() {
                let bx: f32 = widths[..bc].iter().sum();
                let bw = widths[bc] * bars[ri].clamp(0.0, 1.0);
                ui.painter().rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(rrect.left() + bx + 1.0, rrect.bottom() - 3.0),
                        egui::pos2(rrect.left() + bx + 1.0 + bw.max(2.0), rrect.bottom() - 1.0),
                    ),
                    1.0,
                    theme.accent,
                );
            }
        }
    }
}

/// Table cells painted with `painter.text` at explicit anchors: numeric
/// columns right-aligned, text left-aligned. (`ui.put`+`Label` re-sizes the
/// label to its text, defeating column alignment.)
fn paint_table_cells(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    widths: &[f32],
    cells: &[Cell],
    header: bool,
) {
    let mut x = rect.left();
    for (i, cell) in cells.iter().enumerate() {
        let cw = widths.get(i).copied().unwrap_or(60.0);
        let font = egui::FontId::new(
            if header { 11.0 } else { 13.0 },
            if cell.mono && !header {
                egui::FontFamily::Monospace
            } else {
                egui::FontFamily::Proportional
            },
        );
        let (anchor, px) = if cell.right {
            (egui::Align2::RIGHT_CENTER, x + cw - 4.0)
        } else {
            (egui::Align2::LEFT_CENTER, x + 4.0)
        };
        ui.painter()
            .text(egui::pos2(px, rect.center().y), anchor, &cell.text, font, cell.color);
        x += cw;
    }
}

/// Design section: name / part / top / source file.
fn paint_design_body(ui: &mut egui::Ui, app: &mut HelionIde, theme: &theme::Theme) {
    let m = &app.model;
    kv_row(ui, theme, "Design name", &design_name(m));
    kv_row(ui, theme, "Part", m.part());
    kv_row(
        ui,
        theme,
        "Top module",
        m.tree.top.as_deref().unwrap_or("—"),
    );
    let src = app
        .recent
        .first()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "—".into());
    kv_row(ui, theme, "Source file", &truncate_str(&src, 42));
}

/// Synthesis section: status / strategy / part / cells / runtime.
fn paint_synth_body(ui: &mut egui::Ui, app: &mut HelionIde, theme: &theme::Theme) {
    match app.model.runs.iter().find(|r| r.step == "Synthesis") {
        Some(run) => {
            status_row(ui, theme, "Status", &run.status);
            kv_row(ui, theme, "Strategy", &run.strategy);
            kv_row(ui, theme, "Part", &run.part);
            kv_row(
                ui,
                theme,
                "Cells",
                &run.cells.map(|c| c.to_string()).unwrap_or_else(|| "—".into()),
            );
            kv_row(ui, theme, "Runtime", &fmt_duration(run.runtime_ms));
        }
        None => {
            status_row(ui, theme, "Status", "Not started");
            kv_row(ui, theme, "Strategy", "—");
            kv_row(ui, theme, "Part", app.model.part());
            kv_row(ui, theme, "Cells", "—");
            kv_row(ui, theme, "Runtime", "—");
        }
    }
}

/// Implementation section: status / strategy / WNS / LUT+FF / runtime / bitstream.
fn paint_impl_body(ui: &mut egui::Ui, app: &mut HelionIde, theme: &theme::Theme) {
    match app.model.runs.iter().find(|r| r.step == "Implementation") {
        Some(run) => {
            status_row(ui, theme, "Status", &run.status);
            kv_row(ui, theme, "Strategy", &run.strategy);
            kv_row(
                ui,
                theme,
                "Worst Negative Slack (WNS)",
                &run.wns_ps
                    .map(|w| format!("{w} ps"))
                    .unwrap_or_else(|| "—".into()),
            );
            kv_row(
                ui,
                theme,
                "LUT+FF",
                &run
                    .lutff
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "—".into()),
            );
            kv_row(ui, theme, "Runtime", &fmt_duration(run.runtime_ms));
            kv_row(
                ui,
                theme,
                "Bitstream",
                &run
                    .bitstream_hash
                    .map(|h| format!("{h:08x}"))
                    .unwrap_or_else(|| "—".into()),
            );
        }
        None => {
            status_row(ui, theme, "Status", "Not started");
            for (l, v) in [
                ("Strategy", "—"),
                ("Worst Negative Slack (WNS)", "—"),
                ("LUT+FF", "—"),
                ("Runtime", "—"),
                ("Bitstream", "—"),
            ] {
                kv_row(ui, theme, l, v);
            }
        }
    }
}

/// Timing section. Summary view: WNS/TNS headlines, endpoint counts, report
/// link. Paths view: worst-paths table.
fn paint_timing_body(
    view: usize,
    ui: &mut egui::Ui,
    app: &mut HelionIde,
    theme: &theme::Theme,
) -> Option<WorkspaceTab> {
    let mut go = None;
    match app.model.wns_ps() {
        Some(wns) => {
            if view == 1 {
                let mut worst: Vec<_> = app.model.timing_paths.iter().collect();
                worst.sort_by_key(|p| p.slack_ps);
                let rows: Vec<Vec<Cell>> = worst
                    .iter()
                    .take(5)
                    .enumerate()
                    .map(|(i, p)| {
                        vec![
                            Cell::num((i + 1).to_string(), theme.faint),
                            Cell::text(truncate_str(&p.endpoint, 34), theme.text),
                            Cell::num(
                                format!("{} ps", p.slack_ps),
                                if p.slack_ps < 0 {
                                    theme.danger
                                } else {
                                    theme.text
                                },
                            ),
                        ]
                    })
                    .collect();
                spec_table(
                    ui,
                    theme,
                    &[
                        ("#", Some(28.0), true),
                        ("Endpoint", None, false),
                        ("Slack", Some(96.0), true),
                    ],
                    &rows,
                    None,
                    &[],
                );
            } else {
                // Slack is data, not status: negative -> danger, else plain
                // text. Green is reserved for pass/fail status only.
                let wns_color = if wns < 0 { theme.danger } else { theme.text };
                metric_headline(ui, theme, "Worst Negative Slack (WNS)", &format!("{wns} ps"), wns_color);
                ui.add_space(4.0);
                let tns = app.model.timing_summary().tns_ps;
                kv_row(ui, theme, "Total Negative Slack (TNS)", &format!("{tns} ps"));
                let r = app.model.timing_summary();
                kv_row(
                    ui,
                    theme,
                    "Failing endpoints",
                    &(r.failing_setup + r.failing_hold).to_string(),
                );
                kv_row(
                    ui,
                    theme,
                    "Total endpoints",
                    &app.model.timing_paths.len().to_string(),
                );
                if action_link(ui, theme, "Implemented Timing Report") {
                    go = Some(WorkspaceTab::Reports);
                }
            }
        }
        None => {
            if empty_state(ui, theme, "No timing data yet.", "Run Implementation") {
                super::run_implement(app);
            }
        }
    }
    go
}

/// Utilization section: Resource | Used | Available | Util % table with a
/// thin accent bar under each %.
fn paint_util_body(ui: &mut egui::Ui, app: &mut HelionIde, theme: &theme::Theme) {
    let report = app.model.utilization_report();
    if report.part.is_empty() {
        if empty_state(ui, theme, "No utilization data yet.", "Run Implementation") {
            super::run_implement(app);
        }
        return;
    }
    let rows: Vec<Vec<Cell>> = report
        .occupancy
        .iter()
        .map(|r| {
            vec![
                Cell::text(r.resource.to_string(), theme.text),
                Cell::num(r.used.to_string(), theme.text),
                Cell::num(r.available.to_string(), theme.text),
                Cell::num(format!("{}%", r.pct()), theme.text),
            ]
        })
        .collect();
    let bars: Vec<f32> = report
        .occupancy
        .iter()
        .map(|r| {
            if r.available == 0 {
                0.0
            } else {
                r.used as f32 / r.available as f32
            }
        })
        .collect();
    spec_table(
        ui,
        theme,
        &[
            ("Resource", None, false),
            ("Used", Some(84.0), true),
            ("Available", Some(84.0), true),
            ("Util %", Some(84.0), true),
        ],
        &rows,
        Some(3),
        &bars,
    );
}

/// DRC section: status icon+word, violation count, report link.
fn paint_drc_body(
    ui: &mut egui::Ui,
    app: &mut HelionIde,
    theme: &theme::Theme,
) -> Option<WorkspaceTab> {
    let mut go = None;
    match app.model.drc.clone() {
        Some(drc) => {
            if drc.violations.is_empty() {
                status_row(ui, theme, "Status", "Clean");
            } else {
                status_row(
                    ui,
                    theme,
                    "Status",
                    &format!("{} violations", drc.violations.len()),
                );
            }
            kv_row(
                ui,
                theme,
                "Violations",
                &drc.violations.len().to_string(),
            );
            if action_link(ui, theme, "Open DRC Report") {
                go = Some(WorkspaceTab::Drc);
            }
        }
        None => {
            if empty_state(ui, theme, "No DRC results yet.", "Run Implementation") {
                super::run_implement(app);
            }
        }
    }
    go
}

/// Power section: total headline, static/dynamic stacked bar, legend.
fn paint_power_body(ui: &mut egui::Ui, app: &mut HelionIde, theme: &theme::Theme) {
    let report = app.model.power_report();
    if report.part.is_empty() {
        if empty_state(ui, theme, "No power data yet.", "Run Implementation") {
            super::run_implement(app);
        }
        return;
    }
    let mw = |uw: i64| format!("{:.1} mW", uw as f64 / 1000.0);
    metric_headline(ui, theme, "Total Power", &mw(report.total_uw), theme.text);
    ui.add_space(8.0);
    // Stacked bar: static (dim) + dynamic (accent). Track with rounded outline.
    let w = ui.available_width();
    let total = (report.static_uw + report.dynamic_uw).max(1) as f32;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 14.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 7.0, theme.panel);
    let inner = rect.shrink(2.0);
    let sw = inner.width() * report.static_uw as f32 / total;
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            inner.min,
            egui::pos2(inner.left() + sw, inner.bottom()),
        ),
        0.0,
        theme.dim,
    );
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(inner.left() + sw, inner.top()),
            inner.max,
        ),
        0.0,
        theme.accent,
    );
    ui.painter().rect_stroke(
        rect,
        7.0,
        egui::Stroke::new(1.0, theme.line),
        egui::StrokeKind::Inside,
    );
    ui.add_space(8.0);
    for (label, uw, swatch) in [
        ("Static", report.static_uw, theme.dim),
        ("Dynamic", report.dynamic_uw, theme.accent),
    ] {
        let pct = uw as f64 / total as f64 * 100.0;
        ui.horizontal(|ui| {
            let (srect, _) =
                ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().rect_filled(srect, 2.0, swatch);
            ui.label(egui::RichText::new(label).size(13.0).color(theme.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("{} ({pct:.0}%)", mw(uw)))
                        .size(13.0)
                        .monospace()
                        .color(theme.dim),
                );
            });
        });
        ui.add_space(2.0);
    }
}

/// Run History section: # | Run | Stage | Status | WNS | LUT+FF | Duration,
/// newest first, capped at 5 rows with a "view all" link.
fn paint_runs_body(
    ui: &mut egui::Ui,
    app: &mut HelionIde,
    theme: &theme::Theme,
) -> Option<WorkspaceTab> {
    let mut go = None;
    if app.model.runs.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new("No runs yet — create one from the Design Runs tab.")
                    .size(13.0)
                    .color(theme.dim),
            );
        });
        return None;
    }
    let rows: Vec<Vec<Cell>> = app
        .model
        .runs
        .iter()
        .rev()
        .take(5)
        .enumerate()
        .map(|(i, run)| {
            let (icon, scolor) = match run.status.as_str() {
                "Complete" => ("✓", theme.ok),
                "Running" => ("▶", theme.accent),
                "Failed" => ("✗", theme.danger),
                "Warnings" => ("⚠", theme.warn),
                _ => ("○", theme.faint),
            };
            vec![
                Cell::num((i + 1).to_string(), theme.faint),
                Cell::text(run.name.clone(), theme.text),
                Cell::text(run.step.clone(), theme.dim),
                Cell {
                    text: format!("{icon} {}", run.status),
                    color: scolor,
                    right: false,
                    mono: false,
                },
                Cell::num(
                    run.wns_ps
                        .map(|w| format!("{w} ps"))
                        .unwrap_or_else(|| "—".into()),
                    theme.text,
                ),
                Cell::num(
                    run.lutff
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "—".into()),
                    theme.text,
                ),
                Cell::num(fmt_duration(run.runtime_ms), theme.text),
            ]
        })
        .collect();
    spec_table(
        ui,
        theme,
        &[
            ("#", Some(28.0), true),
            ("Run", None, false),
            ("Stage", Some(110.0), false),
            ("Status", Some(110.0), false),
            ("WNS", Some(90.0), true),
            ("LUT+FF", Some(70.0), true),
            ("Duration", Some(80.0), true),
        ],
        &rows,
        None,
        &[],
    );
    if app.model.runs.len() > 5 {
        if action_link(
            ui,
            theme,
            &format!("View all {} runs", app.model.runs.len()),
        ) {
            go = Some(WorkspaceTab::Runs);
        }
    }
    go
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

/// `None` runtime -> "—"; <1s -> "N ms"; else "N.N s".
fn fmt_duration(ms: Option<u64>) -> String {
    match ms {
        Some(m) if m >= 1000 => format!("{:.1} s", m as f64 / 1000.0),
        Some(m) => format!("{m} ms"),
        None => "—".into(),
    }
}

/// Char-boundary-safe truncation with an ellipsis.
fn truncate_str(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if t.len() < s.len() {
        format!("{t}…")
    } else {
        t
    }
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
    const BINS: usize = 20;
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
            // Muted accent per §3.3: the histogram is context, not brand.
            theme.accent_dim
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
        let crit = cdc.critical_count() + cdc.structural_critical_count();
        let warn = cdc.warning_count() + cdc.structural_warning_count();
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
