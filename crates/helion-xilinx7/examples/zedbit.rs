//! Build a Zedboard (xc7z020clg484-1) bitstream from HDL with Helion's own
//! flow: synthesis (SystemVerilog or VHDL) -> pack -> place -> route ->
//! FASM -> `.bit`.
//!
//! ```text
//! cargo run --release -p helion-xilinx7 --example zedbit -- design.vhd|.sv board.xdc out.bit [top]
//! ```
//!
//! Pins come from the XDC (`PACKAGE_PIN`, `IOSTANDARD`, including `-dict`
//! form and multi-port / wildcard `get_ports`). Every top-level port bit
//! must have a pin; a missing pin is an error, never a silent default.
//! The FASM is written next to the bitstream (`out.fasm`).

use std::collections::{BTreeMap, HashMap};

use helion_ir::{CellKind, Design};
use helion_xilinx7::backend::Xilinx7Backend;
use helion_backend::Backend;

fn synth(path: &str, top: Option<&str>) -> Result<Design, String> {
    helion_sv::set_dsp_inference(false);
    // Adders, subtractors and comparators go on the dedicated carry chains.
    helion_vhdl::set_carry_arith(std::env::var("HELION_NO_CARRY").is_err());
    let src = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    if path.ends_with(".vhd") || path.ends_with(".vhdl") {
        helion_vhdl::synth_vhdl_top(&src, top)
    } else {
        helion_sv::synth_sv(&src, path)
    }
}

/// XDC properties per port-bit key (`clk`, `led[3]`).
fn parse_xdc(text: &str) -> Vec<(String, String, String)> {
    // (property, value, port pattern)
    let mut out = Vec::new();
    let mut stmt = String::new();
    let mut stmts: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        stmt.push_str(line);
        if stmt.ends_with('\\') {
            stmt.pop();
            stmt.push(' ');
            continue;
        }
        // Several commands may share a line, separated by `;`.
        for part in std::mem::take(&mut stmt).split(';') {
            if !part.trim().is_empty() {
                stmts.push(part.trim().to_string());
            }
        }
    }
    for s in stmts {
        if !s.starts_with("set_property") {
            continue;
        }
        let Some(gp) = s.find("[get_ports") else { continue };
        let head = s["set_property".len()..gp].trim();
        let ports_part = s[gp + "[get_ports".len()..].trim();
        let ports_part = ports_part.trim_end_matches(']').trim().trim_start_matches('{').trim_end_matches('}').trim();
        let ports: Vec<String> = ports_part.split_whitespace().map(|p| p.trim_matches(|c| c == '{' || c == '}').to_string()).collect();
        let mut props: Vec<(String, String)> = Vec::new();
        if let Some(rest) = head.strip_prefix("-dict") {
            let inner = rest.trim().trim_start_matches('{').trim_end_matches('}');
            let toks: Vec<&str> = inner.split_whitespace().collect();
            for kv in toks.chunks(2) {
                if kv.len() == 2 {
                    props.push((kv[0].to_string(), kv[1].to_string()));
                }
            }
        } else {
            let toks: Vec<&str> = head.split_whitespace().collect();
            if toks.len() >= 2 {
                props.push((toks[0].to_string(), toks[1].to_string()));
            }
        }
        for (k, v) in props {
            for p in &ports {
                out.push((k.clone(), v.clone(), p.clone()));
            }
        }
    }
    out
}

fn glob(pat: &str, s: &str) -> bool {
    if !pat.contains('*') {
        return pat == s;
    }
    let parts: Vec<&str> = pat.split('*').collect();
    let mut pos = 0usize;
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() {
            continue;
        }
        match s[pos..].find(p) {
            Some(k) if i > 0 || k == 0 => pos += k + p.len(),
            _ => return false,
        }
    }
    parts.last().map(|l| l.is_empty() || s.ends_with(l)).unwrap_or(true)
}

fn main() {
    let all: Vec<String> = std::env::args().collect();
    let ignore_timing = all.iter().any(|a| a == "--ignore-timing");
    let args: Vec<String> = all.into_iter().filter(|a| !a.starts_with("--")).collect();
    if args.len() < 4 {
        eprintln!("usage: zedbit <design.vhd|.sv> <board.xdc> <out.bit> [top] [--ignore-timing]");
        std::process::exit(2);
    }
    if let Err(e) = run(&args[1], &args[2], &args[3], args.get(4).map(|s| s.as_str()), ignore_timing) {
        eprintln!("zedbit: {e}");
        std::process::exit(1);
    }
}

/// Clock period (ps) from the first `create_clock -period <ns>`; 10 ns default.
fn xdc_period_ps(text: &str) -> u32 {
    for line in text.lines() {
        let l = line.split('#').next().unwrap_or("");
        if !l.trim_start().starts_with("create_clock") {
            continue;
        }
        let toks: Vec<&str> = l.split_whitespace().collect();
        if let Some(i) = toks.iter().position(|t| *t == "-period") {
            if let Some(ns) = toks.get(i + 1).and_then(|v| v.parse::<f64>().ok()) {
                return (ns * 1000.0).round() as u32;
            }
        }
    }
    10_000
}

fn run(design_path: &str, xdc_path: &str, out: &str, top: Option<&str>, ignore_timing: bool) -> Result<(), String> {
    let t0 = std::time::Instant::now();
    let mut design = synth(design_path, top)?;
    let t_synth = t0.elapsed();
    for k in ["SIM_ONLY", "NO_BODY", "WIDE_CONE", "ASSIGN_NOT_LOWERED", "GENERATE_NOT_LOWERED", "FLATTEN_CAP"] {
        if design.attrs.get(k).is_some() {
            return Err(format!("synthesis flagged {k}: part of the design was not lowered"));
        }
    }
    let xdc = std::fs::read_to_string(xdc_path).map_err(|e| format!("{xdc_path}: {e}"))?;
    let props = parse_xdc(&xdc);
    let lookup = |key: &str, prop: &str| -> Option<String> {
        props.iter().rev().find(|(k, _, p)| k.eq_ignore_ascii_case(prop) && glob(p, key)).map(|(_, v, _)| v.clone())
    };

    // Scalar ports carry LOC on the port (clock inputs are packed from it).
    for port in design.ports.iter_mut() {
        if let Some(pin) = lookup(&port.name, "PACKAGE_PIN") {
            port.attrs.set("LOC", &pin);
            port.attrs.set("IOSTANDARD", &lookup(&port.name, "IOSTANDARD").unwrap_or_else(|| "LVCMOS33".into()));
        }
    }
    let port_names: Vec<String> = design.ports.iter().map(|p| p.name.clone()).collect();
    let iob_attrs: HashMap<String, (Option<String>, Option<String>)> = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::IobOut))
        .map(|c| (c.name.clone(), (c.attrs.get("PORT").map(str::to_string), c.attrs.get("BIT").map(str::to_string))))
        .collect();

    let be = Xilinx7Backend::open("xc7z020")?;
    let t_dev = t0.elapsed();
    let mut packed = be.pack(&design)?;
    let mut missing = Vec::new();
    for iob in packed.iobs.iter_mut() {
        let key = match iob_attrs.get(&iob.name) {
            Some((Some(port), Some(bit))) => format!("{port}[{bit}]"),
            Some((Some(port), None)) => port.clone(),
            _ => {
                // Input IOB: its net is `port` or `port_<bit>`.
                if port_names.iter().any(|p| *p == iob.net) {
                    iob.net.clone()
                } else {
                    let (base, bit) = iob.net.rsplit_once('_').unwrap_or((&iob.net, ""));
                    if !bit.is_empty() && bit.chars().all(|c| c.is_ascii_digit()) && port_names.iter().any(|p| p == base) {
                        format!("{base}[{bit}]")
                    } else {
                        iob.port.clone()
                    }
                }
            }
        };
        match lookup(&key, "PACKAGE_PIN") {
            Some(pin) => {
                iob.loc = Some(pin);
                iob.iostandard = lookup(&key, "IOSTANDARD").unwrap_or_else(|| "LVCMOS33".into());
            }
            None => missing.push(key),
        }
    }
    if !missing.is_empty() {
        return Err(format!("no PACKAGE_PIN in {xdc_path} for: {}", missing.join(", ")));
    }
    let placed = be.place(&packed)?;
    let t_place = t0.elapsed();
    let routed = be.route(&placed)?;
    let t_route = t0.elapsed();
    let period = xdc_period_ps(&xdc);
    let ta = helion_xilinx7::timing::analyze(&routed, period)?;
    eprintln!(
        "zedbit timing: period {:.3} ns, worst slack {:.3} ns ({} of {} endpoints failing), est. fmax {:.0} MHz",
        period as f64 / 1000.0,
        ta.wns_ps as f64 / 1000.0,
        ta.failing_endpoints,
        ta.endpoints,
        ta.fmax_mhz()
    );
    if ta.wns_ps < 0 {
        for st in &ta.worst_path {
            eprintln!("  {:>8.3} ns  {}", st.arrival_ps as f64 / 1000.0, st.what);
        }
        if !ignore_timing {
            return Err(format!(
                "timing not met: worst path {:.3} ns exceeds the {:.3} ns clock period (with setup and margin); no bitstream written (pass --ignore-timing to override)",
                ta.worst_path.last().map(|p| p.arrival_ps).unwrap_or(0) as f64 / 1000.0,
                period as f64 / 1000.0
            ));
        }
    }
    let bs = be.bitstream(&routed)?;
    let fasm_text = bs.fasm.join("\n") + "\n";
    let fasm_path = std::path::Path::new(out).with_extension("fasm");
    std::fs::write(&fasm_path, &fasm_text).map_err(|e| e.to_string())?;
    let name = std::path::Path::new(design_path).file_stem().and_then(|s| s.to_str()).unwrap_or("design");
    let r = helion_xilinx7::bitgen::fasm_to_bit(&fasm_text, "xc7z020clg484-1", name)?;
    if !r.rejected.is_empty() {
        for e in r.rejected.iter().take(20) {
            eprintln!("REJECT {e}");
        }
        return Err(format!("{} FASM lines could not be encoded: {:?}", r.rejected.len(), r.rejected_by_tile_type));
    }
    std::fs::write(out, &r.bit).map_err(|e| e.to_string())?;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for c in &design.cells {
        let k = format!("{:?}", c.kind);
        *counts.entry(k.split([' ', '{', '(']).next().unwrap_or("").to_string()).or_default() += 1;
    }
    eprintln!(
        "zedbit timing: synth {:.1}s, device {:.1}s, pack+place {:.1}s, route {:.1}s, bitgen {:.1}s",
        t_synth.as_secs_f64(),
        (t_dev - t_synth).as_secs_f64(),
        (t_place - t_dev).as_secs_f64(),
        (t_route - t_place).as_secs_f64(),
        (t0.elapsed() - t_route).as_secs_f64()
    );
    println!(
        "zedbit: {} -> {} ({} FASM lines, {} features, cells {:?}, {} luts / {} ffs / {} iobs packed)",
        design_path,
        out,
        bs.fasm.len(),
        r.applied,
        counts,
        packed.luts.len(),
        packed.ffs.len(),
        packed.iobs.len()
    );
    Ok(())
}
