//! Fill an [`ImplView`] from an iCE40 (`helion-ice40`) implementation run.
//!
//! Everything is read from the run's routed design and the IceStorm device
//! model: nothing is packed, placed or routed again.
//!
//! - **Tiles**: the HX8K grid; a logic tile holds eight logic cells, an IO
//!   tile two pads, a RAM pair one block RAM. IO bank from the die edge
//!   (IceStorm: 0 top, 1 right, 2 bottom, 3 left).
//! - **Nets**: each routed net's programmed edges as hops; nets the router
//!   does not carry are site-internal or ride a global network.
//! - **Usage**: occupied sites per tile; span wires a routed edge drives in
//!   the tile, by direction, against the span wires the tile can drive.
//! - **Timing**: [`view_fill::graph_timing`] with the HX8K delay model.
//!
//! What iCE40 does not have is named in [`ImplView::missing`]: clock
//! regions, per-pin IO standards, a power model, a hold corner, a board.
//!
//! No em dashes anywhere.

use std::collections::{BTreeSet, HashMap, HashSet};

use helion_backend::TimingModel;
use helion_ice40::device::{MAX_X, MAX_Y, TileKind, tile_kind};
use helion_ice40::pack::IoDir;
use helion_ice40::route::{RoutedIce, Router};
use helion_ir::Design;

use crate::view::{CellPlacement, ClockNet, ClockUtilization, ImplView, IoPin, IoPlan, Overflow, Provenance, ResourceRow, RoutedNet, Stage, TileClass, TileUsage, ViewTile, WireHop};
use crate::view_fill::{self, GraphFf, GraphLut};
use crate::{Family, view_drc};

const SCOPE: &str = "routed design of this run";

fn tile_name(x: u8, y: u8) -> String {
    format!("X{x}Y{y}")
}

/// `'H'` or `'V'` for a span wire, `None` for a local or site wire.
pub fn wire_direction(name: &str) -> Option<char> {
    if !(name.starts_with("sp") && (name.starts_with("sp4") || name.starts_with("sp12") || name.starts_with("span"))) {
        return None;
    }
    if name.contains("_h_") || name.contains("horz") {
        Some('H')
    } else if name.contains("_v_") || name.contains("vert") {
        Some('V')
    } else {
        None
    }
}

/// IO bank of an IO tile kind (IceStorm numbering).
fn bank_of(kind: TileKind) -> Option<u32> {
    Some(match kind {
        TileKind::IoTop => 0,
        TileKind::IoRight => 1,
        TileKind::IoBottom => 2,
        TileKind::IoLeft => 3,
        _ => return None,
    })
}

fn sites_of(x: u8, y: u8, kind: TileKind) -> Vec<String> {
    let t = tile_name(x, y);
    match kind {
        TileKind::Logic => (0..8).map(|i| format!("{t}/lc{i}")).collect(),
        TileKind::Ramb => vec![format!("{t}/ram")],
        TileKind::Ramt => Vec::new(),
        _ => (0..2).map(|i| format!("{t}/io{i}")).collect(),
    }
}

/// The view of a routed iCE40 run. `design` is the netlist the packer read,
/// `clocks` the constrained clocks as `(source port or net, period in ps)`.
pub fn from_routed(part: &str, design: &Design, routed: &RoutedIce, router: &Router, clocks: &[(String, u32)]) -> ImplView {
    let mut v = ImplView::empty(Family::Ice40, part, Stage::Routed);
    let p = &routed.placed;
    let k = &p.packed;

    // Tiles.
    for y in 0..=MAX_Y {
        for x in 0..=MAX_X {
            let Some(kind) = tile_kind(x, y) else { continue };
            v.tiles.push(ViewTile {
                x: x as u32,
                y: y as u32,
                class: match kind {
                    TileKind::Logic => TileClass::Logic,
                    TileKind::Ramb | TileKind::Ramt => TileClass::Ram,
                    _ => TileClass::Io,
                },
                name: tile_name(x, y),
                clock_region: String::new(),
                sites: sites_of(x, y, kind),
                io_bank: bank_of(kind),
            });
        }
    }
    v.cols = MAX_X as u32 + 1;
    v.rows = MAX_Y as u32 + 1;
    v.row0_top = false;
    v.set_provenance("tiles", Provenance::new("helion-ice40 device", "IceStorm HX8K grid: eight logic cells per logic tile, two pads per IO tile, one block RAM per RAM tile pair; IO bank from the die edge", part));
    v.mark_missing("clock-regions", "iCE40 has no clock regions: its eight global networks reach every tile through column buffers");

    // Placed cells.
    let mut cell = |name: &str, x: u8, y: u8, site: String, bel: Option<&str>, kind: &str| {
        v.cells.push(CellPlacement { cell: name.to_string(), site: format!("{}/{site}", tile_name(x, y)), bel: bel.map(str::to_string), tile: Some(tile_name(x, y)), kind: kind.to_string() });
    };
    for c in &p.luts {
        cell(&c.name, c.x, c.y, format!("lc{}", c.lc), Some("LUT4"), "LUT");
    }
    for c in &p.ffs {
        cell(&c.name, c.x, c.y, format!("lc{}", c.lc), Some("DFF"), "FF");
    }
    for c in &p.ios {
        cell(&c.name, c.x, c.y, format!("io{}", c.iob), None, "IO");
    }
    for c in &p.rams {
        cell(&c.name, c.x, c.y, "ram".into(), None, "RAM");
    }
    for c in &p.plls {
        cell(&c.name, c.x, c.y, "pll".into(), None, "PLL");
    }
    v.set_provenance("placement", Provenance::new("helion-ice40 place", "placed LUTs, flops, pads, block RAMs and PLLs with their tile and logic cell", SCOPE));

    // Nets: driver and loads from the placed netlist, hops from the router.
    let mut pins: HashMap<&str, (Option<String>, Vec<String>)> = HashMap::new();
    for l in &p.luts {
        pins.entry(l.output_net.as_str()).or_default().0 = Some(format!("{}.O", l.name));
        for (i, n) in l.input_nets.iter().enumerate() {
            if let Some(n) = n {
                pins.entry(n.as_str()).or_default().1.push(format!("{}.I{i}", l.name));
            }
        }
    }
    for f in &p.ffs {
        pins.entry(f.q_net.as_str()).or_default().0 = Some(format!("{}.Q", f.name));
        pins.entry(f.d_net.as_str()).or_default().1.push(format!("{}.D", f.name));
        pins.entry(f.clk_net.as_str()).or_default().1.push(format!("{}.CLK", f.name));
    }
    for i in &p.ios {
        match i.dir {
            IoDir::In => pins.entry(i.net.as_str()).or_default().0 = Some(format!("{}.D_IN", i.name)),
            IoDir::Out => pins.entry(i.net.as_str()).or_default().1.push(format!("{}.D_OUT", i.name)),
        }
    }
    for c in &k.clks {
        pins.entry(c.in_net.as_str()).or_default().1.push(format!("{}.I", c.name));
        pins.entry(c.out_net.as_str()).or_default().0 = Some(format!("{}.O", c.name));
    }
    let global: HashSet<&str> = k.clks.iter().map(|c| c.out_net.as_str()).chain(k.plls.iter().map(|c| c.out_net.as_str())).collect();
    let routed_by: HashMap<&str, &helion_ice40::route::RoutedNet> = routed.nets.iter().map(|n| (n.name.as_str(), n)).collect();
    let mut names: Vec<&str> = pins.keys().copied().chain(routed_by.keys().copied()).collect::<BTreeSet<_>>().into_iter().collect();
    names.sort_unstable();
    for name in names {
        let (driver, loads) = pins.get(name).cloned().unwrap_or_default();
        let edges = routed_by.get(name).map(|n| n.edges.as_slice()).unwrap_or(&[]);
        let hops: Vec<WireHop> = edges
            .iter()
            .map(|e| {
                let (src, dst) = (router.rel_src(e.kind_idx, e.rel_idx), router.rel_dst(e.kind_idx, e.rel_idx));
                WireHop { tile: tile_name(e.tile_x, e.tile_y), wire: dst.to_string(), pip: Some((src.to_string(), dst.to_string())), is_pseudo: false }
            })
            .collect();
        let kind = if !hops.is_empty() {
            "pips"
        } else if global.contains(name) {
            "global"
        } else {
            "site-internal"
        };
        v.nets.push(RoutedNet { net: name.to_string(), driver, loads, hops, kind });
    }
    v.set_provenance(
        "routing",
        Provenance::new("helion-ice40 route", "each routed net's programmed edges (source wire to destination wire in a tile); a net with no edge is inside one logic cell or rides a global network; driver and loads from the placed netlist", SCOPE),
    );

    // Usage: sites and span wires per tile.
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
    let mut cap: HashMap<TileKind, (u32, u32)> = HashMap::new();
    let mut usage = Vec::new();
    for t in &v.tiles {
        let Some(kind) = tile_kind(t.x as u8, t.y as u8) else { continue };
        let (ch, cv) = *cap.entry(kind).or_insert_with(|| {
            let (mut h, mut vv) = (BTreeSet::new(), BTreeSet::new());
            for w in router.tile_edge_dsts(kind) {
                match wire_direction(w) {
                    Some('H') => drop(h.insert(w)),
                    Some('V') => drop(vv.insert(w)),
                    _ => {}
                }
            }
            (h.len() as u32, vv.len() as u32)
        });
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
        Provenance::new(
            "helion-ice40 route",
            "occupied logic cells, pads and RAMs per tile; span wires (sp4, sp12, span4, span12) a routed edge drives in the tile, horizontal and vertical apart, against the span wires of that direction the tile's routing database can drive; a span wire is counted in the tile that drives it",
            SCOPE,
        ),
    );
    view_fill::fill_congestion(&mut v, "helion-ice40 route", crate::view_xilinx7::CONGESTION_THRESHOLD_PCT);

    // Utilization rows against the HX8K capacities.
    let used = [k.luts.len(), k.ffs.len(), k.rams.len(), k.ios.len(), k.plls.len(), k.clks.len()];
    v.resources = crate::reports::ICE40_HX8K.iter().zip(used).map(|((name, avail), u)| ResourceRow { name: name.to_string(), depth: 0, used: u as f64, available: *avail as u32 }).collect();
    v.set_provenance("resources", Provenance::new("helion-ice40 pack", "packed LUT4s, flops, block RAMs, pads, PLLs and global buffers of this run against the HX8K datasheet capacities", SCOPE));

    // Clocks: every net that clocks a flop or a RAM port, traced to its pad.
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
    let mut loads: Vec<&str> = p.ffs.iter().map(|f| f.clk_net.as_str()).collect();
    loads.extend(k.rams.iter().flat_map(|r| [r.rclk_net.as_str(), r.wclk_net.as_str()]).filter(|n| !n.is_empty()));
    let plans = helion_ice40::gclk::plan(p).unwrap_or_default();
    let clock_nets: BTreeSet<&str> = loads.iter().copied().collect();
    let nets: Vec<ClockNet> = clock_nets
        .iter()
        .map(|n| ClockNet {
            name: n.to_string(),
            period_ps: period(n).unwrap_or(0),
            loads: loads.iter().filter(|l| *l == n).count(),
            regions: Vec::new(),
            buffer: plans.iter().find(|pl| pl.nets.iter().any(|x| x == n)).map(|pl| format!("glb_netwk_{}", pl.glb)),
        })
        .collect();
    v.clocks = Some(ClockUtilization { clocks: nets, loads_by_region: Default::default(), bufgs_used: routed.glb_clocks.len(), bufgs_available: 8 });
    v.set_provenance("clocks", Provenance::new("helion-ice40 pack/route", "nets clocking a flop or a block RAM port; period from the constraint on the net or on the pad it comes from (0 = unconstrained); buffer is the global network the router assigned", SCOPE));

    // Timing over the placed logic graph.
    let carry: HashMap<&str, (usize, usize)> = k.luts.iter().filter_map(|l| Some((l.name.as_str(), l.carry.as_ref().map(|c| (c.chain, c.seq))?))).collect();
    let chain_at: HashMap<(usize, usize), usize> = p.luts.iter().enumerate().filter_map(|(i, l)| Some((*carry.get(l.name.as_str())?, i))).collect();
    let luts: Vec<GraphLut> = p
        .luts
        .iter()
        .map(|l| GraphLut {
            name: l.name.clone(),
            inputs: l.input_nets.iter().enumerate().filter(|(i, _)| !(l.carry && *i == 3)).filter_map(|(_, n)| n.clone()).collect(),
            output: l.output_net.clone(),
            carry_prev: carry.get(l.name.as_str()).and_then(|&(c, s)| chain_at.get(&(c, s.checked_sub(1)?)).copied()),
            xy: (l.x as u32, l.y as u32),
        })
        .collect();
    let ffs: Vec<GraphFf> = p
        .ffs
        .iter()
        .map(|f| GraphFf { name: f.name.clone(), d: f.d_net.clone(), q: f.q_net.clone(), clk: f.clk_net.clone(), xy: (f.x as u32, f.y as u32), site: format!("{}/lc{}", tile_name(f.x, f.y), f.lc) })
        .collect();
    match view_fill::graph_timing(&luts, &ffs, &TimingModel::ice40_hx8k(), &period) {
        Some(t) => {
            v.timing = Some(t);
            v.set_provenance("timing", Provenance::new("helion-targets view_fill", view_fill::graph_timing_method("iCE40-HX8K delay model (datasheet anchored, not correlated against nextpnr or iCEcube2)"), SCOPE));
            v.mark_missing("hold", view_fill::NO_HOLD);
        }
        None => v.mark_missing("timing", if p.ffs.is_empty() { "the design has no flop, so no timed endpoint".to_string() } else { format!("no create_clock constraint reaches a flop's clock ({})", clock_nets.iter().copied().collect::<Vec<_>>().join(", ")) }),
    }

    // IO.
    let pin_of: HashMap<&str, &str> = k.ios.iter().map(|i| (i.name.as_str(), i.pin.as_str())).collect();
    let mut io: Vec<IoPin> = p
        .ios
        .iter()
        .map(|i| IoPin {
            port: i.port.clone(),
            package_pin: pin_of.get(i.name.as_str()).copied().unwrap_or_default().to_string(),
            bank: tile_kind(i.x, i.y).and_then(bank_of),
            iostandard: String::new(),
            direction: match i.dir {
                IoDir::In => "input",
                IoDir::Out => "output",
            },
            iostandard_default: false,
            pin_default: false,
            // shortcut: a pad in a global-buffer IO tile counts as clock capable; upgrade to the exact pad of the tile (icebox padin_pio_db).
            clock_capable: Some(helion_ice40::gclk::GBUFIN_8K.contains(&(i.x, i.y))),
        })
        .collect();
    io.sort_by(|a, b| a.package_pin.cmp(&b.package_pin));
    v.io = Some(IoPlan { pins: io, package_pins: crate::reports::ICE40_HX8K[3].1 });
    v.set_provenance("io", Provenance::new("helion-ice40 pack/place", "placed pads with their constrained package pin; bank from the pad's die edge; clock capable when the pad's tile holds a global buffer input; package pin count from the CT256 datasheet (the flow's pin map covers the breakout board's pins)", SCOPE));
    v.mark_missing("io-standard", "the iCE40 flow programs no per-pin IO standard: an HX pad's level follows its bank's VCCIO supply");

    view_fill::fill_control_sets(
        &mut v,
        k.ffs.iter().map(|f| (f.clk_net.clone(), String::new(), String::new(), String::new(), f.name.clone())),
        "helion-ice40 pack",
        "packed flops grouped by clock; the iCE40 packer maps every flop to a plain DFF and folds enable and reset into LUT logic, so a control set is its clock",
    );
    let clock_names: HashSet<&str> = clock_nets.iter().copied().collect();
    view_fill::fill_high_fanout(&mut v, &clock_names, 10, "helion-ice40 place");
    v.mark_missing("power", "Helion has no power model for iCE40: no capacitance or static current table for this family");
    v.mark_missing("board", "Helion's board table (boards/*.xdc) has no iCE40 board");
    view_fill::fill_schematic(
        &mut v,
        Err("the helion-sv front end that builds iCE40 designs bit-blasts while it elaborates and keeps no word-level netlist"),
        design,
        |c, pins| view_fill::primitive_name(c, pins, ["SB_DFF", "SB_IO", "SB_IO", "SB_GB", "SB_RAM40_4K", "SB_MAC16", "SB_CARRY"]),
        view_fill::is_output_pin,
        ("helion-sv", "helion-ice40 place"),
        SCOPE,
    );
    view_drc::fill(&mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_wires_have_a_direction_and_local_wires_none() {
        assert_eq!(wire_direction("sp4_h_r_12"), Some('H'));
        assert_eq!(wire_direction("sp12_h_l_3"), Some('H'));
        assert_eq!(wire_direction("sp4_v_b_7"), Some('V'));
        assert_eq!(wire_direction("sp4_r_v_b_40"), Some('V'));
        assert_eq!(wire_direction("span4_horz_5"), Some('H'));
        assert_eq!(wire_direction("span12_vert_1"), Some('V'));
        assert_eq!(wire_direction("local_g0_3"), None);
        assert_eq!(wire_direction("lutff_2/in_1"), None);
        assert_eq!(wire_direction("glb_netwk_4"), None);
    }
}
