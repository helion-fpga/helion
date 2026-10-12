//! End-to-end smoke test: FASM -> frames -> FDRI payload -> `.bit`.
//!
//! Uses a tiny synthetic tilegrid + segbits (no database needed) and checks
//! the produced `.bit` carries the Series-7 preamble, the IDCODE write, and
//! a well-formed FDRI payload.

use helion_xilinx7::bitstream::{self, BitstreamOptions, IDCODE_XC7A35T};
use helion_xilinx7::fasm::parse_fasm;
use helion_xilinx7::frames::{BlockType, FrameStore, SegBitsDb, StubTileLayout, WORDS_PER_FRAME};

const SEGBITS: &str = "\
INT_L.IMUX.A 00_05 !00_06
INT_L.IMUX.B 00_07
";

#[test]
fn fasm_to_bit_smoke() {
    // Synthetic tilegrid: one INT_L tile owning frames 0..1.
    let mut layout = StubTileLayout::new();
    layout.add_tile("INT_L_X0Y0", "INT_L", BlockType::ClbIoClk, 0, 0);
    let mut segbits = SegBitsDb::new();
    segbits
        .add_segbits("INT_L", BlockType::ClbIoClk, SEGBITS)
        .unwrap();

    let fasm = "# smoke\nINT_L_X0Y0.IMUX.A\nINT_L_X0Y0.IMUX.B = 1\n";
    let lines = parse_fasm(fasm).unwrap();
    assert_eq!(lines.len(), 2);

    let mut store = FrameStore::new(&[0, 1]);
    let applied = store.apply_fasm(&lines, &segbits, &layout).unwrap();
    assert_eq!(applied, 2);

    // IMUX.A: 00_05 sets bit 5, !00_06 clears bit 6; IMUX.B: 00_07 sets bit 7.
    let w0 = store.frame_words(0).unwrap()[0];
    assert_eq!(w0, (1 << 5) | (1 << 7));

    let payload = store.fdri_payload();
    // 2 frames, one group, no internal boundary + trailing pad = 4 frames.
    assert_eq!(payload.len(), 4 * WORDS_PER_FRAME);

    let opts = BitstreamOptions::xc7frames2bit("xc7a35tcsg324-1", "smoke.frm", "2026/10/12", "00:00:00");
    let bit = bitstream::write_bit(&payload, &opts).unwrap();
    let bin = bitstream::write_bin(&payload, &opts).unwrap();

    // .bin starts with the 13-word preamble.
    assert_eq!(&bin[..4], &[0xFF, 0xFF, 0xFF, 0xFF]);
    assert_eq!(&bin[48..52], &[0xAA, 0x99, 0x55, 0x66]);

    // IDCODE write header + value present in the stream.
    let words: Vec<u32> = bin
        .chunks_exact(4)
        .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let pos = words
        .windows(2)
        .position(|w| w[0] == 0x3001_8001)
        .expect("IDCODE packet in stream");
    assert_eq!(words[pos + 1], IDCODE_XC7A35T);

    // FDRI Type-2 count matches the payload length.
    let fdri = words.iter().position(|&w| w == 0x3000_4000).unwrap();
    assert_eq!(words[fdri + 1], 0x5000_0000 | payload.len() as u32);

    // .bit wraps the same stream in the TLV envelope.
    assert_eq!(&bit[..13], &[0x00, 0x09, 0x0f, 0xf0, 0x0f, 0xf0, 0x0f, 0xf0, 0x0f, 0xf0, 0x00, 0x00, 0x01]);
    // Find the 'e' (bitstream) TLV key: NUL-terminated previous value, 'e',
    // then the 4-byte big-endian length. A raw byte search for b'e' is wrong
    // because the 'a' field (";Generator=xc7frames2bit") contains 'e's.
    let epos = bit
        .windows(3)
        .position(|w| w == [0, b'e', 0])
        .map(|i| i + 1)
        .unwrap();
    let elen =
        u32::from_be_bytes([bit[epos + 1], bit[epos + 2], bit[epos + 3], bit[epos + 4]]) as usize;
    assert_eq!(&bit[epos + 5..], &bin[..]);
    assert_eq!(elen, bin.len());

    // Ends with 400 NOPs.
    assert!(words[words.len() - 400..].iter().all(|&w| w == 0x2000_0000));
}
