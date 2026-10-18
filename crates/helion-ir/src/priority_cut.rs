//! Priority-Cut 6-Feasible $K$-LUT Technology Mapper (`map_priority_cuts_lut6`)
//! and 4-Input NPN Boolean Equivalence Canonicalization (`npn4_canonical`).
//!
//! Implements:
//! 1. **4-Input NPN Equivalence Canonicalization (`npn4_canonical`)**:
//!    Computes the lexicographically minimal 16-bit truth table under all
//!    $2^4 \times 4! \times 2 = 768$ input-negation, input-permutation, and
//!    output-negation transformations, returning the exact [`Npn4Transform`].
//! 2. **Bottom-Up $K \le 6$ Priority-Cut Enumeration (`PriorityCut`)**:
//!    Enumerates 6-feasible cuts across arbitrary combinational LUT networks,
//!    composing 64-bit truth tables via parallel 64-bit Shannon cofactor
//!    operations and pruning redundant don't-care leaves.
//! 3. **Three-Pass Delay & Area Optimization (`map_priority_cuts_lut6`)**:
//!    - Pass 1 (Forward Delay Minimization): computes optimal arrival times.
//!    - Pass 2 (Backward Required Times + Area-Flow Recovery): selects timing-feasible
//!      cuts minimizing global area flow without increasing critical LUT depth.
//!    - Pass 3 (Exact Local Area / MFFC Recovery): refines cut selection using
//!      reference-counted local cone area and materializes the mapped `Lut6` netlist.

use std::collections::{HashMap, HashSet};

use lasso::Spur;

use crate::{intern, resolve, CellKind, Design, PortDir};

/// Elementary 64-bit truth tables for variables `I0..I5` of a 6-input LUT.
pub const LUT6_VAR_INIT: [u64; 6] = [
    0xAAAA_AAAA_AAAA_AAAA,
    0xCCCC_CCCC_CCCC_CCCC,
    0xF0F0_F0F0_F0F0_F0F0,
    0xFF00_FF00_FF00_FF00,
    0xFFFF_0000_FFFF_0000,
    0xFFFF_FFFF_0000_0000,
];

/// Low cofactor bitmasks for each of the 6 LUT input positions.
const COFACTOR_LO_MASK: [u64; 6] = [
    0x5555_5555_5555_5555,
    0x3333_3333_3333_3333,
    0x0F0F_0F0F_0F0F_0F0F,
    0x00FF_00FF_00FF_00FF,
    0x0000_FFFF_0000_FFFF,
    0x0000_0000_FFFF_FFFF,
];

/// All $4! = 24$ permutations of `[0, 1, 2, 3]`.
const PERMS_4: [[u8; 4]; 24] = [
    [0, 1, 2, 3],
    [0, 1, 3, 2],
    [0, 2, 1, 3],
    [0, 2, 3, 1],
    [0, 3, 1, 2],
    [0, 3, 2, 1],
    [1, 0, 2, 3],
    [1, 0, 3, 2],
    [1, 2, 0, 3],
    [1, 2, 3, 0],
    [1, 3, 0, 2],
    [1, 3, 2, 0],
    [2, 0, 1, 3],
    [2, 0, 3, 1],
    [2, 1, 0, 3],
    [2, 1, 3, 0],
    [2, 3, 0, 1],
    [2, 3, 1, 0],
    [3, 0, 1, 2],
    [3, 0, 2, 1],
    [3, 1, 0, 2],
    [3, 1, 2, 0],
    [3, 2, 0, 1],
    [3, 2, 1, 0],
];

/// Canonical 4-input NPN equivalence class representative and transformation witness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Npn4Transform {
    /// Lexicographically smallest 16-bit truth table in the NPN equivalence class.
    pub canonical: u16,
    /// Input permutation `perm[new_input] = old_input` ($0..4$).
    pub perm: [u8; 4],
    /// 4-bit input negation mask (`(in_neg >> old_input) & 1 != 0`).
    pub in_neg: u8,
    /// True if the output is negated relative to the transformed truth table.
    pub out_neg: bool,
}

/// Apply a 4-input NPN transformation `(perm, in_neg, out_neg)` to a 16-bit truth table `tt`.
///
/// For each `new_addr` $\in 0..16$, input `new_i` is mapped to `old_i = perm[new_i]`
/// and negated if bit `old_i` of `in_neg` is set.
#[inline]
pub fn npn4_apply(tt: u16, perm: [u8; 4], in_neg: u8, out_neg: bool) -> u16 {
    let mut out = 0u16;
    for new_addr in 0..16usize {
        let mut old_addr = 0usize;
        for new_i in 0..4usize {
            let old_i = perm[new_i] as usize;
            let bit = ((new_addr >> new_i) & 1) ^ (((in_neg as usize) >> old_i) & 1);
            old_addr |= bit << old_i;
        }
        out |= (((tt >> old_addr) & 1) as u16) << new_addr;
    }
    if out_neg {
        !out
    } else {
        out
    }
}

/// Reconstruct the original 16-bit truth table from its [`Npn4Transform`] descriptor.
#[inline]
pub fn npn4_reconstruct(t: &Npn4Transform) -> u16 {
    let base = if t.out_neg { !t.canonical } else { t.canonical };
    let mut orig = 0u16;
    for old_addr in 0..16usize {
        let mut new_addr = 0usize;
        for new_i in 0..4usize {
            let old_i = t.perm[new_i] as usize;
            let bit = ((old_addr >> old_i) & 1) ^ (((t.in_neg as usize) >> old_i) & 1);
            new_addr |= bit << new_i;
        }
        orig |= (((base >> new_addr) & 1) as u16) << old_addr;
    }
    orig
}

/// Compute the 4-input NPN canonical representative and witness transformation for `tt`.
///
/// Exhaustively evaluates all $16 \times 24 \times 2 = 768$ NPN transformations and
/// selects the lexicographically minimal 16-bit truth table.
pub fn npn4_canonical(tt: u16) -> Npn4Transform {
    let mut best = Npn4Transform {
        canonical: u16::MAX,
        perm: [0, 1, 2, 3],
        in_neg: 0,
        out_neg: false,
    };

    for &perm in &PERMS_4 {
        for in_neg in 0..16u8 {
            let pos_tt = npn4_apply(tt, perm, in_neg, false);
            if pos_tt < best.canonical {
                best = Npn4Transform {
                    canonical: pos_tt,
                    perm,
                    in_neg,
                    out_neg: false,
                };
            }
            let neg_tt = !pos_tt;
            if neg_tt < best.canonical {
                best = Npn4Transform {
                    canonical: neg_tt,
                    perm,
                    in_neg,
                    out_neg: true,
                };
            }
        }
    }

    best
}

/// Returns true iff two 4-input truth tables `a` and `b` belong to the same NPN equivalence class.
#[inline]
pub fn npn4_are_equivalent(a: u16, b: u16) -> bool {
    npn4_canonical(a).canonical == npn4_canonical(b).canonical
}

/// A $K$-feasible ($K \le 6$) priority cut rooted at a combinational node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PriorityCut {
    /// Sorted distinct leaf net `Spur`s (`leaves[0..k]`).
    pub leaves: [Spur; 6],
    /// Number of active leaves ($0 \le k \le 6$).
    pub k: u8,
    /// 64-bit LUT6 truth table computing the root node's function in terms of `leaves[0..k]`.
    pub init: u64,
    /// Arrival LUT depth of the cut ($0$ for primary/sequential leaves, $\ge 1$ for LUT cuts).
    pub delay: u32,
    /// Heuristic area flow of the cone covered by this cut.
    pub area_flow: f32,
    /// Exact local LUT count (MFFC area) of the cone covered by this cut.
    pub exact_area: u32,
}

impl PriorityCut {
    /// Construct a trivial 0-delay leaf cut for a primary input or register output net.
    #[inline]
    pub fn trivial_leaf(net: Spur) -> Self {
        let mut leaves = [Spur::default(); 6];
        leaves[0] = net;
        Self {
            leaves,
            k: 1,
            init: LUT6_VAR_INIT[0],
            delay: 0,
            area_flow: 0.0,
            exact_area: 0,
        }
    }

    /// Slice of active leaf net `Spur`s (`&self.leaves[..self.k as usize]`).
    #[inline]
    pub fn leaf_slice(&self) -> &[Spur] {
        &self.leaves[..self.k as usize]
    }
}

/// Statistics returned by [`Design::map_priority_cuts_lut6`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PriorityCutStats {
    pub luts_before: usize,
    pub luts_after: usize,
    pub depth_before: usize,
    pub depth_after: usize,
    pub cuts_evaluated: usize,
}

#[inline]
fn lut6_input_dont_care(init: u64, input: usize) -> bool {
    let shift = 1usize << input;
    let mask = COFACTOR_LO_MASK[input];
    (init & mask) == ((init >> shift) & mask)
}

/// Prune don't-care leaves from `(init, leaves[0..k])` and compact remaining leaves in sorted order.
pub fn prune_and_compact_cut(mut init: u64, leaves: &[Spur]) -> (u64, [Spur; 6], u8) {
    let k_in = leaves.len().min(6);
    for i in k_in..6 {
        if !lut6_input_dont_care(init, i) {
            // Replicate low cofactor across unused upper address bit `i`.
            let shift = 1usize << i;
            let lo = init & COFACTOR_LO_MASK[i];
            init = lo | (lo << shift);
        }
    }

    let mut active_old: [usize; 6] = [0; 6];
    let mut out_leaves = [Spur::default(); 6];
    let mut m = 0usize;
    for i in 0..k_in {
        if !lut6_input_dont_care(init, i) {
            active_old[m] = i;
            out_leaves[m] = leaves[i];
            m += 1;
        }
    }

    if m == k_in {
        return (init, out_leaves, m as u8);
    }

    let mut compact_init = 0u64;
    for new_addr in 0..64u64 {
        let mut old_addr = 0u64;
        for new_pos in 0..m {
            if ((new_addr >> new_pos) & 1) != 0 {
                old_addr |= 1u64 << active_old[new_pos];
            }
        }
        compact_init |= ((init >> old_addr) & 1) << new_addr;
    }
    (compact_init, out_leaves, m as u8)
}

/// Evaluate a 6-input LUT truth table `local_init` when its $m$ inputs are driven by
/// 64-bit truth tables `in_tts[0..m]` over a common 6-leaf support $L$, using parallel
/// 64-bit Shannon cofactor composition.
pub fn compose_lut6_shannon(local_init: u64, in_tts: &[u64]) -> u64 {
    // Recursively or iteratively reduce from 64 cofactor bits down to 1 64-bit word.
    let m = in_tts.len().min(6);
    if m == 0 {
        return if (local_init & 1) != 0 {
            u64::MAX
        } else {
            0
        };
    }

    let num_cofactors = 1usize << m;
    let mut words = [0u64; 64];
    for idx in 0..num_cofactors {
        words[idx] = if ((local_init >> idx) & 1) != 0 {
            u64::MAX
        } else {
            0
        };
    }

    for var in 0..m {
        let sel = in_tts[var];
        let half = 1usize << (m - 1 - var);
        for i in 0..half {
            let lo = words[2 * i];
            let hi = words[2 * i + 1];
            words[i] = (lo & !sel) | (hi & sel);
        }
    }

    words[0]
}

/// Lift a child cut `(child_init, child_leaves[0..child_k])` into the 64-bit truth table
/// space of a superset leaf array `union_leaves[0..union_k]`.
pub fn lift_cut_truth_table(
    child_init: u64,
    child_leaves: &[Spur],
    union_leaves: &[Spur],
) -> u64 {
    let mut mapped_tts = [0u64; 6];
    for (c_pos, &c_leaf) in child_leaves.iter().enumerate() {
        if let Some(u_pos) = union_leaves.iter().position(|&u| u == c_leaf) {
            mapped_tts[c_pos] = LUT6_VAR_INIT[u_pos];
        }
    }
    compose_lut6_shannon(child_init, &mapped_tts[..child_leaves.len()])
}

/// Merge two sorted leaf slices `a` and `b` into `out` if their union has $\le 6$ elements.
#[inline]
fn merge_sorted_leaves(a: &[Spur], b: &[Spur], out: &mut [Spur; 6]) -> Option<usize> {
    let mut i = 0usize;
    let mut j = 0usize;
    let mut k = 0usize;
    while i < a.len() && j < b.len() {
        let v = if a[i] < b[j] {
            let x = a[i];
            i += 1;
            x
        } else if b[j] < a[i] {
            let x = b[j];
            j += 1;
            x
        } else {
            let x = a[i];
            i += 1;
            j += 1;
            x
        };
        if k == 6 {
            return None;
        }
        out[k] = v;
        k += 1;
    }
    while i < a.len() {
        if k == 6 {
            return None;
        }
        out[k] = a[i];
        i += 1;
        k += 1;
    }
    while j < b.len() {
        if k == 6 {
            return None;
        }
        out[k] = b[j];
        j += 1;
        k += 1;
    }
    Some(k)
}

/// Priority-Cut 6-Feasible $K$-LUT Technology Mapper (`map_priority_cuts_lut6`).
pub fn map_priority_cuts_lut6(design: &mut Design) -> PriorityCutStats {
    design.map_priority_cuts_lut6()
}

impl Design {
    /// Remap the combinational [`CellKind::Lut6`] network of `self` using a 3-pass
    /// $K \le 6$ priority-cut technology mapper (`map_priority_cuts_lut6`):
    /// 1. Bottom-up $K \le 6$ priority-cut enumeration with 64-bit Shannon truth-table
    ///    composition and delay-optimal arrival time minimization.
    /// 2. Backward required-time propagation and area-flow recovery pass.
    /// 3. Exact local area recovery pass and clean LUT6 netlist reconstruction.
    pub fn map_priority_cuts_lut6(&mut self) -> PriorityCutStats {
        self.rebuild_indexes();
        let luts_before = self
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count();
        let depth_before = self.compute_max_comb_depth();

        if luts_before == 0 {
            return PriorityCutStats::default();
        }

        let i_pins = [
            intern("I0"),
            intern("I1"),
            intern("I2"),
            intern("I3"),
            intern("I4"),
            intern("I5"),
        ];
        let o_pin = intern("O");

        // Identify guarded nets (`mark_debug`, `Ila`, top-level `Out`/`Inout` ports, or non-LUT cell inputs).
        let mut ila_nets: HashSet<Spur> = self
            .cells
            .iter()
            .filter_map(|c| match c.kind {
                CellKind::Ila { net } => Some(net),
                _ => None,
            })
            .collect();
        for p in &self.ports {
            if matches!(p.dir, PortDir::Out | PortDir::Inout) {
                ila_nets.insert(p.name);
            }
        }
        for n in &self.nets {
            if n.attrs.flag("mark_debug") {
                ila_nets.insert(n.name);
            }
        }

        // Map each mappable LUT cell index `ci` -> `(out_net, local_init, raw_inputs[0..6])`.
        let num_cells = self.cells.len();
        let mut lut_out_net: Vec<Option<Spur>> = vec![None; num_cells];
        let mut lut_raw_ins: Vec<[Option<Spur>; 6]> = vec![[None; 6]; num_cells];
        let mut net_driver_lut: HashMap<Spur, usize> = HashMap::new();
        let mut net_fanout_count: HashMap<Spur, usize> = HashMap::new();

        for net in &self.nets {
            let mut loads = 0usize;
            for ep in &net.endpoints {
                let Some(&ci) = self.cell_ix.get(&ep.cell) else {
                    loads += 1;
                    continue;
                };
                if matches!(self.cells[ci].kind, CellKind::Lut6 { .. }) && ep.pin == o_pin {
                    lut_out_net[ci] = Some(net.name);
                    if !self.cells[ci].attrs.flag("DONT_TOUCH")
                        && self.cells[ci].attrs.get("PRIM").is_none()
                    {
                        net_driver_lut.insert(net.name, ci);
                    }
                } else {
                    loads += 1;
                    if matches!(self.cells[ci].kind, CellKind::Lut6 { .. }) {
                        for (p_idx, &p_sp) in i_pins.iter().enumerate() {
                            if ep.pin == p_sp {
                                lut_raw_ins[ci][p_idx] = Some(net.name);
                                break;
                            }
                        }
                    }
                }
            }
            net_fanout_count.insert(net.name, loads.max(1));
        }

        // Topological sort of mappable LUTs.
        let mut topo_luts: Vec<usize> = Vec::with_capacity(luts_before);
        let mut state = vec![0u8; num_cells];
        fn visit_topo(
            ci: usize,
            lut_raw_ins: &[[Option<Spur>; 6]],
            net_driver_lut: &HashMap<Spur, usize>,
            state: &mut [u8],
            topo: &mut Vec<usize>,
        ) {
            if state[ci] != 0 {
                return;
            }
            state[ci] = 1;
            for net_opt in &lut_raw_ins[ci] {
                if let Some(n) = *net_opt {
                    if let Some(&drv_ci) = net_driver_lut.get(&n) {
                        visit_topo(drv_ci, lut_raw_ins, net_driver_lut, state, topo);
                    }
                }
            }
            state[ci] = 2;
            topo.push(ci);
        }

        for (&_net, &ci) in &net_driver_lut {
            if state[ci] == 0 {
                visit_topo(ci, &lut_raw_ins, &net_driver_lut, &mut state, &mut topo_luts);
            }
        }

        // Determine required PO/sink nets that must remain driven in the mapped netlist.
        let mut required_nets: HashSet<Spur> = ila_nets.clone();
        for net in &self.nets {
            for ep in &net.endpoints {
                let Some(&ci) = self.cell_ix.get(&ep.cell) else {
                    required_nets.insert(net.name);
                    continue;
                };
                let c = &self.cells[ci];
                if matches!(c.kind, CellKind::Lut6 { .. }) {
                    if ep.pin == o_pin {
                        if c.attrs.flag("DONT_TOUCH") || c.attrs.get("PRIM").is_some() {
                            required_nets.insert(net.name);
                        }
                    } else if c.attrs.flag("DONT_TOUCH") || c.attrs.get("PRIM").is_some() {
                        required_nets.insert(net.name);
                    }
                } else {
                    // Driven into an Hff/D, IobOut/I, Carry4, BlackBox, etc.
                    required_nets.insert(net.name);
                }
            }
        }

        // Pass 1: Bottom-Up Priority-Cut Enumeration (K <= 6, max 12 priority cuts per LUT).
        const MAX_CUTS_PER_NODE: usize = 12;
        let mut node_cuts: HashMap<Spur, Vec<PriorityCut>> = HashMap::new();
        let mut best_cut_map: HashMap<Spur, PriorityCut> = HashMap::new();
        let mut cuts_evaluated = 0usize;

        for &ci in &topo_luts {
            let out_net = lut_out_net[ci].unwrap();
            let CellKind::Lut6 { init: raw_init } = self.cells[ci].kind else {
                continue;
            };

            // Normalize the LUT's immediate inputs `[Option<Spur>; 6]` into active sorted leaves.
            let (norm_init, norm_leaves, norm_k) =
                normalize_immediate_lut(raw_init, lut_raw_ins[ci]);

            // Start each fanin with its list of candidate cuts (plus the trivial 1-leaf cut for that fanin net).
            let k_imm = norm_k as usize;
            let imm_slice = &norm_leaves[..k_imm];

            // Accumulate partial combinations across the `k_imm` fanins:
            // Each partial entry is `(union_leaves, union_k, fanin_cuts_chosen)`.
            let mut partials: Vec<([Spur; 6], usize, Vec<PriorityCut>)> =
                vec![([Spur::default(); 6], 0, Vec::with_capacity(k_imm))];

            for &fanin_net in imm_slice {
                let mut fanin_options: Vec<PriorityCut> = Vec::with_capacity(MAX_CUTS_PER_NODE + 1);
                // Always include the trivial cut at `fanin_net` (stopping cut expansion at `fanin_net`).
                let mut triv = PriorityCut::trivial_leaf(fanin_net);
                if let Some(best_f) = best_cut_map.get(&fanin_net) {
                    triv.delay = best_f.delay;
                    triv.area_flow = best_f.area_flow;
                    triv.exact_area = best_f.exact_area;
                }
                fanin_options.push(triv);
                if let Some(cuts_f) = node_cuts.get(&fanin_net) {
                    for c in cuts_f {
                        if !fanin_options
                            .iter()
                            .any(|x| x.k == c.k && x.leaves[..x.k as usize] == c.leaves[..c.k as usize])
                        {
                            fanin_options.push(*c);
                        }
                    }
                }

                let mut next_partials: Vec<([Spur; 6], usize, Vec<PriorityCut>)> = Vec::new();
                for (cur_leaves, cur_k, cur_chosen) in &partials {
                    for opt in &fanin_options {
                        let mut merged = [Spur::default(); 6];
                        if let Some(new_k) = merge_sorted_leaves(
                            &cur_leaves[..*cur_k],
                            opt.leaf_slice(),
                            &mut merged,
                        ) {
                            let mut next_chosen = cur_chosen.clone();
                            next_chosen.push(*opt);
                            next_partials.push((merged, new_k, next_chosen));
                        }
                    }
                }
                // Keep top `MAX_CUTS_PER_NODE * 2` partial combinations by smallest leaf count & delay.
                if next_partials.len() > MAX_CUTS_PER_NODE * 2 {
                    next_partials.sort_by_key(|(_, uk, chosen)| {
                        let max_d = chosen.iter().map(|c| c.delay).max().unwrap_or(0);
                        (max_d, *uk)
                    });
                    next_partials.truncate(MAX_CUTS_PER_NODE * 2);
                }
                partials = next_partials;
            }

            let fanout_v = (*net_fanout_count.get(&out_net).unwrap_or(&1)).max(1) as f32;
            let mut candidates: Vec<PriorityCut> = Vec::with_capacity(partials.len());

            for (u_leaves, u_k, chosen) in partials {
                cuts_evaluated += 1;
                let u_slice = &u_leaves[..u_k];
                let mut lifted_in_tts = [0u64; 6];
                for (idx, fc) in chosen.iter().enumerate() {
                    lifted_in_tts[idx] =
                        lift_cut_truth_table(fc.init, fc.leaf_slice(), u_slice);
                }
                let composed = compose_lut6_shannon(norm_init, &lifted_in_tts[..k_imm]);
                let (pruned_init, pruned_leaves, pruned_k) =
                    prune_and_compact_cut(composed, u_slice);

                let p_slice = &pruned_leaves[..pruned_k as usize];
                let max_leaf_delay = p_slice
                    .iter()
                    .map(|l| best_cut_map.get(l).map_or(0, |c| c.delay))
                    .max()
                    .unwrap_or(0);
                let delay = if pruned_k == 0 { 0 } else { 1 + max_leaf_delay };

                let sum_af: f32 = p_slice
                    .iter()
                    .map(|l| best_cut_map.get(l).map_or(0.0, |c| c.area_flow))
                    .sum();
                let area_flow = (1.0 + sum_af) / fanout_v;

                let sum_exact: u32 = p_slice
                    .iter()
                    .map(|l| {
                        let fo = (*net_fanout_count.get(l).unwrap_or(&1)).max(1) as u32;
                        if fo == 1 {
                            best_cut_map.get(l).map_or(0, |c| c.exact_area)
                        } else {
                            0
                        }
                    })
                    .sum();
                let exact_area = 1 + sum_exact;

                let cand = PriorityCut {
                    leaves: pruned_leaves,
                    k: pruned_k,
                    init: pruned_init,
                    delay,
                    area_flow,
                    exact_area,
                };

                if let Some(existing) = candidates.iter_mut().find(|c| {
                    c.k == cand.k && c.leaves[..c.k as usize] == cand.leaves[..cand.k as usize]
                }) {
                    if (cand.delay, cand.exact_area) < (existing.delay, existing.exact_area) {
                        *existing = cand;
                    }
                } else {
                    candidates.push(cand);
                }
            }

            // Sort priority cuts: delay-first for Pass 1!
            candidates.sort_by(|a, b| {
                a.delay
                    .cmp(&b.delay)
                    .then_with(|| a.k.cmp(&b.k))
                    .then_with(|| {
                        a.area_flow
                            .partial_cmp(&b.area_flow)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .then_with(|| a.exact_area.cmp(&b.exact_area))
            });
            candidates.truncate(MAX_CUTS_PER_NODE);

            if let Some(best) = candidates.first().copied() {
                best_cut_map.insert(out_net, best);
            }
            node_cuts.insert(out_net, candidates);
        }

        // Pass 2 & Pass 3: Backward Required-Time Propagation + Area-Flow & Exact-Area Recovery!
        let global_max_delay = required_nets
            .iter()
            .filter_map(|n| best_cut_map.get(n).map(|c| c.delay))
            .max()
            .unwrap_or(0);

        for pass in 0..2 {
            let mut required_time: HashMap<Spur, u32> = HashMap::new();
            for &req_net in &required_nets {
                if best_cut_map.contains_key(&req_net) {
                    required_time.insert(req_net, global_max_delay);
                }
            }

            // Propagate required times in reverse topological order.
            for &ci in topo_luts.iter().rev() {
                let out_net = lut_out_net[ci].unwrap();
                let Some(&req_v) = required_time.get(&out_net) else {
                    continue;
                };
                let Some(sel_cut) = best_cut_map.get(&out_net) else {
                    continue;
                };
                let leaf_budget = req_v.saturating_sub(1);
                for &leaf in sel_cut.leaf_slice() {
                    required_time
                        .entry(leaf)
                        .and_modify(|r| *r = (*r).min(leaf_budget))
                        .or_insert(leaf_budget);
                }
            }

            // Forward pass: among all cuts with `cut.delay <= required_time[v]`,
            // Pass 2 (pass == 0) minimizes `(area_flow, exact_area, k, delay)`,
            // Pass 3 (pass == 1) minimizes `(exact_area, k, area_flow, delay)`.
            for &ci in &topo_luts {
                let out_net = lut_out_net[ci].unwrap();
                let req_v = required_time.get(&out_net).copied().unwrap_or(u32::MAX);
                let Some(cuts) = node_cuts.get_mut(&out_net) else {
                    continue;
                };
                let fanout_v = (*net_fanout_count.get(&out_net).unwrap_or(&1)).max(1) as f32;

                for c in cuts.iter_mut() {
                    let leaves_copy = c.leaves;
                    let p_slice = &leaves_copy[..c.k as usize];
                    let max_d = p_slice
                        .iter()
                        .map(|l| best_cut_map.get(l).map_or(0, |bc| bc.delay))
                        .max()
                        .unwrap_or(0);
                    c.delay = if c.k == 0 { 0 } else { 1 + max_d };
                    let sum_af: f32 = p_slice
                        .iter()
                        .map(|l| best_cut_map.get(l).map_or(0.0, |bc| bc.area_flow))
                        .sum();
                    c.area_flow = (1.0 + sum_af) / fanout_v;
                    let sum_ex: u32 = p_slice
                        .iter()
                        .map(|l| {
                            let fo = (*net_fanout_count.get(l).unwrap_or(&1)).max(1) as u32;
                            if fo == 1 {
                                best_cut_map.get(l).map_or(0, |bc| bc.exact_area)
                            } else {
                                0
                            }
                        })
                        .sum();
                    c.exact_area = 1 + sum_ex;
                }

                let best = cuts
                    .iter()
                    .filter(|c| c.delay <= req_v)
                    .min_by(|a, b| {
                        if pass == 0 {
                            a.area_flow
                                .partial_cmp(&b.area_flow)
                                .unwrap_or(std::cmp::Ordering::Equal)
                                .then_with(|| a.exact_area.cmp(&b.exact_area))
                                .then_with(|| a.k.cmp(&b.k))
                                .then_with(|| a.delay.cmp(&b.delay))
                        } else {
                            a.exact_area
                                .cmp(&b.exact_area)
                                .then_with(|| a.k.cmp(&b.k))
                                .then_with(|| {
                                    a.area_flow
                                        .partial_cmp(&b.area_flow)
                                        .unwrap_or(std::cmp::Ordering::Equal)
                                })
                                .then_with(|| a.delay.cmp(&b.delay))
                        }
                    })
                    .copied()
                    .unwrap_or_else(|| cuts[0]);

                best_cut_map.insert(out_net, best);
            }
        }

        // Materialize selected cuts starting from `required_nets` and mark active LUT cells.
        let mut active_nets: HashSet<Spur> = HashSet::new();
        let mut worklist: Vec<Spur> = required_nets
            .iter()
            .copied()
            .filter(|n| best_cut_map.contains_key(n))
            .collect();
        while let Some(net) = worklist.pop() {
            if !active_nets.insert(net) {
                continue;
            }
            if let Some(cut) = best_cut_map.get(&net) {
                for &leaf in cut.leaf_slice() {
                    if best_cut_map.contains_key(&leaf) && !active_nets.contains(&leaf) {
                        worklist.push(leaf);
                    }
                }
            }
        }

        // Update each active LUT's `init` and input connections, and remove absorbed intermediate LUTs.
        let mut dead_luts: Vec<usize> = Vec::new();
        for &ci in &topo_luts {
            let out_net = lut_out_net[ci].unwrap();
            if !active_nets.contains(&out_net) {
                dead_luts.push(ci);
                continue;
            }
            let sel = best_cut_map[&out_net];
            if let CellKind::Lut6 { init } = &mut self.cells[ci].kind {
                *init = sel.init;
            }
            let cell_name = self.cells[ci].name;
            for net in &mut self.nets {
                net.endpoints
                    .retain(|e| !(e.cell == cell_name && i_pins.contains(&e.pin)));
            }
            self.rebuild_indexes();
            let cell_str = resolve(cell_name);
            for (idx, &leaf_sp) in sel.leaf_slice().iter().enumerate() {
                let pin_str = match idx {
                    0 => "I0",
                    1 => "I1",
                    2 => "I2",
                    3 => "I3",
                    4 => "I4",
                    _ => "I5",
                };
                self.connect(resolve(leaf_sp), cell_str, pin_str);
            }
        }

        if !dead_luts.is_empty() {
            let dead_names: HashSet<Spur> =
                dead_luts.iter().map(|&ci| self.cells[ci].name).collect();
            for net in &mut self.nets {
                net.endpoints.retain(|e| !dead_names.contains(&e.cell));
            }
            self.nets.retain(|n| !n.endpoints.is_empty());
            let dead_set: HashSet<usize> = dead_luts.into_iter().collect();
            let mut idx = 0usize;
            self.cells.retain(|_| {
                let keep = !dead_set.contains(&idx);
                idx += 1;
                keep
            });
            self.rebuild_indexes();
        }

        let luts_after = self
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count();
        let depth_after = self.compute_max_comb_depth();

        PriorityCutStats {
            luts_before,
            luts_after,
            depth_before,
            depth_after,
            cuts_evaluated,
        }
    }

    /// Compute the maximum combinational [`CellKind::Lut6`] depth of `self`.
    pub fn compute_max_comb_depth(&self) -> usize {
        let i_pins = [
            intern("I0"),
            intern("I1"),
            intern("I2"),
            intern("I3"),
            intern("I4"),
            intern("I5"),
        ];
        let o_pin = intern("O");
        let n = self.cells.len();
        let mut ins = vec![[None; 6]; n];
        let mut net_drv: HashMap<Spur, usize> = HashMap::new();

        for net in &self.nets {
            for ep in &net.endpoints {
                let Some(&ci) = self.cell_ix.get(&ep.cell) else {
                    continue;
                };
                if !matches!(self.cells[ci].kind, CellKind::Lut6 { .. }) {
                    continue;
                }
                if ep.pin == o_pin {
                    net_drv.insert(net.name, ci);
                } else {
                    for (idx, &p_sp) in i_pins.iter().enumerate() {
                        if ep.pin == p_sp {
                            ins[ci][idx] = Some(net.name);
                            break;
                        }
                    }
                }
            }
        }

        let mut depth = vec![0usize; n];
        let mut state = vec![0u8; n];
        fn dfs(
            ci: usize,
            ins: &[[Option<Spur>; 6]],
            net_drv: &HashMap<Spur, usize>,
            depth: &mut [usize],
            state: &mut [u8],
        ) -> usize {
            if state[ci] == 2 {
                return depth[ci];
            }
            if state[ci] == 1 {
                return 0;
            }
            state[ci] = 1;
            let mut max_in = 0usize;
            for net_opt in &ins[ci] {
                if let Some(net) = *net_opt {
                    if let Some(&d_ci) = net_drv.get(&net) {
                        max_in = max_in.max(dfs(d_ci, ins, net_drv, depth, state));
                    }
                }
            }
            let d = 1 + max_in;
            depth[ci] = d;
            state[ci] = 2;
            d
        }

        let mut max_d = 0usize;
        for (ci, c) in self.cells.iter().enumerate() {
            if matches!(c.kind, CellKind::Lut6 { .. }) {
                max_d = max_d.max(dfs(ci, &ins, &net_drv, &mut depth, &mut state));
            }
        }
        max_d
    }
}

fn normalize_immediate_lut(
    mut init: u64,
    mut inputs: [Option<Spur>; 6],
) -> (u64, [Spur; 6], u8) {
    // Fold unconnected pins to 0.
    for i in 0..6 {
        if inputs[i].is_none() && !lut6_input_dont_care(init, i) {
            let mut folded = 0u64;
            for addr in 0..64u64 {
                let fixed = addr & !(1u64 << i);
                folded |= ((init >> fixed) & 1) << addr;
            }
            init = folded;
        }
    }
    // Coalesce duplicate pins.
    for i in 0..6 {
        let Some(ni) = inputs[i] else { continue };
        for j in (i + 1)..6 {
            if inputs[j] == Some(ni) {
                let mut collapsed = 0u64;
                for addr in 0..64u64 {
                    let bit_i = (addr >> i) & 1;
                    let eff = (addr & !(1u64 << j)) | (bit_i << j);
                    collapsed |= ((init >> eff) & 1) << addr;
                }
                init = collapsed;
                inputs[j] = None;
            }
        }
    }
    // Prune don't-care pins and sort remaining active pins by Spur.
    let mut active: [(Spur, usize); 6] = [(Spur::default(), 0); 6];
    let mut m = 0usize;
    for (pin, net_opt) in inputs.iter().enumerate() {
        if let Some(n) = *net_opt {
            if !lut6_input_dont_care(init, pin) {
                active[m] = (n, pin);
                m += 1;
            }
        }
    }
    active[..m].sort_unstable_by_key(|(s, _)| *s);

    let mut out_leaves = [Spur::default(); 6];
    for new_pos in 0..m {
        out_leaves[new_pos] = active[new_pos].0;
    }

    let mut perm_init = 0u64;
    for new_addr in 0..64u64 {
        let mut old_addr = 0u64;
        for new_pos in 0..m {
            if ((new_addr >> new_pos) & 1) != 0 {
                old_addr |= 1u64 << active[new_pos].1;
            }
        }
        perm_init |= ((init >> old_addr) & 1) << new_addr;
    }
    (perm_init, out_leaves, m as u8)
}
