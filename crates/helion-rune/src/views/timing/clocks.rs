//! Clock-centric timing reports: the UG949 Clock Interaction matrix, CDC
//! table with severity chips, Clock Networks with totals, the path-group
//! tables (setup and hold), and the endpoint slack histogram.
//!
//! Mirrors the old IDE's `paint_clock_interaction` / `paint_cdc` /
//! `paint_clock_networks` (`helion-ide.rs:7069-7436`) and the dashboard
//! `paint_slack_histogram` (`dashboard.rs:1012-1122`) as Rune `El` trees
//! over [`crate::state::TimingState`] rows filled by real runs.

use rune::{El, col, el, row, text};

use crate::state::{AppState, ClockRelClass, TimingGroupRow};
use crate::theme::VivadoTheme;
use crate::views::report_kit::{
    self as kit, count_chip, header_row, ns_opt, ns3, num_cell, plain_row, slack_color, table,
};

/// Matrix row-label column width.
const AXIS_W: f32 = 120.0;
/// Matrix cell height (relation + WNS lines).
const CELL_H: f32 = 40.0;
/// Numeric column width (nanoseconds).
const NS_W: f32 = 72.0;
/// Count column width.
const COUNT_W: f32 = 64.0;
/// Short text column width (severity, check id, sync).
const SHORT_W: f32 = 72.0;
/// Endpoint histogram bins and plot height.
const BINS: usize = 16;
const PLOT_H: f32 = 64.0;

/// Ink for one relation class (legend and cell stripe).
pub fn class_color(class: ClockRelClass, t: &VivadoTheme) -> rune::Color {
    match class {
        ClockRelClass::Safe => t.pass,
        ClockRelClass::Partial => t.warn,
        ClockRelClass::Unsafe => t.fail,
        ClockRelClass::Ignored => t.accent,
        ClockRelClass::NoPaths => t.text_faint,
    }
}

/// Clock names in matrix order (first appearance as a `from`).
fn matrix_clocks(s: &AppState) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in &s.timing.clock_interaction {
        if !out.contains(&c.from) {
            out.push(c.from.clone());
        }
    }
    out
}

fn clock_source(s: &AppState, name: &str) -> String {
    s.timing
        .clocks
        .iter()
        .find(|c| c.name == name)
        .map(|c| c.source.clone())
        .unwrap_or_else(|| name.to_string())
}

/// One matrix cell: class stripe, relation, and WNS; the hover tooltip
/// carries common period, requirement, path count and source.
fn matrix_cell(s: &AppState, idx: usize, key: String, t: &VivadoTheme) -> El {
    let Some(c) = s.timing.clock_interaction.get(idx) else {
        return el().grow(1.0).min_w(0.0);
    };
    let selected = s.timing_view.selected_interaction == Some(idx);
    let class = c.relation.class();
    let tip = format!(
        "{} → {} ({}) · Common period {} ns · Requirement {} ns · Paths {} · Source {}",
        c.from,
        c.to,
        c.relation.label(),
        ns3(c.common_period_ps as i64),
        ns3(c.requirement_ps),
        c.path_count,
        clock_source(s, &c.from),
    );
    let wns = match c.wns_ps {
        Some(w) => format!("WNS {} ns", ns3(w)),
        None => "WNS –".into(),
    };
    let mut cell = row()
        .key(key)
        .role("button")
        .label(format!("{} to {}: {}", c.from, c.to, c.relation.label()))
        .help(tip)
        .focusable(true)
        .grow(1.0)
        .min_w(0.0)
        .h(CELL_H)
        .gap(6.0)
        .items_center()
        .border(1.0, t.border_light)
        .hover(t.selection)
        .on_press(move |s: &mut AppState| s.timing_view.selected_interaction = Some(idx))
        .child(el().w(3.0).h_full().fill(class_color(class, t)))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(0.0)
                .child(
                    text(c.relation.label())
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text)
                        .truncate(),
                )
                .child(
                    text(wns)
                        .font(VivadoTheme::FONT_MONO.mono())
                        .color(slack_color(c.wns_ps, t))
                        .truncate(),
                ),
        );
    cell = if selected {
        cell.fill(t.selection)
    } else {
        cell.fill(t.panel_bg)
    };
    cell
}

/// Legend row: one swatch per relation class.
fn legend(t: &VivadoTheme) -> El {
    let mut r = row()
        .key("timing-ci-legend")
        .w_full()
        .flex_wrap()
        .gaps(4.0, 12.0)
        .items_center();
    for class in ClockRelClass::ALL {
        r = r.child(
            row()
                .h(kit::ROW_H)
                .items_center()
                .gap(4.0)
                .child(
                    el().size(10.0, 10.0)
                        .radius(2.0)
                        .fill(class_color(class, t)),
                )
                .child(
                    text(class.label())
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text_dim),
                ),
        );
    }
    r
}

/// Clock Interaction page: N x N `From \ To` matrix, legend, and the
/// selected cell's detail.
pub fn interaction_page(s: &AppState, t: &VivadoTheme) -> El {
    let clocks = matrix_clocks(s);
    let mut page = col().key("timing-ci").w_full().gap(8.0).child(
        row()
            .w_full()
            .items_center()
            .gap(8.0)
            .child(kit::section_title("Clock Interaction", t))
            .child(
                text(format!("{n} × {n} clock domains", n = clocks.len()))
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_dim),
            ),
    );
    if clocks.is_empty() {
        return page.child(kit::empty_state(
            "timing-ci-empty",
            "No clocks were analyzed in the last run.",
            None,
            t,
        ));
    }
    let mut head = row()
        .w_full()
        .h(kit::HEADER_H)
        .items_center()
        .fill(t.header_bg)
        .child(
            el().w(AXIS_W)
                .px(kit::PAD_X)
                .child(kit::header_cell("From \\ To", t)),
        );
    for name in &clocks {
        head = head.child(
            el().grow(1.0).min_w(0.0).px(kit::PAD_X).child(
                text(name.clone())
                    .font(VivadoTheme::FONT_TITLE)
                    .color(t.text)
                    .truncate()
                    .help_when_truncated(name.clone()),
            ),
        );
    }
    let mut grid = col()
        .key("timing-ci-matrix")
        .w_full()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(head);
    for (r, from) in clocks.iter().enumerate() {
        let mut line = row().w_full().items_center().child(
            el().w(AXIS_W)
                .h(CELL_H)
                .px(kit::PAD_X)
                .center_y()
                .fill(t.header_bg)
                .child(
                    text(from.clone())
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text)
                        .truncate()
                        .help_when_truncated(from.clone()),
                ),
        );
        for (c, to) in clocks.iter().enumerate() {
            let idx = s
                .timing
                .clock_interaction
                .iter()
                .position(|x| &x.from == from && &x.to == to);
            line = line.child(match idx {
                Some(i) => matrix_cell(s, i, format!("timing-ci-cell-{r}-{c}"), t),
                None => el()
                    .grow(1.0)
                    .min_w(0.0)
                    .h(CELL_H)
                    .center()
                    .border(1.0, t.border_light)
                    .child(text("–").font(VivadoTheme::FONT_UI).color(t.text_faint)),
            });
        }
        grid = grid.child(line);
    }
    page = page.child(grid).child(legend(t));
    match s
        .timing_view
        .selected_interaction
        .and_then(|i| s.timing.clock_interaction.get(i))
    {
        Some(c) => page.child(kit::detail_panel(
            "timing-ci-detail",
            format!("{} → {}", c.from, c.to),
            vec![
                (
                    "Relation",
                    kit::wrap_value(c.relation.label(), class_color(c.relation.class(), t)),
                ),
                (
                    "Common period",
                    kit::wrap_mono(format!("{} ns", ns3(c.common_period_ps as i64)), t.text),
                ),
                (
                    "Requirement",
                    kit::wrap_mono(format!("{} ns", ns3(c.requirement_ps)), t.text),
                ),
                (
                    "WNS",
                    kit::wrap_mono(
                        c.wns_ps
                            .map(|w| format!("{} ns", ns3(w)))
                            .unwrap_or_else(|| "not timed".into()),
                        slack_color(c.wns_ps, t),
                    ),
                ),
                ("Paths", kit::wrap_mono(c.path_count.to_string(), t.text)),
                ("Source", kit::wrap_mono(clock_source(s, &c.from), t.text)),
            ],
            t,
        )),
        None => page.child(
            text("Select a cell to see its requirement and path count.")
                .font(VivadoTheme::FONT_UI)
                .color(t.text_faint)
                .key("timing-ci-hint"),
        ),
    }
}

fn severity_color(sev: &str, t: &VivadoTheme) -> rune::Color {
    match sev {
        "Critical" => t.fail,
        "Warning" => t.warn,
        "Info" => t.accent,
        "Safe" => t.pass,
        _ => t.text,
    }
}

/// CDC section: severity chips plus `From | To | Severity | Check | Sync |
/// Endpoints | Worst Slack | Relation`.
pub fn cdc_section(s: &AppState, t: &VivadoTheme) -> El {
    let count = |sev: &str| s.timing.cdc.iter().filter(|v| v.severity == sev).count();
    let chips = row()
        .key("timing-cdc-chips")
        .w_full()
        .flex_wrap()
        .gaps(4.0, 6.0)
        .child(count_chip(
            "timing-cdc-critical",
            "CRITICAL",
            count("Critical"),
            t.fail,
            t,
        ))
        .child(count_chip(
            "timing-cdc-warning",
            "WARNING",
            count("Warning"),
            t.warn,
            t,
        ))
        .child(count_chip(
            "timing-cdc-info",
            "INFO",
            count("Info"),
            t.accent,
            t,
        ))
        .child(count_chip(
            "timing-cdc-safe",
            "SAFE",
            count("Safe"),
            t.pass,
            t,
        ));
    let mut page = col()
        .key("timing-cdc")
        .w_full()
        .gap(4.0)
        .child(kit::section_title("Clock Domain Crossings", t))
        .child(chips);
    if s.timing.cdc.is_empty() {
        return page.child(kit::empty_state(
            "timing-cdc-empty",
            "No clock-domain crossings — single-clock design.",
            None,
            t,
        ));
    }
    let head = header_row(t)
        .child(kit::header_cell("From", t).grow(1.0).min_w(0.0))
        .child(kit::header_cell("To", t).grow(1.0).min_w(0.0))
        .child(el().w(SHORT_W).child(kit::header_cell("Severity", t)))
        .child(el().w(SHORT_W).child(kit::header_cell("Check", t)))
        .child(el().w(40.0).child(kit::header_cell("Sync", t)))
        .child(kit::header_num("Endpoints", COUNT_W, t))
        .child(kit::header_num("Worst (ns)", NS_W, t))
        .child(kit::header_cell("Relation", t).grow(1.0).min_w(0.0));
    let mut body = col().w_full();
    for (i, v) in s.timing.cdc.iter().enumerate() {
        body = body.child(
            plain_row(format!("timing-cdc-row-{i}"))
                .child(kit::text_cell(v.from.clone(), 1.0, t.text))
                .child(kit::text_cell(v.to.clone(), 1.0, t.text))
                .child(kit::fixed_cell(
                    v.severity.clone(),
                    SHORT_W,
                    severity_color(&v.severity, t),
                ))
                .child(kit::fixed_cell(v.check.clone(), SHORT_W, t.text))
                .child(kit::fixed_cell(
                    if v.sync { "Yes" } else { "No" },
                    40.0,
                    t.text_dim,
                ))
                .child(num_cell(v.endpoints.to_string(), COUNT_W, t.text))
                .child(num_cell(ns_opt(v.wns_ps), NS_W, slack_color(v.wns_ps, t)))
                .child(kit::text_cell(v.relation.clone(), 1.0, t.text_dim)),
        );
    }
    page = page.child(table(head, body, t));
    page
}

/// Clock Networks section: totals line plus `Clock | Period | Source | Net
/// | Loads | Buffers | Fanout | Insertion`.
pub fn networks_section(s: &AppState, t: &VivadoTheme) -> El {
    let nets = &s.timing.clock_networks;
    let mut page = col()
        .key("timing-clknet")
        .w_full()
        .gap(4.0)
        .child(kit::section_title("Clock Networks", t));
    if nets.is_empty() {
        return page.child(kit::empty_state(
            "timing-clknet-empty",
            "No clock trees in the last analysis.",
            None,
            t,
        ));
    }
    let loads: usize = nets.iter().map(|n| n.loads).sum();
    let buffers: usize = nets.iter().map(|n| n.buffers).sum();
    let max_ins = nets.iter().map(|n| n.insertion_ps).max().unwrap_or(0);
    page = page.child(
        text(format!(
            "{loads} loads · {buffers} buffers · {} ns maximum insertion",
            ns3(max_ins)
        ))
        .font(VivadoTheme::FONT_UI)
        .color(t.text_dim)
        .key("timing-clknet-totals"),
    );
    let head = header_row(t)
        .child(kit::header_cell("Clock", t).grow(1.0).min_w(0.0))
        .child(kit::header_num("Period (ns)", NS_W, t))
        .child(kit::header_cell("Source", t).grow(1.0).min_w(0.0))
        .child(kit::header_cell("Net", t).grow(2.0).min_w(0.0))
        .child(kit::header_num("Loads", 52.0, t))
        .child(kit::header_num("Buffers", 52.0, t))
        .child(kit::header_num("Fanout", 52.0, t))
        .child(kit::header_num("Insertion (ns)", 88.0, t));
    let mut body = col().w_full();
    for (i, n) in nets.iter().enumerate() {
        body = body.child(
            plain_row(format!("timing-clknet-row-{i}"))
                .child(kit::text_cell(n.name.clone(), 1.0, t.text))
                .child(num_cell(ns3(n.period_ps as i64), NS_W, t.text))
                .child(kit::text_cell(n.source.clone(), 1.0, t.text))
                .child(kit::text_cell(n.net.clone(), 2.0, t.text))
                .child(num_cell(n.loads.to_string(), 52.0, t.text))
                .child(num_cell(n.buffers.to_string(), 52.0, t.text))
                .child(num_cell(n.fanout.to_string(), 52.0, t.text))
                .child(num_cell(ns3(n.insertion_ps), 88.0, t.text)),
        );
    }
    page.child(table(head, body, t))
}

/// Path-group table: `Group | From | To | WNS | TNS | WHS | THS |
/// Endpoints` (setup and hold slacks of `report_timing_summary`).
pub fn groups_table(
    key_prefix: &'static str,
    title: &'static str,
    groups: &[&TimingGroupRow],
    t: &VivadoTheme,
) -> El {
    let head = header_row(t)
        .child(kit::header_cell("Group", t).grow(2.0).min_w(0.0))
        .child(kit::header_cell("From", t).grow(1.0).min_w(0.0))
        .child(kit::header_cell("To", t).grow(1.0).min_w(0.0))
        .child(kit::header_num("WNS (ns)", NS_W, t))
        .child(kit::header_num("TNS (ns)", NS_W, t))
        .child(kit::header_num("WHS (ns)", NS_W, t))
        .child(kit::header_num("THS (ns)", NS_W, t))
        .child(kit::header_num("Endpoints", COUNT_W, t));
    let mut body = col().w_full();
    for (i, g) in groups.iter().enumerate() {
        body = body.child(
            plain_row(format!("{key_prefix}-row-{i}"))
                .child(kit::text_cell(g.name.clone(), 2.0, t.text))
                .child(kit::text_cell(g.from.clone(), 1.0, t.text_dim))
                .child(kit::text_cell(g.to.clone(), 1.0, t.text_dim))
                .child(num_cell(ns_opt(g.wns_ps), NS_W, slack_color(g.wns_ps, t)))
                .child(num_cell(
                    ns3(g.tns_ps),
                    NS_W,
                    slack_color(Some(g.tns_ps), t),
                ))
                .child(num_cell(ns_opt(g.whs_ps), NS_W, slack_color(g.whs_ps, t)))
                .child(num_cell(
                    ns3(g.ths_ps),
                    NS_W,
                    slack_color(Some(g.ths_ps), t),
                ))
                .child(num_cell(g.endpoints.to_string(), COUNT_W, t.text)),
        );
    }
    col()
        .key(key_prefix)
        .w_full()
        .gap(2.0)
        .child(kit::section_title(title, t))
        .child(table(head, body, t))
}

/// Endpoint slack histogram (16 bins; red where a bin holds negative
/// slack) with its `N endpoints (M violating)` header.
pub fn slack_histogram(s: &AppState, t: &VivadoTheme) -> El {
    let slacks: Vec<i64> = s.timing.paths.iter().map(|p| p.slack_ps).collect();
    let violating = slacks.iter().filter(|v| **v < 0).count();
    let head = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(kit::section_title("Endpoint Slack Histogram", t))
        .child(
            text(format!(
                "{} endpoint{} ({violating} violating)",
                slacks.len(),
                if slacks.len() == 1 { "" } else { "s" }
            ))
            .font(VivadoTheme::FONT_UI)
            .color(if violating > 0 { t.fail } else { t.text_dim })
            .key("timing-hist-count"),
        );
    let block = col().key("timing-hist").w_full().gap(4.0).child(head);
    if slacks.is_empty() {
        return block.child(kit::empty_state(
            "timing-hist-empty",
            "No endpoint paths — paths need placement.",
            None,
            t,
        ));
    }
    let min = slacks.iter().copied().min().unwrap_or(0);
    let max = slacks.iter().copied().max().unwrap_or(0);
    let (lo, hi) = if min == max {
        (min - 500, max + 500)
    } else {
        (min, max)
    };
    let span = (hi - lo).max(1) as f64;
    let mut counts = [0usize; BINS];
    let mut bin_min = [i64::MAX; BINS];
    for v in &slacks {
        let b = ((((v - lo) as f64) / span) * BINS as f64).floor() as usize;
        let b = b.min(BINS - 1);
        counts[b] += 1;
        bin_min[b] = bin_min[b].min(*v);
    }
    let peak = counts.iter().copied().max().unwrap_or(1).max(1) as f32;
    let mut bars = row()
        .key("timing-hist-bars")
        .w_full()
        .h(PLOT_H + 16.0)
        .gap(3.0)
        .px(kit::PAD_X)
        .py(4.0)
        .fill(t.panel_bg)
        .border(1.0, t.border);
    for (i, count) in counts.iter().enumerate() {
        let lo_ns = lo as f64 + span * i as f64 / BINS as f64;
        let hi_ns = lo as f64 + span * (i + 1) as f64 / BINS as f64;
        let tip = format!(
            "{count} endpoint{} between {:.3} and {:.3} ns",
            if *count == 1 { "" } else { "s" },
            lo_ns / 1000.0,
            hi_ns / 1000.0
        );
        let mut slot = col()
            .key(format!("timing-hist-bin-{i}"))
            .grow(1.0)
            .min_w(0.0)
            .h_full()
            .justify_end()
            .gap(1.0)
            .help(tip);
        if *count > 0 {
            let h = (*count as f32 / peak * PLOT_H).max(4.0);
            let color = if bin_min[i] < 0 { t.fail } else { t.accent };
            slot = slot
                .child(
                    text(count.to_string())
                        .font(VivadoTheme::FONT_CAPS)
                        .color(t.text)
                        .w_full()
                        .align(rune::Align::Center),
                )
                .child(el().w_full().h(h).radius(2.0).fill(color));
        } else {
            slot = slot.child(el().w_full().h(2.0).fill(t.border_light));
        }
        bars = bars.child(slot);
    }
    block.child(bars).child(
        row()
            .w_full()
            .justify_between()
            .child(
                text(format!("Min {} ns", ns3(lo)))
                    .font(VivadoTheme::FONT_MONO.mono())
                    .color(t.text_faint)
                    .key("timing-hist-min"),
            )
            .child(
                text(format!("Max {} ns", ns3(hi)))
                    .font(VivadoTheme::FONT_MONO.mono())
                    .color(t.text_faint)
                    .key("timing-hist-max"),
            ),
    )
}
