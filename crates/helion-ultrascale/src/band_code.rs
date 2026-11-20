//! H-1 band code: the computed 48-bit code in words 45-46 of a frame.
//!
//! The band (absolute frame bits 1440-1487) is a GF(2)-linear code over
//! the frame's non-band bits, constant 0, with LUTRAM contents excluded.
//! Proof and full evidence: `H1_ECC_TEST.md` in
//! `~/workspace/ultra-bitgen-crew/decoder-validation/` (verdict CRACKED
//! with a bounded scope, 2026-11-20). Two formulas cover it:
//!
//! - MAP-A (primary): `h1_ecc_sparse_maps.json`, key "A" only, 2,927
//!   entries, exact on 144,332 of 144,816 block-0 corpus frames (99.67%)
//!   when LUTRAM content bits are zeroed first. Never use key "B", and
//!   never use `h1_ecc_formula.json` or `h1_ecc_formula_block0*.json`:
//!   poisoned or garbage variants, documented in the report.
//! - H76 (fallback): `h1_ecc_followup2_formula_h76.json`, 48 band bits,
//!   20,274 source incidences, exact on 143,269 of 144,816 frames
//!   (98.93%). It needs no LUTRAM mask. Every frame H76 gets right,
//!   MAP-A also gets right; MAP-A's extra 1,063 frames are exactly the
//!   LUTRAM-bearing frames the unmasked formula misses.
//!
//! Residual: exactly 484 frames (0.33%) carry residual `w =
//! 0xf0000000000f` under MAP-A; the predicate Q behind them is
//! unidentified. The emitter emits the formula value on those frames
//! and does not special-case Q. The 4 free source positions (absolute
//! bits 1502, 1508, 1514, 1520) are undetermined in the height-76 fit
//! and absent from the H76 source lists. In MAP-A as saved, only 1502
//! is absent; 1508, 1514 and 1520 carry the nonzero columns the sparse
//! fit determined, kept exactly as saved. The generator
//! (`tools/gen_band_code.py`) asserts all of this.
//!
//! Type rules, same as the 7-series `frame_ecc` precedent (but a
//! different code, see the shape check in the report): block type 1
//! (BRAM content) frames carry a zero band. Block types 2 and above
//! are absent from the corpus and untested; their band is left zero
//! here and the gap is documented, not guessed.
//!
//! The tables live in the generated `band_code_tables.rs`; this module
//! is the hand-written logic over them.

#[path = "band_code_tables.rs"]
mod tables;

use crate::bitgen::{FRAME_BITS, FrameWords, far_decode};

/// First absolute band bit (word 45, bit 0).
pub const BAND_LO: u16 = 1440;
/// Last absolute band bit (word 46, bit 15).
pub const BAND_HI: u16 = 1487;
/// Number of band bits.
pub const BAND_BITS: usize = 48;
/// Mask of the 48 band bits inside a u64 band value.
pub const BAND_MASK: u64 = (1u64 << BAND_BITS) - 1;
/// Residual carried by the 484 Q frames under MAP-A (band bits 1440-1443
/// and 1484-1487). Named here for tests and diagnostics only; the
/// emitter never adds it.
pub const Q_RESIDUAL: u64 = 0xf0000000000f;
/// Free source positions, undetermined in the height-76 fit.
pub const FREE_SOURCE_BITS: [u16; 4] = [1502, 1508, 1514, 1520];

/// Which formula computes the band. MAP-A is the primary (it is exact
/// on more frames); H76 is the fallback that needs no LUTRAM mask.
/// Callers select explicitly; the default is MAP-A.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BandFormula {
    /// Masked MAP-A, the primary formula.
    #[default]
    MapA,
    /// Unmasked height-76 formula, the fallback.
    H76,
}

/// True when absolute bit `abs` is set in `words`.
fn bit_is_set(words: &FrameWords, abs: u16) -> bool {
    let abs = abs as usize;
    (words[abs / 32] >> (abs % 32)) & 1 == 1
}

/// Read the stored band value out of a frame (words 45-46).
pub fn frame_band(words: &FrameWords) -> u64 {
    (words[45] as u64) | (((words[46] as u64) & 0xFFFF) << 32)
}

/// Write a band value into a frame, clearing the old band first.
/// Word 46 bits 16-31 are non-band source bits and are preserved.
pub fn write_band(words: &mut FrameWords, band: u64) {
    let band = band & BAND_MASK;
    words[45] = band as u32;
    words[46] = (words[46] & 0xFFFF_0000) | ((band >> 32) as u32 & 0xFFFF);
}

/// MAP-A band for one frame: XOR of the MAP-A columns of the set
/// non-band bits, with LUTRAM content bits zeroed first.
///
/// `lutram_mask[abs]` is true for LUTRAM content positions (INIT bits
/// of RAM-mode LUTs, see `bitgen::emit_frames`); those bits are skipped
/// even when set. Band bits themselves are never inputs: the table
/// lists none of them, and the loop skips the band range explicitly so
/// a future table change cannot turn a band bit into an input.
pub fn compute_band_map_a(words: &FrameWords, lutram_mask: &[bool; FRAME_BITS]) -> u64 {
    let mut band: u64 = 0;
    for &(src, col) in tables::MAP_A {
        if src >= BAND_LO && src <= BAND_HI {
            continue;
        }
        if lutram_mask[src as usize] {
            continue;
        }
        if bit_is_set(words, src) {
            band ^= col;
        }
    }
    band
}

/// H76 band for one frame: for each band bit, the parity of its source
/// bits. No mask is needed. Band bits are never inputs: no H76 source
/// list contains one, and the loop skips the band range explicitly.
pub fn compute_band_h76(words: &FrameWords) -> u64 {
    let mut band: u64 = 0;
    for j in 0..BAND_BITS {
        let start = tables::H76_OFFSETS[j] as usize;
        let end = tables::H76_OFFSETS[j + 1] as usize;
        let mut parity = false;
        for &src in &tables::H76_SOURCES[start..end] {
            if src >= BAND_LO && src <= BAND_HI {
                continue;
            }
            if bit_is_set(words, src) {
                parity = !parity;
            }
        }
        if parity {
            band |= 1u64 << j;
        }
    }
    band
}

/// Band for one frame, dispatched on the FAR block type.
///
/// - Block type 1 (BRAM content): 0, no formula is consulted.
/// - Block type 0 (CLB/IO/CLK): the selected formula; MAP-A uses the
///   LUTRAM mask, H76 ignores it.
/// - Block types 2 and above: 0. They are absent from the corpus and
///   untested; leaving the band zero is the documented gap, not a
///   claim about their real code.
pub fn compute_band(
    far: u32,
    words: &FrameWords,
    lutram_mask: &[bool; FRAME_BITS],
    formula: BandFormula,
) -> u64 {
    let (block, _, _, _) = far_decode(far);
    match block {
        0 => match formula {
            BandFormula::MapA => compute_band_map_a(words, lutram_mask),
            BandFormula::H76 => compute_band_h76(words),
        },
        _ => 0,
    }
}

/// Compute the band for one frame and write it in place. Band bits are
/// written directly into the frame words; no segbits feature covers
/// them, so this must never go through `write_frame_bit`.
pub fn write_computed_band(
    far: u32,
    words: &mut FrameWords,
    lutram_mask: &[bool; FRAME_BITS],
    formula: BandFormula,
) {
    let band = compute_band(far, words, lutram_mask, formula);
    write_band(words, band);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitgen::far_encode;

    fn empty_words() -> FrameWords {
        [0u32; 93]
    }

    fn single_bit_words(abs: u16) -> FrameWords {
        let mut words = empty_words();
        words[(abs / 32) as usize] |= 1u32 << (abs % 32);
        words
    }

    fn no_mask() -> [bool; FRAME_BITS] {
        [false; FRAME_BITS]
    }

    #[test]
    fn empty_frame_computes_zero() {
        let words = empty_words();
        let mask = no_mask();
        assert_eq!(compute_band_map_a(&words, &mask), 0);
        assert_eq!(compute_band_h76(&words), 0);
        assert_eq!(
            compute_band(
                far_encode(0, 0, 0, 0).unwrap(),
                &words,
                &mask,
                BandFormula::MapA
            ),
            0
        );
        assert_eq!(
            compute_band(
                far_encode(0, 0, 0, 0).unwrap(),
                &words,
                &mask,
                BandFormula::H76
            ),
            0
        );
    }

    #[test]
    fn single_bit_frames_match_json_columns() {
        // Column values copied from h1_ecc_sparse_maps.json key "A";
        // the H76 single-bit values below were computed from
        // h1_ecc_followup2_formula_h76.json and agree with MAP-A
        // on every source checked here.
        let mask = no_mask();
        let cases: &[(u16, u64)] = &[
            (0, 0x110100011000),
            (1, 0x220200022000),
            (4, 0x00010100011001),
            (31, 0x00080800088888),
            (32, 0x00010100100000),
            (100, 0x00010100110001),
            (1439, 0x00088008888888),
            (1488, 0x00011010001100),
            (2975, 0x00088888888888),
        ];
        for &(src, col) in cases {
            let words = single_bit_words(src);
            assert_eq!(compute_band_map_a(&words, &mask), col, "MAP-A src {src}");
            assert_eq!(compute_band_h76(&words), col, "H76 src {src}");
        }
    }

    #[test]
    fn free_source_bits_follow_saved_formulas() {
        // MAP-A as saved has no entry for 1502 (coefficient 0) and
        // keeps nonzero columns for 1508, 1514 and 1520; H76 lists
        // none of the four as a source.
        let mask = no_mask();
        assert_eq!(compute_band_map_a(&single_bit_words(1502), &mask), 0);
        assert_eq!(compute_band_h76(&single_bit_words(1502)), 0);
        assert_eq!(
            compute_band_map_a(&single_bit_words(1508), &mask),
            0x11010010001
        );
        assert_eq!(compute_band_h76(&single_bit_words(1508)), 0);
        assert_eq!(
            compute_band_map_a(&single_bit_words(1514), &mask),
            0x44040040040
        );
        assert_eq!(compute_band_h76(&single_bit_words(1514)), 0);
        assert_eq!(
            compute_band_map_a(&single_bit_words(1520), &mask),
            0x11010010100
        );
        assert_eq!(compute_band_h76(&single_bit_words(1520)), 0);
    }

    #[test]
    fn two_bit_frame_xors_columns() {
        // Linearity: the band of bits 0 and 1 together is col0 ^ col1.
        let mut words = single_bit_words(0);
        words[0] |= 1u32 << 1;
        let mask = no_mask();
        assert_eq!(
            compute_band_map_a(&words, &mask),
            0x110100011000u64 ^ 0x220200022000u64
        );
    }

    #[test]
    fn lutram_mask_zeroes_masked_bits() {
        // A masked set bit contributes nothing; an unmasked set bit
        // still contributes its column.
        let mut words = single_bit_words(0);
        words[0] |= 1u32 << 1;
        let mut mask = no_mask();
        mask[0] = true;
        assert_eq!(compute_band_map_a(&words, &mask), 0x220200022000);
        mask[1] = true;
        assert_eq!(compute_band_map_a(&words, &mask), 0);
    }

    #[test]
    fn band_bits_are_never_inputs() {
        // A frame with only band bits set computes 0 under both formulas.
        let mut words = empty_words();
        words[45] = 0xFFFF_FFFF;
        words[46] = 0x0000_FFFF;
        let mask = no_mask();
        assert_eq!(compute_band_map_a(&words, &mask), 0);
        assert_eq!(compute_band_h76(&words), 0);
        // The same holds with other content present: setting the band
        // bits must not change the computed band.
        let mut with_band = single_bit_words(0);
        with_band[45] = 0xFFFF_FFFF;
        with_band[46] = 0x0000_FFFF;
        assert_eq!(
            compute_band_map_a(&with_band, &mask),
            compute_band_map_a(&single_bit_words(0), &mask)
        );
        assert_eq!(
            compute_band_h76(&with_band),
            compute_band_h76(&single_bit_words(0))
        );
    }

    #[test]
    fn block_one_frames_get_zero_band() {
        let mut words = single_bit_words(0);
        words[0] |= 1u32 << 1;
        let mask = no_mask();
        let far = far_encode(1, 0, 0, 0).unwrap();
        assert_eq!(compute_band(far, &words, &mask, BandFormula::MapA), 0);
        assert_eq!(compute_band(far, &words, &mask, BandFormula::H76), 0);
        // Block types 2 and above are untested: zero, documented gap.
        let far2 = far_encode(2, 0, 0, 0).unwrap();
        assert_eq!(compute_band(far2, &words, &mask, BandFormula::MapA), 0);
    }

    #[test]
    fn write_band_roundtrips_and_preserves_word46_high_bits() {
        // Word 46 bits 16-31 are non-band source bits: they feed the
        // code and must survive the band write. Compute the expected
        // band from the same words, then check the write reproduces
        // it and leaves the high bits untouched.
        let mut words = single_bit_words(1488);
        words[46] |= 0xFFFF_0000;
        let expected = compute_band_map_a(&words, &no_mask());
        write_computed_band(
            far_encode(0, 0, 0, 0).unwrap(),
            &mut words,
            &no_mask(),
            BandFormula::MapA,
        );
        assert_eq!(frame_band(&words), expected);
        assert_eq!(words[46] & 0xFFFF_0000, 0xFFFF_0000);
        // A block-1 frame keeps a zero band even with content set.
        let mut bram = single_bit_words(0);
        write_computed_band(
            far_encode(1, 0, 0, 0).unwrap(),
            &mut bram,
            &no_mask(),
            BandFormula::MapA,
        );
        assert_eq!(frame_band(&bram), 0);
    }
}
