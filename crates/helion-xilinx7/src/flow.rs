//! End-to-end flow orchestration for the Xilinx 7-series backend.
//!
//! [`run`] is a thin compatibility shim over the [`helion_backend::Backend`]
//! trait: it opens an [`crate::backend::Xilinx7Backend`] and drives it with
//! [`helion_backend::Backend::run`]. All stage logic lives in the trait
//! implementation; there is no side-channel driver.

use crate::backend::Xilinx7Backend;
use crate::emit::Bitstream7;
use helion_backend::{Backend, NetlistIr};

/// Run the full 7-series flow on `netlist` for the default part (`xc7a35t`),
/// returning the emitted [`Bitstream7`] (FASM lines + frames).
pub fn run(netlist: &NetlistIr) -> Result<Bitstream7, String> {
    run_for_part(netlist, "xc7a35t")
}

/// Same as [`run`] but for an explicit part. Only `xc7a35t` has an X-Ray
/// database wired up; anything else is a clear error, not a silent fallback.
pub fn run_for_part(netlist: &NetlistIr, part: &str) -> Result<Bitstream7, String> {
    Xilinx7Backend::open(part)?.run(netlist)
}
