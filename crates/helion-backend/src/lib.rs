//! Architecture-neutral backend trait for Helion.
//!
//! This crate is scaffolding only: it defines the shape every device backend
//! plugs into, and carries **no vendor data, no device geometry, and no flow
//! algorithms**. Each backend (HL10T, Xilinx 7-series, ...) owns its own
//! crate, its own per-stage types, and its own device databases.
//!
//! The central contract is [`Backend`]: four stage types chosen by the
//! backend via associated types, and four fallible stages
//! (`pack` → `place` → `route` → `bitstream`) wired in that order by the
//! provided [`Backend::run`] driver. Associated
//! types (not concrete HL10T stage types) are deliberate: a backend's packed,
//! placed, routed, and bitstream forms are shaped by its own device model,
//! and forcing them through another target's types would miscompile.
//!
//! Errors are plain `String`s, following the project convention for
//! engine-boundary errors.

pub use helion_ir::NetlistIr;

/// Per-device timing delays in picoseconds, supplied by each backend.
///
/// This is the single source of truth for cell and wire delays. Static
/// timing analysis consumes a `TimingModel`; backends provide theirs via
/// [`Backend::timing_model`]. HL10T analysis defaults to
/// [`TimingModel::hl10t`], which carries the gold values (moving them moves
/// `WNS_PS=9640`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimingModel {
    /// LUT combinational arc (input pin to output).
    pub lut_ps: i64,
    /// Flip-flop clock-to-Q.
    pub ff_ckq_ps: i64,
    /// Flip-flop setup requirement.
    pub setup_ps: i64,
    /// LUT input pin / net arc, per fanin.
    pub pin_ps: i64,
    /// I/O pad delay.
    pub iob_ps: i64,
    /// Routing hop delay (per Manhattan tile step).
    pub hop_ps: i64,
    /// Hold requirement.
    pub hold_req_ps: i64,
    /// Carry chain arc (CI to CO through Carry4). Much faster than LUT.
    pub carry_ps: i64,
}

impl TimingModel {
    /// HL10T: the gold timing values. Do not change without updating the
    /// QoR gate (`WNS_PS=9640` for the counter gold design).
    pub fn hl10t() -> Self {
        Self {
            lut_ps: 150,
            ff_ckq_ps: 80,
            setup_ps: 50,
            pin_ps: 20,
            iob_ps: 100,
            hop_ps: 40,
            hold_req_ps: 20,
            carry_ps: 50,
        }
    }

    /// Xilinx 7-series (`xc7a35t`, speed grade -1).
    ///
    /// Preliminary calibration, NOT correlated against Vivado yet. Magnitudes
    /// follow the DS181 (-1) ranges: LUT6 A->O ~0.1-0.3ns across input pins,
    /// FD Ck->Q ~0.3-0.4ns, IOB LVCMOS33 ~1ns, short-route hops ~50-100ps.
    /// Treat every number here as order-of-magnitude until a timing
    /// correlation campaign against Vivado-placed reference designs lands.
    /// A correlation harness should: run N reference designs through Vivado,
    /// extract per-arc delays from its timing reports, and fit these fields.
    pub fn xilinx7() -> Self {
        Self {
            lut_ps: 180,
            ff_ckq_ps: 380,
            setup_ps: 120,
            pin_ps: 30,
            iob_ps: 1000,
            hop_ps: 70,
            hold_req_ps: 100,
            carry_ps: 60,
        }
    }

    /// iCE40-HX8K.
    ///
    /// Preliminary calibration, NOT correlated against iCECube2 or nextpnr
    /// yet. Datasheet-anchored magnitudes (FPGA-DS-02029, HX8K): tPD
    /// (pin-LUT-pin through one LUT4) 7.30ns max, PIO tCO 5.41ns, PIO tH
    /// 2.38ns, fMAX_GBUF 275MHz, global skew 300ps. The internal cell arcs
    /// below are order-of-magnitude splits of those external numbers, not
    /// measured cell delays. Treat every number here as a placeholder until
    /// a timing correlation campaign against nextpnr-ice40 placed reference
    /// designs lands. A correlation harness should: run N reference designs
    /// through nextpnr-ice40, extract per-arc delays from its timing reports,
    /// and fit these fields.
    pub fn ice40_hx8k() -> Self {
        Self {
            lut_ps: 1000,
            ff_ckq_ps: 500,
            setup_ps: 300,
            pin_ps: 100,
            iob_ps: 2500,
            hop_ps: 200,
            hold_req_ps: 500,
            carry_ps: 300,
        }
    }

    /// Lattice ECP5 LFE5U-85F.
    ///
    /// Preliminary calibration, NOT correlated against nextpnr-ecp5 or
    /// Lattice Diamond yet. Order-of-magnitude placeholders for a mid speed
    /// grade device: LUT4 arc a few hundred ps, FF clock-to-Q in the same
    /// range, IOB pad delay under a nanosecond, per-tile routing hops around
    /// a hundred ps. Treat every number here as a placeholder until a timing
    /// correlation campaign against nextpnr-ecp5 placed reference designs
    /// lands. A correlation harness should: run N reference designs through
    /// nextpnr-ecp5, extract per-arc delays from its timing reports, and fit
    /// these fields.
    pub fn ecp5_85f() -> Self {
        Self {
            lut_ps: 350,
            ff_ckq_ps: 250,
            setup_ps: 150,
            pin_ps: 50,
            iob_ps: 800,
            hop_ps: 100,
            hold_req_ps: 80,
            carry_ps: 120,
        }
    }
}

/// A complete FPGA implementation flow for one device family.
///
/// Implementors bring their own stage types (e.g. a Xilinx 7-series backend
/// carries FASM lines and frame data through [`Backend::Bitstream`]) and
/// drive them through [`Backend::pack`], [`Backend::place`],
/// [`Backend::route`], and [`Backend::bitstream`].
pub trait Backend {
    /// Packed netlist: cells grouped into target sites (shape is backend-owned).
    type Packed;
    /// Placed packed netlist: sites chosen on the device (backend-owned).
    type Placed;
    /// Routed placed netlist: nets assigned to routing resources (backend-owned).
    type Routed;
    /// Device bitstream (or its textual equivalent, e.g. FASM; backend-owned).
    type Bitstream;

    /// Human-readable backend name, e.g. `"xilinx7"`.
    fn name(&self) -> &str;

    /// Concrete device identifier this instance targets, e.g. `"xc7a35t"`.
    fn device_id(&self) -> &str;

    /// Group IR cells into target sites.
    fn pack(&self, netlist: &NetlistIr) -> Result<Self::Packed, String>;

    /// Choose a device site for each packed element.
    fn place(&self, packed: &Self::Packed) -> Result<Self::Placed, String>;

    /// Route every net on the device's routing fabric.
    fn route(&self, placed: &Self::Placed) -> Result<Self::Routed, String>;

    /// Emit the final bitstream (or its textual form, e.g. FASM).
    fn bitstream(&self, routed: &Self::Routed) -> Result<Self::Bitstream, String>;

    /// Drive the full flow in order: pack → place → route → bitstream.
    ///
    /// This is the single sanctioned driver. Backends must not need a
    /// side-channel `flow` module: everything a stage needs lives in `self`
    /// (device model, options, caches), so a third backend is implementable
    /// with no out-of-trait machinery.
    fn run(&self, netlist: &NetlistIr) -> Result<Self::Bitstream, String> {
        let packed = self.pack(netlist)?;
        let placed = self.place(&packed)?;
        drop(packed);
        let routed = self.route(&placed)?;
        drop(placed);
        self.bitstream(&routed)
    }

    /// This backend's timing model: the cell and wire delays static timing
    /// analysis must use for this device. Timing-closed means the same thing
    /// on every backend because every backend's STA consumes its own model.
    fn timing_model(&self) -> TimingModel;
}

/// Zero-allocation structural memory footprint summary of a [`NetlistIr`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetlistFootprint {
    pub ports: usize,
    pub cells: usize,
    pub nets: usize,
    pub endpoints: usize,
    pub instances: usize,
    pub estimated_heap_bytes: usize,
}

impl NetlistFootprint {
    /// Compute structural counts and estimated in-memory footprint in bytes
    /// without allocating.
    pub fn from_netlist(netlist: &NetlistIr) -> Self {
        let ports = netlist.ports.len();
        let cells = netlist.cells.len();
        let nets = netlist.nets.len();
        let instances = netlist.instances.len();
        let mut endpoints = 0usize;
        for n in &netlist.nets {
            endpoints = endpoints.saturating_add(n.endpoints.len());
        }
        let port_bytes = ports.saturating_mul(core::mem::size_of::<helion_ir::Port>());
        let cell_bytes = cells.saturating_mul(core::mem::size_of::<helion_ir::Cell>());
        let net_bytes = nets.saturating_mul(core::mem::size_of::<helion_ir::Net>());
        let ep_bytes = endpoints.saturating_mul(core::mem::size_of::<helion_ir::Endpoint>());
        let inst_bytes = instances.saturating_mul(core::mem::size_of::<helion_ir::Instance>());
        let estimated_heap_bytes = port_bytes
            .saturating_add(cell_bytes)
            .saturating_add(net_bytes)
            .saturating_add(ep_bytes)
            .saturating_add(inst_bytes);
        Self {
            ports,
            cells,
            nets,
            endpoints,
            instances,
            estimated_heap_bytes,
        }
    }

    /// True when the estimated footprint fits within `max_mb` MiB.
    pub fn within_budget_mb(&self, max_mb: usize) -> bool {
        self.estimated_heap_bytes <= max_mb.saturating_mul(1024 * 1024)
    }
}

/// Per-stage memory footprint (bytes) and wall-clock duration (microseconds)
/// across a `pack -> place -> route -> bitstream` backend run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StageFootprint {
    pub netlist_bytes: usize,
    pub packed_bytes: usize,
    pub placed_bytes: usize,
    pub routed_bytes: usize,
    pub bitstream_bytes: usize,
    pub pack_us: u64,
    pub place_us: u64,
    pub route_us: u64,
    pub bitstream_us: u64,
}

impl StageFootprint {
    /// Total implementation runtime in microseconds.
    pub fn total_us(&self) -> u64 {
        self.pack_us
            .saturating_add(self.place_us)
            .saturating_add(self.route_us)
            .saturating_add(self.bitstream_us)
    }

    /// Peak live stage bytes when each intermediate stage is dropped as soon
    /// as its successor stage finishes (`max` of adjacent stage pairs).
    pub fn peak_retained_bytes(&self) -> usize {
        let p1 = self.netlist_bytes.saturating_add(self.packed_bytes);
        let p2 = self
            .netlist_bytes
            .saturating_add(self.packed_bytes)
            .saturating_add(self.placed_bytes);
        let p3 = self
            .netlist_bytes
            .saturating_add(self.placed_bytes)
            .saturating_add(self.routed_bytes);
        let p4 = self
            .netlist_bytes
            .saturating_add(self.routed_bytes)
            .saturating_add(self.bitstream_bytes);
        p1.max(p2).max(p3).max(p4)
    }

    /// Cumulative sum of all stage payloads (upper bound if nothing were dropped early).
    pub fn cumulative_stage_bytes(&self) -> usize {
        self.netlist_bytes
            .saturating_add(self.packed_bytes)
            .saturating_add(self.placed_bytes)
            .saturating_add(self.routed_bytes)
            .saturating_add(self.bitstream_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::Design;

    #[test]
    fn netlist_footprint_counts_and_budget() {
        let d = Design::structural_counter();
        let fp = NetlistFootprint::from_netlist(&d);
        assert_eq!(fp.ports, 2);
        assert_eq!(fp.cells, 9);
        assert!(fp.nets >= 8);
        assert!(fp.endpoints >= 20);
        assert!(fp.estimated_heap_bytes > 0);
        assert!(fp.within_budget_mb(1));
    }

    #[test]
    fn stage_footprint_early_drop_lowers_peak_below_cumulative() {
        let sf = StageFootprint {
            netlist_bytes: 4_096,
            packed_bytes: 8_192,
            placed_bytes: 8_192,
            routed_bytes: 16_384,
            bitstream_bytes: 4_096,
            pack_us: 100,
            place_us: 200,
            route_us: 300,
            bitstream_us: 50,
        };
        assert_eq!(sf.total_us(), 650);
        assert!(sf.peak_retained_bytes() < sf.cumulative_stage_bytes());
    }
}
