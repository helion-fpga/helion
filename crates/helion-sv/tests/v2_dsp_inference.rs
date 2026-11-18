//! Where `*` lands, per front end (HELION_GAPS.md G3a).
//!
//! G3's row said v2's missing DSP inference was "now present via
//! `is_mac_rhs`/`emit_mac27`". It is not: measured 2026-11-18, `v2/` contains no
//! DSP code at all (`grep -rn Dsp48e1 crates/helion-sv/src/v2/` is empty), and
//! an 18x18 registered multiply through `synth_sv_v2` gives 0 Dsp48e1 and 2012
//! Lut6, while the legacy `synth_sv` gives 1 Dsp48e1 for the same source. So the
//! two paths disagree, and everything built through zedbit's default front end
//! gets the 2012-LUT multiplier: that is the Ibex multiplier in LUTs, and why
//! `"dsp":0.0` shows up in every zedbit summary.
//!
//! This test pins both halves so the claim cannot drift again in either
//! direction:
//!
//! - the legacy path must keep inferring a DSP (a regression here is a real
//!   break);
//! - v2 must be *known* not to, and the day it does, `V2_INFERS_DSP` below has
//!   to be flipped - which forces whoever adds inference to come here and say
//!   so, instead of the fix being invisible as it was.

use helion_ir::CellKind;

/// Flip to `true` when v2 gains DSP inference, and update G3a in the ledger.
const V2_INFERS_DSP: bool = false;

const MUL: &str = "module m(input logic clk, input logic [17:0] a, input logic [17:0] b, output logic [35:0] p);\nalways_ff @(posedge clk) p <= a * b;\nendmodule\n";

fn count(d: &helion_ir::Design, want_dsp: bool) -> usize {
    d.cells
        .iter()
        .filter(|c| match (&c.kind, want_dsp) {
            (CellKind::Dsp48e1 { .. }, true) => true,
            (CellKind::Lut6 { .. }, false) => true,
            _ => false,
        })
        .count()
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

/// v2 does not infer a DSP: it lowers `*` through the Wallace tree in LUTs
/// (`v2/lower.rs`, `Op::Mul`). Pinned in both directions so that adding
/// inference is a deliberate, visible change.
#[test]
fn v2_multiply_lands_in_luts_not_a_dsp() {
    let s = helion_sv::synth_sv_v2(MUL, "m.sv", Some("m")).expect("v2 synth");
    let dsp = count(&s.design, true);
    let luts = count(&s.design, false);
    if V2_INFERS_DSP {
        assert!(
            dsp >= 1,
            "V2_INFERS_DSP is true but v2 produced no Dsp48e1 ({luts} Lut6)"
        );
        return;
    }
    assert_eq!(
        dsp, 0,
        "v2 now infers a Dsp48e1 for a plain multiply: set V2_INFERS_DSP = true \
         and close G3a in HELION_GAPS.md"
    );
    // The fallback is expensive and that is the point of the gap: it should be
    // a large LUT count, not a handful of gates.
    assert!(
        luts > 500,
        "expected a Wallace-tree LUT multiplier (hundreds of LUT6), got {luts}"
    );
}

/// The two paths disagreeing is the gap itself. State it as an executable fact
/// so it cannot be described as fixed while it is not.
#[test]
fn the_two_front_ends_disagree_about_the_multiplier() {
    let legacy = count(&helion_sv::synth_sv(MUL, "m.sv").unwrap(), true);
    let v2 = count(&helion_sv::synth_sv_v2(MUL, "m.sv", Some("m")).unwrap().design, true);
    assert_eq!(
        (legacy, v2),
        (1, 0),
        "if these ever agree, G3a has changed: legacy={legacy} v2={v2}. \
         Update V2_INFERS_DSP and the ledger row together"
    );
}
