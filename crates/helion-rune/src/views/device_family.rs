//! Device view for iCE40 / ECP5 parts: the family die from the family
//! crate's device model (helion-targets `reports::die`) with the family
//! placer's primitives on it. HL10T parts keep the HAD-model canvas in
//! [`crate::views::device`].

use std::collections::HashMap;

use rune::{El, Shape, canvas, col, el, row, text};

use crate::runs_family::FamilyDie;
use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::cad;
use crate::views::sources::pane_header;
use helion_targets::reports::TileClass;

/// World pixels per tile at zoom 1.
const TILE: f32 = 9.0;

/// True when `part` names an iCE40 / ECP5 device (name pattern only; the
/// registry lookup that loads every HAD part stays off the UI thread).
pub fn is_family_part(part: &str) -> bool {
    let p = part.to_ascii_lowercase();
    p.starts_with("ice40") || p.starts_with("lfe5u") || p.starts_with("lfe5um")
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

/// Device pane body for a family part.
pub fn family_device_pane(s: &AppState, t: &VivadoTheme) -> El {
    let part = s.project.part.clone();
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
