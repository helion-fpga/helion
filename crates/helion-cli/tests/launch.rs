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

    let counter = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/counter.sv");
    let run = Command::new(bin)
        .args([
            "run",
            counter.to_str().unwrap(),
            "--cycles",
            "16",
        ])
        .output()
        .expect("helion run counter");
    assert!(
        run.status.success(),
        "run failed: {} {}",
        String::from_utf8_lossy(&run.stderr),
        String::from_utf8_lossy(&run.stdout)
    );
    let out = String::from_utf8_lossy(&run.stdout);
    assert!(out.contains("DONE=1"), "{out}");
    assert!(out.contains("LED[16]="), "{out}");
    assert!(
        out.contains("0000000111111110"),
        "counter LED must be cnt[3] over 16 cycles, got {out}"
    );
    assert!(out.contains("ok"), "{out}");

    let util = Command::new(bin)
        .args(["report_utilization", counter.to_str().unwrap()])
        .output()
        .expect("util");
    assert!(util.status.success(), "{}", String::from_utf8_lossy(&util.stderr));
    let uo = String::from_utf8_lossy(&util.stdout);
    assert!(uo.contains("LUTFF=4/8192"), "{uo}");

    let hier = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/hier.sv");
    let hrun = Command::new(bin)
        .args(["run", hier.to_str().unwrap(), "--cycles", "8"])
        .output()
        .expect("hier run");
    assert!(hrun.status.success(), "{}", String::from_utf8_lossy(&hrun.stderr));
    let ho = String::from_utf8_lossy(&hrun.stdout);
    assert!(ho.contains("changes="), "{ho}");
    assert!(!ho.contains("changes=0"), "hier LED must toggle: {ho}");

    let vhd = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/blinky.vhd");
    let vr = Command::new(bin)
        .args(["synth", vhd.to_str().unwrap()])
        .output()
        .expect("vhdl synth");
    assert!(vr.status.success(), "{}", String::from_utf8_lossy(&vr.stderr));
    assert!(String::from_utf8_lossy(&vr.stdout).contains("luts=1"), "{}", String::from_utf8_lossy(&vr.stdout));

    let prj = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/blinky.prj");
    let pr = Command::new(bin)
        .args(["project", prj.to_str().unwrap()])
        .output()
        .expect("project");
    assert!(pr.status.success(), "{}", String::from_utf8_lossy(&pr.stderr));
    let po = String::from_utf8_lossy(&pr.stdout);
    assert!(po.contains("PACKAGE_PIN=1"), "{po}");
    assert!(po.contains("create_clock=1"), "{po}");
}
