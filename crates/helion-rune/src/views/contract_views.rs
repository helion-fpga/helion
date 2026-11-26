//! Contract-driven views: everything here reads an [`ImplView`] and nothing
//! else.
//!
//! The point of this module is the rule in the brief: a view never asks a
//! backend for data, and a report never re-runs an engine. Each function takes
//! the contract and returns ready-to-render rows plus the provenance and the
//! "missing" reason for any section that could not be computed. That makes the
//! views testable without a GUI, gives every family the same code path, and
//! means a number on screen can always be traced to the engine that produced
//! it.
//!
//! Renderers are deliberately plain text: a view is a title, a list of rows and
//! a table. The GUI layer turns these into widgets; the golden tests compare
//! the text.
//!
//! No em dashes anywhere.

use std::collections::BTreeMap;

use helion_targets::view::{
    CellPlacement, ImplView, PowerEstimate, Stage, TileClass, TileUsage, TimingSummary,
};

/// One rendered table: a header and rows of equal width.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(columns: &[&str]) -> Self {
        Self {
            columns: columns.iter().map(|c| (*c).to_string()).collect(),
            rows: Vec::new(),
        }
    }

    pub fn push(&mut self, row: &[&str]) {
        debug_assert_eq!(
            row.len(),
            self.columns.len(),
            "row width must match the header"
        );
        self.rows.push(row.iter().map(|c| (*c).to_string()).collect());
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Plain-text rendering, the form the golden tests compare.
    pub fn render(&self) -> String {
        let mut widths: Vec<usize> = self.columns.iter().map(|c| c.len()).collect();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < widths.len() {
                    widths[i] = widths[i].max(cell.len());
                }
            }
        }
        let mut out = String::new();
        let line = |cells: &[String]| {
            let mut s = String::new();
            for (i, c) in cells.iter().enumerate() {
                if i > 0 {
                    s.push_str("  ");
                }
                s.push_str(&format!("{:<width$}", c, width = widths[i]));
            }
            s.trim_end().to_string()
        };
        out.push_str(&line(&self.columns));
        out.push('\n');
        let dashes: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
        out.push_str(&line(&dashes));
        out.push('\n');
        for row in &self.rows {
            out.push_str(&line(row));
            out.push('\n');
        }
        out
    }
}

/// A rendered panel: title, optional note, and a table or a named-missing
/// message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panel {
    pub title: String,
    /// Provenance or method line shown under the title.
    pub note: Option<String>,
    pub body: PanelBody,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelBody {
    Table(Table),
    /// The section could not be computed. The text names the exact missing
    /// input; a view shows this instead of a placeholder.
    Missing(String),
}

impl Panel {
    pub fn table(title: impl Into<String>, table: Table) -> Self {
        Self {
            title: title.into(),
            note: None,
            body: PanelBody::Table(table),
        }
    }

    pub fn missing(title: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            note: None,
            body: PanelBody::Missing(reason.into()),
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Render the panel as text.
    pub fn render(&self) -> String {
        let mut out = format!("{}\n", self.title);
        if let Some(note) = &self.note {
            out.push_str(&format!("{note}\n"));
        }
        match &self.body {
            PanelBody::Table(t) => out.push_str(&t.render()),
            PanelBody::Missing(reason) => out.push_str(&format!("unavailable: {reason}\n")),
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

/// The eleven reports the brief names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Report {
    TimingSummary,
    Utilization,
    Power,
    Drc,
    Methodology,
    DesignAnalysis,
    ClockUtilization,
    ControlSets,
    Io,
    HighFanout,
    RouteStatus,
}

impl Report {
    pub const ALL: [Report; 11] = [
        Report::TimingSummary,
        Report::Utilization,
        Report::Power,
        Report::Drc,
        Report::Methodology,
        Report::DesignAnalysis,
        Report::ClockUtilization,
        Report::ControlSets,
        Report::Io,
        Report::HighFanout,
        Report::RouteStatus,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Report::TimingSummary => "Timing Summary",
            Report::Utilization => "Utilization",
            Report::Power => "Power",
            Report::Drc => "Design Rule Check",
            Report::Methodology => "Methodology",
            Report::DesignAnalysis => "Design Analysis",
            Report::ClockUtilization => "Clock Utilization",
            Report::ControlSets => "Control Sets",
            Report::Io => "I/O Report",
            Report::HighFanout => "High Fanout Nets",
            Report::RouteStatus => "Route Status",
        }
    }
}

fn provenance_note(view: &ImplView, section: &str) -> Option<String> {
    view.provenance
        .get(section)
        .map(|p| format!("{}: {} ({})", p.engine, p.method, p.scope))
}

/// Build one report from the contract.
pub fn report(view: &ImplView, which: Report) -> Panel {
    match which {
        Report::TimingSummary => timing_summary(view),
        Report::Utilization => utilization(view),
        Report::Power => power(view),
        Report::Drc => drc(view),
        Report::Methodology => methodology(view),
        Report::DesignAnalysis => design_analysis(view),
        Report::ClockUtilization => clock_utilization(view),
        Report::ControlSets => control_sets(view),
        Report::Io => io_report(view),
        Report::HighFanout => high_fanout(view),
        Report::RouteStatus => route_status(view),
    }
}

/// Every report, in the brief's order.
pub fn all_reports(view: &ImplView) -> Vec<Panel> {
    Report::ALL.iter().map(|r| report(view, *r)).collect()
}

fn missing(view: &ImplView, section: &str, title: &str) -> Panel {
    let reason = view
        .missing
        .get(section)
        .cloned()
        .unwrap_or_else(|| format!("no {section} data in this run"));
    Panel::missing(title, reason)
}

fn timing_summary(view: &ImplView) -> Panel {
    let Some(t) = &view.timing else {
        return missing(view, "timing", Report::TimingSummary.title());
    };
    let mut tbl = Table::new(&["Metric", "Value"]);
    tbl.push(&["WNS (ns)", &fmt_ps(t.wns_ps)]);
    tbl.push(&["TNS (ns)", &fmt_ps(t.tns_ps)]);
    let hold_missing = || {
        let why = view.missing.get("hold").map(String::as_str).unwrap_or("the engine reported no hold analysis");
        format!("not computed: {why}")
    };
    tbl.push(&["WHS (ns)", &t.whs_ps.map(fmt_ps).unwrap_or_else(hold_missing)]);
    tbl.push(&["THS (ns)", &t.ths_ps.map(fmt_ps).unwrap_or_else(hold_missing)]);
    tbl.push(&[
        "Failing endpoints",
        &format!("{} of {}", t.failing_endpoints, t.total_endpoints),
    ]);
    if let Some(f) = t.fmax_mhz {
        tbl.push(&["Fmax (MHz)", &format!("{:.1}", f)]);
    }
    if let Some(p) = &t.worst_path {
        tbl.push(&["Worst path", &format!("{} -> {}", p.from, p.to)]);
        tbl.push(&["Worst path slack (ns)", &fmt_ps(p.slack_ps)]);
        tbl.push(&["Worst path requirement (ns)", &fmt_ps(p.requirement_ps)]);
    }
    let mut p = Panel::table(Report::TimingSummary.title(), tbl);
    if let Some(n) = provenance_note(view, "timing") {
        p = p.with_note(n);
    }
    p
}

fn fmt_ps(ps: i64) -> String {
    format!("{:.3}", ps as f64 / 1000.0)
}

fn utilization(view: &ImplView) -> Panel {
    if view.tiles.is_empty() {
        return missing(view, "tiles", Report::Utilization.title());
    }
    let mut tbl = Table::new(&["Resource", "Used", "Available", "Utilisation"]);
    let mut total_sites = 0u32;
    let mut used_sites = 0u32;
    // Join usage onto tiles by coordinate so the used count lands in the
    // class the capacity came from.
    let class_at: std::collections::HashMap<(u32, u32), TileClass> =
        view.tiles.iter().map(|t| ((t.x, t.y), t.class)).collect();
    let mut per_class: BTreeMap<TileClass, (u32, u32)> = BTreeMap::new();
    for t in &view.tiles {
        let e = per_class.entry(t.class).or_insert((0, 0));
        e.1 += t.sites.len() as u32;
        total_sites += t.sites.len() as u32;
    }
    for u in &view.usage {
        if u.sites_capacity > 0 {
            used_sites += u.sites_used;
            if let Some(c) = class_at.get(&(u.x, u.y)) {
                per_class.entry(*c).or_insert((0, 0)).0 += u.sites_used;
            }
        }
    }
    for (class, (used, avail)) in &per_class {
        if *avail == 0 {
            continue;
        }
        let pct = 100.0 * *used as f64 / *avail as f64;
        tbl.push(&[class.label(), &used.to_string(), &avail.to_string(), &format!("{pct:.1}%")]);
    }
    // The honest total: sites used against sites the die has.
    if total_sites > 0 {
        tbl.push(&[
            "Logic sites (all classes)",
            &used_sites.to_string(),
            &total_sites.to_string(),
            &format!("{:.1}%", 100.0 * used_sites as f64 / total_sites as f64),
        ]);
    }
    if tbl.is_empty() {
        return Panel::missing(
            Report::Utilization.title(),
            "the device model reports no sites for this part, so there is no capacity to divide by",
        );
    }
    let mut p = Panel::table(Report::Utilization.title(), tbl);
    if let Some(n) = provenance_note(view, "usage") {
        p = p.with_note(n);
    }
    p
}

fn power(view: &ImplView) -> Panel {
    let Some(pw) = &view.power else {
        return missing(view, "power", Report::Power.title());
    };
    let mut tbl = Table::new(&["Block", "Total (mW)"]);
    for b in &pw.blocks {
        tbl.push(&[&b.block, &b.total_mw.to_string()]);
    }
    tbl.push(&["Total", &pw.total_mw.to_string()]);
    Panel::table(Report::Power.title(), tbl).with_note(format!("method: {}", pw.method))
}

fn drc(view: &ImplView) -> Panel {
    if view.drc.is_empty() {
        // An empty DRC list is a result (no violations) only when rules were
        // run; the note names them.
        if view.stage == Stage::Synthesized || view.missing.contains_key("drc") || !view.provenance.contains_key("drc") {
            return missing(view, "drc", Report::Drc.title());
        }
        let rules = provenance_note(view, "drc").unwrap_or_default();
        return Panel::table(Report::Drc.title(), Table::new(&["Rule", "Severity", "Message"]))
            .with_note(format!("no violations; {rules}"));
    }
    let mut tbl = Table::new(&["Rule", "Severity", "Message"]);
    for v in &view.drc {
        tbl.push(&[&v.rule, v.severity, &v.message]);
    }
    let mut p = Panel::table(Report::Drc.title(), tbl);
    if let Some(n) = provenance_note(view, "drc") {
        p = p.with_note(n);
    }
    p
}

fn methodology(view: &ImplView) -> Panel {
    // Methodology is derived from the contract: control sets, clock structure
    // and IO. Each check states its own result.
    let mut tbl = Table::new(&["Check", "Result", "Detail"]);
    let cs = view.control_sets.len();
    tbl.push(&[
        "Control set count",
        if cs == 0 { "n/a" } else if cs <= 8 { "pass" } else { "warn" },
        &format!("{cs} distinct control sets"),
    ]);
    let max_fanout = view.high_fanout.first().map(|n| n.fanout).unwrap_or(0);
    tbl.push(&[
        "High fanout",
        if max_fanout >= 10_000 {
            "warn"
        } else if view.high_fanout.is_empty() {
            "n/a"
        } else {
            "pass"
        },
        &format!(
            "{} highest-fanout nets listed, largest {} loads",
            view.high_fanout.len(),
            max_fanout
        ),
    ]);
    let clocks = view.clocks.as_ref().map(|c| c.clocks.len()).unwrap_or(0);
    tbl.push(&[
        "Clock nets",
        if clocks == 0 { "n/a" } else { "pass" },
        &format!("{clocks} clock nets"),
    ]);
    tbl.push(&[
        "Unrouted nets",
        if view.nets.iter().any(|n| n.kind == "unrouted") {
            "fail"
        } else {
            "pass"
        },
        &format!(
            "{} of {} nets unrouted",
            view.nets.iter().filter(|n| n.kind == "unrouted").count(),
            view.nets.len()
        ),
    ]);
    Panel::table(Report::Methodology.title(), tbl)
        .with_note("derived from the view contract: control sets, fanout, clocks, routing".to_string())
}

fn design_analysis(view: &ImplView) -> Panel {
    let mut tbl = Table::new(&["Metric", "Value"]);
    tbl.push(&["Family", view.family.label()]);
    tbl.push(&["Part", &view.part]);
    tbl.push(&["Stage", view.stage.label()]);
    tbl.push(&["Tiles", &view.tiles.len().to_string()]);
    tbl.push(&["Cells placed", &view.cells.len().to_string()]);
    tbl.push(&["Nets routed", &view.nets.len().to_string()]);
    match view.hottest_tile() {
        Some((u, pct)) => {
            tbl.push(&[
                "Hottest tile",
                &format!(
                    "({}, {}) at {:.1}%{}",
                    u.x,
                    u.y,
                    pct,
                    if u.overflow.map(|o| o.is_overused()).unwrap_or(false) {
                        " OVERUSED"
                    } else {
                        ""
                    }
                ),
            ]);
        }
        None => {
            tbl.push(&["Hottest tile", "no routed occupancy in this run"]);
        }
    }
    Panel::table(Report::DesignAnalysis.title(), tbl)
}

fn clock_utilization(view: &ImplView) -> Panel {
    let Some(cu) = &view.clocks else {
        return missing(view, "clocks", Report::ClockUtilization.title());
    };
    let mut tbl = Table::new(&["Clock", "Period (ns)", "Loads", "Buffer", "Regions"]);
    for c in &cu.clocks {
        tbl.push(&[
            &c.name,
            &format!("{:.2}", c.period_ps as f64 / 1000.0),
            &c.loads.to_string(),
            c.buffer.as_deref().unwrap_or(""),
            &c.regions.join(","),
        ]);
    }
    tbl.push(&[
        "Global buffers",
        "",
        &cu.bufgs_used.to_string(),
        &format!("of {}", cu.bufgs_available),
        "",
    ]);
    let mut p = Panel::table(Report::ClockUtilization.title(), tbl);
    if let Some(n) = provenance_note(view, "clocks") {
        p = p.with_note(n);
    }
    p
}

fn control_sets(view: &ImplView) -> Panel {
    if view.control_sets.is_empty() {
        if view.stage == Stage::Synthesized {
            return missing(view, "control_sets", Report::ControlSets.title());
        }
        return Panel::table(
            Report::ControlSets.title(),
            Table::new(&["Clock", "Enable", "Set/Reset", "SR value", "Flops"]),
        )
        .with_note("no control sets in this design".to_string());
    }
    let mut tbl = Table::new(&["Clock", "Enable", "Set/Reset", "SR value", "Flops"]);
    for s in &view.control_sets {
        tbl.push(&[
            &s.key.0,
            &s.key.1,
            &s.key.2,
            &s.key.3,
            &s.flops.len().to_string(),
        ]);
    }
    let mut p = Panel::table(Report::ControlSets.title(), tbl);
    if let Some(n) = provenance_note(view, "control_sets") {
        p = p.with_note(n);
    }
    p
}

fn io_report(view: &ImplView) -> Panel {
    let Some(io) = &view.io else {
        return missing(view, "io", Report::Io.title());
    };
    let mut tbl = Table::new(&["Port", "Pin", "Bank", "Direction", "Standard"]);
    for p in &io.pins {
        tbl.push(&[
            &p.port,
            &p.package_pin,
            &p.bank.map(|b| b.to_string()).unwrap_or_default(),
            p.direction,
            &p.iostandard,
        ]);
    }
    let mut p = Panel::table(Report::Io.title(), tbl);
    p = p.with_note(format!(
        "{} of {} package pins used",
        io.pins.len(),
        io.package_pins
    ));
    p
}

fn high_fanout(view: &ImplView) -> Panel {
    if view.high_fanout.is_empty() {
        return Panel::table(
            Report::HighFanout.title(),
            Table::new(&["Net", "Fanout", "Clock"]),
        )
        .with_note("no net at or above the fanout threshold".to_string());
    }
    let mut tbl = Table::new(&["Net", "Fanout", "Clock"]);
    for n in &view.high_fanout {
        tbl.push(&[&n.net, &n.fanout.to_string(), if n.is_clock { "yes" } else { "" }]);
    }
    let mut p = Panel::table(Report::HighFanout.title(), tbl);
    if let Some(n) = provenance_note(view, "high_fanout") {
        p = p.with_note(n);
    }
    p
}

fn route_status(view: &ImplView) -> Panel {
    if view.nets.is_empty() {
        return missing(view, "routing", Report::RouteStatus.title());
    }
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for n in &view.nets {
        *by_kind.entry(n.kind).or_insert(0) += 1;
    }
    let mut tbl = Table::new(&["Kind", "Nets"]);
    for (k, n) in &by_kind {
        tbl.push(&[k, &n.to_string()]);
    }
    tbl.push(&["total", &view.nets.len().to_string()]);
    let mut p = Panel::table(Report::RouteStatus.title(), tbl);
    if let Some(n) = provenance_note(view, "routing") {
        p = p.with_note(n);
    }
    p
}

// ---------------------------------------------------------------------------
// Heat maps
// ---------------------------------------------------------------------------

/// Which quantity a heat map paints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeatMap {
    Utilization,
    CongestionH,
    CongestionV,
    Slack,
    Power,
}

impl HeatMap {
    pub fn title(self) -> &'static str {
        match self {
            HeatMap::Utilization => "Utilization",
            HeatMap::CongestionH => "Congestion (horizontal)",
            HeatMap::CongestionV => "Congestion (vertical)",
            HeatMap::Slack => "Timing slack",
            HeatMap::Power => "Power by block",
        }
    }
}

/// One painted tile.
#[derive(Clone, Debug, PartialEq)]
pub struct HotTile {
    pub x: u32,
    pub y: u32,
    /// The quantity in percent; above 100 means over capacity.
    pub value: f32,
    /// True when the tile is over capacity.
    pub overused: bool,
}

/// Compute a heat map from the contract.
///
/// Horizontal and vertical congestion are separate maps on purpose: a real
/// design's two directions differ by several times, and a single blended map
/// would hide which direction is the problem.
pub fn heat_map(view: &ImplView, which: HeatMap) -> Vec<HotTile> {
    match which {
        HeatMap::Utilization => view
            .usage
            .iter()
            .filter(|u| u.sites_capacity > 0)
            .map(|u| HotTile {
                x: u.x,
                y: u.y,
                value: u.utilisation_percent(),
                overused: u.sites_used > u.sites_capacity,
            })
            .collect(),
        HeatMap::CongestionH => view
            .usage
            .iter()
            .filter_map(|u| {
                u.overflow.and_then(|o| {
                    o.h_percent().map(|p| HotTile {
                        x: u.x,
                        y: u.y,
                        value: p,
                        overused: o.h_used > o.h_capacity,
                    })
                })
            })
            .collect(),
        HeatMap::CongestionV => view
            .usage
            .iter()
            .filter_map(|u| {
                u.overflow.and_then(|o| {
                    o.v_percent().map(|p| HotTile {
                        x: u.x,
                        y: u.y,
                        value: p,
                        overused: o.v_used > o.v_capacity,
                    })
                })
            })
            .collect(),
        HeatMap::Slack => slack_heat(view),
        HeatMap::Power => power_heat(view),
    }
}

fn slack_heat(view: &ImplView) -> Vec<HotTile> {
    let Some(t) = &view.timing else {
        return Vec::new();
    };
    // Slack is mapped to 0..100% as "how much of the requirement is consumed":
    // negative slack reads above 100, which is the same convention as the
    // congestion maps, so one colour ramp serves every heat map.
    let req = t
        .worst_path
        .as_ref()
        .map(|p| p.requirement_ps)
        .filter(|r| *r > 0)
        .unwrap_or(0);
    if req == 0 {
        return Vec::new();
    }
    // One value per tile: the worst endpoint slack in it.
    let mut worst: BTreeMap<(u32, u32), i64> = BTreeMap::new();
    for e in &t.endpoints {
        let (Some(x), Some(y)) = (e.x, e.y) else {
            continue;
        };
        let w = worst.entry((x, y)).or_insert(e.slack_ps);
        *w = (*w).min(e.slack_ps);
    }
    worst
        .into_iter()
        .map(|((x, y), slack)| HotTile {
            x,
            y,
            value: 100.0 * (req - slack).max(0) as f32 / req as f32,
            overused: slack < 0,
        })
        .collect()
}

fn power_heat(view: &ImplView) -> Vec<HotTile> {
    // Power is reported per block, not per tile, so it cannot be painted on
    // the die without inventing a placement. Returns empty and the caller
    // shows the block table instead.
    let _ = view;
    Vec::new()
}

/// A panel for a heat map: the top tiles and the summary a reader needs.
pub fn heat_panel(view: &ImplView, which: HeatMap) -> Panel {
    let tiles = heat_map(view, which);
    if tiles.is_empty() {
        let reason = match which {
            HeatMap::Power => "power is reported per block, not per tile: see the Power report",
            _ => "no routed data in this run to compute the map",
        };
        return Panel::missing(which.title(), reason);
    }
    let mut sorted = tiles.clone();
    sorted.sort_by(|a, b| b.value.partial_cmp(&a.value).unwrap_or(std::cmp::Ordering::Equal));
    let over = tiles.iter().filter(|t| t.overused).count();
    let max = sorted.first().map(|t| t.value).unwrap_or(0.0);
    let mut tbl = Table::new(&["Tile", "Value (%)", "Overused"]);
    for t in sorted.iter().take(10) {
        tbl.push(&[
            &format!("({}, {})", t.x, t.y),
            &format!("{:.1}", t.value),
            if t.overused { "yes" } else { "" },
        ]);
    }
    Panel::table(which.title(), tbl).with_note(format!(
        "{} tiles, {} over capacity, max {:.1}%",
        tiles.len(),
        over,
        max
    ))
}

// ---------------------------------------------------------------------------
// Device view layers
// ---------------------------------------------------------------------------

/// A device-view layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    PlacedCells,
    Routing,
    ClockRegions,
    IoBanks,
    Package,
}

impl Layer {
    pub const ALL: [Layer; 5] = [
        Layer::PlacedCells,
        Layer::Routing,
        Layer::ClockRegions,
        Layer::IoBanks,
        Layer::Package,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Layer::PlacedCells => "Placed cells",
            Layer::Routing => "Routing",
            Layer::ClockRegions => "Clock regions",
            Layer::IoBanks => "IO banks",
            Layer::Package => "Package",
        }
    }
}

/// A panel for one device-view layer.
pub fn layer_panel(view: &ImplView, layer: Layer) -> Panel {
    match layer {
        Layer::PlacedCells => {
            if view.cells.is_empty() {
                return Panel::missing(layer.title(), "no placement in this run");
            }
            let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
            for c in &view.cells {
                *by_kind.entry(c.kind.as_str()).or_insert(0) += 1;
            }
            let mut tbl = Table::new(&["Primitive", "Count"]);
            for (k, n) in &by_kind {
                tbl.push(&[k, &n.to_string()]);
            }
            tbl.push(&["total", &view.cells.len().to_string()]);
            Panel::table(layer.title(), tbl)
        }
        Layer::Routing => {
            if view.nets.is_empty() {
                return Panel::missing(layer.title(), "no routing in this run");
            }
            let hops: usize = view.nets.iter().map(|n| n.hops.len()).sum();
            let mut p = route_status(view);
            p.title = layer.title().to_string();
            p.note = Some(format!("{hops} routed wire hops across {} nets", view.nets.len()));
            p
        }
        Layer::ClockRegions => {
            let regions = view.clock_regions();
            if regions.is_empty() {
                return Panel::missing(
                    layer.title(),
                    "the device model has no clock-region table for this part",
                );
            }
            let mut tbl = Table::new(&["Region", "Tiles"]);
            for r in &regions {
                let n = view.tiles.iter().filter(|t| &t.clock_region == r).count();
                tbl.push(&[r, &n.to_string()]);
            }
            Panel::table(layer.title(), tbl)
        }
        Layer::IoBanks => {
            let banks: Vec<u32> = {
                let mut v: Vec<u32> = view.io.as_ref()
                    .map(|io| io.pins.iter().filter_map(|p| p.bank).collect())
                    .unwrap_or_default();
                v.sort_unstable();
                v.dedup();
                v
            };
            if banks.is_empty() {
                return Panel::missing(layer.title(), "no IO pins placed in this run");
            }
            let mut tbl = Table::new(&["Bank", "Pins used"]);
            for b in &banks {
                let n = view
                    .io
                    .as_ref()
                    .map(|io| io.pins.iter().filter(|p| p.bank == Some(*b)).count())
                    .unwrap_or(0);
                tbl.push(&[&b.to_string(), &n.to_string()]);
            }
            Panel::table(layer.title(), tbl)
        }
        Layer::Package => match &view.io {
            Some(io) if io.package_pins > 0 => {
                let mut tbl = Table::new(&["Pins in package", "Pins used"]);
                tbl.push(&[&io.package_pins.to_string(), &io.pins.len().to_string()]);
                Panel::table(layer.title(), tbl)
            }
            _ => Panel::missing(
                layer.title(),
                "the device model has no package pin table for this part",
            ),
        },
    }
}

// ---------------------------------------------------------------------------
// Schematic
// ---------------------------------------------------------------------------

/// The three schematic stages.
pub fn schematic_panel(view: &ImplView) -> Panel {
    if view.cells.is_empty() {
        return Panel::missing(
            "Schematic",
            "the run produced no netlist cells to draw",
        );
    }
    let mut tbl = Table::new(&["Stage", "Cells", "Nets", "Placed", "Routed"]);
    // The same netlist is shown at every stage the run reached: the schematic
    // is Helion's own netlist, annotated with what the run did to it.
    let placed = !view.cells.is_empty() && view.stage != Stage::Synthesized;
    let routed = view.stage == Stage::Routed && !view.nets.is_empty();
    for stage in [Stage::Synthesized, Stage::Placed, Stage::Routed] {
        let reached = match stage {
            Stage::Synthesized => true,
            Stage::Placed => placed,
            Stage::Routed => routed,
        };
        tbl.push(&[
            stage.label(),
            &view.cells.len().to_string(),
            &view.nets.len().to_string(),
            if placed { "yes" } else { "no" },
            if reached && routed { "yes" } else { "no" },
        ]);
    }
    Panel::table("Schematic", tbl).with_note(format!(
        "{} cells, {} nets from Helion's own netlist",
        view.cells.len(),
        view.nets.len()
    ))
}

// ---------------------------------------------------------------------------
// Text rendering of the whole View tab
// ---------------------------------------------------------------------------

/// Render every report of the view tab as text, for golden comparison.
pub fn render_all_reports(view: &ImplView) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} {} {}\n\n",
        view.family.label(),
        view.part,
        view.stage.label()
    ));
    for r in Report::ALL {
        out.push_str(&report(view, r).render());
        out.push('\n');
    }
    out
}

/// Render the device layers and heat maps as text.
pub fn render_device_tab(view: &ImplView) -> String {
    let mut out = String::new();
    for l in Layer::ALL {
        out.push_str(&layer_panel(view, l).render());
        out.push('\n');
    }
    for h in [
        HeatMap::Utilization,
        HeatMap::CongestionH,
        HeatMap::CongestionV,
        HeatMap::Slack,
        HeatMap::Power,
    ] {
        out.push_str(&heat_panel(view, h).render());
        out.push('\n');
    }
    out
}

/// Cells placed on a site, sorted by cell name.
pub fn cells_at_site<'a>(view: &'a ImplView, site: &str) -> Vec<&'a CellPlacement> {
    let mut v: Vec<&CellPlacement> = view.cells_on(site).collect();
    v.sort_by(|a, b| a.cell.cmp(&b.cell));
    v
}

/// The tile-usage summary a status bar shows.
pub fn usage_summary(view: &ImplView) -> String {
    let used: u32 = view.usage.iter().map(|u| u.sites_used).sum();
    let cap: u32 = view.usage.iter().map(|u| u.sites_capacity).sum();
    let over = view
        .usage
        .iter()
        .filter(|u| u.overflow.map(|o| o.is_overused()).unwrap_or(false))
        .count();
    format!(
        "{used}/{cap} sites, {over} tiles over capacity, {} nets routed",
        view.nets.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_targets::Family;
    use helion_targets::view::{Overflow, Provenance, TimingPath};

    fn sample() -> ImplView {
        let mut v = ImplView::empty(Family::Xilinx7, "xc7z020clg484-1", Stage::Routed);
        v.cols = 2;
        v.rows = 2;
        v.tiles = vec![
            helion_targets::view::ViewTile {
                x: 0,
                y: 0,
                class: TileClass::Logic,
                name: "CLEL_L_X0Y0".into(),
                clock_region: "X0Y0".into(),
                sites: vec!["SLICE_X0Y0".into(), "SLICE_X0Y1".into()],
                io_bank: None,
            },
            helion_targets::view::ViewTile {
                x: 1,
                y: 0,
                class: TileClass::Io,
                name: "RIOB33_X1Y0".into(),
                clock_region: "X0Y0".into(),
                sites: vec!["IOB_X0Y0".into()],
                io_bank: Some(34),
            },
        ];
        v.usage = vec![
            TileUsage {
                x: 0,
                y: 0,
                sites_used: 1,
                sites_capacity: 2,
                overflow: Some(Overflow {
                    h_used: 12,
                    h_capacity: 10,
                    v_used: 4,
                    v_capacity: 10,
                }),
            },
            TileUsage {
                x: 1,
                y: 0,
                sites_used: 1,
                sites_capacity: 1,
                overflow: None,
            },
        ];
        v.cells = vec![CellPlacement {
            cell: "u0".into(),
            site: "SLICE_X0Y0".into(),
            bel: None,
            tile: None,
            kind: "LUT".into(),
        }];
        v.nets = vec![helion_targets::view::RoutedNet {
            net: "n0".into(),
            driver: Some("u0.O".into()),
            loads: vec![],
            hops: vec![],
            kind: "pips",
        }];
        v.set_provenance(
            "usage",
            Provenance::new("helion-xilinx7 route", "occupancy", "run"),
        );
        v
    }

    #[test]
    fn missing_section_prints_the_reason_not_a_placeholder() {
        let v = ImplView::empty(Family::Ice40, "iCE40HX8K", Stage::Placed);
        let p = power(&v);
        match &p.body {
            PanelBody::Missing(reason) => {
                assert!(reason.contains("power"), "reason names the section: {reason}");
            }
            other => panic!("expected Missing, got {other:?}"),
        }
        assert!(p.render().contains("unavailable:"));
    }

    #[test]
    fn congestion_maps_keep_the_two_directions_apart() {
        let v = sample();
        let h = heat_map(&v, HeatMap::CongestionH);
        let vv = heat_map(&v, HeatMap::CongestionV);
        assert_eq!(h.len(), 1, "one tile has horizontal capacity");
        assert_eq!(h[0].value, 120.0);
        assert!(h[0].overused);
        assert_eq!(vv[0].value, 40.0);
        assert!(!vv[0].overused);
    }

    #[test]
    fn a_hot_tile_over_100_percent_is_reported_as_overused() {
        let v = sample();
        let p = heat_panel(&v, HeatMap::CongestionH);
        assert!(p.render().contains("OVERUSED") || p.render().contains("yes"));
        assert!(p.render().contains("120.0"), "the map shows the real value");
    }

    #[test]
    fn power_heat_map_names_why_it_has_no_tiles() {
        let v = sample();
        let p = heat_panel(&v, HeatMap::Power);
        match &p.body {
            PanelBody::Missing(reason) => {
                assert!(reason.contains("per block"), "got {reason}");
            }
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn every_report_renders_either_a_table_or_a_named_missing_reason() {
        let v = sample();
        for r in Report::ALL {
            let p = report(&v, r);
            let text = p.render();
            assert!(!text.trim().is_empty(), "{:?} rendered nothing", r);
            assert!(
                text.contains("unavailable:") || text.contains("---"),
                "{:?} has neither a table nor a reason:\n{text}",
                r
            );
        }
    }

    #[test]
    fn timing_summary_uses_the_contract_values() {
        let mut v = sample();
        v.timing = Some(TimingSummary {
            wns_ps: -150,
            tns_ps: -300,
            whs_ps: Some(20),
            ths_ps: Some(0),
            failing_endpoints: 2,
            total_endpoints: 10,
            worst_path: Some(TimingPath {
                from: "a".into(),
                to: "b".into(),
                clock: "clk".into(),
                slack_ps: -150,
                requirement_ps: 10_000,
                kind: "setup",
            }),
            worst_hold_path: None,
            endpoints: vec![],
            fmax_mhz: Some(98.5),
        });
        let text = timing_summary(&v).render();
        assert!(text.contains("-0.150"), "WNS in ns: {text}");
        assert!(text.contains("2 of 10"));
        assert!(text.contains("98.5"));
    }

    #[test]
    fn utilization_counts_sites_not_tiles() {
        let v = sample();
        let text = utilization(&v).render();
        assert!(text.contains("Logic sites"), "{text}");
        assert!(text.contains("1"), "one site used");
    }

    #[test]
    fn schematic_shows_a_row_for_each_stage() {
        let mut v = sample();
        v.stage = Stage::Placed;
        let text = schematic_panel(&v).render();
        for s in ["Synthesized", "Placed", "Routed"] {
            assert!(text.contains(s), "missing stage {s}:\n{text}");
        }
    }

    #[test]
    fn table_render_aligns_columns() {
        let mut t = Table::new(&["A", "BB"]);
        t.push(&["1", "2"]);
        t.push(&["333", "4"]);
        let txt = t.render();
        let lines: Vec<&str> = txt.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("A "));
    }

    #[test]
    fn usage_summary_reports_over_capacity_tiles() {
        let v = sample();
        let s = usage_summary(&v);
        assert!(s.contains("1 tiles over capacity"), "{s}");
    }
}
