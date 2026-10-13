//! Big-design P&R proof: PicoRV32 through 7-series.
//!
//! Measures wall-clock runtime.

use helion_backend::Backend;
use std::time::Instant;

fn main() {
    // Synthesize PicoRV32.
    println!("Synthesizing PicoRV32...");
    let src = std::fs::read_to_string("third_party/picorv32/picorv32.v")
        .expect("picorv32.v not found");
    let t0 = Instant::now();
    let mut design = helion_sv::synth_sv(&src, "picorv32.v").expect("synth failed");
    let synth_ms = t0.elapsed().as_millis();
    
    let luts = design.cells.iter().filter(|c| matches!(c.kind, helion_ir::CellKind::Lut6 { .. })).count();
    let ffs = design.cells.iter().filter(|c| matches!(c.kind, helion_ir::CellKind::Hff)).count();
    println!("PicoRV32: {luts} LUTs, {ffs} FFs (synth: {synth_ms}ms)");
    
    // Assign dummy LOCs to ports for the 7-series backend.
    // Use a few real package pins and cycle through them.
    let pins = ["E3", "H5", "F6", "G6", "H6", "J6", "K6", "L6", "M6", "N6"];
    for (i, port) in design.ports.iter_mut().enumerate() {
        let pin = pins[i % pins.len()];
        port.attrs.map.insert("LOC".to_string(), pin.to_string());
    }
    println!("Assigned LOCs to {} ports", design.ports.len());
    
    // 7-series backend.
    println!("\n=== 7-Series Backend (xc7a35t) ===");
    let x7 = helion_xilinx7::backend::Xilinx7Backend::open("xc7a35t").expect("x7 open failed");
    
    let t0 = Instant::now();
    let packed = x7.pack(&design).expect("x7 pack failed");
    let pack_ms = t0.elapsed().as_millis();
    println!("Pack: {pack_ms}ms");
    
    let t0 = Instant::now();
    let placed = x7.place(&packed).expect("x7 place failed");
    let place_ms = t0.elapsed().as_millis();
    println!("Place: {place_ms}ms");
    
    println!("\n7-series total pack+place: {}ms", pack_ms + place_ms);
    println!("(Route not measured: PicoRV32 core has no top-level clock LOC)");
}
