//! BitFiltrator-schema device-file loader and BEL locator.
//!
//! Loads the two JSON files that define an UltraScale+ part in the
//! BitFiltrator schema:
//!
//! - the device file (`resources/devices/xcku3p.json` shape): per-SLR
//!   IDCODE and config order, clock-region to FAR-row ranges, and per
//!   row the CLB column majors, CLB tile types, BRAM content / parity /
//!   register column majors, DSP column majors, and minors-per-column
//!   tables;
//! - the architecture summary (`resources/archs/ULTRASCALE_PLUS.json`):
//!   per tile type (CLEL_L, CLEL_R, CLEM, CLEM_R) the LUT and FF bit
//!   locations (minor, frame offset) at every Y offset, plus BRAM
//!   content and parity locations.
//!
//! The locator reproduces `bit_locator.py` exactly: SLR from the BEL Y
//! coordinate, row major from Y and 60 CLBs per column, column major
//! and tile type from the device file, Y offset as Y mod 60, and the
//! (minor, frame offset) pairs from the architecture summary.
//!
//! FAR layout, UltraScale+ (UG570 table 9-21, via BitFiltrator
//! `arch_spec.py` UltraScalePlusSpec): reserved bits 31:27, block
//! type 26:24 (0 = CLB_IO_CLK, 1 = BRAM_CONTENT), row 23:18,
//! column 17:8, minor 7:0. Frame size is 93 32-bit words.
//!
//! Provisional values: a device file may carry a `helion_provisional`
//! object listing every value not copied from a confirmed source with
//! its source. The loader surfaces that list; nothing provisional is
//! ever presented as confirmed. The xcvu3p.json file is provisional in
//! full (see `crates/helion-ultrascale/data/README.md`).
//!
//! Known cross-check, recorded not hidden: summing
//! `num_minors_per_std_colMajor` plus
//! `num_minors_per_bram_content_colMajor` over all rows of xcku3p.json
//! gives 41,460 frames, while AMD UG570 Table 1 lists 41,476
//! configuration frames for KU3P. The gap is 16 frames, exactly 4 per
//! FAR row, systematic across every BitFiltrator device file checked.
//! `DeviceFile::total_frames` reports the file's own total; the UG570
//! delta is documented, not adjusted.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::sync::OnceLock;

use serde::Deserialize;

/// Errors from loading or querying a device file. Unknown encodings and
/// out-of-range lookups fail loud, never return a guessed value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceFileError {
    /// File could not be read.
    Io(String),
    /// JSON did not parse or did not match the schema.
    Json(String),
    /// A required key or entry is absent.
    Missing(String),
    /// A value is outside the range the file allows.
    OutOfRange(String),
    /// A BEL name does not parse or names no known BEL.
    UnknownBel(String),
    /// No such SLR in the device file.
    UnknownSlr(String),
    /// No such tile type in the architecture summary.
    UnknownTileType(String),
}

impl fmt::Display for DeviceFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceFileError::Io(m) => write!(f, "device file IO error: {m}"),
            DeviceFileError::Json(m) => write!(f, "device file JSON error: {m}"),
            DeviceFileError::Missing(m) => write!(f, "device file missing: {m}"),
            DeviceFileError::OutOfRange(m) => write!(f, "device file out of range: {m}"),
            DeviceFileError::UnknownBel(m) => write!(f, "unknown BEL: {m}"),
            DeviceFileError::UnknownSlr(m) => write!(f, "unknown SLR: {m}"),
            DeviceFileError::UnknownTileType(m) => write!(f, "unknown tile type: {m}"),
        }
    }
}

impl std::error::Error for DeviceFileError {}

/// FAR block type, UltraScale+ (UG570 table 9-21).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FarBlockType {
    /// CLB, IO, and clocking frames.
    ClbIoClk = 0,
    /// BRAM content frames.
    BramContent = 1,
}

impl FarBlockType {
    fn from_u32(v: u32) -> Option<Self> {
        match v {
            0 => Some(FarBlockType::ClbIoClk),
            1 => Some(FarBlockType::BramContent),
            _ => None,
        }
    }
}

/// A frame address register value, UltraScale+.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Far {
    /// Reserved bits 31:27, always 0 in practice.
    pub reserved: u32,
    /// Block type bits 26:24.
    pub block_type: FarBlockType,
    /// Row address bits 23:18 (clock-region row inside the SLR).
    pub row: u32,
    /// Column address bits 17:8 (major column).
    pub col: u32,
    /// Minor address bits 7:0 (frame inside the major column).
    pub minor: u32,
}

impl Far {
    /// Build a FAR, rejecting field values that do not fit their bit
    /// widths instead of silently truncating them.
    pub fn new(
        block_type: FarBlockType,
        row: u32,
        col: u32,
        minor: u32,
    ) -> Result<Self, DeviceFileError> {
        if row > 0x3f {
            return Err(DeviceFileError::OutOfRange(format!(
                "FAR row {row} exceeds 6 bits"
            )));
        }
        if col > 0x3ff {
            return Err(DeviceFileError::OutOfRange(format!(
                "FAR column {col} exceeds 10 bits"
            )));
        }
        if minor > 0xff {
            return Err(DeviceFileError::OutOfRange(format!(
                "FAR minor {minor} exceeds 8 bits"
            )));
        }
        Ok(Far {
            reserved: 0,
            block_type,
            row,
            col,
            minor,
        })
    }

    /// Encode to the 32-bit FAR word.
    pub fn to_u32(&self) -> u32 {
        (self.reserved << 27)
            | ((self.block_type as u32) << 24)
            | (self.row << 18)
            | (self.col << 8)
            | self.minor
    }

    /// Encode as 8 lowercase hex digits, BitFiltrator `to_hex` style.
    pub fn to_hex(&self) -> String {
        format!("{:08x}", self.to_u32())
    }

    /// Decode a 32-bit FAR word. Unknown block types fail loud.
    pub fn from_u32(v: u32) -> Result<Self, DeviceFileError> {
        let reserved = (v >> 27) & 0x1f;
        let block_raw = (v >> 24) & 0x7;
        let block_type = FarBlockType::from_u32(block_raw).ok_or_else(|| {
            DeviceFileError::OutOfRange(format!("FAR block type {block_raw} is not defined"))
        })?;
        Ok(Far {
            reserved,
            block_type,
            row: (v >> 18) & 0x3f,
            col: (v >> 8) & 0x3ff,
            minor: v & 0xff,
        })
    }
}

impl fmt::Display for Far {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BLOCK_TYPE = {:?}, ROW_ADDR = {:>3}, COL_ADDR = {:>3}, MINOR_ADDR = {:>3}",
            self.block_type, self.row, self.col, self.minor
        )
    }
}

/// One provisional-value note from a device file's `helion_provisional`
/// object: which field, what value was used, and where it came from.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ProvisionalNote {
    /// Field path in the device file, e.g. `slrs.SLR0.idcode`.
    pub field: String,
    /// The value used, as a string.
    pub value: String,
    /// Source of the value, e.g. `AMD UG570 Table 2`.
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ProvisionalBlock {
    #[serde(default)]
    provisional: bool,
    #[serde(default)]
    notes: Vec<ProvisionalNote>,
}

#[derive(Clone, Debug, Deserialize)]
struct RowMajorRaw {
    #[serde(default, rename = "bram_content_colMajors")]
    bram_content_col_majors: BTreeMap<u32, u32>,
    #[serde(default, rename = "bram_content_parity_colMajors")]
    bram_content_parity_col_majors: BTreeMap<u32, u32>,
    #[serde(default, rename = "bram_reg_colMajors")]
    bram_reg_col_majors: BTreeMap<u32, u32>,
    #[serde(default, rename = "dsp_colMajors")]
    dsp_col_majors: BTreeMap<u32, u32>,
    #[serde(default, rename = "clb_colMajors")]
    clb_col_majors: BTreeMap<u32, u32>,
    #[serde(default, rename = "clb_tileTypes")]
    clb_tile_types: BTreeMap<u32, String>,
    #[serde(rename = "num_minors_per_bram_content_colMajor")]
    num_minors_per_bram_content_col_major: Vec<u32>,
    #[serde(rename = "num_minors_per_std_colMajor")]
    num_minors_per_std_col_major: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize)]
struct SlrRaw {
    slr_idx: u32,
    config_order_idx: u32,
    idcode: String,
    min_clock_region_row_idx: u32,
    max_clock_region_row_idx: u32,
    min_clock_region_col_idx: u32,
    max_clock_region_col_idx: u32,
    min_far_row_idx: u32,
    max_far_row_idx: u32,
    #[serde(rename = "rowMajors")]
    row_majors: BTreeMap<u32, RowMajorRaw>,
}

#[derive(Clone, Debug, Deserialize)]
struct DeviceFileRaw {
    device: String,
    part: String,
    license: String,
    num_brams: u32,
    num_dsps: u32,
    num_luts: u32,
    num_regs: u32,
    num_slices: u32,
    num_slrs: u32,
    #[serde(rename = "tileType_siteType_pairs")]
    tile_type_site_type_pairs: Vec<(String, String)>,
    slrs: BTreeMap<String, SlrRaw>,
    #[serde(default)]
    helion_provisional: ProvisionalBlock,
}

/// A parsed BitFiltrator device file.
#[derive(Clone, Debug)]
pub struct DeviceFile {
    raw: DeviceFileRaw,
    /// Lazily computed tile-name list for the `BitgenDevice` impl
    /// (`bitgen_device.rs`). Computed on first use, never at load:
    /// most consumers (the BEL locator, the demo validation) never
    /// need the full tile enumeration.
    pub(crate) tile_names_cache: OnceLock<Vec<String>>,
    /// Lazily computed config-order FAR list for the `BitgenDevice`
    /// impl. Same laziness rationale as `tile_names_cache`.
    pub(crate) config_order_cache: OnceLock<Vec<u32>>,
}

impl DeviceFile {
    /// Load a device file from disk.
    pub fn from_path(path: &Path) -> Result<Self, DeviceFileError> {
        let text = fs::read_to_string(path)
            .map_err(|e| DeviceFileError::Io(format!("{}: {e}", path.display())))?;
        Self::from_str(&text)
    }

    /// Device name, e.g. `xcku3p`.
    pub fn device(&self) -> &str {
        &self.raw.device
    }

    /// Full part string, e.g. `xcku3p-ffva676-1-e`.
    pub fn part(&self) -> &str {
        &self.raw.part
    }

    /// License string recorded in the file.
    pub fn license(&self) -> &str {
        &self.raw.license
    }

    /// Aggregate counts from the file header. These are file truth, not
    /// derived from the column tables: the BRAM and DSP column tables
    /// do not reproduce num_brams / num_dsps by simple multiplication,
    /// so the loader never asserts those two.
    pub fn num_brams(&self) -> u32 {
        self.raw.num_brams
    }
    /// Aggregate DSP count from the file header.
    pub fn num_dsps(&self) -> u32 {
        self.raw.num_dsps
    }
    /// Aggregate LUT count from the file header.
    pub fn num_luts(&self) -> u32 {
        self.raw.num_luts
    }
    /// Aggregate register count from the file header.
    pub fn num_regs(&self) -> u32 {
        self.raw.num_regs
    }
    /// Aggregate slice count from the file header.
    pub fn num_slices(&self) -> u32 {
        self.raw.num_slices
    }
    /// Number of SLRs in the file.
    pub fn num_slrs(&self) -> u32 {
        self.raw.num_slrs
    }

    /// Tile type to site type pairs from the file.
    pub fn tile_type_site_type_pairs(&self) -> &[(String, String)] {
        &self.raw.tile_type_site_type_pairs
    }

    /// SLR names in file order.
    pub fn slr_names(&self) -> Vec<&str> {
        self.raw.slrs.keys().map(String::as_str).collect()
    }

    /// Row-major keys of an SLR's `rowMajors` table, sorted ascending.
    /// A row major is the FAR row index inside the SLR (the same index
    /// `BitLocator` passes to `Far::new` as the row); for both files on
    /// disk it runs 0..=max_far_row_idx.
    pub fn row_majors(&self, name: &str) -> Result<Vec<u32>, DeviceFileError> {
        Ok(self.slr(name)?.row_majors.keys().copied().collect())
    }

    /// SLR names ordered by config order index (configuration order).
    pub fn slr_names_by_config_order(&self) -> Vec<&str> {
        let mut names: Vec<(&str, u32)> = self
            .raw
            .slrs
            .iter()
            .map(|(n, s)| (n.as_str(), s.config_order_idx))
            .collect();
        names.sort_by_key(|(_, idx)| *idx);
        names.into_iter().map(|(n, _)| n).collect()
    }

    fn slr(&self, name: &str) -> Result<&SlrRaw, DeviceFileError> {
        self.raw
            .slrs
            .get(name)
            .ok_or_else(|| DeviceFileError::UnknownSlr(name.to_string()))
    }

    /// IDCODE of an SLR, parsed from its hex string in the file.
    pub fn slr_idcode(&self, name: &str) -> Result<u32, DeviceFileError> {
        let slr = self.slr(name)?;
        let text = slr.idcode.strip_prefix("0x").unwrap_or(&slr.idcode);
        u32::from_str_radix(text, 16)
            .map_err(|_| DeviceFileError::Json(format!("bad idcode {} for {name}", slr.idcode)))
    }

    /// Config order index of an SLR.
    pub fn slr_config_order_idx(&self, name: &str) -> Result<u32, DeviceFileError> {
        Ok(self.slr(name)?.config_order_idx)
    }

    /// Minimum clock-region row index of an SLR.
    /// SLR index of an SLR (its position in the device, not its
    /// configuration order).
    pub fn slr_idx(&self, name: &str) -> Result<u32, DeviceFileError> {
        Ok(self.slr(name)?.slr_idx)
    }

    /// Clock-region column index range of an SLR.
    pub fn slr_clock_region_col_range(&self, name: &str) -> Result<(u32, u32), DeviceFileError> {
        let slr = self.slr(name)?;
        Ok((slr.min_clock_region_col_idx, slr.max_clock_region_col_idx))
    }

    pub fn min_clock_region_row_idx(&self, name: &str) -> Result<u32, DeviceFileError> {
        Ok(self.slr(name)?.min_clock_region_row_idx)
    }

    /// Maximum clock-region row index of an SLR.
    pub fn max_clock_region_row_idx(&self, name: &str) -> Result<u32, DeviceFileError> {
        Ok(self.slr(name)?.max_clock_region_row_idx)
    }

    /// Minimum FAR row index of an SLR.
    pub fn min_far_row_idx(&self, name: &str) -> Result<u32, DeviceFileError> {
        Ok(self.slr(name)?.min_far_row_idx)
    }

    /// Maximum FAR row index of an SLR.
    pub fn max_far_row_idx(&self, name: &str) -> Result<u32, DeviceFileError> {
        Ok(self.slr(name)?.max_far_row_idx)
    }

    fn row_major(&self, slr: &str, row: u32) -> Result<&RowMajorRaw, DeviceFileError> {
        self.slr(slr)?
            .row_majors
            .get(&row)
            .ok_or_else(|| DeviceFileError::Missing(format!("rowMajor {row} in SLR {slr}")))
    }

    /// CLB column major for logical CLB column x in a row.
    pub fn clb_col_major(&self, slr: &str, row: u32, x: u32) -> Result<u32, DeviceFileError> {
        self.row_major(slr, row)?
            .clb_col_majors
            .get(&x)
            .copied()
            .ok_or_else(|| {
                DeviceFileError::OutOfRange(format!("no CLB column {x} in SLR {slr} row {row}"))
            })
    }

    /// CLB tile type for logical CLB column x in a row.
    pub fn clb_tile_type(&self, slr: &str, row: u32, x: u32) -> Result<&str, DeviceFileError> {
        self.row_major(slr, row)?
            .clb_tile_types
            .get(&x)
            .map(String::as_str)
            .ok_or_else(|| {
                DeviceFileError::OutOfRange(format!("no CLB tile type {x} in SLR {slr} row {row}"))
            })
    }

    /// Number of CLB columns in a row.
    pub fn num_clb_columns(&self, slr: &str, row: u32) -> Result<usize, DeviceFileError> {
        Ok(self.row_major(slr, row)?.clb_col_majors.len())
    }

    /// BRAM content column major for logical BRAM column x in a row.
    pub fn bram_content_col_major(
        &self,
        slr: &str,
        row: u32,
        x: u32,
    ) -> Result<u32, DeviceFileError> {
        self.row_major(slr, row)?
            .bram_content_col_majors
            .get(&x)
            .copied()
            .ok_or_else(|| {
                DeviceFileError::OutOfRange(format!("no BRAM column {x} in SLR {slr} row {row}"))
            })
    }

    /// Number of BRAM columns in a row.
    pub fn num_bram_columns(&self, slr: &str, row: u32) -> Result<usize, DeviceFileError> {
        Ok(self.row_major(slr, row)?.bram_content_col_majors.len())
    }

    /// BRAM register column major for logical BRAM column x in a row.
    pub fn bram_reg_col_major(&self, slr: &str, row: u32, x: u32) -> Result<u32, DeviceFileError> {
        self.row_major(slr, row)?
            .bram_reg_col_majors
            .get(&x)
            .copied()
            .ok_or_else(|| {
                DeviceFileError::OutOfRange(format!(
                    "no BRAM reg column {x} in SLR {slr} row {row}"
                ))
            })
    }

    /// DSP column major for logical DSP column x in a row.
    pub fn dsp_col_major(&self, slr: &str, row: u32, x: u32) -> Result<u32, DeviceFileError> {
        self.row_major(slr, row)?
            .dsp_col_majors
            .get(&x)
            .copied()
            .ok_or_else(|| {
                DeviceFileError::OutOfRange(format!("no DSP column {x} in SLR {slr} row {row}"))
            })
    }

    /// Minors per standard (CLB_IO_CLK) major column in a row, indexed
    /// by physical column major.
    pub fn num_minors_per_std_col_major(
        &self,
        slr: &str,
        row: u32,
    ) -> Result<&[u32], DeviceFileError> {
        Ok(&self.row_major(slr, row)?.num_minors_per_std_col_major)
    }

    /// Minors per BRAM content major column in a row, indexed by
    /// physical BRAM column major.
    pub fn num_minors_per_bram_content_col_major(
        &self,
        slr: &str,
        row: u32,
    ) -> Result<&[u32], DeviceFileError> {
        Ok(&self
            .row_major(slr, row)?
            .num_minors_per_bram_content_col_major)
    }

    /// Total configuration frames the file describes: the sum over
    /// every SLR and FAR row of the standard-column minors plus the
    /// BRAM-content-column minors. File truth, see the module docs for
    /// the known UG570 delta (4 frames per row on every BitFiltrator
    /// device file checked).
    pub fn total_frames(&self) -> u64 {
        let mut total: u64 = 0;
        for slr in self.raw.slrs.values() {
            for row in slr.row_majors.values() {
                total += row
                    .num_minors_per_std_col_major
                    .iter()
                    .map(|v| *v as u64)
                    .sum::<u64>();
                total += row
                    .num_minors_per_bram_content_col_major
                    .iter()
                    .map(|v| *v as u64)
                    .sum::<u64>();
            }
        }
        total
    }

    /// True when the file carries a `helion_provisional` block marking
    /// values as unconfirmed.
    pub fn is_provisional(&self) -> bool {
        self.raw.helion_provisional.provisional
    }

    /// Provisional-value notes, empty for confirmed files.
    pub fn provisional_notes(&self) -> &[ProvisionalNote] {
        &self.raw.helion_provisional.notes
    }

    /// Structural self-check: every column major a row maps to must be
    /// inside that row's minors table, and every CLB column must have
    /// both a major and a tile type. Returns the number of column
    /// entries checked. This does not check resource-count arithmetic
    /// (see `num_brams` docs).
    pub fn validate_structure(&self) -> Result<usize, DeviceFileError> {
        let mut checked = 0;
        for (slr_name, slr) in &self.raw.slrs {
            for (row_idx, row) in &slr.row_majors {
                let n_std = row.num_minors_per_std_col_major.len() as u32;
                let n_bram = row.num_minors_per_bram_content_col_major.len() as u32;
                for (x, major) in &row.clb_col_majors {
                    if *major >= n_std {
                        return Err(DeviceFileError::OutOfRange(format!(
                            "CLB column {x} major {major} beyond {n_std} std columns in {slr_name} row {row_idx}"
                        )));
                    }
                    if !row.clb_tile_types.contains_key(x) {
                        return Err(DeviceFileError::Missing(format!(
                            "CLB column {x} has no tile type in {slr_name} row {row_idx}"
                        )));
                    }
                    checked += 1;
                }
                for (x, major) in &row.bram_content_col_majors {
                    if *major >= n_bram {
                        return Err(DeviceFileError::OutOfRange(format!(
                            "BRAM column {x} major {major} beyond {n_bram} BRAM columns in {slr_name} row {row_idx}"
                        )));
                    }
                    checked += 1;
                }
                for (x, major) in &row.bram_content_parity_col_majors {
                    if *major >= n_bram {
                        return Err(DeviceFileError::OutOfRange(format!(
                            "BRAM parity column {x} major {major} beyond {n_bram} BRAM columns in {slr_name} row {row_idx}"
                        )));
                    }
                    checked += 1;
                }
                for (x, major) in &row.bram_reg_col_majors {
                    if *major >= n_std {
                        return Err(DeviceFileError::OutOfRange(format!(
                            "BRAM reg column {x} major {major} beyond {n_std} std columns in {slr_name} row {row_idx}"
                        )));
                    }
                    checked += 1;
                }
                for (x, major) in &row.dsp_col_majors {
                    if *major >= n_std {
                        return Err(DeviceFileError::OutOfRange(format!(
                            "DSP column {x} major {major} beyond {n_std} std columns in {slr_name} row {row_idx}"
                        )));
                    }
                    checked += 1;
                }
            }
        }
        Ok(checked)
    }
}

impl FromStr for DeviceFile {
    type Err = DeviceFileError;

    /// Parse a device file from its JSON text.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let raw: DeviceFileRaw =
            serde_json::from_str(text).map_err(|e| DeviceFileError::Json(e.to_string()))?;
        Ok(DeviceFile {
            raw,
            tile_names_cache: OnceLock::new(),
            config_order_cache: OnceLock::new(),
        })
    }
}

/// Y offset -> (minors, frame offsets) for one BRAM table.
type BramLocMap = BTreeMap<u32, (Vec<u32>, Vec<u32>)>;
/// Tile type -> Y offset -> BEL name -> (minors, frame offsets).
type LutLocMap = BTreeMap<String, BTreeMap<u32, BTreeMap<String, (Vec<u32>, Vec<u32>)>>>;
/// Tile type -> Y offset -> BEL name -> (minor, frame offset).
type RegLocMap = BTreeMap<String, BTreeMap<u32, BTreeMap<String, (u32, u32)>>>;
/// Y offset -> BEL name -> (minors, frame offsets) for one tile type.
type LutLocByY = BTreeMap<u32, BTreeMap<String, (Vec<u32>, Vec<u32>)>>;

/// Parsed BitFiltrator architecture summary (ULTRASCALE_PLUS.json).
///
/// Parsed from `serde_json::Value` instead of derive, because the file
/// mixes dynamic tile-type keys with stringified integer Y_ofst keys,
/// which derive-plus-flatten cannot express: serde_json's flattened
/// map path rejects integer keys parsed from strings.
#[derive(Clone, Debug)]
pub struct ArchSummary {
    bram_mem: BramLocMap,
    bram_parity: BramLocMap,
    lut: LutLocMap,
    reg: RegLocMap,
}

fn json_u32_array(value: &serde_json::Value, what: &str) -> Result<Vec<u32>, DeviceFileError> {
    value
        .as_array()
        .ok_or_else(|| DeviceFileError::Json(format!("{what} is not an array")))?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| DeviceFileError::Json(format!("{what} has a non-u32 entry")))
        })
        .collect()
}

fn json_u32(value: &serde_json::Value, what: &str) -> Result<u32, DeviceFileError> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| DeviceFileError::Json(format!("{what} is not a u32")))
}

fn json_object<'v>(
    value: &'v serde_json::Value,
    what: &str,
) -> Result<&'v serde_json::Map<String, serde_json::Value>, DeviceFileError> {
    value
        .as_object()
        .ok_or_else(|| DeviceFileError::Json(format!("{what} is not an object")))
}

fn json_get<'v>(
    value: &'v serde_json::Value,
    key: &str,
    what: &str,
) -> Result<&'v serde_json::Value, DeviceFileError> {
    value
        .get(key)
        .ok_or_else(|| DeviceFileError::Missing(format!("{what} has no {key}")))
}

fn parse_bram_table(value: &serde_json::Value, what: &str) -> Result<BramLocMap, DeviceFileError> {
    let y_ofst = json_get(value, "Y_ofst", what)?;
    let mut out = BTreeMap::new();
    for (k, entry) in json_object(y_ofst, &format!("{what} Y_ofst"))? {
        let y: u32 = k
            .parse()
            .map_err(|_| DeviceFileError::Json(format!("{what} Y_ofst key {k} is not a u32")))?;
        let minors = json_u32_array(json_get(entry, "minor", what)?, &format!("{what} minor"))?;
        let ofsts = json_u32_array(
            json_get(entry, "frame_ofst", what)?,
            &format!("{what} frame_ofst"),
        )?;
        if minors.len() != ofsts.len() {
            return Err(DeviceFileError::Json(format!(
                "{what} Y_ofst {y}: {} minors vs {} offsets",
                minors.len(),
                ofsts.len()
            )));
        }
        out.insert(y, (minors, ofsts));
    }
    Ok(out)
}

impl ArchSummary {
    /// Load an architecture summary from disk.
    pub fn from_path(path: &Path) -> Result<Self, DeviceFileError> {
        let text = fs::read_to_string(path)
            .map_err(|e| DeviceFileError::Io(format!("{}: {e}", path.display())))?;
        Self::from_str(&text)
    }

    /// Parse an architecture summary from its JSON text.
    /// (Implemented as `FromStr` below so clippy's
    /// `should_implement_trait` stays clean.)
    fn parse(text: &str) -> Result<Self, DeviceFileError> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| DeviceFileError::Json(e.to_string()))?;
        let root = json_object(&value, "arch summary")?;
        let bram = json_get(&value, "BRAM", "arch summary")?;
        let bram_mem = parse_bram_table(json_get(bram, "BramMemLoc", "BRAM")?, "BramMemLoc")?;
        let bram_parity = parse_bram_table(
            json_get(bram, "BramMemParityLoc", "BRAM")?,
            "BramMemParityLoc",
        )?;
        let mut lut = BTreeMap::new();
        let mut reg = BTreeMap::new();
        for (tile_type, tile) in root {
            if tile_type == "BRAM" {
                continue;
            }
            let lut_table = json_get(tile, "LutLoc", tile_type)?;
            let lut_y = json_get(lut_table, "Y_ofst", &format!("{tile_type} LutLoc"))?;
            let mut lut_by_y: LutLocByY = BTreeMap::new();
            for (k, entry) in json_object(lut_y, &format!("{tile_type} LutLoc Y_ofst"))? {
                let y: u32 = k.parse().map_err(|_| {
                    DeviceFileError::Json(format!("{tile_type} LutLoc Y_ofst key {k} is not a u32"))
                })?;
                let minors = json_get(entry, "minor", "LutLoc")?;
                let ofsts = json_get(entry, "frame_ofst", "LutLoc")?;
                let mut by_bel = BTreeMap::new();
                for (bel, minor_list) in json_object(minors, "LutLoc minor")? {
                    let ofst_list = json_get(ofsts, bel, "LutLoc frame_ofst")?;
                    let m = json_u32_array(minor_list, "LutLoc minor")?;
                    let o = json_u32_array(ofst_list, "LutLoc frame_ofst")?;
                    if m.len() != o.len() {
                        return Err(DeviceFileError::Json(format!(
                            "{tile_type} LutLoc {bel} Y_ofst {y}: {} minors vs {} offsets",
                            m.len(),
                            o.len()
                        )));
                    }
                    by_bel.insert(bel.clone(), (m, o));
                }
                lut_by_y.insert(y, by_bel);
            }
            lut.insert(tile_type.clone(), lut_by_y);
            let reg_table = json_get(tile, "RegLoc", tile_type)?;
            let reg_y = json_get(reg_table, "Y_ofst", &format!("{tile_type} RegLoc"))?;
            let mut reg_by_y: BTreeMap<u32, BTreeMap<String, (u32, u32)>> = BTreeMap::new();
            for (k, entry) in json_object(reg_y, &format!("{tile_type} RegLoc Y_ofst"))? {
                let y: u32 = k.parse().map_err(|_| {
                    DeviceFileError::Json(format!("{tile_type} RegLoc Y_ofst key {k} is not a u32"))
                })?;
                let minors = json_get(entry, "minor", "RegLoc")?;
                let ofsts = json_get(entry, "frame_ofst", "RegLoc")?;
                let mut by_bel = BTreeMap::new();
                for (bel, minor) in json_object(minors, "RegLoc minor")? {
                    let ofst = json_get(ofsts, bel, "RegLoc frame_ofst")?;
                    by_bel.insert(
                        bel.clone(),
                        (
                            json_u32(minor, "RegLoc minor")?,
                            json_u32(ofst, "RegLoc frame_ofst")?,
                        ),
                    );
                }
                reg_by_y.insert(y, by_bel);
            }
            reg.insert(tile_type.clone(), reg_by_y);
        }
        Ok(ArchSummary {
            bram_mem,
            bram_parity,
            lut,
            reg,
        })
    }

    /// Tile types present in the summary.
    pub fn tile_types(&self) -> Vec<&str> {
        self.lut.keys().map(String::as_str).collect()
    }

    /// LUT bit locations for a tile type, Y offset, and LUT name
    /// (e.g. `B6LUT`): (minors, frame offsets), one entry per INIT
    /// bit, in INIT order.
    pub fn lut_loc(
        &self,
        tile_type: &str,
        y_ofst: u32,
        bel_name: &str,
    ) -> Result<(&[u32], &[u32]), DeviceFileError> {
        let tile = self
            .lut
            .get(tile_type)
            .ok_or_else(|| DeviceFileError::UnknownTileType(tile_type.to_string()))?;
        let entry = tile.get(&y_ofst).ok_or_else(|| {
            DeviceFileError::OutOfRange(format!("no LutLoc Y_ofst {y_ofst} for {tile_type}"))
        })?;
        let (minors, ofsts) = entry.get(bel_name).ok_or_else(|| {
            DeviceFileError::UnknownBel(format!("no LUT {bel_name} in {tile_type} LutLoc"))
        })?;
        Ok((minors, ofsts))
    }

    /// FF bit location for a tile type, Y offset, and FF name (e.g.
    /// `BFF`): (minor, frame offset).
    pub fn reg_loc(
        &self,
        tile_type: &str,
        y_ofst: u32,
        bel_name: &str,
    ) -> Result<(u32, u32), DeviceFileError> {
        let tile = self
            .reg
            .get(tile_type)
            .ok_or_else(|| DeviceFileError::UnknownTileType(tile_type.to_string()))?;
        let entry = tile.get(&y_ofst).ok_or_else(|| {
            DeviceFileError::OutOfRange(format!("no RegLoc Y_ofst {y_ofst} for {tile_type}"))
        })?;
        let (minor, ofst) = entry.get(bel_name).ok_or_else(|| {
            DeviceFileError::UnknownBel(format!("no FF {bel_name} in {tile_type} RegLoc"))
        })?;
        Ok((*minor, *ofst))
    }

    /// BRAM content bit locations for a Y offset: (minors, frame
    /// offsets), one entry per content bit.
    pub fn bram_mem_loc_arrays(&self, y_ofst: u32) -> Result<(&[u32], &[u32]), DeviceFileError> {
        let (minors, ofsts) = self
            .bram_mem
            .get(&y_ofst)
            .ok_or_else(|| DeviceFileError::OutOfRange(format!("no BramMemLoc Y_ofst {y_ofst}")))?;
        Ok((minors, ofsts))
    }

    /// BRAM parity bit locations for a Y offset: (minors, frame
    /// offsets), one entry per parity bit.
    pub fn bram_parity_loc_arrays(&self, y_ofst: u32) -> Result<(&[u32], &[u32]), DeviceFileError> {
        let (minors, ofsts) = self.bram_parity.get(&y_ofst).ok_or_else(|| {
            DeviceFileError::OutOfRange(format!("no BramMemParityLoc Y_ofst {y_ofst}"))
        })?;
        Ok((minors, ofsts))
    }
}

impl FromStr for ArchSummary {
    type Err = DeviceFileError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

/// BEL locator reproducing `bit_locator.py` over a loaded device file
/// and architecture summary.
pub struct BitLocator<'a> {
    device: &'a DeviceFile,
    arch: &'a ArchSummary,
}

/// Result of resolving an 18K BRAM BEL: (SLR, content FARs, content
/// frame offsets, parity FARs, parity frame offsets).
pub type BramResolution = (String, Vec<Far>, Vec<u32>, Vec<Far>, Vec<u32>);

/// UG574: CLBs per column (clock-region height in slices).
pub const NUM_CLB_PER_COLUMN: u32 = 60;
/// UG574: 18K BRAMs per column (clock-region height in 18K blocks).
pub const NUM_18K_BRAM_PER_COLUMN: u32 = 24;

impl<'a> BitLocator<'a> {
    /// Build a locator over already-loaded files.
    pub fn new(device: &'a DeviceFile, arch: &'a ArchSummary) -> Self {
        BitLocator { device, arch }
    }

    /// Which SLR holds a BEL at absolute Y, given the number of that
    /// BEL kind per column. Mirrors `_get_slr_name`.
    fn slr_name(&self, bel_y: u32, num_per_column: u32) -> Result<String, DeviceFileError> {
        for slr_name in self.device.slr_names() {
            let min_row = self.device.min_clock_region_row_idx(slr_name)?;
            let max_row = self.device.max_clock_region_row_idx(slr_name)?;
            let min_y = min_row * num_per_column;
            let max_y = (max_row + 1) * num_per_column - 1;
            if min_y <= bel_y && bel_y <= max_y {
                return Ok(slr_name.to_string());
            }
        }
        Err(DeviceFileError::OutOfRange(format!(
            "no SLR holds BEL at Y {bel_y}"
        )))
    }

    /// Row major inside the SLR for a BEL at absolute Y, relative to
    /// the SLR's first row. Mirrors `_get_row_major`.
    fn row_major(
        &self,
        bel_y: u32,
        slr_name: &str,
        num_per_column: u32,
    ) -> Result<u32, DeviceFileError> {
        let min_row = self.device.min_clock_region_row_idx(slr_name)?;
        let max_row = self.device.max_clock_region_row_idx(slr_name)?;
        for row_major in min_row..=max_row {
            let min_y = row_major * num_per_column;
            let max_y = (row_major + 1) * num_per_column - 1;
            if min_y <= bel_y && bel_y <= max_y {
                return Ok(row_major - min_row);
            }
        }
        Err(DeviceFileError::OutOfRange(format!(
            "no row major holds BEL at Y {bel_y} in {slr_name}"
        )))
    }

    /// Parse `SLICE_X<x>Y<y>/<bel>` into (x, y, bel). The BEL is
    /// `[A-H]6LUT` or `[A-H]FF` / `[A-H]FF2`, as in `bit_locator.py`.
    fn parse_slice_bel(name: &str) -> Result<(u32, u32, String), DeviceFileError> {
        let rest = name
            .strip_prefix("SLICE_X")
            .ok_or_else(|| DeviceFileError::UnknownBel(name.to_string()))?;
        let (x_text, rest) = rest
            .split_once('Y')
            .ok_or_else(|| DeviceFileError::UnknownBel(name.to_string()))?;
        let (y_text, bel) = rest
            .split_once('/')
            .ok_or_else(|| DeviceFileError::UnknownBel(name.to_string()))?;
        let x: u32 = x_text
            .parse()
            .map_err(|_| DeviceFileError::UnknownBel(name.to_string()))?;
        let y: u32 = y_text
            .parse()
            .map_err(|_| DeviceFileError::UnknownBel(name.to_string()))?;
        if bel.len() < 2 || bel.len() > 5 {
            return Err(DeviceFileError::UnknownBel(name.to_string()));
        }
        Ok((x, y, bel.to_string()))
    }

    /// Resolve a LUT BEL to (SLR, FARs, frame offsets), one FAR and
    /// one offset per INIT bit, in INIT order.
    pub fn locate_lut(&self, name: &str) -> Result<(String, Vec<Far>, Vec<u32>), DeviceFileError> {
        let (bel_x, bel_y, bel_name) = Self::parse_slice_bel(name)?;
        if !is_lut_name(&bel_name) {
            return Err(DeviceFileError::UnknownBel(format!(
                "{name} is not a LUT BEL"
            )));
        }
        let slr_name = self.slr_name(bel_y, NUM_CLB_PER_COLUMN)?;
        let row_major = self.row_major(bel_y, &slr_name, NUM_CLB_PER_COLUMN)?;
        let col_major = self.device.clb_col_major(&slr_name, row_major, bel_x)?;
        let tile_type = self
            .device
            .clb_tile_type(&slr_name, row_major, bel_x)?
            .to_string();
        let y_ofst = bel_y % NUM_CLB_PER_COLUMN;
        let (minors, ofsts) = self.arch.lut_loc(&tile_type, y_ofst, &bel_name)?;
        let mut fars = Vec::with_capacity(minors.len());
        for minor in minors {
            fars.push(Far::new(
                FarBlockType::ClbIoClk,
                row_major,
                col_major,
                *minor,
            )?);
        }
        Ok((slr_name, fars, ofsts.to_vec()))
    }

    /// Resolve an FF BEL to (SLR, FAR, frame offset).
    pub fn locate_reg(&self, name: &str) -> Result<(String, Far, u32), DeviceFileError> {
        let (bel_x, bel_y, bel_name) = Self::parse_slice_bel(name)?;
        if !is_ff_name(&bel_name) {
            return Err(DeviceFileError::UnknownBel(format!(
                "{name} is not an FF BEL"
            )));
        }
        let slr_name = self.slr_name(bel_y, NUM_CLB_PER_COLUMN)?;
        let row_major = self.row_major(bel_y, &slr_name, NUM_CLB_PER_COLUMN)?;
        let col_major = self.device.clb_col_major(&slr_name, row_major, bel_x)?;
        let tile_type = self
            .device
            .clb_tile_type(&slr_name, row_major, bel_x)?
            .to_string();
        let y_ofst = bel_y % NUM_CLB_PER_COLUMN;
        let (minor, ofst) = self.arch.reg_loc(&tile_type, y_ofst, &bel_name)?;
        let far = Far::new(FarBlockType::ClbIoClk, row_major, col_major, minor)?;
        Ok((slr_name, far, ofst))
    }

    /// Parse `RAMB18_X<x>Y<y>` into (x, y). Only 18K BRAM encodings
    /// are supported, as in `bit_locator.py`.
    fn parse_bram_bel(name: &str) -> Result<(u32, u32), DeviceFileError> {
        let rest = name
            .strip_prefix("RAMB18_X")
            .ok_or_else(|| DeviceFileError::UnknownBel(name.to_string()))?;
        let (x_text, y_text) = rest
            .split_once('Y')
            .ok_or_else(|| DeviceFileError::UnknownBel(name.to_string()))?;
        let x: u32 = x_text
            .parse()
            .map_err(|_| DeviceFileError::UnknownBel(name.to_string()))?;
        let y: u32 = y_text
            .parse()
            .map_err(|_| DeviceFileError::UnknownBel(name.to_string()))?;
        Ok((x, y))
    }

    /// Resolve an 18K BRAM BEL to (SLR, content FARs, content frame
    /// offsets, parity FARs, parity frame offsets).
    pub fn locate_bram(&self, name: &str) -> Result<BramResolution, DeviceFileError> {
        let (bel_x, bel_y) = Self::parse_bram_bel(name)?;
        let slr_name = self.slr_name(bel_y, NUM_18K_BRAM_PER_COLUMN)?;
        let row_major = self.row_major(bel_y, &slr_name, NUM_18K_BRAM_PER_COLUMN)?;
        let col_major = self
            .device
            .bram_content_col_major(&slr_name, row_major, bel_x)?;
        let y_ofst = bel_y % NUM_18K_BRAM_PER_COLUMN;
        let (mem_minors, mem_ofsts) = self.arch.bram_mem_loc_arrays(y_ofst)?;
        let (par_minors, par_ofsts) = self.arch.bram_parity_loc_arrays(y_ofst)?;
        let mut mem_fars = Vec::with_capacity(mem_minors.len());
        for minor in mem_minors {
            mem_fars.push(Far::new(
                FarBlockType::BramContent,
                row_major,
                col_major,
                *minor,
            )?);
        }
        let mut par_fars = Vec::with_capacity(par_minors.len());
        for minor in par_minors {
            par_fars.push(Far::new(
                FarBlockType::BramContent,
                row_major,
                col_major,
                *minor,
            )?);
        }
        Ok((
            slr_name,
            mem_fars,
            mem_ofsts.to_vec(),
            par_fars,
            par_ofsts.to_vec(),
        ))
    }

    /// Check every CLB BEL in the file: locate all 8 LUTs and 16 FFs
    /// of every slice and assert each resolved minor fits inside its
    /// column's minors table, i.e. every FAR maps to a real tile.
    /// Returns the number of (BEL, bit) pairs checked.
    pub fn check_all_clb_bels(&self) -> Result<u64, DeviceFileError> {
        let mut checked: u64 = 0;
        for slr_name in self.device.slr_names() {
            let min_row = self.device.min_clock_region_row_idx(slr_name)?;
            let max_row = self.device.max_clock_region_row_idx(slr_name)?;
            for abs_row in min_row..=max_row {
                let row_major = abs_row - min_row;
                let n_cols = self.device.num_clb_columns(slr_name, row_major)?;
                let minors_table = self
                    .device
                    .num_minors_per_std_col_major(slr_name, row_major)?;
                for x in 0..n_cols as u32 {
                    let col_major = self.device.clb_col_major(slr_name, row_major, x)?;
                    let n_minors = minors_table[col_major as usize];
                    for y_ofst in 0..NUM_CLB_PER_COLUMN {
                        let bel_y = abs_row * NUM_CLB_PER_COLUMN + y_ofst;
                        for letter in b'A'..=b'H' {
                            let lut = format!("SLICE_X{x}Y{bel_y}/{}6LUT", letter as char);
                            let (_, fars, _) = self.locate_lut(&lut)?;
                            for far in &fars {
                                if far.minor >= n_minors {
                                    return Err(DeviceFileError::OutOfRange(format!(
                                        "{lut}: minor {} beyond {n_minors} in column {col_major}",
                                        far.minor
                                    )));
                                }
                                checked += 1;
                            }
                            for suffix in ["FF", "FF2"] {
                                let ff = format!("SLICE_X{x}Y{bel_y}/{}{suffix}", letter as char);
                                let (_, far, _) = self.locate_reg(&ff)?;
                                if far.minor >= n_minors {
                                    return Err(DeviceFileError::OutOfRange(format!(
                                        "{ff}: minor {} beyond {n_minors} in column {col_major}",
                                        far.minor
                                    )));
                                }
                                checked += 1;
                            }
                        }
                    }
                }
            }
        }
        Ok(checked)
    }

    /// Check every BRAM BEL in the file: locate every 18K BRAM and
    /// assert each content and parity minor fits inside its column's
    /// minors table. Returns the number of (BEL, bit) pairs checked.
    pub fn check_all_bram_bels(&self) -> Result<u64, DeviceFileError> {
        let mut checked: u64 = 0;
        for slr_name in self.device.slr_names() {
            let min_row = self.device.min_clock_region_row_idx(slr_name)?;
            let max_row = self.device.max_clock_region_row_idx(slr_name)?;
            for abs_row in min_row..=max_row {
                let row_major = abs_row - min_row;
                let n_cols = self.device.num_bram_columns(slr_name, row_major)?;
                let minors_table = self
                    .device
                    .num_minors_per_bram_content_col_major(slr_name, row_major)?;
                for x in 0..n_cols as u32 {
                    let col_major = self.device.bram_content_col_major(slr_name, row_major, x)?;
                    let n_minors = minors_table[col_major as usize];
                    for y_ofst in 0..NUM_18K_BRAM_PER_COLUMN {
                        let bel_y = abs_row * NUM_18K_BRAM_PER_COLUMN + y_ofst;
                        let bram = format!("RAMB18_X{x}Y{bel_y}");
                        let (_, mem_fars, _, par_fars, _) = self.locate_bram(&bram)?;
                        for far in mem_fars.iter().chain(par_fars.iter()) {
                            if far.minor >= n_minors {
                                return Err(DeviceFileError::OutOfRange(format!(
                                    "{bram}: minor {} beyond {n_minors} in BRAM column {col_major}",
                                    far.minor
                                )));
                            }
                            checked += 1;
                        }
                    }
                }
            }
        }
        Ok(checked)
    }
}

fn is_lut_name(bel: &str) -> bool {
    // [A-H]6LUT
    bel.len() == 5 && (b'A'..=b'H').contains(&bel.as_bytes()[0]) && bel.ends_with("6LUT")
}

fn is_ff_name(bel: &str) -> bool {
    // [A-H]FF or [A-H]FF2
    (bel.len() == 3 || bel.len() == 4)
        && (b'A'..=b'H').contains(&bel.as_bytes()[0])
        && bel[1..].starts_with("FF")
        && (bel.len() == 3 || bel.as_bytes()[3] == b'2')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn data_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data")
    }

    fn load_xcku3p() -> (DeviceFile, ArchSummary) {
        let device =
            DeviceFile::from_path(&data_dir().join("xcku3p.json")).expect("xcku3p.json must load");
        let arch = ArchSummary::from_path(&data_dir().join("ULTRASCALE_PLUS.json"))
            .expect("ULTRASCALE_PLUS.json must load");
        (device, arch)
    }

    #[test]
    fn xcku3p_loads_with_header_counts() {
        let (device, _) = load_xcku3p();
        assert_eq!(device.device(), "xcku3p");
        assert_eq!(device.part(), "xcku3p-ffva676-1-e");
        assert_eq!(device.num_slrs(), 1);
        assert_eq!(device.slr_names(), vec!["SLR0"]);
        assert_eq!(device.slr_names_by_config_order(), vec!["SLR0"]);
        assert_eq!(device.slr_idcode("SLR0").unwrap(), 0x04a6_3093);
        assert_eq!(device.slr_config_order_idx("SLR0").unwrap(), 0);
        assert_eq!(device.max_far_row_idx("SLR0").unwrap(), 3);
        assert_eq!(device.num_slices(), 27_120);
        assert_eq!(device.num_luts(), 162_720);
        assert_eq!(device.num_regs(), 325_440);
        assert_eq!(device.num_brams(), 360);
        assert_eq!(device.num_dsps(), 1368);
        assert!(!device.is_provisional());
        assert!(device.provisional_notes().is_empty());
        // File truth: 41,460 frames. AMD UG570 Table 1 lists 41,476
        // for KU3P, a 16-frame gap (4 per FAR row), systematic across
        // every BitFiltrator device file checked. Documented in the
        // module docs and data/README.md, not adjusted here.
        assert_eq!(device.total_frames(), 41_460);
    }

    #[test]
    fn xcku3p_structure_validates_and_clb_counts_agree() {
        let (device, _) = load_xcku3p();
        let checked = device
            .validate_structure()
            .expect("structure must validate");
        assert!(checked > 0);
        assert_eq!(device.slr_idx("SLR0").unwrap(), 0);
        assert_eq!(device.slr_clock_region_col_range("SLR0").unwrap(), (0, 3));
        // CLB arithmetic agrees with the header slice count: 113
        // columns x 4 rows x 60 slices = 27,120 slices.
        let mut slices = 0;
        for row in 0..=3 {
            slices += device.num_clb_columns("SLR0", row).unwrap() * 60;
        }
        assert_eq!(slices, device.num_slices() as usize);
        // NOTE: the header LUT/FF counts do NOT follow the arch
        // geometry (8 LUTs and 16 FFs per slice). The file says
        // 162,720 LUTs and 325,440 FFs for 27,120 slices, exactly
        // 6 LUTs and 12 FFs per slice, 3/4 of the geometric count.
        // Both header values come from Vivado part properties via
        // create_device_summary.py, so they are reported as file
        // truth and never derived from geometry. The 3/4 ratio is
        // an open question (Vivado part-property counting vs
        // physical sites), recorded in data/README.md. The loader
        // must not assert num_luts == num_slices * 8.
        assert_eq!(device.num_luts(), 162_720);
        assert_eq!(device.num_regs(), 325_440);
    }

    #[test]
    fn far_encode_decode_roundtrip() {
        // Hand-checked: block 0, row 0, col 48, minor 7 is 0x00003007.
        let far = Far::new(FarBlockType::ClbIoClk, 0, 48, 7).unwrap();
        assert_eq!(far.to_u32(), 0x0000_3007);
        assert_eq!(far.to_hex(), "00003007");
        assert_eq!(Far::from_u32(0x0000_3007).unwrap(), far);
        // BRAM content block: block 1, row 0, col 0, minor 0.
        let bram = Far::new(FarBlockType::BramContent, 0, 0, 0).unwrap();
        assert_eq!(bram.to_u32(), 0x0100_0000);
        assert_eq!(Far::from_u32(0x0100_0000).unwrap(), bram);
        // Out-of-range fields fail loud instead of truncating.
        assert!(Far::new(FarBlockType::ClbIoClk, 64, 0, 0).is_err());
        assert!(Far::new(FarBlockType::ClbIoClk, 0, 1024, 0).is_err());
        assert!(Far::new(FarBlockType::ClbIoClk, 0, 0, 256).is_err());
        assert!(Far::from_u32(0x0200_0000).is_err());
    }

    #[test]
    fn locate_b6lut_x23y45_matches_bitfiltrator() {
        // Reference values from BitFiltrator bit_locator.py run on
        // 2026-11-07, hand-checked: X=23 maps to col major 48, tile
        // CLEL_L; Y=45 is row 0, Y offset 45. FAR for minor 7 is
        // (48 << 8) | 7 = 12295 = 0x3007.
        let (device, arch) = load_xcku3p();
        let loc = BitLocator::new(&device, &arch);
        let (slr, fars, ofsts) = loc.locate_lut("SLICE_X23Y45/B6LUT").unwrap();
        assert_eq!(slr, "SLR0");
        assert_eq!(fars.len(), 64);
        assert_eq!(ofsts.len(), 64);
        let minors: Vec<u32> = fars.iter().map(|f| f.minor).collect();
        assert_eq!(&minors[0..8], &[7, 6, 5, 4, 7, 6, 5, 4]);
        assert!(minors.iter().all(|m| [7, 6, 5, 4].contains(m)));
        assert_eq!(fars[0].to_hex(), "00003007");
        assert_eq!(fars[1].to_hex(), "00003006");
        assert_eq!(fars[2].to_hex(), "00003005");
        assert_eq!(fars[3].to_hex(), "00003004");
        assert!(fars.iter().all(|f| f.row == 0 && f.col == 48));
        assert_eq!(
            &ofsts[0..8],
            &[2271, 2271, 2271, 2271, 2270, 2270, 2270, 2270]
        );
        // Offsets descend one per four INIT bits: last is 2271 - 15.
        assert_eq!(ofsts[63], 2256);
        // A6LUT in the same slice uses minors 11..8.
        let (_, a_fars, _) = loc.locate_lut("SLICE_X23Y45/A6LUT").unwrap();
        assert_eq!(a_fars[0].to_hex(), "0000300b");
        assert_eq!(a_fars[1].to_hex(), "0000300a");
    }

    #[test]
    fn locate_ff_x23y45_and_x0y0_match_bitfiltrator() {
        let (device, arch) = load_xcku3p();
        let loc = BitLocator::new(&device, &arch);
        let (slr, far, ofst) = loc.locate_reg("SLICE_X23Y45/BFF").unwrap();
        assert_eq!(slr, "SLR0");
        assert_eq!(far.to_hex(), "0000300c");
        assert_eq!(far.to_u32(), 12_300);
        assert_eq!((far.row, far.col, far.minor), (0, 48, 12));
        assert_eq!(ofst, 2260);
        let (_, far0, ofst0) = loc.locate_reg("SLICE_X0Y0/AFF").unwrap();
        assert_eq!(far0.to_hex(), "0000050c");
        assert_eq!((far0.row, far0.col, far0.minor), (0, 5, 12));
        assert_eq!(ofst0, 2);
    }

    #[test]
    fn locate_bram_x0y0_matches_bitfiltrator() {
        let (device, arch) = load_xcku3p();
        let loc = BitLocator::new(&device, &arch);
        let (slr, mem_fars, mem_ofsts, par_fars, par_ofsts) =
            loc.locate_bram("RAMB18_X0Y0").unwrap();
        assert_eq!(slr, "SLR0");
        assert_eq!(mem_fars.len(), 16_384);
        assert_eq!(mem_ofsts.len(), 16_384);
        assert_eq!(par_fars.len(), 2_048);
        assert_eq!(par_ofsts.len(), 2_048);
        assert_eq!(mem_fars[0].to_hex(), "01000000");
        assert_eq!(mem_ofsts[0], 0);
        assert_eq!(par_fars[0].to_hex(), "01000000");
        assert_eq!(par_ofsts[0], 48);
        // Content spans 256 distinct FARs (minors 0..255), all in
        // BRAM_CONTENT block, row 0, column 0.
        let distinct: std::collections::HashSet<u32> =
            mem_fars.iter().map(|f| f.to_u32()).collect();
        assert_eq!(distinct.len(), 256);
        assert!(
            mem_fars
                .iter()
                .all(|f| f.block_type == FarBlockType::BramContent && f.row == 0 && f.col == 0)
        );
    }

    #[test]
    fn unknown_bel_names_fail_loud() {
        let (device, arch) = load_xcku3p();
        let loc = BitLocator::new(&device, &arch);
        assert!(loc.locate_lut("SLICE_X23Y45/Z6LUT").is_err());
        assert!(loc.locate_lut("SLICE_X23Y45/BFF").is_err());
        assert!(loc.locate_reg("SLICE_X23Y45/B6LUT").is_err());
        assert!(loc.locate_lut("SLICE_X999Y999/A6LUT").is_err());
        assert!(loc.locate_reg("SLICE_X0Y999/AFF").is_err());
        assert!(loc.locate_bram("RAMB36_X0Y0").is_err());
        assert!(loc.locate_bram("RAMB18_X99Y0").is_err());
        assert!(loc.locate_lut("not-a-bel").is_err());
    }

    #[test]
    fn xcku3p_every_clb_far_maps_to_a_real_tile() {
        // Acceptance check: every LUT and FF BEL of every slice
        // resolves to a minor inside its column's minors table.
        // Expected pairs: 27,120 slices x (8 LUTs x 64 bits + 16 FFs).
        let (device, arch) = load_xcku3p();
        let loc = BitLocator::new(&device, &arch);
        let checked = loc.check_all_clb_bels().unwrap();
        assert_eq!(checked, 27_120 * (8 * 64 + 16));
    }

    #[test]
    fn xcku3p_every_bram_far_maps_to_a_real_tile() {
        // Expected pairs: 10 cols x 4 rows x 24 Y x (16,384 + 2,048).
        let (device, arch) = load_xcku3p();
        let loc = BitLocator::new(&device, &arch);
        let checked = loc.check_all_bram_bels().unwrap();
        assert_eq!(checked, 10 * 4 * 24 * (16_384 + 2_048));
    }
}
