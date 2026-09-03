//! CAD chrome: three canvases, one activity rail, More ⋯ overflow.
//!
//! Paint calls these helpers; tests assert the same units at ~1440×900 without eframe.

use crate::{BottomTab, WorkspaceTab};

/// Typical desktop inner size.
pub const DESKTOP_WIDTH: f32 = 1440.0;
pub const DESKTOP_HEIGHT: f32 = 900.0;
/// 40px Helion activity rail (MUST 3).
pub const RAIL_WIDTH: f32 = 40.0;
/// One sidebar (MUST 5).
pub const SIDEBAR_WIDTH: f32 = 240.0;
/// Legacy aliases — side chrome is rail + sidebar, not 680.
pub const NAV_WIDTH: f32 = RAIL_WIDTH;
pub const TREE_WIDTH: f32 = SIDEBAR_WIDTH;
/// Properties dock — shown on selection, not always-on.
pub const PROPERTIES_WIDTH: f32 = 220.0;
pub const TOOLBAR_HEIGHT: f32 = 40.0;
pub const RAIL_MIN_HEIGHT: f32 = TOOLBAR_HEIGHT;
pub const STATUS_HEIGHT: f32 = 22.0;
pub const HIT_PRIMARY: f32 = 32.0;
pub const HIT_SIDEBAR: f32 = 28.0;
pub const HIT_SIDEBAR_ROW: f32 = HIT_SIDEBAR;
/// Bounded height for in-pane Name/Value grids so they cannot eat the CentralPanel.
pub const TABLE_MAX_HEIGHT: f32 = 180.0;
/// Device/Package tables stack above the canvas; keep them compact so the die expands.
pub const DEVICE_TABLES_MAX_HEIGHT: f32 = 220.0;
/// Floorplan / package canvas never shrinks below this if the pane has room.
pub const DRAWING_MIN_HEIGHT: f32 = 280.0;
/// Legacy alias — canvas expands; do not use as a max cap.
pub const DRAWING_MAX_HEIGHT: f32 = DRAWING_MIN_HEIGHT;
/// Minimum column width used to decide whether a grid clips its last column.
pub const MIN_COL_PX: f32 = 80.0;
pub const MORE: &str = "More ⋯";
pub const MORE_LABEL: &str = MORE;
const CHAR_PX: f32 = 7.0;
const TAB_PAD_PX: f32 = 16.0;

/// Example sources (empty state / File → Examples). Do not paint on the rail.
pub const RAIL_OPEN_SOURCES: [(&'static str, &'static str); 5] = [
    ("Open counter.sv", "counter.sv"),
    ("Open blinky.sv", "blinky.sv"),
    ("Open hier.sv", "hier.sv"),
    ("Open complex.sv", "complex.sv"),
    ("Open ysyx_ibex.sv", "ysyx_ibex.sv"),
];

/// On-screen canvases (MUST 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Canvas {
    Editor,
    Device,
    Timing,
}

impl Canvas {
    pub const ALL: [Canvas; 3] = [Canvas::Editor, Canvas::Device, Canvas::Timing];

    pub fn label(self) -> &'static str {
        match self {
            Canvas::Editor => "Editor",
            Canvas::Device => "Device",
            Canvas::Timing => "Timing",
        }
    }

    pub fn shortcut(self) -> &'static str {
        match self {
            Canvas::Editor => "⌘1",
            Canvas::Device => "⌘2",
            Canvas::Timing => "⌘3",
        }
    }

    pub fn parse_label(s: &str) -> Option<Self> {
        match s {
            "Editor" => Some(Canvas::Editor),
            "Device" => Some(Canvas::Device),
            "Timing" => Some(Canvas::Timing),
            _ => None,
        }
    }
}

/// Helion activity rail (MUST 3). One sidebar, not the 9-section vendor tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Files,
    Device,
    Timing,
    Simulate,
    Program,
    Reports,
}

impl Activity {
    pub const ALL: [Activity; 6] = [
        Activity::Files,
        Activity::Device,
        Activity::Timing,
        Activity::Simulate,
        Activity::Program,
        Activity::Reports,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Activity::Files => "Files",
            Activity::Device => "Device",
            Activity::Timing => "Timing",
            Activity::Simulate => "Simulate",
            Activity::Program => "Program",
            Activity::Reports => "Reports",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Activity::Files => "F",
            Activity::Device => "D",
            Activity::Timing => "T",
            Activity::Simulate => "S",
            Activity::Program => "P",
            Activity::Reports => "R",
        }
    }

    pub fn tcl(self) -> &'static str {
        match self {
            Activity::Files => "open_source",
            Activity::Device => "device",
            Activity::Timing => "report_timing",
            Activity::Simulate => "simulation",
            Activity::Program => "program_hw",
            Activity::Reports => "reports",
        }
    }

    pub fn shortcut(self) -> &'static str {
        match self {
            Activity::Files => "⌘1",
            Activity::Device => "⌘2",
            Activity::Timing => "⌘3",
            Activity::Simulate | Activity::Program | Activity::Reports => "",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverflowMode {
    /// Every label fits on one row.
    Fit,
    /// A single non-wrapping, non-scrolling row would drop trailing labels.
    Clip,
    /// Labels wrap onto extra rows so the last one stays on screen.
    Wrap,
    /// One row with horizontal scroll + chevrons.
    Scroll,
    /// One row; overflow goes into a More ⋯ menu (shipped).
    More,
}

impl OverflowMode {
    pub fn keeps_trailing(self) -> bool {
        matches!(
            self,
            OverflowMode::Fit | OverflowMode::Wrap | OverflowMode::Scroll | OverflowMode::More
        )
    }

    pub fn steals_canvas(self) -> bool {
        matches!(self, OverflowMode::Wrap)
    }
}

/// In-pane table scroll policy used by `data_scroll` in helion-ide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableScrollPolicy {
    pub x: bool,
    pub y: bool,
    pub max_height: f32,
    pub last_column_would_clip: bool,
}

/// Planned chrome at a window width.
#[derive(Clone, Debug)]
pub struct ChromeOverflow {
    pub window_w: f32,
    pub workspace_available: f32,
    pub rail_available: f32,
    pub workspace_tabs: Vec<&'static str>,
    pub rail_actions: Vec<&'static str>,
    pub bottom_tabs: Vec<&'static str>,
    pub nav_sections: Vec<&'static str>,
    pub tab_rows: Vec<Vec<&'static str>>,
    pub rail_rows: Vec<Vec<&'static str>>,
    pub more_items: Vec<&'static str>,
    pub naive_workspace_mode: OverflowMode,
    pub workspace_mode: OverflowMode,
    pub rail_mode: OverflowMode,
    pub bottom_mode: OverflowMode,
    pub table: TableScrollPolicy,
}

impl ChromeOverflow {
    pub fn tab_is_selectable(&self, label: &str) -> bool {
        if self.workspace_mode == OverflowMode::More {
            return self.workspace_tabs.iter().any(|l| *l == label);
        }
        self.workspace_mode.keeps_trailing()
            && self
                .tab_rows
                .iter()
                .any(|row| row.iter().any(|l| *l == label))
    }

    pub fn rail_is_selectable(&self, label: &str) -> bool {
        self.rail_mode.keeps_trailing()
            && self
                .rail_rows
                .iter()
                .any(|row| row.iter().any(|l| *l == label))
    }

    pub fn dropped_workspace(&self) -> usize {
        if self.workspace_mode == OverflowMode::More {
            return 0;
        }
        self.workspace_tabs
            .iter()
            .filter(|l| !self.tab_rows.iter().any(|row| row.contains(*l)))
            .count()
    }

    pub fn dropped_rail(&self) -> usize {
        self.rail_actions
            .iter()
            .filter(|l| !self.rail_rows.iter().any(|row| row.contains(*l)))
            .count()
    }
}

pub fn label_extent(label: &str) -> f32 {
    (label.chars().count() as f32).mul_add(CHAR_PX, TAB_PAD_PX)
}

/// Side docks that steal width from the workspace tab strip (rail + one sidebar).
pub fn side_chrome_width() -> f32 {
    RAIL_WIDTH + SIDEBAR_WIDTH
}

pub fn side_chrome_collapsed() -> f32 {
    RAIL_WIDTH
}

pub fn workspace_tab_labels() -> Vec<&'static str> {
    Canvas::ALL.iter().map(|c| c.label()).collect()
}

pub fn rail_action_labels() -> Vec<&'static str> {
    Activity::ALL.iter().map(|a| a.label()).collect()
}

pub fn bottom_tab_labels() -> Vec<&'static str> {
    BottomTab::HOME.iter().map(|t| t.paint_label()).collect()
}

pub fn nav_section_labels() -> Vec<&'static str> {
    Activity::ALL.iter().map(|a| a.label()).collect()
}

pub fn wrap_labels(labels: &[&'static str], available: f32) -> Vec<Vec<&'static str>> {
    let avail = available.max(1.0);
    let mut rows: Vec<Vec<&'static str>> = vec![Vec::new()];
    let mut used = 0.0f32;
    for &lab in labels {
        let w = label_extent(lab);
        if !rows.last().map(|r| r.is_empty()).unwrap_or(true) && used + w > avail {
            rows.push(Vec::new());
            used = 0.0;
        }
        rows.last_mut().unwrap().push(lab);
        used += w;
    }
    rows
}

/// Labels that remain on a single non-wrapping, non-scrolling row (the rest clip).
pub fn visible_if_clipped(labels: &[&'static str], available: f32) -> Vec<&'static str> {
    let avail = available.max(1.0);
    let mut used = 0.0f32;
    let mut vis = Vec::new();
    for &lab in labels {
        let w = label_extent(lab);
        if !vis.is_empty() && used + w > avail {
            break;
        }
        vis.push(lab);
        used += w;
    }
    vis
}

pub fn would_clip(labels: &[&'static str], available: f32) -> bool {
    visible_if_clipped(labels, available).len() < labels.len()
}

/// One row + More ⋯. Never wrap (wrap steals canvas).
pub fn fit_or_more(
    labels: &[&'static str],
    available: f32,
) -> (Vec<&'static str>, Vec<&'static str>) {
    if !would_clip(labels, available) {
        return (labels.to_vec(), Vec::new());
    }
    let vis = visible_if_clipped(labels, (available - label_extent(MORE)).max(1.0));
    let overflow: Vec<_> = labels.iter().copied().filter(|l| !vis.contains(l)).collect();
    let mut row = vis;
    row.push(MORE);
    (row, overflow)
}

/// Naive one-row strip: Fit or Clip. Paint must not use Clip.
pub fn naive_tab_overflow(available: f32) -> OverflowMode {
    let labels = workspace_tab_labels();
    if would_clip(&labels, available) {
        OverflowMode::Clip
    } else {
        OverflowMode::Fit
    }
}

/// Shipped workspace strip: one row + More ⋯. Never wrap.
pub fn workspace_tab_overflow(available: f32) -> OverflowMode {
    match naive_tab_overflow(available) {
        OverflowMode::Clip => OverflowMode::More,
        other => other,
    }
}

fn bottom_mode(available: f32) -> OverflowMode {
    let labels = bottom_tab_labels();
    if would_clip(&labels, available) {
        OverflowMode::More
    } else {
        OverflowMode::Fit
    }
}

/// Last grid column is off-pane if `n_cols * col_w` exceeds available width.
pub fn grid_clips_last_column(n_cols: usize, col_w: f32, available: f32) -> bool {
    n_cols as f32 * col_w.max(1.0) > available.max(1.0)
}

/// Scale HAD die / package cells so the whole drawing fits `avail` (view at once).
pub fn floorplan_fit_cell(cols: u32, rows: u32, avail_w: f32, avail_h: f32) -> f32 {
    let cw = (avail_w - 28.0).max(8.0) / cols.max(1) as f32;
    let ch = (avail_h - 16.0).max(8.0) / rows.max(1) as f32;
    cw.min(ch).clamp(4.0, 24.0)
}

pub fn floorplan_fits_viewport(
    cols: u32,
    rows: u32,
    cell: f32,
    avail_w: f32,
    avail_h: f32,
) -> bool {
    cell * cols.max(1) as f32 + 28.0 <= avail_w + 1.0
        && cell * rows.max(1) as f32 + 16.0 <= avail_h + 1.0
}

/// Paint `data_scroll` reads this: both axes + bounded height.
pub fn table_scroll_policy(n_cols: usize, available: f32) -> TableScrollPolicy {
    let last_column_would_clip = grid_clips_last_column(n_cols, MIN_COL_PX, available);
    TableScrollPolicy {
        x: true,
        y: true,
        max_height: TABLE_MAX_HEIGHT,
        last_column_would_clip,
    }
}

/// Plan chrome at `window_w` (inner size). Never drops a trailing control.
pub fn chrome_at(window_w: f32) -> ChromeOverflow {
    let workspace_available = (window_w - side_chrome_width()).max(160.0);
    let rail_available = window_w.max(160.0);
    let workspace_tabs = workspace_tab_labels();
    let rail_actions = rail_action_labels();
    let bottom_tabs = bottom_tab_labels();
    let nav_sections = nav_section_labels();
    let workspace_mode = workspace_tab_overflow(workspace_available);
    let (row, more_items) = fit_or_more(&workspace_tabs, workspace_available);
    ChromeOverflow {
        window_w,
        workspace_available,
        rail_available,
        naive_workspace_mode: naive_tab_overflow(workspace_available),
        workspace_mode,
        rail_mode: OverflowMode::Fit,
        bottom_mode: bottom_mode(rail_available),
        tab_rows: vec![row],
        rail_rows: vec![rail_actions.clone()],
        more_items,
        workspace_tabs,
        rail_actions,
        bottom_tabs,
        nav_sections,
        table: table_scroll_policy(10, workspace_available),
    }
}

/// Silence unused WorkspaceTab import helper — canvases stay in sync with CANVASES.
pub fn workspace_matches_canvases() -> bool {
    WorkspaceTab::CANVASES.len() == Canvas::ALL.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_overflow_keeps_every_tab_and_rail_action_selectable_at_desktop_width() {
        assert_eq!(side_chrome_width(), RAIL_WIDTH + SIDEBAR_WIDTH);
        assert_eq!(side_chrome_width(), 280.0);
        assert_ne!(side_chrome_width(), 680.0);
        assert_eq!(RAIL_WIDTH, 40.0);
        assert_eq!(SIDEBAR_WIDTH, 240.0);
        assert_eq!(HIT_PRIMARY, 32.0);
        assert_eq!(HIT_SIDEBAR, 28.0);

        let labels = workspace_tab_labels();
        assert_eq!(labels, vec!["Editor", "Device", "Timing"]);
        assert_eq!(labels.len(), 3);
        assert_eq!(WorkspaceTab::ALL.len(), 28);
        assert_eq!(WorkspaceTab::CANVASES.len(), 3);

        let avail = DESKTOP_WIDTH - side_chrome_width();
        assert!(!would_clip(&labels, avail), "3 canvases must fit at 1440 (avail={avail})");
        assert_eq!(naive_tab_overflow(avail), OverflowMode::Fit);
        assert_eq!(workspace_tab_overflow(avail), OverflowMode::Fit);
        assert!(OverflowMode::Wrap.steals_canvas());
        assert!(!OverflowMode::Fit.steals_canvas());
        assert!(!OverflowMode::More.steals_canvas());

        let plan = chrome_at(DESKTOP_WIDTH);
        assert_eq!(plan.window_w, 1440.0);
        assert_eq!(plan.workspace_mode, OverflowMode::Fit);
        assert_ne!(plan.workspace_mode, OverflowMode::Wrap);
        assert_eq!(plan.tab_rows.len(), 1);
        assert_eq!(plan.workspace_tabs, vec!["Editor", "Device", "Timing"]);
        for lab in ["Editor", "Device", "Timing"] {
            assert!(plan.tab_is_selectable(lab), "canvas {lab}");
        }
        assert_eq!(plan.dropped_workspace(), 0);

        assert_eq!(
            plan.rail_actions,
            vec!["Files", "Device", "Timing", "Simulate", "Program", "Reports"]
        );
        for a in Activity::ALL {
            assert!(plan.rail_is_selectable(a.label()), "rail {}", a.label());
        }
        for (label, _) in RAIL_OPEN_SOURCES {
            assert!(!plan.rail_actions.iter().any(|l| *l == label), "{label} on rail");
            assert!(!plan.rail_is_selectable(label));
        }
        assert!(!plan.rail_actions.iter().any(|l| l.starts_with("Open ")));
        assert_eq!(plan.dropped_rail(), 0);
        assert_eq!(plan.nav_sections.len(), 6);
        assert_ne!(plan.nav_sections.len(), 9);

        assert_eq!(plan.bottom_tabs, vec!["Console", "Messages"]);
        assert!(!plan.bottom_tabs.contains(&"Tcl Console"));
        assert!(!plan.bottom_tabs.contains(&"Log"));
        assert!(!plan.bottom_tabs.contains(&"Simulation Log"));

        assert!(grid_clips_last_column(10, 80.0, 400.0));
        let table = table_scroll_policy(10, 400.0);
        assert!(table.last_column_would_clip && table.x && table.y);
        assert_eq!(table.max_height, TABLE_MAX_HEIGHT);

        let squeezed = chrome_at(1100.0);
        assert_eq!(squeezed.workspace_mode, OverflowMode::Fit);
        assert_eq!(squeezed.tab_rows.len(), 1);
        assert!(!squeezed.workspace_mode.steals_canvas());
        assert!(!squeezed.rail_is_selectable("Open hier.sv"));

        let tiny = chrome_at(200.0);
        assert_eq!(tiny.workspace_mode, OverflowMode::More);
        assert_eq!(tiny.tab_rows.len(), 1);
        assert!(
            tiny.tab_rows.iter().any(|r| r.contains(&MORE)),
            "More ⋯: {:?}",
            tiny.tab_rows
        );
        assert_eq!(tiny.dropped_workspace(), 0);
        assert!(tiny.tab_is_selectable("Timing"));

        let cell = floorplan_fit_cell(32, 33, 800.0, 500.0);
        assert!(floorplan_fits_viewport(32, 33, cell, 800.0, 500.0));
        assert!(cell >= 4.0 && cell <= 24.0);
        assert!(DEVICE_TABLES_MAX_HEIGHT < DESKTOP_HEIGHT / 3.0);
        assert!(DRAWING_MIN_HEIGHT > TABLE_MAX_HEIGHT);
        assert!(workspace_matches_canvases());
    }
}
