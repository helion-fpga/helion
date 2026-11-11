//! Design Timing Summary + grouped path tables (center pane).
//!
//! New Rune `El` code written from scratch for the Vivado brief
//! (`/tmp/vivado-ref/lab3-Fig9.png` Timing tab: left report tree plus the
//! 3-column Setup/Hold/Pulse Width WNS/TNS summary and its bold verdict;
//! `lab3-Fig12.png`/`lab3-Fig7.png` grouped Source Clock Path / Data Path /
//! Destination Clock Path tables with `Delay Type | Incr (ns) | Path (ns) |
//! Location | Netlist Resource(s)` columns, `(r)` rise markers, blue linked
//! cells, and bold Arrival/Required Time footers). All colors come from
//! [`VivadoTheme`]; selection is the typed [`TimingNav`] enum (no stringly
//! status, no hardcoded hex, no manual geometry, no unwraps on UI state).
//!
//! Data shapes mirror `helion-gui` read-only: [`TimingState`] numbers mirror
//! `helion_sta::TimingSummary` picosecond slacks, clocks/groups/paths mirror
//! the STA reports, and the pin tables render `ide.rs::TimingPathPin`-shaped
//! rows (`cell`/`incr`/`path`/`location`/`net`/`fanout`) from real run state.
//! Every table binds [`AppState::timing`]; empty sections render a disabled
//! reason, never demo constants. The path table/detail live in [`paths`];
//! the clock interaction matrix, CDC, clock networks, groups and the slack
//! histogram live in [`clocks`].
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el.rs`: `row()/col()/el()/text()/icon()`, `ScrollSpec`.
//! - `crates/rune/src/el/scroll.rs`: `.scroll()` vertical viewport.
//! - `crates/rune/src/el/text.rs`: `.truncate()` for long labels.
//! - `crates/rune-kit/src/glyphs.rs`: `TRIANGLE_RIGHT`, `CIRCLE_X`.

mod clocks;
mod paths;

pub use paths::{PathSort, path_elements, toggle_sort};

use rune::{El, ScrollSpec, col, el, icon, row, text};
use rune_kit::glyphs;

use crate::state::{
    AppState, RunStatus, TimingGroupKind, TimingPathGroup, TimingPathRow, TimingState,
};
use crate::theme::VivadoTheme;
use crate::views::report_kit as kit;
use crate::views::sources::pane_header;
use crate::views::summary::format_ns;

use paths::PinRow;

/// Report-tree width (the brief shows a narrow left nav beside wide tables).
const NAV_W: f32 = 220.0;
/// Report-tree row height.
const NAV_ROW_H: f32 = 24.0;
/// Summary `label: value` row height.
const ROW_H: f32 = 20.0;
/// Summary label width (fits "Total Pulse Width Negative Slack (TPWS):").
const LABEL_W: f32 = 208.0;
/// Summary-grid row-label width (short metric names; acronyms live in the
/// column headers so the three analysis columns fit the center pane).
const SUM_LABEL_W: f32 = 150.0;
/// Check-table short column width.
const CHECK_W: f32 = 110.0;
/// Check-table severity column width.
const SEV_W: f32 = 110.0;

/// Left report-tree entries, in `lab3-Fig9.png` order (plus the UG949
/// Clock Interaction report after the clock summary). Typed so selection
/// is never a string.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimingNav {
    GeneralInfo,
    TimerSettings,
    #[default]
    Summary,
    ClockSummary,
    ClockInteraction,
    CheckTiming,
    IntraClock,
    InterClock,
    OtherGroups,
}

impl TimingNav {
    pub const ALL: [TimingNav; 9] = [
        TimingNav::GeneralInfo,
        TimingNav::TimerSettings,
        TimingNav::Summary,
        TimingNav::ClockSummary,
        TimingNav::ClockInteraction,
        TimingNav::CheckTiming,
        TimingNav::IntraClock,
        TimingNav::InterClock,
        TimingNav::OtherGroups,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TimingNav::GeneralInfo => "General Information",
            TimingNav::TimerSettings => "Timer Settings",
            TimingNav::Summary => "Design Timing Summary",
            TimingNav::ClockSummary => "Clock Summary",
            TimingNav::ClockInteraction => "Clock Interaction",
            TimingNav::CheckTiming => "Check Timing",
            TimingNav::IntraClock => "Intra-Clock Paths",
            TimingNav::InterClock => "Inter-Clock Paths",
            TimingNav::OtherGroups => "Other Path Groups",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            TimingNav::GeneralInfo => "timing-nav-general",
            TimingNav::TimerSettings => "timing-nav-settings",
            TimingNav::Summary => "timing-nav-summary",
            TimingNav::ClockSummary => "timing-nav-clocks",
            TimingNav::ClockInteraction => "timing-nav-interaction",
            TimingNav::CheckTiming => "timing-nav-check",
            TimingNav::IntraClock => "timing-nav-intra",
            TimingNav::InterClock => "timing-nav-inter",
            TimingNav::OtherGroups => "timing-nav-other",
        }
    }

    /// The brief shows collapsed twisties on the check/path groups.
    fn twisty(self) -> bool {
        matches!(
            self,
            TimingNav::CheckTiming
                | TimingNav::IntraClock
                | TimingNav::InterClock
                | TimingNav::OtherGroups
        )
    }
}

/// Timing center-pane UI state: which report-tree entry is shown, which
/// real path the detail tables break down (`None` = worst slack first),
/// the selected clock-interaction cell, and the path-table filter/sort.
#[derive(Clone, Debug, Default)]
pub struct TimingView {
    pub selected: TimingNav,
    pub selected_path: Option<usize>,
    /// Index into [`TimingState::clock_interaction`].
    pub selected_interaction: Option<usize>,
    /// Path-table filter (From / To / group, case-insensitive).
    pub path_filter: rune::TextEdit,
    /// Path-table sort column and direction (`true` = ascending).
    pub path_sort: Option<(PathSort, bool)>,
}

/// The path the detail tables break down: the tapped selection when valid,
/// else the worst-slack path runs store first.
pub(crate) fn detail_path(s: &AppState) -> Option<(usize, &TimingPathRow)> {
    if let Some(i) = s.timing_view.selected_path {
        if let Some(p) = s.timing.paths.get(i) {
            return Some((i, p));
        }
    }
    s.timing.paths.iter().enumerate().next()
}

/// Groups of one kind for the Intra/Inter/Other report pages.
fn family_groups(s: &AppState, kind: TimingGroupKind) -> Vec<&crate::state::TimingGroupRow> {
    s.timing.groups.iter().filter(|g| g.kind == kind).collect()
}

fn ui_label(content: String, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text)
}

fn dim_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text_dim)
}

/// Whether every reported constraint is met (negative slack fails).
fn constraints_met(timing: &TimingState) -> bool {
    timing.wns_ps.unwrap_or(0) >= 0
        && timing.whs_ps.unwrap_or(0) >= 0
        && timing.tns_ps >= 0
        && timing.ths_ps >= 0
        && timing.failing_setup == 0
        && timing.failing_hold == 0
}

/// One report-tree row: twisty, label, and a red alert on the summary entry
/// while constraints fail. The clock entry carries the real clock count.
fn nav_row(nav: TimingNav, s: &AppState, t: &VivadoTheme) -> El {
    let selected = s.timing_view.selected == nav;
    let mut content = row().items_center().gap(4.0);
    if nav.twisty() {
        content = content.child(
            icon(&glyphs::TRIANGLE_RIGHT, 10.0)
                .color(t.text_dim)
                .a11y_hidden(),
        );
    }
    let label = if nav == TimingNav::ClockSummary {
        format!("{} ({})", nav.label(), s.timing.clocks.len())
    } else {
        nav.label().into()
    };
    content = content.child(ui_label(label, t).truncate());
    if nav == TimingNav::Summary && s.timing.has_run && !constraints_met(&s.timing) {
        content = content.child(icon(&glyphs::CIRCLE_X, 12.0).color(t.fail).a11y_hidden());
    }
    let mut item = el()
        .key(nav.key())
        .role("button")
        .label(format!("Timing report: {}", nav.label()))
        .focusable(true)
        .w_full()
        .h(NAV_ROW_H)
        .px(8.0)
        .center_y()
        .hover(t.selection)
        .on_press(move |s: &mut AppState| s.timing_view.selected = nav)
        .child(content);
    if selected {
        item = item.fill(t.selection);
    }
    item
}

/// Slack value as a blue link (red when negative), drilling into the
/// Intra-Clock paths on press.
fn slack_link(key: &'static str, ps: i64, t: &VivadoTheme) -> El {
    let color = if ps < 0 { t.fail } else { t.link };
    el().key(key)
        .role("button")
        .label(format!("Slack {}: show paths", format_ns(ps)))
        .focusable(true)
        .h(ROW_H)
        .px(2.0)
        .center_y()
        .hover(t.selection)
        .on_press(|s: &mut AppState| s.timing_view.selected = TimingNav::IntraClock)
        .child(text(format_ns(ps)).font(VivadoTheme::FONT_UI).color(color))
}

/// Disabled `n/a` cell for analyses the engine does not run (pulse
/// width): visibly empty with the reason in its label, never a number.
fn na_cell(key: &'static str, t: &VivadoTheme) -> El {
    el().key(key)
        .h(ROW_H)
        .center_y()
        .label("Not analyzed by helion-sta")
        .help("Pulse-width checks are not in helion-sta")
        .child(dim_label("n/a", t))
}

/// One `label: value` row (info cards use runtime labels).
fn kv(label_text: &str, value: El, t: &VivadoTheme) -> El {
    row()
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(6.0)
        .child(el().w(LABEL_W).child(dim_label(label_text, t).truncate()))
        .child(value)
}

/// One summary-grid row: metric label plus the Setup/Hold/Pulse Width cells.
fn summary_grid_row(label_text: &'static str, cells: [El; 3], t: &VivadoTheme) -> El {
    let mut grid_row = row().w_full().h(ROW_H).items_center().gap(8.0).child(
        el().w(SUM_LABEL_W)
            .child(dim_label(label_text, t).truncate()),
    );
    for cell in cells {
        grid_row = grid_row.child(el().grow(1.0).min_w(0.0).child(cell));
    }
    grid_row
}

/// The Setup/Hold/Pulse Width WNS/TNS grid plus its bold verdict.
///
/// `lab3-Fig9.png` repeats full `label: value` pairs per column; the shell's
/// center pane is narrower than Vivado's full-width Timing tab, so the same
/// three analyses and four metrics share one grid: metric rows, analysis
/// columns with their slack acronyms in the headers.
fn summary_block(s: &AppState, t: &VivadoTheme) -> El {
    let met = constraints_met(&s.timing);
    let verdict = if met {
        text("All user specified timing constraints are met.")
            .font(VivadoTheme::FONT_TITLE)
            .color(t.pass)
            .key("timing-verdict")
    } else {
        text("Timing constraints are not met.")
            .font(VivadoTheme::FONT_TITLE)
            .color(t.text)
            .key("timing-verdict")
    };
    let header = |key: &'static str, title: &'static str, slacks: &'static str| -> El {
        col()
            .key(key)
            .gap(0.0)
            .child(
                text(title)
                    .font(VivadoTheme::FONT_TITLE)
                    .color(t.text)
                    .truncate(),
            )
            .child(
                text(slacks)
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_dim)
                    .truncate(),
            )
    };
    col()
        .key("timing-summary")
        .w_full()
        .gap(4.0)
        .child(
            text("Design Timing Summary")
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text)
                .key("timing-title"),
        )
        // Non-HL10T parts: say which delay model produced the numbers.
        .children((!s.timing.model.is_empty()).then(|| {
            text(format!("Delay model: {}", s.timing.model))
                .font(VivadoTheme::FONT_UI)
                .color(t.warn)
                .key("timing-model")
                .w_full()
                .truncate()
                .help_when_truncated(s.timing.model.clone())
        }))
        .child(
            row()
                .w_full()
                .h(34.0)
                .items_center()
                .gap(8.0)
                .child(el().w(SUM_LABEL_W))
                .child(
                    header("timing-setup-title", "Setup", "(WNS, TNS)")
                        .grow(1.0)
                        .min_w(0.0),
                )
                .child(
                    header("timing-hold-title", "Hold", "(WHS, THS)")
                        .grow(1.0)
                        .min_w(0.0),
                )
                .child(
                    header("timing-pulse-title", "Pulse Width", "(WPWS, TPWS)")
                        .grow(1.0)
                        .min_w(0.0),
                ),
        )
        .child(summary_grid_row(
            "Worst Slack:",
            [
                slack_link("timing-setup-wns", s.timing.wns_ps.unwrap_or(0), t),
                slack_link("timing-hold-whs", s.timing.whs_ps.unwrap_or(0), t),
                na_cell("timing-pulse-wpws", t),
            ],
            t,
        ))
        .child(summary_grid_row(
            "Total Slack:",
            [
                slack_link("timing-setup-tns", s.timing.tns_ps, t),
                slack_link("timing-hold-ths", s.timing.ths_ps, t),
                na_cell("timing-pulse-tpws", t),
            ],
            t,
        ))
        .child(summary_grid_row(
            "Failing Endpoints:",
            [
                ui_label(s.timing.failing_setup.to_string(), t).key("timing-setup-failing"),
                ui_label(s.timing.failing_hold.to_string(), t).key("timing-hold-failing"),
                na_cell("timing-pulse-failing", t),
            ],
            t,
        ))
        .child(summary_grid_row(
            "Total Endpoints:",
            [
                ui_label(s.timing.endpoints.to_string(), t).key("timing-setup-endpoints"),
                ui_label(s.timing.endpoints.to_string(), t).key("timing-hold-endpoints"),
                na_cell("timing-pulse-endpoints", t),
            ],
            t,
        ))
        .child(
            dim_label(
                "Pulse-width checks are not in helion-sta — Setup/Hold come from the last run.",
                t,
            )
            .key("timing-pulse-note"),
        )
        .child(verdict)
}

/// The selected path's detail: delay-split header, element chips, and the
/// three grouped pin tables (launch clock context plus launch pins, the
/// UG903 data rows, and the requirement derived from the run's clocks plus
/// this path's slack).
fn detail_tables(s: &AppState, t: &VivadoTheme) -> El {
    let Some((_, path)) = detail_path(s) else {
        return col().key("timing-detail").w_full().gap(8.0).child(
            dim_label(
                "No timing paths — paths need placement (Run Implementation).",
                t,
            )
            .key("timing-detail-path"),
        );
    };
    let data = paths::data_rows(path);
    let clk = s
        .timing
        .clocks
        .iter()
        .find(|c| c.name == path.launch_clock)
        .or_else(|| s.timing.clocks.first());
    let clk_name = clk.map(|c| c.name.as_str()).unwrap_or("clk");
    let clk_source = clk.map(|c| c.source.as_str()).unwrap_or("clk");
    let net = s.timing.clock_networks.iter().find(|n| n.name == clk_name);
    let (net_name, loads, insertion_ns) = match net {
        Some(n) => (n.net.clone(), n.loads, n.insertion_ps as f64 / 1000.0),
        None => (clk_source.to_string(), 0, 0.0),
    };
    let mut src = vec![
        PinRow {
            incr_ns: Some(0.0),
            path_ns: Some(0.0),
            ..PinRow::plain(format!("(clock {clk_name} rise edge)"), clk_source)
        },
        PinRow {
            net: net_name.clone(),
            fanout: Some(loads),
            ..PinRow::plain("clock net", net_name.clone())
        },
        PinRow {
            incr_ns: Some(insertion_ns),
            path_ns: Some(insertion_ns),
            net: net_name.clone(),
            ..PinRow::plain("clock network insertion", net_name)
        },
    ];
    match path.pins.iter().find(|p| p.delay_type == "clk") {
        Some(launch) => {
            src.push(PinRow {
                loc: launch.location.clone(),
                net: launch.net.clone(),
                fanout: Some(launch.fanout),
                ..PinRow::plain("", launch.pin.clone())
            });
            src.push(PinRow {
                loc: launch.location.clone(),
                link_cell: Some(launch.cell.clone()),
                ..PinRow::plain("launch cell", launch.cell.clone())
            });
        }
        None => {
            src.push(PinRow::plain(
                "No launch register on this path (combinational startpoint)",
                "",
            ));
        }
    }
    let arrival_ns = path.delay_ps as f64 / 1000.0;
    let required_ns = arrival_ns + path.slack_ps as f64 / 1000.0;
    let capture = s
        .timing
        .clocks
        .iter()
        .find(|c| c.name == path.capture_clock)
        .or(clk);
    let cap_name = capture.map(|c| c.name.as_str()).unwrap_or(clk_name);
    let period_ns = capture.map(|c| c.period_ps as f64 / 1000.0).unwrap_or(0.0);
    let dst = vec![
        PinRow {
            path_ns: Some(required_ns),
            ..PinRow::plain(format!("(clock {cap_name} capture edge)"), "")
        },
        PinRow {
            incr_ns: Some(period_ns),
            path_ns: Some(period_ns),
            ..PinRow::plain("clock period", cap_name)
        },
        PinRow::plain("clock uncertainty (not modeled by helion-sta)", ""),
    ];
    col()
        .key("timing-detail")
        .w_full()
        .gap(8.0)
        .child(paths::detail_header(path, t))
        .child(paths::pin_table(
            "timing-src-clk",
            "Source Clock Path",
            &src,
            None,
            t,
        ))
        .child(paths::pin_table(
            "timing-data",
            "Data Path",
            &data,
            Some(("Arrival Time", "timing-arrival", arrival_ns)),
            t,
        ))
        .child(paths::pin_table(
            "timing-dst-clk",
            "Destination Clock Path",
            &dst,
            Some(("Required Time", "timing-required", required_ns)),
            t,
        ))
}

/// Disabled section placeholder: the section title plus the honest
/// reason it has no rows (unwired engine data is never faked).
fn empty_section(key_prefix: &str, title: &str, reason: &str, t: &VivadoTheme) -> El {
    col()
        .key(key_prefix)
        .w_full()
        .gap(2.0)
        .child(
            text(title.to_string())
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text),
        )
        .child(kit::empty_state(
            format!("{key_prefix}-empty"),
            reason,
            None,
            t,
        ))
}

/// Paths section of one family: the 8-column table, or the honest reason
/// it has no rows.
fn paths_section(
    s: &AppState,
    family: TimingPathGroup,
    title: &'static str,
    reason: &str,
    t: &VivadoTheme,
) -> El {
    if s.timing.paths.iter().any(|p| p.group == family) {
        paths::paths_table(s, family, title, t)
    } else {
        empty_section("timing-paths", title, reason, t)
    }
}

/// Clock Summary page: the run's real STA clocks plus their clock-tree
/// rows (loads/buffers/insertion from `report_clock_networks`).
fn clock_table(s: &AppState, t: &VivadoTheme) -> El {
    if s.timing.clocks.is_empty() {
        return empty_section(
            "timing-clocks",
            "Clock Summary",
            "No clocks — run with an SDC/XDC or the implicit 10ns clock.",
            t,
        );
    }
    let head = kit::header_row(t)
        .child(kit::header_cell("Clock", t).grow(2.0).min_w(0.0))
        .child(kit::header_num("Period (ns)", 88.0, t))
        .child(kit::header_num("Frequency (MHz)", 112.0, t))
        .child(kit::header_cell("Source", t).grow(1.0).min_w(0.0))
        .child(kit::header_cell("Generated", t).grow(1.0).min_w(0.0));
    let mut body = col().w_full();
    for (index, clk) in s.timing.clocks.iter().enumerate() {
        let freq = if clk.period_ps > 0 {
            format!("{:.3}", 1_000_000.0 / clk.period_ps as f64)
        } else {
            "n/a".into()
        };
        let generated = if clk.generated {
            format!("yes ({})", clk.master.as_deref().unwrap_or("-"))
        } else {
            "no".into()
        };
        body = body.child(
            kit::plain_row(format!("timing-clock-row-{index}"))
                .child(kit::text_cell(clk.name.clone(), 2.0, t.link))
                .child(kit::num_cell(kit::ns3(clk.period_ps as i64), 88.0, t.text))
                .child(kit::num_cell(freq, 112.0, t.text))
                .child(kit::text_cell(clk.source.clone(), 1.0, t.text_dim))
                .child(kit::text_cell(generated, 1.0, t.text_dim)),
        );
    }
    col()
        .key("timing-clocks")
        .w_full()
        .gap(10.0)
        .child(
            col()
                .w_full()
                .gap(2.0)
                .child(kit::section_title("Clock Summary", t))
                .child(kit::table(head, body, t)),
        )
        .child(clocks::networks_section(s, t))
}

/// Methodology checks table (`Check | Severity | Objects | Message`) from
/// the real `report_methodology` rows.
fn method_table(s: &AppState, t: &VivadoTheme) -> El {
    let head = kit::header_row(t)
        .child(el().w(CHECK_W).child(kit::header_cell("Check", t)))
        .child(el().w(SEV_W).child(kit::header_cell("Severity", t)))
        .child(kit::header_cell("Objects", t).grow(2.0).min_w(0.0))
        .child(kit::header_cell("Message", t).grow(3.0).min_w(0.0));
    let mut body = col().w_full();
    for (index, v) in s.timing.methodology.iter().enumerate() {
        let ink = match v.severity.as_str() {
            "Error" | "CriticalWarning" => t.fail,
            "Warning" => t.warn,
            _ => t.text,
        };
        body = body.child(
            kit::plain_row(format!("timing-method-row-{index}"))
                .child(kit::fixed_cell(v.id.clone(), CHECK_W, t.text))
                .child(kit::fixed_cell(v.severity.clone(), SEV_W, ink))
                .child(kit::text_cell(v.objects.clone(), 2.0, t.text))
                .child(kit::text_cell(v.message.clone(), 3.0, t.text)),
        );
    }
    col()
        .key("timing-method")
        .w_full()
        .gap(2.0)
        .child(kit::section_title("Methodology Checks", t))
        .child(kit::table(head, body, t))
}

/// Small `label: value` info card for the non-table report pages.
/// Rows carry real run values (design/part/counts), never demo text.
fn info_card(
    key: &'static str,
    title: &'static str,
    rows: &[(String, String)],
    t: &VivadoTheme,
) -> El {
    let mut body = col().w_full().padding(8.0).gap(2.0);
    for (label_text, value) in rows {
        body = body.child(kv(label_text, ui_label(value.clone(), t), t));
    }
    col()
        .key(key)
        .w_full()
        .gap(2.0)
        .child(text(title).font(VivadoTheme::FONT_TITLE).color(t.text))
        .child(
            col()
                .w_full()
                .fill(t.panel_bg)
                .border(1.0, t.border)
                .child(body),
        )
}

/// Empty report: a run-in-progress notice, or one sentence plus the one
/// call-to-action that really launches the implementation run.
fn no_run_content(s: &AppState, t: &VivadoTheme) -> El {
    let running = s.flow.impl_run == RunStatus::Running || s.flow.synth == RunStatus::Running;
    if running {
        return col().w_full().padding(8.0).child(
            row()
                .key("timing-run-status")
                .w_full()
                .h(40.0)
                .items_center()
                .justify_center()
                .gap(4.0)
                .child(dim_label(
                    "A run is in progress — follow it in the Tcl Console.",
                    t,
                )),
        );
    }
    col().w_full().padding(8.0).child(
        row()
            .key("timing-no-run")
            .w_full()
            .h(40.0)
            .items_center()
            .justify_center()
            .gap(4.0)
            .child(kit::link(
                "timing-run-impl",
                "Run Implementation",
                t,
                |s: &mut AppState| crate::runs::launch(s, crate::runs::RunKind::Impl),
            ))
            .child(dim_label("to see timing results.", t)),
    )
}

/// Right-hand report content for the selected tree entry.
fn report_content(s: &AppState, t: &VivadoTheme) -> El {
    if !s.timing.has_run {
        return no_run_content(s, t);
    }
    let mut content = col().w_full().gap(12.0).padding(8.0);
    match s.timing_view.selected {
        TimingNav::Summary => {
            content = content
                .child(summary_block(s, t))
                .child(clocks::slack_histogram(s, t))
                .child(detail_tables(s, t));
        }
        TimingNav::ClockSummary => {
            content = content.child(clock_table(s, t));
        }
        TimingNav::ClockInteraction => {
            content = content.child(clocks::interaction_page(s, t));
        }
        TimingNav::GeneralInfo => {
            let design = if s.timing.design.is_empty() {
                s.project.name.clone()
            } else {
                s.timing.design.clone()
            };
            content = content.child(info_card(
                "timing-info-card",
                "General Information",
                &[
                    ("Design:".into(), design),
                    ("Device:".into(), s.project.part.clone()),
                    ("Report:".into(), "timing_1".into()),
                    ("Endpoints:".into(), s.timing.endpoints.to_string()),
                ],
                t,
            ));
        }
        TimingNav::TimerSettings => {
            let voltage = if s.timing.voltage_mv > 0 {
                format!("{} mV", s.timing.voltage_mv)
            } else {
                "default".into()
            };
            let temp = if s.timing.temp_c != 0 {
                format!("{} °C", s.timing.temp_c)
            } else {
                "default".into()
            };
            content = content.child(info_card(
                "timing-info-card",
                "Timer Settings",
                &[
                    ("Timing engine:".into(), "helion-sta (graph STA)".into()),
                    (
                        "Analysis corners:".into(),
                        "Single (no multi-corner)".into(),
                    ),
                    ("Operating voltage:".into(), voltage),
                    ("Operating temp:".into(), temp),
                ],
                t,
            ));
        }
        TimingNav::CheckTiming => {
            content = content
                .child(info_card(
                    "timing-info-card",
                    "Check Timing",
                    &[
                        (
                            "Methodology checks:".into(),
                            s.timing.methodology.len().to_string(),
                        ),
                        ("CDC crossings:".into(), s.timing.cdc.len().to_string()),
                        ("Clocks:".into(), s.timing.clocks.len().to_string()),
                        ("Endpoints:".into(), s.timing.endpoints.to_string()),
                    ],
                    t,
                ))
                .child(check_tables(s, t));
        }
        TimingNav::IntraClock => {
            let groups = family_groups(s, TimingGroupKind::Intra);
            content = content
                .child(groups_section(
                    "timing-groups",
                    "Intra-Clock Groups",
                    &groups,
                    "No intra-clock groups — the summary groups bind when a run analyzes clocks.",
                    t,
                ))
                .child(paths_section(
                    s,
                    TimingPathGroup::Register,
                    "Intra-Clock Paths",
                    "No register paths — paths need placement (Run Implementation).",
                    t,
                ))
                .child(detail_tables(s, t));
        }
        TimingNav::InterClock => {
            let groups = family_groups(s, TimingGroupKind::Inter);
            content = content.child(groups_section(
                "timing-groups",
                "Inter-Clock Groups",
                &groups,
                "No inter-clock groups — single-clock design.",
                t,
            ));
            if s.timing.cdc.is_empty() {
                content = content.child(empty_section(
                    "timing-paths",
                    "Inter-Clock Paths",
                    "No CDC pairs — add a second clock to analyze crossings.",
                    t,
                ));
            } else {
                content = content
                    .child(clocks::cdc_section(s, t))
                    .child(inter_paths_note(t));
            }
        }
        TimingNav::OtherGroups => {
            let groups = family_groups(s, TimingGroupKind::Other);
            content = content
                .child(groups_section(
                    "timing-groups",
                    "Other Path Groups",
                    &groups,
                    "No group_path groups — user groups bind from XDC group_path.",
                    t,
                ))
                .child(paths_section(
                    s,
                    TimingPathGroup::Output,
                    "Output Paths",
                    "No output paths — paths need placement (Run Implementation).",
                    t,
                ))
                .child(detail_tables(s, t));
        }
    }
    content
}

/// Groups table or its disabled placeholder for one path-group kind.
fn groups_section(
    key_prefix: &'static str,
    title: &'static str,
    groups: &[&crate::state::TimingGroupRow],
    empty_reason: &str,
    t: &VivadoTheme,
) -> El {
    if groups.is_empty() {
        empty_section(key_prefix, title, empty_reason, t)
    } else {
        clocks::groups_table(key_prefix, title, groups, t)
    }
}

/// Methodology + CDC detail for the Check Timing page (cards stay mounted
/// with zero counts; tables mount only with real rows).
fn check_tables(s: &AppState, t: &VivadoTheme) -> El {
    let mut tables = col().w_full().gap(10.0);
    if s.timing.methodology.is_empty() {
        tables = tables.child(empty_section(
            "timing-method",
            "Methodology Checks",
            "No methodology violations reported.",
            t,
        ));
    } else {
        tables = tables.child(method_table(s, t));
    }
    tables.child(clocks::cdc_section(s, t))
}

/// Honest note under the Inter-Clock CDC table: per-path crossings need
/// per-path clock tags the engine does not emit yet.
fn inter_paths_note(t: &VivadoTheme) -> El {
    col().key("timing-paths").w_full().gap(2.0)
        .child(text("Inter-Clock Paths").font(VivadoTheme::FONT_TITLE).color(t.text))
        .child(
            dim_label(
                "Per-path crossings need per-path clock tags (helion-sta paths are clock-agnostic) — groups and CDC above carry the inter-clock result.",
                t,
            )
            .wrap()
            .key("timing-paths-note"),
        )
}

/// Re-run STA through the real `report_timing_summary` verb (a worker job;
/// the button stays busy until it lands).
fn rerun_summary(s: &mut AppState) {
    crate::tcl::eval_command(s, "report_timing_summary");
}

/// Action bar under the pane header: the report name plus the
/// Report Timing Summary (re-run) action.
fn action_bar(s: &AppState, t: &VivadoTheme) -> El {
    let stage = match s.runs.analysis.as_ref().map(|a| a.routed.is_some()) {
        Some(true) => "Post-implementation analysis",
        Some(false) => "Post-synthesis estimate",
        None if s.timing.has_run => "Last analysis",
        None => "No analysis yet",
    };
    row()
        .key("timing-actions")
        .w_full()
        .h(30.0)
        .items_center()
        .gap(8.0)
        .px(8.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], t.border_light)
        .child(
            text(stage)
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .truncate()
                .key("timing-stage"),
        )
        .child(el().grow(1.0))
        .child(kit::action_button(
            "timing-rerun",
            if s.jobs.pending(crate::jobs::JobKind::Timing) {
                "Analyzing…"
            } else {
                "Report Timing Summary"
            },
            crate::jobs::analysis_busy(s).or(crate::analysis::timing_unavailable(s)),
            t,
            rerun_summary,
        ))
}

/// The center Timing pane wired into the shell.
pub fn timing_pane(s: &AppState, t: &VivadoTheme) -> El {
    let mut nav = col().key("timing-nav").w_full().gap(0.0).py(4.0);
    for nav_entry in TimingNav::ALL {
        nav = nav.child(nav_row(nav_entry, s, t));
    }
    col()
        .key("timing-pane")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Timing", "timing", t))
        .child(action_bar(s, t))
        .child(
            row()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                // The report tree scrolls like the report body so long
                // report lists stay reachable at short window heights
                // instead of pushing content toward the console fold.
                .child(
                    col()
                        .key("timing-nav-scroll")
                        .w(NAV_W)
                        .min_h(0.0)
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .child(nav),
                )
                .child(el().w(1.0).grow(0.0).fill(t.border))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .min_h(0.0)
                        // The summary plus three path tables exceed the pane
                        // height, so the report scrolls (framework viewport,
                        // no manual geometry) while the footer stays pinned.
                        .child(
                            col()
                                .key("timing-scroll")
                                .grow(1.0)
                                .min_h(0.0)
                                .scroll(ScrollSpec {
                                    stick_bottom: false,
                                })
                                .child(report_content(s, t)),
                        )
                        .child(
                            el().w_full().px(8.0).py(2.0).child(
                                dim_label("Timing Summary - timing_1", t).key("timing-footer"),
                            ),
                        ),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn harness() -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(init(), crate::view, spec.size, env)
    }

    fn harness_with(state: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(state, crate::view, spec.size, env)
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    /// `lab3-Fig9.png` numbers: setup passes, hold fails on 2 endpoints.
    /// The clocks/groups/paths below are representative run output shaped
    /// like `TimingState` (single 10ns clock, worst path first); views must
    /// render state, never their own constants.
    fn fig9_state() -> AppState {
        use crate::state::{
            ClockNetRow, TimingClock, TimingGroupKind, TimingGroupRow, TimingPathGroup,
            TimingPathRow, TimingPinRow,
        };
        let mut s = init();
        s.center = crate::state::CenterPane::Timing;
        s.timing = TimingState {
            wns_ps: Some(2061),
            tns_ps: 0,
            whs_ps: Some(-1545),
            ths_ps: -3069,
            failing_endpoints: 2,
            failing_setup: 0,
            failing_hold: 2,
            endpoints: 112,
            has_run: true,
            clocks: vec![TimingClock {
                name: "clk".into(),
                period_ps: 10_000,
                source: "clk".into(),
                generated: false,
                master: None,
            }],
            groups: vec![TimingGroupRow {
                name: "clk".into(),
                from: "clk".into(),
                to: "clk".into(),
                kind: TimingGroupKind::Intra,
                wns_ps: Some(2061),
                tns_ps: 0,
                whs_ps: Some(-1545),
                ths_ps: -3069,
                endpoints: 112,
            }],
            paths: vec![TimingPathRow {
                name: "cnt_3->cnt_7".into(),
                from: "cnt_3".into(),
                to: "cnt_7".into(),
                slack_ps: 2061,
                delay_ps: 7939,
                group: TimingPathGroup::Register,
                launch_clock: "clk".into(),
                capture_clock: "clk".into(),
                pins: vec![
                    TimingPinRow {
                        cell: "cnt_3".into(),
                        pin: "cnt_3/CLK".into(),
                        delay_type: "clk".into(),
                        incr_ps: 0,
                        path_ps: 0,
                        net: "clk".into(),
                        fanout: 8,
                        location: "CLB_X1Y2".into(),
                    },
                    TimingPinRow {
                        cell: "cnt_3".into(),
                        pin: "cnt_3/Q".into(),
                        delay_type: "cell".into(),
                        incr_ps: 80,
                        path_ps: 80,
                        net: "cnt_q".into(),
                        fanout: 2,
                        location: "CLB_X1Y2".into(),
                    },
                    TimingPinRow {
                        cell: "cnt_7".into(),
                        pin: "cnt_7/D".into(),
                        delay_type: "setup".into(),
                        incr_ps: 50,
                        path_ps: 7939,
                        net: "cnt_d".into(),
                        fanout: 1,
                        location: "CLB_X1Y3".into(),
                    },
                ],
            }],
            clock_networks: vec![ClockNetRow {
                name: "clk".into(),
                period_ps: 10_000,
                source: "clk".into(),
                net: "clk".into(),
                loads: 8,
                buffers: 1,
                fanout: 8,
                insertion_ps: 120,
            }],
            design: "project1_demo".into(),
            voltage_mv: 1000,
            temp_c: 25,
            ..TimingState::default()
        };
        s
    }

    #[test]
    fn timing_tree_exposes_nav_summary_and_path_tables() {
        let mut rt = harness_with(fig9_state());
        rt.frame(0.0);
        for key in [
            "timing-pane",
            "timing-nav",
            "timing-nav-scroll",
            "timing-scroll",
            "timing-nav-general",
            "timing-nav-settings",
            "timing-nav-summary",
            "timing-nav-clocks",
            "timing-nav-check",
            "timing-nav-intra",
            "timing-nav-inter",
            "timing-nav-other",
            "timing-title",
            "timing-setup-wns",
            "timing-setup-tns",
            "timing-setup-failing",
            "timing-setup-endpoints",
            "timing-hold-whs",
            "timing-hold-ths",
            "timing-hold-failing",
            "timing-hold-endpoints",
            "timing-pulse-wpws",
            "timing-pulse-tpws",
            "timing-pulse-failing",
            "timing-pulse-endpoints",
            "timing-verdict",
            "timing-detail-path",
            "timing-src-clk",
            "timing-data",
            "timing-dst-clk",
            "timing-arrival",
            "timing-required",
            "timing-footer",
        ] {
            assert!(rt.find(key).is_some(), "missing timing key: {key}");
        }
    }

    #[test]
    fn fresh_project_prompts_to_run_implementation() {
        let mut s = init();
        s.center = crate::state::CenterPane::Timing;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("timing-run-impl").is_some(), "run prompt must show");
        for key in ["timing-setup-wns", "timing-verdict", "timing-data"] {
            assert!(rt.find(key).is_none(), "result {key} must wait for a run");
        }
    }

    #[test]
    fn run_prompt_starts_implementation_and_echoes_tcl() {
        let mut s = init();
        s.center = crate::state::CenterPane::Timing;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "timing-run-impl");
        assert_eq!(rt.state.flow.impl_run, RunStatus::Running);
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "launch_runs impl_1"),
            "prompt echoes its Tcl verb"
        );
    }

    #[test]
    fn slack_link_drills_into_intra_clock_paths() {
        let mut rt = harness_with(fig9_state());
        rt.frame(0.0);
        tap(&mut rt, "timing-hold-whs");
        assert_eq!(rt.state.timing_view.selected, TimingNav::IntraClock);
        assert!(rt.find("timing-paths").is_some(), "path list must show");
        assert!(
            rt.find("timing-paths-row-0").is_some(),
            "path rows must show"
        );
    }

    #[test]
    fn every_nav_entry_shows_report_content() {
        for nav in TimingNav::ALL {
            let mut s = fig9_state();
            s.timing_view.selected = nav;
            let mut rt = harness_with(s);
            rt.frame(0.0);
            let key = match nav {
                TimingNav::Summary => "timing-summary",
                TimingNav::ClockSummary => "timing-clocks",
                TimingNav::ClockInteraction => "timing-ci",
                TimingNav::GeneralInfo | TimingNav::TimerSettings | TimingNav::CheckTiming => {
                    "timing-info-card"
                }
                TimingNav::IntraClock | TimingNav::InterClock | TimingNav::OtherGroups => {
                    "timing-paths"
                }
            };
            assert!(rt.find(key).is_some(), "{} must show {key}", nav.label());
            assert!(rt.find("timing-footer").is_some(), "footer stays visible");
        }
    }

    #[test]
    fn verdict_tracks_constraint_state() {
        let mut rt = harness_with(fig9_state());
        rt.frame(0.0);
        assert!(!constraints_met(&rt.state.timing));
        let mut met = fig9_state();
        met.timing.whs_ps = Some(96);
        met.timing.ths_ps = 0;
        met.timing.failing_endpoints = 0;
        met.timing.failing_hold = 0;
        assert!(constraints_met(&met.timing));
        let mut rt_met = harness_with(met);
        rt_met.frame(0.0);
        assert!(rt_met.find("timing-verdict").is_some());
    }

    /// Wheel the report until `key` is presented inside the scroll
    /// viewport (rows below the fold are clipped and take no taps).
    fn scroll_into_view(rt: &mut Runtime<AppState>, key: &str) {
        for i in 0..40 {
            let vp = rt.presented("timing-scroll").unwrap().0;
            let r = rt
                .presented(key)
                .unwrap_or_else(|| panic!("missing {key}"))
                .0;
            if r[1] >= vp[1] && r[1] + r[3] <= vp[1] + vp[3] {
                return;
            }
            rt.wheel(vp[0] + vp[2] / 2.0, vp[1] + vp[3] / 2.0, -120.0);
            rt.frame(0.1 + i as f64 * (1.0 / 60.0));
        }
        panic!("{key} never scrolled into view");
    }

    #[test]
    fn delay_link_selects_the_cell_through_tcl() {
        let mut s = fig9_state();
        crate::live::seed_live_fixture(&mut s).expect("live fixture");
        // Point the launch register at a real fixture cell so the Tcl
        // `select_objects` verb resolves it.
        s.timing.paths[0].pins[0].cell = "u_div_u_ff0".into();
        let mut rt = harness_with(s);
        rt.frame(0.0);
        scroll_into_view(&mut rt, "timing-src-clk-delay-4");
        tap(&mut rt, "timing-src-clk-delay-4");
        assert_eq!(rt.state.flow.impl_run, RunStatus::NotStarted);
        assert!(
            rt.state
                .tcl
                .selected
                .iter()
                .any(|o| o.name == "u_div_u_ff0"),
            "the link must really select the cell: {:?}",
            rt.state.tcl.selected
        );
    }

    #[test]
    fn element_chips_follow_the_selected_path() {
        let mut rt = harness_with(fig9_state());
        rt.frame(0.0);
        for key in [
            "timing-detail-split",
            "timing-elements",
            "timing-element-0",
            "timing-hist",
            "timing-hist-count",
        ] {
            assert!(rt.find(key).is_some(), "missing {key}");
        }
        let chips = path_elements(&rt.state.timing.paths[0]);
        assert_eq!(chips.first().map(|e| e.id.as_str()), Some("clk"));
        assert!(chips.iter().any(|e| e.is_cell && e.id == "cnt_7"));
    }

    #[test]
    fn paths_table_filters_sorts_and_selects() {
        use crate::state::{TimingPathGroup, TimingPathRow};
        let mut s = fig9_state();
        let base = s.timing.paths[0].clone();
        s.timing.paths.push(TimingPathRow {
            name: "a_0->z_9".into(),
            from: "a_0".into(),
            to: "z_9".into(),
            slack_ps: -250,
            group: TimingPathGroup::Register,
            ..base
        });
        s.timing_view.selected = TimingNav::IntraClock;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        for key in [
            "timing-paths-filter",
            "timing-paths-sort-from",
            "timing-paths-sort-to",
            "timing-paths-sort-slack",
            "timing-paths-sort-levels",
            "timing-paths-row-1",
        ] {
            assert!(rt.find(key).is_some(), "missing {key}");
        }
        tap(&mut rt, "timing-paths-sort-slack");
        assert_eq!(
            rt.state.timing_view.path_sort,
            Some((PathSort::Slack, true))
        );
        tap(&mut rt, "timing-paths-row-0");
        assert_eq!(
            rt.state.timing_view.selected_path,
            Some(1),
            "row 0 after an ascending slack sort is the -0.250 ns path"
        );
        tap(&mut rt, "timing-paths-sort-slack");
        assert_eq!(
            rt.state.timing_view.path_sort,
            Some((PathSort::Slack, false))
        );
        rt.state.timing_view.path_filter = rune::TextEdit::new("z_9");
        rt.invalidate();
        rt.frame(0.2);
        assert!(rt.find("timing-paths-row-0").is_some());
        assert!(
            rt.find("timing-paths-row-1").is_none(),
            "filter keeps one path"
        );
        rt.state.timing_view.path_filter = rune::TextEdit::new("nothing-matches");
        rt.invalidate();
        rt.frame(0.3);
        tap(&mut rt, "timing-paths-clear-filter");
        assert!(rt.state.timing_view.path_filter.text.is_empty());
    }

    #[test]
    fn clock_interaction_matrix_selects_a_cell() {
        use crate::state::{ClockInteractionRow, ClockRel};
        let mut s = fig9_state();
        let cell = |from: &str, to: &str, relation: ClockRel| ClockInteractionRow {
            from: from.into(),
            to: to.into(),
            relation,
            common_period_ps: 10_000,
            requirement_ps: 10_000,
            wns_ps: (relation == ClockRel::Timed).then_some(2061),
            path_count: 4,
        };
        s.timing.clock_interaction = vec![
            cell("clk", "clk", ClockRel::Timed),
            cell("clk", "clk2", ClockRel::TimedUnsafe),
            cell("clk2", "clk", ClockRel::Asynchronous),
            cell("clk2", "clk2", ClockRel::Timed),
        ];
        s.timing_view.selected = TimingNav::ClockInteraction;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        for key in [
            "timing-ci",
            "timing-ci-matrix",
            "timing-ci-cell-0-0",
            "timing-ci-cell-1-1",
            "timing-ci-legend",
            "timing-ci-hint",
        ] {
            assert!(rt.find(key).is_some(), "missing {key}");
        }
        tap(&mut rt, "timing-ci-cell-0-1");
        assert_eq!(rt.state.timing_view.selected_interaction, Some(1));
        assert!(rt.find("timing-ci-detail").is_some(), "detail must show");
    }

    #[test]
    fn groups_cdc_and_networks_show_the_new_columns() {
        use crate::state::CdcRow;
        let mut s = fig9_state();
        s.timing.cdc = vec![CdcRow {
            from: "clk".into(),
            to: "clk2".into(),
            severity: "Critical".into(),
            check: "CDC-1".into(),
            endpoints: 3,
            sync: false,
            wns_ps: Some(-40),
            relation: "Timed (unsafe)".into(),
        }];
        s.timing_view.selected = TimingNav::InterClock;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        for key in [
            "timing-cdc-chips",
            "timing-cdc-critical",
            "timing-cdc-safe",
            "timing-cdc-row-0",
        ] {
            assert!(rt.find(key).is_some(), "missing {key}");
        }
        rt.state.timing_view.selected = TimingNav::IntraClock;
        rt.invalidate();
        rt.frame(0.1);
        assert!(
            rt.find("timing-groups-row-0").is_some(),
            "group rows with WHS/THS"
        );
        rt.state.timing_view.selected = TimingNav::ClockSummary;
        rt.invalidate();
        rt.frame(0.2);
        for key in [
            "timing-clknet",
            "timing-clknet-totals",
            "timing-clknet-row-0",
        ] {
            assert!(rt.find(key).is_some(), "missing {key}");
        }
    }

    #[test]
    fn report_timing_summary_action_is_disabled_without_a_run_context() {
        let mut rt = harness_with(fig9_state());
        rt.frame(0.0);
        assert!(rt.find("timing-rerun").is_some(), "action must render");
        let before = rt.state.console.lines.len();
        tap(&mut rt, "timing-rerun");
        assert_eq!(
            rt.state.console.lines.len(),
            before,
            "a disabled action must not run anything"
        );
    }

    #[test]
    fn ns_text_matches_vivado() {
        assert_eq!(format_ns(2061), "2.061 ns");
        assert_eq!(format_ns(-3069), "-3.069 ns");
    }

    #[test]
    fn harness_smoke() {
        let mut rt = harness();
        rt.frame(0.0);
        assert!(rt.find("shell").is_some());
    }

    #[test]
    fn paths_page_renders_real_state_paths_not_demo_rows() {
        // deep-02: the Intra-Clock page must list paths from TimingState
        // (filled by real runs), never hardcoded demo entries.
        use crate::state::{TimingPathGroup, TimingPathRow, TimingPinRow};
        let mut s = fig9_state();
        s.timing.paths = vec![
            TimingPathRow {
                name: "cnt_3->cnt_7".into(),
                from: "cnt_3".into(),
                to: "cnt_7".into(),
                slack_ps: 2061,
                delay_ps: 7939,
                group: TimingPathGroup::Register,
                launch_clock: "clk".into(),
                capture_clock: "clk".into(),
                pins: vec![TimingPinRow {
                    cell: "cnt_3".into(),
                    pin: "cnt_3/Q".into(),
                    delay_type: "cell".into(),
                    incr_ps: 80,
                    path_ps: 80,
                    net: "cnt_3_q".into(),
                    fanout: 2,
                    location: "CLB_X1Y2".into(),
                }],
            },
            TimingPathRow {
                name: "cnt_7->led_o".into(),
                from: "cnt_7".into(),
                to: "led_o".into(),
                slack_ps: 3309,
                delay_ps: 6691,
                group: TimingPathGroup::Output,
                launch_clock: "clk".into(),
                capture_clock: String::new(),
                pins: vec![],
            },
        ];
        s.timing_view.selected = TimingNav::IntraClock;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("timing-paths").is_some(), "path list must show");
        assert!(
            rt.find("timing-paths-row-0").is_some(),
            "real path 0 must show"
        );
        assert!(
            rt.find("timing-paths-row-1").is_none(),
            "output paths belong to Other Groups"
        );
        assert!(
            rt.find("timing-detail-path").is_some(),
            "detail header must show"
        );
    }

    #[test]
    fn wheel_over_paths_report_scrolls_tables_into_view() {
        let mut s = fig9_state();
        s.timing_view.selected = TimingNav::IntraClock;
        // Pad the path list so the report overflows the viewport.
        let mut paths = s.timing.paths.clone();
        for i in 0..30 {
            let mut p = paths[0].clone();
            p.name = format!("pad_{i}->pad_{}", i + 1);
            paths.push(p);
        }
        s.timing.paths = paths;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        let vp = rt.presented("timing-scroll").unwrap().0;
        let y_before = rt.presented("timing-detail-path").unwrap().0[1];
        assert!(
            y_before > vp[1] + vp[3],
            "detail must start below the fold (y={y_before}, fold={})",
            vp[1] + vp[3]
        );
        let (wx, wy) = (vp[0] + vp[2] / 2.0, vp[1] + vp[3] / 2.0);
        for i in 0..10 {
            rt.wheel(wx, wy, -140.0);
            rt.frame(0.1 + i as f64 * (1.0 / 60.0));
        }
        let y_after = rt.presented("timing-detail-path").unwrap().0[1];
        assert!(
            y_after < y_before - 100.0,
            "wheel must move content up (before={y_before}, after={y_after})"
        );
    }

    #[test]
    fn empty_inter_paths_show_a_disabled_reason() {
        // deep-02: with no inter-clock groups the page must say why instead
        // of showing demo crossings.
        let mut s = fig9_state();
        s.timing.paths = Vec::new();
        s.timing.groups = Vec::new();
        s.timing_view.selected = TimingNav::InterClock;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("timing-paths-empty").is_some(),
            "disabled reason must show"
        );
    }

    #[test]
    fn clock_page_renders_real_state_clocks() {
        // deep-02: Clock Summary must list TimingState.clocks from the run's
        // STA constraints, never a hardcoded pair.
        use crate::state::TimingClock;
        let mut s = fig9_state();
        s.timing.clocks = vec![TimingClock {
            name: "clk".into(),
            period_ps: 10_000,
            source: "clk".into(),
            generated: false,
            master: None,
        }];
        s.timing_view.selected = TimingNav::ClockSummary;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("timing-clocks").is_some(), "clock table must show");
        assert!(
            rt.find("timing-clock-row-0").is_some(),
            "real clock 0 must show"
        );
        assert!(
            rt.find("timing-clock-row-1").is_none(),
            "no second demo clock may appear"
        );
    }

    #[test]
    fn no_run_prompt_launches_a_real_run() {
        let mut s = init();
        s.center = crate::state::CenterPane::Timing;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "timing-run-impl");
        assert!(rt.state.runs.is_active(), "prompt must arm a real worker");
        crate::runs::cancel_run(&mut rt.state);
        let start = std::time::Instant::now();
        while rt.state.runs.is_active() {
            crate::runs::poll_runs(&mut rt.state);
            if !rt.state.runs.is_active() {
                break;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(60),
                "run timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn running_run_replaces_prompt_with_status() {
        let mut s = init();
        s.center = crate::state::CenterPane::Timing;
        s.flow.impl_run = RunStatus::Running;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("timing-run-status").is_some(),
            "running notice must show"
        );
        assert!(
            rt.find("timing-run-impl").is_none(),
            "prompt link must hide while running"
        );
    }
}
