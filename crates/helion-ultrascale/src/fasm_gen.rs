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
//! `AFF.INIT = 1'b0` spelling: the lut worker's bitloc `ff_init_fasm`
//! now emits the same canonical `.V0`/`.V1` lines (merged 7f80b54,
//! critic session 9 PASS), with the resolved polarity (bit = NOT(INIT);
//! INIT 0 -> V0 -> bit SET, INIT 1 -> V1 -> bit CLEAR). This module
//! emits canonical lines. The bit polarity is handled by segbits
//! resolution (V0 sets, V1 clears); the FASM line carries no value.
//!
//! Inputs: the packed slices (which FF BELs are used) joined to the
//! placed slices (which site each packed slice landed on), plus an
//! optional per-cell attribute map. Today helion-ir `Hff` carries no
//! INIT/SRVAL params, so every FF is unspecified and defaults to V0;
//! the `FfAttr` map leaves the specified path open for when the IR
//! grows them. LUT, BRAM, and PIP lines are other workers' scope;
//! this module emits FF lines (INIT/SRVAL plus the FFMUX D select)
//! only.

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
/// Single source of truth: bitloc::ff_fasm_name (merged lut-ff-bram
/// tables); this wrapper only maps the error to String. Deliberately NOT
/// re-exported at the crate root, so it cannot collide with bitloc's copy.
pub fn ff_fasm_name(ff_index: u8) -> Result<&'static str, String> {
    crate::bitloc::ff_fasm_name(ff_index).map_err(|e| format!("{e}"))
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
pub fn ff_srval_fasm_line(site: &str, ff_index: u8, srval: Option<bool>) -> Result<String, String> {
    let name = ff_fasm_name(ff_index)?;
    let v = if srval.unwrap_or(false) { 1 } else { 0 };
    Ok(format!("{site}.{name}.SRVAL.V{v}"))
}

/// FASM line for one LUT INIT: `{site}.{letter}6LUT.INIT[63:0] = 0x{init:016x}`.
/// The spelling matches the harness golden FASM; the emitter's
/// parse-back renders `= 64'h...` for the same bits.
pub fn lut_init_fasm_line(site: &str, lut_index: u8, init: u64) -> Result<String, String> {
    let name = crate::bitloc::lut_fasm_name(lut_index).map_err(|e| format!("{e}"))?;
    Ok(format!("{site}.{name}.INIT[63:0] = 0x{init:016x}"))
}

/// Emit LUT INIT FASM for a placed design: one line per used LUT BEL,
/// in deterministic order (placed-slice order, then BEL index
/// ascending). The placed site name is the FASM tile, not the packed
/// slice name.
///
/// Fails loud when a LUT has no recorded INIT, and when a LUT is used
/// as distributed RAM or SRL: LUTRAM emission is BLOCKED (no LUTRAM
/// table) and a plain LUT INIT line for it would miscompile.
pub fn emit_lut_fasm(
    packed: &[PackedUltraSlice],
    placed: &UltraPlaced,
) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for p in &placed.slices {
        let slice = packed.get(p.packed_idx).ok_or_else(|| {
            format!(
                "ULTRA FASM: placed slice {} references packed index {} out of range",
                p.site, p.packed_idx
            )
        })?;
        let mut bels: Vec<u8> = slice.luts.keys().copied().collect();
        bels.sort_unstable();
        for bel in bels {
            if slice.lut_is_memory.get(&bel).copied().unwrap_or(false) {
                return Err(format!(
                    "ULTRA FASM: LUT {} in slice {} is used as distributed RAM or SRL; \
                     LUTRAM emission is blocked (no LUTRAM table), refusing to emit a plain LUT INIT line",
                    slice.luts[&bel], p.site
                ));
            }
            let init = slice.lut_inits.get(&bel).ok_or_else(|| {
                format!(
                    "ULTRA FASM: LUT {} in slice {} has no recorded INIT value",
                    slice.luts[&bel], p.site
                )
            })?;
            lines.push(lut_init_fasm_line(&p.site, bel, *init)?);
        }
    }
    Ok(lines)
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

/// Which FFMUX input feeds an FF's D pin (FFMUX corpus census,
/// tools/ffmux-corpus/FFMUX_CORPUS_REPORT.md, Q2: Vivado encodes a
/// same-slice LUT-O to FF-D edge as D6 in 119,729 cases and as BYP with
/// a fabric route from the same LUT O in exactly 7 of 456,354 rows).
/// `D6`: the same-slice, same-letter LUT O6 feeds D directly inside the
/// slice, so no fabric route exists for that edge. `Byp`: D arrives
/// from outside the slice through the `{letter}X` site pin, routed on
/// the fabric as the flow already does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FfMuxSel {
    D6,
    Byp,
}

impl FfMuxSel {
    pub fn as_str(self) -> &'static str {
        match self {
            FfMuxSel::D6 => "D6",
            FfMuxSel::Byp => "BYP",
        }
    }
}

/// FASM line for one FF D mux: `{site}.FFMUX{letter}{1,2}.{sel}`, e.g.
/// `SLICE_X1Y0.FFMUXA1.D6` for BEL 0, `SLICE_X1Y0.FFMUXA2.BYP` for
/// BEL 1. The letter is `bel / 2`, the number is `bel % 2 + 1`, the
/// same mapping as `ff_fasm_name` (BEL 0 is AFF / FFMUXA1).
pub fn ffmux_fasm_line(site: &str, ff_index: u8, sel: FfMuxSel) -> Result<String, String> {
    let name = ff_fasm_name(ff_index)?;
    let letter = name.chars().next().expect("ff_fasm_name is never empty");
    let num = ff_index % 2 + 1;
    Ok(format!("{site}.FFMUX{letter}{num}.{}", sel.as_str()))
}

/// Emit FFMUX FASM for a placed design: for every used FF BEL in every
/// placed slice, one `{site}.FFMUX{letter}{1,2}.{D6|BYP}` line, in
/// deterministic order (placed-slice order, then BEL index ascending).
/// `sels` maps FF cell name to its selection, as decided by the flow
/// from the design nets (same-slice same-letter LUT O driving D means
/// D6, anything else means BYP); a cell missing from the map fails
/// loud instead of silently defaulting, because the map is built for
/// every packed FF by construction. The placed site name is the FASM
/// tile, not the packed slice name. FFMUX lines resolve through the
/// generic segbits lookup (BYP sets only 15_01, D6 only 14_05 for
/// FFMUXA1); no segbits change is needed.
pub fn emit_ffmux_fasm(
    packed: &[PackedUltraSlice],
    placed: &UltraPlaced,
    sels: &HashMap<String, FfMuxSel>,
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
            let sel = sels.get(cell).ok_or_else(|| {
                format!(
                    "ULTRA FASM: FF {cell} in slice {} has no FFMUX selection",
                    p.site
                )
            })?;
            lines.push(ffmux_fasm_line(&p.site, bel, *sel)?);
        }
    }
    Ok(lines)
}

/// Emit OUTMUX FASM for live LUT letters with no FF in the letter:
/// one `{site}.OUTMUX{letter}.D6` line per such letter, in
/// deterministic order (placed-slice order, then letter ascending).
/// A LUT O drives the fabric through its `{letter}_O` pin
/// (LOGIC_OUTS wire) whether or not the letter has an FF, but the
/// route checker only counts a live LUT O as a driver when the
/// letter is selected by an FFMUX or an OUTMUX (us09v routecheck
/// `_slice_output_pins`, with a negative test pinning that a LUT
/// with no selection at all must not count). A slice with live LUTs
/// and no FFs at all (the FF-less slice of a two-slice counter)
/// therefore left every LOGIC_OUTS leg undriven. Selecting D6 on
/// the letter's OUTMUX records that the letter's O6 output is the
/// letter's mux output, which is what the LUT already drives, and
/// makes the checker count the pin. Letters with an FF already have
/// an FFMUX selection and need no OUTMUX line.
pub fn emit_outmux_fasm(
    packed: &[PackedUltraSlice],
    placed: &UltraPlaced,
) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for p in &placed.slices {
        let slice = packed.get(p.packed_idx).ok_or_else(|| {
            format!(
                "ULTRA FASM: placed slice {} references packed index {} out of range",
                p.site, p.packed_idx
            )
        })?;
        let mut lut_letters: Vec<u8> = slice.luts.keys().copied().collect();
        lut_letters.sort_unstable();
        for bel in lut_letters {
            let ff_in_letter = slice.ffs.contains_key(&(bel * 2))
                || slice.ffs.contains_key(&(bel * 2 + 1));
            if ff_in_letter {
                continue;
            }
            let letter = (b'A' + bel) as char;
            lines.push(format!("{}.OUTMUX{}.D6", p.site, letter));
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{ControlSetLimits, SliceKind, pack_ultra};
    use crate::place::{UltraGrid, UltraPlaceOpts, place_ultra};

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
                "AFF", "AFF2", "BFF", "BFF2", "CFF", "CFF2", "DFF", "DFF2", "EFF", "EFF2", "FFF",
                "FFF2", "GFF", "GFF2", "HFF", "HFF2",
            ]
        );
        assert!(ff_fasm_name(16).is_err());
    }

    #[test]
    fn outmux_lines_cover_ff_less_lut_letters_only() {
        // LUTs in letters A and B with an FF only in letter A: the
        // B letter gets OUTMUXB.D6, the A letter does not (its
        // FFMUX already selects it).
        let mut slice = PackedUltraSlice::new("SLICE_X0Y0".to_string(), SliceKind::Logic);
        slice.luts.insert(0, "lut_a".to_string());
        slice.lut_inits.insert(0, 0x8888888888888888);
        slice.luts.insert(1, "lut_b".to_string());
        slice.lut_inits.insert(1, 0x7fff8000ffff0000);
        slice.ffs.insert(0, "q".to_string());
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
        let lines = emit_outmux_fasm(&packed, &placed).unwrap();
        assert_eq!(lines, vec!["SLICE_X10Y20.OUTMUXB.D6".to_string()]);
    }

    #[test]
    fn ffmux_lines_use_letter_and_index_from_bel() {
        // BEL 0 is AFF, so FFMUXA1; BEL 1 is AFF2, so FFMUXA2. D6 for
        // the FF whose D is the same-letter LUT O, BYP otherwise.
        let mut slice = PackedUltraSlice::new("SLICE_X0Y0".to_string(), SliceKind::Logic);
        slice.ffs.insert(0, "q".to_string());
        slice.ffs.insert(1, "q2".to_string());
        slice.ff_clk.insert(0, "clk".to_string());
        slice.ff_clk.insert(1, "clk".to_string());
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
        let mut sels = HashMap::new();
        sels.insert("q".to_string(), FfMuxSel::D6);
        sels.insert("q2".to_string(), FfMuxSel::Byp);
        let lines = emit_ffmux_fasm(&packed, &placed, &sels).unwrap();
        assert_eq!(
            lines,
            vec![
                "SLICE_X10Y20.FFMUXA1.D6".to_string(),
                "SLICE_X10Y20.FFMUXA2.BYP".to_string(),
            ]
        );
    }

    #[test]
    fn ffmux_missing_selection_fails_loud() {
        let (packed, placed) = one_ff_packed();
        let err = emit_ffmux_fasm(&packed, &placed, &HashMap::new()).unwrap_err();
        assert!(err.contains("no FFMUX selection"), "got {err}");
    }

    #[test]
    fn ffmux_names_cover_all_sixteen() {
        let names: Vec<String> = (0..16)
            .map(|i| ffmux_fasm_line("SLICE_X0Y0", i, FfMuxSel::D6).unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "SLICE_X0Y0.FFMUXA1.D6",
                "SLICE_X0Y0.FFMUXA2.D6",
                "SLICE_X0Y0.FFMUXB1.D6",
                "SLICE_X0Y0.FFMUXB2.D6",
                "SLICE_X0Y0.FFMUXC1.D6",
                "SLICE_X0Y0.FFMUXC2.D6",
                "SLICE_X0Y0.FFMUXD1.D6",
                "SLICE_X0Y0.FFMUXD2.D6",
                "SLICE_X0Y0.FFMUXE1.D6",
                "SLICE_X0Y0.FFMUXE2.D6",
                "SLICE_X0Y0.FFMUXF1.D6",
                "SLICE_X0Y0.FFMUXF2.D6",
                "SLICE_X0Y0.FFMUXG1.D6",
                "SLICE_X0Y0.FFMUXG2.D6",
                "SLICE_X0Y0.FFMUXH1.D6",
                "SLICE_X0Y0.FFMUXH2.D6",
            ]
        );
        assert!(ffmux_fasm_line("SLICE_X0Y0", 16, FfMuxSel::D6).is_err());
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
        let placed = place_ultra(&packed.slices, &SmallGrid, &UltraPlaceOpts::default()).unwrap();
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

    fn one_lut_packed() -> (Vec<PackedUltraSlice>, UltraPlaced) {
        let mut slice = PackedUltraSlice::new("SLICE_X0Y0".to_string(), SliceKind::Logic);
        slice.luts.insert(0, "inv".to_string());
        slice.lut_inits.insert(0, 0x5555555555555555);
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
        (packed, placed)
    }

    #[test]
    fn lut_init_emits_golden_spelling() {
        let (packed, placed) = one_lut_packed();
        let lines = emit_lut_fasm(&packed, &placed).unwrap();
        assert_eq!(
            lines,
            vec!["SLICE_X0Y0.A6LUT.INIT[63:0] = 0x5555555555555555".to_string()]
        );
    }

    #[test]
    fn lut_init_emits_in_bel_order_at_placed_site() {
        let mut slice = PackedUltraSlice::new("SLICE_X0Y0".to_string(), SliceKind::Logic);
        slice.luts.insert(3, "d".to_string());
        slice.lut_inits.insert(3, 0x7f807f807f807f80);
        slice.luts.insert(1, "b".to_string());
        slice.lut_inits.insert(1, 0x6666666666666666);
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
        let lines = emit_lut_fasm(&packed, &placed).unwrap();
        assert_eq!(
            lines,
            vec![
                "SLICE_X10Y20.B6LUT.INIT[63:0] = 0x6666666666666666".to_string(),
                "SLICE_X10Y20.D6LUT.INIT[63:0] = 0x7f807f807f807f80".to_string(),
            ]
        );
    }

    #[test]
    fn lut_init_memory_lut_fails_loud() {
        let (mut packed, placed) = one_lut_packed();
        packed[0].lut_is_memory.insert(0, true);
        let err = emit_lut_fasm(&packed, &placed).unwrap_err();
        assert!(err.contains("LUTRAM"), "got {err}");
    }

    #[test]
    fn lut_init_missing_init_fails_loud() {
        let (mut packed, placed) = one_lut_packed();
        packed[0].lut_inits.remove(&0);
        let err = emit_lut_fasm(&packed, &placed).unwrap_err();
        assert!(err.contains("no recorded INIT"), "got {err}");
    }

    #[test]
    fn lut_init_bad_bel_index_fails_loud() {
        assert!(lut_init_fasm_line("SLICE_X0Y0", 8, 0).is_err());
    }

    #[test]
    fn pack_place_then_generate_lut_init() {
        // End to end through the real packer and placer: one LUT plus
        // one FF yields the LUT INIT line for the placed site.
        let limits = ControlSetLimits::default();
        let luts = vec![("lut0".to_string(), 0xAAAAAAAAAAAAAAAAu64, false)];
        let ffs = vec![("q_reg".to_string(), "clk".to_string(), None, None)];
        let packed = pack_ultra(&luts, &ffs, &[], &limits).unwrap();
        let placed = place_ultra(&packed.slices, &SmallGrid, &UltraPlaceOpts::default()).unwrap();
        let lines = emit_lut_fasm(&packed.slices, &placed).unwrap();
        assert_eq!(lines.len(), 1, "one LUT gives one INIT line");
        assert!(
            lines[0].ends_with(".A6LUT.INIT[63:0] = 0xaaaaaaaaaaaaaaaa"),
            "got {}",
            lines[0]
        );
    }
}
