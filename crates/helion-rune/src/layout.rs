//! Shell layout: which docks show, how big they are, named presets, and the
//! drag splitters that resize them.
//!
//! The shell is Flow Navigator | Sources column | center pane over a bottom
//! console dock. Every dock can be hidden (View menu, pane header ×, ⌘J for
//! the sidebar docks) and the center pane can be maximized (focus mode hides
//! every dock until restored). Sizes come from drag splitters and are clamped
//! so no dock can swallow the center pane.
//!
//! Persistence: [`load`] and [`save`] round-trip the layout, theme and recent
//! projects through `rune::prefs` (`prefs.json` in the platform config dir).
//! Only the interactive boot path loads them; tests and headless scenes start
//! from the defaults so snapshots never depend on the user's machine.

use rune::{Cursor, Drag, El, Phase, Tx, el, row};

use crate::state::{AppState, CenterPane};
use crate::theme::{ThemeMode, VivadoTheme};
use crate::views::console::ConsoleTab;

/// Default bottom dock height (transcript + toolbar + tab strip).
pub const CONSOLE_H: f32 = 216.0;
pub const CONSOLE_MIN_H: f32 = 96.0;
pub const CONSOLE_MAX_H: f32 = 560.0;
/// Default Sources column width.
pub const SOURCES_W: f32 = 300.0;
pub const SOURCES_MIN_W: f32 = 220.0;
pub const SOURCES_MAX_W: f32 = 560.0;
/// Splitter hit thickness (the visible rule is 1pt in the middle).
const SPLIT_HIT: f32 = 6.0;
/// Most recent projects kept on the Start page and File menu.
pub const RECENT_MAX: usize = 8;
/// `rune::prefs` application id.
const PREFS_ID: &str = "dev.helion.ide";

/// Named arrangements of docks + center surface (old `layouts.rs` presets).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayoutPreset {
    #[default]
    Default,
    Design,
    TimingAnalysis,
    Floorplan,
    Simulate,
}

impl LayoutPreset {
    pub const ALL: [LayoutPreset; 5] = [
        LayoutPreset::Default,
        LayoutPreset::Design,
        LayoutPreset::TimingAnalysis,
        LayoutPreset::Floorplan,
        LayoutPreset::Simulate,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LayoutPreset::Default => "Default Layout",
            LayoutPreset::Design => "Design",
            LayoutPreset::TimingAnalysis => "Timing Analysis",
            LayoutPreset::Floorplan => "Floorplan",
            LayoutPreset::Simulate => "Simulate",
        }
    }

    /// Stable id for prefs and the `layout` Tcl verb.
    pub fn id(self) -> &'static str {
        match self {
            LayoutPreset::Default => "default",
            LayoutPreset::Design => "design",
            LayoutPreset::TimingAnalysis => "timing",
            LayoutPreset::Floorplan => "floorplan",
            LayoutPreset::Simulate => "simulate",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        Self::ALL.into_iter().find(|p| {
            p.id().eq_ignore_ascii_case(raw)
                || p.label().eq_ignore_ascii_case(raw)
                || p.label().replace(' ', "").eq_ignore_ascii_case(raw)
        })
    }
}

/// Dock visibility and sizes. Session chrome: survives project reseeds.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutState {
    pub preset: LayoutPreset,
    /// Sizes or visibility changed since the preset was applied.
    pub customized: bool,
    pub show_flow: bool,
    pub show_sources: bool,
    pub show_console: bool,
    /// Center pane maximized: every dock hidden until restored.
    pub focus: bool,
    pub console_h: f32,
    pub sources_w: f32,
    /// Sources tree body collapsed to its header (properties card grows).
    pub sources_tree_collapsed: bool,
    /// Source File Properties card collapsed to its header.
    pub props_collapsed: bool,
    /// Open center views, left to right (the view tab strip).
    pub open_views: Vec<CenterPane>,
    /// Splitter drag in flight: the size at press, so cancel can restore.
    drag_from: Option<f32>,
}

impl Default for LayoutState {
    fn default() -> Self {
        Self {
            preset: LayoutPreset::Default,
            customized: false,
            show_flow: true,
            show_sources: true,
            show_console: true,
            focus: false,
            console_h: CONSOLE_H,
            sources_w: SOURCES_W,
            sources_tree_collapsed: false,
            props_collapsed: false,
            open_views: vec![CenterPane::Summary],
            drag_from: None,
        }
    }
}

impl LayoutState {
    /// Effective visibility after focus mode.
    pub fn flow_visible(&self) -> bool {
        self.show_flow && !self.focus
    }
    pub fn sources_visible(&self) -> bool {
        self.show_sources && !self.focus
    }
    pub fn console_visible(&self) -> bool {
        self.show_console && !self.focus
    }

    /// Status-bar label: the preset, marked when the user tweaked it.
    pub fn label(&self) -> String {
        if self.focus {
            "Maximized".into()
        } else if self.customized {
            format!("{} (modified)", self.preset.label())
        } else {
            self.preset.label().into()
        }
    }
}

/// Apply a preset: dock visibility, sizes and the center surface it is for.
pub fn apply_preset(s: &mut AppState, preset: LayoutPreset) {
    let l = &mut s.layout;
    l.preset = preset;
    l.customized = false;
    l.focus = false;
    l.console_h = CONSOLE_H;
    l.sources_w = SOURCES_W;
    l.sources_tree_collapsed = false;
    l.props_collapsed = false;
    l.show_flow = true;
    l.show_sources = true;
    l.show_console = true;
    s.console.collapsed = false;
    match preset {
        LayoutPreset::Default => {}
        LayoutPreset::Design => {
            s.layout.props_collapsed = true;
            s.navigate(CenterPane::Editor);
            s.console.tab = ConsoleTab::Tcl;
        }
        LayoutPreset::TimingAnalysis => {
            s.layout.show_sources = false;
            s.navigate(CenterPane::Timing);
            s.console.tab = ConsoleTab::Messages;
        }
        LayoutPreset::Floorplan => {
            s.layout.show_sources = false;
            s.console.collapsed = true;
            s.navigate(CenterPane::Device);
        }
        LayoutPreset::Simulate => {
            s.layout.show_flow = false;
            s.navigate(CenterPane::Wave);
            s.console.tab = ConsoleTab::Log;
            s.console.log_run = crate::views::console::LogRun::Simulation;
        }
    }
    save(s);
}

/// Toggle the center-pane maximize (focus) mode.
pub fn toggle_focus(s: &mut AppState) {
    s.layout.focus = !s.layout.focus;
}

/// ⌘J: hide/show both sidebar docks together (old sidebar toggle). Leaving
/// focus mode first so the shortcut always produces a visible change.
pub fn toggle_sidebar(s: &mut AppState) {
    if s.layout.focus {
        s.layout.focus = false;
        return;
    }
    let show = !(s.layout.show_flow || s.layout.show_sources);
    s.layout.show_flow = show;
    s.layout.show_sources = show;
    s.layout.customized = true;
    save(s);
}

pub fn set_flow_visible(s: &mut AppState, on: bool) {
    s.layout.focus = false;
    s.layout.show_flow = on;
    s.layout.customized = true;
    save(s);
}

pub fn set_sources_visible(s: &mut AppState, on: bool) {
    s.layout.focus = false;
    s.layout.show_sources = on;
    s.layout.customized = true;
    save(s);
}

pub fn set_console_visible(s: &mut AppState, on: bool) {
    s.layout.focus = false;
    s.layout.show_console = on;
    if on {
        s.console.collapsed = false;
    }
    s.layout.customized = true;
    save(s);
}

/// Horizontal splitter above the console dock: drag up to grow it, double
/// click restores the default height.
pub fn console_splitter(t: &VivadoTheme) -> El {
    row()
        .key("console-splitter")
        .role("separator")
        .label("Resize console")
        .w_full()
        .h(SPLIT_HIT)
        .items_center()
        .cursor(Cursor::ResizeRow)
        .on_drag(|s: &mut AppState, d: &mut Drag, _tx: &mut Tx| {
            drag_size(s, d, |l| &mut l.console_h, CONSOLE_MIN_H, CONSOLE_MAX_H, -1.0);
        })
        .on_double_press(|s: &mut AppState| {
            s.layout.console_h = CONSOLE_H;
            s.layout.customized = true;
            save(s);
        })
        .child(el().w_full().h(1.0).fill(t.border_light).a11y_hidden())
}

/// Vertical splitter right of the Sources column: drag to widen it.
pub fn sources_splitter(t: &VivadoTheme) -> El {
    row()
        .key("sources-splitter")
        .role("separator")
        .label("Resize sources")
        .w(SPLIT_HIT)
        .h_full()
        .justify_center()
        .cursor(Cursor::ResizeColumn)
        .on_drag(|s: &mut AppState, d: &mut Drag, _tx: &mut Tx| {
            drag_size(s, d, |l| &mut l.sources_w, SOURCES_MIN_W, SOURCES_MAX_W, 1.0);
        })
        .on_double_press(|s: &mut AppState| {
            s.layout.sources_w = SOURCES_W;
            s.layout.customized = true;
            save(s);
        })
        .child(el().w(1.0).h_full().fill(t.border_light).a11y_hidden())
}

/// Shared splitter gesture: remember the size at press, follow the pointer
/// (sign flips for docks that grow against the drag axis), restore on
/// cancel, persist on release.
fn drag_size(
    s: &mut AppState,
    d: &mut Drag,
    field: fn(&mut LayoutState) -> &mut f32,
    min: f32,
    max: f32,
    sign: f32,
) {
    match d.phase {
        Phase::Down => {
            let v = *field(&mut s.layout);
            s.layout.drag_from = Some(v);
            d.data = v;
        }
        Phase::Move | Phase::Up => {
            if d.cancelled() {
                if let Some(v) = s.layout.drag_from.take() {
                    *field(&mut s.layout) = v;
                }
                return;
            }
            let delta = if sign < 0.0 { -d.dy } else { d.dx };
            *field(&mut s.layout) = (d.data + delta).clamp(min, max);
            if d.phase == Phase::Up {
                s.layout.drag_from = None;
                s.layout.customized = true;
                save(s);
            }
        }
    }
}

/// Keep the current center view in the tab strip (views can be opened by
/// menus, rail, palette, Tcl or cross-probes; all land here).
pub fn sync_tabs(s: &mut AppState) -> bool {
    if s.layout.open_views.contains(&s.center) {
        return false;
    }
    s.layout.open_views.push(s.center);
    true
}

/// Close a view tab. Closing the active tab activates its left neighbour
/// (else the right one); the last tab never closes.
pub fn close_tab(s: &mut AppState, pane: CenterPane) {
    let tabs = &mut s.layout.open_views;
    let Some(i) = tabs.iter().position(|p| *p == pane) else {
        return;
    };
    if tabs.len() <= 1 {
        return;
    }
    tabs.remove(i);
    s.nav_back.retain(|p| *p != pane);
    s.nav_forward.retain(|p| *p != pane);
    if s.center == pane {
        let next = tabs[i.saturating_sub(1).min(tabs.len() - 1)];
        s.center = next;
    }
}

/// Close every tab but `keep`.
pub fn close_other_tabs(s: &mut AppState, keep: CenterPane) {
    s.layout.open_views.retain(|p| *p == keep);
    if s.layout.open_views.is_empty() {
        s.layout.open_views.push(keep);
    }
    s.center = keep;
}

/// The view tab strip over the center pane: one tab per open view, active
/// tab raised with an accent underline, × to close (hidden on the last tab).
/// Middle of the strip scrolls sideways when many views are open.
pub fn view_tabs(s: &AppState, t: &VivadoTheme) -> El {
    use rune::{col, text};
    let mut tabs = s.layout.open_views.clone();
    if !tabs.contains(&s.center) {
        tabs.push(s.center);
    }
    let closable = tabs.len() > 1;
    let mut strip = row()
        .key("view-tabs")
        .role("tablist")
        .w_full()
        .h(24.0)
        .items_end()
        .gap(1.0)
        .px(4.0)
        .fill(t.header_bg);
    for pane in tabs {
        let active = pane == s.center;
        let label = crate::views::palette::pane_label(pane);
        let mut tab = row()
            .key(format!("view-tab-{pane:?}"))
            .role("tab")
            .label(label)
            .focus_selected(active)
            .focusable(true)
            .h(23.0)
            .pl(12.0)
            .pr(if closable { 4.0 } else { 12.0 })
            .gap(6.0)
            .items_center()
            .radius(4.0)
            .fill(if active { t.panel_bg } else { t.header_bg })
            .hover(t.selection)
            .on_press(move |s: &mut AppState| s.navigate(pane))
            .child(
                col()
                    .h_full()
                    .justify_center()
                    .child(
                        text(label)
                            .font(if active {
                                VivadoTheme::FONT_TITLE
                            } else {
                                VivadoTheme::FONT_UI
                            })
                            .color(if active { t.text } else { t.text_dim }),
                    ),
            );
        if closable {
            tab = tab.child(
                el().key(format!("view-tab-close-{pane:?}"))
                    .role("button")
                    .label(format!("Close {label}"))
                    .help("Close tab")
                    .focusable(true)
                    .size(18.0, 18.0)
                    .radius(3.0)
                    .center()
                    .hover(t.border_light)
                    .on_press(move |s: &mut AppState| close_tab(s, pane))
                    .child(text("×").font(VivadoTheme::FONT_UI).color(t.text_faint)),
            );
        }
        if active {
            tab = tab.border_sides([0.0, 0.0, 2.0, 0.0], t.accent);
        }
        strip = strip.child(tab);
    }
    let maximize = el()
        .key("view-tabs-max")
        .role("button")
        .label(if s.layout.focus {
            "Restore layout"
        } else {
            "Maximize view"
        })
        .help("Maximize or restore the view (⇧⌘M)")
        .focusable(true)
        .size(22.0, 20.0)
        .radius(3.0)
        .center()
        .hover(t.selection)
        .on_press(toggle_focus)
        .child(
            text(if s.layout.focus { "⤡" } else { "⤢" })
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim),
        );
    row()
        .w_full()
        .h(24.0)
        .items_center()
        .fill(t.header_bg)
        .border(1.0, t.border)
        .child(
            el().grow(1.0)
                .min_w(0.0)
                .h_full()
                .scroll_x()
                .child(strip),
        )
        .child(el().px(4.0).child(maximize))
}

/// Record a project in the recent list (newest first, deduped, capped).
pub fn remember_project(s: &mut AppState, path: &str) {
    if path.is_empty() {
        return;
    }
    s.recent.retain(|p| p != path);
    s.recent.insert(0, path.to_string());
    s.recent.truncate(RECENT_MAX);
    save(s);
}

#[cfg(not(test))]
fn store() -> Option<rune::prefs::Prefs> {
    // Headless runs (`--snap`, `--scene`, scripts) never touch the user's
    // prefs; only the interactive boot sets this.
    PERSIST
        .load(std::sync::atomic::Ordering::Relaxed)
        .then(|| rune::prefs::Prefs::open(PREFS_ID))
}

#[cfg(test)]
fn store() -> Option<rune::prefs::Prefs> {
    None
}

/// Set once by the interactive boot path so layout changes persist.
static PERSIST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Enable persistence and load saved layout, theme and recents into `s`.
pub fn load(s: &mut AppState) {
    PERSIST.store(true, std::sync::atomic::Ordering::Relaxed);
    let Some(p) = store() else { return };
    if let Some(id) = p.get::<String>("layout.preset")
        && let Some(preset) = LayoutPreset::parse(&id)
    {
        s.layout.preset = preset;
    }
    let l = &mut s.layout;
    if let Some(v) = p.get::<f32>("layout.console_h") {
        l.console_h = v.clamp(CONSOLE_MIN_H, CONSOLE_MAX_H);
    }
    if let Some(v) = p.get::<f32>("layout.sources_w") {
        l.sources_w = v.clamp(SOURCES_MIN_W, SOURCES_MAX_W);
    }
    if let Some(v) = p.get::<bool>("layout.show_flow") {
        l.show_flow = v;
    }
    if let Some(v) = p.get::<bool>("layout.show_sources") {
        l.show_sources = v;
    }
    if let Some(v) = p.get::<bool>("layout.show_console") {
        l.show_console = v;
    }
    if let Some(v) = p.get::<bool>("layout.customized") {
        l.customized = v;
    }
    if let Some(theme) = p.get::<String>("theme") {
        s.theme = match theme.as_str() {
            "light" => ThemeMode::Light,
            "dark" => ThemeMode::Dark,
            _ => ThemeMode::System,
        };
    }
    if let Some(recent) = p.get::<Vec<String>>("recent_projects") {
        s.recent = recent.into_iter().take(RECENT_MAX).collect();
    }
}

/// Persist layout, theme and recents (no-op unless the interactive boot
/// enabled persistence). Write errors are not fatal: settings stay in memory.
pub fn save(s: &AppState) {
    let Some(mut p) = store() else { return };
    let l = &s.layout;
    let theme = match s.theme {
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
        ThemeMode::System => "system",
    };
    let _ = p.set("layout.preset", l.preset.id());
    let _ = p.set("layout.console_h", &l.console_h);
    let _ = p.set("layout.sources_w", &l.sources_w);
    let _ = p.set("layout.show_flow", &l.show_flow);
    let _ = p.set("layout.show_sources", &l.show_sources);
    let _ = p.set("layout.show_console", &l.show_console);
    let _ = p.set("layout.customized", &l.customized);
    let _ = p.set("theme", theme);
    let _ = p.set("recent_projects", &s.recent);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_parse_by_id_and_label() {
        for p in LayoutPreset::ALL {
            assert_eq!(LayoutPreset::parse(p.id()), Some(p));
            assert_eq!(LayoutPreset::parse(p.label()), Some(p));
        }
        assert_eq!(
            LayoutPreset::parse("timinganalysis"),
            Some(LayoutPreset::TimingAnalysis)
        );
        assert_eq!(LayoutPreset::parse("nope"), None);
    }

    #[test]
    fn focus_hides_every_dock_and_restores() {
        let mut s = crate::init();
        assert!(s.layout.flow_visible() && s.layout.sources_visible());
        toggle_focus(&mut s);
        assert!(!s.layout.flow_visible());
        assert!(!s.layout.sources_visible());
        assert!(!s.layout.console_visible());
        assert_eq!(s.layout.label(), "Maximized");
        toggle_focus(&mut s);
        assert!(s.layout.flow_visible() && s.layout.console_visible());
    }

    #[test]
    fn presets_pick_their_surface_and_docks() {
        let mut s = crate::init();
        apply_preset(&mut s, LayoutPreset::TimingAnalysis);
        assert_eq!(s.center, CenterPane::Timing);
        assert!(!s.layout.show_sources);
        apply_preset(&mut s, LayoutPreset::Floorplan);
        assert_eq!(s.center, CenterPane::Device);
        assert!(s.console.collapsed);
        apply_preset(&mut s, LayoutPreset::Default);
        assert!(s.layout.show_sources && !s.console.collapsed);
        assert_eq!(s.layout.label(), "Default Layout");
    }

    #[test]
    fn sidebar_toggle_hides_then_shows_both_docks() {
        let mut s = crate::init();
        toggle_sidebar(&mut s);
        assert!(!s.layout.show_flow && !s.layout.show_sources);
        assert!(s.layout.label().ends_with("(modified)"));
        toggle_sidebar(&mut s);
        assert!(s.layout.show_flow && s.layout.show_sources);
    }

    #[test]
    fn recent_projects_dedupe_newest_first_and_cap() {
        let mut s = crate::init();
        for i in 0..12 {
            remember_project(&mut s, &format!("/p/{i}.prj"));
        }
        remember_project(&mut s, "/p/3.prj");
        assert_eq!(s.recent.len(), RECENT_MAX);
        assert_eq!(s.recent[0], "/p/3.prj");
        assert_eq!(s.recent.iter().filter(|p| *p == "/p/3.prj").count(), 1);
    }
}
