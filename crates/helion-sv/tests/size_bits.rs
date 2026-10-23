//! `$size(x)` is a constant (the bit width of `x`). It was unhandled, fell
//! through to a `Call` node, and bit lowering rejected it with an error that
//! the caller swallowed — so `assign y = $size(a);` drove `y` with constant 0
//! and reported nothing at all.

mod common;
use common::Netlist;
use helion_sv::synth_sv;

fn synth(src: &str) -> helion_ir::Design {
    synth_sv(src, "t.sv").expect("synth")
}

#[test]
fn size_of_a_vector_is_its_width() {
    let d = synth("module t(input logic [15:0] a, output logic [7:0] y); assign y = $size(a); endmodule");
    assert_eq!(
        Netlist::drive(&d, &[]).out("y", 8),
        16,
        "$size of a 16-bit vector is 16"
    );
}

#[test]
fn size_of_several_widths() {
    for (decl, want) in [("[7:0]", 8u64), ("[0:0]", 1), ("[31:0]", 32), ("[12:5]", 8)] {
        let src = format!(
            "module t(input logic {decl} a, output logic [7:0] y); assign y = $size(a); endmodule"
        );
        let d = synth(&src);
        assert_eq!(
            Netlist::drive(&d, &[]).out("y", 8),
            want,
            "$size of logic {decl} is {want}"
        );
    }
}

#[test]
fn size_of_a_scalar_is_one() {
    let d = synth("module t(input logic a, output logic [7:0] y); assign y = $size(a); endmodule");
    assert_eq!(Netlist::drive(&d, &[]).out("y", 8), 1, "$size of a scalar is 1");
}

#[test]
fn size_works_inside_an_expression() {
    // A constant on the RHS, so it must fold rather than become a cone.
    let d = synth("module t(input logic [15:0] a, output logic [7:0] y); assign y = $size(a) - 8; endmodule");
    assert_eq!(
        Netlist::drive(&d, &[]).out("y", 8),
        8,
        "$size(a) - 8 with a 16-bit a is 8"
    );
}

#[test]
fn size_emits_logic_not_a_dangling_output() {
    // Before the fix `y` had no driver at all and read 0 through the IOB.
    let d = synth("module t(input logic [15:0] a, output logic [7:0] y); assign y = $size(a); endmodule");
    assert!(d.cells.len() > 1, "the constant must be materialised as logic");
    assert_ne!(
        d.attrs.get("ASSIGN_NOT_LOWERED"),
        Some("1"),
        "$size is a constant, not an unsupported assign"
    );
    assert_eq!(Netlist::drive(&d, &[]).out("y", 8), 16);
}
