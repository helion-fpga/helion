use helion_bits::bitgen;
use helion_device::Device;
use helion_fabric::Fabric;
use helion_pack::pack;
use helion_place::place;
use helion_route::route;
use helion_sv::synth_sv_path;

#[test]
fn sv_blinky_through_pnr_toggles_led() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/blinky.sv");
    let design = synth_sv_path(&root).expect("sv-parser synth");
    let dev = Device::load_part("HL10T-C32-1").unwrap();
    let packed = pack(&design, &dev).unwrap();
    let placed = place(&packed, &dev).unwrap();
    let routed = route(&placed, &dev).unwrap();
    let bits = bitgen(&dev, &routed).unwrap();
    let mut sim = Fabric::new(&dev);
    sim.program(&bits).unwrap();
    sim.finish_startup();
    let iob = routed.iob_src[0].iob;
    let mut last = sim.led_at(iob.0, iob.1);
    let mut changes = 0u32;
    for _ in 0..8 {
        sim.step_user();
        let now = sim.led_at(iob.0, iob.1);
        if now != last {
            changes += 1;
            last = now;
        }
    }
    assert!(changes >= 1, "SV blinky LED must toggle, changes={changes}");
}

#[test]
fn sv_counter_through_pnr_is_cnt3() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv");
    let design = synth_sv_path(&root).expect("sv-parser synth counter");
    assert_eq!(design.lut_inits().len(), 4);
    let dev = Device::load_part("HL10T-C32-1").unwrap();
    let packed = pack(&design, &dev).unwrap();
    let placed = place(&packed, &dev).unwrap();
    let routed = route(&placed, &dev).unwrap();
    assert_eq!(routed.iob_src[0].ble, 3);
    let bits = bitgen(&dev, &routed).unwrap();
    let mut sim = Fabric::new(&dev);
    sim.program(&bits).unwrap();
    sim.finish_startup();
    let iob = routed.iob_src[0].iob;
    let mut w = Vec::new();
    for _ in 0..16 {
        sim.step_user();
        w.push(sim.led_at(iob.0, iob.1));
    }
    assert!(w[0..7].iter().all(|b| !b), "cnt 1..7 LED=0 {w:?}");
    assert!(w[7..15].iter().all(|b| *b), "cnt 8..15 LED=1 {w:?}");
    assert!(!w[15], "{w:?}");
}

#[test]
fn sv_hierarchy_toggles() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/hier.sv");
    let design = synth_sv_path(&root).expect("hier synth");
    assert_eq!(design.lut_inits(), vec![0x5555_5555_5555_5555]);
    let dev = Device::load_part("HL10T-C32-1").unwrap();
    let packed = pack(&design, &dev).unwrap();
    let placed = place(&packed, &dev).unwrap();
    let routed = route(&placed, &dev).unwrap();
    let bits = bitgen(&dev, &routed).unwrap();
    let mut sim = Fabric::new(&dev);
    sim.program(&bits).unwrap();
    sim.finish_startup();
    let iob = routed.iob_src[0].iob;
    let mut changes = 0u32;
    let mut last = sim.led_at(iob.0, iob.1);
    for _ in 0..8 {
        sim.step_user();
        let now = sim.led_at(iob.0, iob.1);
        if now != last {
            changes += 1;
            last = now;
        }
    }
    assert!(changes >= 1, "hierarchical blinky must toggle, changes={changes}");
}
