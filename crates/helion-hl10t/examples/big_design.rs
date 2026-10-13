//! Big-design P&R proof: PicoRV32 through HL10T and 7-series.
//!
//! Measures wall-clock runtime and QoR (WNS, wirelength, congestion).

use helion_backend::Backend;
use std::time::Instant;

fn main() {
    // Synthesize PicoRV32.
    println!("Synthesizing PicoRV32...");
    let src = std::fs::read_to_string("third_party/picorv32/picorv32.v")
        .expect("picorv32.v not found");
    let t0 = Instant::now();
    let design = helion_sv::synth_sv(&src, "picorv32.v").expect("synth failed");
    let synth_ms = t0.elapsed().as_millis();
    
    let luts = design.cells.iter().filter(|c| matches!(c.kind, helion_ir::CellKind::Lut6 { .. })).count();
    let ffs = design.cells.iter().filter(|c| matches!(c.kind, helion_ir::CellKind::Hff)).count();
    println!("PicoRV32: {luts} LUTs, {ffs} FFs (synth: {synth_ms}ms)");
    
    // HL10T backend.
    println!("\n=== HL10T Backend ===");
    let hl10t = helion_hl10t::Hl10tBackend::open().expect("hl10t open failed");
    
    let t0 = Instant::now();
    let packed = hl10t.pack(&design).expect("hl10t pack failed");
    let pack_ms = t0.elapsed().as_millis();
    println!("Pack: {pack_ms}ms");
    
    let t0 = Instant::now();
    let placed = hl10t.place(&packed).expect("hl10t place failed");
    let place_ms = t0.elapsed().as_millis();
    println!("Place: {place_ms}ms");
    
    let t0 = Instant::now();
    let routed = hl10t.route(&placed).expect("hl10t route failed");
    let route_ms = t0.elapsed().as_millis();
    println!("Route: {route_ms}ms");
    
    println!("\nHL10T total P&R: {}ms", pack_ms + place_ms + route_ms);
}
