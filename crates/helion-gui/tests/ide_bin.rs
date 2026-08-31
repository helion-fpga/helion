//! Headless smoke of the GUI binary. Never opens a window.

use std::process::Command;

#[test]
fn helion_ide_version_and_doctor_are_headless() {
    let bin = env!("CARGO_BIN_EXE_helion-ide");
    let v = Command::new(bin)
        .arg("--version")
        .output()
        .expect("helion-ide --version");
    assert!(
        v.status.success(),
        "version failed: {}",
        String::from_utf8_lossy(&v.stderr)
    );
    let stdout = String::from_utf8_lossy(&v.stdout);
    assert!(
        stdout.starts_with("helion-ide 1.0.0"),
        "version must identify the GUI binary: {stdout}"
    );
    assert!(
        !stdout.to_lowercase().contains("error"),
        "{stdout}"
    );

    let d = Command::new(bin)
        .arg("--doctor")
        .output()
        .expect("helion-ide --doctor");
    assert!(
        d.status.success(),
        "doctor failed: {} {}",
        String::from_utf8_lossy(&d.stderr),
        String::from_utf8_lossy(&d.stdout)
    );
    let out = String::from_utf8_lossy(&d.stdout);
    assert!(out.contains("target "), "{out}");
    assert!(out.contains("HAD HL10T-C32-1"), "{out}");
    assert!(
        out.contains("0x00011a1f") || out.contains("0x00011A1F"),
        "{out}"
    );
    assert!(out.contains("ok"), "{out}");
    // A display-less CI host must not try to open a window for these flags.
    assert!(!out.contains("egui"), "doctor must not start the GUI: {out}");
}

#[test]
fn helion_ide_stdin_flow_refuses_route_before_place() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let bin = env!("CARGO_BIN_EXE_helion-ide");
    let sv = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/counter.sv");
    let mut child = Command::new(bin)
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn helion-ide --stdin");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        writeln!(stdin, "open {}", sv.display()).unwrap();
        writeln!(stdin, "flow route").unwrap();
        writeln!(stdin, "tree").unwrap();
        writeln!(stdin, "quit").unwrap();
    }
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "stdin ide failed: {} {}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("Place first"),
        "flow route without place must refuse: {s}"
    );
    assert!(s.contains("u_lut0"), "tree must list real HNF cells: {s}");
    assert!(s.contains("LUT6"), "tree must list primitives: {s}");
    assert!(s.contains("ERROR"), "refusal is journaled as ERROR: {s}");
}

#[test]
fn helion_ide_headless_prints_real_wns() {
    let bin = env!("CARGO_BIN_EXE_helion-ide");
    let sv = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/counter.sv");
    let out = Command::new(bin)
        .args(["--headless", sv.to_str().unwrap()])
        .output()
        .expect("helion-ide --headless");
    assert!(
        out.status.success(),
        "headless failed: {} {}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    let wns: i64 = s
        .split_whitespace()
        .find_map(|t| t.strip_prefix("WNS_PS="))
        .expect("headless must print WNS_PS= from the real Session")
        .parse()
        .expect("WNS must be numeric, not a canned string");
    assert!(wns != 0, "WNS_PS={wns} must come from STA");
    assert!(
        wns.abs() < 100_000,
        "WNS_PS={wns} is not a picosecond slack from STA"
    );
    assert!(!s.to_lowercase().contains("egui"), "headless must not start the GUI: {s}");
}

#[test]
fn helion_ide_help_links_the_binary() {
    let bin = env!("CARGO_BIN_EXE_helion-ide");
    let out = Command::new(bin)
        .arg("--help")
        .output()
        .expect("helion-ide --help");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let s = format!("{}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    assert!(s.contains("helion-ide"), "{s}");
    assert!(s.contains("--headless"), "{s}");
}

#[test]
fn macos_app_script_errors_without_apple_sdk_and_layout_only_ships_had() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = root.join("scripts/build-macos-app.sh");
    assert!(script.is_file(), "missing {}", script.display());

    if cfg!(not(target_os = "macos")) {
        let fail = Command::new("sh")
            .arg(&script)
            .output()
            .expect("run build-macos-app.sh");
        assert!(
            !fail.status.success(),
            "Linux must not pretend to link aarch64-apple-darwin"
        );
        let err = format!(
            "{}{}",
            String::from_utf8_lossy(&fail.stderr),
            String::from_utf8_lossy(&fail.stdout)
        );
        assert!(
            err.contains("macOS") || err.contains("Apple SDK") || err.contains("aarch64-apple-darwin"),
            "script must error clearly: {err}"
        );
    }

    let outdir = std::env::temp_dir().join(format!("helion-app-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&outdir);
    let layout = Command::new("sh")
        .arg(&script)
        .args(["--layout-only", "--out"])
        .arg(&outdir)
        .output()
        .expect("layout-only");
    assert!(
        layout.status.success(),
        "layout-only failed: {} {}",
        String::from_utf8_lossy(&layout.stderr),
        String::from_utf8_lossy(&layout.stdout)
    );
    let had = outdir.join("Helion.app/Contents/Resources/devices/helion/parts/HL10T-C32-1.toml");
    assert!(had.is_file(), "bundle must ship HAD: {}", had.display());
    let plist = outdir.join("Helion.app/Contents/Info.plist");
    let plist_s = std::fs::read_to_string(&plist).unwrap();
    assert!(plist_s.contains("Helion") || plist_s.contains("helion-ide"), "{plist_s}");
    assert!(plist_s.contains("LSMinimumSystemVersion"), "{plist_s}");
    let _ = std::fs::remove_dir_all(&outdir);
}
