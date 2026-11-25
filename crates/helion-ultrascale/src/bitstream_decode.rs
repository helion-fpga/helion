//! Frames to FASM decoder for the UltraScale+ full .bit decoder crew.
//!
//! This module turns configuration frames into FASM for the proving part
//! `xczu3eg-sbva484`, using only public data: the prjuray-db tilegrid and
//! segbits DBs under a `db_root` directory (the `zynqusp` tree). It never
//! invents a feature: every set bit is either owned by a DB feature that
//! matches the frames, listed in `DecodeOutput::unexplained`, or a band
//! bit explained by the N7 check below.
//!
//! # Worker A (packet layer, landed)
//!
//! `Frame` and `parse_bitstream` come from Worker A's
//! `crate::bitstream_packets` (commit c256e0b1, 22/22 tests): one
//! `Frame` per FAR with exactly 93 words, `frame_word_count() == 93`,
//! frames sorted by FAR with last-write-wins on duplicate FARs
//! (duplicates are legitimate, MFWR). `decode_bitstream` routes
//! through it; `decode_frames` consumes its frames. Semantics this
//! decoder depends on, all Worker A's: FAR auto-increment is
//! geometry-aware, not plain +1 (`bitgen.rs` plain +1 is wrong for
//! multi-frame FDRI packets crossing a column boundary, a separate
//! bitgen-core finding); FDRI requires a prior FAR and a prior IDCODE
//! write in the same section; MFWR replication follows UG570 with an
//! anchor check; 2 all-zero end-of-row padding frames are skipped in
//! uncompressed streams only. `decode_frames` also applies
//! last-write-wins if it is handed duplicate FARs directly.
//!
//! # Tilegrid model (verified against the public data, not assumed)
//!
//! The tilegrid is a dict keyed by tile name. Each tile carries a `type`,
//! a `sites` dict, and a `bits` dict keyed by block name (`CLB_IO_CLK`,
//! `BLOCK_RAM`). Each block entry has `baseaddr` (hex FAR of the
//! column's frame 0), `frames` (minor count), `offset`, and `words`.
//!
//! A tile does not own whole frames: a frame is shared by a whole column,
//! and each tile owns a bit window inside every frame of its column:
//! bits `[offset * 16, offset * 16 + words * 16)`. The 16-bit unit is
//! proven two independent ways: the BitFiltrator LUT table steps 48 bits
//! per slice Y (offset steps 3), and the tilegrid offset jumps by 6 units
//! at Y 30, which is exactly the table's +96 clock-region jump. Every
//! segbits DB in the tree satisfies `max frame < frames` and
//! `max bit < words * 16` for its tile type, including HDIO (max bit
//! 1390, window 1392) and XIPHY (max bit 719, window 720).
//!
//! A segbits `BitRef` (`frame`, `bit`) is therefore tile-relative: the
//! absolute position is FAR `baseaddr` with minor `frame`, bit
//! `bit + offset * 16`. A frame whose FAR matches no tile window, a bit
//! in the gap between windows, and a bit in a tile type with no segbits
//! file are all unexplained, never silently dropped. The one overlapping
//! window pair in this tilegrid is `PSS_ALTO_X0Y0` (whole-frame window)
//! over INT tiles in column (block 0, row 2, col 1); PSS_ALTO has no
//! segbits file, so a set bit there is owned by the INT window's feature
//! when one matches, and unexplained otherwise.
//!
//! # FASM tile names
//!
//! The FASM tile is the tilegrid tile name, except for CLB tiles, where
//! it is the tile's single `SLICE_XxYy` site, and BRAM tiles, where it
//! is the `RAMB18_XxYy` site selected by the `RAMB18E2_L` / `RAMB18E2_U`
//! half in the feature name (lower Y is L, upper Y is U), or the
//! `RAMB36_XxYy` site for a `RAMB36E2.*` whole-36 config feature. This
//! matches the harness goldens (`SLICE_X0Y0.A6LUT...`, `INT_X0Y0.PIP...`,
//! `HPIO_RIGHT_X0Y0.IOB_X0Y0...`, `RAMB18_X0Y0.INIT[16383:0]...`).
//! The feature name is rendered as-is from the DB, the same rule as
//! `bitgen.rs` (for example `RAMB18_X0Y0.RAMB18E2_L.INIT_A[0]`);
//! only the regrouped LUT INIT and BRAM content names are synthesized.
//!
//! # Feature matching (mirrors `crate::bitgen::parse_bitstream`)
//!
//! A feature is tested per tile window, in DB-relative coordinates:
//!
//! - Valued features (name contains `[`, `INIT_`, or `INITP_`, the same
//!   rule as `bitgen.rs`): present when any listed set bit reads 1.
//!   The value is reconstructed MSB-first across the listed set bits in
//!   DB order, exactly as `bitgen.rs` does, and rendered as lowercase
//!   `0x` with no redundant leading zeros, which is the harness
//!   `fasm.py` canonical form (`canonical_fasm` normalizes every hex
//!   value to that spelling; `64'h...` is accepted on input but never
//!   emitted here).
//! - Valueless features: present when every listed set bit reads 1 and
//!   every `!` bit reads 0, and the feature is observable: it sets at
//!   least one bit, or it is the exact complement of another feature of
//!   the same tile type (the binary-attribute rule from `bitgen.rs`,
//!   e.g. `AFF.INIT.V1` whose only bit is `!12_02`). An all-`!`
//!   feature with no complement (a mux default) is indistinguishable
//!   from absent and is dropped.
//!
//! Two divergences from `bitgen.rs`, both deliberate for a decoder:
//!
//! - A `!` bit that reads 1 never fails the decode. If the bit is a
//!   set bit of another present feature it is owned by that feature;
//!   otherwise it lands in `unexplained`. (`bitgen.rs` returns
//!   `MustBeZeroViolated` instead; a decoder reports, it does not
//!   refuse the bitstream.)
//! - Only windows with at least one set bit in their window are
//!   evaluated. A window whose frames are all zero contributes no
//!   lines: its features are all at default and indistinguishable from
//!   absent. This keeps an unused tile out of the output; a touched
//!   tile still renders its complement `.V0` / `.V1` lines, which is
//!   the complete-FASM form the crate canonicalizes to. An all-`!`
//!   complement-default line additionally requires the window to have
//!   a set bit a real feature of the window owns (see
//!   `window_has_owned_set_bit`): a window active only on bits no
//!   feature owns renders no complement defaults, so widening a window
//!   over a featureless band (H-1 RCLK) invents no lines.
//!
//! # H-1 band (N7 port)
//!
//! The band (absolute frame bits 1440-1487) is a computed code, not a
//! feature, so no segbits feature claims it. `decode_frames` checks
//! every block-0 and block-1 frame's stored band against the masked
//! MAP-A formula (`band_code::check_stored_band`) with per-frame
//! LUTRAM masks derived from the decoded features: candidate INIT
//! positions of present LUT INIT lines, filtered after the window
//! loop by the tiles carrying a present `{L}LUT.MODE.RAM` line, the
//! same identifiers (`ram_mode_lut_index`, `lut_init_index`) and the
//! same filter order as `parse_bitstream`. A frame whose check is
//! Exact or QResidual has its band bits explained, never listed in
//! `unexplained`; a Mismatch fails loud with a `BandMismatch` error
//! naming the FAR, stored and expected band. Block-2+ frames are not
//! checked: their band bits stay unexplained, as before.
//!
//! The mask derivation resolves MODE.RAM per slice tile (each tile
//! owns its own bit window), and real Vivado frames carry the
//! MODE.RAM bits at per-slice positions (measured on the P-11/P-13/
//! P-40 corpus: 4,992 cases, per-slice in every case, shared in
//! none), so unlike the parse side a column holding a RAM-mode LUT
//! and a logic-mode LUT of the same letter decodes clean (see
//! `tests/n7_decode_mixed_column.rs`).
//!
//! # Regrouping (no new encoding, only the harness canonical form)
//!
//! - LUT INIT: the DB carries per-bit `XLUT.INIT[i]` lines; they are
//!   regrouped into `X6LUT.INIT[63:0]` with
//!   `FileSegbitsDb::synthesize_lut_init`, and the per-bit lines are
//!   suppressed when the regrouped feature exists. This is the golden
//!   FASM form and the `bitloc` form.
//! - BRAM content: the DB carries per-bit `RAMB18E2_[LU].INIT_XX[i]`
//!   and `INITP_XX[i]` lines; they are regrouped into
//!   `INIT[16383:0]` and `INITP[2047:0]` on the `RAMB18_XxYy` tile,
//!   the `bitloc::bram18_fasm` and golden form. A group that is not
//!   complete in the DB falls back to its per-bit lines.
//! - BRAM `INIT_A` / `INIT_B` / `SRVAL_A` / `SRVAL_B` 18-bit features
//!   are left as per-bit lines: no ranged canonical form for them is
//!   established anywhere in the repo or harness, so regrouping would
//!   invent an ordering.
//!
//! # Known coverage gaps (public DB limits, not decoder bugs)
//!
//! - DSP tiles have no `bits` entry in the tilegrid and no segbits
//!   file, so DSP configuration bits are unexplained.
//! - `INT_INTF_R_PCIE4` tiles (95 with frames) have no segbits file,
//!   so their set bits are unexplained.
//! - `PSS_ALTO` has no segbits file; see the overlap note above.
//! - RAMB36 content has no segbits features (no `RAMB36E2.INIT_XX`
//!   lines); BRAM content decodes as two `RAMB18_XxYy` lines. The 42
//!   `RAMB36E2.*` config features in `segbits_bram.db` do decode, on
//!   the `RAMB36_XxYy` site.
//!
//! No em dashes anywhere.

use crate::band_code::{BAND_HI, BAND_LO, BandCheck, check_stored_band};
use crate::bitgen::{
    FRAME_WORDS, far_decode, far_encode, lut_init_index, ram_mode_lut_index, record_lutram_bit,
};
use crate::bitloc::BramHalf;
use crate::bitstream_packets::{Frame, parse_bitstream};
use crate::segbits::{BitRef, FileSegbitsDb, SegbitsDb};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Bits in one UltraScale+ configuration frame (93 words x 32 bits).
const FRAME_BITS: u32 = (FRAME_WORDS as u32) * 32;

/// One unexplained set bit: its FAR, the frame word index holding it,
/// and the bit index within that word.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DecodedBit {
    pub far: u32,
    pub word: usize,
    pub bit: u8,
}

/// Decoder output: canonical FASM lines (sorted, deduplicated) and the
/// set bits no DB feature owns (sorted by FAR, word, bit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeOutput {
    pub fasm: Vec<String>,
    pub unexplained: Vec<DecodedBit>,
}

/// Decode frames into FASM. `db_root` is the prjuray-db `zynqusp`
/// directory: it must contain `xczu3eg-sbva484-1-e/tilegrid.json` (or a
/// top-level `tilegrid.json`, accepted for test fixtures) and the
/// `segbits_*.db` files. Loading failures, a frame with the wrong word
/// count, and a stored H-1 band that does not match the masked MAP-A
/// formula fail loud with a named cause; every other unowned bit is
/// reported in `unexplained`.
pub fn decode_frames(frames: &[Frame], db_root: &str) -> Result<DecodeOutput, String> {
    let windows = load_tilegrid(db_root)?;
    let db = load_segbits(db_root)?;
    let units = build_units(&db)?;
    let mut complements: HashMap<String, HashSet<Vec<(u32, u16)>>> = HashMap::new();
    for tile_type in db.tile_types() {
        complements.insert(
            tile_type.to_string(),
            complement_signatures(&db, tile_type),
        );
    }

    let mut frame_map: HashMap<u32, &[u32]> = HashMap::new();
    for frame in frames {
        frame_map.insert(frame.far, &frame.words);
    }
    let mut set_bits: HashSet<u64> = HashSet::new();
    for frame in frames {
        for (wi, word) in frame.words.iter().enumerate() {
            let mut w = *word;
            while w != 0 {
                let b = w.trailing_zeros();
                set_bits.insert(bit_key(frame.far, (wi as u32) * 32 + b));
                w &= w - 1;
            }
        }
    }
    let mut sorted_fars: Vec<u32> = frame_map.keys().copied().collect();
    sorted_fars.sort_unstable();

    let mut claimed: HashSet<u64> = HashSet::new();
    let mut lines: Vec<String> = Vec::new();
    // Per-tile LUTRAM mask inputs (N7 port): RAM-mode LUT indices
    // decoded on each tile, and candidate INIT positions (LUT index,
    // far, absolute bit) from present LUT INIT features. INIT
    // precedes MODE.RAM in feature order, so candidates are collected
    // first and filtered by the RAM set after the window loop,
    // exactly as `parse_bitstream` does. Only present features
    // contribute; logic-mode INIT bits are never masked.
    let mut tile_ram_luts: HashMap<String, HashSet<u8>> = HashMap::new();
    let mut tile_lut_init_bits: HashMap<String, Vec<(u8, u32, u16)>> = HashMap::new();
    for window in &windows {
        let Some(window_units) = units.get(&window.tile_type) else {
            continue;
        };
        if !window_is_active(window, &frame_map) {
            continue;
        }
        // Lazily computed on first use: whether the window has a set
        // bit a real feature owns (H-1 owned-bit gate, see
        // `window_has_owned_set_bit`).
        let mut owned_set_bit: Option<bool> = None;
        let (block_type, row, col, _) = far_decode(window.base_far);
        for unit in window_units {
            if unit.block != window.block {
                continue;
            }
            let mut abs_refs: Vec<(u32, u32, bool)> = Vec::with_capacity(unit.refs.len());
            let mut out_of_range = false;
            for r in &unit.refs {
                let minor = window.base_minor as u32 + r.frame;
                if minor > u8::MAX as u32 || r.frame >= window.frames {
                    out_of_range = true;
                    break;
                }
                let abs_far = match far_encode(block_type, row, col, minor as u8) {
                    Ok(f) => f,
                    Err(e) => return Err(e.to_string()),
                };
                let abs_bit = r.bit as u32 + window.bit_lo;
                if abs_bit >= window.bit_hi {
                    out_of_range = true;
                    break;
                }
                abs_refs.push((abs_far, abs_bit, r.must_be_zero));
            }
            if out_of_range {
                return Err(format!(
                    "segbits feature {}.{} lists a bit outside tile {} window \
                     (base FAR 0x{:08x}, {} frames, bit window {}..{})",
                    window.tile_type,
                    unit.feature,
                    window.tile_name,
                    window.base_far,
                    window.frames,
                    window.bit_lo,
                    window.bit_hi
                ));
            }
            let set_refs: Vec<&(u32, u32, bool)> =
                abs_refs.iter().filter(|r| !r.2).collect();
            let valued = is_valued(&unit.feature);
            let present: Option<Vec<bool>> = if valued {
                let mut value_bits = vec![false; set_refs.len()];
                let mut any_set = false;
                for (j, r) in set_refs.iter().enumerate() {
                    let v = set_bits.contains(&bit_key(r.0, r.1));
                    value_bits[set_refs.len() - 1 - j] = v;
                    if v {
                        any_set = true;
                    }
                }
                if any_set {
                    Some(value_bits)
                } else {
                    None
                }
            } else {
                let all_set = set_refs
                    .iter()
                    .all(|r| set_bits.contains(&bit_key(r.0, r.1)));
                let all_zero = abs_refs
                    .iter()
                    .filter(|r| r.2)
                    .all(|r| !set_bits.contains(&bit_key(r.0, r.1)));
                let observable = !set_refs.is_empty()
                    || complements
                        .get(&window.tile_type)
                        .map(|s| s.contains(&zero_signature(&unit.refs)))
                        .unwrap_or(false);
                // H-1: an all-`!` complement default renders only when
                // the window has a set bit a real feature owns. A
                // window active on unowned bits alone (the widened
                // RCLK band) emits no complement defaults.
                let owned = if set_refs.is_empty() {
                    *owned_set_bit.get_or_insert_with(|| {
                        window_has_owned_set_bit(window, window_units, &set_bits)
                    })
                } else {
                    true
                };
                if all_set && all_zero && observable && owned {
                    Some(Vec::new())
                } else {
                    None
                }
            };
            if let Some(value_bits) = present {
                for r in &set_refs {
                    claimed.insert(bit_key(r.0, r.1));
                }
                // LUTRAM mask inputs (N7 port): a present MODE.RAM
                // line marks its LUT RAM-mode on this tile; a present
                // LUT INIT line contributes its listed positions as
                // candidates. `must_be_zero` refs are never INIT
                // positions (the emit side records after the same
                // skip), and FF INIT/SRVAL never match
                // `lut_init_index`.
                if let Some(idx) = ram_mode_lut_index(&unit.feature) {
                    tile_ram_luts
                        .entry(window.tile_name.clone())
                        .or_default()
                        .insert(idx);
                }
                if let Some(idx) = lut_init_index(&unit.feature) {
                    let entry = tile_lut_init_bits
                        .entry(window.tile_name.clone())
                        .or_default();
                    for r in abs_refs.iter().filter(|r| !r.2) {
                        entry.push((idx, r.0, r.1 as u16));
                    }
                }
                let tile = fasm_tile_for_window(window, unit)?;
                if valued {
                    lines.push(format!(
                        "{tile}.{} = {}",
                        unit.feature,
                        format_hex_value(&value_bits)
                    ));
                } else {
                    lines.push(format!("{tile}.{}", unit.feature));
                }
            }
        }
    }
    lines.sort();
    lines.dedup();

    // Per-frame LUTRAM masks (N7 port): keep only the INIT positions
    // of RAM-mode LUTs, per tile. Built after the window loop because
    // INIT precedes MODE.RAM in feature order.
    let mut lutram_masks: HashMap<u32, [bool; crate::bitgen::FRAME_BITS]> = HashMap::new();
    for (tile, inits) in &tile_lut_init_bits {
        let Some(ram) = tile_ram_luts.get(tile) else {
            continue;
        };
        for (idx, far, bit) in inits {
            if ram.contains(idx) {
                record_lutram_bit(&mut lutram_masks, *far, *bit);
            }
        }
    }

    let mut unexplained: Vec<DecodedBit> = Vec::new();
    for far in &sorted_fars {
        let words = frame_map[far];
        let words_arr: [u32; FRAME_WORDS] = match words.try_into() {
            Ok(a) => a,
            Err(_) => {
                return Err(format!(
                    "frame FAR 0x{far:08x} has {} words, want {FRAME_WORDS}",
                    words.len()
                ));
            }
        };
        // N7 band check: the band (absolute bits 1440-1487) is a
        // computed code, not a feature. A frame whose stored band
        // matches the masked MAP-A formula (Exact) or the formula XOR
        // the Q residual (QResidual) has its band bits explained;
        // anything else fails loud here, before the unexplained-bits
        // check, exactly as `parse_bitstream` does. Block-1 frames
        // expect a zero band, also checked here. Block-2+ frames are
        // not checked: their band bits stay unexplained, as before.
        let (block, _, _, _) = far_decode(*far);
        if block <= 1 {
            let empty_mask = [false; crate::bitgen::FRAME_BITS];
            let mask = lutram_masks.get(far).unwrap_or(&empty_mask);
            if let BandCheck::Mismatch { stored, expected } =
                check_stored_band(*far, &words_arr, mask)
            {
                return Err(format!(
                    "BandMismatch: FAR 0x{far:08x} has stored band 0x{stored:012x} but the masked MAP-A band is 0x{expected:012x} (a Q frame carries the expected value XOR 0xf0000000000f); the band is a computed code, so a mismatch means the bitstream or the LUTRAM mask derivation is wrong"
                ));
            }
        }
        for (wi, word) in words.iter().enumerate() {
            let mut w = *word;
            while w != 0 {
                let b = w.trailing_zeros() as u8;
                let abs_bit = (wi as u32) * 32 + b as u32;
                // Band bits of a checked frame are explained by the N7
                // check above, never listed. Band bits of a block-2+
                // frame were not checked and stay unexplained.
                let is_band = (BAND_LO as u32..=BAND_HI as u32).contains(&abs_bit);
                if !claimed.contains(&bit_key(*far, abs_bit)) && !(block <= 1 && is_band) {
                    unexplained.push(DecodedBit {
                        far: *far,
                        word: wi,
                        bit: b,
                    });
                }
                w &= w - 1;
            }
        }
    }
    unexplained.sort();
    Ok(DecodeOutput {
        fasm: lines,
        unexplained,
    })
}

/// Parse a bitstream into frames (via Worker A's
/// `crate::bitstream_packets::parse_bitstream`) and decode it.
pub fn decode_bitstream(bytes: &[u8], db_root: &str) -> Result<DecodeOutput, String> {
    let frames = parse_bitstream(bytes)?;
    decode_frames(&frames, db_root)
}

/// One tile window, as loaded from the tilegrid: the tile's name and
/// type, the FAR column it belongs to (`block`, `row`, `col`, with
/// `base_minor` the minor of the column's frame 0), the number of
/// frames in the column, the bit range the tile owns inside every
/// frame of the column (`bit_lo..bit_hi`), its sites, and its grid
/// coordinates (-1 when the tilegrid does not carry them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileWindowInfo {
    pub tile_name: String,
    pub tile_type: String,
    pub block: u8,
    pub row: u8,
    pub col: u16,
    pub base_minor: u8,
    pub frames: u32,
    pub bit_lo: u32,
    pub bit_hi: u32,
    pub sites: Vec<String>,
    pub grid_x: i64,
    pub grid_y: i64,
}

/// Load the tile windows for a `db_root` without decoding anything.
/// Used by analysis tooling (attributing unexplained bits to tiles,
/// deriving base-FAR tables) and by tests that pin the tilegrid model.
pub fn tile_windows(db_root: &str) -> Result<Vec<TileWindowInfo>, String> {
    let windows = load_tilegrid(db_root)?;
    Ok(windows
        .into_iter()
        .map(|w| {
            let (block, row, col, _) = far_decode(w.base_far);
            TileWindowInfo {
                tile_name: w.tile_name,
                tile_type: w.tile_type,
                block,
                row,
                col,
                base_minor: w.base_minor,
                frames: w.frames,
                bit_lo: w.bit_lo,
                bit_hi: w.bit_hi,
                sites: w.sites,
                grid_x: w.grid_x,
                grid_y: w.grid_y,
            }
        })
        .collect())
}

/// One `(far, bit)` position as a single hash key.
fn bit_key(far: u32, bit: u32) -> u64 {
    ((far as u64) << 32) | bit as u64
}

// ---------------------------------------------------------------------------
// Tilegrid
// ---------------------------------------------------------------------------

/// A tilegrid block name: the only two in the xczu3eg tilegrid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum BlockName {
    ClbIoClk,
    BlockRam,
}

impl BlockName {
    fn from_tilegrid(name: &str) -> Option<Self> {
        match name {
            "CLB_IO_CLK" => Some(BlockName::ClbIoClk),
            "BLOCK_RAM" => Some(BlockName::BlockRam),
            _ => None,
        }
    }
}

/// One tile window: the bit range one tile owns inside every frame of
/// its column, for one block. `bit_lo` / `bit_hi` are absolute bit
/// offsets within the frame.
#[derive(Clone, Debug)]
struct TileWindow {
    tile_name: String,
    tile_type: String,
    block: BlockName,
    base_far: u32,
    base_minor: u8,
    frames: u32,
    bit_lo: u32,
    bit_hi: u32,
    sites: Vec<String>,
    grid_x: i64,
    grid_y: i64,
}

fn parse_hex_u32(text: &str) -> Option<u32> {
    let digits = text.strip_prefix("0x").unwrap_or(text);
    u32::from_str_radix(digits, 16).ok()
}

fn load_tilegrid(db_root: &str) -> Result<Vec<TileWindow>, String> {
    let root = Path::new(db_root);
    let part_path = root.join("xczu3eg-sbva484-1-e").join("tilegrid.json");
    let path = if part_path.is_file() {
        part_path
    } else {
        root.join("tilegrid.json")
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read tilegrid {}: {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("cannot parse tilegrid {}: {e}", path.display()))?;
    let obj = value
        .as_object()
        .ok_or_else(|| format!("tilegrid {} is not a JSON object", path.display()))?;
    let mut windows = Vec::new();
    for (tile_name, tile) in obj {
        let tile_type = tile
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("tilegrid tile {tile_name} has no string type"))?
            .to_string();
        let Some(bits) = tile.get("bits").and_then(|v| v.as_object()) else {
            continue;
        };
        let mut sites: Vec<String> = tile
            .get("sites")
            .and_then(|v| v.as_object())
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        sites.sort();
        // Grid coordinates are advisory (adjacency, B2 table derivation);
        // a fixture tilegrid without them still decodes.
        let grid_x = tile.get("grid_x").and_then(|v| v.as_i64()).unwrap_or(-1);
        let grid_y = tile.get("grid_y").and_then(|v| v.as_i64()).unwrap_or(-1);
        for (block_name, info) in bits {
            let block = BlockName::from_tilegrid(block_name).ok_or_else(|| {
                format!("tilegrid tile {tile_name} has unknown block {block_name}")
            })?;
            let base_str = info
                .get("baseaddr")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    format!("tilegrid tile {tile_name} block {block_name} has no baseaddr")
                })?;
            let base_far = parse_hex_u32(base_str).ok_or_else(|| {
                format!(
                    "tilegrid tile {tile_name} block {block_name} has bad baseaddr {base_str:?}"
                )
            })?;
            let frames = info
                .get("frames")
                .and_then(|v| v.as_u64())
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| {
                    format!("tilegrid tile {tile_name} block {block_name} has no frames count")
                })?;
            let offset = info
                .get("offset")
                .and_then(|v| v.as_u64())
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| {
                    format!("tilegrid tile {tile_name} block {block_name} has no offset")
                })?;
            let words = info
                .get("words")
                .and_then(|v| v.as_u64())
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| {
                    format!("tilegrid tile {tile_name} block {block_name} has no words count")
                })?;
            let bit_lo = offset.checked_mul(16).ok_or_else(|| {
                format!("tilegrid tile {tile_name} block {block_name} offset overflows")
            })?;
            let bit_hi = bit_lo
                .checked_add(words.checked_mul(16).ok_or_else(|| {
                    format!("tilegrid tile {tile_name} block {block_name} words overflow")
                })?)
                .ok_or_else(|| {
                    format!("tilegrid tile {tile_name} block {block_name} window overflows")
                })?;
            if bit_hi > FRAME_BITS {
                return Err(format!(
                    "tilegrid tile {tile_name} block {block_name} window ends at bit \
                     {bit_hi}, past the {FRAME_BITS}-bit frame"
                ));
            }
            let (_, _, _, base_minor) = far_decode(base_far);
            windows.push(TileWindow {
                tile_name: tile_name.clone(),
                tile_type: tile_type.clone(),
                block,
                base_far,
                base_minor,
                frames,
                bit_lo,
                bit_hi,
                sites: sites.clone(),
                grid_x,
                grid_y,
            });
        }
    }
    windows.sort_by(|a, b| {
        (a.tile_name.as_str(), a.block).cmp(&(b.tile_name.as_str(), b.block))
    });
    Ok(windows)
}

/// True when any input frame in the window's minor range has a set bit
/// inside the window's bit range.
fn window_is_active(window: &TileWindow, frame_map: &HashMap<u32, &[u32]>) -> bool {
    let (block_type, row, col, _) = far_decode(window.base_far);
    let first_word = (window.bit_lo / 32) as usize;
    let last_word = ((window.bit_hi - 1) / 32) as usize;
    for minor in 0..window.frames {
        let minor_full = window.base_minor as u32 + minor;
        if minor_full > u8::MAX as u32 {
            continue;
        }
        let Ok(far) = far_encode(block_type, row, col, minor_full as u8) else {
            continue;
        };
        let Some(words) = frame_map.get(&far) else {
            continue;
        };
        for wi in first_word..=last_word {
            let Some(word) = words.get(wi) else {
                continue;
            };
            let mut w = *word;
            if wi == first_word {
                let lo = window.bit_lo % 32;
                if lo > 0 {
                    w &= !0u32 << lo;
                }
            }
            if wi == last_word {
                let hi = window.bit_hi % 32;
                if hi > 0 {
                    w &= (1u32 << hi) - 1;
                }
            }
            if w != 0 {
                return true;
            }
        }
    }
    false
}

/// True when the window has at least one set bit at a position a real
/// feature of the window actually owns: a non-`!` ref of some unit of
/// the window's tile type and block, resolved to its absolute
/// (FAR, bit) exactly as the main loop resolves it, and present in
/// `set_bits`.
///
/// H-1: widening an RCLK window from bit 1488 down to 1440 activates
/// the window on set bits in the new band (relative bits 0-47), bits
/// no feature owns yet. Without this gate the newly active window
/// renders its all-`!` complement-default lines (their `!` bits all
/// read 0), inventing FASM lines bit2fasm never emits. A window whose
/// only set bits are unowned is not evidence any of its features is
/// configured, so it emits no complement defaults. A window with an
/// owned set bit is unchanged. (bit2fasm's group-drop rule is the
/// stricter form of this gate; the owned-bit form is used here because
/// it is the minimal change that fixes the H-1 gate while keeping the
/// validated unpatched-root normalized output exact. On the raw
/// unpatched output it removes only 48 pre-existing spurious `.V0`
/// lines in P-11 (one CMT tile, 0 of them in the bit2fasm baseline).
/// If the normalized diff ever shows a decoded-only all-`!` line from
/// a window with owned set bits but no matched non-zero feature, the
/// gate must be tightened to "at least one claimed set bit".)
fn window_has_owned_set_bit(
    window: &TileWindow,
    units: &[Unit],
    set_bits: &HashSet<u64>,
) -> bool {
    let (block_type, row, col, _) = far_decode(window.base_far);
    units
        .iter()
        .filter(|unit| unit.block == window.block)
        .flat_map(|unit| unit.refs.iter())
        .filter(|r| !r.must_be_zero)
        .any(|r| {
            // Same bounds as the main loop: a ref outside the
            // window's frame range has no absolute position and can
            // never be an owned set bit (the main loop itself errors
            // on it when it evaluates that unit).
            if r.frame >= window.frames {
                return false;
            }
            let minor = window.base_minor as u32 + r.frame;
            if minor > u8::MAX as u32 {
                return false;
            }
            let Some(far) = far_encode(block_type, row, col, minor as u8).ok() else {
                return false;
            };
            let abs_bit = r.bit as u32 + window.bit_lo;
            if abs_bit >= window.bit_hi {
                return false;
            }
            set_bits.contains(&bit_key(far, abs_bit))
        })
}

// ---------------------------------------------------------------------------
// Segbits
// ---------------------------------------------------------------------------

/// Load and merge every `segbits_*.db` under `db_root` (excluding
/// `*.origin_info.db`), then synthesize the LUT INIT regrouping.
fn load_segbits(db_root: &str) -> Result<FileSegbitsDb, String> {
    let root = Path::new(db_root);
    let mut names: Vec<PathBuf> = Vec::new();
    let entries = std::fs::read_dir(root)
        .map_err(|e| format!("cannot read segbits DB root {}: {e}", root.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|e| format!("cannot read segbits DB root {}: {e}", root.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("segbits_") && name.ends_with(".db") && !name.contains("origin_info")
        {
            names.push(entry.path());
        }
    }
    names.sort();
    if names.is_empty() {
        return Err(format!("no segbits_*.db files in {db_root}"));
    }
    let refs: Vec<&Path> = names.iter().map(PathBuf::as_path).collect();
    let mut db = FileSegbitsDb::from_files(&refs)
        .map_err(|e| format!("cannot load segbits DBs in {db_root}: {e}"))?;
    db.synthesize_lut_init()
        .map_err(|e| format!("cannot synthesize LUT INIT features: {e}"))?;
    Ok(db)
}

/// Which site of a BRAM tile a feature belongs to: one of the two
/// RAMB18 halves, or the whole RAMB36. A tile is one RAMB36 or two
/// RAMB18, never both; the config features named `RAMB36E2.*` in
/// `segbits_bram.db` (42 of them: clock inversion, DOA_REG,
/// CLOCK_DOMAINS, and the like) belong to the whole 36.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BramSite {
    Lower,
    Upper,
    Whole36,
}

impl BramSite {
    fn from_half(half: BramHalf) -> BramSite {
        match half {
            BramHalf::Lower => BramSite::Lower,
            BramHalf::Upper => BramSite::Upper,
        }
    }
}

/// One decodable feature: its FASM feature name, its DB-relative bit
/// refs, the tilegrid block it lives in, and (BRAM only) the site it
/// belongs to. For regrouped features the name is the ranged form
/// (`A6LUT.INIT[63:0]`, `INIT[16383:0]`) and the refs are MSB-first.
#[derive(Clone, Debug)]
struct Unit {
    feature: String,
    refs: Vec<BitRef>,
    block: BlockName,
    bram_site: Option<BramSite>,
}

/// A feature is value-carrying when its name uses a bit range or a
/// BRAM-style indexed INIT name. Same rule as `bitgen.rs`.
fn is_valued(feature: &str) -> bool {
    feature.contains('[') || feature.contains("INIT_") || feature.contains("INITP_")
}

/// The tilegrid block a feature lives in. BRAM content lines
/// (`INIT_XX[i]` / `INITP_XX[i]`) live in the BLOCK_RAM block; every
/// other feature, including BRAM `INIT_A[i]` site config, lives in the
/// CLB_IO_CLK block.
fn unit_block(tile_type: &str, feature: &str) -> BlockName {
    if tile_type == "BRAM" && parse_bram_content_feature(feature).is_some() {
        BlockName::BlockRam
    } else {
        BlockName::ClbIoClk
    }
}

/// Parse a BRAM content per-bit feature name
/// (`RAMB18E2_L.INIT_00[5]`, `RAMB18E2_U.INITP_07[255]`) into
/// (half, is parity, absolute content bit). Returns `None` for any
/// other feature, including BRAM `INIT_A[i]` site config.
fn parse_bram_content_feature(feature: &str) -> Option<(BramHalf, bool, u32)> {
    let (head, idx_part) = feature.split_once('[')?;
    let idx: u32 = idx_part.strip_suffix(']')?.parse().ok()?;
    let mut parts = head.split('.');
    let bel = parts.next()?;
    let word = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let half = match bel {
        "RAMB18E2_L" => BramHalf::Lower,
        "RAMB18E2_U" => BramHalf::Upper,
        _ => return None,
    };
    let (is_parity, word_hex) = match word.strip_prefix("INITP_") {
        Some(h) => (true, h),
        None => match word.strip_prefix("INIT_") {
            Some(h) => (false, h),
            None => return None,
        },
    };
    if word_hex.len() != 2 || !word_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let word_no = u32::from_str_radix(word_hex, 16).ok()?;
    if idx >= 256 {
        return None;
    }
    Some((half, is_parity, word_no * 256 + idx))
}

/// The LUT letter of a per-bit feature (`ALUT.INIT[5]` -> `A`), or
/// `None` for any other feature name.
fn lut_per_bit_letter(feature: &str) -> Option<char> {
    let mut chars = feature.chars();
    let letter = chars.next()?;
    if !('A'..='H').contains(&letter) {
        return None;
    }
    let rest: String = chars.collect();
    if !rest.starts_with("LUT.INIT[") || !rest.ends_with(']') {
        return None;
    }
    let idx = &rest["LUT.INIT[".len()..rest.len() - 1];
    if idx.is_empty() || !idx.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(letter)
}

/// The regrouped feature name for a per-bit LUT feature
/// (`ALUT.INIT[5]` -> `A6LUT.INIT[63:0]`).
fn lut_synth_name(feature: &str) -> Option<String> {
    lut_per_bit_letter(feature).map(|l| format!("{l}6LUT.INIT[63:0]"))
}

/// The BRAM site of a feature name: `RAMB18E2_L` / `RAMB18E2_U`
/// halves, or the whole `RAMB36E2`. `None` when the name carries no
/// site.
fn parse_bram_site(feature: &str) -> Option<BramSite> {
    if feature.contains("RAMB18E2_L.") {
        Some(BramSite::Lower)
    } else if feature.contains("RAMB18E2_U.") {
        Some(BramSite::Upper)
    } else if feature.contains("RAMB36E2.") {
        Some(BramSite::Whole36)
    } else {
        None
    }
}

/// Build the decodable units per tile type: DB features as-is, except
/// per-bit LUT lines suppressed when the regrouped `[63:0]` feature
/// exists, and BRAM content lines regrouped into whole-range units
/// (or left per-bit when the group is incomplete in the DB).
fn build_units(db: &dyn SegbitsDb) -> Result<HashMap<String, Vec<Unit>>, String> {
    let mut out: HashMap<String, Vec<Unit>> = HashMap::new();
    for tile_type in db.tile_types() {
        let mut units: Vec<Unit> = Vec::new();
        let mut bram_pending: Vec<(String, BitRef, BramHalf, bool, u32)> = Vec::new();
        for feature in db.features(tile_type) {
            let refs = db.lookup(tile_type, feature).ok_or_else(|| {
                format!(
                    "segbits feature {tile_type}.{feature} is listed but lookup returns none"
                )
            })?;
            if tile_type == "BRAM" {
                if let Some((half, is_parity, abs)) = parse_bram_content_feature(feature) {
                    if refs.len() != 1 || refs[0].must_be_zero {
                        return Err(format!(
                            "BRAM content feature {tile_type}.{feature} must list exactly one set bit"
                        ));
                    }
                    bram_pending.push((feature.to_string(), refs[0], half, is_parity, abs));
                    continue;
                }
            }
            if let Some(synth) = lut_synth_name(feature) {
                if db.lookup(tile_type, &synth).is_some() {
                    continue;
                }
            }
            let block = unit_block(tile_type, feature);
            let bram_site = if tile_type == "BRAM" {
                Some(parse_bram_site(feature).ok_or_else(|| {
                    format!("BRAM feature {tile_type}.{feature} names no RAMB18E2_L/U or RAMB36E2 site")
                })?)
            } else {
                None
            };
            units.push(Unit {
                feature: feature.to_string(),
                refs: refs.to_vec(),
                block,
                bram_site,
            });
        }
        if tile_type == "BRAM" {
            let mut groups: HashMap<(BramHalf, bool), Vec<(String, BitRef, u32)>> =
                HashMap::new();
            for (feature, r, half, is_parity, abs) in bram_pending {
                groups
                    .entry((half, is_parity))
                    .or_default()
                    .push((feature, r, abs));
            }
            for ((half, is_parity), items) in groups {
                let total: u32 = if is_parity { 2048 } else { 16384 };
                let mut by_abs: HashMap<u32, BitRef> = HashMap::new();
                for (_, r, abs) in &items {
                    by_abs.insert(*abs, *r);
                }
                let complete =
                    by_abs.len() == total as usize && by_abs.keys().all(|a| *a < total);
                if complete {
                    let mut refs: Vec<BitRef> = Vec::with_capacity(total as usize);
                    for abs in (0..total).rev() {
                        refs.push(by_abs[&abs]);
                    }
                    let feature = if is_parity {
                        "INITP[2047:0]".to_string()
                    } else {
                        "INIT[16383:0]".to_string()
                    };
                    units.push(Unit {
                        feature,
                        refs,
                        block: BlockName::BlockRam,
                        bram_site: Some(BramSite::from_half(half)),
                    });
                } else {
                    for (feature, r, _) in items {
                        units.push(Unit {
                            feature,
                            refs: vec![r],
                            block: BlockName::BlockRam,
                            bram_site: Some(BramSite::from_half(half)),
                        });
                    }
                }
            }
        }
        units.sort_by(|a, b| a.feature.cmp(&b.feature));
        out.insert(tile_type.to_string(), units);
    }
    Ok(out)
}

/// Sorted (frame, bit) pairs of a feature's `!` bits.
fn zero_signature(refs: &[BitRef]) -> Vec<(u32, u16)> {
    let mut sig: Vec<(u32, u16)> = refs
        .iter()
        .filter(|r| r.must_be_zero)
        .map(|r| (r.frame, r.bit))
        .collect();
    sig.sort();
    sig
}

/// Set-bit signatures of the features of one tile type that list no `!`
/// bits. An all-`!` feature whose zero signature is in this set is the
/// exact complement of one of these features (same rule as
/// `bitgen.rs`).
fn complement_signatures(db: &dyn SegbitsDb, tile_type: &str) -> HashSet<Vec<(u32, u16)>> {
    let mut set = HashSet::new();
    for feature in db.features(tile_type) {
        if let Some(refs) = db.lookup(tile_type, feature) {
            if refs.is_empty() || refs.iter().any(|r| r.must_be_zero) {
                continue;
            }
            let mut sig: Vec<(u32, u16)> = refs.iter().map(|r| (r.frame, r.bit)).collect();
            sig.sort();
            set.insert(sig);
        }
    }
    set
}

/// The FASM tile for a unit on a window: the single SLICE site for CLB
/// tiles, the RAMB18 site for the unit's half (or the RAMB36 site for a
/// whole-36 feature) for BRAM tiles, the tilegrid tile name for
/// everything else.
fn fasm_tile_for_window(window: &TileWindow, unit: &Unit) -> Result<String, String> {
    match window.tile_type.as_str() {
        "CLEL_L" | "CLEL_R" | "CLEM" | "CLEM_R" => {
            let slices: Vec<&String> = window
                .sites
                .iter()
                .filter(|s| s.starts_with("SLICE_"))
                .collect();
            if slices.len() != 1 {
                return Err(format!(
                    "CLB tile {} has {} SLICE sites, want exactly one",
                    window.tile_name,
                    slices.len()
                ));
            }
            Ok(slices[0].clone())
        }
        "BRAM" => {
            let site = unit.bram_site.ok_or_else(|| {
                format!(
                    "BRAM feature {} has no site for tile {}",
                    unit.feature, window.tile_name
                )
            })?;
            match site {
                BramSite::Whole36 => {
                    let ramb36: Vec<&String> = window
                        .sites
                        .iter()
                        .filter(|s| s.starts_with("RAMB36_X"))
                        .collect();
                    if ramb36.len() != 1 {
                        return Err(format!(
                            "BRAM tile {} has {} RAMB36 sites, want exactly one",
                            window.tile_name,
                            ramb36.len()
                        ));
                    }
                    Ok(ramb36[0].clone())
                }
                BramSite::Lower | BramSite::Upper => {
                    let mut ramb: Vec<(&String, u32)> = Vec::new();
                    for site in &window.sites {
                        if let Some(y) = parse_site_y(site, "RAMB18_X") {
                            ramb.push((site, y));
                        }
                    }
                    if ramb.len() != 2 {
                        return Err(format!(
                            "BRAM tile {} has {} RAMB18 sites, want exactly two",
                            window.tile_name,
                            ramb.len()
                        ));
                    }
                    ramb.sort_by_key(|(_, y)| *y);
                    Ok(match site {
                        BramSite::Lower => ramb[0].0.clone(),
                        BramSite::Upper => ramb[1].0.clone(),
                        BramSite::Whole36 => unreachable!(),
                    })
                }
            }
        }
        _ => Ok(window.tile_name.clone()),
    }
}

/// The Y number of a site name like `RAMB18_X0Y0` after the prefix
/// `RAMB18_X`, or `None` when the name does not match.
fn parse_site_y(site: &str, prefix: &str) -> Option<u32> {
    let rest = site.strip_prefix(prefix)?;
    let (_, y) = rest.split_once('Y')?;
    y.parse().ok()
}

/// Format LSB-first value bits as the harness canonical hex: lowercase
/// `0x` with no redundant leading zeros (`0x0` for zero).
fn format_hex_value(value_bits: &[bool]) -> String {
    let n = value_bits.len();
    let hex_digits = n.div_ceil(4);
    let mut hex = String::with_capacity(hex_digits);
    for d in (0..hex_digits).rev() {
        let mut nibble = 0u8;
        for b in 0..4 {
            let i = d * 4 + b;
            if i < n && value_bits[i] {
                nibble |= 1 << b;
            }
        }
        hex.push(char::from_digit(nibble as u32, 16).unwrap());
    }
    let trimmed = hex.trim_start_matches('0');
    if trimmed.is_empty() {
        "0x0".to_string()
    } else {
        format!("0x{trimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::band_code::{BandFormula, Q_RESIDUAL, compute_band, frame_band, write_band};
    use crate::bitgen::{
        cmd, reg, BUS_WIDTH_DETECT, BUS_WIDTH_SYNC, DUMMY_WORD, NOOP_WORD, SYNC_WORD,
    };
    use std::fs;
    use std::path::PathBuf;

    /// xczu3eg IDCODE (Worker A's embedded geometry table).
    const XCZU3EG_IDCODE: u32 = 0x04A4_2093;

    /// Fresh fixture directory under the temp dir (removed first so
    /// reruns are clean).
    fn fixture_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("helion_bitstream_decode_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_fixture(dir: &Path, tilegrid: &str, segbits: &[(&str, &str)]) {
        fs::write(dir.join("tilegrid.json"), tilegrid).unwrap();
        for (name, text) in segbits {
            fs::write(dir.join(name), text).unwrap();
        }
    }

    /// One frame with exactly the given absolute bit offsets set,
    /// carrying the computed H-1 band for those bits (empty LUTRAM
    /// mask: no fixture DB here carries a RAM-mode LUT, so no INIT
    /// bit is masked; block-1 frames get a zero band through
    /// `compute_band`). A fixture frame is a valid bitstream frame,
    /// and since the N7 port the decoder fails loud on a stored band
    /// that does not match the masked MAP-A formula. Band bits are
    /// never inputs to the formula, so stamping after setting the
    /// content bits is exact.
    fn frame(far: u32, set_bits: &[u32]) -> Frame {
        let mut words = [0u32; FRAME_WORDS];
        for bit in set_bits {
            words[(*bit as usize) / 32] |= 1u32 << (bit % 32);
        }
        let band = compute_band(
            far,
            &words,
            &[false; crate::bitgen::FRAME_BITS],
            BandFormula::MapA,
        );
        write_band(&mut words, band);
        Frame {
            far,
            words: words.to_vec(),
        }
    }

    /// One CLEL_L tile: base FAR 0x00000F00, 16 frames, window bits
    /// 0..48, single site SLICE_X0Y0.
    const CLB_TILEGRID: &str = r#"{
      "CLEL_L_X0Y0": {"bits": {"CLB_IO_CLK": {"baseaddr": "0x00000F00", "frames": 16, "offset": 0, "words": 3}}, "grid_x": 180, "grid_y": 185, "pin_functions": {}, "sites": {"SLICE_X0Y0": "SLICEL"}, "type": "CLEL_L"}
    }"#;

    /// Same tile with offset 3 (window bits 48..96), site SLICE_X0Y1.
    const CLB_TILEGRID_OFF3: &str = r#"{
      "CLEL_L_X0Y1": {"bits": {"CLB_IO_CLK": {"baseaddr": "0x00000F00", "frames": 16, "offset": 3, "words": 3}}, "grid_x": 180, "grid_y": 184, "pin_functions": {}, "sites": {"SLICE_X0Y1": "SLICEL"}, "type": "CLEL_L"}
    }"#;

    /// Verbatim-style CLB features: an FF INIT/SRVAL complement pair,
    /// a CEUSED complement pair, and an all-`!` mux default with no
    /// complement. No LUT lines here, so LUT tests build their own.
    const CLB_SEGBITS: &str = "\
CLEL_L.AFF.INIT.V0 12_02\n\
CLEL_L.AFF.INIT.V1 !12_02\n\
CLEL_L.AFF.SRVAL.V0 12_01\n\
CLEL_L.AFF.SRVAL.V1 !12_01\n\
CLEL_L.ABCDFF.CEUSED.V0 !14_07\n\
CLEL_L.ABCDFF.CEUSED.V1 14_07\n\
CLEL_L.SOME.MUX.DEFAULT !13_03\n";

    #[test]
    fn known_feature_decodes_to_exactly_that_feature() {
        let dir = fixture_dir("known");
        write_fixture(&dir, CLB_TILEGRID, &[("segbits_clel_l.db", CLB_SEGBITS)]);
        // AFF.INIT.V0 sets frame 12 bit 2: FAR 0xF00 + 12, abs bit 2.
        let frames = vec![frame(0x0000_0F0C, &[2])];
        let out = decode_frames(&frames, dir.to_str().unwrap()).unwrap();
        // Complete FASM: the touched tile also renders the complement
        // line for each untouched FF attribute (SRVAL reads 0, so V1;
        // CEUSED reads 0, so V0). The mux default has no complement
        // and stays absent.
        assert_eq!(
            out.fasm,
            vec![
                "SLICE_X0Y0.ABCDFF.CEUSED.V0".to_string(),
                "SLICE_X0Y0.AFF.INIT.V0".to_string(),
                "SLICE_X0Y0.AFF.SRVAL.V1".to_string(),
            ]
        );
        assert!(out.unexplained.is_empty(), "unexplained: {:?}", out.unexplained);
    }

    #[test]
    fn bit_with_no_owning_feature_lands_in_unexplained() {
        let dir = fixture_dir("unknown_bit");
        write_fixture(&dir, CLB_TILEGRID, &[("segbits_clel_l.db", CLB_SEGBITS)]);
        // Frame 12 bit 5: no feature lists (12, 5) as a set bit.
        // Frame 12 bit 48: outside the tile window [0, 48).
        let frames = vec![frame(0x0000_0F0C, &[5, 48])];
        let out = decode_frames(&frames, dir.to_str().unwrap()).unwrap();
        assert_eq!(
            out.unexplained,
            vec![
                DecodedBit {
                    far: 0x0000_0F0C,
                    word: 0,
                    bit: 5
                },
                DecodedBit {
                    far: 0x0000_0F0C,
                    word: 1,
                    bit: 16
                },
            ]
        );
    }

    #[test]
    fn must_be_zero_set_bit_is_unexplained() {
        let dir = fixture_dir("must_be_zero");
        write_fixture(&dir, CLB_TILEGRID, &[("segbits_clel_l.db", CLB_SEGBITS)]);
        // SOME.MUX.DEFAULT lists !13_03 and nothing lists 13_03 as a
        // set bit, so the set bit is unexplained and the feature stays
        // absent.
        let frames = vec![frame(0x0000_0F0D, &[3])];
        let out = decode_frames(&frames, dir.to_str().unwrap()).unwrap();
        assert_eq!(
            out.unexplained,
            vec![DecodedBit {
                far: 0x0000_0F0D,
                word: 0,
                bit: 3
            }]
        );
        assert!(
            !out.fasm.iter().any(|l| l.contains("MUX.DEFAULT")),
            "fasm: {:?}",
            out.fasm
        );
    }

    /// H-1: a window active only on bits no feature owns renders no
    /// complement-default lines. Frame 12 bit 5 is inside the window
    /// [0, 48) but no feature lists it as a set bit, so the window has
    /// no owned set bit and the all-`!` complement lines (AFF.INIT.V1,
    /// AFF.SRVAL.V1, ABCDFF.CEUSED.V0) must stay absent. Without the
    /// owned-bit gate they render, inventing FASM lines bit2fasm
    /// never emits. The owned-bit case is covered by
    /// `known_feature_decodes_to_exactly_that_feature`, which asserts
    /// the complement lines do render when an owned bit is set.
    #[test]
    fn window_active_only_on_unowned_bits_renders_no_complement_defaults() {
        let dir = fixture_dir("unowned_only");
        write_fixture(&dir, CLB_TILEGRID, &[("segbits_clel_l.db", CLB_SEGBITS)]);
        let frames = vec![frame(0x0000_0F0C, &[5])];
        let out = decode_frames(&frames, dir.to_str().unwrap()).unwrap();
        assert!(out.fasm.is_empty(), "fasm: {:?}", out.fasm);
        assert_eq!(
            out.unexplained,
            vec![DecodedBit {
                far: 0x0000_0F0C,
                word: 0,
                bit: 5
            }]
        );
    }

    /// N7 port: a valid frame (band stamped by `frame()`) with one
    /// band bit flipped. The stored band no longer matches the masked
    /// MAP-A formula, so decode must fail loud naming the FAR, not
    /// list the band bits as unexplained.
    #[test]
    fn flipped_band_bit_fails_loud_with_band_mismatch() {
        let dir = fixture_dir("flipped_band");
        write_fixture(&dir, CLB_TILEGRID, &[("segbits_clel_l.db", CLB_SEGBITS)]);
        let mut f = frame(0x0000_0F0C, &[2]);
        f.words[45] ^= 1;
        let err = decode_frames(&[f], dir.to_str().unwrap()).unwrap_err();
        assert!(err.contains("BandMismatch"), "err: {err}");
        assert!(err.contains("0x00000f0c"), "err: {err}");
    }

    /// N7 port: stored band = formula XOR the Q residual is a Q
    /// frame. Decode accepts it and the band bits are explained, not
    /// listed. The content bits are the known-feature frame, so the
    /// FASM is the known-feature FASM.
    #[test]
    fn q_residual_band_is_accepted_not_unexplained() {
        let dir = fixture_dir("q_band");
        write_fixture(&dir, CLB_TILEGRID, &[("segbits_clel_l.db", CLB_SEGBITS)]);
        let mut f = frame(0x0000_0F0C, &[2]);
        let mut words: [u32; FRAME_WORDS] = f.words.try_into().unwrap();
        let stored = frame_band(&words);
        write_band(&mut words, stored ^ Q_RESIDUAL);
        f.words = words.to_vec();
        let out = decode_frames(&[f], dir.to_str().unwrap()).unwrap();
        assert_eq!(
            out.fasm,
            vec![
                "SLICE_X0Y0.ABCDFF.CEUSED.V0".to_string(),
                "SLICE_X0Y0.AFF.INIT.V0".to_string(),
                "SLICE_X0Y0.AFF.SRVAL.V1".to_string(),
            ]
        );
        assert!(
            out.unexplained.is_empty(),
            "unexplained: {:?}",
            out.unexplained
        );
    }

    #[test]
    fn frame_with_no_tile_is_unexplained_not_dropped() {
        let dir = fixture_dir("unknown_far");
        write_fixture(&dir, CLB_TILEGRID, &[("segbits_clel_l.db", CLB_SEGBITS)]);
        // Column 0x99, minor 0: no tilegrid tile claims this FAR.
        let frames = vec![frame(0x0000_9900, &[0])];
        let out = decode_frames(&frames, dir.to_str().unwrap()).unwrap();
        assert_eq!(
            out.unexplained,
            vec![DecodedBit {
                far: 0x0000_9900,
                word: 0,
                bit: 0
            }]
        );
        assert!(out.fasm.is_empty(), "fasm: {:?}", out.fasm);
    }

    #[test]
    fn lut_init_per_bit_lines_regroup_into_64bit_valued_feature() {
        // 64 per-bit lines, frame 8 + i/16, bit i%16 (the real DB
        // layout for A6LUT: frames 8..11, bits 0..15).
        let mut segbits = String::new();
        for i in 0..64 {
            segbits.push_str(&format!(
                "CLEL_L.ALUT.INIT[{i}] {:02}_{:02}\n",
                8 + i / 16,
                i % 16
            ));
        }
        let dir = fixture_dir("lut");
        write_fixture(&dir, CLB_TILEGRID, &[("segbits_clel_l.db", &segbits)]);
        // INIT bit 0 -> (frame 8, bit 0); INIT bit 63 -> (frame 11,
        // bit 15). Value is 0x8000000000000001.
        let frames = vec![frame(0x0000_0F08, &[0]), frame(0x0000_0F0B, &[15])];
        let out = decode_frames(&frames, dir.to_str().unwrap()).unwrap();
        assert_eq!(
            out.fasm,
            vec!["SLICE_X0Y0.A6LUT.INIT[63:0] = 0x8000000000000001".to_string()]
        );
        assert!(out.unexplained.is_empty(), "unexplained: {:?}", out.unexplained);
    }

    #[test]
    fn nonzero_tile_offset_shifts_bit_positions() {
        let dir = fixture_dir("offset3");
        write_fixture(
            &dir,
            CLB_TILEGRID_OFF3,
            &[("segbits_clel_l.db", CLB_SEGBITS)],
        );
        // AFF.INIT.V0 is 12_02; tile offset 3 shifts the window by
        // 48 bits, so the set bit is absolute bit 50 of FAR 0xF0C.
        let frames = vec![frame(0x0000_0F0C, &[50])];
        let out = decode_frames(&frames, dir.to_str().unwrap()).unwrap();
        assert_eq!(
            out.fasm,
            vec![
                "SLICE_X0Y1.ABCDFF.CEUSED.V0".to_string(),
                "SLICE_X0Y1.AFF.INIT.V0".to_string(),
                "SLICE_X0Y1.AFF.SRVAL.V1".to_string(),
            ]
        );
        assert!(out.unexplained.is_empty(), "unexplained: {:?}", out.unexplained);
    }

    #[test]
    fn bram_content_regroups_and_config_stays_on_clb_block() {
        const BRAM_TILEGRID: &str = r#"{
          "BRAM_X0Y0": {"bits": {"BLOCK_RAM": {"baseaddr": "0x01000000", "frames": 256, "offset": 0, "words": 15}, "CLB_IO_CLK": {"baseaddr": "0x00000600", "frames": 6, "offset": 0, "words": 15}}, "grid_x": 167, "grid_y": 185, "pin_functions": {}, "sites": {"RAMB18_X0Y0": "RAMBFIFO18", "RAMB18_X0Y1": "RAMB181", "RAMB36_X0Y0": "RAMBFIFO36"}, "type": "BRAM"}
        }"#;
        // Full lower-half content DB: abs bit -> (frame abs/240,
        // bit abs%240), a valid injective layout inside the 256-frame
        // x 240-bit BLOCK_RAM window.
        let mut block_ram = String::new();
        for abs in 0..16384u32 {
            block_ram.push_str(&format!(
                "BRAM.RAMB18E2_L.INIT_{:02X}[{}] {:02}_{:02}\n",
                abs / 256,
                abs % 256,
                abs / 240,
                abs % 240
            ));
        }
        const BRAM_CONFIG: &str = "\
BRAM.RAMB18E2_L.CLKARDCLKINV.V0 !05_91\n\
BRAM.RAMB18E2_L.CLKARDCLKINV.V1 05_91\n";
        let dir = fixture_dir("bram");
        write_fixture(
            &dir,
            BRAM_TILEGRID,
            &[
                ("segbits_bram.db", BRAM_CONFIG),
                ("segbits_bram.block_ram.db", &block_ram),
            ],
        );
        // Content bits 0 and 16383: abs 0 -> (frame 0, bit 0),
        // abs 16383 -> (frame 68, bit 63). Config V1 bit: frame 5,
        // bit 91 on the CLB_IO_CLK block (FAR 0x605).
        let frames = vec![
            frame(0x0100_0000, &[0]),
            frame(0x0100_0044, &[63]),
            frame(0x0000_0605, &[91]),
        ];
        let out = decode_frames(&frames, dir.to_str().unwrap()).unwrap();
        let value = format!("0x8{}1", "0".repeat(4094));
        assert_eq!(
            out.fasm,
            vec![
                format!("RAMB18_X0Y0.INIT[16383:0] = {value}"),
                "RAMB18_X0Y0.RAMB18E2_L.CLKARDCLKINV.V1".to_string(),
            ]
        );
        assert!(out.unexplained.is_empty(), "unexplained: {:?}", out.unexplained);
    }

    #[test]
    fn ramb36_config_decodes_on_ramb36_site() {
        // Real `segbits_bram.db` refs for the whole-36 clock-invert
        // feature (config block, frames 4..5). A `RAMB36E2.*` feature
        // used to fail the whole decode with "names no RAMB18E2_L/U
        // half"; it must decode on the tile's RAMB36 site instead.
        const BRAM_TILEGRID: &str = r#"{
          "BRAM_X0Y0": {"bits": {"BLOCK_RAM": {"baseaddr": "0x01000000", "frames": 256, "offset": 0, "words": 15}, "CLB_IO_CLK": {"baseaddr": "0x00000600", "frames": 6, "offset": 0, "words": 15}}, "grid_x": 167, "grid_y": 185, "pin_functions": {}, "sites": {"RAMB18_X0Y0": "RAMBFIFO18", "RAMB18_X0Y1": "RAMB181", "RAMB36_X0Y0": "RAMBFIFO36"}, "type": "BRAM"}
        }"#;
        const BRAM_CONFIG: &str = "\
BRAM.RAMB36E2.CLKARDCLKINV.V0 !04_165 !05_91\n\
BRAM.RAMB36E2.CLKARDCLKINV.V1 04_165 05_91\n";
        let dir = fixture_dir("bram36");
        write_fixture(&dir, BRAM_TILEGRID, &[("segbits_bram.db", BRAM_CONFIG)]);
        // V1 set bits: frame 4 bit 165 (FAR 0x604), frame 5 bit 91
        // (FAR 0x605), both on the CLB_IO_CLK block.
        let frames = vec![frame(0x0000_0604, &[165]), frame(0x0000_0605, &[91])];
        let out = decode_frames(&frames, dir.to_str().unwrap()).unwrap();
        assert_eq!(
            out.fasm,
            vec!["RAMB36_X0Y0.RAMB36E2.CLKARDCLKINV.V1".to_string()]
        );
        assert!(out.unexplained.is_empty(), "unexplained: {:?}", out.unexplained);
    }

    /// Type 1 write header: type 1, opcode 2 (write), register, count.
    fn type1_write(addr: u8, count: u32) -> u32 {
        (1 << 29) | (2 << 27) | ((addr as u32) << 13) | count
    }

    #[test]
    fn decode_bitstream_parses_packets_then_decodes() {
        let dir = fixture_dir("bitstream");
        write_fixture(&dir, CLB_TILEGRID, &[("segbits_clel_l.db", CLB_SEGBITS)]);
        // Minimal stream in Worker A's shape: preamble (dummies,
        // bus-width patterns, sync, NOOP, RCRC, IDCODE for xczu3eg),
        // FAR write, WCFG, one FDRI frame, DESYNC, trailing NOOPs.
        // FDRI requires a prior FAR and IDCODE write in the same
        // section; the FAR (col 15, minor 12) is addressable by the
        // xczu3eg embedded geometry (col 15 has 16 minors).
        let mut words: Vec<u32> = vec![DUMMY_WORD; 8];
        words.push(BUS_WIDTH_DETECT);
        words.push(BUS_WIDTH_SYNC);
        words.extend([DUMMY_WORD; 2]);
        words.push(SYNC_WORD);
        words.push(NOOP_WORD);
        words.push(type1_write(reg::CMD, 1));
        words.push(cmd::RCRC);
        words.push(type1_write(reg::IDCODE, 1));
        words.push(XCZU3EG_IDCODE);
        words.push(type1_write(reg::FAR, 1));
        words.push(0x0000_0F0C);
        words.push(type1_write(reg::CMD, 1));
        words.push(cmd::WCFG);
        words.push(type1_write(reg::FDRI, FRAME_WORDS as u32));
        // The FDRI frame: AFF.INIT.V0 sets frame bit 2 (word 0 bit
        // 2). Built through `frame()` so it carries the computed H-1
        // band, as a valid bitstream frame must since the N7 port.
        words.extend(frame(0x0000_0F0C, &[2]).words);
        words.push(type1_write(reg::CMD, 1));
        words.push(cmd::DESYNC);
        words.extend([NOOP_WORD; 400]);
        let mut bytes = Vec::new();
        for w in words {
            bytes.extend_from_slice(&w.to_be_bytes());
        }
        let out = decode_bitstream(&bytes, dir.to_str().unwrap()).unwrap();
        assert_eq!(
            out.fasm,
            vec![
                "SLICE_X0Y0.ABCDFF.CEUSED.V0".to_string(),
                "SLICE_X0Y0.AFF.INIT.V0".to_string(),
                "SLICE_X0Y0.AFF.SRVAL.V1".to_string(),
            ]
        );
        assert!(out.unexplained.is_empty(), "unexplained: {:?}", out.unexplained);
        assert!(decode_bitstream(&[0u8; 16], dir.to_str().unwrap()).is_err());
    }

    #[test]
    fn missing_db_root_fails_loud() {
        let frames = vec![frame(0x0000_0F0C, &[2])];
        assert!(decode_frames(&frames, "/nonexistent/db_root_xyz").is_err());
    }
}
