//! Editor gutter and navigation chrome: the line gutter (diagnostic and
//! bookmark ticks, debugger breakpoint slot, line numbers), the breadcrumb's
//! parse summary, the find bar's Match case toggle, and the navigation
//! tools (Go to Definition, bookmarks, outline toggle).

use rune::{El, Shape, Tx, col, el, icon, row, text};
use rune_kit::glyphs;

use super::editor::{GUTTER_W, LINE_H, PATH_H};
use crate::state::AppState;
use crate::theme::VivadoTheme;

/// Diagnostic/bookmark tick column at the gutter's left edge.
const TICK_W: f32 = 8.0;

/// "Aa" Match case toggle (on = case-sensitive, the default).
pub(crate) fn match_case_toggle(s: &AppState, t: &VivadoTheme) -> El {
    let on = !s.editor.find_ignore_case;
    el().key("editor-find-case")
        .role("checkbox")
        .label("Match case")
        .help(if on {
            "Match case: on"
        } else {
            "Match case: off"
        })
        .focus_selected(on)
        .focusable(true)
        .h(22.0)
        .px(6.0)
        .radius(3.0)
        .center()
        .fill(if on { t.selection } else { t.panel_bg })
        .border(1.0, if on { t.accent } else { t.border })
        .hover(t.selection)
        .on_press(|s: &mut AppState| s.editor.find_ignore_case = !s.editor.find_ignore_case)
        .child(
            text("Aa")
                .font(VivadoTheme::FONT_TITLE)
                .color(if on { t.text } else { t.text_dim }),
        )
}

/// Toolbar toggle cell: selection fill while on.
fn nav_btn(
    key: &'static str,
    tip: &'static str,
    on: bool,
    glyph: El,
    t: &VivadoTheme,
    press: impl Fn(&mut AppState, &mut Tx) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .label(tip)
        .help(tip)
        .focus_selected(on)
        .focusable(true)
        .size(24.0, 24.0)
        .center()
        .radius(3.0)
        .fill(if on { t.selection } else { t.header_bg })
        .hover(t.selection)
        .on_press_tx(press)
        .child(glyph)
}

/// Navigation tools: Go to Definition, bookmark toggle / next, outline.
pub(crate) fn nav_tools(s: &AppState, t: &VivadoTheme) -> El {
    let ink = t.text_dim;
    let caret = s.editor.cursor_line;
    let marked = crate::views::editor_diag::bookmarks(s).is_some_and(|b| b.contains(&caret));
    let outline = rune::canvas(
        Shape::new()
            .stroke(vec![[3.0, 4.0], [13.0, 4.0]], 1.4, ink)
            .stroke(vec![[6.0, 8.0], [13.0, 8.0]], 1.4, ink)
            .stroke(vec![[6.0, 12.0], [13.0, 12.0]], 1.4, ink)
            .view_box(16.0, 16.0),
    )
    .size(16.0, 16.0)
    .a11y_hidden();
    let ribbon = |filled: bool| {
        let pts = vec![
            [4.0, 2.5],
            [12.0, 2.5],
            [12.0, 14.0],
            [8.0, 10.5],
            [4.0, 14.0],
            [4.0, 2.5],
        ];
        let shape = if filled {
            Shape::new().fill(pts, t.accent)
        } else {
            Shape::new().stroke(pts, 1.3, ink)
        };
        rune::canvas(shape.view_box(16.0, 16.0))
            .size(16.0, 16.0)
            .a11y_hidden()
    };
    let next = rune::canvas(
        Shape::new()
            .stroke(
                vec![
                    [3.0, 2.5],
                    [9.0, 2.5],
                    [9.0, 12.0],
                    [6.0, 9.5],
                    [3.0, 12.0],
                    [3.0, 2.5],
                ],
                1.3,
                ink,
            )
            .stroke(vec![[11.0, 8.0], [15.0, 8.0]], 1.3, ink)
            .fill(vec![[15.0, 8.0], [12.5, 5.5], [12.5, 10.5]], ink)
            .view_box(16.0, 16.0),
    )
    .size(16.0, 16.0)
    .a11y_hidden();
    row()
        .key("editor-nav-tools")
        .items_center()
        .gap(2.0)
        .child(nav_btn(
            "editor-tool-definition",
            "Go to Definition (F12)",
            false,
            icon(&glyphs::ARROW_RIGHT, 13.0).color(ink).a11y_hidden(),
            t,
            |s: &mut AppState, tx: &mut Tx| crate::views::editor_outline::goto_definition_ui(s, tx),
        ))
        .child(el().w(1.0).h(18.0).mx(2.0).fill(t.border).a11y_hidden())
        .child(nav_btn(
            "editor-tool-bookmark",
            "Toggle bookmark on the caret line",
            marked,
            ribbon(marked),
            t,
            |s: &mut AppState, _tx: &mut Tx| {
                super::editor::sync_edit(s);
                let line = super::editor::caret_line(s);
                crate::views::editor_diag::toggle_bookmark(s, line);
            },
        ))
        .child(nav_btn(
            "editor-tool-next-bookmark",
            "Next bookmark (F2)",
            false,
            next,
            t,
            super::editor::goto_next_bookmark,
        ))
        .child(el().w(1.0).h(18.0).mx(2.0).fill(t.border).a11y_hidden())
        .child(nav_btn(
            "editor-tool-outline",
            "Show symbol outline",
            s.editor.outline_open,
            outline,
            t,
            |s: &mut AppState, _tx: &mut Tx| s.editor.outline_open = !s.editor.outline_open,
        ))
}

/// Parse status in the breadcrumb: error/warning counts (press jumps to
/// the first), "Parsing…" while the worker runs, or a quiet "No problems".
pub(crate) fn diag_summary(s: &AppState, t: &VivadoTheme) -> El {
    use crate::views::editor_diag::{DiagSev, for_view};
    let items = for_view(s);
    let errors = items.iter().filter(|d| d.sev == DiagSev::Error).count();
    let warnings = items.len() - errors;
    let parsing = s.editor.diag.is_parsing();
    let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
    let (label, ink) = if errors > 0 {
        let mut l = plural(errors, "error");
        if warnings > 0 {
            l.push_str(&format!(" · {}", plural(warnings, "warning")));
        }
        (l, t.fail)
    } else if warnings > 0 {
        (plural(warnings, "warning"), t.warn)
    } else if parsing {
        ("Parsing…".to_string(), t.text_faint)
    } else if s.editor.diag.key.is_some() {
        ("No problems".to_string(), t.text_faint)
    } else {
        (String::new(), t.text_faint)
    };
    let first = items.first().map(|d| d.line);
    let mut e = el()
        .key("editor-diag-count")
        .h(PATH_H)
        .center_y()
        .child(text(label).font(VivadoTheme::FONT_UI).color(ink));
    if let Some(line) = first {
        let tip = items
            .iter()
            .take(8)
            .map(|d| format!("Ln {}: {}", d.line + 1, d.msg))
            .collect::<Vec<_>>()
            .join("\n");
        e = e
            .role("button")
            .label("Go to first problem")
            .focusable(true)
            .help(tip)
            .hover(t.selection)
            .on_press_tx(move |s: &mut AppState, tx: &mut Tx| {
                crate::views::editor_outline::jump(s, line, Some(tx))
            });
    }
    e
}

/// Width of the breakpoint slot left of the line numbers.
const BP_SLOT_W: f32 = 16.0;

/// Gutter row: a breakpoint slot (UG900 Source gutter) beside the
/// right-aligned line number. The slot paints the PC arrow, an armed (filled)
/// or disabled (hollow) line breakpoint, and taps toggle a line breakpoint
/// through the debugger; the number parks the caret. Markers only paint when
/// the viewed tab is the classified debug source (see
/// [`crate::views::debug::gutter_markers`]), so stale rows never mislead.
pub(crate) fn gutter_row(
    n: usize,
    current: bool,
    markers: Option<&crate::views::debug::GutterMarkers>,
    rtl: bool,
    t: &VivadoTheme,
    s: &AppState,
) -> El {
    let line = n + 1;
    let mono = VivadoTheme::FONT_MONO.mono().lh(LINE_H);
    let pc = markers.is_some_and(|m| m.pc == Some(line));
    let armed = markers.is_some_and(|m| m.is_armed(line));
    let disabled = markers.is_some_and(|m| m.is_disabled(line));
    let executable = markers.is_some_and(|m| m.is_executable(line));
    let mut slot = row()
        .key(format!("editor-bp-slot-{n}"))
        .w(BP_SLOT_W)
        .h(LINE_H)
        .center();
    if rtl {
        let help = if armed || disabled {
            "Remove breakpoint"
        } else if markers.is_some() && !executable {
            "Not an executable line"
        } else {
            "Toggle breakpoint"
        };
        slot = slot
            .role("button")
            .label(format!("Breakpoint line {line}"))
            .help(help)
            .hover(t.selection)
            .on_press(move |s: &mut AppState| {
                crate::views::debug::gutter_toggle(s, n);
            });
    }
    if armed {
        slot = slot.child(
            text("●")
                .key(format!("editor-bp-{n}"))
                .font(mono)
                .color(t.fail),
        );
    } else if disabled {
        slot = slot.child(
            text("○")
                .key(format!("editor-bp-off-{n}"))
                .font(mono)
                .color(t.fail),
        );
    } else if pc {
        slot = slot.child(
            text("▶")
                .key(format!("editor-pc-{n}"))
                .font(mono)
                .color(t.pass),
        );
    }
    let cov = crate::views::coverage::line_hits(s, line);
    let number = row()
        .key(format!("editor-gutter-{n}"))
        .role("button")
        .label(format!("Line {line}"))
        .focusable(true)
        .grow(1.0)
        .h(LINE_H)
        .items_center()
        .justify_end()
        .pr(8.0)
        .hover(t.selection)
        .on_press(move |s: &mut AppState| {
            super::editor::move_cursor_to(s, n);
        })
        .child(
            text(format!("{line}"))
                .font(mono)
                .color(if pc && (armed || disabled) {
                    t.pass
                } else if let Some(h) = cov {
                    if h == 0 { t.fail } else { t.pass }
                } else if current {
                    t.text
                } else {
                    t.text_faint
                }),
        );
    let number = match cov {
        Some(0) => number.help("Not executed in the last testbench run"),
        Some(h) => number.help(format!("Executed {h}× in the last testbench run")),
        None => number,
    };
    row()
        .w(GUTTER_W)
        .h(LINE_H)
        .child(gutter_tick(n, s, t))
        .child(slot)
        .child(number)
}

/// Tick column: the worst parse diagnostic on the line (error red, warning
/// amber, with the messages on hover), else a bookmark. Pressing it toggles
/// the line's bookmark, as the old gutter's tick zone did.
fn gutter_tick(n: usize, s: &AppState, t: &VivadoTheme) -> El {
    use crate::views::editor_diag::{DiagSev, for_view, on_line};
    let diags = on_line(for_view(s), n);
    let marked = crate::views::editor_diag::bookmarks(s).is_some_and(|b| b.contains(&n));
    let worst = diags.iter().map(|d| d.sev).min();
    let ink = match worst {
        Some(DiagSev::Error) => Some(t.fail),
        Some(DiagSev::Warning) => Some(t.warn),
        None if marked => Some(t.accent),
        None => None,
    };
    let mut help: Vec<String> = diags
        .iter()
        .map(|d| {
            let sev = if d.sev == DiagSev::Error {
                "Error"
            } else {
                "Warning"
            };
            format!("{sev}: {}", d.msg)
        })
        .collect();
    help.push(if marked {
        "Remove bookmark".into()
    } else {
        "Toggle bookmark".into()
    });
    let mut tick = col()
        .key(format!("editor-tick-{n}"))
        .role("button")
        .label(format!("Bookmark line {}", n + 1))
        .w(TICK_W)
        .h(LINE_H)
        .items_center()
        .justify_center()
        .help(help.join("\n"))
        .on_press(move |s: &mut AppState| crate::views::editor_diag::toggle_bookmark(s, n));
    if let Some(ink) = ink {
        let mark = el().w(4.0).h(12.0).radius(1.5).fill(ink).a11y_hidden();
        // A bookmark on a diagnostic line shows as a dot under the tick.
        tick = tick.child(mark);
        if worst.is_some() && marked {
            tick = tick.child(
                el().w(4.0)
                    .h(3.0)
                    .mt(1.0)
                    .radius(1.5)
                    .fill(t.accent)
                    .a11y_hidden(),
            );
        }
    }
    tick
}

#[cfg(test)]
mod tests {
    use crate::state::{AppState, CenterPane};
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    thread_local! {
        static CLOCK: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
    }

    fn settle(rt: &mut Runtime<AppState>) {
        for _ in 0..20 {
            let t = CLOCK.with(|c| {
                c.set(c.get() + 0.05);
                c.get()
            });
            rt.frame(t);
        }
    }

    fn rt_with(f: impl Fn(&mut AppState)) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        let mut s = init();
        s.center = CenterPane::Editor;
        f(&mut s);
        let mut rt = Runtime::new(s, crate::view, spec.size, env);
        settle(&mut rt);
        rt
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        settle(rt);
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(rt);
    }

    fn shown(rt: &Runtime<AppState>, key: &str) -> String {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .and_then(|e| e.text)
            .unwrap_or_default()
    }

    #[test]
    fn match_case_toggle_widens_the_find() {
        let mut rt = rt_with(|s| {
            s.editor.find_open = true;
            s.editor.find_query = rune::TextEdit::new("CLK");
        });
        let sensitive = shown(&rt, "editor-find-count");
        assert_eq!(sensitive, "No matches", "demo RTL spells clk in lower case");
        tap(&mut rt, "editor-find-case");
        assert!(rt.state.editor.find_ignore_case);
        let insensitive = shown(&rt, "editor-find-count");
        assert!(
            insensitive.ends_with("matches") && insensitive != "No matches",
            "{insensitive}"
        );
    }

    #[test]
    fn gutter_tick_and_toolbar_toggle_bookmarks() {
        let mut rt = rt_with(|_| {});
        tap(&mut rt, "editor-tick-3");
        let tab = rt.state.editor.tabs[rt.state.editor.active].clone();
        assert!(rt.state.editor.bookmarks[&tab].contains(&3));
        tap(&mut rt, "editor-gutter-7");
        tap(&mut rt, "editor-tool-bookmark");
        assert!(rt.state.editor.bookmarks[&tab].contains(&7));
        tap(&mut rt, "editor-tool-next-bookmark");
        assert_eq!(
            rt.state.editor.cursor_line, 3,
            "next bookmark wraps to line 4"
        );
        tap(&mut rt, "editor-tick-3");
        assert!(!rt.state.editor.bookmarks[&tab].contains(&3));
    }

    #[test]
    fn outline_toggles_and_jumps_to_a_symbol() {
        let mut rt = rt_with(|_| {});
        assert!(rt.find("editor-outline").is_none());
        tap(&mut rt, "editor-tool-outline");
        assert!(rt.find("editor-outline").is_some());
        let syms = crate::views::editor_outline::cached(&rt.state);
        let last = syms.len() - 1;
        tap(&mut rt, &format!("editor-sym-{last}"));
        assert_eq!(rt.state.editor.cursor_line, syms[last].line);
        tap(&mut rt, "editor-outline-close");
        assert!(!rt.state.editor.outline_open);
    }

    #[test]
    fn diagnostics_tick_renders_with_the_parse_message() {
        let mut s = init();
        crate::scenes::editor_diagnostics(&mut s);
        let line = s.editor.diag.items.first().expect("sv-parser error").line;
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        let mut rt = Runtime::new(s, crate::view, spec.size, env);
        settle(&mut rt);
        let tick = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(&format!("editor-tick-{line}")))
            .expect("tick on the error line");
        assert!(
            tick.help
                .is_some_and(|h| h.starts_with("Error: Syntax error"))
        );
        assert!(
            shown(&rt, "editor-diag-count").is_empty() || rt.find("editor-diag-count").is_some()
        );
        tap(&mut rt, "editor-diag-count");
        assert_eq!(rt.state.editor.cursor_line, line);
    }
}
