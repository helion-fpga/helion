//! Synthesis-mapping inventory (GAP-F1, CTO spec step 1).
//!
//! Takes the blinky and counter netlists through the existing synth path
//! (`helion-sv`) and lists every cell type that reaches pack, marking which
//! have an UltraScale primitive mapping and which do not. This is the
//! evidence base for the mapping work: any cell kind printed here as
//! UNMAPPED must fail loud in `extract_cells`
//! (`crates/helion-cli/src/ultra.rs`), never silently drop.
//!
//! Run with output shown:
//! `cargo test -j2 -p helion-ultrascale --test synthmap_inventory -- --nocapture`

use helion_ir::{CellKind, Design, resolve};
use std::collections::BTreeMap;

/// One inventoried cell: name, kind as printed by the IR, attributes,
/// and pin-to-net connections.
#[derive(Debug)]
struct InventoriedCell {
    name: String,
    kind: String,
    attrs: BTreeMap<String, String>,
    pins: BTreeMap<String, String>,
}

/// Mapping verdict for a cell kind reaching pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MapVerdict {
    /// Has an UltraScale primitive mapping in this flow.
    Mapped,
    /// Reaches pack only as a skipped non-logic cell (IOB, clock buffer):
    /// no slice primitive, handled outside pack.
    SkippedOutsidePack,
    /// No UltraScale mapping: must fail loud, never silently drop.
    Unmapped,
    /// Looks like a mapped kind but carries attributes that change its
    /// semantics (SRL shift register, distributed RAM): mapping it as a
    /// plain LUT6 would miscompile.
    Hazard,
}

fn classify(cell: &InventoriedCell, kind: &CellKind) -> (MapVerdict, &'static str) {
    match kind {
        CellKind::Lut6 { .. } => {
            if cell.attrs.contains_key("SRL_TYPE") || cell.attrs.contains_key("SHREG_EXTRACT") {
                (
                    MapVerdict::Hazard,
                    "Lut6 with SRL_TYPE/SHREG_EXTRACT: SRL16E/SRLC32E shift register, \
                     not a plain LUT6; LUTRAM emission is BLOCKED (G4 ruling)",
                )
            } else if cell.attrs.get("MEMORY").map(|s| s == "1").unwrap_or(false) {
                (
                    MapVerdict::Hazard,
                    "Lut6 with MEMORY=1: distributed RAM, SLICEM-only and LUTRAM-blocked",
                )
            } else {
                (MapVerdict::Mapped, "LUT6 (INIT preserved verbatim)")
            }
        }
        CellKind::Hff => (MapVerdict::Mapped, "FDRE (FF with CE/SR)"),
        CellKind::Carry4 => (
            MapVerdict::Mapped,
            "CARRY8 (one Carry4 per CARRY8 today, low nibble; 2:1 pairing is synthmap scope)",
        ),
        CellKind::IobIn => (MapVerdict::SkippedOutsidePack, "IBUF (IOB, outside pack)"),
        CellKind::IobOut => (MapVerdict::SkippedOutsidePack, "OBUF (IOB, outside pack)"),
        CellKind::ClockBuf => (
            MapVerdict::SkippedOutsidePack,
            "BUFGCE/BUFGCTRL (clocking, outside pack)",
        ),
        CellKind::Mac27 => (
            MapVerdict::Unmapped,
            "no DSP48E2 mapping: Mac27 is a deprecated 7-series-flavoured MAC with no \
             UltraScale opmode/width mapping; DSP48E2 encoding is BLOCKED on the DSP worker",
        ),
        CellKind::Dsp48e1 { .. } => (
            MapVerdict::Unmapped,
            "no DSP48E2 mapping: Dsp48e1 is a 7-series primitive (25x18, different \
             opmode/alumode widths); a direct remap to DSP48E2 (27x18) is not semantics-preserving",
        ),
        CellKind::Ila { .. } => (
            MapVerdict::Unmapped,
            "no ILA mapping: inserted ILA core has no UltraScale bitgen encoding in this flow",
        ),
        CellKind::Bram18 => (
            MapVerdict::Unmapped,
            "no RAMB18E2 mapping: deprecated Bram18 has no explicit pin interface",
        ),
        CellKind::Bram18e1 { .. } => (
            MapVerdict::Unmapped,
            "no RAMB18E2 mapping: Bram18e1 is a 7-series primitive; RAMB18E2 pin remap \
             plus INIT/INITP encoding is BLOCKED on the BRAM worker",
        ),
        CellKind::BlackBox { .. } => (
            MapVerdict::Unmapped,
            "no mapping: black-box instance is not supported for UltraScale+",
        ),
    }
}

fn inventory(design: &Design) -> Vec<(InventoriedCell, MapVerdict, &'static str)> {
    let mut out = Vec::new();
    for cell in &design.cells {
        let mut pins = BTreeMap::new();
        for net in &design.nets {
            for ep in &net.endpoints {
                if ep.cell == cell.name {
                    pins.insert(resolve(ep.pin).to_string(), resolve(net.name).to_string());
                }
            }
        }
        let inv = InventoriedCell {
            name: resolve(cell.name).to_string(),
            kind: format!("{:?}", cell.kind),
            attrs: cell.attrs.map.clone(),
            pins,
        };
        let (verdict, note) = classify(&inv, &cell.kind);
        out.push((inv, verdict, note));
    }
    out
}

fn print_inventory(
    label: &str,
    design: &Design,
) -> Vec<(InventoriedCell, MapVerdict, &'static str)> {
    let rows = inventory(design);
    println!(
        "=== inventory: {label} (top {}) cells={} ===",
        resolve(design.name),
        rows.len()
    );
    for (cell, verdict, note) in &rows {
        let v = match verdict {
            MapVerdict::Mapped => "MAPPED",
            MapVerdict::SkippedOutsidePack => "OUTSIDE-PACK",
            MapVerdict::Unmapped => "UNMAPPED",
            MapVerdict::Hazard => "HAZARD",
        };
        println!(
            "  [{v}] {} kind={} attrs={:?} pins={:?}\n        note: {note}",
            cell.name, cell.kind, cell.attrs, cell.pins
        );
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (cell, _, _) in &rows {
        // Count by kind family (strip the INIT payload from Lut6 debug).
        let family = cell
            .kind
            .split([' ', '{', '('])
            .next()
            .unwrap_or("?")
            .to_string();
        *counts.entry(family).or_default() += 1;
    }
    println!("  --- kind counts: {counts:?}");
    rows
}

fn example_path(name: &str) -> String {
    let rel = format!("examples/{name}");
    if std::path::Path::new(&rel).exists() {
        return rel;
    }
    format!("{}/../../{}", env!("CARGO_MANIFEST_DIR"), rel)
}

fn synth_file(name: &str, top: &str) -> Design {
    let path = example_path(name);
    let sv = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("cannot read {path}"));
    helion_sv::synth_sv(&sv, top).unwrap_or_else(|e| panic!("synth failed for {name}: {e}"))
}

fn synth_str(label: &str, sv: &str, top: &str) -> Design {
    helion_sv::synth_sv(sv, top).unwrap_or_else(|e| panic!("synth failed for {label}: {e}"))
}

#[test]
fn blinky_inventory() {
    let design = synth_file("blinky.sv", "blinky");
    let rows = print_inventory("blinky", &design);
    // Pinned from the observed run: 1 LUT6 (inverter) + 1 FF + IOBs.
    let luts = rows
        .iter()
        .filter(|(c, _, _)| c.kind.starts_with("Lut6"))
        .count();
    let ffs = rows.iter().filter(|(c, _, _)| c.kind == "Hff").count();
    assert_eq!(luts, 1, "blinky must synth to exactly 1 LUT6");
    assert_eq!(ffs, 1, "blinky must synth to exactly 1 FF");
    assert!(
        rows.iter()
            .all(|(_, v, _)| matches!(v, MapVerdict::Mapped | MapVerdict::SkippedOutsidePack)),
        "blinky must contain no UNMAPPED or HAZARD cells"
    );
    // The inverter LUT6 INIT is 0x5555555555555555 (bit i = NOT i0).
    for (cell, _, _) in &rows {
        if let Some(init) = cell.kind.strip_prefix("Lut6 { init: ") {
            assert!(
                init.contains("6148914691236517205"),
                "blinky inverter INIT must be 0x5555555555555555, got {init}"
            );
        }
    }
}

#[test]
fn counter_inventory() {
    let design = synth_file("counter.sv", "counter");
    let rows = print_inventory("counter", &design);
    let luts = rows
        .iter()
        .filter(|(c, _, _)| c.kind.starts_with("Lut6"))
        .count();
    let ffs = rows.iter().filter(|(c, _, _)| c.kind == "Hff").count();
    let carry = rows.iter().filter(|(c, _, _)| c.kind == "Carry4").count();
    assert_eq!(luts, 4, "counter must synth to exactly 4 LUT6");
    assert_eq!(ffs, 4, "counter must synth to exactly 4 FFs");
    assert_eq!(
        carry, 0,
        "counter uses a LUT ripple adder, no Carry4 is inferred"
    );
    assert!(
        rows.iter()
            .all(|(_, v, _)| matches!(v, MapVerdict::Mapped | MapVerdict::SkippedOutsidePack)),
        "counter must contain no UNMAPPED or HAZARD cells"
    );
}

#[test]
fn six_input_and_inventory() {
    // The 6-input AND case class that caught the qor2-mapper miscompile
    // (6-input AND to const 0). Here it must survive synth as a LUT6
    // with INIT 0x8000000000000000 (only bit 63 set).
    let sv = r#"
module and6 (
    input logic clk,
    input logic a, b, c, d, e, f,
    output logic q
);
    always_ff @(posedge clk) begin
        q <= a & b & c & d & e & f;
    end
endmodule
"#;
    let design = synth_str("and6", sv, "and6");
    let rows = print_inventory("and6", &design);
    let and_luts: Vec<&(InventoriedCell, MapVerdict, &'static str)> = rows
        .iter()
        .filter(|(c, _, _)| c.kind.starts_with("Lut6"))
        .collect();
    assert_eq!(
        and_luts.len(),
        1,
        "6-input AND must synth to exactly 1 LUT6"
    );
    let kind = &and_luts[0].0.kind;
    assert!(
        kind.contains("9223372036854775808"),
        "6-input AND LUT6 INIT must be 0x8000000000000000 (bit 63 only), got {kind}"
    );
}

#[test]
fn enable_ff_inventory() {
    // An FF with an enable: the front end lowers `if (en) q <= d` to
    // a 3-input mux LUT (en ? d : q) feeding D, NOT to a CE pin on the
    // FF. The FF carries CLK/D/Q only, so CEUSED.V1 is unexercised by
    // the front end today; the CEUSED emitter path is covered by the
    // synthmap unit tests, which build packed slices with CE nets
    // directly. Whether the front end should emit CE pins instead
    // (saving the mux LUT) is a front-end/QoR decision, not a mapping
    // question: the mapping here must describe what synth emits, not
    // what it could emit.
    let sv = r#"
module enff (
    input logic clk,
    input logic en,
    input logic d,
    output logic q
);
    always_ff @(posedge clk) begin
        if (en) q <= d;
    end
endmodule
"#;
    let design = synth_str("enff", sv, "enff");
    let rows = print_inventory("enff", &design);
    let ffs: Vec<&(InventoriedCell, MapVerdict, &'static str)> =
        rows.iter().filter(|(c, _, _)| c.kind == "Hff").collect();
    assert_eq!(ffs.len(), 1, "enable FF design must synth to exactly 1 FF");
    assert!(
        !ffs[0].0.pins.contains_key("CE"),
        "enable FF must NOT carry a CE pin (front end lowers the enable to a D-mux LUT), got pins {:?}",
        ffs[0].0.pins
    );
    let muxes: Vec<&(InventoriedCell, MapVerdict, &'static str)> = rows
        .iter()
        .filter(|(c, _, _)| c.kind.starts_with("Lut6"))
        .collect();
    assert_eq!(muxes.len(), 1, "enable must lower to exactly 1 mux LUT");
    assert!(
        muxes[0].0.pins.values().any(|n| n == "en"),
        "mux LUT must take en as an input, got pins {:?}",
        muxes[0].0.pins
    );
}

#[test]
fn adder_carry_inventory() {
    // An 8-bit combinational adder: does synth infer Carry4 chains, or
    // a LUT ripple adder? The answer decides whether CARRY8 mapping is
    // exercised by ordinary adder RTL or only by explicit Carry4 cells.
    let sv = r#"
module add8 (
    input logic [7:0] a, b,
    output logic [7:0] s
);
    assign s = a + b;
endmodule
"#;
    let design = synth_str("add8", sv, "add8");
    let rows = print_inventory("add8", &design);
    let carry = rows.iter().filter(|(c, _, _)| c.kind == "Carry4").count();
    let luts = rows
        .iter()
        .filter(|(c, _, _)| c.kind.starts_with("Lut6"))
        .count();
    println!("add8: {luts} LUT6, {carry} Carry4");
}
