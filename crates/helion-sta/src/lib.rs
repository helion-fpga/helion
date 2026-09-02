//! Graph STA: create_clock / create_generated_clock / placed Manhattan.

use helion_ir::{CellKind, Design, PortDir};
use helion_place::Placed;
use helion_route::Routed;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Clock {
    pub name: String,
    pub period_ps: u64,
    pub source: String,
    pub generated: bool,
    pub master: Option<String>,
    pub divide_by: u32,
}

#[derive(Clone, Debug)]
pub struct TimingResult {
    pub clocks: Vec<Clock>,
    pub wns_ps: i64,
    pub tns_ps: i64,
    pub endpoints: usize,
    pub r2r_ps: i64,
    pub iob_ps: i64,
    pub setup_ps: i64,
    pub hold_ps: i64,
    pub hold_slack_ps: i64,
    pub route_ps: i64,
}

pub fn create_clock(clocks: &mut Vec<Clock>, name: &str, period_ps: u64, source: &str) {
    clocks.push(Clock {
        name: name.into(),
        period_ps,
        source: source.into(),
        generated: false,
        master: None,
        divide_by: 1,
    });
}

pub fn create_generated_clock(
    clocks: &mut Vec<Clock>,
    name: &str,
    master: &str,
    divide_by: u32,
    source: &str,
) -> Result<(), String> {
    let m = clocks
        .iter()
        .find(|c| c.name == master)
        .ok_or_else(|| format!("unknown master clock {master}"))?;
    let period = m.period_ps.saturating_mul(divide_by.max(1) as u64);
    clocks.push(Clock {
        name: name.into(),
        period_ps: period,
        source: source.into(),
        generated: true,
        master: Some(master.into()),
        divide_by: divide_by.max(1),
    });
    Ok(())
}

const LUT_PS: i64 = 150;
const FF_CKQ_PS: i64 = 80;
const SETUP_PS: i64 = 50;
const PIN_PS: i64 = 20;
/// HAD default I/O pad (LVCMOS18). Gold WNS for counter uses this.
const IOB_PS: i64 = 100;
const HOP_PS: i64 = 40;
const HOLD_REQ_PS: i64 = 20;

/// I/O pad delay for a Helion IOSTANDARD. Unset / LVCMOS18 keeps gold STA.
pub fn iostandard_pad_ps(std: Option<&str>) -> i64 {
    match std.map(|s| s.trim().to_ascii_uppercase()).as_deref() {
        None | Some("") | Some("LVCMOS18") => IOB_PS,
        Some("LVCMOS12") => 60,
        Some("LVCMOS15") => 80,
        Some("LVCMOS25") => 200,
        Some("LVCMOS33") => 280,
        Some("SSTL15") | Some("SSTL15_I") => 160,
        Some("HSTL_I") | Some("HSTL_I_18") => 140,
        _ => IOB_PS,
    }
}

/// Drive vs default 12 mA. Unset / 12 keeps gold STA.
pub fn drive_pad_delta_ps(drive: Option<&str>) -> i64 {
    match drive.and_then(helion_device::Device::parse_drive) {
        None | Some(12) => 0,
        Some(2) => 160,
        Some(4) => 80,
        Some(6) => 40,
        Some(8) => 16,
        Some(16) => -16,
        Some(24) => -32,
        _ => 0,
    }
}

/// Slew vs default SLOW. Unset / SLOW keeps gold STA.
pub fn slew_pad_delta_ps(slew: Option<&str>) -> i64 {
    match slew.map(|s| s.trim().to_ascii_uppercase()).as_deref() {
        None | Some("") | Some("SLOW") => 0,
        Some("FAST") => -40,
        _ => 0,
    }
}

/// Pull vs default NONE. Unset / NONE keeps gold STA.
pub fn pulltype_pad_delta_ps(pull: Option<&str>) -> i64 {
    match pull.map(|s| s.trim().to_ascii_uppercase()).as_deref() {
        None | Some("") | Some("NONE") => 0,
        Some("PULLUP") | Some("PULLDOWN") => 20,
        Some("KEEPER") => 24,
        _ => 0,
    }
}

/// Combined HAD pad delay (IOSTANDARD + DRIVE + SLEW + PULLTYPE).
pub fn port_pad_ps(
    std: Option<&str>,
    drive: Option<&str>,
    slew: Option<&str>,
    pull: Option<&str>,
) -> i64 {
    iostandard_pad_ps(std)
        + drive_pad_delta_ps(drive)
        + slew_pad_delta_ps(slew)
        + pulltype_pad_delta_ps(pull)
}

fn iob_pad_ps(design: &Design) -> i64 {
    design
        .ports
        .iter()
        .filter(|p| p.dir != PortDir::In)
        .map(|p| {
            port_pad_ps(
                p.attrs.get("IOSTANDARD"),
                p.attrs.get("DRIVE"),
                p.attrs.get("SLEW"),
                p.attrs.get("PULLTYPE"),
            )
        })
        .max()
        .unwrap_or(IOB_PS)
}

fn lut_fanin(design: &Design, lut: &str) -> i64 {
    (0..6)
        .filter(|p| design.net_on(lut, &format!("I{p}")).is_some())
        .count() as i64
}

fn r2r_ps(design: &Design) -> i64 {
    let mut max_ps = 0i64;
    for c in &design.cells {
        if !matches!(c.kind, CellKind::Lut6 { .. }) {
            continue;
        }
        let pins = lut_fanin(design, &c.name);
        max_ps = max_ps.max(FF_CKQ_PS + LUT_PS + pins * PIN_PS + SETUP_PS);
    }
    max_ps.max(FF_CKQ_PS + LUT_PS + SETUP_PS)
}

/// Unit-delay STA from netlist arity (no placement).
pub fn report_timing(design: &Design, clocks: &[Clock]) -> Result<TimingResult, String> {
    if clocks.is_empty() {
        return Err("no clocks".into());
    }
    let clk = &clocks[0];
    let ffs = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    let r2r = r2r_ps(design);
    let wns = clk.period_ps as i64 - r2r;
    Ok(TimingResult {
        clocks: clocks.to_vec(),
        wns_ps: wns,
        tns_ps: wns.min(0),
        endpoints: ffs.max(1),
        r2r_ps: r2r,
        iob_ps: 0,
        setup_ps: r2r,
        hold_ps: FF_CKQ_PS,
        hold_slack_ps: FF_CKQ_PS - HOLD_REQ_PS,
        route_ps: 0,
    })
}

/// Placement-aware STA: r2r plus IOB Manhattan.
pub fn report_timing_placed(
    design: &Design,
    placed: &Placed,
    clocks: &[Clock],
) -> Result<TimingResult, String> {
    let mut r = report_timing(design, clocks)?;
    let iob_ps = placed
        .lutff_sites
        .iter()
        .zip(placed.packed.lutffs.iter())
        .filter_map(|((site, _), lf)| {
            let iob = placed.iob_sites.first()?;
            if placed.packed.iobs.iter().any(|io| io.from_net == lf.q_net) {
                Some(FF_CKQ_PS + site.y.abs_diff(iob.y) as i64 * HOP_PS + iob_pad_ps(design))
            } else {
                None
            }
        })
        .max()
        .unwrap_or(0);
    r.iob_ps = iob_ps;
    let path = r.r2r_ps.max(iob_ps);
    r.setup_ps = path;
    r.hold_ps = FF_CKQ_PS + iob_ps.min(path);
    r.route_ps = 0;
    r.wns_ps = clocks[0].period_ps as i64 - path;
    r.tns_ps = r.wns_ps.min(0);
    r.hold_slack_ps = r.hold_ps - HOLD_REQ_PS;
    Ok(r)
}

/// STA using PathFinder hop delay so WNS/hold/setup move with placement.
pub fn report_timing_routed(
    design: &Design,
    routed: &Routed,
    clocks: &[Clock],
) -> Result<TimingResult, String> {
    let mut r = report_timing_placed(design, &routed.placed, clocks)?;
    let route_ps = routed
        .iob_src
        .iter()
        .map(|x| x.delay_ps)
        .max()
        .unwrap_or(0);
    r.route_ps = route_ps;
    r.iob_ps = FF_CKQ_PS + route_ps + iob_pad_ps(design);
    r.setup_ps = r.r2r_ps.max(r.iob_ps);
    r.hold_ps = FF_CKQ_PS + route_ps;
    r.wns_ps = clocks[0].period_ps as i64 - r.setup_ps;
    r.tns_ps = r.wns_ps.min(0);
    r.hold_slack_ps = r.hold_ps - HOLD_REQ_PS;
    Ok(r)
}

#[derive(Clone, Debug, Default)]
pub struct IoLocs {
    pub pins: BTreeMap<String, String>,
}

impl IoLocs {
    pub fn set_pin_loc(&mut self, port: &str, site: &str) {
        self.pins.insert(port.into(), site.into());
    }
}

/// Vivado-style SDC subset: `create_clock -period <ns> [get_ports <name>]`.
pub fn load_sdc(text: &str, clocks: &mut Vec<Clock>) -> Result<(), String> {
    let x = load_xdc(text)?;
    clocks.extend(x.clocks);
    if clocks.is_empty() {
        return Err("SDC contained no create_clock".into());
    }
    Ok(())
}


fn tcl_name(joined: &str, key: &str) -> Option<String> {
    joined.split_once(key).and_then(|(_, r)| {
        r.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .find(|s| !s.is_empty())
            .map(|s| s.to_string())
    })
}

fn tcl_object_name(joined: &str) -> Option<String> {
    tcl_name(joined, "get_ports")
        .or_else(|| tcl_name(joined, "get_pins"))
        .or_else(|| tcl_name(joined, "get_cells"))
        .or_else(|| tcl_name(joined, "get_clocks"))
        .or_else(|| tcl_name(joined, "get_nets"))
}

fn tcl_from_to(toks: &[&str]) -> (String, String) {
    let joined = toks.join(" ");
    let from = joined
        .split_once("-from")
        .and_then(|(_, r)| tcl_object_name(r))
        .unwrap_or_default();
    let to = joined
        .split_once("-to")
        .and_then(|(_, r)| tcl_object_name(r))
        .unwrap_or_default();
    (from, to)
}

/// UG903 `set_multicycle_path`: setup/hold path multipliers.
#[derive(Clone, Debug)]
pub struct MulticyclePath {
    pub from: String,
    pub to: String,
    pub setup_mult: u32,
    pub hold_mult: u32,
    pub start: bool,
    pub end: bool,
}

/// UG903 `set_max_delay`: absolute setup requirement (ps), replacing the period check.
#[derive(Clone, Debug)]
pub struct MaxDelay {
    pub from: String,
    pub to: String,
    pub delay_ps: i64,
    pub datapath_only: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Constraints {
    pub clocks: Vec<Clock>,
    pub input_delay_ps: BTreeMap<String, i64>,
    pub output_delay_ps: BTreeMap<String, i64>,
    pub false_paths: Vec<String>,
    pub multicycle_paths: Vec<MulticyclePath>,
    pub max_delays: Vec<MaxDelay>,
    pub package_pins: BTreeMap<String, String>,
    pub iostandards: BTreeMap<String, String>,
    pub drives: BTreeMap<String, String>,
    pub slews: BTreeMap<String, String>,
    pub pulltypes: BTreeMap<String, String>,
}

impl Constraints {
    /// Setup path multiplier (Vivado default 1). Empty list keeps gold WNS.
    pub fn setup_mult(&self) -> u32 {
        self.multicycle_paths
            .iter()
            .map(|m| m.setup_mult)
            .max()
            .unwrap_or(1)
            .max(1)
    }

    /// Hold path multiplier (Vivado default 0).
    pub fn hold_mult(&self) -> u32 {
        self.multicycle_paths
            .iter()
            .map(|m| m.hold_mult)
            .max()
            .unwrap_or(0)
    }

    /// Tightest `set_max_delay` in ps, if any.
    pub fn max_delay_ps(&self) -> Option<i64> {
        self.max_delays.iter().map(|m| m.delay_ps).min()
    }
}

impl Constraints {
    pub fn apply(&self, design: &mut Design) -> Result<(), String> {
        for (port, site) in &self.package_pins {
            design.set_loc(port, site)?;
        }
        for (port, std) in &self.iostandards {
            design.set_iostandard(port, std)?;
        }
        for (port, ma) in &self.drives {
            design.set_drive(port, ma)?;
        }
        for (port, slew) in &self.slews {
            design.set_slew(port, slew)?;
        }
        for (port, pull) in &self.pulltypes {
            design.set_pulltype(port, pull)?;
        }
        Ok(())
    }
}

/// XDC/SDC: create_clock, create_generated_clock, set_input/output_delay,
/// set_false_path, set_multicycle_path, set_max_delay,
/// set_property PACKAGE_PIN / IOSTANDARD / DRIVE / SLEW / PULLTYPE.
pub fn load_xdc(text: &str) -> Result<Constraints, String> {
    let mut c = Constraints::default();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.is_empty() {
            continue;
        }
        match toks[0] {
            "create_clock" => {
                let mut period_ns: Option<f64> = None;
                let mut name = String::new();
                let mut source = "clk".to_string();
                let mut i = 1;
                while i < toks.len() {
                    if toks[i] == "-period" {
                        period_ns = toks.get(i + 1).and_then(|s| s.parse().ok());
                        i += 2;
                        continue;
                    }
                    if toks[i] == "-name" {
                        name = toks.get(i + 1).unwrap_or(&"clk").to_string();
                        i += 2;
                        continue;
                    }
                    let joined = toks[i..].join(" ");
                    if let Some(n) = tcl_name(&joined, "get_ports") {
                        source = n;
                    }
                    i += 1;
                }
                let ns = period_ns.ok_or_else(|| format!("create_clock missing -period: {line}"))?;
                let ps = (ns * 1000.0).round() as u64;
                if name.is_empty() {
                    name = source.clone();
                }
                create_clock(&mut c.clocks, &name, ps.max(1), &source);
            }
            "create_generated_clock" => {
                let mut name = "genclk".to_string();
                let mut master = c.clocks.first().map(|k| k.name.clone()).unwrap_or_else(|| "clk".into());
                let mut divide_by = 2u32;
                let mut source = String::new();
                let mut i = 1;
                while i < toks.len() {
                    if toks[i] == "-name" {
                        name = toks.get(i + 1).unwrap_or(&"genclk").to_string();
                        i += 2;
                        continue;
                    }
                    if toks[i] == "-divide_by" {
                        divide_by = toks.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(2);
                        i += 2;
                        continue;
                    }
                    if toks[i] == "-source" {
                        let joined = toks[i..].join(" ");
                        if let Some(n) = tcl_name(&joined, "get_ports").or_else(|| tcl_name(&joined, "get_pins")) {
                            master = n;
                        }
                        i += 1;
                        continue;
                    }
                    let joined = toks[i..].join(" ");
                    if let Some(n) = tcl_name(&joined, "get_pins").or_else(|| tcl_name(&joined, "get_ports")) {
                        source = n;
                    }
                    i += 1;
                }
                if source.is_empty() {
                    source = name.clone();
                }
                if c.clocks.iter().all(|k| k.name != master) {
                    // fall back to first clock name for -source [get_ports clk]
                    if let Some(k) = c.clocks.first() {
                        master = k.name.clone();
                    }
                }
                create_generated_clock(&mut c.clocks, &name, &master, divide_by, &source)?;
            }
            "set_input_delay" | "set_output_delay" => {
                let is_out = toks[0] == "set_output_delay";
                let mut delay_ns: Option<f64> = None;
                let mut port = String::new();
                let mut i = 1;
                while i < toks.len() {
                    if toks[i] == "-clock" {
                        i += 2;
                        continue;
                    }
                    if delay_ns.is_none() {
                        if let Ok(v) = toks[i].parse::<f64>() {
                            delay_ns = Some(v);
                            i += 1;
                            continue;
                        }
                    }
                    let joined = toks[i..].join(" ");
                    if let Some(n) = tcl_name(&joined, "get_ports") {
                        port = n;
                    }
                    i += 1;
                }
                let ns = delay_ns.ok_or_else(|| format!("{line}: missing delay"))?;
                let ps = (ns * 1000.0).round() as i64;
                if port.is_empty() {
                    return Err(format!("{line}: missing port"));
                }
                if is_out {
                    c.output_delay_ps.insert(port, ps);
                } else {
                    c.input_delay_ps.insert(port, ps);
                }
            }
            "set_false_path" => {
                let joined = toks.join(" ");
                if let Some(n) = tcl_name(&joined, "get_ports")
                    .or_else(|| tcl_name(&joined, "get_pins"))
                    .or_else(|| tcl_name(&joined, "get_cells"))
                {
                    c.false_paths.push(n);
                } else {
                    c.false_paths.push(joined);
                }
            }
            "set_multicycle_path" => {
                let mut want_setup = false;
                let mut want_hold = false;
                let mut setup_n: Option<u32> = None;
                let mut hold_n: Option<u32> = None;
                let mut bare: Option<u32> = None;
                let mut start = false;
                let mut end = false;
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-setup" => {
                            want_setup = true;
                            if let Some(v) = toks.get(i + 1).and_then(|s| s.parse().ok()) {
                                setup_n = Some(v);
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                        "-hold" => {
                            want_hold = true;
                            if let Some(v) = toks.get(i + 1).and_then(|s| s.parse().ok()) {
                                hold_n = Some(v);
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                        "-start" => {
                            start = true;
                            i += 1;
                        }
                        "-end" => {
                            end = true;
                            i += 1;
                        }
                        s if s.starts_with('-') => i += 1,
                        other => {
                            if bare.is_none() {
                                if let Ok(v) = other.parse::<u32>() {
                                    bare = Some(v);
                                }
                            }
                            i += 1;
                        }
                    }
                }
                if setup_n.is_none() && hold_n.is_none() && bare.is_none() {
                    return Err(format!("{line}: missing path multiplier"));
                }
                let (from, to) = tcl_from_to(&toks);
                let (setup_mult, hold_mult) = if want_hold && !want_setup {
                    (1, hold_n.or(bare).unwrap_or(0))
                } else if want_setup && want_hold {
                    (setup_n.or(bare).unwrap_or(1), hold_n.unwrap_or(0))
                } else if want_setup {
                    (setup_n.or(bare).unwrap_or(1), 0)
                } else {
                    (bare.unwrap_or(1), 0)
                };
                c.multicycle_paths.push(MulticyclePath {
                    from,
                    to,
                    setup_mult: setup_mult.max(1),
                    hold_mult,
                    start,
                    end,
                });
            }
            "set_max_delay" => {
                let mut delay_ns: Option<f64> = None;
                let mut datapath_only = false;
                let mut i = 1;
                while i < toks.len() {
                    let t = toks[i];
                    if t == "-datapath_only" {
                        datapath_only = true;
                        i += 1;
                        continue;
                    }
                    if t.starts_with('-') {
                        i += 1;
                        continue;
                    }
                    if delay_ns.is_none() {
                        if let Ok(v) = t.parse::<f64>() {
                            delay_ns = Some(v);
                        }
                    }
                    i += 1;
                }
                let ns = delay_ns.ok_or_else(|| format!("{line}: missing delay"))?;
                let (from, to) = tcl_from_to(&toks);
                c.max_delays.push(MaxDelay {
                    from,
                    to,
                    delay_ps: (ns * 1000.0).round() as i64,
                    datapath_only,
                });
            }
            "set_property" => {
                let key = toks.get(1).copied().unwrap_or("");
                if key.eq_ignore_ascii_case("PACKAGE_PIN") && toks.len() >= 3 {
                    let site = toks[2].to_string();
                    let joined = toks[3..].join(" ");
                    let port = tcl_name(&joined, "get_ports").unwrap_or_default();
                    if !port.is_empty() {
                        c.package_pins.insert(port, site);
                    }
                } else if (key.eq_ignore_ascii_case("IOSTANDARD")
                    || key.eq_ignore_ascii_case("DRIVE")
                    || key.eq_ignore_ascii_case("SLEW")
                    || key.eq_ignore_ascii_case("PULLTYPE"))
                    && toks.len() >= 3
                {
                    let val = toks[2].to_string();
                    let joined = toks[3..].join(" ");
                    let port = tcl_name(&joined, "get_ports")
                        .or_else(|| {
                            toks.get(3).map(|s| {
                                s.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                                    .to_string()
                            })
                            .filter(|s| !s.is_empty())
                        })
                        .unwrap_or_default();
                    if !port.is_empty() {
                        if key.eq_ignore_ascii_case("IOSTANDARD") {
                            c.iostandards.insert(port, val);
                        } else if key.eq_ignore_ascii_case("DRIVE") {
                            c.drives.insert(port, val);
                        } else if key.eq_ignore_ascii_case("SLEW") {
                            c.slews.insert(port, val);
                        } else {
                            c.pulltypes.insert(port, val);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(c)
}

pub fn apply_xdc(design: &mut Design, xdc: &Constraints) -> Result<(), String> {
    xdc.apply(design)
}

/// Apply `set_input_delay` / `set_output_delay` / `set_false_path` /
/// `set_multicycle_path` / `set_max_delay` to an STA result.
/// False paths drop the IOB contribution; I/O delays add to setup and move WNS.
/// Setup MCP N uses N×period as the requirement; `set_max_delay` replaces it.
/// Hold MCP M subtracts M×period from hold slack. Empty constraints keep gold WNS.
pub fn apply_xdc_delays(r: &mut TimingResult, xdc: &Constraints, period_ps: u64) {
    let false_out = !xdc.false_paths.is_empty();
    let out_d = xdc.output_delay_ps.values().copied().max().unwrap_or(0);
    let in_d = xdc.input_delay_ps.values().copied().max().unwrap_or(0);
    if false_out {
        r.iob_ps = 0;
        r.setup_ps = r.r2r_ps;
    } else if in_d != 0 || out_d != 0 {
        r.setup_ps += out_d + in_d;
    }
    let req_ps = xdc
        .max_delay_ps()
        .unwrap_or_else(|| (period_ps as i64).saturating_mul(xdc.setup_mult() as i64));
    r.wns_ps = req_ps - r.setup_ps;
    r.tns_ps = r.wns_ps.min(0);
    let hold_mult = xdc.hold_mult();
    if hold_mult > 0 {
        r.hold_slack_ps =
            r.hold_ps - HOLD_REQ_PS - (period_ps as i64).saturating_mul(hold_mult as i64);
    }
}

/// STA with XDC delays/false paths applied.
pub fn report_timing_xdc(
    design: &Design,
    clocks: &[Clock],
    xdc: &Constraints,
) -> Result<TimingResult, String> {
    let clks = if xdc.clocks.is_empty() {
        clocks.to_vec()
    } else {
        xdc.clocks.clone()
    };
    let mut r = report_timing(design, &clks)?;
    apply_xdc_delays(&mut r, xdc, clks[0].period_ps);
    Ok(r)
}

/// Routed STA plus XDC I/O delay / false path / multicycle / max_delay
/// (UG893 Timing Constraints Apply).
pub fn report_timing_routed_xdc(
    design: &Design,
    routed: &Routed,
    clocks: &[Clock],
    xdc: &Constraints,
) -> Result<TimingResult, String> {
    let mut r = report_timing_routed(design, routed, clocks)?;
    apply_xdc_delays(&mut r, xdc, clocks[0].period_ps);
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use helion_ir::Design;
    use helion_pack::pack;
    use helion_place::{place, place_with, PlaceOpts};

    #[test]
    fn create_clock_and_generated() {
        let d = Design::structural_blinky();
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        create_generated_clock(&mut clks, "clk_div2", "clk", 2, "u_ff/Q").unwrap();
        assert!(clks[1].generated);
        assert_eq!(clks[1].period_ps, 20_000);
        let r = report_timing(&d, &clks).unwrap();
        assert!(r.endpoints >= 1);
        assert_eq!(r.clocks.len(), 2);
        // non-vacuous: path delay is counted
        assert_ne!(r.wns_ps, clks[0].period_ps as i64);
    }

    #[test]
    fn counter_sta_worse_than_blinky() {
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        let b = report_timing(&Design::structural_blinky(), &clks).unwrap();
        let c = report_timing(&Design::structural_counter(), &clks).unwrap();
        assert!(
            c.r2r_ps > b.r2r_ps,
            "4-input incrementer LUT must be slower than 1-input inverter ({} vs {})",
            c.r2r_ps,
            b.r2r_ps
        );
        assert!(c.wns_ps < b.wns_ps);
    }

    #[test]
    fn placed_timing_moves_with_iob_distance() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_blinky();
        let p = pack(&d, &dev).unwrap();
        let wl = place_with(&p, &dev, PlaceOpts { timing_weight: 0.0 }).unwrap();
        let td = place_with(&p, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        let a = report_timing_placed(&d, &wl, &clks).unwrap();
        let b = report_timing_placed(&d, &td, &clks).unwrap();
        assert!(
            b.iob_ps < a.iob_ps,
            "timing-driven must shorten IOB path (TD {} WL {})",
            b.iob_ps,
            a.iob_ps
        );
        let _ = place(&p, &dev);
    }

    #[test]
    fn routed_wns_hold_setup_move_with_placement() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_blinky();
        let p = pack(&d, &dev).unwrap();
        let wl = place_with(&p, &dev, PlaceOpts { timing_weight: 0.0 }).unwrap();
        let td = place_with(&p, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
        let r_wl = helion_route::route(&wl, &dev).unwrap();
        let r_td = helion_route::route(&td, &dev).unwrap();
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        let a = report_timing_routed(&d, &r_wl, &clks).unwrap();
        let b = report_timing_routed(&d, &r_td, &clks).unwrap();
        assert_ne!(a.wns_ps, b.wns_ps, "WNS must move with placement (WL {} TD {})", a.wns_ps, b.wns_ps);
        assert_ne!(a.hold_ps, b.hold_ps, "hold must move with placement");
        assert_ne!(a.setup_ps, b.setup_ps, "setup must move with placement");
        assert!(b.wns_ps > a.wns_ps, "timing-driven must improve WNS (TD {} WL {})", b.wns_ps, a.wns_ps);
        assert!(b.setup_ps < a.setup_ps);
        assert!(b.hold_ps < a.hold_ps, "shorter route → less hold delay");
        assert!(a.route_ps > 0 && b.route_ps > 0);
    }

    #[test]
    fn io_loc_binds_had_site() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let site = dev.iob_sites().next().unwrap();
        let loc_s = format!("IOB_X{}Y{}", site.x, site.y);
        let mut io = IoLocs::default();
        io.set_pin_loc("led", &loc_s);
        assert_eq!(io.pins["led"], loc_s);
        assert!(dev.iob_major(site.x, site.y).is_some());
    }

    #[test]
    fn sdc_create_clock_period_ns() {
        let mut clks = Vec::new();
        load_sdc("create_clock -period 10.000 [get_ports clk]\n", &mut clks).unwrap();
        assert_eq!(clks[0].period_ps, 10_000);
        assert_eq!(clks[0].source, "clk");
        let d = Design::structural_blinky();
        let r = report_timing(&d, &clks).unwrap();
        assert_ne!(r.wns_ps, 10_000);
    }

    #[test]
    fn xdc_delays_false_path_package_pin_bound_in_place() {
        let xdc = r#"
create_clock -period 10.000 [get_ports clk]
create_generated_clock -name clkdiv -source [get_ports clk] -divide_by 2 [get_pins u_ff/Q]
set_input_delay -clock clk 1.5 [get_ports clk]
set_output_delay -clock clk 2.0 [get_ports led]
set_false_path -from [get_ports clk] -to [get_ports led]
set_property PACKAGE_PIN IOB_X5Y0 [get_ports led]
"#;
        let c = load_xdc(xdc).unwrap();
        assert_eq!(c.clocks.len(), 2);
        assert!(c.clocks[1].generated);
        assert_eq!(c.clocks[1].period_ps, 20_000);
        assert_eq!(c.output_delay_ps["led"], 2000);
        assert_eq!(c.input_delay_ps["clk"], 1500);
        assert!(c.false_paths.iter().any(|p| p == "clk" || p == "led"));
        assert_eq!(c.package_pins["led"], "IOB_X5Y0");

        let mut d = Design::structural_blinky();
        apply_xdc(&mut d, &c).unwrap();
        assert_eq!(d.ports.iter().find(|p| p.name == "led").unwrap().attrs.get("LOC"), Some("IOB_X5Y0"));
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        assert_eq!(pl.iob_sites[0].x, 5, "PACKAGE_PIN must bind IOB in place");
        assert_eq!(pl.lutff_sites[0].0.x, 5);

        let base = report_timing(&d, &c.clocks).unwrap();
        let mut only = c.clone();
        only.false_paths.clear();
        let with_d = report_timing_xdc(&d, &c.clocks, &only).unwrap();
        assert!(
            with_d.wns_ps < base.wns_ps,
            "output/input delay must worsen WNS ({} vs {})",
            with_d.wns_ps,
            base.wns_ps
        );
        let falsep = report_timing_xdc(&d, &c.clocks, &c).unwrap();
        assert_eq!(falsep.setup_ps, falsep.r2r_ps, "false path must drop IOB from setup");
        assert_ne!(with_d.wns_ps, falsep.wns_ps);

        let routed = helion_route::route(&pl, &dev).unwrap();
        let rbase = report_timing_routed(&d, &routed, &c.clocks).unwrap();
        let rdel = report_timing_routed_xdc(&d, &routed, &c.clocks, &only).unwrap();
        assert_eq!(
            rdel.wns_ps,
            rbase.wns_ps - 1500 - 2000,
            "routed I/O delay must subtract from WNS ({} vs {})",
            rdel.wns_ps,
            rbase.wns_ps
        );
        let rfp = report_timing_routed_xdc(&d, &routed, &c.clocks, &c).unwrap();
        assert_eq!(rfp.setup_ps, rfp.r2r_ps);
        assert_eq!(rfp.iob_ps, 0);
        assert_ne!(rdel.wns_ps, rfp.wns_ps);
    }

    #[test]
    fn iostandard_pad_delay_moves_iob_sta() {
        let mut d = Design::structural_counter();
        let xdc = load_xdc("set_property IOSTANDARD LVCMOS33 [get_ports led]\n").unwrap();
        assert_eq!(xdc.iostandards["led"], "LVCMOS33");
        apply_xdc(&mut d, &xdc).unwrap();
        assert_eq!(
            d.ports.iter().find(|p| p.name == "led").unwrap().attrs.get("IOSTANDARD"),
            Some("LVCMOS33")
        );

        let mut plain = Design::structural_counter();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&plain, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let routed = helion_route::route(&pl, &dev).unwrap();
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        let a = report_timing_routed(&plain, &routed, &clks).unwrap();
        plain.set_iostandard("led", "LVCMOS33").unwrap();
        let b = report_timing_routed(&plain, &routed, &clks).unwrap();
        assert!(
            b.iob_ps > a.iob_ps,
            "LVCMOS33 pad must slow IOB STA ({} vs {})",
            b.iob_ps,
            a.iob_ps
        );
        assert!(
            b.wns_ps < a.wns_ps,
            "slower I/O standard must worsen WNS ({} vs {})",
            b.wns_ps,
            a.wns_ps
        );
        assert_eq!(a.iob_ps, FF_CKQ_PS + a.route_ps + IOB_PS);
        assert_eq!(b.iob_ps, FF_CKQ_PS + b.route_ps + iostandard_pad_ps(Some("LVCMOS33")));
    }

    #[test]
    fn drive_slew_pulltype_move_iob_sta() {
        let xdc = load_xdc(
            "set_property DRIVE 4 [get_ports led]\nset_property SLEW FAST [get_ports led]\nset_property PULLTYPE PULLUP [get_ports led]\n",
        )
        .unwrap();
        assert_eq!(xdc.drives["led"], "4");
        assert_eq!(xdc.slews["led"], "FAST");
        assert_eq!(xdc.pulltypes["led"], "PULLUP");

        let mut d = Design::structural_counter();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let routed = helion_route::route(&pl, &dev).unwrap();
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        let a = report_timing_routed(&d, &routed, &clks).unwrap();
        d.set_drive("led", "4").unwrap();
        let b = report_timing_routed(&d, &routed, &clks).unwrap();
        assert!(
            b.iob_ps > a.iob_ps,
            "DRIVE 4 must slow IOB STA ({} vs {})",
            b.iob_ps,
            a.iob_ps
        );
        d.set_slew("led", "FAST").unwrap();
        let c = report_timing_routed(&d, &routed, &clks).unwrap();
        assert!(
            c.iob_ps < b.iob_ps,
            "FAST slew must speed IOB STA ({} vs {})",
            c.iob_ps,
            b.iob_ps
        );
        d.set_pulltype("led", "PULLUP").unwrap();
        let e = report_timing_routed(&d, &routed, &clks).unwrap();
        assert!(
            e.iob_ps > c.iob_ps,
            "PULLUP must add pad load ({} vs {})",
            e.iob_ps,
            c.iob_ps
        );
        d.set_drive("led", "12").unwrap();
        d.set_slew("led", "SLOW").unwrap();
        d.set_pulltype("led", "NONE").unwrap();
        let back = report_timing_routed(&d, &routed, &clks).unwrap();
        assert_eq!(back.iob_ps, a.iob_ps, "defaults must restore gold pad");
        assert_eq!(a.iob_ps, FF_CKQ_PS + a.route_ps + IOB_PS);
    }

    #[test]
    fn xdc_multicycle_max_delay_move_setup_hold() {
        let xdc = load_xdc(
            r#"
create_clock -period 10.000 [get_ports clk]
set_multicycle_path 2 -from [get_ports clk] -to [get_ports led]
set_multicycle_path -hold 1 -from [get_ports clk] -to [get_ports led]
set_max_delay 5.0 -from [get_ports clk] -to [get_ports led]
set_max_delay -datapath_only 2.5 -from [get_pins u_ff/Q] -to [get_ports led]
"#,
        )
        .unwrap();
        assert_eq!(xdc.multicycle_paths.len(), 2);
        assert_eq!(xdc.multicycle_paths[0].setup_mult, 2);
        assert_eq!(xdc.multicycle_paths[0].hold_mult, 0);
        assert_eq!(xdc.multicycle_paths[0].from, "clk");
        assert_eq!(xdc.multicycle_paths[0].to, "led");
        assert_eq!(xdc.multicycle_paths[1].setup_mult, 1);
        assert_eq!(xdc.multicycle_paths[1].hold_mult, 1);
        assert_eq!(xdc.setup_mult(), 2);
        assert_eq!(xdc.hold_mult(), 1);
        assert_eq!(xdc.max_delays.len(), 2);
        assert_eq!(xdc.max_delays[0].delay_ps, 5000);
        assert!(!xdc.max_delays[0].datapath_only);
        assert_eq!(xdc.max_delays[1].delay_ps, 2500);
        assert!(xdc.max_delays[1].datapath_only);
        assert_eq!(xdc.max_delay_ps(), Some(2500));

        let d = Design::structural_counter();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let routed = helion_route::route(&pl, &dev).unwrap();
        let base = report_timing_routed(&d, &routed, &xdc.clocks).unwrap();
        let empty = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &Constraints::default()).unwrap();
        assert_eq!(empty.wns_ps, base.wns_ps, "empty XDC must keep gold WNS");
        assert_eq!(empty.hold_slack_ps, base.hold_slack_ps);

        let mut mcp = Constraints::default();
        mcp.multicycle_paths.push(xdc.multicycle_paths[0].clone());
        let with_mcp = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &mcp).unwrap();
        assert_eq!(
            with_mcp.wns_ps,
            base.wns_ps + 10_000,
            "setup MCP 2 must add one period to WNS ({} vs {})",
            with_mcp.wns_ps,
            base.wns_ps
        );
        assert_eq!(
            with_mcp.hold_slack_ps, base.hold_slack_ps,
            "setup-only MCP must not move hold"
        );
        assert_eq!(with_mcp.setup_ps, base.setup_ps);

        let mut hold = mcp.clone();
        hold.multicycle_paths.push(xdc.multicycle_paths[1].clone());
        let with_hold = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &hold).unwrap();
        assert_eq!(with_hold.wns_ps, with_mcp.wns_ps, "hold MCP must not move setup WNS");
        assert_eq!(
            with_hold.hold_slack_ps,
            base.hold_slack_ps - 10_000,
            "hold MCP 1 must subtract one period from hold slack ({} vs {})",
            with_hold.hold_slack_ps,
            base.hold_slack_ps
        );

        let mut md = Constraints::default();
        md.max_delays.push(xdc.max_delays[0].clone());
        let with_md = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &md).unwrap();
        assert_eq!(
            with_md.wns_ps,
            5000 - base.setup_ps,
            "set_max_delay 5 ns replaces the period check ({} setup {})",
            with_md.wns_ps,
            base.setup_ps
        );
        assert_ne!(with_md.wns_ps, base.wns_ps);
        assert_ne!(with_md.wns_ps, with_mcp.wns_ps);
    }
}
