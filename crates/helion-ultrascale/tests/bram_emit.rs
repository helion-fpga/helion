//! BRAM emission tests: whole-array and per-word INIT/INITP lines
//! through `emit_frames` on the real xcku3p device file.
//!
//! Expected positions are hand-checked against the vendored
//! BitFiltrator tables (`ultrascale_plus_lut_ff_bram.json`, Y_ofst 0),
//! not taken from the code under test:
//! content bit 0 is (minor 0, ofst 0), bit 1 is (0, 12), bit 255 is
//! (3, 107), bit 256 is (4, 0), bit 512 is (8, 0), bit 16128 is
//! (252, 0), bit 16383 is (255, 107); parity bit 0 is (0, 48),
//! bit 255 is (31, 59), bit 256 is (32, 48), bit 2047 is (255, 59).
//! RAMB18_X0Y0 has base FAR 0x01000000 (block 1, row 0, col 0,
//! minor 0), so FAR = 0x01000000 + minor.
//!
//! The known pattern is the golden-design pattern: INIT word 0
//! all-ones, word 1 all-zero, words 2..63 alternating 0xAA/0x55
//! words (same shape for the 8 parity words). Content set bits:
//! 256 + 0 + 62*128 = 8192. Parity set bits: 256 + 0 + 6*128 = 1024.
//!
//! No em dashes anywhere.

use helion_ultrascale::bitgen::{BitgenError, emit_bitstream, emit_frames, parse_bitstream};
use helion_ultrascale::{DeviceFile, FileSegbitsDb};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data")
}

fn load_device() -> DeviceFile {
    DeviceFile::from_path(&data_dir().join("xcku3p.json")).unwrap()
}

fn load_db() -> FileSegbitsDb {
    // BRAM lines never consult segbits; any loaded DB stands in.
    FileSegbitsDb::from_file(&data_dir().join("segbits_int.db")).unwrap()
}

/// INIT words (256 bits each) of the golden pattern: word 0 all-ones,
/// word 1 all-zero, even words 0xAA bytes, odd words 0x55 bytes.
fn pattern_u64_words(n_init_words: usize) -> Vec<u64> {
    let mut words = Vec::with_capacity(n_init_words * 4);
    for w in 0..n_init_words {
        let fill: u64 = match w {
            0 => u64::MAX,
            1 => 0,
            _ if w % 2 == 0 => 0xAAAA_AAAA_AAAA_AAAA,
            _ => 0x5555_5555_5555_5555,
        };
        words.extend([fill; 4]);
    }
    words
}

/// Format little-endian u64 words (word 0 = lowest bits) as one
/// upper-case hex literal, MSB first.
fn words_to_hex(words: &[u64]) -> String {
    let mut s = String::new();
    for w in words.iter().rev() {
        s.push_str(&format!("{w:016X}"));
    }
    s
}

fn golden_fasm() -> String {
    let content = pattern_u64_words(64);
    let parity = pattern_u64_words(8);
    format!(
        "RAMB18_X0Y0.INIT[16383:0] = 0x{}\nRAMB18_X0Y0.INITP[2047:0] = 0x{}\n",
        words_to_hex(&content),
        words_to_hex(&parity),
    )
}

fn frame_bit(frames: &BTreeMap<u32, [u32; 93]>, far: u32, bit: u16) -> bool {
    match frames.get(&far) {
        Some(words) => (words[(bit / 32) as usize] >> (bit % 32)) & 1 == 1,
        None => false,
    }
}

fn count_set_bits(frames: &BTreeMap<u32, [u32; 93]>) -> usize {
    frames
        .values()
        .map(|words| words.iter().map(|w| w.count_ones() as usize).sum::<usize>())
        .sum()
}

#[test]
fn bram_whole_array_known_pattern_spot_bits() {
    let device = load_device();
    let db = load_db();
    let frames = emit_frames(&golden_fasm(), &device, &db).unwrap();
    // Content word 0 (all ones): bits 0, 1, 255 set.
    assert!(frame_bit(&frames, 0x0100_0000, 0), "content bit 0");
    assert!(frame_bit(&frames, 0x0100_0000, 12), "content bit 1");
    assert!(frame_bit(&frames, 0x0100_0003, 107), "content bit 255");
    // Content word 1 (all zero): bits 256, 257, 511 clear.
    assert!(!frame_bit(&frames, 0x0100_0004, 0), "content bit 256");
    assert!(!frame_bit(&frames, 0x0100_0004, 12), "content bit 257");
    assert!(!frame_bit(&frames, 0x0100_0007, 107), "content bit 511");
    // Content word 2 (0xAA bytes): bit 512 is 0, bit 513 is 1.
    assert!(!frame_bit(&frames, 0x0100_0008, 0), "content bit 512");
    assert!(frame_bit(&frames, 0x0100_0008, 12), "content bit 513");
    // Content word 63 (0x55 bytes): bit 16128 is 1, bit 16383 is 0.
    assert!(frame_bit(&frames, 0x0100_00FC, 0), "content bit 16128");
    assert!(!frame_bit(&frames, 0x0100_00FF, 107), "content bit 16383");
    // Parity word 0 (all ones): bits 0, 255 set.
    assert!(frame_bit(&frames, 0x0100_0000, 48), "parity bit 0");
    assert!(frame_bit(&frames, 0x0100_001F, 59), "parity bit 255");
    // Parity word 1 (all zero): bit 256 clear.
    assert!(!frame_bit(&frames, 0x0100_0020, 48), "parity bit 256");
    // Parity word 7 (0x55 bytes): bit 2047 is 0.
    assert!(!frame_bit(&frames, 0x0100_00FF, 59), "parity bit 2047");
    // 8192 content + 1024 parity set bits, no extras: the FASM sets
    // nothing but BRAM bits, and content/parity positions are disjoint.
    assert_eq!(count_set_bits(&frames), 8192 + 1024);
    // Every touched frame is a BRAM content frame of column 0.
    for far in frames.keys() {
        assert!((0x0100_0000..0x0100_0100).contains(far), "far {far:#x}");
    }
}

#[test]
fn bram_per_word_spelling_composes() {
    let device = load_device();
    let db = load_db();
    // INIT_00 = 1 sets only absolute content bit 0; INITP_07 with
    // only its top bit set sets only absolute parity bit 2047.
    let fasm = "RAMB18_X0Y0.INIT_00 = 256'h1\n\
                RAMB18_X0Y0.INITP_07 = 256'h8000000000000000000000000000000000000000000000000000000000000000\n";
    let frames = emit_frames(fasm, &device, &db).unwrap();
    assert!(frame_bit(&frames, 0x0100_0000, 0));
    assert!(frame_bit(&frames, 0x0100_00FF, 59));
    assert_eq!(count_set_bits(&frames), 2);
    // Two per-word lines on one tile compose: words 0 and 1 all-ones
    // set absolute bits 0..511 and nothing else.
    let fasm = "RAMB18_X0Y0.INIT_00 = 256'hFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF\n\
                RAMB18_X0Y0.INIT_01 = 256'hFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF\n";
    let frames = emit_frames(fasm, &device, &db).unwrap();
    assert!(frame_bit(&frames, 0x0100_0000, 0));
    assert!(frame_bit(&frames, 0x0100_0007, 107), "content bit 511");
    assert!(!frame_bit(&frames, 0x0100_0008, 0), "content bit 512");
    assert_eq!(count_set_bits(&frames), 512);
}

#[test]
fn bram_emit_bitstream_smoke() {
    let device = load_device();
    let db = load_db();
    let bytes = emit_bitstream(&golden_fasm(), &device, &db).unwrap();
    assert!(!bytes.is_empty());
    assert!(bytes.windows(4).any(|w| w == [0xAA, 0x99, 0x55, 0x66]));
}

#[test]
fn bram_fail_loud() {
    let device = load_device();
    let db = load_db();
    // RAMB36 tiles do not resolve: unknown tile, before any feature.
    let err = emit_frames("RAMB36_X0Y0.INIT[32767:0] = 0x1\n", &device, &db).unwrap_err();
    assert_eq!(
        err,
        BitgenError::UnknownTile {
            tile: "RAMB36_X0Y0".to_string()
        }
    );
    // Unknown BRAM tile column.
    let err = emit_frames("RAMB18_X99Y0.INIT[16383:0] = 0x1\n", &device, &db).unwrap_err();
    assert_eq!(
        err,
        BitgenError::UnknownTile {
            tile: "RAMB18_X99Y0".to_string()
        }
    );
    // Spellings that are not whole-array or a valid per-word line.
    for feature in [
        "INIT[8191:0]",
        "INIT",
        "INIT_40",
        "INITP_08",
        "INITP[1023:0]",
    ] {
        let fasm = format!("RAMB18_X0Y0.{feature} = 0x1\n");
        let err = emit_frames(&fasm, &device, &db).unwrap_err();
        assert_eq!(
            err,
            BitgenError::UnknownFeature {
                tile: "RAMB18_X0Y0".to_string(),
                feature: feature.to_string(),
            },
            "{feature}"
        );
    }
    // A BRAM INIT line with no value.
    let err = emit_frames("RAMB18_X0Y0.INIT[16383:0]\n", &device, &db).unwrap_err();
    assert!(
        matches!(err, BitgenError::BadValue { .. }),
        "valueless BRAM INIT: {err:?}"
    );
    // A per-word value wider than the word.
    let fasm = format!("RAMB18_X0Y0.INIT_00 = 0x1{}\n", "0".repeat(64));
    let err = emit_frames(&fasm, &device, &db).unwrap_err();
    assert!(
        matches!(err, BitgenError::ValueTooWide { bits: 256, .. }),
        "257-bit per-word value: {err:?}"
    );
    // Two spellings writing different values to one bit conflict.
    let fasm = "RAMB18_X0Y0.INIT_00 = 256'hFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF\n\
                RAMB18_X0Y0.INIT[16383:0] = 0x0\n";
    let err = emit_frames(fasm, &device, &db).unwrap_err();
    assert!(
        matches!(
            err,
            BitgenError::ConflictingBit {
                far: 0x0100_0000,
                bit: 0,
                ..
            }
        ),
        "conflicting BRAM writes: {err:?}"
    );
}

#[test]
fn bram_roundtrip_parse_back() {
    let device = load_device();
    let db = load_db();
    let fasm = golden_fasm();
    let bytes = emit_bitstream(&fasm, &device, &db).unwrap();
    // Parse-back must not fail loud on the set BRAM bits (the
    // UnexplainedBits failure this commit fixes), and must render the
    // two whole-array lines with the exact golden values, in the
    // Verilog spelling the emitter renders.
    let back = parse_bitstream(&bytes, &device, &db).unwrap();
    let content = pattern_u64_words(64);
    let parity = pattern_u64_words(8);
    let expect_content = format!(
        "RAMB18_X0Y0.INIT[16383:0] = 16384'h{}\n",
        words_to_hex(&content)
    );
    let expect_parity = format!(
        "RAMB18_X0Y0.INITP[2047:0] = 2048'h{}\n",
        words_to_hex(&parity)
    );
    assert!(
        back.contains(&expect_content),
        "parse-back content line missing:\n{back}"
    );
    assert!(
        back.contains(&expect_parity),
        "parse-back parity line missing:\n{back}"
    );
    // Re-emitting the parse-back reproduces the original frames.
    let frames_a = emit_frames(&fasm, &device, &db).unwrap();
    let frames_b = emit_frames(&back, &device, &db).unwrap();
    assert_eq!(frames_a, frames_b, "emit(parse(emit(fasm))) frames differ");
}

#[test]
fn bram_parse_back_all_zero_drops_line() {
    let device = load_device();
    let db = load_db();
    // An all-zero array is indistinguishable from absent (valued
    // presence is any-set), so parse-back drops the line rather than
    // rendering a zero value. The frames carry no set BRAM bit, so
    // nothing is unexplained either.
    let fasm = "RAMB18_X0Y0.INIT[16383:0] = 0x0\nRAMB18_X0Y0.INITP[2047:0] = 0x0\n";
    let bytes = emit_bitstream(fasm, &device, &db).unwrap();
    let back = parse_bitstream(&bytes, &device, &db).unwrap();
    assert!(
        !back.contains("RAMB18_X0Y0"),
        "all-zero BRAM must render no line, got:\n{back}"
    );
}

#[test]
fn bram_bits_subcommand_labels() {
    // End to end through the built binary: emit the golden design,
    // then `bits` must label every set bit BRAM_CONTENT/BRAM_PARITY
    // with the golden counts (8192 content, 1024 parity).
    let device = load_device();
    let db = load_db();
    let dir = std::env::temp_dir().join("helion-bram-bits-test");
    std::fs::create_dir_all(&dir).unwrap();
    let bit_path = dir.join("bram.bit");
    let bytes = emit_bitstream(&golden_fasm(), &device, &db).unwrap();
    std::fs::write(&bit_path, &bytes).unwrap();
    let exe = env!("CARGO_BIN_EXE_helion-bitgen");
    let out = std::process::Command::new(exe)
        .args([
            "bits",
            "--bit",
            bit_path.to_str().unwrap(),
            "--part",
            "xcku3p",
            "--data-dir",
            data_dir().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "bits subcommand failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    let mut content = 0usize;
    let mut parity = 0usize;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let _far = parts.next().unwrap();
        let _bit = parts.next().unwrap();
        let _val = parts.next().unwrap();
        match parts.next().unwrap() {
            "BRAM_CONTENT" => content += 1,
            "BRAM_PARITY" => parity += 1,
            other => panic!("unexpected block label {other} in line {line}"),
        }
    }
    assert_eq!(content, 8192, "content bit count");
    assert_eq!(parity, 1024, "parity bit count");
}
