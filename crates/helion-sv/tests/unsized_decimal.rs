//! An unsized decimal literal has the 32-bit Verilog integer width, not the
//! 1 bit its internal `width: 0` marker used to imply. Reporting width 1
//! truncated every unsized literal to its LSB: `assign y = 5;` read back 1.

mod common;
use common::Netlist;
use helion_sv::synth_sv;

fn synth(src: &str) -> helion_ir::Design {
    synth_sv(src, "t.sv").expect("synth")
}

#[test]
fn unsized_decimal_assigned_to_a_bus_keeps_its_value() {
    let d = synth("module t(output logic [7:0] y); assign y = 5; endmodule");
    assert_eq!(Netlist::drive(&d, &[]).out("y", 8), 5, "y = 5");
}

#[test]
fn unsized_decimal_wider_than_a_byte_truncates_normally() {
    // 300 needs 9 bits; into 8 bits it must keep the low byte, not the low bit.
    let d = synth("module t(output logic [7:0] y); assign y = 300; endmodule");
    assert_eq!(Netlist::drive(&d, &[]).out("y", 8), 0x2c, "y = 300");
}

#[test]
fn unsized_decimal_in_a_wide_bus_sets_every_bit() {
    // 5 needs 3 bits; all of them must reach a 16-bit output.
    let d = synth("module t(output logic [15:0] y); assign y = 5; endmodule");
    assert_eq!(Netlist::drive(&d, &[]).out("y", 16), 5, "y = 5");
}

#[test]
fn unsized_decimal_still_works_in_arithmetic() {
    // The arithmetic path already used the 32-bit default; keep it working.
    let d = synth("module t(input logic [7:0] a, output logic [7:0] y); assign y = a + 1; endmodule");
    for a in [0u64, 1, 0x7f, 0xfe, 0xff] {
        let y = Netlist::drive(&d, &[("a", a, 8)]).out("y", 8);
        assert_eq!(y, (a.wrapping_add(1)) & 0xff, "a=0x{a:02x} + 1");
    }
}

#[test]
fn unsized_decimal_in_a_compare_is_not_single_bit() {
    let d = synth("module t(input logic [7:0] a, output logic y); assign y = a > 1; endmodule");
    for a in [0u64, 1, 2, 3, 0xff] {
        let y = Netlist::drive(&d, &[("a", a, 8)]).out("y", 1);
        assert_eq!(y, (a > 1) as u64, "a={a} > 1");
    }
}

#[test]
fn unsized_based_literals_take_the_32_bit_default() {
    // Width used to be the digit count, so `'h1234` was 4 bits and read 0x4.
    for (lit, want) in [
        ("'h1234", 0x1234u64),
        ("'hff", 0xff),
        ("'d5", 5),
        ("'o17", 0o17),
        ("'b1010", 0b1010),
    ] {
        let src = format!("module t(output logic [15:0] y); assign y = {lit}; endmodule");
        let d = synth(&src);
        assert_eq!(
            Netlist::drive(&d, &[]).out("y", 16),
            want,
            "{lit} must widen to its full value"
        );
    }
}

#[test]
fn sized_based_literals_still_truncate_to_their_size() {
    // `8'h1234` is an 8-bit literal: only the low byte survives.
    let d = synth("module t(output logic [15:0] y); assign y = 8'h1234; endmodule");
    assert_eq!(Netlist::drive(&d, &[]).out("y", 16), 0x34, "8'h1234");
    let d = synth("module t(output logic [15:0] y); assign y = 16'h1234; endmodule");
    assert_eq!(Netlist::drive(&d, &[]).out("y", 16), 0x1234, "16'h1234");
}
