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
    // doctor must publish the HAD die/site/FeatureMap text report.
    assert!(out.contains("sites_clb=1024"), "doctor must report HAD CLB sites: {out}");
    assert!(out.contains("sites_iob=32"), "doctor must report HAD IOB sites: {out}");
    assert!(out.contains("featuremap part=HL10T-C32-1"), "{out}");
    assert!(out.contains("BLE0.LUT.INIT[0] minor 0 bit 0"), "{out}");
    assert!(out.contains("BLE0.LUT.FRACTURE minor 4 bit 0"), "{out}");
    assert!(!out.contains("MISSING"), "{out}");
    assert!(
        out.contains("target "),
        "doctor must print the compile-time target triple: {out}"
    );
    assert!(
        out.contains(env!("HELION_TARGET")),
        "doctor target must match this binary's triple {}: {out}",
        env!("HELION_TARGET")
    );
    assert!(
        out.contains("HAD path"),
        "doctor must print the runtime HAD path: {out}"
    );
    assert!(
        out.contains("rustc release:") || out.contains("rustc not"),
        "doctor must report the rustc toolchain: {out}"
    );
    assert!(out.contains("host "), "doctor must report host arch/os: {out}");
    #[cfg(not(target_os = "macos"))]
    {
        assert!(
            !out.contains("aarch64-apple-darwin"),
            "Linux doctor must not claim to be a Mac binary: {out}"
        );
    }

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

    let hnf = Command::new(bin)
        .args(["hnf", counter.to_str().unwrap()])
        .output()
        .expect("hnf");
    assert!(hnf.status.success(), "{}", String::from_utf8_lossy(&hnf.stderr));
    let hs = String::from_utf8_lossy(&hnf.stdout);
    assert!(hs.contains("HNF 1"), "{hs}");
    assert!(hs.contains("cell u_lut"), "{hs}");
}
