//! xczu3eg.json regression test: the vendored BitFiltrator file must
//! load, validate, and resolve BELs, and must NOT be provisional: it
//! is a verbatim BitFiltrator copy, independently verified against
//! the public prjuray-db tilegrid by `tools/verify_xczu3eg.py`
//! (35 of 38 checks pass; the 3 failures are the 12 tilegrid columns
//! that own frames in part.json but have no owning tile, a tilegrid
//! owner gap, not a file defect; see data/README.md).
//!
//! Hand-checked numbers (not taken from the code under test):
//! - Header: 8,820 slices, 70,560 LUTs, 141,120 FFs, 216 BRAMs,
//!   360 DSPs, 1 SLR; IDCODE 0x04a42093; max FAR row 2.
//! - Frame total 14,952; UG1085 Table 1 (PL Bitstream Length) lists
//!   14,964 for ZU3, gap 12 = 4 per FAR row x 3 rows, the same
//!   systematic pattern as xcku3p (16 = 4 x 4). Real bitstreams
//!   carry 14,952 real frames plus 12 padding frames in the FDRI
//!   packet (decoder lane: fdri_frames=14964 padding_skipped=12),
//!   so the gap is explained, not a file defect. Do not tune the
//!   constant to match UG1085; the file's own total is the loader's
//!   truth.
//! - The INT column table (`xczu3eg_int_columns.json`, 99 entries)
//!   is tilegrid-derived ground truth: without it the provisional
//!   INT rule mispredicts INT_X2 and above on this part.

use helion_ultrascale::bitgen::{BitgenDevice, BitgenError, emit_frames};
use helion_ultrascale::segbits::FileSegbitsDb;
use helion_ultrascale::{
    ArchSummary, BitLocator, DeviceFile, IntColumnTable, IntfColumnTable,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn load() -> (DeviceFile, ArchSummary) {
    let data = manifest_dir().join("data");
    let device = DeviceFile::from_path(&data.join("xczu3eg.json")).unwrap();
    let arch = ArchSummary::from_path(&data.join("ULTRASCALE_PLUS.json")).unwrap();
    (device, arch)
}

fn load_int_table() -> IntColumnTable {
    IntColumnTable::from_path(&manifest_dir().join("data").join("xczu3eg_int_columns.json"))
        .unwrap()
}

#[test]
fn xczu3eg_is_not_provisional_with_header_counts() {
    let (device, _) = load();
    assert!(!device.is_provisional());
    assert!(device.provisional_notes().is_empty());
    // DS891 ZU3EG column, device file headers, and the tilegrid all
    // agree (see tools/verify_xczu3eg.py sections 2 and 4).
    assert_eq!(device.part(), "xczu3eg-sbva484-1-e");
    assert_eq!(device.device(), "xczu3eg");
    assert_eq!(device.num_slices(), 8_820);
    assert_eq!(device.num_luts(), 70_560);
    assert_eq!(device.num_regs(), 141_120);
    assert_eq!(device.num_brams(), 216);
    assert_eq!(device.num_dsps(), 360);
    assert_eq!(device.num_slrs(), 1);
    // UG1085 / real Vivado bitstreams (see data/README.md): the
    // AMD forum figure 0x14710093 is the PS TAP IDCODE, not the PL
    // configuration IDCODE written in the bitstream.
    assert_eq!(device.slr_idcode("SLR0").unwrap(), 0x04A4_2093);
    assert_eq!(device.max_far_row_idx("SLR0").unwrap(), 2);
}

#[test]
fn xczu3eg_structure_validates_and_frame_total_is_14952() {
    let (device, _) = load();
    let checked = device
        .validate_structure()
        .expect("structure must validate");
    assert!(checked > 0);
    // The file's own total. UG1085 Table 1 lists 14,964 for ZU3;
    // the 12-frame gap is 4 per FAR row x 3 rows, explained by 12
    // padding frames in real bitstreams (see the module doc). Do
    // not tune this constant to match UG1085.
    assert_eq!(device.total_frames(), 14_952);
    assert_eq!(device.total_frames() + 4 * 3, 14_964);
    assert_eq!(14_964 - device.total_frames(), 12);
}

#[test]
fn xczu3eg_locates_bels_across_all_rows() {
    let (device, arch) = load();
    let loc = BitLocator::new(&device, &arch);
    // First slice, first row.
    let (slr, fars, ofsts) = loc.locate_lut("SLICE_X0Y0/A6LUT").unwrap();
    assert_eq!(slr, "SLR0");
    assert_eq!(fars.len(), 64);
    assert_eq!(ofsts.len(), 64);
    // Last row (row 2, Y 179), last CLB column of a 49-column row.
    let (slr, fars, _) = loc.locate_lut("SLICE_X48Y179/A6LUT").unwrap();
    assert_eq!(slr, "SLR0");
    assert_eq!(fars.len(), 64);
    // A FF in the middle of the die.
    let (slr, _, _) = loc.locate_reg("SLICE_X25Y90/BFF").unwrap();
    assert_eq!(slr, "SLR0");
    // BRAM: first site and last site (X 5, Y 71, row 2).
    let (slr, mem_fars, mem_ofsts, par_fars, par_ofsts) = loc.locate_bram("RAMB18_X0Y0").unwrap();
    assert_eq!(slr, "SLR0");
    assert_eq!(mem_fars.len(), 16384);
    assert_eq!(par_fars.len(), 2048);
    let _ = (mem_ofsts, par_ofsts);
    let (_, mem_fars, _, par_fars, _) = loc.locate_bram("RAMB18_X5Y71").unwrap();
    assert_eq!(mem_fars.len(), 16384);
    assert_eq!(par_fars.len(), 2048);
}

#[test]
fn xczu3eg_int_table_loads_and_attaches() {
    let table = load_int_table();
    assert_eq!(table.part, "xczu3eg-sbva484-1-e");
    // 33 INT columns x 3 FAR rows.
    assert_eq!(table.columns.len(), 99);
    // Tilegrid anchors: row 0 X0 -> col 1, row 1 X11 -> col 35.
    assert_eq!(table.columns[&(0, 0)], 1);
    assert_eq!(table.columns[&(1, 11)], 35);
    assert_eq!(table.columns[&(0, 2)], 7);
    assert_eq!(table.columns.get(&(0, 33)), None);

    let (mut device, _) = load();
    assert!(!device.has_int_columns());
    assert_eq!(device.int_column(0, 0), None);
    device.set_int_columns(table).unwrap();
    assert!(device.has_int_columns());
    assert_eq!(device.int_column(0, 0), Some(1));
    assert_eq!(device.int_column(1, 11), Some(35));
    assert_eq!(device.int_column(0, 33), None);
    assert_eq!(device.int_column_entries().unwrap().len(), 99);
}

#[test]
fn xczu3eg_int_table_rejects_wrong_part() {
    let (mut device, _) = load();
    let wrong = IntColumnTable {
        part: "xcku3p-ffva676-1-e".to_string(),
        columns: BTreeMap::new(),
    };
    assert!(device.set_int_columns(wrong).is_err());
    assert!(!device.has_int_columns());
}

fn load_intf_table() -> IntfColumnTable {
    IntfColumnTable::from_path(&manifest_dir().join("data").join("xczu3eg_intf_columns.json"))
        .unwrap()
}

#[test]
fn xczu3eg_intf_table_loads_and_attaches() {
    let table = load_intf_table();
    assert_eq!(table.part, "xczu3eg-sbva484-1-e");
    // 4 tile types: INT_INTF_R_PCIE4 (4 columns), INT_INTF_LEFT_TERM_PSS
    // (3), HDIO_BOT_RIGHT (4), HDIO_TOP_RIGHT (4). Verified against
    // the tilegrid by tools/verify_xczu3eg.py section 11 (283 windowed
    // tiles, 0 conflicts).
    assert_eq!(table.columns.len(), 15);
    // Tilegrid anchors: R_PCIE4 row 2 X9 -> col 29, R_PCIE4 row 0 X24
    // -> col 75, LEFT_TERM_PSS row 0 X0 -> col 0, HDIO_BOT_RIGHT row 0
    // X9 -> col 30, HDIO_TOP_RIGHT row 2 X24 -> col 76.
    assert_eq!(
        table.columns[&("INT_INTF_R_PCIE4".to_string(), 2, 9)],
        29
    );
    assert_eq!(
        table.columns[&("INT_INTF_R_PCIE4".to_string(), 0, 24)],
        75
    );
    assert_eq!(
        table.columns[&("INT_INTF_LEFT_TERM_PSS".to_string(), 0, 0)],
        0
    );
    assert_eq!(table.columns[&("HDIO_BOT_RIGHT".to_string(), 0, 9)], 30);
    // X24 exists only in row 0 for both HDIO types (one tile per
    // row: X24Y0); rows 1 and 2 have no X24 HDIO tile.
    assert_eq!(table.columns[&("HDIO_TOP_RIGHT".to_string(), 0, 24)], 76);
    // Tile types with no tilegrid windows are not in the table and
    // must not resolve.
    assert_eq!(
        table.columns.get(&("INT_INTF_L".to_string(), 0, 0)),
        None
    );
    assert_eq!(
        table.columns.get(&("INT_INTF_R_PCIE4".to_string(), 2, 10)),
        None
    );

    let (mut device, _) = load();
    assert!(!device.has_intf_columns());
    assert_eq!(device.intf_column("INT_INTF_R_PCIE4", 2, 9), None);
    device.set_intf_columns(table).unwrap();
    assert!(device.has_intf_columns());
    assert_eq!(device.intf_column("INT_INTF_R_PCIE4", 2, 9), Some(29));
    assert_eq!(device.intf_column("INT_INTF_R_PCIE4", 0, 24), Some(75));
    assert_eq!(
        device.intf_column("INT_INTF_LEFT_TERM_PSS", 0, 0),
        Some(0)
    );
    assert_eq!(device.intf_column("HDIO_BOT_RIGHT", 0, 9), Some(30));
    assert_eq!(device.intf_column("HDIO_TOP_RIGHT", 0, 24), Some(76));
    assert_eq!(device.intf_column("INT_INTF_L", 0, 0), None);
    assert_eq!(device.intf_column("INT_INTF_R_PCIE4", 2, 10), None);
    assert_eq!(device.intf_column_entries().unwrap().len(), 15);
}

#[test]
fn xczu3eg_intf_table_rejects_wrong_part() {
    let (mut device, _) = load();
    let wrong = IntfColumnTable {
        part: "xcku3p-ffva676-1-e".to_string(),
        columns: BTreeMap::new(),
    };
    assert!(device.set_intf_columns(wrong).is_err());
    assert!(!device.has_intf_columns());
}

/// The xczu3eg device exactly as the CLI loads it: INT table plus
/// INT_INTF/HDIO table attached.
fn load_cli_device() -> DeviceFile {
    let (mut device, _) = load();
    device.set_int_columns(load_int_table()).unwrap();
    device.set_intf_columns(load_intf_table()).unwrap();
    device
}

/// The vendored segbits DBs exactly as the CLI loads them, in the
/// same order (see SEGBITS_FILES in src/bin/helion-bitgen.rs),
/// including the Helion-derived INT_INTF_R_PCIE4 file.
fn load_cli_segbits() -> FileSegbitsDb {
    let data = manifest_dir().join("data");
    let names = [
        "segbits_int.db",
        "segbits_hpio_right.db",
        "segbits_hdio_top_right.db",
        "segbits_hdio_bot_right.db",
        "segbits_cmt_right.db",
        "segbits_clel_l.db",
        "segbits_clel_r.db",
        "segbits_clem.db",
        "segbits_clem_r.db",
        "segbits_int_intf_r_pcie4.db",
    ];
    let paths: Vec<PathBuf> = names.iter().map(|n| data.join(n)).collect();
    let refs: Vec<&std::path::Path> = paths.iter().map(PathBuf::as_path).collect();
    let mut db = FileSegbitsDb::from_files(&refs).unwrap();
    db.synthesize_lut_init().unwrap();
    db
}

#[test]
fn xczu3eg_intintf_outputs_enabled_emits_corpus_bits() {
    // INT_INTF_R_PCIE4_X9Y120 is the offset-0 tile (row 2, col 29):
    // the emitter's segbits path writes the listed bit as the
    // absolute frame bit, so only offset-0 tiles land correctly
    // (documented F3-class limitation; the decoder, which adds the
    // window offset, is the authority for attribution).
    let device = load_cli_device();
    assert_eq!(
        device.tile_base_far("INT_INTF_R_PCIE4_X9Y120"),
        Some(0x81D00)
    );
    let db = load_cli_segbits();
    let fasm = "INT_INTF_R_PCIE4_X9Y120.OUTPUTS_ENABLED.1\n";
    let frames = emit_frames(fasm, &device, &db).unwrap();
    // Exactly the 3 corpus positions, at the FARs the decoder
    // reports for the 5 BNN designs (P-13 through P-17): frame 1
    // bit 21, frame 2 bit 27, frame 3 bit 24. The band step may set
    // ECC bits in the same frames, so assert the positions are set,
    // not that they are the only bits.
    let mut keys: Vec<u32> = frames.keys().copied().collect();
    keys.sort_unstable();
    assert_eq!(keys, vec![0x81D01, 0x81D02, 0x81D03]);
    let bit_is_set = |far: u32, bit: u16| -> bool {
        let words = &frames[&far];
        (words[(bit / 32) as usize] >> (bit % 32)) & 1 == 1
    };
    assert!(bit_is_set(0x81D01, 21));
    assert!(bit_is_set(0x81D02, 27));
    assert!(bit_is_set(0x81D03, 24));
    // The inverse value clears the same bits (OUTPUTS_ENABLED.0 is
    // the same bits negated): emitting it sets no bit anywhere.
    // (write_frame_bit materializes the frame entries even for a
    // clear, so the map holds 3 all-zero frames; the assertion is on
    // set bits, not on map emptiness.)
    let frames0 = emit_frames(
        "INT_INTF_R_PCIE4_X9Y120.OUTPUTS_ENABLED.0\n",
        &device,
        &db,
    )
    .unwrap();
    let set_bits0: u32 = frames0
        .values()
        .flat_map(|w| w.iter())
        .map(|word| word.count_ones())
        .sum();
    assert_eq!(set_bits0, 0);
}

#[test]
fn xczu3eg_intintf_pip_still_fails_unknown_feature() {
    // B1, the proven-blocked PIP: the tile now resolves (B2 fixed),
    // but no public segbits pin the IMUX PIP, so the emitter must
    // still fail loud with UnknownFeature, not UnknownTile. This
    // test documents the blocked state; it flips to a pass only when
    // a Vivado-built design pins the PIP segbits.
    let device = load_cli_device();
    assert!(device.tile_type("INT_INTF_R_PCIE4_X9Y0").is_some());
    let db = load_cli_segbits();
    let err = emit_frames(
        "INT_INTF_R_PCIE4_X9Y0.PIP.IMUXOUT16.IMUX16\n",
        &device,
        &db,
    )
    .unwrap_err();
    match err {
        BitgenError::UnknownFeature { tile, feature } => {
            assert_eq!(tile, "INT_INTF_R_PCIE4_X9Y0");
            assert_eq!(feature, "PIP.IMUXOUT16.IMUX16");
        }
        other => panic!("expected UnknownFeature, got {other:?}"),
    }
}
