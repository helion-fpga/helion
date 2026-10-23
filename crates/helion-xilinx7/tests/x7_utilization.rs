//! `report_utilization` and `qor` must work on real parts. Both used to load
//! the device through the HL10T path and fail with a file-not-found, so the
//! numbers simply could not be obtained for a Xilinx device.

use helion_xilinx7::build::{x7_utilization, PinConstraints};

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn db() -> std::path::PathBuf {
    std::env::var("HELION_XILINX_DB")
        .map(std::path::PathBuf::from)
        .or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../work/prjxray-db")
                .canonicalize()
        })
        .expect("set HELION_XILINX_DB")
}

fn zedboard(example: &str) -> helion_ir::Design {
    let src = std::fs::read_to_string(root().join("examples").join(example)).expect("read");
    let mut d = helion_sv::synth_sv(&src, example).expect("synth");
    let xdc = std::fs::read_to_string(root().join("examples/zedboard.xdc")).expect("xdc");
    PinConstraints::parse(&xdc).apply(&mut d);
    d
}

#[test]
fn blinky_reports_real_device_capacity_and_slack() {
    let d = zedboard("zedboard_blinky.sv");
    let u = x7_utilization(&d, "xc7z020", Some(&db()), 10_000).expect("utilization");
    // Capacity is the xc7z020's real resources, not a placeholder.
    assert_eq!(u.lut_capacity, 13300, "xc7z020 LUT6 count");
    assert_eq!(u.bram_capacity, 140, "xc7z020 RAMB count");
    assert_eq!(u.dsp_capacity, 220, "xc7z020 DSP count");
    // The concrete package, not the bare die: `tilegrid` and `package_pins`
    // are per-package, and a bare `xc7z020` has no package_pins.csv.
    assert_eq!(u.part, "xc7z020clg484-1");
    // `lut_sites` counts occupied LUT6 sites, not flops: the 7-series flow keeps
    // LUTs and FFs separate, unlike the HL10T flow that reported one LUTFF. A 27-bit counter plus its carry and output buffers needs
    // more LUT sites than 27, so assert the shape rather than an exact count.
    assert!(
        u.lut_sites >= 27,
        "a 27-bit counter cannot use fewer than 27 LUT sites, got {}",
        u.lut_sites
    );
    assert!(
        u.lut_sites < u.lut_capacity,
        "blinky occupies {} of {} LUT sites",
        u.lut_sites,
        u.lut_capacity
    );
    // 8 LED outputs plus the clock input.
    assert_eq!(u.iobs_out, 8, "blinky drives 8 LEDs");
    assert_eq!(u.iobs_in, 1, "blinky has one input, the clock");
    assert!(u.endpoints > 0, "blinky has register-to-register paths");
    assert_eq!(u.period_ps, 10_000);
    // Slack must be measured, not just echoed as the period.
    assert!(u.wns_ps < 10_000, "WNS must be a slack, not the period");
}

/// The clock input must not get a second IOB on top of its clock spine.
#[test]
fn a_clock_input_gets_exactly_one_iob() {
    let d = zedboard("zedboard_blinky.sv");
    let u = x7_utilization(&d, "xc7z020", Some(&db()), 10_000).expect("utilization");
    assert_eq!(
        u.iobs_in, 1,
        "the clock is padded once (by the clock spine), not twice"
    );
}

/// t02 exists to prove data inputs work; before the fix it would not build.
#[test]
fn a_design_with_data_inputs_builds_and_places_them() {
    let d = zedboard("t02_input_passthrough.sv");
    let u = x7_utilization(&d, "xc7z020", Some(&db()), 10_000).expect("utilization");
    assert_eq!(u.iobs_in, 5, "t02 reads btn[0]..btn[4]");
    assert_eq!(u.iobs_out, 8, "t02 drives all 8 LEDs");
}
