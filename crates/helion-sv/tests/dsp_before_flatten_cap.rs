//! A module that trips the flatten cap must still get its DSP inference.
//!
//! `synth_sv` guards the bit-blast of one leftover cone with the flatten cap
//! and returns early. That early return used to skip multiplier emission too,
//! because `emit_mac27` is called from the per-NBA loop *below* the cap, so a
//! capped module kept every `*` as a LUT multiplier — the 96x-LUT path behind
//! G3/D1. Emitting a Dsp48e1 needs no bit-blast (the RHS is classified and the
//! cell written directly), so the cap path now runs the same emission first.
//!
//! Guard for HELION_GAPS.md D1: if the cap stops emitting multipliers, this
//! fails because the capped module reports 0 Dsp48e1.

use helion_ir::CellKind;
use helion_sv::synth_sv;

/// A module with enough non-blocking assigns to trip the cap (>= 2048), plus a
/// registered 18x18 multiply that must become a DSP48E1.
fn capped_module_with_multiplier() -> String {
    let mut s = String::new();
    s.push_str(
        r#"
module capped_mul (
    input  wire clk,
    input  wire [17:0] a,
    input  wire [17:0] b,
    input  wire [3:0] sel,
    output reg  [35:0] p,
    output reg  [7:0] buf
);
    // The multiplier: registered product, the shape UG901 infers a DSP48E1 for.
    always @(posedge clk) begin
        p <= a * b;
    end

    // Bulk registers that push the module past the flatten cap. Small enough
    // that they cannot themselves be inferred as RAM.
"#,
    );
    for i in 0..2050u32 {
        s.push_str(&format!("    reg r{i};\n"));
    }
    s.push_str("    always @(posedge clk) begin\n");
    for i in 0..2050u32 {
        s.push_str(&format!("        r{i} <= a[{n}];\n", n = i % 18));
    }
    s.push_str("    end\n");
    s.push_str("    always @* begin\n        buf = {r3, r2, r1, r0, r7, r6, r5, r4};\n    end\n");
    s.push_str("    wire [7:0] unused_sel = {4'b0, sel};\n");
    s.push_str("endmodule\n");
    s
}

#[test]
fn capped_module_still_infers_dsp() {
    let src = capped_module_with_multiplier();
    let d = synth_sv(&src, "capped_mul.sv").expect("capped module synthesizes");

    // The cap is a diagnostic, and it must actually have tripped — otherwise
    // this test proves nothing about ordering.
    assert_eq!(
        d.attrs.get("FLATTEN_CAP"),
        Some("1"),
        "the module did not trip the flatten cap, so the ordering is untested"
    );

    let dsps = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Dsp48e1 { .. }))
        .count();
    assert!(
        dsps > 0,
        "a capped module produced no Dsp48e1: the flatten cap returned before \
         emit_mac27 (D1). The multiplier fell back to LUTs."
    );

    // And it must be the count the pass recorded, so the two cannot drift.
    let recorded: usize = d
        .attrs
        .get("DSP_BEFORE_FLATTEN_CAP")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    assert_eq!(
        dsps, recorded,
        "DSP_BEFORE_FLATTEN_CAP says {recorded} but {dsps} DSP48E1 cells were emitted"
    );
}
