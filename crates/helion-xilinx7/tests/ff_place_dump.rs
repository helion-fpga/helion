//! Scratch: dump OUR L-side LUT equations (probe).
use std::collections::HashMap;

use helion_xilinx7::device::{default_db_root, Device};
use helion_xilinx7::xdevice::{build_node_map, XDevice};
use helion_xilinx7::{pack, place};

#[test]
fn dump_l_lut_equations() {
    let db_root = default_db_root();
    let mut device = Device::load_xc7z020(&db_root).expect("load xc7z020");
    let src = std::fs::read_to_string("../../examples/blinky_reg.sv").expect("blinky_reg.sv");
    let mut design = helion_sv::synth_sv_carry(&src, "blinky_reg.sv").expect("carry synth");
    for port in &mut design.ports {
        if port.name == "clk" {
            port.attrs.set("LOC", "Y9");
            port.attrs.set("IOSTANDARD", "LVCMOS33");
        }
    }
    let mut packed = pack(&design).expect("pack");
    for iob in &mut packed.iobs {
        if iob.port == "LD0" {
            iob.loc = Some("T22".to_string());
            iob.iostandard = "LVCMOS33".to_string();
        }
    }
    packed.carry_anchor = Some("SLICE_X112Y39".into());
    let node_maps = build_node_map(&mut device);
    let mut xdev = XDevice::new(&device, node_maps);
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = place(&packed, &xdev).expect("place");
    let luts: HashMap<&str, _> =
        packed.luts.iter().map(|l| (l.name.as_str(), l)).collect();
    let mut pls = placed.luts.clone();
    pls.sort_by(|a, b| (&a.tile, &a.site, &a.bel).cmp(&(&b.tile, &b.site, &b.bel)));
    for l in &pls {
        if !(l.tile.starts_with("CLBLM_R_X71Y4") && (l.bel == "A6LUT" || l.bel == "D6LUT")) {
            continue;
        }
        match luts.get(l.name.as_str()) {
            Some(p) => println!(
                "LUT {} {} {} init={:#018x} in={:?} out={}",
                l.tile, l.site, l.bel, p.init, p.input_nets, p.output_net
            ),
            None => println!("LUT {} {} {} (no packed match)", l.tile, l.site, l.bel),
        }
    }
}
