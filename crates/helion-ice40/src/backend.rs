//! [`Ice40Backend`] and its [`helion_backend::Backend`] implementation.
//!
//! The stage types are the coordinator-owned stage modules
//! ([`crate::pack`], [`crate::place`], [`crate::route`], [`crate::asc`]).

use helion_backend::{Backend, NetlistIr, TimingModel};
use std::sync::OnceLock;

pub use crate::asc::emit_asc;
pub use crate::pack::PackedIce;
pub use crate::place::{place, PlacedIce};
pub use crate::route::{route, RoutedIce, Router};

/// iCE40 backend. Currently targets the HX8K (`"ice40-hx8k"`).
#[derive(Clone, Debug, Default)]
pub struct Ice40Backend {
    part: String,
}

impl Ice40Backend {
    /// Create a backend instance for `part` (e.g. `"ice40-hx8k"`).
    pub fn new(part: impl Into<String>) -> Self {
        Self { part: part.into() }
    }

    /// The shared routing graph, built once.
    fn router(&self) -> &'static Router {
        static ROUTER: OnceLock<Router> = OnceLock::new();
        ROUTER.get_or_init(Router::build)
    }
}

/// The bitstream type: IceStorm `.asc` text.
pub type BitstreamIce = String;

impl Backend for Ice40Backend {
    type Packed = PackedIce;
    type Placed = PlacedIce;
    type Routed = RoutedIce;
    type Bitstream = BitstreamIce;

    fn name(&self) -> &str {
        "ice40"
    }

    fn device_id(&self) -> &str {
        &self.part
    }

    fn pack(&self, netlist: &NetlistIr) -> Result<Self::Packed, String> {
        crate::pack::pack(netlist)
    }

    fn place(&self, packed: &Self::Packed) -> Result<Self::Placed, String> {
        place(packed)
    }

    fn route(&self, placed: &Self::Placed) -> Result<Self::Routed, String> {
        route(self.router(), placed)
    }

    fn bitstream(&self, routed: &Self::Routed) -> Result<Self::Bitstream, String> {
        emit_asc(self.router(), routed)
    }

    fn timing_model(&self) -> TimingModel {
        TimingModel::ice40_hx8k()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_stage_types<
        B: Backend<
            Packed = crate::pack::PackedIce,
            Placed = crate::place::PlacedIce,
            Routed = crate::route::RoutedIce,
            Bitstream = BitstreamIce,
        >,
    >() {
    }

    #[test]
    fn name_is_ice40() {
        let b = Ice40Backend::new("ice40-hx8k");
        assert_eq!(b.name(), "ice40");
    }

    #[test]
    fn device_id_mentions_part() {
        let b = Ice40Backend::new("ice40-hx8k");
        assert!(b.device_id().contains("ice40-hx8k"));
    }

    #[test]
    fn stage_types_are_the_coordinator_stage_modules() {
        assert_stage_types::<Ice40Backend>();
    }

    /// The provided `Backend::run` driver must drive the full flow.
    #[test]
    fn run_driver_produces_asc() {
        use helion_ir::{CellKind, PortDir};
        let mut d = NetlistIr::new("buf");
        d.add_port("clk", PortDir::In);
        d.add_port("led0", PortDir::Out);
        d.add_cell("ib", CellKind::IobIn);
        d.connect("a_net", "ib", "O");
        d.connect("clk", "ib", "PAD");
        d.add_cell("lut", CellKind::Lut6 { init: 0xAAAAAAAAAAAAAAAA });
        d.connect("a_net", "lut", "I0");
        d.connect("y_net", "lut", "O");
        d.add_cell("ob", CellKind::IobOut);
        d.connect("y_net", "ob", "I");
        d.connect("led0", "ob", "PAD");

        let b = Ice40Backend::new("ice40-hx8k");
        let asc = b.run(&d).expect("Backend::run");
        assert!(asc.starts_with(".device 8k\n"));
        assert!(asc.contains(".logic_tile"));
    }

    #[test]
    fn timing_model_is_ice40_hx8k() {
        let b = Ice40Backend::new("ice40-hx8k");
        let m = b.timing_model();
        assert_eq!(m, TimingModel::ice40_hx8k());
        assert!(m.lut_ps > 0 && m.iob_ps > m.lut_ps);
    }
}
