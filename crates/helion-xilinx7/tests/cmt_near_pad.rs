//! G58: an MMCM goes on the free CMT site nearest its input pad, so a clock
//! pad on the far side of the chip from the first CMT still reaches its MMCM
//! over the dedicated (same-region) clock path. Before, the first free site
//! was taken whatever the pad, and a pad in bank 34 got the bank 13 CMT.
//!
//! Requires the X-Ray zynq7 database (`HELION_XILINX_DB`, see `device.rs`).

use helion_backend::Backend;
use helion_xilinx7::backend::Xilinx7Backend;
use helion_xilinx7::place::parse_site_xy;

fn mmcm_counter(pin: &str) -> (Xilinx7Backend, helion_xilinx7::pack::Packed7) {
    let src = "module top(input clk, output logic led);
  logic fb, c0, locked, c;
  MMCME2_BASE #(.CLKIN1_PERIOD(10.0), .CLKFBOUT_MULT_F(10.0), .CLKOUT0_DIVIDE_F(20.0)) u_mmcm (
    .CLKIN1(clk), .CLKFBIN(fb), .CLKFBOUT(fb), .CLKOUT0(c0), .LOCKED(locked), .PWRDWN(1'b0), .RST(1'b0));
  BUFG u_bufg (.I(c0), .O(c));
  logic [7:0] n = 8'd0;
  always_ff @(posedge c) n <= n + 8'd1;
  assign led = locked & n[7];
endmodule
";
    helion_sv::v2::set_hard(true);
    helion_sv::v2::dag::set_carry_arith(true);
    let mut d = helion_sv::synth_sv_v2(src, "cmt_near_pad.sv", Some("top")).expect("synth").design;
    d.set_loc("clk", pin).unwrap();
    d.set_loc("led", "T22").unwrap();
    let be = Xilinx7Backend::open("xc7z020").expect("xc7z020 device");
    let packed = be.pack(&d).expect("pack");
    (be, packed)
}

/// The pad reaches the MMCM's CLKIN1 on clock wires only: no general
/// interconnect (Vivado refuses that as CLOCK_DEDICATED_ROUTE).
fn assert_dedicated(routed: &helion_xilinx7::route::Routed7) {
    let clkin = routed.placed.packed.cmts[0].clkin1_net.clone().expect("CLKIN1 net");
    let net = routed.nets.iter().find(|n| n.net == clkin).expect("CLKIN1 net routed");
    if let helion_xilinx7::route::RouteKind::Pips(pips) = &net.kind {
        let int: Vec<&str> = pips.iter().filter(|p| p.tile.starts_with("INT_")).map(|p| p.tile.as_str()).collect();
        assert!(int.is_empty(), "CLKIN1 net `{clkin}` uses general interconnect in {int:?}");
        // Before G58 the far CMT was reached through BUFGCTRL0 used as an
        // unconfigured route-through onto the global clock.
        let bufg: Vec<String> = pips.iter().filter(|p| p.dst_wire.contains("BUFGCTRL")).map(|p| format!("{}.{}", p.tile, p.dst_wire)).collect();
        assert!(bufg.is_empty(), "CLKIN1 net `{clkin}` passes through a global buffer: {bufg:?}");
    }
}

#[test]
fn an_mmcm_fed_from_a_bank_34_pad_goes_on_the_nearest_cmt_and_routes() {
    let (be, packed) = mmcm_counter("L18"); // IO_L12P_T1_MRCC_34, right side
    let placed = be.place(&packed).expect("place");
    let pad = placed.iobs.iter().find(|i| i.port == "clk").expect("clk pad");
    let (px, py) = parse_site_xy(&pad.tile).unwrap();
    let (cx, cy) = parse_site_xy(&placed.cmts[0].tile).unwrap();
    eprintln!("pad {} ({px},{py}) -> {} ({cx},{cy})", pad.tile, placed.cmts[0].tile);
    assert_ne!(placed.cmts[0].tile, "CMT_TOP_R_LOWER_B_X8Y9", "the bank 13 CMT is across the chip from L18");
    let routed = be.route(&placed).expect("route");
    assert_dedicated(&routed);
}

#[test]
fn the_zedboard_clock_pad_keeps_its_cmt() {
    let (be, packed) = mmcm_counter("Y9"); // the board oscillator, bank 13
    let placed = be.place(&packed).expect("place");
    assert_eq!(placed.cmts[0].tile, "CMT_TOP_R_LOWER_B_X8Y9", "rung 4 was proven on this CMT");
    assert_dedicated(&be.route(&placed).expect("route"));
}
