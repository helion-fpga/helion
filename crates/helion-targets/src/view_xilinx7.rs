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
//! - **Schematic**: the v2 front end's elaborated netlist (RTL), the
//!   synthesized netlist the packer read, and that netlist with each cell's
//!   site and BEL from the placement (implemented).
//! - **Board**: the board's own pin file (`boards/<name>.xdc`) joined to the
//!   run's IO plan by package pin.
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
    BoardGroup, BoardItem, BoardPanel, NetlistCell, NetlistPin, NetlistStage, SchematicNetlist,
    CellPlacement, ClockNet, ClockUtilization, CongestionWindow, PowerBlock, PowerEstimate, TilePower, ControlSet, DrcViolation, EndpointSlack,
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

/// Window threshold for the congestion level. shortcut: Vivado's placed
/// threshold is not recovered (DECOMPILE_NOTES 3.3, 9); 85% is the floor the
/// routed table of the hotspot capture states ("less than 85% congestion are
/// not reported"), and that capture's levels (0 at 71-85% single tiles, 3
/// at 88%, 4 at 98%) are consistent with it. Upgrade when the cut-point is
/// recovered.
pub const CONGESTION_THRESHOLD_PCT: f32 = 85.0;

/// `X`/`Y` of a 7-series tile name (`INT_L_X26Y0` -> `(26, 0)`).
fn tile_index(name: &str) -> Option<(u32, u32)> {
    let (_, xy) = name.rsplit_once("_X")?;
    let (x, y) = xy.split_once('Y')?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

/// Congestion levels per direction over the interconnect tiles, on the
/// `INT_*_XnYm` grid (one interconnect tile per CLB column and row, the unit
/// Vivado's windows use).
pub fn fill_congestion(view: &mut ImplView) {
    let name_at: HashMap<(u32, u32), &str> = view.tiles.iter().map(|t| ((t.x, t.y), t.name.as_str())).collect();
    let mut h: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
    let mut v: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
    let mut int_name: HashMap<(u32, u32), &str> = HashMap::new();
    for u in &view.usage {
        let (Some(o), Some(name)) = (u.overflow, name_at.get(&(u.x, u.y))) else { continue };
        let Some(xy) = name.starts_with("INT_").then(|| tile_index(name)).flatten() else { continue };
        h.insert(xy, (o.h_used, o.h_capacity));
        v.insert(xy, (o.v_used, o.v_capacity));
        int_name.insert(xy, name);
    }
    let corner = |x: u32, y: u32| int_name.get(&(x, y)).map(|s| s.to_string()).unwrap_or_else(|| format!("INT_X{x}Y{y}"));
    view.congestion = [("Horizontal", &h), ("Vertical", &v)]
        .into_iter()
        .filter_map(|(dir, g)| {
            let (level, percent, (x0, y0, x1, y1)) = crate::view::congestion_level(g, CONGESTION_THRESHOLD_PCT, 7)?;
            Some(CongestionWindow { direction: dir, level, percent, from_tile: corner(x0, y0), to_tile: corner(x1, y1), overlap: percent > 100.0 })
        })
        .collect();
    view.set_provenance(
        "congestion",
        Provenance::new(
            "helion-xilinx7 route",
            format!("routed wires per interconnect tile by direction against the tile's wires of that direction; level = largest 2^l x 2^l tile window (l <= 7) whose summed use over summed capacity reaches {CONGESTION_THRESHOLD_PCT}% (threshold from the hotspot capture's routed-table floor; Vivado's own cut-point is not recovered), level 0 reports the hottest single tile; H covers East/West and V North/South, Vivado splits them four ways"),
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
    // X-Ray types a block RAM tile's sites FIFO18E1, RAMBFIFO36E1 and
    // RAMB18E1, so count the tiles.
    let ramb18_sites = 2 * dev.bram36_sites().len() as u32;
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

/// Helion's 7-series (xc7z020 -1, commercial, 25 C ambient) power
/// coefficients, mW per unit at 100 MHz and a 12.5% data toggle rate
/// (Vivado's vectorless default). Fitted by least squares per resource
/// class against vendor power reports of six reference designs; only the
/// fitted values live here. See [`POWER_FIT`] for the fit error.
pub mod power_coeffs {
    /// Per LUT (logic, distributed RAM or shift register).
    pub const LUT: f64 = 0.01057;
    pub const FF: f64 = 0.0003750;
    pub const CARRY4: f64 = 0.03121;
    /// Per block RAM tile (a RAMB18 is half).
    pub const BRAM: f64 = 0.07502;
    pub const DSP: f64 = 0.9635;
    /// Per bonded IO buffer.
    pub const IO: f64 = 0.1919;
    /// Per routed fabric wire (non-pseudo PIP) of a data net.
    pub const WIRE: f64 = 0.0003253;
    /// Clock tree: per global buffer and per clocked load (100 MHz, the
    /// clock toggles every cycle whatever the data rate).
    pub const BUFG: f64 = 1.778;
    pub const CLOCK_LOAD: f64 = 0.004013;
    /// Device static: base, per block RAM tile, and the leakage rise per mW
    /// of dynamic power (junction heating).
    pub const STATIC: f64 = 104.7;
    pub const STATIC_PER_BRAM: f64 = 0.08223;
    pub const STATIC_PER_DYNAMIC: f64 = 0.01793;
}

/// The fit's error, stated in the power report's method note.
pub const POWER_FIT: &str = "fit error on the six reference designs' totals (Helion's own counts): RMS 16.8 mW, worst +36.4 mW (+26%, a block RAM design whose routed wire count the model over-weights); Vivado propagates activity through the logic, this model applies one rate to every data net";

/// Vectorless toggle rate (Vivado's default for data nets).
pub const DEFAULT_TOGGLE_RATE: f64 = 0.125;

/// Fill the power estimate: static + dynamic per resource class, scaled by
/// the dominant clock's frequency and the toggle rate, and attributed to
/// tiles. `toggle` is the design's own default toggle rate, if it sets one.
pub fn fill_power(view: &mut ImplView, routed: &Routed7, periods: &HashMap<String, u32>, toggle: Option<f64>) {
    use power_coeffs as k;
    let p = &routed.placed;
    let alpha = toggle.unwrap_or(DEFAULT_TOGGLE_RATE);
    // The clock with the most clocked loads sets the data rate.
    let (clk_name, f_mhz) = view
        .clocks
        .as_ref()
        .and_then(|c| c.clocks.iter().max_by_key(|n| n.loads).map(|n| (n.name.clone(), 1.0e6 / n.period_ps.max(1) as f64)))
        .unwrap_or_else(|| (String::new(), 0.0));
    let data = f_mhz / 100.0 * alpha / DEFAULT_TOGGLE_RATE;
    let clk = f_mhz / 100.0;
    let tile_xy: HashMap<&str, (u32, u32)> = view.tiles.iter().map(|t| (t.name.as_str(), (t.x, t.y))).collect();
    let mut tiles: HashMap<(u32, u32), f64> = HashMap::new();
    let mut blocks: Vec<PowerBlock> = Vec::new();
    let mut class = |name: &str, cells: Vec<(&str, f64)>, mw_each: f64, tiles: &mut HashMap<(u32, u32), f64>| {
        let mut used = 0.0;
        for (tile, w) in &cells {
            used += w;
            if let Some(xy) = tile_xy.get(tile) {
                *tiles.entry(*xy).or_insert(0.0) += w * mw_each;
            }
        }
        blocks.push(PowerBlock { block: name.to_string(), used: used.round() as usize, total_mw: used * mw_each });
    };
    // Physical LUTs: one per (site, letter); LUT RAM by its LUT count.
    let mut luts: BTreeMap<(&str, char), &str> = BTreeMap::new();
    for l in &p.luts {
        luts.insert((l.site.as_str(), l.bel.chars().next().unwrap_or('A')), l.tile.as_str());
    }
    let ram_luts: HashMap<&str, u32> = p.packed.lutrams.iter().map(|r| (r.name.as_str(), lutram_luts(&r.module))).collect();
    let mut lut_cells: Vec<(&str, f64)> = luts.values().map(|t| (*t, 1.0)).collect();
    lut_cells.extend(p.srls.iter().map(|c| (c.tile.as_str(), 1.0)));
    lut_cells.extend(p.lutrams.iter().map(|c| (c.tile.as_str(), ram_luts.get(c.name.as_str()).copied().unwrap_or(1) as f64)));
    class("LUTs (logic and memory)", lut_cells, k::LUT * data, &mut tiles);
    class("Registers", p.ffs.iter().map(|c| (c.tile.as_str(), 1.0)).collect(), k::FF * data, &mut tiles);
    class("CARRY4", p.carries.iter().map(|c| (c.tile.as_str(), 1.0)).collect(), k::CARRY4 * data, &mut tiles);
    let bram: Vec<(&str, f64)> = p.brams.iter().map(|b| (b.tile.as_str(), if b.site.starts_with("RAMB36") { 1.0 } else { 0.5 })).collect();
    let bram_tiles: f64 = bram.iter().map(|b| b.1).sum();
    class("Block RAM", bram, k::BRAM * data, &mut tiles);
    class("DSPs", p.dsps.iter().map(|c| (c.tile.as_str(), 1.0)).collect(), k::DSP * data, &mut tiles);
    class("I/O", p.iobs.iter().map(|c| (c.tile.as_str(), 1.0)).collect(), k::IO * data, &mut tiles);
    // Signals: every fabric wire a data net's PIPs drive, in that PIP's tile.
    let clock_nets: HashSet<&str> = periods.keys().map(String::as_str).collect();
    let mut wires: Vec<(&str, f64)> = Vec::new();
    for n in &routed.nets {
        if let RouteKind::Pips(pips) = &n.kind {
            if !clock_nets.contains(n.net.as_str()) {
                wires.extend(pips.iter().filter(|q| !q.is_pseudo).map(|q| (q.tile.as_str(), 1.0)));
            }
        }
    }
    class("Signals (routed wires)", wires, k::WIRE * data, &mut tiles);
    // Clock tree: buffers plus every clocked primitive's clock pin.
    let mut clocked: Vec<(&str, f64)> = p.ffs.iter().map(|c| (c.tile.as_str(), 1.0)).collect();
    clocked.extend(p.srls.iter().map(|c| (c.tile.as_str(), 1.0)));
    clocked.extend(p.lutrams.iter().map(|c| (c.tile.as_str(), 1.0)));
    clocked.extend(p.brams.iter().map(|c| (c.tile.as_str(), 1.0)));
    clocked.extend(p.dsps.iter().map(|c| (c.tile.as_str(), 1.0)));
    class("Clock loads", clocked, k::CLOCK_LOAD * clk, &mut tiles);
    let bufg: Vec<(&str, f64)> = p.bufgs.iter().filter(|g| g.site.starts_with("BUFGCTRL")).map(|g| (g.tile.as_str(), 1.0)).collect();
    class("Clock buffers", bufg, k::BUFG * clk, &mut tiles);
    let dynamic: f64 = blocks.iter().map(|b| b.total_mw).sum();
    let static_mw = k::STATIC + k::STATIC_PER_BRAM * bram_tiles + k::STATIC_PER_DYNAMIC * dynamic;
    blocks.push(PowerBlock { block: "Device static".into(), used: 0, total_mw: static_mw });
    let mut tiles: Vec<TilePower> = tiles.into_iter().map(|((x, y), mw)| TilePower { x, y, mw }).collect();
    tiles.sort_by(|a, b| b.mw.total_cmp(&a.mw).then((a.x, a.y).cmp(&(b.x, b.y))));
    let method = format!(
        "vectorless estimate: static + dynamic per resource class with Helion's fitted coefficients, data toggle rate {:.1}% ({}), data and clock tree at {clk_name} ({f_mhz:.1} MHz, the clock with the most loads), linear in frequency; {POWER_FIT}",
        alpha * 100.0,
        if toggle.is_some() { "set by the design" } else { "Vivado's vectorless default" }
    );
    view.power = Some(PowerEstimate { total_mw: dynamic + static_mw, static_mw, dynamic_mw: dynamic, toggle_rate: alpha, blocks, tiles, method });
    view.set_provenance("power", Provenance::new("helion-targets view_xilinx7 power", "per-class counts from the placed and routed design", SCOPE));
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
    fill_congestion(&mut v);
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
    let toggle = b.design.attrs.get("DEFAULT_TOGGLE_RATE").and_then(|t| t.trim().trim_end_matches('%').parse::<f64>().ok()).map(|t| if t > 1.0 { t / 100.0 } else { t });
    fill_power(&mut v, &b.routed, &b.periods, toggle);
    fill_schematic(&mut v, b.rtl.as_ref(), &b.design);
    fill_board(&mut v);
    v
}

/// Xilinx primitive name of a synthesized cell, from its kind and pins.
pub fn primitive_of(c: &helion_ir::Cell, pins: &[NetlistPin]) -> String {
    use helion_ir::CellKind as K;
    let has = |p: &str| pins.iter().any(|x| x.name == p);
    match &c.kind {
        K::Lut6 { .. } => format!("LUT{}", pins.iter().filter(|p| !p.output && p.name.starts_with('I')).count().max(1)),
        K::Hff if has("ARST") => if c.attrs.get("ARST_VAL") == Some("1") { "FDPE" } else { "FDCE" }.into(),
        K::Hff if has("SR") || has("RST") => if c.attrs.get("SRVAL") == Some("1") { "FDSE" } else { "FDRE" }.into(),
        K::Hff => "FDRE".into(),
        K::IobIn => "IBUF".into(),
        K::IobOut => "OBUF".into(),
        K::ClockBuf => "BUFG".into(),
        K::Mac27 | K::Dsp48e1 { .. } => "DSP48E1".into(),
        K::Bram18 | K::Bram18e1 { .. } => "RAMB18E1".into(),
        K::Carry4 => "CARRY4".into(),
        K::Ila { .. } => "ILA".into(),
        K::BlackBox { module } => helion_ir::resolve(*module).to_string(),
    }
}

/// Whether a pin of a Helion IR cell drives its net: the cell's `OUT_PINS`
/// list when it has one, else the 7-series output pin names.
// shortcut: name-based for primitives without OUT_PINS; a new primitive with other output names reads as all inputs until it is listed here.
pub fn is_output_pin(c: &helion_ir::Cell, pin: &str) -> bool {
    if let Some(outs) = c.attrs.get("OUT_PINS") {
        return outs.split(',').any(|o| o == pin);
    }
    if matches!(c.kind, helion_ir::CellKind::IobOut) {
        return pin == "PAD";
    }
    let base = pin.split('[').next().unwrap_or(pin).trim_end_matches(|ch: char| ch.is_ascii_digit());
    matches!(base, "O" | "Q" | "CO" | "P" | "PCOUT" | "ACOUT" | "BCOUT" | "CARRYOUT" | "DOADO" | "DOBDO" | "DOPADOP" | "DOPBDOP" | "LOCKED" | "CLKOUT" | "CLKFBOUT")
}

/// The three schematic netlists: elaborated, synthesized, and synthesized
/// with the placement's site and BEL on each cell.
pub fn fill_schematic(view: &mut ImplView, rtl: Option<&Design>, design: &Design) {
    match rtl {
        Some(r) => {
            view.schematic.push(SchematicNetlist::from_ir(r, NetlistStage::Rtl, primitive_of, is_output_pin));
            view.set_provenance(
                NetlistStage::Rtl.section(),
                Provenance::new("helion-sv v2", "elaborated word-level netlist before bit blasting: one cell per operator, register and constant", SCOPE),
            );
        }
        None => view.mark_missing(
            NetlistStage::Rtl.section(),
            "the zb front end built this design (the v2 front end refused it); it keeps no elaborated netlist",
        ),
    }
    let syn = SchematicNetlist::from_ir(design, NetlistStage::Synthesized, primitive_of, is_output_pin);
    view.set_provenance(
        NetlistStage::Synthesized.section(),
        Provenance::new("helion-sv v2 lower", "the mapped netlist the packer reads (LUTs, flops, carry chains, IO and clock buffers)", SCOPE),
    );
    let mut imp = syn.clone();
    imp.stage = NetlistStage::Implemented;
    let at: HashMap<&str, &CellPlacement> = view.cells.iter().map(|c| (c.cell.as_str(), c)).collect();
    let mut placed = 0usize;
    for c in imp.cells.iter_mut() {
        if let Some(p) = at.get(c.name.as_str()) {
            c.site = Some(p.site.clone());
            c.bel = p.bel.clone();
            placed += 1;
        }
    }
    let names: HashSet<&str> = syn.cells.iter().map(|c| c.name.as_str()).collect();
    let mut added = 0usize;
    for p in view.cells.iter().filter(|p| !names.contains(p.cell.as_str())) {
        imp.cells.push(NetlistCell { name: p.cell.clone(), kind: p.kind.clone(), pins: Vec::new(), site: Some(p.site.clone()), bel: p.bel.clone() });
        added += 1;
    }
    view.set_provenance(
        NetlistStage::Implemented.section(),
        Provenance::new(
            "helion-xilinx7 place",
            format!(
                "synthesized netlist with the placement's site and BEL: {placed} of {} cells placed under their own name; {added} cells the packer made (buffers, split LUTs) carry a site but no pins",
                syn.cells.len()
            ),
            SCOPE,
        ),
    );
    view.schematic.push(syn);
    view.schematic.push(imp);
}

/// Board part kind from the board pin file's port name.
fn board_kind(port: &str) -> String {
    let base = port.split('[').next().unwrap_or(port);
    match base {
        "clk" => "Clock".into(),
        "sw" => "Switch".into(),
        "btn" => "Button".into(),
        "led" => "LED".into(),
        b if b.len() == 2 && b.starts_with('j') => format!("Pmod {}", b.to_ascii_uppercase()),
        b => match b.split('_').next().unwrap_or(b) {
            "oled" => "OLED".into(),
            "hd" => "HDMI".into(),
            "ac" => "Audio".into(),
            "vga" => "VGA".into(),
            other => other.to_ascii_uppercase(),
        },
    }
}

/// The board panel: the board's own pin file (each line's trailing comment
/// is the board's label), with the run's IO pins joined by package pin.
pub fn fill_board(view: &mut ImplView) {
    let Some(board) = helion_xilinx7::parts::board_for_part(&view.part) else {
        let known: Vec<&str> = helion_xilinx7::parts::BOARDS.iter().map(|b| b.part).collect();
        view.mark_missing("board", format!("no board in Helion's board table is built on {} (boards: {})", view.part, known.join(", ")));
        return;
    };
    let run: HashMap<&str, &str> = view.io.iter().flat_map(|io| io.pins.iter().map(|p| (p.package_pin.as_str(), p.port.as_str()))).collect();
    let mut groups: Vec<BoardGroup> = Vec::new();
    let mut on_board: HashSet<String> = HashSet::new();
    for line in board.pins.lines() {
        let (code, label) = line.split_once('#').unwrap_or((line, ""));
        for (prop, pin, port) in helion_xilinx7::xdc_flow::parse_xdc(code) {
            if !prop.eq_ignore_ascii_case("PACKAGE_PIN") {
                continue;
            }
            let kind = board_kind(&port);
            let item = BoardItem {
                label: if label.trim().is_empty() { port.clone() } else { label.trim().to_string() },
                port: run.get(pin.as_str()).map(|p| p.to_string()),
                pin: pin.clone(),
            };
            on_board.insert(pin);
            match groups.iter_mut().find(|g| g.kind == kind) {
                Some(g) => g.items.push(item),
                None => groups.push(BoardGroup { kind, items: vec![item] }),
            }
        }
    }
    let off_board = view
        .io
        .iter()
        .flat_map(|io| io.pins.iter())
        .filter(|p| !on_board.contains(&p.package_pin))
        .map(|p| (p.port.clone(), p.package_pin.clone()))
        .collect();
    view.board = Some(BoardPanel { board: board.title.to_string(), groups, off_board });
    view.set_provenance("board", Provenance::new("helion-xilinx7 parts", format!("{} joined to this run's IO pins by package pin", board.xdc), SCOPE));
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
    fn power_scales_with_frequency_and_toggle_and_lands_on_tiles() {
        use helion_xilinx7::place::PlacedFf;
        let mut v = ImplView::empty(FAMILY, "xc7z020clg484-1", ROUTED_STAGE);
        v.tiles.push(ViewTile { x: 3, y: 4, class: TileClass::Logic, name: "CLBLL_L_X0Y0".into(), clock_region: "X0Y0".into(), sites: vec![], io_bank: None });
        let clock = |period_ps| ClockUtilization {
            clocks: vec![ClockNet { name: "clk".into(), period_ps, loads: 2, regions: vec![], buffer: None }],
            loads_by_region: BTreeMap::new(),
            bufgs_used: 0,
            bufgs_available: 32,
        };
        let mut r = Routed7::default();
        for n in ["a", "b"] {
            r.placed.ffs.push(PlacedFf { name: n.into(), tile: "CLBLL_L_X0Y0".into(), site: "SLICE_X0Y0".into(), bel: "AFF".into() });
        }
        let run = |v: &mut ImplView, period, toggle| {
            v.clocks = Some(clock(period));
            fill_power(v, &r, &HashMap::new(), toggle);
            v.power.clone().unwrap()
        };
        let block = |p: &PowerEstimate, name: &str| p.blocks.iter().find(|b| b.block == name).unwrap().total_mw;
        let p = run(&mut v, 10_000, None);
        assert_eq!(p.toggle_rate, DEFAULT_TOGGLE_RATE);
        assert!((block(&p, "Registers") - 2.0 * power_coeffs::FF).abs() < 1e-12);
        assert!((block(&p, "Clock loads") - 2.0 * power_coeffs::CLOCK_LOAD).abs() < 1e-12);
        let tile_sum: f64 = p.tiles.iter().map(|t| t.mw).sum();
        assert!((tile_sum - p.dynamic_mw).abs() < 1e-9 && p.tiles.iter().all(|t| (t.x, t.y) == (3, 4)));
        assert!((p.total_mw - p.static_mw - p.dynamic_mw).abs() < 1e-9);
        // Twice the clock: twice the dynamic power. Twice the toggle rate:
        // twice the data part, the clock tree unchanged.
        let fast = run(&mut v, 5_000, None);
        assert!((fast.dynamic_mw - 2.0 * p.dynamic_mw).abs() < 1e-9);
        let busy = run(&mut v, 10_000, Some(0.25));
        assert!((block(&busy, "Registers") - 2.0 * block(&p, "Registers")).abs() < 1e-12);
        assert_eq!(block(&busy, "Clock loads"), block(&p, "Clock loads"));
        assert!(busy.method.contains("25.0%") && busy.method.contains("set by the design"), "{}", busy.method);
    }

    #[test]
    fn tile_index_reads_the_name_suffix() {
        assert_eq!(tile_index("INT_L_X26Y0"), Some((26, 0)));
        assert_eq!(tile_index("INT_R_X41Y149"), Some((41, 149)));
        assert_eq!(tile_index("INT_L"), None);
    }

    #[test]
    fn empty_view_records_no_die_instead_of_a_zero_grid() {
        let v = ImplView::empty(FAMILY, "xc7z020clg484-1", ROUTED_STAGE);
        assert_eq!(v.cols, 0);
        assert!(v.tiles.is_empty());
    }

    #[test]
    fn board_panel_joins_the_run_pins_to_the_board_parts_and_lists_the_rest() {
        let mut v = ImplView::empty(FAMILY, "xc7z020clg484-1", ROUTED_STAGE);
        let pin = |port: &str, pin: &str| IoPin { port: port.into(), package_pin: pin.into(), bank: None, iostandard: "LVCMOS33".into(), direction: "output" };
        v.io = Some(IoPlan { pins: vec![pin("oled_dc", "U10"), pin("led", "T22"), pin("dbg", "A1")], package_pins: 0 });
        fill_board(&mut v);
        let b = v.board.as_ref().expect("ZedBoard panel");
        let (g, i) = b.part_of("oled_dc").expect("oled_dc on the board");
        assert_eq!((g.kind.as_str(), i.label.as_str()), ("OLED", "OLED-DC"));
        assert_eq!(b.part_of("led").map(|(g, i)| (g.kind.as_str(), i.label.as_str())), Some(("LED", "LD0")));
        assert_eq!(b.off_board, vec![("dbg".to_string(), "A1".to_string())]);
        for k in ["Clock", "Switch", "Button", "LED", "Pmod JA", "Pmod JD", "OLED", "HDMI", "Audio", "VGA"] {
            assert!(b.groups.iter().any(|g| g.kind == k), "no {k} group");
        }
        let unused = b.groups.iter().flat_map(|g| &g.items).filter(|i| i.port.is_none()).count();
        assert_eq!(unused + 2, b.groups.iter().map(|g| g.items.len()).sum::<usize>(), "unused parts stay listed");
        let mut none = ImplView::empty(FAMILY, "xc7k325tffg900-2", ROUTED_STAGE);
        fill_board(&mut none);
        assert!(none.board.is_none() && none.missing["board"].contains("xc7k325tffg900-2"));
    }
}
