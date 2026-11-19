//! UltraScale+ placement legality checker.
//!
//! Verifies that every placed cell obeys the architectural rules:
//! - BEL capacity: at most 8 LUTs, 16 FFs, 1 CARRY8 per slice.
//! - SLICEM legality: distributed RAM and SRL LUTs only on Memory-kind slices.
//! - Control sets: CLK/SR/CE sharing rules per slice.
//! - CARRY8 chains: vertically contiguous, X constant, Y ascending,
//!   in carry-link order.
//! - No site used twice.
//! - Every packed slice placed, every placed index valid, every placed
//!   site known to the grid. Anything else fails loud here, never
//!   passes silently.
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
    TooManyLuts {
        slice: String,
        count: usize,
        max: usize,
    },
    /// Slice exceeds FF capacity.
    TooManyFfs {
        slice: String,
        count: usize,
        max: usize,
    },
    /// Slice has more than one CARRY8.
    MultipleCarry8 { slice: String },
    /// Memory-type LUT on a Logic slice.
    MemoryLutOnSlicel { slice: String, lut: String },
    /// Control-set violation.
    ControlSet { slice: String, detail: String },
    /// CARRY8 chain not vertically contiguous.
    CarryChainBroken { chain: Vec<String>, detail: String },
    /// Site used by multiple slices.
    DuplicateSite { site: String },
    /// Placed on wrong slice kind.
    WrongSliceKind {
        slice: String,
        expected: SliceKind,
        actual: SliceKind,
    },
    /// Placed entry names a packed index that does not exist.
    UnknownPackedIndex { index: usize },
    /// Placed entry names a site the grid does not know.
    UnknownSite { site: String },
    /// Packed slice was never placed.
    UnplacedSlice { slice: String },
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
            Self::UnknownPackedIndex { index } => {
                write!(f, "placed entry names unknown packed index {index}")
            }
            Self::UnknownSite { site } => {
                write!(f, "placed on unknown site {site}")
            }
            Self::UnplacedSlice { slice } => {
                write!(f, "packed slice {slice} was never placed")
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

    // Check for multiple CARRY8 on one site: each placed slice holds at
    // most one CARRY8 by construction, so two at one site means two
    // slices were placed on the same site, each carrying one.
    let mut carry_per_site: HashMap<&str, usize> = HashMap::new();
    for p in placed {
        if let Some(slice) = packed.get(p.packed_idx) {
            if slice.carry8.is_some() {
                let count = carry_per_site.entry(p.site.as_str()).or_default();
                *count += 1;
                if *count > 1 {
                    violations.push(LegalityViolation::MultipleCarry8 {
                        slice: p.site.clone(),
                    });
                }
            }
        }
    }

    // Check each placed slice.
    for p in placed {
        // D2: an out-of-range packed index used to be skipped silently.
        // It is a violation now.
        let Some(slice) = packed.get(p.packed_idx) else {
            violations.push(LegalityViolation::UnknownPackedIndex {
                index: p.packed_idx,
            });
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

        // D3: a site unknown to the grid used to default to Logic.
        // SLICEM checks against an unknown site are meaningless, so
        // report it and skip only the kind-dependent checks for this
        // slice; capacity and control-set checks above still ran.
        let actual_kind = match site_kind.get(&p.site) {
            Some(kind) => Some(*kind),
            None => {
                violations.push(LegalityViolation::UnknownSite {
                    site: p.site.clone(),
                });
                None
            }
        };

        if let Some(actual_kind) = actual_kind {
            // SLICEM legality.
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
        }

        // Control-set checks.
        let mut clks: HashSet<&str> = HashSet::new();
        let mut ces: HashSet<&str> = HashSet::new();
        let mut srs: HashSet<&str> = HashSet::new();
        for clk in slice.ff_clk.values() {
            clks.insert(clk.as_str());
        }
        for ce in slice.ff_ce.values().flatten() {
            ces.insert(ce.as_str());
        }
        for sr in slice.ff_sr.values().flatten() {
            srs.insert(sr.as_str());
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

    // D5: every packed slice must be placed. A slice dropped by the
    // placer used to pass silently.
    let placed_idx: HashSet<usize> = placed.iter().map(|p| p.packed_idx).collect();
    for (idx, slice) in packed.iter().enumerate() {
        if !placed_idx.contains(&idx) {
            violations.push(LegalityViolation::UnplacedSlice {
                slice: slice.name.clone(),
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
                let head_name = packed[head_idx].carry8.clone().unwrap_or_default();
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
        // D6: the Y-sorted order must match the carry-link order.
        // Sorting alone would accept a chain placed in reverse Y order
        // (Y still ascends by 1, X stays constant) while the chain
        // links run the other way.
        let expected: Vec<usize> = match packed
            .iter()
            .position(|s| s.carry8.as_deref() == Some(chain_name.as_str()))
        {
            Some(head_idx) => {
                let mut order = vec![head_idx];
                let mut cur = head_idx;
                while let Some(next_name) = &packed[cur].carry_next {
                    match packed
                        .iter()
                        .position(|s| s.carry8.as_ref() == Some(next_name))
                    {
                        Some(nidx) if !order.contains(&nidx) => {
                            order.push(nidx);
                            cur = nidx;
                        }
                        _ => break,
                    }
                }
                order
            }
            None => Vec::new(),
        };
        let sorted_idx: Vec<usize> = group.iter().map(|p| p.packed_idx).collect();
        if expected.len() != group.len() || expected != sorted_idx {
            violations.push(LegalityViolation::CarryChainBroken {
                chain: vec![chain_name.clone()],
                detail: "placed order does not follow carry chain links".to_string(),
            });
        }
        for w in 1..group.len() {
            if group[w].x != group[w - 1].x {
                violations.push(LegalityViolation::CarryChainBroken {
                    chain: vec![chain_name.clone()],
                    detail: format!("X changed from {} to {}", group[w - 1].x, group[w].x),
                });
            }
            if group[w].y != group[w - 1].y + 1 {
                violations.push(LegalityViolation::CarryChainBroken {
                    chain: vec![chain_name.clone()],
                    detail: format!("Y not contiguous: {} -> {}", group[w - 1].y, group[w].y),
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
            LegalityViolation::MemoryLutOnSlicel { .. } | LegalityViolation::WrongSliceKind { .. }
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
        assert!(
            violations
                .iter()
                .any(|v| matches!(v, LegalityViolation::ControlSet { .. }))
        );
    }

    #[test]
    fn unknown_packed_index_fails_loud() {
        // D2: a placed entry naming a packed index that does not exist
        // used to be skipped silently.
        let grid = test_grid();
        let limits = ControlSetLimits::default();
        let s0 = PackedUltraSlice::new("s0".to_string(), SliceKind::Logic);
        let placed = vec![crate::place::PlacedUltraSlice {
            packed_idx: 7,
            site: "SLICE_X0Y1".to_string(),
            tile: "CLB_X0Y1".to_string(),
            x: 0,
            y: 1,
        }];
        let violations = check_legality(&vec![s0], &placed, &grid, &limits);
        assert!(
            violations
                .iter()
                .any(|v| matches!(v, LegalityViolation::UnknownPackedIndex { index: 7 }))
        );
    }

    #[test]
    fn unknown_site_fails_loud() {
        // D3: a placed site the grid does not know used to default to
        // Logic and could pass SLICEM checks.
        let grid = test_grid();
        let limits = ControlSetLimits::default();
        let s0 = PackedUltraSlice::new("s0".to_string(), SliceKind::Logic);
        let placed = vec![crate::place::PlacedUltraSlice {
            packed_idx: 0,
            site: "SLICE_X99Y99".to_string(),
            tile: "CLB_X99Y99".to_string(),
            x: 99,
            y: 99,
        }];
        let violations = check_legality(&vec![s0], &placed, &grid, &limits);
        assert!(
            violations
                .iter()
                .any(|v| matches!(v, LegalityViolation::UnknownSite { .. }))
        );
    }

    #[test]
    fn multiple_carry8_at_one_site_fails_loud() {
        // D4: the MultipleCarry8 variant was never produced.
        let grid = test_grid();
        let limits = ControlSetLimits::default();
        let mut s0 = PackedUltraSlice::new("s0".to_string(), SliceKind::Logic);
        s0.carry8 = Some("carry0".to_string());
        let mut s1 = PackedUltraSlice::new("s1".to_string(), SliceKind::Logic);
        s1.carry8 = Some("carry1".to_string());
        let placed = vec![
            crate::place::PlacedUltraSlice {
                packed_idx: 0,
                site: "SLICE_X0Y1".to_string(),
                tile: "CLB_X0Y1".to_string(),
                x: 0,
                y: 1,
            },
            crate::place::PlacedUltraSlice {
                packed_idx: 1,
                site: "SLICE_X0Y1".to_string(),
                tile: "CLB_X0Y1".to_string(),
                x: 0,
                y: 1,
            },
        ];
        let violations = check_legality(&vec![s0, s1], &placed, &grid, &limits);
        assert!(
            violations
                .iter()
                .any(|v| matches!(v, LegalityViolation::MultipleCarry8 { .. }))
        );
    }

    #[test]
    fn unplaced_slice_fails_loud() {
        // D5: a packed slice dropped by the placer used to pass.
        let grid = test_grid();
        let limits = ControlSetLimits::default();
        let s0 = PackedUltraSlice::new("s0".to_string(), SliceKind::Logic);
        let s1 = PackedUltraSlice::new("s1".to_string(), SliceKind::Logic);
        let placed = place_ultra(&vec![s0.clone()], &grid, &UltraPlaceOpts::default()).unwrap();
        let violations = check_legality(&vec![s0, s1], &placed.slices, &grid, &limits);
        assert!(violations.iter().any(|v| matches!(
            v,
            LegalityViolation::UnplacedSlice { slice } if slice == "s1"
        )));
    }

    #[test]
    fn reversed_chain_order_fails_loud() {
        // D6: Y-sorted contiguity alone accepts a chain placed in
        // reverse link order. Place carry1 below carry0 (same X, Y
        // ascending by 1) and the checker must still flag it.
        let grid = test_grid();
        let limits = ControlSetLimits::default();
        let mut s0 = PackedUltraSlice::new("s0".to_string(), SliceKind::Logic);
        s0.carry8 = Some("carry0".to_string());
        s0.carry_next = Some("carry1".to_string());
        let mut s1 = PackedUltraSlice::new("s1".to_string(), SliceKind::Logic);
        s1.carry8 = Some("carry1".to_string());
        s1.carry_prev = Some("carry0".to_string());
        // carry1 at Y1, carry0 at Y2: Y-sorted order is [s1, s0], but
        // the chain links run [s0, s1].
        let placed = vec![
            crate::place::PlacedUltraSlice {
                packed_idx: 1,
                site: "SLICE_X0Y1".to_string(),
                tile: "CLB_X0Y1".to_string(),
                x: 0,
                y: 1,
            },
            crate::place::PlacedUltraSlice {
                packed_idx: 0,
                site: "SLICE_X0Y2".to_string(),
                tile: "CLB_X0Y2".to_string(),
                x: 0,
                y: 2,
            },
        ];
        let violations = check_legality(&vec![s0, s1], &placed, &grid, &limits);
        assert!(
            violations
                .iter()
                .any(|v| matches!(v, LegalityViolation::CarryChainBroken { .. }))
        );
    }
}
