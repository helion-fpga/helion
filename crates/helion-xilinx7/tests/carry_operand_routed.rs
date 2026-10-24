//! The carry lane's operand must be routed, not just configured.
//!
//! A CARRY4 lane's `S` LUT is a 2-input XOR: it needs the counter bit AND the
//! constant 1. The bit arrives on the slice input pin (`A6`..`D6`) through the
//! interconnect's input mux. Nothing in the slice wires `AQ` to `A6`, although
//! both are pins of the same slice -- so a net driven by a flop and read by its
//! own lane's operand still has to leave the slice and come back.
//!
//! If that hop is classified as site-internal the design configures perfectly,
//! every slice-level check passes (carry chain, `PRECYINIT`, `FFMUX.XOR`,
//! ZINI/ZRST census) and the LED stays **static**: clocked flops latching a
//! constant, because the adder sums against a floating operand.

use helion_xilinx7::device::Device;
use helion_xilinx7::xdevice::{build_node_map, XDevice};
use helion_xilinx7::route::RouteKind;
use helion_xilinx7::{pack, place, route};

const SRC: &str = r#"
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

const XDC: &str = "\
set_property PACKAGE_PIN Y9 [get_ports {clk}]\n\
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]\n\
set_property PACKAGE_PIN T22 [get_ports {LD0}]\n\
set_property IOSTANDARD LVCMOS33 [get_ports {LD0}]\n";

fn db_root() -> std::path::PathBuf {
    std::env::var("HELION_XILINX_DB")
        .map(std::path::PathBuf::from)
        .expect("HELION_XILINX_DB must point at a prjxray-db checkout")
}

#[test]
fn every_carry_lane_operand_is_routed() {
    let mut dev = Device::load_part_from_db(&db_root(), "xc7z020").expect("load device");
    let nm = build_node_map(&mut dev);
    let mut design = helion_sv::synth_sv(SRC, "blinky.sv").expect("synth");
    helion_xilinx7::build::PinConstraints::parse(XDC).apply(&mut design);

    let packed = pack(&design).expect("pack");
    let mut xdev = XDevice::new(&dev, nm);
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = place(&packed, &xdev).expect("place");
    let routed = route(&placed, &xdev).expect("route");

    // Collect every (site, pin) the router actually delivered a net to, from
    // the pips it emitted. A pip's destination wire is the interconnect wire;
    // the slice pin is reached from the tile that owns the site.
    let mut routed_pairs: Vec<(String, String)> = Vec::new();
    let mut operand_nets: Vec<String> = Vec::new();
    for c in &packed.carries {
        for lane in 0..4 {
            if let Some((a, b)) = &c.lane_operands[lane] {
                operand_nets.extend([a.clone(), b.clone()].into_iter().flatten());
            }
        }
    }
    operand_nets.sort();
    operand_nets.dedup();
    assert!(
        !operand_nets.is_empty(),
        "the counter must publish lane operands; an empty list means packing \
         stopped recording them and this test would pass vacuously"
    );

    let mut seen_operands = 0usize;
    for net in &routed.nets {
        if !operand_nets.contains(&net.net) {
            continue;
        }
        seen_operands += 1;
        match &net.kind {
            RouteKind::Pips(pips) => {
                for p in pips {
                    routed_pairs.push((p.tile.clone(), p.dst_wire.clone()));
                }
            }
            // The whole point: an operand read by a slice input pin cannot be
            // site-internal, because the slice has no internal path to it.
            RouteKind::SiteInternal => panic!(
                "operand net `{}` was classified site-internal; its load is a \
                 slice input pin that the slice cannot reach from inside",
                net.net
            ),
            RouteKind::ClockSpine => {}
        }
    }

    // Guard against a vacuous pass. Every operand is a routed net in its own
    // right, so finding none means `routed.nets` does not carry them (they are
    // `SiteInternal`, or named differently) and the loop above asserted
    // nothing at all. That is exactly how an earlier version of this test
    // "passed" on a design with floating operands.
    assert_eq!(
        seen_operands,
        operand_nets.len(),
        "expected all {} carry-operand nets to be routed nets; found {}.          Collecting none would make the assertions below vacuous.",
        operand_nets.len(),
        seen_operands
    );

    // And the routing must actually reach an input mux: an operand that ends
    // anywhere else never drives the LUT.
    eprintln!("DBG operand_nets={} seen={} pairs={:?}", operand_nets.len(), seen_operands,
        routed_pairs.iter().take(12).collect::<Vec<_>>());
    let imux = routed_pairs
        .iter()
        .filter(|(_, w)| w.starts_with("IMUX"))
        .count();
    assert!(
        imux > 0,
        "no operand net reaches an `IMUX`; the carry's `S` LUT has a floating \
         input, so the chain cannot advance. Routed operand destinations: {:?}",
        routed_pairs
    );
}
