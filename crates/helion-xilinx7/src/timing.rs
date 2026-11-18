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
/// Block RAM, slow corner from X-Ray's `timings/BRAM_L.sdf`: CLK -> DO
/// without the output register, and the worst input setup (DI; address,
/// WE, EN and reset are shorter).
pub const BRAM_CKO_PS: u32 = 2454;
pub const BRAM_SETUP_PS: u32 = 737;

// DSP48E1, slow corner from X-Ray's `timings/DSP_L.sdf` (zynq7). Unlike the
// block RAM the DSP's delay depends heavily on which pipeline registers are
// used, so these are two functions of the register configuration rather than
// one constant. The SDF carries one entry per AREG/DREG/MREG/PREG/OPMODEREG/
// ALUMODEREG/INMODEREG combination; the values below are the slow-corner
// maximum over the entries with that configuration.
//
// Measured spread on CLK -> P alone: 0.192 ns (fully pipelined) to 2.304 ns
// (INMODEREG with no output register). A single constant would be wrong by
// more than a factor of ten at one end, which is why G32 asks for the ports to
// be modelled rather than merely listed.

/// DSP48E1 `CLK -> P` in ps, from the device's pipeline register settings.
///
/// The output register is what breaks the path: with `PREG=1` the P pin is a
/// flop output, so a path *to* P's consumers starts here; with `PREG=0` the
/// value comes through the M register and the adder, which the SDF prices at
/// 1.589..=2.304 ns against 0.192..=0.758 ns when registered.
pub fn dsp_cko_ps(d: &crate::pack::PackedDsp) -> u32 {
    // Slow-corner maxima bucketed by the register stage that produces P.
    if d.preg == 1 {
        758 // finest-grained P register
    } else if d.mreg == 1 {
        1589
    } else if d.opmodereg == 1 || d.alumodereg == 1 || d.inmodereg == 1 {
        2304
    } else {
        2234 // no P-side register at all
    }
}

/// DSP48E1 input setup in ps for a site pin, from the device's settings.
///
/// Only the clocked (registered) inputs have a setup requirement. With
/// `AREG=0` the A/B/D pins feed the multiplier combinationally, so there is no
/// setup edge to fail — the path simply gets longer, which the row's guard
/// checks by requiring a design with AREG=0 to be reported differently from one
/// with AREG=1. INMODE is the worst input in the SDF (5.557 ns).
pub fn dsp_setup_ps(pin: &str) -> Option<u32> {
    let p = pin.to_ascii_uppercase();
    let v = if p.starts_with('A') && !p.starts_with("ALUMODE") {
        5474 // A (worst of A/ACASCREG paths)
    } else if p.starts_with('B') {
        3929
    } else if p == "C" {
        2093
    } else if p.starts_with('D') {
        5342
    } else if p.starts_with("INMODE") {
        5557
    } else if p.starts_with("OPMODE") {
        2600
    } else if p.starts_with("ALUMODE") {
        2369
    } else if p.starts_with("CARRYIN") {
        1690
    } else if p.starts_with("CARRYINSEL") {
        2184
    } else {
        return None;
    };
    Some(v)
}

/// One step of a timing path.
#[derive(Clone, Debug)]
pub struct PathStep {
    pub what: String,
    pub arrival_ps: u32,
    /// Routed delay of the wire into this step.
    pub wire_ps: u32,
    /// Tile of the cell, to set the route against the placer's estimate.
    pub xy: Option<(i32, i32)>,
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
    /// Worst slack (ps) of each clock domain, by clock net.
    pub clocks: Vec<(String, i64)>,
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
    let mut a = analyze_clocks(routed, &|_| Some(period_ps))?;
    a.period_ps = period_ps;
    Ok(a)
}

/// Like [`analyze`], with a period per clock domain. A domain is a flop
/// clock net (after packing, e.g. `clk_bufg`); `period_of` gives its period,
/// or `None` when it is not known, which is an error. Paths are timed
/// within one domain. A data path from a flop of one domain to a flop of
/// another is an error too: the clocks' arrival times are not modelled, so
/// such a path cannot be called safe.
///
/// The returned `period_ps` is the period of the domain with the worst
/// slack.
pub fn analyze_clocks(routed: &Routed7, period_of: &dyn Fn(&str) -> Option<u32>) -> Result<Analysis, String> {
    analyze_clocks_async(routed, period_of, &|_, _| false)
}

/// Like [`analyze_clocks`], where `is_async(launch, capture)` names clock
/// pairs declared asynchronous (`set_clock_groups -asynchronous`). Between
/// those, a flop's D may come straight from another flop's Q (a
/// synchronizer's first stage, or data held stable by a handshake); any
/// logic on such a path is still an error. The path is still timed, against
/// the capturing clock's period.
pub fn analyze_clocks_async(
    routed: &Routed7,
    period_of: &dyn Fn(&str) -> Option<u32>,
    is_async: &dyn Fn(&str, &str) -> bool,
) -> Result<Analysis, String> {
    let mut domains: Vec<&str> = routed.placed.packed.ffs.iter().map(|f| f.clk_net.as_str()).chain(routed.placed.packed.lutrams.iter().map(|r| r.clk.as_str())).collect();
    domains.sort();
    domains.dedup();
    if domains.len() > 1 {
        check_no_crossings(&routed.placed.packed, is_async)?;
    }
    let mut worst: Option<Analysis> = None;
    let (mut endpoints, mut failing) = (0usize, 0usize);
    let mut clocks = Vec::new();
    for dom in &domains {
        let period = period_of(dom).ok_or_else(|| format!("timing: no period known for clock `{dom}`"))?;
        let a = analyze_domain(routed, dom, period)?;
        endpoints += a.endpoints;
        failing += a.failing_endpoints;
        clocks.push((dom.to_string(), a.wns_ps));
        if worst.as_ref().map(|w| a.wns_ps < w.wns_ps).unwrap_or(true) {
            worst = Some(a);
        }
    }
    let mut w = worst.unwrap_or(Analysis { period_ps: 0, wns_ps: 0, failing_endpoints: 0, endpoints: 0, worst_path: Vec::new(), clocks: Vec::new() });
    if domains.is_empty() {
        w.period_ps = period_of("").unwrap_or(0);
        w.wns_ps = w.period_ps as i64;
    }
    w.endpoints = endpoints;
    w.failing_endpoints = failing;
    w.clocks = clocks;
    Ok(w)
}

/// Every flop input's combinational fan-in reaches flops of its own clock
/// only.
fn check_no_crossings(packed: &crate::pack::Packed7, is_async: &dyn Fn(&str, &str) -> bool) -> Result<(), String> {
    let lut_by_out: HashMap<&str, &crate::pack::PackedLut> = packed.luts.iter().map(|l| (l.output_net.as_str(), l)).collect();
    let ff_by_q: HashMap<&str, &crate::pack::PackedFf> = packed.ffs.iter().map(|f| (f.q_net.as_str(), f)).collect();
    let mut carry_ins: HashMap<&str, Vec<&str>> = HashMap::new();
    for c in &packed.carries {
        let mut ins: Vec<&str> = c.s_nets.iter().chain(c.di_nets.iter()).flatten().map(|n| n.as_str()).collect();
        if !c.ci_net.is_empty() {
            ins.push(c.ci_net.as_str());
        }
        for o in c.o_nets.iter().flatten().chain(c.co_net.iter()).chain(c.co_nets.iter().flatten()) {
            carry_ins.insert(o.as_str(), ins.clone());
        }
    }
    // Clock of the flops that launch into each net (memoized walk).
    let mut memo: HashMap<&str, Vec<&str>> = HashMap::new();
    fn launches<'a>(
        net: &'a str,
        memo: &mut HashMap<&'a str, Vec<&'a str>>,
        lut_by_out: &HashMap<&'a str, &'a crate::pack::PackedLut>,
        ff_by_q: &HashMap<&'a str, &'a crate::pack::PackedFf>,
        carry_ins: &HashMap<&'a str, Vec<&'a str>>,
        depth: usize,
    ) -> Vec<&'a str> {
        if let Some(v) = memo.get(net) {
            return v.clone();
        }
        memo.insert(net, Vec::new()); // a loop is reported by the timing pass
        let mut out: Vec<&str> = if let Some(f) = ff_by_q.get(net) {
            vec![f.clk_net.as_str()]
        } else if depth > 10_000 {
            Vec::new()
        } else {
            let ins: Vec<&str> = if let Some(l) = lut_by_out.get(net) {
                l.input_nets.iter().flatten().map(|n| n.as_str()).collect()
            } else {
                carry_ins.get(net).cloned().unwrap_or_default()
            };
            let mut v = Vec::new();
            for i in ins {
                v.extend(launches(i, memo, lut_by_out, ff_by_q, carry_ins, depth + 1));
            }
            v
        };
        out.sort();
        out.dedup();
        memo.insert(net, out.clone());
        out
    }
    for ff in &packed.ffs {
        let mut ins: Vec<&str> = vec![ff.d_net.as_str()];
        ins.extend(ff.ce_net.iter().map(|s| s.as_str()));
        if !ff.sr_async {
            ins.extend(ff.sr_net.iter().map(|s| s.as_str()));
        }
        for (k, i) in ins.into_iter().enumerate() {
            // A declared-asynchronous crossing straight from a flop into D.
            if k == 0 && ff_by_q.get(i).is_some_and(|src| is_async(&src.clk_net, &ff.clk_net)) {
                continue;
            }
            for c in launches(i, &mut memo, &lut_by_out, &ff_by_q, &carry_ins, 0) {
                if c != ff.clk_net {
                    return Err(format!(
                        "timing: data path from clock `{c}` into flop {} on clock `{}`; paths between clock domains are not supported",
                        ff.name, ff.clk_net
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Paths launched by the flops of clock `domain`, timed against `period_ps`.
fn analyze_domain(routed: &Routed7, domain: &str, period_ps: u32) -> Result<Analysis, String> {
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
    let xy_of: HashMap<&str, (i32, i32)> = placed
        .luts
        .iter()
        .map(|l| (l.name.as_str(), l.tile.as_str()))
        .chain(placed.ffs.iter().map(|f| (f.name.as_str(), f.tile.as_str())))
        .chain(placed.carries.iter().map(|c| (c.name.as_str(), c.tile.as_str())))
        .filter_map(|(n, t)| Some((n, tile_xy(t)?)))
        .collect();
    // Only this domain's flops launch paths; another domain's flop output is
    // a path start too, tagged so that reaching an endpoint is reported.
    let ff_by_q: HashMap<&str, &crate::pack::PackedFf> = packed.ffs.iter().map(|f| (f.q_net.as_str(), f)).collect();
    let mut carry_by_net: HashMap<&str, (&crate::pack::PackedCarry, Option<usize>)> = HashMap::new();
    for c in &packed.carries {
        for (j, o) in c.o_nets.iter().enumerate() {
            if let Some(o) = o {
                carry_by_net.insert(o.as_str(), (c, Some(j)));
            }
        }
        // CO0-CO2 taps are timed like CO3 (shortcut: CO3's delay is the
        // longest of the four, so this is pessimistic for the taps).
        for co in c.co_net.iter().chain(c.co_nets.iter().flatten()) {
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
        bram_out: &HashMap<&'a str, &'a str>,
        dsp_out: &HashMap<&'a str, (&'a str, u32)>,
        xy_of: &HashMap<&'a str, (i32, i32)>,
        lr: &HashMap<&'a str, crate::lutram::ReadArc<'a>>,
    ) -> Result<(u32, Vec<PathStep>), String> {
        if let Some(x) = memo.get(net) {
            return Ok(x.clone());
        }
        if depth > 10_000 || !visiting.insert(net.to_string()) {
            return Err(format!("timing: combinational loop through net `{net}`"));
        }
        let r = if let Some((dn, cko)) = dsp_out.get(net) {
            // P (or another CLK-launched DSP output) starts a path here.
            (*cko, vec![PathStep { what: format!("{dn} (DSP48E1 output)"), arrival_ps: *cko, wire_ps: 0, xy: None }])
        } else if let Some(b) = bram_out.get(net) {
            (BRAM_CKO_PS, vec![PathStep { what: format!("{b} (block RAM DO)"), arrival_ps: BRAM_CKO_PS, wire_ps: 0, xy: None }])
        } else if let Some(r) = lr.get(net) {
            // LUT RAM: launched by its clock, or read from its address pins.
            let mut best = (r.cko_ps, vec![PathStep { what: format!("{} (LUT RAM, clock {}, {})", r.name, r.clk, r.site), arrival_ps: r.cko_ps, wire_ps: 0, xy: None }]);
            for (inn, pin) in &r.ins {
                let (a, mut p) = arrival(inn, depth + 1, memo, visiting, lut_by_out, ff_by_q, carry_by_net, site_of, lut_letter, route, bram_out, dsp_out, xy_of, lr)?;
                let w = route(inn, r.site, pin, false);
                let t = a + w + r.read_ps;
                if !p.is_empty() && t > best.0 {
                    p.push(PathStep { what: format!("{} (LUT RAM read)", r.name), arrival_ps: t, wire_ps: w, xy: None });
                    best = (t, p);
                }
            }
            best
        } else if ff_by_q.contains_key(net) {
            let f = ff_by_q[net];
            (FF_CKQ_PS, vec![PathStep { what: format!("{} (flop Q, clock {}, {})", f.name, f.clk_net, site_of.get(f.name.as_str()).copied().unwrap_or("?")), arrival_ps: FF_CKQ_PS, wire_ps: 0, xy: xy_of.get(f.name.as_str()).copied() }])
        } else if let Some(l) = lut_by_out.get(net) {
            let site = site_of.get(l.name.as_str()).copied().unwrap_or("");
            let letter = lut_letter.get(l.name.as_str()).copied().unwrap_or('A');
            let mut best = (0u32, Vec::new());
            let mut wire = 0;
            for (k, inn) in l.input_nets.iter().enumerate() {
                let Some(inn) = inn else { continue };
                let (a, p) = arrival(inn, depth + 1, memo, visiting, lut_by_out, ff_by_q, carry_by_net, site_of, lut_letter, route, bram_out, dsp_out, xy_of, lr)?;
                if p.is_empty() {
                    continue; // launched by a pad or constant: unconstrained
                }
                let w = route(inn, site, &format!("{letter}{}", k + 1), false);
                if a + w > best.0 || best.1.is_empty() {
                    best = (a + w, p);
                    wire = w;
                }
            }
            if best.1.is_empty() {
                (0, Vec::new())
            } else {
                let t = best.0 + LUT_PS;
                let mut p = best.1;
                p.push(PathStep { what: format!("{} (LUT, {site})", l.name), arrival_ps: t, wire_ps: wire, xy: xy_of.get(l.name.as_str()).copied() });
                (t, p)
            }
        } else if let Some((c, j)) = carry_by_net.get(net) {
            let site = site_of.get(c.name.as_str()).copied().unwrap_or("");
            let mut best = (0u32, Vec::new());
            let mut wire = 0;
            let upto = j.map(|j| j + 1).unwrap_or(4);
            for s in c.s_nets.iter().take(upto).flatten() {
                let (a, p) = arrival(s, depth + 1, memo, visiting, lut_by_out, ff_by_q, carry_by_net, site_of, lut_letter, route, bram_out, dsp_out, xy_of, lr)?;
                if p.is_empty() {
                    continue;
                }
                let w = route(s, site, "S", true);
                if a + w > best.0 || best.1.is_empty() {
                    best = (a + w, p);
                    wire = w;
                }
            }
            if c.cyinit.is_none() && !c.ci_net.is_empty() {
                let (a, p) = arrival(&c.ci_net, depth + 1, memo, visiting, lut_by_out, ff_by_q, carry_by_net, site_of, lut_letter, route, bram_out, dsp_out, xy_of, lr)?;
                let cascade = carry_by_net.get(c.ci_net.as_str()).map(|(_, j)| j.is_none()).unwrap_or(false);
                let w = if cascade { 0 } else { route(&c.ci_net, site, "AX", false) };
                if !p.is_empty() && (a + w > best.0 || best.1.is_empty()) {
                    best = (a + w, p);
                    wire = w;
                }
            }
            if best.1.is_empty() {
                (0, Vec::new())
            } else {
                let add = if j.is_some() { CARRY_S_TO_O_PS } else { CARRY_CI_TO_CO_PS };
                let t = best.0 + add;
                let mut p = best.1;
                p.push(PathStep { what: format!("{} (CARRY4, {})", c.name, site_of.get(c.name.as_str()).copied().unwrap_or("?")), arrival_ps: t, wire_ps: wire, xy: xy_of.get(c.name.as_str()).copied() });
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
    // Block RAM ports, timed like flops: every RAM output launches paths
    // (another clock's too, so a crossing is timed against the capturing
    // clock, never skipped), and this clock's RAM inputs end them.
    let mut bram_out: HashMap<&str, &str> = HashMap::new();
    let mut bram_in: Vec<(&str, &str, String)> = Vec::new(); // (net, site, site pin)
    for b in &packed.brams {
        let Some(pb) = placed.brams.iter().find(|p| p.name == b.name) else { continue };
        for (pin, net, is_out) in &b.pins {
            if pin.starts_with("CLK") || pin.starts_with("REGCLK") {
                continue;
            }
            let port_a = pin.starts_with("DOA") || pin.starts_with("DOPA") || pin.starts_with("DIA") || pin.starts_with("DIPA")
                || pin.starts_with("ADDRA") || pin.starts_with("WEA") || pin.starts_with("ENA") || pin.ends_with("ARSTRAM")
                || pin.ends_with("ARSTREG") || pin.starts_with("REGCEA");
            let clk = if port_a { &b.clka_net } else { &b.clkb_net };
            if clk.is_none() {
                continue;
            }
            if *is_out {
                bram_out.insert(net.as_str(), b.name.as_str());
            } else if clk.as_deref() == Some(domain) {
                for sp in crate::bram::site_pins(&pb.site, pin)? {
                    bram_in.push((net.as_str(), pb.site.as_str(), sp));
                }
            }
        }
    }
    // DSP48E1 outputs and registered inputs (G32).
    //
    // Before this, a DSP's P pin was neither an arrival source nor an
    // endpoint, so a path through a DSP simply vanished from the analysis and
    // the design looked faster than it is. P is launched by CLK with the
    // device's own pipeline-depth-dependent delay; a clocked data/control input
    // is a setup endpoint.
    let mut dsp_out: HashMap<&str, (&str, u32)> = HashMap::new();
    let mut dsp_in: Vec<(&str, &str, String, u32)> = Vec::new(); // (net, site, pin, setup)
    let dsp_clk_by_name: HashMap<&str, Option<&str>> = packed
        .dsps
        .iter()
        .map(|d| {
            let clk = d.pins.iter().find(|(p, _, out)| !*out && (p == "CLK" || p == "CLKIN")).map(|(_, n, _)| n.as_str());
            (d.name.as_str(), clk)
        })
        .collect();
    for d in &packed.dsps {
        let Some(pd) = placed.dsps.iter().find(|p| p.name == d.name) else { continue };
        let on_this_clock = dsp_clk_by_name.get(d.name.as_str()).copied().flatten() == Some(domain);
        let cko = dsp_cko_ps(d);
        for (pin, net, is_out) in &d.pins {
            if *is_out {
                // P, PCOUT, CARRYOUT, ... all launch from CLK.
                if pin.starts_with('P') || pin.starts_with("CARRYOUT") {
                    dsp_out.insert(net.as_str(), (d.name.as_str(), cko));
                }
            } else if on_this_clock {
                if let Some(setup) = dsp_setup_ps(pin) {
                    dsp_in.push((net.as_str(), pd.site.as_str(), pin.clone(), setup));
                }
            }
        }
    }
    let lr = crate::lutram::read_arcs(placed);
    let mut visiting = HashSet::new();
    let mut wns = i64::MAX;
    let mut worst_path = Vec::new();
    let mut failing = 0usize;
    let mut endpoints = 0usize;
    for pff in &placed.ffs {
        let Some(ff) = packed.ffs.iter().find(|f| f.name == pff.name) else { continue };
        if ff.clk_net != domain {
            continue;
        }
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
            let (a, mut p) = arrival(net, 0, &mut memo, &mut visiting, &lut_by_out, &ff_by_q, &carry_by_net, &site_of, &lut_letter, &route, &&bram_out, &dsp_out, &xy_of, &lr)?;
            if p.is_empty() {
                continue; // driven by an input pad or a constant: unconstrained
            }
            let w = route(net, &pff.site, &pin, internal);
            let t = a + w;
            endpoints += 1;
            let slack = period_ps as i64 - FF_SETUP_PS as i64 - MARGIN_PS as i64 - t as i64;
            if slack < 0 {
                failing += 1;
            }
            if slack < wns {
                wns = slack;
                p.push(PathStep { what: format!("{} {pin} (flop input, {})", ff.name, site_of.get(ff.name.as_str()).copied().unwrap_or("?")), arrival_ps: t, wire_ps: w, xy: xy_of.get(ff.name.as_str()).copied() });
                worst_path = p;
            }
        }
    }
    // LUT RAM writes: data, write address and enable, against this clock.
    for (net, site, pin, setup, clk) in crate::lutram::write_ends(placed) {
        if clk != domain {
            continue;
        }
        let (a, mut p) = arrival(net, 0, &mut memo, &mut visiting, &lut_by_out, &ff_by_q, &carry_by_net, &site_of, &lut_letter, &route, &bram_out, &dsp_out, &xy_of, &lr)?;
        if p.is_empty() {
            continue;
        }
        let t = a + route(net, site, pin, false);
        endpoints += 1;
        let slack = period_ps as i64 - setup as i64 - MARGIN_PS as i64 - t as i64;
        if slack < 0 {
            failing += 1;
        }
        if slack < wns {
            wns = slack;
            p.push(PathStep { what: format!("{site}.{pin} (LUT RAM input)"), arrival_ps: t, wire_ps: t - a, xy: None });
            worst_path = p;
        }
    }
    for (net, site, sp) in &bram_in {
        let (a, mut p) = arrival(net, 0, &mut memo, &mut visiting, &lut_by_out, &ff_by_q, &carry_by_net, &site_of, &lut_letter, &route, &bram_out, &dsp_out, &xy_of, &lr)?;
        if p.is_empty() {
            continue;
        }
        let t = a + route(net, site, sp, false);
        endpoints += 1;
        let slack = period_ps as i64 - BRAM_SETUP_PS as i64 - MARGIN_PS as i64 - t as i64;
        if slack < 0 {
            failing += 1;
        }
        if slack < wns {
            wns = slack;
            p.push(PathStep { what: format!("{site}.{sp} (block RAM input)"), arrival_ps: t, wire_ps: t - a, xy: None });
            worst_path = p;
        }
    }
    for (net, site, pin, setup) in &dsp_in {
        let (a, mut p) = arrival(net, 0, &mut memo, &mut visiting, &lut_by_out, &ff_by_q, &carry_by_net, &site_of, &lut_letter, &route, &bram_out, &dsp_out, &xy_of, &lr)?;
        if p.is_empty() {
            continue;
        }
        let t = a + route(net, site, pin, false);
        endpoints += 1;
        let slack = period_ps as i64 - *setup as i64 - MARGIN_PS as i64 - t as i64;
        if slack < 0 {
            failing += 1;
        }
        if slack < wns {
            wns = slack;
            p.push(PathStep { what: format!("{site}.{pin} (DSP48E1 input)"), arrival_ps: t, wire_ps: t - a, xy: None });
            worst_path = p;
        }
    }
    if endpoints == 0 {
        wns = period_ps as i64;
    }
    Ok(Analysis { period_ps, wns_ps: wns, failing_endpoints: failing, endpoints, worst_path, clocks: vec![(domain.to_string(), wns)] })
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_backend::Backend;

    fn loc_constrained_counter() -> helion_ir::Design {
        let mut d = helion_ir::Design::structural_counter();
        for port in &mut d.ports {
            match helion_ir::resolve(port.name) {
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

    /// A data path from a flop on one clock into a flop on another is
    /// refused; paths within one clock are fine.
    #[test]
    fn clock_domain_crossing_is_refused() {
        use crate::pack::{Packed7, PackedFf, PackedLut};
        let ff = |name: &str, d: &str, q: &str, clk: &str| PackedFf {
            name: name.into(),
            d_net: d.into(),
            q_net: q.into(),
            clk_net: clk.into(),
            ce_net: None,
            sr_net: None,
            init: false,
            srval: false,
            sr_async: false,
        };
        let mut p = Packed7 {
            luts: vec![PackedLut {
                name: "u_inv".into(),
                init: 0x5555_5555_5555_5555,
                input_nets: [Some("qa".into()), None, None, None, None, None],
                output_net: "x".into(),
            }],
            ffs: vec![ff("a", "pa", "qa", "clk_a"), ff("b", "x", "qb", "clk_a")],
            ..Default::default()
        };
        assert!(check_no_crossings(&p, &|_, _| false).is_ok());
        p.ffs[1].clk_net = "clk_b".into();
        let err = check_no_crossings(&p, &|_, _| false).unwrap_err();
        assert!(err.contains("from clock `clk_a`") && err.contains("flop b"), "got: {err}");
        // Declared asynchronous: still refused through logic (the inverter),
        // allowed straight from a flop (a synchronizer's first stage).
        let asy = |a: &str, b: &str| a != b;
        assert!(check_no_crossings(&p, &asy).is_err());
        p.ffs[1].d_net = "qa".into();
        assert!(check_no_crossings(&p, &asy).is_ok());
        assert!(check_no_crossings(&p, &|_, _| false).is_err());
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
            ..Default::default()
        };
        let routed = Routed7 {
            placed: Placed7 {
                packed: packed.clone(),
                ffs: vec![crate::place::PlacedFf {
                    name: "u_ff".into(),
                    tile: "CLBLL_L_X10Y10".into(),
                    site: "SLICEL_X0Y0".into(),
                    bel: "AFF".into(),
                }],
                ..Default::default()
            },
            nets: vec![],
         load_delays: Default::default(), graph_clocks: false };
        let err = report_timing(&routed, &tm, 10_000).expect_err("cycle must fail");
        assert!(err.contains("cycle"), "{err}");
        let _ = &mut packed;
    }
}
