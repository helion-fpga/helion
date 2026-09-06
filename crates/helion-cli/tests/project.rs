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
