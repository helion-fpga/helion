//! The implementation view contract: one family-neutral description of an
//! implemented design that every view and report in the IDE reads.
//!
//! The rule this module exists to enforce: **a view never talks to a backend.**
//! A backend fills an [`ImplView`]; the Device view, the heat maps, the clock
//! utilization panel and every report read only this struct. That is what lets
//! a new family (or a new engine inside a family) show up in the GUI without a
//! separate code path per view, and it is why a family-specific extra such as
//! the Zynq PS7 is an optional layer ([`Layers::panels`]) rather than a branch
//! inside a view.
//!
//! Every field carries its [`Provenance`]: which engine produced it and how
//! accurate it is. A view that shows a number can therefore say where it came
//! from, which matters where Helion's model is deliberately simpler than the
//! vendor's (for example the flat LUT delay model, or a power estimate with no
//! activity data).
//!
//! Nothing here is family-specific except inside [`Layers`], and nothing here
//! invents a value: a filler that cannot compute a field leaves the section
//! empty and records why in [`ImplView::missing`], so a view can name the exact
//! missing input instead of drawing a placeholder.
//!
//! No em dashes anywhere.

use std::collections::BTreeMap;

use crate::Family;

/// Where a value came from, and how far to trust it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provenance {
    /// Engine that computed it, for example `"helion-xilinx7 place"`,
    /// `"helion-sta"`, `"helion-ice40 pack"`.
    pub engine: String,
    /// One line on the method or its accuracy, for example
    /// `"PathFinder occupancy after the final iteration"` or
    /// `"flat LUT delay model (124 ps/pin)"`.
    pub method: String,
    /// What the number is measured against, for example the routed design's
    /// checkpoint name or the run id.
    pub scope: String,
}

impl Provenance {
    pub fn new(engine: impl Into<String>, method: impl Into<String>, scope: impl Into<String>) -> Self {
        Self {
            engine: engine.into(),
            method: method.into(),
            scope: scope.into(),
        }
    }
}

/// Which engine stage a snapshot describes. The Schematic view and the reports
/// both key off this: a synthesized view has cells but no placement, an
/// implemented view has all three.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// After synthesis: cells and nets, no placement.
    Synthesized,
    /// After placement: sites assigned, no routing.
    Placed,
    /// After routing: sites and wires.
    Routed,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Stage::Synthesized => "Synthesized",
            Stage::Placed => "Placed",
            Stage::Routed => "Routed",
        }
    }
}

/// Tile class, family neutral. Families map their own site kinds onto these;
/// [`TileClass::Other`] keeps a family-specific tile visible instead of
/// dropping it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TileClass {
    Logic,
    Ram,
    Dsp,
    Io,
    Clock,
    /// Configuration, routing-only, power, or anything a family does not
    /// classify further.
    Other,
}

impl TileClass {
    pub fn label(self) -> &'static str {
        match self {
            TileClass::Logic => "Logic",
            TileClass::Ram => "Block RAM",
            TileClass::Dsp => "DSP",
            TileClass::Io => "I/O",
            TileClass::Clock => "Clock",
            TileClass::Other => "Other",
        }
    }
}

/// One tile of the die, with its class, clock region and the sites it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewTile {
    pub x: u32,
    pub y: u32,
    pub class: TileClass,
    /// Family tile name (`INT_L_X10Y100`, `X2Y5`, `TRELLIS_IO` position), used
    /// to join the routing layer back onto the grid.
    pub name: String,
    /// Clock region name, or empty when the family has none or the tile is
    /// outside every region.
    pub clock_region: String,
    /// Sites in this tile, by name. Empty for routing-only tiles.
    pub sites: Vec<String>,
    /// IO bank, when the tile is an IO tile.
    pub io_bank: Option<u32>,
}

/// Maps a cell name from the design netlist onto a physical site and BEL.
///
/// `bel` is optional because not every family distinguishes BELs; when a
/// family has them (`A6LUT`, `SLICE_X0Y0.AFF`) the schematic and the placement
/// inspector can point at the exact element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellPlacement {
    pub cell: String,
    pub site: String,
    pub bel: Option<String>,
    /// The tile the site lives in, when the backend knows it directly.
    pub tile: Option<String>,
    /// The primitive kind, for colour coding and schematic grouping.
    pub kind: String,
}

/// One hop of a routed net: a wire, optionally entered through a PIP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireHop {
    /// Tile the wire belongs to.
    pub tile: String,
    /// Wire name inside that tile.
    pub wire: String,
    /// The PIP that enters this wire from the previous hop: `(src_wire,
    /// dst_wire)` within `tile`. Absent on the first hop.
    pub pip: Option<(String, String)>,
    /// True for a pseudo PIP (a site-pin or bounce, not routing fabric).
    pub is_pseudo: bool,
}

/// A routed net: its driver, its loads and the exact path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedNet {
    pub net: String,
    /// `cell.pin` of the driver, when known.
    pub driver: Option<String>,
    /// `cell.pin` of each load.
    pub loads: Vec<String>,
    /// The ordered wire hops. Empty when the net is site-internal or a
    /// dedicated cascade (recorded in `kind`).
    pub hops: Vec<WireHop>,
    /// How the net is implemented: `"pips"`, `"site-internal"`,
    /// `"clock-spine"`, `"cascade"`, `"cascade-dedicated"`, `"unrouted"`.
    pub kind: &'static str,
}

/// Per-tile resource usage against capacity, horizontal and vertical kept
/// separate exactly as the vendor's congestion map does.
///
/// Values are **not clamped at 1.0**: a hot tile reads above 100%, which is
/// the whole point of a congestion map. The two directions can differ by
/// several times on a real design, so [`Overflow::overflow`] reports each
/// separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Overflow {
    /// Horizontal (east/west) wires used.
    pub h_used: u32,
    /// Horizontal wires available.
    pub h_capacity: u32,
    /// Vertical (north/south) wires used.
    pub v_used: u32,
    /// Vertical wires available.
    pub v_capacity: u32,
}

impl Overflow {
    /// Horizontal usage as a fraction of capacity, in percent. Over 100 when
    /// overused. Returns None when the tile has no horizontal capacity.
    pub fn h_percent(&self) -> Option<f32> {
        if self.h_capacity == 0 {
            None
        } else {
            Some(100.0 * self.h_used as f32 / self.h_capacity as f32)
        }
    }

    pub fn v_percent(&self) -> Option<f32> {
        if self.v_capacity == 0 {
            None
        } else {
            Some(100.0 * self.v_used as f32 / self.v_capacity as f32)
        }
    }

    pub fn is_overused(&self) -> bool {
        self.h_used > self.h_capacity || self.v_used > self.v_capacity
    }

    /// The worse of the two directions, for a single-value heat map colour.
    pub fn worst_percent(&self) -> f32 {
        self.h_percent()
            .unwrap_or(0.0)
            .max(self.v_percent().unwrap_or(0.0))
    }
}

/// Per-tile usage and capacity for the utilization and congestion layers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileUsage {
    /// Tile coordinates, matching [`ViewTile`].
    pub x: u32,
    pub y: u32,
    /// Logic sites occupied and available in this tile.
    pub sites_used: u32,
    pub sites_capacity: u32,
    /// Routing overflow, present when the backend routes on a wire graph.
    pub overflow: Option<Overflow>,
}

impl TileUsage {
    pub fn utilisation_percent(&self) -> f32 {
        if self.sites_capacity == 0 {
            0.0
        } else {
            100.0 * self.sites_used as f32 / self.sites_capacity as f32
        }
    }
}

/// One row of the utilization report in the vendor's own terms (`Slice
/// LUTs`, `F7 Muxes`, `Block RAM Tile`), so a reader can hold it against
/// the vendor report line by line. `depth` is the row's indent level.
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceRow {
    pub name: String,
    pub depth: u8,
    /// Used count; fractional where the vendor counts halves (a RAMB18 is
    /// half a block RAM tile).
    pub used: f64,
    /// Sites of that kind on the part; 0 for a breakdown row with none.
    pub available: u32,
}

/// The congestion level of one routing direction, as Vivado's "Maximum
/// Level Congestion" table states it: the side of the largest square
/// window (`2^level` tiles) whose average use reaches the threshold, that
/// window's average, and its corner tiles. Level 0 means no window of two
/// tiles or more reaches it; the window is then the hottest single tile.
/// Distinct from the per-tile value on purpose (DECOMPILE_NOTES 14.2).
#[derive(Clone, Debug, PartialEq)]
pub struct CongestionWindow {
    /// `"Horizontal"` (east/west wires) or `"Vertical"` (north/south).
    pub direction: &'static str,
    pub level: u8,
    /// Window average use, percent; may exceed 100.
    pub percent: f32,
    pub from_tile: String,
    pub to_tile: String,
    /// The window is over capacity (above 100%).
    pub overlap: bool,
}

/// Find the congestion level over a grid of `(used, capacity)` per
/// `(x, y)`: for each window side `2^l`, `l` in `1..=max_level`, the window
/// with the highest average (summed use over summed capacity) anywhere on
/// the grid; the level is the largest `l` whose best window reaches
/// `threshold_pct`. Returns `(level, percent, (x0, y0, x1, y1))`, or None
/// for an empty grid.
pub fn congestion_level(grid: &BTreeMap<(u32, u32), (u32, u32)>, threshold_pct: f32, max_level: u8) -> Option<(u8, f32, (u32, u32, u32, u32))> {
    let pct = |u: u64, c: u64| if c == 0 { 0.0 } else { 100.0 * u as f32 / c as f32 };
    let hottest = grid
        .iter()
        .filter(|(_, v)| v.1 > 0)
        .map(|(&(x, y), &(u, c))| (pct(u as u64, c as u64), (x, y, x, y)))
        .max_by(|a, b| a.0.total_cmp(&b.0))?;
    let w = grid.keys().map(|k| k.0).max()? as usize + 1;
    let h = grid.keys().map(|k| k.1).max()? as usize + 1;
    // 2-D prefix sums of use and capacity.
    let mut su = vec![0u64; (w + 1) * (h + 1)];
    let mut sc = su.clone();
    let at = |x: usize, y: usize| y * (w + 1) + x;
    for y in 0..h {
        for x in 0..w {
            let (u, c) = grid.get(&(x as u32, y as u32)).copied().unwrap_or((0, 0));
            su[at(x + 1, y + 1)] = u as u64 + su[at(x, y + 1)] + su[at(x + 1, y)] - su[at(x, y)];
            sc[at(x + 1, y + 1)] = c as u64 + sc[at(x, y + 1)] + sc[at(x + 1, y)] - sc[at(x, y)];
        }
    }
    let rect = |s: &Vec<u64>, x0: usize, y0: usize, n: usize| s[at(x0 + n, y0 + n)] + s[at(x0, y0)] - s[at(x0 + n, y0)] - s[at(x0, y0 + n)];
    let mut best = (0u8, hottest.0, hottest.1);
    for l in 1..=max_level {
        let n = 1usize << l;
        if n > w || n > h {
            break;
        }
        let mut top: Option<(f32, (u32, u32, u32, u32))> = None;
        for y0 in 0..=h - n {
            for x0 in 0..=w - n {
                let p = pct(rect(&su, x0, y0, n), rect(&sc, x0, y0, n));
                if top.is_none_or(|t| p > t.0) {
                    top = Some((p, (x0 as u32, y0 as u32, (x0 + n - 1) as u32, (y0 + n - 1) as u32)));
                }
            }
        }
        match top {
            Some((p, win)) if p >= threshold_pct => best = (l, p, win),
            _ => {}
        }
    }
    Some(best)
}

/// Slack at one timing endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointSlack {
    /// Endpoint name, `cell.pin` where available.
    pub endpoint: String,
    /// Slack in picoseconds. Negative is a violation.
    pub slack_ps: i64,
    /// Clock domain the endpoint belongs to.
    pub clock: String,
    /// Site the endpoint sits on, for the slack heat map.
    pub site: Option<String>,
    pub x: Option<u32>,
    pub y: Option<u32>,
}

/// One timing path, for the timing summary and the schematic highlight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimingPath {
    pub from: String,
    pub to: String,
    pub clock: String,
    pub slack_ps: i64,
    pub requirement_ps: i64,
    /// `"setup"` or `"hold"`.
    pub kind: &'static str,
}

/// Timing summary, computed for this run.
#[derive(Clone, Debug, PartialEq)]
pub struct TimingSummary {
    /// Worst setup slack, ps; positive when every endpoint meets.
    pub wns_ps: i64,
    /// Total negative setup slack over all endpoints, ps (0 when none fails).
    pub tns_ps: i64,
    /// Worst hold slack, ps. None when the engine has no hold analysis; the
    /// reason is in [`ImplView::missing`] under `"hold"`.
    pub whs_ps: Option<i64>,
    /// Total hold slack violation, ps. None with `whs_ps`.
    pub ths_ps: Option<i64>,
    /// Number of failing endpoints.
    pub failing_endpoints: usize,
    pub total_endpoints: usize,
    /// Worst path, when one exists.
    pub worst_path: Option<TimingPath>,
    /// Worst hold path, when one exists.
    pub worst_hold_path: Option<TimingPath>,
    /// Per-endpoint slack, for the slack heat map.
    pub endpoints: Vec<EndpointSlack>,
    /// Achieved frequency, MHz, when the worst path is periodic.
    pub fmax_mhz: Option<f64>,
}

/// One clock net and what it drives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockNet {
    pub name: String,
    /// Period in ps, as constrained.
    pub period_ps: u32,
    /// Number of loads (flop clock pins, plus any gated branches).
    pub loads: usize,
    /// Clock regions the net reaches.
    pub regions: Vec<String>,
    /// Global buffer driving it, when one is placed.
    pub buffer: Option<String>,
}

/// Clock utilization, for the clock panel and the clock-region layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockUtilization {
    pub clocks: Vec<ClockNet>,
    /// Loads per clock region, sorted by region.
    pub loads_by_region: BTreeMap<String, usize>,
    /// Global clock buffers used and available.
    pub bufgs_used: usize,
    pub bufgs_available: usize,
}

/// One IO pin: its port, package pin, bank and level standard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IoPin {
    pub port: String,
    pub package_pin: String,
    pub bank: Option<u32>,
    pub iostandard: String,
    /// `"input"`, `"output"`, `"inout"`.
    pub direction: &'static str,
}

/// IO planning data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IoPlan {
    pub pins: Vec<IoPin>,
    /// Pins in the package, when the device model knows the package.
    pub package_pins: usize,
}

/// Power of one resource class (Vivado's "On-Chip Components" rows).
#[derive(Clone, Debug, PartialEq)]
pub struct PowerBlock {
    pub block: String,
    /// How many of the class the estimate counted (LUTs, flops, routed
    /// wires, clock loads, ...); 0 for device static power.
    pub used: usize,
    pub total_mw: f64,
}

/// Dynamic power attributed to one tile: each placed cell's power in its
/// own tile, each routed wire's in the tile that drives it.
#[derive(Clone, Debug, PartialEq)]
pub struct TilePower {
    pub x: u32,
    pub y: u32,
    pub mw: f64,
}

/// Power estimate for the design. `method` states how it was derived and
/// how far it is from the vendor's numbers, because without switching
/// activity this is a vectorless estimate.
#[derive(Clone, Debug, PartialEq)]
pub struct PowerEstimate {
    pub total_mw: f64,
    pub static_mw: f64,
    pub dynamic_mw: f64,
    /// Toggle rate assumed for data nets (0.125 = 12.5% per clock).
    pub toggle_rate: f64,
    pub blocks: Vec<PowerBlock>,
    pub tiles: Vec<TilePower>,
    pub method: String,
}

/// A control set: the clock, enable and set/reset nets of a flip-flop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlSet {
    /// `(clock, enable, set/reset, set/reset value)`; empty string for an
    /// absent control.
    pub key: (String, String, String, String),
    /// Flip-flops on this control set.
    pub flops: Vec<String>,
}

/// A high fanout net.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighFanoutNet {
    pub net: String,
    pub fanout: usize,
    /// True when the net drives a clock buffer rather than fabric loads.
    pub is_clock: bool,
}

/// A design rule check result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrcViolation {
    /// Rule id, for example `"IO-BANK-OVERUSED"`.
    pub rule: String,
    pub severity: &'static str,
    pub message: String,
}

/// The ZedBoard (or any board) panel: the physical bits the XDC names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardPanel {
    pub board: String,
    pub groups: Vec<BoardGroup>,
}

/// A group of board features of one kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardGroup {
    /// `"LED"`, `"Switch"`, `"Button"`, `"Pmod"`, `"OLED"`, `"HDMI"`,
    /// `"Audio"`, `"Clock"`.
    pub kind: String,
    pub items: Vec<BoardItem>,
}

/// One board feature and what it connects to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardItem {
    /// Label from the board's XDC (`led[3]`, `sw[0]`).
    pub label: String,
    /// Package pin.
    pub pin: String,
    /// The design port it is wired to, when used.
    pub port: Option<String>,
}

/// Family-specific extra layers. A view enables these when present and never
/// asks whether a family "supports" them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layers {
    /// Panels keyed by name, for example `"ps7"` (Zynq processing system) or
    /// `"ecp5-dsp"`. Each is a ready-to-render title and its rows, so a view
    /// needs no knowledge of the family.
    pub panels: BTreeMap<String, Vec<(String, String)>>,
}

/// The whole view contract for one implementation run.
#[derive(Clone, Debug)]
pub struct ImplView {
    /// Family and part this describes.
    pub family: Family,
    pub part: String,
    /// How far the run got.
    pub stage: Stage,
    /// Die tiles.
    pub tiles: Vec<ViewTile>,
    /// Grid extent.
    pub cols: u32,
    pub rows: u32,
    /// True when row 0 is the top edge.
    pub row0_top: bool,
    /// Placed cells.
    pub cells: Vec<CellPlacement>,
    /// Routed nets.
    pub nets: Vec<RoutedNet>,
    /// Per-tile usage and capacity.
    pub usage: Vec<TileUsage>,
    /// Utilization rows in the family's vendor terms.
    pub resources: Vec<ResourceRow>,
    /// Congestion level per routing direction.
    pub congestion: Vec<CongestionWindow>,
    /// Timing summary, when the run has timing.
    pub timing: Option<TimingSummary>,
    /// Clock utilization.
    pub clocks: Option<ClockUtilization>,
    /// IO plan.
    pub io: Option<IoPlan>,
    /// Power estimate.
    pub power: Option<PowerEstimate>,
    /// Control sets.
    pub control_sets: Vec<ControlSet>,
    /// High fanout nets.
    pub high_fanout: Vec<HighFanoutNet>,
    /// DRC results.
    pub drc: Vec<DrcViolation>,
    /// Board panel, when the project pins a known board.
    pub board: Option<BoardPanel>,
    /// Family-specific extra layers.
    pub layers: Layers,
    /// Provenance per section, keyed by section name (`"placement"`,
    /// `"timing"`, `"power"`, ...).
    pub provenance: BTreeMap<String, Provenance>,
    /// Why a section is empty, keyed by section name. A view shows this text
    /// instead of a placeholder, so the reader learns the exact missing input.
    pub missing: BTreeMap<String, String>,
}

impl ImplView {
    /// An empty view for a part, at a stage. Fillers start here and fill what
    /// they can compute.
    pub fn empty(family: Family, part: impl Into<String>, stage: Stage) -> Self {
        Self {
            family,
            part: part.into(),
            stage,
            tiles: Vec::new(),
            cols: 0,
            rows: 0,
            row0_top: false,
            cells: Vec::new(),
            nets: Vec::new(),
            usage: Vec::new(),
            resources: Vec::new(),
            congestion: Vec::new(),
            timing: None,
            clocks: None,
            io: None,
            power: None,
            control_sets: Vec::new(),
            high_fanout: Vec::new(),
            drc: Vec::new(),
            board: None,
            layers: Layers::default(),
            provenance: BTreeMap::new(),
            missing: BTreeMap::new(),
        }
    }

    /// Record that a section could not be computed, and why.
    pub fn mark_missing(&mut self, section: &str, reason: impl Into<String>) {
        self.missing.insert(section.to_string(), reason.into());
    }

    /// Record where a section's values came from.
    pub fn set_provenance(&mut self, section: &str, p: Provenance) {
        self.provenance.insert(section.to_string(), p);
    }

    /// Tiles of one class.
    pub fn tiles_of(&self, class: TileClass) -> impl Iterator<Item = &ViewTile> {
        self.tiles.iter().filter(move |t| t.class == class)
    }

    /// The usage record for a tile, if the backend computed one.
    pub fn usage_at(&self, x: u32, y: u32) -> Option<&TileUsage> {
        self.usage.iter().find(|u| u.x == x && u.y == y)
    }

    /// Cells placed on a site.
    pub fn cells_on(&self, site: &str) -> impl Iterator<Item = &CellPlacement> {
        self.cells.iter().filter(move |c| c.site == site)
    }

    /// The distinct clock region names, sorted.
    pub fn clock_regions(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .tiles
            .iter()
            .filter(|t| !t.clock_region.is_empty())
            .map(|t| t.clock_region.clone())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Hottest tile by worst-direction utilization, for the congestion
    /// summary. Returns the tile and its record.
    pub fn hottest_tile(&self) -> Option<(&TileUsage, f32)> {
        self.usage
            .iter()
            .filter_map(|u| u.overflow.map(|o| (u, o.worst_percent())))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_percentages_may_exceed_100_and_directions_stay_separate() {
        // A hot tile reads above capacity, and the two directions differ
        // several times over: this is the shape a real congestion map has.
        let o = Overflow {
            h_used: 144,
            h_capacity: 100,
            v_used: 55,
            v_capacity: 100,
        };
        assert_eq!(o.h_percent(), Some(144.0));
        assert_eq!(o.v_percent(), Some(55.0));
        assert!(o.is_overused(), "over capacity is overused");
        assert_eq!(o.worst_percent(), 144.0, "the worse direction wins");
    }

    #[test]
    fn overflow_with_no_capacity_reports_none_not_zero() {
        // A tile with no vertical wires must not read "0% used": that would
        // paint it as free. It reports None and a view omits the band.
        let o = Overflow {
            h_used: 3,
            h_capacity: 10,
            v_used: 0,
            v_capacity: 0,
        };
        assert_eq!(o.v_percent(), None);
        assert_eq!(o.worst_percent(), 30.0);
    }

    #[test]
    fn missing_sections_are_named_not_blank() {
        let mut v = ImplView::empty(Family::Ice40, "iCE40HX8K", Stage::Placed);
        v.mark_missing("power", "no activity file and no capacitance table for this family");
        assert!(v.power.is_none());
        assert_eq!(
            v.missing.get("power").map(String::as_str),
            Some("no activity file and no capacitance table for this family")
        );
    }

    #[test]
    fn empty_view_has_no_values_that_look_computed() {
        let v = ImplView::empty(Family::Ecp5, "LFE5U-25F", Stage::Synthesized);
        assert!(v.tiles.is_empty());
        assert!(v.usage.is_empty());
        assert!(v.timing.is_none());
        assert!(v.clocks.is_none());
        assert!(v.io.is_none());
        assert!(v.power.is_none());
        assert!(v.board.is_none());
        assert!(v.clocks.is_none(), "no clock panel from no data");
    }

    #[test]
    fn clock_regions_are_deduped_and_sorted() {
        let mut v = ImplView::empty(Family::Ice40, "part", Stage::Placed);
        v.tiles.push(ViewTile {
            x: 0,
            y: 0,
            class: TileClass::Logic,
            name: "t0".into(),
            clock_region: "X1Y0".into(),
            sites: vec![],
            io_bank: None,
        });
        v.tiles.push(ViewTile {
            x: 1,
            y: 0,
            class: TileClass::Logic,
            name: "t1".into(),
            clock_region: "X0Y0".into(),
            sites: vec![],
            io_bank: None,
        });
        v.tiles.push(ViewTile {
            x: 2,
            y: 0,
            class: TileClass::Logic,
            name: "t2".into(),
            clock_region: "X1Y0".into(),
            sites: vec![],
            io_bank: None,
        });
        v.tiles.push(ViewTile {
            x: 3,
            y: 0,
            class: TileClass::Other,
            name: "t3".into(),
            clock_region: String::new(),
            sites: vec![],
            io_bank: None,
        });
        assert_eq!(v.clock_regions(), vec!["X0Y0".to_string(), "X1Y0".to_string()]);
    }

    #[test]
    fn hottest_tile_picks_the_worst_direction() {
        let mut v = ImplView::empty(Family::Ice40, "part", Stage::Routed);
        v.usage.push(TileUsage {
            x: 0,
            y: 0,
            sites_used: 1,
            sites_capacity: 4,
            overflow: Some(Overflow {
                h_used: 10,
                h_capacity: 10,
                v_used: 2,
                v_capacity: 10,
            }),
        });
        v.usage.push(TileUsage {
            x: 1,
            y: 0,
            sites_used: 3,
            sites_capacity: 4,
            overflow: Some(Overflow {
                h_used: 5,
                h_capacity: 10,
                v_used: 13,
                v_capacity: 10,
            }),
        });
        let (tile, pct) = v.hottest_tile().expect("a hot tile");
        assert_eq!((tile.x, tile.y), (1, 0));
        assert_eq!(pct, 130.0);
    }

    #[test]
    fn congestion_level_is_the_largest_window_at_threshold_not_the_hottest_tile() {
        // A lone 150% tile is level 0; a 4x4 block at 90% is level 2 even
        // though no tile in it is as hot as the lone one.
        let mut g = BTreeMap::new();
        for x in 0..16 {
            for y in 0..16 {
                g.insert((x, y), (10, 100));
            }
        }
        g.insert((0, 0), (150, 100));
        assert_eq!(congestion_level(&g, 85.0, 6), Some((0, 150.0, (0, 0, 0, 0))));
        for x in 8..12 {
            for y in 4..8 {
                g.insert((x, y), (90, 100));
            }
        }
        let (level, pct, win) = congestion_level(&g, 85.0, 6).unwrap();
        assert_eq!((level, pct, win), (2, 90.0, (8, 4, 11, 7)));
        // Over capacity across the window: the percentage passes 100.
        for x in 8..12 {
            for y in 4..8 {
                g.insert((x, y), (130, 100));
            }
        }
        assert_eq!(congestion_level(&g, 85.0, 6).unwrap().1, 130.0);
        assert_eq!(congestion_level(&BTreeMap::new(), 85.0, 6), None);
    }

    #[test]
    fn provenance_records_engine_and_method() {
        let mut v = ImplView::empty(Family::Xilinx7, "xc7z020clg484-1", Stage::Routed);
        v.set_provenance(
            "timing",
            Provenance::new("helion-sta", "routes then STA at the constrained period", "run-7"),
        );
        let p = v.provenance.get("timing").expect("timing provenance");
        assert_eq!(p.engine, "helion-sta");
        assert_eq!(p.scope, "run-7");
    }
}
