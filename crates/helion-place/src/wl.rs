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
