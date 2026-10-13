//! Design tokens for `helion-ide` (P0-4).
//!
//! Single source of truth for color, typography, and spacing. All paint code
//! consumes [`Theme`] fields - never hardcode `Color32::from_rgb` literals in
//! the shell. Dark is the default; [`Theme::light`] overrides the same names.
//!
//! Token values follow `~/workspace/helion-research/ide-ux-plan.md` §5:
//! flat surfaces, 1px `--line` separators, one blue accent, semantic colors
//! only for real states, body text ≥ 4.5:1 contrast.

use eframe::egui::{self, Color32, FontFamily, FontId};

/// Row/padding density. Compact keeps ≥ 28px hit rows (see `chrome.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    Compact,
    Comfortable,
}

/// Semantic color tokens + density. Copy: cheap, passed by value everywhere.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub bg: Color32,
    pub panel: Color32,
    pub surface: Color32,
    pub surface_hover: Color32,
    pub line: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub faint: Color32,
    pub accent: Color32,
    /// Accent at ~25% over surface - selection fill.
    pub sel: Color32,
    /// Accent dimmed for active-but-muted glyphs.
    pub accent_dim: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub danger: Color32,
    pub info: Color32,
    /// Secondary categorical color (charts, advisory severities).
    pub violet: Color32,
    /// Device / waveform canvas background (darker than `--bg`).
    pub die_bg: Color32,
    /// Categorical blue: IOB sites, categorical data blue.
    pub cat_blue: Color32,
    /// Waveform trace / cursor B (cyan). Trace A uses `--danger`.
    pub trace_b: Color32,
    /// Selected-net highlight, pblock-adjacent gold.
    pub gold: Color32,
    /// Pblock outlines, categorical amber.
    pub amber: Color32,
    pub density: Density,
}

impl Theme {
    /// Dark default. Values from the UX plan token table.
    pub fn dark() -> Self {
        Self {
            bg: Color32::from_rgb(0x16, 0x1c, 0x22),
            panel: Color32::from_rgb(0x1b, 0x22, 0x2b),
            surface: Color32::from_rgb(0x23, 0x2c, 0x37),
            surface_hover: Color32::from_rgb(0x2b, 0x35, 0x42),
            line: Color32::from_rgb(0x3a, 0x42, 0x4a),
            text: Color32::from_rgb(0xdc, 0xe0, 0xe4),
            dim: Color32::from_rgb(0xa0, 0xa8, 0xb0),
            faint: Color32::from_rgb(0x6b, 0x74, 0x80),
            accent: Color32::from_rgb(0x4d, 0x9f, 0xff),
            sel: Color32::from_rgb(0x2a, 0x44, 0x66),
            accent_dim: Color32::from_rgb(0x7e, 0xa8, 0xd8),
            ok: Color32::from_rgb(0x3d, 0xb8, 0x7a),
            warn: Color32::from_rgb(0xe0, 0xa0, 0x40),
            danger: Color32::from_rgb(0xe0, 0x6c, 0x75),
            info: Color32::from_rgb(0x7e, 0xc8, 0xe3),
            violet: Color32::from_rgb(0x90, 0x60, 0xc0),
            die_bg: Color32::from_rgb(0x12, 0x16, 0x1a),
            cat_blue: Color32::from_rgb(0x5b, 0x9b, 0xd5),
            trace_b: Color32::from_rgb(0x56, 0xb6, 0xc2),
            gold: Color32::from_rgb(0xe5, 0xc0, 0x7b),
            amber: Color32::from_rgb(0xe5, 0x9a, 0x3c),
            density: Density::Comfortable,
        }
    }

    /// Light theme: same token names, overridden values. WCAG floors hold
    /// (body text ≥ 4.5:1 on `--bg`).
    pub fn light() -> Self {
        Self {
            bg: Color32::from_rgb(0xf4, 0xf5, 0xf7),
            panel: Color32::from_rgb(0xea, 0xec, 0xef),
            surface: Color32::from_rgb(0xff, 0xff, 0xff),
            surface_hover: Color32::from_rgb(0xe8, 0xeb, 0xf0),
            line: Color32::from_rgb(0xd4, 0xd8, 0xde),
            text: Color32::from_rgb(0x1e, 0x24, 0x2c),
            dim: Color32::from_rgb(0x5a, 0x64, 0x70),
            faint: Color32::from_rgb(0x8a, 0x94, 0x9e),
            accent: Color32::from_rgb(0x1a, 0x6f, 0xe0),
            sel: Color32::from_rgb(0xd0, 0xe2, 0xfa),
            accent_dim: Color32::from_rgb(0x4a, 0x8a, 0xc8),
            ok: Color32::from_rgb(0x1e, 0x8a, 0x4e),
            warn: Color32::from_rgb(0xa8, 0x6a, 0x10),
            danger: Color32::from_rgb(0xb0, 0x30, 0x38),
            info: Color32::from_rgb(0x1a, 0x6f, 0xa8),
            violet: Color32::from_rgb(0x6a, 0x3f, 0xb0),
            die_bg: Color32::from_rgb(0xe2, 0xe6, 0xec),
            cat_blue: Color32::from_rgb(0x2a, 0x6c, 0xb4),
            trace_b: Color32::from_rgb(0x1f, 0x8a, 0xa8),
            gold: Color32::from_rgb(0x96, 0x69, 0x0f),
            amber: Color32::from_rgb(0xa8, 0x6a, 0x10),
            density: Density::Comfortable,
        }
    }

    /// Push this theme into egui: visuals + text styles + spacing.
    /// Call once at startup and whenever the theme/density changes.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut visuals = if self.is_dark() {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        visuals.override_text_color = Some(self.text);
        visuals.panel_fill = self.panel;
        visuals.window_fill = self.surface;
        visuals.extreme_bg_color = self.bg;
        visuals.faint_bg_color = self.surface;
        visuals.code_bg_color = self.bg;
        visuals.warn_fg_color = self.warn;
        visuals.error_fg_color = self.danger;
        // Selection: accent-tinted fill, full-strength text.
        visuals.selection.bg_fill = self.sel;
        visuals.selection.stroke = egui::Stroke::new(1.0, self.accent);
        // Widgets: flat surfaces, 1px --line separators (no chunky borders).
        for state in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
        ] {
            state.bg_fill = self.surface;
            state.fg_stroke = egui::Stroke::new(1.0, self.dim);
        }
        visuals.widgets.hovered.bg_fill = self.surface_hover;
        visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, self.accent_dim);
        visuals.widgets.active.bg_fill = self.sel;
        visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, self.accent);
        ctx.set_visuals(visuals);

        ctx.style_mut(|s| {
            use egui::TextStyle as TS;
            s.text_styles.insert(TS::Body, font_ui(13.0));
            s.text_styles.insert(TS::Button, font_ui(13.0));
            s.text_styles.insert(TS::Small, font_ui(11.0));
            s.text_styles.insert(TS::Heading, font_ui_medium(16.0));
            s.text_styles
                .insert(TS::Name("caption".into()), font_ui(11.0));
            s.text_styles
                .insert(TS::Name("wordmark".into()), font_ui_medium(20.0));
            s.text_styles.insert(TS::Monospace, font_code(12.0));
            let (row, pad) = match self.density {
                Density::Compact => (24.0, 4.0),
                Density::Comfortable => (28.0, 8.0),
            };
            s.spacing.interact_size.y = row;
            s.spacing.button_padding = egui::vec2(pad + 4.0, pad * 0.75);
            s.spacing.item_spacing = egui::vec2(8.0, pad * 0.5);
            s.spacing.indent = 16.0;
        });
    }

    fn is_dark(&self) -> bool {
        // Luminance heuristic on --bg.
        let (r, g, b) = (self.bg.r() as u32, self.bg.g() as u32, self.bg.b() as u32);
        (r * 299 + g * 587 + b * 114) / 1000 < 128
    }
}

/// Install bundled fonts: Inter (UI) + JetBrains Mono (code).
/// Called once from the eframe setup closure, before [`Theme::apply`].
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter-regular".into(),
        egui::FontData::from_owned(include_bytes!("../../../fonts/Inter-Regular.otf").to_vec())
            .into(),
    );
    fonts.font_data.insert(
        "inter-medium".into(),
        egui::FontData::from_owned(include_bytes!("../../../fonts/Inter-Medium.otf").to_vec())
            .into(),
    );
    fonts.font_data.insert(
        "inter-semibold".into(),
        egui::FontData::from_owned(include_bytes!("../../../fonts/Inter-SemiBold.otf").to_vec())
            .into(),
    );
    fonts.font_data.insert(
        "jbmono-regular".into(),
        egui::FontData::from_owned(
            include_bytes!("../../../fonts/JetBrainsMono-Regular.ttf").to_vec(),
        )
        .into(),
    );
    fonts.font_data.insert(
        "jbmono-bold".into(),
        egui::FontData::from_owned(
            include_bytes!("../../../fonts/JetBrainsMono-Bold.ttf").to_vec(),
        )
        .into(),
    );
    // CJK fallback for source comments (e.g. Chinese in ysyx_ibex.sv).
    // DroidSansFallback subset to CJK blocks (~2.8MB); wired as a per-glyph
    // fallback after the primary faces so missing glyphs render instead of
    // tofu. egui tries family fonts in order per glyph.
    fonts.font_data.insert(
        "cjk-fallback".into(),
        egui::FontData::from_owned(
            include_bytes!("../../../fonts/DroidSansFallback.ttf").to_vec(),
        )
        .into(),
    );
    if let Some(fams) = fonts.families.get_mut(&FontFamily::Proportional) {
        *fams = vec!["inter-regular".into(), "cjk-fallback".into()];
    }
    if let Some(fams) = fonts.families.get_mut(&FontFamily::Monospace) {
        *fams = vec!["jbmono-regular".into(), "cjk-fallback".into()];
    }
    fonts.families.insert(
        FontFamily::Name("ui-medium".into()),
        vec!["inter-medium".into()],
    );
    fonts.families.insert(
        FontFamily::Name("ui-semibold".into()),
        vec!["inter-semibold".into()],
    );
    fonts.families.insert(
        FontFamily::Name("mono-bold".into()),
        vec!["jbmono-bold".into()],
    );
    ctx.set_fonts(fonts);
}

/// UI typeface (Inter) at `size` px.
pub fn font_ui(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// UI typeface, medium weight - headings, rail labels, wordmark.
pub fn font_ui_medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("ui-medium".into()))
}

/// UI typeface, semibold - primary buttons, section titles.
#[allow(dead_code)]
pub fn font_ui_semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("ui-semibold".into()))
}

/// Code typeface (JetBrains Mono) at `size` px.
pub fn font_code(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}
