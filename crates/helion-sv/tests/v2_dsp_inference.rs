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
//! What v2 does not do yet (G3a stage 2): products wider than one
//! multiplier, such as the unsigned 18x18 below (19x19 signed), stay in
//! LUTs. The legacy front end puts that one in a single DSP; whether its
//! mapping keeps `b[17]` unsigned is not checked here.

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

/// Unsigned 18x18 needs a 19x19 signed multiplier: one DSP48E1 cannot hold
/// it exactly, so v2 keeps it in LUTs (stage 2 of G3a splits it). Pinned so
/// the day it changes, this file says so.
#[test]
fn v2_keeps_an_unsigned_18x18_in_luts() {
    let d = v2(MUL);
    assert_eq!(count(&d, true), 0);
    assert!(count(&d, false) > 500, "expected the Wallace-tree LUT multiplier");
}

#[test]
fn the_front_ends_agree_on_a_product_that_fits_and_differ_on_one_that_does_not() {
    let legacy = |s: &str| count(&helion_sv::synth_sv(s, "m.sv").unwrap(), true);
    assert_eq!((legacy(MUL_SIGNED), count(&v2(MUL_SIGNED), true)), (1, 1));
    assert_eq!((legacy(MUL), count(&v2(MUL), true)), (1, 0));
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
    // 25 x 17 unsigned is 26 x 18 signed: no port takes 26 bits.
    assert!(dsps(&v2("module t(input logic [24:0] a, input logic [16:0] b, output logic [41:0] p); assign p = a * b; endmodule")).is_empty());
}

#[test]
fn a_signed_times_unsigned_product_is_unsigned() {
    // SystemVerilog: one unsigned operand makes both unsigned, so s is zero-extended.
    let (d, c, _) = one("module t(input logic signed [19:0] s, input logic [16:0] u, output logic [47:0] p); assign p = s * u; endmodule");
    assert_eq!(net(&d, &c, "A19").as_deref(), Some("s_19"));
    assert_eq!(net(&d, &c, "A20").as_deref(), Some("const0"));
    // A 25-bit signed operand becomes 25 bits unsigned: 26 signed bits, too many.
    assert!(dsps(&v2("module t(input logic signed [24:0] s, input logic [16:0] u, output logic [47:0] p); assign p = s * u; endmodule")).is_empty());
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
