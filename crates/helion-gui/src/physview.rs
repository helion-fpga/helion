//! Physical View 2.0: OpenROAD-style device view model logic.
//!
//! Pure functions over `IdeModel` data with no egui dependency, so the layer
//! model, heat-map ramps, slack buckets, find matching, ruler math, and
//! inspector property providers are all unit-testable. The `helion-ide`
//! binary paints these; colors are plain RGB triples here and get mapped to
//! the app theme at the paint site.

use crate::ide::{DeviceSiteView, IdeModel, Pblock, TimingPath};
use helion_device::SiteKind;
use helion_ir::{CellKind, resolve};
use helion_route::HOP_DELAY_PS;
use std::collections::{BTreeSet, HashMap, HashSet};

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
            LayerId::CongestionHeat => "Congestion",
            LayerId::CriticalityHeat => "Criticality",
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
// Wave-9: Multi-Mode Floorplan Heatmap Engine
// ---------------------------------------------------------------------------

/// Vivado-class Device Floorplan Heatmap Overlay mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FloorplanHeatmapMode {
    #[default]
    Off,
    RoutingCongestion,
    SetupSlack,
    ThermalPowerDensity,
    ControlSetUtilization,
}

impl FloorplanHeatmapMode {
    pub const ALL: [FloorplanHeatmapMode; 5] = [
        FloorplanHeatmapMode::Off,
        FloorplanHeatmapMode::RoutingCongestion,
        FloorplanHeatmapMode::SetupSlack,
        FloorplanHeatmapMode::ThermalPowerDensity,
        FloorplanHeatmapMode::ControlSetUtilization,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::RoutingCongestion => "Routing Congestion",
            Self::SetupSlack => "Setup Slack",
            Self::ThermalPowerDensity => "Thermal Power Density",
            Self::ControlSetUtilization => "Control-Set Utilization",
        }
    }

    pub fn parse(spec: &str) -> Option<Self> {
        match spec.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => Some(Self::Off),
            "congestion" | "routing" | "routing_congestion" | "routingcongestion" => {
                Some(Self::RoutingCongestion)
            }
            "slack" | "setup_slack" | "setupslack" | "criticality" => Some(Self::SetupSlack),
            "power" | "thermal" | "thermal_power" | "thermal_power_density" | "thermalpowerdensity" => {
                Some(Self::ThermalPowerDensity)
            }
            "control_set"
            | "controlset"
            | "control_sets"
            | "control_set_utilization"
            | "controlsetutilization" => Some(Self::ControlSetUtilization),
            _ => None,
        }
    }
}

/// RGB color ramp for a given [`FloorplanHeatmapMode`] and normalized `t` in `[0.0, 1.0]`.
pub fn heatmap_mode_ramp(mode: FloorplanHeatmapMode, t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    match mode {
        FloorplanHeatmapMode::Off => [0, 0, 0],
        FloorplanHeatmapMode::RoutingCongestion => congestion_ramp(t),
        FloorplanHeatmapMode::SetupSlack => {
            const SLACK_STOPS: [[u8; 3]; 4] = [
                [35, 190, 95],   // comfortable slack (green)
                [180, 215, 45],  // moderate slack (lime)
                [245, 165, 30],  // near-zero slack (amber)
                [230, 35, 35],   // negative/violating slack (red)
            ];
            ramp(SLACK_STOPS, t)
        }
        FloorplanHeatmapMode::ThermalPowerDensity => {
            const THERMAL_STOPS: [[u8; 3]; 4] = [
                [35, 25, 110],   // cool leakage baseline (indigo)
                [155, 35, 145],  // moderate switching (magenta)
                [240, 110, 25],  // hot tile (orange)
                [255, 230, 80],  // peak thermal density (bright yellow)
            ];
            ramp(THERMAL_STOPS, t)
        }
        FloorplanHeatmapMode::ControlSetUtilization => {
            const CTRL_STOPS: [[u8; 3]; 4] = [
                [25, 130, 160],  // 1 control set (teal)
                [70, 190, 120],  // 2 control sets (emerald)
                [240, 180, 35],  // 3 control sets (gold)
                [225, 50, 60],   // saturated SLICE control sets (crimson)
            ];
            ramp(CTRL_STOPS, t)
        }
    }
}

/// Per-tile heatmap sample at die tile `(x, y)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileHeatmapCell {
    pub x: u32,
    pub y: u32,
    /// Normalized intensity in `[0.0, 1.0]`.
    pub value: f32,
    /// Raw metric value (PIP demand ratio, worst slack ps, power mW, or control-set count).
    pub raw: f64,
    /// RGB color triple from [`heatmap_mode_ramp`].
    pub rgb: [u8; 3],
}

/// Complete die-indexed heatmap grid for a [`FloorplanHeatmapMode`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TileHeatmapGrid {
    pub mode: FloorplanHeatmapMode,
    pub x0: u32,
    pub y0: u32,
    pub cols: u32,
    pub rows: u32,
    /// Row-major `cols * rows` normalized values in `[0.0, 1.0]` (`values[dy * cols + dx]`).
    pub values: Vec<f32>,
    /// Row-major `cols * rows` RGB triples (`colors[dy * cols + dx]`).
    pub colors: Vec<[u8; 3]>,
    /// Row-major `cols * rows` tile records (`cells[dy * cols + dx]`).
    pub cells: Vec<TileHeatmapCell>,
    pub min_raw: f64,
    pub max_raw: f64,
}

impl TileHeatmapGrid {
    pub fn cell_at(&self, x: u32, y: u32) -> Option<&TileHeatmapCell> {
        if x < self.x0 || y < self.y0 {
            return None;
        }
        let dx = x - self.x0;
        let dy = y - self.y0;
        if dx >= self.cols || dy >= self.rows {
            return None;
        }
        self.cells.get((dy * self.cols + dx) as usize)
    }

    pub fn value_at(&self, x: u32, y: u32) -> f32 {
        self.cell_at(x, y).map(|c| c.value).unwrap_or(0.0)
    }

    pub fn color_at(&self, x: u32, y: u32) -> [u8; 3] {
        self.cell_at(x, y).map(|c| c.rgb).unwrap_or([0, 0, 0])
    }
}

/// Compute the normalized `[0.0, 1.0]` per-tile heatmap grid and RGB color ramps
/// for `mode` across the device die.
pub fn compute_tile_heatmap_grid(
    model: &IdeModel,
    mode: FloorplanHeatmapMode,
) -> TileHeatmapGrid {
    let cols = model.device.cols;
    let rows = model.device.rows;
    let x0 = model.device.x0;
    let y0 = model.device.y0;
    let n_tiles = (cols as usize).saturating_mul(rows as usize);
    let mut grid = TileHeatmapGrid {
        mode,
        x0,
        y0,
        cols,
        rows,
        values: vec![0.0; n_tiles],
        colors: vec![[0, 0, 0]; n_tiles],
        cells: Vec::with_capacity(n_tiles),
        min_raw: 0.0,
        max_raw: 0.0,
    };
    if n_tiles == 0 {
        return grid;
    }

    let mut raws = vec![0.0_f64; n_tiles];
    let mut active = vec![false; n_tiles];
    let idx_of = |x: u32, y: u32| -> Option<usize> {
        if x < x0 || y < y0 {
            return None;
        }
        let dx = x - x0;
        let dy = y - y0;
        if dx >= cols || dy >= rows {
            return None;
        }
        Some((dy * cols + dx) as usize)
    };

    match mode {
        FloorplanHeatmapMode::Off => {
            for dy in 0..rows {
                for dx in 0..cols {
                    grid.cells.push(TileHeatmapCell {
                        x: x0 + dx,
                        y: y0 + dy,
                        value: 0.0,
                        raw: 0.0,
                        rgb: [0, 0, 0],
                    });
                }
            }
            return grid;
        }
        FloorplanHeatmapMode::RoutingCongestion => {
            // Combine Routed::congestion with route tile crossings when congestion vector is empty.
            if model.device.congestion.len() == n_tiles {
                for (i, &u) in model.device.congestion.iter().enumerate() {
                    raws[i] = u as f64;
                    active[i] = u > 0;
                }
            }
            for r in &model.device.routes {
                for &(tx, ty) in &r.tiles {
                    if let Some(i) = idx_of(tx, ty) {
                        if model.device.congestion.len() != n_tiles {
                            raws[i] += 1.0;
                        }
                        active[i] = true;
                    }
                }
            }
            let max_u = raws.iter().copied().fold(0.0_f64, f64::max);
            grid.min_raw = 0.0;
            grid.max_raw = max_u;
            for dy in 0..rows {
                for dx in 0..cols {
                    let i = (dy * cols + dx) as usize;
                    let val = if max_u > 0.0 {
                        (raws[i] / max_u).clamp(0.0, 1.0) as f32
                    } else {
                        0.0
                    };
                    let rgb = if val > 0.0 {
                        heatmap_mode_ramp(mode, val)
                    } else {
                        [0, 0, 0]
                    };
                    grid.values[i] = val;
                    grid.colors[i] = rgb;
                    grid.cells.push(TileHeatmapCell {
                        x: x0 + dx,
                        y: y0 + dy,
                        value: val,
                        raw: raws[i],
                        rgb,
                    });
                }
            }
        }
        FloorplanHeatmapMode::SetupSlack => {
            let mut net_slack = model.device.net_slack.clone();
            if net_slack.is_empty() && !model.timing_paths.is_empty() {
                net_slack = build_net_slack(&model.timing_paths);
            }
            let mut tile_slack: Vec<Option<i64>> = vec![None; n_tiles];
            for r in &model.device.routes {
                if let Some(&slk) = net_slack.get(&r.net) {
                    for &(tx, ty) in &r.tiles {
                        if let Some(i) = idx_of(tx, ty) {
                            tile_slack[i] = Some(match tile_slack[i] {
                                Some(prev) => prev.min(slk),
                                None => slk,
                            });
                        }
                    }
                }
            }
            // Also project path cell slacks onto their placed sites.
            for path in &model.timing_paths {
                for cell in &path.cells {
                    if let Some(site) = model.device.occupant_of(cell) {
                        if let Some(i) = idx_of(site.x, site.y) {
                            tile_slack[i] = Some(match tile_slack[i] {
                                Some(prev) => prev.min(path.slack_ps),
                                None => path.slack_ps,
                            });
                        }
                    }
                }
            }
            let period_ps = (model.clock_period_ps.max(1_000)) as f64;
            let mut min_s = f64::INFINITY;
            let mut max_s = f64::NEG_INFINITY;
            for dy in 0..rows {
                for dx in 0..cols {
                    let i = (dy * cols + dx) as usize;
                    let (val, raw, rgb) = match tile_slack[i] {
                        Some(slk) => {
                            let s = slk as f64;
                            min_s = min_s.min(s);
                            max_s = max_s.max(s);
                            // Criticality t in [0.0, 1.0]: negative slack -> [0.75..1.0],
                            // zero slack -> 0.70, comfortable positive slack -> 0.15..0.70.
                            let crit = if s < 0.0 {
                                (0.75 + (-s / 2_000.0) * 0.25).clamp(0.75, 1.0) as f32
                            } else {
                                (0.70 - (s / period_ps).clamp(0.0, 1.0) * 0.55).clamp(0.15, 0.70)
                                    as f32
                            };
                            (crit, s, heatmap_mode_ramp(mode, crit))
                        }
                        None => (0.0, 0.0, [0, 0, 0]),
                    };
                    grid.values[i] = val;
                    grid.colors[i] = rgb;
                    grid.cells.push(TileHeatmapCell {
                        x: x0 + dx,
                        y: y0 + dy,
                        value: val,
                        raw,
                        rgb,
                    });
                }
            }
            if min_s.is_finite() {
                grid.min_raw = min_s;
                grid.max_raw = max_s;
            }
        }
        FloorplanHeatmapMode::ThermalPowerDensity => {
            let pwr = model.power_report();
            let static_mw_tile = (pwr.static_uw as f64 / 1000.0) / (n_tiles.max(1) as f64);
            raws.fill(static_mw_tile);
            let occ_count = model.device.occupied_count().max(1) as f64;
            let dyn_logic_mw = (pwr.logic_uw + pwr.clocks_uw + pwr.signals_uw) as f64 / 1000.0;
            let dyn_io_mw = pwr.io_uw as f64 / 1000.0;
            let dyn_bram_mw = pwr.bram_uw as f64 / 1000.0;
            let dyn_dsp_mw = pwr.dsp_uw as f64 / 1000.0;

            for site in &model.device.sites {
                if site.occupant.is_none() && site.bels.is_empty() {
                    continue;
                }
                if let Some(i) = idx_of(site.x, site.y) {
                    let bel_weight = (site.bels.len().max(1)) as f64;
                    let add_mw = match site.kind {
                        SiteKind::Clb => (dyn_logic_mw / occ_count).max(0.25) * bel_weight,
                        SiteKind::Iob => dyn_io_mw.max(0.40),
                        SiteKind::Bram => dyn_bram_mw.max(1.20),
                        SiteKind::Dsp => dyn_dsp_mw.max(1.50),
                        SiteKind::Clk => 0.35,
                    };
                    raws[i] += add_mw;
                }
            }
            for r in &model.device.routes {
                for &(tx, ty) in &r.tiles {
                    if let Some(i) = idx_of(tx, ty) {
                        raws[i] += 0.05;
                    }
                }
            }
            let min_p = raws.iter().copied().fold(f64::INFINITY, f64::min);
            let max_p = raws.iter().copied().fold(0.0_f64, f64::max);
            grid.min_raw = if min_p.is_finite() { min_p } else { 0.0 };
            grid.max_raw = max_p;
            for dy in 0..rows {
                for dx in 0..cols {
                    let i = (dy * cols + dx) as usize;
                    let val = if max_p > 0.0 {
                        (raws[i] / max_p).clamp(0.0, 1.0) as f32
                    } else {
                        0.0
                    };
                    let rgb = heatmap_mode_ramp(mode, val);
                    grid.values[i] = val;
                    grid.colors[i] = rgb;
                    grid.cells.push(TileHeatmapCell {
                        x: x0 + dx,
                        y: y0 + dy,
                        value: val,
                        raw: raws[i],
                        rgb,
                    });
                }
            }
        }
        FloorplanHeatmapMode::ControlSetUtilization => {
            const MAX_CTRL_SETS_PER_SLICE: f64 = 4.0;
            for site in &model.device.sites {
                if site.occupant.is_none() && site.bels.is_empty() {
                    continue;
                }
                let Some(i) = idx_of(site.x, site.y) else {
                    continue;
                };
                let mut ctrl_sets: BTreeSet<(String, String, String)> = BTreeSet::new();
                if let Some(d) = model.shell.session.design.as_ref() {
                    let mut cand_cells: Vec<&str> = Vec::new();
                    if let Some(occ) = site.occupant.as_deref() {
                        cand_cells.push(occ);
                    }
                    for b in &site.bels {
                        cand_cells.push(b.as_str());
                    }
                    for cname in cand_cells {
                        if let Some(cell) = d.cells.iter().find(|c| resolve(c.name) == cname) {
                            if matches!(cell.kind, CellKind::Hff) {
                                let mut clk = "-".to_string();
                                let mut sr = "-".to_string();
                                let mut ce = "-".to_string();
                                for net in &d.nets {
                                    for ep in &net.endpoints {
                                        if resolve(ep.cell) == cname {
                                            let p = resolve(ep.pin);
                                            if p.eq_ignore_ascii_case("CLK")
                                                || p.eq_ignore_ascii_case("C")
                                            {
                                                clk = resolve(net.name).to_string();
                                            } else if p.eq_ignore_ascii_case("RST")
                                                || p.eq_ignore_ascii_case("SR")
                                                || p.eq_ignore_ascii_case("R")
                                                || p.eq_ignore_ascii_case("S")
                                            {
                                                sr = resolve(net.name).to_string();
                                            } else if p.eq_ignore_ascii_case("CE")
                                                || p.eq_ignore_ascii_case("E")
                                            {
                                                ce = resolve(net.name).to_string();
                                            }
                                        }
                                    }
                                }
                                ctrl_sets.insert((clk, sr, ce));
                            }
                        }
                    }
                }
                // Fallback when synthetic site has BELs/occupant without a full HNF design:
                if ctrl_sets.is_empty() && site.kind == SiteKind::Clb {
                    ctrl_sets.insert(("clk".into(), "-".into(), "-".into()));
                }
                raws[i] = ctrl_sets.len() as f64;
            }
            let max_cs = raws.iter().copied().fold(0.0_f64, f64::max);
            grid.min_raw = 0.0;
            grid.max_raw = max_cs;
            for dy in 0..rows {
                for dx in 0..cols {
                    let i = (dy * cols + dx) as usize;
                    let val = (raws[i] / MAX_CTRL_SETS_PER_SLICE).clamp(0.0, 1.0) as f32;
                    let rgb = if val > 0.0 {
                        heatmap_mode_ramp(mode, val)
                    } else {
                        [0, 0, 0]
                    };
                    grid.values[i] = val;
                    grid.colors[i] = rgb;
                    grid.cells.push(TileHeatmapCell {
                        x: x0 + dx,
                        y: y0 + dy,
                        value: val,
                        raw: raws[i],
                        rgb,
                    });
                }
            }
        }
    }
    grid
}

// ---------------------------------------------------------------------------
// Wave-9: 2×4 (X0Y0..X1Y3) Clock-Region Grid & BUFG/MMCM Spine Visualizer
// ---------------------------------------------------------------------------

/// Color-coded clock domain badge inside a clock region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockDomainBadge {
    pub net: String,
    pub loads: usize,
    pub rgb: [u8; 3],
}

/// Per-region clock spine, domain badges, and MMCM/PLL status in the 2×4 (`X0Y0..X1Y3`) grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockRegionSpine {
    /// Region coordinate name (`"X0Y0"` .. `"X1Y3"`).
    pub name: String,
    pub grid_x: u32,
    pub grid_y: u32,
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    pub total_sites: usize,
    pub occupied_sites: usize,
    pub bufg_used: usize,
    pub bufg_capacity: usize,
    pub bufh_used: usize,
    pub bufh_capacity: usize,
    pub clock_domains: Vec<ClockDomainBadge>,
    pub mmcm_locked: bool,
    pub pll_locked: bool,
}

/// 2×4 (`X0Y0..X1Y3`) Clock-Region Grid & BUFG/MMCM Spine Visualizer model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClockRegionGrid {
    pub cols: u32,
    pub rows: u32,
    /// The 8 clock regions (`X0Y0`, `X1Y0`, `X0Y1`, `X1Y1`, `X0Y2`, `X1Y2`, `X0Y3`, `X1Y3`).
    pub regions: Vec<ClockRegionSpine>,
    pub total_bufg_used: usize,
    pub total_bufg_capacity: usize,
    pub all_mmcm_locked: bool,
}

impl ClockRegionGrid {
    pub fn region(&self, name: &str) -> Option<&ClockRegionSpine> {
        self.regions.iter().find(|r| r.name.eq_ignore_ascii_case(name))
    }
}

const CLOCK_DOMAIN_PALETTE: [[u8; 3]; 6] = [
    [56, 178, 246],  // sky blue
    [52, 211, 153],  // emerald
    [245, 158, 11],  // amber
    [167, 139, 250], // violet
    [244, 114, 182], // pink
    [45, 212, 191],  // teal
];

/// Partition the device fabric into a 2×4 (`X0Y0..X1Y3`) clock-region grid with
/// per-region `BUFG`/`BUFH` clock spine utilization, clock net domain color badges,
/// and MMCM/PLL lock status.
pub fn build_clock_region_grid(model: &IdeModel) -> ClockRegionGrid {
    let nx = 2u32;
    let ny = 4u32;
    let die_cols = model.device.cols.max(nx);
    let die_rows = model.device.rows.max(ny);
    let x0 = model.device.x0;
    let y0 = model.device.y0;
    let cw = die_cols.div_ceil(nx).max(1);
    let rh = die_rows.div_ceil(ny).max(1);

    // Collect active clock nets in deterministic order.
    let mut clock_nets: Vec<String> = Vec::new();
    for c in &model.constraints.clocks {
        if !c.name.is_empty() && !clock_nets.contains(&c.name) {
            clock_nets.push(c.name.clone());
        }
    }
    let cn_report = model.clock_networks();
    for net in &cn_report.networks {
        if !net.net.is_empty() && !clock_nets.contains(&net.net) {
            clock_nets.push(net.net.clone());
        }
    }
    if clock_nets.is_empty() && model.device.occupied_count() > 0 {
        clock_nets.push("clk".to_string());
    }

    let locked = !clock_nets.is_empty() && model.clock_period_ps > 0;
    let mut regions = Vec::with_capacity((nx * ny) as usize);

    for iy in 0..ny {
        for ix in 0..nx {
            let rx0 = x0 + ix * cw;
            let ry0 = y0 + iy * rh;
            let rx1 = (x0 + (ix + 1) * cw - 1).min(x0 + die_cols - 1).max(rx0);
            let ry1 = (y0 + (iy + 1) * rh - 1).min(y0 + die_rows - 1).max(ry0);

            let mut total_sites = 0usize;
            let mut occupied_sites = 0usize;
            let mut region_cells: Vec<&str> = Vec::new();
            for s in &model.device.sites {
                if s.x >= rx0 && s.x <= rx1 && s.y >= ry0 && s.y <= ry1 {
                    total_sites += 1;
                    if let Some(occ) = s.occupant.as_deref() {
                        occupied_sites += 1;
                        region_cells.push(occ);
                    }
                    for b in &s.bels {
                        region_cells.push(b.as_str());
                    }
                }
            }

            let mut domains = Vec::new();
            for (cidx, cnet) in clock_nets.iter().enumerate() {
                let rgb = CLOCK_DOMAIN_PALETTE[cidx % CLOCK_DOMAIN_PALETTE.len()];
                let mut loads_in_region = 0usize;
                if let Some(d) = model.shell.session.design.as_ref() {
                    if let Some(net) = d.nets.iter().find(|n| resolve(n.name) == cnet.as_str()) {
                        for ep in &net.endpoints {
                            let cname = resolve(ep.cell);
                            if region_cells.contains(&cname) {
                                loads_in_region += 1;
                            }
                        }
                    }
                }
                if loads_in_region == 0 && occupied_sites > 0 && cidx == 0 {
                    loads_in_region = occupied_sites;
                }
                if loads_in_region > 0 {
                    domains.push(ClockDomainBadge {
                        net: cnet.clone(),
                        loads: loads_in_region,
                        rgb,
                    });
                }
            }

            let bufg_used = domains.len();
            let bufh_used = domains.iter().map(|d| usize::from(d.loads > 0)).sum();
            regions.push(ClockRegionSpine {
                name: format!("X{ix}Y{iy}"),
                grid_x: ix,
                grid_y: iy,
                x0: rx0,
                y0: ry0,
                x1: rx1,
                y1: ry1,
                total_sites,
                occupied_sites,
                bufg_used,
                bufg_capacity: 16,
                bufh_used,
                bufh_capacity: 12,
                clock_domains: domains,
                mmcm_locked: locked,
                pll_locked: locked,
            });
        }
    }

    ClockRegionGrid {
        cols: nx,
        rows: ny,
        regions,
        total_bufg_used: clock_nets.len(),
        total_bufg_capacity: 32,
        all_mmcm_locked: locked,
    }
}

// ---------------------------------------------------------------------------
// Wave-9: Interactive Pblock Resize & Resource Estimator
// ---------------------------------------------------------------------------

/// Eight cardinal and corner resize handles on a Pblock rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PblockResizeHandle {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

impl PblockResizeHandle {
    pub const ALL: [PblockResizeHandle; 8] = [
        PblockResizeHandle::N,
        PblockResizeHandle::S,
        PblockResizeHandle::E,
        PblockResizeHandle::W,
        PblockResizeHandle::NE,
        PblockResizeHandle::NW,
        PblockResizeHandle::SE,
        PblockResizeHandle::SW,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "N" => Some(Self::N),
            "S" => Some(Self::S),
            "E" => Some(Self::E),
            "W" => Some(Self::W),
            "NE" => Some(Self::NE),
            "NW" => Some(Self::NW),
            "SE" => Some(Self::SE),
            "SW" => Some(Self::SW),
            _ => None,
        }
    }
}

fn snap_coord_to_grid(val: i32, origin: i32, grid: i32) -> i32 {
    if grid <= 1 {
        return val;
    }
    let rel = val - origin;
    let snapped = ((rel + grid / 2).div_euclid(grid)) * grid;
    origin + snapped
}

/// Resize a Pblock rectangle `(x0, y0, x1, y1)` by dragging `handle` by
/// `(delta_x, delta_y)` tiles, grid-snapping to `snap_grid` (minimum 1) and
/// clamping within `die_bounds = (min_x, min_y, max_x, max_y)` while maintaining
/// `x0 <= x1` and `y0 <= y1`.
pub fn resize_pblock_rect(
    rect: (u32, u32, u32, u32),
    handle: PblockResizeHandle,
    delta_x: i32,
    delta_y: i32,
    die_bounds: (u32, u32, u32, u32),
    snap_grid: u32,
) -> (u32, u32, u32, u32) {
    let (bx0, by0, bx1, by1) = (
        die_bounds.0.min(die_bounds.2) as i32,
        die_bounds.1.min(die_bounds.3) as i32,
        die_bounds.0.max(die_bounds.2) as i32,
        die_bounds.1.max(die_bounds.3) as i32,
    );
    let grid = snap_grid.max(1) as i32;
    let mut x0 = (rect.0.min(rect.2) as i32).clamp(bx0, bx1);
    let mut y0 = (rect.1.min(rect.3) as i32).clamp(by0, by1);
    let mut x1 = (rect.0.max(rect.2) as i32).clamp(bx0, bx1);
    let mut y1 = (rect.1.max(rect.3) as i32).clamp(by0, by1);

    match handle {
        PblockResizeHandle::W | PblockResizeHandle::NW | PblockResizeHandle::SW => {
            x0 = snap_coord_to_grid(x0 + delta_x, bx0, grid).clamp(bx0, x1);
        }
        PblockResizeHandle::E | PblockResizeHandle::NE | PblockResizeHandle::SE => {
            x1 = snap_coord_to_grid(x1 + delta_x, bx0, grid).clamp(x0, bx1);
        }
        PblockResizeHandle::N | PblockResizeHandle::S => {}
    }

    match handle {
        PblockResizeHandle::S | PblockResizeHandle::SE | PblockResizeHandle::SW => {
            y0 = snap_coord_to_grid(y0 + delta_y, by0, grid).clamp(by0, y1);
        }
        PblockResizeHandle::N | PblockResizeHandle::NE | PblockResizeHandle::NW => {
            y1 = snap_coord_to_grid(y1 + delta_y, by0, grid).clamp(y0, by1);
        }
        PblockResizeHandle::E | PblockResizeHandle::W => {}
    }

    (x0 as u32, y0 as u32, x1 as u32, y1 as u32)
}

/// Instant capacity vs. assigned cell demand calculation for a Pblock rectangle.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PblockResourceEstimate {
    pub pblock_name: String,
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    /// Enclosed CLB / SLICE capacity inside `(x0..=x1, y0..=y1)`.
    pub slice_capacity: usize,
    /// Assigned SLICE (LUT/FF) demand for this Pblock.
    pub slice_demand: usize,
    /// Enclosed DSP48 site capacity inside `(x0..=x1, y0..=y1)`.
    pub dsp48_capacity: usize,
    /// Assigned DSP48 (`Mac27` / `Dsp48e1`) demand for this Pblock.
    pub dsp48_demand: usize,
    /// Enclosed RAMB18 site capacity inside `(x0..=x1, y0..=y1)`.
    pub ramb18_capacity: usize,
    /// Assigned RAMB18 (`Bram18` / `Bram18e1`) demand for this Pblock.
    pub ramb18_demand: usize,
    /// Enclosed RAMB36 equivalent capacity (`ramb18_capacity / 2`).
    pub ramb36_capacity: usize,
    /// Assigned RAMB36 demand for this Pblock.
    pub ramb36_demand: usize,
    /// True when every resource's enclosed capacity is `>=` its assigned demand.
    pub fits: bool,
}

impl PblockResourceEstimate {
    pub fn summary_text(&self) -> String {
        format!(
            "pblock_est NAME={} RANGE=CLB_X{}Y{}:CLB_X{}Y{} SLICE={}/{} DSP48={}/{} RAMB18={}/{} RAMB36={}/{} FITS={}",
            self.pblock_name,
            self.x0,
            self.y0,
            self.x1,
            self.y1,
            self.slice_demand,
            self.slice_capacity,
            self.dsp48_demand,
            self.dsp48_capacity,
            self.ramb18_demand,
            self.ramb18_capacity,
            self.ramb36_demand,
            self.ramb36_capacity,
            u8::from(self.fits)
        )
    }
}

/// Calculate enclosed `SLICE`, `DSP48`, and `RAMB18`/`RAMB36` site capacity vs.
/// assigned cell demand for `pblock`.
pub fn estimate_pblock_resources(model: &IdeModel, pblock: &Pblock) -> PblockResourceEstimate {
    let mut slice_capacity = 0usize;
    let mut dsp48_capacity = 0usize;
    let mut ramb18_capacity = 0usize;
    if pblock.ranged {
        for s in &model.device.sites {
            if pblock.contains(s.x, s.y) {
                match s.kind {
                    SiteKind::Clb => slice_capacity += 1,
                    SiteKind::Dsp => dsp48_capacity += 1,
                    SiteKind::Bram => ramb18_capacity += 1,
                    SiteKind::Iob | SiteKind::Clk => {}
                }
            }
        }
    }
    let ramb36_capacity = ramb18_capacity / 2;

    let mut lut_demand = 0usize;
    let mut ff_demand = 0usize;
    let mut carry_demand = 0usize;
    let mut dsp48_demand = 0usize;
    let mut ramb18_demand = 0usize;
    let mut ramb36_demand = 0usize;

    if let Some(d) = model.shell.session.design.as_ref() {
        let filter_set: HashSet<&str> = pblock.cells.iter().map(String::as_str).collect();
        for cell in &d.cells {
            let cname = resolve(cell.name);
            if !filter_set.is_empty() && !filter_set.contains(cname) {
                continue;
            }
            match cell.kind {
                CellKind::Lut6 { .. } => lut_demand += 1,
                CellKind::Hff => ff_demand += 1,
                CellKind::Carry4 => carry_demand += 1,
                CellKind::Mac27 | CellKind::Dsp48e1 { .. } => dsp48_demand += 1,
                CellKind::Bram18 | CellKind::Bram18e1 { .. } => ramb18_demand += 1,
                CellKind::IobIn
                | CellKind::IobOut
                | CellKind::ClockBuf
                | CellKind::Ila { .. }
                | CellKind::BlackBox { .. } => {}
            }
        }
    } else {
        for cname in &pblock.cells {
            let low = cname.to_ascii_lowercase();
            if low.contains("dsp") || low.contains("mac") {
                dsp48_demand += 1;
            } else if low.contains("ramb36") {
                ramb36_demand += 1;
                ramb18_demand += 2;
            } else if low.contains("bram") || low.contains("ramb18") {
                ramb18_demand += 1;
            } else if low.contains("ff") {
                ff_demand += 1;
            } else {
                lut_demand += 1;
            }
        }
    }

    let slice_demand = lut_demand.max(ff_demand) + carry_demand;
    let fits = pblock.ranged
        && slice_demand <= slice_capacity
        && dsp48_demand <= dsp48_capacity
        && ramb18_demand <= ramb18_capacity
        && ramb36_demand <= ramb36_capacity;

    PblockResourceEstimate {
        pblock_name: pblock.name.clone(),
        x0: pblock.x0,
        y0: pblock.y0,
        x1: pblock.x1,
        y1: pblock.y1,
        slice_capacity,
        slice_demand,
        dsp48_capacity,
        dsp48_demand,
        ramb18_capacity,
        ramb18_demand,
        ramb36_capacity,
        ramb36_demand,
        fits,
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
    fn inspect_pblock_rows() {
        use crate::ide::Pblock;
        let mut model = fixture_model();
        model.pblocks.push(Pblock {
            name: "pb0".into(),
            x0: 0,
            y0: 0,
            x1: 1,
            y1: 0,
            cells: vec!["lut_cell".into()],
            frames: 10,
            bytes: 100,
            ranged: true,
        });
        let rows = inspect_pblock(&model, "pb0");
        let get = |k: &str| rows.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("Pblock").as_deref(), Some("pb0"));
        assert_eq!(get("Frames").as_deref(), Some("10"));
        assert_eq!(get("Sites").as_deref(), Some("2"));
        assert_eq!(get("Occupied").as_deref(), Some("2"));
        assert_eq!(get("Objects").as_deref(), Some("lut_cell"));
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
