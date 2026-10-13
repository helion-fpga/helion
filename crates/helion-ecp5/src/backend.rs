//! [`Ecp5Backend`] and its [`helion_backend::Backend`] implementation.
//!
//! The backend owns its device model ([`crate::device::Device`]), loaded by
//! [`Ecp5Backend::open`]; the trait stage methods need no side-channel
//! driver. Everything a stage needs lives in `self`.
//!
//! Stage wiring: [`crate::pack::pack`] -> [`crate::place::place`] ->
//! [`crate::route::route_on_device`] -> [`crate::textcfg::emit_textcfg`].
//! The [`helion_backend::Backend`] trait threads only the routed design
//! into `bitstream()`; the placed design (needed for the textcfg site
//! payloads) is cached at `place()` time. Calling `bitstream()` without a
//! preceding `place()` fails loudly.

use crate::device::Device;
use helion_backend::{Backend, NetlistIr, TimingModel};
use std::cell::RefCell;

/// ECP5 backend. Currently targets the LFE5U-85F (`"LFE5U-85F"`).
#[derive(Debug)]
pub struct Ecp5Backend {
    part: String,
    device: Device,
    /// The placed design from the last `place()` call, for `bitstream()`'s
    /// textcfg payload mapping. The `Backend` trait does not thread it
    /// through; `run()` always calls `place()` before `bitstream()`.
    placed_cache: RefCell<Option<crate::place::PlacedEcp5>>,
}

impl Ecp5Backend {
    /// Open the backend for `part`, parsing the vendored Trellis database.
    ///
    /// Only parts with a vendored database are accepted; anything else is a
    /// clear error, not a silent fallback. Currently: `"LFE5U-85F"`.
    pub fn open(part: impl Into<String>) -> Result<Self, String> {
        let part = part.into();
        if part != "LFE5U-85F" {
            return Err(format!(
                "unsupported part {part}: only LFE5U-85F has a Trellis database vendored"
            ));
        }
        let device = Device::open()?;
        Ok(Self {
            part,
            device,
            placed_cache: RefCell::new(None),
        })
    }

    /// Borrow the owned device model.
    pub fn device(&self) -> &Device {
        &self.device
    }
}

impl Backend for Ecp5Backend {
    type Packed = crate::pack::PackedEcp5;
    type Placed = crate::place::PlacedEcp5;
    type Routed = crate::route::RoutedEcp5;
    type Bitstream = crate::textcfg::BitstreamEcp5;

    fn name(&self) -> &str {
        "ecp5"
    }

    fn device_id(&self) -> &str {
        &self.part
    }

    fn pack(&self, netlist: &NetlistIr) -> Result<Self::Packed, String> {
        crate::pack::pack(netlist)
    }

    fn place(&self, packed: &Self::Packed) -> Result<Self::Placed, String> {
        let placed = crate::place::place(self.device(), packed)?;
        self.placed_cache.replace(Some(placed.clone()));
        Ok(placed)
    }

    fn route(&self, placed: &Self::Placed) -> Result<Self::Routed, String> {
        crate::route::route_on_device(self.device(), placed)
    }

    fn bitstream(&self, routed: &Self::Routed) -> Result<Self::Bitstream, String> {
        let cache = self.placed_cache.borrow();
        let placed = cache.as_ref().ok_or_else(|| {
            "ecp5 bitstream: no placed design cached (call place() before bitstream())".to_string()
        })?;
        crate::textcfg::emit_textcfg(routed, placed, self.device())
    }

    fn timing_model(&self) -> TimingModel {
        TimingModel::ecp5_85f()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{pack, place, route, textcfg};

    #[test]
    fn name_is_ecp5() {
        let b = Ecp5Backend::open("LFE5U-85F").unwrap();
        assert_eq!(b.name(), "ecp5");
    }

    #[test]
    fn device_id_mentions_part() {
        let b = Ecp5Backend::open("LFE5U-85F").unwrap();
        assert!(b.device_id().contains("LFE5U-85F"));
    }

    #[test]
    fn open_rejects_unknown_part() {
        assert!(Ecp5Backend::open("LFE5U-45F").is_err());
        assert!(Ecp5Backend::open("").is_err());
    }

    #[test]
    fn device_model_is_owned() {
        let b = Ecp5Backend::open("LFE5U-85F").unwrap();
        assert_eq!(b.device().counts().pfu_tiles, 10455);
    }

    #[test]
    fn stage_types_are_the_real_ones() {
        fn assert_types<B: Backend<Packed = pack::PackedEcp5, Placed = place::PlacedEcp5, Routed = route::RoutedEcp5, Bitstream = textcfg::BitstreamEcp5>>() {
        }
        assert_types::<Ecp5Backend>();
    }

    #[test]
    fn bitstream_without_place_fails_loud() {
        let b = Ecp5Backend::open("LFE5U-85F").unwrap();
        let routed = route::RoutedEcp5::default();
        let err = b.bitstream(&routed).unwrap_err();
        assert!(err.contains("place()"), "unexpected error: {err}");
    }

    #[test]
    fn timing_model_is_ecp5_85f() {
        let b = Ecp5Backend::open("LFE5U-85F").unwrap();
        let m = b.timing_model();
        assert_eq!(m, TimingModel::ecp5_85f());
        assert!(m.lut_ps > 0 && m.iob_ps > m.lut_ps);
    }
}
