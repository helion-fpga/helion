//! Vivado theme tokens for the Helion Rune shell, in light and dark.
//!
//! Every color in `helion-rune` views MUST come from [`VivadoTheme`]; no hex
//! literals or `Color::` constants anywhere else (critique do-not-repeat:
//! hardcoded fills break the other mode). Light values emulate the
//! `/tmp/vivado-ref/*.png` light aesthetic: gray chrome, white panels, blue
//! links, sentence-case CAD density. Dark values follow the same token
//! meanings on a `#2B2D30` CAD-dark ramp.

use rune::tok::TextStyle;
use rune::{Color, color};

/// Theme choice persisted in [`crate::state::AppState::theme`].
///
/// `System` (the default) follows `cx.system_dark()` from
/// `rune/crates/rune/src/runtime/cx.rs`; the View menu's Dark Theme row
/// pins an explicit [`ThemeMode::Light`] or [`ThemeMode::Dark`] override,
/// and Follow System Theme returns to the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    /// Follow the OS appearance (`cx.system_dark()`).
    #[default]
    System,
    /// Pinned light tokens regardless of the OS appearance.
    Light,
    /// Pinned dark tokens regardless of the OS appearance.
    Dark,
}

impl ThemeMode {
    /// Whether this choice renders dark under the given system appearance.
    pub fn is_dark(self, system_dark: bool) -> bool {
        match self {
            ThemeMode::System => system_dark,
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
        }
    }

    /// The token set for this choice under the given system appearance.
    pub fn resolve(self, system_dark: bool) -> VivadoTheme {
        if self.is_dark(system_dark) {
            VivadoTheme::dark()
        } else {
            VivadoTheme::light()
        }
    }
}

/// Full token set for the Vivado light look. One instance per [`crate::view`]
/// call; cheap (plain `Copy` colors).
#[derive(Clone, Copy, Debug)]
pub struct VivadoTheme {
    /// Window chrome / status bar gray.
    pub chrome_bg: Color,
    /// Flow-navigator rail and side panes.
    pub rail_bg: Color,
    /// Central panels (Sources, Summary, editor, console).
    pub panel_bg: Color,
    /// Active project tab strip.
    pub titlebar_bg: Color,
    /// Pane-title and card-header band.
    pub header_bg: Color,
    /// Primary UI ink.
    pub text: Color,
    /// Secondary labels, captions.
    pub text_dim: Color,
    /// Placeholder / disabled ink.
    pub text_faint: Color,
    /// Pane outlines, splitters.
    pub border: Color,
    /// Inner rules, table gridlines.
    pub border_light: Color,
    /// Hyperlinks (part names, strategies, Run Implementation).
    pub link: Color,
    /// Accent for toggles / focus (same family as [`Self::link`]).
    pub accent: Color,
    /// Pass / complete green.
    pub pass: Color,
    /// Fail / error red.
    pub fail: Color,
    /// Warning amber.
    pub warn: Color,
    /// Editor current-line wash.
    pub current_line: Color,
    /// Tcl console background.
    pub console_bg: Color,
    /// Selected-row wash.
    pub selection: Color,
    /// Switch knob.
    pub knob: Color,
    /// Switch off-track.
    pub control_off: Color,
    /// Editor keyword ink (Verilog/XDC words, directives).
    pub code_keyword: Color,
    /// Editor comment ink (gray; painted italic).
    pub code_comment: Color,
    /// Editor numeric-literal ink.
    pub code_number: Color,
    /// Editor string-literal ink.
    pub code_string: Color,
    /// Editor type / system-task ink (`logic`, `std_logic`, `$display`).
    pub code_type: Color,
    /// Start page "Quick Start" banner fill (Helion green).
    pub banner_quick: Color,
    /// Start page "Tasks" / "Learning Center" banner fill (Helion teal).
    pub banner_teal: Color,
    /// Banner titles and links (white on the banner fills).
    pub banner_ink: Color,
    /// Device die canvas background (dark navy, `lab3-Fig13.png`).
    pub die_bg: Color,
    /// Device site-tile fill.
    pub die_tile: Color,
    /// Device tile-grid and die-frame lines.
    pub die_grid: Color,
    /// Placed-cell fill (yellow).
    pub die_cell: Color,
    /// Routed-net wire ink.
    pub die_net: Color,
    /// Clock-region boundary lines.
    pub die_region: Color,
    /// Waveform trace ink (Vivado sim green).
    pub wave_trace: Color,
    /// Waveform lane gridlines.
    pub wave_grid: Color,
    /// Waveform main cursor.
    pub wave_cursor: Color,
    /// Waveform A/B cursors and marker flags.
    pub wave_marker: Color,
}

fn c(hex: &str) -> Color {
    color::hex(hex).unwrap_or(Color::BLACK)
}

impl VivadoTheme {
    /// The dark theme: same token meanings on a `#2B2D30` CAD-dark ramp.
    /// Banner fills and die/wave canvas inks stay shared with light mode;
    /// links, accents, and status inks are brightened for dark surfaces.
    pub fn dark() -> Self {
        // Cursor-signature dark world: near-black chrome, one step up for the
        // editor surface, hairline borders, crisp ink, single blue accent.
        Self {
            chrome_bg: c("#141414"),
            rail_bg: c("#181818"),
            panel_bg: c("#1F1F1F"),
            titlebar_bg: c("#141414"),
            header_bg: c("#252525"),
            text: c("#E8E8E8"),
            text_dim: c("#B0B0B0"),
            text_faint: c("#7A7A7A"),
            border: c("#333333"),
            border_light: c("#2A2A2A"),
            link: c("#4DA3FF"),
            accent: c("#4DA3FF"),
            pass: c("#89D185"),
            fail: c("#F14C4C"),
            warn: c("#E8A33D"),
            current_line: c("#2A2D2E"),
            console_bg: c("#181818"),
            selection: c("#264F78"),
            knob: c("#9D9D9D"),
            control_off: c("#3A3A3A"),
            code_keyword: c("#569CD6"),
            code_comment: c("#6A9955"),
            code_number: c("#B5CEA8"),
            code_string: c("#CE9178"),
            code_type: c("#4EC9B0"),
            banner_quick: c("#2E7D32"),
            banner_teal: c("#14707F"),
            banner_ink: c("#FFFFFF"),
            die_bg: c("#151C33"),
            die_tile: c("#1D2642"),
            die_grid: c("#31405F"),
            die_cell: c("#F2C200"),
            die_net: c("#F5F5F5"),
            die_region: c("#5B9BD5"),
            wave_trace: c("#4EC94E"),
            wave_grid: c("#333333"),
            wave_cursor: c("#E6B800"),
            wave_marker: c("#C586C0"),
        }
    }

    /// Translucent hover wash for links on the banner fills. The banners
    /// are constant across modes, so one wash serves both themes.
    pub fn banner_hover(&self) -> Color {
        Color {
            a: 0.14,
            ..self.banner_ink
        }
    }

    /// The light theme. Together with [`Self::dark`] the only place hex
    /// literals may live in this crate.
    pub fn light() -> Self {
        Self {
            chrome_bg: c("#EDEDED"),
            rail_bg: c("#F7F7F7"),
            panel_bg: c("#FFFFFF"),
            titlebar_bg: c("#D3E0F0"),
            header_bg: c("#F0F0F0"),
            text: c("#1E1E1E"),
            text_dim: c("#5A5A5A"),
            text_faint: c("#767676"),
            border: c("#C8C8C8"),
            border_light: c("#E1E1E1"),
            link: c("#0072C6"),
            accent: c("#0072C6"),
            pass: c("#107C10"),
            fail: c("#D13438"),
            warn: c("#CA5010"),
            current_line: c("#FFF8DC"),
            console_bg: c("#FFFFFF"),
            selection: c("#D6E9F8"),
            knob: c("#FFFFFF"),
            control_off: c("#C8C8C8"),
            code_keyword: c("#7F0055"),
            code_comment: c("#6E6E6E"),
            code_number: c("#AF00DB"),
            code_string: c("#A31515"),
            code_type: c("#267F99"),
            banner_quick: c("#2E7D32"),
            banner_teal: c("#14707F"),
            banner_ink: c("#FFFFFF"),
            die_bg: c("#1A2440"),
            die_tile: c("#232F52"),
            die_grid: c("#3A4A73"),
            die_cell: c("#F2C200"),
            die_net: c("#F5F5F5"),
            die_region: c("#5B9BD5"),
            wave_trace: c("#0F7B0F"),
            wave_grid: c("#E0E0E0"),
            wave_cursor: c("#E6B800"),
            wave_marker: c("#8E24AA"),
        }
    }

    /// A `rune-kit` kit derived from the Vivado tokens
    /// (`crates/rune-kit/src/palette.rs`: `Kit::from_palette`).
    pub fn kit(&self) -> rune_kit::Kit {
        let mut kit = rune_kit::Kit::from_palette(rune_kit::PaletteSeed {
            bg: self.panel_bg,
            fg: self.text,
            accent: self.accent,
        });
        kit.radius.sm = 2.0;
        kit.radius.md = 3.0;
        kit.c.border_focus = self.accent;
        kit
    }

    /// CAD-density UI type: compact 11-12px sans per the Vivado brief.
    pub const FONT_UI: TextStyle = TextStyle::new(12.0, 400.0);
    /// Pane titles and card headers.
    pub const FONT_TITLE: TextStyle = TextStyle::new(12.0, 600.0);
    /// Flow-navigator caps section labels.
    pub const FONT_CAPS: TextStyle = TextStyle::new(11.0, 600.0);
    /// Tcl console / log / report monospace.
    pub const FONT_MONO: TextStyle = TextStyle::new(12.0, 400.0);
    /// Start page banner titles (large light display type).
    pub const FONT_DISPLAY: TextStyle = TextStyle::new(26.0, 300.0);
    /// Start page banner links.
    pub const FONT_BANNER_LINK: TextStyle = TextStyle::new(13.0, 400.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Relative luminance. `rune::color::hex` stores linear channels
    /// (`rune/crates/rune/src/color.rs`), so raw WCAG coefficients apply.
    fn luminance(c: Color) -> f32 {
        0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
    }

    #[test]
    fn dark_theme_tokens_are_opaque_and_dark() {
        let t = VivadoTheme::dark();
        for (name, col) in [
            ("chrome_bg", t.chrome_bg),
            ("rail_bg", t.rail_bg),
            ("panel_bg", t.panel_bg),
            ("titlebar_bg", t.titlebar_bg),
            ("header_bg", t.header_bg),
            ("text", t.text),
            ("text_dim", t.text_dim),
            ("text_faint", t.text_faint),
            ("border", t.border),
            ("border_light", t.border_light),
            ("link", t.link),
            ("accent", t.accent),
            ("pass", t.pass),
            ("fail", t.fail),
            ("warn", t.warn),
            ("current_line", t.current_line),
            ("console_bg", t.console_bg),
            ("selection", t.selection),
            ("knob", t.knob),
            ("control_off", t.control_off),
            ("code_keyword", t.code_keyword),
            ("code_comment", t.code_comment),
            ("code_number", t.code_number),
            ("code_string", t.code_string),
            ("code_type", t.code_type),
            ("banner_quick", t.banner_quick),
            ("banner_teal", t.banner_teal),
            ("banner_ink", t.banner_ink),
            ("die_bg", t.die_bg),
            ("die_tile", t.die_tile),
            ("die_grid", t.die_grid),
            ("die_cell", t.die_cell),
            ("die_net", t.die_net),
            ("die_region", t.die_region),
            ("wave_trace", t.wave_trace),
            ("wave_grid", t.wave_grid),
            ("wave_cursor", t.wave_cursor),
            ("wave_marker", t.wave_marker),
        ] {
            assert_eq!(col.a, 1.0, "dark {name} must be opaque");
            assert_ne!(
                col.hex(),
                "#000000ff",
                "dark {name} must parse, not fall back"
            );
        }
        // Dark surfaces stay dark while ink stays light: no light-only
        // wash may leak into the dark set.
        assert!(luminance(t.panel_bg) < 0.15, "dark panel must be dark");
        assert!(luminance(t.chrome_bg) < 0.15, "dark chrome must be dark");
        assert!(luminance(t.text) > 0.4, "dark ink must be light");
        assert!(
            luminance(t.text) - luminance(t.panel_bg) > 0.3,
            "dark text needs contrast"
        );
        assert!(
            luminance(t.text_dim) - luminance(t.panel_bg) > 0.2,
            "dark dim text needs contrast"
        );
        let light = VivadoTheme::light();
        assert!(
            luminance(light.panel_bg) > 0.7,
            "light panel must stay light"
        );
    }

    #[test]
    fn theme_mode_follows_system_by_default_with_manual_override() {
        assert_eq!(ThemeMode::default(), ThemeMode::System);
        assert!(ThemeMode::System.is_dark(true));
        assert!(!ThemeMode::System.is_dark(false));
        assert!(
            ThemeMode::Dark.is_dark(false),
            "manual dark wins over a light system"
        );
        assert!(
            !ThemeMode::Light.is_dark(true),
            "manual light wins over a dark system"
        );
        assert_eq!(
            ThemeMode::System.resolve(true).panel_bg,
            VivadoTheme::dark().panel_bg
        );
        assert_eq!(
            ThemeMode::System.resolve(false).panel_bg,
            VivadoTheme::light().panel_bg
        );
    }

    #[test]
    fn every_token_is_opaque() {
        let t = VivadoTheme::light();
        for (name, col) in [
            ("chrome_bg", t.chrome_bg),
            ("rail_bg", t.rail_bg),
            ("panel_bg", t.panel_bg),
            ("titlebar_bg", t.titlebar_bg),
            ("header_bg", t.header_bg),
            ("text", t.text),
            ("text_dim", t.text_dim),
            ("text_faint", t.text_faint),
            ("border", t.border),
            ("border_light", t.border_light),
            ("link", t.link),
            ("accent", t.accent),
            ("pass", t.pass),
            ("fail", t.fail),
            ("warn", t.warn),
            ("current_line", t.current_line),
            ("console_bg", t.console_bg),
            ("selection", t.selection),
            ("knob", t.knob),
            ("control_off", t.control_off),
            ("code_keyword", t.code_keyword),
            ("code_comment", t.code_comment),
            ("code_number", t.code_number),
            ("code_string", t.code_string),
            ("code_type", t.code_type),
            ("banner_quick", t.banner_quick),
            ("banner_teal", t.banner_teal),
            ("banner_ink", t.banner_ink),
            ("die_bg", t.die_bg),
            ("die_tile", t.die_tile),
            ("die_grid", t.die_grid),
            ("die_cell", t.die_cell),
            ("die_net", t.die_net),
            ("die_region", t.die_region),
            ("wave_trace", t.wave_trace),
            ("wave_grid", t.wave_grid),
            ("wave_cursor", t.wave_cursor),
            ("wave_marker", t.wave_marker),
        ] {
            assert_eq!(col.a, 1.0, "{name} must be opaque");
            assert_ne!(col.hex(), "#000000ff", "{name} must parse, not fall back");
        }
    }
}
