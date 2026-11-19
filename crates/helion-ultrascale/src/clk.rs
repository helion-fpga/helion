//! Clocking (CLK block) emission: row-clock and CMT features that exist
//! in public sources, plus fail-loud handling for the ones that do not.
//!
//! Ground truth is the vendored prjuray-db segbits data (CC0 1.0); the
//! full inventory is in `data/CLK_AUDIT.md`. What this module does:
//!
//! - Resolves every clocking feature that has a DB line, by exact
//!   feature lookup, never by a polarity rule: CMT HROUTE/HDISTR PIP
//!   and USED features (`CMT_RIGHT`), RCLK distribution PIPs and USED
//!   bits (all fifteen `segbits_rclk_*.db` tile types), BUFCE_LEAF
//!   site config (`RCLK_INT_L` / `RCLK_INT_R`), and BUFGCE_HDIO input
//!   inversion (`RCLK_HDIO`).
//! - Fails loud, with the exact missing encoding named, for every
//!   clocking site whose config has no public encoding: CMT BUFGCE,
//!   BUFGCTRL, BUFGCE_DIV, BUFCE_ROW, BUFCE_ROW_FSR, MMCME4_ADV, and
//!   PLLE4_ADV. A design using one of those sites cannot emit bits for
//!   it, and this module never invents them.
//!
//! Resolution here is tile-relative: the returned `BitRef`s are
//! (frame, bit) offsets inside the tile, exactly as the segbits DB
//! stores them. Absolute FARs need the tile's base FAR, and no public
//! source gives base FARs for CMT or RCLK tiles (no xcku3p/xcvu3p
//! tilegrid; `bitgen_device.rs` returns `None` for those tiles, so
//! `emit_bitstream` fails loud with `UnknownTile` on them today).
//! The diff harness compares CLK features tile-relatively for the
//! same reason (`harness/reference.py`, `resolve_cmt_pip`).
//!
//! FASM spellings follow the DB heads exactly: site features are
//! `<site>.<bel>.<attr>` under the tile type, e.g.
//! `BUFCE_LEAF_X0Y0.BUFCE_LEAF.CE_TYPE.SYNC`, and PIPs are sink-first
//! `PIP.<sink>.<source>`, as in `int_pip.rs`.
//!
//! No em dashes anywhere.

use crate::segbits::{BitRef, SegbitsDb};
use std::fmt;

/// Failure to resolve or validate a clocking feature. Every variant
/// names the site, feature, or attribute and why it cannot emit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClkError {
    /// No DB line exists for this (tile type, feature). The feature
    /// string may be misspelled, or the encoding may not exist publicly.
    UnknownFeature { tile_type: String, feature: String },
    /// The site and attribute are real (the site-type JSON documents
    /// the pin or pip), but no public segbits line encodes the
    /// attribute. Unblocker is the Vivado clocking fuzz job
    /// (`data/CLK_FUZZ_JOB.md`, Job 4). Never emit guessed bits.
    MissingEncoding {
        site: String,
        attribute: String,
        detail: String,
    },
    /// A config value has no DB line (e.g. a BUFCE_LEAF delay tap
    /// outside {0, 1, 2, 4, 8}). The value is rejected, not rounded.
    InvalidValue {
        site: String,
        attribute: String,
        value: String,
    },
    /// A site-pip name is not in the site's inventory at all (not a
    /// known pip lacking an encoding, but an unknown name).
    UnknownPip { site: String, pip: String },
}

impl fmt::Display for ClkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFeature { tile_type, feature } => {
                write!(f, "unknown clocking feature {tile_type}.{feature}")
            }
            Self::MissingEncoding {
                site,
                attribute,
                detail,
            } => write!(f, "no public encoding for {site} {attribute}: {detail}"),
            Self::InvalidValue {
                site,
                attribute,
                value,
            } => write!(
                f,
                "invalid value {value} for {site} {attribute}: no DB line encodes it"
            ),
            Self::UnknownPip { site, pip } => {
                write!(f, "unknown site pip {pip} for {site}")
            }
        }
    }
}

impl std::error::Error for ClkError {}

/// One resolved clocking feature: its FASM line plus the exact bits it
/// controls, split into bits to set and bits that must read 0.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedClkFeature {
    pub tile: String,
    pub feature: String,
    pub set_bits: Vec<BitRef>,
    pub zero_bits: Vec<BitRef>,
}

impl ResolvedClkFeature {
    pub fn fasm_line(&self) -> String {
        format!("{}.{}", self.tile, self.feature)
    }
}

/// A clocking feature the design needed that has no DB entry. Kept
/// exact: tile type, feature, and design. This list is the input to
/// the CLK completeness record in the diff harness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingClkFeature {
    pub tile_type: String,
    pub feature: String,
    pub design: String,
}

impl fmt::Display for MissingClkFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{} (design {})",
            self.tile_type, self.feature, self.design
        )
    }
}

/// Partition of a requested feature list into resolved and missing.
/// Emission fails loud on a non-empty missing list; the partition
/// itself never drops a request silently.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ClkResolution {
    pub resolved: Vec<ResolvedClkFeature>,
    pub missing: Vec<MissingClkFeature>,
}

/// Look up one clocking feature by exact DB key. Returns the raw
/// `BitRef` list (set and zero bits in file order), or
/// `ClkError::UnknownFeature`.
pub fn resolve_clk_feature(
    db: &dyn SegbitsDb,
    tile_type: &str,
    feature: &str,
) -> Result<Vec<BitRef>, ClkError> {
    db.lookup(tile_type, feature)
        .map(|refs| refs.to_vec())
        .ok_or_else(|| ClkError::UnknownFeature {
            tile_type: tile_type.to_string(),
            feature: feature.to_string(),
        })
}

/// Resolve a list of (tile, tile type, feature) requests, partitioning
/// into resolved and missing. `tile` is the Vivado tile name for the
/// FASM line (e.g. `RCLK_INT_L_X0Y0`); `tile_type` is the segbits tile
/// type (e.g. `RCLK_INT_L`).
pub fn resolve_clk_features(
    db: &dyn SegbitsDb,
    requests: &[(&str, &str, &str)],
    design: &str,
) -> ClkResolution {
    let mut out = ClkResolution::default();
    for (tile, tile_type, feature) in requests {
        match db.lookup(tile_type, feature) {
            Some(refs) => {
                let (set_bits, zero_bits): (Vec<BitRef>, Vec<BitRef>) =
                    refs.iter().partition(|r| !r.must_be_zero);
                out.resolved.push(ResolvedClkFeature {
                    tile: (*tile).to_string(),
                    feature: (*feature).to_string(),
                    set_bits,
                    zero_bits,
                });
            }
            None => out.missing.push(MissingClkFeature {
                tile_type: (*tile_type).to_string(),
                feature: (*feature).to_string(),
                design: design.to_string(),
            }),
        }
    }
    out
}

/// FASM lines for resolved features, one per line, no values:
/// `<tile>.<feature>`.
pub fn clk_features_to_fasm(resolved: &[ResolvedClkFeature]) -> String {
    let mut text = String::new();
    for r in resolved {
        text.push_str(&r.fasm_line());
        text.push('\n');
    }
    text
}

// ---------------------------------------------------------------------------
// BUFCE_LEAF
// ---------------------------------------------------------------------------

/// BUFCE_LEAF site config, as encoded in `segbits_rclk_int_l.db` and
/// `segbits_rclk_int_r.db` (32 sites per tile type, 13 features each).
///
/// - `ce_type_sync`: false selects `CE_TYPE.ASYNC` (sets the bit),
///   true selects `CE_TYPE.SYNC` (clears it). The pair is complementary
///   on one bit; the DB line, not a rule, decides which spelling sets it.
/// - `ce_inverted`, `input_inverted`, `in_use`: each selects the `.V1`
///   spelling when true (sets the bit) and `.V0` when false (clears it),
///   again exactly as the DB lines read.
/// - `delay_tap`: must be one of {0, 1, 2, 4, 8}; those are the only
///   `DELAY_TAP.V*` lines in the DB. Any other value is rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufceLeafConfig {
    pub ce_type_sync: bool,
    pub ce_inverted: bool,
    pub delay_tap: u8,
    pub input_inverted: bool,
    pub in_use: bool,
}

impl BufceLeafConfig {
    /// The config of a leaf that is used with defaults: synchronous CE,
    /// nothing inverted, delay tap 0, in use. Every field is still
    /// resolved from DB lines; defaults here are attribute values, not
    /// invented bits.
    pub fn default_used() -> Self {
        Self {
            ce_type_sync: true,
            ce_inverted: false,
            delay_tap: 0,
            input_inverted: false,
            in_use: true,
        }
    }

    /// The five feature strings for `site` (e.g. `BUFCE_LEAF_X0Y0`),
    /// in DB order: CE_TYPE, CEINV, DELAY_TAP, IINV, IN_USE.
    pub fn features(&self, site: &str) -> Result<Vec<String>, ClkError> {
        if !matches!(self.delay_tap, 0 | 1 | 2 | 4 | 8) {
            return Err(ClkError::InvalidValue {
                site: site.to_string(),
                attribute: "BUFCE_LEAF.DELAY_TAP".to_string(),
                value: self.delay_tap.to_string(),
            });
        }
        let ce_type = if self.ce_type_sync { "SYNC" } else { "ASYNC" };
        let v = |b: bool| if b { "V1" } else { "V0" };
        Ok(vec![
            format!("{site}.BUFCE_LEAF.CE_TYPE.{ce_type}"),
            format!("{site}.BUFCE_LEAF.CEINV.{}", v(self.ce_inverted)),
            format!("{site}.BUFCE_LEAF.DELAY_TAP.V{}", self.delay_tap),
            format!("{site}.BUFCE_LEAF.IINV.{}", v(self.input_inverted)),
            format!("{site}.BUFCE_LEAF.IN_USE.{}", v(self.in_use)),
        ])
    }
}

/// Resolve a BUFCE_LEAF config to its exact bits. Every feature must
/// resolve in the DB for `tile_type` (`RCLK_INT_L` or `RCLK_INT_R`);
/// a missing line fails loud with `UnknownFeature`.
pub fn resolve_bufce_leaf(
    db: &dyn SegbitsDb,
    tile_type: &str,
    site: &str,
    config: &BufceLeafConfig,
) -> Result<Vec<BitRef>, ClkError> {
    let mut bits = Vec::new();
    for feature in config.features(site)? {
        bits.extend(resolve_clk_feature(db, tile_type, &feature)?);
    }
    Ok(bits)
}

// ---------------------------------------------------------------------------
// BUFGCE_HDIO
// ---------------------------------------------------------------------------

/// BUFGCE_HDIO input inversion, the only BUFGCE_HDIO attribute with a
/// public encoding (`segbits_rclk_hdio.db`, 4 sites, IINV V0/V1 only).
/// CE inversion and CE type have no lines for this site and are not
/// representable here; requesting them is a `MissingEncoding`, raised
/// by `bufgce_hdio_ce_blocked`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufgceHdioConfig {
    pub input_inverted: bool,
}

impl BufgceHdioConfig {
    pub fn feature(&self, site: &str) -> String {
        let v = if self.input_inverted { "V1" } else { "V0" };
        format!("{site}.BUFGCE_HDIO.IINV.{v}")
    }
}

/// Resolve a BUFGCE_HDIO IINV feature to its exact bit.
pub fn resolve_bufgce_hdio(
    db: &dyn SegbitsDb,
    site: &str,
    config: &BufgceHdioConfig,
) -> Result<Vec<BitRef>, ClkError> {
    resolve_clk_feature(db, "RCLK_HDIO", &config.feature(site))
}

/// Fail loud for a BUFGCE_HDIO CE attribute: the site inventory
/// (`site_type_BUFGCE_HDIO.json`) lists a CE inversion pip, but no
/// segbits line encodes it.
pub fn bufgce_hdio_ce_blocked(site: &str, attribute: &str) -> ClkError {
    ClkError::MissingEncoding {
        site: site.to_string(),
        attribute: attribute.to_string(),
        detail: "site_type_BUFGCE_HDIO.json lists the CE inversion pip, \
                 but segbits_rclk_hdio.db carries only IINV lines for \
                 BUFGCE_HDIO (8 lines, 4 sites); CE encoding needs the \
                 Vivado clocking fuzz job (data/CLK_FUZZ_JOB.md, Job 4)"
            .to_string(),
    }
}

// ---------------------------------------------------------------------------
// CMT HROUTE / HDISTR
// ---------------------------------------------------------------------------

/// CMT `WIRE.CLK_HDISTR_0_<n>.USED` feature for index 0..24.
pub fn cmt_hdistr_used_feature(index: u32) -> Result<String, ClkError> {
    if index >= 24 {
        return Err(ClkError::InvalidValue {
            site: "CMT_RIGHT".to_string(),
            attribute: "WIRE.CLK_HDISTR_0.USED".to_string(),
            value: index.to_string(),
        });
    }
    Ok(format!("WIRE.CLK_HDISTR_0_{index}.USED.V1"))
}

/// CMT `WIRE.CLK_HROUTE_1_<n>.USED` feature for index 0..24.
pub fn cmt_hroute_used_feature(index: u32) -> Result<String, ClkError> {
    if index >= 24 {
        return Err(ClkError::InvalidValue {
            site: "CMT_RIGHT".to_string(),
            attribute: "WIRE.CLK_HROUTE_1.USED".to_string(),
            value: index.to_string(),
        });
    }
    Ok(format!("WIRE.CLK_HROUTE_1_{index}.USED.V1"))
}

/// Resolve one CMT_RIGHT feature (PIP or WIRE) by exact lookup.
pub fn resolve_cmt_feature(db: &dyn SegbitsDb, feature: &str) -> Result<Vec<BitRef>, ClkError> {
    resolve_clk_feature(db, "CMT_RIGHT", feature)
}

// ---------------------------------------------------------------------------
// Fail-loud site config: BUFGCE, BUFGCTRL, BUFGCE_DIV, BUFCE_ROW,
// BUFCE_ROW_FSR, MMCM, PLL
// ---------------------------------------------------------------------------

/// Inversion pips in the vendored `site_type_BUFGCE.json` inventory
/// (pins CLK_IN, CLK_OUT, CE_PRE_OPTINV; site pips CEINV, IINV).
/// The names are real; their bit encodings do not exist publicly.
pub const BUFGCE_INVERSION_PIPS: [&str; 2] = ["CEINV", "IINV"];

/// Validate a BUFGCE site-pip name against the site inventory, then
/// fail loud on its missing encoding. An unknown pip name fails with
/// `UnknownPip` (a different defect from a known pip with no bits).
/// This function always returns `Err`; it exists so callers get the
/// precise failure instead of a generic unknown-feature error.
pub fn bufgce_site_config(site: &str, pip: &str) -> Result<Vec<BitRef>, ClkError> {
    if !BUFGCE_INVERSION_PIPS.contains(&pip) {
        return Err(ClkError::UnknownPip {
            site: site.to_string(),
            pip: pip.to_string(),
        });
    }
    Err(ClkError::MissingEncoding {
        site: site.to_string(),
        attribute: format!("BUFGCE.{pip}"),
        detail: "site_type_BUFGCE.json lists this inversion pip, but no \
                 segbits line encodes any BUFGCE site attribute in \
                 segbits_cmt_right.db (104 lines, HROUTE/HDISTR only) or \
                 any segbits_rclk_*.db file, and BitFiltrator has no CMT \
                 data; encoding needs the Vivado clocking fuzz job \
                 (data/CLK_FUZZ_JOB.md, Job 4)"
            .to_string(),
    })
}

/// Fail loud for a BUFGCTRL site config request. No BUFGCTRL config
/// line exists in any public source.
pub fn bufgctrl_site_config(site: &str, attribute: &str) -> ClkError {
    ClkError::MissingEncoding {
        site: site.to_string(),
        attribute: attribute.to_string(),
        detail: "no BUFGCTRL config line exists in segbits_cmt_right.db, \
                 any segbits_rclk_*.db file, or BitFiltrator; encoding \
                 needs the Vivado clocking fuzz job (data/CLK_FUZZ_JOB.md, Job 4)"
            .to_string(),
    }
}

/// Fail loud for a BUFGCE_DIV site config request. The site inventory
/// exists (`site_type_BUFGCE_DIV.json`); its config encoding does not.
pub fn bufgce_div_site_config(site: &str, attribute: &str) -> ClkError {
    ClkError::MissingEncoding {
        site: site.to_string(),
        attribute: attribute.to_string(),
        detail: "no BUFGCE_DIV config line exists in any public source; \
                 encoding needs the Vivado clocking fuzz job \
                 (data/CLK_FUZZ_JOB.md, Job 4)"
            .to_string(),
    }
}

/// Fail loud for a BUFCE_ROW or BUFCE_ROW_FSR site config request.
/// The sites appear only as PIP endpoints in the RCLK DBs; their
/// CEINV/IINV site pips have no bit encoding.
pub fn bufce_row_site_config(site: &str, attribute: &str) -> ClkError {
    ClkError::MissingEncoding {
        site: site.to_string(),
        attribute: attribute.to_string(),
        detail: "BUFCE_ROW / BUFCE_ROW_FSR appear only as PIP endpoints \
                 in the RCLK segbits DBs; no config line encodes their \
                 CEINV/IINV site pips; encoding needs the Vivado \
                 clocking fuzz job (data/CLK_FUZZ_JOB.md, Job 4)"
            .to_string(),
    }
}

/// Fail loud for any MMCME4_ADV instantiation. Per SPEC.md, MMCM/PLL
/// attribute fuzzing is out of scope until a design needs it; the safe
/// default is a design that instantiates no MMCM, and any instantiated
/// MMCM is blocked here. Zero MMCM config lines exist publicly.
pub fn mmcm_site_config(site: &str, attribute: &str) -> ClkError {
    ClkError::MissingEncoding {
        site: site.to_string(),
        attribute: attribute.to_string(),
        detail: "no MMCM config line exists in any public source \
                 (site_type_MMCM.json documents pins only, BitFiltrator \
                 has no CMT data); attribute fuzzing is out of scope \
                 until a design needs it (SPEC.md) and needs the Vivado \
                 clocking fuzz job (data/CLK_FUZZ_JOB.md, Job 4)"
            .to_string(),
    }
}

/// Fail loud for any PLLE4_ADV instantiation, same rule as MMCM.
pub fn pll_site_config(site: &str, attribute: &str) -> ClkError {
    ClkError::MissingEncoding {
        site: site.to_string(),
        attribute: attribute.to_string(),
        detail: "no PLL config line exists in any public source \
                 (site_type_PLL.json documents pins only, BitFiltrator \
                 has no CMT data); attribute fuzzing is out of scope \
                 until a design needs it (SPEC.md) and needs the Vivado \
                 clocking fuzz job (data/CLK_FUZZ_JOB.md, Job 4)"
            .to_string(),
    }
}

/// Guard for designs that must not instantiate clock managers: returns
/// `Ok` only when nothing is instantiated. Any MMCM or PLL instance
/// is blocked with its site named.
pub fn require_no_clock_manager(mmcm_sites: &[&str], pll_sites: &[&str]) -> Result<(), ClkError> {
    if let Some(site) = mmcm_sites.first() {
        return Err(mmcm_site_config(site, "MMCME4_ADV"));
    }
    if let Some(site) = pll_sites.first() {
        return Err(pll_site_config(site, "PLLE4_ADV"));
    }
    Ok(())
}
