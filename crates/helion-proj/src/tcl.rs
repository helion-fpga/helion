//! Vivado Batch & Project Mode TCL Engine (UG835 / UG892 / UG903).
//!
//! Supports both Non-Project Batch TCL (`read_verilog`, `read_vhdl`, `read_xdc`,
//! `synth_design`, `opt_design`, `place_design`, `phys_opt_design`, `route_design`,
//! `write_checkpoint`, `read_checkpoint`, `write_bitstream`, `report_*`) and
//! Project Mode TCL (`create_project`, `open_project`, `add_files`, `set_property`,
//! `create_run`, `launch_runs`, `report_qor_suggestions`), plus design object
//! queries (`get_ports`, `get_pins`, `get_cells`, `get_nets`, `get_clocks`,
//! `all_inputs`, `all_outputs`, `all_registers`).

use crate::checkpoint2::{load_checkpoint, netlist_sha256};
use crate::qor::report_qor_suggestions;
use crate::reports::{
    report_cdc_detailed, report_clock_interaction_detailed, report_clock_utilization_detailed,
    report_design_analysis_detailed, report_drc_detailed, report_high_fanout_nets_detailed,
    report_power_detailed, report_timing_detailed, report_timing_summary_text,
    report_utilization_detailed,
};
use crate::{
    ImplStrategy, Mode, PrjImplRun, ProjectFile, ReuseReport, Session, apply_design_xdc,
    constraints_from_project, expand_ip_packages, format_prj, load_prj, opt_design,
    resolve_prj_path, write_prj,
};
use helion_device::Device;
use helion_ir::{Cell, CellKind, Design, Net, Port, PortDir, intern, resolve};
use helion_sta::{Constraints, create_clock, load_xdc};
use helion_sv::{SvCompileOpts, elaborate_sv_sources};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Glob & -filter expression evaluation for Vivado object queries
// ---------------------------------------------------------------------------

/// Match a Tcl/Vivado glob pattern (`*`, `?`, `[abc]`, `[a-z]`).
pub fn tcl_glob_match(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let s: Vec<char> = text.chars().collect();
    glob_rec(&p, 0, &s, 0)
}

fn glob_rec(p: &[char], mut pi: usize, s: &[char], mut si: usize) -> bool {
    while pi < p.len() {
        match p[pi] {
            '*' => {
                while pi < p.len() && p[pi] == '*' {
                    pi += 1;
                }
                if pi == p.len() {
                    return true;
                }
                for k in si..=s.len() {
                    if glob_rec(p, pi, s, k) {
                        return true;
                    }
                }
                return false;
            }
            '?' => {
                if si >= s.len() {
                    return false;
                }
                pi += 1;
                si += 1;
            }
            '\\' => {
                pi += 1;
                if pi >= p.len() || si >= s.len() || p[pi] != s[si] {
                    return false;
                }
                pi += 1;
                si += 1;
            }
            _ => {
                if si >= s.len() || p[pi] != s[si] {
                    return false;
                }
                pi += 1;
                si += 1;
            }
        }
    }
    si == s.len()
}

/// Evaluate a Vivado `-filter` expression (e.g. `DIRECTION == IN`, `NAME =~ *lut* && IS_PRIMITIVE == 1`).
pub fn eval_tcl_filter(expr: &str, props: &BTreeMap<String, String>) -> bool {
    let expr = expr.trim().trim_matches(|c| c == '{' || c == '}' || c == '"');
    if expr.is_empty() {
        return true;
    }
    if let Some((l, r)) = split_top_op(expr, "||") {
        return eval_tcl_filter(l, props) || eval_tcl_filter(r, props);
    }
    if let Some((l, r)) = split_top_op(expr, "&&") {
        return eval_tcl_filter(l, props) && eval_tcl_filter(r, props);
    }
    if let Some(rest) = expr.strip_prefix('!') {
        let rest = rest.trim();
        if !rest.starts_with('=') && !rest.starts_with('~') {
            return !eval_tcl_filter(rest, props);
        }
    }
    if expr.starts_with('(') && expr.ends_with(')') {
        return eval_tcl_filter(&expr[1..expr.len() - 1], props);
    }
    for op in ["==", "!=", "=~", "!~", ">=", "<=", ">", "<"] {
        if let Some((lhs, rhs)) = expr.split_once(op) {
            let key = lhs.trim().to_ascii_uppercase();
            let rhs = rhs.trim().trim_matches(|c| c == '"' || c == '\'' || c == '{' || c == '}');
            let val = props.get(&key).map(|s| s.as_str()).unwrap_or("");
            return match op {
                "==" => val.eq_ignore_ascii_case(rhs) || bool_eq(val, rhs),
                "!=" => !(val.eq_ignore_ascii_case(rhs) || bool_eq(val, rhs)),
                "=~" => tcl_glob_match(rhs, val) || val.contains(rhs.trim_matches('*')),
                "!~" => !(tcl_glob_match(rhs, val) || val.contains(rhs.trim_matches('*'))),
                ">=" => val.parse::<f64>().unwrap_or(0.0) >= rhs.parse::<f64>().unwrap_or(0.0),
                "<=" => val.parse::<f64>().unwrap_or(0.0) <= rhs.parse::<f64>().unwrap_or(0.0),
                ">" => val.parse::<f64>().unwrap_or(0.0) > rhs.parse::<f64>().unwrap_or(0.0),
                "<" => val.parse::<f64>().unwrap_or(0.0) < rhs.parse::<f64>().unwrap_or(0.0),
                _ => false,
            };
        }
    }
    let key = expr.to_ascii_uppercase();
    props
        .get(&key)
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn bool_eq(a: &str, b: &str) -> bool {
    let norm = |s: &str| match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" | "" => Some(false),
        _ => None,
    };
    matches!((norm(a), norm(b)), (Some(x), Some(y)) if x == y)
}

fn split_top_op<'a>(s: &'a str, op: &str) -> Option<(&'a str, &'a str)> {
    let mut depth = 0i32;
    let bytes = s.as_bytes();
    let op_b = op.as_bytes();
    let mut i = 0;
    while i + op_b.len() <= bytes.len() {
        match bytes[i] {
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' => depth -= 1,
            _ => {}
        }
        if depth == 0 && &bytes[i..i + op_b.len()] == op_b {
            return Some((&s[..i], &s[i + op_b.len()..]));
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Object Property Maps & Standalone Query Helpers
// ---------------------------------------------------------------------------

pub fn cell_properties(design: &Design, cell: &Cell) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let cname = resolve(cell.name).to_string();
    let (ref_name, is_seq) = match cell.kind {
        CellKind::Lut6 { init } => {
            m.insert("INIT".into(), format!("{init:#x}"));
            ("LUT6", false)
        }
        CellKind::Hff => ("FDRE", true),
        CellKind::IobOut => ("OBUF", false),
        CellKind::Mac27 => ("DSP48E1", true),
        CellKind::Bram18 => ("RAMB18E1", true),
        _ => ("CELL", false),
    };
    m.insert("NAME".into(), cname);
    m.insert("CLASS".into(), "cell".into());
    m.insert("REF_NAME".into(), ref_name.into());
    m.insert("ORIG_REF_NAME".into(), ref_name.into());
    m.insert("PRIMITIVE_TYPE".into(), ref_name.into());
    m.insert("IS_PRIMITIVE".into(), "1".into());
    m.insert("IS_SEQUENTIAL".into(), if is_seq { "1" } else { "0" }.into());
    for (k, v) in &cell.attrs.map {
        m.insert(k.to_ascii_uppercase(), v.clone());
    }
    let _ = design;
    m
}

pub fn net_properties(net: &Net) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let nname = resolve(net.name).to_string();
    let fanout = net.endpoints.len().saturating_sub(1);
    let is_clk = nname.eq_ignore_ascii_case("clk")
        || nname.eq_ignore_ascii_case("clock")
        || net.endpoints.iter().any(|e| e.pin == intern("CLK"));
    m.insert("NAME".into(), nname);
    m.insert("CLASS".into(), "net".into());
    m.insert("FLAT_PIN_COUNT".into(), net.endpoints.len().to_string());
    m.insert("FANOUT".into(), fanout.to_string());
    m.insert("TYPE".into(), if is_clk { "GLOBAL_CLOCK" } else { "SIGNAL" }.into());
    for (k, v) in &net.attrs.map {
        m.insert(k.to_ascii_uppercase(), v.clone());
    }
    m
}

pub fn port_properties(port: &Port) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let pname = resolve(port.name).to_string();
    let dir = match port.dir {
        PortDir::In => "IN",
        PortDir::Out => "OUT",
        PortDir::Inout => "INOUT",
    };
    m.insert("NAME".into(), pname);
    m.insert("CLASS".into(), "port".into());
    m.insert("DIRECTION".into(), dir.into());
    for (k, v) in &port.attrs.map {
        m.insert(k.to_ascii_uppercase(), v.clone());
    }
    m
}

pub fn pin_properties(design: &Design, cell: &Cell, pin: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let cname = resolve(cell.name);
    let full = format!("{cname}/{pin}");
    let dir = match pin {
        "O" | "Q" | "PAD" => "OUT",
        _ => "IN",
    };
    m.insert("NAME".into(), full);
    m.insert("REF_PIN_NAME".into(), pin.into());
    m.insert("CELL".into(), cname.into());
    m.insert("CLASS".into(), "pin".into());
    m.insert("DIRECTION".into(), dir.into());
    m.insert("IS_CLOCK".into(), if pin == "CLK" { "1" } else { "0" }.into());
    if let Some(net) = design.net_on(cell.name, intern(pin)) {
        m.insert("NET".into(), resolve(net).to_string());
    }
    m
}

/// Query top-level ports in `d`, optionally filtered by substring or glob pattern.
pub fn get_ports(d: &Design, filter: Option<&str>) -> Vec<String> {
    d.ports
        .iter()
        .map(|p| resolve(p.name).to_string())
        .filter(|name| match filter {
            None => true,
            Some(f) => {
                if f.contains('*') || f.contains('?') {
                    tcl_glob_match(f, name)
                } else {
                    name.contains(f)
                }
            }
        })
        .collect()
}

/// Query clocks in `xdc` (falls back to default `clk` when empty only if requested by session).
pub fn get_clocks(xdc: &Constraints, filter: Option<&str>) -> Vec<String> {
    xdc.clocks
        .iter()
        .map(|c| c.name.clone())
        .filter(|name| match filter {
            None => true,
            Some(f) => {
                if f.contains('*') || f.contains('?') {
                    tcl_glob_match(f, name)
                } else {
                    name.contains(f)
                }
            }
        })
        .collect()
}

/// Return all input ports in `d` (`UG835 all_inputs`).
pub fn all_inputs(d: &Design) -> Vec<String> {
    d.ports
        .iter()
        .filter(|p| matches!(p.dir, PortDir::In | PortDir::Inout))
        .map(|p| resolve(p.name).to_string())
        .collect()
}

/// Return all output ports in `d` (`UG835 all_outputs`).
pub fn all_outputs(d: &Design) -> Vec<String> {
    d.ports
        .iter()
        .filter(|p| matches!(p.dir, PortDir::Out | PortDir::Inout))
        .map(|p| resolve(p.name).to_string())
        .collect()
}

/// Return all sequential register cells (`Hff`, `Bram18`, `Mac27`) in `d` (`UG835 all_registers`).
pub fn all_registers(d: &Design) -> Vec<String> {
    d.cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff | CellKind::Bram18 | CellKind::Mac27))
        .map(|c| resolve(c.name).to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Built-in VHDL-2008 -> SV Translator for standalone `read_vhdl` + `synth_design`
// ---------------------------------------------------------------------------

/// Translate synthesizable VHDL-2008 entity/architecture text into SystemVerilog
/// so `helion-proj` can synthesize multi-file VHDL + SV projects directly.
pub fn translate_vhdl_to_sv(vhdl_src: &str) -> Result<String, String> {
    let mut cleaned = String::with_capacity(vhdl_src.len());
    for line in vhdl_src.lines() {
        let code = line.split("--").next().unwrap_or("");
        cleaned.push_str(code);
        cleaned.push('\n');
    }

    let lower = cleaned.to_ascii_lowercase();
    let mut sv_out = String::new();
    let mut search_pos = 0usize;

    while let Some(ent_rel) = lower[search_pos..].find("entity ") {
        let ent_start = search_pos + ent_rel + "entity ".len();
        let rest_ent = &cleaned[ent_start..];
        let rest_low = &lower[ent_start..];
        let Some(is_rel) = rest_low.find(" is") else {
            break;
        };
        let entity_name = rest_ent[..is_rel].trim();
        if entity_name.is_empty()
            || !entity_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            search_pos = ent_start + is_rel + 3;
            continue;
        }
        let end_pat1 = format!("end entity {entity_name}").to_ascii_lowercase();
        let end_pat2 = format!("end {entity_name}").to_ascii_lowercase();
        let body_end = rest_low
            .find(&end_pat1)
            .or_else(|| rest_low.find(&end_pat2))
            .or_else(|| rest_low.find("end entity;"))
            .unwrap_or(rest_ent.len());
        let ent_body = &rest_ent[is_rel + 3..body_end];
        search_pos = ent_start + body_end + 3;

        let generics = extract_vhdl_generics(ent_body);
        let ports = extract_vhdl_ports(ent_body);

        // Find matching architecture
        let arch_marker = format!(" of {}", entity_name.to_ascii_lowercase());
        let (signals, stmts) = if let Some(of_pos) = lower.find(&arch_marker) {
            let after_of = &cleaned[of_pos + arch_marker.len()..];
            let after_of_low = &lower[of_pos + arch_marker.len()..];
            if let Some(begin_pos) = find_kw_pos(after_of_low, "begin") {
                let decl_part = &after_of[..begin_pos];
                let after_begin = &after_of[begin_pos + 5..];
                let after_begin_low = &after_of_low[begin_pos + 5..];
                let arch_end = after_begin_low
                    .rfind("end ")
                    .unwrap_or(after_begin.len());
                let stmt_part = &after_begin[..arch_end];
                (
                    extract_vhdl_signals(decl_part),
                    translate_vhdl_arch_body(stmt_part, &ports),
                )
            } else {
                (Vec::new(), Vec::new())
            }
        } else {
            (Vec::new(), Vec::new())
        };

        // Emit SV module
        if generics.is_empty() {
            sv_out.push_str(&format!("module {entity_name} (\n"));
        } else {
            let params: Vec<String> = generics
                .iter()
                .map(|(k, v)| format!("parameter {k} = {v}"))
                .collect();
            sv_out.push_str(&format!(
                "module {entity_name} #({}) (\n",
                params.join(", ")
            ));
        }
        let port_decls: Vec<String> = ports
            .iter()
            .map(|(pname, pdir, pwidth)| {
                let dir_kw = if pdir == "out" { "output logic" } else { "input logic" };
                if *pwidth > 1 {
                    format!("  {dir_kw} [{}:0] {pname}", pwidth - 1)
                } else {
                    format!("  {dir_kw} {pname}")
                }
            })
            .collect();
        sv_out.push_str(&port_decls.join(",\n"));
        sv_out.push_str("\n);\n");
        for (sname, swidth) in &signals {
            if *swidth > 1 {
                sv_out.push_str(&format!("  logic [{}:0] {sname};\n", swidth - 1));
            } else {
                sv_out.push_str(&format!("  logic {sname};\n"));
            }
        }
        for st in &stmts {
            sv_out.push_str("  ");
            sv_out.push_str(st);
            sv_out.push('\n');
        }
        sv_out.push_str("endmodule\n\n");
    }

    if sv_out.trim().is_empty() {
        return Err("read_vhdl: no synthesizable VHDL entity found".into());
    }
    Ok(sv_out)
}

fn find_kw_pos(s: &str, kw: &str) -> Option<usize> {
    let b = s.as_bytes();
    let kb = kw.as_bytes();
    if kb.is_empty() || b.len() < kb.len() {
        return None;
    }
    for i in 0..=b.len() - kb.len() {
        if &b[i..i + kb.len()] == kb {
            let prev_ok = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
            let next_ok = i + kb.len() == b.len()
                || !(b[i + kb.len()].is_ascii_alphanumeric() || b[i + kb.len()] == b'_');
            if prev_ok && next_ok {
                return Some(i);
            }
        }
    }
    None
}

fn extract_balanced_parens<'a>(text: &'a str, kw: &str) -> Option<&'a str> {
    let low = text.to_ascii_lowercase();
    let pos = find_kw_pos(&low, kw)?;
    let rest = &text[pos + kw.len()..];
    let open = rest.find('(')?;
    let mut depth = 0i32;
    for (i, ch) in rest[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&rest[open + 1..open + i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_vhdl_width(ty_str: &str) -> usize {
    let low = ty_str.to_ascii_lowercase();
    if let Some(d_pos) = low.find("downto") {
        let left = low[..d_pos]
            .rsplit('(')
            .next()
            .unwrap_or("0")
            .trim();
        let right = low[d_pos + 6..]
            .split(')')
            .next()
            .unwrap_or("0")
            .trim();
        let hi: i64 = eval_simple_int(left).unwrap_or(0);
        let lo: i64 = eval_simple_int(right).unwrap_or(0);
        return (hi - lo).unsigned_abs() as usize + 1;
    }
    1
}

fn eval_simple_int(expr: &str) -> Option<i64> {
    let expr = expr.trim();
    if let Some((a, b)) = expr.split_once('-') {
        return Some(a.trim().parse::<i64>().ok()? - b.trim().parse::<i64>().ok()?);
    }
    if let Some((a, b)) = expr.split_once('+') {
        return Some(a.trim().parse::<i64>().ok()? + b.trim().parse::<i64>().ok()?);
    }
    expr.parse::<i64>().ok()
}

fn extract_vhdl_generics(ent_body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(inner) = extract_balanced_parens(ent_body, "generic") else {
        return out;
    };
    for item in inner.split(';') {
        let item = item.trim();
        let Some((names, rest)) = item.split_once(':') else {
            continue;
        };
        let default_val = rest
            .split_once(":=")
            .map(|(_, v)| v.trim().to_string())
            .unwrap_or_else(|| "1".into());
        for n in names.split(',') {
            let n = n.trim();
            if !n.is_empty() {
                out.push((n.to_string(), default_val.clone()));
            }
        }
    }
    out
}

fn extract_vhdl_ports(ent_body: &str) -> Vec<(String, String, usize)> {
    let mut out = Vec::new();
    let Some(inner) = extract_balanced_parens(ent_body, "port") else {
        return out;
    };
    for item in inner.split(';') {
        let item = item.trim();
        let Some((names, rest)) = item.split_once(':') else {
            continue;
        };
        let rest_trim = rest.trim();
        let mut toks = rest_trim.split_whitespace();
        let dir_tok = toks.next().unwrap_or("in").to_ascii_lowercase();
        let dir = if dir_tok == "out" || dir_tok == "buffer" {
            "out"
        } else {
            "in"
        };
        let width = parse_vhdl_width(rest_trim);
        for n in names.split(',') {
            let n = n.trim();
            if !n.is_empty() {
                out.push((n.to_string(), dir.to_string(), width));
            }
        }
    }
    out
}

fn extract_vhdl_signals(decl_part: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for stmt in decl_part.split(';') {
        let stmt = stmt.trim();
        let low = stmt.to_ascii_lowercase();
        let Some(sig_pos) = find_kw_pos(&low, "signal") else {
            continue;
        };
        let after_sig = stmt[sig_pos + 6..].trim();
        let Some((names, ty_part)) = after_sig.split_once(':') else {
            continue;
        };
        let width = parse_vhdl_width(ty_part);
        for n in names.split(',') {
            let n = n.trim();
            if !n.is_empty() {
                out.push((n.to_string(), width));
            }
        }
    }
    out
}

fn vhdl_expr_to_sv(expr: &str) -> String {
    let mut s = expr.trim().to_string();
    for (pat, rep) in [
        ("'0'", "1'b0"),
        ("'1'", "1'b1"),
        ("\"", "'h"),
        (" and ", " & "),
        (" AND ", " & "),
        (" or ", " | "),
        (" OR ", " | "),
        (" xor ", " ^ "),
        (" XOR ", " ^ "),
        ("not ", "~"),
        ("NOT ", "~"),
        ("unsigned(", "("),
        ("std_logic_vector(", "("),
        ("std_ulogic_vector(", "("),
    ] {
        s = s.replace(pat, rep);
    }
    // Convert `sig(idx)` bit index to `sig[idx]` when sig is an identifier
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '(' && i > 0 && (chars[i - 1].is_ascii_alphanumeric() || chars[i - 1] == '_')
        {
            if let Some(close_rel) = s[i + 1..].find(')') {
                let inner = s[i + 1..i + 1 + close_rel].trim();
                let low_inner = inner.to_ascii_lowercase();
                if let Some((hi, lo)) = low_inner.split_once("downto") {
                    out.push_str(&format!("[{}:{}]", hi.trim(), lo.trim()));
                } else {
                    out.push_str(&format!("[{inner}]"));
                }
                i += close_rel + 2;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn translate_vhdl_arch_body(
    stmt_part: &str,
    ports: &[(String, String, usize)],
) -> Vec<String> {
    let mut out = Vec::new();
    let clk_name = ports
        .iter()
        .find(|(n, d, _)| {
            d == "in"
                && (n.eq_ignore_ascii_case("clk")
                    || n.eq_ignore_ascii_case("clock")
                    || n.to_ascii_lowercase().contains("clk"))
        })
        .map(|(n, _, _)| n.clone())
        .unwrap_or_else(|| "clk".into());

    let lower = stmt_part.to_ascii_lowercase();
    let mut cursor = 0usize;

    while let Some(proc_rel) = lower[cursor..].find("process") {
        let proc_start = cursor + proc_rel;
        // Translate any concurrent assignments before `process`
        translate_concurrent_chunk(&stmt_part[cursor..proc_start], &mut out);

        let after_proc = &stmt_part[proc_start..];
        let after_proc_low = &lower[proc_start..];
        let end_proc_rel = after_proc_low
            .find("end process")
            .unwrap_or(after_proc.len());
        let proc_block = &after_proc[..end_proc_rel];
        let proc_block_low = &after_proc_low[..end_proc_rel];
        if let Some(bpos) = find_kw_pos(proc_block_low, "begin") {
            let pbody = &proc_block[bpos + 5..];
            out.push(format!("always_ff @(posedge {clk_name}) begin"));
            for raw_stmt in pbody.split(';') {
                let st = raw_stmt.trim();
                if let Some((lhs, rhs)) = st.split_once("<=") {
                    let lhs_clean = lhs
                        .rsplit("then")
                        .next()
                        .unwrap_or(lhs)
                        .rsplit("else")
                        .next()
                        .unwrap_or(lhs)
                        .trim();
                    if !lhs_clean.is_empty()
                        && !lhs_clean.to_ascii_lowercase().starts_with("if ")
                        && !lhs_clean.to_ascii_lowercase().starts_with("end ")
                    {
                        let sv_lhs = vhdl_expr_to_sv(lhs_clean);
                        let sv_rhs = vhdl_expr_to_sv(rhs);
                        out.push(format!("  {sv_lhs} <= {sv_rhs};"));
                    }
                }
            }
            out.push("end".into());
        }
        let semi_after = after_proc[end_proc_rel..]
            .find(';')
            .map(|p| end_proc_rel + p + 1)
            .unwrap_or(after_proc.len());
        cursor = proc_start + semi_after;
    }

    if cursor < stmt_part.len() {
        translate_concurrent_chunk(&stmt_part[cursor..], &mut out);
    }
    out
}

fn translate_concurrent_chunk(chunk: &str, out: &mut Vec<String>) {
    for raw in chunk.split(';') {
        let st = raw.trim();
        if st.is_empty() {
            continue;
        }
        // Component / entity instantiation: `u1: entity work.foo port map (a => x, b => y)`
        let low = st.to_ascii_lowercase();
        if let Some(pm_pos) = low.find("port map") {
            let header = st[..pm_pos].trim();
            if let Some((inst_label, mod_spec)) = header.split_once(':') {
                let inst_name = inst_label.trim();
                let mod_name = mod_spec
                    .split_whitespace()
                    .last()
                    .unwrap_or("")
                    .rsplit('.')
                    .next()
                    .unwrap_or("")
                    .trim();
                if let Some(map_inner) = extract_balanced_parens(&st[pm_pos..], "map") {
                    let mut conns = Vec::new();
                    for pair in map_inner.split(',') {
                        if let Some((formal, actual)) = pair.split_once("=>") {
                            conns.push(format!(
                                ".{}({})",
                                formal.trim(),
                                vhdl_expr_to_sv(actual.trim())
                            ));
                        }
                    }
                    if !mod_name.is_empty() && !inst_name.is_empty() {
                        out.push(format!("{mod_name} {inst_name} ({});", conns.join(", ")));
                        continue;
                    }
                }
            }
        }
        if let Some((lhs, rhs)) = st.split_once("<=") {
            let lhs = lhs.trim();
            if !lhs.is_empty() && !lhs.contains(':') {
                out.push(format!(
                    "assign {} = {};",
                    vhdl_expr_to_sv(lhs),
                    vhdl_expr_to_sv(rhs)
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Vivado Batch & Project Mode TCL Session
// ---------------------------------------------------------------------------

/// Complete Vivado-compatible Project & Non-Project Batch TCL execution environment.
#[derive(Clone, Debug)]
pub struct VivadoTclSession {
    pub session: Session,
    pub project: ProjectFile,
    pub project_name: String,
    pub project_dir: PathBuf,
    pub project_file_path: Option<PathBuf>,
    pub constraints: Constraints,
    pub inline_xdc_lines: Vec<String>,
    pub vars: BTreeMap<String, String>,
    pub procs: BTreeMap<String, (Vec<String>, String)>,
    pub generics: BTreeMap<String, String>,
    pub verilog_defines: BTreeMap<String, String>,
    pub synth_args: BTreeMap<String, String>,
    pub run_props: BTreeMap<String, BTreeMap<String, String>>,
    pub phys_opt_enabled: bool,
    pub incremental_ref_path: Option<PathBuf>,
    pub incremental_armed: bool,
    pub last_reuse_report: Option<ReuseReport>,
    pub current_run: String,
    pub current_fileset: String,
    pub output_log: Vec<String>,
}

impl Default for VivadoTclSession {
    fn default() -> Self {
        Self::new(Mode::NonProject)
    }
}

impl VivadoTclSession {
    pub fn new(mode: Mode) -> Self {
        let mut project = ProjectFile::default();
        project.part = "HL10T-C32-1".into();
        project.impl_runs.push(PrjImplRun {
            name: "synth_1".into(),
            strategy: "Default".into(),
            spec: String::new(),
        });
        project.impl_runs.push(PrjImplRun {
            name: "impl_1".into(),
            strategy: "Default".into(),
            spec: String::new(),
        });
        Self {
            session: Session::new(mode),
            project,
            project_name: "project_1".into(),
            project_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            project_file_path: None,
            constraints: Constraints::default(),
            inline_xdc_lines: Vec::new(),
            vars: BTreeMap::new(),
            procs: BTreeMap::new(),
            generics: BTreeMap::new(),
            verilog_defines: BTreeMap::new(),
            synth_args: BTreeMap::new(),
            run_props: BTreeMap::new(),
            phys_opt_enabled: false,
            incremental_ref_path: None,
            incremental_armed: false,
            last_reuse_report: None,
            current_run: "impl_1".into(),
            current_fileset: "sources_1".into(),
            output_log: Vec::new(),
        }
    }

    pub fn with_base_dir(mut self, dir: impl AsRef<Path>) -> Self {
        self.project_dir = dir.as_ref().to_path_buf();
        self
    }

    pub fn device(&self) -> Result<Device, String> {
        let part = if self.session.part.is_empty() {
            if self.project.part.is_empty() {
                "HL10T-C32-1"
            } else {
                &self.project.part
            }
        } else {
            &self.session.part
        };
        Device::load_part(part).or_else(|_| Device::load_part("HL10T-C32-1"))
    }

    pub fn resolve_path(&self, raw: &str) -> PathBuf {
        if raw == "." || raw == "./" {
            return self.project_dir.clone();
        }
        let p = Path::new(raw);
        if p.is_absolute() {
            return p.to_path_buf();
        }
        let cand = self.project_dir.join(p);
        if cand.exists() {
            return cand;
        }
        if let Some(pf) = &self.project_file_path {
            let resolved = resolve_prj_path(pf, raw);
            if resolved.exists() {
                return resolved;
            }
        }
        if p.exists() {
            return p.to_path_buf();
        }
        cand
    }

    /// Evaluate a multi-statement Vivado TCL script and return the last command's result.
    pub fn eval_script(&mut self, script: &str) -> Result<String, String> {
        let stmts = split_tcl_statements(script);
        let mut last = String::new();
        for stmt in stmts {
            let words = self.parse_and_subst_words(&stmt)?;
            if words.is_empty() {
                continue;
            }
            last = self.dispatch_command(&words)?;
        }
        Ok(last)
    }

    /// Evaluate a single TCL line/command.
    pub fn eval(&mut self, cmd_line: &str) -> Result<String, String> {
        self.eval_script(cmd_line)
    }

    fn parse_and_subst_words(&mut self, stmt: &str) -> Result<Vec<String>, String> {
        let chars: Vec<char> = stmt.chars().collect();
        let mut words = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            if i >= chars.len() || chars[i] == '#' {
                break;
            }
            if chars[i] == '{' {
                let mut depth = 0i32;
                let start = i + 1;
                while i < chars.len() {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 2;
                        continue;
                    }
                    if chars[i] == '{' {
                        depth += 1;
                    } else if chars[i] == '}' {
                        depth -= 1;
                        if depth == 0 {
                            let body: String = chars[start..i].iter().collect();
                            words.push(body);
                            i += 1;
                            break;
                        }
                    }
                    i += 1;
                }
                continue;
            }
            let mut buf = String::new();
            let in_quote = if chars[i] == '"' {
                i += 1;
                true
            } else {
                false
            };
            while i < chars.len() {
                if in_quote {
                    if chars[i] == '"' {
                        i += 1;
                        break;
                    }
                } else if chars[i].is_whitespace() {
                    break;
                }
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                    buf.push(chars[i]);
                    i += 1;
                    continue;
                }
                if chars[i] == '[' {
                    let mut depth = 0i32;
                    let sub_start = i + 1;
                    while i < chars.len() {
                        if chars[i] == '[' {
                            depth += 1;
                        } else if chars[i] == ']' {
                            depth -= 1;
                            if depth == 0 {
                                let inner: String = chars[sub_start..i].iter().collect();
                                let val = self.eval_script(&inner)?;
                                buf.push_str(&val);
                                i += 1;
                                break;
                            }
                        }
                        i += 1;
                    }
                    continue;
                }
                if chars[i] == '$' {
                    i += 1;
                    if i < chars.len() && chars[i] == '{' {
                        i += 1;
                        let vstart = i;
                        while i < chars.len() && chars[i] != '}' {
                            i += 1;
                        }
                        let vname: String = chars[vstart..i].iter().collect();
                        if i < chars.len() {
                            i += 1;
                        }
                        if let Some(val) = self.vars.get(&vname) {
                            buf.push_str(val);
                        }
                    } else {
                        let vstart = i;
                        while i < chars.len()
                            && (chars[i].is_ascii_alphanumeric() || chars[i] == '_')
                        {
                            i += 1;
                        }
                        let vname: String = chars[vstart..i].iter().collect();
                        if vname.is_empty() {
                            buf.push('$');
                        } else if let Some(val) = self.vars.get(&vname) {
                            buf.push_str(val);
                        }
                    }
                    continue;
                }
                buf.push(chars[i]);
                i += 1;
            }
            words.push(buf);
        }
        Ok(words)
    }
}

fn split_tcl_statements(script: &str) -> Vec<String> {
    let mut joined = String::with_capacity(script.len());
    let mut lines = script.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed_end = line.trim_end();
        if let Some(without_bs) = trimmed_end.strip_suffix('\\') {
            joined.push_str(without_bs);
            joined.push(' ');
        } else {
            joined.push_str(line);
            joined.push('\n');
        }
    }

    let mut out = Vec::new();
    let mut cur = String::new();
    let mut brace_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut in_quote = false;
    let chars: Vec<char> = joined.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            cur.push(c);
            cur.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if brace_depth == 0 && !in_quote && c == '#' && cur.trim().is_empty() {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if brace_depth == 0 && c == '"' {
            in_quote = !in_quote;
        } else if !in_quote {
            match c {
                '{' => brace_depth += 1,
                '}' => brace_depth = (brace_depth - 1).max(0),
                '[' => bracket_depth += 1,
                ']' => bracket_depth = (bracket_depth - 1).max(0),
                _ => {}
            }
        }
        if !in_quote && brace_depth == 0 && bracket_depth == 0 && (c == '\n' || c == ';') {
            let trimmed = cur.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
            cur.clear();
        } else {
            cur.push(c);
        }
        i += 1;
    }
    let trimmed = cur.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    out
}

pub fn split_tcl_list(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.trim().chars().collect();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        if chars[i] == '{' {
            let mut depth = 0i32;
            let start = i + 1;
            while i < chars.len() {
                if chars[i] == '{' {
                    depth += 1;
                } else if chars[i] == '}' {
                    depth -= 1;
                    if depth == 0 {
                        out.push(chars[start..i].iter().collect());
                        i += 1;
                        break;
                    }
                }
                i += 1;
            }
        } else if chars[i] == '"' {
            i += 1;
            let start = i;
            while i < chars.len() && chars[i] != '"' {
                i += 1;
            }
            out.push(chars[start..i].iter().collect());
            if i < chars.len() {
                i += 1;
            }
        } else {
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            out.push(chars[start..i].iter().collect());
        }
    }
    out
}

struct QueryArgs {
    filter: Option<String>,
    of_objects: Option<String>,
    clock: Option<String>,
    data_pins: bool,
    clock_pins: bool,
    output_pins: bool,
    async_pins: bool,
    patterns: Vec<String>,
}

fn parse_query_args(argv: &[String]) -> QueryArgs {
    let mut out = QueryArgs {
        filter: None,
        of_objects: None,
        clock: None,
        data_pins: false,
        clock_pins: false,
        output_pins: false,
        async_pins: false,
        patterns: Vec::new(),
    };
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "-filter" => {
                if i + 1 < argv.len() {
                    out.filter = Some(argv[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            "-of_objects" | "-of" => {
                if i + 1 < argv.len() {
                    out.of_objects = Some(argv[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            "-clock" | "-clocks" => {
                if i + 1 < argv.len() {
                    out.clock = Some(argv[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            "-data_pins" => out.data_pins = true,
            "-clock_pins" => out.clock_pins = true,
            "-output_pins" => out.output_pins = true,
            "-async_pins" => out.async_pins = true,
            "-hierarchical" | "-hier" | "-regexp" | "-nocase" | "-quiet" | "-leaf"
            | "-primitives" | "-segments" => {}
            arg if !arg.starts_with('-') => {
                for item in split_tcl_list(arg) {
                    if !item.is_empty() {
                        out.patterns.push(item);
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn match_patterns(name: &str, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return true;
    }
    patterns.iter().any(|p| {
        if p.contains('*') || p.contains('?') {
            tcl_glob_match(p, name)
        } else {
            name == p || name.contains(p.as_str())
        }
    })
}

impl VivadoTclSession {
    fn dispatch_command(&mut self, argv: &[String]) -> Result<String, String> {
        let cmd = argv[0].as_str();
        if let Some((params, body)) = self.procs.get(cmd).cloned() {
            let saved = self.vars.clone();
            for (idx, p) in params.iter().enumerate() {
                let val = argv.get(idx + 1).cloned().unwrap_or_default();
                self.vars.insert(p.clone(), val);
            }
            let res = self.eval_script(&body);
            self.vars = saved;
            return res;
        }

        match cmd {
            // --- Core Tcl builtins ---
            "set" => {
                if argv.len() < 2 {
                    return Err("set: missing variable name".into());
                }
                if argv.len() == 2 {
                    return Ok(self.vars.get(&argv[1]).cloned().unwrap_or_default());
                }
                let val = argv[2..].join(" ");
                self.vars.insert(argv[1].clone(), val.clone());
                Ok(val)
            }
            "puts" => {
                let msg = if argv.len() >= 3 && (argv[1] == "-nonewline" || argv[1] == "stdout" || argv[1] == "stderr") {
                    argv[2..].join(" ")
                } else {
                    argv[1..].join(" ")
                };
                self.output_log.push(msg.clone());
                Ok(msg)
            }
            "list" => Ok(argv[1..].join(" ")),
            "llength" => {
                let items = argv.get(1).map(|s| split_tcl_list(s)).unwrap_or_default();
                Ok(items.len().to_string())
            }
            "lindex" => {
                let items = argv.get(1).map(|s| split_tcl_list(s)).unwrap_or_default();
                let idx: usize = argv.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                Ok(items.get(idx).cloned().unwrap_or_default())
            }
            "expr" => {
                let joined = argv[1..].join("");
                if let Some(v) = eval_simple_int(&joined) {
                    Ok(v.to_string())
                } else {
                    Ok(joined)
                }
            }
            "foreach" => {
                if argv.len() < 4 {
                    return Err("foreach: expected var list body".into());
                }
                let var = argv[1].clone();
                let items = split_tcl_list(&argv[2]);
                let body = argv[3].clone();
                let mut last = String::new();
                for item in items {
                    self.vars.insert(var.clone(), item);
                    last = self.eval_script(&body)?;
                }
                Ok(last)
            }
            "if" => {
                if argv.len() < 3 {
                    return Err("if: expected cond body".into());
                }
                let cond_str = &argv[1];
                let empty_props = BTreeMap::new();
                let truthy = cond_str == "1"
                    || cond_str.eq_ignore_ascii_case("true")
                    || eval_tcl_filter(cond_str, &empty_props);
                let then_idx = if argv.get(2).map(|s| s.as_str()) == Some("then") {
                    3
                } else {
                    2
                };
                if truthy {
                    if let Some(body) = argv.get(then_idx) {
                        return self.eval_script(body);
                    }
                } else {
                    let else_idx = if argv.get(then_idx + 1).map(|s| s.as_str()) == Some("else") {
                        then_idx + 2
                    } else {
                        then_idx + 1
                    };
                    if let Some(body) = argv.get(else_idx) {
                        return self.eval_script(body);
                    }
                }
                Ok(String::new())
            }
            "proc" => {
                if argv.len() < 4 {
                    return Err("proc: expected name args body".into());
                }
                let params = split_tcl_list(&argv[2]);
                self.procs
                    .insert(argv[1].clone(), (params, argv[3].clone()));
                Ok(argv[1].clone())
            }
            "source" => {
                let path = argv.get(1).ok_or("source: missing file path")?;
                let resolved = self.resolve_path(path);
                let content = std::fs::read_to_string(&resolved)
                    .map_err(|e| format!("source {}: {e}", resolved.display()))?;
                self.eval_script(&content)
            }

            // --- Project & Source Management (UG835 / UG892) ---
            "create_project" => self.cmd_create_project(argv),
            "open_project" => self.cmd_open_project(argv),
            "close_project" => Ok("close_project ok".into()),
            "save_project_as" => self.cmd_save_project(argv),
            "current_project" => Ok(self.project_name.clone()),
            "current_fileset" => Ok(self.current_fileset.clone()),
            "get_filesets" => {
                let pat = argv.iter().skip(1).find(|a| !a.starts_with('-'));
                if let Some(p) = pat {
                    Ok(p.clone())
                } else {
                    Ok("sources_1 constrs_1 sim_1".into())
                }
            }
            "current_run" => Ok(self.current_run.clone()),
            "get_runs" => {
                let pat = argv.iter().skip(1).find(|a| !a.starts_with('-'));
                let names: Vec<String> = self
                    .project
                    .impl_runs
                    .iter()
                    .map(|r| r.name.clone())
                    .filter(|n| match pat {
                        None => true,
                        Some(p) => tcl_glob_match(p, n) || n == p.as_str(),
                    })
                    .collect();
                if names.is_empty() {
                    if let Some(p) = pat {
                        Ok(p.clone())
                    } else {
                        Ok("synth_1 impl_1".into())
                    }
                } else {
                    Ok(names.join(" "))
                }
            }
            "current_design" => {
                let d = self
                    .session
                    .design
                    .as_ref()
                    .ok_or("current_design: no open design")?;
                Ok(resolve(d.name).to_string())
            }
            "add_files" | "import_files" => self.cmd_add_files(argv),
            "get_files" => {
                let mut all = self.project.sources.clone();
                all.extend(self.project.constraint_files.clone());
                Ok(all.join(" "))
            }
            "read_verilog" | "read_sv" => self.cmd_read_hdl(argv, false),
            "read_vhdl" => self.cmd_read_hdl(argv, true),
            "read_xdc" | "read_sdc" => self.cmd_read_xdc(argv),
            "read_ip" => {
                for arg in argv.iter().skip(1) {
                    if !arg.starts_with('-') {
                        for f in split_tcl_list(arg) {
                            if !self.project.ip_packages.contains(&f) {
                                self.project.ip_packages.push(f);
                            }
                        }
                    }
                }
                Ok(self.project.ip_packages.join(" "))
            }

            // --- Properties (UG835 / UG903) ---
            "set_property" => self.cmd_set_property(argv),
            "get_property" => self.cmd_get_property(argv),
            "list_property" | "list_properties" => {
                Ok("NAME CLASS DIRECTION REF_NAME INIT LOC PACKAGE_PIN IOSTANDARD DRIVE SLEW PULLTYPE DIFF_TERM IN_TERM MARK_DEBUG DONT_TOUCH MAX_FANOUT STRATEGY PART TOP".into())
            }
            "report_property" => {
                let target = argv.last().cloned().unwrap_or_default();
                Ok(format!("report_property {target} CLASS=object NAME={target}"))
            }
            "set_part" => {
                let part = argv
                    .iter()
                    .skip(1)
                    .find(|a| !a.starts_with('-'))
                    .ok_or("set_part: missing part")?;
                self.project.part = part.clone();
                self.session.part = part.clone();
                Ok(part.clone())
            }

            // --- Design Object Queries (UG835 / UG903) ---
            "get_ports" => self.cmd_get_ports(argv),
            "get_pins" => self.cmd_get_pins(argv),
            "get_cells" => self.cmd_get_cells(argv),
            "get_nets" => self.cmd_get_nets(argv),
            "get_clocks" => self.cmd_get_clocks(argv),
            "all_inputs" => self.cmd_all_inputs(argv),
            "all_outputs" => self.cmd_all_outputs(argv),
            "all_registers" | "all_ffs" => self.cmd_all_registers(argv),
            "all_clocks" => self.cmd_get_clocks(&["get_clocks".to_string()]),
            "get_pblocks" => {
                let names: Vec<String> = self
                    .project
                    .pblocks
                    .iter()
                    .map(|p| p.name.clone())
                    .collect();
                Ok(names.join(" "))
            }

            // --- Inline XDC / Floorplan Commands ---
            "create_clock"
            | "create_generated_clock"
            | "set_input_delay"
            | "set_output_delay"
            | "set_false_path"
            | "set_multicycle_path"
            | "set_max_delay"
            | "set_min_delay"
            | "set_clock_groups"
            | "set_clock_uncertainty"
            | "set_clock_latency"
            | "set_disable_timing"
            | "set_case_analysis"
            | "set_propagated_clock"
            | "set_clock_sense"
            | "set_input_jitter"
            | "set_system_jitter"
            | "set_timing_derate"
            | "set_operating_conditions"
            | "set_bus_skew"
            | "group_path"
            | "set_max_time_borrow"
            | "set_data_check" => self.cmd_inline_xdc(argv),
            "create_pblock" | "resize_pblock" | "add_cells_to_pblock" => self.cmd_pblock(argv),
            "mark_debug" => {
                let net = argv.get(1).ok_or("mark_debug: missing net")?;
                for n in split_tcl_list(net) {
                    if !self.project.mark_debug.contains(&n) {
                        self.project.mark_debug.push(n.clone());
                    }
                    if self.session.design.is_some() {
                        let _ = self.session.mark_debug(&n);
                    }
                }
                Ok(format!("mark_debug {net}"))
            }

            // --- Synthesis, Implementation, Checkpoints & Bitstream ---
            "synth_design" => self.cmd_synth_design(argv),
            "opt_design" => self.cmd_opt_design(argv),
            "place_design" => self.cmd_place_design(argv),
            "phys_opt_design" => self.cmd_phys_opt_design(argv),
            "route_design" => self.cmd_route_design(argv),
            "write_bitstream" => self.cmd_write_bitstream(argv),
            "write_checkpoint" => self.cmd_write_checkpoint(argv),
            "read_checkpoint" | "open_checkpoint" => self.cmd_read_checkpoint(argv),

            // --- Multi-Strategy Runs ---
            "create_run" => self.cmd_create_run(argv),
            "launch_runs" => self.cmd_launch_runs(argv),
            "wait_on_run" | "wait_on_runs" => {
                let r = argv.get(1).cloned().unwrap_or_else(|| "impl_1".into());
                Ok(format!("wait_on_run {r} Complete"))
            }
            "open_run" => {
                let r = argv.get(1).cloned().unwrap_or_else(|| "impl_1".into());
                self.current_run = r.clone();
                Ok(format!("open_run {r}"))
            }
            "reset_run" => {
                let r = argv.get(1).cloned().unwrap_or_else(|| "impl_1".into());
                if r.starts_with("synth") {
                    self.session.reset_synth();
                } else {
                    self.session.reset_impl();
                }
                Ok(format!("reset_run {r}"))
            }

            // --- Reporting Commands (UG835 / UG906 / UG949) ---
            "report_timing_summary" => self.cmd_report_timing_summary(argv),
            "report_timing" => self.cmd_report_timing(argv),
            "report_utilization" => self.cmd_report_utilization(argv),
            "report_power" => self.cmd_report_power(argv),
            "report_drc" => self.cmd_report_drc(argv),
            "report_cdc" => self.cmd_report_cdc(argv),
            "report_clock_interaction" => self.cmd_report_clock_interaction(argv),
            "report_clock_utilization" | "report_clock_networks" => {
                self.cmd_report_clock_utilization(argv)
            }
            "report_high_fanout_nets" => self.cmd_report_high_fanout_nets(argv),
            "report_design_analysis" => self.cmd_report_design_analysis(argv),
            "report_qor_suggestions" | "report_qor_assessment" => {
                self.cmd_report_qor_suggestions(argv)
            }

            other => Err(format!("unknown Vivado TCL command: {other}")),
        }
    }

    fn cmd_create_project(&mut self, argv: &[String]) -> Result<String, String> {
        let mut name = String::new();
        let mut dir: Option<String> = None;
        let mut part: Option<String> = None;
        let mut in_memory = false;
        let mut i = 1;
        while i < argv.len() {
            match argv[i].as_str() {
                "-part" => {
                    if i + 1 < argv.len() {
                        part = Some(argv[i + 1].clone());
                        i += 2;
                        continue;
                    }
                }
                "-in_memory" => in_memory = true,
                "-force" | "-ip" => {}
                arg if !arg.starts_with('-') => {
                    if name.is_empty() {
                        name = arg.to_string();
                    } else if dir.is_none() {
                        dir = Some(arg.to_string());
                    }
                }
                _ => {}
            }
            i += 1;
        }
        if name.is_empty() {
            name = "project_1".into();
        }
        self.session.mode = if in_memory {
            Mode::NonProject
        } else {
            Mode::Project
        };
        self.project_name = name.clone();
        self.project.project_name = Some(name.clone());
        if let Some(d) = dir {
            let resolved = self.resolve_path(&d);
            self.project_dir = resolved;
            self.project.project_dir = Some(d);
        }
        if let Some(p) = part {
            self.project.part = p.clone();
            self.session.part = p;
        }
        Ok(format!(
            "create_project {} part={}",
            self.project_name, self.project.part
        ))
    }

    fn cmd_open_project(&mut self, argv: &[String]) -> Result<String, String> {
        let path_str = argv
            .iter()
            .skip(1)
            .find(|a| !a.starts_with('-'))
            .ok_or("open_project: missing project file path")?;
        let resolved = self.resolve_path(path_str);
        let text = std::fs::read_to_string(&resolved)
            .map_err(|e| format!("open_project {}: {e}", resolved.display()))?;
        let mut prj = load_prj(&text)?;
        let _ = expand_ip_packages(&mut prj, &resolved);
        if let Some(parent) = resolved.parent() {
            if !parent.as_os_str().is_empty() {
                self.project_dir = parent.to_path_buf();
            }
        }
        self.project_name = resolved
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("project_1")
            .to_string();
        self.project_file_path = Some(resolved.clone());
        self.session.mode = Mode::Project;
        self.session.part = prj.part.clone();
        self.generics = prj.generics.iter().cloned().collect();
        self.verilog_defines = prj.verilog_defines.iter().cloned().collect();
        self.synth_args = prj.synth_args.iter().cloned().collect();
        self.phys_opt_enabled = prj.phys_opt_enabled;
        if let Ok(xdc) = constraints_from_project(&prj, &resolved) {
            self.constraints = xdc;
        }
        self.project = prj;
        Ok(format!(
            "open_project {} part={} sources={}",
            self.project_name,
            self.project.part,
            self.project.sources.len()
        ))
    }

    fn cmd_save_project(&mut self, argv: &[String]) -> Result<String, String> {
        let name = argv
            .iter()
            .skip(1)
            .find(|a| !a.starts_with('-'))
            .cloned()
            .unwrap_or_else(|| format!("{}.prj", self.project_name));
        let path = self.resolve_path(&name);
        write_prj(&path, &self.project)?;
        self.project_file_path = Some(path.clone());
        Ok(format!("save_project_as {}", path.display()))
    }

    fn cmd_add_files(&mut self, argv: &[String]) -> Result<String, String> {
        let mut fileset = self.current_fileset.clone();
        let mut added = Vec::new();
        let mut i = 1;
        while i < argv.len() {
            match argv[i].as_str() {
                "-fileset" => {
                    if i + 1 < argv.len() {
                        fileset = argv[i + 1].clone();
                        i += 2;
                        continue;
                    }
                }
                "-norecurse" | "-quiet" | "-force" | "-scan_for_includes" => {}
                arg if !arg.starts_with('-') => {
                    for f in split_tcl_list(arg) {
                        if f.is_empty() {
                            continue;
                        }
                        let ext = Path::new(&f)
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        if fileset.contains("constr") || ext == "xdc" || ext == "sdc" {
                            let _ = self.load_xdc_file(&f);
                            if !self.project.constraint_files.contains(&f) {
                                self.project.constraint_files.push(f.clone());
                            }
                        } else if ext == "helion" || ext == "xci" {
                            if !self.project.ip_packages.contains(&f) {
                                self.project.ip_packages.push(f.clone());
                            }
                        } else if !self.project.sources.contains(&f) {
                            self.project.sources.push(f.clone());
                        }
                        added.push(f);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        Ok(added.join(" "))
    }

    fn cmd_read_hdl(&mut self, argv: &[String], _is_vhdl: bool) -> Result<String, String> {
        let mut added = Vec::new();
        let mut i = 1;
        while i < argv.len() {
            match argv[i].as_str() {
                "-library" | "-work" => {
                    i += 2;
                    continue;
                }
                "-sv" | "-vhdl2008" | "-quiet" | "-verbose" => {}
                arg if !arg.starts_with('-') => {
                    for f in split_tcl_list(arg) {
                        if !f.is_empty() {
                            if !self.project.sources.contains(&f) {
                                self.project.sources.push(f.clone());
                            }
                            added.push(f);
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
        Ok(added.join(" "))
    }

    fn load_xdc_file(&mut self, raw_path: &str) -> Result<(), String> {
        let resolved = self.resolve_path(raw_path);
        if resolved.exists() {
            let body = std::fs::read_to_string(&resolved)
                .map_err(|e| format!("read_xdc {}: {e}", resolved.display()))?;
            for line in body.lines() {
                let trimmed = line.split('#').next().unwrap_or("").trim();
                if !trimmed.is_empty() {
                    self.inline_xdc_lines.push(trimmed.to_string());
                }
            }
            self.rebuild_constraints()?;
        }
        Ok(())
    }

    fn cmd_read_xdc(&mut self, argv: &[String]) -> Result<String, String> {
        let mut added = Vec::new();
        let mut i = 1;
        while i < argv.len() {
            match argv[i].as_str() {
                "-ref" | "-cells" => {
                    i += 2;
                    continue;
                }
                "-unmanaged" | "-quiet" | "-verbose" | "-mode" => {}
                arg if !arg.starts_with('-') => {
                    for f in split_tcl_list(arg) {
                        if !f.is_empty() {
                            if !self.project.constraint_files.contains(&f) {
                                self.project.constraint_files.push(f.clone());
                            }
                            self.load_xdc_file(&f)?;
                            added.push(f);
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
        Ok(added.join(" "))
    }

    fn rebuild_constraints(&mut self) -> Result<(), String> {
        let mut blob = String::new();
        for l in &self.project.sdc {
            blob.push_str(l);
            blob.push('\n');
        }
        for l in &self.inline_xdc_lines {
            blob.push_str(l);
            blob.push('\n');
        }
        for (port, site) in &self.project.package_pins {
            blob.push_str(&format!(
                "set_property PACKAGE_PIN {site} [get_ports {port}]\n"
            ));
        }
        for (port, std) in &self.project.iostandards {
            blob.push_str(&format!(
                "set_property IOSTANDARD {std} [get_ports {port}]\n"
            ));
        }
        for (port, val) in &self.project.drives {
            blob.push_str(&format!("set_property DRIVE {val} [get_ports {port}]\n"));
        }
        for (port, val) in &self.project.slews {
            blob.push_str(&format!("set_property SLEW {val} [get_ports {port}]\n"));
        }
        for (port, val) in &self.project.pulltypes {
            blob.push_str(&format!(
                "set_property PULLTYPE {val} [get_ports {port}]\n"
            ));
        }
        for (port, val) in &self.project.diff_terms {
            blob.push_str(&format!(
                "set_property DIFF_TERM {val} [get_ports {port}]\n"
            ));
        }
        for (port, val) in &self.project.in_terms {
            blob.push_str(&format!("set_property IN_TERM {val} [get_ports {port}]\n"));
        }
        if !blob.trim().is_empty() {
            if let Ok(c) = load_xdc(&blob) {
                self.constraints = c;
            }
        }
        if let Some(d) = self.session.design.as_mut() {
            let _ = apply_design_xdc(d, &self.constraints);
        }
        Ok(())
    }

    fn cmd_inline_xdc(&mut self, argv: &[String]) -> Result<String, String> {
        let mut line = String::new();
        for (idx, tok) in argv.iter().enumerate() {
            if idx > 0 {
                line.push(' ');
            }
            line.push_str(tok);
        }
        if argv[0] == "create_clock" || argv[0] == "create_generated_clock" {
            self.project.sdc.push(line.clone());
        } else {
            self.inline_xdc_lines.push(line.clone());
        }
        self.rebuild_constraints()?;
        Ok(line)
    }

    fn cmd_pblock(&mut self, argv: &[String]) -> Result<String, String> {
        let line = argv.join(" ");
        let mut dummy = self.project.clone();
        if dummy.sources.is_empty() {
            dummy.sources.push("__dummy__.sv".into());
        }
        let mut text = format_prj(&dummy);
        text.push_str(&line);
        text.push('\n');
        if let Ok(parsed) = load_prj(&text) {
            self.project.pblocks = parsed.pblocks;
        }
        Ok(line)
    }

    fn cmd_set_property(&mut self, argv: &[String]) -> Result<String, String> {
        // Support both `set_property PROP VAL OBJS` and `set_property -name PROP -value VAL -objects OBJS`
        let mut prop = String::new();
        let mut val = String::new();
        let mut targets: Vec<String> = Vec::new();
        let mut pos = Vec::new();
        let mut i = 1;
        while i < argv.len() {
            match argv[i].as_str() {
                "-name" => {
                    if i + 1 < argv.len() {
                        prop = argv[i + 1].clone();
                        i += 2;
                        continue;
                    }
                }
                "-value" => {
                    if i + 1 < argv.len() {
                        val = argv[i + 1].clone();
                        i += 2;
                        continue;
                    }
                }
                "-objects" => {
                    if i + 1 < argv.len() {
                        targets.extend(split_tcl_list(&argv[i + 1]));
                        i += 2;
                        continue;
                    }
                }
                "-quiet" | "-dict" => {}
                arg => pos.push(arg.to_string()),
            }
            i += 1;
        }
        if prop.is_empty() && pos.len() >= 2 {
            prop = pos[0].clone();
            val = pos[1].clone();
            for t in pos.iter().skip(2) {
                targets.extend(split_tcl_list(t));
            }
        }
        if prop.is_empty() {
            return Err("set_property: missing property name".into());
        }
        let prop_upper = prop.to_ascii_uppercase();
        let first_target = targets.first().map(|s| s.as_str()).unwrap_or("");

        match prop_upper.as_str() {
            "TOP" => {
                self.project.top = Some(val.clone());
                return Ok(val);
            }
            "PART" | "TARGET_PART" => {
                self.project.part = val.clone();
                self.session.part = val.clone();
                return Ok(val);
            }
            "GENERIC" => {
                for item in split_tcl_list(&val) {
                    if let Some((k, v)) = item.split_once('=') {
                        let k = k.trim().to_string();
                        let v = v.trim().to_string();
                        self.generics.insert(k.clone(), v.clone());
                        if let Some(existing) =
                            self.project.generics.iter_mut().find(|(ek, _)| ek == &k)
                        {
                            existing.1 = v;
                        } else {
                            self.project.generics.push((k, v));
                        }
                    }
                }
                return Ok(val);
            }
            "VERILOG_DEFINE" => {
                for item in split_tcl_list(&val) {
                    let (k, v) = item
                        .split_once('=')
                        .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
                        .unwrap_or_else(|| (item.trim().to_string(), "1".into()));
                    self.verilog_defines.insert(k.clone(), v.clone());
                    if let Some(existing) = self
                        .project
                        .verilog_defines
                        .iter_mut()
                        .find(|(ek, _)| ek == &k)
                    {
                        existing.1 = v;
                    } else {
                        self.project.verilog_defines.push((k, v));
                    }
                }
                return Ok(val);
            }
            "STEPS.PHYS_OPT_DESIGN.IS_ENABLED"
            | "STEPS.POST_ROUTE_PHYS_OPT_DESIGN.IS_ENABLED" => {
                let enabled = matches!(
                    val.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                );
                self.phys_opt_enabled = enabled;
                self.project.phys_opt_enabled = enabled;
                let rname = if first_target.is_empty() {
                    "impl_1"
                } else {
                    first_target
                };
                self.run_props
                    .entry(rname.to_string())
                    .or_default()
                    .insert(prop_upper, val.clone());
                return Ok(val);
            }
            "STRATEGY" => {
                let strat = ImplStrategy::parse(&val)?;
                self.session.strategy = strat;
                let rname = if first_target.is_empty() {
                    "impl_1"
                } else {
                    first_target
                };
                if let Some(r) = self
                    .project
                    .impl_runs
                    .iter_mut()
                    .find(|r| r.name.eq_ignore_ascii_case(rname))
                {
                    r.strategy = strat.label().to_string();
                } else {
                    self.project.impl_runs.push(PrjImplRun {
                        name: rname.to_string(),
                        strategy: strat.label().to_string(),
                        spec: String::new(),
                    });
                }
                return Ok(strat.label().to_string());
            }
            "INCREMENTAL_CHECKPOINT" => {
                let p = self.resolve_path(&val);
                self.incremental_ref_path = Some(p);
                self.incremental_armed = true;
                self.project.incremental_checkpoint = Some(val.clone());
                return Ok(val);
            }
            _ if prop_upper.starts_with("STEPS.SYNTH_DESIGN.ARGS.") => {
                let arg_key = prop_upper
                    .trim_start_matches("STEPS.SYNTH_DESIGN.ARGS.")
                    .to_string();
                self.synth_args.insert(arg_key.clone(), val.clone());
                if let Some(existing) = self
                    .project
                    .synth_args
                    .iter_mut()
                    .find(|(k, _)| k == &arg_key)
                {
                    existing.1 = val.clone();
                } else {
                    self.project.synth_args.push((arg_key, val.clone()));
                }
                let rname = if first_target.is_empty() {
                    "synth_1"
                } else {
                    first_target
                };
                self.run_props
                    .entry(rname.to_string())
                    .or_default()
                    .insert(prop_upper, val.clone());
                return Ok(val);
            }
            _ if prop_upper.starts_with("STEPS.") => {
                let rname = if first_target.is_empty() {
                    "impl_1"
                } else {
                    first_target
                };
                self.run_props
                    .entry(rname.to_string())
                    .or_default()
                    .insert(prop_upper, val.clone());
                return Ok(val);
            }
            _ => {}
        }

        // Port / Cell / Net properties
        for target in &targets {
            let target = target.trim_matches(|c| c == '{' || c == '}' || c == '[' || c == ']');
            if target.is_empty() {
                continue;
            }
            match prop_upper.as_str() {
                "PACKAGE_PIN" | "LOC" => {
                    upsert_pair(&mut self.project.package_pins, target, &val);
                }
                "IOSTANDARD" => {
                    upsert_pair(&mut self.project.iostandards, target, &val);
                }
                "DRIVE" => {
                    upsert_pair(&mut self.project.drives, target, &val);
                }
                "SLEW" => {
                    upsert_pair(&mut self.project.slews, target, &val);
                }
                "PULLTYPE" => {
                    upsert_pair(&mut self.project.pulltypes, target, &val);
                }
                "DIFF_TERM" => {
                    upsert_pair(&mut self.project.diff_terms, target, &val);
                }
                "IN_TERM" => {
                    upsert_pair(&mut self.project.in_terms, target, &val);
                }
                "MARK_DEBUG" => {
                    if !self.project.mark_debug.contains(&target.to_string()) {
                        self.project.mark_debug.push(target.to_string());
                    }
                }
                _ => {}
            }
            if self.session.design.is_some() {
                let _ = self.session.set_property(&prop_upper, &val, target);
                if let Some(d) = self.session.design.as_mut() {
                    if d.net(target).is_some() {
                        let _ = d.set_net_attr(target, &prop_upper, val.clone());
                    } else if d.port_mut(target).is_some() {
                        let _ = d.set_port_attr(target, &prop_upper, val.clone());
                    }
                }
            }
        }
        self.rebuild_constraints()?;
        Ok(val)
    }

    fn cmd_get_property(&mut self, argv: &[String]) -> Result<String, String> {
        let pos: Vec<&String> = argv.iter().skip(1).filter(|a| !a.starts_with('-')).collect();
        if pos.len() < 2 {
            return Err("get_property: expected <property> <object>".into());
        }
        let prop = pos[0].to_ascii_uppercase();
        let obj = pos[1].as_str();
        match prop.as_str() {
            "TOP" => return Ok(self.project.top.clone().unwrap_or_default()),
            "PART" | "TARGET_PART" => return Ok(self.project.part.clone()),
            "GENERIC" => {
                let list: Vec<String> = self
                    .generics
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect();
                return Ok(list.join(" "));
            }
            "VERILOG_DEFINE" => {
                let list: Vec<String> = self
                    .verilog_defines
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect();
                return Ok(list.join(" "));
            }
            "STEPS.PHYS_OPT_DESIGN.IS_ENABLED" => {
                return Ok(if self.phys_opt_enabled { "1" } else { "0" }.into());
            }
            "STRATEGY" => {
                if let Some(r) = self
                    .project
                    .impl_runs
                    .iter()
                    .find(|r| r.name.eq_ignore_ascii_case(obj))
                {
                    return Ok(r.strategy.clone());
                }
                return Ok(self.session.strategy.label().to_string());
            }
            _ => {}
        }
        if let Some(arg_key) = prop.strip_prefix("STEPS.SYNTH_DESIGN.ARGS.") {
            if let Some(v) = self.synth_args.get(arg_key) {
                return Ok(v.clone());
            }
        }
        if let Some(rmap) = self.run_props.get(obj) {
            if let Some(v) = rmap.get(&prop) {
                return Ok(v.clone());
            }
        }
        if let Some(d) = &self.session.design {
            if let Some(c) = d.cell(obj) {
                let props = cell_properties(d, c);
                if let Some(v) = props.get(&prop) {
                    return Ok(v.clone());
                }
            }
            if let Some(n) = d.net(obj) {
                let props = net_properties(n);
                if let Some(v) = props.get(&prop) {
                    return Ok(v.clone());
                }
            }
            if let Some(p) = d.ports.iter().find(|p| resolve(p.name) == obj) {
                let props = port_properties(p);
                if let Some(v) = props.get(&prop) {
                    return Ok(v.clone());
                }
            }
        }
        Ok(String::new())
    }

    fn cmd_get_ports(&mut self, argv: &[String]) -> Result<String, String> {
        let q = parse_query_args(argv);
        let Some(d) = &self.session.design else {
            return Ok(q.patterns.join(" "));
        };
        let mut out = Vec::new();
        let of_list = q.of_objects.as_deref().map(split_tcl_list);
        for p in &d.ports {
            let pname = resolve(p.name).to_string();
            if let Some(objs) = &of_list {
                if !objs.iter().any(|o| o == &pname) {
                    continue;
                }
            }
            if !match_patterns(&pname, &q.patterns) {
                continue;
            }
            if let Some(f) = &q.filter {
                let props = port_properties(p);
                if !eval_tcl_filter(f, &props) {
                    continue;
                }
            }
            out.push(pname);
        }
        if out.is_empty() && q.filter.is_none() && q.of_objects.is_none() && !q.patterns.is_empty() {
            // Pre-elaboration XDC fallback so `get_ports clk` works before synth_design
            let has_glob = q.patterns.iter().any(|p| p.contains('*') || p.contains('?'));
            if !has_glob {
                return Ok(q.patterns.join(" "));
            }
        }
        Ok(out.join(" "))
    }

    fn cmd_get_cells(&mut self, argv: &[String]) -> Result<String, String> {
        let q = parse_query_args(argv);
        let Some(d) = &self.session.design else {
            return Ok(q.patterns.join(" "));
        };
        let of_cells: Option<Vec<String>> = q.of_objects.as_ref().map(|of_s| {
            let mut acc = Vec::new();
            for o in split_tcl_list(of_s) {
                if let Some((cell, _pin)) = o.split_once('/') {
                    acc.push(cell.to_string());
                } else if let Some(net) = d.net(&o) {
                    for ep in &net.endpoints {
                        acc.push(resolve(ep.cell).to_string());
                    }
                } else {
                    acc.push(o);
                }
            }
            acc
        });
        let mut out = Vec::new();
        for c in &d.cells {
            let cname = resolve(c.name).to_string();
            if let Some(allowed) = &of_cells {
                if !allowed.iter().any(|a| a == &cname) {
                    continue;
                }
            }
            if !match_patterns(&cname, &q.patterns) {
                continue;
            }
            if let Some(f) = &q.filter {
                let props = cell_properties(d, c);
                if !eval_tcl_filter(f, &props) {
                    continue;
                }
            }
            out.push(cname);
        }
        Ok(out.join(" "))
    }

    fn cmd_get_nets(&mut self, argv: &[String]) -> Result<String, String> {
        let q = parse_query_args(argv);
        let Some(d) = &self.session.design else {
            return Ok(q.patterns.join(" "));
        };
        let of_nets: Option<Vec<String>> = q.of_objects.as_ref().map(|of_s| {
            let pins = d.pin_index();
            let mut acc = Vec::new();
            for o in split_tcl_list(of_s) {
                if let Some((cell, pin)) = o.split_once('/') {
                    if let Some(n) = pins.net_on(cell, pin) {
                        acc.push(resolve(n).to_string());
                    }
                } else if d.cell(&o).is_some() {
                    for pin in [
                        "I0", "I1", "I2", "I3", "I4", "I5", "O", "D", "Q", "CLK", "I", "PAD",
                    ] {
                        if let Some(n) = pins.net_on(&o, pin) {
                            let ns = resolve(n).to_string();
                            if !acc.contains(&ns) {
                                acc.push(ns);
                            }
                        }
                    }
                } else {
                    acc.push(o);
                }
            }
            acc
        });
        let mut out = Vec::new();
        for n in &d.nets {
            let nname = resolve(n.name).to_string();
            if let Some(allowed) = &of_nets {
                if !allowed.iter().any(|a| a == &nname) {
                    continue;
                }
            }
            if !match_patterns(&nname, &q.patterns) {
                continue;
            }
            if let Some(f) = &q.filter {
                let props = net_properties(n);
                if !eval_tcl_filter(f, &props) {
                    continue;
                }
            }
            out.push(nname);
        }
        Ok(out.join(" "))
    }

    fn cmd_get_pins(&mut self, argv: &[String]) -> Result<String, String> {
        let q = parse_query_args(argv);
        let Some(d) = &self.session.design else {
            return Ok(q.patterns.join(" "));
        };
        let mut candidates: Vec<(Cell, String)> = Vec::new();
        let pin_idx = d.pin_index();
        let all_pin_names = [
            "I0", "I1", "I2", "I3", "I4", "I5", "O", "D", "Q", "CLK", "I", "PAD",
        ];
        if let Some(of_s) = &q.of_objects {
            for o in split_tcl_list(of_s) {
                if let Some(c) = d.cell(&o) {
                    let cname = resolve(c.name);
                    for &p in &all_pin_names {
                        if pin_idx.net_on(cname, p).is_some() {
                            candidates.push((c.clone(), p.to_string()));
                        }
                    }
                } else if let Some(net) = d.net(&o) {
                    for ep in &net.endpoints {
                        if let Some(c) = d.cell(resolve(ep.cell)) {
                            candidates.push((c.clone(), resolve(ep.pin).to_string()));
                        }
                    }
                }
            }
        } else {
            for c in &d.cells {
                let cname = resolve(c.name);
                for &p in &all_pin_names {
                    if pin_idx.net_on(cname, p).is_some() {
                        candidates.push((c.clone(), p.to_string()));
                    }
                }
            }
        }
        let mut out = Vec::new();
        for (cell, pin) in candidates {
            let full = format!("{}/{pin}", resolve(cell.name));
            if !match_patterns(&full, &q.patterns) {
                continue;
            }
            if let Some(f) = &q.filter {
                let props = pin_properties(d, &cell, &pin);
                if !eval_tcl_filter(f, &props) {
                    continue;
                }
            }
            if !out.contains(&full) {
                out.push(full);
            }
        }
        Ok(out.join(" "))
    }

    fn cmd_get_clocks(&mut self, argv: &[String]) -> Result<String, String> {
        let q = parse_query_args(argv);
        let mut clks = self.constraints.clocks.clone();
        if clks.is_empty() && self.session.design.is_some() {
            create_clock(&mut clks, "clk", 10_000, "clk");
        }
        let mut out = Vec::new();
        for c in &clks {
            if !match_patterns(&c.name, &q.patterns) {
                continue;
            }
            if let Some(f) = &q.filter {
                let mut props = BTreeMap::new();
                props.insert("NAME".into(), c.name.clone());
                props.insert("CLASS".into(), "clock".into());
                props.insert("PERIOD".into(), format!("{:.3}", c.period_ps as f64 / 1000.0));
                props.insert("IS_GENERATED".into(), if c.generated { "1" } else { "0" }.into());
                if !eval_tcl_filter(f, &props) {
                    continue;
                }
            }
            out.push(c.name.clone());
        }
        Ok(out.join(" "))
    }

    fn cmd_all_inputs(&mut self, _argv: &[String]) -> Result<String, String> {
        let Some(d) = &self.session.design else {
            return Ok(String::new());
        };
        Ok(all_inputs(d).join(" "))
    }

    fn cmd_all_outputs(&mut self, _argv: &[String]) -> Result<String, String> {
        let Some(d) = &self.session.design else {
            return Ok(String::new());
        };
        Ok(all_outputs(d).join(" "))
    }

    fn cmd_all_registers(&mut self, argv: &[String]) -> Result<String, String> {
        let q = parse_query_args(argv);
        let Some(d) = &self.session.design else {
            return Ok(String::new());
        };
        let regs = all_registers(d);
        if q.data_pins {
            return Ok(regs.into_iter().map(|r| format!("{r}/D")).collect::<Vec<_>>().join(" "));
        }
        if q.clock_pins {
            return Ok(regs.into_iter().map(|r| format!("{r}/CLK")).collect::<Vec<_>>().join(" "));
        }
        if q.output_pins {
            return Ok(regs.into_iter().map(|r| format!("{r}/Q")).collect::<Vec<_>>().join(" "));
        }
        if q.async_pins {
            return Ok(Vec::<String>::new().join(" "));
        }
        Ok(regs.join(" "))
    }
}

fn upsert_pair(vec: &mut Vec<(String, String)>, key: &str, val: &str) {
    if let Some(existing) = vec.iter_mut().find(|(k, _)| k == key) {
        existing.1 = val.to_string();
    } else {
        vec.push((key.to_string(), val.to_string()));
    }
}

impl VivadoTclSession {
    fn cmd_synth_design(&mut self, argv: &[String]) -> Result<String, String> {
        let mut top = self.project.top.clone();
        let mut part = self.project.part.clone();
        let mut directive = self
            .synth_args
            .get("DIRECTIVE")
            .cloned()
            .unwrap_or_else(|| "Default".into());
        let mut i = 1;
        while i < argv.len() {
            match argv[i].as_str() {
                "-top" => {
                    if i + 1 < argv.len() {
                        top = Some(argv[i + 1].clone());
                        self.project.top = top.clone();
                        i += 2;
                        continue;
                    }
                }
                "-part" => {
                    if i + 1 < argv.len() {
                        part = argv[i + 1].clone();
                        self.project.part = part.clone();
                        self.session.part = part.clone();
                        i += 2;
                        continue;
                    }
                }
                "-generic" => {
                    if i + 1 < argv.len() {
                        for item in split_tcl_list(&argv[i + 1]) {
                            if let Some((k, v)) = item.split_once('=') {
                                self.generics
                                    .insert(k.trim().to_string(), v.trim().to_string());
                                upsert_pair(
                                    &mut self.project.generics,
                                    k.trim(),
                                    v.trim(),
                                );
                            }
                        }
                        i += 2;
                        continue;
                    }
                }
                "-verilog_define" => {
                    if i + 1 < argv.len() {
                        for item in split_tcl_list(&argv[i + 1]) {
                            let (k, v) = item
                                .split_once('=')
                                .map(|(a, b)| (a.trim(), b.trim()))
                                .unwrap_or((item.trim(), "1"));
                            self.verilog_defines.insert(k.to_string(), v.to_string());
                            upsert_pair(&mut self.project.verilog_defines, k, v);
                        }
                        i += 2;
                        continue;
                    }
                }
                "-directive" => {
                    if i + 1 < argv.len() {
                        directive = argv[i + 1].clone();
                        self.synth_args
                            .insert("DIRECTIVE".into(), directive.clone());
                        i += 2;
                        continue;
                    }
                }
                "-flatten_hierarchy" | "-fsm_extraction" | "-mode" => {
                    if i + 1 < argv.len() {
                        let k = argv[i].trim_start_matches('-').to_ascii_uppercase();
                        self.synth_args.insert(k, argv[i + 1].clone());
                        i += 2;
                        continue;
                    }
                }
                "-retiming" => {
                    self.synth_args.insert("RETIMING".into(), "true".into());
                }
                _ => {}
            }
            i += 1;
        }

        if !part.is_empty() {
            self.session.part = part;
        }

        if !self.project.ip_packages.is_empty() {
            let anchor = self
                .project_file_path
                .clone()
                .unwrap_or_else(|| self.project_dir.join("project.prj"));
            let _ = expand_ip_packages(&mut self.project, &anchor);
        }

        let mut design = if !self.project.sources.is_empty() {
            self.synthesize_loaded_sources(top.as_deref())?
        } else if let Some(d) = self.session.design.clone() {
            d
        } else {
            return Err("synth_design: no HDL sources loaded (use read_verilog / read_vhdl / add_files)".into());
        };

        if let Some(t) = &top {
            design.name = intern(t);
        }
        self.rebuild_constraints()?;
        apply_design_xdc(&mut design, &self.constraints)?;
        for net in &self.project.mark_debug {
            let _ = design.mark_debug(net);
        }
        if directive.to_ascii_lowercase().contains("area") {
            let _ = opt_design(&mut design);
        }
        let cells = design.cells.len();
        let nets = design.nets.len();
        let dname = resolve(design.name).to_string();
        self.session.synth_design(design);
        Ok(format!(
            "synth_design top={dname} part={} cells={cells} nets={nets} directive={directive}",
            self.session.part
        ))
    }

    fn synthesize_loaded_sources(&self, top: Option<&str>) -> Result<Design, String> {
        let mut sv_units: Vec<(String, String)> = Vec::new();
        // Prepend any verilog_define macros as `define lines
        let mut define_header = String::new();
        for (k, v) in &self.verilog_defines {
            if v.is_empty() {
                define_header.push_str(&format!("`define {k}\n"));
            } else {
                define_header.push_str(&format!("`define {k} {v}\n"));
            }
        }
        for src_ref in &self.project.sources {
            let path = self.resolve_path(src_ref);
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| format!("synth_design read {}: {e}", path.display()))?;
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("sv")
                .to_ascii_lowercase();
            if ext == "hnf" {
                return Design::from_hnf(&raw);
            }
            let sv_text = if ext == "vhd" || ext == "vhdl" {
                translate_vhdl_to_sv(&raw)?
            } else {
                raw
            };
            let full_sv = if define_header.is_empty() {
                sv_text
            } else {
                format!("{define_header}{sv_text}")
            };
            sv_units.push((path.display().to_string(), full_sv));
        }

        let mut params: HashMap<String, u128> = HashMap::new();
        for (k, v) in &self.generics {
            let clean = v.trim().trim_start_matches("0x").trim_start_matches("0X");
            if let Ok(n) = v.trim().parse::<u128>() {
                params.insert(k.clone(), n);
            } else if let Ok(n) = u128::from_str_radix(clean, 16) {
                params.insert(k.clone(), n);
            }
        }
        let opts = SvCompileOpts {
            defines: self
                .verilog_defines
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            include_paths: vec![self.project_dir.display().to_string()],
        };
        let refs: Vec<(&str, &str)> = sv_units
            .iter()
            .map(|(o, s)| (o.as_str(), s.as_str()))
            .collect();
        let (d, _) = elaborate_sv_sources(&refs, top, &params, &opts)?;
        Ok(d)
    }

    fn cmd_opt_design(&mut self, argv: &[String]) -> Result<String, String> {
        let mut directive = "Default".to_string();
        let mut i = 1;
        while i < argv.len() {
            if argv[i] == "-directive" && i + 1 < argv.len() {
                directive = argv[i + 1].clone();
                i += 2;
                continue;
            }
            i += 1;
        }
        let removed = self.session.opt_design_step()?;
        Ok(format!("opt_design directive={directive} removed_lutff={removed}"))
    }

    fn cmd_place_design(&mut self, argv: &[String]) -> Result<String, String> {
        let mut directive = String::new();
        let mut no_timing = false;
        let mut i = 1;
        while i < argv.len() {
            match argv[i].as_str() {
                "-directive" => {
                    if i + 1 < argv.len() {
                        directive = argv[i + 1].clone();
                        i += 2;
                        continue;
                    }
                }
                "-no_timing_driven" => no_timing = true,
                _ => {}
            }
            i += 1;
        }
        let dev = self.device()?;
        if let Some(d) = self.session.design.as_mut() {
            let _ = apply_design_xdc(d, &self.constraints);
        }

        // If `read_checkpoint -incremental` armed a reference checkpoint, perform guided reuse
        if self.incremental_armed {
            if let Some(prev) = self.session.impl_checkpoint.clone() {
                let rep = self.session.incremental_place(&dev, &prev)?;
                self.last_reuse_report = Some(rep.clone());
                return Ok(format!("place_design incremental {}", rep.text()));
            }
        }

        if !directive.is_empty() {
            if let Ok(strat) = ImplStrategy::parse(&directive) {
                self.session.strategy = strat;
            }
        }
        let mut opts = self.session.strategy.place_opts();
        if no_timing {
            opts.timing_weight = 0.0;
        }
        if let Some(pb) = self.project.pblocks.iter().find(|p| p.ranged).cloned() {
            self.session.place_pblock_in(&dev, &pb, opts)?;
        } else {
            self.session.place_design_with(&dev, opts)?;
        }
        let sites = self
            .session
            .placed
            .as_ref()
            .map(|p| p.lutff_sites.len())
            .unwrap_or(0);
        Ok(format!(
            "place_design strategy={} sites={sites}",
            self.session.strategy.label()
        ))
    }

    fn cmd_phys_opt_design(&mut self, argv: &[String]) -> Result<String, String> {
        let mut directive = "Default".to_string();
        let mut i = 1;
        while i < argv.len() {
            if argv[i] == "-directive" && i + 1 < argv.len() {
                directive = argv[i + 1].clone();
                i += 2;
                continue;
            }
            i += 1;
        }
        let dev = self.device()?;
        let res = self.session.phys_opt_design(&dev)?;
        Ok(format!("{res} directive={directive}"))
    }

    fn cmd_route_design(&mut self, argv: &[String]) -> Result<String, String> {
        let mut directive = String::new();
        let mut i = 1;
        while i < argv.len() {
            if argv[i] == "-directive" && i + 1 < argv.len() {
                directive = argv[i + 1].clone();
                i += 2;
                continue;
            }
            i += 1;
        }
        let dev = self.device()?;
        if self.session.placed.is_none() {
            self.session.place_design(&dev)?;
        }
        if !directive.is_empty() {
            if let Ok(strat) = ImplStrategy::parse(&directive) {
                self.session.strategy = strat;
            }
        }
        let mut ropts = self.session.strategy.route_opts();
        if self.phys_opt_enabled && ropts.extra_hops == 0 {
            ropts.extra_hops = 8;
        }
        self.session.route_design_with(&dev, ropts)?;
        let hops = self
            .session
            .routed
            .as_ref()
            .and_then(|r| r.iob_src.first())
            .map(|io| io.hops)
            .unwrap_or(0);
        Ok(format!(
            "route_design strategy={} hops={hops}",
            self.session.strategy.label()
        ))
    }

    fn cmd_write_bitstream(&mut self, argv: &[String]) -> Result<String, String> {
        let mut bin_file = false;
        let mut file_arg: Option<String> = None;
        for arg in argv.iter().skip(1) {
            if arg == "-bin_file" {
                bin_file = true;
            } else if !arg.starts_with('-') {
                file_arg = Some(arg.clone());
            }
        }
        let dev = self.device()?;
        if self.session.placed.is_none() {
            self.session.place_design(&dev)?;
        }
        if self.session.routed.is_none() {
            self.session.route_design(&dev)?;
        }
        let bits = self.session.write_bitstream(&dev)?.clone();
        if let Some(f) = file_arg {
            let path = self.resolve_path(&f);
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("write_bitstream mkdir: {e}"))?;
                }
            }
            std::fs::write(&path, &bits.packets)
                .map_err(|e| format!("write_bitstream {}: {e}", path.display()))?;
            if bin_file {
                let bin_path = path.with_extension("bin");
                std::fs::write(&bin_path, &bits.packets)
                    .map_err(|e| format!("write_bitstream {}: {e}", bin_path.display()))?;
            }
            return Ok(format!(
                "write_bitstream {} bytes={} frames={}",
                path.display(),
                bits.packets.len(),
                bits.frames.len()
            ));
        }
        Ok(format!(
            "write_bitstream bytes={} frames={}",
            bits.packets.len(),
            bits.frames.len()
        ))
    }

    fn cmd_write_checkpoint(&mut self, argv: &[String]) -> Result<String, String> {
        let file_arg = argv
            .iter()
            .skip(1)
            .find(|a| !a.starts_with('-'))
            .cloned();
        if let Some(f) = file_arg {
            let path = self.resolve_path(&f);
            self.project.checkpoint_path = Some(f.clone());
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("dcp")
                .to_ascii_lowercase();
            if ext == "hckp" && self.session.bitstream.is_some() {
                return self.session.write_checkpoint_to(&path);
            }
            let meta = self.session.write_dcp(&path, &self.constraints)?;
            return Ok(format!(
                "write_checkpoint {} stage={} bytes={} sha256={}",
                path.display(),
                meta.stage.label(),
                meta.bytes,
                meta.netlist_sha256
            ));
        }
        self.session.write_checkpoint()
    }

    fn cmd_read_checkpoint(&mut self, argv: &[String]) -> Result<String, String> {
        let mut incremental = false;
        let mut file_arg: Option<String> = None;
        for arg in argv.iter().skip(1) {
            if arg == "-incremental" || arg == "-inc" {
                incremental = true;
            } else if !arg.starts_with('-') {
                file_arg = Some(arg.clone());
            }
        }
        let file_str = file_arg.ok_or("read_checkpoint: missing checkpoint path")?;
        let path = self.resolve_path(&file_str);
        let dev = self.device()?;

        if incremental {
            let rep = self.session.read_checkpoint_incremental(&path, &dev)?;
            self.incremental_ref_path = Some(path.clone());
            self.incremental_armed = true;
            self.last_reuse_report = Some(rep.clone());
            return Ok(format!(
                "read_checkpoint -incremental {} {}",
                path.display(),
                rep.text()
            ));
        }

        let bytes = std::fs::read(&path)
            .map_err(|e| format!("read_checkpoint {}: {e}", path.display()))?;
        if bytes.starts_with(b"HCK2") {
            let rc = load_checkpoint(&path, &dev)?;
            self.session = rc.session;
            self.constraints = rc.constraints;
            return Ok(format!(
                "read_checkpoint {} stage={} sha256={}",
                path.display(),
                rc.meta.stage.label(),
                rc.meta.netlist_sha256
            ));
        }
        let s = Session::read_checkpoint(&path, &dev)?;
        let sha = s
            .design
            .as_ref()
            .map(netlist_sha256)
            .unwrap_or_default();
        self.session = s;
        Ok(format!("read_checkpoint {} sha256={sha}", path.display()))
    }

    fn cmd_create_run(&mut self, argv: &[String]) -> Result<String, String> {
        let mut name = String::new();
        let mut strategy = String::from("Default");
        let mut spec: Option<String> = None;
        let mut i = 1;
        while i < argv.len() {
            match argv[i].as_str() {
                "-strategy" | "-strat" => {
                    if i + 1 < argv.len() {
                        strategy = argv[i + 1].clone();
                        i += 2;
                        continue;
                    }
                }
                "-spec" => {
                    if i + 1 < argv.len() {
                        spec = Some(argv[i + 1].clone());
                        i += 2;
                        continue;
                    }
                }
                "-flow" | "-parent_run" | "-constrset" => {
                    i += 2;
                    continue;
                }
                arg if !arg.starts_with('-') && name.is_empty() => {
                    name = arg.to_string();
                }
                _ => {}
            }
            i += 1;
        }
        if name.is_empty() {
            return Err("create_run: missing run name".into());
        }
        if !name.to_ascii_lowercase().starts_with("synth") {
            let parsed = ImplStrategy::parse(&strategy)?;
            strategy = parsed.label().to_string();
        }
        if let Some(r) = self
            .project
            .impl_runs
            .iter_mut()
            .find(|r| r.name.eq_ignore_ascii_case(&name))
        {
            r.strategy = strategy.clone();
            if let Some(sp) = &spec {
                r.spec = sp.clone();
            }
        } else {
            self.project.impl_runs.push(PrjImplRun {
                name: name.clone(),
                strategy: strategy.clone(),
                spec: spec.unwrap_or_default(),
            });
        }
        Ok(format!("create_run {name} strategy={strategy}"))
    }

    fn cmd_launch_runs(&mut self, argv: &[String]) -> Result<String, String> {
        let mut run_names: Vec<String> = Vec::new();
        let mut to_step: Option<String> = None;
        let mut i = 1;
        while i < argv.len() {
            match argv[i].as_str() {
                "-jobs" | "-lsf" | "-sge" => {
                    i += 2;
                    continue;
                }
                "-to_step" => {
                    if i + 1 < argv.len() {
                        to_step = Some(argv[i + 1].clone());
                        i += 2;
                        continue;
                    }
                }
                "-next_step" | "-scripts_only" => {}
                arg if !arg.starts_with('-') => {
                    for r in split_tcl_list(arg) {
                        if !r.is_empty() {
                            run_names.push(r);
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
        if run_names.is_empty() {
            run_names.push("impl_1".into());
        }

        if (self.session.design.is_none()
            || run_names
                .iter()
                .any(|r| r.to_ascii_lowercase().starts_with("synth")))
            && !self.project.sources.is_empty()
        {
            self.cmd_synth_design(&["synth_design".into()])?;
        }

        let impl_targets: Vec<&str> = run_names
            .iter()
            .map(|s| s.as_str())
            .filter(|r| !r.to_ascii_lowercase().starts_with("synth"))
            .collect();
        if impl_targets.is_empty() {
            return Ok("launch_runs synth_1 Complete".into());
        }

        let dev = self.device()?;
        let results = crate::qor::launch_project_runs(
            &mut self.session,
            &self.project,
            &dev,
            &self.constraints,
            &impl_targets,
        )?;
        if let Some(step) = &to_step {
            if step != "write_bitstream" && self.session.bitstream.is_none() {
                let _ = self.session.write_bitstream(&dev);
            }
        }
        let summary: Vec<String> = results
            .iter()
            .map(|r| {
                format!(
                    "{}:strategy={},WNS_PS={},LUT={}",
                    r.run_name,
                    r.strategy.label(),
                    r.wns_ps,
                    r.luts
                )
            })
            .collect();
        Ok(format!("launch_runs {}", summary.join(" ")))
    }

    fn maybe_write_report_file(&self, argv: &[String], text: &str) -> Result<(), String> {
        let mut i = 1;
        while i < argv.len() {
            if argv[i] == "-file" && i + 1 < argv.len() {
                let path = self.resolve_path(&argv[i + 1]);
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| format!("report -file mkdir: {e}"))?;
                    }
                }
                std::fs::write(&path, text)
                    .map_err(|e| format!("report -file {}: {e}", path.display()))?;
                break;
            }
            i += 1;
        }
        Ok(())
    }

    fn cmd_report_timing_summary(&mut self, argv: &[String]) -> Result<String, String> {
        let dev = self.device()?;
        let text = report_timing_summary_text(&self.session, &dev, &self.constraints)?;
        self.maybe_write_report_file(argv, &text)?;
        Ok(text)
    }

    fn cmd_report_timing(&mut self, argv: &[String]) -> Result<String, String> {
        let mut max_paths = 1usize;
        let mut delay_type = "max".to_string();
        let mut from = None;
        let mut to = None;
        let mut through = None;
        let mut i = 1;
        while i < argv.len() {
            match argv[i].as_str() {
                "-max_paths" | "-nworst" => {
                    if i + 1 < argv.len() {
                        max_paths = argv[i + 1].parse().unwrap_or(1);
                        i += 2;
                        continue;
                    }
                }
                "-delay_type" => {
                    if i + 1 < argv.len() {
                        delay_type = argv[i + 1].clone();
                        i += 2;
                        continue;
                    }
                }
                "-from" => {
                    if i + 1 < argv.len() {
                        from = Some(argv[i + 1].clone());
                        i += 2;
                        continue;
                    }
                }
                "-to" => {
                    if i + 1 < argv.len() {
                        to = Some(argv[i + 1].clone());
                        i += 2;
                        continue;
                    }
                }
                "-through" => {
                    if i + 1 < argv.len() {
                        through = Some(argv[i + 1].clone());
                        i += 2;
                        continue;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let dev = self.device()?;
        let text = report_timing_detailed(
            &self.session,
            &dev,
            &self.constraints,
            max_paths,
            &delay_type,
            from.as_deref(),
            to.as_deref(),
            through.as_deref(),
        )?;
        self.maybe_write_report_file(argv, &text)?;
        Ok(text)
    }

    fn cmd_report_utilization(&mut self, argv: &[String]) -> Result<String, String> {
        let hier = argv.iter().any(|a| a == "-hierarchical" || a == "-hier");
        let dev = self.device()?;
        let text = report_utilization_detailed(&self.session, &dev, hier)?;
        self.maybe_write_report_file(argv, &text)?;
        Ok(text)
    }

    fn cmd_report_power(&mut self, argv: &[String]) -> Result<String, String> {
        let dev = self.device()?;
        let text = report_power_detailed(&self.session, &dev, &self.constraints)?;
        self.maybe_write_report_file(argv, &text)?;
        Ok(text)
    }

    fn cmd_report_drc(&mut self, argv: &[String]) -> Result<String, String> {
        let dev = self.device()?;
        let text = report_drc_detailed(&self.session, &dev, &self.constraints)?;
        self.maybe_write_report_file(argv, &text)?;
        Ok(text)
    }

    fn cmd_report_cdc(&mut self, argv: &[String]) -> Result<String, String> {
        let text = report_cdc_detailed(&self.session, &self.constraints)?;
        self.maybe_write_report_file(argv, &text)?;
        Ok(text)
    }

    fn cmd_report_clock_interaction(&mut self, argv: &[String]) -> Result<String, String> {
        let text = report_clock_interaction_detailed(&self.session, &self.constraints)?;
        self.maybe_write_report_file(argv, &text)?;
        Ok(text)
    }

    fn cmd_report_clock_utilization(&mut self, argv: &[String]) -> Result<String, String> {
        let dev = self.device()?;
        let text = report_clock_utilization_detailed(&self.session, &dev, &self.constraints)?;
        self.maybe_write_report_file(argv, &text)?;
        Ok(text)
    }

    fn cmd_report_high_fanout_nets(&mut self, argv: &[String]) -> Result<String, String> {
        let mut max_nets = 10usize;
        let mut min_fanout = 0usize;
        let mut i = 1;
        while i < argv.len() {
            match argv[i].as_str() {
                "-max_nets" => {
                    if i + 1 < argv.len() {
                        max_nets = argv[i + 1].parse().unwrap_or(10);
                        i += 2;
                        continue;
                    }
                }
                "-fanout_greater_than" => {
                    if i + 1 < argv.len() {
                        min_fanout = argv[i + 1].parse::<usize>().unwrap_or(0) + 1;
                        i += 2;
                        continue;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let text = report_high_fanout_nets_detailed(&self.session, max_nets, min_fanout)?;
        self.maybe_write_report_file(argv, &text)?;
        Ok(text)
    }

    fn cmd_report_design_analysis(&mut self, argv: &[String]) -> Result<String, String> {
        let dev = self.device()?;
        let text = report_design_analysis_detailed(&self.session, &dev, &self.constraints)?;
        self.maybe_write_report_file(argv, &text)?;
        Ok(text)
    }

    fn cmd_report_qor_suggestions(&mut self, argv: &[String]) -> Result<String, String> {
        let dev = self.device()?;
        let rep = report_qor_suggestions(&self.session, &dev, &self.constraints)?;
        let text = rep.text();
        self.maybe_write_report_file(argv, &text)?;
        Ok(text)
    }
}

/// Execute a Vivado batch or project-mode TCL script and return the resulting [`VivadoTclSession`].
pub fn run_vivado_tcl(
    script: &str,
    base_dir: Option<&Path>,
) -> Result<VivadoTclSession, String> {
    let mut tcl = VivadoTclSession::new(Mode::NonProject);
    if let Some(dir) = base_dir {
        tcl = tcl.with_base_dir(dir);
    }
    tcl.eval_script(script)?;
    Ok(tcl)
}
