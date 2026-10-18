//! Universal EDA Netlist Interchange Engine (`crates/helion-proj`).
//!
//! Provides lossless bidirectional import and export between [`helion_ir::Design`]
//! and standard industry EDA netlist formats:
//! - **Yosys / nextpnr JSON (`write_yosys_json`, `read_yosys_json`)**:
//!   Standard `"modules"` JSON with `"ports"`, `"cells"` (`"LUT6"`, `"CARRY4"`,
//!   `"FDRE"`, `"MUXF7"`, `"MUXF8"`, `"RAMB18E1"`, `"DSP48E1"`, `"OBUF"`, `"IBUF"`,
//!   `"BUFG"`), `"parameters"` (`"INIT"`, etc.), `"port_directions"`,
//!   `"connections"`, and `"netnames"` with 1-based bit indices and `"0"`/`"1"` constants.
//! - **Berkeley `.blif` / `.eblif` (`write_eblif`, `read_eblif`)**:
//!   `.model`, `.inputs`, `.outputs`, `.names` (SOP truth tables <-> 64-bit `Lut6`
//!   `INIT`), `.latch`, and `.subckt` (`CARRY4`, `FDRE`, `RAMB18E1`, `DSP48E1`,
//!   `OBUF`, `IBUF`, `BUFG`) with `.param`, `.attr`, and `.cname`.
//! - **IEEE EDIF 2 0 0 (`write_edif`, `read_edif`)**:
//!   Vivado/Synplify-compatible `(edif ... (library UNISIMS ...) (cell ...) (net ... (joined ...)))`
//!   S-expression netlists.
//! - **Structural Gate-Level UNISIM Verilog (`write_verilog_netlist`, `read_verilog_netlist`)**:
//!   `LUT6 #(.INIT(64'h...)) u_lut (.I0(...), .O(...));`, `FDRE`, `CARRY4`, `RAMB18E1`,
//!   `DSP48E1`, `OBUF`, `IBUF`, `BUFG`.

use helion_ir::{Attrs, Cell, CellKind, Design, PortDir, Spur, intern, resolve};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Supported EDA netlist interchange formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetlistFormat {
    YosysJson,
    Eblif,
    Edif,
    VerilogNetlist,
    Hnf,
}

fn is_output_pin_for_cell(kind: &CellKind, pin: &str) -> bool {
    match kind {
        CellKind::Lut6 { .. } | CellKind::IobIn | CellKind::ClockBuf => pin == "O",
        CellKind::Hff => pin == "Q",
        CellKind::IobOut => pin == "PAD" || pin == "O",
        CellKind::Mac27 => pin == "P" || pin == "O",
        CellKind::Dsp48e1 { .. } => pin == "P" || pin.starts_with("P["),
        CellKind::Bram18 => pin == "O" || pin.starts_with("DO"),
        CellKind::Bram18e1 { .. } => pin.starts_with("DOA") || pin.starts_with("DOB"),
        CellKind::Carry4 => pin.starts_with("O") || pin.starts_with("CO"),
        CellKind::Ila { .. } | CellKind::BlackBox { .. } => false,
    }
}

fn cell_type_name(c: &Cell) -> String {
    if let Some(orig) = c.attrs.get("UNISIM_ORIG") {
        return orig.to_string();
    }
    match &c.kind {
        CellKind::Lut6 { .. } => "LUT6".to_string(),
        CellKind::Hff => "FDRE".to_string(),
        CellKind::Carry4 => "CARRY4".to_string(),
        CellKind::Bram18 | CellKind::Bram18e1 { .. } => "RAMB18E1".to_string(),
        CellKind::Mac27 | CellKind::Dsp48e1 { .. } => "DSP48E1".to_string(),
        CellKind::IobOut => "OBUF".to_string(),
        CellKind::IobIn => "IBUF".to_string(),
        CellKind::ClockBuf => "BUFG".to_string(),
        CellKind::Ila { .. } => "ILA".to_string(),
        CellKind::BlackBox { module } => resolve(*module).to_string(),
    }
}

fn parse_cell_type_and_params(
    cell_type: &str,
    params: &BTreeMap<String, String>,
) -> Result<(CellKind, Attrs), String> {
    let mut attrs = Attrs::default();
    for (k, v) in params {
        if !matches!(
            k.as_str(),
            "INIT"
                | "OPMODE"
                | "ALUMODE"
                | "READ_WIDTH_A"
                | "READ_WIDTH_B"
                | "WRITE_WIDTH_A"
                | "WRITE_WIDTH_B"
        ) {
            attrs.set(k.clone(), v.clone());
        }
    }
    let up = cell_type.trim().to_ascii_uppercase();
    match up.as_str() {
        "LUT1" | "LUT2" | "LUT3" | "LUT4" | "LUT5" | "LUT6" | "$LUT" => {
            let init = params
                .get("INIT")
                .map(|s| parse_init_u64(s))
                .transpose()?
                .unwrap_or(0);
            Ok((CellKind::Lut6 { init }, attrs))
        }
        "MUXF7" | "MUXF8" => {
            // 2:1 mux: O = S ? I1 : I0 -> truth table with I0=bit0, I1=bit1, I2(S)=bit2
            let init = params
                .get("INIT")
                .map(|s| parse_init_u64(s))
                .transpose()?
                .unwrap_or(0xD8D8_D8D8_D8D8_D8D8);
            attrs.set("UNISIM_ORIG", up);
            Ok((CellKind::Lut6 { init }, attrs))
        }
        "FDRE" | "FDSE" | "FDCE" | "FDPE" | "HFF" | "$DFF" | "$DFFE" => {
            Ok((CellKind::Hff, attrs))
        }
        "CARRY4" => Ok((CellKind::Carry4, attrs)),
        "RAMB18E1" | "BRAM18" => {
            let ra = params
                .get("READ_WIDTH_A")
                .and_then(|v| parse_init_u64(v).ok())
                .unwrap_or(18) as u8;
            let rb = params
                .get("READ_WIDTH_B")
                .and_then(|v| parse_init_u64(v).ok())
                .unwrap_or(18) as u8;
            let wa = params
                .get("WRITE_WIDTH_A")
                .and_then(|v| parse_init_u64(v).ok())
                .unwrap_or(18) as u8;
            let wb = params
                .get("WRITE_WIDTH_B")
                .and_then(|v| parse_init_u64(v).ok())
                .unwrap_or(18) as u8;
            if let Some(init_s) = params.get("INIT") {
                attrs.set("INIT", init_s.clone());
            }
            if params.contains_key("READ_WIDTH_A")
                || params.contains_key("WRITE_WIDTH_A")
                || up == "RAMB18E1"
            {
                Ok((
                    CellKind::Bram18e1 {
                        read_width_a: ra,
                        read_width_b: rb,
                        write_width_a: wa,
                        write_width_b: wb,
                    },
                    attrs,
                ))
            } else {
                Ok((CellKind::Bram18, attrs))
            }
        }
        "DSP48E1" => {
            let opmode = params
                .get("OPMODE")
                .and_then(|v| parse_init_u64(v).ok())
                .unwrap_or(0b0000101) as u8;
            let alumode = params
                .get("ALUMODE")
                .and_then(|v| parse_init_u64(v).ok())
                .unwrap_or(0) as u8;
            Ok((CellKind::Dsp48e1 { opmode, alumode }, attrs))
        }
        "MAC27" => Ok((CellKind::Mac27, attrs)),
        "OBUF" | "IOBOUT" => Ok((CellKind::IobOut, attrs)),
        "IBUF" | "IOBIN" => Ok((CellKind::IobIn, attrs)),
        "BUFG" | "BUFGCTRL" | "CLOCKBUF" => Ok((CellKind::ClockBuf, attrs)),
        other => Ok((
            CellKind::BlackBox {
                module: intern(other),
            },
            attrs,
        )),
    }
}

/// Normalize UNISIM pin names onto Helion's canonical IR pin names (`FDRE` `C` -> `CLK`, etc.).
fn normalize_pin_to_ir(cell_type: &str, kind: &CellKind, pin: &str) -> String {
    let up = cell_type.trim().to_ascii_uppercase();
    if matches!(kind, CellKind::Hff) && pin == "C" {
        return "CLK".to_string();
    }
    if (up == "MUXF7" || up == "MUXF8") && pin == "S" {
        return "I2".to_string();
    }
    if matches!(kind, CellKind::IobOut) && pin == "O" {
        return "PAD".to_string();
    }
    pin.to_string()
}

fn parse_init_u64(raw: &str) -> Result<u64, String> {
    let s = raw.trim().trim_matches('"').trim();
    if let Some((_, hex)) = s.split_once("'h").or_else(|| s.split_once("'H")) {
        let clean: String = hex.chars().filter(|&c| c != '_').collect();
        return u64::from_str_radix(&clean, 16)
            .map_err(|e| format!("invalid hex INIT '{raw}': {e}"));
    }
    if let Some((_, bin)) = s.split_once("'b").or_else(|| s.split_once("'B")) {
        let clean: String = bin.chars().filter(|&c| c != '_').collect();
        return u64::from_str_radix(&clean, 2)
            .map_err(|e| format!("invalid binary INIT '{raw}': {e}"));
    }
    if let Some((_, dec)) = s.split_once("'d").or_else(|| s.split_once("'D")) {
        let clean: String = dec.chars().filter(|&c| c != '_').collect();
        return clean
            .parse::<u64>()
            .map_err(|e| format!("invalid decimal INIT '{raw}': {e}"));
    }
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        let clean: String = hex.chars().filter(|&c| c != '_').collect();
        return u64::from_str_radix(&clean, 16)
            .map_err(|e| format!("invalid hex INIT '{raw}': {e}"));
    }
    if s.len() > 16 && s.chars().all(|c| c == '0' || c == '1') {
        return u64::from_str_radix(s, 2).map_err(|e| format!("invalid binary INIT '{raw}': {e}"));
    }
    if let Ok(d) = s.parse::<u64>() {
        return Ok(d);
    }
    u64::from_str_radix(s, 16).map_err(|e| format!("invalid INIT '{raw}': {e}"))
}

// =============================================================================
// 1. Yosys / nextpnr JSON Interchange (`write_yosys_json`, `read_yosys_json`)
// =============================================================================

/// Serialize a [`Design`] to standard Yosys / nextpnr `"modules"` JSON format.
pub fn write_yosys_json(design: &Design) -> String {
    let mod_name = resolve(design.name);

    // Assign deterministic 1-based bit indices (starting at 2; 0 and 1 are reserved for constants)
    let mut net_to_id: BTreeMap<String, usize> = BTreeMap::new();
    let mut next_bit_id = 2usize;

    for p in &design.ports {
        let pname = resolve(p.name).to_string();
        net_to_id.entry(pname).or_insert_with(|| {
            let id = next_bit_id;
            next_bit_id += 1;
            id
        });
    }
    for n in &design.nets {
        let nname = resolve(n.name).to_string();
        net_to_id.entry(nname).or_insert_with(|| {
            let id = next_bit_id;
            next_bit_id += 1;
            id
        });
    }

    // Build cell -> [(pin, net_name)] map in deterministic order
    let mut cell_conns: HashMap<Spur, BTreeMap<String, String>> = HashMap::new();
    for n in &design.nets {
        let nname = resolve(n.name).to_string();
        for ep in &n.endpoints {
            cell_conns
                .entry(ep.cell)
                .or_default()
                .insert(resolve(ep.pin).to_string(), nname.clone());
        }
    }

    // Ports JSON
    let mut port_entries = Vec::new();
    for p in &design.ports {
        let pname = resolve(p.name);
        let dir = match p.dir {
            PortDir::In => "input",
            PortDir::Out => "output",
            PortDir::Inout => "inout",
        };
        let bit_id = net_to_id[pname];
        port_entries.push(format!(
            "        \"{}\": {{\n          \"direction\": \"{}\",\n          \"bits\": [ {} ]\n        }}",
            json_esc(pname),
            dir,
            bit_id
        ));
    }

    // Cells JSON
    let mut cell_entries = Vec::new();
    for c in &design.cells {
        let cname = resolve(c.name);
        let ctype = cell_type_name(c);
        let mut params: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in &c.attrs.map {
            if k != "UNISIM_ORIG" {
                params.insert(k.clone(), v.clone());
            }
        }
        match &c.kind {
            CellKind::Lut6 { init } => {
                params.insert("INIT".to_string(), format!("64'h{init:016x}"));
            }
            CellKind::Dsp48e1 { opmode, alumode } => {
                params.insert("OPMODE".to_string(), opmode.to_string());
                params.insert("ALUMODE".to_string(), alumode.to_string());
            }
            CellKind::Bram18e1 {
                read_width_a,
                read_width_b,
                write_width_a,
                write_width_b,
            } => {
                params.insert("READ_WIDTH_A".to_string(), read_width_a.to_string());
                params.insert("READ_WIDTH_B".to_string(), read_width_b.to_string());
                params.insert("WRITE_WIDTH_A".to_string(), write_width_a.to_string());
                params.insert("WRITE_WIDTH_B".to_string(), write_width_b.to_string());
            }
            _ => {}
        }

        let params_str = if params.is_empty() {
            "{}".to_string()
        } else {
            let rows: Vec<String> = params
                .iter()
                .map(|(k, v)| format!("            \"{}\": \"{}\"", json_esc(k), json_esc(v)))
                .collect();
            format!("{{\n{}\n          }}", rows.join(",\n"))
        };

        let conns = cell_conns.get(&c.name);
        let mut dir_rows = Vec::new();
        let mut conn_rows = Vec::new();
        if let Some(pin_map) = conns {
            for (pin, net_name) in pin_map {
                let pdir = if is_output_pin_for_cell(&c.kind, pin) {
                    "output"
                } else {
                    "input"
                };
                let bit_val = if net_name == "1'b0" || net_name == "$false" {
                    "\"0\"".to_string()
                } else if net_name == "1'b1" || net_name == "$true" {
                    "\"1\"".to_string()
                } else {
                    net_to_id.get(net_name).copied().unwrap_or(2).to_string()
                };
                dir_rows.push(format!("            \"{}\": \"{}\"", json_esc(pin), pdir));
                conn_rows.push(format!(
                    "            \"{}\": [ {} ]",
                    json_esc(pin),
                    bit_val
                ));
            }
        }

        let dirs_str = if dir_rows.is_empty() {
            "{}".to_string()
        } else {
            format!("{{\n{}\n          }}", dir_rows.join(",\n"))
        };
        let conns_str = if conn_rows.is_empty() {
            "{}".to_string()
        } else {
            format!("{{\n{}\n          }}", conn_rows.join(",\n"))
        };

        cell_entries.push(format!(
            "        \"{}\": {{\n          \"hide_name\": 0,\n          \"type\": \"{}\",\n          \"parameters\": {},\n          \"attributes\": {{}},\n          \"port_directions\": {},\n          \"connections\": {}\n        }}",
            json_esc(cname),
            json_esc(&ctype),
            params_str,
            dirs_str,
            conns_str
        ));
    }

    // Netnames JSON
    let mut netname_entries = Vec::new();
    for (nname, &bit_id) in &net_to_id {
        let mut attrs_rows = Vec::new();
        if let Some(net) = design.net(nname) {
            for (k, v) in &net.attrs.map {
                attrs_rows.push(format!("            \"{}\": \"{}\"", json_esc(k), json_esc(v)));
            }
        }
        let attrs_str = if attrs_rows.is_empty() {
            "{}".to_string()
        } else {
            format!("{{\n{}\n          }}", attrs_rows.join(",\n"))
        };
        netname_entries.push(format!(
            "        \"{}\": {{\n          \"hide_name\": 0,\n          \"bits\": [ {} ],\n          \"attributes\": {}\n        }}",
            json_esc(nname),
            bit_id,
            attrs_str
        ));
    }

    format!(
        "{{\n  \"creator\": \"Helion EDA\",\n  \"modules\": {{\n    \"{}\": {{\n      \"attributes\": {{}},\n      \"ports\": {{\n{}\n      }},\n      \"cells\": {{\n{}\n      }},\n      \"netnames\": {{\n{}\n      }}\n    }}\n  }}\n}}\n",
        json_esc(mod_name),
        port_entries.join(",\n"),
        cell_entries.join(",\n"),
        netname_entries.join(",\n")
    )
}

/// Parse a Yosys / nextpnr `"modules"` JSON netlist into a [`Design`].
pub fn read_yosys_json(json_text: &str) -> Result<Design, String> {
    let root = JsonParser::new(json_text).parse_value()?;
    let root_obj = root.as_obj().ok_or("Yosys JSON root must be an object")?;
    let modules = root_obj
        .get("modules")
        .and_then(|v| v.as_obj())
        .ok_or("Yosys JSON missing 'modules' object")?;
    let (mod_name, mod_val) = modules
        .iter()
        .next()
        .ok_or("Yosys JSON 'modules' is empty")?;
    let mod_obj = mod_val.as_obj().ok_or("module entry must be an object")?;

    let mut d = Design::new(mod_name);
    let mut bit_to_net: HashMap<usize, String> = HashMap::new();

    // 1. Parse netnames first to build bit_id -> net_name mapping
    let mut net_attrs_to_apply: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    if let Some(netnames) = mod_obj.get("netnames").and_then(|v| v.as_obj()) {
        for (nname, nval) in netnames {
            let Some(nobj) = nval.as_obj() else {
                continue;
            };
            if let Some(bits) = nobj.get("bits").and_then(|v| v.as_arr()) {
                if bits.len() == 1 {
                    if let Some(id) = bits[0].as_usize() {
                        bit_to_net.insert(id, nname.clone());
                    }
                } else {
                    for (idx, b) in bits.iter().enumerate() {
                        if let Some(id) = b.as_usize() {
                            bit_to_net.insert(id, format!("{nname}_{idx}"));
                        }
                    }
                }
            }
            if let Some(attrs) = nobj.get("attributes").and_then(|v| v.as_obj()) {
                let mut map = BTreeMap::new();
                for (k, v) in attrs {
                    if let Some(s) = v.as_str() {
                        map.insert(k.clone(), s.to_string());
                    }
                }
                if !map.is_empty() {
                    net_attrs_to_apply.push((nname.clone(), map));
                }
            }
        }
    }

    // 2. Parse ports
    if let Some(ports) = mod_obj.get("ports").and_then(|v| v.as_obj()) {
        for (pname, pval) in ports {
            let Some(pobj) = pval.as_obj() else {
                continue;
            };
            let dir = match pobj
                .get("direction")
                .and_then(|v| v.as_str())
                .unwrap_or("input")
            {
                "output" => PortDir::Out,
                "inout" => PortDir::Inout,
                _ => PortDir::In,
            };
            d.add_port(pname, dir);
            if let Some(bits) = pobj.get("bits").and_then(|v| v.as_arr()) {
                if bits.len() == 1 {
                    if let Some(id) = bits[0].as_usize() {
                        bit_to_net.entry(id).or_insert_with(|| pname.clone());
                    }
                } else {
                    for (idx, b) in bits.iter().enumerate() {
                        if let Some(id) = b.as_usize() {
                            bit_to_net
                                .entry(id)
                                .or_insert_with(|| format!("{pname}_{idx}"));
                        }
                    }
                }
            }
        }
    }

    // 3. Parse cells and connections
    if let Some(cells) = mod_obj.get("cells").and_then(|v| v.as_obj()) {
        for (cname, cval) in cells {
            let Some(cobj) = cval.as_obj() else {
                continue;
            };
            let ctype = cobj
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("LUT6");
            let mut params = BTreeMap::new();
            if let Some(pobj) = cobj.get("parameters").and_then(|v| v.as_obj()) {
                for (k, v) in pobj {
                    if let Some(s) = v.as_str() {
                        params.insert(k.clone(), s.to_string());
                    } else if let Some(n) = v.as_usize() {
                        params.insert(k.clone(), n.to_string());
                    }
                }
            }
            let (kind, attrs) = parse_cell_type_and_params(ctype, &params)?;
            d.push_cell(Cell {
                name: intern(cname),
                kind: kind.clone(),
                attrs,
            });

            if let Some(conns) = cobj.get("connections").and_then(|v| v.as_obj()) {
                for (pin_raw, bit_arr_val) in conns {
                    let Some(bit_arr) = bit_arr_val.as_arr() else {
                        continue;
                    };
                    let pin_ir = normalize_pin_to_ir(ctype, &kind, pin_raw);
                    for (idx, bval) in bit_arr.iter().enumerate() {
                        let pin_name = if bit_arr.len() == 1 {
                            pin_ir.clone()
                        } else {
                            format!("{pin_ir}[{idx}]")
                        };
                        let net_name = if let Some(id) = bval.as_usize() {
                            bit_to_net
                                .get(&id)
                                .cloned()
                                .unwrap_or_else(|| format!("_net_{id}"))
                        } else if let Some(s) = bval.as_str() {
                            match s {
                                "0" => "1'b0".to_string(),
                                "1" => "1'b1".to_string(),
                                other => other.to_string(),
                            }
                        } else {
                            continue;
                        };
                        d.connect(&net_name, cname, &pin_name);
                    }
                }
            }
        }
    }

    for (nname, map) in net_attrs_to_apply {
        if d.net(&nname).is_some() {
            for (k, v) in map {
                let _ = d.set_net_attr(&nname, &k, v);
            }
        }
    }

    Ok(d)
}

// =============================================================================
// 2. Berkeley `.blif` / `.eblif` Interchange (`write_eblif`, `read_eblif`)
// =============================================================================

/// Emit a [`Design`] in extended Berkeley Logic Interchange Format (`.eblif` / `.blif`).
pub fn write_eblif(design: &Design) -> String {
    let mut out = format!(
        "# Generated by Helion EDA Netlist Interchange\n.model {}\n",
        resolve(design.name)
    );

    let inputs: Vec<&str> = design
        .ports
        .iter()
        .filter(|p| matches!(p.dir, PortDir::In | PortDir::Inout))
        .map(|p| resolve(p.name))
        .collect();
    let outputs: Vec<&str> = design
        .ports
        .iter()
        .filter(|p| matches!(p.dir, PortDir::Out))
        .map(|p| resolve(p.name))
        .collect();

    if !inputs.is_empty() {
        out.push_str(&format!(".inputs {}\n", inputs.join(" ")));
    }
    if !outputs.is_empty() {
        out.push_str(&format!(".outputs {}\n", outputs.join(" ")));
    }

    let pins = design.pin_index();
    for c in &design.cells {
        let cname = resolve(c.name);
        match &c.kind {
            CellKind::Lut6 { init } => {
                const IPINS: [&str; 6] = ["I0", "I1", "I2", "I3", "I4", "I5"];
                let mut connected_ins: Vec<(usize, String)> = Vec::new();
                for (bit_idx, ipin) in IPINS.iter().enumerate() {
                    if let Some(net) = pins.net_on(cname, ipin) {
                        connected_ins.push((bit_idx, resolve(net).to_string()));
                    }
                }
                let Some(o_net) = pins.net_on(cname, "O") else {
                    continue;
                };
                let mut names_line = String::from(".names");
                for (_, inet) in &connected_ins {
                    names_line.push(' ');
                    names_line.push_str(inet);
                }
                names_line.push(' ');
                names_line.push_str(resolve(o_net));
                out.push_str(&names_line);
                out.push('\n');
                out.push_str(&format!(".cname {cname}\n"));
                out.push_str(&format!(".attr LUT_INIT {init:#018x}\n"));
                for (k, v) in &c.attrs.map {
                    out.push_str(&format!(".attr {k} {v}\n"));
                }

                // Emit SOP truth-table rows over connected inputs (I0 = first column)
                if connected_ins.is_empty() {
                    if (*init & 1) == 1 {
                        out.push_str("1\n");
                    }
                } else {
                    let k = connected_ins.len();
                    let n_rows = 1usize << k;
                    for row_mask in 0..n_rows {
                        let mut addr = 0usize;
                        for (col_idx, (orig_bit, _)) in connected_ins.iter().enumerate() {
                            if (row_mask >> col_idx) & 1 == 1 {
                                addr |= 1usize << orig_bit;
                            }
                        }
                        if (*init >> addr) & 1 == 1 {
                            let mut cube = String::with_capacity(k + 2);
                            for col_idx in 0..k {
                                cube.push(if (row_mask >> col_idx) & 1 == 1 {
                                    '1'
                                } else {
                                    '0'
                                });
                            }
                            cube.push_str(" 1\n");
                            out.push_str(&cube);
                        }
                    }
                }
            }
            CellKind::Hff => {
                let d_net = pins.net_on(cname, "D").map(resolve).unwrap_or("$undef");
                let q_net = pins.net_on(cname, "Q").map(resolve).unwrap_or("$undef");
                let clk_net = pins.net_on(cname, "CLK").map(resolve).unwrap_or("clk");
                out.push_str(&format!(".latch {d_net} {q_net} re {clk_net} 0\n"));
                out.push_str(&format!(".cname {cname}\n"));
                for (k, v) in &c.attrs.map {
                    out.push_str(&format!(".attr {k} {v}\n"));
                }
            }
            _ => {
                let ctype = cell_type_name(c);
                let mut pin_pairs = Vec::new();
                for n in &design.nets {
                    for ep in &n.endpoints {
                        if ep.cell == c.name {
                            pin_pairs.push((
                                resolve(ep.pin).to_string(),
                                resolve(n.name).to_string(),
                            ));
                        }
                    }
                }
                pin_pairs.sort();
                let mut sub = format!(".subckt {ctype}");
                for (p, n) in &pin_pairs {
                    sub.push_str(&format!(" {p}={n}"));
                }
                out.push_str(&sub);
                out.push('\n');
                out.push_str(&format!(".cname {cname}\n"));
                match &c.kind {
                    CellKind::Dsp48e1 { opmode, alumode } => {
                        out.push_str(&format!(".param OPMODE {opmode}\n"));
                        out.push_str(&format!(".param ALUMODE {alumode}\n"));
                    }
                    CellKind::Bram18e1 {
                        read_width_a,
                        read_width_b,
                        write_width_a,
                        write_width_b,
                    } => {
                        out.push_str(&format!(".param READ_WIDTH_A {read_width_a}\n"));
                        out.push_str(&format!(".param READ_WIDTH_B {read_width_b}\n"));
                        out.push_str(&format!(".param WRITE_WIDTH_A {write_width_a}\n"));
                        out.push_str(&format!(".param WRITE_WIDTH_B {write_width_b}\n"));
                    }
                    _ => {}
                }
                for (k, v) in &c.attrs.map {
                    out.push_str(&format!(".param {k} {v}\n"));
                }
            }
        }
    }

    out.push_str(".end\n");
    out
}

/// Parse a Berkeley `.blif` / `.eblif` netlist into a [`Design`].
pub fn read_eblif(text: &str) -> Result<Design, String> {
    let mut joined_lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for raw in text.lines() {
        let no_comment = raw.split('#').next().unwrap_or("").trim_end();
        if let Some(stripped) = no_comment.strip_suffix('\\') {
            cur.push_str(stripped);
            cur.push(' ');
        } else {
            cur.push_str(no_comment);
            let trimmed = cur.trim().to_string();
            if !trimmed.is_empty() {
                joined_lines.push(trimmed);
            }
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        joined_lines.push(cur.trim().to_string());
    }

    let mut d = Design::new("top");
    let mut auto_idx = 0usize;
    let mut i = 0usize;

    while i < joined_lines.len() {
        let line = &joined_lines[i];
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.is_empty() {
            i += 1;
            continue;
        }
        match toks[0] {
            ".model" => {
                if let Some(m) = toks.get(1) {
                    d.name = intern(m);
                }
                i += 1;
            }
            ".inputs" => {
                for &p in &toks[1..] {
                    d.add_port(p, PortDir::In);
                }
                i += 1;
            }
            ".outputs" => {
                for &p in &toks[1..] {
                    d.add_port(p, PortDir::Out);
                }
                i += 1;
            }
            ".names" => {
                let nets = &toks[1..];
                if nets.is_empty() {
                    return Err(".names requires at least an output net".into());
                }
                let out_net = nets[nets.len() - 1];
                let in_nets = &nets[..nets.len() - 1];
                let mut cname: Option<String> = None;
                let mut explicit_init: Option<u64> = None;
                let mut attrs = Attrs::default();
                let mut cubes: Vec<(&str, &str)> = Vec::new();
                i += 1;
                while i < joined_lines.len() {
                    let sub = &joined_lines[i];
                    if sub.starts_with('.') {
                        let stoks: Vec<&str> = sub.split_whitespace().collect();
                        match stoks.first().copied().unwrap_or("") {
                            ".cname" => {
                                if let Some(n) = stoks.get(1) {
                                    cname = Some((*n).to_string());
                                }
                                i += 1;
                            }
                            ".attr" | ".param" => {
                                if stoks.len() >= 3 {
                                    let k = stoks[1];
                                    let v = stoks[2..].join(" ");
                                    if k == "LUT_INIT" || k == "INIT" {
                                        if let Ok(val) = parse_init_u64(&v) {
                                            explicit_init = Some(val);
                                        }
                                    } else {
                                        attrs.set(k, v);
                                    }
                                }
                                i += 1;
                            }
                            _ => break,
                        }
                    } else {
                        let ctoks: Vec<&str> = sub.split_whitespace().collect();
                        if ctoks.len() == 1 {
                            cubes.push(("", ctoks[0]));
                        } else if ctoks.len() >= 2 {
                            cubes.push((ctoks[0], ctoks[1]));
                        }
                        i += 1;
                    }
                }
                let init = if let Some(v) = explicit_init {
                    v
                } else {
                    sop_cubes_to_lut6_init(in_nets.len(), &cubes)
                };
                let cell_name = cname.unwrap_or_else(|| {
                    auto_idx += 1;
                    format!("u_lut_{auto_idx}")
                });
                d.push_cell(Cell {
                    name: intern(&cell_name),
                    kind: CellKind::Lut6 { init },
                    attrs,
                });
                for (pin_idx, &inet) in in_nets.iter().take(6).enumerate() {
                    d.connect(inet, &cell_name, &format!("I{pin_idx}"));
                }
                d.connect(out_net, &cell_name, "O");
            }
            ".latch" => {
                // .latch <d> <q> [<type> <clk> [<init>]]
                if toks.len() < 3 {
                    return Err(".latch requires <input> <output>".into());
                }
                let d_net = toks[1];
                let q_net = toks[2];
                let clk_net = toks.get(4).copied().unwrap_or("clk");
                let mut cname: Option<String> = None;
                let mut attrs = Attrs::default();
                i += 1;
                while i < joined_lines.len() {
                    let sub = &joined_lines[i];
                    let stoks: Vec<&str> = sub.split_whitespace().collect();
                    match stoks.first().copied().unwrap_or("") {
                        ".cname" => {
                            if let Some(n) = stoks.get(1) {
                                cname = Some((*n).to_string());
                            }
                            i += 1;
                        }
                        ".attr" | ".param" => {
                            if stoks.len() >= 3 {
                                attrs.set(stoks[1], stoks[2..].join(" "));
                            }
                            i += 1;
                        }
                        _ => break,
                    }
                }
                let cell_name = cname.unwrap_or_else(|| {
                    auto_idx += 1;
                    format!("u_ff_{auto_idx}")
                });
                d.push_cell(Cell {
                    name: intern(&cell_name),
                    kind: CellKind::Hff,
                    attrs,
                });
                d.connect(d_net, &cell_name, "D");
                d.connect(q_net, &cell_name, "Q");
                d.connect(clk_net, &cell_name, "CLK");
            }
            ".subckt" => {
                if toks.len() < 2 {
                    return Err(".subckt requires model name".into());
                }
                let ctype = toks[1];
                let mut conns: Vec<(String, String)> = Vec::new();
                for pair in &toks[2..] {
                    if let Some((pin, net)) = pair.split_once('=') {
                        conns.push((pin.to_string(), net.to_string()));
                    }
                }
                let mut cname: Option<String> = None;
                let mut params: BTreeMap<String, String> = BTreeMap::new();
                i += 1;
                while i < joined_lines.len() {
                    let sub = &joined_lines[i];
                    let stoks: Vec<&str> = sub.split_whitespace().collect();
                    match stoks.first().copied().unwrap_or("") {
                        ".cname" => {
                            if let Some(n) = stoks.get(1) {
                                cname = Some((*n).to_string());
                            }
                            i += 1;
                        }
                        ".param" | ".attr" => {
                            if stoks.len() >= 3 {
                                params.insert(stoks[1].to_string(), stoks[2..].join(" "));
                            }
                            i += 1;
                        }
                        _ => break,
                    }
                }
                let (kind, attrs) = parse_cell_type_and_params(ctype, &params)?;
                let cell_name = cname.unwrap_or_else(|| {
                    auto_idx += 1;
                    format!("u_cell_{auto_idx}")
                });
                d.push_cell(Cell {
                    name: intern(&cell_name),
                    kind: kind.clone(),
                    attrs,
                });
                for (pin, net) in conns {
                    let ir_pin = normalize_pin_to_ir(ctype, &kind, &pin);
                    d.connect(&net, &cell_name, &ir_pin);
                }
            }
            ".end" => break,
            _ => {
                i += 1;
            }
        }
    }

    Ok(d)
}

fn sop_cubes_to_lut6_init(k: usize, cubes: &[(&str, &str)]) -> u64 {
    if k == 0 {
        for &(_, out_val) in cubes {
            if out_val == "1" {
                return u64::MAX;
            }
        }
        return 0;
    }
    let k = k.min(6);
    let mut init = 0u64;
    for minterm in 0..64usize {
        let mut onset = false;
        let mut offset = false;
        for &(pat, out_val) in cubes {
            let mut matches_cube = true;
            for (bit_idx, ch) in pat.chars().take(k).enumerate() {
                let bit = (minterm >> bit_idx) & 1;
                match ch {
                    '1' if bit != 1 => {
                        matches_cube = false;
                        break;
                    }
                    '0' if bit != 0 => {
                        matches_cube = false;
                        break;
                    }
                    '-' => {}
                    _ => {}
                }
            }
            if matches_cube {
                if out_val == "1" {
                    onset = true;
                } else if out_val == "0" {
                    offset = true;
                }
            }
        }
        if onset && !offset {
            init |= 1u64 << minterm;
        }
    }
    init
}

// =============================================================================
// 3. IEEE EDIF 2 0 0 Interchange (`write_edif`, `read_edif`)
// =============================================================================

/// Serialize a [`Design`] to an IEEE EDIF 2 0 0 netlist S-expression string.
pub fn write_edif(design: &Design) -> String {
    let top_name = resolve(design.name);
    let mut prim_types: Vec<String> = design.cells.iter().map(cell_type_name).collect();
    prim_types.sort();
    prim_types.dedup();

    let mut s = format!(
        "(edif {top_name}\n  (edifVersion 2 0 0)\n  (edifLevel 0)\n  (keywordMap (keywordLevel 0))\n  (library UNISIMS\n    (edifLevel 0)\n    (technology (numberDefinition))\n"
    );
    for pt in &prim_types {
        s.push_str(&format!(
            "    (cell {pt} (cellType GENERIC) (view PRIM (viewType NETLIST) (interface)))\n"
        ));
    }
    s.push_str("  )\n");
    s.push_str(&format!(
        "  (library work\n    (edifLevel 0)\n    (technology (numberDefinition))\n    (cell {top_name}\n      (cellType GENERIC)\n      (view netlist\n        (viewType NETLIST)\n        (interface\n"
    ));

    for p in &design.ports {
        let dir = match p.dir {
            PortDir::In => "INPUT",
            PortDir::Out => "OUTPUT",
            PortDir::Inout => "INOUT",
        };
        s.push_str(&format!(
            "          (port {} (direction {dir}))\n",
            resolve(p.name)
        ));
    }
    s.push_str("        )\n        (contents\n");

    for c in &design.cells {
        let cname = resolve(c.name);
        let ctype = cell_type_name(c);
        s.push_str(&format!(
            "          (instance {cname}\n            (viewRef PRIM (cellRef {ctype} (libraryRef UNISIMS)))\n"
        ));
        match &c.kind {
            CellKind::Lut6 { init } => {
                s.push_str(&format!(
                    "            (property INIT (string \"64'h{init:016x}\"))\n"
                ));
            }
            CellKind::Dsp48e1 { opmode, alumode } => {
                s.push_str(&format!(
                    "            (property OPMODE (integer {opmode}))\n            (property ALUMODE (integer {alumode}))\n"
                ));
            }
            CellKind::Bram18e1 {
                read_width_a,
                read_width_b,
                write_width_a,
                write_width_b,
            } => {
                s.push_str(&format!(
                    "            (property READ_WIDTH_A (integer {read_width_a}))\n            (property READ_WIDTH_B (integer {read_width_b}))\n            (property WRITE_WIDTH_A (integer {write_width_a}))\n            (property WRITE_WIDTH_B (integer {write_width_b}))\n"
                ));
            }
            _ => {}
        }
        for (k, v) in &c.attrs.map {
            if k != "UNISIM_ORIG" {
                s.push_str(&format!(
                    "            (property {k} (string \"{}\"))\n",
                    json_esc(v)
                ));
            }
        }
        s.push_str("          )\n");
    }

    let port_names: HashSet<&str> = design.ports.iter().map(|p| resolve(p.name)).collect();
    for n in &design.nets {
        let nname = resolve(n.name);
        s.push_str(&format!("          (net {nname}\n            (joined\n"));
        if port_names.contains(nname) {
            s.push_str(&format!("              (portRef {nname})\n"));
        }
        for ep in &n.endpoints {
            s.push_str(&format!(
                "              (portRef {} (instanceRef {}))\n",
                resolve(ep.pin),
                resolve(ep.cell)
            ));
        }
        s.push_str("            )\n          )\n");
    }

    s.push_str(&format!(
        "        )\n      )\n    )\n  )\n  (design {top_name} (cellRef {top_name} (libraryRef work)))\n)\n"
    ));
    s
}

/// Parse an IEEE EDIF 2 0 0 S-expression netlist into a [`Design`].
pub fn read_edif(text: &str) -> Result<Design, String> {
    let sexpr = parse_sexpr(text)?;
    let list = sexpr.as_list().ok_or("EDIF root must be a list")?;
    if list.first().and_then(|x| x.as_atom()) != Some("edif") {
        return Err("EDIF root must start with (edif ...)".into());
    }
    let top_fallback = list.get(1).and_then(|x| x.as_atom()).unwrap_or("top");

    // Locate the `(cell <name> ... (view ... (interface ...) (contents ...)))` that has `(contents ...)`
    let mut target_cell: Option<&[Sexpr]> = None;
    for item in list {
        if let Some(lib_items) = item.as_list() {
            if lib_items.first().and_then(|x| x.as_atom()) == Some("library") {
                for sub in &lib_items[2..] {
                    if let Some(cell_items) = sub.as_list() {
                        if cell_items.first().and_then(|x| x.as_atom()) == Some("cell")
                            && cell_has_contents(cell_items)
                        {
                            target_cell = Some(cell_items);
                        }
                    }
                }
            }
        }
    }

    let cell_items = target_cell.ok_or("EDIF contains no cell with (contents ...)")?;
    let cell_name = edif_ident_of(cell_items.get(1)).unwrap_or_else(|| top_fallback.to_string());
    let mut d = Design::new(&cell_name);
    let mut cell_types: HashMap<String, (String, CellKind)> = HashMap::new();

    for sub in &cell_items[2..] {
        let Some(view_items) = sub.as_list() else {
            continue;
        };
        if view_items.first().and_then(|x| x.as_atom()) != Some("view") {
            continue;
        }
        for vsub in &view_items[2..] {
            let Some(vlist) = vsub.as_list() else {
                continue;
            };
            match vlist.first().and_then(|x| x.as_atom()) {
                Some("interface") => {
                    for pitem in &vlist[1..] {
                        let Some(plist) = pitem.as_list() else {
                            continue;
                        };
                        if plist.first().and_then(|x| x.as_atom()) == Some("port") {
                            let Some(pname) = edif_ident_of(plist.get(1)) else {
                                continue;
                            };
                            let mut dir = PortDir::In;
                            for pprop in &plist[2..] {
                                if let Some(dlist) = pprop.as_list() {
                                    if dlist.first().and_then(|x| x.as_atom()) == Some("direction")
                                    {
                                        dir = match dlist
                                            .get(1)
                                            .and_then(|x| x.as_atom())
                                            .unwrap_or("INPUT")
                                            .to_ascii_uppercase()
                                            .as_str()
                                        {
                                            "OUTPUT" => PortDir::Out,
                                            "INOUT" => PortDir::Inout,
                                            _ => PortDir::In,
                                        };
                                    }
                                }
                            }
                            d.add_port(&pname, dir);
                        }
                    }
                }
                Some("contents") => {
                    for citem in &vlist[1..] {
                        let Some(clist) = citem.as_list() else {
                            continue;
                        };
                        match clist.first().and_then(|x| x.as_atom()) {
                            Some("instance") => {
                                let Some(iname) = edif_ident_of(clist.get(1)) else {
                                    continue;
                                };
                                let mut ctype = "LUT6".to_string();
                                let mut params = BTreeMap::new();
                                for iprop in &clist[2..] {
                                    let Some(ilist) = iprop.as_list() else {
                                        continue;
                                    };
                                    match ilist.first().and_then(|x| x.as_atom()) {
                                        Some("viewRef") => {
                                            for vr in &ilist[2..] {
                                                if let Some(cr) = vr.as_list() {
                                                    if cr.first().and_then(|x| x.as_atom())
                                                        == Some("cellRef")
                                                    {
                                                        if let Some(ct) = edif_ident_of(cr.get(1)) {
                                                            ctype = ct;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        Some("property") => {
                                            if let Some(pk) = edif_ident_of(ilist.get(1)) {
                                                if let Some(pval_list) =
                                                    ilist.get(2).and_then(|x| x.as_list())
                                                {
                                                    if let Some(pv) =
                                                        pval_list.get(1).and_then(|x| x.as_atom())
                                                    {
                                                        params.insert(pk, pv.to_string());
                                                    }
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                let (kind, attrs) = parse_cell_type_and_params(&ctype, &params)?;
                                cell_types.insert(iname.clone(), (ctype, kind.clone()));
                                d.push_cell(Cell {
                                    name: intern(&iname),
                                    kind,
                                    attrs,
                                });
                            }
                            Some("net") => {
                                let Some(nname) = edif_ident_of(clist.get(1)) else {
                                    continue;
                                };
                                for nprop in &clist[2..] {
                                    let Some(jlist) = nprop.as_list() else {
                                        continue;
                                    };
                                    if jlist.first().and_then(|x| x.as_atom()) != Some("joined") {
                                        continue;
                                    }
                                    for pref in &jlist[1..] {
                                        let Some(pr_list) = pref.as_list() else {
                                            continue;
                                        };
                                        if pr_list.first().and_then(|x| x.as_atom())
                                            != Some("portRef")
                                        {
                                            continue;
                                        }
                                        let Some(pin_raw) = edif_ident_of(pr_list.get(1)) else {
                                            continue;
                                        };
                                        let mut inst_ref: Option<String> = None;
                                        for sub_pr in &pr_list[2..] {
                                            if let Some(ir_list) = sub_pr.as_list() {
                                                if ir_list.first().and_then(|x| x.as_atom())
                                                    == Some("instanceRef")
                                                {
                                                    inst_ref = edif_ident_of(ir_list.get(1));
                                                }
                                            }
                                        }
                                        if let Some(cname) = inst_ref {
                                            let pin = if let Some((ct, ck)) = cell_types.get(&cname)
                                            {
                                                normalize_pin_to_ir(ct, ck, &pin_raw)
                                            } else {
                                                pin_raw
                                            };
                                            d.connect(&nname, &cname, &pin);
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    Ok(d)
}

fn cell_has_contents(cell_items: &[Sexpr]) -> bool {
    for sub in &cell_items[2..] {
        if let Some(vlist) = sub.as_list() {
            if vlist.first().and_then(|x| x.as_atom()) == Some("view") {
                for vsub in &vlist[2..] {
                    if let Some(clist) = vsub.as_list() {
                        if clist.first().and_then(|x| x.as_atom()) == Some("contents") {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

fn edif_ident_of(node: Option<&Sexpr>) -> Option<String> {
    let n = node?;
    if let Some(a) = n.as_atom() {
        return Some(a.to_string());
    }
    if let Some(list) = n.as_list() {
        if list.first().and_then(|x| x.as_atom()) == Some("rename") {
            return list.get(1).and_then(|x| x.as_atom()).map(|s| s.to_string());
        }
    }
    None
}

// =============================================================================
// 4. Structural Gate-Level UNISIM Verilog (`write_verilog_netlist`, `read_verilog_netlist`)
// =============================================================================

/// Emit a [`Design`] as a structural gate-level UNISIM Verilog netlist.
pub fn write_verilog_netlist(design: &Design) -> String {
    let top_name = resolve(design.name);
    let mut port_decls = Vec::new();
    let mut port_set = HashSet::new();
    for p in &design.ports {
        let pname = resolve(p.name);
        port_set.insert(pname);
        let dir = match p.dir {
            PortDir::In => "input  wire",
            PortDir::Out => "output wire",
            PortDir::Inout => "inout  wire",
        };
        port_decls.push(format!("    {dir} {pname}"));
    }

    let mut out = format!(
        "// Generated by Helion EDA Netlist Interchange\nmodule {top_name} (\n{}\n);\n",
        port_decls.join(",\n")
    );

    for n in &design.nets {
        let nname = resolve(n.name);
        if !port_set.contains(nname) && !nname.starts_with("1'") {
            out.push_str(&format!("    wire {nname};\n"));
        }
    }

    let mut cell_conns: HashMap<Spur, BTreeMap<String, String>> = HashMap::new();
    for n in &design.nets {
        let nname = resolve(n.name).to_string();
        for ep in &n.endpoints {
            cell_conns
                .entry(ep.cell)
                .or_default()
                .insert(resolve(ep.pin).to_string(), nname.clone());
        }
    }

    for c in &design.cells {
        let cname = resolve(c.name);
        let ctype = cell_type_name(c);
        let param_str = match &c.kind {
            CellKind::Lut6 { init } => format!(" #(.INIT(64'h{init:016x}))"),
            CellKind::Dsp48e1 { opmode, alumode } => {
                format!(" #(.OPMODE({opmode}), .ALUMODE({alumode}))")
            }
            CellKind::Bram18e1 {
                read_width_a,
                read_width_b,
                write_width_a,
                write_width_b,
            } => format!(
                " #(.READ_WIDTH_A({read_width_a}), .READ_WIDTH_B({read_width_b}), .WRITE_WIDTH_A({write_width_a}), .WRITE_WIDTH_B({write_width_b}))"
            ),
            _ => String::new(),
        };
        let mut pins_out = Vec::new();
        if let Some(pmap) = cell_conns.get(&c.name) {
            for (pin, net) in pmap {
                pins_out.push(format!(".{pin}({net})"));
            }
        }
        out.push_str(&format!(
            "    {ctype}{param_str} {cname} ({});\n",
            pins_out.join(", ")
        ));
    }

    out.push_str("endmodule\n");
    out
}

/// Parse a structural gate-level UNISIM Verilog netlist into a [`Design`].
pub fn read_verilog_netlist(text: &str) -> Result<Design, String> {
    let mut cleaned = String::with_capacity(text.len());
    for line in text.lines() {
        let no_comment = line.split("//").next().unwrap_or("");
        cleaned.push_str(no_comment);
        cleaned.push('\n');
    }

    let mut d = Design::new("top");
    for raw_stmt in cleaned.split(';') {
        let stmt = raw_stmt.trim();
        if stmt.is_empty() || stmt == "endmodule" {
            continue;
        }
        let stmt = stmt
            .strip_prefix("endmodule")
            .map(|s| s.trim())
            .unwrap_or(stmt);
        if stmt.is_empty() {
            continue;
        }
        if let Some(rest) = stmt.strip_prefix("module ") {
            let rest = rest.trim();
            let (mname, port_part) = rest.split_once('(').unwrap_or((rest, ""));
            d.name = intern(mname.trim());
            let inner = port_part.trim().trim_end_matches(')');
            for item in inner.split(',') {
                let toks: Vec<&str> = item.split_whitespace().collect();
                if toks.is_empty() {
                    continue;
                }
                let pname = *toks.last().unwrap();
                let dir = match toks[0] {
                    "output" => PortDir::Out,
                    "inout" => PortDir::Inout,
                    _ => PortDir::In,
                };
                if !pname.is_empty() {
                    d.add_port(pname, dir);
                }
            }
            continue;
        }
        if stmt.starts_with("input ") || stmt.starts_with("output ") || stmt.starts_with("inout ") {
            let toks: Vec<&str> = stmt.split_whitespace().collect();
            let dir = match toks[0] {
                "output" => PortDir::Out,
                "inout" => PortDir::Inout,
                _ => PortDir::In,
            };
            if let Some(&pname) = toks.last() {
                if !d.ports.iter().any(|p| resolve(p.name) == pname) {
                    d.add_port(pname, dir);
                }
            }
            continue;
        }
        if stmt.starts_with("wire ") {
            continue;
        }

        // Cell instantiation:
        // `<TYPE> [#(.PARAM(VAL), ...)] <INST_NAME> (.PIN(NET), ...)`
        let mut idx = 0usize;
        let bytes = stmt.as_bytes();
        while idx < bytes.len() && !bytes[idx].is_ascii_whitespace() && bytes[idx] != b'#' {
            idx += 1;
        }
        let ctype = stmt[..idx].trim();
        let mut rest = stmt[idx..].trim();
        let mut params = BTreeMap::new();
        if let Some(after_hash) = rest.strip_prefix('#') {
            let after_hash = after_hash.trim();
            if after_hash.starts_with('(') {
                let (param_block, after_params) = slice_balanced_parens(after_hash)?;
                for (k, v) in parse_dot_pairs(&param_block) {
                    params.insert(k, v);
                }
                rest = after_params.trim();
            }
        }
        let Some((inst_name, conn_part)) = rest.split_once('(') else {
            continue;
        };
        let cname = inst_name.trim();
        let conn_inner = conn_part.trim().trim_end_matches(')');
        let (kind, attrs) = parse_cell_type_and_params(ctype, &params)?;
        d.push_cell(Cell {
            name: intern(cname),
            kind: kind.clone(),
            attrs,
        });
        for (pin, net) in parse_dot_pairs(conn_inner) {
            if !net.is_empty() {
                let ir_pin = normalize_pin_to_ir(ctype, &kind, &pin);
                d.connect(&net, cname, &ir_pin);
            }
        }
    }

    Ok(d)
}

fn slice_balanced_parens(s: &str) -> Result<(String, &str), String> {
    let mut depth = 0usize;
    for (i, ch) in s.char_indices() {
        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth -= 1;
            if depth == 0 {
                return Ok((s[1..i].to_string(), &s[i + 1..]));
            }
        }
    }
    Err(format!("unbalanced parentheses in '{s}'"))
}

fn parse_dot_pairs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '.' {
            i += 1;
            let mut key = String::new();
            while i < chars.len() && chars[i] != '(' && !chars[i].is_whitespace() {
                key.push(chars[i]);
                i += 1;
            }
            while i < chars.len() && chars[i] != '(' {
                i += 1;
            }
            if i < chars.len() && chars[i] == '(' {
                i += 1;
                let mut depth = 1usize;
                let mut val = String::new();
                while i < chars.len() && depth > 0 {
                    if chars[i] == '(' {
                        depth += 1;
                        val.push('(');
                    } else if chars[i] == ')' {
                        depth -= 1;
                        if depth > 0 {
                            val.push(')');
                        }
                    } else {
                        val.push(chars[i]);
                    }
                    i += 1;
                }
                out.push((key.trim().to_string(), val.trim().to_string()));
            }
        } else {
            i += 1;
        }
    }
    out
}

// =============================================================================
// Minimal Zero-Dependency JSON & S-Expression Parsers
// =============================================================================

fn json_esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[derive(Clone, Debug)]
enum JsonVal {
    Str(String),
    Num(usize),
    Arr(Vec<JsonVal>),
    Obj(BTreeMap<String, JsonVal>),
    Bool,
    Null,
}

impl JsonVal {
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }
    fn as_usize(&self) -> Option<usize> {
        match self {
            Self::Num(n) => Some(*n),
            _ => None,
        }
    }
    fn as_arr(&self) -> Option<&[JsonVal]> {
        match self {
            Self::Arr(a) => Some(a),
            _ => None,
        }
    }
    fn as_obj(&self) -> Option<&BTreeMap<String, JsonVal>> {
        match self {
            Self::Obj(o) => Some(o),
            _ => None,
        }
    }
}

struct JsonParser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> JsonParser<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            b: s.as_bytes(),
            i: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn parse_value(&mut self) -> Result<JsonVal, String> {
        self.skip_ws();
        let Some(&ch) = self.b.get(self.i) else {
            return Err("unexpected EOF in JSON".into());
        };
        match ch {
            b'"' => Ok(JsonVal::Str(self.parse_str()?)),
            b'{' => self.parse_obj(),
            b'[' => self.parse_arr(),
            b't' => {
                self.i += 4;
                Ok(JsonVal::Bool)
            }
            b'f' => {
                self.i += 5;
                Ok(JsonVal::Bool)
            }
            b'n' => {
                self.i += 4;
                Ok(JsonVal::Null)
            }
            c if c.is_ascii_digit() || c == b'-' => self.parse_num(),
            other => Err(format!("unexpected JSON byte '{}'", other as char)),
        }
    }

    fn parse_str(&mut self) -> Result<String, String> {
        self.skip_ws();
        if self.b.get(self.i) != Some(&b'"') {
            return Err("expected '\"' in JSON".into());
        }
        self.i += 1;
        let mut out = String::new();
        while self.i < self.b.len() {
            let c = self.b[self.i];
            self.i += 1;
            if c == b'"' {
                return Ok(out);
            }
            if c == b'\\' && self.i < self.b.len() {
                let esc = self.b[self.i];
                self.i += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    other => out.push(other as char),
                }
            } else {
                out.push(c as char);
            }
        }
        Err("unterminated JSON string".into())
    }

    fn parse_num(&mut self) -> Result<JsonVal, String> {
        let start = self.i;
        while self.i < self.b.len()
            && (self.b[self.i].is_ascii_digit() || self.b[self.i] == b'-' || self.b[self.i] == b'.')
        {
            self.i += 1;
        }
        let s = std::str::from_utf8(&self.b[start..self.i]).map_err(|e| e.to_string())?;
        let n = s.parse::<usize>().unwrap_or(0);
        Ok(JsonVal::Num(n))
    }

    fn parse_arr(&mut self) -> Result<JsonVal, String> {
        self.i += 1; // '['
        let mut arr = Vec::new();
        loop {
            self.skip_ws();
            if self.b.get(self.i) == Some(&b']') {
                self.i += 1;
                return Ok(JsonVal::Arr(arr));
            }
            arr.push(self.parse_value()?);
            self.skip_ws();
            if self.b.get(self.i) == Some(&b',') {
                self.i += 1;
            }
        }
    }

    fn parse_obj(&mut self) -> Result<JsonVal, String> {
        self.i += 1; // '{'
        let mut map = BTreeMap::new();
        loop {
            self.skip_ws();
            if self.b.get(self.i) == Some(&b'}') {
                self.i += 1;
                return Ok(JsonVal::Obj(map));
            }
            let key = self.parse_str()?;
            self.skip_ws();
            if self.b.get(self.i) != Some(&b':') {
                return Err("expected ':' in JSON object".into());
            }
            self.i += 1;
            let val = self.parse_value()?;
            map.insert(key, val);
            self.skip_ws();
            if self.b.get(self.i) == Some(&b',') {
                self.i += 1;
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Sexpr {
    Atom(String),
    List(Vec<Sexpr>),
}

impl Sexpr {
    fn as_atom(&self) -> Option<&str> {
        match self {
            Self::Atom(s) => Some(s),
            _ => None,
        }
    }
    fn as_list(&self) -> Option<&[Sexpr]> {
        match self {
            Self::List(l) => Some(l),
            _ => None,
        }
    }
}

fn parse_sexpr(text: &str) -> Result<Sexpr, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    parse_sexpr_node(&chars, &mut i)
}

fn parse_sexpr_node(chars: &[char], i: &mut usize) -> Result<Sexpr, String> {
    while *i < chars.len() && chars[*i].is_whitespace() {
        *i += 1;
    }
    if *i >= chars.len() {
        return Err("unexpected EOF in EDIF S-expression".into());
    }
    if chars[*i] == '(' {
        *i += 1;
        let mut items = Vec::new();
        loop {
            while *i < chars.len() && chars[*i].is_whitespace() {
                *i += 1;
            }
            if *i >= chars.len() {
                return Err("unclosed '(' in EDIF".into());
            }
            if chars[*i] == ')' {
                *i += 1;
                return Ok(Sexpr::List(items));
            }
            items.push(parse_sexpr_node(chars, i)?);
        }
    } else if chars[*i] == '"' {
        *i += 1;
        let mut s = String::new();
        while *i < chars.len() && chars[*i] != '"' {
            if chars[*i] == '\\' && *i + 1 < chars.len() {
                *i += 1;
            }
            s.push(chars[*i]);
            *i += 1;
        }
        if *i < chars.len() {
            *i += 1;
        }
        Ok(Sexpr::Atom(s))
    } else {
        let mut s = String::new();
        while *i < chars.len()
            && !chars[*i].is_whitespace()
            && chars[*i] != '('
            && chars[*i] != ')'
        {
            s.push(chars[*i]);
            *i += 1;
        }
        Ok(Sexpr::Atom(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yosys_json_roundtrip_counter_and_special_cells() {
        let mut d = Design::structural_counter();
        d.add_cell("u_carry", CellKind::Carry4);
        d.connect("q0", "u_carry", "DI0");
        d.connect("co0", "u_carry", "CO0");
        d.add_cell(
            "u_bram",
            CellKind::Bram18e1 {
                read_width_a: 18,
                read_width_b: 18,
                write_width_a: 18,
                write_width_b: 18,
            },
        );
        d.connect("clk", "u_bram", "CLKARDCLK");

        let json = write_yosys_json(&d);
        assert!(json.contains("\"type\": \"LUT6\""));
        assert!(json.contains("\"type\": \"FDRE\""));
        assert!(json.contains("\"type\": \"CARRY4\""));
        assert!(json.contains("\"type\": \"RAMB18E1\""));

        let back = read_yosys_json(&json).expect("read_yosys_json");
        assert_eq!(resolve(back.name), "counter");
        assert_eq!(back.ports.len(), d.ports.len());
        assert_eq!(back.cells.len(), d.cells.len());
        assert_eq!(back.lut_inits(), d.lut_inits());
    }

    #[test]
    fn eblif_roundtrip_preserves_lut6_truth_table_and_latches() {
        let d = Design::structural_counter();
        let eblif = write_eblif(&d);
        assert!(eblif.contains(".model counter"));
        assert!(eblif.contains(".inputs clk"));
        assert!(eblif.contains(".outputs led"));
        assert!(eblif.contains(".names"));
        assert!(eblif.contains(".latch"));
        assert!(eblif.contains(".subckt OBUF"));

        let back = read_eblif(&eblif).expect("read_eblif");
        assert_eq!(resolve(back.name), "counter");
        assert_eq!(back.ports.len(), d.ports.len());
        assert_eq!(back.cells.len(), d.cells.len());
        assert_eq!(back.lut_inits(), d.lut_inits());

        // Also test SOP truth table decoding without .attr LUT_INIT
        let pure_blif = r#"
.model xor_gate
.inputs a b
.outputs y
.names a b y
10 1
01 1
.end
"#;
        let xor_d = read_eblif(pure_blif).expect("read pure BLIF");
        let inits = xor_d.lut_inits();
        assert_eq!(inits.len(), 1);
        assert_eq!(inits[0], 0x6666_6666_6666_6666);
    }

    #[test]
    fn edif_2_0_0_roundtrip_preserves_unisims_and_connectivity() {
        let d = Design::structural_counter();
        let edif = write_edif(&d);
        assert!(edif.contains("(edifVersion 2 0 0)"));
        assert!(edif.contains("(library UNISIMS"));
        assert!(edif.contains("(cellRef LUT6 (libraryRef UNISIMS))"));
        assert!(edif.contains("(cellRef FDRE (libraryRef UNISIMS))"));

        let back = read_edif(&edif).expect("read_edif");
        assert_eq!(resolve(back.name), "counter");
        assert_eq!(back.ports.len(), d.ports.len());
        assert_eq!(back.cells.len(), d.cells.len());
        assert_eq!(back.lut_inits(), d.lut_inits());
        assert_eq!(
            back.net_on(intern("u_lut3"), intern("I3")),
            Some(intern("q3"))
        );
    }

    #[test]
    fn structural_verilog_netlist_roundtrip() {
        let d = Design::structural_counter();
        let v = write_verilog_netlist(&d);
        assert!(v.contains("module counter"));
        assert!(v.contains("LUT6 #(.INIT(64'h5555555555555555)) u_lut0"));
        assert!(v.contains("FDRE u_ff0"));
        assert!(v.contains("OBUF u_iob"));

        let back = read_verilog_netlist(&v).expect("read_verilog_netlist");
        assert_eq!(resolve(back.name), "counter");
        assert_eq!(back.ports.len(), d.ports.len());
        assert_eq!(back.cells.len(), d.cells.len());
        assert_eq!(back.lut_inits(), d.lut_inits());
    }

    #[test]
    fn cross_format_chain_and_muxf7_muxf8_support() {
        let yosys_mux = r#"{
  "creator": "Yosys",
  "modules": {
    "mux_top": {
      "ports": {
        "a": { "direction": "input", "bits": [ 2 ] },
        "b": { "direction": "input", "bits": [ 3 ] },
        "sel": { "direction": "input", "bits": [ 4 ] },
        "y": { "direction": "output", "bits": [ 5 ] }
      },
      "cells": {
        "u_mux7": {
          "type": "MUXF7",
          "parameters": {},
          "port_directions": { "I0": "input", "I1": "input", "S": "input", "O": "output" },
          "connections": { "I0": [ 2 ], "I1": [ 3 ], "S": [ 4 ], "O": [ 5 ] }
        }
      },
      "netnames": {
        "a": { "bits": [ 2 ] },
        "b": { "bits": [ 3 ] },
        "sel": { "bits": [ 4 ] },
        "y": { "bits": [ 5 ] }
      }
    }
  }
}"#;
        let d1 = read_yosys_json(yosys_mux).expect("parse MUXF7 JSON");
        assert_eq!(d1.cells.len(), 1);
        assert_eq!(d1.cells[0].attrs.get("UNISIM_ORIG"), Some("MUXF7"));

        // Full 4-format chain on blinky: Design -> Yosys JSON -> EBLIF -> EDIF -> Structural Verilog -> Design
        let gold = Design::structural_blinky();
        let j = write_yosys_json(&gold);
        let from_j = read_yosys_json(&j).unwrap();
        let b = write_eblif(&from_j);
        let from_b = read_eblif(&b).unwrap();
        let e = write_edif(&from_b);
        let from_e = read_edif(&e).unwrap();
        let v = write_verilog_netlist(&from_e);
        let final_d = read_verilog_netlist(&v).unwrap();
        assert_eq!(resolve(final_d.name), "blinky");
        assert_eq!(final_d.cells.len(), gold.cells.len());
        assert_eq!(final_d.lut_inits(), gold.lut_inits());
    }
}
