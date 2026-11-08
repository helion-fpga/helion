//! `BitgenDevice` implementation for the real device file
//! (bitgen-device worker, session 1; closes interfaces session 2
//! item 4 FLAG).
//!
//! Every expected number below is hand-checked against the device
//! file and the reference sources, not taken from the code under
//! test:
//!
//! - SLICE base FARs and the DB-frame cross-check use
//!   `BitLocator` (the device file's own BEL resolver) plus the
//!   prjuray-db frame numbers: `CLEL_L.BFF.INIT.V0` is frame 12
//!   and `CLEL_L.AFF.INIT.V0` is frame 12 in
//!   `segbits_clel_l.db` (commit affbc5e, CC0 1.0); the A6LUT
//!   per-bit features are `ALUT.INIT[0]` frame 11, `INIT[1]`
//!   frame 10, `INIT[2]` frame 9. The locator minors for the
//!   same BELs are 12, 12, and 11, 10, 9, so DB frame == locator
//!   minor and the tile base minor is 0.
//! - INT_X0Y0 resolves by the provisional harness rule, tile-type
//!   dependent (CLEL_R: CLB column major - 1): CLB column 0 is
//!   major 5, so the INT column is major 4, which carries exactly
//!   76 minors, the INT segbits frame span (vendored
//!   `segbits_int.db`, frames 0..75). Base FAR is therefore 0x400.
//!   INT_X1Y0 resolves by the +1 branch (CLB column 1 is CLEM,
//!   major 6, INT major 7, 76 minors): base FAR 0x700, the same
//!   INT column INT_X2Y0 resolves to (CLB column 2 is CLEL_R,
//!   major 8, INT major 7).
//! - The two golden PIP lines resolve to set bits (0x440, bit 0)
//!   for BOUNCE (`64_00`) and (0x429, bit 3) for GCLK (`41_03`)
//!   in the vendored `segbits_int.db`.
//!
//! No em dashes anywhere.

use helion_ultrascale::bitgen::{emit_bitstream, emit_frames, parse_bitstream};
use helion_ultrascale::{ArchSummary, BitLocator, BitgenDevice, DeviceFile, Far, FileSegbitsDb};
use std::collections::HashSet;
use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn data_dir() -> PathBuf {
    manifest_dir().join("data")
}

fn load_xcku3p() -> (DeviceFile, ArchSummary) {
    let device = DeviceFile::from_path(&data_dir().join("xcku3p.json")).unwrap();
    let arch = ArchSummary::from_path(&data_dir().join("ULTRASCALE_PLUS.json")).unwrap();
    (device, arch)
}

fn load_xcvu3p() -> (DeviceFile, ArchSummary) {
    let device = DeviceFile::from_path(&data_dir().join("xcvu3p.json")).unwrap();
    let arch = ArchSummary::from_path(&data_dir().join("ULTRASCALE_PLUS.json")).unwrap();
    (device, arch)
}

#[test]
fn xcku3p_part_and_idcode() {
    let (device, _) = load_xcku3p();
    // The trait `part` returns the short device name per the trait
    // doc ("Part name, e.g. 'xcku3p'"), matching the stub
    // convention; the inherent `DeviceFile::part()` is the full
    // part string the bitstream header names. Assert both, the
    // trait method by qualification (inherent methods win method
    // resolution, so a bare `device.part()` pins the inherent one).
    assert_eq!(<DeviceFile as BitgenDevice>::part(&device), "xcku3p");
    assert_eq!(device.part(), "xcku3p-ffva676-1-e");
    assert_eq!(device.device(), "xcku3p");
    // UG570 Table 2.
    assert_eq!(device.idcode(), Some(0x04A6_3093));
    assert!(!device.is_provisional());
}

#[test]
fn xcvu3p_part_and_idcode_and_provisional() {
    let (device, _) = load_xcvu3p();
    // Trait `part` is the short device name (see the xcku3p test);
    // the inherent `DeviceFile::part()` is the full part string.
    assert_eq!(<DeviceFile as BitgenDevice>::part(&device), "xcvu3p");
    assert_eq!(device.part(), "xcvu3p-ffvc1517-1-e");
    assert_eq!(device.device(), "xcvu3p");
    // UG570 Table 2.
    assert_eq!(device.idcode(), Some(0x14B3_9093));
    // The whole file is provisional (18 notes); the impl must not
    // present any of its values as confirmed.
    assert!(device.is_provisional());
    assert_eq!(device.provisional_notes().len(), 18);
}

#[test]
fn slice_base_far_matches_bitlocator_xcku3p() {
    let (device, arch) = load_xcku3p();
    let loc = BitLocator::new(&device, &arch);
    // SLICE_X23Y45: row 0, CLB column 23 is major 48 per the file;
    // B6LUT minors are 7..4 and BFF minor is 12 (device_file tests).
    // The tile type is CLEL_L.
    assert_eq!(device.tile_type("SLICE_X23Y45"), Some("CLEL_L"));
    let base = device.tile_base_far("SLICE_X23Y45").unwrap();
    assert_eq!(base, 0x0000_3000);
    assert_eq!(Far::from_u32(base).unwrap().minor, 0);
    // Base + DB frame == locator FAR: BFF.INIT.V0 is DB frame 12.
    let (_, far, _) = loc.locate_reg("SLICE_X23Y45/BFF").unwrap();
    assert_eq!(far.to_u32(), base + 12);
    // A6LUT bit 0 is DB frame 11 (ALUT.INIT[0] 11_15).
    let (_, fars, _) = loc.locate_lut("SLICE_X23Y45/A6LUT").unwrap();
    assert_eq!(fars[0].to_u32(), base + 11);
    // SLICE_X0Y0: CLB column 0 is major 5, tile type CLEL_R.
    assert_eq!(device.tile_type("SLICE_X0Y0"), Some("CLEL_R"));
    let base0 = device.tile_base_far("SLICE_X0Y0").unwrap();
    assert_eq!(base0, 0x0000_0500);
    let (_, far0, _) = loc.locate_reg("SLICE_X0Y0/AFF").unwrap();
    assert_eq!(far0.to_u32(), base0 + 12);
    // Last slice of the device: row 3, Y 239, still resolves.
    assert!(device.tile_base_far("SLICE_X112Y239").is_some());
    assert!(device.tile_base_far("SLICE_X113Y0").is_none());
    assert!(device.tile_base_far("SLICE_X0Y240").is_none());
}

#[test]
fn slice_base_far_matches_bitlocator_xcvu3p() {
    let (device, arch) = load_xcvu3p();
    let loc = BitLocator::new(&device, &arch);
    // Same cross-check on the provisional file: base + DB frame 12
    // must equal the locator FAR for AFF. The CLB column major is
    // derived from the locator, not hardcoded: the provisional file
    // must still agree with its own BEL resolver.
    let (_, far, _) = loc.locate_reg("SLICE_X0Y0/AFF").unwrap();
    let base = device.tile_base_far("SLICE_X0Y0").unwrap();
    assert_eq!(far.to_u32(), base + 12);
    assert_eq!(Far::from_u32(base).unwrap().minor, 0);
    assert_eq!(Far::from_u32(base).unwrap().row, far.row);
    assert_eq!(Far::from_u32(base).unwrap().col, far.col);
    // 7 FAR rows: the last row resolves, the row after does not.
    assert!(device.tile_base_far("SLICE_X0Y419").is_some());
    assert!(device.tile_base_far("SLICE_X0Y420").is_none());
}

#[test]
fn int_tile_resolves_by_provisional_rule_and_validates() {
    let (device, _) = load_xcku3p();
    // INT_X0Y0: CLB column 0 is major 5, INT column is major 4
    // (76 minors, the INT frame span). Base FAR 0x400.
    assert_eq!(device.tile_type("INT_X0Y0"), Some("INT"));
    assert_eq!(device.tile_base_far("INT_X0Y0"), Some(0x0000_0400));
    // INT_X2Y0 also resolves: CLB column 2 is major 8, INT major 7.
    assert_eq!(device.tile_base_far("INT_X2Y0"), Some(0x0000_0700));
    // INT_X1Y0 resolves by the +1 branch: CLB column 1 is CLEM,
    // major 6, so the INT column is major 7 (76 minors), the same
    // INT column INT_X2Y0 resolves to. Base FAR 0x700.
    assert_eq!(device.tile_type("INT_X1Y0"), Some("INT"));
    assert_eq!(device.tile_base_far("INT_X1Y0"), Some(0x0000_0700));
    // INT_X7Y0 resolves by the +1 branch too: CLB column 7 is
    // CLEL_L, major 18, INT major 19 (76 minors). Base FAR 0x1300.
    assert_eq!(device.tile_type("INT_X7Y0"), Some("INT"));
    assert_eq!(device.tile_base_far("INT_X7Y0"), Some(0x0000_1300));
    // INT tiles exist in every FAR row.
    assert_eq!(device.tile_base_far("INT_X0Y3"), Some(0x000C_0400));
    assert_eq!(device.tile_base_far("INT_X0Y4"), None);
}

#[test]
fn bram_tile_resolves() {
    let (device, arch) = load_xcku3p();
    let loc = BitLocator::new(&device, &arch);
    // RAMB18_X0Y0: BRAM content block, row 0, column 0, minor 0.
    assert_eq!(device.tile_type("RAMB18_X0Y0"), Some("BRAM"));
    assert_eq!(device.tile_base_far("RAMB18_X0Y0"), Some(0x0100_0000));
    let (_, mem_fars, _, _, _) = loc.locate_bram("RAMB18_X0Y0").unwrap();
    assert_eq!(mem_fars[0].to_u32(), 0x0100_0000);
    // RAMB36 has no resolver (BitLocator rejects it too).
    assert_eq!(device.tile_type("RAMB36_X0Y0"), None);
    assert_eq!(device.tile_base_far("RAMB36_X0Y0"), None);
}

#[test]
fn hpio_and_other_tiles_fail_loud() {
    let (device, _) = load_xcku3p();
    // No HPIO/HDIO/CMT/DSP column mapping exists in the device
    // file or any public tilegrid: any base FAR would be invented.
    // The golden's HPIO lines cannot emit until the tilegrid lands.
    for tile in [
        "HPIO_RIGHT_X0Y0",
        "HPIO_L_X0Y0",
        "HDIO_TOP_RIGHT_X0Y0",
        "HDIO_BOT_RIGHT_X0Y0",
        "CMT_RIGHT_X0Y0",
        "DSP_X0Y0",
        "NOT_A_TILE",
        "SLICE_X0",
        "INT_X0Y0_EXTRA",
    ] {
        assert_eq!(device.tile_type(tile), None, "{tile} must not resolve");
        assert_eq!(device.tile_base_far(tile), None, "{tile} must not resolve");
    }
}

#[test]
fn tile_names_enumerate_resolving_tiles_xcku3p() {
    let (device, _) = load_xcku3p();
    let names = device.tile_names();
    // Sorted, unique.
    assert!(names.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(names.len(), names.iter().collect::<HashSet<_>>().len());
    // Counts: 27,120 SLICE + 452 INT (all 113 CLB columns per row
    // resolve under the tile-type-dependent rule: 52 via -1, 61
    // via +1, x 4 rows) + 960 RAMB18 (10 cols x 4 rows x 24).
    assert_eq!(names.len(), 27_120 + 452 + 960);
    assert_eq!(
        names.iter().filter(|n| n.starts_with("SLICE_")).count(),
        27_120
    );
    assert_eq!(names.iter().filter(|n| n.starts_with("INT_")).count(), 452);
    assert_eq!(
        names.iter().filter(|n| n.starts_with("RAMB18_")).count(),
        960
    );
    assert!(names.contains(&"SLICE_X0Y0".to_string()));
    assert!(names.contains(&"SLICE_X112Y239".to_string()));
    assert!(names.contains(&"INT_X0Y0".to_string()));
    assert!(names.contains(&"INT_X1Y0".to_string()));
    assert!(names.contains(&"INT_X112Y3".to_string()));
    assert!(names.contains(&"RAMB18_X0Y0".to_string()));
    assert!(!names.iter().any(|n| n.starts_with("HPIO")));
    assert!(!names.iter().any(|n| n.starts_with("RAMB36")));
}

#[test]
fn tile_names_enumerate_resolving_tiles_xcvu3p() {
    let (device, _) = load_xcvu3p();
    let names = device.tile_names();
    assert!(names.windows(2).all(|w| w[0] < w[1]));
    // Counts: 49,260 SLICE + 791 INT (113 resolving CLB columns
    // per row x 7 rows; the tail CLB columns 113..117 resolve to
    // no 76-minor neighbour and do not resolve) + 1,680 RAMB18
    // (10 cols x 7 rows x 24).
    assert_eq!(names.len(), 49_260 + 791 + 1_680);
    assert_eq!(
        names.iter().filter(|n| n.starts_with("SLICE_")).count(),
        49_260
    );
    assert_eq!(names.iter().filter(|n| n.starts_with("INT_")).count(), 791);
    assert_eq!(
        names.iter().filter(|n| n.starts_with("RAMB18_")).count(),
        1_680
    );
    // The +1 branch resolves CLEM column 1 here too, while the
    // tail column 113 (CLEM_R, major 230, neighbours 8 and 16
    // minors) fails loud.
    assert!(names.contains(&"INT_X1Y0".to_string()));
    assert!(!names.contains(&"INT_X113Y0".to_string()));
}

#[test]
fn config_order_covers_every_frame_xcku3p() {
    let (device, _) = load_xcku3p();
    let order = device.config_order_fars();
    // The file's own total: 41,460 frames (UG570 lists 41,476; the
    // 16-frame delta is documented in data/README.md, not adjusted).
    assert_eq!(order.len(), 41_460);
    assert_eq!(order.len(), device.total_frames() as usize);
    assert!(order.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(order.len(), order.iter().collect::<HashSet<_>>().len());
    // First frame is block 0, row 0, column 0, minor 0.
    assert_eq!(order[0], 0x0000_0000);
    // Last frame is the highest BRAM content FAR: block 1, row 3,
    // column 9, minor 255.
    assert_eq!(*order.last().unwrap(), 0x010C_09FF);
}

#[test]
fn config_order_covers_every_frame_xcvu3p() {
    let (device, _) = load_xcvu3p();
    let order = device.config_order_fars();
    // The provisional construction sums to 73,035 against UG570
    // Table 1's 71,820 (overshoot 1,215, flagged in the file).
    // This impl enumerates the file's own frames, never tuned.
    assert_eq!(order.len(), 73_035);
    assert_eq!(order.len(), device.total_frames() as usize);
    assert!(order.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(order[0], 0x0000_0000);
    assert_eq!(*order.last().unwrap(), 0x0118_09FF);
}

#[test]
fn int_pip_frames_land_at_expected_fars() {
    let (device, _) = load_xcku3p();
    let db = FileSegbitsDb::from_file(&data_dir().join("segbits_int.db")).unwrap();
    // The two golden INT PIP lines on INT_X0Y0 (blinky/counter).
    let fasm = "INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1\n\
                INT_X0Y0.PIP.INT_NODE_GLOBAL_0_INT_OUT0.GCLK_B_0_0\n";
    let frames = emit_frames(fasm, &device, &db).unwrap();
    // BOUNCE sets frame 64 bit 0 at base 0x400 -> FAR 0x440.
    let bounce = frames.get(&0x440).expect("BOUNCE frame 0x440");
    assert_eq!(bounce[0] & 1, 1);
    assert_eq!(bounce.iter().sum::<u32>(), 1);
    // GCLK sets frame 41 bit 3 at base 0x400 -> FAR 0x429.
    let gclk = frames.get(&0x429).expect("GCLK frame 0x429");
    assert_eq!(gclk[0], 8);
    assert_eq!(gclk.iter().sum::<u32>(), 8);
    // The map also materializes the zero-checked FARs as all-zero
    // frames (emit_frames inserts the entry to test the bit): 5
    // zero bits for BOUNCE (frames 66, 68, 72 twice, 74) and 4 for
    // GCLK (frames 37, 39, 43, 47), so 2 set + 8 zero = 10.
    // They are all zeros, and the bitstream writer writes them as
    // zero frames, identical to untouched frames.
    assert_eq!(frames.len(), 10);
    for (far, words) in &frames {
        if *far != 0x440 && *far != 0x429 {
            assert!(
                words.iter().all(|w| *w == 0),
                "zero-checked frame {far:#X} must be all zeros"
            );
        }
    }
}

#[test]
fn emit_parse_roundtrip_on_real_device() {
    let (device, _) = load_xcku3p();
    let db = FileSegbitsDb::from_file(&data_dir().join("segbits_int.db")).unwrap();
    // FRAMING proof at the library level: the real device file plus
    // the real vendored INT DB emit a bitstream that parses back to
    // the same FASM. Full config order (41,460 frames) is emitted;
    // this is the real bitstream shape, not a touched-frames-only
    // shortcut.
    let fasm = "INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1\n\
                INT_X0Y0.PIP.INT_NODE_GLOBAL_0_INT_OUT0.GCLK_B_0_0\n";
    let bytes = emit_bitstream(fasm, &device, &db).unwrap();
    let back = parse_bitstream(&bytes, &device, &db).unwrap();
    assert_eq!(back, fasm);
    // Re-emitting the parse-back is byte-identical: the round trip
    // is stable, not just close.
    let bytes2 = emit_bitstream(&back, &device, &db).unwrap();
    assert_eq!(bytes, bytes2);
}
