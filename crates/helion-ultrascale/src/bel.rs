//! Basic Element (BEL) definitions for UltraScale+.
//!
//! Source: UG574 (UltraScale Architecture Configurable Logic Block), CLB
//! Resources section. Every CLB contains ONE slice with eight 6-input LUTs and
//! sixteen storage elements. The LUTs are labeled A through H, storage
//! elements AQ/AQ2 through HQ/HQ2. One CARRY8 per slice (8-bit carry chain,
//! or DUAL_CY4 mode for two independent 4-bit chains per UG974).

use std::fmt;

/// Identifies a tile in the device grid. Coordinates are logical; the
/// physical Vivado grid is not published in open sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId {
    pub x: u32,
    pub y: u32,
}

impl TileId {
    pub fn new(x: u32, y: u32) -> Self {
        Self { x, y }
    }
}

impl fmt::Display for TileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TILE_X{}Y{}", self.x, self.y)
    }
}

/// Tile type. Tile type names follow the BitFiltrator tileType_siteType_pairs
/// vocabulary (CLEL_L, CLEM, DSP, BRAM) which matches Vivado tile naming.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TileKind {
    /// CLEL_L / CLEL_R: CLB with SLICEL (logic only).
    Clel,
    /// CLEM / CLEM_R: CLB with SLICEM (LUTs usable as distributed RAM/SRL).
    Clem,
    /// DSP tile: DSP48E2 sites.
    Dsp,
    /// BRAM tile: RAMB36E2 / RAMB18E2 sites.
    Bram,
    /// URAM tile: URAM288 sites.
    Uram,
    /// I/O tile.
    Io,
    /// Clocking tile (BUFGCE, MMCM, PLL).
    Clk,
}

/// Primitive BEL kind. Names match UNISIM primitives (UG974).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BelKind {
    /// 6-input LUT. 8 per CLB slice, labeled A-H (UG574).
    Lut6,
    /// Flip-flop with clock enable and sync/async reset. 16 per CLB slice:
    /// AQ, AQ2, BQ, BQ2, ..., HQ, HQ2 (UG574).
    FdRe,
    /// 8-bit fast carry chain. 1 per CLB slice (UG574). Supports DUAL_CY4
    /// mode via CARRY_TYPE attribute (UG974).
    Carry8,
    /// Wide-function multiplexer combining adjacent LUTs (UG574).
    F7Mux,
    /// Wide-function multiplexer combining F7MUXs (UG574).
    F8Mux,
    /// Wide-function multiplexer combining F8MUXs (UG574).
    F9Mux,
    /// DSP slice (UG578).
    Dsp48e2,
    /// 36Kb block RAM (UG573).
    Ramb36e2,
    /// 18Kb block RAM (half of RAMB36E2, UG573).
    Ramb18e2,
    /// 288Kb UltraRAM (UG573).
    Uram288,
}

impl BelKind {
    /// Human-readable UNISIM name.
    pub fn unisim(&self) -> &'static str {
        match self {
            BelKind::Lut6 => "LUT6",
            BelKind::FdRe => "FDRE",
            BelKind::Carry8 => "CARRY8",
            BelKind::F7Mux => "F7MUX",
            BelKind::F8Mux => "F8MUX",
            BelKind::F9Mux => "F9MUX",
            BelKind::Dsp48e2 => "DSP48E2",
            BelKind::Ramb36e2 => "RAMB36E2",
            BelKind::Ramb18e2 => "RAMB18E2",
            BelKind::Uram288 => "URAM288",
        }
    }
}

/// A placed primitive: kind + location + index within the tile.
///
/// For CLB BELs, index encodes the UG574 letter designation: LUTs A-H map to
/// 0-7, FFs AQ/AQ2/BQ/BQ2/.../HQ/HQ2 map to 0-15, CARRY8 is index 0.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Bel {
    pub kind: BelKind,
    pub tile: TileId,
    pub index: u32,
}

impl Bel {
    pub fn new(kind: BelKind, tile: TileId, index: u32) -> Self {
        Self { kind, tile, index }
    }

    /// BEL name in UG574 letter convention, e.g. "A6LUT", "AQ", "CARRY8".
    /// Returns None for indices outside the valid range for the kind.
    ///
    /// Names are injective within a tile: two different BELs never share a
    /// name. Wide muxes use the UG574 lettered names (F7AMUX..F7DMUX,
    /// F8AMUX, F8BMUX). The two RAMB18 halves of a BRAM tile are
    /// RAMB18E2_L (Bel index 0, lower half) and RAMB18E2_U (Bel index 1,
    /// upper half), following the Project U-Ray L/U naming; the index to
    /// half mapping is a convention of this model, not a Vivado-verified
    /// fact. Any consumer that keys on name (including the FASM handoff)
    /// can therefore tell the halves apart.
    pub fn name(&self) -> Option<String> {
        match self.kind {
            BelKind::Lut6 => {
                if self.index < 8 {
                    Some(format!("{}6LUT", (b'A' + self.index as u8) as char))
                } else {
                    None
                }
            }
            BelKind::FdRe => {
                if self.index < 16 {
                    let letter = (b'A' + (self.index / 2) as u8) as char;
                    let suffix = if self.index % 2 == 0 { "Q" } else { "Q2" };
                    Some(format!("{letter}{suffix}"))
                } else {
                    None
                }
            }
            BelKind::Carry8 => {
                if self.index == 0 {
                    Some("CARRY8".to_string())
                } else {
                    None
                }
            }
            BelKind::F7Mux => {
                if self.index < 4 {
                    Some(format!("F7{}MUX", (b'A' + self.index as u8) as char))
                } else {
                    None
                }
            }
            BelKind::F8Mux => {
                if self.index < 2 {
                    Some(format!("F8{}MUX", (b'A' + self.index as u8) as char))
                } else {
                    None
                }
            }
            BelKind::F9Mux => {
                if self.index == 0 {
                    Some("F9MUX".to_string())
                } else {
                    None
                }
            }
            BelKind::Ramb18e2 => match self.index {
                0 => Some("RAMB18E2_L".to_string()),
                1 => Some("RAMB18E2_U".to_string()),
                _ => None,
            },
            BelKind::Dsp48e2 | BelKind::Ramb36e2 | BelKind::Uram288 => {
                if self.index == 0 {
                    Some(self.kind.unisim().to_string())
                } else {
                    None
                }
            }
        }
    }
}

/// BEL inventory for one CLB slice. Source: UG574 Table 1, "Logic Resources
/// in One CLB Slice".
pub struct ClbBelInventory;

impl ClbBelInventory {
    /// LUTs per CLB slice (UG574: 8).
    pub const LUTS_PER_SLICE: u32 = 8;
    /// Flip-flops per CLB slice (UG574: 16).
    pub const FFS_PER_SLICE: u32 = 16;
    /// CARRY8 per CLB slice (UG574: 1 eight-bit chain).
    pub const CARRY8_PER_SLICE: u32 = 1;
    /// Wide muxes per slice: 4x F7, 2x F8, 1x F9 (UG574).
    pub const F7_PER_SLICE: u32 = 4;
    pub const F8_PER_SLICE: u32 = 2;
    pub const F9_PER_SLICE: u32 = 1;

    /// All BELs in a CLB tile at the given TileId.
    pub fn bels_at(tile: TileId) -> Vec<Bel> {
        let mut bels = Vec::new();
        for i in 0..Self::LUTS_PER_SLICE {
            bels.push(Bel::new(BelKind::Lut6, tile, i));
        }
        for i in 0..Self::FFS_PER_SLICE {
            bels.push(Bel::new(BelKind::FdRe, tile, i));
        }
        for i in 0..Self::CARRY8_PER_SLICE {
            bels.push(Bel::new(BelKind::Carry8, tile, i));
        }
        for i in 0..Self::F7_PER_SLICE {
            bels.push(Bel::new(BelKind::F7Mux, tile, i));
        }
        for i in 0..Self::F8_PER_SLICE {
            bels.push(Bel::new(BelKind::F8Mux, tile, i));
        }
        for i in 0..Self::F9_PER_SLICE {
            bels.push(Bel::new(BelKind::F9Mux, tile, i));
        }
        bels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clb_slice_has_ug574_resource_counts() {
        // UG574 Table 1: 8 LUTs, 16 FFs, 1 carry chain, F7/F8/F9 muxes.
        assert_eq!(ClbBelInventory::LUTS_PER_SLICE, 8);
        assert_eq!(ClbBelInventory::FFS_PER_SLICE, 16);
        assert_eq!(ClbBelInventory::CARRY8_PER_SLICE, 1);
        let bels = ClbBelInventory::bels_at(TileId::new(0, 0));
        let luts = bels.iter().filter(|b| b.kind == BelKind::Lut6).count();
        let ffs = bels.iter().filter(|b| b.kind == BelKind::FdRe).count();
        let carry = bels.iter().filter(|b| b.kind == BelKind::Carry8).count();
        assert_eq!(luts, 8);
        assert_eq!(ffs, 16);
        assert_eq!(carry, 1);
    }

    #[test]
    fn bel_names_are_injective_within_a_tile() {
        // Critic F1 (base review): Bel::name() must identify a BEL, so no
        // two BELs on one tile may share a name. Covers the wide muxes and
        // the two RAMB18 halves, the cases the old fallback arm aliased.
        let tile = TileId::new(0, 0);
        let mut names = std::collections::HashSet::new();
        for bel in ClbBelInventory::bels_at(tile) {
            let name = bel.name().expect("every CLB BEL must have a name");
            assert!(names.insert(name.clone()), "duplicate BEL name {name}");
        }
        // Wide mux lettering per UG574.
        assert_eq!(
            Bel::new(BelKind::F7Mux, tile, 0).name(),
            Some("F7AMUX".to_string())
        );
        assert_eq!(
            Bel::new(BelKind::F7Mux, tile, 3).name(),
            Some("F7DMUX".to_string())
        );
        assert_eq!(Bel::new(BelKind::F7Mux, tile, 4).name(), None);
        assert_eq!(
            Bel::new(BelKind::F8Mux, tile, 1).name(),
            Some("F8BMUX".to_string())
        );
        assert_eq!(Bel::new(BelKind::F8Mux, tile, 2).name(), None);
        assert_eq!(
            Bel::new(BelKind::F9Mux, tile, 0).name(),
            Some("F9MUX".to_string())
        );
        assert_eq!(Bel::new(BelKind::F9Mux, tile, 1).name(), None);
        // RAMB18 halves are distinct names.
        assert_eq!(
            Bel::new(BelKind::Ramb18e2, tile, 0).name(),
            Some("RAMB18E2_L".to_string())
        );
        assert_eq!(
            Bel::new(BelKind::Ramb18e2, tile, 1).name(),
            Some("RAMB18E2_U".to_string())
        );
        assert_eq!(Bel::new(BelKind::Ramb18e2, tile, 2).name(), None);
        // Single-instance hard blocks name only at index 0.
        assert_eq!(
            Bel::new(BelKind::Dsp48e2, tile, 0).name(),
            Some("DSP48E2".to_string())
        );
        assert_eq!(Bel::new(BelKind::Dsp48e2, tile, 1).name(), None);
    }

    #[test]
    fn bel_names_follow_ug574_letter_convention() {
        let tile = TileId::new(0, 0);
        // LUTs A-H
        assert_eq!(
            Bel::new(BelKind::Lut6, tile, 0).name(),
            Some("A6LUT".to_string())
        );
        assert_eq!(
            Bel::new(BelKind::Lut6, tile, 7).name(),
            Some("H6LUT".to_string())
        );
        assert_eq!(Bel::new(BelKind::Lut6, tile, 8).name(), None);
        // FFs AQ/AQ2 ... HQ/HQ2
        assert_eq!(
            Bel::new(BelKind::FdRe, tile, 0).name(),
            Some("AQ".to_string())
        );
        assert_eq!(
            Bel::new(BelKind::FdRe, tile, 1).name(),
            Some("AQ2".to_string())
        );
        assert_eq!(
            Bel::new(BelKind::FdRe, tile, 14).name(),
            Some("HQ".to_string())
        );
        assert_eq!(
            Bel::new(BelKind::FdRe, tile, 15).name(),
            Some("HQ2".to_string())
        );
        assert_eq!(Bel::new(BelKind::FdRe, tile, 16).name(), None);
        // CARRY8
        assert_eq!(
            Bel::new(BelKind::Carry8, tile, 0).name(),
            Some("CARRY8".to_string())
        );
    }
}
