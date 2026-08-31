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
