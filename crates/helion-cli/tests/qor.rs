//! QoR gate: the numbers published in README.md are asserted here.
//! A change to examples/counter.sv LUT count or WNS fails until this table
//! is updated in the same commit (with a comment explaining the move).
//! `qor_beats_previous_commit` additionally holds every axis (LUT, WNS,
//! bitstream size, wall time) against the previous Helion commit.

use std::process::Command;

/// (design, LUTFF, WNS_PS) — must match the README QoR table.
const GOLD: &[(&str, u32, i64)] = &[
    ("examples/blinky.sv", 1, 9700),
    ("examples/counter.sv", 4, 9640),
    ("examples/hier.sv", 1, 9700),
];

/// Previous Helion commit (9b864af) per design: (LUTFF, WNS_PS, `.hbits` bytes).
/// A new commit may not lose on any axis; `.hbits` size must strictly improve
/// or be re-baselined here with the README change-log row that explains it.
const PREV: &[(&str, u32, i64, usize)] = &[
    ("examples/blinky.sv", 1, 9700, 272_485),
    ("examples/counter.sv", 4, 9640, 272_485),
    ("examples/hier.sv", 1, 9700, 272_485),
];

/// Whole synth -> bitgen flow budget per design.
const WALL_MS_BUDGET: u128 = 2_000;

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

#[test]
fn qor_beats_previous_commit() {
    let bin = env!("CARGO_BIN_EXE_helion");
    let root = root();
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    for (src, prev_luts, prev_wns, prev_bytes) in PREV {
        let path = root.join(src);
        let out = Command::new(bin)
            .args(["qor", path.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let o = String::from_utf8_lossy(&out.stdout);

        let luts: u32 = field(&o, "LUTFF=").parse().unwrap();
        let wns: i64 = field(&o, "WNS_PS=").parse().unwrap();
        let bytes: usize = field(&o, "BYTES=").parse().unwrap();
        let elapsed: u128 = field(&o, "ELAPSED_MS=").parse().unwrap();

        // The flow must still do real work: an empty bitstream is not a win.
        assert!(luts > 0, "{src} produced no LUTFF: {o}");
        assert!(
            bytes > 64,
            "{src} .hbits is header-only ({bytes} B) — bitgen became a no-op: {o}"
        );

        assert!(
            luts <= *prev_luts,
            "{src} LUTFF regressed {prev_luts} -> {luts}; beat or re-baseline PREV with a README reason"
        );
        assert!(
            wns >= *prev_wns,
            "{src} WNS_PS regressed {prev_wns} -> {wns}; beat or re-baseline PREV with a README reason"
        );
        assert!(
            bytes < *prev_bytes,
            "{src} .hbits size must beat the previous commit ({prev_bytes} B), got {bytes} B"
        );
        assert!(
            elapsed <= WALL_MS_BUDGET,
            "{src} flow took {elapsed} ms, over the {WALL_MS_BUDGET} ms budget"
        );

        // README publishes the size that was just measured.
        let name = src.rsplit('/').next().unwrap();
        let row = readme
            .lines()
            .find(|l| l.contains(name) && l.starts_with('|'))
            .unwrap_or_else(|| panic!("README QoR table has no row for {name}"));
        assert!(
            row.contains(&format!(" {bytes} ")),
            "README row for {name} must publish .hbits {bytes} B: {row}"
        );
    }
    assert!(
        readme.contains("272485 B"),
        "README change log must record the previous-commit bitstream size"
    );
}
