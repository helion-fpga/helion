//! An `always_comb` branch that keeps the previous value needs a latch, and
//! `helion-ir` has no latch cell. The mapper used to rewrite the feedback to
//! constant 0, silently turning `q = c ? d : q` into `q = c & d` — the held
//! value was lost for every input with `c` low. It must refuse instead.

mod common;
use helion_ir::CellKind;
use helion_sv::synth_sv;

fn synth(src: &str) -> helion_ir::Design {
    synth_sv(src, "t.sv").expect("synth")
}

fn lut_count(d: &helion_ir::Design) -> usize {
    d.cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count()
}

#[test]
fn incomplete_comb_if_is_refused_not_folded_to_zero() {
    // Before: emitted `u_clut0_s0` with init 0x8888.. (`q = c & d`).
    let d = synth(
        "module t(input logic c, input logic d, output logic q);
           always_comb if (c) q = d;
         endmodule",
    );
    assert_eq!(
        d.attrs.get("ASSIGN_NOT_LOWERED"),
        Some("1"),
        "a latch must be refused loudly"
    );
    assert_eq!(lut_count(&d), 0, "no LUT may be invented for a latch");
}

#[test]
fn incomplete_comb_case_is_refused_not_folded_to_zero() {
    let d = synth(
        "module t(input logic [1:0] s, input logic d, output logic q);
           always_comb case (s) 2'd0: q = d; endcase
         endmodule",
    );
    assert_eq!(
        d.attrs.get("ASSIGN_NOT_LOWERED"),
        Some("1"),
        "a case without default that holds the old value is a latch"
    );
    assert_eq!(lut_count(&d), 0, "no LUT may be invented for a latch");
}

#[test]
fn priority_chain_with_an_unconditional_base_still_lowers() {
    // `q = e; if (c) q = d;` is not a latch: the base assign supplies the tip.
    let d = synth(
        "module t(input logic c, input logic d, input logic e, output logic q);
           always_comb begin q = e; if (c) q = d; end
         endmodule",
    );
    assert_ne!(
        d.attrs.get("ASSIGN_NOT_LOWERED"),
        Some("1"),
        "a full priority chain is not a latch"
    );
    assert!(lut_count(&d) >= 1, "the priority select must map a LUT");
}
