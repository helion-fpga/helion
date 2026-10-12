//! [`Xilinx7Backend`] and its [`helion_backend::Backend`] implementation.
//!
//! The stage types are re-exported from the coordinator-owned stage modules
//! ([`crate::pack`], [`crate::place`], [`crate::route`], [`crate::emit`]):
//! this module names them via `pub use` so the trait's associated types stay
//! wired to the real stage definitions as the flow logic lands.

use helion_backend::{Backend, NetlistIr};

pub use crate::emit::Bitstream7;
pub use crate::pack::Packed7;
pub use crate::place::Placed7;
pub use crate::route::{Routed7, RoutedNet7};

/// Xilinx 7-series backend. `part` is the concrete target part, e.g. `xc7a35t`.
#[derive(Clone, Debug)]
pub struct Xilinx7Backend {
    part: String,
}

impl Xilinx7Backend {
    /// Create a backend instance for `part` (e.g. `"xc7a35t"`).
    pub fn new(part: impl Into<String>) -> Self {
        Self { part: part.into() }
    }
}

const NOT_IMPL: &str = "not yet implemented";

impl Backend for Xilinx7Backend {
    type Packed = Packed7;
    type Placed = Placed7;
    type Routed = Routed7;
    type Bitstream = Bitstream7;

    fn name(&self) -> &str {
        "xilinx7"
    }

    fn device_id(&self) -> &str {
        &self.part
    }

    fn pack(&self, _netlist: &NetlistIr) -> Result<Self::Packed, String> {
        Err(NOT_IMPL.to_string())
    }

    fn place(&self, _packed: &Self::Packed) -> Result<Self::Placed, String> {
        Err(NOT_IMPL.to_string())
    }

    fn route(&self, _placed: &Self::Placed) -> Result<Self::Routed, String> {
        Err(NOT_IMPL.to_string())
    }

    fn bitstream(&self, _routed: &Self::Routed) -> Result<Self::Bitstream, String> {
        Err(NOT_IMPL.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time assertion: the trait's associated types are exactly the
    /// coordinator-owned stage types.
    fn assert_stage_types<
        B: Backend<
            Packed = crate::pack::Packed7,
            Placed = crate::place::Placed7,
            Routed = crate::route::Routed7,
            Bitstream = crate::emit::Bitstream7,
        >,
    >() {
    }

    #[test]
    fn name_is_xilinx7() {
        let b = Xilinx7Backend::new("xc7a35t");
        assert_eq!(b.name(), "xilinx7");
    }

    #[test]
    fn device_id_mentions_part() {
        let b = Xilinx7Backend::new("xc7a35t");
        assert!(
            b.device_id().contains("xc7a35t"),
            "device_id = {}",
            b.device_id()
        );
    }

    #[test]
    fn stage_types_are_the_coordinator_stage_modules() {
        assert_stage_types::<Xilinx7Backend>();
    }
}
