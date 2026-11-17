//! A clock made in the fabric (a toggle flop halving the pad clock) on the
//! ZedBoard part: it gets its own global buffer, the route from the flop to
//! that buffer never passes through another buffer, and every global buffer
//! that carries a clock is configured.
//!
//! Requires the X-Ray zynq7 database (`HELION_XILINX_DB`, see `device.rs`).
//! `x7v flow` checks the same design from its bitstream.

use helion_backend::Backend;
use helion_ir::{CellKind, Design, PortDir};
use helion_xilinx7::backend::Xilinx7Backend;
use helion_xilinx7::route::RouteKind;

fn divided_counter() -> Design {
    let mut d = Design::new("div");
    d.add_port("clk", PortDir::In);
    d.add_port("q", PortDir::Out);
    d.set_loc("clk", "Y9").unwrap();
    d.set_loc("q", "T22").unwrap();
    // clk_half toggles on every clk edge.
    d.add_cell("half_reg", CellKind::Hff);
    d.connect("half_n", "half_reg", "D");
    d.connect("half", "half_reg", "Q");
    d.connect("clk", "half_reg", "CLK");
    d.add_cell("u_half_inv", CellKind::Lut6 { init: 0x5555_5555_5555_5555 });
    d.connect("half", "u_half_inv", "I0");
    d.connect("half_n", "u_half_inv", "O");
    // q toggles on every clk_half edge.
    d.add_cell("q_reg", CellKind::Hff);
    d.connect("q_n", "q_reg", "D");
    d.connect("q_int", "q_reg", "Q");
    d.connect("half", "q_reg", "CLK");
    d.add_cell("u_q_inv", CellKind::Lut6 { init: 0x5555_5555_5555_5555 });
    d.connect("q_int", "u_q_inv", "I0");
    d.connect("q_n", "u_q_inv", "O");
    d.add_cell("u_iob", CellKind::IobOut);
    d.connect("q_int", "u_iob", "I");
    d.connect("q", "u_iob", "PAD");
    d
}

/// An 8-bit counter on a halved clock, synthesized by the v2 front end, with
/// the pins `x7v flow` gives it. Before the router stopped at global buffer
/// inputs, this one reached its buffer through four unused ones.
fn divided_counter_v2() -> helion_xilinx7::pack::Packed7 {
    let src = "module top(input clk, input rst, input en, output logic [7:0] q);
  logic clk_half = 1'b0;
  always_ff @(posedge clk) clk_half <= ~clk_half;
  always_ff @(posedge clk_half) begin
    if (rst) q <= 8'd0;
    else if (en) q <= q + 8'd3;
  end
endmodule
";
    let mut d = helion_sv::synth_sv_v2(src, "div_count.sv", Some("top")).expect("synth").design;
    for (p, pin) in [("clk", "Y9"), ("rst", "F22"), ("en", "G22")] {
        d.set_loc(p, pin).unwrap();
    }
    let be = Xilinx7Backend::open("xc7z020").expect("xc7z020 device");
    let mut packed = be.pack(&d).expect("pack");
    let q_pins = ["U14", "U19", "W22", "V22", "U21", "U22", "T21", "T22"];
    let bit_of: std::collections::HashMap<String, usize> = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::IobOut))
        .filter_map(|c| Some((helion_ir::resolve(c.name).to_string(), c.attrs.get("BIT")?.parse().ok()?)))
        .collect();
    for iob in packed.iobs.iter_mut() {
        if let Some(&b) = bit_of.get(&iob.name) {
            iob.loc = Some(q_pins[b].to_string());
        } else if let Some(b) = iob.net.strip_prefix("q_").and_then(|b| b.parse::<usize>().ok()) {
            iob.loc = Some(q_pins[b].to_string());
        }
    }
    packed
}

#[test]
fn fabric_clock_routes_into_its_own_global_buffer() {
    let be = Xilinx7Backend::open("xc7z020").expect("xc7z020 device");
    let packed = be.pack(&divided_counter()).expect("pack");
    assert_eq!(packed.bufgs.len(), 2, "a buffer for the pad clock and one for the divided clock");
    let q = packed.ffs.iter().find(|f| f.name == "q_reg").unwrap();
    assert_eq!(q.clk_net, "half_bufg");
    check_clock_routes(&be, &packed);
    check_clock_routes(&be, &divided_counter_v2());
}

fn check_clock_routes(be: &Xilinx7Backend, packed: &helion_xilinx7::pack::Packed7) {
    let placed = be.place(packed).expect("place");
    let routed = be.route(&placed).expect("route");

    // No route leaves a global buffer input: that would pass through a
    // buffer that is not configured, and the clock would never arrive.
    for n in &routed.nets {
        if let RouteKind::Pips(pips) = &n.kind {
            for p in pips {
                let w = &p.src_wire;
                assert!(
                    !(w.starts_with("CLK_BUFG_BUFGCTRL") && (w.ends_with("_I0") || w.ends_with("_I1"))),
                    "net {} passes through a global buffer: {}.{} -> {}",
                    n.net,
                    p.tile,
                    p.src_wire,
                    p.dst_wire
                );
            }
        }
    }

    // Every buffer that drives a global line is configured.
    let bs = be.bitstream(&routed).expect("bitstream");
    let text = bs.fasm.join("\n");
    for b in &placed.bufgs {
        assert!(
            text.contains(&format!("BUFGCTRL.{}.IN_USE", b.site)),
            "buffer {} at {} is not configured:\n{text}",
            b.name,
            b.site
        );
    }
}

/// The timing analysis gives each clock its own period and refuses a data
/// path between them.
#[test]
fn fabric_clock_is_timed_per_domain() {
    let be = Xilinx7Backend::open("xc7z020").expect("xc7z020 device");
    let packed = be.pack(&divided_counter()).expect("pack");
    let routed = be.route(&be.place(&packed).expect("place")).expect("route");
    let period = |c: &str| match c {
        "clk_bufg" => Some(10_000),
        "half_bufg" => Some(20_000),
        _ => None,
    };
    let a = helion_xilinx7::timing::analyze_clocks(&routed, &period).expect("timing");
    assert_eq!(a.endpoints, 2);
    assert!(a.wns_ps > 0, "slack {}", a.wns_ps);
    let doms: Vec<&str> = a.clocks.iter().map(|(c, _)| c.as_str()).collect();
    assert_eq!(doms, ["clk_bufg", "half_bufg"]);
    assert_eq!(a.clocks.iter().map(|c| c.1).min(), Some(a.wns_ps));
    assert!(helion_xilinx7::timing::analyze_clocks(&routed, &|c| (c == "clk_bufg").then_some(10_000)).is_err());
}
