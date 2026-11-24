//! Where `*` lands, per front end (HELION_GAPS.md G3a).
//!
//! Until 2026-11-19 `v2/` had no DSP code: an 18x18 registered multiply
//! through `synth_sv_v2` gave 0 Dsp48e1 and 2012 Lut6, so everything built
//! through zedbit's default front end got LUT multipliers (`"dsp":0.0` in
//! every zedbit summary). `v2/dsp.rs` now maps every product that fits one
//! DSP48E1 multiplier exactly (25 x 18 signed) onto a `DSP48E1` cell and
//! moves the registers around it into the slice when they match exactly.
//! Behaviour is checked against the RTL by `tools/svcheck`
//! (`tools/svcheck/tests_hard/dsp_*.sv`); these tests pin the netlist shape.
//!
//! A product wider than one multiplier, such as the unsigned 18x18 below
//! (19x19 signed), is split into partial products that fit, added in the
//! fabric (G3a stage 2). The legacy front end puts that one in a single
//! DSP; whether its mapping keeps `b[17]` unsigned is not checked here.

use helion_ir::{resolve, CellKind, Design};
use std::collections::HashMap;

/// Flip to `true` when v2 gains DSP inference, and update G3a in the ledger.
const V2_INFERS_DSP: bool = true;

const MUL: &str = "module m(input logic clk, input logic [17:0] a, input logic [17:0] b, output logic [35:0] p);\nalways_ff @(posedge clk) p <= a * b;\nendmodule\n";

const MUL_SIGNED: &str = "module m(input logic clk, input logic signed [24:0] a, input logic signed [17:0] b, output logic signed [42:0] p);\nalways_ff @(posedge clk) p <= a * b;\nendmodule\n";

fn count(d: &helion_ir::Design, want_dsp: bool) -> usize {
    d.cells
        .iter()
        .filter(|c| match (&c.kind, want_dsp) {
            (CellKind::Dsp48e1 { .. }, true) => true,
            (CellKind::BlackBox { module }, true) => resolve(*module) == "DSP48E1",
            (CellKind::Lut6 { .. }, false) => true,
            _ => false,
        })
        .count()
}

/// The v2 front end as zedbit runs it: hard blocks and carry chains.
fn v2(src: &str) -> Design {
    helion_sv::v2::set_hard(true);
    helion_sv::v2::dag::set_carry_arith(true);
    helion_sv::synth_sv_v2(src, "t.sv", None).expect("v2 synth").design
}

/// The DSP48E1 cells: name -> attributes.
fn dsps(d: &Design) -> Vec<(String, HashMap<String, String>)> {
    d.cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::BlackBox { module } if resolve(module) == "DSP48E1"))
        .map(|c| (resolve(c.name).to_string(), c.attrs.map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()))
        .collect()
}

fn net(d: &Design, cell: &str, pin: &str) -> Option<String> {
    d.pin_index().net_on(cell, pin).map(|n| resolve(n).to_string())
}

/// OPMODE as driven by the cell's constant pins.
fn opmode(d: &Design, cell: &str) -> u32 {
    (0..7).map(|i| ((net(d, cell, &format!("OPMODE{i}")).as_deref() == Some("const1")) as u32) << i).sum()
}

fn p_bits(d: &Design, cell: &str) -> usize {
    (0..48).filter(|i| net(d, cell, &format!("P{i}")).is_some()).count()
}

fn flops(d: &Design) -> usize {
    d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count()
}

/// The one DSP of `src`, with its registers as "AREG BREG MREG PREG".
fn one(src: &str) -> (Design, String, String) {
    let d = v2(src);
    let ds = dsps(&d);
    assert_eq!(ds.len(), 1, "expected one DSP48E1, got {:?}", ds.iter().map(|x| &x.0).collect::<Vec<_>>());
    let (name, a) = ds.into_iter().next().unwrap();
    let regs = ["AREG", "BREG", "MREG", "PREG"].iter().map(|k| a[*k].as_str()).collect::<Vec<_>>().join(" ");
    (d, name, regs)
}

/// The legacy front end infers a DSP48E1 for a registered multiply. This is the
/// same shape `dsp_before_flatten_cap.rs` relies on, checked outside the cap.
#[test]
fn legacy_front_end_infers_a_dsp_for_a_registered_multiply() {
    let d = helion_sv::synth_sv(MUL, "m.sv").expect("legacy synth");
    assert_eq!(
        count(&d, true),
        1,
        "legacy `synth_sv` used to infer exactly one Dsp48e1 for `p <= a * b`"
    );
}

/// v2 infers a DSP for a registered multiply that fits, with the product
/// register inside the slice and no LUT multiplier left.
#[test]
fn v2_multiply_lands_in_a_dsp() {
    let d = v2(MUL_SIGNED);
    let (dsp, luts) = (count(&d, true), count(&d, false));
    if !V2_INFERS_DSP {
        assert_eq!(dsp, 0, "v2 now infers a Dsp48e1: set V2_INFERS_DSP = true and close G3a in HELION_GAPS.md");
        return;
    }
    assert_eq!(dsp, 1, "V2_INFERS_DSP is true but v2 produced {dsp} DSP48E1 ({luts} Lut6)");
    // Only the constant drivers of the tied pins remain.
    assert!(luts <= 2, "expected no LUT multiplier, got {luts} Lut6");
    assert_eq!(flops(&d), 0, "p is the slice's P register");
}

/// Unsigned 18x18 needs a 19x19 signed multiplier: one DSP48E1 takes
/// a x b[16:0] and b[17] is one row of AND gates added in the fabric.
#[test]
fn an_unsigned_18x18_is_one_dsp_and_one_fabric_row() {
    let d = v2(MUL);
    assert_eq!(count(&d, true), 1);
    let luts = count(&d, false);
    assert!(luts < 60, "expected one 19-bit row and its add, got {luts} Lut6 (2012 as a LUT multiplier)");
    assert_eq!(flops(&d), 36, "a partial product keeps p in flops");
}

#[test]
fn the_front_ends_agree_on_products_up_to_18x18() {
    let legacy = |s: &str| count(&helion_sv::synth_sv(s, "m.sv").unwrap(), true);
    assert_eq!((legacy(MUL_SIGNED), count(&v2(MUL_SIGNED), true)), (1, 1));
    assert_eq!((legacy(MUL), count(&v2(MUL), true)), (1, 1));
}

// ---- products wider than one multiplier ----------------------------------

/// The DSPs of `src` with their registers; every one combinational P = M.
fn split(src: &str) -> (Design, usize) {
    let d = v2(src);
    let ds = dsps(&d);
    for (name, a) in &ds {
        let regs: Vec<&str> = ["AREG", "BREG", "MREG", "PREG"].iter().map(|k| a[*k].as_str()).collect();
        assert_eq!(regs, ["0", "0", "0", "0"], "{name}: a partial product is combinational");
        assert_eq!(opmode(&d, name), 0b0000101, "{name}: P = M");
    }
    let n = ds.len();
    (d, n)
}

/// 32 x 32 to 64 bits (RISC-V MULH, MULHU): each operand is a 24- or
/// 17-bit unsigned chunk under a signed top chunk, so four partial products.
#[test]
fn a_32x32_product_is_four_partial_products() {
    for (sign, what) in [("signed", "signed"), ("", "unsigned")] {
        let (d, n) = split(&format!("module t(input logic {sign} [31:0] a, b, output logic {sign} [63:0] p); assign p = a * b; endmodule"));
        assert_eq!(n, 4, "{what}");
        let luts = count(&d, false);
        assert!(luts < 200, "{what}: three adds in the fabric, got {luts} Lut6");
    }
}

/// RISC-V MUL keeps the low 32 bits: the partial product of the two top
/// chunks starts at bit 41 and is dropped.
#[test]
fn a_truncated_32x32_drops_the_partial_product_above_it() {
    let (_, n) = split("module t(input logic [31:0] a, b, output logic [31:0] p); assign p = a * b; endmodule");
    assert_eq!(n, 3);
}

/// Signed 26 x 19 splits either operand: 26 bits over B's 17 takes two
/// partial products, over A's 24 and 19 over B's 17 would take four.
#[test]
fn the_split_takes_the_way_with_fewer_partial_products() {
    let (_, n) = split("module t(input logic signed [25:0] a, input logic signed [18:0] b, output logic signed [44:0] p); assign p = a * b; endmodule");
    assert_eq!(n, 2);
}

/// The slice registers would delay one partial product against the sum
/// that reads it, so a registered wide product keeps its flops.
#[test]
fn a_registered_wide_product_keeps_its_registers_in_the_fabric() {
    let (d, n) = split(
        "module t(input logic clk, input logic signed [31:0] a, b, output logic signed [63:0] p);
         logic signed [31:0] ra, rb; always_ff @(posedge clk) begin ra <= a; rb <= b; p <= ra * rb; end endmodule",
    );
    assert_eq!(n, 4);
    assert_eq!(flops(&d), 128, "ra, rb and p stay flops");
}

// ---- operand shapes ----------------------------------------------------

#[test]
fn signed_operands_sign_extend_onto_the_ports() {
    let (d, c, regs) = one("module t(input logic signed [24:0] a, input logic signed [17:0] b, output logic signed [42:0] p); assign p = a * b; endmodule");
    assert_eq!(regs, "0 0 0 0");
    assert_eq!(opmode(&d, &c), 0b0000101, "P = M");
    assert_eq!(net(&d, &c, "A24").as_deref(), Some("a_24"));
    assert_eq!(net(&d, &c, "A29").as_deref(), Some("a_24"), "A[29:25] repeat the sign");
    assert_eq!(net(&d, &c, "B17").as_deref(), Some("b_17"));
    assert_eq!(p_bits(&d, &c), 43);
    assert_eq!(net(&d, &c, "CLK"), None, "no register, no clock");
}

#[test]
fn unsigned_operands_zero_extend_up_to_24x17() {
    let (d, c, _) = one("module t(input logic [23:0] a, input logic [16:0] b, output logic [40:0] p); assign p = a * b; endmodule");
    assert_eq!(net(&d, &c, "A23").as_deref(), Some("a_23"));
    assert_eq!(net(&d, &c, "A24").as_deref(), Some("const0"));
    assert_eq!(net(&d, &c, "B16").as_deref(), Some("b_16"));
    assert_eq!(net(&d, &c, "B17").as_deref(), Some("const0"));
    // 25 x 17 unsigned is 26 x 18 signed, more than A takes: split, b on A
    // times a[16:0] and a[24:17].
    assert_eq!(dsps(&v2("module t(input logic [24:0] a, input logic [16:0] b, output logic [41:0] p); assign p = a * b; endmodule")).len(), 2);
}

#[test]
fn a_signed_times_unsigned_product_is_unsigned() {
    // SystemVerilog: one unsigned operand makes both unsigned, so s is zero-extended.
    let (d, c, _) = one("module t(input logic signed [19:0] s, input logic [16:0] u, output logic [47:0] p); assign p = s * u; endmodule");
    assert_eq!(net(&d, &c, "A19").as_deref(), Some("s_19"));
    assert_eq!(net(&d, &c, "A20").as_deref(), Some("const0"));
    // A 25-bit signed operand becomes 25 bits unsigned: 26 signed bits, split.
    assert_eq!(dsps(&v2("module t(input logic signed [24:0] s, input logic [16:0] u, output logic [47:0] p); assign p = s * u; endmodule")).len(), 2);
}

#[test]
fn either_operand_order_takes_the_wide_one_on_a() {
    let (d, c, _) = one("module t(input logic signed [24:0] a, input logic signed [17:0] b, output logic signed [42:0] p); assign p = b * a; endmodule");
    assert_eq!(net(&d, &c, "A24").as_deref(), Some("a_24"));
    assert_eq!(net(&d, &c, "B17").as_deref(), Some("b_17"));
}

#[test]
fn a_truncated_result_reads_only_its_bits() {
    let (d, c, _) = one("module t(input logic [15:0] a, b, output logic [15:0] p); assign p = a * b; endmodule");
    assert_eq!(p_bits(&d, &c), 16);
}

#[test]
fn a_result_wider_than_the_product_repeats_its_top_bit() {
    // 20 x 12 signed fits in 32 bits: P[31] gives bits 31..63.
    let (d, c, _) = one("module t(input logic signed [19:0] a, input logic signed [11:0] b, output logic signed [63:0] p); assign p = a * b; endmodule");
    assert_eq!(p_bits(&d, &c), 32);
}

#[test]
fn constants_with_one_adder_stay_in_the_fabric() {
    let n = |e: &str| dsps(&v2(&format!("module t(input logic [15:0] a, input logic b, output logic [31:0] p); assign p = {e}; endmodule"))).len();
    assert_eq!(n("a * 3"), 0, "x * 3 is x + 2x");
    assert_eq!(n("a * 10"), 0);
    assert_eq!(n("a * 16"), 0, "a power of two is a shift");
    assert_eq!(n("a * b"), 0, "a 1-bit operand is an AND");
    assert_eq!(n("a * 7"), 1);
    assert_eq!(n("a * 100"), 1);
    assert_eq!(n("$signed(a) * -3"), 1, "-3 is a subtraction and a sign row: 67 LUTs");
}

#[test]
fn one_product_used_twice_is_one_dsp() {
    let d = v2("module t(input logic signed [24:0] a, input logic signed [17:0] b, output logic signed [42:0] p, q); assign p = a * b; assign q = a * b; endmodule");
    assert_eq!(dsps(&d).len(), 1);
}

// ---- registers -----------------------------------------------------------

const IO: &str = "input logic clk, rst, en, input logic signed [24:0] a, input logic signed [17:0] b";

#[test]
fn product_register_is_preg() {
    let (d, c, regs) = one(&format!("module t({IO}, output logic signed [42:0] p); always_ff @(posedge clk) p <= a * b; endmodule"));
    assert_eq!(regs, "0 0 0 1");
    assert_eq!(net(&d, &c, "CLK").as_deref(), Some("clk"));
    assert_eq!(net(&d, &c, "CEP").as_deref(), Some("const1"));
    assert_eq!(net(&d, &c, "RSTP").as_deref(), Some("const0"));
}

#[test]
fn clock_enable_and_sync_reset_go_to_ce_and_rst() {
    let (d, c, regs) = one(&format!("module t({IO}, output logic signed [42:0] p); always_ff @(posedge clk) if (rst) p <= '0; else if (en) p <= a * b; endmodule"));
    assert_eq!(regs, "0 0 0 1");
    assert_eq!(net(&d, &c, "CEP").as_deref(), Some("en"));
    assert_eq!(net(&d, &c, "RSTP").as_deref(), Some("rst"));
    assert_eq!(flops(&d), 0);
}

#[test]
fn input_registers_are_areg_and_breg() {
    let (d, c, regs) = one(&format!(
        "module t({IO}, output logic signed [42:0] p); logic signed [24:0] ar; logic signed [17:0] br;
         always_ff @(posedge clk) begin if (en) ar <= a; br <= b; p <= ar * br; end endmodule"
    ));
    assert_eq!(regs, "1 1 0 1");
    assert_eq!(net(&d, &c, "A0").as_deref(), Some("a_0"), "A takes the register's input");
    assert_eq!(net(&d, &c, "CEA2").as_deref(), Some("en"));
    assert_eq!(net(&d, &c, "CEB2").as_deref(), Some("const1"));
    assert_eq!(flops(&d), 0);
}

#[test]
fn an_input_register_read_elsewhere_stays_in_the_fabric() {
    let (d, _, regs) = one(&format!(
        "module t({IO}, output logic signed [42:0] p, output logic signed [24:0] o); logic signed [24:0] ar;
         always_ff @(posedge clk) begin ar <= a; p <= ar * b; end assign o = ar; endmodule"
    ));
    assert_eq!(regs, "0 0 0 1");
    assert_eq!(flops(&d), 25, "ar's flops feed o and A");
}

#[test]
fn an_input_register_beside_a_constant_1_stays_in_the_fabric() {
    // The 1 would go through AREG, which is 0 after configuration and reset.
    let (d, _, regs) = one(&format!(
        "module t({IO}, output logic [31:0] p); logic [11:0] r;
         always_ff @(posedge clk) if (rst) r <= '0; else if (en) r <= a[11:0]; assign p = {{1'b1, r}} * b[15:0]; endmodule"
    ));
    assert_eq!(regs, "0 0 0 0");
    assert_eq!(flops(&d), 12);
}

#[test]
fn product_then_register_is_mreg_and_preg() {
    let (d, c, regs) = one(&format!(
        "module t({IO}, output logic signed [47:0] p); logic signed [47:0] m;
         always_ff @(posedge clk) begin m <= a * b; p <= m; end endmodule"
    ));
    assert_eq!(regs, "0 0 1 1");
    assert_eq!(opmode(&d, &c), 0b0000101);
}

#[test]
fn accumulate_is_opmode_z_p() {
    let (d, c, regs) = one(&format!("module t({IO}, output logic signed [47:0] p); always_ff @(posedge clk) p <= p + a * b; endmodule"));
    assert_eq!(regs, "0 0 0 1");
    assert_eq!(opmode(&d, &c), 0b0100101, "P <= P + M");
    assert_eq!(p_bits(&d, &c), 48);
}

#[test]
fn the_dsp_fill_lane_is_one_slice() {
    // examples/zedboard_dsp_fill.sv's lane: A, B, M and P, accumulate, synchronous reset.
    let (d, c, regs) = one(&format!(
        "module t({IO}, output logic signed [47:0] acc); logic signed [24:0] ar; logic signed [17:0] br; logic signed [47:0] m;
         always_ff @(posedge clk) if (rst) begin ar <= '0; br <= '0; m <= '0; acc <= '0; end
           else if (en) begin ar <= a; br <= b; m <= ar * br; acc <= acc + m; end endmodule"
    ));
    assert_eq!(regs, "1 1 1 1");
    assert_eq!(opmode(&d, &c), 0b0100101);
    for p in ["RSTA", "RSTB", "RSTM", "RSTP"] {
        assert_eq!(net(&d, &c, p).as_deref(), Some("rst"), "{p}");
    }
    for p in ["CEA2", "CEB2", "CEM", "CEP"] {
        assert_eq!(net(&d, &c, p).as_deref(), Some("en"), "{p}");
    }
    assert_eq!(flops(&d), 0);
}

/// Registers the slice cannot hold exactly stay flops behind a combinational DSP.
#[test]
fn registers_the_slice_cannot_match_stay_in_the_fabric() {
    for (what, body) in [
        ("asynchronous reset", "always_ff @(posedge clk or posedge rst) if (rst) p <= '0; else p <= a * b;"),
        ("power-up value", "initial p = 5; always_ff @(posedge clk) p <= a * b;"),
        ("reset to 1", "always_ff @(posedge clk) if (rst) p <= 1; else p <= a * b;"),
        ("reset under the enable", "always_ff @(posedge clk) if (en) begin if (rst) p <= '0; else p <= a * b; end"),
        ("falling edge", "always_ff @(negedge clk) p <= a * b;"),
        ("product also read", "always_ff @(posedge clk) p <= a * b; assign o = a * b;"),
        ("select also read", "always_ff @(posedge clk) if (en) p <= a * b; assign o = en ? a * b : p;"),
    ] {
        let (d, _, regs) = one(&format!("module t({IO}, output logic signed [42:0] p, o); {body} endmodule"));
        assert_eq!(regs, "0 0 0 0", "{what}");
        assert_eq!(flops(&d), 43, "{what}: p stays 43 flops");
    }
}

#[test]
fn an_m_register_read_elsewhere_is_p_instead() {
    let (d, _, regs) = one(&format!(
        "module t({IO}, output logic signed [47:0] p, o); logic signed [47:0] m;
         always_ff @(posedge clk) begin m <= a * b; p <= m; end assign o = m; endmodule"
    ));
    assert_eq!(regs, "0 0 0 1");
    assert_eq!(flops(&d), 48, "p is a flop after m");
}

#[test]
fn an_accumulator_over_48_bits_keeps_its_add_in_the_fabric() {
    let (d, _, regs) = one(&format!("module t({IO}, output logic signed [63:0] p); always_ff @(posedge clk) p <= p + a * b; endmodule"));
    assert_eq!(regs, "0 0 0 0");
    assert_eq!(flops(&d), 64);
}

/// `q <= p1 * c` reads the first slice's P register whichever block comes
/// first in the source. Consumer first used to take p1 as the second
/// slice's A register, leaving `a * b` a combinational DSP in front of it
/// (A -> P then A setup: zedbit refused the bitstream).
#[test]
fn a_product_register_read_by_another_product_is_its_p_in_either_order() {
    let (prod, cons) = ("always_ff @(posedge clk) p1 <= a * b;", "always_ff @(posedge clk) q <= p1 * c;");
    for (what, body) in [("producer first", format!("{prod} {cons}")), ("consumer first", format!("{cons} {prod}"))] {
        let d = v2(&format!(
            "module t(input logic clk, input logic signed [11:0] a, b, input logic signed [17:0] c, output logic signed [41:0] q);
             logic signed [23:0] p1; {body} endmodule"
        ));
        let mut ds: Vec<(String, String)> =
            dsps(&d).into_iter().map(|(n, a)| (n.split("_dsp").next().unwrap().to_string(), ["AREG", "BREG", "MREG", "PREG"].map(|k| a[k].as_str()).join(" "))).collect();
        ds.sort();
        assert_eq!(ds, [("p1".to_string(), "0 0 0 1".to_string()), ("q".to_string(), "0 0 0 1".to_string())], "{what}");
        assert!(count(&d, false) <= 2, "{what}: no LUT multiplier, got {} Lut6", count(&d, false));
        assert_eq!(flops(&d), 0, "{what}");
    }
}

/// A register on another clock, or behind a clock gate, is not the slice's.
#[test]
fn registers_on_another_or_gated_clock_stay_in_the_fabric() {
    let gate = "logic en_l, gclk; always_latch if (!clk) en_l = en; assign gclk = clk & en_l;";
    for (what, body, regs, ff) in [
        ("A on clk2", "logic signed [24:0] ar; always_ff @(posedge clk2) ar <= a; always_ff @(posedge clk) p <= ar * b;".to_string(), "0 0 0 1", 25),
        ("M on clk, P on clk2", "logic signed [47:0] m; always_ff @(posedge clk) m <= a * b; always_ff @(posedge clk2) p <= m;".to_string(), "0 0 0 1", 48),
        ("P on a gated clock", format!("{gate} always_ff @(posedge gclk) p <= a * b;"), "0 0 0 0", 48),
        ("A on a gated clock", format!("{gate} logic signed [24:0] ar; always_ff @(posedge gclk) ar <= a; assign p = ar * b;"), "0 0 0 0", 25),
    ] {
        let (d, _, r) = one(&format!("module t({IO}, input logic clk2, output logic signed [47:0] p); {body} endmodule"));
        assert_eq!(r, regs, "{what}");
        assert_eq!(flops(&d), ff, "{what}");
    }
}

/// Products past the part's DSP48E1 count stay LUT multipliers.
#[test]
fn products_past_the_dsp_sites_stay_in_the_fabric() {
    let n = helion_sv::v2::dsp::DSP_SITES.load(std::sync::atomic::Ordering::Relaxed);
    let k = n + 10;
    let d = v2(&format!(
        "module t(input logic [{w}:0] a, b, output logic [{w2}:0] p);
         for (genvar i = 0; i < {k}; i++) begin : g assign p[i*16 +: 16] = a[i*8 +: 8] * b[i*8 +: 8]; end endmodule",
        w = k * 8 - 1,
        w2 = k * 16 - 1
    ));
    assert_eq!(dsps(&d).len(), n);
    assert!(count(&d, false) > 10 * 50, "ten 8x8 LUT multipliers");
}

/// Instantiated DSP48E1 cells take sites first: inference fills what is left.
#[test]
fn instantiated_dsps_count_against_the_sites() {
    let n = helion_sv::v2::dsp::DSP_SITES.load(std::sync::atomic::Ordering::Relaxed);
    let inst = "DSP48E1 #(.AREG(0),.BREG(0),.MREG(0),.PREG(0),.ACASCREG(0),.BCASCREG(0),.CREG(0),.DREG(0),.ADREG(0),.INMODEREG(0),.OPMODEREG(0),.ALUMODEREG(0),.CARRYINREG(0),.CARRYINSELREG(0))";
    let d = v2(&format!(
        "module t(input logic [{w}:0] a, b, input logic [29:0] x, input logic [17:0] y, output logic [{w2}:0] p, output logic [47:0] o0, o1, o2);
         for (genvar i = 0; i < {n}; i++) begin : g assign p[i*16 +: 16] = a[i*8 +: 8] * b[i*8 +: 8]; end
         {inst} d0 (.A(x), .B(y), .P(o0), .OPMODE(7'b0000101));
         {inst} d1 (.A(x), .B(y), .P(o1), .OPMODE(7'b0000101));
         {inst} d2 (.A(x), .B(y), .P(o2), .OPMODE(7'b0000101)); endmodule",
        w = n * 8 - 1,
        w2 = n * 16 - 1
    ));
    let ds = dsps(&d);
    assert_eq!(ds.len(), n);
    assert_eq!(ds.iter().filter(|(c, _)| c.contains("_dsp")).count(), n - 3, "three sites already taken");
}

/// `m <= a * b; p1 <= m; q <= p1 * c`: p1 is the first slice's P behind its
/// M register, never q's A register, whatever order the source lists them in.
#[test]
fn an_m_then_p_pair_read_by_another_product_is_the_same_in_any_order() {
    let st = ["always_ff @(posedge clk) m <= a * b;", "always_ff @(posedge clk) p1 <= m;", "always_ff @(posedge clk) q <= p1 * c;"];
    for o in [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]] {
        let body = o.map(|i| st[i]).join(" ");
        let d = v2(&format!(
            "module t(input logic clk, input logic signed [11:0] a, b, input logic signed [17:0] c, output logic signed [41:0] q);
             logic signed [23:0] m, p1; {body} endmodule"
        ));
        let mut ds: Vec<(String, String)> =
            dsps(&d).into_iter().map(|(n, a)| (n.split("_dsp").next().unwrap().to_string(), ["AREG", "BREG", "MREG", "PREG"].map(|k| a[k].as_str()).join(" "))).collect();
        ds.sort();
        assert_eq!(ds, [("p1".to_string(), "0 0 1 1".to_string()), ("q".to_string(), "0 0 0 1".to_string())], "order {o:?}");
        assert_eq!(flops(&d), 0, "order {o:?}");
    }
}

/// Every order of the statements in `st`.
fn orders(st: &[&str]) -> Vec<String> {
    if st.len() <= 1 {
        return st.iter().map(|s| s.to_string()).collect();
    }
    (0..st.len())
        .flat_map(|i| {
            let mut rest = st.to_vec();
            let s = rest.remove(i);
            orders(&rest).into_iter().map(move |r| format!("{s} {r}"))
        })
        .collect()
}

fn regs_of(d: &Design) -> Vec<(String, String)> {
    let mut ds: Vec<(String, String)> =
        dsps(d).into_iter().map(|(n, a)| (n.split("_dsp").next().unwrap().to_string(), ["AREG", "BREG", "MREG", "PREG"].map(|k| a[k].as_str()).join(" "))).collect();
    ds.sort();
    ds
}

#[test]
fn a_register_two_hops_behind_an_m_register_stays_free_for_the_next_product_in_any_order() {
    // p1 is the first slice's P; p2 is nobody's, so it is q's A register.
    let st = ["always_ff @(posedge clk) m <= a * b;", "always_ff @(posedge clk) p1 <= m;", "always_ff @(posedge clk) p2 <= p1;", "always_ff @(posedge clk) q <= p2 * c;"];
    let all = orders(&st);
    assert_eq!(all.len(), 24);
    for body in all {
        let d = v2(&format!(
            "module t(input logic clk, input logic signed [11:0] a, b, input logic signed [17:0] c, output logic signed [41:0] q);
             logic signed [23:0] m, p1, p2; {body} endmodule"
        ));
        assert_eq!(regs_of(&d), [("p1".to_string(), "0 0 1 1".to_string()), ("q".to_string(), "1 0 0 1".to_string())], "{body}");
        assert_eq!(flops(&d), 0, "{body}");
    }
}

#[test]
fn a_product_that_is_split_does_not_keep_its_registers_from_other_products() {
    // 20x20 does not fit one slice: m and p1 stay in the fabric, and p1's
    // low bits are free to be q's A register whichever comes first.
    let st = ["always_ff @(posedge clk) m <= a * b;", "always_ff @(posedge clk) p1 <= m;", "always_ff @(posedge clk) q <= $signed(p1[23:0]) * c;"];
    let mut seen = Vec::new();
    for body in orders(&st) {
        let d = v2(&format!(
            "module t(input logic clk, input logic signed [19:0] a, b, input logic signed [17:0] c, output logic signed [41:0] q);
             logic signed [39:0] m, p1; {body} endmodule"
        ));
        let q: Vec<String> = regs_of(&d).into_iter().filter(|(n, _)| n == "q").map(|(_, r)| r).collect();
        assert_eq!(q.len(), 1, "{body}");
        assert!(q[0].starts_with("1 "), "q AREG in {body}: {}", q[0]);
        seen.push(flops(&d));
    }
    assert!(seen.iter().all(|&f| f == seen[0]), "flops by order: {seen:?}");
}

/// The DSPs with their registers, flops and LUTs of `head` + `body`, for every order of `st`.
fn by_order(head: &str, st: &[&str]) -> Vec<(String, (Vec<(String, String)>, usize, usize))> {
    orders(st)
        .into_iter()
        .map(|body| {
            let d = v2(&format!("{head} {body} endmodule"));
            let shape = (regs_of(&d), flops(&d), count(&d, false));
            (body, shape)
        })
        .collect()
}

#[test]
fn a_product_whose_operand_an_earlier_slice_narrows_maps_the_same_in_any_order() {
    // m1 is 30 bits but only its low 24 are significant once a * b is a
    // slice: r <= m1 * c then fits one (24 x 18), with r as its M and pr
    // its P, whichever statement comes first.
    let head = "module t(input logic clk, input logic signed [11:0] a, b, input logic signed [17:0] c, d, output logic signed [41:0] q);
        logic signed [29:0] m1; logic signed [47:0] r, pr;";
    let st = [
        "always_ff @(posedge clk) m1 <= a * b;",
        "always_ff @(posedge clk) r <= m1 * c;",
        "always_ff @(posedge clk) pr <= r;",
        "always_ff @(posedge clk) q <= $signed(pr[23:0]) * d;",
    ];
    let all = by_order(head, &st);
    assert_eq!(all.len(), 24);
    let want = [("m1", "0 0 0 1"), ("pr", "0 0 1 1"), ("q", "0 0 0 1")].map(|(n, r)| (n.to_string(), r.to_string()));
    assert_eq!((all[0].1).0, want);
    for (body, shape) in &all {
        assert_eq!(shape, &all[0].1, "{body} against {}", all[0].0);
    }
    // The 2-hop chain of `a_register_two_hops_behind_...`, counted the same way.
    let head = "module t(input logic clk, input logic signed [11:0] a, b, input logic signed [17:0] c, output logic signed [41:0] q);
        logic signed [23:0] m, p1, p2;";
    let st = ["always_ff @(posedge clk) m <= a * b;", "always_ff @(posedge clk) p1 <= m;", "always_ff @(posedge clk) p2 <= p1;", "always_ff @(posedge clk) q <= p2 * c;"];
    let all = by_order(head, &st);
    for (body, shape) in &all {
        assert_eq!(shape, &all[0].1, "{body} against {}", all[0].0);
    }
}

#[test]
fn a_product_reading_a_product_cycle_maps_the_same_in_any_order() {
    // x1 and x2 read each other, so neither is upstream of the other; z
    // reads x1, narrowed to 24 bits once x1 is a slice, and must not be
    // judged (and split) before it, whichever statement comes first.
    let head = "module t(input logic clk, input logic sel, input logic signed [11:0] a, b, input logic signed [17:0] c,
        output logic signed [47:0] z, output logic signed [29:0] o1, o2);
        logic signed [29:0] x1, x2; assign o1 = x1; assign o2 = x2;";
    let st = [
        "always_ff @(posedge clk) x1 <= $signed(sel ? b : x2[11:0]) * a;",
        "always_ff @(posedge clk) x2 <= $signed(x1[11:0]) * b;",
        "always_ff @(posedge clk) z <= x1 * c;",
    ];
    let all = by_order(head, &st);
    assert_eq!(all.len(), 6);
    for (body, shape) in &all {
        assert_eq!((shape.0.len(), shape.1), (3, 0), "{body}");
        assert_eq!(shape, &all[0].1, "{body} against {}", all[0].0);
    }
    // z on a cycle of its own with w: the x1/x2 cycle that feeds it goes first.
    let head = "module t(input logic clk, input logic sel, input logic signed [11:0] a, b, input logic signed [17:0] c,
        output logic signed [47:0] z, output logic signed [29:0] o1, o2, o3);
        logic signed [29:0] x1, x2, w; assign o1 = x1; assign o2 = x2; assign o3 = w;";
    let st = [
        "always_ff @(posedge clk) x1 <= $signed(sel ? b : x2[11:0]) * a;",
        "always_ff @(posedge clk) x2 <= $signed(x1[11:0]) * b;",
        "always_ff @(posedge clk) z <= x1 * $signed(w[17:0]);",
        "always_ff @(posedge clk) w <= $signed(z[11:0]) * a;",
    ];
    let all = by_order(head, &st);
    assert_eq!(all.len(), 24);
    for (body, shape) in &all {
        assert_eq!((shape.0.len(), shape.1), (4, 0), "{body}");
        assert_eq!(shape, &all[0].1, "{body} against {}", all[0].0);
    }
}
