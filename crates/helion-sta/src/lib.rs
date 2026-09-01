//! Graph STA: create_clock / create_generated_clock / placed Manhattan.

use helion_ir::{CellKind, Design};
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
const IOB_PS: i64 = 100;
const HOP_PS: i64 = 40;
const HOLD_REQ_PS: i64 = 20;

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
                Some(FF_CKQ_PS + site.y.abs_diff(iob.y) as i64 * HOP_PS + IOB_PS)
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
    r.iob_ps = FF_CKQ_PS + route_ps + IOB_PS;
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

#[derive(Clone, Debug, Default)]
pub struct Constraints {
    pub clocks: Vec<Clock>,
    pub input_delay_ps: BTreeMap<String, i64>,
    pub output_delay_ps: BTreeMap<String, i64>,
    pub false_paths: Vec<String>,
    pub package_pins: BTreeMap<String, String>,
}

impl Constraints {
    pub fn apply(&self, design: &mut Design) -> Result<(), String> {
        for (port, site) in &self.package_pins {
            design.set_loc(port, site)?;
        }
        Ok(())
    }
}

/// XDC/SDC: create_clock, create_generated_clock, set_input/output_delay,
/// set_false_path, set_property PACKAGE_PIN.
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
            "set_property" => {
                if toks.get(1).copied() == Some("PACKAGE_PIN") && toks.len() >= 3 {
                    let site = toks[2].to_string();
                    let joined = toks[3..].join(" ");
                    let port = tcl_name(&joined, "get_ports").unwrap_or_default();
                    if !port.is_empty() {
                        c.package_pins.insert(port, site);
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

/// Apply `set_input_delay` / `set_output_delay` / `set_false_path` to an STA result.
/// False paths drop the IOB contribution; I/O delays add to setup and move WNS.
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
    r.wns_ps = period_ps as i64 - r.setup_ps;
    r.tns_ps = r.wns_ps.min(0);
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

/// Routed STA plus XDC I/O delay / false path (UG893 Timing Constraints Apply).
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
}
