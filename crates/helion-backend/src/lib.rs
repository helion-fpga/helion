//! Backend trait: the pack → place → route → bitstream boundary.
//!
//! Roadmap milestone M1: the implement flow is driven through [`Backend`], not
//! through the free functions in `helion-pack` / `helion-place` / `helion-route` /
//! `helion-bits` directly. [`Hl10tBackend`] ports the existing HL10T flow to the
//! trait with **zero QoR movement** — same functions, same options, same order.
//!
//! Future (foreign) backends implement `Backend` against the frozen
//! [`NetlistIr`] contract (see `helion-ir`). This crate is architecture-neutral
//! scaffolding only: no vendor data, no vendor backends live here.

/// Re-exported flow types so backend drivers only need to depend on this crate.
pub use helion_bits::Bitstream;
pub use helion_device::Device;
pub use helion_ir::NetlistIr;
pub use helion_pack::Packed;
pub use helion_place::{PlaceOpts, Placed};
pub use helion_route::{RouteOpts, Routed};

/// One FPGA backend: consumes a frozen [`NetlistIr`] and produces a bitstream.
///
/// Errors stay `String`, matching the existing free-function convention; the
/// trait exists to fix the *stage order and the types*, not the error model.
pub trait Backend {
    /// Stable backend name, e.g. `"hl10t"`.
    fn name(&self) -> &str;
    /// Technology map: `NetlistIr` → packed LUTFF/IOB/BRAM/DSP clusters.
    fn pack(&self, netlist: &NetlistIr) -> Result<Packed, String>;
    /// Place packed clusters onto device sites.
    fn place(&self, packed: &Packed) -> Result<Placed, String>;
    /// PathFinder route placed clusters.
    fn route(&self, placed: &Placed) -> Result<Routed, String>;
    /// Encode the routed design into a device bitstream.
    fn bitstream(&self, routed: &Routed) -> Result<Bitstream, String>;
}

/// The HL10T backend: the existing free-function flow, ported to [`Backend`].
///
/// Behavior is byte-for-byte what `helion-cli` did before M1 (same functions,
/// same default options, same diagnostics), including the IOB-trim hang
/// mitigation and the empty-design bitgen fallback.
pub struct Hl10tBackend {
    dev: Device,
    timing_weight: f64,
}

impl Hl10tBackend {
    /// HL10T backend for `dev`, with placer `timing_weight = 0.0`.
    pub fn new(dev: Device) -> Self {
        Self {
            dev,
            timing_weight: 0.0,
        }
    }

    /// Set the placer timing weight (the CLI uses 0.75 for timing-closed runs).
    pub fn with_timing_weight(mut self, w: f64) -> Self {
        self.timing_weight = w;
        self
    }

    /// The device this backend targets.
    pub fn device(&self) -> &Device {
        &self.dev
    }
}

impl Backend for Hl10tBackend {
    fn name(&self) -> &str {
        "hl10t"
    }

    fn pack(&self, netlist: &NetlistIr) -> Result<Packed, String> {
        helion_pack::pack(netlist, &self.dev)
    }

    fn place(&self, packed: &Packed) -> Result<Placed, String> {
        // IOB trim (drop tied/undriven outputs) and the die-capacity gate
        // live in helion_place::place_with_guide — the single choke point for
        // the CLI, GUI engine thread, and Tcl session alike. Driven IOBs
        // beyond the pin budget fail loudly there instead of being capped.
        let t1 = std::time::Instant::now();
        let placed = helion_place::place_with(
            packed,
            &self.dev,
            PlaceOpts {
                timing_weight: self.timing_weight,
            },
        )?;
        eprintln!(
            "hang_diag place lutff_sites={} ms={}",
            placed.lutff_sites.len(),
            t1.elapsed().as_millis()
        );
        Ok(placed)
    }

    fn route(&self, placed: &Placed) -> Result<Routed, String> {
        let t2 = std::time::Instant::now();
        let route_opts = RouteOpts {
            max_iters: if placed.packed.lutffs.len() > 2000 {
                24
            } else {
                8
            },
            extra_hops: 0,
        };
        let routed = helion_route::route_with(placed, &self.dev, route_opts)?;
        eprintln!(
            "hang_diag route imux={} imux_skip={} pathfinder_iters={} overused={} ms={}",
            routed.imux.len(),
            routed.imux_skip,
            routed.pathfinder_iters,
            routed.overused,
            t2.elapsed().as_millis()
        );
        Ok(routed)
    }

    fn bitstream(&self, routed: &Routed) -> Result<Bitstream, String> {
        let t3 = std::time::Instant::now();
        let bits = match helion_bits::bitgen(&self.dev, routed) {
            Ok(b) => b,
            Err(e) if e.contains("empty design") => {
                // 0-LUT identity / no_body: reports still match occupancy.
                // write_bitstream / `helion bitstream` keep refusing empty frames.
                eprintln!(
                    "hang_diag bitgen empty/honest ms={}",
                    t3.elapsed().as_millis()
                );
                Bitstream::empty(&self.dev)
            }
            Err(e) => return Err(e),
        };
        eprintln!(
            "hang_diag bitgen bytes={} ms={}",
            bits.packets.len(),
            t3.elapsed().as_millis()
        );
        Ok(bits)
    }
}
