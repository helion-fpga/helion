//! Architecture-neutral backend trait for Helion.
//!
//! This crate is scaffolding only: it defines the shape every device backend
//! plugs into, and carries **no vendor data, no device geometry, and no flow
//! algorithms**. Each backend (HL10T, Xilinx 7-series, ...) owns its own
//! crate, its own per-stage types, and its own device databases.
//!
//! The central contract is [`Backend`]: four stage types chosen by the
//! backend via associated types, and four fallible stages
//! (`pack` → `place` → `route` → `bitstream`) wired in that order. Associated
//! types (not concrete HL10T stage types) are deliberate: a backend's packed,
//! placed, routed, and bitstream forms are shaped by its own device model,
//! and forcing them through another target's types would miscompile.
//!
//! Errors are plain `String`s, following the project convention for
//! engine-boundary errors.

pub use helion_ir::NetlistIr;

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
}
