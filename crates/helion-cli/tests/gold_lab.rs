//! Gating: gold WNS, empty-shell honesty, bitstream refuse, no-cable program.

use std::process::Command;

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_helion")
}

#[test]
fn report_timing_counter_prints_wns_ps_9640() {
    let root = root();
    let src = root.join("examples/counter.sv");
    let out = Command::new(bin())
        .args(["report_timing", src.to_str().unwrap()])
        .current_dir(&root)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "report_timing failed: {text}");
    assert!(
        text.contains("WNS_PS=9640"),
        "gold WNS missing in shipped report_timing: {text}"
    );
}

#[test]
fn report_timing_empty_shell_does_not_close_wns() {
    let dir = std::env::temp_dir().join(format!("helion_empty_shell_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let src = dir.join("empty_shell.sv");
    std::fs::write(
        &src,
        "module empty_shell(input clk, input rst, output led);\nendmodule\n",
    )
    .unwrap();
    let out = Command::new(bin())
        .args(["report_timing", src.to_str().unwrap()])
        .current_dir(root())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let text = format!("{}{}", stdout, String::from_utf8_lossy(&out.stderr));
    assert!(
        !stdout.contains("WNS_PS=9640"),
        "empty shell must not inherit counter gold WNS: {text}"
    );
    let closed = stdout.split_whitespace().any(|t| {
        t.strip_prefix("WNS_PS=")
            .and_then(|v| v.parse::<i64>().ok())
            .map(|w| w != 0)
            .unwrap_or(false)
    });
    assert!(
        !closed,
        "empty/no_body must not print a closed WNS_PS on stdout: {text}"
    );
    assert!(
        stdout.contains("no_body") || text.contains("no_body") || !out.status.success(),
        "empty shell should diagnose no_body or fail honestly: {text}"
    );
}

#[test]
fn bitstream_refuses_empty_shell() {
    let dir = std::env::temp_dir().join(format!("helion_empty_bits_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let src = dir.join("empty_bits.sv");
    std::fs::write(
        &src,
        "module empty_bits(input clk, output led);\nendmodule\n",
    )
    .unwrap();
    let outp = dir.join("empty.hbits");
    let out = Command::new(bin())
        .args([
            "bitstream",
            src.to_str().unwrap(),
            "-o",
            outp.to_str().unwrap(),
        ])
        .current_dir(root())
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "write_bitstream must refuse empty .hbits: {text}"
    );
    assert!(
        text.to_ascii_lowercase().contains("refus")
            || text.to_ascii_lowercase().contains("empty")
            || text.contains("no_body")
            || text.contains("no configured frames"),
        "refuse message missing: {text}"
    );
    assert!(!outp.exists() || std::fs::metadata(&outp).map(|m| m.len()).unwrap_or(0) == 0);
}

#[test]
fn hw_program_without_bitstream_refuses_done_on_non_sim() {
    let out = Command::new(bin())
        .args(["hw", "program", "--cable", "native"])
        .current_dir(root())
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "native program without bitstream must not succeed: {text}"
    );
    let low = text.to_ascii_lowercase();
    assert!(
        !low.contains("stat done=1") && !text.contains("DONE=1"),
        "must not claim DONE without a cable/bitstream: {text}"
    );
}
