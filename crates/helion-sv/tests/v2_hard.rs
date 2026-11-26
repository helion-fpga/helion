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
    // 16 rows of RAMB18 rather than 8 of RAMB36: equal cost, and the router
    // does not wire RAMB36E1 yet (G11). Checked on the board flow: a 64K x 4
    // memory with the row bits changing every cycle co-simulates clean.
    let b = boxes(&d);
    assert_eq!(b.len(), 16);
    assert!(b.iter().all(|x| x.1 == "RAMB18E1"));
    let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
    assert_eq!(ffs, 2, "the row select register");
}

#[test]
fn asynchronous_read_is_lut_ram() {
    let (d, _) = synth(
        "module a (input clk, input we, input [6:0] x, input [15:0] di, output [15:0] q);
           logic [15:0] mem [0:127];
           always_ff @(posedge clk) if (we) mem[x] <= di;
           assign q = mem[x];
         endmodule",
    );
    let b = boxes(&d);
    assert_eq!(b.len(), 16, "{b:?}");
    assert!(b.iter().all(|(_, m)| m == "RAM128X1S"), "{b:?}");
    assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)));
}

/// G1 guard: a 2-read, 1-write register file is RAM32M (ports A-C read
/// anywhere), with its contents, and no flops.
#[test]
fn register_file_is_ram32m() {
    let (d, _) = synth(
        "module rf (input clk, input we, input [4:0] wa, ra1, ra2, input [7:0] wd, output [7:0] rd1, rd2);
           logic [7:0] r [0:31];
           initial for (int i = 0; i < 32; i++) r[i] = 8'(i * 37 + 5);
           always_ff @(posedge clk) if (we) r[wa] <= wd;
           assign rd1 = r[ra1];
           assign rd2 = r[ra2];
         endmodule",
    );
    let b = boxes(&d);
    assert_eq!(b.iter().filter(|(_, m)| m == "RAM32M").count(), 3, "16 read bits / 6 per RAM32M: {b:?}");
    assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)));
    // Port A of the first cell holds bits 1:0 of rd1: entry a at INIT_A[2a+1:2a].
    let init: u64 = (0..32).map(|a| ((a * 37 + 5) as u64 & 3) << (2 * a)).sum();
    assert_eq!(attr(&d, &b[0].0, "INIT_A"), Some(format!("64'h{init:016X}").as_str()));
    assert_eq!(net(&d, &b[0].0, "ADDRD0").as_deref(), Some("wa_0"));
}

/// G1 guard: the cell kind follows depth and ports, as Vivado picks it.
#[test]
fn lut_ram_cell_by_depth() {
    let one = |depth: u32, ports: &str, reads: &str| {
        let a = 32 - (depth - 1).leading_zeros();
        let (d, _) = synth(&format!(
            "module m (input clk, input we, input [{h}:0] wa, ra, input di, output qw, qr);
               logic mem [0:{top}];
               initial for (int i = 0; i < {depth}; i++) mem[i] = 1'(i >> 2);
               always_ff @(posedge clk) if (we) mem[wa] <= di;
               assign qw = mem[wa];
               assign qr = {reads};
             endmodule",
            h = a - 1,
            top = depth - 1
        ));
        let b = boxes(&d);
        assert_eq!(b.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), [ports], "depth {depth}");
        b
    };
    let b = one(64, "RAM64X1S", "mem[wa]");
    let init: u64 = (0..64).map(|i| ((i >> 2) as u64 & 1) << i).sum();
    let d = synth("module m(input clk, input we, input [5:0] wa, input di, output q); logic mem [0:63]; initial for (int i = 0; i < 64; i++) mem[i] = 1'(i >> 2); always_ff @(posedge clk) if (we) mem[wa] <= di; assign q = mem[wa]; endmodule").0;
    assert_eq!(attr(&d, &b[0].0, "INIT"), Some(format!("64'h{init:016X}").as_str()));
    one(128, "RAM128X1D", "mem[ra]");
    one(256, "RAM256X1S", "mem[wa]");
}

#[test]
fn two_write_ports_stay_in_flops() {
    let (d, w) = synth(
        "module a (input clk, input we, input [5:0] x, y, input [15:0] di, output [15:0] q);
           logic [15:0] mem [0:63];
           always_ff @(posedge clk) if (we) begin mem[x] <= di; mem[y] <= ~di; end
           assign q = mem[x ^ y];
         endmodule",
    );
    assert!(boxes(&d).is_empty());
    assert!(w.iter().any(|w| w.contains("built from flops") && w.contains("LUT RAM needs one write port")), "{w:?}");
}

/// G1 guard: PicoRV32's register file is 11 RAM32M, not ~1,000 flops
/// (Vivado 2017.3: 829 LUTs + 48 LUT RAM, 488 flops).
#[test]
fn picorv32_register_file_is_lut_ram() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let files = ["examples/zedboard_pico_soc.sv", "third_party/picorv32/picorv32.v"];
    let sources: Vec<(String, String)> = files.iter().map(|f| (f.to_string(), std::fs::read_to_string(root.join(f)).expect(f))).collect();
    helion_sv::v2::set_hard(true);
    helion_sv::v2::dag::set_carry_arith(true);
    let s = helion_sv::v2::synth(&sources, Some("zedboard_pico_soc"), &[], vec![]).expect("pico");
    let rams = boxes(&s.design).iter().filter(|(_, m)| m == "RAM32M").count();
    assert_eq!(rams, 11);
    assert!(s.ffs <= 600, "{} flops", s.ffs);
    assert!(s.luts + 4 * rams <= 1600, "{} LUTs + {} LUT RAM LUTs", s.luts, 4 * rams);
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

#[test]
fn registered_primitive_passes_through() {
    let src = "module z (output [1:0] q);
           wire c; wire [3:0] f;
           ZPRIM u_z (.FCLK(f), .ACLK(c), .Q(q));
           assign c = f[0];
         endmodule";
    helion_sv::v2::set_hard(true);
    let err = helion_sv::synth_sv_v2(src, "t.sv", None).err().unwrap_or_default();
    assert!(err.contains("ZPRIM"), "an unknown module is an error until registered: {err}");
    helion_sv::v2::mem::register_prim("ZPRIM", &[("ACLK".into(), true, 1), ("TIE".into(), true, 2), ("FCLK".into(), false, 4), ("Q".into(), false, 2)]);
    let (d, _) = synth(src);
    assert_eq!(boxes(&d), vec![("u_z".to_string(), "ZPRIM".to_string())]);
    assert_eq!(net(&d, "u_z", "FCLK0"), net(&d, "u_z", "ACLK"));
    assert_eq!(net(&d, "u_z", "Q1").as_deref(), Some("q_1"));
    assert!(net(&d, "u_z", "TIE0").is_none(), "an unconnected input gets no pin");
    let bad = helion_sv::synth_sv_v2("module y; ZPRIM u (.NOPE(1'b0)); endmodule", "t.sv", None).err().unwrap_or_default();
    assert!(bad.contains("has no port 'NOPE'"), "{bad}");
}
