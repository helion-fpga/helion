//! Lab profile: program + STAT via sim cable. Must not depend on pack/place/route/map.
//!
//! Empty bitstream is refused (never DONE=1). Overlay programs a real bitstream,
//! `step_user`s the sim fabric, and samples LED — labeled **overlay**, not board DONE.

use helion_bits::Bitstream;
use helion_device::Device;
use helion_hw::{overlay_program_led, refuse_empty_bitstream, OverlayReport, COUNTER_OVERLAY_LED};

/// Lab path: empty bitstream is always Err. Never reports DONE=1 on empty.
pub fn lab_program_empty() -> Result<String, String> {
    let dev = Device::load_part("HL10T-C32-1")?;
    refuse_empty_bitstream(&Bitstream::empty(&dev))?;
    Err("lab: empty bitstream refused (no configured frames) — refusing DONE on empty".into())
}

/// Overlay: real bitstream + `step_user` + LED sample. Not board DONE.
pub fn lab_overlay(bits: &Bitstream) -> Result<OverlayReport, String> {
    let dev = Device::load_part("HL10T-C32-1")?;
    overlay_program_led(&dev, bits, 16)
}

pub fn lab_overlay_line(bits: &Bitstream) -> Result<String, String> {
    Ok(lab_overlay(bits)?.summary_line())
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_bits::bitgen;
    use helion_ir::Design;
    use helion_pack::pack;
    use helion_place::place;
    use helion_route::route;

    #[test]
    fn lab_has_no_synth_dep_and_refuses_empty() {
        let err = lab_program_empty().unwrap_err();
        let low = err.to_ascii_lowercase();
        assert!(
            low.contains("empty") || low.contains("refus"),
            "lab must refuse empty bitstream: {err}"
        );
        assert!(
            !err.contains("DONE=1") && !err.contains("STAT DONE=1"),
            "must not claim DONE on empty: {err}"
        );
        assert!(!low.contains("stat done=1"), "{err}");
    }

    #[test]
    fn overlay_counter_blink_led_gold() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let packed = pack(&Design::structural_counter(), &dev).unwrap();
        let placed = place(&packed, &dev).unwrap();
        let routed = route(&placed, &dev).unwrap();
        let bits = bitgen(&dev, &routed).unwrap();
        assert!(!bits.frames.is_empty());

        let empty_ov = lab_overlay(&Bitstream::empty(&dev)).unwrap_err();
        assert!(
            empty_ov.to_ascii_lowercase().contains("empty")
                || empty_ov.to_ascii_lowercase().contains("refus"),
            "{empty_ov}"
        );
        assert!(!empty_ov.contains("DONE=1"), "{empty_ov}");

        let r = lab_overlay(&bits).expect("overlay");
        assert_eq!(r.led, COUNTER_OVERLAY_LED, "gold LED overlay {r:?}");
        let s = r.summary_line();
        assert!(s.contains("overlay"), "{s}");
        assert!(s.contains(&format!("LED={COUNTER_OVERLAY_LED}")), "{s}");
        assert!(s.contains("not board DONE"), "{s}");
        assert!(!s.to_ascii_lowercase().contains("board done=1"), "{s}");
    }
}
