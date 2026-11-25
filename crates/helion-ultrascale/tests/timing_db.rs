//! Timing DB regression tests (GAP-F5).
//!
//! Every delay value is pinned to its source. The preliminary DBs are
//! ballpark estimates from the AMD forum ~0.3 ns per logic level split
//! (120 ps LUT + 180 ps one INT hop); the documented DBs carry exactly
//! one datasheet value each (BRAM CLK to DOUT, DS922/DS923 Table 33
//! TRCKO_DO, 1.02 ns at -2 / 0.85V as cited, and DS925 v1.30 Table 80
//! TRCKO_DO, 1.02 ns at -2 / 0.85V for xczu3eg) and UNKNOWN everywhere
//! else.

use helion_ultrascale::timing_db::{
    documented_xcku3p, documented_xcvu3p, documented_xczu3eg, preliminary_xcku3p,
    preliminary_xcvu3p, preliminary_xczu3eg, sta_db_for_part,
};
use helion_ultrascale::timing_db::{DelaySource, SourcedDelay, UltraScaleTimingDb};

fn assert_ballpark(d: &SourcedDelay, ps: i64) {
    assert_eq!(d.ps, Some(ps), "delay {d:?} must be {ps} ps");
    assert!(
        matches!(d.source, DelaySource::Ballpark(_)),
        "delay {d:?} must be Ballpark-sourced"
    );
    if let DelaySource::Ballpark(src) = &d.source {
        assert!(
            src.contains("0.3ns"),
            "ballpark source must name the forum 0.3 ns level: {src}"
        );
        assert!(
            src.contains("assumed allocation"),
            "ballpark source must say the value is an assumed allocation, not a forum-stated value: {src}"
        );
    }
}

fn assert_unknown(d: &SourcedDelay) {
    assert_eq!(d.ps, None, "delay {d:?} must be UNKNOWN");
    assert_eq!(d.source, DelaySource::Unknown);
}

/// All 20 delay fields of a DB, in struct order, for exhaustive checks.
fn all_fields(db: &UltraScaleTimingDb) -> Vec<(&'static str, &SourcedDelay)> {
    vec![
        ("cells.lut6", &db.cells.lut6),
        ("cells.ff_ckq", &db.cells.ff_ckq),
        ("cells.ff_setup", &db.cells.ff_setup),
        ("cells.ff_hold", &db.cells.ff_hold),
        ("carry8.ci_to_co_bit", &db.carry8.ci_to_co_bit),
        ("carry8.s_to_o", &db.carry8.s_to_o),
        ("carry8.cyinit_to_co", &db.carry8.cyinit_to_co),
        ("dsp.mult_a_b_to_p", &db.dsp.mult_a_b_to_p),
        ("dsp.clk_to_p", &db.dsp.clk_to_p),
        ("bram.clk_to_do", &db.bram.clk_to_do),
        ("bram.addr_setup", &db.bram.addr_setup),
        ("uram.clk_to_do", &db.uram.clk_to_do),
        ("uram.addr_setup", &db.uram.addr_setup),
        ("interconnect.single", &db.interconnect.single),
        ("interconnect.double", &db.interconnect.double),
        ("interconnect.quad", &db.interconnect.quad),
        ("interconnect.long", &db.interconnect.long),
        ("interconnect.pip", &db.interconnect.pip),
        ("iob.pad_to_fabric", &db.iob.pad_to_fabric),
        ("iob.fabric_to_pad", &db.iob.fabric_to_pad),
    ]
}

fn assert_preliminary_values(db: &UltraScaleTimingDb, part: &str) {
    assert_eq!(db.part, part);
    assert_eq!(db.speed_grade, "-2");
    assert_ballpark(&db.cells.lut6, 120);
    assert_ballpark(&db.cells.ff_ckq, 150);
    assert_ballpark(&db.cells.ff_setup, 80);
    assert_ballpark(&db.cells.ff_hold, 60);
    assert_ballpark(&db.carry8.ci_to_co_bit, 40);
    assert_ballpark(&db.carry8.s_to_o, 100);
    assert_ballpark(&db.carry8.cyinit_to_co, 60);
    assert_unknown(&db.dsp.mult_a_b_to_p);
    assert_unknown(&db.dsp.clk_to_p);
    assert_unknown(&db.bram.clk_to_do);
    assert_unknown(&db.bram.addr_setup);
    assert_unknown(&db.uram.clk_to_do);
    assert_unknown(&db.uram.addr_setup);
    assert_ballpark(&db.interconnect.single, 180);
    assert_unknown(&db.interconnect.double);
    assert_unknown(&db.interconnect.quad);
    assert_unknown(&db.interconnect.long);
    assert_ballpark(&db.interconnect.pip, 50);
    assert_unknown(&db.iob.pad_to_fabric);
    assert_unknown(&db.iob.fabric_to_pad);
    assert!(db.is_estimate(), "preliminary DB must be an estimate");
}

#[test]
fn preliminary_xcku3p_values_pinned_to_forum_ballpark() {
    assert_preliminary_values(&preliminary_xcku3p(), "xcku3p");
}

#[test]
fn preliminary_xcvu3p_values_pinned_to_forum_ballpark() {
    assert_preliminary_values(&preliminary_xcvu3p(), "xcvu3p");
}

#[test]
fn preliminary_xczu3eg_values_pinned_to_forum_ballpark() {
    assert_preliminary_values(&preliminary_xczu3eg(), "xczu3eg");
}

#[test]
fn preliminary_xczu3eg_differs_only_by_part() {
    let zu = preliminary_xczu3eg();
    let ku = preliminary_xcku3p();
    let vu = preliminary_xcvu3p();
    assert_eq!(zu.cells.lut6, ku.cells.lut6);
    assert_eq!(zu.cells.lut6, vu.cells.lut6);
    assert_eq!(zu.interconnect.single, ku.interconnect.single);
    assert_eq!(zu.interconnect.single, vu.interconnect.single);
    assert_ne!(zu.part, ku.part);
    assert_ne!(zu.part, vu.part);
}

#[test]
fn preliminary_dbs_differ_only_by_part() {
    let ku = preliminary_xcku3p();
    let vu = preliminary_xcvu3p();
    assert_eq!(ku.cells.lut6, vu.cells.lut6);
    assert_eq!(ku.interconnect.single, vu.interconnect.single);
    assert_ne!(ku.part, vu.part);
}

#[test]
fn documented_xcku3p_has_only_bram_clk_to_do() {
    let db = documented_xcku3p();
    assert_eq!(db.part, "xcku3p");
    assert_eq!(db.speed_grade, "-2");
    assert_eq!(db.bram.clk_to_do.ps, Some(1020));
    assert_eq!(
        db.bram.clk_to_do.source,
        DelaySource::Documented("DS922 Table 33 TRCKO_DO, -2 0.85V".to_string()),
        "BRAM delay must cite DS922 Table 33 exactly"
    );
    for (name, d) in all_fields(&db) {
        if name != "bram.clk_to_do" {
            assert_unknown(d);
        }
    }
    assert!(!db.is_estimate(), "documented DB has no ballpark value");
}

#[test]
fn documented_xcvu3p_has_only_bram_clk_to_do() {
    let db = documented_xcvu3p();
    assert_eq!(db.part, "xcvu3p");
    assert_eq!(db.bram.clk_to_do.ps, Some(1020));
    assert_eq!(
        db.bram.clk_to_do.source,
        DelaySource::Documented("DS923 Table 33 TRCKO_DO, -2 0.85V".to_string()),
        "BRAM delay must cite DS923 Table 33 exactly"
    );
    for (name, d) in all_fields(&db) {
        if name != "bram.clk_to_do" {
            assert_unknown(d);
        }
    }
}

/// xczu3eg documented DB: exactly one value, BRAM CLK to DOUT 1020 ps
/// from DS925 v1.30 Table 80 TRCKO_DO at -2 / 0.85V (1.02 ns, Max,
/// without output register). The -2 at 0.85V column is the second data
/// column; -2 at 0.72V is 1.46 ns and must not be confused with it
/// (see nit N64 in the DS925 source audit). Every other field is
/// UNKNOWN, and the DB carries no ballpark value, so it is not an
/// estimate and stays deliberately unusable for STA.
#[test]
fn documented_xczu3eg_has_only_bram_clk_to_do() {
    let db = documented_xczu3eg();
    assert_eq!(db.part, "xczu3eg");
    assert_eq!(db.speed_grade, "-2");
    assert_eq!(db.bram.clk_to_do.ps, Some(1020));
    assert_eq!(
        db.bram.clk_to_do.source,
        DelaySource::Documented("DS925 v1.30 Table 80 TRCKO_DO, -2 0.85V".to_string()),
        "BRAM delay must cite DS925 v1.30 Table 80 exactly"
    );
    for (name, d) in all_fields(&db) {
        if name != "bram.clk_to_do" {
            assert_unknown(d);
        }
    }
    assert!(!db.is_estimate(), "documented DB has no ballpark value");
}

#[test]
fn sta_db_for_part_selects_per_part() {
    assert_eq!(sta_db_for_part("xcku3p").unwrap().part, "xcku3p");
    assert_eq!(sta_db_for_part("xcvu3p").unwrap().part, "xcvu3p");
    assert_eq!(sta_db_for_part("xczu3eg").unwrap().part, "xczu3eg");
    assert!(sta_db_for_part("xcku3p").unwrap().is_estimate());
    assert!(sta_db_for_part("xczu3eg").unwrap().is_estimate());
    assert_preliminary_values(&sta_db_for_part("xczu3eg").unwrap(), "xczu3eg");
}

#[test]
fn sta_db_for_part_rejects_unknown_part() {
    let err = sta_db_for_part("xc7a35t").expect_err("must fail loud");
    assert!(err.contains("xc7a35t"), "err: {err}");
    assert!(err.contains("unsupported part"), "err: {err}");
}

/// Path of the xczu3eg timing source audit, relative to this crate.
fn xczu3eg_timing_audit_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs")
        .join("xczu3eg_timing_sources.md")
}

/// The xtiming lane outcome is now A-partial (one documented BRAM
/// value, the rest still blocked), proven by a committed audit: the
/// audit records the DS925 read and the remaining Vivado unblocker.
/// This test pins the audit file itself: it must exist and must name
/// the document searched (DS925), the one value that exists
/// (TRCKO_DO), the prior verdict (BLOCKED), and the named unblocker
/// (Vivado report_timing). Fail-before: on the base code the file
/// does not exist and this test fails at the read.
#[test]
fn xczu3eg_timing_audit_present_and_names_unblocker() {
    let path = xczu3eg_timing_audit_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("xczu3eg timing audit missing at {}: {e}", path.display()));
    for marker in ["DS925", "TRCKO_DO", "BLOCKED", "report_timing", "write_sdf"] {
        assert!(
            text.contains(marker),
            "audit {} must name {marker}",
            path.display()
        );
    }
}

/// Guard against an invented documented value: the preliminary
/// xczu3eg DB must carry no Documented delay at all. Every field is
/// Ballpark or Unknown, and the DB stays an estimate. This pins the
/// current state rather than a change, so it passes on the base too;
/// its value is regression protection, and the audit-presence test
/// above is the one with fail-before proof.
#[test]
fn preliminary_xczu3eg_has_no_documented_value() {
    let db = preliminary_xczu3eg();
    for (name, d) in all_fields(&db) {
        assert!(
            !matches!(d.source, DelaySource::Documented(_)),
            "preliminary xczu3eg field {name} must not be Documented: {d:?}"
        );
    }
    assert!(
        db.is_estimate(),
        "preliminary xczu3eg DB must be an estimate"
    );
}
