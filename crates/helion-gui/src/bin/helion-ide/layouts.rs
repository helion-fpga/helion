//! Layout presets + chrome persistence (P1-1).
//!
//! Task-curated arrangements of activity / canvas / workspace / sidebar, plus
//! `~/.helion/chrome.ini` persistence for the window chrome (preset, sidebar
//! width, console height, theme, density). File IO happens only on explicit
//! user action - never per frame.

use eframe::egui;
use helion_gui::chrome::{self, Activity, Canvas};
use helion_gui::{BottomTab, WorkspaceTab};

use super::theme::{Density, Theme};
use super::HelionIde;
use std::path::PathBuf;

/// Task-curated chrome arrangements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LayoutPreset {
    #[default]
    Design,
    TimingAnalysis,
    Floorplan,
    Simulate,
}

impl LayoutPreset {
    pub const ALL: [LayoutPreset; 4] = [
        LayoutPreset::Design,
        LayoutPreset::TimingAnalysis,
        LayoutPreset::Floorplan,
        LayoutPreset::Simulate,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LayoutPreset::Design => "Design",
            LayoutPreset::TimingAnalysis => "Timing Analysis",
            LayoutPreset::Floorplan => "Floorplan",
            LayoutPreset::Simulate => "Simulate",
        }
    }
}

/// Persisted chrome choice: which preset the user last picked.
#[derive(Default)]
pub struct LayoutState {
    pub preset: LayoutPreset,
}

/// Arrange activity / canvas / workspace / sidebar for `preset`, and record
/// the choice in `layout_state`.
pub fn apply_preset(app: &mut HelionIde, preset: LayoutPreset) {
    match preset {
        LayoutPreset::Design => {
            app.set_activity(Activity::Files);
            app.set_canvas(Canvas::Editor);
            super::open_more_workspace(app, WorkspaceTab::TextEditor);
            app.sidebar_hidden = false;
        }
        LayoutPreset::TimingAnalysis => {
            app.set_activity(Activity::Timing);
            app.set_canvas(Canvas::Timing);
            super::open_more_workspace(app, WorkspaceTab::Reports);
            app.sidebar_hidden = false;
        }
        LayoutPreset::Floorplan => {
            // Max die area: the sidebar is hidden for this preset.
            app.set_activity(Activity::Device);
            app.set_canvas(Canvas::Device);
            super::open_more_workspace(app, WorkspaceTab::Device);
            app.sidebar_hidden = true;
        }
        LayoutPreset::Simulate => {
            app.set_activity(Activity::Simulate);
            // Canvas stays where it was; the sim workspace takes the pane.
            super::open_more_workspace(app, WorkspaceTab::Wave);
            app.model.bottom_tab = BottomTab::SimLog;
            app.sidebar_hidden = false;
        }
    }
    app.layout_state.preset = preset;
}

/// Compact preset dropdown labeled with the current preset. Persists the
/// chrome state on every user change (not per frame).
pub fn paint_layout_selector(ui: &mut egui::Ui, app: &mut HelionIde) {
    let current = app.layout_state.preset;
    let mut picked: Option<LayoutPreset> = None;
    egui::ComboBox::from_id_salt("layout_preset")
        .selected_text(current.label())
        .show_ui(ui, |ui| {
            for preset in LayoutPreset::ALL {
                if ui
                    .selectable_label(preset == current, preset.label())
                    .clicked()
                {
                    picked = Some(preset);
                }
            }
        });
    if let Some(preset) = picked {
        if preset != current {
            apply_preset(app, preset);
            save_chrome_state(app);
        }
    }
}

/// Where the chrome ini lives. Mirrors `recent_store_path`'s dir logic with a
/// distinct filename (and its own override env var).
fn chrome_store_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("HELION_CHROME") {
        let pb = PathBuf::from(p.trim());
        if !pb.as_os_str().is_empty() {
            return Some(pb);
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".helion").join("chrome.ini"))
}

/// Ini key for a preset (stable across runs; `label()` is display text).
fn preset_name(preset: LayoutPreset) -> &'static str {
    match preset {
        LayoutPreset::Design => "Design",
        LayoutPreset::TimingAnalysis => "TimingAnalysis",
        LayoutPreset::Floorplan => "Floorplan",
        LayoutPreset::Simulate => "Simulate",
    }
}

fn parse_preset(s: &str) -> Option<LayoutPreset> {
    match s.trim() {
        "Design" => Some(LayoutPreset::Design),
        "TimingAnalysis" | "Timing Analysis" => Some(LayoutPreset::TimingAnalysis),
        "Floorplan" => Some(LayoutPreset::Floorplan),
        "Simulate" => Some(LayoutPreset::Simulate),
        _ => None,
    }
}

/// Luminance heuristic mirroring `theme::Theme::is_dark` (private there).
fn theme_is_dark(theme: &Theme) -> bool {
    let (r, g, b) = (
        theme.bg.r() as u32,
        theme.bg.g() as u32,
        theme.bg.b() as u32,
    );
    (r * 299 + g * 587 + b * 114) / 1000 < 128
}

/// Parsed `chrome.ini` values. Every field is optional so a missing or
/// corrupt file degrades to "keep the defaults" instead of failing.
#[derive(Default)]
struct ChromeIni {
    preset: Option<LayoutPreset>,
    sidebar_width: Option<f32>,
    console_height: Option<f32>,
    dark: Option<bool>,
    density: Option<Density>,
}

/// Parse the ini body; unknown keys and bad values are skipped silently.
fn parse_chrome_ini(text: &str) -> ChromeIni {
    let mut ini = ChromeIni::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "preset" => ini.preset = parse_preset(value),
            "sidebar_width" => ini.sidebar_width = value.trim().parse::<f32>().ok(),
            "console_height" => ini.console_height = value.trim().parse::<f32>().ok(),
            "theme" => match value.trim() {
                "dark" => ini.dark = Some(true),
                "light" => ini.dark = Some(false),
                _ => {}
            },
            "density" => match value.trim() {
                "compact" => ini.density = Some(Density::Compact),
                "comfortable" => ini.density = Some(Density::Comfortable),
                _ => {}
            },
            _ => {}
        }
    }
    ini
}

/// Persist sidebar width, console height, layout preset, theme and density.
/// Call only on user action - never per frame.
pub fn save_chrome_state(app: &HelionIde) {
    let Some(path) = chrome_store_path() else {
        return;
    };
    let theme_name = if theme_is_dark(&app.theme) {
        "dark"
    } else {
        "light"
    };
    let density_name = match app.theme.density {
        Density::Compact => "compact",
        Density::Comfortable => "comfortable",
    };
    let body = format!(
        "preset={}\nsidebar_width={:.1}\nconsole_height={:.1}\ntheme={theme_name}\ndensity={density_name}\n",
        preset_name(app.layout_state.preset),
        app.sidebar_width,
        app.console_height,
    );
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(dir);
        }
    }
    let _ = std::fs::write(&path, body);
}

/// Restore persisted chrome state. Called from `HelionIde::new`. Missing or
/// corrupt files are tolerated silently; explicit launch env hooks
/// (`HELION_WORKSPACE`, `HELION_ACTIVITY`, `HELION_FLOW`, `HELION_SIDEBAR_WIDTH`,
/// `HELION_CONSOLE_HEIGHT`) win over persisted values.
pub fn load_chrome_state(app: &mut HelionIde) {
    let Some(path) = chrome_store_path() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let ini = parse_chrome_ini(&text);
    if std::env::var("HELION_WORKSPACE").is_err()
        && std::env::var("HELION_ACTIVITY").is_err()
        && std::env::var("HELION_FLOW").is_err()
    {
        if let Some(p) = ini.preset {
            apply_preset(app, p);
        }
    }
    if std::env::var("HELION_SIDEBAR_WIDTH").is_err() {
        if let Some(w) = ini.sidebar_width {
            app.sidebar_width = w.clamp(chrome::SIDEBAR_MIN_WIDTH, chrome::SIDEBAR_MAX_WIDTH);
        }
    }
    if std::env::var("HELION_CONSOLE_HEIGHT").is_err() {
        if let Some(h) = ini.console_height {
            app.console_height = h.clamp(chrome::CONSOLE_MIN_HEIGHT, chrome::CONSOLE_MAX_HEIGHT);
        }
    }
    let mut theme = app.theme;
    if let Some(d) = ini.dark {
        theme = if d { Theme::dark() } else { Theme::light() };
    }
    if let Some(dy) = ini.density {
        theme.density = dy;
    }
    if ini.dark.is_some() || ini.density.is_some() {
        app.theme = theme;
        app.theme_dirty = true;
    }
}
