//! Independent spot-checks of the 12-item frontend audit, written after the
//! fact and from the outside: each test drives the emitted netlist and asserts
//! the function, so it would fail against the *old* behavior. Deliberately not
//! a copy of the audited agent's tests -- these are the claims most worth
//! re-deriving, because each one was a silent wrong-value bug.

mod common;
use common::Netlist;
use helion_sv::synth_sv;

fn synth(src: &str) -> helion_ir::Design {
    synth_sv(src, "t.sv").expect("synth")
}

/// Bug 3, the `helion-ir` half: `absorb_lut_inverters` used to remove a LUT
/// endpoint without re-driving it, and an unconnected LUT input reads 0, so
/// `a & (b == 4)` silently became `a & b[2]`. Exhaustive over all 16 inputs.
#[test]
fn equality_against_a_constant_is_not_a_single_bit_test() {
    let d = synth(
        "module t(input logic a, input logic [2:0] b, output logic y); \
         assign y = a & (b == 3'd4); endmodule",
    );
    let mut wrong = Vec::new();
    for b in 0u64..8 {
        for a in 0u64..2 {
            let got = Netlist::drive(&d, &[("a", a, 1), ("b", b, 3)]).out("y", 1);
            let want = (a == 1 && b == 4) as u64;
            if got != want {
                wrong.push((a, b, want, got));
            }
        }
    }
    assert!(wrong.is_empty(), "a & (b==4) mismatches: {wrong:?}");
}

/// Bug 3, the `helion-sv` half: a variable shift amount must build a barrel.
#[test]
fn variable_shifts_by_a_real_signal() {
    // Shifts large enough to expose the old "SHL wrong for shift >= 5" bug.
    let d = synth(
        "module t(input logic [7:0] a, input logic [3:0] s, output logic [7:0] y); \
         assign y = a << s; endmodule",
    );
    for a in [0u64, 1, 0x0f, 0xff] {
        for s in 0u64..9 {
            let got = Netlist::drive(&d, &[("a", a, 8), ("s", s, 4)]).out("y", 8);
            let want = (a << s) & 0xff;
            assert_eq!(got, want, "a=0x{a:02x} << {s}");
        }
    }
    let d = synth(
        "module t(input logic [7:0] a, input logic [3:0] s, output logic [7:0] y); \
         assign y = a >> s; endmodule",
    );
    for a in [0xffu64, 0x80, 0x0f, 1] {
        for s in 0u64..9 {
            let got = Netlist::drive(&d, &[("a", a, 8), ("s", s, 4)]).out("y", 8);
            assert_eq!(got, (a >> s) & 0xff, "a=0x{a:02x} >> {s}");
        }
    }
}

/// Bug 5: an unsized decimal is a 32-bit value, not its low bit.
#[test]
fn unsized_decimals_keep_their_value() {
    let d = synth(
        "module t(input logic [9:0] x, output logic [9:0] y); \
         assign y = x + 300; endmodule",
    );
    for x in [0u64, 1, 700, 1023] {
        let got = Netlist::drive(&d, &[("x", x, 10)]).out("y", 10);
        assert_eq!(got, (x + 300) & 0x3ff, "x={x} + 300");
    }
    // `assign y = 5;` used to read 1, because the literal was sized to its
    // single decimal digit.
    let d = synth("module t(output logic [3:0] y); assign y = 5; endmodule");
    assert_eq!(Netlist::drive(&d, &[]).out("y", 4), 5);
    let d = synth("module t(output logic [9:0] y); assign y = 300; endmodule");
    assert_eq!(Netlist::drive(&d, &[]).out("y", 10), 300);
}

/// Bug 2: `$signed` must sign-extend, so a negative operand stays negative.
#[test]
fn signed_operands_sign_extend() {
    let d = synth(
        "module t(input logic [3:0] a, output logic [7:0] y); \
         assign y = $signed(a); endmodule",
    );
    for a in 0u64..16 {
        let got = Netlist::drive(&d, &[("a", a, 4)]).out("y", 8);
        // Sign-extend the 4-bit value to 8 bits.
        let want = if a & 0x8 != 0 { a | 0xf0 } else { a };
        assert_eq!(got, want, "signed(a) for a={a}");
    }
}

/// Bug 6: a sized signed literal carries its sign.
#[test]
fn sized_signed_literals_are_not_dropped() {
    let d = synth(
        "module t(input logic [7:0] a, output logic [7:0] y); \
         assign y = a + 8'sd1; endmodule",
    );
    for a in [0u64, 1, 200, 255] {
        let got = Netlist::drive(&d, &[("a", a, 8)]).out("y", 8);
        assert_eq!(got, (a + 1) & 0xff, "a={a} + 8'sd1");
    }
}

/// Bug 4: a wide compare against a constant must lower, not vanish to 0.
#[test]
fn wide_constant_compare_lowers() {
    let d = synth(
        "module t(input logic [15:0] a, output logic y); \
         assign y = a > 16'h1234; endmodule",
    );
    for a in [0u64, 0x1233, 0x1234, 0x1235, 0xffff] {
        let got = Netlist::drive(&d, &[("a", a, 16)]).out("y", 1);
        assert_eq!(got, (a > 0x1234) as u64, "a=0x{a:04x} > 0x1234");
    }
}
