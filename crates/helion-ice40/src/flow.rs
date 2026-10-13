//! Thin driver for the iCE40 backend.
//!
//! The [`helion_backend::Backend`] trait owns the flow now
//! ([`Backend::run`](helion_backend::Backend::run) drives
//! pack → place → route → bitstream). These helpers are convenience shims
//! over it so callers do not need to construct [`Ice40Backend`] themselves.

use crate::backend::{BitstreamIce, Ice40Backend};
use helion_backend::{Backend, NetlistIr};

/// Run the full flow for `part` (e.g. `"ice40-hx8k"`), returning `.asc` text.
pub fn run_for_part(part: &str, netlist: &NetlistIr) -> Result<BitstreamIce, String> {
    Ice40Backend::new(part).run(netlist)
}

/// Run the full flow for the default HX8K target.
pub fn run(netlist: &NetlistIr) -> Result<BitstreamIce, String> {
    run_for_part("ice40-hx8k", netlist)
}
