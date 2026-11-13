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
                // A hard block does not move: its site is a fixed point.
                NetSrc::Hard { blk, .. } => net.fixed.extend(placed.hard_block_site(blk).map(|s| (s.x as i32, s.y as i32))),
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
    for h in &conn.hard_pins {
        if let Some(s) = placed.hard_block_site(h.blk) {
            let k = get(h.src, &mut nets);
            nets[k].fixed.push((s.x as i32, s.y as i32));
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
pub(crate) struct ClockInfo {
    line: Vec<i8>,
    source: Vec<bool>,
    /// Asynchronous reset of the cluster's flop: line * 2 + level, or -1.
    rst: Vec<i16>,
}

impl ClockInfo {
    /// `None` when the design has one clock from a pin: nothing to respect.
    fn of(placed: &Placed) -> Option<Self> {
        Self::of_conn(&placed.packed.conn, placed.lutff_sites.len())
    }

    /// [`ClockInfo::of`] for `n` clusters of connectivity `conn`.
    pub(crate) fn of_conn(conn: &helion_pack::Connectivity, n: usize) -> Option<Self> {
        let any_gen = conn.clocks.iter().any(|c| matches!(c.src, helion_pack::ClockSrc::Gen(_)));
        let lines: std::collections::HashSet<u8> = conn.ff_clock.iter().flatten().map(|f| f.0).collect();
        let any_rst = conn.ff_reset.iter().any(|r| r.is_some());
        if conn.clocks.len() <= 1 && !any_gen && lines.len() <= 1 && !any_rst {
            return None;
        }
        let mut line = vec![-1i8; n];
        for (i, c) in conn.ff_clock.iter().enumerate().take(n) {
            if let Some((l, _)) = c {
                line[i] = *l as i8;
            }
        }
        let mut source = vec![false; n];
        for c in conn.clocks.iter().chain(conn.resets.iter()) {
            if let helion_pack::ClockSrc::Gen(helion_pack::NetSrc::BleQ(i) | helion_pack::NetSrc::BleO(i)) = c.src {
                if (i as usize) < n {
                    source[i as usize] = true;
                }
            }
        }
        let mut rst = vec![-1i16; n];
        for (i, r) in conn.ff_reset.iter().enumerate().take(n) {
            if let Some((l, level, _)) = r {
                rst[i] = *l as i16 * 2 + *level as i16;
            }
        }
        Some(ClockInfo { line, source, rst })
    }

    /// Whether the clusters in `occ` (one CLB) may share it: flops of one
    /// clock line, and at most one clock source.
    pub(crate) fn tile_ok(&self, occ: &[u32]) -> bool {
        let mut seen: i8 = -1;
        let mut seen_rst: i16 = -1;
        let mut gens = 0;
        for &c in occ {
            if c == u32::MAX {
                continue;
            }
            let r = self.rst[c as usize];
            if r >= 0 {
                if seen_rst >= 0 && seen_rst != r {
                    return false;
                }
                seen_rst = r;
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
    let reserved = placed.reserved_tiles(dev);
    let sites: Vec<Site> = dev.clb_sites().filter(|s| !reserved.contains(&(s.x, s.y))).collect();
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
            let first_rst = tile.iter().filter(|c| **c != u32::MAX).map(|c| ck.rst[*c as usize]).find(|r| *r >= 0).unwrap_or(-1);
            let off_reset = (0..n_ble).find(|&b| tile[b] != u32::MAX && ck.rst[tile[b] as usize] >= 0 && ck.rst[tile[b] as usize] != first_rst);
            let slot = t * n_ble + off_clock.or(extra_gen).or(off_reset).ok_or("cannot separate the clocks of a CLB")?;
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

/// Timing-driven passes after the wirelength pass.
const TIMING_PASSES: usize = 2;

/// A connection the timing passes weigh. Nodes `0..n` are the clusters,
/// `n..` the hard blocks connected to the fabric (fixed while annealing).
#[derive(Clone, Copy, Debug)]
struct TConn {
    s: u32,
    d: u32,
    /// Arrival at the source when it is a register (a flop's Q, a RAM's DO,
    /// a registered multiplier's P); `None`: the source node's arrival.
    start: Option<f32>,
    /// Tile rows above the node's position the source leaves from and the
    /// destination pin is on (a multiplier takes two tiles).
    sdy: i32,
    ddy: i32,
    /// Loading at the destination pin (a LUT's per-pin delay; 0 at a block).
    pin: f32,
    /// When the destination is a block input a clock edge latches: what
    /// comes between the pin and the register's setup (the product for a
    /// registered multiplier's A/B/C, else 0).
    end: Option<f32>,
}

/// Weight of each connection from a timing estimate of the placement:
/// delay per tile of distance, a LUT per level, a combinational
/// multiplier's product as an arc, a RAM's or registered multiplier's
/// inputs as endpoints and their outputs as path starts (the same model
/// as `helion_sta::graph`, see [`helion_pack::HardTiming`]). A connection
/// on the slowest path gets the full weight, one with slack to spare gets
/// none. `pos` covers every node; `has_ff` the clusters (`0..n`).
fn criticality_weights(conns: &[TConn], pos: &[(i32, i32)], has_ff: &[bool], n: usize) -> Vec<f32> {
    let tm = helion_backend::TimingModel::hl10t();
    let (lut, hop, ckq, setup, mac) = (tm.lut_ps as f32, tm.hop_ps as f32, tm.ff_ckq_ps as f32, tm.setup_ps as f32, tm.mac_ps as f32);
    let nodes = pos.len();
    let delay = |k: usize| -> f32 {
        let c = conns[k];
        let (a, b) = (pos[c.s as usize], pos[c.d as usize]);
        ((a.0 - b.0).abs() + ((a.1 + c.sdy) - (b.1 + c.ddy)).abs()) as f32 * hop + c.pin
    };
    // Delay of a node from its inputs to its output: a LUT, or a
    // combinational multiplier's product.
    let through = |i: usize| if i < n { lut } else { mac };
    // Fan-in connections of each node (block inputs that end a path are
    // not fan-in), and a topological order over the connections whose
    // source is a node (registers start paths).
    let mut fanin: Vec<Vec<u32>> = vec![Vec::new(); nodes];
    for (k, c) in conns.iter().enumerate() {
        if c.end.is_none() {
            fanin[c.d as usize].push(k as u32);
        }
    }
    let mut order: Vec<u32> = Vec::with_capacity(nodes);
    let mut state = vec![0u8; nodes];
    for root in 0..nodes {
        if state[root] != 0 {
            continue;
        }
        let mut stack = vec![root as u32];
        while let Some(&i) = stack.last() {
            let iu = i as usize;
            if state[iu] == 2 {
                stack.pop();
                continue;
            }
            state[iu] = 1;
            let mut ready = true;
            for &k in &fanin[iu] {
                let c = conns[k as usize];
                if c.start.is_none() && state[c.s as usize] == 0 {
                    stack.push(c.s);
                    ready = false;
                }
            }
            if ready {
                state[iu] = 2;
                order.push(i);
                stack.pop();
            }
        }
    }
    // Arrival at each node's output.
    let mut arr = vec![0.0f32; nodes];
    let src_at = |c: &TConn, arr: &[f32]| c.start.unwrap_or(arr[c.s as usize]);
    for &i in &order {
        let iu = i as usize;
        let mut a = if iu < n { ckq } else { 0.0 };
        for &k in &fanin[iu] {
            a = a.max(src_at(&conns[k as usize], &arr) + delay(k as usize));
        }
        arr[iu] = a + through(iu);
    }
    let mut worst = (0..n).map(|i| arr[i] + if has_ff[i] { setup } else { 0.0 }).fold(1.0f32, f32::max);
    for (k, c) in conns.iter().enumerate() {
        if let Some(e) = c.end {
            worst = worst.max(src_at(c, &arr) + delay(k) + e + setup);
        }
    }
    // Required time at each node's output: the slowest path has no slack.
    let mut req = vec![worst; nodes];
    for i in 0..n {
        if has_ff[i] {
            req[i] = worst - setup;
        }
    }
    for (k, c) in conns.iter().enumerate() {
        if let (Some(e), None) = (c.end, c.start) {
            let r = worst - setup - e - delay(k);
            if r < req[c.s as usize] {
                req[c.s as usize] = r;
            }
        }
    }
    for &i in order.iter().rev() {
        let iu = i as usize;
        for &k in &fanin[iu] {
            let c = conns[k as usize];
            if c.start.is_none() {
                let r = req[iu] - through(iu) - delay(k as usize);
                if r < req[c.s as usize] {
                    req[c.s as usize] = r;
                }
            }
        }
    }
    conns
        .iter()
        .enumerate()
        .map(|(k, c)| {
            let src = src_at(c, &arr);
            let slack = match c.end {
                Some(e) => worst - setup - e - delay(k) - src,
                None => req[c.d as usize] - through(c.d as usize) - delay(k) - src,
            };
            let crit = (1.0 - slack / worst).clamp(0.0, 1.0);
            // Steep: only connections near the slowest path are pulled.
            4.0 * crit.powi(8)
        })
        .collect()
}

/// The connections the timing passes weigh (see [`TConn`]): LUT inputs
/// from clusters and from hard blocks, and hard-block inputs from
/// clusters and other blocks. Input pads are left out (they do not move).
fn timing_conns(placed: &Placed, n: usize) -> Vec<TConn> {
    use helion_pack::{HardTiming, NetSrc};
    let tm = helion_backend::TimingModel::hl10t();
    let (ckq, pin) = (tm.ff_ckq_ps as f32, tm.pin_ps as f32);
    let conn = &placed.packed.conn;
    let nh = conn.hard.len();
    let placed_blk = |b: usize| b < nh && placed.hard_block_site(b as u32).is_some();
    // (node, start arrival, rows above the node's position) of a source.
    let source = |src: NetSrc| -> Option<(u32, Option<f32>, i32)> {
        match src {
            NetSrc::BleQ(j) if (j as usize) < n => Some((j, Some(ckq), 0)),
            NetSrc::BleO(j) if (j as usize) < n => Some((j, None, 0)),
            NetSrc::Hard { blk, bit } if placed_blk(blk as usize) => {
                let start = match placed.packed.hard_timing(blk as usize) {
                    HardTiming::Ram => Some(tm.bram_ckq_ps as f32),
                    HardTiming::Reg => Some(ckq),
                    HardTiming::Comb => None,
                };
                let dy = helion_device::hard::output(conn.hard[blk as usize].kind, bit as u32).map(|o| o.0).unwrap_or(0);
                Some((n as u32 + blk, start, dy as i32))
            }
            _ => None,
        }
    };
    let mut out: Vec<TConn> = Vec::new();
    for p in &conn.pins {
        if (p.lutff as usize) >= n {
            continue;
        }
        if let Some((s, start, sdy)) = source(p.src) {
            out.push(TConn { s, d: p.lutff, start, sdy, ddy: 0, pin, end: None });
        }
    }
    for h in &conn.hard_pins {
        let b = h.blk as usize;
        if !placed_blk(b) {
            continue;
        }
        let Some((s, start, sdy)) = source(h.src) else { continue };
        let product = conn.hard[b].kind == helion_pack::HardKind::Mac && helion_pack::mac_through_product(h.dy, h.m);
        let end = match placed.packed.hard_timing(b) {
            // CE and RST of a combinational multiplier time nothing.
            HardTiming::Comb if !product => continue,
            HardTiming::Comb => None,
            HardTiming::Reg if product => Some(tm.mac_ps as f32),
            HardTiming::Reg | HardTiming::Ram => Some(0.0),
        };
        out.push(TConn { s, d: (n + b) as u32, start, sdy, ddy: h.dy as i32, pin: 0.0, end });
    }
    out
}

/// Anneal the placement of `placed` in place. Returns (before, after) HPWL.
pub fn refine(placed: &mut Placed, dev: &Device) -> (u64, u64) {
    let n = placed.lutff_sites.len();
    let (nets, of_cell) = build_nets(placed);
    let reserved = placed.reserved_tiles(dev);
    let sites: Vec<Site> = dev.clb_sites().filter(|s| !reserved.contains(&(s.x, s.y))).collect();
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
    let site_xy = |t: u32| (sites[t as usize].x as i32, sites[t as usize].y as i32);
    // Connections for the timing passes (clusters and hard blocks).
    let tconns: Vec<TConn> = timing_conns(placed, n);
    // Positions of every node: the clusters (moved below), then the hard
    // blocks (fixed).
    let nh = placed.packed.conn.hard.len();
    for b in 0..nh {
        let s = placed.hard_block_site(b as u32).map(|s| (s.x as i32, s.y as i32)).unwrap_or((0, 0));
        pos.push(s);
    }
    let mut conn_of: Vec<Vec<u32>> = vec![Vec::new(); n + nh];
    for (k, c) in tconns.iter().enumerate() {
        conn_of[c.s as usize].push(k as u32);
        if c.s != c.d {
            conn_of[c.d as usize].push(k as u32);
        }
    }
    let dist = |k: usize, pos: &[(i32, i32)]| -> i32 {
        let c = tconns[k];
        let (a, b) = (pos[c.s as usize], pos[c.d as usize]);
        (a.0 - b.0).abs() + ((a.1 + c.sdy) - (b.1 + c.ddy)).abs()
    };
    let mut cw: Vec<f32> = vec![0.0; tconns.len()];
    let has_ff: Vec<bool> = placed.packed.lutffs.iter().take(n).map(|l| !l.ff_cell.is_empty()).collect();
    // Pass 0 minimises wirelength; the later passes also pull together the
    // connections that the timing estimate finds on slow paths.
    for pass in 0..=TIMING_PASSES {
    let mut window = if pass == 0 { gw.max(gh) as f64 } else { 6.0 };
    // Start warm enough to accept most single-tile moves, not hot enough to
    // undo what the constructive placer already got right.
    let mut temp = if pass == 0 { 2.0f64 } else { 0.15 };
    if pass > 0 {
        if tconns.is_empty() {
            break;
        }
        cw = criticality_weights(&tconns, &pos, &has_ff, n);
    }
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
            let mut delta = delta as f64;
            if pass > 0 {
                // Weighted length of the connections of the moved clusters.
                let (old_a, old_b) = (pa, pb);
                let mut before = 0.0f64;
                let mut after = 0.0f64;
                let movers: [u32; 2] = [a as u32, b];
                for (mi, &c) in movers.iter().enumerate() {
                    if c == u32::MAX {
                        continue;
                    }
                    for &k in &conn_of[c as usize] {
                        let (s, d) = (tconns[k as usize].s, tconns[k as usize].d);
                        // A connection between the two movers is counted once.
                        if mi == 1 && (s == a as u32 || d == a as u32) {
                            continue;
                        }
                        let w = cw[k as usize] as f64;
                        if w == 0.0 {
                            continue;
                        }
                        after += w * dist(k as usize, &pos) as f64;
                        let at = |c: u32| -> (i32, i32) {
                            if c == a as u32 {
                                old_a
                            } else if c == b {
                                old_b
                            } else {
                                pos[c as usize]
                            }
                        };
                        let (p, q) = (at(s), at(d));
                        let (sdy, ddy) = (tconns[k as usize].sdy, tconns[k as usize].ddy);
                        before += w * ((p.0 - q.0).abs() + ((p.1 + sdy) - (q.1 + ddy)).abs()) as f64;
                    }
                }
                delta += after - before;
            }
            if delta <= 0.0 || rng.unit() < (-delta / temp).exp() {
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
    }
    for c in 0..n {
        let (t, ble) = (slot[c] / n_ble as u32, slot[c] % n_ble as u32);
        placed.lutff_sites[c] = (sites[t as usize], ble as u8);
    }
    let after: u64 = len.iter().map(|&l| l as u64).sum();
    (before, after)
}
