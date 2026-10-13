//! ECP5 backend for Helion: device model and flow scaffolding for the
//! LFE5U-85F, built on Project Trellis's open device database.
//!
//! Stage pipeline: pack → place → route → bitstream, wired through the
//! [`backend::Ecp5Backend`] implementation of [`helion_backend::Backend`].
//! The provided [`helion_backend::Backend::run`] driver calls the stages in
//! order; everything a stage needs lives in the backend (device model,
//! options, caches), so there is no side-channel flow module.
//!
//! Module map:
//! - [`db`]: Trellis JSON parsing (`tilegrid.json`, `iodb.json`,
//!   `globals.json`) and the `bits.db` routing/config parser.
//! - [`tiledata`]: registry of the vendored per-tile-type `bits.db` texts.
//! - [`device`]: the [`device::Device`] model: tile grid, sites, resource
//!   counts, package pins, clocking, and the routing-graph node inventory.
//! - [`place`]: the [`place::PlacedEcp5`] type and [`place::place`]: site
//!   assignment on the Trellis tile grid (IOs to package pins, EBR/DSP/PLL
//!   to hard-macro bels, carry chains to adjacent slices, LUT/FF logic by
//!   seeded simulated annealing, clock nets to DCC globals).
//! - [`backend`]: [`backend::Ecp5Backend`], wiring the
//!   [`helion_backend::Backend`] trait to the real stage types
//!   ([`pack::PackedEcp5`], [`place::PlacedEcp5`], [`route::RoutedEcp5`],
//!   [`textcfg::BitstreamEcp5`]).
//!
//! Data strategy: the Trellis JSON (~9.4MB) and all 148 tile types'
//! `bits.db` (~2.1MB) are vendored under `data/` and embedded with
//! `include_str!`, parsed once into [`device::Device`] (see
//! [`device::Device::global`]). This follows the helion-ice40 precedent
//! (7.1MB IceStorm DB embedded the same way): the crate stays
//! self-contained with no runtime file dependencies, at the cost of ~12MB
//! of binary size, acceptable for a CAD tool.

pub mod backend;
pub mod db;
pub mod device;
pub mod pack;
pub mod place;
pub mod route;
pub mod textcfg;
pub mod tiledata;

pub use backend::Ecp5Backend;
pub use device::Device;
/// The packed type; see [`pack::PackedEcp5`].
pub use pack::PackedEcp5;
/// The placed type; see [`place::PlacedEcp5`].
pub use place::PlacedEcp5;
/// The routed type; see [`route::RoutedEcp5`].
pub use route::RoutedEcp5;
/// The bitstream type; see [`textcfg::BitstreamEcp5`].
pub use textcfg::BitstreamEcp5;
