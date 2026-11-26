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
    /// Open the backend for `part`: a full part name the X-Ray database
    /// describes (`"xc7a35tcsg324-1"`, `"xc7z020clg484-1"`, ...; see
    /// [`crate::parts`]) or the short `"xc7a35t"` / `"xc7z020"`. Loads the
    /// device database and precomputes the routing node maps.
    ///
    /// A part the database does not describe is a clear error, not a silent
    /// fallback. (`"xc7a35t"` alone falls back to a small synthetic device
    /// when no database is installed, for unit tests.)
    pub fn open(part: impl Into<String>) -> Result<Self, String> {
        let part = part.into();
        let root = default_db_root();
        let mut device = match crate::parts::resolve(&root, &part) {
            Ok(p) => Device::load_part(&p)?,
            Err(_) if part == "xc7a35t" => Device::load_xc7a35t(&root)?,
            Err(e) => return Err(e),
        };
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

    /// The part this backend was opened for.
    pub fn part(&self) -> &str {
        &self.part
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
        let mut placed = crate::place::place_with_pools(packed, &xdev, (self.device.bram18_sites(), self.device.bram36_sites(), self.device.dsp48_sites()))?;
        placed.tie_eps = crate::bram::tie0_endpoints(&placed, &self.device, &xdev);
        Ok(placed)
    }

    fn route(&self, placed: &Self::Placed) -> Result<Self::Routed, String> {
        let xdev = self.xdevice(&placed.packed.bufgs);
        crate::route::route(placed, &xdev)
    }

    fn bitstream(&self, routed: &Self::Routed) -> Result<Self::Bitstream, String> {
        let xdev = self.xdevice(&routed.placed.packed.bufgs);
        let mut bs = crate::emit::emit_fasm(routed, &xdev)?;
        // Constant BRAM and DSP inputs: ground ties need the device graph, which
        // emit's device trait does not offer.
        bs.fasm.extend(crate::bram::tie_lines(&routed.placed, &routed.nets, &self.device, &xdev)?);
        bs.fasm.extend(crate::dsp::tie_lines(&routed.placed, &routed.nets, &self.device, &xdev)?);
        bs.fasm.extend(crate::cmt::tie_lines(&routed.placed, &routed.nets, &self.device, &xdev)?);
        bs.fasm.extend(crate::ps7::tie_lines(&routed.placed, &routed.nets, &self.device, &xdev)?);
        let casc = crate::bram::cascade_out_lines(&bs.fasm);
        bs.fasm.extend(casc);
        bs.fasm.sort();
        bs.fasm.dedup();
        Ok(bs)
    }

    fn timing_model(&self) -> TimingModel {
        TimingModel::xilinx7()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::resolve;

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
    fn open_accepts_xc7z020() {
        let b = Xilinx7Backend::open("xc7z020").expect("open xc7z020");
        assert!(
            b.device_id().contains("xc7z020"),
            "device_id = {}",
            b.device_id()
        );
        assert_eq!(b.device().part().idcode, 0x0372_7093);
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
            match resolve(port.name) {
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
