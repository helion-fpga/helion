//! Thin driver for the iCE40 backend: pack → place → route → bitstream.
//!
//! Kept deliberately small so it migrates trivially once the `Backend`
//! trait hardens. All device knowledge lives in the stage modules.

use crate::backend::{BitstreamIce, Ice40Backend};
use helion_backend::{Backend, NetlistIr};

/// Run the full flow for `part` (e.g. `"ice40-hx8k"`), returning `.asc` text.
pub fn run_for_part(part: &str, netlist: &NetlistIr) -> Result<BitstreamIce, String> {
    let backend = Ice40Backend::new(part);
    let packed = backend.pack(netlist)?;
    let placed = backend.place(&packed)?;
    let routed = backend.route(&placed)?;
    backend.bitstream(&routed)
}

/// Run the full flow for the default HX8K target.
pub fn run(netlist: &NetlistIr) -> Result<BitstreamIce, String> {
    run_for_part("ice40-hx8k", netlist)
}
