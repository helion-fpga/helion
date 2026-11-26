//! The `helion-ide` binary's windowless modes. None of these may open a
//! window (CI hosts have no display).

use std::io::Write;
use std::process::{Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_helion-ide")
}

fn example(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name)
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn version_help_and_doctor_identify_helion_ide() {
    let v = Command::new(bin()).arg("--version").output().unwrap();
    assert!(v.status.success());
    let s = stdout(&v);
    assert!(s.starts_with("helion-ide "), "{s}");
    assert!(s.contains(env!("CARGO_PKG_VERSION")), "{s}");

    let h = Command::new(bin()).arg("--help").output().unwrap();
    assert!(h.status.success());
    let s = stdout(&h);
    for flag in ["--headless", "--source", "--stdin", "--doctor"] {
        assert!(s.contains(flag), "help lacks {flag}: {s}");
    }

    let d = Command::new(bin()).arg("--doctor").output().unwrap();
    assert!(d.status.success());
    let s = stdout(&d);
    assert!(s.contains("HAD HL10T-C32-1") && s.contains("ok"), "{s}");
}

#[test]
fn headless_counter_is_the_release_gold() {
    for file in ["counter.sv", "counter.prj"] {
        let out = Command::new(bin())
            .arg("--headless")
            .arg(example(file))
            .output()
            .unwrap();
        let s = stdout(&out);
        assert!(out.status.success(), "{file}: {s}{}", String::from_utf8_lossy(&out.stderr));
        assert!(s.lines().any(|l| l == "WNS_PS=9640"), "{file}: {s}");
    }
}

/// A lone source plus its sibling `.xdc` on a 7-series part runs the same
/// XDC flow as a `.prj` and prints the view contract's reports.
#[test]
fn headless_lone_sv_with_xdc_on_a_7series_part() {
    let db = std::env::var_os("HELION_XILINX_DB")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache/helion/prjxray-db")));
    if !db.is_some_and(|d| d.join("zynq7").is_dir()) {
        eprintln!("skipped: no Project X-Ray zynq7 database (set HELION_XILINX_DB)");
        return;
    }
    let out = Command::new(bin())
        .args(["--headless"])
        .arg(example("zedboard_oled_demo.sv"))
        .args(["--part", "xc7z020clg484-1"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success(), "{s}{}", String::from_utf8_lossy(&out.stderr));
    assert!(s.starts_with("Xilinx 7-Series xc7z020clg484-1 Routed"), "{s}");
    assert!(s.contains("Slice LUTs") && s.contains("WHS (ns)"), "{s}");
    assert!(s.lines().any(|l| l.starts_with("WNS_PS=")), "{s}");
}

/// Every family's headless run prints the report set a 7-series run
/// prints: the eleven reports, the device layers and heat maps, the
/// schematic and board summary, the usage line and the WNS.
#[test]
fn headless_prints_the_same_report_set_for_hl10_ice40_and_ecp5() {
    for (file, head) in [("counter.prj", "Helion HL10T-C32-1 Routed"), ("ice40_counter.prj", "Lattice iCE40 ice40-hx8k Routed"), ("ecp5_counter.prj", "Lattice ECP5 LFE5U-85F Routed")] {
        let out = Command::new(bin()).arg("--headless").arg(example(file)).output().unwrap();
        let s = stdout(&out);
        assert!(out.status.success(), "{file}: {s}{}", String::from_utf8_lossy(&out.stderr));
        assert!(s.starts_with(head), "{file}: {s}");
        for title in [
            "Timing Summary", "Utilization", "Power", "Design Rule Check", "Methodology", "Design Analysis", "Clock Utilization", "Control Sets", "I/O Report", "High Fanout Nets", "Route Status",
            "Placed cells", "Routing", "Clock regions", "IO banks", "Package", "Congestion (horizontal)", "Congestion (vertical)", "Timing slack", "Power by tile",
        ] {
            assert!(s.lines().any(|l| l == title), "{file}: no {title} section: {s}");
        }
        for line in ["SCHEMATIC_RTL:", "SCHEMATIC_SYNTHESIZED:", "SCHEMATIC_IMPLEMENTED:", "BOARD:", "WNS_PS="] {
            assert!(s.lines().any(|l| l.starts_with(line)), "{file}: no {line} line: {s}");
        }
        assert!(s.contains("WNS (ns)") && s.contains("sites, 0 tiles over capacity"), "{file}: {s}");
        assert!(!s.contains("not available"), "{file}: a section names its missing input instead: {s}");
    }
}

#[test]
fn headless_missing_file_fails() {
    let out = Command::new(bin())
        .args(["--headless", "/nonexistent/x.sv"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn stdin_console_runs_tcl_and_keeps_going_after_errors() {
    let mut child = Command::new(bin())
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    {
        let mut i = child.stdin.take().unwrap();
        writeln!(i, "open {}", example("counter.sv").display()).unwrap();
        writeln!(i, "no_such_verb").unwrap();
        writeln!(i, "set x 21").unwrap();
        writeln!(i, "expr {{$x * 2}}").unwrap();
        writeln!(i, "report_utilization").unwrap();
        writeln!(i, "quit").unwrap();
        writeln!(i, "puts never").unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let s = stdout(&out);
    assert!(s.contains("synth counter"), "{s}");
    assert!(s.contains("ERROR: invalid command name \"no_such_verb\""), "{s}");
    assert!(s.lines().any(|l| l == "42"), "{s}");
    assert!(s.contains("LUTFF"), "{s}");
    assert!(!s.contains("never"), "quit must stop the console: {s}");
}

#[test]
fn source_runs_a_script_and_fails_on_the_first_error() {
    let dir = std::env::temp_dir().join(format!("helion-ide-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ok = dir.join("ok.tcl");
    std::fs::write(
        &ok,
        format!(
            "read_verilog {{{}}}\nputs \"synth done\"\nreport_timing\n",
            example("counter.sv").display()
        ),
    )
    .unwrap();
    let out = Command::new(bin()).arg("--source").arg(&ok).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let s = stdout(&out);
    assert!(s.contains("synth done") && s.contains("WNS 9.640ns"), "{s}");

    let bad = dir.join("bad.tcl");
    std::fs::write(&bad, "set a 1\nno_such_verb\nputs after\n").unwrap();
    let out = Command::new(bin()).arg("--source").arg(&bad).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(!stdout(&out).contains("after"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no_such_verb"));
    let _ = std::fs::remove_dir_all(&dir);
}
