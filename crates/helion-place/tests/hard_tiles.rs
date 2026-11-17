//! No placer puts a LUT or flop on a tile of a hard block connected to the
//! fabric (bitgen refuses such a bitstream): the region (pblock),
//! incremental, analytical, congestion-spread, phys-opt, detailed and
//! macro-column placers, each from its own entry point, on designs whose
//! RAM and multiplier tiles are where their logic wants to be.

use helion_device::{Device, Site};
use helion_ir::{CellKind, Design};
use helion_pack::{pack, Packed};
use helion_place::regions::{RegionConstraint, place_with_regions, region_violations};
use helion_place::*;
use std::collections::HashSet;

struct Nl {
    d: Design,
    k: usize,
}

impl Nl {
    fn new() -> Self {
        let mut d = Design::new("top");
        d.add_port("clk", helion_ir::PortDir::In);
        Nl { d, k: 0 }
    }
    fn fresh(&mut self, p: &str) -> String {
        self.k += 1;
        format!("{p}{}", self.k)
    }
    fn inputs(&mut self, name: &str, n: usize) -> Vec<String> {
        self.d.add_port(name, helion_ir::PortDir::In);
        (0..n).map(|i| format!("{name}_{i}")).collect()
    }
    fn lut(&mut self, ins: &[&str], f: impl Fn(u32) -> bool) -> String {
        let mask = (1u32 << ins.len()) - 1;
        let init = (0..64u32).fold(0u64, |a, x| a | ((f(x & mask) as u64) << x));
        let (cell, out) = (self.fresh("u_lut"), self.fresh("n"));
        self.d.add_cell(&cell, CellKind::Lut6 { init });
        for (i, n) in ins.iter().enumerate() {
            self.d.connect(n, &cell, &format!("I{i}"));
        }
        self.d.connect(&out, &cell, "O");
        out
    }
    fn ff_to(&mut self, d: &str, q: &str) {
        self.ff_on("clk", d, q);
    }
    fn ff_on(&mut self, clk: &str, d: &str, q: &str) {
        let cell = self.fresh("u_ff");
        self.d.add_cell(&cell, CellKind::Hff);
        self.d.connect(clk, &cell, "CLK");
        self.d.connect(d, &cell, "D");
        self.d.connect(q, &cell, "Q");
    }
    fn ff(&mut self, d: &str) -> String {
        let q = self.fresh("q");
        self.ff_to(d, &q);
        q
    }
    fn counter(&mut self, n: usize) -> Vec<String> {
        let q: Vec<String> = (0..n).map(|_| self.fresh("cnt")).collect();
        let mut carry: Option<String> = None;
        for i in 0..n {
            let next = match &carry {
                None => self.lut(&[&q[i]], |x| x & 1 == 0),
                Some(c) => self.lut(&[&q[i], c], |x| (x & 1) ^ (x >> 1) == 1),
            };
            let c2 = match &carry {
                None => q[i].clone(),
                Some(c) => self.lut(&[&q[i], c], |x| x == 3),
            };
            self.ff_to(&next, &q[i]);
            carry = Some(c2);
        }
        q
    }
    fn output(&mut self, name: &str, net: &str) {
        self.d.add_port(name, helion_ir::PortDir::Out);
        let cell = format!("pad_{name}");
        self.d.add_cell(&cell, CellKind::IobOut);
        self.d.connect(net, &cell, "I");
        self.d.connect(name, &cell, "PAD");
    }
}

/// A RAM at a counter's addresses, its DO through LUTs and flops, and a
/// registered multiplier of two flopped buses: about 60 clusters, all
/// talking to the two blocks.
fn design() -> Design {
    design_with(0, 0)
}

/// [`design`] with `extra` more clusters (shift registers from the RAM's
/// outputs into the multiplier's inputs) and `macs` more multipliers,
/// each multiplying two of those registers into a LUT.
fn design_with(extra: usize, macs: usize) -> Design {
    let mut n = Nl::new();
    let d = n.inputs("d", 8);
    let cnt = n.counter(10);
    n.d.add_cell("u_ram", CellKind::Bram18);
    n.d.connect("clk", "u_ram", "CLK");
    for i in 0..10 {
        n.d.connect(&cnt[i], "u_ram", &format!("WADDR{i}"));
        let r = n.lut(&[&cnt[i], &d[i % 8]], |x| x == 1);
        n.d.connect(&r, "u_ram", &format!("RADDR{i}"));
    }
    for i in 0..8 {
        let q = n.ff(&d[i]);
        n.d.connect(&q, "u_ram", &format!("DI{i}"));
    }
    n.d.connect(&d[0], "u_ram", "WE");
    let mut acc = Vec::new();
    for i in 0..8 {
        n.d.connect(&format!("do{i}"), "u_ram", &format!("DO{i}"));
        let x = n.lut(&[&format!("do{i}"), &cnt[i]], |x| x != 0);
        acc.push(n.ff(&x));
    }
    for k in 0..extra {
        let i = k % 8;
        let x = n.lut(&[&acc[i], &format!("do{}", (k / 8) % 8)], |x| x == 1);
        acc[i] = n.ff(&x);
    }
    n.d.add_cell("u_mul", CellKind::Mac27);
    n.d.set_cell_attr("u_mul", "PREG", "1").unwrap();
    n.d.connect("clk", "u_mul", "CLK");
    for i in 0..8 {
        n.d.connect(&acc[i], "u_mul", &format!("A{i}"));
        n.d.connect(&acc[7 - i], "u_mul", &format!("B{i}"));
    }
    for i in 0..8 {
        n.d.connect(&format!("p{i}"), "u_mul", &format!("P{i}"));
        let y = n.lut(&[&format!("p{i}"), &acc[i]], |x| x == 2);
        let q = n.ff(&y);
        n.output(&format!("o{i}"), &q);
    }
    for m in 0..macs {
        let cell = format!("u_mul_x{m}");
        n.d.add_cell(&cell, CellKind::Mac27);
        n.d.connect(&acc[m % 8], &cell, "A0");
        n.d.connect(&acc[(m + 1) % 8], &cell, "B0");
        let p = format!("px{m}");
        n.d.connect(&p, &cell, "P0");
        let y = n.lut(&[&p, &acc[(m + 2) % 8]], |x| x == 1);
        let q = n.ff(&y);
        n.output(&format!("ox{m}"), &q);
    }
    n.d
}

fn setup() -> (Device, Packed) {
    let dev = Device::load_part("HL10S-C64-1").unwrap();
    let packed = pack(&design(), &dev).unwrap();
    (dev, packed)
}

/// Clusters on a tile of a connected block, and a check that both
/// blocks are still on whole sites of their kind.
fn on_block_tiles(p: &Placed, dev: &Device) -> Vec<usize> {
    let mut sites = HashSet::new();
    for b in 0..p.packed.conn.hard.len() as u32 {
        let s = p.hard_block_site(b).expect("block placed");
        let (kind, _) = dev.hard_site(s).expect("a hard-block site");
        assert_eq!(dev.hard_tiles(s).len(), kind.tiles() as usize, "{s:?} has not all of its tiles");
        assert!(sites.insert(s), "two blocks on {s:?}");
    }
    let reserved = p.reserved_tiles(dev);
    assert!(!reserved.is_empty());
    (0..p.lutff_sites.len()).filter(|&i| reserved.contains(&(p.lutff_sites[i].0.x, p.lutff_sites[i].0.y))).collect()
}

/// No two clusters on one BLE.
fn distinct(p: &Placed) {
    let mut seen = HashSet::new();
    for (s, b) in &p.lutff_sites {
        assert!(seen.insert((s.x, s.y, *b)), "two clusters on {s:?} BLE{b}");
    }
}

/// `p` with clusters moved onto every tile of both blocks (onto free
/// BLEs), as a placer that knew nothing of them would leave it.
fn squatted(p: &Placed, dev: &Device) -> Placed {
    let mut p = p.clone();
    let tiles: Vec<(u32, u32)> = p.reserved_tiles(dev).into_iter().collect();
    for (k, t) in tiles.iter().enumerate() {
        p.lutff_sites[k] = (Site { x: t.0, y: t.1, kind: helion_device::SiteKind::Clb }, 0);
    }
    assert_eq!(on_block_tiles(&p, dev).len(), tiles.len());
    p
}

fn the_main_placement() -> (Device, Placed) {
    let (dev, packed) = setup();
    let p = place(&packed, &dev).unwrap();
    assert!(on_block_tiles(&p, &dev).is_empty());
    (dev, p)
}

/// A pblock whose first CLB (in fill order) is the RAM's tile.
#[test]
fn region_placer() {
    let (dev, base) = the_main_placement();
    let ram = base.bram_sites[0];
    let (_, packed) = setup();
    let p = place_in_region(&packed, &dev, PlaceOpts::default(), ram.x, ram.y, ram.x + 2, ram.y + 6).unwrap();
    assert_eq!(p.bram_sites[0], ram);
    assert!(on_block_tiles(&p, &dev).is_empty(), "{:?}", on_block_tiles(&p, &dev));
    assert!(p.lutff_sites.iter().all(|(s, _)| s.x >= ram.x && s.x <= ram.x + 2 && s.y >= ram.y && s.y <= ram.y + 6));
}

/// The previous placement had the RAM on another site and clusters on
/// the tile where the fresh placement puts it: the RAM goes back to its
/// previous site, the clusters keep theirs, and nothing is on a block.
#[test]
fn incremental_placer() {
    let (dev, fresh) = the_main_placement();
    let mut prev = fresh.clone();
    let ram = fresh.bram_sites[0];
    let other = dev.bram_sites().find(|s| *s != ram && s.y > ram.y + 4).unwrap();
    prev.bram_sites[0] = other;
    // Clusters 0..3 sat on the fresh RAM tile; the tile of the previous
    // RAM site is free.
    for k in 0..3 {
        prev.lutff_sites[k] = (Site { x: ram.x, y: ram.y, kind: helion_device::SiteKind::Clb }, k as u8);
    }
    for k in 3..prev.lutff_sites.len() {
        if (prev.lutff_sites[k].0.x, prev.lutff_sites[k].0.y) == (other.x, other.y) {
            prev.lutff_sites[k].0.x += 1;
        }
    }
    let (_, packed) = setup();
    let (p, reused) = place_incremental(&packed, &dev, &prev, PlaceOpts::default()).unwrap();
    assert!(on_block_tiles(&p, &dev).is_empty(), "{:?}", on_block_tiles(&p, &dev));
    assert_eq!(p.bram_sites[0], other, "the RAM keeps its previous site");
    assert_eq!(reused, packed.lutffs.len());
    // A previous placement with the RAM where the fresh one puts it, and
    // clusters on its tile: they move off it, and do not count as reused.
    let mut prev2 = prev.clone();
    prev2.bram_sites[0] = ram;
    let (p, reused) = place_incremental(&packed, &dev, &prev2, PlaceOpts::default()).unwrap();
    assert!(on_block_tiles(&p, &dev).is_empty(), "{:?}", on_block_tiles(&p, &dev));
    assert_eq!(reused, packed.lutffs.len() - 3);
}

/// A pblock owning every LUT and flop of the design, around the RAM's
/// tile, and owning the multiplier from a site outside it.
#[test]
fn pblock_regions() {
    let (dev, base) = the_main_placement();
    let ram = base.bram_sites[0];
    let (_, packed) = setup();
    let mac_in = dev.dsp_sites().find(|s| *s != base.mac_sites[0]).unwrap();
    let (x0, x1) = (mac_in.x.min(ram.x) - 1, mac_in.x.max(ram.x) + 1);
    let (y0, y1) = (ram.y.min(mac_in.y), ram.y.max(mac_in.y + 1) + 2);
    let mut cells: Vec<String> = packed.lutffs.iter().flat_map(|l| [l.lut_cell.clone(), l.ff_cell.clone()]).filter(|c| !c.is_empty()).collect();
    cells.push("u_mul".into());
    cells.push("u_ram".into());
    let r = RegionConstraint::new("pb", x0, y0, x1, y1).with_cells(cells);
    let regions = [r];
    let (p, stats) = place_with_regions(&packed, &dev, PlaceOpts::default(), &regions, &TimingGuide::default()).unwrap();
    assert!(region_violations(&p, &regions).is_empty(), "{:?}", region_violations(&p, &regions));
    assert!(on_block_tiles(&p, &dev).is_empty(), "{:?}", on_block_tiles(&p, &dev));
    distinct(&p);
    assert_eq!(stats[0].hard_blocks, 2);
    // Both tiles of the multiplier are in the region.
    for t in dev.hard_tiles(p.mac_sites[0]) {
        assert!(regions[0].contains(t.0, t.1));
    }
    // A pblock that covers only the multiplier's base tile cannot have it.
    let half = RegionConstraint::new("half", mac_in.x, mac_in.y, mac_in.x, mac_in.y).with_cells(["u_mul"]);
    let err = place_with_regions(&packed, &dev, PlaceOpts::default(), &[half], &TimingGuide::default()).unwrap_err();
    assert!(err.contains("covers no Dsp site"), "{err}");
}

/// Clusters stacked beside the RAM, spread one per tile, and clusters
/// already on the blocks' tiles.
#[test]
fn congestion_spread() {
    let (dev, base) = the_main_placement();
    let ram = base.bram_sites[0];
    let mut p = base.clone();
    for k in 0..8 {
        p.lutff_sites[k] = (Site { x: ram.x + 1, y: ram.y, kind: helion_device::SiteKind::Clb }, k as u8);
    }
    let (out, moved) = place_congestion_spread(&p, &dev, 1).unwrap();
    assert!(moved >= 7);
    assert!(on_block_tiles(&out, &dev).is_empty(), "{:?}", on_block_tiles(&out, &dev));
    distinct(&out);
    let (out, _) = place_congestion_spread(&squatted(&base, &dev), &dev, 8).unwrap();
    assert!(on_block_tiles(&out, &dev).is_empty(), "{:?}", on_block_tiles(&out, &dev));
    distinct(&out);
}

/// The RAM's tile sits between a cluster's driver and its sinks: phys-opt
/// pulls it toward them, never onto the tile.
fn pulled_toward_the_ram(dev: &Device, base: &Placed) -> Placed {
    let ram = base.bram_sites[0];
    let mut p = base.clone();
    // Every cluster gathers around the RAM tile, one per BLE of its
    // neighbours, so the centroids fall on it.
    let around = [(ram.x - 1, ram.y), (ram.x + 1, ram.y), (ram.x, ram.y + 1), (ram.x, ram.y.saturating_sub(1).max(dev.clb_y0))];
    let mut k = 0;
    for &(x, y) in &around {
        if (x, y) == (ram.x, ram.y) {
            continue;
        }
        for b in 0..dev.n_ble as u8 {
            if k < p.lutff_sites.len() {
                p.lutff_sites[k] = (Site { x, y, kind: helion_device::SiteKind::Clb }, b);
                k += 1;
            }
        }
    }
    // The rest far away, so moving toward the middle pays.
    for i in k..p.lutff_sites.len() {
        p.lutff_sites[i] = (Site { x: ram.x + 12 + (i as u32 % 8), y: ram.y + 12, kind: helion_device::SiteKind::Clb }, (i / 8 % 8) as u8);
    }
    distinct(&p);
    p
}

#[test]
fn phys_opt() {
    let (dev, base) = the_main_placement();
    for p in [pulled_toward_the_ram(&dev, &base), squatted(&base, &dev)] {
        let (out, _) = phys_opt_place_with_stats(&p, &dev).unwrap();
        assert!(on_block_tiles(&out, &dev).is_empty(), "{:?}", on_block_tiles(&out, &dev));
        distinct(&out);
        let out = phys_opt_place(&p, &dev).unwrap();
        assert!(on_block_tiles(&out, &dev).is_empty());
    }
}

#[test]
fn detailed_placer() {
    let (dev, base) = the_main_placement();
    let opts = DetailedPlaceOpts { swap_radius: 4, ..Default::default() };
    for p in [pulled_toward_the_ram(&dev, &base), squatted(&base, &dev)] {
        let (out, _) = detailed_place_optimize_with_stats(&p, &dev, &opts).unwrap();
        assert!(on_block_tiles(&out, &dev).is_empty(), "{:?}", on_block_tiles(&out, &dev));
        distinct(&out);
        let out = detailed_place_optimize(&p, &dev, &opts).unwrap();
        assert!(on_block_tiles(&out, &dev).is_empty());
    }
}

/// The analytical placer fills row 1 from the left; with every DSP site
/// taken, the multipliers' tiles start at (8, 1).
#[test]
fn analytical_placer() {
    let dev = Device::load_part("HL10S-C64-1").unwrap();
    let packed = pack(&design_with(100, 3), &dev).unwrap();
    assert!(place(&packed, &dev).unwrap().reserved_tiles(&dev).contains(&(dev.dsp_x, dev.clb_y0)));
    for opts in [AnalyticalPlaceOpts::default(), AnalyticalPlaceOpts { timing_weight: 1.0, ..Default::default() }] {
        let (p, _) = place_analytical_with_stats(&packed, &dev, &opts).unwrap();
        assert!(on_block_tiles(&p, &dev).is_empty(), "{:?}", on_block_tiles(&p, &dev));
        distinct(&p);
    }
    let p = place_analytical(&packed, &dev).unwrap();
    assert!(on_block_tiles(&p, &dev).is_empty());
}

/// The macro-column legalizer keeps the connected blocks where `place`
/// put them (it used to snap them to the sites nearest the first
/// cluster's row, onto clusters), and moves clusters off their tiles.
#[test]
fn macro_column_legalizer() {
    let (dev, base) = the_main_placement();
    for p in [base.clone(), squatted(&base, &dev)] {
        let (out, stats) = legalize_macro_columns(&p, &dev, None).unwrap();
        assert_eq!((out.mac_sites.clone(), out.bram_sites.clone()), (base.mac_sites.clone(), base.bram_sites.clone()));
        assert_eq!((stats.dsp_macros_legalized, stats.bram_macros_legalized), (1, 1));
        assert!(on_block_tiles(&out, &dev).is_empty(), "{:?}", on_block_tiles(&out, &dev));
        distinct(&out);
    }
    // A connected multiplier on a RAM site goes to the nearest DSP site
    // with both tiles free of other blocks.
    let mut p = base.clone();
    p.mac_sites[0] = dev.bram_sites().last().unwrap();
    let (out, _) = legalize_macro_columns(&p, &dev, None).unwrap();
    assert!(on_block_tiles(&out, &dev).is_empty());
    // A name-only RAM beside them takes the first free site in the snap
    // order, never the connected RAM's.
    let mut d = design();
    d.add_cell("u_ila", CellKind::Bram18);
    let packed = pack(&d, &dev).unwrap();
    let p = place(&packed, &dev).unwrap();
    let (out, _) = legalize_macro_columns(&p, &dev, None).unwrap();
    assert_eq!(out.bram_sites.len(), 2);
    assert_ne!(out.bram_sites[0], out.bram_sites[1]);
    assert_eq!(out.bram_sites[0], p.bram_sites[0]);
}


/// [`design`] and a second clock domain: a counter on `clk2` whose flops
/// would fit on the blocks' tiles as well as the first domain's.
fn two_clock_design() -> Design {
    let mut n = Nl { d: design(), k: 10_000 };
    n.d.add_port("clk2", helion_ir::PortDir::In);
    let q: Vec<String> = (0..24).map(|_| n.fresh("cnt2_")).collect();
    for i in 0..24 {
        let next = if i == 0 { n.lut(&[&q[0]], |x| x & 1 == 0) } else { n.lut(&[&q[i], &q[i - 1]], |x| (x & 1) ^ (x >> 1) == 1) };
        n.ff_on("clk2", &next, &q[i]);
    }
    for i in 0..4 {
        n.output(&format!("c2_{i}"), &q[20 + i]);
    }
    n.d
}

/// CLBs holding flops of more than one clock line (bitgen refuses them).
fn mixed_clock_clbs(p: &Placed) -> Vec<(u32, u32)> {
    let mut lines: std::collections::HashMap<(u32, u32), HashSet<u8>> = Default::default();
    for (i, (s, _)) in p.lutff_sites.iter().enumerate() {
        if let Some(Some((l, _))) = p.packed.conn.ff_clock.get(i) {
            lines.entry((s.x, s.y)).or_default().insert(*l);
        }
    }
    let mut v: Vec<(u32, u32)> = lines.into_iter().filter(|(_, l)| l.len() > 1).map(|(t, _)| t).collect();
    v.sort();
    v
}

/// Moving clusters off the blocks' tiles puts them only where the CLB
/// keeps one clock: the main placement with flops of one clock line on
/// each block tile (alternating lines from tile to tile, several choices
/// of flops) goes through each placer that does only that besides its
/// own work. (phys_opt and the detailed placer swap clusters without
/// regard to clocks, blocks or not; they are not checked here.)
#[test]
fn moving_off_the_blocks_keeps_one_clock_per_clb() {
    let dev = Device::load_part("HL10S-C64-1").unwrap();
    let packed = pack(&two_clock_design(), &dev).unwrap();
    let base = place(&packed, &dev).unwrap();
    assert!(on_block_tiles(&base, &dev).is_empty());
    assert!(mixed_clock_clbs(&base).is_empty());
    let lines: HashSet<u8> = packed.conn.ff_clock.iter().flatten().map(|c| c.0).collect();
    assert_eq!(lines.len(), 2);
    let reserved = base.reserved_tiles(&dev);
    let mut tiles: Vec<(u32, u32)> = reserved.iter().copied().collect();
    tiles.sort();
    let line = |k: usize| packed.conn.ff_clock.get(k).copied().flatten().map(|c| c.0);
    let clb = |t: (u32, u32)| Site { x: t.0, y: t.1, kind: helion_device::SiteKind::Clb };
    let n_ble = dev.n_ble as usize;
    for off in 0..2u8 {
        for step in [1usize, 3] {
            // Flops of line `off` on the blocks' tiles; one flop of the
            // other line on BLE 0 of each tile beside them (the nearest
            // free slots); everything else far away, a tile per clock.
            let mut sq = base.clone();
            let mut placed_k: HashSet<usize> = HashSet::new();
            let of_line = |l: u8, n: usize, step: usize, placed_k: &mut HashSet<usize>| -> Vec<usize> {
                let v: Vec<usize> = (0..sq.lutff_sites.len()).filter(|&k| !placed_k.contains(&k) && line(k) == Some(l)).step_by(step).take(n).collect();
                placed_k.extend(v.iter().copied());
                v
            };
            let mut moves: Vec<(usize, (u32, u32), u8)> = Vec::new();
            for t in &tiles {
                for (b, k) in of_line(off, 4, step, &mut placed_k).into_iter().enumerate() {
                    moves.push((k, *t, b as u8));
                }
            }
            let mut beside: Vec<(u32, u32)> = dev.clb_sites().map(|c| (c.x, c.y)).filter(|c| !reserved.contains(c) && tiles.iter().any(|t| t.0.abs_diff(c.0) + t.1.abs_diff(c.1) == 1)).collect();
            beside.sort();
            beside.dedup();
            let others = of_line(1 - off, beside.len(), 1, &mut placed_k);
            assert_eq!(others.len(), beside.len());
            for (k, t) in others.into_iter().zip(&beside) {
                moves.push((k, *t, 0));
            }
            let mut far: Vec<(u32, u32)> = dev.clb_sites().map(|c| (c.x, c.y)).filter(|c| !reserved.contains(c) && tiles.iter().all(|t| t.0.abs_diff(c.0) + t.1.abs_diff(c.1) > 3)).collect();
            far.sort_by_key(|c| std::cmp::Reverse(c.0 + c.1));
            let mut far = far.into_iter();
            for group in [Some(0u8), Some(1), None] {
                let rest: Vec<usize> = (0..sq.lutff_sites.len()).filter(|&k| !placed_k.contains(&k) && line(k) == group).collect();
                for chunk in rest.chunks(n_ble) {
                    let t = far.next().expect("room");
                    for (b, &k) in chunk.iter().enumerate() {
                        moves.push((k, t, b as u8));
                    }
                }
            }
            for (k, t, b) in moves {
                sq.lutff_sites[k] = (clb(t), b);
            }
            distinct(&sq);
            let case = format!("off {off} step {step}");
            assert!(!on_block_tiles(&sq, &dev).is_empty(), "{case}");
            assert!(mixed_clock_clbs(&sq).is_empty(), "{case}");
            let mut outs: Vec<(&str, Placed)> = Vec::new();
            let mut e = sq.clone();
            evict_from_hard_tiles(&mut e, &dev).unwrap();
            outs.push(("evict_from_hard_tiles", e));
            outs.push(("place_congestion_spread", place_congestion_spread(&sq, &dev, 8).unwrap().0));
            outs.push(("legalize_macro_columns", legalize_macro_columns(&sq, &dev, None).unwrap().0));
            outs.push(("place_incremental", place_incremental(&packed, &dev, &sq, PlaceOpts::default()).unwrap().0));
            for (name, out) in &outs {
                assert!(on_block_tiles(out, &dev).is_empty(), "{name} {case}");
                assert!(mixed_clock_clbs(out).is_empty(), "{name} {case}: {:?}", mixed_clock_clbs(out));
                distinct(out);
            }
        }
    }
}

/// `cell_bels` answers `cell_bel` for every cell, with RAM, MAC, IOB and
/// LUT/FF clusters all placed.
#[test]
fn cell_bels_matches_cell_bel() {
    let (_, p) = the_main_placement();
    let bels = p.cell_bels();
    let mut names: Vec<&str> = p.packed.lutffs.iter().flat_map(|l| [l.lut_cell.as_str(), l.ff_cell.as_str()]).collect();
    names.extend(p.packed.iobs.iter().map(|c| c.cell.as_str()));
    names.extend(p.packed.macs.iter().map(|c| c.cell.as_str()));
    names.extend(p.packed.brams.iter().map(|c| c.cell.as_str()));
    names.push("no_such_cell");
    assert!(!p.packed.brams.is_empty() && !p.packed.iobs.is_empty());
    for n in names.into_iter().filter(|n| !n.is_empty()) {
        assert_eq!(bels.get(n).cloned().flatten(), p.cell_bel(n), "{n}");
    }
}
