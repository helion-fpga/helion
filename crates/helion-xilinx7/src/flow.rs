//! End-to-end flow orchestration for the Xilinx 7-series backend.
//!
//! [`run`] drives the four stages in order: pack → place → route → FASM
//! emission. The stage functions live in the coordinator-owned modules
//! ([`crate::pack`], [`crate::place`], [`crate::route`], [`crate::emit`]);
//! this module only loads the X-Ray device database, builds the
//! [`crate::xdevice::XDevice`] query adapter, and threads the stages together.

use crate::device::{Device, default_db_root};
use crate::emit::{Bitstream7, emit_fasm};
use crate::pack::pack;
use crate::place::place;
use crate::route::route;
use crate::xdevice::XDevice;
use helion_backend::NetlistIr;

/// Run the full 7-series flow on `netlist` for the default part (`xc7a35t`),
/// returning the emitted [`Bitstream7`] (FASM lines + frames).
pub fn run(netlist: &NetlistIr) -> Result<Bitstream7, String> {
    run_for_part(netlist, "xc7a35t")
}

/// Same as [`run`] but for an explicit part. Only `xc7a35t` has an X-Ray
/// database wired up; anything else is a clear error, not a silent fallback.
pub fn run_for_part(netlist: &NetlistIr, part: &str) -> Result<Bitstream7, String> {
    if part != "xc7a35t" {
        return Err(format!(
            "unsupported part {part}: only xc7a35t has an X-Ray database wired up"
        ));
    }
    let mut device = Device::load_xc7a35t(&default_db_root())?;
    let node_maps = crate::xdevice::build_node_map(&mut device);
    let xdev = XDevice::new(&device, node_maps);
    let packed = pack(netlist)?;
    let placed = place(&packed, &xdev)?;
    let routed = route(&placed, &xdev)?;
    emit_fasm(&routed, &xdev)
}
