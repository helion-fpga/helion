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
                sr_net: None,
            }],
            iobs: vec![],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![],
            carries: vec![],
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
        };
        let err = report_timing(&routed, &tm, 10_000).expect_err("cycle must fail");
        assert!(err.contains("cycle"), "{err}");
        let _ = &mut packed;
    }
}
