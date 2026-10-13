//! iCE40 backend for Helion: pack, place, route, and bitstream emission for
//! the iCE40-HX8K, built on Project IceStorm's open device database.
//!
//! Stage pipeline: [`pack`] → [`place`] → [`route`] → [`asc`], wired through
//! the [`backend::Ice40Backend`] implementation of [`helion_backend::Backend`].
//! [`flow`] is the thin driver.

pub mod asc;
pub mod backend;
pub mod db;
pub mod device;
pub mod flow;
pub mod gclk;
pub mod pack;
pub mod place;
pub mod route;

pub use backend::Ice40Backend;
pub use flow::{run, run_for_part};
