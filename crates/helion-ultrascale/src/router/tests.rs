//! Router tests: determinism, completion, fail-loud.

use super::mock::MockWireDb;
use super::*;

fn site(x: i32, y: i32) -> UltraSite {
    UltraSite { x, y, kind: 0 }
}

fn net(name: &str, dx: i32, dy: i32, sinks: &[(i32, i32)]) -> RoutableNet {
    RoutableNet {
        name: name.to_string(),
        driver: (site(dx, dy), "A6LUT".to_string(), "O6".to_string()),
        sinks: sinks
            .iter()
            .map(|(x, y)| (site(*x, *y), "A6LUT".to_string(), "A1".to_string()))
            .collect(),
        is_clock: false,
    }
}

fn small_db() -> MockWireDb {
    let mut db = MockWireDb::new(10, 10);
    for y in 0..10 {
        for x in 0..10 {
            let s = site(x, y);
            db.add_pin(s, "A6LUT", "O6");
            db.add_pin(s, "A6LUT", "A1");
        }
    }
    db
}

/// The vendored INT tile JSON (BitFiltrator, MIT), shared with the
/// wire_db tests.
const TILE_JSON: &str = include_str!("../../data/tile_type_INT.json");

/// One INT tile at (1, 0), the tile the counter flow routes over
/// (SLICE_X1Y0 partners INT_X1Y0).
fn int_tile_db() -> crate::wire_db::IntTileWireDb {
    crate::wire_db::IntTileWireDb::single_tile("INT_X1Y0", 1, 0, TILE_JSON).unwrap()
}

fn counter_site() -> UltraSite {
    UltraSite { x: 1, y: 0, kind: 0 }
}

/// Register one BEL pin to its INT wire in the tile DB.
fn pin(db: &mut crate::wire_db::IntTileWireDb, bel: &str, pin: &str, wire: &str) {
    db.add_pin_wire(counter_site(), bel, pin, "INT_X1Y0", wire)
        .unwrap();
}

fn rnet(name: &str, dbel: &str, dpin: &str, sinks: &[(&str, &str)]) -> RoutableNet {
    RoutableNet {
        name: name.to_string(),
        driver: (counter_site(), dbel.to_string(), dpin.to_string()),
        sinks: sinks
            .iter()
            .map(|(b, p)| (counter_site(), b.to_string(), p.to_string()))
            .collect(),
        is_clock: false,
    }
}

#[test]
fn counter_repaired_netlist_routes() {
    // The counter netlist after the flow's X-pin repair (cnt_2's FF
    // moved BQ2 -> DQ, so cnt_2_d takes D6 and is not routed):
    // the exact driver/sink wires from the flow's debug log. Must
    // route with zero overuse; on the base router this netlist
    // times out (OveruseTimeout on BYPASS_W6 after 50 iters).
    let mut db = int_tile_db();
    pin(&mut db, "AQ", "Q", "LOGIC_OUTS_W14");
    pin(&mut db, "AQ2", "Q", "LOGIC_OUTS_W23");
    pin(&mut db, "BQ", "Q", "LOGIC_OUTS_W6");
    pin(&mut db, "DQ", "Q", "LOGIC_OUTS_W10");
    pin(&mut db, "B6LUT", "O6", "LOGIC_OUTS_W12");
    pin(&mut db, "C6LUT", "O6", "LOGIC_OUTS_W4");
    pin(&mut db, "A6LUT", "I0", "IMUX_W10");
    pin(&mut db, "A6LUT", "I1", "IMUX_W8");
    pin(&mut db, "B6LUT", "I0", "IMUX_W11");
    pin(&mut db, "C6LUT", "I0", "IMUX_W6");
    pin(&mut db, "D6LUT", "I0", "IMUX_W7");
    pin(&mut db, "A6LUT", "I2", "IMUX_W24");
    pin(&mut db, "C6LUT", "I1", "IMUX_W12");
    pin(&mut db, "D6LUT", "I1", "IMUX_W13");
    pin(&mut db, "A6LUT", "I3", "IMUX_W22");
    pin(&mut db, "D6LUT", "I2", "IMUX_W17");
    pin(&mut db, "AQ2", "D", "BOUNCE_W_0_FT1");
    pin(&mut db, "BQ", "D", "BYPASS_W1");
    let nets = vec![
        rnet("cnt_3", "AQ", "Q", &[("A6LUT", "I0")]),
        rnet(
            "cnt_0",
            "AQ2",
            "Q",
            &[
                ("A6LUT", "I1"),
                ("B6LUT", "I0"),
                ("C6LUT", "I0"),
                ("D6LUT", "I0"),
            ],
        ),
        rnet(
            "cnt_1",
            "BQ",
            "Q",
            &[("A6LUT", "I2"), ("C6LUT", "I1"), ("D6LUT", "I1")],
        ),
        rnet("cnt_2", "DQ", "Q", &[("A6LUT", "I3"), ("D6LUT", "I2")]),
        rnet("cnt_0_d", "B6LUT", "O6", &[("AQ2", "D")]),
        rnet("cnt_1_d", "C6LUT", "O6", &[("BQ", "D")]),
    ];
    let r = route_ultra(&nets, &db, UltraRouteOpts::default())
        .expect("repaired counter netlist must route");
    assert_eq!(r.overused_nodes, 0);
    assert_eq!(r.routes.len(), nets.len());
}

#[test]
fn counter_unrepaired_netlist_still_fails_loud() {
    // The counter netlist BEFORE the X-pin repair: cnt_1's and
    // cnt_2's FFs share letter B, so both D nets sink at the shared
    // BX pin (BYPASS_W1). Zero overuse is impossible for this
    // netlist: cnt_2 -> IMUX_W17 requires BYPASS_W6, and cnt_0_d
    // and cnt_2_d each require BYPASS_W6 or BYPASS_W4, so one of
    // those wires is shared by two nets in every routing. The
    // router must still fail loud with OveruseTimeout here (and not
    // silently emit an overused routing): the fix is the packing
    // repair, not a weaker router.
    let mut db = int_tile_db();
    pin(&mut db, "AQ", "Q", "LOGIC_OUTS_W14");
    pin(&mut db, "AQ2", "Q", "LOGIC_OUTS_W23");
    pin(&mut db, "BQ", "Q", "LOGIC_OUTS_W6");
    pin(&mut db, "BQ2", "Q", "LOGIC_OUTS_W19");
    pin(&mut db, "B6LUT", "O6", "LOGIC_OUTS_W12");
    pin(&mut db, "C6LUT", "O6", "LOGIC_OUTS_W4");
    pin(&mut db, "D6LUT", "O6", "LOGIC_OUTS_W16");
    pin(&mut db, "A6LUT", "I0", "IMUX_W10");
    pin(&mut db, "A6LUT", "I1", "IMUX_W8");
    pin(&mut db, "B6LUT", "I0", "IMUX_W11");
    pin(&mut db, "C6LUT", "I0", "IMUX_W6");
    pin(&mut db, "D6LUT", "I0", "IMUX_W7");
    pin(&mut db, "A6LUT", "I2", "IMUX_W24");
    pin(&mut db, "C6LUT", "I1", "IMUX_W12");
    pin(&mut db, "D6LUT", "I1", "IMUX_W13");
    pin(&mut db, "A6LUT", "I3", "IMUX_W22");
    pin(&mut db, "D6LUT", "I2", "IMUX_W17");
    pin(&mut db, "AQ2", "D", "BOUNCE_W_0_FT1");
    pin(&mut db, "BQ", "D", "BYPASS_W1");
    pin(&mut db, "BQ2", "D", "BYPASS_W1");
    let nets = vec![
        rnet("cnt_3", "AQ", "Q", &[("A6LUT", "I0")]),
        rnet(
            "cnt_0",
            "AQ2",
            "Q",
            &[
                ("A6LUT", "I1"),
                ("B6LUT", "I0"),
                ("C6LUT", "I0"),
                ("D6LUT", "I0"),
            ],
        ),
        rnet(
            "cnt_1",
            "BQ",
            "Q",
            &[("A6LUT", "I2"), ("C6LUT", "I1"), ("D6LUT", "I1")],
        ),
        rnet("cnt_2", "BQ2", "Q", &[("A6LUT", "I3"), ("D6LUT", "I2")]),
        rnet("cnt_0_d", "B6LUT", "O6", &[("AQ2", "D")]),
        rnet("cnt_1_d", "C6LUT", "O6", &[("BQ", "D")]),
        rnet("cnt_2_d", "D6LUT", "O6", &[("BQ2", "D")]),
    ];
    let err = route_ultra(&nets, &db, UltraRouteOpts::default())
        .expect_err("unrepaired counter netlist must fail loud");
    assert_eq!(err.kind, RouteErrorKind::OveruseTimeout);
}

#[test]
fn sink_wire_is_not_shared_with_a_through_net() {
    // counter5 regression, pinned as an invariant: cnt_1_d sinks at
    // BYPASS_W1 (its FF D pin stitches to that wire) while cnt_3
    // passed through the same wire on its way to IMUX_W26 in the
    // full flow. The old router excluded terminals from presence,
    // so it reported overused=0 while emitting two PIPs into
    // BYPASS_W1, which bitgen rejects (MustBeZero). With terminals
    // counted, cnt_3 must detour around BYPASS_W1 and the two nets
    // share no wire at all. Note this small netlist also routes
    // cleanly on the old router (the sharing needs the full
    // counter5 congestion to emerge), so it pins the invariant
    // rather than failing on the base tip; the fail-on-base proof
    // for this fix is the counter5 flow itself (bitgen MustBeZero
    // on base, verify-bit PASS with the fix).
    let mut db = int_tile_db();
    pin(&mut db, "C6LUT", "O6", "LOGIC_OUTS_W4");
    pin(&mut db, "BQ", "D", "BYPASS_W1");
    pin(&mut db, "AQ", "Q", "LOGIC_OUTS_W2");
    pin(&mut db, "A6LUT", "I4", "IMUX_W26");
    pin(&mut db, "A6LUT", "I5", "IMUX_W44");
    let nets = vec![
        rnet(
            "cnt_3",
            "AQ",
            "Q",
            &[("A6LUT", "I4"), ("A6LUT", "I5")],
        ),
        rnet("cnt_1_d", "C6LUT", "O6", &[("BQ", "D")]),
    ];
    let r = route_ultra(&nets, &db, UltraRouteOpts::default())
        .expect("both nets must route");
    assert_eq!(r.overused_nodes, 0);
    let bypass = db.wire_id("INT_X1Y0", "BYPASS_W1").unwrap();
    assert!(
        r.routes[1].contains(&bypass),
        "cnt_1_d must still sink at BYPASS_W1"
    );
    assert!(
        !r.routes[0].contains(&bypass),
        "cnt_3 must not pass through the sink wire BYPASS_W1"
    );
}

#[test]
fn bypass_sink_wire_is_terminal_for_other_nets() {
    // N58 (a): a BYPASS wire feeding a used site pin is terminal-only,
    // never transit. The victim net sinks at BYPASS_W1 (its FF D pin
    // stitches to that wire); the through net's shortest path to
    // IMUX_W26 runs through BYPASS_W1 (8 wires; the shortest path
    // avoiding it is 10). On the base router the through net takes
    // the shortcut in iteration 0 and the call fails loud with
    // OveruseTimeout at max_iters=1, because both nets claim the
    // wire. With the fix the through net detours around BYPASS_W1
    // in iteration 0 and both nets route with zero overuse. The
    // victim still sinks at BYPASS_W1: the rule blocks other nets,
    // never the net the pin belongs to.
    let mut db = int_tile_db();
    pin(&mut db, "C6LUT", "O6", "LOGIC_OUTS_W4");
    pin(&mut db, "BQ", "D", "BYPASS_W1");
    pin(&mut db, "EQ2", "Q", "LOGIC_OUTS_W7");
    pin(&mut db, "A6LUT", "I4", "IMUX_W26");
    let nets = vec![
        rnet("through", "EQ2", "Q", &[("A6LUT", "I4")]),
        rnet("victim_d", "C6LUT", "O6", &[("BQ", "D")]),
    ];
    let opts = UltraRouteOpts {
        seed: 0,
        max_iters: 1,
    };
    let r = route_ultra(&nets, &db, opts).expect("both nets must route");
    assert_eq!(r.overused_nodes, 0);
    let bypass = db.wire_id("INT_X1Y0", "BYPASS_W1").unwrap();
    assert!(
        !r.routes[0].contains(&bypass),
        "through net must not pass through the victim's BYPASS sink wire"
    );
    assert!(
        r.routes[1].contains(&bypass),
        "victim net must still sink at BYPASS_W1"
    );
}

#[test]
fn determinism_same_seed_identical_routes() {
    let db = small_db();
    let nets = vec![
        net("n0", 1, 1, &[(8, 8), (5, 5)]),
        net("n1", 2, 2, &[(7, 7)]),
        net("n2", 0, 9, &[(9, 0)]),
    ];
    let opts = UltraRouteOpts {
        seed: 42,
        max_iters: 20,
    };
    let r1 = route_ultra(&nets, &db, opts).expect("route");
    let r2 = route_ultra(&nets, &db, opts).expect("route");
    assert_eq!(r1.routes, r2.routes, "same seed must give identical routes");
    assert_eq!(r1.iters_used, r2.iters_used);
}

#[test]
fn determinism_different_seeds_both_complete() {
    let db = small_db();
    let nets = vec![net("n0", 1, 1, &[(8, 8)]), net("n1", 8, 1, &[(1, 8)])];
    let mut seen_routes = Vec::new();
    for seed in [1u64, 2, 3, 7, 12345] {
        let opts = UltraRouteOpts {
            seed,
            max_iters: 20,
        };
        let r = route_ultra(&nets, &db, opts).expect("route");
        assert_eq!(r.overused_nodes, 0);
        // Every net has a non-empty route covering driver and sinks.
        for route in &r.routes {
            assert!(!route.is_empty());
        }
        seen_routes.push(r.routes);
    }
    // At least two seeds should differ (seeds actually change the search).
    let first = &seen_routes[0];
    assert!(
        seen_routes[1..].iter().any(|r| r != first),
        "different seeds should explore different routes"
    );
}

#[test]
fn completion_three_designs_zero_unrouted() {
    // Design 1: counter-like (few nets, short).
    // Design 2: moderate fanout, no forced crossings.
    // Design 3: high fanout net.
    let db = small_db();
    let designs: Vec<Vec<RoutableNet>> = vec![
        vec![net("cnt0", 1, 1, &[(2, 1)]), net("cnt1", 1, 2, &[(2, 2)])],
        vec![
            net("a", 0, 0, &[(2, 2)]),
            net("b", 9, 0, &[(7, 2)]),
            net("c", 0, 9, &[(2, 7)]),
            net("d", 9, 9, &[(7, 7)]),
        ],
        vec![net(
            "clk_en",
            5,
            5,
            &[(0, 0), (9, 0), (0, 9), (9, 9), (5, 0)],
        )],
    ];
    for (i, nets) in designs.iter().enumerate() {
        let opts = UltraRouteOpts {
            seed: 100 + i as u64,
            max_iters: 50,
        };
        let r = route_ultra(nets, &db, opts)
            .unwrap_or_else(|e| panic!("design {} failed to route: {}", i, e));
        assert_eq!(r.overused_nodes, 0, "design {} has overuse", i);
        assert_eq!(r.routes.len(), nets.len());
        for (ni, route) in r.routes.iter().enumerate() {
            assert!(!route.is_empty(), "design {} net {} unrouted", i, ni);
        }
    }
}

#[test]
fn pathfinder_negotiates_crossing_nets() {
    // Stress test: 4 nets crossing. On a 20x20 grid with 4 tracks per tile,
    // PathFinder must spread them out (zero overuse after negotiation).
    let mut db = MockWireDb::with_tracks(20, 20, 4);
    for y in 0..20 {
        for x in 0..20 {
            let s = site(x, y);
            db.add_pin(s, "A6LUT", "O6");
            db.add_pin(s, "A6LUT", "A1");
        }
    }
    let nets = vec![
        net("a", 0, 0, &[(19, 19)]),
        net("b", 19, 0, &[(0, 19)]),
        net("c", 0, 10, &[(19, 10)]),
        net("d", 10, 0, &[(10, 19)]),
    ];
    let opts = UltraRouteOpts {
        seed: 7,
        max_iters: 50,
    };
    let r = route_ultra(&nets, &db, opts).expect("negotiation should succeed");
    assert_eq!(r.overused_nodes, 0);
}

#[test]
fn fail_loud_on_unroutable_net() {
    // 1-wide DB: two nets cannot both cross the single wire without overuse,
    // and with max_iters=1 the router must report overuse loudly.
    let mut db = MockWireDb::new(3, 1);
    for x in 0..3 {
        let s = site(x, 0);
        db.add_pin(s, "A6LUT", "O6");
        db.add_pin(s, "A6LUT", "A1");
    }
    let nets = vec![
        net("n0", 0, 0, &[(2, 0)]),
        net("n1", 0, 0, &[(2, 0)]),
        net("n2", 2, 0, &[(0, 0)]),
    ];
    let opts = UltraRouteOpts {
        seed: 1,
        max_iters: 1,
    };
    let err = route_ultra(&nets, &db, opts).expect_err("must fail loudly");
    assert!(!err.net.is_empty(), "error must name the congested net");
    assert!(
        !err.location.is_empty(),
        "error must give the congestion location"
    );
    assert_eq!(err.kind, RouteErrorKind::OveruseTimeout);
}

#[test]
fn fail_loud_on_missing_pin() {
    let db = small_db(); // no pins registered for this site
    let nets = vec![net("n0", 50, 50, &[(1, 1)])];
    let opts = UltraRouteOpts::default();
    let err = route_ultra(&nets, &db, opts).expect_err("must fail on missing pin");
    assert_eq!(err.net, "n0");
    assert_eq!(err.kind, RouteErrorKind::NoPath);
}

#[test]
fn fail_loud_on_clock_net_phase1() {
    let db = small_db();
    let mut n = net("clk", 1, 1, &[(8, 8)]);
    n.is_clock = true;
    let opts = UltraRouteOpts::default();
    let err = route_ultra(&[n], &db, opts).expect_err("clock nets are phase 2");
    assert_eq!(err.kind, RouteErrorKind::ClockNetInPhase1);
}

#[test]
fn mock_wire_info_is_none() {
    // The mock has grid positions only and cannot name wires, so it can
    // never feed the router-to-PIP translator. Expected.
    let db = small_db();
    assert_eq!(db.wire_info(0), None);
    assert_eq!(db.wire_info(42), None);
}

#[test]
fn default_route_opts_is_the_single_budget_home() {
    // The route iteration budget lives here and nowhere else: the
    // impl flow takes `..Default::default()` instead of restating
    // the number, so the multiseed path and the impl path can never
    // drift apart on it once routing lands for multiseed.
    let opts = UltraRouteOpts::default();
    assert_eq!(opts.max_iters, 50);
    assert_eq!(opts.seed, 0);
}
