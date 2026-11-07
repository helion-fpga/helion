//! INT PIP completeness audit, part 1 (DB-internal).
//!
//! Enumerates every PIP of one INT tile from the vendored
//! `tile_type_INT.json` and checks it against the vendored
//! `segbits_int.db` through the public `SegbitsDb` API (the ported
//! `FileSegbitsDb`). Full numbers and the pending part 2 (PinPlanning
//! probe comparison) are recorded in
//! `crates/helion-ultrascale/data/INT_PIP_AUDIT.md`.
//!
//! Mapping, verified by this test: a tile pip with `src_wire` S and
//! `dst_wire` D maps to the segbits feature `INT.PIP.D.S`. The 4
//! bidirectional PIPs (`is_directional` = "0") have no bare entry;
//! each is covered by a `.FWD` / `.REV` pair instead.
//!
//! The "nothing unexplained" side is proven by counting, not by
//! enumerating DB features: every expected feature resolves, the
//! expected set has 3778 entries (3770 directional plus 4 x 2
//! FWD/REV), and the DB holds exactly 3778 features, so the two sets
//! are equal.

use helion_ultrascale::{FileSegbitsDb, SegbitsDb};
use std::collections::HashSet;
use std::path::PathBuf;

const TILE_TYPE_INT_JSON: &str = include_str!("../data/tile_type_INT.json");

fn data_file(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join(name)
}

#[test]
fn int_pip_audit_db_internal() {
    let db = FileSegbitsDb::from_file(&data_file("segbits_int.db")).unwrap();
    assert_eq!(db.feature_count(), 3778, "vendored INT feature count");
    assert_eq!(db.tile_types(), &["INT".to_string()]);

    let tile: serde_json::Value =
        serde_json::from_str(TILE_TYPE_INT_JSON).expect("tile_type_INT.json must parse");
    let pips = tile["pips"]
        .as_object()
        .expect("tile_type_INT.json must have a pips object");

    // 3774 tile PIPs, per the audit record.
    assert_eq!(pips.len(), 3774, "tile_type_INT.json pip count");

    let mut directional = 0usize;
    let mut bidirectional = 0usize;
    let mut expected: HashSet<String> = HashSet::new();
    for (_key, pip) in pips {
        let src = pip["src_wire"].as_str().expect("pip needs src_wire");
        let dst = pip["dst_wire"].as_str().expect("pip needs dst_wire");
        let base = format!("PIP.{dst}.{src}");
        match pip["is_directional"].as_str() {
            Some("1") => {
                directional += 1;
                assert!(
                    db.lookup("INT", &base).is_some(),
                    "directional tile PIP {base} has no segbits entry"
                );
                expected.insert(base);
            }
            Some("0") => {
                bidirectional += 1;
                // The bare base feature must NOT exist; only .FWD/.REV.
                assert!(
                    db.lookup("INT", &base).is_none(),
                    "bidirectional tile PIP {base} must not have a bare entry"
                );
                for suffix in [".FWD", ".REV"] {
                    let directed = format!("{base}{suffix}");
                    assert!(
                        db.lookup("INT", &directed).is_some(),
                        "bidirectional tile PIP {directed} has no segbits entry"
                    );
                    expected.insert(directed);
                }
            }
            other => panic!("unexpected is_directional value: {other:?}"),
        }
    }

    assert_eq!(directional, 3770, "directional tile PIP count");
    assert_eq!(bidirectional, 4, "bidirectional tile PIP count");
    // 3770 + 4 x 2 = 3778 expected features, exactly the DB size, and
    // every one resolved above: the vendored set equals the tile
    // enumeration, with nothing unexplained on either side.
    assert_eq!(expected.len(), 3778, "expected INT feature count");
}
