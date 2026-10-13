//! [`Xilinx7Backend`] and its [`helion_backend::Backend`] implementation.
//!
//! The backend owns its device model: [`Xilinx7Backend::open`] loads the
//! X-Ray [`crate::device::Device`] and precomputes the node maps, so the
//! trait stage methods need no side-channel driver. Each stage builds a
//! short-lived [`crate::xdevice::XDevice`] borrowing the owned device.

use crate::device::{Device, default_db_root};
use crate::xdevice::{NodeMaps, XDevice, build_node_map};
use helion_backend::{Backend, NetlistIr, TimingModel};

pub use crate::emit::Bitstream7;
pub use crate::pack::Packed7;
pub use crate::place::Placed7;
pub use crate::route::{Routed7, RoutedNet7};

/// Xilinx 7-series backend. `part` is the concrete target part, e.g. `xc7a35t`.
///
/// Construct with [`Xilinx7Backend::open`], which loads the X-Ray device
/// database; the stage methods then run entirely through the [`Backend`]
/// trait with no out-of-trait driver.
#[derive(Clone, Debug)]
pub struct Xilinx7Backend {
    part: String,
    device: Device,
    node_maps: NodeMaps,
}

impl Xilinx7Backend {
    /// Open the backend for `part` (e.g. `"xc7a35t"`), loading the X-Ray
    /// device database and precomputing the routing node maps.
    ///
    /// Only parts with a wired-up database are accepted; anything else is a
    /// clear error, not a silent fallback.
    pub fn open(part: impl Into<String>) -> Result<Self, String> {
        let part = part.into();
        if part != "xc7a35t" {
            return Err(format!(
                "unsupported part {part}: only xc7a35t has an X-Ray database wired up"
            ));
        }
        let mut device = Device::load_xc7a35t(&default_db_root())?;
        let node_maps = build_node_map(&mut device);
        Ok(Self {
            part,
            device,
            node_maps,
        })
    }

    /// Borrow the owned device model.
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Build the per-stage [`XDevice`] query adapter, registering the packed
    /// BUFG outputs as clock nets so the router uses the dedicated spine.
    fn xdevice(&self, bufgs: &[crate::pack::PackedBufg]) -> XDevice<'_> {
        let mut xdev = XDevice::new(&self.device, self.node_maps.clone());
        for b in bufgs {
            xdev.clock_nets.insert(b.out_net.clone());
        }
        xdev
    }
}

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

    fn pack(&self, netlist: &NetlistIr) -> Result<Self::Packed, String> {
        crate::pack::pack(netlist)
    }

    fn place(&self, packed: &Self::Packed) -> Result<Self::Placed, String> {
        let xdev = self.xdevice(&packed.bufgs);
        crate::place::place(packed, &xdev)
    }

    fn route(&self, placed: &Self::Placed) -> Result<Self::Routed, String> {
        let xdev = self.xdevice(&placed.packed.bufgs);
        crate::route::route(placed, &xdev)
    }

    fn bitstream(&self, routed: &Self::Routed) -> Result<Self::Bitstream, String> {
        let xdev = self.xdevice(&routed.placed.packed.bufgs);
        crate::emit::emit_fasm(routed, &xdev)
    }

    fn timing_model(&self) -> TimingModel {
        TimingModel::xilinx7()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_backend::Backend as _;

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
        let b = Xilinx7Backend::open("xc7a35t").expect("open xc7a35t");
        assert_eq!(b.name(), "xilinx7");
    }

    #[test]
    fn device_id_mentions_part() {
        let b = Xilinx7Backend::open("xc7a35t").expect("open xc7a35t");
        assert!(
            b.device_id().contains("xc7a35t"),
            "device_id = {}",
            b.device_id()
        );
    }

    #[test]
    fn open_rejects_unknown_part() {
        let err = Xilinx7Backend::open("xc7a100t").expect_err("unknown part must fail");
        assert!(err.contains("xc7a100t"), "{err}");
    }

    #[test]
    fn stage_types_are_the_coordinator_stage_modules() {
        assert_stage_types::<Xilinx7Backend>();
    }

    #[test]
    fn trait_run_matches_flow_run() {
        let mut netlist = helion_ir::Design::structural_counter();
        // Arty A7 LOCs, mirroring the blinky e2e test.
        for port in &mut netlist.ports {
            match port.name.as_str() {
                "clk" => port.attrs.set("LOC", "E3"),
                "led" => port.attrs.set("LOC", "H5"),
                _ => {}
            }
        }
        let via_trait = Xilinx7Backend::open("xc7a35t")
            .expect("open")
            .run(&netlist)
            .expect("trait run");
        let via_flow = crate::flow::run_for_part(&netlist, "xc7a35t").expect("flow run");
        assert_eq!(
            via_trait.fasm, via_flow.fasm,
            "trait-driven and flow-driven FASM must agree"
        );
    }
}
