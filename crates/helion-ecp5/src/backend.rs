//! [`Ecp5Backend`] and its [`helion_backend::Backend`] implementation.
//!
//! The backend owns its device model ([`crate::device::Device`]), loaded by
//! [`Ecp5Backend::open`]; the trait stage methods need no side-channel
//! driver. Everything a stage needs lives in `self`.
//!
//! Stage status (v1 skeleton): the stage *types* below are scaffolding that
//! the pack/place/route/bitstream workers will replace with real shapes, and
//! the stage *methods* fail loudly until those workers land. Each is marked
//! `SCAFFOLD`.

use crate::device::Device;
use helion_backend::{Backend, NetlistIr, TimingModel};

// SCAFFOLD (pack worker): replace with the real packed representation
// (LUT4/FF slice packing, carry chains, EBR/DSP inference, IO packing).
/// Packed netlist: cells grouped into ECP5 sites.
#[derive(Clone, Debug, Default)]
pub struct PackedEcp5 {
    /// Top module name, carried through the stages.
    pub top: String,
}

// SCAFFOLD (place worker): replace with the real placed representation
// (site assignments on the tile grid).
/// Placed packed netlist: a device site chosen for each packed element.
#[derive(Clone, Debug, Default)]
pub struct PlacedEcp5 {
    /// Top module name, carried through the stages.
    pub top: String,
}

// SCAFFOLD (route worker): replace with the real routed representation
// (per-net routing resource assignments from the Trellis mux graph).
/// Routed placed netlist: nets assigned to routing resources.
#[derive(Clone, Debug, Default)]
pub struct RoutedEcp5 {
    /// Top module name, carried through the stages.
    pub top: String,
}

// SCAFFOLD (bitstream worker): fill with the real Trellis text config.
// The interchange format is Trellis text configuration (`.config` text),
// the same format `nextpnr-ecp5 --textcfg` emits and `ecppack` consumes,
// so Helion output stays diffable against the reference flow.
/// Final bitstream in Trellis text-configuration form.
#[derive(Clone, Debug, Default)]
pub struct BitstreamEcp5 {
    /// Trellis text configuration.
    pub textcfg: String,
}

/// ECP5 backend. Currently targets the LFE5U-85F (`"LFE5U-85F"`).
#[derive(Debug)]
pub struct Ecp5Backend {
    part: String,
    device: Device,
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
        Ok(Self { part, device })
    }

    /// Borrow the owned device model.
    pub fn device(&self) -> &Device {
        &self.device
    }
}

impl Backend for Ecp5Backend {
    type Packed = PackedEcp5;
    type Placed = PlacedEcp5;
    type Routed = RoutedEcp5;
    type Bitstream = BitstreamEcp5;

    fn name(&self) -> &str {
        "ecp5"
    }

    fn device_id(&self) -> &str {
        &self.part
    }

    // SCAFFOLD (pack worker): implement LUT4/FF packing into slices.
    fn pack(&self, netlist: &NetlistIr) -> Result<Self::Packed, String> {
        let _ = netlist;
        Err("not yet implemented: ECP5 pack worker has not landed".to_string())
    }

    // SCAFFOLD (place worker): implement site assignment on the tile grid.
    fn place(&self, packed: &Self::Packed) -> Result<Self::Placed, String> {
        let _ = packed;
        Err("not yet implemented: ECP5 place worker has not landed".to_string())
    }

    // SCAFFOLD (route worker): implement routing on the Trellis mux graph.
    fn route(&self, placed: &Self::Placed) -> Result<Self::Routed, String> {
        let _ = placed;
        Err("not yet implemented: ECP5 route worker has not landed".to_string())
    }

    // SCAFFOLD (bitstream worker): implement Trellis textcfg emission.
    fn bitstream(&self, routed: &Self::Routed) -> Result<Self::Bitstream, String> {
        let _ = routed;
        Err("not yet implemented: ECP5 bitstream worker has not landed".to_string())
    }

    fn timing_model(&self) -> TimingModel {
        TimingModel::ecp5_85f()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_stage_types<
        B: Backend<
            Packed = PackedEcp5,
            Placed = PlacedEcp5,
            Routed = RoutedEcp5,
            Bitstream = BitstreamEcp5,
        >,
    >() {
    }

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
    fn stage_types_compile() {
        assert_stage_types::<Ecp5Backend>();
    }

    #[test]
    fn device_model_is_owned() {
        let b = Ecp5Backend::open("LFE5U-85F").unwrap();
        assert_eq!(b.device().counts().pfu_tiles, 10455);
    }

    #[test]
    fn stages_fail_loud_until_workers_land() {
        let b = Ecp5Backend::open("LFE5U-85F").unwrap();
        let nl = NetlistIr::new("top");
        let e = b.pack(&nl).unwrap_err();
        assert!(e.contains("not yet implemented"));
        let e = b.place(&PackedEcp5::default()).unwrap_err();
        assert!(e.contains("not yet implemented"));
        let e = b.route(&PlacedEcp5::default()).unwrap_err();
        assert!(e.contains("not yet implemented"));
        let e = b.bitstream(&RoutedEcp5::default()).unwrap_err();
        assert!(e.contains("not yet implemented"));
        // The provided run() driver propagates the failure instead of
        // silently succeeding.
        assert!(b.run(&nl).is_err());
    }

    #[test]
    fn timing_model_is_ecp5_85f() {
        let b = Ecp5Backend::open("LFE5U-85F").unwrap();
        let m = b.timing_model();
        assert_eq!(m, TimingModel::ecp5_85f());
        assert!(m.lut_ps > 0 && m.iob_ps > m.lut_ps);
    }
}
