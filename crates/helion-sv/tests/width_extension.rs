//! Context-determined assignment width.
//!
//! Verilog extends an assignment's RHS to the LHS width, it does not truncate
//! the assignment to the RHS width. Getting this wrong left output bits with
//! no driver, and those bits were then silently removed from the design.

fn iobs(src: &str) -> usize {
    let d = helion_sv::synth_sv(src, "t.sv").expect("synth");
    d.cells
        .iter()
        .filter(|c| matches!(c.kind, helion_ir::CellKind::IobOut))
        .count()
}

#[test]
fn wider_output_is_zero_extended() {
    // `assign y = a;` with 4-bit `a` and 16-bit `y` used to drive only 4 bits,
    // leaving y_4..y_15 undriven; iob_trim then deleted 12 of 16 outputs.
    assert_eq!(
        iobs("module t1(input logic [3:0] a, output logic [15:0] y); assign y = a; endmodule"),
        16
    );
}

#[test]
fn short_constant_fills_the_whole_output() {
    assert_eq!(
        iobs("module t2(output logic [15:0] y); assign y = 4'b1; endmodule"),
        16
    );
}

#[test]
fn fill_literal_is_all_ones() {
    // `'1` is a fill literal, not a 1-bit value.
    assert_eq!(
        iobs("module t3(output logic [15:0] y); assign y = '1; endmodule"),
        16
    );
}

#[test]
fn narrower_output_still_truncates() {
    assert_eq!(
        iobs("module t4(input logic [7:0] a, output logic [3:0] y); assign y = a; endmodule"),
        4
    );
}

#[test]
fn equal_width_is_unchanged() {
    assert_eq!(
        iobs("module t5(input logic [15:0] a, output logic [15:0] y); assign y = a; endmodule"),
        16
    );
}
