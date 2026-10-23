//! `bus < const` wider than a LUT6 used to be refused, the refusal was
//! swallowed, and the output pin was left at constant 0 — correct for no
//! input. These pin the real values.

mod common;
use common::Netlist;
use helion_sv::synth_sv;

fn synth(src: &str) -> helion_ir::Design {
    synth_sv(src, "t.sv").expect("synth")
}

#[test]
fn wide_less_than_const_lowers_to_real_logic() {
    // 32-bit `a` against a bound whose lowest set bit is at 8: the compare
    // depends on bits [31:8], far more than a LUT6, so it ripples.
    let d = synth("module t(input logic [31:0] a, output logic y); assign y = a < 32'h0000_0100; endmodule");
    for a in [0u64, 1, 0xff, 0x100, 0x101, 0x1ff, 0xffff_ffff] {
        let y = Netlist::drive(&d, &[("a", a, 32)]).out("y", 1);
        assert_eq!(y, (a < 0x100) as u64, "a=0x{a:08x} < 0x100");
    }
}

#[test]
fn wide_greater_equal_const_lowers_to_real_logic() {
    let d = synth("module t(input logic [31:0] a, output logic y); assign y = a >= 32'h0000_0100; endmodule");
    for a in [0u64, 0xff, 0x100, 0x1ff, 0xffff_ffff] {
        let y = Netlist::drive(&d, &[("a", a, 32)]).out("y", 1);
        assert_eq!(y, (a >= 0x100) as u64, "a=0x{a:08x} >= 0x100");
    }
}

#[test]
fn wide_const_less_than_bus_lowers_to_real_logic() {
    // `const < bus` is `bus > const`.
    let d = synth("module t(input logic [31:0] a, output logic y); assign y = 32'h0000_0100 < a; endmodule");
    for a in [0u64, 0xff, 0x100, 0x101, 0x1ff, 0xffff_ffff] {
        let y = Netlist::drive(&d, &[("a", a, 32)]).out("y", 1);
        assert_eq!(y, (0x100u64 < a) as u64, "0x100 < a=0x{a:08x}");
    }
}

#[test]
fn narrow_compare_keeps_the_single_lut_shortcut() {
    // Bound 32'h1000_0000: only bits [31:28] matter, so this stays one LUT6.
    let d = synth("module t(input logic [31:0] a, output logic y); assign y = a < 32'h1000_0000; endmodule");
    for a in [0u64, 0x0fff_ffff, 0x1000_0000, 0xffff_ffff] {
        let y = Netlist::drive(&d, &[("a", a, 32)]).out("y", 1);
        assert_eq!(y, (a < 0x1000_0000) as u64, "a=0x{a:08x} < 0x1000_0000");
    }
}

#[test]
fn compare_boundary_values_are_exact() {
    // Every bit position of the bound must be honoured, including the
    // all-ones and zero edges.
    let d = synth("module t(input logic [7:0] a, output logic y); assign y = a < 8'h80; endmodule");
    for a in 0..=255u64 {
        let y = Netlist::drive(&d, &[("a", a, 8)]).out("y", 1);
        assert_eq!(y, (a < 0x80) as u64, "a={a} < 0x80");
    }
    let d = synth("module t(input logic [7:0] a, output logic y); assign y = a < 8'h01; endmodule");
    for a in 0..=255u64 {
        let y = Netlist::drive(&d, &[("a", a, 8)]).out("y", 1);
        assert_eq!(y, (a < 1) as u64, "a={a} < 1");
    }
}
