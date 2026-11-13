//! Power-up values from `initial` blocks in the v2 front end: plain
//! assignments, loops and `$readmemh` / `$readmemb`, and the errors for
//! anything that is not constant at elaboration. `tools/svcheck` checks the
//! same designs against a reference simulator; these tests pin the rules
//! it cannot (IEEE address order where simulators differ, diagnostics).

use helion_ir::{resolve, CellKind, Design};
use std::collections::HashMap;
use std::path::PathBuf;

/// Evaluate the netlist's combinational logic from power-up (every flop
/// net 0, which is how v2 builds power-up values) and read output `port`.
fn out_after(d: &Design, inputs: &[(&str, u32, u64)], port: &str, clocks: u32) -> u64 {
    let pins = d.pin_index();
    let net = |cell: &str, pin: &str| pins.net_on(cell, pin).map(|n| resolve(n).to_string());
    let mut luts = Vec::new();
    let mut ffs = Vec::new();
    let mut outs = Vec::new();
    for c in &d.cells {
        let name = resolve(c.name);
        match c.kind {
            CellKind::Lut6 { init } => luts.push((init, (0..6).map(|i| net(name, &format!("I{i}"))).collect::<Vec<_>>(), net(name, "O").unwrap())),
            CellKind::Hff => ffs.push((net(name, "D").unwrap(), net(name, "Q").unwrap())),
            CellKind::IobOut => outs.push((c.attrs.get("PORT").unwrap().to_string(), c.attrs.get("BIT").unwrap().parse::<u32>().unwrap(), net(name, "I").unwrap())),
            _ => panic!("unexpected cell in {name}"),
        }
    }
    let mut val: HashMap<String, bool> = HashMap::new();
    for (p, w, v) in inputs {
        for i in 0..*w {
            val.insert(if *w == 1 { p.to_string() } else { format!("{p}_{i}") }, (v >> i) & 1 == 1);
        }
    }
    let settle = |val: &mut HashMap<String, bool>| {
        for _ in 0..200 {
            let mut changed = false;
            for (init, ins, o) in &luts {
                let a = ins.iter().enumerate().fold(0u64, |a, (i, n)| a | ((n.as_ref().is_some_and(|n| val.get(n).copied().unwrap_or(false)) as u64) << i));
                let v = (init >> a) & 1 == 1;
                changed |= val.insert(o.clone(), v) != Some(v);
            }
            if !changed {
                return;
            }
        }
        panic!("no settle");
    };
    settle(&mut val);
    for _ in 0..clocks {
        let next: Vec<(String, bool)> = ffs.iter().map(|(d, q)| (q.clone(), val.get(d).copied().unwrap_or(false))).collect();
        val.extend(next);
        settle(&mut val);
    }
    outs.iter().filter(|o| o.0 == port).fold(0, |v, (_, b, n)| v | ((val.get(n).copied().unwrap_or(false) as u64) << b))
}

struct Dir(PathBuf);
impl Dir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("helion_v2_initial_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn file(&self, name: &str, text: &str) -> PathBuf {
        let p = self.0.join(name);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(&p, text).unwrap();
        p
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn synth_in(dir: &Dir, src: &str) -> Result<helion_sv::v2::Synth, String> {
    let p = dir.file("top.sv", src);
    helion_sv::synth_sv_v2(src, &p.display().to_string(), None)
}

/// Words of an 8 x 8 ROM read combinationally at `a`.
fn rom_words(d: &Design) -> Vec<u64> {
    (0..8).map(|a| out_after(d, &[("a", 3, a)], "y", 0)).collect()
}

const ROM: &str = "module t(input logic [2:0] a, output logic [7:0] y);\n  logic [7:0] m [0:7] = '{default: 8'hee};\n  initial LOAD;\n  assign y = m[a];\nendmodule\n";

fn rom(dir: &Dir, load: &str) -> Result<helion_sv::v2::Synth, String> {
    synth_in(dir, &ROM.replace("LOAD", load))
}

#[test]
fn readmem_address_order_follows_the_standard() {
    let dir = Dir::new("order");
    dir.file("w.bin", "0000_0001 // one\n00000010\n/* c */ 00000011\n");
    // No addresses: from the lowest address up.
    let s = rom(&dir, "$readmemb(\"w.bin\", m)").unwrap();
    assert_eq!(rom_words(&s.design), vec![1, 2, 3, 0xee, 0xee, 0xee, 0xee, 0xee]);
    // A start: from there up.
    let s = rom(&dir, "$readmemb(\"w.bin\", m, 4)").unwrap();
    assert_eq!(rom_words(&s.design), vec![0xee, 0xee, 0xee, 0xee, 1, 2, 3, 0xee]);
    // Start above end: downwards (IEEE 1800 21.4).
    let s = rom(&dir, "$readmemb(\"w.bin\", m, 6, 4)").unwrap();
    assert_eq!(rom_words(&s.design), vec![0xee, 0xee, 0xee, 0xee, 3, 2, 1, 0xee]);
    // An @ address in the file moves the load.
    dir.file("a.hex", "11 @6 22 33\n");
    let s = rom(&dir, "$readmemh(\"a.hex\", m)").unwrap();
    assert_eq!(rom_words(&s.design), vec![0x11, 0xee, 0xee, 0xee, 0xee, 0xee, 0x22, 0x33]);
}

#[test]
fn readmem_warnings() {
    let dir = Dir::new("warn");
    dir.file("x.hex", "1x 2z 3?\n");
    let s = rom(&dir, "$readmemh(\"x.hex\", m)").unwrap();
    assert_eq!(rom_words(&s.design)[..3], [0x10, 0x20, 0x30]);
    assert!(s.warnings.iter().any(|w| w.contains("x or z digits load as 0")), "{:?}", s.warnings);
    dir.file("long.hex", "1 2 3 4 5 6 7 8 9 a\n");
    let s = rom(&dir, "$readmemh(\"long.hex\", m)").unwrap();
    assert!(s.warnings.iter().any(|w| w.contains("more words")), "{:?}", s.warnings);
    assert_eq!(rom_words(&s.design), vec![1, 2, 3, 4, 5, 6, 7, 8]);
    let s = rom(&dir, "$readmemh(\"x.hex\", m, 0, 5)").unwrap();
    assert!(s.warnings.iter().any(|w| w.contains("ends before address 5")), "{:?}", s.warnings);
    dir.file("wide.hex", "1ff\n");
    let s = rom(&dir, "$readmemh(\"wide.hex\", m)").unwrap();
    assert_eq!(rom_words(&s.design)[0], 0xff);
    assert!(s.warnings.iter().any(|w| w.contains("wider than the 8 bits")), "{:?}", s.warnings);
}

#[test]
fn readmem_paths_resolve_next_to_the_source_then_include_dirs() {
    let dir = Dir::new("path");
    dir.file("sub/inc/r.hex", "5 6 7\n");
    // Not next to the source: an error naming where it looked.
    let e = rom(&dir, "$readmemh(\"r.hex\", m)").err().unwrap();
    assert!(e.contains("cannot find the memory file 'r.hex'") && e.contains("line 3"), "{e}");
    // Relative to the source file's directory.
    let s = rom(&dir, "$readmemh(\"sub/inc/r.hex\", m)").unwrap();
    assert_eq!(rom_words(&s.design)[..3], [5, 6, 7]);
    // Through an include directory.
    let src = ROM.replace("LOAD", "$readmemh(\"r.hex\", m)");
    let top = dir.file("top.sv", &src);
    let s = helion_sv::v2::synth(&[(top.display().to_string(), src.clone())], None, &[], vec![dir.0.join("sub/inc")]).unwrap();
    assert_eq!(rom_words(&s.design)[..3], [5, 6, 7]);
    // An absolute path.
    let abs = dir.0.join("sub/inc/r.hex").display().to_string();
    let s = rom(&dir, &format!("$readmemh(\"{abs}\", m)")).unwrap();
    assert_eq!(rom_words(&s.design)[..3], [5, 6, 7]);
}

#[test]
fn what_is_not_constant_at_elaboration_is_an_error_with_its_line() {
    let dir = Dir::new("err");
    dir.file("ok.hex", "1\n");
    let cases: &[(&str, &str)] = &[
        ("module t(input logic clk, input logic [3:0] d, output logic [3:0] q);\n  initial begin\n    q = d;\n  end\n  always_ff @(posedge clk) q <= d;\nendmodule\n", "line 3"),
        ("module t(input logic clk, input logic c, input logic [3:0] d, output logic [3:0] q);\n  initial\n    if (c) q = 1;\n  always_ff @(posedge clk) q <= d;\nendmodule\n", "line 3"),
        ("module t(input logic clk, input logic [3:0] d, output logic [3:0] q);\n  initial begin\n    q = 1;\n    #5 q = 2;\n  end\n  always_ff @(posedge clk) q <= d;\nendmodule\n", "line 4"),
        ("module t(input logic clk, input logic [2:0] a, output logic [7:0] q);\n  logic [7:0] m [0:7];\n  always_ff @(posedge clk) begin\n    $readmemh(\"ok.hex\", m);\n    q <= m[a];\n  end\nendmodule\n", "line 4"),
        ("module t(input logic clk, input logic [7:0] d, output logic [7:0] q);\n  logic [7:0] r;\n  initial $readmemh(\"ok.hex\", r);\n  always_ff @(posedge clk) q <= d;\nendmodule\n", "line 3"),
        ("module t(input logic clk, input logic [2:0] a, output logic [7:0] q);\n  logic [7:0] m [0:7];\n  initial $readmemh(\"ok.hex\", m, 2, 9);\n  always_ff @(posedge clk) q <= m[a];\nendmodule\n", "line 3"),
    ];
    for (src, line) in cases {
        let e = synth_in(&dir, src).err().unwrap_or_else(|| panic!("accepted:\n{src}"));
        assert!(e.contains(line), "expected {line} in: {e}\n{src}");
    }
    dir.file("bad.hex", "1 2\n@9 3\n");
    let e = rom(&dir, "$readmemh(\"bad.hex\", m)").err().unwrap();
    assert!(e.contains("bad.hex:2: address @9 is outside"), "{e}");
    dir.file("bad2.hex", "1 2\n 3g\n");
    let e = rom(&dir, "$readmemh(\"bad2.hex\", m)").err().unwrap();
    assert!(e.contains("bad2.hex:2: '3g' is not a hexadecimal number"), "{e}");
}

#[test]
fn initial_blocks_set_register_power_up_values() {
    let dir = Dir::new("regs");
    // A counter powering up at 250 (loop over a local, nonblocking on top
    // of blocking, a later block overriding part of an earlier one).
    let src = "module t(input logic clk, output logic [7:0] q, output logic [3:0] k);\n  logic [7:0] c;\n  logic [3:0] kk;\n  initial begin\n    c = 0;\n    for (int i = 0; i < 10; i++) c = c + 8'd25;\n    c[0] <= 1'b1;\n  end\n  initial kk = 4'd9;\n  initial kk[3] = 1'b0;\n  always_ff @(posedge clk) c <= c + 8'd1;\n  assign q = c;\n  assign k = kk;\nendmodule\n";
    let s = synth_in(&dir, src).unwrap();
    assert_eq!(out_after(&s.design, &[], "q", 0), 251);
    assert_eq!(out_after(&s.design, &[], "q", 3), 254);
    // A variable nothing but `initial` assigns is a constant.
    assert_eq!(out_after(&s.design, &[], "k", 2), 1);
    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    // Simulation-only values in an initial block are dropped with a warning.
    let src = "module t(input logic clk, input logic d, output logic q);\n  integer fd;\n  initial fd = $fopen(\"log.txt\", \"w\");\n  always_ff @(posedge clk) q <= d;\nendmodule\n";
    let s = synth_in(&dir, src).unwrap();
    assert!(s.warnings.iter().any(|w| w.contains("'$fopen' in an 'initial' block is simulation only")), "{:?}", s.warnings);
}

/// Only a variable has a power-up value: an `initial` block assigning a
/// net (a `wire`, an input, an output with no data type) is an error with
/// its line, as the language has it (IEEE 1800, 6.5); one assigning a
/// variable that a continuous assignment or combinational logic also
/// drives warns that the value is ignored.
#[test]
fn initial_blocks_assign_variables_not_nets() {
    let dir = Dir::new("nets");
    let nets: &[&str] = &[
        "module t(input logic [7:0] a, output logic [7:0] q);\n  wire [7:0] w;\n  initial w = 8'h5;\n  assign q = w;\nendmodule\n",
        "module t(input logic [7:0] a, output [7:0] q);\n  initial\n    q = 8'h5;\nendmodule\n",
        "module t(input logic [7:0] a, output wire logic [7:0] q);\n  initial q = 8'h5;\nendmodule\n",
        "module t(input logic [7:0] a, output logic [7:0] q);\n  initial a = 8'h5;\n  assign q = a;\nendmodule\n",
        "module t(a, q);\n  input [7:0] a;\n  output [7:0] q;\n  initial q = 8'h5;\nendmodule\n",
        "module t(input logic [7:0] a, output logic [7:0] q);\n  wire [7:0] w = a;\n  initial begin\n    w[1] = 1'b1;\n  end\n  assign q = w;\nendmodule\n",
    ];
    for src in nets {
        let e = synth_in(&dir, src).err().unwrap_or_else(|| panic!("accepted:\n{src}"));
        assert!(e.contains("is a net; an 'initial' block cannot assign it"), "{e}\n{src}");
        let line = src.lines().position(|l| l.contains(" = 8'h5") || l.contains("w[1] =")).unwrap() + 1;
        assert!(e.contains(&format!("line {line}: ")), "expected line {line}: {e}\n{src}");
    }
    // Variables: an output with a data type, a non-ANSI port declared a
    // reg, a plain variable. Nothing else drives them, so they are
    // constants, without a warning.
    let vars: &[&str] = &[
        "module t(input logic [7:0] a, output logic [7:0] q);\n  initial q = 8'h5;\nendmodule\n",
        "module t(a, q);\n  input [7:0] a;\n  output [7:0] q;\n  reg [7:0] q;\n  initial q = 8'h5;\nendmodule\n",
        "module t(input logic [7:0] a, output reg [7:0] q);\n  initial q = 8'h5;\nendmodule\n",
        "module t(input logic [7:0] a, output logic [7:0] q);\n  logic [7:0] v;\n  initial v = 8'h5;\n  assign q = v;\nendmodule\n",
    ];
    for src in vars {
        let s = synth_in(&dir, src).unwrap_or_else(|e| panic!("{e}\n{src}"));
        assert_eq!(out_after(&s.design, &[("a", 8, 0)], "q", 0), 5, "{src}");
        assert!(s.warnings.is_empty(), "{:?}\n{src}", s.warnings);
    }
    // A variable something else drives: the initial value is ignored, with
    // a warning naming the block's line.
    let driven: &[&str] = &[
        "module t(input logic [7:0] a, output logic [7:0] q);\n  initial q = 8'h3;\n  assign q = a;\nendmodule\n",
        "module t(input logic [7:0] a, output logic [7:0] q);\n  logic [7:0] c;\n  initial c = 8'h3;\n  always_comb c = a + 8'd1;\n  assign q = c;\nendmodule\n",
    ];
    for src in driven {
        let s = synth_in(&dir, src).unwrap_or_else(|e| panic!("{e}\n{src}"));
        let line = src.lines().position(|l| l.contains("initial")).unwrap() + 1;
        assert!(
            s.warnings.iter().any(|w| w.starts_with(&format!("line {line}:")) && w.contains("also driven by a continuous assignment or combinational logic")),
            "{:?}\n{src}",
            s.warnings
        );
    }
    // A register keeps its initial value without a warning.
    let s = synth_in(&dir, "module t(input logic clk, input logic [7:0] a, output logic [7:0] q);\n  initial q = 8'h3;\n  always_ff @(posedge clk) q <= a;\nendmodule\n").unwrap();
    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
}
