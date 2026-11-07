//! UltraScale+ bitstream emitter and parser (bitgen-core).
//!
//! FASM in, bitstream out, and parse-back to identical FASM. This module
//! owns the emitter only: FASM parsing, segbits resolution, frame assembly,
//! and bitstream framing. It does not own per-BEL emission logic (lut-ff-bram
//! worker), INT PIP emission logic (int-pip worker), or end-to-end CLI wiring
//! (integration worker).
//!
//! Sources, in order of authority:
//! - UG570 (UltraScale Architecture Configuration User Guide): FAR layout
//!   table 9-21, packet formats tables 9-16/9-18, register map table 9-19,
//!   command codes table 9-22, IDCODE table 2, frame size 93 words for
//!   UltraScale+.
//! - BitFiltrator (EPFL, FPL 2022, MIT): `bitfiltrator-ref/src/` in the base
//!   worktree. FAR field widths (`arch_spec.py`, `UltraScalePlusSpec`),
//!   frame bit order (`frame.py`, `ConfigFrame.bit`: bit N is word N/32,
//!   bit N%32, LSB-first within the word), per-frame CRC packet sequence
//!   (`bitstream.py` comment: WCFG, FAR, FDRI, FAR rewrite, CRC), sync word
//!   and dummy words (`bitstream_spec.py`), 2 end-of-row padding frames.
//! - prjxray `tools/bits2rbt/crc.h`: the configuration CRC is CRC-32C
//!   (Castagnoli, reflected polynomial 0x82F63B78), computed LSB-first over
//!   37 bits per (5-bit register address, 32-bit data word) pair, starting
//!   from 0 after RCRC. Validated here against a real Vivado bitstream: the
//!   BitFiltrator per-frame CRC example (123-word all-zero frame, FAR 1)
//!   reproduces CRC 0x06b1285e exactly. A previously listed register
//!   exclusion list ([15, 18, 20, 21, 22], cited to an unverifiable
//!   `bitstream_crc.py` gist derived from Unisim SIM_CONFIGE2.vhd) was
//!   removed: no such source exists on disk, and bits2rbt
//!   `GetFpgaConfigurationCRC` feeds every non-NOP write packet into the
//!   CRC with no exclusions.
//! - Vivado 7-series bitstream `zedboard_blinky.bit` (in this workspace):
//!   observed preamble register order and ending sequence (RCRC, GRESTORE,
//!   DGHIGH_LFRM, START, DESYNC, trailing NOOPs). That file carries no
//!   CRC packets, so it does not validate CRC values.
//!
//! Contract notes (for the critic and the interfaces owner). The
//! INTERFACES.md shapes are followed except where they cannot work:
//! - `BitRef.bit` is `u16`, not `u8`: a frame holds 93*32 = 2976 bits, so a
//!   u8 cannot address a frame bit. A u8 would silently truncate.
//! - `BitRef.frame` is a frame offset relative to the tile base FAR, not an
//!   absolute FAR. The segbits lookup is keyed by (tile_type, feature) only,
//!   so it cannot distinguish two tiles of the same type; the tile base FAR
//!   (from the device file) plus the offset gives the absolute FAR. This is
//!   the prjxray segbits convention.
//! - `SegbitsDb` gains `features(tile_type)`: `parse_bitstream` must invert
//!   the mapping, which needs enumeration; `lookup` alone cannot enumerate.
//! - `emit_bitstream` takes `&dyn BitgenDevice`, not `&dyn UltraScaleDevice`:
//!   `UltraScaleDevice` is a struct, so `&dyn UltraScaleDevice` does not
//!   compile. The trait is the fix. The device-file worker implements it for
//!   the real device file; the logical grid in `device.rs` must NOT be used
//!   as ground truth for FAR mapping (CTO ruling, correctness critic base
//!   review G2).
//!
//! INIT expansion rule (from the brief): the value expands MSB-first across
//! the listed bits, bit 0 of the value is its LSB. Listed bit j takes value
//! bit (N-1-j), where N is the number of listed bits. The segbits-vendor
//! worker must order INIT BitRef lists MSB-first (list index 0 = most
//! significant value bit).
//!
//! Known limits, stated plainly:
//! - The preamble register writes Vivado emits between RCRC and the first
//!   FDRI (TIMER, WBSTAR, POST_CRC, COR0, COR1, SWITCH, MASK, CTL0, CTL1)
//!   are NOT emitted here: their UltraScale+ values are not in any open
//!   source, and guessed values are never emitted. The emitter writes the
//!   minimal documented preamble (NOOP, RCRC, IDCODE, WCFG). Consequence:
//!   the first frame CRC differs from Vivado's by exactly the preamble
//!   contribution. The required post-preamble CRC state for the BitFiltrator
//!   example frame is 0x01e2bcae; a future preamble candidate can be checked
//!   against that number before it is trusted.
//! - `parse_bitstream` returns the FASM of features whose configuration is
//!   observable in the frames. A valued feature whose value is all zero
//!   sets no bits and is indistinguishable from an absent feature, so it is
//!   dropped on parse-back. A valueless all-`!` feature is dropped too,
//!   UNLESS it is the exact complement of another feature of the same
//!   tile type (a binary attribute pair such as FF INIT .V0/.V1, where
//!   V1's zero set is V0's set and V0 lists no `!` bits): exactly one of
//!   the pair holds in every bit pattern, so the all-`!` half is
//!   observable and renders. A mux default such as VCC_WIRE has no
//!   complement and is dropped; otherwise every all-`!` feature of every
//!   tile would be emitted for unconfigured frames. Round-trip exactness
//!   holds for observable features; the tests pin this boundary.
//! - FF INIT parse-back renders the DB vocabulary `.V0`/`.V1`, never bare
//!   `AFF.INIT`. Polarity, resolved by researcher session 1 from
//!   f4pga/prjuray (tools/dump_features.tcl:897-902 names the feature
//!   from the cell INIT property, so the V suffix IS the attribute
//!   value; utils/spec/flipflops.py:100 instantiates .INIT(1'b0/1'b1);
//!   vendored DB lines CLEL_L.AFF.INIT.V0 12_02, V1 !12_02 confirm):
//!   INIT value 0 selects .V0, which SETS the bit; INIT value 1 selects
//!   .V1, which CLEARS the bit. bit = NOT(INIT). SRVAL is the same:
//!   bit = NOT(SRVAL). So a bit reading 1 renders .V0 (INIT 0) and a
//!   bit reading 0 renders .V1 (INIT 1). Consequence: parse-back renders
//!   exactly one .V0/.V1 line per FF in the device for DBs with the pair
//!   structure, so round-trip exactness on real device data requires
//!   the input FASM to specify every FF's INIT (complete FASM); a
//!   partial FASM gains .V1 lines for unmentioned FFs.
//! - Frames with set bits no enumerated feature explains fail loud in
//!   `parse_bitstream` (`UnexplainedBits`). Use `parse_frames` for foreign
//!   bitstreams whose DB coverage is incomplete.

use crate::segbits::{BitRef, SegbitsDb};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

// ---------------------------------------------------------------------------
// Constants (UG570)
// ---------------------------------------------------------------------------

/// Sync word (UG570, BitFiltrator `bitstream_spec.py`).
pub const SYNC_WORD: u32 = 0xAA99_5566;
/// Dummy word.
pub const DUMMY_WORD: u32 = 0xFFFF_FFFF;
/// Bus width detect pattern (8-bit bus).
pub const BUS_WIDTH_DETECT: u32 = 0x0000_00BB;
/// Bus width sync pattern.
pub const BUS_WIDTH_SYNC: u32 = 0x1122_0044;
/// NOOP packet: Type 1, opcode 0, register 0, word count 0.
pub const NOOP_WORD: u32 = 0x2000_0000;

/// Words per UltraScale+ configuration frame (UG570 table 1).
pub const FRAME_WORDS: usize = 93;
/// Bits per UltraScale+ configuration frame.
pub const FRAME_BITS: usize = FRAME_WORDS * 32;

/// Configuration register addresses (UG570 table 9-19).
pub mod reg {
    pub const CRC: u8 = 0;
    pub const FAR: u8 = 1;
    pub const FDRI: u8 = 2;
    pub const FDRO: u8 = 3;
    pub const CMD: u8 = 4;
    pub const CTL0: u8 = 5;
    pub const MASK: u8 = 6;
    pub const STAT: u8 = 7;
    pub const LOUT: u8 = 8;
    pub const COR0: u8 = 9;
    pub const MFWR: u8 = 10;
    pub const CBC: u8 = 11;
    pub const IDCODE: u8 = 12;
    pub const AXSS: u8 = 13;
    pub const COR1: u8 = 14;
    pub const WBSTAR: u8 = 16;
    pub const TIMER: u8 = 17;
    pub const BOOTSTS: u8 = 22;
    pub const CTL1: u8 = 24;
    pub const BSPI: u8 = 31;
}

/// Command codes (UG570 table 9-22).
pub mod cmd {
    pub const NULL: u32 = 0;
    pub const WCFG: u32 = 1;
    pub const MFW: u32 = 2;
    pub const DGHIGH_LFRM: u32 = 3;
    pub const RCFG: u32 = 4;
    pub const START: u32 = 5;
    pub const URAM: u32 = 6;
    pub const RCRC: u32 = 7;
    pub const AGHIGH: u32 = 8;
    pub const SWITCH: u32 = 9;
    pub const GRESTORE: u32 = 10;
    pub const SHUTDOWN: u32 = 11;
    pub const DESYNC: u32 = 13;
    pub const IPROG: u32 = 15;
    pub const CRCC: u32 = 16;
    pub const LTIMER: u32 = 17;
    pub const BSPI_READ: u32 = 18;
    pub const FALL_EDGE: u32 = 19;
}

/// FAR block types (UG570 table 9-21, BitFiltrator `frame_spec.py`).
pub mod block {
    pub const CLB_IO_CLK: u8 = 0;
    pub const BRAM_CONTENT: u8 = 1;
}

/// Trailing NOOP count after DESYNC, matching the observed Vivado
/// 7-series bitstream in this workspace (`zedboard_blinky.bit`: 400).
const TRAILING_NOOPS: usize = 400;

// ---------------------------------------------------------------------------
// FAR (UG570 table 9-21)
// ---------------------------------------------------------------------------

/// Encode a Frame Address Register value.
///
/// Layout, MSB to LSB: reserved [31:27], block type [26:24], row [23:18],
/// column [17:8], minor [7:0]. Reserved bits are always 0.
pub fn far_encode(block_type: u8, row: u8, col: u16, minor: u8) -> Result<u32, BitgenError> {
    if block_type > 7 {
        return Err(BitgenError::BadFar {
            reason: format!("block type {block_type} exceeds 3 bits"),
        });
    }
    if row > 63 {
        return Err(BitgenError::BadFar {
            reason: format!("row {row} exceeds 6 bits"),
        });
    }
    if col > 1023 {
        return Err(BitgenError::BadFar {
            reason: format!("column {col} exceeds 10 bits"),
        });
    }
    Ok(((block_type as u32) << 24) | ((row as u32) << 18) | ((col as u32) << 8) | minor as u32)
}

/// Decode a FAR value into (block type, row, column, minor).
pub fn far_decode(far: u32) -> (u8, u8, u16, u8) {
    (
        ((far >> 24) & 0x7) as u8,
        ((far >> 18) & 0x3F) as u8,
        ((far >> 8) & 0x3FF) as u16,
        (far & 0xFF) as u8,
    )
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Every failure names what was being built and why. Unknown features and
/// unknown encodings fail loud; nothing here ever emits a guessed bit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BitgenError {
    /// FASM line is malformed.
    BadFasm { line: usize, reason: String },
    /// FASM value cannot be parsed.
    BadValue {
        tile: String,
        feature: String,
        value: String,
    },
    /// Value has set bits above the feature bit list.
    ValueTooWide {
        tile: String,
        feature: String,
        bits: usize,
        value_bits: usize,
    },
    /// Tile name is unknown to the device file.
    UnknownTile { tile: String },
    /// Tile has no tile type in the device file.
    UnknownTileType { tile: String },
    /// Feature has no segbits entry for the tile type.
    UnknownFeature { tile: String, feature: String },
    /// Duplicate (tile, feature) line in the FASM input.
    DuplicateFeature { tile: String, feature: String },
    /// A `!` bit reads 1 when the feature requires 0.
    MustBeZeroViolated {
        tile: String,
        feature: String,
        far: u32,
        bit: u16,
    },
    /// Two features write different values to the same frame bit.
    ConflictingBit {
        tile: String,
        feature: String,
        far: u32,
        bit: u16,
    },
    /// Device IDCODE is unknown (no device file yet).
    UnknownIdcode { part: String },
    /// Bitstream IDCODE does not match the device.
    IdcodeMismatch { expected: u32, found: u32 },
    /// A touched frame is not in the device config order.
    FrameNotInConfigOrder { far: u32 },
    /// Config order lists the same FAR twice.
    DuplicateFarInConfigOrder { far: u32 },
    /// FAR field out of range.
    BadFar { reason: String },
    /// No sync word in the bitstream.
    MissingSync,
    /// Bitstream ends mid-packet.
    Truncated { at_byte: usize, reason: String },
    /// Packet header is not Type 1 or Type 2.
    BadPacket { at_byte: usize, header: u32 },
    /// FDRI payload is not a whole number of frames.
    PartialFrame { far: u32, words: usize },
    /// A CRC-register write does not match the running CRC.
    CrcMismatch { far: u32, expected: u32, found: u32 },
    /// A frame bit is set that no enumerated feature explains.
    UnexplainedBits { far: u32, bits: Vec<u16> },
    /// A feature reads back inconsistently from the frames.
    InconsistentFeature {
        tile: String,
        feature: String,
        reason: String,
    },
    /// Second distinct IDCODE in one bitstream (multi-SLR, out of scope).
    MultipleIdcodes { first: u32, second: u32 },
}

impl fmt::Display for BitgenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BitgenError::BadFasm { line, reason } => {
                write!(f, "FASM line {line}: {reason}")
            }
            BitgenError::BadValue {
                tile,
                feature,
                value,
            } => {
                write!(
                    f,
                    "tile {tile} feature {feature}: cannot parse value '{value}'"
                )
            }
            BitgenError::ValueTooWide {
                tile,
                feature,
                bits,
                value_bits,
            } => {
                write!(
                    f,
                    "tile {tile} feature {feature}: value has {value_bits} bits but the feature lists {bits}"
                )
            }
            BitgenError::UnknownTile { tile } => write!(f, "unknown tile {tile}"),
            BitgenError::UnknownTileType { tile } => {
                write!(f, "tile {tile} has no tile type in the device file")
            }
            BitgenError::UnknownFeature { tile, feature } => {
                write!(f, "tile {tile}: unknown feature {feature}")
            }
            BitgenError::DuplicateFeature { tile, feature } => {
                write!(f, "tile {tile}: duplicate feature {feature}")
            }
            BitgenError::MustBeZeroViolated {
                tile,
                feature,
                far,
                bit,
            } => {
                write!(
                    f,
                    "tile {tile} feature {feature}: bit {bit} of FAR 0x{far:08x} must read 0 but reads 1"
                )
            }
            BitgenError::ConflictingBit {
                tile,
                feature,
                far,
                bit,
            } => {
                write!(
                    f,
                    "tile {tile} feature {feature}: frame bit {bit} of FAR 0x{far:08x} already written with a different value"
                )
            }
            BitgenError::UnknownIdcode { part } => {
                write!(f, "device {part} has no IDCODE in the device file")
            }
            BitgenError::IdcodeMismatch { expected, found } => {
                write!(
                    f,
                    "IDCODE mismatch: expected 0x{expected:08x}, found 0x{found:08x}"
                )
            }
            BitgenError::FrameNotInConfigOrder { far } => {
                write!(f, "frame FAR 0x{far:08x} is not in the device config order")
            }
            BitgenError::DuplicateFarInConfigOrder { far } => {
                write!(f, "config order lists FAR 0x{far:08x} twice")
            }
            BitgenError::BadFar { reason } => write!(f, "bad FAR: {reason}"),
            BitgenError::MissingSync => write!(f, "no sync word 0xAA995566 in bitstream"),
            BitgenError::Truncated { at_byte, reason } => {
                write!(f, "bitstream truncated at byte {at_byte}: {reason}")
            }
            BitgenError::BadPacket { at_byte, header } => {
                write!(f, "bad packet header 0x{header:08x} at byte {at_byte}")
            }
            BitgenError::PartialFrame { far, words } => {
                write!(
                    f,
                    "FDRI payload for FAR 0x{far:08x} is {words} words, not a whole number of {FRAME_WORDS}-word frames"
                )
            }
            BitgenError::CrcMismatch {
                far,
                expected,
                found,
            } => {
                write!(
                    f,
                    "CRC mismatch at FAR 0x{far:08x}: expected 0x{expected:08x}, found 0x{found:08x}"
                )
            }
            BitgenError::UnexplainedBits { far, bits } => {
                write!(
                    f,
                    "FAR 0x{far:08x} has set bits no feature explains: {bits:?}"
                )
            }
            BitgenError::InconsistentFeature {
                tile,
                feature,
                reason,
            } => {
                write!(
                    f,
                    "tile {tile} feature {feature} reads back inconsistently: {reason}"
                )
            }
            BitgenError::MultipleIdcodes { first, second } => {
                write!(
                    f,
                    "multiple IDCODEs in one bitstream (multi-SLR out of scope): 0x{first:08x} then 0x{second:08x}"
                )
            }
        }
    }
}

impl std::error::Error for BitgenError {}

// ---------------------------------------------------------------------------
// Device traits
// ---------------------------------------------------------------------------
//
// `BitRef` and `SegbitsDb` live in `crate::segbits` (the loader's home):
// the single definition, with u16 bit width and `features()`, per the
// interfaces session 2 item 6 reconciliation. This module used to define
// its own duplicates (`set`/`clear` constructors); they are deleted here.
// Constructor spelling picked: `set`/`zero`, the loader's spelling, because
// `zero` names the `!` token's meaning (the bit must read 0) and matches
// the `must_be_zero` field; `clear` is dropped and its call sites renamed.

/// Device data the emitter consumes. Implemented by the device-file worker
/// for the real device file; the logical grid in `device.rs` does NOT
/// implement this (CTO ruling: it is not ground truth for FAR mapping).
pub trait BitgenDevice {
    /// Part name, e.g. "xcku3p".
    fn part(&self) -> &str;
    /// JTAG IDCODE from UG570 table 2, or None if the device file has none.
    fn idcode(&self) -> Option<u32>;
    /// Tile type for a Vivado tile name, e.g. "SLICE_X0Y0" -> "CLEL_L".
    fn tile_type(&self, tile: &str) -> Option<&str>;
    /// Base FAR for a Vivado tile name. BitRef frame offsets add to this.
    fn tile_base_far(&self, tile: &str) -> Option<u32>;
    /// All tile names in the device file, sorted.
    fn tile_names(&self) -> &[String];
    /// FARs in config order (ascending FAR integer within the device file).
    /// Empty means the config order is unknown: emit only touched frames.
    fn config_order_fars(&self) -> &[u32];
}

// ---------------------------------------------------------------------------
// FASM parsing
// ---------------------------------------------------------------------------

/// A parsed FASM value: LSB-first bytes plus the significant bit count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FasmValue {
    bytes: Vec<u8>,
    nbits: usize,
}

impl FasmValue {
    /// Value bit i (bit 0 is the LSB of the value).
    pub fn bit(&self, i: usize) -> bool {
        if i >= self.nbits {
            return false;
        }
        let byte = self.bytes.get(i / 8).copied().unwrap_or(0);
        (byte >> (i % 8)) & 1 == 1
    }

    /// Highest set bit position plus one (0 when the value is zero).
    pub fn significant_bits(&self) -> usize {
        self.nbits
    }

    fn from_bytes(mut bytes: Vec<u8>) -> Self {
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        let nbits = bytes
            .iter()
            .rposition(|b| *b != 0)
            .map(|i| i * 8 + (8 - bytes[i].leading_zeros() as usize))
            .unwrap_or(0);
        Self { bytes, nbits }
    }

    fn from_hex_digits(digits: &str) -> Self {
        let digits: Vec<u8> = digits
            .bytes()
            .filter(|b| *b != b'_')
            .map(|b| (b as char).to_digit(16).unwrap_or(0) as u8)
            .collect();
        let mut bytes = Vec::new();
        let mut i = digits.len();
        while i > 0 {
            let lo = digits[i - 1];
            let hi = if i >= 2 { digits[i - 2] } else { 0 };
            bytes.push((hi << 4) | lo);
            i = i.saturating_sub(2);
        }
        Self::from_bytes(bytes)
    }

    fn from_binary_digits(digits: &str) -> Self {
        let bits: Vec<bool> = digits
            .bytes()
            .filter(|b| *b != b'_')
            .map(|b| b == b'1')
            .collect();
        let nbits = bits.len();
        let mut bytes = vec![0u8; nbits.div_ceil(8)];
        for (i, b) in bits.iter().rev().enumerate() {
            if *b {
                bytes[i / 8] |= 1 << (i % 8);
            }
        }
        Self::from_bytes(bytes)
    }

    fn from_decimal_digits(digits: &str) -> Self {
        let mut bytes: Vec<u8> = vec![0];
        for ch in digits.bytes().filter(|b| *b != b'_') {
            let d = (ch - b'0') as u32;
            let mut carry = d;
            for byte in bytes.iter_mut() {
                let v = (*byte as u32) * 10 + carry;
                *byte = (v & 0xFF) as u8;
                carry = v >> 8;
            }
            while carry > 0 {
                bytes.push((carry & 0xFF) as u8);
                carry >>= 8;
            }
        }
        Self::from_bytes(bytes)
    }
}

/// One parsed FASM line: `<tile>.<feature> [= <value>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FasmEntry {
    pub tile: String,
    pub feature: String,
    pub value: Option<FasmValue>,
}

/// Parse FASM text. `#` starts a comment. Blank lines are skipped.
pub fn parse_fasm(text: &str) -> Result<Vec<FasmEntry>, BitgenError> {
    let mut entries = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let no_comment = match raw_line.find('#') {
            Some(p) => &raw_line[..p],
            None => raw_line,
        };
        let line = no_comment.trim();
        if line.is_empty() {
            continue;
        }
        let (lhs, rhs) = match line.split_once('=') {
            Some((l, r)) => (l.trim(), Some(r.trim())),
            None => (line, None),
        };
        let (tile, feature) = match lhs.split_once('.') {
            Some((t, f)) if !t.is_empty() && !f.is_empty() => (t, f),
            _ => {
                return Err(BitgenError::BadFasm {
                    line: line_no,
                    reason: format!("expected '<tile>.<feature> [= <value>]', got '{line}'"),
                });
            }
        };
        if feature.contains(char::is_whitespace) || tile.contains(char::is_whitespace) {
            return Err(BitgenError::BadFasm {
                line: line_no,
                reason: format!("whitespace in tile or feature: '{line}'"),
            });
        }
        let value = match rhs {
            Some(v) => Some(parse_fasm_value(tile, feature, v)?),
            None => None,
        };
        if !seen.insert((tile.to_string(), feature.to_string())) {
            return Err(BitgenError::DuplicateFeature {
                tile: tile.to_string(),
                feature: feature.to_string(),
            });
        }
        entries.push(FasmEntry {
            tile: tile.to_string(),
            feature: feature.to_string(),
            value,
        });
    }
    Ok(entries)
}

fn parse_fasm_value(tile: &str, feature: &str, text: &str) -> Result<FasmValue, BitgenError> {
    let bad = || BitgenError::BadValue {
        tile: tile.to_string(),
        feature: feature.to_string(),
        value: text.to_string(),
    };
    let clean: String = text.chars().filter(|c| *c != '_').collect();
    if let Some(hex) = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))
    {
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
        return Ok(FasmValue::from_hex_digits(hex));
    }
    if let Some((width, rest)) = clean.split_once('\'') {
        let width: usize = width.parse().map_err(|_| bad())?;
        let (radix, digits) = rest.split_at(1);
        if digits.is_empty() {
            return Err(bad());
        }
        let v = match radix {
            "h" | "H" => {
                if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(bad());
                }
                FasmValue::from_hex_digits(digits)
            }
            "b" | "B" => {
                if !digits.bytes().all(|b| b == b'0' || b == b'1') {
                    return Err(bad());
                }
                if digits.len() > width {
                    return Err(bad());
                }
                FasmValue::from_binary_digits(digits)
            }
            "d" | "D" => {
                if !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(bad());
                }
                FasmValue::from_decimal_digits(digits)
            }
            _ => return Err(bad()),
        };
        // A value whose significant bits exceed the declared width is
        // malformed Verilog (e.g. 1'hFF).
        if v.significant_bits() > width {
            return Err(bad());
        }
        return Ok(v);
    }
    if !clean.is_empty() && clean.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(FasmValue::from_decimal_digits(&clean));
    }
    if !clean.is_empty() && clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(FasmValue::from_hex_digits(&clean));
    }
    Err(bad())
}

/// A feature is value-carrying when its name uses a bit range (`INIT[63:0]`)
/// or a BRAM-style indexed INIT name (`INIT_00`, `INITP_00`). Plain feature
/// names (`AFF.INIT`, `PIP....`) are presence-only.
fn feature_is_valued(feature: &str) -> bool {
    feature.contains('[') || feature.contains("INIT_") || feature.contains("INITP_")
}

// ---------------------------------------------------------------------------
// Frame assembly
// ---------------------------------------------------------------------------

/// One configuration frame: FAR plus 93 words.
pub type FrameWords = [u32; FRAME_WORDS];

/// Resolve FASM entries to frames. Frame bits start at 0; `!` bits must read
/// 0 in the frameset as built so far, and two features writing different
/// values to one bit is an error.
pub fn emit_frames(
    fasm: &str,
    device: &dyn BitgenDevice,
    segbits: &dyn SegbitsDb,
) -> Result<BTreeMap<u32, FrameWords>, BitgenError> {
    let entries = parse_fasm(fasm)?;
    let mut frames: BTreeMap<u32, FrameWords> = BTreeMap::new();
    let mut written: HashMap<(u32, u16), bool> = HashMap::new();
    for entry in &entries {
        let tile_type = device
            .tile_type(&entry.tile)
            .ok_or_else(|| BitgenError::UnknownTile {
                tile: entry.tile.clone(),
            })?;
        let base_far =
            device
                .tile_base_far(&entry.tile)
                .ok_or_else(|| BitgenError::UnknownTileType {
                    tile: entry.tile.clone(),
                })?;
        let refs = segbits.lookup(tile_type, &entry.feature).ok_or_else(|| {
            BitgenError::UnknownFeature {
                tile: entry.tile.clone(),
                feature: entry.feature.clone(),
            }
        })?;
        if let Some(v) = &entry.value {
            let highest = v.significant_bits();
            let n_set = refs.iter().filter(|r| !r.must_be_zero).count();
            if highest > n_set {
                return Err(BitgenError::ValueTooWide {
                    tile: entry.tile.clone(),
                    feature: entry.feature.clone(),
                    bits: n_set,
                    value_bits: highest,
                });
            }
        }
        let n_set = refs.iter().filter(|r| !r.must_be_zero).count();
        let mut set_idx = 0usize;
        for r in refs.iter() {
            if r.bit as usize >= FRAME_BITS {
                return Err(BitgenError::BadFar {
                    reason: format!(
                        "tile {} feature {}: frame bit {} exceeds {FRAME_BITS}",
                        entry.tile, entry.feature, r.bit
                    ),
                });
            }
            let far = base_far
                .checked_add(r.frame)
                .ok_or_else(|| BitgenError::BadFar {
                    reason: format!(
                        "tile {} feature {}: base FAR 0x{base_far:08x} + offset {} overflows",
                        entry.tile, entry.feature, r.frame
                    ),
                })?;
            let words = frames.entry(far).or_insert([0u32; FRAME_WORDS]);
            let current = (words[(r.bit / 32) as usize] >> (r.bit % 32)) & 1 == 1;
            if r.must_be_zero {
                if current {
                    return Err(BitgenError::MustBeZeroViolated {
                        tile: entry.tile.clone(),
                        feature: entry.feature.clone(),
                        far,
                        bit: r.bit,
                    });
                }
                continue;
            }
            let want = match &entry.value {
                // MSB-first expansion over the set bits only: the first
                // listed set bit takes the most significant value bit.
                Some(v) => v.bit(n_set - 1 - set_idx),
                None => true,
            };
            set_idx += 1;
            if let Some(prev) = written.get(&(far, r.bit)) {
                if *prev != want {
                    return Err(BitgenError::ConflictingBit {
                        tile: entry.tile.clone(),
                        feature: entry.feature.clone(),
                        far,
                        bit: r.bit,
                    });
                }
            }
            written.insert((far, r.bit), want);
            let slot = &mut words[(r.bit / 32) as usize];
            if want {
                *slot |= 1u32 << (r.bit % 32);
            } else {
                *slot &= !(1u32 << (r.bit % 32));
            }
        }
    }
    Ok(frames)
}

// ---------------------------------------------------------------------------
// Configuration CRC (CRC-32C, prjxray icap_crc, SIM_CONFIGE2 model)
// ---------------------------------------------------------------------------

/// One CRC step: feed a (5-bit register address, 32-bit data word) pair.
/// Direct port of prjxray `tools/bits2rbt/crc.h` `icap_crc`.
pub fn crc32c_step(addr: u32, data: u32, prev: u32) -> u32 {
    const POLY: u64 = 0x82F63B78u64 << 1;
    let mut val: u64 = ((addr as u64) << 32) | data as u64;
    let mut crc: u64 = prev as u64;
    for _ in 0..37 {
        if (val & 1) != (crc & 1) {
            crc ^= POLY;
        }
        val >>= 1;
        crc >>= 1;
    }
    crc as u32
}

/// Running CRC state over emitted packets.
struct CrcState {
    crc: u32,
}

impl CrcState {
    fn new() -> Self {
        Self { crc: 0 }
    }

    /// Feed one packet: each data word feeds as (address, word), except
    /// RCRC resets to 0 without feeding and a CRC-register write checks
    /// then resets. Every non-NOP write packet feeds: the previously
    /// listed register exclusion list was removed (critic FLAG F-A), no
    /// source for it exists, and bits2rbt `GetFpgaConfigurationCRC`
    /// feeds every non-NOP write with no exclusions.
    ///
    /// Returns the value to write into the CRC register for a CRC write, or
    /// None for other packets.
    fn feed_packet(&mut self, addr: u8, data: &[u32]) -> Option<u32> {
        if addr == reg::CMD && data.first().copied().unwrap_or(0) & 0x1F == cmd::RCRC {
            self.crc = 0;
            return None;
        }
        if addr == reg::CRC {
            let value = self.crc;
            self.crc = 0;
            return Some(value);
        }
        for word in data {
            self.crc = crc32c_step(addr as u32, *word, self.crc);
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Bitstream emission
// ---------------------------------------------------------------------------

/// Type 1 packet header: type 001, opcode write (10), register, word count.
fn type1_write_header(reg: u8, word_count: u32) -> u32 {
    debug_assert!(word_count <= 0x7FF);
    (1u32 << 29) | (2u32 << 27) | ((reg as u32) << 13) | (word_count & 0x7FF)
}

/// Emit a raw bitstream (no .bit TLV header) for a FASM design.
///
/// Packet order, per the observed Vivado sequences (7-series file in this
/// workspace for preamble/ending, BitFiltrator per-frame CRC comment for the
/// frame loop): dummy words, bus width detect, sync, NOOP, RCRC, IDCODE,
/// WCFG, then per frame in config order FAR, FDRI (93 words), FAR rewrite,
/// CRC, then RCRC, GRESTORE, DGHIGH_LFRM, START, DESYNC, trailing NOOPs.
pub fn emit_bitstream(
    fasm: &str,
    device: &dyn BitgenDevice,
    segbits: &dyn SegbitsDb,
) -> Result<Vec<u8>, BitgenError> {
    let idcode = device.idcode().ok_or_else(|| BitgenError::UnknownIdcode {
        part: device.part().to_string(),
    })?;
    let touched = emit_frames(fasm, device, segbits)?;
    let config_order = device.config_order_fars();
    let fars: Vec<u32> = if config_order.is_empty() {
        touched.keys().copied().collect()
    } else {
        let mut seen: HashSet<u32> = HashSet::new();
        for far in config_order {
            if !seen.insert(*far) {
                return Err(BitgenError::DuplicateFarInConfigOrder { far: *far });
            }
            // Untouched frames emit as zeros; they are still part of the
            // device config order.
        }
        for far in touched.keys() {
            if !seen.contains(far) {
                return Err(BitgenError::FrameNotInConfigOrder { far: *far });
            }
        }
        config_order.to_vec()
    };

    let mut words: Vec<u32> = Vec::new();
    let mut crc = CrcState::new();
    // Preamble: dummy words, bus width detect, sync, NOOP. These words are
    // not CRC-covered (coverage starts after RCRC).
    words.extend([DUMMY_WORD; 8]);
    words.push(BUS_WIDTH_DETECT);
    words.push(BUS_WIDTH_SYNC);
    words.extend([DUMMY_WORD; 2]);
    words.push(SYNC_WORD);
    words.push(NOOP_WORD);

    // RCRC resets the running CRC; IDCODE then WCFG feed it.
    push_write(&mut words, &mut crc, reg::CMD, &[cmd::RCRC]);
    push_write(&mut words, &mut crc, reg::IDCODE, &[idcode]);
    push_write(&mut words, &mut crc, reg::CMD, &[cmd::WCFG]);

    let zero_frame: FrameWords = [0u32; FRAME_WORDS];
    for far in &fars {
        let frame = touched.get(far).unwrap_or(&zero_frame);
        push_write(&mut words, &mut crc, reg::FAR, &[*far]);
        push_write(&mut words, &mut crc, reg::FDRI, frame);
        push_write(&mut words, &mut crc, reg::FAR, &[*far]);
        let value = crc
            .feed_packet(reg::CRC, &[0])
            .expect("CRC packet always yields a value");
        words.push(type1_write_header(reg::CRC, 1));
        words.push(value);
    }

    // Ending, mirroring the observed Vivado file: RCRC, GRESTORE,
    // DGHIGH_LFRM, START, DESYNC, trailing NOOPs. START runs the startup
    // sequence (DONE goes high); without it the configured device never
    // runs. DESYNC terminates the stream; it is also this module's own
    // parse_frames end marker. The FAR/MASK/CTL0 writes Vivado emits
    // after START are NOT emitted: their UltraScale+ values are unknown
    // (the 7-series values 0x03be0000 / 0x501 are not UltraScale+
    // values). The ~100 NOOP words Vivado emits between DGHIGH_LFRM and
    // START are not reproduced: their exact count is tier-2 detail.
    push_write(&mut words, &mut crc, reg::CMD, &[cmd::RCRC]);
    words.push(NOOP_WORD);
    words.push(NOOP_WORD);
    push_write(&mut words, &mut crc, reg::CMD, &[cmd::GRESTORE]);
    words.push(NOOP_WORD);
    push_write(&mut words, &mut crc, reg::CMD, &[cmd::DGHIGH_LFRM]);
    push_write(&mut words, &mut crc, reg::CMD, &[cmd::START]);
    push_write(&mut words, &mut crc, reg::CMD, &[cmd::DESYNC]);
    words.extend([NOOP_WORD; TRAILING_NOOPS]);

    let mut out = Vec::with_capacity(words.len() * 4);
    for w in words {
        out.extend_from_slice(&w.to_be_bytes());
    }
    Ok(out)
}

/// Append one Type 1 write packet, feeding the CRC state. The CRC-register
/// write is handled by the caller (it needs the value first).
fn push_write(words: &mut Vec<u32>, crc: &mut CrcState, addr: u8, data: &[u32]) {
    words.push(type1_write_header(addr, data.len() as u32));
    words.extend_from_slice(data);
    let _ = crc.feed_packet(addr, data);
}

// ---------------------------------------------------------------------------
// Bitstream parsing
// ---------------------------------------------------------------------------

/// One parsed packet.
struct Packet {
    opcode: u32,
    addr: Option<u8>,
    data: Vec<u32>,
}

/// Strip a .bit TLV header if present, returning the raw word stream.
/// Format per BitFiltrator `bitstream_spec.py`: field 1 is 2-byte length
/// 0x0009 plus 9 bytes starting 0x0ff0..., then key 'a', design name, then
/// TLV fields 'b' (part), 'c' (date), 'd' (time), 'e' (4-byte length plus raw
/// data). Returns the input unchanged when there is no TLV header.
fn strip_bit_header(bytes: &[u8]) -> Result<&[u8], BitgenError> {
    if bytes.len() < 13 || bytes[0] != 0x00 || bytes[1] != 0x09 {
        return Ok(bytes);
    }
    if bytes[2] != 0x0F || bytes[3] != 0xF0 {
        return Ok(bytes);
    }
    // Field 1 value is 9 bytes (bytes 2..11), then field 2 length (2 bytes)
    // and key 'a' (1 byte).
    let mut i = 11;
    if i + 3 > bytes.len() || bytes[i + 2] != 0x61 {
        return Err(BitgenError::Truncated {
            at_byte: i,
            reason: "bad .bit header: expected 'a' field".to_string(),
        });
    }
    i += 3;
    // Field 3: 2-byte length, design name plus NUL.
    if i + 2 > bytes.len() {
        return Err(BitgenError::Truncated {
            at_byte: i,
            reason: "bad .bit header: truncated design field".to_string(),
        });
    }
    let len3 = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
    i += 2 + len3;
    // TLV fields until 'e'.
    loop {
        if i + 1 > bytes.len() {
            return Err(BitgenError::Truncated {
                at_byte: i,
                reason: "bad .bit header: ran out of fields before 'e'".to_string(),
            });
        }
        let tag = bytes[i];
        i += 1;
        if tag == 0x65 {
            if i + 4 > bytes.len() {
                return Err(BitgenError::Truncated {
                    at_byte: i,
                    reason: "bad .bit header: truncated 'e' length".to_string(),
                });
            }
            let len =
                u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
            i += 4;
            if i + len > bytes.len() {
                return Err(BitgenError::Truncated {
                    at_byte: i,
                    reason: "bad .bit header: 'e' length exceeds file".to_string(),
                });
            }
            return Ok(&bytes[i..i + len]);
        }
        if i + 2 > bytes.len() {
            return Err(BitgenError::Truncated {
                at_byte: i,
                reason: "bad .bit header: truncated TLV length".to_string(),
            });
        }
        let len = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
        i += 2 + len;
    }
}

/// Parse the raw word stream after the sync word into packets.
fn parse_packets(words: &[u32]) -> Result<Vec<Packet>, BitgenError> {
    let mut packets = Vec::new();
    let mut i = 0usize;
    let mut prev_addr: Option<u8> = None;
    while i < words.len() {
        let header = words[i];
        let ptype = header >> 29;
        if ptype == 0 {
            // Type 0 is not a packet: dummy words after DESYNC or padding.
            // Only tolerate all-ones dummy words here.
            if header == DUMMY_WORD {
                i += 1;
                continue;
            }
            return Err(BitgenError::BadPacket {
                at_byte: i * 4,
                header,
            });
        }
        if ptype != 1 && ptype != 2 {
            return Err(BitgenError::BadPacket {
                at_byte: i * 4,
                header,
            });
        }
        let opcode = (header >> 27) & 0x3;
        let (addr, count) = if ptype == 1 {
            (
                Some(((header >> 13) & 0x1F) as u8),
                (header & 0x7FF) as usize,
            )
        } else {
            (prev_addr, (header & 0x07FF_FFFF) as usize)
        };
        if i + 1 + count > words.len() {
            return Err(BitgenError::Truncated {
                at_byte: i * 4,
                reason: format!(
                    "packet declares {count} words but only {} remain",
                    words.len() - i - 1
                ),
            });
        }
        let data = words[i + 1..i + 1 + count].to_vec();
        prev_addr = addr;
        packets.push(Packet { opcode, addr, data });
        i += 1 + count;
    }
    Ok(packets)
}

/// Parse a bitstream into frames, verifying every CRC-register write.
/// Accepts raw (.bin) input or .bit input with a TLV header. Returns frames
/// keyed by FAR. FDRI writes auto-increment the FAR by integer +1 per frame,
/// which follows FAR increment order (minor, column, row, block type).
pub fn parse_frames(bytes: &[u8]) -> Result<BTreeMap<u32, FrameWords>, BitgenError> {
    if bytes.len() % 4 != 0 {
        return Err(BitgenError::Truncated {
            at_byte: bytes.len(),
            reason: "bitstream length is not a whole number of words".to_string(),
        });
    }
    let raw = strip_bit_header(bytes)?;
    let words: Vec<u32> = raw
        .chunks_exact(4)
        .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let sync_pos = words
        .iter()
        .position(|w| *w == SYNC_WORD)
        .ok_or(BitgenError::MissingSync)?;
    let packets = parse_packets(&words[sync_pos + 1..])?;
    let mut frames: BTreeMap<u32, FrameWords> = BTreeMap::new();
    let mut far: u32 = 0;
    let mut crc = CrcState::new();
    let mut idcode_seen: Option<u32> = None;
    let mut stopped = false;
    for packet in &packets {
        if stopped {
            break;
        }
        let addr = match packet.addr {
            Some(a) => a,
            None => continue,
        };
        if packet.opcode != 2 {
            // Reads and NOOPs do not feed the CRC and carry no frame data.
            continue;
        }
        match addr {
            reg::FAR => {
                if let Some(v) = packet.data.first() {
                    far = *v;
                }
                let _ = crc.feed_packet(addr, &packet.data);
            }
            reg::FDRI => {
                if packet.data.len() % FRAME_WORDS != 0 {
                    return Err(BitgenError::PartialFrame {
                        far,
                        words: packet.data.len(),
                    });
                }
                for chunk in packet.data.chunks_exact(FRAME_WORDS) {
                    let mut frame = [0u32; FRAME_WORDS];
                    frame.copy_from_slice(chunk);
                    frames.insert(far, frame);
                    far = far.wrapping_add(1);
                }
                let _ = crc.feed_packet(addr, &packet.data);
            }
            reg::CRC => {
                let found = packet.data.first().copied().unwrap_or(0);
                let expected = crc.crc;
                if found != expected {
                    return Err(BitgenError::CrcMismatch {
                        far,
                        expected,
                        found,
                    });
                }
                crc.crc = 0;
            }
            reg::IDCODE => {
                if let Some(v) = packet.data.first() {
                    match idcode_seen {
                        None => idcode_seen = Some(*v),
                        Some(first) if first != *v => {
                            return Err(BitgenError::MultipleIdcodes { first, second: *v });
                        }
                        _ => {}
                    }
                }
                let _ = crc.feed_packet(addr, &packet.data);
            }
            reg::CMD => {
                let cmd = packet.data.first().copied().unwrap_or(0) & 0x1F;
                if cmd == cmd::DESYNC {
                    stopped = true;
                }
                let _ = crc.feed_packet(addr, &packet.data);
            }
            _ => {
                let _ = crc.feed_packet(addr, &packet.data);
            }
        }
    }
    let _ = idcode_seen;
    Ok(frames)
}

/// Parse a bitstream back to FASM text (canonical order: sorted by tile,
/// then feature). Inverse of `emit_bitstream` for observable features. See
/// the module docs for the all-zero-value limit and the fail-loud rule on
/// unexplained set bits.
pub fn parse_bitstream(
    bytes: &[u8],
    device: &dyn BitgenDevice,
    segbits: &dyn SegbitsDb,
) -> Result<String, BitgenError> {
    let frames = parse_frames(bytes)?;
    if let Some(expected_idcode) = device.idcode() {
        let found = find_idcode(bytes)?;
        if let Some(found) = found {
            if found != expected_idcode {
                return Err(BitgenError::IdcodeMismatch {
                    expected: expected_idcode,
                    found,
                });
            }
        }
    }
    // Every (far, bit) any enumerated feature lists, for the unexplained
    // check. Absolute FARs need each tile base, so enumerate per tile.
    let mut explained: HashSet<(u32, u16)> = HashSet::new();
    // (tile, feature, value-string-or-None) lines, collected then sorted.
    let mut lines: Vec<(String, String, Option<String>)> = Vec::new();
    // For each tile type, the set-bit signatures of features that list no
    // `!` bits. An all-`!` feature whose zero signature is one of these
    // is the exact complement of that feature (a binary attribute pair
    // such as FF INIT .V0/.V1). Built lazily per tile type; a tile type
    // appears on many tiles.
    let mut complement_sets: HashMap<String, HashSet<Vec<(u32, u16)>>> = HashMap::new();
    for tile in device.tile_names() {
        let tile_type = device
            .tile_type(tile)
            .ok_or_else(|| BitgenError::UnknownTileType { tile: tile.clone() })?;
        let complements = complement_sets
            .entry(tile_type.to_string())
            .or_insert_with(|| complement_signatures(segbits, tile_type));
        let base_far = device
            .tile_base_far(tile)
            .ok_or_else(|| BitgenError::UnknownTileType { tile: tile.clone() })?;
        for feature in segbits.features(tile_type) {
            let refs = segbits.lookup(tile_type, feature).ok_or_else(|| {
                BitgenError::InconsistentFeature {
                    tile: tile.clone(),
                    feature: feature.clone(),
                    reason: "features() lists it but lookup() returns None".to_string(),
                }
            })?;
            for r in refs {
                let far = base_far
                    .checked_add(r.frame)
                    .ok_or_else(|| BitgenError::BadFar {
                        reason: format!("tile {tile} feature {feature}: base FAR overflow"),
                    })?;
                explained.insert((far, r.bit));
            }
            let valued = feature_is_valued(feature);
            let n_set = refs.iter().filter(|r| !r.must_be_zero).count();
            // Reconstruct: listed set bit j holds value bit (n_set-1-j).
            let mut value_bits = vec![false; n_set];
            let mut any_set = false;
            let mut set_idx = 0usize;
            for r in refs.iter() {
                if r.must_be_zero {
                    continue;
                }
                let far = base_far + r.frame;
                let bit = frame_bit(&frames, far, r.bit);
                value_bits[n_set - 1 - set_idx] = bit;
                if bit {
                    any_set = true;
                }
                set_idx += 1;
            }
            let present = if valued {
                any_set
            } else {
                // A valueless feature is present when every listed bit
                // agrees (all set bits 1, all `!` bits 0) and the feature
                // is observable: it sets at least one bit, or it is the
                // exact complement of another feature of the same tile
                // type (a binary attribute pair such as FF INIT .V0/.V1,
                // where V1's zero set is V0's set and V0 lists no `!`
                // bits). An all-`!` feature with no complement (a mux
                // default such as VCC_WIRE) sets nothing and is
                // indistinguishable from absent, so it is dropped. A `!`
                // bit reading 1 means the feature is absent (another
                // selection holds), not an error; the error path is
                // reserved for genuinely inconsistent input. No inversion
                // is applied here: the DB's V0/V1 features already encode
                // the polarity (V0 sets, V1 clears), so rendering the
                // matching feature renders the right value.
                let has_complement =
                    n_set == 0 && !refs.is_empty() && complements.contains(&zero_signature(refs));
                all_set_bits_one(&frames, base_far, refs)
                    && all_zero_bits_zero(&frames, base_far, refs)
                    && (n_set > 0 || has_complement)
            };
            if !present {
                continue;
            }
            // A present feature must have all its `!` bits reading 0.
            // For valueless features this is already implied by the
            // presence rule above; the check guards valued features,
            // where presence is any-set and a `!` bit reading 1 is
            // genuinely inconsistent input.
            for r in refs.iter().filter(|r| r.must_be_zero) {
                let far = base_far + r.frame;
                if frame_bit(&frames, far, r.bit) {
                    return Err(BitgenError::MustBeZeroViolated {
                        tile: tile.clone(),
                        feature: feature.clone(),
                        far,
                        bit: r.bit,
                    });
                }
            }
            if valued {
                lines.push((
                    tile.clone(),
                    feature.clone(),
                    Some(format_value(&value_bits)),
                ));
            } else {
                lines.push((tile.clone(), feature.clone(), None));
            }
        }
    }
    // Fail loud on set bits no feature explains.
    let mut unexplained: Vec<(u32, Vec<u16>)> = Vec::new();
    for (far, words) in &frames {
        let mut bits = Vec::new();
        for (wi, word) in words.iter().enumerate() {
            let mut w = *word;
            while w != 0 {
                let b = w.trailing_zeros() as u16;
                let bit = (wi * 32 + b as usize) as u16;
                if !explained.contains(&(*far, bit)) {
                    bits.push(bit);
                }
                w &= w - 1;
            }
        }
        if !bits.is_empty() {
            unexplained.push((*far, bits));
        }
    }
    if let Some((far, bits)) = unexplained.into_iter().next() {
        return Err(BitgenError::UnexplainedBits { far, bits });
    }
    lines.sort();
    let mut out = String::new();
    for (tile, feature, value) in lines {
        match value {
            Some(v) => out.push_str(&format!("{tile}.{feature} = {v}\n")),
            None => out.push_str(&format!("{tile}.{feature}\n")),
        }
    }
    Ok(out)
}

/// Read one frame bit (0 when the frame was never written).
fn frame_bit(frames: &BTreeMap<u32, FrameWords>, far: u32, bit: u16) -> bool {
    match frames.get(&far) {
        Some(words) => {
            let idx = bit as usize;
            (words[idx / 32] >> (idx % 32)) & 1 == 1
        }
        None => false,
    }
}

/// True when every listed non-`!` bit reads 1 (presence test for a
/// valueless feature). Vacuously true for an empty set-bit list, so
/// callers must also require observability: at least one set bit, or
/// an exact complement (see the presence rule in `parse_bitstream`).
fn all_set_bits_one(frames: &BTreeMap<u32, FrameWords>, base_far: u32, refs: &[BitRef]) -> bool {
    refs.iter()
        .filter(|r| !r.must_be_zero)
        .all(|r| frame_bit(frames, base_far + r.frame, r.bit))
}

/// True when every listed `!` bit reads 0.
fn all_zero_bits_zero(frames: &BTreeMap<u32, FrameWords>, base_far: u32, refs: &[BitRef]) -> bool {
    refs.iter()
        .filter(|r| r.must_be_zero)
        .all(|r| !frame_bit(frames, base_far + r.frame, r.bit))
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
/// exact complement of one of these features (a binary attribute pair
/// such as FF INIT .V0/.V1). Features the DB lists but lookup cannot
/// resolve are skipped here; the main loop fails loud on them.
fn complement_signatures(segbits: &dyn SegbitsDb, tile_type: &str) -> HashSet<Vec<(u32, u16)>> {
    let mut set = HashSet::new();
    for feature in segbits.features(tile_type) {
        if let Some(refs) = segbits.lookup(tile_type, feature) {
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

/// Format reconstructed value bits (LSB-first vec, bit 0 is LSB) as
/// `N'h<HEX>` with N = number of bits, hex uppercase, zero-padded.
fn format_value(bits: &[bool]) -> String {
    let n = bits.len();
    let hex_digits = n.div_ceil(4);
    let mut hex = String::with_capacity(hex_digits);
    for d in (0..hex_digits).rev() {
        let mut nibble = 0u8;
        for b in 0..4 {
            let i = d * 4 + b;
            if i < n && bits[i] {
                nibble |= 1 << b;
            }
        }
        hex.push(
            char::from_digit(nibble as u32, 16)
                .unwrap()
                .to_ascii_uppercase(),
        );
    }
    format!("{n}'h{hex}")
}

/// Find the first IDCODE packet value in a bitstream, if any.
fn find_idcode(bytes: &[u8]) -> Result<Option<u32>, BitgenError> {
    let raw = strip_bit_header(bytes)?;
    if raw.len() % 4 != 0 {
        return Err(BitgenError::Truncated {
            at_byte: raw.len(),
            reason: "bitstream length is not a whole number of words".to_string(),
        });
    }
    let words: Vec<u32> = raw
        .chunks_exact(4)
        .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let sync_pos = words
        .iter()
        .position(|w| *w == SYNC_WORD)
        .ok_or(BitgenError::MissingSync)?;
    let packets = parse_packets(&words[sync_pos + 1..])?;
    for packet in &packets {
        if packet.opcode == 2 && packet.addr == Some(reg::IDCODE) {
            return Ok(packet.data.first().copied());
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Provisional stubs (until segbits-vendor and device-file land)
// ---------------------------------------------------------------------------

/// In-memory segbits DB for tests. Entries are (tile_type, feature, bits).
/// INIT lists must be ordered MSB-first (list index 0 = most significant
/// value bit). Provisional: replaced by the vendored prjuray-db data.
#[derive(Debug, Default)]
pub struct StubSegbitsDb {
    entries: HashMap<(String, String), Vec<BitRef>>,
    tile_types: Vec<String>,
    features: HashMap<String, Vec<String>>,
}

impl StubSegbitsDb {
    pub fn new(entries: Vec<(String, String, Vec<BitRef>)>) -> Self {
        let mut db = Self::default();
        for (tile_type, feature, bits) in entries {
            if !db.tile_types.contains(&tile_type) {
                db.tile_types.push(tile_type.clone());
            }
            db.features
                .entry(tile_type.clone())
                .or_default()
                .push(feature.clone());
            db.entries.insert((tile_type, feature), bits);
        }
        db.tile_types.sort();
        for list in db.features.values_mut() {
            list.sort();
        }
        db
    }
}

impl SegbitsDb for StubSegbitsDb {
    fn lookup(&self, tile_type: &str, feature: &str) -> Option<&[BitRef]> {
        self.entries
            .get(&(tile_type.to_string(), feature.to_string()))
            .map(Vec::as_slice)
    }

    fn tile_types(&self) -> &[String] {
        &self.tile_types
    }

    fn features(&self, tile_type: &str) -> &[String] {
        self.features
            .get(tile_type)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// In-memory device for tests. Tiles are (tile name, tile type, base FAR).
/// Provisional: replaced by the device-file worker's BitFiltrator-schema
/// device. IDCODEs below are from UG570 table 2 (xcku3p 0x04A63093,
/// xcvu3p 0x14B39093 with revision nibble 1).
#[derive(Debug, Default)]
pub struct StubDevice {
    part: String,
    idcode: Option<u32>,
    tile_type: HashMap<String, String>,
    tile_base_far: HashMap<String, u32>,
    tile_names: Vec<String>,
    config_order: Vec<u32>,
}

impl StubDevice {
    pub fn new(
        part: &str,
        idcode: Option<u32>,
        tiles: Vec<(String, String, u32)>,
        config_order: Vec<u32>,
    ) -> Self {
        let mut dev = Self {
            part: part.to_string(),
            idcode,
            ..Self::default()
        };
        for (name, ty, base) in tiles {
            dev.tile_type.insert(name.clone(), ty);
            dev.tile_base_far.insert(name.clone(), base);
            dev.tile_names.push(name);
        }
        dev.tile_names.sort();
        dev.config_order = config_order;
        dev
    }
}

impl BitgenDevice for StubDevice {
    fn part(&self) -> &str {
        &self.part
    }

    fn idcode(&self) -> Option<u32> {
        self.idcode
    }

    fn tile_type(&self, tile: &str) -> Option<&str> {
        self.tile_type.get(tile).map(String::as_str)
    }

    fn tile_base_far(&self, tile: &str) -> Option<u32> {
        self.tile_base_far.get(tile).copied()
    }

    fn tile_names(&self) -> &[String] {
        &self.tile_names
    }

    fn config_order_fars(&self) -> &[u32] {
        &self.config_order
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn far_codec_roundtrips_ug570_layout() {
        // UG570 table 9-21: reserved [31:27], block [26:24], row [23:18],
        // column [17:8], minor [7:0].
        let far = far_encode(1, 3, 226, 12).unwrap();
        assert_eq!(far, (1 << 24) | (3 << 18) | (226 << 8) | 12);
        assert_eq!(far_decode(far), (1, 3, 226, 12));
        assert_eq!(far_encode(0, 0, 0, 0).unwrap(), 0);
        assert!(far_encode(8, 0, 0, 0).is_err());
        assert!(far_encode(0, 64, 0, 0).is_err());
        assert!(far_encode(0, 0, 1024, 0).is_err());
    }

    #[test]
    fn crc32c_matches_real_vivado_bitstream_vector() {
        // BitFiltrator bitstream.py per-frame CRC example (Vivado-generated,
        // UltraScale 123-word frame): all-zero frame at FAR 1, CRC written
        // after the FDRI words plus the FAR rewrite, is 0x06b1285e.
        let frame = [0u32; 123];
        let mut crc = 0u32;
        for word in frame {
            crc = crc32c_step(reg::FDRI as u32, word, crc);
        }
        crc = crc32c_step(reg::FAR as u32, 1, crc);
        assert_eq!(crc, 0x06b1_285e);
    }

    #[test]
    fn crc32c_zero_feed_is_identity() {
        assert_eq!(crc32c_step(0, 0, 0), 0);
    }

    #[test]
    fn fasm_parser_accepts_comments_and_verilog_values() {
        let entries = parse_fasm(
            "# comment\nSLICE_X0Y0.A6LUT.INIT[63:0] = 64'h8000000000000001\n\nSLICE_X0Y0.AFF.INIT\nINT_X0Y0.PIP.A.B\n",
        )
        .unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].tile, "SLICE_X0Y0");
        assert_eq!(entries[0].feature, "A6LUT.INIT[63:0]");
        let v = entries[0].value.as_ref().unwrap();
        assert!(v.bit(63));
        assert!(v.bit(0));
        assert!(!v.bit(1));
        assert!(entries[1].value.is_none());
    }

    #[test]
    fn fasm_parser_rejects_bad_lines_and_duplicates() {
        assert!(parse_fasm("NOT_A_FEATURE\n").is_err());
        assert!(parse_fasm("A.B = not_a_number\n").is_err());
        assert!(parse_fasm("SLICE_X0Y0.AFF.INIT\nSLICE_X0Y0.AFF.INIT\n").is_err());
    }

    fn one_bit_db() -> (StubDevice, StubSegbitsDb) {
        let device = StubDevice::new(
            "xcku3p",
            Some(0x04A6_3093),
            vec![("SLICE_X0Y0".to_string(), "CLEL_L".to_string(), 0x500)],
            vec![0x50C],
        );
        let db = StubSegbitsDb::new(vec![(
            "CLEL_L".to_string(),
            "AFF.INIT".to_string(),
            vec![BitRef::set(12, 64)],
        )]);
        (device, db)
    }

    #[test]
    fn unknown_feature_names_tile_and_feature() {
        let (device, db) = one_bit_db();
        let err = emit_frames("SLICE_X0Y0.NOSUCH.INIT\n", &device, &db).unwrap_err();
        assert_eq!(
            err,
            BitgenError::UnknownFeature {
                tile: "SLICE_X0Y0".to_string(),
                feature: "NOSUCH.INIT".to_string()
            }
        );
    }

    #[test]
    fn unknown_tile_fails_loud() {
        let (device, db) = one_bit_db();
        let err = emit_frames("SLICE_X9Y9.AFF.INIT\n", &device, &db).unwrap_err();
        assert_eq!(
            err,
            BitgenError::UnknownTile {
                tile: "SLICE_X9Y9".to_string()
            }
        );
    }

    #[test]
    fn must_be_zero_bit_set_by_earlier_feature_fails_loud() {
        let device = StubDevice::new(
            "xcku3p",
            Some(0x04A6_3093),
            vec![("SLICE_X0Y0".to_string(), "CLEL_L".to_string(), 0x500)],
            vec![0x50C],
        );
        let db = StubSegbitsDb::new(vec![
            (
                "CLEL_L".to_string(),
                "AFF.INIT".to_string(),
                vec![BitRef::set(12, 64)],
            ),
            (
                "CLEL_L".to_string(),
                "BFF.INIT".to_string(),
                vec![BitRef::zero(12, 64)],
            ),
        ]);
        let err =
            emit_frames("SLICE_X0Y0.AFF.INIT\nSLICE_X0Y0.BFF.INIT\n", &device, &db).unwrap_err();
        assert_eq!(
            err,
            BitgenError::MustBeZeroViolated {
                tile: "SLICE_X0Y0".to_string(),
                feature: "BFF.INIT".to_string(),
                far: 0x50C,
                bit: 64
            }
        );
    }

    #[test]
    fn value_wider_than_feature_list_fails_loud() {
        let (device, db) = one_bit_db();
        let err = emit_frames("SLICE_X0Y0.AFF.INIT = 1'h1\n", &device, &db).unwrap();
        assert_eq!(err.len(), 1);
        // AFF.INIT with an explicit 1-bit value works; a 2-bit value must fail.
        let err2 = emit_frames("SLICE_X0Y0.AFF.INIT = 2'h3\n", &device, &db).unwrap_err();
        assert_eq!(
            err2,
            BitgenError::ValueTooWide {
                tile: "SLICE_X0Y0".to_string(),
                feature: "AFF.INIT".to_string(),
                bits: 1,
                value_bits: 2
            }
        );
    }

    #[test]
    fn missing_sync_and_bad_crc_fail_loud() {
        let (device, db) = one_bit_db();
        let bytes = emit_bitstream("SLICE_X0Y0.AFF.INIT\n", &device, &db).unwrap();
        assert!(parse_frames(&bytes[..48]).is_err());
        let mut corrupt = bytes.clone();
        // Flip a bit inside the FDRI payload (word 23 is the first frame word).
        corrupt[23 * 4 + 3] ^= 0x01;
        assert!(matches!(
            parse_frames(&corrupt),
            Err(BitgenError::CrcMismatch { .. })
        ));
    }

    #[test]
    fn empty_fasm_emits_valid_framing() {
        let (device, db) = one_bit_db();
        let bytes = emit_bitstream("", &device, &db).unwrap();
        let frames = parse_frames(&bytes).unwrap();
        // Config order lists one FAR, emitted as a zero frame.
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[&0x50C], [0u32; FRAME_WORDS]);
        assert_eq!(parse_bitstream(&bytes, &device, &db).unwrap(), "");
    }
}
