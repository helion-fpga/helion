//! DSP48E2 fail-loud regression tests.
//!
//! No DSP48E2 config-bit encoding exists in any public source (search
//! record: `crates/helion-ultrascale/data/DSP48E2_SEARCH.md`). A design
//! using a DSP48E2 must therefore error with the specific
//! `BitgenError::Dsp48e2Blocked` variant, never with a generic
//! `UnknownFeature` / `UnknownTileType`, and never emit a bitstream.
//!
//! The FASM spellings below are test inputs to the detector only. They
//! are not claimed to be real FASM vocabulary: no DSP vocabulary exists
//! until Vivado fuzzing job 2 lands the encoding.

use helion_ultrascale::BitRef;
use helion_ultrascale::bitgen::{
    BitgenError, StubDevice, StubSegbitsDb, emit_bitstream, emit_frames,
};

fn test_device() -> StubDevice {
    StubDevice::new(
        "xcku3p",
        Some(0x04A6_3093),
        vec![
            ("SLICE_X0Y0".to_string(), "CLEL_L".to_string(), 0x500),
            // The DSP tile resolves in this stub on purpose: the guard
            // must fire even when the device knows the tile.
            ("DSP48E2_X0Y0".to_string(), "DSP".to_string(), 0x700),
            ("DSP_X0Y0".to_string(), "DSP".to_string(), 0x700),
        ],
        vec![0x500, 0x700],
    )
}

fn test_db() -> StubSegbitsDb {
    let mut lut: Vec<BitRef> = Vec::new();
    for bit in (0..64).rev() {
        lut.push(BitRef::set(12, bit));
    }
    StubSegbitsDb::new(vec![
        (
            "CLEL_L".to_string(),
            "A6LUT.INIT[63:0]".to_string(),
            lut,
        ),
        // A hypothetical DSP segbits entry, planted to prove the guard
        // fires before lookup: even if a DB entry existed, the emitter
        // must not trust it, because no public source provides it.
        (
            "DSP".to_string(),
            "AREG".to_string(),
            vec![BitRef::set(0, 0)],
        ),
    ])
}

fn blocked(tile: &str, feature: &str, err: &BitgenError) {
    assert_eq!(
        err,
        &BitgenError::Dsp48e2Blocked {
            tile: tile.to_string(),
            feature: feature.to_string(),
        },
        "expected Dsp48e2Blocked, got {err}"
    );
    let msg = err.to_string();
    assert!(msg.contains("DSP48E2"), "message names DSP48E2: {msg}");
    assert!(msg.contains("fuzzing job 2"), "message names job 2: {msg}");
    assert!(
        msg.contains("refusing to emit guessed bits"),
        "message states the refusal: {msg}"
    );
}

#[test]
fn dsp_site_tile_errors_with_dsp_blocked() {
    let device = test_device();
    let db = test_db();
    let err = emit_frames("DSP48E2_X0Y0.AREG = 1\n", &device, &db).unwrap_err();
    blocked("DSP48E2_X0Y0", "AREG", &err);
}

#[test]
fn dsp_site_tile_errors_even_with_db_entry() {
    // The stub DB above carries DSP/AREG. The guard must still fire:
    // a segbits entry with no public source behind it is not an
    // encoding.
    let device = test_device();
    let db = test_db();
    let err = emit_frames("DSP48E2_X0Y0.AREG = 1\n", &device, &db).unwrap_err();
    blocked("DSP48E2_X0Y0", "AREG", &err);
}

#[test]
fn dsp_tile_pip_errors_with_dsp_blocked() {
    let device = test_device();
    let db = test_db();
    let err = emit_frames(
        "DSP_X0Y0.PIP.DSP_DSP48_0_A0.DSP_DSP48_0_A0_PIN\n",
        &device,
        &db,
    )
    .unwrap_err();
    blocked(
        "DSP_X0Y0",
        "PIP.DSP_DSP48_0_A0.DSP_DSP48_0_A0_PIN",
        &err,
    );
}

#[test]
fn dsp_feature_on_dsp_tile_errors_with_dsp_blocked() {
    let device = test_device();
    let db = test_db();
    let err = emit_frames("DSP_X0Y0.DSP48E2.AREG = 1\n", &device, &db).unwrap_err();
    blocked("DSP_X0Y0", "DSP48E2.AREG", &err);
}

#[test]
fn bare_dsp_attribute_errors_with_dsp_blocked() {
    let device = test_device();
    let db = test_db();
    let err = emit_frames("SLICE_X0Y0.USE_MULT = 1\n", &device, &db).unwrap_err();
    blocked("SLICE_X0Y0", "USE_MULT", &err);
}

#[test]
fn dsp_line_among_good_lines_still_blocks_and_emits_nothing() {
    let device = test_device();
    let db = test_db();
    let fasm = "SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h8000000000000001\nDSP48E2_X0Y0.AREG = 1\n";
    let err = emit_frames(fasm, &device, &db).unwrap_err();
    blocked("DSP48E2_X0Y0", "AREG", &err);
    // End to end: emit_bitstream returns the same error, so no bytes
    // are produced for a design that uses a DSP.
    let err = emit_bitstream(fasm, &device, &db).unwrap_err();
    blocked("DSP48E2_X0Y0", "AREG", &err);
}

#[test]
fn non_dsp_unknown_feature_is_still_unknown_feature() {
    // The detector must not swallow ordinary unknown features: those
    // stay UnknownFeature, so a typo keeps its old diagnosis.
    let device = test_device();
    let db = test_db();
    let err = emit_frames("SLICE_X0Y0.BOGUS_FEATURE\n", &device, &db).unwrap_err();
    assert_eq!(
        err,
        BitgenError::UnknownFeature {
            tile: "SLICE_X0Y0".to_string(),
            feature: "BOGUS_FEATURE".to_string(),
        }
    );
}

#[test]
fn good_lut_design_still_emits() {
    // The guard must not change behavior for non-DSP designs.
    let device = test_device();
    let db = test_db();
    let frames = emit_frames(
        "SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h8000000000000001\n",
        &device,
        &db,
    )
    .unwrap();
    assert_eq!(frames.len(), 1);
    let frame = frames.get(&0x50C).unwrap();
    assert_eq!(frame[0], 0x0000_0001);
    assert_eq!(frame[1], 0x8000_0000);
}
