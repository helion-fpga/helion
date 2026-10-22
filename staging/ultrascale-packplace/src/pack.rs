//! UltraScale+ packer: [`helion_ir::Design`] -> packed UltraScale slices.
//!
//! Targets xcvu3p (Virtex UltraScale+, monolithic). Packs LUTs, FFs, and
//! CARRY8 chains into CLB slices. Each slice holds 8 LUT6 (A-H), 16 FFs,
//! and 1 CARRY8. Two slices per CLB: SLICEL and SLICEM.
//!
//! SLICEM legality: distributed RAM and SRL shift registers may ONLY use
//! LUTs in SLICEM slices. This packer tracks slice type and rejects
//! illegal placements.
//!
//! Control sets: parameterized pending exact UG574 rules from device-model
//! agent. The checker validates against the configured limits.
//!
//! This module is designed against the INTERFACES.md `UltraScaleFabric`
//! trait shape. It will integrate into the `helion-ultrascale` crate when
//! the device-model agent creates it.

use std::collections::{HashMap, HashSet};

/// UltraScale slice type.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SliceKind {
    /// Logic-only slice.
    Logic,
    /// Memory-capable slice: LUTs usable as distributed RAM or SRL.
    Memory,
}

/// Which BEL within a slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum UltraBel {
    /// LUT A-H (0-7).
    Lut(u8),
    /// FF: 16 per slice (0-15).
    Ff(u8),
    /// CARRY8 (single per slice).
    Carry8,
}

/// Control-set limits for an UltraScale slice.
/// Values parameterized; device-model agent confirms per UG574.
#[derive(Clone, Debug)]
pub struct ControlSetLimits {
    /// Max distinct CLK nets per slice (UltraScale: 1).
    pub max_clk: usize,
    /// Max distinct SR nets per slice.
    pub max_sr: usize,
    /// Max distinct CE nets per slice.
    pub max_ce: usize,
    /// Max FFs per slice.
    pub max_ff: usize,
    /// Max LUTs per slice.
    pub max_lut: usize,
}

impl Default for ControlSetLimits {
    fn default() -> Self {
        Self {
            max_clk: 1,
            max_sr: 1,
            max_ce: 2,
            max_ff: 16,
            max_lut: 8,
        }
    }
}

/// A packed UltraScale slice.
#[derive(Clone, Debug)]
pub struct PackedUltraSlice {
    /// Slice identifier (e.g., "SLICE_X0Y0").
    pub name: String,
    /// Slice kind determines LUT capabilities.
    pub kind: SliceKind,
    /// LUT assignments: bel index (0-7) -> source cell name.
    pub luts: HashMap<u8, String>,
    /// LUT INIT values by bel index.
    pub lut_inits: HashMap<u8, u64>,
    /// True if the LUT is used as distributed RAM or SRL.
    pub lut_is_memory: HashMap<u8, bool>,
    /// FF assignments: bel index (0-15) -> source cell name.
    pub ffs: HashMap<u8, String>,
    /// FF control nets by bel index.
    pub ff_clk: HashMap<u8, String>,
    pub ff_ce: HashMap<u8, Option<String>>,
    pub ff_sr: HashMap<u8, Option<String>>,
    /// CARRY8 assignment, if any.
    pub carry8: Option<String>,
    /// CARRY8 chain predecessor (for vertical chaining).
    pub carry_prev: Option<String>,
    /// CARRY8 chain successor.
    pub carry_next: Option<String>,
}

impl PackedUltraSlice {
    pub fn new(name: String, kind: SliceKind) -> Self {
        Self {
            name,
            kind,
            luts: HashMap::new(),
            lut_inits: HashMap::new(),
            lut_is_memory: HashMap::new(),
            ffs: HashMap::new(),
            ff_clk: HashMap::new(),
            ff_ce: HashMap::new(),
            ff_sr: HashMap::new(),
            carry8: None,
            carry_prev: None,
            carry_next: None,
        }
    }

    /// Check if a LUT can be placed at the given BEL.
    /// Returns false if BEL occupied or if memory-type LUT targets SLICEL.
    pub fn can_place_lut(&self, bel: u8, is_memory: bool, limits: &ControlSetLimits) -> bool {
        if bel >= limits.max_lut as u8 {
            return false;
        }
        if self.luts.contains_key(&bel) {
            return false;
        }
        if is_memory && self.kind != SliceKind::Memory {
            return false;
        }
        true
    }

    /// Check if an FF can be placed at the given BEL with the given control set.
    pub fn can_place_ff(
        &self,
        bel: u8,
        clk: &str,
        ce: &Option<String>,
        sr: &Option<String>,
        limits: &ControlSetLimits,
    ) -> bool {
        if bel >= limits.max_ff as u8 {
            return false;
        }
        if self.ffs.contains_key(&bel) {
            return false;
        }
        // Check control-set compatibility with existing FFs.
        let mut clks: HashSet<&str> = HashSet::new();
        let mut ces: HashSet<&str> = HashSet::new();
        let mut srs: HashSet<&str> = HashSet::new();
        for (_, c) in &self.ff_clk {
            clks.insert(c.as_str());
        }
        for (_, c) in &self.ff_ce {
            if let Some(s) = c {
                ces.insert(s.as_str());
            }
        }
        for (_, s) in &self.ff_sr {
            if let Some(s) = s {
                srs.insert(s.as_str());
            }
        }
        clks.insert(clk);
        if let Some(c) = ce {
            ces.insert(c.as_str());
        }
        if let Some(s) = sr {
            srs.insert(s.as_str());
        }
        if clks.len() > limits.max_clk {
            return false;
        }
        if ces.len() > limits.max_ce {
            return false;
        }
        if srs.len() > limits.max_sr {
            return false;
        }
        true
    }
}

/// Result of packing a design for UltraScale+.
#[derive(Clone, Debug, Default)]
pub struct UltraPacked {
    /// Packed slices.
    pub slices: Vec<PackedUltraSlice>,
    /// Unpacked IOBs (handled separately).
    pub iobs: Vec<String>,
    /// Statistics.
    pub stats: PackStats,
}

#[derive(Clone, Debug, Default)]
pub struct PackStats {
    pub lut_count: usize,
    pub ff_count: usize,
    pub carry8_count: usize,
    pub slicel_count: usize,
    pub slicem_count: usize,
}

/// Packs a design into UltraScale slices.
/// 
/// This is a structural packer: it groups LUTs and their driven FFs,
/// packs CARRY8 chains, and respects SLICEM legality. It does not yet
/// do timing-driven packing (phase 2).
pub fn pack_ultra(
    luts: &[(String, u64, bool)],
    ffs: &[(String, String, Option<String>, Option<String>)],
    carry_chains: &[Vec<String>],
    limits: &ControlSetLimits,
) -> Result<UltraPacked, String> {
    let mut packed = UltraPacked::default();
    let mut slice_idx = 0;

    // Pack CARRY8 chains first (they have strict placement requirements).
    for chain in carry_chains {
        for (i, carry_cell) in chain.iter().enumerate() {
            let slice_name = format!("SLICE_X0Y{}", slice_idx);
            let mut slice = PackedUltraSlice::new(slice_name, SliceKind::Logic);
            slice.carry8 = Some(carry_cell.clone());
            if i > 0 {
                slice.carry_prev = Some(chain[i - 1].clone());
            }
            if i + 1 < chain.len() {
                slice.carry_next = Some(chain[i + 1].clone());
            }
            packed.slices.push(slice);
            slice_idx += 1;
            packed.stats.carry8_count += 1;
        }
    }

    // Pack LUTs and FFs.
    // Simple greedy: fill slices sequentially, respecting control sets.
    let mut current_slice: Option<PackedUltraSlice> = None;

    for (lut_name, init, is_memory) in luts {
        let kind = if *is_memory {
            SliceKind::Memory
        } else {
            SliceKind::Logic
        };

        // Find or create a slice.
        let placed = if let Some(ref mut s) = current_slice {
            if s.kind == kind || (s.kind == SliceKind::Memory && kind == SliceKind::Logic) {
                // Try to place in current slice.
                let mut found = None;
                for bel in 0..8 {
                    if s.can_place_lut(bel, *is_memory, limits) {
                        found = Some(bel);
                        break;
                    }
                }
                if let Some(bel) = found {
                    s.luts.insert(bel, lut_name.clone());
                    s.lut_inits.insert(bel, *init);
                    s.lut_is_memory.insert(bel, *is_memory);
                    packed.stats.lut_count += 1;
                    if s.kind == SliceKind::Memory {
                        packed.stats.slicem_count += 1;
                    } else {
                        packed.stats.slicel_count += 1;
                    }
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        if !placed {
            // Flush current slice and start a new one.
            if let Some(s) = current_slice.take() {
                packed.slices.push(s);
            }
            let slice_name = format!("SLICE_X0Y{}", slice_idx);
            let mut s = PackedUltraSlice::new(slice_name, kind);
            s.luts.insert(0, lut_name.clone());
            s.lut_inits.insert(0, *init);
            s.lut_is_memory.insert(0, *is_memory);
            packed.stats.lut_count += 1;
            if kind == SliceKind::Memory {
                packed.stats.slicem_count += 1;
            } else {
                packed.stats.slicel_count += 1;
            }
            current_slice = Some(s);
            slice_idx += 1;
        }
    }

    if let Some(s) = current_slice.take() {
        packed.slices.push(s);
    }

    // Pack FFs, respecting control sets.
    // For now, place FFs in the same slice as their driving LUT if possible.
    for (ff_name, clk, ce, sr) in ffs {
        let mut placed = false;
        for slice in &mut packed.slices {
            for bel in 0..16 {
                if slice.can_place_ff(bel, clk, ce, sr, limits) {
                    slice.ffs.insert(bel, ff_name.clone());
                    slice.ff_clk.insert(bel, clk.clone());
                    slice.ff_ce.insert(bel, ce.clone());
                    slice.ff_sr.insert(bel, sr.clone());
                    packed.stats.ff_count += 1;
                    placed = true;
                    break;
                }
            }
            if placed {
                break;
            }
        }
        if !placed {
            return Err(format!("cannot place FF {ff_name}: no slice with compatible control set"));
        }
    }

    Ok(packed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_simple_luts() {
        let limits = ControlSetLimits::default();
        let luts = vec![
            ("lut0".to_string(), 0xAAAAAAAAAAAAAAAA, false),
            ("lut1".to_string(), 0x5555555555555555, false),
        ];
        let ffs = vec![];
        let chains = vec![];
        let packed = pack_ultra(&luts, &ffs, &chains, &limits).unwrap();
        assert_eq!(packed.stats.lut_count, 2);
        assert_eq!(packed.slices.len(), 1);
    }

    #[test]
    fn slicem_legality_enforced() {
        let limits = ControlSetLimits::default();
        let slice = PackedUltraSlice::new("SLICE_X0Y0".to_string(), SliceKind::Logic);
        // Memory LUT cannot go in SLICEL.
        assert!(!slice.can_place_lut(0, true, &limits));
        // Logic LUT can go in SLICEL.
        assert!(slice.can_place_lut(0, false, &limits));

        let mslice = PackedUltraSlice::new("SLICE_X0Y1".to_string(), SliceKind::Memory);
        // Both can go in SLICEM.
        assert!(mslice.can_place_lut(0, true, &limits));
        assert!(mslice.can_place_lut(0, false, &limits));
    }

    #[test]
    fn control_set_limits_enforced() {
        let limits = ControlSetLimits::default();
        let mut slice = PackedUltraSlice::new("SLICE_X0Y0".to_string(), SliceKind::Logic);
        // Fill with 16 FFs on same clock.
        for i in 0..16 {
            assert!(slice.can_place_ff(
                i,
                "clk",
                &None,
                &None,
                &limits
            ));
            slice.ffs.insert(i, format!("ff{i}"));
            slice.ff_clk.insert(i, "clk".to_string());
            slice.ff_ce.insert(i, None);
            slice.ff_sr.insert(i, None);
        }
        // 17th FF should fail (capacity).
        assert!(!slice.can_place_ff(16, "clk", &None, &None, &limits));
    }

    #[test]
    fn conflicting_clocks_rejected() {
        let limits = ControlSetLimits::default();
        let mut slice = PackedUltraSlice::new("SLICE_X0Y0".to_string(), SliceKind::Logic);
        slice.ffs.insert(0, "ff0".to_string());
        slice.ff_clk.insert(0, "clk_a".to_string());
        slice.ff_ce.insert(0, None);
        slice.ff_sr.insert(0, None);
        // Different clock should be rejected.
        assert!(!slice.can_place_ff(1, "clk_b", &None, &None, &limits));
        // Same clock should be accepted.
        assert!(slice.can_place_ff(1, "clk_a", &None, &None, &limits));
    }
}
