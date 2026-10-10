//! Command registry, fuzzy command palette, menu bar, shortcut cheatsheet
//! (P0-2, P1-6, P2 cheatsheet). Implemented by the feature agent.

use eframe::egui;

use super::HelionIde;

/// Palette mode: `>` prefix or Cmd+Shift+P forces commands; Cmd+P is go-to-file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PaletteMode {
    #[default]
    Files,
    Commands,
}

#[derive(Default)]
pub struct PaletteState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    pub mode: PaletteMode,
}

/// Fuzzy command palette overlay (VS Code Quick Pick style).
/// Keyboard: type to filter, Up/Down to move, Enter to run, Esc to close.
pub fn paint_palette(_ctx: &egui::Context, _app: &mut HelionIde) {
    // TODO(feature): full implementation.
}

/// Menu bar (File / Edit / View / Flow / Tools / Help), painted inside the
/// 44px toolbar row via `ui.menu_button`. Mirrors the command registry.
pub fn paint_menu_bar(_ui: &mut egui::Ui, _app: &mut HelionIde) {
    // TODO(feature): full implementation.
}

/// Shortcut cheatsheet overlay (`?` or via palette): searchable keybinding list.
pub fn paint_shortcut_cheatsheet(_ctx: &egui::Context, _app: &mut HelionIde) {
    // TODO(feature): full implementation.
}
