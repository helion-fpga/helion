//! End-to-end 0.1: structural LUT+FF netlist → pack/place/route/bitgen → fabric sim LED toggle.

use helion_bits::bitgen;
use helion_device::Device;
use helion_fabric::Fabric;
use helion_ir::Design;
use helion_pack::pack;
use helion_place::place;
use helion_route::route;

#[test]
fn blinky_led_toggles_in_fabric_sim() {
    let dev = Device::load_part("HL10T-C32-1").expect("load Helion-T HAD");
    let design = Design::structural_blinky();
    let packed = pack(&design, &dev).expect("pack");
    let placed = place(&packed, &dev).expect("place");
    let routed = route(&placed, &dev).expect("route");
    let bits = bitgen(&dev, &routed).expect("bitgen");

    let mut sim = Fabric::new(&dev);
    sim.program(&bits).expect("program");
    sim.finish_startup();
    assert!(sim.stat.done && sim.stat.gwe && !sim.stat.gsr && !sim.stat.gts);

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
    assert!(
        changes >= 1,
        "LED must toggle, got {changes} changes (constant output means P&R/sim is broken)"
    );
}
