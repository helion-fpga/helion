//! W-L1: headless stage= / provenance= and gold empty-XDC WNS_PS=9640.

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

fn field(out: &str, key: &str) -> String {
    out.split_whitespace()
        .find_map(|t| t.strip_prefix(key).map(|v| v.to_string()))
        .unwrap_or_else(|| panic!("no {key} in {out}"))
}

#[test]
fn report_timing_empty_xdc_counter_default_period_gold() {
    let root = root();
    let src = root.join("examples/counter.sv");
    let out = Command::new(bin())
        .args(["report_timing", src.to_str().unwrap(), "--print-stages"])
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
        "gold WNS missing: {text}"
    );
    assert_eq!(
        field(&text, "provenance="),
        "DefaultPeriod",
        "empty-XDC counter injects 10 ns: {text}"
    );
    assert!(
        text.contains("stage=Sta") || text.contains("stage=Bitgen"),
        "timing line must print stage=: {text}"
    );
    assert!(
        text.contains("stage=Idle ready=1")
            && text.contains("stage=Elaborated ready=1")
            && text.contains("stage=Packed ready=1")
            && text.contains("stage=Placed ready=1")
            && text.contains("stage=Routed ready=1")
            && text.contains("stage=Sta ready=1"),
        "--print-stages table missing: {text}"
    );
}

#[test]
fn report_timing_user_xdc_counter_is_userxdc_gold() {
    let root = root();
    let src = root.join("examples/counter.sv");
    let sdc = root.join("examples/counter.sdc");
    let out = Command::new(bin())
        .args([
            "report_timing",
            src.to_str().unwrap(),
            "--sdc",
            sdc.to_str().unwrap(),
        ])
        .current_dir(&root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let text = format!("{}{}", stdout, String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "report_timing --sdc failed: {text}");
    assert_eq!(field(&stdout, "WNS_PS="), "9640", "{text}");
    assert_eq!(
        field(&stdout, "provenance="),
        "UserXdc",
        "counter.sdc create_clock is UserXdc: {text}"
    );
}

#[test]
fn project_counter_prints_userxdc_and_stages() {
    let root = root();
    let prj = root.join("examples/counter.prj");
    let out = Command::new(bin())
        .args(["project", prj.to_str().unwrap(), "--print-stages"])
        .current_dir(&root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let text = format!("{}{}", stdout, String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "project failed: {text}");
    assert_eq!(field(&stdout, "WNS_PS="), "9640", "{text}");
    assert_eq!(
        field(&stdout, "provenance="),
        "UserXdc",
        "counter.prj SDC is UserXdc: {text}"
    );
    assert_eq!(
        field(&stdout, "stage="),
        "Bitgen",
        "impl writes bitstream: {text}"
    );
    assert!(
        text.contains("stage=Bitgen ready=1"),
        "--print-stages must show Bitgen ready: {text}"
    );
}

#[test]
fn report_timing_combo_no_clock_path() {
    let dir = std::env::temp_dir().join(format!("helion_noclk_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let src = dir.join("combo.sv");
    std::fs::write(
        &src,
        "module combo(input a, output led);\n  assign led = a;\nendmodule\n",
    )
    .unwrap();
    let out = Command::new(bin())
        .args(["report_timing", src.to_str().unwrap()])
        .current_dir(root())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let text = format!("{}{}", stdout, String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "combo STA should complete honestly: {text}");
    assert!(
        !stdout.contains("WNS_PS=9640"),
        "combo must not inherit counter gold: {text}"
    );
    assert_eq!(
        field(&stdout, "provenance="),
        "NoClockPath",
        "combo without FF clock path: {text}"
    );
}
