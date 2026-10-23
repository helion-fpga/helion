//! Variable (non-constant) shift amounts must lower, or refuse loudly.
//!
//! `y = a << b` with a signal `b` used to emit no logic for the affected
//! bits, so `y` was silently undriven. These tests check the shifted *value*
//! across the shift range, which is what actually matters.

mod common;

use common::Netlist;
use helion_sv::synth_sv;

#[test]
fn variable_left_shift_is_correct() {
    let d = synth_sv(
        r#"module t(input logic [7:0] a, input logic [2:0] b, output logic [7:0] y);
             assign y = a << b;
           endmodule"#,
        "t.sv",
    )
    .expect("synth");

    for a in [0x00u64, 0x01, 0x12, 0x80, 0xff] {
        for b in 0..8u64 {
            let y = Netlist::drive(&d, &[("a", a, 8), ("b", b, 3)]).out("y", 8);
            let want = ((a as u16) << b) as u64 & 0xff;
            assert_eq!(
                y, want,
                "a=0x{a:02x} << b={b} must be 0x{want:02x}, got 0x{y:02x}"
            );
        }
    }
}

#[test]
fn variable_right_shift_is_correct() {
    let d = synth_sv(
        r#"module t(input logic [7:0] a, input logic [2:0] b, output logic [7:0] y);
             assign y = a >> b;
           endmodule"#,
        "t.sv",
    )
    .expect("synth");

    for a in [0x00u64, 0x01, 0x12, 0x80, 0xff] {
        for b in 0..8u64 {
            let y = Netlist::drive(&d, &[("a", a, 8), ("b", b, 3)]).out("y", 8);
            let want = a >> b;
            assert_eq!(
                y, want,
                "a=0x{a:02x} >> b={b} must be 0x{want:02x}, got 0x{y:02x}"
            );
        }
    }
}

#[test]
fn zero_shift_amount_is_identity() {
    let d = synth_sv(
        r#"module t(input logic [7:0] a, input logic [2:0] b, output logic [7:0] y);
             assign y = a << b;
           endmodule"#,
        "t.sv",
    )
    .expect("synth");
    for a in [0x00u64, 0x5a, 0xff] {
        let y = Netlist::drive(&d, &[("a", a, 8), ("b", 0, 3)]).out("y", 8);
        assert_eq!(y, a, "shifting by zero must return the input unchanged");
    }
}
