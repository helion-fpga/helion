//! Legality checker on real corpus designs.
//!
//! Synthesizes 3 designs with helion-sv, packs and places them for
//! UltraScale+, and verifies legality. This is the acceptance test
//! from the pack-place brief.

use std::collections::HashMap;
use ultrascale_packplace_staging::legality::check_legality;
use ultrascale_packplace_staging::pack::{pack_ultra, ControlSetLimits, SliceKind};
use ultrascale_packplace_staging::place::{place_ultra, UltraGrid, UltraPlaceOpts};

/// Fake xcvu3p-like grid for testing.
/// Real grid comes from device-model agent's UltraScaleFabric.
struct FakeVu3pGrid {
    sites: Vec<(String, String, i32, i32, SliceKind)>,
}

impl UltraGrid for FakeVu3pGrid {
    fn slice_sites(&self) -> Vec<(String, String, i32, i32, SliceKind)> {
        self.sites.clone()
    }
    fn grid_size(&self) -> (i32, i32) {
        (100, 100)
    }
}

fn make_grid() -> FakeVu3pGrid {
    let mut sites = Vec::new();
    // 100x100 grid, every 4th column is SLICEM (approximates real distribution).
    for y in 0..100 {
        for x in 0..100 {
            let kind = if x % 4 == 0 {
                SliceKind::Memory
            } else {
                SliceKind::Logic
            };
            sites.push((
                format!("SLICE_X{x}Y{y}"),
                format!("CLB_X{x}Y{y}"),
                x,
                y,
                kind,
            ));
        }
    }
    FakeVu3pGrid { sites }
}

/// Extract packer inputs from a synthesized Design.
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

    // Build pin->net map.
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
                // Check if used as memory (SRL or dist RAM) via attributes.
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
                        // Find driver of this net.
                        design.nets.iter().find(|n| n.name == *net).and_then(|n| {
                            n.endpoints.iter().find_map(|ep| {
                                let ep_name = resolve(ep.cell).to_string();
                                if ep_name.starts_with("u_carry") || ep.pin == helion_ir::intern("CO3") {
                                    Some(ep_name)
                                } else {
                                    None
                                }
                            })
                        })
                    });
                let next = None; // Simplified: chains built from prev links.
                carry_map.insert(name.clone(), (prev, next));
            }
            _ => {}
        }
    }

    // Build chains from carry_map.
    let mut visited = std::collections::HashSet::new();
    for (name, (prev, _)) in &carry_map {
        if prev.is_none() && !visited.contains(name) {
            // Chain head.
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

fn test_design(name: &str, sv_path: &str, top: &str) {
    let sv = std::fs::read_to_string(sv_path)
        .unwrap_or_else(|_| panic!("cannot read {sv_path}"));
    let design = helion_sv::synth_sv(&sv, top)
        .unwrap_or_else(|e| panic!("synth failed for {name}: {e}"));

    let (luts, ffs, chains) = extract_cells(&design);
    println!(
        "{name}: {} LUTs, {} FFs, {} carry chains",
        luts.len(),
        ffs.len(),
        chains.len()
    );

    let limits = ControlSetLimits::default();
    let packed = pack_ultra(&luts, &ffs, &chains, &limits)
        .unwrap_or_else(|e| panic!("pack failed for {name}: {e}"));

    let grid = make_grid();
    let opts = UltraPlaceOpts {
        seed: 42,
        timing_weight: 0.0,
    };
    let placed = place_ultra(&packed.slices, &grid, &opts)
        .unwrap_or_else(|e| panic!("place failed for {name}: {e}"));

    let violations = check_legality(&packed.slices, &placed.slices, &grid, &limits);
    assert!(
        violations.is_empty(),
        "{name}: legality violations: {violations:?}"
    );
    println!(
        "{name}: LEGAL - {} slices placed, {} LUTs, {} FFs",
        placed.slices.len(),
        packed.stats.lut_count,
        packed.stats.ff_count
    );
}

#[test]
fn counter_is_legal() {
    test_design(
        "counter",
        "/home/hatch/workspace/helion-ultra-packplace/examples/counter.sv",
        "counter",
    );
}

#[test]
fn blinky_is_legal() {
    test_design(
        "blinky",
        "/home/hatch/workspace/helion-ultra-packplace/examples/blinky.sv",
        "blinky",
    );
}

#[test]
fn serv_is_legal() {
    test_design(
        "serv_alu",
        "/home/hatch/workspace/bench/corpus/sources/serv/rtl/serv_alu.v",
        "serv_alu",
    );
}
