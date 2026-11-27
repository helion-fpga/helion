//! Decode-side mixed-column test (N7 port): a column holding a
//! RAM-mode LUT and a logic-mode LUT of the same letter.
//!
//! The parse side false-rejects this shape (N16, pinned by the ignored
//! test in `n7_probe_mixed_column.rs`): MODE.RAM bits are shared by all
//! 60 slices of a column in the parse model, so parse renders MODE.RAM
//! on every slice tile and masks the logic LUT's INIT bits too. The
//! decoder does NOT share that limitation: each slice tile owns its own
//! bit window (tilegrid offset steps 3 per slice Y), and real Vivado
//! frames put the MODE.RAM bits at per-slice positions (measured on the
//! P-11/P-13/P-40 corpus: 4,992 cases, per-slice in every case, shared
//! in none). Decode therefore sees MODE.RAM only on the RAM slice's
//! tile, masks only that slice's INIT bits, and accepts the mixed
//! column. This test is NOT ignored: it pins the decode-side behavior.
//!
//! Frames are hand-built (no emitter): the emitter writes MODE.RAM at
//! shared Y0 positions, which is the parse-model encoding, not the
//! Vivado per-slice encoding, so an emitter-built mixed bitstream
//! would test the emitter's MODE.RAM placement, not the decoder's
//! mask. The band on each frame is computed here with
//! `compute_band_map_a` under the intended mask, the same computation
//! the emitter performs.
//!
//! No em dashes anywhere.

use helion_ultrascale::band_code::{compute_band_map_a, write_band};
use helion_ultrascale::bitgen::FRAME_BITS;
use helion_ultrascale::bitstream_decode::decode_frames;
use helion_ultrascale::bitstream_packets::Frame;
use std::path::PathBuf;

/// Two CLEM tiles sharing base FAR 0x600: SLICE_X0Y0 at offset 0 and
/// SLICE_X0Y20 at offset 60 (bit window 960..1008).
const TILEGRID: &str = r#"{
  "CLEM_X0Y0": {"bits": {"CLB_IO_CLK": {"baseaddr": "0x00000600", "frames": 16, "offset": 0, "words": 3}}, "grid_x": 0, "grid_y": 0, "pin_functions": {}, "sites": {"SLICE_X0Y0": "SLICEM"}, "type": "CLEM"},
  "CLEM_X0Y20": {"bits": {"CLB_IO_CLK": {"baseaddr": "0x00000600", "frames": 16, "offset": 60, "words": 3}}, "grid_x": 0, "grid_y": 20, "pin_functions": {}, "sites": {"SLICE_X0Y20": "SLICEM"}, "type": "CLEM"}
}"#;

fn segbits_text() -> String {
    // Per-bit A-LUT INIT lines, real-DB refs: INIT[63] -> (08,00),
    // INIT[0] -> (11,15); MODE.RAM set (09,19), ! (11,18).
    let mut s = String::new();
    for i in 0..64 {
        let k = 63 - i;
        s.push_str(&format!(
            "CLEM.ALUT.INIT[{i}] {:02}_{:02}\n",
            8 + k / 16,
            k % 16
        ));
    }
    s.push_str("CLEM.ALUT.MODE.RAM 09_19 !11_18\n");
    s
}

fn fixture_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("helion_decode_mixed_{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("tilegrid.json"), TILEGRID).unwrap();
    std::fs::write(dir.join("segbits_clem.db"), segbits_text()).unwrap();
    dir
}

/// One frame with exactly the given absolute bits set, carrying the
/// MAP-A band computed under `mask` (LUTRAM mask for this frame).
fn frame_with_band(far: u32, set_bits: &[u16], mask: &[bool; FRAME_BITS]) -> Frame {
    let mut words = [0u32; 93];
    for b in set_bits {
        words[(*b as usize) / 32] |= 1u32 << (b % 32);
    }
    let band = compute_band_map_a(&words, mask);
    write_band(&mut words, band);
    Frame {
        far,
        words: words.to_vec(),
    }
}

/// RAM on SLICE_X0Y0 letter A (INIT bit 0 at FAR 0x60b bit 15,
/// MODE.RAM set at FAR 0x609 bit 19) plus logic INIT on SLICE_X0Y20
/// letter A (INIT bit 0 at FAR 0x60b bit 975). The 0x60b band is
/// computed with only the Y0 INIT position masked. Decode must accept
/// the mixed column: MODE.RAM on the Y0 tile only, both INIT lines
/// back, zero unexplained bits.
#[test]
fn mixed_column_ram_plus_logic_decodes_clean() {
    let dir = fixture_dir("mixed");
    let mut mask_60b = [false; FRAME_BITS];
    mask_60b[15] = true;
    let frames = vec![
        frame_with_band(0x609, &[19], &[false; FRAME_BITS]),
        frame_with_band(0x60b, &[15, 975], &mask_60b),
    ];
    let out = decode_frames(&frames, dir.to_str().unwrap()).unwrap();
    assert_eq!(
        out.fasm,
        vec![
            "SLICE_X0Y0.A6LUT.INIT[63:0] = 0x1".to_string(),
            "SLICE_X0Y0.ALUT.MODE.RAM".to_string(),
            "SLICE_X0Y20.A6LUT.INIT[63:0] = 0x1".to_string(),
        ]
    );
    assert!(
        out.unexplained.is_empty(),
        "unexplained: {:?}",
        out.unexplained
    );
}

/// Same frames, but the 0x60b band is stored unmasked (both INIT
/// columns). Under the N16 per-bit-subset acceptance this now decodes
/// clean, and that is intended, not a silent error: the decoder masks
/// the Y0 INIT bit, expects only the Y20 column, and the stored band
/// differs from that expectation by exactly col(15), the column of
/// the one masked set bit, rank 1, inside the bound of 24. The
/// decoded FASM is correct and complete, both INIT lines and MODE.RAM
/// on the Y0 tile only, with zero unexplained bits, identical to the
/// masked case above. The old fail-loud expectation was the N16
/// false-reject behavior on the decode side; the corrupted-band test
/// below is now the pin that the band check still fails loud.
#[test]
fn mixed_column_with_unmasked_band_decodes_clean() {
    let dir = fixture_dir("mixed-unmasked");
    let frames = vec![
        frame_with_band(0x609, &[19], &[false; FRAME_BITS]),
        frame_with_band(0x60b, &[15, 975], &[false; FRAME_BITS]),
    ];
    let out = decode_frames(&frames, dir.to_str().unwrap()).unwrap();
    assert_eq!(
        out.fasm,
        vec![
            "SLICE_X0Y0.A6LUT.INIT[63:0] = 0x1".to_string(),
            "SLICE_X0Y0.ALUT.MODE.RAM".to_string(),
            "SLICE_X0Y20.A6LUT.INIT[63:0] = 0x1".to_string(),
        ]
    );
    assert!(
        out.unexplained.is_empty(),
        "unexplained: {:?}",
        out.unexplained
    );
}

/// Same frames, but the 0x60b stored band is corrupted by one bit off
/// any subset XOR: the band is computed with only the Y0 INIT bit
/// masked (expected col(975), ambiguous subset XORs col(975) and
/// col(975)^col(15)), then band bit 0 is flipped, so the stored band
/// is col(975)^1. That delta is in no subset span, with or without Q,
/// so decode must still fail loud with BandMismatch naming FAR 0x60b.
/// This pins that the N16 acceptance is a bounded subset check, not
/// a blanket accept of any band on a masked frame.
#[test]
fn mixed_column_with_corrupted_band_fails_loud() {
    let dir = fixture_dir("mixed-corrupted");
    let mut mask_60b = [false; FRAME_BITS];
    mask_60b[15] = true;
    let mut frame_60b = frame_with_band(0x60b, &[15, 975], &mask_60b);
    frame_60b.words[45] ^= 1;
    let frames = vec![
        frame_with_band(0x609, &[19], &[false; FRAME_BITS]),
        frame_60b,
    ];
    let err = decode_frames(&frames, dir.to_str().unwrap()).unwrap_err();
    assert!(err.contains("BandMismatch"), "err: {err}");
    assert!(err.contains("0x0000060b"), "err: {err}");
}

/// Logic-only control: no MODE.RAM anywhere, both INIT bits feed the
/// band unmasked. Decodes clean with both INIT lines and no MODE.RAM.
#[test]
fn logic_only_column_decodes_clean() {
    let dir = fixture_dir("logic-only");
    let frames = vec![frame_with_band(0x60b, &[15, 975], &[false; FRAME_BITS])];
    let out = decode_frames(&frames, dir.to_str().unwrap()).unwrap();
    assert_eq!(
        out.fasm,
        vec![
            "SLICE_X0Y0.A6LUT.INIT[63:0] = 0x1".to_string(),
            "SLICE_X0Y20.A6LUT.INIT[63:0] = 0x1".to_string(),
        ]
    );
    assert!(
        out.unexplained.is_empty(),
        "unexplained: {:?}",
        out.unexplained
    );
}
