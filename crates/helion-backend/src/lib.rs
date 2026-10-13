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
        let routed = self.route(&placed)?;
        self.bitstream(&routed)
    }

    /// This backend's timing model: the cell and wire delays static timing
    /// analysis must use for this device. Timing-closed means the same thing
    /// on every backend because every backend's STA consumes its own model.
    fn timing_model(&self) -> TimingModel;
}
