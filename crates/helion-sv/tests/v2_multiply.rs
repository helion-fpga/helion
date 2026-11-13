//! Multiplication in the v2 front end: products by a constant are
//! shift-and-add over canonical signed digits, summed carry-save, in LUT
//! logic (smaller and shallower than a general multiplier), and with
//! HELION_V2_HARD=1 signed products wider than one Mac27 take the fewest
//! blocks. `tools/svcheck` (tests/mul_const.sv, hard/mul_const_hard.sv,
//! hard/mul_signed_wide.sv) checks the same products against a reference
//! simulator.

use helion_ir::{resolve, CellKind, Design};
use std::collections::HashMap;

fn synth(src: &str, hard: bool) -> helion_sv::v2::Synth {
    helion_sv::v2::set_hard_blocks(Some(hard));
    let s = helion_sv::synth_sv_v2(src, "t.sv", None).unwrap_or_else(|e| panic!("synthesis failed: {e}\n{src}"));
    helion_sv::v2::set_hard_blocks(None);
    s
}

/// Output `port` of a combinational LUT netlist for input values.
fn eval(d: &Design, inputs: &[(&str, u32, u64)], port: &str) -> u64 {
    let pins = d.pin_index();
    let net = |cell: &str, pin: &str| pins.net_on(cell, pin).map(|n| resolve(n).to_string());
    let mut val: HashMap<String, bool> = HashMap::new();
    for (p, w, v) in inputs {
        for i in 0..*w {
            val.insert(if *w == 1 { p.to_string() } else { format!("{p}_{i}") }, (v >> i) & 1 == 1);
        }
    }
    let luts: Vec<(u64, Vec<Option<String>>, String)> = d
        .cells
        .iter()
        .filter_map(|c| match c.kind {
            CellKind::Lut6 { init } => Some((init, (0..6).map(|i| net(resolve(c.name), &format!("I{i}"))).collect(), net(resolve(c.name), "O").unwrap())),
            _ => None,
        })
        .collect();
    for _ in 0..500 {
        let mut changed = false;
        for (init, ins, o) in &luts {
            let a = ins.iter().enumerate().fold(0u64, |a, (i, n)| a | ((n.as_ref().is_some_and(|n| val.get(n).copied().unwrap_or(false)) as u64) << i));
            let v = (init >> a) & 1 == 1;
            changed |= val.insert(o.clone(), v) != Some(v);
        }
        if !changed {
            break;
        }
    }
    d.cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::IobOut) && c.attrs.get("PORT") == Some(port))
        .fold(0, |v, c| {
            let b: u32 = c.attrs.get("BIT").unwrap().parse().unwrap();
            let n = net(resolve(c.name), "I").unwrap();
            v | ((val.get(&n).copied().unwrap_or(false) as u64) << b)
        })
}

fn macs(d: &Design) -> usize {
    d.cells.iter().filter(|c| matches!(c.kind, CellKind::Mac27)).count()
}

/// Every 8-bit input times constants with runs of ones, alternating
/// bits, all ones and negative values, unsigned and signed, with and
/// without hard blocks (a constant factor is never a block).
#[test]
fn products_by_constants_are_exact() {
    let ks: [u64; 12] = [0, 1, 3, 7, 0x5a, 0x7f, 0x81, 0xab, 0xef, 0xf0, 0xff, 0x3b];
    for hard in [false, true] {
        for &k in &ks {
            let src = format!(
                "module t(input logic [7:0] a, output logic [11:0] p, output logic signed [11:0] s, output logic [5:0] q);\n  assign p = a * 12'd{k};\n  assign s = $signed(a) * -12'sd{k};\n  assign q = a[5:0] * 6'd{};\nendmodule\n",
                k & 63
            );
            let d = synth(&src, hard).design;
            assert_eq!(macs(&d), 0, "constant {k}: no block");
            for a in 0..256u64 {
                let sa = (a as i8) as i64;
                assert_eq!(eval(&d, &[("a", 8, a)], "p"), (a * k) & 0xfff, "{a} * {k}");
                assert_eq!(eval(&d, &[("a", 8, a)], "s"), ((sa * -(k as i64)) as u64) & 0xfff, "{sa} * -{k}");
                assert_eq!(eval(&d, &[("a", 8, a)], "q"), ((a & 63) * (k & 63)) & 63, "{} * {}", a & 63, k & 63);
            }
        }
    }
}

/// A product by a constant is smaller and shallower than the general
/// multiplier of its width, and no larger or deeper than the balanced sum
/// of one shifted copy per set bit (what it was built as before); with a
/// run of ones, strictly smaller and shallower.
#[test]
fn products_by_constants_are_small_and_shallow() {
    let cases: [(u32, u64, bool); 7] = [
        (16, 255, true),
        (16, 0x0ff0, true),
        (16, 123, true),
        (16, 0x5555, false),
        (24, 0xfff0ff, true),
        (32, 1_000_003, true),
        (16, 0x7fff, true),
    ];
    for (w, k, run) in cases {
        let hi = w - 1;
        let konst = synth(&format!("module t(input logic [{hi}:0] a, output logic [{hi}:0] p);\n  assign p = a * {w}'d{k};\nendmodule\n"), false);
        let general = synth(&format!("module t(input logic [{hi}:0] a, input logic [{hi}:0] b, output logic [{hi}:0] p);\n  assign p = a * b;\nendmodule\n"), false);
        let mut terms: Vec<String> = (0..w).filter(|j| (k >> j) & 1 == 1).map(|j| format!("(a << {j})")).collect();
        while terms.len() > 1 {
            terms = terms.chunks(2).map(|c| if c.len() == 2 { format!("({} + {})", c[0], c[1]) } else { c[0].clone() }).collect();
        }
        let sum = synth(&format!("module t(input logic [{hi}:0] a, output logic [{hi}:0] p);\n  assign p = {};\nendmodule\n", terms[0]), false);
        let (c, g, s) = ((konst.luts, konst.depth), (general.luts, general.depth), (sum.luts, sum.depth));
        assert!(c.0 < g.0 && c.1 < g.1, "{w}'d{k}: {c:?} against the general multiplier's {g:?}");
        assert!(c.0 <= s.0 && c.1 <= s.1, "{w}'d{k}: {c:?} against the plain sum's {s:?}");
        if run {
            assert!(c.0 < s.0 && c.1 < s.1, "{w}'d{k}: {c:?} against the plain sum's {s:?}");
        }
    }
}

/// A run of ones costs one subtraction: `a * 16'h7fff` is no larger or
/// deeper than `(a << 15) - a` written out, nor `a * 16'h0ff0` than
/// `(a << 12) - (a << 4)`.
#[test]
fn a_run_of_ones_is_one_subtraction() {
    for (k, by_hand) in [("16'h7fff", "(a << 15) - a"), ("16'h0ff0", "(a << 12) - (a << 4)"), ("16'hfffe", "16'd0 - (a << 1)")] {
        let m = |e: &str| synth(&format!("module t(input logic [15:0] a, output logic [15:0] p);\n  assign p = {e};\nendmodule\n"), false);
        let (c, h) = (m(&format!("a * {k}")), m(by_hand));
        assert!(c.luts <= h.luts && c.depth <= h.depth, "{k}: {} LUTs, {} levels against {} and {}", c.luts, c.depth, h.luts, h.depth);
    }
}

/// Signed products wider than one block: ceil(n/16) * ceil(m/16) blocks
/// for n- and m-bit operands (the partial products reaching the result),
/// the top piece of a signed operand signed.
#[test]
fn signed_wide_products_take_the_fewest_blocks() {
    let cases: [(&str, &str, &str, usize); 9] = [
        ("signed [19:0] a, input logic signed [19:0] b", "signed [39:0]", "a * b", 4),
        ("signed [23:0] a, input logic signed [23:0] b", "signed [47:0]", "a * b", 4),
        ("signed [31:0] a, input logic signed [31:0] b", "signed [63:0]", "a * b", 4),
        ("signed [31:0] a, input logic signed [31:0] b", "signed [31:0]", "a * b", 3),
        ("signed [30:0] a, input logic signed [16:0] b", "signed [47:0]", "a * b", 4),
        ("signed [39:0] a, input logic signed [11:0] b", "signed [51:0]", "a * b", 3),
        ("signed [15:0] a, input logic [15:0] b", "signed [31:0]", "a * $signed({1'b0, b})", 1),
        ("signed [16:0] a, input logic signed [16:0] b", "signed [33:0]", "a * b", 4),
        ("[23:0] a, input logic [23:0] b", "[47:0]", "a * b", 4),
    ];
    for (ins, out, expr, want) in cases {
        let src = format!("module t(input logic {ins}, output logic {out} p);\n  assign p = {expr};\nendmodule\n");
        let d = synth(&src, true).design;
        assert_eq!(macs(&d), want, "{ins} -> {out}");
        assert_eq!(macs(&synth(&src, false).design), 0);
    }
}
