//! Integration tests for the `helion-bitgen decode` subcommand path:
//! bitstream bytes in, FASM text out, loud failure on truncation or on
//! bits no feature explains.
//!
//! The library-level tests use a hand-built stub device and stub segbits
//! DB (same shape as `bitgen_roundtrip.rs`, smaller): one CLE LUT
//! feature, one FF INIT pair, one INT PIP. The CLI tests shell out to
//! the real `helion-bitgen` binary (`CARGO_BIN_EXE_helion-bitgen`) and
//! decode a bitstream emitted through the real xcku3p device file and
//! vendored segbits DBs.
//!
//! Exact round-trip equality (`decode(emit(f)) == f`) is asserted only
//! on the stub device, where the tile inventory is exactly the tiles
//! the FASM mentions. On the real device, `parse_bitstream` also
//! renders the observable zero state of every untouched FF (its `.V1`
//! complement line), so the CLI tests assert containment plus
//! set-bit equality on re-emit, never exact stdout equality. That is
//! the library's current behavior, not a test tolerance: see the
//! `decode` notes in `src/bin/helion-bitgen.rs`.
//!
//! No em dashes anywhere.

// ---------------------------------------------------------------------------
// Stub device / DB (hand-built, minimal)
// ---------------------------------------------------------------------------

use helion_ultrascale::BitRef;
use helion_ultrascale::bitgen::{
    BitgenError, StubDevice, StubSegbitsDb, emit_bitstream, emit_frames, parse_bitstream,
    parse_frames,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const FASM_LUT_AND_PIP: &str = "INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1\n\
SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h8000000000000001\n\
SLICE_X0Y0.AFF.INIT.V0\n";

fn mini_device() -> StubDevice {
    StubDevice::new(
        "xcku3p",
        Some(0x04A6_3093),
        vec![
            ("SLICE_X0Y0".to_string(), "CLEL_L".to_string(), 0x500),
            ("INT_X0Y0".to_string(), "INT".to_string(), 0x300),
        ],
        vec![0x304, 0x50C],
    )
}

fn mini_db() -> StubSegbitsDb {
    let mut lut: Vec<BitRef> = Vec::new();
    for bit in (0..64).rev() {
        lut.push(BitRef::set(12, bit));
    }
    StubSegbitsDb::new(vec![
        ("CLEL_L".to_string(), "A6LUT.INIT[63:0]".to_string(), lut),
        (
            "CLEL_L".to_string(),
            "AFF.INIT.V0".to_string(),
            vec![BitRef::set(12, 64)],
        ),
        (
            "CLEL_L".to_string(),
            "AFF.INIT.V1".to_string(),
            vec![BitRef::zero(12, 64)],
        ),
        (
            "INT".to_string(),
            "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1".to_string(),
            vec![
                BitRef::set(4, 100),
                BitRef::zero(4, 101),
                BitRef::zero(4, 102),
            ],
        ),
    ])
}

/// Set bits of a frame map, as (FAR, bit) pairs.
fn set_bits(frames: &std::collections::BTreeMap<u32, [u32; 93]>) -> BTreeSet<(u32, u16)> {
    let mut out = BTreeSet::new();
    for (far, words) in frames {
        for (wi, word) in words.iter().enumerate() {
            let mut w = *word;
            while w != 0 {
                let b = w.trailing_zeros() as usize;
                out.insert((*far, (wi * 32 + b) as u16));
                w &= w - 1;
            }
        }
    }
    out
}

/// MAP-A band the emitter computes for one frame, cross-checked
/// against the band stored in the frame words. The FASM under test
/// has no RAM-mode LUT, so the LUTRAM mask is all false; if a masked
/// bit ever contributed, the stored band would differ from this
/// unmasked computation and the assert would fail loud instead of
/// silently passing with the wrong expectation.
fn expected_band(far: u32, words: &[u32; 93]) -> u64 {
    use helion_ultrascale::band_code::{BandFormula, compute_band, frame_band};
    use helion_ultrascale::bitgen::FRAME_BITS;
    let mask = [false; FRAME_BITS];
    let computed = compute_band(far, words, &mask, BandFormula::MapA);
    assert_eq!(
        frame_band(words),
        computed,
        "emitter stored band must equal the MAP-A computation for FAR 0x{far:08x}"
    );
    computed
}

/// Absolute bit positions (1440-1487) of the computed band for one
/// frame, as (FAR, bit) pairs.
fn expected_band_bits(far: u32, words: &[u32; 93]) -> BTreeSet<(u32, u16)> {
    use helion_ultrascale::band_code::BAND_LO;
    let band = expected_band(far, words);
    let mut out = BTreeSet::new();
    for j in 0..48 {
        if (band >> j) & 1 == 1 {
            out.insert((far, BAND_LO + j as u16));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// (a) Synthetic minimal .bit, built by hand, round-trips the packet layer
// ---------------------------------------------------------------------------

#[test]
fn synthetic_minimal_bit_round_trips_through_packet_layer() {
    // One LUT line, hand-written, value with only bit 0 set. The
    // hand-computed frame: LUT refs are frame offset 12, bits 63..0
    // MSB-first, so value bit 0 lands on frame bit 0 of FAR 0x50C,
    // i.e. word 0 == 0x00000001 and every other word 0.
    //
    // emit_bitstream writes the whole device config order, not just
    // the touched frames: the stub config order is [0x304, 0x50C],
    // so the parsed map has two frames, the untouched INT frame as
    // all zeros and the LUT frame with the one set bit. emit_frames
    // returns only the touched frame.
    let device = mini_device();
    let db = mini_db();
    let fasm = "SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h0000000000000001\n";
    let bytes = emit_bitstream(fasm, &device, &db).unwrap();
    let frames = parse_frames(&bytes).unwrap();
    assert_eq!(frames.len(), 2, "both config-order frames");
    let int_frame = frames.get(&0x304).expect("INT frame FAR 0x304");
    assert!(
        int_frame.iter().all(|w| *w == 0),
        "untouched INT frame is all zeros"
    );
    let frame = frames.get(&0x50C).expect("LUT frame FAR 0x50C");
    assert_eq!(frame[0], 0x0000_0001, "value bit 0 is frame bit 0");
    // The emitter writes the computed H-1 band into words 45-46 of
    // this block-0 frame, so words 45/46 must carry EXACTLY the
    // MAP-A band for the single set bit and nothing else; every
    // other word besides 0, 45 and 46 is zero. The band is recomputed
    // here from the emitter's own computation and cross-checked
    // against the stored band, never copied from a previous run.
    let band = expected_band(0x50C, frame);
    assert_eq!(frame[45], band as u32, "word 45 is the low band word");
    assert_eq!(
        frame[46],
        (band >> 32) as u32,
        "word 46 low half is the high band word"
    );
    assert!(
        frame
            .iter()
            .enumerate()
            .all(|(i, w)| i == 0 || i == 45 || i == 46 || *w == 0),
        "all words besides 0, 45 and 46 are zero"
    );
    // The packet layer agrees with the frame emitter exactly on the
    // touched frame.
    let emitted = emit_frames(fasm, &device, &db).unwrap();
    assert_eq!(emitted.len(), 1, "emit_frames returns only touched frames");
    assert_eq!(emitted.get(&0x50C), Some(frame));
    // Decode renders the LUT line plus the observable FF zero state:
    // the AFF INIT bit reads 0, and .V1 is the exact complement of
    // .V0, so the decoder reports AFF.INIT.V1. That line is real
    // decoded state, not noise: re-emitting it sets no bits.
    let back = parse_bitstream(&bytes, &device, &db).unwrap();
    assert_eq!(
        back,
        "SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h0000000000000001\nSLICE_X0Y0.AFF.INIT.V1\n"
    );
}

// ---------------------------------------------------------------------------
// (b) decode(emit(f)) == f on a CLE LUT plus an INT PIP
// ---------------------------------------------------------------------------

#[test]
fn decode_emit_round_trips_lut_and_pip() {
    let device = mini_device();
    let db = mini_db();
    let bytes = emit_bitstream(FASM_LUT_AND_PIP, &device, &db).unwrap();
    let back = parse_bitstream(&bytes, &device, &db).unwrap();
    assert_eq!(back, FASM_LUT_AND_PIP, "decode(emit(f)) == f");
    let parsed = parse_frames(&bytes).unwrap();
    assert_eq!(parsed, emit_frames(FASM_LUT_AND_PIP, &device, &db).unwrap());
    // Hand-computed feature set bits: LUT value 0x8000...0001 sets
    // frame bits 63 and 0 of FAR 0x50C; AFF.INIT.V0 sets bit 64 of
    // FAR 0x50C; the PIP sets bit 100 of FAR 0x304. The emitter also
    // writes the computed H-1 band (absolute bits 1440-1487) into
    // each block-0 frame, so the full set is the feature bits plus
    // EXACTLY the MAP-A band bits recomputed here per frame: no band
    // bit missing, no bit outside the band range, nothing else.
    let mut expected: BTreeSet<(u32, u16)> =
        BTreeSet::from([(0x50C, 0), (0x50C, 63), (0x50C, 64), (0x304, 100)]);
    for (far, words) in &parsed {
        expected.extend(expected_band_bits(*far, words));
    }
    assert!(
        expected.iter().all(|&(_, bit)| bit <= 1487),
        "no set bit may sit above the band 1440-1487: {expected:?}"
    );
    assert_eq!(set_bits(&parsed), expected);
}

#[test]
fn decode_with_missing_feature_fails_loud_on_unexplained_bits() {
    // Same bitstream, decoded against a DB that lacks the LUT feature:
    // the two LUT set bits are unexplained and the decode must fail
    // with UnexplainedBits naming the exact FAR and bits, not silently
    // drop them or panic.
    let device = mini_device();
    let db = mini_db();
    let bytes = emit_bitstream(FASM_LUT_AND_PIP, &device, &db).unwrap();
    let partial = StubSegbitsDb::new(vec![
        (
            "CLEL_L".to_string(),
            "AFF.INIT.V0".to_string(),
            vec![BitRef::set(12, 64)],
        ),
        (
            "CLEL_L".to_string(),
            "AFF.INIT.V1".to_string(),
            vec![BitRef::zero(12, 64)],
        ),
        (
            "INT".to_string(),
            "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1".to_string(),
            vec![
                BitRef::set(4, 100),
                BitRef::zero(4, 101),
                BitRef::zero(4, 102),
            ],
        ),
    ]);
    let err = parse_bitstream(&bytes, &device, &partial).unwrap_err();
    match err {
        BitgenError::UnexplainedBits { far, bits } => {
            assert_eq!(far, 0x50C);
            assert_eq!(bits, vec![0, 63]);
        }
        other => panic!("expected UnexplainedBits, got {other}"),
    }
}

// ---------------------------------------------------------------------------
// (c) A truncated .bit fails loud: no panic, no silent success
// ---------------------------------------------------------------------------

#[test]
fn truncated_bit_fails_loud() {
    let device = mini_device();
    let db = mini_db();
    let bytes = emit_bitstream(FASM_LUT_AND_PIP, &device, &db).unwrap();
    assert!(bytes.len() > 100, "emitted bitstream is non-trivial");
    // Each case must fail in both layers. A panic in either layer
    // fails this test by aborting it; a silent Ok fails the assert.
    let mut cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", vec![]),
        ("one word", bytes[..4].to_vec()),
        ("no sync word", vec![0xAA; 100]),
        ("first half", bytes[..bytes.len() / 2].to_vec()),
        // Cut 450 words off the end. The stream ends with 413 words
        // after the last frame's CRC (RCRC, GRESTORE, DGHIGH_LFRM,
        // START, DESYNC, 400 trailing NOOPs), so this lands 37 words
        // inside the last frame's 100-word block, mid-FDRI: the
        // packet declares 93 words but fewer remain. Dropping only
        // trailing NOOPs is NOT a failure case: parse_frames stops
        // at DESYNC and ignores trailing words by design.
        (
            "cut inside the last frame's data",
            bytes[..bytes.len() - 4 * 450].to_vec(),
        ),
        (
            "length not a whole number of words",
            bytes[..bytes.len() - 3].to_vec(),
        ),
    ];
    // A corrupted payload word breaks the per-frame CRC: the stream
    // is the right length and has a sync word, but must still fail.
    let mut corrupted = bytes.clone();
    corrupted[100] ^= 0xFF;
    cases.push(("corrupted payload word", corrupted));
    for (name, input) in cases {
        assert!(
            parse_frames(&input).is_err(),
            "parse_frames must fail on {name}"
        );
        assert!(
            parse_bitstream(&input, &device, &db).is_err(),
            "parse_bitstream must fail on {name}"
        );
    }
}

// ---------------------------------------------------------------------------
// CLI tests: the real helion-bitgen decode subcommand
// ---------------------------------------------------------------------------

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data")
}

fn cli_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_helion-bitgen"))
}

fn run_cli(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(cli_bin())
        .args(args)
        .output()
        .expect("run helion-bitgen");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("helion-decode-test-{}-{name}", std::process::id()))
}

/// The real xcku3p device plus the vendored segbits DBs (the same
/// 9-file set the binary loads), for emitting a genuine .bit.
fn real_db() -> helion_ultrascale::segbits::FileSegbitsDb {
    use helion_ultrascale::segbits::FileSegbitsDb;
    const FILES: [&str; 9] = [
        "segbits_int.db",
        "segbits_hpio_right.db",
        "segbits_hdio_top_right.db",
        "segbits_hdio_bot_right.db",
        "segbits_cmt_right.db",
        "segbits_clel_l.db",
        "segbits_clel_r.db",
        "segbits_clem.db",
        "segbits_clem_r.db",
    ];
    let dir = data_dir();
    let paths: Vec<PathBuf> = FILES.iter().map(|n| dir.join(n)).collect();
    let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let mut db = FileSegbitsDb::from_files(&refs).unwrap();
    db.synthesize_lut_init().unwrap();
    db
}

fn real_device() -> helion_ultrascale::device_file::DeviceFile {
    helion_ultrascale::device_file::DeviceFile::from_path(&data_dir().join("xcku3p.json")).unwrap()
}

/// Canonical FASM for the CLI round trip (the spellings parse-back
/// renders: 64'h LUT value, .V0 FF spelling). No HPIO lines: the
/// plain DeviceFile has no HPIO bridge (that lives in the binary).
const CLI_FASM: &str = "INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1\n\
INT_X0Y0.PIP.INT_NODE_GLOBAL_0_INT_OUT0.GCLK_B_0_0\n\
SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h5555555555555555\n\
SLICE_X0Y0.AFF.INIT.V0\n";

fn emit_cli_bitstream() -> Vec<u8> {
    use helion_ultrascale::bitgen::emit_bitstream;
    let device = real_device();
    let db = real_db();
    emit_bitstream(CLI_FASM, &device, &db).unwrap()
}

#[test]
fn cli_decode_round_trips_xcku3p() {
    let bytes = emit_cli_bitstream();
    let bit_path = temp_path("blinky.bit");
    std::fs::write(&bit_path, &bytes).unwrap();
    let db_root = data_dir();
    let (ok, stdout, stderr) = run_cli(&[
        "decode",
        "--bit",
        bit_path.to_str().unwrap(),
        "--db-root",
        db_root.to_str().unwrap(),
        "--part",
        "xcku3p",
    ]);
    let _ = std::fs::remove_file(&bit_path);
    assert!(ok, "decode must exit 0, stderr: {stderr}");
    assert!(
        stderr.is_empty(),
        "decode stderr must be empty on success: {stderr}"
    );
    for line in CLI_FASM.lines() {
        assert!(
            stdout.contains(line),
            "decode stdout must contain {line:?}, got:\n{stdout}"
        );
    }
    // Re-emitting the decoded FASM must set identical bits, and a
    // second decode of the re-emitted stream must print identical
    // FASM: decode is stable on its own output.
    use helion_ultrascale::bitgen::emit_frames;
    let device = real_device();
    let db = real_db();
    let frames1 = emit_frames(CLI_FASM, &device, &db).unwrap();
    let frames2 = emit_frames(&stdout, &device, &db).unwrap();
    assert_eq!(
        set_bits(&frames1),
        set_bits(&frames2),
        "decoded FASM re-emits identical set bits"
    );
    let bytes2 = emit_bitstream(&stdout, &device, &db).unwrap();
    let bit_path2 = temp_path("blinky2.bit");
    std::fs::write(&bit_path2, &bytes2).unwrap();
    let (ok2, stdout2, stderr2) = run_cli(&[
        "decode",
        "--bit",
        bit_path2.to_str().unwrap(),
        "--db-root",
        db_root.to_str().unwrap(),
        "--part",
        "xcku3p",
    ]);
    let _ = std::fs::remove_file(&bit_path2);
    assert!(ok2, "second decode must exit 0, stderr: {stderr2}");
    assert_eq!(stdout2, stdout, "decode is idempotent on its own output");
}

#[test]
fn cli_decode_truncated_bit_exits_nonzero() {
    let bytes = emit_cli_bitstream();
    let bit_path = temp_path("truncated.bit");
    std::fs::write(&bit_path, &bytes[..bytes.len() / 2]).unwrap();
    let (ok, _stdout, stderr) = run_cli(&[
        "decode",
        "--bit",
        bit_path.to_str().unwrap(),
        "--db-root",
        data_dir().to_str().unwrap(),
        "--part",
        "xcku3p",
    ]);
    let _ = std::fs::remove_file(&bit_path);
    assert!(!ok, "decode of a truncated .bit must exit non-zero");
    assert!(
        stderr.contains("decode failed"),
        "stderr must name the failure, got: {stderr}"
    );
}

#[test]
fn cli_decode_unexplained_bits_listed_on_stderr() {
    // Emit with the full DB, decode against a db-root holding only
    // the INT segbits: the LUT bits become unexplained and the CLI
    // must list them on stderr and exit non-zero.
    let bytes = emit_cli_bitstream();
    let bit_path = temp_path("unexplained.bit");
    std::fs::write(&bit_path, &bytes).unwrap();
    let partial_root = temp_path("partial-db");
    std::fs::create_dir_all(&partial_root).unwrap();
    std::fs::copy(
        data_dir().join("segbits_int.db"),
        partial_root.join("segbits_int.db"),
    )
    .unwrap();
    let (ok, _stdout, stderr) = run_cli(&[
        "decode",
        "--bit",
        bit_path.to_str().unwrap(),
        "--db-root",
        partial_root.to_str().unwrap(),
        "--part",
        "xcku3p",
    ]);
    let _ = std::fs::remove_file(&bit_path);
    let _ = std::fs::remove_dir_all(&partial_root);
    assert!(!ok, "decode with unexplained bits must exit non-zero");
    assert!(
        stderr.contains("unexplained"),
        "stderr must list unexplained bits, got: {stderr}"
    );
    assert!(
        stderr.contains("0x0000050c"),
        "stderr must name the LUT FAR 0x0000050c, got: {stderr}"
    );
}

#[test]
fn cli_decode_missing_bit_file_exits_nonzero() {
    let (ok, _stdout, stderr) = run_cli(&[
        "decode",
        "--bit",
        "/definitely/not/a/real/bitstream.bit",
        "--db-root",
        data_dir().to_str().unwrap(),
        "--part",
        "xcku3p",
    ]);
    assert!(!ok, "decode of a missing file must exit non-zero");
    assert!(
        stderr.contains("cannot read bitstream"),
        "stderr must name the missing file, got: {stderr}"
    );
}

#[test]
fn cli_decode_unknown_part_exits_nonzero() {
    let bytes = emit_cli_bitstream();
    let bit_path = temp_path("part.bit");
    std::fs::write(&bit_path, &bytes).unwrap();
    let (ok, _stdout, stderr) = run_cli(&[
        "decode",
        "--bit",
        bit_path.to_str().unwrap(),
        "--db-root",
        data_dir().to_str().unwrap(),
        "--part",
        "nosuchpart123",
    ]);
    let _ = std::fs::remove_file(&bit_path);
    assert!(!ok, "decode of an unknown part must exit non-zero");
    assert!(
        stderr.contains("has no decoder device"),
        "stderr must name the unsupported part, got: {stderr}"
    );
}

#[test]
fn cli_decode_missing_db_root_exits_nonzero() {
    let bytes = emit_cli_bitstream();
    let bit_path = temp_path("dbroot.bit");
    std::fs::write(&bit_path, &bytes).unwrap();
    let (ok, _stdout, stderr) = run_cli(&[
        "decode",
        "--bit",
        bit_path.to_str().unwrap(),
        "--db-root",
        "/definitely/not/a/db/root",
        "--part",
        "xcku3p",
    ]);
    let _ = std::fs::remove_file(&bit_path);
    assert!(!ok, "decode with a missing db-root must exit non-zero");
    assert!(
        stderr.contains("cannot read segbits DB root") || stderr.contains("no segbits_*.db files"),
        "stderr must name the bad db-root, got: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// CLI test: default part (xczu3eg-sbva484-1-e) via Worker B's decoder
// ---------------------------------------------------------------------------

/// prjuray-db family dir for the xczu3eg CLI test: same resolution
/// as the binary tests (`$HELION_DECODE_DB_ROOT`, else the reference
/// clone). Fails loud with the path when the clone is absent.
fn zynqusp_db_root() -> PathBuf {
    if let Ok(p) = std::env::var("HELION_DECODE_DB_ROOT") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("workspace/prjuray-db-ref/zynqusp")
}

/// The real zynqusp segbits DBs (all `segbits_*.db` except
/// `*.origin_info.db`), for emitting a synthetic xczu3eg bitstream
/// whose bits Worker B's decoder must own.
fn real_zynqusp_db(db_root: &Path) -> helion_ultrascale::segbits::FileSegbitsDb {
    use helion_ultrascale::segbits::FileSegbitsDb;
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(db_root).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("segbits_") && name.ends_with(".db") && !name.contains("origin_info") {
            paths.push(entry.path());
        }
    }
    paths.sort();
    assert!(
        !paths.is_empty(),
        "no segbits_*.db files in {}",
        db_root.display()
    );
    let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let mut db = FileSegbitsDb::from_files(&refs).unwrap();
    db.synthesize_lut_init().unwrap();
    db
}

/// Emit one LUT plus one PIP on offset-0 xczu3eg tiles with the real
/// DB (the stub device only supplies tile type and base FAR:
/// SLICE_X0Y0 is CLEL_R at 0x200, INT_X0Y0 is INT at 0x100, both read
/// from the public tilegrid). Returns the bitstream bytes and the
/// emitted frame map.
fn emit_xczu3eg_synthetic_bitstream() -> (Vec<u8>, std::collections::BTreeMap<u32, [u32; 93]>) {
    use helion_ultrascale::bitgen::{StubDevice, emit_bitstream, emit_frames};
    let db_root = zynqusp_db_root();
    let db = real_zynqusp_db(&db_root);
    let fasm = "INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1\n\
        SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h8000000000000001\n\
        SLICE_X0Y0.AFF.INIT.V0\n";
    let tiles = vec![
        ("SLICE_X0Y0".to_string(), "CLEL_R".to_string(), 0x200),
        ("INT_X0Y0".to_string(), "INT".to_string(), 0x100),
    ];
    let probe = StubDevice::new(
        "xczu3eg-sbva484-1-e",
        Some(0x04A4_2093),
        tiles.clone(),
        vec![],
    );
    let frames = emit_frames(fasm, &probe, &db).unwrap();
    let mut order: Vec<u32> = frames.keys().copied().collect();
    order.sort();
    let device = StubDevice::new("xczu3eg-sbva484-1-e", Some(0x04A4_2093), tiles, order);
    let bytes = emit_bitstream(fasm, &device, &db).unwrap();
    (bytes, frames)
}

#[test]
fn cli_decode_default_part_decodes_xczu3eg() {
    // Decode with no --part flag: the default must be
    // xczu3eg-sbva484-1-e and the run must exit 0 with the three
    // lines on stdout and empty stderr. Worker B renders valued
    // features as lowercase 0x, so the LUT line is asserted in that
    // spelling. The emitter writes the computed H-1 band into every
    // block-0 frame and the decoder now explains band bits through
    // the N7 check (masked MAP-A, no RAM-mode LUT here so the mask
    // is all false), so no bit is unexplained.
    let (bytes, _frames) = emit_xczu3eg_synthetic_bitstream();
    let db_root = zynqusp_db_root();
    let bit_path = temp_path("xczu3eg.bit");
    std::fs::write(&bit_path, &bytes).unwrap();
    let (ok, stdout, stderr) = run_cli(&[
        "decode",
        "--bit",
        bit_path.to_str().unwrap(),
        "--db-root",
        db_root.to_str().unwrap(),
    ]);
    let _ = std::fs::remove_file(&bit_path);
    assert!(ok, "decode must exit 0, stderr: {stderr}");
    assert!(
        stderr.is_empty(),
        "decode stderr must be empty on success: {stderr}"
    );
    assert!(
        stdout.contains("INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1"),
        "PIP line missing, got:\n{stdout}"
    );
    assert!(
        stdout.contains("SLICE_X0Y0.A6LUT.INIT[63:0] = 0x8000000000000001"),
        "LUT line missing, got:\n{stdout}"
    );
    assert!(
        stdout.contains("SLICE_X0Y0.AFF.INIT.V0"),
        "FF line missing, got:\n{stdout}"
    );
}

#[test]
fn cli_decode_flipped_band_bit_fails_loud_with_band_mismatch() {
    // Emit the xczu3eg synthetic bitstream, flip one band bit in the
    // LUT frame's FDRI payload, fix up that frame's CRC (replayed
    // with crc32c_step, asserting the pre-flip CRCs match so a replay
    // bug shows up as a CRC failure, not a band failure), and confirm
    // CLI decode fails loud with BandMismatch naming FAR 0x20c, not
    // with unexplained bits. Same replay pattern as the N7 parse test
    // (`parse_rejects_flipped_band_bit_with_named_error`).
    use helion_ultrascale::band_code::frame_band;
    use helion_ultrascale::bitgen::{DUMMY_WORD, SYNC_WORD, cmd, crc32c_step, reg};
    let (bytes, frames) = emit_xczu3eg_synthetic_bitstream();
    let db_root = zynqusp_db_root();
    let stored_before = frame_band(&frames[&0x20c]);
    let mut words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let sync = words.iter().position(|w| *w == SYNC_WORD).unwrap();
    let mut i = sync + 1;
    let mut far = 0u32;
    let mut target: Option<usize> = None;
    let mut flipped_crc: Option<usize> = None;
    while i < words.len() {
        let header = words[i];
        let ptype = header >> 29;
        if ptype == 0 {
            assert_eq!(header, DUMMY_WORD);
            i += 1;
            continue;
        }
        assert_eq!(ptype, 1, "emit writes type-1 packets only");
        let opcode = (header >> 27) & 0x3;
        let addr = ((header >> 13) & 0x1F) as u8;
        let count = (header & 0x7FF) as usize;
        if opcode == 2 && addr == reg::FAR && count == 1 {
            far = words[i + 1];
        }
        if opcode == 2 && addr == reg::FDRI && far == 0x20c {
            assert_eq!(count, 93);
            target = Some(i + 1 + 45);
        }
        if opcode == 2 && addr == reg::CRC && count == 1 && far == 0x20c {
            flipped_crc = Some(i + 1);
        }
        i += 1 + count;
    }
    let bit_word = target.expect("FDRI payload for FAR 0x20c");
    let crc_word = flipped_crc.expect("CRC packet for FAR 0x20c");
    words[bit_word] ^= 1u32;
    // Flip band bit 1440 (word 45, bit 0): stored becomes
    // stored_before ^ 1 against the unchanged expected band.
    let mut crc: u32 = 0;
    let mut i = sync + 1;
    while i < words.len() {
        let header = words[i];
        let ptype = header >> 29;
        if ptype == 0 {
            assert_eq!(header, DUMMY_WORD);
            i += 1;
            continue;
        }
        let opcode = (header >> 27) & 0x3;
        let addr = ((header >> 13) & 0x1F) as u8;
        let count = (header & 0x7FF) as usize;
        if opcode == 0 {
            // NOOP: nothing to feed.
        } else if addr == reg::CMD && count > 0 && words[i + 1] & 0x1F == cmd::RCRC {
            crc = 0;
        } else if addr == reg::CRC {
            assert_eq!(count, 1);
            if i + 1 != crc_word {
                assert_eq!(
                    words[i + 1],
                    crc,
                    "replay must reproduce the stored CRC before the flip"
                );
            }
            words[i + 1] = crc;
            crc = 0;
        } else {
            for j in 0..count {
                crc = crc32c_step(addr as u32, words[i + 1 + j], crc);
            }
        }
        i += 1 + count;
    }
    let mut flipped = Vec::with_capacity(words.len() * 4);
    for w in words {
        flipped.extend_from_slice(&w.to_be_bytes());
    }
    let bit_path = temp_path("flipped-band.bit");
    std::fs::write(&bit_path, &flipped).unwrap();
    let (ok, _stdout, stderr) = run_cli(&[
        "decode",
        "--bit",
        bit_path.to_str().unwrap(),
        "--db-root",
        db_root.to_str().unwrap(),
    ]);
    let _ = std::fs::remove_file(&bit_path);
    assert!(!ok, "decode of a flipped-band .bit must exit non-zero");
    assert!(
        stderr.contains("BandMismatch"),
        "stderr must name BandMismatch, got: {stderr}"
    );
    assert!(
        stderr.contains("0x0000020c"),
        "stderr must name the LUT FAR 0x0000020c, got: {stderr}"
    );
    assert!(
        stderr.contains(&format!("0x{:012x}", stored_before ^ 1)),
        "stderr must name the flipped stored band, got: {stderr}"
    );
    assert!(
        stderr.contains(&format!("0x{:012x}", stored_before)),
        "stderr must name the expected band, got: {stderr}"
    );
}
