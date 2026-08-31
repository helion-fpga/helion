//! QoR gate: the numbers published in README.md are asserted here.
//! A change to examples/counter.sv LUT count or WNS fails until this table
//! is updated in the same commit (with a comment explaining the move).

use std::process::Command;

/// (design, LUTFF, WNS_PS) — must match the README QoR table.
const GOLD: &[(&str, u32, i64)] = &[
    ("examples/blinky.sv", 1, 9700),
    ("examples/counter.sv", 4, 9640),
    ("examples/hier.sv", 1, 9700),
];

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn field(out: &str, key: &str) -> String {
    out.split_whitespace()
        .find_map(|t| t.strip_prefix(key).map(|v| v.to_string()))
        .unwrap_or_else(|| panic!("no {key} in {out}"))
}

#[test]
fn qor_table_matches_readme() {
    let bin = env!("CARGO_BIN_EXE_helion");
    let root = root();
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    for (src, luts, wns) in GOLD {
        let path = root.join(src);
        let u = Command::new(bin)
            .args(["report_utilization", path.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(u.status.success(), "{}", String::from_utf8_lossy(&u.stderr));
        let uo = String::from_utf8_lossy(&u.stdout);
        let lutff = field(&uo, "LUTFF=");
        let got_luts: u32 = lutff.split('/').next().unwrap().parse().unwrap();
        assert_eq!(
            got_luts, *luts,
            "{src} LUTFF moved {luts} -> {got_luts}; update the README QoR table \
             in this commit and say why"
        );

        let t = Command::new(bin)
            .args(["report_timing", path.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(t.status.success(), "{}", String::from_utf8_lossy(&t.stderr));
        let to = String::from_utf8_lossy(&t.stdout);
        let got_wns: i64 = field(&to, "WNS_PS=").parse().unwrap();
        assert_eq!(
            got_wns, *wns,
            "{src} WNS_PS moved {wns} -> {got_wns}; update the README QoR table \
             in this commit and say why"
        );

        // README must publish the same numbers.
        let name = src.rsplit('/').next().unwrap();
        let row = readme
            .lines()
            .find(|l| l.contains(name) && l.starts_with('|'))
            .unwrap_or_else(|| panic!("README QoR table has no row for {name}"));
        assert!(
            row.contains(&format!(" {luts} ")),
            "README row for {name} must publish LUTFF {luts}: {row}"
        );
        assert!(
            row.contains(&wns.to_string()),
            "README row for {name} must publish WNS_PS {wns}: {row}"
        );
    }

    // counter LED waveform is the 1.0 gold and is part of QoR.
    let run = Command::new(bin)
        .args([
            "run",
            root.join("examples/counter.sv").to_str().unwrap(),
            "--cycles",
            "16",
        ])
        .output()
        .unwrap();
    let ro = String::from_utf8_lossy(&run.stdout);
    assert!(ro.contains("0000000111111110"), "counter LED gold: {ro}");
    assert!(readme.contains("0000000111111110"), "README must publish the gold waveform");
}
