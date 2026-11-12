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
//! move between rows. Every other row is read-only text; a click parks the
//! caret on its line at the clicked column ([`col_at_x`]).
//!
//! Long lines scroll sideways rather than truncate: the body is a
//! [`rune::El::scroll_xy`] viewport whose row list is as wide as the
//! longest line (measured once per buffer fingerprint, so the width does
//! not jump as rows are built), and each row's gutter holds to the left
//! edge with [`rune::El::sticky_left`], as rune-kit's diff view does. Only
//! the rows around the viewport exist, so this costs nothing per line.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use rune::{
    Drag, El, GrowthSpec, Key, Phase, ScrollSpec, TextEdit, TextStyle, Tx, col, row, text,
    text_input,
};

use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::highlight::{Dialect, block_open_after, highlight_line_from};

/// Above this many lines the editor body virtualizes.
pub const LARGE_LINES: usize = 1500;
/// Lines Page Up / Page Down move in large-file mode.
const PAGE_LINES: usize = 40;
/// Columns a tab may take (widest case, for the content width).
const TAB_COLS: usize = 4;
/// Room after the longest line so its last character clears the edge.
const END_PAD: f32 = 24.0;

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
    /// Display columns of the widest line (tabs as [`TAB_COLS`]).
    max_cols: usize,
    /// That line's index.
    widest: usize,
    /// Its drawn width, measured once per index (see [`content_width`]).
    widest_w: std::cell::OnceCell<f32>,
}

impl LineIndex {
    pub fn build(text: &str, dialect: Dialect) -> Self {
        let mut starts = vec![0];
        let mut in_block = Vec::new();
        let mut open = false;
        let mut max_cols = 0;
        let mut widest = 0;
        for line in text.split('\n') {
            in_block.push(open);
            // Bytes x TAB_COLS bounds the columns: count chars only for
            // lines that could beat the current widest.
            if line.len() * TAB_COLS > max_cols {
                let cols = line
                    .chars()
                    .map(|c| if c == '\t' { TAB_COLS } else { 1 })
                    .sum::<usize>();
                if cols > max_cols {
                    max_cols = cols;
                    widest = in_block.len() - 1;
                }
            }
            open = block_open_after(line, dialect, open);
            starts.push(starts.last().copied().unwrap_or(0) + line.len() + 1);
        }
        starts.pop();
        LineIndex {
            starts,
            in_block,
            len: text.len(),
            max_cols,
            widest,
            widest_w: std::cell::OnceCell::new(),
        }
    }

    pub fn count(&self) -> usize {
        self.starts.len()
    }

    /// Display columns of the widest line.
    pub fn max_cols(&self) -> usize {
        self.max_cols
    }

    /// Byte range of 0-based line `i` (without its `\n`).
    pub fn range(&self, i: usize) -> std::ops::Range<usize> {
        let start = self.starts.get(i).copied().unwrap_or(self.len);
        let end = self.starts.get(i + 1).map(|n| n - 1).unwrap_or(self.len);
        start.min(self.len)..end.min(self.len)
    }

    /// 0-based line holding byte `at` (binary search over line starts).
    pub fn line_of(&self, at: usize) -> usize {
        self.starts.partition_point(|&st| st <= at).saturating_sub(1)
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
    // Stops at the LARGE_LINES-th newline: O(threshold) per frame, not
    // O(file), on a 20k-line buffer.
    s.editor
        .edit
        .text
        .bytes()
        .filter(|&b| b == b'\n')
        .nth(LARGE_LINES - 1)
        .is_some()
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

/// Byte offset in `line` of the caret position nearest to `x` points from
/// the start of the text as drawn in `style`: a binary search over char
/// boundaries on measured prefix advances (exact for any font, tabs and
/// wide characters; ~log2(len) measurements per click).
pub fn col_at_x(line: &str, x: f32, style: TextStyle) -> usize {
    if x <= 0.0 || line.is_empty() {
        return 0;
    }
    let bounds: Vec<usize> = line
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(line.len()))
        .collect();
    let adv = |k: usize| rune::text::advance(&line[..bounds[k]], style);
    // Last boundary whose advance is <= x.
    let (mut lo, mut hi) = (0usize, bounds.len() - 1);
    if adv(hi) <= x {
        return line.len();
    }
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if adv(mid) <= x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    // Round to the nearer edge of the character under x.
    if x - adv(lo) <= adv(hi) - x {
        bounds[lo]
    } else {
        bounds[hi]
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
    /// Caret line, counted once per frame (not once per row).
    caret: usize,
}

fn mono(line_h: f32) -> TextStyle {
    VivadoTheme::FONT_MONO.mono().lh(line_h)
}

fn row_el(s: &AppState, i: usize, cx: &RowCx) -> El {
    let t = &cx.t;
    let current = i == cx.caret;
    let mono = mono(cx.line_h);
    let r = cx.idx.range(i);
    let line = s.editor.edit.text.get(r).unwrap_or("");
    // The gutter holds to the viewport's left edge while long lines scroll
    // sideways under it, on its own opaque fill.
    let gutter =
        crate::views::editor_gutter::gutter_row(i, current, cx.markers.as_ref(), cx.rtl, t, s)
            .fill(if current { t.current_line } else { t.panel_bg })
            .sticky_left(0.0);
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
        // Whole line, unclipped: the viewport scrolls sideways to it.
        text(line.to_string())
            .key(format!("editor-text-{i}"))
            .font(mono)
            .color(t.text)
            .spans(spans)
            .grow(1.0)
            .role("button")
            .label(format!("Edit line {}", i + 1))
            // A click lands on the clicked column (its Up runs before the
            // press, which then re-seats the same column).
            .on_drag(move |s: &mut AppState, d: &mut Drag, _tx: &mut Tx| {
                if d.phase == Phase::Up && !d.cancelled() {
                    click_line(s, i, d.x, mono);
                }
            })
            // Keyboard activation keeps the caret column.
            .on_press(move |s: &mut AppState| {
                if s.editor.line_at != Some(i) {
                    let col = s.editor.line_edit.focus;
                    goto_line_col(s, i, col);
                }
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

/// Park the caret on `line` at the column under `x` (text-local points).
fn click_line(s: &mut AppState, line: usize, x: f32, style: TextStyle) {
    let r = s.editor.edit.line_range(line);
    let col = col_at_x(&s.editor.edit.text[r], x, style);
    goto_line_col(s, line, col);
}

/// Width of the row list: gutter, text inset, the widest line, end room.
/// The widest line is shaped once per buffer version (the index is cached
/// by fingerprint), so scrolling never re-measures it.
fn content_width(s: &AppState, idx: &LineIndex, line_h: f32) -> f32 {
    let style = mono(line_h);
    let text_w = *idx.widest_w.get_or_init(|| {
        let line = s.editor.edit.text.get(idx.range(idx.widest)).unwrap_or("");
        // Tabs: at least TAB_COLS digit widths each.
        let cols = idx.max_cols as f32 * rune::text::advance("0", style);
        rune::text::advance(line, style).max(cols)
    });
    crate::views::editor::GUTTER_W + 4.0 + text_w + END_PAD
}

/// The virtualized code body for a large buffer.
pub fn large_body(s: &AppState, t: &VivadoTheme, line_h: f32) -> El {
    let idx = index(s);
    let count = idx.count();
    let width = content_width(s, &idx, line_h);
    // From the index: a binary search, not a scan to the caret.
    let caret = idx.line_of(s.editor.edit.focus);
    let cx = RowCx {
        idx,
        dialect: crate::views::editor::active_dialect(s),
        t: *t,
        markers: crate::views::debug::gutter_markers(s),
        rtl: !crate::views::editor::active_is_xdc(s),
        line_h,
        caret,
    };
    col()
        .key("editor-scroll")
        .w_full()
        .min_w(0.0)
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_xy()
        .child(
            col()
                .key("editor-rows")
                .w_full()
                .min_w(width)
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
    fn pressing_a_row_moves_the_live_field_there() {
        let mut rt = rt(big_state(3_000));
        for k in 1..20 {
            rt.frame(k as f64 * 0.05);
        }
        let (x, y) = rt.find("editor-text-12").expect("row 12 is built");
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for k in 20..40 {
            rt.frame(k as f64 * 0.05);
        }
        assert_eq!(rt.state.editor.line_at, Some(12));
        assert_eq!(rt.state.editor.cursor_line, 12);
        let field = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("editor-line-field"))
            .expect("caret row is a field");
        assert!(field.focused, "the moved field takes focus");
        assert!(rt.find("editor-text-12").is_none(), "row 12 is now the field");
    }

    /// `big_state` with line `at` replaced by a `cols`-character line.
    fn with_long_line(mut s: AppState, at: usize, cols: usize) -> AppState {
        let r = s.editor.edit.line_range(at);
        let long: String = (0..cols)
            .map(|k| char::from(b'a' + (k % 26) as u8))
            .collect();
        s.editor.edit.text.replace_range(r, &long);
        let text = s.editor.edit.text.clone();
        s.editor.lines = text.lines().map(str::to_string).collect();
        s.editor.edit = TextEdit::new(text);
        s
    }

    fn settle(rt: &mut Runtime<AppState>, from: usize) -> usize {
        for k in from..from + 20 {
            rt.frame(k as f64 * 0.05);
        }
        from + 20
    }

    fn click(rt: &mut Runtime<AppState>, x: f32, y: f32) {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
    }

    #[test]
    fn line_index_measures_the_widest_line_with_tabs() {
        let idx = LineIndex::build("ab\n\t\tx\nabcdefg\n", Dialect::Verilog);
        assert_eq!(idx.max_cols(), 2 * TAB_COLS + 1);
        let idx = LineIndex::build("ab\nabcdefghij\n\t", Dialect::Verilog);
        assert_eq!(idx.max_cols(), 10);
    }

    #[test]
    fn col_at_x_maps_points_to_the_nearest_column() {
        let style = mono(crate::views::editor::LINE_H);
        let line = "assign q = d;";
        // x within the 6th character (between the advances of 5 and 6
        // chars), nearer its left or right edge.
        let at = |k: usize| rune::text::advance(&line[..k], style);
        let (l, r) = (at(5), at(6));
        assert!(r > l, "measured: {l} {r}");
        assert_eq!(col_at_x(line, -3.0, style), 0);
        assert_eq!(col_at_x(line, l + 0.3 * (r - l), style), 5);
        assert_eq!(col_at_x(line, l + 0.7 * (r - l), style), 6);
        assert_eq!(col_at_x(line, at(line.len()) + 50.0, style), line.len());
        // Multi-byte text lands on char boundaries.
        let wide = "é = ü;";
        let x = rune::text::advance("é ", style) + 0.1;
        let c = col_at_x(wide, x, style);
        assert!(wide.is_char_boundary(c), "{c}");
        assert_eq!(c, "é ".len());
    }

    #[test]
    fn clicking_a_row_lands_on_the_clicked_column() {
        let mut s = big_state(3_000);
        // The caret sits at column 0 of line 5 first: the click must not
        // restore that column.
        crate::views::editor::move_cursor_to(&mut s, 5);
        park_line_edit(&mut s);
        let mut rt = rt(s);
        let t = settle(&mut rt, 1);
        let (r, _) = rt.presented("editor-text-12").expect("row 12 is built");
        let line12 = {
            let st = &rt.state.editor.edit;
            st.text[st.line_range(12)].to_string()
        };
        let style = mono(crate::views::editor::LINE_H);
        let x = rune::text::advance(&line12[..10], style) + 0.5;
        click(&mut rt, r[0] + x, r[1] + r[3] * 0.5);
        settle(&mut rt, t);
        assert_eq!(rt.state.editor.line_at, Some(12));
        assert_eq!(rt.state.editor.line_edit.focus, 10, "the clicked column");
        let line = rt.state.editor.edit.line_range(12);
        assert_eq!(rt.state.editor.edit.focus, line.start + 10);
    }

    #[test]
    fn long_lines_scroll_sideways_under_a_sticky_gutter() {
        let mut rt = rt(with_long_line(big_state(3_000), 12, 900));
        let mut t = settle(&mut rt, 1);
        let view = rt.presented("editor-scroll").unwrap().0;
        let rows = rt.presented("editor-rows").unwrap().0;
        let style = mono(crate::views::editor::LINE_H);
        let long: String = {
            let st = &rt.state.editor.edit;
            st.text[st.line_range(12)].to_string()
        };
        let long_w = rune::text::advance(&long, style);
        assert!(
            rows[2] >= long_w,
            "the list is as wide as the longest line: {} vs view {}",
            rows[2],
            view[2]
        );
        let text0 = rt.presented("editor-text-12").unwrap().0;
        assert!(text0[2] >= long_w - 1.0, "the line is not truncated: {}", text0[2]);
        let gutter0 = rt.presented("editor-gutter-12").unwrap().0;
        // A horizontal gesture scrolls the code; the gutter stays put.
        let (cx, cy) = (view[0] + view[2] * 0.6, text0[1] + text0[3] * 0.5);
        for _ in 0..10 {
            rt.wheel_xy(cx, cy, -120.0, 0.0);
            t = settle(&mut rt, t);
        }
        let text1 = rt.presented("editor-text-12").unwrap().0;
        let gutter1 = rt.presented("editor-gutter-12").unwrap().0;
        assert!(text1[0] < text0[0] - 200.0, "code moved left: {} -> {}", text0[0], text1[0]);
        assert_eq!(gutter1[0], gutter0[0], "gutter held to the left edge");
        // A click on the scrolled line still lands on the column under it.
        let col = col_at_x(&long, cx - text1[0], style);
        click(&mut rt, cx, cy);
        t = settle(&mut rt, t);
        assert_eq!(rt.state.editor.line_at, Some(12));
        assert!(rt.state.editor.line_edit.focus.abs_diff(col) <= 1, "{} vs {col}", rt.state.editor.line_edit.focus);
        // End moves the caret off-screen right; the viewport follows it.
        rt.key(rune::KeyEvent {
            key: Key::End,
            mods: rune::Mods::default(),
        });
        settle(&mut rt, t);
        assert_eq!(rt.state.editor.line_edit.focus, 900);
        let crumb = rt
            .elements()
            .into_iter()
            .filter_map(|e| e.label.clone().or(e.text.clone()))
            .any(|l| l.contains("Ln 13, Col 901"));
        assert!(crumb, "the breadcrumb follows the caret-line field");
        let field = rt.presented("editor-line-field").unwrap().0;
        let caret_x = field[0] + long_w;
        assert!(
            caret_x <= view[0] + view[2] + 1.0 && caret_x >= view[0],
            "caret at {caret_x} inside view {view:?}"
        );
    }

    /// A real 27k-line source and a generated 20k-line netlist with long
    /// lines stay responsive: bounded elements, quick frames while
    /// scrolling both ways.
    #[test]
    fn large_real_and_netlist_files_stay_responsive() {
        let ibex = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/ysyx_ibex.sv"
        ))
        .expect("examples/ysyx_ibex.sv");
        let mut netlist = String::from("module top_netlist (input clk);\n");
        for i in 0..20_000 {
            netlist.push_str(&format!(
                "  LUT6 #(.INIT(64'h{:016X})) u_lut_{i} (.O(n{i}), .I0(n{}), .I1(n{}), .I2(n{}), .I3(n{}), .I4(n{}), .I5(n{})); // {}\n",
                (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
                i + 1, i + 2, i + 3, i + 4, i + 5, i + 6,
                "src/very/long/hierarchy/path/".repeat(4)
            ));
        }
        netlist.push_str("endmodule");
        for (name, text) in [("ysyx_ibex.sv", ibex), ("netlist.v", netlist)] {
            let mut s = init();
            s.center = CenterPane::Editor;
            crate::views::editor::sync_edit(&mut s);
            s.editor.lines = text.lines().map(str::to_string).collect();
            s.editor.edit = TextEdit::new(text);
            assert!(is_large(&s), "{name}");
            let started = std::time::Instant::now();
            let mut rt = rt(s);
            let first = started.elapsed();
            assert!(rt.elements().len() < 4_000, "{name}: {}", rt.elements().len());
            let view = rt.presented("editor-scroll").unwrap().0;
            let (cx, cy) = (view[0] + view[2] * 0.5, view[1] + view[3] * 0.5);
            // Worst frame per axis: a vertical page builds fresh rows, a
            // sideways step only moves the ones already built.
            let mut worst = [std::time::Duration::ZERO; 2];
            let mut t = 1.0;
            for k in 0..40 {
                let axis = (k / 4) % 2;
                if axis == 0 {
                    rt.wheel_xy(cx, cy, 0.0, -400.0);
                } else {
                    rt.wheel_xy(cx, cy, -80.0, 0.0);
                }
                let f = std::time::Instant::now();
                rt.frame(t);
                t += 0.2;
                worst[axis] = worst[axis].max(f.elapsed());
            }
            assert!(rt.elements().len() < 4_000, "{name}: {}", rt.elements().len());
            eprintln!(
                "{name}: first frame {first:?}, worst vertical {:?}, worst sideways {:?}",
                worst[0], worst[1]
            );
            let worst = worst[0].max(worst[1]);
            // Debug build on a shared machine: a generous bound that still
            // catches building or shaping every line per frame.
            assert!(worst < std::time::Duration::from_secs(3), "{name}: {worst:?}");
        }
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
        // line_of agrees with the byte ranges, newline bytes included.
        for (i, _) in lines.iter().enumerate() {
            let r = idx.range(i);
            assert_eq!(idx.line_of(r.start), i);
            assert_eq!(idx.line_of(r.end), i, "the newline belongs to its line");
        }
    }
}
