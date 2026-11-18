//! Where a flop's reset lands: the SR pin, or a LUT in front of D
//! (HELION_GAPS G59).
//!
//! A counter with a reset costs one LUT per flop more than the same counter
//! without one, and that is the LUT bloat behind G59 (the rung 4 MMCM design
//! packs 317 LUTs against Vivado's 37). Measured on a 26-bit counter with the
//! same 26 flops in every case:
//!
//! | reset                      | LUT6 | flops |
//! |----------------------------|------|-------|
//! | none                       |   26 |    26 |
//! | `always_ff @(posedge clk or posedge rst)`  |   26 |    26 |
//! | `always_ff @(posedge clk) if (rst)`        |   52 |    26 |
//!
//! The asynchronous form reaches the flop's `ARST` pin and costs nothing extra.
//! The synchronous form has no front-end path at all: `v2` puts the reset in
//! front of `D` as a mux, one LUT per flop, even though the back end can build
//! it (pack.rs understands `SR`/`SRVAL`/`SR_ASYNC` for FDRE/FDSE). So every
//! synchronous reset in every design pays this.
//!
//! This test pins all three numbers. They are not a target: when the sync path
//! lands, `SYNC_RESET_USES_SR_PIN` has to be flipped, which forces the ledger
//! row and the expected counts to move together instead of the fix arriving
//! silently.

use helion_ir::CellKind;

/// Flip to `true` when a synchronous reset is built on the flop's SR pin.
const SYNC_RESET_USES_SR_PIN: bool = false;

fn shapes(body: &str) -> (usize, usize) {
    helion_sv::v2::set_hard(true);
    helion_sv::v2::dag::set_carry_arith(true);
    let src = format!(
        "module c(input logic clk, input logic rst, output logic [7:0] led);\n  logic [25:0] w;\n  {body}\n  assign led = w[7:0];\nendmodule\n"
    );
    let s = helion_sv::synth_sv_v2(&src, "c.sv", Some("c")).expect("synth");
    let lut = s.design.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count();
    let ff = s.design.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
    (lut, ff)
}

/// A counter with no reset: one LUT per flop is the floor (the carry chain
/// supplies the sum). If this ever rises, the mapper regressed, not the reset.
#[test]
fn a_plain_counter_costs_one_lut_per_flop() {
    let (lut, ff) = shapes("always_ff @(posedge clk) w <= w + 26'd1;");
    assert_eq!(ff, 26, "fixture should keep 26 flops");
    assert_eq!(lut, 26, "a 26-bit counter with no reset used to need exactly 26 LUT6");
}

/// An asynchronous reset is free: it lands on the flop's ARST pin.
#[test]
fn an_asynchronous_reset_costs_no_extra_luts() {
    let (lut, ff) = shapes("always_ff @(posedge clk or posedge rst) if (rst) w <= 0; else w <= w + 26'd1;");
    assert_eq!(ff, 26);
    assert_eq!(lut, 26, "an async reset used to cost no LUTs beyond the counter itself");
}

/// The gap: the same reset written synchronously doubles the LUT count,
/// because it is a mux in front of D rather than the SR pin.
#[test]
fn a_synchronous_reset_muxes_into_d_until_the_sr_path_exists() {
    let (lut, ff) = shapes("always_ff @(posedge clk) if (rst) w <= 0; else w <= w + 26'd1;");
    assert_eq!(ff, 26, "the sync and async forms must hold the same number of flops");
    if SYNC_RESET_USES_SR_PIN {
        assert_eq!(
            lut, 26,
            "SYNC_RESET_USES_SR_PIN is true but a sync reset still costs {lut} LUTs (it should match the async form's 26)"
        );
        return;
    }
    assert_eq!(
        lut, 52,
        "a synchronous reset now costs {lut} LUTs, not the 52 of the D-mux form: set \
         SYNC_RESET_USES_SR_PIN = true and close G59 in HELION_GAPS.md"
    );
}

/// The contrast is the gap, stated as one executable fact: same function,
/// same flops, twice the LUTs for the synchronous spelling.
#[test]
fn sync_and_async_reset_differ_by_one_lut_per_flop() {
    let (async_lut, _) = shapes("always_ff @(posedge clk or posedge rst) if (rst) w <= 0; else w <= w + 26'd1;");
    let (sync_lut, _) = shapes("always_ff @(posedge clk) if (rst) w <= 0; else w <= w + 26'd1;");
    if SYNC_RESET_USES_SR_PIN {
        assert_eq!(sync_lut, async_lut, "with the SR path both spellings should cost the same");
    } else {
        assert_eq!(
            sync_lut - async_lut,
            26,
            "sync ({sync_lut}) minus async ({async_lut}) should be one LUT per flop (26); if \
             these now agree, G59 has been fixed"
        );
    }
}
