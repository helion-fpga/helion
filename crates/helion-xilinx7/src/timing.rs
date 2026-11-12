//! Static timing analysis for the 7-series backend.
//!
//! This is the second consumer of [`helion_backend::TimingModel`]: it
//! computes WNS/TNS over the placed and routed 7-series netlist using the
//! backend's own model ([`helion_backend::TimingModel::xilinx7`]), so
//! "timing-closed" means the same thing here as on HL10T (WNS against the
//! clock period).
//!
//! The analysis is deliberately simple and documented: arrival times are
//! built by tracing each flip-flop's fanin cone through packed LUTs, with
//! net delay estimated from Manhattan tile distance times the model's
//! hop delay. It is NOT a Vivado-correlated signoff STA; the model values
//! themselves are preliminary (see [`helion_backend::TimingModel::xilinx7`]).

use crate::pack::{IobDir, Packed7};
use crate::place::Placed7;
use crate::route::Routed7;
use helion_backend::TimingModel;
use std::collections::{HashMap, HashSet};

/// Timing result for a 7-series placed/routed netlist.
#[derive(Clone, Debug)]
pub struct TimingReport7 {
    /// Worst negative slack across all flip-flop endpoints (ps).
    pub wns_ps: i64,
    /// Total negative slack (ps).
    pub tns_ps: i64,
    /// Number of timing endpoints (flip-flops).
    pub endpoints: usize,
    /// Worst arrival time (ps).
    pub worst_arrival_ps: i64,
    /// Endpoint with the worst slack, e.g. `"u_ff3"`.
    pub worst_endpoint: String,
}

/// Parse `(x, y)` from a 7-series tile name like `"INT_L_X10Y100"`.
fn tile_xy(tile: &str) -> Option<(i32, i32)> {
    let xi = tile.rfind("_X")? + 2;
    let yi = tile[xi..].find('Y').map(|i| xi + i)?;
    let x: i32 = tile[xi..yi].parse().ok()?;
    let y: i32 = tile[yi + 1..].parse().ok()?;
    Some((x, y))
}

enum Driver<'a> {
    Ff(&'a str),
    Lut(&'a str),
    IobIn(&'a str),
    Other,
}

struct Ctx<'a> {
    packed: &'a Packed7,
    tm: &'a TimingModel,
    /// net -> driver cell name + kind.
    drivers: HashMap<&'a str, Driver<'a>>,
    /// cell name -> tile (x, y).
    sites: HashMap<&'a str, (i32, i32)>,
}

impl<'a> Ctx<'a> {
    fn route_delay(&self, from: (i32, i32), to: (i32, i32)) -> i64 {
        ((from.0 - to.0).abs() + (from.1 - to.1).abs()) as i64 * self.tm.hop_ps
    }

    /// Arrival time of `net` at `load_xy`, tracing the fanin cone.
    /// `depth` caps recursion; combinational cycles are a loud error.
    fn arrival(
        &self,
        net: &str,
        load_xy: (i32, i32),
        depth: usize,
        visiting: &mut HashSet<String>,
    ) -> Result<i64, String> {
        if depth > 64 {
            return Err(format!("timing: combinational cycle through net {net}"));
        }
        let key = net.to_string();
        if !visiting.insert(key.clone()) {
            return Err(format!("timing: combinational cycle through net {net}"));
        }
        let result = (|| -> Result<i64, String> {
            let (driver, kind) = match self.drivers.get(net) {
                Some(Driver::Ff(name)) => (*name, 0),
                Some(Driver::Lut(name)) => (*name, 1),
                Some(Driver::IobIn(name)) => (*name, 2),
                Some(Driver::Other) | None => return Ok(0),
            };
            let site = self.sites.get(driver).copied().unwrap_or(load_xy);
            let wire = self.route_delay(site, load_xy);
            match kind {
                0 => Ok(self.tm.ff_ckq_ps + wire),
                1 => {
                    let lut = self
                        .packed
                        .luts
                        .iter()
                        .find(|l| l.name == driver)
                        .ok_or_else(|| format!("timing: LUT {driver} vanished"))?;
                    let mut worst = 0i64;
                    for inn in lut.input_nets.iter().flatten() {
                        worst = worst.max(self.arrival(inn, site, depth + 1, visiting)?);
                    }
                    Ok(worst + self.tm.lut_ps + wire)
                }
                _ => Ok(self.tm.iob_ps + wire),
            }
        })();
        visiting.remove(&key);
        result
    }
}

/// Compute WNS/TNS for `routed` against `period_ps`, using `tm` delays.
///
/// Every flip-flop D pin is an endpoint. Arrival traces the combinational
/// fanin cone; required time is `period_ps - setup`. Returns an error on
/// combinational cycles rather than a bogus number.
pub fn report_timing(
    routed: &Routed7,
    tm: &TimingModel,
    period_ps: u64,
) -> Result<TimingReport7, String> {
    let placed: &Placed7 = &routed.placed;
    let packed: &Packed7 = &placed.packed;

    let mut drivers: HashMap<&str, Driver<'_>> = HashMap::new();
    for lut in &packed.luts {
        drivers.insert(lut.output_net.as_str(), Driver::Lut(lut.name.as_str()));
    }
    for ff in &packed.ffs {
        drivers.insert(ff.q_net.as_str(), Driver::Ff(ff.name.as_str()));
    }
    for iob in &packed.iobs {
        if matches!(iob.dir, IobDir::In) {
            drivers.insert(iob.net.as_str(), Driver::IobIn(iob.name.as_str()));
        }
    }
    for bufg in &packed.bufgs {
        drivers.insert(bufg.out_net.as_str(), Driver::Other);
    }

    let mut sites: HashMap<&str, (i32, i32)> = HashMap::new();
    for lut in &placed.luts {
        if let Some(xy) = tile_xy(&lut.tile) {
            sites.insert(lut.name.as_str(), xy);
        }
    }
    for ff in &placed.ffs {
        if let Some(xy) = tile_xy(&ff.tile) {
            sites.insert(ff.name.as_str(), xy);
        }
    }
    for iob in &placed.iobs {
        if let Some(xy) = tile_xy(&iob.tile) {
            sites.insert(iob.name.as_str(), xy);
        }
    }

    let ctx = Ctx {
        packed,
        tm,
        drivers,
        sites,
    };
    let ff_by_name: HashMap<&str, &crate::pack::PackedFf> =
        packed.ffs.iter().map(|f| (f.name.as_str(), f)).collect();

    let mut wns = i64::MAX;
    let mut tns = 0i64;
    let mut worst_arrival = 0i64;
    let mut worst_endpoint = String::new();
    let mut endpoints = 0usize;

    for pff in &placed.ffs {
        let pff_pack = match ff_by_name.get(pff.name.as_str()) {
            Some(f) => f,
            None => continue,
        };
        let load_xy = ctx.sites.get(pff.name.as_str()).copied().unwrap_or((0, 0));
        let mut visiting = HashSet::new();
        let arrival = ctx.arrival(&pff_pack.d_net, load_xy, 0, &mut visiting)?;
        let slack = period_ps as i64 - tm.setup_ps - arrival;
        endpoints += 1;
        if slack < wns {
            wns = slack;
            worst_endpoint = pff.name.clone();
        }
        if slack < 0 {
            tns += slack;
        }
        worst_arrival = worst_arrival.max(arrival);
    }

    if endpoints == 0 {
        return Ok(TimingReport7 {
            wns_ps: 0,
            tns_ps: 0,
            endpoints: 0,
            worst_arrival_ps: 0,
            worst_endpoint: String::new(),
        });
    }

    Ok(TimingReport7 {
        wns_ps: wns,
        tns_ps: tns,
        endpoints,
        worst_arrival_ps: worst_arrival,
        worst_endpoint,
    })
}

/// Slow-corner cell delays (ps), from Project X-Ray's zynq7/artix7 SDF
/// (`timings/CLBLL_L.sdf`): LUT A*->O6, FF CLK->Q, FF setup, carry paths.
pub const LUT_PS: u32 = 124;
pub const FF_CKQ_PS: u32 = 303;
pub const FF_SETUP_PS: u32 = 109;
/// O6 -> FFMUX -> D and O6 -> carry S inside the slice.
pub const SITE_INTERNAL_PS: u32 = 60;
/// S -> O (sum) through one CARRY4 and CI -> CO through one CARRY4.
pub const CARRY_S_TO_O_PS: u32 = 520;
pub const CARRY_CI_TO_CO_PS: u32 = 293;
/// Route delay assumed for a fabric load the router left no delay for.
pub const UNKNOWN_ROUTE_PS: u32 = 1200;
/// Margin for clock skew/uncertainty and model error.
pub const MARGIN_PS: u32 = 400;

/// One step of a timing path.
#[derive(Clone, Debug)]
pub struct PathStep {
    pub what: String,
    pub arrival_ps: u32,
}

/// Register-to-register timing of a routed design.
#[derive(Clone, Debug)]
pub struct Analysis {
    pub period_ps: u32,
    /// Worst slack (ps); positive when every path meets the period.
    pub wns_ps: i64,
    pub failing_endpoints: usize,
    pub endpoints: usize,
    /// Worst path, launch first.
    pub worst_path: Vec<PathStep>,
}

impl Analysis {
    pub fn fmax_mhz(&self) -> f64 {
        let crit = self.period_ps as i64 - self.wns_ps;
        if crit <= 0 {
            return f64::INFINITY;
        }
        1.0e6 / crit as f64
    }
}

/// Static timing of every flip-flop D/CE/SR input against `period_ps`
/// using routed interconnect delays (`Routed7::load_delays`) and SDF cell
/// delays. Paths start at flip-flop outputs (I/O is unconstrained, as in
/// Vivado without set_input_delay/set_output_delay). A combinational loop
/// is an error.
pub fn analyze(routed: &Routed7, period_ps: u32) -> Result<Analysis, String> {
    let placed = &routed.placed;
    let packed = &placed.packed;
    let site_of: HashMap<&str, &str> = placed
        .luts
        .iter()
        .map(|l| (l.name.as_str(), l.site.as_str()))
        .chain(placed.ffs.iter().map(|f| (f.name.as_str(), f.site.as_str())))
        .chain(placed.carries.iter().map(|c| (c.name.as_str(), c.site.as_str())))
        .collect();
    let lut_by_out: HashMap<&str, &crate::pack::PackedLut> = packed.luts.iter().map(|l| (l.output_net.as_str(), l)).collect();
    let ff_by_q: HashMap<&str, &crate::pack::PackedFf> = packed.ffs.iter().map(|f| (f.q_net.as_str(), f)).collect();
    let mut carry_by_net: HashMap<&str, (&crate::pack::PackedCarry, Option<usize>)> = HashMap::new();
    for c in &packed.carries {
        for (j, o) in c.o_nets.iter().enumerate() {
            if let Some(o) = o {
                carry_by_net.insert(o.as_str(), (c, Some(j)));
            }
        }
        if let Some(co) = &c.co_net {
            carry_by_net.insert(co.as_str(), (c, None));
        }
    }
    // Pins as the router named them: LUT inputs A1..D6 (letter from BEL).
    let lut_letter: HashMap<&str, char> = placed.luts.iter().map(|l| (l.name.as_str(), l.bel.chars().next().unwrap_or('A'))).collect();
    let route = |net: &str, site: &str, pin: &str, internal: bool| -> u32 {
        if internal {
            return SITE_INTERNAL_PS;
        }
        routed
            .load_delays
            .get(&(net.to_string(), site.to_string(), pin.to_string()))
            .copied()
            .unwrap_or(UNKNOWN_ROUTE_PS)
    };
    // Arrival at a net's driver output, with the path that produced it.
    let mut memo: HashMap<String, (u32, Vec<PathStep>)> = HashMap::new();
    fn arrival<'a>(
        net: &str,
        depth: usize,
        memo: &mut HashMap<String, (u32, Vec<PathStep>)>,
        visiting: &mut HashSet<String>,
        lut_by_out: &HashMap<&'a str, &'a crate::pack::PackedLut>,
        ff_by_q: &HashMap<&'a str, &'a crate::pack::PackedFf>,
        carry_by_net: &HashMap<&'a str, (&'a crate::pack::PackedCarry, Option<usize>)>,
        site_of: &HashMap<&'a str, &'a str>,
        lut_letter: &HashMap<&'a str, char>,
        route: &dyn Fn(&str, &str, &str, bool) -> u32,
    ) -> Result<(u32, Vec<PathStep>), String> {
        if let Some(x) = memo.get(net) {
            return Ok(x.clone());
        }
        if depth > 10_000 || !visiting.insert(net.to_string()) {
            return Err(format!("timing: combinational loop through net `{net}`"));
        }
        let r = if ff_by_q.contains_key(net) {
            (FF_CKQ_PS, vec![PathStep { what: format!("{} (flop Q)", ff_by_q[net].name), arrival_ps: FF_CKQ_PS }])
        } else if let Some(l) = lut_by_out.get(net) {
            let site = site_of.get(l.name.as_str()).copied().unwrap_or("");
            let letter = lut_letter.get(l.name.as_str()).copied().unwrap_or('A');
            let mut best = (0u32, Vec::new());
            for (k, inn) in l.input_nets.iter().enumerate() {
                let Some(inn) = inn else { continue };
                let (a, p) = arrival(inn, depth + 1, memo, visiting, lut_by_out, ff_by_q, carry_by_net, site_of, lut_letter, route)?;
                if p.is_empty() {
                    continue; // launched by a pad or constant: unconstrained
                }
                let t = a + route(inn, site, &format!("{letter}{}", k + 1), false);
                if t > best.0 || best.1.is_empty() {
                    best = (t, p);
                }
            }
            if best.1.is_empty() {
                (0, Vec::new())
            } else {
                let t = best.0 + LUT_PS;
                let mut p = best.1;
                p.push(PathStep { what: format!("{} (LUT)", l.name), arrival_ps: t });
                (t, p)
            }
        } else if let Some((c, j)) = carry_by_net.get(net) {
            let site = site_of.get(c.name.as_str()).copied().unwrap_or("");
            let mut best = (0u32, Vec::new());
            let upto = j.map(|j| j + 1).unwrap_or(4);
            for s in c.s_nets.iter().take(upto).flatten() {
                let (a, p) = arrival(s, depth + 1, memo, visiting, lut_by_out, ff_by_q, carry_by_net, site_of, lut_letter, route)?;
                if p.is_empty() {
                    continue;
                }
                let t = a + route(s, site, "S", true);
                if t > best.0 || best.1.is_empty() {
                    best = (t, p);
                }
            }
            if c.cyinit.is_none() && !c.ci_net.is_empty() {
                let (a, p) = arrival(&c.ci_net, depth + 1, memo, visiting, lut_by_out, ff_by_q, carry_by_net, site_of, lut_letter, route)?;
                let cascade = carry_by_net.get(c.ci_net.as_str()).map(|(_, j)| j.is_none()).unwrap_or(false);
                let t = a + if cascade { 0 } else { route(&c.ci_net, site, "AX", false) };
                if !p.is_empty() && (t > best.0 || best.1.is_empty()) {
                    best = (t, p);
                }
            }
            if best.1.is_empty() {
                (0, Vec::new())
            } else {
                let add = if j.is_some() { CARRY_S_TO_O_PS } else { CARRY_CI_TO_CO_PS };
                let t = best.0 + add;
                let mut p = best.1;
                p.push(PathStep { what: format!("{} (CARRY4)", c.name), arrival_ps: t });
                (t, p)
            }
        } else {
            // Input pads, constants, clock buffers: not timed (unconstrained).
            (0, Vec::new())
        };
        visiting.remove(net);
        memo.insert(net.to_string(), r.clone());
        Ok(r)
    }
    let mut visiting = HashSet::new();
    let mut wns = i64::MAX;
    let mut worst_path = Vec::new();
    let mut failing = 0usize;
    let mut endpoints = 0usize;
    for pff in &placed.ffs {
        let Some(ff) = packed.ffs.iter().find(|f| f.name == pff.name) else { continue };
        let internal_d = matches!(crate::place::ff_d_source(pff, placed), crate::place::FfDSource::LutO6 | crate::place::FfDSource::CarryXor);
        let letter = pff.bel.chars().next().unwrap_or('A');
        let d_pin = if internal_d { format!("{letter}FF_D") } else { format!("{letter}X") };
        let mut inputs: Vec<(&str, String, bool)> = vec![(ff.d_net.as_str(), d_pin, internal_d)];
        if let Some(ce) = &ff.ce_net {
            inputs.push((ce.as_str(), "CE".into(), false));
        }
        if let Some(sr) = &ff.sr_net {
            if !ff.sr_async {
                inputs.push((sr.as_str(), "SR".into(), false));
            }
        }
        for (net, pin, internal) in inputs {
            let (a, mut p) = arrival(net, 0, &mut memo, &mut visiting, &lut_by_out, &ff_by_q, &carry_by_net, &site_of, &lut_letter, &route)?;
            if p.is_empty() {
                continue; // driven by an input pad or a constant: unconstrained
            }
            let t = a + route(net, &pff.site, &pin, internal);
            endpoints += 1;
            let slack = period_ps as i64 - FF_SETUP_PS as i64 - MARGIN_PS as i64 - t as i64;
            if slack < 0 {
                failing += 1;
            }
            if slack < wns {
                wns = slack;
                p.push(PathStep { what: format!("{} {pin} (flop input)", ff.name), arrival_ps: t });
                worst_path = p;
            }
        }
    }
    if endpoints == 0 {
        wns = period_ps as i64;
    }
    Ok(Analysis { period_ps, wns_ps: wns, failing_endpoints: failing, endpoints, worst_path })
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_backend::Backend;

    fn loc_constrained_counter() -> helion_ir::Design {
        let mut d = helion_ir::Design::structural_counter();
        for port in &mut d.ports {
            match port.name.as_str() {
                "clk" => port.attrs.set("LOC", "E3"),
                "led" => port.attrs.set("LOC", "H5"),
                _ => {}
            }
        }
        d
    }

    #[test]
    fn tile_xy_parses() {
        assert_eq!(tile_xy("INT_L_X10Y100"), Some((10, 100)));
        assert_eq!(tile_xy("CLBLL_L_X12Y50"), Some((12, 50)));
        assert_eq!(tile_xy("nope"), None);
    }

    #[test]
    fn wns_is_sane_on_blinky_flow() {
        let backend = crate::backend::Xilinx7Backend::open("xc7a35t").expect("open");
        let netlist = loc_constrained_counter();
        let packed = backend.pack(&netlist).expect("pack");
        let placed = backend.place(&packed).expect("place");
        let routed = backend.route(&placed).expect("route");
        let tm = backend.timing_model();
        // 100 MHz target.
        let r = report_timing(&routed, &tm, 10_000).expect("timing");
        assert!(r.endpoints > 0, "no endpoints");
        // Sanity: arrival must be positive but far below the period.
        assert!(r.worst_arrival_ps > 0, "arrival = {}", r.worst_arrival_ps);
        assert!(
            r.worst_arrival_ps < 10_000,
            "arrival {} exceeds period",
            r.worst_arrival_ps
        );
        assert_eq!(r.wns_ps, 10_000 - tm.setup_ps - r.worst_arrival_ps);
    }

    #[test]
    fn combinational_cycle_is_loud() {
        // A LUT feeding itself must error, not return a number.
        let tm = TimingModel::xilinx7();
        let mut packed = Packed7 {
            luts: vec![crate::pack::PackedLut {
                name: "u_lut".into(),
                init: 0,
                input_nets: [
                    Some("n_loop".into()),
                    None,
                    None,
                    None,
                    None,
                    None,
                ],
                output_net: "n_loop".into(),
            }],
            ffs: vec![crate::pack::PackedFf {
                name: "u_ff".into(),
                d_net: "n_loop".into(),
                q_net: "q".into(),
                clk_net: "clk".into(),
                ce_net: None,
                sr_net: None, init: false, srval: false, sr_async: false,
            }],
            iobs: vec![],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![],
            carries: vec![],
            carry_anchor: None,
        };
        let routed = Routed7 {
            placed: Placed7 {
                packed: packed.clone(),
                luts: vec![],
                ffs: vec![crate::place::PlacedFf {
                    name: "u_ff".into(),
                    tile: "CLBLL_L_X10Y10".into(),
                    site: "SLICEL_X0Y0".into(),
                    bel: "AFF".into(),
                }],
                iobs: vec![],
                bufgs: vec![],
                brams: vec![],
            dsps: vec![],
            carries: vec![],
            },
            nets: vec![],
         load_delays: Default::default(), graph_clocks: false };
        let err = report_timing(&routed, &tm, 10_000).expect_err("cycle must fail");
        assert!(err.contains("cycle"), "{err}");
        let _ = &mut packed;
    }
}
