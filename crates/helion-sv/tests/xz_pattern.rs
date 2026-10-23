//! X/Z have no representation in the fabric. Lowering a literal with
//! don't-care bits read every such bit as 0, so `assign y = 4'b1x0z;` silently
//! became the constant 8 with no diagnostic at all.

mod common;
use helion_ir::CellKind;
use helion_sv::synth_sv;

fn synth(src: &str) -> helion_ir::Design {
    synth_sv(src, "t.sv").expect("synth")
}

fn luts(d: &helion_ir::Design) -> usize {
    d.cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count()
}

#[test]
fn assign_of_a_partial_xz_literal_is_refused() {
    // Before: constant 0x08 (x and z read as 0), no diagnostic.
    for lit in ["4'b10xz", "4'b1x0z", "4'bxxzz", "8'bx", "8'b01xz_01xz"] {
        let src = format!("module t(output logic [7:0] y); assign y = {lit}; endmodule");
        let d = synth(&src);
        assert_eq!(
            d.attrs.get("ASSIGN_NOT_LOWERED"),
            Some("1"),
            "{lit} must be refused, not read as zeros"
        );
        assert_eq!(luts(&d), 0, "{lit} must not invent logic");
    }
}

#[test]
fn plain_literals_in_arithmetic_are_not_flagged() {
    // The don't-care check must not fire on an unsized decimal.
    let d = synth("module t(input wire [31:0] pc, output wire [31:0] y); assign y = pc + 4; endmodule");
    assert_ne!(
        d.attrs.get("ASSIGN_NOT_LOWERED"),
        Some("1"),
        "`pc + 4` is an ordinary constant addend"
    );
    let driven = (0..32)
        .filter(|b| {
            let n = format!("y_{b}");
            d.nets
                .iter()
                .any(|net| net.name == n && net.endpoints.iter().any(|e| e.pin == "O"))
        })
        .count();
    assert_eq!(driven, 32, "every output bit must lower");
}

#[test]
fn sized_all_care_literals_still_lower() {
    for lit in ["4'b1010", "4'hf", "8'd200", "4'o17"] {
        let src = format!("module t(output logic [7:0] y); assign y = {lit}; endmodule");
        let d = synth(&src);
        assert_ne!(
            d.attrs.get("ASSIGN_NOT_LOWERED"),
            Some("1"),
            "{lit} has no don't-care bits"
        );
    }
}
