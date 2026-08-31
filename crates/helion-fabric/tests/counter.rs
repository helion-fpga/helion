//! 4-bit incrementer through pack/place/PathFinder/bitgen/fabric. LED = cnt[3].

use helion_bits::bitgen;
use helion_device::Device;
use helion_fabric::Fabric;
use helion_ir::Design;
use helion_pack::pack;
use helion_place::place;
use helion_route::route;

fn wave(design: Design, cycles: usize) -> Vec<bool> {
    let dev = Device::load_part("HL10T-C32-1").expect("HAD");
    let packed = pack(&design, &dev).expect("pack");
    let placed = place(&packed, &dev).expect("place");
    let routed = route(&placed, &dev).expect("route");
    assert_eq!(routed.iob_src[0].ble, 3, "LED must be driven by cnt[3]");
    let bits = bitgen(&dev, &routed).expect("bitgen");
    let mut sim = Fabric::new(&dev);
    sim.program(&bits).expect("program");
    sim.finish_startup();
    let iob = routed.iob_src[0].iob;
    let mut w = Vec::new();
    for _ in 0..cycles {
        sim.step_user();
        w.push(sim.led_at(iob.0, iob.1));
    }
    w
}

#[test]
fn structural_counter_led_is_cnt3() {
    let w = wave(Design::structural_counter(), 16);
    // After k steps, cnt==k (mod 16); LED = cnt[3].
    assert!(w[0..7].iter().all(|b| !b), "cnt 1..7 LED=0 got {w:?}");
    assert!(w[7..15].iter().all(|b| *b), "cnt 8..15 LED=1 got {w:?}");
    assert!(!w[15], "cnt wraps to 0, LED=0 got {w:?}");
}
