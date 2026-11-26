//! HDL + XDC to a 7-series `.bit` with Helion's own flow: synthesis
//! (SystemVerilog or VHDL) -> pack -> place -> route -> timing -> FASM ->
//! `.bit`. This is the flow the `zedbit` example runs on the command line and
//! the IDE runs for 7-series parts, so both give the same bitstream; the
//! options are documented on `examples/zedbit.rs`.

macro_rules! say {
    ($log:expr, $($t:tt)*) => {
        ($log)(&format!($($t)*))
    };
}

/// What [`run`] built. The `.bit` and `.fasm` are on disk.
pub struct Built {
    /// One-line JSON of counts and times (`zedbit: summary`; read by
    /// tools/qor/run.py).
    pub summary: String,
    /// Closing line naming the files and counts.
    pub done: String,
    /// Worst slack and the clock period it was checked against (ns).
    pub slack_ns: f64,
    pub period_ns: f64,
    /// The routed design (placement and routing for the IDE views).
    pub routed: crate::backend::Routed7,
    /// The synthesized netlist (pins from the XDC on its ports).
    pub design: Design,
    /// The elaborated netlist from the v2 front end (`None` when the zb
    /// front end built the design: it keeps no word-level netlist).
    pub rtl: Option<Design>,
    /// The backend, which owns the device model the sites refer to.
    pub backend: Xilinx7Backend,
    /// Timing: worst slack per clock and the worst path.
    pub timing: crate::timing::Analysis,
    /// Period (ps) of each flop clock net.
    pub periods: HashMap<String, u32>,
}

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use helion_backend::Backend;
use helion_ir::{resolve, CellKind, Design};
use crate::backend::Xilinx7Backend;

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
fn synth_v2(paths: &[&str], top: Option<&str>, log: &mut dyn FnMut(&str)) -> Result<(Design, Vec<(String, String)>, Option<Design>), String> {
    helion_sv::v2::set_hard(true);
    // The router does not build SRL16E/SRLC32E pins yet (HELION_GAPS G53):
    // shift registers stay flops unless HELION_V2_SRL=1.
    helion_sv::v2::srl::set_off(true);
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
        say!(log, "zedbit: warning: {w}");
    }
    LUT_LEVELS.store(s.depth as usize, std::sync::atomic::Ordering::Relaxed);
    say!(log, "zedbit synth: v2 front end, {} LUTs, {} flops, {} LUT levels", s.luts, s.ffs, s.depth);
    Ok((s.design, renames, Some(s.rtl)))
}

/// LUT levels from the front end (G44: the summary line reports it, and
/// `synth()` does not return it, so the synthesis pass records it here).
static LUT_LEVELS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The third value is the elaborated (RTL) netlist; the zb front end has none.
pub fn synth(design_arg: &str, top: Option<&str>, front: &str, log: &mut dyn FnMut(&str)) -> Result<(Design, Vec<(String, String)>, Option<Design>), String> {
    let paths: Vec<&str> = design_arg.split(',').filter(|p| !p.is_empty()).collect();
    if paths.is_empty() {
        return Err("no design file".into());
    }
    match front {
        "zb" => synth_zb(&paths, top).map(|d| (d, Vec::new(), None)),
        "v2" => synth_v2(&paths, top, log),
        "auto" => match synth_v2(&paths, top, &mut *log) {
            Ok(d) => Ok(d),
            Err(e) => {
                say!(log, "zedbit: v2 front end: {e}");
                say!(log, "zedbit: using the zb front end for this design");
                synth_zb(&paths, top).map(|d| (d, Vec::new(), None))
            }
        },
        other => Err(format!("unknown front end `{other}` (auto, v2, zb)")),
    }
}

/// XDC properties per port-bit key (`clk`, `led[3]`).
pub fn parse_xdc(text: &str) -> Vec<(String, String, String)> {
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

/// Period of every clock domain after packing (`timing::clock_domains`). A clock from a pad has the
/// XDC period, a PS7 fabric clock its own (`fclk`, net -> ps). A clock made by a flop has twice its source's period when
/// that flop is a plain toggle (its input is its own output inverted, with
/// no enable or reset); any other derived clock is refused, because its
/// period cannot be known.
fn clock_periods(packed: &crate::pack::Packed7, pin_period: u32, fclk: &HashMap<String, u32>) -> Result<HashMap<String, u32>, String> {
    let mut periods: HashMap<String, u32> = HashMap::new();
    let doms: Vec<String> = crate::timing::clock_domains(packed)?.into_iter().map(str::to_string).collect();
    // Buffer input -> output, and which buffer inputs come from pads.
    let pads: std::collections::HashSet<&str> = packed
        .iobs
        .iter()
        .filter(|i| i.dir == crate::pack::IobDir::In)
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
            if let Some(&p) = fclk.get(&b.in_net) {
                periods.insert(dom.clone(), p);
                return false;
            }
            // An MMCM/PLL output: T_in * D * O / M (UG472), from a pad clock
            // or a clock already known.
            if let Some((c, i)) = packed.cmts.iter().find_map(|c| c.clkout_nets.iter().position(|n| n.as_deref() == Some(b.in_net.as_str())).map(|i| (c, i))) {
                let Some(inp) = c.clkin1_net.as_deref() else { return true };
                let src = if pads.contains(inp) { pin_period } else if let Some(&p) = periods.get(inp) { p } else { return true };
                let m = c.clkfbout_mult_f.round() as u64;
                let o = if i == 0 { c.clkout0_divide_f.round() as u64 } else { c.clkout_divide[i] as u64 };
                // Rounded down: a shorter period is the safe side.
                periods.insert(dom.clone(), (src as u64 * c.divclk_divide as u64 * o / m) as u32);
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
    for c in &packed.cmts {
        // The attribute the CMT was configured for must be the clock it gets.
        if c.clkin1_net.as_deref().is_some_and(|n| pads.contains(n)) && (c.clkin1_period_ns * 1000.0 - pin_period as f64).abs() > pin_period as f64 * 0.01 {
            return Err(format!(
                "{}: CLKIN1_PERIOD is {} ns but its input clock has a {:.3} ns period (create_clock)",
                c.name, c.clkin1_period_ns, pin_period as f64 / 1000.0
            ));
        }
    }
    if !pending.is_empty() {
        return Err(format!(
            "clock {} is not a pad clock, a PS7 FCLK, an MMCM/PLL output or a divide-by-two of one; its period is unknown, so timing cannot be checked",
            pending.join(", ")
        ));
    }
    Ok(periods)
}

/// Clock domains (flop clock nets after packing) of each `-group` of every
/// `set_clock_groups -asynchronous` in the XDC. A group names clocks as
/// `[get_clocks -of_objects [get_pins CELL/CLKOUTn]]` (an MMCM/PLL output)
/// or `[get_clocks NAME]` (a `create_clock -name` on a pad or a PS7 FCLK). Anything
/// else is an error, never ignored.
fn async_groups(xdc: &str, packed: &crate::pack::Packed7) -> Result<Vec<Vec<String>>, String> {
    let bufg_of = |net: &str| packed.bufgs.iter().find(|b| b.in_net == net).map(|b| b.out_net.clone());
    let mut out = Vec::new();
    for line in xdc.lines() {
        let l = line.split('#').next().unwrap_or("").trim();
        if !l.starts_with("set_clock_groups") {
            continue;
        }
        if !l.contains("-asynchronous") {
            return Err(format!("XDC: only `set_clock_groups -asynchronous` is supported: {l}"));
        }
        for g in l.split("-group").skip(1) {
            let mut doms = Vec::new();
            for (i, _) in g.match_indices("get_pins") {
                let rest = g[i + 8..].trim_start().trim_start_matches('{');
                let pin: String = rest.chars().take_while(|c| !c.is_whitespace() && *c != ']' && *c != '}').collect();
                let (cell, port) = pin.rsplit_once('/').ok_or_else(|| format!("XDC: bad pin `{pin}` in {l}"))?;
                let c = packed.cmts.iter().find(|c| c.name == cell).ok_or_else(|| {
                    format!("XDC: `{cell}` is not an MMCM/PLL (have: {})", packed.cmts.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", "))
                })?;
                let k: usize = port.strip_prefix("CLKOUT").and_then(|d| d.parse().ok()).ok_or_else(|| format!("XDC: `{pin}`: only CLKOUTn pins name clocks"))?;
                let net = c.clkout_nets.get(k).cloned().flatten().ok_or_else(|| format!("XDC: `{pin}` is not connected"))?;
                doms.push(bufg_of(&net).ok_or_else(|| format!("XDC: `{pin}` does not drive a BUFG"))?);
            }
            if !g.contains("get_pins") {
                for (i, _) in g.match_indices("get_clocks") {
                    let name: String = g[i + 10..].trim_start().trim_start_matches('{').chars().take_while(|c| !c.is_whitespace() && *c != ']' && *c != '}').collect();
                    let cl = xdc
                        .lines()
                        .map(|cl| cl.split('#').next().unwrap_or(""))
                        .find(|cl| cl.trim_start().starts_with("create_clock") && cl.split_whitespace().collect::<Vec<_>>().windows(2).any(|w| w == ["-name", name.as_str()]))
                        .ok_or_else(|| format!("XDC: clock `{name}` has no create_clock"))?;
                    // The object after `kw`, without its braces or the command's closing bracket.
                    let obj = |kw: &str| {
                        let mut o: String = cl.split(kw).nth(1)?.trim_start().trim_start_matches('{').chars().take_while(|c| !c.is_whitespace() && *c != '}').collect();
                        while o.matches(']').count() > o.matches('[').count() {
                            o.pop();
                        }
                        Some(o)
                    };
                    let src = if let Some(port) = obj("get_ports") {
                        packed.iobs.iter().find(|i| i.port == port).ok_or_else(|| format!("XDC: no pad for clock port `{port}`"))?.net.clone()
                    } else if let Some(pin) = obj("get_pins") {
                        // A PS7 fabric clock, `<cell>/FCLKCLK[n]` (as ps7::fclk_periods reads it).
                        let bit = pin.rsplit_once("/FCLKCLK[").and_then(|(_, b)| b.strip_suffix(']')).ok_or_else(|| format!("XDC: clock `{name}`: only a PS7 FCLKCLK pin names a clock, not `{pin}`"))?;
                        packed
                            .ps7s
                            .iter()
                            .flat_map(|p| &p.pins)
                            .find(|(n, _, out)| *out && *n == format!("FCLKCLK{bit}"))
                            .map(|x| x.1.clone())
                            .ok_or_else(|| format!("XDC: clock `{name}`: `{pin}` is not connected"))?
                    } else {
                        return Err(format!("XDC: clock `{name}` is on neither a port nor a pin: {cl}"));
                    };
                    doms.push(bufg_of(&src).ok_or_else(|| format!("XDC: clock `{name}` does not drive a BUFG"))?);
                }
            }
            if doms.is_empty() {
                return Err(format!("XDC: a clock group names no clock zedbit understands: {l}"));
            }
            out.push(doms);
        }
    }
    Ok(out)
}

/// Pad clock period (ps) from the first `create_clock -period <ns>` on
/// `get_ports` (one on a PS7 pin is that clock's own, `ps7::fclk_periods`);
/// 10 ns default.
fn xdc_period_ps(text: &str) -> u32 {
    for line in text.lines() {
        let l = line.split('#').next().unwrap_or("");
        if !l.trim_start().starts_with("create_clock") || !l.contains("get_ports") {
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

pub fn run(
    design_path: &str,
    xdc_path: &str,
    out: &str,
    top: Option<&str>,
    ignore_timing: bool,
    front: &str,
    part: &str,
    vivado: Option<(String, bool)>,
    log: &mut dyn FnMut(&str),
) -> Result<Option<Built>, String> {
    let t0 = std::time::Instant::now();
    // The Zynq PS7 cell, with its ports from the database.
    if part.starts_with("xc7z") {
        match crate::ps7::sv_ports() {
            Ok(p) => helion_sv::v2::mem::register_prim("PS7", &p),
            Err(e) => say!(log, "zedbit: no PS7 cell for this part: {e}"),
        }
    }
    let (mut design, renames, rtl) = synth(design_path, top, front, &mut *log)?;
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

    say!(log, "zedbit: synthesized in {:.1}s; loading the device", t_synth.as_secs_f64());
    let be = Xilinx7Backend::open(part)?;
    say!(log, "zedbit: part {part}");
    let t_dev = t0.elapsed();
    let mut packed = be.pack(&design)?;
    let ramb36 = packed.brams.iter().filter(|b| b.kind == crate::pack::BramKind::Ramb36e1).count();
    say!(log, 
        "zedbit: packed {} LUTs, {} flops, {} carry cells, {} RAMB36 + {} RAMB18, {} DSP48, {} LUT RAMs ({} LUT RAM LUTs), {} pads; placing",
        packed.luts.len(),
        packed.ffs.len(),
        packed.carries.len(),
        ramb36,
        packed.brams.len() - ramb36,
        packed.dsps.len(),
        packed.lutrams.len(),
        packed.lutrams.iter().map(|r| r.init.iter().flatten().count()).sum::<usize>(),
        packed.iobs.len()
    );
    let mut missing = Vec::new();
    let mut iob_keys: HashMap<String, String> = HashMap::new();
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
        iob_keys.insert(iob.name.clone(), key.clone());
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
    say!(log, "zedbit: placed in {:.1}s", (t_place - t_dev).as_secs_f64());
    if let Some((prefix, only)) = &vivado {
        let base = Path::new(prefix).file_name().and_then(|s| s.to_str()).ok_or("bad --vivado prefix")?;
        let f = crate::vivado_export::export(&design, &placed, &iob_keys, &xdc, resolve(design.name), part, base)?;
        for (ext, text) in [(".v", &f.verilog), (".xdc", &f.xdc), (".tcl", &f.tcl), ("_noroute.tcl", &f.tcl_noroute)] {
            std::fs::write(format!("{prefix}{ext}"), text).map_err(|e| format!("{prefix}{ext}: {e}"))?;
        }
        say!(log, "zedbit: Vivado export written to {prefix}.v/.xdc/.tcl/_noroute.tcl");
        if *only {
            return Ok(None);
        }
    }
    say!(log, "zedbit: routing");
    let routed = be.route(&placed)?;
    let t_route = t0.elapsed();
    // G41 wire-model probe: `HELION_HOP_DUMP=<path>` writes
    // `<dx> <dy> <lx> <ly> <routed_ps> <pips>` for every fabric load, so the
    // placer's wire estimate can be fitted on all connections rather than on a
    // single worst path (fitting one path is what produced the bad 964 + 49
    // figure). Off unless the variable is set.
    if let Some(dst) = std::env::var_os("HELION_HOP_DUMP") {
        let mut xy: std::collections::HashMap<&str, (i32, i32)> = std::collections::HashMap::new();
        for l in &routed.placed.luts {
            if let Some(p) = crate::place::parse_site_xy(&l.site) { xy.insert(l.site.as_str(), p); }
        }
        for f in &routed.placed.ffs {
            if let Some(p) = crate::place::parse_site_xy(&f.site) { xy.insert(f.site.as_str(), p); }
        }
        for c in &routed.placed.carries {
            if let Some(p) = crate::place::parse_site_xy(&c.site) { xy.insert(c.site.as_str(), p); }
        }
        // Driver site per net: join the packed cells (which know their nets) to
        // the placed cells (which know their sites) on the cell name.
        let site_of = |name: &str| {
            routed
                .placed
                .luts
                .iter()
                .map(|l| (l.name.as_str(), l.site.as_str()))
                .chain(routed.placed.ffs.iter().map(|f| (f.name.as_str(), f.site.as_str())))
                .find(|(n, _)| *n == name)
                .and_then(|(_, s)| crate::place::parse_site_xy(s))
        };
        let mut drv_xy: std::collections::HashMap<&str, (i32, i32)> = std::collections::HashMap::new();
        for l in &routed.placed.packed.luts {
            if let Some(p) = site_of(&l.name) { drv_xy.insert(l.output_net.as_str(), p); }
        }
        for f in &routed.placed.packed.ffs {
            if let Some(p) = site_of(&f.name) { drv_xy.insert(f.q_net.as_str(), p); }
        }
        let mut lines = format!("# {} hops, part {part}\n", routed.load_delays.len());
        // Pips per net, to test whether the cost is driven by hop count
        // (mux levels) rather than by Manhattan distance.
        let mut pips_of: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for n in &routed.nets {
            if let crate::route::RouteKind::Pips(ps) = &n.kind {
                pips_of.insert(n.net.as_str(), ps.len());
            }
        }
        for ((net, site, pin), &delay) in routed.load_delays.iter() {
            let (lx, ly) = xy.get(site.as_str()).copied().unwrap_or((-1, -1));
            let (dx, dy) = drv_xy.get(net.as_str()).copied().unwrap_or((-1, -1));
            let np = pips_of.get(net.as_str()).copied().unwrap_or(0);
            lines.push_str(&format!("{dx} {dy} {lx} {ly} {delay} {np}\n"));
        }
        let _ = std::fs::write(dst, lines);
    }
    say!(log, "zedbit: routed in {:.1}s", (t_route - t_place).as_secs_f64());
    let mut period = xdc_period_ps(&xdc);
    let mut fclk = crate::ps7::fclk_periods(&xdc, &packed.ps7s)?;
    // Flops on both clock edges: a path from one edge to the other has half
    // a period. The analysis has one period for every path, so it is run
    // at half the clock period, which is safe for all of them.
    let n_neg = packed.negedge_ffs.len();
    if n_neg > 0 && n_neg < packed.ffs.len() {
        say!(log, 
            "zedbit timing: {n_neg} of {} flops clock on the falling edge; every path is checked against half the period",
            packed.ffs.len()
        );
        period /= 2;
        fclk.values_mut().for_each(|p| *p /= 2);
    }
    let periods = clock_periods(&packed, period, &fclk)?;
    let groups = async_groups(&xdc, &packed)?;
    let group_of = |c: &str| groups.iter().position(|g| g.iter().any(|d| d == c));
    if !groups.is_empty() {
        say!(log, "zedbit timing: asynchronous clock groups {groups:?}: flop-to-flop crossings between them allowed (timed against the capturing clock)");
    }
    let ta = crate::timing::analyze_clocks_async(&routed, &|c| periods.get(c).copied(), &|a, b| {
        matches!((group_of(a), group_of(b)), (Some(x), Some(y)) if x != y)
    })?;
    for (clk, wns) in &ta.clocks {
        say!(log, "zedbit timing: clock {clk}: period {:.3} ns, worst slack {:.3} ns", periods[clk] as f64 / 1000.0, *wns as f64 / 1000.0);
    }
    say!(log, 
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
            (Some(a), Some(b)) if a != b && st.wire_ps > 0 => format!("{:>6.3}", crate::place_sa::wire_ps((a.0 - b.0).abs() + (a.1 - b.1).abs()) as f64 / 1000.0),
            _ => "     -".into(),
        };
        say!(log, "  {:>8.3} ns  wire {:>6.3} (placer est {est})  {}", st.arrival_ps as f64 / 1000.0, st.wire_ps as f64 / 1000.0, st.what);
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
    let r = crate::bitgen::fasm_to_bit(&fasm_text, part, name)?;
    if !r.rejected.is_empty() {
        for e in r.rejected.iter().take(20) {
            say!(log, "REJECT {e}");
        }
        return Err(format!("{} FASM lines could not be encoded: {:?}", r.rejected.len(), r.rejected_by_tile_type));
    }
    std::fs::write(out, &r.bit).map_err(|e| e.to_string())?;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for c in &design.cells {
        let k = format!("{:?}", c.kind);
        *counts.entry(k.split([' ', '{', '(']).next().unwrap_or("").to_string()).or_default() += 1;
    }
    say!(log, 
        "zedbit timing: synth {:.1}s, device {:.1}s, pack+place {:.1}s, route {:.1}s, bitgen {:.1}s",
        t_synth.as_secs_f64(),
        (t_dev - t_synth).as_secs_f64(),
        (t_place - t_dev).as_secs_f64(),
        (t_route - t_place).as_secs_f64(),
        (t0.elapsed() - t_route).as_secs_f64()
    );
    // One machine-readable line (G44). tools/qor/run.py scraped the human
    // lines above with regexes and could silently record `null` for a number
    // when the wording changed; this line is the contract it reads instead.
    // Keep the keys in step with `COUNTS`/`TIMES` in tools/qor/run.py.
    //
    // The counts come from the *placed* design, not from `packed` (G48).
    // Placement replicates a driver shared by many block RAMs/DSPs once per
    // group (`replicate_hard_block_drivers`), so the bitstream holds those
    // copies while `packed` never saw them: bram_fill read 864 flops here
    // while x7v extracted 1,524 from the same `.bit`. The "packed ..." line
    // above still reports `packed`, because that is genuinely the pre-
    // placement count and it is printed before placement runs.
    let built = &routed.placed.packed;
    let ramb36 = built.brams.iter().filter(|b| b.kind == crate::pack::BramKind::Ramb36e1).count();
    let summary = json_object(&[
            ("lut", built.luts.len() as f64),
            ("lut_rt", carry_route_throughs(built) as f64),
            ("ff", built.ffs.len() as f64),
            ("carry", built.carries.len() as f64),
            ("ramb36", ramb36 as f64),
            ("ramb18", (built.brams.len() - ramb36) as f64),
            ("dsp", built.dsps.len() as f64),
            ("lutram", built.lutrams.len() as f64),
            ("pad", built.iobs.len() as f64),
            ("lut_levels", LUT_LEVELS.load(std::sync::atomic::Ordering::Relaxed) as f64),
            ("t_synth", t_synth.as_secs_f64()),
            ("t_device", (t_dev - t_synth).as_secs_f64()),
            ("t_pack_place", (t_place - t_dev).as_secs_f64()),
            ("t_route", (t_route - t_place).as_secs_f64()),
            ("t_bitgen", (t0.elapsed() - t_route).as_secs_f64()),
            ("period_ns", ta.period_ps as f64 / 1000.0),
            ("slack_ns", ta.wns_ps as f64 / 1000.0),
            ("failing", ta.failing_endpoints as f64),
            ("endpoints", ta.endpoints as f64),
            ("features", r.applied as f64),
            ("fasm", bs.fasm.len() as f64),
        ]);
    let done = format!(
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
    Ok(Some(Built {
        summary,
        done,
        slack_ns: ta.wns_ps as f64 / 1000.0,
        period_ns: ta.period_ps as f64 / 1000.0,
        routed,
        design,
        rtl,
        backend: be,
        timing: ta,
        periods,
    }))
}

/// A one-line JSON object of numbers, without pulling a serializer into this
/// example (every value is a count or a duration, so `{:?}` on f64 is a valid
/// JSON number). Keys are fixed and ASCII, so no escaping is needed.
fn json_object(fields: &[(&str, f64)]) -> String {
    let body: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("\"{k}\":{v:?}"))
        .collect();
    format!("{{{}}}", body.join(","))
}

/// LUTs that only pass one net into a carry chain's S input. Vivado's
/// "Slice LUTs" leaves these route-throughs out (the rung 4 design: 30 CARRY4
/// and 12 LUT1 in its 37), so comparisons with it subtract them. A LUT that
/// also drives a constant DI from O5 still counts (an incrementer's chain).
fn carry_route_throughs(p: &crate::pack::Packed7) -> usize {
    let tie = |n: &str| match n {
        "0" | "gnd" | "GND" | "$false" | "const0" => Some(0u64),
        "1" | "vcc" | "VCC" | "$true" | "const1" => Some(1),
        _ => None,
    };
    let mut readers: std::collections::HashMap<&str, (usize, usize)> = std::collections::HashMap::new(); // net -> (carry S reads, other reads)
    for c in &p.carries {
        for n in c.s_nets.iter().flatten() {
            readers.entry(n.as_str()).or_default().0 += 1;
        }
    }
    let others = p
        .luts
        .iter()
        .flat_map(|l| l.input_nets.iter().flatten())
        .chain(p.ffs.iter().flat_map(|f| [Some(&f.d_net), f.ce_net.as_ref(), f.sr_net.as_ref()].into_iter().flatten()))
        .chain(p.iobs.iter().map(|i| &i.net));
    for n in others {
        readers.entry(n.as_str()).or_default().1 += 1;
    }
    p.luts
        .iter()
        .filter(|l| {
            let live: Vec<usize> = (0..6).filter(|&k| l.input_nets[k].as_deref().is_some_and(|n| tie(n).is_none())).collect();
            let [k] = live[..] else { return false };
            // O6 equals input k wherever the tied inputs hold their value (A6 high on a dual LUT).
            let dual = p.dual_luts.contains(&l.name);
            let identity = (0..64u64).all(|a| {
                let held = (0..6).all(|j| l.input_nets[j].as_deref().and_then(tie).is_none_or(|v| (a >> j) & 1 == v));
                !held || (dual && a & 32 == 0) || (l.init >> a) & 1 == (a >> k) & 1
            });
            identity && readers.get(l.output_net.as_str()).is_some_and(|&(s, o)| s > 0 && o == 0)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAD: &str = "create_clock -period 3.000 -name gclk [get_ports gclk]";
    const FCLK: &str = "create_clock -period 10.000 -name fclk0 [get_pins ps7/FCLKCLK[0]]";

    #[test]
    fn pad_period_ignores_a_ps7_clock_in_either_order() {
        assert_eq!(xdc_period_ps(&format!("{FCLK}\n{PAD}\n")), 3_000);
        assert_eq!(xdc_period_ps(&format!("{PAD}\n{FCLK}\n")), 3_000);
        assert_eq!(xdc_period_ps(&format!("{FCLK}\n")), 10_000, "no pad clock: the default");
    }

    #[test]
    fn clock_groups_name_pad_and_ps7_clocks() {
        use crate::pack::{IobDir, PackedBufg, PackedIob, Packed7};
        let bufg = |i: &str, o: &str| PackedBufg { name: format!("b_{o}"), in_net: i.into(), out_net: o.into(), ..Default::default() };
        let packed = Packed7 {
            iobs: vec![PackedIob { name: "gclk_ibuf".into(), port: "gclk".into(), dir: IobDir::In, net: "gclk".into(), loc: None, iostandard: "LVCMOS33".into(), drive: None, slew: None }],
            bufgs: vec![bufg("gclk", "gclk_bufg"), bufg("fclk_0", "clk_ps")],
            ps7s: vec![crate::ps7::PackedPs7 { name: "ps7".into(), pins: vec![("FCLKCLK0".into(), "fclk_0".into(), true)], ties: Vec::new() }],
            ..Default::default()
        };
        let groups = "set_clock_groups -asynchronous -group [get_clocks fclk0] -group [get_clocks {gclk}]";
        let g = async_groups(&format!("{PAD}\n{FCLK}\n{groups}\n"), &packed).unwrap();
        assert_eq!(g, [["clk_ps"], ["gclk_bufg"]]);
        // `-name gclk` is not `-name gclk2`.
        let e = async_groups(&format!("{FCLK}\ncreate_clock -period 3 -name gclk2 [get_ports gclk]\n{groups}\n"), &packed).unwrap_err();
        assert!(e.contains("clock `gclk` has no create_clock"), "{e}");
    }
}
