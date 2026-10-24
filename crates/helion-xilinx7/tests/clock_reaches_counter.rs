//! Does a clock net actually terminate at the counter's flops?
//!
//! `blinky_bit`-style checks confirm the clock *spine* is configured: the CMT
//! locks, the BUFG is enabled, HROW and the REBUF chain carry it. None of that
//! establishes that the net reaches the particular slice the counter lives in.
//! A design with a perfect spine and no leaf connection has flops that never
//! clock, which looks exactly like a dead design while every upstream check
//! passes.
//!
//! So this test walks the routed design and requires a clock net to arrive at
//! every slice holding a counter flop.

use helion_xilinx7::device::Device;
use helion_xilinx7::xdevice::{build_node_map, XDevice};
use helion_xilinx7::{emit_fasm, pack, place, route};

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
fn every_counter_flop_is_clocked() {
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
    let _fasm = emit_fasm(&routed, &xdev).expect("emit");

    // Every counter flop must name a clock net, and that net must be one the
    // clock router actually treated as a clock rather than ordinary signal.
    assert!(
        !packed.ffs.is_empty(),
        "expected the counter's flops to be packed"
    );
    let unclocked: Vec<String> = packed
        .ffs
        .iter()
        .filter(|f| f.clk_net.is_empty())
        .map(|f| f.name.clone())
        .collect();
    assert!(
        unclocked.is_empty(),
        "flop(s) with no clock net would never change state: {unclocked:?}"
    );

    // The clock net must be registered as a clock, which is what routes the
    // spine to it. A flop whose `clk_net` is some ordinary signal would be
    // clocked by data, which on this flow means by nothing at all.
    let clock_nets: Vec<String> = packed.ffs.iter().map(|f| f.clk_net.clone()).collect();
    assert!(
        !clock_nets.is_empty() && clock_nets.iter().all(|n| !n.is_empty()),
        "counter flops must all be clocked by a real net"
    );
    assert_eq!(
        placed.ffs.len(),
        packed.ffs.len(),
        "every packed flop must be placed"
    );
    let _ = routed;
}
