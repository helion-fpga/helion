//! Geometry-aware FAR increment: a multi-frame FDRI packet crossing a
//! column boundary must assign frames by the device geometry (minor wraps
//! at the column height, then column +1), not by plain integer +1.
//!
//! The packet built here is Vivado-shaped: one FAR write followed by one
//! FDRI write carrying two 93-word frames, with no CRC packets (the parser
//! only verifies CRC when a CRC packet is present). The two frames start
//! at the last minor of column 0, so plain +1 would place the second frame
//! at the nonexistent minor 2 of column 0, while the geometry places it at
//! minor 0 of column 1.
//!
//! Geometry source: the emitter's own device config order, via
//! `ConfigOrderGeometry::from_config_order` over a stub device whose
//! config order lists every frame of a two-row, two-column device:
//! row 0 and row 1 each have column 0 with 2 minors and column 1 with
//! 3 minors, plus one block 1 column with 2 minors.

use helion_ultrascale::bitgen::{
    BUS_WIDTH_DETECT, BUS_WIDTH_SYNC, BitgenDevice, ConfigOrderGeometry, DUMMY_WORD, FRAME_WORDS,
    FrameWords, NOOP_WORD, SYNC_WORD, StubDevice, cmd, far_encode, far_increment, parse_frames,
    parse_frames_with_geometry, reg,
};

/// Append one Type 1 write packet (UG570 tables 9-16/9-18).
fn type1_write(reg_addr: u8, data: &[u32], words: &mut Vec<u32>) {
    words.push(
        (1u32 << 29) | (2u32 << 27) | ((reg_addr as u32) << 13) | (data.len() as u32 & 0x7FF),
    );
    words.extend_from_slice(data);
}

/// Raw bitstream bytes: preamble, one FAR write, one FDRI write carrying
/// every frame in `frames`, DESYNC, trailing NOOPs. No CRC packets.
fn multiframe_stream(start_far: u32, frames: &[FrameWords]) -> Vec<u8> {
    let mut words: Vec<u32> = Vec::new();
    words.extend([DUMMY_WORD; 8]);
    words.push(BUS_WIDTH_DETECT);
    words.push(BUS_WIDTH_SYNC);
    words.extend([DUMMY_WORD; 2]);
    words.push(SYNC_WORD);
    words.push(NOOP_WORD);
    type1_write(reg::CMD, &[cmd::RCRC], &mut words);
    type1_write(reg::IDCODE, &[0x04A6_3093], &mut words);
    type1_write(reg::CMD, &[cmd::WCFG], &mut words);
    type1_write(reg::FAR, &[start_far], &mut words);
    let mut payload: Vec<u32> = Vec::new();
    for frame in frames {
        payload.extend_from_slice(frame);
    }
    type1_write(reg::FDRI, &payload, &mut words);
    type1_write(reg::CMD, &[cmd::DESYNC], &mut words);
    words.extend([NOOP_WORD; 400]);
    let mut bytes = Vec::with_capacity(words.len() * 4);
    for word in words {
        bytes.extend_from_slice(&word.to_be_bytes());
    }
    bytes
}

/// Two distinct frames, distinguishable by their first and last words.
fn two_frames() -> [FrameWords; 2] {
    let mut first = [0u32; FRAME_WORDS];
    first[0] = 0x1111_1111;
    first[1] = 0xA5A5_A5A5;
    let mut second = [0u32; FRAME_WORDS];
    second[0] = 0x2222_2222;
    second[FRAME_WORDS - 1] = 0xDEAD_BEEF;
    [first, second]
}

/// Stub device whose config order is the whole two-row device: rows 0
/// and 1, column 0 with minors 0..1, column 1 with minors 0..2, plus one
/// block 1 column with minors 0..1 so the block roll has a real frame.
fn two_row_device() -> StubDevice {
    let mut order: Vec<u32> = Vec::new();
    for row in [0u8, 1u8] {
        for (col, minors) in [(0u16, 2u8), (1u16, 3u8)] {
            for minor in 0..minors {
                order.push(far_encode(0, row, col, minor).unwrap());
            }
        }
    }
    for minor in 0..2u8 {
        order.push(far_encode(1, 0, 0, minor).unwrap());
    }
    StubDevice::new("xcku3p", Some(0x04A6_3093), vec![], order)
}

#[test]
fn far_increment_wraps_at_column_row_and_block() {
    let device = two_row_device();
    let geometry = ConfigOrderGeometry::from_config_order(device.config_order_fars());
    // Minor advance inside column 0.
    assert_eq!(far_increment(&geometry, 0x0000_0000), Some(0x0000_0001));
    // Column wrap: last minor of column 0 (height 2) goes to column 1
    // minor 0. Plain +1 would give 0x00000002, a minor column 0 has not got.
    assert_eq!(far_increment(&geometry, 0x0000_0001), Some(0x0000_0100));
    // Minor advance inside column 1.
    assert_eq!(far_increment(&geometry, 0x0000_0100), Some(0x0000_0101));
    // Row wrap: last minor of row 0 column 1 goes to row 1 column 0 minor 0.
    assert_eq!(far_increment(&geometry, 0x0000_0102), Some(0x0004_0000));
    // Block roll: last minor of the last block 0 row and column goes to
    // block 1 row 0 column 0 minor 0, the first block 1 frame.
    assert_eq!(far_increment(&geometry, 0x0004_0102), Some(0x0100_0000));
    // Minor advance inside the block 1 column.
    assert_eq!(far_increment(&geometry, 0x0100_0000), Some(0x0100_0001));
    // Past the device end: last minor of the last block 1 frame has no
    // successor.
    assert_eq!(far_increment(&geometry, 0x0100_0001), None);
    // A block 2 FAR is outside the geometry.
    assert_eq!(far_increment(&geometry, 0x0200_0000), None);
}

#[test]
fn far_increment_matches_decoder_d2_walk() {
    // Decoder crew proof (bitstream_packets.rs module docs): d2.bit run
    // starting at FAR 0x0004c738 (block 0, row 1, column 199, minor 56),
    // 26 frames: 20 frames in column 199 (76 minors) plus 6 frames in
    // column 200 (16 minors), ending at 0x0004c805. Plain +1 predicts
    // 0x0004c751, a minor past column 199 height.
    let mut order: Vec<u32> = Vec::new();
    for col in 0..=200u16 {
        let height = if col == 199 { 76u8 } else { 16u8 };
        for minor in 0..height {
            order.push(far_encode(0, 1, col, minor).unwrap());
        }
    }
    let geometry = ConfigOrderGeometry::from_config_order(&order);
    let mut far = 0x0004_c738;
    for _ in 0..25 {
        far = far_increment(&geometry, far).expect("walk stays inside the device");
    }
    assert_eq!(far, 0x0004_c805);
    assert_eq!(0x0004_c738u32.wrapping_add(25), 0x0004_c751);
    assert_ne!(far, 0x0004_c751);
}

#[test]
fn multiframe_fdri_packet_crossing_column_boundary_is_geometry_aware() {
    let device = two_row_device();
    let geometry = ConfigOrderGeometry::from_config_order(device.config_order_fars());
    let frames = two_frames();
    // Start at the last minor of column 0; the second frame must land on
    // column 1 minor 0 (0x100), not on plain +1 address 0x2.
    let bytes = multiframe_stream(0x0000_0001, &frames);
    let parsed = parse_frames_with_geometry(&bytes, &geometry).unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[&0x0000_0001], frames[0]);
    assert_eq!(parsed[&0x0000_0100], frames[1]);
    assert!(!parsed.contains_key(&0x0000_0002));
}

#[test]
fn multiframe_fdri_packet_without_geometry_fails_loud() {
    // Without geometry the parser cannot know the column heights, so a
    // multi-frame FDRI packet must fail instead of silently assigning
    // frames by plain +1.
    let frames = two_frames();
    let bytes = multiframe_stream(0x0000_0001, &frames);
    assert!(parse_frames(&bytes).is_err());
}
