//! Place stage: [`PackedIce`] → [`PlacedIce`].
//!
//! IOs go to their package pins, RAMs to RAM tile pairs, PLLs to the PLL
//! sites. LUT/FF pairs sharing a `D`/`O` net pack into one logic cell;
//! unpaired cells get their own LC. A greedy seed followed by short
//! simulated annealing minimizes bounding-box wirelength.

use crate::device::{all_logic_tiles, ct256_pin_map, is_logic_tile};
use crate::pack::{HardIpKind, IoDir, OscKind, PackedIce, PllKind};
use std::collections::{HashMap, HashSet};

/// A placed LUT4.
#[derive(Clone, Debug)]
pub struct PlacedLut {
    pub name: String,
    pub init: u16,
    pub input_nets: [Option<String>; 4],
    pub output_net: String,
    pub x: u8,
    pub y: u8,
    pub lc: u8,
    /// True when this LC's carry unit is enabled (CarryEnable bit).
    /// `in_3` is then the dedicated carry, not a routable input.
    pub carry: bool,
}

/// A placed flip-flop.
#[derive(Clone, Debug)]
pub struct PlacedFf {
    pub name: String,
    pub d_net: String,
    pub q_net: String,
    pub clk_net: String,
    pub x: u8,
    pub y: u8,
    pub lc: u8,
}

/// A placed SB_IO.
#[derive(Clone, Debug)]
pub struct PlacedIo {
    pub name: String,
    pub port: String,
    pub dir: IoDir,
    pub net: String,
    pub x: u8,
    pub y: u8,
    pub iob: u8,
}

/// A placed SB_RAM40_4K. `(x, y)` is the RAMB tile; the RAMT half is `(x, y+1)`.
#[derive(Clone, Debug)]
pub struct PlacedRam {
    pub name: String,
    pub x: u8,
    pub y: u8,
}

/// A placed SB_PLL40.
#[derive(Clone, Debug)]
pub struct PlacedPll {
    pub name: String,
    pub kind: PllKind,
    pub x: u8,
    pub y: u8,
}

/// A placed `SB_MAC16` 16x16 DSP block.
#[derive(Clone, Debug)]
pub struct PlacedMac16 {
    pub name: String,
    pub x: u8,
    pub y: u8,
}

/// A placed `SB_SPRAM256KA` 256Kb Single-Port RAM block.
#[derive(Clone, Debug)]
pub struct PlacedSpram {
    pub name: String,
    pub x: u8,
    pub y: u8,
}

/// A placed `SB_HFOSC` / `SB_LFOSC` internal oscillator.
#[derive(Clone, Debug)]
pub struct PlacedOsc {
    pub name: String,
    pub kind: OscKind,
    pub out_net: String,
    pub x: u8,
    pub y: u8,
}

/// A placed hard IP block (`SB_I2C`, `SB_SPI`, `SB_RGBA_DRV`, `SB_LEDDA_IP`).
#[derive(Clone, Debug)]
pub struct PlacedHardIp {
    pub name: String,
    pub kind: HardIpKind,
    pub x: u8,
    pub y: u8,
}

/// Carry `carry_in_mux` configuration for one logic tile.
#[derive(Clone, Debug)]
pub struct PlacedCarryMux {
    pub x: u8,
    pub y: u8,
    pub mode: CarryMuxMode,
}

/// How a tile's `carry_in_mux` is driven.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CarryMuxMode {
    /// Constant 1 (`CarryInSet` bit).
    One,
    /// `lutff_7/cout` of the tile below (`carry_in -> carry_in_mux` buffer).
    FromBelow,
}

/// Backend-owned placed netlist for the HX8K / UltraPlus.
#[derive(Clone, Debug, Default)]
pub struct PlacedIce {
    pub packed: PackedIce,
    pub luts: Vec<PlacedLut>,
    pub ffs: Vec<PlacedFf>,
    pub ios: Vec<PlacedIo>,
    pub rams: Vec<PlacedRam>,
    pub plls: Vec<PlacedPll>,
    pub mac16s: Vec<PlacedMac16>,
    pub sprams: Vec<PlacedSpram>,
    pub oscs: Vec<PlacedOsc>,
    pub hard_ips: Vec<PlacedHardIp>,
    /// Tiles whose `carry_in_mux` needs configuration (constant 0 needs no bits).
    pub carry_mux: Vec<PlacedCarryMux>,
}

/// A logic-cell slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct Slot {
    x: u8,
    y: u8,
    lc: u8,
}

/// Tiny deterministic PRNG (xorshift64) so placement is reproducible.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Place packed cells onto device sites.
pub fn place(packed: &PackedIce) -> Result<PlacedIce, String> {
    let mut out = PlacedIce {
        packed: packed.clone(),
        ..Default::default()
    };
    let pin_map = ct256_pin_map();
    let mut used_iobs: HashSet<(u8, u8, u8)> = HashSet::new();

    // 1. IOs to package pins.
    let mut anchor: Option<(u8, u8)> = None;
    for io in &packed.ios {
        let (x, y, iob) = pin_map.get(io.pin.as_str()).copied().ok_or_else(|| {
            format!("place: no IO site for package pin {} (port {})", io.pin, io.port)
        })?;
        if !used_iobs.insert((x, y, iob)) {
            return Err(format!(
                "place: package pin {} (IOB {iob} at {x},{y}) used twice",
                io.pin
            ));
        }
        if anchor.is_none() {
            anchor = Some((x, y));
        }
        out.ios.push(PlacedIo {
            name: io.name.clone(),
            port: io.port.clone(),
            dir: io.dir,
            net: io.net.clone(),
            x,
            y,
            iob,
        });
    }

    // 2. RAMs to RAM tile pairs (RAMB at odd y, RAMT at y+1).
    let mut used_ram: HashSet<(u8, u8)> = HashSet::new();
    for ram in &packed.rams {
        let site = (1..33u8)
            .flat_map(|y| [8u8, 25u8].into_iter().map(move |x| (x, y)))
            .find(|&(x, y)| y % 2 == 1 && !used_ram.contains(&(x, y)))
            .ok_or_else(|| format!("place: no free RAM40 site for {}", ram.name))?;
        used_ram.insert(site);
        out.rams.push(PlacedRam {
            name: ram.name.clone(),
            x: site.0,
            y: site.1,
        });
    }

    // 3. PLLs to the documented 8k PLL sites.
    for pll in &packed.plls {
        let (x, y) = if pll.kind.is_pad() {
            (16u8, 0u8)
        } else {
            (16u8, 33u8)
        };
        out.plls.push(PlacedPll {
            name: pll.name.clone(),
            kind: pll.kind,
            x,
            y,
        });
    }

    // 3a-dsp. Place SB_MAC16 DSP blocks onto the 8 UltraPlus DSP sites.
    const MAC16_SITES: [(u8, u8); 8] = [
        (0, 5),
        (0, 10),
        (0, 15),
        (0, 25),
        (33, 5),
        (33, 10),
        (33, 15),
        (33, 25),
    ];
    if packed.mac16s.len() > MAC16_SITES.len() {
        return Err(format!(
            "place: design has {} SB_MAC16 blocks, exceeding the 8 device DSP sites",
            packed.mac16s.len()
        ));
    }
    for (idx, mac) in packed.mac16s.iter().enumerate() {
        let (x, y) = MAC16_SITES[idx];
        out.mac16s.push(PlacedMac16 {
            name: mac.name.clone(),
            x,
            y,
        });
    }

    // 3a-spram. Place SB_SPRAM256KA blocks onto the 4 UltraPlus 256Kb SPRAM sites.
    const SPRAM_SITES: [(u8, u8); 4] = [(0, 1), (0, 2), (33, 1), (33, 2)];
    if packed.sprams.len() > SPRAM_SITES.len() {
        return Err(format!(
            "place: design has {} SB_SPRAM256KA blocks, exceeding the 4 device SPRAM sites",
            packed.sprams.len()
        ));
    }
    for (idx, spram) in packed.sprams.iter().enumerate() {
        let (x, y) = SPRAM_SITES[idx];
        out.sprams.push(PlacedSpram {
            name: spram.name.clone(),
            x,
            y,
        });
    }

    // 3a-osc. Place SB_HFOSC / SB_LFOSC blocks at dedicated corner oscillator sites.
    for osc in &packed.oscs {
        let (x, y) = match osc.kind {
            OscKind::HfOsc => (0u8, 32u8),
            OscKind::LfOsc => (33u8, 1u8),
        };
        out.oscs.push(PlacedOsc {
            name: osc.name.clone(),
            kind: osc.kind,
            out_net: osc.out_net.clone(),
            x,
            y,
        });
    }

    // 3a-ip. Place hard peripheral IP blocks (SB_I2C, SB_SPI, SB_RGBA_DRV, SB_LEDDA_IP).
    for ip in &packed.hard_ips {
        let (x, y) = match &ip.kind {
            HardIpKind::I2c { bus_addr74 } => {
                if (bus_addr74 & 0b0010) == 0 {
                    (0u8, 31u8)
                } else {
                    (33u8, 31u8)
                }
            }
            HardIpKind::Spi { bus_addr74 } => {
                if (bus_addr74 & 0b0010) == 0 {
                    (0u8, 3u8)
                } else {
                    (33u8, 3u8)
                }
            }
            HardIpKind::RgbaDrv { .. } => (0u8, 30u8),
            HardIpKind::LeddaIp => (33u8, 30u8),
        };
        out.hard_ips.push(PlacedHardIp {
            name: ip.name.clone(),
            kind: ip.kind.clone(),
            x,
            y,
        });
    }
    // Work on a mutable copy: we may insert buffer LUTs and a GND LUT.
    let mut packed = packed.clone();

    // 3b. Every FF's D must come from its own LC's LUT. If the D net is not
    //     driven by a packed LUT, insert a feedthrough buffer LUT.
    {
        let lut_outputs: HashSet<String> =
            packed.luts.iter().map(|l| l.output_net.clone()).collect();
        let mut bufs: Vec<crate::pack::PackedLut> = Vec::new();
        for ff in &mut packed.ffs {
            if !lut_outputs.contains(&ff.d_net) {
                let buf_out = format!("{}$buf", ff.name);
                bufs.push(crate::pack::PackedLut {
                    name: format!("{}$feed", ff.name),
                    init: 0xAAAA, // f(in_0) = in_0
                    input_nets: [Some(ff.d_net.clone()), None, None, None],
                    output_net: buf_out.clone(),
                    carry: None,
                });
                ff.d_net = buf_out;
            }
        }
        packed.luts.extend(bufs);
    }

    // 3c. GND net: a LUT4 with init=0 drives all otherwise-unused LUT inputs.
    //     (Floating iCE40 LUT inputs are not reliably low.)
    let need_gnd = packed.luts.iter().any(|l| l.input_nets.contains(&None));
    if need_gnd {
        packed.luts.push(crate::pack::PackedLut {
            name: "$gnd".to_string(),
            init: 0x0000,
            input_nets: [None, None, None, None],
            output_net: "$gnd".to_string(),
            carry: None,
        });
        for lut in packed.luts.iter_mut() {
            if lut.name == "$gnd" {
                continue;
            }
            for slot in lut.input_nets.iter_mut() {
                if slot.is_none() {
                    *slot = Some("$gnd".to_string());
                }
            }
        }
    }

    // 4. Pair FFs with their driver LUTs (LUT output net == FF D net and the
    //    LUT drives nothing else).
    let mut lut_output_users: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, lut) in packed.luts.iter().enumerate() {
        lut_output_users
            .entry(lut.output_net.clone())
            .or_default()
            .push(i);
    }
    let mut ff_driver_lut: Vec<Option<usize>> = vec![None; packed.ffs.len()];
    let mut lut_paired = vec![false; packed.luts.len()];
    for (fi, ff) in packed.ffs.iter().enumerate() {
        if let Some(users) = lut_output_users.get(&ff.d_net) {
            if users.len() == 1 {
                let li = users[0];
                ff_driver_lut[fi] = Some(li);
                lut_paired[li] = true;
            }
        }
    }

    // 5. Greedy seed: place paired units and singletons near the anchor.
    //    Clock domains get disjoint tile sets: lutff_global/clk is shared
    //    per tile, so FFs on different clocks must not share a tile.
    let (ax, ay) = anchor.unwrap_or((16, 16));
    let tiles = all_logic_tiles();
    let mut ordered: Vec<(u8, u8)> = tiles.clone();
    ordered.sort_by_key(|&(x, y)| {
        (x as i32 - ax as i32).abs() + (y as i32 - ay as i32).abs()
    });
    // Distinct clocks across FFs, in first-seen order.
    let mut clocks: Vec<String> = Vec::new();
    for ff in &packed.ffs {
        if !clocks.contains(&ff.clk_net) {
            clocks.push(ff.clk_net.clone());
        }
    }
    if clocks.is_empty() {
        clocks.push("$noclk".to_string());
    }
    // Partition the ordered tiles round-robin into per-clock chunks.
    let mut tile_clock: HashMap<(u8, u8), usize> = HashMap::new();
    for (i, &t) in ordered.iter().enumerate() {
        tile_clock.insert(t, i % clocks.len());
    }

    // Connectivity for the annealer: net -> list of unit indices.
    // Units: 0..n_lut are LUTs (paired FF rides along), then unpaired FFs.
    struct Unit {
        luts: Vec<usize>, // packed.luts indices in this LC
        ffs: Vec<usize>,  // packed.ffs indices in this LC
        nets: Vec<String>, // all nets touching this unit
    }
    let mut units: Vec<Unit> = Vec::new();
    let mut lut_unit = vec![usize::MAX; packed.luts.len()];
    let mut ff_unit = vec![usize::MAX; packed.ffs.len()];
    for (li, lut) in packed.luts.iter().enumerate() {
        if lut_paired[li] {
            continue; // handled with its FF
        }
        let mut nets: Vec<String> = lut.input_nets.iter().filter_map(|n| n.clone()).collect();
        nets.push(lut.output_net.clone());
        lut_unit[li] = units.len();
        units.push(Unit {
            luts: vec![li],
            ffs: vec![],
            nets,
        });
    }
    for (fi, ff) in packed.ffs.iter().enumerate() {
        let mut nets = vec![ff.d_net.clone(), ff.q_net.clone(), ff.clk_net.clone()];
        let mut luts = vec![];
        if let Some(li) = ff_driver_lut[fi] {
            let lut = &packed.luts[li];
            for n in lut.input_nets.iter().flatten() {
                nets.push(n.clone());
            }
            luts.push(li);
            lut_unit[li] = units.len();
        }
        ff_unit[fi] = units.len();
        units.push(Unit {
            luts,
            ffs: vec![fi],
            nets,
        });
    }
    if units.len() > tiles.len() * 8 {
        return Err(format!(
            "place: design needs {} LCs but HX8K has {}",
            units.len(),
            tiles.len() * 8
        ));
    }

    // Seed placement: units in order into ordered slots, respecting the
    // clock-domain tile partition. Each unit's domain is its FF's clock
    // (combinational-only units join domain 0).
    let mut unit_domain = vec![0usize; units.len()];
    for (ui, u) in units.iter().enumerate() {
        if let Some(&fi) = u.ffs.first() {
            let clk = &packed.ffs[fi].clk_net;
            unit_domain[ui] = clocks.iter().position(|c| c == clk).unwrap_or(0);
        }
    }
    // Per-domain slot cursors over the partitioned tile list.
    let mut domain_tiles: Vec<Vec<(u8, u8)>> = vec![Vec::new(); clocks.len()];
    for &t in &ordered {
        domain_tiles[tile_clock[&t]].push(t);
    }

    // 5b. Carry chains: place each chain's bits in vertically adjacent LCs
    //     of one column (carry flows upward: LC i -> LC i+1, then the tile
    //     above via carry_in_mux). Chains take fresh tiles from the
    //     anchor-ordered list starting at LC0; their slots are locked so the
    //     seeder and annealer leave them alone.
    let mut locked: HashSet<Slot> = HashSet::new();
    let mut carry_slot: HashMap<usize, Slot> = HashMap::new(); // unit_idx -> Slot
    {
        // Group carry LUTs by chain.
        let mut chain_luts: HashMap<usize, Vec<usize>> = HashMap::new();
        for (li, lut) in packed.luts.iter().enumerate() {
            if let Some(c) = &lut.carry {
                chain_luts.entry(c.chain).or_default().push(li);
            }
        }
        let mut chain_ids: Vec<usize> = chain_luts.keys().copied().collect();
        chain_ids.sort();
        for cid in chain_ids {
            let mut lis = chain_luts[&cid].clone();
            lis.sort_by_key(|&li| packed.luts[li].carry.as_ref().map(|c| c.seq).unwrap_or(usize::MAX));
            let nbits = lis.len();
            if nbits == 0 {
                continue;
            }
            // Sanity: seq must be 0..nbits.
            for (s, &li) in lis.iter().enumerate() {
                let seq = packed.luts[li].carry.as_ref().map(|c| c.seq).unwrap_or(usize::MAX);
                if seq != s {
                    return Err(format!(
                        "place: carry chain {cid} has non-contiguous bit sequence"
                    ));
                }
            }
            let chain_info = packed.carry_chains.get(cid).ok_or_else(|| {
                format!("place: carry chain {cid} missing from packed.carry_chains")
            })?;
            // Allocate ceil(nbits/8) VERTICALLY ADJACENT tiles: the carry
            // crosses tiles via carry_in_mux from the tile directly below.
            // Find the anchor-nearest column run of enough logic tiles.
            let tiles_needed = nbits.div_ceil(8);
            let mut start: Option<(u8, u8)> = None;
            for &(x, y) in &ordered {
                if y.checked_add(tiles_needed as u8 - 1).is_none_or(|y1| y1 > 33) {
                    continue;
                }
                let mut ok = true;
                for k in 0..tiles_needed {
                    let yy = y + k as u8;
                    if !is_logic_tile(x, yy) || locked.contains(&Slot { x, y: yy, lc: 0 }) {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    start = Some((x, y));
                    break;
                }
            }
            let (sx, sy) = start.ok_or("place: no vertical tile run for carry chain")?;
            let mut slots: Vec<Slot> = Vec::new();
            for k in 0..tiles_needed {
                let (x, y) = (sx, sy + k as u8);
                if k == 0 {
                    if chain_info.cin_one {
                        out.carry_mux.push(PlacedCarryMux {
                            x,
                            y,
                            mode: CarryMuxMode::One,
                        });
                    }
                    // cin_zero needs no bits (carry_in_mux defaults low).
                } else {
                    out.carry_mux.push(PlacedCarryMux {
                        x,
                        y,
                        mode: CarryMuxMode::FromBelow,
                    });
                }
                for lc in 0..8u8 {
                    if slots.len() < nbits {
                        let s = Slot { x, y, lc };
                        slots.push(s);
                        locked.insert(s);
                    }
                }
            }
            for (&li, &slot) in lis.iter().zip(slots.iter()) {
                let ui = lut_unit[li];
                if ui == usize::MAX {
                    return Err(format!(
                        "place: carry LUT {} is in no unit",
                        packed.luts[li].name
                    ));
                }
                if carry_slot.insert(ui, slot).is_some() {
                    return Err(format!(
                        "place: carry chain {cid} maps two bits to one unit"
                    ));
                }
                locked.insert(slot);
            }
        }
    }

    // Free slots per domain (locked carry slots excluded).
    let mut domain_free: Vec<Vec<Slot>> = vec![Vec::new(); clocks.len()];
    for (d, tiles_d) in domain_tiles.iter().enumerate() {
        for &(x, y) in tiles_d {
            for lc in 0..8u8 {
                let s = Slot { x, y, lc };
                if !locked.contains(&s) {
                    domain_free[d].push(s);
                }
            }
        }
    }
    let mut domain_cursor = vec![0usize; clocks.len()];
    let mut unit_slot: Vec<Slot> = vec![Slot { x: 0, y: 0, lc: 0 }; units.len()];
    for (ui, &d) in unit_domain.iter().enumerate() {
        if let Some(&s) = carry_slot.get(&ui) {
            unit_slot[ui] = s;
        } else {
            let cursor = &mut domain_cursor[d];
            let s = domain_free[d]
                .get(*cursor)
                .copied()
                .ok_or("place: out of LC slots in clock domain")?;
            *cursor += 1;
            unit_slot[ui] = s;
        }
    }

    // Net -> units, for wirelength.
    let mut net_units: HashMap<String, Vec<usize>> = HashMap::new();
    for (ui, u) in units.iter().enumerate() {
        for n in &u.nets {
            net_units.entry(n.clone()).or_default().push(ui);
        }
    }

    // 6. Simulated annealing on bounding-box half-perimeter.
    let cost = |slots: &[Slot]| -> i64 {
        let mut total = 0i64;
        for members in net_units.values() {
            if members.len() < 2 {
                continue;
            }
            let (mut x0, mut x1, mut y0, mut y1) = (99i32, -99i32, 99i32, -99i32);
            for &ui in members {
                let s = slots[ui];
                x0 = x0.min(s.x as i32);
                x1 = x1.max(s.x as i32);
                y0 = y0.min(s.y as i32);
                y1 = y1.max(s.y as i32);
            }
            total += (x1 - x0) as i64 + (y1 - y0) as i64;
        }
        total
    };
    let mut rng = Rng(0x1CE40);
    let mut cur_cost = cost(&unit_slot);
    // Iteration budget scales with design size but stays small.
    let iters = (units.len() * 200).clamp(2000, 60000);
    let mut temp = 4.0f64;
    // Units grouped by domain so swaps never cross clock partitions.
    // Carry-chain units are fixed (their slots are locked); exclude them.
    let mut domain_units: Vec<Vec<usize>> = vec![Vec::new(); clocks.len()];
    for (ui, &d) in unit_domain.iter().enumerate() {
        if carry_slot.contains_key(&ui) {
            continue;
        }
        domain_units[d].push(ui);
    }
    for it in 0..iters {
        let d = rng.below(clocks.len());
        if domain_units[d].len() < 2 {
            continue;
        }
        let a = domain_units[d][rng.below(domain_units[d].len())];
        let b = domain_units[d][rng.below(domain_units[d].len())];
        if a == b {
            continue;
        }
        unit_slot.swap(a, b);
        let new_cost = cost(&unit_slot);
        let delta = new_cost - cur_cost;
        let accept = delta <= 0
            || {
                let p = ((-delta as f64) / temp).exp();
                (rng.next() % 1000) as f64 / 1000.0 < p
            };
        if accept {
            cur_cost = new_cost;
        } else {
            unit_slot.swap(a, b);
        }
        if it % 1000 == 999 && temp > 0.05 {
            temp *= 0.9;
        }
    }

    // 7. Emit placed cells.
    for (ui, u) in units.iter().enumerate() {
        let s = unit_slot[ui];
        debug_assert!(is_logic_tile(s.x, s.y));
        for &li in &u.luts {
            let lut = &packed.luts[li];
            out.luts.push(PlacedLut {
                name: lut.name.clone(),
                init: lut.init,
                input_nets: lut.input_nets.clone(),
                output_net: lut.output_net.clone(),
                x: s.x,
                y: s.y,
                lc: s.lc,
                carry: lut.carry.is_some(),
            });
        }
        for &fi in &u.ffs {
            let ff = &packed.ffs[fi];
            out.ffs.push(PlacedFf {
                name: ff.name.clone(),
                d_net: ff.d_net.clone(),
                q_net: ff.q_net.clone(),
                clk_net: ff.clk_net.clone(),
                x: s.x,
                y: s.y,
                lc: s.lc,
            });
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{PackedFf, PackedIo, PackedLut};

    fn tiny_packed() -> PackedIce {
        PackedIce {
            luts: vec![PackedLut {
                name: "lut0".into(),
                init: 0xAAAA,
                input_nets: [Some("a".into()), None, None, None],
                output_net: "w".into(),
                carry: None,
            }],
            ffs: vec![PackedFf {
                name: "ff0".into(),
                d_net: "w".into(),
                q_net: "q".into(),
                clk_net: "clk".into(),
            }],
            ios: vec![
                PackedIo {
                    name: "ib".into(),
                    port: "clk".into(),
                    dir: IoDir::In,
                    net: "clk".into(),
                    pin: "J3".into(),
                },
                PackedIo {
                    name: "ob".into(),
                    port: "led0".into(),
                    dir: IoDir::Out,
                    net: "q".into(),
                    pin: "B5".into(),
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn pairs_ff_with_driver_lut() {
        let placed = place(&tiny_packed()).unwrap();
        // 2 LUTs: the original plus the $gnd tie-off LUT.
        assert_eq!(placed.luts.len(), 2);
        assert_eq!(placed.ffs.len(), 1);
        let lut0 = placed.luts.iter().find(|l| l.name == "lut0").unwrap();
        // Same LC.
        assert_eq!(
            (lut0.x, lut0.y, lut0.lc),
            (placed.ffs[0].x, placed.ffs[0].y, placed.ffs[0].lc)
        );
        // IOs on the right tiles.
        let ib = placed.ios.iter().find(|i| i.port == "clk").unwrap();
        assert_eq!((ib.x, ib.y, ib.iob), (0, 16, 1));
    }

    #[test]
    fn rejects_unknown_pin() {
        let mut p = tiny_packed();
        p.ios[0].pin = "ZZ99".into();
        assert!(place(&p).is_err());
    }
}
