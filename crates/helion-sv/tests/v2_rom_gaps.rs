//! Guards for the ROM gaps found on a two-layer image classifier
//! (HELION_GAPS G95, G97, G98, G11): ROM contents must reach the netlist,
//! big pattern initializers must stay fast, and ROM shapes must stay on
//! primitives the back end can route.

use helion_ir::{resolve, CellKind, Design};

fn synth(src: &str) -> Result<(Design, Vec<String>), String> {
    helion_sv::v2::set_hard(true);
    helion_sv::synth_sv_v2(src, "t.sv", None).map(|s| (s.design, s.warnings))
}

fn boxes(d: &Design) -> Vec<String> {
    d.cells
        .iter()
        .filter_map(|c| match c.kind {
            CellKind::BlackBox { module } => Some(resolve(module).to_string()),
            _ => None,
        })
        .collect()
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("helion_rom_gaps_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// G95: `$readmemh(FILE_NAME, mem)` with a string parameter loads the file.
#[test]
fn g95_readmemh_takes_a_string_parameter() {
    let dir = tmp("g95");
    let f = dir.join("w.mem");
    std::fs::write(&f, (0..2048).map(|i| format!("{:02x}\n", (i * 7) & 255)).collect::<String>()).unwrap();
    let (d, _) = synth(&format!(
        "module r #(parameter FILE_NAME = \"{}\") (input clk, input [10:0] a, output logic [7:0] q);
           logic [7:0] rom [0:2047];
           initial $readmemh(FILE_NAME, rom);
           always_ff @(posedge clk) q <= rom[a];
         endmodule",
        f.display()
    ))
    .unwrap();
    let init = d.cells.iter().find(|c| resolve(c.name) == "rom_bram0").and_then(|c| c.attrs.get("INIT_00")).unwrap().to_string();
    // bytes 0x00, 0x07, 0x0e, 0x15 ... LSB first
    assert!(init.ends_with("312A231C150E0700"), "{init}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// G95: a `$readmem` that cannot load is an error, not a warning with an
/// all-zero memory.
#[test]
fn g95_failed_readmem_is_an_error() {
    let e = synth(
        "module r (input clk, input [10:0] a, output logic [7:0] q);
           logic [7:0] rom [0:2047];
           initial $readmemh(\"/nonexistent/helion_no_such.mem\", rom);
           always_ff @(posedge clk) q <= rom[a];
         endmodule",
    )
    .err()
    .expect("a missing $readmemh file must stop the build");
    assert!(e.contains("$readmem"), "{e}");
}

/// G98: a 10 x 32 ROM (too small for block RAM) keeps its `$readmemh`
/// contents instead of reading 0.
#[test]
fn g98_small_readmemh_rom_keeps_its_contents() {
    let dir = tmp("g98");
    let f = dir.join("b2.mem");
    std::fs::write(&f, (0..10).map(|i| format!("{:08x}\n", 0x1000_0001u32 * (i + 1))).collect::<String>()).unwrap();
    let (d, w) = synth(&format!(
        "module r (input clk, input [3:0] a, output logic [31:0] q);
           logic [31:0] b2_mem [0:9];
           initial $readmemh(\"{}\", b2_mem);
           always_ff @(posedge clk) q <= b2_mem[a];
         endmodule",
        f.display()
    ))
    .unwrap();
    assert!(!w.iter().any(|x| x.contains("never driven")), "{w:?}");
    let luts = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count();
    assert!(luts > 0, "the ROM is logic, not constant 0");
    let _ = std::fs::remove_dir_all(&dir);
}

/// G97: a 50,176-entry `'{...}` initializer (what the VHDL front end emits
/// for a weight ROM) elaborates in linear time. Merging constants
/// pairwise took 9 s and 3 GB at 16,384 entries.
#[test]
fn g97_big_pattern_initializer_is_fast() {
    let items: Vec<String> = (0..50176).map(|i| format!("8'h{:02x}", (i * 7) & 255)).collect();
    let src = format!(
        "module r (input clk, input [15:0] a, output logic [7:0] q);
           logic [7:0] rom [0:50175] = '{{{}}};
           always_ff @(posedge clk) if (a < 50176) q <= rom[a];
         endmodule",
        items.join(", ")
    );
    let t = std::time::Instant::now();
    let (d, _) = synth(&src).unwrap();
    assert!(t.elapsed().as_secs() < 20, "took {:?}", t.elapsed());
    assert_eq!(boxes(&d).iter().filter(|m| *m == "RAMB18E1").count(), 25);
}

/// A ROM read whose address goes through a wire (`assign a10 = a_q[9:0]`)
/// is block RAM, like one read through the register itself.
#[test]
fn rom_read_through_a_wire_address_is_block_ram() {
    let items: Vec<String> = (0..640).map(|i| format!("8'h{:02x}", (i * 5) & 255)).collect();
    let (d, w) = synth(&format!(
        "module r (input clk, input inc, output logic [7:0] q);
           logic [15:0] a = 0;
           logic [7:0] rom [0:639] = '{{{}}};
           wire [9:0] a10 = a[9:0];
           wire [7:0] t = rom[a10];
           always_ff @(posedge clk) a <= inc ? a + 1 : a;
           always_ff @(posedge clk) q <= (a10 < 640) ? t : q;
         endmodule",
        items.join(", ")
    ))
    .unwrap();
    assert_eq!(boxes(&d), vec!["RAMB18E1".to_string()], "{w:?}");
}

/// G11: a 64 x 32 ROM costs the same as two RAMB18
/// or one RAMB36; it must take the RAMB18s, since the router does not wire
/// RAMB36E1 yet.
#[test]
fn g11_tie_between_ramb18_and_ramb36_takes_ramb18() {
    let items: Vec<String> = (0..64).map(|i| format!("32'h{:08x}", 0x0101_0101u32.wrapping_mul(i + 1))).collect();
    let (d, _) = synth(&format!(
        "module r (input clk, input [5:0] a, output logic [31:0] q);
           logic [31:0] rom [0:63] = '{{{}}};
           always_ff @(posedge clk) q <= rom[a];
         endmodule",
        items.join(", ")
    ))
    .unwrap();
    assert_eq!(boxes(&d), vec!["RAMB18E1".to_string(), "RAMB18E1".to_string()]);
}
