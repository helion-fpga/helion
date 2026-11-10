//! A design with no carry chain must not spend SLICEM halves.
//!
//! Only one half of each CLB is a SLICEM, and it is the scarce resource: it is
//! the only half that can host distributed RAM and the only half a carry chain
//! can run in (`column_run` filters candidates to it). Spending it on a plain
//! LUT or flop costs a capability a SLICEL would not have, so it is the wrong
//! trade whenever both work.
//!
//! `slice_near` broke ties arbitrarily -- `min_by_key` on Manhattan distance
//! with no half preference, i.e. whichever `Device::slices()` listed first. On
//! the ZedBoard that is systematically the SLICEM. Measured against Vivado on
//! three designs:
//!
//! ```text
//! design              Vivado SLICEM/SLICEL   ours (before)
//! toggle flop              0 / 2                 2 / 0
//! 4-bit counter            0 / 2                 4 / 0
//! blinky (carry chain)    26 / 2                26 / 2   <- matched
//! ```
//!
//! The blinky agreed only because it HAS a carry chain, which is exactly how
//! the defect stayed hidden: the one design the flow was calibrated against was
//! the one design for which the wrong rule gave the right answer.
//!
//! The guard is on the emitted FASM, not on the placer's internals, so it also
//! catches a future rule that reaches the same wrong result another way. It is
//! deliberately paired with the opposite assertion for a carry design, so a
//! "fix" that simply bans SLICEM everywhere fails here.

use helion_xilinx7::device::Device;
use helion_xilinx7::xdevice::{build_node_map, XDevice};
use helion_xilinx7::{emit_fasm, pack, place, route};

/// A plain toggle: one flop, no adder, no carry chain, no memory.
const TOGGLE: &str = r#"
module top(input wire clk, output reg q = 0);
  always @(posedge clk) q <= ~q;
endmodule
"#;

const XDC_TOGGLE: &str = "\
set_property PACKAGE_PIN Y9 [get_ports {clk}]\n\
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]\n\
set_property PACKAGE_PIN T22 [get_ports {q}]\n\
set_property IOSTANDARD LVCMOS33 [get_ports {q}]\n";

/// A registered counter: has a carry chain, and MUST be allowed SLICEM.
const COUNTER: &str = r#"
module blinky (input wire clk, output reg LD0);
  reg [24:0] count = 25'd0;
  always @(posedge clk) begin
      count <= count + 1'b1;
      LD0   <= count[24];
  end
endmodule
"#;

const XDC_COUNTER: &str = "\
set_property PACKAGE_PIN Y9 [get_ports {clk}]\n\
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]\n\
set_property PACKAGE_PIN T22 [get_ports {LD0}]\n\
set_property IOSTANDARD LVCMOS33 [get_ports {LD0}]\n";

/// Emit FASM for `src`/`xdc` and return the feature names using a given half.
fn fasm_for(src: &str, xdc: &str) -> Vec<String> {
    let db_root = std::env::var("HELION_XILINX_DB")
        .map(std::path::PathBuf::from)
        .expect("HELION_XILINX_DB must point at a prjxray-db checkout");
    let device = Device::load_part_from_db(&db_root, "xc7z020").expect("load xc7z020");
    let nm = build_node_map(&mut device.clone());

    let mut design = helion_sv::synth_sv(src, "half.sv").expect("synth");
    helion_xilinx7::build::PinConstraints::parse(xdc).apply(&mut design);

    let packed = pack(&design).expect("pack");
    let mut xdev = XDevice::new(&device, nm);
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = place(&packed, &xdev).expect("place");
    let routed = route(&placed, &xdev).expect("route");
    emit_fasm(&routed, &xdev).expect("emit").fasm
}

/// Slice half a FASM line configures.
///
/// The shape is `TILE.SLICEM_X0.AFF.ZINI` -- the half is the SECOND dot-field,
/// not part of the tile name. Parsing the first field finds only `CLBLM_R` and
/// reports "no slice configuration at all" for a perfectly good design, which
/// makes any guard built on it vacuous.
fn half_of(line: &str) -> Option<&'static str> {
    let bel = line.split_whitespace().next()?;
    let mut fields = bel.split('.');
    let _tile = fields.next()?;
    let site = fields.next()?;
    if site.starts_with("SLICEM") {
        Some("M")
    } else if site.starts_with("SLICEL") {
        Some("L")
    } else {
        None
    }
}

#[test]
fn a_design_without_a_carry_chain_does_not_spend_slicem() {
    let fasm = fasm_for(TOGGLE, XDC_TOGGLE);
    let mut used_m: Vec<&str> = Vec::new();
    let mut used_l: Vec<&str> = Vec::new();
    for line in &fasm {
        match half_of(line) {
            Some("M") => used_m.push(line.as_str()),
            Some("L") => used_l.push(line.as_str()),
            _ => {}
        }
    }
    assert!(
        used_m.is_empty(),
        "a toggle with no carry chain must not configure a SLICEM half, \
         but these lines do: {used_m:?}"
    );
    assert!(
        !used_l.is_empty(),
        "expected the toggle's flop to land in a SLICEL half; no SLICEL \
         configuration found at all in {fasm:?}"
    );
}

#[test]
fn a_carry_chain_still_gets_slicem() {
    // The opposite direction, so a blanket "never use SLICEM" would fail:
    // a carry chain CANNOT run in a SLICEL half, and the reference puts the
    // blinky's entire chain in SLICEM (26 flops there, 2 in SLICEL).
    let fasm = fasm_for(COUNTER, XDC_COUNTER);
    let used_m = fasm
        .iter()
        .filter(|l| matches!(half_of(l), Some("M")))
        .count();
    assert!(
        used_m > 0,
        "the registered blinky's carry chain must configure SLICEM halves; \
         none found, so the chain has been pushed into SLICEL where it cannot run"
    );
}
