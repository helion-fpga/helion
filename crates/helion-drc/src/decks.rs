//! Active Multi-Deck UG906 / UG949 DRC & Methodology Evaluators.
//!
//! Evaluates `TIMING-*`, `SYNTH-*`, `METHODOLOGY-*`, `CDC-*`, `PDRC-*`,
//! `REQP-*`, and `AVAL-*` rule decks against a [`Design`] netlist (and optional
//! [`Placed`] / [`Routed`] physical implementation).

use crate::taxonomy::{RuleDeckCategory, lookup_rule};
use crate::{Drc, DrcSeverity, check_placed, check_routed};
use helion_device::Device;
use helion_ir::{CellKind, Design, PortDir, intern, resolve};
use helion_place::Placed;
use helion_route::Routed;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Record a violation using the catalog's default severity for `rule_id`.
pub fn emit_rule(
    drc: &mut Drc,
    rule_id: &str,
    objects: impl Into<String>,
    message: impl Into<String>,
) {
    let sev = lookup_rule(rule_id)
        .map(|r| r.default_severity)
        .unwrap_or(DrcSeverity::Warning);
    drc.add_with_severity(rule_id, sev, objects, message);
}

/// Evaluate `TIMING-*` rules on `design`.
pub fn check_timing_deck(design: &Design, drc: &mut Drc) {
    let pins = design.pin_index();
    let mut lut_out_nets: HashMap<&str, &str> = HashMap::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Lut6 { .. }) {
            if let Some(o_net) = pins.net_on(resolve(c.name), "O") {
                lut_out_nets.insert(resolve(o_net), resolve(c.name));
            }
        }
    }

    let mut clk_domains: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for c in &design.cells {
        let cname = resolve(c.name);
        match c.kind {
            CellKind::Hff => {
                if let Some(clk_spur) = pins.net_on(cname, "CLK") {
                    let clk_net = resolve(clk_spur);
                    clk_domains
                        .entry(clk_net.to_string())
                        .or_default()
                        .push(cname.to_string());
                    if let Some(&lut_cell) = lut_out_nets.get(clk_net) {
                        emit_rule(
                            drc,
                            "TIMING-14",
                            format!("{lut_cell} -> {cname}/CLK"),
                            format!(
                                "LUT6 {lut_cell} drives sequential clock pin {cname}/CLK on net {clk_net}"
                            ),
                        );
                    }
                } else {
                    emit_rule(
                        drc,
                        "TIMING-17",
                        cname,
                        format!("Sequential cell {cname} has no clock connected to CLK"),
                    );
                }
            }
            CellKind::Dsp48e1 { .. } => {
                if c.attrs.flag("PREG") || c.attrs.flag("MREG") {
                    if pins.net_on(cname, "CLK").is_none() {
                        emit_rule(
                            drc,
                            "TIMING-17",
                            cname,
                            format!("Registered DSP48E1 {cname} is missing CLK connection"),
                        );
                    }
                }
            }
            _ => {}
        }
    }

    if !clk_domains.is_empty() {
        for p in &design.ports {
            let pname = resolve(p.name);
            if clk_domains.contains_key(pname) || pname.eq_ignore_ascii_case("clk") {
                continue;
            }
            let has_io_delay = match p.dir {
                PortDir::In => p.attrs.get("INPUT_DELAY").is_some(),
                PortDir::Out => p.attrs.get("OUTPUT_DELAY").is_some(),
                PortDir::Inout => {
                    p.attrs.get("INPUT_DELAY").is_some() && p.attrs.get("OUTPUT_DELAY").is_some()
                }
            };
            if !has_io_delay {
                emit_rule(
                    drc,
                    "TIMING-18",
                    pname,
                    format!(
                        "Port {pname} is missing input/output delay constraint relative to design clock"
                    ),
                );
            }
        }
    }

    if clk_domains.len() > 1 && design.attrs.get("CLOCK_GROUPS").is_none() {
        let clks: Vec<String> = clk_domains.keys().cloned().collect();
        emit_rule(
            drc,
            "TIMING-6",
            clks.join(" "),
            format!(
                "Multiple primary clocks ({}) have no common clock source or set_clock_groups exception",
                clks.join(", ")
            ),
        );
    }
}

/// Evaluate `SYNTH-*` rules on `design`.
pub fn check_synth_deck(design: &Design, drc: &mut Drc) {
    let pins = design.pin_index();

    // SYNTH-1: High fanout net (> 32 endpoints) & SYNTH-6: Multi-driven net.
    for n in &design.nets {
        let nname = resolve(n.name);
        if n.endpoints.len() > 32 && !nname.eq_ignore_ascii_case("clk") {
            emit_rule(
                drc,
                "SYNTH-1",
                nname,
                format!(
                    "Net {nname} has high fanout ({} endpoints > 32)",
                    n.endpoints.len()
                ),
            );
        }
        let mut drivers = Vec::new();
        for e in &n.endpoints {
            let pin = resolve(e.pin);
            if matches!(pin, "O" | "Q" | "P" | "DOADO" | "DOBDO") {
                drivers.push(format!("{}/{}", resolve(e.cell), pin));
            }
        }
        if drivers.len() > 1 {
            emit_rule(
                drc,
                "SYNTH-6",
                nname,
                format!(
                    "Net {nname} has multiple active drivers: {}",
                    drivers.join(", ")
                ),
            );
        }
    }

    // SYNTH-2: Unconnected top-level port.
    for p in &design.ports {
        let pname = resolve(p.name);
        let connected = design
            .net(pname)
            .map_or(false, |n| !n.endpoints.is_empty());
        if !connected {
            emit_rule(
                drc,
                "SYNTH-2",
                pname,
                format!("Top-level port {pname} has no connected cell pins"),
            );
        }
    }

    // Build LUT output -> LUT cell map for SYNTH-3 combinational loop check.
    let mut lut_out_to_cell: HashMap<helion_ir::Spur, &str> = HashMap::new();
    let mut lut_inputs: HashMap<&str, Vec<helion_ir::Spur>> = HashMap::new();
    for c in &design.cells {
        let cname = resolve(c.name);
        match c.kind {
            CellKind::Lut6 { .. } => {
                if let Some(o_net) = pins.net_on(cname, "O") {
                    lut_out_to_cell.insert(o_net, cname);
                }
                let mut ins = Vec::new();
                for ipin in ["I0", "I1", "I2", "I3", "I4", "I5"] {
                    if let Some(in_net) = pins.net_on(cname, ipin) {
                        ins.push(in_net);
                    }
                }
                lut_inputs.insert(cname, ins);
            }
            CellKind::BlackBox { module } => {
                emit_rule(
                    drc,
                    "SYNTH-7",
                    cname,
                    format!(
                        "BlackBox instance {cname} (module {}) is unelaborated",
                        resolve(module)
                    ),
                );
            }
            CellKind::Dsp48e1 { .. } => {
                if !c.attrs.flag("PREG") && !c.attrs.flag("MREG") {
                    emit_rule(
                        drc,
                        "SYNTH-8",
                        cname,
                        format!(
                            "DSP48E1 {cname} has neither MREG nor PREG pipeline registers enabled"
                        ),
                    );
                }
            }
            CellKind::Bram18 | CellKind::Bram18e1 { .. } => {
                if !c.attrs.flag("DOA_REG") && !c.attrs.flag("DOB_REG") {
                    emit_rule(
                        drc,
                        "SYNTH-9",
                        cname,
                        format!(
                            "Block RAM {cname} does not enable output register (DOA_REG/DOB_REG)"
                        ),
                    );
                }
            }
            _ => {}
        }
    }

    // SYNTH-3: Detect 1-hop or 2-hop combinational LUT loops (no Hff in between).
    let mut reported_loops = BTreeSet::new();
    for (&lut_name, ins) in &lut_inputs {
        for &in_net in ins {
            if let Some(&src_lut) = lut_out_to_cell.get(&in_net) {
                if src_lut == lut_name {
                    if reported_loops.insert(lut_name.to_string()) {
                        emit_rule(
                            drc,
                            "SYNTH-3",
                            lut_name,
                            format!(
                                "Combinational self-loop on LUT6 {lut_name} via net {}",
                                resolve(in_net)
                            ),
                        );
                    }
                } else if let Some(src_ins) = lut_inputs.get(src_lut) {
                    for &src_in in src_ins {
                        if lut_out_to_cell.get(&src_in) == Some(&lut_name) {
                            let pair = if lut_name < src_lut {
                                format!("{lut_name} <-> {src_lut}")
                            } else {
                                format!("{src_lut} <-> {lut_name}")
                            };
                            if reported_loops.insert(pair.clone()) {
                                emit_rule(
                                    drc,
                                    "SYNTH-3",
                                    &pair,
                                    format!("Combinational LUT cycle between {pair}"),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Evaluate `METHODOLOGY-*` (UG949 UltraFast Design Methodology) rules on `design`.
pub fn check_methodology_deck(design: &Design, drc: &mut Drc) {
    let pins = design.pin_index();

    // Track FF Q nets and LUT O nets for METHODOLOGY-5 (unregistered output IOB).
    let mut ff_q_nets = BTreeSet::new();
    let mut lut_o_nets = BTreeSet::new();
    let mut control_sets = BTreeSet::new();
    let mut dont_touch_cells = Vec::new();

    for c in &design.cells {
        let cname = resolve(c.name);
        if c.attrs.flag("DONT_TOUCH") {
            dont_touch_cells.push(cname.to_string());
        }
        match c.kind {
            CellKind::Hff => {
                if let Some(q) = pins.net_on(cname, "Q") {
                    ff_q_nets.insert(q);
                }
                let clk = pins.net_on(cname, "CLK").map(resolve).unwrap_or("none");
                let ce = c.attrs.get("CE").unwrap_or("1");
                let sr = c.attrs.get("SR").unwrap_or("0");
                control_sets.insert((clk.to_string(), ce.to_string(), sr.to_string()));
            }
            CellKind::Lut6 { .. } => {
                if let Some(o) = pins.net_on(cname, "O") {
                    lut_o_nets.insert(o);
                }
            }
            _ => {}
        }
    }

    if control_sets.len() > 4 {
        emit_rule(
            drc,
            "METHODOLOGY-2",
            resolve(design.name),
            format!(
                "Design has {} distinct (CLK, CE, SR) control sets (> 4 threshold)",
                control_sets.len()
            ),
        );
    }

    for c in &design.cells {
        if matches!(c.kind, CellKind::IobOut) {
            let cname = resolve(c.name);
            if let Some(i_net) = pins.net_on(cname, "I") {
                if lut_o_nets.contains(&i_net) && !ff_q_nets.contains(&i_net) {
                    emit_rule(
                        drc,
                        "METHODOLOGY-5",
                        cname,
                        format!(
                            "Output IOB {cname} is driven by combinational LUT net {} instead of a register",
                            resolve(i_net)
                        ),
                    );
                }
            }
        }
    }

    for p in &design.ports {
        let pname = resolve(p.name);
        if p.attrs.get("LOC").is_none() {
            emit_rule(
                drc,
                "METHODOLOGY-8",
                pname,
                format!("Port {pname} has no explicit LOC package pin assignment"),
            );
        }
        if p.attrs.get("IOSTANDARD").is_none() {
            emit_rule(
                drc,
                "METHODOLOGY-9",
                pname,
                format!("Port {pname} relies on default IOSTANDARD without explicit XDC property"),
            );
        }
    }

    if dont_touch_cells.len() >= 4 {
        emit_rule(
            drc,
            "METHODOLOGY-10",
            dont_touch_cells.join(" "),
            format!(
                "{} cells carry DONT_TOUCH, restricting netlist optimization",
                dont_touch_cells.len()
            ),
        );
    }
}

/// Evaluate `CDC-*` (Clock Domain Crossing) rules on `design`.
pub fn check_cdc_deck(design: &Design, drc: &mut Drc) {
    let pins = design.pin_index();

    // Map FF cell -> clock net, and Q net -> (FF cell, clock net).
    let mut ff_clk: HashMap<&str, &str> = HashMap::new();
    let mut q_to_ff: HashMap<helion_ir::Spur, (&str, &str)> = HashMap::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Hff) {
            let cname = resolve(c.name);
            if let Some(clk) = pins.net_on(cname, "CLK") {
                let clk_name = resolve(clk);
                ff_clk.insert(cname, clk_name);
                if let Some(q) = pins.net_on(cname, "Q") {
                    q_to_ff.insert(q, (cname, clk_name));
                }
            }
        }
    }

    // Also map LUT O net -> (LUT cell, list of source (ff_cell, clk_name)) to detect combinational CDC.
    let mut lut_o_sources: HashMap<helion_ir::Spur, (&str, Vec<(&str, &str)>)> = HashMap::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Lut6 { .. }) {
            let cname = resolve(c.name);
            if let Some(o_net) = pins.net_on(cname, "O") {
                let mut srcs = Vec::new();
                for ipin in ["I0", "I1", "I2", "I3", "I4", "I5"] {
                    if let Some(in_net) = pins.net_on(cname, ipin) {
                        if let Some(&src) = q_to_ff.get(&in_net) {
                            srcs.push(src);
                        }
                    }
                }
                lut_o_sources.insert(o_net, (cname, srcs));
            }
        }
    }

    let mut pair_crossings: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();

    for c in &design.cells {
        if !matches!(c.kind, CellKind::Hff) {
            continue;
        }
        let dst_ff = resolve(c.name);
        let Some(&dst_clk) = ff_clk.get(dst_ff) else {
            continue;
        };
        let Some(d_net) = pins.net_on(dst_ff, "D") else {
            continue;
        };
        let has_async_reg = c.attrs.flag("ASYNC_REG") || c.attrs.flag("async_reg");

        // Direct FF Q -> FF D crossing:
        if let Some(&(src_ff, src_clk)) = q_to_ff.get(&d_net) {
            if src_clk != dst_clk {
                pair_crossings
                    .entry((src_clk.to_string(), dst_clk.to_string()))
                    .or_default()
                    .push(format!("{src_ff}->{dst_ff}"));
                if !has_async_reg {
                    emit_rule(
                        drc,
                        "CDC-1",
                        format!("{src_ff} -> {dst_ff}"),
                        format!(
                            "Unsynchronized CDC from {src_ff} ({src_clk}) to {dst_ff} ({dst_clk}) without ASYNC_REG"
                        ),
                    );
                }
            }
        }

        // Combinational LUT on CDC path: FF Q -> LUT -> FF D across different clocks.
        if let Some((lut_cell, srcs)) = lut_o_sources.get(&d_net) {
            for &(src_ff, src_clk) in srcs {
                if src_clk != dst_clk {
                    pair_crossings
                        .entry((src_clk.to_string(), dst_clk.to_string()))
                        .or_default()
                        .push(format!("{src_ff}->{lut_cell}->{dst_ff}"));
                    emit_rule(
                        drc,
                        "CDC-10",
                        format!("{src_ff} -> {lut_cell} -> {dst_ff}"),
                        format!(
                            "Combinational LUT {lut_cell} in CDC path from {src_ff} ({src_clk}) to {dst_ff} ({dst_clk})"
                        ),
                    );
                    if !has_async_reg {
                        emit_rule(
                            drc,
                            "CDC-1",
                            dst_ff,
                            format!(
                                "Destination FF {dst_ff} ({dst_clk}) receives cross-domain signal from {src_clk} without ASYNC_REG"
                            ),
                        );
                    }
                }
            }
        }
    }

    for ((src_clk, dst_clk), paths) in pair_crossings {
        if paths.len() >= 2 {
            emit_rule(
                drc,
                "CDC-4",
                paths.join(" "),
                format!(
                    "Multi-bit bus CDC ({} bits) between {src_clk} and {dst_clk} requires Gray code or FIFO",
                    paths.len()
                ),
            );
        }
    }
}

/// Evaluate `REQP-*` (Required Primitive Pin) rules on `design`.
pub fn check_reqp_deck(design: &Design, drc: &mut Drc) {
    let pins = design.pin_index();
    for c in &design.cells {
        let cname = resolve(c.name);
        match c.kind {
            CellKind::Hff => {
                if pins.net_on(cname, "D").is_none() {
                    emit_rule(
                        drc,
                        "REQP-1",
                        cname,
                        format!("HFF {cname} has unconnected required input pin D"),
                    );
                }
                if pins.net_on(cname, "CLK").is_none() {
                    emit_rule(
                        drc,
                        "REQP-2",
                        cname,
                        format!("HFF {cname} has unconnected required clock pin CLK"),
                    );
                }
            }
            CellKind::Lut6 { .. } => {
                let any_in = ["I0", "I1", "I2", "I3", "I4", "I5"]
                    .iter()
                    .any(|p| pins.net_on(cname, p).is_some());
                if !any_in {
                    emit_rule(
                        drc,
                        "REQP-5",
                        cname,
                        format!("LUT6 {cname} has no connected input pins (I0..I5)"),
                    );
                }
            }
            CellKind::Bram18e1 { .. } => {
                if pins.net_on(cname, "CLKARDCLK").is_none() {
                    emit_rule(
                        drc,
                        "REQP-10",
                        cname,
                        format!("RAMB18E1 {cname} is missing required CLKARDCLK connection"),
                    );
                }
            }
            CellKind::Dsp48e1 { .. } => {
                let has_p = design.nets.iter().any(|n| {
                    n.endpoints.iter().any(|e| {
                        e.cell == c.name && resolve(e.pin).starts_with('P')
                    })
                });
                if !has_p {
                    emit_rule(
                        drc,
                        "REQP-15",
                        cname,
                        format!("DSP48E1 {cname} has no connected P output pins"),
                    );
                }
            }
            CellKind::IobOut => {
                if pins.net_on(cname, "I").is_none() || pins.net_on(cname, "PAD").is_none() {
                    emit_rule(
                        drc,
                        "REQP-20",
                        cname,
                        format!("IobOut {cname} is missing required I or PAD pin connection"),
                    );
                }
            }
            _ => {}
        }
    }
}

/// Evaluate `AVAL-*` (Architecture Value / Attribute Legality) rules on `design`.
pub fn check_aval_deck(design: &Design, drc: &mut Drc) {
    const LEGAL_BRAM_WIDTHS: [u8; 7] = [0, 1, 2, 4, 9, 18, 36];
    for c in &design.cells {
        let cname = resolve(c.name);
        match c.kind {
            CellKind::Bram18e1 {
                read_width_a,
                read_width_b,
                write_width_a,
                write_width_b,
            } => {
                for (wname, w) in [
                    ("READ_WIDTH_A", read_width_a),
                    ("READ_WIDTH_B", read_width_b),
                    ("WRITE_WIDTH_A", write_width_a),
                    ("WRITE_WIDTH_B", write_width_b),
                ] {
                    if !LEGAL_BRAM_WIDTHS.contains(&w) {
                        emit_rule(
                            drc,
                            "AVAL-1",
                            cname,
                            format!(
                                "RAMB18E1 {cname} has illegal {wname}={w} (allowed: 0, 1, 2, 4, 9, 18, 36)"
                            ),
                        );
                    }
                }
            }
            CellKind::Dsp48e1 { opmode, alumode } => {
                if opmode > 0x7F || alumode > 0x0F {
                    emit_rule(
                        drc,
                        "AVAL-5",
                        cname,
                        format!(
                            "DSP48E1 {cname} has out-of-range opmode={opmode:#x} (7-bit) or alumode={alumode:#x} (4-bit)"
                        ),
                    );
                }
            }
            CellKind::Lut6 { init } => {
                if init == 0 || init == u64::MAX {
                    emit_rule(
                        drc,
                        "AVAL-10",
                        cname,
                        format!("LUT6 {cname} has constant INIT={init:#018x}"),
                    );
                }
            }
            _ => {}
        }
    }
}

/// Evaluate `PDRC-*` physical DRC rules on `design` + optional `Placed` / `Routed`.
pub fn check_pdrc_deck(
    design: &Design,
    placed: Option<&Placed>,
    routed: Option<&Routed>,
    dev: &Device,
    drc: &mut Drc,
) {
    if let Some(r) = routed {
        let phys = check_routed(design, r, dev);
        for item in phys.items {
            if item.id == "PLACE-4" {
                emit_rule(drc, "PDRC-1", &item.objects, &item.message);
            } else if item.id == "IOSTD-2" {
                emit_rule(drc, "PDRC-2", &item.objects, &item.message);
            } else if item.id == "ROUTE-2" {
                emit_rule(drc, "PDRC-3", &item.objects, &item.message);
            }
            drc.add_with_severity(&item.id, item.severity, item.objects, item.message);
        }
    } else if let Some(pl) = placed {
        let phys = check_placed(design, pl, dev);
        for item in phys.items {
            if item.id == "PLACE-4" {
                emit_rule(drc, "PDRC-1", &item.objects, &item.message);
            } else if item.id == "IOSTD-2" {
                emit_rule(drc, "PDRC-2", &item.objects, &item.message);
            }
            drc.add_with_severity(&item.id, item.severity, item.objects, item.message);
        }
    } else {
        // Pre-placement CARRY4 check.
        let carry_count = design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Carry4))
            .count();
        if carry_count as u32 > dev.interior_rows {
            emit_rule(
                drc,
                "PDRC-4",
                resolve(design.name),
                format!(
                    "Design instantiates {carry_count} CARRY4 cells exceeding single column height {}",
                    dev.interior_rows
                ),
            );
        }
    }
}

/// Evaluate a specific [`RuleDeckCategory`] on `design`.
pub fn check_rule_deck(
    category: RuleDeckCategory,
    design: &Design,
    placed: Option<&Placed>,
    routed: Option<&Routed>,
    dev: &Device,
) -> Drc {
    let mut drc = Drc::default();
    match category {
        RuleDeckCategory::Timing => check_timing_deck(design, &mut drc),
        RuleDeckCategory::Synth => check_synth_deck(design, &mut drc),
        RuleDeckCategory::Methodology => check_methodology_deck(design, &mut drc),
        RuleDeckCategory::Cdc => check_cdc_deck(design, &mut drc),
        RuleDeckCategory::Reqp => check_reqp_deck(design, &mut drc),
        RuleDeckCategory::Aval => check_aval_deck(design, &mut drc),
        RuleDeckCategory::Pdrc | RuleDeckCategory::PlacementIo => {
            check_pdrc_deck(design, placed, routed, dev, &mut drc)
        }
    }
    drc
}

/// Run all Vivado UG906 / UG949 DRC & Methodology decks (`TIMING`, `SYNTH`, `METHODOLOGY`,
/// `CDC`, `REQP`, `AVAL`, `PDRC`) and return the combined [`Drc`] report.
pub fn check_all_ug906_ug949_decks(
    design: &Design,
    placed: Option<&Placed>,
    routed: Option<&Routed>,
    dev: &Device,
) -> Drc {
    let mut drc = Drc::default();
    check_timing_deck(design, &mut drc);
    check_synth_deck(design, &mut drc);
    check_methodology_deck(design, &mut drc);
    check_cdc_deck(design, &mut drc);
    check_reqp_deck(design, &mut drc);
    check_aval_deck(design, &mut drc);
    check_pdrc_deck(design, placed, routed, dev, &mut drc);
    drc
}
