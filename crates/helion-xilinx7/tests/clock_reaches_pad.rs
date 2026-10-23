//! A clock derived from a pin must actually reach its buffer.
//!
//! This is the bug that made the ZedBoard sit inert while `done` read 1. The
//! bitstream was structurally valid -- the CRC passed and the fabric came up --
//! but the clock net from the Y9 pad to the BUFG routed like this:
//!
//!     BUFGCTRL1_I0 <- FBG_OUT1
//!     BUFGCTRL2_I0 <- FBG_OUT2
//!     ...
//!     BUFGCTRL0_I0 <- FBG_OUT1
//!
//! 19 pips cascading through unused BUFG slots *inside* the buffer tile, and
//! not one pip leaving the pad. The search had "arrived" at its goal without
//! travelling there, so every flip-flop sat on a buffer with no input.
//!
//! The A* heuristic triggered it: once `tile_xy` gave real distances, the
//! slot-to-slot chain looked like a shorter path than the genuine dedicated
//! route through the IOI, HCLK_CMT and HROW.
//!
//! These tests are behavioural rather than name-based on purpose. The guards
//! that failed to prevent this matched wire names (`FBG_OUT`, `CK_MUXED`) on
//! `dst_wire`, but in `BUFGCTRL1_I0 <- FBG_OUT1` the tell-tale string is the
//! *source*, so the cascade passed straight through them.

use helion_xilinx7::route::{RouteKind, RouteQuery};

fn db_root() -> std::path::PathBuf {
    std::env::var("HELION_XILINX_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../work/prjxray-db")
        })
}

const XDC: &str = "\
set_property PACKAGE_PIN Y9 [get_ports {clk}]
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]
set_property PACKAGE_PIN T22 [get_ports {led[0]}]
set_property PACKAGE_PIN T21 [get_ports {led[1]}]
set_property IOSTANDARD LVCMOS33 [get_ports {led}]
";

/// The counter pins its lower bits to LEDs so the FFs cannot be optimized away.
const SRC: &str = "module top(input logic clk, output logic [1:0] led);
    logic [26:0] cnt;
    always_ff @(posedge clk) cnt <= cnt + 27'b1;
    assign led[0] = cnt[26];
    assign led[1] = cnt[25];
endmodule";

/// Route the design and return every routed net as `(name, pips)`.
fn routed() -> Vec<(String, Vec<helion_xilinx7::route::RoutedPip>)> {
    let mut design = helion_sv::synth_sv(SRC, "t.sv").unwrap();
    helion_xilinx7::build::PinConstraints::parse(XDC).apply(&mut design);
    let mut device =
        helion_xilinx7::device::Device::load_part_from_db(&db_root(), "xc7z020").unwrap();
    let nm = helion_xilinx7::xdevice::build_node_map(&mut device);
    let packed = helion_xilinx7::pack::pack(&design).unwrap();
    let mut xdev = helion_xilinx7::xdevice::XDevice::new(&device, nm);
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = helion_xilinx7::place::place(&packed, &xdev).unwrap();
    let r = helion_xilinx7::route::route(&placed, &xdev).unwrap();
    r.nets
        .iter()
        .map(|n| {
            let p = match &n.kind {
                RouteKind::Pips(p) => p.clone(),
                _ => Vec::new(),
            };
            (n.net.clone(), p)
        })
        .collect()
}

#[test]
fn clock_net_leaves_the_input_pad() {
    // The net carrying the pad's IBUF output must include a pip whose tile is
    // the pad's IOI tile. Without one, the pad is not connected to anything and
    // the clock never enters the fabric.
    let nets = routed();
    let clk = nets
        .iter()
        .find(|(n, _)| n == "clk_ibuf")
        .expect("there must be a clk_ibuf net for a port-driven clock");
    let leaves_pad = clk.1.iter().any(|p| p.tile.contains("LIOI3") || p.tile.contains("LIOB33"));
    assert!(
        leaves_pad,
        "clk_ibuf does not touch the pad's own tile, so the clock is unconnected.\n\
         Its pips were:\n{}",
        clk.1.iter()
            .map(|p| format!("    {}.{} <- {}", p.tile, p.dst_wire, p.src_wire))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn clock_net_does_not_cascade_through_unused_bufg_slots() {
    // The signature of the bug: many pips inside one BUFG tile hopping between
    // slots (`BUFGCTRL<n>_O -> ... -> BUFGCTRL<m>_I0`), which is an illegal
    // cascade and also a route that never leaves the tile.
    let nets = routed();
    let clk = nets.iter().find(|(n, _)| n == "clk_ibuf").unwrap();
    let in_bufg: Vec<&helion_xilinx7::route::RoutedPip> = clk
        .1
        .iter()
        .filter(|p| p.tile.contains("CLK_BUFG_"))
        .collect();
    // Entering the buffer's own mux is expected; a handful of pips is fine.
    // The cascade produced 19 and touched four different slots.
    let slots: std::collections::HashSet<&str> = in_bufg
        .iter()
        .filter_map(|p| {
            let w = &p.dst_wire;
            w.split("BUFGCTRL").nth(1).map(|rest| {
                rest.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("")
            })
        })
        .collect();
    assert!(
        slots.len() <= 1,
        "the clock net touches {} BUFG slots ({slots:?}); it must only enter \
         the one buffer it uses. Pips:\n{}",
        slots.len(),
        in_bufg
            .iter()
            .map(|p| format!("    {}.{} <- {}", p.tile, p.dst_wire, p.src_wire))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn clock_net_reaches_the_buffer_through_the_clock_network() {
    // The genuine path goes pad -> IOI -> HCLK_CMT -> HROW -> BUFG. Requiring
    // the CMT and HROW stages distinguishes a real clock route from any
    // shortcut that happens to end at the right mux.
    let nets = routed();
    let clk = nets.iter().find(|(n, _)| n == "clk_ibuf").unwrap();
    let tiles: Vec<&str> = clk.1.iter().map(|p| p.tile.as_str()).collect();
    for want in ["HCLK_CMT", "CLK_HROW"] {
        assert!(
            tiles.iter().any(|t| t.contains(want)),
            "clk_ibuf never passes through {want}, so it is not using the clock \
             network. Tiles: {tiles:?}"
        );
    }
}

#[test]
fn the_buffer_output_leaves_for_the_spine() {
    // The mirror of the bug on the other side: `emit.rs` writes the BUFG input
    // select, and the FFs hang off the BUFG output. If the output cannot reach
    // the spine the FFs are equally dead, so guard both directions.
    let nets = routed();
    let bufg = nets
        .iter()
        .find(|(n, _)| n == "clk_bufg")
        .expect("the buffered clock is a net of its own");
    assert!(
        bufg.1.iter().any(|p| p.dst_wire.contains("CK_GCLK")),
        "the BUFG output does not reach a global clock spine wire"
    );
    assert!(
        bufg.1.iter().any(|p| p.dst_wire.starts_with("CLBL") || p.dst_wire.contains("_CLK")),
        "the buffered clock never reaches a slice clock pin"
    );
}
