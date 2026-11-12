//! Wirelength refinement of a legal placement.
//!
//! The constructive placer keeps a LUT near the flops that feed it. A large
//! design also has LUT-to-LUT nets, input nets and pads fed from anywhere,
//! and all of those travel on the general interconnect, whose capacity is
//! the scarce resource. This pass anneals the cluster-to-slot assignment to
//! minimise the half-perimeter wirelength of every net of the packed
//! connection list, so that the router is handed short nets.
//!
//! The result is deterministic (fixed seed) and always a legal placement:
//! clusters only move between BLE slots of CLB sites.

use crate::Placed;
use helion_device::{Device, Site};
use helion_pack::NetSrc;
use std::collections::HashMap;

/// Designs smaller than this are left exactly as the constructive placer
/// made them: they route without help.
pub const MIN_CLUSTERS: usize = 200;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A net: movable cluster pins and fixed points (pads).
struct Net {
    cells: Vec<u32>,
    fixed: Vec<(i32, i32)>,
}

/// Half-perimeter wirelength of the packed connection list (tiles).
pub fn hpwl(placed: &Placed) -> u64 {
    let (nets, _) = build_nets(placed);
    let pos: Vec<(i32, i32)> = placed.lutff_sites.iter().map(|(s, _)| (s.x as i32, s.y as i32)).collect();
    nets.iter().map(|n| net_len(n, &pos) as u64).sum()
}

fn build_nets(placed: &Placed) -> (Vec<Net>, Vec<Vec<u32>>) {
    let conn = &placed.packed.conn;
    let n = placed.lutff_sites.len();
    let mut idx: HashMap<NetSrc, usize> = HashMap::new();
    let mut nets: Vec<Net> = Vec::new();
    let mut get = |src: NetSrc, nets: &mut Vec<Net>| -> usize {
        *idx.entry(src).or_insert_with(|| {
            let mut net = Net { cells: Vec::new(), fixed: Vec::new() };
            match src {
                NetSrc::BleQ(i) | NetSrc::BleO(i) if (i as usize) < n => net.cells.push(i),
                _ => {}
            }
            nets.push(net);
            nets.len() - 1
        })
    };
    for pc in &conn.pins {
        if (pc.lutff as usize) < n {
            let k = get(pc.src, &mut nets);
            nets[k].cells.push(pc.lutff);
        }
    }
    for (ii, io) in placed.packed.iobs.iter().enumerate() {
        let (Some(src), Some(site)) = (conn.iob_src.get(&io.cell), placed.iob_sites.get(ii)) else { continue };
        let k = get(*src, &mut nets);
        nets[k].fixed.push((site.x as i32, site.y as i32));
    }
    let mut of_cell: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (k, net) in nets.iter_mut().enumerate() {
        net.cells.sort_unstable();
        net.cells.dedup();
        for &c in &net.cells {
            of_cell[c as usize].push(k as u32);
        }
    }
    (nets, of_cell)
}

fn net_len(net: &Net, pos: &[(i32, i32)]) -> i32 {
    let mut it = net.cells.iter().map(|&c| pos[c as usize]).chain(net.fixed.iter().copied());
    let Some((x, y)) = it.next() else { return 0 };
    let (mut x0, mut x1, mut y0, mut y1) = (x, x, y, y);
    for (x, y) in it {
        x0 = x0.min(x);
        x1 = x1.max(x);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    (x1 - x0) + (y1 - y0)
}

/// Clocking of each cluster: the line of its flop (if it has one) and
/// whether it is the source of a clock made by logic.
struct ClockInfo {
    line: Vec<i8>,
    source: Vec<bool>,
}

impl ClockInfo {
    /// `None` when the design has one clock from a pin: nothing to respect.
    fn of(placed: &Placed) -> Option<Self> {
        let conn = &placed.packed.conn;
        let any_gen = conn.clocks.iter().any(|c| matches!(c.src, helion_pack::ClockSrc::Gen(_)));
        if conn.clocks.len() <= 1 && !any_gen {
            return None;
        }
        let n = placed.lutff_sites.len();
        let mut line = vec![-1i8; n];
        for (i, c) in conn.ff_clock.iter().enumerate().take(n) {
            if let Some((l, _)) = c {
                line[i] = *l as i8;
            }
        }
        let mut source = vec![false; n];
        for c in &conn.clocks {
            if let helion_pack::ClockSrc::Gen(helion_pack::NetSrc::BleQ(i) | helion_pack::NetSrc::BleO(i)) = c.src {
                if (i as usize) < n {
                    source[i as usize] = true;
                }
            }
        }
        Some(ClockInfo { line, source })
    }

    /// Whether the clusters in `occ` (one CLB) may share it: flops of one
    /// clock line, and at most one clock source.
    fn tile_ok(&self, occ: &[u32]) -> bool {
        let mut seen: i8 = -1;
        let mut gens = 0;
        for &c in occ {
            if c == u32::MAX {
                continue;
            }
            let l = self.line[c as usize];
            if l >= 0 {
                if seen >= 0 && seen != l {
                    return false;
                }
                seen = l;
            }
            gens += self.source[c as usize] as u32;
        }
        gens <= 1
    }
}

/// Make the placement respect the fabric's clocking: every CLB holds flops
/// of one clock line and drives at most one clock line. Clusters that
/// break this move to the nearest CLB where they fit. Returns how many moved.
pub fn legalize_clocks(placed: &mut Placed, dev: &Device) -> Result<u32, String> {
    let Some(ck) = ClockInfo::of(placed) else { return Ok(0) };
    let sites: Vec<Site> = dev.clb_sites().collect();
    let n_ble = dev.n_ble.min(8) as usize;
    let index: HashMap<(u32, u32), usize> = sites.iter().enumerate().map(|(i, s)| ((s.x, s.y), i)).collect();
    let mut occ: Vec<u32> = vec![u32::MAX; sites.len() * n_ble];
    for (c, (s, ble)) in placed.lutff_sites.iter().enumerate() {
        let t = *index.get(&(s.x, s.y)).ok_or("a cluster is placed outside the CLB grid")?;
        occ[t * n_ble + *ble as usize] = c as u32;
    }
    let mut moved = 0;
    for t in 0..sites.len() {
        while !ck.tile_ok(&occ[t * n_ble..(t + 1) * n_ble]) {
            // Evict a flop off the CLB's majority clock, or a second clock source.
            let tile = &occ[t * n_ble..(t + 1) * n_ble];
            let mut counts = [0u32; 8];
            for &c in tile {
                if c != u32::MAX && ck.line[c as usize] >= 0 {
                    counts[ck.line[c as usize] as usize] += 1;
                }
            }
            let keep = (0..8).max_by_key(|&l| counts[l]).unwrap_or(0) as i8;
            let off_clock = (0..n_ble).find(|&b| tile[b] != u32::MAX && ck.line[tile[b] as usize] >= 0 && ck.line[tile[b] as usize] != keep);
            let extra_gen = (0..n_ble).filter(|&b| tile[b] != u32::MAX && ck.source[tile[b] as usize]).nth(1);
            let slot = t * n_ble + off_clock.or(extra_gen).ok_or("cannot separate the clocks of a CLB")?;
            let c = occ[slot];
            let (sx, sy) = (sites[t].x as i64, sites[t].y as i64);
            let mut order: Vec<usize> = (0..sites.len()).filter(|&u| u != t).collect();
            order.sort_by_key(|&u| (sites[u].x as i64 - sx).abs() + (sites[u].y as i64 - sy).abs());
            let mut done = false;
            for u in order {
                let Some(free) = (0..n_ble).find(|b| occ[u * n_ble + b] == u32::MAX) else { continue };
                let mut with = occ[u * n_ble..(u + 1) * n_ble].to_vec();
                with[free] = c;
                if ck.tile_ok(&with) {
                    occ[u * n_ble + free] = c;
                    occ[slot] = u32::MAX;
                    placed.lutff_sites[c as usize] = (sites[u], free as u8);
                    moved += 1;
                    done = true;
                    break;
                }
            }
            if !done {
                return Err("place: no room to keep each CLB on one clock; the design does not fit this part".to_string());
            }
        }
    }
    Ok(moved)
}

/// Anneal the placement of `placed` in place. Returns (before, after) HPWL.
pub fn refine(placed: &mut Placed, dev: &Device) -> (u64, u64) {
    let n = placed.lutff_sites.len();
    let (nets, of_cell) = build_nets(placed);
    let sites: Vec<Site> = dev.clb_sites().collect();
    let n_ble = dev.n_ble.min(8) as usize;
    if n == 0 || nets.is_empty() || sites.is_empty() || n_ble == 0 {
        return (0, 0);
    }
    // Tiles on a dense grid for window moves.
    let (mut gx0, mut gy0, mut gx1, mut gy1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for s in &sites {
        gx0 = gx0.min(s.x as i32);
        gy0 = gy0.min(s.y as i32);
        gx1 = gx1.max(s.x as i32);
        gy1 = gy1.max(s.y as i32);
    }
    let (gw, gh) = ((gx1 - gx0 + 1) as usize, (gy1 - gy0 + 1) as usize);
    let mut tile_at: Vec<u32> = vec![u32::MAX; gw * gh];
    for (i, s) in sites.iter().enumerate() {
        tile_at[(s.y as i32 - gy0) as usize * gw + (s.x as i32 - gx0) as usize] = i as u32;
    }
    // slot = tile * n_ble + ble
    let mut occ: Vec<u32> = vec![u32::MAX; sites.len() * n_ble];
    let mut slot: Vec<u32> = vec![0; n];
    let mut pos: Vec<(i32, i32)> = vec![(0, 0); n];
    for (c, (s, ble)) in placed.lutff_sites.iter().enumerate() {
        let gi = (s.y as i32 - gy0) as usize * gw + (s.x as i32 - gx0) as usize;
        let t = tile_at.get(gi).copied().unwrap_or(u32::MAX);
        if t == u32::MAX || *ble as usize >= n_ble || occ[t as usize * n_ble + *ble as usize] != u32::MAX {
            return (0, 0); // not a placement this pass understands; leave it
        }
        occ[t as usize * n_ble + *ble as usize] = c as u32;
        slot[c] = t * n_ble as u32 + *ble as u32;
        pos[c] = (s.x as i32, s.y as i32);
    }
    let ck = ClockInfo::of(placed);
    let mut len: Vec<i32> = nets.iter().map(|net| net_len(net, &pos)).collect();
    let before: u64 = len.iter().map(|&l| l as u64).sum();

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut touched: Vec<u32> = Vec::new();
    let mut stamp: Vec<u32> = vec![0; nets.len()];
    let mut tick = 0u32;
    let moves_per_temp = (n * 12).max(2000);
    let mut window = gw.max(gh) as f64;
    // Start warm enough to accept most single-tile moves, not hot enough to
    // undo what the constructive placer already got right.
    let mut temp = 2.0f64;
    let site_xy = |t: u32| (sites[t as usize].x as i32, sites[t as usize].y as i32);
    while temp > 0.02 {
        let mut accepted = 0usize;
        for _ in 0..moves_per_temp {
            let a = rng.below(n);
            let (ax, ay) = pos[a];
            let w = window.max(1.0) as i32;
            let tx = (ax - gx0 + rng.below((2 * w + 1) as usize) as i32 - w).clamp(0, gw as i32 - 1);
            let ty = (ay - gy0 + rng.below((2 * w + 1) as usize) as i32 - w).clamp(0, gh as i32 - 1);
            let t = tile_at[ty as usize * gw + tx as usize];
            if t == u32::MAX {
                continue;
            }
            let sb = t * n_ble as u32 + rng.below(n_ble) as u32;
            let sa = slot[a];
            if sa == sb {
                continue;
            }
            let b = occ[sb as usize];
            let (pa, pb) = (pos[a], site_xy(t));
            if pa == pb {
                continue; // same CLB: nothing changes
            }
            // Clocking: both CLBs must still be legal after the swap.
            if let Some(ck) = &ck {
                let (ta, tb) = ((sa as usize / n_ble) * n_ble, (sb as usize / n_ble) * n_ble);
                let involved = (ck.line[a] >= 0 || ck.source[a]) || (b != u32::MAX && (ck.line[b as usize] >= 0 || ck.source[b as usize]));
                if involved {
                    let mut ra = occ[ta..ta + n_ble].to_vec();
                    let mut rb = occ[tb..tb + n_ble].to_vec();
                    ra[sa as usize - ta] = b;
                    rb[sb as usize - tb] = a as u32;
                    if !ck.tile_ok(&ra) || !ck.tile_ok(&rb) {
                        continue;
                    }
                }
            }
            tick += 1;
            touched.clear();
            for &k in &of_cell[a] {
                if stamp[k as usize] != tick {
                    stamp[k as usize] = tick;
                    touched.push(k);
                }
            }
            if b != u32::MAX {
                for &k in &of_cell[b as usize] {
                    if stamp[k as usize] != tick {
                        stamp[k as usize] = tick;
                        touched.push(k);
                    }
                }
            }
            pos[a] = pb;
            if b != u32::MAX {
                pos[b as usize] = pa;
            }
            let mut delta = 0i32;
            for &k in &touched {
                delta += net_len(&nets[k as usize], &pos) - len[k as usize];
            }
            if delta <= 0 || rng.unit() < (-(delta as f64) / temp).exp() {
                for &k in &touched {
                    len[k as usize] = net_len(&nets[k as usize], &pos);
                }
                occ[sb as usize] = a as u32;
                occ[sa as usize] = b;
                slot[a] = sb;
                if b != u32::MAX {
                    slot[b as usize] = sa;
                }
                accepted += 1;
            } else {
                pos[a] = pa;
                if b != u32::MAX {
                    pos[b as usize] = pb;
                }
            }
        }
        // Keep the acceptance rate useful: shrink the window as moves fail.
        let rate = accepted as f64 / moves_per_temp as f64;
        window = (window * (0.56 + rate)).clamp(1.0, gw.max(gh) as f64);
        temp *= 0.9;
    }
    for c in 0..n {
        let (t, ble) = (slot[c] / n_ble as u32, slot[c] % n_ble as u32);
        placed.lutff_sites[c] = (sites[t as usize], ble as u8);
    }
    let after: u64 = len.iter().map(|&l| l as u64).sum();
    (before, after)
}
