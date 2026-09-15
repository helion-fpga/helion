//! Lab profile: program + STAT via sim cable. Must not depend on pack/place/route/map.
//!
//! Empty bitstream is refused (never DONE=1). Overlay programs a real bitstream,
//! `step_user`s the sim fabric, and samples LED — labeled **overlay**, not board DONE.
//! Native/auto lab wrappers attempt board cables on this host; with USB=0 they Err
//! honestly (no DONE=1 / no board DONE claim).
//! Lab `lab_program_native` never soft-succeeds via OFL fallback (CLI may still
//! NotImplemented→OFL for `helion-prog --cable native`).

use helion_bits::Bitstream;
use helion_device::Device;
use helion_hw::{
    overlay_program_led, program_hbits_with_cable, refuse_empty_bitstream, resolve_cable,
    usb_native_feature_enabled, CableBackend, OverlayReport, COUNTER_OVERLAY_LED,
};
use std::path::Path;

/// Lab path: empty bitstream is always Err. Never reports DONE=1 on empty.
pub fn lab_program_empty() -> Result<String, String> {
    let dev = Device::load_part("HL10T-C32-1")?;
    // Bitstream::empty always fails refuse_empty_bitstream — single Err path, no dead Ok arm.
    Err(refuse_empty_bitstream(&Bitstream::empty(&dev))
        .err()
        .map(|e| format!("lab: {e}"))
        .unwrap_or_else(|| {
            "lab: empty bitstream refused (no configured frames) — refusing DONE on empty".into()
        }))
}

/// Overlay: real bitstream + `step_user` + LED sample. Not board DONE.
pub fn lab_overlay(bits: &Bitstream) -> Result<OverlayReport, String> {
    let dev = Device::load_part("HL10T-C32-1")?;
    overlay_program_led(&dev, bits, 16)
}

pub fn lab_overlay_line(bits: &Bitstream) -> Result<String, String> {
    Ok(lab_overlay(bits)?.summary_line())
}

/// Honesty gate for lab cable programs: 0-byte, empty-frame HBIT, or clearly
/// bogus all-zero payloads are Err — never invent STAT / DONE=1.
fn lab_gate_bitstream_file(path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("lab: read {}: {e}", path.display()))?;
    if bytes.is_empty() {
        return Err("lab: empty bitstream (0 bytes) — refusing DONE on empty".into());
    }
    if bytes.iter().all(|&b| b == 0) {
        return Err(
            "lab: bogus all-zero bitstream refused (no configured frames) — no STAT invented"
                .into(),
        );
    }
    if bytes.starts_with(b"HBIT") {
        let bits = Bitstream::from_packets(&bytes).map_err(|e| format!("lab: {e}"))?;
        refuse_empty_bitstream(&bits).map_err(|e| format!("lab: {e}"))?;
    }
    Ok(())
}

fn lab_program_cable(spec: &str, path: &Path, expect: CableBackend) -> Result<String, String> {
    lab_gate_bitstream_file(path)?;
    let dev = Device::load_part("HL10T-C32-1")?;
    let cable = resolve_cable(spec).map_err(|e| format!("lab: {e}"))?;
    if cable.backend != expect {
        return Err(format!(
            "lab: cable {spec:?} expected {:?}, got {:?} ({})",
            expect, cable.backend, cable.id
        ));
    }
    // (A) Lab native: fail fast when usb-native is off — never spawn OFL soft-fallback.
    if expect == CableBackend::NativeUsb && !usb_native_feature_enabled() {
        return Err(
            "lab: native requires --features usb-native — refusing OFL soft-fallback for lab"
                .into(),
        );
    }
    let outcome = program_hbits_with_cable(&dev, path, &cable, false)
        .map_err(|e| format!("lab: {e}"))?;
    // (B) Lab native: refuse OFL outcome even if hw soft-fallback somehow Ok'd.
    if expect == CableBackend::NativeUsb && outcome.backend() != CableBackend::NativeUsb {
        return Err(
            "lab: native path fell back to OFL — refusing soft-success".into(),
        );
    }
    Ok(outcome.summary_line("lab", &dev.part))
}

/// Attempt program via `--cable native` ([`CableBackend::NativeUsb`]).
///
/// Requires `--features usb-native` for a real native path. Lab **never** soft-succeeds
/// via OFL fallback (CLI `helion-prog --cable native` may still NotImplemented→OFL).
/// On this box with USB=0 / no FTDI session → Err. Never claims DONE=1 or board DONE
/// without a live validated native STAT path (delegates to helion-hw).
pub fn lab_program_native(path: &Path) -> Result<String, String> {
    lab_program_cable("native", path, CableBackend::NativeUsb)
}

/// Attempt program via `--cable auto` (resolves to OFL board path — never sim DONE).
///
/// With USB=0 → Err from helion-hw program path. Never invents DONE=1 / board DONE.
pub fn lab_program_auto(path: &Path) -> Result<String, String> {
    lab_program_cable("auto", path, CableBackend::OpenFpgaLoader)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_bits::bitgen;
    use helion_ir::Design;
    use helion_pack::pack;
    use helion_place::place;
    use helion_route::route;

    fn bitgen_structural_counter() -> Bitstream {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let packed = pack(&Design::structural_counter(), &dev).unwrap();
        let placed = place(&packed, &dev).unwrap();
        let routed = route(&placed, &dev).unwrap();
        bitgen(&dev, &routed).unwrap()
    }

    fn assert_no_board_done_claim(msg: &str) {
        assert!(!msg.contains("DONE=1"), "must not claim DONE=1: {msg}");
        assert!(
            !msg.to_ascii_lowercase().contains("board done=1"),
            "must not claim board DONE: {msg}"
        );
        assert!(
            !msg.to_ascii_lowercase().contains("soft-hold"),
            "must not soft-hold: {msg}"
        );
    }

    #[test]
    fn lab_has_no_synth_dep_and_refuses_empty() {
        let err = lab_program_empty().unwrap_err();
        let low = err.to_ascii_lowercase();
        assert!(
            low.contains("empty") || low.contains("refus"),
            "lab must refuse empty bitstream: {err}"
        );
        assert_no_board_done_claim(&err);
        assert!(!low.contains("stat done=1"), "{err}");
    }

    #[test]
    fn overlay_counter_blink_led_gold() {
        let bits = bitgen_structural_counter();
        assert!(!bits.frames.is_empty());

        let empty_ov = lab_overlay(&Bitstream::empty(&Device::load_part("HL10T-C32-1").unwrap()))
            .unwrap_err();
        assert!(
            empty_ov.to_ascii_lowercase().contains("empty")
                || empty_ov.to_ascii_lowercase().contains("refus"),
            "{empty_ov}"
        );
        assert_no_board_done_claim(&empty_ov);

        let r = lab_overlay(&bits).expect("overlay");
        assert_eq!(r.led, COUNTER_OVERLAY_LED, "gold LED overlay {r:?}");
        // Waveform proof: LED must change across cycles (not all-0 / all-1).
        assert!(
            r.led.contains('0') && r.led.contains('1'),
            "blink waveform must include 0 and 1: {}",
            r.led
        );
        assert_ne!(r.led, "0".repeat(r.led.len()), "LED must not be all-0");
        assert_ne!(r.led, "1".repeat(r.led.len()), "LED must not be all-1");

        let s = r.summary_line();
        assert!(s.contains("overlay"), "{s}");
        assert!(s.contains(&format!("LED={COUNTER_OVERLAY_LED}")), "{s}");
        assert!(s.contains("not board DONE"), "{s}");
        assert!(s.contains("sim_DONE="), "{s}");
        assert!(s.contains("sim_INIT="), "{s}");
        assert!(s.contains("sim_GWE="), "{s}");
        // Board-style bare DONE= must not appear; sim_DONE= is OK.
        assert!(!s.contains(" DONE="), "{s}");
        assert!(!s.to_ascii_lowercase().contains("board done=1"), "{s}");
    }

    #[test]
    fn lab_native_and_auto_refuse_without_cable() {
        let bits = bitgen_structural_counter();
        let dir = std::env::temp_dir().join("helion-lab-usb0");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("counter.hbits");
        std::fs::write(&path, &bits.packets).unwrap();

        let native_err = lab_program_native(&path).unwrap_err();
        let native_low = native_err.to_ascii_lowercase();
        assert!(
            native_low.contains("native")
                || native_low.contains("usb-native")
                || native_err.contains("no USB")
                || native_err.contains("I/O")
                || native_err.contains("no FTDI")
                || native_low.contains("ofl soft-fallback")
                || native_low.contains("refusing soft-success")
                || native_low.contains("refusing ofl"),
            "native must honest-fail (no OFL soft-success): {native_err}"
        );
        assert!(
            !native_err.contains("programmer_ok=1"),
            "lab native must not return OFL soft-success summary: {native_err}"
        );
        assert_no_board_done_claim(&native_err);

        let auto_err = lab_program_auto(&path).unwrap_err();
        assert!(
            auto_err.contains("no USB")
                || auto_err.contains("openFPGALoader")
                || auto_err.contains("programmer")
                || auto_err.to_ascii_lowercase().contains("ofl"),
            "auto USB=0 must honest-fail: {auto_err}"
        );
        assert_no_board_done_claim(&auto_err);
    }

    #[test]
    fn lab_refuses_bogus_and_empty_frames_no_stat() {
        let dir = std::env::temp_dir().join("helion-lab-bogus");
        let _ = std::fs::create_dir_all(&dir);
        let dev = Device::load_part("HL10T-C32-1").unwrap();

        let zero = dir.join("zero.hbits");
        std::fs::write(&zero, []).unwrap();
        let z = lab_program_auto(&zero).unwrap_err();
        assert!(
            z.to_ascii_lowercase().contains("empty") || z.to_ascii_lowercase().contains("refus"),
            "{z}"
        );
        assert_no_board_done_claim(&z);
        assert!(!z.to_ascii_lowercase().contains("stat=0x"), "{z}");

        let all0 = dir.join("allzero.hbits");
        std::fs::write(&all0, vec![0u8; 64]).unwrap();
        let a = lab_program_native(&all0).unwrap_err();
        assert!(
            a.to_ascii_lowercase().contains("bogus")
                || a.to_ascii_lowercase().contains("refus")
                || a.to_ascii_lowercase().contains("zero"),
            "{a}"
        );
        assert_no_board_done_claim(&a);
        assert!(!a.to_ascii_lowercase().contains("stat=0x"), "{a}");

        let empty_hbit = dir.join("empty-frames.hbits");
        std::fs::write(&empty_hbit, &Bitstream::empty(&dev).packets).unwrap();
        let e = lab_program_auto(&empty_hbit).unwrap_err();
        assert!(
            e.to_ascii_lowercase().contains("empty") || e.to_ascii_lowercase().contains("refus"),
            "{e}"
        );
        assert_no_board_done_claim(&e);
        assert!(!e.to_ascii_lowercase().contains("stat=0x"), "{e}");
    }

    #[test]
    fn lab_native_refuses_ofl_fallback_no_soft_success() {
        let bits = bitgen_structural_counter();
        let dir = std::env::temp_dir().join("helion-lab-native-no-ofl");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("counter.hbits");
        std::fs::write(&path, &bits.packets).unwrap();

        let err = lab_program_native(&path).unwrap_err();
        let low = err.to_ascii_lowercase();
        if !usb_native_feature_enabled() {
            assert!(
                low.contains("usb-native")
                    || low.contains("ofl soft-fallback")
                    || low.contains("refusing ofl"),
                "feature off → lab must refuse before OFL: {err}"
            );
        } else {
            // Feature on + no FTDI → Io / no device; still never Ok / never OFL summary.
            assert!(
                err.contains("I/O")
                    || err.contains("no FTDI")
                    || low.contains("native")
                    || low.contains("refusing soft-success"),
                "feature on + no FTDI must Err: {err}"
            );
        }
        assert!(!err.contains("programmer_ok=1"), "{err}");
        assert_no_board_done_claim(&err);
    }
}
