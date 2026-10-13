//! HL10T backend: Helion's own die as a [`helion_backend::Backend`].
//!
//! This is the reference implementor of the trait: it wires the existing
//! engine crates ([`helion_pack`], [`helion_place`], [`helion_route`],
//! [`helion_bits`]) through the four trait stages with no side-channel
//! driver. A third backend (iCE40, ECP5, ...) follows this exact shape:
//! own the device model in the struct, delegate each stage to the
//! device-specific engine functions.

use helion_backend::{Backend, NetlistIr, TimingModel};
use helion_device::Device;

/// HL10T backend targeting part `HL10T-C32-1`.
///
/// Construct with [`Hl10tBackend::open`], which loads the HAD part
/// description; the stage methods then run entirely through the [`Backend`]
/// trait.
#[derive(Clone, Debug)]
pub struct Hl10tBackend {
    device: Device,
}

impl Hl10tBackend {
    /// Open the backend, loading the `HL10T-C32-1` device model from HAD.
    pub fn open() -> Result<Self, String> {
        let device = Device::load_part("HL10T-C32-1")
            .map_err(|e| format!("load HL10T-C32-1: {e}"))?;
        Ok(Self { device })
    }

    /// Borrow the owned device model.
    pub fn device(&self) -> &Device {
        &self.device
    }
}

impl Backend for Hl10tBackend {
    type Packed = helion_pack::Packed;
    type Placed = helion_place::Placed;
    type Routed = helion_route::Routed;
    type Bitstream = helion_bits::Bitstream;

    fn name(&self) -> &str {
        "hl10t"
    }

    fn device_id(&self) -> &str {
        "HL10T-C32-1"
    }

    fn pack(&self, netlist: &NetlistIr) -> Result<Self::Packed, String> {
        helion_pack::pack(netlist, &self.device)
    }

    fn place(&self, packed: &Self::Packed) -> Result<Self::Placed, String> {
        helion_place::place(packed, &self.device)
    }

    fn route(&self, placed: &Self::Placed) -> Result<Self::Routed, String> {
        helion_route::route(placed, &self.device)
    }

    fn bitstream(&self, routed: &Self::Routed) -> Result<Self::Bitstream, String> {
        helion_bits::bitgen(&self.device, routed)
    }

    fn timing_model(&self) -> TimingModel {
        TimingModel::hl10t()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_backend::Backend as _;

    fn structural_counter() -> NetlistIr {
        helion_ir::Design::structural_counter()
    }

    #[test]
    fn name_and_device_id() {
        let b = Hl10tBackend::open().expect("open HL10T");
        assert_eq!(b.name(), "hl10t");
        assert_eq!(b.device_id(), "HL10T-C32-1");
    }

    #[test]
    fn trait_run_matches_direct_flow() {
        let netlist = structural_counter();
        let backend = Hl10tBackend::open().expect("open HL10T");

        let via_trait = backend.run(&netlist).expect("trait run");

        let device = helion_device::Device::load_part("HL10T-C32-1").expect("load part");
        let packed = helion_pack::pack(&netlist, &device).expect("pack");
        let placed = helion_place::place(&packed, &device).expect("place");
        let routed = helion_route::route(&placed, &device).expect("route");
        let direct = helion_bits::bitgen(&device, &routed).expect("bitgen");

        assert_eq!(
            via_trait.packets, direct.packets,
            "trait-driven and direct HL10T bitstreams must agree"
        );
    }
}
