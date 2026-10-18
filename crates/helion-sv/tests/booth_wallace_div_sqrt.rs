use std::collections::HashMap;

use helion_ir::{intern, resolve, CellKind, Design};
use helion_sv::{
    booth_radix4_pp_lut6_init, synth_sv, synthesize_booth_wallace_mul, synthesize_isqrt,
    synthesize_nonrestoring_div_rem,
};

/// Evaluate a combinational `Design` netlist composed of `Lut6` and `Carry4` cells
/// to fixpoint and read out `out_nets` as an unsigned `u128` (LSB-first).
fn eval_arith_design(d: &Design, inputs: &[(&str, u128, usize)], out_nets: &[String]) -> u128 {
    let mut vals: HashMap<String, bool> = HashMap::new();
    for &(prefix, val, width) in inputs {
        if width == 1 {
            vals.insert(prefix.to_string(), (val & 1) != 0);
        }
        for i in 0..width {
            let bit = ((val >> i) & 1) != 0;
            vals.insert(format!("{prefix}_{i}"), bit);
        }
    }

    // Build a lookup from (cell_Spur, pin_Spur) -> net_name String.
    let mut pin_net: HashMap<(helion_ir::Spur, helion_ir::Spur), String> = HashMap::new();
    for net in &d.nets {
        let nname = resolve(net.name).to_string();
        for ep in &net.endpoints {
            pin_net.insert((ep.cell, ep.pin), nname.clone());
        }
    }

    let get_pin =
        |vals: &HashMap<String, bool>, cell: helion_ir::Spur, pin: &str| -> bool {
            pin_net
                .get(&(cell, intern(pin)))
                .and_then(|n| vals.get(n).copied())
                .unwrap_or(false)
        };

    // Iterate to fixpoint (cells are emitted in topological order, so 1-2 passes suffice).
    for _ in 0..8 {
        let mut changed = false;
        for cell in &d.cells {
            match cell.kind {
                CellKind::Lut6 { init } => {
                    let mut addr = 0u64;
                    for i in 0..6 {
                        if get_pin(&vals, cell.name, &format!("I{i}")) {
                            addr |= 1u64 << i;
                        }
                    }
                    let out_val = ((init >> addr) & 1) != 0;
                    if let Some(out_n) = pin_net.get(&(cell.name, intern("O"))) {
                        if vals.insert(out_n.clone(), out_val) != Some(out_val) {
                            changed = true;
                        }
                    }
                }
                CellKind::Carry4 => {
                    let mut cin = get_pin(&vals, cell.name, "CI");
                    for i in 0..4 {
                        let di = get_pin(&vals, cell.name, &format!("DI{i}"));
                        let s = get_pin(&vals, cell.name, &format!("S{i}"));
                        let o = s ^ cin;
                        let co = if s { cin } else { di };
                        if let Some(on) = pin_net.get(&(cell.name, intern(&format!("O{i}")))) {
                            if vals.insert(on.clone(), o) != Some(o) {
                                changed = true;
                            }
                        }
                        if let Some(con) = pin_net.get(&(cell.name, intern(&format!("CO{i}")))) {
                            if vals.insert(con.clone(), co) != Some(co) {
                                changed = true;
                            }
                        }
                        cin = co;
                    }
                }
                _ => {}
            }
        }
        if !changed {
            break;
        }
    }

    let mut result = 0u128;
    for (i, net) in out_nets.iter().enumerate() {
        if vals.get(net).copied().unwrap_or(false) {
            result |= 1u128 << i;
        }
    }
    result
}

#[test]
fn booth_radix4_pp_lut6_truth_table_exhaustively_matches_spec() {
    let init = booth_radix4_pp_lut6_init();
    for addr in 0..32u32 {
        let ym1 = (addr & 1) as i32;
        let y0 = ((addr >> 1) & 1) as i32;
        let y1 = ((addr >> 2) & 1) as i32;
        let x_prev = ((addr >> 3) & 1) != 0;
        let x_cur = ((addr >> 4) & 1) != 0;
        let digit = -2 * y1 + y0 + ym1;
        let mag = match digit.abs() {
            0 => false,
            1 => x_cur,
            2 => x_prev,
            _ => unreachable!(),
        };
        let expected = mag ^ (y1 != 0);
        let actual = ((init >> addr) & 1) != 0;
        assert_eq!(
            actual, expected,
            "Booth PP LUT6 mismatch at addr={addr:#07b} (digit={digit})"
        );
    }
}

#[test]
fn booth_wallace_unsigned_mul_exhaustive_4x4_and_sampled_8x8() {
    let mut d4 = Design::new("bw_u4x4");
    let a4: Vec<String> = (0..4).map(|i| format!("a_{i}")).collect();
    let b4: Vec<String> = (0..4).map(|i| format!("b_{i}")).collect();
    let p8: Vec<String> = (0..8).map(|i| format!("p_{i}")).collect();
    let st4 = synthesize_booth_wallace_mul(&mut d4, "u4", &a4, &b4, &p8, false);
    assert_eq!(st4.booth_groups, 3);
    assert!(st4.wallace_stages >= 1);
    assert!(st4.csa_full_adders >= 1);
    assert_eq!(st4.carry4_cells, 2);

    for a in 0..16u128 {
        for b in 0..16u128 {
            let got = eval_arith_design(&d4, &[("a", a, 4), ("b", b, 4)], &p8);
            assert_eq!(got, a * b, "unsigned 4x4 mismatch: {a} * {b} = {got}");
        }
    }

    let mut d8 = Design::new("bw_u8x8");
    let a8: Vec<String> = (0..8).map(|i| format!("a_{i}")).collect();
    let b8: Vec<String> = (0..8).map(|i| format!("b_{i}")).collect();
    let p16: Vec<String> = (0..16).map(|i| format!("p_{i}")).collect();
    let st8 = synthesize_booth_wallace_mul(&mut d8, "u8", &a8, &b8, &p16, false);
    assert_eq!(st8.booth_groups, 5);
    assert!(st8.wallace_stages >= 2);
    assert_eq!(st8.carry4_cells, 4);

    let samples = [
        0u128, 1, 2, 3, 7, 15, 16, 31, 37, 53, 63, 64, 127, 128, 170, 213, 254, 255,
    ];
    for &a in &samples {
        for &b in &samples {
            let got = eval_arith_design(&d8, &[("a", a, 8), ("b", b, 8)], &p16);
            assert_eq!(got, a * b, "unsigned 8x8 mismatch: {a} * {b} = {got}");
        }
    }
}

#[test]
fn booth_wallace_signed_mul_exhaustive_4x4_and_sampled_8x8() {
    let mut d4 = Design::new("bw_s4x4");
    let a4: Vec<String> = (0..4).map(|i| format!("a_{i}")).collect();
    let b4: Vec<String> = (0..4).map(|i| format!("b_{i}")).collect();
    let p8: Vec<String> = (0..8).map(|i| format!("p_{i}")).collect();
    let st4 = synthesize_booth_wallace_mul(&mut d4, "s4", &a4, &b4, &p8, true);
    assert_eq!(st4.booth_groups, 2);
    assert_eq!(st4.carry4_cells, 2);

    for a in -8i32..=7 {
        for b in -8i32..=7 {
            let au = (a as u8 & 0x0F) as u128;
            let bu = (b as u8 & 0x0F) as u128;
            let expected = ((a * b) as i8 as u8) as u128;
            let got = eval_arith_design(&d4, &[("a", au, 4), ("b", bu, 4)], &p8);
            assert_eq!(
                got, expected,
                "signed 4x4 mismatch: {a} * {b} => got {got:#04x}, expected {expected:#04x}"
            );
        }
    }

    let mut d8 = Design::new("bw_s8x8");
    let a8: Vec<String> = (0..8).map(|i| format!("a_{i}")).collect();
    let b8: Vec<String> = (0..8).map(|i| format!("b_{i}")).collect();
    let p16: Vec<String> = (0..16).map(|i| format!("p_{i}")).collect();
    let st8 = synthesize_booth_wallace_mul(&mut d8, "s8", &a8, &b8, &p16, true);
    assert_eq!(st8.booth_groups, 4);
    assert_eq!(st8.carry4_cells, 4);

    let s8_samples: [i32; 14] = [
        -128, -127, -85, -64, -37, -2, -1, 0, 1, 2, 53, 64, 126, 127,
    ];
    for &a in &s8_samples {
        for &b in &s8_samples {
            let au = (a as i8 as u8) as u128;
            let bu = (b as i8 as u8) as u128;
            let expected = ((a * b) as i16 as u16) as u128;
            let got = eval_arith_design(&d8, &[("a", au, 8), ("b", bu, 8)], &p16);
            assert_eq!(
                got, expected,
                "signed 8x8 mismatch: {a} * {b} => got {got:#06x}, expected {expected:#06x}"
            );
        }
    }
}

#[test]
fn nonrestoring_unsigned_div_rem_exhaustive_4bit_and_sampled_8bit() {
    let mut d4 = Design::new("div_u4");
    let a4: Vec<String> = (0..4).map(|i| format!("a_{i}")).collect();
    let b4: Vec<String> = (0..4).map(|i| format!("b_{i}")).collect();
    let q4: Vec<String> = (0..4).map(|i| format!("q_{i}")).collect();
    let r4: Vec<String> = (0..4).map(|i| format!("r_{i}")).collect();
    let st = synthesize_nonrestoring_div_rem(&mut d4, "du4", &a4, &b4, Some(&q4), Some(&r4), false);
    assert_eq!(st.stages, 4);
    assert!(st.lut_cells > 0);

    for a in 0..16u128 {
        for b in 0..16u128 {
            let (exp_q, exp_r) = if b == 0 { (0xFu128, a) } else { (a / b, a % b) };
            let got_q = eval_arith_design(&d4, &[("a", a, 4), ("b", b, 4)], &q4);
            let got_r = eval_arith_design(&d4, &[("a", a, 4), ("b", b, 4)], &r4);
            assert_eq!(got_q, exp_q, "unsigned 4b div mismatch: {a} / {b}");
            assert_eq!(got_r, exp_r, "unsigned 4b rem mismatch: {a} % {b}");
        }
    }

    let mut d8 = Design::new("div_u8");
    let a8: Vec<String> = (0..8).map(|i| format!("a_{i}")).collect();
    let b8: Vec<String> = (0..8).map(|i| format!("b_{i}")).collect();
    let q8: Vec<String> = (0..8).map(|i| format!("q_{i}")).collect();
    let r8: Vec<String> = (0..8).map(|i| format!("r_{i}")).collect();
    synthesize_nonrestoring_div_rem(&mut d8, "du8", &a8, &b8, Some(&q8), Some(&r8), false);

    let samples = [0u128, 1, 2, 3, 7, 13, 42, 99, 127, 128, 200, 251, 255];
    for &a in &samples {
        for &b in &samples {
            let (exp_q, exp_r) = if b == 0 {
                (0xFFu128, a)
            } else {
                (a / b, a % b)
            };
            let got_q = eval_arith_design(&d8, &[("a", a, 8), ("b", b, 8)], &q8);
            let got_r = eval_arith_design(&d8, &[("a", a, 8), ("b", b, 8)], &r8);
            assert_eq!(got_q, exp_q, "unsigned 8b div mismatch: {a} / {b}");
            assert_eq!(got_r, exp_r, "unsigned 8b rem mismatch: {a} % {b}");
        }
    }
}

#[test]
fn nonrestoring_signed_div_rem_exhaustive_5bit() {
    let mut d5 = Design::new("div_s5");
    let a5: Vec<String> = (0..5).map(|i| format!("a_{i}")).collect();
    let b5: Vec<String> = (0..5).map(|i| format!("b_{i}")).collect();
    let q5: Vec<String> = (0..5).map(|i| format!("q_{i}")).collect();
    let r5: Vec<String> = (0..5).map(|i| format!("r_{i}")).collect();
    synthesize_nonrestoring_div_rem(&mut d5, "ds5", &a5, &b5, Some(&q5), Some(&r5), true);

    for a in -15i32..=15 {
        for b in -15i32..=15 {
            let au = ((a as u32) & 0x1F) as u128;
            let bu = ((b as u32) & 0x1F) as u128;
            let (exp_q, exp_r) = if b == 0 {
                (0x1Fu128, au)
            } else {
                let q = a / b;
                let r = a % b;
                (((q as u32) & 0x1F) as u128, ((r as u32) & 0x1F) as u128)
            };
            let got_q = eval_arith_design(&d5, &[("a", au, 5), ("b", bu, 5)], &q5);
            let got_r = eval_arith_design(&d5, &[("a", au, 5), ("b", bu, 5)], &r5);
            assert_eq!(
                got_q, exp_q,
                "signed 5b div mismatch: {a} / {b} => got {got_q:#x}, expected {exp_q:#x}"
            );
            assert_eq!(
                got_r, exp_r,
                "signed 5b rem mismatch: {a} % {b} => got {got_r:#x}, expected {exp_r:#x}"
            );
        }
    }
}

#[test]
fn nonrestoring_isqrt_exhaustive_8bit_and_sampled_12bit() {
    let mut d8 = Design::new("isqrt_8");
    let x8: Vec<String> = (0..8).map(|i| format!("x_{i}")).collect();
    let y4: Vec<String> = (0..4).map(|i| format!("y_{i}")).collect();
    let r5: Vec<String> = (0..5).map(|i| format!("r_{i}")).collect();
    let st = synthesize_isqrt(&mut d8, "sq8", &x8, &y4, Some(&r5));
    assert_eq!(st.stages, 4);
    assert_eq!(st.root_bits, 4);
    assert_eq!(st.rem_bits, 5);

    for x in 0..=255u128 {
        let exp_y = x.isqrt();
        let exp_r = x - exp_y * exp_y;
        let got_y = eval_arith_design(&d8, &[("x", x, 8)], &y4);
        let got_r = eval_arith_design(&d8, &[("x", x, 8)], &r5);
        assert_eq!(got_y, exp_y, "8b isqrt root mismatch for x={x}");
        assert_eq!(got_r, exp_r, "8b isqrt rem mismatch for x={x}");
    }

    let mut d12 = Design::new("isqrt_12");
    let x12: Vec<String> = (0..12).map(|i| format!("x_{i}")).collect();
    let y6: Vec<String> = (0..6).map(|i| format!("y_{i}")).collect();
    let r7: Vec<String> = (0..7).map(|i| format!("r_{i}")).collect();
    synthesize_isqrt(&mut d12, "sq12", &x12, &y6, Some(&r7));

    for x in [0u128, 1, 2, 3, 4, 8, 9, 15, 16, 99, 100, 1023, 1024, 2048, 3969, 4095] {
        let exp_y = x.isqrt();
        let exp_r = x - exp_y * exp_y;
        let got_y = eval_arith_design(&d12, &[("x", x, 12)], &y6);
        let got_r = eval_arith_design(&d12, &[("x", x, 12)], &r7);
        assert_eq!(got_y, exp_y, "12b isqrt root mismatch for x={x}");
        assert_eq!(got_r, exp_r, "12b isqrt rem mismatch for x={x}");
    }
}

#[test]
fn sv_end_to_end_div_rem_sqrt_and_booth_mul_synthesis() {
    let sv = r#"
module arith_top (
    input  logic [7:0] a,
    input  logic [7:0] b,
    output logic [7:0] q_div,
    output logic [7:0] r_mod,
    output logic [3:0] root_a,
    output logic [7:0] prod_bw
);
    assign q_div   = a / b;
    assign r_mod   = a % b;
    assign root_a  = $sqrt(a);
    assign prod_bw = $booth_mul(a, b);
endmodule
"#;
    let d = synth_sv(sv, "arith_top").expect("synth arith_top");
    let q_nets: Vec<String> = (0..8).map(|i| format!("q_div_{i}")).collect();
    let r_nets: Vec<String> = (0..8).map(|i| format!("r_mod_{i}")).collect();
    let root_nets: Vec<String> = (0..4).map(|i| format!("root_a_{i}")).collect();
    let prod_nets: Vec<String> = (0..8).map(|i| format!("prod_bw_{i}")).collect();

    for &(a, b) in &[
        (0u128, 1u128),
        (100, 7),
        (255, 16),
        (144, 12),
        (81, 0),
        (17, 13),
    ] {
        let inputs = [("a", a, 8usize), ("b", b, 8usize)];
        let got_q = eval_arith_design(&d, &inputs, &q_nets);
        let got_r = eval_arith_design(&d, &inputs, &r_nets);
        let got_root = eval_arith_design(&d, &inputs, &root_nets);
        let got_prod = eval_arith_design(&d, &inputs, &prod_nets);

        let exp_q = if b == 0 { 0xFF } else { a / b };
        let exp_r = if b == 0 { a } else { a % b };
        let exp_root = a.isqrt();
        let exp_prod = (a * b) & 0xFF;

        assert_eq!(got_q, exp_q, "SV end-to-end a/b mismatch for {a}/{b}");
        assert_eq!(got_r, exp_r, "SV end-to-end a%b mismatch for {a}%{b}");
        assert_eq!(got_root, exp_root, "SV end-to-end $sqrt(a) mismatch for {a}");
        assert_eq!(
            got_prod, exp_prod,
            "SV end-to-end $booth_mul(a,b) mismatch for {a}*{b}"
        );
    }
}
