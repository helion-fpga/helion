//! Device die canvas: live site tiles, placed cells, net spans, layers, find.
//!
//! New Rune `El` code written from scratch for the Vivado brief: the
//! Implemented Design "Device" viewer — a dark die canvas (navy die, white
//! net spans, tile grid from the real HAD model) with placed yellow cells,
//! over a CAD toolbar / layers panel / find palette / element inspector.
//! Cells, nets, and sites all come from the live engine state
//! ([`crate::live`]): the session design for cells/nets, the `impl_1`
//! placement for sites/BELs, and the HAD part model for the tile grid.
//! Nothing here is a static demo netlist: with no elaborated design the die
//! stays visibly empty with a reason, and a synthesized-but-unplaced design
//! lists its cells as unplaced instead of inventing coordinates.
//!
//! Behavior: per-layer visibility ([`DeviceLayers`]), zoom in/out/fit toolbar,
//! pan via the scroll container, a live find palette over real cells+nets, and
//! cross-probe selection shared between canvas cells, find rows, the element
//! inspector, and the Sources tree (`Locate in Sources` selects the file and
//! echoes the Tcl verb, like the schematic links). Net spans are straight
//! orthogonal legs between real placed endpoints (labeled as spans, not
//! routed geometry); the clock-regions layer stays visibly disabled because
//! the HAD model exposes no region table. The Package view is the old HAD
//! IOB package grid with bank colors, live port occupancy, and pin/port
//! cross-probe selection.
//!
//! All colors come from [`VivadoTheme`]; no manual geometry beyond the
//! tile-grid world coordinates scaled by the zoom factor.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el.rs`: `row()/col()/el()/text()/icon()/canvas()`,
//!   `text_input(&TextEdit, style)`.
//! - `crates/rune/src/shape.rs`: `Shape::new/stroke/fill`, `LineCap`.
//! - `crates/rune/src/el/layout.rs`: `.absolute()` overlay nodes.
//! - `crates/rune/src/el/scroll.rs`: `.scroll_xy()` die pan container,
//!   `.scroll()` vertical package-grid viewport (so the wrapped balls take
//!   the viewport width instead of `scroll_xy`'s natural content width).
//! - `crates/rune/src/el/handlers.rs`: `.on_press()` selection dispatch.
//! - `crates/rune/src/el/text.rs`: `.single_line()`, `.placeholder()`,
//!   `.caret()`, `.on_edit()`, `.on_submit()`.
//! - `crates/rune/src/edit.rs`: `TextEdit::new`, its `text` field for reads.
//! - `crates/rune-kit/src/checkbox.rs`: `Checkbox::new/label/description/disabled/on_toggle/build`.
//! - `crates/rune-kit/src/glyphs.rs`: `SEARCH`.
//! - `crates/rune-kit/src/popover.rs`: `anchored/raised/fit_scroll` pins the
//!   Package Pin list to the table's own trigger cell, with `overlay::scrim`
//!   for outside-press close.

use rune::tok::TextStyle;
use rune::{El, Shape, Tx, canvas, col, el, icon, row, text, text_input};
use rune_kit::Checkbox;
use rune_kit::glyphs;

use crate::live::{
    LiveDieGeom, LiveNetlist, LivePlaced, live_geom, live_netlist, live_placed, slug,
};
use crate::state::{AppState, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::sources::{SourceFileId, pane_header};

/// Tile width in world points.
const TILE_W: f32 = 40.0;
/// Tile height in world points.
const TILE_H: f32 = 42.0;
/// Layers panel width.
const LAYERS_W: f32 = 158.0;
/// Find + inspector column width.
const RIGHT_W: f32 = 232.0;
/// Find results list height.
const FIND_H: f32 = 180.0;
/// Find row height (compact CAD density).
const ROW_H: f32 = 20.0;
/// Toolbar height.
const TOOL_H: f32 = 26.0;
/// Zoom step per in/out press.
const ZOOM_STEP: f32 = 1.25;
/// Zoom limits.
const ZOOM_MIN: f32 = 0.25;
/// Zoom limits.
const ZOOM_MAX: f32 = 4.0;

/// Canvas label type: smaller than CAD body text.
const FONT_PIN: TextStyle = TextStyle::new(10.0, 400.0);

/// Die world width for live geometry at 100%.
pub fn world_w(geom: &LiveDieGeom) -> f32 {
    geom.cols as f32 * TILE_W
}

/// Die world height for live geometry at 100%.
pub fn world_h(geom: &LiveDieGeom) -> f32 {
    geom.rows as f32 * TILE_H
}

/// World origin of a site tile at 100%.
pub fn site_xy(geom: &LiveDieGeom, x: u32, y: u32) -> [f32; 2] {
    [
        (x.saturating_sub(geom.x0)) as f32 * TILE_W,
        (y.saturating_sub(geom.y0)) as f32 * TILE_H,
    ]
}

/// Cross-probed die selection: a real placed cell, a real net, or a real
/// package pin. Names are engine names, never display strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceSel {
    Cell(String),
    Net(String),
    Pin(String),
}

impl DeviceSel {
    pub fn name(&self) -> &str {
        match self {
            DeviceSel::Cell(n) | DeviceSel::Net(n) | DeviceSel::Pin(n) => n,
        }
    }

    /// Canvas key for a cell (nets have no canvas node; they select via find).
    pub fn cell_key(name: &str) -> String {
        format!("device-cell-{}", slug(name))
    }

    pub fn key(&self) -> String {
        match self {
            DeviceSel::Cell(n) => Self::cell_key(n),
            DeviceSel::Net(n) => format!("device-net-{}", slug(n)),
            DeviceSel::Pin(n) => package_pin_key(n),
        }
    }

    pub fn kind_label(&self, live: &LiveNetlist) -> String {
        match self {
            DeviceSel::Cell(n) => live
                .cell(n)
                .map(|c| c.kind.label().to_string())
                .unwrap_or_else(|| "Cell".into()),
            DeviceSel::Net(_) => "Logical net".into(),
            DeviceSel::Pin(_) => "Package pin".into(),
        }
    }

    /// Tcl verb echo for the console, mirroring the schematic links.
    pub fn tcl(&self) -> String {
        match self {
            DeviceSel::Cell(n) => format!("select_objects [get_cells {n}]"),
            DeviceSel::Net(n) => format!("select_objects [get_nets {n}]"),
            DeviceSel::Pin(n) => format!("select_package_pin {n}"),
        }
    }
}

/// Device center-pane view: the placed die or the HAD package grid. Typed
/// so the toolbar toggle and `package` command cannot drift into strings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeviceView {
    #[default]
    Die,
    Package,
}

impl DeviceView {
    pub const ALL: [DeviceView; 2] = [DeviceView::Die, DeviceView::Package];

    pub fn label(self) -> &'static str {
        match self {
            DeviceView::Die => "Die",
            DeviceView::Package => "Package",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            DeviceView::Die => "device-view-die",
            DeviceView::Package => "device-view-package",
        }
    }
}

/// Die canvas layer visibility. Typed flags behind the layers panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceLayers {
    pub sites: bool,
    pub cells: bool,
    pub nets: bool,
    /// Requested regions visibility; always renders off (no region model).
    pub regions: bool,
}

impl Default for DeviceLayers {
    fn default() -> Self {
        Self {
            sites: true,
            cells: true,
            nets: false,
            regions: false,
        }
    }
}

/// Layer panel rows. Typed so toggles are never strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerId {
    Sites,
    Cells,
    Nets,
    Regions,
}

impl LayerId {
    pub const ALL: [LayerId; 4] = [
        LayerId::Sites,
        LayerId::Cells,
        LayerId::Nets,
        LayerId::Regions,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LayerId::Sites => "Site tiles",
            LayerId::Cells => "Placed cells",
            LayerId::Nets => "Net spans",
            LayerId::Regions => "Clock regions",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            LayerId::Sites => "device-layer-sites",
            LayerId::Cells => "device-layer-cells",
            LayerId::Nets => "device-layer-nets",
            LayerId::Regions => "device-layer-regions",
        }
    }

    /// The regions layer is unwirable: the HAD model exposes sites but no
    /// clock-region table, so it stays visibly disabled with a reason.
    pub fn enabled(self) -> bool {
        !matches!(self, LayerId::Regions)
    }

    pub fn disabled_reason(self) -> Option<&'static str> {
        match self {
            LayerId::Regions => Some("No HAD region table"),
            _ => None,
        }
    }

    pub fn get(self, layers: &DeviceLayers) -> bool {
        match self {
            LayerId::Sites => layers.sites,
            LayerId::Cells => layers.cells,
            LayerId::Nets => layers.nets,
            LayerId::Regions => layers.regions,
        }
    }

    pub fn toggle(self, layers: &mut DeviceLayers) {
        if !self.enabled() {
            return;
        }
        match self {
            LayerId::Sites => layers.sites = !layers.sites,
            LayerId::Cells => layers.cells = !layers.cells,
            LayerId::Nets => layers.nets = !layers.nets,
            LayerId::Regions => {}
        }
    }
}

/// Device-owned UI state: die/package view, zoom, shared cross-probe
/// selection, layer flags, find palette visibility, and the find filter edit.
#[derive(Clone, Debug)]
pub struct DeviceState {
    pub view: DeviceView,
    pub zoom: f32,
    pub selected: Option<DeviceSel>,
    pub layers: DeviceLayers,
    pub find_open: bool,
    pub find: rune::TextEdit,
    /// Port whose Package Pin popover is open (`None` closed; exclusive —
    /// see [`push_device_pops`]).
    pub io_pin_pop: Option<String>,
}

impl Default for DeviceState {
    fn default() -> Self {
        Self {
            view: DeviceView::Die,
            zoom: 1.0,
            selected: None,
            layers: DeviceLayers::default(),
            find_open: true,
            find: rune::TextEdit::new(""),
            io_pin_pop: None,
        }
    }
}

impl DeviceState {
    pub fn zoom_in(&mut self) {
        self.zoom = (self.zoom * ZOOM_STEP).min(ZOOM_MAX);
    }

    pub fn zoom_out(&mut self) {
        self.zoom = (self.zoom / ZOOM_STEP).max(ZOOM_MIN);
    }

    pub fn zoom_fit(&mut self) {
        self.zoom = 1.0;
    }

    pub fn zoom_label(&self) -> String {
        format!("{}%", (self.zoom * 100.0).round())
    }

    /// Cells mount only while their layer is on.
    pub fn cell_visible(&self) -> bool {
        self.layers.cells
    }

    /// Net spans draw only while their layer is on.
    pub fn net_visible(&self) -> bool {
        self.layers.nets
    }

    /// Find matches in stable order: live cells first, then live nets. The
    /// query matches cell names and real site ids plus net names,
    /// case-insensitively.
    pub fn matches(
        &self,
        query: &str,
        live: &LiveNetlist,
        placed: Option<&LivePlaced>,
    ) -> Vec<DeviceSel> {
        let q = query.to_lowercase();
        let mut out = Vec::new();
        for cell in &live.cells {
            let site = placed
                .and_then(|p| p.site_of(&cell.name))
                .map(|c| c.site.as_str())
                .unwrap_or("");
            if q.is_empty()
                || cell.name.to_lowercase().contains(&q)
                || (!site.is_empty() && site.to_lowercase().contains(&q))
            {
                out.push(DeviceSel::Cell(cell.name.clone()));
            }
        }
        for net in &live.nets {
            if q.is_empty() || net.name.to_lowercase().contains(&q) {
                out.push(DeviceSel::Net(net.name.clone()));
            }
        }
        out
    }
}

/// Package pin key: one stable button per HAD site id.
fn package_pin_key(pin: &str) -> String {
    format!("device-package-pin-{}", slug(pin))
}

/// One HAD IOB package pin: engine site plus the owning top-level port, if
/// any. Derived per frame from the live device and I/O rows (old
/// `PackagePin`, refreshed by `refresh_package`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackagePin {
    pub pin: String,
    pub x: u32,
    pub y: u32,
    pub port: Option<String>,
    pub bank: u32,
}

/// HAD IOB bounding box behind the package grid (old `PackageDrawing`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageGrid {
    pub part: String,
    pub x0: u32,
    pub y0: u32,
    pub cols: u32,
    pub rows: u32,
}

/// Package pins plus their bounding box: every HAD IOB site, with placed
/// sites first and LOC constraints filling unplaced ports (old
/// `refresh_package` uses each port's site, else its package pin). Banks
/// group eight consecutive IOBs along the grid's long axis.
pub fn package_pins(s: &AppState) -> (PackageGrid, Vec<PackagePin>) {
    let (dev, part) = crate::live::live_device(s);
    let assigned: Vec<(String, String)> = io_port_rows(s)
        .into_iter()
        .filter_map(|r| r.site.or(r.package_pin).map(|site| (site, r.name)))
        .collect();
    let mut pins: Vec<PackagePin> = dev
        .iob_sites()
        .map(|site| {
            let pin = site.id();
            let port = assigned
                .iter()
                .find(|(site, _)| site == &pin)
                .map(|(_, name)| name.clone());
            PackagePin {
                pin,
                x: site.x,
                y: site.y,
                port,
                bank: 0,
            }
        })
        .collect();
    let (x0, y0, cols, rows) = match (
        pins.iter().map(|p| p.x).min(),
        pins.iter().map(|p| p.x).max(),
        pins.iter().map(|p| p.y).min(),
        pins.iter().map(|p| p.y).max(),
    ) {
        (Some(xmin), Some(xmax), Some(ymin), Some(ymax)) => {
            (xmin, ymin, xmax - xmin + 1, ymax - ymin + 1)
        }
        _ => (0, 0, 0, 0),
    };
    const BANK_PINS: u32 = 8;
    for p in &mut pins {
        let dx = p.x.saturating_sub(x0);
        let dy = p.y.saturating_sub(y0);
        p.bank = if cols >= rows {
            dx / BANK_PINS
        } else {
            dy / BANK_PINS
        };
    }
    (
        PackageGrid {
            part,
            x0,
            y0,
            cols,
            rows,
        },
        pins,
    )
}

/// One package pin by exact engine id.
pub fn package_pin(s: &AppState, pin: &str) -> Option<PackagePin> {
    package_pins(s).1.into_iter().find(|p| p.pin == pin)
}

/// Package drawing dump: HAD bounding box plus the occupancy map, exactly
/// like old `package_drawing_text` (including its `L`/`P`/`.` marks).
pub fn package_grid_text(s: &AppState) -> String {
    let (grid, pins) = package_pins(s);
    let assigned = pins.iter().filter(|p| p.port.is_some()).count();
    let banks = {
        let mut seen = std::collections::BTreeSet::new();
        for p in &pins {
            seen.insert(p.bank);
        }
        seen.len()
    };
    let mut out = format!(
        "package drawing part={} cols={} rows={} pins={} assigned={} x0={} y0={} banks={}",
        grid.part,
        grid.cols,
        grid.rows,
        pins.len(),
        assigned,
        grid.x0,
        grid.y0,
        banks
    );
    if grid.cols == 0 || grid.rows == 0 {
        return out;
    }
    for dy in 0..grid.rows {
        let y = grid.y0 + dy;
        let mut row = format!(" row y={y}");
        let mut map = String::new();
        for dx in 0..grid.cols {
            let x = grid.x0 + dx;
            if let Some(p) = pins.iter().find(|p| p.x == x && p.y == y) {
                row.push_str(&format!(
                    " {}={}:bank={}",
                    p.pin,
                    p.port.as_deref().unwrap_or("-"),
                    p.bank
                ));
                map.push(match p.port.as_deref() {
                    Some("led") => 'L',
                    Some(_) => 'P',
                    None => '.',
                });
            } else {
                map.push(' ');
            }
        }
        out.push_str(&format!("{row} map={map}"));
    }
    out
}

/// Bank ink from theme tokens (no hardcoded package palette).
fn package_bank_color(t: &VivadoTheme, bank: u32) -> rune::Color {
    match bank % 4 {
        0 => t.die_region,
        1 => t.link,
        2 => t.pass,
        _ => t.warn,
    }
}

/// Echo a Tcl verb to the console without flipping run state.
fn echo(s: &mut AppState, tcl: String) {
    s.console.lines.push(ConsoleLine {
        text: tcl,
        sev: MsgSev::Info,
    });
}

/// Select a die element and cross-probe it to the console.
fn select(s: &mut AppState, sel: DeviceSel) {
    s.device.selected = Some(sel);
    echo(s, s.device.selected.clone().expect("just set").tcl());
}

/// Select the first find match (find input submit).
fn select_first_match(s: &mut AppState) {
    let query = s.device.find.text.clone();
    if let Some(live) = live_netlist(s) {
        let placed = live_placed(s);
        let hits = s.device.matches(&query, &live, placed.as_ref());
        if let Some(sel) = hits.first() {
            select(s, sel.clone());
        }
    }
}

fn ui_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text)
}

fn dim_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text_dim)
}

/// Small toolbar button with a text glyph (matches the schematic style).
fn tool_btn(
    key: &'static str,
    label: &'static str,
    glyph_text: &'static str,
    t: &VivadoTheme,
) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .size(22.0, 22.0)
        .center()
        .hover(t.selection)
        .child(
            text(glyph_text)
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text_dim)
                .a11y_hidden(),
        )
}

/// Die/Package view toggle button in the Device toolbar.
fn view_toggle(view: DeviceView, s: &AppState, t: &VivadoTheme) -> El {
    let active = s.device.view == view;
    el().key(view.key())
        .role("button")
        .label(format!("Show {} view", view.label()))
        .focus_selected(active)
        .focusable(true)
        .h(22.0)
        .px(8.0)
        .center()
        .fill(if active { t.selection } else { t.panel_bg })
        .hover(t.selection)
        .on_press(move |s: &mut AppState| s.device.view = view)
        .child(ui_label(view.label(), t))
}

/// Die/package toolbar. Zoom belongs only to the die canvas; the
/// package grid always fills its own viewport.
fn device_toolbar(s: &AppState, t: &VivadoTheme) -> El {
    let mut bar = row()
        .key("device-toolbar")
        .role("toolbar")
        .w_full()
        .h(TOOL_H)
        .items_center()
        .gap(2.0)
        .px(6.0)
        .fill(t.panel_bg);
    if s.device.view == DeviceView::Die {
        bar = bar
            .child(
                tool_btn("device-zoom-out", "Zoom out", "−", t)
                    .on_press(|s: &mut AppState| s.device.zoom_out()),
            )
            .child(
                el().key("device-zoom-label")
                    .role("status")
                    .label(format!("Zoom {}", s.device.zoom_label()))
                    .w(44.0)
                    .center()
                    .child(ui_label(&s.device.zoom_label(), t)),
            )
            .child(
                tool_btn("device-zoom-in", "Zoom in", "+", t)
                    .on_press(|s: &mut AppState| s.device.zoom_in()),
            )
            .child(
                el().key("device-zoom-fit")
                    .role("button")
                    .label("Zoom to fit")
                    .focusable(true)
                    .h(22.0)
                    .px(6.0)
                    .center()
                    .hover(t.selection)
                    .on_press(|s: &mut AppState| s.device.zoom_fit())
                    .child(ui_label("Fit", t)),
            );
    }
    bar.child(view_toggle(DeviceView::Die, s, t))
        .child(view_toggle(DeviceView::Package, s, t))
        .child(el().grow(1.0))
        .child(
            el().key("device-find-toggle")
                .role("button")
                .label(if s.device.find_open {
                    "Hide find palette"
                } else {
                    "Show find palette"
                })
                .focusable(true)
                .h(22.0)
                .px(6.0)
                .center()
                .hover(t.selection)
                .on_press(|s: &mut AppState| s.device.find_open = !s.device.find_open)
                .child(ui_label(
                    if s.device.find_open {
                        "Hide Find"
                    } else {
                        "Find"
                    },
                    t,
                )),
        )
}

/// Layers panel: one checkbox per die layer; unwirable layers mount disabled
/// with their reason.
fn layers_panel(s: &AppState, t: &VivadoTheme) -> El {
    let kit = t.kit();
    let mut body = col()
        .key("device-layers")
        .role("group")
        .label("Die layers")
        .w(LAYERS_W)
        .h_full()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            row()
                .w_full()
                .h(22.0)
                .items_center()
                .px(8.0)
                .fill(t.header_bg)
                .child(text("Layers").font(VivadoTheme::FONT_TITLE).color(t.text)),
        );
    for layer in LayerId::ALL {
        let mut check = Checkbox::new(layer.get(&s.device.layers))
            .label(layer.label())
            .disabled(!layer.enabled())
            .on_toggle::<AppState>(move |s: &mut AppState| layer.toggle(&mut s.device.layers));
        if let Some(reason) = layer.disabled_reason() {
            check = check.description(reason);
        }
        body = body.child(check.build(&kit).key(layer.key()));
    }
    body = body.child(
        el().w_full()
            .px(8.0)
            .py(4.0)
            .child(dim_label("Toggle die layers", t)),
    );
    body
}

/// One real site tile rect at 100%: `[x, y, 1, 1]` in site units.
fn tile_rect(geom: &LiveDieGeom, x: u32, y: u32) -> [f32; 4] {
    let [ox, oy] = site_xy(geom, x, y);
    [ox, oy, TILE_W, TILE_H]
}

/// Die shape for the canvas: real HAD site tiles, special columns, die
/// frame, and net spans between real placed endpoints (red when touching
/// the selection).
fn die_shape(
    s: &AppState,
    t: &VivadoTheme,
    geom: &LiveDieGeom,
    live: Option<&LiveNetlist>,
    placed: Option<&LivePlaced>,
) -> Shape {
    let z = s.device.zoom;
    let ww = world_w(geom);
    let wh = world_h(geom);
    let mut shape = Shape::new();
    // Site tiles from the real part geometry (mirrors the Device iterators:
    // CLB grid, bottom IOB row, CLK spine, DSP/BRAM columns).
    if s.device.layers.sites {
        for dy in 0..geom.interior_rows {
            for dx in 0..geom.interior_cols {
                let [ox, oy, w, h] = tile_rect(geom, geom.clb_x0 + dx, geom.clb_y0 + dy);
                let (x, y, w, h) = (ox * z, oy * z, w * z, h * z);
                shape = shape.fill(
                    vec![
                        [x + 1.0, y + 1.0],
                        [x + w - 1.0, y + 1.0],
                        [x + w - 1.0, y + h - 1.0],
                        [x + 1.0, y + h - 1.0],
                    ],
                    t.die_tile,
                );
                shape = shape.stroke(
                    vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
                    1.0,
                    t.die_grid,
                );
            }
        }
        // Bottom IOB row under each CLB column.
        let iob_y = geom.clb_y0.saturating_sub(1);
        for dx in 0..geom.interior_cols {
            shape = special_tile(shape, geom, z, geom.clb_x0 + dx, iob_y, t.die_tile, t.pass);
        }
        // CLK spine, DSP and BRAM columns (outlines over the CLB grid).
        for dy in 0..geom.interior_rows {
            shape = outline_tile(
                shape,
                geom,
                z,
                geom.clk_spine,
                geom.clb_y0 + dy,
                t.die_region,
            );
        }
        for i in 0..geom.n_dsp {
            shape = outline_tile(shape, geom, z, geom.dsp_x, geom.dsp_y0 + i * 2, t.link);
        }
        for i in 0..geom.n_bram {
            shape = outline_tile(shape, geom, z, geom.bram_x, geom.bram_y0 + i * 2, t.warn);
        }
    }
    // Die frame.
    shape = shape.stroke(
        vec![
            [0.5, 0.5],
            [ww * z - 0.5, 0.5],
            [ww * z - 0.5, wh * z - 0.5],
            [0.5, wh * z - 0.5],
        ],
        1.0,
        t.die_grid,
    );
    // Net spans between real placed endpoints.
    if s.device.net_visible() {
        if let (Some(live), Some(placed)) = (live, placed) {
            for net in &live.nets {
                let ends: Vec<&str> = net
                    .cells
                    .iter()
                    .map(String::as_str)
                    .filter(|c| placed.site_of(c).is_some())
                    .collect();
                if ends.len() < 2 {
                    continue;
                }
                let hot = match &s.device.selected {
                    Some(DeviceSel::Net(n)) => *n == net.name,
                    Some(DeviceSel::Cell(c)) => s.device.layers.cells && ends.contains(&c.as_str()),
                    Some(DeviceSel::Pin(_)) | None => false,
                };
                let driver = placed.site_of(ends[0]).expect("filtered");
                let [dx, dy] = site_xy(geom, driver.site_x, driver.site_y);
                let x0 = dx + TILE_W / 2.0;
                let y0 = dy + TILE_H / 2.0;
                for sink in &ends[1..] {
                    let ps = placed.site_of(sink).expect("filtered");
                    let [sx, sy] = site_xy(geom, ps.site_x, ps.site_y);
                    let x1 = sx + TILE_W / 2.0;
                    let y1 = sy + TILE_H / 2.0;
                    let mx = (x0 + x1) / 2.0;
                    shape = shape.stroke(
                        vec![
                            [x0 * z, y0 * z],
                            [mx * z, y0 * z],
                            [mx * z, y1 * z],
                            [x1 * z, y1 * z],
                        ],
                        if hot { 2.5 } else { 1.5 },
                        if hot { t.fail } else { t.die_net },
                    );
                }
            }
        }
    }
    shape
}

fn outline_tile(
    shape: Shape,
    geom: &LiveDieGeom,
    z: f32,
    x: u32,
    y: u32,
    ink: rune::Color,
) -> Shape {
    if x < geom.x0 || y < geom.y0 {
        return shape;
    }
    let [ox, oy, w, h] = tile_rect(geom, x, y);
    let (x, y, w, h) = (ox * z, oy * z, w * z, h * z);
    shape.stroke(
        vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
        1.5,
        ink,
    )
}

fn special_tile(
    shape: Shape,
    geom: &LiveDieGeom,
    z: f32,
    x: u32,
    y: u32,
    fill: rune::Color,
    ink: rune::Color,
) -> Shape {
    if x < geom.x0 || y < geom.y0 {
        return shape;
    }
    let [ox, oy, w, h] = tile_rect(geom, x, y);
    let (x, y, w, h) = (ox * z, oy * z, w * z, h * z);
    shape
        .fill(
            vec![
                [x + 1.0, y + 1.0],
                [x + w - 1.0, y + 1.0],
                [x + w - 1.0, y + h - 1.0],
                [x + 1.0, y + h - 1.0],
            ],
            fill,
        )
        .stroke(
            vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
            1.5,
            ink,
        )
}

/// World rect for a placed cell: site-mates share the tile in deterministic
/// 2x2 quads so every packed cell stays tappable.
fn placed_rect(geom: &LiveDieGeom, placed: &LivePlaced, cell: &str) -> Option<[f32; 4]> {
    let here = placed.site_of(cell)?;
    let mates: Vec<&str> = placed
        .cells
        .iter()
        .filter(|c| c.site_x == here.site_x && c.site_y == here.site_y)
        .map(|c| c.cell.as_str())
        .collect();
    let idx = mates.iter().position(|m| *m == cell).unwrap_or(0);
    let [ox, oy] = site_xy(geom, here.site_x, here.site_y);
    if mates.len() <= 1 {
        return Some([ox + 3.0, oy + 3.0, TILE_W - 6.0, TILE_H - 6.0]);
    }
    let qw = (TILE_W - 6.0) / 2.0;
    let qh = (TILE_H - 6.0) / 2.0;
    let qx = (idx % 2) as f32;
    let qy = ((idx / 2) % 2) as f32;
    let j = (idx / 4) as f32 * 2.0;
    Some([
        ox + 3.0 + qx * qw + j,
        oy + 3.0 + qy * qh + j,
        qw - 2.0,
        qh - 2.0,
    ])
}

/// One canvas cell node: an absolutely positioned keyed block on its real site tile.
fn cell_node(
    s: &AppState,
    t: &VivadoTheme,
    geom: &LiveDieGeom,
    placed: &LivePlaced,
    live: &LiveNetlist,
    name: &str,
) -> Option<El> {
    let [x, y, w, h] = placed_rect(geom, placed, name)?;
    let z = s.device.zoom;
    let owned = name.to_string();
    let sel = DeviceSel::Cell(owned.clone());
    let selected = s.device.selected.as_ref() == Some(&sel);
    let site = placed.site_of(name).map(|c| c.site.as_str()).unwrap_or("?");
    let kind = live.cell(name).map(|c| c.kind.label()).unwrap_or("Cell");
    let mut node = el()
        .key(sel.key())
        .role("button")
        .label(format!("{name} ({kind} at {site})"))
        .focus_selected(selected)
        .focusable(true)
        .absolute(x * z, y * z)
        .size(w * z, h * z)
        .fill(t.die_cell)
        .border(
            if selected { 2.0 } else { 1.0 },
            if selected { t.fail } else { t.text },
        )
        .on_press(move |s: &mut AppState| select(s, DeviceSel::Cell(owned.clone())));
    // Name labels only at close zoom, like the Vivado die.
    if z >= 2.0 {
        node = node.child(
            col()
                .w_full()
                .h_full()
                .center()
                .child(text(name).font(FONT_PIN).color(t.text)),
        );
    }
    Some(node)
}

/// The pan/zoom die canvas: a scroll container over the zoomed sheet with
/// the tile/span shape painted under the live cell nodes. Pressing empty die
/// space clears the cross-probe selection. Without a design or placement the
/// canvas keeps its chrome and shows the reason instead of cells.
fn device_canvas(
    s: &AppState,
    t: &VivadoTheme,
    geom: &LiveDieGeom,
    live: Option<&LiveNetlist>,
    placed: Option<&LivePlaced>,
) -> El {
    let z = s.device.zoom;
    let ww = world_w(geom);
    let wh = world_h(geom);
    let mut sheet = el()
        .w(ww * z)
        .h(wh * z)
        .fill(t.die_bg)
        .on_press(|s: &mut AppState| s.device.selected = None)
        .child(
            canvas(die_shape(s, t, geom, live, placed))
                .w_full()
                .h_full(),
        );
    match (live, placed) {
        (Some(live), Some(placed)) if s.device.cell_visible() => {
            for cell in &live.cells {
                if let Some(node) = cell_node(s, t, geom, placed, live, &cell.name) {
                    sheet = sheet.child(node);
                }
            }
        }
        (Some(_), None) => {
            sheet = sheet.child(
                col()
                    .key("device-unplaced")
                    .role("status")
                    .absolute(12.0 * z, 12.0 * z)
                    .gap(4.0)
                    .child(
                        text("Design not placed")
                            .font(VivadoTheme::FONT_TITLE)
                            .color(t.banner_ink),
                    )
                    .child(
                        text("Run launch_runs impl_1 to place it — sites stay empty until then.")
                            .font(VivadoTheme::FONT_UI)
                            .color(t.banner_ink),
                    ),
            );
        }
        (None, _) => {
            sheet = sheet.child(
                col()
                    .key("device-empty")
                    .role("status")
                    .absolute(12.0 * z, 12.0 * z)
                    .gap(4.0)
                    .child(
                        text("No elaborated design")
                            .font(VivadoTheme::FONT_TITLE)
                            .color(t.banner_ink),
                    )
                    .child(
                        text("Run launch_runs synth_1, synth_design, or read_sv to elaborate RTL.")
                            .font(VivadoTheme::FONT_UI)
                            .color(t.banner_ink),
                    ),
            );
        }
        _ => {}
    }
    sheet = sheet.child(
        text(format!("{} · Device", geom.part))
            .font(FONT_PIN)
            .color(t.banner_ink)
            .key("device-die-label")
            .absolute(8.0 * z, (wh - 18.0) * z),
    );
    col()
        .key("device-canvas")
        .role("application")
        .label("FPGA device canvas")
        .items_start()
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.die_bg)
        .border(1.0, t.border)
        .scroll_xy()
        .child(sheet)
}

/// One find result row: tap to cross-probe the element.
fn find_row(
    s: &AppState,
    t: &VivadoTheme,
    index: usize,
    sel: DeviceSel,
    live: &LiveNetlist,
    placed: Option<&LivePlaced>,
) -> El {
    let selected = s.device.selected.as_ref() == Some(&sel);
    let detail = match &sel {
        DeviceSel::Cell(n) => placed
            .and_then(|p| p.site_of(n))
            .map(|c| c.site.clone())
            .unwrap_or_else(|| "unplaced".into()),
        DeviceSel::Net(n) => live
            .net(n)
            .map(|w| format!("net · {} loads", w.loads))
            .unwrap_or_else(|| "net".into()),
        DeviceSel::Pin(n) => package_pin(s, n)
            .map(|p| format!("BANK{} · X{}Y{}", p.bank, p.x, p.y))
            .unwrap_or_else(|| "stale selection (device changed)".into()),
    };
    let tap = sel.clone();
    let name = sel.name().to_string();
    let label = format!("{} ({})", sel.name(), sel.kind_label(live));
    row()
        .key(format!("device-find-row-{index}"))
        .role("option")
        .label(label)
        .focus_selected(selected)
        .focusable(true)
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(6.0)
        .px(8.0)
        .fill(if selected { t.selection } else { t.panel_bg })
        .hover(t.selection)
        .on_press(move |s: &mut AppState| select(s, tap.clone()))
        .child(ui_label(&name, t))
        .child(el().grow(1.0))
        .child(dim_label(&detail, t))
}

/// Find palette: live filter over real cells+nets with result rows.
fn find_palette(
    s: &AppState,
    t: &VivadoTheme,
    live: Option<&LiveNetlist>,
    placed: Option<&LivePlaced>,
) -> El {
    let query = s.device.find.text.clone();
    let hits: Vec<DeviceSel> = live
        .map(|live| s.device.matches(&query, live, placed))
        .unwrap_or_default();
    let input: &rune::TextEdit = &s.device.find;
    let body = col()
        .key("device-find")
        .role("search")
        .label("Find die elements")
        .w_full()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            row()
                .w_full()
                .h(22.0)
                .items_center()
                .gap(4.0)
                .px(8.0)
                .fill(t.header_bg)
                .child(icon(&glyphs::SEARCH, 12.0).color(t.text_dim).a11y_hidden())
                .child(text("Find").font(VivadoTheme::FONT_TITLE).color(t.text))
                .child(el().grow(1.0))
                .child(dim_label(&format!("{} matches", hits.len()), t).key("device-find-count")),
        )
        .child(
            text_input(input, VivadoTheme::FONT_UI)
                .single_line()
                .key("device-find-input")
                .label("Find cells or nets")
                .w_full()
                .color(t.text)
                .placeholder("Find cells or nets", t.text_faint)
                .caret(t.text, t.selection)
                .on_edit(|s: &mut AppState| &mut s.device.find)
                .on_submit(|s: &mut AppState, _tx: &mut Tx| select_first_match(s)),
        );
    let mut list = col().w_full().h(FIND_H).scroll_xy();
    if hits.is_empty() {
        list = list.child(
            row().w_full().h(ROW_H).items_center().px(8.0).child(
                dim_label(
                    if live.is_none() {
                        "No elaborated design"
                    } else {
                        "No matches"
                    },
                    t,
                )
                .key("device-find-empty"),
            ),
        );
    } else if let Some(live) = live {
        for (n, sel) in hits.iter().enumerate() {
            list = list.child(find_row(s, t, n, sel.clone(), live, placed));
        }
    }
    body.child(list)
}

/// Element inspector: the cross-probed selection readout over live data.
fn device_inspector(
    s: &AppState,
    t: &VivadoTheme,
    live: Option<&LiveNetlist>,
    placed: Option<&LivePlaced>,
) -> El {
    let body = col()
        .key("device-inspector")
        .role("status")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            row()
                .w_full()
                .h(22.0)
                .items_center()
                .px(8.0)
                .fill(t.header_bg)
                .child(
                    text("Element Inspector")
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text),
                ),
        );
    let empty_hint = match (live, &s.device.selected) {
        (None, _) => Some("No elaborated design — run synth_1"),
        (Some(_), None) => Some("Click a cell, net, or find row"),
        _ => None,
    };
    if let Some(hint) = empty_hint {
        return body.child(
            row()
                .w_full()
                .h(ROW_H)
                .items_center()
                .px(8.0)
                .child(dim_label(hint, t)),
        );
    }
    let (Some(sel), Some(live)) = (&s.device.selected, live) else {
        return body;
    };
    let (kind, site, detail) = match sel {
        DeviceSel::Cell(n) => (
            sel.kind_label(live),
            placed
                .and_then(|p| p.site_of(n))
                .map(|c| c.site.clone())
                .unwrap_or_else(|| "unplaced".into()),
            match (live.cell(n), placed.and_then(|p| p.site_of(n))) {
                (Some(c), Some(p)) => format!("BEL {} · fanout {}", p.bel, c.fanout),
                (Some(c), None) => format!("unplaced · fanout {} — run impl_1", c.fanout),
                _ => "stale selection (design changed)".into(),
            },
        ),
        DeviceSel::Net(n) => (
            "Logical net".to_string(),
            match live.net(n) {
                Some(w) => {
                    let p = placed
                        .map(|p| w.cells.iter().filter(|c| p.site_of(c).is_some()).count())
                        .unwrap_or(0);
                    format!("{p}/{} endpoints placed", w.cells.len())
                }
                None => "stale selection (design changed)".into(),
            },
            match live.net(n) {
                Some(w) => format!("{} loads · endpoint spans", w.loads),
                None => "stale selection (design changed)".into(),
            },
        ),
        DeviceSel::Pin(n) => match package_pin(s, n) {
            Some(p) => (
                "Package pin".to_string(),
                format!("BANK{} · X{}Y{}", p.bank, p.x, p.y),
                p.port
                    .map(|port| format!("Assigned to {port}"))
                    .unwrap_or_else(|| "Unassigned".into()),
            ),
            None => (
                "Package pin".to_string(),
                "stale selection (device changed)".into(),
                "stale selection (device changed)".into(),
            ),
        },
    };
    body.child(
        row().w_full().h(ROW_H).items_center().px(8.0).child(
            text(sel.name())
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text)
                .key("device-inspector-name"),
        ),
    )
    .child(
        row()
            .w_full()
            .h(ROW_H)
            .items_center()
            .px(8.0)
            .child(dim_label(&kind, t).key("device-inspector-kind")),
    )
    .child(
        row()
            .w_full()
            .h(ROW_H)
            .items_center()
            .px(8.0)
            .child(dim_label(&site, t).key("device-inspector-site")),
    )
    .child(
        row()
            .w_full()
            .h(ROW_H)
            .items_center()
            .px(8.0)
            .child(dim_label(&detail, t).key("device-inspector-detail")),
    )
    .child(
        el().key("device-locate")
            .role("button")
            .label("Locate selection in Sources")
            .focusable(true)
            .h(ROW_H)
            .px(8.0)
            .center_y()
            .hover(t.selection)
            .on_press(|s: &mut AppState| {
                s.sources.selected = SourceFileId::Demo;
                echo(s, "select_objects [get_files project1_demo.v]".into());
            })
            .child(
                text("Locate in Sources")
                    .font(VivadoTheme::FONT_UI)
                    .color(t.link),
            ),
    )
}

/// Status strip: real part plus live placement/net counts.
fn device_status(
    s: &AppState,
    t: &VivadoTheme,
    geom: &LiveDieGeom,
    live: Option<&LiveNetlist>,
    placed: Option<&LivePlaced>,
) -> El {
    let text = match (live, placed) {
        (Some(live), Some(placed)) => format!(
            "{} · {} of {} cells placed · {} nets · {}",
            geom.part,
            placed.cells.len(),
            live.cells.len(),
            if s.device.layers.nets {
                live.nets.len()
            } else {
                0
            },
            s.device.zoom_label(),
        ),
        (Some(live), None) => format!(
            "{} · {} cells unplaced (run impl_1) · {} nets · {}",
            geom.part,
            live.cells.len(),
            live.nets.len(),
            s.device.zoom_label(),
        ),
        (None, _) => format!(
            "{} · No elaborated design — run launch_runs synth_1",
            geom.part
        ),
    };
    row()
        .key("device-status")
        .role("status")
        .w_full()
        .h(22.0)
        .items_center()
        .gap(8.0)
        .px(8.0)
        .fill(t.header_bg)
        .child(dim_label(&text, t))
}

/// Package pin cell size: site id plus owner, dense enough to wrap the
/// HAD strip into a filled grid.
const PACKAGE_PIN_W: f32 = 104.0;
const PACKAGE_PIN_H: f32 = 64.0;
const PACKAGE_BALL: f32 = 18.0;

/// Package pin tap: an empty pin binds the selected I/O port through real
/// `set_property PACKAGE_PIN`; any other tap routes through real
/// `select_package_pin` (old paint's click branch).
fn package_pick(s: &mut AppState, pin: &str) {
    let Some(found) = package_pin(s, pin) else {
        return;
    };
    if found.port.is_none()
        && let Some(DeviceSel::Net(port)) = s.device.selected.clone()
        && io_port_rows(s).iter().any(|r| r.name == port)
    {
        let before = s.console.lines.len();
        crate::tcl::eval_command(
            s,
            &format!("set_property PACKAGE_PIN {pin} [get_ports {port}]"),
        );
        let failed = s.console.lines[before..]
            .iter()
            .any(|l| l.sev == MsgSev::Error);
        if !failed {
            s.device.selected = Some(DeviceSel::Net(port));
        }
        return;
    }
    crate::tcl::eval_command(s, &format!("select_package_pin {pin}"));
}

/// One tappable package ball: bank wash, assigned ink, selected ring, site
/// id plus owner (no hover-only state).
fn package_pin_cell(s: &AppState, t: &VivadoTheme, pin: &PackagePin) -> El {
    let assigned = pin.port.is_some();
    let selected = match &s.device.selected {
        Some(DeviceSel::Pin(n)) => n == &pin.pin,
        Some(DeviceSel::Net(n)) => pin.port.as_deref() == Some(n.as_str()),
        _ => false,
    };
    let owned = pin.pin.clone();
    let status = pin.port.clone().unwrap_or_else(|| "Unassigned".into());
    el().key(package_pin_key(&pin.pin))
        .role("button")
        .label(format!("{} ({}, BANK{})", pin.pin, status, pin.bank))
        .description(format!("X{}Y{}", pin.x, pin.y))
        .focus_selected(selected)
        .focusable(true)
        .size(PACKAGE_PIN_W, PACKAGE_PIN_H)
        .fill(package_bank_color(t, pin.bank).alpha(0.16))
        .border(
            if selected { 2.0 } else { 1.0 },
            if selected { t.warn } else { t.border },
        )
        .radius(8.0)
        .hover(t.selection)
        .on_press(move |s: &mut AppState| package_pick(s, &owned))
        .child(
            col()
                .w_full()
                .h_full()
                .center()
                .gap(2.0)
                .child(
                    el().size(PACKAGE_BALL, PACKAGE_BALL)
                        .radius(PACKAGE_BALL / 2.0)
                        .fill(if assigned { t.link } else { t.text_faint })
                        .a11y_hidden(),
                )
                .child(
                    text(&pin.pin)
                        .font(FONT_PIN)
                        .color(t.text)
                        .truncate()
                        .a11y_hidden(),
                )
                .child(
                    text(&status)
                        .font(FONT_PIN)
                        .color(if assigned { t.text } else { t.text_dim })
                        .truncate()
                        .a11y_hidden(),
                ),
        )
}

/// Package header: part/grid/pin counts plus the live assignment target.
fn package_summary(s: &AppState, t: &VivadoTheme, grid: &PackageGrid, pins: &[PackagePin]) -> El {
    let assigned = pins.iter().filter(|p| p.port.is_some()).count();
    let mut head = col().key("device-package-summary").w_full().gap(2.0).child(
        row()
            .w_full()
            .items_center()
            .gap(8.0)
            .px(8.0)
            .py(4.0)
            .child(dim_label(
                &format!(
                    "Part {} · {}×{} grid · {} pins ({} assigned)",
                    grid.part,
                    grid.cols,
                    grid.rows,
                    pins.len(),
                    assigned
                ),
                t,
            )),
    );
    if let Some(DeviceSel::Net(port)) = &s.device.selected
        && io_port_rows(s).iter().any(|r| &r.name == port)
    {
        head = head.child(
            dim_label(&format!("Assigning {port} — tap an empty pin"), t)
                .key("device-package-assigning")
                .px(8.0),
        );
    }
    head
}

/// Package legend: occupancy inks plus every bank present on this part.
fn package_legend(t: &VivadoTheme, pins: &[PackagePin]) -> El {
    let mut legend = row()
        .key("device-package-legend")
        .w_full()
        .items_center()
        .gap(8.0)
        .px(8.0)
        .py(2.0);
    for (label, ink) in [("Unassigned", t.text_faint), ("Assigned port", t.link)] {
        legend = legend.child(
            row()
                .items_center()
                .gap(4.0)
                .child(el().size(10.0, 10.0).radius(5.0).fill(ink).a11y_hidden())
                .child(dim_label(label, t)),
        );
    }
    let mut banks = std::collections::BTreeSet::new();
    for p in pins {
        banks.insert(p.bank);
    }
    for bank in banks {
        legend = legend.child(
            row()
                .key(format!("device-package-legend-bank-{bank}"))
                .items_center()
                .gap(4.0)
                .child(
                    el().size(12.0, 12.0)
                        .radius(2.0)
                        .fill(package_bank_color(t, bank))
                        .a11y_hidden(),
                )
                .child(dim_label(&format!("BANK{bank}"), t)),
        );
    }
    legend
}

/// Package view: old HAD IOB drawing as tappable bank-colored balls over a
/// scroll viewport. Empty only if the part exposes no IOB sites.
fn package_pane(s: &AppState, t: &VivadoTheme) -> El {
    let (grid, pins) = package_pins(s);
    let pane = col()
        .key("device-package")
        .role("application")
        .label("Package pin grid")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Package", "device-package-pane", t))
        .child(package_summary(s, t, &grid, &pins))
        .child(package_legend(t, &pins));
    if pins.is_empty() {
        return pane.child(
            dim_label("This part exposes no IOB sites.", t)
                .key("device-package-empty")
                .px(8.0),
        );
    }
    let mut balls = row()
        .key("device-package-grid")
        .w_full()
        .flex_wrap()
        .gap(6.0)
        .padding(8.0);
    for pin in &pins {
        balls = balls.child(package_pin_cell(s, t, pin));
    }
    pane.child(
        col()
            .w_full()
            .grow(1.0)
            .min_w(0.0)
            .min_h(0.0)
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            })
            .child(balls),
    )
}

/// One I/O Planning row: a top-level port, its placed IOB site (once
/// implementation runs), and its `set_property` pin constraints (old
/// `IoPortView`; the Package Pin cell edits LOC through a popover).
#[derive(Clone, Debug)]
pub struct IoPortRow {
    pub name: String,
    pub dir: &'static str,
    pub site: Option<String>,
    pub iob_cell: Option<String>,
    pub package_pin: Option<String>,
    pub iostandard: Option<String>,
    pub drive: Option<String>,
    pub slew: Option<String>,
    pub pulltype: Option<String>,
    pub diff_term: Option<String>,
    pub in_term: Option<String>,
}

impl IoPortRow {
    fn cell(v: &Option<String>) -> &str {
        v.as_deref().unwrap_or("-")
    }
}

/// I/O port rows from the live design: top-level ports, the placed IOB
/// site on each port's net, and the port's pin-constraint attrs. Empty
/// until a design is synthesized.
///
/// The site match finds the placed IOB-kind cell on the port's net (old
/// `refresh_io_ports` matched packed-cell names by substring, with a
/// first-IOB fallback for outputs that could attribute a wrong site;
/// unmapped ports honestly show `-`).
pub fn io_port_rows(s: &AppState) -> Vec<IoPortRow> {
    let Some(design) = crate::live::live_design(s) else {
        return Vec::new();
    };
    let nets = crate::live::live_netlist(s)
        .map(|n| n.nets)
        .unwrap_or_default();
    let placed = crate::live::live_placed(s)
        .map(|p| p.cells)
        .unwrap_or_default();
    design
        .ports
        .iter()
        .map(|p| {
            let name = helion_ir::resolve(p.name).to_string();
            let dir = match p.dir {
                helion_ir::PortDir::In => "IN",
                helion_ir::PortDir::Out => "OUT",
                helion_ir::PortDir::Inout => "INOUT",
            };
            let on_net: &[String] = nets
                .iter()
                .find(|n| n.name == name)
                .map(|n| n.cells.as_slice())
                .unwrap_or(&[]);
            let iob = placed
                .iter()
                .find(|c| c.site_kind == "IOB" && on_net.iter().any(|n| n == &c.cell));
            let attr = |k: &str| p.attrs.get(k).map(str::to_string);
            IoPortRow {
                name,
                dir,
                site: iob.map(|c| c.site.clone()),
                iob_cell: iob.map(|c| c.cell.clone()),
                package_pin: attr("LOC"),
                iostandard: attr("IOSTANDARD"),
                drive: attr("DRIVE"),
                slew: attr("SLEW"),
                pulltype: attr("PULLTYPE"),
                diff_term: attr("DIFF_TERM"),
                in_term: attr("IN_TERM"),
            }
        })
        .collect()
}

/// I/O Ports planning table under the die canvas (old Package tab's port
/// grid, read-only): name/dir/site/pin + pin constraints. Row taps
/// cross-probe the die (placed IOB cell, else the port net).
fn io_ports_table(s: &AppState, t: &VivadoTheme) -> El {
    let rows = io_port_rows(s);
    let mut body = col().w_full();
    body = body.child(
        row()
            .key("device-io-head")
            .w_full()
            .items_center()
            .px(8.0)
            .child(el().grow(1.0).min_w(0.0).child(dim_label("Name", t)))
            .child(el().w(44.0).child(dim_label("Dir", t)))
            .child(el().w(96.0).child(dim_label("Site", t)))
            .child(el().w(128.0).child(dim_label("Package Pin", t)))
            .child(el().w(96.0).child(dim_label("IOSTANDARD", t)))
            .child(el().w(56.0).child(dim_label("Drive", t)))
            .child(el().w(56.0).child(dim_label("Slew", t)))
            .child(el().w(72.0).child(dim_label("Pull Type", t)))
            .child(el().w(64.0).child(dim_label("Diff Term", t)))
            .child(el().w(96.0).child(dim_label("In Term", t))),
    );
    if rows.is_empty() {
        body = body.child(
            dim_label("Synth a design to list ports.", t)
                .key("device-io-empty")
                .px(8.0),
        );
    }
    for (i, r) in rows.iter().enumerate() {
        let sel = match (&r.iob_cell, s.device.selected.as_ref()) {
            (Some(cell), Some(DeviceSel::Cell(n))) => n == cell,
            (None, Some(DeviceSel::Net(n))) => n == &r.name,
            _ => false,
        };
        let pick = match r.iob_cell.clone() {
            Some(cell) => DeviceSel::Cell(cell),
            None => DeviceSel::Net(r.name.clone()),
        };
        let mut line = row()
            .key(format!("device-io-row-{i}"))
            .role("row")
            .w_full()
            .h(22.0)
            .items_center()
            .px(8.0)
            .hover(t.selection)
            .on_press(move |s: &mut AppState| {
                s.device.selected = Some(pick.clone());
            })
            .child(
                el().grow(1.0)
                    .min_w(0.0)
                    .child(ui_label(&r.name, t).truncate()),
            )
            .child(el().w(44.0).child(ui_label(r.dir, t)))
            .child(el().w(96.0).child(mono_cell(IoPortRow::cell(&r.site), t)))
            .child(
                el().w(128.0)
                    .child(pin_trigger(&r.name, IoPortRow::cell(&r.package_pin), t)),
            )
            .child(
                el().w(96.0)
                    .child(mono_cell(IoPortRow::cell(&r.iostandard), t)),
            )
            .child(el().w(56.0).child(mono_cell(IoPortRow::cell(&r.drive), t)))
            .child(el().w(56.0).child(mono_cell(IoPortRow::cell(&r.slew), t)))
            .child(
                el().w(72.0)
                    .child(mono_cell(IoPortRow::cell(&r.pulltype), t)),
            )
            .child(
                el().w(64.0)
                    .child(mono_cell(IoPortRow::cell(&r.diff_term), t)),
            )
            .child(
                el().w(96.0)
                    .child(mono_cell(IoPortRow::cell(&r.in_term), t)),
            );
        if sel {
            line = line.fill(t.selection);
        }
        body = body.child(line);
    }
    col()
        .key("device-io-ports")
        .w_full()
        .max_h(190.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .px(8.0)
                .py(4.0)
                .child(ui_label("I/O Ports", t))
                .child(dim_label(&format!("{} ports", rows.len()), t)),
        )
        .child(
            col()
                .w_full()
                .max_h(156.0)
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(body),
        )
}

fn mono_cell(content: &str, t: &VivadoTheme) -> El {
    text(content)
        .font(VivadoTheme::FONT_MONO.mono())
        .color(t.text)
        .truncate()
}

/// Package Pin options: every IOB die site `(id, label)`, annotated with
/// the owning port when one is assigned (old combo's `{pin} ({port})`).
fn pin_options(s: &AppState) -> Vec<(String, String)> {
    let (dev, _) = crate::live::live_device(s);
    let rows = io_port_rows(s);
    dev.iob_sites()
        .map(|site| {
            let id = site.id();
            let owner = rows.iter().find_map(|r| {
                if r.package_pin.as_deref() == Some(id.as_str())
                    || r.site.as_deref() == Some(id.as_str())
                {
                    Some(r.name.clone())
                } else {
                    None
                }
            });
            let label = owner
                .map(|port| format!("{id} ({port})"))
                .unwrap_or_else(|| id.clone());
            (id, label)
        })
        .collect()
}

/// Apply a Package Pin pick: the same `set_property LOC` the old combo
/// execs, then close the popover.
fn choose_pin(s: &mut AppState, port: &str, site: &str) {
    crate::tcl::eval_command(s, &format!("set_property LOC {site} {port}"));
    s.device.io_pin_pop = None;
    s.device.selected = Some(DeviceSel::Net(port.to_string()));
}

/// Package Pin combo trigger cell (built identically in the table and in
/// [`push_device_pops`]; the popover anchors to its key).
fn pin_trigger(port: &str, current: &str, t: &VivadoTheme) -> El {
    let name = port.to_string();
    el().key(format!("device-io-pin-{}", slug(port)))
        .role("button")
        .label(format!("Package pin for {port}, current {current}"))
        .focusable(true)
        .w_full()
        .h(22.0)
        .px(6.0)
        .center_y()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .radius(4.0)
        .hover(t.selection)
        .on_press(move |s: &mut AppState| {
            s.device.io_pin_pop = if s.device.io_pin_pop.as_deref() == Some(&name) {
                None
            } else {
                Some(name.clone())
            };
        })
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .gap(4.0)
                .child(mono_cell(current, t).grow(1.0).min_w(0.0))
                .child(
                    icon(&glyphs::CHEVRON_DOWN, 12.0)
                        .color(t.text_dim)
                        .a11y_hidden(),
                ),
        )
}

/// Push the open Package Pin popover (called with the overlay layer from
/// the root view; the trigger cell lives in [`io_ports_table`]).
pub fn push_device_pops(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme) {
    let Some(port) = s.device.io_pin_pop.clone() else {
        return;
    };
    let kit = t.kit();
    let options = pin_options(s);
    let current = io_port_rows(s)
        .iter()
        .find(|r| r.name == port)
        .and_then(|r| r.package_pin.clone())
        .unwrap_or_else(|| "- set site -".to_string());
    let mut opts = col().w_full().gap(0.0);
    for (j, (id, label)) in options.iter().enumerate() {
        let site = id.clone();
        let name = port.clone();
        let mut opt_row = el()
            .key(format!("device-io-pin-opt-{j}"))
            .role("button")
            .label(format!("Choose {label}"))
            .focusable(true)
            .w_full()
            .h(24.0)
            .px(8.0)
            .center_y()
            .hover(t.selection)
            .on_press(move |s: &mut AppState| choose_pin(s, &name, &site))
            .child(mono_cell(label, t));
        if label == &current || id == &current {
            opt_row = opt_row.fill(t.selection);
        }
        opts = opts.child(opt_row);
    }
    // Anchor to the trigger already mounted in the table. A second trigger
    // built here would never mount (its keyed anchor wrapper is dropped),
    // leaving the list unanchored at the window corner.
    let pop = format!("device-io-pin-pop-{}", slug(&port));
    let anchor = format!("device-io-pin-{}", slug(&port));
    let content = col()
        .key(format!("{pop}-content"))
        .role("dialog")
        .label(format!("Package pin for {port}"))
        .focus_return(&anchor)
        .min_w(220.0)
        .max_w(320.0)
        .child(
            col()
                .key("body")
                .padding(4.0)
                .shrink()
                .min_h(0.0)
                .child(rune_kit::popover::fit_scroll(opts)),
        );
    layer.push(rune_kit::scrim(
        format!("{pop}-scrim"),
        |s: &mut AppState| s.device.io_pin_pop = None,
    ));
    layer.push(rune_kit::popover::anchored(
        &kit,
        rune_kit::popover::raised(&kit, content, kit.radius.md),
        &anchor,
        rune_kit::Placement::BottomStart,
        4.0,
    ));
}

pub fn device_pane(s: &AppState, t: &VivadoTheme) -> El {
    let geom = live_geom(s);
    let live = live_netlist(s);
    let placed = live_placed(s);
    let mut right = col().w(RIGHT_W).h_full().gap(6.0).min_h(0.0);
    if s.device.find_open {
        right = right.child(find_palette(s, t, live.as_ref(), placed.as_ref()));
    }
    right = right.child(device_inspector(s, t, live.as_ref(), placed.as_ref()));
    col()
        .key("device")
        .grow(1.0)
        .h_full()
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Device", "device-pane", t))
        .child(device_toolbar(s, t))
        .child(match s.device.view {
            DeviceView::Die => row()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .gap(6.0)
                .padding(6.0)
                .child(layers_panel(s, t))
                .child(device_canvas(s, t, &geom, live.as_ref(), placed.as_ref()))
                .child(right),
            DeviceView::Package => row()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .gap(6.0)
                .padding(6.0)
                .child(package_pane(s, t))
                .child(right),
        })
        .child(io_ports_table(s, t))
        .child(device_status(s, t, &geom, live.as_ref(), placed.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn harness_with(state: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(state, crate::view, spec.size, env)
    }

    fn device_state() -> AppState {
        let mut s = init();
        s.center = crate::state::CenterPane::Device;
        crate::live::seed_live_fixture(&mut s).expect("fixture must seed");
        s
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        // Settle hover on the fresh tree before pressing (stale hover after
        // the popover opens otherwise eats the tap).
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn layers_gate_cells_and_nets_but_never_regions() {
        let mut st = DeviceState::default();
        assert!(st.cell_visible());
        assert!(!st.net_visible());
        st.layers.nets = true;
        assert!(st.net_visible());
        st.layers.cells = false;
        assert!(!st.cell_visible());
        assert!(st.net_visible());
        assert!(
            !LayerId::Regions.enabled(),
            "regions stay disabled: no HAD region table"
        );
        assert!(LayerId::Regions.disabled_reason().is_some());
        LayerId::Regions.toggle(&mut st.layers);
        assert!(!st.layers.regions, "disabled toggle must be a no-op");
    }

    #[test]
    fn zoom_label_tracks_zoom() {
        let mut st = DeviceState::default();
        assert_eq!(st.zoom_label(), "100%");
        st.zoom_in();
        assert_eq!(st.zoom_label(), "125%");
        st.zoom_fit();
        assert_eq!(st.zoom_label(), "100%");
    }

    #[test]
    fn find_matches_live_names_and_real_sites_case_insensitively() {
        let s = device_state();
        let live = live_netlist(&s).unwrap();
        let placed = live_placed(&s).unwrap();
        let st = DeviceState::default();
        assert_eq!(
            st.matches("", &live, Some(&placed)).len(),
            live.cells.len() + live.nets.len()
        );
        let hits = st.matches("u_div", &live, Some(&placed));
        assert!(
            hits.contains(&DeviceSel::Cell("u_div_u_ff0".into())),
            "prefix hits: {hits:?}"
        );
        assert!(
            !hits.iter().any(|h| h.name() == "u_ff0"),
            "top-level cell must not match"
        );
        let site = placed
            .site_of("u_ff0")
            .expect("fixture places every cell")
            .site
            .clone();
        let mates: Vec<DeviceSel> = placed
            .cells
            .iter()
            .filter(|c| c.site == site)
            .map(|c| DeviceSel::Cell(c.cell.clone()))
            .collect();
        assert!(mates.len() > 1, "packing shares sites between cells");
        assert_eq!(
            st.matches(&site.to_lowercase(), &live, Some(&placed)),
            mates,
            "a real site id must match exactly its packed cells"
        );
        assert!(st.matches("zzz-no-such", &live, Some(&placed)).is_empty());
        assert!(
            st.matches("clk", &live, Some(&placed))
                .contains(&DeviceSel::Net("clk".into()))
        );
    }

    #[test]
    fn io_port_rows_empty_without_design() {
        let s = init();
        assert!(io_port_rows(&s).is_empty(), "no design means no port rows");
    }

    #[test]
    fn io_port_rows_map_ports_sites_and_props() {
        let mut s = device_state();
        let rows = io_port_rows(&s);
        assert_eq!(rows.len(), 4, "fixture has 4 top ports: {rows:?}");
        assert_eq!(rows[0].name, "clk");
        assert_eq!(rows[0].dir, "IN");
        assert_eq!(rows[3].name, "divclk");
        assert_eq!(rows[3].dir, "OUT");
        for r in &rows {
            if let Some(site) = r.site.as_deref() {
                assert!(
                    site.starts_with("IOB_"),
                    "placed ports map to IOB sites: {r:?}"
                );
                assert!(r.iob_cell.is_some(), "sites keep the IOB cell: {r:?}");
            }
            assert_eq!(r.package_pin, None, "no LOC yet: {r:?}");
        }
        assert!(
            rows.iter().any(|r| r.site.is_some()),
            "outputs must map to placed IOBs: {rows:?}"
        );
        let seg = rows.iter().find(|r| r.name == "seg").unwrap();
        assert_eq!(seg.site.as_deref(), Some("IOB_X2Y0"));
        assert_eq!(seg.iob_cell.as_deref(), Some("u_iob"));
        let div = rows.iter().find(|r| r.name == "divclk").unwrap();
        assert_eq!(div.site.as_deref(), Some("IOB_X3Y0"));
        assert_eq!(div.iob_cell.as_deref(), Some("u_div_u_iob"));
        for name in ["clk", "rst"] {
            let r = rows.iter().find(|r| r.name == name).unwrap();
            assert_eq!(r.site, None, "unmapped inputs stay honest: {r:?}");
        }
        s.runs
            .live_design
            .as_mut()
            .unwrap()
            .set_loc("clk", "IOB_X0Y0")
            .unwrap();
        s.runs
            .live_design
            .as_mut()
            .unwrap()
            .set_iostandard("clk", "LVCMOS33")
            .unwrap();
        let rows = io_port_rows(&s);
        assert_eq!(rows[0].package_pin.as_deref(), Some("IOB_X0Y0"));
        assert_eq!(rows[0].iostandard.as_deref(), Some("LVCMOS33"));
    }

    #[test]
    fn io_row_tap_cross_probes_the_placed_iob() {
        let mut rt = harness_with(device_state());
        rt.frame(0.0);
        assert!(rt.find("device-io-ports").is_some());
        assert!(rt.find("device-io-head").is_some());
        assert!(rt.find("device-io-row-3").is_some());
        tap(&mut rt, "device-io-row-3");
        assert!(
            matches!(rt.state.device.selected, Some(DeviceSel::Cell(_))),
            "tap must select the placed IOB cell: {:?}",
            rt.state.device.selected
        );
    }

    #[test]
    fn pin_popover_anchors_to_its_table_trigger() {
        let mut rt = harness_with(device_state());
        rt.frame(0.0);
        tap(&mut rt, "device-io-pin-clk");
        for _ in 0..8 {
            rt.frame(1.0 / 60.0);
        }
        let (trigger, _) = rt.presented("device-io-pin-clk").expect("trigger rect");
        let (content, _) = rt
            .presented("device-io-pin-pop-clk-content")
            .expect("content rect");
        // The I/O table sits near the window bottom, so the long site list
        // flips above its trigger; either fitted side must stay adjacent.
        let below = content[1] >= trigger[1] + trigger[3] - 1.0;
        let above = content[1] + content[3] <= trigger[1] + 1.0;
        assert!(
            below || above,
            "content must hug its trigger: trigger {trigger:?} content {content:?}"
        );
        assert!(
            (content[0] - trigger[0]).abs() < 2.0,
            "content must align to the trigger start: trigger {trigger:?} content {content:?}"
        );
    }

    #[test]
    fn pin_combo_pick_sets_loc_and_shows_it() {
        let mut rt = harness_with(device_state());
        rt.frame(0.0);
        tap(&mut rt, "device-io-pin-clk");
        assert_eq!(rt.state.device.io_pin_pop.as_deref(), Some("clk"));
        assert!(
            rt.find("device-io-pin-opt-0").is_some(),
            "pin options must pop over"
        );
        tap(&mut rt, "device-io-pin-opt-0");
        assert_eq!(rt.state.device.io_pin_pop, None, "pick closes the pop");
        assert_eq!(
            rt.state.device.selected,
            Some(DeviceSel::Net("clk".into())),
            "a pin pick cross-probes the assigned port"
        );
        let rows = io_port_rows(&rt.state);
        let clk = rows.iter().find(|r| r.name == "clk").unwrap();
        assert!(
            clk.package_pin
                .as_deref()
                .is_some_and(|p| p.starts_with("IOB_")),
            "pick must set LOC on the port: {clk:?}"
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text.starts_with("set_property LOC IOB_")),
            "pick must echo its Tcl verb"
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text.starts_with("set_property PACKAGE_PIN IOB_")
                    && l.text.contains(" replaced=")),
            "PACKAGE_PIN must report its placement outcome, not repeat the verb: {:?}",
            rt.state
                .console
                .lines
                .iter()
                .map(|l| &l.text)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn package_pins_cover_every_had_iob_with_bank_occupancy() {
        let mut s = device_state();
        let (dev, part) = crate::live::live_device(&s);
        let (grid, pins) = package_pins(&s);
        let n = dev.iob_sites().count();
        assert!(n > 1, "HAD must list real IOB sites");
        assert_eq!(pins.len(), n, "drawing pins are HAD IOB sites");
        assert_eq!(grid.part, part);
        assert_eq!(grid.cols * grid.rows, n as u32, "cells are the IOB box");
        for p in &pins {
            assert!(
                dev.iob_major(p.x, p.y).is_some(),
                "{} is not a HAD IOB site",
                p.pin
            );
            assert_eq!(p.pin, format!("IOB_X{}Y{}", p.x, p.y));
            assert_eq!(p.bank, p.x.saturating_sub(grid.x0) / 8, "8 pins/bank");
        }
        let owner = |pin: &str| {
            pins.iter()
                .find(|p| p.pin == pin)
                .and_then(|p| p.port.clone())
        };
        assert_eq!(owner("IOB_X2Y0").as_deref(), Some("seg"));
        assert_eq!(owner("IOB_X3Y0").as_deref(), Some("divclk"));
        let free = pins
            .iter()
            .find(|p| p.port.is_none())
            .expect("fixture leaves unassigned pins")
            .pin
            .clone();
        crate::tcl::eval_command(&mut s, &format!("set_property LOC {free} rst"));
        let pins = package_pins(&s).1;
        assert_eq!(
            pins.iter()
                .find(|p| p.pin == free)
                .and_then(|p| p.port.as_deref()),
            Some("rst"),
            "LOC constraints join the occupancy map"
        );
        assert!(
            s.console.lines.iter().any(|l| l.text
                == format!("set_property PACKAGE_PIN {free} rst loc={free} replaced=1 rerouted=0")),
            "loc= must name this port's site/constraint: {:?}",
            s.console.lines.iter().map(|l| &l.text).collect::<Vec<_>>()
        );
        let dump = package_grid_text(&s);
        assert!(dump.contains(&format!("pins={n} assigned=3")), "{dump}");
        assert!(dump.contains(&format!("{free}=rst")), "{dump}");
        assert!(dump.contains("map="), "{dump}");
    }

    #[test]
    fn package_view_toggle_mounts_grid_and_selects_ports() {
        let mut rt = harness_with(device_state());
        rt.frame(0.0);
        assert!(rt.find("device-canvas").is_some());
        assert!(rt.find("device-package").is_none());
        tap(&mut rt, "device-view-package");
        assert_eq!(rt.state.device.view, DeviceView::Package);
        for key in [
            "device-package",
            "device-package-summary",
            "device-package-legend",
            "device-package-legend-bank-0",
            "device-package-grid",
            "device-package-pin-iob-x2y0",
        ] {
            assert!(rt.find(key).is_some(), "missing key: {key}");
        }
        assert!(rt.find("device-canvas").is_none(), "package replaces die");
        assert!(rt.find("device-layers").is_none(), "no die layers here");
        assert!(
            rt.find("device-zoom-label").is_none(),
            "package needs no die zoom"
        );
        tap(&mut rt, "device-package-pin-iob-x2y0");
        assert_eq!(
            rt.state.device.selected,
            Some(DeviceSel::Net("seg".into())),
            "assigned pins cross-select the port"
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text.contains("package_pin IOB_X2Y0 port=seg")),
            "pin taps route through select_package_pin: {:?}",
            rt.state
                .console
                .lines
                .iter()
                .map(|l| &l.text)
                .collect::<Vec<_>>()
        );
        tap(&mut rt, "device-view-die");
        assert_eq!(rt.state.device.view, DeviceView::Die);
        assert!(rt.find("device-canvas").is_some());
        assert!(rt.find("device-zoom-label").is_some());
        assert!(rt.find("device-package").is_none());
    }

    #[test]
    fn package_empty_pin_assigns_the_selected_port() {
        let mut rt = harness_with(device_state());
        rt.frame(0.0);
        tap(&mut rt, "device-view-package");
        tap(&mut rt, "device-io-row-0");
        assert_eq!(
            rt.state.device.selected,
            Some(DeviceSel::Net("clk".into())),
            "row tap chooses the assignment target"
        );
        assert!(rt.find("device-package-assigning").is_some());
        let free = package_pins(&rt.state)
            .1
            .into_iter()
            .find(|p| p.port.is_none())
            .expect("fixture leaves unassigned pins")
            .pin;
        let key = package_pin_key(&free);
        tap(&mut rt, &key);
        assert_eq!(
            package_pin(&rt.state, &free).and_then(|p| p.port),
            Some("clk".to_string()),
            "empty package taps bind the selected port: pin={free} key={key} selected={:?} console={:?}",
            rt.state.device.selected,
            rt.state
                .console
                .lines
                .iter()
                .map(|l| &l.text)
                .collect::<Vec<_>>()
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == format!("set_property PACKAGE_PIN {free} [get_ports clk]")),
            "assignment must use the real verb: {:?}",
            rt.state
                .console
                .lines
                .iter()
                .map(|l| &l.text)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn io_table_is_honestly_empty_without_design() {
        let mut s = init();
        s.center = crate::state::CenterPane::Device;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("device-io-empty").is_some());
        assert!(rt.find("device-io-row-0").is_none());
    }

    #[test]
    fn device_tree_exposes_toolbar_layers_canvas_find_and_inspector() {
        let mut rt = harness_with(device_state());
        rt.frame(0.0);
        for key in [
            "device",
            "device-toolbar",
            "device-zoom-out",
            "device-zoom-label",
            "device-zoom-in",
            "device-zoom-fit",
            "device-view-die",
            "device-view-package",
            "device-find-toggle",
            "device-layers",
            "device-layer-sites",
            "device-layer-cells",
            "device-layer-nets",
            "device-layer-regions",
            "device-canvas",
            "device-cell-u-div-u-ff0",
            "device-cell-u-ff0",
            "device-die-label",
            "device-find",
            "device-find-input",
            "device-find-count",
            "device-find-row-0",
            "device-inspector",
            "device-status",
        ] {
            assert!(rt.find(key).is_some(), "missing key: {key}");
        }
        assert!(
            rt.find("device-inspector-name").is_none(),
            "no selection yet"
        );
        assert!(rt.find("device-package").is_none(), "die view only");
        assert!(
            rt.find("device-find-empty").is_none(),
            "empty filter matches all"
        );
        assert!(
            rt.find("device-empty").is_none(),
            "live design must clear the empty state"
        );
        assert!(
            rt.find("device-unplaced").is_none(),
            "placement must clear the unplaced note"
        );
    }

    #[test]
    fn empty_session_shows_the_reason_not_demo_cells() {
        let mut s = init();
        s.center = crate::state::CenterPane::Device;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("device-empty").is_some(), "empty reason must mount");
        assert!(
            rt.find("device-cell-u-ff0").is_none(),
            "no cells without a design"
        );
        assert!(
            rt.find("device-find-empty").is_some(),
            "find reports the missing design"
        );
    }

    #[test]
    fn synth_without_impl_lists_cells_unplaced() {
        let mut s = init();
        s.center = crate::state::CenterPane::Device;
        s.tcl.design = Some(
            helion_sv::synth_sv(crate::live::FIXTURE_SRC, "fixture.sv")
                .expect("fixture must elaborate"),
        );
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("device-unplaced").is_some(),
            "unplaced note must mount"
        );
        assert!(
            rt.find("device-cell-u-ff0").is_none(),
            "no canvas cells without sites"
        );
        assert!(
            rt.find("device-find-row-0").is_some(),
            "find still lists real cells"
        );
    }

    #[test]
    fn canvas_tap_selects_cell_and_echoes_tcl() {
        let mut rt = harness_with(device_state());
        rt.frame(0.0);
        // The top-left-most placed cell is visible without scrolling.
        let placed = live_placed(&rt.state).unwrap();
        let first = placed
            .cells
            .iter()
            .min_by_key(|c| (c.site_y, c.site_x))
            .unwrap()
            .cell
            .clone();
        tap(&mut rt, &DeviceSel::cell_key(&first));
        assert_eq!(
            rt.state.device.selected,
            Some(DeviceSel::Cell(first.clone()))
        );
        assert!(
            rt.find("device-inspector-name").is_some(),
            "inspector must show the cell"
        );
        assert!(
            rt.find("device-inspector-site").is_some(),
            "inspector must show the real site"
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == format!("select_objects [get_cells {first}]")),
            "select must echo its Tcl verb"
        );
    }

    #[test]
    fn find_row_tap_selects_net_and_filter_narrows_rows() {
        // Filter to the clock net so its row is visible without scrolling.
        let mut netted = device_state();
        netted.device.find = rune::TextEdit::new("clk");
        let mut rt = harness_with(netted);
        rt.frame(0.0);
        tap(&mut rt, "device-find-row-0");
        assert_eq!(rt.state.device.selected, Some(DeviceSel::Net("clk".into())));
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "select_objects [get_nets clk]"),
            "net select must echo its Tcl verb"
        );
        // Filter edits bind at harness build (the widget owns the edit after).
        let mut filtered = device_state();
        filtered.device.find = rune::TextEdit::new("u_led_u_iob7");
        let mut rt = harness_with(filtered);
        rt.frame(0.0);
        assert!(
            rt.find("device-find-row-0").is_some(),
            "one match must mount"
        );
        assert!(
            rt.find("device-find-row-1").is_none(),
            "other rows must leave"
        );
        tap(&mut rt, "device-find-row-0");
        assert_eq!(
            rt.state.device.selected,
            Some(DeviceSel::Cell("u_led_u_iob7".into()))
        );
    }

    #[test]
    fn cells_layer_toggle_unmounts_canvas_cells() {
        let mut rt = harness_with(device_state());
        rt.frame(0.0);
        assert!(rt.find("device-cell-u-ff0").is_some());
        tap(&mut rt, "device-layer-cells");
        assert!(!rt.state.device.layers.cells);
        assert!(rt.find("device-cell-u-ff0").is_none(), "cells must unmount");
        tap(&mut rt, "device-layer-cells");
        assert!(rt.find("device-cell-u-ff0").is_some(), "cells must remount");
    }

    #[test]
    fn zoom_controls_rescale_and_fit_restores() {
        let mut rt = harness_with(device_state());
        rt.frame(0.0);
        tap(&mut rt, "device-zoom-in");
        tap(&mut rt, "device-zoom-in");
        assert!(rt.state.device.zoom > 1.5);
        tap(&mut rt, "device-zoom-fit");
        assert_eq!(rt.state.device.zoom, 1.0);
    }
}
