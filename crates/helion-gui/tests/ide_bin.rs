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
