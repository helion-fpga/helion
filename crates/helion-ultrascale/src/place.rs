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
        for (_x, mut col) in by_x {
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

    // Phase 2: Place remaining slices in seeded order.
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

        let mut placed_slice = false;
        for (site, tile, x, y) in candidates {
            if !used_sites.contains(site) {
                placed.slices.push(PlacedUltraSlice {
                    packed_idx,
                    site: site.clone(),
                    tile: tile.clone(),
                    x: *x,
                    y: *y,
                });
                used_sites.insert(site.clone());
                placed_slice = true;
                break;
            }
        }
        if !placed_slice {
            return Err(format!(
                "cannot place slice {}: no free {} sites",
                slice.name,
                if needs_memory { "SLICEM" } else { "SLICE" }
            ));
        }
    }

    Ok(placed)
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
}
