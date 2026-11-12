//! Shared CAD widgets for the Hardware Manager, IP, Bitstream, and Find
//! surfaces: compact buttons that disable with a reason, table headers and
//! rows at the 20pt CAD rhythm, KPI cards, status badges, and empty states.
//!
//! Every color comes from [`VivadoTheme`]; nothing here holds state. Views
//! pass real handlers in: a disabled control never mounts a handler and
//! always carries a `.description()` + `.help()` reason.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el/a11y.rs`: `.disabled()/.description()`.
//! - `crates/rune/src/el.rs`: `.help()/.help_when_truncated()/.align()/.text_case()`.
//! - `crates/rune/src/el/visuals.rs`: `.hover()/.layer_opacity()/.radius()`.

use rune::tok::TextStyle;
use rune::{Align, Color, El, TextCase, col, el, row, text};

use crate::state::AppState;
use crate::theme::VivadoTheme;

/// Table row height (compact CAD density).
pub const ROW_H: f32 = 20.0;
/// Table header band height.
pub const HEAD_H: f32 = 22.0;
/// Toolbar strip height.
pub const TOOL_H: f32 = 28.0;
/// Toolbar button height.
pub const BTN_H: f32 = 22.0;
/// KPI value type: one step above body text.
pub const FONT_KPI: TextStyle = TextStyle::new(17.0, 600.0);
/// Canvas pin/label type: smaller than CAD body text.
pub const FONT_PIN: TextStyle = TextStyle::new(10.0, 400.0);

/// Monospace body style (numbers, hex, engine names).
pub fn mono_style() -> TextStyle {
    VivadoTheme::FONT_MONO.mono()
}

pub fn ui(content: impl Into<String>, t: &VivadoTheme) -> El {
    text(content.into())
        .font(VivadoTheme::FONT_UI)
        .color(t.text)
}

pub fn dim(content: impl Into<String>, t: &VivadoTheme) -> El {
    text(content.into())
        .font(VivadoTheme::FONT_UI)
        .color(t.text_dim)
}

pub fn mono(content: impl Into<String>, t: &VivadoTheme) -> El {
    text(content.into()).font(mono_style()).color(t.text)
}

/// Small-caps section/KPI label.
pub fn caps(content: impl Into<String>, t: &VivadoTheme) -> El {
    text(content.into())
        .font(VivadoTheme::FONT_CAPS)
        .color(t.text_dim)
        .text_case(TextCase::Uppercase)
}

/// Bold section title.
pub fn title(content: impl Into<String>, t: &VivadoTheme) -> El {
    text(content.into())
        .font(VivadoTheme::FONT_TITLE)
        .color(t.text)
}

/// Truncating text cell with the full string as an expansion tooltip.
pub fn cut(content: impl Into<String>, style: TextStyle, color: Color) -> El {
    let s: String = content.into();
    text(s.clone())
        .font(style)
        .color(color)
        .truncate()
        .help_when_truncated(s)
}

/// Toolbar button. `reason = Some(..)` renders it disabled: no handler, a
/// screen-reader description, a hover reason, and faded ink.
pub fn btn(
    key: impl Into<String>,
    label: &str,
    t: &VivadoTheme,
    reason: Option<&str>,
    on: impl Fn(&mut AppState) + 'static,
) -> El {
    let enabled = reason.is_none();
    let mut b = el()
        .key(key.into())
        .role("button")
        .label(label.to_string())
        .focusable(enabled)
        .disabled(!enabled)
        .h(BTN_H)
        .px(8.0)
        .center()
        .radius(3.0)
        .border(1.0, t.border)
        .fill(t.panel_bg)
        .child(ui(label, t));
    match reason {
        None => b = b.hover(t.selection).on_press(on),
        Some(why) => {
            b = b
                .description(why.to_string())
                .help(why.to_string())
                .layer_opacity(0.45)
        }
    }
    b
}

/// Primary (accent-filled) action. Same disabled contract as [`btn`].
pub fn primary_btn(
    key: impl Into<String>,
    label: &str,
    t: &VivadoTheme,
    reason: Option<&str>,
    on: impl Fn(&mut AppState) + 'static,
) -> El {
    let enabled = reason.is_none();
    let mut b = el()
        .key(key.into())
        .role("button")
        .label(label.to_string())
        .focusable(enabled)
        .disabled(!enabled)
        .h(BTN_H + 2.0)
        .px(12.0)
        .center()
        .radius(3.0)
        .fill(if enabled { t.accent } else { t.control_off })
        .child(
            text(label.to_string())
                .font(VivadoTheme::FONT_TITLE)
                .color(if enabled { t.banner_ink } else { t.text_dim }),
        );
    match reason {
        None => b = b.hover(t.banner_hover()).on_press(on),
        Some(why) => b = b.description(why.to_string()).help(why.to_string()),
    }
    b
}

/// Blue text link running a real action.
pub fn link(
    key: impl Into<String>,
    label: &str,
    t: &VivadoTheme,
    on: impl Fn(&mut AppState) + 'static,
) -> El {
    el().key(key.into())
        .role("button")
        .label(label.to_string())
        .focusable(true)
        .h(ROW_H)
        .px(2.0)
        .center_y()
        .hover(t.selection)
        .on_press(on)
        .child(
            text(label.to_string())
                .font(VivadoTheme::FONT_UI)
                .color(t.link),
        )
}

/// Toolbar strip under a pane header.
pub fn toolbar(key: &'static str, t: &VivadoTheme) -> El {
    row()
        .key(key)
        .role("toolbar")
        .w_full()
        .h(TOOL_H)
        .items_center()
        .gap(6.0)
        .px(8.0)
        .fill(t.panel_bg)
        .border(1.0, t.border_light)
}

/// Thin vertical toolbar separator.
pub fn tool_sep(t: &VivadoTheme) -> El {
    el().w(1.0).h(16.0).fill(t.border).a11y_hidden()
}

/// Column width rule for [`header`] and [`cell`].
#[derive(Clone, Copy, Debug)]
pub enum ColW {
    Fixed(f32),
    Grow(f32),
}

/// One table column: title, width rule, and right alignment for numbers.
#[derive(Clone, Copy, Debug)]
pub struct ColSpec {
    pub title: &'static str,
    pub w: ColW,
    pub right: bool,
}

impl ColSpec {
    pub const fn fixed(title: &'static str, w: f32) -> Self {
        Self {
            title,
            w: ColW::Fixed(w),
            right: false,
        }
    }

    pub const fn num(title: &'static str, w: f32) -> Self {
        Self {
            title,
            w: ColW::Fixed(w),
            right: true,
        }
    }

    pub const fn grow(title: &'static str, g: f32) -> Self {
        Self {
            title,
            w: ColW::Grow(g),
            right: false,
        }
    }
}

/// Wrap `content` in the column's width box (numbers right-aligned).
pub fn cell(spec: &ColSpec, content: El) -> El {
    let content = if spec.right {
        content.w_full().align(Align::End)
    } else {
        content
    };
    let boxed = el().h(ROW_H).center_y().child(content);
    match spec.w {
        ColW::Fixed(w) => boxed.w(w),
        ColW::Grow(g) => boxed.grow(g).min_w(0.0),
    }
}

/// Table header band (`header_bg`, dim titles, same gaps as rows).
pub fn header(key: impl Into<String>, cols: &[ColSpec], t: &VivadoTheme) -> El {
    let mut r = row()
        .key(key.into())
        .w_full()
        .h(HEAD_H)
        .items_center()
        .gap(8.0)
        .px(8.0)
        .fill(t.header_bg);
    for c in cols {
        let head = text(c.title)
            .font(VivadoTheme::FONT_UI)
            .color(t.text_dim)
            .truncate();
        let head = if c.right {
            head.w_full().align(Align::End)
        } else {
            head
        };
        let boxed = el().center_y().child(head);
        r = r.child(match c.w {
            ColW::Fixed(w) => boxed.w(w),
            ColW::Grow(g) => boxed.grow(g).min_w(0.0),
        });
    }
    r
}

/// Selectable table row shell: selection fill, hover wash, button role.
/// Callers add one [`cell`] per column plus `.on_press`.
pub fn table_row(key: impl Into<String>, label: String, selected: bool, t: &VivadoTheme) -> El {
    row()
        .key(key.into())
        .role("button")
        .label(label)
        .focus_selected(selected)
        .focusable(true)
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(8.0)
        .px(8.0)
        .fill(if selected { t.selection } else { t.panel_bg })
        .hover(t.selection)
}

/// KPI card: small-caps label, large value, one-line sub-text.
pub fn kpi(
    key: impl Into<String>,
    label: &str,
    value: impl Into<String>,
    value_color: Color,
    sub: impl Into<String>,
    t: &VivadoTheme,
) -> El {
    let key = key.into();
    let value: String = value.into();
    let sub: String = sub.into();
    col()
        .key(key.clone())
        .grow(1.0)
        .min_w(0.0)
        .gap(2.0)
        .px(10.0)
        .py(8.0)
        .radius(4.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(caps(label, t).truncate())
        .child(
            text(value.clone())
                .font(FONT_KPI)
                .color(value_color)
                .truncate()
                .help_when_truncated(value)
                .key(format!("{key}-value")),
        )
        .child(cut(sub, VivadoTheme::FONT_UI, t.text_dim))
}

/// Titled card: bold title over a bordered, padded body.
pub fn card(key: impl Into<String>, heading: &str, body: El, t: &VivadoTheme) -> El {
    col()
        .key(key.into())
        .w_full()
        .gap(4.0)
        .child(title(heading, t))
        .child(
            col()
                .w_full()
                .fill(t.panel_bg)
                .border(1.0, t.border)
                .radius(4.0)
                .padding(8.0)
                .gap(4.0)
                .child(body),
        )
}

/// `label: value` row with a fixed label column.
pub fn kv(label: &str, value: El, label_w: f32, t: &VivadoTheme) -> El {
    row()
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(6.0)
        .child(el().w(label_w).child(dim(label, t).truncate()))
        .child(el().grow(1.0).min_w(0.0).child(value))
}

/// Outlined status pill (HIGH/LOW, In BD, Generated...).
pub fn badge(content: impl Into<String>, color: Color, t: &VivadoTheme) -> El {
    let _ = t;
    el().h(16.0)
        .px(6.0)
        .center()
        .radius(8.0)
        .border(1.0, color)
        .child(text(content.into()).font(FONT_PIN.mono()).color(color))
}

/// Empty state: one sentence plus at most one real call-to-action.
pub fn empty_state(key: &'static str, sentence: &str, cta: Option<El>, t: &VivadoTheme) -> El {
    let mut c = col()
        .key(key)
        .w_full()
        .gap(8.0)
        .px(8.0)
        .py(12.0)
        .child(dim(sentence, t).key(format!("{key}-text")));
    if let Some(cta) = cta {
        c = c.child(row().child(cta));
    }
    c
}

/// Bordered table frame around a header and its rows.
pub fn table_frame(key: impl Into<String>, t: &VivadoTheme) -> El {
    // 1pt inset keeps row fills from painting over the frame border.
    col()
        .key(key.into())
        .w_full()
        .padding(1.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
}

/// Tab strip button (active tab reads bold on the panel fill).
pub fn tab(
    key: impl Into<String>,
    label: &str,
    active: bool,
    t: &VivadoTheme,
    on: impl Fn(&mut AppState) + 'static,
) -> El {
    el().key(key.into())
        .role("tab")
        .label(label.to_string())
        .focus_selected(active)
        .focusable(true)
        .h(24.0)
        .px(10.0)
        .center_y()
        .fill(if active { t.panel_bg } else { t.header_bg })
        .hover(t.selection)
        .on_press(on)
        .child(
            text(label.to_string())
                .font(if active {
                    VivadoTheme::FONT_TITLE
                } else {
                    VivadoTheme::FONT_UI
                })
                .color(t.text),
        )
}

/// Single-line search field with a leading glyph.
pub fn search_input(
    key: &'static str,
    edit: &rune::TextEdit,
    placeholder: &'static str,
    t: &VivadoTheme,
    on_edit: impl for<'a> Fn(&'a mut AppState) -> &'a mut rune::TextEdit + 'static,
) -> El {
    row()
        .key(format!("{key}-box"))
        .role("search")
        .h(22.0)
        .items_center()
        .gap(6.0)
        .px(8.0)
        .radius(4.0)
        .fill(t.chrome_bg)
        .border(1.0, t.border_light)
        .child(
            rune::icon(&rune_kit::glyphs::SEARCH, 11.0)
                .color(t.text_dim)
                .a11y_hidden(),
        )
        .child(
            rune::text_input(edit, VivadoTheme::FONT_UI)
                .single_line()
                .key(key)
                .label(placeholder)
                .grow(1.0)
                .min_w(0.0)
                .color(t.text)
                .placeholder(placeholder, t.text_faint)
                .caret(t.text, t.selection)
                .on_edit(on_edit),
        )
}

/// `0x4000_0000`-style grouped hex for addresses.
pub fn addr_hex(v: u64) -> String {
    let raw = format!("{v:08x}");
    let mut out = String::from("0x");
    let n = raw.len();
    for (i, ch) in raw.chars().enumerate() {
        if i > 0 && (n - i) % 4 == 0 {
            out.push('_');
        }
        out.push(ch);
    }
    out
}

/// `4K`/`64K`/`1M`-style address range (Vivado Address Editor wording).
pub fn range_label(bytes: u64) -> String {
    if bytes >= 1 << 20 && bytes % (1 << 20) == 0 {
        format!("{}M", bytes >> 20)
    } else if bytes >= 1 << 10 && bytes % (1 << 10) == 0 {
        format!("{}K", bytes >> 10)
    } else {
        format!("{bytes}")
    }
}

/// Thousands-grouped integer (`4096` -> `4,096`).
pub fn grouped(n: usize) -> String {
    let raw = n.to_string();
    let mut out = String::new();
    for (i, ch) in raw.chars().enumerate() {
        if i > 0 && (raw.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formats_group_and_scale() {
        assert_eq!(addr_hex(0x4000_0000), "0x4000_0000");
        assert_eq!(addr_hex(0x1000), "0x0000_1000");
        assert_eq!(range_label(0x1000), "4K");
        assert_eq!(range_label(1 << 20), "1M");
        assert_eq!(range_label(100), "100");
        assert_eq!(grouped(4096), "4,096");
        assert_eq!(grouped(12), "12");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}
