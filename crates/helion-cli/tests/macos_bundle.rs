//! Linux-verifiable Helion.app layout. Does not claim to run on Apple Silicon.

use std::process::Command;

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn macos_app_layout_bundles_had_and_info_plist() {
    let root = root();
    let script = root.join("scripts/build-macos-app.sh");
    assert!(script.is_file(), "missing {}", script.display());
    let out = root.join("target/helion-app-layout-test");
    let _ = std::fs::remove_dir_all(&out);
    let status = Command::new("bash")
        .args([
            script.to_str().unwrap(),
            "--layout-only",
            "--out",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("run build-macos-app.sh");
    assert!(status.success(), "layout-only failed");

    let app = out.join("Helion.app");
    let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
    assert!(plist.contains("fpga.helion.ide"), "{plist}");
    assert!(plist.contains("CFBundleExecutable"), "{plist}");
    assert!(plist.contains("Helion"), "{plist}");
    assert!(
        plist.contains("LSMinimumSystemVersion"),
        "min macOS version: {plist}"
    );

    let had = app.join("Contents/Resources/devices/helion/parts/HL10T-C32-1.toml");
    assert!(had.is_file(), "HAD not bundled at {}", had.display());
    let toml = std::fs::read_to_string(&had).unwrap();
    assert!(toml.contains("HL10T-C32-1"), "{toml}");

    assert!(app.join("Contents/Resources/AppIcon.png").is_file());
    assert!(app.join("Contents/MacOS/Helion").is_file());
    assert!(app.join("Contents/MacOS/helion-ide").is_file());
    assert!(app.join("Contents/MacOS/helion").is_file());
    assert!(
        app.join("Contents/Resources/examples/counter.sv").is_file(),
        "examples must be bundled next to HAD"
    );

    // Runtime search used by Device::devices_dir: MacOS/../Resources/devices/helion
    let macos = app.join("Contents/MacOS");
    let rel = macos.join("../Resources/devices/helion");
    assert!(
        rel.join("parts/HL10T-C32-1.toml").is_file(),
        "exe-relative HAD path broken: {}",
        rel.display()
    );
}
