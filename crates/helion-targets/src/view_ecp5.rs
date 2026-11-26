//! Fill an [`ImplView`] from an ECP5 (`helion-ecp5`) implementation run.
//!
//! Read from the run's placed and routed design and the Trellis device
//! model; nothing is packed, placed or routed again.
//!
//! - **Tiles**: one per grid position (Trellis stacks several tile entries
//!   on a position; their sites are merged). Clock region is the global
//!   clock quadrant (`UL`, `UR`, `LL`, `LR`) from `globals.json`; IO bank
//!   from `iodb.json`.
//! - **Nets**: each routed net's programmed arcs as hops; carry nets are
//!   dedicated cascades, a global clock is its DCC input, spine and slice
//!   arcs.
//! - **Usage**: occupied sites per position; `H`/`V` wires a routed arc
//!   drives there against the `H`/`V` wires the position's muxes can drive.
//! - **Timing**: [`view_fill::graph_timing`] with the LFE5U-85F delay model.
//!
//! What the ECP5 flow does not have is named in [`ImplView::missing`]:
//! per-pin IO types, a power model, a hold corner, a board.
//!
//! No em dashes anywhere.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use helion_backend::TimingModel;
use helion_ecp5::device::SiteClass;
use helion_ecp5::pack::IoDir;
use helion_ecp5::place::{PlacedEcp5, SiteAddr};
use helion_ecp5::route::{NetKind, RoutedEcp5, RoutedSegment};
use helion_ecp5::Device;
use helion_ir::Design;

use crate::view::{CellPlacement, ClockNet, ClockUtilization, ImplView, IoPin, IoPlan, Overflow, Provenance, ResourceRow, RoutedNet, Stage, TileClass, TileUsage, ViewTile, WireHop};
use crate::view_fill::{self, GraphFf, GraphLut};
use crate::{Family, view_drc};

const SCOPE: &str = "routed design of this run";

fn tile_name(row: u32, col: u32) -> String {
    format!("R{row}C{col}")
}

fn site_name(s: &SiteAddr) -> String {
    format!("{}/{}", tile_name(s.tile_row, s.tile_col), s.site)
}

/// `'H'` or `'V'` for a Trellis span wire (`H02E0701`, `V06N0303`, with or
/// without a neighbour prefix such as `E1_`); `None` for anything else,
/// the tile-internal `H00`/`V00` stubs included.
pub fn wire_direction(name: &str) -> Option<char> {
    let base = match name.split_once('_') {
        Some((pre, rest)) if pre.len() >= 2 && pre.starts_with(['N', 'S', 'E', 'W']) && pre[1..].chars().all(|c| c.is_ascii_digit()) => rest,
        _ => name,
    };
    let b = base.as_bytes();
    if b.len() < 4 || !b[1].is_ascii_digit() || !b[2].is_ascii_digit() || &base[1..3] == "00" || !matches!(b[3], b'N' | b'S' | b'E' | b'W' | b'L' | b'R' | b'T' | b'B') {
        return None;
    }
    match b[0] {
        b'H' => Some('H'),
        b'V' => Some('V'),
        _ => None,
    }
}

fn class_of(c: SiteClass) -> TileClass {
    match c {
        SiteClass::Slice => TileClass::Logic,
        SiteClass::Ebr => TileClass::Ram,
        SiteClass::Dsp => TileClass::Dsp,
        SiteClass::Pio | SiteClass::IoLogic => TileClass::Io,
        SiteClass::Pll | SiteClass::Dcc | SiteClass::Dcs | SiteClass::ClkDiv | SiteClass::Eclk => TileClass::Clock,
        _ => TileClass::Other,
    }
}

/// The view of a routed ECP5 run. `design` is the netlist the packer read,
/// `clocks` the constrained clocks as `(source port or net, period in ps)`.
pub fn from_routed(part: &str, design: &Design, dev: &Device, p: &PlacedEcp5, routed: &RoutedEcp5, clocks: &[(String, u32)]) -> ImplView {
    let mut v = ImplView::empty(Family::Ecp5, part, Stage::Routed);
    let k = &p.packed;

    // Tiles, one per grid position.
    let bank_at: HashMap<(i32, i32), u32> = dev.pio_metadata().iter().map(|m| ((m.col as i32, m.row as i32), m.bank)).collect();
    let quadrant = |col: u32, row: u32| -> String {
        let mut q: Vec<&String> = dev.globals().quadrants.iter().filter(|(_, q)| (q.x0..=q.x1).contains(&col) && (q.y0..=q.y1).contains(&row)).map(|(n, _)| n).collect();
        q.sort();
        q.first().map(|s| s.to_string()).unwrap_or_default()
    };
    let mut at: BTreeMap<(u32, u32), ViewTile> = BTreeMap::new();
    // H and V wires each position's muxes can drive.
    let mut cap: HashMap<(u32, u32), (BTreeSet<&str>, BTreeSet<&str>)> = HashMap::new();
    for t in dev.tiles() {
        let e = at.entry((t.row, t.col)).or_insert_with(|| ViewTile { x: t.col, y: t.row, class: TileClass::Other, name: tile_name(t.row, t.col), clock_region: quadrant(t.col, t.row), sites: Vec::new(), io_bank: None });
        for s in &t.sites {
            e.class = e.class.min(class_of(s.class));
            e.sites.push(format!("{}/{}", e.name, s.name));
            if s.class == SiteClass::Pio {
                e.io_bank = e.io_bank.or(bank_at.get(&(s.bel_col, s.bel_row)).copied());
            }
        }
        if let Ok(bits) = dev.tile_bits(&t.tile_type) {
            let c = cap.entry((t.row, t.col)).or_default();
            for m in &bits.muxes {
                match wire_direction(&m.dst) {
                    Some('H') => drop(c.0.insert(m.dst.as_str())),
                    Some('V') => drop(c.1.insert(m.dst.as_str())),
                    _ => {}
                }
            }
        }
    }
    v.tiles = at.into_values().collect();
    v.cols = helion_ecp5::device::GRID_COLS;
    v.rows = helion_ecp5::device::GRID_ROWS;
    v.row0_top = true;
    v.set_provenance("tiles", Provenance::new("helion-ecp5 device", "Trellis tilegrid, the tile entries of one grid position merged; clock region is the global clock quadrant from globals.json; IO bank from iodb.json", part));

    // Placed cells.
    let mut cell = |name: &str, s: &SiteAddr, bel: Option<String>, kind: &str| {
        v.cells.push(CellPlacement { cell: name.to_string(), site: site_name(s), bel, tile: Some(tile_name(s.tile_row, s.tile_col)), kind: kind.to_string() });
    };
    for c in &p.luts {
        cell(&c.name, &c.site, Some(format!("LUT{}", c.half)), "LUT");
    }
    for c in &p.ffs {
        cell(&c.name, &c.site, Some(format!("FF{}", c.half)), "FF");
    }
    for c in &p.ios {
        cell(&c.name, &c.site, None, "PIO");
    }
    for c in &p.ebrs {
        cell(&c.name, &c.site, None, "EBR");
    }
    for c in &p.dsps {
        cell(&c.name, &c.site, None, "DSP");
    }
    for c in &p.plls {
        cell(&c.name, &c.site, None, "PLL");
    }
    for (name, site) in p.dcus.iter().map(|c| (&c.name, &c.site)).chain(p.extrefs.iter().map(|c| (&c.name, &c.site))) {
        cell(name, site, None, "DCU");
    }
    for (name, site) in p.ddrdlls.iter().map(|c| (&c.name, &c.site)).chain(p.dqs_blocks.iter().map(|c| (&c.name, &c.site))).chain(p.iologics.iter().map(|c| (&c.name, &c.site))) {
        cell(name, site, None, "IOLOGIC");
    }
    for c in &p.sys_configs {
        cell(&c.name, &c.site, None, "CONFIG");
    }
    v.set_provenance("placement", Provenance::new("helion-ecp5 place", "placed primitives with their tile, site and slice half", SCOPE));

    // Nets: pins from the placed netlist, hops from the router.
    let mut pins: HashMap<&str, (Option<String>, Vec<String>)> = HashMap::new();
    for l in &p.luts {
        pins.entry(l.output.as_str()).or_default().0 = Some(format!("{}.F", l.name));
        for (i, n) in l.inputs.iter().enumerate() {
            if let Some(n) = n {
                pins.entry(n.as_str()).or_default().1.push(format!("{}.{}", l.name, ["A", "B", "C", "D"][i]));
            }
        }
    }
    for f in &p.ffs {
        pins.entry(f.q.as_str()).or_default().0 = Some(format!("{}.Q", f.name));
        pins.entry(f.d.as_str()).or_default().1.push(format!("{}.DI", f.name));
        pins.entry(f.clk.as_str()).or_default().1.push(format!("{}.CLK", f.name));
        for (n, pin) in [(&f.ce, "CE"), (&f.sr, "LSR")] {
            if let Some(n) = n {
                pins.entry(n.as_str()).or_default().1.push(format!("{}.{pin}", f.name));
            }
        }
    }
    for i in &p.ios {
        match i.dir {
            IoDir::In => pins.entry(i.net.as_str()).or_default().0 = Some(format!("{}.O", i.name)),
            IoDir::Out => pins.entry(i.net.as_str()).or_default().1.push(format!("{}.I", i.name)),
        }
    }
    for c in &k.clks {
        pins.entry(c.in_net.as_str()).or_default().1.push(format!("{}.CLKI", c.name));
        pins.entry(c.out_net.as_str()).or_default().0 = Some(format!("{}.CLKO", c.name));
    }
    let hop = |s: &RoutedSegment| WireHop { tile: tile_name(s.tile_row, s.tile_col), wire: s.dst.clone(), pip: Some((s.src.clone(), s.dst.clone())), is_pseudo: false };
    let clock_hops: HashMap<&str, Vec<WireHop>> =
        routed.clocks.iter().map(|c| (c.net.as_str(), std::iter::once(&c.dcc_input).chain(&c.spine).chain(&c.slices).map(hop).collect())).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut nets = Vec::new();
    for n in &routed.nets {
        seen.insert(n.name.as_str());
        let (driver, loads) = pins.get(n.name.as_str()).cloned().unwrap_or_default();
        let (kind, hops) = match (clock_hops.get(n.name.as_str()), n.kind) {
            (Some(h), _) => ("clock-spine", h.clone()),
            (None, NetKind::Carry) => ("cascade-dedicated", Vec::new()),
            (None, NetKind::Const0 | NetKind::Const1) => ("constant", Vec::new()),
            (None, NetKind::Signal) if n.segments.is_empty() => ("site-internal", Vec::new()),
            (None, NetKind::Signal) => ("pips", n.segments.iter().map(hop).collect()),
        };
        nets.push(RoutedNet { net: n.name.clone(), driver, loads, hops, kind });
    }
    for (name, h) in &clock_hops {
        if !seen.contains(name) {
            let (driver, loads) = pins.get(name).cloned().unwrap_or_default();
            nets.push(RoutedNet { net: name.to_string(), driver, loads, hops: h.clone(), kind: "clock-spine" });
        }
    }
    nets.sort_by(|a, b| a.net.cmp(&b.net));
    v.nets = nets;
    v.set_provenance(
        "routing",
        Provenance::new("helion-ecp5 route", "each routed net's programmed arcs (source wire to destination wire in a tile); a global clock is its DCC input, spine and slice arcs; carry nets are dedicated cascades; driver and loads from the placed LUTs, flops, pads and clock buffers (block RAM and DSP pins are not listed)", SCOPE),
    );

    // Usage.
    let mut site_use: HashMap<&str, HashSet<&str>> = HashMap::new();
    for c in &v.cells {
        if let Some(t) = &c.tile {
            site_use.entry(t.as_str()).or_default().insert(c.site.as_str());
        }
    }
    let mut wire_use: HashMap<&str, (HashSet<&str>, HashSet<&str>)> = HashMap::new();
    for n in &v.nets {
        for h in &n.hops {
            let e = wire_use.entry(h.tile.as_str()).or_default();
            match wire_direction(&h.wire) {
                Some('H') => drop(e.0.insert(h.wire.as_str())),
                Some('V') => drop(e.1.insert(h.wire.as_str())),
                _ => {}
            }
        }
    }
    let mut usage = Vec::new();
    for t in &v.tiles {
        let (ch, cv) = cap.get(&(t.y, t.x)).map(|c| (c.0.len() as u32, c.1.len() as u32)).unwrap_or((0, 0));
        if ch == 0 && cv == 0 && t.sites.is_empty() {
            continue;
        }
        let (uh, uv) = wire_use.get(t.name.as_str()).map(|e| (e.0.len() as u32, e.1.len() as u32)).unwrap_or((0, 0));
        usage.push(TileUsage {
            x: t.x,
            y: t.y,
            sites_used: site_use.get(t.name.as_str()).map(|s| s.len() as u32).unwrap_or(0),
            sites_capacity: t.sites.len() as u32,
            overflow: (ch > 0 || cv > 0).then_some(Overflow { h_used: uh, h_capacity: ch, v_used: uv, v_capacity: cv }),
        });
    }
    v.usage = usage;
    v.set_provenance(
        "usage",
        Provenance::new("helion-ecp5 route", "occupied sites per grid position; H and V span wires a routed arc drives there, apart, against the H and V wires the position's routing muxes can drive (tile-internal H00/V00 stubs left out)", SCOPE),
    );
    view_fill::fill_congestion(&mut v, "helion-ecp5 route", crate::view_xilinx7::CONGESTION_THRESHOLD_PCT);

    // Utilization rows.
    let c = dev.counts();
    let mult18 = dev.tiles().iter().map(|t| t.sites.iter().filter(|s| s.name.starts_with("MULT18_")).count()).sum::<usize>();
    let package_pins = dev.package_pins(&p.package).map(|m| m.len()).unwrap_or(c.bonded_pios);
    v.resources = [
        ("LUT4", k.luts.len(), c.lut4),
        ("FF", k.ffs.len(), c.ff),
        ("EBR (18 Kb)", k.rams.len(), c.ebr_bels),
        ("I/O", k.ios.len(), package_pins),
        ("MULT18", k.dsps.len(), mult18),
        ("PLL", k.plls.len(), c.pll_bels),
        ("Global clock (DCC)", k.clks.len(), c.dcc_bels),
    ]
    .into_iter()
    .map(|(name, used, avail)| ResourceRow { name: name.into(), depth: 0, used: used as f64, available: avail as u32 })
    .collect();
    v.set_provenance("resources", Provenance::new("helion-ecp5 pack", "packed primitives of this run against the Trellis device's site counts", SCOPE));

    // Clocks.
    let root = |net: &str| -> String {
        let mut n = net;
        for _ in 0..8 {
            match k.clks.iter().find(|c| c.out_net == n) {
                Some(c) => n = c.in_net.as_str(),
                None => break,
            }
        }
        k.ios.iter().find(|i| i.dir == IoDir::In && i.net == n).map(|i| i.port.clone()).unwrap_or_else(|| n.to_string())
    };
    let period = |net: &str| -> Option<u32> {
        let r = root(net);
        clocks.iter().find(|(src, _)| *src == r || src == net).map(|c| c.1)
    };
    let region_of: HashMap<&str, &str> = v.tiles.iter().map(|t| (t.name.as_str(), t.clock_region.as_str())).collect();
    let clock_nets: BTreeSet<&str> = p.ffs.iter().map(|f| f.clk.as_str()).collect();
    let mut by_region: BTreeMap<String, usize> = BTreeMap::new();
    let nets: Vec<ClockNet> = clock_nets
        .iter()
        .map(|n| {
            let mut regions = BTreeSet::new();
            let mut loads = 0;
            for f in p.ffs.iter().filter(|f| f.clk == *n) {
                loads += 1;
                if let Some(r) = region_of.get(tile_name(f.site.tile_row, f.site.tile_col).as_str()).filter(|r| !r.is_empty()) {
                    regions.insert(r.to_string());
                    *by_region.entry(r.to_string()).or_insert(0) += 1;
                }
            }
            ClockNet { name: n.to_string(), period_ps: period(n).unwrap_or(0), loads, regions: regions.into_iter().collect(), buffer: p.clocks.iter().find(|c| c.net == *n).map(|c| c.dcc.clone()) }
        })
        .collect();
    v.clocks = Some(ClockUtilization { clocks: nets, loads_by_region: by_region, bufgs_used: p.clocks.len(), bufgs_available: c.dcc_bels });
    v.set_provenance("clocks", Provenance::new("helion-ecp5 place", "nets clocking a flop, their flops by global clock quadrant, and the DCC the placer assigned; period from the constraint on the net or on its pad (0 = unconstrained); block RAM and DSP clock pins are not counted", SCOPE));

    // Timing.
    let chain_at: HashMap<(usize, usize), usize> = p.luts.iter().enumerate().filter_map(|(i, l)| l.carry.map(|c| ((c.chain, c.seq), i))).collect();
    let luts: Vec<GraphLut> = p
        .luts
        .iter()
        .map(|l| GraphLut {
            name: l.name.clone(),
            inputs: l.inputs.iter().flatten().cloned().collect(),
            output: l.output.clone(),
            carry_prev: l.carry.and_then(|c| chain_at.get(&(c.chain, c.seq.checked_sub(1)?)).copied()),
            xy: (l.site.tile_col, l.site.tile_row),
        })
        .collect();
    let ffs: Vec<GraphFf> = p.ffs.iter().map(|f| GraphFf { name: f.name.clone(), d: f.d.clone(), q: f.q.clone(), clk: f.clk.clone(), xy: (f.site.tile_col, f.site.tile_row), site: site_name(&f.site) }).collect();
    match view_fill::graph_timing(&luts, &ffs, &TimingModel::ecp5_85f(), &period) {
        Some(t) => {
            v.timing = Some(t);
            v.set_provenance("timing", Provenance::new("helion-targets view_fill", view_fill::graph_timing_method("ECP5 LFE5U-85F delay model (order-of-magnitude values, not correlated against nextpnr or Diamond)"), SCOPE));
            v.mark_missing("hold", view_fill::NO_HOLD);
        }
        None => v.mark_missing("timing", if p.ffs.is_empty() { "the design has no flop, so no timed endpoint".to_string() } else { format!("no create_clock constraint reaches a flop's clock ({})", clock_nets.iter().copied().collect::<Vec<_>>().join(", ")) }),
    }

    // IO.
    let packed_io: HashMap<&str, &helion_ecp5::pack::PackedEcp5Io> = k.ios.iter().map(|i| (i.name.as_str(), i)).collect();
    let mut io: Vec<IoPin> = p
        .ios
        .iter()
        .map(|i| {
            let pk = packed_io.get(i.name.as_str());
            IoPin {
                port: i.port.clone(),
                package_pin: i.pin.clone(),
                bank: pk.map(|x| x.bank),
                iostandard: String::new(),
                direction: match i.dir {
                    IoDir::In => "input",
                    IoDir::Out => "output",
                },
                iostandard_default: false,
                pin_default: false,
                clock_capable: pk.map(|x| dev.pio_metadata().iter().any(|m| m.col == x.pio_col && m.row == x.pio_row && m.pio == x.pio && m.function.as_deref().is_some_and(|f| f.contains("PCLK")))),
            }
        })
        .collect();
    io.sort_by(|a, b| a.package_pin.cmp(&b.package_pin));
    v.io = Some(IoPlan { pins: io, package_pins });
    v.set_provenance("io", Provenance::new("helion-ecp5 pack/place", format!("placed pads with their constrained {} pin and its bank from iodb.json; clock capable when the pad's special function is a primary clock input (PCLK)", p.package), SCOPE));
    v.mark_missing("io-standard", "the ECP5 packer keeps no IO type per port: every pad is written with the flow's one default IO type");

    view_fill::fill_control_sets(
        &mut v,
        p.ffs.iter().map(|f| (f.clk.clone(), f.ce.clone().unwrap_or_default(), f.sr.clone().unwrap_or_default(), String::new(), f.name.clone())),
        "helion-ecp5 place",
        "placed flops grouped by (clock, enable, set/reset net); the placed flop does not record whether its set/reset is synchronous, so the kind column is empty",
    );
    let clock_names: HashSet<&str> = clock_nets.iter().copied().collect();
    view_fill::fill_high_fanout(&mut v, &clock_names, 10, "helion-ecp5 place");
    v.mark_missing("power", "Helion has no power model for ECP5: no capacitance or static current table for this family");
    v.mark_missing("board", "Helion's board table (boards/*.xdc) has no ECP5 board");
    view_fill::fill_schematic(
        &mut v,
        Err("the helion-sv front end that builds ECP5 designs bit-blasts while it elaborates and keeps no word-level netlist"),
        design,
        |c, pins| view_fill::primitive_name(c, pins, ["TRELLIS_FF", "TRELLIS_IO", "TRELLIS_IO", "DCCA", "DP16KD", "MULT18X18D", "CCU2C"]),
        view_fill::is_output_pin,
        ("helion-sv", "helion-ecp5 place"),
        SCOPE,
    );
    view_drc::fill(&mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_wires_have_a_direction_with_or_without_a_neighbour_prefix() {
        assert_eq!(wire_direction("H02E0701"), Some('H'));
        assert_eq!(wire_direction("V06N0303"), Some('V'));
        assert_eq!(wire_direction("E1_H02E0701"), Some('H'));
        assert_eq!(wire_direction("N3_V06S0003"), Some('V'));
        assert_eq!(wire_direction("H00L0000"), None, "a tile-internal stub is not a span");
        assert_eq!(wire_direction("F0_SLICE"), None);
        assert_eq!(wire_direction("G_HPBX0000"), None);
        assert_eq!(wire_direction("A0"), None);
    }
}
