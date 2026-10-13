//! CAD chrome: three canvases, one activity rail, More ... overflow.
//!
//! Paint calls these helpers; tests assert the same units at ~1440×900 without eframe.

use eframe::egui;

use crate::{ide::WaveDensity, BottomTab, WorkspaceTab};

/// Typical desktop inner size.
pub const DESKTOP_WIDTH: f32 = 1440.0;
pub const DESKTOP_HEIGHT: f32 = 900.0;
/// Minimum window inner size, used by `run_gui` for `with_min_inner_size`.
/// VS Code sets no hard window minimum either; its parts declare minimums
/// instead (activity bar 48px, sidebar 170px, panel collapses by priority) and
/// the grid never overlaps. 720x400 is Helion's analog: at 720 the chrome
/// budget is rail 88 + sidebar floor 128, leaving the central pane 604px wide;
/// at 400 the console clamps to its 36px floor and the timing canvas scrolls.
/// Every surface has a tested reflow path at this size (see the resize tests).
pub const MIN_WINDOW_W: f32 = 720.0;
pub const MIN_WINDOW_H: f32 = 400.0;
/// Activity rail - letter above the full name. 48px clipped "Device"/"Program"/"Reports".
pub const RAIL_WIDTH: f32 = 88.0;
/// Tall enough for letter glyph + full name under it (hit ≥28px).
pub const HIT_RAIL: f32 = 56.0;
/// One sidebar (MUST 5).
pub const SIDEBAR_WIDTH: f32 = 220.0;
/// Legacy aliases - side chrome is rail + sidebar, not 680.
pub const NAV_WIDTH: f32 = RAIL_WIDTH;
pub const TREE_WIDTH: f32 = SIDEBAR_WIDTH;
/// Properties dock - shown on selection, not always-on.
pub const PROPERTIES_WIDTH: f32 = 220.0;
pub const TOOLBAR_HEIGHT: f32 = 44.0;
pub const RAIL_MIN_HEIGHT: f32 = TOOLBAR_HEIGHT;
pub const STATUS_HEIGHT: f32 = 22.0;
pub const HIT_PRIMARY: f32 = 32.0;
/// Comfort primary (Implement / Open) - ≥44 where the toolbar has room.
pub const HIT_COMFORT: f32 = 36.0;
/// Every toolbar / flow chip shares this height. Width may follow the label; height may not.
pub const TOOLBAR_CTRL_H: f32 = HIT_COMFORT;
/// Approx glyph width at the rail's primary (word) size - letter is secondary.
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
/// Floorplan/device paint is O(pins + tiles) per *input* frame only - not a synth loop.
pub const IDLE_PAINT_POLICY: &str = "reactive-no-request_repaint";
/// Soft budget: idle CPU should be ~0% when the window is unfocused / no input (OS compositor).
pub const IDLE_CPU_SOFT_PCT: u32 = 1;
/// Soft budget: no uncapped animation timer; paint only on egui events.
pub const IDLE_ANIM_HZ: u32 = 0;

/// Legacy alias - canvas expands; do not use as a max cap.
pub const DRAWING_MAX_HEIGHT: f32 = DRAWING_MIN_HEIGHT;
/// Minimum column width used to decide whether a grid clips its last column.
pub const MIN_COL_PX: f32 = 80.0;
pub const MORE: &str = "More ...";
pub const MORE_LABEL: &str = MORE;
const CHAR_PX: f32 = RAIL_WORD_CHAR_PX;
const TAB_PAD_PX: f32 = 16.0;

/// Toolbar control size: one height, width from the label (min 72, max 140).
pub fn toolbar_ctrl_size(label: &str) -> [f32; 2] {
    let w = (label.chars().count() as f32 * 8.0 + 28.0).clamp(72.0, 140.0);
    [w, TOOLBAR_CTRL_H]
}

/// Synth/Opt/Place/Route chips - same height as Open/Implement.
pub fn flow_chip_size() -> [f32; 2] {
    [72.0, TOOLBAR_CTRL_H]
}

/// Waveform trace row height. One or two traces must fill the remaining pane
/// instead of painting a 32px strip over a black slab.
/// Waveform lane height comes from the density toggle (GTKWave-style),
/// not from stretching rows to fill the pane - stretching produced 150px
/// lanes with few traces. Leftover space below the lanes is normal.
pub fn wave_trace_row_h(density: WaveDensity) -> f32 {
    density.lane_h()
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

/// Estimated drawn width of a content-sized text button (matches `ui.button`).
pub fn text_button_w(label: &str) -> f32 {
    label.chars().count() as f32 * 7.0 + 22.0
}

/// Estimated drawn width of a checkbox with its label.
pub fn checkbox_w(label: &str) -> f32 {
    18.0 + 6.0 + label.chars().count() as f32 * 7.0
}

/// One item in an overflow-capable toolbar row.
pub struct OverflowItem {
    /// Estimated drawn width in px (see `text_button_w` / `checkbox_w`).
    pub width: f32,
}

/// Render toolbar items left-to-right; items that do not fit go into a ">>"
/// overflow menu (VS Code pattern) instead of truncating into "Zoo..." soup.
/// `render(ui, index, in_menu)` draws item `index`; `in_menu` lets it adapt
/// (menu rows are full-width). At least one item always stays inline.
pub fn overflow_toolbar(
    ui: &mut egui::Ui,
    items: &[OverflowItem],
    mut render: impl FnMut(&mut egui::Ui, usize, bool),
) {
    let avail_w = ui.available_width().max(64.0);
    let spacing = ui.spacing().item_spacing.x;
    let more_w = 40.0; // the ">>" button itself
    if items.is_empty() {
        return;
    }
    let total: f32 =
        items.iter().map(|i| i.width).sum::<f32>() + spacing * items.len().saturating_sub(1) as f32;
    ui.horizontal(|ui| {
        if total <= avail_w {
            for idx in 0..items.len() {
                render(ui, idx, false);
            }
            return;
        }
        // Overflow: fit as many leading items as possible, reserving the ">>".
        let mut inline_count = 1; // never strand the row with only ">>"
        let mut used = items[0].width;
        for (idx, item) in items.iter().enumerate().skip(1) {
            if used + spacing + item.width + spacing + more_w <= avail_w {
                used += spacing + item.width;
                inline_count = idx + 1;
            } else {
                break;
            }
        }
        for idx in 0..inline_count {
            render(ui, idx, false);
        }
        ui.menu_button(">>", |ui| {
            ui.label(egui::RichText::new("More actions").strong().small());
            ui.separator();
            for idx in inline_count..items.len() {
                render(ui, idx, true);
            }
        })
        .response
        .on_hover_text("More toolbar actions");
    });
}

/// Status bar segment visibility (VS Code pattern: status bar items declare a
/// priority; when the bar is too narrow, low-priority items are hidden, never
/// painted over each other or the right-side indicator). `widths` are the
/// measured segment widths in paint order; the first segment (the location
/// crumb) is always kept. `limit` is the x coordinate where the reserved
/// right-side indicator begins. Returns the number of leading segments that
/// stay visible; the rest are skipped, not overlapped.
pub fn status_visible_count(widths: &[f32], limit: f32) -> usize {
    if widths.is_empty() {
        return 0;
    }
    let mut x = 0.0f32;
    let mut n = 0usize;
    for (i, w) in widths.iter().enumerate() {
        if i > 0 && x + w > limit {
            break;
        }
        x += w;
        n = i + 1;
    }
    n
}

/// Sim workspace nav/wave split. A fixed 220px nav left the wave pane with a
/// negative width on narrow windows; the nav now keeps a proportional share
/// (VS Code's GridView scales children proportionally when the container
/// shrinks), between a 160px floor and the usual 220px, never wider than the
/// pane itself.
pub fn sim_nav_share(pane_w: f32) -> f32 {
    (pane_w * 0.32)
        .clamp(160.0, SIDEBAR_WIDTH)
        .min(pane_w.max(0.0))
}

/// Sim workspace rect split: (nav_w, sep_w, wave_w) for a pane of `pane_w`.
/// The three always sum to the pane: on absurdly narrow panes the separator
/// yields first (it is parent-clipped anyway), then the wave pane, so egui
/// never gets an inverted rect.
pub fn sim_split(pane_w: f32) -> (f32, f32, f32) {
    let w = pane_w.max(0.0);
    let nav = sim_nav_share(w);
    let sep = SPLITTER_GRAB_PX.min((w - nav).max(0.0));
    let wave = (w - nav - sep).max(0.0);
    (nav, sep, wave)
}

/// Spec-style table column widths that always sum to exactly `avail`, so the
/// last column is never clipped on narrow windows. `fixed` holds each column's
/// spec width in order (`None` = fill column, each at least `fill_min`).
/// Above the floor the fixed columns keep their widths and fill columns share
/// the leftover; below the floor every column scales together - VS Code's
/// Problems panel likewise never scrolls horizontally, its rows shrink instead.
pub fn spec_col_widths(fixed: &[Option<f32>], fill_min: f32, avail: f32) -> Vec<f32> {
    let fixed_sum: f32 = fixed.iter().filter_map(|c| *c).sum();
    let n_fill = fixed.iter().filter(|c| c.is_none()).count().max(1) as f32;
    let full = fixed_sum + n_fill * fill_min;
    let w = avail.max(1.0);
    if w >= full {
        let fill = (w - fixed_sum) / n_fill;
        fixed.iter().map(|c| c.unwrap_or(fill)).collect()
    } else {
        let scale = w / full;
        fixed.iter().map(|c| c.unwrap_or(fill_min) * scale).collect()
    }
}

/// DRC violations table: fixed 80px ID badge plus Objects/Message fill columns
/// in a 35/65 split. Same never-clip contract as `spec_col_widths`: the split
/// holds above the floor, everything scales together below it.
pub fn drc_col_widths(avail: f32) -> [f32; 3] {
    const ID: f32 = 80.0;
    const FILL_MIN: f32 = 90.0;
    let w = avail.max(1.0);
    let full = ID + 2.0 * FILL_MIN;
    if w >= full {
        let obj = ((w - ID) * 0.35).max(FILL_MIN);
        let msg = (w - ID - obj).max(FILL_MIN);
        [ID, obj, msg]
    } else {
        let scale = w / full;
        [ID * scale, FILL_MIN * scale, FILL_MIN * scale]
    }
}

/// Timing path table column widths that always sum to the pane width.
/// Column order: # | From | To | Launch clock | Capture clock | Slack | Levels | Path group.
pub fn timing_path_col_widths(avail: f32) -> Vec<f32> {
    spec_col_widths(
        &[
            Some(40.0),
            None,
            None,
            Some(110.0),
            Some(110.0),
            Some(96.0),
            Some(64.0),
            Some(96.0),
        ],
        80.0,
        avail,
    )
}

/// ILA setup pane: the probes column keeps a proportional share (VS Code's
/// debug panels share space through sashes with minimums) instead of a fixed
/// 340px that clipped the trigger column on narrow panes.
pub fn ila_probes_share(pane_w: f32) -> f32 {
    (pane_w * 0.42).clamp(220.0, 340.0).min(pane_w.max(0.0))
}

/// Device toolbar items as width estimates for `overflow_toolbar` (the same
/// house pattern the schematic toolbar uses): estimated button widths in
/// paint order. The "Zoom NN%" readout is informational and drops into the
/// overflow menu first on narrow windows.
pub fn device_toolbar_widths(mark_debug_label: &str, zoom_label: &str) -> Vec<OverflowItem> {
    let mut items: Vec<OverflowItem> = [
        "Find...",
        "Ruler",
        "Clear",
        "Layers [+]",
        "Inspector [+]",
        "Zoom to selection",
        "Zoom In",
        "Zoom Out",
        "Zoom Fit",
        mark_debug_label,
        "Add Probe",
    ]
    .iter()
    .map(|l| OverflowItem {
        width: text_button_w(l),
    })
    .collect();
    items.push(OverflowItem {
        width: text_button_w(zoom_label),
    });
    items
}

/// Editor gutter / minimap geometry for the reflow rule below.
pub const EDITOR_GUTTER_W: f32 = 56.0;
pub const EDITOR_MINIMAP_W: f32 = 110.0;
/// Text floor below which the minimap would push the text out of the pane.
pub const EDITOR_MIN_TEXT_W: f32 = 160.0;

/// The minimap is a fixed 110px dock that must never push the text out of the
/// pane (VS Code: the minimap never overflows the editor viewport). Below this
/// pane width the minimap hides and the text takes the leftover.
pub fn editor_minimap_visible(pane_w: f32) -> bool {
    pane_w >= EDITOR_GUTTER_W + EDITOR_MIN_TEXT_W + EDITOR_MINIMAP_W + 4.0
}

/// Welcome column width: centered up to 620px, but never wider than the pane
/// (a 320px floor painted past the pane edge on narrow windows).
pub fn welcome_col_w(avail_w: f32) -> f32 {
    (avail_w - 64.0).clamp(0.0, 620.0)
}

/// Welcome primary actions stack vertically when a three-across row would crush
/// them below a readable width (VS Code's welcome page likewise reflows its
/// action rows instead of clipping).
pub fn welcome_stack_actions(col_w: f32) -> bool {
    (col_w - 2.0 * 12.0) / 3.0 < 110.0
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

    /// Name under the letter. Full words - the rail is wide enough to paint them.
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
    /// One row; overflow goes into a More ... menu (shipped).
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

/// One row + More .... Never wrap (wrap steals canvas).
pub fn fit_or_more(
    labels: &[&'static str],
    available: f32,
) -> (Vec<&'static str>, Vec<&'static str>) {
    if !would_clip(labels, available) {
        return (labels.to_vec(), Vec::new());
    }
    let vis = visible_if_clipped(labels, (available - label_extent(MORE)).max(1.0));
    let overflow: Vec<_> = labels
        .iter()
        .copied()
        .filter(|l| !vis.contains(l))
        .collect();
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

/// Shipped workspace strip: one row + More .... Never wrap.
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

/// Left gutter for Y index labels. Right chrome is a few px, not a letterbox slab.
pub const FLOORPLAN_LEFT: f32 = 26.0;
pub const FLOORPLAN_RIGHT: f32 = 6.0;
pub const FLOORPLAN_TOP: f32 = 4.0;
pub const FLOORPLAN_BOT: f32 = 18.0;

fn floorplan_zoom_clamped(zoom: f32) -> f32 {
    if zoom.is_finite() {
        zoom.clamp(0.40, 6.0)
    } else {
        1.0
    }
}

/// Anisotropic die cells. Zoom Fit fills leftover width and height (right gap is
/// `FLOORPLAN_RIGHT`, not a void). Zoom-out shrinks both axes. Zoom-in grows both
/// so the parent ScrollArea can scroll to the die bottom.
///
/// At Zoom Fit (`zoom` ≤ 1) the painted extent must not exceed the leftover rect.
/// A short window shrinks the die into that rect; it must not clip Y=0.
pub fn floorplan_zoom_cells(
    cols: u32,
    rows: u32,
    avail_w: f32,
    avail_h: f32,
    zoom: f32,
) -> (f32, f32) {
    let cols_f = cols.max(1) as f32;
    let rows_f = rows.max(1) as f32;
    let aw = avail_w.max(1.0);
    let ah = avail_h.max(1.0);
    let z = floorplan_zoom_clamped(zoom);
    let inner_w = (aw - FLOORPLAN_LEFT - FLOORPLAN_RIGHT).max(1.0);
    let inner_h = (ah - FLOORPLAN_TOP - FLOORPLAN_BOT).max(1.0);
    let mut cw = inner_w / cols_f * z;
    let mut ch = inner_h / rows_f * z;
    if z <= 1.001 {
        // Chrome floors must not invent height past a short leftover.
        let inner_w_fit = (aw - FLOORPLAN_LEFT - FLOORPLAN_RIGHT).max(0.0);
        let inner_h_fit = (ah - FLOORPLAN_TOP - FLOORPLAN_BOT).max(0.0);
        cw = (inner_w_fit / cols_f) * z;
        ch = (inner_h_fit / rows_f) * z;
        if inner_w_fit < 1.0 {
            cw = (aw * 0.82 / cols_f).max(0.05);
        }
        if inner_h_fit > 0.0 {
            ch = ch.min(inner_h_fit / rows_f);
        } else {
            ch = (ah * 0.55 / rows_f).max(0.05);
        }
    } else {
        cw = cw.max(0.5);
        ch = ch.max(0.5);
    }
    (cw.max(0.05), ch.max(0.05))
}

/// Smaller cell, for tests that still speak a single scale.
/// Zoom Fit fills both axes; this is `min(cell_w, cell_h)`, not a width cap that letterboxes.
pub fn floorplan_zoom_cell(cols: u32, rows: u32, avail_w: f32, avail_h: f32, zoom: f32) -> f32 {
    let (cw, ch) = floorplan_zoom_cells(cols, rows, avail_w, avail_h, zoom);
    cw.min(ch)
}

/// Content size of the die including axis chrome.
pub fn floorplan_zoom_content(
    cols: u32,
    rows: u32,
    avail_w: f32,
    avail_h: f32,
    zoom: f32,
) -> (f32, f32) {
    let (cw, ch) = floorplan_zoom_cells(cols, rows, avail_w, avail_h, zoom);
    (
        FLOORPLAN_LEFT + cw * cols.max(1) as f32 + FLOORPLAN_RIGHT,
        FLOORPLAN_TOP + ch * rows.max(1) as f32 + FLOORPLAN_BOT,
    )
}

/// Empty pixels to the right of a Zoom Fit die (left-aligned paint).
pub fn floorplan_zoom_right_gap(
    cols: u32,
    rows: u32,
    avail_w: f32,
    avail_h: f32,
    zoom: f32,
) -> f32 {
    let (w, _) = floorplan_zoom_content(cols, rows, avail_w, avail_h, zoom);
    (avail_w - w).max(0.0)
}

/// True when a zoomed die still fits both axes (Zoom Fit and zoom-out).
pub fn floorplan_zoom_fits(cols: u32, rows: u32, avail_w: f32, avail_h: f32, zoom: f32) -> bool {
    let (w, h) = floorplan_zoom_content(cols, rows, avail_w, avail_h, zoom);
    w <= avail_w + 1.0 && h <= avail_h + 1.0
}

/// Fit zoom that keeps the whole schematic sheet, including PORT_OUT, inside the pane.
pub fn schematic_fit_zoom(sheet_w: f32, sheet_h: f32, vw: f32, vh: f32) -> f32 {
    // Touch a pane edge (no letterbox inset): surface Zoom Fit contract.
    let zx = vw.max(1.0) / sheet_w.max(1.0);
    let zy = vh.max(1.0) / sheet_h.max(1.0);
    zx.min(zy).clamp(0.05, 16.0)
}

/// One Zoom Out step, never larger than the pane. Pan is reset by `schematic_frame_pan`.
pub fn schematic_zoom_out_zoom(sheet_w: f32, sheet_h: f32, vw: f32, vh: f32, current: f32) -> f32 {
    let fit = schematic_fit_zoom(sheet_w, sheet_h, vw, vh);
    let cur = if current.is_finite() {
        current.max(0.05)
    } else {
        1.0
    };
    (cur / 1.35).clamp(0.05, 16.0).min(fit)
}

/// Center the sheet so left and right symbols (PORT_IN / PORT_OUT) stay in the canvas.
pub fn schematic_frame_pan(sheet_w: f32, sheet_h: f32, vw: f32, vh: f32, zoom: f32) -> (f32, f32) {
    let z = if zoom.is_finite() {
        zoom.max(0.05)
    } else {
        1.0
    };
    (
        (vw - sheet_w.max(1.0) * z) * 0.5,
        (vh - sheet_h.max(1.0) * z) * 0.5,
    )
}

/// Identity camera whose sheet bottom (full cell, including a name under the box)
/// falls outside the pane. Zoom Fit then frames that bottom; a user pan/zoom is left alone.
pub fn schematic_identity_clips_bottom(
    zoom: f32,
    pan_x: f32,
    pan_y: f32,
    sheet_h: f32,
    vh: f32,
) -> bool {
    let identity = (zoom - 1.0).abs() < 0.02 && pan_x.abs() < 0.5 && pan_y.abs() < 0.5;
    if !identity {
        return false;
    }
    let z = if zoom.is_finite() {
        zoom.max(0.05)
    } else {
        1.0
    };
    pan_y + sheet_h.max(1.0) * z > vh - 2.0
}

/// Device sidebar port line. Site is the full HAD name (`IOB_X17Y0`), never ellipsized.
pub fn device_io_site_line(name: &str, dir: &str, site: &str) -> String {
    let site = if site.is_empty() || site == "-" {
        "unplaced"
    } else {
        site
    };
    format!("{name}  {dir}  {site}")
}

pub fn device_io_site_line_fits(name: &str, dir: &str, site: &str, avail: f32) -> bool {
    let line = device_io_site_line(name, dir, site);
    line.chars().count() as f32 * 7.0 + 8.0 <= avail + 0.5
}

/// Shared chrome for the current window. Floors, then scroll - never a fixed letterbox
/// and never a pane taller than the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SharedChrome {
    pub sidebar_w: f32,
    pub sidebar_floor: f32,
    pub sidebar_cap: f32,
    pub console_h: f32,
    pub console_floor: f32,
    pub console_cap: f32,
    pub tables_max_h: f32,
    pub canvas_floor_h: f32,
}

/// Device, schematic, reports, and console share the available rect.
/// Wide windows leave spare for the die/sheet. Narrow or short windows shrink to a floor, then scroll.
pub fn share_available(window_w: f32, window_h: f32) -> SharedChrome {
    let w = window_w.max(1.0);
    let h = window_h.max(1.0);
    let chrome_y = TOOLBAR_HEIGHT + STATUS_HEIGHT + 6.0;
    let body_h = (h - chrome_y).max(1.0);
    let side_budget = (w - RAIL_WIDTH).max(1.0);

    let sidebar_floor = 128.0_f32.min(side_budget * 0.46).max(88.0);
    let sidebar_cap = (side_budget * 0.30)
        .clamp(sidebar_floor, SIDEBAR_MAX_WIDTH)
        .max(sidebar_floor);
    // Hold the usual 220 when the pane can spare it; shrink before the canvas letterboxes.
    let sidebar_w = SIDEBAR_WIDTH.clamp(sidebar_floor, sidebar_cap);

    // Floor hugs the tab strip (~40px) so the console can collapse to
    // "completely small" - the user's drag, not the chrome, decides the height.
    let console_floor = 40.0_f32.min(body_h * 0.28).max(36.0);
    let console_cap = (body_h * 0.26)
        .clamp(console_floor, CONSOLE_MAX_HEIGHT)
        .max(console_floor);
    let console_h = (body_h * 0.16).clamp(console_floor, console_cap);

    let remain_h = (body_h - console_h).max(1.0);
    let tables_max_h = (remain_h * 0.24).clamp(44.0, DEVICE_TABLES_MAX_HEIGHT);
    // Canvas claims the rest. Floor is a scroll threshold, not a min painted slab.
    let canvas_floor_h = (remain_h * 0.40).clamp(64.0, 240.0);

    SharedChrome {
        sidebar_w,
        sidebar_floor,
        sidebar_cap,
        console_h,
        console_floor,
        console_cap,
        tables_max_h,
        canvas_floor_h,
    }
}

/// Drawing height that fills leftover height and never exceeds it.
pub fn pane_view_h(avail_h: f32) -> f32 {
    avail_h.max(1.0)
}

/// Narrow Device window: tables and legend floor, then scroll sideways.
pub const DEVICE_NARROW_W: f32 = 1100.0;
/// Short Device window: clock-region table floors on whole rows; leftover is Zoom Fit.
pub const DEVICE_SHORT_H: f32 = 540.0;
/// Die leftover on a narrow window. A few pixels is a crushed sliver.
pub const DEVICE_DIE_MIN_H: f32 = 168.0;
/// One legend / table row. Viewport heights are multiples of this so a row is not sliced.
pub const DEVICE_LEGEND_ROW_H: f32 = 22.0;
pub const DEVICE_TABLE_FLOOR_H: f32 = 92.0;
pub const DEVICE_CR_HEAD_H: f32 = 22.0;
pub const DEVICE_CR_ROW_H: f32 = 22.0;

pub fn device_window_narrow(window_w: f32) -> bool {
    window_w < DEVICE_NARROW_W
}

pub fn device_window_short(window_h: f32) -> bool {
    window_h < DEVICE_SHORT_H
}

/// Clock-region viewport: heading plus whole rows only.
pub fn clock_region_viewport_h(visible_rows: u32) -> f32 {
    DEVICE_CR_HEAD_H + DEVICE_CR_ROW_H * visible_rows.max(1) as f32
}

/// How the Device pane under the legend is split.
/// Narrow: tables floor then scroll horizontally; die keeps a usable height.
/// Short: clock-region table floors on whole rows and scrolls; leftover is Zoom Fit.
/// Wide / default: pblock and clock-region tables are a short strip
/// (heading plus 1–2 rows); the die takes the majority of leftover.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceBandShare {
    pub narrow: bool,
    pub short: bool,
    pub tables_h: f32,
    pub clock_h: f32,
    pub die_min_h: f32,
}

pub fn device_band_share(window_w: f32, window_h: f32, after_legend: f32) -> DeviceBandShare {
    let narrow = device_window_narrow(window_w);
    let short = device_window_short(window_h);
    let after = after_legend.max(1.0);
    if !narrow && !short {
        // Heading plus two rows. Do not hand clock regions 168px or 38–55% of the pane.
        let strip = clock_region_viewport_h(2);
        let clock_h = strip;
        let tables_h = strip;
        let die_min_h = (after - tables_h - clock_h - 8.0).max(after * 0.55);
        return DeviceBandShare {
            narrow,
            short,
            tables_h,
            clock_h,
            die_min_h,
        };
    }
    let cr_rows = if short { 1 } else { 2 };
    let clock_h = clock_region_viewport_h(cr_rows);
    let tables_floor = if short { 72.0 } else { DEVICE_TABLE_FLOOR_H };
    let die_want = if short {
        // Leftover after the floors is Zoom Fit. Do not invent a slab past that leftover.
        // For very short windows (after < 64), clamp bounds must stay valid:
        // f32::clamp panics when min > max. Floor the die at 1px, not 64px.
        let lo = 1.0_f32.min(after.max(1.0));
        let hi = after.max(1.0);
        (after - tables_floor - clock_h - 10.0).clamp(lo, hi)
    } else {
        DEVICE_DIE_MIN_H.min((after * 0.50).max(120.0))
    };
    let tables_h = tables_floor.min((after - clock_h - die_want - 8.0).max(48.0));
    let die_left = (after - tables_h - clock_h - 8.0).max(1.0);
    DeviceBandShare {
        narrow,
        short,
        tables_h,
        clock_h,
        die_min_h: die_left,
    }
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
    fn from_drawn(
        pane_w: f32,
        pane_h: f32,
        scale_x: f32,
        scale_y: f32,
        drawn_w: f32,
        drawn_h: f32,
    ) -> Self {
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
        self.fill + 0.000_5 >= PANE_FILL_MIN
            && self.empty_gap <= PANE_EMPTY_GAP_MAX
            && self.right_clip <= 0.5
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
    (sheet_w / pane_w.max(1.0))
        .min(sheet_h / pane_h.max(1.0))
        .clamp(0.0, 1.0)
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

/// Silence unused WorkspaceTab import helper - canvases stay in sync with CANVASES.
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

/// Central pane a `WorkspaceTab` must fill. More ... destinations never collapse to Timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspacePane {
    Editor,
    Device,
    Timing,
    ReportsCatalog,
    Schematic,
    Package,
    Bitstream,
    Hardware,
    Ip,
    Find,
    Settings,
    Summary,
    Wave,
    Ila,
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
    Qor,
    Drc,
    Utilization,
    Testbench,
}

/// Map every WorkspaceTab onto a real pane. None of these is a stub / popup / Timing dump.
pub fn pane_for_workspace(tab: WorkspaceTab) -> WorkspacePane {
    match tab {
        WorkspaceTab::TextEditor => WorkspacePane::Editor,
        WorkspaceTab::Device => WorkspacePane::Device,
        WorkspaceTab::Reports => WorkspacePane::ReportsCatalog,
        WorkspaceTab::Schematic => WorkspacePane::Schematic,
        WorkspaceTab::Package => WorkspacePane::Package,
        WorkspaceTab::Bitstream => WorkspacePane::Bitstream,
        WorkspaceTab::Hardware => WorkspacePane::Hardware,
        WorkspaceTab::Ip => WorkspacePane::Ip,
        WorkspaceTab::Find => WorkspacePane::Find,
        WorkspaceTab::Settings => WorkspacePane::Settings,
        WorkspaceTab::Summary => WorkspacePane::Summary,
        WorkspaceTab::Wave => WorkspacePane::Wave,
        WorkspaceTab::Ila => WorkspacePane::Ila,
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
        WorkspaceTab::Qor => WorkspacePane::Qor,
        WorkspaceTab::Drc => WorkspacePane::Drc,
        WorkspaceTab::Utilization => WorkspacePane::Utilization,
        WorkspaceTab::Testbench => WorkspacePane::Testbench,
    }
}

/// Non-canvas WorkspaceTabs live in More ... and must paint `pane_for_workspace` in the center.
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
            | WorkspaceTab::Qor
            | WorkspaceTab::Drc
            | WorkspaceTab::Utilization
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
        assert_eq!(WorkspaceTab::ALL.len(), 29);
        assert_eq!(WorkspaceTab::CANVASES.len(), 3);

        let avail = DESKTOP_WIDTH - side_chrome_width();
        assert!(
            !would_clip(&labels, avail),
            "3 canvases must fit at 1440 (avail={avail})"
        );
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
            vec![
                "Files", "Device", "Timing", "Simulate", "Program", "Reports"
            ]
        );
        for a in Activity::ALL {
            assert!(plan.rail_is_selectable(a.label()), "rail {}", a.label());
        }
        for (label, _) in RAIL_OPEN_SOURCES {
            assert!(
                !plan.rail_actions.iter().any(|l| *l == label),
                "{label} on rail"
            );
            assert!(!plan.rail_is_selectable(label));
        }
        assert!(!plan.rail_actions.iter().any(|l| l.starts_with("Open ")));
        assert_eq!(plan.dropped_rail(), 0);
        assert_eq!(plan.nav_sections.len(), 6);
        assert_ne!(plan.nav_sections.len(), 9);

        assert_eq!(plan.bottom_tabs, vec!["Console", "Messages", "Jobs"]);
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
            "More ...: {:?}",
            tiny.tab_rows
        );
        assert_eq!(tiny.dropped_workspace(), 0);
        assert!(tiny.tab_is_selectable("Timing"));

        let cell = floorplan_fit_cell(32, 33, 800.0, 500.0);
        assert!(cell >= 4.0 && cell <= 64.0);
        let fit = floorplan_zoom_cell(32, 33, 800.0, 360.0, 1.0);
        assert!(
            floorplan_zoom_fits(32, 33, 800.0, 360.0, 1.0),
            "Zoom Fit must fit both axes, cell={fit}"
        );
        assert!(
            floorplan_zoom_fits(32, 33, 520.0, 72.0, 1.0),
            "short leftover must Zoom Fit, not clip Y=0"
        );
        let (short_w, short_h) = floorplan_zoom_content(32, 33, 520.0, 72.0, 1.0);
        assert!(short_h <= 72.0 + 1.0, "short die height {short_h}");
        assert!(short_w <= 520.0 + 1.0, "short die width {short_w}");
        let (wide_w, _) = floorplan_zoom_content(32, 33, 1100.0, 360.0, 1.0);
        let (fit_w, _) = floorplan_zoom_content(32, 33, 800.0, 360.0, 1.0);
        assert!(
            wide_w > fit_w + 40.0,
            "wide window must grow the die, {wide_w} vs {fit_w}"
        );
        assert!(
            !floorplan_zoom_fits(32, 33, 800.0, 360.0, 2.0),
            "zoom-in must overflow so the die ScrollArea can scroll"
        );
        let zout = floorplan_zoom_cell(32, 33, 800.0, 360.0, 0.5);
        assert!(
            zout < fit && zout > 2.0,
            "zoom-out stays readable, cell={zout}"
        );
        // Zoom Fit fills leftover width: right gap is chrome, not a letterbox void.
        let gap = floorplan_zoom_right_gap(32, 33, 800.0, 360.0, 1.0);
        assert!(gap <= 8.0, "Zoom Fit right gap must be a few px, got {gap}");
        let (cw, ch) = floorplan_zoom_content(32, 33, 800.0, 360.0, 1.82);
        assert!(
            ch > 360.0,
            "zoom-in must grow past the pane so the die bottom scrolls, h={ch}"
        );
        assert!(cw > 800.0 || ch > 360.0);
        let z = schematic_zoom_out_zoom(1400.0, 400.0, 900.0, 500.0, 1.0);
        let (px, _py) = schematic_frame_pan(1400.0, 400.0, 900.0, 500.0, z);
        let right = px + 1400.0 * z;
        assert!(
            right <= 900.0 + 0.5,
            "PORT_OUT must stay inside after zoom-out, right={right}"
        );
        assert!(px + 0.5 >= 0.0, "PORT_IN must stay inside, pan={px}");
        assert!(device_io_site_line_fits(
            "led",
            "OUT",
            "IOB_X31Y16",
            SIDEBAR_WIDTH - 16.0
        ));
        assert!(!device_io_site_line("led", "OUT", "IOB_X31Y16").contains('…'));
        let narrow_band = device_band_share(920.0, 620.0, 360.0);
        assert!(narrow_band.narrow && !narrow_band.short);
        assert!(
            narrow_band.die_min_h >= 120.0,
            "narrow die must stay usable, got {}",
            narrow_band.die_min_h
        );
        assert!(narrow_band.tables_h <= DEVICE_TABLE_FLOOR_H + 0.5);
        assert_eq!(narrow_band.clock_h, clock_region_viewport_h(2));
        assert_eq!(
            clock_region_viewport_h(2) - DEVICE_CR_HEAD_H,
            DEVICE_CR_ROW_H * 2.0
        );
        let short_band = device_band_share(1120.0, 490.0, 240.0);
        assert!(short_band.short && !short_band.narrow);
        assert_eq!(short_band.clock_h, clock_region_viewport_h(1));
        let (short_die_w, short_die_h) =
            floorplan_zoom_content(32, 33, 700.0, short_band.die_min_h, 1.0);
        assert!(
            short_die_h <= short_band.die_min_h + 1.0,
            "short leftover must Zoom Fit, not a clipped 100% band, h={short_die_h}"
        );
        assert!(short_die_w <= 700.0 + 1.0);
        let wide_band = device_band_share(1270.0, 630.0, 400.0);
        assert!(!wide_band.narrow && !wide_band.short);
        assert!(
            wide_band.die_min_h > 400.0 * 0.5 && wide_band.die_min_h > 56.0,
            "wide die must take the majority of leftover under the legend, got {}",
            wide_band.die_min_h
        );
        assert_eq!(wide_band.clock_h, clock_region_viewport_h(2));
        assert!(
            wide_band.clock_h < 168.0,
            "wide clock strip must stay short, got {}",
            wide_band.clock_h
        );
        assert!(
            wide_band.tables_h <= clock_region_viewport_h(2) + 0.5,
            "wide pblock strip must stay short, got {}",
            wide_band.tables_h
        );
        let wide = share_available(1600.0, 900.0);
        let narrow = share_available(780.0, 700.0);
        let short = share_available(1200.0, 420.0);
        assert!(
            wide.sidebar_w + 8.0 < 1600.0 - RAIL_WIDTH,
            "wide spare stays for the die"
        );
        assert!(narrow.sidebar_w <= wide.sidebar_w);
        assert!(narrow.sidebar_w >= narrow.sidebar_floor - 0.5);
        assert!(
            short.console_h < CONSOLE_DEFAULT_HEIGHT,
            "short window shrinks console, got {}",
            short.console_h
        );
        assert!(short.tables_max_h <= DEVICE_TABLES_MAX_HEIGHT);
        assert!(short.console_h + short.canvas_floor_h < 420.0);
        assert_eq!(pane_view_h(90.0), 90.0);
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
        for label in [
            "New Project…",
            "Open…",
            "Bitstream",
            "Implement",
            "Implementing…",
        ] {
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
            narrow.workspace_mode == OverflowMode::Fit
                || narrow.tab_rows.iter().any(|r| r.contains(&MORE)),
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
        assert_eq!(
            toolbar_ctrl_size("Open…")[1],
            toolbar_ctrl_size("Implement")[1]
        );
        // Waveform lanes are density-driven (no stretch-to-fill).
        assert_eq!(wave_trace_row_h(WaveDensity::Comfortable), 56.0);
        assert_eq!(wave_trace_row_h(WaveDensity::Compact), 36.0);
        let w = stretched_col_w(7, 700.0);
        assert!(w >= 48.0, "{w}");
        assert!(
            w * 7.0 + 12.0 * 6.0 <= 700.0 + 1.0,
            "stretched columns plus gaps must fit the pane ({w})"
        );
        assert!(
            !schematic_pin_visible(true, false),
            "n/c hidden unless selected"
        );
        assert!(
            schematic_pin_visible(true, true),
            "n/c shown on selected cell"
        );
        assert!(
            schematic_pin_visible(false, false),
            "connected pins always shown"
        );
        assert!(ip_catalog_name_fits("h_rv32_hb1"));
        assert!(ip_catalog_name_fits("h_uart"));
        assert!(ip_catalog_chip_w("h_rv32_hb1") >= 96.0);
        // Program bbox must not be padded up to 80% by the helper itself.
        let hw = hardware_content_bbox(8, 1000.0, 500.0);
        assert!(
            hw.drawn_w < 1000.0 * PANE_FILL_MIN || hw.fill >= PANE_FILL_MIN,
            "bbox must measure tiles, not clamp to 80%: {hw:?}"
        );
        assert!(
            hw.fills() || hw.fill >= PANE_FILL_MIN,
            "program tiles {hw:?}"
        );
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
        assert!(
            tight > legacy,
            "tight occupancy bars {tight} vs legacy {legacy}"
        );
        assert!(
            pane_w - labels4 - tight <= PANE_EMPTY_GAP_MAX,
            "occupancy bars must meet Hierarchical right edge"
        );
    }

    #[test]
    fn more_destinations_never_fall_through_to_timing_pane() {
        use crate::WorkspaceTab;
        assert_eq!(WorkspaceTab::ALL.len(), 29);
        assert_eq!(
            pane_for_workspace(WorkspaceTab::Schematic),
            WorkspacePane::Schematic
        );
        assert_ne!(
            pane_for_workspace(WorkspaceTab::Schematic),
            WorkspacePane::Timing
        );
        assert_eq!(
            pane_for_workspace(WorkspaceTab::Package),
            WorkspacePane::Package
        );
        // Grammar R15: Hierarchy is a Z3 sidebar section, not a tab. No
        // WorkspaceTab routes to a hierarchy central pane anymore.
        assert!(WorkspaceTab::parse_label("Hierarchy").is_none());
        assert_eq!(
            pane_for_workspace(WorkspaceTab::Summary),
            WorkspacePane::Summary
        );
        assert_eq!(
            pane_for_workspace(WorkspaceTab::Settings),
            WorkspacePane::Settings
        );
        assert_eq!(
            pane_for_workspace(WorkspaceTab::Bitstream),
            WorkspacePane::Bitstream
        );
        assert_eq!(
            pane_for_workspace(WorkspaceTab::Hardware),
            WorkspacePane::Hardware
        );
        assert_eq!(pane_for_workspace(WorkspaceTab::Ip), WorkspacePane::Ip);
        assert_eq!(pane_for_workspace(WorkspaceTab::Find), WorkspacePane::Find);
        assert_eq!(
            pane_for_workspace(WorkspaceTab::Testbench),
            WorkspacePane::Testbench
        );
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
        assert_eq!(more, 26, "29 tabs − 3 canvases");
        assert!(is_report_detail(WorkspaceTab::Utilization));
        assert!(!is_report_detail(WorkspaceTab::Schematic));
        assert_eq!(
            pane_for_workspace(WorkspaceTab::Reports),
            WorkspacePane::ReportsCatalog
        );
        assert_eq!(
            pane_for_workspace(WorkspaceTab::TextEditor),
            WorkspacePane::Editor
        );
        assert_eq!(
            pane_for_workspace(WorkspaceTab::Device),
            WorkspacePane::Device
        );
    }

    /// Grammar R12/R13 (P4): run management lives in the Z6 bottom panel as the
    /// Jobs tab, next to the console that drives it. Design Runs and
    /// Checkpoints must not exist as Z5 workspace tabs or report details.
    #[test]
    fn runs_and_checkpoints_live_in_z6_jobs_not_z5() {
        use crate::BottomTab;
        use crate::WorkspaceTab;
        // Z6: the Jobs tab exists, ordered after Console/Messages (grammar section 4).
        assert!(BottomTab::ALL.contains(&BottomTab::Jobs));
        assert_eq!(BottomTab::Jobs.paint_label(), "Jobs");
        assert_eq!(BottomTab::Jobs.label(), "Jobs");
        let home: Vec<&str> = BottomTab::HOME.iter().map(|t| t.paint_label()).collect();
        assert_eq!(home, vec!["Console", "Messages", "Jobs"]);
        // Z5: no workspace tab routes to a runs/checkpoints pane.
        for tab in WorkspaceTab::ALL {
            let dbg = format!("{tab:?}");
            assert_ne!(dbg, "Runs", "WorkspaceTab::Runs must not exist (R12)");
            assert_ne!(
                dbg, "Checkpoints",
                "WorkspaceTab::Checkpoints must not exist (R13)"
            );
        }
        assert!(WorkspaceTab::parse_label("Design Runs").is_none());
        assert!(WorkspaceTab::parse_label("Checkpoints").is_none());
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

#[cfg(test)]
mod resize_tests {
    use super::*;

    /// Minimum window keeps a usable central pane (VS Code pattern: parts
    /// declare minimums, the grid never overlaps). At 720x400 the chrome
    /// budget is rail 88 + sidebar floor, leaving the pane room to breathe.
    #[test]
    fn min_window_keeps_usable_central_pane() {
        assert_eq!(MIN_WINDOW_W, 720.0);
        assert_eq!(MIN_WINDOW_H, 400.0);
        let share = share_available(MIN_WINDOW_W, MIN_WINDOW_H);
        let central = MIN_WINDOW_W - RAIL_WIDTH - share.sidebar_floor;
        assert!(
            central >= 500.0,
            "central pane {central} too narrow at minimum window"
        );
        // Console still has its floor; the timing canvas scrolls below it.
        assert!(share.console_floor <= 40.0);
        assert!(share.console_h <= MIN_WINDOW_H);
    }

    /// Status bar: low-priority segments hide instead of overlapping the
    /// right-side indicator (VS Code priority-hide pattern).
    #[test]
    fn status_bar_hides_low_priority_segments_when_narrow() {
        let widths = [120.0, 90.0, 90.0, 90.0, 90.0];
        // Wide: everything fits.
        assert_eq!(status_visible_count(&widths, 2000.0), 5);
        // Narrow: the crumb always survives; later segments drop.
        assert_eq!(status_visible_count(&widths, 250.0), 2);
        assert_eq!(status_visible_count(&widths, 119.0), 1);
        // Empty input: nothing to show.
        assert_eq!(status_visible_count(&[], 2000.0), 0);
        // A segment that exactly fits the limit stays.
        assert_eq!(status_visible_count(&[100.0, 100.0], 200.0), 2);
    }

    /// Sim nav keeps a proportional share; nav + separator + wave always sum
    /// to the pane, so egui never gets an inverted rect.
    #[test]
    fn sim_split_stays_valid_at_every_pane_width() {
        for pane_w in [1440.0, 720.0, 504.0, 300.0, 200.0, 166.0, 100.0, 0.0] {
            let (nav, sep, wave) = sim_split(pane_w);
            assert!(nav >= 0.0 && nav <= pane_w.max(0.0), "nav {nav} at {pane_w}");
            assert!(sep >= 0.0 && sep <= SPLITTER_GRAB_PX, "sep {sep} at {pane_w}");
            assert!(wave >= 0.0, "wave {wave} at {pane_w}");
            assert!(
                (nav + sep + wave - pane_w.max(0.0)).abs() < 0.01,
                "split sums {} at {pane_w}",
                nav + sep + wave
            );
        }
        // Wide pane: the usual 220px nav.
        let (nav, sep, _) = sim_split(1200.0);
        assert!((nav - SIDEBAR_WIDTH).abs() < 0.01, "nav {nav}");
        assert!((sep - SPLITTER_GRAB_PX).abs() < 0.01, "sep {sep}");
        // Narrow pane: proportional, floored at 160.
        let (nav, _, wave) = sim_split(500.0);
        assert!((nav - 160.0).abs() < 0.01, "nav {nav}");
        assert!(wave > 300.0, "wave {wave}");
    }

    /// Spec tables always sum to the pane width: fixed widths when wide,
    /// proportional shrink when narrow. No clipped last column.
    #[test]
    fn spec_col_widths_sum_to_pane_at_every_width() {
        for avail in [1440.0, 1200.0, 900.0, 676.0, 500.0, 442.0, 300.0, 212.0] {
            let ws = timing_path_col_widths(avail);
            assert_eq!(ws.len(), 8);
            let sum: f32 = ws.iter().sum();
            assert!(
                (sum - avail.max(1.0)).abs() < 1.0,
                "cols sum {sum} != pane {avail}"
            );
            assert!(ws.iter().all(|w| *w >= 0.0), "negative width at {avail}");
        }
        // Wide: spec widths intact, fill columns share the leftover.
        let ws = timing_path_col_widths(1200.0);
        assert!((ws[0] - 40.0).abs() < 0.01);
        assert!((ws[3] - 110.0).abs() < 0.01);
        assert!((ws[1] - 342.0).abs() < 1.0, "fill {:?}", ws[1]);
        // At the 676 floor the fills are exactly their 80px minimum.
        let ws = timing_path_col_widths(676.0);
        assert!((ws[1] - 80.0).abs() < 0.01);
        // Below the floor everything scales together.
        let ws = timing_path_col_widths(338.0);
        assert!((ws[0] - 20.0).abs() < 0.01, "hash {:?}", ws[0]);
        // Pin-detail table uses the same contract.
        let ws = spec_col_widths(
            &[None, Some(110.0), Some(90.0), Some(90.0), None, Some(70.0), Some(150.0)],
            80.0,
            442.0,
        );
        let sum: f32 = ws.iter().sum();
        assert!((sum - 442.0).abs() < 1.0, "pin detail sum {sum}");
    }

    /// ILA probes column: proportional share with a 340px cap and 220px
    /// floor; never wider than the pane itself.
    #[test]
    fn ila_probes_share_adapts_to_pane() {
        assert!((ila_probes_share(1200.0) - 340.0).abs() < 0.01);
        assert!((ila_probes_share(700.0) - 294.0).abs() < 1.0);
        assert!((ila_probes_share(400.0) - 220.0).abs() < 0.01);
        assert!((ila_probes_share(150.0) - 150.0).abs() < 0.01);
    }

    /// Device toolbar: estimated widths overflow a minimum-window pane (so the
    /// ">>" menu engages) and fit a wide one.
    #[test]
    fn device_toolbar_overflows_only_when_narrow() {
        let items = device_toolbar_widths("Mark Debug clk", "Zoom 100%");
        assert_eq!(items.len(), 13);
        let total: f32 = items.iter().map(|i| i.width).sum();
        let narrow_avail = 720.0 - side_chrome_width();
        assert!(
            total > narrow_avail,
            "toolbar {total} should overflow the {narrow_avail}px pane at min window"
        );
        let wide_avail = 1600.0 - side_chrome_width();
        assert!(
            total < wide_avail,
            "toolbar {total} should fit the {wide_avail}px pane at 1600"
        );
    }

    /// Editor minimap hides below the width where it would push the text out
    /// of the pane (VS Code: the minimap never overflows the viewport).
    #[test]
    fn editor_minimap_hides_before_it_can_overflow() {
        let floor = EDITOR_GUTTER_W + EDITOR_MIN_TEXT_W + EDITOR_MINIMAP_W + 4.0;
        assert!(!editor_minimap_visible(floor - 0.1));
        assert!(editor_minimap_visible(floor));
        assert!(editor_minimap_visible(1440.0));
        assert!(!editor_minimap_visible(200.0));
    }

    /// Welcome column never exceeds the pane; actions stack when a row would
    /// crush them.
    #[test]
    fn welcome_column_reflows_at_narrow_panes() {
        assert!((welcome_col_w(1600.0) - 620.0).abs() < 0.01);
        assert!((welcome_col_w(400.0) - 336.0).abs() < 0.01);
        assert!(welcome_col_w(200.0) <= 200.0);
        assert!(!welcome_stack_actions(620.0));
        assert!(welcome_stack_actions(336.0));
        assert!(welcome_stack_actions(200.0));
    }

    /// DRC table keeps its 35/65 Objects/Message split when wide and sums to
    /// the pane width when narrow.
    #[test]
    fn drc_col_widths_sum_to_pane_at_every_width() {
        for avail in [1200.0, 900.0, 500.0, 260.0, 200.0, 130.0] {
            let ws = drc_col_widths(avail);
            let sum: f32 = ws.iter().sum();
            assert!(
                (sum - avail.max(1.0)).abs() < 1.0,
                "drc cols sum {sum} != pane {avail}"
            );
            if avail >= 260.0 {
                assert!((ws[0] - 80.0).abs() < 0.01, "id badge {:?}", ws[0]);
            }
        }
        // Wide: the 35/65 split holds.
        let ws = drc_col_widths(1000.0);
        assert!((ws[1] - 322.0).abs() < 1.0, "objects {:?}", ws[1]);
        assert!((ws[2] - 598.0).abs() < 1.0, "message {:?}", ws[2]);
    }
}
