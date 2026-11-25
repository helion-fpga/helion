//! N48 regression tests: the exact counter12-s0 and counter16-s0
//! netlists from the flow debug logs route against the real INT-tile
//! graph within the 50-iteration budget.
//!
//! The fixtures (n48_fixtures.rs) are the `route_debug resolved`
//! netlists from ~/workspace/n48-logs/impl/dbg-c12-s0-r1.log and
//! dbg-c16-s0-r1.log: same placement, same net order (md5-identical
//! across processes), same wire ids. The flow routes these netlists
//! on TileconnWireDb, which delegates fanout to the inner
//! IntTileWireDb built here, over the same tile set (c12: X0..X6 x
//! Y0..Y1, 14 tiles; c16: X0..X7 x Y0..Y1, 16 tiles), with the same
//! clock-leg forbidden PIPs removed.
//!
//! What these pin (N48): on the base code these netlists route in
//! 28-50 iterations or time out with OveruseTimeout depending on
//! the process (fanout order came from a HashMap: c12 failed 2/10
//! harness runs, c16 4/10, with the same failing wires the flows
//! showed, 4914/1513/9238). With deterministic fanout (wire_db
//! sorts each fanout list) plus history cost growing by the
//! present-use count, both route deterministically: c12 in 20
//! iterations, c16 in 28. The iters bound (<= 40) pins the
//! convergence margin: a return to near-budget grinding fails
//! here before it fails a flow.
//!
//! Run with `--nocapture` to see the iters_used print.

use helion_ultrascale::fabric::UltraSite;
use helion_ultrascale::router::{route_ultra, RoutableNet, UltraRouteOpts};
use helion_ultrascale::wire_db::{IntTile, IntTileWireDb};

include!("n48_fixtures.rs");

const TILE_JSON: &str = include_str!("../data/tile_type_INT.json");

/// Convergence margin: both fixture netlists must route well
/// inside the 50-iteration budget (measured 20 and 28).
const N48_ITERS_BOUND: u32 = 40;

fn site() -> UltraSite {
    UltraSite {
        x: 0,
        y: 0,
        kind: 0,
    }
}

fn build_db(width: i32, tiles_y: i32, forbidden: &[((i32, i32), &str, &str)]) -> IntTileWireDb {
    let tiles: Vec<IntTile> = (0..tiles_y)
        .flat_map(|y| (0..width).map(move |x| IntTile::new(&format!("INT_X{x}Y{y}"), x, y)))
        .collect();
    let mut db = IntTileWireDb::from_json(TILE_JSON, &tiles).unwrap();
    for ((x, y), src, dst) in forbidden {
        let tile = format!("INT_X{x}Y{y}");
        assert!(
            db.forbid_pip(&tile, src, dst),
            "forbidden PIP {tile}.PIP.{dst}.{src} was not an edge"
        );
    }
    db
}

fn build_nets(
    db: &mut IntTileWireDb,
    fixture: &[(&str, (i32, i32, &str), &[(i32, i32, &str)])],
) -> Vec<RoutableNet> {
    let mut nets = Vec::with_capacity(fixture.len());
    for (i, (name, (sx, sy, sw), sinks)) in fixture.iter().enumerate() {
        let dbel = format!("D{i}");
        db.add_pin_wire(site(), &dbel, "Q", &format!("INT_X{sx}Y{sy}"), sw)
            .unwrap();
        let mut rsinks = Vec::with_capacity(sinks.len());
        for (j, (tx, ty, tw)) in sinks.iter().enumerate() {
            let sbel = format!("S{i}_{j}");
            db.add_pin_wire(site(), &sbel, "I", &format!("INT_X{tx}Y{ty}"), tw)
                .unwrap();
            rsinks.push((site(), sbel, "I".to_string()));
        }
        nets.push(RoutableNet {
            name: name.to_string(),
            driver: (site(), dbel, "Q".to_string()),
            sinks: rsinks,
            is_clock: false,
        });
    }
    nets
}

fn run_case(
    tag: &str,
    fixture: &[(&str, (i32, i32, &str), &[(i32, i32, &str)])],
    width: i32,
    forbidden: &[((i32, i32), &str, &str)],
    opts: UltraRouteOpts,
) -> u32 {
    let mut db = build_db(width, 2, forbidden);
    let nets = build_nets(&mut db, fixture);
    match route_ultra(&nets, &db, opts) {
        Ok(r) => {
            println!(
                "{tag}: ROUTED iters_used={} overused={}",
                r.iters_used, r.overused_nodes
            );
            assert_eq!(r.overused_nodes, 0, "{tag}: routed with overuse");
            r.iters_used
        }
        Err(e) => panic!("{tag}: FAILED {e}"),
    }
}

#[test]
fn n48_counter12_seed0_routes_within_budget() {
    // Tile-order anchors: wire ids from the flow logs must reproduce.
    let db = build_db(7, 2, &[]);
    assert_eq!(db.wire_id("INT_X1Y0", "BYPASS_W3"), Some(1508));
    assert_eq!(
        db.wire_id("INT_X3Y0", "INT_NODE_IMUX_53_INT_OUT0"),
        Some(4914)
    );
    assert_eq!(
        db.wire_id("INT_X1Y0", "INT_NODE_IMUX_62_INT_OUT1"),
        Some(1980)
    );
    let forbidden = [
        ((1, 0), "LOGIC_OUTS_W22", "INT_NODE_SDQ_80_INT_OUT1"),
        ((1, 0), "VCC_WIRE", "CTRL_W5"),
        ((3, 0), "VCC_WIRE", "CTRL_W4"),
    ];
    let iters = run_case(
        "c12-s0 default",
        C12_S0_NETS,
        7,
        &forbidden,
        UltraRouteOpts {
            seed: 0,
            max_iters: 50,
        },
    );
    assert!(
        iters <= N48_ITERS_BOUND,
        "c12-s0 took {iters} iterations (bound {N48_ITERS_BOUND})"
    );
}

#[test]
fn n48_counter16_seed0_routes_within_budget() {
    let db = build_db(8, 2, &[]);
    assert_eq!(db.wire_id("INT_X1Y0", "BYPASS_W8"), Some(1513));
    assert_eq!(
        db.wire_id("INT_X5Y0", "INT_NODE_IMUX_53_INT_OUT0"),
        Some(7864)
    );
    assert_eq!(
        db.wire_id("INT_X6Y0", "INT_NODE_GLOBAL_2_INT_OUT1"),
        Some(9238)
    );
    let forbidden = [
        ((1, 0), "LOGIC_OUTS_W22", "INT_NODE_SDQ_80_INT_OUT1"),
        ((1, 0), "VCC_WIRE", "CTRL_W5"),
        ((3, 0), "VCC_WIRE", "CTRL_W4"),
        ((5, 0), "VCC_WIRE", "CTRL_W4"),
    ];
    let iters = run_case(
        "c16-s0 default",
        C16_S0_NETS,
        8,
        &forbidden,
        UltraRouteOpts {
            seed: 0,
            max_iters: 50,
        },
    );
    assert!(
        iters <= N48_ITERS_BOUND,
        "c16-s0 took {iters} iterations (bound {N48_ITERS_BOUND})"
    );
}
