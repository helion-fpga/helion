//! Timing paths: the sortable, filterable 8-column path table and the
//! selected path's detail (delay split header, path-element chips, and the
//! grouped UG903 pin tables with Net/Fanout columns).
//!
//! Mirrors the old IDE's `paint_timing_paths` / `paint_timing_path_table` /
//! `paint_timing_pin_detail` (`helion-ide.rs:6407-6830`) in Rune `El` trees:
//! `# | From | To | Launch clock | Capture clock | Slack | Levels | Path
//! group` with From/To/Slack/Levels sorting and a From/To/group filter; row
//! taps select the path and drive the detail below. Cell links and element
//! chips run the real `select_objects` verb through the Tcl session.

use rune::{Align, El, col, el, row, text, text_input};

use crate::state::{AppState, TimingPathGroup, TimingPathRow};
use crate::theme::VivadoTheme;
use crate::views::report_kit::{
    self as kit, PAD_X, ROW_H, header_row, ns3, num_cell, select_row, table,
};

/// Fixed column widths of the path table.
const IDX_W: f32 = 28.0;
const CLK_W: f32 = 92.0;
const SLACK_W: f32 = 72.0;
const LEVELS_W: f32 = 52.0;
const GROUP_W: f32 = 72.0;

/// Pin-table column widths.
const INCR_W: f32 = 64.0;
const PATH_W: f32 = 64.0;
const LOC_W: f32 = 112.0;
const FANOUT_W: f32 = 48.0;

/// Sortable path-table columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathSort {
    From,
    To,
    Slack,
    Levels,
}

impl PathSort {
    fn key(self) -> &'static str {
        match self {
            PathSort::From => "from",
            PathSort::To => "to",
            PathSort::Slack => "slack",
            PathSort::Levels => "levels",
        }
    }
}

/// Toggle a sort column: first press ascends, a second press descends.
pub fn toggle_sort(s: &mut AppState, col: PathSort) {
    s.timing_view.path_sort = Some(match s.timing_view.path_sort {
        Some((cur, asc)) if cur == col => (col, !asc),
        _ => (col, true),
    });
}

/// `(state_index, path)` pairs of one family after the filter and sort.
pub fn visible_paths(s: &AppState, family: TimingPathGroup) -> Vec<(usize, &TimingPathRow)> {
    let filt = s.timing_view.path_filter.text.trim().to_lowercase();
    let mut out: Vec<(usize, &TimingPathRow)> = s
        .timing
        .paths
        .iter()
        .enumerate()
        .filter(|(_, p)| p.group == family)
        .filter(|(_, p)| {
            filt.is_empty()
                || p.from.to_lowercase().contains(&filt)
                || p.to.to_lowercase().contains(&filt)
                || p.group.short_label().to_lowercase().contains(&filt)
        })
        .collect();
    if let Some((col, asc)) = s.timing_view.path_sort {
        out.sort_by(|(_, a), (_, b)| {
            let ord = match col {
                PathSort::From => a.from.cmp(&b.from),
                PathSort::To => a.to.cmp(&b.to),
                PathSort::Slack => a.slack_ps.cmp(&b.slack_ps),
                PathSort::Levels => a.levels().cmp(&b.levels()),
            };
            if asc { ord } else { ord.reverse() }
        });
    }
    out
}

/// Clickable sort header: label plus a ▲/▼ marker on the active column.
fn sort_header(
    s: &AppState,
    col: PathSort,
    label: &'static str,
    numeric: bool,
    t: &VivadoTheme,
) -> El {
    let state = match s.timing_view.path_sort {
        Some((cur, asc)) if cur == col => Some(asc),
        _ => None,
    };
    let caption = match state {
        Some(true) => format!("{label} ▲"),
        Some(false) => format!("{label} ▼"),
        None => label.to_string(),
    };
    let mut cell = text(caption)
        .font(VivadoTheme::FONT_UI)
        .color(if state.is_some() { t.text } else { t.text_dim })
        .truncate();
    if numeric {
        cell = cell.w_full().align(Align::End);
    }
    el().key(format!("timing-paths-sort-{}", col.key()))
        .role("button")
        .label(format!("Sort by {label}"))
        .sorted(state)
        .focusable(true)
        .h(kit::HEADER_H)
        .center_y()
        .hover(t.selection)
        .on_press(move |s: &mut AppState| toggle_sort(s, col))
        .child(cell)
}

/// Filter field bound to [`crate::views::timing::TimingView::path_filter`].
fn filter_field(s: &AppState, t: &VivadoTheme) -> El {
    el().w(180.0)
        .h(ROW_H)
        .px(6.0)
        .center_y()
        .radius(3.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            text_input(&s.timing_view.path_filter, VivadoTheme::FONT_UI)
                .single_line()
                .key("timing-paths-filter")
                .label("Filter paths by From, To or group")
                .w_full()
                .color(t.text)
                .placeholder("Filter paths…", t.text_faint)
                .caret(t.text, t.selection)
                .on_edit(|s: &mut AppState| &mut s.timing_view.path_filter),
        )
}

/// The 8-column path table for one family, with its title bar, filter,
/// and selection. `None` when the family has no paths at all.
pub fn paths_table(
    s: &AppState,
    family: TimingPathGroup,
    title: &'static str,
    t: &VivadoTheme,
) -> El {
    let total = s.timing.paths.iter().filter(|p| p.group == family).count();
    let rows = visible_paths(s, family);
    let head = header_row(t)
        .child(kit::header_num("#", IDX_W, t))
        .child(
            sort_header(s, PathSort::From, "From", false, t)
                .grow(2.0)
                .min_w(0.0),
        )
        .child(
            sort_header(s, PathSort::To, "To", false, t)
                .grow(2.0)
                .min_w(0.0),
        )
        .child(el().w(CLK_W).child(kit::header_cell("Launch clock", t)))
        .child(el().w(CLK_W).child(kit::header_cell("Capture clock", t)))
        .child(
            el().w(SLACK_W)
                .child(sort_header(s, PathSort::Slack, "Slack (ns)", true, t)),
        )
        .child(
            el().w(LEVELS_W)
                .child(sort_header(s, PathSort::Levels, "Levels", true, t)),
        )
        .child(el().w(GROUP_W).child(kit::header_cell("Path group", t)));
    let mut body = col().w_full();
    if rows.is_empty() {
        body = body.child(kit::empty_state(
            "timing-paths-nomatch",
            "No paths match the filter.",
            Some(kit::link(
                "timing-paths-clear-filter",
                "Clear filter",
                t,
                |s: &mut AppState| s.timing_view.path_filter = rune::TextEdit::new(""),
            )),
            t,
        ));
    }
    let selected = super::detail_path(s).map(|(i, _)| i);
    for (n, (idx, p)) in rows.iter().enumerate() {
        let idx = *idx;
        let dash = |c: &str| {
            if c.is_empty() {
                "–".to_string()
            } else {
                c.to_string()
            }
        };
        body = body.child(
            select_row(
                format!("timing-paths-row-{n}"),
                format!("Path {} to {}, slack {} ns", p.from, p.to, ns3(p.slack_ps)),
                selected == Some(idx),
                t,
                move |s: &mut AppState| s.timing_view.selected_path = Some(idx),
            )
            .child(num_cell((n + 1).to_string(), IDX_W, t.text_faint))
            .child(kit::text_cell(p.from.clone(), 2.0, t.text))
            .child(kit::text_cell(p.to.clone(), 2.0, t.text))
            .child(kit::fixed_cell(dash(&p.launch_clock), CLK_W, t.text_dim))
            .child(kit::fixed_cell(dash(&p.capture_clock), CLK_W, t.text_dim))
            .child(num_cell(
                ns3(p.slack_ps),
                SLACK_W,
                kit::slack_color(Some(p.slack_ps), t),
            ))
            .child(num_cell(p.levels().to_string(), LEVELS_W, t.text))
            .child(kit::fixed_cell(p.group.short_label(), GROUP_W, t.text_dim)),
        );
    }
    let noun = if total == 1 { "path" } else { "paths" };
    let count = if rows.len() == total {
        format!("{total} {noun}")
    } else {
        format!("{} of {total} {noun}", rows.len())
    };
    col()
        .key("timing-paths")
        .w_full()
        .gap(4.0)
        .child(
            row()
                .w_full()
                .h(24.0)
                .items_center()
                .gap(8.0)
                .child(kit::section_title(title, t))
                .child(
                    text(count)
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text_dim)
                        .key("timing-paths-count"),
                )
                .child(el().grow(1.0))
                .child(filter_field(s, t)),
        )
        .child(table(head, body, t))
}

/// One element of the selected path in dataflow order, merged across a
/// cell's pins (mirroring `ide.rs::probe_elements`): a `cell` arc's row
/// carries the net it drives, so the cell precedes that net; every other
/// row carries the net feeding its pin, so the net precedes the cell.
#[derive(Clone, Debug, PartialEq)]
pub struct PathElement {
    pub id: String,
    pub is_cell: bool,
    pub arrival_ps: i64,
}

pub fn path_elements(path: &TimingPathRow) -> Vec<PathElement> {
    let mut out: Vec<PathElement> = Vec::new();
    let mut push = |id: &str, is_cell: bool, arrival_ps: i64| {
        if id.is_empty() {
            return;
        }
        if let Some(prev) = out.last_mut() {
            if prev.id == id && prev.is_cell == is_cell {
                prev.arrival_ps = prev.arrival_ps.max(arrival_ps);
                return;
            }
        }
        out.push(PathElement {
            id: id.to_string(),
            is_cell,
            arrival_ps,
        });
    };
    for pin in &path.pins {
        if pin.delay_type == "cell" {
            push(&pin.cell, true, pin.path_ps);
            push(&pin.net, false, pin.path_ps);
        } else {
            push(
                &pin.net,
                false,
                pin.path_ps.saturating_sub(pin.incr_ps.max(0)),
            );
            push(&pin.cell, true, pin.path_ps);
        }
    }
    out
}

/// Select one netlist object through the real Tcl `select_objects` verb
/// (the console shows the result or the honest "no such object" error).
pub fn select_object(s: &mut AppState, id: &str) {
    crate::tcl::eval_command(s, &format!("select_objects {{{id}}}"));
}

/// Wrapping strip of path-element chips (`cell cnt_3 · 0.080`), each
/// selecting its object.
fn element_strip(path: &TimingPathRow, t: &VivadoTheme) -> El {
    let elements = path_elements(path);
    let mut strip = row()
        .key("timing-elements")
        .w_full()
        .flex_wrap()
        .gaps(4.0, 4.0)
        .items_center();
    for (i, e) in elements.iter().enumerate() {
        if i > 0 {
            strip = strip.child(
                text("→")
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_faint)
                    .a11y_hidden(),
            );
        }
        let id = e.id.clone();
        let kind = if e.is_cell { "cell" } else { "net" };
        strip = strip.child(
            row()
                .key(format!("timing-element-{i}"))
                .role("button")
                .label(format!("Select {kind} {}", e.id))
                .help(format!("select_objects {}", e.id))
                .focusable(true)
                .h(ROW_H)
                .px(6.0)
                .gap(4.0)
                .items_center()
                .radius(3.0)
                .border(1.0, t.border_light)
                .hover(t.selection)
                .on_press(move |s: &mut AppState| select_object(s, &id))
                .child(text(kind).font(VivadoTheme::FONT_CAPS).color(if e.is_cell {
                    t.text_dim
                } else {
                    t.link
                }))
                .child(
                    text(e.id.clone())
                        .font(VivadoTheme::FONT_MONO.mono())
                        .color(t.text),
                )
                .child(
                    text(ns3(e.arrival_ps))
                        .font(VivadoTheme::FONT_MONO.mono())
                        .color(t.text_dim),
                ),
        );
    }
    strip
}

/// One grouped pin-table row: the UG903 delay type plus its cumulative
/// nanoseconds, location, net/fanout, and netlist resource.
pub struct PinRow {
    pub delay: String,
    pub incr_ns: Option<f64>,
    pub path_ns: Option<f64>,
    /// Bare site (`CLB_X1Y2`); rendered as `Site: CLB_X1Y2`.
    pub loc: String,
    pub net: String,
    pub fanout: Option<usize>,
    pub resource: String,
    /// Cell whose delay link selects it (`None` = plain delay text).
    pub link_cell: Option<String>,
}

impl PinRow {
    pub fn plain(delay: impl Into<String>, resource: impl Into<String>) -> Self {
        PinRow {
            delay: delay.into(),
            incr_ns: None,
            path_ns: None,
            loc: String::new(),
            net: String::new(),
            fanout: None,
            resource: resource.into(),
            link_cell: None,
        }
    }
}

/// Data-path pin rows (everything after the launch clock pin).
pub fn data_rows(path: &TimingPathRow) -> Vec<PinRow> {
    path.pins
        .iter()
        .filter(|p| p.delay_type != "clk")
        .map(|pin| PinRow {
            delay: pin.delay_type.clone(),
            incr_ns: Some(pin.incr_ps as f64 / 1000.0),
            path_ns: Some(pin.path_ps as f64 / 1000.0),
            loc: pin.location.clone(),
            net: pin.net.clone(),
            fanout: Some(pin.fanout),
            resource: pin.pin.clone(),
            link_cell: (pin.delay_type == "cell").then(|| pin.cell.clone()),
        })
        .collect()
}

fn opt_ns(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.3}")).unwrap_or_default()
}

fn pin_row(key_prefix: &str, index: usize, r: &PinRow, t: &VivadoTheme) -> El {
    let delay = match &r.link_cell {
        Some(cell) => {
            let cell = cell.clone();
            el().key(format!("{key_prefix}-delay-{index}"))
                .role("button")
                .label(format!("Select {cell}"))
                .help(format!("select_objects {cell}"))
                .focusable(true)
                .h(ROW_H)
                .center_y()
                .hover(t.selection)
                .on_press(move |s: &mut AppState| select_object(s, &cell))
                .child(
                    text(r.delay.clone())
                        .font(VivadoTheme::FONT_UI)
                        .color(t.link)
                        .truncate(),
                )
        }
        None => el().h(ROW_H).center_y().child(
            text(r.delay.clone())
                .font(VivadoTheme::FONT_UI)
                .color(t.text)
                .truncate()
                .help_when_truncated(r.delay.clone()),
        ),
    };
    let loc = if r.loc.is_empty() {
        String::new()
    } else {
        format!("Site: {}", r.loc)
    };
    kit::plain_row(format!("{key_prefix}-row-{index}"))
        .child(delay.grow(3.0).min_w(0.0))
        .child(num_cell(opt_ns(r.incr_ns), INCR_W, t.text))
        .child(num_cell(opt_ns(r.path_ns), PATH_W, t.text))
        .child(kit::fixed_cell(loc, LOC_W, t.text_dim))
        .child(kit::text_cell(r.net.clone(), 2.0, t.text))
        .child(num_cell(
            r.fanout.map(|f| f.to_string()).unwrap_or_default(),
            FANOUT_W,
            t.text,
        ))
        .child(kit::text_cell(r.resource.clone(), 3.0, t.text))
}

/// One grouped table: bold title, column band, rows, optional bold footer.
pub fn pin_table(
    key_prefix: &'static str,
    title: &'static str,
    rows: &[PinRow],
    footer: Option<(&'static str, &'static str, f64)>,
    t: &VivadoTheme,
) -> El {
    let head = header_row(t)
        .child(kit::header_cell("Delay Type", t).grow(3.0).min_w(0.0))
        .child(kit::header_num("Incr (ns)", INCR_W, t))
        .child(kit::header_num("Path (ns)", PATH_W, t))
        .child(el().w(LOC_W).child(kit::header_cell("Location", t)))
        .child(kit::header_cell("Net", t).grow(2.0).min_w(0.0))
        .child(kit::header_num("Fanout", FANOUT_W, t))
        .child(
            kit::header_cell("Netlist Resource(s)", t)
                .grow(3.0)
                .min_w(0.0),
        );
    let mut body = col().w_full();
    for (i, r) in rows.iter().enumerate() {
        body = body.child(pin_row(key_prefix, i, r, t));
    }
    if let Some((label, key, value)) = footer {
        body = body.child(
            row()
                .w_full()
                .h(ROW_H)
                .items_center()
                .gap(kit::COL_GAP)
                .px(PAD_X)
                .border_sides([1.0, 0.0, 0.0, 0.0], t.border_light)
                .child(
                    text(label)
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text)
                        .grow(3.0)
                        .min_w(0.0),
                )
                .child(el().w(INCR_W))
                .child(
                    el().w(PATH_W).child(
                        text(format!("{value:.3}"))
                            .font(VivadoTheme::FONT_MONO.mono())
                            .color(t.text)
                            .w_full()
                            .align(Align::End)
                            .key(key),
                    ),
                )
                .child(el().w(LOC_W))
                .child(el().grow(2.0).min_w(0.0))
                .child(el().w(FANOUT_W))
                .child(el().grow(3.0).min_w(0.0)),
        );
    }
    col()
        .key(key_prefix)
        .w_full()
        .gap(2.0)
        .child(kit::section_title(title, t))
        .child(table(head, body, t))
}

/// Header block of the path detail: title, delay split, and the element
/// strip.
pub fn detail_header(path: &TimingPathRow, t: &VivadoTheme) -> El {
    let dash = |c: &str| {
        if c.is_empty() {
            "–".to_string()
        } else {
            c.to_string()
        }
    };
    let split = format!(
        "Slack {} ns · Delay {} ns (logic {} ns, net {} ns) · Levels {} · Launch {} · Capture {}",
        ns3(path.slack_ps),
        ns3(path.delay_ps),
        ns3(path.logic_ps()),
        ns3(path.net_ps()),
        path.levels(),
        dash(&path.launch_clock),
        dash(&path.capture_clock),
    );
    let mut block = col()
        .w_full()
        .gap(4.0)
        .child(
            text(format!("Path: {} → {}", path.from, path.to))
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text)
                .truncate()
                .help_when_truncated(format!("{} → {}", path.from, path.to))
                .key("timing-detail-path"),
        )
        .child(
            text(split.clone())
                .font(VivadoTheme::FONT_UI)
                .color(if path.slack_ps < 0 {
                    t.fail
                } else {
                    t.text_dim
                })
                .truncate()
                .help_when_truncated(split)
                .key("timing-detail-split"),
        );
    if !path.pins.is_empty() {
        block = block
            .child(
                text("Path elements")
                    .font(VivadoTheme::FONT_CAPS)
                    .color(t.text_dim),
            )
            .child(element_strip(path, t));
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TimingPinRow;

    fn pin(cell: &str, ty: &str, incr: i64, path: i64, net: &str) -> TimingPinRow {
        TimingPinRow {
            cell: cell.into(),
            pin: format!("{cell}/P"),
            delay_type: ty.into(),
            incr_ps: incr,
            path_ps: path,
            net: net.into(),
            fanout: 1,
            location: String::new(),
        }
    }

    #[test]
    fn elements_follow_dataflow_and_merge_cell_pins() {
        let p = TimingPathRow {
            pins: vec![
                pin("a", "clk", 0, 0, "clk"),
                pin("a", "cell", 80, 80, "q"),
                pin("lut", "net", 300, 380, "q"),
                pin("lut", "cell", 120, 500, "d"),
                pin("b", "setup", 50, 550, "d"),
            ],
            ..TimingPathRow::default()
        };
        let ids: Vec<(String, bool)> = path_elements(&p)
            .into_iter()
            .map(|e| (e.id, e.is_cell))
            .collect();
        assert_eq!(
            ids,
            vec![
                ("clk".into(), false),
                ("a".into(), true),
                ("q".into(), false),
                ("lut".into(), true),
                ("d".into(), false),
                ("b".into(), true),
            ]
        );
        assert_eq!(p.logic_ps(), 80 + 120 + 50);
        assert_eq!(p.net_ps(), 300);
        assert_eq!(p.levels(), 1, "the LUT is the only logic level");
    }
}
