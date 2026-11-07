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
//!   RAMB18_X0Y0.INIT_00 = 256'h8000...0001
//!   SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h8000000000000001
//!   SLICE_X0Y0.AFF.INIT.V0
//!
//! Stub device (xcku3p, IDCODE 0x04A63093 per UG570 table 2):
//!   SLICE_X0Y0  type CLEL_L  base FAR 0x00000500 (block 0, row 0, col 5, minor 0)
//!   INT_X0Y0    type INT     base FAR 0x00000300 (block 0, row 0, col 3, minor 0)
//!   RAMB18_X0Y0 type BRAM    base FAR 0x01000000 (block 1, row 0, col 0, minor 0)
//!   config order (ascending FAR): 0x304, 0x50C, 0x1000000
//!
//! Stub segbits DB (BitRef.frame is a frame offset from the tile base FAR;
//! INIT lists are MSB-first: list index 0 is the most significant value
//! bit, per the brief):
//!   CLEL_L A6LUT.INIT[63:0] : 64 refs, frame offset 12, bits 63..0
//!   CLEL_L AFF.INIT.V0      : 1 ref, frame offset 12, bit 64
//!   CLEL_L AFF.INIT.V1      : 1 ref, frame offset 12, `!` bit 64
//!   INT    PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1 : set bit 100, `!` bits 101, 102
//!   BRAM   INIT_00          : 256 refs, frame offset 0, bits 255..0
//!
//! The FF INIT spelling is the DB vocabulary `.V0`/`.V1` (interfaces
//! critic session 2 item 5), not bare `AFF.INIT`. V0 sets the INIT bit;
//! V1 requires it clear. The value-to-feature polarity mapping is
//! RESOLVED (researcher session 1, verified from the fuzzer source):
//! bit = NOT(INIT) and bit = NOT(SRVAL); V0 is selected by value 0
//! and sets the bit, V1 by value 1 and clears it. This test pins the
//! bit pattern and the value meaning together.
//!
//! Expected frames (hand-computed):
//!   F2 = 0x00000304: word3 = 0x00000010 (frame bit 100), rest 0
//!   F1 = 0x0000050C: word0 = 0x00000001 (bit 0), word1 = 0x80000000 (bit 63),
//!                     word2 = 0x00000001 (bit 64), rest 0
//!   F3 = 0x01000000: word0 = 0x00000001 (bit 0), word7 = 0x80000000 (bit 255)
//!
//! Expected CRCs (Python reference, emit order RCRC, IDCODE, WCFG, frames;
//! the IDCODE write feeds the CRC, so only the first frame's CRC depends on
//! the preamble; later frames start from 0 after the CRC-register write):
//!   F2 = 0x9ac4dc29, F1 = 0xaa75db0b, F3 = 0x66924d7d
//!
//! Expected stream: 733 words = 2932 bytes. Sync word at byte 48 (word 12).
//! Packet word offsets: RCRC header 14, IDCODE header 16, WCFG header 18,
//! F2 FAR 20 / FDRI 22 / FARrw 116 / CRC 118 (value 119),
//! F1 FAR 120 / FDRI 122 / FARrw 216 / CRC 218 (value 219),
//! F3 FAR 220 / FDRI 222 / FARrw 316 / CRC 318 (value 319),
//! ending RCRC 320, GRESTORE 324, DGHIGH 327, START 329, DESYNC 331,
//! trailing NOOPs 333..732.

use helion_ultrascale::BitRef;
use helion_ultrascale::bitgen::{
    SYNC_WORD, StubDevice, StubSegbitsDb, emit_bitstream, emit_frames, parse_bitstream,
    parse_frames,
};

const FASM: &str = "INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1\n\
RAMB18_X0Y0.INIT_00 = 256'h8000000000000000000000000000000000000000000000000000000000000001\n\
SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h8000000000000001\n\
SLICE_X0Y0.AFF.INIT.V0\n";

const F2: u32 = 0x0000_0304;
const F1: u32 = 0x0000_050C;
const F3: u32 = 0x0100_0000;

fn test_device() -> StubDevice {
    StubDevice::new(
        "xcku3p",
        Some(0x04A6_3093),
        vec![
            ("SLICE_X0Y0".to_string(), "CLEL_L".to_string(), 0x500),
            ("INT_X0Y0".to_string(), "INT".to_string(), 0x300),
            ("RAMB18_X0Y0".to_string(), "BRAM".to_string(), 0x100_0000),
        ],
        vec![F2, F1, F3],
    )
}

fn test_db() -> StubSegbitsDb {
    let mut lut: Vec<BitRef> = Vec::new();
    for bit in (0..64).rev() {
        lut.push(BitRef::set(12, bit));
    }
    let mut bram: Vec<BitRef> = Vec::new();
    for bit in (0..256).rev() {
        bram.push(BitRef::set(0, bit));
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
        ("BRAM".to_string(), "INIT_00".to_string(), bram),
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
    let frames = emit_frames(FASM, &device, &db).unwrap();
    assert_eq!(frames.len(), 3);

    let f2 = frames.get(&F2).unwrap();
    assert_eq!(f2[3], 0x0000_0010);
    assert!(f2.iter().enumerate().all(|(i, w)| i == 3 || *w == 0));

    let f1 = frames.get(&F1).unwrap();
    assert_eq!(f1[0], 0x0000_0001);
    assert_eq!(f1[1], 0x8000_0000);
    assert_eq!(f1[2], 0x0000_0001);
    assert!(
        f1.iter()
            .enumerate()
            .all(|(i, w)| (0..3).contains(&i) || *w == 0)
    );

    let f3 = frames.get(&F3).unwrap();
    assert_eq!(f3[0], 0x0000_0001);
    assert_eq!(f3[7], 0x8000_0000);
    assert!(
        f3.iter()
            .enumerate()
            .all(|(i, w)| i == 0 || i == 7 || *w == 0)
    );
}

#[test]
fn bitstream_has_expected_length_sync_headers_and_crcs() {
    let device = test_device();
    let db = test_db();
    let bytes = emit_bitstream(FASM, &device, &db).unwrap();
    assert_eq!(bytes.len(), 733 * 4);

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
    assert_eq!(word(&bytes, 116), 0x3000_2001);
    assert_eq!(word(&bytes, 117), F2);
    assert_eq!(word(&bytes, 118), 0x3000_0001);
    assert_eq!(word(&bytes, 119), 0x9ac4_dc29);

    // Frame F1.
    assert_eq!(word(&bytes, 120), 0x3000_2001);
    assert_eq!(word(&bytes, 121), F1);
    assert_eq!(word(&bytes, 122), 0x3000_405D);
    assert_eq!(word(&bytes, 123), 0x0000_0001);
    assert_eq!(word(&bytes, 124), 0x8000_0000);
    assert_eq!(word(&bytes, 125), 0x0000_0001);
    assert_eq!(word(&bytes, 216), 0x3000_2001);
    assert_eq!(word(&bytes, 217), F1);
    assert_eq!(word(&bytes, 218), 0x3000_0001);
    assert_eq!(word(&bytes, 219), 0xaa75_db0b);

    // Frame F3.
    assert_eq!(word(&bytes, 220), 0x3000_2001);
    assert_eq!(word(&bytes, 221), F3);
    assert_eq!(word(&bytes, 222), 0x3000_405D);
    assert_eq!(word(&bytes, 223), 0x0000_0001);
    assert_eq!(word(&bytes, 230), 0x8000_0000);
    assert_eq!(word(&bytes, 316), 0x3000_2001);
    assert_eq!(word(&bytes, 317), F3);
    assert_eq!(word(&bytes, 318), 0x3000_0001);
    assert_eq!(word(&bytes, 319), 0x6692_4d7d);

    // Ending: RCRC, 2 NOOP, GRESTORE, NOOP, DGHIGH, START, DESYNC,
    // 400 NOOP. START = 5 and DESYNC = 13 are UG570 table 9-22 codes.
    assert_eq!(word(&bytes, 320), 0x3000_8001);
    assert_eq!(word(&bytes, 321), 0x0000_0007);
    assert_eq!(word(&bytes, 322), 0x2000_0000);
    assert_eq!(word(&bytes, 323), 0x2000_0000);
    assert_eq!(word(&bytes, 324), 0x3000_8001);
    assert_eq!(word(&bytes, 325), 0x0000_000A);
    assert_eq!(word(&bytes, 326), 0x2000_0000);
    assert_eq!(word(&bytes, 327), 0x3000_8001);
    assert_eq!(word(&bytes, 328), 0x0000_0003);
    assert_eq!(word(&bytes, 329), 0x3000_8001);
    assert_eq!(word(&bytes, 330), 0x0000_0005);
    assert_eq!(word(&bytes, 331), 0x3000_8001);
    assert_eq!(word(&bytes, 332), 0x0000_000D);
    for i in 333..733 {
        assert_eq!(word(&bytes, i), 0x2000_0000, "trailing NOOP at word {i}");
    }
}

#[test]
fn round_trip_returns_identical_fasm() {
    let device = test_device();
    let db = test_db();
    let bytes = emit_bitstream(FASM, &device, &db).unwrap();
    let back = parse_bitstream(&bytes, &device, &db).unwrap();
    assert_eq!(back, FASM);
}

#[test]
fn parse_frames_matches_emit_frames() {
    let device = test_device();
    let db = test_db();
    let bytes = emit_bitstream(FASM, &device, &db).unwrap();
    let emitted = emit_frames(FASM, &device, &db).unwrap();
    let parsed = parse_frames(&bytes).unwrap();
    assert_eq!(parsed, emitted);
    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed[&F2][3], 0x0000_0010);
    assert_eq!(parsed[&F1][1], 0x8000_0000);
    assert_eq!(parsed[&F3][7], 0x8000_0000);
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
