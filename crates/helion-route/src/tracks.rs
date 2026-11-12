//! Routing on the fabric's general interconnect (`helion_device::tracks`).
//!
//! Every connection of the packed design is realised: LUT inputs driven by
//! flops, by other LUTs and by input pads, and output pads from any column.
//! A connection the IMUX can make directly (the driver's flop is a near
//! neighbour, or the driver is in the same CLB) stays direct; everything
//! else is routed over wires.
//!
//! Wires are routed with negotiated congestion (PathFinder): each net is a
//! tree from its driver to all of its wire-routed sinks, a wire may carry
//! one net, and nets sharing a wire are ripped up and rerouted with rising
//! present and history costs until no wire is shared. A design that still
//! has a shared wire, an unplaced driver or more pads than the part is an
//! error: no bitstream is written for a design that is not fully connected.

use crate::{imux_sel, ImuxRoute};
use helion_device::tracks::{self as tk, TRACKS};
use helion_device::{Device, Site, SiteKind};
use helion_pack::NetSrc;
use helion_place::Placed;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

/// Switch-box setting of one wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SbRoute {
    pub x: u32,
    pub y: u32,
    pub dir: u8,
    pub track: u8,
    /// `SB[dir][track]` select (see `helion_device::tracks`).
    pub sel: u8,
}

/// An output pad reading a track of the CLB above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IobTrack {
    pub x: u32,
    pub y: u32,
    pub track: u8,
}

/// An input pad and the packed input it carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IobInput {
    pub x: u32,
    pub y: u32,
    /// Index into `packed.conn.inputs`.
    pub input: u32,
    pub net: String,
}

/// Everything the general interconnect is programmed with.
#[derive(Clone, Debug, Default)]
pub struct TrackRoutes {
    /// The design was routed from its full connection list.
    pub active: bool,
    pub sb: Vec<SbRoute>,
    /// LUT inputs reading a wire: `sel` is `side*TRACKS + track`, `IMUXT` set.
    pub imux: Vec<ImuxRoute>,
    pub iob_out: Vec<IobTrack>,
    pub iob_in: Vec<IobInput>,
    /// Nets routed over wires, and negotiation rounds used.
    pub nets: u32,
    pub iters: u32,
    /// LUT inputs the netlist leaves without a driver (they read 0).
    pub undriven: u32,
}

/// How an output pad gets its signal.
pub(crate) enum PadPlan {
    /// Column-local source select in the pad's own word.
    Direct { clb: (u32, u32), ble: u8, from_ff: bool },
    /// Over wires; `path` runs from the driver tile to the pad tile.
    Track { clb: (u32, u32), ble: u8, from_ff: bool, path: Vec<(u32, u32)> },
}

pub(crate) struct Routing {
    pub direct_imux: Vec<ImuxRoute>,
    pub tracks: TrackRoutes,
    /// Per `placed.iob_sites` index.
    pub pads: Vec<Option<PadPlan>>,
}

#[derive(Clone, Copy)]
enum Src {
    /// Driver BLE: tile and switch-box select.
    Ble { x: u32, y: u32, sel: u8 },
    /// Input pad at the bottom of column `x` (assigned later).
    Pad { input: u32 },
}

#[derive(Clone, Copy)]
enum Sink {
    Pin { x: u32, y: u32, mux: u32 },
    Pad { iob: usize },
}

struct Net {
    src: Src,
    sinks: Vec<Sink>,
}

struct Grid {
    x0: u32,
    /// Row of the pads.
    y0: u32,
    cols: u32,
    /// CLB rows plus the pad row.
    rows: u32,
}

impl Grid {
    fn tile(&self, x: u32, y: u32) -> Option<u32> {
        (x >= self.x0 && y >= self.y0 && x - self.x0 < self.cols && y - self.y0 < self.rows).then(|| (y - self.y0) * self.cols + (x - self.x0))
    }
    fn xy(&self, tile: u32) -> (u32, u32) {
        (self.x0 + tile % self.cols, self.y0 + tile / self.cols)
    }
    fn wire(&self, x: u32, y: u32, dir: u32, track: u32) -> Option<u32> {
        // A wire exists when both ends are on the grid; the pad row sends
        // north only and is entered from the north only.
        let (nx, ny) = tk::step(x, y, dir)?;
        self.tile(nx, ny)?;
        if (y == self.y0 && dir != tk::N) || (ny == self.y0 && dir != tk::S) {
            return None;
        }
        Some((self.tile(x, y)? * 4 + dir) * TRACKS + track)
    }
    fn parts(&self, wire: u32) -> (u32, u32, u32, u32) {
        let (track, rest) = (wire % TRACKS, wire / TRACKS);
        let (x, y) = self.xy(rest / 4);
        (x, y, rest % 4, track)
    }
    /// Tile a wire arrives in.
    fn dest(&self, wire: u32) -> (u32, u32) {
        let (x, y, dir, _) = self.parts(wire);
        tk::step(x, y, dir).expect("wire has a destination")
    }
    fn len(&self) -> usize {
        (self.cols * self.rows * 4 * TRACKS) as usize
    }
}

const BASE_COST: u32 = 10;
const MAX_ROUNDS: u32 = 80;
/// Tree wires this many tiles farther from a sink than the nearest one are
/// not considered as branch points.
const SEED_SLACK: u32 = 3;
/// Tiles a bounded search may leave the box around its seeds and sink.
const BOX_MARGIN: u32 = 4;

fn pad_xy(dev: &Device, site: &Site) -> (u32, u32) {
    let _ = dev;
    (site.x, site.y)
}

pub(crate) fn route(placed: &Placed, dev: &Device) -> Result<Routing, String> {
    let packed = &placed.packed;
    let conn = &packed.conn;
    let n_sites = placed.lutff_sites.len();
    if n_sites < packed.lutffs.len() {
        return Err(format!(
            "route: {} of {} LUT/flop clusters have no site on this part: the design does not fit",
            packed.lutffs.len() - n_sites,
            packed.lutffs.len()
        ));
    }
    // A LUT input whose net nothing drives cannot be connected. It reads 0
    // (bitgen makes the LUT independent of it) and is reported: the
    // netlist handed to the back end is incomplete.
    if !conn.undriven.is_empty() {
        eprintln!(
            "route: warning: {} LUT inputs have no driver in the netlist and read 0 (first: {})",
            conn.undriven.len(),
            conn.undriven[0]
        );
    }
    let grid = Grid { x0: dev.clb_x0, y0: dev.clb_y0.saturating_sub(1), cols: dev.interior_cols, rows: dev.interior_rows + 1 };
    let site_of = |i: u32| -> Result<(Site, u8), String> {
        placed.lutff_sites.get(i as usize).copied().ok_or_else(|| format!("route: cluster {i} is not placed"))
    };
    let registered = |i: u32| packed.lutffs.get(i as usize).map(|l| !l.ff_cell.is_empty()).unwrap_or(false);

    let mut direct_imux: Vec<ImuxRoute> = Vec::new();
    let mut nets: Vec<Net> = Vec::new();
    let mut net_of: HashMap<NetSrc, usize> = HashMap::new();
    let mut net_for = |src: NetSrc, nets: &mut Vec<Net>| -> Result<usize, String> {
        if let Some(&i) = net_of.get(&src) {
            return Ok(i);
        }
        let s = match src {
            NetSrc::BleQ(i) => {
                let (site, ble) = site_of(i)?;
                Src::Ble { x: site.x, y: site.y, sel: (tk::SB_BLE_OUT + ble as u32) as u8 }
            }
            NetSrc::BleO(i) => {
                let (site, ble) = site_of(i)?;
                Src::Ble { x: site.x, y: site.y, sel: (tk::SB_LUT_O + ble as u32) as u8 }
            }
            NetSrc::Input(i) => Src::Pad { input: i },
        };
        nets.push(Net { src: s, sinks: Vec::new() });
        net_of.insert(src, nets.len() - 1);
        Ok(nets.len() - 1)
    };

    // LUT inputs.
    for pc in &conn.pins {
        let (site, ble) = site_of(pc.lutff)?;
        let mux = ble as u32 * 8 + pc.pin as u32;
        let direct = match pc.src {
            NetSrc::BleQ(d) => {
                let (dsite, dble) = site_of(d)?;
                imux_sel(dsite, site, dble).ok()
            }
            NetSrc::BleO(d) => {
                let (dsite, dble) = site_of(d)?;
                // The LUT output is visible to the IMUX in its own CLB only.
                (dsite.x == site.x && dsite.y == site.y).then_some(24 + dble)
            }
            NetSrc::Input(_) => None,
        };
        match direct {
            Some(sel) => direct_imux.push(ImuxRoute { x: site.x, y: site.y, mux, sel }),
            None => {
                let n = net_for(pc.src, &mut nets)?;
                nets[n].sinks.push(Sink::Pin { x: site.x, y: site.y, mux });
            }
        }
    }

    // Output pads.
    let mut pads: Vec<Option<PadPlan>> = Vec::with_capacity(placed.iob_sites.len());
    let mut pad_net: Vec<Option<usize>> = vec![None; placed.iob_sites.len()];
    for (ii, site) in placed.iob_sites.iter().enumerate() {
        let Some(io) = packed.iobs.get(ii) else {
            pads.push(None);
            continue;
        };
        let Some(&src) = conn.iob_src.get(&io.cell) else {
            return Err(format!("route: output pad {} (net {}) has no driver the fabric can connect", io.cell, io.from_net));
        };
        let (i, is_q) = match src {
            NetSrc::BleQ(i) => (i, true),
            NetSrc::BleO(i) => (i, false),
            NetSrc::Input(_) => return Err(format!("route: output pad {} is fed straight from an input pad", io.cell)),
        };
        let (csite, ble) = site_of(i)?;
        let (px, py) = pad_xy(dev, site);
        // The pad's own source select reaches the BLE output (flop when the
        // flop is used, else the LUT) of any CLB in its column.
        let column_local = csite.x == px && csite.y > py && (is_q || !registered(i));
        if column_local {
            pads.push(Some(PadPlan::Direct { clb: (csite.x, csite.y), ble, from_ff: is_q }));
        } else {
            let n = net_for(src, &mut nets)?;
            nets[n].sinks.push(Sink::Pad { iob: ii });
            pad_net[ii] = Some(n);
            pads.push(Some(PadPlan::Track { clb: (csite.x, csite.y), ble, from_ff: is_q, path: Vec::new() }));
        }
    }

    // Input pads: a free pad, the one asked for by LOC or else the one
    // nearest the middle of the loads.
    let pad_row = grid.y0;
    let mut taken: HashSet<u32> = placed.iob_sites.iter().map(|s| s.x).collect();
    let all_pads: Vec<u32> = dev.iob_sites().filter(|s| s.kind == SiteKind::Iob).map(|s| s.x).collect();
    let mut input_x: HashMap<u32, u32> = HashMap::new();
    let mut iob_in: Vec<IobInput> = Vec::new();
    let mut want: Vec<(u32, Option<u32>, u32)> = Vec::new(); // (input, LOC x, mean sink x)
    for net in &nets {
        let Src::Pad { input } = net.src else { continue };
        let xs: Vec<u32> = net
            .sinks
            .iter()
            .map(|s| match s {
                Sink::Pin { x, .. } => *x,
                Sink::Pad { iob } => placed.iob_sites[*iob].x,
            })
            .collect();
        let mean = xs.iter().sum::<u32>() / xs.len().max(1) as u32;
        let loc = conn.inputs.get(input as usize).and_then(|i| i.loc.as_deref()).and_then(|l| {
            let rest = l.strip_prefix("IOB_X")?;
            let (x, y) = rest.split_once('Y')?;
            (y.parse::<u32>().ok()? == pad_row).then_some(x.parse::<u32>().ok()?)
        });
        want.push((input, loc, mean));
    }
    want.sort_by_key(|w| (w.1.is_none(), w.0));
    for (input, loc, mean) in want {
        let x = match loc {
            Some(x) if all_pads.contains(&x) && !taken.contains(&x) => x,
            Some(x) => return Err(format!("route: input {} asks for pad IOB_X{x}Y{pad_row}, which is taken or not on this part", conn.inputs[input as usize].net)),
            None => *all_pads
                .iter()
                .filter(|x| !taken.contains(x))
                .min_by_key(|x| (x.abs_diff(mean), **x))
                .ok_or_else(|| {
                    format!(
                        "route: the design needs {} pads ({} outputs, {} inputs) and the part has {}",
                        placed.iob_sites.len() + conn.inputs.len(),
                        placed.iob_sites.len(),
                        conn.inputs.len(),
                        all_pads.len()
                    )
                })?,
        };
        taken.insert(x);
        input_x.insert(input, x);
        iob_in.push(IobInput { x, y: pad_row, input, net: conn.inputs[input as usize].net.clone() });
    }

    // Start wires and sink tiles.
    let starts: Vec<Vec<u32>> = nets
        .iter()
        .map(|net| {
            let (x, y) = match net.src {
                Src::Ble { x, y, .. } => (x, y),
                Src::Pad { input } => (input_x[&input], pad_row),
            };
            let mut v = Vec::new();
            for d in 0..4 {
                for t in 0..TRACKS {
                    v.extend(grid.wire(x, y, d, t));
                }
            }
            v
        })
        .collect();
    let sink_tile = |s: &Sink| -> (u32, u32) {
        match s {
            Sink::Pin { x, y, .. } => (*x, *y),
            Sink::Pad { iob } => (placed.iob_sites[*iob].x, placed.iob_sites[*iob].y),
        }
    };

    // Negotiated routing.
    let n_wires = grid.len();
    let mut occ: Vec<u16> = vec![0; n_wires];
    let mut hist: Vec<u32> = vec![0; n_wires];
    // Per net: wires of its tree with their parent wire (`u32::MAX`: the
    // driver), and the wire each sink reads.
    let mut tree: Vec<Vec<(u32, u32)>> = vec![Vec::new(); nets.len()];
    let mut sink_wire: Vec<Vec<u32>> = nets.iter().map(|n| vec![u32::MAX; n.sinks.len()]).collect();
    let mut g: Vec<u32> = vec![0; n_wires];
    let mut parent: Vec<u32> = vec![0; n_wires];
    let mut seen: Vec<u32> = vec![0; n_wires];
    let mut closed: Vec<u32> = vec![0; n_wires];
    let mut chan_closed: Vec<u32> = vec![0; n_wires / TRACKS as usize];
    let mut in_tree: Vec<u32> = vec![0; n_wires];
    let mut epoch = 0u32;
    let mut tree_epoch = 0u32;
    let mut pres_fac = 0.5f32;
    let mut rounds = 0u32;
    let mut todo: Vec<usize> = (0..nets.len()).filter(|&i| !nets[i].sinks.is_empty()).collect();
    let routed_nets = todo.len() as u32;
    loop {
        rounds += 1;
        for &ni in &todo {
            for &(w, _) in &tree[ni] {
                occ[w as usize] -= 1;
            }
            tree[ni].clear();
            tree_epoch += 1;
            // Near sinks first, so far ones branch off the tree.
            let src_xy = match nets[ni].src {
                Src::Ble { x, y, .. } => (x, y),
                Src::Pad { input } => (input_x[&input], pad_row),
            };
            let mut order: Vec<usize> = (0..nets[ni].sinks.len()).collect();
            order.sort_by_key(|&k| {
                let (x, y) = sink_tile(&nets[ni].sinks[k]);
                (x.abs_diff(src_xy.0) + y.abs_diff(src_xy.1), k)
            });
            for k in order {
                let goal = sink_tile(&nets[ni].sinks[k]);
                let h = |w: u32| -> u32 {
                    let (x, y) = grid.dest(w);
                    (x.abs_diff(goal.0) + y.abs_diff(goal.1)) * BASE_COST
                };
                let cost = |w: u32, occ: &[u16], hist: &[u32]| -> u32 { ((BASE_COST + hist[w as usize]) as f32 * (1.0 + pres_fac * occ[w as usize] as f32)) as u32 };
                let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();
                // Branch off the part of the tree nearest the sink: only
                // tree wires within a few tiles of the closest one are
                // seeds, and the search stays in a box around seeds and
                // sink. A search that finds nothing is repeated without
                // either limit.
                // The box widens every round: congested nets need room to spread.
                let margin = BOX_MARGIN + 4 * (rounds - 1);
                let near = tree[ni].iter().map(|&(w, _)| h(w)).min();
                let mut found = None;
                for bounded in [true, false] {
                    epoch += 1;
                    heap.clear();
                    let (mut bx0, mut by0, mut bx1, mut by1) = (goal.0, goal.1, goal.0, goal.1);
                    let mut grow = |x: u32, y: u32| {
                        bx0 = bx0.min(x);
                        by0 = by0.min(y);
                        bx1 = bx1.max(x);
                        by1 = by1.max(y);
                    };
                    let reach = near.map(|n| n + SEED_SLACK * BASE_COST);
                    for &(w, _) in &tree[ni] {
                        if bounded && reach.is_some_and(|r| h(w) > r) {
                            continue;
                        }
                        seen[w as usize] = epoch;
                        g[w as usize] = 0;
                        parent[w as usize] = u32::MAX - 1; // already in the tree
                        heap.push(Reverse((h(w), w)));
                        let (x, y) = grid.dest(w);
                        grow(x, y);
                    }
                    // The driver itself is a seed unless the tree is closer.
                    if !bounded || near.is_none() || near.is_some_and(|n| starts[ni].first().is_none_or(|&w0| h(w0) <= n + (SEED_SLACK + 1) * BASE_COST)) {
                        for &w in &starts[ni] {
                            if in_tree[w as usize] == tree_epoch {
                                continue;
                            }
                            let c = cost(w, &occ, &hist);
                            seen[w as usize] = epoch;
                            g[w as usize] = c;
                            parent[w as usize] = u32::MAX;
                            heap.push(Reverse((c + h(w), w)));
                            let (x, y) = grid.dest(w);
                            grow(x, y);
                        }
                    }
                    let (bx0, by0, bx1, by1) = (bx0.saturating_sub(margin), by0.saturating_sub(margin), bx1 + margin, by1 + margin);
                while let Some(Reverse((_, w))) = heap.pop() {
                    if closed[w as usize] == epoch || chan_closed[(w / TRACKS) as usize] == epoch {
                        continue;
                    }
                    closed[w as usize] = epoch;
                    chan_closed[(w / TRACKS) as usize] = epoch;
                    let (dx, dy) = grid.dest(w);
                    if (dx, dy) == goal {
                        found = Some(w);
                        break;
                    }
                    if dy == pad_row {
                        continue; // a pad is an end, not a way through
                    }
                    if bounded && (dx < bx0 || dx > bx1 || dy < by0 || dy > by1) {
                        continue;
                    }
                    let (_, _, dir, _) = grid.parts(w);
                    for d2 in 0..4 {
                        if d2 == tk::opposite(dir) {
                            continue;
                        }
                        // Every wire of a channel ends in the same tile with
                        // the same ways on, so only the cheapest free one
                        // is worth trying.
                        let Some(w0) = grid.wire(dx, dy, d2, 0) else { continue };
                        if chan_closed[(w0 / TRACKS) as usize] == epoch {
                            continue;
                        }
                        let mut best: Option<(u32, u32)> = None;
                        for w2 in w0..w0 + TRACKS {
                            // A tree wire has its driver: it is a start, never re-entered.
                            if in_tree[w2 as usize] == tree_epoch {
                                continue;
                            }
                            let c = cost(w2, &occ, &hist);
                            if best.is_none_or(|(bc, _)| c < bc) {
                                best = Some((c, w2));
                            }
                        }
                        let Some((c, w2)) = best else { continue };
                        let ng = g[w as usize] + c;
                        if seen[w2 as usize] != epoch || ng < g[w2 as usize] {
                            seen[w2 as usize] = epoch;
                            g[w2 as usize] = ng;
                            parent[w2 as usize] = w;
                            heap.push(Reverse((ng + h(w2), w2)));
                        }
                    }
                }
                    if found.is_some() {
                        break;
                    }
                }
                let Some(end) = found else {
                    return Err(format!("route: no path on the interconnect to tile ({}, {})", goal.0, goal.1));
                };
                sink_wire[ni][k] = end;
                let mut w = end;
                while parent[w as usize] != u32::MAX - 1 {
                    let p = parent[w as usize];
                    tree[ni].push((w, p));
                    in_tree[w as usize] = tree_epoch;
                    occ[w as usize] += 1;
                    if p == u32::MAX {
                        break;
                    }
                    w = p;
                }
            }
        }
        let shared: Vec<bool> = occ.iter().map(|&c| c > 1).collect();
        let n_shared = shared.iter().filter(|s| **s).count();
        if n_shared == 0 {
            break;
        }
        if rounds >= MAX_ROUNDS {
            return Err(format!(
                "route: congestion not resolved after {rounds} rounds: {n_shared} wires still carry more than one net"
            ));
        }
        for (w, &c) in occ.iter().enumerate() {
            if c > 1 {
                hist[w] += (c as u32 - 1) * (BASE_COST / 2);
            }
        }
        pres_fac = (pres_fac * 1.7).min(1000.0);
        todo = (0..nets.len()).filter(|&i| tree[i].iter().any(|&(w, _)| shared[w as usize])).collect();
    }

    // Results.
    let mut out = TrackRoutes { active: true, iob_in, nets: routed_nets, iters: rounds, undriven: conn.undriven.len() as u32, ..Default::default() };
    for (ni, net) in nets.iter().enumerate() {
        for &(w, p) in &tree[ni] {
            let (x, y, dir, track) = grid.parts(w);
            let sel = if p == u32::MAX {
                match net.src {
                    Src::Ble { sel, .. } => sel as u32,
                    // The pad drives its own wires; nothing to select.
                    Src::Pad { .. } => continue,
                }
            } else {
                let (_, _, pdir, ptrack) = grid.parts(p);
                tk::SB_WIRE + tk::opposite(pdir) * TRACKS + ptrack
            };
            out.sb.push(SbRoute { x, y, dir: dir as u8, track: track as u8, sel: sel as u8 });
        }
        for (k, sink) in net.sinks.iter().enumerate() {
            let w = sink_wire[ni][k];
            let (_, _, dir, track) = grid.parts(w);
            match *sink {
                Sink::Pin { x, y, mux } => out.imux.push(ImuxRoute { x, y, mux, sel: (tk::opposite(dir) * TRACKS + track) as u8 }),
                Sink::Pad { iob } => {
                    let s = &placed.iob_sites[iob];
                    out.iob_out.push(IobTrack { x: s.x, y: s.y, track: track as u8 });
                    // Tiles from the driver to the pad, for delay and display.
                    let by_wire: HashMap<u32, u32> = tree[ni].iter().copied().collect();
                    let mut tiles = vec![(s.x, s.y)];
                    let mut cur = w;
                    loop {
                        let (x, y, _, _) = grid.parts(cur);
                        tiles.push((x, y));
                        match by_wire.get(&cur) {
                            Some(&p) if p != u32::MAX => cur = p,
                            _ => break,
                        }
                    }
                    tiles.reverse();
                    if let Some(Some(PadPlan::Track { path, .. })) = pads.get_mut(iob) {
                        *path = tiles;
                    }
                }
            }
        }
    }
    let _ = pad_net;
    out.sb.sort_by_key(|s| (s.y, s.x, s.dir, s.track));
    Ok(Routing { direct_imux, tracks: out, pads })
}
