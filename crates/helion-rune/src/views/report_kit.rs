//! Shared building blocks for the analysis report surfaces (Timing,
//! Reports, Constraints): compact action buttons, table frames, header and
//! numeric cells, count chips, detail panels, and one-sentence empty states.
//!
//! Every helper takes [`VivadoTheme`] tokens only (no hex), uses the shared
//! CAD density (20pt rows, 22pt header band, 8pt horizontal padding), and
//! never wires a handler that only echoes Tcl: callers pass the real work.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el.rs`: `El::help`/`help_when_truncated` tooltips.
//! - `crates/rune/src/el/a11y.rs`: `disabled`, `description`, `sorted`.
//! - `crates/rune/src/el/text.rs`: `truncate`, `wrap`.

use rune::{Align, Color, El, col, el, row, text};

use crate::state::AppState;
use crate::theme::VivadoTheme;

/// Table header band height.
pub const HEADER_H: f32 = 22.0;
/// Table row height (compact CAD density).
pub const ROW_H: f32 = 20.0;
/// Horizontal cell padding shared by headers and rows.
pub const PAD_X: f32 = 8.0;
/// Gap between table columns (keeps a right-aligned number clear of the
/// left-aligned text column after it).
pub const COL_GAP: f32 = 12.0;

/// Section title above a table or card.
pub fn section_title(content: impl Into<String>, t: &VivadoTheme) -> El {
    text(content.into())
        .font(VivadoTheme::FONT_TITLE)
        .color(t.text)
}

/// Dim header-band text (left aligned, truncating).
pub fn header_cell(content: &'static str, t: &VivadoTheme) -> El {
    text(content)
        .font(VivadoTheme::FONT_UI)
        .color(t.text_dim)
        .truncate()
}

/// Right-aligned header text inside a fixed-width column.
pub fn header_num(content: &'static str, w: f32, t: &VivadoTheme) -> El {
    el().w(w).child(
        text(content)
            .font(VivadoTheme::FONT_UI)
            .color(t.text_dim)
            .w_full()
            .align(Align::End)
            .truncate(),
    )
}

/// The header band row holding header cells.
pub fn header_row(t: &VivadoTheme) -> El {
    row()
        .w_full()
        .h(HEADER_H)
        .items_center()
        .gap(COL_GAP)
        .px(PAD_X)
        .fill(t.header_bg)
}

/// Bordered table frame: header band over the body rows.
pub fn table(header: El, body: El, t: &VivadoTheme) -> El {
    col()
        .w_full()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(header)
        .child(body)
}

/// One table text cell that grows by `grow`, truncating with a hover
/// tooltip carrying the full text.
pub fn text_cell(content: impl Into<String>, grow: f32, color: Color) -> El {
    let content = content.into();
    text(content.clone())
        .font(VivadoTheme::FONT_UI)
        .color(color)
        .grow(grow)
        .min_w(0.0)
        .truncate()
        .help_when_truncated(content)
}

/// Fixed-width text cell (left aligned, truncating with tooltip).
pub fn fixed_cell(content: impl Into<String>, w: f32, color: Color) -> El {
    let content = content.into();
    el().w(w).child(
        text(content.clone())
            .font(VivadoTheme::FONT_UI)
            .color(color)
            .truncate()
            .help_when_truncated(content),
    )
}

/// Fixed-width right-aligned monospace value cell.
pub fn num_cell(content: impl Into<String>, w: f32, color: Color) -> El {
    let content = content.into();
    el().w(w).child(
        text(content.clone())
            .font(VivadoTheme::FONT_MONO.mono())
            .color(color)
            .w_full()
            .align(Align::End)
            .truncate()
            .help_when_truncated(content),
    )
}

/// A selectable table row: hover wash, `selection` fill when selected,
/// button role, and the caller's real selection handler.
pub fn select_row(
    key: impl Into<String>,
    label: impl Into<String>,
    selected: bool,
    t: &VivadoTheme,
    on: impl Fn(&mut AppState) + 'static,
) -> El {
    let mut r = row()
        .key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(COL_GAP)
        .px(PAD_X)
        .hover(t.selection)
        .on_press(on);
    if selected {
        r = r.fill(t.selection);
    }
    r
}

/// A plain (non-interactive) table row.
pub fn plain_row(key: impl Into<String>) -> El {
    row()
        .key(key)
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(COL_GAP)
        .px(PAD_X)
}

/// Compact bordered action button. Enabled buttons run `on` (real work
/// only); disabled ones render faint, take no input, and explain why via
/// their accessible description and hover tooltip.
pub fn action_button(
    key: impl Into<String>,
    label: &'static str,
    disabled_reason: Option<&str>,
    t: &VivadoTheme,
    on: impl Fn(&mut AppState) + 'static,
) -> El {
    let b = el()
        .key(key)
        .role("button")
        .label(label)
        .h(ROW_H)
        .px(PAD_X)
        .center_y()
        .radius(3.0)
        .fill(t.panel_bg)
        .border(1.0, t.border);
    match disabled_reason {
        None => b
            .focusable(true)
            .hover(t.selection)
            .on_press(on)
            .child(text(label).font(VivadoTheme::FONT_UI).color(t.text)),
        Some(reason) => b
            .disabled(true)
            .description(reason)
            .help(reason)
            .child(text(label).font(VivadoTheme::FONT_UI).color(t.text_faint)),
    }
}

/// Blue text link running real work (one call-to-action per empty state).
pub fn link(
    key: impl Into<String>,
    label: &'static str,
    t: &VivadoTheme,
    on: impl Fn(&mut AppState) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .h(ROW_H)
        .px(2.0)
        .center_y()
        .hover(t.selection)
        .on_press(on)
        .child(text(label).font(VivadoTheme::FONT_UI).color(t.link))
}

/// Empty state: one short sentence plus at most one real call-to-action.
pub fn empty_state(key: impl Into<String>, sentence: &str, cta: Option<El>, t: &VivadoTheme) -> El {
    let mut r = row()
        .key(key)
        .w_full()
        .h(32.0)
        .items_center()
        .gap(6.0)
        .px(PAD_X)
        .child(
            text(sentence.to_string())
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .truncate()
                .help_when_truncated(sentence.to_string()),
        );
    if let Some(c) = cta {
        r = r.child(c);
    }
    r
}

/// Count chip: dim caption plus a bold colored count (`Critical 2`).
pub fn count_chip(
    key: impl Into<String>,
    caption: &'static str,
    count: usize,
    color: Color,
    t: &VivadoTheme,
) -> El {
    row()
        .key(key)
        .h(ROW_H)
        .items_center()
        .gap(4.0)
        .px(PAD_X)
        .radius(3.0)
        .border(1.0, t.border_light)
        .child(text(caption).font(VivadoTheme::FONT_CAPS).color(t.text_dim))
        .child(
            text(count.to_string())
                .font(VivadoTheme::FONT_TITLE)
                .color(if count == 0 { t.text_faint } else { color }),
        )
}

/// Detail panel: a titled bordered card of `label: value` rows whose values
/// wrap (long messages never overflow).
pub fn detail_panel(
    key: impl Into<String>,
    title: impl Into<String>,
    rows: Vec<(&'static str, El)>,
    t: &VivadoTheme,
) -> El {
    let mut body = col().w_full().padding(PAD_X).gap(4.0);
    for (label, value) in rows {
        body = body.child(
            row()
                .w_full()
                .items_start()
                .gap(8.0)
                .child(
                    el().w(96.0)
                        .child(text(label).font(VivadoTheme::FONT_UI).color(t.text_dim)),
                )
                .child(el().grow(1.0).min_w(0.0).child(value)),
        );
    }
    col()
        .key(key)
        .w_full()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            row()
                .w_full()
                .h(HEADER_H)
                .items_center()
                .px(PAD_X)
                .fill(t.header_bg)
                .child(
                    text(title.into())
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text)
                        .truncate(),
                ),
        )
        .child(body)
}

/// Wrapping detail value text (full message, objects, hints).
pub fn wrap_value(content: impl Into<String>, color: Color) -> El {
    text(content.into())
        .font(VivadoTheme::FONT_UI)
        .color(color)
        .w_full()
        .wrap_anywhere()
}

/// Wrapping monospace detail value (Tcl commands, source lines).
pub fn wrap_mono(content: impl Into<String>, color: Color) -> El {
    text(content.into())
        .font(VivadoTheme::FONT_MONO.mono())
        .color(color)
        .w_full()
        .wrap_anywhere()
}

/// Horizontal bar track with a `w_frac` fill (shares, occupancy).
pub fn bar(frac: f32, color: Color, t: &VivadoTheme) -> El {
    el().grow(1.0)
        .min_w(0.0)
        .h(8.0)
        .radius(2.0)
        .fill(t.border_light)
        .child(
            el().w_frac(frac.clamp(0.0, 1.0))
                .h_full()
                .radius(2.0)
                .fill(color),
        )
}

/// Picoseconds as `"2.061"` nanoseconds (no unit; column headers carry it).
pub fn ns3(ps: i64) -> String {
    format!("{:.3}", ps as f64 / 1000.0)
}

/// Optional slack as nanoseconds or an en dash.
pub fn ns_opt(ps: Option<i64>) -> String {
    ps.map(ns3).unwrap_or_else(|| "–".into())
}

/// Slack ink: red when negative, ink otherwise.
pub fn slack_color(ps: Option<i64>, t: &VivadoTheme) -> Color {
    if ps.is_some_and(|v| v < 0) {
        t.fail
    } else {
        t.text
    }
}
