//! One flop driving 32 pads spread over all four PL banks of the ZedBoard
//! part: the router branches each far load near the source instead of
//! chaining it off the nearest load already reached (G38).
//!
//! Requires the X-Ray zynq7 database (`HELION_XILINX_DB`, see `device.rs`).

use helion_backend::Backend;
use helion_ir::{CellKind, Design, PortDir};
use helion_xilinx7::backend::Xilinx7Backend;

/// Eight pins per bank (13, 33, 34, 35), from the bottom of each bank to its top.
const PINS: [&str; 32] = [
    "U7", "V4", "Y4", "AA6", "AA9", "AB12", "V12", "R7", "U14", "Y15", "AA16", "Y16", "Y19", "AB22", "W20", "U19", "R15",
    "T17", "P20", "N20", "K19", "K18", "L17", "H15", "H18", "E20", "B21", "C20", "C17", "B15", "G15", "H17",
];

#[test]
fn chip_wide_net_branches_near_its_source() {
    let mut d = Design::new("fan");
    for (p, pin) in [("clk", "Y9"), ("sw", "F22")] {
        d.add_port(p, PortDir::In);
        d.set_loc(p, pin).unwrap();
    }
    d.add_cell("src_reg", CellKind::Hff);
    d.connect("sw", "src_reg", "D");
    d.connect("clk", "src_reg", "CLK");
    d.connect("fan", "src_reg", "Q");
    for (i, pin) in PINS.iter().enumerate() {
        let port = format!("q{i}");
        d.add_port(&port, PortDir::Out);
        d.set_loc(&port, pin).unwrap();
        d.add_cell(&format!("u_q{i}"), CellKind::IobOut);
        d.connect("fan", &format!("u_q{i}"), "I");
        d.connect(&port, &format!("u_q{i}"), "PAD");
    }
    let be = Xilinx7Backend::open("xc7z020").expect("xc7z020 device");
    let routed = be.route(&be.place(&be.pack(&d).expect("pack")).expect("place")).expect("route");
    let delays: Vec<u32> = routed.load_delays.iter().filter(|((n, _, _), _)| n == "fan").map(|(_, &ps)| ps).collect();
    assert_eq!(delays.len(), 32, "a routed delay per pad");
    let worst = *delays.iter().max().unwrap();
    // Vivado crosses this chip with a 286-load net in 8.6 ns; chained from
    // pad to pad the last one arrived after 15.2 ns.
    assert!(worst < 8_000, "worst pad {worst} ps");
}
