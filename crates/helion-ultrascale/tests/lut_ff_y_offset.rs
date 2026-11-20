//! F1 regression: LUT INIT and FF INIT/SRVAL on a slice off Y0 must emit
//! at the true Y_ofst table positions, not the segbits Y_ofst=0 ones.
//!
//! The case is the US-07 finding on SLICE_X48Y80 (rand001, CLEL_R,
//! y_ofst 20): A6LUT INIT 0x0b4702d25e05c990 has 25 set bits. Before the
//! fix the emitter placed all 25 at the Y0 positions (25/25 at y_ofst 0,
//! 0/25 at the true y_ofst 20); after the fix all 25 sit at the Y20
//! positions and none at the Y0 ones. FF minors are constant across Y
//! but frame offsets vary (AFF INIT: 2 at Y0, 962 at Y20), so the same
//! check pins FF INIT/SRVAL.
//!
//! Every expected (minor, frame_ofst) pair below was read by hand from
//! the vendored BitFiltrator JSON (`data/ULTRASCALE_PLUS.json`) with
//! python3, not from the code under test, following the standard in
//! `tests/lut_ff_bram.rs`. The device is the real vendored xcku3p
//! device file; the segbits DB is the four vendored CLB DBs with the
//! grouped LUT INIT entries synthesized, exactly as the `helion-bitgen`
//! CLI loads them.

use helion_ultrascale::band_code::{BAND_HI, BAND_LO};
use helion_ultrascale::bitgen::{
    BitgenDevice, FrameWords, emit_bitstream, emit_frames, parse_bitstream,
};
use helion_ultrascale::segbits::FileSegbitsDb;
use helion_ultrascale::DeviceFile;
use std::collections::HashSet;
use std::path::PathBuf;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data")
}

fn load_device() -> DeviceFile {
    DeviceFile::from_path(&data_dir().join("xcku3p.json")).unwrap()
}

/// The four vendored CLB segbits DBs, with grouped `X6LUT.INIT[63:0]`
/// entries synthesized from the per-bit lines (the CLI's load step).
fn load_db() -> FileSegbitsDb {
    let files = [
        "segbits_clel_l.db",
        "segbits_clel_r.db",
        "segbits_clem.db",
        "segbits_clem_r.db",
    ];
    let paths: Vec<PathBuf> = files.iter().map(|n| data_dir().join(n)).collect();
    let refs: Vec<&std::path::Path> = paths.iter().map(PathBuf::as_path).collect();
    let mut db = FileSegbitsDb::from_files(&refs).unwrap();
    db.synthesize_lut_init().unwrap();
    db
}

/// Set bits of the frameset as (frame offset from the tile base FAR,
/// bit offset within the frame), restricted to frames of this tile
/// (base FAR minor is 0 for CLB tiles, so the offset is the minor).
/// The computed band bits (1440-1487) are excluded: they are a code
/// over the feature bits, not feature positions, and are pinned by
/// tests/band_code.rs and tests/bitgen_roundtrip.rs.
fn set_bits(
    frames: &std::collections::BTreeMap<u32, FrameWords>,
    base_far: u32,
) -> HashSet<(u32, u16)> {
    let mut out = HashSet::new();
    for (far, words) in frames {
        if *far < base_far || *far >= base_far + 256 {
            continue;
        }
        for (wi, word) in words.iter().enumerate() {
            let mut w = *word;
            while w != 0 {
                let b = w.trailing_zeros() as u16;
                let bit = (wi * 32 + b as usize) as u16;
                if bit < BAND_LO || bit > BAND_HI {
                    out.insert((far - base_far, bit));
                }
                w &= w - 1;
            }
        }
    }
    out
}

/// The 25 set positions of A6LUT INIT 0x0b4702d25e05c990 at y_ofst 20
/// (SLICE_X48Y80, CLEL_R), read from the vendored JSON.
fn y20_set() -> HashSet<(u32, u16)> {
    [
        (8, 961),
        (8, 966),
        (8, 969),
        (8, 972),
        (8, 973),
        (8, 974),
        (9, 962),
        (9, 963),
        (9, 966),
        (9, 968),
        (9, 969),
        (9, 971),
        (9, 972),
        (10, 961),
        (10, 963),
        (10, 965),
        (10, 967),
        (10, 969),
        (11, 961),
        (11, 963),
        (11, 966),
        (11, 968),
        (11, 971),
        (11, 973),
        (11, 974),
    ]
    .into_iter()
    .collect()
}

/// The same 25 set positions at y_ofst 0: same minors, offsets 960 lower.
fn y0_set() -> HashSet<(u32, u16)> {
    [
        (8, 1),
        (8, 6),
        (8, 9),
        (8, 12),
        (8, 13),
        (8, 14),
        (9, 2),
        (9, 3),
        (9, 6),
        (9, 8),
        (9, 9),
        (9, 11),
        (9, 12),
        (10, 1),
        (10, 3),
        (10, 5),
        (10, 7),
        (10, 9),
        (11, 1),
        (11, 3),
        (11, 6),
        (11, 8),
        (11, 11),
        (11, 13),
        (11, 14),
    ]
    .into_iter()
    .collect()
}

#[test]
fn lut_init_off_y0_emits_at_true_y_offset() {
    let device = load_device();
    let db = load_db();
    assert_eq!(device.tile_type("SLICE_X48Y80"), Some("CLEL_R"));
    // Block 0, row 1 (Y 80 is in clock-region row 1), column major 99.
    assert_eq!(device.tile_base_far("SLICE_X48Y80"), Some(0x0004_6300));
    let frames = emit_frames(
        "SLICE_X48Y80.A6LUT.INIT[63:0] = 0x0b4702d25e05c990\n",
        &device,
        &db,
    )
    .unwrap();
    let got = set_bits(&frames, 0x0004_6300);
    // The F1 symptom, inverted: before the fix this set equalled y0_set
    // exactly (25/25) and shared nothing with y20_set (0/25).
    assert_eq!(got, y20_set(), "emitted set bits must be the Y20 positions");
    assert!(
        got.intersection(&y0_set()).next().is_none(),
        "no emitted bit may sit at a Y0 position"
    );
}

#[test]
fn lut_init_y0_control_is_unchanged() {
    let device = load_device();
    let db = load_db();
    assert_eq!(device.tile_base_far("SLICE_X0Y0"), Some(0x0000_0500));
    let frames = emit_frames(
        "SLICE_X0Y0.A6LUT.INIT[63:0] = 0x0b4702d25e05c990\n",
        &device,
        &db,
    )
    .unwrap();
    let got = set_bits(&frames, 0x0000_0500);
    assert_eq!(got, y0_set(), "Y0 emission must not move");
}

#[test]
fn lut_init_per_bit_off_y0_emits_at_true_y_offset() {
    // Per-bit spelling `ALUT.INIT[4]`: INIT bit 4 sits at (11, 974) at
    // y_ofst 20 and at (11, 14) at y_ofst 0.
    let device = load_device();
    let db = load_db();
    let frames = emit_frames("SLICE_X48Y80.ALUT.INIT[4]\n", &device, &db).unwrap();
    let got = set_bits(&frames, 0x0004_6300);
    assert_eq!(got, HashSet::from([(11, 974)]));
}

#[test]
fn ff_init_srval_off_y0_emit_at_true_y_offset() {
    let device = load_device();
    let db = load_db();
    // AFF INIT 0 sets its bit, AFF SRVAL 0 sets its bit (one frame
    // offset below INIT for A/C/E/G letters), GFF INIT 0 (valued
    // spelling) sets its bit, BFF INIT 1 clears its bit. AFF INIT at
    // y_ofst 20 is (12, 962); AFF SRVAL is (12, 961); GFF INIT is
    // (12, 992); BFF INIT is (12, 964). The Y0 positions are
    // (12, 2), (12, 1), (12, 32), (12, 4).
    let frames = emit_frames(
        "SLICE_X48Y80.AFF.INIT.V0\n\
         SLICE_X48Y80.AFF.SRVAL.V0\n\
         SLICE_X48Y80.GFF.INIT = 0\n\
         SLICE_X48Y80.BFF.INIT.V1\n",
        &device,
        &db,
    )
    .unwrap();
    let got = set_bits(&frames, 0x0004_6300);
    assert_eq!(
        got,
        HashSet::from([(12, 962), (12, 961), (12, 992)]),
        "FF bits must sit at the Y20 positions"
    );
    for y0 in [(12, 2), (12, 1), (12, 32), (12, 4)] {
        assert!(!got.contains(&y0), "Y0 position {y0:?} must stay clear");
    }
}

#[test]
fn ff_srval_valued_off_y0_emits_at_true_y_offset() {
    // Valued SRVAL spelling: SRVAL 0 sets the derived SRVAL bit
    // (AFF: INIT offset 962, delta -1, so 961).
    let device = load_device();
    let db = load_db();
    let frames = emit_frames("SLICE_X48Y80.AFF.SRVAL = 0\n", &device, &db).unwrap();
    let got = set_bits(&frames, 0x0004_6300);
    assert_eq!(got, HashSet::from([(12, 961)]));
}

#[test]
fn lut_ff_off_y0_roundtrip_through_parse() {
    let device = load_device();
    let db = load_db();
    let fasm = "SLICE_X48Y80.A6LUT.INIT[63:0] = 0x0b4702d25e05c990\n\
                SLICE_X48Y80.AFF.INIT.V0\n\
                SLICE_X48Y80.AFF.SRVAL.V0\n\
                SLICE_X48Y80.GFF.INIT = 0\n\
                SLICE_X48Y80.BFF.INIT.V1\n";
    let bytes = emit_bitstream(fasm, &device, &db).unwrap();
    let back = parse_bitstream(&bytes, &device, &db).unwrap();
    let has = |line: &str| back.lines().any(|l| l == line);
    assert!(
        has("SLICE_X48Y80.A6LUT.INIT[63:0] = 64'h0B4702D25E05C990"),
        "parse-back must render the LUT INIT at its value, got:\n{}",
        back.lines()
            .filter(|l| l.starts_with("SLICE_X48Y80.A6LUT"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(has("SLICE_X48Y80.AFF.INIT.V0"));
    assert!(has("SLICE_X48Y80.AFF.SRVAL.V0"));
    assert!(has("SLICE_X48Y80.GFF.INIT.V0"));
    assert!(has("SLICE_X48Y80.BFF.INIT.V1"));
    // The wrong Y0 reading would render the complement instead.
    assert!(!has("SLICE_X48Y80.AFF.INIT.V1"));
    assert!(!has("SLICE_X48Y80.GFF.INIT.V1"));
}
