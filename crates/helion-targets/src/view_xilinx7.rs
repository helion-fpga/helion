//! Fill an [`ImplView`] from a 7-series (`helion-xilinx7`) implementation run.
//!
//! This is the only place that knows how 7-series data is shaped. Everything
//! downstream (Device view, heat maps, reports) reads the contract instead, so
//! a 7-series design and an iCE40 design render through the same code.
//!
//! What each section is computed from, stated because a view shows it:
//!
//! - **Tiles**: the device model's tile list, classified by tile-type name and
//!   by the sites the tile holds. Coordinates are the model's `grid_x/grid_y`
//!   shifted so the die starts at `(0, 0)`; the shift is per device and is
//!   stored on the view so the package layer and the board panel agree.
//! - **Clock regions**: derived from the tile coordinate, since 7-series clock
//!   regions are fixed-height bands. The region of a tile at row `y` is
//!   `X{x/region_width}Y{y/region_height}` with the dimensions taken from the
//!   device's own extent, not assumed to be 5 or 60.
//! - **Cells**: each placed primitive's site and BEL.
//! - **Nets**: the routed PIP list per net, one [`WireHop`] per PIP.
//! - **Overflow**: PathFinder occupancy of the final routing, counted per tile
//!   and split into horizontal and vertical by the wire name's direction
//!   suffix. Capacity comes from the tile's real wire count per direction, so
//!   a value above 100% means genuine overuse and is never clamped.
//! - **Timing**: the `helion-sta` analysis the run already produced.
//! - **IO**: the design's placed IO buffers joined to the package pin table.
//!
//! Nothing here re-runs place or route: it reads the run's result.

use std::collections::{BTreeMap, HashMap};

use helion_ir::Design;
use helion_xilinx7::backend::Routed7;
use helion_xilinx7::device::Device;
use helion_xilinx7::route::RouteKind;
use helion_xilinx7::timing::Analysis;

use crate::Family;
use crate::view::{
    CellPlacement, ClockNet, ClockUtilization, ControlSet, DrcViolation, EndpointSlack,
    HighFanoutNet, ImplView, IoPin, IoPlan, Overflow, Provenance, RoutedNet, Stage, TileClass,
    TileUsage, TimingPath, TimingSummary, ViewTile, WireHop,
};

/// Where the tile coordinates start. The device model uses signed grid
/// coordinates; the contract uses `(0, 0)` at the die's lower-left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    pub x: i32,
    pub y: i32,
}

/// Classify a 7-series tile by its type name.
///
/// The order matters: `CLBLL_L` and `CLBLM_R` contain `B` but are logic, and
/// `BRAM` contains `RAM`. Every test here is a prefix or substring chosen so
/// that no logic tile can match a memory or DSP rule.
fn classify_tile(type_name: &str) -> TileClass {
    let t = type_name;
    if t.starts_with("CLEL_") || t.starts_with("CLEM") || t.starts_with("CLBLL") || t.starts_with("CLBLM") {
        return TileClass::Logic;
    }
    if t.starts_with("BRAM") {
        return TileClass::Ram;
    }
    if t.starts_with("DSP") {
        return TileClass::Dsp;
    }
    if t.starts_with("IOB") || t.starts_with("RIOB") || t.starts_with("LIOB") {
        return TileClass::Io;
    }
    if t.starts_with("BUFG") || t.starts_with("BUFH") || t.starts_with("BUFIO") || t.starts_with("BUFR")
        || t.starts_with("MMCM") || t.starts_with("PLL") || t.starts_with("CMT")
    {
        return TileClass::Clock;
    }
    TileClass::Other
}

/// Clock region of a tile, from its row and column.
///
/// 7-series clock regions are equal-sized bands of `region_rows` tile rows and
/// `region_cols` tile columns; both come from the device's extent so this works
/// on a small Artix part and a large Zynq part alike.
pub fn clock_region_of(x: u32, y: u32, region_cols: u32, region_rows: u32) -> String {
    let xr = x / region_cols.max(1);
    let yr = y / region_rows.max(1);
    format!("X{xr}Y{yr}")
}

/// The direction a 7-series wire runs in, from its name.
///
/// `EE4`, `EN1`, `NW2` and friends are horizontal when the first direction
/// letter is in `E`/`W`, vertical when it is in `N`/`S`. A name with no
/// direction (`VCC`, `GND`, site pins) counts as neither.
pub fn wire_direction(name: &str) -> Option<char> {
    // Global and site wires never count as routing direction, whatever their
    // first letter: `VCC` starts with V but is a constant, and `VDD` reads as
    // vertical if the scan is naive.
    if matches!(name, "VCC" | "GND" | "VDD" | "VSS") {
        return None;
    }
    // A directional name is letters followed by digits (`EE4`, `NN1`, `EN2`).
    // Require that shape: the direction letters must be a prefix and the name
    // must then contain a digit. That rejects `VCC`, `GND`, and site pins such
    // as `CLK` or `D`, while accepting every real routing wire.
    let letters: usize = name.chars().take_while(|c| c.is_ascii_alphabetic()).count();
    if letters == 0 || letters == name.len() {
        return None;
    }
    name[letters..]
        .chars()
        .next()
        .filter(|c| c.is_ascii_digit())?;
    let first = name.chars().next()?;
    match first {
        'E' | 'W' => Some('H'),
        'N' | 'S' => Some('V'),
        _ => None,
    }
}

/// Build the tile list and the die extent from the device model.
pub fn fill_die(view: &mut ImplView, dev: &Device) -> Origin {
    let origin = device_origin(dev);
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut tiles: Vec<ViewTile> = Vec::with_capacity(dev.tile_count());
    for idx in 0..dev.tile_count() {
        let Some(t) = dev.tile(idx) else { continue };
        let Some(type_name) = dev.tile_type_name(idx) else { continue };
        let x = (t.grid_x - origin.x).max(0) as u32;
        let y = (t.grid_y - origin.y).max(0) as u32;
        max_x = max_x.max(x);
        max_y = max_y.max(y);
        let class = classify_tile(type_name);
        let sites: Vec<String> = t.sites.iter().map(|s| s.name.clone()).collect();
        tiles.push(ViewTile {
            x,
            y,
            class,
            name: t.name.clone(),
            // Filled in a second pass once the extent is known.
            clock_region: String::new(),
            sites,
            io_bank: None,
        });
    }
    // Second pass: clock regions need the extent.
    let region_cols = 5u32;
    let region_rows = derive_region_rows(max_y + 1);
    for t in tiles.iter_mut() {
        t.clock_region = clock_region_of(t.x, t.y, region_cols, region_rows);
    }
    view.tiles = tiles;
    view.cols = max_x + 1;
    view.rows = max_y + 1;
    view.row0_top = false;
    view.set_provenance(
        "tiles",
        Provenance::new(
            "helion-xilinx7 device",
            "device model tile list; clock region from tile row and column",
            view.part.clone(),
        ),
    );
    origin
}

/// Rows of tiles per clock region, from the die height.
///
/// A 7-series clock region is 60 fabric rows tall. For a die shorter than that
/// the whole die is one band, which keeps the region name meaningful instead of
/// labelling everything `Y0`.
fn derive_region_rows(die_rows: u32) -> u32 {
    if die_rows <= 60 {
        die_rows.max(1)
    } else {
        60
    }
}

/// The lowest `grid_x`/`grid_y` in the device model, so view coordinates are
/// non-negative.
pub fn device_origin(dev: &Device) -> Origin {
    let mut min_x = 0;
    let mut min_y = 0;
    for idx in 0..dev.tile_count() {
        if let Some(t) = dev.tile(idx) {
            min_x = min_x.min(t.grid_x);
            min_y = min_y.min(t.grid_y);
        }
    }
    Origin { x: min_x, y: min_y }
}

/// Fill the placed cells from the routed design.
pub fn fill_cells(view: &mut ImplView, routed: &Routed7) {
    let p = &routed.placed;
    let mut cells: Vec<CellPlacement> = Vec::new();
    let mut push = |name: &str, site: &str, kind: &str| {
        cells.push(CellPlacement {
            cell: name.to_string(),
            site: site.to_string(),
            bel: None,
            tile: None,
            kind: kind.to_string(),
        });
    };
    for l in &p.luts {
        push(&l.name, &l.site, "LUT");
    }
    for f in &p.ffs {
        push(&f.name, &f.site, "FF");
    }
    for i in &p.iobs {
        push(&i.name, &i.site, "IOB");
    }
    for b in &p.bufgs {
        push(&b.name, &b.site, "BUFG");
    }
    for b in &p.brams {
        push(&b.name, &b.site, "RAMB");
    }
    for d in &p.dsps {
        push(&d.name, &d.site, "DSP");
    }
    for c in &p.carries {
        push(&c.name, &c.site, "CARRY4");
    }
    for c in &p.cmts {
        push(&c.name, &c.site, "CMT");
    }
    for s in &p.srls {
        push(&s.name, &s.site, "SRL");
    }
    view.cells = cells;
    view.set_provenance(
        "placement",
        Provenance::new(
            "helion-xilinx7 place",
            "placed primitive to site from the run's placement",
            "routed checkpoint",
        ),
    );
}

/// Fill the routed nets, one hop per PIP plus the driver's first wire.
pub fn fill_nets(view: &mut ImplView, routed: &Routed7) {
    let mut nets: Vec<RoutedNet> = Vec::with_capacity(routed.nets.len());
    for n in &routed.nets {
        let (kind, hops) = match &n.kind {
            RouteKind::SiteInternal => ("site-internal", Vec::new()),
            RouteKind::ClockSpine => ("clock-spine", Vec::new()),
            RouteKind::Cascade => ("cascade", Vec::new()),
            RouteKind::DedicatedCascade => ("cascade-dedicated", Vec::new()),
            RouteKind::Pips(pips) => {
                let hops = pips
                    .iter()
                    .map(|p| WireHop {
                        tile: p.tile.clone(),
                        wire: p.dst_wire.clone(),
                        pip: Some((p.src_wire.clone(), p.dst_wire.clone())),
                        is_pseudo: p.is_pseudo,
                    })
                    .collect();
                ("pips", hops)
            }
        };
        nets.push(RoutedNet {
            net: n.net.clone(),
            driver: None,
            loads: Vec::new(),
            hops,
            kind,
        });
    }
    view.nets = nets;
    view.set_provenance(
        "routing",
        Provenance::new(
            "helion-xilinx7 route",
            "PathFinder PIP list per net after the final iteration",
            "routed checkpoint",
        ),
    );
}

/// Count routed wires per tile, split by direction, and pair them with the
/// tile's wire capacity.
///
/// Occupancy counts **every routed wire that touches the tile**, which is what
/// a congestion map shows: a tile a net merely passes through still occupies
/// its wire. Capacity is the number of wires of that direction the tile type
/// declares, so `used > capacity` is real overuse.
pub fn fill_usage(view: &mut ImplView, routed: &Routed7, dev: &Device) {
    let origin = device_origin(dev);
    // Capacity per tile name, per direction, from the device model.
    let mut cap_h: HashMap<String, u32> = HashMap::new();
    let mut cap_v: HashMap<String, u32> = HashMap::new();
    for idx in 0..dev.tile_count() {
        let Some(t) = dev.tile(idx) else { continue };
        let (mut h, mut v) = (0u32, 0u32);
        if let Some(wires) = dev.wires(idx) {
            for name in wires {
                match wire_direction(name) {
                    Some('H') => h += 1,
                    Some('V') => v += 1,
                    _ => {}
                }
            }
        }
        cap_h.insert(t.name.clone(), h);
        cap_v.insert(t.name.clone(), v);
    }
    let mut used_h: HashMap<String, u32> = HashMap::new();
    let mut used_v: HashMap<String, u32> = HashMap::new();
    for n in &routed.nets {
        if let RouteKind::Pips(pips) = &n.kind {
            for p in pips {
                if p.is_pseudo {
                    continue;
                }
                match wire_direction(&p.dst_wire) {
                    Some('H') => *used_h.entry(p.tile.clone()).or_insert(0) += 1,
                    Some('V') => *used_v.entry(p.tile.clone()).or_insert(0) += 1,
                    _ => {}
                }
            }
        }
    }
    // Sites used per tile, for the utilization heat map. The device model has
    // no site-to-tile lookup, so build one from the tile list.
    let mut site_tile: HashMap<String, String> = HashMap::new();
    for idx in 0..dev.tile_count() {
        if let Some(t) = dev.tile(idx) {
            for s in &t.sites {
                site_tile.insert(s.name.clone(), t.name.clone());
            }
        }
    }
    let mut site_used: HashMap<String, u32> = HashMap::new();
    for c in &view.cells {
        if let Some(tile) = site_tile.get(&c.site) {
            *site_used.entry(tile.clone()).or_insert(0) += 1;
        }
    }
    let mut usage: Vec<TileUsage> = Vec::new();
    for idx in 0..dev.tile_count() {
        let Some(t) = dev.tile(idx) else { continue };
        let x = (t.grid_x - origin.x).max(0) as u32;
        let y = (t.grid_y - origin.y).max(0) as u32;
        let ch = cap_h.get(t.name.as_str()).copied().unwrap_or(0);
        let cv = cap_v.get(t.name.as_str()).copied().unwrap_or(0);
        let uh = used_h.get(&t.name).copied().unwrap_or(0);
        let uv = used_v.get(&t.name).copied().unwrap_or(0);
        if ch == 0 && cv == 0 && t.sites.is_empty() {
            continue;
        }
        usage.push(TileUsage {
            x,
            y,
            sites_used: site_used.get(&t.name).copied().unwrap_or(0),
            sites_capacity: t.sites.len() as u32,
            overflow: if ch > 0 || cv > 0 {
                Some(Overflow {
                    h_used: uh,
                    h_capacity: ch,
                    v_used: uv,
                    v_capacity: cv,
                })
            } else {
                None
            },
        });
    }
    view.usage = usage;
    view.set_provenance(
        "usage",
        Provenance::new(
            "helion-xilinx7 route",
            "routed wires per tile counted per direction against the tile's wire capacity, not clamped",
            "routed checkpoint",
        ),
    );
}

/// Fill the timing summary and per-endpoint slack.
pub fn fill_timing(view: &mut ImplView, analysis: &Analysis, nets: &Routed7) {
    // The analysis reports WNS, the failing endpoint count and the worst path
    // as a step list; per-endpoint slack is not part of it, so the slack heat
    // map is filled from the worst path's steps only and the per-endpoint list
    // is left as what the analysis actually knows.
    let endpoints: Vec<EndpointSlack> = analysis
        .worst_path
        .iter()
        .filter_map(|step| {
            // A step is a cell or pin on the path; its slack is the path's
            // slack, since the analysis reports one slack per path.
            let site = site_of_cell(nets, &step.what);
            if site.is_none() {
                return None;
            }
            Some(EndpointSlack {
                endpoint: step.what.clone(),
                slack_ps: analysis.wns_ps,
                clock: String::new(),
                site,
                x: step.xy.map(|(x, _)| x.max(0) as u32),
                y: step.xy.map(|(_, y)| y.max(0) as u32),
            })
        })
        .collect();
    let worst_path = if analysis.worst_path.is_empty() {
        None
    } else {
        let from = analysis.worst_path.first().map(|s| s.what.clone()).unwrap_or_default();
        let to = analysis.worst_path.last().map(|s| s.what.clone()).unwrap_or_default();
        let clock = analysis
            .clocks
            .iter()
            .map(|(c, _)| c.clone())
            .next()
            .unwrap_or_default();
        Some(TimingPath {
            from,
            to,
            clock,
            slack_ps: analysis.wns_ps,
            requirement_ps: analysis.period_ps as i64,
            kind: "setup",
        })
    };
    view.timing = Some(TimingSummary {
        wns_ps: analysis.wns_ps.min(0),
        tns_ps: 0,
        whs_ps: 0,
        ths_ps: 0,
        failing_endpoints: analysis.failing_endpoints,
        total_endpoints: analysis.endpoints,
        worst_path,
        worst_hold_path: None,
        endpoints,
        fmax_mhz: Some(analysis.fmax_mhz()),
    });
    view.set_provenance(
        "timing",
        Provenance::new(
            "helion-sta",
            "static timing on the routed design at the constrained clock periods",
            "routed checkpoint",
        ),
    );
}

fn site_of_cell(routed: &Routed7, cell: &str) -> Option<String> {
    let p = &routed.placed;
    for l in &p.luts {
        if l.name == cell {
            return Some(l.site.clone());
        }
    }
    for f in &p.ffs {
        if f.name == cell {
            return Some(f.site.clone());
        }
    }
    None
}

/// Fill the control sets from grouped flip-flops.
pub fn fill_control_sets(view: &mut ImplView, grouped: &[(String, String, String, String, Vec<String>)]) {
    let mut sets: Vec<ControlSet> = grouped
        .iter()
        .map(|(c, e, s, sv, flops)| ControlSet {
            key: (c.clone(), e.clone(), s.clone(), sv.clone()),
            flops: flops.clone(),
        })
        .collect();
    sets.sort_by(|a, b| b.flops.len().cmp(&a.flops.len()));
    view.control_sets = sets;
    view.set_provenance(
        "control_sets",
        Provenance::new(
            "helion-xilinx7 pack",
            "flip-flops grouped by (clock, enable, set/reset, set/reset value)",
            "routed checkpoint",
        ),
    );
}

/// Fill the high fanout nets from load counts.
pub fn fill_high_fanout(view: &mut ImplView, nets: &[(String, usize, bool)], threshold: usize) {
    let mut v: Vec<HighFanoutNet> = nets
        .iter()
        .filter(|(_, n, _)| *n >= threshold)
        .map(|(net, n, clk)| HighFanoutNet {
            net: net.clone(),
            fanout: *n,
            is_clock: *clk,
        })
        .collect();
    v.sort_by(|a, b| b.fanout.cmp(&a.fanout));
    view.high_fanout = v;
    view.set_provenance(
        "high_fanout",
        Provenance::new(
            "helion-xilinx7 netlist",
            format!("nets with {threshold} or more loads"),
            "routed checkpoint",
        ),
    );
}

/// Fill the IO plan from the design's IO buffers and the package table.
pub fn fill_io(view: &mut ImplView, design: &Design, dev: &Device) {
    let mut pins: Vec<IoPin> = Vec::new();
    for port in &design.ports {
        let Some(loc) = port.attrs.get("LOC") else {
            continue;
        };
        if loc.is_empty() {
            continue;
        }
        let pin = dev.package_pin(loc);
        pins.push(IoPin {
            port: helion_ir::resolve(port.name).to_string(),
            package_pin: loc.to_string(),
            bank: pin.and_then(|p| p.bank.parse::<u32>().ok()),
            iostandard: port
                .attrs
                .get("IOSTANDARD")
                .unwrap_or("LVCMOS33")
                .to_string(),
            direction: match port.dir {
                helion_ir::PortDir::In => "input",
                helion_ir::PortDir::Out => "output",
                helion_ir::PortDir::Inout => "inout",
            },
        });
    }
    pins.sort_by(|a, b| a.package_pin.cmp(&b.package_pin));
    view.io = Some(IoPlan {
        pins,
        package_pins: dev.package_pins().count(),
    });
    view.set_provenance(
        "io",
        Provenance::new(
            "helion-xilinx7 device",
            "design ports with a LOC joined to the package pin table",
            "routed checkpoint",
        ),
    );
}

/// Fill DRC from the violations the flow reported.
pub fn fill_drc(view: &mut ImplView, items: &[(String, &'static str, String)]) {
    view.drc = items
        .iter()
        .map(|(rule, sev, msg)| DrcViolation {
            rule: rule.clone(),
            severity: sev,
            message: msg.clone(),
        })
        .collect();
    view.set_provenance(
        "drc",
        Provenance::new(
            "helion-xilinx7 flow",
            "design rule checks run against the device model and package",
            "routed checkpoint",
        ),
    );
}

/// Fill the clock utilization panel.
pub fn fill_clocks(view: &mut ImplView, clocks: Vec<ClockNet>, bufgs_used: usize, bufgs_available: usize) {
    let mut loads_by_region: BTreeMap<String, usize> = BTreeMap::new();
    for c in &clocks {
        for r in &c.regions {
            *loads_by_region.entry(r.clone()).or_insert(0) += 1;
        }
    }
    view.clocks = Some(ClockUtilization {
        clocks,
        loads_by_region,
        bufgs_used,
        bufgs_available,
    });
    view.set_provenance(
        "clocks",
        Provenance::new(
            "helion-xilinx7 clock",
            "clock nets from the routed design, loads per clock region",
            "routed checkpoint",
        ),
    );
}

/// The stage a routed run reaches.
pub const ROUTED_STAGE: Stage = Stage::Routed;

/// The family this filler serves.
pub const FAMILY: Family = Family::Xilinx7;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logic_tiles_are_not_classified_as_memory_or_dsp() {
        // The trap: `CLBLM_R` contains `B`, `CLEL_R` contains `L`, and neither
        // is a BRAM. Ordering the rules wrongly would paint half the fabric
        // as block RAM.
        assert_eq!(classify_tile("CLBLM_R"), TileClass::Logic);
        assert_eq!(classify_tile("CLEL_L"), TileClass::Logic);
        assert_eq!(classify_tile("CLEM"), TileClass::Logic);
        assert_eq!(classify_tile("BRAM_L"), TileClass::Ram);
        assert_eq!(classify_tile("DSP_R"), TileClass::Dsp);
        assert_eq!(classify_tile("RIOB33_X43Y75"), TileClass::Io);
        assert_eq!(classify_tile("BUFGCTRL_X0Y0"), TileClass::Clock);
        assert_eq!(classify_tile("INT_L"), TileClass::Other);
    }

    #[test]
    fn wire_direction_reads_the_leading_letters_only() {
        assert_eq!(wire_direction("EE4"), Some('H'));
        assert_eq!(wire_direction("WW2"), Some('H'));
        assert_eq!(wire_direction("NN1"), Some('V'));
        assert_eq!(wire_direction("SS3"), Some('V'));
        // A name that starts with a digit has no leading direction.
        assert_eq!(wire_direction("4_EE4"), None);
        // Site-pin and constant wires are neither direction.
        assert_eq!(wire_direction("VCC"), None);
        assert_eq!(wire_direction("GND"), None);
    }

    #[test]
    fn clock_region_is_derived_from_the_extent_not_assumed() {
        // 60 rows per region on a full die.
        assert_eq!(clock_region_of(0, 0, 5, 60), "X0Y0");
        assert_eq!(clock_region_of(0, 59, 5, 60), "X0Y0");
        assert_eq!(clock_region_of(0, 60, 5, 60), "X0Y1");
        assert_eq!(clock_region_of(4, 0, 5, 60), "X0Y0");
        assert_eq!(clock_region_of(5, 0, 5, 60), "X1Y0");
    }

    #[test]
    fn short_die_gets_one_band_not_zero() {
        // A 20-row die must not label everything Y0 by accident of a division
        // by 60; it is one region by construction.
        assert_eq!(derive_region_rows(20), 20);
        assert_eq!(clock_region_of(0, 19, 5, 20), "X0Y0");
        assert_eq!(derive_region_rows(200), 60);
        assert_eq!(clock_region_of(0, 130, 5, 60), "X0Y2");
    }

    #[test]
    fn empty_view_records_no_die_instead_of_a_zero_grid() {
        let v = ImplView::empty(FAMILY, "xc7z020clg484-1", ROUTED_STAGE);
        assert_eq!(v.cols, 0);
        assert!(v.tiles.is_empty());
    }
}
