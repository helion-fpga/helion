//! Regression tests for `helion synth` multi-file SoC support (commit ad0db15).
//!
//! Covers: multiple .sv/.v files synthesizing together, flag placement
//! before/between/after source files, mixed unsupported extensions failing
//! clearly, and single-file synthesis still working.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const LEAF_SV: &str = r#"module leaf(input logic clk, input logic d, output logic q);
    always @(posedge clk) q <= d;
endmodule
"#;

const SOC_SV: &str = r#"module soc_top(input logic clk, input logic d, output logic q);
    leaf u_leaf(.clk(clk), .d(d), .q(q));
endmodule
"#;

const LEAF_V: &str = r#"module leaf_v(input wire clk, input wire d, output reg q);
    always @(posedge clk) q <= d;
endmodule
"#;

const SOC_V_TOP: &str = r#"module soc_top_v(input wire clk, input wire d, output wire q);
    leaf_v u_leaf(.clk(clk), .d(d), .q(q));
endmodule
"#;

/// Unique scratch dir per test so parallel tests never collide.
fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("helion-multifile-{}-{}", test, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn write(dir: &PathBuf, name: &str, contents: &str) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, contents).expect("write test source");
    p
}

fn helion() -> Command {
    Command::new(env!("CARGO_BIN_EXE_helion"))
}

/// Parse `cells=N` out of the `synth <name> cells=N luts=M ...` stdout line.
fn cells_of(stdout: &str) -> usize {
    let line = stdout
        .lines()
        .find(|l| l.starts_with("synth "))
        .unwrap_or_else(|| panic!("no `synth ...` line in stdout:\n{stdout}"));
    let cells = line
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix("cells="))
        .unwrap_or_else(|| panic!("no cells= in line: {line}"));
    cells.parse().unwrap_or_else(|_| panic!("bad cells value: {cells}"))
}

#[test]
fn multifile_two_sv_synthesizes_together() {
    let dir = scratch("two-sv");
    let leaf = write(&dir, "leaf.sv", LEAF_SV);
    let soc = write(&dir, "soc.sv", SOC_SV);

    let out = helion()
        .arg("synth")
        .arg("--top")
        .arg("soc_top")
        .arg(&leaf)
        .arg(&soc)
        .output()
        .expect("run helion synth");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "multi-file synth failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("synth soc_top"),
        "expected top module soc_top in output:\n{stdout}"
    );
    assert!(cells_of(&stdout) > 0, "expected nonzero cells:\n{stdout}");
}

#[test]
fn multifile_mixed_sv_and_v() {
    let dir = scratch("sv-and-v");
    let leaf = write(&dir, "leaf.v", LEAF_V);
    let soc = write(&dir, "soc_v.sv", SOC_V_TOP);

    let out = helion()
        .arg("synth")
        .arg("--top")
        .arg("soc_top_v")
        .arg(&leaf)
        .arg(&soc)
        .output()
        .expect("run helion synth");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "mixed .sv/.v synth failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(cells_of(&stdout) > 0, "expected nonzero cells:\n{stdout}");
}

#[test]
fn multifile_flag_placement_before_between_after() {
    // `--part` before, between, and after the source files must all parse
    // the same two positionals and produce the same design.
    let dir = scratch("flag-placement");
    let leaf = write(&dir, "leaf.sv", LEAF_SV);
    let soc = write(&dir, "soc.sv", SOC_SV);
    let leaf_s = leaf.to_string_lossy().to_string();
    let soc_s = soc.to_string_lossy().to_string();

    let argsets: Vec<Vec<String>> = vec![
        vec![
            "synth".into(),
            "--part".into(),
            "HL10T-C32-1".into(),
            leaf_s.clone(),
            soc_s.clone(),
        ],
        vec![
            "synth".into(),
            leaf_s.clone(),
            "--part".into(),
            "HL10T-C32-1".into(),
            soc_s.clone(),
        ],
        vec![
            "synth".into(),
            leaf_s.clone(),
            soc_s.clone(),
            "--part".into(),
            "HL10T-C32-1".into(),
        ],
    ];

    let mut counts = Vec::new();
    for argv in &argsets {
        let out = helion()
            .args(&argv[1..])
            .arg("--top")
            .arg("soc_top")
            .output()
            .expect("run helion synth");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "flag placement {argv:?} failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        counts.push(cells_of(&stdout));
    }
    assert_eq!(
        counts[0], counts[1],
        "before/between flag placement diverged: {counts:?}"
    );
    assert_eq!(
        counts[1], counts[2],
        "between/after flag placement diverged: {counts:?}"
    );
    assert!(counts[0] > 0);
}

#[test]
fn multifile_mixed_unsupported_extension_fails_clearly() {
    let dir = scratch("bad-ext");
    let good = write(&dir, "leaf.sv", LEAF_SV);
    let bad = write(&dir, "notes.txt", "these are not hardware\n");

    let out = helion()
        .arg("synth")
        .arg(&good)
        .arg(&bad)
        .output()
        .expect("run helion synth");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "expected failure for mixed .sv/.txt input"
    );
    assert!(
        stderr.contains("multi-file only supports .sv/.v files"),
        "expected clear extension error, got:\n{stderr}"
    );
}

#[test]
fn single_file_still_works_no_regression() {
    let dir = scratch("single");
    let leaf = write(&dir, "leaf.sv", LEAF_SV);

    let out = helion()
        .arg("synth")
        .arg(&leaf)
        .output()
        .expect("run helion synth");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "single-file synth regressed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("synth leaf"),
        "expected module leaf in output:\n{stdout}"
    );
    assert!(cells_of(&stdout) > 0);
}

#[test]
fn multifile_top_flag_selects_module() {
    // `--top` must select which module is synthesized.
    let dir = scratch("top-flag");
    let leaf = write(&dir, "leaf.sv", LEAF_SV);
    let soc = write(&dir, "soc.sv", SOC_SV);

    for top in ["soc_top", "leaf"] {
        let out = helion()
            .arg("synth")
            .arg("--top")
            .arg(top)
            .arg(&leaf)
            .arg(&soc)
            .output()
            .expect("run helion synth");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "--top {top} failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            stdout.contains(&format!("synth {top}")),
            "expected `synth {top}` in output:\n{stdout}"
        );
    }
}
