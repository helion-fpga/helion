//! Tcl object model + flow commands (the design-database side).
//!
//! Object queries (`get_cells`, `get_nets`, `get_pins`, `get_ports`,
//! `get_clocks`, `get_timing_paths`) support glob patterns, Vivado-style
//! `-filter` expressions and `-of_objects` relationship traversal. Objects
//! stringify to their hierarchical names; [`get_property`] exposes real
//! properties pulled from the design database (type, site, fanout, slack…).
//! Flow commands (`synth_design`, `place_design`, …) call the same
//! [`helion_proj::Session`] engines as the GUI buttons.

use super::interp::{eval_filter, join_list, split_list, Ctl, TclError, TclInterp};
use crate::{impl_if_needed, synth_hdl_path, GpuiShell};
use helion_device::Device;
use helion_ir::{Cell, CellKind, Design, Net, PortDir};
use helion_proj::opt_design;
use helion_sta::{create_clock, report_timing_routed_xdc, timing_paths_routed, TimingPath};
use std::collections::HashMap;
use std::path::Path;

fn err(line: usize, msg: impl Into<String>) -> Ctl {
    Ctl::Error(TclError {
        msg: msg.into(),
        line,
    })
}

/// Host (design/flow/console) command names, shared by `info commands`,
/// `help` topics and the unknown-command suggestion.
pub const HOST_COMMANDS: &[&str] = &[
    "help",
    "clear",
    "get_cells",
    "get_nets",
    "get_pins",
    "get_ports",
    "get_clocks",
    "get_timing_paths",
    "get_property",
    "set_property",
    "report_property",
    "list_property",
    "list_properties",
    "select_objects",
    "highlight_objects",
    "get_selected_objects",
    "current_design",
    "synth_design",
    "read_sv",
    "read_verilog",
    "read_vhdl",
    "opt_design",
    "place_design",
    "route_design",
    "write_bitstream",
    "write_checkpoint",
    "create_clock",
    "report_timing",
    "report_utilization",
    "mark_debug",
    "add_probe",
    "eco",
    "open_hw_manager",
    "program_hw",
    "program_hw_devices",
    "report_die",
    "report_hw_targets",
    "report_featuremap",
    "write_hnf",
    "hds::synth",
    "hds::impl",
    "hds::flow",
    "hds::tree",
];

/// Levenshtein distance over chars (small strings only).
fn edit_dist(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (x, ca) in a.iter().enumerate() {
        let mut prev = x + 1;
        for (y, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let cur = (row[y + 1] + 1).min((prev + 1).min(row[y] + cost));
            row[y] = prev;
            prev = cur;
        }
        row[b.len()] = prev;
    }
    row[b.len()]
}

/// Up to 3 "did you mean …?" candidates for an unknown command name:
/// prefix matches first, then close edit-distance matches.
fn suggest_commands(i: &TclInterp, name: &str) -> Vec<String> {
    let mut cands: Vec<&str> = Vec::new();
    cands.extend(super::interp::CORE_COMMANDS.iter().copied());
    cands.extend(HOST_COMMANDS.iter().copied());
    let procs = i.proc_names();
    cands.extend(procs.iter().map(|s| s.as_str()));
    let mut scored: Vec<(usize, &str)> = cands
        .iter()
        .filter(|c| **c != name)
        .map(|c| {
            let d = if c.starts_with(name) || name.starts_with(c) {
                0
            } else {
                edit_dist(name, c)
            };
            (d, *c)
        })
        .filter(|(d, _)| *d <= 3)
        .collect();
    scored.sort();
    scored.dedup();
    scored.into_iter().take(3).map(|(_, c)| c.to_string()).collect()
}

/// Tcl-style unknown-command error with suggestions (single line, no panic).
fn unknown_command(i: &TclInterp, name: &str, line: usize) -> Ctl {
    let mut msg = format!("invalid command name \"{name}\"");
    let sugg = suggest_commands(i, name);
    if !sugg.is_empty() {
        msg.push_str(&format!(
            "; did you mean {}?",
            sugg.iter()
                .map(|s| format!("\"{s}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    err(line, msg)
}

fn design<'a>(shell: &'a GpuiShell, cmd: &str, line: usize) -> Result<&'a Design, TclError> {
    shell.session.design.as_ref().ok_or_else(|| TclError {
        msg: format!("{cmd}: no design (synth_design first)"),
        line,
    })
}

// ---- glob ----

/// Tcl/Vivado glob: `*`, `?`, `[seq]`, `[a-z]`, `\` escapes.
pub fn glob_match(pat: &str, s: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pat.chars().collect(), s.chars().collect());
    let (mut pi, mut si) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while si < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '[' {
            match match_class(&p, &mut pi) {
                Some(set) => {
                    if set.contains(&t[si]) {
                        si += 1;
                    } else {
                        return false;
                    }
                }
                None => return false,
            }
        } else if pi < p.len() && p[pi] == '\\' && pi + 1 < p.len() {
            if p[pi + 1] == t[si] {
                pi += 2;
                si += 1;
            } else {
                return false;
            }
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Match a `[...]` class at `p[*pi]`; advances `*pi` past `]`.
fn match_class(p: &[char], pi: &mut usize) -> Option<Vec<char>> {
    // p[*pi] == '['
    let mut k = *pi + 1;
    let mut set = Vec::new();
    let mut ok = false;
    while k < p.len() {
        if p[k] == ']' && ok {
            *pi = k + 1;
            return Some(set);
        }
        ok = true;
        if p[k] == '\\' && k + 1 < p.len() {
            set.push(p[k + 1]);
            k += 2;
        } else if k + 2 < p.len() && p[k + 1] == '-' && p[k + 2] != ']' {
            for c in (p[k] as u32)..=(p[k + 2] as u32) {
                set.push(char::from_u32(c).unwrap_or('?'));
            }
            k += 3;
        } else {
            set.push(p[k]);
            k += 1;
        }
    }
    None
}

// ---- properties ----

fn cell_type(kind: &CellKind) -> &'static str {
    match kind {
        CellKind::Lut6 { .. } => "lut",
        CellKind::Hff => "ff",
        CellKind::IobOut => "iob",
        CellKind::IobIn => "iob",
        CellKind::ClockBuf => "bufg",
        CellKind::Mac27 => "dsp",
        CellKind::Bram18 => "bram",
        CellKind::BlackBox { .. } => "blackbox",
        CellKind::Ila { .. } => "ila",
    }
}

/// Max sink count over nets the cell drives (for the FANOUT property).
fn cell_fanouts(d: &Design) -> HashMap<String, usize> {
    let mut m: HashMap<String, usize> = HashMap::new();
    for net in &d.nets {
        let sinks = net.endpoints.len().saturating_sub(1);
        for ep in &net.endpoints {
            if matches!(ep.pin.as_str(), "O" | "Q" | "PAD") {
                let e = m.entry(ep.cell.clone()).or_insert(0);
                *e = (*e).max(sinks);
            }
        }
    }
    m
}

fn cell_props(
    shell: &GpuiShell,
    cell: &Cell,
    fanouts: &HashMap<String, usize>,
) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("NAME".into(), cell.name.clone());
    m.insert("TYPE".into(), cell_type(&cell.kind).into());
    if let CellKind::Lut6 { init } = cell.kind {
        m.insert("INIT".into(), format!("{init:#x}"));
    }
    if let CellKind::BlackBox { ref module } = cell.kind {
        m.insert("MODULE".into(), module.clone());
    }
    if let Some(placed) = shell.session.placed.as_ref() {
        if let Some(bel) = placed.cell_bel(&cell.name) {
            m.insert("SITE".into(), bel.site.id());
            m.insert("BEL".into(), bel.id());
        }
    }
    m.insert(
        "FANOUT".into(),
        fanouts.get(&cell.name).copied().unwrap_or(0).to_string(),
    );
    for (k, v) in &cell.attrs.map {
        m.insert(k.to_uppercase(), v.clone());
    }
    m
}

fn net_props(net: &Net) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("NAME".into(), net.name.clone());
    m.insert(
        "FANOUT".into(),
        net.endpoints.len().saturating_sub(1).to_string(),
    );
    let driver = net
        .endpoints
        .iter()
        .find(|ep| matches!(ep.pin.as_str(), "O" | "Q" | "PAD"))
        .map(|ep| ep.cell.clone())
        .unwrap_or_default();
    m.insert("DRIVER".into(), driver);
    m.insert(
        "TYPE".into(),
        net.attrs.get("TYPE").unwrap_or("").to_string(),
    );
    for (k, v) in &net.attrs.map {
        m.insert(k.to_uppercase(), v.clone());
    }
    m
}

fn pin_dir(kind: &CellKind, pin: &str) -> &'static str {
    match pin {
        "O" | "Q" => "OUT",
        "PAD" => match kind {
            CellKind::IobOut => "OUT",
            _ => "IN",
        },
        _ => "IN",
    }
}

fn pin_props(d: &Design, cell: &Cell, pin: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("NAME".into(), format!("{}/{}", cell.name, pin));
    m.insert("CELL".into(), cell.name.clone());
    m.insert("PIN".into(), pin.to_string());
    m.insert("DIRECTION".into(), pin_dir(&cell.kind, pin).into());
    let net = d
        .pin_index()
        .net_on(&cell.name, pin)
        .unwrap_or("")
        .to_string();
    m.insert("NET".into(), net);
    m
}

fn port_props(d: &Design, port: &helion_ir::Port) -> HashMap<String, String> {
    let _ = d;
    let mut m = HashMap::new();
    m.insert("NAME".into(), port.name.clone());
    m.insert(
        "DIRECTION".into(),
        match port.dir {
            PortDir::In => "IN",
            PortDir::Out => "OUT",
            PortDir::Inout => "INOUT",
        }
        .into(),
    );
    for key in ["PACKAGE_PIN", "IOSTANDARD", "DRIVE", "SLEW", "PULLTYPE"] {
        m.insert(
            key.into(),
            port.attrs.get(key).unwrap_or("").to_string(),
        );
    }
    for (k, v) in &port.attrs.map {
        m.insert(k.to_uppercase(), v.clone());
    }
    m
}

fn clock_props(c: &helion_sta::Clock) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("NAME".into(), c.name.clone());
    m.insert("PERIOD".into(), c.period_ps.to_string());
    m.insert("SOURCE".into(), c.source.clone());
    m
}

fn path_props(p: &TimingPath) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("NAME".into(), p.name.clone());
    m.insert("STARTPOINT".into(), p.startpoint.cell.clone());
    m.insert("ENDPOINT".into(), p.endpoint.cell.clone());
    m.insert("SLACK".into(), p.slack_ps.to_string());
    m.insert("DELAY".into(), p.delay_ps.to_string());
    m.insert("START_SITE".into(), p.startpoint.site_id.clone());
    m.insert("END_SITE".into(), p.endpoint.site_id.clone());
    m.insert("CELLS".into(), p.cells.len().to_string());
    m.insert("NETS".into(), p.nets.len().to_string());
    m
}

// ---- object resolution ----

#[derive(Clone, Debug, PartialEq)]
pub enum ObjKind {
    Cell,
    Net,
    Pin,
    Port,
    Clock,
    Path,
}

/// Resolve a name to an object. Search order: cells, nets, pins (`a/b`),
/// ports, clocks, then (optionally) timing paths (runs STA).
fn resolve_obj(
    i: &mut TclInterp,
    shell: &mut GpuiShell,
    name: &str,
    line: usize,
    allow_timing: bool,
) -> Result<(ObjKind, HashMap<String, String>), TclError> {
    resolve_candidates(i, shell, name, line, allow_timing)?
        .into_iter()
        .next()
        .ok_or_else(|| TclError {
            msg: format!("no such object \"{name}\""),
            line,
        })
}

/// One resolved object: its kind plus its property map.
type ObjCandidate = (ObjKind, HashMap<String, String>);

/// All objects matching `name`, in `resolve_obj` priority order. Used by
/// `get_property`, which picks the first candidate that actually HAS the
/// requested property (so `PERIOD` finds a clock even when a same-named
/// port shadows it, and `DIRECTION` still finds the port).
fn resolve_candidates(
    i: &mut TclInterp,
    shell: &mut GpuiShell,
    name: &str,
    line: usize,
    allow_timing: bool,
) -> Result<Vec<ObjCandidate>, TclError> {
    let d = design(shell, "get_property", line)?;
    let mut out = Vec::new();
    if let Some(cell) = d.cells.iter().find(|c| c.name == name) {
        let fanouts = cell_fanouts(d);
        out.push((ObjKind::Cell, cell_props(shell, cell, &fanouts)));
    }
    if let Some(net) = d.nets.iter().find(|n| n.name == name) {
        out.push((ObjKind::Net, net_props(net)));
    }
    if let Some((cell_name, pin)) = name.split_once('/') {
        if let Some(cell) = d.cells.iter().find(|c| c.name == cell_name) {
            if d.pin_index().net_on(cell_name, pin).is_some() {
                out.push((ObjKind::Pin, pin_props(d, cell, pin)));
            }
        }
    }
    if let Some(port) = d.ports.iter().find(|p| p.name == name) {
        out.push((ObjKind::Port, port_props(d, port)));
    }
    if let Some(c) = i.clocks.clocks.iter().find(|c| c.name == name) {
        out.push((ObjKind::Clock, clock_props(c)));
    }
    if allow_timing {
        let paths = match ensure_timing(i, shell, line) {
            Ok((_, _, p)) => p,
            Err(_) => Vec::new(),
        };
        if let Some(p) = paths.iter().find(|p| p.name == name) {
            out.push((ObjKind::Path, path_props(p)));
        }
    }
    Ok(out)
}

// ---- get_* options ----

struct GetOpts {
    filter: Option<String>,
    of_objects: Option<String>,
    patterns: Vec<String>,
    max_paths: Option<usize>,
}

fn parse_get_opts(argv: &[String], line: usize, cmd: &str) -> Result<GetOpts, TclError> {
    let mut o = GetOpts {
        filter: None,
        of_objects: None,
        patterns: Vec::new(),
        max_paths: None,
    };
    let mut k = 1;
    while k < argv.len() {
        match argv[k].as_str() {
            "-filter" => {
                k += 1;
                o.filter = Some(argv.get(k).cloned().ok_or_else(|| TclError {
                    msg: format!("{cmd}: -filter needs an argument"),
                    line,
                })?);
            }
            "-of_objects" => {
                k += 1;
                o.of_objects = Some(argv.get(k).cloned().ok_or_else(|| TclError {
                    msg: format!("{cmd}: -of_objects needs an argument"),
                    line,
                })?);
            }
            "-max_paths" => {
                k += 1;
                let n: usize = argv
                    .get(k)
                    .ok_or_else(|| TclError {
                        msg: format!("{cmd}: -max_paths needs an argument"),
                        line,
                    })?
                    .parse()
                    .map_err(|_| TclError {
                        msg: format!("{cmd}: -max_paths must be an integer"),
                        line,
                    })?;
                o.max_paths = Some(n);
            }
            "-hierarchical" => {} // accepted; Helion names are flat
            s if s.starts_with('-') => {
                return Err(TclError {
                    msg: format!("{cmd}: unknown option \"{s}\""),
                    line,
                })
            }
            _ => break,
        }
        k += 1;
    }
    o.patterns = argv[k..].to_vec();
    Ok(o)
}

fn apply_patterns(names: &mut Vec<String>, patterns: &[String]) {
    if !patterns.is_empty() {
        names.retain(|n| patterns.iter().any(|p| glob_match(p, n)));
    }
}

/// Expand an `-of_objects` argument (a list of object names) into cells.
fn cells_of(d: &Design, of: &str, line: usize) -> Result<Vec<String>, TclError> {
    let objs = split_list(of).map_err(|m| TclError {
        msg: format!("-of_objects: {m}"),
        line,
    })?;
    let mut out = Vec::new();
    for o in objs {
        if let Some((cell, _pin)) = o.split_once('/') {
            if d.cells.iter().any(|c| c.name == cell) {
                out.push(cell.to_string());
            }
        } else if d.cells.iter().any(|c| c.name == o) {
            out.push(o);
        } else if let Some(net) = d.nets.iter().find(|n| n.name == o) {
            for ep in &net.endpoints {
                if !out.contains(&ep.cell) {
                    out.push(ep.cell.clone());
                }
            }
        } else {
            return Err(TclError {
                msg: format!("-of_objects: no such object \"{o}\""),
                line,
            });
        }
    }
    Ok(out)
}

/// Expand `-of_objects` into nets.
fn nets_of(d: &Design, of: &str, line: usize) -> Result<Vec<String>, TclError> {
    let objs = split_list(of).map_err(|m| TclError {
        msg: format!("-of_objects: {m}"),
        line,
    })?;
    let idx = d.pin_index();
    let mut out = Vec::new();
    for o in objs {
        if let Some((cell, pin)) = o.split_once('/') {
            match idx.net_on(cell, pin) {
                Some(n) if !n.is_empty() && !out.contains(&n.to_string()) => {
                    out.push(n.to_string())
                }
                _ => {}
            }
        } else if d.cells.iter().any(|c| c.name == o) {
            for net in &d.nets {
                if net.endpoints.iter().any(|ep| ep.cell == o) && !out.contains(&net.name) {
                    out.push(net.name.clone());
                }
            }
        } else if d.nets.iter().any(|n| n.name == o) {
            out.push(o);
        } else {
            return Err(TclError {
                msg: format!("-of_objects: no such object \"{o}\""),
                line,
            });
        }
    }
    Ok(out)
}

/// Expand `-of_objects` into pins (`cell/pin`).
fn pins_of(d: &Design, of: &str, line: usize) -> Result<Vec<String>, TclError> {
    let objs = split_list(of).map_err(|m| TclError {
        msg: format!("-of_objects: {m}"),
        line,
    })?;
    let mut out = Vec::new();
    for o in objs {
        if o.contains('/') {
            out.push(o);
        } else if d.cells.iter().any(|c| c.name == o) {
            out.extend(helion_proj::get_pins(d, &o));
        } else if let Some(net) = d.nets.iter().find(|n| n.name == o) {
            for ep in &net.endpoints {
                out.push(format!("{}/{}", ep.cell, ep.pin));
            }
        } else {
            return Err(TclError {
                msg: format!("-of_objects: no such object \"{o}\""),
                line,
            });
        }
    }
    Ok(out)
}

// ---- timing ----

/// Run the implementation flow if needed, then STA with the console's
/// `create_clock` state (empty = the 10 ns `clk` fallback, exactly like
/// `Session::report_timing`). Same engine as the timing view:
/// `helion_sta::report_timing_routed_xdc`.
fn ensure_timing(
    i: &mut TclInterp,
    shell: &mut GpuiShell,
    line: usize,
) -> Result<(String, helion_sta::TimingResult, Vec<TimingPath>), TclError> {
    let map = |msg: String| TclError { msg, line };
    impl_if_needed(shell).map_err(map)?;
    let (clks, xdc) = if i.clocks.clocks.is_empty() {
        let mut c = Vec::new();
        create_clock(&mut c, "clk", 10_000, "clk");
        (c, helion_sta::Constraints::default())
    } else {
        (i.clocks.clocks.clone(), i.clocks.clone())
    };
    let d = design(shell, "report_timing", line)?;
    let r = shell.session.routed.as_ref().ok_or_else(|| TclError {
        msg: "report_timing: not routed".into(),
        line,
    })?;
    let t = report_timing_routed_xdc(d, r, &clks, &xdc).map_err(map)?;
    let paths = timing_paths_routed(d, r, &t);
    Ok((d.name.clone(), t, paths))
}

/// Vivado-style timing table: summary line plus the worst paths, so the
/// console output pane shows a readable report instead of a key=value dump.
fn format_timing_table(
    name: &str,
    t: &helion_sta::TimingResult,
    paths: &[TimingPath],
) -> String {
    fn ns(ps: i64) -> String {
        format!("{:.3}ns", ps as f64 / 1000.0)
    }
    let mut s = format!(
        "Timing Report: {name}\n\
         WNS {wns} ({wns_ps}ps)   TNS {tns} ({tns_ps}ps)   endpoints {ep}\n\
         Setup {setup} ({setup_ps}ps)   Hold {hold} ({hold_ps}ps)   Hold slack {hslack} ({hslack_ps}ps)\n\
         Delay model: reg2reg {r2r}ps  iob {iob}ps  route {route}ps\n",
        wns = ns(t.wns_ps),
        wns_ps = t.wns_ps,
        tns = ns(t.tns_ps),
        tns_ps = t.tns_ps,
        ep = t.endpoints,
        setup = ns(t.setup_ps),
        setup_ps = t.setup_ps,
        hold = ns(t.hold_ps),
        hold_ps = t.hold_ps,
        hslack = ns(t.hold_slack_ps),
        hslack_ps = t.hold_slack_ps,
        r2r = t.r2r_ps,
        iob = t.iob_ps,
        route = t.route_ps,
    );
    let show = paths.len().min(10);
    if show == 0 {
        s.push_str("No timing paths.\n");
        return s;
    }
    s.push_str(&format!(
        "\n{:<12} {:>12} {:>12}  {:<28} {:<28}\n",
        "Slack", "Slack(ps)", "Delay(ps)", "Startpoint", "Endpoint"
    ));
    for p in &paths[..show] {
        s.push_str(&format!(
            "{:<12} {:>12} {:>12}  {:<28} {:<28}\n",
            ns(p.slack_ps),
            p.slack_ps,
            p.delay_ps,
            truncate(&p.startpoint.cell, 28),
            truncate(&p.endpoint.cell, 28),
        ));
    }
    if paths.len() > show {
        s.push_str(&format!("… {} more paths (get_timing_paths -max_paths N)\n", paths.len() - show));
    }
    s
}

fn truncate(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(w - 1).collect();
        t.push('…');
        t
    }
}

/// Vivado-style utilization table instead of the single-line
/// `Session::report_utilization` dump.
fn format_util_table(shell: &GpuiShell, dev: &Device) -> Result<String, String> {
    let p = shell
        .session
        .placed
        .as_ref()
        .map(|pl| &pl.packed)
        .or(shell.session.packed.as_ref())
        .ok_or_else(|| "report_utilization: not packed".to_string())?;
    let name = shell
        .session
        .design
        .as_ref()
        .map(|d| d.name.as_str())
        .unwrap_or("?");
    let rows: [(&str, usize, u64); 4] = [
        ("LUTFF", p.lutffs.len(), u64::from(dev.lut6_count())),
        ("IOB", p.iobs.len(), dev.iob_sites().count() as u64),
        ("BRAM", p.brams.len(), u64::from(dev.n_bram)),
        ("DSP", p.macs.len(), u64::from(dev.n_dsp)),
    ];
    let mut s = format!("Utilization: {name}  (part {})\n", dev.part);
    s.push_str(&format!(
        "{:<10} {:>8} {:>8} {:>7}\n",
        "Resource", "Used", "Total", "Pct"
    ));
    for (r, used, total) in rows {
        let pct = if total == 0 {
            "—".to_string()
        } else {
            format!("{:.1}%", used as f64 * 100.0 / total as f64)
        };
        s.push_str(&format!("{r:<10} {used:>8} {total:>8} {pct:>7}\n"));
    }
    Ok(s)
}

/// Dispatch a non-core command against the design database / flow engines.
pub fn eval_host(
    i: &mut TclInterp,
    name: &str,
    argv: &[String],
    line: usize,
    shell: &mut GpuiShell,
) -> Ctl {
    match name {
        "help" => cmd_help(i, argv, line),
        "clear" => {
            if argv.len() != 1 {
                return err(line, "wrong # args: should be \"clear\"");
            }
            i.clear_console = true;
            Ctl::Value("console cleared".into())
        }
        "get_cells" => cmd_get_cells(i, argv, line, shell),
        "get_nets" => cmd_get_nets(i, argv, line, shell),
        "get_pins" => cmd_get_pins(i, argv, line, shell),
        "get_ports" => cmd_get_ports(i, argv, line, shell),
        "get_clocks" => cmd_get_clocks(i, argv, line, shell),
        "get_timing_paths" => cmd_get_timing_paths(i, argv, line, shell),
        "get_property" => cmd_get_property(i, argv, line, shell),
        "set_property" => cmd_set_property(i, argv, line, shell),
        "report_property" => cmd_report_property(i, argv, line, shell),
        "list_property" | "list_properties" => cmd_list_property(i, argv, line, shell),
        "select_objects" => cmd_select_objects(i, argv, line, shell, false),
        "highlight_objects" => cmd_select_objects(i, argv, line, shell, true),
        "get_selected_objects" => Ctl::Value(join_list(
            &i.selected.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
        )),
        "current_design" => match design(shell, "current_design", line) {
            Ok(d) => Ctl::Value(d.name.clone()),
            Err(e) => Ctl::Error(e),
        },
        "synth_design" | "read_sv" | "read_verilog" | "read_vhdl" | "hds::synth" => {
            cmd_synth(i, argv, line, shell)
        }
        "opt_design" => {
            let d = match shell.session.design.as_mut() {
                Some(d) => d,
                None => return err(line, "opt_design: no design"),
            };
            let n = opt_design(d);
            Ctl::Value(format!("opt removed {n} dead LUTFF"))
        }
        "place_design" => {
            let dev = match Device::load_part(&shell.part) {
                Ok(d) => d,
                Err(e) => return err(line, e),
            };
            if let Err(e) = shell.session.place_design(&dev) {
                return err(line, e);
            }
            let n = shell
                .session
                .placed
                .as_ref()
                .map(|p| p.lutff_sites.len())
                .unwrap_or(0);
            Ctl::Value(format!("place_design sites {n}"))
        }
        "route_design" => {
            let dev = match Device::load_part(&shell.part) {
                Ok(d) => d,
                Err(e) => return err(line, e),
            };
            if shell.session.placed.is_none() {
                if let Err(e) = shell.session.place_design(&dev) {
                    return err(line, e);
                }
            }
            if let Err(e) = shell.session.route_design(&dev) {
                return err(line, e);
            }
            let hops = shell
                .session
                .routed
                .as_ref()
                .and_then(|r| r.iob_src.first())
                .map(|io| io.hops)
                .unwrap_or(0);
            Ctl::Value(format!("route_design hops {hops}"))
        }
        "write_bitstream" => {
            if let Err(e) = impl_if_needed(shell) {
                return err(line, e);
            }
            let n = shell
                .session
                .bitstream
                .as_ref()
                .map(|b| b.packets.len())
                .unwrap_or(0);
            Ctl::Value(format!("write_bitstream {n} bytes"))
        }
        "write_checkpoint" => {
            let ck = shell.session.checkpoint();
            Ctl::Value(format!(
                "write_checkpoint {} bytes hash {:#x}",
                ck.len(),
                shell.session.blinky_hash().unwrap_or(0)
            ))
        }
        "create_clock" => cmd_create_clock(i, argv, line),
        "report_timing" => match ensure_timing(i, shell, line) {
            Ok((name, t, paths)) => Ctl::Value(format_timing_table(&name, &t, &paths)),
            Err(e) => Ctl::Error(e),
        },
        "report_utilization" => {
            if let Err(e) = impl_if_needed(shell) {
                return err(line, e);
            }
            let dev = match Device::load_part(&shell.part) {
                Ok(d) => d,
                Err(e) => return err(line, e),
            };
            match format_util_table(shell, &dev) {
                Ok(s) => Ctl::Value(s),
                Err(e) => err(line, e),
            }
        }
        "mark_debug" => {
            let net = match argv.get(1) {
                Some(n) => n.clone(),
                None => return err(line, "wrong # args: should be \"mark_debug net\""),
            };
            match shell.session.mark_debug(&net) {
                Ok(()) => Ctl::Value(format!("mark_debug {net}")),
                Err(e) => err(line, e),
            }
        }
        "add_probe" => {
            let net = match argv.get(1) {
                Some(n) => n.clone(),
                None => return err(line, "wrong # args: should be \"add_probe net\""),
            };
            match shell.session.mark_debug(&net) {
                Ok(()) => Ctl::Value(format!("add_probe {net}")),
                Err(e) => err(line, e),
            }
        }
        "eco" => {
            let cell = argv.get(1).map(|s| s.as_str()).unwrap_or("u_lut");
            let init_s = argv
                .get(2)
                .map(|s| s.as_str())
                .unwrap_or("0xAAAAAAAAAAAAAAAA");
            let init = u64::from_str_radix(
                init_s.trim_start_matches("0x").trim_start_matches("0X"),
                16,
            )
            .unwrap_or(0xAAAA_AAAA_AAAA_AAAA);
            let dev = match Device::load_part(&shell.part) {
                Ok(d) => d,
                Err(e) => return err(line, e),
            };
            if let Err(e) = impl_if_needed(shell) {
                return err(line, e);
            }
            match shell.session.eco(&dev, cell, init) {
                Ok(()) => Ctl::Value(format!("eco {cell} {init:#x}")),
                Err(e) => err(line, e),
            }
        }
        "open_hw_manager" => {
            shell.session.open_hw_manager();
            Ctl::Value("open_hw_manager sim".into())
        }
        "program_hw" | "program_hw_devices" => {
            let dev = match Device::load_part(&shell.part) {
                Ok(d) => d,
                Err(e) => return err(line, e),
            };
            if shell.session.bitstream.is_none() {
                if let Err(e) = impl_if_needed(shell) {
                    return err(line, e);
                }
            }
            if !shell.session.hw_open {
                shell.session.open_hw_manager();
            }
            let cable = argv
                .iter()
                .skip(1)
                .find_map(|a| a.strip_prefix("cable="))
                .unwrap_or("auto");
            match shell.session.program_hw_cable(&dev, cable) {
                Ok(s) => Ctl::Value(s),
                Err(e) => err(line, e),
            }
        }
        "report_die" | "report_hw_targets" => {
            let dev = match Device::load_part(&shell.part) {
                Ok(d) => d,
                Err(e) => return err(line, e),
            };
            Ctl::Value(dev.report_die())
        }
        "report_featuremap" => {
            let dev = match Device::load_part(&shell.part) {
                Ok(d) => d,
                Err(e) => return err(line, e),
            };
            Ctl::Value(dev.report_featuremap())
        }
        "write_hnf" => match design(shell, "write_hnf", line) {
            Ok(d) => Ctl::Value(d.to_hnf()),
            Err(e) => Ctl::Error(e),
        },
        "hds::impl" => {
            if let Err(e) = impl_if_needed(shell) {
                return err(line, e);
            }
            let n = shell
                .session
                .bitstream
                .as_ref()
                .map(|b| b.frames.len())
                .unwrap_or(0);
            Ctl::Value(format!("impl frames {n}"))
        }
        "hds::flow" => Ctl::Value(shell.flow.steps.join(" ")),
        "hds::tree" => Ctl::Value(shell.tree.nodes.join(" ")),
        _ => unknown_command(i, name, line),
    }
}

/// `help ?topic?`: overview with no args, per-command usage otherwise.
fn cmd_help(i: &mut TclInterp, argv: &[String], line: usize) -> Ctl {
    match argv.len() {
        1 => Ctl::Value(help_overview()),
        2 => {
            let topic = argv[1].as_str();
            if let Some((usage, desc)) = help_topic(topic) {
                return Ctl::Value(format!("{usage}\n  {desc}"));
            }
            // User procs get help from their definition.
            if let Some(params) = i.proc_params(topic) {
                let plist: Vec<String> = params
                    .iter()
                    .map(|(n, d)| match d {
                        Some(d) => format!("{{{n} {d}}}"),
                        None => n.clone(),
                    })
                    .collect();
                return Ctl::Value(format!(
                    "proc {topic} {{{}}}\n  user procedure (info body {topic} for source)",
                    plist.join(" ")
                ));
            }
            let mut msg = format!("no help topic \"{topic}\"");
            let sugg = suggest_commands(i, topic);
            if !sugg.is_empty() {
                msg.push_str(&format!(
                    "; did you mean {}?",
                    sugg.iter()
                        .map(|s| format!("\"{s}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            err(line, msg)
        }
        _ => err(line, "wrong # args: should be \"help ?topic?\""),
    }
}

/// (name, usage, one-line description) for every host command.
fn help_topic(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "help" => (
            "help ?topic?",
            "this overview, or usage for one command",
        ),
        "clear" => ("clear", "clear the console transcript"),
        "get_cells" => (
            "get_cells ?-hierarchical? ?-filter expr? ?-of_objects objs? ?-quiet? ?patterns...?",
            "cells in the netlist (u_lut0, u_ff3…); -filter {TYPE == lut}",
        ),
        "get_nets" => (
            "get_nets ?-filter expr? ?-of_objects objs? ?patterns...?",
            "nets in the netlist",
        ),
        "get_pins" => (
            "get_pins ?-of_objects cells? ?patterns...?",
            "pins as cell/pin (u_lut0/A)",
        ),
        "get_ports" => ("get_ports ?patterns...?", "top-level ports"),
        "get_clocks" => (
            "get_clocks ?patterns...?",
            "clocks from create_clock (empty until constrained)",
        ),
        "get_timing_paths" => (
            "get_timing_paths ?-max_paths N? ?-filter expr? ?patterns...?",
            "timing path endpoints; -filter {SLACK < 0} finds violations",
        ),
        "get_property" => (
            "get_property ?-quiet? property object",
            "one property (TYPE, NAME, SLACK, PERIOD, SITE…)",
        ),
        "set_property" => (
            "set_property ?-quiet? name value objects…",
            "DONT_TOUCH KEEP MARK_DEBUG LOC PACKAGE_PIN IOSTANDARD DRIVE",
        ),
        "report_property" => (
            "report_property object",
            "all properties of an object as a table",
        ),
        "list_property" | "list_properties" => (
            "list_property object",
            "property names defined on an object",
        ),
        "select_objects" => (
            "select_objects objects",
            "select cells/nets/pins in the IDE views",
        ),
        "highlight_objects" => (
            "highlight_objects objects",
            "highlight objects (stronger than select)",
        ),
        "get_selected_objects" => (
            "get_selected_objects",
            "names currently selected via select_objects",
        ),
        "current_design" => ("current_design", "name of the loaded design"),
        "synth_design" | "read_sv" => (
            "synth_design ?file…? | read_sv file",
            "synthesize SystemVerilog into the netlist",
        ),
        "read_verilog" | "read_vhdl" => (
            "read_verilog|read_vhdl file",
            "synth alias (Vivado compat)",
        ),
        "opt_design" => ("opt_design", "dead-code sweep on the netlist"),
        "place_design" => ("place_design", "pack + PathFinder place"),
        "route_design" => ("route_design", "route the placed design"),
        "write_bitstream" => ("write_bitstream", "generate the bitstream"),
        "write_checkpoint" => (
            "write_checkpoint",
            "snapshot the session (size + hash)",
        ),
        "create_clock" => (
            "create_clock -period <ns> ?-name n? ?-waveform {r f}? objects",
            "define a clock for STA (period in ns)",
        ),
        "report_timing" => (
            "report_timing",
            "STA table: WNS/TNS plus the worst paths",
        ),
        "report_utilization" => (
            "report_utilization",
            "resource usage table vs the target die",
        ),
        "mark_debug" | "add_probe" => (
            "mark_debug net",
            "mark a net for ILA-style probing",
        ),
        "eco" => (
            "eco ?cell? ?init?",
            "engineering change: rewrite one LUT init",
        ),
        "open_hw_manager" => ("open_hw_manager", "open the HW manager (sim)"),
        "program_hw" | "program_hw_devices" => (
            "program_hw ?cable=id?",
            "program the device (sim fabric by default)",
        ),
        "report_die" | "report_hw_targets" => (
            "report_die",
            "target die geometry and capacity",
        ),
        "report_featuremap" => ("report_featuremap", "device feature map"),
        "write_hnf" => ("write_hnf", "dump the netlist in HNF text form"),
        "hds::synth" => ("hds::synth", "Helion flow: synthesize"),
        "hds::impl" => ("hds::impl", "Helion flow: implement through bitstream"),
        "hds::flow" => ("hds::flow", "flow step names"),
        "hds::tree" => ("hds::tree", "source tree entries"),
        // core language
        "set" => ("set var ?value?", "read/write a variable"),
        "unset" => ("unset ?-nocomplain? var…", "delete variables"),
        "puts" => ("puts ?-nonewline? string", "write to the console"),
        "expr" => ("expr arg…", "evaluate a math/logic expression"),
        "if" => (
            "if cond body ?elseif c2 b2…? ?else? body",
            "conditional",
        ),
        "foreach" => ("foreach var list body", "loop over a list"),
        "for" => ("for init test next body", "C-style loop"),
        "while" => ("while test body", "loop while true"),
        "proc" => ("proc name params body", "define a procedure"),
        "return" => ("return ?value?", "return from a procedure"),
        "break" => ("break", "exit the innermost loop"),
        "continue" => ("continue", "next loop iteration"),
        "list" => ("list ?values…?", "build a list"),
        "llength" => ("llength list", "number of list elements"),
        "lindex" => ("lindex list i", "element i (supports end-N)"),
        "lrange" => ("lrange list a b", "sub-list from a to b"),
        "lappend" => ("lappend var ?values…?", "append to a list variable"),
        "concat" => ("concat ?lists…?", "join lists"),
        "join" => ("join list ?sep?", "join elements with sep"),
        "split" => ("split string ?seps?", "split into a list"),
        "source" => ("source file", "evaluate a Tcl script file"),
        "catch" => ("catch script ?var?", "trap errors, Tcl-style"),
        "error" => ("error msg", "raise an error"),
        "eval" => ("eval arg…", "evaluate args as a script"),
        "subst" => ("subst string", "perform substitutions"),
        "format" => ("format fmt args…", "%s %d %f %x subset"),
        "incr" => ("incr var ?n?", "increment an integer variable"),
        "append" => ("append var ?values…?", "append to a variable"),
        "string" => (
            "string op ?args…?",
            "length|range|index|match|tolower|toupper|trim|first|equal",
        ),
        "info" => (
            "info exists|vars|procs|commands|args|body …",
            "introspection",
        ),
        _ => return None,
    })
}

fn help_overview() -> String {
    [
        "Helion Tcl console. Core: set puts expr if foreach for while proc return break continue",
        "  list llength lindex lrange lappend concat join split string info catch error eval subst source format incr append",
        "Objects: get_cells get_nets get_pins get_ports get_clocks get_timing_paths [-filter expr] [-of_objects objs] [patterns]",
        "  get_property prop obj | set_property name value objs | report_property obj | list_property obj",
        "  select_objects highlight_objects get_selected_objects | current_design | clear",
        "Flow: synth_design|read_sv file opt_design place_design route_design write_bitstream write_checkpoint",
        "  create_clock report_timing report_utilization mark_debug eco open_hw_manager program_hw report_die report_featuremap",
        "help <command> for usage. See docs/tcl-subset.md for the supported subset.",
    ]
    .join("\n")
}

fn cmd_get_cells(
    _i: &mut TclInterp,
    argv: &[String],
    line: usize,
    shell: &mut GpuiShell,
) -> Ctl {
    let opts = match parse_get_opts(argv, line, "get_cells") {
        Ok(o) => o,
        Err(e) => return Ctl::Error(e),
    };
    let d = match design(shell, "get_cells", line) {
        Ok(d) => d,
        Err(e) => return Ctl::Error(e),
    };
    let mut names: Vec<String> = match &opts.of_objects {
        Some(of) => match cells_of(d, of, line) {
            Ok(n) => n,
            Err(e) => return Ctl::Error(e),
        },
        None => d.cells.iter().map(|c| c.name.clone()).collect(),
    };
    apply_patterns(&mut names, &opts.patterns);
    if let Some(f) = &opts.filter {
        let fanouts = cell_fanouts(d);
        let mut kept = Vec::new();
        for n in &names {
            if let Some(cell) = d.cells.iter().find(|c| &c.name == n) {
                let props = cell_props(shell, cell, &fanouts);
                match eval_filter(f, &props) {
                    Ok(true) => kept.push(n.clone()),
                    Ok(false) => {}
                    Err(m) => return err(line, format!("-filter: {m}")),
                }
            }
        }
        names = kept;
    }
    Ctl::Value(join_list(&names))
}

fn cmd_get_nets(
    i: &mut TclInterp,
    argv: &[String],
    line: usize,
    shell: &mut GpuiShell,
) -> Ctl {
    let _ = i;
    let opts = match parse_get_opts(argv, line, "get_nets") {
        Ok(o) => o,
        Err(e) => return Ctl::Error(e),
    };
    let d = match design(shell, "get_nets", line) {
        Ok(d) => d,
        Err(e) => return Ctl::Error(e),
    };
    let mut names: Vec<String> = match &opts.of_objects {
        Some(of) => match nets_of(d, of, line) {
            Ok(n) => n,
            Err(e) => return Ctl::Error(e),
        },
        None => d.nets.iter().map(|n| n.name.clone()).collect(),
    };
    apply_patterns(&mut names, &opts.patterns);
    if let Some(f) = &opts.filter {
        let mut kept = Vec::new();
        for n in &names {
            if let Some(net) = d.nets.iter().find(|x| &x.name == n) {
                match eval_filter(f, &net_props(net)) {
                    Ok(true) => kept.push(n.clone()),
                    Ok(false) => {}
                    Err(m) => return err(line, format!("-filter: {m}")),
                }
            }
        }
        names = kept;
    }
    Ctl::Value(join_list(&names))
}

fn cmd_get_pins(
    i: &mut TclInterp,
    argv: &[String],
    line: usize,
    shell: &mut GpuiShell,
) -> Ctl {
    let _ = i;
    let opts = match parse_get_opts(argv, line, "get_pins") {
        Ok(o) => o,
        Err(e) => return Ctl::Error(e),
    };
    let d = match design(shell, "get_pins", line) {
        Ok(d) => d,
        Err(e) => return Ctl::Error(e),
    };
    let mut names: Vec<String> = match &opts.of_objects {
        Some(of) => match pins_of(d, of, line) {
            Ok(n) => n,
            Err(e) => return Ctl::Error(e),
        },
        None => {
            let mut all = Vec::new();
            for cell in &d.cells {
                all.extend(helion_proj::get_pins(d, &cell.name));
            }
            all
        }
    };
    apply_patterns(&mut names, &opts.patterns);
    if let Some(f) = &opts.filter {
        let mut kept = Vec::new();
        for n in &names {
            if let Some((cell_name, pin)) = n.split_once('/') {
                if let Some(cell) = d.cells.iter().find(|c| c.name == cell_name) {
                    match eval_filter(f, &pin_props(d, cell, pin)) {
                        Ok(true) => kept.push(n.clone()),
                        Ok(false) => {}
                        Err(m) => return err(line, format!("-filter: {m}")),
                    }
                }
            }
        }
        names = kept;
    }
    Ctl::Value(join_list(&names))
}

fn cmd_get_ports(
    i: &mut TclInterp,
    argv: &[String],
    line: usize,
    shell: &mut GpuiShell,
) -> Ctl {
    let _ = i;
    let opts = match parse_get_opts(argv, line, "get_ports") {
        Ok(o) => o,
        Err(e) => return Ctl::Error(e),
    };
    let d = match design(shell, "get_ports", line) {
        Ok(d) => d,
        Err(e) => return Ctl::Error(e),
    };
    let mut names: Vec<String> = d.ports.iter().map(|p| p.name.clone()).collect();
    apply_patterns(&mut names, &opts.patterns);
    if let Some(f) = &opts.filter {
        let mut kept = Vec::new();
        for n in &names {
            if let Some(port) = d.ports.iter().find(|p| &p.name == n) {
                match eval_filter(f, &port_props(d, port)) {
                    Ok(true) => kept.push(n.clone()),
                    Ok(false) => {}
                    Err(m) => return err(line, format!("-filter: {m}")),
                }
            }
        }
        names = kept;
    }
    Ctl::Value(join_list(&names))
}

fn cmd_get_clocks(
    i: &mut TclInterp,
    argv: &[String],
    line: usize,
    _shell: &mut GpuiShell,
) -> Ctl {
    let opts = match parse_get_opts(argv, line, "get_clocks") {
        Ok(o) => o,
        Err(e) => return Ctl::Error(e),
    };
    let mut names: Vec<String> = i.clocks.clocks.iter().map(|c| c.name.clone()).collect();
    apply_patterns(&mut names, &opts.patterns);
    if let Some(f) = &opts.filter {
        let mut kept = Vec::new();
        for n in &names {
            if let Some(c) = i.clocks.clocks.iter().find(|x| &x.name == n) {
                match eval_filter(f, &clock_props(c)) {
                    Ok(true) => kept.push(n.clone()),
                    Ok(false) => {}
                    Err(m) => return err(line, format!("-filter: {m}")),
                }
            }
        }
        names = kept;
    }
    Ctl::Value(join_list(&names))
}

fn cmd_get_timing_paths(
    i: &mut TclInterp,
    argv: &[String],
    line: usize,
    shell: &mut GpuiShell,
) -> Ctl {
    let opts = match parse_get_opts(argv, line, "get_timing_paths") {
        Ok(o) => o,
        Err(e) => return Ctl::Error(e),
    };
    let mut paths = match ensure_timing(i, shell, line) {
        Ok((_, _, p)) => p,
        Err(e) => return Ctl::Error(e),
    };
    if let Some(n) = opts.max_paths {
        paths.truncate(n);
    }
    if let Some(f) = &opts.filter {
        let mut kept = Vec::new();
        for p in &paths {
            match eval_filter(f, &path_props(p)) {
                Ok(true) => kept.push(p.clone()),
                Ok(false) => {}
                Err(m) => return err(line, format!("-filter: {m}")),
            }
        }
        paths = kept;
    }
    let mut names: Vec<String> = paths.iter().map(|p| p.name.clone()).collect();
    apply_patterns(&mut names, &opts.patterns);
    Ctl::Value(join_list(&names))
}

/// `get_property` accepts Vivado order (`prop obj`) and, as a fallback, the
/// `obj prop` order, so both scripting styles work.
fn cmd_get_property(
    i: &mut TclInterp,
    argv: &[String],
    line: usize,
    shell: &mut GpuiShell,
) -> Ctl {
    let mut k = 1;
    let mut quiet = false;
    while k < argv.len() && argv[k].starts_with('-') {
        match argv[k].as_str() {
            "-quiet" => quiet = true,
            other => return err(line, format!("get_property: unknown option \"{other}\"")),
        }
        k += 1;
    }
    if argv.len() != k + 2 {
        return err(
            line,
            "wrong # args: should be \"get_property ?-quiet? property object\"",
        );
    }
    let (a, b) = (argv[k].clone(), argv[k + 1].clone());
    // Try Vivado order first: a = property, b = object. When several
    // object types share the name (e.g. port `clk` vs clock `clk`), the
    // first candidate that HAS the property wins.
    let prop_key = a.to_uppercase();
    let vivado = resolve_candidates(i, shell, &b, line, true)
        .ok()
        .and_then(|cands| {
            cands
                .into_iter()
                .find_map(|(_, props)| props.get(&prop_key).cloned())
        });
    if let Some(v) = vivado {
        return Ctl::Value(v);
    }
    // Fallback: a = object, b = property.
    match resolve_obj(i, shell, &a, line, true) {
        Ok((_, props)) => match props.get(&b.to_uppercase()) {
            Some(v) => Ctl::Value(v.clone()),
            None if quiet => Ctl::Value(String::new()),
            None => err(
                line,
                format!("get_property: unknown property \"{b}\" on \"{a}\""),
            ),
        },
        Err(_) if quiet => Ctl::Value(String::new()),
        Err(_) => err(
            line,
            format!("get_property: no such object \"{b}\" (or unknown property \"{a}\")"),
        ),
    }
}

fn cmd_set_property(
    _i: &mut TclInterp,
    argv: &[String],
    line: usize,
    shell: &mut GpuiShell,
) -> Ctl {
    let mut k = 1;
    let mut quiet = false;
    while k < argv.len() && argv[k].starts_with('-') {
        match argv[k].as_str() {
            "-quiet" => quiet = true,
            other => return err(line, format!("set_property: unknown option \"{other}\"")),
        }
        k += 1;
    }
    if argv.len() < k + 3 {
        return err(
            line,
            "wrong # args: should be \"set_property ?-quiet? name value objects...\"",
        );
    }
    let (prop, value) = (argv[k].to_uppercase(), argv[k + 1].clone());
    let mut objs: Vec<String> = Vec::new();
    for a in &argv[k + 2..] {
        match split_list(a) {
            Ok(v) => objs.extend(v),
            Err(m) => return err(line, format!("set_property: {m}")),
        }
    }
    let d = match shell.session.design.as_mut() {
        Some(d) => d,
        None => return err(line, "set_property: no design"),
    };
    let mut applied = 0usize;
    for obj in &objs {
        let r = match prop.as_str() {
            "DONT_TOUCH" | "KEEP" => d.set_cell_attr(obj, "DONT_TOUCH", &value),
            "MARK_DEBUG" => d.set_net_attr(obj, "mark_debug", &value),
            "LOC" | "PACKAGE_PIN" => d.set_loc(obj, &value),
            "IOSTANDARD" => d.set_iostandard(obj, &value),
            "DRIVE" => d.set_drive(obj, &value),
            "SLEW" => d.set_slew(obj, &value),
            "PULLTYPE" => d.set_pulltype(obj, &value),
            "DIFF_TERM" => d.set_diff_term(obj, &value),
            "IN_TERM" => d.set_in_term(obj, &value),
            other => Err(format!("unsupported property \"{other}\"")),
        };
        match r {
            Ok(()) => applied += 1,
            Err(e) if quiet => {
                let _ = e;
            }
            Err(e) => return err(line, format!("set_property: {e}")),
        }
    }
    if objs.len() == 1 {
        Ctl::Value(format!(
            "set_property {} {} {}",
            argv[k], value, objs[0]
        ))
    } else {
        Ctl::Value(format!("set_property {} {value}: {applied} objects", argv[k]))
    }
}

fn cmd_report_property(
    i: &mut TclInterp,
    argv: &[String],
    line: usize,
    shell: &mut GpuiShell,
) -> Ctl {
    if argv.len() < 2 || argv.len() > 3 {
        return err(
            line,
            "wrong # args: should be \"report_property object ?pattern?\"",
        );
    }
    let pat = argv.get(2).map(|s| s.as_str());
    match resolve_obj(i, shell, &argv[1], line, true) {
        Ok((_, props)) => {
            let mut keys: Vec<&String> = props.keys().collect();
            keys.sort();
            let mut out = Vec::new();
            for k in keys {
                if pat.is_none_or(|p| glob_match(p, k)) {
                    out.push(format!("{k} = {}", props[k]));
                }
            }
            Ctl::Value(out.join("\n"))
        }
        Err(e) => Ctl::Error(e),
    }
}

fn cmd_list_property(
    i: &mut TclInterp,
    argv: &[String],
    line: usize,
    shell: &mut GpuiShell,
) -> Ctl {
    if argv.len() < 2 || argv.len() > 3 {
        return err(
            line,
            "wrong # args: should be \"list_property object ?pattern?\"",
        );
    }
    let pat = argv.get(2).map(|s| s.as_str());
    match resolve_obj(i, shell, &argv[1], line, true) {
        Ok((_, props)) => {
            let mut keys: Vec<String> = props
                .keys()
                .filter(|k| pat.is_none_or(|p| glob_match(p, k)))
                .cloned()
                .collect();
            keys.sort();
            Ctl::Value(join_list(&keys))
        }
        Err(e) => Ctl::Error(e),
    }
}

fn cmd_select_objects(
    i: &mut TclInterp,
    argv: &[String],
    line: usize,
    shell: &mut GpuiShell,
    highlighted: bool,
) -> Ctl {
    if argv.len() < 2 {
        return err(
            line,
            "wrong # args: should be \"select_objects objects...\"",
        );
    }
    let mut objs: Vec<String> = Vec::new();
    for a in &argv[1..] {
        match split_list(a) {
            Ok(v) => objs.extend(v),
            Err(m) => return err(line, format!("select_objects: {m}")),
        }
    }
    // Validate every name resolves (no timing fallback: selection is cheap).
    for o in &objs {
        if resolve_obj(i, shell, o, line, false).is_err() {
            return err(line, format!("select_objects: no such object \"{o}\""));
        }
    }
    i.selected.clear();
    for o in &objs {
        let kind = resolve_obj(i, shell, o, line, false)
            .map(|(k, _)| format!("{k:?}").to_lowercase())
            .unwrap_or_else(|_| "unknown".into());
        i.selected.push(super::SelObj {
            kind,
            name: o.clone(),
            highlighted,
        });
    }
    i.selection_dirty = true;
    Ctl::Value(join_list(&objs))
}

fn cmd_synth(
    _i: &mut TclInterp,
    argv: &[String],
    line: usize,
    shell: &mut GpuiShell,
) -> Ctl {
    if argv[0] == "hds::synth" && argv.len() == 1 {
        return Ctl::Value("synth ok".into());
    }
    let path = match argv.get(1) {
        Some(p) => p,
        None => {
            return err(
                line,
                format!("wrong # args: should be \"{} file\"", argv[0]),
            )
        }
    };
    let d = match synth_hdl_path(Path::new(path)) {
        Ok(d) => d,
        Err(e) => return err(line, e),
    };
    shell.tree.nodes.push(d.name.clone());
    let msg = format!(
        "synth {} cells {} luts {}",
        d.name,
        d.cells.len(),
        d.lut_inits().len()
    );
    shell.session.synth_design(d);
    Ctl::Value(msg)
}

fn cmd_create_clock(i: &mut TclInterp, argv: &[String], line: usize) -> Ctl {
    let mut period_ns: Option<f64> = None;
    let mut name: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut k = 1;
    while k < argv.len() {
        match argv[k].as_str() {
            "-period" => {
                k += 1;
                let v: f64 = match argv.get(k) {
                    Some(s) => match s.parse() {
                        Ok(v) => v,
                        Err(_) => {
                            return err(
                                line,
                                format!("create_clock: bad -period \"{s}\""),
                            );
                        }
                    },
                    None => {
                        return err(line, "create_clock: -period needs an argument");
                    }
                };
                period_ns = Some(v);
            }
            "-name" => {
                k += 1;
                name = Some(match argv.get(k) {
                    Some(s) => s.clone(),
                    None => return err(line, "create_clock: -name needs an argument"),
                });
            }
            s if s.starts_with('-') => {
                return err(line, format!("create_clock: unknown option \"{s}\""));
            }
            _ => positional.push(argv[k].clone()),
        }
        k += 1;
    }
    let clk_name = name
        .or_else(|| positional.first().cloned())
        .unwrap_or_else(|| "clk".into());
    let source = positional.get(1).cloned().unwrap_or_else(|| clk_name.clone());
    let period_ps = (period_ns.unwrap_or(10.0) * 1000.0).round() as u64;
    i.clocks.clocks.retain(|c| c.name != clk_name);
    create_clock(&mut i.clocks.clocks, &clk_name, period_ps, &source);
    Ctl::Value(format!("clock {clk_name} period {period_ps}ps"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tcl::interp::TclInterp;

    fn shell_with_counter() -> GpuiShell {
        let mut sh = GpuiShell::default();
        let sv = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/counter.sv");
        let r = crate::tcl_eval(&mut sh, &format!("read_sv {}", sv.display())).unwrap();
        assert!(r.contains("synth"), "{r}");
        sh
    }

    #[test]
    fn get_cells_nets_pins_ports() {
        let mut sh = shell_with_counter();
        let mut i = TclInterp::new();
        let cells = eval_host(&mut i, "get_cells", &["get_cells".into()], 1, &mut sh);
        let cells = match cells {
            Ctl::Value(s) => s,
            _ => panic!(),
        };
        assert!(cells.contains("u_lut"), "{cells}");
        // pattern
        let pat = eval_host(
            &mut i,
            "get_cells",
            &["get_cells".into(), "u_lut*".into()],
            1,
            &mut sh,
        );
        let pat = match pat {
            Ctl::Value(s) => s,
            _ => panic!(),
        };
        assert!(pat.contains("u_lut") && !pat.contains("u_ff"), "{pat}");
        // property + filter (filter text as the interpreter delivers it:
        // braces are Tcl grouping, stripped by the parser)
        let filt = eval_host(
            &mut i,
            "get_cells",
            &["get_cells".into(), "-filter".into(), "TYPE == lut".into()],
            1,
            &mut sh,
        );
        let filt = match filt {
            Ctl::Value(s) => s,
            _ => panic!(),
        };
        assert!(filt.contains("u_lut"), "{filt}");
        let nets = eval_host(&mut i, "get_nets", &["get_nets".into()], 1, &mut sh);
        assert!(matches!(&nets, Ctl::Value(s) if s.contains("cnt_")), "{nets:?}");
        let pins = eval_host(
            &mut i,
            "get_pins",
            &["get_pins".into(), "-of_objects".into(), "u_lut0".into()],
            1,
            &mut sh,
        );
        assert!(matches!(&pins, Ctl::Value(s) if s.contains("u_lut0/")), "{pins:?}");
    }

    #[test]
    fn get_property_orders() {
        let mut sh = shell_with_counter();
        let mut i = TclInterp::new();
        // Vivado order
        let v = eval_host(
            &mut i,
            "get_property",
            &["get_property".into(), "TYPE".into(), "u_lut0".into()],
            1,
            &mut sh,
        );
        assert!(matches!(&v, Ctl::Value(s) if s == "lut"), "{v:?}");
        // task order (obj prop)
        let v = eval_host(
            &mut i,
            "get_property",
            &["get_property".into(), "u_lut0".into(), "TYPE".into()],
            1,
            &mut sh,
        );
        assert!(matches!(&v, Ctl::Value(s) if s == "lut"), "{v:?}");
        // unknown property
        let e = eval_host(
            &mut i,
            "get_property",
            &["get_property".into(), "BOGUS".into(), "u_lut0".into()],
            1,
            &mut sh,
        );
        assert!(matches!(&e, Ctl::Error(_)), "{e:?}");
        // list_property
        let lp = eval_host(
            &mut i,
            "list_property",
            &["list_property".into(), "u_lut0".into()],
            1,
            &mut sh,
        );
        assert!(matches!(&lp, Ctl::Value(s) if s.contains("TYPE")), "{lp:?}");
    }

    #[test]
    fn clocks_roundtrip() {
        let mut sh = shell_with_counter();
        let mut i = TclInterp::new();
        let r = eval_host(
            &mut i,
            "create_clock",
            &[
                "create_clock".into(),
                "-period".into(),
                "10".into(),
                "clk".into(),
            ],
            1,
            &mut sh,
        );
        assert!(matches!(&r, Ctl::Value(s) if s == "clock clk period 10000ps"), "{r:?}");
        let g = eval_host(&mut i, "get_clocks", &["get_clocks".into()], 1, &mut sh);
        assert!(matches!(&g, Ctl::Value(s) if s == "clk"), "{g:?}");
        let p = eval_host(
            &mut i,
            "get_property",
            &["get_property".into(), "PERIOD".into(), "clk".into()],
            1,
            &mut sh,
        );
        assert!(matches!(&p, Ctl::Value(s) if s == "10000"), "{p:?}");
    }

    #[test]
    fn select_objects_roundtrip() {
        let mut sh = shell_with_counter();
        let mut i = TclInterp::new();
        let s = eval_host(
            &mut i,
            "select_objects",
            &["select_objects".into(), "u_lut0".into()],
            1,
            &mut sh,
        );
        assert!(matches!(&s, Ctl::Value(v) if v.contains("u_lut0")), "{s:?}");
        assert_eq!(i.selected.len(), 1);
        assert_eq!(i.selected[0].kind, "cell");
        let g = eval_host(
            &mut i,
            "get_selected_objects",
            &["get_selected_objects".into()],
            1,
            &mut sh,
        );
        assert!(matches!(&g, Ctl::Value(v) if v == "u_lut0"), "{g:?}");
        let bad = eval_host(
            &mut i,
            "select_objects",
            &["select_objects".into(), "no_such_cell".into()],
            1,
            &mut sh,
        );
        assert!(matches!(&bad, Ctl::Error(_)), "{bad:?}");
    }

    #[test]
    fn report_timing_matches_session_engine() {
        let mut sh = shell_with_counter();
        let mut i = TclInterp::new();
        // Same engine as Session::report_timing; counter.sv gold WNS.
        let (name, t, paths) = ensure_timing(&mut i, &mut sh, 1).unwrap();
        assert_eq!(name, "counter");
        assert_eq!(t.wns_ps, 9640, "gold WNS_PS must hold");
        assert!(!paths.is_empty());
        let p0 = &paths[0];
        let props = path_props(p0);
        assert_eq!(props["SLACK"], p0.slack_ps.to_string());
        // get_timing_paths end-to-end
        let mut i2 = TclInterp::new();
        let g = eval_host(
            &mut i2,
            "get_timing_paths",
            &["get_timing_paths".into(), "-max_paths".into(), "2".into()],
            1,
            &mut sh,
        );
        match g {
            Ctl::Value(s) => assert!(!s.is_empty(), "expected path names"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn of_objects_traversal() {
        let mut sh = shell_with_counter();
        let mut i = TclInterp::new();
        // nets of a cell
        let n = eval_host(
            &mut i,
            "get_nets",
            &[
                "get_nets".into(),
                "-of_objects".into(),
                "u_lut0".into(),
            ],
            1,
            &mut sh,
        );
        assert!(matches!(&n, Ctl::Value(s) if !s.is_empty()), "{n:?}");
        // cells of a net
        if let Ctl::Value(s) = n {
            let first_net = split_list(&s).unwrap()[0].clone();
            let c = eval_host(
                &mut i,
                "get_cells",
                &["get_cells".into(), "-of_objects".into(), first_net],
                1,
                &mut sh,
            );
            assert!(matches!(&c, Ctl::Value(v) if v.contains("u_lut0")), "{c:?}");
        }
    }

    #[test]
    fn help_topics() {
        let mut sh = shell_with_counter();
        let mut i = TclInterp::new();
        // per-command usage
        let h = eval_host(
            &mut i,
            "help",
            &["help".into(), "get_cells".into()],
            1,
            &mut sh,
        );
        assert!(
            matches!(&h, Ctl::Value(s) if s.contains("get_cells") && s.contains("-filter")),
            "{h:?}"
        );
        // core command topic
        let h2 = eval_host(&mut i, "help", &["help".into(), "foreach".into()], 1, &mut sh);
        assert!(matches!(&h2, Ctl::Value(s) if s.contains("foreach")), "{h2:?}");
        // alias topic
        let h3 = eval_host(
            &mut i,
            "help",
            &["help".into(), "list_properties".into()],
            1,
            &mut sh,
        );
        assert!(matches!(&h3, Ctl::Value(_)), "{h3:?}");
        // overview with no args
        let h0 = eval_host(&mut i, "help", &["help".into()], 1, &mut sh);
        assert!(
            matches!(&h0, Ctl::Value(s) if s.contains("get_cells") && s.contains("report_timing")),
            "{h0:?}"
        );
        // unknown topic suggests
        let hb = eval_host(
            &mut i,
            "help",
            &["help".into(), "get_cellz".into()],
            1,
            &mut sh,
        );
        assert!(
            matches!(&hb, Ctl::Error(e) if e.msg.contains("did you mean") && e.msg.contains("get_cells")),
            "{hb:?}"
        );
        // user proc gets help from its definition
        crate::tcl_eval(&mut sh, "proc mysum {a {b 10}} { expr {$a + $b} }").unwrap();
        let mut i2 = std::mem::replace(&mut sh.tcl_interp, TclInterp::new());
        let hp = eval_host(&mut i2, "help", &["help".into(), "mysum".into()], 1, &mut sh);
        sh.tcl_interp = i2;
        assert!(
            matches!(&hp, Ctl::Value(s) if s.contains("mysum") && s.contains("{b 10}")),
            "{hp:?}"
        );
    }

    #[test]
    fn unknown_command_suggests_similar() {
        let mut sh = shell_with_counter();
        let mut i = TclInterp::new();
        let e = eval_host(&mut i, "get_cellz", &["get_cellz".into()], 1, &mut sh);
        match e {
            Ctl::Error(te) => {
                assert!(te.msg.contains("invalid command name"), "{}", te.msg);
                assert!(te.msg.contains("get_cells"), "{}", te.msg);
                assert!(!te.msg.contains('\n'), "{}", te.msg);
            }
            other => panic!("{other:?}"),
        }
        // far-off name: no suggestion, still a clean error
        let e2 = eval_host(&mut i, "zzzqqq", &["zzzqqq".into()], 1, &mut sh);
        assert!(matches!(&e2, Ctl::Error(_)), "{e2:?}");
    }

    #[test]
    fn clear_sets_console_flag() {
        let mut sh = shell_with_counter();
        let mut i = TclInterp::new();
        assert!(!i.clear_console);
        let r = eval_host(&mut i, "clear", &["clear".into()], 1, &mut sh);
        assert!(matches!(&r, Ctl::Value(_)), "{r:?}");
        assert!(i.clear_console);
        let bad = eval_host(&mut i, "clear", &["clear".into(), "x".into()], 1, &mut sh);
        assert!(matches!(&bad, Ctl::Error(_)), "{bad:?}");
    }

    #[test]
    fn list_properties_alias() {
        let mut sh = shell_with_counter();
        let mut i = TclInterp::new();
        for cmd in ["list_property", "list_properties"] {
            let lp = eval_host(&mut i, cmd, &[cmd.into(), "u_lut0".into()], 1, &mut sh);
            assert!(
                matches!(&lp, Ctl::Value(s) if s.contains("TYPE")),
                "{cmd}: {lp:?}"
            );
        }
    }

    #[test]
    fn report_tables_are_readable() {
        let mut sh = shell_with_counter();
        let mut i = TclInterp::new();
        let t = eval_host(&mut i, "report_timing", &["report_timing".into()], 1, &mut sh);
        match t {
            Ctl::Value(s) => {
                assert!(s.contains("Timing Report"), "{s}");
                assert!(s.contains("WNS"), "{s}");
                assert!(s.contains("9640"), "{s}"); // gold WNS_PS surfaces
                assert!(s.contains("Startpoint"), "{s}");
            }
            other => panic!("{other:?}"),
        }
        let u = eval_host(
            &mut i,
            "report_utilization",
            &["report_utilization".into()],
            1,
            &mut sh,
        );
        match u {
            Ctl::Value(s) => {
                assert!(s.contains("Utilization"), "{s}");
                assert!(s.contains("LUTFF"), "{s}");
                assert!(s.contains("Resource"), "{s}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn demo_analysis_script_end_to_end() {
        // A realistic user script through the real entry point: query the
        // object model, filter in Tcl, and summarize — no direct eval_host.
        let mut sh = shell_with_counter();
        let r = crate::tcl_eval(
            &mut sh,
            "set cells [get_cells]\n\
             set nluts 0\n\
             foreach c $cells {\n\
             \x20 if {[get_property TYPE $c] eq \"lut\"} { incr nluts }\n\
             }\n\
             list [llength $cells] $nluts",
        )
        .unwrap();
        assert_eq!(r, "9 4", "{r}");
        // -filter agrees with the scripted count
        let f = crate::tcl_eval(&mut sh, "llength [get_cells -filter {TYPE == lut}]").unwrap();
        assert_eq!(f, "4", "{f}");
        // selection round-trips into the interp state
        let s =
            crate::tcl_eval(&mut sh, "select_objects [get_cells u_lut*]; get_selected_objects")
                .unwrap();
        assert!(s.contains("u_lut0"), "{s}");
        assert_eq!(sh.tcl_interp.selected.len(), 4);
    }
}
