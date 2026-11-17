//! Placement refinement: simulated annealing over whole slices.
//!
//! The greedy placer ([`crate::place::place`]) produces a legal placement
//! (carry columns, control sets, LUT/FF pairing). This pass keeps every
//! slice's contents together and moves slices between sites to shorten the
//! nets, measured as half-perimeter wirelength. Carry slices stay put (their
//! cascade needs the column), as do pads and clock buffers.
//!
//! The random sequence is fixed, so a design always places the same way.

use crate::place::{DeviceQuery, Placed7};
use std::collections::{HashMap, HashSet};

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

struct Net {
    /// Bounding box of the pins that cannot move: (xmin, xmax, ymin, ymax).
    fixed: Option<(i32, i32, i32, i32)>,
    units: Vec<u32>,
    /// Cost per unit of half-perimeter.
    weight: i64,
}

/// Weight of a net between slices against one that reaches a pad. Pad paths
/// are not timed against the clock, so logic stays clustered and the pads
/// only decide where the cluster sits.
const INTERNAL_WEIGHT: i64 = 8;

/// Nets above this many pins are left out of the cost (resets, enables):
/// they reach everywhere whatever the placement, and cost time per move.
const MAX_PINS: usize = 96;

pub fn refine(out: &mut Placed7, device: &dyn DeviceQuery) {
    let sites = device.all_slices();
    if sites.is_empty() {
        return;
    }
    let site_idx: HashMap<&str, usize> = sites.iter().enumerate().map(|(i, s)| (s.1.as_str(), i)).collect();
    let carry_sites: HashSet<&str> = out.carries.iter().map(|c| c.site.as_str()).collect();

    // Movable units: slices holding LUTs/FFs but no carry.
    let mut unit_of_site: HashMap<usize, u32> = HashMap::new();
    let mut pos: Vec<usize> = Vec::new();
    let mut cell_sites: Vec<&str> = out.luts.iter().map(|l| l.site.as_str()).chain(out.ffs.iter().map(|f| f.site.as_str())).chain(out.lutrams.iter().map(|r| r.site.as_str())).collect();
    cell_sites.sort();
    cell_sites.dedup();
    for s in cell_sites {
        if carry_sites.contains(s) {
            continue;
        }
        let Some(&si) = site_idx.get(s) else { return };
        unit_of_site.insert(si, pos.len() as u32);
        pos.push(si);
    }
    let n = pos.len();
    if n < 2 {
        return;
    }
    // LUT RAM slices move between SLICEM sites only.
    let slicem = device.slicem_sites();
    let is_m: Vec<bool> = sites.iter().map(|s| slicem.contains(&s.1)).collect();
    let lr_sites: HashSet<&str> = out.lutrams.iter().map(|r| r.site.as_str()).collect();
    let mut need_m = vec![false; n];
    for (&si, &u) in &unit_of_site {
        need_m[u as usize] = lr_sites.contains(sites[si].1.as_str());
    }

    // Pins per net.
    enum Pin {
        Unit(u32),
        Fixed(i32, i32),
        Pad(i32, i32),
    }
    let locate = |site: &str| -> Option<Pin> {
        let si = *site_idx.get(site)?;
        Some(match unit_of_site.get(&si) {
            Some(&u) => Pin::Unit(u),
            None => Pin::Fixed(sites[si].2, sites[si].3),
        })
    };
    let mut pins: HashMap<&str, Vec<Pin>> = HashMap::new();
    let packed = &out.packed;
    let mut skip: HashSet<&str> = HashSet::new();
    for b in &packed.bufgs {
        skip.insert(b.in_net.as_str());
        skip.insert(b.out_net.as_str());
    }
    for f in &packed.ffs {
        skip.insert(f.clk_net.as_str());
    }
    let lut_site: HashMap<&str, &str> = out.luts.iter().map(|l| (l.name.as_str(), l.site.as_str())).collect();
    let ff_site: HashMap<&str, &str> = out.ffs.iter().map(|f| (f.name.as_str(), f.site.as_str())).collect();
    let carry_site: HashMap<&str, &str> = out.carries.iter().map(|c| (c.name.as_str(), c.site.as_str())).collect();
    for l in &packed.luts {
        let Some(site) = lut_site.get(l.name.as_str()) else { continue };
        for net in l.input_nets.iter().flatten().chain(std::iter::once(&l.output_net)) {
            if let Some(p) = locate(site) {
                pins.entry(net.as_str()).or_default().push(p);
            }
        }
    }
    for f in &packed.ffs {
        let Some(site) = ff_site.get(f.name.as_str()) else { continue };
        for net in [Some(&f.d_net), Some(&f.q_net), f.ce_net.as_ref(), f.sr_net.as_ref()].into_iter().flatten() {
            if let Some(p) = locate(site) {
                pins.entry(net.as_str()).or_default().push(p);
            }
        }
    }
    let lr_site: HashMap<&str, &str> = out.lutrams.iter().map(|r| (r.name.as_str(), r.site.as_str())).collect();
    for r in &packed.lutrams {
        let Some(site) = lr_site.get(r.name.as_str()) else { continue };
        for net in r.ins.iter().map(|x| x.1.as_str()).chain(r.outs.iter().map(|o| o.1.as_str())) {
            if let Some(p) = locate(site) {
                pins.entry(net).or_default().push(p);
            }
        }
    }
    for c in &packed.carries {
        let Some(site) = carry_site.get(c.name.as_str()) else { continue };
        let nets = c.o_nets.iter().flatten().chain(c.s_nets.iter().flatten()).chain(c.co_net.iter()).chain(std::iter::once(&c.ci_net));
        for net in nets {
            if let Some(p) = locate(site) {
                pins.entry(net.as_str()).or_default().push(p);
            }
        }
    }
    // Block RAM and DSP pins pull the logic they talk to into their columns.
    let hard = packed
        .brams
        .iter()
        .filter_map(|b| out.brams.iter().find(|p| p.name == b.name).map(|p| (&p.tile, &b.pins)))
        .chain(packed.dsps.iter().filter_map(|d| out.dsps.iter().find(|p| p.name == d.name).map(|p| (&p.tile, &d.pins))));
    for (tile, hpins) in hard {
        let Some((x, y)) = device.tile_xy(tile) else { continue };
        for (_, net, _) in hpins {
            pins.entry(net.as_str()).or_default().push(Pin::Fixed(x, y));
        }
    }
    for (iob, placed) in packed.iobs.iter().zip(&out.iobs) {
        if let Some((x, y)) = device.tile_xy(&placed.tile) {
            pins.entry(iob.net.as_str()).or_default().push(Pin::Pad(x, y));
        }
    }
    let mut names: Vec<&str> = pins.keys().copied().collect();
    names.sort();
    let mut nets: Vec<Net> = Vec::new();
    let mut unit_nets: Vec<Vec<u32>> = vec![Vec::new(); n];
    for name in names {
        if name.is_empty() || skip.contains(name) {
            continue;
        }
        let ps = &pins[name];
        if ps.len() < 2 || ps.len() > MAX_PINS {
            continue;
        }
        // The slice-to-slice part of the net is timed against the clock;
        // the part reaching a pad is not. They are costed separately.
        let grow = |b: Option<(i32, i32, i32, i32)>, x: i32, y: i32| {
            Some(match b {
                None => (x, x, y, y),
                Some((a, bb, c, d)) => (a.min(x), bb.max(x), c.min(y), d.max(y)),
            })
        };
        let mut fixed: Option<(i32, i32, i32, i32)> = None;
        let mut with_pads: Option<(i32, i32, i32, i32)> = None;
        let mut has_pad = false;
        let mut units: Vec<u32> = Vec::new();
        for p in ps {
            match p {
                Pin::Unit(u) => units.push(*u),
                Pin::Fixed(x, y) => {
                    fixed = grow(fixed, *x, *y);
                    with_pads = grow(with_pads, *x, *y);
                }
                Pin::Pad(x, y) => {
                    has_pad = true;
                    with_pads = grow(with_pads, *x, *y);
                }
            }
        }
        units.sort();
        units.dedup();
        if units.is_empty() {
            continue;
        }
        let mut add = |fixed: Option<(i32, i32, i32, i32)>, weight: i64, nets: &mut Vec<Net>| {
            if units.len() == 1 && fixed.is_none() {
                return;
            }
            let ni = nets.len() as u32;
            for &u in &units {
                unit_nets[u as usize].push(ni);
            }
            nets.push(Net { fixed, units: units.clone(), weight });
        };
        add(fixed, INTERNAL_WEIGHT, &mut nets);
        if has_pad {
            add(with_pads, 1, &mut nets);
        }
    }
    if nets.is_empty() {
        return;
    }

    // Site grid in compact column coordinates.
    let mut cols: Vec<i32> = sites.iter().map(|s| s.2).collect();
    cols.sort();
    cols.dedup();
    let col_of: HashMap<i32, usize> = cols.iter().enumerate().map(|(i, x)| (*x, i)).collect();
    let mut grid: HashMap<(usize, i32), Vec<usize>> = HashMap::new();
    for (i, s) in sites.iter().enumerate() {
        grid.entry((col_of[&s.2], s.3)).or_default().push(i);
    }
    // Site occupancy: unit id, FREE, or BLOCKED (carry slices).
    const FREE: u32 = u32::MAX;
    const BLOCKED: u32 = u32::MAX - 1;
    let mut occ: Vec<u32> = vec![FREE; sites.len()];
    for s in &carry_sites {
        if let Some(&si) = site_idx.get(s) {
            occ[si] = BLOCKED;
        }
    }
    for (u, &si) in pos.iter().enumerate() {
        occ[si] = u as u32;
    }

    let xy = |si: usize| (sites[si].2, sites[si].3);
    let net_cost = |net: &Net, pos: &[usize]| -> i64 {
        let (mut x0, mut x1, mut y0, mut y1) = net.fixed.unwrap_or((i32::MAX, i32::MIN, i32::MAX, i32::MIN));
        for &u in &net.units {
            let (x, y) = xy(pos[u as usize]);
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
        ((x1 - x0) as i64 + (y1 - y0) as i64) * net.weight
    };
    let mut cost: Vec<i64> = nets.iter().map(|nt| net_cost(nt, &pos)).collect();
    let mut total: i64 = cost.iter().sum();
    let initial = total;

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let max_rx = cols.len() as i32;
    let (ymin, ymax) = sites.iter().fold((i32::MAX, i32::MIN), |(a, b), s| (a.min(s.3), b.max(s.3)));
    let max_ry = ymax - ymin + 1;
    let mut window = 1.0f64; // fraction of the device a move may span
    let moves_per_t = ((n as f64).powf(4.0 / 3.0) * 8.0) as usize + 200;
    let mut touched: Vec<u32> = Vec::new();
    let mut mark: Vec<u32> = vec![0; nets.len()];
    let mut stamp = 0u32;

    // One move: unit `a` to a random site in the window, swapping with the
    // unit there. Returns the cost change, leaving the move applied.
    let mut try_move = |pos: &mut Vec<usize>, occ: &mut Vec<u32>, cost: &mut Vec<i64>, rng: &mut Rng, window: f64, temp: f64| -> (i64, bool) {
        let a = rng.below(n);
        let from = pos[a];
        let (fx, fy) = xy(from);
        let rx = ((max_rx as f64 * window).ceil() as i32).max(1);
        let ry = ((max_ry as f64 * window).ceil() as i32).max(2);
        let cx = col_of[&fx] as i32 + (rng.below((2 * rx + 1) as usize) as i32 - rx);
        let cy = fy + (rng.below((2 * ry + 1) as usize) as i32 - ry);
        if cx < 0 {
            return (0, false);
        }
        let Some(cands) = grid.get(&(cx as usize, cy)) else { return (0, false) };
        let to = cands[rng.below(cands.len())];
        if to == from || occ[to] == BLOCKED {
            return (0, false);
        }
        let b = occ[to];
        if (need_m[a] && !is_m[to]) || (b != FREE && need_m[b as usize] && !is_m[from]) {
            return (0, false);
        }
        stamp = stamp.wrapping_add(1);
        touched.clear();
        for &ni in &unit_nets[a] {
            if mark[ni as usize] != stamp {
                mark[ni as usize] = stamp;
                touched.push(ni);
            }
        }
        if b != FREE {
            for &ni in &unit_nets[b as usize] {
                if mark[ni as usize] != stamp {
                    mark[ni as usize] = stamp;
                    touched.push(ni);
                }
            }
        }
        pos[a] = to;
        if b != FREE {
            pos[b as usize] = from;
        }
        let mut delta = 0i64;
        for &ni in &touched {
            delta += net_cost(&nets[ni as usize], pos) - cost[ni as usize];
        }
        let accept = delta <= 0 || (temp > 0.0 && rng.unit() < (-(delta as f64) / temp).exp());
        if accept {
            for &ni in &touched {
                cost[ni as usize] = net_cost(&nets[ni as usize], pos);
            }
            occ[to] = a as u32;
            occ[from] = b;
            (delta, true)
        } else {
            pos[a] = from;
            if b != FREE {
                pos[b as usize] = to;
            }
            (0, false)
        }
    };

    // Starting temperature: the typical size of an uphill move.
    let mut temp = {
        let mut sum = 0f64;
        let mut cnt = 0usize;
        for _ in 0..(n * 4).clamp(200, 20_000) {
            let (d, ok) = try_move(&mut pos, &mut occ, &mut cost, &mut rng, window, f64::INFINITY);
            if ok {
                total += d;
                sum += d.abs() as f64;
                cnt += 1;
            }
        }
        if cnt == 0 { 1.0 } else { sum / cnt as f64 }
    };
    let floor = 0.002 * (total.max(1) as f64 / nets.len() as f64).max(1.0);
    while temp > floor {
        let mut accepted = 0usize;
        for _ in 0..moves_per_t {
            let (d, ok) = try_move(&mut pos, &mut occ, &mut cost, &mut rng, window, temp);
            if ok {
                total += d;
                accepted += 1;
            }
        }
        let rate = accepted as f64 / moves_per_t as f64;
        window = (window * (0.56 + rate)).clamp(0.01, 1.0);
        temp *= if rate > 0.96 {
            0.5
        } else if rate > 0.8 {
            0.9
        } else if rate > 0.15 {
            0.95
        } else {
            0.8
        };
    }
    // Greedy finish.
    for _ in 0..moves_per_t {
        let (d, ok) = try_move(&mut pos, &mut occ, &mut cost, &mut rng, 0.02, 0.0);
        if ok {
            total += d;
        }
    }
    if std::env::var("HELION_PLACE_DEBUG").is_ok() {
        eprintln!("place: {n} slices, {} nets, wirelength {initial} -> {total}", nets.len());
    }

    // Write the new sites back.
    let new_site: HashMap<String, (String, String)> = out
        .luts
        .iter()
        .map(|l| l.site.as_str())
        .chain(out.ffs.iter().map(|f| f.site.as_str()))
        .chain(out.lutrams.iter().map(|r| r.site.as_str()))
        .filter_map(|s| {
            let si = *site_idx.get(s)?;
            let u = *unit_of_site.get(&si)?;
            let t = &sites[pos[u as usize]];
            Some((s.to_string(), (t.0.clone(), t.1.clone())))
        })
        .collect();
    for l in out.luts.iter_mut() {
        if let Some((t, s)) = new_site.get(&l.site) {
            l.tile = t.clone();
            l.site = s.clone();
        }
    }
    for f in out.ffs.iter_mut() {
        if let Some((t, s)) = new_site.get(&f.site) {
            f.tile = t.clone();
            f.site = s.clone();
        }
    }
    for r in out.lutrams.iter_mut() {
        if let Some((t, s)) = new_site.get(&r.site) {
            r.tile = t.clone();
            r.site = s.clone();
        }
    }
}
