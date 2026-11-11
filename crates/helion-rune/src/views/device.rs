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
use crate::views::sources::pane_header;

pub use crate::views::device_io::{IoPortRow, io_port_rows, push_device_pops};
pub use crate::views::device_package::{
    PackageGrid, PackagePin, package_grid_text, package_pin, package_pins,
};
pub use crate::views::device_floor::DeviceBottom;
pub use crate::views::device_layers::{DeviceLayers, LayerId};
use crate::views::device_io::io_ports_table;
use crate::views::device_package::{package_pane, package_pin_key};

/// Tile width in world points.
pub(crate) const TILE_W: f32 = 40.0;
/// Tile height in world points.
pub(crate) const TILE_H: f32 = 42.0;
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
pub(crate) const ZOOM_MIN: f32 = 0.1;
/// Zoom limits.
pub(crate) const ZOOM_MAX: f32 = 4.0;

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
    /// Port the I/O quick-set rows act on.
    pub io_selected: Option<String>,
    /// Die canvas viewport size in points (from `on_layout`; Fit reads it).
    pub viewport: [f32; 2],
    /// Floorplan pblocks (helion-proj `PrjPblock`s) and the selected one.
    pub pblocks: Vec<helion_proj::PrjPblock>,
    pub pblock_sel: Option<String>,
    /// Project `.prj` the pblocks were loaded from.
    pub pblocks_from: Option<std::path::PathBuf>,
    /// Bottom panel tab (I/O Ports or Pblocks).
    pub bottom: DeviceBottom,
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
            io_selected: None,
            viewport: [0.0, 0.0],
            pblocks: Vec::new(),
            pblock_sel: None,
            pblocks_from: None,
            bottom: DeviceBottom::IoPorts,
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
                    .on_press_tx(|s: &mut AppState, tx: &mut Tx| {
                        crate::tcl::eval_command(s, "device_zoom_fit");
                        tx.scroll_to_instant("device-canvas", 0.0, 0.0);
                    })
                    .child(ui_label("Fit", t)),
            )
            .child({
                let why = crate::views::device_layers::zoom_selection_unavailable(s);
                let mut b = el()
                    .key("device-zoom-selection")
                    .role("button")
                    .label("Zoom to selection")
                    .h(22.0)
                    .px(6.0)
                    .center()
                    .child(ui_label("Zoom to Selection", t));
                match why {
                    None => {
                        b = b.focusable(true).hover(t.selection).on_press_tx(
                            |s: &mut AppState, tx: &mut Tx| {
                                crate::views::device_layers::zoom_to_selection(s, tx)
                            },
                        )
                    }
                    Some(r) => b = b.disabled(true).description(r).help(r).layer_opacity(0.45),
                }
                b
            });
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
    let body = col()
        .key("device-layers")
        .role("group")
        .label("Die layers")
        .w(LAYERS_W)
        .h_full()
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
                .child(text("Layers").font(VivadoTheme::FONT_TITLE).color(t.text)),
        );
    let geom = live_geom(s);
    // The toggle list scrolls so the die row can shrink on short windows
    // instead of pushing the pane past the window edge.
    let mut rows = col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        });
    for layer in LayerId::ALL {
        let why = layer.unavailable(s, &geom);
        let check = Checkbox::new(layer.get(&s.device.layers) && why.is_none())
            .label(layer.label())
            .disabled(why.is_some())
            .on_toggle::<AppState>(move |s: &mut AppState| {
                crate::views::device_layers::toggle_layer(s, layer);
            });
        let mut built = check.build(&kit).key(layer.key()).grow(1.0).min_w(0.0);
        if let Some(r) = why {
            built = built.description(r).help(r);
        }
        let mut line = row().w_full().items_center().pr(8.0).child(built);
        if let Some(k) = layer.shortcut() {
            line = line.child(
                text(k.to_string())
                    .font(crate::views::cad::FONT_PIN.mono())
                    .color(t.text_faint)
                    .a11y_hidden(),
            );
        }
        rows = rows.child(line);
    }
    rows = rows.child(
        el().w_full()
            .px(8.0)
            .py(4.0)
            .child(dim_label("Keys 1–9 · 0 heat · F fit · R rulers", t).truncate()),
    );
    body.child(
        col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(rune::ScrollSpec { stick_bottom: false })
            .child(rows),
    )
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
    let lay = s.device.layers;
    if lay.clb {
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
    }
    // Bottom IOB row under each CLB column.
    if lay.iob {
        let iob_y = geom.clb_y0.saturating_sub(1);
        for dx in 0..geom.interior_cols {
            shape = special_tile(shape, geom, z, geom.clb_x0 + dx, iob_y, t.die_tile, t.pass);
        }
    }
    // CLK spine, DSP and BRAM columns (outlines over the CLB grid).
    if lay.clk {
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
    }
    if lay.dsp {
        for i in 0..geom.n_dsp {
            shape = outline_tile(shape, geom, z, geom.dsp_x, geom.dsp_y0 + i * 2, t.link);
        }
    }
    if lay.bram {
        for i in 0..geom.n_bram {
            shape = outline_tile(shape, geom, z, geom.bram_x, geom.bram_y0 + i * 2, t.warn);
        }
    }
    // Congestion / criticality heat over the tiles, under cells and spans.
    shape = crate::views::device_layers::heat_shape(shape, s, t, geom, placed);
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
        .fill(if crate::views::device_layers::heat_on(s, geom) {
            t.die_cell.alpha(0.12)
        } else {
            t.die_cell
        })
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
    for node in crate::views::device_layers::pblock_nodes(s, t, geom) {
        sheet = sheet.child(node);
    }
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
    for node in crate::views::device_layers::ruler_nodes(s, t, geom) {
        sheet = sheet.child(node);
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
        .focusable(true)
        .on_key(crate::views::device_layers::canvas_key)
        .on_layout(|s: &mut AppState, r: [f32; 4]| {
            let v = [r[2], r[3]];
            let old = s.device.viewport;
            if (v[0] - old[0]).abs() > 0.5 || (v[1] - old[1]).abs() > 0.5 {
                s.device.viewport = v;
            }
        })
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
        .grow(1.0)
        .min_h(0.0)
        .max_h(FIND_H + 48.0)
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
    let mut list = col().w_full().grow(1.0).min_h(0.0).max_h(FIND_H).scroll_xy();
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
                crate::views::sources::locate_design_top(s);
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
        .child(dim_label(&text, t).truncate().grow(1.0).min_w(0.0))
        .children(crate::views::device_layers::heat_legend(s, t, geom))
}



/// Bottom panel under the die: I/O Ports (with quick-set) or Pblocks.
fn bottom_panel(s: &AppState, t: &VivadoTheme) -> El {
    let ports = io_port_rows(s).len();
    let tabs = row()
        .key("device-bottom-tabs")
        .role("tablist")
        .w_full()
        .items_center()
        .fill(t.header_bg)
        .child(crate::views::cad::tab(
            "device-tab-io",
            &format!("I/O Ports ({ports})"),
            s.device.bottom == DeviceBottom::IoPorts,
            t,
            |s: &mut AppState| s.device.bottom = DeviceBottom::IoPorts,
        ))
        .child(crate::views::cad::tab(
            "device-tab-pblocks",
            &format!("Pblocks ({})", s.device.pblocks.len()),
            s.device.bottom == DeviceBottom::Pblocks,
            t,
            |s: &mut AppState| s.device.bottom = DeviceBottom::Pblocks,
        ));
    col()
        .key("device-bottom")
        .w_full()
        .min_w(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(tabs)
        .child(match s.device.bottom {
            DeviceBottom::IoPorts => io_ports_table(s, t),
            DeviceBottom::Pblocks => crate::views::device_floor::pblocks_table(s, t),
        })
}

pub fn device_pane(s: &AppState, t: &VivadoTheme) -> El {
    // iCE40 / ECP5 parts draw their own die (family device model), never
    // the HL10T canvas.
    if crate::views::device_family::is_family_part(&s.project.part) {
        return crate::views::device_family::family_device_pane(s, t);
    }
    let geom = live_geom(s);
    let live = live_netlist(s);
    let placed = live_placed(s);
    // The find list gives up height first, so the inspector (and its
    // Locate link) always fits when the bottom tabs take space.
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
        .min_w(0.0)
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
        .child(bottom_panel(s, t))
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
    fn layers_gate_cells_and_nets() {
        let mut st = DeviceState::default();
        assert!(st.cell_visible());
        assert!(!st.net_visible());
        st.layers.nets = true;
        assert!(st.net_visible());
        st.layers.cells = false;
        assert!(!st.cell_visible());
        assert!(st.net_visible());
        assert!(
            LayerId::ALL.iter().all(|l| !l.label().contains("region")),
            "no clock-region layer: the HAD model has no region table"
        );
    }

    #[test]
    fn zoom_label_tracks_zoom() {
        let mut st = DeviceState::default();
        assert_eq!(st.zoom_label(), "100%");
        st.zoom_in();
        assert_eq!(st.zoom_label(), "125%");
        st.zoom_out();
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
            "device-layer-clb",
            "device-layer-cells",
            "device-layer-nets",
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
        let geom = live_geom(&rt.state);
        let [vw, vh] = rt.state.device.viewport;
        let z = rt.state.device.zoom;
        assert!(vw > 100.0 && vh > 100.0, "canvas reports its viewport");
        assert!(
            world_w(&geom) * z <= vw + 0.5 && world_h(&geom) * z <= vh + 0.5,
            "fit shows the whole die"
        );
    }
}
