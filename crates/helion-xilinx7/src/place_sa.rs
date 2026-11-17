//! Placement refinement: simulated annealing over LUT/FF slots.
//!
//! The greedy placer ([`crate::place::place`]) produces a legal placement
//! (carry columns, control sets, LUT/FF pairing), filling slices in netlist
//! order. This pass moves the contents of one BEL letter of a slice (a LUT,
//! an FF, or a LUT with the FF it drives) to another letter anywhere, so
//! unrelated logic the greedy order put side by side can part. Every slice
//! keeps one FF control set. Slices with carry, wide-mux or shift-register
//! cells stay put, as do pads and clock buffers.
//!
//! The cost is half-perimeter wirelength plus, per driver-to-sink
//! connection, its length weighted by how close the longest path through it
//! comes to the critical path. Delays for that come from distance
//! (`wire_ps`) and the SDF cell delays, and are recomputed at every
//! temperature (`HELION_PLACE_NO_TIMING` turns this off).
//!
//! The random sequence is fixed, so a design always places the same way.

use crate::place::{CtrlSet, DeviceQuery, Placed7};
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
    /// Cost per unit of half-perimeter (timing reweights it as it goes).
    weight: std::cell::Cell<i64>,
}

/// Weight of a net between slices against one that reaches a pad. Pad paths
/// are not timed against the clock, so logic stays clustered and the pads
/// only decide where the cluster sits.
const INTERNAL_WEIGHT: i64 = 8;

/// Nets above this many pins are left out of the cost (resets, enables):
/// they reach everywhere whatever the placement, and cost time per move.
const MAX_PINS: usize = 96;

const LETTERS: [char; 4] = ['A', 'B', 'C', 'D'];

/// Weight per tile of a connection: `INTERNAL_WEIGHT * GAIN * crit^EXP`,
/// on top of its net's wirelength.
const TIMING_GAIN: f64 = 16.0;
const TIMING_EXP: i32 = 4;

/// Estimated route delay between two slices `d` tiles apart (Manhattan).
/// shortcut: a straight line in distance, fit to routed delays; a delay
/// lookup by (dx, dy) would track the wire types better.
pub fn wire_ps(d: i32) -> i64 {
    WIRE_BASE_PS + WIRE_TILE_PS * d as i64
}
const WIRE_BASE_PS: i64 = 500;
const WIRE_TILE_PS: i64 = 60;

#[derive(Clone, Copy)]
enum Loc {
    Unit(u32),
    Site(usize),
}

/// A LUT or CARRY4 for timing: input nets (and whether the input is the
/// dedicated carry cascade), output nets, place, delay.
struct TCell {
    ins: Vec<(u32, bool)>,
    outs: Vec<u32>,
    loc: Loc,
    delay: i64,
}

/// The register-to-register timing graph, cells in topological order.
struct TGraph {
    cells: Vec<TCell>,
    /// Where each net is driven from; `None` for pads and constants (not timed).
    drv: Vec<Option<Loc>>,
    /// Nets driven by a flop.
    ff_q: Vec<u32>,
    /// Flop D inputs: (net, flop place).
    ends: Vec<(u32, Loc)>,
    /// Net names, to print the estimated critical path (`HELION_PLACE_DEBUG`).
    debug_names: Option<Vec<String>>,
}

impl TGraph {
    /// Criticality of every connection (in `conns` order): the longest
    /// register-to-register path through it over the longest path overall
    /// (0 when not timed); and that longest path.
    fn criticality(&self, site_of: &dyn Fn(Loc) -> usize, xy: &dyn Fn(usize) -> (i32, i32)) -> (Vec<f64>, i64) {
        use crate::timing::{CARRY_CI_TO_CO_PS, CARRY_S_TO_O_PS, FF_CKQ_PS, FF_SETUP_PS, SITE_INTERNAL_PS};
        let n = self.drv.len();
        let wire = |net: u32, to: Loc, dedicated: bool| -> i64 {
            let Some(from) = self.drv[net as usize] else { return 0 };
            let (a, b) = (site_of(from), site_of(to));
            if dedicated {
                // CI to CO is the whole stage: take back the S-to-O cell delay.
                CARRY_CI_TO_CO_PS as i64 - CARRY_S_TO_O_PS as i64
            } else if a == b {
                SITE_INTERNAL_PS as i64
            } else {
                let ((x0, y0), (x1, y1)) = (xy(a), xy(b));
                wire_ps((x0 - x1).abs() + (y0 - y1).abs())
            }
        };
        const NONE: i64 = i64::MIN / 4;
        let mut arr = vec![NONE; n];
        for &q in &self.ff_q {
            arr[q as usize] = FF_CKQ_PS as i64;
        }
        for c in &self.cells {
            let mut a = NONE;
            for &(i, ded) in &c.ins {
                if arr[i as usize] > NONE {
                    a = a.max(arr[i as usize] + wire(i, c.loc, ded));
                }
            }
            if a > NONE {
                for &o in &c.outs {
                    arr[o as usize] = arr[o as usize].max(a + c.delay);
                }
            }
        }
        let mut down = vec![NONE; n];
        let mut dmax = 1i64;
        let mut worst = None;
        for &(net, at) in &self.ends {
            let d = wire(net, at, false) + FF_SETUP_PS as i64;
            down[net as usize] = down[net as usize].max(d);
            if arr[net as usize] > NONE && arr[net as usize] + d > dmax {
                dmax = arr[net as usize] + d;
                worst = Some(net);
            }
        }
        if let (Some(names), Some(mut net)) = (&self.debug_names, worst) {
            // The estimated critical path, endpoint first.
            loop {
                let at = self.drv[net as usize].map(site_of).map(xy);
                eprintln!("place: est {:>6} ps  {} at {at:?}", arr[net as usize], names[net as usize]);
                let Some(c) = self.cells.iter().find(|c| c.outs.contains(&net)) else { break };
                let Some(&(i, _)) = c.ins.iter().filter(|&&(i, _)| arr[i as usize] > NONE).max_by_key(|&&(i, ded)| arr[i as usize] + wire(i, c.loc, ded)) else { break };
                net = i;
            }
        }
        for c in self.cells.iter().rev() {
            let d = c.outs.iter().map(|&o| down[o as usize]).max().unwrap_or(NONE);
            if d > NONE {
                for &(i, ded) in &c.ins {
                    down[i as usize] = down[i as usize].max(wire(i, c.loc, ded) + c.delay + d);
                }
            }
        }
        let ratio = |a: i64, rest: i64| if a > NONE && rest > NONE { ((a + rest) as f64 / dmax as f64).clamp(0.0, 1.0) } else { 0.0 };
        let mut conn = Vec::new();
        for c in &self.cells {
            let d = c.outs.iter().map(|&o| down[o as usize]).max().unwrap_or(NONE);
            for &(i, _) in c.ins.iter().filter(|x| !x.1) {
                conn.push(ratio(arr[i as usize], if d > NONE { wire(i, c.loc, false) + c.delay + d } else { NONE }));
            }
        }
        for &(net, at) in &self.ends {
            conn.push(ratio(arr[net as usize], wire(net, at, false) + FF_SETUP_PS as i64));
        }
        (conn, dmax)
    }

    /// Every timed driver-to-sink connection `(net, sink place)`: cell inputs
    /// (not the dedicated carry cascade) in cell order, then flop D inputs.
    fn conns(&self) -> Vec<(u32, Loc)> {
        let cells = self.cells.iter().flat_map(|c| c.ins.iter().filter(|x| !x.1).map(move |&(i, _)| (i, c.loc)));
        cells.chain(self.ends.iter().copied()).collect()
    }
}

pub fn refine(out: &mut Placed7, device: &dyn DeviceQuery) {
    let sites = device.all_slices();
    if sites.is_empty() {
        return;
    }
    let site_idx: HashMap<&str, usize> = sites.iter().enumerate().map(|(i, s)| (s.1.as_str(), i)).collect();
    let letter = |bel: &str| LETTERS.iter().position(|c| bel.starts_with(*c));

    // Slices that stay as they are: wide-mux and shift-register slices,
    // carry slices outside adder chains, and any slice whose LUTs or FFs are
    // not one per letter.
    let mut blocked: HashSet<usize> = HashSet::new();
    for s in out.muxfs.iter().map(|m| m.site.as_str()).chain(out.srls.iter().map(|s| s.site.as_str())) {
        if let Some(&si) = site_idx.get(s) {
            blocked.insert(si);
        }
    }
    // An adder chain is a plain cascade (COUT of SLICE_XxYy feeds CIN of
    // SLICE_XxY(y+1)): each run of its slices up one column moves whole.
    let sxy: Vec<Option<(i32, i32)>> = sites.iter().map(|s| crate::place::parse_site_xy(&s.1)).collect();
    let site_at: HashMap<(i32, i32), usize> = sxy.iter().enumerate().filter_map(|(i, p)| Some(((*p)?, i))).collect();
    let packed_carry: HashMap<&str, &crate::pack::PackedCarry> = out.packed.carries.iter().map(|c| (c.name.as_str(), c)).collect();
    // (column, row, site, CI net, CO net) of each adder slice.
    let mut chain: Vec<(i32, i32, usize, &str, &str)> = Vec::new();
    for c in &out.carries {
        let Some(&si) = site_idx.get(c.site.as_str()) else { return };
        let pc = packed_carry.get(c.name.as_str());
        match (sxy[si], pc) {
            (Some((x, y)), Some(pc)) if out.packed.adder_carries.contains(&c.name) && !blocked.contains(&si) => {
                chain.push((x, y, si, pc.ci_net.as_str(), pc.co_net.as_deref().unwrap_or("")))
            }
            _ => {
                blocked.insert(si);
            }
        }
    }
    chain.sort();
    // A run continues only where the slice's CI is the CO of the slice below.
    let mut runs: Vec<Vec<usize>> = Vec::new();
    for (k, &(x, y, si, ci, _)) in chain.iter().enumerate() {
        match runs.last_mut() {
            Some(r) if k > 0 && chain[k - 1].0 == x && chain[k - 1].1 + 1 == y && !ci.is_empty() && chain[k - 1].4 == ci => r.push(si),
            _ => runs.push(vec![si]),
        }
    }
    let in_run: HashSet<usize> = chain.iter().map(|c| c.2).collect();
    if in_run.len() != chain.len() {
        return; // two carries in one slice: leave the placement as it is
    }
    let mut seen: HashSet<(usize, usize, bool)> = HashSet::new();
    for (site, bel, is_ff) in out.luts.iter().map(|l| (&l.site, &l.bel, false)).chain(out.ffs.iter().map(|f| (&f.site, &f.bel, true))) {
        let Some(&si) = site_idx.get(site.as_str()) else { return };
        if in_run.contains(&si) {
            continue;
        }
        let ok = if is_ff { bel.len() == 3 && bel.ends_with("FF") } else { bel.len() == 5 && bel.ends_with("6LUT") };
        match letter(bel) {
            Some(k) if ok && seen.insert((si, k, is_ff)) => {}
            _ => {
                blocked.insert(si);
            }
        }
    }

    // Movable units: the contents of one letter of an unblocked slice.
    // A slot is site * 4 + letter.
    let mut unit_of_slot: HashMap<usize, u32> = HashMap::new();
    let mut pos: Vec<usize> = Vec::new();
    let mut unit_ff: Vec<Option<usize>> = Vec::new();
    let mut unit_lut: Vec<Option<usize>> = Vec::new();
    let cells = out.luts.iter().map(|l| (&l.site, &l.bel, false)).chain(out.ffs.iter().map(|f| (&f.site, &f.bel, true)));
    let (mut li, mut fi) = (0usize, 0usize);
    for (site, bel, is_ff) in cells {
        let idx = if is_ff { fi += 1; fi - 1 } else { li += 1; li - 1 };
        let si = site_idx[site.as_str()];
        if blocked.contains(&si) || in_run.contains(&si) {
            continue;
        }
        let slot = si * 4 + letter(bel).unwrap();
        let u = *unit_of_slot.entry(slot).or_insert_with(|| {
            pos.push(slot);
            unit_ff.push(None);
            unit_lut.push(None);
            pos.len() as u32 - 1
        }) as usize;
        if is_ff {
            unit_ff[u] = Some(idx);
        } else {
            unit_lut[u] = Some(idx);
        }
    }
    // Units n.. are the slices of adder chains: one per slice, all four
    // letters, moved only with the rest of their run.
    let n = pos.len();
    let mut site_unit: HashMap<usize, u32> = HashMap::new();
    let mut run_of: Vec<usize> = Vec::new();
    let mut run_units: Vec<Vec<u32>> = Vec::new();
    for (r, run) in runs.iter().enumerate() {
        let mut us = Vec::new();
        for &si in run {
            site_unit.insert(si, pos.len() as u32);
            us.push(pos.len() as u32);
            pos.push(si * 4);
            unit_ff.push(None);
            unit_lut.push(None);
            run_of.push(r);
        }
        run_units.push(us);
    }
    let n_all = pos.len();
    let home: Vec<usize> = pos[n..].iter().map(|p| p / 4).collect();
    if n_all < 2 {
        return;
    }
    // Control set of each unit's FF, and per site the set in use and the
    // number of FFs using it.
    let packed = &out.packed;
    let ff_by_name: HashMap<&str, &crate::pack::PackedFf> = packed.ffs.iter().map(|f| (f.name.as_str(), f)).collect();
    let mut ctrl_ids: Vec<CtrlSet> = Vec::new();
    let unit_ctrl: Vec<Option<u32>> = unit_ff
        .iter()
        .map(|f| {
            let pf = ff_by_name.get(out.ffs[(*f)?].name.as_str())?;
            let c = CtrlSet::of(pf, packed);
            Some(match ctrl_ids.iter().position(|x| *x == c) {
                Some(i) => i as u32,
                None => {
                    ctrl_ids.push(c);
                    ctrl_ids.len() as u32 - 1
                }
            })
        })
        .collect();
    let mut site_ctrl: Vec<(u32, u8)> = vec![(0, 0); sites.len()];
    for (u, c) in unit_ctrl.iter().enumerate() {
        if let Some(c) = c {
            let e = &mut site_ctrl[pos[u] / 4];
            *e = (*c, e.1 + 1);
        }
    }

    // Pins per net.
    enum Pin {
        Unit(u32),
        Fixed(i32, i32),
        Pad(i32, i32),
    }
    let locate = |site: &str, bel: &str| -> Option<Pin> {
        let si = *site_idx.get(site)?;
        if let Some(&u) = site_unit.get(&si) {
            return Some(Pin::Unit(u));
        }
        Some(match letter(bel).and_then(|k| unit_of_slot.get(&(si * 4 + k))) {
            Some(&u) if !blocked.contains(&si) => Pin::Unit(u),
            _ => Pin::Fixed(sites[si].2, sites[si].3),
        })
    };
    let mut pins: HashMap<&str, Vec<Pin>> = HashMap::new();
    let mut skip: HashSet<&str> = HashSet::new();
    for b in &packed.bufgs {
        skip.insert(b.in_net.as_str());
        skip.insert(b.out_net.as_str());
    }
    for f in &packed.ffs {
        skip.insert(f.clk_net.as_str());
    }
    let lut_at: HashMap<&str, (&str, &str)> = out.luts.iter().map(|l| (l.name.as_str(), (l.site.as_str(), l.bel.as_str()))).collect();
    let ff_at: HashMap<&str, (&str, &str)> = out.ffs.iter().map(|f| (f.name.as_str(), (f.site.as_str(), f.bel.as_str()))).collect();
    let carry_site: HashMap<&str, &str> = out.carries.iter().map(|c| (c.name.as_str(), c.site.as_str())).collect();
    for l in &packed.luts {
        let Some((site, bel)) = lut_at.get(l.name.as_str()) else { continue };
        for net in l.input_nets.iter().flatten().chain(std::iter::once(&l.output_net)) {
            if let Some(p) = locate(site, bel) {
                pins.entry(net.as_str()).or_default().push(p);
            }
        }
    }
    for f in &packed.ffs {
        let Some((site, bel)) = ff_at.get(f.name.as_str()) else { continue };
        for net in [Some(&f.d_net), Some(&f.q_net), f.ce_net.as_ref(), f.sr_net.as_ref()].into_iter().flatten() {
            if let Some(p) = locate(site, bel) {
                pins.entry(net.as_str()).or_default().push(p);
            }
        }
    }
    for c in &packed.carries {
        let Some(site) = carry_site.get(c.name.as_str()) else { continue };
        let nets = c.o_nets.iter().flatten().chain(c.s_nets.iter().flatten()).chain(c.di_nets.iter().flatten()).chain(c.co_net.iter()).chain(std::iter::once(&c.ci_net));
        for net in nets {
            if let Some(p) = locate(site, "") {
                pins.entry(net.as_str()).or_default().push(p);
            }
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
    // Net name of each internal net (timed); "" for the pad part.
    let mut net_names: Vec<&str> = Vec::new();
    let mut unit_nets: Vec<Vec<u32>> = vec![Vec::new(); n_all];
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
                return false;
            }
            let ni = nets.len() as u32;
            for &u in &units {
                unit_nets[u as usize].push(ni);
            }
            nets.push(Net { fixed, units: units.clone(), weight: weight.into() });
            true
        };
        if add(fixed, INTERNAL_WEIGHT, &mut nets) {
            net_names.push(name);
        }
        if has_pad && add(with_pads, 1, &mut nets) {
            net_names.push("");
        }
    }
    if nets.is_empty() {
        return;
    }
    let timed = std::env::var("HELION_PLACE_NO_TIMING").is_err();
    let debug = std::env::var("HELION_PLACE_DEBUG").is_ok();
    let tg = if timed || debug { timing_graph(out, &site_idx, &unit_of_slot, &site_unit, &blocked) } else { None };
    // Timing acts per connection: each driver-to-sink pair is a two-pin net
    // weighted by its criticality, so a critical sink is pulled to its
    // driver without dragging the rest of a high-fanout net along.
    let mut conn_net: Vec<Option<usize>> = Vec::new();
    if let (true, Some(g)) = (timed, &tg) {
        let at = |l: Loc| match l {
            Loc::Unit(u) => (Some(u), None),
            Loc::Site(s) => (None, Some((sites[s].2, sites[s].3))),
        };
        for (net, sink) in g.conns() {
            let Some(from) = g.drv[net as usize] else {
                conn_net.push(None);
                continue;
            };
            let ((ua, fa), (ub, fb)) = (at(from), at(sink));
            let mut units: Vec<u32> = ua.into_iter().chain(ub).collect();
            units.dedup();
            let fixed = fa.or(fb).map(|(x, y)| (x, x, y, y));
            if units.is_empty() || (units.len() == 1 && fixed.is_none()) {
                conn_net.push(None);
                continue;
            }
            let ni = nets.len() as u32;
            for &u in &units {
                unit_nets[u as usize].push(ni);
            }
            nets.push(Net { fixed, units, weight: 0.into() });
            net_names.push("");
            conn_net.push(Some(ni as usize));
        }
    }

    // Site grid in compact column coordinates; blocked slices are left out.
    let mut cols: Vec<i32> = sites.iter().map(|s| s.2).collect();
    cols.sort();
    cols.dedup();
    let col_of: HashMap<i32, usize> = cols.iter().enumerate().map(|(i, x)| (*x, i)).collect();
    let mut grid: HashMap<(usize, i32), Vec<usize>> = HashMap::new();
    for (i, s) in sites.iter().enumerate() {
        if !blocked.contains(&i) {
            grid.entry((col_of[&s.2], s.3)).or_default().push(i);
        }
    }
    // Slot occupancy: unit id or FREE.
    const FREE: u32 = u32::MAX;
    let mut occ: Vec<u32> = vec![FREE; sites.len() * 4];
    for (u, &slot) in pos.iter().enumerate() {
        if u < n {
            occ[slot] = u as u32;
        } else {
            occ[slot..slot + 4].fill(u as u32);
        }
    }

    let xy = |slot: usize| (sites[slot / 4].2, sites[slot / 4].3);
    let net_cost = |net: &Net, pos: &[usize]| -> i64 {
        let (mut x0, mut x1, mut y0, mut y1) = net.fixed.unwrap_or((i32::MAX, i32::MIN, i32::MAX, i32::MIN));
        for &u in &net.units {
            let (x, y) = xy(pos[u as usize]);
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
        ((x1 - x0) as i64 + (y1 - y0) as i64) * net.weight.get()
    };
    // Timing weights from the current places; returns the estimated critical path.
    let reweight = |nets: &[Net], pos: &[usize]| -> i64 {
        let Some(g) = &tg else { return 0 };
        let site_of = |l: Loc| match l {
            Loc::Unit(u) => pos[u as usize] / 4,
            Loc::Site(s) => s,
        };
        let (conn, dmax) = g.criticality(&site_of, &|s: usize| (sites[s].2, sites[s].3));
        for (c, ni) in conn.iter().zip(&conn_net) {
            if let Some(ni) = ni {
                nets[*ni].weight.set((INTERNAL_WEIGHT as f64 * TIMING_GAIN * c.powi(TIMING_EXP)) as i64);
            }
        }
        dmax
    };
    let est0 = reweight(&nets, &pos);
    let mut cost: Vec<i64> = nets.iter().map(|nt| net_cost(nt, &pos)).collect();
    let mut total: i64 = cost.iter().sum();
    let initial = total;

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let max_rx = cols.len() as i32;
    let (ymin, ymax) = sites.iter().fold((i32::MAX, i32::MIN), |(a, b), s| (a.min(s.3), b.max(s.3)));
    let max_ry = ymax - ymin + 1;
    let mut window: f64; // fraction of the device a move may span
    let moves_per_t = ((n_all as f64).powf(4.0 / 3.0) * 2.0) as usize + 200;
    let mut touched: Vec<u32> = Vec::new();
    let mut mark: Vec<u32> = vec![0; nets.len()];
    let mut stamp = 0u32;

    // One move: a unit to a random slot in the window, swapping with the
    // unit there, or a chain run to a free place in a column (keeping the
    // picked slice's offset in its run). Returns the cost change, leaving
    // the move applied.
    let mut moved: Vec<(u32, usize)> = Vec::new();
    let mut try_move = |pos: &mut Vec<usize>, occ: &mut Vec<u32>, site_ctrl: &mut Vec<(u32, u8)>, cost: &mut Vec<i64>, rng: &mut Rng, window: f64, temp: f64| -> (i64, bool) {
        let a = rng.below(n_all);
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
        let ts = cands[rng.below(cands.len())];
        moved.clear();
        let mut b = FREE;
        let to = ts * 4 + rng.below(4);
        if a < n {
            if to == from {
                return (0, false);
            }
            b = occ[to];
            if b != FREE && b as usize >= n {
                return (0, false); // a chain slice
            }
            // One control set per slice.
            let (fs, ts) = (from / 4, to / 4);
            if fs != ts {
                let fits = |site: usize, c: Option<u32>, leaving: u32| match c {
                    None => true,
                    Some(c) => {
                        let (have, cnt) = site_ctrl[site];
                        let leaving_ff = leaving != FREE && unit_ctrl[leaving as usize].is_some();
                        cnt - leaving_ff as u8 == 0 || have == c
                    }
                };
                if !fits(ts, unit_ctrl[a], b) || (b != FREE && !fits(fs, unit_ctrl[b as usize], a as u32)) {
                    return (0, false);
                }
            }
            moved.push((a as u32, from));
            pos[a] = to;
            if b != FREE {
                moved.push((b, to));
                pos[b as usize] = from;
            }
        } else {
            let r = run_of[a - n];
            let us = &run_units[r];
            let k0 = us.iter().position(|&u| u as usize == a).unwrap() as i32;
            let Some((tx, ty)) = sxy[ts] else { return (0, false) };
            // Free target slices are taken; LUT/FF slots in a target slice
            // swap into the run's old slice (same letter), which needs the
            // old and new places apart.
            let mut targets: Vec<usize> = Vec::with_capacity(us.len());
            let (mut own, mut units) = (false, false);
            for i in 0..us.len() as i32 {
                let Some(&t) = site_at.get(&(tx, ty - k0 + i)) else { return (0, false) };
                if blocked.contains(&t) {
                    return (0, false);
                }
                for &o in &occ[t * 4..t * 4 + 4] {
                    if o == FREE {
                    } else if (o as usize) < n {
                        units = true;
                    } else if run_of[o as usize - n] == r {
                        own = true;
                    } else {
                        return (0, false);
                    }
                }
                targets.push(t);
            }
            if targets[0] * 4 == pos[us[0] as usize] || (own && units) {
                return (0, false);
            }
            for (&u, &t) in us.iter().zip(&targets) {
                let old = pos[u as usize];
                moved.push((u, old));
                pos[u as usize] = t * 4;
                for k in 0..4 {
                    let o = occ[t * 4 + k];
                    if o != FREE && (o as usize) < n {
                        moved.push((o, t * 4 + k));
                        pos[o as usize] = old + k;
                    }
                }
            }
        }
        stamp = stamp.wrapping_add(1);
        touched.clear();
        for &(u, _) in &moved {
            for &ni in &unit_nets[u as usize] {
                if mark[ni as usize] != stamp {
                    mark[ni as usize] = stamp;
                    touched.push(ni);
                }
            }
        }
        let mut delta = 0i64;
        for &ni in &touched {
            delta += net_cost(&nets[ni as usize], pos) - cost[ni as usize];
        }
        let accept = delta <= 0 || (temp > 0.0 && rng.unit() < (-(delta as f64) / temp).exp());
        if !accept {
            for &(u, old) in &moved {
                pos[u as usize] = old;
            }
            return (delta, false);
        }
        for &ni in &touched {
            cost[ni as usize] = net_cost(&nets[ni as usize], pos);
        }
        if a < n {
            occ[to] = a as u32;
            occ[from] = b;
            let (fs, ts) = (from / 4, to / 4);
            if fs != ts {
                if let Some(c) = unit_ctrl[a] {
                    site_ctrl[fs].1 -= 1;
                    site_ctrl[ts] = (c, site_ctrl[ts].1 + 1);
                }
                if b != FREE {
                    if let Some(c) = unit_ctrl[b as usize] {
                        site_ctrl[ts].1 -= 1;
                        site_ctrl[fs] = (c, site_ctrl[fs].1 + 1);
                    }
                }
            }
        } else {
            for &(u, old) in &moved {
                if u as usize >= n {
                    occ[old..old + 4].fill(FREE);
                }
            }
            for &(u, old) in &moved {
                let p = pos[u as usize];
                if u as usize >= n {
                    occ[p..p + 4].fill(u);
                } else {
                    occ[p] = u;
                    // The slice's control set goes with its slots.
                    let (src, dst) = (old / 4, p / 4);
                    if site_ctrl[src].1 > 0 {
                        site_ctrl[dst] = site_ctrl[src];
                        site_ctrl[src] = (0, 0);
                    }
                }
            }
        }
        (delta, true)
    };

    // Starting temperature: the typical uphill move within a local window.
    // The greedy placement is compact; scattering it over the device (most
    // of which a design leaves empty) only for the anneal to gather it
    // back is what an infinite-temperature start would do.
    window = 0.05;
    let mut temp = {
        let mut sum = 0f64;
        let mut cnt = 0usize;
        for _ in 0..(n_all * 4).clamp(200, 20_000) {
            let (d, ok) = try_move(&mut pos, &mut occ, &mut site_ctrl, &mut cost, &mut rng, window, 0.0);
            if ok {
                total += d;
            } else if d > 0 {
                sum += d as f64;
                cnt += 1;
            }
        }
        if cnt == 0 { 1.0 } else { sum / cnt as f64 }
    };
    // Stop when the temperature is a small fraction of the average net cost.
    while temp > 0.005 * (total.max(1) as f64 / nets.len() as f64).max(1.0) {
        let mut accepted = 0usize;
        for _ in 0..moves_per_t {
            let (d, ok) = try_move(&mut pos, &mut occ, &mut site_ctrl, &mut cost, &mut rng, window, temp);
            if ok {
                total += d;
                accepted += 1;
            }
        }
        let rate = accepted as f64 / moves_per_t as f64;
        window = (window * (0.56 + rate)).clamp(0.01, 1.0);
        if timed && tg.is_some() {
            reweight(&nets, &pos);
            for (c, nt) in cost.iter_mut().zip(&nets) {
                *c = net_cost(nt, &pos);
            }
            total = cost.iter().sum();
        }
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
        let (d, ok) = try_move(&mut pos, &mut occ, &mut site_ctrl, &mut cost, &mut rng, 0.02, 0.0);
        if ok {
            total += d;
        }
    }
    if debug {
        let est1 = reweight(&nets, &pos);
        let (mut xs, mut ys): (Vec<i32>, Vec<i32>) = pos.iter().map(|&p| xy(p)).unzip();
        xs.sort();
        ys.sort();
        let q = |v: &[i32], f: f64| v[((v.len() - 1) as f64 * f) as usize];
        eprintln!("place: units span x {}..{} (10-90% {}..{}), y {}..{} (10-90% {}..{})", xs[0], xs[xs.len() - 1], q(&xs, 0.1), q(&xs, 0.9), ys[0], ys[ys.len() - 1], q(&ys, 0.1), q(&ys, 0.9));
        eprintln!("place: {n} slots, {} chain runs, {} nets, cost {initial} -> {total}, estimated critical path {est0} -> {est1} ps", runs.len(), nets.len());
    }

    // Write the new places back: chain slices first (by their old site;
    // no unit started in one), then units.
    let moved_site: HashMap<String, (String, String)> = (n..n_all)
        .filter(|&u| pos[u] / 4 != home[u - n])
        .map(|u| (sites[home[u - n]].1.clone(), (sites[pos[u] / 4].0.clone(), sites[pos[u] / 4].1.clone())))
        .collect();
    let relocate = |tile: &mut String, site: &mut String| {
        if let Some((t, s)) = moved_site.get(site.as_str()) {
            (*tile, *site) = (t.clone(), s.clone());
        }
    };
    for l in out.luts.iter_mut() {
        relocate(&mut l.tile, &mut l.site);
    }
    for f in out.ffs.iter_mut() {
        relocate(&mut f.tile, &mut f.site);
    }
    for c in out.carries.iter_mut() {
        relocate(&mut c.tile, &mut c.site);
    }
    for (u, &slot) in pos.iter().enumerate().take(n) {
        let (tile, site) = (&sites[slot / 4].0, &sites[slot / 4].1);
        let k = LETTERS[slot % 4];
        if let Some(li) = unit_lut[u] {
            let l = &mut out.luts[li];
            (l.tile, l.site, l.bel) = (tile.clone(), site.clone(), format!("{k}6LUT"));
        }
        if let Some(fi) = unit_ff[u] {
            let f = &mut out.ffs[fi];
            (f.tile, f.site, f.bel) = (tile.clone(), site.clone(), format!("{k}FF"));
        }
    }
}

/// The timing graph of the placed design. `None` when the logic has a
/// combinational cycle (timing reports that).
fn timing_graph<'a>(out: &'a Placed7, site_idx: &HashMap<&str, usize>, unit_of_slot: &HashMap<usize, u32>, site_unit: &HashMap<usize, u32>, blocked: &HashSet<usize>) -> Option<TGraph> {
    use crate::timing::{CARRY_S_TO_O_PS, LUT_PS};
    let packed = &out.packed;
    let mut ids: HashMap<&str, u32> = HashMap::new();
    let mut id = |s: &'a str| -> u32 {
        let k = ids.len() as u32;
        *ids.entry(s).or_insert(k)
    };
    let loc = |site: &str, bel: &str| -> Option<Loc> {
        let si = *site_idx.get(site)?;
        if let Some(&u) = site_unit.get(&si) {
            return Some(Loc::Unit(u));
        }
        let k = LETTERS.iter().position(|c| bel.starts_with(*c));
        Some(match k.and_then(|k| unit_of_slot.get(&(si * 4 + k))) {
            Some(&u) if !blocked.contains(&si) => Loc::Unit(u),
            _ => Loc::Site(si),
        })
    };
    let lut_at: HashMap<&str, (&str, &str)> = out.luts.iter().map(|l| (l.name.as_str(), (l.site.as_str(), l.bel.as_str()))).collect();
    let ff_at: HashMap<&str, (&str, &str)> = out.ffs.iter().map(|f| (f.name.as_str(), (f.site.as_str(), f.bel.as_str()))).collect();
    let carry_at: HashMap<&str, &str> = out.carries.iter().map(|c| (c.name.as_str(), c.site.as_str())).collect();
    let mut cells: Vec<TCell> = Vec::new();
    for l in &packed.luts {
        let Some(&(site, bel)) = lut_at.get(l.name.as_str()) else { continue };
        let Some(at) = loc(site, bel) else { continue };
        let ins = l.input_nets.iter().flatten().map(|n| (id(n), false)).collect();
        cells.push(TCell { ins, outs: vec![id(&l.output_net)], loc: at, delay: LUT_PS as i64 });
    }
    for c in &packed.carries {
        let Some(at) = carry_at.get(c.name.as_str()).and_then(|s| loc(s, "")) else { continue };
        let mut ins: Vec<(u32, bool)> = c.s_nets.iter().chain(&c.di_nets).flatten().map(|n| (id(n), false)).collect();
        if !c.ci_net.is_empty() {
            ins.push((id(&c.ci_net), true));
        }
        let outs = c.o_nets.iter().chain(&c.co_nets).flatten().chain(c.co_net.iter()).map(|n| id(n)).collect();
        cells.push(TCell { ins, outs, loc: at, delay: CARRY_S_TO_O_PS as i64 });
    }
    let mut ff_q = Vec::new();
    let mut ends = Vec::new();
    let mut ff_locs: Vec<(u32, Loc)> = Vec::new();
    for f in &packed.ffs {
        let Some(&(site, bel)) = ff_at.get(f.name.as_str()) else { continue };
        let Some(at) = loc(site, bel) else { continue };
        ff_q.push(id(&f.q_net));
        ff_locs.push((id(&f.q_net), at));
        ends.push((id(&f.d_net), at));
    }
    let n = ids.len();
    let mut drv: Vec<Option<Loc>> = vec![None; n];
    let mut drv_cell: Vec<Option<usize>> = vec![None; n];
    for (ci, c) in cells.iter().enumerate() {
        for &o in &c.outs {
            drv[o as usize] = Some(c.loc);
            drv_cell[o as usize] = Some(ci);
        }
    }
    for (q, at) in ff_locs {
        drv[q as usize] = Some(at);
    }
    // Topological order (Kahn) over cell-to-cell edges.
    let mut indeg = vec![0usize; cells.len()];
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); cells.len()];
    for (ci, c) in cells.iter().enumerate() {
        let mut from: Vec<usize> = c.ins.iter().filter_map(|&(i, _)| drv_cell[i as usize]).collect();
        from.sort();
        from.dedup();
        for p in from {
            indeg[ci] += 1;
            succ[p].push(ci);
        }
    }
    let mut order: Vec<usize> = (0..cells.len()).filter(|&c| indeg[c] == 0).collect();
    let mut k = 0;
    while k < order.len() {
        let c = order[k];
        k += 1;
        for &s in &succ[c] {
            indeg[s] -= 1;
            if indeg[s] == 0 {
                order.push(s);
            }
        }
    }
    if order.len() != cells.len() {
        return None;
    }
    let mut slots: Vec<Option<TCell>> = cells.into_iter().map(Some).collect();
    let cells = order.into_iter().map(|c| slots[c].take().unwrap()).collect();
    let debug_names = std::env::var("HELION_PLACE_DEBUG").is_ok().then(|| {
        let mut v = vec![String::new(); ids.len()];
        for (k, &i) in &ids {
            v[i as usize] = k.to_string();
        }
        v
    });
    Some(TGraph { cells, drv, ff_q, ends, debug_names })
}
