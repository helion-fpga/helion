//! LUT re-mapping: cover the mapped LUT network again with 6-input cuts.
//!
//! Front ends often emit one small LUT per source operator. This pass treats
//! the LUT netlist as a Boolean network and re-covers it: every LUT gets a
//! set of cuts (at most six nets that determine its output), the cover is
//! chosen for minimum depth and then for area on paths with slack, and the
//! function of each chosen cut becomes one LUT. LUTs no longer needed
//! disappear.
//!
//! Unchanged by construction: LUTs marked `DONT_TOUCH`, LUTs whose output is
//! `mark_debug`, ILA-observed or a top-level port net, and LUTs in a
//! combinational loop. A LUT on a `Carry4` propagate pin stays a LUT of at
//! most four inputs (its second output carries the generate). Cell and net names of surviving LUTs are kept.
//! An unconnected LUT input reads 0, as everywhere in this IR.

use std::collections::HashMap;

use crate::{CellKind, Design, Endpoint};

/// Result of [`Design::remap_luts`].
#[derive(Clone, Copy, Debug, Default)]
pub struct RemapStats {
    pub luts_before: usize,
    pub luts_after: usize,
    pub depth_before: u32,
    pub depth_after: u32,
}

const K: usize = 6;
const CARRY_K: usize = 4;
/// Cuts kept per LUT.
const KEEP: usize = 10;
/// Partial leaf sets kept while merging one LUT's inputs.
const PARTIAL: usize = 48;
const VAR: [u64; 6] = [
    0xAAAA_AAAA_AAAA_AAAA,
    0xCCCC_CCCC_CCCC_CCCC,
    0xF0F0_F0F0_F0F0_F0F0,
    0xFF00_FF00_FF00_FF00,
    0xFFFF_0000_FFFF_0000,
    0xFFFF_FFFF_0000_0000,
];

#[derive(Clone, Debug)]
struct Cut {
    leaves: Vec<u32>, // net indices, sorted
    depth: u32,
    flow: f32,
}

struct Node {
    cell: usize,
    out: u32,
    init: u64,
    ins: [Option<u32>; 6],
    fixed: bool,
    /// Most inputs this LUT may have after re-mapping.
    k: usize,
}

/// `init` applied bitwise to six input words.
fn apply(init: u64, w: &[u64; 6]) -> u64 {
    let mut out = 0u64;
    for j in 0..64 {
        let mut addr = 0usize;
        for (i, wi) in w.iter().enumerate() {
            addr |= (((wi >> j) & 1) as usize) << i;
        }
        out |= ((init >> addr) & 1) << j;
    }
    out
}

/// Whether the truth table depends on variable `i`.
fn depends(tt: u64, i: usize) -> bool {
    let s = 1u32 << i;
    ((tt >> s) & !VAR[i]) != (tt & !VAR[i])
}

/// Drop variable `i` (which `tt` does not depend on), shifting higher
/// variables down by one.
fn drop_var(tt: u64, i: usize) -> u64 {
    let mut out = 0u64;
    for addr in 0..64usize {
        let low = addr & ((1 << i) - 1);
        let high = (addr >> i) << (i + 1);
        out |= ((tt >> ((high | low) & 63)) & 1) << addr;
    }
    out
}

impl Design {
    /// Re-cover the LUT network with 6-input cuts. See the module docs.
    pub fn remap_luts(&mut self) -> RemapStats {
        self.rebuild_indexes();
        let mut stats = RemapStats::default();
        let cell_ix: HashMap<&str, usize> = self.cells.iter().enumerate().map(|(i, c)| (c.name.as_str(), i)).collect();
        let n_nets = self.nets.len();
        let ila: std::collections::HashSet<&str> = self
            .cells
            .iter()
            .filter_map(|c| match &c.kind {
                CellKind::Ila { net } => Some(net.as_str()),
                _ => None,
            })
            .collect();

        // LUT pins and net usage.
        let mut out_of: HashMap<usize, u32> = HashMap::new();
        let mut ins_of: HashMap<usize, [Option<u32>; 6]> = HashMap::new();
        let mut root: Vec<bool> = vec![false; n_nets]; // read by something other than a LUT input
        let mut to_carry: Vec<bool> = vec![false; n_nets];
        let mut fanout: Vec<u32> = vec![0; n_nets];
        for (ni, n) in self.nets.iter().enumerate() {
            if n.attrs.flag("mark_debug") || ila.contains(n.name.as_str()) || self.is_output_port_bit_net(&n.name) {
                root[ni] = true;
            }
            for e in &n.endpoints {
                let Some(&ci) = cell_ix.get(e.cell.as_str()) else {
                    root[ni] = true;
                    continue;
                };
                let is_lut = matches!(self.cells[ci].kind, CellKind::Lut6 { .. });
                let pin_idx = e.pin.strip_prefix('I').and_then(|d| d.parse::<usize>().ok()).filter(|i| *i < 6);
                match (is_lut, e.pin.as_str(), pin_idx) {
                    (true, "O", _) => {
                        out_of.insert(ci, ni as u32);
                    }
                    (true, _, Some(i)) => {
                        ins_of.entry(ci).or_insert([None; 6])[i] = Some(ni as u32);
                        fanout[ni] += 1;
                    }
                    _ => {
                        root[ni] = true;
                        fanout[ni] += 1;
                        // The LUT on a carry propagate pin shares its
                        // site with the carry: it keeps its shape.
                        if matches!(self.cells[ci].kind, CellKind::Carry4) && e.pin.starts_with('S') {
                            to_carry[ni] = true;
                        }
                    }
                }
            }
        }

        // Nodes.
        let mut nodes: Vec<Node> = Vec::new();
        let mut node_of_net: Vec<u32> = vec![u32::MAX; n_nets];
        for (ci, c) in self.cells.iter().enumerate() {
            let CellKind::Lut6 { init } = c.kind else { continue };
            let Some(&out) = out_of.get(&ci) else { continue };
            let fixed = c.attrs.flag("DONT_TOUCH");
            // A carry propagate LUT also produces the generate on its
            // second output: four inputs for the propagate leave the fifth
            // for a generate that is not among them.
            let k = if to_carry[out as usize] { CARRY_K } else { K };
            node_of_net[out as usize] = nodes.len() as u32;
            nodes.push(Node { cell: ci, out, init, ins: ins_of.get(&ci).copied().unwrap_or([None; 6]), fixed, k });
        }
        stats.luts_before = nodes.len();
        stats.luts_after = nodes.len();
        if nodes.is_empty() {
            return stats;
        }

        // Topological order (Kahn). Nodes left over sit in or behind a loop.
        let nn = nodes.len();
        let mut indeg: Vec<u32> = vec![0; nn];
        let mut users: Vec<Vec<u32>> = vec![Vec::new(); nn];
        for (i, nd) in nodes.iter().enumerate() {
            for f in nd.ins.iter().flatten() {
                let d = node_of_net[*f as usize];
                if d != u32::MAX {
                    indeg[i] += 1;
                    users[d as usize].push(i as u32);
                }
            }
        }
        let mut order: Vec<u32> = Vec::with_capacity(nn);
        let mut queue: Vec<u32> = (0..nn as u32).filter(|i| indeg[*i as usize] == 0).collect();
        while let Some(i) = queue.pop() {
            order.push(i);
            for &u in &users[i as usize] {
                indeg[u as usize] -= 1;
                if indeg[u as usize] == 0 {
                    queue.push(u);
                }
            }
        }
        let mut topo_pos: Vec<u32> = vec![u32::MAX; nn];
        for (p, &i) in order.iter().enumerate() {
            topo_pos[i as usize] = p as u32;
        }
        for (i, nd) in nodes.iter_mut().enumerate() {
            if topo_pos[i] == u32::MAX {
                nd.fixed = true;
            }
        }
        // A fixed LUT keeps its inputs, so their drivers must survive.
        for nd in &nodes {
            if nd.fixed {
                root[nd.out as usize] = true;
                for f in nd.ins.iter().flatten() {
                    root[*f as usize] = true;
                }
            }
        }

        // Cut enumeration in topological order.
        let mut net_depth: Vec<u32> = vec![0; n_nets];
        let mut net_flow: Vec<f32> = vec![0.0; n_nets];
        let mut old_depth: Vec<u32> = vec![0; n_nets];
        let mut cuts: Vec<Vec<Cut>> = vec![Vec::new(); nn];
        for &i in &order {
            let nd = &nodes[i as usize];
            let mut fanins: Vec<u32> = nd.ins.iter().flatten().copied().collect();
            fanins.sort();
            fanins.dedup();
            old_depth[nd.out as usize] = 1 + fanins.iter().map(|f| old_depth[*f as usize]).max().unwrap_or(0);
            let cost = |leaves: &[u32], net_depth: &[u32], net_flow: &[f32]| -> (u32, f32) {
                let depth = 1 + leaves.iter().map(|l| net_depth[*l as usize]).max().unwrap_or(0);
                let flow = 1.0 + leaves.iter().map(|l| net_flow[*l as usize] / fanout[*l as usize].max(1) as f32).sum::<f32>();
                (depth, flow)
            };
            let mut list: Vec<Cut> = Vec::new();
            let (d0, f0) = cost(&fanins, &net_depth, &net_flow);
            list.push(Cut { leaves: fanins.clone(), depth: d0, flow: f0 });
            if !nd.fixed {
                let mut partial: Vec<Vec<u32>> = vec![Vec::new()];
                for f in &fanins {
                    let mut options: Vec<&[u32]> = vec![std::slice::from_ref(f)];
                    let d = node_of_net[*f as usize];
                    // A net that must exist anyway (a root) stays a leaf
                    // candidate, but its cone may still be duplicated here.
                    if d != u32::MAX && !nodes[d as usize].fixed && topo_pos[d as usize] != u32::MAX {
                        for c in &cuts[d as usize] {
                            options.push(&c.leaves);
                        }
                    }
                    let mut next: Vec<Vec<u32>> = Vec::new();
                    for p in &partial {
                        for o in &options {
                            let mut m = p.clone();
                            for l in o.iter() {
                                if let Err(pos) = m.binary_search(l) {
                                    m.insert(pos, *l);
                                }
                            }
                            if m.len() <= nd.k {
                                next.push(m);
                            }
                        }
                    }
                    next.sort();
                    next.dedup();
                    if next.len() > PARTIAL {
                        next.sort_by(|a, b| {
                            let (da, fa) = cost(a, &net_depth, &net_flow);
                            let (db, fb) = cost(b, &net_depth, &net_flow);
                            da.cmp(&db).then(fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal)).then(a.cmp(b))
                        });
                        next.truncate(PARTIAL);
                    }
                    partial = next;
                }
                for leaves in partial {
                    if leaves == fanins {
                        continue;
                    }
                    let (depth, flow) = cost(&leaves, &net_depth, &net_flow);
                    list.push(Cut { leaves, depth, flow });
                }
                let own = list.remove(0);
                list.sort_by(|a, b| a.depth.cmp(&b.depth).then(a.flow.partial_cmp(&b.flow).unwrap_or(std::cmp::Ordering::Equal)).then(a.leaves.cmp(&b.leaves)));
                list.truncate(KEEP - 1);
                list.push(own);
            }
            let best = list
                .iter()
                .min_by(|a, b| a.depth.cmp(&b.depth).then(a.flow.partial_cmp(&b.flow).unwrap_or(std::cmp::Ordering::Equal)))
                .expect("every LUT has its own cut");
            net_depth[nd.out as usize] = best.depth;
            net_flow[nd.out as usize] = best.flow;
            cuts[i as usize] = list;
        }

        // Cover: required depth flows from the roots towards the inputs.
        let dmax = nodes.iter().filter(|n| root[n.out as usize]).map(|n| net_depth[n.out as usize]).max().unwrap_or(0);
        stats.depth_before = nodes.iter().filter(|n| root[n.out as usize]).map(|n| old_depth[n.out as usize]).max().unwrap_or(0);
        stats.depth_after = dmax;
        let mut req: Vec<u32> = vec![u32::MAX; nn];
        for (i, nd) in nodes.iter().enumerate() {
            if root[nd.out as usize] && topo_pos[i] != u32::MAX {
                req[i] = dmax.max(net_depth[nd.out as usize]);
            }
        }
        let mut chosen: Vec<Option<usize>> = vec![None; nn];
        for &i in order.iter().rev() {
            let r = req[i as usize];
            if r == u32::MAX {
                continue;
            }
            let nd = &nodes[i as usize];
            let list = &cuts[i as usize];
            let pick = if nd.fixed {
                list.len() - 1
            } else {
                (0..list.len())
                    .filter(|c| list[*c].depth <= r)
                    .min_by(|a, b| {
                        let (ca, cb) = (&list[*a], &list[*b]);
                        ca.flow.partial_cmp(&cb.flow).unwrap_or(std::cmp::Ordering::Equal).then(ca.leaves.len().cmp(&cb.leaves.len())).then(a.cmp(b))
                    })
                    .unwrap_or(list.len() - 1)
            };
            chosen[i as usize] = Some(pick);
            for l in &list[pick].leaves {
                let d = node_of_net[*l as usize];
                if d != u32::MAX && topo_pos[d as usize] != u32::MAX {
                    let want = r.saturating_sub(1).max(net_depth[*l as usize]);
                    if req[d as usize] == u32::MAX || want < req[d as usize] {
                        req[d as usize] = want;
                    }
                }
            }
        }

        // New function and inputs of every kept LUT.
        let mut new_lut: HashMap<usize, (u64, Vec<u32>)> = HashMap::new(); // cell -> (init, leaf nets)
        let mut removed: Vec<usize> = Vec::new();
        let mut value: HashMap<u32, u64> = HashMap::new();
        let mut cone: Vec<u32> = Vec::new();
        let mut stack: Vec<u32> = Vec::new();
        for (i, nd) in nodes.iter().enumerate() {
            if nd.fixed || topo_pos[i] == u32::MAX {
                continue;
            }
            let Some(pick) = chosen[i] else {
                removed.push(nd.cell);
                continue;
            };
            // Its own inputs: the LUT stays exactly as it is.
            if pick == cuts[i].len() - 1 {
                continue;
            }
            let leaves = &cuts[i][pick].leaves;
            value.clear();
            for (v, l) in leaves.iter().enumerate() {
                value.insert(*l, VAR[v]);
            }
            // Cone of LUTs between the leaves and this output.
            cone.clear();
            stack.clear();
            stack.push(i as u32);
            let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
            seen.insert(i as u32);
            while let Some(x) = stack.pop() {
                cone.push(x);
                for f in nodes[x as usize].ins.iter().flatten() {
                    if value.contains_key(f) {
                        continue;
                    }
                    let d = node_of_net[*f as usize];
                    if d != u32::MAX && seen.insert(d) {
                        stack.push(d);
                    }
                }
            }
            cone.sort_by_key(|x| topo_pos[*x as usize]);
            for &x in &cone {
                let c = &nodes[x as usize];
                let mut w = [0u64; 6];
                for (k, f) in c.ins.iter().enumerate() {
                    if let Some(f) = f {
                        w[k] = value.get(f).copied().unwrap_or(0);
                    }
                }
                value.insert(c.out, apply(c.init, &w));
            }
            let mut tt = value[&nd.out];
            let mut kept: Vec<u32> = leaves.clone();
            let mut v = 0;
            while v < kept.len() {
                if depends(tt, v) {
                    v += 1;
                } else {
                    tt = drop_var(tt, v);
                    kept.remove(v);
                }
            }
            // Variables above the kept ones repeat the pattern.
            new_lut.insert(nd.cell, (tt, kept));
        }
        if new_lut.is_empty() && removed.is_empty() {
            return stats;
        }

        // Rewrite the netlist.
        let rewired: HashMap<&str, usize> = new_lut.keys().map(|ci| (self.cells[*ci].name.as_str(), *ci)).collect();
        let gone: std::collections::HashSet<&str> = removed.iter().map(|ci| self.cells[*ci].name.as_str()).collect();
        let mut keep_eps: Vec<Vec<bool>> = Vec::with_capacity(n_nets);
        for n in &self.nets {
            keep_eps.push(
                n.endpoints
                    .iter()
                    .map(|e| !gone.contains(e.cell.as_str()) && !(rewired.contains_key(e.cell.as_str()) && e.pin != "O"))
                    .collect(),
            );
        }
        let mut adds: Vec<(u32, String, String)> = Vec::new();
        for (ci, (_, leaves)) in &new_lut {
            for (k, l) in leaves.iter().enumerate() {
                adds.push((*l, self.cells[*ci].name.clone(), format!("I{k}")));
            }
        }
        adds.sort();
        let inits: Vec<(usize, u64)> = new_lut.iter().map(|(ci, (tt, _))| (*ci, *tt)).collect();
        for (n, keep) in self.nets.iter_mut().zip(keep_eps) {
            let mut it = keep.into_iter();
            n.endpoints.retain(|_| it.next().unwrap_or(true));
        }
        for (ni, cell, pin) in adds {
            self.nets[ni as usize].endpoints.push(Endpoint { cell, pin });
        }
        for (ci, tt) in inits {
            self.cells[ci].kind = CellKind::Lut6 { init: tt };
        }
        let dead: std::collections::HashSet<usize> = removed.iter().copied().collect();
        let mut idx = 0usize;
        self.cells.retain(|_| {
            let keep = !dead.contains(&idx);
            idx += 1;
            keep
        });
        self.nets.retain(|n| !n.endpoints.is_empty());
        self.rebuild_indexes();
        stats.luts_after = stats.luts_before - dead.len();
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PortDir;

    /// y = (a & b) ^ (c | d) built from three 2-input LUTs feeding a flop.
    #[test]
    fn three_small_luts_become_one() {
        let mut d = Design::new("t");
        for p in ["a", "b", "c", "d"] {
            d.add_port(p, PortDir::In);
        }
        d.add_cell("l_and", CellKind::Lut6 { init: 0x8888_8888_8888_8888 });
        d.add_cell("l_or", CellKind::Lut6 { init: 0xEEEE_EEEE_EEEE_EEEE });
        d.add_cell("l_xor", CellKind::Lut6 { init: 0x6666_6666_6666_6666 });
        d.add_cell("ff", CellKind::Hff);
        d.connect("a", "l_and", "I0");
        d.connect("b", "l_and", "I1");
        d.connect("n1", "l_and", "O");
        d.connect("c", "l_or", "I0");
        d.connect("d", "l_or", "I1");
        d.connect("n2", "l_or", "O");
        d.connect("n1", "l_xor", "I0");
        d.connect("n2", "l_xor", "I1");
        d.connect("y", "l_xor", "O");
        d.connect("y", "ff", "D");
        let st = d.remap_luts();
        assert_eq!((st.luts_before, st.luts_after), (3, 1));
        assert_eq!((st.depth_before, st.depth_after), (2, 1));
        let CellKind::Lut6 { init } = d.cell("l_xor").unwrap().kind else { panic!() };
        let pin = |p: &str| d.net_on("l_xor", p).unwrap().to_string();
        let order: Vec<String> = (0..4).map(|i| pin(&format!("I{i}"))).collect();
        for addr in 0..16u64 {
            let val = |name: &str| (addr >> order.iter().position(|o| o == name).unwrap()) & 1;
            let want = (val("a") & val("b")) ^ (val("c") | val("d"));
            assert_eq!((init >> addr) & 1, want, "addr {addr}");
        }
    }

    #[test]
    fn carry_propagate_lut_stays_a_lut() {
        let mut d = Design::new("t");
        d.add_port("a", PortDir::In);
        d.add_cell("l_inv", CellKind::Lut6 { init: 0x5555_5555_5555_5555 });
        d.add_cell("l_s", CellKind::Lut6 { init: 0xAAAA_AAAA_AAAA_AAAA });
        d.add_cell("c4", CellKind::Carry4);
        d.connect("a", "l_inv", "I0");
        d.connect("n", "l_inv", "O");
        d.connect("n", "l_s", "I0");
        d.connect("s", "l_s", "O");
        d.connect("s", "c4", "S0");
        d.remap_luts();
        // The inverter may be absorbed, but the propagate LUT still drives S0.
        assert_eq!(d.net_on("c4", "S0"), Some("s"));
        assert!(matches!(d.cell("l_s").map(|c| &c.kind), Some(CellKind::Lut6 { .. })));
    }
}
