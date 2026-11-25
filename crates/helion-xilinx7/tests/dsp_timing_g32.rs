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

/// Setup is that of the register a pin first reaches: its own, else M,
/// else P (X-Ray's zynq7 timings/DSP_L.sdf, slow corner). Pins are per bit
/// (`A0`, `C5`, `CARRYINSEL1`), as `pack` names them.
#[test]
fn dsp_pin_setups_match_the_sdf() {
    use helion_xilinx7::pack::PackedDsp;
    use helion_xilinx7::timing::dsp_setup_ps;
    let m = |f: fn(&mut PackedDsp)| {
        let mut d = PackedDsp { use_mult: "MULTIPLY".into(), ..Default::default() };
        f(&mut d);
        d
    };
    let s = dsp_setup_ps;
    // AREG_1_A_INPUT_DIRECT 0.362; AREG_0 ... MREG_1 1.928; AREG_0 ... MREG_0_PREG_1 3.722 (PATDET 4.114, with the D port 5.474).
    assert_eq!(s(&m(|d| { d.areg = 1; d.preg = 1 }), "A0"), Some(362));
    assert_eq!(s(&m(|d| { d.mreg = 1; d.preg = 1 }), "A24"), Some(1928));
    assert_eq!(s(&m(|d| d.preg = 1), "A3"), Some(3722));
    assert_eq!(s(&m(|d| { d.preg = 1; d.use_pattern_detect = true }), "A3"), Some(4114));
    assert_eq!(s(&m(|d| { d.preg = 1; d.use_pattern_detect = true; d.use_dport = true }), "A3"), Some(5474));
    assert_eq!(s(&m(|d| d.breg = 1), "B17"), Some(450));
    assert_eq!(s(&m(|d| d.preg = 1), "B0"), Some(3536));
    assert_eq!(s(&m(|d| { d.preg = 1; d.use_dport = true }), "D4"), Some(4951));
    assert_eq!(s(&m(|d| d.preg = 1), "INMODE2"), Some(3805));
    assert_eq!(s(&m(|d| d.creg = 1), "C5"), Some(233));
    assert_eq!(s(&m(|d| d.preg = 1), "C47"), Some(1701));
    assert_eq!(s(&m(|d| d.preg = 1), "OPMODE6"), Some(2209));
    assert_eq!(s(&m(|d| d.preg = 1), "ALUMODE0"), Some(1978));
    assert_eq!(s(&m(|d| d.preg = 1), "CARRYINSEL1"), Some(1793));
    assert_eq!(s(&m(|d| d.preg = 1), "CARRYIN"), Some(1298));
    assert_eq!(s(&m(|d| d.areg = 1), "CEA2"), Some(497));
    assert_eq!(s(&m(|d| d.preg = 1), "RSTP"), Some(347));
    // No register on the way: a combinational arc to P, not an endpoint.
    for pin in ["A0", "B0", "C0", "OPMODE0", "CEA1", "RSTP"] {
        assert_eq!(s(&m(|_| {}), pin), None, "{pin}");
    }
    assert_eq!(s(&m(|d| d.preg = 1), "D0"), None, "no D port");
    for pin in ["P0", "PCOUT3", "CARRYOUT0", "CLK"] {
        assert_eq!(s(&m(|d| { d.areg = 1; d.preg = 1 }), pin), None, "{pin}");
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
    assert_eq!(dsp_comb_ps(&helion_xilinx7::pack::PackedDsp { breg: 1, ..none.clone() }, "B0"), None);
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

/// AREG=1 and no P register: A's flop ends at the A register (0.362 ns
/// setup) and P is launched by CLK through the multiplier (4.009 ns) on
/// to the flop after it.
#[test]
fn an_input_registered_dsp_launches_p_from_its_clock() {
    let mut d = Design::new("areg_dsp");
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
    for k in ["BREG", "CREG", "DREG", "ADREG", "MREG", "PREG", "OPMODEREG", "ALUMODEREG", "INMODEREG", "CARRYINREG", "CARRYINSELREG"] {
        d.set_cell_attr("u_dsp", k, "0").unwrap();
    }
    d.set_cell_attr("u_dsp", "AREG", "1").unwrap();
    d.set_cell_attr("u_dsp", "ACASCREG", "1").unwrap();
    d.connect("clk", "u_dsp", "CLK");
    d.connect("qa", "u_dsp", "A0");
    d.connect("p", "u_dsp", "P0");
    let be = Xilinx7Backend::open("xc7z020").unwrap();
    let routed = be.route(&be.place(&be.pack(&d).expect("pack")).expect("place")).expect("route");
    let a = helion_xilinx7::timing::analyze_clocks(&routed, &|c: &str| c.contains("clk").then_some(10_000)).expect("timing");
    let path: Vec<String> = a.worst_path.iter().map(|s| format!("{} @{}", s.what, s.arrival_ps)).collect();
    assert!(a.worst_path[0].what.contains("u_dsp (DSP48E1 output)") && a.worst_path[0].arrival_ps == 4009, "P launched by CLK: {path:?}");
    assert!(a.worst_path.iter().all(|s| !s.what.contains("A0 -> P")), "A is registered, no A -> P arc: {path:?}");
}
