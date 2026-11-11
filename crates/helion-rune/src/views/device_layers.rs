//! Device die layers: per-site-kind toggles (CLB/IOB/BRAM/DSP/CLK), the
//! congestion and timing-criticality heat maps, the pblock overlay, rulers,
//! and the die canvas keyboard (1–9 / 0 / F / R / Esc) plus Fit and Zoom to
//! Selection. Ports old `physview.rs` (`LayerId`, `ALL_LAYERS`, heat modes)
//! and `device_shortcuts` (`helion-ide.rs:9827-9883`).
//!
//! Data sources, all from the engine:
//! - congestion: `helion_route::Routed::congestion` kept in the last run's
//!   [`crate::runs::AnalysisCtx::routed`];
//! - criticality: worst STA slack per placed site from the timing paths of
//!   the last run ([`crate::state::TimingState::paths`]);
//! - pblocks: [`crate::views::device_floor`] (helion-proj `PrjPblock`).
//!
//! The HL10T HAD model exposes no clock-region table, so the old Clock
//! Regions layer is not offered (the old IDE invented a 2×2 grid).

use std::collections::HashMap;

use rune::{El, Shape, Tx, el, text};

use crate::live::{LiveDieGeom, LivePlaced};
use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::device::{
    DeviceSel, TILE_H, TILE_W, ZOOM_MAX, ZOOM_MIN, site_xy, world_h, world_w,
};

/// Die canvas layer visibility. Typed flags behind the layers panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceLayers {
    pub clb: bool,
    pub iob: bool,
    pub bram: bool,
    pub dsp: bool,
    pub clk: bool,
    pub congestion: bool,
    pub criticality: bool,
    pub pblocks: bool,
    pub rulers: bool,
    pub cells: bool,
    pub nets: bool,
}

impl Default for DeviceLayers {
    /// Old `default_on`: heat maps off, everything else on (rulers and net
    /// spans start off so the die reads clean).
    fn default() -> Self {
        Self {
            clb: true,
            iob: true,
            bram: true,
            dsp: true,
            clk: true,
            congestion: false,
            criticality: false,
            pblocks: true,
            rulers: false,
            cells: true,
            nets: false,
        }
    }
}

/// Layer panel rows, in old `ALL_LAYERS` order (keys 1–9 toggle the first
/// nine). Typed so toggles are never strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerId {
    Clb,
    Iob,
    Bram,
    Dsp,
    Clk,
    Congestion,
    Criticality,
    Pblocks,
    Rulers,
    Cells,
    Nets,
}

impl LayerId {
    pub const ALL: [LayerId; 11] = [
        LayerId::Clb,
        LayerId::Iob,
        LayerId::Bram,
        LayerId::Dsp,
        LayerId::Clk,
        LayerId::Congestion,
        LayerId::Criticality,
        LayerId::Pblocks,
        LayerId::Rulers,
        LayerId::Cells,
        LayerId::Nets,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LayerId::Clb => "CLB sites",
            LayerId::Iob => "IOB sites",
            LayerId::Bram => "BRAM sites",
            LayerId::Dsp => "DSP sites",
            LayerId::Clk => "CLK sites",
            LayerId::Congestion => "Congestion",
            LayerId::Criticality => "Criticality",
            LayerId::Pblocks => "Pblocks",
            LayerId::Rulers => "Rulers",
            LayerId::Cells => "Placed cells",
            LayerId::Nets => "Net spans",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            LayerId::Clb => "device-layer-clb",
            LayerId::Iob => "device-layer-iob",
            LayerId::Bram => "device-layer-bram",
            LayerId::Dsp => "device-layer-dsp",
            LayerId::Clk => "device-layer-clk",
            LayerId::Congestion => "device-layer-congestion",
            LayerId::Criticality => "device-layer-criticality",
            LayerId::Pblocks => "device-layer-pblocks",
            LayerId::Rulers => "device-layer-rulers",
            LayerId::Cells => "device-layer-cells",
            LayerId::Nets => "device-layer-nets",
        }
    }

    /// Number-key shortcut (`1`–`9`), if any.
    pub fn shortcut(self) -> Option<char> {
        Self::ALL
            .iter()
            .position(|l| *l == self)
            .filter(|i| *i < 9)
            .and_then(|i| char::from_digit(i as u32 + 1, 10))
    }

    /// Why the layer cannot be shown right now (`None` = available).
    pub fn unavailable(self, s: &AppState, geom: &LiveDieGeom) -> Option<&'static str> {
        match self {
            LayerId::Bram if geom.n_bram == 0 => Some("This part has no BRAM sites"),
            LayerId::Dsp if geom.n_dsp == 0 => Some("This part has no DSP sites"),
            LayerId::Congestion if congestion_grid(s).is_none() => {
                Some("Run Implementation to route the design")
            }
            LayerId::Criticality if s.timing.paths.is_empty() || s.runs.live_placed.is_none() => {
                Some("Run Implementation for placed timing paths")
            }
            LayerId::Pblocks if !s.device.pblocks.iter().any(|p| p.ranged) => {
                Some("No ranged pblocks — create one in the Pblocks tab")
            }
            _ => None,
        }
    }

    pub fn get(self, l: &DeviceLayers) -> bool {
        match self {
            LayerId::Clb => l.clb,
            LayerId::Iob => l.iob,
            LayerId::Bram => l.bram,
            LayerId::Dsp => l.dsp,
            LayerId::Clk => l.clk,
            LayerId::Congestion => l.congestion,
            LayerId::Criticality => l.criticality,
            LayerId::Pblocks => l.pblocks,
            LayerId::Rulers => l.rulers,
            LayerId::Cells => l.cells,
            LayerId::Nets => l.nets,
        }
    }

    fn slot(self, l: &mut DeviceLayers) -> &mut bool {
        match self {
            LayerId::Clb => &mut l.clb,
            LayerId::Iob => &mut l.iob,
            LayerId::Bram => &mut l.bram,
            LayerId::Dsp => &mut l.dsp,
            LayerId::Clk => &mut l.clk,
            LayerId::Congestion => &mut l.congestion,
            LayerId::Criticality => &mut l.criticality,
            LayerId::Pblocks => &mut l.pblocks,
            LayerId::Rulers => &mut l.rulers,
            LayerId::Cells => &mut l.cells,
            LayerId::Nets => &mut l.nets,
        }
    }

    /// Flip the layer. The two heat maps are exclusive (one ramp at a time,
    /// like the old `HeatMode`).
    pub fn toggle(self, l: &mut DeviceLayers) {
        let on = !self.get(l);
        *self.slot(l) = on;
        if on {
            match self {
                LayerId::Congestion => l.criticality = false,
                LayerId::Criticality => l.congestion = false,
                _ => {}
            }
        }
    }

    /// Effective visibility: requested and currently available.
    pub fn shown(self, s: &AppState, geom: &LiveDieGeom) -> bool {
        self.get(&s.device.layers) && self.unavailable(s, geom).is_none()
    }
}

/// Toggle a layer only when it is available (keys and checkboxes).
pub fn toggle_layer(s: &mut AppState, layer: LayerId) -> bool {
    let geom = crate::live::live_geom(s);
    if layer.unavailable(s, &geom).is_some() {
        return false;
    }
    layer.toggle(&mut s.device.layers);
    true
}

// ---- heat data ----

/// Routed per-tile usage of the last implementation run:
/// `(x0, y0, cols, rows, usage)`; `None` without a routed result.
pub fn congestion_grid(s: &AppState) -> Option<(u32, u32, u32, u32, &[u32])> {
    let r = s.runs.analysis.as_ref()?.routed.as_ref()?;
    if r.congestion.is_empty() || r.congestion_cols == 0 {
        return None;
    }
    Some((
        r.congestion_x0,
        r.congestion_y0,
        r.congestion_cols,
        r.congestion_rows,
        &r.congestion,
    ))
}

/// Worst STA slack (ps) per placed site `(x, y)` over the timing paths'
/// cells (start, end, and every pin row).
pub fn site_slack(s: &AppState, placed: &LivePlaced) -> HashMap<(u32, u32), i64> {
    let mut cell_slack: HashMap<&str, i64> = HashMap::new();
    for p in &s.timing.paths {
        for pin in &p.pins {
            let e = cell_slack.entry(pin.cell.as_str()).or_insert(p.slack_ps);
            *e = (*e).min(p.slack_ps);
        }
    }
    let mut out: HashMap<(u32, u32), i64> = HashMap::new();
    for c in &placed.cells {
        if let Some(sl) = cell_slack.get(c.cell.as_str()) {
            let e = out.entry((c.site_x, c.site_y)).or_insert(*sl);
            *e = (*e).min(*sl);
        }
    }
    out
}

/// Heat ramp 0..1 over theme tokens: pass → warn → fail.
pub fn heat_color(t: &VivadoTheme, v: f32) -> rune::Color {
    let v = v.clamp(0.0, 1.0);
    let c = if v < 0.5 {
        t.pass.mix(t.warn, v * 2.0)
    } else {
        t.warn.mix(t.fail, (v - 0.5) * 2.0)
    };
    c.alpha(0.30 + 0.45 * v)
}

fn quad(x: f32, y: f32, w: f32, h: f32) -> Vec<[f32; 2]> {
    vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]]
}

/// Heat-map fills over the site tiles (drawn under cells and spans).
pub fn heat_shape(
    mut shape: Shape,
    s: &AppState,
    t: &VivadoTheme,
    geom: &LiveDieGeom,
    placed: Option<&LivePlaced>,
) -> Shape {
    let z = s.device.zoom;
    if LayerId::Congestion.shown(s, geom) {
        if let Some((x0, y0, cols, rows, usage)) = congestion_grid(s) {
            let max = usage.iter().copied().max().unwrap_or(0).max(1) as f32;
            for dy in 0..rows {
                for dx in 0..cols {
                    let u = usage[(dy * cols + dx) as usize];
                    if u == 0 {
                        continue;
                    }
                    let [ox, oy] = site_xy(geom, x0 + dx, y0 + dy);
                    shape = shape.fill(
                        quad(
                            ox * z + 1.0,
                            oy * z + 1.0,
                            TILE_W * z - 2.0,
                            TILE_H * z - 2.0,
                        ),
                        heat_color(t, u as f32 / max),
                    );
                }
            }
        }
    }
    if LayerId::Criticality.shown(s, geom) {
        if let Some(placed) = placed {
            let slack = site_slack(s, placed);
            let worst = slack.values().copied().min().unwrap_or(0);
            let best = slack.values().copied().max().unwrap_or(0);
            for ((x, y), sl) in &slack {
                let v = if best > worst {
                    (best - sl) as f32 / (best - worst) as f32
                } else if *sl < 0 {
                    1.0
                } else {
                    0.5
                };
                let [ox, oy] = site_xy(geom, *x, *y);
                shape = shape.fill(
                    quad(
                        ox * z + 1.0,
                        oy * z + 1.0,
                        TILE_W * z - 2.0,
                        TILE_H * z - 2.0,
                    ),
                    heat_color(t, v),
                );
            }
        }
    }
    shape
}

/// Legend under the layer toggles for the active heat map (real range).
pub fn heat_legend(s: &AppState, t: &VivadoTheme, geom: &LiveDieGeom) -> Option<El> {
    let (title, lo, hi) = if LayerId::Congestion.shown(s, geom) {
        let (_, _, _, _, usage) = congestion_grid(s)?;
        let max = usage.iter().copied().max().unwrap_or(0);
        (
            "Congestion".to_string(),
            "0".to_string(),
            format!("{max} nets/tile"),
        )
    } else if LayerId::Criticality.shown(s, geom) {
        let placed = crate::live::live_placed(s)?;
        let slack = site_slack(s, &placed);
        let worst = slack.values().copied().min()?;
        let best = slack.values().copied().max()?;
        (
            "Slack".to_string(),
            format!("{:.2} ns", best as f64 / 1000.0),
            format!("{:.2} ns", worst as f64 / 1000.0),
        )
    } else {
        return None;
    };
    let mut ramp = rune::row().gap(1.0).items_center();
    for i in 0..6 {
        ramp = ramp.child(
            el().size(10.0, 10.0)
                .fill(heat_color(t, i as f32 / 5.0))
                .a11y_hidden(),
        );
    }
    let small = |v: String| {
        text(v)
            .font(crate::views::cad::FONT_PIN.mono())
            .color(t.text_dim)
    };
    Some(
        rune::row()
            .key("device-heat-legend")
            .role("note")
            .label(format!("{title} from {lo} to {hi}"))
            .items_center()
            .gap(6.0)
            .child(crate::views::cad::caps(title, t))
            .child(small(lo))
            .child(ramp)
            .child(small(hi)),
    )
}

// ---- pblocks + rulers overlay ----

/// Pblock rectangles (dashed, inert so cells stay tappable), drawn under
/// the placed cells.
pub fn pblock_nodes(s: &AppState, t: &VivadoTheme, geom: &LiveDieGeom) -> Vec<El> {
    let z = s.device.zoom;
    let mut out = Vec::new();
    if LayerId::Pblocks.shown(s, geom) {
        for pb in s.device.pblocks.iter().filter(|p| p.ranged) {
            let [x0, y0] = site_xy(geom, pb.x0, pb.y0);
            let [x1, y1] = site_xy(geom, pb.x1, pb.y1);
            let (lx, ly) = (x0.min(x1), y0.min(y1));
            let (w, h) = ((x0 - x1).abs() + TILE_W, (y0 - y1).abs() + TILE_H);
            let on = s.device.pblock_sel.as_deref() == Some(pb.name.as_str());
            out.push(
                el().key(format!("device-pblock-{}", crate::live::slug(&pb.name)))
                    .role("note")
                    .label(format!("Pblock {} {}", pb.name, pb.range_text()))
                    .absolute(lx * z, ly * z)
                    .size(w * z, h * z)
                    .fill(t.die_net.alpha(if on { 0.16 } else { 0.08 }))
                    .border(if on { 2.5 } else { 1.5 }, t.die_net)
                    .border_dash(6.0, 4.0)
                    .inert()
                    .child(
                        text(pb.name.clone())
                            .font(crate::views::cad::FONT_PIN)
                            .color(t.die_net)
                            .absolute(4.0, 2.0),
                    ),
            );
        }
    }
    out
}

/// Rulers, drawn above the placed cells.
pub fn ruler_nodes(s: &AppState, t: &VivadoTheme, geom: &LiveDieGeom) -> Vec<El> {
    if LayerId::Rulers.shown(s, geom) {
        rulers(s, t, geom)
    } else {
        Vec::new()
    }
}

/// Whether a heat ramp is on (cells then draw translucent over it).
pub fn heat_on(s: &AppState, geom: &LiveDieGeom) -> bool {
    LayerId::Congestion.shown(s, geom) || LayerId::Criticality.shown(s, geom)
}

/// Site-coordinate rulers along the die's top and left edges, labeled every
/// few sites so ticks stay at least ~48pt apart at any zoom.
fn rulers(s: &AppState, t: &VivadoTheme, geom: &LiveDieGeom) -> Vec<El> {
    let z = s.device.zoom;
    let (ww, wh) = (world_w(geom) * z, world_h(geom) * z);
    let band = 14.0;
    let step_x = ((48.0 / (TILE_W * z)).ceil() as u32).max(1);
    let step_y = ((48.0 / (TILE_H * z)).ceil() as u32).max(1);
    let mut ticks = Shape::new();
    let mut out = Vec::new();
    let mut dx = 0;
    while dx < geom.cols {
        let x = dx as f32 * TILE_W * z;
        ticks = ticks.stroke(vec![[x, 0.0], [x, band / 2.0]], 1.0, t.banner_ink);
        out.push(ruler_label(format!("X{}", geom.x0 + dx), x + 2.0, 1.0, t));
        dx += step_x;
    }
    let mut dy = 0;
    while dy < geom.rows {
        let y = dy as f32 * TILE_H * z;
        ticks = ticks.stroke(vec![[0.0, y], [band / 2.0, y]], 1.0, t.banner_ink);
        if y > band {
            out.push(ruler_label(format!("Y{}", geom.y0 + dy), 1.0, y + 1.0, t));
        }
        dy += step_y;
    }
    let mut v = vec![
        rune::canvas(ticks)
            .key("device-rulers")
            .role("img")
            .label("Site rulers")
            .absolute(0.0, 0.0)
            .size(ww, wh)
            .inert(),
    ];
    v.extend(out.into_iter().map(|e| e.inert()));
    v
}

/// One ruler label on a small die-colored chip so it reads over tiles.
fn ruler_label(label: String, x: f32, y: f32, t: &VivadoTheme) -> El {
    el().absolute(x, y)
        .px(2.0)
        .radius(2.0)
        .fill(t.die_bg.alpha(0.8))
        .a11y_hidden()
        .child(
            text(label)
                .font(crate::views::cad::FONT_PIN.mono())
                .color(t.banner_ink),
        )
}

// ---- zoom + keys ----

/// Zoom that fits the whole die into the canvas viewport (`None` before
/// the canvas has been laid out once).
pub fn fit_zoom(s: &AppState, geom: &LiveDieGeom) -> Option<f32> {
    let [vw, vh] = s.device.viewport;
    if vw < 8.0 || vh < 8.0 {
        return None;
    }
    Some(
        ((vw - 4.0) / world_w(geom))
            .min((vh - 4.0) / world_h(geom))
            .clamp(ZOOM_MIN, ZOOM_MAX),
    )
}

/// Fit the die to the viewport (old `device_zoom_fit`).
pub fn fit(s: &mut AppState) -> String {
    let geom = crate::live::live_geom(s);
    s.device.zoom = fit_zoom(s, &geom).unwrap_or(1.0);
    format!("device_zoom fit zoom={:.3}", s.device.zoom)
}

/// World bounding box `[x, y, w, h]` of the selection on the die.
pub fn selection_bbox(s: &AppState) -> Option<[f32; 4]> {
    let geom = crate::live::live_geom(s);
    let placed = crate::live::live_placed(s)?;
    let cells: Vec<String> = match s.device.selected.as_ref()? {
        DeviceSel::Cell(c) => vec![c.clone()],
        DeviceSel::Net(n) => crate::live::live_netlist(s)?.net(n)?.cells.clone(),
        DeviceSel::Pin(_) => return None,
    };
    let mut bb: Option<[f32; 4]> = None;
    for c in &cells {
        let Some(p) = placed.site_of(c) else { continue };
        let [x, y] = site_xy(&geom, p.site_x, p.site_y);
        bb = Some(match bb {
            None => [x, y, x + TILE_W, y + TILE_H],
            Some([a, b, c2, d]) => [a.min(x), b.min(y), c2.max(x + TILE_W), d.max(y + TILE_H)],
        });
    }
    bb.map(|[a, b, c, d]| [a, b, c - a, d - b])
}

/// Why Zoom to Selection is unavailable (`None` = available).
pub fn zoom_selection_unavailable(s: &AppState) -> Option<&'static str> {
    match &s.device.selected {
        None => Some("Select a placed cell or net"),
        Some(DeviceSel::Pin(_)) => Some("Package pins are not on the die"),
        Some(_) if selection_bbox(s).is_none() => Some("Selection is not placed"),
        _ => None,
    }
}

/// Zoom so the selection fills most of the viewport and center on it.
pub fn zoom_to_selection(s: &mut AppState, tx: &mut Tx) {
    let Some([x, y, w, h]) = selection_bbox(s) else {
        return;
    };
    let [vw, vh] = s.device.viewport;
    let (vw, vh) = if vw < 8.0 { (600.0, 400.0) } else { (vw, vh) };
    let pad = 2.0 * TILE_W;
    let z = ((vw / (w + pad)).min(vh / (h + pad))).clamp(ZOOM_MIN, 3.0);
    s.device.zoom = z;
    let cx = (x + w / 2.0) * z - vw / 2.0;
    let cy = (y + h / 2.0) * z - vh / 2.0;
    tx.scroll_to_instant("device-canvas", cx.max(0.0), cy.max(0.0));
}

/// Die canvas keys while it has focus (old `device_shortcuts`): `1`–`9`
/// toggle the first nine layers, `0` the congestion heat map, `F` fits,
/// `R` toggles rulers, Escape clears the selection.
pub fn canvas_key(s: &mut AppState, e: &rune::KeyEvent, tx: &mut Tx) -> bool {
    if e.mods.cmd || e.mods.ctrl || e.mods.alt {
        return false;
    }
    match &e.key {
        rune::Key::Escape => {
            if s.device.selected.is_some() {
                s.device.selected = None;
                return true;
            }
            false
        }
        rune::Key::Text(t) => {
            let mut chars = t.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return false;
            };
            match c.to_ascii_lowercase() {
                '0' => {
                    toggle_layer(s, LayerId::Congestion);
                    true
                }
                d @ '1'..='9' => {
                    let i = d.to_digit(10).unwrap_or(1) as usize - 1;
                    toggle_layer(s, LayerId::ALL[i]);
                    true
                }
                'f' => {
                    fit(s);
                    tx.scroll_to_instant("device-canvas", 0.0, 0.0);
                    true
                }
                'r' => {
                    toggle_layer(s, LayerId::Rulers);
                    true
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// Scene/test fixture: the live fixture placement routed by the real
/// router plus a real STA pass, so both heat maps have engine data.
pub fn seed_heat_fixture(s: &mut AppState) -> Result<(), String> {
    crate::live::seed_live_fixture(s)?;
    let dev = helion_device::Device::load_part(crate::live::FIXTURE_PART)?;
    let placed = s
        .runs
        .live_placed
        .as_ref()
        .ok_or("fixture has no placement")?;
    let routed = helion_route::route(placed, &dev)?;
    s.runs.analysis = Some(crate::runs::AnalysisCtx {
        kind: crate::runs::RunKind::Impl,
        // No sibling constraints: STA uses the implicit 10 ns clock.
        source: std::env::temp_dir().join("helion-heat-fixture-none.sv"),
        part: crate::live::FIXTURE_PART.to_string(),
        routed: Some(routed),
        constraints: None,
    });
    crate::analysis::rerun_timing(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init;

    #[test]
    fn shortcuts_cover_the_first_nine_layers() {
        assert_eq!(LayerId::Clb.shortcut(), Some('1'));
        assert_eq!(LayerId::Rulers.shortcut(), Some('9'));
        assert_eq!(LayerId::Cells.shortcut(), None);
    }

    #[test]
    fn heat_maps_are_exclusive_and_gated_on_real_data() {
        let s = init();
        let geom = crate::live::live_geom(&s);
        assert!(
            LayerId::Congestion.unavailable(&s, &geom).is_some(),
            "no routed run yet"
        );
        assert!(
            LayerId::Criticality.unavailable(&s, &geom).is_some(),
            "no timing paths yet"
        );
        assert!(
            LayerId::Pblocks.unavailable(&s, &geom).is_some(),
            "no pblocks yet"
        );
        let mut l = DeviceLayers::default();
        LayerId::Congestion.toggle(&mut l);
        assert!(l.congestion);
        LayerId::Criticality.toggle(&mut l);
        assert!(l.criticality && !l.congestion, "one heat ramp at a time");
    }

    #[test]
    fn unavailable_layers_ignore_toggles() {
        let mut s = init();
        assert!(!toggle_layer(&mut s, LayerId::Congestion));
        assert!(!s.device.layers.congestion);
        assert!(toggle_layer(&mut s, LayerId::Rulers));
        assert!(s.device.layers.rulers);
    }

    #[test]
    fn routed_fixture_feeds_both_heat_maps() {
        let mut s = init();
        seed_heat_fixture(&mut s).expect("route + STA");
        let geom = crate::live::live_geom(&s);
        assert!(LayerId::Congestion.unavailable(&s, &geom).is_none());
        assert!(
            LayerId::Criticality.unavailable(&s, &geom).is_none(),
            "{} paths",
            s.timing.paths.len()
        );
        let placed = crate::live::live_placed(&s).unwrap();
        assert!(
            !site_slack(&s, &placed).is_empty(),
            "paths map onto placed sites"
        );
        assert!(toggle_layer(&mut s, LayerId::Criticality));
        let t = VivadoTheme::dark();
        assert!(heat_legend(&s, &t, &geom).is_some());
    }

    #[test]
    fn heat_ramp_runs_pass_to_fail() {
        let t = VivadoTheme::dark();
        let (lo, hi) = (heat_color(&t, 0.0), heat_color(&t, 1.0));
        assert!((lo.a - 0.30).abs() < 1e-5 && (lo.g - t.pass.g).abs() < 1e-5);
        assert!((hi.a - 0.75).abs() < 1e-5 && (hi.r - t.fail.r).abs() < 1e-5);
    }

    fn harness_with(state: AppState) -> rune::Runtime<AppState> {
        let env = rune::Env {
            system_dark: false,
            scale: 2.0,
        };
        rune::Runtime::new(state, crate::view, crate::app_spec().size, env)
    }

    fn device_state() -> AppState {
        let mut s = init();
        s.center = crate::state::CenterPane::Device;
        crate::live::seed_live_fixture(&mut s).expect("fixture must seed");
        s
    }

    fn tap(rt: &mut rune::Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn canvas_keys_toggle_layers_fit_and_clear_selection() {
        let mut s = device_state();
        s.device.selected = Some(DeviceSel::Cell("u_ff0".into()));
        let mut rt = harness_with(s);
        rt.frame(0.0);
        let (x, y) = rt.find("device-canvas").unwrap();
        // Focus the canvas by pressing empty die space (clears selection),
        // then re-select through state.
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        rt.state.device.selected = Some(DeviceSel::Cell("u_ff0".into()));
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        let key = |rt: &mut rune::Runtime<AppState>, k: rune::Key| {
            rt.key(rune::KeyEvent {
                key: k,
                mods: rune::Mods::default(),
            });
            rt.frame(1.0 / 60.0);
        };
        assert!(rt.state.device.layers.clb);
        key(&mut rt, rune::Key::Text("1".into()));
        assert!(!rt.state.device.layers.clb, "1 toggles CLB sites");
        key(&mut rt, rune::Key::Text("r".into()));
        assert!(rt.state.device.layers.rulers, "R toggles rulers");
        assert!(rt.find("device-rulers").is_some());
        key(&mut rt, rune::Key::Text("6".into()));
        assert!(!rt.state.device.layers.congestion, "unrouted: heat key is a no-op");
        rt.state.device.zoom = 2.0;
        key(&mut rt, rune::Key::Text("f".into()));
        assert!(rt.state.device.zoom < 2.0, "F fits the die");
        key(&mut rt, rune::Key::Escape);
        assert_eq!(rt.state.device.selected, None, "Esc clears the selection");
    }

    #[test]
    fn zoom_to_selection_frames_a_placed_cell() {
        let mut s = device_state();
        s.device.selected = Some(DeviceSel::Cell("u_ff0".into()));
        let mut rt = harness_with(s);
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        tap(&mut rt, "device-zoom-selection");
        assert!(rt.state.device.zoom > 2.0, "a single cell zooms in");
        rt.state.device.selected = Some(DeviceSel::Pin("IOB_X2Y0".into()));
        assert!(crate::views::device_layers::zoom_selection_unavailable(&rt.state).is_some());
    }
}
