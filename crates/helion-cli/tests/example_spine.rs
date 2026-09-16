//! W-L5 example spine. Labs 1–2 always; 3–5 landed mapped (SOFT=0).
//! Empty-XDC counter gold stays WNS_PS=9640.

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

fn helion(args: &[&str]) -> String {
    let root = root();
    let out = Command::new(bin())
        .args(args)
        .current_dir(&root)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "{} failed: {text}",
        args.join(" ")
    );
    text
}

fn assert_no_axi(text: &str) {
    assert!(
        !text.to_ascii_lowercase().contains("axi"),
        "example spine must not mention AXI: {text}"
    );
}

#[test]
fn spine_counter_empty_xdc_gold_wns_9640() {
    let text = helion(&["report_timing", "examples/counter.sv"]);
    assert_eq!(field(&text, "WNS_PS="), "9640", "{text}");
    assert!(text.contains("SOFT=0"), "{text}");
}

#[test]
fn spine_blinky_user_clock_and_loc() {
    let text = helion(&["project", "examples/blinky.prj"]);
    assert!(text.contains("xdc_files=1"), "{text}");
    assert!(text.contains("create_clock=1"), "{text}");
    assert!(text.contains("PACKAGE_PIN=1"), "{text}");
    assert_eq!(field(&text, "cells="), "3", "{text}");
    assert_eq!(field(&text, "lutffs="), "1", "{text}");
    assert_eq!(field(&text, "WNS_PS="), "9700", "{text}");
    assert!(text.contains("SOFT=0"), "{text}");
}

#[test]
fn spine_uart_maps() {
    let text = helion(&["project", "examples/uart/uart.prj"]);
    assert!(text.contains("create_clock=1"), "{text}");
    assert!(text.contains("PACKAGE_PIN=2"), "{text}");
    assert_eq!(field(&text, "cells="), "10", "{text}");
    assert_eq!(field(&text, "lutffs="), "5", "{text}");
    assert_eq!(field(&text, "WNS_PS="), "9640", "{text}");
    assert!(text.contains("SOFT=0"), "{text}");
    assert_no_axi(&text);
}

#[test]
fn spine_scratch_mm_maps() {
    let text = helion(&["project", "examples/scratch_mm/scratch_mm.prj"]);
    assert!(text.contains("create_clock=1"), "{text}");
    assert!(text.contains("top=scratch_mm"), "{text}");
    assert_eq!(field(&text, "cells="), "97", "{text}");
    assert_eq!(field(&text, "lutffs="), "48", "{text}");
    assert_eq!(field(&text, "WNS_PS="), "9540", "{text}");
    assert!(text.contains("SOFT=0"), "{text}");
    assert_no_axi(&text);
}

#[test]
fn spine_hcore_maps() {
    let text = helion(&["project", "examples/hcore/hcore.prj"]);
    assert!(text.contains("create_clock=1"), "{text}");
    assert_eq!(field(&text, "cells="), "36", "{text}");
    assert_eq!(field(&text, "lutffs="), "21", "{text}");
    assert_eq!(field(&text, "WNS_PS="), "9600", "{text}");
    assert!(text.contains("SOFT=0"), "{text}");
    assert_no_axi(&text);
}
