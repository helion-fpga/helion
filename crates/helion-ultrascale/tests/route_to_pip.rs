//! Router to PIP translator, end to end: route on the real INT graph,
//! translate to PIPs, emit against the vendored segbits DB, and the
//! blinky/counter end-to-end attempt.
//!
//! The positive tests prove the router-to-PIP-to-bits chain on real DB
//! data with caller-supplied pin mappings. The end-to-end attempt runs
//! blinky and counter through synth, pack, and place on the real
//! xcku3p device model and shows exactly where the flow stops: ROUTE
//! fails loud because no BEL pin has a wire in the graph. No public
//! source maps BEL pins to INT wires (that table is US-01 tilegrid
//! data), so the failure is a named data gap, not a code gap.

use helion_ultrascale::pack::{pack_ultra, ControlSetLimits};
use helion_ultrascale::place_ultra;
use helion_ultrascale::route_to_pip::translated_to_fasm;
use helion_ultrascale::{
    emit_pips, route_ultra, translate_routes, verify_zero_bits, Bel, BelKind, FileSegbitsDb,
    IntTile, IntTileWireDb, RoutableNet, RouteErrorKind, TileId, UltraPlaceOpts, UltraRouteOpts,
    UltraScaleDevice, WireDb,
};
use std::collections::HashMap;
use std::path::PathBuf;

fn data_file(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join(name)
}

fn tile_json() -> String {
    std::fs::read_to_string(data_file("tile_type_INT.json")).unwrap()
}

fn workspace_example(name: &str) -> String {
    let rel = format!("examples/{name}");
    if std::path::Path::new(&rel).exists() {
        return rel;
    }
    format!("{}/../../{}", env!("CARGO_MANIFEST_DIR"), rel)
}

/// A one-tile graph plus the pin mappings the positive tests supply.
/// The mappings are test-supplied, not device truth: no public source
/// maps BEL pins to INT wires.
fn blinky_graph() -> IntTileWireDb {
    let mut db = IntTileWireDb::single_tile("INT_X12Y45", 12, 45, &tile_json()).unwrap();
    let site = helion_ultrascale::UltraSite {
        x: 12,
        y: 45,
        kind: 0,
    };
    db.add_pin_wire(site, "A6LUT", "O6", "INT_X12Y45", "LOGIC_OUTS_E0")
        .unwrap();
    db.add_pin_wire(site, "AFF", "D", "INT_X12Y45", "BOUNCE_E_0_FT1")
        .unwrap();
    db
}

#[test]
fn routed_blinky_net_emits_golden_pips() {
    let db = blinky_graph();
    let site = helion_ultrascale::UltraSite {
        x: 12,
        y: 45,
        kind: 0,
    };
    let nets = vec![RoutableNet {
        name: "q".to_string(),
        driver: (site, "A6LUT".to_string(), "O6".to_string()),
        sinks: vec![(site, "AFF".to_string(), "D".to_string())],
        is_clock: false,
    }];
    let routed = route_ultra(&nets, &db, UltraRouteOpts::default()).unwrap();
    assert_eq!(routed.routes.len(), 1);
    // Physical order: LOGIC_OUTS_E0 -> INODE_E_1_FT1 -> BOUNCE_E_0_FT1.
    let names: Vec<&str> = routed.routes[0]
        .iter()
        .map(|w| db.wire_info(*w).unwrap().name)
        .collect();
    assert_eq!(
        names,
        vec!["LOGIC_OUTS_E0", "INODE_E_1_FT1", "BOUNCE_E_0_FT1"]
    );
    let translated = translate_routes(&db, &routed, "blinky").unwrap();
    assert_eq!(translated.len(), 1);
    assert_eq!(translated[0].continuations, 0);
    assert_eq!(
        translated_to_fasm(&translated),
        "INT_X12Y45.PIP.INODE_E_1_FT1.LOGIC_OUTS_E0\n\
         INT_X12Y45.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1\n"
    );
    // Emit against the vendored segbits DB and check the exact bits.
    let segbits = FileSegbitsDb::from_file(&data_file("segbits_int.db")).unwrap();
    let pips: Vec<_> = translated.iter().flat_map(|t| t.pips.clone()).collect();
    let emitted = emit_pips("blinky", &pips, &segbits).unwrap();
    assert_eq!(emitted.len(), 2);
    assert_eq!(
        emitted[0].fasm_line,
        "INT_X12Y45.PIP.INODE_E_1_FT1.LOGIC_OUTS_E0"
    );
    // INT.PIP.INODE_E_1_FT1.LOGIC_OUTS_E0 57_00 !59_00 !61_00 !65_01 !67_01 !69_01
    assert_eq!(emitted[0].set_bits.len(), 1);
    assert_eq!(emitted[0].set_bits[0].frame, 57);
    assert_eq!(emitted[0].zero_bits.len(), 5);
    assert_eq!(
        emitted[1].fasm_line,
        "INT_X12Y45.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1"
    );
    // The BOUNCE PIP is the golden blinky PIP (modulo tile name).
    assert_eq!(
        emitted[1].fasm_line,
        "INT_X12Y45.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1"
    );
    assert!(verify_zero_bits(&emitted, &|_, _| false).is_ok());
}

#[test]
fn multi_tile_route_skips_continuation_and_emits_remote_pip() {
    let mut db = IntTileWireDb::from_json(
        &tile_json(),
        &[
            IntTile::new("INT_X12Y45", 12, 45),
            IntTile::new("INT_X12Y57", 12, 57),
        ],
    )
    .unwrap();
    let site45 = helion_ultrascale::UltraSite {
        x: 12,
        y: 45,
        kind: 0,
    };
    let site57 = helion_ultrascale::UltraSite {
        x: 12,
        y: 57,
        kind: 0,
    };
    db.add_pin_wire(site45, "A6LUT", "O6", "INT_X12Y45", "NN12_BEG7")
        .unwrap();
    db.add_pin_wire(site57, "AFF", "D", "INT_X12Y57", "NN12_BEG7")
        .unwrap();
    let nets = vec![RoutableNet {
        name: "long".to_string(),
        driver: (site45, "A6LUT".to_string(), "O6".to_string()),
        sinks: vec![(site57, "AFF".to_string(), "D".to_string())],
        is_clock: false,
    }];
    let routed = route_ultra(&nets, &db, UltraRouteOpts::default()).unwrap();
    let names: Vec<String> = routed.routes[0]
        .iter()
        .map(|w| {
            let info = db.wire_info(*w).unwrap();
            format!("{}.{}", info.tile, info.name)
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "INT_X12Y45.NN12_BEG7",
            "INT_X12Y57.NN12_END7",
            "INT_X12Y57.NN12_BEG7",
        ]
    );
    let translated = translate_routes(&db, &routed, "blinky").unwrap();
    assert_eq!(translated[0].continuations, 1);
    assert_eq!(
        translated_to_fasm(&translated),
        "INT_X12Y57.PIP.NN12_BEG7.NN12_END7\n"
    );
    let segbits = FileSegbitsDb::from_file(&data_file("segbits_int.db")).unwrap();
    let pips: Vec<_> = translated.iter().flat_map(|t| t.pips.clone()).collect();
    assert!(emit_pips("blinky", &pips, &segbits).is_ok());
}

/// Extract packer inputs from a synthesized Design.
/// Mirrors tests/corpus_legality.rs; kept local so this test does not
/// depend on another test file's private helpers.
fn extract_cells(
    design: &helion_ir::Design,
) -> (
    Vec<(String, u64, bool)>,
    Vec<(String, String, Option<String>, Option<String>)>,
    Vec<Vec<String>>,
) {
    use helion_ir::{resolve, CellKind};

    let mut luts = Vec::new();
    let mut ffs = Vec::new();
    let mut carry_chains: Vec<Vec<String>> = Vec::new();
    let mut carry_map: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    let mut pin_to_net: HashMap<(helion_ir::Spur, helion_ir::Spur), helion_ir::Spur> =
        HashMap::new();
    for net in &design.nets {
        for ep in &net.endpoints {
            pin_to_net.insert((ep.cell, ep.pin), net.name);
        }
    }
    for cell in &design.cells {
        let name = resolve(cell.name).to_string();
        match cell.kind {
            CellKind::Lut6 { init } => {
                let is_mem = cell.attrs.get("MEMORY").map(|s| s == "1").unwrap_or(false);
                luts.push((name, init, is_mem));
            }
            CellKind::Hff => {
                let clk = pin_to_net
                    .get(&(cell.name, helion_ir::intern("CLK")))
                    .map(|s| resolve(*s).to_string())
                    .unwrap_or_else(|| "clk".to_string());
                let ce = pin_to_net
                    .get(&(cell.name, helion_ir::intern("CE")))
                    .map(|s| resolve(*s).to_string());
                let sr = pin_to_net
                    .get(&(cell.name, helion_ir::intern("SR")))
                    .map(|s| resolve(*s).to_string());
                ffs.push((name, clk, ce, sr));
            }
            CellKind::Carry4 => {
                let prev = pin_to_net
                    .get(&(cell.name, helion_ir::intern("CI")))
                    .and_then(|net| {
                        design.nets.iter().find(|n| n.name == *net).and_then(|n| {
                            n.endpoints.iter().find_map(|ep| {
                                let ep_name = resolve(ep.cell).to_string();
                                if ep_name.starts_with("u_carry")
                                    || ep.pin == helion_ir::intern("CO3")
                                {
                                    Some(ep_name)
                                } else {
                                    None
                                }
                            })
                        })
                    });
                carry_map.insert(name.clone(), (prev, None));
            }
            _ => {}
        }
    }
    let mut visited = std::collections::HashSet::new();
    for (name, (prev, _)) in &carry_map {
        if prev.is_none() && !visited.contains(name) {
            let mut chain = vec![name.clone()];
            visited.insert(name.clone());
            let mut cur = name.clone();
            loop {
                let mut found = None;
                for (n2, (p2, _)) in &carry_map {
                    if p2.as_ref() == Some(&cur) && !visited.contains(n2) {
                        found = Some(n2.clone());
                        break;
                    }
                }
                if let Some(nxt) = found {
                    chain.push(nxt.clone());
                    visited.insert(nxt.clone());
                    cur = nxt;
                } else {
                    break;
                }
            }
            if !chain.is_empty() {
                carry_chains.push(chain);
            }
        }
    }
    (luts, ffs, carry_chains)
}

/// Cell name to placed (site x, site y, BEL name). BEL names come from
/// `Bel::name` (UG574 letter convention); the pin spellings are the
/// synth netlist spellings, not verified site pin spellings. Both are
/// only used to show that ROUTE fails loud for lack of a pin-to-wire
/// mapping, never to claim a routed net.
fn place_cells(
    packed: &helion_ultrascale::pack::UltraPacked,
    placed: &helion_ultrascale::place::UltraPlaced,
) -> HashMap<String, (i32, i32, String)> {
    let mut out: HashMap<String, (i32, i32, String)> = HashMap::new();
    for ps in placed.slices.iter() {
        let packed_slice = &packed.slices[ps.packed_idx];
        let tile = TileId::new(ps.x as u32, ps.y as u32);
        for (idx, cell) in &packed_slice.luts {
            if let Some(name) = Bel::new(BelKind::Lut6, tile, *idx as u32).name() {
                out.insert(cell.clone(), (ps.x, ps.y, name));
            }
        }
        for (idx, cell) in &packed_slice.ffs {
            if let Some(name) = Bel::new(BelKind::FdRe, tile, *idx as u32).name() {
                out.insert(cell.clone(), (ps.x, ps.y, name));
            }
        }
        if let Some(carry) = &packed_slice.carry8 {
            out.insert(carry.clone(), (ps.x, ps.y, "CARRY8".to_string()));
        }
    }
    out
}

/// End-to-end attempt for one design: synth, pack, place on the real
/// xcku3p device model, then route. Pack and place succeed; route
/// fails loud with NoPath naming a BEL pin that has no wire, because
/// no public source maps BEL pins to INT wires. The test asserts that
/// exact failure so a future pin-wire table turns it green instead of
/// silently passing.
fn attempt_design(name: &str, sv_path: &str, top: &str) {
    let sv = std::fs::read_to_string(sv_path).unwrap();
    let design = helion_sv::synth_sv(&sv, top).unwrap();
    let (luts, ffs, chains) = extract_cells(&design);
    let packed = pack_ultra(&luts, &ffs, &chains, &ControlSetLimits::default()).unwrap();
    let device = UltraScaleDevice::xcku3p();
    let placed = place_ultra(
        &packed.slices,
        &device,
        &UltraPlaceOpts {
            seed: 42,
            timing_weight: 0.0,
        },
    )
    .unwrap();
    assert!(!placed.slices.is_empty(), "{name}: nothing placed");
    let cells = place_cells(&packed, &placed);
    // INT tiles for the placed slices. PROVISIONAL: INT tile X/Y equal
    // the slice X/Y (the golden blinky FASM uses INT_X0Y0 for
    // SLICE_X0Y0); no xcku3p tilegrid exists in any public source.
    let mut tiles: Vec<IntTile> = placed
        .slices
        .iter()
        .map(|s| IntTile::new(&format!("INT_X{}Y{}", s.x, s.y), s.x, s.y))
        .collect();
    tiles.sort_by_key(|t| (t.y, t.x));
    tiles.dedup_by(|a, b| a.name == b.name);
    let db = IntTileWireDb::from_json(&tile_json(), &tiles).unwrap();
    assert!(db.wire_count() > 0, "{name}: graph is empty");
    // Build routable nets from the design nets, skipping port-driven
    // nets (top-level inputs/outputs need IO routing, a separate gap).
    use helion_ir::resolve;
    let mut nets: Vec<RoutableNet> = Vec::new();
    for net in &design.nets {
        let net_name = resolve(net.name).to_string();
        let mut driver: Option<(helion_ultrascale::UltraSite, String, String)> = None;
        let mut sinks = Vec::new();
        for ep in &net.endpoints {
            let cell_name = resolve(ep.cell).to_string();
            let Some((x, y, bel)) = cells.get(&cell_name) else {
                continue;
            };
            let pin = resolve(ep.pin).to_string();
            let site = helion_ultrascale::UltraSite {
                x: *x,
                y: *y,
                kind: 0,
            };
            if pin == "O" || pin == "Q" || pin == "CO3" {
                driver = Some((site, bel.clone(), pin));
            } else {
                sinks.push((site, bel.clone(), pin));
            }
        }
        if let Some(d) = driver {
            if !sinks.is_empty() {
                nets.push(RoutableNet {
                    name: net_name,
                    driver: d,
                    sinks,
                    is_clock: false,
                });
            }
        }
    }
    assert!(!nets.is_empty(), "{name}: no cell-driven nets found");
    let err = route_ultra(&nets, &db, UltraRouteOpts::default()).unwrap_err();
    assert_eq!(
        err.kind,
        RouteErrorKind::NoPath,
        "{name}: route failed differently than expected: {}",
        err
    );
    assert!(
        err.to_string().contains("has no wire"),
        "{name}: route error does not name the missing pin wire: {}",
        err
    );
    println!(
        "{name}: synth+pack+place OK ({} slices, {} nets); ROUTE fails loud: {}",
        placed.slices.len(),
        nets.len(),
        err
    );
}

#[test]
fn blinky_and_counter_stop_at_route_without_pin_wire_map() {
    attempt_design("blinky", &workspace_example("blinky.sv"), "blinky");
    attempt_design("counter", &workspace_example("counter.sv"), "counter");
}
