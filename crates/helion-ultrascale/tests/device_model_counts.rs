//! Device-model count cross-checks (device-model worker, GAP-F7).
//!
//! Every number below is hand-checked against the primary sources,
//! not taken from the code under test:
//!
//! - AMD DS890 Table 9 (v4.10): KU3P CLB LUTs 162,720, CLB FFs
//!   325,440, Block RAM Blocks 360, DSP Slices 1,368; VU3P CLB LUTs
//!   394,080, CLB FFs 788,160, Block RAM Blocks 720, UltraRAM
//!   Blocks 320, DSP Slices 2,280. Neither DS890 nor XMP103
//!   publishes a slice count; slice counts here are LUT-derived
//!   (LUTs / 8) and labeled as such.
//! - AMD DS891 (Zynq UltraScale+ MPSoC Overview): ZU3EG CLB LUTs
//!   70,560, CLB FFs 141,120, Block RAM 7.6 Mb, DSP Slices 360,
//!   no UltraRAM. ZU3EG slices 8,820 are geometry-equal (49 CLB
//!   columns x 3 FAR rows x 60), unlike KU3P's usable-vs-geometry
//!   split.
//! - AMD UG570 Table 1: KU3P and KU5P 41,476 configuration frames
//!   each; VU3P 71,820; frame length 93 words. UG570 FAR table:
//!   valid block types are 000 (CLB, I/O, CLK) and 001 (BRAM
//!   content); a normal bitstream does not include 010, 011, or 100.
//! - AMD UG1085 Table 1 (PL Bitstream Length): ZU3 14,964
//!   configuration frames. The xczu3eg file totals 14,952, gap 12
//!   = 4 per FAR row x 3 rows, explained by 12 padding frames in
//!   real bitstreams (decoder lane: fdri_frames=14964
//!   padding_skipped=12).
//! - BitFiltrator xcku3p.json (MIT, byte-identical to
//!   bitfiltrator-ref/resources/devices/xcku3p.json, SHA-256
//!   07e6a7d3d586bf9ae3ad6c5d0959b669c5332af36f473ecd44e169c47512cfbe):
//!   header num_slices 27,120 is the Vivado SLICES part property
//!   (create_device_summary.py) and equals the file geometry
//!   (113 CLB columns x 4 FAR rows x 60 slices). KU3P and KU5P
//!   files share identical rowMajors geometry; KU3P usable LUTs,
//!   DSPs, and BRAMs are exactly 3/4 of KU5P (162,720 vs 216,960;
//!   1,368 vs 1,824; 360 vs 480).
//! - The 16-frame UG570 gap is exactly 4 per FAR row on every
//!   device whose UG570 count was checked 2026-11-19 (xcku3p,
//!   xcku5p, xcau20p, xcau25p: UG570 41,476 each, file 41,460
//!   each, 4 FAR rows each).
//!
//! No em dashes anywhere.

use helion_ultrascale::{
    ArchSummary, BelKind, BitLocator, BitgenDevice, DeviceFile, UltraScaleDevice, Xcku3pSpec,
    Xcvu3pSpec, Xczu3egSpec,
};
use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn load(name: &str) -> DeviceFile {
    DeviceFile::from_path(&manifest_dir().join("data").join(name)).unwrap()
}

fn load_arch() -> ArchSummary {
    ArchSummary::from_path(&manifest_dir().join("data").join("ULTRASCALE_PLUS.json")).unwrap()
}

/// DSP columns in a row, counted by probing contiguous x until the
/// file says the column does not exist. DSP keys are 0..n in the
/// file; the count is asserted against the known value per row.
fn count_dsp_columns(device: &DeviceFile, row: u32) -> usize {
    let mut n = 0;
    while device.dsp_col_major("SLR0", row, n as u32).is_ok() {
        n += 1;
    }
    n
}

#[test]
fn xcku3p_header_slices_are_geometry_sites_not_usable_slices() {
    let device = load("xcku3p.json");
    // Header num_slices equals the file geometry exactly.
    let mut geo_slices = 0usize;
    let mut clb_cols = 0usize;
    for row in 0..=device.max_far_row_idx("SLR0").unwrap() {
        let n = device.num_clb_columns("SLR0", row).unwrap();
        assert_eq!(n, 113, "xcku3p has 113 CLB columns in every row");
        clb_cols += n;
        geo_slices += n * 60;
    }
    assert_eq!(clb_cols, 452);
    assert_eq!(geo_slices, 27_120);
    assert_eq!(device.num_slices(), 27_120);
    assert_eq!(geo_slices as u32, device.num_slices());

    // Usable slices are LUT-derived from the AMD-published counts
    // (DS890 Table 9: 162,720 LUTs, 325,440 FFs), not from the file.
    assert_eq!(Xcku3pSpec::CLB_SLICE_COUNT, 20_340);
    assert_eq!(device.num_luts(), 162_720);
    assert_eq!(device.num_regs(), 325_440);
    assert_eq!(device.num_luts() / 8, 20_340);
    assert_eq!(device.num_regs() / 16, 20_340);
    // The logical device model counts usable slices, and its LUT/FF
    // totals equal the AMD-published header counts.
    let model = UltraScaleDevice::xcku3p();
    assert_eq!(model.count_bels(BelKind::Lut6), 162_720);
    assert_eq!(model.count_bels(BelKind::FdRe), 325_440);
    assert_eq!(model.count_bels(BelKind::Carry8), 20_340);
    // The two slice counts differ by exactly one clock row.
    assert_eq!(device.num_slices() - Xcku3pSpec::CLB_SLICE_COUNT, 6_780);
    assert_eq!(6_780, 113 * 60);
}

#[test]
fn xcku3p_dsp_and_bram_headers_match_table_density() {
    let device = load("xcku3p.json");
    // DSP: 19 columns per row, 76 total; header 1,368 is exactly
    // 18 per column (19 x 18 = 342 per row, x 4 rows).
    let mut dsp_cols = 0usize;
    for row in 0..=3 {
        assert_eq!(count_dsp_columns(&device, row), 19);
        dsp_cols += 19;
    }
    assert_eq!(dsp_cols, 76);
    assert_eq!(device.num_dsps(), 1_368);
    assert_eq!(device.num_dsps() as usize % dsp_cols, 0);
    assert_eq!(device.num_dsps() as usize / dsp_cols, 18);
    // BRAM: 10 columns per row, 40 total; header 360 is exactly
    // 9 per column (10 x 9 = 90 per row, x 4 rows).
    let mut bram_cols = 0usize;
    for row in 0..=3 {
        assert_eq!(device.num_bram_columns("SLR0", row).unwrap(), 10);
        bram_cols += 10;
    }
    assert_eq!(bram_cols, 40);
    assert_eq!(device.num_brams(), 360);
    assert_eq!(device.num_brams() as usize % bram_cols, 0);
    assert_eq!(device.num_brams() as usize / bram_cols, 9);
    // The 3/4 usable fraction: the same geometry at full density
    // would hold 24 DSPs and 12 BRAMs per column (KU5P header:
    // 1,824 DSPs, 480 BRAMs over the same 76 / 40 columns).
    assert_eq!(18 * 4, 24 * 3);
    assert_eq!(9 * 4, 12 * 3);
}

#[test]
fn xcku3p_frame_gap_is_four_per_far_row_vs_ug570() {
    let device = load("xcku3p.json");
    let rows = device.max_far_row_idx("SLR0").unwrap() + 1;
    assert_eq!(rows, 4);
    // File truth, from the minors tables.
    assert_eq!(device.total_frames(), 41_460);
    // AMD UG570 Table 1 lists 41,476 configuration frames for KU3P.
    // The gap is exactly 4 per FAR row; do not tune the tables.
    assert_eq!(device.total_frames() + 4 * rows as u64, 41_476);
    assert_eq!(41_476 - device.total_frames(), 16);
}

#[test]
fn xcvu3p_slices_are_lut_derived_and_geometry_matches_header() {
    let device = load("xcvu3p.json");
    // DS890 Table 9 VU3P: 394,080 LUTs, 788,160 FFs exactly.
    assert_eq!(Xcvu3pSpec::CLB_SLICE_COUNT, 49_260);
    assert_eq!(device.num_slices(), 49_260);
    assert_eq!(device.num_luts(), 394_080);
    assert_eq!(device.num_regs(), 788_160);
    assert_eq!(device.num_slices() * 8, device.num_luts());
    assert_eq!(device.num_slices() * 16, device.num_regs());
    // The provisional geometry is self-consistent with the header:
    // 821 CLB columns x 60 slices = 49,260.
    let mut geo_slices = 0usize;
    for row in 0..=device.max_far_row_idx("SLR0").unwrap() {
        geo_slices += device.num_clb_columns("SLR0", row).unwrap() * 60;
    }
    assert_eq!(geo_slices, 49_260);
    assert_eq!(geo_slices as u32, device.num_slices());
    // Header-vs-geometry gaps of the same class as xcku3p, pinned
    // so a future generator change fails loudly: tables imply
    // 3,192 DSP sites (133 cols x 24) vs header 2,280, and 840
    // BRAM sites (70 cols x 12) vs header 720.
    let mut dsp_cols = 0usize;
    let mut bram_cols = 0usize;
    for row in 0..=6 {
        dsp_cols += count_dsp_columns(&device, row);
        bram_cols += device.num_bram_columns("SLR0", row).unwrap();
    }
    assert_eq!(dsp_cols, 133);
    assert_eq!(bram_cols, 70);
    assert_eq!(dsp_cols * 24, 3_192);
    assert_eq!(bram_cols * 12, 840);
    assert_eq!(device.num_dsps(), 2_280);
    assert_eq!(device.num_brams(), 720);
}

#[test]
fn xcvu3p_frame_total_overshoots_ug570_by_1215() {
    let device = load("xcvu3p.json");
    assert_eq!(device.total_frames(), 73_035);
    // AMD UG570 Table 1 lists 71,820 configuration frames for VU3P.
    // The construction overshoots; do not tune the tables to match.
    assert_eq!(device.total_frames() - 71_820, 1_215);
}

#[test]
fn far_mapping_comes_from_device_file_not_logical_grid() {
    // G2: the device.rs 144x400 grid is invented, not ground truth,
    // and bitgen-core must not consume it for FAR mapping. The grid
    // and the file geometry disagree outright: the grid is 144 wide
    // and 400 tall, while the xcku3p file has 113 CLB columns per
    // row and 4 FAR rows. Any code equating them is wrong.
    let grid = UltraScaleDevice::xcvu3p();
    assert_eq!((grid.grid_width(), grid.grid_height()), (144, 400));
    let device = load("xcku3p.json");
    assert_eq!(device.num_clb_columns("SLR0", 0).unwrap(), 113);
    assert_eq!(device.max_far_row_idx("SLR0").unwrap(), 3);
    // The bitgen path resolves FARs from the DeviceFile through the
    // BitgenDevice trait, and agrees with the file's own BEL
    // resolver (BitLocator): SLICE_X23Y45 base FAR 0x3000, BFF
    // minor 12, A6LUT bit 0 minor 11.
    let arch = load_arch();
    let loc = BitLocator::new(&device, &arch);
    let base = device.tile_base_far("SLICE_X23Y45").unwrap();
    assert_eq!(base, 0x0000_3000);
    let (_, far, _) = loc.locate_reg("SLICE_X23Y45/BFF").unwrap();
    assert_eq!(far.to_u32(), base + 12);
    let (_, fars, _) = loc.locate_lut("SLICE_X23Y45/A6LUT").unwrap();
    assert_eq!(fars[0].to_u32(), base + 11);
    // The grid device has no FAR mapping to consult: tile_base_far
    // exists only on the BitgenDevice trait, implemented by the
    // device file (and the test stub), never by UltraScaleDevice.
    assert_eq!(<DeviceFile as BitgenDevice>::part(&device), "xcku3p");
}

#[test]
fn xczu3eg_header_slices_are_geometry_sites() {
    // Unlike xcku3p, usable slices equal geometry slices on
    // xczu3eg: 49 CLB columns x 3 FAR rows x 60 slices = 8,820,
    // and the header, the DS891 LUT/FF counts, and the tilegrid
    // CLE tile count (5,220 SLICEL + 3,600 SLICEM) all agree.
    let device = load("xczu3eg.json");
    let mut clb_cols = 0;
    for row in 0..3 {
        let n = device.num_clb_columns("SLR0", row).unwrap();
        assert_eq!(n, 49, "xczu3eg has 49 CLB columns in every row");
        clb_cols += n as u64;
    }
    assert_eq!(clb_cols, 147);
    let geo_slices = clb_cols * 60;
    assert_eq!(geo_slices, 8_820);
    assert_eq!(device.num_slices(), 8_820);
    assert_eq!(geo_slices as u32, device.num_slices());

    assert_eq!(Xczu3egSpec::CLB_SLICE_COUNT, 8_820);
    assert_eq!(device.num_luts(), 70_560);
    assert_eq!(device.num_regs(), 141_120);
    assert_eq!(device.num_luts() / 8, 8_820);
    assert_eq!(device.num_regs() / 16, 8_820);
    // No usable-vs-geometry gap on this part.
    assert_eq!(device.num_slices() - Xczu3egSpec::CLB_SLICE_COUNT, 0);

    // The logical grid model carries the same counts.
    let model = UltraScaleDevice::xczu3eg();
    assert_eq!((model.grid_width(), model.grid_height()), (80, 130));
    assert_eq!(model.part, "xczu3eg");
    assert_eq!(model.count_bels(BelKind::Lut6), 70_560);
    assert_eq!(model.count_bels(BelKind::FdRe), 141_120);
    assert_eq!(model.count_bels(BelKind::Carry8), 8_820);
    assert_eq!(model.count_bels(BelKind::Dsp48e2), 360);
    assert_eq!(model.count_bels(BelKind::Ramb36e2), 216);
    assert_eq!(model.count_bels(BelKind::Uram288), 0);
}

#[test]
fn xczu3eg_dsp_and_bram_headers_match_table_density() {
    let device = load("xczu3eg.json");
    // DSP: 5 columns per row, 3 rows, 24 sites per column.
    let mut dsp_cols = 0;
    for row in 0..3 {
        assert_eq!(count_dsp_columns(&device, row), 5);
        dsp_cols += 5;
    }
    assert_eq!(dsp_cols, 15);
    assert_eq!(device.num_dsps(), 360);
    assert_eq!(device.num_dsps() as usize % dsp_cols, 0);
    assert_eq!(device.num_dsps() as usize / dsp_cols, 24);
    // BRAM: 6 columns per row, 3 rows, 12 sites per column.
    let mut bram_cols = 0;
    for row in 0..3 {
        assert_eq!(device.num_bram_columns("SLR0", row).unwrap(), 6);
        bram_cols += 6;
    }
    assert_eq!(bram_cols, 18);
    assert_eq!(device.num_brams(), 216);
    assert_eq!(device.num_brams() as usize % bram_cols, 0);
    assert_eq!(device.num_brams() as usize / bram_cols, 12);
}

#[test]
fn xczu3eg_frame_gap_is_four_per_far_row_vs_ug1085() {
    // UG1085 Table 1 lists 14,964 configuration frames for ZU3.
    // The file totals 14,952: gap 12 = 4 per FAR row x 3 rows, the
    // same systematic pattern as xcku3p (16 = 4 x 4), explained by
    // 12 padding frames in real bitstreams (see
    // tests/device_file_xczu3eg.rs). Do not tune to match UG1085.
    let device = load("xczu3eg.json");
    let rows = device.max_far_row_idx("SLR0").unwrap() + 1;
    assert_eq!(rows, 3);
    assert_eq!(device.total_frames(), 14_952);
    assert_eq!(device.total_frames() + 4 * rows as u64, 14_964);
    assert_eq!(14_964 - device.total_frames(), 12);
}
