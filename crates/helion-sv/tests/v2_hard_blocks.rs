//! Block RAM and multiplier inference in the v2 front end: off by
//! default, and with it on only for the shapes a Helion Bram18 / Mac27
//! implement exactly (everything else stays LUTs and flops).

use helion_ir::{CellKind, Design};

fn synth(src: &str, hard: bool) -> Design {
    helion_sv::v2::set_hard_blocks(Some(hard));
    let d = helion_sv::synth_sv_v2(src, "t.sv", None).expect("synth").design;
    helion_sv::v2::set_hard_blocks(None);
    d
}

fn count(d: &Design, k: fn(&CellKind) -> bool) -> usize {
    d.cells.iter().filter(|c| k(&c.kind)).count()
}

fn rams(d: &Design) -> usize {
    count(d, |k| matches!(k, CellKind::Bram18))
}

fn macs(d: &Design) -> usize {
    count(d, |k| matches!(k, CellKind::Mac27))
}

const RAM: &str = r#"
module t(input logic clk, input logic we, input logic [7:0] wa, input logic [7:0] ra, input logic [9:0] d, output logic [9:0] q);
  logic [9:0] mem [0:255] = '{0: 10'h3ff, 2: 10'h001, default: 10'h0};
  always_ff @(posedge clk) begin
    if (we) mem[wa] <= d;
    q <= mem[ra];
  end
endmodule
"#;

#[test]
fn ram_becomes_a_block_ram_only_when_asked() {
    assert_eq!(rams(&synth(RAM, false)), 0);
    let d = synth(RAM, true);
    assert_eq!(rams(&d), 1);
    assert_eq!(count(&d, |k| matches!(k, CellKind::Hff)), 0);
    let ram = d.cells.iter().find(|c| matches!(c.kind, CellKind::Bram18)).unwrap();
    // Declared [0:255]: word 0 is 0x3ff, word 2 is 1.
    assert_eq!(ram.attrs.get("INIT"), Some("1000003ff"));
    let pins = d.pin_index();
    let name = helion_ir::resolve(ram.name);
    for p in ["WADDR7", "RADDR0", "DI9", "WE", "CLK", "DO9"] {
        assert!(pins.net_on(name, p).is_some(), "{p}");
    }
    // An always-enabled read leaves RE unconnected (it reads 1).
    assert!(pins.net_on(name, "RE").is_none() && pins.net_on(name, "WADDR8").is_none());
}

#[test]
fn memories_a_block_ram_cannot_hold_stay_flops() {
    let cases = [
        // Read combinationally as well.
        RAM.replace("output logic [9:0] q);", "output logic [9:0] q, output logic [9:0] c);\n  assign c = mem[wa];"),
        // Two write addresses.
        RAM.replace("if (we) mem[wa] <= d;", "if (we) mem[wa] <= d; else mem[ra] <= d;"),
        // Not a power of two deep.
        RAM.replace("[0:255]", "[0:199]").replace(" = '{0: 10'h3ff, 2: 10'h001, default: 10'h0}", ""),
        // Wider than 16 bits.
        RAM.replace("[9:0] d", "[16:0] d").replace("[9:0] q", "[16:0] q").replace("logic [9:0] mem", "logic [16:0] mem").replace(" = '{0: 10'h3ff, 2: 10'h001, default: 10'h0}", ""),
        // The read register has a reset.
        RAM.replace("q <= mem[ra];", "q <= we ? 10'd0 : mem[ra];"),
        // Read on the falling edge.
        RAM.replace("always_ff @(posedge clk) begin", "always_ff @(negedge clk) begin"),
    ];
    for (i, src) in cases.iter().enumerate() {
        let d = synth(src, true);
        assert_eq!(rams(&d), 0, "case {i}");
        assert!(count(&d, |k| matches!(k, CellKind::Hff)) > 0, "case {i}");
    }
}

#[test]
fn products_become_multipliers_only_when_they_fit() {
    let src = |decl: &str, expr: &str| format!("module t(input logic {decl} a, input logic {decl} b, output logic [31:0] p);\n  assign p = {expr};\nendmodule\n");
    let unsigned = src("[15:0]", "a * b");
    assert_eq!(macs(&synth(&unsigned, false)), 0);
    let d = synth(&unsigned, true);
    assert_eq!(macs(&d), 1);
    assert!(d.cells.iter().all(|c| c.attrs.get("SIGNED").is_none()));
    let d = synth(&src("signed [15:0]", "a * b"), true);
    assert_eq!(macs(&d), 1);
    assert!(d.cells.iter().any(|c| c.attrs.get("SIGNED") == Some("1")));
    // 17-bit operands, a 33-bit product, a product by a constant and a
    // signed-by-unsigned product are built from LUTs.
    assert_eq!(macs(&synth(&src("[16:0]", "a * b"), true)), 0);
    let wide = "module t(input logic [15:0] a, input logic [15:0] b, output logic [32:0] p);\n  assign p = a * b;\nendmodule\n";
    assert_eq!(macs(&synth(wide, true)), 0);
    // Zero-extended 17-bit operands are 16-bit values: that fits.
    assert_eq!(macs(&synth(&src("[15:0]", "{1'b0, a} * {1'b0, b} >> 1"), true)), 1);
    assert_eq!(macs(&synth(&src("[15:0]", "a * 16'd1234"), true)), 0);
    assert_eq!(macs(&synth(&src("[15:0]", "$signed(a) * $signed({1'b0, b[14:0]})"), true)), 1);
    assert_eq!(macs(&synth(&src("[15:0]", "$signed(a) * $signed({1'b0, b})"), true)), 0);
}
