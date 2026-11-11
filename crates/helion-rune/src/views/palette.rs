//! Fuzzy command palette + shortcut cheatsheet (VS Code Quick Pick style).
//!
//! Sweep port of the old IDE's `commands.rs` (registry, fuzzy palette,
//! cheatsheet), re-expressed on Rune: the registry holds new-IDE commands
//! (center panes, real run launches, dialogs, Tcl recipes), the overlay is a
//! top-anchored modal panel, and the global chords arrive via
//! `AppSpec::on_key` ([`app_key`]).
//!
//! Faithfulness notes (old core, new IA):
//! - `>` prefix forces command mode; `Esc`/`Up`/`Down`/`Enter` behave as the
//!   old `palette_keys`; selection wraps (`rem_euclid`).
//! - [`fuzzy_score`] is the old subsequence scorer verbatim (boundary and
//!   run bonuses, gap penalty, length penalty).
//! - Files mode lists open editor tabs (old: open file + project sources +
//!   recents; the new IDE has no unopened-file inventory yet, so only tabs).
//! - Commands with no new-IDE counterpart are omitted, never faked:
//!   `ExampleXdc` (no `read_xdc` Tcl verb yet), `ToggleSidebar`,
//!   `ToggleDensity`. The cheatsheet lists only true bindings.
//! - Every pick funnels through [`run_command`], like the old `run_command`.

use rune::{Color, El, Key, KeyEvent, ScrollSpec, TextEdit, Tx, col, el, row, text, text_input};
use rune_kit::{Button, ButtonVariant};

use crate::state::{AppState, CenterPane, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;

/// Quick Pick panel width (old `chrome::PALETTE_WIDTH`).
pub const PALETTE_WIDTH: f32 = 616.0;
/// Cheatsheet card width (old `chrome::CHEATSHEET_WIDTH`).
pub const CHEATSHEET_WIDTH: f32 = 520.0;
/// One result-row height (old `PALETTE_ROW_H`).
const ROW_H: f32 = 30.0;
/// Panel drop from the window top (old `CENTER_TOP + 82` anchor).
const TOP_GAP: f32 = 82.0;
/// Backdrop dim alpha (old `from_black_alpha(110)`).
const SCRIM_A: f32 = 0.43;
/// Result list cap (old `15 rows × 30pt` scroll cap).
const LIST_MAX_H: f32 = 450.0;
/// Cheatsheet scroll cap (old `max_height(400)`).
const CHEAT_MAX_H: f32 = 400.0;

/// Palette mode: `>` prefix or Cmd+Shift+P forces commands; Cmd+P is files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PaletteMode {
    #[default]
    Files,
    Commands,
}

/// Open/closed + query + selection, plus the cheatsheet card state.
#[derive(Clone, Debug, Default)]
pub struct PaletteState {
    pub open: bool,
    pub mode: PaletteMode,
    pub query: TextEdit,
    pub selected: usize,
    pub cheatsheet: bool,
    pub cheat_query: TextEdit,
}

/// Every invokable palette command. Rows, hints, and [`run_command`] all
/// derive from this one enum, mirroring the old `Command` registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteCommand {
    GoTo(CenterPane),
    RunSynth,
    RunImpl,
    GenBitstream,
    OpenFile,
    NewProject,
    Tcl {
        label: &'static str,
        tcl: &'static str,
    },
    ShowStart,
    Cheatsheet,
    ToggleTheme,
    ToggleConsole,
    PaletteFiles,
    PaletteCommands,
}

/// Center panes in palette display order.
const PANES: [CenterPane; 11] = [
    CenterPane::Summary,
    CenterPane::Editor,
    CenterPane::Timing,
    CenterPane::Start,
    CenterPane::Schematic,
    CenterPane::Device,
    CenterPane::Wave,
    CenterPane::Sim,
    CenterPane::Debug,
    CenterPane::Program,
    CenterPane::Reports,
];

/// Tcl recipes shared by the registry (old `menu_tcl_recipes`).
const TCL_RECIPES: [(&str, &str); 3] = [
    ("Report timing", "report_timing"),
    ("Report utilization", "report_utilization"),
    (
        "Create clock 10ns",
        "create_clock -name clk -period 10 [get_ports clk]",
    ),
];

fn pane_label(pane: CenterPane) -> &'static str {
    match pane {
        CenterPane::Summary => "Summary",
        CenterPane::Editor => "Editor",
        CenterPane::Timing => "Timing",
        CenterPane::Start => "Start Page",
        CenterPane::Schematic => "Schematic",
        CenterPane::Device => "Device",
        CenterPane::Wave => "Waveform",
        CenterPane::Sim => "Simulation",
        CenterPane::Debug => "Debug",
        CenterPane::Program => "Hardware Manager",
        CenterPane::Reports => "Reports",
        CenterPane::Bitstream => "Bitstream",
    }
}

impl PaletteCommand {
    /// Every command, in palette display order.
    pub fn all() -> Vec<PaletteCommand> {
        let mut cmds = Vec::new();
        for pane in PANES {
            cmds.push(PaletteCommand::GoTo(pane));
        }
        cmds.push(PaletteCommand::RunSynth);
        cmds.push(PaletteCommand::RunImpl);
        cmds.push(PaletteCommand::GenBitstream);
        cmds.push(PaletteCommand::OpenFile);
        cmds.push(PaletteCommand::NewProject);
        for (label, tcl) in TCL_RECIPES {
            cmds.push(PaletteCommand::Tcl { label, tcl });
        }
        cmds.push(PaletteCommand::ShowStart);
        cmds.push(PaletteCommand::Cheatsheet);
        cmds.push(PaletteCommand::PaletteFiles);
        cmds.push(PaletteCommand::PaletteCommands);
        cmds.push(PaletteCommand::ToggleConsole);
        cmds.push(PaletteCommand::ToggleTheme);
        cmds
    }

    /// Display label for palette rows.
    pub fn label(self) -> String {
        match self {
            PaletteCommand::GoTo(pane) => format!("Go to {}", pane_label(pane)),
            PaletteCommand::RunSynth => "Run Synthesis".into(),
            PaletteCommand::RunImpl => "Run Implementation".into(),
            PaletteCommand::GenBitstream => "Generate Bitstream".into(),
            PaletteCommand::OpenFile => "Open file…".into(),
            PaletteCommand::NewProject => "New project…".into(),
            PaletteCommand::Tcl { label, .. } => label.to_string(),
            PaletteCommand::ShowStart => "Show start page".into(),
            PaletteCommand::Cheatsheet => "Keyboard shortcuts".into(),
            PaletteCommand::ToggleTheme => "Toggle dark / light theme".into(),
            PaletteCommand::ToggleConsole => "Toggle Tcl console".into(),
            PaletteCommand::PaletteFiles => "Palette: go to file".into(),
            PaletteCommand::PaletteCommands => "Palette: commands".into(),
        }
    }

    /// Keyboard shortcut shown dimmed, e.g. "⌘O".
    pub fn shortcut_text(self) -> &'static str {
        match self {
            PaletteCommand::GoTo(CenterPane::Editor) => "⌘1",
            PaletteCommand::GoTo(CenterPane::Device) => "⌘2",
            PaletteCommand::GoTo(CenterPane::Timing) => "⌘3",
            PaletteCommand::RunImpl => "⌘↩",
            PaletteCommand::OpenFile => "⌘O",
            PaletteCommand::Cheatsheet => "⌘K",
            PaletteCommand::ToggleConsole => "⌘`",
            PaletteCommand::PaletteFiles => "⌘P",
            PaletteCommand::PaletteCommands => "⇧⌘P",
            _ => "",
        }
    }

    /// Tcl verb shown dimmed when there is no shortcut.
    pub fn tcl_text(self) -> &'static str {
        match self {
            PaletteCommand::RunSynth => "launch_runs synth_1",
            PaletteCommand::RunImpl => "launch_runs impl_1",
            PaletteCommand::GenBitstream => "launch_runs -to_step write_bitstream",
            PaletteCommand::OpenFile => "open_project",
            PaletteCommand::NewProject => "create_project",
            PaletteCommand::Tcl { tcl, .. } => tcl,
            PaletteCommand::ToggleTheme => "set_theme",
            _ => "",
        }
    }

    /// Right-side dimmed hint: shortcut preferred, else the Tcl verb.
    pub fn hint(self) -> &'static str {
        let sc = self.shortcut_text();
        if sc.is_empty() { self.tcl_text() } else { sc }
    }
}

/// Subsequence fuzzy match score (higher = better); `None` when `query` is
/// not a subsequence of `target`. Case-insensitive; rewards word-boundary
/// and consecutive hits, penalizes gaps and long targets. Verbatim from the
/// old `fuzzy_score`.
pub fn fuzzy_score(query: &str, target: &str) -> Option<i64> {
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

/// What a palette row activates: an editor tab or a registry command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PalettePick {
    File(usize),
    Cmd(PaletteCommand),
}

/// One rendered palette row: label, dimmed hint, payload, fuzzy score.
#[derive(Clone, Debug)]
pub struct PaletteRow {
    pub pick: PalettePick,
    pub label: String,
    pub hint: String,
    pub score: i64,
}

/// Effective mode + match text: a leading `>` forces command mode.
pub fn effective_query(mode: PaletteMode, query: &str) -> (PaletteMode, &str) {
    let mut mode = mode;
    let mut q = query;
    if let Some(rest) = q.strip_prefix('>') {
        mode = PaletteMode::Commands;
        q = rest.trim_start();
    }
    (mode, q)
}

/// Files rows: open editor tabs, fuzzy-filtered by path (or tab name when
/// the buffer was never saved to disk). The active tab tags `open`, saved
/// tabs `source`, unsaved buffers `buffer`.
pub fn file_rows(s: &AppState, query: &str) -> Vec<PaletteRow> {
    let mut rows = Vec::new();
    for (i, tab) in s.editor.tabs.iter().enumerate() {
        let haystack = s
            .editor
            .paths
            .get(tab)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| tab.clone());
        let Some(score) = fuzzy_score(query, &haystack) else {
            continue;
        };
        let label = std::path::PathBuf::from(tab)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| tab.clone());
        let hint = if i == s.editor.active {
            "open"
        } else if s.editor.paths.contains_key(tab) {
            "source"
        } else {
            "buffer"
        };
        rows.push(PaletteRow {
            pick: PalettePick::File(i),
            label,
            hint: hint.into(),
            score,
        });
    }
    rows.sort_by(|a, b| b.score.cmp(&a.score));
    rows
}

/// Command rows: the whole registry, fuzzy-filtered by label.
pub fn command_rows(query: &str) -> Vec<PaletteRow> {
    let mut rows = Vec::new();
    for cmd in PaletteCommand::all() {
        let label = cmd.label();
        let Some(score) = fuzzy_score(query, &label) else {
            continue;
        };
        rows.push(PaletteRow {
            pick: PalettePick::Cmd(cmd),
            label,
            hint: cmd.hint().to_string(),
            score,
        });
    }
    rows.sort_by(|a, b| b.score.cmp(&a.score));
    rows
}

/// Rows for the palette's current mode/query (honoring the `>` override).
pub fn current_rows(s: &AppState) -> Vec<PaletteRow> {
    let (mode, q) = effective_query(s.palette.mode, &s.palette.query.text);
    match mode {
        PaletteMode::Files => file_rows(s, q),
        PaletteMode::Commands => command_rows(q),
    }
}

/// Open the palette in `mode`: the cheatsheet closes, the query clears, the
/// selection parks on the first row (old `open_palette`).
pub fn open_palette(s: &mut AppState, mode: PaletteMode) {
    s.palette.open = true;
    s.palette.cheatsheet = false;
    s.palette.mode = mode;
    s.palette.selected = 0;
    s.palette.query.clear();
    s.palette.query.set_caret(0);
}

/// Close the palette (the query keeps its text for the next open).
pub fn close_palette(s: &mut AppState) {
    s.palette.open = false;
}

/// Open the cheatsheet card (the palette closes; the filter persists).
pub fn open_cheatsheet(s: &mut AppState) {
    s.palette.cheatsheet = true;
    s.palette.open = false;
}

/// Close the cheatsheet card.
pub fn close_cheatsheet(s: &mut AppState) {
    s.palette.cheatsheet = false;
}

/// Clamp the selection onto the current rows (typed queries shrink the
/// list; the old view clamps every paint).
pub fn clamp_selection(s: &mut AppState) {
    let n = current_rows(s).len();
    s.palette.selected = s.palette.selected.min(n.saturating_sub(1));
}

/// Move the selection, wrapping at the ends (old `rem_euclid` nav).
pub fn move_selection(s: &mut AppState, delta: i32) {
    let n = current_rows(s).len();
    if n == 0 {
        s.palette.selected = 0;
        return;
    }
    let sel = (s.palette.selected.min(n - 1) as i32 + delta).rem_euclid(n as i32);
    s.palette.selected = sel as usize;
}

/// Activate the selected row: files switch tabs, commands run through
/// [`run_command`]. The palette closes first, like the old overlay.
pub fn activate_selected(s: &mut AppState, tx: &mut Tx) {
    let rows = current_rows(s);
    if rows.is_empty() {
        return;
    }
    let sel = s.palette.selected.min(rows.len() - 1);
    activate_pick(s, rows[sel].pick.clone(), tx);
}

/// Activate one row payload by index (row taps).
pub fn activate_row(s: &mut AppState, index: usize, tx: &mut Tx) {
    let rows = current_rows(s);
    let Some(row) = rows.get(index) else {
        return;
    };
    activate_pick(s, row.pick.clone(), tx);
}

fn activate_pick(s: &mut AppState, pick: PalettePick, tx: &mut Tx) {
    close_palette(s);
    match pick {
        PalettePick::File(i) => crate::views::editor::select_tab(s, i),
        PalettePick::Cmd(cmd) => run_command(s, cmd, tx),
    }
}

/// Run one registry command. Handler-only (palette Enter/click, menu rows,
/// global chords): run launches and Tcl exec block.
pub fn run_command(s: &mut AppState, cmd: PaletteCommand, tx: &mut Tx) {
    match cmd {
        PaletteCommand::GoTo(pane) => s.navigate(pane),
        PaletteCommand::RunSynth => crate::runs::launch(s, crate::runs::RunKind::Synth),
        PaletteCommand::RunImpl => crate::runs::launch(s, crate::runs::RunKind::Impl),
        PaletteCommand::GenBitstream => crate::runs::launch_verb(
            s,
            crate::runs::RunKind::Impl,
            "launch_runs -to_step write_bitstream",
        ),
        PaletteCommand::OpenFile => crate::pickers::open_project(tx),
        PaletteCommand::NewProject => {
            s.dialogs.open(crate::views::dialogs::DialogId::NewProject);
        }
        PaletteCommand::Tcl { tcl, .. } => crate::tcl::eval_command(s, tcl),
        PaletteCommand::ShowStart => s.navigate(CenterPane::Start),
        PaletteCommand::Cheatsheet => open_cheatsheet(s),
        PaletteCommand::ToggleTheme => toggle_theme(s),
        PaletteCommand::ToggleConsole => {
            s.console.collapsed = !s.console.collapsed;
        }
        PaletteCommand::PaletteFiles => open_palette(s, PaletteMode::Files),
        PaletteCommand::PaletteCommands => open_palette(s, PaletteMode::Commands),
    }
}

/// Flip the theme pin between light and dark (the OS-follow default pins
/// dark first), echoing the `set_theme` verb like the View-menu rows.
fn toggle_theme(s: &mut AppState) {
    use crate::theme::ThemeMode;
    s.theme = match s.theme {
        ThemeMode::Dark => ThemeMode::Light,
        _ => ThemeMode::Dark,
    };
    let verb = match s.theme {
        ThemeMode::Dark => "set_theme dark",
        _ => "set_theme light",
    };
    s.console.lines.push(ConsoleLine {
        text: verb.into(),
        sev: MsgSev::Info,
    });
}

/// Save the active editor buffer, echoing like the File-menu Save row.
fn save_open_file(s: &mut AppState) {
    match crate::views::editor::save_active(s) {
        Ok(path) => s.console.lines.push(ConsoleLine {
            text: format!("save_file {}", path.display()),
            sev: MsgSev::Info,
        }),
        Err(e) => s.console.lines.push(ConsoleLine {
            text: format!("ERROR: save_file: {e}"),
            sev: MsgSev::Error,
        }),
    }
}

/// A global chord's effect. Pure data so [`match_chord`] stays unit-testable
/// without a `Tx`; [`run_chord`] performs it inside a real handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChordAction {
    Palette(PaletteMode),
    Cheatsheet,
    OpenFile,
    Save,
    GoTo(CenterPane),
    ToggleConsole,
    LaunchImpl,
}

/// Match a key event against the true global bindings (old `SHORTCUTS`,
/// minus chords with no new-IDE counterpart). Shift-⌘P must win over ⌘P:
/// `is_command` ignores extra modifiers.
pub fn match_chord(key: &KeyEvent) -> Option<ChordAction> {
    if key.is_command('p') && key.mods.shift {
        return Some(ChordAction::Palette(PaletteMode::Commands));
    }
    if key.is_command('p') {
        return Some(ChordAction::Palette(PaletteMode::Files));
    }
    if key.is_command('k') {
        return Some(ChordAction::Cheatsheet);
    }
    if key.is_command('o') {
        return Some(ChordAction::OpenFile);
    }
    if key.is_command('s') {
        return Some(ChordAction::Save);
    }
    if key.is_command('1') {
        return Some(ChordAction::GoTo(CenterPane::Editor));
    }
    if key.is_command('2') {
        return Some(ChordAction::GoTo(CenterPane::Device));
    }
    if key.is_command('3') {
        return Some(ChordAction::GoTo(CenterPane::Timing));
    }
    if key.is_command('`') {
        return Some(ChordAction::ToggleConsole);
    }
    if key.key == Key::Enter && key.mods.cmd {
        return Some(ChordAction::LaunchImpl);
    }
    None
}

/// Perform a matched chord. The cheatsheet toggles; anything else first
/// closes both overlays so a picked surface never hides underneath.
pub fn run_chord(s: &mut AppState, action: ChordAction, tx: &mut Tx) {
    match action {
        ChordAction::Cheatsheet => {
            if s.palette.cheatsheet {
                close_cheatsheet(s);
            } else {
                open_cheatsheet(s);
            }
            return;
        }
        ChordAction::Palette(mode) => {
            open_palette(s, mode);
            return;
        }
        _ => {
            close_palette(s);
            close_cheatsheet(s);
        }
    }
    match action {
        ChordAction::OpenFile => crate::pickers::open_project(tx),
        ChordAction::Save => save_open_file(s),
        ChordAction::GoTo(pane) => s.navigate(pane),
        ChordAction::ToggleConsole => {
            s.console.collapsed = !s.console.collapsed;
        }
        ChordAction::LaunchImpl => crate::runs::launch(s, crate::runs::RunKind::Impl),
        ChordAction::Cheatsheet | ChordAction::Palette(_) => unreachable!(),
    }
}

/// `AppSpec::on_key`: keys the focused control declined (or every key with
/// no focus). While the palette owns its modal keys it declines everything;
/// while the cheatsheet is up only ⌘K toggles it off.
pub fn app_key(s: &mut AppState, key: &KeyEvent, tx: &mut Tx) -> bool {
    if s.palette.open {
        return false;
    }
    let Some(action) = match_chord(key) else {
        return false;
    };
    if s.palette.cheatsheet && action != ChordAction::Cheatsheet {
        return false;
    }
    run_chord(s, action, tx);
    true
}

/// The true global keybindings (old `SHORTCUTS`, cut to what this build
/// binds: no sidebar, density, or wrap chords exist to list).
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("⌘O", "Open file…"),
    ("⌘S", "Save open file (Editor canvas)"),
    ("⌘↩", "Implement: synthesis → opt → place → route"),
    ("⌘1", "Go to Editor"),
    ("⌘2", "Go to Device"),
    ("⌘3", "Go to Timing"),
    ("⌘`", "Toggle Tcl console"),
    ("⌘P", "Command palette: go to file"),
    ("⇧⌘P", "Command palette: commands"),
    ("⌘K", "Toggle this cheatsheet"),
];

/// Keys that only work while the palette is open (old `PALETTE_KEYS`).
pub const PALETTE_KEYS: &[(&str, &str)] = &[
    ("↑ / ↓", "Move selection (wraps)"),
    ("Enter", "Run the selected item"),
    ("Esc", "Close the palette"),
    (">", "Type as first character: switch to command mode"),
];

fn ui_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text)
}

fn dim_label(content: &str, t: &VivadoTheme) -> El {
    text(content)
        .font(VivadoTheme::FONT_MONO.mono())
        .color(t.text_faint)
}

fn divider(t: &VivadoTheme) -> El {
    el().w_full().h(1.0).fill(t.border_light).a11y_hidden()
}

/// Push the palette scrim + top-dropped panel (old `CENTER_TOP + 82`).
pub fn push_palette(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme) {
    if !s.palette.open {
        return;
    }
    let (mode, _) = effective_query(s.palette.mode, &s.palette.query.text);
    let rows = current_rows(s);
    let sel = s.palette.selected.min(rows.len().saturating_sub(1));
    let (badge, keys) = match mode {
        PaletteMode::Files => ("FILES", "⌘P"),
        PaletteMode::Commands => ("COMMANDS", "⇧⌘P"),
    };
    let hint = match mode {
        PaletteMode::Files => "Type to filter files — prefix with > for commands",
        PaletteMode::Commands => "Type to filter commands",
    };
    let mut list = col()
        .key("palette-rows")
        .role("listbox")
        .w_full()
        .max_h(LIST_MAX_H)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    if rows.is_empty() {
        list = list.child(
            text("No matches")
                .font(VivadoTheme::FONT_UI)
                .color(t.text_faint)
                .px(10.0),
        );
    }
    for (i, r) in rows.iter().enumerate() {
        list = list.child(palette_row(i, r, i == sel, t));
    }
    let panel = col()
        .key("palette-panel")
        .w(PALETTE_WIDTH)
        .fill(t.panel_bg)
        .border(1.0, t.accent)
        .radius(8.0)
        .shadow(8.0, 24.0, Color::BLACK.alpha(0.55))
        .px(14.0)
        .py(12.0)
        .gap(6.0)
        .modal(false, |s: &mut AppState| close_palette(s))
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(text(badge).font(VivadoTheme::FONT_UI).color(t.accent))
                .child(dim_label(keys, t)),
        )
        .child(
            text_input(&s.palette.query, VivadoTheme::FONT_MONO.mono())
                .single_line()
                .key("palette-query")
                .label("Palette filter input")
                .w_full()
                .px(8.0)
                .py(4.0)
                .fill(t.chrome_bg)
                .border(1.0, t.border_light)
                .radius(4.0)
                .color(t.text)
                .placeholder(hint, t.text_faint)
                .caret(t.text, t.selection)
                .on_edit(|s: &mut AppState| &mut s.palette.query)
                .on_submit(|s: &mut AppState, tx: &mut Tx| activate_selected(s, tx))
                .on_input_change(|s: &mut AppState, _tx: &mut Tx| clamp_selection(s))
                .on_input_key(
                    |s: &mut AppState, key: &KeyEvent, _tx: &mut Tx| match key.key {
                        Key::Up => {
                            move_selection(s, -1);
                            true
                        }
                        Key::Down => {
                            move_selection(s, 1);
                            true
                        }
                        Key::Escape => {
                            close_palette(s);
                            true
                        }
                        _ => false,
                    },
                ),
        )
        .child(list)
        .child(divider(t))
        .child(dim_label(
            "↑↓ navigate · Enter run · Esc close · > commands",
            t,
        ));
    layer.push(
        el().key("palette-scrim")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(Color::BLACK.alpha(SCRIM_A))
            .a11y_hidden(),
    );
    layer.push(
        el().key("palette-layer")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .child(
                col()
                    .w_full()
                    .h_full()
                    .items_center()
                    .pt(TOP_GAP)
                    .child(panel),
            ),
    );
}

/// One palette row: label left, dimmed hint right. Hover-selects like the
/// old row; a press activates its own index, not the selection.
fn palette_row(i: usize, r: &PaletteRow, selected: bool, t: &VivadoTheme) -> El {
    let mut out = row()
        .key(format!("palette-row-{i}"))
        .role("option")
        .label(r.label.clone())
        .focusable(true)
        .w_full()
        .h(ROW_H)
        .items_center()
        .justify_between()
        .px(10.0)
        .on_press_tx(move |s: &mut AppState, tx: &mut Tx| activate_row(s, i, tx))
        .on_hover(move |s: &mut AppState| {
            s.palette.selected = i;
        })
        .child(ui_label(&r.label, t))
        .child(dim_label(&r.hint, t));
    if selected {
        out = out.fill(t.selection);
    }
    out
}

/// Push the cheatsheet scrim + centered card.
pub fn push_cheatsheet(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme) {
    if !s.palette.cheatsheet {
        return;
    }
    let q = s.palette.cheat_query.text.clone();
    let kit = t.kit();
    let mut body = col().w_full().gap(8.0);
    if let Some(sec) = cheat_section("Global", SHORTCUTS, &q, t) {
        body = body.child(sec);
    }
    if let Some(sec) = cheat_section("In the command palette", PALETTE_KEYS, &q, t) {
        body = body.child(sec);
    }
    let panel = col()
        .key("cheat-panel")
        .w(CHEATSHEET_WIDTH)
        .fill(t.panel_bg)
        .border(1.0, t.accent)
        .radius(8.0)
        .shadow(8.0, 24.0, Color::BLACK.alpha(0.55))
        .px(16.0)
        .py(14.0)
        .gap(8.0)
        .modal(false, |s: &mut AppState| close_cheatsheet(s))
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(ui_label("Keyboard shortcuts", t))
                .child(
                    Button::new("Close")
                        .variant(ButtonVariant::Neutral)
                        .on_press::<AppState>(|s| close_cheatsheet(s))
                        .build(&kit)
                        .key("cheat-close"),
                ),
        )
        .child(
            text_input(&s.palette.cheat_query, VivadoTheme::FONT_MONO.mono())
                .single_line()
                .key("cheat-filter")
                .label("Filter shortcuts")
                .w_full()
                .px(8.0)
                .py(4.0)
                .fill(t.chrome_bg)
                .border(1.0, t.border_light)
                .radius(4.0)
                .color(t.text)
                .placeholder("Filter shortcuts…", t.text_faint)
                .caret(t.text, t.selection)
                .on_edit(|s: &mut AppState| &mut s.palette.cheat_query)
                .on_input_key(
                    |s: &mut AppState, key: &KeyEvent, _tx: &mut Tx| match key.key {
                        Key::Escape => {
                            close_cheatsheet(s);
                            true
                        }
                        _ => false,
                    },
                ),
        )
        .child(
            col()
                .key("cheat-rows")
                .w_full()
                .max_h(CHEAT_MAX_H)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(body),
        )
        .child(divider(t))
        .child(dim_label("Esc closes · ⌘K toggles this panel", t));
    layer.push(
        el().key("cheat-scrim")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(Color::BLACK.alpha(SCRIM_A))
            .a11y_hidden(),
    );
    layer.push(
        el().key("cheat-layer")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .child(
                col()
                    .w_full()
                    .h_full()
                    .items_center()
                    .justify_center()
                    .child(panel),
            ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    /// A runtime whose state is configured BEFORE the first frame (mutating
    /// `rt.state` after the fact leaves a stale tree).
    fn rt_with(f: impl Fn(&mut AppState)) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        let mut s = init();
        f(&mut s);
        let mut rt = Runtime::new(s, crate::view, spec.size, env);
        rt.frame(0.0);
        rt
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        // Move first, like a live pointer: teleporting between taps leaves
        // stale hover behind a changed tree and the press never lands. The
        // move needs its own frame to settle before the press.
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn chord(c: char, shift: bool) -> KeyEvent {
        use rune::Key;
        KeyEvent {
            key: Key::Command(c),
            mods: rune::Mods {
                cmd: true,
                shift,
                ..Default::default()
            },
        }
    }

    #[test]
    fn fuzzy_score_rewards_boundaries_runs_and_rejects_gaps() {
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        assert_eq!(fuzzy_score("zxq", "abc"), None);
        assert_eq!(fuzzy_score("abcdef", "abc"), None);
        let boundary = fuzzy_score("fb", "foo-bar").unwrap();
        let mid = fuzzy_score("fb", "xfoob").unwrap();
        assert!(
            boundary > mid,
            "word-boundary hits must outscore mid-word: {boundary} vs {mid}"
        );
        // Same start offset, so the boundary bonus cannot confound the run.
        let together = fuzzy_score("bc", "abc").unwrap();
        let apart = fuzzy_score("bc", "axbc").unwrap();
        assert!(
            together > apart,
            "consecutive runs must outscore gapped: {together} vs {apart}"
        );
    }

    #[test]
    fn effective_query_prefix_forces_command_mode() {
        assert_eq!(
            effective_query(PaletteMode::Files, ">syn"),
            (PaletteMode::Commands, "syn")
        );
        assert_eq!(
            effective_query(PaletteMode::Files, ">  syn"),
            (PaletteMode::Commands, "syn")
        );
        assert_eq!(
            effective_query(PaletteMode::Files, "syn"),
            (PaletteMode::Files, "syn")
        );
        assert_eq!(
            effective_query(PaletteMode::Commands, "x"),
            (PaletteMode::Commands, "x")
        );
    }

    #[test]
    fn open_palette_resets_query_selection_and_closes_cheatsheet() {
        let mut s = init();
        s.palette.cheatsheet = true;
        s.palette.query = TextEdit::new("stale");
        s.palette.selected = 4;
        open_palette(&mut s, PaletteMode::Commands);
        assert!(s.palette.open);
        assert_eq!(s.palette.mode, PaletteMode::Commands);
        assert_eq!(s.palette.query.text, "");
        assert_eq!(s.palette.selected, 0);
        assert!(!s.palette.cheatsheet);
    }

    #[test]
    fn move_selection_wraps_at_both_ends() {
        let mut s = init();
        open_palette(&mut s, PaletteMode::Commands);
        let n = current_rows(&s).len();
        assert!(n > 2);
        move_selection(&mut s, -1);
        assert_eq!(s.palette.selected, n - 1);
        move_selection(&mut s, 1);
        assert_eq!(s.palette.selected, 0);
    }

    #[test]
    fn match_chord_maps_the_true_global_bindings() {
        use rune::{Key, Mods};
        assert_eq!(
            match_chord(&chord('p', false)),
            Some(ChordAction::Palette(PaletteMode::Files))
        );
        assert_eq!(
            match_chord(&chord('p', true)),
            Some(ChordAction::Palette(PaletteMode::Commands))
        );
        assert_eq!(
            match_chord(&chord('k', false)),
            Some(ChordAction::Cheatsheet)
        );
        assert_eq!(match_chord(&chord('o', false)), Some(ChordAction::OpenFile));
        assert_eq!(match_chord(&chord('s', false)), Some(ChordAction::Save));
        assert_eq!(
            match_chord(&chord('1', false)),
            Some(ChordAction::GoTo(CenterPane::Editor))
        );
        assert_eq!(
            match_chord(&chord('2', false)),
            Some(ChordAction::GoTo(CenterPane::Device))
        );
        assert_eq!(
            match_chord(&chord('3', false)),
            Some(ChordAction::GoTo(CenterPane::Timing))
        );
        assert_eq!(
            match_chord(&chord('`', false)),
            Some(ChordAction::ToggleConsole)
        );
        assert_eq!(
            match_chord(&KeyEvent {
                key: Key::Enter,
                mods: Mods {
                    cmd: true,
                    ..Default::default()
                },
            }),
            Some(ChordAction::LaunchImpl)
        );
        assert_eq!(
            match_chord(&KeyEvent {
                key: Key::Enter,
                mods: Mods::default(),
            }),
            None,
            "plain Enter must not implement"
        );
        assert_eq!(
            match_chord(&KeyEvent {
                key: Key::Text("p".into()),
                mods: Mods::default(),
            }),
            None,
            "typed text must not open the palette"
        );
    }

    #[test]
    fn files_mode_lists_tabs_and_pick_activates() {
        let mut rt = rt_with(|s| open_palette(s, PaletteMode::Files));
        assert!(rt.find("palette-panel").is_some());
        assert!(rt.find("palette-query").is_some());
        assert!(rt.find("palette-row-0").is_some());
        assert!(rt.find("palette-row-1").is_some());
        assert!(
            rt.find("cheat-panel").is_none(),
            "cheatsheet must stay shut"
        );
        tap(&mut rt, "palette-row-1");
        assert_eq!(rt.state.editor.active, 1);
        assert!(!rt.state.palette.open, "picking must close the palette");
    }

    #[test]
    fn files_mode_filters_by_name() {
        let mut s = init();
        open_palette(&mut s, PaletteMode::Files);
        s.palette.query = TextEdit::new("xdc");
        let rows = current_rows(&s);
        assert_eq!(rows.len(), 1, "only the xdc tab matches: {rows:?}");
        assert_eq!(rows[0].label, "project1.xdc");
    }

    #[test]
    fn command_pick_navigates_and_closes() {
        let mut s = init();
        open_palette(&mut s, PaletteMode::Commands);
        s.palette.query = TextEdit::new("device");
        let rows = current_rows(&s);
        assert_eq!(
            rows[0].pick,
            PalettePick::Cmd(PaletteCommand::GoTo(CenterPane::Device)),
            "top hit must be Go to Device: {rows:?}"
        );
        let mut rt = rt_with(|s| {
            open_palette(s, PaletteMode::Commands);
            s.palette.query = TextEdit::new("device");
        });
        tap(&mut rt, "palette-row-0");
        assert_eq!(rt.state.center, CenterPane::Device);
        assert!(!rt.state.palette.open);
    }

    #[test]
    fn command_pick_runs_synthesis_for_real() {
        let mut rt = rt_with(|s| {
            open_palette(s, PaletteMode::Commands);
            s.palette.query = TextEdit::new("synthesis");
        });
        tap(&mut rt, "palette-row-0");
        assert_eq!(rt.state.flow.synth, crate::state::RunStatus::Running);
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "launch_runs synth_1"),
            "the run must echo its verb"
        );
        assert!(!rt.state.palette.open);
        crate::runs::cancel_run(&mut rt.state);
        let start = std::time::Instant::now();
        while rt.state.runs.is_active() {
            crate::runs::poll_runs(&mut rt.state);
            assert!(
                start.elapsed() < std::time::Duration::from_secs(60),
                "cancelled run must wind down"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn command_pick_raises_a_real_open_dialog() {
        let mut rt = rt_with(|s| {
            open_palette(s, PaletteMode::Commands);
            s.palette.query = TextEdit::new("open file");
        });
        tap(&mut rt, "palette-row-0");
        assert!(
            rt.file_dialog().is_some(),
            "Open file must raise the native dialog"
        );
        assert!(!rt.state.palette.open);
    }

    #[test]
    fn command_pick_opens_cheatsheet_and_closes_palette() {
        let mut rt = rt_with(|s| {
            open_palette(s, PaletteMode::Commands);
            s.palette.query = TextEdit::new("shortcuts");
        });
        tap(&mut rt, "palette-row-0");
        assert!(rt.state.palette.cheatsheet);
        assert!(!rt.state.palette.open);
        assert!(rt.find("cheat-panel").is_some());
        assert!(rt.find("cheat-filter").is_some());
        tap(&mut rt, "cheat-close");
        assert!(
            !rt.state.palette.cheatsheet,
            "Close must fire on the first press, not merely steal focus"
        );
    }

    #[test]
    fn cheatsheet_filter_drops_empty_sections() {
        let theme = crate::theme::VivadoTheme::light();
        assert!(
            cheat_section("Global", SHORTCUTS, "cheatsheet", &theme).is_some(),
            "the ⌘K row matches the filter"
        );
        assert!(
            cheat_section("In the command palette", PALETTE_KEYS, "cheatsheet", &theme).is_none(),
            "sections with no matches must vanish"
        );
        assert!(
            cheat_section("Global", SHORTCUTS, "", &theme).is_some(),
            "empty filter keeps every section"
        );
    }

    #[test]
    fn tools_menu_opens_both_palette_modes() {
        // One runtime per mode: the open palette's modal scrim covers the
        // menubar, so a second menu tap in the same runtime cannot land.
        let mut rt = rt_with(|_| {});
        tap(&mut rt, "menu-tools");
        tap(&mut rt, "menu-tools-item5");
        assert!(rt.state.palette.open);
        assert_eq!(rt.state.palette.mode, PaletteMode::Files);
        let mut rt = rt_with(|_| {});
        tap(&mut rt, "menu-tools");
        tap(&mut rt, "menu-tools-item6");
        assert!(rt.state.palette.open);
        assert_eq!(rt.state.palette.mode, PaletteMode::Commands);
    }
}

/// One cheatsheet section, fuzzy-filtered by the search box (`None` when
/// nothing matches, so empty sections vanish like the old card).
fn cheat_section(title: &str, items: &[(&str, &str)], query: &str, t: &VivadoTheme) -> Option<El> {
    let shown: Vec<(&str, &str)> = items
        .iter()
        .copied()
        .filter(|(k, d)| query.is_empty() || fuzzy_score(query, &format!("{k} {d}")).is_some())
        .collect();
    if shown.is_empty() {
        return None;
    }
    let mut out = col().w_full().gap(2.0);
    out = out.child(text(title).font(VivadoTheme::FONT_UI).color(t.text_dim));
    for (keys, desc) in shown {
        out = out.child(
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .child(
                    text(keys)
                        .font(VivadoTheme::FONT_MONO.mono())
                        .color(t.accent),
                )
                .child(ui_label(desc, t)),
        );
    }
    Some(out)
}
