//! UltraScale+ pack and place for xcvu3p.
//!
//! Staging implementation for the pack-place crew member. Designed against
//! the INTERFACES.md `UltraScaleFabric` trait shape. Will integrate into
//! the `helion-ultrascale` crate when the device-model agent creates it.

pub mod legality;
pub mod pack;
pub mod place;

pub use legality::{check_legality, LegalityViolation};
pub use pack::{
    pack_ultra, ControlSetLimits, PackedUltraSlice, SliceKind, UltraBel, UltraPacked,
};
pub use place::{place_ultra, seeded_order, PlacedUltraSlice, UltraGrid, UltraPlaceOpts, UltraPlaced};
