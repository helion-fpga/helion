//! Every constrained output bit must become a placed IOB.
//!
//! Round 39 found the worst failure mode this project has hit: an XDC written in
//! Helion's own flat bit spelling (`led_0`, what `bit_name` produces for a
//! vector bit's net) was silently dropped, the build exited **0**, and the
//! bitstream was full-size with the full frame count -- but contained no output
//! pads at all. 1742 FASM, 0 `OLOGIC.OQUSED` sites, no error.
//!
//! Nothing in the suite caught it. The existing tests assert frame counts, the
//! IDCODE, the header package name and `features_applied > 2000`; a design that
//! lost every output still satisfies all of those. `features_applied` for the
//! broken build was 1742, comfortably over the threshold, because the baseline
//! per-tile configuration dominates the count.
//!
//! The check that actually catches it is the one used by hand: count the placed
//! output sites and compare against the number of pin-constrained output bits.
//! These tests pin that invariant for both XDC spellings.
use helion_xilinx7::build::PinConstraints;

const SRC: &str = r#"
module v(input clk, output reg [3:0] led);
  reg [23:0] c;
  always @(posedge clk) begin c <= c + 1; led <= c[23:20]; end
endmodule
"#;

/// Four output bits constrained in the bracket spelling an XDC normally uses.
const XDC_BRACKET: &str = r#"
set_property PACKAGE_PIN Y9  [get_ports {clk}]
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]
set_property PACKAGE_PIN T22 [get_ports {led[0]}]
set_property PACKAGE_PIN U22 [get_ports {led[1]}]
set_property PACKAGE_PIN U21 [get_ports {led[2]}]
set_property PACKAGE_PIN V22 [get_ports {led[3]}]
create_clock -period 10.000 -name sys_clk [get_ports {clk}]
"#;

/// The same pins in the flat spelling Helion's own `bit_name` emits. This is the
/// form that used to be dropped on the floor.
const XDC_FLAT: &str = r#"
set_property PACKAGE_PIN Y9  [get_ports {clk}]
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]
set_property PACKAGE_PIN T22 [get_ports {led_0}]
set_property PACKAGE_PIN U22 [get_ports {led_1}]
set_property PACKAGE_PIN U21 [get_ports {led_2}]
set_property PACKAGE_PIN V22 [get_ports {led_3}]
create_clock -period 10.000 -name sys_clk [get_ports {clk}]
"#;

/// Run the 7-series flow and return how many output IOBs got a pad.
fn placed_outputs(xdc: &str) -> (usize, usize) {
    let mut design = helion_sv::synth_sv(SRC, "v.sv").expect("synth");
    let pins = PinConstraints::parse(xdc);
    pins.apply(&mut design);
    let packed = helion_xilinx7::pack::pack(&design).expect("pack");
    let outs = packed
        .iobs
        .iter()
        .filter(|i| i.dir == helion_xilinx7::pack::IobDir::Out)
        .count();
    (outs, pins.package_pins.len())
}

/// Both spellings must produce one IOB per constrained output bit.
///
/// The count is what matters, not merely "no error": the broken build was a
/// clean exit with zero IOBs.
#[test]
fn every_constrained_output_bit_becomes_an_iob() {
    for (name, xdc) in [("bracket", XDC_BRACKET), ("flat", XDC_FLAT)] {
        let (outs, pins) = placed_outputs(xdc);
        println!("{name}: {outs} output IOB(s), {pins} PACKAGE_PIN(s) total");
        assert_eq!(
            outs, 4,
            "{name} spelling: expected 4 output IOBs (one per constrained bit), got {outs}. \
             A drop to 0 means the XDC bit syntax is not being recognised -- the silent \
             no-outputs bitstream from round 39."
        );
    }
}

/// The two spellings must be equivalent, so a user cannot get a different
/// bitstream by writing `led_0` instead of `led[0]`.
#[test]
fn flat_and_bracket_spellings_agree() {
    let (a, _) = placed_outputs(XDC_BRACKET);
    let (b, _) = placed_outputs(XDC_FLAT);
    assert_eq!(a, b, "flat and bracket XDC spellings placed different IOB counts");
}

/// An unconstrained output bit is dropped, not fatal -- the other half of the
/// round-38 fix. Constraining only `led[0]` must still build, with exactly one
/// output IOB, rather than aborting the whole design.
#[test]
fn a_partly_constrained_vector_still_builds_with_the_constrained_bits() {
    let xdc = r#"
set_property PACKAGE_PIN Y9  [get_ports {clk}]
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]
set_property PACKAGE_PIN T22 [get_ports {led[0]}]
create_clock -period 10.000 -name sys_clk [get_ports {clk}]
"#;
    let (outs, _) = placed_outputs(xdc);
    println!("partly constrained: {outs} output IOB(s)");
    assert_eq!(
        outs, 1,
        "only led[0] is constrained, so exactly one output IOB should be placed"
    );
}

/// `bit_of_port` must accept both bit spellings and reject near-misses. The
/// separator and the digits-only check are what stop a port named `led` from
/// swallowing an unrelated `ledx_0` or a non-numeric `led_foo`.
#[test]
fn bit_spellings_are_accepted_and_near_misses_rejected() {
    // Exercised through the public parse+apply path: a key that resolves sets a
    // LOC attribute on the matching port.
    let mut design = helion_sv::synth_sv(SRC, "v.sv").expect("synth");
    let pins = PinConstraints::parse(
        r#"
set_property PACKAGE_PIN T22 [get_ports {led_0}]
set_property PACKAGE_PIN U22 [get_ports {led[1]}]
set_property PACKAGE_PIN U21 [get_ports {ledx_2}]
set_property PACKAGE_PIN V22 [get_ports {led_foo}]
"#,
    );
    pins.apply(&mut design);
    let led = design
        .ports
        .iter()
        .find(|p| p.name == "led")
        .expect("port led");
    assert_eq!(
        led.attrs.get("LOC[0]"),
        Some("T22"),
        "flat spelling led_0 must set LOC[0]"
    );
    assert_eq!(
        led.attrs.get("LOC[1]"),
        Some("U22"),
        "bracket spelling led[1] must set LOC[1]"
    );
    assert!(
        led.attrs.get("LOC[2]").is_none(),
        "ledx_2 must NOT resolve to led bit 2 -- the separator is required"
    );
    assert!(
        led.attrs.get("LOC[foo]").is_none() && led.attrs.get("LOC").is_none(),
        "led_foo must NOT resolve -- the suffix must be all digits"
    );
}
