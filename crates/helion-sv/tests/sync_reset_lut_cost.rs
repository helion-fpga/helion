//! Where a flop's reset lands: the SR pin, or a LUT in front of D
//! (HELION_GAPS G59).
//!
//! Measured on a 26-bit counter with the same 26 flops in every case:
//!
//! | reset                      | LUT6 before G59 | LUT6 now |
//! |----------------------------|-----------------|----------|
//! | none                       |   26 |   26 |
//! | `always_ff @(posedge clk or posedge rst)`  |   26 |   26 |
//! | `always_ff @(posedge clk) if (rst)`        |   52 |   26 |
//!
//! The asynchronous form reaches the flop's `ARST` pin. The synchronous form
//! used to be a mux in front of `D`, one LUT per flop; `v2` now peels an
//! outermost `if (rst) q <= K` off the next value onto the flop's `SR` pin
//! with `SRVAL` = K (lower.rs `sync_reset`), the way the back end builds
//! FDRE/FDSE. The rung 4 MMCM design went from 317 to 155 LUTs and the DSP fill
//! from 8,140 to 4,479.

use helion_ir::CellKind;

/// Flip to `true` when a synchronous reset is built on the flop's SR pin.
const SYNC_RESET_USES_SR_PIN: bool = true;

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

/// A reset to 1 lands on SR too (SRVAL = 1, FDSE), and a reset with an
/// enable keeps both pins: the reset wins over the enable, as on FDRE.
#[test]
fn set_and_enable_with_reset_use_the_flop_pins() {
    let (lut, ff) = shapes("always_ff @(posedge clk) if (rst) w <= 26'h3ffffff; else w <= w + 26'd1;");
    assert_eq!((lut, ff), (26, 26), "a reset to all ones should cost no LUTs either");
    let (lut, ff) = shapes("always_ff @(posedge clk) if (rst) w <= 0; else if (led[0]) w <= w + 26'd1;");
    assert_eq!(ff, 26);
    assert!(lut <= 27, "reset plus enable cost {lut} LUTs: the reset or the enable went in front of D");
}

/// A reset under an enable must not reach SR: SR would act while the enable
/// is off. It stays in front of D.
#[test]
fn a_reset_inside_an_enable_stays_in_front_of_d() {
    helion_sv::v2::set_hard(true);
    helion_sv::v2::dag::set_carry_arith(true);
    let src = "module c(input logic clk, input logic rst, input logic en, output logic [7:0] q);\n  always_ff @(posedge clk) if (en) begin if (rst) q <= 0; else q <= q + 8'd1; end\nendmodule\n";
    let s = helion_sv::synth_sv_v2(src, "c.sv", Some("c")).expect("synth");
    let pins = s.design.pin_index();
    for c in s.design.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)) {
        assert!(pins.net_on(helion_ir::resolve(c.name), "SR").is_none(), "{} took the reset on SR", helion_ir::resolve(c.name));
    }
}
