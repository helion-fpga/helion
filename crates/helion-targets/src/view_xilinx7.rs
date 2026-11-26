//! Fill an [`ImplView`] from a 7-series (`helion-xilinx7`) implementation run.
//!
//! This is the only place that knows how 7-series data is shaped. Everything
//! downstream (Device view, heat maps, reports) reads the contract instead, so
//! a 7-series design and an iCE40 design render through the same code.
//!
//! What each section is computed from, stated because a view shows it:
//!
//! - **Tiles**: the device model's tile list, classified by tile-type name.
//!   Coordinates are the X-Ray `grid_x/grid_y` (row 0 at the top).
//! - **Clock regions**: the `clock_region` X-Ray's `tilegrid.json` records per
//!   tile. Only when the database has none for the part are they derived from
//!   the grid, and the provenance says so.
//! - **IO banks**: the package pin table's bank per IOB tile.
//! - **Cells**: each placed primitive's site, BEL (where the placer picks
//!   one) and tile.
//! - **Nets**: the routed PIP list per net, one [`WireHop`] per PIP, and the
//!   site pins the router connected.
//! - **Usage**: occupied sites per tile; routed wires per tile counted per
//!   direction against the tile's wire count per direction, never clamped.
//! - **Timing**: the run's setup analysis, every endpoint's slack.
//! - **Clocks, control sets, fanout, IO**: computed from the packed netlist,
//!   the placement and the router's net pins.
//!
//! Nothing here re-runs place or route: it reads the run's result.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use helion_ir::Design;
use helion_xilinx7::backend::Routed7;
use helion_xilinx7::device::Device;
use helion_xilinx7::pack::IobDir;
use helion_xilinx7::route::RouteKind;
use helion_xilinx7::timing::Analysis;
use helion_xilinx7::xdc_flow::Built;

use crate::Family;
use crate::view::{
    CellPlacement, ClockNet, ClockUtilization, ControlSet, DrcViolation, EndpointSlack,
    HighFanoutNet, ImplView, IoPin, IoPlan, Overflow, Provenance, ResourceRow, RoutedNet, Stage, TileClass,
    TileUsage, TimingPath, TimingSummary, ViewTile, WireHop,
};

/// Scope string every section of a routed 7-series run carries.
const SCOPE: &str = "routed design of this run";

/// Where the tile coordinates start. The device model uses signed grid
/// coordinates; the contract uses `(0, 0)` at the die's top-left.
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
    if t.starts_with("BRAM") && !t.contains("INT") {
        return TileClass::Ram;
    }
    if t.starts_with("DSP") {
        return TileClass::Dsp;
    }
    if t.starts_with("IOB") || t.starts_with("RIOB") || t.starts_with("LIOB") {
        return TileClass::Io;
    }
    if t.starts_with("BUFG") || t.starts_with("BUFH") || t.starts_with("BUFIO") || t.starts_with("BUFR")
        || t.starts_with("MMCM") || t.starts_with("PLL") || t.starts_with("CMT") || t.starts_with("CLK_BUFG")
    {
        return TileClass::Clock;
    }
    TileClass::Other
}

/// Clock region of a tile, from its row and column (fallback only, for a
/// database without `clock_region`).
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
    // first letter: `VCC` starts with V but is a constant.
    if matches!(name, "VCC" | "GND" | "VDD" | "VSS") {
        return None;
    }
    // A directional name is letters followed by digits (`EE4`, `NN1`, `EN2`).
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

/// Build the tile list, the die extent, clock regions and IO banks from the
/// device model.
pub fn fill_die(view: &mut ImplView, dev: &Device) -> Origin {
    let origin = device_origin(dev);
    let bank_of_tile: HashMap<&str, u32> = dev
        .package_pins()
        .filter_map(|p| Some((p.tile.as_str(), p.bank.parse::<u32>().ok()?)))
        .collect();
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
        tiles.push(ViewTile {
            x,
            y,
            class: classify_tile(type_name),
            name: t.name.clone(),
            clock_region: t.clock_region.clone().unwrap_or_default(),
            sites: t.sites.iter().map(|s| s.name.clone()).collect(),
            io_bank: bank_of_tile.get(t.name.as_str()).copied(),
        });
    }
    let from_db = tiles.iter().any(|t| !t.clock_region.is_empty());
    let region_method = if from_db {
        "device model tile list; clock region per tile from the database's tilegrid.json".to_string()
    } else {
        // shortcut: equal 5-column, 60-row bands, approximate; upgrade when the part's tilegrid.json carries clock_region.
        let rows = derive_region_rows(max_y + 1);
        for t in tiles.iter_mut() {
            t.clock_region = clock_region_of(t.x, t.y, 5, rows);
        }
        format!("device model tile list; the database has no clock_region for this part, so regions are DERIVED from the grid (5 columns x {rows} rows per band, approximate)")
    };
    view.tiles = tiles;
    view.cols = max_x + 1;
    view.rows = max_y + 1;
    view.row0_top = true;
    view.set_provenance("tiles", Provenance::new("helion-xilinx7 device", region_method, view.part.clone()));
    origin
}

/// Rows of tiles per clock region for the fallback derivation.
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

/// View coordinates of the tile holding `site`.
fn site_xy(dev: &Device, origin: Origin, site: &str) -> Option<(u32, u32)> {
    let t = dev.tile(dev.site_by_name(site)?.0)?;
    Some(((t.grid_x - origin.x).max(0) as u32, (t.grid_y - origin.y).max(0) as u32))
}

/// Fill the placed cells from the routed design.
pub fn fill_cells(view: &mut ImplView, routed: &Routed7) {
    let p = &routed.placed;
    let mut cells: Vec<CellPlacement> = Vec::new();
    let mut push = |name: &str, site: &str, tile: &str, bel: Option<&str>, kind: &str| {
        cells.push(CellPlacement {
            cell: name.to_string(),
            site: site.to_string(),
            bel: bel.map(str::to_string),
            tile: Some(tile.to_string()),
            kind: kind.to_string(),
        });
    };
    for c in &p.luts {
        push(&c.name, &c.site, &c.tile, Some(&c.bel), "LUT");
    }
    for c in &p.ffs {
        push(&c.name, &c.site, &c.tile, Some(&c.bel), "FF");
    }
    for c in &p.srls {
        push(&c.name, &c.site, &c.tile, Some(&c.bel), "SRL");
    }
    for c in &p.muxfs {
        push(&c.name, &c.site, &c.tile, Some(&c.bel), "MUXF");
    }
    for c in &p.carries {
        push(&c.name, &c.site, &c.tile, None, "CARRY4");
    }
    for c in &p.lutrams {
        push(&c.name, &c.site, &c.tile, None, "LUTRAM");
    }
    for c in &p.iobs {
        push(&c.name, &c.site, &c.tile, None, "IOB");
    }
    for c in &p.bufgs {
        push(&c.name, &c.site, &c.tile, None, "BUFG");
    }
    for c in &p.brams {
        push(&c.name, &c.site, &c.tile, None, "RAMB");
    }
    for c in &p.dsps {
        push(&c.name, &c.site, &c.tile, None, "DSP48E1");
    }
    for c in &p.cmts {
        push(&c.name, &c.site, &c.tile, None, "CMT");
    }
    for c in &p.ioddrs {
        push(&c.name, &c.site, &c.tile, None, "IODDR");
    }
    for c in &p.serdes {
        push(&c.name, &c.site, &c.tile, None, "SERDES");
    }
    for c in &p.iodelays {
        push(&c.name, &c.site, &c.tile, None, "IODELAY");
    }
    for c in &p.xadcs {
        push(&c.name, &c.site, &c.tile, None, "XADC");
    }
    for c in &p.cfg_prims {
        push(&c.name, &c.site, &c.tile, None, "CONFIG");
    }
    for c in &p.ps7s {
        push(&c.name, &c.site, &c.tile, None, "PS7");
    }
    for c in &p.gtp_channels {
        push(&c.name, &c.site, &c.tile, None, "GTP_CHANNEL");
    }
    for c in &p.gtp_commons {
        push(&c.name, &c.site, &c.tile, None, "GTP_COMMON");
    }
    for c in &p.ibufds_gte2s {
        push(&c.name, &c.site, &c.tile, None, "IBUFDS_GTE2");
    }
    for c in &p.pcies {
        push(&c.name, &c.site, &c.tile, None, "PCIE");
    }
    view.cells = cells;
    view.set_provenance(
        "placement",
        Provenance::new("helion-xilinx7 place", "placed primitive to site and BEL from the run's placement", SCOPE),
    );
}

/// Fill the routed nets: one hop per PIP, plus the driver and load site pins
/// the router connected.
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
        let pins = routed.net_pins.get(&n.net);
        let pin = |e: &helion_xilinx7::route::Endpoint| format!("{}.{}", e.site, e.pin);
        nets.push(RoutedNet {
            net: n.net.clone(),
            driver: pins.and_then(|v| v.iter().find(|e| e.is_driver)).map(pin),
            loads: pins.map(|v| v.iter().filter(|e| !e.is_driver).map(pin).collect()).unwrap_or_default(),
            hops,
            kind,
        });
    }
    view.nets = nets;
    view.set_provenance(
        "routing",
        Provenance::new("helion-xilinx7 route", "PathFinder PIP list per net after the final iteration; site pins as the router connected them", SCOPE),
    );
}

/// Count occupied sites and routed wires per tile, split by direction, and
/// pair them with the tile's capacity.
///
/// Occupancy counts **every routed wire that a PIP drives in the tile**: a
/// tile a net merely passes through still occupies its wire. Capacity is the
/// number of wires of that direction the tile type declares, so
/// `used > capacity` is real overuse.
pub fn fill_usage(view: &mut ImplView, routed: &Routed7, dev: &Device) {
    let origin = device_origin(dev);
    let mut used_h: HashMap<&str, u32> = HashMap::new();
    let mut used_v: HashMap<&str, u32> = HashMap::new();
    for n in &routed.nets {
        if let RouteKind::Pips(pips) = &n.kind {
            for p in pips {
                if p.is_pseudo {
                    continue;
                }
                match wire_direction(&p.dst_wire) {
                    Some('H') => *used_h.entry(p.tile.as_str()).or_insert(0) += 1,
                    Some('V') => *used_v.entry(p.tile.as_str()).or_insert(0) += 1,
                    _ => {}
                }
            }
        }
    }
    // Distinct occupied sites per tile: a slice holding 8 LUTs is one site.
    let mut sites_used: HashMap<String, HashSet<&str>> = HashMap::new();
    for c in &view.cells {
        if let Some(tile) = &c.tile {
            sites_used.entry(tile.clone()).or_default().insert(c.site.as_str());
        }
    }
    let mut usage: Vec<TileUsage> = Vec::new();
    for idx in 0..dev.tile_count() {
        let Some(t) = dev.tile(idx) else { continue };
        let (mut ch, mut cv) = (0u32, 0u32);
        for name in dev.wires(idx).unwrap_or(&[]) {
            match wire_direction(name) {
                Some('H') => ch += 1,
                Some('V') => cv += 1,
                _ => {}
            }
        }
        if ch == 0 && cv == 0 && t.sites.is_empty() {
            continue;
        }
        let (uh, uv) = (
            used_h.get(t.name.as_str()).copied().unwrap_or(0),
            used_v.get(t.name.as_str()).copied().unwrap_or(0),
        );
        usage.push(TileUsage {
            x: (t.grid_x - origin.x).max(0) as u32,
            y: (t.grid_y - origin.y).max(0) as u32,
            sites_used: sites_used.get(&t.name).map(|s| s.len() as u32).unwrap_or(0),
            sites_capacity: t.sites.len() as u32,
            overflow: (ch > 0 || cv > 0).then_some(Overflow { h_used: uh, h_capacity: ch, v_used: uv, v_capacity: cv }),
        });
    }
    view.usage = usage;
    view.set_provenance(
        "usage",
        Provenance::new(
            "helion-xilinx7 route",
            "occupied sites per tile; PIP-driven wires per tile counted by direction (E/W horizontal, N/S vertical) against the tile type's wires of that direction, not clamped",
            SCOPE,
        ),
    );
}

/// LUTs a LUT RAM primitive occupies.
fn lutram_luts(module: &str) -> u32 {
    match module {
        "RAM32X1S" | "RAM64X1S" => 1,
        "RAM32X1D" | "RAM64X1D" | "RAM128X1S" => 2,
        _ => 4, // RAM32M, RAM64M, RAM128X1D, RAM256X1S
    }
}

/// Utilization in `report_utilization`'s rows: slice logic, slices, block
/// RAM tiles, DSPs, bonded IOB and global clock buffers, each against the
/// part's own site count.
pub fn fill_resources(view: &mut ImplView, routed: &Routed7, dev: &Device) {
    let p = &routed.placed;
    let slices = dev.slices().len() as u32;
    let slicem = dev.sites_of_types(&["SLICEM"]).len() as u32;
    // One physical LUT per (site, letter): an O5/O6 pair shares it.
    let logic: HashSet<(&str, char)> = p.luts.iter().map(|l| (l.site.as_str(), l.bel.chars().next().unwrap_or('A'))).collect();
    let ram: u32 = p.packed.lutrams.iter().map(|r| lutram_luts(&r.module)).sum();
    let srl = p.srls.len() as u32;
    let f8 = p.muxfs.iter().filter(|m| m.bel.starts_with("F8")).count() as u32;
    let used_slices: HashSet<&str> = p
        .luts
        .iter()
        .map(|c| c.site.as_str())
        .chain(p.ffs.iter().map(|c| c.site.as_str()))
        .chain(p.srls.iter().map(|c| c.site.as_str()))
        .chain(p.muxfs.iter().map(|c| c.site.as_str()))
        .chain(p.carries.iter().map(|c| c.site.as_str()))
        .chain(p.lutrams.iter().map(|c| c.site.as_str()))
        .collect();
    let b36 = p.brams.iter().filter(|b| b.site.starts_with("RAMB36")).count() as f64;
    let b18 = p.brams.len() as f64 - b36;
    // X-Ray types a block RAM tile's lower RAMB18 site FIFO18E1, so count tiles.
    let ramb18_sites = 2 * dev.sites_of_types(&["RAMB36E1"]).len() as u32;
    let iob_sites = dev.package_pins().filter(|pp| pp.site.starts_with("IOB_")).count() as u32;
    let iobs: HashSet<&str> = p.iobs.iter().map(|i| i.site.as_str()).collect();
    let bufg = p.bufgs.iter().filter(|g| g.site.starts_with("BUFGCTRL")).count() as f64;
    let row = |name: &str, depth: u8, used: f64, available: u32| ResourceRow { name: name.into(), depth, used, available };
    let n = logic.len() as f64;
    view.resources = vec![
        row("Slice LUTs", 0, n + (ram + srl) as f64, slices * 4),
        row("LUT as Logic", 1, n, slices * 4),
        row("LUT as Memory", 1, (ram + srl) as f64, slicem * 4),
        row("LUT as Distributed RAM", 2, ram as f64, 0),
        row("LUT as Shift Register", 2, srl as f64, 0),
        row("Slice Registers", 0, p.ffs.len() as f64, slices * 8),
        row("F7 Muxes", 0, (p.muxfs.len() as u32 - f8) as f64, slices * 2),
        row("F8 Muxes", 0, f8 as f64, slices),
        row("Slice", 0, used_slices.len() as f64, slices),
        row("Block RAM Tile", 0, b36 + b18 / 2.0, ramb18_sites / 2),
        row("RAMB36/FIFO", 1, b36, ramb18_sites / 2),
        row("RAMB18", 1, b18, ramb18_sites),
        row("DSPs", 0, p.dsps.len() as f64, dev.dsp_sites().len() as u32),
        row("Bonded IOB", 0, iobs.len() as f64, iob_sites),
        row("BUFGCTRL", 0, bufg, dev.bufg_sites().len() as u32),
    ];
    view.set_provenance(
        "resources",
        Provenance::new(
            "helion-xilinx7 place",
            "placed primitives in report_utilization's rows: a LUT is one per (slice, letter) so an O5/O6 pair counts once; LUT RAM by its primitive's LUT count; slices are distinct occupied SLICE sites; a RAMB18 is half a block RAM tile; bonded IOB against package pins bonded to IOB sites",
            SCOPE,
        ),
    );
}

/// Fill the timing summary and per-endpoint slack from the run's setup
/// analysis.
pub fn fill_timing(view: &mut ImplView, analysis: &Analysis, dev: &Device) {
    if analysis.endpoints == 0 {
        view.mark_missing("timing", "no timed endpoint: no flop, LUT RAM, block RAM or DSP input is driven by clocked logic");
        return;
    }
    let origin = device_origin(dev);
    let endpoints: Vec<EndpointSlack> = analysis
        .endpoint_slacks
        .iter()
        .map(|e| {
            let xy = site_xy(dev, origin, &e.site);
            EndpointSlack {
                endpoint: e.endpoint.clone(),
                slack_ps: e.slack_ps,
                clock: e.clock.clone(),
                site: Some(e.site.clone()),
                x: xy.map(|p| p.0),
                y: xy.map(|p| p.1),
            }
        })
        .collect();
    let worst_clock = analysis.clocks.iter().min_by_key(|c| c.1).map(|c| c.0.clone()).unwrap_or_default();
    let worst_path = match (analysis.worst_path.first(), analysis.worst_path.last()) {
        (Some(f), Some(l)) => Some(TimingPath {
            from: f.what.clone(),
            to: l.what.clone(),
            clock: worst_clock,
            slack_ps: analysis.wns_ps,
            requirement_ps: analysis.period_ps as i64,
            kind: "setup",
        }),
        _ => None,
    };
    let fmax = analysis.fmax_mhz();
    let h = &analysis.hold;
    let worst_hold_path = match (h.worst_path.first(), h.worst_path.last()) {
        (Some(f), Some(l)) => Some(TimingPath {
            from: f.what.clone(),
            to: l.what.clone(),
            clock: h.clock.clone(),
            slack_ps: h.whs_ps,
            requirement_ps: 0,
            kind: "hold",
        }),
        _ => None,
    };
    view.timing = Some(TimingSummary {
        wns_ps: analysis.wns_ps,
        tns_ps: endpoints.iter().map(|e| e.slack_ps.min(0)).sum(),
        whs_ps: (h.endpoints > 0).then_some(h.whs_ps),
        ths_ps: (h.endpoints > 0).then_some(h.ths_ps),
        failing_endpoints: analysis.failing_endpoints,
        total_endpoints: analysis.endpoints,
        worst_path,
        worst_hold_path,
        endpoints,
        fmax_mhz: fmax.is_finite().then_some(fmax),
    });
    view.set_provenance(
        "timing",
        Provenance::new(
            "helion-xilinx7 timing",
            "setup: routed wire delays and X-Ray SDF slow-corner cell delays at the XDC clock periods, flat 400 ps margin for skew and uncertainty; hold: shortest path with X-Ray SDF fast-corner cell delays, routed wire delays scaled by the LUT fast/slow ratio 45/124, ideal clock (zero skew); IO paths unconstrained",
            SCOPE,
        ),
    );
}

/// Fill the control sets from grouped flip-flops:
/// `(clock, enable, set/reset, set/reset kind, flops)`.
pub fn fill_control_sets(view: &mut ImplView, grouped: &[(String, String, String, String, Vec<String>)]) {
    let mut sets: Vec<ControlSet> = grouped
        .iter()
        .map(|(c, e, s, sv, flops)| ControlSet {
            key: (c.clone(), e.clone(), s.clone(), sv.clone()),
            flops: flops.clone(),
        })
        .collect();
    sets.sort_by(|a, b| b.flops.len().cmp(&a.flops.len()).then(a.key.cmp(&b.key)));
    view.control_sets = sets;
    view.set_provenance(
        "control_sets",
        Provenance::new("helion-xilinx7 pack", "packed flip-flops grouped by (clock, enable, set/reset, set/reset kind)", SCOPE),
    );
}

/// Group the packed flip-flops by control set.
pub fn control_set_groups(routed: &Routed7) -> Vec<(String, String, String, String, Vec<String>)> {
    let mut m: BTreeMap<(String, String, String, String), Vec<String>> = BTreeMap::new();
    for f in &routed.placed.packed.ffs {
        let sr_kind = match (&f.sr_net, f.sr_async, f.srval) {
            (None, ..) => String::new(),
            (Some(_), false, false) => "sync reset".into(),
            (Some(_), false, true) => "sync set".into(),
            (Some(_), true, false) => "async clear".into(),
            (Some(_), true, true) => "async preset".into(),
        };
        m.entry((f.clk_net.clone(), f.ce_net.clone().unwrap_or_default(), f.sr_net.clone().unwrap_or_default(), sr_kind))
            .or_default()
            .push(f.name.clone());
    }
    m.into_iter().map(|((c, e, s, k), v)| (c, e, s, k, v)).collect()
}

/// Fill the high fanout nets from `(net, loads, is_clock)`: the `max_nets`
/// largest, as `report_high_fanout_nets` lists by default.
pub fn fill_high_fanout(view: &mut ImplView, nets: &[(String, usize, bool)], max_nets: usize) {
    let mut v: Vec<HighFanoutNet> = nets
        .iter()
        .map(|(net, n, clk)| HighFanoutNet { net: net.clone(), fanout: *n, is_clock: *clk })
        .collect();
    v.sort_by(|a, b| b.fanout.cmp(&a.fanout).then(a.net.cmp(&b.net)));
    v.truncate(max_nets);
    view.high_fanout = v;
    view.set_provenance(
        "high_fanout",
        Provenance::new(
            "helion-xilinx7 route",
            format!("the {max_nets} nets with the most routed load site pins (pins shared inside a slice count once)"),
            SCOPE,
        ),
    );
}

/// `(net, routed loads, is_clock)` for every routed net.
pub fn net_fanouts(routed: &Routed7, clocks: &HashSet<&str>) -> Vec<(String, usize, bool)> {
    routed
        .net_pins
        .iter()
        .map(|(n, eps)| (n.clone(), eps.iter().filter(|e| !e.is_driver).count(), clocks.contains(n.as_str())))
        .collect()
}

/// Fill the IO plan from the placed IO buffers, joined to the package pin
/// table for the bank.
pub fn fill_io(view: &mut ImplView, routed: &Routed7, design: &Design, dev: &Device) {
    let packed = &routed.placed.packed;
    // Output IOB cells name their port bit; inputs carry it in the net name.
    let bit_of: HashMap<&str, String> = design
        .cells
        .iter()
        .filter_map(|c| Some((helion_ir::resolve(c.name), c.attrs.get("BIT")?.to_string())))
        .collect();
    let mut per_port: HashMap<&str, usize> = HashMap::new();
    for i in &packed.iobs {
        *per_port.entry(i.port.as_str()).or_insert(0) += 1;
    }
    let mut pins: Vec<IoPin> = Vec::new();
    for i in &packed.iobs {
        let Some(loc) = &i.loc else { continue };
        let port = if per_port[i.port.as_str()] > 1 {
            let bit = bit_of.get(i.name.as_str()).cloned().or_else(|| {
                let (_, b) = i.net.rsplit_once('_')?;
                b.chars().all(|c| c.is_ascii_digit()).then(|| b.to_string())
            });
            match bit {
                Some(b) => format!("{}[{b}]", i.port),
                None => i.name.clone(),
            }
        } else {
            i.port.clone()
        };
        pins.push(IoPin {
            port,
            package_pin: loc.clone(),
            bank: dev.package_pin(loc).and_then(|p| p.bank.parse::<u32>().ok()),
            iostandard: i.iostandard.clone(),
            direction: match i.dir {
                IobDir::In => "input",
                IobDir::Out => "output",
            },
        });
    }
    pins.sort_by(|a, b| a.package_pin.cmp(&b.package_pin));
    view.io = Some(IoPlan { pins, package_pins: dev.package_pins().count() });
    view.set_provenance(
        "io",
        Provenance::new("helion-xilinx7 pack", "placed IO buffers with their XDC PACKAGE_PIN and IOSTANDARD, bank from the package pin table", SCOPE),
    );
}

/// Supply voltage an IO standard needs from its bank's VCCO, when the
/// standard fixes one.
fn vcco_of(iostandard: &str) -> Option<&'static str> {
    let s = iostandard.to_ascii_uppercase();
    Some(match s.as_str() {
        "LVCMOS33" | "LVTTL" | "TMDS_33" => "3.3",
        "LVCMOS25" | "LVDS_25" => "2.5",
        "LVCMOS18" => "1.8",
        "LVCMOS15" => "1.5",
        "LVCMOS12" => "1.2",
        _ => return None,
    })
}

/// Bank VCCO consistency (Vivado rule BIVC-1): every IO standard placed in a
/// bank must want the same VCCO.
pub fn bank_vcco_drc(io: &IoPlan) -> Vec<(String, &'static str, String)> {
    let mut by_bank: BTreeMap<u32, BTreeMap<&'static str, BTreeSet<&str>>> = BTreeMap::new();
    for p in &io.pins {
        if let (Some(b), Some(v)) = (p.bank, vcco_of(&p.iostandard)) {
            by_bank.entry(b).or_default().entry(v).or_default().insert(p.port.as_str());
        }
    }
    by_bank
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(bank, v)| {
            let detail: Vec<String> = v.iter().map(|(volt, ports)| format!("{volt} V: {}", ports.iter().copied().collect::<Vec<_>>().join(", "))).collect();
            ("BIVC-1".to_string(), "Error", format!("bank {bank} mixes VCCO levels ({})", detail.join("; ")))
        })
        .collect()
}

/// Fill DRC from the violations found and the rules that were run.
pub fn fill_drc(view: &mut ImplView, items: &[(String, &'static str, String)], rules_run: &str) {
    view.drc = items
        .iter()
        .map(|(rule, sev, msg)| DrcViolation { rule: rule.clone(), severity: sev, message: msg.clone() })
        .collect();
    view.set_provenance("drc", Provenance::new("helion-targets view", format!("rules run: {rules_run}"), SCOPE));
}

/// Fill the clock utilization panel.
pub fn fill_clocks(
    view: &mut ImplView,
    clocks: Vec<ClockNet>,
    loads_by_region: BTreeMap<String, usize>,
    bufgs_used: usize,
    bufgs_available: usize,
) {
    view.clocks = Some(ClockUtilization { clocks, loads_by_region, bufgs_used, bufgs_available });
    view.set_provenance(
        "clocks",
        Provenance::new(
            "helion-xilinx7 pack/place",
            "clock nets of the timed domains; loads are clocked primitives (flops, SRLs, LUT RAMs, block RAM ports, DSPs) by the clock region of their placed tile",
            SCOPE,
        ),
    );
}

/// Clock nets with their loads and regions, and the loads per region.
pub fn clock_nets(view: &ImplView, routed: &Routed7, periods: &HashMap<String, u32>) -> (Vec<ClockNet>, BTreeMap<String, usize>) {
    let p = &routed.placed;
    let k = &p.packed;
    let region_of_site: HashMap<&str, &str> = {
        let tile_region: HashMap<&str, &str> = view.tiles.iter().map(|t| (t.name.as_str(), t.clock_region.as_str())).collect();
        view.cells
            .iter()
            .filter_map(|c| Some((c.site.as_str(), *tile_region.get(c.tile.as_deref()?)?)))
            .collect()
    };
    let site_of: HashMap<&str, &str> = view.cells.iter().map(|c| (c.cell.as_str(), c.site.as_str())).collect();
    // (clock net, load cell) for every clocked primitive.
    let mut loads: Vec<(&str, &str)> = Vec::new();
    loads.extend(k.ffs.iter().map(|f| (f.clk_net.as_str(), f.name.as_str())));
    loads.extend(k.srls.iter().map(|s| (s.clk_net.as_str(), s.name.as_str())));
    loads.extend(k.lutrams.iter().map(|r| (r.clk.as_str(), r.name.as_str())));
    for b in &k.brams {
        loads.extend([b.clka_net.as_deref(), b.clkb_net.as_deref()].into_iter().flatten().map(|c| (c, b.name.as_str())));
    }
    loads.extend(k.dsps.iter().filter_map(|d| Some((d.clk_net.as_deref()?, d.name.as_str()))));
    let mut by_region: BTreeMap<String, usize> = BTreeMap::new();
    let mut clocks: Vec<ClockNet> = Vec::new();
    let mut names: Vec<&String> = periods.keys().collect();
    names.sort();
    for name in names {
        let mut n = 0usize;
        let mut regions: BTreeSet<String> = BTreeSet::new();
        for (_, cell) in loads.iter().filter(|(c, _)| *c == name.as_str()) {
            n += 1;
            if let Some(r) = site_of.get(cell).and_then(|s| region_of_site.get(s)).filter(|r| !r.is_empty()) {
                regions.insert(r.to_string());
                *by_region.entry(r.to_string()).or_insert(0) += 1;
            }
        }
        let buffer = k
            .bufgs
            .iter()
            .find(|b| b.out_net == *name)
            .and_then(|b| p.bufgs.iter().find(|pb| pb.name == b.name))
            .map(|pb| pb.site.clone());
        clocks.push(ClockNet { name: name.clone(), period_ps: periods[name], loads: n, regions: regions.into_iter().collect(), buffer });
    }
    (clocks, by_region)
}

/// The whole view of a routed 7-series run.
pub fn from_built(b: &Built) -> ImplView {
    let dev = b.backend.device();
    let mut v = ImplView::empty(FAMILY, b.backend.part(), ROUTED_STAGE);
    fill_die(&mut v, dev);
    fill_cells(&mut v, &b.routed);
    fill_nets(&mut v, &b.routed);
    fill_usage(&mut v, &b.routed, dev);
    fill_resources(&mut v, &b.routed, dev);
    fill_timing(&mut v, &b.timing, dev);
    fill_io(&mut v, &b.routed, &b.design, dev);
    let (clocks, by_region) = clock_nets(&v, &b.routed, &b.periods);
    let bufgs_used = b.routed.placed.bufgs.iter().filter(|g| g.site.starts_with("BUFGCTRL")).count();
    fill_clocks(&mut v, clocks, by_region, bufgs_used, dev.bufg_sites().len());
    fill_control_sets(&mut v, &control_set_groups(&b.routed));
    let clock_names: HashSet<&str> = b.periods.keys().map(String::as_str).collect();
    fill_high_fanout(&mut v, &net_fanouts(&b.routed, &clock_names), 10);
    let drc = v.io.as_ref().map(bank_vcco_drc).unwrap_or_default();
    fill_drc(
        &mut v,
        &drc,
        "BIVC-1 (bank VCCO consistency); the flow itself stops on a port without PACKAGE_PIN, packer overflow and unresolved routing overuse, so a finished run passed those",
    );
    v.mark_missing("power", "no power model: Helion has no 7-series capacitance or static-power table and the run has no switching activity");
    v.mark_missing("board", "the board panel is not built yet: it needs the board's XDC labels joined to the design ports");
    v
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
    fn bank_vcco_rule_flags_only_a_bank_that_mixes_levels() {
        let pin = |port: &str, bank: u32, std: &str| IoPin {
            port: port.into(),
            package_pin: String::new(),
            bank: Some(bank),
            iostandard: std.into(),
            direction: "output",
        };
        let io = IoPlan {
            pins: vec![
                pin("a", 33, "LVCMOS33"),
                pin("b", 33, "LVCMOS18"),
                pin("c", 13, "LVCMOS33"),
                pin("d", 13, "LVTTL"),
                // A standard with no fixed VCCO is not judged.
                pin("e", 34, "LVCMOS33"),
                pin("f", 34, "DIFF_SSTL15"),
            ],
            package_pins: 0,
        };
        let v = bank_vcco_drc(&io);
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].0, "BIVC-1");
        assert!(v[0].2.contains("bank 33") && v[0].2.contains("3.3 V: a") && v[0].2.contains("1.8 V: b"), "{}", v[0].2);
    }

    #[test]
    fn empty_view_records_no_die_instead_of_a_zero_grid() {
        let v = ImplView::empty(FAMILY, "xc7z020clg484-1", ROUTED_STAGE);
        assert_eq!(v.cols, 0);
        assert!(v.tiles.is_empty());
    }
}
