//! Physical View 2.0: OpenROAD-style device view model logic.
//!
//! Pure functions over `IdeModel` data with no egui dependency, so the layer
//! model, heat-map ramps, slack buckets, find matching, ruler math, and
//! inspector property providers are all unit-testable. The `helion-ide`
//! binary paints these; colors are plain RGB triples here and get mapped to
//! the app theme at the paint site.

use crate::ide::{DeviceSiteView, IdeModel, TimingPath};
use helion_device::SiteKind;
use helion_route::HOP_DELAY_PS;
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Layer model
// ---------------------------------------------------------------------------

/// Paint-ordered device layers. Order of declaration is paint order
/// (later paints over earlier).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayerId {
    ClbSites,
    IobSites,
    BramSites,
    DspSites,
    ClkSites,
    /// Congestion heat map (paints under routes/sites, over die background).
    CongestionHeat,
    /// Criticality heat map (paints under routes/sites, over die background).
    CriticalityHeat,
    ClockRegions,
    Pblocks,
    Routes,
    Rulers,
}

/// All layers in paint order.
pub const ALL_LAYERS: [LayerId; 11] = [
    LayerId::ClbSites,
    LayerId::IobSites,
    LayerId::BramSites,
    LayerId::DspSites,
    LayerId::ClkSites,
    LayerId::CongestionHeat,
    LayerId::CriticalityHeat,
    LayerId::ClockRegions,
    LayerId::Pblocks,
    LayerId::Routes,
    LayerId::Rulers,
];

impl LayerId {
    pub fn label(&self) -> &'static str {
        match self {
            LayerId::ClbSites => "CLB sites",
            LayerId::IobSites => "IOB sites",
            LayerId::BramSites => "BRAM sites",
            LayerId::DspSites => "DSP sites",
            LayerId::ClkSites => "CLK sites",
            LayerId::CongestionHeat => "Congestion heat map",
            LayerId::CriticalityHeat => "Criticality heat map",
            LayerId::ClockRegions => "Clock regions",
            LayerId::Pblocks => "Pblocks",
            LayerId::Routes => "Routes",
            LayerId::Rulers => "Rulers",
        }
    }

    /// Default visibility. Heat maps default off; everything else on.
    pub fn default_on(&self) -> bool {
        !matches!(
            self,
            LayerId::CongestionHeat | LayerId::CriticalityHeat
        )
    }

    /// Site layer for a HAD site kind, if any.
    pub fn for_site_kind(kind: SiteKind) -> Option<LayerId> {
        match kind {
            SiteKind::Clb => Some(LayerId::ClbSites),
            SiteKind::Iob => Some(LayerId::IobSites),
            SiteKind::Bram => Some(LayerId::BramSites),
            SiteKind::Dsp => Some(LayerId::DspSites),
            SiteKind::Clk => Some(LayerId::ClkSites),
        }
    }
}

/// Which heat-map overlay is active (radio behavior: at most one).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeatMode {
    #[default]
    Off,
    Congestion,
    Criticality,
}

/// Visibility + solo state for the device layers. Persists per session on
/// the app (not the model: engine-thread jobs must never disturb it).
#[derive(Clone, Debug)]
pub struct LayerModel {
    visible: HashSet<LayerId>,
    /// Isolate: when `Some`, every other layer dims to `SOLO_DIM_ALPHA`.
    solo: Option<LayerId>,
    pub heat_mode: HeatMode,
    /// Heat-map overlay opacity, 0.10..=0.90.
    pub heat_opacity: f32,
}

/// Alpha applied to layers that are not the solo target.
pub const SOLO_DIM_ALPHA: f32 = 0.12;

impl Default for LayerModel {
    fn default() -> Self {
        Self {
            visible: ALL_LAYERS
                .iter()
                .copied()
                .filter(|l| l.default_on())
                .collect(),
            solo: None,
            heat_mode: HeatMode::Off,
            heat_opacity: 0.55,
        }
    }
}

impl LayerModel {
    pub fn is_visible(&self, layer: LayerId) -> bool {
        match layer {
            LayerId::CongestionHeat => self.heat_mode == HeatMode::Congestion,
            LayerId::CriticalityHeat => self.heat_mode == HeatMode::Criticality,
            _ => self.visible.contains(&layer),
        }
    }

    pub fn set_visible(&mut self, layer: LayerId, on: bool) {
        match layer {
            LayerId::CongestionHeat => {
                self.heat_mode = if on {
                    HeatMode::Congestion
                } else if self.heat_mode == HeatMode::Congestion {
                    HeatMode::Off
                } else {
                    self.heat_mode
                };
            }
            LayerId::CriticalityHeat => {
                self.heat_mode = if on {
                    HeatMode::Criticality
                } else if self.heat_mode == HeatMode::Criticality {
                    HeatMode::Off
                } else {
                    self.heat_mode
                };
            }
            _ => {
                if on {
                    self.visible.insert(layer);
                } else {
                    self.visible.remove(&layer);
                }
            }
        }
    }

    /// Toggle solo on a layer; returns the new solo state.
    pub fn toggle_solo(&mut self, layer: LayerId) -> Option<LayerId> {
        self.solo = if self.solo == Some(layer) {
            None
        } else {
            Some(layer)
        };
        self.solo
    }

    pub fn solo(&self) -> Option<LayerId> {
        self.solo
    }

    pub fn clear_solo(&mut self) {
        self.solo = None;
    }

    /// Paint alpha for a layer: 1.0 normally, dimmed when another layer is soloed.
    pub fn layer_alpha(&self, layer: LayerId) -> f32 {
        match self.solo {
            None => 1.0,
            Some(s) if s == layer => 1.0,
            Some(_) => SOLO_DIM_ALPHA,
        }
    }

    pub fn set_heat_opacity(&mut self, v: f32) {
        self.heat_opacity = v.clamp(0.10, 0.90);
    }
}

// ---------------------------------------------------------------------------
// Heat-map ramps
// ---------------------------------------------------------------------------

/// Blue -> cyan -> yellow -> red ramp for congestion, `t` in [0, 1].
/// Returns an RGB triple; the paint site applies `heat_opacity`.
pub fn congestion_ramp(t: f32) -> [u8; 3] {
    const STOPS: [[u8; 3]; 4] = [
        [30, 90, 230],   // blue
        [20, 200, 220],  // cyan
        [240, 190, 20],  // yellow
        [225, 35, 25],   // red
    ];
    ramp(STOPS, t)
}

fn ramp(stops: [[u8; 3]; 4], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    let seg = (t * 3.0).min(2.999);
    let i = seg as usize;
    let f = seg - i as f32;
    let a = stops[i].map(|c| c as f32);
    let b = stops[i + 1].map(|c| c as f32);
    [
        (a[0] + (b[0] - a[0]) * f).round() as u8,
        (a[1] + (b[1] - a[1]) * f).round() as u8,
        (a[2] + (b[2] - a[2]) * f).round() as u8,
    ]
}

/// Normalize a raw per-tile congestion count against the die maximum.
pub fn congestion_t(u: u32, max_u: u32) -> f32 {
    if max_u == 0 {
        0.0
    } else {
        (u as f32 / max_u as f32).clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// Timing criticality
// ---------------------------------------------------------------------------

/// Slack bucket for a route polyline or tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlackBucket {
    /// slack > 1 ns: comfortable.
    Safe,
    /// 0 <= slack <= 1 ns: watch.
    Watch,
    /// slack < 0: violating.
    Violating,
}

pub fn slack_bucket(slack_ps: i64) -> SlackBucket {
    if slack_ps < 0 {
        SlackBucket::Violating
    } else if slack_ps <= 1_000 {
        SlackBucket::Watch
    } else {
        SlackBucket::Safe
    }
}

/// Route color for a slack bucket (RGB triple; paint site applies alpha).
pub fn slack_color(bucket: SlackBucket) -> [u8; 3] {
    match bucket {
        SlackBucket::Safe => [45, 190, 90],
        SlackBucket::Watch => [235, 200, 40],
        SlackBucket::Violating => [225, 45, 35],
    }
}

/// Worst (minimum) slack per net across all timing paths.
pub fn build_net_slack(paths: &[TimingPath]) -> HashMap<String, i64> {
    let mut map: HashMap<String, i64> = HashMap::new();
    for p in paths {
        for net in &p.nets {
            map.entry(net.clone())
                .and_modify(|s| *s = (*s).min(p.slack_ps))
                .or_insert(p.slack_ps);
        }
    }
    map
}

// ---------------------------------------------------------------------------
// Ruler
// ---------------------------------------------------------------------------

/// One click-drag tile measurement on the die.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RulerMeasurement {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl RulerMeasurement {
    pub fn dx(&self) -> u32 {
        self.x0.abs_diff(self.x1)
    }

    pub fn dy(&self) -> u32 {
        self.y0.abs_diff(self.y1)
    }

    pub fn manhattan(&self) -> u32 {
        self.dx() + self.dy()
    }

    /// Estimated wire delay: Manhattan tile steps times the route hop model.
    /// Labeled an estimate everywhere it is shown.
    pub fn est_ps(&self) -> i64 {
        self.manhattan() as i64 * HOP_DELAY_PS
    }

    pub fn readout(&self) -> String {
        format!(
            "dx={} dy={} manhattan={} est ~{} ps",
            self.dx(),
            self.dy(),
            self.manhattan(),
            self.est_ps()
        )
    }
}

// ---------------------------------------------------------------------------
// Find palette
// ---------------------------------------------------------------------------

/// One Find (`Ctrl+K`) hit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindHit {
    pub kind: FindKind,
    pub label: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindKind {
    Site,
    Cell,
    Net,
}

/// Case-insensitive subsequence match with a small consecutive-run bonus.
/// Higher is better; `None` means no match.
pub fn fuzzy_score(query: &str, target: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let t: Vec<char> = query_target_lower(target);
    let mut ti = 0usize;
    let mut score: i64 = 0;
    let mut run: i64 = 0;
    for qc in q {
        let mut hit = false;
        while ti < t.len() {
            let tc = t[ti];
            ti += 1;
            if tc == qc {
                run += 1;
                score += 8 + run * 4;
                hit = true;
                break;
            }
            run = 0;
            score -= 2;
        }
        if !hit {
            return None;
        }
    }
    score -= t.len() as i64 / 4;
    Some(score)
}

fn query_target_lower(target: &str) -> Vec<char> {
    target.to_lowercase().chars().collect()
}

/// Find over site IDs, cell/BEL names, and net names. Sorted best-first.
pub fn find_matches(model: &IdeModel, query: &str, limit: usize) -> Vec<FindHit> {
    let mut scored: Vec<(i64, FindHit)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    fn push(
        scored: &mut Vec<(i64, FindHit)>,
        seen: &mut HashSet<String>,
        query: &str,
        kind: FindKind,
        label: String,
        detail: String,
    ) {
        let key = format!("{:?}:{label}", kind);
        if !seen.insert(key) {
            return;
        }
        if let Some(s) = fuzzy_score(query, &label) {
            scored.push((s, FindHit { kind, label, detail }));
        }
    }
    for s in &model.device.sites {
        let name = s.site_name();
        let detail = match &s.occupant {
            Some(o) => format!("{} occupant={o}", kind_name(s.kind)),
            None => kind_name(s.kind).to_string(),
        };
        push(&mut scored, &mut seen, query, FindKind::Site, name, detail);
        if let Some(o) = &s.occupant {
            push(
                &mut scored,
                &mut seen,
                query,
                FindKind::Cell,
                o.clone(),
                format!("at {}", s.site_name()),
            );
        }
        for b in &s.bels {
            push(
                &mut scored,
                &mut seen,
                query,
                FindKind::Cell,
                b.clone(),
                format!("at {}", s.site_name()),
            );
        }
    }
    for r in &model.device.routes {
        push(
            &mut scored,
            &mut seen,
            query,
            FindKind::Net,
            r.net.clone(),
            format!("hops={} delay_ps={}", r.hops, r.delay_ps),
        );
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.truncate(limit);
    scored.into_iter().map(|(_, h)| h).collect()
}

fn kind_name(kind: SiteKind) -> &'static str {
    match kind {
        SiteKind::Clb => "CLB",
        SiteKind::Iob => "IOB",
        SiteKind::Bram => "BRAM",
        SiteKind::Dsp => "DSP",
        SiteKind::Clk => "CLK",
    }
}

// ---------------------------------------------------------------------------
// Inspector property providers
// ---------------------------------------------------------------------------

/// What the inspector panel is showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectorKind {
    Site(String),
    Net(String),
    Pblock(String),
    ClockRegion(String),
    Ruler(usize),
    Empty,
}

/// Resolve the current `model.selected` id to an inspector kind.
pub fn inspector_kind_for(model: &IdeModel) -> InspectorKind {
    let Some(id) = model.selected.as_deref() else {
        return InspectorKind::Empty;
    };
    if model.device.sites.iter().any(|s| {
        s.site_name() == id
            || s.occupant.as_deref() == Some(id)
            || s.bels.iter().any(|b| b == id)
    }) {
        return InspectorKind::Site(id.to_string());
    }
    if model.device.routes.iter().any(|r| r.net == id) {
        return InspectorKind::Net(id.to_string());
    }
    if model.pblocks.iter().any(|p| p.name == id) {
        return InspectorKind::Pblock(id.to_string());
    }
    if model
        .device
        .clock_regions
        .iter()
        .any(|c| c.name == id)
    {
        return InspectorKind::ClockRegion(id.to_string());
    }
    InspectorKind::Empty
}

fn site_id_for(model: &IdeModel, id: &str) -> Option<DeviceSiteView> {
    model
        .device
        .sites
        .iter()
        .find(|s| {
            s.site_name() == id
                || s.occupant.as_deref() == Some(id)
                || s.bels.iter().any(|b| b == id)
        })
        .cloned()
}

/// OpenROAD-style Name/Value rows for a site.
pub fn inspect_site(model: &IdeModel, id: &str) -> Vec<(String, String)> {
    let Some(s) = site_id_for(model, id) else {
        return Vec::new();
    };
    let mut rows = vec![
        ("Kind".into(), kind_name(s.kind).into()),
        ("Site".into(), s.site_name()),
        (
            "Occupant".into(),
            s.occupant.clone().unwrap_or_else(|| "-".into()),
        ),
        (
            "BELs".into(),
            if s.bels.is_empty() {
                "-".into()
            } else {
                s.bels.join(", ")
            },
        ),
    ];
    if let Some(cr) = model.device.clock_region_at(s.x, s.y) {
        rows.push(("Clock region".into(), cr.name.clone()));
    }
    let pb: Vec<String> = model
        .pblocks
        .iter()
        .filter(|p| p.contains(s.x, s.y))
        .map(|p| p.name.clone())
        .collect();
    rows.push((
        "Pblock".into(),
        if pb.is_empty() { "-".into() } else { pb.join(", ") },
    ));
    let driven: Vec<String> = model
        .device
        .routes
        .iter()
        .filter(|r| r.tiles.first() == Some(&(s.x, s.y)))
        .map(|r| r.net.clone())
        .collect();
    let sunk: Vec<String> = model
        .device
        .routes
        .iter()
        .filter(|r| r.tiles.last() == Some(&(s.x, s.y)))
        .map(|r| r.net.clone())
        .collect();
    let through: Vec<String> = model
        .device
        .routes
        .iter()
        .filter(|r| {
            r.tiles.contains(&(s.x, s.y))
                && r.tiles.first() != Some(&(s.x, s.y))
                && r.tiles.last() != Some(&(s.x, s.y))
        })
        .map(|r| r.net.clone())
        .collect();
    rows.push((
        "Fanout nets".into(),
        if driven.is_empty() { "-".into() } else { driven.join(", ") },
    ));
    rows.push((
        "Fanin nets".into(),
        if sunk.is_empty() { "-".into() } else { sunk.join(", ") },
    ));
    rows.push((
        "Through nets".into(),
        if through.is_empty() { "-".into() } else { through.join(", ") },
    ));
    rows.push((
        "Highlighted".into(),
        if s.highlighted { "yes".into() } else { "no".into() },
    ));
    rows
}

/// OpenROAD-style Name/Value rows for a net/route.
pub fn inspect_net(
    model: &IdeModel,
    net: &str,
    net_slack: &HashMap<String, i64>,
) -> Vec<(String, String)> {
    let Some(r) = model.device.route_named(net) else {
        return Vec::new();
    };
    let driver = r
        .tiles
        .first()
        .and_then(|(x, y)| model.device.site_at(*x, *y))
        .map(|s| s.site_name())
        .unwrap_or_else(|| "-".into());
    let sink = r
        .tiles
        .last()
        .and_then(|(x, y)| model.device.site_at(*x, *y))
        .map(|s| s.site_name())
        .unwrap_or_else(|| "-".into());
    let mut rows = vec![
        ("Net".into(), r.net.clone()),
        ("Driver site".into(), driver),
        ("Sink site".into(), sink),
        ("Loads".into(), r.tiles.len().saturating_sub(1).max(1).to_string()),
        ("Hops".into(), r.hops.to_string()),
        ("Delay ps".into(), r.delay_ps.to_string()),
        (
            "Routed".into(),
            if r.hops > 0 { "yes".into() } else { "no (unrouted)".into() },
        ),
    ];
    match net_slack.get(net) {
        Some(slack) => rows.push(("Worst slack ps".into(), slack.to_string())),
        None => rows.push(("Worst slack ps".into(), "no timing data".into())),
    }
    rows.push(("Tiles".into(), r.tiles.len().to_string()));
    rows
}

/// Name/Value rows for a pblock (reuses the table row data).
pub fn inspect_pblock(model: &IdeModel, name: &str) -> Vec<(String, String)> {
    let Some(p) = model.pblocks.iter().find(|p| p.name == name) else {
        return Vec::new();
    };
    vec![
        ("Pblock".into(), p.name.clone()),
        ("X/Y range".into(), p.english_range()),
        ("Frames".into(), p.frames.to_string()),
        ("Bytes".into(), p.bytes.to_string()),
        ("Sites".into(), p.site_count(&model.device.sites).to_string()),
        (
            "Occupied".into(),
            model
                .device
                .sites
                .iter()
                .filter(|s| p.contains(s.x, s.y) && s.occupant.is_some())
                .count()
                .to_string(),
        ),
        (
            "Objects".into(),
            if p.cells.is_empty() {
                "-".into()
            } else {
                p.cells.join(", ")
            },
        ),
    ]
}

/// Name/Value rows for a clock region (reuses `row_text` data).
pub fn inspect_clock_region(model: &IdeModel, name: &str) -> Vec<(String, String)> {
    let Some(c) = model.device.clock_region_named(name) else {
        return Vec::new();
    };
    vec![
        ("Clock region".into(), c.name.clone()),
        (
            "X0/Y0/X1/Y1".into(),
            format!("{}/{}/{}/{}", c.x0, c.y0, c.x1, c.y1),
        ),
        ("Sites".into(), c.site_count(&model.device.sites).to_string()),
        (
            "Occupied".into(),
            c.occupied_count(&model.device.sites).to_string(),
        ),
    ]
}

/// Name/Value rows for a ruler measurement.
pub fn inspect_ruler(m: &RulerMeasurement) -> Vec<(String, String)> {
    vec![
        ("Measurement".into(), "ruler".into()),
        (
            "From".into(),
            format!("tile ({}, {})", m.x0, m.y0),
        ),
        ("To".into(), format!("tile ({}, {})", m.x1, m.y1)),
        ("Delta x".into(), m.dx().to_string()),
        ("Delta y".into(), m.dy().to_string()),
        ("Manhattan".into(), format!("{} tiles", m.manhattan())),
        (
            "Est wire delay".into(),
            format!("~{} ps (estimate)", m.est_ps()),
        ),
    ]
}

/// Dispatch to the right provider for an inspector kind.
pub fn inspect(
    model: &IdeModel,
    kind: &InspectorKind,
    net_slack: &HashMap<String, i64>,
    rulers: &[RulerMeasurement],
) -> Vec<(String, String)> {
    match kind {
        InspectorKind::Site(id) => inspect_site(model, id),
        InspectorKind::Net(net) => inspect_net(model, net, net_slack),
        InspectorKind::Pblock(name) => inspect_pblock(model, name),
        InspectorKind::ClockRegion(name) => inspect_clock_region(model, name),
        InspectorKind::Ruler(i) => rulers
            .get(*i)
            .map(inspect_ruler)
            .unwrap_or_default(),
        InspectorKind::Empty => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ide::{ClockRegion, DeviceRoute, DeviceView};
    use helion_device::Device;

    fn fixture_model() -> IdeModel {
        let mut model = IdeModel::new();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        // Minimal die: 2x2 CLB sites + one route.
        model.device = DeviceView {
            cols: 2,
            rows: 2,
            x0: 0,
            y0: 0,
            sites: vec![
                DeviceSiteView {
                    x: 0,
                    y: 0,
                    kind: SiteKind::Clb,
                    occupant: Some("lut_cell".into()),
                    bels: vec!["BLE0.LUT".into()],
                    highlighted: false,
                },
                DeviceSiteView {
                    x: 1,
                    y: 0,
                    kind: SiteKind::Iob,
                    occupant: Some("out_pad".into()),
                    bels: Vec::new(),
                    highlighted: false,
                },
                DeviceSiteView {
                    x: 0,
                    y: 1,
                    kind: SiteKind::Clb,
                    occupant: None,
                    bels: Vec::new(),
                    highlighted: false,
                },
                DeviceSiteView {
                    x: 1,
                    y: 1,
                    kind: SiteKind::Bram,
                    occupant: None,
                    bels: Vec::new(),
                    highlighted: false,
                },
            ],
            clock_regions: vec![ClockRegion {
                name: "X0Y0".into(),
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            }],
            routes: vec![DeviceRoute {
                net: "led_net".into(),
                hops: 2,
                delay_ps: 80,
                tiles: vec![(0, 0), (1, 0)],
                highlighted: false,
            }],
            pblocks: Vec::new(),
            congestion: Vec::new(),
            net_slack: HashMap::new(),
        };
        let _ = dev;
        model
    }

    #[test]
    fn layer_defaults_match_design() {
        let m = LayerModel::default();
        assert!(m.is_visible(LayerId::ClbSites));
        assert!(m.is_visible(LayerId::Routes));
        assert!(m.is_visible(LayerId::ClockRegions));
        assert!(m.is_visible(LayerId::Pblocks));
        assert!(m.is_visible(LayerId::Rulers));
        assert!(!m.is_visible(LayerId::CongestionHeat));
        assert!(!m.is_visible(LayerId::CriticalityHeat));
        assert_eq!(m.solo(), None);
    }

    #[test]
    fn solo_dims_other_layers_to_12pct() {
        let mut m = LayerModel::default();
        assert_eq!(m.toggle_solo(LayerId::Routes), Some(LayerId::Routes));
        assert_eq!(m.layer_alpha(LayerId::Routes), 1.0);
        assert!((m.layer_alpha(LayerId::ClbSites) - SOLO_DIM_ALPHA).abs() < 1e-6);
        assert_eq!(m.toggle_solo(LayerId::Routes), None);
        assert_eq!(m.layer_alpha(LayerId::ClbSites), 1.0);
    }

    #[test]
    fn heat_mode_is_radio() {
        let mut m = LayerModel::default();
        m.set_visible(LayerId::CongestionHeat, true);
        assert!(m.is_visible(LayerId::CongestionHeat));
        assert!(!m.is_visible(LayerId::CriticalityHeat));
        m.set_visible(LayerId::CriticalityHeat, true);
        assert!(m.is_visible(LayerId::CriticalityHeat));
        assert!(!m.is_visible(LayerId::CongestionHeat));
        m.set_visible(LayerId::CriticalityHeat, false);
        assert_eq!(m.heat_mode, HeatMode::Off);
    }

    #[test]
    fn heat_opacity_clamps() {
        let mut m = LayerModel::default();
        m.set_heat_opacity(2.0);
        assert_eq!(m.heat_opacity, 0.90);
        m.set_heat_opacity(-1.0);
        assert_eq!(m.heat_opacity, 0.10);
    }

    #[test]
    fn congestion_ramp_endpoints() {
        let lo = congestion_ramp(0.0);
        let hi = congestion_ramp(1.0);
        assert!(lo[2] > lo[0], "t=0 is blue");
        assert!(hi[0] > hi[2], "t=1 is red");
        let mid = congestion_ramp(0.5);
        assert!(mid[1] > mid[2], "t=0.5 leans yellow/green, not blue");
    }

    #[test]
    fn congestion_t_normalizes() {
        assert_eq!(congestion_t(0, 10), 0.0);
        assert_eq!(congestion_t(10, 10), 1.0);
        assert_eq!(congestion_t(5, 10), 0.5);
        assert_eq!(congestion_t(3, 0), 0.0);
    }

    #[test]
    fn slack_bucket_boundaries() {
        assert_eq!(slack_bucket(1_001), SlackBucket::Safe);
        assert_eq!(slack_bucket(1_000), SlackBucket::Watch);
        assert_eq!(slack_bucket(0), SlackBucket::Watch);
        assert_eq!(slack_bucket(-1), SlackBucket::Violating);
    }

    #[test]
    fn fuzzy_matches_subsequence() {
        assert!(fuzzy_score("clb", "CLB_X2Y1").is_some());
        assert!(fuzzy_score("x2y1", "CLB_X2Y1").is_some());
        assert!(fuzzy_score("zzz", "CLB_X2Y1").is_none());
        assert!(fuzzy_score("", "anything").is_some());
        let a = fuzzy_score("led", "led_net").unwrap();
        let b = fuzzy_score("led", "unrelated_led_stuff").unwrap();
        assert!(a > b, "shorter target scores higher");
    }

    #[test]
    fn find_hits_cover_sites_cells_nets() {
        let model = fixture_model();
        let hits = find_matches(&model, "lut", 10);
        assert!(hits.iter().any(|h| h.kind == FindKind::Cell && h.label == "lut_cell"));
        let hits = find_matches(&model, "led_net", 10);
        assert!(hits.iter().any(|h| h.kind == FindKind::Net));
        let hits = find_matches(&model, "CLB_X0Y0", 10);
        assert!(hits.iter().any(|h| h.kind == FindKind::Site));
    }

    #[test]
    fn inspect_site_rows() {
        let model = fixture_model();
        let rows = inspect_site(&model, "CLB_X0Y0");
        let get = |k: &str| rows.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("Kind").as_deref(), Some("CLB"));
        assert_eq!(get("Occupant").as_deref(), Some("lut_cell"));
        assert_eq!(get("BELs").as_deref(), Some("BLE0.LUT"));
        assert_eq!(get("Clock region").as_deref(), Some("X0Y0"));
        assert_eq!(get("Fanout nets").as_deref(), Some("led_net"));
        assert_eq!(get("Fanin nets").as_deref(), Some("-"));
    }

    #[test]
    fn inspect_net_rows() {
        let model = fixture_model();
        let slack = HashMap::from([("led_net".into(), -50i64)]);
        let rows = inspect_net(&model, "led_net", &slack);
        let get = |k: &str| rows.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("Driver site").as_deref(), Some("CLB_X0Y0"));
        assert_eq!(get("Sink site").as_deref(), Some("IOB_X1Y0"));
        assert_eq!(get("Worst slack ps").as_deref(), Some("-50"));
        assert_eq!(get("Routed").as_deref(), Some("yes"));
    }

    #[test]
    fn inspect_clock_region_rows() {
        let model = fixture_model();
        let rows = inspect_clock_region(&model, "X0Y0");
        let get = |k: &str| rows.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("Sites").as_deref(), Some("4"));
        assert_eq!(get("Occupied").as_deref(), Some("2"));
    }

    #[test]
    fn ruler_readout() {
        let m = RulerMeasurement { x0: 2, y0: 3, x1: 5, y1: 7 };
        assert_eq!(m.dx(), 3);
        assert_eq!(m.dy(), 4);
        assert_eq!(m.manhattan(), 7);
        assert_eq!(m.est_ps(), 7 * HOP_DELAY_PS);
        assert!(m.readout().contains("manhattan=7"));
    }

    #[test]
    fn net_slack_takes_worst() {
        let mk = |nets: Vec<&str>, slack: i64| TimingPath {
            name: "p".into(),
            startpoint: String::new(),
            endpoint: String::new(),
            cells: Vec::new(),
            nets: nets.into_iter().map(|s| s.to_string()).collect(),
            delay_ps: 0,
            slack_ps: slack,
            pins: Vec::new(),
            group: String::new(),
        };
        let paths = vec![mk(vec!["n1"], 500), mk(vec!["n1", "n2"], -20)];
        let map = build_net_slack(&paths);
        assert_eq!(map["n1"], -20);
        assert_eq!(map["n2"], -20);
    }
}
