//! CAD chrome overflow: wrap or scroll trailing labels so they stay selectable.
//!
//! Paint calls these helpers; tests assert the same units at ~1440×900 without eframe.

use crate::{BottomTab, FlowStep, LayoutKind, NavSection, WorkspaceTab};

/// UG893-typical desktop inner size.
pub const DESKTOP_WIDTH: f32 = 1440.0;
pub const DESKTOP_HEIGHT: f32 = 900.0;
/// Default dock widths (Flow Navigator + Sources/Netlist + Properties).
pub const NAV_WIDTH: f32 = 220.0;
pub const TREE_WIDTH: f32 = 240.0;
pub const PROPERTIES_WIDTH: f32 = 220.0;
/// Two-row rail so Open-source actions are not clipped off the first row.
pub const RAIL_MIN_HEIGHT: f32 = 72.0;
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
const CHAR_PX: f32 = 7.0;
const TAB_PAD_PX: f32 = 16.0;

/// Open-example rail actions (painted as buttons, must remain selectable).
pub const RAIL_OPEN_SOURCES: [(&'static str, &'static str); 5] = [
    ("Open counter.sv", "counter.sv"),
    ("Open blinky.sv", "blinky.sv"),
    ("Open hier.sv", "hier.sv"),
    ("Open complex.sv", "complex.sv"),
    ("Open ysyx_ibex.sv", "ysyx_ibex.sv"),
];

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
}

impl OverflowMode {
    pub fn keeps_trailing(self) -> bool {
        matches!(
            self,
            OverflowMode::Fit | OverflowMode::Wrap | OverflowMode::Scroll
        )
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
    pub naive_workspace_mode: OverflowMode,
    pub workspace_mode: OverflowMode,
    pub rail_mode: OverflowMode,
    pub bottom_mode: OverflowMode,
    pub table: TableScrollPolicy,
}

impl ChromeOverflow {
    pub fn tab_is_selectable(&self, label: &str) -> bool {
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

/// Side docks that steal width from the workspace tab strip (default layout).
pub fn side_chrome_width() -> f32 {
    NAV_WIDTH + TREE_WIDTH + PROPERTIES_WIDTH
}

pub fn workspace_tab_labels() -> Vec<&'static str> {
    WorkspaceTab::ALL.iter().map(|t| t.label()).collect()
}

pub fn rail_action_labels() -> Vec<&'static str> {
    let mut v = vec!["Layout"];
    v.extend(LayoutKind::ALL.iter().map(|l| l.label()));
    v.push("Flow");
    v.extend(FlowStep::ALL.iter().map(|s| s.label()));
    v.extend(RAIL_OPEN_SOURCES.iter().map(|(label, _)| *label));
    v
}

pub fn bottom_tab_labels() -> Vec<&'static str> {
    BottomTab::ALL.iter().map(|t| t.label()).collect()
}

pub fn nav_section_labels() -> Vec<&'static str> {
    NavSection::ALL.iter().map(|s| s.label()).collect()
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

/// Naive one-row strip: Fit or Clip. Paint must not use Clip.
pub fn naive_tab_overflow(available: f32) -> OverflowMode {
    let labels = workspace_tab_labels();
    if would_clip(&labels, available) {
        OverflowMode::Clip
    } else {
        OverflowMode::Fit
    }
}

/// Shipped workspace strip: wrap instead of clipping so Bitstream stays on screen.
pub fn workspace_tab_overflow(available: f32) -> OverflowMode {
    match naive_tab_overflow(available) {
        OverflowMode::Clip => OverflowMode::Wrap,
        other => other,
    }
}

fn rail_mode(available: f32) -> OverflowMode {
    let labels = rail_action_labels();
    let total: f32 = labels.iter().map(|l| label_extent(l)).sum();
    if total <= available {
        OverflowMode::Fit
    } else {
        OverflowMode::Scroll
    }
}

fn bottom_mode(available: f32) -> OverflowMode {
    let labels = bottom_tab_labels();
    if would_clip(&labels, available) {
        OverflowMode::Scroll
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
    let tab_rows = wrap_labels(&workspace_tabs, workspace_available);
    let rail_rows = wrap_labels(&rail_actions, rail_available);
    ChromeOverflow {
        window_w,
        workspace_available,
        rail_available,
        naive_workspace_mode: naive_tab_overflow(workspace_available),
        workspace_mode: workspace_tab_overflow(workspace_available),
        rail_mode: rail_mode(rail_available),
        bottom_mode: bottom_mode(rail_available),
        tab_rows,
        rail_rows,
        workspace_tabs,
        rail_actions,
        bottom_tabs,
        nav_sections,
        table: table_scroll_policy(10, workspace_available),
    }
}
