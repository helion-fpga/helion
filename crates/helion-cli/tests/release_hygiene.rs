//! Release / stranger-clone gates (W-L6). Do not take helion-sv or examples/.

use std::process::Command;

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn cargo_workspace_version() -> String {
    let toml = std::fs::read_to_string(root().join("Cargo.toml")).unwrap();
    toml.lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("version = \"")
                .and_then(|r| r.strip_suffix('"'))
                .map(|s| s.to_string())
        })
        .expect("workspace.package.version in Cargo.toml")
}

#[test]
fn plist_version_matches_cargo_toml() {
    let ver = cargo_workspace_version();
    assert_eq!(ver, "2.0.1", "bump this test when the workspace version moves");
    let plist = std::fs::read_to_string(root().join("packaging/macos/Info.plist")).unwrap();
    for key in ["CFBundleShortVersionString", "CFBundleVersion"] {
        let needle = format!("<key>{key}</key>");
        let idx = plist.find(&needle).unwrap_or_else(|| panic!("missing {key}"));
        let after = &plist[idx + needle.len()..];
        assert!(
            after.contains(&format!("<string>{ver}</string>")),
            "{key} must be {ver}: {plist}"
        );
    }
}

#[test]
fn readme_stranger_path_is_clone_test_headless_gold() {
    let readme = std::fs::read_to_string(root().join("README.md")).unwrap();
    assert!(
        readme.contains("2.0.1"),
        "README must name the current release, not a stale 1.3 line"
    );
    assert!(
        !readme.contains("current release **1.3**"),
        "README still claims current release 1.3"
    );
    for needle in [
        "git clone https://github.com/helion-fpga/helion.git",
        "cargo test --workspace",
        "helion-ide -- --headless examples/counter.sv",
        "WNS_PS=9640",
        "SOFT",
        "docs/stages.md",
    ] {
        assert!(readme.contains(needle), "README stranger path missing {needle}");
    }
}

#[test]
fn helion_ide_version_test_tracks_cargo_pkg_version() {
    let src = std::fs::read_to_string(root().join("crates/helion-gui/tests/ide_bin.rs")).unwrap();
    assert!(
        src.contains("CARGO_PKG_VERSION"),
        "helion-ide --version must follow workspace.package.version, not a stale 1.0.0"
    );
    assert!(
        !src.contains("helion-ide 1.0.0"),
        "stale helion-ide 1.0.0 pin would fail cargo test --workspace"
    );
}

#[test]
fn release_workflow_gates_gold_and_size() {
    let yml = std::fs::read_to_string(root().join(".github/workflows/release.yml")).unwrap();
    assert!(
        yml.contains("WNS_PS=9640"),
        "release must re-check gold before packing"
    );
    assert!(
        yml.contains("check-artifact-size.sh"),
        "release must measure artifact sizes"
    );
    assert!(
        yml.contains("CARGO_PROFILE_RELEASE_STRIP"),
        "release must strip so macOS/Linux assets stay small"
    );
    assert!(
        yml.contains("Info.plist"),
        "release must refuse a plist/Cargo version skew"
    );
}

#[test]
fn pack_unix_release_omits_ip_ingest_and_fits_size_gate() {
    let root = root();
    let script = root.join("scripts/pack-unix-release.sh");
    let size_sh = root.join("packaging/check-artifact-size.sh");
    assert!(script.is_file());
    assert!(size_sh.is_file());

    let tmp = root.join("target/helion-pack-hygiene");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("bin")).unwrap();
    std::fs::write(
        tmp.join("bin/helion"),
        "#!/bin/sh\necho helion-layout-stub\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(tmp.join("bin/helion"))
            .unwrap()
            .permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(tmp.join("bin/helion"), p).unwrap();
    }

    let out = Command::new("sh")
        .arg(&script)
        .args(["0.0.0-test", "x86_64-unknown-linux-gnu"])
        .arg(tmp.join("bin"))
        .arg(tmp.join("out"))
        .output()
        .expect("pack-unix-release.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "pack failed: stdout={stdout} stderr={stderr}"
    );
    let tgz = tmp
        .join("out")
        .join("helion-0.0.0-test-x86_64-unknown-linux-gnu.tar.gz");
    assert!(tgz.is_file(), "missing tarball {stdout}");

    let listing = Command::new("tar")
        .args(["-tzf", tgz.to_str().unwrap()])
        .output()
        .expect("tar tzf");
    let names = String::from_utf8_lossy(&listing.stdout);
    assert!(
        names.contains("examples/counter.sv"),
        "tarball must ship gold counter: {names}"
    );
    assert!(
        !names.contains("ip_ingest"),
        "tarball must omit examples/ip_ingest: {names}"
    );

    let sized = Command::new("sh")
        .arg(&size_sh)
        .arg(&tgz)
        .output()
        .expect("check-artifact-size.sh");
    let st = format!(
        "{}{}",
        String::from_utf8_lossy(&sized.stdout),
        String::from_utf8_lossy(&sized.stderr)
    );
    assert!(sized.status.success(), "size gate failed on tiny stub pack: {st}");
    assert!(st.contains("artifact"), "{st}");
}
