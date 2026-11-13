//! Timing over the real logic graph of a routed design.
//!
//! The first-generation analysis charged every path one LUT. This walks
//! the packed connection list instead: a path runs from a flop or an input
//! pad through as many LUTs as the netlist has, and each connection costs
//! what it was routed as: nothing extra inside a CLB, one hop per tile for
//! a direct neighbour connection, one hop per wire on the interconnect.

use helion_backend::TimingModel;
use helion_pack::{HardKind, NetSrc};
use helion_route::Routed;
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct GraphTiming {
    /// Worst arrival at a flop's data input plus setup (ps).
    pub reg_ps: i64,
    /// Worst arrival at an output pad, pad delay included (ps).
    pub out_ps: i64,
    /// LUTs on the longest path by LUT count.
    pub levels: u32,
    /// Flop data inputs and output pads analysed.
    pub endpoints: usize,
    /// Endpoints that miss `period_ps`, and the sum of their negative slack.
    pub failing: usize,
    pub tns_ps: i64,
    /// The slowest path, start to end: (what, arrival in ps).
    pub worst_path: Vec<(String, i64)>,
}

/// [`analyze`] with the HL10T delay model.
pub fn analyze_hl10t(routed: &Routed, period_ps: i64) -> Option<GraphTiming> {
    analyze(routed, &TimingModel::hl10t(), period_ps)
}

/// Analyse `routed` against a clock period. `None` when the design was not
/// routed from its full connection list.
pub fn analyze(routed: &Routed, tm: &TimingModel, period_ps: i64) -> Option<GraphTiming> {
    analyze_with_pad(routed, tm, period_ps, tm.iob_ps)
}

/// [`analyze`] with the output pad delay given (it depends on the I/O
/// standard, drive and termination of the design's pads).
pub fn analyze_with_pad(routed: &Routed, tm: &TimingModel, period_ps: i64, pad_ps: i64) -> Option<GraphTiming> {
    if !routed.tracks.active {
        return None;
    }
    let placed = &routed.placed;
    let packed = &placed.packed;
    let conn = &packed.conn;
    let n = packed.lutffs.len().min(placed.lutff_sites.len());
    // Pins of each cluster.
    let mut pins: Vec<Vec<(u8, NetSrc)>> = vec![Vec::new(); n];
    for p in &conn.pins {
        if (p.lutff as usize) < n {
            pins[p.lutff as usize].push((p.pin, p.src));
        }
    }
    // Hard blocks connected to the fabric: how each one's outputs are
    // timed, its cell, its site, and its connected inputs (dy, m, driver).
    let nh = conn.hard.len();
    let hout: Vec<HardOut> = conn
        .hard
        .iter()
        .map(|r| match r.kind {
            HardKind::Bram => HardOut::Ram,
            HardKind::Mac if packed.macs.get(r.idx as usize).is_some_and(|m| m.preg) => HardOut::Reg,
            HardKind::Mac => HardOut::Comb,
        })
        .collect();
    let hcell = |b: usize| -> &str {
        let r = conn.hard[b];
        match r.kind {
            HardKind::Bram => packed.brams.get(r.idx as usize).map(|c| c.cell.as_str()),
            HardKind::Mac => packed.macs.get(r.idx as usize).map(|c| c.cell.as_str()),
        }
        .unwrap_or("?")
    };
    let hsite: Vec<Option<(u32, u32)>> = (0..nh as u32).map(|b| placed.hard_block_site(b).map(|s| (s.x, s.y))).collect();
    let mut hpins: Vec<Vec<(u8, u8, NetSrc)>> = vec![Vec::new(); nh];
    for h in &conn.hard_pins {
        if (h.blk as usize) < nh {
            hpins[h.blk as usize].push((h.dy, h.m, h.src));
        }
    }
    let wire_hops: HashMap<(u32, u32, u32), u32> = routed.tracks.pin_hops.iter().map(|h| ((h.0, h.1, h.2), h.3)).collect();
    let pad_hops: HashMap<(u32, u32), u32> = routed.tracks.pad_hops.iter().map(|h| ((h.0, h.1), h.2)).collect();
    let site = |i: usize| placed.lutff_sites[i];
    let has_ff = |i: usize| !packed.lutffs[i].ff_cell.is_empty();
    // Tile a source leaves from.
    let src_tile = |src: NetSrc| -> Option<(u32, u32)> {
        match src {
            NetSrc::BleQ(j) | NetSrc::BleO(j) if (j as usize) < n => {
                let d = site(j as usize).0;
                Some((d.x, d.y))
            }
            NetSrc::Hard { blk, bit } => {
                let (x, y) = hsite.get(blk as usize).copied().flatten()?;
                let dy = helion_device::hard::output(conn.hard[blk as usize].kind, bit as u32).map(|o| o.0).unwrap_or(0);
                Some((x, y + dy))
            }
            _ => None,
        }
    };
    // Wires from `src` to the input `mux` of tile (x, y): as routed, else
    // the tile distance.
    let hops_to = |x: u32, y: u32, mux: u32, src: NetSrc| -> i64 {
        match wire_hops.get(&(x, y, mux)) {
            Some(&h) => h as i64,
            None => src_tile(src).map(|(sx, sy)| (sx.abs_diff(x) + sy.abs_diff(y)) as i64).unwrap_or(0),
        }
    };

    // Arrival at each cluster's LUT output (nodes 0..n) and at each
    // combinational multiplier's P (nodes n..n+nh), its LUT depth and its
    // worst fan-in.
    let mut arr: Vec<Option<(i64, u32, Option<NetSrc>)>> = vec![None; n + nh];
    let mut state = vec![0u8; n + nh];
    // Arrival of a source as seen at its output, and its depth.
    let src_arrival = |src: NetSrc, arr: &[Option<(i64, u32, Option<NetSrc>)>]| -> (i64, u32) {
        match src {
            NetSrc::BleQ(_) => (tm.ff_ckq_ps, 0),
            NetSrc::Input(_) => (tm.iob_ps, 0),
            NetSrc::BleO(j) => arr.get(j as usize).copied().flatten().map(|a| (a.0, a.1)).unwrap_or((0, 0)),
            NetSrc::Hard { blk, .. } => match hout.get(blk as usize) {
                Some(HardOut::Ram) => (tm.bram_ckq_ps, 0),
                Some(HardOut::Reg) => (tm.ff_ckq_ps, 0),
                Some(HardOut::Comb) => arr.get(n + blk as usize).copied().flatten().map(|a| (a.0, a.1)).unwrap_or((0, 0)),
                None => (tm.ff_ckq_ps, 0),
            },
        }
    };
    // The node a source's arrival is computed at, if any.
    let node_of = |src: NetSrc| -> Option<usize> {
        match src {
            NetSrc::BleO(j) if (j as usize) < n => Some(j as usize),
            NetSrc::Hard { blk, .. } if hout.get(blk as usize) == Some(&HardOut::Comb) => Some(n + blk as usize),
            _ => None,
        }
    };
    // Inputs of a multiplier that go through its product (not CE / RST).
    let through_product = |dy: u8, m: u8| dy > 0 || u32::from(m) < 2 * helion_device::hard::MAC_IN_BITS;
    let deps = |u: usize| -> Vec<NetSrc> {
        if u < n {
            pins[u].iter().map(|p| p.1).collect()
        } else {
            hpins[u - n].iter().filter(|p| through_product(p.0, p.1)).map(|p| p.2).collect()
        }
    };
    for root in 0..n + nh {
        if state[root] == 2 || (root >= n && hout[root - n] != HardOut::Comb) {
            continue;
        }
        let mut stack = vec![root];
        while let Some(&i) = stack.last() {
            if state[i] == 2 {
                stack.pop();
                continue;
            }
            state[i] = 1;
            let mut ready = true;
            for src in deps(i) {
                if let Some(j) = node_of(src) {
                    if state[j] == 0 {
                        stack.push(j);
                        ready = false;
                    }
                    // state 1: a loop; treated as already settled.
                }
            }
            if !ready {
                continue;
            }
            let mut best: (i64, u32, Option<NetSrc>) = (0, 0, None);
            if i < n {
                let (sx, sy, sble) = (site(i).0.x, site(i).0.y, site(i).1);
                for &(pin, src) in &pins[i] {
                    let (a, depth) = src_arrival(src, &arr);
                    let t = a + hops_to(sx, sy, sble as u32 * 8 + pin as u32, src) * tm.hop_ps;
                    if best.2.is_none() || t > best.0 {
                        best = (t, depth, Some(src));
                    }
                }
                // The model charges a LUT its input loading per connected pin.
                let load = pins[i].len() as i64 * tm.pin_ps;
                arr[i] = Some((best.0 + load + tm.lut_ps, best.1 + 1, best.2));
            } else if let Some((x, y)) = hsite[i - n] {
                for &(dy, m, src) in hpins[i - n].iter().filter(|p| through_product(p.0, p.1)) {
                    let (a, depth) = src_arrival(src, &arr);
                    let t = a + hops_to(x, y + dy as u32, m as u32, src) * tm.hop_ps;
                    if best.2.is_none() || t > best.0 {
                        best = (t, depth, Some(src));
                    }
                }
                arr[i] = Some((best.0 + tm.mac_ps, best.1, best.2));
            }
            state[i] = 2;
            stack.pop();
        }
    }

    let mut out = GraphTiming::default();
    // (arrival incl. setup or pad, description of the endpoint, driving
    // source, a step between the source and the endpoint)
    type Worst = Option<(i64, String, NetSrc, Option<(String, i64)>)>;
    let mut worst: Worst = None;
    let note = |t: i64, what: String, src: NetSrc, step: Option<(String, i64)>, out: &mut GraphTiming, worst: &mut Worst| {
        out.endpoints += 1;
        if t > period_ps {
            out.failing += 1;
            out.tns_ps += period_ps - t;
        }
        if worst.as_ref().map(|w| t > w.0).unwrap_or(true) {
            *worst = Some((t, what, src, step));
        }
    };
    for i in 0..n {
        let Some((a, depth, _)) = arr[i] else { continue };
        out.levels = out.levels.max(depth);
        if has_ff(i) {
            let t = a + tm.setup_ps;
            out.reg_ps = out.reg_ps.max(t);
            note(t, format!("{} (setup)", packed.lutffs[i].ff_cell), NetSrc::BleO(i as u32), None, &mut out, &mut worst);
        }
    }
    // Block inputs a clock edge latches: every input of a RAM (data,
    // addresses and enables, for its write and its registered read), and
    // every input of a registered multiplier, A/B/C through the product.
    for b in 0..nh {
        if hout[b] == HardOut::Comb {
            continue;
        }
        let Some((x, y)) = hsite[b] else { continue };
        let kind = conn.hard[b].kind;
        let names: HashMap<(u32, u32), String> = helion_device::hard::inputs(kind).into_iter().map(|(p, dy, m)| ((dy, m), p)).collect();
        for &(dy, m, src) in &hpins[b] {
            let (a, depth) = src_arrival(src, &arr);
            out.levels = out.levels.max(depth);
            let at_pin = a + hops_to(x, y + dy as u32, m as u32, src) * tm.hop_ps;
            let pin = names.get(&(dy as u32, m as u32)).cloned().unwrap_or_else(|| format!("{dy}/{m}"));
            let (t, step) = if kind == HardKind::Mac && through_product(dy, m) {
                let p = at_pin + tm.mac_ps;
                (p + tm.setup_ps, Some((format!("{} (multiplier, {pin} to the P register)", hcell(b)), p)))
            } else {
                (at_pin + tm.setup_ps, None)
            };
            out.reg_ps = out.reg_ps.max(t);
            note(t, format!("{} {pin} (setup)", hcell(b)), src, step, &mut out, &mut worst);
        }
    }
    for (ii, io) in packed.iobs.iter().enumerate() {
        let (Some(&src), Some(pad)) = (conn.iob_src.get(&io.cell), placed.iob_sites.get(ii)) else { continue };
        let (a, _) = match src {
            // A registered output leaves from the flop.
            NetSrc::BleO(j) if (j as usize) < n && has_ff(j as usize) && packed.lutffs[j as usize].q_net == io.from_net => (tm.ff_ckq_ps, 0),
            s => src_arrival(s, &arr),
        };
        let hops = match pad_hops.get(&(pad.x, pad.y)) {
            Some(&h) => h as i64,
            None => match src {
                NetSrc::BleQ(j) | NetSrc::BleO(j) if (j as usize) < n => site(j as usize).0.y.abs_diff(pad.y) as i64,
                _ => 0,
            },
        };
        let t = a + hops * tm.hop_ps + pad_ps;
        out.out_ps = out.out_ps.max(t);
        note(t, format!("pad {}", io.cell), src, None, &mut out, &mut worst);
    }
    // Trace the slowest path back to its start.
    if let Some((t, what, mut src, step)) = worst {
        let mut path = vec![(what, t)];
        path.extend(step);
        for _ in 0..4096 {
            match src {
                NetSrc::BleQ(j) => {
                    path.push((format!("{} (clock to Q)", packed.lutffs.get(j as usize).map(|l| l.ff_cell.as_str()).unwrap_or("?")), tm.ff_ckq_ps));
                    break;
                }
                NetSrc::Input(k) => {
                    path.push((format!("input {}", conn.inputs.get(k as usize).map(|i| i.net.as_str()).unwrap_or("?")), tm.iob_ps));
                    break;
                }
                NetSrc::Hard { blk, bit } => {
                    let b = blk as usize;
                    match hout.get(b) {
                        Some(HardOut::Comb) => {
                            let Some(Some((a, _, prev))) = arr.get(n + b) else { break };
                            path.push((format!("{} (multiplier, to P{bit})", hcell(b)), *a));
                            match prev {
                                Some(p) => src = *p,
                                None => break,
                            }
                        }
                        Some(HardOut::Ram) => {
                            path.push((format!("{} (clock to DO{bit})", hcell(b)), tm.bram_ckq_ps));
                            break;
                        }
                        _ => {
                            let cell = if b < nh { hcell(b) } else { "?" };
                            path.push((format!("{cell} (clock to P{bit})"), tm.ff_ckq_ps));
                            break;
                        }
                    }
                }
                NetSrc::BleO(j) => {
                    let Some(Some((a, _, prev))) = arr.get(j as usize) else { break };
                    path.push((packed.lutffs[j as usize].lut_cell.clone(), *a));
                    match prev {
                        Some(p) => src = *p,
                        None => break,
                    }
                }
            }
        }
        path.reverse();
        out.worst_path = path;
    }
    Some(out)
}

/// How a hard block's outputs are timed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HardOut {
    /// A RAM: clock to DO.
    Ram,
    /// A registered multiplier: clock to P.
    Reg,
    /// A combinational multiplier: its inputs through the product.
    Comb,
}
