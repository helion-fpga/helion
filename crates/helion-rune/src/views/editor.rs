//! Verilog editor pane: file tabs + path breadcrumb + toolbar + gutter + code.
//!
//! New Rune `El` code written from scratch for the Vivado brief
//! (`/tmp/vivado-ref/rd-f58667ba*.png`, `rd-29675bf9*.png`): the center editor
//! with its `project1_demo.v` / `project1.xdc` tabs, full-path breadcrumb,
//! icon toolbar (find, save, undo/redo, cut/copy, `//` comment toggle), line
//! numbers, keyword coloring (magenta-ish), gray-italic comments, and the
//! pale-yellow current-line wash. All colors come from [`VivadoTheme`]; rows
//! use framework layout only (no manual gutter geometry, no hardcoded hex, no
//! unwraps on UI state).
//!
//! Data shapes mirror `helion-gui` read-only: [`EditorState`] (tabs, active
//! tab, lines, cursor line) is the small editor slice of [`AppState`].
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el.rs`: `row()/col()/el()/text()`, `Align::End`.
//! - `crates/rune/src/el/text.rs`: `.truncate_middle()` for the breadcrumb.
//! - `crates/rune/src/text/span.rs`: `Span::new(s,e).color().italic()` over
//!   UTF-8 byte ranges (re-exported as `rune::Span`).
//! - `crates/rune/src/tok.rs`: `TextStyle::mono()` for the code face.
//! - `crates/rune-kit/src/glyphs.rs`: `SEARCH`, `COPY`, `CLOSE`,
//!   `CHEVRON_UP/DOWN`.
//! - `crates/rune-kit/src/text_input.rs`: `TextInput::new/placeholder/on_edit/
//!   build` for the find bar query field.

use rune::{El, Key, Shape, Span, TextEdit, Tx, col, el, icon, row, text, text_input};
use rune_kit::{TextInput, glyphs};

use crate::state::{AppState, ConsoleLine, EditorState, MsgSev, ToastKind};
use crate::theme::VivadoTheme;

/// Tab strip height.
const TAB_H: f32 = 26.0;
/// Breadcrumb row height.
const PATH_H: f32 = 20.0;
/// Editor toolbar height.
const TOOLBAR_H: f32 = 28.0;
/// Code line height (compact CAD density).
const LINE_H: f32 = 18.0;
/// Gutter width: fits 4-digit line numbers in the 12px mono face.
const GUTTER_W: f32 = 44.0;
/// Directory shown in the breadcrumb (matches the `rd-*.png` brief).
const EDITOR_DIR: &str = "C:/EE214/project_1/project_1.srcs/sources_1/new";

/// Verilog keywords painted in the theme keyword color.
const VERILOG_KEYWORDS: &[&str] = &[
    "module",
    "endmodule",
    "input",
    "output",
    "inout",
    "reg",
    "wire",
    "always",
    "begin",
    "end",
    "if",
    "else",
    "posedge",
    "negedge",
    "parameter",
    "localparam",
    "assign",
    "initial",
    "forever",
    "for",
    "while",
    "case",
    "endcase",
    "default",
    "function",
    "endfunction",
    "task",
    "endtask",
    "generate",
    "endgenerate",
];

/// XDC/Tcl command words painted as keywords on `.xdc` tabs.
const XDC_KEYWORDS: &[&str] = &[
    "set_property",
    "create_clock",
    "get_ports",
    "get_cells",
    "get_nets",
    "set_input_delay",
    "set_output_delay",
    "current_design",
];

fn is_word_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn is_number_char(b: u8) -> bool {
    matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' | b'x' | b'X' | b'z' | b'Z' | b'_' | b'\'')
}

/// Byte-range syntax spans for one code line.
///
/// `hash_comments` selects the `.xdc` dialect (`#` comments, Tcl keywords);
/// otherwise Verilog (`//` comments, backtick directives, keywords). All
/// boundaries sit on ASCII bytes, so multi-byte text can never split a span.
pub fn highlight_spans(line: &str, hash_comments: bool, t: &VivadoTheme) -> Vec<Span> {
    let keywords = if hash_comments {
        XDC_KEYWORDS
    } else {
        VERILOG_KEYWORDS
    };
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // Line comments run to end of line.
        if !hash_comments && b == b'/' && bytes.get(i + 1) == Some(&b'/') {
            spans.push(Span::new(i, bytes.len()).color(t.code_comment).italic());
            break;
        }
        if hash_comments && b == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            spans.push(Span::new(i, bytes.len()).color(t.code_comment).italic());
            break;
        }
        // Verilog compiler directives: `` `timescale `` etc.
        if !hash_comments && b == b'`' {
            let mut j = i + 1;
            while j < bytes.len() && is_word_char(bytes[j]) {
                j += 1;
            }
            if j > i + 1 {
                spans.push(Span::new(i, j).color(t.code_keyword));
                i = j;
                continue;
            }
            i += 1;
            continue;
        }
        // Keywords and identifiers.
        if is_word_start(b) {
            let mut j = i + 1;
            while j < bytes.len() && is_word_char(bytes[j]) {
                j += 1;
            }
            if keywords.contains(&&line[i..j]) {
                spans.push(Span::new(i, j).color(t.code_keyword));
            }
            i = j;
            continue;
        }
        // Numeric literals: sized (`24'd499999`) or plain, never identifiers
        // that merely contain digits (must start with a digit or quote one).
        if b.is_ascii_digit() {
            let mut j = i + 1;
            while j < bytes.len() && is_number_char(bytes[j]) {
                j += 1;
            }
            spans.push(Span::new(i, j).color(t.code_number));
            i = j;
            continue;
        }
        i += 1;
    }
    spans
}

/// Echo a Tcl verb to the console without flipping run state.
fn echo(s: &mut AppState, tcl: &str) {
    s.console.lines.push(ConsoleLine {
        text: tcl.into(),
        sev: MsgSev::Info,
    });
}

/// Which buffer the active tab shows. Typed so routing is never a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BufferKind {
    Rtl,
    Xdc,
    Tb,
}

/// Route the active tab to its buffer: `.xdc` tabs show the constraints
/// buffer, the testbench tab shows the TB buffer, everything else shows the
/// Verilog buffer.
fn active_buffer(s: &AppState) -> BufferKind {
    if s.editor.active_is_tb() {
        BufferKind::Tb
    } else if active_is_xdc(s) {
        BufferKind::Xdc
    } else {
        BufferKind::Rtl
    }
}

/// Mutable lines of the active buffer.
fn active_lines_mut(s: &mut AppState) -> &mut Vec<String> {
    match active_buffer(s) {
        BufferKind::Xdc => &mut s.editor.xdc_lines,
        BufferKind::Tb => &mut s.editor.tb_lines,
        BufferKind::Rtl => &mut s.editor.lines,
    }
}

/// Route an arbitrary tab index to its buffer (tab switches flush the old
/// tab's text before the active index moves, so routing never uses it).
fn buffer_kind_for_tab(s: &AppState, i: usize) -> BufferKind {
    match s.editor.tabs.get(i).map(|t| t.as_str()) {
        Some("project1_tb.v") => BufferKind::Tb,
        Some(name) if name.ends_with(".xdc") => BufferKind::Xdc,
        _ => BufferKind::Rtl,
    }
}

/// Write the live field text back into the buffer it mirrors. Every action
/// that reads line buffers or moves off a tab calls this first so typing is
/// never stranded in the field.
pub fn flush_edit(s: &mut AppState) {
    let Some(i) = s.editor.edit_tab else { return };
    if s.editor.tabs.get(i).is_none() {
        s.editor.edit_tab = None;
        return;
    }
    let fresh: Vec<String> = s.editor.edit.text.lines().map(str::to_string).collect();
    let fresh = if fresh.is_empty() {
        vec![String::new()]
    } else {
        fresh
    };
    match buffer_kind_for_tab(s, i) {
        BufferKind::Xdc => s.editor.xdc_lines = fresh,
        BufferKind::Tb => s.editor.tb_lines = fresh,
        BufferKind::Rtl => s.editor.lines = fresh,
    }
}

/// Activate tab `i`: clamp the cursor onto the buffer and mirror it into
/// the live field. Shared by tab taps and the command palette's file rows.
pub fn select_tab(s: &mut AppState, i: usize) {
    if i >= s.editor.tabs.len() {
        return;
    }
    s.editor.active = i;
    s.editor.cursor_line = cursor(&s.editor);
    sync_edit(s);
}

/// Mirror the active tab's buffer into the live field, flushing whatever the
/// field held before. A no-op when the field already mirrors the tab.
pub fn sync_edit(s: &mut AppState) {
    if s.editor.tabs.is_empty() {
        s.editor.edit = TextEdit::new("");
        s.editor.edit_tab = None;
        return;
    }
    s.editor.active = s.editor.active.min(s.editor.tabs.len() - 1);
    if s.editor.edit_tab == Some(s.editor.active) {
        return;
    }
    flush_edit(s);
    let text = active_lines(s).join("\n");
    let at = s.editor.cursor_line;
    s.editor.edit = TextEdit::new(text);
    s.editor.edit_tab = Some(s.editor.active);
    let line = at.min(line_count(s).saturating_sub(1));
    let byte = s.editor.edit.line_range(line).start;
    s.editor.edit.set_caret(byte);
    s.editor.cursor_line = line;
}

/// Reload the field from the active buffer after an action rewrote the lines
/// (undo/redo/comment/reload/open), keeping the caret where it was.
fn reload_edit(s: &mut AppState) {
    if s.editor.tabs.is_empty() {
        s.editor.edit = TextEdit::new("");
        s.editor.edit_tab = None;
        return;
    }
    let caret = s.editor.edit.focus.min(s.editor.edit.text.len());
    let text = active_lines(s).join("\n");
    s.editor.edit = TextEdit::new(text);
    s.editor.edit_tab = Some(s.editor.active);
    s.editor.edit.set_caret(caret.min(s.editor.edit.text.len()));
    s.editor.cursor_line = caret_line(s);
}

/// Caret's line in the live field (0 when the field is empty).
fn caret_line(s: &AppState) -> usize {
    s.editor.edit.line_of_byte(s.editor.edit.focus)
}

/// Toolbar/menu undo: flush typing, restore the snapshot, reload the field.
pub fn undo_synced(s: &mut AppState) -> bool {
    flush_edit(s);
    if s.editor.undo_active(active_is_xdc(s)) {
        reload_edit(s);
        true
    } else {
        false
    }
}

/// Toolbar/menu redo: flush typing, restore the snapshot, reload the field.
pub fn redo_synced(s: &mut AppState) -> bool {
    flush_edit(s);
    if s.editor.redo_active(active_is_xdc(s)) {
        reload_edit(s);
        true
    } else {
        false
    }
}

/// Logical line count of the live field (an empty field still shows line 1).
fn line_count(s: &AppState) -> usize {
    s.editor.edit.text.lines().count().max(1)
}

/// Default disk path for a tab saved for the first time: the temp dir keeps
/// demo saves real without touching the fake `C:/...` project paths.
fn default_save_path(tab: &str) -> Result<std::path::PathBuf, String> {
    let file = std::path::PathBuf::from(tab)
        .file_name()
        .map(|n| n.to_owned())
        .ok_or_else(|| format!("cannot save {tab:?}: bad file name"))?;
    Ok(std::env::temp_dir().join("helion-rune-files").join(file))
}

/// Save the active buffer to its disk path (recording the path on the first
/// save) and clear the dirty flag. Failures leave the buffer dirty and
/// report the real I/O error; nothing is ever a fake success.
pub fn save_active(s: &mut AppState) -> Result<std::path::PathBuf, String> {
    flush_edit(s);
    let tab = s
        .editor
        .tabs
        .get(s.editor.active)
        .cloned()
        .ok_or_else(|| "no open file".to_string())?;
    let path = match s.editor.paths.get(&tab) {
        Some(p) => p.clone(),
        None => default_save_path(&tab)?,
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    std::fs::write(&path, active_lines(s).join("\n"))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    s.editor.paths.insert(tab, path.clone());
    s.editor.mark_saved();
    Ok(path)
}

/// Reload the active buffer from its disk path, replacing the buffer
/// (undoable, and clean afterwards since it matches the disk). Errors when
/// the tab was never saved or the file cannot be read.
pub fn reload_active(s: &mut AppState) -> Result<(), String> {
    let tab = s
        .editor
        .tabs
        .get(s.editor.active)
        .cloned()
        .ok_or_else(|| "no open file".to_string())?;
    let path = s
        .editor
        .paths
        .get(&tab)
        .cloned()
        .ok_or_else(|| format!("{tab} has never been saved"))?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let fresh: Vec<String> = text.lines().map(str::to_string).collect();
    flush_edit(s);
    s.editor.push_undo_active(active_is_xdc(s));
    *active_lines_mut(s) = fresh;
    s.editor.cursor_line = cursor(&s.editor);
    reload_edit(s);
    s.editor.mark_saved();
    Ok(())
}

/// Open a natively picked file as an editor tab: add/activate the tab, load
/// its real contents into the routed buffer (undoable, like a reload),
/// record its disk path so Save writes back to it, and park the cursor.
pub fn open_file_tab(s: &mut AppState, name: String, text: String, path: std::path::PathBuf) {
    flush_edit(s);
    if let Some(i) = s.editor.tabs.iter().position(|t| *t == name) {
        s.editor.active = i;
    } else {
        s.editor.tabs.push(name.clone());
        s.editor.active = s.editor.tabs.len() - 1;
    }
    let fresh: Vec<String> = text.lines().map(str::to_string).collect();
    s.editor.push_undo_active(active_is_xdc(s));
    *active_lines_mut(s) = fresh;
    s.editor.paths.insert(name, path);
    s.editor.cursor_line = cursor(&s.editor);
    // Force a load of the new buffer (the active index may be unchanged).
    s.editor.edit_tab = None;
    sync_edit(s);
    s.editor.mark_saved();
}

/// Clamp the cursor onto a real line (empty buffer parks on line 0).
fn cursor(ed: &EditorState) -> usize {
    ed.cursor_line.min(ed.lines.len().saturating_sub(1))
}

/// Toggle the line-comment prefix on the cursor line (`//` for Verilog,
/// `#` for XDC), preserving leading indentation. Acts on the active buffer.
fn toggle_comment(s: &mut AppState) {
    let kind = active_buffer(s);
    let marker = if kind == BufferKind::Xdc { "#" } else { "//" };
    flush_edit(s);
    let len = active_lines(s).len();
    if len == 0 {
        return;
    }
    // Comment the caret's line (typing moves the caret, not the stored row).
    let at = caret_line(s).min(len - 1);
    s.editor.push_undo_active(active_is_xdc(s));
    let buf = active_lines_mut(s);
    if let Some(line) = buf.get_mut(at) {
        let indent = line.len() - line.trim_start().len();
        let body = line[indent..].to_string();
        let toggled = if let Some(rest) = body.strip_prefix(marker) {
            format!(
                "{}{}",
                &line[..indent],
                rest.strip_prefix(' ').unwrap_or(rest)
            )
        } else {
            format!("{}{marker} {body}", &line[..indent])
        };
        *line = toggled;
    }
    s.editor.cursor_line = at;
    reload_edit(s);
}

/// Whether the active tab is an XDC file (never indexes out of bounds).
pub(crate) fn active_is_xdc(s: &AppState) -> bool {
    s.editor
        .tabs
        .get(s.editor.active)
        .is_some_and(|t| t.ends_with(".xdc"))
}

/// Lines of the active buffer (Verilog, XDC constraints, or testbench).
fn active_lines(s: &AppState) -> &[String] {
    match active_buffer(s) {
        BufferKind::Xdc => &s.editor.xdc_lines,
        BufferKind::Tb => &s.editor.tb_lines,
        BufferKind::Rtl => &s.editor.lines,
    }
}

/// Line indices containing `query` (case-sensitive; blank query matches nothing).
fn find_matches(lines: &[String], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains(query))
        .map(|(n, _)| n)
        .collect()
}

/// Move the caret to the next (or previous) find match, wrapping at the ends.
fn find_step(s: &mut AppState, forward: bool) {
    sync_edit(s);
    let query = s.editor.find_query.text.clone();
    let lines: Vec<String> = s.editor.edit.text.lines().map(str::to_string).collect();
    let hits = find_matches(&lines, &query);
    if hits.is_empty() {
        return;
    }
    let at = caret_line(s);
    let line = if forward {
        hits.iter().copied().find(|h| *h > at).unwrap_or(hits[0])
    } else {
        hits.iter()
            .copied()
            .rev()
            .find(|h| *h < at)
            .unwrap_or(hits[hits.len() - 1])
    };
    // Park the caret on the match start so the viewport follows it.
    let byte = s.editor.edit.line_range(line).start;
    let off = lines[line].find(&query).unwrap_or(0);
    s.editor.edit.set_caret(byte + off);
    s.editor.cursor_line = line;
}

/// Close tab `i`, keeping the active tab and cursor on real lines.
fn close_tab(s: &mut AppState, i: usize) {
    if i >= s.editor.tabs.len() {
        return;
    }
    flush_edit(s);
    s.editor.tabs.remove(i);
    s.editor.edit_tab = None;
    s.editor.active = s.editor.active.min(s.editor.tabs.len().saturating_sub(1));
    s.editor.cursor_line = cursor(&s.editor);
    sync_edit(s);
}

/// One editor toolbar button: 24px cell with a glyph or `//` mark.
fn tool_btn(
    key: String,
    tip: &'static str,
    glyph: El,
    on_press: impl Fn(&mut AppState) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .label(tip)
        .focusable(true)
        .size(24.0, 24.0)
        .center()
        .on_press(on_press)
        .child(glyph)
}

/// Glyph for an editor tool, drawn from theme ink (see `chrome::tool_glyph`).
fn tool_glyph(kind: EditorTool, t: &VivadoTheme) -> El {
    let ink = t.text;
    match kind {
        EditorTool::Find => icon(&glyphs::SEARCH, 13.0).color(t.text_dim).a11y_hidden(),
        EditorTool::Copy => icon(&glyphs::COPY, 13.0).color(t.text_dim).a11y_hidden(),
        EditorTool::Save => rune::canvas(
            Shape::new()
                .stroke(
                    vec![
                        [3.0, 3.0],
                        [11.0, 3.0],
                        [13.0, 5.0],
                        [13.0, 13.0],
                        [3.0, 13.0],
                        [3.0, 3.0],
                    ],
                    1.4,
                    ink,
                )
                .fill(vec![[5.5, 3.0], [10.5, 3.0], [10.5, 7.0], [5.5, 7.0]], ink)
                .view_box(16.0, 16.0),
        )
        .size(16.0, 16.0)
        .a11y_hidden(),
        EditorTool::Undo | EditorTool::Redo => {
            let head: [[f32; 2]; 3] = match kind {
                EditorTool::Undo => [[4.0, 8.0], [7.5, 5.0], [7.5, 11.0]],
                _ => [[12.0, 8.0], [8.5, 5.0], [8.5, 11.0]],
            };
            rune::canvas(
                Shape::new()
                    .stroke(vec![[4.5, 8.0], [11.5, 8.0]], 1.6, ink)
                    .fill(head.to_vec(), ink)
                    .view_box(16.0, 16.0),
            )
            .size(16.0, 16.0)
            .a11y_hidden()
        }
        EditorTool::Cut => rune::canvas(
            Shape::new()
                .stroke(vec![[4.0, 4.0], [12.0, 12.0]], 1.6, ink)
                .stroke(vec![[12.0, 4.0], [4.0, 12.0]], 1.6, ink)
                .view_box(16.0, 16.0),
        )
        .size(16.0, 16.0)
        .a11y_hidden(),
        EditorTool::Comment => text("//")
            .font(VivadoTheme::FONT_MONO.mono())
            .color(t.text_dim)
            .a11y_hidden(),
    }
}

#[derive(Clone, Copy)]
enum EditorTool {
    Find,
    Save,
    Undo,
    Redo,
    Cut,
    Copy,
    Comment,
}

const EDITOR_TOOLS: &[(EditorTool, &str, &'static str)] = &[
    (EditorTool::Find, "editor-tool-find", "Find in file"),
    (EditorTool::Save, "editor-tool-save", "Save file"),
    (EditorTool::Undo, "editor-tool-undo", "Undo"),
    (EditorTool::Redo, "editor-tool-redo", "Redo"),
    (EditorTool::Cut, "editor-tool-cut", "Cut"),
    (EditorTool::Copy, "editor-tool-copy", "Copy"),
    (
        EditorTool::Comment,
        "editor-tool-comment",
        "Toggle line comment",
    ),
];

/// Editor icon toolbar: find/save/undo/redo/cut/copy/comment, as in the brief.
fn editor_toolbar(t: &VivadoTheme) -> El {
    let mut bar = row()
        .key("editor-toolbar")
        .role("toolbar")
        .w_full()
        .h(TOOLBAR_H)
        .items_center()
        .gap(2.0)
        .px(8.0)
        .fill(t.header_bg)
        .border(1.0, t.border_light);
    for (i, (kind, key, tip)) in EDITOR_TOOLS.iter().enumerate() {
        if i == 2 || i == 4 || i == 6 {
            bar = bar.child(el().w(1.0).h(18.0).mx(2.0).fill(t.border).a11y_hidden());
        }
        let glyph = tool_glyph(*kind, t);
        let tcl: &'static str = match kind {
            EditorTool::Find => "find_text",
            EditorTool::Save => "save_file",
            EditorTool::Undo => "undo",
            EditorTool::Redo => "redo",
            EditorTool::Cut => "cut",
            EditorTool::Copy => "copy",
            EditorTool::Comment => "toggle_comment",
        };
        if matches!(kind, EditorTool::Comment) {
            bar = bar.child(tool_btn((*key).into(), tip, glyph, |s| {
                toggle_comment(s);
                echo(s, "toggle_comment");
            }));
        } else if matches!(kind, EditorTool::Save) {
            bar = bar.child(tool_btn((*key).into(), tip, glyph, move |s| {
                let _ = tcl;
                match save_active(s) {
                    Ok(path) => echo(s, &format!("save_file {}", path.display())),
                    Err(e) => {
                        s.console.lines.push(ConsoleLine {
                            text: format!("ERROR: save_file: {e}"),
                            sev: MsgSev::Error,
                        });
                        s.toasts.push(ToastKind::Error, "Save failed", &e);
                    }
                }
            }));
        } else if matches!(kind, EditorTool::Undo) {
            bar = bar.child(tool_btn((*key).into(), tip, glyph, move |s| {
                if undo_synced(s) {
                    echo(s, tcl);
                }
            }));
        } else if matches!(kind, EditorTool::Redo) {
            bar = bar.child(tool_btn((*key).into(), tip, glyph, move |s| {
                if redo_synced(s) {
                    echo(s, tcl);
                }
            }));
        } else if matches!(kind, EditorTool::Find) {
            bar = bar.child(tool_btn((*key).into(), tip, glyph, |s| {
                s.editor.find_open = !s.editor.find_open;
                if s.editor.find_open {
                    echo(s, "find_text");
                }
            }));
        } else {
            bar = bar.child(tool_btn((*key).into(), tip, glyph, move |s| echo(s, tcl)));
        }
    }
    bar
}

/// Find bar under the toolbar: query field, live match count, prev/next
/// steppers that move the cursor through the active buffer, and close.
fn find_bar(s: &AppState, t: &VivadoTheme) -> El {
    let kit = t.kit();
    let query = s.editor.find_query.text.clone();
    let live: Vec<String> = s.editor.edit.text.lines().map(str::to_string).collect();
    let hits = find_matches(&live, &query);
    let count = if query.is_empty() {
        "Find in file".to_string()
    } else {
        match hits.len() {
            0 => "No matches".to_string(),
            1 => "1 match".to_string(),
            n => format!("{n} matches"),
        }
    };
    row()
        .key("editor-find")
        .w_full()
        .h(TOOLBAR_H)
        .items_center()
        .gap(4.0)
        .px(8.0)
        .fill(t.panel_bg)
        .border(1.0, t.border_light)
        .child(
            icon(&glyphs::SEARCH, 12.0)
                .color(t.text_faint)
                .a11y_hidden(),
        )
        .child(
            TextInput::new(&s.editor.find_query)
                .placeholder("Find")
                .on_edit(|s: &mut AppState| &mut s.editor.find_query)
                .build(&kit)
                .key("editor-find-input")
                .w(220.0),
        )
        .child(
            text(count)
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .key("editor-find-count"),
        )
        .child(tool_btn(
            "editor-find-prev".into(),
            "Previous match",
            icon(&glyphs::CHEVRON_UP, 12.0)
                .color(t.text_dim)
                .a11y_hidden(),
            |s| find_step(s, false),
        ))
        .child(tool_btn(
            "editor-find-next".into(),
            "Next match",
            icon(&glyphs::CHEVRON_DOWN, 12.0)
                .color(t.text_dim)
                .a11y_hidden(),
            |s| find_step(s, true),
        ))
        .child(tool_btn(
            "editor-find-close".into(),
            "Close find",
            icon(&glyphs::CLOSE, 10.0).color(t.text_dim).a11y_hidden(),
            |s| s.editor.find_open = false,
        ))
}

/// File tab strip: active tab bold on panel white, inactive on header gray.
fn tab_strip(s: &AppState, t: &VivadoTheme) -> El {
    let mut strip = row()
        .key("editor-tabs")
        .w_full()
        .h(TAB_H)
        .items_center()
        .gap(2.0)
        .px(4.0)
        .fill(t.header_bg)
        .border(1.0, t.border_light);
    for (i, name) in s.editor.tabs.iter().enumerate() {
        let active = i == s.editor.active;
        let style = if active {
            VivadoTheme::FONT_TITLE
        } else {
            VivadoTheme::FONT_UI
        };
        let bg = if active { t.panel_bg } else { t.header_bg };
        let shown = if active && s.editor.dirty {
            format!("• {name}")
        } else {
            name.clone()
        };
        strip = strip.child(
            row()
                .key(format!("editor-tab-{i}"))
                .role("button")
                .label(name.clone())
                .focusable(true)
                .h(TAB_H - 4.0)
                .items_center()
                .gap(6.0)
                .px(8.0)
                .fill(bg)
                .hover(t.selection)
                .on_press(move |s: &mut AppState| {
                    select_tab(s, i);
                })
                .child(text(shown).font(style).color(t.text))
                .child(
                    el().key(format!("editor-close-{i}"))
                        .role("button")
                        .label(format!("Close {name}"))
                        .focusable(true)
                        .size(16.0, 16.0)
                        .center()
                        .hover(t.selection)
                        .on_press(move |s: &mut AppState| close_tab(s, i))
                        .child(icon(&glyphs::CLOSE, 10.0).color(t.text_dim).a11y_hidden()),
                ),
        );
    }
    strip
}

/// Full-path breadcrumb under the tabs, middle-truncated like Vivado.
fn breadcrumb(s: &AppState, t: &VivadoTheme) -> El {
    let path = s
        .editor
        .tabs
        .get(s.editor.active)
        .map(|name| format!("{EDITOR_DIR}/{name}"))
        .unwrap_or_else(|| "(no open file)".into());
    row()
        .key("editor-path")
        .w_full()
        .h(PATH_H)
        .items_center()
        .px(8.0)
        .fill(t.panel_bg)
        .child(
            text(path)
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .grow(1.0)
                .shrink()
                .truncate_middle(),
        )
}

/// One code line: right-aligned gutter number plus highlighted mono code.
/// The cursor line gets the pale-yellow [`VivadoTheme::current_line`] wash;
/// pressing a line moves the cursor there.
/// Gutter row: right-aligned line number, bright on the caret line; click
/// parks the caret at the line start.
fn gutter_row(n: usize, current: bool, t: &VivadoTheme) -> El {
    row()
        .key(format!("editor-gutter-{n}"))
        .role("button")
        .label(format!("Line {}", n + 1))
        .focusable(true)
        .w(GUTTER_W)
        .h(LINE_H)
        .items_center()
        .justify_end()
        .pr(8.0)
        .hover(t.selection)
        .on_press(move |s: &mut AppState| {
            sync_edit(s);
            let line = n.min(line_count(s).saturating_sub(1));
            let byte = s.editor.edit.line_range(line).start;
            s.editor.edit.set_caret(byte);
            s.editor.cursor_line = line;
        })
        .child(
            text(format!("{}", n + 1))
                .font(VivadoTheme::FONT_MONO.mono().lh(LINE_H))
                .color(if current { t.text } else { t.text_faint }),
        )
}

/// Whole-buffer spans for the live field: syntax colors plus a wash over
/// every find match. Byte offsets are relative to the field text.
fn field_spans(s: &AppState, is_xdc: bool, t: &VivadoTheme) -> Vec<Span> {
    let text = &s.editor.edit.text;
    let mut out = Vec::new();
    let mut start = 0usize;
    for line in text.split('\n') {
        for mut sp in highlight_spans(line, is_xdc, t) {
            sp.start += start as u32;
            sp.end += start as u32;
            out.push(sp);
        }
        start += line.len() + 1;
    }
    let q = &s.editor.find_query.text;
    if !q.is_empty() {
        for (i, _) in text.match_indices(q) {
            out.push(Span::new(i, i + q.len()).bg(t.selection, 1.0));
        }
    }
    out
}

/// The center editor pane wired into the shell.
pub fn editor_pane(s: &AppState, t: &VivadoTheme) -> El {
    let mut pane = col()
        .key("editor")
        .grow(1.0)
        .h_full()
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(tab_strip(s, t))
        .child(breadcrumb(s, t))
        .child(editor_toolbar(t));
    if s.editor.find_open && !s.editor.tabs.is_empty() {
        pane = pane.child(find_bar(s, t));
    }
    if s.editor.tabs.is_empty() {
        return pane.child(
            col().w_full().grow(1.0).center().child(
                text("No open files")
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_faint)
                    .key("editor-empty"),
            ),
        );
    }
    let is_xdc = active_buffer(s) == BufferKind::Xdc;
    let mono = VivadoTheme::FONT_MONO.mono().lh(LINE_H);
    let count = line_count(s);
    let caret = caret_line(s);
    let mut gutter = col().w(GUTTER_W);
    for n in 0..count {
        gutter = gutter.child(gutter_row(n, n == caret, t));
    }
    let field: &TextEdit = &s.editor.edit;
    // One scroll container over gutter + field (the OG's single-ScrollArea
    // architecture): both ride the same offsets, so rows stay aligned.
    let code = col()
        .key("editor-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll_xy()
        .child(
            row().py(4.0).child(gutter).child(
                text_input(field, mono)
                    .key("editor-field")
                    .label("Source editor")
                    .color(t.text)
                    .spans(field_spans(s, is_xdc, t))
                    .caret(t.text, t.selection)
                    .on_edit(|s: &mut AppState| &mut s.editor.edit)
                    .on_submit(|s: &mut AppState, _tx: &mut Tx| {
                        s.editor.edit.insert("\n");
                        s.editor.dirty = true;
                    })
                    .on_input_key(
                        |s: &mut AppState, key: &rune::KeyEvent, _tx: &mut Tx| match &key.key {
                            Key::Text(ch) if key.mods.cmd && (ch == "s" || ch == "S") => {
                                let _ = save_active(s);
                                s.editor.cursor_line = caret_line(s);
                                true
                            }
                            Key::Text(_) | Key::Enter | Key::Backspace | Key::Delete | Key::Tab => {
                                s.editor.dirty = true;
                                false
                            }
                            _ => false,
                        },
                    )
                    .on_input_paste(|s: &mut AppState, _, _| {
                        s.editor.dirty = true;
                        false
                    }),
            ),
        );
    pane.child(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CenterPane;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn harness_with(state: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(state, crate::view, spec.size, env)
    }

    fn editor_state() -> AppState {
        let mut s = init();
        s.center = CenterPane::Editor;
        s
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    /// Rendered text of a keyed element (`elements.rs`: `ElementInfo::text`).
    fn shown_text(rt: &Runtime<AppState>, key: &str) -> String {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .and_then(|e| e.text)
            .unwrap_or_default()
    }

    #[test]
    fn editor_tree_exposes_tabs_toolbar_gutter_and_code() {
        let mut rt = harness_with(editor_state());
        rt.frame(0.0);
        for key in [
            "editor",
            "editor-tabs",
            "editor-tab-0",
            "editor-tab-1",
            "editor-close-0",
            "editor-path",
            "editor-toolbar",
            "editor-tool-find",
            "editor-tool-save",
            "editor-tool-undo",
            "editor-tool-comment",
            "editor-scroll",
            "editor-field",
            "editor-gutter-0",
        ] {
            assert!(rt.find(key).is_some(), "missing editor key: {key}");
        }
    }

    #[test]
    fn keywords_directives_numbers_and_comments_get_spans() {
        let t = VivadoTheme::light();
        let spans = highlight_spans("module project1_demo(", false, &t);
        assert_eq!(spans.len(), 1, "only `module` is a keyword");
        assert_eq!((spans[0].start, spans[0].end), (0, 6));
        assert_eq!(spans[0].color, Some(t.code_keyword));

        let spans = highlight_spans("`timescale 1ns / 1ps", false, &t);
        assert!(
            spans
                .iter()
                .any(|s| (s.start, s.end) == (0, 10) && s.color == Some(t.code_keyword))
        );
        assert!(spans.iter().any(|s| s.color == Some(t.code_number)));

        let spans = highlight_spans("reg [23:0] counter = 24'd0;", false, &t);
        assert!(spans.iter().any(|s| (s.start, s.end) == (0, 3)));
        assert!(spans.iter().any(|s| s.color == Some(t.code_number)));
        // `counter` merely contains digits-adjacent letters: never a number.
        assert!(
            !spans
                .iter()
                .any(|s| (s.start, s.end) == (11, 18) && s.color == Some(t.code_number))
        );

        let spans = highlight_spans("// Clock Divider: 100MHz", false, &t);
        assert_eq!(spans.len(), 1);
        assert_eq!((spans[0].start, spans[0].end), (0, 24));
        assert_eq!(spans[0].color, Some(t.code_comment));
        assert!(spans[0].italic);

        let spans = highlight_spans("set_property PACKAGE_PIN E3 [get_ports clk]", true, &t);
        assert!(
            spans
                .iter()
                .any(|s| (s.start, s.end) == (0, 12) && s.color == Some(t.code_keyword))
        );
        let spans = highlight_spans("# 100MHz clock", true, &t);
        assert_eq!(spans.len(), 1);
        assert!(spans[0].italic);
    }

    #[test]
    fn highlight_never_splits_multibyte_text() {
        let t = VivadoTheme::light();
        let line = "reg led_🔴 = 1'b0; // café";
        let spans = highlight_spans(line, false, &t);
        for s in &spans {
            assert!(s.start <= s.end && (s.end as usize) <= line.len());
            assert!(line.is_char_boundary(s.start as usize));
            assert!(line.is_char_boundary(s.end as usize));
        }
        assert!(spans.iter().any(|s| (s.start, s.end) == (0, 3)));
    }

    #[test]
    fn pressing_a_line_moves_the_cursor() {
        let mut rt = harness_with(editor_state());
        rt.frame(0.0);
        tap(&mut rt, "editor-gutter-5");
        assert_eq!(rt.state.editor.cursor_line, 5);
        assert_eq!(
            rt.state
                .editor
                .edit
                .line_of_byte(rt.state.editor.edit.focus),
            5
        );
    }

    #[test]
    fn tab_switch_selects_xdc_buffer_and_back() {
        let mut rt = harness_with(editor_state());
        rt.frame(0.0);
        tap(&mut rt, "editor-tab-1");
        assert_eq!(rt.state.editor.active, 1);
        assert_eq!(rt.state.editor.edit_tab, Some(1));
        assert_eq!(
            rt.state.editor.edit.text,
            rt.state.editor.xdc_lines.join("\n")
        );
        tap(&mut rt, "editor-tab-0");
        assert_eq!(rt.state.editor.active, 0);
        assert_eq!(rt.state.editor.edit_tab, Some(0));
        assert_eq!(rt.state.editor.edit.text, rt.state.editor.lines.join("\n"));
    }

    #[test]
    fn comment_toggle_prefixes_and_restores_the_cursor_line() {
        let mut rt = harness_with(editor_state());
        rt.frame(0.0);
        tap(&mut rt, "editor-gutter-3");
        let before = rt.state.editor.lines[3].clone();
        assert!(!before.trim_start().starts_with("//"));
        tap(&mut rt, "editor-tool-comment");
        assert!(rt.state.editor.lines[3].trim_start().starts_with("// "));
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "toggle_comment")
        );
        tap(&mut rt, "editor-tool-comment");
        assert_eq!(rt.state.editor.lines[3], before);
    }

    #[test]
    fn closing_every_tab_shows_the_empty_placeholder() {
        let mut rt = harness_with(editor_state());
        rt.frame(0.0);
        tap(&mut rt, "editor-close-0");
        assert_eq!(rt.state.editor.tabs.len(), 1);
        tap(&mut rt, "editor-close-0");
        assert!(rt.state.editor.tabs.is_empty());
        assert!(rt.find("editor-empty").is_some());
        assert!(rt.find("editor-field").is_none());
    }

    #[test]
    fn xdc_tab_renders_the_live_buffer_not_a_const() {
        let mut s = editor_state();
        s.editor.active = 1;
        s.editor.xdc_lines[1] = "create_clock -period 20.000 -name clk [get_ports clk]".into();
        sync_edit(&mut s);
        let mut rt = harness_with(s);
        rt.frame(0.0);
        let shown = shown_text(&rt, "editor-field");
        assert!(
            shown.contains("20.000"),
            "xdc tab must render the live buffer, got: {shown:?}"
        );
    }

    #[test]
    fn comment_toggle_acts_on_the_xdc_buffer_with_undo() {
        let mut rt = harness_with(editor_state());
        rt.frame(0.0);
        tap(&mut rt, "editor-tab-1");
        tap(&mut rt, "editor-gutter-2");
        let before = rt.state.editor.xdc_lines[2].clone();
        assert!(!before.trim_start().starts_with('#'));
        tap(&mut rt, "editor-tool-comment");
        assert!(
            rt.state.editor.xdc_lines[2].trim_start().starts_with("# "),
            "xdc toggle must prefix #: {}",
            rt.state.editor.xdc_lines[2]
        );
        tap(&mut rt, "editor-tool-undo");
        assert_eq!(
            rt.state.editor.xdc_lines[2], before,
            "undo must restore the xdc line"
        );
    }

    #[test]
    fn find_button_toggles_the_find_bar() {
        let mut rt = harness_with(editor_state());
        rt.frame(0.0);
        assert!(
            rt.find("editor-find").is_none(),
            "find bar starts unmounted"
        );
        tap(&mut rt, "editor-tool-find");
        assert!(rt.state.editor.find_open);
        for key in [
            "editor-find",
            "editor-find-input",
            "editor-find-count",
            "editor-find-next",
            "editor-find-close",
        ] {
            assert!(rt.find(key).is_some(), "missing find key: {key}");
        }
        tap(&mut rt, "editor-tool-find");
        assert!(!rt.state.editor.find_open);
        assert!(
            rt.find("editor-find").is_none(),
            "find bar must unmount on toggle"
        );
    }

    #[test]
    fn find_count_labels_matches_and_next_steps_through_them() {
        let mut s = editor_state();
        s.editor.find_open = true;
        s.editor.find_query = rune::TextEdit::new("divclk");
        s.editor.cursor_line = 0;
        s.editor.edit.set_caret(0);
        let mut rt = harness_with(s);
        rt.frame(0.0);
        let shown = shown_text(&rt, "editor-find-count");
        assert!(
            shown.contains("4 matches"),
            "count must label the hits, got: {shown:?}"
        );
        tap(&mut rt, "editor-find-next");
        assert_eq!(
            rt.state.editor.cursor_line, 10,
            "next must land on the first divclk line"
        );
        tap(&mut rt, "editor-find-next");
        assert_eq!(rt.state.editor.cursor_line, 19);
        tap(&mut rt, "editor-find-prev");
        assert_eq!(rt.state.editor.cursor_line, 10);
    }

    #[test]
    fn find_close_button_unmounts_the_bar() {
        let mut s = editor_state();
        s.editor.find_open = true;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("editor-find").is_some());
        tap(&mut rt, "editor-find-close");
        assert!(!rt.state.editor.find_open);
        assert!(rt.find("editor-find").is_none());
    }

    #[test]
    fn sources_tb_file_opens_with_its_own_buffer() {
        let mut rt = harness_with(editor_state());
        rt.frame(0.0);
        tap(&mut rt, "src-sim-set");
        tap(&mut rt, "src-tb");
        assert!(
            rt.state.editor.tabs.iter().any(|t| t == "project1_tb.v"),
            "tb must open a tab"
        );
        let line2 = rt
            .state
            .editor
            .edit
            .text
            .lines()
            .nth(2)
            .unwrap_or("")
            .to_string();
        assert_eq!(
            line2, "module project1_tb;",
            "tb tab must show the tb buffer, not the demo buffer"
        );
    }

    #[test]
    fn tb_buffer_save_writes_disk_and_reload_restores() {
        let mut rt = harness_with(editor_state());
        rt.frame(0.0);
        tap(&mut rt, "src-sim-set");
        tap(&mut rt, "src-tb");
        let path =
            std::env::temp_dir().join(format!("helion-rune-tb-save-{}.v", std::process::id()));
        rt.state
            .editor
            .paths
            .insert("project1_tb.v".into(), path.clone());
        tap(&mut rt, "editor-tool-save");
        let disk = std::fs::read_to_string(&path).expect("save must write the file");
        assert_eq!(
            disk,
            rt.state.editor.tb_lines.join("\n"),
            "disk must match the buffer"
        );
        assert!(!rt.state.editor.dirty, "save clears the dirty flag");
        rt.state.editor.tb_lines[2] = "module CORRUPTED;".into();
        reload_active(&mut rt.state).expect("reload must read the file");
        assert_eq!(rt.state.editor.tb_lines[2], "module project1_tb;");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn find_next_prev_step_through_tb_matches() {
        let mut s = editor_state();
        s.editor.tabs.push("project1_tb.v".into());
        s.editor.active = 2;
        s.editor.find_open = true;
        s.editor.find_query = rune::TextEdit::new("clk");
        s.editor.cursor_line = 0;
        sync_edit(&mut s);
        s.editor.edit.set_caret(0);
        let mut rt = harness_with(s);
        rt.frame(0.0);
        let shown = shown_text(&rt, "editor-find-count");
        assert!(
            shown.contains("3 matches"),
            "tb has 3 clk lines, got: {shown:?}"
        );
        tap(&mut rt, "editor-find-next");
        assert_eq!(rt.state.editor.cursor_line, 3);
        tap(&mut rt, "editor-find-next");
        assert_eq!(rt.state.editor.cursor_line, 10);
        tap(&mut rt, "editor-find-prev");
        assert_eq!(rt.state.editor.cursor_line, 3);
    }

    #[test]
    fn cursor_tap_moves_on_the_tb_tab() {
        let mut s = editor_state();
        s.editor.tabs.push("project1_tb.v".into());
        s.editor.active = 2;
        sync_edit(&mut s);
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "editor-gutter-10");
        assert_eq!(rt.state.editor.cursor_line, 10);
        let line10 = rt
            .state
            .editor
            .edit
            .text
            .lines()
            .nth(10)
            .unwrap_or("")
            .to_string();
        assert!(
            line10.contains(".clk(clk)"),
            "tb line 10 must show the port map"
        );
    }

    #[test]
    fn typing_in_the_field_inserts_and_marks_dirty() {
        use rune::{KeyEvent, Mods};
        let mut rt = harness_with(editor_state());
        rt.frame(0.0);
        tap(&mut rt, "editor-field");
        let before = rt.state.editor.edit.text.clone();
        rt.key(KeyEvent {
            key: Key::Text("x".into()),
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
        assert!(
            rt.state.editor.edit.text.len() > before.len(),
            "typing must insert"
        );
        assert!(rt.state.editor.dirty, "typing must mark dirty");
    }

    #[test]
    fn cmd_s_saves_the_live_text_to_disk() {
        use rune::{KeyEvent, Mods};
        let mut rt = harness_with(editor_state());
        rt.frame(0.0);
        tap(&mut rt, "editor-field");
        rt.key(KeyEvent {
            key: Key::Text("x".into()),
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
        assert!(rt.state.editor.dirty);
        let mods = Mods {
            cmd: true,
            ..Mods::default()
        };
        rt.key(KeyEvent {
            key: Key::Text("s".into()),
            mods,
        });
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.editor.dirty, "Cmd+S must save and clear dirty");
        assert!(
            rt.state.editor.lines.join("\n").contains('x'),
            "save must flush the typed text into the buffer"
        );
    }

    #[test]
    fn wheel_over_code_scrolls_tall_buffers() {
        let mut s = editor_state();
        s.editor.lines = (0..200).map(|i| format!("wire w{i};")).collect();
        s.editor.edit = TextEdit::new(s.editor.lines.join("\n"));
        s.editor.edit_tab = Some(0);
        let mut rt = harness_with(s);
        rt.frame(0.0);
        let vp = rt.presented("editor-scroll").unwrap().0;
        let y_before = rt.presented("editor-gutter-199").unwrap().0[1];
        assert!(
            y_before > vp[1] + vp[3],
            "last gutter row must start below the fold (y={y_before}, fold={})",
            vp[1] + vp[3]
        );
        let (wx, wy) = (vp[0] + vp[2] / 2.0, vp[1] + vp[3] / 2.0);
        for i in 0..10 {
            rt.wheel(wx, wy, -140.0);
            rt.frame(0.1 + i as f64 * (1.0 / 60.0));
        }
        let y_after = rt.presented("editor-gutter-199").unwrap().0[1];
        assert!(
            y_after < y_before - 100.0,
            "wheel must move content up (before={y_before}, after={y_after})"
        );
    }

    #[test]
    fn xdc_review_content_round_trips_through_disk() {
        use crate::views::dialogs::{DialogId, XdcStep};
        let mut s = editor_state();
        s.dialogs.open(DialogId::Constraints);
        s.dialogs.xdc.step = XdcStep::Review;
        let path =
            std::env::temp_dir().join(format!("helion-rune-xdc-save-{}.xdc", std::process::id()));
        s.editor.paths.insert("project1.xdc".into(), path.clone());
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "xdc-finish");
        assert_eq!(rt.state.dialogs.open, None);
        assert!(
            rt.state
                .editor
                .xdc_lines
                .iter()
                .any(|l| l.starts_with("create_clock -name clk")),
            "finish must append the SDC: {:?}",
            rt.state.editor.xdc_lines
        );
        // Let the dialog unmount so the next tap reaches the toolbar.
        for _ in 0..8 {
            rt.frame(1.0 / 60.0);
        }
        tap(&mut rt, "editor-tool-save");
        let disk = std::fs::read_to_string(&path).expect("save must write the xdc file");
        assert!(
            disk.contains("create_clock -name clk -period 10.000"),
            "disk must hold the review content: {disk:?}"
        );
        rt.state.editor.xdc_lines.clear();
        reload_active(&mut rt.state).expect("reload must read the file");
        assert!(
            rt.state
                .editor
                .xdc_lines
                .iter()
                .any(|l| l.starts_with("create_clock -name clk")),
            "reload must restore the review content"
        );
        let _ = std::fs::remove_file(&path);
    }
}
