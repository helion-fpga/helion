//! Hard blocks from the v2 front end (`helion_sv::v2::set_hard`): memories
//! become RAMB18E1 / RAMB36E1 cells and device primitives pass through.
//! Behaviour is checked against the RTL by `tools/svcheck` (corpus
//! `tools/svcheck/tests_hard`); these tests pin the netlist shape.

use helion_ir::{resolve, CellKind, Design};

fn synth(src: &str) -> (Design, Vec<String>) {
    helion_sv::v2::set_hard(true);
    let s = helion_sv::synth_sv_v2(src, "t.sv", None).unwrap();
    (s.design, s.warnings)
}

fn boxes(d: &Design) -> Vec<(String, String)> {
    d.cells
        .iter()
        .filter_map(|c| match c.kind {
            CellKind::BlackBox { module } => Some((resolve(c.name).to_string(), resolve(module).to_string())),
            _ => None,
        })
        .collect()
}

fn attr<'a>(d: &'a Design, cell: &str, k: &str) -> Option<&'a str> {
    d.cells.iter().find(|c| resolve(c.name) == cell)?.attrs.get(k)
}

fn net(d: &Design, cell: &str, pin: &str) -> Option<String> {
    d.pin_index().net_on(cell, pin).map(|n| resolve(n).to_string())
}

#[test]
fn single_port_ram_is_one_ramb18() {
    let (d, _) = synth(
        "module sp (input clk, input we, input [9:0] a, input [7:0] di, output reg [7:0] q);
           reg [7:0] mem [0:1023];
           always @(posedge clk) begin if (we) mem[a] <= di; q <= mem[a]; end
         endmodule",
    );
    assert_eq!(boxes(&d), vec![("mem_bram0".to_string(), "RAMB18E1".to_string())]);
    assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)), "no flops: the RAM's output latch is q");
    assert_eq!(attr(&d, "mem_bram0", "READ_WIDTH_A"), Some("9"));
    assert_eq!(attr(&d, "mem_bram0", "WRITE_MODE_A"), Some("READ_FIRST"));
    // 1K x 9: word address on pins 3..12, the data straight from the ports.
    assert_eq!(net(&d, "mem_bram0", "ADDRARDADDR3").as_deref(), Some("a_0"));
    assert_eq!(net(&d, "mem_bram0", "ADDRARDADDR12").as_deref(), Some("a_9"));
    assert_eq!(net(&d, "mem_bram0", "DIADI7").as_deref(), Some("di_7"));
    assert_eq!(net(&d, "mem_bram0", "WEA0").as_deref(), Some("we"));
    assert_eq!(net(&d, "mem_bram0", "CLKARDCLK").as_deref(), Some("clk"));
    assert!(net(&d, "mem_bram0", "DOADO0").is_some());
}

#[test]
fn contents_from_initial_loop_and_readmemh() {
    let dir = std::env::temp_dir().join(format!("helion_v2_hard_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let hex = dir.join("rom.hex");
    std::fs::write(&hex, "// comment\n12 34\n@4 ab\n").unwrap();
    let (d, _) = synth(&format!(
        "module r (input clk, input [10:0] a, output [7:0] q, output [7:0] p);
           logic [7:0] rom [0:2047];
           logic [7:0] ram [0:2047];
           logic [10:0] a_q;
           initial $readmemh(\"{}\", rom);
           initial for (int i = 0; i < 2048; i++) ram[i] = 8'(i + 1);
           always_ff @(posedge clk) a_q <= a;
           assign q = rom[a_q];
           assign p = ram[a_q];
         endmodule",
        hex.display()
    ));
    let b = boxes(&d);
    assert_eq!(b.len(), 2, "{b:?}");
    // rom: bytes 0x12, 0x34 at 0 and 1, 0xab at 4; INIT_00 is LSB first.
    let r0 = attr(&d, "rom_bram0", "INIT_00").unwrap();
    assert!(r0.starts_with("256'h") && r0.ends_with("000000AB00003412"), "{r0}");
    // ram[i] = i + 1: first word 0x01, then 0x02 ...
    assert!(attr(&d, "ram_bram0", "INIT_00").unwrap().ends_with("0807060504030201"));
    // The output latch powers up showing element 0, as the RTL does.
    assert_eq!(attr(&d, "rom_bram0", "INIT_A"), Some("18'h00012"));
    assert_eq!(attr(&d, "rom_bram0", "WRITE_MODE_A"), Some("READ_FIRST"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn deep_memory_splits_into_rows_with_a_select_register() {
    let (d, _) = synth(
        "module deep (input clk, input en, input we, input [15:0] wa, input [15:0] ra, input [3:0] wd, output logic [3:0] q);
           logic [3:0] mem [0:65535];
           always_ff @(posedge clk) begin if (we) mem[wa] <= wd; if (en) q <= mem[ra]; end
         endmodule",
    );
    let b = boxes(&d);
    assert_eq!(b.len(), 8);
    assert!(b.iter().all(|x| x.1 == "RAMB36E1"));
    let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
    assert_eq!(ffs, 1, "one flop remembers which row was read");
}

#[test]
fn asynchronous_read_stays_in_flops() {
    let (d, w) = synth(
        "module a (input clk, input we, input [6:0] x, input [15:0] di, output [15:0] q);
           logic [15:0] mem [0:127];
           always_ff @(posedge clk) if (we) mem[x] <= di;
           assign q = mem[x];
         endmodule",
    );
    assert!(boxes(&d).is_empty());
    assert!(w.iter().any(|w| w.contains("built from flops") && w.contains("asynchronous")), "{w:?}");
}

#[test]
fn primitive_instances_pass_through() {
    let (d, _) = synth(
        "module m (input clk, input [29:0] a, input [17:0] b, output [47:0] p, output o);
           DSP48E1 #(.AREG(1), .OPMODE_REG(0), .USE_MULT(\"MULTIPLY\"), .MASK(48'h3fffffffffff))
             u_dsp (.CLK(clk), .A(a), .B(b), .OPMODE(7'b0000101), .ALUMODE(4'b0), .P(p), .CEA2(1'b1));
           MMCME2_BASE #(.CLKFBOUT_MULT_F(10.0), .CLKIN1_PERIOD(10.000), .CLKOUT0_DIVIDE_F(5.5), .CLKOUT1_PHASE(-90.0))
             u_mmcm (.CLKIN1(clk), .CLKFBIN(fb), .CLKFBOUT(fb), .CLKOUT0(o), .RST(1'b0), .PWRDWN(1'b0));
           wire fb;
         endmodule",
    );
    let b = boxes(&d);
    assert!(b.contains(&("u_dsp".into(), "DSP48E1".into())) && b.contains(&("u_mmcm".into(), "MMCME2_BASE".into())), "{b:?}");
    assert_eq!(attr(&d, "u_dsp", "AREG"), Some("1"));
    assert_eq!(attr(&d, "u_dsp", "USE_MULT"), Some("MULTIPLY"));
    assert_eq!(attr(&d, "u_dsp", "MASK"), Some("48'h3FFFFFFFFFFF"));
    assert_eq!(attr(&d, "u_mmcm", "CLKFBOUT_MULT_F"), Some("10.0"));
    assert_eq!(attr(&d, "u_mmcm", "CLKIN1_PERIOD"), Some("10.000"));
    assert_eq!(attr(&d, "u_mmcm", "CLKOUT1_PHASE"), Some("-90.0"));
    assert_eq!(net(&d, "u_dsp", "A29").as_deref(), Some("a_29"));
    assert_eq!(net(&d, "u_dsp", "B0").as_deref(), Some("b_0"));
    assert_eq!(net(&d, "u_dsp", "P47").as_deref(), Some("p_47"));
    assert_eq!(net(&d, "u_mmcm", "CLKFBIN"), net(&d, "u_mmcm", "CLKFBOUT"));
    assert_eq!(net(&d, "u_mmcm", "CLKOUT0").as_deref(), Some("o"));
}
