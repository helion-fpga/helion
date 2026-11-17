//! Build a 7-series bitstream from HDL with Helion's own flow: synthesis
//! (SystemVerilog or VHDL) -> pack -> place -> route -> FASM -> `.bit`.
//!
//! ```text
//! cargo run --release -p helion-xilinx7 --example zedbit -- \
//!     design.sv[,more.sv...] pins.xdc out.bit [top] [--board=NAME | --part=PART]
//!     [--front=auto|v2|zb] [--ignore-timing]
//! ```
//!
//! The part is the ZedBoard's (`xc7z020clg484-1`) unless `--board` names a
//! board (`zedboard`, `arty-a7-35`, `basys3`, `nexys-a7-100t`; its pins are in
//! `boards/<name>.xdc`) or `--part` names any part the Project X-Ray database
//! describes (see `helion_xilinx7::parts`).
//!
//! Front ends (`--front`, or `HELION_ZED_FRONT`):
//!
//! * `v2`: Helion's current SystemVerilog front end
//!   (`helion_sv::synth_sv_v2_files`). VHDL is translated to SystemVerilog
//!   first. Arithmetic is LUT logic; there is no carry-chain inference yet.
//! * `zb`: the front end this flow first ran on the board with
//!   (`helion-zb-front`), which puts adders on `CARRY4` chains.
//! * `auto` (default): `v2`, and `zb` only for a design `v2` refuses; the
//!   switch is announced.
//!
//! Pins come from the XDC (`PACKAGE_PIN`, `IOSTANDARD`, including `-dict`
//! form and multi-port / wildcard `get_ports`). Every top-level port bit
//! must have a pin; a missing pin is an error, never a silent default.
//! The FASM is written next to the bitstream (`out.fasm`).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use helion_backend::Backend;
use helion_ir::{resolve, CellKind, Design};
use helion_xilinx7::backend::Xilinx7Backend;

fn is_vhdl(path: &str) -> bool {
    path.ends_with(".vhd") || path.ends_with(".vhdl")
}

/// The front end proven on the board: one source text, carry chains for
/// arithmetic unless `HELION_NO_CARRY` is set.
fn synth_zb(paths: &[&str], top: Option<&str>) -> Result<Design, String> {
    helion_zb_front::set_dsp_inference(false);
    helion_zb_front::set_carry_arith(std::env::var("HELION_NO_CARRY").is_err());
    let mut src = String::new();
    for p in paths {
        src.push_str(&std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?);
        src.push('\n');
    }
    if is_vhdl(paths[0]) {
        helion_zb_front::synth_vhdl_top(&src, top)
    } else {
        helion_zb_front::synth_sv(&src, paths[0])
    }
}

/// The current front end. VHDL goes through the VHDL-to-SystemVerilog
/// translator first; the second value lists the VHDL port names it had to
/// rename (SystemVerilog keywords), as `(VHDL name, netlist name)`.
fn synth_v2(paths: &[&str], top: Option<&str>) -> Result<(Design, Vec<(String, String)>), String> {
    let mut renames = Vec::new();
    helion_sv::v2::dag::set_carry_arith(std::env::var("HELION_NO_CARRY").is_err());
    let s = if is_vhdl(paths[0]) {
        let mut src = String::new();
        for p in paths {
            src.push_str(&std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?);
            src.push('\n');
        }
        let (sv, r) = helion_zb_front::vhdl_to_sv(&src, top).map_err(|e| format!("{}: {e}", paths[0]))?;
        renames = r;
        helion_sv::synth_sv_v2(&sv, "vhdl.sv", None)?
    } else {
        let files: Vec<&Path> = paths.iter().map(|p| Path::new(*p)).collect();
        helion_sv::synth_sv_v2_files(&files, top)?
    };
    for w in &s.warnings {
        eprintln!("zedbit: warning: {w}");
    }
    eprintln!("zedbit synth: v2 front end, {} LUTs, {} flops, {} LUT levels", s.luts, s.ffs, s.depth);
    Ok((s.design, renames))
}

fn synth(design_arg: &str, top: Option<&str>, front: &str) -> Result<(Design, Vec<(String, String)>), String> {
    let paths: Vec<&str> = design_arg.split(',').filter(|p| !p.is_empty()).collect();
    if paths.is_empty() {
        return Err("no design file".into());
    }
    match front {
        "zb" => synth_zb(&paths, top).map(|d| (d, Vec::new())),
        "v2" => synth_v2(&paths, top),
        "auto" => match synth_v2(&paths, top) {
            Ok(d) => Ok(d),
            Err(e) => {
                eprintln!("zedbit: v2 front end: {e}");
                eprintln!("zedbit: using the zb front end for this design");
                synth_zb(&paths, top).map(|d| (d, Vec::new()))
            }
        },
        other => Err(format!("unknown front end `{other}` (auto, v2, zb)")),
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
    let front = all
        .iter()
        .find_map(|a| a.strip_prefix("--front=").map(str::to_string))
        .or_else(|| std::env::var("HELION_ZED_FRONT").ok())
        .unwrap_or_else(|| "auto".to_string());
    let opt = |k: &str| all.iter().find_map(|a| a.strip_prefix(k).map(str::to_string));
    let part = match (opt("--board="), opt("--part=")) {
        (Some(_), Some(_)) => {
            eprintln!("zedbit: give --board or --part, not both");
            std::process::exit(2);
        }
        (Some(b), None) => match helion_xilinx7::parts::board(&b) {
            Ok(b) => b.part.to_string(),
            Err(e) => {
                eprintln!("zedbit: {e}");
                std::process::exit(2);
            }
        },
        (None, Some(p)) => p,
        (None, None) => "xc7z020clg484-1".to_string(),
    };
    let args: Vec<String> = all.into_iter().filter(|a| !a.starts_with("--")).collect();
    if args.len() < 4 {
        eprintln!("usage: zedbit <design.vhd|.sv[,more...]> <pins.xdc> <out.bit> [top] [--board=NAME | --part=PART] [--front=auto|v2|zb] [--ignore-timing]");
        std::process::exit(2);
    }
    if let Err(e) = run(&args[1], &args[2], &args[3], args.get(4).map(|s| s.as_str()), ignore_timing, &front, &part) {
        eprintln!("zedbit: {e}");
        std::process::exit(1);
    }
}

/// Period of every flop clock after packing. A clock from a pad has the
/// XDC period. A clock made by a flop has twice its source's period when
/// that flop is a plain toggle (its input is its own output inverted, with
/// no enable or reset); any other derived clock is refused, because its
/// period cannot be known.
fn clock_periods(packed: &helion_xilinx7::pack::Packed7, pin_period: u32) -> Result<HashMap<String, u32>, String> {
    let mut periods: HashMap<String, u32> = HashMap::new();
    let mut doms: Vec<String> = packed.ffs.iter().map(|f| f.clk_net.clone()).collect();
    doms.sort();
    doms.dedup();
    // Buffer input -> output, and which buffer inputs come from pads.
    let pads: std::collections::HashSet<&str> = packed
        .iobs
        .iter()
        .filter(|i| i.dir == helion_xilinx7::pack::IobDir::In)
        .map(|i| i.net.as_str())
        .collect();
    let mut pending = doms.clone();
    for _ in 0..8 {
        pending.retain(|dom| {
            let Some(b) = packed.bufgs.iter().find(|b| &b.out_net == dom) else { return true };
            if pads.contains(b.in_net.as_str()) {
                periods.insert(dom.clone(), pin_period);
                return false;
            }
            let Some(f) = packed.ffs.iter().find(|f| f.q_net == b.in_net) else { return true };
            let Some(&src) = periods.get(&f.clk_net) else { return true };
            let toggles = f.ce_net.is_none()
                && f.sr_net.is_none()
                && !packed.negedge_ffs.contains(&f.name)
                && packed.luts.iter().find(|l| l.output_net == f.d_net).is_some_and(|l| {
                    let pins: Vec<usize> = (0..6).filter(|&k| l.input_nets[k].as_deref() == Some(f.q_net.as_str())).collect();
                    let only_q = l.input_nets.iter().flatten().all(|n| *n == f.q_net);
                    let at = |v: u64| -> u64 { pins.iter().fold(0u64, |a, &k| a | (v << k)) };
                    only_q && !pins.is_empty() && (l.init >> at(0)) & 1 == 1 && (l.init >> at(1)) & 1 == 0
                });
            if toggles {
                periods.insert(dom.clone(), src * 2);
                false
            } else {
                true
            }
        });
    }
    if !pending.is_empty() {
        return Err(format!(
            "clock {} is not a pad clock or a divide-by-two of one; its period is unknown, so timing cannot be checked",
            pending.join(", ")
        ));
    }
    Ok(periods)
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

fn run(design_path: &str, xdc_path: &str, out: &str, top: Option<&str>, ignore_timing: bool, front: &str, part: &str) -> Result<(), String> {
    let t0 = std::time::Instant::now();
    let (mut design, renames) = synth(design_path, top, front)?;
    let t_synth = t0.elapsed();
    for k in ["SIM_ONLY", "NO_BODY", "WIDE_CONE", "ASSIGN_NOT_LOWERED", "GENERATE_NOT_LOWERED", "FLATTEN_CAP"] {
        if design.attrs.get(k).is_some() {
            return Err(format!("synthesis flagged {k}: part of the design was not lowered"));
        }
    }
    let xdc = std::fs::read_to_string(xdc_path).map_err(|e| format!("{xdc_path}: {e}"))?;
    let props = parse_xdc(&xdc);
    let lookup = |key: &str, prop: &str| -> Option<String> {
        // The XDC uses the VHDL name of a port the translator renamed.
        let vhdl = renames.iter().find_map(|(v, e)| key.strip_prefix(e.as_str()).filter(|r| r.is_empty() || r.starts_with('[')).map(|r| format!("{v}{r}")));
        let key = vhdl.as_deref().unwrap_or(key);
        props.iter().rev().find(|(k, _, p)| k.eq_ignore_ascii_case(prop) && glob(p, key)).map(|(_, v, _)| v.clone())
    };

    // Scalar ports carry LOC on the port (clock inputs are packed from it).
    for port in design.ports.iter_mut() {
        let name = resolve(port.name);
        if let Some(pin) = lookup(name, "PACKAGE_PIN") {
            port.attrs.set("LOC", &pin);
            port.attrs.set("IOSTANDARD", &lookup(name, "IOSTANDARD").unwrap_or_else(|| "LVCMOS33".into()));
        }
    }
    let port_names: Vec<String> = design.ports.iter().map(|p| resolve(p.name).to_string()).collect();
    let iob_attrs: HashMap<String, (Option<String>, Option<String>)> = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::IobOut))
        .map(|c| (resolve(c.name).to_string(), (c.attrs.get("PORT").map(str::to_string), c.attrs.get("BIT").map(str::to_string))))
        .collect();

    eprintln!("zedbit: synthesized in {:.1}s; loading the device", t_synth.as_secs_f64());
    let be = Xilinx7Backend::open(part)?;
    eprintln!("zedbit: part {part}");
    let t_dev = t0.elapsed();
    let mut packed = be.pack(&design)?;
    eprintln!(
        "zedbit: packed {} LUTs, {} flops, {} carry cells, {} pads; placing",
        packed.luts.len(),
        packed.ffs.len(),
        packed.carries.len(),
        packed.iobs.len()
    );
    let mut missing = Vec::new();
    for iob in packed.iobs.iter_mut() {
        let key = match iob_attrs.get(&iob.name) {
            // A scalar port is bit 0 of itself; its XDC names it bare.
            Some((Some(port), Some(bit))) if bit == "0" && lookup(&format!("{port}[0]"), "PACKAGE_PIN").is_none() && lookup(port, "PACKAGE_PIN").is_some() => port.clone(),
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
    eprintln!("zedbit: placed in {:.1}s; routing", (t_place - t_dev).as_secs_f64());
    let routed = be.route(&placed)?;
    let t_route = t0.elapsed();
    eprintln!("zedbit: routed in {:.1}s", (t_route - t_place).as_secs_f64());
    let mut period = xdc_period_ps(&xdc);
    // Flops on both clock edges: a path from one edge to the other has half
    // a period. The analysis has one period for every path, so it is run
    // at half the clock period, which is safe for all of them.
    let n_neg = packed.negedge_ffs.len();
    if n_neg > 0 && n_neg < packed.ffs.len() {
        eprintln!(
            "zedbit timing: {n_neg} of {} flops clock on the falling edge; every path is checked against half the period",
            packed.ffs.len()
        );
        period /= 2;
    }
    let periods = clock_periods(&packed, period)?;
    let ta = helion_xilinx7::timing::analyze_clocks(&routed, &|c| periods.get(c).copied())?;
    for (clk, wns) in &ta.clocks {
        eprintln!("zedbit timing: clock {clk}: period {:.3} ns, worst slack {:.3} ns", periods[clk] as f64 / 1000.0, *wns as f64 / 1000.0);
    }
    eprintln!(
        "zedbit timing: worst clock period {:.3} ns, worst slack {:.3} ns ({} of {} endpoints failing), est. fmax {:.0} MHz on that clock",
        ta.period_ps as f64 / 1000.0,
        ta.wns_ps as f64 / 1000.0,
        ta.failing_endpoints,
        ta.endpoints,
        ta.fmax_mhz()
    );
    // Each hop's routed wire next to the placer's estimate (none for dedicated carry cascades).
    let mut prev: Option<(i32, i32)> = None;
    for st in &ta.worst_path {
        let est = match (prev, st.xy) {
            (Some(a), Some(b)) if a != b && st.wire_ps > 0 => format!("{:>6.3}", helion_xilinx7::place_sa::wire_ps((a.0 - b.0).abs() + (a.1 - b.1).abs()) as f64 / 1000.0),
            _ => "     -".into(),
        };
        eprintln!("  {:>8.3} ns  wire {:>6.3} (placer est {est})  {}", st.arrival_ps as f64 / 1000.0, st.wire_ps as f64 / 1000.0, st.what);
        prev = st.xy.or(prev);
    }
    if ta.wns_ps < 0 {
        if !ignore_timing {
            return Err(format!(
                "timing not met: worst path {:.3} ns exceeds the {:.3} ns clock period (with setup and margin); no bitstream written (pass --ignore-timing to override)",
                ta.worst_path.last().map(|p| p.arrival_ps).unwrap_or(0) as f64 / 1000.0,
                ta.period_ps as f64 / 1000.0
            ));
        }
    }
    let bs = be.bitstream(&routed)?;
    let fasm_text = bs.fasm.join("\n") + "\n";
    let fasm_path = std::path::Path::new(out).with_extension("fasm");
    std::fs::write(&fasm_path, &fasm_text).map_err(|e| e.to_string())?;
    let name = std::path::Path::new(design_path).file_stem().and_then(|s| s.to_str()).unwrap_or("design");
    let r = helion_xilinx7::bitgen::fasm_to_bit(&fasm_text, part, name)?;
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
