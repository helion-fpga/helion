//! Shift registers become SRL16E / SRLC32E in the 7-series flow
//! (`helion_sv::v2::srl`). Behaviour is checked against the RTL by
//! `tools/svcheck` (`tools/svcheck/tests_hard/srl_*.sv`); these tests pin the
//! netlist shape. Own test binary: it turns on process-wide switches.

use helion_ir::{resolve, CellKind, Design};

fn build(src: &str) -> Design {
    helion_sv::v2::set_hard(true);
    helion_sv::v2::dag::set_carry_arith(true);
    helion_sv::v2::synth(&[("t.sv".into(), src.into())], None, &[], vec![]).unwrap().design
}

fn count(d: &Design, m: &str) -> usize {
    d.cells.iter().filter(|c| matches!(c.kind, CellKind::BlackBox { module } if resolve(module) == m)).count()
}

fn ffs(d: &Design) -> usize {
    d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count()
}

#[test]
fn fixed_tap_with_enable_is_one_srl16e() {
    let d = build("module t(input clk, ce, din, output q); reg [9:0] sr; always @(posedge clk) if (ce) sr <= {sr[8:0], din}; assign q = sr[9]; endmodule");
    assert_eq!((count(&d, "SRL16E"), ffs(&d)), (1, 0));
    let srl = d.cells.iter().find(|c| matches!(c.kind, CellKind::BlackBox { .. })).unwrap();
    assert_eq!(d.pin_index().net_on(resolve(srl.name), "CE").map(resolve), Some("ce"));
}

#[test]
fn dynamic_tap_puts_the_address_on_a() {
    let d = build("module t(input clk, din, input [4:0] a, output q); reg [31:0] sr; always @(posedge clk) sr <= {sr[30:0], din}; assign q = sr[a]; endmodule");
    assert_eq!((count(&d, "SRLC32E"), ffs(&d)), (1, 0));
    let srl = d.cells.iter().find(|c| matches!(c.kind, CellKind::BlackBox { .. })).unwrap();
    let pins = d.pin_index();
    for i in 0..5 {
        assert_eq!(pins.net_on(resolve(srl.name), &format!("A{i}")).map(resolve), Some(format!("a_{i}").as_str()));
    }
}

#[test]
fn long_chains_cascade_through_q31() {
    let d = build("module t(input clk, din, output q); reg [69:0] sr; always @(posedge clk) sr <= {sr[68:0], din}; assign q = sr[69]; endmodule");
    assert_eq!((count(&d, "SRLC32E"), ffs(&d)), (3, 0));
    let d = build("module t(input clk, din, input [5:0] a, output q); reg [63:0] sr; always @(posedge clk) sr <= {sr[62:0], din}; assign q = sr[a]; endmodule");
    assert_eq!((count(&d, "SRLC32E"), ffs(&d)), (2, 0));
}

#[test]
fn taps_split_chains_and_short_chains_stay_flops() {
    let d = build("module t(input clk, din, output q, p); reg [7:0] sr; always @(posedge clk) sr <= {sr[6:0], din}; assign q = sr[7]; assign p = sr[3]; endmodule");
    assert_eq!((count(&d, "SRL16E"), ffs(&d)), (2, 0));
    let d = build("module t(input clk, din, output q); reg [1:0] sr; always @(posedge clk) sr <= {sr[0], din}; assign q = sr[1]; endmodule");
    assert_eq!((count(&d, "SRL16E"), ffs(&d)), (0, 2));
}

#[test]
fn reset_and_shreg_extract_no_keep_flops() {
    let d = build("module t(input clk, rst, din, output q); reg [7:0] sr; always @(posedge clk) if (rst) sr <= 0; else sr <= {sr[6:0], din}; assign q = sr[7]; endmodule");
    assert_eq!((count(&d, "SRL16E"), ffs(&d)), (0, 8));
    let d = build("module t(input clk, din, output q); (* shreg_extract = \"no\" *) reg [7:0] sr; always @(posedge clk) sr <= {sr[6:0], din}; assign q = sr[7]; endmodule");
    assert_eq!((count(&d, "SRL16E"), ffs(&d)), (0, 8));
    let d = build("module t(input clk, din, output q); (* srl_style = \"register\" *) reg [7:0] sr; always @(posedge clk) sr <= {sr[6:0], din}; assign q = sr[7]; endmodule");
    assert_eq!((count(&d, "SRL16E"), ffs(&d)), (0, 8));
}

#[test]
fn async_reg_synchronizer_keeps_flops() {
    let d = build("module t(input clk, din, output q); (* ASYNC_REG = \"TRUE\" *) reg [2:0] s; always @(posedge clk) s <= {s[1:0], din}; assign q = s[2]; endmodule");
    assert_eq!((count(&d, "SRL16E"), ffs(&d)), (0, 3));
    let d = build("module t(input clk, din, output q); reg [2:0] s; always @(posedge clk) s <= {s[1:0], din}; assign q = s[2]; endmodule");
    assert_eq!((count(&d, "SRL16E"), ffs(&d)), (1, 0));
}
