//! Mixed-column parse test (N16): a column holding a RAM-mode LUT
//! and a logic-mode LUT of the same letter.
//!
//! MODE.RAM bits are shared by all 60 slices of a column (F1), so
//! parse renders MODE.RAM on every slice tile in the column and
//! masks the logic LUT's INIT bits too. The stored band (emitted
//! with the logic bits unmasked) then differs from parse's masked
//! prediction, and parse used to fail loud with BandMismatch on a
//! valid emitted bitstream. N16 accepts the difference when it is a
//! subset XOR of the ambiguous masked columns with GF(2) rank at
//! most MAX_AMBIGUOUS_RANK (24) in `band_code::check_stored_band`:
//! the pinned mixed case has rank 2 and parses clean now, while a
//! dense mixed column (rank 32 on every INIT frame) still fails loud
//! with BandMismatch, as pinned by `mixed_column_dense_still_fails_loud`.
//!
//! Measured 2026-11-21 on the stub DB below: RAM on SLICE_X0Y0
//! letter A plus logic INIT on SLICE_X0Y20 letter A used to fail
//! parse with BandMismatch on FAR 0x60b, stored 0x088000008088 (the
//! MAP-A column of the Y20 INIT bit), expected 0x000000000000
//! (masked). The logic-only control parses clean. MODE.RAM still
//! renders on both slice tiles in the parsed FASM (F1, out of scope
//! for N16); only the band verdict changed.
//!
//! No em dashes anywhere.

use helion_ultrascale::BitRef;
use helion_ultrascale::bitgen::{
    BitgenError, StubDevice, StubSegbitsDb, emit_bitstream, parse_bitstream,
};
use helion_ultrascale::bitloc::{ClbTileType, lut_locations};

fn stub_db() -> StubSegbitsDb {
    let mut lut: Vec<BitRef> = Vec::new();
    for bit in (0..64).rev() {
        lut.push(BitRef::set(12, bit));
    }
    StubSegbitsDb::new(vec![
        ("CLEM".to_string(), "A6LUT.INIT[63:0]".to_string(), lut),
        (
            "CLEM".to_string(),
            "ALUT.MODE.RAM".to_string(),
            vec![BitRef::set(9, 19), BitRef::set(10, 19)],
        ),
    ])
}

fn mixed_device() -> StubDevice {
    StubDevice::new(
        "xcku3p",
        Some(0x04A6_3093),
        vec![
            ("SLICE_X0Y0".to_string(), "CLEM".to_string(), 0x600),
            ("SLICE_X0Y20".to_string(), "CLEM".to_string(), 0x600),
        ],
        vec![],
    )
}

fn dense_device() -> StubDevice {
    let mut tiles = vec![("SLICE_X0Y0".to_string(), "CLEM".to_string(), 0x600)];
    for y in 20..30 {
        tiles.push((format!("SLICE_X0Y{y}"), "CLEM".to_string(), 0x600));
    }
    StubDevice::new("xcku3p", Some(0x04A6_3093), tiles, vec![])
}

#[test]
fn mixed_column_ram_plus_logic_parses_clean() {
    let db = stub_db();
    let device = mixed_device();
    let loc = lut_locations(ClbTileType::Clem, 20, 0).unwrap()[0];
    let y20_far = 0x600 + loc.minor;
    assert_eq!(y20_far, 0x60b);

    // Control: logic INIT on Y20 only, no RAM anywhere. Parses clean.
    let logic_only = "SLICE_X0Y20.A6LUT.INIT[63:0] = 64'h0000000000000001\n";
    let bytes = emit_bitstream(logic_only, &device, &db).unwrap();
    assert!(
        parse_bitstream(&bytes, &device, &db).is_ok(),
        "logic-only control must parse clean"
    );

    // Mixed: RAM on Y0 (letter A) plus logic INIT on Y20 (letter A).
    // The stored band on FAR 0x60b carries the Y20 INIT bit's MAP-A
    // column (0x088000008088) while parse's over-masked prediction
    // is 0. N16 accepts it: the difference is a one-column subset
    // of the two ambiguous masked columns, rank 2, within the bound.
    let mixed = "SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h0000000000000001\n\
                 SLICE_X0Y0.ALUT.MODE.RAM\n\
                 SLICE_X0Y20.A6LUT.INIT[63:0] = 64'h0000000000000001\n";
    let bytes = emit_bitstream(mixed, &device, &db).unwrap();
    let fasm = parse_bitstream(&bytes, &device, &db).expect("mixed column must parse clean");
    assert!(
        fasm.contains("SLICE_X0Y0.A6LUT.INIT[63:0]"),
        "Y0 INIT line missing: {fasm}"
    );
    assert!(
        fasm.contains("SLICE_X0Y20.A6LUT.INIT[63:0]"),
        "Y20 INIT line missing: {fasm}"
    );
    assert!(
        fasm.contains("ALUT.MODE.RAM"),
        "MODE.RAM line missing: {fasm}"
    );
}

#[test]
fn mixed_column_dense_still_fails_loud() {
    // RAM on Y0 (letter A) plus logic INIT on Y20..Y29 (letter A).
    // Every INIT frame (minors 8..11) carries 160 ambiguous masked
    // set bits whose MAP-A columns have rank 32, above
    // MAX_AMBIGUOUS_RANK (24), so parse must still fail loud with
    // BandMismatch instead of silently accepting the band. One
    // asymmetry is deliberate: the 16 columns of one slice in one
    // minor XOR to zero, so a symmetric all-ones design carries a
    // zero band on every INIT frame and parses clean despite the
    // rank; clearing Y29 INIT bit 0 makes the minor-11 band
    // 0x888008880888, nonzero, while the rank stays 32.
    let db = stub_db();
    let device = dense_device();
    let mut fasm = String::from(
        "SLICE_X0Y0.A6LUT.INIT[63:0] = 64'hFFFFFFFFFFFFFFFF\nSLICE_X0Y0.ALUT.MODE.RAM\n",
    );
    for y in 20..29 {
        fasm.push_str(&format!(
            "SLICE_X0Y{y}.A6LUT.INIT[63:0] = 64'hFFFFFFFFFFFFFFFF\n"
        ));
    }
    fasm.push_str("SLICE_X0Y29.A6LUT.INIT[63:0] = 64'hFFFFFFFFFFFFFFFE\n");
    let bytes = emit_bitstream(&fasm, &device, &db).unwrap();
    let err = parse_bitstream(&bytes, &device, &db).unwrap_err();
    assert!(
        matches!(err, BitgenError::BandMismatch { .. }),
        "dense mixed column must fail loud as BandMismatch, got: {err}"
    );
}
