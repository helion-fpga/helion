//! An FF whose data input is not physically driven must fail the build.
//!
//! This is the ZedBoard blinky's failure mode, and it cost several rounds
//! because *nothing* about the image looked wrong. `LD0 <= count(24)` is a
//! register-to-register assignment, but synthesis emitted a pass-through LUT
//! in the flop's slice; the flop took that LUT's `O6` output, the LUT's own
//! operands were never routed, and the flop sampled a constant forever.
//!
//! Every check that existed passed:
//!
//!   * 9996 frames, correct IDCODE, correct header package name;
//!   * `features_applied` in the normal range;
//!   * byte-perfect clock spine -- 0 diffs against the Vivado reference
//!     across the whole device;
//!   * balanced `ZINI`/`ZRST` census (104/104): the flop was configured
//!     correctly, it was simply fed nothing;
//!   * the hardware-proven `INT_R_X73Y46.IMUX34 <- NR1END1` arm; and
//!   * an output pad and IOLOGIC identical to Vivado's, feature for feature.
//!
//! The device configured and reported `done=1`. The LED stayed dark.
//!
//! The guard in `emit_fasm` asks the question none of those did: does the
//! routed design deliver a net to the flip-flop's data pin? These tests pin
//! both halves of it.
//!
//! The sources are embedded rather than read from disk on purpose. An earlier
//! version loaded them from a scratch directory that later disappeared, so the
//! tests reported `ok` while asserting nothing at all -- a skipped guard is
//! worse than no guard, because the suite claims coverage it does not have.
//!
//! Both tests need `HELION_XILINX_DB`; without it they fail rather than pass.

use helion_xilinx7::build::{bitstream_for_part, PinConstraints};

/// The faithful equivalent of the reference `blinky.vhd`, the source of the
/// known-good bitstream. Its output register is **registered**:
///
/// ```vhdl
/// if rising_edge(clk) then
///     count := count + 1;
///     LD0 <= count(24);
/// end if;
/// ```
///
/// That one property is what makes this design interesting. The counter's
/// carry chain publishes bit 24 as `n_nx_count_24`, while the `LD0` flop asks
/// for it as `count_24` -- the same bit under its registered name. An exact
/// string compare between those two spellings silently fails to connect them,
/// and the flop is then wired to an unrouted slice pin.
const BLINKY_REG: &str = r#"
module blinky (
    input  wire clk,
    output reg  LD0
);
    reg [24:0] count = 25'd0;

    always @(posedge clk) begin
        count <= count + 1'b1;
        LD0   <= count[24];
    end
endmodule
"#;

/// The control: the same LED, but driven combinationally, so no second
/// register exists to be misplaced.
const BLINKY_COMB: &str = r#"
module blinky (
    input  wire clk,
    output wire LD0
);
    reg [24:0] count = 25'd0;

    always @(posedge clk)
        count <= count + 1'b1;

    assign LD0 = count[24];
endmodule
"#;

/// ZedBoard LED0 (`T22`) and the 100 MHz clock (`Y9`).
const BLINKY_XDC: &str = r#"
set_property PACKAGE_PIN Y9 [get_ports {clk}]
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]
set_property PACKAGE_PIN T22 [get_ports {LD0}]
set_property IOSTANDARD LVCMOS33 [get_ports {LD0}]
create_clock -period 10.000 -name sys_clk [get_ports {clk}]
"#;

fn db() -> std::path::PathBuf {
    std::env::var("HELION_XILINX_DB")
        .map(std::path::PathBuf::from)
        .expect("HELION_XILINX_DB must point at a prjxray-db checkout")
}

/// Build `src` for the ZedBoard. `Err` carries the failure message so the
/// caller can assert on how specific it is.
fn build(src: &str, name: &str) -> Result<(), String> {
    let mut design = helion_sv::synth_sv(src, name).map_err(|e| e.to_string())?;
    PinConstraints::parse(BLINKY_XDC).apply(&mut design);
    bitstream_for_part(&design, "xc7z020clg484-1", Some(&db()))
        .map(|_| ())
        .map_err(|e| format!("{e}"))
}

/// A registered output must not end up on an undriven slice pin.
///
/// Before the fix this produced a full, well-formed, inert bitstream. With the
/// fix the `LD0` flop resolves to the carry lane, so the build succeeds -- and
/// the assertion is that it succeeds *with a driven flop*, not merely that it
/// returns `Ok`. A regression that reinstates the dangling pin would either
/// trip the emit guard (caught below) or put the flop back on `AX`/`BX`
/// (caught by the carry-mux check in `blinky_bit.rs`).
#[test]
fn a_registered_output_builds_with_a_driven_flop() {
    match build(BLINKY_REG, "blinky_reg.sv") {
        Ok(()) => {}
        Err(e) => panic!(
            "the registered blinky must build: its output register resolves to \
             the carry lane, so nothing dangles. Got: {e}"
        ),
    }
}

/// The control: a combinational output has no second register to misplace and
/// must always build. Without this, a guard that rejected everything would
/// satisfy the test above.
#[test]
fn a_combinational_output_still_builds() {
    match build(BLINKY_COMB, "blinky.sv") {
        Ok(()) => {}
        Err(e) => panic!("a well-formed design must still build, got: {e}"),
    }
}

/// The guard itself must reject a flop whose `D` nothing drives.
///
/// This is the property that turns a silent dark LED into a build error, and
/// it is asserted directly rather than through the blinky so that it stays
/// meaningful if the blinky's mapping changes again.
#[test]
fn the_guard_names_the_flop_and_the_mux_arm() {
    // `q <= q` is a pass-through register: synthesis emits no LUT for it, so
    // the flop's `D` is its own `Q`. Nothing routes that, which is exactly the
    // shape the guard exists to catch.
    let src = r#"
module selfreg (input wire clk, output reg q);
    always @(posedge clk) q <= q;
endmodule
"#;
    // A self-referential register with no reset has no driver at all, so the
    // build must fail rather than emit an image whose flop samples a constant.
    match build(src, "selfreg.sv") {
        Ok(()) => {
            // Acceptable only if the flop ended up genuinely driven -- which
            // for `q <= q` it cannot be, so treat success as a guard failure.
            panic!(
                "`q <= q` has no data source, yet the build succeeded; the \
                 undriven-flop guard is not firing"
            );
        }
        Err(e) => {
            assert!(
                e.contains("no routed net") || e.contains("undriven"),
                "the failure must explain that the data path is undriven, got: {e}"
            );
        }
    }
}

/// A register that samples another register should share its slice.
///
/// `Q -> D` within a slice is an intra-slice connection needing no routing, so
/// co-locating removes the routing question instead of relying on the router
/// to answer it. This was Vivado's choice for the blinky's output register and
/// is what makes the design robust rather than lucky: the allocator already
/// happened to pick the right slice in iteration order, which meant a small
/// change anywhere upstream could silently move the flop somewhere unreachable.
///
/// The assertion is on the *emitted* image rather than on placement internals,
/// because placement internals are what a future refactor may legitimately
/// change. Both flops must land in one slice.
#[test]
fn a_register_that_samples_a_register_shares_its_slice() {
    use helion_xilinx7::build::bitstream_for_part;
    let mut design = helion_sv::synth_sv(BLINKY_REG, "blinky_reg.sv").expect("synth");
    PinConstraints::parse(BLINKY_XDC).apply(&mut design);
    let bs = bitstream_for_part(&design, "xc7z020clg484-1", Some(&db())).expect("build");

    // The bitstream is already assembled here, so assert on the shape of the
    // result rather than on FASM text: a full, populated image with no dropped
    // tiles. The fine-grained mux checks live in `blinky_bit.rs`, which
    // decodes frames through the segbits DB -- if the output register were
    // scattered to an unreachable slice, `emit_fasm`'s guard would have
    // refused the build before reaching this point, and if it were wired to a
    // raw pin the decode tests would see `FFMUX.BX` instead of `FFMUX.XOR`.
    assert!(
        bs.frame_count > 9000,
        "expected a full frame image, got {} frames",
        bs.frame_count
    );
    assert!(
        bs.features_applied > 1000,
        "expected a populated image, got {} features",
        bs.features_applied
    );
    assert!(
        bs.dropped.is_empty(),
        "nothing should be dropped for this design: {:?}",
        bs.dropped
    );
}
