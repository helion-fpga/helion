//! Timing over the real logic graph of a routed design.
//!
//! The first-generation analysis charged every path one LUT. This walks
//! the packed connection list instead: a path runs from a flop or an input
//! pad through as many LUTs as the netlist has, and each connection costs
//! what it was routed as: nothing extra inside a CLB, one hop per tile for
//! a direct neighbour connection, one hop per wire on the interconnect.

use helion_backend::TimingModel;
use helion_pack::NetSrc;
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
    /// Every endpoint analysed: its cell, the arrival it must meet the
    /// period with (setup or pad delay included) and its site's tile.
    pub endpoint_arrivals: Vec<EndpointArrival>,
}

/// One analysed endpoint: a flop data input or an output pad.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointArrival {
    /// Flop cell, or the pad's IOB cell.
    pub cell: String,
    pub arrival_ps: i64,
    pub x: u32,
    pub y: u32,
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
    let wire_hops: HashMap<(u32, u32, u32), u32> = routed.tracks.pin_hops.iter().map(|h| ((h.0, h.1, h.2), h.3)).collect();
    let pad_hops: HashMap<(u32, u32), u32> = routed.tracks.pad_hops.iter().map(|h| ((h.0, h.1), h.2)).collect();
    let site = |i: usize| placed.lutff_sites[i];
    let has_ff = |i: usize| !packed.lutffs[i].ff_cell.is_empty();

    // Arrival at each cluster's LUT output, its LUT depth and its worst fan-in.
    let mut arr: Vec<Option<(i64, u32, Option<NetSrc>)>> = vec![None; n];
    let mut state = vec![0u8; n];
    // Arrival of a source as seen at its output, and its depth.
    fn src_arrival(src: NetSrc, arr: &[Option<(i64, u32, Option<NetSrc>)>], tm: &TimingModel) -> (i64, u32) {
        match src {
            NetSrc::BleQ(_) => (tm.ff_ckq_ps, 0),
            NetSrc::Input(_) => (tm.iob_ps, 0),
            NetSrc::BleO(j) => arr.get(j as usize).copied().flatten().map(|a| (a.0, a.1)).unwrap_or((0, 0)),
        }
    }
    for root in 0..n {
        if state[root] == 2 {
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
            for &(_, src) in &pins[i] {
                if let NetSrc::BleO(j) = src {
                    let j = j as usize;
                    if j < n && state[j] == 0 {
                        stack.push(j);
                        ready = false;
                    }
                    // state 1: a loop through LUTs; treated as already settled.
                }
            }
            if !ready {
                continue;
            }
            let (sx, sy, sble) = (site(i).0.x, site(i).0.y, site(i).1);
            let mut best: (i64, u32, Option<NetSrc>) = (0, 0, None);
            for &(pin, src) in &pins[i] {
                let (a, depth) = src_arrival(src, &arr, tm);
                let mux = sble as u32 * 8 + pin as u32;
                let hops = match wire_hops.get(&(sx, sy, mux)) {
                    Some(&h) => h as i64,
                    None => match src {
                        NetSrc::BleQ(j) | NetSrc::BleO(j) if (j as usize) < n => {
                            let d = site(j as usize).0;
                            (d.x.abs_diff(sx) + d.y.abs_diff(sy)) as i64
                        }
                        _ => 0,
                    },
                };
                let t = a + hops * tm.hop_ps;
                if best.2.is_none() || t > best.0 {
                    best = (t, depth, Some(src));
                }
            }
            // The model charges a LUT its input loading per connected pin.
            let load = pins[i].len() as i64 * tm.pin_ps;
            arr[i] = Some((best.0 + load + tm.lut_ps, best.1 + 1, best.2));
            state[i] = 2;
            stack.pop();
        }
    }

    let mut out = GraphTiming::default();
    // (arrival incl. setup or pad, description of the endpoint, driving source)
    let mut worst: Option<(i64, String, NetSrc)> = None;
    let mut note = |t: i64, what: String, src: NetSrc, out: &mut GraphTiming, worst: &mut Option<(i64, String, NetSrc)>| {
        out.endpoints += 1;
        if t > period_ps {
            out.failing += 1;
            out.tns_ps += period_ps - t;
        }
        if worst.as_ref().map(|w| t > w.0).unwrap_or(true) {
            *worst = Some((t, what, src));
        }
    };
    for i in 0..n {
        let Some((a, depth, _)) = arr[i] else { continue };
        out.levels = out.levels.max(depth);
        if has_ff(i) {
            let t = a + tm.setup_ps;
            out.reg_ps = out.reg_ps.max(t);
            out.endpoint_arrivals.push(EndpointArrival { cell: packed.lutffs[i].ff_cell.clone(), arrival_ps: t, x: site(i).0.x, y: site(i).0.y });
            note(t, format!("{} (setup)", packed.lutffs[i].ff_cell), NetSrc::BleO(i as u32), &mut out, &mut worst);
        }
    }
    for (ii, io) in packed.iobs.iter().enumerate() {
        let (Some(&src), Some(pad)) = (conn.iob_src.get(&io.cell), placed.iob_sites.get(ii)) else { continue };
        let (a, _) = match src {
            // A registered output leaves from the flop.
            NetSrc::BleO(j) if (j as usize) < n && has_ff(j as usize) && packed.lutffs[j as usize].q_net == io.from_net => (tm.ff_ckq_ps, 0),
            s => src_arrival(s, &arr, tm),
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
        out.endpoint_arrivals.push(EndpointArrival { cell: io.cell.clone(), arrival_ps: t, x: pad.x, y: pad.y });
        note(t, format!("pad {}", io.cell), src, &mut out, &mut worst);
    }
    // Trace the slowest path back to its start.
    if let Some((t, what, mut src)) = worst {
        let mut path = vec![(what, t)];
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
