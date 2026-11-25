//! LUT INIT, FF INIT, and BRAM content/parity bit locations for UltraScale+.
//!
//! Source of truth: the vendored BitFiltrator tables in
//! `data/ultrascale_plus_lut_ff_bram.json` (see `data/README.md` for
//! provenance). Every lookup here is a direct table read: no formula, no
//! guessed bit. The tables are not regular enough for a formula anyway:
//! frame offsets jump by an extra 96 at `Y_ofst` 30 (clock-region
//! boundary).
//!
//! What a lookup returns: a [`BitLoc`] is the (`minor`, `frame_ofst`) pair
//! from the table. `minor` is the minor address of the configuration frame
//! holding the bit; `frame_ofst` is the bit offset inside that 93-word
//! frame. An absolute frame address additionally needs the device file
//! (block type, row major, column major, SLR); that mapping is owned by
//! the device-file worker, not this module. See `INTERFACES.md` frame
//! addressing and `bitfiltrator-ref/src/bit_locator.py`.
//!
//! Lookup order follows `ArchSummary` in the BitFiltrator reference:
//!
//! - LUT: list index is the INIT bit index (0..63). `y_ofst` is the slice
//!   Y modulo 60 (clock-region height, `ArchSpec.num_clb_per_column`).
//! - FF: one pair per FF. BitFiltrator names the storage elements `AFF`
//!   through `HFF2`; Helion BELs name the same elements `AQ` through
//!   `HQ2` (UG574 Q output pin). Mapping is AFF=AQ, AFF2=AQ2, through
//!   HFF2=HQ2.
//! - BRAM: list index is the absolute content bit, `INIT` word * 256 +
//!   bit (see `src/u-ray_bram_comparator.py`: `abs_idx = init_idx * 256
//!   + bit_idx`). 16384 content bits are `INIT_00`..`INIT_3F` of one
//!   RAMB18; 2048 parity bits are `INITP_00`..`INITP_07`. `y_ofst` is
//!   the RAMB18 Y modulo 24 (`ArchSpec.num_18k_bram_per_column`).
//!
//! FASM emission follows the crew contract (`INTERFACES.md`) and the
//! day-1 scout report (`report.md`, route-to-bitgen handoff):
//! `SLICE_XxYy.A6LUT.INIT[63:0] = 64'h...`,
//! `SLICE_XxYy.AFF.INIT.V0` / `SLICE_XxYy.AFF.INIT.V1`,
//! `SLICE_XxYy.AFF.SRVAL.V0` / `SLICE_XxYy.AFF.SRVAL.V1`,
//! `RAMB18_XxYy.INIT[16383:0] = 16384'h...`,
//! `RAMB36_XxYy.INIT[32767:0] = 32768'h...`. INIT values expand MSB-first
//! across the listed bits, bit 0 = LSB of the value.
//!
//! FF INIT/SRVAL polarity (resolved 2026-11-07, researcher session 1):
//! the configuration bit is the INVERSE of the attribute value: INIT 0
//! selects the `.V0` feature, which sets the bit; INIT 1 selects `.V1`,
//! which clears it. Same for SRVAL. The V suffix names the attribute
//! value (prjuray `tools/dump_features.tcl:897-902` builds the feature
//! as `$bel_name.INIT.V[string range $init_prop end end]` from the cell
//! property; `utils/spec/flipflops.py:100` instantiates `.INIT(1'b0)` /
//! `.INIT(1'b1)`). The vendored DB lines agree:
//! `CLEL_L.AFF.INIT.V0 12_02`, `V1 !12_02`;
//! `CLEL_L.AFF.SRVAL.V0 12_01`, `V1 !12_01`. The session-1 emission
//! (bit = value, valued spelling `AFF.INIT = 1'b0/1`) was inverted and
//! non-canonical; the `.V0`/`.V1` spelling below selects the feature by
//! value and lets segbits resolution carry the polarity.
//!
//! LUTRAM content bits ARE the LUT INIT bits (derived, not assumed).
//! Three independent public sources agree:
//! - BitFiltrator's own extraction code
//!   (`bitfiltrator-ref/src/extract_encoding_from_logic_loc.py`, the
//!   `LutramLoc`/`LutLoc` branch): "the INIT bits we were interested in
//!   ended up having the same encoding for LUTRAMs and standard LUTs
//!   when we extracted the data", so both map to the one `LutLoc` key.
//!   The pipeline extracts CLEM/CLEM_R `LutLoc` from Vivado
//!   logic-location files of all-LUTRAM columns (`^LutramLoc$`, see
//!   `create_arch_summary.py`, the SLICEM loop) and CLEL_L/CLEL_R
//!   `LutLoc` from a LUT INIT fuzzing sweep (the SLICEL loop).
//! - The vendored tables are byte-identical across all four tile types
//!   (checked by `tests/lutram.rs`: 0 differing pairs out of 30,720 per
//!   tile-type pair, all 60 Y rows, all 8 LUTs, all 64 bits). The
//!   LUTRAM-extracted table and the LUT-sweep table are the same table.
//! - prjuray-db segbits agree bit for bit: every `xLUT.INIT[i]` line in
//!   all four vendored `segbits_cle*.db` files equals the `LutLoc` Y0
//!   pair (512 of 512 per tile type, checked by `tests/lutram.rs`).
//! The LUTRAM mode-selection bits live in the vendored segbits DBs, not
//! in this table: `xLUT.MODE.RAM` (2 bits per LUT), `xLUT.DI1.*` (1 bit,
//! A..G LUTs only; the H LUT has no DI1 feature), `xLUT.SIZE.*` (1 bit
//! per LUT), `WA7/8/9USED.V0/V1` (1 bit each, slice-wide). None of them
//! collides with any INIT bit (checked by `tests/lutram.rs`).
//! A LUTRAM is therefore emitted as its 64 content bits through
//! [`lut_placed_bits`] with [`LutMode::Lutram`] (same locations as a
//! logic LUT, block label `LUT_INIT`) plus its mode lines through
//! [`lutram_fasm`] (block `LUTRAM`). The mode-line selection for a
//! 64x1 single-port RAM (RAM64X1S) follows the f4pga Project X-Ray
//! DRAM documentation (7-series feature family, same concepts under
//! UltraScale+ spellings): 64x1S needs `xLUT.RAM` plus the data-in mux
//! to the LUT's own input, `SMALL` only for 32-bit use, `WA7/8USED`
//! only for 128x1/256x1 use. That selection is a documented
//! cross-family inference, not a Vivado-verified UltraScale+ fact:
//! tier 2 (a Vivado bitstream diff) remains the arbiter for it. The
//! bit LOCATIONS are proven by the three sources above; only the
//! SELECTION of the mode values awaits tier 2.
//! LUTRAMs live on SLICEM tiles only (CLEM/CLEM_R). A LUTRAM request on
//! a SLICEL tile fails loud with [`BitLocError::LutramRequiresSlicem`].
//! The H LUT has no DI1 feature in the vendored DB, so [`lutram_fasm`]
//! fails loud for it with [`BitLocError::LutramNoDataInput`]: the H
//! data-input path is unverified, never guessed.
//!
//! Interface to the synthesis-mapping worker (fm/ultra-bitgen-synthmap):
//! that worker owns mapping netlist RAM cells to UltraScale primitives
//! (RAM64X1S and friends) and deciding which packed LUTs are memory
//! LUTs (`pack::lut_is_memory`, `LutMode::Lutram`); this module owns
//! only the bit locations and the FASM spelling for one such LUT.
//! Coordination goes through the orchestrator, never by editing the
//! other worker's branch.
//!
//! BRAM tile use (CTO ruling on critic G3): a BRAM tile is 720 RAMB36E2
//! total across the part. The two RAMB18E2 BELs on a tile are alternate
//! views of the same physical halves, not additional capacity. A design
//! may use a tile as one RAMB36 OR as two RAMB18, never both. That
//! exclusivity is encoded in [`BramTileUse`]: one variant per tile.
//! Capacity accounting (the 720 total) is the device-file worker's; this
//! module never counts capacity and never sums both views.

use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One configuration bit location: the frame (by minor address) and the bit
/// offset inside that frame, exactly as the BitFiltrator tables list them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BitLoc {
    /// Minor address of the configuration frame holding this bit.
    pub minor: u32,
    /// Bit offset inside the frame (0..2976 for a 93-word frame).
    pub frame_ofst: u32,
}

/// CLB tile type, using the BitFiltrator vocabulary (CLEL_L, CLEL_R, CLEM,
/// CLEM_R). The vendored tables are identical across all four types
/// (verified at extraction; see `data/README.md`), but lookups still take
/// the type so a future table difference cannot silently alias.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClbTileType {
    ClelL,
    ClelR,
    Clem,
    ClemR,
}

impl ClbTileType {
    /// Key into the vendored JSON tables.
    pub fn json_key(self) -> &'static str {
        match self {
            ClbTileType::ClelL => "CLEL_L",
            ClbTileType::ClelR => "CLEL_R",
            ClbTileType::Clem => "CLEM",
            ClbTileType::ClemR => "CLEM_R",
        }
    }
}

impl std::str::FromStr for ClbTileType {
    type Err = BitLocError;

    /// Parse a tile-type name. Unknown names fail loud, never default.
    fn from_str(name: &str) -> Result<Self, BitLocError> {
        match name {
            "CLEL_L" => Ok(ClbTileType::ClelL),
            "CLEL_R" => Ok(ClbTileType::ClelR),
            "CLEM" => Ok(ClbTileType::Clem),
            "CLEM_R" => Ok(ClbTileType::ClemR),
            other => Err(BitLocError::UnknownTileType(other.to_string())),
        }
    }
}

/// Whether a LUT is used as plain logic or as distributed RAM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LutMode {
    /// Plain 6-input LUT. INIT bits come from `LutLoc`.
    Logic,
    /// LUTRAM (distributed RAM). Content bits use the LUT INIT
    /// locations (proven same encoding, see the module docs); only
    /// SLICEM tiles (CLEM/CLEM_R) accept it, enforced in
    /// [`lut_placed_bits`].
    Lutram,
}

/// Diff-harness block names (INTERFACES.md block registry).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Block {
    LutInit,
    FfInit,
    BramContent,
    BramParity,
    /// LUTRAM mode-selection bits (`xLUT.MODE.RAM`, `xLUT.DI1.*`,
    /// `xLUT.SIZE.*`, `WA7/8/9USED`). A LUTRAM's 64 content bits are
    /// `LutInit` bits (same encoding, proven in the module docs); this
    /// block labels only the mode bits, which arrive via the segbits DB,
    /// not via [`PlacedBit`].
    Lutram,
}

impl Block {
    pub fn name(self) -> &'static str {
        match self {
            Block::LutInit => "LUT_INIT",
            Block::FfInit => "FF_INIT",
            Block::BramContent => "BRAM_CONTENT",
            Block::BramParity => "BRAM_PARITY",
            Block::Lutram => "LUTRAM",
        }
    }
}

/// One resolved configuration bit with its value, the exact input the diff
/// harness consumes: (block, frame minor, frame offset, value).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlacedBit {
    pub block: Block,
    pub minor: u32,
    pub frame_ofst: u32,
    pub value: bool,
}

/// Which half of a RAMB36 tile a RAMB18 site is. Lower is Bel index 0
/// (`RAMB18E2_L`), upper is Bel index 1 (`RAMB18E2_U`); see `Bel::name`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BramHalf {
    Lower,
    Upper,
}

impl BramHalf {
    /// Suffix used in segbits feature names (`RAMB18E2_L` / `RAMB18E2_U`).
    pub fn suffix(self) -> &'static str {
        match self {
            BramHalf::Lower => "L",
            BramHalf::Upper => "U",
        }
    }
}

/// Content of one RAMB18 half: 16384 content bits as 256 u64 words plus
/// 2048 parity bits as 32 u64 words. Word `w` holds absolute bits
/// `w*64 .. w*64+64`; absolute bit `i` is INIT word `i/256`, bit `i%256`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BramHalfContent {
    pub content: [u64; 256],
    pub parity: [u64; 32],
}

impl BramHalfContent {
    pub fn zeros() -> Self {
        Self {
            content: [0; 256],
            parity: [0; 32],
        }
    }
}

/// How one BRAM tile is used. Exactly one variant holds at a time, so a
/// tile can never be configured as a RAMB36 and as RAMB18 halves at once
/// (CTO ruling on critic G3). A RAMB36 holds 32768 content bits
/// (512 u64 words) and 4096 parity bits (64 u64 words); words 0..256 are
/// the lower half, words 256..512 the upper half, matching the U-Ray L/U
/// split of the RAMB36 `INIT_00`..`INIT_7F` words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BramTileUse {
    Ramb36 {
        content: [u64; 512],
        parity: [u64; 64],
    },
    Ramb18 {
        lower: Option<BramHalfContent>,
        upper: Option<BramHalfContent>,
    },
}

/// Errors from bit-location lookup and emission. Unknown encodings fail
/// loud; nothing here ever emits a guessed bit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BitLocError {
    /// LUT Bel index outside 0..8.
    LutIndexOutOfRange(u8),
    /// FF Bel index outside 0..16.
    FfIndexOutOfRange(u8),
    /// BRAM absolute bit index outside the table (`max` is the table size).
    BramIndexOutOfRange { index: u32, max: u32 },
    /// Tile-type name is not one of CLEL_L, CLEL_R, CLEM, CLEM_R.
    UnknownTileType(String),
    /// URAM288 content emission requested. No URAM288 content table
    /// exists in any public source (prjuray-db has no URAM mention;
    /// BitFiltrator ULTRASCALE_PLUS.json has no URAM table), and
    /// URAM288 has no INIT attributes in UG974, so there is nothing
    /// to vary for a content sweep. Blocked until the Vivado fuzzing
    /// job (Job 5, phase 3 probe) lands the encoding.
    UramBlocked,
    /// LUTRAM requested on a SLICEL tile (CLEL_L/CLEL_R). LUTRAMs live
    /// on SLICEM tiles (CLEM/CLEM_R) only; the vendored segbits DBs
    /// carry no MODE.RAM feature for SLICEL tiles.
    LutramRequiresSlicem(ClbTileType),
    /// LUTRAM FASM requested for a LUT with no DI1 feature in the
    /// vendored segbits DB (the H LUT). The H data-input path is
    /// unverified; it fails loud instead of guessing a DI source.
    LutramNoDataInput(u8),
    /// The vendored table file is missing a key or has the wrong shape.
    TableCorrupt(String),
}

impl fmt::Display for BitLocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BitLocError::LutIndexOutOfRange(i) => {
                write!(f, "LUT Bel index {i} out of range 0..8")
            }
            BitLocError::FfIndexOutOfRange(i) => {
                write!(f, "FF Bel index {i} out of range 0..16")
            }
            BitLocError::BramIndexOutOfRange { index, max } => {
                write!(f, "BRAM bit index {index} out of range 0..{max}")
            }
            BitLocError::UnknownTileType(name) => {
                write!(f, "unknown CLB tile type {name:?}")
            }
            BitLocError::LutramRequiresSlicem(tile) => write!(
                f,
                "LUTRAM requested on SLICEL tile {}: LUTRAMs live on SLICEM tiles (CLEM/CLEM_R) only",
                tile.json_key()
            ),
            BitLocError::LutramNoDataInput(lut) => write!(
                f,
                "LUTRAM on LUT {lut} has no DI1 feature in the vendored segbits DB: the data-input path is unverified"
            ),
            BitLocError::UramBlocked => write!(
                f,
                "URAM288 emission blocked: no URAM288 content or config encoding in any public source (prjuray-db and BitFiltrator ULTRASCALE_PLUS.json carry no URAM data) and URAM288 has no INIT attributes in UG974; blocked until the Vivado fuzzing job (Job 5) lands it"
            ),
            BitLocError::TableCorrupt(detail) => {
                write!(f, "vendored bit-location table corrupt: {detail}")
            }
        }
    }
}

impl std::error::Error for BitLocError {}

// ---------------------------------------------------------------------------
// Constants from the reference (BitFiltrator arch_spec.py, UG574 pg 11)
// ---------------------------------------------------------------------------

/// CLBs per column (clock-region height). `y_ofst = slice_y % 60`.
pub const NUM_CLB_PER_COLUMN: u32 = 60;
/// RAMB18 sites per BRAM column. `y_ofst = bram_y % 24`.
pub const NUM_BRAM18_PER_COLUMN: u32 = 24;
/// Bits in one UltraScale+ configuration frame (93 words x 32 bits).
pub const FRAME_BITS: u32 = 93 * 32;
/// INIT bits per 6-LUT.
pub const LUT_INIT_BITS: usize = 64;
/// Content bits per RAMB18 site.
pub const BRAM_MEM_BITS: usize = 16384;
/// Parity bits per RAMB18 site.
pub const BRAM_PARITY_BITS: usize = 2048;

// ---------------------------------------------------------------------------
// Tables (parsed once from the vendored JSON)
// ---------------------------------------------------------------------------

struct Tables {
    // lut_minor[tile][y_ofst][lut][bit], lut_frame_ofst likewise.
    lut_minor: Vec<Vec<[[u32; LUT_INIT_BITS]; 8]>>,
    lut_frame_ofst: Vec<Vec<[[u32; LUT_INIT_BITS]; 8]>>,
    // reg_minor[tile][y_ofst][ff], reg_frame_ofst likewise.
    reg_minor: Vec<Vec<[u32; 16]>>,
    reg_frame_ofst: Vec<Vec<[u32; 16]>>,
    // bram_mem_minor[y_ofst][bit], bram_mem_frame_ofst likewise.
    bram_mem_minor: Vec<Vec<u32>>,
    bram_mem_frame_ofst: Vec<Vec<u32>>,
    bram_parity_minor: Vec<Vec<u32>>,
    bram_parity_frame_ofst: Vec<Vec<u32>>,
}

const TILE_TYPES: [ClbTileType; 4] = [
    ClbTileType::ClelL,
    ClbTileType::ClelR,
    ClbTileType::Clem,
    ClbTileType::ClemR,
];

fn tables() -> Result<&'static Tables, BitLocError> {
    static TABLES: OnceLock<Result<Tables, BitLocError>> = OnceLock::new();
    TABLES
        .get_or_init(load_tables)
        .as_ref()
        .map_err(|e| e.clone())
}

fn load_tables() -> Result<Tables, BitLocError> {
    let raw = include_str!("../data/ultrascale_plus_lut_ff_bram.json");
    let root: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| BitLocError::TableCorrupt(e.to_string()))?;
    let corrupt = |what: &str| BitLocError::TableCorrupt(what.to_string());

    let mut lut_minor = Vec::new();
    let mut lut_frame_ofst = Vec::new();
    let mut reg_minor = Vec::new();
    let mut reg_frame_ofst = Vec::new();
    for tile in TILE_TYPES {
        let key = tile.json_key();
        let lut_y = root[key]["LutLoc"]["Y_ofst"]
            .as_object()
            .ok_or_else(|| corrupt("LutLoc Y_ofst not an object"))?;
        if lut_y.len() != NUM_CLB_PER_COLUMN as usize {
            return Err(corrupt("LutLoc Y_ofst count is not 60"));
        }
        let mut tile_lut_minor: Vec<[[u32; LUT_INIT_BITS]; 8]> = Vec::new();
        let mut tile_lut_frame: Vec<[[u32; LUT_INIT_BITS]; 8]> = Vec::new();
        let mut tile_reg_minor: Vec<[u32; 16]> = Vec::new();
        let mut tile_reg_frame: Vec<[u32; 16]> = Vec::new();
        for y in 0..NUM_CLB_PER_COLUMN {
            let entry = &lut_y[&y.to_string()];
            let mut minors = [[0u32; LUT_INIT_BITS]; 8];
            let mut frames = [[0u32; LUT_INIT_BITS]; 8];
            for lut in 0..8 {
                let letter = (b'A' + lut as u8) as char;
                let name = format!("{letter}6LUT");
                let m = u32_array(&entry["minor"][&name], "LutLoc minor")?;
                let f = u32_array(&entry["frame_ofst"][&name], "LutLoc frame_ofst")?;
                if m.len() != LUT_INIT_BITS || f.len() != LUT_INIT_BITS {
                    return Err(corrupt("LutLoc per-LUT length is not 64"));
                }
                minors[lut].copy_from_slice(&m);
                frames[lut].copy_from_slice(&f);
            }
            tile_lut_minor.push(minors);
            tile_lut_frame.push(frames);

            let reg_y = root[key]["RegLoc"]["Y_ofst"]
                .as_object()
                .ok_or_else(|| corrupt("RegLoc Y_ofst not an object"))?;
            let reg_entry = &reg_y[&y.to_string()];
            let mut rmin = [0u32; 16];
            let mut rframe = [0u32; 16];
            for ff in 0..16 {
                let name = ff_fasm_name(ff as u8).expect("ff index 0..16 is valid");
                let m = reg_entry["minor"][name]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| corrupt("RegLoc minor"))?;
                let f = reg_entry["frame_ofst"][name]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| corrupt("RegLoc frame_ofst"))?;
                rmin[ff] = m;
                rframe[ff] = f;
            }
            tile_reg_minor.push(rmin);
            tile_reg_frame.push(rframe);
        }
        lut_minor.push(tile_lut_minor);
        lut_frame_ofst.push(tile_lut_frame);
        reg_minor.push(tile_reg_minor);
        reg_frame_ofst.push(tile_reg_frame);
    }

    let (bram_mem_minor, bram_mem_frame_ofst) =
        load_bram_table(&root, "BramMemLoc", BRAM_MEM_BITS)?;
    let (bram_parity_minor, bram_parity_frame_ofst) =
        load_bram_table(&root, "BramMemParityLoc", BRAM_PARITY_BITS)?;

    let tables = Tables {
        lut_minor,
        lut_frame_ofst,
        reg_minor,
        reg_frame_ofst,
        bram_mem_minor,
        bram_mem_frame_ofst,
        bram_parity_minor,
        bram_parity_frame_ofst,
    };
    validate_tables(&tables)?;
    Ok(tables)
}

/// One BRAM table (`BramMemLoc` or `BramMemParityLoc`) as (minors, frame
/// offsets) per Y_ofst. Returns the two parallel vectors.
type BramTable = (Vec<Vec<u32>>, Vec<Vec<u32>>);

fn load_bram_table(
    root: &serde_json::Value,
    table: &str,
    count: usize,
) -> Result<BramTable, BitLocError> {
    let corrupt = |what: &str| BitLocError::TableCorrupt(what.to_string());
    let y_obj = root["BRAM"][table]["Y_ofst"]
        .as_object()
        .ok_or_else(|| corrupt("Bram Y_ofst not an object"))?;
    if y_obj.len() != NUM_BRAM18_PER_COLUMN as usize {
        return Err(corrupt("Bram Y_ofst count is not 24"));
    }
    let mut minors = Vec::new();
    let mut frames = Vec::new();
    for y in 0..NUM_BRAM18_PER_COLUMN {
        let entry = &y_obj[&y.to_string()];
        let m = u32_array(&entry["minor"], "Bram minor")?;
        let f = u32_array(&entry["frame_ofst"], "Bram frame_ofst")?;
        if m.len() != count || f.len() != count {
            return Err(corrupt("Bram per-Y length mismatch"));
        }
        minors.push(m);
        frames.push(f);
    }
    Ok((minors, frames))
}

/// Read a JSON array of small ints as `Vec<u32>`, failing loud on shape.
fn u32_array(v: &serde_json::Value, what: &str) -> Result<Vec<u32>, BitLocError> {
    let corrupt = || BitLocError::TableCorrupt(what.to_string());
    let arr = v.as_array().ok_or_else(corrupt)?;
    arr.iter()
        .map(|x| {
            x.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(corrupt)
        })
        .collect()
}

/// Structural validation: every (minor, frame_ofst) pair must address a real
/// bit of a real frame (minor is the 8-bit FAR minor field, frame_ofst is
/// inside a 93-word frame), and every LUT/FF set of pairs must be distinct
/// (two INIT bits never share one frame bit).
fn validate_tables(t: &Tables) -> Result<(), BitLocError> {
    let corrupt = |what: String| BitLocError::TableCorrupt(what);
    let check_pair = |minor: u32, frame_ofst: u32, what: &str| -> Result<(), BitLocError> {
        if minor > 255 {
            return Err(corrupt(format!("{what}: minor {minor} exceeds 255")));
        }
        if frame_ofst >= FRAME_BITS {
            return Err(corrupt(format!(
                "{what}: frame_ofst {frame_ofst} exceeds frame size {FRAME_BITS}"
            )));
        }
        Ok(())
    };
    for (ti, _) in TILE_TYPES.iter().enumerate() {
        for y in 0..NUM_CLB_PER_COLUMN as usize {
            for lut in 0..8 {
                let mut seen = HashMap::new();
                for bit in 0..LUT_INIT_BITS {
                    let minor = t.lut_minor[ti][y][lut][bit];
                    let frame = t.lut_frame_ofst[ti][y][lut][bit];
                    check_pair(minor, frame, "LutLoc")?;
                    if seen.insert((minor, frame), bit).is_some() {
                        return Err(corrupt(format!(
                            "LutLoc tile {ti} y {y} lut {lut}: duplicate pair"
                        )));
                    }
                }
            }
            let mut seen = HashMap::new();
            for ff in 0..16 {
                let minor = t.reg_minor[ti][y][ff];
                let frame = t.reg_frame_ofst[ti][y][ff];
                check_pair(minor, frame, "RegLoc")?;
                if seen.insert((minor, frame), ff).is_some() {
                    return Err(corrupt(format!("RegLoc tile {ti} y {y}: duplicate pair")));
                }
            }
        }
    }
    for y in 0..NUM_BRAM18_PER_COLUMN as usize {
        for bit in 0..BRAM_MEM_BITS {
            check_pair(
                t.bram_mem_minor[y][bit],
                t.bram_mem_frame_ofst[y][bit],
                "BramMemLoc",
            )?;
        }
        for bit in 0..BRAM_PARITY_BITS {
            check_pair(
                t.bram_parity_minor[y][bit],
                t.bram_parity_frame_ofst[y][bit],
                "BramMemParityLoc",
            )?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// FASM / BitFiltrator LUT name for a Bel index 0..8: `A6LUT`..`H6LUT`.
pub fn lut_fasm_name(lut_index: u8) -> Result<&'static str, BitLocError> {
    match lut_index {
        0 => Ok("A6LUT"),
        1 => Ok("B6LUT"),
        2 => Ok("C6LUT"),
        3 => Ok("D6LUT"),
        4 => Ok("E6LUT"),
        5 => Ok("F6LUT"),
        6 => Ok("G6LUT"),
        7 => Ok("H6LUT"),
        other => Err(BitLocError::LutIndexOutOfRange(other)),
    }
}

/// FASM / BitFiltrator FF name for a Bel index 0..16: `AFF`..`HFF2`.
/// Bel index 0 is AQ (AFF), 1 is AQ2 (AFF2), through 15 is HQ2 (HFF2).
pub fn ff_fasm_name(ff_index: u8) -> Result<&'static str, BitLocError> {
    match ff_index {
        0 => Ok("AFF"),
        1 => Ok("AFF2"),
        2 => Ok("BFF"),
        3 => Ok("BFF2"),
        4 => Ok("CFF"),
        5 => Ok("CFF2"),
        6 => Ok("DFF"),
        7 => Ok("DFF2"),
        8 => Ok("EFF"),
        9 => Ok("EFF2"),
        10 => Ok("FFF"),
        11 => Ok("FFF2"),
        12 => Ok("GFF"),
        13 => Ok("GFF2"),
        14 => Ok("HFF"),
        15 => Ok("HFF2"),
        other => Err(BitLocError::FfIndexOutOfRange(other)),
    }
}

// ---------------------------------------------------------------------------
// LUT
// ---------------------------------------------------------------------------

/// The 64 (`minor`, `frame_ofst`) pairs for one LUT, list index = INIT bit
/// index. `slice_y` is the Vivado SLICE Y; the table row is
/// `slice_y % 60`.
pub fn lut_locations(
    tile_type: ClbTileType,
    slice_y: u32,
    lut_index: u8,
) -> Result<[BitLoc; LUT_INIT_BITS], BitLocError> {
    if lut_index >= 8 {
        return Err(BitLocError::LutIndexOutOfRange(lut_index));
    }
    let t = tables()?;
    let ti = TILE_TYPES
        .iter()
        .position(|t| *t == tile_type)
        .expect("tile in list");
    let y = (slice_y % NUM_CLB_PER_COLUMN) as usize;
    let mut out = [BitLoc {
        minor: 0,
        frame_ofst: 0,
    }; LUT_INIT_BITS];
    for (bit, slot) in out.iter_mut().enumerate() {
        *slot = BitLoc {
            minor: t.lut_minor[ti][y][lut_index as usize][bit],
            frame_ofst: t.lut_frame_ofst[ti][y][lut_index as usize][bit],
        };
    }
    Ok(out)
}

/// The 64 placed INIT bits for one LUT, in INIT-bit order, each with its
/// table location and its value from `init`. LUTRAM mode returns the same
/// 64 locations as logic mode (proven same encoding, see the module
/// docs), labeled block `LutInit`; the mode-selection bits are separate
/// segbits features, emitted by [`lutram_fasm`]. LUTRAM mode on a SLICEL
/// tile fails loud with [`BitLocError::LutramRequiresSlicem`].
pub fn lut_placed_bits(
    tile_type: ClbTileType,
    slice_y: u32,
    lut_index: u8,
    mode: LutMode,
    init: u64,
) -> Result<Vec<PlacedBit>, BitLocError> {
    if mode == LutMode::Lutram {
        match tile_type {
            ClbTileType::Clem | ClbTileType::ClemR => {}
            other => return Err(BitLocError::LutramRequiresSlicem(other)),
        }
    }
    let locs = lut_locations(tile_type, slice_y, lut_index)?;
    Ok(locs
        .iter()
        .enumerate()
        .map(|(bit, loc)| PlacedBit {
            block: Block::LutInit,
            minor: loc.minor,
            frame_ofst: loc.frame_ofst,
            value: (init >> bit) & 1 == 1,
        })
        .collect())
}

/// FASM line for one logic LUT:
/// `SLICE_X{x}Y{y}.{L}6LUT.INIT[63:0] = 64'h{init:016X}`.
pub fn lut_init_fasm(
    slice_x: u32,
    slice_y: u32,
    lut_index: u8,
    init: u64,
) -> Result<String, BitLocError> {
    let name = lut_fasm_name(lut_index)?;
    Ok(format!(
        "SLICE_X{slice_x}Y{slice_y}.{name}.INIT[63:0] = 64'h{init:016X}"
    ))
}

/// FASM lines for one 64x1 single-port LUTRAM (RAM64X1S) on one LUT, in
/// emission order: the content line (same `INIT[63:0]` spelling as a
/// logic LUT, same 64 locations), then the mode lines, all bare
/// presence-only features resolved through the vendored segbits DBs:
/// `{L}LUT.MODE.RAM` (RAM mode, 2 bits), `{L}LUT.DI1.DI1` (write data
/// from the DI1 input, not serial-in), `{L}LUT.SIZE.LARGE` (64-bit use:
/// all 6 address inputs; the `SIZE` bit reads 0 for LARGE).
///
/// Mode-line selection follows the f4pga Project X-Ray DRAM
/// documentation (7-series feature family, same concepts under
/// UltraScale+ spellings): 64x1S needs `xLUT.RAM` plus the data-in mux
/// to the LUT's own input; `SMALL` only for 32-bit use; `WA7/8USED`
/// only for 128x1/256x1 use (those are slice-wide, see
/// [`lutram_slice_fasm`]). That selection is a documented cross-family
/// inference; tier 2 (a Vivado bitstream diff) remains its arbiter.
/// The H LUT (index 7) has no DI1 feature in the vendored DB, so this
/// fails loud for it with [`BitLocError::LutramNoDataInput`]. Callers
/// must place the LUT on a SLICEM tile (pack/legality enforces it);
/// emitting these lines for a SLICEL tile fails loud at segbits
/// resolution (no MODE.RAM feature exists for SLICEL tiles).
pub fn lutram_fasm(
    slice_x: u32,
    slice_y: u32,
    lut_index: u8,
    init: u64,
) -> Result<Vec<String>, BitLocError> {
    if lut_index == 7 {
        return Err(BitLocError::LutramNoDataInput(lut_index));
    }
    let letter = (b'A' + lut_index) as char;
    Ok(vec![
        lut_init_fasm(slice_x, slice_y, lut_index, init)?,
        format!("SLICE_X{slice_x}Y{slice_y}.{letter}LUT.MODE.RAM"),
        format!("SLICE_X{slice_x}Y{slice_y}.{letter}LUT.DI1.DI1"),
        format!("SLICE_X{slice_x}Y{slice_y}.{letter}LUT.SIZE.LARGE"),
    ])
}

/// FASM lines for the slice-wide write-address-use bits of a SLICEM
/// slice holding only 64-deep RAMs: `WA7USED.V0`, `WA8USED.V0`,
/// `WA9USED.V0` (WA7/8/9 not used: the f4pga DRAM documentation
/// requires them only for 128x1/256x1 slice-wide modes). The `.V0`
/// spelling names the value 0 (not used); each line resolves to a
/// must-be-0 bit in the vendored segbits DBs. Emit once per slice,
/// not once per LUT.
pub fn lutram_slice_fasm(slice_x: u32, slice_y: u32) -> Vec<String> {
    vec![
        format!("SLICE_X{slice_x}Y{slice_y}.WA7USED.V0"),
        format!("SLICE_X{slice_x}Y{slice_y}.WA8USED.V0"),
        format!("SLICE_X{slice_x}Y{slice_y}.WA9USED.V0"),
    ]
}

// ---------------------------------------------------------------------------
// FF
// ---------------------------------------------------------------------------

/// The single (`minor`, `frame_ofst`) pair holding one FF's INIT bit.
/// `slice_y` is the Vivado SLICE Y; the table row is `slice_y % 60`.
pub fn ff_location(
    tile_type: ClbTileType,
    slice_y: u32,
    ff_index: u8,
) -> Result<BitLoc, BitLocError> {
    if ff_index >= 16 {
        return Err(BitLocError::FfIndexOutOfRange(ff_index));
    }
    let t = tables()?;
    let ti = TILE_TYPES
        .iter()
        .position(|t| *t == tile_type)
        .expect("tile in list");
    let y = (slice_y % NUM_CLB_PER_COLUMN) as usize;
    Ok(BitLoc {
        minor: t.reg_minor[ti][y][ff_index as usize],
        frame_ofst: t.reg_frame_ofst[ti][y][ff_index as usize],
    })
}

/// The placed INIT bit for one FF. The configuration bit is the inverse
/// of the INIT value (bit = NOT(INIT), resolved from the fuzzer source;
/// see the module docs): INIT 0 sets the bit, INIT 1 clears it. The
/// session-1 value (bit = value) was inverted.
pub fn ff_placed_bit(
    tile_type: ClbTileType,
    slice_y: u32,
    ff_index: u8,
    init: bool,
) -> Result<PlacedBit, BitLocError> {
    let loc = ff_location(tile_type, slice_y, ff_index)?;
    Ok(PlacedBit {
        block: Block::FfInit,
        minor: loc.minor,
        frame_ofst: loc.frame_ofst,
        value: !init,
    })
}

/// FASM line for one FF: `SLICE_X{x}Y{y}.{AFF}.INIT.V0` (INIT 0) or
/// `.INIT.V1` (INIT 1). The V suffix names the INIT value; the bit
/// polarity comes from segbits resolution (V0 sets the bit, V1 clears
/// it). The valued spelling (`AFF.INIT = 1'b0/1`) is non-canonical and
/// is never emitted.
pub fn ff_init_fasm(
    slice_x: u32,
    slice_y: u32,
    ff_index: u8,
    init: bool,
) -> Result<String, BitLocError> {
    let name = ff_fasm_name(ff_index)?;
    let v = if init { '1' } else { '0' };
    Ok(format!("SLICE_X{slice_x}Y{slice_y}.{name}.INIT.V{v}"))
}

/// Frame-offset delta from a FF's INIT bit to its SRVAL bit, within one
/// slice. Verified against prjuray-db `segbits_clel_l.db`,
/// `segbits_clel_r.db`, `segbits_clem.db`, and `segbits_clem_r.db` for
/// all 16 FFs: A/C/E/G letters sit one bit below INIT, B/D/F/H letters
/// one bit above. The delta is a within-slice offset, so it holds at
/// every slice Y: the whole 48-bit slice block shifts with Y (plus the
/// +96 clock-region jump at Y 30), and relative offsets inside the block
/// do not change. The derivation is checked against verbatim DB lines in
/// `tests/lut_ff_bram.rs` (`ff_init_srval_resolve_through_segbits`).
fn srval_frame_ofst_delta(ff_index: u8) -> i64 {
    // Letter index 0=A, 1=B, ... 7=H; even letters -1, odd letters +1.
    if (ff_index / 2) % 2 == 0 { -1 } else { 1 }
}

/// The single (`minor`, `frame_ofst`) pair holding one FF's SRVAL bit.
///
/// The vendored BitFiltrator JSON carries no SRVAL table (RegLoc is the
/// INIT location only), so this derives the SRVAL location from the INIT
/// location by the DB-verified within-slice delta (see
/// [`srval_frame_ofst_delta`]): same frame minor, one frame offset below
/// INIT for A/C/E/G FFs, one above for B/D/F/H FFs. Out-of-range results
/// fail loud instead of wrapping.
pub fn ff_srval_location(
    tile_type: ClbTileType,
    slice_y: u32,
    ff_index: u8,
) -> Result<BitLoc, BitLocError> {
    let init = ff_location(tile_type, slice_y, ff_index)?;
    let ofst = init.frame_ofst as i64 + srval_frame_ofst_delta(ff_index);
    if ofst < 0 || ofst >= FRAME_BITS as i64 {
        return Err(BitLocError::TableCorrupt(format!(
            "SRVAL frame offset {ofst} out of range for FF {ff_index} at slice Y {slice_y}"
        )));
    }
    Ok(BitLoc {
        minor: init.minor,
        frame_ofst: ofst as u32,
    })
}

/// The placed SRVAL bit for one FF. Same inverse polarity as INIT
/// (bit = NOT(SRVAL)): SRVAL 0 sets the bit, SRVAL 1 clears it.
pub fn ff_srval_placed_bit(
    tile_type: ClbTileType,
    slice_y: u32,
    ff_index: u8,
    srval: bool,
) -> Result<PlacedBit, BitLocError> {
    let loc = ff_srval_location(tile_type, slice_y, ff_index)?;
    Ok(PlacedBit {
        block: Block::FfInit,
        minor: loc.minor,
        frame_ofst: loc.frame_ofst,
        value: !srval,
    })
}

/// FASM line for one FF's SRVAL: `SLICE_X{x}Y{y}.{AFF}.SRVAL.V0`
/// (SRVAL 0) or `.SRVAL.V1` (SRVAL 1). Same spelling rule as INIT.
pub fn ff_srval_fasm(
    slice_x: u32,
    slice_y: u32,
    ff_index: u8,
    srval: bool,
) -> Result<String, BitLocError> {
    let name = ff_fasm_name(ff_index)?;
    let v = if srval { '1' } else { '0' };
    Ok(format!("SLICE_X{slice_x}Y{slice_y}.{name}.SRVAL.V{v}"))
}

// ---------------------------------------------------------------------------
// BRAM
// ---------------------------------------------------------------------------

/// The 16384 (`minor`, `frame_ofst`) pairs for one RAMB18 site's content,
/// list index = absolute content bit (`INIT` word * 256 + bit). `bram_y`
/// is the Vivado RAMB18 Y; the table row is `bram_y % 24`.
pub fn bram_mem_locations(bram_y: u32) -> Result<Vec<BitLoc>, BitLocError> {
    let t = tables()?;
    let y = (bram_y % NUM_BRAM18_PER_COLUMN) as usize;
    Ok((0..BRAM_MEM_BITS)
        .map(|bit| BitLoc {
            minor: t.bram_mem_minor[y][bit],
            frame_ofst: t.bram_mem_frame_ofst[y][bit],
        })
        .collect())
}

/// One RAMB18 content bit location by absolute index (0..16384).
pub fn bram_mem_location(bram_y: u32, abs_index: u32) -> Result<BitLoc, BitLocError> {
    if abs_index as usize >= BRAM_MEM_BITS {
        return Err(BitLocError::BramIndexOutOfRange {
            index: abs_index,
            max: BRAM_MEM_BITS as u32,
        });
    }
    let t = tables()?;
    let y = (bram_y % NUM_BRAM18_PER_COLUMN) as usize;
    Ok(BitLoc {
        minor: t.bram_mem_minor[y][abs_index as usize],
        frame_ofst: t.bram_mem_frame_ofst[y][abs_index as usize],
    })
}

/// The 2048 (`minor`, `frame_ofst`) pairs for one RAMB18 site's parity,
/// list index = absolute parity bit (`INITP` word * 256 + bit).
pub fn bram_parity_locations(bram_y: u32) -> Result<Vec<BitLoc>, BitLocError> {
    let t = tables()?;
    let y = (bram_y % NUM_BRAM18_PER_COLUMN) as usize;
    Ok((0..BRAM_PARITY_BITS)
        .map(|bit| BitLoc {
            minor: t.bram_parity_minor[y][bit],
            frame_ofst: t.bram_parity_frame_ofst[y][bit],
        })
        .collect())
}

/// One RAMB18 parity bit location by absolute index (0..2048).
pub fn bram_parity_location(bram_y: u32, abs_index: u32) -> Result<BitLoc, BitLocError> {
    if abs_index as usize >= BRAM_PARITY_BITS {
        return Err(BitLocError::BramIndexOutOfRange {
            index: abs_index,
            max: BRAM_PARITY_BITS as u32,
        });
    }
    let t = tables()?;
    let y = (bram_y % NUM_BRAM18_PER_COLUMN) as usize;
    Ok(BitLoc {
        minor: t.bram_parity_minor[y][abs_index as usize],
        frame_ofst: t.bram_parity_frame_ofst[y][abs_index as usize],
    })
}

/// The placed content bits for one RAMB18 half, in absolute-bit order.
pub fn bram_mem_placed_bits(
    bram_y: u32,
    content: &[u64; 256],
) -> Result<Vec<PlacedBit>, BitLocError> {
    let locs = bram_mem_locations(bram_y)?;
    Ok(locs
        .iter()
        .enumerate()
        .map(|(bit, loc)| PlacedBit {
            block: Block::BramContent,
            minor: loc.minor,
            frame_ofst: loc.frame_ofst,
            value: (content[bit / 64] >> (bit % 64)) & 1 == 1,
        })
        .collect())
}

/// The placed parity bits for one RAMB18 half, in absolute-bit order.
pub fn bram_parity_placed_bits(
    bram_y: u32,
    parity: &[u64; 32],
) -> Result<Vec<PlacedBit>, BitLocError> {
    let locs = bram_parity_locations(bram_y)?;
    Ok(locs
        .iter()
        .enumerate()
        .map(|(bit, loc)| PlacedBit {
            block: Block::BramParity,
            minor: loc.minor,
            frame_ofst: loc.frame_ofst,
            value: (parity[bit / 64] >> (bit % 64)) & 1 == 1,
        })
        .collect())
}

/// The placed content and parity bits for one RAMB36 tile: content words
/// 0..256 are the lower half at `lower_bram_y`, words 256..512 the upper
/// half at `upper_bram_y`; same split for the 64 parity words. The two
/// half Y values come from the device file; this module does not guess
/// the pairing.
pub fn bram36_placed_bits(
    lower_bram_y: u32,
    upper_bram_y: u32,
    content: &[u64; 512],
    parity: &[u64; 64],
) -> Result<Vec<PlacedBit>, BitLocError> {
    let mut lower_content = [0u64; 256];
    let mut upper_content = [0u64; 256];
    lower_content.copy_from_slice(&content[0..256]);
    upper_content.copy_from_slice(&content[256..512]);
    let mut lower_parity = [0u64; 32];
    let mut upper_parity = [0u64; 32];
    lower_parity.copy_from_slice(&parity[0..32]);
    upper_parity.copy_from_slice(&parity[32..64]);
    let mut out = bram_mem_placed_bits(lower_bram_y, &lower_content)?;
    out.extend(bram_parity_placed_bits(lower_bram_y, &lower_parity)?);
    out.extend(bram_mem_placed_bits(upper_bram_y, &upper_content)?);
    out.extend(bram_parity_placed_bits(upper_bram_y, &upper_parity)?);
    Ok(out)
}

// ---------------------------------------------------------------------------
// BRAM FASM
// ---------------------------------------------------------------------------

/// Format `words` (little-endian u64 words, word 0 = lowest bits) as one
/// upper-case hex literal of exactly `hex_digits` digits, MSB first.
fn words_to_hex(words: &[u64], hex_digits: usize) -> String {
    let mut s = String::with_capacity(hex_digits);
    for w in words.iter().rev() {
        s.push_str(&format!("{w:016X}"));
    }
    debug_assert_eq!(s.len(), hex_digits);
    s
}

/// FASM lines for one RAMB18 half: content then parity, whole-range form
/// from the day-1 scout report (`RAMB18_XxYy.INIT[16383:0] = 16384'h...`).
/// The `INITP[2047:0]` parity spelling follows the same pattern; bitgen-core
/// must confirm both feature names against the vendored prjuray segbits DB
/// and fail loud if either is absent.
pub fn bram18_fasm(bram_x: u32, bram_y: u32, half_content: &BramHalfContent) -> Vec<String> {
    let content_hex = words_to_hex(&half_content.content, 4096);
    let parity_hex = words_to_hex(&half_content.parity, 512);
    vec![
        format!("RAMB18_X{bram_x}Y{bram_y}.INIT[16383:0] = 16384'h{content_hex}"),
        format!("RAMB18_X{bram_x}Y{bram_y}.INITP[2047:0] = 2048'h{parity_hex}"),
    ]
}

/// FASM lines for one BRAM tile under a [`BramTileUse`]. A RAMB36 tile
/// emits `RAMB36_XxYy.INIT[32767:0]` and `INITP[4095:0]`; words 0..256 of
/// the value are the lower half, words 256..512 the upper half. A tile
/// used as RAMB18 halves emits one `RAMB18_XxYy` pair of lines per
/// configured half, addressed at `lower_bram_y` / `upper_bram_y`. The
/// RAMB36 tile Y is the tile's own Y; the half Y values come from the
/// device file.
pub fn bram_tile_fasm(
    bram_x: u32,
    bram_y: u32,
    lower_bram_y: u32,
    upper_bram_y: u32,
    tile_use: &BramTileUse,
) -> Vec<String> {
    match tile_use {
        BramTileUse::Ramb36 { content, parity } => {
            let content_hex = words_to_hex(content, 8192);
            let parity_hex = words_to_hex(parity, 1024);
            vec![
                format!("RAMB36_X{bram_x}Y{bram_y}.INIT[32767:0] = 32768'h{content_hex}"),
                format!("RAMB36_X{bram_x}Y{bram_y}.INITP[4095:0] = 4096'h{parity_hex}"),
            ]
        }
        BramTileUse::Ramb18 { lower, upper } => {
            let mut lines = Vec::new();
            if let Some(half) = lower {
                lines.extend(bram18_fasm(bram_x, lower_bram_y, half));
            }
            if let Some(half) = upper {
                lines.extend(bram18_fasm(bram_x, upper_bram_y, half));
            }
            lines
        }
    }
}

// ---------------------------------------------------------------------------
// URAM288 (blocked: no public encoding)
// ---------------------------------------------------------------------------

/// Content-bit locations for one URAM288 site (4096 words x 72 bits =
/// 294912 content bits). There is no table to read: prjuray-db has no
/// URAM mention and BitFiltrator ULTRASCALE_PLUS.json has no URAM table,
/// and URAM288 has no INIT attributes in UG974, so the content encoding
/// cannot even be swept the BRAM way. This is the seam where the Vivado
/// fuzzing job (Job 5, phase 3 probe) lands the table; until then every
/// call fails loud with [`BitLocError::UramBlocked`] and emits no bits.
/// `_uram_y` is the Vivado URAM288 site Y, kept so the signature matches
/// the BRAM location functions when the table lands.
pub fn uram_content_locations(_uram_y: u32) -> Result<Vec<BitLoc>, BitLocError> {
    Err(BitLocError::UramBlocked)
}
