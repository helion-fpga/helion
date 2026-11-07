//! UltraScale+ placement legality checker.
//!
//! Verifies that every placed cell obeys the architectural rules:
//! - BEL capacity: at most 8 LUTs, 16 FFs, 1 CARRY8 per slice.
//! - SLICEM legality: distributed RAM and SRL LUTs only on Memory-kind slices.
//! - Control sets: CLK/SR/CE sharing rules per slice.
//! - CARRY8 chains: vertically contiguous, X constant, Y ascending.
//! - No site used twice.
//!
//! This checker runs as a test on every placed design. A placement that
//! fails legality is a bug, not a QoR issue.

use crate::pack::{ControlSetLimits, PackedUltraSlice, SliceKind};
use crate::place::{PlacedUltraSlice, UltraGrid};
use std::collections::{HashMap, HashSet};

/// A legality violation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LegalityViolation {
    /// Slice exceeds LUT capacity.
    TooManyLuts { slice: String, count: usize, max: usize },
    /// Slice exceeds FF capacity.
    TooManyFfs { slice: String, count: usize, max: usize },
    /// Slice has more than one CARRY8.
    MultipleCarry8 { slice: String },
    /// Memory-type LUT on a Logic slice.
    MemoryLutOnSlicel { slice: String, lut: String },
    /// Control-set violation.
    ControlSet {
        slice: String,
        detail: String,
    },
    /// CARRY8 chain not vertically contiguous.
    CarryChainBroken {
        chain: Vec<String>,
        detail: String,
    },
    /// Site used by multiple slices.
    DuplicateSite { site: String },
    /// Placed on wrong slice kind.
    WrongSliceKind {
        slice: String,
        expected: SliceKind,
        actual: SliceKind,
    },
}

impl std::fmt::Display for LegalityViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyLuts { slice, count, max } => {
                write!(f, "{slice}: {count} LUTs exceeds max {max}")
            }
            Self::TooManyFfs { slice, count, max } => {
                write!(f, "{slice}: {count} FFs exceeds max {max}")
            }
            Self::MultipleCarry8 { slice } => {
                write!(f, "{slice}: multiple CARRY8 in one slice")
            }
            Self::MemoryLutOnSlicel { slice, lut } => {
                write!(f, "{slice}: memory LUT {lut} on SLICEL (requires SLICEM)")
            }
            Self::ControlSet { slice, detail } => {
                write!(f, "{slice}: control-set violation: {detail}")
            }
            Self::CarryChainBroken { chain, detail } => {
                write!(f, "CARRY8 chain {chain:?} broken: {detail}")
            }
            Self::DuplicateSite { site } => {
                write!(f, "site {site} used by multiple slices")
            }
            Self::WrongSliceKind {
                slice,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "{slice}: expected {expected:?} slice, placed on {actual:?}"
                )
            }
        }
    }
}

/// Checks legality of a placed design.
///
/// Returns a list of violations (empty = legal).
pub fn check_legality(
    packed: &[PackedUltraSlice],
    placed: &[PlacedUltraSlice],
    grid: &dyn UltraGrid,
    limits: &ControlSetLimits,
) -> Vec<LegalityViolation> {
    let mut violations = Vec::new();

    // Build site -> kind map.
    let site_kind: HashMap<String, SliceKind> = grid
        .slice_sites()
        .into_iter()
        .map(|(site, _, _, _, kind)| (site, kind))
        .collect();

    // Check for duplicate sites.
    let mut seen_sites = HashSet::new();
    for p in placed {
        if !seen_sites.insert(&p.site) {
            violations.push(LegalityViolation::DuplicateSite {
                site: p.site.clone(),
            });
        }
    }

    // Check each placed slice.
    for p in placed {
        let Some(slice) = packed.get(p.packed_idx) else {
            continue;
        };

        // Capacity checks.
        if slice.luts.len() > limits.max_lut {
            violations.push(LegalityViolation::TooManyLuts {
                slice: p.site.clone(),
                count: slice.luts.len(),
                max: limits.max_lut,
            });
        }
        if slice.ffs.len() > limits.max_ff {
            violations.push(LegalityViolation::TooManyFfs {
                slice: p.site.clone(),
                count: slice.ffs.len(),
                max: limits.max_ff,
            });
        }

        // SLICEM legality.
        let actual_kind = site_kind.get(&p.site).copied().unwrap_or(SliceKind::Logic);
        for (bel, lut_name) in &slice.luts {
            let is_mem = slice.lut_is_memory.get(bel).copied().unwrap_or(false);
            if is_mem && actual_kind != SliceKind::Memory {
                violations.push(LegalityViolation::MemoryLutOnSlicel {
                    slice: p.site.clone(),
                    lut: lut_name.clone(),
                });
            }
        }

        // If the packed slice requires Memory kind, verify placement.
        if slice.kind == SliceKind::Memory && actual_kind != SliceKind::Memory {
            violations.push(LegalityViolation::WrongSliceKind {
                slice: p.site.clone(),
                expected: SliceKind::Memory,
                actual: actual_kind,
            });
        }

        // Control-set checks.
        let mut clks: HashSet<&str> = HashSet::new();
        let mut ces: HashSet<&str> = HashSet::new();
        let mut srs: HashSet<&str> = HashSet::new();
        for (_, clk) in &slice.ff_clk {
            clks.insert(clk.as_str());
        }
        for (_, ce) in &slice.ff_ce {
            if let Some(c) = ce {
                ces.insert(c.as_str());
            }
        }
        for (_, sr) in &slice.ff_sr {
            if let Some(s) = sr {
                srs.insert(s.as_str());
            }
        }
        if clks.len() > limits.max_clk {
            violations.push(LegalityViolation::ControlSet {
                slice: p.site.clone(),
                detail: format!(
                    "{} distinct CLK nets exceeds max {}",
                    clks.len(),
                    limits.max_clk
                ),
            });
        }
        if ces.len() > limits.max_ce {
            violations.push(LegalityViolation::ControlSet {
                slice: p.site.clone(),
                detail: format!(
                    "{} distinct CE nets exceeds max {}",
                    ces.len(),
                    limits.max_ce
                ),
            });
        }
        if srs.len() > limits.max_sr {
            violations.push(LegalityViolation::ControlSet {
                slice: p.site.clone(),
                detail: format!(
                    "{} distinct SR nets exceeds max {}",
                    srs.len(),
                    limits.max_sr
                ),
            });
        }
    }

    // Check CARRY8 chain contiguity.
    // Group placed slices by chain via packed carry_prev/carry_next.
    let mut chain_groups: HashMap<String, Vec<&PlacedUltraSlice>> = HashMap::new();
    for p in placed {
        if let Some(slice) = packed.get(p.packed_idx) {
            if slice.carry8.is_some() {
                // Find chain head by walking carry_prev.
                let mut head_idx = p.packed_idx;
                let mut cur = p.packed_idx;
                while let Some(prev_name) = &packed[cur].carry_prev {
                    let mut found = None;
                    for (idx, s) in packed.iter().enumerate() {
                        if s.carry8.as_ref() == Some(prev_name) {
                            found = Some(idx);
                            break;
                        }
                    }
                    if let Some(pi) = found {
                        head_idx = pi;
                        cur = pi;
                    } else {
                        break;
                    }
                }
                let head_name = packed[head_idx]
                    .carry8
                    .clone()
                    .unwrap_or_default();
                chain_groups.entry(head_name).or_default().push(p);
            }
        }
    }

    for (chain_name, mut group) in chain_groups {
        if group.len() < 2 {
            continue;
        }
        // Sort by Y coordinate.
        group.sort_by_key(|p| p.y);
        for w in 1..group.len() {
            if group[w].x != group[w - 1].x {
                violations.push(LegalityViolation::CarryChainBroken {
                    chain: vec![chain_name.clone()],
                    detail: format!(
                        "X changed from {} to {}",
                        group[w - 1].x, group[w].x
                    ),
                });
            }
            if group[w].y != group[w - 1].y + 1 {
                violations.push(LegalityViolation::CarryChainBroken {
                    chain: vec![chain_name.clone()],
                    detail: format!(
                        "Y not contiguous: {} -> {}",
                        group[w - 1].y, group[w].y
                    ),
                });
            }
        }
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::PackedUltraSlice;
    use crate::place::{UltraGrid, UltraPlaceOpts, place_ultra};

    struct FakeGrid {
        sites: Vec<(String, String, i32, i32, SliceKind)>,
    }

    impl UltraGrid for FakeGrid {
        fn slice_sites(&self) -> Vec<(String, String, i32, i32, SliceKind)> {
            self.sites.clone()
        }
        fn grid_size(&self) -> (i32, i32) {
            (10, 10)
        }
    }

    fn test_grid() -> FakeGrid {
        let mut sites = Vec::new();
        for y in 0..10 {
            for x in 0..10 {
                let kind = if (x + y) % 4 == 0 {
                    SliceKind::Memory
                } else {
                    SliceKind::Logic
                };
                sites.push((
                    format!("SLICE_X{x}Y{y}"),
                    format!("CLB_X{x}Y{y}"),
                    x,
                    y,
                    kind,
                ));
            }
        }
        FakeGrid { sites }
    }

    #[test]
    fn legal_placement_passes() {
        let grid = test_grid();
        let limits = ControlSetLimits::default();
        let mut s0 = PackedUltraSlice::new("s0".to_string(), SliceKind::Logic);
        s0.luts.insert(0, "lut0".to_string());
        s0.lut_inits.insert(0, 0xAAAAAAAAAAAAAAAA);
        s0.lut_is_memory.insert(0, false);
        s0.ffs.insert(0, "ff0".to_string());
        s0.ff_clk.insert(0, "clk".to_string());
        s0.ff_ce.insert(0, None);
        s0.ff_sr.insert(0, None);

        let packed = vec![s0];
        let placed = place_ultra(&packed, &grid, &UltraPlaceOpts::default()).unwrap();
        let violations = check_legality(&packed, &placed.slices, &grid, &limits);
        assert!(violations.is_empty(), "violations: {violations:?}");
    }

    #[test]
    fn catches_memory_lut_on_slicel() {
        let grid = test_grid();
        let limits = ControlSetLimits::default();
        // Manually place a memory LUT on a Logic site.
        let mut s0 = PackedUltraSlice::new("s0".to_string(), SliceKind::Memory);
        s0.luts.insert(0, "ram_lut".to_string());
        s0.lut_is_memory.insert(0, true);

        // Find a Logic site.
        let logic_site = grid
            .slice_sites()
            .iter()
            .find(|(_, _, _, _, k)| *k == SliceKind::Logic)
            .unwrap()
            .clone();

        let placed = vec![crate::place::PlacedUltraSlice {
            packed_idx: 0,
            site: logic_site.0.clone(),
            tile: logic_site.1.clone(),
            x: logic_site.2,
            y: logic_site.3,
        }];

        let violations = check_legality(&vec![s0], &placed, &grid, &limits);
        assert!(!violations.is_empty());
        assert!(violations.iter().any(|v| matches!(
            v,
            LegalityViolation::MemoryLutOnSlicel { .. }
                | LegalityViolation::WrongSliceKind { .. }
        )));
    }

    #[test]
    fn catches_control_set_violation() {
        let grid = test_grid();
        let limits = ControlSetLimits::default();
        let mut s0 = PackedUltraSlice::new("s0".to_string(), SliceKind::Logic);
        // Two FFs with different clocks.
        s0.ffs.insert(0, "ff0".to_string());
        s0.ff_clk.insert(0, "clk_a".to_string());
        s0.ff_ce.insert(0, None);
        s0.ff_sr.insert(0, None);
        s0.ffs.insert(1, "ff1".to_string());
        s0.ff_clk.insert(1, "clk_b".to_string());
        s0.ff_ce.insert(1, None);
        s0.ff_sr.insert(1, None);

        let placed = place_ultra(&vec![s0.clone()], &grid, &UltraPlaceOpts::default()).unwrap();
        let violations = check_legality(&vec![s0], &placed.slices, &grid, &limits);
        assert!(violations.iter().any(|v| matches!(
            v,
            LegalityViolation::ControlSet { .. }
        )));
    }
}
