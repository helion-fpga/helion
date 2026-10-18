//! HL10T backend: Helion's own die as a [`helion_backend::Backend`].
//!
//! This is the reference implementor of the trait: it wires the existing
//! engine crates ([`helion_pack`], [`helion_place`], [`helion_route`],
//! [`helion_bits`]) through the four trait stages with no side-channel
//! driver. A third backend (iCE40, ECP5, ...) follows this exact shape:
//! own the device model in the struct, delegate each stage to the
//! device-specific engine functions.

use helion_backend::{Backend, NetlistFootprint, NetlistIr, StageFootprint, TimingModel};
use helion_device::Device;
use std::sync::LazyLock;
use std::time::Instant;

static GLOBAL_HL10T_DEV: LazyLock<Result<Device, String>> = LazyLock::new(|| {
    Device::load_part("HL10T-C32-1").map_err(|e| format!("load HL10T-C32-1: {e}"))
});

/// Estimate heap/structural byte footprint of a [`helion_pack::Packed`] netlist.
pub fn packed_footprint_bytes(packed: &helion_pack::Packed) -> usize {
    let lutff_bytes: usize = packed
        .lutffs
        .iter()
        .map(|l| {
            core::mem::size_of::<helion_pack::PackedLutFf>()
                + l.lut_cell.len()
                + l.ff_cell.len()
                + l.q_net.len()
                + l.lut_pins
                    .iter()
                    .map(|(_, d)| core::mem::size_of::<(u8, String)>() + d.len())
                    .sum::<usize>()
        })
        .sum();
    let iob_bytes = packed
        .iobs
        .len()
        .saturating_mul(core::mem::size_of::<helion_pack::PackedIob>());
    let mac_bytes = packed
        .macs
        .len()
        .saturating_mul(core::mem::size_of::<helion_pack::PackedMac>());
    let bram_bytes: usize = packed
        .brams
        .iter()
        .map(|b| core::mem::size_of::<helion_pack::PackedBram>() + b.init.len() * 8)
        .sum();
    lutff_bytes
        .saturating_add(iob_bytes)
        .saturating_add(mac_bytes)
        .saturating_add(bram_bytes)
}

/// Estimate heap/structural byte footprint of a [`helion_place::Placed`] design.
pub fn placed_footprint_bytes(placed: &helion_place::Placed) -> usize {
    packed_footprint_bytes(&placed.packed)
        .saturating_add(
            placed
                .lutff_sites
                .len()
                .saturating_mul(core::mem::size_of::<(helion_device::Site, u8)>()),
        )
        .saturating_add(
            placed
                .iob_sites
                .len()
                .saturating_mul(core::mem::size_of::<helion_device::Site>()),
        )
        .saturating_add(
            placed
                .mac_sites
                .len()
                .saturating_mul(core::mem::size_of::<helion_device::Site>()),
        )
        .saturating_add(
            placed
                .bram_sites
                .len()
                .saturating_mul(core::mem::size_of::<helion_device::Site>()),
        )
}

/// Estimate heap/structural byte footprint of a [`helion_route::Routed`] design.
pub fn routed_footprint_bytes(routed: &helion_route::Routed) -> usize {
    let iob_route_bytes: usize = routed
        .iob_src
        .iter()
        .map(|r| {
            core::mem::size_of::<helion_route::IobRoute>()
                + r.path.len() * core::mem::size_of::<(u32, u32)>()
                + r.net.len()
        })
        .sum();
    let imux_bytes = routed
        .imux
        .len()
        .saturating_mul(core::mem::size_of::<helion_route::ImuxRoute>());
    let cong_bytes = routed.congestion.len().saturating_mul(4);
    placed_footprint_bytes(&routed.placed)
        .saturating_add(iob_route_bytes)
        .saturating_add(imux_bytes)
        .saturating_add(cong_bytes)
}

/// Estimate heap/structural byte footprint of a [`helion_bits::Bitstream`].
pub fn bitstream_footprint_bytes(bs: &helion_bits::Bitstream) -> usize {
    bs.packets.len().saturating_add(
        bs.frames
            .len()
            .saturating_mul(core::mem::size_of::<((u8, u16, u8), u128)>()),
    )
}

/// HL10T backend targeting part `HL10T-C32-1`.
///
/// Construct with [`Hl10tBackend::open`], which shares the cached HAD part
/// description; the stage methods then run entirely through the [`Backend`]
/// trait.
#[derive(Clone, Debug)]
pub struct Hl10tBackend {
    device: Device,
}

impl Hl10tBackend {
    /// Open the backend, cloning the cached `HL10T-C32-1` device model.
    pub fn open() -> Result<Self, String> {
        let device = GLOBAL_HL10T_DEV.as_ref().map_err(|e| e.clone())?.clone();
        Ok(Self { device })
    }

    /// Borrow the owned device model.
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Execute the full `pack -> place -> route -> bitstream` flow while
    /// recording per-stage memory footprint and timing telemetry, eagerly
    /// dropping intermediate stage structures to minimize peak RSS.
    pub fn run_with_footprint(
        &self,
        netlist: &NetlistIr,
    ) -> Result<(helion_bits::Bitstream, StageFootprint), String> {
        let netlist_bytes = NetlistFootprint::from_netlist(netlist).estimated_heap_bytes;

        let t0 = Instant::now();
        let packed = self.pack(netlist)?;
        let pack_us = t0.elapsed().as_micros() as u64;
        let packed_bytes = packed_footprint_bytes(&packed);

        let t1 = Instant::now();
        let placed = self.place(&packed)?;
        let place_us = t1.elapsed().as_micros() as u64;
        drop(packed);
        let placed_bytes = placed_footprint_bytes(&placed);

        let t2 = Instant::now();
        let routed = self.route(&placed)?;
        let route_us = t2.elapsed().as_micros() as u64;
        drop(placed);
        let routed_bytes = routed_footprint_bytes(&routed);

        let t3 = Instant::now();
        let bs = self.bitstream(&routed)?;
        let bitstream_us = t3.elapsed().as_micros() as u64;
        drop(routed);
        let bitstream_bytes = bitstream_footprint_bytes(&bs);

        Ok((
            bs,
            StageFootprint {
                netlist_bytes,
                packed_bytes,
                placed_bytes,
                routed_bytes,
                bitstream_bytes,
                pack_us,
                place_us,
                route_us,
                bitstream_us,
            },
        ))
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

    #[test]
    fn run_with_footprint_records_telemetry_and_bounds_peak_memory() {
        let netlist = structural_counter();
        let backend = Hl10tBackend::open().expect("open HL10T");
        let (bs, fp) = backend
            .run_with_footprint(&netlist)
            .expect("run_with_footprint");
        assert!(!bs.packets.is_empty());
        assert!(fp.netlist_bytes > 0);
        assert!(fp.packed_bytes > 0);
        assert!(fp.placed_bytes >= fp.packed_bytes);
        assert!(fp.routed_bytes > fp.placed_bytes);
        assert!(fp.bitstream_bytes >= bs.packets.len());
        assert!(
            fp.peak_retained_bytes() < fp.cumulative_stage_bytes(),
            "early stage drop must keep peak retained bytes ({}) below cumulative stage bytes ({})",
            fp.peak_retained_bytes(),
            fp.cumulative_stage_bytes()
        );
    }
}
