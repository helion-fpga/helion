//! Per-block tests for LUT INIT, FF INIT, and BRAM content/parity emission.
//!
//! Every expected (minor, frame_ofst) pair in this file was read by hand
//! directly from the BitFiltrator source JSON
//! (`bitfiltrator-ref/resources/archs/ULTRASCALE_PLUS.json`) with python3,
//! not from the code under test. The vendored copy in
//! `crates/helion-ultrascale/data/` is the same data, verified verbatim at
//! extraction time. One further hand check comes from the paper itself:
//! section V-C works `SLICE_X136Y247/A6LUT` bit 0 as minor 11, frame
//! offset 351, which the table reproduces at `Y_ofst` 7.

use helion_ultrascale::bitloc::{
    BitLoc, BitLocError, Block, BramHalfContent, BramTileUse, ClbTileType, LutMode,
    bram_mem_location, bram_mem_locations, bram_mem_placed_bits, bram_parity_location,
    bram_parity_locations, bram_parity_placed_bits, bram_tile_fasm, bram18_fasm,
    bram36_placed_bits, ff_fasm_name, ff_init_fasm, ff_location, ff_placed_bit, ff_srval_fasm,
    ff_srval_location, ff_srval_placed_bit, lut_fasm_name, lut_init_fasm, lut_locations,
    lut_placed_bits,
};
use helion_ultrascale::segbits::{BitRef, FileSegbitsDb, SegbitsDb};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// LUT
// ---------------------------------------------------------------------------

#[test]
fn lut_a6lut_y0_hand_checked_bits() {
    // Read by hand from CLEL_L LutLoc Y_ofst 0, A6LUT.
    let locs = lut_locations(ClbTileType::ClelL, 0, 0).unwrap();
    assert_eq!(
        locs[0],
        BitLoc {
            minor: 11,
            frame_ofst: 15
        }
    );
    assert_eq!(
        locs[1],
        BitLoc {
            minor: 10,
            frame_ofst: 15
        }
    );
    assert_eq!(
        locs[4],
        BitLoc {
            minor: 11,
            frame_ofst: 14
        }
    );
    assert_eq!(
        locs[63],
        BitLoc {
            minor: 8,
            frame_ofst: 0
        }
    );
    // The Y0 A6LUT pattern read off the table: frame_ofst falls 15..0 in
    // groups of four, minor cycles 11, 10, 9, 8.
    for (bit, loc) in locs.iter().enumerate() {
        assert_eq!(loc.frame_ofst, 15 - (bit as u32 / 4));
        assert_eq!(loc.minor, 11 - (bit as u32 % 4));
    }
    // All 64 pairs distinct: two INIT bits never share one frame bit.
    let pairs: HashSet<(u32, u32)> = locs.iter().map(|l| (l.minor, l.frame_ofst)).collect();
    assert_eq!(pairs.len(), 64);
}

#[test]
fn lut_h6lut_y0_hand_checked_bit() {
    let locs = lut_locations(ClbTileType::ClelL, 0, 7).unwrap();
    assert_eq!(
        locs[0],
        BitLoc {
            minor: 11,
            frame_ofst: 47
        }
    );
}

#[test]
fn lut_paper_vc_worked_example() {
    // Paper section V-C: SLICE_X136Y247/A6LUT bit 0 is in minor column 11
    // at frame offset 351. Y_ofst is 247 mod 60 = 7.
    let locs = lut_locations(ClbTileType::ClelL, 247, 0).unwrap();
    assert_eq!(
        locs[0],
        BitLoc {
            minor: 11,
            frame_ofst: 351
        }
    );
    assert_eq!(
        locs[63],
        BitLoc {
            minor: 8,
            frame_ofst: 336
        }
    );
    // Same slice reached through the wrapped Y.
    assert_eq!(lut_locations(ClbTileType::ClelL, 7, 0).unwrap(), locs);
    // Y wraps every 60 slices.
    assert_eq!(
        lut_locations(ClbTileType::ClelL, 60, 0).unwrap(),
        lut_locations(ClbTileType::ClelL, 0, 0).unwrap()
    );
}

#[test]
fn lut_y30_clock_region_jump() {
    // Frame offsets are not a clean 48-per-Y formula: at Y_ofst 30 they
    // jump by an extra 96 (clock-region boundary). Y29 bit 0 is 1407
    // (15 + 48*29); Y30 bit 0 is 1551 (15 + 48*30 + 96).
    let y29 = lut_locations(ClbTileType::ClelL, 29, 0).unwrap();
    let y30 = lut_locations(ClbTileType::ClelL, 30, 0).unwrap();
    assert_eq!(
        y29[0],
        BitLoc {
            minor: 11,
            frame_ofst: 1407
        }
    );
    assert_eq!(
        y30[0],
        BitLoc {
            minor: 11,
            frame_ofst: 1551
        }
    );
    assert_eq!(
        y30[63],
        BitLoc {
            minor: 8,
            frame_ofst: 1536
        }
    );
    // Last Y still fits inside one 93-word frame (2976 bits).
    let y59 = lut_locations(ClbTileType::ClelL, 59, 7).unwrap();
    assert_eq!(
        y59[0],
        BitLoc {
            minor: 11,
            frame_ofst: 2975
        }
    );
}

#[test]
fn lut_tables_identical_across_tile_types() {
    // Verified at extraction; the API must agree for every tile type.
    for tile in [ClbTileType::ClelR, ClbTileType::Clem, ClbTileType::ClemR] {
        assert_eq!(
            lut_locations(tile, 0, 0).unwrap(),
            lut_locations(ClbTileType::ClelL, 0, 0).unwrap()
        );
        assert_eq!(
            ff_location(tile, 0, 0).unwrap(),
            ff_location(ClbTileType::ClelL, 0, 0).unwrap()
        );
    }
}

#[test]
fn lut_xor_pattern_placed_bits() {
    // Brief's test case: XOR pattern 64'h6996966996696996.
    let init: u64 = 0x6996_9669_9669_6996;
    let bits = lut_placed_bits(ClbTileType::ClelL, 0, 0, LutMode::Logic, init).unwrap();
    assert_eq!(bits.len(), 64);
    assert!(bits.iter().all(|b| b.block == Block::LutInit));
    // 32 of the 64 INIT bits are set (every nibble has popcount 2).
    assert_eq!(bits.iter().filter(|b| b.value).count(), 32);
    // LSB nibble 6 = 0b0110: bit 0 clear, bits 1 and 2 set, bit 3 clear.
    assert!(!bits[0].value);
    assert!(bits[1].value);
    assert!(bits[2].value);
    assert!(!bits[3].value);
    // Set bits carry the hand-checked locations.
    assert_eq!(bits[0].minor, 11);
    assert_eq!(bits[0].frame_ofst, 15);
    assert_eq!(bits[1].minor, 10);
    assert_eq!(bits[1].frame_ofst, 15);
    assert_eq!(bits[63].minor, 8);
    assert_eq!(bits[63].frame_ofst, 0);
    // Round trip (self-consistency only, not DB truth): applying the
    // placed bits to a blank map and reading the INIT back gives the
    // input value.
    let mut rebuilt: u64 = 0;
    let locs = lut_locations(ClbTileType::ClelL, 0, 0).unwrap();
    for (bit, placed) in bits.iter().enumerate() {
        assert_eq!(
            (placed.minor, placed.frame_ofst),
            (locs[bit].minor, locs[bit].frame_ofst)
        );
        if placed.value {
            rebuilt |= 1u64 << bit;
        }
    }
    assert_eq!(rebuilt, init);
}

#[test]
fn lut_fasm_line() {
    assert_eq!(
        lut_init_fasm(23, 45, 0, 0x6996_9669_9669_6996).unwrap(),
        "SLICE_X23Y45.A6LUT.INIT[63:0] = 64'h6996966996696996"
    );
    assert_eq!(
        lut_init_fasm(0, 0, 7, 0).unwrap(),
        "SLICE_X0Y0.H6LUT.INIT[63:0] = 64'h0000000000000000"
    );
    assert_eq!(lut_fasm_name(0).unwrap(), "A6LUT");
    assert_eq!(lut_fasm_name(7).unwrap(), "H6LUT");
}

#[test]
fn lut_errors_fail_loud() {
    assert_eq!(
        lut_locations(ClbTileType::ClelL, 0, 8),
        Err(BitLocError::LutIndexOutOfRange(8))
    );
    assert_eq!(
        lut_init_fasm(0, 0, 8, 0),
        Err(BitLocError::LutIndexOutOfRange(8))
    );
    assert_eq!(
        "CLEL_X".parse::<ClbTileType>(),
        Err(BitLocError::UnknownTileType("CLEL_X".to_string()))
    );
}

#[test]
fn lutram_blocked_fail_loud() {
    // G4: no LUTRAM table, no verified equivalence. A LUTRAM request
    // must error and emit no bits, never fall back to the LUT table.
    let err = lut_placed_bits(
        ClbTileType::Clem,
        0,
        0,
        LutMode::Lutram,
        0xFFFF_FFFF_FFFF_FFFF,
    );
    assert_eq!(err, Err(BitLocError::LutramBlocked));
}

// ---------------------------------------------------------------------------
// FF
// ---------------------------------------------------------------------------

#[test]
fn ff_y0_hand_checked_bits() {
    // Read by hand from CLEL_L RegLoc Y_ofst 0.
    assert_eq!(
        ff_location(ClbTileType::ClelL, 0, 0).unwrap(),
        BitLoc {
            minor: 12,
            frame_ofst: 2
        }
    ); // AFF (AQ)
    assert_eq!(
        ff_location(ClbTileType::ClelL, 0, 1).unwrap(),
        BitLoc {
            minor: 12,
            frame_ofst: 14
        }
    ); // AFF2 (AQ2)
    assert_eq!(
        ff_location(ClbTileType::ClelL, 0, 2).unwrap(),
        BitLoc {
            minor: 12,
            frame_ofst: 4
        }
    ); // BFF (BQ)
    assert_eq!(
        ff_location(ClbTileType::ClelL, 0, 15).unwrap(),
        BitLoc {
            minor: 12,
            frame_ofst: 46
        }
    ); // HFF2 (HQ2)
    assert_eq!(
        ff_location(ClbTileType::ClelL, 0, 9).unwrap(),
        BitLoc {
            minor: 12,
            frame_ofst: 38
        }
    ); // EFF2 (EQ2)
    // All 16 FFs share minor 12 at Y0 and have distinct frame offsets.
    let mut seen = HashSet::new();
    for ff in 0..16 {
        let loc = ff_location(ClbTileType::ClelL, 0, ff).unwrap();
        assert_eq!(loc.minor, 12);
        assert!(seen.insert(loc.frame_ofst));
    }
    assert_eq!(seen.len(), 16);
}

#[test]
fn ff_y30_hand_checked_bit() {
    // Same +96 clock-region jump as LUTs: AFF at Y30 is 2 + 48*30 + 96.
    assert_eq!(
        ff_location(ClbTileType::ClelL, 30, 0).unwrap(),
        BitLoc {
            minor: 12,
            frame_ofst: 1538
        }
    );
}

#[test]
fn ff_init_zero_and_one() {
    // Resolved polarity (researcher session 1): bit = NOT(INIT).
    // INIT 0 sets the bit, INIT 1 clears it. The session-1 test asserted
    // the inverted values (bit = value); that was the bug this session
    // fixes.
    let zero = ff_placed_bit(ClbTileType::ClelL, 0, 0, false).unwrap();
    assert_eq!(zero.block, Block::FfInit);
    assert_eq!((zero.minor, zero.frame_ofst), (12, 2));
    assert!(zero.value);
    let one = ff_placed_bit(ClbTileType::ClelL, 0, 0, true).unwrap();
    assert_eq!((one.minor, one.frame_ofst), (12, 2));
    assert!(!one.value);
    // Same location for INIT 0 and INIT 1; only the value differs.
    assert_eq!(zero.minor, one.minor);
    assert_eq!(zero.frame_ofst, one.frame_ofst);
}

#[test]
fn ff_fasm_lines() {
    assert_eq!(
        ff_init_fasm(23, 45, 0, false).unwrap(),
        "SLICE_X23Y45.AFF.INIT.V0"
    );
    assert_eq!(
        ff_init_fasm(23, 45, 1, true).unwrap(),
        "SLICE_X23Y45.AFF2.INIT.V1"
    );
    assert_eq!(ff_fasm_name(0).unwrap(), "AFF");
    assert_eq!(ff_fasm_name(1).unwrap(), "AFF2");
    assert_eq!(ff_fasm_name(15).unwrap(), "HFF2");
    assert_eq!(
        ff_location(ClbTileType::ClelL, 0, 16),
        Err(BitLocError::FfIndexOutOfRange(16))
    );
}
/// Verbatim INIT/SRVAL lines for all 16 FFs in CLEL_L and CLEM, copied
/// from prjuray-db `zynqusp/segbits_clel_l.db` and `segbits_clem.db`
/// (commit affbc5e555ebae16475f32e8fb2d6565d4204f3f, CC0 1.0). The CLEL
/// DBs are not vendored in the repo (only int/hpio/hdio/cmt are), so
/// this excerpt is the test's copy of the DB, extracted by script from
/// the clone and never generated from the code under test.
const FF_SEGBITS_FIXTURE: &str = r#"
CLEL_L.AFF.INIT.V0 12_02
CLEL_L.AFF.INIT.V1 !12_02
CLEL_L.AFF.SRVAL.V0 12_01
CLEL_L.AFF.SRVAL.V1 !12_01
CLEL_L.AFF2.INIT.V0 12_14
CLEL_L.AFF2.INIT.V1 !12_14
CLEL_L.AFF2.SRVAL.V0 12_13
CLEL_L.AFF2.SRVAL.V1 !12_13
CLEL_L.BFF.INIT.V0 12_04
CLEL_L.BFF.INIT.V1 !12_04
CLEL_L.BFF.SRVAL.V0 12_05
CLEL_L.BFF.SRVAL.V1 !12_05
CLEL_L.BFF2.INIT.V0 12_16
CLEL_L.BFF2.INIT.V1 !12_16
CLEL_L.BFF2.SRVAL.V0 12_17
CLEL_L.BFF2.SRVAL.V1 !12_17
CLEL_L.CFF.INIT.V0 12_08
CLEL_L.CFF.INIT.V1 !12_08
CLEL_L.CFF.SRVAL.V0 12_07
CLEL_L.CFF.SRVAL.V1 !12_07
CLEL_L.CFF2.INIT.V0 12_20
CLEL_L.CFF2.INIT.V1 !12_20
CLEL_L.CFF2.SRVAL.V0 12_19
CLEL_L.CFF2.SRVAL.V1 !12_19
CLEL_L.DFF.INIT.V0 12_10
CLEL_L.DFF.INIT.V1 !12_10
CLEL_L.DFF.SRVAL.V0 12_11
CLEL_L.DFF.SRVAL.V1 !12_11
CLEL_L.DFF2.INIT.V0 12_22
CLEL_L.DFF2.INIT.V1 !12_22
CLEL_L.DFF2.SRVAL.V0 12_23
CLEL_L.DFF2.SRVAL.V1 !12_23
CLEL_L.EFF.INIT.V0 12_26
CLEL_L.EFF.INIT.V1 !12_26
CLEL_L.EFF.SRVAL.V0 12_25
CLEL_L.EFF.SRVAL.V1 !12_25
CLEL_L.EFF2.INIT.V0 12_38
CLEL_L.EFF2.INIT.V1 !12_38
CLEL_L.EFF2.SRVAL.V0 12_37
CLEL_L.EFF2.SRVAL.V1 !12_37
CLEL_L.FFF.INIT.V0 12_28
CLEL_L.FFF.INIT.V1 !12_28
CLEL_L.FFF.SRVAL.V0 12_29
CLEL_L.FFF.SRVAL.V1 !12_29
CLEL_L.FFF2.INIT.V0 12_40
CLEL_L.FFF2.INIT.V1 !12_40
CLEL_L.FFF2.SRVAL.V0 12_41
CLEL_L.FFF2.SRVAL.V1 !12_41
CLEL_L.GFF.INIT.V0 12_32
CLEL_L.GFF.INIT.V1 !12_32
CLEL_L.GFF.SRVAL.V0 12_31
CLEL_L.GFF.SRVAL.V1 !12_31
CLEL_L.GFF2.INIT.V0 12_44
CLEL_L.GFF2.INIT.V1 !12_44
CLEL_L.GFF2.SRVAL.V0 12_43
CLEL_L.GFF2.SRVAL.V1 !12_43
CLEL_L.HFF.INIT.V0 12_34
CLEL_L.HFF.INIT.V1 !12_34
CLEL_L.HFF.SRVAL.V0 12_35
CLEL_L.HFF.SRVAL.V1 !12_35
CLEL_L.HFF2.INIT.V0 12_46
CLEL_L.HFF2.INIT.V1 !12_46
CLEL_L.HFF2.SRVAL.V0 12_47
CLEL_L.HFF2.SRVAL.V1 !12_47
CLEM.AFF.INIT.V0 12_02
CLEM.AFF.INIT.V1 !12_02
CLEM.AFF.SRVAL.V0 12_01
CLEM.AFF.SRVAL.V1 !12_01
CLEM.AFF2.INIT.V0 12_14
CLEM.AFF2.INIT.V1 !12_14
CLEM.AFF2.SRVAL.V0 12_13
CLEM.AFF2.SRVAL.V1 !12_13
CLEM.BFF.INIT.V0 12_04
CLEM.BFF.INIT.V1 !12_04
CLEM.BFF.SRVAL.V0 12_05
CLEM.BFF.SRVAL.V1 !12_05
CLEM.BFF2.INIT.V0 12_16
CLEM.BFF2.INIT.V1 !12_16
CLEM.BFF2.SRVAL.V0 12_17
CLEM.BFF2.SRVAL.V1 !12_17
CLEM.CFF.INIT.V0 12_08
CLEM.CFF.INIT.V1 !12_08
CLEM.CFF.SRVAL.V0 12_07
CLEM.CFF.SRVAL.V1 !12_07
CLEM.CFF2.INIT.V0 12_20
CLEM.CFF2.INIT.V1 !12_20
CLEM.CFF2.SRVAL.V0 12_19
CLEM.CFF2.SRVAL.V1 !12_19
CLEM.DFF.INIT.V0 12_10
CLEM.DFF.INIT.V1 !12_10
CLEM.DFF.SRVAL.V0 12_11
CLEM.DFF.SRVAL.V1 !12_11
CLEM.DFF2.INIT.V0 12_22
CLEM.DFF2.INIT.V1 !12_22
CLEM.DFF2.SRVAL.V0 12_23
CLEM.DFF2.SRVAL.V1 !12_23
CLEM.EFF.INIT.V0 12_26
CLEM.EFF.INIT.V1 !12_26
CLEM.EFF.SRVAL.V0 12_25
CLEM.EFF.SRVAL.V1 !12_25
CLEM.EFF2.INIT.V0 12_38
CLEM.EFF2.INIT.V1 !12_38
CLEM.EFF2.SRVAL.V0 12_37
CLEM.EFF2.SRVAL.V1 !12_37
CLEM.FFF.INIT.V0 12_28
CLEM.FFF.INIT.V1 !12_28
CLEM.FFF.SRVAL.V0 12_29
CLEM.FFF.SRVAL.V1 !12_29
CLEM.FFF2.INIT.V0 12_40
CLEM.FFF2.INIT.V1 !12_40
CLEM.FFF2.SRVAL.V0 12_41
CLEM.FFF2.SRVAL.V1 !12_41
CLEM.GFF.INIT.V0 12_32
CLEM.GFF.INIT.V1 !12_32
CLEM.GFF.SRVAL.V0 12_31
CLEM.GFF.SRVAL.V1 !12_31
CLEM.GFF2.INIT.V0 12_44
CLEM.GFF2.INIT.V1 !12_44
CLEM.GFF2.SRVAL.V0 12_43
CLEM.GFF2.SRVAL.V1 !12_43
CLEM.HFF.INIT.V0 12_34
CLEM.HFF.INIT.V1 !12_34
CLEM.HFF.SRVAL.V0 12_35
CLEM.HFF.SRVAL.V1 !12_35
CLEM.HFF2.INIT.V0 12_46
CLEM.HFF2.INIT.V1 !12_46
CLEM.HFF2.SRVAL.V0 12_47
CLEM.HFF2.SRVAL.V1 !12_47
"#;

#[test]
fn ff_srval_fasm_and_location() {
    assert_eq!(
        ff_srval_fasm(23, 45, 0, false).unwrap(),
        "SLICE_X23Y45.AFF.SRVAL.V0"
    );
    assert_eq!(
        ff_srval_fasm(23, 45, 2, true).unwrap(),
        "SLICE_X23Y45.BFF.SRVAL.V1"
    );
    // A letter: SRVAL one bit below INIT (AFF INIT 2, SRVAL 1).
    assert_eq!(
        ff_srval_location(ClbTileType::ClelL, 0, 0).unwrap(),
        BitLoc {
            minor: 12,
            frame_ofst: 1
        }
    );
    // B letter: SRVAL one bit above INIT (BFF INIT 4, SRVAL 5).
    assert_eq!(
        ff_srval_location(ClbTileType::ClelL, 0, 2).unwrap(),
        BitLoc {
            minor: 12,
            frame_ofst: 5
        }
    );
    // The delta is a within-slice offset, so it survives the +96
    // clock-region jump: AFF INIT at Y30 is 1538, SRVAL is 1537.
    assert_eq!(
        ff_srval_location(ClbTileType::ClelL, 30, 0).unwrap(),
        BitLoc {
            minor: 12,
            frame_ofst: 1537
        }
    );
    // Placed SRVAL bit: same inverse polarity as INIT.
    let zero = ff_srval_placed_bit(ClbTileType::ClelL, 0, 0, false).unwrap();
    assert_eq!(zero.block, Block::FfInit);
    assert_eq!((zero.minor, zero.frame_ofst), (12, 1));
    assert!(zero.value);
    let one = ff_srval_placed_bit(ClbTileType::ClelL, 0, 0, true).unwrap();
    assert_eq!((one.minor, one.frame_ofst), (12, 1));
    assert!(!one.value);
    assert_eq!(
        ff_srval_location(ClbTileType::ClelL, 0, 16),
        Err(BitLocError::FfIndexOutOfRange(16))
    );
}

#[test]
fn ff_init_srval_resolve_through_segbits() {
    // The emitted FASM feature, resolved through the segbits DB, must set
    // or clear exactly the bit the table locates, with the resolved
    // polarity (bit = NOT(value)): V0 sets, V1 clears, for INIT and
    // SRVAL alike. The DB is the authority here; the table locations,
    // the FASM spelling, and the placed-bit values are all checked
    // against it, for all 16 FFs in both tile types.
    let db = FileSegbitsDb::from_text(FF_SEGBITS_FIXTURE, "ff_init_srval fixture").unwrap();
    assert_eq!(db.feature_count(), 128);
    for (tile_type, tile_key) in [(ClbTileType::ClelL, "CLEL_L"), (ClbTileType::Clem, "CLEM")] {
        for ff in 0..16u8 {
            let name = ff_fasm_name(ff).unwrap();
            // The table location is the DB bit: at Y0 the frame offset
            // equals the bit the DB line carries for this FF.
            let init_loc = ff_location(tile_type, 0, ff).unwrap();
            let srval_loc = ff_srval_location(tile_type, 0, ff).unwrap();
            assert_eq!(init_loc.minor, 12);
            assert_eq!(srval_loc.minor, 12);
            // INIT 0 emits .V0, which the DB resolves to setting the bit.
            assert_eq!(
                ff_init_fasm(23, 45, ff, false).unwrap(),
                format!("SLICE_X23Y45.{name}.INIT.V0")
            );
            assert_eq!(
                db.lookup(tile_key, &format!("{name}.INIT.V0")).unwrap(),
                &[BitRef::set(12, init_loc.frame_ofst as u16)]
            );
            // INIT 1 emits .V1, which the DB resolves to clearing it.
            assert_eq!(
                ff_init_fasm(23, 45, ff, true).unwrap(),
                format!("SLICE_X23Y45.{name}.INIT.V1")
            );
            assert_eq!(
                db.lookup(tile_key, &format!("{name}.INIT.V1")).unwrap(),
                &[BitRef::zero(12, init_loc.frame_ofst as u16)]
            );
            // SRVAL the same, at the derived SRVAL location.
            assert_eq!(
                ff_srval_fasm(23, 45, ff, false).unwrap(),
                format!("SLICE_X23Y45.{name}.SRVAL.V0")
            );
            assert_eq!(
                db.lookup(tile_key, &format!("{name}.SRVAL.V0")).unwrap(),
                &[BitRef::set(12, srval_loc.frame_ofst as u16)]
            );
            assert_eq!(
                ff_srval_fasm(23, 45, ff, true).unwrap(),
                format!("SLICE_X23Y45.{name}.SRVAL.V1")
            );
            assert_eq!(
                db.lookup(tile_key, &format!("{name}.SRVAL.V1")).unwrap(),
                &[BitRef::zero(12, srval_loc.frame_ofst as u16)]
            );
            // The placed bits carry the same polarity: value 0 sets the
            // bit the V0 line sets, value 1 clears it.
            assert!(ff_placed_bit(tile_type, 0, ff, false).unwrap().value);
            assert!(!ff_placed_bit(tile_type, 0, ff, true).unwrap().value);
            assert!(ff_srval_placed_bit(tile_type, 0, ff, false).unwrap().value);
            assert!(!ff_srval_placed_bit(tile_type, 0, ff, true).unwrap().value);
        }
    }
}

// ---------------------------------------------------------------------------
// BRAM
// ---------------------------------------------------------------------------

#[test]
fn bram_mem_y0_hand_checked_bits() {
    // Read by hand from BRAM BramMemLoc Y_ofst 0.
    assert_eq!(
        bram_mem_location(0, 0).unwrap(),
        BitLoc {
            minor: 0,
            frame_ofst: 0
        }
    );
    assert_eq!(
        bram_mem_location(0, 1).unwrap(),
        BitLoc {
            minor: 0,
            frame_ofst: 12
        }
    );
    assert_eq!(
        bram_mem_location(0, 255).unwrap(),
        BitLoc {
            minor: 3,
            frame_ofst: 107
        }
    );
    assert_eq!(
        bram_mem_location(0, 256).unwrap(),
        BitLoc {
            minor: 4,
            frame_ofst: 0
        }
    );
    assert_eq!(
        bram_mem_location(0, 16383).unwrap(),
        BitLoc {
            minor: 255,
            frame_ofst: 107
        }
    );
    assert_eq!(bram_mem_locations(0).unwrap().len(), 16384);
}

#[test]
fn bram_parity_y0_hand_checked_bits() {
    // Read by hand from BRAM BramMemParityLoc Y_ofst 0.
    assert_eq!(
        bram_parity_location(0, 0).unwrap(),
        BitLoc {
            minor: 0,
            frame_ofst: 48
        }
    );
    assert_eq!(
        bram_parity_location(0, 1).unwrap(),
        BitLoc {
            minor: 0,
            frame_ofst: 54
        }
    );
    assert_eq!(
        bram_parity_location(0, 255).unwrap(),
        BitLoc {
            minor: 31,
            frame_ofst: 59
        }
    );
    assert_eq!(
        bram_parity_location(0, 256).unwrap(),
        BitLoc {
            minor: 32,
            frame_ofst: 48
        }
    );
    assert_eq!(
        bram_parity_location(0, 2047).unwrap(),
        BitLoc {
            minor: 255,
            frame_ofst: 59
        }
    );
    assert_eq!(bram_parity_locations(0).unwrap().len(), 2048);
}

#[test]
fn bram_y_ofst_varies_and_wraps() {
    // Y_ofst 1 is a different table (lower vs upper half of a RAMB36
    // column position), not a copy of Y_ofst 0.
    assert_ne!(
        bram_mem_locations(0).unwrap(),
        bram_mem_locations(1).unwrap()
    );
    assert_eq!(
        bram_mem_location(1, 0).unwrap(),
        BitLoc {
            minor: 0,
            frame_ofst: 132
        }
    );
    assert_eq!(
        bram_parity_location(1, 0).unwrap(),
        BitLoc {
            minor: 0,
            frame_ofst: 180
        }
    );
    // Last Y still fits in one frame; Y wraps every 24 sites.
    assert_eq!(
        bram_mem_location(23, 16383).unwrap(),
        BitLoc {
            minor: 255,
            frame_ofst: 2975
        }
    );
    assert_eq!(
        bram_parity_location(23, 2047).unwrap(),
        BitLoc {
            minor: 255,
            frame_ofst: 2927
        }
    );
    assert_eq!(
        bram_mem_locations(24).unwrap(),
        bram_mem_locations(0).unwrap()
    );
}

#[test]
fn bram_small_known_pattern() {
    // Content with only bits 0 and 16383 set, parity with only bit 0 set.
    let mut content = [0u64; 256];
    content[0] = 1;
    content[255] = 1u64 << 63;
    let mut parity = [0u64; 32];
    parity[0] = 1;
    let mem = bram_mem_placed_bits(0, &content).unwrap();
    assert_eq!(mem.len(), 16384);
    assert!(mem.iter().all(|b| b.block == Block::BramContent));
    assert_eq!(mem.iter().filter(|b| b.value).count(), 2);
    assert!(mem[0].value);
    assert_eq!((mem[0].minor, mem[0].frame_ofst), (0, 0));
    assert!(mem[16383].value);
    assert_eq!((mem[16383].minor, mem[16383].frame_ofst), (255, 107));
    assert!(!mem[1].value);
    let par = bram_parity_placed_bits(0, &parity).unwrap();
    assert_eq!(par.len(), 2048);
    assert!(par.iter().all(|b| b.block == Block::BramParity));
    assert_eq!(par.iter().filter(|b| b.value).count(), 1);
    assert_eq!((par[0].minor, par[0].frame_ofst), (0, 48));
}

#[test]
fn bram36_splits_lower_upper_halves() {
    // RAMB36 content words 0..256 go to the lower half, 256..512 to the
    // upper half. One bit set in each half lands at that half's table.
    let mut content = [0u64; 512];
    content[0] = 1; // lower half, absolute bit 0
    content[256] = 1; // upper half, absolute bit 0
    let parity = [0u64; 64];
    let bits = bram36_placed_bits(0, 1, &content, &parity).unwrap();
    assert_eq!(bits.len(), 2 * (16384 + 2048));
    let set: Vec<&_> = bits.iter().filter(|b| b.value).collect();
    assert_eq!(set.len(), 2);
    // Lower half bit 0 at Y0: (0, 0). Upper half bit 0 at Y1: (0, 132).
    assert!(
        set.iter()
            .any(|b| b.block == Block::BramContent && (b.minor, b.frame_ofst) == (0, 0))
    );
    assert!(
        set.iter()
            .any(|b| b.block == Block::BramContent && (b.minor, b.frame_ofst) == (0, 132))
    );
}

#[test]
fn bram_fasm_lines() {
    // One content bit set: the 16384-bit value is 4096 hex digits ending
    // in ...0001; parity all zero is 512 hex zeros.
    let mut half = BramHalfContent::zeros();
    half.content[0] = 1;
    let lines = bram18_fasm(2, 12, &half);
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0],
        format!("RAMB18_X2Y12.INIT[16383:0] = 16384'h{}1", "0".repeat(4095))
    );
    assert_eq!(
        lines[1],
        format!("RAMB18_X2Y12.INITP[2047:0] = 2048'h{}", "0".repeat(512))
    );
    // RAMB36 all zero: 8192 hex zeros of content, 1024 of parity.
    let tile = BramTileUse::Ramb36 {
        content: [0; 512],
        parity: [0; 64],
    };
    let lines = bram_tile_fasm(4, 20, 40, 41, &tile);
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0],
        format!("RAMB36_X4Y20.INIT[32767:0] = 32768'h{}", "0".repeat(8192))
    );
    assert_eq!(
        lines[1],
        format!("RAMB36_X4Y20.INITP[4095:0] = 4096'h{}", "0".repeat(1024))
    );
    // RAMB18 pair with only the lower half configured emits only that
    // half's lines; the upper half is untouched, never defaulted.
    let tile = BramTileUse::Ramb18 {
        lower: Some(BramHalfContent::zeros()),
        upper: None,
    };
    let lines = bram_tile_fasm(2, 0, 12, 13, &tile);
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("RAMB18_X2Y12.INIT[16383:0]"));
    assert!(lines[1].starts_with("RAMB18_X2Y12.INITP[2047:0]"));
}

#[test]
fn bram_errors_fail_loud() {
    assert_eq!(
        bram_mem_location(0, 16384),
        Err(BitLocError::BramIndexOutOfRange {
            index: 16384,
            max: 16384
        })
    );
    assert_eq!(
        bram_parity_location(0, 2048),
        Err(BitLocError::BramIndexOutOfRange {
            index: 2048,
            max: 2048
        })
    );
}
