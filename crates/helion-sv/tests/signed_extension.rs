//! `$signed` and signed operands must sign-extend when widened.
//!
//! `assign y = $signed(a);` with 8-bit `a` and 16-bit `y` used to widen
//! exactly like an unsigned source: `y_15..y_8` were driven to constant 0.
//! For `a = -1` (0x80..0xff) that silently produced a small positive number
//! instead of all ones. The check is functional, not a cell count.

mod common;

use common::Netlist;
use helion_sv::synth_sv;

/// Evaluate `y = <rhs>` for a single 8-bit signed input `a` across a few
/// values that distinguish sign extension from zero extension.
fn widen_y(rhs: &str, signed_decl: bool) -> Vec<(u64, u64)> {
    let decl = if signed_decl {
        "input logic signed [7:0] a"
    } else {
        "input logic [7:0] a"
    };
    let src = format!(
        "module t({decl}, output logic [15:0] y);\n assign y = {rhs};\nendmodule"
    );
    let d = synth_sv(&src, "t.sv").expect("synth");
    [0x00u64, 0x01, 0x7f, 0x80, 0xff]
        .into_iter()
        .map(|a| {
            let y = Netlist::drive(&d, &[("a", a, 8)]).out("y", 16);
            (a, y)
        })
        .collect()
}

#[test]
fn dollar_signed_sign_extends_on_widening() {
    // `a` is declared unsigned here, so the cast is what makes it signed.
    for (a, y) in widen_y("$signed(a)", false) {
        let want = ((a as i8 as i64) as u16) as u64;
        assert_eq!(
            y, want,
            "$signed(a) with a=0x{a:02x} must sign-extend to 0x{want:04x}, got 0x{y:04x}"
        );
    }
}

#[test]
fn declared_signed_operand_sign_extends() {
    for (a, y) in widen_y("a", true) {
        let want = ((a as i8 as i64) as u16) as u64;
        assert_eq!(
            y, want,
            "signed a=0x{a:02x} must sign-extend to 0x{want:04x}, got 0x{y:04x}"
        );
    }
}

#[test]
fn negative_value_is_all_ones_not_zero_extended() {
    let rows = widen_y("$signed(a)", false);
    let (_, minus_one) = rows
        .into_iter()
        .find(|(a, _)| *a == 0xff)
        .expect("0xff row");
    assert_eq!(
        minus_one, 0xffff,
        "-1 must widen to all ones; zero-extension gave 0x00ff"
    );
}

#[test]
fn unsigned_operand_still_zero_extends() {
    // The regression guard's other half: an unsigned source must NOT acquire
    // sign bits, or `assign y = a;` with a=0xff would wrongly become 0xffff.
    for (a, y) in widen_y("a", false) {
        assert_eq!(
            y, a,
            "unsigned a=0x{a:02x} must zero-extend, got 0x{y:04x}"
        );
    }
}

#[test]
fn same_width_signed_assign_is_unchanged() {
    let d = synth_sv(
        r#"module t(input logic signed [7:0] a, output logic signed [7:0] y);
             assign y = a;
           endmodule"#,
        "t.sv",
    )
    .expect("synth");
    for a in [0x00u64, 0x80, 0xff] {
        let y = Netlist::drive(&d, &[("a", a, 8)]).out("y", 8);
        assert_eq!(y, a, "same-width signed copy must be the identity");
    }
}

#[test]
fn narrowing_signed_assign_truncates() {
    // Widening must not change narrowing: `y[3:0] = a[7:4]`-style assignments
    // still keep only the low bits they have always kept.
    let d = synth_sv(
        r#"module t(input logic signed [7:0] a, output logic signed [3:0] y);
             assign y = a;
           endmodule"#,
        "t.sv",
    )
    .expect("synth");
    for a in [0x0fu64, 0xf0, 0xff] {
        let y = Netlist::drive(&d, &[("a", a, 8)]).out("y", 4);
        assert_eq!(y, a & 0xf, "narrowing must truncate, not re-extend");
    }
}
