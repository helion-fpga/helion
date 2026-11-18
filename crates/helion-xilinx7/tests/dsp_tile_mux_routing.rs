//! DSP48E1 input-pin reachability (HELION_GAPS.md G35).
//!
//! Measured 2026-11-17 against X-Ray zynq7 (`tile_type_DSP_R.json`) and
//! Helion's router on xc7z020. The row is real, and it is wider than the row
//! states: it is not only the `D` bus. Every DSP input except `A` is
//! unreachable from the fabric.
//!
//! In the tile type, each input pin's non-constant sources are:
//!
//!   A0       <- DSP_IMUX23_0            (routes: OK)
//!   CEAD     <- DSP_BYP0_1
//!   CED      <- DSP_FAN0_1
//!   RSTD     <- DSP_BYP4_1
//!   INMODE0  <- DSP_BYP6_3
//!   OPMODE6  <- DSP_FAN7_2
//!   ALUMODE2 <- DSP_FAN0_0
//!   D0       <- DSP_BYP7_0
//!
//! and **no pip anywhere in the tile type takes a `DSP_BYP*` or `DSP_FAN*`
//! wire as a destination** (40 `DSP_BYP` and 38 `DSP_FAN` pips exist, all of
//! them leaving those wires; none feeds them). The `_4` variants are stitched
//! to `INT_INTERFACE_*` by `tileconn.json`, but the `_0.._3` variants have no
//! link from the `_4` variant, so they dangle.
//!
//! Consequence: a packed DSP can only be wired up on its `A`, `CLK` and `P`
//! pins. That is why `pack` ties everything else off (`dsp::has_tile_mux`), and
//! why a DSP that genuinely needs `D`, `B`, `C` or a control pin cannot route
//! at all.
//!
//! Fixing this is a loader change: synthesise `DSP_BYP<k>_<j> -> DSP_BYP<k>_4`
//! (and likewise `DSP_FAN`, `DSP_IMUX`, `DSP_CTRL`) for `j` in 0..3 from the
//! existing `INT_INTERFACE` stitch. It is not a router change.
//!
//! These tests pin the current behaviour so a fix is noticed: when the
//! unroutable case starts routing, the tests fail and should be flipped.

use helion_backend::Backend;
use helion_ir::{intern, CellKind, Design, PortDir};
use helion_xilinx7::backend::Xilinx7Backend;

/// Pack, place and route a DSP with `pin` driven by a top-level input.
fn routes_with(pin: &str) -> Result<(), String> {
    let mut d = Design::new("g35");
    d.add_port("clk", PortDir::In);
    d.add_port("i", PortDir::In);
    d.add_port("p", PortDir::Out);
    let locs = [("clk", "Y9"), ("i", "V5"), ("p", "U14")];
    for port in d.ports.iter_mut() {
        if let Some((_, loc)) = locs.iter().find(|(n, _)| port.name == intern(n)) {
            port.attrs.set("LOC", *loc);
        }
    }
    d.add_cell(
        "u_dsp",
        CellKind::BlackBox {
            module: intern("DSP48E1"),
        },
    );
    d.set_cell_attr("u_dsp", "PREG", "1").unwrap();
    d.connect("clk", "u_dsp", "CLK");
    d.connect("i", "u_dsp", pin);
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

/// `A` is fed over `DSP_IMUX`, which the router reaches. This is the only DSP
/// input that works, and it anchors the rest of these tests.
#[test]
fn dsp_a_pin_routes() {
    routes_with("A0").unwrap_or_else(|e| panic!("A0 must route: {e}"));
}

/// The row's subject: `D` does not route. Every `D` bit has a `DSP_BYP` as its
/// only non-constant source, and no pip feeds any `DSP_BYP` wire.
#[test]
fn dsp_d_pins_are_unroutable() {
    let mut routed = Vec::new();
    for pin in ["D0", "D4", "D8", "D12", "D16", "D20"] {
        if routes_with(pin).is_ok() {
            routed.push(pin);
        }
    }
    assert!(
        routed.is_empty(),
        "{routed:?} now route: G35 has been fixed for D — update HELION_GAPS.md and flip these tests"
    );
}

/// The broader finding: the control pins are in the same state, so a DSP
/// needing `CEAD`/`CED`/`RSTD`/`INMODE`/`OPMODE`/`ALUMODE` cannot be wired up
/// either. This is what makes G35 more than a `D`-bus problem.
#[test]
fn dsp_control_pins_are_unroutable() {
    let mut routed = Vec::new();
    for pin in ["CEAD", "CED", "RSTD", "INMODE0", "OPMODE6", "ALUMODE2", "CARRYINSEL2"] {
        if routes_with(pin).is_ok() {
            routed.push(pin);
        }
    }
    assert!(
        routed.is_empty(),
        "{routed:?} now route: G35 has been fixed for control pins — update HELION_GAPS.md and flip these tests"
    );
}
