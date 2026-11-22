//! Tileconn wire DB, end to end on the real public DBs (US-01 wiredb lane).
//!
//! These tests need the prjuray-db family root, pointed at by
//! `HELION_TILECONN_DB_ROOT` (the same directory the `helion-bitgen`
//! decoder takes as `--db-root`, e.g.
//! `~/workspace/prjuray-db-ref/zynqusp`). When the variable is unset
//! or the directory is absent, every test in this file skips with a
//! printed note instead of failing: the 84 MB `tileconn.json` is
//! runtime data, not vendored test input. Run with the variable set
//! to exercise the real loader:
//!   HELION_TILECONN_DB_ROOT=~/workspace/prjuray-db-ref/zynqusp \
//!     cargo test --release -j1 -p helion-ultrascale --test tileconn_route
//!
//! What is pinned here: the real `tileconn.json` entry/pair counts,
//! the per-site-type coverage, the known BEL-pin resolutions
//! (CLEL_R A1 to IMUX_E10 and friends, verbatim from the DBs), and
//! the blinky q net routing on the loaded map and translating to
//! PIPs that exist in `segbits_int.db`.

use helion_ultrascale::route_to_pip::translated_to_fasm;
use helion_ultrascale::{
    PlacedUltraSlice, RoutableNet, TileconnDb, TileconnWireDb, UltraPlaced, UltraRouteOpts,
    UltraSite, WireDb, route_ultra,
};
use std::path::PathBuf;

const PART: &str = "xczu3eg-sbva484-1-e";

fn tile_json() -> String {
    std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("data")
            .join("tile_type_INT.json"),
    )
    .unwrap()
}

/// The prjuray-db family root, or `None` when the tests must skip.
fn db_root() -> Option<PathBuf> {
    let root = std::env::var("HELION_TILECONN_DB_ROOT").ok()?;
    if root.is_empty() {
        return None;
    }
    let path = PathBuf::from(root);
    if path.is_dir() {
        Some(path)
    } else {
        None
    }
}

fn load_db() -> Option<TileconnDb> {
    let root = db_root()?;
    match TileconnDb::load(&root, PART) {
        Ok(db) => Some(db),
        Err(e) => panic!("tileconn load failed for part {PART} under {}: {e}", root.display()),
    }
}

fn blinky_placed() -> UltraPlaced {
    UltraPlaced {
        slices: vec![PlacedUltraSlice {
            packed_idx: 0,
            site: "SLICE_X0Y0".to_string(),
            tile: "CLB_X0Y0".to_string(),
            x: 0,
            y: 0,
        }],
        seed: 0,
    }
}

fn site0() -> UltraSite {
    UltraSite {
        x: 0,
        y: 0,
        kind: 0,
    }
}

#[test]
fn tileconn_real_counts_and_coverage() {
    let Some(db) = load_db() else {
        println!("tileconn_route: skipping, HELION_TILECONN_DB_ROOT is unset or absent");
        return;
    };
    assert_eq!(db.tileconn_entries, 81_640);
    assert_eq!(db.tileconn_pairs, 658_732);
    assert!(db.missing_tile_types.is_empty(), "missing: {:?}", db.missing_tile_types);
    // CLE coverage: every pin stitches somewhere; all but CIN/COUT
    // (carry chain, never INT) reach INT fabric.
    let slicel = db.coverage("CLEL_L", "SLICEL");
    assert_eq!((slicel.pins, slicel.any_partner, slicel.int_partner), (106, 106, 104));
    let slicel_r = db.coverage("CLEL_R", "SLICEL");
    assert_eq!((slicel_r.pins, slicel_r.any_partner, slicel_r.int_partner), (106, 106, 104));
    let slicem = db.coverage("CLEM", "SLICEM");
    assert_eq!((slicem.pins, slicem.any_partner, slicem.int_partner), (108, 108, 106));
    let slicem_r = db.coverage("CLEM_R", "SLICEM");
    assert_eq!((slicem_r.pins, slicem_r.any_partner, slicem_r.int_partner), (108, 108, 106));
    // DSP/BRAM coverage as the loader reads it: DSP pins that stitch
    // only to a neighboring DSP tile (cascade) or nowhere have no INT
    // partner, and the RAMBFIFO36 site is a view over the two 18K
    // halves, so most of its CORE_3 wires never appear in tileconn.
    // The DSP/BRAM pins that do reach fabric stitch to INT_INTF
    // tiles (interface columns), not plain INT tiles, so they are
    // counted under intf_partner, not int_partner. These numbers pin
    // the loader; they do not block CLE routing.
    let dsp = db.coverage("DSP", "DSP48E2");
    assert_eq!((dsp.pins, dsp.any_partner, dsp.int_partner, dsp.intf_partner), (864, 668, 0, 472));
    let ramb18 = db.coverage("BRAM", "RAMB181");
    assert_eq!((ramb18.pins, ramb18.any_partner, ramb18.int_partner, ramb18.intf_partner), (203, 203, 0, 131));
    let ramb36 = db.coverage("BRAM", "RAMBFIFO36");
    assert_eq!((ramb36.pins, ramb36.any_partner, ramb36.int_partner, ramb36.intf_partner), (533, 88, 0, 81));
    let ramb18b = db.coverage("BRAM", "RAMBFIFO18");
    assert_eq!((ramb18b.pins, ramb18b.any_partner, ramb18b.int_partner, ramb18b.intf_partner), (242, 242, 0, 166));
    // No CLE pin touches an INT_INTF tile either: all 8,820 CLE
    // tiles partner plain INT tiles.
    for (tt, st) in [("CLEL_L", "SLICEL"), ("CLEL_R", "SLICEL"), ("CLEM", "SLICEM"), ("CLEM_R", "SLICEM")] {
        assert_eq!(db.coverage(tt, st).intf_partner, 0, "{tt}/{st} has INT_INTF partners");
    }
    // No pin the router can ask for is ambiguous: every CLE, DSP,
    // and BRAM pin has at most one INT edge.
    for (tt, st) in [
        ("CLEL_L", "SLICEL"),
        ("CLEL_R", "SLICEL"),
        ("CLEM", "SLICEM"),
        ("CLEM_R", "SLICEM"),
        ("DSP", "DSP48E2"),
        ("BRAM", "RAMB181"),
        ("BRAM", "RAMBFIFO36"),
        ("BRAM", "RAMBFIFO18"),
    ] {
        assert_eq!(db.coverage(tt, st).ambiguous, 0, "{tt}/{st} has ambiguous pins");
    }
    println!(
        "tileconn_route: ambiguous pins (IO/clock/config, no single edge): {}",
        db.total_ambiguous()
    );
}

#[test]
fn tileconn_known_cle_resolutions() {
    let Some(db) = load_db() else {
        println!("tileconn_route: skipping, HELION_TILECONN_DB_ROOT is unset or absent");
        return;
    };
    // Worked examples from the US-01 characterization report, Q3.
    let a1 = db.pin_wire_for_pin("CLEL_R", "A1").unwrap();
    assert_eq!(a1.int_wire, "IMUX_E10");
    assert_eq!(a1.delta, (-1, 0));
    assert_eq!(a1.site_wire, "CLE_CLE_L_SITE_0_A1");
    let a1l = db.pin_wire_for_pin("CLEL_L", "A1").unwrap();
    assert_eq!(a1l.int_wire, "IMUX_W10");
    assert_eq!(a1l.delta, (1, 0));
    assert_eq!(
        db.pin_wire_for_pin("CLEL_R", "A_O").unwrap().int_wire,
        "LOGIC_OUTS_E0"
    );
    assert_eq!(
        db.pin_wire_for_pin("CLEL_R", "AQ").unwrap().int_wire,
        "LOGIC_OUTS_E14"
    );
    assert_eq!(
        db.pin_wire_for_pin("CLEL_R", "AQ2").unwrap().int_wire,
        "LOGIC_OUTS_E23"
    );
    assert_eq!(
        db.pin_wire_for_pin("CLEL_R", "AX").unwrap().int_wire,
        "BOUNCE_E_0_FT1"
    );
    assert_eq!(
        db.pin_wire_for_pin("CLEL_R", "CLK1").unwrap().int_wire,
        "CTRL_E4"
    );
    assert_eq!(
        db.pin_wire_for_pin("CLEM", "A1").unwrap().int_wire,
        "IMUX_W10"
    );
    // Carry pins never reach INT fabric.
    assert_eq!(db.pin_wire_for_pin("CLEL_R", "CIN"), None);
    assert_eq!(db.pin_wire_for_pin("CLEL_R", "COUT"), None);
    // Site to INT tile resolution.
    assert_eq!(
        db.int_tile_for_site("SLICE_X0Y0", "A1"),
        Some(("INT_X0Y0".to_string(), "IMUX_E10".to_string()))
    );
    assert_eq!(
        db.int_tile_for_site("SLICE_X0Y0", "AX"),
        Some(("INT_X0Y0".to_string(), "BOUNCE_E_0_FT1".to_string()))
    );
    // A site away from the origin resolves through the tilegrid, not
    // through the provisional INT_X{x}Y{y} naming: SLICE_X7Y0 sits
    // in CLEL_L_X5Y0 and partners INT_X5Y0.
    assert_eq!(
        db.int_tile_for_site("SLICE_X7Y0", "A1"),
        Some(("INT_X5Y0".to_string(), "IMUX_W10".to_string()))
    );
}

#[test]
fn blinky_routes_on_loaded_tileconn_map() {
    let Some(root) = db_root() else {
        println!("tileconn_route: skipping, HELION_TILECONN_DB_ROOT is unset or absent");
        return;
    };
    let wdb = TileconnWireDb::from_placed_slices(&tile_json(), &root, PART, &blinky_placed())
        .unwrap();
    assert_eq!(wdb.tile_count(), 1);
    assert_eq!(wdb.wire_count(), 1475);
    // The flow's BEL-pin names resolve to the DB's INT wires.
    assert_eq!(
        wdb.wire_for_bel_pin(site0(), "A6LUT", "O6"),
        wdb.wire_id("INT_X0Y0", "LOGIC_OUTS_E0")
    );
    assert_eq!(
        wdb.wire_for_bel_pin(site0(), "A6LUT", "I0"),
        wdb.wire_id("INT_X0Y0", "IMUX_E10")
    );
    assert_eq!(
        wdb.wire_for_bel_pin(site0(), "AQ", "D"),
        wdb.wire_id("INT_X0Y0", "BOUNCE_E_0_FT1")
    );
    assert_eq!(
        wdb.wire_for_bel_pin(site0(), "AQ", "Q"),
        wdb.wire_id("INT_X0Y0", "LOGIC_OUTS_E14")
    );
    assert_eq!(wdb.wire_for_bel_pin(site0(), "A6LUT", "I9"), None);
    assert_eq!(wdb.wire_for_bel_pin(site0(), "CARRY8", "CIN"), None);

    // The blinky q net exactly as build_routable_nets builds it.
    let nets = vec![RoutableNet {
        name: "q".to_string(),
        driver: (site0(), "A6LUT".to_string(), "O6".to_string()),
        sinks: vec![(site0(), "AQ".to_string(), "D".to_string())],
        is_clock: false,
    }];
    let routed = route_ultra(&nets, &wdb, UltraRouteOpts::default()).unwrap();
    assert_eq!(routed.routes.len(), 1);
    let route = &routed.routes[0];
    let names: Vec<&str> = route
        .iter()
        .map(|w| wdb.wire_info(*w).unwrap().name)
        .collect();
    println!("tileconn_route: blinky q route wires: {}", names.join(" -> "));
    assert_eq!(
        names,
        vec!["LOGIC_OUTS_E0", "INODE_E_1_FT1", "BOUNCE_E_0_FT1"]
    );
    let translated = wdb.translate_route(route, "blinky").unwrap();
    assert_eq!(translated.pips.len(), 2);
    let fasm = translated_to_fasm(std::slice::from_ref(&translated));
    println!("tileconn_route: blinky q PIP FASM:\n{fasm}");
    assert!(
        fasm.contains("INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1"),
        "translated FASM:\n{fasm}"
    );
    assert!(
        fasm.contains("INT_X0Y0.PIP.INODE_E_1_FT1.LOGIC_OUTS_E0"),
        "translated FASM:\n{fasm}"
    );
    // Every emitted PIP exists in segbits_int.db: the emitted PIPs
    // are real configurable PIPs, not graph artifacts.
    let segbits = std::fs::read_to_string(root.join("segbits_int.db")).unwrap();
    for pip in &translated.pips {
        // Segbits lines read `INT.PIP.<dst>.<src> <bits...>`; the
        // trailing space keeps the match exact (no wire-name prefix
        // can extend it).
        let feature = format!("INT.PIP.{}.{} ", pip.dst, pip.src);
        assert!(
            segbits.contains(&feature),
            "emitted PIP {feature} is missing from segbits_int.db"
        );
    }
}
