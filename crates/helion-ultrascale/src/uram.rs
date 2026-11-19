//! URAM288 fail-loud: no config-bit or content encoding exists in any
//! public source.
//!
//! Search record: `crates/helion-ultrascale/data/URAM288_SEARCH.md`.
//! Summary of what was searched (prjuray-db reference clone at commit
//! `affbc5e`, CC0 1.0; BitFiltrator reference clone at commit `79b9bd6`,
//! MIT; AMD UG974 2023.1, UG573, WP477, read 2026-11-19):
//!
//! - prjuray-db `zynqusp/` ships no `segbits_uram*.db` and no
//!   `segbits_rclk_uram*.db`; a case-insensitive grep for `uram` over
//!   the whole directory returns nothing. The 18 vendored `.db` files
//!   carry 0 URAM lines in total.
//! - BitFiltrator `ULTRASCALE_PLUS.json` has only the top-level keys
//!   `BRAM`, `CLEL_L`, `CLEL_R`, `CLEM`, `CLEM_R`: 0 mentions of
//!   `URAM`. Its device files name URAM tiles (`URAM_URAM_FT`,
//!   `URAM_URAM_DELAY_FT`, site type `URAM288`) but carry no encoding.
//! - UG974 lists 34 URAM288 attributes and no `INIT_xxx` attribute,
//!   so there is no per-word INIT to sweep the way BRAM content is
//!   swept. UG573 and WP477 agree one URAM288 is 4096 words x 72 bits
//!   (294,912 content bits).
//!
//! Consequence: any FASM entry that configures a URAM288, or any entry
//! on a URAM tile at all, has no encoding to resolve against. The
//! emitter refuses it with [`BitgenError::UramBlocked`](crate::bitgen::BitgenError::UramBlocked)
//! instead of the generic `UnknownFeature` / `UnknownTileType`, so the
//! failure names the missing data and the unblocker (Vivado fuzzing
//! job 5, the URAM288 attribute and content sweep) rather than looking
//! like a typo.
//!
//! The detector is deliberately conservative: it matches tile/feature
//! spellings that the public sources actually use, plus the bare
//! URAM288 attribute names from UG974 (the job-5 baseline design). It
//! never matches a LUT, FF, BRAM, INT, or IO entry. Two names need an
//! explicit exclusion, mirroring the DSP detector's treatment of
//! generic words:
//!
//! - `CASCADE_ORDER_A` / `CASCADE_ORDER_B` are URAM288 attributes in
//!   UG974, but they are also RAMB36E2 attributes (UG974 RAMB36E2
//!   template). A bare `CASCADE_ORDER_A` feature on a non-URAM tile
//!   must stay `UnknownFeature` (or resolve against a future BRAM
//!   encoding), not be misattributed to URAM. They still block
//!   whenever the tile or feature itself names URAM (rules 1-3).
//! - `IS_CLK_INVERTED` is in this list and is also a DSP48E2
//!   attribute name. On this branch a bare `IS_CLK_INVERTED` reports
//!   `UramBlocked`; on the DSP branch the same bare name reports
//!   `Dsp48e2Blocked`. Both fail loud and emit no bits, so the
//!   difference is only the error name; the merge-time ordering of
//!   the two detectors is a CTO decision, not a correctness issue.
//!   The vendored segbits DBs do carry `IS_CLK_INVERTED` in 4 lines,
//!   but only as a trailing component of OSERDESE3 features
//!   (`HDIOLOGIC_M_X0Y1.OPTFF.OSERDESE3.IS_CLK_INVERTED.V0/V1`); the
//!   detector matches the feature's first component only, so those
//!   lines are unaffected (see `other_blocks_are_not_uram`).
//!
//! No em dashes anywhere.

/// URAM288 primitive attribute names, exactly as in the AMD UG974
/// (2023.1 English) URAM288 "Available Attributes" table. These are
/// attribute *names* from the public UltraScale libraries
/// documentation, not encodings: no bit positions are known for any of
/// them.
///
/// `CASCADE_ORDER_A` and `CASCADE_ORDER_B` are deliberately excluded:
/// they are also RAMB36E2 attribute names, and a bare feature of that
/// name on a non-URAM tile must stay an `UnknownFeature`, not a URAM
/// block. They are still fuzzed by job 5 (all 34 UG974 attributes),
/// and they still block whenever the tile or feature itself names
/// URAM288 or a URAM tile.
pub const URAM288_ATTRIBUTES: &[&str] = &[
    "AUTO_SLEEP_LATENCY",
    "AVG_CONS_INACTIVE_CYCLES",
    "BWE_MODE_A",
    "BWE_MODE_B",
    "EN_AUTO_SLEEP_MODE",
    "EN_ECC_RD_A",
    "EN_ECC_RD_B",
    "EN_ECC_WR_A",
    "EN_ECC_WR_B",
    "IREG_PRE_A",
    "IREG_PRE_B",
    "IS_CLK_INVERTED",
    "IS_EN_A_INVERTED",
    "IS_EN_B_INVERTED",
    "IS_RDB_WR_A_INVERTED",
    "IS_RDB_WR_B_INVERTED",
    "IS_RST_A_INVERTED",
    "IS_RST_B_INVERTED",
    "OREG_A",
    "OREG_B",
    "OREG_ECC_A",
    "OREG_ECC_B",
    "REG_CAS_A",
    "REG_CAS_B",
    "RST_MODE_A",
    "RST_MODE_B",
    "SELF_ADDR_A",
    "SELF_ADDR_B",
    "SELF_MASK_A",
    "SELF_MASK_B",
    "USE_EXT_CE_A",
    "USE_EXT_CE_B",
];

/// True when a FASM entry (`tile`, `feature`) touches a URAM288 or a
/// URAM tile and therefore has no config-bit or content encoding in
/// any public source.
///
/// The four rules, in order:
/// 1. The tile names a URAM288 site (`URAM288_X0Y0`).
/// 2. The feature names a URAM288 (`URAM_X0Y0.URAM288.OREG_A` style).
/// 3. The tile is a URAM tile (`URAM_X0Y0`): prjuray-db ships no
///    segbits DB for the URAM tile type, so no feature on such a tile,
///    PIPs included, has an encoding.
/// 4. The feature's first component is a URAM288 attribute name (rule
///    4 catches bare-attribute spellings such as `OREG_A` on a tile
///    that is not itself URAM-named).
pub fn is_uram288_entry(tile: &str, feature: &str) -> bool {
    if tile.contains("URAM288") || feature.contains("URAM288") {
        return true;
    }
    if tile.contains("URAM") {
        return true;
    }
    let first = feature.split('.').next().unwrap_or("");
    // Attribute names with a value range or index (`SELF_ADDR_A[10:0]`)
    // match on the bare name before the bracket.
    let first = first.split('[').next().unwrap_or(first);
    URAM288_ATTRIBUTES.contains(&first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_named_tile_is_uram() {
        assert!(is_uram288_entry("URAM288_X0Y0", "OREG_A"));
        assert!(is_uram288_entry("URAM288_X0Y0", "CASCADE_ORDER_A"));
    }

    #[test]
    fn feature_naming_uram288_is_uram() {
        assert!(is_uram288_entry("URAM_X0Y0", "URAM288.OREG_A"));
        assert!(is_uram288_entry("URAM_X0Y0", "URAM288.CASCADE_ORDER_A"));
        assert!(is_uram288_entry("SLICE_X0Y0", "URAM288.OREG_A"));
    }

    #[test]
    fn uram_tile_is_uram_even_for_pips() {
        // No segbits DB exists for the URAM tile type at all, so even a
        // routing PIP on a URAM tile has no encoding.
        assert!(is_uram288_entry("URAM_X0Y0", "PIP.URAM_X0Y0_FOO.URAM_X0Y0_BAR"));
    }

    #[test]
    fn bare_attribute_name_is_uram() {
        assert!(is_uram288_entry("SLICE_X0Y0", "OREG_A"));
        assert!(is_uram288_entry("SLICE_X0Y0", "EN_ECC_RD_A"));
        assert!(is_uram288_entry("SLICE_X0Y0", "SELF_MASK_B"));
        assert!(is_uram288_entry("SLICE_X0Y0", "IS_CLK_INVERTED"));
        assert!(is_uram288_entry("SLICE_X0Y0", "SELF_ADDR_A[10:0]"));
    }

    #[test]
    fn cascade_order_bare_name_is_not_uram() {
        // CASCADE_ORDER_A/B are also RAMB36E2 attributes (UG974). A
        // bare feature of that name on a non-URAM tile must stay
        // UnknownFeature, not be misattributed to URAM.
        assert!(!is_uram288_entry("SLICE_X0Y0", "CASCADE_ORDER_A"));
        assert!(!is_uram288_entry("SLICE_X0Y0", "CASCADE_ORDER_B"));
        assert!(!is_uram288_entry("RAMB36_X0Y0", "CASCADE_ORDER_A"));
        // But on a URAM tile or site the same name is URAM.
        assert!(is_uram288_entry("URAM_X0Y0", "CASCADE_ORDER_A"));
        assert!(is_uram288_entry("URAM288_X0Y0", "CASCADE_ORDER_B"));
    }

    #[test]
    fn detector_list_is_the_32_name_subset_of_the_34_fuzzed() {
        // Job 5 fuzzes 34 attributes (uram_variant_table.tsv, UG974).
        // The detector lists 32 by bare name: CASCADE_ORDER_A and
        // CASCADE_ORDER_B are excluded as shared with RAMB36E2 (see
        // cascade_order_bare_name_is_not_uram). If this count changes,
        // JOB5.md and URAM288_SEARCH.md must change with it.
        assert_eq!(URAM288_ATTRIBUTES.len(), 32);
        assert!(!URAM288_ATTRIBUTES.contains(&"CASCADE_ORDER_A"));
        assert!(!URAM288_ATTRIBUTES.contains(&"CASCADE_ORDER_B"));
    }

    #[test]
    fn other_blocks_are_not_uram() {
        assert!(!is_uram288_entry("SLICE_X0Y0", "A6LUT.INIT[63:0]"));
        assert!(!is_uram288_entry("SLICE_X0Y0", "AFF.INIT.V0"));
        assert!(!is_uram288_entry(
            "INT_X0Y0",
            "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1"
        ));
        assert!(!is_uram288_entry("RAMB18_X0Y0", "INIT_00"));
        assert!(!is_uram288_entry("SLICE_X0Y0", "BOGUS_FEATURE"));
        assert!(!is_uram288_entry("SLICE_X0Y0", ""));
        // IS_CLK_INVERTED appears in the vendored HDIO DBs only as a
        // trailing component of an OSERDESE3 feature, never as the
        // first component. The detector matches the first component
        // only, so those lines are not URAM.
        assert!(!is_uram288_entry(
            "HDIO_TOP_RIGHT_X0Y1",
            "OPTFF.OSERDESE3.IS_CLK_INVERTED.V0"
        ));
    }
}
