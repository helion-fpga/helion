//! G99 guard: a ROM-fed multiply-accumulate written in plain VHDL
//! (examples/zedboard_rom_mac.vhd: two 32768-entry ROMs filled by
//! functions, a 64-entry result array read by a runtime index, a signed
//! product into an accumulator) meets 100 MHz on the ZedBoard.
//!
//! It fails if any of these comes back: arrays read as priority chains
//! instead of mux trees, `+`/`*` lowered to gates instead of CARRY4/DSP48,
//! `resize(a * b, 32)` keeping the product's register out of the DSP, or
//! block RAMs and DSPs left far from their logic (without the placer's
//! re-site passes this design misses by about 2.2 ns).
//!
//! Requires the X-Ray zynq7 database (`HELION_XILINX_DB`, see `device.rs`).

use helion_xilinx7::xdc_flow;

#[test]
fn rom_mac_meets_100_mhz() {
    let ex = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples");
    let out = std::env::temp_dir().join(format!("helion_rom_mac_{}.bit", std::process::id()));
    let mut log = |_: &str| {};
    let b = xdc_flow::run(
        &format!("{ex}/zedboard_rom_mac.vhd"),
        &format!("{ex}/zedboard_rom_mac.xdc"),
        out.to_str().unwrap(),
        Some("zedboard_rom_mac"),
        true,
        "auto",
        "xc7z020clg484-1",
        None,
        &mut log,
    )
    .expect("flow")
    .expect("a built design");
    let _ = std::fs::remove_file(&out);
    assert_eq!(b.period_ns, 10.0);
    assert!(b.slack_ns >= 0.0, "worst slack {} ns at 100 MHz; {}", b.slack_ns, b.summary);
    assert!(b.summary.contains("\"dsp\":1.0"), "the product is one DSP48: {}", b.summary);
}
