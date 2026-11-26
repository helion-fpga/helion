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

// --- INT_INTF tile model (DSP/BRAM routing) ---

/// Real DSP/BRAM geometry used by the INT_INTF tests, from the
/// xczu3eg tilegrid: DSP_X13Y0 at grid (219,185) with sites
/// DSP48E2_X1Y0/X1Y1, flanked west by INT_INTF_R_X13Y0..Y4 at grid
/// (218,185..181), which stitch to INT_X13Y0..Y4 at grid (217,185..181).
/// The CLE slice next door is SLICE_X19Y0 in CLEM_X13Y0 at grid
/// (216,185), partnering INT_X13Y0. BRAM_X12Y0 at grid (210,185) with
/// sites RAMB18_X2Y0 (RAMBFIFO18), RAMB18_X2Y1 (RAMB181),
/// RAMB36_X2Y0 (RAMBFIFO36), flanked east by INT_INTF_L_X12Y0..Y4 at
/// grid (211,185..181), stitching to INT_X12Y0..Y4 at grid (212,185..181);
/// the CLE slice next door is SLICE_X18Y0 in CLEL_R_X12Y0 at grid (213,185).
fn dsp_site0() -> UltraSite {
    UltraSite {
        x: 1,
        y: 0,
        kind: 2,
    }
}

fn dsp_site1() -> UltraSite {
    UltraSite {
        x: 1,
        y: 1,
        kind: 2,
    }
}

fn bram18_site() -> UltraSite {
    // RAMB18_X2Y0 (RAMBFIFO18) and RAMB36_X2Y0 (RAMBFIFO36) share
    // these coords: the 36-bit site is a view over the two 18K halves,
    // so the two views are never in one graph together.
    UltraSite {
        x: 2,
        y: 0,
        kind: 3,
    }
}

fn bram181_site() -> UltraSite {
    UltraSite {
        x: 2,
        y: 1,
        kind: 3,
    }
}

fn slice_x19y0() -> UltraSite {
    UltraSite {
        x: 19,
        y: 0,
        kind: 0,
    }
}

fn slice_x18y0() -> UltraSite {
    UltraSite {
        x: 18,
        y: 0,
        kind: 0,
    }
}

/// Every counted DSP/BRAM pin is edged in the graph: build one graph
/// per site type and count the pins that resolve. The counts are the
/// loader's intf_partner numbers, now proven edged instead of merely
/// counted: DSP 472, RAMB181 131, RAMBFIFO36 81, RAMBFIFO18 166
/// (850 total). The two DSP sites share pin names, so the count runs
/// per site template; the three BRAM views share one tile, so each
/// gets its own graph (a real design places one view per tile).
#[test]
fn tileconn_intintf_pins_edge_into_graph() {
    if db_root().is_none() {
        println!("tileconn_route: skipping, HELION_TILECONN_DB_ROOT is unset or absent");
        return;
    }
    let db = load_db().unwrap();
    // DSP: both sites of DSP_X13Y0 in one graph.
    let wdb = TileconnWireDb::from_db_and_sites(
        &tile_json(),
        db,
        &[
            (dsp_site0(), "DSP48E2_X1Y0".into()),
            (dsp_site1(), "DSP48E2_X1Y1".into()),
        ],
    )
    .unwrap();
    let dsp_file = wdb.tileconn_db().tile_types.get("DSP").unwrap();
    let mut resolved = 0usize;
    let mut pins = 0usize;
    for (site, name) in [(dsp_site0(), "DSP48E2_X1Y0"), (dsp_site1(), "DSP48E2_X1Y1")] {
        let template = dsp_file
            .sites
            .iter()
            .find(|s| {
                s.site_type == "DSP48E2"
                    && ((name.ends_with("X1Y0") && s.name == "X0Y0")
                        || (name.ends_with("X1Y1") && s.name == "X0Y1"))
            })
            .unwrap();
        for (pin, info) in &template.site_pins {
            if info.is_none() {
                continue;
            }
            pins += 1;
            if wdb.wire_for_bel_pin(site, pin, pin).is_some() {
                resolved += 1;
            }
        }
    }
    assert_eq!(pins, 864);
    assert_eq!(resolved, 472);
    assert_eq!(
        wdb.tileconn_db().coverage("DSP", "DSP48E2").intf_partner,
        472
    );
    // Spot checks, verbatim from tileconn: A0 is an input (IMUX),
    // P0 an output (LOGIC_OUTS_R); site 1 uses different wires.
    assert_eq!(
        wdb.tileconn_db().int_tile_for_site("DSP48E2_X1Y0", "A0"),
        Some(("INT_INTF_R_X13Y0".to_string(), "IMUX_FT0_1".to_string()))
    );
    assert_eq!(
        wdb.tileconn_db().int_tile_for_site("DSP48E2_X1Y0", "P0"),
        Some(("INT_INTF_R_X13Y0".to_string(), "LOGIC_OUTS_R2".to_string()))
    );
    assert_eq!(
        wdb.tileconn_db().int_tile_for_site("DSP48E2_X1Y1", "A0"),
        Some(("INT_INTF_R_X13Y1".to_string(), "IMUX_FT0_24".to_string()))
    );
    // BRAM: one graph per view.
    for (site_type, site_name, site, want) in [
        ("RAMB181", "RAMB18_X2Y1", bram181_site(), 131),
        ("RAMBFIFO36", "RAMB36_X2Y0", bram18_site(), 81),
        ("RAMBFIFO18", "RAMB18_X2Y0", bram18_site(), 166),
    ] {
        let db = load_db().unwrap();
        let wdb =
            TileconnWireDb::from_db_and_sites(&tile_json(), db, &[(site, site_name.to_string())])
                .unwrap();
        let bram_file = wdb.tileconn_db().tile_types.get("BRAM").unwrap();
        let template = bram_file
            .sites
            .iter()
            .find(|s| s.site_type == site_type)
            .unwrap();
        let mut resolved = 0usize;
        let mut pins = 0usize;
        for (pin, info) in &template.site_pins {
            if info.is_none() {
                continue;
            }
            pins += 1;
            if wdb.wire_for_bel_pin(site, pin, pin).is_some() {
                resolved += 1;
            }
        }
        assert_eq!(resolved, want, "{site_type} resolved pins");
        assert_eq!(
            wdb.tileconn_db().coverage("BRAM", site_type).intf_partner,
            want,
            "{site_type} intf_partner"
        );
        println!("tileconn_route: {site_type} pins={pins} edged={resolved}");
    }
    // BRAM spot checks, verbatim from tileconn.
    let db = load_db().unwrap();
    assert_eq!(
        db.int_tile_for_site("RAMB18_X2Y1", "ADDRAU1"),
        Some(("INT_INTF_L_X12Y0".to_string(), "IMUX_FT0_13".to_string()))
    );
    assert_eq!(
        db.int_tile_for_site("RAMB18_X2Y1", "DOAU0"),
        Some(("INT_INTF_L_X12Y2".to_string(), "LOGIC_OUTS_L27".to_string()))
    );
    assert_eq!(
        db.int_tile_for_site("RAMB36_X2Y0", "ADDRAL0"),
        Some(("INT_INTF_L_X12Y0".to_string(), "IMUX_FT0_8".to_string()))
    );
    assert_eq!(
        db.int_tile_for_site("RAMB18_X2Y0", "DOAL0"),
        Some(("INT_INTF_L_X12Y0".to_string(), "LOGIC_OUTS_L0".to_string()))
    );
}

/// A LUT driving a DSP input pin routes through the INT fabric and
/// one stitch, and translates to INT PIPs only: input pins need no
/// INT_INTF PIP. Every emitted INT PIP exists in segbits_int.db.
#[test]
fn tileconn_dsp_input_route_translates_without_intintf_pip() {
    let Some(root) = db_root() else {
        println!("tileconn_route: skipping, HELION_TILECONN_DB_ROOT is unset or absent");
        return;
    };
    let db = load_db().unwrap();
    let wdb = TileconnWireDb::from_db_and_sites(
        &tile_json(),
        db,
        &[
            (slice_x19y0(), "SLICE_X19Y0".into()),
            (dsp_site0(), "DSP48E2_X1Y0".into()),
        ],
    )
    .unwrap();
    let nets = vec![RoutableNet {
        name: "a".to_string(),
        driver: (slice_x19y0(), "A6LUT".to_string(), "O6".to_string()),
        sinks: vec![(dsp_site0(), "A0".to_string(), "A0".to_string())],
        is_clock: false,
    }];
    let routed = route_ultra(&nets, &wdb, UltraRouteOpts::default()).unwrap();
    assert_eq!(routed.routes.len(), 1);
    let route = &routed.routes[0];
    let names: Vec<(&str, &str)> = route
        .iter()
        .map(|w| {
            let info = wdb.wire_info(*w).unwrap();
            (info.tile, info.name)
        })
        .collect();
    println!("tileconn_route: LUT to DSP A0 route: {names:?}");
    assert_eq!(names.first(), Some(&("INT_X13Y0", "LOGIC_OUTS_W0")));
    assert_eq!(names.last(), Some(&("INT_INTF_R_X13Y0", "IMUX_FT0_1")));
    assert!(
        names.iter().any(|(t, _)| t.starts_with("INT_X")),
        "route never leaves the INT fabric: {names:?}"
    );
    let translated = wdb.translate_route(route, "dsp-input").unwrap();
    assert!(translated.continuations >= 1, "route crosses a stitch");
    let fasm = translated_to_fasm(std::slice::from_ref(&translated));
    println!("tileconn_route: LUT to DSP A0 FASM:\n{fasm}");
    assert!(
        !fasm.contains("INT_INTF"),
        "input route must not program an INT_INTF PIP:\n{fasm}"
    );
    let segbits = std::fs::read_to_string(root.join("segbits_int.db")).unwrap();
    for pip in &translated.pips {
        let feature = format!("INT.PIP.{}.{} ", pip.dst, pip.src);
        assert!(
            segbits.contains(&feature),
            "emitted PIP {feature} is missing from segbits_int.db"
        );
    }
}

/// Same as the DSP input route, for a BRAM input pin: LUT O6 to
/// RAMB181 ADDRAU1, through INT_X12Y0 and one stitch.
#[test]
fn tileconn_bram_input_route_translates_without_intintf_pip() {
    let Some(root) = db_root() else {
        println!("tileconn_route: skipping, HELION_TILECONN_DB_ROOT is unset or absent");
        return;
    };
    let db = load_db().unwrap();
    let wdb = TileconnWireDb::from_db_and_sites(
        &tile_json(),
        db,
        &[
            (slice_x18y0(), "SLICE_X18Y0".into()),
            (bram181_site(), "RAMB18_X2Y1".into()),
        ],
    )
    .unwrap();
    let nets = vec![RoutableNet {
        name: "addr".to_string(),
        driver: (slice_x18y0(), "A6LUT".to_string(), "O6".to_string()),
        sinks: vec![(bram181_site(), "ADDRAU1".to_string(), "ADDRAU1".to_string())],
        is_clock: false,
    }];
    let routed = route_ultra(&nets, &wdb, UltraRouteOpts::default()).unwrap();
    assert_eq!(routed.routes.len(), 1);
    let route = &routed.routes[0];
    let names: Vec<(&str, &str)> = route
        .iter()
        .map(|w| {
            let info = wdb.wire_info(*w).unwrap();
            (info.tile, info.name)
        })
        .collect();
    println!("tileconn_route: LUT to BRAM ADDRAU1 route: {names:?}");
    assert_eq!(names.first(), Some(&("INT_X12Y0", "LOGIC_OUTS_E0")));
    assert_eq!(names.last(), Some(&("INT_INTF_L_X12Y0", "IMUX_FT0_13")));
    let translated = wdb.translate_route(route, "bram-input").unwrap();
    assert!(translated.continuations >= 1, "route crosses a stitch");
    let fasm = translated_to_fasm(std::slice::from_ref(&translated));
    println!("tileconn_route: LUT to BRAM ADDRAU1 FASM:\n{fasm}");
    assert!(
        !fasm.contains("INT_INTF"),
        "input route must not program an INT_INTF PIP:\n{fasm}"
    );
    let segbits = std::fs::read_to_string(root.join("segbits_int.db")).unwrap();
    for pip in &translated.pips {
        let feature = format!("INT.PIP.{}.{} ", pip.dst, pip.src);
        assert!(
            segbits.contains(&feature),
            "emitted PIP {feature} is missing from segbits_int.db"
        );
    }
}

/// A DSP output pin routes through the LOGIC_OUTS pass-through PIP,
/// which translates to an INT_INTF_R PIP FASM line. That PIP has no
/// public segbits: the family root ships no segbits_int_intf_r.db
/// (only segbits_int_intf_left_term_pss.db exists), so bitgen would
/// fail loud on it. This test documents the block: the route and the
/// translation work, the bitstream emission does not (B1 class).
#[test]
fn tileconn_dsp_output_route_needs_intintf_pip_without_segbits() {
    let Some(root) = db_root() else {
        println!("tileconn_route: skipping, HELION_TILECONN_DB_ROOT is unset or absent");
        return;
    };
    let db = load_db().unwrap();
    let wdb = TileconnWireDb::from_db_and_sites(
        &tile_json(),
        db,
        &[
            (slice_x19y0(), "SLICE_X19Y0".into()),
            (dsp_site0(), "DSP48E2_X1Y0".into()),
        ],
    )
    .unwrap();
    let nets = vec![RoutableNet {
        name: "p".to_string(),
        driver: (dsp_site0(), "P0".to_string(), "P0".to_string()),
        sinks: vec![(slice_x19y0(), "AQ".to_string(), "D".to_string())],
        is_clock: false,
    }];
    let routed = route_ultra(&nets, &wdb, UltraRouteOpts::default()).unwrap();
    assert_eq!(routed.routes.len(), 1);
    let route = &routed.routes[0];
    let names: Vec<(&str, &str)> = route
        .iter()
        .map(|w| {
            let info = wdb.wire_info(*w).unwrap();
            (info.tile, info.name)
        })
        .collect();
    println!("tileconn_route: DSP P0 to FF D route: {names:?}");
    assert_eq!(names.first(), Some(&("INT_INTF_R_X13Y0", "LOGIC_OUTS_R2")));
    assert_eq!(names.last(), Some(&("INT_X13Y0", "BOUNCE_W_0_FT1")));
    let translated = wdb.translate_route(route, "dsp-output").unwrap();
    assert!(translated.continuations >= 1, "route crosses a stitch");
    let fasm = translated_to_fasm(std::slice::from_ref(&translated));
    println!("tileconn_route: DSP P0 to FF D FASM:\n{fasm}");
    let intintf_pips: Vec<_> = translated
        .pips
        .iter()
        .filter(|p| p.tile.starts_with("INT_INTF"))
        .collect();
    assert_eq!(intintf_pips.len(), 1, "exactly one INT_INTF PIP: {fasm}");
    assert_eq!(intintf_pips[0].tile, "INT_INTF_R_X13Y0");
    assert_eq!(intintf_pips[0].src, "LOGIC_OUTS_R2");
    assert_eq!(intintf_pips[0].dst, "LOGIC_OUTS_L2");
    // No public segbits cover this PIP: there is no
    // segbits_int_intf_r.db in the family root, and the one
    // INT_INTF segbits file that exists (left_term_pss) names a
    // different tile type.
    assert!(!root.join("segbits_int_intf_r.db").is_file());
    assert!(!root.join("segbits_int_intf_l.db").is_file());
    let pss = std::fs::read_to_string(root.join("segbits_int_intf_left_term_pss.db")).unwrap();
    assert!(
        !pss.contains("INT_INTF_R.PIP.LOGIC_OUTS_L2.LOGIC_OUTS_R2"),
        "left_term_pss segbits must not name the INT_INTF_R PIP"
    );
}

/// Wire verification for the INT_INTF model, to the US-01 standard:
/// every new edge is checked against tileconn.json re-parsed from
/// disk (not the loader's own index), with 0 contradictions. Totals
/// re-derived here: 850 pin edges (472 DSP + 131 RAMB181 + 81
/// RAMBFIFO36 + 166 RAMBFIFO18), 1,878 distinct INT_INTF to INT
/// stitch pairs, 864 INT_INTF PIPs across all tile_type_INT_INTF_*
/// files, every PIP's src/dst in its tile's wire list, 0 pseudo PIPs.
#[test]
fn tileconn_intintf_wire_verification() {
    use std::collections::HashSet;
    let Some(root) = db_root() else {
        println!("tileconn_route: skipping, HELION_TILECONN_DB_ROOT is unset or absent");
        return;
    };
    let db = load_db().unwrap();
    // Re-parse tileconn.json into the raw pair set, both directions
    // as stored: (type_a, wire_a, type_b, wire_b, dx, dy).
    #[derive(serde::Deserialize)]
    struct RawEntry {
        grid_deltas: [i32; 2],
        tile_types: [String; 2],
        wire_pairs: Vec<[String; 2]>,
    }
    let raw: Vec<RawEntry> = serde_json::from_str(
        &std::fs::read_to_string(root.join(PART).join("tileconn.json")).unwrap(),
    )
    .unwrap();
    let mut pairs: HashSet<(String, String, String, String, i32, i32)> = HashSet::new();
    for e in &raw {
        let (dx, dy) = (e.grid_deltas[0], e.grid_deltas[1]);
        for [wa, wb] in &e.wire_pairs {
            pairs.insert((
                e.tile_types[0].clone(),
                wa.clone(),
                e.tile_types[1].clone(),
                wb.clone(),
                dx,
                dy,
            ));
        }
    }
    // Every counted pin edge, via the loader's resolution, checked
    // against the raw pair set. The site wire comes from the tile-type
    // template (external form, _PIN stripped); the delta from the site
    // tile grid to the partner tile grid.
    let grid_of: std::collections::HashMap<String, (i32, i32)> = db
        .tile_by_grid
        .iter()
        .map(|((gx, gy), (name, _))| (name.clone(), (*gx, *gy)))
        .collect();
    let mut checked = 0usize;
    for (tile_type, site_type, site_name) in [
        ("DSP", "DSP48E2", "DSP48E2_X1Y0"),
        ("DSP", "DSP48E2", "DSP48E2_X1Y1"),
        ("BRAM", "RAMB181", "RAMB18_X2Y1"),
        ("BRAM", "RAMBFIFO36", "RAMB36_X2Y0"),
        ("BRAM", "RAMBFIFO18", "RAMB18_X2Y0"),
    ] {
        let file = db.tile_types.get(tile_type).unwrap();
        let template = file
            .sites
            .iter()
            .find(|s| {
                s.site_type == site_type
                    && match (tile_type, site_type, site_name) {
                        ("DSP", _, "DSP48E2_X1Y0") => s.name == "X0Y0",
                        ("DSP", _, "DSP48E2_X1Y1") => s.name == "X0Y1",
                        ("BRAM", "RAMB181", _) => s.name == "X0Y1",
                        _ => s.name == "X0Y0",
                    }
            })
            .unwrap();
        let st = db.site_tiles.get(site_name).unwrap();
        for (pin, info) in &template.site_pins {
            let Some(info) = info else { continue };
            let Some((tile, wire)) = db.int_tile_for_site(site_name, pin) else {
                continue;
            };
            let ext = info.wire.strip_suffix("_PIN").unwrap_or(&info.wire);
            let (tx, ty) = grid_of[&tile];
            let (sx, sy) = (st.grid_x, st.grid_y);
            let ptype = db.tile_by_grid[&(tx, ty)].1.clone();
            // Either stored orientation counts: the pair is in
            // tileconn.json if the forward tuple or its reverse
            // (swapped types/wires, negated delta) is present.
            let fwd = (
                tile_type.to_string(),
                ext.to_string(),
                ptype.clone(),
                wire.clone(),
                tx - sx,
                ty - sy,
            );
            let rev = (
                ptype.clone(),
                wire.clone(),
                tile_type.to_string(),
                ext.to_string(),
                sx - tx,
                sy - ty,
            );
            assert!(
                pairs.contains(&fwd) || pairs.contains(&rev),
                "pin edge {tile_type}/{site_type} {pin} {ext} -> {ptype} {wire} contradicts tileconn.json"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 850, "pin edges checked");
    // Every INT_INTF to INT stitch pair: the intf wire exists in its
    // tile JSON, the INT wire exists in the INT tile JSON.
    let int_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("data")
                .join("tile_type_INT.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let int_wires: HashSet<&str> = int_json["wires"]
        .as_object()
        .unwrap()
        .keys()
        .map(|s| s.as_str())
        .collect();
    let mut stitch_pairs = 0usize;
    let mut intf_pips = 0usize;
    let mut intf_types: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(root.join("tile_types")).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        if let Some(t) = name
            .strip_prefix("tile_type_INT_INTF_")
            .and_then(|s| s.strip_suffix(".json"))
        {
            intf_types.push(format!("INT_INTF_{t}"));
        }
    }
    intf_types.sort();
    for t in &intf_types {
        let j: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("tile_types").join(format!("tile_type_{t}.json")))
                .unwrap(),
        )
        .unwrap();
        let wires: HashSet<&str> = j["wires"]
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        for (key, pip) in j["pips"].as_object().unwrap() {
            let src = pip["src_wire"].as_str().unwrap();
            let dst = pip["dst_wire"].as_str().unwrap();
            assert!(
                wires.contains(src),
                "{t} PIP {key} src {src} missing from wires"
            );
            assert!(
                wires.contains(dst),
                "{t} PIP {key} dst {dst} missing from wires"
            );
            assert_eq!(
                pip["is_pseudo"].as_str().unwrap(),
                "0",
                "{t} PIP {key} is pseudo"
            );
            intf_pips += 1;
        }
        // Entries store the INT side first ("INT", "INT_INTF_*"):
        // wa is the INT wire, wb the INT_INTF wire.
        for (ta, wa, tb, wb, _, _) in pairs.iter() {
            if ta == "INT" && tb == t {
                assert!(wires.contains(wb.as_str()), "{t} stitch wire {wb} missing");
                assert!(
                    int_wires.contains(wa.as_str()),
                    "INT stitch wire {wa} missing"
                );
                stitch_pairs += 1;
            }
        }
    }
    assert_eq!(stitch_pairs, 1_878, "INT_INTF to INT stitch pairs");
    assert_eq!(intf_pips, 864, "INT_INTF PIPs across all tile types");
    println!(
        "tileconn_route: wire verification: {checked} pin edges, {stitch_pairs} stitch pairs, {intf_pips} INT_INTF PIPs, 0 contradictions"
    );
}
