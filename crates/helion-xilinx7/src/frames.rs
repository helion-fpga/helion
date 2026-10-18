//! FASM to configuration-frame assembly.
//!
//! Resolves parsed FASM features to frame bits through the X-Ray segbits
//! database. For each `(tile, feature)` the tile's type selects a
//! `segbits_<type>.db` table whose entries name frame-relative bits as
//! `[!]frame_offset_bitpos`, and the tile's `bits` record from
//! `tilegrid.json` gives the absolute frame base address plus the in-frame
//! word offset:
//!
//! ```text
//! frame_addr = tile_baseaddr + frame_offset
//! word       = tile_offset + bitpos / 32
//! bit        = bitpos % 32
//! ```
//!
//! Feature application mirrors prjxray `fasm2frames` exactly:
//!
//! * Only *enabled* bits take effect. A `= 0` value is a no-op, and within
//!   a bit vector only the 1 bits are applied (the `fasm` library's
//!   `canonical_features` drops the rest).
//! * A set feature sets plain bits and clears `!` bits.
//! * A bit selection `[i]` with `i == 0` resolves like the bare feature:
//!   the unbracketed segbits entry wins when present, otherwise the
//!   `[00]`-style indexed entry (the index is compared numerically, so the
//!   database's zero padding is irrelevant).
//! * Features listed in `ppips_<type>.db` (pseudo-pips: `always`/`default`)
//!   are no-ops, as in `fasm2frames`.
//! * Unknown tiles or features are hard errors (fail loud).
//!
//! [`FrameStore::fdri_payload`] lays the frames out for the FDRI write with
//! the exact zero-frame padding prjxray's `xc7frames2bit` emits: two zero
//! frames after every bus/row/half boundary, plus two trailing zero frames
//! (the FAR autoincrementer skips those addresses but the clocks must still
//! be fed). For xc7a35t that is 5408 + 12 frames = 547,420 words.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;

use crate::fasm::{FasmLine, FasmValue};

/// Words per configuration frame on 7-series.
pub const WORDS_PER_FRAME: usize = 101;
/// Frame count of xc7a35t (from `part.json`).
pub const XC7A35T_FRAME_COUNT: usize = 5408;
/// Zero pad frames inserted at each bus/row/half boundary and at the end.
pub const ZERO_PAD_FRAMES: usize = 2;

/// Configuration bus a tile's bits live on (FAR bits 25:23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockType {
    /// Bus 0: CLB/IO/CLK frames.
    ClbIoClk,
    /// Bus 1: block-RAM frames.
    BlockRam,
}

/// Absolute location of a tile's configuration bits within the frame array.
///
/// Mirrors one `tilegrid.json` `bits` entry (`baseaddr`/`offset`; the
/// `frames`/`words` extents are implied by the frame store and segbits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameAddr {
    /// Absolute frame address (FAR value) of the tile's first frame.
    pub baseaddr: u32,
    /// First word within each frame owned by this tile.
    pub offset: u32,
}

/// Lookup of per-tile frame geometry.
///
/// Implemented by the device-database loader (which resolves tile-type
/// aliases and `bits` alias records from `tilegrid.json`); tests use
/// [`StubTileLayout`].
pub trait TileBitLayout {
    /// Frame geometry for `tile` on `block`, or `None` if unknown.
    fn frame_addr(&self, tile: &str, block: BlockType) -> Option<FrameAddr>;
    /// Tile type selecting the segbits table (after alias resolution), or
    /// `None` if the tile is unknown.
    fn tile_type(&self, tile: &str) -> Option<&str>;
}

/// Derive a tile type from a tile name by stripping the `_X<num>Y<num>`
/// grid suffix, e.g. `INT_L_X10Y146` -> `INT_L`. Mirrors prjxray's
/// `_TILE_XY_RE`. Returns the full name when there is no such suffix.
pub fn tile_type_of_name(tile: &str) -> &str {
    let bytes = tile.as_bytes();
    // Walk back over trailing digits, then 'Y', then digits, then 'X'.
    let mut i = bytes.len();
    while i > 0 && bytes[i - 1].is_ascii_digit() {
        i -= 1;
    }
    if i == 0 || i == bytes.len() || bytes[i - 1] != b'Y' {
        return tile;
    }
    let mut j = i - 1;
    while j > 0 && bytes[j - 1].is_ascii_digit() {
        j -= 1;
    }
    if j == 0 || j == i - 1 || bytes[j - 1] != b'X' {
        return tile;
    }
    let k = j - 1;
    if k == 0 || bytes[k - 1] != b'_' {
        return tile;
    }
    &tile[..k - 1]
}

/// One frame bit from a segbits entry: `[!]frame_offset_bitpos`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegBit {
    /// Frame offset relative to the tile's base address.
    pub frame_offset: u32,
    /// Bit position relative to the tile's word offset.
    pub bitpos: u32,
    /// `!`-prefixed: cleared when the feature is set.
    pub invert: bool,
}

/// Parsed segbits (plus ppips) databases, keyed by tile type.
///
/// Load with [`SegBitsDb::add_segbits`] once per `(tile type, bus)` from the
/// corresponding `segbits_<type>.db` text, and [`SegBitsDb::add_ppips`]
/// from `ppips_<type>.db`. Feature keys are stored in canonical form
/// (`FOO[00]` and `FOO[0]` unify), so lookups are padding-insensitive.
#[derive(Debug, Default, Clone)]
pub struct SegBitsDb {
    tables: HashMap<String, TileSegbits>,
}

#[derive(Debug, Default, Clone)]
struct TileSegbits {
    /// (bus) -> (canonical full feature key) -> bits.
    tables: HashMap<BlockType, HashMap<String, Vec<SegBit>>>,
    /// (base feature key) -> (bit index) -> (bus, canonical full key).
    addresses: HashMap<String, HashMap<u32, (BlockType, String)>>,
    /// Pseudo-pip features (no-ops).
    ppips: HashMap<String, ()>,
}

impl SegBitsDb {
    /// Empty database.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse one `segbits_<type>.db` text for `tile_type` on `block` and add it.
    pub fn add_segbits(
        &mut self,
        tile_type: &str,
        block: BlockType,
        text: &str,
    ) -> Result<(), FramesError> {
        let entry = self.tables.entry(tile_type.to_string()).or_default();
        // Insert buses in CLB_IO_CLK, BLOCK_RAM order so the indexed-address
        // map matches prjxray's build order on collisions.
        let table = entry.tables.entry(block).or_default();
        for (lineno, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            let feature = parts.next().ok_or_else(|| FramesError {
                message: format!("segbits {tile_type}: empty line {}", lineno + 1),
            })?;
            let key = canonical_feature(feature);
            let mut bits = Vec::new();
            for token in parts {
                bits.push(parse_segbit(token).ok_or_else(|| FramesError {
                    message: format!(
                        "segbits {tile_type}: bad bit token {token:?} on line {}",
                        lineno + 1
                    ),
                })?);
            }
            if bits.is_empty() {
                return Err(FramesError {
                    message: format!(
                        "segbits {tile_type}: feature {feature:?} has no bits (line {})",
                        lineno + 1
                    ),
                });
            }
            if let Some((base, index)) = split_index(&key) {
                entry
                    .addresses
                    .entry(base.to_string())
                    .or_default()
                    .insert(index, (block, key.clone()));
            }
            table.insert(key, bits);
        }
        Ok(())
    }

    /// Parse one `ppips_<type>.db` text (`<feature> <always|default>`) for
    /// `tile_type`. Listed features resolve as no-ops during apply, exactly
    /// like prjxray `fasm2frames`.
    pub fn add_ppips(&mut self, tile_type: &str, text: &str) -> Result<(), FramesError> {
        let entry = self.tables.entry(tile_type.to_string()).or_default();
        for (lineno, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let feature = line.split_whitespace().next().ok_or_else(|| FramesError {
                message: format!("ppips {tile_type}: empty line {}", lineno + 1),
            })?;
            entry.ppips.insert(canonical_feature(feature), ());
        }
        Ok(())
    }

    /// True when `feature` (already prefixed with the tile type, e.g.
    /// `INT_L.BYP_ALT0.VCC_WIRE`) is a pseudo-pip and therefore a no-op.
    pub fn is_pseudo_pip(&self, tile_type: &str, feature: &str) -> bool {
        self.tables
            .get(tile_type)
            .is_some_and(|t| t.ppips.contains_key(feature))
    }

    /// Resolve `(tile_type, feature, address)` to its bus and bits, mirroring
    /// prjxray's `TileSegbits.feature_to_bits`: address 0 first tries the
    /// unbracketed feature, then falls back to the indexed entry.
    /// `feature` is the bare feature without brackets or tile-type prefix.
    pub fn feature_bits(
        &self,
        tile_type: &str,
        feature: &str,
        address: Option<u32>,
    ) -> Option<(BlockType, &[SegBit])> {
        let t = self.tables.get(tile_type)?;
        let base = format!("{tile_type}.{feature}");
        let addr = address.unwrap_or(0);
        if addr == 0 {
            for block in [BlockType::ClbIoClk, BlockType::BlockRam] {
                if let Some(bits) = t.tables.get(&block).and_then(|tab| tab.get(&base)) {
                    return Some((block, bits));
                }
            }
        }
        let (block, full) = t.addresses.get(&base)?.get(&addr)?;
        Some((*block, t.tables.get(block)?.get(full)?))
    }
}

/// Canonicalize a segbits feature key: strip leading zeros in a trailing
/// numeric `[..]` index so `FOO[00]` and `FOO[0]` unify.
fn canonical_feature(feature: &str) -> String {
    match split_index(feature) {
        Some((base, index)) => format!("{base}[{index}]"),
        None => feature.to_string(),
    }
}

/// Split a trailing numeric `[index]`; returns `(base, index)`.
fn split_index(feature: &str) -> Option<(&str, u32)> {
    let open = feature.rfind('[')?;
    if !feature.ends_with(']') {
        return None;
    }
    let inner = &feature[open + 1..feature.len() - 1];
    if inner.is_empty() || !inner.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let index: u32 = inner.trim_start_matches('0').parse().unwrap_or(0);
    Some((&feature[..open], index))
}

/// Parse one segbits bit token: `[!]frame_offset_bitpos`.
fn parse_segbit(token: &str) -> Option<SegBit> {
    let (invert, rest) = match token.strip_prefix('!') {
        Some(r) => (true, r),
        None => (false, token),
    };
    let (frame_s, bit_s) = rest.split_once('_')?;
    let frame_offset: u32 = frame_s.parse().ok()?;
    let bitpos: u32 = bit_s.parse().ok()?;
    Some(SegBit {
        frame_offset,
        bitpos,
        invert,
    })
}

/// A frame-assembly failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramesError {
    /// Human-readable description.
    pub message: String,
}

impl fmt::Display for FramesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "frames error: {}", self.message)
    }
}

impl Error for FramesError {}

/// All configuration frames of the device, keyed by absolute frame address.
///
/// Frames are zero-filled on creation; [`FrameStore::apply_fasm`] sets and
/// clears bits per the FASM input.
#[derive(Debug, Clone)]
pub struct FrameStore {
    /// Frame addresses in ascending numeric order.
    addrs: Vec<u32>,
    /// `addrs.len() * WORDS_PER_FRAME` words, frame-major.
    words: Vec<u32>,
    /// Address -> frame index.
    index: HashMap<u32, usize>,
}

impl FrameStore {
    /// Zero-filled store over `addrs` (sorted and deduplicated).
    pub fn new(addrs: &[u32]) -> Self {
        let mut sorted: Vec<u32> = addrs.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let index: HashMap<u32, usize> =
            sorted.iter().enumerate().map(|(i, &a)| (a, i)).collect();
        let words = vec![0u32; sorted.len() * WORDS_PER_FRAME];
        FrameStore {
            addrs: sorted,
            words,
            index,
        }
    }

    /// Number of frames in the store.
    pub fn frame_count(&self) -> usize {
        self.addrs.len()
    }

    /// Frame addresses in ascending order.
    pub fn frame_addresses(&self) -> &[u32] {
        &self.addrs
    }

    /// Raw words of one frame, or `None` for an unknown address.
    pub fn frame_words(&self, addr: u32) -> Option<&[u32]> {
        let i = *self.index.get(&addr)?;
        Some(&self.words[i * WORDS_PER_FRAME..(i + 1) * WORDS_PER_FRAME])
    }

    /// Apply parsed FASM lines to the frames.
    ///
    /// Returns the number of features that resolved to segbits (pseudo-pips
    /// and zero values are skipped silently, like `fasm2frames`).
    pub fn apply_fasm(
        &mut self,
        lines: &[FasmLine],
        segbits: &SegBitsDb,
        layout: &dyn TileBitLayout,
    ) -> Result<usize, FramesError> {
        let mut applied = 0usize;
        for line in lines {
            let tile_type = layout.tile_type(&line.tile).ok_or_else(|| FramesError {
                message: format!("unknown tile {:?}", line.tile),
            })?;
            for (feature, address) in canonical_assigns(line) {
                let namespaced = format!("{tile_type}.{feature}");
                if segbits.is_pseudo_pip(tile_type, &namespaced) {
                    continue;
                }
                let (block, bits) =
                    segbits.feature_bits(tile_type, feature, address).ok_or_else(|| {
                        FramesError {
                            message: format!(
                                "tile {:?} (type {tile_type}): no segbits for feature {namespaced:?}",
                                line.tile,
                            ),
                        }
                    })?;
                let geom = layout.frame_addr(&line.tile, block).ok_or_else(|| FramesError {
                    message: format!(
                        "tile {:?}: no frame geometry on bus {block:?}",
                        line.tile
                    ),
                })?;
                self.apply_bits(&namespaced, geom, bits)?;
                applied += 1;
            }
        }
        Ok(applied)
    }

    fn apply_bits(
        &mut self,
        feature: &str,
        geom: FrameAddr,
        bits: &[SegBit],
    ) -> Result<(), FramesError> {
        for b in bits {
            let frame = geom.baseaddr.checked_add(b.frame_offset).ok_or_else(|| {
                FramesError {
                    message: format!("feature {feature:?}: frame address overflow"),
                }
            })?;
            let word = geom
                .offset
                .checked_add(b.bitpos / 32)
                .ok_or_else(|| FramesError {
                    message: format!("feature {feature:?}: word offset overflow"),
                })?;
            if word as usize >= WORDS_PER_FRAME {
                return Err(FramesError {
                    message: format!(
                        "feature {feature:?}: word {word} outside 0..{WORDS_PER_FRAME}"
                    ),
                });
            }
            let frame_idx = *self.index.get(&frame).ok_or_else(|| FramesError {
                message: format!("feature {feature:?}: frame {frame:#010x} not in store"),
            })?;
            let slot = &mut self.words[frame_idx * WORDS_PER_FRAME + word as usize];
            let bit = b.bitpos % 32;
            // A set feature sets plain bits and clears `!` bits.
            if b.invert {
                *slot &= !(1u32 << bit);
            } else {
                *slot |= 1u32 << bit;
            }
        }
        Ok(())
    }

    /// FDRI payload: frames in ascending address order, with two all-zero
    /// frames inserted after every bus/row/half boundary and two trailing
    /// zero frames at the end, exactly like prjxray `xc7frames2bit`
    /// (`createType2ConfigurationPacketData`).
    ///
    /// Requires the store to hold the device's complete frame set in
    /// numeric order (which [`FrameStore::new`] guarantees).
    pub fn fdri_payload(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(
            (self.addrs.len() + 2 * (ZERO_PAD_FRAMES + 1)) * WORDS_PER_FRAME,
        );
        for (n, &addr) in self.addrs.iter().enumerate() {
            let base = n * WORDS_PER_FRAME;
            out.extend_from_slice(&self.words[base..base + WORDS_PER_FRAME]);
            let boundary = match self.addrs.get(n + 1) {
                Some(&next) => far_group(next) != far_group(addr),
                // Unconditional trailing pad after the last frame.
                None => true,
            };
            if boundary {
                out.extend_from_slice(&[0u32; ZERO_PAD_FRAMES * WORDS_PER_FRAME]);
            }
        }
        out
    }
}

/// (bus, half, row) group of a frame address: FAR bits 25:23, 22, 21:17.
/// Bit 22 set means the *bottom* half (prjxray `is_bottom_half_rows`).
fn far_group(addr: u32) -> (u32, u32, u32) {
    ((addr >> 23) & 0x7, (addr >> 22) & 0x1, (addr >> 17) & 0x1f)
}

/// Expand a parsed line into `(bare feature, address)` assignments for the
/// *enabled* bits only, mirroring the `fasm` library's `canonical_features`:
/// a zero value (or zero bits of a vector) is a no-op, and address 0 is
/// reported as `None` (bare-feature lookup).
fn canonical_assigns(line: &FasmLine) -> Vec<(&str, Option<u32>)> {
    if line.value.is_zero() {
        return Vec::new();
    }
    // Without a bit selection the fasm library asserts value == 1.
    if line.bit_range.is_none() && !matches!(line.value, FasmValue::One) {
        return Vec::new();
    }
    let mut out = Vec::new();
    match line.bit_range {
        None => out.push((line.feature.as_str(), None)),
        Some((hi, lo)) => {
            for addr in lo..=hi {
                if line.value.bit(addr - lo) {
                    out.push((line.feature.as_str(), (addr != 0).then_some(addr)));
                }
            }
        }
    }
    out
}

/// Minimal [`TileBitLayout`] fed by an explicit tiny tilegrid, for tests and
/// for unblocking the bitstream writer before the real DB loader lands.
#[derive(Debug, Default, Clone)]
pub struct StubTileLayout {
    tiles: HashMap<String, StubTile>,
}

#[derive(Debug, Clone)]
struct StubTile {
    tile_type: String,
    addrs: HashMap<BlockType, FrameAddr>,
}

impl StubTileLayout {
    /// Empty layout; add tiles with [`StubTileLayout::add_tile`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `tile` of type `tile_type` with frame geometry on `block`.
    pub fn add_tile(
        &mut self,
        tile: &str,
        tile_type: &str,
        block: BlockType,
        baseaddr: u32,
        offset: u32,
    ) {
        self.tiles
            .entry(tile.to_string())
            .or_insert_with(|| StubTile {
                tile_type: tile_type.to_string(),
                addrs: HashMap::new(),
            })
            .addrs
            .insert(block, FrameAddr { baseaddr, offset });
    }
}

impl TileBitLayout for StubTileLayout {
    fn frame_addr(&self, tile: &str, block: BlockType) -> Option<FrameAddr> {
        self.tiles.get(tile)?.addrs.get(&block).copied()
    }

    fn tile_type(&self, tile: &str) -> Option<&str> {
        self.tiles.get(tile).map(|t| t.tile_type.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fasm::parse_fasm;

    const INT_SEGBITS: &str = "\
INT_L.BYP_ALT0.BYP_BOUNCE_N3_3 21_07 !22_07 !23_07 24_07 !25_07
INT_L.IMUX_L1.N1 33_14
INT_L.IMUX_L1.N2 33_15 !33_16
";

    const CLB_SEGBITS: &str = "\
CLBLL_L.SLICEL_X0.ALUT.INIT[00] 32_15
CLBLL_L.SLICEL_X0.ALUT.INIT[01] 33_15
CLBLL_L.SLICEL_X0.ALUT.INIT[02] 32_14
CLBLL_L.SLICEL_X0.AFF.ZINI 10_03 !10_04
";

    fn test_db() -> SegBitsDb {
        let mut db = SegBitsDb::new();
        db.add_segbits("INT_L", BlockType::ClbIoClk, INT_SEGBITS).unwrap();
        db.add_segbits("CLBLL_L", BlockType::ClbIoClk, CLB_SEGBITS).unwrap();
        db
    }

    fn test_layout() -> StubTileLayout {
        let mut layout = StubTileLayout::new();
        layout.add_tile("INT_L_X10Y146", "INT_L", BlockType::ClbIoClk, 0x0000_0000, 0);
        layout.add_tile("CLBLL_L_X12Y34", "CLBLL_L", BlockType::ClbIoClk, 0x0000_1000, 0);
        layout
    }

    #[test]
    fn segbits_set_and_invert_bits() {
        let db = test_db();
        let layout = test_layout();
        let lines = parse_fasm("INT_L_X10Y146.BYP_ALT0.BYP_BOUNCE_N3_3\n").unwrap();
        // Tile base 0x0, offsets 21..25 -> frames 0x15..0x19.
        let mut store = FrameStore::new(&[0x15, 0x16, 0x17, 0x18, 0x19]);
        // Pre-set bit 7 of word 0 in frame 0x16; !22_07 must clear it.
        store.words[WORDS_PER_FRAME] |= 1 << 7;
        store.apply_fasm(&lines, &db, &layout).unwrap();
        // 21_07 sets bit 7 of word 0 in frame 0x15; 24_07 sets bit 7 of
        // word 0 in frame 0x18.
        assert_eq!(store.frame_words(0x15).unwrap()[0], 1 << 7);
        assert_eq!(store.frame_words(0x18).unwrap()[0], 1 << 7);
        // !22_07, !23_07, !25_07 cleared their bits.
        assert_eq!(store.frame_words(0x16).unwrap()[0], 0);
        assert_eq!(store.frame_words(0x17).unwrap()[0], 0);
        assert_eq!(store.frame_words(0x19).unwrap()[0], 0);
    }

    #[test]
    fn zero_value_is_noop_like_fasm2frames() {
        let db = test_db();
        let mut store = FrameStore::new(&[0x0000_0000, 0x0000_0001]);
        let lines = parse_fasm("INT_L_X10Y146.IMUX_L1.N1 = 0\n").unwrap();
        let n = store.apply_fasm(&lines, &db, &test_layout()).unwrap();
        assert_eq!(n, 0);
        assert!(store.words.iter().all(|&w| w == 0));
    }

    #[test]
    fn bit_vector_applies_only_one_bits() {
        let db = test_db();
        let mut store = FrameStore::new(&[0x0000_1020, 0x0000_1021]);
        // INIT[02:00] = 3'b101 -> bits 2 and 0 set, bit 1 skipped.
        let lines = parse_fasm("CLBLL_L_X12Y34.SLICEL_X0.ALUT.INIT[02:00] = 3'b101\n").unwrap();
        store.apply_fasm(&lines, &db, &test_layout()).unwrap();
        // Tile base 0x1000: INIT[00]=32_15 -> frame 0x1020 word 0 bit 15;
        // INIT[02]=32_14 -> frame 0x1020 word 0 bit 14. INIT[01] is skipped
        // (its value bit is 0).
        let w0 = store.frame_words(0x0000_1020).unwrap();
        assert_eq!(w0[0], (1 << 15) | (1 << 14));
        let w1 = store.frame_words(0x0000_1021).unwrap();
        assert_eq!(w1[0], 0);
    }

    #[test]
    fn zero_padded_index_unifies() {
        // The DB uses [00]; FASM may say [0].
        let db = test_db();
        let mut store = FrameStore::new(&[0x0000_1020]);
        let lines = parse_fasm("CLBLL_L_X12Y34.SLICEL_X0.ALUT.INIT[0]\n").unwrap();
        store.apply_fasm(&lines, &db, &test_layout()).unwrap();
        let w = store.frame_words(0x0000_1020).unwrap();
        assert_eq!(w[0], 1 << 15);
    }

    #[test]
    fn address_zero_uses_bare_feature_when_present() {
        // AFF.ZINI exists unbracketed; [0] must resolve to it, not ZINI[00].
        let db = test_db();
        let mut store = FrameStore::new(&[0x0000_100A]);
        let lines = parse_fasm("CLBLL_L_X12Y34.SLICEL_X0.AFF.ZINI[0]\n").unwrap();
        store.apply_fasm(&lines, &db, &test_layout()).unwrap();
        // 10_03 -> frame 0x100A word 0 bit 3 set; !10_04 clears bit 4.
        let w = store.frame_words(0x0000_100A).unwrap();
        assert_eq!(w[0], (1 << 3));
    }

    #[test]
    fn pseudo_pips_are_noops() {
        let mut db = test_db();
        db.add_ppips("INT_L", "INT_L.BYP_ALT0.VCC_WIRE default\n").unwrap();
        let mut store = FrameStore::new(&[0x0000_0000]);
        let lines = parse_fasm("INT_L_X10Y146.BYP_ALT0.VCC_WIRE\n").unwrap();
        let n = store.apply_fasm(&lines, &db, &test_layout()).unwrap();
        assert_eq!(n, 0);
        assert!(store.words.iter().all(|&w| w == 0));
    }

    #[test]
    fn unknown_tile_and_feature_fail_loud() {
        let db = test_db();
        let mut store = FrameStore::new(&[0]);
        let lines = parse_fasm("NOPE_X0Y0.F\n").unwrap();
        assert!(store.apply_fasm(&lines, &db, &test_layout()).is_err());
        let lines = parse_fasm("INT_L_X10Y146.DOES_NOT_EXIST\n").unwrap();
        assert!(store.apply_fasm(&lines, &db, &test_layout()).is_err());
    }

    #[test]
    fn tile_type_of_name_strips_grid_suffix() {
        assert_eq!(tile_type_of_name("INT_L_X10Y146"), "INT_L");
        assert_eq!(tile_type_of_name("CLBLL_L_X12Y34"), "CLBLL_L");
        assert_eq!(tile_type_of_name("RIOB33_X43Y81"), "RIOB33");
        assert_eq!(tile_type_of_name("INT_L"), "INT_L");
        assert_eq!(tile_type_of_name("FOO_X1"), "FOO_X1");
    }

    #[test]
    fn fdri_payload_inserts_boundary_and_trailing_zeros() {
        // Two frames in the same group, then one in another bus: zeros
        // after the boundary and two trailing zeros at the end.
        let store = FrameStore::new(&[0x0000_0000, 0x0000_0001, 0x0080_0000]);
        let payload = store.fdri_payload();
        // 3 frames + 2 (boundary) + 2 (trailing) = 7 frames.
        assert_eq!(payload.len(), 7 * WORDS_PER_FRAME);
        assert!(payload[3 * WORDS_PER_FRAME..5 * WORDS_PER_FRAME]
            .iter()
            .all(|&w| w == 0));
        assert!(payload[6 * WORDS_PER_FRAME..7 * WORDS_PER_FRAME]
            .iter()
            .all(|&w| w == 0));
        // No boundary inside a group: contiguous groups get no padding.
        let store = FrameStore::new(&[0x0000_0000, 0x0000_0001]);
        let payload = store.fdri_payload();
        assert_eq!(payload.len(), 4 * WORDS_PER_FRAME);
    }

    #[test]
    fn far_group_bit22_is_bottom() {
        // prjxray: bit 22 set == bottom half rows.
        assert_eq!(far_group(0x0040_0000), (0, 1, 0));
        assert_eq!(far_group(0x0000_0000), (0, 0, 0));
        assert_eq!(far_group(0x0080_0000), (1, 0, 0));
        assert_eq!(far_group(0x00C2_0000), (1, 1, 1));
    }
}
