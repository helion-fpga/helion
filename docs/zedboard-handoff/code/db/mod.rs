//! Project X-Ray database loading: raw file readers and JSON loaders.
//!
//! The raw database lives outside git (see `HELION_XILINX_DB`, defaulting to
//! `~/workspace/xilinx-dbsrc`). [`parse`] holds the line-oriented text parsers
//! for the `.db` files and `package_pins.csv`; [`load`] holds the typed
//! `serde_json` loaders for `tilegrid.json`, `tile_type_*.json`,
//! `tileconn.json` and `part.json`.

pub mod load;
pub mod parse;
