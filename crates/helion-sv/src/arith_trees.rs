//! Radix-4 Modified Booth + 3:2 CSA Wallace Tree Multiplier, Non-Restoring
//! Unsigned/Signed Divider & Remainder, and Digit-by-Digit Non-Restoring
//! Integer Square-Root (`$sqrt`) synthesis into Helion `Design` netlists.

use helion_ir::{CellKind, Design};

/// LUT6 truth table for 2-input XOR (`I0 ^ I1`).
const LUT6_XOR2: u64 = 0x6666_6666_6666_6666;
/// LUT6 truth table for 3-input XOR (`I0 ^ I1 ^ I2`).
const LUT6_XOR3: u64 = 0x9696_9696_9696_9696;
/// LUT6 truth table for 3-input majority (`(I0 & I1) | (I0 & I2) | (I1 & I2)`).
const LUT6_MAJ3: u64 = 0xE8E8_E8E8_E8E8_E8E8;
/// LUT6 truth table for 1-input identity buffer (`I0`).
const LUT6_BUF: u64 = 0xAAAA_AAAA_AAAA_AAAA;
/// LUT6 truth table for 2:1 mux (`I0 ? I1 : I2`).
const LUT6_MUX2: u64 = 0xD8D8_D8D8_D8D8_D8D8;

/// Statistics from Radix-4 Booth + Wallace tree + Carry4 CPA synthesis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoothWallaceStats {
    pub booth_groups: usize,
    pub partial_product_luts: usize,
    pub wallace_stages: usize,
    pub csa_full_adders: usize,
    pub csa_half_adders: usize,
    pub carry4_cells: usize,
}

/// Statistics from non-restoring divider / remainder synthesis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DivRemStats {
    pub stages: usize,
    pub lut_cells: usize,
    pub carry4_cells: usize,
}

/// Statistics from digit-by-digit non-restoring integer square-root synthesis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IsqrtStats {
    pub stages: usize,
    pub root_bits: usize,
    pub rem_bits: usize,
    pub lut_cells: usize,
}

fn emit_lut(d: &mut Design, cell: &str, out: &str, init: u64, pins: &[&str]) {
    d.add_cell(cell, CellKind::Lut6 { init });
    d.connect(out, cell, "O");
    for (i, pin_net) in pins.iter().enumerate() {
        d.connect(pin_net, cell, &format!("I{i}"));
    }
}

fn emit_const_bit(d: &mut Design, prefix: &str, idx: &mut usize, val: bool) -> String {
    let cell = format!("{prefix}_const_{}", *idx);
    let net = format!("n_{prefix}_const_{}", *idx);
    *idx += 1;
    let init = if val { u64::MAX } else { 0 };
    emit_lut(d, &cell, &net, init, &[]);
    let _ = d.dont_touch(&cell);
    net
}

/// Compute the 64-bit LUT6 INIT for one Radix-4 Modified Booth partial-product bit:
/// - `I0 = y_{2i-1}`
/// - `I1 = y_{2i}`
/// - `I2 = y_{2i+1}` (also the Booth sign `neg_i`)
/// - `I3 = x_{j-1}` (shifted multiplicand bit for `|d| == 2`)
/// - `I4 = x_j` (unshifted multiplicand bit for `|d| == 1`)
///
/// The Booth digit is `d = -2*I2 + I1 + I0` in `{-2, -1, 0, +1, +2}`:
/// - `single = I0 ^ I1` (`|d| == 1`)
/// - `double = (~I2 & I1 & I0) | (I2 & ~I1 & ~I0)` (`|d| == 2`)
/// - `mag = (single & I4) | (double & I3)`
/// - `pp_bit = mag ^ I2`
pub const fn booth_radix4_pp_lut6_init() -> u64 {
    let mut init = 0u64;
    let mut addr = 0u32;
    while addr < 64 {
        let ym1 = (addr & 1) != 0;
        let y0 = (addr & 2) != 0;
        let y1 = (addr & 4) != 0;
        let x_prev = (addr & 8) != 0;
        let x_cur = (addr & 16) != 0;
        let single = ym1 ^ y0;
        let double = (!y1 && y0 && ym1) || (y1 && !y0 && !ym1);
        let mag = (single && x_cur) || (double && x_prev);
        let pp = mag ^ y1;
        if pp {
            init |= 1u64 << addr;
        }
        addr += 1;
    }
    init
}

/// Emit a Carry4-based CPA summing `a_nets` and `b_nets` into `out_nets`.
fn emit_carry4_cpa(
    d: &mut Design,
    prefix: &str,
    a_nets: &[Option<String>],
    b_nets: &[Option<String>],
    out_nets: &[String],
) -> usize {
    let width = out_nets.len();
    if width == 0 {
        return 0;
    }
    let mut const_idx = 0usize;
    let mut zero_net: Option<String> = None;
    let mut carry_in: Option<String> = None;
    let n_chunks = (width + 3) / 4;
    for chunk in 0..n_chunks {
        let base = chunk * 4;
        let cell = format!("u_carry4_{prefix}_{chunk}");
        d.add_cell(&cell, CellKind::Carry4);
        if chunk > 0 {
            if let Some(ci) = carry_in.take() {
                d.connect(&ci, &cell, "CI");
            }
        }
        for i in 0..4 {
            let bit = base + i;
            if bit >= width {
                break;
            }
            let an = a_nets.get(bit).and_then(|x| x.as_deref());
            let bn = b_nets.get(bit).and_then(|x| x.as_deref());
            let s_net = format!("n_carry4_{prefix}_{chunk}_s{i}");
            let s_cell = format!("u_carry4_{prefix}_{chunk}_s{i}");
            match (an, bn) {
                (Some(a), Some(b)) => {
                    d.connect(a, &cell, &format!("DI{i}"));
                    emit_lut(d, &s_cell, &s_net, LUT6_XOR2, &[a, b]);
                }
                (Some(a), None) => {
                    d.connect(a, &cell, &format!("DI{i}"));
                    emit_lut(d, &s_cell, &s_net, LUT6_BUF, &[a]);
                }
                (None, Some(b)) => {
                    let z = zero_net
                        .get_or_insert_with(|| emit_const_bit(d, prefix, &mut const_idx, false))
                        .clone();
                    d.connect(&z, &cell, &format!("DI{i}"));
                    emit_lut(d, &s_cell, &s_net, LUT6_BUF, &[b]);
                }
                (None, None) => {
                    let z = zero_net
                        .get_or_insert_with(|| emit_const_bit(d, prefix, &mut const_idx, false))
                        .clone();
                    d.connect(&z, &cell, &format!("DI{i}"));
                    emit_lut(d, &s_cell, &s_net, 0, &[]);
                }
            }
            d.connect(&s_net, &cell, &format!("S{i}"));
            d.connect(&out_nets[bit], &cell, &format!("O{i}"));
        }
        let co_net = format!("n_carry4_{prefix}_{chunk}_co3");
        d.connect(&co_net, &cell, "CO3");
        carry_in = Some(co_net);
    }
    n_chunks
}

/// Synthesize a Radix-4 Modified Booth + 3:2 CSA Wallace tree + `Carry4` CPA
/// multiplier into `d`.
///
/// - `a_nets`: LSB-first input nets for multiplicand `X` (`M` bits).
/// - `b_nets`: LSB-first input nets for multiplier `Y` (`N` bits).
/// - `out_nets`: LSB-first output nets for product `P` (`W` bits).
/// - `signed`: `true` for two's-complement signed multiplication, `false` for unsigned.
pub fn synthesize_booth_wallace_mul(
    d: &mut Design,
    prefix: &str,
    a_nets: &[String],
    b_nets: &[String],
    out_nets: &[String],
    signed: bool,
) -> BoothWallaceStats {
    let mut stats = BoothWallaceStats::default();
    let w = out_nets.len();
    if w == 0 {
        return stats;
    }
    let m = a_nets.len();
    let n = b_nets.len();
    if m == 0 || n == 0 {
        for (i, o) in out_nets.iter().enumerate() {
            emit_lut(d, &format!("u_{prefix}_zero_{i}"), o, 0, &[]);
        }
        return stats;
    }

    let mut const_idx = 0usize;
    let zero_net = emit_const_bit(d, prefix, &mut const_idx, false);

    // Extend Y so Radix-4 groups cover all N bits (plus 1 sign/zero bit so
    // unsigned MSB=1 is never misinterpreted as negative in the top group).
    let y_ext_len = if signed {
        if n % 2 == 0 {
            n
        } else {
            n + 1
        }
    } else if n % 2 == 0 {
        n + 2
    } else {
        n + 1
    };
    let num_groups = y_ext_len / 2;
    stats.booth_groups = num_groups;

    let y_bit = |idx: isize| -> String {
        if idx < 0 {
            zero_net.clone()
        } else {
            let u = idx as usize;
            if u < n {
                b_nets[u].clone()
            } else if signed {
                b_nets[n - 1].clone()
            } else {
                zero_net.clone()
            }
        }
    };

    let x_bit = |idx: isize| -> String {
        if idx < 0 {
            zero_net.clone()
        } else {
            let u = idx as usize;
            if u < m {
                a_nets[u].clone()
            } else if signed {
                a_nets[m - 1].clone()
            } else {
                zero_net.clone()
            }
        }
    };

    // Column buckets for partial product bits (0..w).
    let mut cols: Vec<Vec<String>> = vec![Vec::new(); w];
    let pp_init = booth_radix4_pp_lut6_init();

    for grp in 0..num_groups {
        let base_col = 2 * grp;
        if base_col >= w {
            break;
        }
        let ym1 = y_bit((2 * grp) as isize - 1);
        let y0 = y_bit((2 * grp) as isize);
        let y1 = y_bit((2 * grp + 1) as isize);
        let max_j = w - base_col;
        for j in 0..max_j {
            let x_prev = x_bit(j as isize - 1);
            let x_cur = x_bit(j as isize);
            let pp_cell = format!("u_{prefix}_bpp_g{grp}_b{j}");
            let pp_net = format!("n_{prefix}_bpp_g{grp}_b{j}");
            emit_lut(
                d,
                &pp_cell,
                &pp_net,
                pp_init,
                &[ym1.as_str(), y0.as_str(), y1.as_str(), x_prev.as_str(), x_cur.as_str()],
            );
            cols[base_col + j].push(pp_net);
            stats.partial_product_luts += 1;
        }
        // Inject two's-complement +neg_i correction bit (y_{2i+1}) at column 2*grp.
        cols[base_col].push(y1);
    }

    // 3:2 CSA Wallace tree reduction until every column has <= 2 bits.
    let mut stage = 0usize;
    let mut csa_id = 0usize;
    while cols.iter().any(|c| c.len() > 2) {
        let mut next_cols: Vec<Vec<String>> = vec![Vec::new(); w];
        for col in 0..w {
            let items = &cols[col];
            let mut idx = 0usize;
            while idx + 3 <= items.len() {
                let a = &items[idx];
                let b = &items[idx + 1];
                let c = &items[idx + 2];
                idx += 3;
                let sum_cell = format!("u_{prefix}_csa_s{stage}_c{col}_fa{csa_id}_s");
                let sum_net = format!("n_{prefix}_csa_s{stage}_c{col}_fa{csa_id}_s");
                emit_lut(
                    d,
                    &sum_cell,
                    &sum_net,
                    LUT6_XOR3,
                    &[a.as_str(), b.as_str(), c.as_str()],
                );
                next_cols[col].push(sum_net);
                if col + 1 < w {
                    let cout_cell = format!("u_{prefix}_csa_s{stage}_c{col}_fa{csa_id}_c");
                    let cout_net = format!("n_{prefix}_csa_s{stage}_c{col}_fa{csa_id}_c");
                    emit_lut(
                        d,
                        &cout_cell,
                        &cout_net,
                        LUT6_MAJ3,
                        &[a.as_str(), b.as_str(), c.as_str()],
                    );
                    next_cols[col + 1].push(cout_net);
                }
                csa_id += 1;
                stats.csa_full_adders += 1;
            }
            while idx < items.len() {
                next_cols[col].push(items[idx].clone());
                idx += 1;
            }
        }
        cols = next_cols;
        stage += 1;
    }
    stats.wallace_stages = stage;

    // Extract the final 2 rows (S, C) and sum them with a Carry4 CPA.
    let mut row_a: Vec<Option<String>> = Vec::with_capacity(w);
    let mut row_b: Vec<Option<String>> = Vec::with_capacity(w);
    for col in &cols {
        row_a.push(col.first().cloned());
        row_b.push(col.get(1).cloned());
    }
    stats.carry4_cells = emit_carry4_cpa(d, &format!("{prefix}_cpa"), &row_a, &row_b, out_nets);
    stats
}

/// Emit a ripple conditional two's-complement negator: `out = neg ? (-inp) : inp`.
fn emit_cond_negate(
    d: &mut Design,
    prefix: &str,
    inp: &[String],
    neg: &str,
    lut_count: &mut usize,
) -> Vec<String> {
    let w = inp.len();
    let mut out = Vec::with_capacity(w);
    // Carry chain for `(inp ^ neg) + neg`:
    // At bit 0: sum_0 = (inp[0] ^ neg) ^ neg = inp[0], carry_0 = (inp[0] ^ neg) & neg = ~inp[0] & neg.
    let mut carry = neg.to_string();
    for i in 0..w {
        // inv_i = inp[i] ^ neg
        // sum_i = inv_i ^ carry
        // cout_i = inv_i & carry
        // Both sum_i and cout_i are 3-input functions of (inp[i], neg, carry)!
        // For sum_i: I0=inp[i], I1=neg, I2=carry -> (I0 ^ I1) ^ I2 = XOR3!
        let s_cell = format!("u_{prefix}_neg_s{i}");
        let s_net = format!("n_{prefix}_neg_s{i}");
        emit_lut(
            d,
            &s_cell,
            &s_net,
            LUT6_XOR3,
            &[inp[i].as_str(), neg, carry.as_str()],
        );
        *lut_count += 1;
        out.push(s_net);
        if i + 1 < w {
            // cout_i = (I0 ^ I1) & I2 -> addr where (bit0 ^ bit1) & bit2 == 1 -> bits 5 (101) and 6 (110) = 0x60
            let c_init = 0x6060_6060_6060_6060u64;
            let c_cell = format!("u_{prefix}_neg_c{i}");
            let c_net = format!("n_{prefix}_neg_c{i}");
            emit_lut(
                d,
                &c_cell,
                &c_net,
                c_init,
                &[inp[i].as_str(), neg, carry.as_str()],
            );
            *lut_count += 1;
            carry = c_net;
        }
    }
    out
}

/// Synthesize a non-restoring unsigned or signed divider and remainder unit
/// (`Q = A / B`, `R = A % B`) with IEEE 1800 / RISC-V divide-by-zero handling
/// (`B == 0 => Q = '1, R = A`).
pub fn synthesize_nonrestoring_div_rem(
    d: &mut Design,
    prefix: &str,
    a_nets: &[String],
    b_nets: &[String],
    q_out_nets: Option<&[String]>,
    r_out_nets: Option<&[String]>,
    signed: bool,
) -> DivRemStats {
    let mut stats = DivRemStats::default();
    let w = a_nets
        .len()
        .max(b_nets.len())
        .max(q_out_nets.map(|q| q.len()).unwrap_or(0))
        .max(r_out_nets.map(|r| r.len()).unwrap_or(0));
    if w == 0 {
        return stats;
    }

    let mut const_idx = 0usize;
    let zero_net = emit_const_bit(d, prefix, &mut const_idx, false);
    let one_net = emit_const_bit(d, prefix, &mut const_idx, true);
    stats.lut_cells += 2;

    let ext_input = |nets: &[String], idx: usize| -> String {
        if nets.is_empty() {
            zero_net.clone()
        } else if idx < nets.len() {
            nets[idx].clone()
        } else if signed {
            nets[nets.len() - 1].clone()
        } else {
            zero_net.clone()
        }
    };

    let a_ext: Vec<String> = (0..w).map(|i| ext_input(a_nets, i)).collect();
    let b_ext: Vec<String> = (0..w).map(|i| ext_input(b_nets, i)).collect();

    // Detect B != 0 (OR-reduction of b_ext) and div_zero = ~b_nonzero.
    let mut b_or_level = b_ext.clone();
    let mut or_round = 0usize;
    while b_or_level.len() > 1 {
        let mut next = Vec::new();
        for (ci, chunk) in b_or_level.chunks(6).enumerate() {
            if chunk.len() == 1 {
                next.push(chunk[0].clone());
                continue;
            }
            let used = (1u64 << chunk.len()) - 1;
            let mut init = 0u64;
            for addr in 0..64u64 {
                if (addr & used) != 0 {
                    init |= 1u64 << addr;
                }
            }
            let cell = format!("u_{prefix}_bnz_r{or_round}_{ci}");
            let net = format!("n_{prefix}_bnz_r{or_round}_{ci}");
            let pins: Vec<&str> = chunk.iter().map(|s| s.as_str()).collect();
            emit_lut(d, &cell, &net, init, &pins);
            stats.lut_cells += 1;
            next.push(net);
        }
        b_or_level = next;
        or_round += 1;
    }
    let b_nonzero_net = b_or_level[0].clone();

    // Extract magnitudes |A| and |B| when signed.
    let (a_mag, b_mag, sign_a, sign_q) = if signed {
        let sa = a_ext[w - 1].clone();
        let sb = b_ext[w - 1].clone();
        let a_m = emit_cond_negate(
            d,
            &format!("{prefix}_amag"),
            &a_ext,
            &sa,
            &mut stats.lut_cells,
        );
        let b_m = emit_cond_negate(
            d,
            &format!("{prefix}_bmag"),
            &b_ext,
            &sb,
            &mut stats.lut_cells,
        );
        let sq_net = format!("n_{prefix}_sign_q");
        emit_lut(
            d,
            &format!("u_{prefix}_sign_q"),
            &sq_net,
            LUT6_XOR2,
            &[sa.as_str(), sb.as_str()],
        );
        stats.lut_cells += 1;
        (a_m, b_m, sa, sq_net)
    } else {
        (
            a_ext.clone(),
            b_ext.clone(),
            zero_net.clone(),
            zero_net.clone(),
        )
    };

    // Non-restoring division array: W stages (k = w-1 down to 0).
    // Partial remainder `rem` has W bits `rem[0..w-1]` and sign bit `rem_sign` (bit W).
    // Initially rem = 0, rem_sign = 0, so sub_mode = ~rem_sign = 1.
    let mut rem: Vec<String> = vec![zero_net.clone(); w];
    let mut sub_mode: String = one_net.clone();
    let mut q_mag: Vec<String> = vec![String::new(); w];

    // Full-adder LUT6 with conditional B inversion:
    // I0 = p_shift_j, I1 = b_mag_j, I2 = sub_mode, I3 = cin
    // b_eff = I1 ^ I2
    // sum = I0 ^ b_eff ^ I3
    // cout = (I0 & b_eff) | (I0 & I3) | (b_eff & I3)
    let mut addsub_sum_init = 0u64;
    let mut addsub_cout_init = 0u64;
    for addr in 0..64u64 {
        let p = (addr & 1) != 0;
        let b = (addr & 2) != 0;
        let sub = (addr & 4) != 0;
        let cin = (addr & 8) != 0;
        let b_eff = b ^ sub;
        let s = p ^ b_eff ^ cin;
        let c = (p && b_eff) || (p && cin) || (b_eff && cin);
        if s {
            addsub_sum_init |= 1u64 << addr;
        }
        if c {
            addsub_cout_init |= 1u64 << addr;
        }
    }

    stats.stages = w;
    for k in (0..w).rev() {
        // Shift rem left by 1, bringing in a_mag[k] at bit 0; bit W of p_shift is rem[w-1].
        let mut p_shift: Vec<String> = Vec::with_capacity(w + 1);
        p_shift.push(a_mag[k].clone());
        for j in 0..w {
            p_shift.push(rem[j].clone());
        }

        let mut cin = sub_mode.clone();
        let mut next_rem: Vec<String> = Vec::with_capacity(w);
        let mut qk_net = String::new();

        for j in 0..=w {
            let b_bit = if j < w {
                b_mag[j].as_str()
            } else {
                zero_net.as_str()
            };
            let s_cell = format!("u_{prefix}_nr_k{k}_b{j}_s");
            let s_net = format!("n_{prefix}_nr_k{k}_b{j}_s");
            let s_init = if j < w {
                addsub_sum_init
            } else {
                // At MSB (j == w), output q_mag[k] = ~sign_next directly!
                !addsub_sum_init
            };
            emit_lut(
                d,
                &s_cell,
                &s_net,
                s_init,
                &[p_shift[j].as_str(), b_bit, sub_mode.as_str(), cin.as_str()],
            );
            stats.lut_cells += 1;
            if j < w {
                next_rem.push(s_net);
                let c_cell = format!("u_{prefix}_nr_k{k}_b{j}_c");
                let c_net = format!("n_{prefix}_nr_k{k}_b{j}_c");
                emit_lut(
                    d,
                    &c_cell,
                    &c_net,
                    addsub_cout_init,
                    &[p_shift[j].as_str(), b_bit, sub_mode.as_str(), cin.as_str()],
                );
                stats.lut_cells += 1;
                cin = c_net;
            } else {
                qk_net = s_net;
            }
        }

        rem = next_rem;
        sub_mode = qk_net.clone();
        q_mag[k] = qk_net;
    }

    // Final quotient outputs: apply sign_q if signed, and force all-1s on div_zero.
    if let Some(q_outs) = q_out_nets {
        let q_signed = if signed {
            emit_cond_negate(
                d,
                &format!("{prefix}_qsign"),
                &q_mag,
                &sign_q,
                &mut stats.lut_cells,
            )
        } else {
            q_mag
        };
        // b_nonzero ? q_signed[i] : 1 = (~I0) | I1 = 0xDDDD_DDDD_DDDD_DDDD
        let q_divz_init = 0xDDDD_DDDD_DDDD_DDDDu64;
        for (i, q_out) in q_outs.iter().enumerate() {
            let src = if i < w {
                q_signed[i].as_str()
            } else if signed {
                q_signed[w - 1].as_str()
            } else {
                zero_net.as_str()
            };
            emit_lut(
                d,
                &format!("u_{prefix}_qout_{i}"),
                q_out,
                q_divz_init,
                &[b_nonzero_net.as_str(), src],
            );
            stats.lut_cells += 1;
        }
    }

    // Final remainder outputs:
    // If final remainder was negative (~sub_mode == 1), restore by adding b_mag:
    // r_mag = sub_mode ? rem : (rem + b_mag).
    if let Some(r_outs) = r_out_nets {
        // Compute restored = rem + (sub_mode ? 0 : b_mag)
        let mut cin = zero_net.clone();
        let mut r_mag = Vec::with_capacity(w);
        // For each bit j: b_gated = ~sub_mode & b_mag[j]
        // I0 = rem[j], I1 = b_mag[j], I2 = sub_mode, I3 = cin
        let mut r_sum_init = 0u64;
        let mut r_cout_init = 0u64;
        for addr in 0..64u64 {
            let r = (addr & 1) != 0;
            let b = (addr & 2) != 0;
            let sub_ok = (addr & 4) != 0;
            let c = (addr & 8) != 0;
            let bg = !sub_ok && b;
            if r ^ bg ^ c {
                r_sum_init |= 1u64 << addr;
            }
            if (r && bg) || (r && c) || (bg && c) {
                r_cout_init |= 1u64 << addr;
            }
        }
        for j in 0..w {
            let s_cell = format!("u_{prefix}_rrest_s{j}");
            let s_net = format!("n_{prefix}_rrest_s{j}");
            emit_lut(
                d,
                &s_cell,
                &s_net,
                r_sum_init,
                &[
                    rem[j].as_str(),
                    b_mag[j].as_str(),
                    sub_mode.as_str(),
                    cin.as_str(),
                ],
            );
            stats.lut_cells += 1;
            r_mag.push(s_net);
            if j + 1 < w {
                let c_cell = format!("u_{prefix}_rrest_c{j}");
                let c_net = format!("n_{prefix}_rrest_c{j}");
                emit_lut(
                    d,
                    &c_cell,
                    &c_net,
                    r_cout_init,
                    &[
                        rem[j].as_str(),
                        b_mag[j].as_str(),
                        sub_mode.as_str(),
                        cin.as_str(),
                    ],
                );
                stats.lut_cells += 1;
                cin = c_net;
            }
        }

        let r_signed = if signed {
            emit_cond_negate(
                d,
                &format!("{prefix}_rsign"),
                &r_mag,
                &sign_a,
                &mut stats.lut_cells,
            )
        } else {
            r_mag
        };

        for (i, r_out) in r_outs.iter().enumerate() {
            let r_bit = if i < w {
                r_signed[i].as_str()
            } else if signed {
                r_signed[w - 1].as_str()
            } else {
                zero_net.as_str()
            };
            let a_bit = if i < w {
                a_ext[i].as_str()
            } else if signed {
                a_ext[w - 1].as_str()
            } else {
                zero_net.as_str()
            };
            // b_nonzero ? r_bit : a_bit
            emit_lut(
                d,
                &format!("u_{prefix}_rout_{i}"),
                r_out,
                LUT6_MUX2,
                &[b_nonzero_net.as_str(), r_bit, a_bit],
            );
            stats.lut_cells += 1;
        }
    }

    stats
}

/// Synthesize a digit-by-digit non-restoring integer square-root unit
/// computing `root = floor(sqrt(X))` and optional remainder `rem = X - root^2`.
///
/// - `x_nets`: LSB-first unsigned input radicand bits (`N` bits).
/// - `root_out_nets`: LSB-first output root bits.
/// - `rem_out_nets`: optional LSB-first output remainder bits (`X - root^2`).
pub fn synthesize_isqrt(
    d: &mut Design,
    prefix: &str,
    x_nets: &[String],
    root_out_nets: &[String],
    rem_out_nets: Option<&[String]>,
) -> IsqrtStats {
    let mut stats = IsqrtStats::default();
    let n_in = x_nets.len();
    let k = (n_in + 1) / 2;
    stats.stages = k;
    stats.root_bits = root_out_nets.len();
    stats.rem_bits = rem_out_nets.map(|r| r.len()).unwrap_or(0);

    let mut const_idx = 0usize;
    let zero_net = emit_const_bit(d, prefix, &mut const_idx, false);
    let one_net = emit_const_bit(d, prefix, &mut const_idx, true);
    stats.lut_cells += 2;

    if k == 0 {
        for (i, o) in root_out_nets.iter().enumerate() {
            emit_lut(d, &format!("u_{prefix}_rzero_{i}"), o, 0, &[]);
            stats.lut_cells += 1;
        }
        if let Some(rems) = rem_out_nets {
            for (i, o) in rems.iter().enumerate() {
                emit_lut(d, &format!("u_{prefix}_remzero_{i}"), o, 0, &[]);
                stats.lut_cells += 1;
            }
        }
        return stats;
    }

    // Pad x_nets to 2*k bits (even length).
    let x_bit = |idx: usize| -> String {
        if idx < n_in {
            x_nets[idx].clone()
        } else {
            zero_net.clone()
        }
    };

    // Width of partial remainder needed across all k stages is W_R = k + 2 bits
    // (including the two's-complement sign bit at index W_R - 1).
    let wr = k + 2;
    let mut rem: Vec<String> = vec![zero_net.clone(); wr];
    // sub_mode = 1 when previous remainder >= 0 (subtract `(Y << 2) | 01`),
    // sub_mode = 0 when previous remainder < 0  (add      `(Y << 2) | 11`).
    let mut sub_mode: String = one_net.clone();
    let mut y_bits: Vec<String> = vec![zero_net.clone(); k];

    // Full-adder LUT6 with conditional inversion:
    // I0 = a_j, I1 = t_j, I2 = sub_mode, I3 = cin
    // b_eff = I1 ^ I2
    let mut addsub_sum_init = 0u64;
    let mut addsub_cout_init = 0u64;
    for addr in 0..64u64 {
        let a = (addr & 1) != 0;
        let t = (addr & 2) != 0;
        let sub = (addr & 4) != 0;
        let cin = (addr & 8) != 0;
        let b_eff = t ^ sub;
        if a ^ b_eff ^ cin {
            addsub_sum_init |= 1u64 << addr;
        }
        if (a && b_eff) || (a && cin) || (b_eff && cin) {
            addsub_cout_init |= 1u64 << addr;
        }
    }

    for step in (0..k).rev() {
        // Shift rem left by 2 and bring down x[2*step+1 : 2*step].
        let mut r_shift: Vec<String> = Vec::with_capacity(wr);
        r_shift.push(x_bit(2 * step));
        r_shift.push(x_bit(2 * step + 1));
        for j in 0..(wr - 2) {
            r_shift.push(rem[j].clone());
        }

        // Trial operand T at step `step`:
        // When sub_mode == 1 (subtract): T = (Y_{step+1} << 2) | 01_2
        // When sub_mode == 0 (add):      T = (Y_{step+1} << 2) | 11_2
        // Notice at j == 1: t_bit = ~sub_mode, so b_eff = t_bit ^ sub_mode = 1 ALWAYS!
        // We pass t_bit = one_net and use a constant-1 b_eff truth table at j == 1,
        // and at MSB (j == wr - 1) we use !addsub_sum_init to output y_net = ~sign_next directly!
        let mut b1_sum_init = 0u64;
        let mut b1_cout_init = 0u64;
        for addr in 0..64u64 {
            let a = (addr & 1) != 0;
            let cin = (addr & 8) != 0;
            let b_eff = true;
            if a ^ b_eff ^ cin {
                b1_sum_init |= 1u64 << addr;
            }
            if (a && b_eff) || (a && cin) || (b_eff && cin) {
                b1_cout_init |= 1u64 << addr;
            }
        }

        let mut cin = sub_mode.clone();
        let mut next_rem: Vec<String> = Vec::with_capacity(wr);
        let mut y_net = String::new();
        for j in 0..wr {
            let t_bit = if j == 0 || j == 1 {
                one_net.as_str()
            } else {
                let y_idx = step + 1 + (j - 2);
                if y_idx < k {
                    y_bits[y_idx].as_str()
                } else {
                    zero_net.as_str()
                }
            };
            let s_init = if j + 1 == wr {
                if j == 1 {
                    !b1_sum_init
                } else {
                    !addsub_sum_init
                }
            } else if j == 1 {
                b1_sum_init
            } else {
                addsub_sum_init
            };
            let c_init = if j == 1 {
                b1_cout_init
            } else {
                addsub_cout_init
            };
            let s_cell = format!("u_{prefix}_sq_s{step}_b{j}_s");
            let s_net = format!("n_{prefix}_sq_s{step}_b{j}_s");
            emit_lut(
                d,
                &s_cell,
                &s_net,
                s_init,
                &[r_shift[j].as_str(), t_bit, sub_mode.as_str(), cin.as_str()],
            );
            stats.lut_cells += 1;
            if j + 1 < wr {
                next_rem.push(s_net);
                let c_cell = format!("u_{prefix}_sq_s{step}_b{j}_c");
                let c_net = format!("n_{prefix}_sq_s{step}_b{j}_c");
                emit_lut(
                    d,
                    &c_cell,
                    &c_net,
                    c_init,
                    &[r_shift[j].as_str(), t_bit, sub_mode.as_str(), cin.as_str()],
                );
                stats.lut_cells += 1;
                cin = c_net;
            } else {
                y_net = s_net.clone();
                next_rem.push(s_net);
            }
        }

        rem = next_rem;
        sub_mode = y_net.clone();
        y_bits[step] = y_net;
    }

    for (i, o) in root_out_nets.iter().enumerate() {
        let src = if i < k {
            y_bits[i].as_str()
        } else {
            zero_net.as_str()
        };
        emit_lut(d, &format!("u_{prefix}_root_{i}"), o, LUT6_BUF, &[src]);
        stats.lut_cells += 1;
    }

    // Optional remainder restoration: if final remainder < 0 (~sub_mode == 1),
    // add `(Y << 1) | 1` to `rem`.
    if let Some(rem_outs) = rem_out_nets {
        let mut r_sum_init = 0u64;
        let mut r_cout_init = 0u64;
        for addr in 0..64u64 {
            let r = (addr & 1) != 0;
            let t = (addr & 2) != 0;
            let sub_ok = (addr & 4) != 0;
            let c = (addr & 8) != 0;
            let tg = !sub_ok && t;
            if r ^ tg ^ c {
                r_sum_init |= 1u64 << addr;
            }
            if (r && tg) || (r && c) || (tg && c) {
                r_cout_init |= 1u64 << addr;
            }
        }
        let mut cin = zero_net.clone();
        let mut r_rest = Vec::with_capacity(wr);
        for j in 0..wr {
            let t_bit = if j == 0 {
                one_net.as_str()
            } else if j - 1 < k {
                y_bits[j - 1].as_str()
            } else {
                zero_net.as_str()
            };
            let s_cell = format!("u_{prefix}_sq_rest_s{j}");
            let s_net = format!("n_{prefix}_sq_rest_s{j}");
            emit_lut(
                d,
                &s_cell,
                &s_net,
                r_sum_init,
                &[rem[j].as_str(), t_bit, sub_mode.as_str(), cin.as_str()],
            );
            stats.lut_cells += 1;
            r_rest.push(s_net);
            if j + 1 < wr {
                let c_cell = format!("u_{prefix}_sq_rest_c{j}");
                let c_net = format!("n_{prefix}_sq_rest_c{j}");
                emit_lut(
                    d,
                    &c_cell,
                    &c_net,
                    r_cout_init,
                    &[rem[j].as_str(), t_bit, sub_mode.as_str(), cin.as_str()],
                );
                stats.lut_cells += 1;
                cin = c_net;
            }
        }
        for (i, o) in rem_outs.iter().enumerate() {
            let src = if i < wr {
                r_rest[i].as_str()
            } else {
                zero_net.as_str()
            };
            emit_lut(d, &format!("u_{prefix}_rem_{i}"), o, LUT6_BUF, &[src]);
            stats.lut_cells += 1;
        }
    }

    stats
}
