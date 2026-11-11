//! Synthesize a design with Helion's own front end and write the resulting
//! netlist IR as structural Verilog, plus a JSON sidecar describing ports,
//! output IOBs, undriven nets and unsupported cells.
//!
//! This is the synthesis half of the verification flow: the Verilog is
//! co-simulated against the RTL (tools/x7v `synthcheck`), so front-end bugs
//! are caught before placement, routing or bitgen are involved.
//!
//! ```text
//! cargo run --release -p helion-vhdl --example synth2v -- design.vhd out.v out.json
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use helion_ir::{CellKind, Design, PortDir};

fn synth(path: &str) -> Result<Design, String> {
    // Target: Xilinx 7-series, which has no DSP48 support yet. Multiplies
    // must lower to LUTs or they vanish.
    helion_sv::set_dsp_inference(false);
    let p = std::path::Path::new(path);
    match p.extension().and_then(|e| e.to_str()).unwrap_or("sv").to_ascii_lowercase().as_str() {
        "vhd" | "vhdl" => helion_vhdl::synth_vhdl_path(p),
        _ => {
            let src = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
            helion_sv::synth_sv(&src, path)
        }
    }
}

fn id(net: &str) -> String {
    let mut s = String::from("n_");
    for c in net.chars() {
        s.push(if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' });
    }
    s
}

fn is_output_pin(kind: &CellKind, pin: &str) -> bool {
    match kind {
        CellKind::Lut6 { .. } => pin == "O",
        CellKind::Hff => pin == "Q",
        CellKind::Carry4 => pin == "CO" || matches!(pin, "O0" | "O1" | "O2" | "O3"),
        CellKind::IobIn | CellKind::ClockBuf => pin == "O",
        _ => false,
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: synth2v <design.vhd|.sv> <out.v> <out.json>");
        std::process::exit(2);
    }
    let d = match synth(&args[1]) {
        Ok(d) => d,
        Err(e) => {
            std::fs::write(&args[3], format!("{{\"error\": {}}}\n", json_str(&e))).unwrap();
            eprintln!("synth error: {e}");
            std::process::exit(1);
        }
    };
    let pins = d.pin_index();
    let kind_of: BTreeMap<&str, &CellKind> = d.cells.iter().map(|c| (c.name.as_str(), &c.kind)).collect();

    // Driver of every net (by output pin), and every referenced net.
    let mut driven: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut referenced: BTreeSet<&str> = BTreeSet::new();
    for n in &d.nets {
        for e in &n.endpoints {
            referenced.insert(n.name.as_str());
            if let Some(k) = kind_of.get(e.cell.as_str()) {
                if is_output_pin(k, &e.pin) {
                    driven.entry(n.name.as_str()).or_default().push(format!("{}.{}", e.cell, e.pin));
                }
            }
        }
    }
    let multi: Vec<(&str, &Vec<String>)> = driven.iter().filter(|(_, v)| v.len() > 1).map(|(k, v)| (*k, v)).collect();
    // Undriven nets that some cell input reads (port inputs or real bugs).
    let mut undriven: BTreeSet<&str> = BTreeSet::new();
    for n in &d.nets {
        if driven.contains_key(n.name.as_str()) {
            continue;
        }
        let read = n.endpoints.iter().any(|e| match kind_of.get(e.cell.as_str()) {
            Some(CellKind::IobOut) => e.pin == "I",
            Some(k) => !is_output_pin(k, &e.pin),
            None => false,
        });
        if read {
            undriven.insert(n.name.as_str());
        }
    }

    let mut v = String::new();
    let mut unsupported: Vec<String> = Vec::new();
    let iobs: Vec<&helion_ir::Cell> = d.cells.iter().filter(|c| matches!(c.kind, CellKind::IobOut)).collect();
    let mut ports: Vec<String> = undriven.iter().map(|n| format!("input wire {}", id(n))).collect();
    for c in &iobs {
        ports.push(format!("output wire o_{}", id(&c.name)));
    }
    writeln!(v, "// Helion netlist IR for `{}`, written by synth2v", d.name).unwrap();
    writeln!(v, "`timescale 1ns/1ps").unwrap();
    writeln!(v, "module helion_netlist({});", ports.join(", ")).unwrap();
    for n in &d.nets {
        if undriven.contains(n.name.as_str()) || !referenced.contains(n.name.as_str()) {
            continue;
        }
        if let Some(drv) = driven.get(n.name.as_str()) {
            let (cell, pin) = drv[0].split_once('.').unwrap();
            if matches!(kind_of.get(cell), Some(CellKind::Hff)) && pin == "Q" {
                continue; // declared as reg below
            }
        }
        writeln!(v, "wire {};", id(&n.name)).unwrap();
    }
    // Flop outputs are regs, declared before any logic reads them.
    for c in &d.cells {
        if matches!(c.kind, CellKind::Hff) {
            if let Some(q) = pins.net_on(&c.name, "Q") {
                let init = if c.attrs.get("INIT") == Some("1") { 1 } else { 0 };
                writeln!(v, "reg {} = 1'b{init};", id(q)).unwrap();
            }
        }
    }
    let net = |cell: &str, pin: &str| -> String {
        pins.net_on(cell, pin).map(id).unwrap_or_else(|| "1'b0".to_string())
    };
    for c in &d.cells {
        match &c.kind {
            CellKind::Lut6 { init } => {
                let ins: Vec<String> = (0..6).rev().map(|i| net(&c.name, &format!("I{i}"))).collect();
                if let Some(o) = pins.net_on(&c.name, "O") {
                    writeln!(v, "assign {} = 64'h{init:016x} >> {{{}}};", id(o), ins.join(", ")).unwrap();
                }
            }
            CellKind::Hff => {
                let (Some(q), Some(clk)) = (pins.net_on(&c.name, "Q"), pins.net_on(&c.name, "CLK")) else {
                    unsupported.push(format!("{}: Hff without Q or CLK", c.name));
                    continue;
                };
                let dn = net(&c.name, "D");
                for extra in ["CE", "SR", "RST"] {
                    if pins.net_on(&c.name, extra).is_some() {
                        unsupported.push(format!("{}: Hff pin {extra} connected (not modelled by synth2v)", c.name));
                    }
                }
                writeln!(v, "always @(posedge {}) {} <= {dn};", id(clk), id(q)).unwrap();
            }
            CellKind::Carry4 => {
                let ci = match pins.net_on(&c.name, "CI") {
                    Some(n) => id(n),
                    None => match c.attrs.get("CYINIT") {
                        Some("1") => "1'b1".into(),
                        _ => "1'b0".into(),
                    },
                };
                let mut carry = ci;
                for i in 0..4 {
                    let s = net(&c.name, &format!("S{i}"));
                    let di = net(&c.name, &format!("DI{i}"));
                    let co = format!("{}_co{i}", id(&c.name));
                    writeln!(v, "wire {co} = ({s}) ? ({carry}) : ({di});").unwrap();
                    if let Some(o) = pins.net_on(&c.name, &format!("O{i}")) {
                        writeln!(v, "assign {} = ({s}) ^ ({carry});", id(o)).unwrap();
                    }
                    carry = co;
                }
                if let Some(co) = pins.net_on(&c.name, "CO") {
                    writeln!(v, "assign {} = {carry};", id(co)).unwrap();
                }
            }
            CellKind::IobOut => {
                writeln!(v, "assign o_{} = {};", id(&c.name), net(&c.name, "I")).unwrap();
            }
            CellKind::IobIn | CellKind::ClockBuf => {
                let i = net(&c.name, if matches!(c.kind, CellKind::IobIn) { "PAD" } else { "I" });
                if let Some(o) = pins.net_on(&c.name, "O") {
                    writeln!(v, "assign {} = {i};", id(o)).unwrap();
                }
            }
            other => unsupported.push(format!("{}: {:?} not modelled", c.name, other)),
        }
    }
    writeln!(v, "endmodule").unwrap();
    std::fs::write(&args[2], v).unwrap();

    // Sidecar.
    let mut j = String::from("{\n");
    writeln!(j, "  \"design\": {},", json_str(&d.name)).unwrap();
    let attrs: Vec<String> = d.attrs.map.iter().map(|(k, val)| format!("{}: {}", json_str(k), json_str(val))).collect();
    writeln!(j, "  \"attrs\": {{{}}},", attrs.join(", ")).unwrap();
    let ps: Vec<String> = d
        .ports
        .iter()
        .map(|p| {
            let dir = match p.dir { PortDir::In => "in", PortDir::Out => "out", PortDir::Inout => "inout" };
            let a: Vec<String> = p.attrs.map.iter().map(|(k, val)| format!("{}: {}", json_str(k), json_str(val))).collect();
            format!("{{\"name\": {}, \"dir\": \"{dir}\", \"attrs\": {{{}}}}}", json_str(&p.name), a.join(", "))
        })
        .collect();
    writeln!(j, "  \"ports\": [{}],", ps.join(", ")).unwrap();
    let io: Vec<String> = iobs
        .iter()
        .map(|c| {
            let a: Vec<String> = c.attrs.map.iter().map(|(k, val)| format!("{}: {}", json_str(k), json_str(val))).collect();
            format!(
                "{{\"cell\": {}, \"verilog\": {}, \"pad\": {}, \"attrs\": {{{}}}}}",
                json_str(&c.name),
                json_str(&format!("o_{}", id(&c.name))),
                json_str(pins.net_on(&c.name, "PAD").unwrap_or("")),
                a.join(", ")
            )
        })
        .collect();
    writeln!(j, "  \"iobs_out\": [{}],", io.join(", ")).unwrap();
    let und: Vec<String> = undriven.iter().map(|n| format!("{{\"net\": {}, \"verilog\": {}}}", json_str(n), json_str(&id(n)))).collect();
    writeln!(j, "  \"undriven\": [{}],", und.join(", ")).unwrap();
    let mu: Vec<String> = multi.iter().map(|(n, v)| format!("{}: {}", json_str(n), json_str(&v.join(" ")))).collect();
    writeln!(j, "  \"multiply_driven\": {{{}}},", mu.join(", ")).unwrap();
    let un: Vec<String> = unsupported.iter().map(|s| json_str(s)).collect();
    writeln!(j, "  \"unsupported\": [{}],", un.join(", ")).unwrap();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for c in &d.cells {
        let k = format!("{:?}", c.kind);
        let k = k.split([' ', '{', '(']).next().unwrap_or("").to_string();
        *counts.entry(k).or_default() += 1;
    }
    let cs: Vec<String> = counts.iter().map(|(k, n)| format!("{}: {n}", json_str(k))).collect();
    writeln!(j, "  \"cells\": {{{}}}", cs.join(", ")).unwrap();
    j.push_str("}\n");
    std::fs::write(&args[3], j).unwrap();
}
