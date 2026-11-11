//! Large-file editor body: virtual rows instead of one text field.
//!
//! A buffer up to [`LARGE_LINES`] lines renders as one multi-line field
//! beside a full gutter. Past that (the old IDE switched at 800 lines,
//! `C/editor.rs:817-819`, for the 27,117-line `ysyx_ibex.sv`) every line is
//! a virtual row ([`rune::El::virtual_rows`]): layout builds only the rows
//! around the viewport, each lexed on its own with the block-comment state
//! at its first byte from a cached line index, so the gutter, highlighting
//! and element count stay bounded however long the file is.
//!
//! The caret line is the one live field (`EditorState::line_edit`): typing
//! splices it back into the buffer as it happens, Enter splits the line,
//! Backspace at column 0 joins it to the line above, and arrows/Page keys
//! move between rows. Every other row is read-only text that parks the
//! caret on its line when pressed.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use rune::{El, GrowthSpec, Key, ScrollSpec, TextEdit, Tx, col, row, text, text_input};

use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::highlight::{Dialect, block_open_after, highlight_line_from};

/// Above this many lines the editor body virtualizes.
pub const LARGE_LINES: usize = 1500;
/// Lines Page Up / Page Down move in large-file mode.
const PAGE_LINES: usize = 40;

/// Cheap 64-bit fingerprint of a buffer (FNV-1a over 8-byte words), used
/// as the cache key for the line index, symbols and diagnostics.
pub fn fingerprint(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ text.len() as u64;
    let bytes = text.as_bytes();
    let mut chunks = bytes.chunks_exact(8);
    for c in &mut chunks {
        let w = u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
        h = (h ^ w).wrapping_mul(0x0000_0100_0000_01b3);
    }
    for &b in chunks.remainder() {
        h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Line starts plus the block-comment state at each line's first byte.
pub struct LineIndex {
    starts: Vec<usize>,
    in_block: Vec<bool>,
    len: usize,
}

impl LineIndex {
    pub fn build(text: &str, dialect: Dialect) -> Self {
        let mut starts = vec![0];
        let mut in_block = Vec::new();
        let mut open = false;
        for line in text.split('\n') {
            in_block.push(open);
            open = block_open_after(line, dialect, open);
            starts.push(starts.last().copied().unwrap_or(0) + line.len() + 1);
        }
        starts.pop();
        LineIndex {
            starts,
            in_block,
            len: text.len(),
        }
    }

    pub fn count(&self) -> usize {
        self.starts.len()
    }

    /// Byte range of 0-based line `i` (without its `\n`).
    pub fn range(&self, i: usize) -> std::ops::Range<usize> {
        let start = self.starts.get(i).copied().unwrap_or(self.len);
        let end = self.starts.get(i + 1).map(|n| n - 1).unwrap_or(self.len);
        start.min(self.len)..end.min(self.len)
    }

    fn in_block(&self, i: usize) -> bool {
        self.in_block.get(i).copied().unwrap_or(false)
    }
}

thread_local! {
    static INDEX: RefCell<Option<(u64, Dialect, Rc<LineIndex>)>> = const { RefCell::new(None) };
}

/// The line index of the live field, cached by fingerprint.
pub fn index(s: &AppState) -> Rc<LineIndex> {
    let dialect = crate::views::editor::active_dialect(s);
    let text = &s.editor.edit.text;
    let fp = fingerprint(text);
    INDEX.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((f, d, idx)) = c.as_ref()
            && *f == fp
            && *d == dialect
        {
            return idx.clone();
        }
        let idx = Rc::new(LineIndex::build(text, dialect));
        *c = Some((fp, dialect, idx.clone()));
        idx
    })
}

/// Whether the active buffer renders as virtual rows.
pub fn is_large(s: &AppState) -> bool {
    s.editor.edit.text.bytes().filter(|&b| b == b'\n').count() + 1 > LARGE_LINES
}

/// Key of the element that shows 0-based `line` (scroll-into-view target).
pub fn line_key(s: &AppState, line: usize) -> String {
    if is_large(s) {
        format!("editor-row-{line}")
    } else {
        format!("editor-gutter-{line}")
    }
}

// ---- caret line field ----

fn caret_line(s: &AppState) -> usize {
    s.editor.edit.line_of_byte(s.editor.edit.focus)
}

/// Re-seat the caret-line field on the caret's line (after a jump, undo,
/// replace ...), keeping the caret column, and ask for focus.
pub fn park_line_edit(s: &mut AppState) {
    let line = caret_line(s);
    let r = s.editor.edit.line_range(line);
    let col = s.editor.edit.focus.clamp(r.start, r.end) - r.start;
    let mut le = TextEdit::new(s.editor.edit.text[r.clone()].to_string());
    le.set_caret(col);
    le.focused = true;
    s.editor.line_edit = le;
    s.editor.line_at = Some(line);
    s.editor.cursor_line = line;
}

/// The caret-line field, re-seated first when it no longer mirrors its line.
fn line_edit_mut(s: &mut AppState) -> &mut TextEdit {
    let line = caret_line(s);
    let r = s.editor.edit.line_range(line);
    let stale = s.editor.line_at != Some(line) || s.editor.line_edit.text != s.editor.edit.text[r];
    if stale {
        park_line_edit(s);
    }
    &mut s.editor.line_edit
}

/// Write the caret-line field back into the buffer.
fn splice(s: &mut AppState) {
    let Some(line) = s.editor.line_at else { return };
    let r = s.editor.edit.line_range(line);
    let new = s.editor.line_edit.text.clone();
    if s.editor.edit.text[r.clone()] == new {
        return;
    }
    s.editor.edit.text.replace_range(r.clone(), &new);
    let caret = r.start + s.editor.line_edit.focus.min(new.len());
    s.editor.edit.set_caret(caret);
    s.editor.dirty = true;
}

/// Move the caret to `line` (clamped), column `col` clamped to the line.
fn goto_line_col(s: &mut AppState, line: usize, col: usize) {
    let last = s.editor.edit.text.bytes().filter(|&b| b == b'\n').count();
    let line = line.min(last);
    let r = s.editor.edit.line_range(line);
    let byte = (r.start + col).min(r.end);
    let byte = s.editor.edit.text.floor_char_boundary(byte);
    s.editor.edit.set_caret(byte);
    park_line_edit(s);
}

/// Enter: split the caret line at the caret.
fn split_line(s: &mut AppState, tx: &mut Tx) {
    splice(s);
    let Some(line) = s.editor.line_at else { return };
    let r = s.editor.edit.line_range(line);
    let at = r.start + s.editor.line_edit.focus.min(r.end - r.start);
    s.editor.edit.text.insert(at, '\n');
    s.editor.dirty = true;
    goto_line_col(s, line + 1, 0);
    tx.scroll_into_view(format!("editor-row-{}", line + 1));
}

/// Keys the caret-line field handles itself. True = consumed.
fn line_key_event(s: &mut AppState, key: &rune::KeyEvent, tx: &mut Tx) -> bool {
    let Some(line) = s.editor.line_at else {
        return false;
    };
    let col = s.editor.line_edit.focus;
    let empty_sel = s.editor.line_edit.anchor == s.editor.line_edit.focus;
    let len = s.editor.line_edit.text.len();
    let last = s.editor.edit.text.bytes().filter(|&b| b == b'\n').count();
    let target = match &key.key {
        Key::Up if !key.mods.shift => Some(line.saturating_sub(1)),
        Key::Down if !key.mods.shift => Some((line + 1).min(last)),
        Key::PageUp => Some(line.saturating_sub(PAGE_LINES)),
        Key::PageDown => Some((line + PAGE_LINES).min(last)),
        Key::Backspace if col == 0 && empty_sel && line > 0 => {
            splice(s);
            let start = s.editor.edit.line_range(line).start;
            let prev_len = s.editor.edit.line_range(line - 1).len();
            s.editor.edit.text.remove(start - 1);
            s.editor.dirty = true;
            goto_line_col(s, line - 1, prev_len);
            tx.scroll_into_view(format!("editor-row-{}", line - 1));
            return true;
        }
        Key::Delete if col == len && empty_sel && line < last => {
            splice(s);
            let end = s.editor.edit.line_range(line).end;
            s.editor.edit.text.remove(end);
            s.editor.dirty = true;
            goto_line_col(s, line, col);
            return true;
        }
        _ => None,
    };
    match target {
        Some(to) => {
            splice(s);
            goto_line_col(s, to, col);
            tx.scroll_into_view(format!("editor-row-{to}"));
            true
        }
        None => false,
    }
}

// ---- view ----

/// Find-match washes inside one line (respecting Match case).
fn match_spans(line: &str, query: &str, ignore_case: bool, t: &VivadoTheme) -> Vec<rune::Span> {
    if query.is_empty() {
        return Vec::new();
    }
    let hits: Vec<usize> = if ignore_case {
        let hay = line.to_ascii_lowercase();
        let q = query.to_ascii_lowercase();
        hay.match_indices(&q).map(|(i, _)| i).collect()
    } else {
        line.match_indices(query).map(|(i, _)| i).collect()
    };
    hits.into_iter()
        .map(|i| rune::Span::new(i, i + query.len()).bg(t.selection, 1.0))
        .collect()
}

/// Per-frame inputs every row reads (captured once, not per row).
struct RowCx {
    idx: Rc<LineIndex>,
    dialect: Dialect,
    t: VivadoTheme,
    markers: Option<crate::views::debug::GutterMarkers>,
    rtl: bool,
    line_h: f32,
}

fn row_el(s: &AppState, i: usize, cx: &RowCx) -> El {
    let t = &cx.t;
    let caret = caret_line(s);
    let current = i == caret;
    let mono = VivadoTheme::FONT_MONO.mono().lh(cx.line_h);
    let r = cx.idx.range(i);
    let line = s.editor.edit.text.get(r).unwrap_or("");
    let gutter =
        crate::views::editor_gutter::gutter_row(i, current, cx.markers.as_ref(), cx.rtl, t, s);
    let mut spans = highlight_line_from(line, cx.dialect, cx.idx.in_block(i), t);
    let q = &s.editor.find_query.text;
    let content = if current {
        let mirrors = s.editor.line_at == Some(i) && s.editor.line_edit.text == line;
        let edit = if mirrors {
            s.editor.line_edit.clone()
        } else {
            let mut e = TextEdit::new(line);
            let col = s.editor.edit.focus.saturating_sub(cx.idx.range(i).start);
            e.set_caret(col.min(line.len()));
            e
        };
        spans.extend(match_spans(&edit.text, q, s.editor.find_ignore_case, t));
        text_input(&edit, mono)
            .single_line()
            .key("editor-line-field")
            .label(format!("Line {}", i + 1))
            .grow(1.0)
            .min_w(0.0)
            .color(t.text)
            .spans(spans)
            .caret(t.text, t.selection)
            .on_edit(line_edit_mut)
            .on_input_change(|s: &mut AppState, _tx: &mut Tx| splice(s))
            .on_submit(split_line)
            .on_input_key(|s: &mut AppState, key: &rune::KeyEvent, tx: &mut Tx| {
                line_key_event(s, key, tx) || crate::views::editor::editor_key(s, key, tx)
            })
    } else {
        spans.extend(match_spans(line, q, s.editor.find_ignore_case, t));
        let full = line.to_string();
        text(full.clone())
            .key(format!("editor-text-{i}"))
            .font(mono)
            .color(t.text)
            .spans(spans)
            .grow(1.0)
            .min_w(0.0)
            .truncate()
            .help_when_truncated(full)
            .role("button")
            .label(format!("Edit line {}", i + 1))
            .on_press(move |s: &mut AppState| {
                let col = s.editor.line_edit.focus;
                goto_line_col(s, i, col);
            })
    };
    let mut r = row()
        .w_full()
        .h(cx.line_h)
        .items_center()
        .child(gutter)
        .child(
            row()
                .grow(1.0)
                .min_w(0.0)
                .h(cx.line_h)
                .items_center()
                .pl(4.0)
                .child(content),
        );
    if current {
        r = r.fill(t.current_line);
    }
    r
}

/// The virtualized code body for a large buffer.
pub fn large_body(s: &AppState, t: &VivadoTheme, line_h: f32) -> El {
    let idx = index(s);
    let count = idx.count();
    let cx = RowCx {
        idx,
        dialect: crate::views::editor::active_dialect(s),
        t: *t,
        markers: crate::views::debug::gutter_markers(s),
        rtl: !crate::views::editor::active_is_xdc(s),
        line_h,
    };
    col()
        .key("editor-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(
            col()
                .key("editor-rows")
                .w_full()
                .growth(GrowthSpec::default())
                .virtual_rows(
                    count,
                    line_h,
                    |_: &AppState, i| Cow::Owned(format!("editor-row-{i}")),
                    move |s: &AppState, i| row_el(s, i, &cx),
                )
                .virtual_rows_version(0u8),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CenterPane;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn big_state(lines: usize) -> AppState {
        let mut s = init();
        s.center = CenterPane::Editor;
        let mut text = String::new();
        for i in 0..lines {
            if i % 100 == 0 {
                text.push_str(&format!("module m{i}(input clk, output q{i});\n"));
            } else if i % 100 == 99 {
                text.push_str("endmodule\n");
            } else {
                text.push_str(&format!("  /* row */ assign w{i} = {i}'d0; // tail\n"));
            }
        }
        text.pop();
        crate::views::editor::sync_edit(&mut s);
        s.editor.lines = text.lines().map(str::to_string).collect();
        s.editor.edit = TextEdit::new(text);
        s
    }

    fn rt(s: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        let mut rt = Runtime::new(s, crate::view, spec.size, env);
        rt.frame(0.0);
        rt
    }

    #[test]
    fn a_30k_line_buffer_builds_a_bounded_view() {
        let s = big_state(30_000);
        assert!(is_large(&s));
        let started = std::time::Instant::now();
        let mut rt = rt(s);
        let first = started.elapsed();
        let n = rt.elements().len();
        assert!(n < 4_000, "element count must stay bounded, got {n}");
        assert!(rt.find("editor-row-0").is_some());
        assert!(rt.find("editor-gutter-0").is_some());
        assert!(
            rt.find("editor-row-29999").is_none(),
            "far rows are not built"
        );
        assert!(rt.find("editor-gutter-29999").is_none());
        // A second frame reuses the cached index: no full rebuild.
        rt.invalidate();
        let again = std::time::Instant::now();
        rt.frame(0.1);
        assert!(again.elapsed() < first.max(std::time::Duration::from_secs(2)));
        assert!(rt.elements().len() < 4_000);
    }

    #[test]
    fn small_buffers_keep_the_single_field() {
        let mut rt = rt({
            let mut s = init();
            s.center = CenterPane::Editor;
            s
        });
        rt.frame(0.1);
        assert!(rt.find("editor-field").is_some());
        assert!(rt.find("editor-rows").is_none());
    }

    #[test]
    fn caret_line_edits_split_and_join_lines() {
        let mut s = big_state(2_000);
        crate::views::editor::move_cursor_to(&mut s, 5);
        park_line_edit(&mut s);
        let before = s.editor.edit.line_range(5);
        let orig = s.editor.edit.text[before.clone()].to_string();
        // Type at column 0 of line 5.
        s.editor.line_edit.set_caret(0);
        s.editor.line_edit.insert("X");
        splice(&mut s);
        let now = s.editor.edit.line_range(5);
        assert_eq!(&s.editor.edit.text[now], format!("X{orig}"));
        assert!(s.editor.dirty);
        // Split after the X, then join back with Backspace at column 0.
        s.editor.line_edit.set_caret(1);
        let lines_before = s.editor.edit.text.lines().count();
        let mut tx = Tx::default();
        split_line(&mut s, &mut tx);
        assert_eq!(s.editor.edit.text.lines().count(), lines_before + 1);
        assert_eq!(s.editor.line_at, Some(6));
        assert_eq!(&s.editor.edit.text[s.editor.edit.line_range(5)], "X");
        let key = rune::KeyEvent {
            key: Key::Backspace,
            mods: rune::Mods::default(),
        };
        assert!(line_key_event(&mut s, &key, &mut tx));
        assert_eq!(s.editor.edit.text.lines().count(), lines_before);
        assert_eq!(
            &s.editor.edit.text[s.editor.edit.line_range(5)],
            format!("X{orig}")
        );
        assert_eq!((s.editor.line_at, s.editor.line_edit.focus), (Some(5), 1));
        // Down keeps the column.
        let down = rune::KeyEvent {
            key: Key::Down,
            mods: rune::Mods::default(),
        };
        assert!(line_key_event(&mut s, &down, &mut tx));
        assert_eq!(s.editor.line_at, Some(6));
    }

    #[test]
    fn line_index_matches_text_lines() {
        let text = "a\n/* b\nc */ d\n\ne";
        let idx = LineIndex::build(text, Dialect::Verilog);
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(idx.count(), lines.len());
        for (i, l) in lines.iter().enumerate() {
            assert_eq!(&text[idx.range(i)], *l);
        }
        assert_eq!(
            (0..5).map(|i| idx.in_block(i)).collect::<Vec<_>>(),
            vec![false, false, true, false, false]
        );
        assert_ne!(fingerprint("abc"), fingerprint("abd"));
    }
}
