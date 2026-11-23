//! UltraScale+ placer: packed slices -> placed sites on xcvu3p.
//!
//! Adapts Helion's placement approach to the UltraScale CLB grid.
//! Key requirements:
//! - Deterministic seeding: the multiseed agent depends on seed control.
//!   Uses SplitMix64 + Fisher-Yates, same pattern as helion-place.
//! - CARRY8 chains must occupy vertically contiguous slices (X constant,
//!   Y ascending) for the dedicated carry cascade.
//! - SLICEM-only cells (dist RAM, SRL) must land on Memory-kind slices.
//!
//! This module is designed against the INTERFACES.md `UltraScaleFabric`
//! trait shape. It will integrate into the `helion-ultrascale` crate when
//! the device-model agent creates it.

use crate::pack::{PackedUltraSlice, SliceKind};
use std::collections::{HashMap, HashSet};

/// SplitMix64: deterministic PRNG for seeded placement.
/// Same implementation as helion-place (no new dependencies).
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Deterministic Fisher-Yates shuffle of `0..n` driven by `seed`.
/// Seed 0 = identity order (legacy behavior).
pub fn seeded_order(n: usize, seed: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    if seed == 0 {
        return order;
    }
    let mut state = seed;
    for i in (1..n).rev() {
        let j = (splitmix64(&mut state) % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    order
}

/// Placement options.
#[derive(Clone, Debug)]
pub struct UltraPlaceOpts {
    /// Deterministic perturbation seed. 0 = legacy order.
    /// Non-zero = seeded shuffle of slice processing order.
    pub seed: u64,
    /// Weight for timing-driven placement (0 = wirelength only).
    pub timing_weight: f64,
}

impl Default for UltraPlaceOpts {
    fn default() -> Self {
        Self {
            seed: 0,
            timing_weight: 0.0,
        }
    }
}

/// A placed slice: packed slice assigned to a device site.
#[derive(Clone, Debug)]
pub struct PlacedUltraSlice {
    /// Index into the packed slice list.
    pub packed_idx: usize,
    /// Device site name (e.g., "SLICE_X10Y20").
    pub site: String,
    /// Tile name (e.g., "CLBLL_L_X10Y20").
    pub tile: String,
    /// Grid coordinates for distance estimation.
    pub x: i32,
    pub y: i32,
}

/// Result of UltraScale placement.
#[derive(Clone, Debug, Default)]
pub struct UltraPlaced {
    pub slices: Vec<PlacedUltraSlice>,
    /// Seed used (for reproducibility records).
    pub seed: u64,
}

/// Minimal device grid query interface.
/// The device-model agent's `UltraScaleFabric` will provide the real
/// implementation. This trait captures what the placer needs.
pub trait UltraGrid {
    /// All slice sites in deterministic order.
    /// Returns (site_name, tile_name, x, y, kind).
    fn slice_sites(&self) -> Vec<(String, String, i32, i32, SliceKind)>;
    /// Grid dimensions.
    fn grid_size(&self) -> (i32, i32);
}

/// Places packed slices onto the device grid.
///
/// - CARRY8 chains are placed first, in vertically contiguous slices.
/// - Remaining slices are placed in seeded order.
/// - SLICEM-required slices only go to Memory-kind sites.
pub fn place_ultra(
    packed: &[PackedUltraSlice],
    grid: &dyn UltraGrid,
    opts: &UltraPlaceOpts,
) -> Result<UltraPlaced, String> {
    let mut placed = UltraPlaced {
        seed: opts.seed,
        ..Default::default()
    };
    let mut used_sites: HashSet<String> = HashSet::new();
    let sites = grid.slice_sites();

    // Index sites by kind for fast lookup.
    let mut logic_sites: Vec<(String, String, i32, i32)> = Vec::new();
    let mut memory_sites: Vec<(String, String, i32, i32)> = Vec::new();
    for (site, tile, x, y, kind) in &sites {
        match kind {
            SliceKind::Logic => logic_sites.push((site.clone(), tile.clone(), *x, *y)),
            SliceKind::Memory => memory_sites.push((site.clone(), tile.clone(), *x, *y)),
        }
    }

    // Phase 1: Place CARRY8 chains in vertically contiguous slices.
    // Build chain groups from carry_prev/carry_next links.
    let mut chain_heads: Vec<usize> = Vec::new();
    let mut in_chain = HashSet::new();
    for (idx, slice) in packed.iter().enumerate() {
        if slice.carry8.is_some() && slice.carry_prev.is_none() {
            chain_heads.push(idx);
        }
        if slice.carry8.is_some() {
            in_chain.insert(idx);
        }
    }

    // For each chain, find a vertical column of free slices.
    for &head_idx in &chain_heads {
        // Collect the full chain.
        let mut chain: Vec<usize> = vec![head_idx];
        let mut cur = head_idx;
        while let Some(ref next_name) = packed[cur].carry_next {
            let mut found = None;
            for (idx, s) in packed.iter().enumerate() {
                if s.carry8.as_ref() == Some(next_name) {
                    found = Some(idx);
                    break;
                }
            }
            if let Some(nidx) = found {
                chain.push(nidx);
                cur = nidx;
            } else {
                break;
            }
        }

        // Find vertical run of `chain.len()` free slices at constant X.
        let mut placed_chain = false;
        // Group logic sites by X coordinate.
        let mut by_x: HashMap<i32, Vec<(String, String, i32, i32)>> = HashMap::new();
        for s in &logic_sites {
            if !used_sites.contains(&s.0) {
                by_x.entry(s.2).or_default().push(s.clone());
            }
        }
        // HashMap iteration order is not deterministic; sort columns by
        // X so the same seed always picks the same column (D1). Without
        // this, one seed could place the same chain on different columns
        // across runs.
        let mut columns: Vec<(i32, Vec<(String, String, i32, i32)>)> = by_x.into_iter().collect();
        columns.sort_by_key(|(x, _)| *x);
        for (_x, mut col) in columns {
            col.sort_by_key(|s| s.3);
            for window in col.windows(chain.len()) {
                // Check Y contiguity.
                let mut contiguous = true;
                for w in 1..window.len() {
                    if window[w].3 != window[w - 1].3 + 1 {
                        contiguous = false;
                        break;
                    }
                }
                if contiguous {
                    for (i, &packed_idx) in chain.iter().enumerate() {
                        let (site, tile, x, y) = &window[i];
                        placed.slices.push(PlacedUltraSlice {
                            packed_idx,
                            site: site.clone(),
                            tile: tile.clone(),
                            x: *x,
                            y: *y,
                        });
                        used_sites.insert(site.clone());
                    }
                    placed_chain = true;
                    break;
                }
            }
            if placed_chain {
                break;
            }
        }
        if !placed_chain {
            return Err(format!(
                "cannot place CARRY8 chain of length {}: no vertical contiguous slices",
                chain.len()
            ));
        }
    }

    // Phase 2: Place remaining slices in seeded order, spread so
    // consecutive slices do not stack on adjacent sites. The old
    // code took the first free site in (Y, X) order, so every slice
    // landed at X=1, X=2, ... on Y=0; the two slices of a 2-slice
    // design flank the same INT tile, and the wire DB saw one tile.
    // A candidate now needs Manhattan distance at least 2 from
    // every already placed slice (chains from Phase 1 count), which
    // steps slices two logical columns apart: X=1, X=3, X=5, ...
    // on Y=0, and consecutive slices flank different INT tiles
    // (physical tilegrid: SLICE_X1Y0 and SLICE_X2Y0 both flank
    // INT_X1Y0, SLICE_X3Y0 flanks INT_X2Y0). When no candidate
    // satisfies the distance (a nearly full row), fall back to the
    // first free site so placement still succeeds. The first slice
    // is unaffected (no placed slice to measure against), so
    // single-slice designs keep their old site. Candidate order is
    // the grid's deterministic (Y, X) order in both passes, so the
    // choice is deterministic for a fixed seed.
    let remaining: Vec<usize> = (0..packed.len())
        .filter(|i| !in_chain.contains(i))
        .collect();
    let order = seeded_order(remaining.len(), opts.seed);

    for &order_idx in &order {
        let packed_idx = remaining[order_idx];
        let slice = &packed[packed_idx];
        let needs_memory =
            slice.kind == SliceKind::Memory || slice.lut_is_memory.values().any(|&b| b);

        let candidates = if needs_memory {
            &memory_sites
        } else {
            &logic_sites
        };

        let placed_positions: Vec<(i32, i32)> =
            placed.slices.iter().map(|p| (p.x, p.y)).collect();
        let mut chosen: Option<&(String, String, i32, i32)> = None;
        for cand in candidates {
            if used_sites.contains(&cand.0) {
                continue;
            }
            let spread_ok = placed_positions.iter().all(|(px, py)| {
                (cand.2 - px).abs() + (cand.3 - py).abs() >= 2
            });
            if spread_ok {
                chosen = Some(cand);
                break;
            }
        }
        if chosen.is_none() {
            for cand in candidates {
                if !used_sites.contains(&cand.0) {
                    chosen = Some(cand);
                    break;
                }
            }
        }
        let Some((site, tile, x, y)) = chosen else {
            return Err(format!(
                "cannot place slice {}: no free {} sites",
                slice.name,
                if needs_memory { "SLICEM" } else { "SLICE" }
            ));
        };
        placed.slices.push(PlacedUltraSlice {
            packed_idx,
            site: site.clone(),
            tile: tile.clone(),
            x: *x,
            y: *y,
        });
        used_sites.insert(site.clone());
    }

    Ok(placed)
}

/// Render a deterministic placed-netlist text for the router handoff
/// (GAP-F4, US-09). One line per BEL assignment, slices sorted by site,
/// BELs sorted by index. BELs are numeric (`LUT0`, `FF3`) to avoid
/// asserting a letter mapping this renderer does not own; the UG574
/// letter names live in bel.rs. A placed entry naming a packed index
/// that does not exist is rendered as `UNPLACED_INDEX`, never dropped
/// silently. The caller is expected to run `check_legality` first; this
/// renderer stays total so a bad placement is visible, not hidden.
pub fn render_placed_netlist(packed: &[PackedUltraSlice], placed: &[PlacedUltraSlice]) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("# placed netlist: {} slices", placed.len()));
    let mut ordered: Vec<&PlacedUltraSlice> = placed.iter().collect();
    ordered.sort_by_key(|p| p.site.clone());
    for p in ordered {
        let Some(slice) = packed.get(p.packed_idx) else {
            lines.push(format!("{} UNPLACED_INDEX {}", p.site, p.packed_idx));
            continue;
        };
        let mut luts: Vec<(&u8, &String)> = slice.luts.iter().collect();
        luts.sort_by_key(|(bel, _)| **bel);
        for (bel, cell) in luts {
            let init = slice.lut_inits.get(bel).copied().unwrap_or(0);
            let mem = slice.lut_is_memory.get(bel).copied().unwrap_or(false);
            lines.push(format!(
                "{} LUT{bel} {cell} INIT=0x{init:016X}{}",
                p.site,
                if mem { " MEM" } else { "" }
            ));
        }
        let mut ffs: Vec<(&u8, &String)> = slice.ffs.iter().collect();
        ffs.sort_by_key(|(bel, _)| **bel);
        for (bel, cell) in ffs {
            let clk = slice.ff_clk.get(bel).map(String::as_str).unwrap_or("?");
            lines.push(format!("{} FF{bel} {cell} CLK={clk}", p.site));
        }
        if let Some(carry) = &slice.carry8 {
            let mut line = format!("{} CARRY8 {carry}", p.site);
            if let Some(prev) = &slice.carry_prev {
                line.push_str(&format!(" PREV={prev}"));
            }
            if let Some(next) = &slice.carry_next {
                line.push_str(&format!(" NEXT={next}"));
            }
            lines.push(line);
        }
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{PackedUltraSlice, SliceKind};

    /// Fake grid for testing.
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
    fn seed_determinism() {
        // Same seed must produce same order.
        let o1 = seeded_order(100, 42);
        let o2 = seeded_order(100, 42);
        assert_eq!(o1, o2);
        // Different seeds produce different orders (with high probability).
        let o3 = seeded_order(100, 43);
        assert_ne!(o1, o3);
        // Seed 0 is identity.
        let o0 = seeded_order(10, 0);
        assert_eq!(o0, (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn place_simple_slices() {
        let grid = test_grid();
        let mut s0 = PackedUltraSlice::new("s0".to_string(), SliceKind::Logic);
        s0.luts.insert(0, "lut0".to_string());
        let mut s1 = PackedUltraSlice::new("s1".to_string(), SliceKind::Memory);
        s1.luts.insert(0, "lut1".to_string());
        s1.lut_is_memory.insert(0, true);

        let packed = vec![s0, s1];
        let opts = UltraPlaceOpts::default();
        let placed = place_ultra(&packed, &grid, &opts).unwrap();
        assert_eq!(placed.slices.len(), 2);
        // Memory slice must be on a Memory site.
        let mem_placed = placed.slices.iter().find(|p| p.packed_idx == 1).unwrap();
        let site_kind = grid
            .slice_sites()
            .iter()
            .find(|(s, _, _, _, _)| s == &mem_placed.site)
            .map(|(_, _, _, _, k)| *k)
            .unwrap();
        assert_eq!(site_kind, SliceKind::Memory);
    }

    #[test]
    fn consecutive_slices_spread_two_columns_apart() {
        // Two logic slices must not land on adjacent sites: the old
        // first-free placer put them at X=0 and X=1 (distance 1),
        // which in the real tilegrid flanks one INT tile. The spread
        // placer keeps Manhattan distance at least 2, so the second
        // slice lands at least two columns away, flanking a
        // different INT tile. The first slice keeps the old site.
        let grid = test_grid();
        let mut s0 = PackedUltraSlice::new("s0".to_string(), SliceKind::Logic);
        s0.luts.insert(0, "lut0".to_string());
        let mut s1 = PackedUltraSlice::new("s1".to_string(), SliceKind::Logic);
        s1.luts.insert(0, "lut1".to_string());
        let packed = vec![s0, s1];
        let placed = place_ultra(&packed, &grid, &UltraPlaceOpts::default()).unwrap();
        assert_eq!(placed.slices.len(), 2);
        let p0 = placed.slices.iter().find(|p| p.packed_idx == 0).unwrap();
        let p1 = placed.slices.iter().find(|p| p.packed_idx == 1).unwrap();
        assert_eq!((p0.x, p0.y), (1, 0));
        assert!((p0.x - p1.x).abs() + (p0.y - p1.y).abs() >= 2);
        assert_eq!((p1.x, p1.y), (3, 0));
    }

    #[test]
    fn carry_chain_vertical() {
        let grid = test_grid();
        let mut s0 = PackedUltraSlice::new("s0".to_string(), SliceKind::Logic);
        s0.carry8 = Some("carry0".to_string());
        s0.carry_next = Some("carry1".to_string());
        let mut s1 = PackedUltraSlice::new("s1".to_string(), SliceKind::Logic);
        s1.carry8 = Some("carry1".to_string());
        s1.carry_prev = Some("carry0".to_string());

        let packed = vec![s0, s1];
        let opts = UltraPlaceOpts::default();
        let placed = place_ultra(&packed, &grid, &opts).unwrap();
        assert_eq!(placed.slices.len(), 2);
        // Check vertical contiguity: same X, Y differs by 1.
        let p0 = placed.slices.iter().find(|p| p.packed_idx == 0).unwrap();
        let p1 = placed.slices.iter().find(|p| p.packed_idx == 1).unwrap();
        assert_eq!(p0.x, p1.x);
        assert!((p0.y - p1.y).abs() == 1);
    }

    #[test]
    fn carry_chain_placement_is_deterministic() {
        // D1: before the fix, chain column choice iterated a HashMap, so
        // repeated placements of the same chain could land on different
        // columns. Each HashMap gets a fresh random seed, so ten runs
        // make a pre-fix failure near-certain and a post-fix pass
        // meaningful.
        let grid = test_grid();
        let mut s0 = PackedUltraSlice::new("s0".to_string(), SliceKind::Logic);
        s0.carry8 = Some("carry0".to_string());
        s0.carry_next = Some("carry1".to_string());
        let mut s1 = PackedUltraSlice::new("s1".to_string(), SliceKind::Logic);
        s1.carry8 = Some("carry1".to_string());
        s1.carry_prev = Some("carry0".to_string());
        s1.carry_next = Some("carry2".to_string());
        let mut s2 = PackedUltraSlice::new("s2".to_string(), SliceKind::Logic);
        s2.carry8 = Some("carry2".to_string());
        s2.carry_prev = Some("carry1".to_string());

        let packed = vec![s0, s1, s2];
        let opts = UltraPlaceOpts::default();
        let first = place_ultra(&packed, &grid, &opts).unwrap();
        let sites = |p: &UltraPlaced| {
            p.slices
                .iter()
                .map(|s| (s.packed_idx, s.site.clone()))
                .collect::<Vec<_>>()
        };
        let expected = sites(&first);
        for _ in 0..9 {
            let again = place_ultra(&packed, &grid, &opts).unwrap();
            assert_eq!(sites(&again), expected);
        }
    }

    #[test]
    fn render_placed_netlist_lists_bels_by_site() {
        let grid = test_grid();
        let mut s0 = PackedUltraSlice::new("s0".to_string(), SliceKind::Logic);
        s0.luts.insert(0, "lut0".to_string());
        s0.lut_inits.insert(0, 0xAAAAAAAAAAAAAAAA);
        s0.lut_is_memory.insert(0, false);
        s0.ffs.insert(0, "ff0".to_string());
        s0.ff_clk.insert(0, "clk".to_string());
        s0.ff_ce.insert(0, None);
        s0.ff_sr.insert(0, None);
        s0.carry8 = Some("carry0".to_string());

        let packed = vec![s0];
        let placed = place_ultra(&packed, &grid, &UltraPlaceOpts::default()).unwrap();
        let text = render_placed_netlist(&packed, &placed.slices);
        let site = &placed.slices[0].site;
        assert!(text.contains(&format!("{site} LUT0 lut0 INIT=0xAAAAAAAAAAAAAAAA")));
        assert!(text.contains(&format!("{site} FF0 ff0 CLK=clk")));
        assert!(text.contains(&format!("{site} CARRY8 carry0")));
        // A placed entry with a bogus packed index is shown, not dropped.
        let bogus = vec![PlacedUltraSlice {
            packed_idx: 99,
            site: "SLICE_X9Y9".to_string(),
            tile: "CLB_X9Y9".to_string(),
            x: 9,
            y: 9,
        }];
        let text = render_placed_netlist(&packed, &bogus);
        assert!(text.contains("SLICE_X9Y9 UNPLACED_INDEX 99"));
    }
}
