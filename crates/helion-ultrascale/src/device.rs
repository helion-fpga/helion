//! xcvu3p device definition.
//!
//! Aggregate resource counts from AMD XMP103 (UltraScale+ Product Selection
//! Guide, v2.8): VU3P column.
//! - CLB LUTs: 394K
//! - CLB Flip-Flops: 788K
//! - DSP Slices: 2,280
//! - Total Block RAM: 25.3 Mb
//! - UltraRAM: 90.0 Mb
//! - Max Single-Ended HP I/Os: 520
//!
//! Derived counts (shown with derivation, not AMD-published directly):
//! - RAMB36E2: 25.3 Mb * 1024 Kb/Mb / 36 Kb = 719.6, rounds to 720
//! - URAM288: 90.0 Mb * 1024 Kb/Mb / 288 Kb = 320 exactly
//! - CLB slices: 394,000 LUTs / 8 per slice = 49,250
//!
//! CLB structure from UG574. Tile type vocabulary from BitFiltrator
//! tileType_siteType_pairs (CLEL_L/R, CLEM/R, DSP, BRAM).

use crate::bel::{Bel, BelKind, ClbBelInventory, TileId, TileKind};
use std::collections::HashMap;

/// xcvu3p aggregate resources. Values from AMD XMP103 unless marked derived.
pub struct Xcvu3pSpec;

impl Xcvu3pSpec {
    /// Part name.
    pub const PART: &'static str = "xcvu3p";
    /// CLB LUTs, K (XMP103: 394).
    pub const CLB_LUTS_K: u32 = 394;
    /// CLB flip-flops, K (XMP103: 788).
    pub const CLB_FFS_K: u32 = 788;
    /// DSP slices (XMP103: 2,280).
    pub const DSP_SLICES: u32 = 2280;
    /// Block RAM total, Mb (XMP103: 25.3).
    pub const BRAM_MB: f64 = 25.3;
    /// UltraRAM total, Mb (XMP103: 90.0).
    pub const URAM_MB: f64 = 90.0;
    /// Max single-ended HP I/Os (XMP103: 520).
    pub const HP_IOS: u32 = 520;

    /// RAMB36E2 count, derived: 25.3 * 1024 / 36 = 719.6 -> 720.
    pub const RAMB36_COUNT: u32 = 720;
    /// URAM288 count, derived: 90.0 * 1024 / 288 = 320 exactly.
    pub const URAM288_COUNT: u32 = 320;
    /// CLB slice count, derived: 394_000 / 8 = 49,250.
    pub const CLB_SLICE_COUNT: u32 = 49_250;
}

/// A Virtex UltraScale+ device: tile grid plus resource inventory.
///
/// The grid is logical. X and Y dimensions are derived from the CLB slice
/// count assuming a columnar layout; they do NOT reproduce Vivado's physical
/// grid, which is not published in open sources. The BEL structure within
/// each tile is exact per UG574.
pub struct UltraScaleDevice {
    pub part: String,
    grid_w: u32,
    grid_h: u32,
    tiles: HashMap<TileId, TileKind>,
    /// BELs per tile, populated at construction.
    bel_cache: HashMap<TileId, Vec<Bel>>,
}

impl UltraScaleDevice {
    /// Build the xcvu3p model.
    pub fn xcvu3p() -> Self {
        Self::build_xcvu3p()
    }

    fn build_xcvu3p() -> Self {
        // Grid derivation (documented, not AMD-published):
        // - 49,250 CLB slices (394K LUTs / 8).
        // - 2,280 DSP48E2 (XMP103).
        // - 720 RAMB36E2 (derived from 25.3 Mb).
        // - 320 URAM288 (derived from 90.0 Mb).
        // - Total functional tiles: 52,570.
        // - Grid 144 x 400 = 57,600 slots. Column 0 is clocking backbone.
        //   Hard-block columns interleave every 12 columns starting at x=12,
        //   a common UltraScale floorplan pattern (exact Vivado layout not open).
        //   Remaining slots after exact-count placement become I/O tiles.
        let grid_w = 144;
        let grid_h = 400;

        let mut tiles: HashMap<TileId, TileKind> = HashMap::new();
        // Column 0: clocking.
        for y in 0..grid_h {
            tiles.insert(TileId::new(0, y), TileKind::Clk);
        }

        // Hard-block columns: x = 12, 24, ..., 132 (11 columns).
        // Allocate DSP/BRAM/URAM counts across them in order.
        let mut dsp_left = Xcvu3pSpec::DSP_SLICES;
        let mut bram_left = Xcvu3pSpec::RAMB36_COUNT;
        let mut uram_left = Xcvu3pSpec::URAM288_COUNT;
        let hb_cols: Vec<u32> = (1..12).map(|i| i * 12).collect();
        for &x in &hb_cols {
            for y in 0..grid_h {
                let id = TileId::new(x, y);
                let kind = if dsp_left > 0 {
                    dsp_left -= 1;
                    TileKind::Dsp
                } else if bram_left > 0 {
                    bram_left -= 1;
                    TileKind::Bram
                } else if uram_left > 0 {
                    uram_left -= 1;
                    TileKind::Uram
                } else {
                    TileKind::Io
                };
                tiles.insert(id, kind);
            }
        }
        assert_eq!(dsp_left, 0, "DSP placement must consume all 2280");
        assert_eq!(bram_left, 0, "BRAM placement must consume all 720");
        assert_eq!(uram_left, 0, "URAM placement must consume all 320");

        // CLB tiles: fill remaining slots up to exactly 49,250.
        let mut clb_left = Xcvu3pSpec::CLB_SLICE_COUNT;
        for y in 0..grid_h {
            for x in 0..grid_w {
                if clb_left == 0 {
                    break;
                }
                let id = TileId::new(x, y);
                if tiles.contains_key(&id) {
                    continue;
                }
                // Alternate SLICEL/SLICEM ~2:1. Exact ratio device-specific,
                // not open; documented as model choice.
                let kind = if (x + y) % 3 == 0 {
                    TileKind::Clem
                } else {
                    TileKind::Clel
                };
                tiles.insert(id, kind);
                clb_left -= 1;
            }
            if clb_left == 0 {
                break;
            }
        }
        assert_eq!(clb_left, 0, "CLB placement must consume all 49250");

        // Remaining empty slots become I/O tiles.
        for y in 0..grid_h {
            for x in 0..grid_w {
                let id = TileId::new(x, y);
                tiles.entry(id).or_insert(TileKind::Io);
            }
        }

        // Populate BEL cache.
        let mut bel_cache = HashMap::new();
        for (id, kind) in &tiles {
            let bels = match kind {
                TileKind::Clel | TileKind::Clem => ClbBelInventory::bels_at(*id),
                TileKind::Dsp => vec![Bel::new(BelKind::Dsp48e2, *id, 0)],
                TileKind::Bram => vec![
                    // BRAM capacity accounting (CTO ruling G3): one tile is
                    // ONE RAMB36E2 or TWO RAMB18E2 halves, never both. The
                    // two Ramb18e2 BELs are alternate views of the same
                    // physical halves, not additional capacity: 720 tiles
                    // hold 720 RAMB36E2 or 1440 RAMB18E2 halves, and the
                    // counts must never be added together. A design uses a
                    // tile as one RAMB36 or as two RAMB18, never both.
                    Bel::new(BelKind::Ramb36e2, *id, 0),
                    Bel::new(BelKind::Ramb18e2, *id, 0),
                    Bel::new(BelKind::Ramb18e2, *id, 1),
                ],
                TileKind::Uram => vec![Bel::new(BelKind::Uram288, *id, 0)],
                _ => vec![],
            };
            bel_cache.insert(*id, bels);
        }

        Self {
            part: Xcvu3pSpec::PART.to_string(),
            grid_w,
            grid_h,
            tiles,
            bel_cache,
        }
    }

    /// Grid width in tiles.
    pub fn grid_width(&self) -> u32 {
        self.grid_w
    }

    /// Grid height in tiles.
    pub fn grid_height(&self) -> u32 {
        self.grid_h
    }

    /// Tile kind at the given coordinates, or None if out of bounds.
    pub fn tile_kind(&self, tile: TileId) -> Option<TileKind> {
        self.tiles.get(&tile).copied()
    }

    /// BELs at the given tile. Empty slice if out of bounds.
    pub fn bels_at(&self, tile: TileId) -> &[Bel] {
        self.bel_cache.get(&tile).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Iterate over all tiles.
    pub fn tiles(&self) -> impl Iterator<Item = (TileId, TileKind)> + '_ {
        self.tiles.iter().map(|(k, v)| (*k, *v))
    }

    /// Count tiles of a given kind.
    pub fn count_tiles(&self, kind: TileKind) -> usize {
        self.tiles.values().filter(|k| **k == kind).count()
    }

    /// Count BELs of a given kind across the device.
    pub fn count_bels(&self, kind: BelKind) -> usize {
        self.bel_cache
            .values()
            .flat_map(|v| v.iter())
            .filter(|b| b.kind == kind)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xcvu3p_part_name() {
        let d = UltraScaleDevice::xcvu3p();
        assert_eq!(d.part, "xcvu3p");
    }

    #[test]
    fn xcvu3p_clb_counts_match_amd_docs() {
        // XMP103: 394K LUTs, 788K FFs. Model: 49,250 slices x (8 LUT + 16 FF).
        let d = UltraScaleDevice::xcvu3p();
        let luts = d.count_bels(BelKind::Lut6);
        let ffs = d.count_bels(BelKind::FdRe);
        assert_eq!(luts, 49_250 * 8, "LUT count must match 394K from XMP103");
        assert_eq!(ffs, 49_250 * 16, "FF count must match 788K from XMP103");
        // Within 1% of published K numbers (394K, 788K).
        assert!((luts as f64 - 394_000.0).abs() / 394_000.0 < 0.01);
        assert!((ffs as f64 - 788_000.0).abs() / 788_000.0 < 0.01);
    }

    #[test]
    fn xcvu3p_carry8_one_per_slice() {
        // UG574: one CARRY8 per CLB slice.
        let d = UltraScaleDevice::xcvu3p();
        assert_eq!(d.count_bels(BelKind::Carry8), 49_250);
    }

    #[test]
    fn every_tile_bels_resolve() {
        // Trait coherence: every tile's BELs resolve, no dangling references.
        let d = UltraScaleDevice::xcvu3p();
        for (id, kind) in d.tiles() {
            let bels = d.bels_at(id);
            for b in bels {
                assert_eq!(b.tile, id, "BEL tile must match query tile");
                assert!(
                    b.name().is_some(),
                    "BEL {:?} at {:?} must have a valid name",
                    b.kind,
                    id
                );
            }
            // CLB tiles must have the full UG574 complement.
            match kind {
                TileKind::Clel | TileKind::Clem => {
                    assert!(!bels.is_empty(), "CLB tile {:?} has no BELs", id)
                }
                _ => {}
            }
        }
    }

    #[test]
    fn xcvu3p_hard_block_counts_match_amd_docs() {
        // XMP103: 2,280 DSP slices. Derived: 720 RAMB36E2 (25.3 Mb),
        // 320 URAM288 (90.0 Mb).
        let d = UltraScaleDevice::xcvu3p();
        assert_eq!(
            d.count_bels(BelKind::Dsp48e2),
            2280,
            "DSP count must match XMP103"
        );
        assert_eq!(
            d.count_bels(BelKind::Ramb36e2),
            720,
            "RAMB36 count derived from 25.3 Mb"
        );
        assert_eq!(
            d.count_bels(BelKind::Uram288),
            320,
            "URAM288 count derived from 90.0 Mb"
        );
    }

    #[test]
    fn bram_capacity_is_720_ramb36_with_no_double_count() {
        // G3: BRAM tile capacity is 720 RAMB36E2 total (derived from
        // 25.3 Mb). The two RAMB18E2 BELs per tile are alternate views
        // of the same physical halves, not additional capacity: a tile
        // used as one RAMB36 cannot also be used as two RAMB18, so the
        // counts must never be added (720 + 1440 would double count).
        let d = UltraScaleDevice::xcvu3p();
        assert_eq!(d.count_tiles(TileKind::Bram), 720, "BRAM tile count");
        assert_eq!(
            d.count_bels(BelKind::Ramb36e2),
            720,
            "RAMB36E2 count is the tile count"
        );
        assert_eq!(
            d.count_bels(BelKind::Ramb18e2),
            1440,
            "RAMB18E2 halves are exactly two per tile"
        );
        assert_eq!(
            d.count_bels(BelKind::Ramb18e2),
            2 * d.count_bels(BelKind::Ramb36e2),
            "halves are the same tiles viewed as halves, not extra capacity"
        );
        // Per tile: exactly one RAMB36 view and two RAMB18 views.
        for (id, kind) in d.tiles() {
            if kind != TileKind::Bram {
                continue;
            }
            let bels = d.bels_at(id);
            assert_eq!(
                bels.iter().filter(|b| b.kind == BelKind::Ramb36e2).count(),
                1,
                "BRAM tile {id:?} must have exactly one RAMB36 view"
            );
            assert_eq!(
                bels.iter().filter(|b| b.kind == BelKind::Ramb18e2).count(),
                2,
                "BRAM tile {id:?} must have exactly two RAMB18 views"
            );
        }
    }

    #[test]
    fn tile_kind_roundtrip() {
        let d = UltraScaleDevice::xcvu3p();
        let id = TileId::new(1, 0);
        assert!(d.tile_kind(id).is_some());
        assert!(d.tile_kind(TileId::new(9999, 9999)).is_none());
    }
}
