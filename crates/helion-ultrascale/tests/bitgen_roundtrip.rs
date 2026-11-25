//! Round-trip test for the UltraScale+ bitstream emitter (bitgen-core).
//!
//! Every expected number below is hand-computed independently of the Rust
//! code (Python reference implementation of the CRC and frame layout, run
//! before this test was written), from:
//! - UG570 table 9-21 (FAR layout) and table 9-19 (registers),
//! - BitFiltrator `frame.py` (frame bit N is word N/32, bit N%32,
//!   LSB-first within the word) and the per-frame CRC packet sequence
//!   (WCFG, FAR, FDRI, FAR rewrite, CRC),
//! - the CRC-32C algorithm validated against the real Vivado bitstream
//!   vector in BitFiltrator `bitstream.py` (123-word all-zero frame, FAR 1,
//!   CRC 0x06b1285e).
//!
//! Test design (canonical sorted FASM order):
//!   INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1
//!   RAMB18_X0Y0.INIT[16383:0] = 16384'h<4096 digits, bits 0 and 255 set>
//!   SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h8000000000000001
//!   SLICE_X0Y0.AFF.INIT.V0
//!
//! BRAM renders through the bit-location tables, never through segbits:
//! content bit 0 is (minor 0, ofst 0) and bit 255 is (minor 3, ofst 107)
//! at Y_ofst 0 (vendored ultrascale_plus_lut_ff_bram.json, U-Ray
//! verified). The per-word spelling INIT_00 sets the same two bits and
//! is covered by per_word_spelling_emits_identical_frames; parse-back
//! normalizes to the whole-array spelling, so the round-trip FASM uses
//! it. The stub segbits DB carries NO BRAM entry (the real vendored
//! DBs carry none either); a stale BRAM entry here would render a
//! second, conflicting INIT_00 line on parse-back.
//!
//! Stub device (xcku3p, IDCODE 0x04A63093 per UG570 table 2):
//!   SLICE_X0Y0  type CLEL_L  base FAR 0x00000500 (block 0, row 0, col 5, minor 0)
//!   INT_X0Y0    type INT     base FAR 0x00000300 (block 0, row 0, col 3, minor 0)
//!   RAMB18_X0Y0 type BRAM    base FAR 0x01000000 (block 1, row 0, col 0, minor 0)
//!   config order (ascending FAR): 0x304, 0x50C, 0x1000000, 0x1000003.
//!   Only the two BRAM minors with set bits are listed: emit does not
//!   materialize all-zero BRAM minors.
//!
//! Stub segbits DB (BitRef.frame is a frame offset from the tile base FAR;
//! INIT lists are MSB-first: list index 0 is the most significant value
//! bit, per the brief):
//!   CLEL_L A6LUT.INIT[63:0] : 64 refs, frame offset 12, bits 63..0
//!   CLEL_L AFF.INIT.V0      : 1 ref, frame offset 12, bit 64
//!   CLEL_L AFF.INIT.V1      : 1 ref, frame offset 12, `!` bit 64
//!   INT    PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1 : set bit 100, `!` bits 101, 102
//!
//! The FF INIT spelling is the DB vocabulary `.V0`/`.V1` (interfaces
//! critic session 2 item 5), not bare `AFF.INIT`. V0 sets the INIT bit;
//! V1 requires it clear. The value-to-feature polarity mapping is
//! RESOLVED (researcher session 1, verified from the fuzzer source):
//! bit = NOT(INIT) and bit = NOT(SRVAL); V0 is selected by value 0
//! and sets the bit, V1 by value 1 and clears it. This test pins the
//! bit pattern and the value meaning together.
//!
//! Expected frames (hand-computed; band words from MAP-A key "A",
//! recomputed independently in decoder-validation/band_crc_recompute.py):
//!   F2 = 0x00000304: word3 = 0x00000010 (frame bit 100), word45 = 0x00110001,
//!                     word46 = 0x00000101 (band 0x010100110001), rest 0
//!   F1 = 0x0000050C: word0 = 0x00000001 (bit 0), word1 = 0x80000000 (bit 63),
//!                     word2 = 0x00000001 (bit 64), word45 = 0x00910888,
//!                     word46 = 0x00008808 (band 0x880800910888), rest 0
//!   F3a = 0x01000000: word0 = 0x00000001 (content bit 0), rest 0
//!   F3d = 0x01000003: word3 = 0x00000800 (content bit 255: ofst 107,
//!                     word 3 bit 11), rest 0
//!
//! Expected CRCs (Python reference, emit order RCRC, IDCODE, WCFG, frames;
//! the IDCODE write feeds the CRC, so only the first frame's CRC depends on
//! the preamble; later frames start from 0 after the CRC-register write.
//! The port was validated by reproducing the pre-BRAM-branch known CRCs
//! F2 = 0x9ac4dc29, F1 = 0xaa75db0b, F3 = 0x66924d7d for the old layout,
//! and the Python model in band_crc_recompute.py reproduces the
//! pre-band CRCs F2 = 0x9ac4dc29, F1 = 0xaa75db0b, F3a = 0xd9880946,
//! F3d = 0x0b9f8503 exactly before computing the band-carrying ones):
//!   F2 = 0x85e74bef, F1 = 0x8a32ed70, F3a = 0xd9880946, F3d = 0x0b9f8503
//!
//! Expected stream: 833 words = 3332 bytes. Sync word at byte 48 (word 12).
//! Packet word offsets: RCRC header 14, IDCODE header 16, WCFG header 18,
//! F2 FAR 20 / FDRI 22 / FARrw 116 / CRC 118 (value 119),
//! F1 FAR 120 / FDRI 122 / FARrw 216 / CRC 218 (value 219),
//! F3a FAR 220 / FDRI 222 / FARrw 316 / CRC 318 (value 319),
//! F3d FAR 320 / FDRI 322 / FARrw 416 / CRC 418 (value 419),
//! ending RCRC 420, GRESTORE 424, DGHIGH 427, START 429, DESYNC 431,
//! trailing NOOPs 433..832.

use helion_ultrascale::BitRef;
use helion_ultrascale::bitgen::{
    SYNC_WORD, StubDevice, StubSegbitsDb, emit_bitstream, emit_frames, parse_bitstream,
    parse_frames,
};

/// Whole-array BRAM INIT value with absolute bits 0 and 255 set:
/// 4096 hex digits, digit 63 from the right is 8, rightmost digit is 1.
fn bram_init_hex() -> String {
    let mut s = "0".repeat(4032);
    s.push('8');
    s.push_str(&"0".repeat(62));
    s.push('1');
    debug_assert_eq!(s.len(), 4096);
    s
}

fn test_fasm() -> String {
    format!(
        "INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1\n\
        RAMB18_X0Y0.INIT[16383:0] = 16384'h{}\n\
        SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h8000000000000001\n\
        SLICE_X0Y0.AFF.INIT.V0\n",
        bram_init_hex()
    )
}

const F2: u32 = 0x0000_0304;
const F1: u32 = 0x0000_050C;
const F3A: u32 = 0x0100_0000;
const F3D: u32 = 0x0100_0003;

fn test_device() -> StubDevice {
    StubDevice::new(
        "xcku3p",
        Some(0x04A6_3093),
        vec![
            ("SLICE_X0Y0".to_string(), "CLEL_L".to_string(), 0x500),
            ("INT_X0Y0".to_string(), "INT".to_string(), 0x300),
            ("RAMB18_X0Y0".to_string(), "BRAM".to_string(), 0x100_0000),
        ],
        vec![F2, F1, F3A, F3D],
    )
}

fn test_db() -> StubSegbitsDb {
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

/// Read one big-endian word from the byte stream.
fn word(bytes: &[u8], index: usize) -> u32 {
    let o = index * 4;
    u32::from_be_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
}

#[test]
fn frames_match_hand_computed_words() {
    let device = test_device();
    let db = test_db();
    let frames = emit_frames(&test_fasm(), &device, &db).unwrap();
    assert_eq!(frames.len(), 4);

    let f2 = frames.get(&F2).unwrap();
    assert_eq!(f2[3], 0x0000_0010);
    // Band for the single set source bit 100 (MAP-A column 0x010100110001).
    assert_eq!(f2[45], 0x0011_0001);
    assert_eq!(f2[46], 0x0000_0101);
    assert!(f2
        .iter()
        .enumerate()
        .all(|(i, w)| i == 3 || i == 45 || i == 46 || *w == 0));

    let f1 = frames.get(&F1).unwrap();
    assert_eq!(f1[0], 0x0000_0001);
    assert_eq!(f1[1], 0x8000_0000);
    assert_eq!(f1[2], 0x0000_0001);
    // Band for set source bits 0, 63, 64 (XOR of MAP-A columns 0x880800910888).
    assert_eq!(f1[45], 0x0091_0888);
    assert_eq!(f1[46], 0x0000_8808);
    assert!(f1
        .iter()
        .enumerate()
        .all(|(i, w)| (0..3).contains(&i) || i == 45 || i == 46 || *w == 0));

    let f3a = frames.get(&F3A).unwrap();
    assert_eq!(f3a[0], 0x0000_0001);
    assert!(f3a.iter().enumerate().all(|(i, w)| i == 0 || *w == 0));
    // Minors 1 and 2 hold no set bit and are not materialized.
    assert!(!frames.contains_key(&0x0100_0001));
    assert!(!frames.contains_key(&0x0100_0002));
    let f3d = frames.get(&F3D).unwrap();
    assert_eq!(f3d[3], 0x0000_0800);
    assert!(f3d.iter().enumerate().all(|(i, w)| i == 3 || *w == 0));
}

#[test]
fn bitstream_has_expected_length_sync_headers_and_crcs() {
    let device = test_device();
    let db = test_db();
    let bytes = emit_bitstream(&test_fasm(), &device, &db).unwrap();
    assert_eq!(bytes.len(), 833 * 4);

    // Preamble: 8 dummy, bus width detect, bus width sync, 2 dummy, sync, NOOP.
    assert_eq!(word(&bytes, 12), SYNC_WORD);
    assert_eq!(word(&bytes, 8), 0x0000_00BB);
    assert_eq!(word(&bytes, 9), 0x1122_0044);
    assert_eq!(word(&bytes, 13), 0x2000_0000);

    // RCRC, IDCODE, WCFG.
    assert_eq!(word(&bytes, 14), 0x3000_8001);
    assert_eq!(word(&bytes, 15), 0x0000_0007);
    assert_eq!(word(&bytes, 16), 0x3001_8001);
    assert_eq!(word(&bytes, 17), 0x04A6_3093);
    assert_eq!(word(&bytes, 18), 0x3000_8001);
    assert_eq!(word(&bytes, 19), 0x0000_0001);

    // Frame F2: FAR, FDRI (93 words), FAR rewrite, CRC.
    assert_eq!(word(&bytes, 20), 0x3000_2001);
    assert_eq!(word(&bytes, 21), F2);
    assert_eq!(word(&bytes, 22), 0x3000_405D);
    assert_eq!(word(&bytes, 23 + 3), 0x0000_0010);
    // Band words of the F2 frame (FDRI starts at stream word 23).
    assert_eq!(word(&bytes, 23 + 45), 0x0011_0001);
    assert_eq!(word(&bytes, 23 + 46), 0x0000_0101);
    assert_eq!(word(&bytes, 116), 0x3000_2001);
    assert_eq!(word(&bytes, 117), F2);
    assert_eq!(word(&bytes, 118), 0x3000_0001);
    assert_eq!(word(&bytes, 119), 0x85e7_4bef);

    // Frame F1.
    assert_eq!(word(&bytes, 120), 0x3000_2001);
    assert_eq!(word(&bytes, 121), F1);
    assert_eq!(word(&bytes, 122), 0x3000_405D);
    assert_eq!(word(&bytes, 123), 0x0000_0001);
    assert_eq!(word(&bytes, 124), 0x8000_0000);
    assert_eq!(word(&bytes, 125), 0x0000_0001);
    // Band words of the F1 frame (FDRI starts at stream word 123).
    assert_eq!(word(&bytes, 123 + 45), 0x0091_0888);
    assert_eq!(word(&bytes, 123 + 46), 0x0000_8808);
    assert_eq!(word(&bytes, 216), 0x3000_2001);
    assert_eq!(word(&bytes, 217), F1);
    assert_eq!(word(&bytes, 218), 0x3000_0001);
    assert_eq!(word(&bytes, 219), 0x8a32_ed70);

    // Frame F3a (BRAM minor 0).
    assert_eq!(word(&bytes, 220), 0x3000_2001);
    assert_eq!(word(&bytes, 221), F3A);
    assert_eq!(word(&bytes, 222), 0x3000_405D);
    assert_eq!(word(&bytes, 223), 0x0000_0001);
    assert_eq!(word(&bytes, 316), 0x3000_2001);
    assert_eq!(word(&bytes, 317), F3A);
    assert_eq!(word(&bytes, 318), 0x3000_0001);
    assert_eq!(word(&bytes, 319), 0xd988_0946);

    // Frame F3d (BRAM minor 3).
    assert_eq!(word(&bytes, 320), 0x3000_2001);
    assert_eq!(word(&bytes, 321), F3D);
    assert_eq!(word(&bytes, 322), 0x3000_405D);
    assert_eq!(word(&bytes, 326), 0x0000_0800);
    assert_eq!(word(&bytes, 416), 0x3000_2001);
    assert_eq!(word(&bytes, 417), F3D);
    assert_eq!(word(&bytes, 418), 0x3000_0001);
    assert_eq!(word(&bytes, 419), 0x0b9f_8503);

    // Ending: RCRC, 2 NOOP, GRESTORE, NOOP, DGHIGH, START, DESYNC,
    // 400 NOOP. START = 5 and DESYNC = 13 are UG570 table 9-22 codes.
    assert_eq!(word(&bytes, 420), 0x3000_8001);
    assert_eq!(word(&bytes, 421), 0x0000_0007);
    assert_eq!(word(&bytes, 422), 0x2000_0000);
    assert_eq!(word(&bytes, 423), 0x2000_0000);
    assert_eq!(word(&bytes, 424), 0x3000_8001);
    assert_eq!(word(&bytes, 425), 0x0000_000A);
    assert_eq!(word(&bytes, 426), 0x2000_0000);
    assert_eq!(word(&bytes, 427), 0x3000_8001);
    assert_eq!(word(&bytes, 428), 0x0000_0003);
    assert_eq!(word(&bytes, 429), 0x3000_8001);
    assert_eq!(word(&bytes, 430), 0x0000_0005);
    assert_eq!(word(&bytes, 431), 0x3000_8001);
    assert_eq!(word(&bytes, 432), 0x0000_000D);
    for i in 433..833 {
        assert_eq!(word(&bytes, i), 0x2000_0000, "trailing NOOP at word {i}");
    }
}

#[test]
fn round_trip_returns_identical_fasm() {
    let device = test_device();
    let db = test_db();
    let fasm = test_fasm();
    let bytes = emit_bitstream(&fasm, &device, &db).unwrap();
    let back = parse_bitstream(&bytes, &device, &db).unwrap();
    assert_eq!(back, fasm);
}

#[test]
fn per_word_spelling_emits_identical_frames() {
    // The per-word spelling is accepted on emit and lands the same
    // two BRAM bits as the whole-array spelling; only the spelling
    // differs, not the bits. Parse-back normalizes to whole-array,
    // so this test (not the round-trip test) is where per-word
    // coverage lives.
    let device = test_device();
    let db = test_db();
    let per_word = "RAMB18_X0Y0.INIT_00 = 256'h8000000000000000000000000000000000000000000000000000000000000001\n";
    let a = emit_frames(per_word, &device, &db).unwrap();
    let b = emit_frames(&test_fasm(), &device, &db).unwrap();
    let bram_a: Vec<_> = a.iter().filter(|(far, _)| **far >= 0x0100_0000).collect();
    let bram_b: Vec<_> = b.iter().filter(|(far, _)| **far >= 0x0100_0000).collect();
    assert_eq!(bram_a, bram_b);
}

#[test]
fn parse_frames_matches_emit_frames() {
    let device = test_device();
    let db = test_db();
    let fasm = test_fasm();
    let bytes = emit_bitstream(&fasm, &device, &db).unwrap();
    let emitted = emit_frames(&fasm, &device, &db).unwrap();
    let parsed = parse_frames(&bytes).unwrap();
    assert_eq!(parsed, emitted);
    assert_eq!(parsed.len(), 4);
    assert_eq!(parsed[&F2][3], 0x0000_0010);
    assert_eq!(parsed[&F1][1], 0x8000_0000);
    assert_eq!(parsed[&F3A][0], 0x0000_0001);
    assert_eq!(parsed[&F3D][3], 0x0000_0800);
}

#[test]
fn all_zero_valued_feature_is_dropped_on_parse_back() {
    // A valued feature whose value is all zero sets no bits and is
    // indistinguishable from an absent feature. This pins the documented
    // limit: parse-back drops the A6LUT line. The AFF.INIT.V1 line in
    // the output is NOT a failure of that limit: the AFF INIT bit reads
    // 0 and V1 is the exact complement of V0, so the FF INIT value is
    // observable and renders (bit = NOT(INIT), so bit 0 is INIT 1).
    let device = test_device();
    let db = test_db();
    let fasm = "SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h0000000000000000\n";
    let bytes = emit_bitstream(fasm, &device, &db).unwrap();
    let back = parse_bitstream(&bytes, &device, &db).unwrap();
    assert_eq!(back, "SLICE_X0Y0.AFF.INIT.V1\n");
}

// ---------------------------------------------------------------------------
// All-`!` valueless features (critic session 6 D1).
//
// The real `segbits_int.db` BOUNCE_E_0_FT1 mux (prjuray-db, verified
// against the on-disk clone): six selections each set exactly one bit
// of 64_00, 66_00, 68_00, 72_00, 72_01, 74_00, and VCC_WIRE sets
// nothing and requires all six clear. Frame numbers below are offsets
// from the INT tile base FAR 0x300, so the config order lists
// 0x340, 0x342, 0x344, 0x348, 0x34A.
// ---------------------------------------------------------------------------

fn bounce_device() -> StubDevice {
    StubDevice::new(
        "xcku3p",
        Some(0x04A6_3093),
        vec![("INT_X0Y0".to_string(), "INT".to_string(), 0x300)],
        vec![0x340, 0x342, 0x344, 0x348, 0x34A],
    )
}

fn bounce_db() -> StubSegbitsDb {
    StubSegbitsDb::new(vec![
        (
            "INT".to_string(),
            "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1".to_string(),
            vec![
                BitRef::set(64, 0),
                BitRef::zero(66, 0),
                BitRef::zero(68, 0),
                BitRef::zero(72, 0),
                BitRef::zero(72, 1),
                BitRef::zero(74, 0),
            ],
        ),
        (
            "INT".to_string(),
            "PIP.BOUNCE_E_0_FT1.INODE_E_5_FT1".to_string(),
            vec![
                BitRef::zero(64, 0),
                BitRef::zero(66, 0),
                BitRef::set(68, 0),
                BitRef::zero(72, 0),
                BitRef::zero(72, 1),
                BitRef::zero(74, 0),
            ],
        ),
        (
            "INT".to_string(),
            "PIP.BOUNCE_E_0_FT1.INODE_E_9_FT1".to_string(),
            vec![
                BitRef::zero(64, 0),
                BitRef::zero(66, 0),
                BitRef::zero(68, 0),
                BitRef::zero(72, 0),
                BitRef::set(72, 1),
                BitRef::zero(74, 0),
            ],
        ),
        (
            "INT".to_string(),
            "PIP.BOUNCE_E_0_FT1.INODE_E_BLN_54_FT1".to_string(),
            vec![
                BitRef::zero(64, 0),
                BitRef::zero(66, 0),
                BitRef::zero(68, 0),
                BitRef::set(72, 0),
                BitRef::zero(72, 1),
                BitRef::zero(74, 0),
            ],
        ),
        (
            "INT".to_string(),
            "PIP.BOUNCE_E_0_FT1.INODE_E_BLN_58_FT1".to_string(),
            vec![
                BitRef::zero(64, 0),
                BitRef::set(66, 0),
                BitRef::zero(68, 0),
                BitRef::zero(72, 0),
                BitRef::zero(72, 1),
                BitRef::zero(74, 0),
            ],
        ),
        (
            "INT".to_string(),
            "PIP.BOUNCE_E_0_FT1.INODE_E_BLN_62_FT1".to_string(),
            vec![
                BitRef::zero(64, 0),
                BitRef::zero(66, 0),
                BitRef::zero(68, 0),
                BitRef::zero(72, 0),
                BitRef::zero(72, 1),
                BitRef::set(74, 0),
            ],
        ),
        (
            "INT".to_string(),
            "PIP.BOUNCE_E_0_FT1.VCC_WIRE".to_string(),
            vec![
                BitRef::zero(64, 0),
                BitRef::zero(66, 0),
                BitRef::zero(68, 0),
                BitRef::zero(72, 0),
                BitRef::zero(72, 1),
                BitRef::zero(74, 0),
            ],
        ),
    ])
}

#[test]
fn all_bang_mux_routed_case_round_trips() {
    // D1 error mode: before the fix, parse-back errored with
    // MustBeZeroViolated for VCC_WIRE because 64_00 reads 1 when the
    // mux is routed to INODE_E_1_FT1. A `!` bit reading 1 must mean
    // the feature is absent (another selection holds), not an error.
    let device = bounce_device();
    let db = bounce_db();
    let fasm = "INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1\n";
    let bytes = emit_bitstream(fasm, &device, &db).unwrap();
    let back = parse_bitstream(&bytes, &device, &db).unwrap();
    assert_eq!(back, fasm);
}

#[test]
fn all_bang_mux_untouched_case_round_trips() {
    // D1 spurious-lines mode: before the fix, parse-back emitted a
    // VCC_WIRE line for the untouched mux (all-zero frames), breaking
    // round-trip exactness. An all-`!` feature with no exact complement
    // sets no bits, so it is indistinguishable from absent and must not
    // be emitted. (An all-`!` feature WITH a complement, such as an FF
    // .V1, does render; VCC_WIRE has none: no single feature sets
    // exactly its six bits with no `!` bits.)
    let device = bounce_device();
    let db = bounce_db();
    let bytes = emit_bitstream("", &device, &db).unwrap();
    let back = parse_bitstream(&bytes, &device, &db).unwrap();
    assert_eq!(back, "");
}

#[test]
fn all_bang_feature_explicitly_selected_is_dropped_on_parse_back() {
    // VCC_WIRE sets no bits and has no exact complement, so it is
    // indistinguishable from absent (same boundary as the all-zero-value
    // limit for valued features). Emit accepts it (it constrains the
    // mux bits to zero) but parse-back drops it.
    let device = bounce_device();
    let db = bounce_db();
    let fasm = "INT_X0Y0.PIP.BOUNCE_E_0_FT1.VCC_WIRE\n";
    let bytes = emit_bitstream(fasm, &device, &db).unwrap();
    let back = parse_bitstream(&bytes, &device, &db).unwrap();
    assert_eq!(back, "");
}

// ---------------------------------------------------------------------------
// FF INIT polarity (researcher session 1, resolved from f4pga/prjuray).
//
// Mapping: INIT value 0 selects .V0, which SETS the bit; INIT value 1
// selects .V1, which CLEARS the bit. bit = NOT(INIT). SRVAL is the
// same. Source chain: tools/dump_features.tcl:897-902 names the feature
// from the cell INIT property, so the V suffix IS the attribute value
// (INIT 1'b0 gives V0, INIT 1'b1 gives V1); utils/spec/flipflops.py:100
// instantiates .INIT(1'b0) or .INIT(1'b1); vendored DB lines
// CLEL_L.AFF.INIT.V0 12_02, V1 !12_02 and BFF.INIT.V0 12_04,
// V1 !12_04 confirm V0 sets and V1 clears. Bit positions below are
// those real vendored positions (frame offset 12, bits 2 and 4), not
// the stub scheme test_db uses for AFF (bit 64, kept clear of the
// LUT bits 63..0).
// ---------------------------------------------------------------------------

fn polarity_device() -> StubDevice {
    StubDevice::new(
        "xcku3p",
        Some(0x04A6_3093),
        vec![("SLICE_X0Y0".to_string(), "CLEL_L".to_string(), 0x500)],
        vec![0x50C],
    )
}

fn polarity_db() -> StubSegbitsDb {
    StubSegbitsDb::new(vec![
        (
            "CLEL_L".to_string(),
            "AFF.INIT.V0".to_string(),
            vec![BitRef::set(12, 2)],
        ),
        (
            "CLEL_L".to_string(),
            "AFF.INIT.V1".to_string(),
            vec![BitRef::zero(12, 2)],
        ),
        (
            "CLEL_L".to_string(),
            "BFF.INIT.V0".to_string(),
            vec![BitRef::set(12, 4)],
        ),
        (
            "CLEL_L".to_string(),
            "BFF.INIT.V1".to_string(),
            vec![BitRef::zero(12, 4)],
        ),
    ])
}

#[test]
fn ff_init_polarity_renders_v0_for_bit_one_and_v1_for_bit_zero() {
    // A bit reading 1 renders .V0 (INIT 0); a bit reading 0 renders
    // .V1 (INIT 1). No inversion is applied in the parse path: the
    // DB's V0/V1 features already encode the polarity (V0 sets, V1
    // clears), so rendering the matching feature renders the right
    // value. The bare AFF.INIT spelling is non-canonical and must
    // not appear.
    let device = polarity_device();
    let db = polarity_db();
    let fasm = "SLICE_X0Y0.AFF.INIT.V0\nSLICE_X0Y0.BFF.INIT.V1\n";
    let bytes = emit_bitstream(fasm, &device, &db).unwrap();
    let back = parse_bitstream(&bytes, &device, &db).unwrap();
    assert_eq!(back, fasm);
    assert!(!back.contains("AFF.INIT\n") && !back.contains("BFF.INIT\n"));
}
