//! DSP48E2 fail-loud: no config-bit encoding exists in any public source.
//!
//! Search record: `crates/helion-ultrascale/data/DSP48E2_SEARCH.md`.
//! Summary of what was searched (prjuray-db reference clone at commit
//! `affbc5e`, CC0 1.0; BitFiltrator reference clone at commit `79b9bd6`,
//! MIT):
//!
//! - prjuray-db `zynqusp/` ships no `segbits_dsp*.db`. The only DSP-named
//!   segbits files are `segbits_rclk_dsp_intf_{l,r,clkbuf_l}.db`, which
//!   carry clock PIPs only (0 mentions of `OPMODE`, `ALUMODE`, `INMODE`,
//!   or `DSP48`).
//! - `site_type_DSP48E2.json` has only the keys `site_pins`, `site_pips`,
//!   `type`: pins and pips, no config bits.
//! - `tile_type_DSP.json` has only the keys `pips`, `sites`, `tile_type`,
//!   `wires`: routing pips and wires, no `segbits` key.
//! - BitFiltrator `ULTRASCALE_PLUS.json` has only the top-level keys
//!   `BRAM`, `CLEL_L`, `CLEL_R`, `CLEM`, `CLEM_R`: 0 mentions of `DSP`
//!   or `DSP48`. Its device files carry DSP column majors (`dsp_colMajors`,
//!   frame addressing) and `num_dsps`, not config encodings.
//!
//! Consequence: any FASM entry that configures a DSP48E2, or any entry on
//! a DSP tile at all, has no encoding to resolve against. The emitter
//! refuses it with [`BitgenError::Dsp48e2Blocked`](crate::bitgen::BitgenError::Dsp48e2Blocked)
//! instead of the generic `UnknownFeature` / `UnknownTileType`, so the
//! failure names the missing data and the unblocker (Vivado fuzzing job 2,
//! the DSP48E2 attribute sweep) rather than looking like a typo.
//!
//! The detector is deliberately conservative: it matches tile/feature
//! spellings that the public sources actually use, plus the bare
//! DSP48E2 attribute names from the BitFiltrator `dsp.v` template
//! (the job-2 baseline design). It never matches a LUT, FF, BRAM, INT,
//! or IO entry. Generic words that happen to be DSP attribute names
//! (`MASK`, `PATTERN`, `RND`) are NOT matched on their own: a bare
//! `MASK` feature on a non-DSP tile is better reported as
//! `UnknownFeature` than misattributed to DSP.
//!
//! Dynamic control ports (`OPMODE`, `ALUMODE`, `INMODE`, `CARRYINSEL`)
//! are live inputs, not primitive attributes, so they have no attribute
//! sweep entry; their values are routing, not DSP config bits. Job 2
//! fuzzes the 46 primitive attributes in `dsp_variant_table.tsv` only;
//! `DSP48E2_ATTRIBUTES` below is the 43-name bare-name detector subset
//! of that set (`RND`, `MASK`, and `PATTERN` excluded as generic words).
//!
//! No em dashes anywhere.

/// DSP48E2 primitive attribute names, exactly as in the BitFiltrator
/// `dsp.v` template (the job-2 baseline design, shipped in the Vivado
/// fuzzing package). These are attribute *names* from the public
/// UltraScale libraries documentation as embodied in that template, not
/// encodings: no bit positions are known for any of them.
///
/// `MASK`, `PATTERN`, and `RND` are deliberately excluded: they are
/// generic words, and a bare feature of that name on a non-DSP tile
/// must stay an `UnknownFeature`, not a DSP block.
pub const DSP48E2_ATTRIBUTES: &[&str] = &[
    "AMULTSEL",
    "A_INPUT",
    "BMULTSEL",
    "B_INPUT",
    "PREADDINSEL",
    "USE_MULT",
    "USE_SIMD",
    "USE_WIDEXOR",
    "XORSIMD",
    "AUTORESET_PATDET",
    "AUTORESET_PRIORITY",
    "SEL_MASK",
    "SEL_PATTERN",
    "USE_PATTERN_DETECT",
    "IS_ALUMODE_INVERTED",
    "IS_CARRYIN_INVERTED",
    "IS_CLK_INVERTED",
    "IS_INMODE_INVERTED",
    "IS_OPMODE_INVERTED",
    "IS_RSTALLCARRYIN_INVERTED",
    "IS_RSTALUMODE_INVERTED",
    "IS_RSTA_INVERTED",
    "IS_RSTB_INVERTED",
    "IS_RSTCTRL_INVERTED",
    "IS_RSTC_INVERTED",
    "IS_RSTD_INVERTED",
    "IS_RSTINMODE_INVERTED",
    "IS_RSTM_INVERTED",
    "IS_RSTP_INVERTED",
    "ACASCREG",
    "ADREG",
    "ALUMODEREG",
    "AREG",
    "BCASCREG",
    "BREG",
    "CARRYINREG",
    "CARRYINSELREG",
    "CREG",
    "DREG",
    "INMODEREG",
    "MREG",
    "OPMODEREG",
    "PREG",
];

/// True when a FASM entry (`tile`, `feature`) touches a DSP48E2 or a DSP
/// tile and therefore has no config-bit encoding in any public source.
///
/// The four rules, in order:
/// 1. The tile names a DSP48E2 site (`DSP48E2_X2Y10`): the site-named
///    tile spelling used by the day-1 scout report FASM section and by
///    the diff harness block labeller.
/// 2. The feature names a DSP48E2 (`DSP_X0Y0.DSP48E2.AREG` style).
/// 3. The tile is a DSP tile (`DSP_X0Y0`): prjuray-db ships no segbits
///    DB for the DSP tile type, so no feature on such a tile, PIPs
///    included, has an encoding.
/// 4. The feature's first component is a DSP48E2 attribute name (rule
///    4 catches bare-attribute spellings such as `AREG` on a tile that
///    is not itself DSP-named).
pub fn is_dsp48e2_entry(tile: &str, feature: &str) -> bool {
    if tile.contains("DSP48E2") || feature.contains("DSP48E2") {
        return true;
    }
    if tile.starts_with("DSP_") {
        return true;
    }
    let first = feature.split('.').next().unwrap_or("");
    // Attribute names with a value range or index (`AREG[1:0]`,
    // `MASK[47:0]`) match on the bare name before the bracket.
    let first = first.split('[').next().unwrap_or(first);
    DSP48E2_ATTRIBUTES.contains(&first)
}

/// What the emitter should do with one DSP48E2 entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DspEntry {
    /// Not a DSP entry at all; the emitter continues to the normal lookup.
    NotDsp,
    /// A DSP attribute whose value is the base value: emits no bit.
    Base,
    /// A DSP attribute with a measured encoding: emit these bits.
    Measured(Vec<crate::dsp_fuzz::Bit>),
    /// A DSP attribute with no measured encoding. Carries the attribute name
    /// so the error names what is missing.
    Blocked { attribute: String, reason: String },
}

/// Split a DSP feature spelling into `(attribute, value)`.
///
/// The spellings seen in practice are `AREG` (bare, meaning a boolean is set),
/// `AREG.A` or `AMULTSEL.AD`, and `MASK[47:0]`. Returns None when the feature
/// is not an attribute setting at all (a PIP, or an empty feature).
fn split_attribute(feature: &str) -> Option<(String, Option<String>)> {
    if feature.starts_with("PIP.") || feature.is_empty() {
        return None;
    }
    let mut parts = feature.split('.');
    let first = parts.next()?;
    let (name, bracketed) = match first.find('[') {
        Some(i) => (&first[..i], Some(&first[i..])),
        None => (first, None),
    };
    // `AMULTSEL.AD` style: value is the next component.
    if let Some(value) = parts.next() {
        return Some((name.to_string(), Some(value.to_string())));
    }
    // `AREG[1:0]` style: the bracket itself is the value spelling, which the
    // resolver compares textually. A bare name carries no value.
    Some((name.to_string(), bracketed.map(|s| s.to_string())))
}

/// Decide one DSP entry against the measured fuzz table.
///
/// This is the whole "lift" of the blanket block: an entry is allowed when
/// its attribute is either at the base value or has a measured encoding, and
/// blocked (naming the attribute) otherwise. Non-DSP entries are passed
/// through untouched.
pub fn classify_entry(tile: &str, feature: &str) -> DspEntry {
    if !is_dsp48e2_entry(tile, feature) {
        return DspEntry::NotDsp;
    }
    let (attribute, value) = match split_attribute(feature) {
        Some(pair) => pair,
        // A PIP on a DSP tile: routing, not config. Still has no encoding
        // from any public source, so it stays blocked.
        None => {
            return DspEntry::Blocked {
                attribute: feature.to_string(),
                reason: "routing or non-attribute feature on a DSP tile".to_string(),
            }
        }
    };
    // A bare attribute name with no value is the set form (for example
    // `AREG` means the register is 1). Treat it as value "1".
    let value = value.unwrap_or_else(|| "1".to_string());
    // Measured bits only mean anything on a tile that really is a DSP48E2.
    // `is_dsp48e2_entry` also fires for a bare attribute name on ANY tile
    // (rule 4), which is how a stray `SLICE_X0Y0.AREG` gets caught. Such an
    // entry has no DSP tile to place bits in, so it stays blocked even when
    // the attribute and value are measured.
    if !is_dsp_tile(tile) {
        return DspEntry::Blocked {
            attribute,
            reason: "DSP48E2 attribute named on a non-DSP tile".to_string(),
        };
    }
    match crate::dsp_fuzz::resolve(&attribute, &value) {
        Ok(crate::dsp_fuzz::Resolved::Base) => DspEntry::Base,
        Ok(crate::dsp_fuzz::Resolved::Bits(bits)) => DspEntry::Measured(bits),
        Err(crate::dsp_fuzz::Unmeasured::Attribute) => DspEntry::Blocked {
            attribute,
            reason: "no measured encoding for any value of this attribute".to_string(),
        },
        Err(crate::dsp_fuzz::Unmeasured::Value { from, to }) => DspEntry::Blocked {
            attribute,
            reason: format!(
                "measured only for {from} -> {to}; the requested value is not covered"
            ),
        },
    }
}

/// True when the tile name denotes a DSP48E2 site or a DSP tile, i.e. a tile
/// the measured bits can actually be written into. Mirrors rules 1 and 3 of
/// [`is_dsp48e2_entry`] but excludes the bare-name rule.
pub fn is_dsp_tile(tile: &str) -> bool {
    tile.contains("DSP48E2") || tile.starts_with("DSP_")
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_named_tile_is_dsp() {
        assert!(is_dsp48e2_entry("DSP48E2_X2Y10", "AREG"));
        assert!(is_dsp48e2_entry("DSP48E2_X0Y0", "PIP.DSP_DSP48_0_A0.DSP_DSP48_0_A0_PIN"));
    }

    #[test]
    fn feature_naming_dsp48e2_is_dsp() {
        assert!(is_dsp48e2_entry("DSP_X0Y0", "DSP48E2.AREG"));
        assert!(is_dsp48e2_entry("DSP_X0Y0", "DSP48E2.OPMODE[6:0]"));
    }

    #[test]
    fn dsp_tile_is_dsp_even_for_pips() {
        // No segbits DB exists for the DSP tile type at all, so even a
        // routing PIP on a DSP tile has no encoding.
        assert!(is_dsp48e2_entry(
            "DSP_X0Y0",
            "PIP.DSP_DSP48_0_A0.DSP_DSP48_0_A0_PIN"
        ));
    }

    #[test]
    fn bare_attribute_name_is_dsp() {
        assert!(is_dsp48e2_entry("SLICE_X0Y0", "AREG"));
        assert!(is_dsp48e2_entry("SLICE_X0Y0", "USE_MULT"));
        assert!(is_dsp48e2_entry("SLICE_X0Y0", "IS_CLK_INVERTED"));
        assert!(is_dsp48e2_entry("SLICE_X0Y0", "OPMODEREG[0]"));
    }

    #[test]
    fn generic_words_are_not_dsp() {
        // MASK, PATTERN, RND are DSP attribute names too, but they are
        // generic words: on a non-DSP tile they must stay UnknownFeature.
        assert!(!is_dsp48e2_entry("SLICE_X0Y0", "MASK"));
        assert!(!is_dsp48e2_entry("SLICE_X0Y0", "PATTERN"));
        assert!(!is_dsp48e2_entry("SLICE_X0Y0", "RND"));
    }

    #[test]
    fn detector_list_is_the_43_name_subset_of_the_46_fuzzed() {
        // Job 2 fuzzes 46 attributes (dsp_variant_table.tsv). The
        // detector lists 43 by bare name: RND, MASK, and PATTERN are
        // excluded as generic words (see generic_words_are_not_dsp).
        // If this count changes, JOB2.md and DSP48E2_SEARCH.md must
        // change with it.
        assert_eq!(DSP48E2_ATTRIBUTES.len(), 43);
        assert!(!DSP48E2_ATTRIBUTES.contains(&"RND"));
        assert!(!DSP48E2_ATTRIBUTES.contains(&"MASK"));
        assert!(!DSP48E2_ATTRIBUTES.contains(&"PATTERN"));
    }

    #[test]
    fn other_blocks_are_not_dsp() {
        assert!(!is_dsp48e2_entry("SLICE_X0Y0", "A6LUT.INIT[63:0]"));
        assert!(!is_dsp48e2_entry("SLICE_X0Y0", "AFF.INIT.V0"));
        assert!(!is_dsp48e2_entry("INT_X0Y0", "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1"));
        assert!(!is_dsp48e2_entry("RAMB18_X0Y0", "INIT_00"));
        assert!(!is_dsp48e2_entry("SLICE_X0Y0", "BOGUS_FEATURE"));
        assert!(!is_dsp48e2_entry("SLICE_X0Y0", ""));
    }
}
