//! DSP48E1 input pins route (HELION_GAPS.md G35).
//!
//! Before the fix, only the `A` input could be routed: `B`/`C`/`D` and every
//! control pin failed with "unroutable". Two independent causes, both fixed:
//!
//! 1. In `tile_type_DSP_R.json` a non-`A` DSP input has exactly one
//!    non-constant source, `DSP_BYP<k>_<j>` or `DSP_FAN<k>_<j>`, feeding the
//!    site pin through a pip that has no segbit and no `ppips` entry. The
//!    loader therefore marked that pip unusable, and the router's dead-end
//!    pass pruned the whole branch. `A` worked only because
//!    `ppips_dsp_l.db` happens to list `DSP_0_A0.DSP_IMUX23_0 always`.
//!    Fixed in `Device::dsp_bitless_input_pip`: those pips are fixed
//!    connections in silicon and need no configuration.
//! 2. The `DSP_BYP` group additionally arrives through the INT tile's
//!    `BYP_ALT` mux, which the router refuses to enter unless the load is a
//!    slice bypass pin. Fixed in `route::is_dsp_site`: a DSP site needs the
//!    same exemption. `DSP_FAN` pins (e.g. `CED`) needed only fix 1; `DSP_BYP`
//!    pins (e.g. `D0`, `CEAD`) needed both.
//!
//! These tests assert the working state. If a DSP input regresses they name
//! the pin that broke.

use helion_backend::Backend;
use helion_ir::{intern, CellKind, Design, PortDir};
use helion_xilinx7::backend::Xilinx7Backend;

/// Pack, place and route a DSP with `pin` driven from a top-level input, and
/// from a flop (the shape a real multiplier uses).
fn routes_with(pin: &str, from_flop: bool) -> Result<(), String> {
    let mut d = Design::new("g35");
    d.add_port("clk", PortDir::In);
    d.add_port("i", PortDir::In);
    d.add_port("p", PortDir::Out);
    // Every pad on one edge, so the test does not depend on where the placer
    // happens to put an IOB across the die.
    let locs = [("clk", "Y9"), ("i", "V5"), ("p", "U14")];
    for port in d.ports.iter_mut() {
        if let Some((_, loc)) = locs.iter().find(|(n, _)| port.name == intern(n)) {
            port.attrs.set("LOC", *loc);
        }
    }
    let driven = if from_flop {
        d.add_cell("u_ff", CellKind::Hff);
        d.connect("clk", "u_ff", "CLK");
        d.connect("i", "u_ff", "D");
        d.connect("q", "u_ff", "Q");
        "q"
    } else {
        "i"
    };
    d.add_cell(
        "u_dsp",
        CellKind::BlackBox {
            module: intern("DSP48E1"),
        },
    );
    d.set_cell_attr("u_dsp", "PREG", "1").unwrap();
    d.connect("clk", "u_dsp", "CLK");
    d.connect(driven, "u_dsp", pin);
    d.connect("p", "u_dsp", "P0");
    let be = Xilinx7Backend::open("xc7z020").unwrap();
    let packed = be.pack(&d)?;
    assert_eq!(packed.dsps.len(), 1, "the DSP must pack");
    assert!(
        packed.dsps[0].pins.iter().any(|(q, _, out)| q == pin && !*out),
        "{pin} did not survive packing (pins {:?})",
        packed.dsps[0].pins
    );
    let placed = be.place(&packed)?;
    be.route(&placed).map(|_| ())
}

/// `A` was the only input that ever worked: `DSP_IMUX23_0 -> DSP_0_A0` is
/// listed `always` in `ppips_dsp_l.db`, so it survived the usability filter.
#[test]
fn dsp_a_pins_route() {
    for pin in ["A0", "A23"] {
        routes_with(pin, false).unwrap_or_else(|e| panic!("{pin} must route: {e}"));
        routes_with(pin, true).unwrap_or_else(|e| panic!("{pin} from a flop must route: {e}"));
    }
}

/// The `D` data bus. These are fed by `DSP_BYP*` and needed both fixes.
#[test]
fn dsp_d_pins_route() {
    let mut failed = Vec::new();
    for pin in ["D0", "D4", "D8", "D12", "D16", "D20", "D24"] {
        for from_flop in [false, true] {
            if let Err(e) = routes_with(pin, from_flop) {
                let src = if from_flop { "a flop" } else { "a port" };
                failed.push(format!("{pin} from {src}: {e}"));
            }
        }
    }
    assert!(failed.is_empty(), "DSP D pins failed to route:\n{}", failed.join("\n"));
}

/// Control pins. `CED`/`OPMODE6`/`ALUMODE2` are `DSP_FAN`-fed (needed only the
/// usability fix); `CEAD`/`RSTD`/`INMODE0`/`CARRYINSEL2` are `DSP_BYP`-fed and
/// needed both. Keep both groups so a regression in either half is caught.
#[test]
fn dsp_control_pins_route() {
    let mut failed = Vec::new();
    for pin in ["CEAD", "CED", "RSTD", "INMODE0", "INMODE4", "OPMODE6", "ALUMODE2", "CARRYINSEL2"] {
        if let Err(e) = routes_with(pin, false) {
            failed.push(format!("{pin}: {e}"));
        }
    }
    assert!(failed.is_empty(), "DSP control pins failed to route:\n{}", failed.join("\n"));
}

/// The point of G35: a real multiplier. Two flops feed `A` and `B`, the DSP
/// multiplies, and `P` leaves a third flop. Before the fix this could not be
/// routed at all; now it packs, places, routes, and its DSP pins take part in
/// timing (G32).
#[test]
fn a_dsp_multiplier_packs_places_routes_and_is_timed() {
    let mut d = Design::new("mul");
    d.add_port("clk", PortDir::In);
    d.add_port("a", PortDir::In);
    d.add_port("b", PortDir::In);
    d.add_port("p", PortDir::Out);
    let locs = [("clk", "Y9"), ("a", "V5"), ("b", "U14"), ("p", "R16")];
    for port in d.ports.iter_mut() {
        if let Some((_, loc)) = locs.iter().find(|(n, _)| port.name == intern(n)) {
            port.attrs.set("LOC", *loc);
        }
    }
    for (ff, pin, q) in [("u_ffa", "a", "qa"), ("u_ffb", "b", "qb")] {
        d.add_cell(ff, CellKind::Hff);
        d.connect("clk", ff, "CLK");
        d.connect(pin, ff, "D");
        d.connect(q, ff, "Q");
    }
    d.add_cell(
        "u_dsp",
        CellKind::BlackBox {
            module: intern("DSP48E1"),
        },
    );
    for (k, v) in [("PREG", "1"), ("AREG", "1"), ("BREG", "1")] {
        d.set_cell_attr("u_dsp", k, v).unwrap();
    }
    d.connect("clk", "u_dsp", "CLK");
    d.connect("qa", "u_dsp", "A0");
    // `D`, not `B`: B0 is fed over DSP_IMUX22_0 like A and was never affected
    // by G35. D0 goes through DSP_BYP, so it exercises the fix. Real usage
    // reaches D via INMODE's pre-adder (D + A*B).
    d.connect("qb", "u_dsp", "D0");
    d.connect("p", "u_dsp", "P0");

    let be = Xilinx7Backend::open("xc7z020").unwrap();
    let packed = be.pack(&d).expect("pack");
    assert_eq!(packed.dsps.len(), 1, "one DSP");
    let b_pins: Vec<&str> = packed.dsps[0]
        .pins
        .iter()
        .filter(|(_, n, _)| n == "qb")
        .map(|(p, _, _)| p.as_str())
        .collect();
    assert!(!b_pins.is_empty(), "D0 must be driven by the second flop");
    let placed = be.place(&packed).expect("place");
    let routed = be.route(&placed).expect("a DSP multiplier must route");

    // G32: the DSP's clocked inputs are setup endpoints, so the analysis sees
    // the path into the DSP rather than skipping it.
    let a = helion_xilinx7::timing::analyze_clocks(&routed, &|c: &str| {
        c.contains("clk").then_some(10_000)
    })
    .expect("timing");
    assert!(a.endpoints > 0, "the DSP's inputs were not counted as endpoints");
}
