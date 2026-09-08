//! CAD chrome: three canvases, one activity rail, More ⋯ overflow.
//!
//! Paint calls these helpers; tests assert the same units at ~1440×900 without eframe.

use crate::{BottomTab, WorkspaceTab};

/// Typical desktop inner size.
pub const DESKTOP_WIDTH: f32 = 1440.0;
pub const DESKTOP_HEIGHT: f32 = 900.0;
/// Activity rail — letter above the full name. 48px clipped "Device"/"Program"/"Reports".
pub const RAIL_WIDTH: f32 = 88.0;
/// Tall enough for letter glyph + full name under it (hit ≥28px).
pub const HIT_RAIL: f32 = 56.0;
/// One sidebar (MUST 5).
pub const SIDEBAR_WIDTH: f32 = 220.0;
/// Legacy aliases — side chrome is rail + sidebar, not 680.
pub const NAV_WIDTH: f32 = RAIL_WIDTH;
pub const TREE_WIDTH: f32 = SIDEBAR_WIDTH;
/// Properties dock — shown on selection, not always-on.
pub const PROPERTIES_WIDTH: f32 = 220.0;
pub const TOOLBAR_HEIGHT: f32 = 44.0;
pub const RAIL_MIN_HEIGHT: f32 = TOOLBAR_HEIGHT;
pub const STATUS_HEIGHT: f32 = 22.0;
pub const HIT_PRIMARY: f32 = 32.0;
/// Comfort primary (Implement / Open) — ≥44 where the toolbar has room.
pub const HIT_COMFORT: f32 = 36.0;
/// Every toolbar / flow chip shares this height. Width may follow the label; height may not.
pub const TOOLBAR_CTRL_H: f32 = HIT_COMFORT;
/// Approx glyph width at the rail's primary (word) size — letter is secondary.
const RAIL_WORD_CHAR_PX: f32 = 8.0;
pub const HIT_SIDEBAR: f32 = 28.0;
pub const HIT_SIDEBAR_ROW: f32 = HIT_SIDEBAR;
/// Occupancy / util bars (NICE leftover: was 12px).
pub const OCCUPANCY_BAR_H: f32 = 20.0;
/// Calm splitter grab radius (4–6px ship; not neon QA slab).
pub const SPLITTER_GRAB_PX: f32 = 6.0;
/// Sidebar may shrink/grow; the drag must be able to move ≥40px and keep the size.
pub const SIDEBAR_MIN_WIDTH: f32 = 160.0;
pub const SIDEBAR_MAX_WIDTH: f32 = 420.0;
pub const CONSOLE_MIN_HEIGHT: f32 = 80.0;
pub const CONSOLE_DEFAULT_HEIGHT: f32 = 180.0;
pub const CONSOLE_MAX_HEIGHT: f32 = 420.0;
/// Proof bar: a splitter that cannot travel this far is a dead grip.
pub const SPLITTER_MIN_DELTA_PX: f32 = 40.0;
/// Bounded height for in-pane Name/Value grids so they cannot eat the CentralPanel.
pub const TABLE_MAX_HEIGHT: f32 = 180.0;
/// Device/Package tables stack above the canvas; keep them compact so the die expands.
pub const DEVICE_TABLES_MAX_HEIGHT: f32 = 140.0;
/// Floorplan / package canvas never shrinks below this if the pane has room.
pub const DRAWING_MIN_HEIGHT: f32 = 280.0;
/// Idle paint policy (Air budgets): eframe reactive; no continuous `request_repaint`.
/// Floorplan/device paint is O(pins + tiles) per *input* frame only — not a synth loop.
pub const IDLE_PAINT_POLICY: &str = "reactive-no-request_repaint";
/// Soft budget: idle CPU should be ~0% when the window is unfocused / no input (OS compositor).
pub const IDLE_CPU_SOFT_PCT: u32 = 1;
/// Soft budget: no uncapped animation timer; paint only on egui events.
pub const IDLE_ANIM_HZ: u32 = 0;

/// Legacy alias — canvas expands; do not use as a max cap.
pub const DRAWING_MAX_HEIGHT: f32 = DRAWING_MIN_HEIGHT;
/// Minimum column width used to decide whether a grid clips its last column.
pub const MIN_COL_PX: f32 = 80.0;
pub const MORE: &str = "More ⋯";
pub const MORE_LABEL: &str = MORE;
const CHAR_PX: f32 = RAIL_WORD_CHAR_PX;
const TAB_PAD_PX: f32 = 16.0;

/// Toolbar control size: one height, width from the label (min 72, max 140).
pub fn toolbar_ctrl_size(label: &str) -> [f32; 2] {
    let w = (label.chars().count() as f32 * 8.0 + 28.0).clamp(72.0, 140.0);
    [w, TOOLBAR_CTRL_H]
}

/// Synth/Opt/Place/Route chips — same height as Open/Implement.
pub fn flow_chip_size() -> [f32; 2] {
    [72.0, TOOLBAR_CTRL_H]
}

/// Waveform trace row height. One or two traces must fill the remaining pane
/// instead of painting a 32px strip over a black slab.
pub fn wave_trace_row_h(n_traces: usize, remaining_h: f32) -> f32 {
    let n = n_traces.max(1) as f32;
    let remain = remaining_h.max(36.0);
    (remain / n).clamp(36.0, remain)
}

/// Stretch a table across the remaining pane (Win32: leftover empty regions are incorrect).
/// Subtracts `gap` between columns so the last header is not clipped.
pub fn stretched_col_w(n_cols: usize, avail: f32) -> f32 {
    stretched_col_w_gap(n_cols, avail, 12.0)
}

pub fn stretched_col_w_gap(n_cols: usize, avail: f32, gap: f32) -> f32 {
    let n = n_cols.max(1) as f32;
    let gaps = gap * (n - 1.0).max(0.0);
    ((avail - gaps).max(48.0) / n).max(48.0)
}

/// Vivado Autohide Pins / Quartus hide instance pins: unconnected pins stay
/// in the model but are not painted unless the symbol is selected.
pub fn schematic_pin_visible(net_empty: bool, symbol_selected: bool) -> bool {
    !net_empty || symbol_selected
}

/// Compact Name/Value stack (Settings 7 rows, Summary gadgets) before stretch.
pub fn nv_table_compact_h(n_rows: usize) -> f32 {
    n_rows.max(1) as f32 * 22.0 + 28.0
}

/// Settings/Summary landing: table occupies the remaining pane, not a postage stamp.
pub fn nv_table_bbox(n_rows: usize, pane_w: f32, pane_h: f32) -> DrawingFit {
    let compact_h = nv_table_compact_h(n_rows);
    fill_pane(pane_w.max(1.0), compact_h, pane_w, pane_h)
}

/// Occupancy / power-share bars were 120–160px strips with a >80px right gap.
pub fn occupancy_bar_w(avail: f32) -> f32 {
    avail.max(80.0)
}

/// Compact label columns left of occupancy bars (Resource/Used/Available/Pct).
/// A 280px reserve for 4 cols left a right inset vs Hierarchical.
pub fn occupancy_label_reserve(n_cols: usize) -> f32 {
    n_cols.max(1) as f32 * 50.0 + 12.0
}

/// Occupancy bar height. A 20px strip in remaining pane is a >80px void.
pub fn occupancy_bar_h(n_rows: usize, remaining_h: f32) -> f32 {
    let n = n_rows.max(1) as f32;
    let remain = remaining_h.max(OCCUPANCY_BAR_H);
    let gaps = 4.0 * n;
    ((remain - gaps) / n).clamp(OCCUPANCY_BAR_H, remain)
}

/// Occupancy bars share leftover height after a compact Hierarchical footer.
/// Counting footer rows as occupancy bars leaves a >80px hole under Hierarchical.
pub fn occupancy_bars_h_after_footer(n_bars: usize, footer_rows: usize, remaining_h: f32) -> f32 {
    let footer = if footer_rows == 0 {
        0.0
    } else {
        occupancy_table_compact_h(footer_rows)
    };
    occupancy_bar_h(n_bars.max(1), (remaining_h - footer).max(OCCUPANCY_BAR_H))
}

/// Compact occupancy / utilization / power-share stack before remaining-pane stretch.
pub fn occupancy_table_compact_h(n_rows: usize) -> f32 {
    n_rows.max(1) as f32 * (OCCUPANCY_BAR_H + 4.0) + 28.0
}

/// Occupancy landing: table occupies remaining pane, not a postage stamp over a void.
pub fn occupancy_table_bbox(n_rows: usize, pane_w: f32, pane_h: f32) -> DrawingFit {
    let compact_h = occupancy_table_compact_h(n_rows);
    fill_pane(pane_w.max(1.0), compact_h, pane_w, pane_h)
}

/// Empty Find/Bitstream/Settings canvases: the CTA occupies remaining pane height
/// (Win32 unused-region fail; Apple empty state with a next action).
pub fn empty_cta_occupies_pane(chrome_h: f32, pane_h: f32) -> DrawingFit {
    let ph = pane_h.max(1.0);
    let remain = (ph - chrome_h).max(ph * PANE_FILL_MIN);
    DrawingFit::from_drawn(1.0, ph, 1.0, 1.0, 1.0, remain)
}

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

    /// Name under the letter. Full words — the rail is wide enough to paint them.
    pub fn short_label(self) -> &'static str {
        self.label()
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

    /// Rail is click-only. ⌘1/⌘2/⌘3 belong to the three canvases.
    pub fn shortcut(self) -> &'static str {
        ""
    }

    pub fn hover(self) -> &'static str {
        match self {
            Activity::Files => "Files\nSources",
            Activity::Device => "Device\nFloorplan and I/O",
            Activity::Timing => "Timing\nSlack and paths",
            Activity::Simulate => "Simulate\nScopes and wave",
            Activity::Program => "Program\nCable and bitstream",
            Activity::Reports => "Reports\nTiming and utilization",
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

/// Scale HAD die / package cells for the Device canvas.
///
/// Fit both axes when that still clears the die-fill bar (≥80% of `avail_w`).
/// Otherwise prefer **width** so the right gap stays ≤80px on large Mac windows
/// (cell was previously capped at 24px → postage-stamp letterboxing). Vertical
/// overflow is acceptable; the parent canvas/scroll can absorb it. Max cell 64.
pub fn floorplan_fit_cell(cols: u32, rows: u32, avail_w: f32, avail_h: f32) -> f32 {
    let cols_f = cols.max(1) as f32;
    let rows_f = rows.max(1) as f32;
    let cw = (avail_w - 28.0).max(8.0) / cols_f;
    let ch = (avail_h - 16.0).max(8.0) / rows_f;
    let fit_both = cw.min(ch);
    let die_w_both = fit_both * cols_f + 28.0;
    let cell = if die_w_both < 0.80 * avail_w.max(1.0) {
        cw
    } else {
        fit_both
    };
    cell.clamp(4.0, 64.0)
}

/// Die drawn width (axis gutter included) for a chosen cell size.
pub fn floorplan_die_width(cols: u32, cell: f32) -> f32 {
    cell * cols.max(1) as f32 + 28.0
}

/// Horizontal fill ratio of the die inside `avail_w` (1.0 = full width).
pub fn floorplan_die_fill_ratio(cols: u32, cell: f32, avail_w: f32) -> f32 {
    let w = avail_w.max(1.0);
    (floorplan_die_width(cols, cell) / w).clamp(0.0, 1.0)
}

/// Empty pixels to the right of the die inside `avail_w` (left-aligned paint).
pub fn floorplan_right_gap_px(cols: u32, cell: f32, avail_w: f32) -> f32 {
    (avail_w - floorplan_die_width(cols, cell)).max(0.0)
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

/// Remaining-pane fill bar (Program / Package / Schematic / Hierarchy / IP drawings).
pub const PANE_FILL_MIN: f32 = 0.80;
pub const PANE_EMPTY_GAP_MAX: f32 = 80.0;
/// Compact IP catalog strip so the BD canvas can still hit the fill bar.
pub const IP_CATALOG_MAX_HEIGHT: f32 = 88.0;

/// Chip width so catalog names like `h_rv32_hb1` are not clipped in the strip.
pub fn ip_catalog_chip_w(name: &str) -> f32 {
    (name.chars().count() as f32 * CHAR_PX + 20.0).clamp(96.0, 240.0)
}

pub fn ip_catalog_name_fits(name: &str) -> bool {
    let w = ip_catalog_chip_w(name);
    w + 0.5 >= name.chars().count() as f32 * CHAR_PX + 16.0
}

/// How a content bbox sits in the remaining central pane after chrome/tables.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawingFit {
    pub pane_w: f32,
    pub pane_h: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub drawn_w: f32,
    pub drawn_h: f32,
    pub fill: f32,
    pub empty_gap: f32,
    pub right_clip: f32,
}

impl DrawingFit {
    fn from_drawn(pane_w: f32, pane_h: f32, scale_x: f32, scale_y: f32, drawn_w: f32, drawn_h: f32) -> Self {
        let pw = pane_w.max(1.0);
        let ph = pane_h.max(1.0);
        Self {
            pane_w: pw,
            pane_h: ph,
            scale_x,
            scale_y,
            drawn_w,
            drawn_h,
            fill: (drawn_w / pw).min(drawn_h / ph).clamp(0.0, 1.0),
            empty_gap: (pw - drawn_w).max(ph - drawn_h).max(0.0),
            right_clip: (drawn_w - pw).max(0.0),
        }
    }

    pub fn fills(&self) -> bool {
        self.fill + 0.000_5 >= PANE_FILL_MIN && self.empty_gap <= PANE_EMPTY_GAP_MAX && self.right_clip <= 0.5
    }
}

/// Stretch content so the drawn bbox fills the pane (package pins, hardware dashboard).
pub fn fill_pane(content_w: f32, content_h: f32, pane_w: f32, pane_h: f32) -> DrawingFit {
    let cw = content_w.max(1.0);
    let ch = content_h.max(1.0);
    let pw = pane_w.max(1.0);
    let ph = pane_h.max(1.0);
    let sx = pw / cw;
    let sy = ph / ch;
    DrawingFit::from_drawn(pw, ph, sx, sy, pw, ph)
}

/// Isotropic fit: content stays inside the pane (schematic sheet). Right clip is 0.
pub fn fit_pane(content_w: f32, content_h: f32, pane_w: f32, pane_h: f32) -> DrawingFit {
    let cw = content_w.max(1.0);
    let ch = content_h.max(1.0);
    let pw = pane_w.max(1.0);
    let ph = pane_h.max(1.0);
    let scale = (pw / cw).min(ph / ch).clamp(0.05, 16.0);
    let dw = cw * scale;
    let dh = ch * scale;
    DrawingFit::from_drawn(pw, ph, scale, scale, dw, dh)
}

/// Old Package paint: square cell + DRAWING_MIN_HEIGHT pad. Leaves a black slab
/// when the package is a 1-row pin strip.
pub fn legacy_package_content_fill(cols: u32, rows: u32, pane_w: f32, pane_h: f32) -> f32 {
    let cell = floorplan_fit_cell(cols, rows, pane_w, pane_h);
    let content_h = cell * rows.max(1) as f32 + 16.0;
    content_h / pane_h.max(1.0)
}

/// Anisotropic pin cells so the package grid fills remaining pane width and height.
pub fn package_cell(cols: u32, rows: u32, pane_w: f32, pane_h: f32) -> (f32, f32) {
    let cw = (pane_w - 28.0).max(8.0) / cols.max(1) as f32;
    let ch = (pane_h - 16.0).max(8.0) / rows.max(1) as f32;
    (cw.max(4.0), ch.max(4.0))
}

/// Remaining dashboard after Hardware Manager chrome (buttons/status).
pub fn hardware_dashboard_size(pane_w: f32, pane_h: f32, chrome_h: f32) -> (f32, f32) {
    let remain = (pane_h - chrome_h).max(pane_h * PANE_FILL_MIN);
    (pane_w.max(80.0), remain)
}

/// STAT/ILA tiles that fill the Program pane (content bbox, not a dark empty slab).
pub fn stat_bit_grid(n_bits: usize, pane_w: f32, pane_h: f32) -> (u32, u32, f32, f32) {
    let n = n_bits.max(1) as u32;
    let aspect = (pane_w / pane_h.max(1.0)).clamp(0.25, 8.0);
    let cols = ((n as f32 * aspect).sqrt().ceil() as u32).max(1);
    let rows = n.div_ceil(cols).max(1);
    let (cw, ch) = package_cell(cols, rows, pane_w, pane_h);
    (cols, rows, cw, ch)
}

/// Compact CAD cards (Windows compact / Apple content-first): tile strip + ILA rest.
/// Tile height is capped so 3 empty-state cables are not fullscreen candy slabs.
pub fn program_layout(n_tiles: usize, pane_w: f32, pane_h: f32) -> (f32, f32, u32, u32, f32, f32) {
    let ph = pane_h.max(1.0);
    let tile_band = (ph * 0.42).clamp(88.0, 132.0);
    let ila_h = (ph - tile_band).max(ph * 0.40);
    let (cols, rows, cw, ch) = stat_bit_grid(n_tiles.max(1), pane_w.max(1.0), tile_band);
    (tile_band, ila_h, cols, rows, cw, ch.min(tile_band))
}

/// Content bbox of the Program STAT grid + ILA rest (both are content).
pub fn hardware_content_bbox(n_bits: usize, pane_w: f32, pane_h: f32) -> DrawingFit {
    let (tile_band, ila_h, cols, rows, cw, ch) = program_layout(n_bits, pane_w, pane_h);
    let drawn_w = (cw * cols as f32 + 16.0).min(pane_w.max(1.0));
    let drawn_h = tile_band + ila_h;
    let _ = (rows, ch);
    DrawingFit::from_drawn(pane_w.max(1.0), pane_h.max(1.0), 1.0, 1.0, drawn_w, drawn_h)
}

/// Native Hierarchy/IP sheet as a fraction of the pane (postage stamp in the More shots).
pub fn native_drawing_fill(sheet_w: f32, sheet_h: f32, pane_w: f32, pane_h: f32) -> f32 {
    (sheet_w / pane_w.max(1.0)).min(sheet_h / pane_h.max(1.0)).clamp(0.0, 1.0)
}

/// Stretch a Hierarchy/IP sheet so the drawn bbox fills the remaining pane.
pub fn hierarchy_fit(sheet_w: f32, sheet_h: f32, pane_w: f32, pane_h: f32) -> DrawingFit {
    fill_pane(sheet_w, sheet_h, pane_w, pane_h)
}

/// Shot leftover: "Select a report in the sidebar" is a ~48px stub.
pub fn reports_stub_fill(pane_h: f32) -> f32 {
    48.0 / pane_h.max(1.0)
}

/// Compact catalog row stack (name/category/status/summary) before it is stretched.
pub fn reports_catalog_compact_h(n_rows: usize) -> f32 {
    n_rows.max(1) as f32 * 22.0 + 28.0
}

/// Reports landing: catalog table occupies the remaining pane, not Timing and not a stub.
pub fn reports_catalog_bbox(n_rows: usize, pane_w: f32, pane_h: f32) -> DrawingFit {
    let compact_h = reports_catalog_compact_h(n_rows);
    fill_pane(pane_w.max(1.0), compact_h, pane_w, pane_h)
}

/// IP Integrator: capped catalog strip + canvas occupying the rest of the pane.
pub fn ip_layout(pane_w: f32, pane_h: f32) -> (f32, DrawingFit) {
    let ph = pane_h.max(1.0);
    let catalog_h = IP_CATALOG_MAX_HEIGHT
        .min(ph * (1.0 - PANE_FILL_MIN))
        .min(ph * 0.22)
        .max(48.0);
    let canvas_h = (ph - catalog_h).max(ph * PANE_FILL_MIN);
    let catalog_h = (ph - canvas_h).max(0.0);
    (
        catalog_h,
        DrawingFit::from_drawn(pane_w.max(1.0), ph, 1.0, 1.0, pane_w.max(1.0), canvas_h),
    )
}

/// Grow a tiny identity sheet to fill the pane. Never shrink a large sheet:
/// the user pans and pinches. Zoom Fit is explicit.
pub fn schematic_should_auto_fit(
    zoom: f32,
    pan_x: f32,
    pan_y: f32,
    sheet_w: f32,
    sheet_h: f32,
    vw: f32,
    vh: f32,
) -> bool {
    let identity = (zoom - 1.0).abs() < 0.02 && pan_x.abs() < 0.5 && pan_y.abs() < 0.5;
    identity && sheet_w * zoom < vw * 0.80 && sheet_h * zoom < vh * 0.80
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

/// Horizontal pixels the rail name occupies (letter sits above; this is the word).
pub fn rail_name_extent(label: &str) -> f32 {
    label.chars().count() as f32 * CHAR_PX
}

/// True when `label` paints fully inside the rail (padding 8px). Hover is not a substitute.
pub fn rail_name_fits(label: &str) -> bool {
    rail_name_extent(label) + 8.0 <= RAIL_WIDTH
}

/// Central pane a `WorkspaceTab` must fill. More ⋯ destinations never collapse to Timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspacePane {
    Editor,
    Device,
    Timing,
    ReportsCatalog,
    Schematic,
    Package,
    Hierarchy,
    Bitstream,
    Hardware,
    Ip,
    Find,
    Settings,
    Summary,
    Wave,
    Source,
    Memory,
    Breakpoints,
    Locals,
    Forces,
    SimSettings,
    Constraints,
    ClockInteraction,
    Cdc,
    ClockNetworks,
    Power,
    Methodology,
    Drc,
    Utilization,
    Runs,
}

/// Map every WorkspaceTab onto a real pane. None of these is a stub / popup / Timing dump.
pub fn pane_for_workspace(tab: WorkspaceTab) -> WorkspacePane {
    match tab {
        WorkspaceTab::TextEditor => WorkspacePane::Editor,
        WorkspaceTab::Device => WorkspacePane::Device,
        WorkspaceTab::Reports => WorkspacePane::ReportsCatalog,
        WorkspaceTab::Schematic => WorkspacePane::Schematic,
        WorkspaceTab::Package => WorkspacePane::Package,
        WorkspaceTab::Hierarchy => WorkspacePane::Hierarchy,
        WorkspaceTab::Bitstream => WorkspacePane::Bitstream,
        WorkspaceTab::Hardware => WorkspacePane::Hardware,
        WorkspaceTab::Ip => WorkspacePane::Ip,
        WorkspaceTab::Find => WorkspacePane::Find,
        WorkspaceTab::Settings => WorkspacePane::Settings,
        WorkspaceTab::Summary => WorkspacePane::Summary,
        WorkspaceTab::Wave => WorkspacePane::Wave,
        WorkspaceTab::Source => WorkspacePane::Source,
        WorkspaceTab::Memory => WorkspacePane::Memory,
        WorkspaceTab::Breakpoints => WorkspacePane::Breakpoints,
        WorkspaceTab::Locals => WorkspacePane::Locals,
        WorkspaceTab::Forces => WorkspacePane::Forces,
        WorkspaceTab::SimSettings => WorkspacePane::SimSettings,
        WorkspaceTab::Constraints => WorkspacePane::Constraints,
        WorkspaceTab::ClockInteraction => WorkspacePane::ClockInteraction,
        WorkspaceTab::Cdc => WorkspacePane::Cdc,
        WorkspaceTab::ClockNetworks => WorkspacePane::ClockNetworks,
        WorkspaceTab::Power => WorkspacePane::Power,
        WorkspaceTab::Methodology => WorkspacePane::Methodology,
        WorkspaceTab::Drc => WorkspacePane::Drc,
        WorkspaceTab::Utilization => WorkspacePane::Utilization,
        WorkspaceTab::Runs => WorkspacePane::Runs,
    }
}

/// Non-canvas WorkspaceTabs live in More ⋯ and must paint `pane_for_workspace` in the center.
pub fn is_more_destination(tab: WorkspaceTab) -> bool {
    !tab.is_canvas()
}

/// Report-detail tabs that belong under the Reports rail (catalog stays in the sidebar).
pub fn is_report_detail(tab: WorkspaceTab) -> bool {
    matches!(
        tab,
        WorkspaceTab::Constraints
            | WorkspaceTab::ClockInteraction
            | WorkspaceTab::Cdc
            | WorkspaceTab::ClockNetworks
            | WorkspaceTab::Power
            | WorkspaceTab::Methodology
            | WorkspaceTab::Drc
            | WorkspaceTab::Utilization
            | WorkspaceTab::Runs
    )
}

/// Splitters must travel at least this far inside their min/max range.
pub fn splitter_can_travel(min: f32, max: f32) -> bool {
    max - min >= SPLITTER_MIN_DELTA_PX
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_overflow_keeps_every_tab_and_rail_action_selectable_at_desktop_width() {
        assert_eq!(side_chrome_width(), RAIL_WIDTH + SIDEBAR_WIDTH);
        assert_eq!(side_chrome_width(), 308.0); // RAIL 88 + SIDEBAR 220 (full names)
        assert_ne!(side_chrome_width(), 680.0);
        assert_eq!(RAIL_WIDTH, 88.0);
        assert_eq!(HIT_RAIL, 56.0);
        assert_eq!(SIDEBAR_WIDTH, 220.0);
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
        assert!(cell >= 4.0 && cell <= 64.0);
        // Width-first when letterboxing would miss the ≥80% die-fill bar.
        assert!(
            floorplan_die_fill_ratio(32, cell, 800.0) >= 0.80,
            "die fill must be ≥80% at 800px canvas, got {:.3}",
            floorplan_die_fill_ratio(32, cell, 800.0)
        );
        assert!(
            floorplan_right_gap_px(32, cell, 800.0) <= 80.0,
            "right gap must be ≤80px, got {:.1}",
            floorplan_right_gap_px(32, cell, 800.0)
        );
        // Large Mac-like Device pane (rail+sidebar already subtracted).
        let mac_w = 1152.0;
        let mac_h = 628.0;
        let mac_cell = floorplan_fit_cell(32, 33, mac_w, mac_h);
        assert!(
            floorplan_die_fill_ratio(32, mac_cell, mac_w) >= 0.80,
            "Mac die fill {:.3}",
            floorplan_die_fill_ratio(32, mac_cell, mac_w)
        );
        assert!(
            floorplan_right_gap_px(32, mac_cell, mac_w) <= 80.0,
            "Mac right gap {:.1}",
            floorplan_right_gap_px(32, mac_cell, mac_w)
        );
        assert!(DEVICE_TABLES_MAX_HEIGHT < DESKTOP_HEIGHT / 3.0);
        assert!(DRAWING_MIN_HEIGHT > TABLE_MAX_HEIGHT);
        assert!(workspace_matches_canvases());
    }

    #[test]
    fn activity_rail_letter_and_full_name_are_visible() {
        for a in Activity::ALL {
            assert_eq!(a.icon().chars().count(), 1, "{a:?} letter");
            assert!(!a.short_label().is_empty(), "{a:?} name");
            assert_eq!(a.short_label(), a.label(), "{a:?} must paint the full name");
            assert!(
                rail_name_fits(a.label()),
                "{a:?} name {:?} clips on {RAIL_WIDTH}px rail (extent {})",
                a.label(),
                rail_name_extent(a.label())
            );
            assert!(
                rail_name_fits(a.short_label()),
                "{a:?} short {:?} clips",
                a.short_label()
            );
        }
        assert_eq!(Activity::Simulate.short_label(), "Simulate");
        assert_eq!(Activity::Program.short_label(), "Program");
        assert_eq!(Activity::Device.short_label(), "Device");
        assert_eq!(Activity::Reports.short_label(), "Reports");
        assert_eq!(RAIL_WIDTH, 88.0);
        assert_eq!(HIT_RAIL, 56.0);
        assert!(HIT_RAIL >= 28.0);
        assert_eq!(HIT_COMFORT, 36.0);
        assert_eq!(TOOLBAR_CTRL_H, HIT_COMFORT);
        assert_eq!(flow_chip_size()[1], HIT_COMFORT);
        for label in ["Open…", "Bitstream", "Implement", "Implementing…"] {
            let s = toolbar_ctrl_size(label);
            assert_eq!(s[1], HIT_COMFORT, "{label} height");
            assert!(s[0] >= 72.0, "{label} width {}", s[0]);
        }
        assert_eq!(OCCUPANCY_BAR_H, 20.0);
        assert!((4.0..=6.0).contains(&SPLITTER_GRAB_PX));
        assert_eq!(DEVICE_TABLES_MAX_HEIGHT, 140.0);
        assert!(splitter_can_travel(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH));
        assert!(splitter_can_travel(CONSOLE_MIN_HEIGHT, CONSOLE_MAX_HEIGHT));
        assert!(SIDEBAR_WIDTH >= SIDEBAR_MIN_WIDTH && SIDEBAR_WIDTH <= SIDEBAR_MAX_WIDTH);

        let desktop = chrome_at(DESKTOP_WIDTH);
        assert_eq!(desktop.workspace_mode, OverflowMode::Fit);
        for lab in ["Editor", "Device", "Timing"] {
            assert!(desktop.tab_is_selectable(lab), "desktop {lab}");
        }
        let narrow = chrome_at(1100.0);
        assert_eq!(narrow.dropped_workspace(), 0);
        assert!(
            narrow.workspace_mode == OverflowMode::Fit || narrow.tab_rows.iter().any(|r| r.contains(&MORE)),
            "narrow window keeps canvases or More: {:?}",
            narrow.tab_rows
        );
        for lab in ["Editor", "Device", "Timing"] {
            assert!(narrow.tab_is_selectable(lab), "narrow {lab}");
        }
        let table = table_scroll_policy(10, 400.0);
        assert!(table.last_column_would_clip && table.x && table.y);
    }

    #[test]
    fn toolbar_and_wave_geometry_are_uniform_and_fill() {
        assert_eq!(TOOLBAR_CTRL_H, 36.0);
        assert_eq!(flow_chip_size(), [72.0, 36.0]);
        assert_eq!(toolbar_ctrl_size("Open…")[1], toolbar_ctrl_size("Implement")[1]);
        // One trace in a 400px remaining pane must not be a 32px strip.
        let h1 = wave_trace_row_h(1, 400.0);
        assert!(
            h1 >= 400.0 * PANE_FILL_MIN,
            "single wave trace row {h1} leaves a black slab in 400px"
        );
        let h2 = wave_trace_row_h(2, 400.0);
        assert!(h2 >= 180.0, "two traces share the pane, got {h2}");
        assert_eq!(wave_trace_row_h(20, 400.0), 36.0);
        let w = stretched_col_w(7, 700.0);
        assert!(w >= 48.0, "{w}");
        assert!(
            w * 7.0 + 12.0 * 6.0 <= 700.0 + 1.0,
            "stretched columns plus gaps must fit the pane ({w})"
        );
        assert!(!schematic_pin_visible(true, false), "n/c hidden unless selected");
        assert!(schematic_pin_visible(true, true), "n/c shown on selected cell");
        assert!(schematic_pin_visible(false, false), "connected pins always shown");
        assert!(ip_catalog_name_fits("h_rv32_hb1"));
        assert!(ip_catalog_name_fits("h_uart"));
        assert!(ip_catalog_chip_w("h_rv32_hb1") >= 96.0);
        // Program bbox must not be padded up to 80% by the helper itself.
        let hw = hardware_content_bbox(8, 1000.0, 500.0);
        assert!(
            hw.drawn_w < 1000.0 * PANE_FILL_MIN || hw.fill >= PANE_FILL_MIN,
            "bbox must measure tiles, not clamp to 80%: {hw:?}"
        );
        assert!(hw.fills() || hw.fill >= PANE_FILL_MIN, "program tiles {hw:?}");
        let empty = empty_cta_occupies_pane(80.0, 400.0);
        assert!(
            empty.fills() || empty.fill >= PANE_FILL_MIN,
            "empty CTA must occupy remaining pane, got {empty:?}"
        );
        assert!(empty.drawn_h >= 400.0 * PANE_FILL_MIN);
    }

    #[test]
    fn settings_and_summary_tables_fill_remaining_pane() {
        let pane_w = 1000.0;
        let pane_h = 400.0;
        let compact = nv_table_compact_h(7);
        assert!(
            compact / pane_h < PANE_FILL_MIN,
            "unstretched 7-row Name/Value {compact} must fail so stretch is required"
        );
        let bbox = nv_table_bbox(7, pane_w, pane_h);
        assert!(
            bbox.fills() || bbox.fill >= PANE_FILL_MIN,
            "Settings/Summary table must fill the pane, got {bbox:?}"
        );
        assert!(bbox.empty_gap <= PANE_EMPTY_GAP_MAX);
        let remain = pane_w - 180.0;
        assert!(
            120.0 < remain * PANE_FILL_MIN,
            "legacy 120px power share bar must fail the fill bar"
        );
        assert!(
            160.0 < remain * PANE_FILL_MIN,
            "legacy 160px occupancy bar must fail the fill bar"
        );
        let bar = occupancy_bar_w(remain);
        assert!(
            bar > 160.0,
            "legacy 160px occupancy bar must fail the fill bar, got {bar}"
        );
        assert!(
            bar >= remain * PANE_FILL_MIN,
            "occupancy/share bars must span remaining width, got {bar}"
        );
        assert!(
            remain - bar <= PANE_EMPTY_GAP_MAX,
            "right gap after occupancy/share bar {bar} in {remain}"
        );
        let shrunk = bar * 0.25;
        assert!(
            remain - shrunk > PANE_EMPTY_GAP_MAX,
            "0.25× occupancy bars reintroduce a >80px void (shrunk={shrunk})"
        );
        // Power Utilization Details was Block/Used/Available (~180px) with no bar.
        let details_labels = 180.0;
        assert!(
            details_labels < pane_w * PANE_FILL_MIN,
            "Utilization Details without occupancy bars must fail the fill bar"
        );
        let details_bar = occupancy_bar_w(pane_w - details_labels);
        assert!(
            details_bar >= (pane_w - details_labels) * PANE_FILL_MIN,
            "Utilization Details occupancy bars must span remaining width, got {details_bar}"
        );
        assert!(
            pane_w - details_labels - details_bar <= PANE_EMPTY_GAP_MAX,
            "right gap after Utilization Details occupancy bar"
        );
        let compact_occ = occupancy_table_compact_h(5);
        assert!(
            compact_occ / pane_h < PANE_FILL_MIN,
            "unstretched 5-row occupancy table {compact_occ} must fail so stretch is required"
        );
        let occ = occupancy_table_bbox(5, pane_w, pane_h);
        assert!(
            occ.fills() || occ.fill >= PANE_FILL_MIN,
            "occupancy/utilization table must fill remaining pane, got {occ:?}"
        );
        assert!(occ.empty_gap <= PANE_EMPTY_GAP_MAX);
        // Utilization Hierarchical was 6 compact columns with a >80px right gap.
        let compact_hier = 6.0 * 64.0;
        assert!(
            compact_hier < pane_w * PANE_FILL_MIN,
            "compact Hierarchical columns must fail the fill bar"
        );
        let hier = stretched_col_w_gap(6, pane_w, 8.0);
        let hier_span = hier * 6.0 + 8.0 * 5.0;
        assert!(
            hier_span >= pane_w * PANE_FILL_MIN,
            "Hierarchical columns must span remaining width, span={hier_span}"
        );
        assert!(pane_w - hier_span <= PANE_EMPTY_GAP_MAX);
        let compact_bars = OCCUPANCY_BAR_H * 5.0;
        assert!(
            compact_bars / pane_h < PANE_FILL_MIN,
            "20px occupancy bars must fail so stretch is required"
        );
        let bh = occupancy_bar_h(5, pane_h);
        assert!(
            bh * 5.0 >= pane_h * PANE_FILL_MIN,
            "occupancy bar height {bh} leaves a void in {pane_h}"
        );
        let n_bars = 4usize;
        let footer = 2usize;
        let wrong = occupancy_bar_h(n_bars + 1 + footer, pane_h);
        let wrong_drawn = wrong * n_bars as f32 + occupancy_table_compact_h(footer);
        assert!(
            pane_h - wrong_drawn > PANE_EMPTY_GAP_MAX,
            "counting Hierarchical rows as occupancy bars must leave a void ({wrong_drawn})"
        );
        let bh_footer = occupancy_bars_h_after_footer(n_bars, footer, pane_h);
        let drawn = bh_footer * n_bars as f32 + occupancy_table_compact_h(footer);
        assert!(
            drawn >= pane_h * PANE_FILL_MIN || pane_h - drawn <= PANE_EMPTY_GAP_MAX,
            "occupancy bars after Hierarchical footer {drawn} in {pane_h}"
        );
        let labels4 = occupancy_label_reserve(4);
        assert!(
            280.0 - labels4 > 40.0,
            "legacy 280px occupancy label reserve over-subtracts ({labels4})"
        );
        let tight = occupancy_bar_w(pane_w - labels4);
        let legacy = occupancy_bar_w(pane_w - 280.0);
        assert!(tight > legacy, "tight occupancy bars {tight} vs legacy {legacy}");
        assert!(
            pane_w - labels4 - tight <= PANE_EMPTY_GAP_MAX,
            "occupancy bars must meet Hierarchical right edge"
        );
    }

    #[test]
    fn more_destinations_never_fall_through_to_timing_pane() {
        use crate::WorkspaceTab;
        assert_eq!(WorkspaceTab::ALL.len(), 28);
        assert_eq!(pane_for_workspace(WorkspaceTab::Schematic), WorkspacePane::Schematic);
        assert_ne!(pane_for_workspace(WorkspaceTab::Schematic), WorkspacePane::Timing);
        assert_eq!(pane_for_workspace(WorkspaceTab::Package), WorkspacePane::Package);
        assert_eq!(pane_for_workspace(WorkspaceTab::Hierarchy), WorkspacePane::Hierarchy);
        assert_eq!(pane_for_workspace(WorkspaceTab::Summary), WorkspacePane::Summary);
        assert_eq!(pane_for_workspace(WorkspaceTab::Settings), WorkspacePane::Settings);
        assert_eq!(pane_for_workspace(WorkspaceTab::Bitstream), WorkspacePane::Bitstream);
        assert_eq!(pane_for_workspace(WorkspaceTab::Hardware), WorkspacePane::Hardware);
        assert_eq!(pane_for_workspace(WorkspaceTab::Ip), WorkspacePane::Ip);
        assert_eq!(pane_for_workspace(WorkspaceTab::Find), WorkspacePane::Find);
        let mut more = 0usize;
        for tab in WorkspaceTab::ALL {
            let pane = pane_for_workspace(tab);
            if is_more_destination(tab) {
                more += 1;
                assert_ne!(
                    pane,
                    WorkspacePane::Timing,
                    "{tab:?} More destination must not paint Timing"
                );
                assert_ne!(
                    pane,
                    WorkspacePane::ReportsCatalog,
                    "{tab:?} More destination is not the Reports catalog"
                );
            }
        }
        assert_eq!(more, 25, "28 tabs − 3 canvases");
        assert!(is_report_detail(WorkspaceTab::Utilization));
        assert!(!is_report_detail(WorkspaceTab::Schematic));
        assert_eq!(
            pane_for_workspace(WorkspaceTab::Reports),
            WorkspacePane::ReportsCatalog
        );
        assert_eq!(pane_for_workspace(WorkspaceTab::TextEditor), WorkspacePane::Editor);
        assert_eq!(pane_for_workspace(WorkspaceTab::Device), WorkspacePane::Device);
    }

    #[test]
    fn reports_landing_catalog_fills_pane_and_is_not_timing() {
        let pane_w = 1000.0;
        let pane_h = 400.0;
        assert_eq!(
            pane_for_workspace(WorkspaceTab::Reports),
            WorkspacePane::ReportsCatalog
        );
        assert_ne!(
            pane_for_workspace(WorkspaceTab::Reports),
            WorkspacePane::Timing
        );
        assert!(
            reports_stub_fill(pane_h) < PANE_FILL_MIN,
            "Select-a-report stub must fail the fill bar"
        );
        let compact = reports_catalog_compact_h(8);
        assert!(
            compact / pane_h < PANE_FILL_MIN,
            "unstretched 8-row catalog {compact} must fail so stretch is required"
        );
        let bbox = reports_catalog_bbox(8, pane_w, pane_h);
        assert!(
            bbox.fills() || bbox.fill >= PANE_FILL_MIN,
            "Reports catalog must fill the landing pane, got {bbox:?}"
        );
        assert!(bbox.empty_gap <= PANE_EMPTY_GAP_MAX);
        assert!(bbox.right_clip <= 0.5);
    }

    #[test]
    fn package_and_hardware_fill_remaining_pane_not_a_black_slab() {
        let pane_w = 1000.0;
        let pane_h = 400.0;
        // The 1-row pin strip + DRAWING_MIN_HEIGHT pad is the shot black-hole.
        assert!(
            legacy_package_content_fill(32, 1, pane_w, pane_h) < PANE_FILL_MIN,
            "legacy letterbox must fail the fill bar so the new cell path is required"
        );
        let (cw, ch) = package_cell(32, 1, pane_w, pane_h);
        let drawn_w = cw * 32.0 + 28.0;
        let drawn_h = ch * 1.0 + 16.0;
        let fill = (drawn_w / pane_w).min(drawn_h / pane_h);
        assert!(fill >= PANE_FILL_MIN, "package fill {fill}");
        assert!((pane_w - drawn_w).max(pane_h - drawn_h) <= PANE_EMPTY_GAP_MAX);
        let filled = fill_pane(32.0, 1.0, pane_w, pane_h);
        assert!(filled.fills(), "{filled:?}");
        let hw = hardware_content_bbox(3, pane_w, pane_h);
        assert!(hw.fills(), "Program cards+ILA content bbox {hw:?}");
        let (tile_band, ila_h, _, _, _, ch) = program_layout(3, pane_w, pane_h);
        assert!(
            ch <= 132.0 && tile_band <= 132.0,
            "empty-state cable cards must stay compact, ch={ch} band={tile_band}"
        );
        assert!(ila_h >= pane_h * 0.40, "ILA rest is content, not a void");
        assert!(
            !schematic_should_auto_fit(2.0, 0.0, 0.0, 1400.0, 720.0, 900.0, 500.0),
            "user zoom-in must not be auto-fitted"
        );
        assert!(
            !schematic_should_auto_fit(1.0, 0.0, 0.0, 1400.0, 720.0, 900.0, 500.0),
            "a large sheet is panned/zoomed, not auto-shrunk"
        );
        assert!(
            schematic_should_auto_fit(1.0, 0.0, 0.0, 200.0, 150.0, 900.0, 500.0),
            "a tiny sheet may grow to fill empty pane"
        );
    }

    #[test]
    fn hierarchy_and_ip_canvas_fill_remaining_pane_not_postage_stamps() {
        let pane_w = 1000.0;
        let pane_h = 400.0;
        // Counter hierarchy sheet from the More Hierarchy shot: 316×258.
        let native = native_drawing_fill(316.0, 258.0, pane_w, pane_h);
        assert!(
            native < PANE_FILL_MIN,
            "native hierarchy postage stamp must fail the fill bar (got {native})"
        );
        let fit = hierarchy_fit(316.0, 258.0, pane_w, pane_h);
        assert!(
            fit.fills(),
            "hierarchy must stretch into the remaining pane, got {fit:?}"
        );
        assert!(fit.right_clip <= 0.5);
        let (catalog_h, canvas) = ip_layout(pane_w, pane_h);
        assert!(
            catalog_h <= IP_CATALOG_MAX_HEIGHT,
            "IP catalog strip {catalog_h} must stay compact"
        );
        assert!(
            canvas.fills() || canvas.fill >= PANE_FILL_MIN,
            "IP BD canvas {canvas:?}"
        );
        assert!(canvas.empty_gap <= PANE_EMPTY_GAP_MAX);
    }

    #[test]
    fn schematic_fit_does_not_clip_rightmost_iob() {
        // Counter sheet is wider than a 900px pane at zoom=1.
        let sheet_w = 1400.0;
        let sheet_h = 720.0;
        let pane_w = 900.0;
        let pane_h = 500.0;
        let raw_clip = sheet_w - pane_w;
        assert!(raw_clip > 80.0, "unfitted sheet must clip (got {raw_clip})");
        let fit = fit_pane(sheet_w, sheet_h, pane_w, pane_h);
        assert!(fit.right_clip <= 0.5, "right clip {}", fit.right_clip);
        assert!(fit.drawn_w <= pane_w + 0.5);
        assert!(fit.drawn_h <= pane_h + 0.5);
        assert!(fit.fill >= PANE_FILL_MIN, "fit fill {}", fit.fill);
    }
}

#[cfg(test)]
mod idle_budget_tests {
    #[test]
    fn idle_policy_is_reactive_no_continuous_anim() {
        assert_eq!(super::IDLE_PAINT_POLICY, "reactive-no-request_repaint");
        assert_eq!(super::IDLE_ANIM_HZ, 0);
        assert!(super::IDLE_CPU_SOFT_PCT <= 5);
    }
}
