//! Command registry, fuzzy command palette, menu bar, shortcut cheatsheet
//! (P0-2, P1-6, P2 cheatsheet).
//!
//! Single source of truth for invokable IDE commands: the [`Command`] enum.
//! The palette ([`paint_palette`]), the menu bar ([`paint_menu_bar`]), and the
//! shortcut cheatsheet ([`paint_shortcut_cheatsheet`]) all render from it, and
//! every action funnels through `run_command` so palette and menus behave
//! identically.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use helion_gui::chrome::{Activity, Canvas};
use helion_gui::{BottomTab, FlowStep, WorkspaceTab};

use super::layouts::LayoutPreset;
use super::theme::{font_code, font_ui, font_ui_medium, Density, Theme};
use super::toasts::ToastKind;
use super::{
    native_close_project, native_open, native_save_project_as, open_more_workspace,
    recent_menu_label, run_implement, tip, CreateProjectWizard, HelionIde,
};

/// Palette mode: `>` prefix or Cmd+Shift+P forces commands; Cmd+P is go-to-file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PaletteMode {
    #[default]
    Files,
    Commands,
}

/// Open/closed + query + selection for the fuzzy palette overlay.
#[derive(Default)]
pub struct PaletteState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    pub mode: PaletteMode,
}

// ---------------------------------------------------------------------------
// Command registry
// ---------------------------------------------------------------------------

/// Every invokable command in the IDE. Palette rows, menu items, and the
/// cheatsheet all derive their labels/hints from this one enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Workspace(WorkspaceTab),
    ActivityTab(Activity),
    CanvasTab(Canvas),
    Layout(LayoutPreset),
    Implement,
    FlowStep(FlowStep),
    OpenFile,
    NewProject,
    Tcl {
        label: &'static str,
        tcl: &'static str,
    },
    ExampleXdc,
    ToggleWelcome,
    Cheatsheet,
    ToggleTheme,
    ToggleDensity,
    ToggleSidebar,
    ToggleTcl,
    PaletteFiles,
    PaletteCommands,
}

impl Command {
    /// Every command, in palette/menu display order.
    fn all() -> Vec<Command> {
        let mut cmds = Vec::new();
        for tab in WorkspaceTab::ALL {
            cmds.push(Command::Workspace(tab));
        }
        for a in Activity::ALL {
            cmds.push(Command::ActivityTab(a));
        }
        for c in Canvas::ALL {
            cmds.push(Command::CanvasTab(c));
        }
        for p in LayoutPreset::ALL {
            cmds.push(Command::Layout(p));
        }
        cmds.push(Command::Implement);
        for s in FlowStep::ALL {
            cmds.push(Command::FlowStep(s));
        }
        cmds.push(Command::OpenFile);
        cmds.push(Command::NewProject);
        cmds.push(Command::Tcl {
            label: "Report timing",
            tcl: "report_timing",
        });
        cmds.push(Command::Tcl {
            label: "Report utilization",
            tcl: "report_utilization",
        });
        cmds.push(Command::Tcl {
            label: "Create clock 10ns",
            tcl: "create_clock -name clk -period 10 [get_ports clk]",
        });
        cmds.push(Command::ExampleXdc);
        cmds.push(Command::ToggleWelcome);
        cmds.push(Command::Cheatsheet);
        cmds.push(Command::PaletteFiles);
        cmds.push(Command::PaletteCommands);
        cmds.push(Command::ToggleSidebar);
        cmds.push(Command::ToggleTcl);
        cmds.push(Command::ToggleTheme);
        cmds.push(Command::ToggleDensity);
        cmds
    }

    /// Display label for palette rows.
    fn label(self) -> String {
        match self {
            Command::Workspace(tab) => format!("Go to {}", tab.label()),
            Command::ActivityTab(a) => format!("View: {}", a.label()),
            Command::CanvasTab(c) => format!("Canvas: {}", c.label()),
            Command::Layout(p) => format!("Layout: {}", p.label()),
            Command::Implement => "Implement design".to_string(),
            Command::FlowStep(s) => format!("Run {}", s.label()),
            Command::OpenFile => "Open file…".to_string(),
            Command::NewProject => "New project…".to_string(),
            Command::Tcl { label, .. } => label.to_string(),
            Command::ExampleXdc => "Read example constraints (counter.sdc)".to_string(),
            Command::ToggleWelcome => "Show welcome page".to_string(),
            Command::Cheatsheet => "Keyboard shortcuts".to_string(),
            Command::ToggleTheme => "Toggle dark / light theme".to_string(),
            Command::ToggleDensity => "Toggle compact / comfortable density".to_string(),
            Command::ToggleSidebar => "Toggle sidebar".to_string(),
            Command::ToggleTcl => "Toggle Tcl console".to_string(),
            Command::PaletteFiles => "Palette: go to file".to_string(),
            Command::PaletteCommands => "Palette: commands".to_string(),
        }
    }

    /// Keyboard shortcut shown dimmed, e.g. "⌘O".
    fn shortcut_text(self) -> &'static str {
        match self {
            Command::CanvasTab(c) => c.shortcut(),
            Command::Implement => "⌘↩",
            Command::OpenFile => "⌘O",
            Command::Cheatsheet => "⌘K",
            Command::ToggleSidebar => "⌘J",
            Command::ToggleTcl => "⌘`",
            Command::PaletteFiles => "⌘P",
            Command::PaletteCommands => "⇧⌘P",
            _ => "",
        }
    }

    /// Tcl verb shown dimmed when there is no shortcut.
    fn tcl_text(self) -> &'static str {
        match self {
            Command::ActivityTab(a) => a.tcl(),
            Command::Implement => "impl_design",
            Command::FlowStep(s) => s.tcl(),
            Command::OpenFile => "open_source",
            Command::NewProject => "create_project",
            Command::Tcl { tcl, .. } => tcl,
            Command::ExampleXdc => "read_xdc",
            _ => "",
        }
    }

    /// Right-side dimmed hint: shortcut preferred, else the Tcl verb.
    fn hint(self) -> &'static str {
        let sc = self.shortcut_text();
        if sc.is_empty() {
            self.tcl_text()
        } else {
            sc
        }
    }
}

/// Run one registry command. Call only from discrete UI handlers (palette
/// Enter/click, menu items) — never per-frame: flow steps and Tcl exec block.
fn run_command(app: &mut HelionIde, cmd: Command) {
    match cmd {
        Command::Workspace(tab) => open_more_workspace(app, tab),
        Command::ActivityTab(a) => app.set_activity(a),
        Command::CanvasTab(c) => app.set_canvas(c),
        Command::Layout(p) => super::layouts::apply_preset(app, p),
        Command::Implement => run_implement(app),
        Command::FlowStep(step) => run_flow_step(app, step),
        Command::OpenFile => native_open(app),
        Command::NewProject => {
            app.create_wizard = CreateProjectWizard::default();
            app.show_create_project = true;
        }
        Command::Tcl { tcl, .. } => exec_tcl(app, tcl),
        Command::ExampleXdc => {
            let p = helion_device::Device::examples_dir().join("counter.sdc");
            exec_tcl(app, &format!("read_xdc {}", p.display()));
        }
        Command::ToggleWelcome => {
            app.welcome_dismissed = false;
            // The welcome page only shows with no project open; say so instead
            // of silently doing nothing.
            if !super::welcome::should_show(app) {
                app.toasts.push(
                    ToastKind::Warn,
                    "Close the current project to see the welcome page.",
                );
            }
        }
        Command::Cheatsheet => app.show_cheatsheet = true,
        Command::ToggleTheme => toggle_theme(app),
        Command::ToggleDensity => toggle_density(app),
        Command::ToggleSidebar => app.sidebar_hidden = !app.sidebar_hidden,
        Command::ToggleTcl => toggle_tcl(app),
        Command::PaletteFiles => open_palette(app, PaletteMode::Files),
        Command::PaletteCommands => open_palette(app, PaletteMode::Commands),
    }
}

/// Open the palette in `mode`, mirroring `handle_shortcuts`.
fn open_palette(app: &mut HelionIde, mode: PaletteMode) {
    app.palette.open = true;
    app.palette.mode = mode;
    app.palette.selected = 0;
    app.palette.query.clear();
}

/// Run one Tcl recipe through the model; journal + status on success, toast on
/// failure. Blocking — handler-only.
fn exec_tcl(app: &mut HelionIde, tcl: &str) {
    match app.model.exec(tcl) {
        Ok(out) => app.progress = out,
        Err(e) => {
            app.progress = e.clone();
            app.toasts.push(ToastKind::Error, e);
        }
    }
}

/// Run one flow step on the calling thread. Refuses blocked steps with the
/// prerequisite reason instead of surfacing an engine error.
fn run_flow_step(app: &mut HelionIde, step: FlowStep) {
    if let Some(why) = app.model.step_blocked(step) {
        app.progress = why.to_string();
        app.toasts.push(ToastKind::Warn, why);
        return;
    }
    match app.model.run_step(step) {
        Ok(out) => {
            app.progress = out;
            app.toasts
                .push(ToastKind::Ok, format!("{} finished", step.label()));
        }
        Err(e) => {
            app.progress = e.clone();
            app.toasts.push(ToastKind::Error, e);
        }
    }
}

/// Flip dark/light, preserving the row density. Marks the theme dirty so the
/// shell re-applies visuals.
fn toggle_theme(app: &mut HelionIde) {
    let is_dark = app.theme.bg == Theme::dark().bg;
    let mut next = if is_dark {
        Theme::light()
    } else {
        Theme::dark()
    };
    next.density = app.theme.density;
    app.theme = next;
    app.theme_dirty = true;
    super::layouts::save_chrome_state(app);
}

/// Flip compact/comfortable row density. Marks the theme dirty.
fn toggle_density(app: &mut HelionIde) {
    app.theme.density = match app.theme.density {
        Density::Compact => Density::Comfortable,
        Density::Comfortable => Density::Compact,
    };
    app.theme_dirty = true;
    super::layouts::save_chrome_state(app);
}

/// Toggle the Tcl console, focusing its input when shown.
fn toggle_tcl(app: &mut HelionIde) {
    app.show_tcl = !app.show_tcl;
    if app.show_tcl {
        app.tcl_focus = true;
        app.model.bottom_tab = BottomTab::Tcl;
    }
}

// ---------------------------------------------------------------------------
// Fuzzy palette
// ---------------------------------------------------------------------------

/// Subsequence fuzzy match score (higher = better); `None` when `query` is not
/// a subsequence of `target`. Case-insensitive; rewards word-boundary and
/// consecutive hits, penalizes gaps and long targets.
fn fuzzy_score(query: &str, target: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let t: Vec<char> = target.to_lowercase().chars().collect();
    let mut ti = 0usize;
    let mut score: i64 = 0;
    let mut run: i64 = 0;
    for qc in q {
        let mut hit = false;
        while ti < t.len() {
            let boundary = ti == 0 || !t[ti - 1].is_alphanumeric();
            let tc = t[ti];
            ti += 1;
            if tc == qc {
                run += 1;
                score += 8 + run * 4;
                if boundary {
                    score += 12;
                }
                hit = true;
                break;
            }
            run = 0;
            score -= 2;
        }
        if !hit {
            return None;
        }
    }
    score -= t.len() as i64 / 4;
    Some(score)
}

/// What a palette row activates.
#[derive(Clone)]
enum PalettePick {
    File(PathBuf),
    Cmd(Command),
}

/// One rendered palette row: owned label/hint/hover plus its payload.
struct PaletteRow {
    pick: PalettePick,
    label: String,
    hint: String,
    hover: String,
    score: i64,
}

/// File name for the row label; full path when there is no file name.
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Effective mode + match text: a leading `>` forces command mode.
fn effective_query(palette: &PaletteState) -> (PaletteMode, String) {
    let mut mode = palette.mode;
    let mut q = palette.query.as_str();
    if q.starts_with('>') {
        mode = PaletteMode::Commands;
        q = q[1..].trim_start();
    }
    (mode, q.to_string())
}

/// Discrete palette keys, consumed before the input widget sees them so Enter
/// never lands in the text box. Returns (nav_delta, activate, cancel).
fn palette_keys(ctx: &egui::Context) -> (i32, bool, bool) {
    let mut nav = 0;
    let mut activate = false;
    let mut cancel = false;
    ctx.input_mut(|i| {
        if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
            cancel = true;
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
            nav -= 1;
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
            nav += 1;
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
            activate = true;
        }
    });
    (nav, activate, cancel)
}

/// Build, filter, and score the row list for the current mode/query.
fn palette_items(app: &HelionIde, mode: PaletteMode, query: &str) -> Vec<PaletteRow> {
    match mode {
        PaletteMode::Files => file_rows(app, query),
        PaletteMode::Commands => command_rows(query),
    }
}

/// Files rows: open file, then project sources, then recents (deduped).
fn file_rows(app: &HelionIde, query: &str) -> Vec<PaletteRow> {
    let mut entries: Vec<(PathBuf, &'static str)> = Vec::new();
    if let Some(sel) = app.model.selected_source.as_deref() {
        entries.push((PathBuf::from(sel), "open"));
    }
    for s in &app.model.tree.sources {
        entries.push((PathBuf::from(s), "source"));
    }
    for r in &app.recent {
        entries.push((r.clone(), "recent"));
    }
    let mut rows: Vec<PaletteRow> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for (path, tag) in entries {
        if seen.contains(&path) {
            continue;
        }
        seen.push(path.clone());
        let haystack = path.to_string_lossy();
        let Some(score) = fuzzy_score(query, &haystack) else {
            continue;
        };
        let label = file_label(&path);
        rows.push(PaletteRow {
            hover: tip(&label, tag, &haystack),
            hint: tag.to_string(),
            pick: PalettePick::File(path),
            label,
            score,
        });
    }
    rows.sort_by(|a, b| b.score.cmp(&a.score));
    rows
}

/// Command rows: the whole registry, fuzzy-filtered by label.
fn command_rows(query: &str) -> Vec<PaletteRow> {
    let mut rows: Vec<PaletteRow> = Vec::new();
    for cmd in Command::all() {
        let label = cmd.label();
        let Some(score) = fuzzy_score(query, &label) else {
            continue;
        };
        rows.push(PaletteRow {
            hover: tip(&label, cmd.shortcut_text(), cmd.tcl_text()),
            hint: cmd.hint().to_string(),
            pick: PalettePick::Cmd(cmd),
            label,
            score,
        });
    }
    rows.sort_by(|a, b| b.score.cmp(&a.score));
    rows
}

/// Palette header: mode badge + singleline query box (focus grabbed on open).
fn paint_palette_header(
    ui: &mut egui::Ui,
    app: &mut HelionIde,
    theme: &Theme,
    mode: PaletteMode,
    just_opened: bool,
) {
    ui.horizontal(|ui| {
        let (name, keys) = match mode {
            PaletteMode::Files => ("FILES", "⌘P"),
            PaletteMode::Commands => ("COMMANDS", "⇧⌘P"),
        };
        ui.label(
            RichText::new(name)
                .font(font_ui_medium(11.0))
                .color(theme.accent_dim),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(keys).font(font_code(11.0)).color(theme.faint));
        });
    });
    ui.add_space(2.0);
    let hint = match mode {
        PaletteMode::Files => "Type to filter files — prefix with > for commands",
        PaletteMode::Commands => "Type to filter commands",
    };
    let resp = ui.add(
        egui::TextEdit::singleline(&mut app.palette.query)
            .desired_width(f32::INFINITY)
            .hint_text(hint)
            .font(font_ui(14.0)),
    );
    if just_opened {
        resp.request_focus();
    }
}

const PALETTE_ROW_H: f32 = 30.0;

/// One palette row: label left, dimmed hint right. Hover-selects; returns true
/// when clicked.
fn paint_palette_row(
    ui: &mut egui::Ui,
    app: &mut HelionIde,
    theme: &Theme,
    row: &PaletteRow,
    index: usize,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), PALETTE_ROW_H),
        egui::Sense::click(),
    );
    let resp = resp.on_hover_text(&row.hover);
    if resp.hovered() {
        app.palette.selected = index;
    }
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if index == app.palette.selected {
            p.rect_filled(rect, 4.0, theme.sel);
        } else if resp.hovered() {
            p.rect_filled(rect, 4.0, theme.surface_hover);
        }
        p.text(
            egui::pos2(rect.left() + 10.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &row.label,
            font_ui(13.0),
            theme.text,
        );
        p.text(
            egui::pos2(rect.right() - 10.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            &row.hint,
            font_code(11.0),
            theme.faint,
        );
    }
    resp.clicked()
}

/// Palette result list. Applies keyboard nav (wrapping) and returns the
/// clicked row's payload, if any.
fn paint_palette_rows(
    ui: &mut egui::Ui,
    app: &mut HelionIde,
    theme: &Theme,
    rows: &[PaletteRow],
    nav: i32,
) -> Option<PalettePick> {
    let n = rows.len();
    if n == 0 {
        ui.label(
            RichText::new("No matches")
                .font(font_ui(12.0))
                .color(theme.faint),
        );
        return None;
    }
    let mut sel = app.palette.selected.min(n - 1);
    if nav != 0 {
        sel = (sel as i32 + nav).rem_euclid(n as i32) as usize;
    }
    app.palette.selected = sel;
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .id_salt("palette_results_scroll")
        .max_height(15.0 * PALETTE_ROW_H)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (i, row) in rows.iter().enumerate() {
                if paint_palette_row(ui, app, theme, row, i) {
                    clicked = Some(row.pick.clone());
                }
            }
        });
    clicked
}

/// Fuzzy command palette overlay (VS Code Quick Pick style), centered-top.
/// Keyboard: type to filter, Up/Down to move, Enter to run, Esc to close.
pub fn paint_palette(ctx: &egui::Context, app: &mut HelionIde) {
    // One-shot focus tracking without touching `PaletteState`'s shape.
    let focus_id = egui::Id::new("palette_focus_seen");
    if !app.palette.open {
        ctx.data_mut(|d| d.insert_temp(focus_id, false));
        return;
    }
    let was_open: bool = ctx.data(|d| d.get_temp(focus_id)).unwrap_or(false);
    ctx.data_mut(|d| d.insert_temp(focus_id, true));
    let just_opened = !was_open;

    let (nav, activate, cancel) = palette_keys(ctx);
    let (mode, query) = effective_query(&app.palette);
    let theme = app.theme;
    let rows = palette_items(app, mode, &query);

    let mut pick: Option<PalettePick> = None;
    egui::Area::new(egui::Id::new("command_palette"))
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme.surface)
                .stroke(egui::Stroke::new(1.0, theme.line))
                .corner_radius(8.0)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(616.0);
                    paint_palette_header(ui, app, &theme, mode, just_opened);
                    ui.add_space(6.0);
                    if let Some(p) = paint_palette_rows(ui, app, &theme, &rows, nav) {
                        pick = Some(p);
                    }
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("↑↓ navigate · Enter run · Esc close · > commands")
                            .font(font_ui(11.0))
                            .color(theme.faint),
                    );
                });
        });

    if cancel {
        app.palette.open = false;
        return;
    }
    let run = pick.or_else(|| {
        if activate {
            rows.get(app.palette.selected).map(|r| r.pick.clone())
        } else {
            None
        }
    });
    if let Some(p) = run {
        app.palette.open = false;
        match p {
            PalettePick::File(path) => app.open_path(&path),
            PalettePick::Cmd(cmd) => run_command(app, cmd),
        }
    }
}

// ---------------------------------------------------------------------------
// Menu bar
// ---------------------------------------------------------------------------

/// One menu row: label left, shortcut/Tcl hint dimmed right.
fn menu_row(ui: &mut egui::Ui, theme: &Theme, label: &str, hint: &str) -> egui::Response {
    ui.horizontal(|ui| {
        ui.set_width(ui.available_width());
        let r = ui.button(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(hint).font(font_ui(11.0)).color(theme.faint));
        });
        r
    })
    .inner
}

/// Menu row wired to a registry command: runs it and closes the menu.
fn menu_cmd(ui: &mut egui::Ui, theme: &Theme, app: &mut HelionIde, cmd: Command, label: &str) {
    let hover = tip(label, cmd.shortcut_text(), cmd.tcl_text());
    if menu_row(ui, theme, label, cmd.hint())
        .on_hover_text(hover)
        .clicked()
    {
        run_command(app, cmd);
        ui.close();
    }
}

/// Shared Tcl recipe rows for the Flow and Tools menus.
fn menu_tcl_recipes(ui: &mut egui::Ui, theme: &Theme, app: &mut HelionIde) {
    ui.label(RichText::new("Tcl").font(font_ui(11.0)).color(theme.faint));
    for cmd in [
        Command::Tcl {
            label: "Report timing",
            tcl: "report_timing",
        },
        Command::Tcl {
            label: "Report utilization",
            tcl: "report_utilization",
        },
        Command::Tcl {
            label: "Create clock 10ns",
            tcl: "create_clock -name clk -period 10 [get_ports clk]",
        },
        Command::ExampleXdc,
    ] {
        let label = cmd.label();
        menu_cmd(ui, theme, app, cmd, &label);
    }
}

fn menu_file(ui: &mut egui::Ui, app: &mut HelionIde, theme: &Theme) {
    menu_cmd(ui, theme, app, Command::NewProject, "New project…");
    menu_cmd(ui, theme, app, Command::OpenFile, "Open…");
    ui.separator();
    if app.recent.is_empty() {
        ui.label(
            RichText::new("No recent files")
                .font(font_ui(12.0))
                .color(theme.faint),
        );
    } else {
        let recent: Vec<PathBuf> = app.recent.iter().take(6).cloned().collect();
        for p in recent {
            let label = recent_menu_label(&p);
            if menu_row(ui, theme, &label, "").clicked() {
                app.open_path_async(&p);
                ui.close();
            }
        }
    }
    ui.separator();
    if menu_row(ui, theme, "Save project as…", "").clicked() {
        native_save_project_as(app);
        ui.close();
    }
    if menu_row(ui, theme, "Close project", "").clicked() {
        native_close_project(app);
        ui.close();
    }
}

fn menu_edit(ui: &mut egui::Ui, app: &mut HelionIde, theme: &Theme) {
    menu_cmd(
        ui,
        theme,
        app,
        Command::Workspace(WorkspaceTab::Find),
        "Find in files",
    );
    ui.separator();
    menu_cmd(ui, theme, app, Command::ToggleTcl, "Tcl console");
}

fn menu_view(ui: &mut egui::Ui, app: &mut HelionIde, theme: &Theme) {
    for c in Canvas::ALL {
        menu_cmd(ui, theme, app, Command::CanvasTab(c), c.label());
    }
    ui.separator();
    for a in Activity::ALL {
        menu_cmd(ui, theme, app, Command::ActivityTab(a), a.label());
    }
    ui.separator();
    for p in LayoutPreset::ALL {
        menu_cmd(ui, theme, app, Command::Layout(p), p.label());
    }
    ui.separator();
    menu_cmd(ui, theme, app, Command::ToggleSidebar, "Sidebar");
    menu_cmd(ui, theme, app, Command::ToggleWelcome, "Welcome page");
    ui.separator();
    menu_cmd(ui, theme, app, Command::ToggleTheme, "Dark / light theme");
    menu_cmd(
        ui,
        theme,
        app,
        Command::ToggleDensity,
        "Compact / comfortable rows",
    );
    ui.separator();
    menu_cmd(ui, theme, app, Command::Cheatsheet, "Keyboard shortcuts…");
}

fn menu_flow(ui: &mut egui::Ui, app: &mut HelionIde, theme: &Theme) {
    menu_cmd(ui, theme, app, Command::Implement, "Implement");
    ui.separator();
    ui.label(
        RichText::new("Run step")
            .font(font_ui(11.0))
            .color(theme.faint),
    );
    for step in FlowStep::ALL {
        let blocked = app.model.step_blocked(step);
        let label = format!("Run {}", step.label());
        ui.add_enabled_ui(blocked.is_none(), |ui| {
            let mut r = menu_row(ui, theme, &label, step.tcl());
            if let Some(why) = blocked {
                r = r.on_hover_text(why);
            }
            if r.clicked() {
                run_command(app, Command::FlowStep(step));
                ui.close();
            }
        });
    }
    ui.separator();
    menu_tcl_recipes(ui, theme, app);
}

fn menu_tools(ui: &mut egui::Ui, app: &mut HelionIde, theme: &Theme) {
    menu_cmd(ui, theme, app, Command::PaletteFiles, "Palette: go to file");
    menu_cmd(
        ui,
        theme,
        app,
        Command::PaletteCommands,
        "Palette: commands",
    );
    ui.separator();
    menu_cmd(ui, theme, app, Command::ToggleTcl, "Tcl console");
    ui.separator();
    menu_tcl_recipes(ui, theme, app);
}

fn menu_help(ui: &mut egui::Ui, app: &mut HelionIde, theme: &Theme) {
    menu_cmd(ui, theme, app, Command::Cheatsheet, "Keyboard shortcuts…");
    menu_cmd(ui, theme, app, Command::ToggleWelcome, "Welcome page");
}

/// Menu bar (File / Edit / View / Flow / Tools / Help), painted inside the
/// toolbar row via `ui.menu_button`. Every item calls the same `run_command`
/// as the palette. Additive only — no existing toolbar behavior changes.
pub fn paint_menu_bar(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    ui.horizontal(|ui| {
        ui.menu_button("File", |ui| menu_file(ui, app, &theme));
        ui.menu_button("Edit", |ui| menu_edit(ui, app, &theme));
        ui.menu_button("View", |ui| menu_view(ui, app, &theme));
        ui.menu_button("Flow", |ui| menu_flow(ui, app, &theme));
        ui.menu_button("Tools", |ui| menu_tools(ui, app, &theme));
        ui.menu_button("Help", |ui| menu_help(ui, app, &theme));
    });
}

// ---------------------------------------------------------------------------
// Shortcut cheatsheet
// ---------------------------------------------------------------------------

/// The true global keybindings, mirroring `handle_shortcuts` in the parent.
const SHORTCUTS: &[(&str, &str)] = &[
    ("⌘O", "Open file or project…"),
    ("⌘↩", "Implement: synthesis → opt → place → route"),
    ("⌘1", "Canvas: Editor"),
    ("⌘2", "Canvas: Device"),
    ("⌘3", "Canvas: Timing"),
    ("⌘J", "Toggle sidebar"),
    ("⌘`", "Toggle Tcl console (focuses its input)"),
    ("⌘P", "Command palette: go to file"),
    ("⇧⌘P", "Command palette: commands"),
    ("⌘K", "Toggle this cheatsheet"),
    ("⌘S", "Save open file (Editor canvas)"),
    ("Alt+Z", "Toggle soft wrap (Editor canvas)"),
];

/// Keys that only work while the palette is open.
const PALETTE_KEYS: &[(&str, &str)] = &[
    ("↑ / ↓", "Move selection (wraps)"),
    ("Enter", "Run the selected item"),
    ("Esc", "Close the palette"),
    (">", "Type as first character: switch to command mode"),
];

/// One cheatsheet section, filtered by the search box.
fn paint_shortcut_rows(
    ui: &mut egui::Ui,
    theme: &Theme,
    title: &str,
    items: &[(&str, &str)],
    query: &str,
) {
    let shown: Vec<(&str, &str)> = items
        .iter()
        .copied()
        .filter(|(k, d)| query.is_empty() || fuzzy_score(query, &format!("{k} {d}")).is_some())
        .collect();
    if shown.is_empty() {
        return;
    }
    ui.label(
        RichText::new(title)
            .font(font_ui_medium(12.0))
            .color(theme.dim),
    );
    ui.add_space(2.0);
    for (keys, desc) in shown {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(keys)
                    .font(font_code(12.0))
                    .color(theme.accent_dim),
            );
            ui.add_space(8.0);
            ui.label(RichText::new(desc).font(font_ui(12.0)).color(theme.text));
        });
    }
    ui.add_space(8.0);
}

/// Cheatsheet card header: title, Close button, and the filter box.
/// Returns true when Close was clicked.
fn paint_cheatsheet_header(ui: &mut egui::Ui, theme: &Theme, query: &mut String) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Keyboard shortcuts")
                .font(font_ui_medium(15.0))
                .color(theme.text),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Close").clicked() {
                close = true;
            }
        });
    });
    ui.add_space(6.0);
    ui.add(
        egui::TextEdit::singleline(query)
            .desired_width(f32::INFINITY)
            .hint_text("Filter shortcuts…")
            .font(font_ui(13.0)),
    );
    ui.add_space(8.0);
    close
}

/// Shortcut cheatsheet overlay: searchable keybinding list. Esc or the toggle
/// (⌘K) closes it.
pub fn paint_shortcut_cheatsheet(ctx: &egui::Context, app: &mut HelionIde) {
    if !app.show_cheatsheet {
        return;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        app.show_cheatsheet = false;
        return;
    }
    let theme = app.theme;
    let id = egui::Id::new("cheatsheet_query");
    let mut query: String = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let mut close = false;
    egui::Area::new(egui::Id::new("shortcut_cheatsheet"))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme.surface)
                .stroke(egui::Stroke::new(1.0, theme.line))
                .corner_radius(8.0)
                .inner_margin(egui::Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_width(520.0);
                    if paint_cheatsheet_header(ui, &theme, &mut query) {
                        close = true;
                    }
                    egui::ScrollArea::vertical()
                        .id_salt("cheatsheet_scroll")
                        .max_height(400.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            paint_shortcut_rows(ui, &theme, "Global", SHORTCUTS, &query);
                            paint_shortcut_rows(
                                ui,
                                &theme,
                                "In the command palette",
                                PALETTE_KEYS,
                                &query,
                            );
                        });
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Esc closes · ⌘K toggles this panel")
                            .font(font_ui(11.0))
                            .color(theme.faint),
                    );
                });
        });
    ctx.data_mut(|d| d.insert_temp(id, query));
    if close {
        app.show_cheatsheet = false;
    }
}
