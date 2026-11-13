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
        // Read by a register that also reads another address.
        RAM.replace("q <= mem[ra];", "q <= we ? mem[wa] : mem[ra];"),
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
    // A product by a constant is built from LUTs.
    assert_eq!(macs(&synth(&src("[15:0]", "a * 16'd1234"), true)), 0);
    // A 33-bit product of 16-bit operands is one block, P extended.
    let wide = "module t(input logic [15:0] a, input logic [15:0] b, output logic [32:0] p);\n  assign p = a * b;\nendmodule\n";
    assert_eq!(macs(&synth(wide, true)), 1);
    // Zero-extended 17-bit operands are 16-bit values: that fits.
    assert_eq!(macs(&synth(&src("[15:0]", "{1'b0, a} * {1'b0, b} >> 1"), true)), 1);
    assert_eq!(macs(&synth(&src("[15:0]", "$signed(a) * $signed({1'b0, b[14:0]})"), true)), 1);
    // Wider operands: one block per 16 x 16 partial product that reaches
    // the result, unsigned, and adders. 17 x 17 -> 32: (0,0), (0,1), (1,0).
    let d = synth(&src("[16:0]", "a * b"), true);
    assert_eq!(macs(&d), 3);
    assert!(d.cells.iter().all(|c| c.attrs.get("SIGNED").is_none()));
    // Signed by unsigned: the signed operand sign-extended to 32 bits is
    // two pieces.
    assert_eq!(macs(&synth(&src("[15:0]", "$signed(a) * $signed({1'b0, b})"), true)), 2);
    // 32 x 32 -> 64: four.
    let w64 = "module t(input logic [31:0] a, input logic [31:0] b, output logic [63:0] p);\n  assign p = {32'd0, a} * {32'd0, b};\nendmodule\n";
    assert_eq!(macs(&synth(w64, true)), 4);
    assert_eq!(macs(&synth(w64, false)), 0);
}

#[test]
fn registered_products_and_addends_go_on_the_multiplier() {
    let src = |body: &str| format!("module t(input logic clk, input logic en, input logic rst, input logic [15:0] a, input logic [15:0] b, input logic [31:0] c, output logic [31:0] p);\n{body}\nendmodule\n");
    let pins = |d: &Design| {
        let pi = d.pin_index();
        let m = d.cells.iter().find(|c| matches!(c.kind, CellKind::Mac27)).unwrap();
        let name = helion_ir::resolve(m.name);
        let has = |p: &str| pi.net_on(name, p).is_some();
        (m.attrs.get("PREG") == Some("1"), has("CLK"), has("CE"), has("RST"), has("C0"))
    };
    let d = synth(&src("  always_ff @(posedge clk) p <= a * b;"), true);
    assert_eq!((macs(&d), count(&d, |k| matches!(k, CellKind::Hff))), (1, 0));
    assert_eq!(pins(&d), (true, true, false, false, false));
    let d = synth(&src("  always_ff @(posedge clk) if (rst) p <= '0; else if (en) p <= a * b + c;"), true);
    assert_eq!((macs(&d), count(&d, |k| matches!(k, CellKind::Hff))), (1, 0));
    assert_eq!(pins(&d), (true, true, true, true, true));
    let d = synth(&src("  assign p = c + a * b;"), true);
    assert_eq!((macs(&d), count(&d, |k| matches!(k, CellKind::Lut6 { .. }))), (1, 0));
    assert_eq!(pins(&d), (false, false, false, false, true));
    // Off, nothing changes: flops and LUTs.
    let d = synth(&src("  always_ff @(posedge clk) if (rst) p <= '0; else if (en) p <= a * b + c;"), false);
    assert_eq!(macs(&d), 0);
    assert_eq!(count(&d, |k| matches!(k, CellKind::Hff)), 32);
    // A reset to a value other than 0 keeps the register in flops (the
    // product still goes on a block).
    let d = synth(&src("  always_ff @(posedge clk) if (rst) p <= 32'd5; else p <= a * b;"), true);
    assert_eq!(macs(&d), 1);
    assert_eq!(count(&d, |k| matches!(k, CellKind::Hff)), 32);
}

#[test]
fn wide_deep_and_two_port_memories_take_several_block_rams() {
    let mem = |w: u32, depth: u32, two: bool| {
        let ab = depth.trailing_zeros();
        format!(
            "module t(input logic clk, input logic we, input logic [{a}:0] wa, input logic [{a}:0] ra, input logic [{a}:0] rb, input logic [{x}:0] d, output logic [{x}:0] q, output logic [{x}:0] q2);\n  logic [{x}:0] mem [0:{n}];\n  always_ff @(posedge clk) begin\n    if (we) mem[wa] <= d;\n    q <= mem[ra];\n{second}  end\nendmodule\n",
            a = ab - 1,
            x = w - 1,
            n = depth - 1,
            second = if two { "    q2 <= mem[rb];\n" } else { "" }
        )
    };
    let flops = |d: &Design| count(d, |k| matches!(k, CellKind::Hff));
    // 24 bits: two side by side.
    let d = synth(&mem(24, 256, false), true);
    assert_eq!((rams(&d), flops(&d)), (2, 0));
    // 4096 words: four deep, and two flops of registered bank select.
    let d = synth(&mem(8, 4096, false), true);
    assert_eq!((rams(&d), flops(&d)), (4, 2));
    // Two read ports: one RAM each.
    let d = synth(&mem(16, 512, true), true);
    assert_eq!((rams(&d), flops(&d)), (2, 0));
    // All of it: 2 ports x 2 banks x 2 slices.
    let d = synth(&mem(20, 2048, true), true);
    assert_eq!((rams(&d), flops(&d)), (8, 2));
}

#[test]
fn products_through_wires_go_on_the_p_register() {
    let src = |body: &str| format!("module t(input logic clk, input logic [15:0] a, input logic [15:0] b, input logic [31:0] c, output logic [31:0] p);\n  logic [31:0] m;\n  logic [31:0] n;\n  assign m = a * b;\n  assign n = m;\n{body}\nendmodule\n");
    let flops = |d: &Design| count(d, |k| matches!(k, CellKind::Hff));
    let preg = |d: &Design| d.cells.iter().filter(|c| matches!(c.kind, CellKind::Mac27) && c.attrs.get("PREG") == Some("1")).count();
    // Through one, two and three wires.
    for body in ["  always_ff @(posedge clk) p <= m;", "  always_ff @(posedge clk) p <= n;", "  logic [31:0] k;\n  assign k = n;\n  always_ff @(posedge clk) p <= k;"] {
        let d = synth(&src(body), true);
        assert_eq!((macs(&d), preg(&d), flops(&d)), (1, 1, 0), "{body}");
    }
    // Off, nothing changes.
    let d = synth(&src("  always_ff @(posedge clk) p <= m;"), false);
    assert_eq!((macs(&d), flops(&d)), (0, 32));
}

#[test]
fn memories_wider_than_64_bits_keep_every_init_bit() {
    let src = "module t(input logic clk, input logic we, input logic [7:0] wa, input logic [7:0] ra, input logic [71:0] d, output logic [71:0] q);
  logic [71:0] mem [0:255] = '{0: 72'hA5_0123456789ABCDEF, 1: 72'h80_0000000000000000, 7: 72'hFF_FFFF0000FFFF0000, 255: 72'h3C_DEADBEEFCAFEF00D, default: 72'h12_3456789ABCDEF012};
  always_ff @(posedge clk) begin
    if (we) mem[wa] <= d;
    q <= mem[ra];
  end
endmodule
";
    let word = |a: usize| -> u128 {
        match a {
            0 => 0xA5_0123456789ABCDEF,
            1 => 0x80_0000000000000000,
            7 => 0xFF_FFFF0000FFFF0000,
            255 => 0x3C_DEADBEEFCAFEF00D,
            _ => 0x12_3456789ABCDEF012,
        }
    };
    let d = synth(src, true);
    assert_eq!(rams(&d), 5);
    for s in 0..5 {
        let mut init: Vec<u64> = (0..64).map(|w| (0..4).fold(0u64, |v, k| v | (((word(4 * w + k) >> (16 * s)) & 0xffff) as u64) << (16 * k))).collect();
        while init.last() == Some(&0) {
            init.pop();
        }
        let want = init.iter().map(|w| format!("{w:x}")).collect::<Vec<_>>().join(",");
        let name = format!("mem_ram_r0_b0_s{s}");
        let c = d.cells.iter().find(|c| helion_ir::resolve(c.name) == name).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(c.attrs.get("INIT"), Some(want.as_str()), "slice {s}");
    }
}

/// Every net has at most one driver and every cell name is used once.
fn one_driver_each(d: &Design) {
    let mut cells = std::collections::HashSet::new();
    for c in &d.cells {
        assert!(cells.insert(c.name), "cell {} twice", helion_ir::resolve(c.name));
    }
    let kind: std::collections::HashMap<_, _> = d.cells.iter().map(|c| (c.name, c.kind.clone())).collect();
    for n in &d.nets {
        let drivers: Vec<String> = n
            .endpoints
            .iter()
            .filter(|e| {
                let pin = helion_ir::resolve(e.pin);
                let num = |p: &str| pin.strip_prefix(p).is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()));
                match kind.get(&e.cell) {
                    Some(CellKind::Hff) => pin == "Q",
                    Some(CellKind::Lut6 { .. }) => pin == "O",
                    Some(CellKind::Bram18) => num("DO"),
                    Some(CellKind::Mac27) => num("P"),
                    _ => false,
                }
            })
            .map(|e| format!("{}.{}", helion_ir::resolve(e.cell), helion_ir::resolve(e.pin)))
            .collect();
        assert!(drivers.len() <= 1, "net {} has drivers {drivers:?}", helion_ir::resolve(n.name));
    }
}

#[test]
fn nets_made_up_for_blocks_take_no_name_of_the_design() {
    let deep = |extra_ports: &str, extra: &str| {
        format!(
            "module t(input logic clk, input logic we, input logic t, input logic [10:0] wa, input logic [10:0] ra, input logic [7:0] d, output logic [7:0] q{extra_ports});\n  logic [7:0] mem [0:2047];\n  always_ff @(posedge clk) begin\n    if (we) mem[wa] <= d;\n    q <= mem[ra];\n{extra}  end\nendmodule\n"
        )
    };
    let cases = [
        // A register named like the bank select.
        deep(", output logic q_bank0", "    q_bank0 <= t;\n"),
        // ... powering up at 1 (its net is q_bank0_n, its cell q_bank0_reg).
        deep(", output logic q_bank0", "    q_bank0 <= t;\n").replace("output logic q_bank0);", "output logic q_bank0 = 1'b1);"),
        // A register named like a bank's data out, and a port.
        deep(", output logic q_bank1_do3, output logic q_bank0_do0", "    q_bank1_do3 <= t;\n").replace("endmodule", "  assign q_bank0_do0 = ~t;\nendmodule"),
        // A register that is not read, named like the bank select.
        deep("", "    q_bank0 <= t;\n").replace("logic [7:0] mem", "logic q_bank0;\n  logic [7:0] mem"),
    ];
    for (i, src) in cases.iter().enumerate() {
        let d = synth(src, true);
        assert_eq!(rams(&d), 2, "case {i}");
        one_driver_each(&d);
    }
    // A register named like a multiplier's product.
    let mul = "module t(input logic clk, input logic t, input logic [15:0] a, input logic [15:0] b, output logic [31:0] p, output logic u_mul0_p3);\n  assign p = a * b;\n  always_ff @(posedge clk) u_mul0_p3 <= t;\nendmodule\n";
    let d = synth(mul, true);
    assert_eq!(macs(&d), 1);
    one_driver_each(&d);
}
