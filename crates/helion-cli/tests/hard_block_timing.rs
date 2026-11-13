//! `helion report_timing` on designs whose only registers are hard
//! blocks (with HELION_V2_HARD=1): a registered product between pads and
//! a deep-enough RAM read between pads. They are timed (they used to be
//! reported as an empty shell, "no timing"), and at 1 ns they fail.

use std::process::Command;

fn report(name: &str, src: &str) -> String {
    let dir = std::env::temp_dir().join(format!("helion_hard_block_timing_{}_{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sv = dir.join(format!("{name}.sv"));
    let sdc = dir.join(format!("{name}.sdc"));
    std::fs::write(&sv, src).unwrap();
    std::fs::write(&sdc, "create_clock -name clk -period 1.000 [get_ports clk]\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_helion"))
        .args(["report_timing", sv.to_str().unwrap(), "--sdc", sdc.to_str().unwrap(), "--part", "HL10S-C64-1"])
        .env("HELION_V2_HARD", "1")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).lines().find(|l| l.starts_with("report_timing ")).unwrap_or_default().to_string()
}

fn field(line: &str, key: &str) -> i64 {
    line.split_whitespace().find_map(|t| t.strip_prefix(key)).unwrap_or_else(|| panic!("no {key} in {line}")).parse().unwrap()
}

#[test]
fn blocks_only_designs_are_timed() {
    let preg = "module preg_only(input logic clk, input logic [15:0] x, input logic [15:0] y, output logic [31:0] p);\n  always_ff @(posedge clk) p <= x * y;\nendmodule\n";
    let ram = "module ram_only(input logic clk, input logic we, input logic [9:0] wa, input logic [9:0] ra, input logic [15:0] d, output logic [15:0] q);\n  logic [15:0] mem [0:1023];\n  always_ff @(posedge clk) begin\n    if (we) mem[wa] <= d;\n    q <= mem[ra];\n  end\nendmodule\n";
    for (name, src) in [("preg_only", preg), ("ram_only", ram)] {
        let line = report(name, src);
        assert!(!line.contains("no_body"), "{line}");
        assert!(field(&line, "WNS_PS=") < 0 && field(&line, "TNS_PS=") < 0, "{line}");
        assert!(field(&line, "endpoints=") > 0, "{line}");
    }
}
