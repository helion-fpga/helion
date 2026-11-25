//! UltraScale+ `.bit` packet-layer decoder.
//!
//! Parses a Xilinx UltraScale+ `.bit` file (or raw `.bin` word stream) into
//! configuration frames addressed by their Frame Address Register (FAR).
//! This module owns the packet layer only: `.bit` header, sync word,
//! UG570 configuration packets, FAR tracking, FDRI frame collection, and
//! the MFWR multiple-frame-write replication used by compressed bitstreams.
//! It does not decode frames into FASM (that is the frames-to-FASM worker).
//! It does verify the configuration CRC: every CRC-register write must
//! carry the running CRC-32C of the stream (prjxray `icap_crc` model,
//! the same `crc32c_step` as `bitgen.rs` `CrcState`), proven against 16
//! real Vivado files (see the CRC section below). A mismatch fails loud,
//! so a payload-corrupted `.bit` that still parses can no longer decode
//! to wrong FASM silently.
//!
//! Sources, in order of authority:
//! - UG570 (UltraScale Architecture Configuration User Guide), live at
//!   docs.amd.com: packet formats tables 9-16/9-18, register map table
//!   9-19, FAR layout table 9-21, command codes table 9-22, frame size
//!   93 words for UltraScale+ (table 1). Verified against the live UG570
//!   pages: MFWR is register `01010`, MFW is command `00010`, described as
//!   "Multiple frame write: used to perform a write of a single frame data
//!   to multiple frame addresses", and the CMD register note says the
//!   command present in CMD "is executed each time the FAR register is
//!   loaded with a new value".
//! - BitFiltrator (EPFL, FPL 2022), `bitfiltrator-ref/src/` in the base
//!   worktree: `packet.py` / `packet_spec.py` (packet layout, register and
//!   command enums, RSVD_30 sinkhole), `bitstream.py` (packet decode loop,
//!   per-section decode until DESYNC, end-of-row padding frames,
//!   per-frame-CRC notes), `frame.py` (`FrameAddressIncrementer`, the
//!   geometry-aware FAR increment), `arch_spec.py`
//!   (`UltraScalePlusSpec.frame_size_words() == 93`, FAR field widths),
//!   `bitstream_spec.py` (`.bit` TLV header, sync/dummy words, 2
//!   end-of-row padding frames), and `resources/devices/*.json` (per-part
//!   minor counts per configuration column, used for the embedded FAR
//!   geometry tables below).
//! - AMD answer record 16996 and the older UG002/XAPP452 MFWR algorithm,
//!   which AMD states applies to Vivado for newer families: write WCFG,
//!   write the frame to FDRI, write FAR with the first address, write the
//!   MFW command, write dummy words to MFWR, write FAR with the next
//!   address, write dummy words to MFWR, repeat. The frame is loaded once
//!   into the configuration logic and copied to many addresses. In
//!   UltraScale+ Vivado streams the MFWR dummy write is 14 zero words
//!   (observed in every compressed corpus file, 55,154 writes, all 14
//!   words, all zero).
//! - Helion `bitgen.rs` (`parse_packets`, `parse_frames`,
//!   `strip_bit_header`): the in-repo packet parser. This module agrees
//!   with it on header layout, sync word, Type 1/2 layout, register and
//!   command codes, and the 93-word frame. It differs in two places where
//!   the real Vivado files prove `bitgen.rs` incomplete: `bitgen.rs` reads
//!   the register address as bits [17:13] only, while UG570 defines the
//!   field as bits [26:13] (this module decodes the full 14-bit field and
//!   rejects values above 31; the two agree whenever bits [26:18] are
//!   zero, which holds for every real header observed); and `bitgen.rs`
//!   increments FAR by plain integer +1 per FDRI frame, while the device
//!   increments geometry-aware (minor wraps at the column height, then
//!   column, then row, then block type). Plain +1 is wrong for any
//!   multi-frame FDRI packet that crosses a column boundary.
//!
//! FAR auto-increment, proven three ways on real files: (1) BitFiltrator
//! `FrameAddressIncrementer` wraps minor at the per-column count from the
//!   device summary; (2) the MFWR run-start anchors in `d2.bit` (xcku3p)
//!   and `xcau10p.bit` match geometry-aware increment on all 68 runs and
//!   mismatch plain +1 on 4 of them, with the mismatch values matching
//!   the device JSON column heights exactly (for example `d2.bit`
//!   start FAR 0x0004c738, 26 frames: plain +1 predicts last frame
//!   0x0004c751, the run starts at 0x0004c805, which is 20 frames in
//!   column 199 with 76 minors plus 6 frames in column 200 with 16
//!   minors, exactly the JSON counts); (3) the decoded unique frame
//!   totals equal the device totals exactly (xcku3p 41,460, xcau10p
//!   14,364, xczu7ev 51,882) only under geometry-aware increment with
//!   MFWR replication and end-of-row padding handling.
//!
//! Rules enforced here (fail loud, never silently skip data):
//! - Unknown opcode (3), malformed NOOP, nonzero Type 1 reserved bits,
//!   register field above 31, truncated packet payload, Type 2 packet
//!   with no preceding Type 1, Type 2 for any register other than FDRI
//!   (except the RSVD_30 sinkhole, whose word count is bogus by design
//!   and carries no payload), and any non-packet word at a packet
//!   boundary are all errors.
//! - FDRI payload must be a nonzero multiple of 93 words. A 7-series
//!   stream (101-word frames) is rejected here, not misdecoded.
//! - FDRI requires a prior FAR write and a prior IDCODE write in the
//!   same section, and a geometry that can address the FAR (embedded
//!   table by IDCODE, or caller-supplied geometry). FAR writes must
//!   carry exactly one word, with reserved bits [31:27] zero and block
//!   type 0 (CLB/IO/CLK), 1 (BRAM content), or 7 (the post-START
//!   sentinel FAR Vivado writes, which addresses no frame). Block types
//!   2-6 are rejected: no real file uses them and UG570 defines only
//!   0 and 1.
//! - MFWR payloads must be all zero (they are dummy pad words). A
//!   nonzero MFWR payload is an error. The replicated frame is the
//!   last FDRI frame written, and replication happens when FAR is
//!   loaded while CMD holds MFW, per UG570. The run-start FAR is
//!   checked against the last FDRI frame FAR (anchor check); a
//!   mismatch is an error.
//! - The stream must contain at least one section ending in DESYNC,
//!   and at least one FDRI frame. Trailing words after DESYNC may only
//!   be dummy words, NOOPs, bus-width patterns, or a resync starting
//!   the next SLR section. A packet after DESYNC without a resync is
//!   an error.
//! - Register 19 is accepted: UG570 calls it a reserved/unused
//!   register and its own sample bitstream writes it in the preamble
//!   (header 0x30026001), and every real Vivado file observed writes
//!   it once with value 0. Other undocumented register addresses are
//!   accepted the same way (payload consumed as a known packet).
//! - The `.bit` header is parsed strictly: the 9-byte field-1 value,
//!   the `a` design field, and TLV fields through `e` must be present
//!   and well formed. Input without the `.bit` magic is treated as a
//!   raw word stream. Words before the sync word must be dummy words
//!   or bus-width detect/sync patterns. Zero-word runs between packets
//!   are tolerated only as whole padding frames (a multiple of 93
//!   zero words), per BitFiltrator.
//!
//! Known limits, stated plainly:
//! - Configuration CRC verification (CRC-32C, prjxray `icap_crc`:
//!   `crc32c_step` over each (5-bit register address, 32-bit data word)
//!   pair, LSB-first, 37 bits per pair, starting from 0 after RCRC).
//!   The feed rule is: RCRC resets the running CRC without feeding; a
//!   CRC-register write must carry the running CRC and then resets it;
//!   every other non-NOP write packet feeds each data word, with no
//!   register exclusions; reads and NOOPs never feed. Proven against 16
//!   real Vivado files (`decoder-validation/crc_probe.py`, same feed
//!   rule in Python): all 14 xczu3eg pairs plus compressed `d2.bit`
//!   (xcku3p, 41,119 MFWR packets) and `xcau10p.bit`, 32 CRC writes in
//!   total, 32 match, 0 mismatch. Every file carries exactly 2 CRC
//!   writes: the first covers the whole configuration payload (preamble
//!   plus all FDRI data) since RCRC, the second covers the ending
//!   commands (GRESTORE, DGHIGH_LFRM, START, sentinel FAR, MASK, CTL0)
//!   since the first CRC write reset the state (Vivado writes no RCRC
//!   between them; the CRC write itself is the reset). A stream with no
//!   CRC writes at all is accepted without verification (synthetic test
//!   streams, partial bitstreams, and 7-series files carry none); a
//!   present-but-wrong CRC value is a hard error. The CRC state is one
//!   running value for the whole file and is not reset by a resync: a
//!   new section is expected to start with its own RCRC, as Vivado
//!   emits. `bitgen.rs` `parse_frames` implements the identical check
//!   (`BitgenError::CrcMismatch`) for the emitter round-trip path.
//! - FAR geometry is required for correct addressing. `parse_bitstream`
//!   uses the embedded tables below, keyed by the stream IDCODE with
//!   the version nibble masked. Parts without an embedded table
//!   (notably xczu9eg: no BitFiltrator device JSON and no prjuray
//!   tilegrid exist on disk for it) fail loud with the IDCODE named;
//!   use `parse_bitstream_with_geometry` with caller-supplied geometry
//!   for those parts. The trait geometry is the general path; the
//!   embedded tables exist only because the mandated bytes-only API
//!   takes no geometry argument.
//! - End-of-row padding (2 all-zero frames after the last frame of a
//!   row, inside a multi-frame FDRI packet) is skipped only when the
//!   stream contains no MFW command, which is the uncompressed case
//!   (verified: exactly 24 padding frames skipped in
//!   `resizer_zcu104.bit`, 6 rows times 2 blocks times 2). Compressed
//!   streams carry no padding (verified: the compressed totals match
//!   the device totals exactly with zero padding skipped). A partial
//!   bitstream that mixes MFW with row-end padding is not handled.
//! - Frames are returned sorted by FAR with last-write-wins on
//!   duplicate FARs. Duplicates are legitimate: the MFWR run repeats
//!   the seed frame at its own FAR, and a group-start FAR load under
//!   a stale MFW command replicates the previous seed before the
//!   group's FDRI overwrites it. Both are hardware behavior, not
//!   errors.
//! - Multi-SLR streams are handled per section (resync resets FAR,
//!   IDCODE, CMD, and frame buffer; each section must end in DESYNC),
//!   including the RSVD_30 sinkhole packet that marks an SLR boundary.
//!   No multi-SLR file is available locally, so that path is covered
//!   by synthetic unit tests only, not by a real-file check.
//!
//! prjuray note: the task named `utils/bitstream.py` in
//! `prjuray-tools-ref` as ground truth for packet layout, but that file
//! contains no packet parser at all (only `load_bitdata` helpers over
//! already-decoded frames), and the `prjuray.bitstream` package it
//! imports (`FRAME_WORD_COUNT`, `WORD_SIZE_BITS`) is not vendored in
//! that checkout and is not installed here. The packet-level ground
//! truth actually on disk is BitFiltrator (`packet.py`,
//! `packet_spec.py`, `bitstream.py`) plus UG570 itself, and this module
//! is checked bit-for-bit against both, plus against five real Vivado
//! `.bit` files. No UG570-versus-reference disagreement was found.
//! The one apparent disagreement (register field width, bits [26:13]
//! in UG570/BitFiltrator versus bits [17:13] in `bitgen.rs`) is not a
//! real disagreement: both decode every observed header identically.

use crate::bitgen::crc32c_step;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// One configuration frame: its FAR plus exactly 93 words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub far: u32,
    pub words: Vec<u32>,
}

/// Words per UltraScale+ configuration frame, verified three ways:
/// BitFiltrator `UltraScalePlusSpec.frame_size_words() == 93`, UG570
/// table 1 as cited by `bitgen.rs` (`FRAME_WORDS`), and empirically:
/// every FDRI payload in five real Vivado UltraScale+ `.bit` files is a
/// multiple of 93 and none is a multiple of 101 (7-series frame size).
pub fn frame_word_count() -> usize {
    FRAME_WORDS
}

/// Geometry a caller supplies when the stream's part has no embedded
/// table (see `parse_bitstream_with_geometry`). The FAR increment rule
/// is: minor +1, wrapping to the next column at the column height,
/// then the next row, then the next block type. All queries are about
/// the frame address space, not about tiles or features.
pub trait FarGeometry {
    /// Number of minor addresses (frames) in one column of one row of
    /// one block. `block` is 0 (CLB/IO/CLK) or 1 (BRAM content).
    fn minor_count(&self, block: u8, row: u8, col: u16) -> Option<u16>;
    /// Number of columns in one row of one block.
    fn column_count(&self, block: u8, row: u8) -> Option<u16>;
    /// Number of FAR rows in one block.
    fn row_count(&self, block: u8) -> Option<u8>;
}

/// Parse a `.bit` file (or raw `.bin` word stream) into frames.
///
/// Uses the embedded FAR geometry tables, selected by the stream
/// IDCODE. Fails loud on unknown parts, malformed packets, truncated
/// payloads, bad frame counts, anchor mismatches, and unterminated
/// sections. Frames come back sorted by FAR; a repeated FAR keeps the
/// last write (see the module docs on duplicates).
pub fn parse_bitstream(bytes: &[u8]) -> Result<Vec<Frame>, String> {
    parse_inner(bytes, None)
}

/// Parse with caller-supplied FAR geometry, for parts with no embedded
/// table (for example xczu9eg). Identical to `parse_bitstream` except
/// the IDCODE-to-table lookup is skipped: the supplied geometry must
/// cover every section in the stream.
pub fn parse_bitstream_with_geometry(
    bytes: &[u8],
    geometry: &dyn FarGeometry,
) -> Result<Vec<Frame>, String> {
    parse_inner(bytes, Some(geometry))
}

// ---------------------------------------------------------------------------
// Constants (UG570)
// ---------------------------------------------------------------------------

/// Sync word (UG570, BitFiltrator `bitstream_spec.py`).
const SYNC_WORD: u32 = 0xAA99_5566;
/// Dummy word.
const DUMMY_WORD: u32 = 0xFFFF_FFFF;
/// Bus width detect pattern.
const BUS_WIDTH_DETECT: u32 = 0x0000_00BB;
/// Bus width sync pattern.
const BUS_WIDTH_SYNC: u32 = 0x1122_0044;
/// NOOP packet: Type 1, opcode 0, register 0, word count 0.
const NOOP_WORD: u32 = 0x2000_0000;

/// Words per UltraScale+ configuration frame (UG570 table 1).
const FRAME_WORDS: usize = 93;

/// Configuration register addresses (UG570 table 9-19).
const REG_CRC: u8 = 0;
const REG_FAR: u8 = 1;
const REG_FDRI: u8 = 2;
const REG_CMD: u8 = 4;
const REG_MFWR: u8 = 10;
const REG_IDCODE: u8 = 12;
const REG_RSVD_30: u8 = 30;

/// Command codes (UG570 table 9-22).
const CMD_WCFG: u32 = 1;
const CMD_MFW: u32 = 2;
const CMD_RCRC: u32 = 7;
const CMD_DESYNC: u32 = 13;

/// FAR block types (UG570 table 9-21). Block 7 is the post-START
/// sentinel FAR Vivado writes (observed as 0x07FC0000 in the
/// uncompressed corpus files); it addresses no frame.
const BLOCK_CLB_IO_CLK: u8 = 0;
const BLOCK_BRAM_CONTENT: u8 = 1;
const BLOCK_SENTINEL: u8 = 7;

// ---------------------------------------------------------------------------
// Embedded FAR geometry tables
// ---------------------------------------------------------------------------

/// One part's FAR geometry, in the compact encoding the device data
/// allows: every FAR row in a part has the identical minor-count list
/// per block (verified by the generator for each part below), so one
/// list per block plus a row count captures the whole address space.
///
/// Generated from the BitFiltrator device JSONs
/// (`bitfiltrator-ref/resources/devices/<part>.json`,
/// `num_minors_per_std_colMajor` for block 0 and
/// `num_minors_per_bram_content_colMajor` for block 1) by
/// `/tmp/gen_tables.py`, which asserts the identical-rows property
/// and the IDCODE match for each part. Regenerate from those JSONs
/// if the tables ever need a new part; do not hand-edit the numbers.
struct PartGeometry {
    /// IDCODE with the version nibble masked off (`idcode & 0x0FFF_FFFF`).
    idcode: u32,
    /// Number of FAR rows per block.
    rows: u8,
    /// Minor counts per column, block 0 (CLB/IO/CLK), one row template.
    std_minors: &'static [u16],
    /// Minor counts per column, block 1 (BRAM content), one row template.
    bram_minors: &'static [u16],
}

impl FarGeometry for PartGeometry {
    fn minor_count(&self, block: u8, row: u8, col: u16) -> Option<u16> {
        if row >= self.rows {
            return None;
        }
        let list = match block {
            BLOCK_CLB_IO_CLK => self.std_minors,
            BLOCK_BRAM_CONTENT => self.bram_minors,
            _ => return None,
        };
        list.get(col as usize).copied()
    }

    fn column_count(&self, block: u8, row: u8) -> Option<u16> {
        if row >= self.rows {
            return None;
        }
        match block {
            BLOCK_CLB_IO_CLK => Some(self.std_minors.len() as u16),
            BLOCK_BRAM_CONTENT => Some(self.bram_minors.len() as u16),
            _ => None,
        }
    }

    fn row_count(&self, block: u8) -> Option<u8> {
        match block {
            BLOCK_CLB_IO_CLK | BLOCK_BRAM_CONTENT => Some(self.rows),
            _ => None,
        }
    }
}

/// The table layout below is intentional (16 minor counts per line, as
/// emitted by the generator) and is excluded from rustfmt so regenerated
/// tables diff cleanly against these.
#[rustfmt::skip]
const PART_GEOMETRIES: &[PartGeometry] = &[
    // xcku3p: IDCODE 0x04a63093, 4 FAR rows, 230 std cols (7805 frames/row), 10 bram cols (2560 frames/row), device total 41460 frames.
    PartGeometry { idcode: 0x04a63093, rows: 4,
        std_minors: &[
        8, 4, 12, 4, 76, 16, 16, 76, 16, 6, 76, 16, 16, 76, 8, 16,
        76, 16, 16, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 16, 6, 76,
        16, 16, 76, 8, 16, 76, 16, 16, 76, 16, 16, 76, 8, 16, 76, 16,
        16, 76, 16, 16, 76, 16, 6, 76, 16, 16, 76, 8, 16, 76, 16, 16,
        76, 8, 16, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 16, 6, 76,
        16, 16, 76, 16, 16, 76, 4, 10, 4, 76, 16, 16, 76, 16, 16, 76,
        16, 6, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 8, 16, 76, 16,
        16, 76, 8, 16, 76, 16, 16, 76, 16, 6, 76, 16, 16, 76, 8, 16,
        76, 16, 16, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 16, 16, 76,
        16, 6, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 16, 16, 76, 9,
        76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 16, 16, 76, 16, 6, 76,
        16, 16, 76, 8, 16, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 8,
        16, 76, 16, 16, 76, 16, 6, 76, 16, 16, 76, 4, 10, 4, 76, 16,
        16, 76, 16, 6, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 8, 16,
        76, 16, 16, 76, 4, 8,
        ],
        bram_minors: &[
        256, 256, 256, 256, 256, 256, 256, 256, 256, 256,
        ],
    },
    // xcau10p: IDCODE 0x04ac4093, 3 FAR rows, 114 std cols (3764 frames/row), 4 bram cols (1024 frames/row), device total 14364 frames.
    PartGeometry { idcode: 0x04ac4093, rows: 3,
        std_minors: &[
        8, 4, 12, 4, 76, 16, 16, 76, 16, 16, 76, 16, 6, 76, 16, 16,
        76, 8, 16, 76, 16, 16, 76, 16, 16, 76, 16, 16, 76, 16, 16, 76,
        16, 16, 76, 4, 10, 4, 76, 16, 16, 76, 16, 16, 76, 16, 6, 76,
        16, 16, 76, 8, 16, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 8,
        16, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 8, 16, 76, 16, 16,
        76, 16, 6, 76, 16, 16, 76, 4, 10, 4, 76, 16, 16, 76, 16, 6,
        76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76,
        4, 8,
        ],
        bram_minors: &[
        256, 256, 256, 256,
        ],
    },
    // xczu7ev: IDCODE 0x04a5a093, 6 FAR rows, 218 std cols (7367 frames/row), 5 bram cols (1280 frames/row), device total 51882 frames.
    PartGeometry { idcode: 0x04a5a093, rows: 6,
        std_minors: &[
        8, 4, 12, 4, 76, 16, 16, 76, 16, 16, 76, 16, 6, 76, 16, 16,
        76, 8, 16, 76, 16, 16, 76, 16, 16, 76, 16, 16, 76, 16, 16, 76,
        16, 16, 76, 4, 10, 4, 76, 16, 16, 76, 16, 16, 76, 16, 16, 76,
        16, 16, 76, 16, 16, 76, 16, 16, 76, 16, 16, 76, 16, 16, 76, 16,
        16, 76, 16, 16, 76, 16, 16, 76, 16, 16, 76, 8, 16, 76, 16, 16,
        76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 8, 16, 76, 16, 6, 76,
        16, 16, 76, 8, 16, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 16,
        16, 76, 9, 76, 16, 16, 76, 8, 16, 76, 16, 8, 4, 12, 4, 76,
        16, 16, 76, 16, 6, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 8,
        16, 76, 16, 16, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 16, 16,
        76, 8, 16, 76, 16, 16, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76,
        16, 16, 76, 8, 16, 76, 16, 16, 76, 16, 6, 76, 16, 16, 76, 4,
        10, 4, 76, 16, 16, 76, 16, 6, 76, 16, 16, 76, 8, 16, 76, 16,
        16, 76, 16, 16, 76, 16, 16, 76, 4, 8,
        ],
        bram_minors: &[
        256, 256, 256, 256, 256,
        ],
    },
    // xczu3eg: IDCODE 0x04a42093, 3 FAR rows, 104 std cols (3448 frames/row), 6 bram cols (1536 frames/row), device total 14952 frames.
    PartGeometry { idcode: 0x04a42093, rows: 3,
        std_minors: &[
        16, 76, 16, 16, 76, 16, 6, 76, 16, 16, 76, 8, 16, 76, 16, 16,
        76, 16, 16, 76, 16, 16, 76, 16, 6, 76, 16, 16, 76, 4, 10, 4,
        76, 16, 16, 76, 16, 6, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76,
        16, 16, 76, 16, 16, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 16,
        6, 76, 16, 16, 76, 16, 6, 76, 16, 16, 76, 4, 10, 4, 76, 16,
        16, 76, 8, 16, 76, 16, 16, 76, 8, 16, 76, 16, 16, 76, 16, 6,
        76, 16, 16, 76, 4, 12, 4, 8,
        ],
        bram_minors: &[
        256, 256, 256, 256, 256, 256,
        ],
    },
];

/// Look up an embedded geometry table by masked IDCODE.
fn geometry_for_idcode(idcode: u32) -> Option<&'static PartGeometry> {
    let masked = idcode & 0x0FFF_FFFF;
    PART_GEOMETRIES.iter().find(|g| g.idcode == masked)
}

/// Decode a FAR value into (block type, row, column, minor), UG570
/// table 9-21: reserved [31:27], block type [26:24], row [23:18],
/// column [17:8], minor [7:0].
fn far_decode(far: u32) -> (u8, u8, u16, u8) {
    (
        ((far >> 24) & 0x7) as u8,
        ((far >> 18) & 0x3F) as u8,
        ((far >> 8) & 0x3FF) as u16,
        (far & 0xFF) as u8,
    )
}

fn far_encode(block: u8, row: u8, col: u16, minor: u8) -> u32 {
    ((block as u32) << 24) | ((row as u32) << 18) | ((col as u32) << 8) | minor as u32
}

/// Geometry-aware FAR increment: minor +1, wrapping to the next
/// column at the column height, then the next row, then block 1.
/// Returns `None` when the FAR is outside the geometry or the
/// increment would run past the last frame of block 1.
fn far_increment(geometry: &dyn FarGeometry, far: u32) -> Option<u32> {
    let (block, row, col, minor) = far_decode(far);
    let height = geometry.minor_count(block, row, col)?;
    let mut minor = minor as u16 + 1;
    let mut col = col;
    let mut row = row;
    let mut block = block;
    if minor < height {
        return Some(far_encode(block, row, col, minor as u8));
    }
    minor = 0;
    col += 1;
    let ncols = geometry.column_count(block, row)?;
    if col < ncols {
        return Some(far_encode(block, row, col, minor as u8));
    }
    col = 0;
    row += 1;
    let nrows = geometry.row_count(block)?;
    if row < nrows {
        return Some(far_encode(block, row, col, minor as u8));
    }
    row = 0;
    block += 1;
    if block > BLOCK_BRAM_CONTENT {
        return None;
    }
    Some(far_encode(block, row, col, minor as u8))
}

/// True when this FAR is the last frame of its row: last column of
/// the row and last minor of that column.
fn far_is_last_of_row(geometry: &dyn FarGeometry, far: u32) -> bool {
    let (block, row, col, minor) = far_decode(far);
    match (
        geometry.minor_count(block, row, col),
        geometry.column_count(block, row),
    ) {
        (Some(height), Some(ncols)) => col == ncols - 1 && (minor as u16) == height - 1,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Pass 1: `.bit` header and packet decode
// ---------------------------------------------------------------------------

/// One decoded packet (NOOP packets are dropped by the decoder: they
/// carry no data and only mark time between real packets).
#[derive(Debug, Clone)]
struct Packet {
    /// Opcode: 1 Read, 2 Write. (0 NOP never reaches this struct.)
    opcode: u8,
    /// Target register address (Type 2 inherits the previous Type 1).
    reg: u8,
    /// Payload words, in stream order.
    data: Vec<u32>,
    /// Word index of the packet header within the word stream.
    word_index: usize,
}

/// Items of the decoded stream: packets, plus resync markers where a
/// sync word starts a new SLR section, plus sinkhole markers where a
/// bogus RSVD_30 Type 2 packet ends an SLR section.
#[derive(Debug, Clone)]
enum StreamItem {
    Packet(Packet),
    /// A sync word found at a packet boundary, at this word index.
    Resync(usize),
    /// A sinkhole Type 2 packet (register RSVD_30, bogus word count,
    /// no payload). It ends an SLR section without a DESYNC; the next
    /// sync word starts the following section.
    Sinkhole,
}

/// Strip a `.bit` TLV header if present, returning the raw word stream.
/// Format per BitFiltrator `bitstream_spec.py` and `bitgen.rs`
/// `strip_bit_header`: field 1 is 2-byte length 0x0009 plus 9 bytes
/// starting 0x0ff0..., then key `a`, design name, then TLV fields `b`
/// (part), `c` (date), `d` (time), `e` (4-byte length plus raw data).
/// Returns the input unchanged when there is no TLV header.
fn strip_bit_header(bytes: &[u8]) -> Result<&[u8], String> {
    if bytes.len() < 13 || bytes[0] != 0x00 || bytes[1] != 0x09 {
        return Ok(bytes);
    }
    if bytes[2] != 0x0F || bytes[3] != 0xF0 {
        return Ok(bytes);
    }
    // Field 1 value is 9 bytes (bytes 2..11), then field 2 length
    // (2 bytes) and key `a` (1 byte).
    let mut i = 11;
    if i + 3 > bytes.len() || bytes[i + 2] != 0x61 {
        return Err(format!("bad .bit header at byte {i}: expected `a` field"));
    }
    i += 3;
    // Field 3: 2-byte length, design name plus NUL.
    if i + 2 > bytes.len() {
        return Err(format!(
            "bad .bit header at byte {i}: truncated design field"
        ));
    }
    let len3 = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
    i += 2 + len3;
    // TLV fields until `e`.
    loop {
        if i + 1 > bytes.len() {
            return Err(format!(
                "bad .bit header at byte {i}: ran out of fields before `e`"
            ));
        }
        let tag = bytes[i];
        i += 1;
        if tag == 0x65 {
            if i + 4 > bytes.len() {
                return Err(format!("bad .bit header at byte {i}: truncated `e` length"));
            }
            let len =
                u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
            i += 4;
            if i + len > bytes.len() {
                return Err(format!(
                    "bad .bit header at byte {i}: `e` length {len} exceeds file"
                ));
            }
            return Ok(&bytes[i..i + len]);
        }
        if i + 2 > bytes.len() {
            return Err(format!("bad .bit header at byte {i}: truncated TLV length"));
        }
        let len = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
        i += 2 + len;
    }
}

/// Decode the word stream after the sync word into packets (pass 1).
/// Dummy words, bus-width patterns, and whole-frame zero runs between
/// packets are skipped; NOOP packets are dropped; a sync word at a
/// packet boundary becomes a `Resync` marker. Every other word must
/// be a well-formed Type 1 or Type 2 header with its payload present,
/// or this returns an error naming the word index.
fn decode_packets(words: &[u32]) -> Result<Vec<StreamItem>, String> {
    let sync_pos = words
        .iter()
        .position(|w| *w == SYNC_WORD)
        .ok_or_else(|| "no sync word (0xAA995566) found in bitstream".to_string())?;
    // Words before sync must be dummy words or bus-width patterns.
    for (idx, w) in words[..sync_pos].iter().enumerate() {
        if *w != DUMMY_WORD && *w != BUS_WIDTH_DETECT && *w != BUS_WIDTH_SYNC {
            return Err(format!(
                "unexpected word 0x{w:08x} before sync word at word {idx}"
            ));
        }
    }
    let mut items = Vec::new();
    let mut i = sync_pos + 1;
    let mut prev_reg: Option<u8> = None;
    while i < words.len() {
        let header = words[i];
        if header == DUMMY_WORD || header == BUS_WIDTH_DETECT || header == BUS_WIDTH_SYNC {
            i += 1;
            continue;
        }
        if header == SYNC_WORD {
            items.push(StreamItem::Resync(i));
            prev_reg = None;
            i += 1;
            continue;
        }
        if header == 0 {
            // Zero-word run between packets: end-of-row padding emitted
            // as raw words (BitFiltrator). It must be a whole number of
            // frames, or the stream is corrupt.
            let mut j = i;
            while j < words.len() && words[j] == 0 {
                j += 1;
            }
            let run = j - i;
            if run % FRAME_WORDS != 0 {
                return Err(format!(
                    "zero-word run of {run} words at word {i} is not a whole number of {FRAME_WORDS}-word frames"
                ));
            }
            i = j;
            continue;
        }
        let ptype = header >> 29;
        if ptype != 1 && ptype != 2 {
            return Err(format!(
                "bad packet header 0x{header:08x} at word {i}: type {ptype} is not Type 1 or Type 2"
            ));
        }
        let opcode = ((header >> 27) & 0x3) as u8;
        if opcode == 3 {
            return Err(format!(
                "reserved opcode 3 in packet header 0x{header:08x} at word {i}"
            ));
        }
        if ptype == 1 {
            // Type 1 (UG570 table 9-16): type [31:29], opcode [28:27],
            // register address [26:13], reserved [12:11], count [10:0].
            let reg_field = (header >> 13) & 0x3FFF;
            if reg_field > 31 {
                return Err(format!(
                    "packet header 0x{header:08x} at word {i}: register field {reg_field} exceeds 31"
                ));
            }
            let reg = reg_field as u8;
            let reserved = (header >> 11) & 0x3;
            if reserved != 0 {
                return Err(format!(
                    "packet header 0x{header:08x} at word {i}: reserved bits are 0x{reserved:x}, expected 0"
                ));
            }
            let count = (header & 0x7FF) as usize;
            if opcode == 0 {
                // NOOP must be exactly 0x20000000: register 0, count 0.
                if reg != 0 || count != 0 {
                    return Err(format!(
                        "malformed NOOP packet 0x{header:08x} at word {i}: expected register 0 and count 0"
                    ));
                }
                prev_reg = Some(reg);
                i += 1;
                continue;
            }
            if i + 1 + count > words.len() {
                return Err(format!(
                    "truncated packet at word {i}: header declares {count} words but only {} remain",
                    words.len() - i - 1
                ));
            }
            let data = words[i + 1..i + 1 + count].to_vec();
            prev_reg = Some(reg);
            items.push(StreamItem::Packet(Packet {
                opcode,
                reg,
                data,
                word_index: i,
            }));
            i += 1 + count;
        } else {
            // Type 2 (UG570 table 9-18): type [31:29], opcode [28:27],
            // word count [26:0]. The register comes from the previous
            // Type 1 packet.
            let count = (header & 0x07FF_FFFF) as usize;
            let reg = match prev_reg {
                Some(r) => r,
                None => {
                    return Err(format!(
                        "Type 2 packet 0x{header:08x} at word {i} has no preceding Type 1 packet"
                    ));
                }
            };
            if reg == REG_RSVD_30 {
                // Sinkhole (BitFiltrator): a write to RSVD_30 followed
                // by a Type 2 packet whose huge word count is bogus and
                // carries no payload; it marks the end of an SLR. The
                // header is consumed and no payload words are read.
                items.push(StreamItem::Sinkhole);
                i += 1;
                continue;
            }
            if reg != REG_FDRI {
                return Err(format!(
                    "Type 2 packet at word {i} targets register {reg}, only FDRI uses Type 2 packets"
                ));
            }
            if i + 1 + count > words.len() {
                return Err(format!(
                    "truncated Type 2 packet at word {i}: header declares {count} words but only {} remain",
                    words.len() - i - 1
                ));
            }
            let data = words[i + 1..i + 1 + count].to_vec();
            items.push(StreamItem::Packet(Packet {
                opcode,
                reg,
                data,
                word_index: i,
            }));
            i += 1 + count;
        }
    }
    Ok(items)
}

// ---------------------------------------------------------------------------
// Pass 2: walk packets into frames
// ---------------------------------------------------------------------------

/// Per-section walk state. A section starts at a sync word and ends at
/// DESYNC; a resync resets everything except the collected frames.
struct WalkState<'g> {
    /// Caller-supplied geometry (trait path), if any. Applies to every
    /// section; `None` on the bytes-only path.
    geometry_override: Option<&'g dyn FarGeometry>,
    /// Geometry for the current section: the override, or the embedded
    /// table resolved from the section IDCODE.
    geometry: Option<&'g dyn FarGeometry>,
    /// Current FAR value, if a FAR write has been seen in this section.
    far: Option<u32>,
    /// IDCODE written in this section, if any.
    idcode: Option<u32>,
    /// Last command value written to CMD in this section, if any.
    cmd: Option<u32>,
    /// Last completed FDRI frame (93 words), the MFWR replication
    /// source. Set by every FDRI frame written.
    buf: Option<Vec<u32>>,
    /// Frames collected so far, last write wins per FAR.
    frames: BTreeMap<u32, Vec<u32>>,
    /// True once any packet or resync has been seen.
    section_seen: bool,
    /// True when the current section ended with DESYNC.
    desync_seen: bool,
    /// True when a sinkhole packet was seen in the current section.
    /// A sinkhole ends an SLR section without a DESYNC, so a resync
    /// after a sinkhole is legitimate.
    sinkhole_seen: bool,
    /// FAR of the last frame of the most recent FDRI packet, awaiting
    /// the MFWR run-start anchor check.
    expect_anchor: Option<u32>,
    /// True when an MFW command was written since the last FDRI packet.
    mfw_armed: bool,
    /// True once the run-start anchor has been verified for the
    /// current MFW arming.
    anchor_checked: bool,
    /// FAR of the most recent FAR write in this section, if any.
    last_far_written: Option<u32>,
    /// True when the stream contains any MFW command. Compressed
    /// streams use MFW and carry no end-of-row padding; uncompressed
    /// streams carry 2 zero padding frames after every row end.
    mfw_present: bool,
    /// Running configuration CRC (CRC-32C, prjxray `icap_crc`). One
    /// value for the whole file, not reset by a resync: RCRC resets
    /// it, a CRC-register write checks it and then resets it, and
    /// every other non-NOP write packet feeds it. Starts at 0.
    crc: u32,
}

fn is_known_command(value: u32) -> bool {
    matches!(
        value,
        0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 13 | 15 | 16 | 17 | 18 | 19
    )
}

/// Walk the decoded stream items into frames.
fn walk_items(
    items: &[StreamItem],
    geometry_override: Option<&dyn FarGeometry>,
) -> Result<Vec<Frame>, String> {
    let mfw_present = items.iter().any(|item| match item {
        StreamItem::Packet(p) => {
            p.opcode == 2 && p.reg == REG_CMD && p.data.first() == Some(&CMD_MFW)
        }
        StreamItem::Resync(_) | StreamItem::Sinkhole => false,
    });
    let mut st = WalkState {
        geometry_override,
        geometry: geometry_override,
        far: None,
        idcode: None,
        cmd: None,
        buf: None,
        frames: BTreeMap::new(),
        section_seen: false,
        desync_seen: false,
        sinkhole_seen: false,
        expect_anchor: None,
        mfw_armed: false,
        anchor_checked: false,
        last_far_written: None,
        mfw_present,
        crc: 0,
    };
    for item in items {
        match item {
            StreamItem::Resync(word_index) => {
                if st.section_seen && !st.desync_seen && !st.sinkhole_seen {
                    return Err(format!(
                        "resync at word {word_index} without a preceding DESYNC or sinkhole: section is unterminated"
                    ));
                }
                st.section_seen = true;
                st.desync_seen = false;
                st.sinkhole_seen = false;
                st.geometry = st.geometry_override;
                st.far = None;
                st.idcode = None;
                st.cmd = None;
                st.buf = None;
                st.expect_anchor = None;
                st.mfw_armed = false;
                st.anchor_checked = false;
                st.last_far_written = None;
            }
            StreamItem::Sinkhole => {
                st.section_seen = true;
                st.sinkhole_seen = true;
            }
            StreamItem::Packet(packet) => {
                if st.desync_seen {
                    return Err(format!(
                        "packet at word {} after DESYNC without a resync",
                        packet.word_index
                    ));
                }
                st.section_seen = true;
                if packet.opcode == 1 {
                    // Reads carry no frame data and never feed the
                    // CRC; their payload words were consumed as
                    // packet data in pass 1.
                    continue;
                }
                // Opcode 2 (Write) from here on; opcode 0 never
                // reaches this walk.
                //
                // Configuration CRC tracking (CRC-32C, prjxray
                // `icap_crc`, same model as `bitgen.rs` `CrcState`):
                // every non-NOP write packet feeds each data word as
                // (register address, word), except the RCRC command,
                // which resets the running CRC without feeding, and
                // a CRC-register write, which is checked against the
                // running CRC in its own arm below and then resets
                // it. NOOPs never reach this walk (dropped in pass 1).
                let is_crc_write = packet.reg == REG_CRC;
                let is_rcrc = packet.reg == REG_CMD
                    && packet.data.first().copied().unwrap_or(0) & 0x1F == CMD_RCRC;
                if !is_crc_write && !is_rcrc {
                    for word in &packet.data {
                        st.crc = crc32c_step(packet.reg as u32, *word, st.crc);
                    }
                }
                match packet.reg {
                    REG_FAR => {
                        if packet.data.len() != 1 {
                            return Err(format!(
                                "FAR write at word {} carries {} words, expected exactly 1",
                                packet.word_index,
                                packet.data.len()
                            ));
                        }
                        let new_far = packet.data[0];
                        if (new_far >> 27) != 0 {
                            return Err(format!(
                                "FAR write 0x{new_far:08x} at word {} has nonzero reserved bits [31:27]",
                                packet.word_index
                            ));
                        }
                        let (block, _, _, _) = far_decode(new_far);
                        if block != BLOCK_CLB_IO_CLK
                            && block != BLOCK_BRAM_CONTENT
                            && block != BLOCK_SENTINEL
                        {
                            return Err(format!(
                                "FAR write 0x{new_far:08x} at word {} has unknown block type {block}",
                                packet.word_index
                            ));
                        }
                        if block != BLOCK_SENTINEL && st.cmd == Some(CMD_MFW) {
                            // The MFW command executes each time FAR is
                            // loaded (UG570): replicate the buffered
                            // frame to the newly loaded FAR.
                            let buf = st.buf.as_ref().ok_or_else(|| {
                                format!(
                                    "MFW replication at word {} has no buffered FDRI frame",
                                    packet.word_index
                                )
                            })?;
                            st.frames.insert(new_far, buf.clone());
                        }
                        st.far = Some(new_far);
                        st.last_far_written = Some(new_far);
                    }
                    REG_CMD => {
                        if packet.data.len() != 1 {
                            return Err(format!(
                                "CMD write at word {} carries {} words, expected exactly 1",
                                packet.word_index,
                                packet.data.len()
                            ));
                        }
                        let value = packet.data[0];
                        if !is_known_command(value) {
                            return Err(format!(
                                "unknown command code {value} in CMD write at word {}",
                                packet.word_index
                            ));
                        }
                        st.cmd = Some(value);
                        if value & 0x1F == CMD_RCRC {
                            // RCRC resets the running CRC without
                            // feeding (the RCRC command word itself
                            // is not CRC-covered; the pre-loop feed
                            // already skipped it for this reason).
                            st.crc = 0;
                        }
                        if value == CMD_MFW {
                            st.mfw_armed = true;
                            st.anchor_checked = false;
                        }
                        if value == CMD_DESYNC {
                            st.desync_seen = true;
                        }
                    }
                    REG_IDCODE => {
                        if packet.data.len() != 1 {
                            return Err(format!(
                                "IDCODE write at word {} carries {} words, expected exactly 1",
                                packet.word_index,
                                packet.data.len()
                            ));
                        }
                        let value = packet.data[0];
                        if let Some(prev) = st.idcode {
                            if prev != value {
                                return Err(format!(
                                    "conflicting IDCODE writes in one section: 0x{prev:08x} then 0x{value:08x} at word {}",
                                    packet.word_index
                                ));
                            }
                        }
                        st.idcode = Some(value);
                        if st.geometry_override.is_none() {
                            st.geometry = geometry_for_idcode(value).map(|g| g as &dyn FarGeometry);
                        }
                    }
                    REG_FDRI => {
                        if packet.data.is_empty() {
                            // Empty Type 1 placeholder for a following
                            // long Type 2 packet (BitFiltrator, UG570
                            // KU040 sample). It carries no frame data.
                            continue;
                        }
                        let start_far = st.far.ok_or_else(|| {
                            format!(
                                "FDRI write at word {} has no prior FAR write in this section",
                                packet.word_index
                            )
                        })?;
                        if st.idcode.is_none() {
                            return Err(format!(
                                "FDRI write at word {} has no prior IDCODE write in this section",
                                packet.word_index
                            ));
                        }
                        if packet.data.len() % FRAME_WORDS != 0 {
                            return Err(format!(
                                "FDRI write at word {} carries {} words, not a nonzero multiple of {FRAME_WORDS}",
                                packet.word_index,
                                packet.data.len()
                            ));
                        }
                        let geometry = st.geometry.ok_or_else(|| {
                            format!(
                                "no FAR geometry table for IDCODE 0x{:08x}; use parse_bitstream_with_geometry with caller-supplied geometry",
                                st.idcode.unwrap_or(0)
                            )
                        })?;
                        let nframes = packet.data.len() / FRAME_WORDS;
                        let mut cur = start_far;
                        let mut last_emitted = start_far;
                        let mut idx = 0;
                        while idx < nframes {
                            let chunk =
                                packet.data[idx * FRAME_WORDS..(idx + 1) * FRAME_WORDS].to_vec();
                            st.frames.insert(cur, chunk.clone());
                            st.buf = Some(chunk);
                            last_emitted = cur;
                            idx += 1;
                            if idx >= nframes {
                                break;
                            }
                            // End-of-row padding, uncompressed streams
                            // only: after the last frame of a row, the
                            // next 2 frames in this packet are zero pad
                            // frames, not real frames. They are skipped
                            // (and asserted zero); the FAR does not
                            // advance for them.
                            if !st.mfw_present
                                && far_is_last_of_row(geometry, cur)
                                && idx + 1 < nframes
                            {
                                let next = &packet.data[idx * FRAME_WORDS..(idx + 1) * FRAME_WORDS];
                                let next2 =
                                    &packet.data[(idx + 1) * FRAME_WORDS..(idx + 2) * FRAME_WORDS];
                                if next.iter().all(|w| *w == 0) && next2.iter().all(|w| *w == 0) {
                                    idx += 2;
                                    if idx >= nframes {
                                        break;
                                    }
                                }
                            }
                            cur = far_increment(geometry, cur).ok_or_else(|| {
                                format!(
                                    "FAR increment past the device end at FAR 0x{cur:08x} in FDRI write at word {}, frame {idx}/{nframes}",
                                    packet.word_index
                                )
                            })?;
                        }
                        // Track the FAR after the last frame for any
                        // following FDRI packet without a FAR rewrite;
                        // None when the packet ended exactly at the
                        // device end.
                        st.far = far_increment(geometry, last_emitted);
                        st.expect_anchor = Some(last_emitted);
                        st.mfw_armed = false;
                    }
                    REG_MFWR => {
                        if packet.data.iter().any(|w| *w != 0) {
                            return Err(format!(
                                "MFWR write at word {} carries nonzero dummy words; MFWR payloads must be all zero",
                                packet.word_index
                            ));
                        }
                        if st.mfw_armed && !st.anchor_checked {
                            let anchor = st.expect_anchor.ok_or_else(|| {
                                format!(
                                    "MFW run at word {} has no preceding FDRI frame to replicate",
                                    packet.word_index
                                )
                            })?;
                            let run_start = st.last_far_written.ok_or_else(|| {
                                format!(
                                    "MFWR write at word {} has no preceding FAR write",
                                    packet.word_index
                                )
                            })?;
                            if run_start != anchor {
                                return Err(format!(
                                    "MFWR run-start anchor mismatch: last FDRI frame is at FAR 0x{anchor:08x} but the run starts at FAR 0x{run_start:08x} (word {})",
                                    packet.word_index
                                ));
                            }
                            st.anchor_checked = true;
                        }
                    }
                    REG_CRC => {
                        if packet.data.len() != 1 {
                            return Err(format!(
                                "CRC write at word {} carries {} words, expected exactly 1",
                                packet.word_index,
                                packet.data.len()
                            ));
                        }
                        // The CRC register carries the running CRC of
                        // every write packet since the last RCRC or
                        // CRC write; the device compares and then
                        // resets. A mismatch means the payload (in
                        // practice FDRI frame data) was corrupted
                        // after Vivado wrote it, so decoding on would
                        // produce wrong FASM silently. Fail loud
                        // instead, naming the CRC packet word index
                        // and both values: the corrupted word is in a
                        // packet before this CRC write.
                        let found = packet.data[0];
                        if found != st.crc {
                            return Err(format!(
                                "CRC mismatch at word {}: CRC register carries 0x{found:08x} but the running CRC is 0x{:08x}",
                                packet.word_index, st.crc
                            ));
                        }
                        st.crc = 0;
                    }
                    _ => {
                        // Every other register (CTL0, MASK, COR0, COR1,
                        // TIMER, WBSTAR, CTL1, STAT, LOUT, CBC, AXSS,
                        // BOOTSTS, BSPI, FDRO, and the undocumented
                        // addresses including register 19): payload
                        // consumed as a known packet, no frame effect.
                    }
                }
            }
        }
    }
    if !st.section_seen {
        return Err("no configuration section found in bitstream".to_string());
    }
    if !st.desync_seen {
        return Err("unterminated section: end of stream without DESYNC".to_string());
    }
    if st.frames.is_empty() {
        return Err("no FDRI frames found in bitstream".to_string());
    }
    Ok(st
        .frames
        .into_iter()
        .map(|(far, words)| Frame { far, words })
        .collect())
}

/// Shared entry point for both public parse functions.
fn parse_inner(
    bytes: &[u8],
    geometry_override: Option<&dyn FarGeometry>,
) -> Result<Vec<Frame>, String> {
    let raw = strip_bit_header(bytes)?;
    if raw.is_empty() || raw.len() % 4 != 0 {
        return Err(format!(
            "bitstream data is {} bytes, not a whole number of 32-bit words",
            raw.len()
        ));
    }
    let words: Vec<u32> = raw
        .chunks_exact(4)
        .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let items = decode_packets(&words)?;
    walk_items(&items, geometry_override)
}

// ---------------------------------------------------------------------------
// Unit tests (synthetic packet streams; no hardware, no files)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// XCKU3P IDCODE (embedded table), used by most synthetic streams.
    const TEST_IDCODE: u32 = 0x04A6_3093;

    fn t1(opcode: u32, reg: u8, count: u32) -> u32 {
        (1 << 29) | (opcode << 27) | ((reg as u32) << 13) | count
    }

    fn t2(opcode: u32, count: u32) -> u32 {
        (2 << 29) | (opcode << 27) | count
    }

    fn far(block: u8, row: u8, col: u16, minor: u8) -> u32 {
        far_encode(block, row, col, minor)
    }

    /// Minimal `.bit` header wrapping raw word-stream bytes.
    fn bit_header(data_len: usize) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x00, 0x09]);
        out.extend_from_slice(&[0x0F, 0xF0, 0x0F, 0xF0, 0x0F, 0xF0, 0x0F, 0xF0, 0x00]);
        out.extend_from_slice(&[0x00, 0x01, 0x61]);
        let design = b"test;COMPRESS=FALSE\x00";
        out.extend_from_slice(&(design.len() as u16).to_be_bytes());
        out.extend_from_slice(design);
        for (tag, value) in [
            (0x62u8, b"xcku3p-ffvb676-2-e\x00".as_slice()),
            (0x63u8, b"2026/11/19\x00".as_slice()),
            (0x64u8, b"00:00:00\x00".as_slice()),
        ] {
            out.push(tag);
            out.extend_from_slice(&(value.len() as u16).to_be_bytes());
            out.extend_from_slice(value);
        }
        out.push(0x65);
        out.extend_from_slice(&(data_len as u32).to_be_bytes());
        out
    }

    fn words_to_bytes(words: &[u32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(words.len() * 4);
        for w in words {
            out.extend_from_slice(&w.to_be_bytes());
        }
        out
    }

    /// Preamble words: dummies, bus-width patterns, sync, NOOP, RCRC,
    /// IDCODE. Every synthetic stream starts with these.
    fn preamble() -> Vec<u32> {
        let mut w = vec![DUMMY_WORD; 8];
        w.push(BUS_WIDTH_DETECT);
        w.push(BUS_WIDTH_SYNC);
        w.extend([DUMMY_WORD; 2]);
        w.push(SYNC_WORD);
        w.push(NOOP_WORD);
        w.push(t1(2, REG_CMD, 1));
        w.push(7); // RCRC
        w.push(t1(2, REG_IDCODE, 1));
        w.push(TEST_IDCODE);
        w
    }

    fn write_reg(words: &mut Vec<u32>, reg: u8, data: &[u32]) {
        words.push(t1(2, reg, data.len() as u32));
        words.extend_from_slice(data);
    }

    fn write_cmd(words: &mut Vec<u32>, cmd: u32) {
        write_reg(words, REG_CMD, &[cmd]);
    }

    fn ending(words: &mut Vec<u32>) {
        write_cmd(words, CMD_DESYNC);
        words.extend([NOOP_WORD; 400]);
    }

    /// One 93-word frame whose words are all `marker`, except word 0
    /// which holds `tag` so frames are distinguishable.
    fn frame_words(tag: u32, marker: u32) -> Vec<u32> {
        let mut w = vec![marker; FRAME_WORDS];
        w[0] = tag;
        w
    }

    fn parse(words: &[u32]) -> Result<Vec<Frame>, String> {
        parse_bitstream(&words_to_bytes(words))
    }

    fn parse_with_header(words: &[u32]) -> Result<Vec<Frame>, String> {
        let data = words_to_bytes(words);
        let mut bytes = bit_header(data.len());
        bytes.extend_from_slice(&data);
        parse_bitstream(&bytes)
    }

    #[test]
    fn frame_word_count_is_93() {
        assert_eq!(frame_word_count(), 93);
    }

    #[test]
    fn minimal_valid_stream_with_bit_header() {
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[0]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &frame_words(0xA11CE, 0x1111_1111));
        ending(&mut w);
        let frames = parse_with_header(&w).expect("minimal stream must parse");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].far, 0);
        assert_eq!(frames[0].words.len(), 93);
        assert_eq!(frames[0].words[0], 0xA11CE);
        assert!(frames[0].words[1..].iter().all(|x| *x == 0x1111_1111));
    }

    #[test]
    fn raw_stream_without_header_parses_the_same() {
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[0]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &frame_words(0xB00, 0x2222_2222));
        ending(&mut w);
        let frames = parse(&w).expect("raw stream must parse");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].far, 0);
        assert_eq!(frames[0].words[0], 0xB00);
    }

    #[test]
    fn truncated_stream_fails_loud() {
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[0]);
        write_cmd(&mut w, CMD_WCFG);
        // FDRI header declares 93 words; only 10 follow, then EOF.
        w.push(t1(2, REG_FDRI, 93));
        w.extend([0xDEAD_BEEF; 10]);
        let err = parse(&w).expect_err("truncated FDRI must fail");
        assert!(err.contains("truncated"), "unexpected error: {err}");
    }

    #[test]
    fn far_change_mid_stream() {
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[0]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &frame_words(0xF001, 0x0101_0101));
        write_reg(&mut w, REG_FAR, &[0x100]);
        write_reg(&mut w, REG_FDRI, &frame_words(0xF002, 0x0202_0202));
        ending(&mut w);
        let frames = parse(&w).expect("FAR change stream must parse");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].far, 0);
        assert_eq!(frames[0].words[0], 0xF001);
        assert_eq!(frames[1].far, 0x100);
        assert_eq!(frames[1].words[0], 0xF002);
    }

    #[test]
    fn mfwr_replication_copies_last_fdri_frame() {
        let seed = frame_words(0x5EED, 0x0BAD_F00D);
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[0]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &seed);
        write_cmd(&mut w, CMD_MFW);
        // Run: FAR 0 (repeat of the seed itself), FAR 1, FAR 0x100.
        for target in [0u32, 1, 0x100] {
            write_reg(&mut w, REG_FAR, &[target]);
            write_reg(&mut w, REG_MFWR, &[0; 14]);
        }
        ending(&mut w);
        let frames = parse(&w).expect("MFWR stream must parse");
        assert_eq!(frames.len(), 3);
        for (far, frame) in [(0u32, &frames[0]), (1, &frames[1]), (0x100, &frames[2])] {
            assert_eq!(frame.far, far);
            assert_eq!(frame.words, seed);
        }
    }

    #[test]
    fn mfwr_run_anchor_mismatch_fails_loud() {
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[0]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &frame_words(0x1, 0));
        write_cmd(&mut w, CMD_MFW);
        // Run starts at FAR 0x50, but the last FDRI frame is at FAR 0.
        write_reg(&mut w, REG_FAR, &[0x50]);
        write_reg(&mut w, REG_MFWR, &[0; 14]);
        ending(&mut w);
        let err = parse(&w).expect_err("anchor mismatch must fail");
        assert!(err.contains("anchor"), "unexpected error: {err}");
    }

    #[test]
    fn geometry_wrap_across_column_boundary() {
        // xcku3p row 1, column 199 has 76 minors (0..75). Three frames
        // from minor 74 must land at (199,74), (199,75), (200,0),
        // which plain integer +1 would get wrong (it predicts
        // (199,76) for the third frame).
        let mut payload = Vec::new();
        for tag in [0xC1u32, 0xC2, 0xC3] {
            payload.extend(frame_words(tag, 0));
        }
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[far(0, 1, 199, 74)]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &payload);
        ending(&mut w);
        let frames = parse(&w).expect("column wrap stream must parse");
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].far, far(0, 1, 199, 74));
        assert_eq!(frames[1].far, far(0, 1, 199, 75));
        assert_eq!(frames[2].far, far(0, 1, 200, 0));
        assert_eq!(frames[2].words[0], 0xC3);
    }

    #[test]
    fn unknown_opcode_fails_loud() {
        let mut w = preamble();
        w.push((1 << 29) | (3 << 27)); // Type 1, opcode 3 (reserved)
        let err = parse(&w).expect_err("reserved opcode must fail");
        assert!(err.contains("opcode"), "unexpected error: {err}");
    }

    #[test]
    fn fdri_wrong_word_count_fails_loud() {
        // 100 words is not a multiple of 93; 101 is the 7-series size.
        for count in [100usize, 101, 1] {
            let mut w = preamble();
            write_reg(&mut w, REG_FAR, &[0]);
            write_cmd(&mut w, CMD_WCFG);
            w.push(t1(2, REG_FDRI, count as u32));
            w.extend(vec![0u32; count]);
            ending(&mut w);
            let err = parse(&w).expect_err("bad FDRI count must fail");
            assert!(
                err.contains("multiple of 93"),
                "count {count}: unexpected error: {err}"
            );
        }
    }

    #[test]
    fn missing_sync_fails_loud() {
        let w = vec![DUMMY_WORD; 32];
        let err = parse(&w).expect_err("missing sync must fail");
        assert!(err.contains("sync"), "unexpected error: {err}");
    }

    #[test]
    fn packet_after_desync_without_resync_fails_loud() {
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[0]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &frame_words(0x1, 0));
        write_cmd(&mut w, CMD_DESYNC);
        write_reg(&mut w, REG_FAR, &[0x200]);
        let err = parse(&w).expect_err("packet after DESYNC must fail");
        assert!(err.contains("DESYNC"), "unexpected error: {err}");
    }

    #[test]
    fn mfwr_nonzero_payload_fails_loud() {
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[0]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &frame_words(0x1, 0));
        write_cmd(&mut w, CMD_MFW);
        write_reg(&mut w, REG_FAR, &[0]);
        let mut payload = [0u32; 14];
        payload[3] = 0x1234_5678;
        write_reg(&mut w, REG_MFWR, &payload);
        ending(&mut w);
        let err = parse(&w).expect_err("nonzero MFWR payload must fail");
        assert!(err.contains("MFWR"), "unexpected error: {err}");
    }

    #[test]
    fn type2_without_type1_fails_loud() {
        // A Type 2 header as the very first packet after sync: no
        // preceding Type 1 exists to supply its register.
        let mut w = vec![DUMMY_WORD; 4];
        w.push(SYNC_WORD);
        w.push(t2(2, 10));
        w.extend([0u32; 10]);
        let err = parse(&w).expect_err("orphan Type 2 must fail");
        assert!(err.contains("Type 2"), "unexpected error: {err}");
    }

    #[test]
    fn type2_for_non_fdri_register_fails_loud() {
        // A Type 2 header following a Type 1 IDCODE write: Type 2 is
        // only valid for FDRI payloads (and the RSVD_30 sinkhole).
        let mut w = preamble();
        w.push(t2(2, 10));
        w.extend([0u32; 10]);
        let err = parse(&w).expect_err("non-FDRI Type 2 must fail");
        assert!(err.contains("Type 2"), "unexpected error: {err}");
    }

    #[test]
    fn unknown_idcode_fails_loud_without_geometry() {
        let mut w = preamble();
        // Replace the test IDCODE with an unknown one.
        let pos = w.iter().position(|x| *x == TEST_IDCODE).unwrap();
        w[pos] = 0xDEAD_BEEF;
        write_reg(&mut w, REG_FAR, &[0]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &frame_words(0x1, 0));
        ending(&mut w);
        let err = parse(&w).expect_err("unknown IDCODE must fail");
        assert!(err.contains("geometry"), "unexpected error: {err}");
    }

    #[test]
    fn resync_starts_new_section() {
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[0]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &frame_words(0x51, 0));
        write_cmd(&mut w, CMD_DESYNC);
        w.extend([DUMMY_WORD; 4]);
        w.push(SYNC_WORD);
        w.push(NOOP_WORD);
        write_cmd(&mut w, 7); // RCRC
        write_reg(&mut w, REG_IDCODE, &[TEST_IDCODE]);
        write_reg(&mut w, REG_FAR, &[0x200]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &frame_words(0x52, 0));
        ending(&mut w);
        let frames = parse(&w).expect("two-section stream must parse");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].far, 0);
        assert_eq!(frames[0].words[0], 0x51);
        assert_eq!(frames[1].far, 0x200);
        assert_eq!(frames[1].words[0], 0x52);
    }

    #[test]
    fn sinkhole_resync_without_desync() {
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[0]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &frame_words(0x61, 0));
        // SLR boundary: RSVD_30 write, then a bogus Type 2 packet.
        write_reg(&mut w, REG_RSVD_30, &[]);
        w.push(t2(2, 0x07FF_FFFF));
        w.extend([DUMMY_WORD; 4]);
        w.push(BUS_WIDTH_DETECT);
        w.push(BUS_WIDTH_SYNC);
        w.extend([DUMMY_WORD; 2]);
        w.push(SYNC_WORD);
        w.push(NOOP_WORD);
        write_cmd(&mut w, 7); // RCRC
        write_reg(&mut w, REG_IDCODE, &[TEST_IDCODE]);
        write_reg(&mut w, REG_FAR, &[0x200]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &frame_words(0x62, 0));
        ending(&mut w);
        let frames = parse(&w).expect("sinkhole stream must parse");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].far, 0);
        assert_eq!(frames[1].far, 0x200);
        assert_eq!(frames[1].words[0], 0x62);
    }

    #[test]
    fn unterminated_section_fails_loud() {
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[0]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &frame_words(0x1, 0));
        // No DESYNC, no trailing NOOPs.
        let err = parse(&w).expect_err("unterminated stream must fail");
        assert!(err.contains("DESYNC"), "unexpected error: {err}");
    }

    #[test]
    fn type2_fdri_after_empty_type1() {
        // The long-payload pattern: Type 1 FDRI with count 0, then a
        // Type 2 FDRI carrying the frames. This is how Vivado emits
        // large FDRI payloads (and the KU040 sample in UG570).
        let mut payload = Vec::new();
        payload.extend(frame_words(0x71, 0));
        payload.extend(frame_words(0x72, 0));
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[0x300]);
        write_cmd(&mut w, CMD_WCFG);
        w.push(t1(2, REG_FDRI, 0));
        w.push(t2(2, payload.len() as u32));
        w.extend_from_slice(&payload);
        ending(&mut w);
        let frames = parse(&w).expect("Type 2 FDRI must parse");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].far, 0x300);
        assert_eq!(frames[0].words[0], 0x71);
        assert_eq!(frames[1].far, 0x301);
        assert_eq!(frames[1].words[0], 0x72);
    }

    #[test]
    fn sentinel_far_block_7_is_accepted() {
        // Vivado writes a block-7 FAR after START (0x07FC0000 in the
        // uncompressed corpus files). It addresses no frame and must
        // not break the parse.
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[0]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &frame_words(0x1, 0));
        write_cmd(&mut w, 5); // START
        write_reg(&mut w, REG_FAR, &[0x07FC_0000]);
        ending(&mut w);
        let frames = parse(&w).expect("sentinel FAR must parse");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].far, 0);
    }

    #[test]
    fn duplicate_far_keeps_last_write() {
        // The MFWR run repeats the seed frame at its own FAR, and a
        // group-start FAR load under a stale MFW replicates before the
        // group's FDRI overwrites it. Both produce duplicate FARs and
        // the last write must win.
        let seed = frame_words(0x5EED, 0x0BAD_F00D);
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[0x400]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &seed);
        write_cmd(&mut w, CMD_MFW);
        write_reg(&mut w, REG_FAR, &[0x400]);
        write_reg(&mut w, REG_MFWR, &[0; 14]);
        // Next group reuses FAR 0x400 under the stale MFW, then its
        // FDRI overwrites it with new data.
        write_reg(&mut w, REG_FAR, &[0x400]);
        write_cmd(&mut w, CMD_WCFG);
        let replacement = frame_words(0x9E99, 0x3333_3333);
        write_reg(&mut w, REG_FDRI, &replacement);
        ending(&mut w);
        let frames = parse(&w).expect("duplicate FAR stream must parse");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].far, 0x400);
        assert_eq!(frames[0].words, replacement);
    }

    /// Fold one register write into a running CRC, following the feed
    /// rule the parser checks: every write feeds each data word as
    /// (address, word). Callers handle RCRC (reset) and CRC writes
    /// (check, then reset) themselves.
    fn feed_crc(mut crc: u32, reg: u8, data: &[u32]) -> u32 {
        for word in data {
            crc = crc32c_step(reg as u32, *word, crc);
        }
        crc
    }

    /// Build a minimal valid stream with a CRC write after the frame
    /// data, without the DESYNC ending (callers append `ending`, or
    /// more packets first). Preamble (RCRC, IDCODE), WCFG, FAR 0, one
    /// FDRI frame, FAR rewrite, CRC carrying the running CRC.
    /// Returns the word vector and the index of the first FDRI payload
    /// word, so the corruption test can flip it.
    fn stream_with_crc(frame: &[u32]) -> (Vec<u32>, usize) {
        let mut w = preamble();
        // Preamble fed: RCRC resets, then IDCODE feeds.
        let mut crc = feed_crc(0, REG_IDCODE, &[TEST_IDCODE]);
        write_reg(&mut w, REG_FAR, &[0]);
        crc = feed_crc(crc, REG_FAR, &[0]);
        write_cmd(&mut w, CMD_WCFG);
        crc = feed_crc(crc, REG_CMD, &[CMD_WCFG]);
        let fdri_payload_at = w.len() + 1;
        write_reg(&mut w, REG_FDRI, frame);
        crc = feed_crc(crc, REG_FDRI, frame);
        write_reg(&mut w, REG_FAR, &[0]);
        crc = feed_crc(crc, REG_FAR, &[0]);
        write_reg(&mut w, REG_CRC, &[crc]);
        (w, fdri_payload_at)
    }

    #[test]
    fn crc_write_with_correct_value_passes() {
        let frame = frame_words(0xC0FF_EE, 0x4444_4444);
        let (mut w, _) = stream_with_crc(&frame);
        ending(&mut w);
        let frames = parse(&w).expect("stream with a correct CRC must parse");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].words, frame);
    }

    #[test]
    fn crc_mismatch_fails_loud() {
        let frame = frame_words(0xC0FF_EE, 0x4444_4444);
        let (mut w, _) = stream_with_crc(&frame);
        ending(&mut w);
        // Corrupt the CRC value itself: flip its low bit. The CRC
        // packet is the only CRC-register header in the stream.
        let crc_header = t1(2, REG_CRC, 1);
        let pos = w
            .iter()
            .rposition(|x| *x == crc_header)
            .expect("CRC header present");
        w[pos + 1] ^= 1;
        let err = parse(&w).expect_err("wrong CRC value must fail");
        assert!(err.contains("CRC mismatch"), "unexpected error: {err}");
        assert!(err.contains("0x"), "error must show the values: {err}");
    }

    #[test]
    fn corrupted_fdri_payload_fails_crc() {
        // A single flipped FDRI word changes the running CRC, so the
        // stored CRC no longer matches: the exact failure the CRC
        // check exists to catch.
        let frame = frame_words(0xC0FF_EE, 0x4444_4444);
        let (mut w, fdri_payload_at) = stream_with_crc(&frame);
        ending(&mut w);
        w[fdri_payload_at] ^= 0x0000_0001;
        let err = parse(&w).expect_err("corrupted FDRI payload must fail the CRC check");
        assert!(err.contains("CRC mismatch"), "unexpected error: {err}");
    }

    #[test]
    fn stream_without_crc_writes_still_parses() {
        // Absence of CRC writes is not an error: synthetic streams,
        // partial bitstreams, and 7-series files carry none. Only a
        // present-but-wrong CRC fails.
        let mut w = preamble();
        write_reg(&mut w, REG_FAR, &[0]);
        write_cmd(&mut w, CMD_WCFG);
        write_reg(&mut w, REG_FDRI, &frame_words(0x1, 0));
        ending(&mut w);
        let frames = parse(&w).expect("stream without CRC writes must parse");
        assert_eq!(frames.len(), 1);
    }

    #[test]
    fn second_crc_write_after_ending_commands_is_checked() {
        // Vivado writes two CRCs: one after the frame data, one after
        // the ending commands (GRESTORE, DGHIGH_LFRM, START, sentinel
        // FAR, MASK, CTL0), with the first CRC write itself as the
        // reset between them (no RCRC in between). The second CRC
        // covers exactly those ending writes.
        let frame = frame_words(0xC0FF_EE, 0x4444_4444);
        let (mut w, _) = stream_with_crc(&frame);
        let mut crc = 0;
        for (reg, value) in [
            (REG_CMD, 10u32), // GRESTORE
            (REG_CMD, 3u32),  // DGHIGH_LFRM
            (REG_CMD, 5u32),  // START
            (REG_FAR, 0x07FC_0000u32),
            (6u8, 0x101u32), // MASK
            (5u8, 0x101u32), // CTL0
        ] {
            write_reg(&mut w, reg, &[value]);
            crc = feed_crc(crc, reg, &[value]);
        }
        write_reg(&mut w, REG_CRC, &[crc]);
        ending(&mut w);
        let frames = parse(&w).expect("stream with two correct CRCs must parse");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].words, frame);
    }
}
