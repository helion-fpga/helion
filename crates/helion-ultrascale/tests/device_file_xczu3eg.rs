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

use helion_ultrascale::{ArchSummary, BitLocator, DeviceFile, IntColumnTable};
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
