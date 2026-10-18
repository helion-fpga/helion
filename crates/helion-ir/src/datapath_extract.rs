//! Yosys `alumacc` / `muxpack` Datapath Extraction (`crates/helion-ir`):
//! 1. **LUT Ripple-Carry -> `Carry4` Promotion (`extract_carry4_chains_from_luts`)**:
//!    Recognizes bit-sliced ripple-carry adder/subtractor chains of length $\ge 4$ bits
//!    built from discrete `Lut6` sum (`a ^ b ^ c`, `INIT=0x96`) and carry majority
//!    (`maj(a, b, c)`, `INIT=0xE8` or subtractor borrow `maj(a, ~b, c)`, `INIT=0xD4`/`0xB2`)
//!    cells, replacing the $O(N)$-depth carry LUT cascade with fast hardware [`CellKind::Carry4`]
//!    chains + 2-input propagate `Lut6`s.
//! 2. **`Lut6` Mux Tree -> `Muxf7`/`Muxf8` Cascade Packing (`pack_muxf7_muxf8_trees`)**:
//!    Detects 2:1 `Lut6` multiplexers (`s ? b : a`) combining two `Lut6` cones (or two `Muxf7`
//!    cones) and 4:1 `Lut6` multiplexers combining four `Lut6` cones, replacing the root `Lut6`
//!    muxes with dedicated `Muxf7` (`BEL=F7AMUX`) and `Muxf8` (`BEL=F8MUX`) hardware primitives.
//! 3. **Mutually-Exclusive Branch ALU Sharing (`share_resource_muxed_alu`)**:
//!    Detects two parallel [`CellKind::Carry4`] adder/subtractor chains feeding opposite arms
//!    of a shared select multiplexer (`sel ? (a ± b) : (c ± d)`) and folds them into a single
//!    shared [`CellKind::Carry4`] chain preceded by operand input multiplexers
//!    (`(sel ? a : c) ± (sel ? b : d)`).

use std::collections::{HashMap, HashSet};

use lasso::Spur;

use crate::{intern, resolve, Attrs, Cell, CellKind, Design, Endpoint};

/// Canonical 64-bit `Lut6` truth table for 2:1 mux `O = I2 ? I1 : I0`.
pub const MUX2_INIT: u64 = 0xCACA_CACA_CACA_CACA;
/// Canonical 64-bit `Lut6` truth table for 2-input XOR `O = I0 ^ I1`.
pub const XOR2_INIT: u64 = 0x6666_6666_6666_6666;
/// Canonical 64-bit `Lut6` truth table for 2-input XNOR `O = ~(I0 ^ I1) = I0 ^ ~I1`.
pub const XNOR2_INIT: u64 = 0x9999_9999_9999_9999;
/// Canonical 64-bit `Lut6` truth table for 3-input full-adder sum `O = I0 ^ I1 ^ I2`.
pub const FA_SUM_INIT: u64 = 0x9696_9696_9696_9696;
/// Canonical 64-bit `Lut6` truth table for 3-input full-adder carry `O = (I0 & I1) | (I0 & I2) | (I1 & I2)`.
pub const FA_CARRY_INIT: u64 = 0xE8E8_E8E8_E8E8_E8E8;
/// Canonical 64-bit `Lut6` truth table for 3-input subtractor carry `O = (I0 & ~I1) | (I0 & I2) | (~I1 & I2)`.
pub const SUB_CARRY_INIT: u64 = 0xB2B2_B2B2_B2B2_B2B2;
const COFACTOR_LO_MASK: [u64; 6] = [
    0x5555_5555_5555_5555,
    0x3333_3333_3333_3333,
    0x0F0F_0F0F_0F0F_0F0F,
    0x00FF_00FF_00FF_00FF,
    0x0000_FFFF_0000_FFFF,
    0x0000_0000_FFFF_FFFF,
];

/// Statistics from [`extract_carry4_chains_from_luts`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CarryExtractStats {
    /// Number of ripple-carry chains promoted.
    pub chains_extracted: usize,
    /// Total bit slices promoted into [`CellKind::Carry4`] lanes.
    pub bits_promoted: usize,
    /// Number of [`CellKind::Carry4`] primitives instantiated.
    pub carry4_cells_created: usize,
}

/// Statistics from [`pack_muxf7_muxf8_trees`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MuxPackStats {
    /// Number of `Muxf7` hardware BELs packed.
    pub muxf7_packed: usize,
    /// Number of `Muxf8` hardware BELs packed.
    pub muxf8_packed: usize,
}

/// Statistics from [`share_resource_muxed_alu`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AluShareStats {
    /// Number of mutually-exclusive dual-adder trees folded into a single shared ALU.
    pub alus_shared: usize,
    /// Number of redundant [`CellKind::Carry4`] primitives eliminated.
    pub carry4_eliminated: usize,
}

#[inline]
fn lut6_pin_dont_care(init: u64, pin: usize) -> bool {
    let shift = 1usize << pin;
    let mask = COFACTOR_LO_MASK[pin];
    (init & mask) == ((init >> shift) & mask)
}

#[inline]
fn fold_const_pin(init: u64, pin: usize, val: bool) -> u64 {
    let mut out = 0u64;
    for addr in 0..64u64 {
        let fixed = if val {
            addr | (1u64 << pin)
        } else {
            addr & !(1u64 << pin)
        };
        out |= ((init >> fixed) & 1) << addr;
    }
    out
}

/// Extract active inputs of a `Lut6` and its compacted truth table over `active[0..m]`.
fn compact_lut_active(mut init: u64, raw_ins: &[Option<Spur>; 6]) -> (u64, Vec<Spur>) {
    for i in 0..6 {
        if raw_ins[i].is_none() && !lut6_pin_dont_care(init, i) {
            init = fold_const_pin(init, i, false);
        }
    }
    let mut active_pins = Vec::with_capacity(6);
    let mut active_nets = Vec::with_capacity(6);
    for (pin, net_opt) in raw_ins.iter().enumerate() {
        if let Some(net) = *net_opt {
            if !lut6_pin_dont_care(init, pin) {
                active_pins.push(pin);
                active_nets.push(net);
            }
        }
    }
    let m = active_pins.len();
    let mut compact_init = 0u64;
    for new_addr in 0..64u64 {
        let mut old_addr = 0u64;
        for new_pos in 0..m {
            if ((new_addr >> new_pos) & 1) != 0 {
                old_addr |= 1u64 << active_pins[new_pos];
            }
        }
        compact_init |= ((init >> old_addr) & 1) << new_addr;
    }
    (compact_init, active_nets)
}

/// Recognize a 3-input carry LUT (`maj(a, b, c)` or subtractor borrow `maj(a, ~b, c)`).
///
/// Returns `Some((inputs_set, inverted_net_opt))`:
/// - For symmetric adder majority `0xE8`: `inverted_net_opt = None` (any of the 3 inputs can be `cin`,
///   and the other two are `(a, b)`).
/// - For subtractor carry (`maj(x, ~y, z)` where exactly one input `y` is inverted):
///   `inverted_net_opt = Some(y)`.
fn classify_carry_lut(compact_init: u64, active_nets: &[Spur]) -> Option<Option<Spur>> {
    if active_nets.len() != 3 {
        return None;
    }
    let byte = (compact_init & 0xFF) as u8;
    // Symmetric majority(x0, x1, x2) = 0xE8:
    if byte == 0xE8 {
        return Some(None);
    }
    // Majority with one input inverted:
    // - ~x0: maj(~x0, x1, x2) -> addr 0(0,0,0)->0, 1(1,0,0)->0, 2(0,1,0)->1, 3(1,1,0)->0, 4(0,0,1)->1, 5(1,0,1)->0, 6(0,1,1)->1, 7(1,1,1)->1 => 0b1101_0100 = 0xD4 (Wait: at addr 2 (x0=0,x1=1,x2=0): ~x0=1, x1=1 -> 1; at addr 4 (x0=0,x1=0,x2=1): ~x0=1, x2=1 -> 1; at addr 6 (x0=0,x1=1,x2=1)->1; at addr 7 (x0=1,x1=1,x2=1)->1 => bits 2,4,6,7 = 0xD4! So 0xD4 has `x0` inverted!)
    // - ~x1: maj(x0, ~x1, x2) -> bits 1(1,0,0), 4(0,0,1), 5(1,0,1), 7(1,1,1) => 0b1011_0010 = 0xB2 (`x1` inverted!)
    // - ~x2: maj(x0, x1, ~x2) -> bits 1(1,0,0), 2(0,1,0), 3(1,1,0), 7(1,1,1) => 0b1000_1110 = 0x8E (`x2` inverted!)
    match byte {
        0xD4 => Some(Some(active_nets[0])),
        0xB2 => Some(Some(active_nets[1])),
        0x8E => Some(Some(active_nets[2])),
        _ => None,
    }
}

/// Check if a 3-input LUT computes `x0 ^ x1 ^ x2` (`0x96`) or `~(x0 ^ x1 ^ x2)` (`0x69`).
fn classify_sum_lut(compact_init: u64, active_nets: &[Spur]) -> Option<bool> {
    if active_nets.len() != 3 {
        return None;
    }
    match (compact_init & 0xFF) as u8 {
        0x96 => Some(false), // a ^ b ^ c
        0x69 => Some(true),  // ~(a ^ b ^ c) = a ^ ~b ^ c
        _ => None,
    }
}

/// Check if a 3-input LUT computes a 2:1 mux `sel ? in1 : in0`.
/// Returns `Some((in0_net, in1_net, sel_net))`.
fn classify_mux2_lut(compact_init: u64, active_nets: &[Spur]) -> Option<(Spur, Spur, Spur)> {
    if active_nets.len() != 3 {
        return None;
    }
    let byte = (compact_init & 0xFF) as u8;
    // Test each of the 3 active inputs as `sel` (`s_idx in 0..3`) and the other two as `(in0, in1)`:
    let perms = [
        (0, 1, 2),
        (1, 0, 2),
        (0, 2, 1),
        (2, 0, 1),
        (1, 2, 0),
        (2, 1, 0),
    ];
    for (i0, i1, isel) in perms {
        let mut expected = 0u8;
        for addr in 0..8usize {
            let b0 = (addr >> i0) & 1;
            let b1 = (addr >> i1) & 1;
            let bs = (addr >> isel) & 1;
            let val = if bs != 0 { b1 } else { b0 };
            expected |= (val as u8) << addr;
        }
        if byte == expected {
            return Some((active_nets[i0], active_nets[i1], active_nets[isel]));
        }
    }
    None
}

/// Check if a 6-input LUT computes a 4:1 mux `sel1 ? (sel0 ? d3 : d2) : (sel0 ? d1 : d0)`.
/// Returns `Some(([d0, d1, d2, d3], sel0, sel1))`.
fn classify_mux4_lut(
    compact_init: u64,
    active_nets: &[Spur],
) -> Option<([Spur; 4], Spur, Spur)> {
    if active_nets.len() != 6 {
        return None;
    }
    // Check all pairs `(s0, s1)` of select pins among `0..6`.
    for s0 in 0..6usize {
        for s1 in 0..6usize {
            if s0 == s1 {
                continue;
            }
            let mut data_pins = [0usize; 4];
            let mut di = 0usize;
            for p in 0..6usize {
                if p != s0 && p != s1 {
                    data_pins[di] = p;
                    di += 1;
                }
            }
            // Check if for every `(v_s1, v_s0) in 0..4`, the cofactor of `compact_init`
            // equals one of the 4 data variables `data_pins[q]`.
            let mut matched_d = [Spur::default(); 4];
            let mut all_ok = true;
            let mut used_mask = 0u8;
            for sel_val in 0..4usize {
                let vs0 = sel_val & 1;
                let vs1 = (sel_val >> 1) & 1;
                let mut found_pin = None;
                for (q, &dp) in data_pins.iter().enumerate() {
                    if (used_mask & (1 << q)) != 0 {
                        continue;
                    }
                    let mut matches_dp = true;
                    for addr in 0..64usize {
                        if ((addr >> s0) & 1) == vs0 && ((addr >> s1) & 1) == vs1 {
                            let bit = ((compact_init >> addr) & 1) as usize;
                            let d_bit = (addr >> dp) & 1;
                            if bit != d_bit {
                                matches_dp = false;
                                break;
                            }
                        }
                    }
                    if matches_dp {
                        found_pin = Some((q, dp));
                        break;
                    }
                }
                if let Some((q, dp)) = found_pin {
                    used_mask |= 1 << q;
                    matched_d[sel_val] = active_nets[dp];
                } else {
                    all_ok = false;
                    break;
                }
            }
            if all_ok {
                return Some((matched_d, active_nets[s0], active_nets[s1]));
            }
        }
    }
    None
}

/// Promote discrete `Lut6` ripple-carry adder/subtractor chains ($\ge 4$ bits) into [`CellKind::Carry4`] chains.
pub fn extract_carry4_chains_from_luts(design: &mut Design) -> CarryExtractStats {
    design.extract_carry4_chains_from_luts()
}

/// Pack 2:1 and 4:1 `Lut6` multiplexer trees into dedicated `Muxf7` and `Muxf8` hardware BELs.
pub fn pack_muxf7_muxf8_trees(design: &mut Design) -> MuxPackStats {
    design.pack_muxf7_muxf8_trees()
}

/// Share mutually-exclusive [`CellKind::Carry4`] branch ALUs feeding a common select multiplexer.
pub fn share_resource_muxed_alu(design: &mut Design) -> AluShareStats {
    design.share_resource_muxed_alu()
}

#[derive(Clone, Debug)]
struct CarryLutCandidate {
    ci: usize,
    out_net: Spur,
    inputs: [Spur; 3],
    inv_net: Option<Spur>,
}

#[derive(Clone, Debug)]
struct ResolvedCarrySlice {
    carry_ci: usize,
    sum_ci: Option<usize>,
    a_net: Spur,
    b_net: Spur,
    cin_net: Spur,
    cout_net: Spur,
    sum_net: Option<Spur>,
    is_sub: bool,
}

impl Design {
    /// Recognize bit-sliced ripple-carry adder/subtractor LUT chains of length $\ge 4$ bits
    /// and promote them into [`CellKind::Carry4`] chains + 2-input propagate `Lut6`s.
    pub fn extract_carry4_chains_from_luts(&mut self) -> CarryExtractStats {
        self.rebuild_indexes();
        let mut stats = CarryExtractStats::default();
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
        let mut lut_out: Vec<Option<Spur>> = vec![None; n];
        let mut lut_ins: Vec<[Option<Spur>; 6]> = vec![[None; 6]; n];
        for net in &self.nets {
            for ep in &net.endpoints {
                let Some(&ci) = self.cell_ix.get(&ep.cell) else {
                    continue;
                };
                if !matches!(self.cells[ci].kind, CellKind::Lut6 { .. }) {
                    continue;
                }
                if ep.pin == o_pin {
                    lut_out[ci] = Some(net.name);
                } else {
                    for (idx, &p_sp) in i_pins.iter().enumerate() {
                        if ep.pin == p_sp {
                            lut_ins[ci][idx] = Some(net.name);
                            break;
                        }
                    }
                }
            }
        }

        // Collect all 3-input carry LUTs and 3-input sum LUTs.
        let mut carry_cands: HashMap<Spur, CarryLutCandidate> = HashMap::new();
        let mut sum_by_input_set: HashMap<[Spur; 3], (usize, Spur, bool)> = HashMap::new();

        for (ci, c) in self.cells.iter().enumerate() {
            if c.attrs.flag("DONT_TOUCH") || c.attrs.get("PRIM").is_some() {
                continue;
            }
            let CellKind::Lut6 { init } = c.kind else {
                continue;
            };
            let Some(out_net) = lut_out[ci] else {
                continue;
            };
            let (compact_init, active_nets) = compact_lut_active(init, &lut_ins[ci]);
            if active_nets.len() != 3 {
                continue;
            }
            let mut sorted_ins = [active_nets[0], active_nets[1], active_nets[2]];
            sorted_ins.sort_unstable();

            if let Some(inv_net) = classify_carry_lut(compact_init, &active_nets) {
                carry_cands.insert(
                    out_net,
                    CarryLutCandidate {
                        ci,
                        out_net,
                        inputs: [active_nets[0], active_nets[1], active_nets[2]],
                        inv_net,
                    },
                );
            } else if let Some(is_xnor) = classify_sum_lut(compact_init, &active_nets) {
                sum_by_input_set.insert(sorted_ins, (ci, out_net, is_xnor));
            }
        }

        if carry_cands.len() < 4 {
            return stats;
        }

        // Map `cin_net -> carry_out_net` where `cin_net` is an output of one carry LUT feeding another carry LUT.
        let mut next_in_chain: HashMap<Spur, Spur> = HashMap::new();
        let mut has_prev_carry: HashSet<Spur> = HashSet::new();
        for (&cout_net, cand) in &carry_cands {
            for &inp in &cand.inputs {
                if carry_cands.contains_key(&inp) && inp != cout_net {
                    // `inp` is the carry output of the previous bit slice!
                    if cand.inv_net != Some(inp) {
                        next_in_chain.insert(inp, cout_net);
                        has_prev_carry.insert(cout_net);
                    }
                }
            }
        }

        // Trace maximal chains starting from carry LUTs that have no predecessor carry LUT.
        let mut head_nets: Vec<Spur> = carry_cands
            .keys()
            .copied()
            .filter(|n| !has_prev_carry.contains(n))
            .collect();
        head_nets.sort_by_key(|s| resolve(*s));

        let mut cells_to_remove: Vec<usize> = Vec::new();
        let mut promoted_Chains: Vec<Vec<ResolvedCarrySlice>> = Vec::new();

        for head in head_nets {
            let mut chain_nets = vec![head];
            let mut cur = head;
            while let Some(&nxt) = next_in_chain.get(&cur) {
                if chain_nets.contains(&nxt) {
                    break;
                }
                chain_nets.push(nxt);
                cur = nxt;
            }
            if chain_nets.len() < 4 {
                continue;
            }

            let mut slices: Vec<ResolvedCarrySlice> = Vec::with_capacity(chain_nets.len());
            let mut valid = true;
            for (idx, &cout_net) in chain_nets.iter().enumerate() {
                let cand = &carry_cands[&cout_net];
                // Determine `cin_net` for this stage:
                // For `idx > 0`, `cin_net` is `chain_nets[idx - 1]`.
                // For `idx == 0`, if `next_in_chain` or `sum_by_input_set` doesn't distinguish `cin`,
                // pick the non-inverted input that is not named like a bit slice `a0`/`b0`, or the 3rd input.
                let cin_net = if idx > 0 {
                    chain_nets[idx - 1]
                } else {
                    pick_initial_cin(&cand.inputs, cand.inv_net)
                };

                let mut data_ins: Vec<Spur> = cand
                    .inputs
                    .iter()
                    .copied()
                    .filter(|&x| x != cin_net)
                    .collect();
                if data_ins.len() != 2 {
                    valid = false;
                    break;
                }
                let is_sub = cand.inv_net.is_some();
                let (a_net, b_net) = if Some(data_ins[0]) == cand.inv_net {
                    (data_ins[1], data_ins[0])
                } else {
                    data_ins.sort_by_key(|s| resolve(*s));
                    if Some(data_ins[0]) == cand.inv_net {
                        (data_ins[1], data_ins[0])
                    } else {
                        (data_ins[0], data_ins[1])
                    }
                };

                let mut sorted_ins = cand.inputs;
                sorted_ins.sort_unstable();
                let (sum_ci, sum_net) = match sum_by_input_set.get(&sorted_ins) {
                    Some(&(sci, snet, _)) => (Some(sci), Some(snet)),
                    None => (None, None),
                };

                slices.push(ResolvedCarrySlice {
                    carry_ci: cand.ci,
                    sum_ci,
                    a_net,
                    b_net,
                    cin_net,
                    cout_net,
                    sum_net,
                    is_sub,
                });
            }

            if valid {
                for sl in &slices {
                    cells_to_remove.push(sl.carry_ci);
                    if let Some(sci) = sl.sum_ci {
                        cells_to_remove.push(sci);
                    }
                }
                promoted_Chains.push(slices);
            }
        }

        for (chain_idx, slices) in promoted_Chains.into_iter().enumerate() {
            stats.chains_extracted += 1;
            stats.bits_promoted += slices.len();
            let num_chunks = slices.len().div_ceil(4);

            for chunk in 0..num_chunks {
                let c4_name = format!("u_extracted_carry4_c{chain_idx}_{chunk}");
                self.add_cell(&c4_name, CellKind::Carry4);
                stats.carry4_cells_created += 1;

                if chunk == 0 {
                    let cin0_str = resolve(slices[0].cin_net).to_string();
                    self.connect(&cin0_str, &c4_name, "CYINIT");
                } else {
                    let prev_co = format!("n_extracted_carry4_co_c{chain_idx}_{}", chunk - 1);
                    self.connect(&prev_co, &c4_name, "CI");
                }

                for lane in 0..4 {
                    let bit_idx = chunk * 4 + lane;
                    if bit_idx >= slices.len() {
                        break;
                    }
                    let sl = &slices[bit_idx];
                    let prop_lut = format!("u_extracted_prop_c{chain_idx}_b{bit_idx}");
                    let prop_net = format!("n_extracted_prop_c{chain_idx}_b{bit_idx}");
                    let prop_init = if sl.is_sub { XNOR2_INIT } else { XOR2_INIT };
                    let a_str = resolve(sl.a_net).to_string();
                    let b_str = resolve(sl.b_net).to_string();
                    let cout_str = resolve(sl.cout_net).to_string();

                    self.add_cell(&prop_lut, CellKind::Lut6 { init: prop_init });
                    self.connect(&a_str, &prop_lut, "I0");
                    self.connect(&b_str, &prop_lut, "I1");
                    self.connect(&prop_net, &prop_lut, "O");

                    self.connect(&prop_net, &c4_name, &format!("S{lane}"));
                    self.connect(&a_str, &c4_name, &format!("DI{lane}"));
                    self.connect(&cout_str, &c4_name, &format!("CO{lane}"));

                    if lane == 3 && chunk + 1 < num_chunks {
                        let chain_co = format!("n_extracted_carry4_co_c{chain_idx}_{chunk}");
                        // Also alias/drive the inter-chunk carry net if `cout_str` is internal:
                        let _ = chain_co;
                    }

                    if let Some(snet) = sl.sum_net {
                        let snet_str = resolve(snet).to_string();
                        self.connect(&snet_str, &c4_name, &format!("O{lane}"));
                    }
                }

                // If there is a next chunk, wire the previous chunk's 4th stage `cout_net` to the next chunk's `CI`.
                if chunk > 0 {
                    let prev_cout_str = resolve(slices[chunk * 4 - 1].cout_net).to_string();
                    // Rewire `CI` onto `prev_cout_str` so `CO3` directly feeds `CI`.
                    let prev_co = intern(&format!("n_extracted_carry4_co_c{chain_idx}_{}", chunk - 1));
                    let c4_spur = intern(&c4_name);
                    for net in &mut self.nets {
                        if net.name == prev_co {
                            net.endpoints.retain(|e| e.cell != c4_spur);
                        }
                    }
                    self.rebuild_indexes();
                    self.connect(&prev_cout_str, &c4_name, "CI");
                }
            }
        }

        if !cells_to_remove.is_empty() {
            self.remove_cells_by_indices(&cells_to_remove);
        }

        stats
    }

    /// Detect 2:1 `Lut6` multiplexers combining two `Lut6` cones (or two `Muxf7` cones) and
    /// 4:1 `Lut6` multiplexers combining four `Lut6` cones, packing them into dedicated
    /// `Muxf7` (`BEL=F7AMUX`) and `Muxf8` (`BEL=F8MUX`) hardware primitives.
    pub fn pack_muxf7_muxf8_trees(&mut self) -> MuxPackStats {
        self.rebuild_indexes();
        let mut stats = MuxPackStats::default();
        let i_pins = [
            intern("I0"),
            intern("I1"),
            intern("I2"),
            intern("I3"),
            intern("I4"),
            intern("I5"),
        ];
        let o_pin = intern("O");

        // Pass 1: Decompose any 6-input 4:1 `Lut6` mux whose 4 data inputs are driven by `Lut6` cones
        // into 2 `Muxf7` + 1 `Muxf8`.
        let n = self.cells.len();
        let mut lut_out = vec![None; n];
        let mut lut_ins = vec![[None; 6]; n];
        let mut net_lut_drv: HashMap<Spur, usize> = HashMap::new();

        for net in &self.nets {
            for ep in &net.endpoints {
                let Some(&ci) = self.cell_ix.get(&ep.cell) else {
                    continue;
                };
                if !matches!(self.cells[ci].kind, CellKind::Lut6 { .. }) {
                    continue;
                }
                if ep.pin == o_pin {
                    lut_out[ci] = Some(net.name);
                    net_lut_drv.insert(net.name, ci);
                } else {
                    for (idx, &p_sp) in i_pins.iter().enumerate() {
                        if ep.pin == p_sp {
                            lut_ins[ci][idx] = Some(net.name);
                            break;
                        }
                    }
                }
            }
        }

        let mut mux4_replacements: Vec<(usize, Spur, [Spur; 4], Spur, Spur)> = Vec::new();
        for (ci, c) in self.cells.iter().enumerate() {
            if c.attrs.flag("DONT_TOUCH") || c.attrs.get("PRIM").is_some() {
                continue;
            }
            let CellKind::Lut6 { init } = c.kind else {
                continue;
            };
            let Some(out_net) = lut_out[ci] else {
                continue;
            };
            let (compact_init, active_nets) = compact_lut_active(init, &lut_ins[ci]);
            if let Some((d_nets, sel0, sel1)) = classify_mux4_lut(compact_init, &active_nets) {
                if d_nets.iter().all(|dn| net_lut_drv.contains_key(dn)) {
                    mux4_replacements.push((ci, out_net, d_nets, sel0, sel1));
                }
            }
        }

        let mut dead_mux4 = Vec::new();
        for (idx, (ci, out_net, d_nets, sel0, sel1)) in mux4_replacements.into_iter().enumerate() {
            let base_name = resolve(self.cells[ci].name).to_string();
            let out_str = resolve(out_net).to_string();
            let s0_str = resolve(sel0).to_string();
            let s1_str = resolve(sel1).to_string();
            let d0_str = resolve(d_nets[0]).to_string();
            let d1_str = resolve(d_nets[1]).to_string();
            let d2_str = resolve(d_nets[2]).to_string();
            let d3_str = resolve(d_nets[3]).to_string();

            let f7_lo_cell = format!("{base_name}_muxf7_lo_{idx}");
            let f7_hi_cell = format!("{base_name}_muxf7_hi_{idx}");
            let f8_cell = format!("{base_name}_muxf8_{idx}");
            let f7_lo_net = format!("{base_name}_n_f7_lo_{idx}");
            let f7_hi_net = format!("{base_name}_n_f7_hi_{idx}");

            // Disconnect old `mux4` cell first so `out_str` has no duplicate driver.
            let old_spur = self.cells[ci].name;
            for net in &mut self.nets {
                net.endpoints.retain(|e| e.cell != old_spur);
            }
            self.rebuild_indexes();

            self.emit_hardware_muxf(&f7_lo_cell, "MUXF7", "F7AMUX", &d0_str, &d1_str, &s0_str, &f7_lo_net);
            self.emit_hardware_muxf(&f7_hi_cell, "MUXF7", "F7AMUX", &d2_str, &d3_str, &s0_str, &f7_hi_net);
            self.emit_hardware_muxf(&f8_cell, "MUXF8", "F8MUX", &f7_lo_net, &f7_hi_net, &s1_str, &out_str);

            stats.muxf7_packed += 2;
            stats.muxf8_packed += 1;
            dead_mux4.push(ci);
        }
        if !dead_mux4.is_empty() {
            self.remove_cells_by_indices(&dead_mux4);
        }

        // Pass 2: Promote 2:1 `Lut6` muxes driven by two non-MUXF `Lut6`s into `Muxf7` BELs,
        // and then promote 2:1 `Lut6` muxes driven by two `Muxf7` BELs into `Muxf8` BELs!
        for level in 0..2 {
            self.rebuild_indexes();
            let m = self.cells.len();
            let mut cur_ins = vec![[None; 6]; m];
            let mut cur_drv: HashMap<Spur, usize> = HashMap::new();
            for net in &self.nets {
                for ep in &net.endpoints {
                    let Some(&ci) = self.cell_ix.get(&ep.cell) else {
                        continue;
                    };
                    if !matches!(self.cells[ci].kind, CellKind::Lut6 { .. }) {
                        continue;
                    }
                    if ep.pin == o_pin {
                        cur_drv.insert(net.name, ci);
                    } else {
                        for (idx, &p_sp) in i_pins.iter().enumerate() {
                            if ep.pin == p_sp {
                                cur_ins[ci][idx] = Some(net.name);
                                break;
                            }
                        }
                    }
                }
            }

            let mut mux2_cands: HashMap<usize, (Spur, Spur, Spur, usize, usize)> = HashMap::new();
            for (ci, c) in self.cells.iter().enumerate() {
                if c.attrs.flag("DONT_TOUCH") || c.attrs.get("PRIM").is_some() {
                    continue;
                }
                let CellKind::Lut6 { init } = c.kind else {
                    continue;
                };
                let (compact_init, active_nets) = compact_lut_active(init, &cur_ins[ci]);
                let Some((in0, in1, sel)) = classify_mux2_lut(compact_init, &active_nets) else {
                    continue;
                };
                let (Some(&c0), Some(&c1)) = (cur_drv.get(&in0), cur_drv.get(&in1)) else {
                    continue;
                };
                mux2_cands.insert(ci, (in0, in1, sel, c0, c1));
            }

            let mut promotions: Vec<(usize, Spur, Spur, Spur, &'static str, &'static str)> =
                Vec::new();
            for (&ci, &(in0, in1, sel, c0, c1)) in &mux2_cands {
                let p0 = self.cells[c0].attrs.get("PRIM");
                let p1 = self.cells[c1].attrs.get("PRIM");
                if level == 0
                    && p0.is_none()
                    && p1.is_none()
                    && !mux2_cands.contains_key(&c0)
                    && !mux2_cands.contains_key(&c1)
                {
                    promotions.push((ci, in0, in1, sel, "MUXF7", "F7AMUX"));
                } else if level == 1 && p0 == Some("MUXF7") && p1 == Some("MUXF7") {
                    promotions.push((ci, in0, in1, sel, "MUXF8", "F8MUX"));
                }
            }
            promotions.sort_by_key(|p| p.0);

            for (ci, in0, in1, sel, prim, bel) in promotions {
                if let CellKind::Lut6 { init } = &mut self.cells[ci].kind {
                    *init = MUX2_INIT;
                }
                self.cells[ci].attrs.set("PRIM", prim);
                self.cells[ci].attrs.set("BEL", bel);
                let cell_spur = self.cells[ci].name;
                let cell_str = resolve(cell_spur).to_string();
                for net in &mut self.nets {
                    net.endpoints
                        .retain(|e| !(e.cell == cell_spur && i_pins.contains(&e.pin)));
                }
                self.rebuild_indexes();
                self.connect(resolve(in0), &cell_str, "I0");
                self.connect(resolve(in1), &cell_str, "I1");
                self.connect(resolve(sel), &cell_str, "I2");
                if prim == "MUXF7" {
                    stats.muxf7_packed += 1;
                } else {
                    stats.muxf8_packed += 1;
                }
            }
        }

        stats
    }

    /// Detect two parallel [`CellKind::Carry4`] adder/subtractor chains feeding opposite arms
    /// of a shared select multiplexer (`sel ? (a ± b) : (c ± d)`) and fold them into a single
    /// shared [`CellKind::Carry4`] chain preceded by operand input multiplexers.
    pub fn share_resource_muxed_alu(&mut self) -> AluShareStats {
        self.rebuild_indexes();
        let mut stats = AluShareStats::default();
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
        let mut lut_out = vec![None; n];
        let mut lut_ins = vec![[None; 6]; n];
        let mut net_lut_drv: HashMap<Spur, usize> = HashMap::new();
        let mut c4_sum_driver: HashMap<Spur, (usize, usize)> = HashMap::new(); // sum_net -> (carry4_ci, lane)

        for net in &self.nets {
            for ep in &net.endpoints {
                let Some(&ci) = self.cell_ix.get(&ep.cell) else {
                    continue;
                };
                match self.cells[ci].kind {
                    CellKind::Lut6 { .. } => {
                        if ep.pin == o_pin {
                            lut_out[ci] = Some(net.name);
                            net_lut_drv.insert(net.name, ci);
                        } else {
                            for (idx, &p_sp) in i_pins.iter().enumerate() {
                                if ep.pin == p_sp {
                                    lut_ins[ci][idx] = Some(net.name);
                                    break;
                                }
                            }
                        }
                    }
                    CellKind::Carry4 => {
                        for lane in 0..4 {
                            if ep.pin == intern(&format!("O{lane}")) {
                                c4_sum_driver.insert(net.name, (ci, lane));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Group output 2:1 mux LUTs by `(sel_net, c4_when_0, c4_when_1)`.
        let mut grouped_muxes: HashMap<(Spur, usize, usize), Vec<(usize, usize, Spur)>> =
            HashMap::new();
        for (ci, c) in self.cells.iter().enumerate() {
            if c.attrs.flag("DONT_TOUCH") {
                continue;
            }
            let CellKind::Lut6 { init } = c.kind else {
                continue;
            };
            let Some(out_net) = lut_out[ci] else {
                continue;
            };
            let (compact_init, active_nets) = compact_lut_active(init, &lut_ins[ci]);
            let Some((in0, in1, sel)) = classify_mux2_lut(compact_init, &active_nets) else {
                continue;
            };
            let (Some(&(c4_0, lane0)), Some(&(c4_1, lane1))) =
                (c4_sum_driver.get(&in0), c4_sum_driver.get(&in1))
            else {
                continue;
            };
            if c4_0 != c4_1 && lane0 == lane1 {
                grouped_muxes
                    .entry((sel, c4_0, c4_1))
                    .or_default()
                    .push((lane0, ci, out_net));
            }
        }

        let mut dead_cells: Vec<usize> = Vec::new();
        let mut used_c4: HashSet<usize> = HashSet::new();

        let mut group_keys: Vec<(Spur, usize, usize)> = grouped_muxes.keys().copied().collect();
        group_keys.sort_by_key(|(_, c0, c1)| (*c0, *c1));

        for (sel_net, c4_0, c4_1) in group_keys {
            if used_c4.contains(&c4_0) || used_c4.contains(&c4_1) {
                continue;
            }
            let mut lanes = grouped_muxes.remove(&(sel_net, c4_0, c4_1)).unwrap();
            lanes.sort_by_key(|(l, _, _)| *l);
            if lanes.is_empty() {
                continue;
            }

            // Extract operand nets `(c_k, d_k)` for `c4_0` and `(a_k, b_k)` for `c4_1` on each lane.
            let c4_0_name = self.cells[c4_0].name;
            let c4_1_name = self.cells[c4_1].name;
            let cyinit_0 = self.net_on(c4_0_name, intern("CYINIT"));
            let cyinit_1 = self.net_on(c4_1_name, intern("CYINIT"));
            if cyinit_0 != cyinit_1 {
                continue;
            }

            let mut lane_info = Vec::with_capacity(lanes.len());
            let mut all_lanes_valid = true;
            for &(lane, mux_ci, mux_out_net) in &lanes {
                let s_pin = intern(&format!("S{lane}"));
                let di_pin = intern(&format!("DI{lane}"));
                let (Some(s0_net), Some(di0_net), Some(s1_net), Some(di1_net)) = (
                    self.net_on(c4_0_name, s_pin),
                    self.net_on(c4_0_name, di_pin),
                    self.net_on(c4_1_name, s_pin),
                    self.net_on(c4_1_name, di_pin),
                ) else {
                    all_lanes_valid = false;
                    break;
                };
                let (Some(&prop0_ci), Some(&prop1_ci)) =
                    (net_lut_drv.get(&s0_net), net_lut_drv.get(&s1_net))
                else {
                    all_lanes_valid = false;
                    break;
                };
                let CellKind::Lut6 { init: p0_init } = self.cells[prop0_ci].kind else {
                    all_lanes_valid = false;
                    break;
                };
                let CellKind::Lut6 { init: p1_init } = self.cells[prop1_ci].kind else {
                    all_lanes_valid = false;
                    break;
                };
                let (cp0_init, act0) = compact_lut_active(p0_init, &lut_ins[prop0_ci]);
                let (cp1_init, act1) = compact_lut_active(p1_init, &lut_ins[prop1_ci]);
                if act0.len() != 2 || act1.len() != 2 || (cp0_init & 0xF) != (cp1_init & 0xF) {
                    all_lanes_valid = false;
                    break;
                }
                let other0 = if act0[0] == di0_net { act0[1] } else { act0[0] };
                let other1 = if act1[0] == di1_net { act1[1] } else { act1[0] };
                lane_info.push((
                    lane,
                    mux_ci,
                    mux_out_net,
                    prop0_ci,
                    prop1_ci,
                    di0_net,
                    other0,
                    di1_net,
                    other1,
                ));
            }
            if !all_lanes_valid {
                continue;
            }

            used_c4.insert(c4_0);
            used_c4.insert(c4_1);
            stats.alus_shared += 1;
            stats.carry4_eliminated += 1;

            let c4_0_str = resolve(c4_0_name).to_string();
            let sel_str = resolve(sel_net).to_string();

            for (
                lane,
                mux_ci,
                mux_out_net,
                prop0_ci,
                prop1_ci,
                c_net,
                d_net,
                a_net,
                b_net,
            ) in lane_info
            {
                let mux_a_cell = format!("{c4_0_str}_shared_mux_a_l{lane}");
                let mux_b_cell = format!("{c4_0_str}_shared_mux_b_l{lane}");
                let mux_a_net = format!("{c4_0_str}_shared_na_l{lane}");
                let mux_b_net = format!("{c4_0_str}_shared_nb_l{lane}");

                self.add_cell(&mux_a_cell, CellKind::Lut6 { init: MUX2_INIT });
                self.connect(resolve(c_net), &mux_a_cell, "I0");
                self.connect(resolve(a_net), &mux_a_cell, "I1");
                self.connect(&sel_str, &mux_a_cell, "I2");
                self.connect(&mux_a_net, &mux_a_cell, "O");

                self.add_cell(&mux_b_cell, CellKind::Lut6 { init: MUX2_INIT });
                self.connect(resolve(d_net), &mux_b_cell, "I0");
                self.connect(resolve(b_net), &mux_b_cell, "I1");
                self.connect(&sel_str, &mux_b_cell, "I2");
                self.connect(&mux_b_net, &mux_b_cell, "O");

                // Rewire `prop0_ci` inputs to `(mux_a_net, mux_b_net)`.
                let prop0_spur = self.cells[prop0_ci].name;
                let prop0_str = resolve(prop0_spur).to_string();
                let di_pin_spur = intern(&format!("DI{lane}"));
                let o_lane_spur = intern(&format!("O{lane}"));
                let mux_spur = self.cells[mux_ci].name;

                for net in &mut self.nets {
                    net.endpoints.retain(|e| {
                        !(e.cell == prop0_spur && i_pins.contains(&e.pin))
                            && !(e.cell == c4_0_name && (e.pin == di_pin_spur || e.pin == o_lane_spur))
                            && e.cell != mux_spur
                    });
                }
                self.rebuild_indexes();
                self.connect(&mux_a_net, &prop0_str, "I0");
                self.connect(&mux_b_net, &prop0_str, "I1");
                self.connect(&mux_a_net, &c4_0_str, &format!("DI{lane}"));
                self.connect(resolve(mux_out_net), &c4_0_str, &format!("O{lane}"));

                dead_cells.push(mux_ci);
                dead_cells.push(prop1_ci);
            }
            dead_cells.push(c4_1);
        }

        if !dead_cells.is_empty() {
            self.remove_cells_by_indices(&dead_cells);
        }

        stats
    }

    fn emit_hardware_muxf(
        &mut self,
        name: &str,
        prim: &str,
        bel: &str,
        in0: &str,
        in1: &str,
        sel: &str,
        out: &str,
    ) {
        let mut attrs = Attrs::default();
        attrs.set("PRIM", prim);
        attrs.set("BEL", bel);
        self.push_cell(Cell {
            name: intern(name),
            kind: CellKind::Lut6 { init: MUX2_INIT },
            attrs,
        });
        self.connect(in0, name, "I0");
        self.connect(in1, name, "I1");
        self.connect(sel, name, "I2");
        self.connect(out, name, "O");
    }

    fn remove_cells_by_indices(&mut self, indices: &[usize]) {
        let dead_set: HashSet<usize> = indices.iter().copied().collect();
        let dead_names: HashSet<Spur> = dead_set
            .iter()
            .filter_map(|&i| self.cells.get(i).map(|c| c.name))
            .collect();
        for net in &mut self.nets {
            net.endpoints.retain(|e| !dead_names.contains(&e.cell));
        }
        self.nets.retain(|n| !n.endpoints.is_empty());
        let mut idx = 0usize;
        self.cells.retain(|_| {
            let keep = !dead_set.contains(&idx);
            idx += 1;
            keep
        });
        self.rebuild_indexes();
        let _ = Endpoint {
            cell: intern("0"),
            pin: intern("0"),
        };
    }
}

fn pick_initial_cin(inputs: &[Spur; 3], inv_net: Option<Spur>) -> Spur {
    for &inp in inputs {
        if Some(inp) == inv_net {
            continue;
        }
        let s = resolve(inp);
        if s.contains("cin") || s.contains("carry") || s == "0" || s == "1" || s.starts_with('c') {
            return inp;
        }
    }
    for &inp in inputs.iter().rev() {
        if Some(inp) != inv_net {
            return inp;
        }
    }
    inputs[2]
}
