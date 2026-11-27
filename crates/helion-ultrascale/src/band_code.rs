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

/// Outcome of checking one stored band against the formula. This is
/// the parse-side verdict (N7): the stored band must equal the masked
/// MAP-A value, or that value XOR the Q residual, and anything else is
/// a named mismatch, never a silent accept. N16 adds the two
/// `AmbiguousSubset` variants below: a stored band that differs from
/// the masked value by a subset of the ambiguous masked columns is
/// accepted when that column set is small (rank at most
/// `MAX_AMBIGUOUS_RANK`), because parse cannot tell RAM content bits
/// from logic INIT bits in a mixed column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandCheck {
    /// Stored band equals the expected value.
    Exact,
    /// Stored band equals the expected value XOR `Q_RESIDUAL`: a Q
    /// frame, accepted, not explained.
    QResidual,
    /// Stored band equals the expected value XOR the XOR of some
    /// subset of the ambiguous masked columns: the masked set bits
    /// whose MAP-A columns have GF(2) rank at most
    /// `MAX_AMBIGUOUS_RANK`. Accepted, not explained: the subset is
    /// the logic INIT bits among the masked positions, which parse
    /// cannot identify from bits alone.
    AmbiguousSubset,
    /// As `AmbiguousSubset`, plus the Q residual: the stored band
    /// equals the expected value XOR `Q_RESIDUAL` XOR a subset of
    /// the ambiguous masked columns. A frame that is both a Q frame
    /// and mixed-column.
    AmbiguousSubsetQResidual,
    /// Stored band matches neither the expected value nor the expected
    /// value XOR `Q_RESIDUAL`. `expected` is the masked MAP-A value
    /// for block-0 frames and 0 for block-1 and block-2+ frames.
    Mismatch { stored: u64, expected: u64 },
}

/// Maximum GF(2) rank of the ambiguous masked-column set for which
/// N16 subset acceptance applies.
///
/// Measured on the 14-design Vivado corpus (144,816 block-0 frames,
/// `~/workspace/ultra-bitgen-crew/logs/n16fix_measure3.py`): 5,584
/// frames sit in a RAM-letter column and 3,948 of those carry at
/// least one ambiguous masked set bit. Their per-bit column rank is
/// 40 to 44 on 3,863 frames, so no bound anywhere near the full rank
/// is honest: the whole MAP-A table spans only a 44-dimensional
/// subspace of the 48-bit band, and at rank 44 a random wrong band
/// lands in the span with probability 1 in 16. At rank 24 the
/// accepted set holds at most 2^24 deltas (2^25 with the Q-combined
/// variant), so a uniformly random wrong band passes with
/// probability at most 1 in 8,388,608; over the whole 144,816-frame
/// corpus that is under 0.02 expected slips. The acceptance ladder
/// on real ambiguous frames is 5 frames at rank 16, 20 at rank 24,
/// 36 at rank 32, so 24 is the largest bound that keeps the
/// corpus-scale slip expectation below 0.02 while accepting four
/// times the frames rank 16 does. Dense real mixed columns
/// (rank 40 to 44 on 3,863 frames) still fail loud; that residual is
/// the documented
/// N16 remainder, not a silent accept. A group-aware variant (the
/// true subset is a union of (slice, letter) groups, measured rank
/// 24 or below on 801 real frames) would accept more, but
/// `check_stored_band` receives no grouping input, so it stays
/// future work.
pub const MAX_AMBIGUOUS_RANK: usize = 24;

/// Insert `value` into a GF(2) basis keyed by highest set bit.
/// Returns true when the value was independent (rank grows by one).
/// Basis slots hold 0 when absent; a basis vector is never 0.
fn basis_insert(basis: &mut [u64; BAND_BITS], mut value: u64) -> bool {
    while value != 0 {
        let pivot = 63 - value.leading_zeros() as usize;
        if basis[pivot] == 0 {
            basis[pivot] = value;
            return true;
        }
        value ^= basis[pivot];
    }
    false
}

/// Reduce `value` by a GF(2) basis keyed by highest set bit.
/// Returns the remainder; zero means the value is in the span.
fn basis_reduce(basis: &[u64; BAND_BITS], mut value: u64) -> u64 {
    while value != 0 {
        let pivot = 63 - value.leading_zeros() as usize;
        if basis[pivot] == 0 {
            break;
        }
        value ^= basis[pivot];
    }
    value
}

/// The MAP-A columns of the ambiguous masked set bits of one frame:
/// positions that are both masked (LUTRAM per parse) and set in the
/// frame, outside the band range, with a nonzero column. Band bits
/// are never inputs, matching `compute_band_map_a`.
fn ambiguous_columns(words: &FrameWords, lutram_mask: &[bool; FRAME_BITS]) -> Vec<u64> {
    let mut out = Vec::new();
    for &(src, col) in tables::MAP_A {
        if src >= BAND_LO && src <= BAND_HI {
            continue;
        }
        if col == 0 {
            continue;
        }
        if lutram_mask[src as usize] && bit_is_set(words, src) {
            out.push(col & BAND_MASK);
        }
    }
    out
}

/// Check one stored band against the masked MAP-A formula. This is the
/// function `parse_bitstream` calls (N7) and the function the corpus
/// run (`band_check`) calls over the stored frames, so the parse check
/// and the corpus proof are the same code, not two implementations.
///
/// - Block type 0: expected is `compute_band_map_a` with the LUTRAM
///   mask; accept the stored band when it equals the expected value,
///   or the expected value XOR `Q_RESIDUAL` (the 484 Q frames).
/// - N16: when neither holds, the difference may still be explained
///   by the mixed-column ambiguity: every masked set bit may be RAM
///   content (excluded) or logic INIT (included), so the stored band
///   may equal the expected value XOR any subset of the ambiguous
///   masked columns. That subset is found by Gaussian elimination
///   over the 48-bit column vectors, never by enumerating subsets
///   and never by the two extremes (masked vs unmasked), which the
///   pinned test disproves: the stored band there is one column
///   alone, neither extreme. Subset acceptance applies only when
///   the ambiguous columns have GF(2) rank at most
///   `MAX_AMBIGUOUS_RANK`; above it the check fails loud exactly as
///   before, so dense mixed columns stay rejected. The same subset
///   XOR the Q residual is accepted as its own variant.
/// - Block type 1 (BRAM content) and block types 2 and above: the
///   expected band is 0, matching `compute_band`; any nonzero stored
///   band is a mismatch. Block types 2 and above are untested and
///   their zero band is the documented gap, not a claim.
pub fn check_stored_band(
    far: u32,
    words: &FrameWords,
    lutram_mask: &[bool; FRAME_BITS],
) -> BandCheck {
    let stored = frame_band(words) & BAND_MASK;
    let (block, _, _, _) = far_decode(far);
    let expected = match block {
        0 => compute_band_map_a(words, lutram_mask),
        _ => 0,
    };
    if stored == expected {
        return BandCheck::Exact;
    }
    if block == 0 && stored == (expected ^ Q_RESIDUAL) {
        return BandCheck::QResidual;
    }
    if block != 0 {
        return BandCheck::Mismatch { stored, expected };
    }
    let columns = ambiguous_columns(words, lutram_mask);
    if columns.is_empty() {
        return BandCheck::Mismatch { stored, expected };
    }
    let mut basis = [0u64; BAND_BITS];
    let mut rank = 0usize;
    for col in &columns {
        if basis_insert(&mut basis, *col) {
            rank += 1;
        }
    }
    if rank > MAX_AMBIGUOUS_RANK {
        return BandCheck::Mismatch { stored, expected };
    }
    let delta = (stored ^ expected) & BAND_MASK;
    if basis_reduce(&basis, delta) == 0 {
        return BandCheck::AmbiguousSubset;
    }
    if basis_reduce(&basis, delta ^ Q_RESIDUAL) == 0 {
        return BandCheck::AmbiguousSubsetQResidual;
    }
    BandCheck::Mismatch { stored, expected }
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
    fn check_stored_band_accepts_exact_and_q_and_rejects_other() {
        // One set content bit: its MAP-A column is the expected band.
        let words = single_bit_words(0);
        let expected = compute_band_map_a(&words, &no_mask());
        assert_eq!(expected, 0x110100011000);
        let mut stored = words;
        write_band(&mut stored, expected);
        assert_eq!(
            check_stored_band(far_encode(0, 0, 0, 0).unwrap(), &stored, &no_mask()),
            BandCheck::Exact
        );
        // The same frame carrying formula + w is a Q frame: accepted
        // as QResidual, never silently, and never as Exact.
        let mut q_words = words;
        write_band(&mut q_words, expected ^ Q_RESIDUAL);
        assert_eq!(
            check_stored_band(far_encode(0, 0, 0, 0).unwrap(), &q_words, &no_mask()),
            BandCheck::QResidual
        );
        // One flipped band bit off the formula value is a mismatch
        // with the expected value named, not an accept.
        let mut bad_words = words;
        write_band(&mut bad_words, expected ^ 1);
        assert_eq!(
            check_stored_band(far_encode(0, 0, 0, 0).unwrap(), &bad_words, &no_mask()),
            BandCheck::Mismatch {
                stored: expected ^ 1,
                expected
            }
        );
        // A masked content bit contributes nothing on its own: the
        // stored band must be 0 for Exact. N16 changed what a stored
        // column value means here: the single masked set bit is
        // ambiguous (it may be a logic INIT bit), its column has
        // rank 1, so the stored column is accepted as
        // AmbiguousSubset, not a mismatch. A corrupted value is
        // still a mismatch; see the dedicated N16 tests below.
        let mut mask = no_mask();
        mask[0] = true;
        assert_eq!(
            check_stored_band(far_encode(0, 0, 0, 0).unwrap(), &single_bit_words(0), &mask),
            BandCheck::Exact,
            "masked bit set, stored band 0"
        );
        let mut unmasked_band = single_bit_words(0);
        write_band(&mut unmasked_band, expected);
        assert_eq!(
            check_stored_band(far_encode(0, 0, 0, 0).unwrap(), &unmasked_band, &mask),
            BandCheck::AmbiguousSubset
        );
        // Block-1 frames expect a zero band with no Q allowance: a
        // stored w is a mismatch, not a QResidual.
        let mut bram = single_bit_words(0);
        write_band(&mut bram, Q_RESIDUAL);
        assert_eq!(
            check_stored_band(far_encode(1, 0, 0, 0).unwrap(), &bram, &no_mask()),
            BandCheck::Mismatch {
                stored: Q_RESIDUAL,
                expected: 0
            }
        );
        let bram_zero = single_bit_words(0);
        assert_eq!(
            check_stored_band(far_encode(1, 0, 0, 0).unwrap(), &bram_zero, &no_mask()),
            BandCheck::Exact
        );
    }

    fn words_with_bits(bits: &[u16]) -> FrameWords {
        let mut words = empty_words();
        for &abs in bits {
            words[(abs / 32) as usize] |= 1u32 << (abs % 32);
        }
        words
    }

    fn mask_for_bits(bits: &[u16]) -> [bool; FRAME_BITS] {
        let mut mask = no_mask();
        for &abs in bits {
            mask[abs as usize] = true;
        }
        mask
    }

    #[test]
    fn check_stored_band_accepts_ambiguous_subsets() {
        // N16 pinned numbers, confirmed against
        // h1_ecc_sparse_maps.json key "A": col(15) is the Y0 INIT
        // bit, col(975) the Y20 INIT bit on FAR minor 11.
        let col15 = compute_band_map_a(&single_bit_words(15), &no_mask());
        let col975 = compute_band_map_a(&single_bit_words(975), &no_mask());
        assert_eq!(col15, 0x880800088088);
        assert_eq!(col975, 0x088000008088);
        assert_eq!(col15 ^ col975, 0x808800080000);
        // Two masked set bits, both ambiguous: the masked
        // prediction is 0, and every subset of the two columns is
        // accepted, including the single-column stored band the
        // two-extremes rule misses.
        let far = far_encode(0, 0, 0, 0).unwrap();
        let words = words_with_bits(&[15, 975]);
        let mask = mask_for_bits(&[15, 975]);
        for stored in [0, col15, col975, col15 ^ col975] {
            let mut frame = words;
            write_band(&mut frame, stored);
            let got = check_stored_band(far, &frame, &mask);
            let want = if stored == 0 {
                BandCheck::Exact
            } else {
                BandCheck::AmbiguousSubset
            };
            assert_eq!(got, want, "stored 0x{stored:012x}");
        }
        // The same subset plus the Q residual is its own variant,
        // while the bare residual with no subset stays QResidual.
        let mut frame_q = words;
        write_band(&mut frame_q, col975 ^ Q_RESIDUAL);
        assert_eq!(
            check_stored_band(far, &frame_q, &mask),
            BandCheck::AmbiguousSubsetQResidual
        );
        let mut frame_q_only = words;
        write_band(&mut frame_q_only, Q_RESIDUAL);
        assert_eq!(
            check_stored_band(far, &frame_q_only, &mask),
            BandCheck::QResidual
        );
    }

    #[test]
    fn check_stored_band_rejects_corrupted_band_under_ambiguity() {
        // Same two-bit ambiguous frame: a stored band one bit off
        // any subset XOR is a mismatch, not an accept. The span
        // holds 4 values; col(975) with one band bit flipped and an
        // unrelated value are both outside it, with and without Q.
        let far = far_encode(0, 0, 0, 0).unwrap();
        let words = words_with_bits(&[15, 975]);
        let mask = mask_for_bits(&[15, 975]);
        let col975 = compute_band_map_a(&single_bit_words(975), &no_mask());
        for stored in [col975 ^ 1, 0x123456789abc, col975 ^ Q_RESIDUAL ^ 1] {
            let mut frame = words;
            write_band(&mut frame, stored);
            assert_eq!(
                check_stored_band(far, &frame, &mask),
                BandCheck::Mismatch {
                    stored,
                    expected: 0
                },
                "stored 0x{stored:012x} must fail loud"
            );
        }
    }

    #[test]
    fn check_stored_band_rejects_over_bound_ambiguous_set() {
        // Sources 0..96 carry 97 masked set bits whose columns have
        // GF(2) rank 25 (first prefix of band_code_tables.rs MAP_A
        // to reach rank 25), one above MAX_AMBIGUOUS_RANK. A stored
        // band that IS a subset XOR of those columns must still fail
        // loud: the bound, not the value, decides.
        let far = far_encode(0, 0, 0, 0).unwrap();
        let bits: Vec<u16> = (0..97).collect();
        let words = words_with_bits(&bits);
        let mask = mask_for_bits(&bits);
        let subset = compute_band_map_a(&words_with_bits(&[0, 1, 2]), &no_mask());
        let mut frame = words;
        write_band(&mut frame, subset);
        assert_eq!(
            check_stored_band(far, &frame, &mask),
            BandCheck::Mismatch {
                stored: subset,
                expected: 0
            }
        );
        let mut frame_q = words;
        write_band(&mut frame_q, subset ^ Q_RESIDUAL);
        assert_eq!(
            check_stored_band(far, &frame_q, &mask),
            BandCheck::Mismatch {
                stored: subset ^ Q_RESIDUAL,
                expected: 0
            }
        );
        // The same stored band with only the three subset bits
        // masked (rank 3) is accepted: same value, small set.
        let small_words = words_with_bits(&[0, 1, 2]);
        let small_mask = mask_for_bits(&[0, 1, 2]);
        let mut small_frame = small_words;
        write_band(&mut small_frame, subset);
        assert_eq!(
            check_stored_band(far, &small_frame, &small_mask),
            BandCheck::AmbiguousSubset
        );
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
