//! Known limitation of the N7 parse-side LUTRAM mask derivation
//! (ignored test: it pins behavior that is expected to fail).
//!
//! A column that holds both a RAM-mode LUT and a logic-mode LUT of
//! the same letter cannot be told apart from the bitstream alone:
//! MODE.RAM bits are shared by all 60 slices of a column (F1), so
//! parse renders MODE.RAM on every slice tile in the column and
//! masks the logic LUT's INIT bits too. The stored band (emitted
//! with the logic bits unmasked) then differs from parse's masked
//! prediction, and parse fails loud with BandMismatch on a valid
//! emitted bitstream.
//!
//! Measured 2026-11-21 on the stub DB below: RAM on SLICE_X0Y0
//! letter A plus logic INIT on SLICE_X0Y20 letter A fails parse
//! with BandMismatch on FAR 0x60b, stored 0x088000008088 (the MAP-A
//! column of the Y20 INIT bit), expected 0x000000000000 (masked).
//! The logic-only control parses clean. Do not un-ignore this test
//! without fixing the limitation: the parse-side mask derivation
//! cannot distinguish the slices from bits alone, so a fix needs
//! either column-level ambiguity handling in the check or FASM-side
//! information parse does not have.

use helion_ultrascale::bitgen::{
    emit_bitstream, parse_bitstream, BitgenError, StubDevice, StubSegbitsDb,
};
use helion_ultrascale::bitloc::{lut_locations, ClbTileType};
use helion_ultrascale::BitRef;

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

#[test]
#[ignore = "N7 known limitation: F1 column sharing makes parse over-mask logic INIT bits in a mixed RAM+logic column, so parse false-rejects the emitted bitstream with BandMismatch"]
fn mixed_column_ram_plus_logic_false_rejects() {
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
    // Parse fails loud with BandMismatch on the Y20 INIT frame: the
    // stored band carries the Y20 INIT bit's MAP-A column
    // (0x088000008088) while parse's over-masked prediction is 0.
    let mixed = "SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h0000000000000001\n\
                 SLICE_X0Y0.ALUT.MODE.RAM\n\
                 SLICE_X0Y20.A6LUT.INIT[63:0] = 64'h0000000000000001\n";
    let bytes = emit_bitstream(mixed, &device, &db).unwrap();
    let err = parse_bitstream(&bytes, &device, &db).unwrap_err();
    assert!(
        matches!(
            err,
            BitgenError::BandMismatch {
                far: 0x60b,
                stored: 0x088000008088,
                expected: 0
            }
        ),
        "mixed column must fail loud as BandMismatch, got: {err}"
    );
}
