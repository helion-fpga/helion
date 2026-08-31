//! Graph STA: create_clock / create_generated_clock / placed Manhattan.

use helion_ir::{CellKind, Design};
use helion_place::Placed;
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
    r.wns_ps = clocks[0].period_ps as i64 - path;
    r.tns_ps = r.wns_ps.min(0);
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
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if !line.contains("create_clock") {
            continue;
        }
        let mut period_ns: Option<f64> = None;
        let mut source = "clk".to_string();
        let toks: Vec<&str> = line.split_whitespace().collect();
        let mut i = 0;
        while i < toks.len() {
            if toks[i] == "-period" {
                period_ns = toks.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
                continue;
            }
            if toks[i] == "-name" {
                i += 2;
                continue;
            }
            if toks[i].contains("get_ports") {
                if let Some(name) = toks[i]
                    .split_once("get_ports")
                    .and_then(|(_, r)| r.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').find(|s| !s.is_empty()))
                    .map(|s| s.to_string())
                {
                    if !name.is_empty() {
                        source = name;
                    }
                }
            }
            i += 1;
        }
        let ns = period_ns.ok_or_else(|| format!("create_clock missing -period: {line}"))?;
        let ps = (ns * 1000.0).round() as u64;
        create_clock(clocks, &source, ps.max(1), &source);
    }
    if clocks.is_empty() {
        return Err("SDC contained no create_clock".into());
    }
    Ok(())
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
}
