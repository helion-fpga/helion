//! Lab profile: honesty-facing wrappers over [`helion_hw`] — empty refuse, **overlay**
//! LED sample, and native/auto cable programs (not only "sim cable STAT").
//!
//! Runtime deps stay thin (`helion-hw` / `helion-device` / `helion-bits`). Pack /
//! place / route are **dev-dependencies** for overlay gold tests only — not crate
//! runtime deps (lab must not pull the implement stack into products).
//!
//! Empty bitstream is refused (never DONE=1). Overlay programs a real bitstream,
//! `step_user`s the sim fabric, and samples LED — labeled **overlay**, not board DONE.
//! Native/auto lab wrappers attempt board cables on this host; with USB=0 they Err
//! honestly (no DONE=1 / no board DONE claim).
//! Lab `lab_program_native` never soft-succeeds via OFL fallback (CLI may still
//! NotImplemented→OFL for `helion-prog --cable native`).
//!
//! Stable refuse codes (re-export Bits/hw locked tokens):
//! [`ERR_CODE_EMPTY_BITSTREAM`] / [`ERR_CODE_USB_0`].

use helion_bits::Bitstream;
use helion_device::Device;
use helion_hw::{
    CableBackend, OverlayReport, overlay_program_led, program_hbits_with_cable,
    refuse_empty_bitstream, resolve_cable, usb_native_feature_enabled,
};
use std::path::Path;

pub mod ibert;
pub mod sysmon;

pub use ibert::{
    BathtubCurve, BathtubPoint, EyeMetrics, IbertEyeScan, IbertLinkConfig, PrbsEngine,
    PrbsPattern, erfc_approx, q_tail, run_ibert_eye_scan, sweep_bathtub,
};
pub use sysmon::{
    SysmonAlarms, SysmonMonitor, SysmonRailReading, SysmonTelemetry, adc_to_temp_c,
    adc_to_voltage, drp_addr, temp_c_to_adc, voltage_to_adc,
};

pub mod bscan_1149;
pub mod svf_stapl;

pub use bscan_1149::{
    BoardFault, BoardNet, BoardPinRef, BoundaryScanBoard, BsdlCell, BsdlCellKind, BsdlDevice,
    DiagnosedFault, InterconnectTestReport, parse_bsdl,
};
pub use svf_stapl::{
    SvfCommand, SvfExecutionReport, execute_hw_stapl, execute_hw_svf, parse_svf, write_hw_stapl,
    write_hw_svf,
};

pub mod axi_pcie_analyzer;

pub use axi_pcie_analyzer::{
    AxiBurstKind, AxiBurstTxn, AxiPerfMonitor, AxiPerfReport, AxiProtocolChecker,
    AxiProtocolViolation, AxiTrafficGenConfig, AxiTrafficGenerator, AxiTrafficPattern,
    AxiViolationKind, AxiWriteChannelSample, LatencyStats, PcieGenSpeed, PcieLinkIssue,
    PcieLtssmAnalyzer, PcieLtssmReport, PcieLtssmState, PcieTraceSample,
};

/// Locked Bits/hw refuse codes — re-exported (identical `HELION_E_*` tokens; do not rename).
pub use helion_hw::{ERR_CODE_EMPTY_BITSTREAM, ERR_CODE_USB_0};

fn ensure_err_code(code: &str, msg: impl AsRef<str>) -> String {
    let m = msg.as_ref();
    if m.contains(code) {
        m.to_string()
    } else {
        format!("{code}: {m}")
    }
}

fn lab_empty_err(msg: impl AsRef<str>) -> String {
    ensure_err_code(ERR_CODE_EMPTY_BITSTREAM, msg)
}

fn lab_usb0_err(msg: impl AsRef<str>) -> String {
    ensure_err_code(ERR_CODE_USB_0, msg)
}

/// Lab path: empty bitstream is always Err. Never reports DONE=1 on empty.
pub fn lab_program_empty() -> Result<String, String> {
    let dev = Device::load_part("HL10T-C32-1")?;
    // Bitstream::empty always fails refuse_empty_bitstream — single Err path, no dead Ok arm.
    Err(lab_empty_err(
        refuse_empty_bitstream(&Bitstream::empty(&dev))
            .err()
            .map(|e| format!("lab: {e}"))
            .unwrap_or_else(|| {
                "lab: empty bitstream refused (no configured frames) — refusing DONE on empty"
                    .into()
            }),
    ))
}

/// Overlay: real bitstream + `step_user` + LED sample. Not board DONE.
pub fn lab_overlay(bits: &Bitstream) -> Result<OverlayReport, String> {
    let dev = Device::load_part("HL10T-C32-1")?;
    overlay_program_led(&dev, bits, 16).map_err(|e| {
        let low = e.to_ascii_lowercase();
        if low.contains("empty") || low.contains("refus") {
            lab_empty_err(format!("lab: {e}"))
        } else {
            format!("lab: {e}")
        }
    })
}

pub fn lab_overlay_line(bits: &Bitstream) -> Result<String, String> {
    Ok(lab_overlay(bits)?.summary_line())
}

/// Honesty gate for lab cable programs: 0-byte, empty-frame HBIT, or clearly
/// bogus all-zero payloads are Err — never invent STAT / DONE=1.
fn lab_gate_bitstream_file(path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("lab: read {}: {e}", path.display()))?;
    if bytes.is_empty() {
        return Err(lab_empty_err(
            "lab: empty bitstream (0 bytes) — refusing DONE on empty",
        ));
    }
    if bytes.iter().all(|&b| b == 0) {
        return Err(lab_empty_err(
            "lab: bogus all-zero bitstream refused (no configured frames) — no STAT invented",
        ));
    }
    if bytes.starts_with(b"HBIT") {
        let bits = Bitstream::from_packets(&bytes).map_err(|e| format!("lab: {e}"))?;
        refuse_empty_bitstream(&bits).map_err(|e| lab_empty_err(format!("lab: {e}")))?;
    }
    Ok(())
}

fn lab_program_cable(spec: &str, path: &Path, expect: CableBackend) -> Result<String, String> {
    lab_gate_bitstream_file(path)?;
    let dev = Device::load_part("HL10T-C32-1")?;
    let cable = resolve_cable(spec).map_err(|e| lab_usb0_err(format!("lab: {e}")))?;
    if cable.backend != expect {
        return Err(lab_usb0_err(format!(
            "lab: cable {spec:?} expected {:?}, got {:?} ({})",
            expect, cable.backend, cable.id
        )));
    }
    // (A) Lab native: fail fast when usb-native is off — never spawn OFL soft-fallback.
    if expect == CableBackend::NativeUsb && !usb_native_feature_enabled() {
        return Err(lab_usb0_err(
            "lab: native requires --features usb-native — refusing OFL soft-fallback for lab",
        ));
    }
    let outcome = program_hbits_with_cable(&dev, path, &cable, false)
        .map_err(|e| lab_usb0_err(format!("lab: {e}")))?;
    // (B) Lab native: refuse OFL outcome even if hw soft-fallback somehow Ok'd.
    if expect == CableBackend::NativeUsb && outcome.backend() != CableBackend::NativeUsb {
        return Err(lab_usb0_err(
            "lab: native path fell back to OFL — refusing soft-success",
        ));
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
    use helion_hw::COUNTER_OVERLAY_LED;
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
            err.contains(ERR_CODE_EMPTY_BITSTREAM),
            "stable empty code missing: {err}"
        );
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

        let empty_ov = lab_overlay(&Bitstream::empty(
            &Device::load_part("HL10T-C32-1").unwrap(),
        ))
        .unwrap_err();
        assert!(
            empty_ov.contains(ERR_CODE_EMPTY_BITSTREAM),
            "stable empty code missing: {empty_ov}"
        );
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
            native_err.contains(ERR_CODE_USB_0),
            "stable USB=0 code missing on native: {native_err}"
        );
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
            auto_err.contains(ERR_CODE_USB_0),
            "stable USB=0 code missing on auto: {auto_err}"
        );
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
            z.contains(ERR_CODE_EMPTY_BITSTREAM),
            "stable empty code missing: {z}"
        );
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
            a.contains(ERR_CODE_EMPTY_BITSTREAM),
            "stable empty code missing: {a}"
        );
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
            e.contains(ERR_CODE_EMPTY_BITSTREAM),
            "stable empty code missing: {e}"
        );
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
        assert!(
            err.contains(ERR_CODE_USB_0),
            "stable USB=0 code missing: {err}"
        );
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

    #[test]
    fn sysmon_xadc_transfer_functions_and_drp_alarms() {
        // UG480 transfer function round-trip checks.
        for &t_c in &[-40.0, 0.0, 25.0, 62.5, 85.0, 125.0] {
            let code = temp_c_to_adc(t_c);
            let back = adc_to_temp_c(code);
            assert!(
                (back - t_c).abs() < 0.25,
                "temp {t_c}C -> adc {code:#x} -> {back}C"
            );
        }
        for &v in &[0.95, 1.00, 1.03, 1.80, 2.50] {
            let code = voltage_to_adc(v);
            let back = adc_to_voltage(code);
            assert!(
                (back - v).abs() < 0.002,
                "volt {v}V -> adc {code:#x} -> {back}V"
            );
        }

        let mut mon = SysmonMonitor::new(25.0);
        assert!(!mon.telemetry.alarms.any());
        assert_eq!(mon.read_drp(drp_addr::ALARM_FLAGS), 0);

        // Normal operating sample: 52.0C, nominal rails.
        mon.ingest_sample(52.0, 0.998, 1.795, 1.001);
        assert!(!mon.telemetry.alarms.any());
        assert!(mon.telemetry.max_temp_c > 51.5);
        assert!(mon.telemetry.min_temp_c < 25.5);

        // High temperature (92.0C >= 85.0C user limit) + VCCINT droop (0.94V < 0.97V).
        mon.ingest_sample(92.0, 0.94, 1.80, 1.00);
        assert!(mon.telemetry.alarms.user_temp_alarm);
        assert!(!mon.telemetry.alarms.over_temp_shutdown);
        assert!(mon.telemetry.alarms.vccint_alarm);
        assert!(!mon.telemetry.alarms.vccaux_alarm);
        assert!(mon.telemetry.alarms.any());
        let flags = mon.read_drp(drp_addr::ALARM_FLAGS);
        assert_eq!(flags & 0b0011, 0b0011, "user_temp + vccint alarm bits set");

        // Over-temperature shutdown (128.0C >= 125.0C OT limit).
        mon.ingest_sample(128.0, 1.00, 1.80, 1.00);
        assert!(mon.telemetry.alarms.over_temp_shutdown);
        assert!(mon.telemetry.summary_line().contains("SYSMON TEMP="));
    }

    #[test]
    fn sysmon_bitstream_telemetry_scales_with_activity_and_refuses_empty() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut mon = SysmonMonitor::new(25.0);

        let empty_err = mon
            .sample_bitstream(&dev, &Bitstream::empty(&dev), 25.0, 0.5)
            .unwrap_err();
        assert!(empty_err.contains(ERR_CODE_EMPTY_BITSTREAM));

        let bits = bitgen_structural_counter();
        let idle = mon.sample_bitstream(&dev, &bits, 25.0, 0.0).unwrap();
        let busy = mon.sample_bitstream(&dev, &bits, 25.0, 1.5).unwrap();
        assert!(
            busy.die_temp_c > idle.die_temp_c,
            "higher toggle rate must increase die temp: busy={} idle={}",
            busy.die_temp_c,
            idle.die_temp_c
        );
        assert!(
            busy.vccint.current_v <= idle.vccint.current_v,
            "higher toggle rate must increase IR droop on VCCINT"
        );
    }

    #[test]
    fn ibert_prbs_lfsr_period_and_bathtub_eye_scan_metrics() {
        // PRBS-7 LFSR must have exact maximal period 2^7 - 1 = 127.
        let mut prbs = PrbsEngine::new(PrbsPattern::Prbs7, 0x01);
        let init_state = prbs.lfsr;
        for step in 1..=127 {
            let _ = prbs.next_bit();
            if step < 127 {
                assert_ne!(prbs.lfsr, init_state, "premature repeat at step {step}");
            } else {
                assert_eq!(prbs.lfsr, init_state, "must repeat at period 127");
            }
        }
        let ber_with_err = prbs.run_burst(10_000, 5);
        assert!(ber_with_err > 0.0 && ber_with_err < 1e-2);

        // Healthy 10.3125 Gbps link with CTLE + DFE enabled.
        let good_cfg = IbertLinkConfig::default();
        let bathtub = sweep_bathtub(&good_cfg, 41, 10_000_000_000).unwrap();
        assert!(
            bathtub.horizontal_opening_ui >= 0.45,
            "healthy link horizontal opening UI={}",
            bathtub.horizontal_opening_ui
        );
        assert!(
            bathtub.center_ber < 1e-12,
            "center BER={:e}",
            bathtub.center_ber
        );
        assert!(
            bathtub.points.first().unwrap().ber > 0.1,
            "edge at -0.5 UI must have high BER"
        );

        let scan_open = run_ibert_eye_scan(&good_cfg, 33, 21).unwrap();
        assert!(scan_open.metrics.margin_pass);
        assert!(scan_open.metrics.horizontal_opening_ui >= 0.45);
        assert!(scan_open.metrics.vertical_opening_mv >= 150.0);
        assert!(scan_open.metrics.eye_area_ui_mv > 50.0);
        let ascii = scan_open.render_ascii();
        assert!(ascii.contains("IBERT Eye Scan MGT_X0Y0"));
        assert!(ascii.contains("pass=true"));

        // Severely degraded 25 dB loss channel with equalization turned off -> smaller/closed eye.
        let mut degraded_cfg = IbertLinkConfig::default();
        degraded_cfg.channel_loss_db = 28.0;
        degraded_cfg.tx_pre_cursor_db = 0.0;
        degraded_cfg.tx_post_cursor_db = 0.0;
        degraded_cfg.rx_ctle_gain_db = 0.0;
        degraded_cfg.rx_dfe_enabled = false;
        let scan_degraded = run_ibert_eye_scan(&degraded_cfg, 33, 21).unwrap();
        assert!(
            scan_degraded.metrics.eye_area_ui_mv < scan_open.metrics.eye_area_ui_mv,
            "unequalized high-loss channel must have smaller eye area"
        );
        assert!(
            scan_degraded.metrics.vertical_opening_mv < scan_open.metrics.vertical_opening_mv
        );
    }

    #[test]
    fn write_and_execute_hw_svf_and_stapl_with_tdo_mask_verification() {
        use helion_hw::Tap;
        let dev = Device::load_part("HL10T-C32-1").unwrap();

        // Empty bitstream must be refused by write_hw_svf and write_hw_stapl.
        let empty_err = write_hw_svf(&dev, &Bitstream::empty(&dev)).unwrap_err();
        assert!(empty_err.contains(ERR_CODE_EMPTY_BITSTREAM));

        let bits = bitgen_structural_counter();
        let svf = write_hw_svf(&dev, &bits).unwrap();
        assert!(svf.contains("FREQUENCY 10000000 HZ;"));
        assert!(svf.contains("SIR 6 TDI (03);"));
        assert!(svf.contains("RUNTEST IDLE 33 TCK;"));

        let mut tap = Tap::new(&dev);
        let rep = execute_hw_svf(&mut tap, &svf).unwrap();
        assert_eq!(rep.frequency_hz, Some(10_000_000));
        assert_eq!(rep.sir_scans, 3);
        assert_eq!(rep.sdr_scans, 3);
        assert_eq!(rep.tdo_checks_passed, 2);
        assert_eq!(rep.final_idcode, dev.idcode);
        assert!(tap.fabric().stat.done && tap.fabric().stat.eos);

        // Tampered IDCODE TDO expectation must fail execute_hw_svf with TDO mismatch.
        let bad_svf = svf.replace(
            &format!("TDO ({:08x})", dev.idcode),
            "TDO (deadbeef)",
        );
        let mut tap_bad = Tap::new(&dev);
        let tdo_err = execute_hw_svf(&mut tap_bad, &bad_svf).unwrap_err();
        assert!(tdo_err.contains("SDR TDO mismatch"), "{tdo_err}");

        // STAPL export & playback round-trip.
        let stapl = write_hw_stapl(&dev, &bits).unwrap();
        assert!(stapl.contains("ACTION PROGRAM = DO_PROGRAM;"));
        assert!(stapl.contains("PROCEDURE DO_PROGRAM;"));
        let mut tap_stapl = Tap::new(&dev);
        let stapl_rep = execute_hw_stapl(&mut tap_stapl, &stapl).unwrap();
        assert_eq!(stapl_rep.tdo_checks_passed, 2);
        assert!(tap_stapl.fabric().stat.done);
    }

    #[test]
    fn bsdl_roundtrip_and_ieee_1149_1_extest_interconnect_fault_diagnosis() {
        let chip0 = BsdlDevice::helion_default("HL10T_U0", 0x0362_D093, 8);
        let chip1 = BsdlDevice::helion_default("HL10T_U1", 0x0362_D093, 8);

        // Verify BSDL text round-trip.
        let bsdl_text = chip0.to_bsdl_string();
        let parsed_chip0 = parse_bsdl(&bsdl_text).unwrap();
        assert_eq!(parsed_chip0, chip0);

        let mut board = BoundaryScanBoard::new(vec![chip0, chip1]);
        // Net 0: multi-drop from U0.IO_0 -> U1.IO_0 and U1.IO_7
        board.add_net(
            "DATA_0",
            BoardPinRef::new(0, "IO_0"),
            vec![BoardPinRef::new(1, "IO_0"), BoardPinRef::new(1, "IO_7")],
        );
        for i in 1..6 {
            board.add_net(
                format!("DATA_{i}"),
                BoardPinRef::new(0, format!("IO_{i}")),
                vec![BoardPinRef::new(1, format!("IO_{i}"))],
            );
        }

        // 1. Healthy board passes all Walking-1 / Walking-0 / Counting EXTEST vectors.
        let clean_rep = board.run_interconnect_test();
        assert!(clean_rep.pass);
        assert!(clean_rep.diagnosed_faults.is_empty());
        assert_eq!(clean_rep.total_bsr_bits_per_scan, 16);

        // 2. Inject StuckAt0 on DATA_1, StuckAt1 on DATA_2, OpenPin on U1.IO_7 (on multi-drop DATA_0),
        //    and BridgedNets (wired-AND) between DATA_3 and DATA_4.
        board.inject_fault(BoardFault::StuckAt0 {
            net_name: "DATA_1".into(),
        });
        board.inject_fault(BoardFault::StuckAt1 {
            net_name: "DATA_2".into(),
        });
        board.inject_fault(BoardFault::OpenPin {
            pin: BoardPinRef::new(1, "IO_7"),
            open_pull_value: false,
        });
        board.inject_fault(BoardFault::BridgedNets {
            net_a: "DATA_3".into(),
            net_b: "DATA_4".into(),
            wired_and: true,
        });

        let fault_rep = board.run_interconnect_test();
        assert!(!fault_rep.pass);
        assert_eq!(
            fault_rep.diagnosed_faults,
            vec![
                DiagnosedFault::BridgedShort {
                    net_a: "DATA_3".into(),
                    net_b: "DATA_4".into(),
                    wired_and: true,
                },
                DiagnosedFault::OpenReceiverPin {
                    net_name: "DATA_0".into(),
                    receiver: BoardPinRef::new(1, "IO_7"),
                    stuck_level: false,
                },
                DiagnosedFault::StuckAt0 {
                    net_name: "DATA_1".into(),
                    receiver: BoardPinRef::new(1, "IO_1"),
                },
                DiagnosedFault::StuckAt1 {
                    net_name: "DATA_2".into(),
                    receiver: BoardPinRef::new(1, "IO_2"),
                },
            ]
        );
    }

    #[test]
    fn axi_traffic_generator_and_performance_monitor_metrics() {
        for pat in [
            AxiTrafficPattern::Sequential,
            AxiTrafficPattern::Strided { stride_bytes: 512 },
            AxiTrafficPattern::Random { seed: 42 },
            AxiTrafficPattern::AddressSweep {
                low_addr: 0x4000_0000,
                high_addr: 0x4001_0000,
                step_bytes: 256,
            },
        ] {
            let mut cfg = AxiTrafficGenConfig::default();
            cfg.pattern = pat;
            cfg.burst_len = 16;
            cfg.data_width_bits = 64;
            let mut tg = AxiTrafficGenerator::new(cfg.clone());
            let txns = tg.generate_bursts(40);
            assert_eq!(txns.len(), 40);

            // Verify no generated burst crosses a 4KB boundary.
            let mut checker = AxiProtocolChecker::default();
            for t in &txns {
                checker.check_address_phase(t);
            }
            assert!(
                checker.violations.is_empty(),
                "generated traffic must be 4KB boundary safe: {:?}",
                checker.violations
            );

            let mut mon = AxiPerfMonitor::new();
            mon.record_batch(txns);
            let rep = mon.analyze(cfg.clock_freq_mhz, cfg.data_width_bits);
            assert!(rep.total_throughput_mbps > 100.0);
            assert!(rep.bus_utilization_pct > 10.0 && rep.bus_utilization_pct <= 100.0);
            assert!(rep.read_latency.min_cycles <= rep.read_latency.p50_cycles);
            assert!(rep.read_latency.p50_cycles <= rep.read_latency.p99_cycles);
            assert!(rep.read_latency.p99_cycles <= rep.read_latency.max_cycles);
            assert_eq!(rep.peak_outstanding, 8);
        }
    }

    #[test]
    fn axi_protocol_checker_detects_pg101_violations() {
        let mut checker = AxiProtocolChecker::new(4);

        // 1. 4KB boundary crossing: start at 0x4000_0FF0 with 4 beats x 8B = 32B -> crosses 0x4000_1000.
        let bad_4k = AxiBurstTxn {
            id: 1,
            is_write: true,
            addr: 0x4000_0FF0,
            beats: 4,
            beat_bytes: 8,
            burst_kind: AxiBurstKind::Incr,
            exclusive: false,
            issue_cycle: 10,
            complete_cycle: 20,
            outstanding_at_issue: 1,
        };
        checker.check_address_phase(&bad_4k);
        assert!(
            checker
                .violations
                .iter()
                .any(|v| v.kind == AxiViolationKind::AwaddrBoundary4K)
        );

        // 2. Invalid exclusive burst (> 16 beats).
        let mut bad_excl = bad_4k.clone();
        bad_excl.addr = 0x4000_0000;
        bad_excl.beats = 32;
        bad_excl.exclusive = true;
        checker.check_address_phase(&bad_excl);
        assert!(
            checker
                .violations
                .iter()
                .any(|v| v.kind == AxiViolationKind::ExclAccessInvalid)
        );

        // 3. Unstable WDATA while stalled (WVALID=1, WREADY=0), MAXWAITS deadlock, and WLAST beat mismatch.
        let stream = vec![
            AxiWriteChannelSample {
                cycle: 1,
                wvalid: true,
                wready: false,
                wdata: 0x1111,
                wstrb: 0xFF,
                wlast: false,
            },
            AxiWriteChannelSample {
                cycle: 2,
                wvalid: true,
                wready: false,
                wdata: 0x2222, // Mutated while WREADY=0 -> AXI_ERRM_WDATA_STABLE!
                wstrb: 0xFF,
                wlast: false,
            },
            AxiWriteChannelSample {
                cycle: 3,
                wvalid: true,
                wready: false,
                wdata: 0x2222,
                wstrb: 0xFF,
                wlast: false,
            },
            AxiWriteChannelSample {
                cycle: 4,
                wvalid: true,
                wready: false,
                wdata: 0x2222,
                wstrb: 0xFF,
                wlast: false,
            },
            AxiWriteChannelSample {
                cycle: 5,
                wvalid: true,
                wready: true,
                wdata: 0x2222,
                wstrb: 0xFF,
                wlast: true, // WLAST on beat 1 when expected_beats = 2 -> AXI_ERRM_WDATA_NUM!
            },
        ];
        checker.check_write_stream(2, &stream);
        assert!(
            checker
                .violations
                .iter()
                .any(|v| v.kind == AxiViolationKind::WdataUnstable)
        );
        assert!(
            checker
                .violations
                .iter()
                .any(|v| v.kind == AxiViolationKind::MaxWaitsDeadlock)
        );
        assert!(
            checker
                .violations
                .iter()
                .any(|v| v.kind == AxiViolationKind::WdataNumMismatch)
        );
    }

    #[test]
    fn pcie_gen1_to_gen4_ltssm_trace_analyzer_and_equalization_diagnostics() {
        let mut analyzer = PcieLtssmAnalyzer::new();
        let trace = [
            (0, PcieLtssmState::DetectQuiet, PcieGenSpeed::Gen1, 8, 4, 80),
            (100, PcieLtssmState::DetectActive, PcieGenSpeed::Gen1, 8, 4, 80),
            (250, PcieLtssmState::PollingActive, PcieGenSpeed::Gen1, 8, 4, 80),
            (400, PcieLtssmState::ConfigurationLinkwidthStart, PcieGenSpeed::Gen1, 8, 4, 80),
            (600, PcieLtssmState::ConfigurationComplete, PcieGenSpeed::Gen1, 8, 4, 80),
            (700, PcieLtssmState::L0, PcieGenSpeed::Gen1, 8, 4, 80),
            // Speed upgrade to Gen3 (8.0 GT/s) with Phase 0..3 Equalization
            (900, PcieLtssmState::RecoveryRcvrLock, PcieGenSpeed::Gen1, 8, 4, 75),
            (1000, PcieLtssmState::RecoverySpeed, PcieGenSpeed::Gen3, 8, 4, 75),
            (1100, PcieLtssmState::RecoveryEqualizationPhase0, PcieGenSpeed::Gen3, 8, 4, 75),
            (1200, PcieLtssmState::RecoveryEqualizationPhase1, PcieGenSpeed::Gen3, 8, 4, 75),
            (1300, PcieLtssmState::RecoveryEqualizationPhase2, PcieGenSpeed::Gen3, 8, 7, 68),
            (1400, PcieLtssmState::RecoveryEqualizationPhase3, PcieGenSpeed::Gen3, 8, 7, 72),
            (1500, PcieLtssmState::L0, PcieGenSpeed::Gen3, 8, 7, 72),
            // Speed upgrade to Gen4 (16.0 GT/s), with poor FOM on Phase 2 triggering diagnostic
            (1700, PcieLtssmState::RecoveryRcvrLock, PcieGenSpeed::Gen3, 8, 7, 70),
            (1800, PcieLtssmState::RecoverySpeed, PcieGenSpeed::Gen4, 8, 7, 60),
            (1900, PcieLtssmState::RecoveryEqualizationPhase2, PcieGenSpeed::Gen4, 8, 12, 10),
            // Fallback downgrade to Gen3 L0
            (2000, PcieLtssmState::L0, PcieGenSpeed::Gen3, 8, 7, 65),
        ];
        for &(ts, state, speed, width, preset, fom) in &trace {
            analyzer.push_sample(PcieTraceSample {
                timestamp_ns: ts,
                state,
                speed,
                link_width: width,
                tx_eq_preset: preset,
                rx_eq_fom: fom,
            });
        }

        let rep = analyzer.analyze();
        assert!(rep.reached_l0);
        assert_eq!(rep.max_speed, PcieGenSpeed::Gen4);
        assert_eq!(rep.final_speed, PcieGenSpeed::Gen3);
        assert_eq!(rep.final_link_width, 8);
        assert_eq!(rep.speed_upgrades, 2);
        assert_eq!(rep.speed_downgrades, 1);
        assert_eq!(rep.eq_phases_visited, vec![0, 1, 2, 3]);
        assert_eq!(rep.recovery_retrain_count, 2);
        assert!(rep.issues.iter().any(|i| matches!(
            i,
            PcieLinkIssue::EqualizationPresetFailure { preset: 12, fom: 10, .. }
        )));
        assert!(rep.issues.iter().any(|i| matches!(
            i,
            PcieLinkIssue::EqualizationFallbackDowngrade {
                from_speed: PcieGenSpeed::Gen4,
                to_speed: PcieGenSpeed::Gen3,
                ..
            }
        )));
    }
}
