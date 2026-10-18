//! Performance and deterministic QoR verification across pack, place, route, STA, DRC, and bitgen.

use helion_bits::bitgen;
use helion_device::Device;
use helion_drc::check_routed;
use helion_ir::{CellKind, Design, INC4_INIT, PortDir};
use helion_pack::pack;
use helion_place::{PlaceOpts, place_incremental, place_with};
use helion_route::{RouteOpts, route_with};
use helion_sta::{
    Clock, Constraints, create_clock, critical_path, report_cdc, report_timing_placed,
    report_timing_routed, timing_paths_routed,
};
use std::time::Instant;

fn build_synthetic_datapath(n_stages: usize) -> Design {
    let mut d = Design::new("synth_datapath");
    d.add_port("clk", PortDir::In);
    d.add_port("led0", PortDir::Out);
    d.add_port("led1", PortDir::Out);
    for i in 0..n_stages {
        let lut = format!("u_lut_{i}");
        let ff = format!("u_ff_{i}");
        let init = INC4_INIT[i % 4];
        d.add_cell(&lut, CellKind::Lut6 { init });
        d.add_cell(&ff, CellKind::Hff);
        d.connect("clk", &ff, "CLK");
        d.connect(&format!("d_{i}"), &lut, "O");
        d.connect(&format!("d_{i}"), &ff, "D");
        d.connect(&format!("q_{i}"), &ff, "Q");
        d.connect(&format!("q_{i}"), &lut, "I0");
        if i >= 1 {
            d.connect(&format!("q_{}", i - 1), &lut, "I1");
        }
        if i >= 2 {
            d.connect(&format!("q_{}", i - 2), &lut, "I2");
        }
        if i >= 3 {
            d.connect(&format!("q_{}", i - 3), &lut, "I3");
        }
    }
    d.add_cell("u_iob0", CellKind::IobOut);
    d.connect(&format!("q_{}", n_stages / 2), "u_iob0", "I");
    d.connect("led0", "u_iob0", "PAD");
    d.add_cell("u_iob1", CellKind::IobOut);
    d.connect(&format!("q_{}", n_stages - 1), "u_iob1", "I");
    d.connect("led1", "u_iob1", "PAD");
    d
}

#[test]
fn end_to_end_pnr_sta_drc_bits_perf_and_deterministic_qor() {
    let dev = Device::load_part("HL10T-C32-1").expect("load HL10T-C32-1");
    let design = build_synthetic_datapath(600);
    let mut clks: Vec<Clock> = Vec::new();
    create_clock(&mut clks, "clk", 10_000, "clk");

    let t0 = Instant::now();
    let packed1 = pack(&design, &dev).expect("pack 1");
    let pack_ms = t0.elapsed().as_millis();

    let t_place = Instant::now();
    let placed1 = place_with(
        &packed1,
        &dev,
        PlaceOpts {
            timing_weight: 0.75,
            seed: 0,
        },
    )
    .expect("place 1");
    let place_ms = t_place.elapsed().as_millis();

    let t_inc = Instant::now();
    let (inc_placed, reused) = place_incremental(
        &packed1,
        &dev,
        &placed1,
        PlaceOpts {
            timing_weight: 0.75,
            seed: 0,
        },
    )
    .expect("incremental place");
    let inc_ms = t_inc.elapsed().as_millis();
    assert_eq!(reused, placed1.lutff_sites.len());
    assert_eq!(inc_placed.lutff_sites, placed1.lutff_sites);

    let t_route = Instant::now();
    let routed1 = route_with(&placed1, &dev, RouteOpts::default()).expect("route 1");
    let route_ms = t_route.elapsed().as_millis();

    let t_sta = Instant::now();
    let sta_placed1 = report_timing_placed(&design, &placed1, &clks).expect("sta placed");
    let sta1 = report_timing_routed(&design, &routed1, &clks).expect("sta routed 1");
    let paths1 = timing_paths_routed(&design, &routed1, &sta1);
    let cpath1 = critical_path(&design, &routed1, &sta1).expect("critical path 1");
    let cdc1 = report_cdc(&clks, &Constraints::default(), Some(&sta1), Some(&design));
    let sta_ms = t_sta.elapsed().as_millis();

    let t_drc = Instant::now();
    let drc1 = check_routed(&design, &routed1, &dev);
    let drc_ms = t_drc.elapsed().as_millis();
    drc1.fail().expect("DRC clean");

    let t_bits = Instant::now();
    let bits1 = bitgen(&dev, &routed1).expect("bitgen 1");
    let bits_ms = t_bits.elapsed().as_millis();

    // Repeat run to verify 100% bit-exact determinism across every stage.
    let packed2 = pack(&design, &dev).expect("pack 2");
    let placed2 = place_with(
        &packed2,
        &dev,
        PlaceOpts {
            timing_weight: 0.75,
            seed: 0,
        },
    )
    .expect("place 2");
    let routed2 = route_with(&placed2, &dev, RouteOpts::default()).expect("route 2");
    let sta2 = report_timing_routed(&design, &routed2, &clks).expect("sta routed 2");
    let paths2 = timing_paths_routed(&design, &routed2, &sta2);
    let bits2 = bitgen(&dev, &routed2).expect("bitgen 2");

    assert_eq!(placed1.lutff_sites, placed2.lutff_sites, "deterministic placement");
    assert_eq!(routed1.congestion, routed2.congestion, "deterministic congestion");
    assert_eq!(routed1.imux_skip, routed2.imux_skip, "deterministic IMUX reach");
    assert_eq!(sta1.wns_ps, sta2.wns_ps, "deterministic WNS");
    assert_eq!(sta1.setup_ps, sta2.setup_ps, "deterministic setup delay");
    assert_eq!(sta1.hold_slack_ps, sta2.hold_slack_ps, "deterministic hold slack");
    assert_eq!(paths1, paths2, "deterministic timing paths");
    assert_eq!(bits1.packets, bits2.packets, "bit-exact bitstream packets");
    assert!(sta_placed1.endpoints == 600);
    assert!(cpath1.delay_ps > 0);
    assert!(cdc1.crossings.is_empty());

    // Wall-clock budget even under unoptimized debug builds.
    let total_ms = t0.elapsed().as_millis();
    eprintln!(
        "perf_qor 600-stage: pack={pack_ms}ms place={place_ms}ms inc={inc_ms}ms route={route_ms}ms sta={sta_ms}ms drc={drc_ms}ms bits={bits_ms}ms total={total_ms}ms"
    );
    assert!(
        total_ms < 5_000,
        "end-to-end 600-stage flow took {total_ms}ms (expected < 5000ms)"
    );
}
