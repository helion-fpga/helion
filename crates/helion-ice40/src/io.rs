//! Bind a synthesized design's top-level ports to package pins.
//!
//! The v2 front end (`helion_sv::synth_sv_v2_files`) leaves input ports as
//! bare nets (`clk`, or `sw_0`.. for bits of a vector) and gives every
//! output bit an `IobOut` cell carrying `PORT`/`BIT` attributes. The iCE40
//! packer wants an IO cell per pad with a `LOC`. [`bind_pcf`] adds an
//! `IobIn` (and, for nets that clock flip-flops or RAMs, a `ClockBuf` so the
//! clock rides a global network) for each used input bit, and gives every
//! IO cell the `LOC` its `set_io` names.
//!
//! Port bits are named as in a PCF: `clk` for a scalar, `led[3]` for a bit
//! of a vector. A pad with no `set_io` is an error; a `set_io` that names no
//! pad of the design is reported in [`IoBinding::unused`] (an input bit the
//! logic never reads has no pad).

use crate::pcf::SetIo;
use helion_ir::{intern, resolve, CellKind, Design, PortDir};
use std::collections::{BTreeMap, HashMap};

/// What [`bind_pcf`] did.
#[derive(Clone, Debug, Default)]
pub struct IoBinding {
    /// `(port bit, pin, direction)` of every bound pad.
    pub pads: Vec<(String, String, PortDir)>,
    /// Port bits put on a global clock buffer.
    pub clocks: Vec<String>,
    /// `set_io` constraints that match no pad of the design.
    pub unused: Vec<SetIo>,
}

/// The PCF-style name of an IO cell's port bit: `PORT[BIT]` for a bit of a
/// vector, `PORT` for a scalar (`PORT`/`BIT` attributes, else the `PAD`
/// net's name).
pub fn io_port_name(design: &Design, cell: &str) -> Option<String> {
    let c = design.cell(cell)?;
    if let Some(port) = c.attrs.get("PORT") {
        if c.attrs.get("VECTOR") == Some("0") {
            return Some(port.to_string());
        }
        let bit = c.attrs.get("BIT").unwrap_or("0");
        return Some(format!("{port}[{bit}]"));
    }
    design.net_on(intern(cell), intern("PAD")).map(|n| resolve(n).to_string())
}

fn is_clock_pin(kind: &CellKind, pin: &str) -> bool {
    match kind {
        CellKind::Hff => pin == "CLK",
        CellKind::Bram18 | CellKind::Bram18e1 { .. } => matches!(pin, "CLK" | "RCLK" | "WCLK" | "RCLKN" | "WCLKN"),
        CellKind::BlackBox { module } if resolve(*module).starts_with("SB_RAM40_4K") => {
            matches!(pin, "RCLK" | "WCLK" | "RCLKN" | "WCLKN")
        }
        _ => false,
    }
}

/// Add input pads and clock buffers and set every IO cell's `LOC` from
/// `cons`. See the module docs.
pub fn bind_pcf(design: &mut Design, cons: &[SetIo]) -> Result<IoBinding, String> {
    let by_port: HashMap<&str, &SetIo> = cons.iter().map(|c| (c.port.as_str(), c)).collect();
    let mut used: Vec<bool> = vec![false; cons.len()];
    let index_of = |c: &SetIo| cons.iter().position(|x| std::ptr::eq(x, c)).unwrap_or(0);

    // Output (and already present input) pads: group IobOut/IobIn cells by
    // PORT so a one-cell PORT with BIT 0 reads as a scalar.
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::IobOut | CellKind::IobIn) {
            if let Some(port) = c.attrs.get("PORT") {
                groups.entry(port.to_string()).or_default().push(resolve(c.name).to_string());
            }
        }
    }
    for (port, cells) in &groups {
        let scalar = cells.len() == 1
            && design.cell(&cells[0]).and_then(|c| c.attrs.get("BIT")).unwrap_or("0") == "0"
            && !by_port.contains_key(format!("{port}[0]").as_str());
        if scalar {
            design.set_cell_attr(&cells[0], "VECTOR", "0")?;
        }
    }

    // Input bits that are bare nets: (port bit name, net name).
    let mut inputs: Vec<(String, String)> = Vec::new();
    let ports: Vec<(String, PortDir)> = design.ports.iter().map(|p| (resolve(p.name).to_string(), p.dir)).collect();
    for (port, dir) in &ports {
        match dir {
            PortDir::Out => {}
            PortDir::Inout => return Err(format!("port '{port}': inout ports are not supported by the iCE40 backend")),
            PortDir::In => {
                let has_pad = |net: &str| {
                    design.net(net).is_some_and(|n| n.endpoints.iter().any(|e| resolve(e.pin) == "PAD"))
                };
                if design.net(port).is_some_and(|n| !n.endpoints.is_empty()) {
                    if !has_pad(port) {
                        inputs.push((port.clone(), port.clone()));
                    }
                    continue;
                }
                let prefix = format!("{port}_");
                let mut bits: Vec<(u32, String)> = design
                    .nets
                    .iter()
                    .filter_map(|n| {
                        let name = resolve(n.name);
                        let bit = name.strip_prefix(&prefix)?.parse::<u32>().ok()?;
                        (!n.endpoints.is_empty()).then(|| (bit, name.to_string()))
                    })
                    .collect();
                bits.sort();
                for (bit, net) in bits {
                    if !has_pad(&net) {
                        inputs.push((format!("{port}[{bit}]"), net));
                    }
                }
            }
        }
    }

    let mut out = IoBinding::default();
    let mut missing: Vec<String> = Vec::new();
    let take = |key: &str, used: &mut Vec<bool>| -> Option<String> {
        let c = by_port.get(key).copied()?;
        used[index_of(c)] = true;
        Some(c.pin.clone())
    };

    // Existing pads.
    let pad_cells: Vec<(String, bool)> = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::IobOut | CellKind::IobIn))
        .map(|c| (resolve(c.name).to_string(), matches!(c.kind, CellKind::IobIn)))
        .collect();
    for (cell, is_in) in pad_cells {
        let key = io_port_name(design, &cell).ok_or_else(|| format!("IO cell {cell} has no PAD net and no PORT attribute"))?;
        match take(&key, &mut used) {
            Some(pin) => {
                design.set_cell_attr(&cell, "LOC", pin.clone())?;
                out.pads.push((key, pin, if is_in { PortDir::In } else { PortDir::Out }));
            }
            None => missing.push(key),
        }
    }

    // New input pads.
    for (key, net) in inputs {
        let Some(pin) = take(&key, &mut used) else {
            missing.push(key);
            continue;
        };
        let clocked = design.net(&net).is_some_and(|n| {
            n.endpoints.iter().any(|e| design.cell(resolve(e.cell)).is_some_and(|c| is_clock_pin(&c.kind, resolve(e.pin))))
        });
        let ib = format!("{key}$ibuf");
        design.add_cell(&ib, CellKind::IobIn);
        let (port, bit) = match key.split_once('[') {
            Some((p, b)) => (p.to_string(), b.trim_end_matches(']').to_string()),
            None => (key.clone(), "0".to_string()),
        };
        design.set_cell_attr(&ib, "PORT", port)?;
        design.set_cell_attr(&ib, "BIT", bit)?;
        if !key.contains('[') {
            design.set_cell_attr(&ib, "VECTOR", "0")?;
        }
        design.set_cell_attr(&ib, "LOC", pin.clone())?;
        design.connect(&format!("{key}$pad"), &ib, "PAD");
        if clocked {
            let raw = format!("{net}$pad_o");
            let gb = format!("{key}$gb");
            design.connect(&raw, &ib, "O");
            design.add_cell(&gb, CellKind::ClockBuf);
            design.connect(&raw, &gb, "I");
            design.connect(&net, &gb, "O");
            out.clocks.push(key.clone());
        } else {
            design.connect(&net, &ib, "O");
        }
        out.pads.push((key, pin, PortDir::In));
    }
    design.rebuild_indexes();

    if !missing.is_empty() {
        return Err(format!(
            "no set_io for {} port bit(s): {} (every pad needs a pin)",
            missing.len(),
            missing.join(", ")
        ));
    }
    out.unused = cons.iter().zip(&used).filter(|(_, u)| !**u).map(|(c, _)| c.clone()).collect();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcf::parse_pcf;

    /// The shape the v2 front end produces: bare input nets, IobOut cells
    /// with PORT/BIT.
    fn v2_like() -> Design {
        let mut d = Design::new("t");
        d.add_port("clk", PortDir::In);
        d.add_port("sw", PortDir::In);
        d.add_port("led", PortDir::Out);
        d.add_port("done", PortDir::Out);
        d.add_cell("r", CellKind::Hff);
        d.connect("clk", "r", "CLK");
        d.connect("sw_1", "r", "D");
        d.connect("q", "r", "Q");
        for (i, cell) in ["u_iob", "u_iob1"].iter().enumerate() {
            d.add_cell(cell, CellKind::IobOut);
            d.set_cell_attr(cell, "PORT", "led").unwrap();
            d.set_cell_attr(cell, "BIT", i.to_string()).unwrap();
            d.connect("q", cell, "I");
            d.connect("led", cell, "PAD");
        }
        d.add_cell("u_iob2", CellKind::IobOut);
        d.set_cell_attr("u_iob2", "PORT", "done").unwrap();
        d.set_cell_attr("u_iob2", "BIT", "0").unwrap();
        d.connect("q", "u_iob2", "I");
        d.connect("done", "u_iob2", "PAD");
        d
    }

    #[test]
    fn binds_v2_ports_and_buffers_the_clock() {
        let mut d = v2_like();
        let pcf = parse_pcf("set_io clk J3\nset_io sw[1] B1\nset_io sw[0] B2\nset_io led[0] B5\nset_io led[1] B4\nset_io done A2\n", "t.pcf").unwrap();
        let b = bind_pcf(&mut d, &pcf).unwrap();
        assert_eq!(b.clocks, vec!["clk".to_string()]);
        assert_eq!(b.unused.len(), 1, "sw[0] is never read");
        assert_eq!(d.cell("u_iob1").unwrap().attrs.get("LOC"), Some("B4"));
        assert_eq!(d.cell("u_iob2").unwrap().attrs.get("LOC"), Some("A2"));
        assert_eq!(d.cell("sw[1]$ibuf").unwrap().attrs.get("LOC"), Some("B1"));
        let pins = d.pin_index();
        assert_eq!(pins.net_on("clk$gb", "O").map(resolve), Some("clk"));
        assert_eq!(pins.net_on("sw[1]$ibuf", "O").map(resolve), Some("sw_1"));
        assert_eq!(io_port_name(&d, "u_iob2").as_deref(), Some("done"));
        assert_eq!(io_port_name(&d, "u_iob1").as_deref(), Some("led[1]"));
    }

    #[test]
    fn a_pad_without_set_io_is_an_error() {
        let mut d = v2_like();
        let pcf = parse_pcf("set_io clk J3\nset_io led[0] B5\nset_io done A2\n", "t.pcf").unwrap();
        let e = bind_pcf(&mut d, &pcf).unwrap_err();
        assert!(e.contains("led[1]") && e.contains("sw[1]"), "{e}");
    }
}
