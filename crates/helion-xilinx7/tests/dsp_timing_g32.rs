//! DSP48E1 pins participate in timing (HELION_GAPS.md G32).
//!
//! Before this, a DSP's `P` output was neither an arrival source nor an
//! endpoint, so any path through a DSP disappeared from the analysis and the
//! design looked faster than it is. These tests pin three things:
//!
//! 1. A clocked DSP input is a setup endpoint.
//! 2. A DSP `P` output is an arrival source, so a path out of it is timed and
//!    its delay depends on the device's pipeline depth (the SDF range is
//!    0.434..=5.470 ns for CLK -> P, so one constant would be wrong by >10x).
//! 3. A design with no output register (`PREG=0`) is charged more than a
//!    registered one — the row's guard asks for AREG=0 to be distinguishable.

use helion_backend::Backend;
use helion_ir::{intern, CellKind, Design, PortDir};
use helion_xilinx7::backend::Xilinx7Backend;

/// A DSP with the given register settings, clocked from the board oscillator.
fn dsp_design(preg: &str, mreg: &str, areg: &str) -> Design {
    let mut d = Design::new("dsp_timing");
    d.add_port("clk", PortDir::In);
    d.add_port("a", PortDir::In);
    d.add_port("p", PortDir::Out);
    for port in d.ports.iter_mut() {
        port.attrs.set("LOC", "Y9"); // only the clock pin needs a real pad
    }
    d.add_cell(
        "u_dsp",
        CellKind::BlackBox {
            module: intern("DSP48E1"),
        },
    );
    d.set_cell_attr("u_dsp", "PREG", preg).unwrap();
    d.set_cell_attr("u_dsp", "MREG", mreg).unwrap();
    d.set_cell_attr("u_dsp", "AREG", areg).unwrap();
    d.set_cell_attr("u_dsp", "BREG", areg).unwrap();
    // A/B driven by a flop so the path into the DSP has an arrival.
    d.add_cell("u_ff", CellKind::Hff);
    d.connect("clk", "u_ff", "CLK");
    d.connect("a", "u_ff", "D");
    d.connect("a_q", "u_ff", "Q");
    d.connect("clk", "u_dsp", "CLK");
    d.connect("a_q", "u_dsp", "A");
    // B is tied rather than driven: G35 was fixed (a DSP's non-A inputs now
    // route, see tests/dsp_tile_mux_routing.rs), but this test is about the
    // timing model, so it keeps the smallest design that has a clocked DSP
    // input. Routing coverage for B/C/D lives in the G35 test.
    d.set_cell_attr("u_dsp", "B", "0").ok();
    d.connect("p", "u_dsp", "P");
    d
}

/// The DSP output delay the model assigns for a register configuration.
fn cko_of(preg: u8, mreg: u8, opmodereg: u8) -> u32 {
    // Mirror `dsp_cko_ps` through a packed device so the test uses the public
    // API rather than duplicating the constants.
    let mut d = helion_xilinx7::pack::PackedDsp {
        name: "x".into(),
        opmode: 0,
        alumode: 0,
        inmode: 0,
        areg: 0,
        breg: 0,
        acascreg: 0,
        bcascreg: 0,
        creg: 0,
        dreg: 0,
        adreg: 0,
        mreg,
        preg,
        opmodereg,
        alumodereg: 0,
        inmodereg: 0,
        ..Default::default()
    };
    d.preg = preg;
    helion_xilinx7::timing::dsp_cko_ps(&d)
}

#[test]
fn dsp_output_delay_depends_on_pipeline_depth() {
    // The SDF spans 0.434..=5.470 ns for CLK -> P. Whichever exact bucket the
    // model uses, the fully registered case must be markedly faster than the
    // unregistered one; a single constant cannot express this.
    let registered = cko_of(1, 1, 1);
    let unregistered = cko_of(0, 0, 0);
    assert!(
        registered < unregistered,
        "PREG=1 ({registered} ps) must be faster than PREG=0/MREG=0 ({unregistered} ps)"
    );
    assert!(
        unregistered >= 2 * registered,
        "the SDF range is >10x across configurations; {registered} vs {unregistered} ps \
         suggests the model collapsed to one constant"
    );
}

#[test]
fn dsp_pin_setups_match_the_sdf() {
    // Slow-corner maxima from X-Ray's timings/DSP_L.sdf (zynq7).
    use helion_xilinx7::timing::dsp_setup_ps;
    assert_eq!(dsp_setup_ps("A"), Some(5474));
    assert_eq!(dsp_setup_ps("B"), Some(3929));
    assert_eq!(dsp_setup_ps("C"), Some(2093));
    assert_eq!(dsp_setup_ps("D"), Some(5342));
    assert_eq!(dsp_setup_ps("INMODE"), Some(5557));
    assert_eq!(dsp_setup_ps("OPMODE"), Some(2600));
    // Not a clocked data input.
    assert_eq!(dsp_setup_ps("P"), None);
    assert_eq!(dsp_setup_ps("CLK"), None);
}

/// What G32 owns is the *model*, so test it at the model boundary: given a DSP
/// on the clock, its clocked inputs must be classified as setup endpoints and
/// its P must be an arrival source. (G35, which used to block routing every DSP
/// input but A, is fixed; see tests/dsp_tile_mux_routing.rs for routing and
/// `a_dsp_multiplier_packs_places_routes_and_is_timed` for the end-to-end case.)
#[test]
fn dsp_pins_are_classified_for_timing() {
    // An input with a setup requirement must be recognised as an endpoint
    // candidate; one without must not be.
    use helion_xilinx7::timing::dsp_setup_ps;
    let clocked_inputs = ["A", "B", "C", "D", "INMODE", "OPMODE", "ALUMODE", "CARRYIN"];
    for pin in clocked_inputs {
        assert!(
            dsp_setup_ps(pin).is_some(),
            "{pin} is a clocked DSP input but the model gives no setup"
        );
    }
    // Outputs and the clock itself are not setup endpoints.
    for pin in ["P", "PCOUT", "CARRYOUT", "CLK"] {
        assert!(
            dsp_setup_ps(pin).is_none(),
            "{pin} is not a clocked data input but the model treats it as one"
        );
    }
}

/// CLK -> P and input -> P are the SDF's slow corner (its second triple,
/// as for the LUT and flop constants), picked by the registers before P.
#[test]
fn dsp_arcs_are_the_sdf_slow_corner() {
    use helion_xilinx7::timing::{dsp_cko_ps, dsp_comb_ps};
    let none = helion_xilinx7::pack::PackedDsp { use_mult: "MULTIPLY".into(), ..Default::default() };
    assert_eq!(cko_of(0, 1, 0), 2305, "MREG_1_PREG_0 CLK -> P");
    assert_eq!(dsp_cko_ps(&helion_xilinx7::pack::PackedDsp { areg: 1, ..none.clone() }), 4009);
    assert_eq!(dsp_cko_ps(&helion_xilinx7::pack::PackedDsp { breg: 1, ..none.clone() }), 3813);
    assert_eq!(dsp_comb_ps(&none, "A7"), Some(3841));
    assert_eq!(dsp_comb_ps(&none, "B0"), Some(3656));
    assert_eq!(dsp_comb_ps(&none, "C3"), Some(1820));
    // A registered input or product is not a combinational arc to P.
    assert_eq!(dsp_comb_ps(&helion_xilinx7::pack::PackedDsp { areg: 1, ..none.clone() }, "A0"), None);
    assert_eq!(dsp_comb_ps(&helion_xilinx7::pack::PackedDsp { mreg: 1, ..none.clone() }, "B0"), None);
    assert_eq!(dsp_comb_ps(&helion_xilinx7::pack::PackedDsp { preg: 1, ..none }, "C0"), None);
}

/// A DSP with no register (G3a stage 1's STA gap): P follows A, so the path
/// is the flop before it, the route to A, A -> P (3.841 ns) and on to the
/// flop after it. Before, P started a path at a fixed 2.234 ns.
#[test]
fn a_combinational_dsp_passes_its_input_arrival_to_p() {
    let mut d = Design::new("comb_dsp");
    d.add_port("clk", PortDir::In);
    d.add_port("a", PortDir::In);
    d.add_port("q", PortDir::Out);
    for (port, loc) in d.ports.iter_mut().zip(["Y9", "V5", "R16"]) {
        port.attrs.set("LOC", loc);
    }
    for (ff, din, q) in [("u_ffa", "a", "qa"), ("u_ffp", "p", "q")] {
        d.add_cell(ff, CellKind::Hff);
        d.connect("clk", ff, "CLK");
        d.connect(din, ff, "D");
        d.connect(q, ff, "Q");
    }
    d.add_cell("u_dsp", CellKind::BlackBox { module: intern("DSP48E1") });
    for k in ["AREG", "BREG", "CREG", "DREG", "ADREG", "MREG", "PREG", "OPMODEREG", "ALUMODEREG", "INMODEREG", "CARRYINREG", "CARRYINSELREG"] {
        d.set_cell_attr("u_dsp", k, "0").unwrap();
    }
    d.connect("qa", "u_dsp", "A0");
    d.connect("p", "u_dsp", "P0");
    let be = Xilinx7Backend::open("xc7z020").unwrap();
    let routed = be.route(&be.place(&be.pack(&d).expect("pack")).expect("place")).expect("route");
    let a = helion_xilinx7::timing::analyze_clocks(&routed, &|c: &str| c.contains("clk").then_some(10_000)).expect("timing");
    let path: Vec<&str> = a.worst_path.iter().map(|s| s.what.as_str()).collect();
    let k = a.worst_path.iter().position(|s| s.what.contains("A0 -> P")).unwrap_or_else(|| panic!("no A -> P arc in {path:?}"));
    assert!(k > 0 && a.worst_path[0].what.contains("u_ffa"), "the path must start at the flop feeding A: {path:?}");
    let (before, arc) = (&a.worst_path[k - 1], &a.worst_path[k]);
    assert_eq!(arc.arrival_ps, before.arrival_ps + arc.wire_ps + 3841, "{path:?}");
}
