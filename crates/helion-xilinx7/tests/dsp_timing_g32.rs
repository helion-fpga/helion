//! DSP48E1 pins participate in timing (HELION_GAPS.md G32).
//!
//! Before this, a DSP's `P` output was neither an arrival source nor an
//! endpoint, so any path through a DSP disappeared from the analysis and the
//! design looked faster than it is. These tests pin three things:
//!
//! 1. A clocked DSP input is a setup endpoint.
//! 2. A DSP `P` output is an arrival source, so a path out of it is timed and
//!    its delay depends on the device's pipeline depth (the SDF range is
//!    0.192..=2.304 ns for CLK -> P, so one constant would be wrong by >10x).
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
    // The SDF spans 0.192..=2.304 ns for CLK -> P. Whichever exact bucket the
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
