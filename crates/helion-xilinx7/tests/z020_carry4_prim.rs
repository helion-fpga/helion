//! A directly instantiated `CARRY4` whose sum and carry-out bits drive
//! fabric (G13): the outputs leave the slice on its `AMUX`..`DMUX` pins.
//!
//! Requires the X-Ray zynq7 database (`HELION_XILINX_DB`, see `device.rs`).

use helion_backend::Backend;
use helion_ir::{CellKind, Design, PortDir};
use helion_xilinx7::backend::Xilinx7Backend;

/// `outs`: (CARRY4 pin, output port). Inputs a0..a3 drive S and DI.
fn carry_design(outs: &[(&str, &str)]) -> Design {
    let mut d = Design::new("c4");
    d.add_cell("u_c4", CellKind::Carry4);
    d.connect("0", "u_c4", "CYINIT");
    for (j, pin) in ["F22", "G22", "H22", "F21"].iter().enumerate() {
        let a = format!("a{j}");
        d.add_port(&a, PortDir::In);
        d.set_loc(&a, pin).unwrap();
        d.connect(&a, "u_c4", &format!("S{j}"));
        d.connect(&a, "u_c4", &format!("DI{j}"));
    }
    let leds = ["T22", "T21", "U22", "U21", "V22", "W22"];
    for (i, (pin, port)) in outs.iter().enumerate() {
        let n = format!("{port}_n");
        d.connect(&n, "u_c4", pin);
        d.add_port(port, PortDir::Out);
        d.set_loc(port, leds[i]).unwrap();
        let iob = format!("u_{port}");
        d.add_cell(&iob, CellKind::IobOut);
        d.connect(&n, &iob, "I");
        d.connect(port, &iob, "PAD");
    }
    d
}

fn fasm(d: &Design) -> Result<String, String> {
    let be = Xilinx7Backend::open("xc7z020").expect("xc7z020 device");
    let packed = be.pack(d)?;
    let routed = be.route(&be.place(&packed)?)?;
    Ok(be.bitstream(&routed)?.fasm.join("\n"))
}

#[test]
fn carry4_sum_and_carry_out_reach_fabric() {
    let text = fasm(&carry_design(&[("O0", "s0"), ("CO1", "c1"), ("O2", "s2"), ("CO3", "c3")])).expect("build");
    for f in ["AOUTMUX.XOR", "BOUTMUX.CY", "COUTMUX.XOR", "DOUTMUX.CY"] {
        assert!(text.contains(f), "missing {f}:\n{text}");
    }
}

/// All four sum bits and CO3 in fabric: DMUX carries O3, and CO3 leaves
/// through a one-bit tail CARRY4 above (cascade in, O0 = CI out), as Vivado
/// builds it.
#[test]
fn carry4_co3_beside_o3_leaves_through_a_tail() {
    let text = fasm(&carry_design(&[("O0", "s0"), ("O1", "s1"), ("O2", "s2"), ("O3", "s3"), ("CO3", "c3")])).expect("build");
    assert!(text.contains("DOUTMUX.XOR"), "{text}");
    assert!(!text.contains("DOUTMUX.CY"), "{text}");
    assert!(text.contains("PRECYINIT.CIN"), "the tail takes the cascade:\n{text}");
    assert_eq!(text.matches("AOUTMUX.XOR").count(), 2, "O0 of the head and of the tail:\n{text}");
}
