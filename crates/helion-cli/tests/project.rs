//! Project honesty: multi-file sources + read_xdc wired into impl/run.
//! Counter gold WNS_PS=9640 and LED waveform must hold via `helion project run`.

use std::process::Command;

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn field(out: &str, key: &str) -> String {
    out.split_whitespace()
        .find_map(|t| t.strip_prefix(key).map(|v| v.to_string()))
        .unwrap_or_else(|| panic!("no {key} in {out}"))
}

#[test]
fn project_counter_sdc_holds_gold_wns() {
    let bin = env!("CARGO_BIN_EXE_helion");
    let root = root();
    let prj = root.join("examples/counter.prj");
    let out = Command::new(bin)
        .args(["project", "run", prj.to_str().unwrap(), "--cycles", "16"])
        .current_dir(&root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "project run failed:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("sources=1"),
        "expected single source: {stdout}"
    );
    assert!(
        stdout.contains("xdc_files=1"),
        "expected read_xdc: {stdout}"
    );
    assert!(
        stdout.contains("create_clock=1"),
        "SDC create_clock must load: {stdout}"
    );
    let wns: i64 = field(&stdout, "WNS_PS=").parse().unwrap();
    assert_eq!(wns, 9640, "counter project WNS must stay gold: {stdout}");
    assert!(
        stdout.contains("0000000111111110"),
        "counter LED gold via project run: {stdout}"
    );
}

#[test]
fn project_multi_file_with_sdc_impls() {
    let bin = env!("CARGO_BIN_EXE_helion");
    let root = root();
    let prj = root.join("examples/multi/multi.prj");
    let out = Command::new(bin)
        .args(["project", prj.to_str().unwrap()])
        .current_dir(&root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "multi project failed:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("sources=2"),
        "multi-file sources list: {stdout}"
    );
    assert!(
        stdout.contains("top=top"),
        "top module from prj: {stdout}"
    );
    assert!(
        stdout.contains("xdc_files=1"),
        "read_xdc wired: {stdout}"
    );
    assert!(
        stdout.contains("lutffs=1"),
        "hier-equivalent multi-file should pack 1 LUTFF: {stdout}"
    );
    let wns: i64 = field(&stdout, "WNS_PS=").parse().unwrap();
    assert_eq!(wns, 9700, "multi/top should match hier gold WNS: {stdout}");
}

#[test]
fn project_read_ip_counter_holds_gold() {
    let bin = env!("CARGO_BIN_EXE_helion");
    let root = root();
    let prj = root.join("examples/ip_ingest/counter_ip.prj");
    let out = Command::new(bin)
        .args(["project", "run", prj.to_str().unwrap(), "--cycles", "16"])
        .current_dir(&root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "read_ip project run failed:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("ip=1"),
        "expected read_ip expand count: {stdout}"
    );
    assert!(
        stdout.contains("sources=1"),
        "counter.helion must expand to one SV: {stdout}"
    );
    assert!(
        stdout.contains("xdc_files=1"),
        "counter.helion xdc must expand: {stdout}"
    );
    let wns: i64 = field(&stdout, "WNS_PS=").parse().unwrap();
    assert_eq!(wns, 9640, "read_ip counter WNS must stay gold: {stdout}");
    assert!(
        stdout.contains("0000000111111110"),
        "counter LED gold via read_ip: {stdout}"
    );
}

#[test]
fn helion_ip_show_gpio_catalog_package() {
    let bin = env!("CARGO_BIN_EXE_helion");
    let root = root();
    let out = Command::new(bin)
        .args(["ip", "show", "ip/h_gpio/h_gpio.helion"])
        .current_dir(&root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "ip show failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("vlnv=community:helion:h_gpio:1.0"), "{stdout}");
    assert!(stdout.contains("bus=Helion-MM"), "{stdout}");
    assert!(stdout.contains("h_gpio.v"), "{stdout}");
    assert!(!stdout.to_ascii_lowercase().contains("axi"), "{stdout}");
}

#[test]
fn project_read_ip_examples_ip_dir_holds_gold() {
    let bin = env!("CARGO_BIN_EXE_helion");
    let root = root();
    let prj = root.join("examples/ip/read_ip_counter.prj");
    let out = Command::new(bin)
        .args(["project", "run", prj.to_str().unwrap(), "--cycles", "16"])
        .current_dir(&root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "examples/ip read_ip failed:\n{stdout}\n{stderr}"
    );
    assert!(stdout.contains("ip=1"), "expected read_ip expand: {stdout}");
    assert!(stdout.contains("sources=1"), "dir package SV expand: {stdout}");
    assert!(stdout.contains("xdc_files=1"), "dir package xdc expand: {stdout}");
    let wns: i64 = field(&stdout, "WNS_PS=").parse().unwrap();
    assert_eq!(wns, 9640, "examples/ip counter WNS gold: {stdout}");
    assert!(
        stdout.contains("0000000111111110"),
        "examples/ip LED gold: {stdout}"
    );
}

#[test]
fn helion_ip_show_rejects_axi_fence() {
    let bin = env!("CARGO_BIN_EXE_helion");
    let root = root();
    let bad = root.join("target/test_axi_reject.helion");
    std::fs::write(
        &bad,
        "format 1\nvlnv community:helion:bad:1.0\nbus AXI\ntop t\nfile missing.v\n",
    )
    .unwrap();
    let out = Command::new(bin)
        .args(["ip", "show", bad.to_str().unwrap()])
        .current_dir(&root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}{stderr}");
    assert!(!out.status.success(), "AXI package must fail: {combined}");
    assert!(
        combined.contains("not AXI") || combined.to_ascii_lowercase().contains("axi"),
        "legal fence message: {combined}"
    );
}

