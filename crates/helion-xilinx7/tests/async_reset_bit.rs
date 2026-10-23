//! t04 asserts a hardware behavior in its comment: the asynchronous reset
//! reaches the flop. This checks the emitted configuration says what that
//! comment claims, instead of trusting the prose.
//!
//! `bitstream_for_part` deliberately exposes only counts, so this drives the
//! lower layers directly to read the FASM feature list.

use helion_xilinx7::build::PinConstraints;

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fasm_for(example: &str) -> (Vec<String>, usize) {
    let src = std::fs::read_to_string(root().join("examples").join(example)).expect("read source");
    let mut design = helion_sv::synth_sv(&src, example).expect("synth");
    let xdc = std::fs::read_to_string(root().join("examples/zedboard.xdc")).expect("read xdc");
    PinConstraints::parse(&xdc).apply(&mut design);

    let db = std::env::var("HELION_XILINX_DB")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../work/prjxray-db").canonicalize())
        .expect("set HELION_XILINX_DB");
    let mut device =
        helion_xilinx7::device::Device::load_part_from_db(&db, "xc7z020").expect("load device");
    let nm = helion_xilinx7::xdevice::build_node_map(&mut device);
    let packed = helion_xilinx7::pack::pack(&design).expect("pack");
    let mut xdev = helion_xilinx7::xdevice::XDevice::new(&device, nm);
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = helion_xilinx7::place::place(&packed, &xdev).expect("place");
    let routed = helion_xilinx7::route::route(&placed, &xdev).expect("route");
    let bits = helion_xilinx7::emit::emit_fasm(&routed, &xdev).expect("emit");
    (bits.fasm, packed.ffs.len())
}

/// The reset must actually reach a flop: `ZRST` selects the SR pin as RESET and
/// `SRUSEDMUX` enables the slice-level SR mux. Without both, the FF silently
/// ignores its reset pin and t04's pass criterion would be wrong on hardware.
#[test]
fn t04_wires_the_async_reset_to_a_flop() {
    let (fasm, ffs) = fasm_for("t04_ff_reset.sv");
    assert!(ffs >= 1, "t04 must pack at least one flop, got {ffs}");
    let joined = fasm.join("\n");
    assert!(
        joined.contains("SRUSEDMUX"),
        "the SR input mux is not enabled, so the reset cannot reach the flop;\n\
         features present: {:?}",
        fasm.iter().filter(|l| l.contains("FF") || l.contains("SLICE")).take(12).collect::<Vec<_>>()
    );
    assert!(
        joined.contains("ZRST") || joined.contains("SR"),
        "no SR/ZRST feature was emitted, so the reset pin is unconfigured"
    );
}

/// The reset must be *asynchronous*: the design's FF is sensitive to a reset
/// edge, so the packed flop must carry an SR net at all. A synchronous reset
/// would leave the SR pin unconnected and this would fail.
#[test]
fn t04_flop_has_an_sr_net() {
    let src = std::fs::read_to_string(root().join("examples/t04_ff_reset.sv")).unwrap();
    let mut design = helion_sv::synth_sv(&src, "t04.sv").expect("synth");
    let xdc = std::fs::read_to_string(root().join("examples/zedboard.xdc")).unwrap();
    PinConstraints::parse(&xdc).apply(&mut design);
    let packed = helion_xilinx7::pack::pack(&design).expect("pack");
    assert!(
        packed.ffs.iter().any(|f| f.sr_net.is_some()),
        "no packed flop has an SR net, so `posedge btn[0]` was dropped"
    );
}

/// The control comparison: t03 has no reset and must NOT enable the SR mux, so
/// this asserts the check above distinguishes the two designs rather than
/// matching any FASM at all.
#[test]
fn t03_does_not_enable_the_sr_mux() {
    let (fasm, _) = fasm_for("t03_lut_truth_table.sv");
    let joined = fasm.join("\n");
    assert!(
        !joined.contains("SRUSEDMUX"),
        "a design with no reset enabled the SR mux"
    );
}
