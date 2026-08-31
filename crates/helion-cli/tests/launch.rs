use std::process::Command;

#[test]
fn version_and_doctor() {
    let bin = env!("CARGO_BIN_EXE_helion");
    let v = Command::new(bin)
        .arg("--version")
        .output()
        .expect("helion --version");
    assert!(v.status.success(), "version failed: {}", String::from_utf8_lossy(&v.stderr));
    let stdout = String::from_utf8_lossy(&v.stdout);
    assert!(stdout.contains("helion"), "{stdout}");

    let d = Command::new(bin)
        .arg("doctor")
        .output()
        .expect("helion doctor");
    assert!(d.status.success(), "doctor failed: {}", String::from_utf8_lossy(&d.stderr));
    let out = String::from_utf8_lossy(&d.stdout);
    assert!(out.contains("0x00011a1f") || out.contains("0x00011A1F"), "{out}");
    assert!(out.contains("ok"), "{out}");

    let hw = Command::new(bin)
        .args(["hw", "program", "--cable", "sim"])
        .output()
        .expect("hw program");
    assert!(hw.status.success(), "{}", String::from_utf8_lossy(&hw.stderr));
    let hwo = String::from_utf8_lossy(&hw.stdout);
    assert!(hwo.contains("DONE=1"), "{hwo}");
}
