//! End-to-end: blinky.sv → pack → place → route → FASM for xc7a35t.
//!
//! Requires the X-Ray DB at `~/workspace/xilinx-dbsrc` (or `HELION_XILINX_DB`).

use helion_xilinx7::device::{default_db_root, Device};
use helion_xilinx7::xdevice::{build_node_map, XDevice};
use helion_xilinx7::{emit_fasm, pack, place, route};

#[test]
fn blinky_e2e_fasm() {
    let db_root = default_db_root();
    let mut device = Device::load_xc7a35t(&db_root).expect("load device");
    assert!(device.tile_count() > 10000, "tile_count = {}", device.tile_count());
    let node_maps = build_node_map(&mut device);

    // Synthesize.
    let src = std::fs::read_to_string("../../examples/blinky.sv").expect("blinky.sv");
    let mut design = helion_sv::synth_sv(&src, "blinky.sv").expect("synth");
    // LOC constraints (Arty A7).
    for port in &mut design.ports {
        match port.name.as_str() {
            "clk" => port.attrs.set("LOC", "E3"),
            "led" => port.attrs.set("LOC", "H5"),
            _ => {}
        }
    }

    let packed = pack(&design).expect("pack");
    assert!(!packed.luts.is_empty(), "no LUTs packed");
    assert!(!packed.ffs.is_empty(), "no FFs packed");
    assert_eq!(packed.bufgs.len(), 1, "expected 1 BUFG");

    let mut xdev = XDevice::new(&device, node_maps);
    // Clock nets for the router.
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = place(&packed, &xdev).expect("place");
    assert_eq!(placed.iobs.len(), 2, "expected 2 IOBs");

    let routed = route(&placed, &xdev).expect("route");
    assert!(!routed.nets.is_empty(), "no nets routed");

    let bs = emit_fasm(&routed, &xdev).expect("emit");
    assert!(!bs.fasm.is_empty(), "empty FASM");
    let text = bs.fasm.join("\n");

    // Spot-check key features.
    assert!(text.contains(".ALUT.INIT[63:0]"), "missing LUT INIT:\n{text}");
    assert!(text.contains(".AFF.ZINI") || text.contains("FF.ZINI"), "missing FF:\n{text}");
    assert!(text.contains("LVCMOS"), "missing IOB config:\n{text}");
    assert!(text.contains("BUFGCTRL"), "missing BUFG:\n{text}");

    // Write FASM for inspection / Worker C validation.
    let out = std::path::PathBuf::from("/tmp/x7/blinky.fasm");
    std::fs::create_dir_all(out.parent().unwrap()).ok();
    std::fs::write(&out, bs.fasm.join("\n") + "\n").unwrap();
    println!("wrote {} ({} lines)", out.display(), bs.fasm.len());
    println!("--- FASM ---\n{text}\n--- end ---");
}
