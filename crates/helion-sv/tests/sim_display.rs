//! A `$display` (or `$write`) anywhere in a module condemned the WHOLE module:
//! `synth_sub` then cleared every cell, so a design with real `assign` logic
//! plus one debug print synthesised to nothing. The sim-only verdict must
//! apply to the module only when the construct is woven into the block that
//! would otherwise become its logic (a vendor simulation model).

mod common;
use common::Netlist;
use helion_sv::synth_sv;

fn synth(src: &str) -> helion_ir::Design {
    synth_sv(src, "t.sv").expect("synth")
}

#[test]
fn display_in_its_own_initial_keeps_the_assign_logic() {
    // Before: 0 cells and y read 0; the whole module was wiped.
    let d = synth(
        r#"module t(input logic [7:0] a, output logic [7:0] y);
             assign y = a + 8'h10;
             initial $display("hi");
           endmodule"#,
    );
    assert_ne!(d.attrs.get("SIM_ONLY"), Some("1"), "logic must survive");
    assert_ne!(d.attrs.get("NO_BODY"), Some("1"));
    let y = Netlist::drive(&d, &[("a", 1, 8)]).out("y", 8);
    assert_eq!(y, 0x11, "the assign must still be synthesised");
}

#[test]
fn display_after_the_logic_also_keeps_it() {
    let d = synth(
        r#"module t(input logic [7:0] a, output logic [7:0] y);
             initial $display("hi");
             assign y = a + 8'h10;
           endmodule"#,
    );
    let y = Netlist::drive(&d, &[("a", 2, 8)]).out("y", 8);
    assert_eq!(y, 0x12, "declaration order must not matter");
}

#[test]
fn display_does_not_invent_logic_of_its_own() {
    let d = synth(
        r#"module t(input logic a, output logic y);
             assign y = a;
             initial $display("hi");
           endmodule"#,
    );
    // Exactly the passthrough: no LUT invented for the print.
    let luts = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, helion_ir::CellKind::Lut6 { .. }))
        .count();
    assert_eq!(luts, 0, "a $display is not logic");
}

#[test]
fn vendor_sim_model_with_display_inside_always_is_still_refused() {
    // Here the `=== 1'bx` compare and `$display` are inside the always block
    // that would otherwise become the counter, so no gates may be invented.
    let d = synth(
        r#"
module sim_n_cntr(clk, reset, cout, modulus);
  input clk; input reset; input [31:0] modulus; output cout;
  integer count; reg tmp_cout;
  initial count = 1;
  always @(reset or clk) begin
    if (reset) count = 1;
    else if (clk === 1'bx) $display("Warning : X");
    else if (count < modulus) count = count + 1;
    else begin count = 1; tmp_cout = ~tmp_cout; end
  end
  assign cout = tmp_cout;
endmodule
"#,
    );
    assert_eq!(d.attrs.get("SIM_ONLY"), Some("1"), "still a sim model");
    assert_eq!(d.attrs.get("NO_BODY"), Some("1"));
    assert!(d.cells.is_empty(), "no gates may be invented: {:?}", d.cells);
}
