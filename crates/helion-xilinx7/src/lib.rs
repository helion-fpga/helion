//! Xilinx 7-series backend for Helion (`xc7a35t` and friends).
//!
//! **Open-bitstream-databases-only policy.** This backend targets Xilinx
//! 7-series parts using only publicly documented, open bitstream databases
//! (Project X-Ray / prjxray). No vendor tools (Vivado, ISE), no vendor
//! bitstream documentation, and no vendor proprietary data are used or
//! required anywhere in this crate. The output path is FASM text
//! (assembled to bits by open tooling), keeping the whole flow auditable.
//!
//! The flow stages live in [`backend`] ([`backend::Xilinx7Backend`] implements
//! [`helion_backend::Backend`] with Xilinx-shaped associated types), and the
//! end-to-end driver lives in [`flow`]. The stage modules
//! ([`pack`], [`place`], [`route`], [`emit`]) are coordinator-owned: they
//! currently hold placeholder stage types and will grow the real flow logic.

pub mod backend;
pub mod bitstream;
pub mod db;
pub mod device;
pub mod emit;
pub mod fasm;
pub mod flow;
pub mod frames;
pub mod pack;
pub mod place;
pub mod route;
pub mod xdevice;

pub use backend::{Bitstream7, Packed7, Placed7, Routed7, RoutedNet7, Xilinx7Backend};
pub use device::Device;
pub use emit::emit_fasm;
pub use flow::run;
pub use pack::pack;
pub use place::place;
pub use route::route;
