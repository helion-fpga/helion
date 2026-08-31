//! Packaging smoke: the Mac .app layout is real files, not a README promise.
//! This Linux VM cannot produce aarch64-apple-darwin, so the test runs the
//! script with HELION_SKIP_BUILD=1 and asserts the bundle *shape*.

use std::process::Command;

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn build_script_documents_apple_silicon_release() {
    let sh = std::fs::read_to_string(root().join("scripts/build-macos-app.sh")).unwrap();
    assert!(
        sh.contains("cargo build --release --target"),
        "script must invoke cargo --release --target"
    );
    assert!(
        sh.contains("aarch64-apple-darwin"),
        "script must target Apple Silicon: {sh}"
    );
    assert!(!sh.contains("docker"), "no Docker: {sh}");
    assert!(!sh.contains("rosetta") || sh.contains("no Rosetta"), "{sh}");
    let plist = std::fs::read_to_string(root().join("packaging/macos/Info.plist")).unwrap();
    assert!(plist.contains("fpga.helion.ide"), "{plist}");
    assert!(plist.contains("<string>Helion</string>"), "{plist}");
    assert!(plist.contains("LSRequiresNativeExecution"), "{plist}");
    assert!(plist.contains("arm64"), "{plist}");
    assert!(!plist.contains("x86_64"), "no Rosetta slice: {plist}");
    let icon = root().join("packaging/macos/AppIcon.png");
    let bytes = std::fs::read(&icon).unwrap();
    assert!(bytes.starts_with(b"\x89PNG"), "icon placeholder must be a PNG");
    assert!(bytes.len() > 100, "icon too small: {}", bytes.len());
}

#[test]
fn skip_build_assembles_helion_app_layout() {
    let root = root();
    let ide = env!("CARGO_BIN_EXE_helion-ide");
    let bin_dir = std::path::Path::new(ide).parent().unwrap();
    let dist = root.join("target/helion-app-test");
    let _ = std::fs::remove_dir_all(&dist);
    let out = Command::new("sh")
        .arg(root.join("scripts/build-macos-app.sh"))
        .env("HELION_SKIP_BUILD", "1")
        .env("HELION_BIN_DIR", bin_dir)
        .env("HELION_DIST", &dist)
        .output()
        .expect("run build-macos-app.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "script failed: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("NOT a Mac")
            || stdout.contains("not a Mac")
            || stdout.contains("WITHOUT an aarch64-apple-darwin"),
        "skip-build must refuse to pose as a Mac app: {stdout}"
    );

    let app = dist.join("Helion.app");
    assert!(app.join("Contents/Info.plist").is_file(), "missing Info.plist");
    assert!(app.join("Contents/MacOS/Helion").is_file(), "missing GUI binary");
    assert!(
        app.join("Contents/Resources/AppIcon.png").is_file(),
        "missing icon placeholder"
    );
    assert!(
        app.join("Contents/Resources/devices/helion/parts/HL10T-C32-1.toml")
            .is_file(),
        "HAD must be inside the bundle so Device::devices_dir finds it"
    );
    assert!(
        app.join("Contents/Resources/examples/counter.sv").is_file(),
        "examples must ship in Resources"
    );
    let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
    assert!(plist.contains("CFBundleExecutable"));
    assert!(plist.contains("Helion"));

    // Headless flags still work on the copied Linux binary (not a Mac binary).
    let v = Command::new(app.join("Contents/MacOS/Helion"))
        .arg("--version")
        .output()
        .expect("bundled --version");
    assert!(v.status.success(), "{}", String::from_utf8_lossy(&v.stderr));
    let vs = String::from_utf8_lossy(&v.stdout);
    assert!(vs.contains("helion-ide"), "{vs}");
}
