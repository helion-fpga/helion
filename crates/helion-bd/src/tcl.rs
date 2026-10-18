//! Vivado UG994 Block Design TCL Command Bridge (`exec_bd_tcl`).

use crate::BlockDesign;
use crate::address_drc::{assign_bd_address, assign_bd_address_explicit, validate_bd_design};
use crate::codegen::{generate_top_sv, make_wrapper, save_bd_design};
use crate::graph::{
    BdAutomationRule, connect_bd_intf_net, connect_bd_net, create_bd_cell, create_bd_design,
    create_bd_intf_port, create_bd_port, norm_ep, set_bd_cell_property,
};
use helion_ipxact::{BusInterfaceType, InterfaceMode, PortDirection, parse_int_or_hex};
use std::path::Path;

/// Execute a single Vivado Block Design TCL command or a multi-line TCL script against `bd`.
/// Supports:
/// - `create_bd_design <name>`
/// - `create_bd_cell -type ip -vlnv <vlnv_or_name> <instance_name>`
/// - `create_bd_port -dir <I|O|IO> [-type <clk|rst|intr>] [-from <msb> -to <lsb>] <name>`
/// - `create_bd_intf_port -mode <Master|Slave> -vlnv <bus_type> <name>`
/// - `set_property -dict [list CONFIG.<k> <v> ...] [get_bd_cells <inst>]` / `set_property CONFIG.<k> <v> [get_bd_cells <inst>]`
/// - `connect_bd_intf_net [-intf_net <name>] [get_bd_intf_pins <a/b>] [get_bd_intf_pins <c/d>]`
/// - `connect_bd_net [-net <name>] [get_bd_pins <a/b>] [get_bd_ports <p>] ...`
/// - `apply_bd_automation -rule xilinx.com:bd_rule:<axi4|clkrst|board|bram_cntlr> -config { ... } [get_bd_intf_pins <cell/intf>]`
/// - `assign_bd_address [-offset <hex> -range <size|hex>] [get_bd_addr_segs <cell/intf...>]`
/// - `validate_bd_design`
/// - `make_wrapper` / `generate_top_sv`
/// - `save_bd_design [path]`
pub fn exec_bd_tcl(bd: &mut BlockDesign, script: &str) -> Result<String, String> {
    let mut last_out = String::new();
    for raw_line in coalesce_tcl_lines(script) {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        last_out = exec_single_tcl_cmd(bd, line)?;
    }
    Ok(last_out)
}

fn coalesce_tcl_lines(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut brace_depth: i32 = 0;
    let mut bracket_depth: i32 = 0;

    for raw in script.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let continues = trimmed.ends_with('\\');
        let piece = if continues {
            trimmed.trim_end_matches('\\').trim()
        } else {
            trimmed
        };
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(piece);
        for ch in piece.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => brace_depth = (brace_depth - 1).max(0),
                '[' => bracket_depth += 1,
                ']' => bracket_depth = (bracket_depth - 1).max(0),
                _ => {}
            }
        }
        if !continues && brace_depth == 0 && bracket_depth == 0 {
            for stmt in split_semicolons(&current) {
                if !stmt.is_empty() {
                    out.push(stmt);
                }
            }
            current.clear();
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

fn split_semicolons(s: &str) -> Vec<String> {
    let mut res = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for ch in s.chars() {
        match ch {
            '{' | '[' | '"' => {
                depth += 1;
                cur.push(ch);
            }
            '}' | ']' => {
                depth = (depth - 1).max(0);
                cur.push(ch);
            }
            ';' if depth == 0 => {
                res.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        res.push(cur.trim().to_string());
    }
    res
}

fn tokenize_tcl(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            if !cur.is_empty() {
                tokens.push(unwrap_tcl_token(&cur));
                cur.clear();
            }
        } else if ch == '{' {
            if !cur.is_empty() {
                tokens.push(unwrap_tcl_token(&cur));
                cur.clear();
            }
            let mut depth = 1;
            let mut inner = String::new();
            for c in chars.by_ref() {
                if c == '{' {
                    depth += 1;
                    inner.push(c);
                } else if c == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    inner.push(c);
                } else {
                    inner.push(c);
                }
            }
            tokens.push(inner.trim().to_string());
        } else if ch == '[' {
            if !cur.is_empty() {
                tokens.push(unwrap_tcl_token(&cur));
                cur.clear();
            }
            let mut depth = 1;
            let mut inner = String::new();
            for c in chars.by_ref() {
                if c == '[' {
                    depth += 1;
                    inner.push(c);
                } else if c == ']' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    inner.push(c);
                } else {
                    inner.push(c);
                }
            }
            tokens.push(eval_bracket_expr(inner.trim()));
        } else if ch == '"' {
            let mut inner = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                inner.push(c);
            }
            tokens.push(inner);
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        tokens.push(unwrap_tcl_token(&cur));
    }
    tokens
}

fn unwrap_tcl_token(tok: &str) -> String {
    let t = tok.trim();
    if let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        eval_bracket_expr(inner)
    } else {
        t.trim_matches('"').to_string()
    }
}

fn eval_bracket_expr(expr: &str) -> String {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.is_empty() {
        return String::new();
    }
    match parts[0] {
        "get_bd_cells" | "get_bd_intf_pins" | "get_bd_pins" | "get_bd_ports"
        | "get_bd_intf_ports" | "get_bd_addr_segs" | "get_files" => parts
            .last()
            .copied()
            .unwrap_or("")
            .trim_matches(['"', '{', '}'])
            .trim_start_matches('/')
            .to_string(),
        "list" => parts[1..].join(" "),
        _ => expr.to_string(),
    }
}

fn exec_single_tcl_cmd(bd: &mut BlockDesign, line: &str) -> Result<String, String> {
    let tokens = tokenize_tcl(line);
    if tokens.is_empty() {
        return Ok(String::new());
    }
    let cmd = tokens[0].as_str();
    let args = &tokens[1..];

    match cmd {
        "create_bd_design" => {
            let name = args
                .iter()
                .find(|a| !a.starts_with('-'))
                .ok_or_else(|| "create_bd_design: missing design name".to_string())?;
            *bd = create_bd_design(name);
            Ok(name.clone())
        }
        "create_bd_cell" => {
            let mut vlnv = None;
            let mut inst = None;
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "-type" => i += 2,
                    "-vlnv" => {
                        vlnv = args.get(i + 1).cloned();
                        i += 2;
                    }
                    other if !other.starts_with('-') => {
                        if vlnv.is_none() && args.len() >= 2 && inst.is_none() {
                            vlnv = Some(other.to_string());
                        } else {
                            inst = Some(other.to_string());
                        }
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            let vlnv = vlnv.ok_or_else(|| "create_bd_cell: missing -vlnv".to_string())?;
            let inst = inst.ok_or_else(|| "create_bd_cell: missing instance name".to_string())?;
            create_bd_cell(bd, &vlnv, &inst)
        }
        "create_bd_port" => {
            let mut dir = PortDirection::In;
            let mut port_type = None;
            let mut from_bit: Option<u32> = None;
            let mut to_bit: Option<u32> = None;
            let mut name = None;
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "-dir" => {
                        if let Some(d) = args.get(i + 1) {
                            dir = match d.to_ascii_uppercase().as_str() {
                                "O" | "OUT" | "OUTPUT" => PortDirection::Out,
                                "IO" | "INOUT" => PortDirection::InOut,
                                _ => PortDirection::In,
                            };
                        }
                        i += 2;
                    }
                    "-type" => {
                        if let Some(t) = args.get(i + 1) {
                            port_type = BusInterfaceType::parse(t);
                        }
                        i += 2;
                    }
                    "-from" => {
                        from_bit = args.get(i + 1).and_then(|s| s.parse().ok());
                        i += 2;
                    }
                    "-to" => {
                        to_bit = args.get(i + 1).and_then(|s| s.parse().ok());
                        i += 2;
                    }
                    other if !other.starts_with('-') => {
                        name = Some(other.to_string());
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            let width = match (from_bit, to_bit) {
                (Some(hi), Some(lo)) => hi.abs_diff(lo) + 1,
                _ => 1,
            };
            let name = name.ok_or_else(|| "create_bd_port: missing port name".to_string())?;
            create_bd_port(bd, &name, dir, width, port_type)
        }
        "create_bd_intf_port" => {
            let mut mode = InterfaceMode::Master;
            let mut bus_type = BusInterfaceType::Gpio;
            let mut name = None;
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "-mode" => {
                        if let Some(m) = args.get(i + 1) {
                            mode = if m.eq_ignore_ascii_case("slave") {
                                InterfaceMode::Slave
                            } else {
                                InterfaceMode::Master
                            };
                        }
                        i += 2;
                    }
                    "-vlnv" | "-type" => {
                        if let Some(v) = args.get(i + 1) {
                            let part = v.split(':').nth(2).unwrap_or(v.as_str()).trim_end_matches("_rtl");
                            bus_type = BusInterfaceType::parse(part)
                                .or_else(|| BusInterfaceType::parse(v))
                                .unwrap_or(BusInterfaceType::Gpio);
                        }
                        i += 2;
                    }
                    other if !other.starts_with('-') => {
                        name = Some(other.to_string());
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            let name = name.ok_or_else(|| "create_bd_intf_port: missing name".to_string())?;
            create_bd_intf_port(bd, &name, bus_type, mode)
        }
        "set_property" => {
            if args.first().map(|s| s.as_str()) == Some("-dict") {
                let dict_str = args
                    .get(1)
                    .ok_or_else(|| "set_property: missing dict".to_string())?;
                let target = args
                    .last()
                    .ok_or_else(|| "set_property: missing target".to_string())?;
                let kv_tokens = tokenize_tcl(dict_str);
                let mut idx = 0;
                if kv_tokens.first().map(|s| s.as_str()) == Some("list") {
                    idx = 1;
                }
                while idx + 1 < kv_tokens.len() {
                    set_bd_cell_property(bd, target, &kv_tokens[idx], &kv_tokens[idx + 1])?;
                    idx += 2;
                }
                Ok(target.clone())
            } else if args.len() >= 3 {
                let key = &args[0];
                let val = &args[1];
                let target = &args[2];
                set_bd_cell_property(bd, target, key, val)?;
                Ok(target.clone())
            } else {
                Err("set_property: expected 'set_property <key> <val> <target>'".into())
            }
        }
        "connect_bd_intf_net" => {
            let mut net_name = None;
            let mut eps = Vec::new();
            let mut i = 0;
            while i < args.len() {
                if args[i] == "-intf_net" {
                    net_name = args.get(i + 1).cloned();
                    i += 2;
                } else if !args[i].starts_with('-') {
                    eps.push(args[i].as_str());
                    i += 1;
                } else {
                    i += 1;
                }
            }
            if eps.len() < 2 {
                return Err("connect_bd_intf_net: requires 2 interface endpoints".into());
            }
            connect_bd_intf_net(bd, net_name.as_deref(), eps[0], eps[1])
        }
        "connect_bd_net" => {
            let mut net_name = None;
            let mut eps = Vec::new();
            let mut i = 0;
            while i < args.len() {
                if args[i] == "-net" {
                    net_name = args.get(i + 1).cloned();
                    i += 2;
                } else if !args[i].starts_with('-') {
                    eps.push(args[i].as_str());
                    i += 1;
                } else {
                    i += 1;
                }
            }
            connect_bd_net(bd, net_name.as_deref(), &eps)
        }
        "apply_bd_automation" => {
            let mut rule_str = String::new();
            let mut config_str = String::new();
            let mut target = String::new();
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "-rule" => {
                        rule_str = args.get(i + 1).cloned().unwrap_or_default();
                        i += 2;
                    }
                    "-config" => {
                        config_str = args.get(i + 1).cloned().unwrap_or_default();
                        i += 2;
                    }
                    other if !other.starts_with('-') => {
                        target = norm_ep(other);
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            let cfg_tokens = tokenize_tcl(&config_str);
            let get_cfg = |k: &str| -> Option<String> {
                let mut j = 0;
                while j + 1 < cfg_tokens.len() {
                    if cfg_tokens[j].eq_ignore_ascii_case(k) {
                        return Some(cfg_tokens[j + 1].clone());
                    }
                    j += 2;
                }
                None
            };
            let (t_cell, t_intf) = target
                .split_once('/')
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .unwrap_or_else(|| (target.clone(), "S_AXI".into()));

            let rule = if rule_str.contains("clkrst") {
                BdAutomationRule::ClkRst {
                    clk_source: get_cfg("Clk").unwrap_or_else(|| "aclk".into()),
                    rst_source: get_cfg("Rst").unwrap_or_else(|| "aresetn".into()),
                    clock_domain: get_cfg("Clk"),
                }
            } else if rule_str.contains("board") {
                BdAutomationRule::Board {
                    target_cell: t_cell,
                    target_intf: t_intf,
                    external_port_name: get_cfg("Board_Interface"),
                }
            } else if rule_str.contains("bram_cntlr") {
                BdAutomationRule::BramCntlr {
                    target_cell: t_cell,
                    target_intf: t_intf,
                }
            } else {
                BdAutomationRule::Axi4 {
                    target_cell: t_cell,
                    target_intf: t_intf,
                    master: get_cfg("Master"),
                    clk_source: get_cfg("Clk_master").or_else(|| get_cfg("Clk")),
                    rst_source: get_cfg("Rst"),
                }
            };
            crate::graph::apply_bd_automation(bd, rule)
        }
        "assign_bd_address" => {
            let mut offset: Option<u64> = None;
            let mut range: Option<u64> = None;
            let mut target_seg: Option<String> = None;
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "-offset" => {
                        offset = args
                            .get(i + 1)
                            .and_then(|s| parse_int_or_hex(s))
                            .map(|v| v as u64);
                        i += 2;
                    }
                    "-range" => {
                        if let Some(r_str) = args.get(i + 1) {
                            range = parse_range_size(r_str);
                        }
                        i += 2;
                    }
                    "-target_address_space" => i += 2,
                    other if !other.starts_with('-') => {
                        target_seg = Some(norm_ep(other));
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            if let (Some(off), Some(rng), Some(tgt)) = (offset, range, target_seg) {
                // Normalize `cell/intf/Reg` -> `cell/intf`
                let parts: Vec<&str> = tgt.split('/').collect();
                let ep = if parts.len() >= 2 {
                    format!("{}/{}", parts[0], parts[1])
                } else {
                    format!("{tgt}/S_AXI")
                };
                let seg = assign_bd_address_explicit(bd, &ep, off, rng)?;
                Ok(format!(
                    "{}: 0x{:08X}..0x{:08X}",
                    seg.name,
                    seg.base_address,
                    seg.high_address()
                ))
            } else {
                let segs = assign_bd_address(bd)?;
                Ok(format!("assigned {} address segment(s)", segs.len()))
            }
        }
        "validate_bd_design" => {
            let rep = validate_bd_design(bd);
            if rep.ok {
                Ok("validate_bd_design: PASS (0 errors)".into())
            } else {
                Err(format!("validate_bd_design: FAIL: {}", rep.errors.join("; ")))
            }
        }
        "make_wrapper" => make_wrapper(bd),
        "generate_top_sv" => generate_top_sv(bd),
        "save_bd_design" => {
            let path_str = args
                .iter()
                .find(|a| !a.starts_with('-'))
                .cloned()
                .unwrap_or_else(|| {
                    std::env::temp_dir()
                        .join(format!("{}.bd", bd.name))
                        .display()
                        .to_string()
                });
            let p = save_bd_design(bd, Path::new(&path_str))?;
            Ok(p.display().to_string())
        }
        other => Err(format!("exec_bd_tcl: unsupported command '{other}'")),
    }
}

fn parse_range_size(s: &str) -> Option<u64> {
    let u = s.trim().to_ascii_uppercase();
    if let Some(k) = u.strip_suffix('K') {
        k.parse::<u64>().ok().map(|v| v * 1024)
    } else if let Some(m) = u.strip_suffix('M') {
        m.parse::<u64>().ok().map(|v| v * 1024 * 1024)
    } else if let Some(g) = u.strip_suffix('G') {
        g.parse::<u64>().ok().map(|v| v * 1024 * 1024 * 1024)
    } else {
        parse_int_or_hex(s).map(|v| v as u64)
    }
}
