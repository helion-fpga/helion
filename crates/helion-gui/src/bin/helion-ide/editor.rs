//! Editable source editor (UX fixes, Part 2).
//!
//! Replaces the old read-only label renderer with a real multiline [`egui::TextEdit`]:
//! caret, typing, selection, Cmd+S save with a dirty indicator, a synced gutter
//! overlay (line numbers + diagnostic/bookmark ticks, click to select), and a
//! VS Code-style minimap. Syntax highlighting is reused from
//! [`super::highlight_line_job`] through a custom layouter.
//!
//! Architecture: the [`egui::TextEdit`] is sized to its content
//! (`desired_rows` = line count, `desired_width` = longest line) so it never
//! scrolls internally. One outer [`egui::ScrollArea`] does all the scrolling,
//! which keeps the gutter overlay and the minimap viewport math trivially in
//! sync. The gutter and minimap are painted as overlays *after* the scroll
//! area, positioned from the current frame's scroll state - no lag.

use super::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Must match the `line_height` forced in the editor layouter.
const ROW_H: f32 = 13.0 * 1.5;
/// Above this many lines the minimap degrades from tiny text to bars.
const MINIMAP_TEXT_CAP: usize = 1500;
// Gutter / minimap geometry lives in chrome (layout math is tested there);
// import the shared constants.
use helion_gui::chrome::{
    editor_minimap_visible, EDITOR_GUTTER_W as GUTTER_W, EDITOR_MIN_TEXT_W as MIN_TEXT_W,
    EDITOR_MINIMAP_W as MINIMAP_W,
};

/// Load the open file into the edit buffer when the selection changes.
fn sync_editor_buffer(app: &mut HelionIde) {
    let path = app
        .model
        .selected_source
        .clone()
        .or_else(|| app.model.tree.sources.first().cloned())
        .map(PathBuf::from);
    if path != app.editor_path {
        app.editor_path = path.clone();
        app.editor_text = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| t.replace("\r\n", "\n"))
            .unwrap_or_default();
        app.editor_dirty = false;
        app.editor_line_rows.clear();
    }
}

/// Write the buffer back to disk and refresh the model's line table
/// (preserving bookmarks/selection, unlike `load_rtl_source`).
pub fn save_editor(app: &mut HelionIde) {
    let Some(path) = app.editor_path.clone() else {
        return;
    };
    if std::fs::write(&path, &app.editor_text).is_err() {
        return;
    }
    app.editor_dirty = false;
    app.model.refresh_source_text(&path.to_string_lossy());
}

/// Block-comment state after scanning one line.
pub(crate) fn block_state_after(line: &str, mut in_block: bool) -> bool {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if !in_block && i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
            in_block = true;
            i += 2;
        } else if in_block && i + 1 < b.len() && b[i] == b'*' && b[i + 1] == b'/' {
            in_block = false;
            i += 2;
        } else {
            i += 1;
        }
    }
    in_block
}

/// Append one highlighted line's sections into a [`egui::text::LayoutJob`].
pub(crate) fn append_highlighted(
    job: &mut egui::text::LayoutJob,
    line: &str,
    lang: highlight::Lang,
    starts_in_block: bool,
    theme: &theme::Theme,
    font_size: f32,
) {
    let line_job = highlight_line_job(line, lang, starts_in_block, theme);
    for sec in &line_job.sections {
        if let Some(t) = line.get(sec.byte_range.clone()) {
            let mut fmt = sec.format.clone();
            fmt.font_id = theme::font_code(font_size);
            if font_size == 13.0 {
                fmt.line_height = Some(ROW_H);
            }
            job.append(t, 0.0, fmt);
        }
    }
}

fn base_format(theme: &theme::Theme, font_size: f32) -> egui::TextFormat {
    let mut f = egui::TextFormat {
        font_id: theme::font_code(font_size),
        color: theme.text,
        ..Default::default()
    };
    if font_size == 13.0 {
        f.line_height = Some(ROW_H);
    }
    f
}

/// Build the editor [`egui::Galley`] with syntax highlighting.
///
/// In wrap mode each logical line is also laid out on its own so the gutter
/// can size rows to the true visual height; the counts are reported through
/// `line_rows_out` (visual rows per logical line, same frame - no lag).
fn build_editor_galley(
    ui: &egui::Ui,
    text: &str,
    lang: highlight::Lang,
    theme: &theme::Theme,
    wrap: bool,
    wrap_width: f32,
    line_rows_out: &Rc<RefCell<Vec<usize>>>,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = if wrap { wrap_width } else { f32::INFINITY };
    let base = base_format(theme, 13.0);
    let mut in_block = false;
    let mut first = true;
    let mut rows_vec = Vec::new();
    for line in text.split('\n') {
        if !first {
            job.append("\n", 0.0, base.clone());
        }
        first = false;
        append_highlighted(&mut job, line, lang, in_block, theme, 13.0);
        if wrap {
            let mut lj = egui::text::LayoutJob::default();
            lj.wrap.max_width = wrap_width;
            append_highlighted(&mut lj, line, lang, in_block, theme, 13.0);
            let g = ui.fonts(|f| f.layout_job(lj));
            rows_vec.push(g.rows.len().max(1));
        }
        if lang == highlight::Lang::SystemVerilog {
            in_block = block_state_after(line, in_block);
        }
    }
    if !wrap {
        rows_vec = vec![1; text.split('\n').count()];
    }
    *line_rows_out.borrow_mut() = rows_vec;
    ui.fonts(|f| f.layout_job(job))
}

/// Tiny highlighted galley for the minimap (never wraps; clipped instead).
fn build_minimap_galley(
    ui: &egui::Ui,
    text: &str,
    lang: highlight::Lang,
    theme: &theme::Theme,
    font_size: f32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let base = base_format(theme, font_size);
    let mut in_block = false;
    let mut first = true;
    for line in text.split('\n') {
        if !first {
            job.append("\n", 0.0, base.clone());
        }
        first = false;
        append_highlighted(&mut job, line, lang, in_block, theme, font_size);
        if lang == highlight::Lang::SystemVerilog {
            in_block = block_state_after(line, in_block);
        }
    }
    ui.fonts(|f| f.layout_job(job))
}

/// Map a content-space y to a 0-based logical line.
fn y_to_line(y: f32, n_lines: usize, wrap: bool, line_rows: &[usize]) -> usize {
    if wrap && line_rows.len() == n_lines {
        let mut acc = 0.0;
        for (i, r) in line_rows.iter().enumerate() {
            let h = *r as f32 * ROW_H;
            if y < acc + h {
                return i;
            }
            acc += h;
        }
        n_lines.saturating_sub(1)
    } else {
        ((y / ROW_H) as usize).min(n_lines.saturating_sub(1))
    }
}

fn gutter_tick_color(theme: &theme::Theme, kind: &str) -> Option<egui::Color32> {
    match kind {
        "error" => Some(theme.danger),
        "warning" => Some(theme.warn),
        "advisory" => Some(theme.info),
        "bookmark" => Some(theme.accent),
        "probe" => Some(theme.gold),
        _ => None,
    }
}

#[derive(Clone, Debug)]
struct EditorAuxState {
    find_open: bool,
    find_query: String,
    replace_text: String,
    case_sensitive: bool,
    current_match: usize,
    outline_open: bool,
}

impl Default for EditorAuxState {
    fn default() -> Self {
        let find_env = std::env::var("HELION_EDITOR_FIND").as_deref() == Ok("1");
        Self {
            find_open: find_env,
            find_query: if find_env { "cnt".into() } else { String::new() },
            replace_text: String::new(),
            case_sensitive: false,
            current_match: 0,
            outline_open: true,
        }
    }
}

pub fn toggle_find_bar(ctx: &egui::Context) {
    let id = egui::Id::new("editor_aux_state");
    let mut st: EditorAuxState = ctx.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    st.find_open = !st.find_open;
    ctx.data_mut(|d| d.insert_temp(id, st));
}

pub fn set_find_demo(ctx: &egui::Context, open: bool, query: &str) {
    let id = egui::Id::new("editor_aux_state");
    let mut st: EditorAuxState = ctx.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    st.find_open = open;
    if !query.is_empty() {
        st.find_query = query.to_string();
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
}

#[derive(Clone, Debug)]
struct RtlSymbol {
    kind: &'static str,
    name: String,
    line: usize,
}

fn extract_rtl_symbols(text: &str) -> Vec<RtlSymbol> {
    let mut out = Vec::new();
    for (idx, raw) in text.split('\n').enumerate() {
        let line = idx + 1;
        let code = raw.split("//").next().unwrap_or(raw).trim();
        if code.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = code
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '$')
            .filter(|s| !s.is_empty())
            .collect();
        let Some(&first) = tokens.first() else {
            continue;
        };
        match first {
            "module" | "entity" | "package" | "interface" => {
                if let Some(&name) = tokens.get(1) {
                    out.push(RtlSymbol { kind: "MOD", name: name.to_string(), line });
                }
            }
            "input" | "output" | "inout" => {
                if let Some(&name) = tokens.last() {
                    if name != first && name != "logic" && name != "wire" && name != "reg" {
                        out.push(RtlSymbol { kind: "PORT", name: name.to_string(), line });
                    }
                }
            }
            "logic" | "reg" | "wire" | "bit" | "integer" => {
                if let Some(&name) = tokens.last() {
                    if name != first {
                        out.push(RtlSymbol { kind: "SIG", name: name.to_string(), line });
                    }
                }
            }
            "parameter" | "localparam" => {
                if let Some(eq_pos) = code.find('=') {
                    let lhs = code[..eq_pos].trim();
                    if let Some(name) = lhs
                        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .filter(|s| !s.is_empty())
                        .last()
                    {
                        out.push(RtlSymbol { kind: "PARM", name: name.to_string(), line });
                    }
                }
            }
            "always_ff" | "always_comb" | "always_latch" | "always" | "initial" => {
                out.push(RtlSymbol { kind: "BLK", name: first.to_string(), line });
            }
            "assign" => {
                if let Some(&lhs) = tokens.get(1) {
                    out.push(RtlSymbol { kind: "ASN", name: lhs.to_string(), line });
                }
            }
            "function" | "task" => {
                if let Some(&name) = tokens.get(1) {
                    out.push(RtlSymbol { kind: "FN", name: name.to_string(), line });
                }
            }
            _ => {}
        }
    }
    out
}

fn find_match_lines(text: &str, query: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    let q = if case_sensitive {
        query.to_string()
    } else {
        query.to_ascii_lowercase()
    };
    let mut hits = Vec::new();
    for (idx, line) in text.split('\n').enumerate() {
        let hay = if case_sensitive {
            line.to_string()
        } else {
            line.to_ascii_lowercase()
        };
        let mut start = 0;
        while let Some(pos) = hay[start..].find(&q) {
            hits.push((idx + 1, start + pos));
            start += pos + q.len().max(1);
        }
    }
    hits
}

pub fn paint_editor(ui: &mut egui::Ui, app: &mut HelionIde, theme: &theme::Theme) {
    sync_editor_buffer(app);
    ui.set_width(ui.available_width());

    if ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
        toggle_find_bar(ui.ctx());
    }
    let aux_id = egui::Id::new("editor_aux_state");
    let mut aux: EditorAuxState = ui.ctx().data_mut(|d| d.get_temp(aux_id)).unwrap_or_default();

    let lang = {
        let m = &app.model;
        m.selected_source
            .as_deref()
            .or_else(|| m.tree.sources.first().map(String::as_str))
            .map(highlight::detect_lang)
            .unwrap_or(highlight::Lang::Unknown)
    };
    let file_name = {
        let m = &app.model;
        m.selected_source
            .as_deref()
            .or_else(|| m.tree.sources.first().map(String::as_str))
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
            .unwrap_or_else(|| "no file".to_string())
    };
    let has_sources = {
        let m = &app.model;
        !(m.tree.sources.is_empty() && m.source_line_rows().is_empty())
    };
    let cursor = {
        let m = &app.model;
        let n = m.source_line_rows().len();
        m.selected_source_line.unwrap_or(1).min(n.max(1))
    };
    let crumbs = {
        let m = &app.model;
        editor_crumbs(m.source_line_rows(), cursor, lang)
    };
    let sym_cache_id = egui::Id::new("editor_symbols_cache");
    let cache_key = (
        app.editor_path.clone(),
        app.editor_text.len(),
    );
    let (n_lines, symbols): (usize, Arc<Vec<RtlSymbol>>) = ui
        .ctx()
        .data_mut(|d| {
            d.get_temp::<(Option<PathBuf>, usize, usize, Arc<Vec<RtlSymbol>>)>(sym_cache_id)
        })
        .filter(|(p, len, _, _)| p == &cache_key.0 && *len == cache_key.1)
        .map(|(_, _, nl, syms)| (nl, syms))
        .unwrap_or_else(|| {
            let nl = app.editor_text.split('\n').count().max(1);
            let syms = Arc::new(extract_rtl_symbols(&app.editor_text));
            ui.ctx().data_mut(|d| {
                d.insert_temp(sym_cache_id, (cache_key.0, cache_key.1, nl, syms.clone()));
            });
            (nl, syms)
        });
    let mut do_open = false;
    let mut do_goto = false;
    let mut do_goto_def = false;
    let mut do_cross_probe = false;
    let mut do_save = false;
    let mut do_wrap = false;
    ui.add_space(2.0);
    if ui
        .ctx()
        .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F12))
    {
        do_goto_def = true;
    }
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let mut name = RichText::new(&file_name)
            .color(theme.text)
            .font(theme::font_ui_medium(12.5));
        if app.editor_dirty {
            name = RichText::new(format!("{file_name} ●"))
                .color(theme.warn)
                .font(theme::font_ui_medium(12.5));
        }
        ui.label(name)
            .on_hover_text(if app.editor_dirty { "Unsaved changes (⌘S)" } else { "Active source file" });
        if let Some((name, line)) = &crumbs.module {
            ui.label(RichText::new("›").color(theme.faint).font(theme::font_ui(12.0)));
            if ui
                .link(RichText::new(name).color(theme.info).font(theme::font_code(12.0)))
                .on_hover_text(format!("Module declaration · Go to line {line}"))
                .clicked()
            {
                ui.ctx()
                    .memory_mut(|m| m.data.insert_temp(egui::Id::new("text_editor_scroll_to"), *line));
            }
        }
        if let Some((name, line)) = &crumbs.block {
            ui.label(RichText::new("›").color(theme.faint).font(theme::font_ui(12.0)));
            if ui
                .link(RichText::new(name).color(theme.text).font(theme::font_code(12.0)))
                .on_hover_text(format!("Procedural block · Go to line {line}"))
                .clicked()
            {
                ui.ctx()
                    .memory_mut(|m| m.data.insert_temp(egui::Id::new("text_editor_scroll_to"), *line));
            }
        }
        ui.label(
            RichText::new(format!("Ln {cursor}/{n_lines}"))
                .color(theme.faint)
                .font(theme::font_code(11.0)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(6.0);
            if ui
                .small_button("Probe (F4)")
                .on_hover_text("Synchronize selection across RTL, Schematic, Device, Timing, DRC, and Waveform (F4)")
                .clicked()
            {
                do_cross_probe = true;
            }
            if ui
                .small_button("Def (F12)")
                .on_hover_text("Jump to SV/VHDL/XDC symbol declaration (F12 or Ctrl/⌘+Click)")
                .clicked()
            {
                do_goto_def = true;
            }
            if ui
                .small_button("Save")
                .on_hover_text("Save file (⌘S)")
                .clicked()
            {
                do_save = true;
            }
            let wrap_label = if app.editor_wrap { "Wrap: on" } else { "Wrap: off" };
            if ui
                .small_button(wrap_label)
                .on_hover_text("Toggle soft wrap (Alt+Z)")
                .clicked()
            {
                do_wrap = true;
            }
            let outline_label = if aux.outline_open {
                format!("Outline ({})", symbols.len())
            } else {
                "Outline".to_string()
            };
            if ui
                .add(egui::Button::new(RichText::new(outline_label).size(11.0)).selected(aux.outline_open))
                .on_hover_text("Toggle RTL Symbol Outline dock")
                .clicked()
            {
                aux.outline_open = !aux.outline_open;
            }
            if ui
                .add(egui::Button::new(RichText::new("Find").size(11.0)).selected(aux.find_open))
                .on_hover_text("Find and replace in current file (⌘F)")
                .clicked()
            {
                aux.find_open = !aux.find_open;
            }
        });
    });
    ui.separator();

    // ---- Inline Find / Replace Pop-up Bar ----
    let matches = if aux.find_open && !aux.find_query.is_empty() {
        find_match_lines(&app.editor_text, &aux.find_query, aux.case_sensitive)
    } else {
        Vec::new()
    };
    if !matches.is_empty() && aux.current_match >= matches.len() {
        aux.current_match = 0;
    }
    if aux.find_open {
        let mut jump_line: Option<usize> = None;
        egui::Frame::new()
            .fill(theme.panel)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Find").font(theme::font_ui_medium(12.0)).color(theme.dim));
                    let find_resp = ui.add(
                        egui::TextEdit::singleline(&mut aux.find_query)
                            .desired_width(180.0)
                            .hint_text("Search in file…")
                            .font(theme::font_code(12.0)),
                    );
                    if find_resp.changed() {
                        aux.current_match = 0;
                        if let Some(&(ln, _)) = find_match_lines(&app.editor_text, &aux.find_query, aux.case_sensitive).first() {
                            jump_line = Some(ln);
                        }
                    }
                    let badge_txt = if aux.find_query.is_empty() {
                        "Type to search".to_string()
                    } else if matches.is_empty() {
                        "0 results".to_string()
                    } else {
                        format!("{} of {}", aux.current_match + 1, matches.len())
                    };
                    let badge_col = if !aux.find_query.is_empty() && matches.is_empty() {
                        theme.danger
                    } else {
                        theme.dim
                    };
                    ui.label(RichText::new(badge_txt).font(theme::font_code(11.0)).color(badge_col));
                    if ui
                        .add_enabled(!matches.is_empty(), egui::Button::new("▲").small())
                        .on_hover_text("Previous match")
                        .clicked()
                    {
                        aux.current_match = (aux.current_match + matches.len() - 1) % matches.len();
                        jump_line = Some(matches[aux.current_match].0);
                    }
                    if ui
                        .add_enabled(!matches.is_empty(), egui::Button::new("▼").small())
                        .on_hover_text("Next match")
                        .clicked()
                    {
                        aux.current_match = (aux.current_match + 1) % matches.len();
                        jump_line = Some(matches[aux.current_match].0);
                    }
                    if ui
                        .add(egui::Button::new(RichText::new("Aa").monospace().size(11.0)).selected(aux.case_sensitive))
                        .on_hover_text("Match case")
                        .clicked()
                    {
                        aux.case_sensitive = !aux.case_sensitive;
                    }
                    ui.separator();
                    ui.label(RichText::new("Replace").font(theme::font_ui_medium(12.0)).color(theme.dim));
                    ui.add(
                        egui::TextEdit::singleline(&mut aux.replace_text)
                            .desired_width(150.0)
                            .hint_text("Replace with…")
                            .font(theme::font_code(12.0)),
                    );
                    if ui
                        .add_enabled(!matches.is_empty() && !aux.find_query.is_empty(), egui::Button::new("Replace").small())
                        .clicked()
                    {
                        if let Some(pos) = app.editor_text.find(&aux.find_query) {
                            app.editor_text.replace_range(pos..pos + aux.find_query.len(), &aux.replace_text);
                            app.editor_dirty = true;
                        }
                    }
                    if ui
                        .add_enabled(!matches.is_empty() && !aux.find_query.is_empty(), egui::Button::new("Replace All").small())
                        .clicked()
                    {
                        app.editor_text = app.editor_text.replace(&aux.find_query, &aux.replace_text);
                        app.editor_dirty = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Close Find bar (Esc)").clicked() {
                            aux.find_open = false;
                        }
                    });
                });
            });
        ui.separator();
        if let Some(ln) = jump_line {
            let _ = app.model.select_editor_line(&ln.to_string());
            ui.ctx()
                .memory_mut(|m| m.data.insert_temp(egui::Id::new("text_editor_scroll_to"), ln));
        }
    }

    if do_save {
        save_editor(app);
    }
    if do_wrap {
        app.editor_wrap = !app.editor_wrap;
    }
    if do_open {
        let _ = app.model.open_text_editor();
    }
    if do_goto {
        let _ = app.model.goto_editor("");
    }
    ui.ctx().data_mut(|d| d.insert_temp(aux_id, aux.clone()));

    if do_goto_def {
        if let Ok(loc) = app.model.goto_definition("") {
            ui.ctx()
                .memory_mut(|m| m.data.insert_temp(egui::Id::new("text_editor_scroll_to"), loc.line));
        }
    }
    if do_cross_probe {
        let _ = app.model.cross_probe("");
    }
    if !has_sources {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new("No sources yet. Open a file to edit.")
                    .color(theme.faint)
                    .font(theme::font_ui(13.0)),
            );
        });
        return;
    }

    // ---- main row: gutter | text scroll | minimap | symbol outline ----
    let avail = ui.available_size();
    let wrap = app.editor_wrap;
    let outline_w = 172.0_f32;
    let show_outline = aux.outline_open && avail.x >= 560.0;
    let rem_for_editor = if show_outline {
        (avail.x - outline_w - 8.0).max(240.0)
    } else {
        avail.x
    };
    let show_minimap = editor_minimap_visible(rem_for_editor);
    let mut jump_to_symbol: Option<usize> = None;
    let match_lines: std::collections::HashSet<usize> = matches.iter().map(|&(l, _)| l).collect();

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let gutter_resp = ui.allocate_exact_size(
            egui::vec2(GUTTER_W, avail.y),
            egui::Sense::click(),
        );
        let text_w = if show_minimap {
            (rem_for_editor - GUTTER_W - MINIMAP_W).max(MIN_TEXT_W)
        } else {
            (rem_for_editor - GUTTER_W).max(120.0)
        };
        let scroll_resp = ui.allocate_ui_with_layout(
            egui::vec2(text_w, avail.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| paint_editor_text(ui, app, theme, lang, n_lines, wrap),
        );
        let mini_resp = if show_minimap {
            Some(ui.allocate_exact_size(
                egui::vec2(MINIMAP_W, avail.y),
                egui::Sense::click_and_drag(),
            ))
        } else {
            None
        };

        if show_outline {
            let (sep_r, _) = ui.allocate_exact_size(egui::vec2(1.0, avail.y), egui::Sense::hover());
            ui.painter().line_segment(
                [sep_r.left_top(), sep_r.left_bottom()],
                egui::Stroke::new(1.0, theme.line),
            );
            ui.allocate_ui_with_layout(
                egui::vec2(outline_w, avail.y),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::new()
                        .fill(theme.panel)
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.set_min_size(egui::vec2(outline_w - 16.0, (avail.y - 12.0).max(1.0)));
                            ui.label(
                                RichText::new("SYMBOL OUTLINE")
                                    .font(theme::font_ui_semibold(10.5))
                                    .color(theme.faint),
                            );
                            ui.add_space(4.0);
                            if symbols.is_empty() {
                                ui.label(RichText::new("No symbols parsed.").small().color(theme.faint));
                            } else {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                egui::ScrollArea::vertical()
                                    .id_salt("editor_symbol_outline")
                                    .auto_shrink([false, false])
                                    .show_rows(ui, 22.0, symbols.len(), |ui, sym_range| {
                                        let row_w = ui.available_width().max(80.0);
                                        for idx in sym_range {
                                            let Some(sym) = symbols.get(idx) else { continue };
                                            let on = cursor == sym.line;
                                            let badge_col = match sym.kind {
                                                "MOD" => theme.accent,
                                                "PORT" => theme.info,
                                                "SIG" => theme.ok,
                                                "BLK" => theme.warn,
                                                _ => theme.violet,
                                            };
                                            let (rrect, rresp) = ui.allocate_exact_size(
                                                egui::vec2(row_w, 22.0),
                                                egui::Sense::click(),
                                            );
                                            if ui.is_rect_visible(rrect) {
                                                let p = ui.painter();
                                                if on {
                                                    p.rect_filled(rrect, 3.0, theme.sel);
                                                } else if rresp.hovered() {
                                                    p.rect_filled(rrect, 3.0, theme.surface_hover);
                                                }
                                                p.text(
                                                    egui::pos2(rrect.left() + 4.0, rrect.center().y),
                                                    egui::Align2::LEFT_CENTER,
                                                    sym.kind,
                                                    theme::font_code(9.5),
                                                    badge_col,
                                                );
                                                let ltxt = format!(":{}", sym.line);
                                                let lgalley = p.layout_no_wrap(
                                                    ltxt,
                                                    theme::font_code(9.5),
                                                    theme.faint,
                                                );
                                                let lw = lgalley.size().x;
                                                let lx = (rrect.right() - 4.0 - lw).max(rrect.left() + 64.0);
                                                p.galley(
                                                    egui::pos2(lx, rrect.center().y - lgalley.size().y * 0.5),
                                                    lgalley,
                                                    theme.faint,
                                                );
                                                let name_clip = egui::Rect::from_min_max(
                                                    egui::pos2(rrect.left() + 34.0, rrect.top()),
                                                    egui::pos2((lx - 4.0).max(rrect.left() + 48.0), rrect.bottom()),
                                                );
                                                let ngalley = p.layout_no_wrap(
                                                    sym.name.clone(),
                                                    theme::font_code(11.0),
                                                    if on { theme.text } else { theme.dim },
                                                );
                                                p.with_clip_rect(name_clip).galley(
                                                    egui::pos2(rrect.left() + 36.0, rrect.center().y - ngalley.size().y * 0.5),
                                                    ngalley,
                                                    if on { theme.text } else { theme.dim },
                                                );
                                            }
                                            if rresp.on_hover_text(format!("{} {} (line {})", sym.kind, sym.name, sym.line)).clicked() {
                                                jump_to_symbol = Some(sym.line);
                                            }
                                        }
                                    });
                            }
                        });
                },
            );
        }

        let scroll_out = &scroll_resp.inner;
        paint_gutter_overlay(
            ui,
            app,
            theme,
            &gutter_resp.1,
            scroll_out,
            n_lines,
            wrap,
            &match_lines,
        );
        if let Some(mini_resp) = mini_resp {
            paint_minimap(ui, app, theme, &mini_resp.1, scroll_out, n_lines, lang);
        }
        handle_gutter_click(app, &gutter_resp.1, scroll_out, n_lines, wrap);
    });

    if let Some(line) = jump_to_symbol {
        let _ = app.model.select_editor_line(&line.to_string());
        ui.ctx()
            .memory_mut(|m| m.data.insert_temp(egui::Id::new("text_editor_scroll_to"), line));
    }
}

/// The scroll area + text view. Uses full `TextEdit::multiline` for files <= 800 lines,
/// and viewport-virtualized `show_rows` for large files (e.g. 27,117-line `ysyx_ibex.sv`)
/// so only the ~45 visible rows are syntax-highlighted and tessellated per frame.
fn paint_editor_text(
    ui: &mut egui::Ui,
    app: &mut HelionIde,
    theme: &theme::Theme,
    lang: highlight::Lang,
    n_lines: usize,
    wrap: bool,
) -> egui::scroll_area::ScrollAreaOutput<()> {
    let mut scroll = egui::ScrollArea::both()
        .id_salt("ug893_text_editor")
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible);
    if let Some(y) = app.editor_mini_scroll.take() {
        scroll = scroll.vertical_scroll_offset(y);
    } else {
        let scroll_id = egui::Id::new("text_editor_scroll_to");
        let target: Option<usize> = ui.ctx().memory(|m| m.data.get_temp(scroll_id));
        if target.is_some() {
            ui.ctx().memory_mut(|m| m.data.remove::<usize>(scroll_id));
        }
        if let Some(tline) = target {
            let y =
                (tline.saturating_sub(1) as f32 * ROW_H - ui.available_height() / 2.0).max(0.0);
            scroll = scroll.vertical_scroll_offset(y);
        }
    }
    if n_lines > 800 {
        app.editor_line_rows.clear();
        let spans_id = egui::Id::new("editor_line_spans_cache_v2");
        let skey = (app.editor_path.clone(), app.editor_text.len());
        let (spans, max_cols): (Arc<Vec<(usize, usize, bool)>>, usize) = ui
            .ctx()
            .data_mut(|d| {
                d.get_temp::<(Option<PathBuf>, usize, Arc<Vec<(usize, usize, bool)>>, usize)>(spans_id)
            })
            .filter(|(p, len, _, _)| p == &skey.0 && *len == skey.1)
            .map(|(_, _, s, mc)| (s, mc))
            .unwrap_or_else(|| {
                let mut v = Vec::with_capacity(n_lines);
                let mut start = 0usize;
                let mut in_blk = false;
                let mut max_c = 80usize;
                let bytes = app.editor_text.as_bytes();
                for (idx, &b) in bytes.iter().enumerate() {
                    if b == b'\n' {
                        let line_s = app.editor_text.get(start..idx).unwrap_or("");
                        max_c = max_c.max(line_s.len());
                        v.push((start, idx, in_blk));
                        if lang == highlight::Lang::SystemVerilog {
                            in_blk = block_state_after(line_s, in_blk);
                        }
                        start = idx + 1;
                    }
                }
                if start <= app.editor_text.len() {
                    let line_s = app.editor_text.get(start..app.editor_text.len()).unwrap_or("");
                    max_c = max_c.max(line_s.len());
                    v.push((start, app.editor_text.len(), in_blk));
                }
                let arc = Arc::new(v);
                ui.ctx()
                    .data_mut(|d| d.insert_temp(spans_id, (skey.0, skey.1, arc.clone(), max_c)));
                (arc, max_c)
            });
        let viewport_w = ui.available_width().max(480.0);
        let row_w = if wrap {
            viewport_w
        } else {
            (max_cols as f32 * 7.8 + 32.0).max(viewport_w)
        };
        let selected_line = app.model.selected_source_line;
        let mut clicked_line: Option<usize> = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        let out = scroll.show_rows(ui, ROW_H, spans.len(), |ui, row_range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_min_width(row_w);
            for row_idx in row_range {
                let (s, e, starts_in_blk) = spans.get(row_idx).copied().unwrap_or((0, 0, false));
                let line_str = app.editor_text.get(s..e).unwrap_or("");
                let line_no = row_idx + 1;
                let is_sel = selected_line == Some(line_no);
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(row_w, ROW_H),
                    egui::Sense::click(),
                );
                if ui.is_rect_visible(rect) {
                    if is_sel {
                        ui.painter().rect_filled(rect, 0.0, theme.sel.gamma_multiply(0.45));
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, 0.0, theme.surface_hover.gamma_multiply(0.35));
                    }
                    if !line_str.is_empty() {
                        let mut job = egui::text::LayoutJob::default();
                        job.wrap.max_width = if wrap { viewport_w - 8.0 } else { f32::INFINITY };
                        append_highlighted(&mut job, line_str, lang, starts_in_blk, theme, 13.0);
                        let galley = ui.fonts(|f| f.layout_job(job));
                        ui.painter().galley(
                            egui::pos2(rect.left() + 6.0, rect.top()),
                            galley,
                            theme.text,
                        );
                    }
                }
                if resp.clicked() {
                    clicked_line = Some(line_no);
                }
            }
        });
        if let Some(ln) = clicked_line {
            let _ = app.model.select_editor_line(&ln.to_string());
        }
        return out;
    }
    scroll.show(ui, |ui| {
        let rows_rc: Rc<RefCell<Vec<usize>>> = Rc::new(RefCell::new(Vec::new()));
        let rows_w = rows_rc.clone();
        let theme_c = *theme;
        let mut layouter =
            move |lui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                build_editor_galley(lui, buf.as_str(), lang, &theme_c, wrap, wrap_width, &rows_w)
            };
        let viewport_w = ui.available_width().max(160.0);
        let des_w = if wrap {
            viewport_w
        } else {
            app.editor_content_w.max(viewport_w)
        };
        let out = egui::TextEdit::multiline(&mut app.editor_text)
            .id(egui::Id::new("source_editor_text"))
            .desired_rows(n_lines)
            .desired_width(des_w)
            .frame(false)
            .layouter(&mut layouter)
            .show(ui);
        if out.response.changed() {
            app.editor_dirty = true;
        }
        if !wrap {
            app.editor_content_w = out.galley.size().x.max(200.0);
        }
        app.editor_line_rows = rows_rc.borrow().clone();
        // Inline SV/VHDL/XDC symbol hover tooltip & Ctrl/Cmd+Click go-to-definition.
        if let Some(pos) = out.response.hover_pos() {
            let rel_y = (pos.y - out.galley_pos.y).max(0.0);
            let rel_x = (pos.x - out.galley_pos.x).max(0.0);
            let glyph_w = ui
                .fonts(|f| f.glyph_width(&theme::font_code(13.0), 'M'))
                .max(7.2);
            let hovered_line = y_to_line(rel_y, n_lines, wrap, &app.editor_line_rows) + 1;
            let hovered_col = (rel_x / glyph_w).floor() as usize + 1;
            if let Some(sym) = app.model.symbol_at_line_col(hovered_line, hovered_col) {
                if let Some(info) = app.model.symbol_hover(&sym) {
                    let theme_h = *theme;
                    let info_c = info.clone();
                    out.response.clone().on_hover_ui_at_pointer(|ui| {
                        ui.set_max_width(360.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(&info_c.symbol)
                                    .font(theme::font_code(13.0))
                                    .color(theme_h.accent)
                                    .strong(),
                            );
                            ui.label(
                                RichText::new(format!("({} · {}-bit)", info_c.kind, info_c.width))
                                    .font(theme::font_ui(11.5))
                                    .color(theme_h.dim),
                            );
                        });
                        ui.separator();
                        ui.label(
                            RichText::new(format!(
                                "Width: {} bit(s)   ·   Drivers: {}   ·   Loads: {}",
                                info_c.width, info_c.drivers, info_c.loads
                            ))
                            .font(theme::font_code(11.5))
                            .color(theme_h.text),
                        );
                        let (slack_txt, slack_col) = match info_c.slack_ps {
                            Some(s) if s >= 0 => (format!("Worst Slack: +{s} ps (MET)"), theme_h.ok),
                            Some(s) => (format!("Worst Slack: {s} ps (VIOLATED)"), theme_h.danger),
                            None => ("Worst Slack: n/a (unrouted)".to_string(), theme_h.faint),
                        };
                        ui.label(
                            RichText::new(slack_txt)
                                .font(theme::font_code(11.5))
                                .color(slack_col),
                        );
                        if let Some(def) = &info_c.definition {
                            let bname = def.file.rsplit('/').next().unwrap_or(&def.file);
                            ui.label(
                                RichText::new(format!(
                                    "Defined at {}:{} — {}",
                                    bname, def.line, def.decl_text
                                ))
                                .font(theme::font_code(11.0))
                                .color(theme_h.dim),
                            );
                        }
                        ui.label(
                            RichText::new("F12 / ⌘+Click: Go to Definition  ·  F4: Cross-Probe")
                                .font(theme::font_ui(10.5))
                                .color(theme_h.faint),
                        );
                    });
                }
                let cmd_click = out.response.clicked()
                    && ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
                let alt_click = out.response.clicked() && ui.input(|i| i.modifiers.alt);
                if cmd_click {
                    if let Ok(loc) = app.model.goto_definition(&sym) {
                        ui.ctx().memory_mut(|m| {
                            m.data
                                .insert_temp(egui::Id::new("text_editor_scroll_to"), loc.line)
                        });
                    }
                } else if alt_click {
                    let _ = app.model.cross_probe(&sym);
                }
            }
        }
    })
}

/// Gutter overlay: line numbers + diagnostic/bookmark ticks, painted in screen
/// space from the current frame's scroll state (no lag).
#[allow(clippy::too_many_arguments)]
fn paint_gutter_overlay(
    ui: &egui::Ui,
    app: &HelionIde,
    theme: &theme::Theme,
    gutter_resp: &egui::Response,
    scroll: &egui::scroll_area::ScrollAreaOutput<()>,
    n_lines: usize,
    wrap: bool,
    match_lines: &std::collections::HashSet<usize>,
) {
    let gutter_rect = gutter_resp.rect;
    let offset_y = scroll.state.offset.y;
    let viewport_h = scroll.inner_rect.height();
    let line_rows = &app.editor_line_rows;
    let markers = if n_lines <= 2_500 {
        app.model.editor_markers()
    } else {
        Vec::new()
    };
    let bookmarks = &app.model.editor_bookmarks;
    let selected = app.model.selected_source_line;

    let painter = ui.painter().with_clip_rect(gutter_rect);
    painter.rect_filled(gutter_rect, 0.0, theme.panel.gamma_multiply(0.55));
    painter.line_segment(
        [
            egui::pos2(gutter_rect.max.x - 1.0, gutter_rect.min.y),
            egui::pos2(gutter_rect.max.x - 1.0, gutter_rect.max.y),
        ],
        egui::Stroke::new(1.0, theme.line),
    );
    // Jump directly to first visible line in non-wrap mode for O(1) gutter rendering on 27k-line files.
    let start_i = if !wrap || line_rows.len() != n_lines {
        ((offset_y / ROW_H).floor() as usize).min(n_lines)
    } else {
        0
    };
    let mut cy = start_i as f32 * ROW_H;
    let mut i = start_i;
    while i < n_lines {
        let h = if wrap && line_rows.len() == n_lines {
            line_rows[i] as f32 * ROW_H
        } else {
            ROW_H
        };
        if cy + h >= offset_y && cy <= offset_y + viewport_h {
            let sy = gutter_rect.min.y + (cy - offset_y);
            let num = (i + 1).to_string();
            let on = selected == Some(i + 1);
            let is_match = match_lines.contains(&(i + 1));
            if on {
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(gutter_rect.min.x, sy),
                        egui::vec2(gutter_rect.width() - 1.0, h),
                    ),
                    2.0,
                    theme.sel,
                );
            } else if is_match {
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(gutter_rect.min.x, sy),
                        egui::vec2(gutter_rect.width() - 1.0, h),
                    ),
                    2.0,
                    theme.gold.gamma_multiply(0.22),
                );
            }
            painter.text(
                egui::pos2(gutter_rect.max.x - 8.0, sy + ROW_H * 0.5),
                egui::Align2::RIGHT_CENTER,
                num,
                theme::font_code(11.5),
                if on {
                    theme.text
                } else if is_match {
                    theme.gold
                } else {
                    theme.faint
                },
            );
            // Diagnostic tick (worst severity wins, as before).
            let tick = markers
                .iter()
                .filter(|m| m.line == i + 1)
                .filter_map(|m| gutter_tick_color(theme, m.kind.as_str()))
                .next()
                .or_else(|| {
                    bookmarks
                        .contains(&(i + 1))
                        .then_some(theme.accent)
                })
                .or_else(|| is_match.then_some(theme.gold));
            if let Some(c) = tick {
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(gutter_rect.min.x + 4.0, sy + (ROW_H - 12.0) / 2.0),
                        egui::vec2(6.0, 12.0),
                    ),
                    2.0,
                    c,
                );
            }
        }
        cy += h;
        i += 1;
        if cy > offset_y + viewport_h {
            break;
        }
    }
}

/// Gutter click: tick zone toggles a bookmark, elsewhere selects the line.
fn handle_gutter_click(
    app: &mut HelionIde,
    gutter_resp: &egui::Response,
    scroll: &egui::scroll_area::ScrollAreaOutput<()>,
    n_lines: usize,
    wrap: bool,
) {
    if !gutter_resp.clicked() {
        return;
    }
    let Some(pos) = gutter_resp.interact_pointer_pos() else {
        return;
    };
    let offset_y = scroll.state.offset.y;
    let content_y = pos.y - gutter_resp.rect.min.y + offset_y;
    let line = y_to_line(content_y.max(0.0), n_lines, wrap, &app.editor_line_rows) + 1;
    if pos.x < gutter_resp.rect.min.x + 14.0 {
        let _ = app.model.toggle_editor_bookmark(&line.to_string());
    } else {
        let _ = app.model.select_editor_line(&line.to_string());
    }
}

/// VS Code-style minimap: tiny highlighted text (bars for huge files),
/// a viewport rectangle, click/drag to scroll.
#[allow(clippy::too_many_arguments)]
fn paint_minimap(
    ui: &egui::Ui,
    app: &mut HelionIde,
    theme: &theme::Theme,
    mini_resp: &egui::Response,
    scroll: &egui::scroll_area::ScrollAreaOutput<()>,
    n_lines: usize,
    lang: highlight::Lang,
) {
    let mini_rect = mini_resp.rect;
    if mini_rect.width() < 8.0 || mini_rect.height() < 8.0 {
        return;
    }
    let painter = ui.painter().with_clip_rect(mini_rect);
    painter.rect_filled(mini_rect, 0.0, theme.panel.gamma_multiply(0.45));
    painter.line_segment(
        [mini_rect.left_top(), mini_rect.left_bottom()],
        egui::Stroke::new(1.0, theme.line),
    );
    let offset_y = scroll.state.offset.y;
    let viewport_h = scroll.inner_rect.height().max(1.0);
    // Use the scroll area's measured content extent. Reconstructing this from
    // line counts drifts from TextEdit's actual galley height and can pin the
    // minimap thumb at the wrong end on short or wrapped files.
    let total_h = scroll.content_size.y.max(1.0);
    let mini_h = mini_rect.height();

    let row_mini = (mini_h / n_lines as f32).min(4.0);
    // The minimap TEXT height: the viewport rectangle must map onto the
    // text, not the widget. For short files the galley is shorter than the
    // widget; mapping onto the widget puts the thumb below the text, falsely
    // claiming end-of-file. (Bars mode always fills the widget.)
    let text_top = mini_rect.min.y + 2.0;
    let mut text_h = mini_h;
    // Bars mode fills the widget from its top edge; text mode is anchored
    // 2px down where the galley is painted.
    let mut text_anchor = text_top;
    if n_lines <= MINIMAP_TEXT_CAP && row_mini >= 2.0 {
        let font_size = (row_mini * 0.85).clamp(2.0, 4.0);
        // Cache the tiny galley: rebuild only when the text changes.
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        app.editor_text.hash(&mut hasher);
        n_lines.hash(&mut hasher);
        let key = (hasher.finish(), font_size.to_bits());
        let cached: Option<Arc<egui::Galley>> = app
            .editor_minimap_cache
            .as_ref()
            .filter(|(k, fb, _)| (*k, *fb) == key)
            .map(|(_, _, g)| g.clone());
        let galley = match cached {
            Some(g) => g,
            None => {
                let g = build_minimap_galley(ui, &app.editor_text, lang, theme, font_size);
                app.editor_minimap_cache = Some((key.0, key.1, g.clone()));
                g
            }
        };
        ui.painter().with_clip_rect(mini_rect).galley(
            egui::pos2(mini_rect.min.x + 4.0, text_top),
            galley.clone(),
            theme.text,
        );
        text_h = galley.size().y.max(1.0);
    } else {
        // Sampled code-bar minimap for large files (~180 rows max, zero solid gray block).
        let spans_id = egui::Id::new("editor_line_spans_cache_v2");
        let spans = ui
            .ctx()
            .data_mut(|d| {
                d.get_temp::<(Option<PathBuf>, usize, Arc<Vec<(usize, usize, bool)>>, usize)>(spans_id)
            })
            .map(|(_, _, s, _)| s);
        let max_bars = (mini_h / 3.0).floor().max(60.0) as usize;
        let step = (n_lines / max_bars).max(1);
        let avail_bar_w = (mini_rect.width() - 8.0).max(8.0);
        let mut idx = 0usize;
        while idx < n_lines {
            let (indent, len, is_comment) = if let Some(sp) = &spans {
                let (s, e, in_blk) = sp.get(idx).copied().unwrap_or((0, 0, false));
                let ls = app.editor_text.get(s..e).unwrap_or("");
                let trimmed = ls.trim_start();
                let ind = ls.len().saturating_sub(trimmed.len()).min(24);
                let cmt = in_blk || trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*');
                (ind, trimmed.len(), cmt)
            } else {
                (2, 32, false)
            };
            if len > 0 {
                let y = mini_rect.min.y + (idx as f32 / n_lines as f32) * mini_h;
                let x_off = (indent as f32 * 1.2).min(avail_bar_w * 0.45);
                let bw = ((len as f32 / 80.0) * (avail_bar_w - x_off)).clamp(4.0, avail_bar_w - x_off);
                let col = if is_comment {
                    theme.faint.gamma_multiply(0.55)
                } else {
                    theme.dim.gamma_multiply(0.65)
                };
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(mini_rect.min.x + 4.0 + x_off, y),
                        egui::vec2(bw, 1.8),
                    ),
                    0.5,
                    col,
                );
            }
            idx += step;
        }
        text_anchor = mini_rect.min.y;
    }

    // Viewport rectangle: scroll fraction mapped onto the minimap TEXT
    // (text_anchor..text_anchor+text_h), never the bare widget. Standard scrollbar
    // math: thumb_pos = offset/content * track, thumb_size = viewport/content
    // * track; at max scroll the thumb bottom meets the text bottom exactly.
    let (v_top, v_h) = minimap_viewport(text_anchor, text_h, offset_y, viewport_h, total_h);
    let v_rect = egui::Rect::from_min_size(
        egui::pos2(mini_rect.min.x + 2.0, v_top),
        egui::vec2(mini_rect.width() - 4.0, v_h),
    );
    painter.rect_filled(v_rect, 2.0, egui::Color32::from_white_alpha(26));
    painter.rect_stroke(
        v_rect,
        2.0,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(60)),
        egui::StrokeKind::Inside,
    );

    // Click / drag to scroll: map the pointer onto the text region, not the
    // widget, so clicks below short-file text do not jump to end-of-file.
    if mini_resp.clicked() || mini_resp.dragged() {
        if let Some(pos) = mini_resp.interact_pointer_pos() {
            let frac = ((pos.y - text_anchor) / text_h).clamp(0.0, 1.0);
            app.editor_mini_scroll = Some((frac * total_h - viewport_h / 2.0).max(0.0));
        }
    }
}

/// Pure viewport-rectangle math for the minimap, extracted for testing.
/// Returns (top_y, height) of the viewport overlay.
///
/// `text_top`/`text_h`: where the minimap text actually is; `offset_y`: the
/// editor's current scroll offset; `viewport_h`: visible editor height;
/// `total_h`: full editor content height. The thumb position is
/// `offset / content * track` and the thumb size is `viewport / content *
/// track`, so the indicator always agrees with the viewport.
fn minimap_viewport(
    text_top: f32,
    text_h: f32,
    offset_y: f32,
    viewport_h: f32,
    total_h: f32,
) -> (f32, f32) {
    let track = text_h.max(1.0);
    let content = total_h.max(1.0);
    let top = text_top + (offset_y / content * track).clamp(0.0, track);
    // Thumb wants to be >= 10px for visibility, but never larger than the
    // track itself: on tiny minimaps (track < 10) clamp(10, track) would
    // panic with min > max.
    let mut h = (viewport_h / content * track).clamp(10.0_f32.min(track), track);
    // If the top clamped (offset past max scroll), shrink the thumb so it
    // never extends past the text bottom: the indicator must never present
    // false state.
    h = h.min((text_top + track - top).max(0.0));
    (top, h)
}

#[cfg(test)]
mod tests {
    use super::minimap_viewport;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.5
    }

    /// Priority repro (Saksham): the minimap viewport thumb must track the
    /// real scroll offset. For each scroll position, the thumb's fractional
    /// position must equal offset / content_height within rounding, and at
    /// max scroll the thumb bottom must meet the text bottom exactly.
    #[test]
    fn minimap_thumb_tracks_scroll_offset() {
        // Short file: 71 lines like complex.sv; text much shorter than widget.
        let text_top = 102.0;
        let text_h = 150.0;
        let total_h = 71.0 * 19.5; // n_lines * ROW_H
        let viewport_h = 600.0;
        let max_offset = total_h - viewport_h;

        for frac in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let offset = frac * max_offset;
            let (v_top, v_h) = minimap_viewport(text_top, text_h, offset, viewport_h, total_h);
            let thumb_frac = (v_top - text_top) / text_h;
            let expect_frac = offset / total_h;
            assert!(
                approx(thumb_frac, expect_frac),
                "offset {offset}: thumb fraction {thumb_frac} != scroll fraction {expect_frac}"
            );
            // Thumb height follows viewport/content * track.
            let expect_h = (viewport_h / total_h * text_h).clamp(10.0_f32.min(text_h), text_h);
            assert!(
                approx(v_h, expect_h),
                "offset {offset}: thumb height {v_h} != {expect_h}"
            );
        }

        // At max scroll the thumb bottom meets the text bottom exactly:
        // no false "end of file" gap, no overshoot.
        let (v_top, v_h) = minimap_viewport(text_top, text_h, max_offset, viewport_h, total_h);
        assert!(
            approx(v_top + v_h, text_top + text_h),
            "max scroll: thumb bottom {} != text bottom {}",
            v_top + v_h,
            text_top + text_h
        );
        // At zero scroll the thumb sits at the text top.
        let (v_top0, _) = minimap_viewport(text_top, text_h, 0.0, viewport_h, total_h);
        assert!(
            approx(v_top0, text_top),
            "zero scroll: thumb top {v_top0} != text top {text_top}"
        );
    }

    /// Thumb never leaves the text region, even for degenerate inputs.
    #[test]
    fn minimap_thumb_stays_inside_text() {
        let (v_top, v_h) = minimap_viewport(50.0, 200.0, 10_000.0, 600.0, 1384.5);
        assert!(v_top >= 50.0 && v_top <= 250.0, "thumb top escaped: {v_top}");
        assert!(v_top + v_h <= 250.5, "thumb bottom escaped: {}", v_top + v_h);
        let (v_top, _) = minimap_viewport(50.0, 200.0, -100.0, 600.0, 1384.5);
        assert!(v_top >= 50.0, "negative offset escaped upward: {v_top}");
    }

    /// Tiny minimap (text_h < 10px): the 10px minimum thumb must not panic
    /// with min > max in the clamp. Fuzz-found via crash-59c2055905bbbe0e
    /// replay (frame 357).
    #[test]
    fn minimap_thumb_tiny_track_no_panic() {
        for text_h in [1.0, 5.0, 9.9] {
            let (v_top, v_h) = minimap_viewport(0.0, text_h, 0.0, 600.0, 1384.5);
            assert!(v_h <= text_h + 0.5, "thumb {v_h} exceeds tiny track {text_h}");
            assert!(v_top >= 0.0, "thumb top escaped: {v_top}");
        }
    }

    /// Repaint determinism across real content/viewport ratios and window
    /// heights. The measured content size is the ScrollAreaOutput's
    /// `content_size.y`, passed directly to this geometry helper.
    #[test]
    fn minimap_thumb_is_bit_identical_across_repaints_and_sizes() {
        let cases: [(f32, f32, f32); 4] = [
            (96.0, 234.0, 510.0),
            (620.0, 12_480.0, 740.0),
            (1050.0, 18_525.0, 430.0),
            (220.0, 2_418.0, 2_000.0),
        ];
        for (text_h, total_h, viewport_h) in cases {
            let max_offset = (total_h - viewport_h).max(0.0_f32);
            for position in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let offset = max_offset * position;
                let first = minimap_viewport(13.0, text_h, offset, viewport_h, total_h);
                for _ in 0..20 {
                    let again = minimap_viewport(13.0, text_h, offset, viewport_h, total_h);
                    assert_eq!(first.0.to_bits(), again.0.to_bits());
                    assert_eq!(first.1.to_bits(), again.1.to_bits());
                }
                let expected_top = 13.0 + offset / total_h * text_h;
                assert!(approx(first.0, expected_top));
                assert!(first.0 >= 13.0);
                assert!(first.0 + first.1 <= 13.0 + text_h + 0.5);
            }
        }
    }
}
