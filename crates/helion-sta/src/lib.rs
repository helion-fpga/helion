//! Graph STA: create_clock / create_generated_clock.

use helion_ir::Design;
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

/// Unit-delay STA: each LUT+FF is 100ps, WNS = period - path.
pub fn report_timing(design: &Design, clocks: &[Clock]) -> Result<TimingResult, String> {
    if clocks.is_empty() {
        return Err("no clocks".into());
    }
    let clk = &clocks[0];
    let luts = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, helion_ir::CellKind::Lut6 { .. }))
        .count();
    let ffs = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, helion_ir::CellKind::Hff))
        .count();
    let path_ps = (luts + ffs) as i64 * 100;
    let wns = clk.period_ps as i64 - path_ps;
    Ok(TimingResult {
        clocks: clocks.to_vec(),
        wns_ps: wns,
        tns_ps: wns.min(0),
        endpoints: ffs.max(1),
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use helion_ir::Design;

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
    fn io_loc_binds_had_site() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let site = dev.iob_sites().next().unwrap();
        let loc_s = format!("IOB_X{}Y{}", site.x, site.y);
        let mut io = IoLocs::default();
        io.set_pin_loc("led", &loc_s);
        assert_eq!(io.pins["led"], loc_s);
        assert!(dev.iob_major(site.x, site.y).is_some());
    }
}
