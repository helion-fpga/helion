//! The `UltraScaleFabric` trait: the contract that pack-place, router, and
//! timing agents code against.
//!
//! `WireDb` and `TimingDb` are seams. The routing-graph agent owns the
//! `WireDb` internals; the timing agent owns the `TimingDb` internals. This
//! crate defines the trait shapes only.

use crate::bel::{Bel, TileId, TileKind};
use crate::device::UltraScaleDevice;

/// Routing graph seam. The routing-graph agent provides the implementation.
///
/// Required queries (to be finalized by the routing-graph agent):
/// - wires incident to a BEL pin
/// - wire-to-wire connectivity (PIP model)
/// - tile-to-tile adjacency
pub trait WireDb: Send + Sync {
    /// Number of wire nodes in the database. Zero for the placeholder.
    fn wire_count(&self) -> usize;
}

/// Timing database seam. The timing agent provides the implementation.
///
/// Required queries (to be finalized by the timing agent):
/// - BEL pin-to-pin delays
/// - wire delays
pub trait TimingDb: Send + Sync {
    /// True if the database has real delay numbers (false for placeholder).
    fn has_data(&self) -> bool;
}

/// Placeholder wire database. The routing-graph agent replaces this.
#[derive(Debug, Default)]
pub struct EmptyWireDb;

impl WireDb for EmptyWireDb {
    fn wire_count(&self) -> usize {
        0
    }
}

/// Placeholder timing database. The timing agent replaces this.
#[derive(Debug, Default)]
pub struct EmptyTimingDb;

impl TimingDb for EmptyTimingDb {
    fn has_data(&self) -> bool {
        false
    }
}

/// The fabric contract. Pack-place, router, and timing agents code against
/// this trait, not against `UltraScaleDevice` directly.
pub trait UltraScaleFabric: Send + Sync {
    /// BELs available at a tile. Empty slice if the tile is out of bounds.
    fn bels_at(&self, tile: TileId) -> &[Bel];
    /// Kind of the tile at the given coordinates, or None if out of bounds.
    fn tile_kind(&self, tile: TileId) -> Option<TileKind>;
    /// Routing graph. Sealed behind the `WireDb` trait until the
    /// routing-graph agent delivers.
    fn wires(&self) -> &dyn WireDb;
    /// Timing database. Sealed behind the `TimingDb` trait until the timing
    /// agent delivers.
    fn timing(&self) -> &dyn TimingDb;
    /// Part name, e.g. "xcvu3p".
    fn part(&self) -> &str;
}

/// `UltraScaleDevice` implements the fabric contract with placeholder
/// wire and timing databases.
pub struct FabricWithPlaceholders {
    device: UltraScaleDevice,
    wires: EmptyWireDb,
    timing: EmptyTimingDb,
}

impl FabricWithPlaceholders {
    pub fn xcvu3p() -> Self {
        Self {
            device: UltraScaleDevice::xcvu3p(),
            wires: EmptyWireDb,
            timing: EmptyTimingDb,
        }
    }

    /// Access the underlying device model.
    pub fn device(&self) -> &UltraScaleDevice {
        &self.device
    }
}

impl UltraScaleFabric for FabricWithPlaceholders {
    fn bels_at(&self, tile: TileId) -> &[Bel] {
        self.device.bels_at(tile)
    }

    fn tile_kind(&self, tile: TileId) -> Option<TileKind> {
        self.device.tile_kind(tile)
    }

    fn wires(&self) -> &dyn WireDb {
        &self.wires
    }

    fn timing(&self) -> &dyn TimingDb {
        &self.timing
    }

    fn part(&self) -> &str {
        self.device.part.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fabric_trait_shape_is_coherent() {
        let f = FabricWithPlaceholders::xcvu3p();
        assert_eq!(f.part(), "xcvu3p");
        // Wires and timing are placeholders on day 1.
        assert_eq!(f.wires().wire_count(), 0);
        assert!(!f.timing().has_data());
        // But BEL queries work through the trait.
        let tile = TileId::new(1, 0);
        assert!(f.tile_kind(tile).is_some());
        assert!(!f.bels_at(tile).is_empty());
    }

    #[test]
    fn fabric_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FabricWithPlaceholders>();
    }
}
