//! Loader tests for the vendored segbits DBs (public API).
//!
//! These exercise `FileSegbitsDb` (ported from the int-pip worker,
//! commit 6bb30b6 on fm/ultra-bitgen-int) against the vendored repo
//! data in `crates/helion-ultrascale/data/`. Paths come from
//! `CARGO_MANIFEST_DIR`, never from `/tmp`.
//!
//! NOTE: only the INT and CMT DBs are loaded whole here. The HPIO and
//! HDIO DBs contain bit offsets above 255 (max 1390), which the
//! ported loader rejects under its u8 bit type; loading them whole is
//! covered by the u16 widening commit, which adds its regression test.

use helion_ultrascale::{BitRef, FileSegbitsDb, SegbitsDb};
use std::path::PathBuf;

fn data_file(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join(name)
}

#[test]
fn day_one_example_pip_resolves_through_public_api() {
    let db = FileSegbitsDb::from_file(&data_file("segbits_int.db")).unwrap();
    let bits = db
        .lookup("INT", "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1")
        .expect("day-1 example PIP must resolve");
    assert_eq!(
        bits,
        &[
            BitRef::set(64, 0),
            BitRef::zero(66, 0),
            BitRef::zero(68, 0),
            BitRef::zero(72, 0),
            BitRef::zero(72, 1),
            BitRef::zero(74, 0),
        ]
    );
    assert_eq!(db.feature_count(), 3778);
    assert_eq!(db.tile_types(), &["INT".to_string()]);
}

#[test]
fn cmt_features_resolve() {
    let db = FileSegbitsDb::from_file(&data_file("segbits_cmt_right.db")).unwrap();
    assert!(db
        .lookup("CMT_RIGHT", "PIP.CLK_HROUTE_1_5.VCC_WIRE")
        .is_some());
    assert_eq!(db.feature_count(), 104);
}

#[test]
fn unknown_lookups_fail_loud_with_none() {
    let db = FileSegbitsDb::from_file(&data_file("segbits_int.db")).unwrap();
    assert!(db.lookup("INT", "PIP.DOES.NOT.EXIST").is_none());
    assert!(db
        .lookup("DSP", "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1")
        .is_none());
    assert!(db.lookup("", "").is_none());
}
