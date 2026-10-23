//! Sized signed based literals (`8'sd-5`, `8'shfb`) used to compile to
//! nothing at all: the tokenizer did not recognise the `s` signed marker, so
//! the base looked like `s` and the literal was rejected, and a sign written
//! after the base split the literal into two terms the nested-arith guard
//! then refused. Every case below silently produced 0 before.

mod common;
use common::Netlist;
use helion_sv::synth_sv;

fn synth(src: &str) -> helion_ir::Design {
    synth_sv(src, "t.sv").expect("synth")
}

#[test]
fn signed_literal_forms_keep_their_value() {
    for (lit, want) in [
        ("8'sd-5", 0xfbu64),
        ("8'sd5", 0x05),
        ("8'sd-1", 0xff),
        ("8'shfb", 0xfb),
        ("-8'sd5", 0xfb),
        ("-8'd5", 0xfb),
        ("4'sd-5", 0x0b),
    ] {
        let src = format!("module t(output logic [7:0] y); assign y = {lit}; endmodule");
        let d = synth(&src);
        assert_eq!(
            Netlist::drive(&d, &[]).out("y", 8),
            want,
            "{lit} must be 0x{want:02x}"
        );
    }
}

#[test]
fn signed_negative_literal_in_arithmetic_emits_logic() {
    // `a + 8'sd-5` used to hit the nested-arith guard and emit no cells.
    let d = synth("module t(input logic [7:0] a, output logic [7:0] y); assign y = a + 8'sd-5; endmodule");
    assert!(!d.cells.is_empty(), "the assign must produce logic");
    for a in [0u64, 3, 5, 10, 255] {
        let y = Netlist::drive(&d, &[("a", a, 8)]).out("y", 8);
        assert_eq!(y, a.wrapping_sub(5) & 0xff, "a={a} + (-5)");
    }
}

#[test]
fn signed_positive_literal_in_arithmetic_is_unchanged() {
    let d = synth("module t(input logic [7:0] a, output logic [7:0] y); assign y = a + 8'sd5; endmodule");
    for a in [0u64, 3, 5, 10, 255] {
        let y = Netlist::drive(&d, &[("a", a, 8)]).out("y", 8);
        assert_eq!(y, a.wrapping_add(5) & 0xff, "a={a} + 5");
    }
}
