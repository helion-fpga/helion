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
    let nets = vec![
        net("n0", 1, 1, &[(8, 8)]),
        net("n1", 8, 1, &[(1, 8)]),
    ];
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
        vec![net("clk_en", 5, 5, &[(0, 0), (9, 0), (0, 9), (9, 9), (5, 0)])],
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
    assert!(
        !err.net.is_empty(),
        "error must name the congested net"
    );
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
