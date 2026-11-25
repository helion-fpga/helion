//! INT_INTF tile model: DSP/BRAM pins edged in the routing graph
//! (follow-up to the US-01 gap: DSP/BRAM pins stitch to INT_INTF
//! tiles, not plain INT tiles, and the graph was built from the plain
//! INT tile JSON only).
//!
//! Fixture tests need no DB root: the tileconn/tilegrid/tile-type
//! files are synthetic strings written to a temp dir. The fixture
//! mirrors the real xczu3eg geometry at small scale: a DSP-like tile
//! with two sites sharing pin names (the site_index pattern), an
//! INT_INTF-like tile with LOGIC_OUTS pass-through PIPs, and the
//! stitch pairs into the plain INT tile.
//!
//! What is pinned here: site-indexed DSP registration (each site's
//! own INT_INTF wire), LUT to DSP-input routing through one stitch
//! with zero INT_INTF PIPs, DSP-output routing through the
//! LOGIC_OUTS PIP, and translation that crosses a stitch without a
//! PIP (counted as a continuation, like a span continuation).

use helion_ultrascale::route_to_pip::translated_to_fasm;
use helion_ultrascale::{
    RoutableNet, TileconnDb, TileconnWireDb, UltraRouteOpts, UltraSite, WireDb, route_ultra,
};
use std::fs;
use std::path::PathBuf;

const TILE_JSON: &str = include_str!("../data/tile_type_INT.json");

/// Synthetic tileconn: CLEL_R to INT (the TINY entries), a DSP-like
/// tile to an INT_INTF-like tile, and the INT_INTF-like tile to INT.
/// Deltas are grid deltas from the first-named tile type to the second.
const FIXTURE_TILECONN: &str = r#"[
  {"grid_deltas": [-1, 0], "tile_types": ["CLEL_R", "INT"], "wire_pairs": [
    ["CLE_CLE_L_SITE_0_A1", "IMUX_E10"],
    ["CLE_CLE_L_SITE_0_A_O", "LOGIC_OUTS_E0"],
    ["CLE_CLE_L_SITE_0_AQ", "LOGIC_OUTS_E14"],
    ["CLE_CLE_L_SITE_0_AX", "BOUNCE_E_0_FT1"]
  ]},
  {"grid_deltas": [0, 1], "tile_types": ["CLEL_R", "CLEL_R"], "wire_pairs": [
    ["CLE_CLE_L_SITE_0_CIN", "CLE_CLE_L_SITE_0_COUT"]
  ]},
  {"grid_deltas": [-1, 0], "tile_types": ["TESTDSP", "INT_INTF_R"], "wire_pairs": [
    ["DSP_DSP48_0_A0", "IMUX_FT0_1"],
    ["DSP_DSP48_0_P0", "LOGIC_OUTS_R0"],
    ["DSP_DSP48_1_A0", "IMUX_FT0_3"],
    ["DSP_DSP48_1_P0", "LOGIC_OUTS_R1"]
  ]},
  {"grid_deltas": [-2, 0], "tile_types": ["INT_INTF_R", "INT"], "wire_pairs": [
    ["IMUX_FT0_1", "BOUNCE_E_0_FT1"],
    ["IMUX_FT0_3", "BOUNCE_E_13_FT0"],
    ["LOGIC_OUTS_L0", "LOGIC_OUTS_E0"],
    ["LOGIC_OUTS_L1", "LOGIC_OUTS_E1"]
  ]}
]"#;

/// Synthetic tilegrid: two CLE rows (Y0/Y1) each with their INT tile,
/// and a DSP column on row Y1: DSP at grid x 163, its INT_INTF tile
/// at 162 (delta -1), stitching to the row-Y1 INT tile at 160
/// (delta -2). Each grid holds exactly one tile, as in the real DB:
/// the earlier draft put the CLE tile and the INT_INTF tile on the
/// same grid and tile_by_grid kept one at random.
const FIXTURE_TILEGRID: &str = r#"{
  "CLEL_R_X0Y0": {"grid_x": 161, "grid_y": 185, "type": "CLEL_R",
    "sites": {"SLICE_X0Y0": "SLICEL"}},
  "INT_X0Y0": {"grid_x": 160, "grid_y": 185, "type": "INT", "sites": {}},
  "CLEL_R_X0Y1": {"grid_x": 161, "grid_y": 184, "type": "CLEL_R",
    "sites": {"SLICE_X0Y1": "SLICEL"}},
  "INT_X0Y1": {"grid_x": 160, "grid_y": 184, "type": "INT", "sites": {}},
  "TESTDSP_X0Y0": {"grid_x": 163, "grid_y": 184, "type": "TESTDSP",
    "sites": {"DSP48E2_X0Y0": "DSP48E2", "DSP48E2_X0Y1": "DSP48E2"}},
  "INT_INTF_R_X0Y0": {"grid_x": 162, "grid_y": 184, "type": "INT_INTF_R", "sites": {}}
}"#;

const FIXTURE_TILE_TYPE_CLEL_R: &str = r#"{
  "tile_type": "CLEL_R",
  "sites": [{"name": "X0Y0", "type": "SLICEL", "site_pins": {
    "A1": {"wire": "CLE_CLE_L_SITE_0_A1"},
    "A_O": {"wire": "CLE_CLE_L_SITE_0_A_O"},
    "AQ": {"wire": "CLE_CLE_L_SITE_0_AQ"},
    "AX": {"wire": "CLE_CLE_L_SITE_0_AX"},
    "CIN": {"wire": "CLE_CLE_L_SITE_0_CIN_PIN"},
    "COUT": {"wire": "CLE_CLE_L_SITE_0_COUT"}
  }}]
}"#;

/// Two DSP48E2-like sites sharing pin names (A0 input, P0 output)
/// but with distinct site wires: the site_index pattern. The
/// INT_INTF tile type below is INT_INTF_R (no sites of its own).
const FIXTURE_TILE_TYPE_TESTDSP: &str = r#"{
  "tile_type": "TESTDSP",
  "sites": [
    {"name": "X0Y0", "type": "DSP48E2", "site_pins": {
      "A0": {"wire": "DSP_DSP48_0_A0_PIN"},
      "P0": {"wire": "DSP_DSP48_0_P0"}
    }},
    {"name": "X0Y1", "type": "DSP48E2", "site_pins": {
      "A0": {"wire": "DSP_DSP48_1_A0_PIN"},
      "P0": {"wire": "DSP_DSP48_1_P0"}
    }}
  ]
}"#;

/// The INT_INTF-like tile type: LOGIC_OUTS pass-through PIPs from the
/// DSP side (R) to the INT side (L), mirroring INT_INTF_R. Input
/// wires (IMUX_FT0_*) stitch straight through to INT wires; they
/// have no PIP.
const FIXTURE_TILE_TYPE_INT_INTF_R: &str = r#"{
  "tile_type": "INT_INTF_R",
  "pips": {
    "INT_INTF_R.LOGIC_OUTS_R0->>LOGIC_OUTS_L0": {"src_wire": "LOGIC_OUTS_R0", "dst_wire": "LOGIC_OUTS_L0", "is_directional": "1", "is_pseudo": "0"},
    "INT_INTF_R.LOGIC_OUTS_R1->>LOGIC_OUTS_L1": {"src_wire": "LOGIC_OUTS_R1", "dst_wire": "LOGIC_OUTS_L1", "is_directional": "1", "is_pseudo": "0"}
  },
  "wires": {
    "IMUX_FT0_1": null, "IMUX_FT0_3": null,
    "LOGIC_OUTS_L0": null, "LOGIC_OUTS_L1": null,
    "LOGIC_OUTS_R0": null, "LOGIC_OUTS_R1": null
  }
}"#;

fn fixture_db() -> (TempfileDir, TileconnDb) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static FIXTURE_SEQ: AtomicU64 = AtomicU64::new(0);
    let n = FIXTURE_SEQ.fetch_add(1, Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("helion-intintf-fixture-{}-{n}", std::process::id()));
    let part_dir = dir.join("xczu3eg-sbva484-1-e");
    let _ = fs::create_dir_all(&part_dir);
    fs::write(part_dir.join("tileconn.json"), FIXTURE_TILECONN).unwrap();
    fs::write(part_dir.join("tilegrid.json"), FIXTURE_TILEGRID).unwrap();
    let root_types = dir.join("tile_types");
    let _ = fs::create_dir_all(&root_types);
    fs::write(
        root_types.join("tile_type_CLEL_R.json"),
        FIXTURE_TILE_TYPE_CLEL_R,
    )
    .unwrap();
    fs::write(
        root_types.join("tile_type_TESTDSP.json"),
        FIXTURE_TILE_TYPE_TESTDSP,
    )
    .unwrap();
    fs::write(
        root_types.join("tile_type_INT_INTF_R.json"),
        FIXTURE_TILE_TYPE_INT_INTF_R,
    )
    .unwrap();
    let db = TileconnDb::load(&dir, "xczu3eg-sbva484-1-e").unwrap();
    (TempfileDir(dir), db)
}

struct TempfileDir(PathBuf);
impl Drop for TempfileDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn slice_y1() -> UltraSite {
    UltraSite {
        x: 0,
        y: 1,
        kind: 0,
    }
}

fn dsp0() -> UltraSite {
    UltraSite {
        x: 0,
        y: 0,
        kind: 2,
    }
}

fn dsp1() -> UltraSite {
    UltraSite {
        x: 0,
        y: 1,
        kind: 2,
    }
}

fn graph() -> (TempfileDir, TileconnWireDb) {
    let (dir, db) = fixture_db();
    let wdb = TileconnWireDb::from_db_and_sites(
        TILE_JSON,
        db,
        &[
            (slice_y1(), "SLICE_X0Y1".into()),
            (dsp0(), "DSP48E2_X0Y0".into()),
            (dsp1(), "DSP48E2_X0Y1".into()),
        ],
    )
    .unwrap();
    (dir, wdb)
}

#[test]
fn intintf_dsp_pins_register_site_indexed() {
    let (_dir, wdb) = graph();
    // The INT_INTF tile is in the graph (before the fix the graph
    // built without it and the DSP pins had no wire).
    assert!(wdb.wire_id("INT_INTF_R_X0Y0", "IMUX_FT0_1").is_some());
    assert!(wdb.wire_id("INT_INTF_R_X0Y0", "LOGIC_OUTS_R0").is_some());
    // Each DSP site's A0/P0 resolves to its OWN INT_INTF wire: the
    // two sites share pin names, so a registration without the site
    // index would conflate them (last wire wins).
    assert_eq!(
        wdb.wire_for_bel_pin(dsp0(), "A0", "A0"),
        wdb.wire_id("INT_INTF_R_X0Y0", "IMUX_FT0_1")
    );
    assert_eq!(
        wdb.wire_for_bel_pin(dsp0(), "P0", "P0"),
        wdb.wire_id("INT_INTF_R_X0Y0", "LOGIC_OUTS_R0")
    );
    assert_eq!(
        wdb.wire_for_bel_pin(dsp1(), "A0", "A0"),
        wdb.wire_id("INT_INTF_R_X0Y0", "IMUX_FT0_3")
    );
    assert_eq!(
        wdb.wire_for_bel_pin(dsp1(), "P0", "P0"),
        wdb.wire_id("INT_INTF_R_X0Y0", "LOGIC_OUTS_R1")
    );
    // The CLE pins on the same graph still resolve to the INT tile.
    assert_eq!(
        wdb.wire_for_bel_pin(slice_y1(), "A6LUT", "O6"),
        wdb.wire_id("INT_X0Y1", "LOGIC_OUTS_E0")
    );
    assert_eq!(
        wdb.wire_for_bel_pin(slice_y1(), "A6LUT", "I0"),
        wdb.wire_id("INT_X0Y1", "IMUX_E10")
    );
    // The stitch is a graph edge both ways: the INT wire and the
    // INT_INTF wire are the same node.
    let int_wire = wdb.wire_id("INT_X0Y1", "BOUNCE_E_0_FT1").unwrap();
    let intf_wire = wdb.wire_id("INT_INTF_R_X0Y0", "IMUX_FT0_1").unwrap();
    assert!(wdb.fanout(int_wire).contains(&intf_wire));
    assert!(wdb.fanout(intf_wire).contains(&int_wire));
    // The LOGIC_OUTS stitch is an edge too.
    let logic_int = wdb.wire_id("INT_X0Y1", "LOGIC_OUTS_E0").unwrap();
    let logic_intf = wdb.wire_id("INT_INTF_R_X0Y0", "LOGIC_OUTS_L0").unwrap();
    assert!(wdb.fanout(logic_int).contains(&logic_intf));
    assert!(wdb.fanout(logic_intf).contains(&logic_int));
}

#[test]
fn intintf_lut_to_dsp_input_routes_through_one_stitch() {
    let (_dir, wdb) = graph();
    // LUT O6 drives DSP A0 (an input pin: its INT_INTF wire stitches
    // straight to the INT wire, so the route needs no INT_INTF PIP).
    let nets = vec![RoutableNet {
        name: "a".to_string(),
        driver: (slice_y1(), "A6LUT".to_string(), "O6".to_string()),
        sinks: vec![(dsp0(), "A0".to_string(), "A0".to_string())],
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
    assert_eq!(
        names,
        vec![
            ("INT_X0Y1", "LOGIC_OUTS_E0"),
            ("INT_X0Y1", "INODE_E_1_FT1"),
            ("INT_X0Y1", "BOUNCE_E_0_FT1"),
            ("INT_INTF_R_X0Y0", "IMUX_FT0_1"),
        ]
    );
    let t = wdb.translate_route(route, "intintf-lut-to-dsp").unwrap();
    assert_eq!(t.pips.len(), 2);
    // One stitch crossed (INT to INT_INTF), no span continuation.
    assert_eq!(t.continuations, 1);
    let fasm = translated_to_fasm(std::slice::from_ref(&t));
    assert!(
        fasm.contains("INT_X0Y1.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1"),
        "translated FASM:\n{fasm}"
    );
    assert!(
        fasm.contains("INT_X0Y1.PIP.INODE_E_1_FT1.LOGIC_OUTS_E0"),
        "translated FASM:\n{fasm}"
    );
    assert!(
        !fasm.contains("INT_INTF_R"),
        "input route must not program an INT_INTF PIP:\n{fasm}"
    );
}

#[test]
fn intintf_dsp_output_routes_through_logic_outs_pip() {
    let (_dir, wdb) = graph();
    // DSP P0 (an output pin) drives the slice FF D: the route must
    // cross the LOGIC_OUTS pass-through PIP, then the stitch, then
    // the INT PIPs.
    let nets = vec![RoutableNet {
        name: "p".to_string(),
        driver: (dsp0(), "P0".to_string(), "P0".to_string()),
        sinks: vec![(slice_y1(), "AQ".to_string(), "D".to_string())],
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
    assert_eq!(
        names,
        vec![
            ("INT_INTF_R_X0Y0", "LOGIC_OUTS_R0"),
            ("INT_INTF_R_X0Y0", "LOGIC_OUTS_L0"),
            ("INT_X0Y1", "LOGIC_OUTS_E0"),
            ("INT_X0Y1", "INODE_E_1_FT1"),
            ("INT_X0Y1", "BOUNCE_E_0_FT1"),
        ]
    );
    let t = wdb.translate_route(route, "intintf-dsp-to-lut").unwrap();
    assert_eq!(t.pips.len(), 3);
    // One stitch crossed (INT_INTF to INT), no span continuation.
    assert_eq!(t.continuations, 1);
    let fasm = translated_to_fasm(std::slice::from_ref(&t));
    assert!(
        fasm.contains("INT_INTF_R_X0Y0.PIP.LOGIC_OUTS_L0.LOGIC_OUTS_R0"),
        "translated FASM:\n{fasm}"
    );
}
