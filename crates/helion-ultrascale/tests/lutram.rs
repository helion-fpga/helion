//! LUTRAM derivation regression tests.
//!
//! The claim under test: the LUTRAM content-bit table IS the LUT INIT
//! table (`LutLoc`), and the LUTRAM mode-selection bits live in the
//! vendored prjuray segbits DBs. Evidence, all re-checked here on every
//! run (not trusted from the derivation scripts):
//!
//! 1. BitFiltrator's own extraction code
//!    (`bitfiltrator-ref/src/extract_encoding_from_logic_loc.py`, the
//!    `LutramLoc`/`LutLoc` branch) states the INIT bits "ended up having
//!    the same encoding for LUTRAMs and standard LUTs when we extracted
//!    the data". Its pipeline extracts CLEM/CLEM_R `LutLoc` from Vivado
//!    logic-location files of all-LUTRAM columns (`^LutramLoc$`) and
//!    CLEL_L/CLEL_R `LutLoc` from a LUT INIT fuzzing sweep.
//! 2. The vendored tables are byte-identical across all four tile types
//!    (`lutram_tables_identical_across_tile_types`): the LUTRAM-extracted
//!    table and the LUT-sweep table are the same table.
//! 3. prjuray-db segbits agree bit for bit
//!    (`lutram_segbits_init_matches_lutloc_y0`): every `xLUT.INIT[i]`
//!    line in all four vendored `segbits_cle*.db` files equals the
//!    `LutLoc` Y0 pair.
//! 4. The mode bits (`lutram_mode_bits_resolve_through_segbits`) resolve
//!    to exact BitRefs in the vendored DBs, never collide with any INIT
//!    bit, and do not exist for SLICEL tiles.
//!
//! No em dashes anywhere.

use helion_ultrascale::bitloc::{
    BitLocError, Block, ClbTileType, LutMode, lut_locations, lut_placed_bits, lutram_fasm,
    lutram_slice_fasm,
};
use helion_ultrascale::segbits::{BitRef, FileSegbitsDb, SegbitsDb};
use std::collections::HashSet;
use std::path::PathBuf;

const TILE_TYPES: [ClbTileType; 4] = [
    ClbTileType::ClelL,
    ClbTileType::ClelR,
    ClbTileType::Clem,
    ClbTileType::ClemR,
];

fn data_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join(name)
}

/// The XOR pattern used as the LUTRAM golden INIT (32 bits set).
const GOLDEN_INIT: u64 = 0x6996_9669_9669_6996;

#[test]
fn lutram_tables_identical_across_tile_types() {
    // Source 2 of the derivation: CLEM/CLEM_R LutLoc was extracted from
    // LUTRAM logic-location data, CLEL_L/CLEL_R from a LUT INIT sweep.
    // If the encodings differed, some pair below would differ. 60 Y
    // rows x 8 LUTs x 64 bits = 30,720 pairs per tile-type pair.
    for (i, a) in TILE_TYPES.iter().enumerate() {
        for b in &TILE_TYPES[i + 1..] {
            let mut differing = 0;
            for y in 0..60 {
                for lut in 0..8 {
                    let la = lut_locations(*a, y, lut).unwrap();
                    let lb = lut_locations(*b, y, lut).unwrap();
                    for bit in 0..64 {
                        if la[bit] != lb[bit] {
                            differing += 1;
                        }
                    }
                }
            }
            assert_eq!(
                differing,
                0,
                "LutLoc {:?} vs {:?}: {differing} differing pairs",
                a.json_key(),
                b.json_key()
            );
        }
    }
}

#[test]
fn lutram_content_bits_match_lut_init_spot_values() {
    // Hand-checked values, read from the source JSON, not from code:
    // Y0 A6LUT bit 0 = (11, 15), bit 1 = (10, 15), bit 4 = (11, 14),
    // bit 63 = (8, 0); H6LUT bit 0 = (11, 47); paper V-C worked example
    // SLICE_X136Y247/A6LUT bit 0 = (11, 351) at Y_ofst 7 (247 mod 60).
    // Same values on the LUTRAM-extracted CLEM table and the
    // LUT-sweep CLEL_L table.
    for tile in [ClbTileType::Clem, ClbTileType::ClelL] {
        let y0 = lut_locations(tile, 0, 0).unwrap();
        assert_eq!((y0[0].minor, y0[0].frame_ofst), (11, 15));
        assert_eq!((y0[1].minor, y0[1].frame_ofst), (10, 15));
        assert_eq!((y0[4].minor, y0[4].frame_ofst), (11, 14));
        assert_eq!((y0[63].minor, y0[63].frame_ofst), (8, 0));
        let h0 = lut_locations(tile, 0, 7).unwrap();
        assert_eq!((h0[0].minor, h0[0].frame_ofst), (11, 47));
        let y7 = lut_locations(tile, 247, 0).unwrap();
        assert_eq!((y7[0].minor, y7[0].frame_ofst), (11, 351));
    }
}

#[test]
fn lutram_placed_bits_share_lut_init_locations() {
    // LUTRAM mode returns the same 64 locations as logic mode, with
    // values from the INIT word, labeled block LUT_INIT: the bits ARE
    // LUT INIT bits, and the harness attributes them there. The
    // LUTRAM-ness lives in the mode lines (block LUTRAM), not here.
    for tile in [ClbTileType::Clem, ClbTileType::ClemR] {
        for (y, lut) in [(0, 0), (7, 3), (30, 7), (59, 5)] {
            let ram = lut_placed_bits(tile, y, lut, LutMode::Lutram, GOLDEN_INIT).unwrap();
            let logic = lut_placed_bits(tile, y, lut, LutMode::Logic, GOLDEN_INIT).unwrap();
            assert_eq!(ram.len(), 64);
            for (r, l) in ram.iter().zip(logic.iter()) {
                assert_eq!((r.minor, r.frame_ofst), (l.minor, l.frame_ofst));
                assert_eq!(r.value, l.value);
                assert_eq!(r.block, Block::LutInit);
            }
            // Values follow the INIT word, bit 0 = LSB.
            assert_eq!(ram[0].value, GOLDEN_INIT & 1 == 1);
            assert_eq!(ram[63].value, GOLDEN_INIT >> 63 & 1 == 1);
        }
    }
    assert_eq!(Block::Lutram.name(), "LUTRAM");
}

#[test]
fn lutram_on_slicel_fails_loud() {
    // LUTRAMs live on SLICEM only. A LUTRAM request on a SLICEL tile
    // fails loud naming the tile; it never silently emits logic-LUT
    // bits for a RAM.
    for tile in [ClbTileType::ClelL, ClbTileType::ClelR] {
        assert_eq!(
            lut_placed_bits(tile, 0, 0, LutMode::Lutram, GOLDEN_INIT),
            Err(BitLocError::LutramRequiresSlicem(tile))
        );
    }
}

#[test]
fn lutram_fasm_lines_ram64x1s() {
    // Exact FASM for one RAM64X1S on SLICE_X1Y0 ALUT (the golden
    // placement: xcku3p row 0, CLB column 1, tile type CLEM).
    let lines = lutram_fasm(1, 0, 0, GOLDEN_INIT).unwrap();
    assert_eq!(
        lines,
        vec![
            "SLICE_X1Y0.A6LUT.INIT[63:0] = 64'h6996966996696996".to_string(),
            "SLICE_X1Y0.ALUT.MODE.RAM".to_string(),
            "SLICE_X1Y0.ALUT.DI1.DI1".to_string(),
            "SLICE_X1Y0.ALUT.SIZE.LARGE".to_string(),
        ]
    );
    assert_eq!(
        lutram_slice_fasm(1, 0),
        vec![
            "SLICE_X1Y0.WA7USED.V0".to_string(),
            "SLICE_X1Y0.WA8USED.V0".to_string(),
            "SLICE_X1Y0.WA9USED.V0".to_string(),
        ]
    );
}

#[test]
fn lutram_fasm_h_lut_fails_loud() {
    // The H LUT has no DI1 feature in the vendored DB (A..G only), so
    // its data-input path is unverified and fails loud instead of
    // guessing a DI source.
    assert_eq!(
        lutram_fasm(1, 0, 7, GOLDEN_INIT),
        Err(BitLocError::LutramNoDataInput(7))
    );
}

#[test]
fn lutram_mode_bits_resolve_through_segbits() {
    // Source 4 of the derivation: exact BitRefs, read from the vendored
    // DB lines (not from code). ALUT.MODE.RAM sets (9, 19) and clears
    // (11, 18); the LUT/SRL alternatives are the other combinations of
    // the same two bits, so exactly one mode holds at a time.
    let paths = [
        data_path("segbits_clem.db"),
        data_path("segbits_clem_r.db"),
    ];
    let refs: Vec<&std::path::Path> = paths.iter().map(PathBuf::as_path).collect();
    let db = FileSegbitsDb::from_files(&refs).unwrap();
    for tile in ["CLEM", "CLEM_R"] {
        let mode_ram = db.lookup(tile, "ALUT.MODE.RAM").unwrap();
        assert_eq!(
            mode_ram,
            &[
                BitRef { frame: 9, bit: 19, must_be_zero: false },
                BitRef { frame: 11, bit: 18, must_be_zero: true },
            ]
        );
        assert_eq!(
            db.lookup(tile, "ALUT.MODE.LUT").unwrap(),
            &[
                BitRef { frame: 9, bit: 19, must_be_zero: true },
                BitRef { frame: 11, bit: 18, must_be_zero: true },
            ]
        );
        assert_eq!(
            db.lookup(tile, "ALUT.MODE.SRL").unwrap(),
            &[
                BitRef { frame: 9, bit: 19, must_be_zero: true },
                BitRef { frame: 11, bit: 18, must_be_zero: false },
            ]
        );
        assert_eq!(
            db.lookup(tile, "ALUT.DI1.DI1").unwrap(),
            &[BitRef { frame: 10, bit: 19, must_be_zero: false }]
        );
        assert_eq!(
            db.lookup(tile, "ALUT.DI1.SIN").unwrap(),
            &[BitRef { frame: 10, bit: 19, must_be_zero: true }]
        );
        assert_eq!(
            db.lookup(tile, "ALUT.SIZE.LARGE").unwrap(),
            &[BitRef { frame: 8, bit: 18, must_be_zero: true }]
        );
        assert_eq!(
            db.lookup(tile, "ALUT.SIZE.SMALL").unwrap(),
            &[BitRef { frame: 8, bit: 18, must_be_zero: false }]
        );
        assert_eq!(
            db.lookup(tile, "WA7USED.V0").unwrap(),
            &[BitRef { frame: 8, bit: 23, must_be_zero: true }]
        );
        assert_eq!(
            db.lookup(tile, "WA7USED.V1").unwrap(),
            &[BitRef { frame: 8, bit: 23, must_be_zero: false }]
        );
        assert_eq!(
            db.lookup(tile, "WA8USED.V0").unwrap(),
            &[BitRef { frame: 11, bit: 22, must_be_zero: true }]
        );
        assert_eq!(
            db.lookup(tile, "WA9USED.V0").unwrap(),
            &[BitRef { frame: 9, bit: 22, must_be_zero: true }]
        );
        // Every LUT letter carries MODE.RAM; DI1 exists for A..G only.
        for letter in 'A'..='H' {
            let feat = format!("{letter}LUT.MODE.RAM");
            assert!(
                db.lookup(tile, &feat).is_some(),
                "{tile}.{feat} missing"
            );
        }
        for letter in 'A'..='G' {
            assert!(db.lookup(tile, &format!("{letter}LUT.DI1.DI1")).is_some());
        }
        assert!(db.lookup(tile, "HLUT.DI1.DI1").is_none());
        // No mode bit collides with any INIT bit: the mode selection
        // cannot change a content bit, and vice versa.
        let mut init_bits: HashSet<(u32, u16)> = HashSet::new();
        for letter in 'A'..='H' {
            for i in 0..64 {
                let refs = db.lookup(tile, &format!("{letter}LUT.INIT[{i}]")).unwrap();
                assert_eq!(refs.len(), 1);
                init_bits.insert((refs[0].frame, refs[0].bit));
            }
        }
        assert_eq!(init_bits.len(), 512);
        for feat in [
            "ALUT.MODE.RAM",
            "ALUT.MODE.LUT",
            "ALUT.MODE.SRL",
            "ALUT.DI1.DI1",
            "ALUT.SIZE.SMALL",
            "WA7USED.V1",
            "WA8USED.V1",
            "WA9USED.V1",
        ] {
            for r in db.lookup(tile, feat).unwrap() {
                assert!(
                    !init_bits.contains(&(r.frame, r.bit)),
                    "{tile}.{feat} bit ({}, {}) collides with an INIT bit",
                    r.frame,
                    r.bit
                );
            }
        }
    }
    // SLICEL tiles carry no RAM mode at all: a LUTRAM there is
    // unrepresentable, matching the LutramRequiresSlicem gate.
    let clel_paths = [
        data_path("segbits_clel_l.db"),
        data_path("segbits_clel_r.db"),
    ];
    let clel_refs: Vec<&std::path::Path> = clel_paths.iter().map(PathBuf::as_path).collect();
    let clel = FileSegbitsDb::from_files(&clel_refs).unwrap();
    for tile in ["CLEL_L", "CLEL_R"] {
        assert!(clel.lookup(tile, "ALUT.MODE.RAM").is_none());
        assert!(clel.lookup(tile, "WA7USED.V0").is_none());
    }
}

#[test]
fn lutram_segbits_init_matches_lutloc_y0() {
    // Source 3 of the derivation: prjuray `xLUT.INIT[i]` equals the
    // BitFiltrator `LutLoc` Y0 pair, all 64 bits, all 8 LUTs, all four
    // tile types (512 of 512 per type).
    let files = [
        ("CLEL_L", "segbits_clel_l.db"),
        ("CLEL_R", "segbits_clel_r.db"),
        ("CLEM", "segbits_clem.db"),
        ("CLEM_R", "segbits_clem_r.db"),
    ];
    for (tile_name, file) in files {
        let db = FileSegbitsDb::from_file(&data_path(file)).unwrap();
        let tile: ClbTileType = tile_name.parse().unwrap();
        let mut checked = 0;
        for lut in 0..8 {
            let letter = (b'A' + lut) as char;
            let locs = lut_locations(tile, 0, lut).unwrap();
            for bit in 0..64 {
                let refs = db
                    .lookup(tile_name, &format!("{letter}LUT.INIT[{bit}]"))
                    .unwrap_or_else(|| panic!("{tile_name}.{letter}LUT.INIT[{bit}] missing"));
                assert_eq!(refs.len(), 1);
                assert!(!refs[0].must_be_zero);
                assert_eq!(refs[0].frame, locs[bit].minor, "minor mismatch");
                assert_eq!(refs[0].bit as u32, locs[bit].frame_ofst, "offset mismatch");
                checked += 1;
            }
        }
        assert_eq!(checked, 512);
    }
}
