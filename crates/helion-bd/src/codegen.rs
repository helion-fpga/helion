//! Synthesizable SystemVerilog Top-Level RTL, Crossbar Fabric, and Wrapper Generator (`generate_top_sv` / `make_wrapper`).

use crate::BlockDesign;
use crate::address_drc::validate_bd_design;
use crate::graph::{BdCell, BdGraph, get_explicit_bd_graph};
use helion_ipxact::{
    BusInterfaceType, InterfaceMode, PortDirection, ResetPolarity, generate_axi_interconnect_sv,
    parse_int_or_hex,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Generate clean, human-readable, 100% synthesizable SystemVerilog for the entire `BlockDesign`,
/// including all instantiated IP modules, the parameterized AXI address decoder / crossbar fabric,
/// and the block design structural assembly module.
pub fn generate_top_sv(bd: &BlockDesign) -> Result<String, String> {
    let report = validate_bd_design(bd);
    if !report.ok {
        return Err(report.errors.join("; "));
    }

    let Some(graph) = get_explicit_bd_graph(bd) else {
        // Legacy Helion-MM/ST BlockDesign fallback
        let mut s = format!(
            "module {} (\n  input logic clk,\n  input logic resetn\n);\n",
            bd.name
        );
        for c in &bd.cores {
            s.push_str(&format!(
                "  {} #() u_{} (.clk(clk), .resetn(resetn));\n",
                c.name, c.name
            ));
        }
        s.push_str("endmodule\n");
        return Ok(s);
    };

    emit_bd_systemverilog(&graph, false)
}

/// Vivado `make_wrapper -files [get_files <bd>.bd] -top`:
/// Emits the complete synthesizable SystemVerilog including all IP modules, the `<bd.name>` block design
/// module, and the top-level `<bd.name>_wrapper` module with tristate I/O buffer logic.
pub fn make_wrapper(bd: &BlockDesign) -> Result<String, String> {
    let report = validate_bd_design(bd);
    if !report.ok {
        return Err(report.errors.join("; "));
    }
    let Some(graph) = get_explicit_bd_graph(bd) else {
        let base = generate_top_sv(bd)?;
        let wrapper = format!(
            "\nmodule {}_wrapper (\n  input logic clk,\n  input logic resetn\n);\n  {} {}_i (\n    .clk(clk),\n    .resetn(resetn)\n  );\nendmodule\n",
            bd.name, bd.name, bd.name
        );
        return Ok(format!("{base}{wrapper}"));
    };

    emit_bd_systemverilog(&graph, true)
}

fn emit_bd_systemverilog(graph: &BdGraph, include_wrapper: bool) -> Result<String, String> {
    let mut sv = String::new();
    sv.push_str(&format!(
        "// ============================================================================\n// Helion Vivado IP Integrator (UG994) Synthesizable Block Design: {}\n// ============================================================================\n\n",
        graph.name
    ));

    // 1. Emit synthesizable RTL definitions for all instantiated IP components
    let mut emitted_modules = BTreeSet::new();
    for cell in &graph.cells {
        let mod_name = &cell.component.core.name;
        if mod_name == "axi_interconnect" || mod_name == "axi_crossbar" {
            // Emit a dedicated parameterized interconnect module per interconnect instance
            let ic_mod_name = format!("{}_{}", graph.name, cell.instance_name);
            if emitted_modules.insert(ic_mod_name.clone()) {
                let m = cell
                    .component
                    .get_param_i64(&cell.properties, "C_NUM_MASTERS")
                    .max(1) as u32;
                let s = cell
                    .component
                    .get_param_i64(&cell.properties, "C_NUM_SLAVES")
                    .max(1) as u32;
                let regions = resolve_interconnect_regions(graph, cell, s);
                sv.push_str(&generate_axi_interconnect_sv(
                    &ic_mod_name,
                    m,
                    s,
                    32,
                    32,
                    &regions,
                ));
                sv.push('\n');
            }
        } else if emitted_modules.insert(mod_name.clone()) {
            let ip_sv = cell.component.generate_sv(&cell.properties)?;
            sv.push_str(&ip_sv);
            sv.push('\n');
        }
    }

    // 2. Collect top-level ports for `module <graph.name>`
    let top_ports = collect_top_ports(graph);

    sv.push_str(&format!("module {} (\n", graph.name));
    for (idx, (dir, width, name)) in top_ports.iter().enumerate() {
        let sep = if idx + 1 == top_ports.len() { "" } else { "," };
        if *width > 1 {
            sv.push_str(&format!(
                "  {} logic [{}:0] {}{}\n",
                dir.sv_keyword(),
                width - 1,
                name,
                sep
            ));
        } else {
            sv.push_str(&format!("  {} logic {}{}\n", dir.sv_keyword(), name, sep));
        }
    }
    sv.push_str(");\n\n");

    // 3. Emit address map summary comments
    if !graph.address_segments.is_empty() {
        sv.push_str("  // Address Map Summary:\n");
        for seg in &graph.address_segments {
            sv.push_str(&format!(
                "  //   {}/{} -> {}/{}: Base=0x{:08X}, High=0x{:08X}, Range=0x{:X}\n",
                seg.master_cell,
                seg.master_intf,
                seg.slave_cell,
                seg.slave_intf,
                seg.base_address,
                seg.high_address(),
                seg.range
            ));
        }
        sv.push('\n');
    }

    // 4. Build pin-to-wire mapping and declare internal nets
    let mut pin_to_wire: BTreeMap<String, String> = BTreeMap::new();
    let mut internal_wires: Vec<(String, u32)> = Vec::new();
    let mut declared_wires: BTreeSet<String> = top_ports.iter().map(|(_, _, n)| n.clone()).collect();

    // Scalar nets
    for net in &graph.nets {
        // If net touches a top-level port, use that port's name as the wire name
        let top_port_ep = net.endpoints.iter().find(|ep| !ep.contains('/')).cloned();
        let wire_name = top_port_ep.unwrap_or_else(|| net.name.clone());
        if declared_wires.insert(wire_name.clone()) {
            let w = infer_net_width(graph, &net.endpoints);
            internal_wires.push((wire_name.clone(), w));
        }
        for ep in &net.endpoints {
            if ep.contains('/') {
                pin_to_wire.insert(ep.to_ascii_lowercase(), wire_name.clone());
            }
        }
    }

    // Interface nets
    for inet in &graph.intf_nets {
        if inet.endpoints.len() < 2 {
            continue;
        }
        // Check if one of the endpoints is an external BdIntfPort
        let ext_port_ep = inet.endpoints.iter().find(|ep| !ep.contains('/')).cloned();
        let cell_eps: Vec<&String> = inet.endpoints.iter().filter(|ep| ep.contains('/')).collect();

        if let (Some(ext_name), Some(cell_ep)) = (ext_port_ep, cell_eps.first()) {
            if let Some((c_inst, i_name)) = cell_ep.split_once('/') {
                if let Some(cell) = graph.find_cell(c_inst) {
                    if let Some(intf) = cell.get_interface(i_name) {
                        for pm in &intf.port_maps {
                            let top_sig = format!("{}_{}", ext_name, pm.logical_name.to_ascii_lowercase());
                            let key = format!("{c_inst}/{}", pm.physical_name).to_ascii_lowercase();
                            pin_to_wire.insert(key, top_sig);
                        }
                    }
                }
            }
        } else if cell_eps.len() >= 2 {
            // Internal interface net between two cells
            let mut logical_widths: BTreeMap<String, u32> = BTreeMap::new();
            for cep in &cell_eps {
                if let Some((c_inst, i_name)) = cep.split_once('/') {
                    if let Some(cell) = graph.find_cell(c_inst) {
                        if let Some(intf) = cell.get_interface(i_name) {
                            for pm in &intf.port_maps {
                                let w = resolve_port_width(cell, &pm.physical_name, pm.width);
                                let entry = logical_widths.entry(pm.logical_name.clone()).or_insert(w);
                                *entry = (*entry).max(w);
                            }
                        }
                    }
                }
            }
            for (logical, width) in &logical_widths {
                let wname = format!("{}_{}", inet.name, logical.to_ascii_lowercase());
                if declared_wires.insert(wname.clone()) {
                    internal_wires.push((wname.clone(), *width));
                }
                for cep in &cell_eps {
                    if let Some((c_inst, i_name)) = cep.split_once('/') {
                        if let Some(cell) = graph.find_cell(c_inst) {
                            if let Some(intf) = cell.get_interface(i_name) {
                                if let Some(pm) = intf
                                    .port_maps
                                    .iter()
                                    .find(|p| p.logical_name.eq_ignore_ascii_case(logical))
                                {
                                    let key =
                                        format!("{c_inst}/{}", pm.physical_name).to_ascii_lowercase();
                                    pin_to_wire.insert(key, wname.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if !internal_wires.is_empty() {
        for (wname, width) in &internal_wires {
            if *width > 1 {
                sv.push_str(&format!("  logic [{}:0] {wname};\n", width - 1));
            } else {
                sv.push_str(&format!("  logic {wname};\n"));
            }
        }
        sv.push('\n');
    }

    // 5. Instantiate every cell in `graph.cells`
    let default_clk = graph
        .ports
        .iter()
        .find(|p| p.port_type == Some(BusInterfaceType::Clock))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "aclk".into());
    let default_rstn = graph
        .ports
        .iter()
        .find(|p| p.port_type == Some(BusInterfaceType::Reset))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "aresetn".into());

    for cell in &graph.cells {
        let mod_name = if cell.component.core.name == "axi_interconnect"
            || cell.component.core.name == "axi_crossbar"
        {
            format!("{}_{}", graph.name, cell.instance_name)
        } else {
            cell.component.core.name.clone()
        };

        // Collect parameter overrides that match formal parameters on the component
        let mut param_assigns = Vec::new();
        if cell.component.core.name != "axi_interconnect"
            && cell.component.core.name != "axi_crossbar"
        {
            for p in &cell.component.parameters {
                if let Some(val) = cell
                    .properties
                    .get(&p.name)
                    .or_else(|| cell.properties.get(&format!("CONFIG.{}", p.name)))
                {
                    if let Some(parsed) = parse_int_or_hex(val) {
                        param_assigns.push(format!(".{}({})", p.name, parsed));
                    } else {
                        param_assigns.push(format!(".{}({})", p.name, val));
                    }
                }
            }
        }

        if param_assigns.is_empty() {
            sv.push_str(&format!("  {} {} (\n", mod_name, cell.instance_name));
        } else {
            sv.push_str(&format!(
                "  {} #(\n    {}\n  ) {} (\n",
                mod_name,
                param_assigns.join(",\n    "),
                cell.instance_name
            ));
        }

        for (p_idx, port) in cell.component.ports.iter().enumerate() {
            let sep = if p_idx + 1 == cell.component.ports.len() {
                ""
            } else {
                ","
            };
            let lookup_key = format!("{}/{}", cell.instance_name, port.name).to_ascii_lowercase();
            let expr = if let Some(w) = pin_to_wire.get(&lookup_key) {
                w.clone()
            } else if port.direction == PortDirection::In {
                let lower = port.name.to_ascii_lowercase();
                if lower.contains("clk") || lower == "clka" {
                    cell.clock_domain
                        .clone()
                        .unwrap_or_else(|| default_clk.clone())
                } else if lower.contains("aresetn") || lower.contains("resetn") || lower == "presetn"
                {
                    default_rstn.clone()
                } else if lower == "rsta" || lower == "wb_rst_i" {
                    format!("~{default_rstn}")
                } else {
                    port.default_value.clone().unwrap_or_else(|| "'0".into())
                }
            } else {
                String::new()
            };
            sv.push_str(&format!("    .{}({}){}\n", port.name, expr, sep));
        }
        sv.push_str("  );\n\n");
    }

    sv.push_str("endmodule\n");

    // 6. Optional top-level `<graph.name>_wrapper` module (`make_wrapper`)
    if include_wrapper {
        sv.push_str(&format!(
            "\n// Top-Level Synthesizable Wrapper for {}\nmodule {}_wrapper (\n",
            graph.name, graph.name
        ));
        for (idx, (dir, width, name)) in top_ports.iter().enumerate() {
            let sep = if idx + 1 == top_ports.len() { "" } else { "," };
            if *width > 1 {
                sv.push_str(&format!(
                    "  {} logic [{}:0] {}{}\n",
                    dir.sv_keyword(),
                    width - 1,
                    name,
                    sep
                ));
            } else {
                sv.push_str(&format!("  {} logic {}{}\n", dir.sv_keyword(), name, sep));
            }
        }
        sv.push_str(");\n");
        sv.push_str(&format!("  {} {}_i (\n", graph.name, graph.name));
        for (idx, (_, _, name)) in top_ports.iter().enumerate() {
            let sep = if idx + 1 == top_ports.len() { "" } else { "," };
            sv.push_str(&format!("    .{name}({name}){sep}\n"));
        }
        sv.push_str("  );\nendmodule\n");
    }

    Ok(sv)
}

fn resolve_interconnect_regions(graph: &BdGraph, ic_cell: &BdCell, num_slaves: u32) -> Vec<(u64, u64)> {
    let mut regions = Vec::new();
    for idx in 0..num_slaves {
        let base_key = format!("C_M{idx:02}_BASE_ADDR");
        let width_key = format!("C_M{idx:02}_ADDR_WIDTH");
        if let Some(base_str) = ic_cell.properties.get(&base_key) {
            let base = parse_int_or_hex(base_str).unwrap_or(0x4000_0000) as u64;
            let aw = ic_cell
                .properties
                .get(&width_key)
                .and_then(|w| parse_int_or_hex(w))
                .unwrap_or(16)
                .clamp(4, 32) as u32;
            regions.push((base, 1u64 << aw));
            continue;
        }
        // Otherwise check which slave is connected to M{idx:02}_AXI and look up its BdAddressSegment
        let m_ep = format!("{}/M{idx:02}_AXI", ic_cell.instance_name);
        let mut found = None;
        for inet in &graph.intf_nets {
            if inet.endpoints.iter().any(|e| e.eq_ignore_ascii_case(&m_ep)) {
                for peer in &inet.endpoints {
                    if !peer.eq_ignore_ascii_case(&m_ep) {
                        if let Some((s_cell, s_intf)) = peer.split_once('/') {
                            if let Some(seg) = graph.address_segments.iter().find(|seg| {
                                seg.slave_cell.eq_ignore_ascii_case(s_cell)
                                    && seg.slave_intf.eq_ignore_ascii_case(s_intf)
                            }) {
                                found = Some((seg.base_address, seg.range));
                            }
                        }
                    }
                }
            }
        }
        regions.push(
            found.unwrap_or((0x4000_0000 + (idx as u64) * 0x0001_0000, 0x0001_0000)),
        );
    }
    regions
}

fn collect_top_ports(graph: &BdGraph) -> Vec<(PortDirection, u32, String)> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for p in &graph.ports {
        if seen.insert(p.name.clone()) {
            out.push((p.direction, p.width.max(1), p.name.clone()));
        }
    }
    // Also expand external BdIntfPort signals
    for ip in &graph.intf_ports {
        for inet in &graph.intf_nets {
            if inet.endpoints.iter().any(|e| e.eq_ignore_ascii_case(&ip.name)) {
                if let Some(cell_ep) = inet.endpoints.iter().find(|e| e.contains('/')) {
                    if let Some((c_inst, i_name)) = cell_ep.split_once('/') {
                        if let Some(cell) = graph.find_cell(c_inst) {
                            if let Some(intf) = cell.get_interface(i_name) {
                                for pm in &intf.port_maps {
                                    let sig_name = format!(
                                        "{}_{}",
                                        ip.name,
                                        pm.logical_name.to_ascii_lowercase()
                                    );
                                    if seen.insert(sig_name.clone()) {
                                        let w = resolve_port_width(cell, &pm.physical_name, pm.width);
                                        out.push((pm.direction, w, sig_name));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if out.is_empty() {
        out.push((PortDirection::In, 1, "aclk".into()));
        out.push((PortDirection::In, 1, "aresetn".into()));
    }
    let _ = ResetPolarity::ActiveLow;
    let _ = InterfaceMode::Master;
    out
}

fn resolve_port_width(cell: &BdCell, physical_name: &str, default_width: u32) -> u32 {
    if cell.component.core.name == "axi_gpio" && physical_name.starts_with("gpio_io_") {
        return cell
            .component
            .get_param_i64(&cell.properties, "C_GPIO_WIDTH")
            .max(1) as u32;
    }
    if (cell.component.core.name == "cdc_fifo" || cell.component.core.name == "axis_data_fifo")
        && physical_name.ends_with("_tdata")
    {
        return cell
            .component
            .get_param_i64(&cell.properties, "C_TDATA_WIDTH")
            .max(8) as u32;
    }
    default_width.max(1)
}

fn infer_net_width(graph: &BdGraph, endpoints: &[String]) -> u32 {
    let mut w = 1u32;
    for ep in endpoints {
        if let Some((c_inst, p_name)) = ep.split_once('/') {
            if let Some(cell) = graph.find_cell(c_inst) {
                if let Some(port) = cell
                    .component
                    .ports
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case(p_name))
                {
                    w = w.max(resolve_port_width(cell, &port.name, port.width));
                }
            }
        }
    }
    w
}

/// Save the block design description and generated wrapper to disk (`save_bd_design`).
pub fn save_bd_design(bd: &BlockDesign, path: &Path) -> Result<PathBuf, String> {
    let target = if path.extension().is_some() {
        path.to_path_buf()
    } else {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        path.join(format!("{}.bd", bd.name))
    };
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }
    let graph = get_explicit_bd_graph(bd).unwrap_or_default();
    let mut manifest = String::new();
    manifest.push_str(&format!("# Helion Block Design (.bd)\ndesign_name {}\n", bd.name));
    for cell in &graph.cells {
        manifest.push_str(&format!("cell {} vlnv={}\n", cell.instance_name, cell.vlnv));
        for (k, v) in &cell.properties {
            manifest.push_str(&format!("  param {k}={v}\n"));
        }
    }
    for inet in &graph.intf_nets {
        manifest.push_str(&format!(
            "intf_net {} type={} endpoints={}\n",
            inet.name,
            inet.bus_type,
            inet.endpoints.join(",")
        ));
    }
    for net in &graph.nets {
        manifest.push_str(&format!(
            "net {} endpoints={}\n",
            net.name,
            net.endpoints.join(",")
        ));
    }
    for seg in &graph.address_segments {
        manifest.push_str(&format!(
            "addr_seg {} master={}/{} slave={}/{} offset=0x{:08X} range=0x{:X}\n",
            seg.name,
            seg.master_cell,
            seg.master_intf,
            seg.slave_cell,
            seg.slave_intf,
            seg.base_address,
            seg.range
        ));
    }
    std::fs::write(&target, manifest).map_err(|e| e.to_string())?;
    Ok(target)
}
