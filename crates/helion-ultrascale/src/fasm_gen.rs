//! Complete-FASM generation for FF INIT/SRVAL (integration-owned).
//!
//! CTO ruling 2026-11-07 (complete FASM): parse-back renders exactly one
//! `.V0`/`.V1` line per FF in the device for DBs with the pair structure
//! (exact-complement rule, bitgen-core 88070a5), so round-trip exactness
//! requires COMPLETE FASM: every FF in the design gets an INIT line and
//! an SRVAL line. The Xilinx default FF INIT is 0, so an unspecified FF
//! INIT means INIT=0, which selects `.V0` (bit set); same for SRVAL
//! (default V0). The emitter does not invent defaults; this generator
//! does, explicitly, at generation time. The diff-harness worker applies
//! the same canonicalization on the comparison side; this module is the
//! generation side.
//!
//! Canonical spelling: `{site}.{FF}.INIT.V0` / `.V1` and
//! `{site}.{FF}.SRVAL.V0` / `.V1`, V selected by value. No bare
//! `AFF.INIT = 1'b0` spelling: that is the lut worker's bitloc
//! `ff_init_fasm`, bit=value, INVERTED per the resolved polarity
//! (bit = NOT(INIT); INIT 0 -> V0 -> bit SET, INIT 1 -> V1 -> bit
//! CLEAR). The lut worker's emission must change separately; this module
//! emits canonical lines. The bit polarity is handled by segbits
//! resolution (V0 sets, V1 clears); the FASM line carries no value.
//!
//! Inputs: the packed slices (which FF BELs are used) joined to the
//! placed slices (which site each packed slice landed on), plus an
//! optional per-cell attribute map. Today helion-ir `Hff` carries no
//! INIT/SRVAL params, so every FF is unspecified and defaults to V0;
//! the `FfAttr` map leaves the specified path open for when the IR
//! grows them. LUT, BRAM, and PIP lines are other workers' scope;
//! this module emits FF lines only.

use crate::pack::PackedUltraSlice;
use crate::place::UltraPlaced;
use std::collections::HashMap;

/// Per-FF attribute values as specified by the design. `None` =
/// unspecified, defaults to V0 (Xilinx default INIT/SRVAL 0).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FfAttr {
    pub init: Option<bool>,
    pub srval: Option<bool>,
}

/// FASM / BitFiltrator FF name for a BEL index 0..16: `AFF`..`HFF2`.
/// Bel index 0 is AQ (AFF), 1 is AQ2 (AFF2), through 15 is HQ2 (HFF2).
/// Duplicated from bitloc::ff_fasm_name (lut worker's file, not in this
/// branch yet); the orchestrator reconciles at merge and the generator
/// then calls bitloc directly. Deliberately NOT re-exported at the crate
/// root, so it cannot collide with bitloc's copy.
pub fn ff_fasm_name(ff_index: u8) -> Result<&'static str, String> {
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
        other => Err(format!("ULTRA FASM: FF BEL index {other} out of range 0..16")),
    }
}

/// FASM line for one FF INIT: `{site}.{FF}.INIT.V0` or `.V1`.
/// `None` (unspecified) defaults to V0 (Xilinx default INIT 0).
pub fn ff_init_fasm_line(site: &str, ff_index: u8, init: Option<bool>) -> Result<String, String> {
    let name = ff_fasm_name(ff_index)?;
    let v = if init.unwrap_or(false) { 1 } else { 0 };
    Ok(format!("{site}.{name}.INIT.V{v}"))
}

/// FASM line for one FF SRVAL: `{site}.{FF}.SRVAL.V0` or `.V1`.
/// `None` (unspecified) defaults to V0 (Xilinx default SRVAL 0).
pub fn ff_srval_fasm_line(
    site: &str,
    ff_index: u8,
    srval: Option<bool>,
) -> Result<String, String> {
    let name = ff_fasm_name(ff_index)?;
    let v = if srval.unwrap_or(false) { 1 } else { 0 };
    Ok(format!("{site}.{name}.SRVAL.V{v}"))
}

/// Emit complete FF FASM for a placed design: for every used FF BEL in
/// every placed slice, one INIT line and one SRVAL line, in deterministic
/// order (placed-slice order, then BEL index ascending). `attrs` maps FF
/// cell name to its specified INIT/SRVAL; a cell absent from the map is
/// unspecified and defaults to V0 for both. The placed site name is the
/// FASM tile, not the packed slice name.
pub fn emit_ff_fasm(
    packed: &[PackedUltraSlice],
    placed: &UltraPlaced,
    attrs: &HashMap<String, FfAttr>,
) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for p in &placed.slices {
        let slice = packed.get(p.packed_idx).ok_or_else(|| {
            format!(
                "ULTRA FASM: placed slice {} references packed index {} out of range",
                p.site, p.packed_idx
            )
        })?;
        let mut bels: Vec<u8> = slice.ffs.keys().copied().collect();
        bels.sort_unstable();
        for bel in bels {
            let cell = &slice.ffs[&bel];
            let attr = attrs.get(cell).copied().unwrap_or_default();
            lines.push(ff_init_fasm_line(&p.site, bel, attr.init)?);
            lines.push(ff_srval_fasm_line(&p.site, bel, attr.srval)?);
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{pack_ultra, ControlSetLimits, SliceKind};
    use crate::place::{place_ultra, UltraGrid, UltraPlaceOpts};

    /// Minimal grid for pack/place-through-generate tests: 4x4 logic sites.
    struct SmallGrid;

    impl UltraGrid for SmallGrid {
        fn slice_sites(&self) -> Vec<(String, String, i32, i32, SliceKind)> {
            let mut sites = Vec::new();
            for y in 0..4 {
                for x in 0..4 {
                    sites.push((
                        format!("SLICE_X{x}Y{y}"),
                        format!("CLB_X{x}Y{y}"),
                        x,
                        y,
                        SliceKind::Logic,
                    ));
                }
            }
            sites
        }

        fn grid_size(&self) -> (i32, i32) {
            (4, 4)
        }
    }

    fn one_ff_packed() -> (Vec<PackedUltraSlice>, UltraPlaced) {
        let mut slice = PackedUltraSlice::new("SLICE_X0Y0".to_string(), SliceKind::Logic);
        slice.ffs.insert(0, "q_reg".to_string());
        slice.ff_clk.insert(0, "clk".to_string());
        let packed = vec![slice];
        let placed = UltraPlaced {
            slices: vec![crate::place::PlacedUltraSlice {
                packed_idx: 0,
                site: "SLICE_X10Y20".to_string(),
                tile: "CLB_X10Y20".to_string(),
                x: 10,
                y: 20,
            }],
            seed: 0,
        };
        (packed, placed)
    }

    #[test]
    fn unspecified_ff_init_emits_v0_line() {
        // The acceptance case (CTO ruling 2026-11-07, complete FASM):
        // an FF with no specified INIT emits .INIT.V0 (Xilinx default
        // INIT 0), and no specified SRVAL emits .SRVAL.V0.
        let (packed, placed) = one_ff_packed();
        let lines = emit_ff_fasm(&packed, &placed, &HashMap::new()).unwrap();
        assert_eq!(
            lines,
            vec![
                "SLICE_X10Y20.AFF.INIT.V0".to_string(),
                "SLICE_X10Y20.AFF.SRVAL.V0".to_string(),
            ]
        );
    }

    #[test]
    fn specified_ff_init_one_emits_v1_line() {
        let (packed, placed) = one_ff_packed();
        let mut attrs = HashMap::new();
        attrs.insert(
            "q_reg".to_string(),
            FfAttr {
                init: Some(true),
                srval: Some(true),
            },
        );
        let lines = emit_ff_fasm(&packed, &placed, &attrs).unwrap();
        assert_eq!(
            lines,
            vec![
                "SLICE_X10Y20.AFF.INIT.V1".to_string(),
                "SLICE_X10Y20.AFF.SRVAL.V1".to_string(),
            ]
        );
    }

    #[test]
    fn mixed_specified_values_pick_v_per_attribute() {
        let (packed, placed) = one_ff_packed();
        let mut attrs = HashMap::new();
        attrs.insert(
            "q_reg".to_string(),
            FfAttr {
                init: Some(false),
                srval: Some(true),
            },
        );
        let lines = emit_ff_fasm(&packed, &placed, &attrs).unwrap();
        assert_eq!(
            lines,
            vec![
                "SLICE_X10Y20.AFF.INIT.V0".to_string(),
                "SLICE_X10Y20.AFF.SRVAL.V1".to_string(),
            ]
        );
    }

    #[test]
    fn placed_site_is_used_not_packed_name() {
        // The packed slice is named SLICE_X0Y0 but placed at
        // SLICE_X10Y20; the FASM tile is the placed site.
        let (packed, placed) = one_ff_packed();
        let lines = emit_ff_fasm(&packed, &placed, &HashMap::new()).unwrap();
        assert!(lines[0].starts_with("SLICE_X10Y20."));
        assert!(!lines[0].starts_with("SLICE_X0Y0."));
    }

    #[test]
    fn ff_bel_names_cover_all_sixteen() {
        let names: Vec<&str> = (0..16).map(|i| ff_fasm_name(i).unwrap()).collect();
        assert_eq!(
            names,
            vec![
                "AFF", "AFF2", "BFF", "BFF2", "CFF", "CFF2", "DFF", "DFF2", "EFF", "EFF2",
                "FFF", "FFF2", "GFF", "GFF2", "HFF", "HFF2",
            ]
        );
        assert!(ff_fasm_name(16).is_err());
    }

    #[test]
    fn multiple_ffs_emit_in_bel_order() {
        let mut slice = PackedUltraSlice::new("SLICE_X0Y0".to_string(), SliceKind::Logic);
        slice.ffs.insert(5, "r5".to_string());
        slice.ffs.insert(2, "r2".to_string());
        slice.ff_clk.insert(5, "clk".to_string());
        slice.ff_clk.insert(2, "clk".to_string());
        let packed = vec![slice];
        let placed = UltraPlaced {
            slices: vec![crate::place::PlacedUltraSlice {
                packed_idx: 0,
                site: "SLICE_X0Y0".to_string(),
                tile: "CLB_X0Y0".to_string(),
                x: 0,
                y: 0,
            }],
            seed: 0,
        };
        let lines = emit_ff_fasm(&packed, &placed, &HashMap::new()).unwrap();
        assert_eq!(
            lines,
            vec![
                "SLICE_X0Y0.BFF.INIT.V0".to_string(),
                "SLICE_X0Y0.BFF.SRVAL.V0".to_string(),
                "SLICE_X0Y0.CFF2.INIT.V0".to_string(),
                "SLICE_X0Y0.CFF2.SRVAL.V0".to_string(),
            ]
        );
    }

    #[test]
    fn pack_place_then_generate_emits_v0_for_unspecified_ff() {
        // End to end through the real packer and placer: one LUT plus
        // one FF, no attributes anywhere, yields the V0 lines.
        let limits = ControlSetLimits::default();
        let luts = vec![("lut0".to_string(), 0xAAAAAAAAAAAAAAAAu64, false)];
        let ffs = vec![("q_reg".to_string(), "clk".to_string(), None, None)];
        let packed = pack_ultra(&luts, &ffs, &[], &limits).unwrap();
        let placed = place_ultra(
            &packed.slices,
            &SmallGrid,
            &UltraPlaceOpts::default(),
        )
        .unwrap();
        let lines = emit_ff_fasm(&packed.slices, &placed, &HashMap::new()).unwrap();
        assert_eq!(lines.len(), 2, "one FF gives one INIT plus one SRVAL line");
        assert!(lines[0].ends_with(".AFF.INIT.V0"), "got {}", lines[0]);
        assert!(lines[1].ends_with(".AFF.SRVAL.V0"), "got {}", lines[1]);
    }

    #[test]
    fn placed_index_out_of_range_fails_loud() {
        let (packed, mut placed) = one_ff_packed();
        placed.slices[0].packed_idx = 99;
        let err = emit_ff_fasm(&packed, &placed, &HashMap::new()).unwrap_err();
        assert!(err.contains("out of range"), "got {err}");
    }
}
