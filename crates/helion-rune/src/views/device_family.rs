//! Device view for iCE40 / ECP5 / 7-series parts: the family die from the
//! family crate's device model (helion-targets `reports::die`) with the
//! family placer's primitives on it. After a 7-series implementation run the
//! die is drawn from the run's view contract instead (`runs.impl_view`):
//! placed cells, routing, clock regions and IO banks as layers, with the
//! utilization and H/V congestion heat maps. HL10T parts keep the HAD-model
//! canvas in [`crate::views::device`].

use std::collections::HashMap;

use rune::{El, Shape, canvas, col, el, row, text};

use crate::runs_family::FamilyDie;
use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::cad;
use crate::views::contract_views::{self as cv, HeatMap};
use crate::views::device_layers::heat_color;
use crate::views::sources::pane_header;
use helion_targets::reports::TileClass;
use helion_targets::view::{ImplView, TileClass as VTileClass};
use rune_kit::Checkbox;

/// World pixels per tile at zoom 1.
const TILE: f32 = 9.0;

/// True when `part` names an iCE40 / ECP5 / 7-series device (name pattern
/// only; the registry lookup that loads every HAD part stays off the UI
/// thread).
pub fn is_family_part(part: &str) -> bool {
    let p = part.to_ascii_lowercase();
    p.starts_with("ice40") || p.starts_with("lfe5u") || p.starts_with("lfe5um") || p.starts_with("xc7")
}

fn class_color(c: TileClass, t: &VivadoTheme) -> rune::Color {
    match c {
        TileClass::Logic => t.die_tile,
        TileClass::Ram => t.warn,
        TileClass::Dsp => t.link,
        TileClass::Io => t.pass,
        TileClass::Clock => t.die_region,
        TileClass::Other => t.die_grid,
    }
}

fn class_name(c: TileClass) -> &'static str {
    match c {
        TileClass::Logic => "Logic",
        TileClass::Ram => "RAM",
        TileClass::Dsp => "DSP",
        TileClass::Io => "I/O",
        TileClass::Clock => "Clock",
        TileClass::Other => "Other",
    }
}

fn die_shape(die: &FamilyDie, occupied: &HashMap<(u32, u32), usize>, s: f32, t: &VivadoTheme) -> Shape {
    let g = &die.grid;
    let mut shape = Shape::new();
    for tile in &g.tiles {
        let vy = if g.row0_top { tile.y } else { g.rows - 1 - tile.y };
        let (x, y) = (tile.x as f32 * s, vy as f32 * s);
        let used = occupied.contains_key(&(tile.x, tile.y));
        let fill = if used { t.die_cell } else { class_color(tile.class, t) };
        let inset = if used { 0.5 } else { 1.0 };
        shape = shape.fill(
            vec![[x + inset, y + inset], [x + s - inset, y + inset], [x + s - inset, y + s - inset], [x + inset, y + s - inset]],
            fill,
        );
    }
    shape
}

fn legend(die: &FamilyDie, t: &VivadoTheme) -> El {
    let mut r = row().key("device-family-legend").gap(10.0).items_center();
    for c in [TileClass::Logic, TileClass::Ram, TileClass::Dsp, TileClass::Io, TileClass::Clock] {
        if die.grid.count(c) == 0 {
            continue;
        }
        r = r.child(
            row()
                .gap(4.0)
                .items_center()
                .child(el().w(10.0).h(10.0).fill(class_color(c, t)))
                .child(cad::dim(class_name(c), t)),
        );
    }
    r.child(
        row()
            .gap(4.0)
            .items_center()
            .child(el().w(10.0).h(10.0).fill(t.die_cell))
            .child(cad::dim("Placed", t)),
    )
}

fn view_class_color(c: VTileClass, t: &VivadoTheme) -> Option<rune::Color> {
    Some(match c {
        VTileClass::Logic => t.die_tile,
        VTileClass::Ram => t.warn,
        VTileClass::Dsp => t.link,
        VTileClass::Io => t.pass,
        VTileClass::Clock => t.die_region,
        // Interconnect and other tiles stay die background, as on the HAD canvas.
        VTileClass::Other => return None,
    })
}

fn quad(x: f32, y: f32, w: f32, h: f32) -> Vec<[f32; 2]> {
    vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]]
}

/// The die drawn from the view contract: tile classes, then the heat map,
/// placed sites and routed tiles the layer toggles turn on.
fn view_shape(v: &ImplView, s: &AppState, tile: f32, t: &VivadoTheme) -> Shape {
    let vy = |y: u32| if v.row0_top { y } else { v.rows.saturating_sub(1 + y) };
    let at = |x: u32, y: u32, inset: f32| quad(x as f32 * tile + inset, vy(y) as f32 * tile + inset, tile - 2.0 * inset, tile - 2.0 * inset);
    let mut shape = Shape::new();
    for tl in &v.tiles {
        if let Some(c) = view_class_color(tl.class, t) {
            shape = shape.fill(at(tl.x, tl.y, 1.0), c);
        }
    }
    let heat = s.device.view_heat;
    if let Some(h) = heat {
        for ht in cv::heat_map(v, h) {
            if ht.value <= 0.0 && !ht.overused {
                continue;
            }
            let c = if ht.overused { t.fail } else { heat_color(t, ht.value / 100.0) };
            shape = shape.fill(at(ht.x, ht.y, 0.5), c);
        }
    }
    let l = &s.device.layers;
    if l.cells {
        let c = if heat.is_some() { t.die_cell.alpha(0.55) } else { t.die_cell };
        for u in v.usage.iter().filter(|u| u.sites_used > 0) {
            shape = shape.fill(at(u.x, u.y, 0.5), c);
        }
    }
    if l.nets {
        let xy: HashMap<&str, (u32, u32)> = v.tiles.iter().map(|t| (t.name.as_str(), (t.x, t.y))).collect();
        let mut hit: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        for n in &v.nets {
            for h in n.hops.iter().filter(|h| !h.is_pseudo) {
                if let Some(p) = xy.get(h.tile.as_str()) {
                    hit.insert(*p);
                }
            }
        }
        let inset = tile * 0.3;
        for (x, y) in hit {
            shape = shape.fill(at(x, y, inset), t.die_net.alpha(0.8));
        }
    }
    shape
}

/// Outlined, labelled boxes around each clock region and each used IO bank.
fn view_regions(v: &ImplView, s: &AppState, tile: f32, t: &VivadoTheme) -> Vec<El> {
    let vy = |y: u32| if v.row0_top { y } else { v.rows.saturating_sub(1 + y) };
    // (key, label, colour, label y offset, bbox): bank labels sit one line
    // lower so a bank in a region's corner does not print over the region name.
    let mut boxes: Vec<(String, String, rune::Color, f32, (u32, u32, u32, u32))> = Vec::new();
    let mut grow = |key: String, label: String, color: rune::Color, dy: f32, x: u32, y: u32| {
        let y = vy(y);
        match boxes.iter_mut().find(|b| b.0 == key) {
            Some(b) => b.4 = (b.4.0.min(x), b.4.1.min(y), b.4.2.max(x), b.4.3.max(y)),
            None => boxes.push((key, label, color, dy, (x, y, x, y))),
        }
    };
    if s.device.layers.clk {
        for tl in v.tiles.iter().filter(|t| !t.clock_region.is_empty()) {
            grow(format!("device-view-region-{}", tl.clock_region), tl.clock_region.clone(), t.die_region, 2.0, tl.x, tl.y);
        }
    }
    if s.device.layers.iob {
        let used: std::collections::BTreeSet<u32> = v.io.iter().flat_map(|io| io.pins.iter().filter_map(|p| p.bank)).collect();
        for tl in v.tiles.iter().filter(|t| t.io_bank.is_some_and(|b| used.contains(&b))) {
            let b = tl.io_bank.unwrap_or_default();
            grow(format!("device-view-bank-{b}"), format!("Bank {b}"), t.pass, 16.0, tl.x, tl.y);
        }
    }
    boxes
        .into_iter()
        .map(|(key, label, color, dy, (x0, y0, x1, y1))| {
            el().key(key)
                .role("note")
                .label(label.clone())
                .absolute(x0 as f32 * tile, y0 as f32 * tile)
                .size((x1 - x0 + 1) as f32 * tile, (y1 - y0 + 1) as f32 * tile)
                .border(1.5, color)
                .inert()
                .child(text(label).font(cad::FONT_PIN).color(color).absolute(4.0, dy))
        })
        .collect()
}

/// Layer checkboxes and the heat-map choice for the contract die.
fn view_controls(s: &AppState, t: &VivadoTheme) -> El {
    let kit = t.kit();
    let l = &s.device.layers;
    let check = |key: &'static str, label: &'static str, on: bool, f: fn(&mut AppState)| {
        Checkbox::new(on).label(label).on_toggle::<AppState>(f).build(&kit).key(key)
    };
    let mut r = row()
        .key("device-view-layers")
        .w_full()
        .gap(10.0)
        .items_center()
        .child(check("device-view-cells", "Placed cells", l.cells, |s| s.device.layers.cells = !s.device.layers.cells))
        .child(check("device-view-routing", "Routing", l.nets, |s| s.device.layers.nets = !s.device.layers.nets))
        .child(check("device-view-regions", "Clock regions", l.clk, |s| s.device.layers.clk = !s.device.layers.clk))
        .child(check("device-view-banks", "IO banks", l.iob, |s| s.device.layers.iob = !s.device.layers.iob))
        .child(el().w(1.0).h(16.0).fill(t.border))
        .child(cad::caps("Heat map", t));
    let modes = [
        (None, "Off", "device-view-heat-off"),
        (Some(HeatMap::Utilization), "Utilization", "device-view-heat-util"),
        (Some(HeatMap::CongestionH), "Congestion H", "device-view-heat-cong-h"),
        (Some(HeatMap::CongestionV), "Congestion V", "device-view-heat-cong-v"),
    ];
    for (mode, label, key) in modes {
        r = r.child(cad::tab(key, label, s.device.view_heat == mode, t, move |s: &mut AppState| s.device.view_heat = mode));
    }
    r
}

/// Device pane body for a 7-series run's view contract.
fn view_body(v: &ImplView, s: &AppState, t: &VivadoTheme) -> El {
    let z = s.device.zoom.max(0.25);
    let [vw, vh] = s.device.viewport;
    let fit = if vw > 0.0 && vh > 0.0 { ((vw - 8.0) / v.cols as f32).min((vh - 8.0) / v.rows as f32) } else { TILE };
    let tile = fit.max(3.0) * z;
    let (ww, wh) = (v.cols as f32 * tile, v.rows as f32 * tile);
    let heat_line = match s.device.view_heat {
        None => cv::usage_summary(v),
        Some(h) => {
            let p = cv::heat_panel(v, h);
            match (&p.body, p.note) {
                (cv::PanelBody::Missing(why), _) => format!("{}: not computed: {why}", h.title()),
                (_, Some(n)) => format!("{}: {n}", h.title()),
                (_, None) => h.title().to_string(),
            }
        }
    };
    let regions = v.clock_regions().len();
    let mut world = el().w(ww).h(wh).child(canvas(view_shape(v, s, tile, t)).w_full().h_full());
    for b in view_regions(v, s, tile, t) {
        world = world.child(b);
    }
    col()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .gap(6.0)
        .padding(6.0)
        .child(
            row()
                .w_full()
                .gap(8.0)
                .items_center()
                .child(cad::badge(v.family.label(), t.accent, t).key("device-family-badge"))
                .child(
                    text(format!(
                        "{}: {}×{} tiles · {} cells placed · {} nets routed · {regions} clock regions",
                        v.part,
                        v.cols,
                        v.rows,
                        v.cells.len(),
                        v.nets.len()
                    ))
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text)
                    .key("device-family-summary")
                    .grow(1.0)
                    .min_w(0.0)
                    .truncate(),
                ),
        )
        .child(view_controls(s, t))
        .child(cad::dim(heat_line, t).key("device-view-heat-line"))
        .child(
            col()
                .key("device-family-canvas")
                .role("img")
                .label(format!("{} die from the implementation view", v.family.label()))
                .grow(1.0)
                .w_full()
                .min_h(0.0)
                .fill(t.die_bg)
                .border(1.0, t.border)
                .scroll_xy()
                .on_layout(|s: &mut AppState, r: [f32; 4]| {
                    let v = [r[2], r[3]];
                    let old = s.device.viewport;
                    if (v[0] - old[0]).abs() > 0.5 || (v[1] - old[1]).abs() > 0.5 {
                        s.device.viewport = v;
                    }
                })
                .child(world),
        )
}

/// Device pane body for a family part.
pub fn family_device_pane(s: &AppState, t: &VivadoTheme) -> El {
    let part = helion_targets::canonical_part(&s.project.part).to_string();
    if let Some(v) = s.runs.impl_view.as_ref().filter(|v| v.part.eq_ignore_ascii_case(&part)) {
        return col()
            .key("device")
            .grow(1.0)
            .h_full()
            .min_h(0.0)
            .min_w(0.0)
            .fill(t.panel_bg)
            .border(1.0, t.border)
            .child(pane_header("Device", "device-pane", t))
            .child(view_body(v, s, t));
    }
    let die = s.runs.family_die.as_ref().filter(|d| d.grid.part.eq_ignore_ascii_case(&part));
    let body = match die {
        None => cad::empty_state(
            "device-family-empty",
            &format!(
                "No die loaded for {part}. Run synthesis or implementation: the run worker loads the family device model (and places the design on implementation)."
            ),
            None,
            t,
        ),
        Some(die) => {
            let z = s.device.zoom.max(0.25);
            let mut occupied: HashMap<(u32, u32), usize> = HashMap::new();
            for p in &die.placed {
                *occupied.entry((p.x, p.y)).or_insert(0) += 1;
            }
            let g = &die.grid;
            // Fit the die to the canvas at zoom 1 (viewport recorded on layout).
            let [vw, vh] = s.device.viewport;
            let fit = if vw > 0.0 && vh > 0.0 { ((vw - 8.0) / g.cols as f32).min((vh - 8.0) / g.rows as f32) } else { TILE };
            let tile = fit.max(3.0) * z;
            let (ww, wh) = (g.cols as f32 * tile, g.rows as f32 * tile);
            let counts = [TileClass::Logic, TileClass::Ram, TileClass::Dsp, TileClass::Io]
                .iter()
                .map(|c| format!("{} {}", g.count(*c), class_name(*c)))
                .collect::<Vec<_>>()
                .join(" · ");
            let placed_line = if die.placed.is_empty() {
                "Not placed yet: run implementation to place the design on this die.".to_string()
            } else {
                format!("{} primitive(s) placed on {} tile(s)", die.placed.len(), occupied.len())
            };
            col()
                .grow(1.0)
                .min_h(0.0)
                .w_full()
                .gap(6.0)
                .padding(6.0)
                .child(
                    row()
                        .w_full()
                        .gap(8.0)
                        .items_center()
                        .child(cad::badge(g.family.label(), t.accent, t).key("device-family-badge"))
                        .child(
                            text(format!("{part}: {}×{} tiles · {counts}", g.cols, g.rows))
                                .font(VivadoTheme::FONT_UI)
                                .color(t.text)
                                .key("device-family-summary")
                                .grow(1.0)
                                .min_w(0.0)
                                .truncate(),
                        ),
                )
                .child(row().w_full().gap(8.0).items_center().child(legend(die, t)).child(cad::dim(placed_line, t).key("device-family-placed")))
                .child(
                    col()
                        .key("device-family-canvas")
                        .role("img")
                        .label(format!("{} die", g.family.label()))
                        .grow(1.0)
                        .w_full()
                        .min_h(0.0)
                        .fill(t.die_bg)
                        .border(1.0, t.border)
                        .scroll_xy()
                        .on_layout(|s: &mut AppState, r: [f32; 4]| {
                            let v = [r[2], r[3]];
                            let old = s.device.viewport;
                            if (v[0] - old[0]).abs() > 0.5 || (v[1] - old[1]).abs() > 0.5 {
                                s.device.viewport = v;
                            }
                        })
                        .child(el().w(ww).h(wh).child(canvas(die_shape(die, &occupied, tile, t)).w_full().h_full())),
                )
        }
    };
    col()
        .key("device")
        .grow(1.0)
        .h_full()
        .min_h(0.0)
        .min_w(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Device", "device-pane", t))
        .child(body)
}
