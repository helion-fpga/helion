//! Static timing analysis for UltraScale+ placed/routed netlists.
//!
//! Mirrors the existing hang_diag fields (WNS_PS, TNS_PS, endpoints, r2r_ps,
//! iob_ps) so the multiseed agent can parse output unchanged.
//!
//! This is a preliminary STA using
//! [`crate::timing_db::preliminary_xcvu3p`]. It is NOT Vivado-correlated
//! signoff timing.

use crate::timing_db::UltraScaleTimingDb;
use std::collections::{HashMap, HashSet};

/// Timing report mirroring the hang_diag fields.
///
/// This is the INTERFACES.md TimingReport contract. Field names and semantics
/// match the existing `hang_diag timing` line so multiseed parses it unchanged.
#[derive(Clone, Debug)]
pub struct TimingReport {
    /// Worst negative slack across all endpoints (ps). Negative means violation.
    pub wns_ps: i64,
    /// Total negative slack (ps).
    pub tns_ps: i64,
    /// Number of timing endpoints analyzed.
    pub endpoints: usize,
    /// Worst register-to-register arrival (ps).
    pub r2r_ps: i64,
    /// Worst I/O pad delay (ps).
    pub iob_ps: i64,
    /// Worst slack path, startpoint cell name.
    pub worst_startpoint: String,
    /// Worst slack path, endpoint cell name.
    pub worst_endpoint: String,
    /// Worst path delay (ps).
    pub worst_delay_ps: i64,
}

impl TimingReport {
    /// Format exactly like the existing hang_diag timing line.
    pub fn hang_diag_line(&self, ms: u128) -> String {
        format!(
            "hang_diag timing WNS_PS={} TNS_PS={} endpoints={} r2r_ps={} iob_ps={} ms={}",
            self.wns_ps, self.tns_ps, self.endpoints, self.r2r_ps, self.iob_ps, ms
        )
    }
}

/// Minimal placed/routed netlist view for STA.
///
/// The pack-place and router agents fill the real structures; this trait lets
/// STA code against the shape now. When the real types land, this adapts.
pub trait StaNetlist {
    /// All flip-flop cells: (cell_name, tile_x, tile_y).
    fn flip_flops(&self) -> Vec<(String, i32, i32)>;
    /// All LUT cells: (cell_name, tile_x, tile_y).
    fn luts(&self) -> Vec<(String, i32, i32)>;
    /// Net driver: net name -> (driver_cell, tile_x, tile_y).
    /// For FF Q outputs, driver_cell is "ff:<name>".
    fn net_drivers(&self) -> HashMap<String, (String, i32, i32)>;
    /// Cell fanin: cell name -> list of input net names.
    fn cell_fanin(&self) -> HashMap<String, Vec<String>>;
    /// Net name driven by a flip-flop's Q pin.
    fn ff_q_net(&self, ff: &str) -> Option<String>;
    /// Net name feeding a flip-flop's D pin.
    fn ff_d_net(&self, ff: &str) -> Option<String>;
}

/// Run STA over a placed/routed netlist.
///
/// `period_ps` is the target clock period. Returns an error on combinational
/// cycles or when a required delay is UNKNOWN in the TimingDb.
pub fn sta_routed<N: StaNetlist>(
    nl: &N,
    db: &UltraScaleTimingDb,
    period_ps: i64,
) -> Result<TimingReport, String> {
    let lut_ps = db
        .cells
        .lut6
        .ps
        .ok_or_else(|| "timing: LUT6 delay UNKNOWN in TimingDb".to_string())?;
    let ckq_ps = db
        .cells
        .ff_ckq
        .ps
        .ok_or_else(|| "timing: FF clock-to-Q UNKNOWN in TimingDb".to_string())?;
    let setup_ps = db
        .cells
        .ff_setup
        .ps
        .ok_or_else(|| "timing: FF setup UNKNOWN in TimingDb".to_string())?;
    let hop_ps = db
        .interconnect
        .single
        .ps
        .ok_or_else(|| "timing: INT single-hop delay UNKNOWN in TimingDb".to_string())?;

    let drivers = nl.net_drivers();
    let fanin = nl.cell_fanin();
    let ffs = nl.flip_flops();

    let mut sites: HashMap<String, (i32, i32)> = HashMap::new();
    for (name, x, y) in nl.flip_flops() {
        sites.insert(name, (x, y));
    }
    for (name, x, y) in nl.luts() {
        sites.insert(name, (x, y));
    }

    let route_delay = |from: (i32, i32), to: (i32, i32)| -> i64 {
        ((from.0 - to.0).abs() + (from.1 - to.1).abs()) as i64 * hop_ps
    };

    #[allow(clippy::too_many_arguments)]
    fn arrival(
        net: &str,
        load_xy: (i32, i32),
        drivers: &HashMap<String, (String, i32, i32)>,
        fanin: &HashMap<String, Vec<String>>,
        sites: &HashMap<String, (i32, i32)>,
        lut_ps: i64,
        ckq_ps: i64,
        route_delay: &dyn Fn((i32, i32), (i32, i32)) -> i64,
        depth: usize,
        visiting: &mut HashSet<String>,
    ) -> Result<(i64, String), String> {
        if depth > 64 {
            return Err(format!("timing: combinational cycle through net {net}"));
        }
        if !visiting.insert(net.to_string()) {
            return Err(format!("timing: combinational cycle through net {net}"));
        }
        let result = (|| -> Result<(i64, String), String> {
            let (drv, dx, dy) = drivers
                .get(net)
                .ok_or_else(|| format!("timing: no driver for net {net}"))?;
            let wire = route_delay((*dx, *dy), load_xy);
            if let Some(ff_name) = drv.strip_prefix("ff:") {
                return Ok((ckq_ps + wire, ff_name.to_string()));
            }
            let inputs = fanin.get(drv).cloned().unwrap_or_default();
            let drv_xy = sites.get(drv).copied().unwrap_or((*dx, *dy));
            let mut worst = 0i64;
            let mut worst_src = String::new();
            for inp in &inputs {
                let (a, src) = arrival(
                    inp,
                    drv_xy,
                    drivers,
                    fanin,
                    sites,
                    lut_ps,
                    ckq_ps,
                    route_delay,
                    depth + 1,
                    visiting,
                )?;
                if a > worst {
                    worst = a;
                    worst_src = src;
                }
            }
            Ok((worst + lut_ps + wire, worst_src))
        })();
        visiting.remove(net);
        result
    }

    let mut wns_ps = i64::MAX;
    let mut tns_ps = 0i64;
    let mut worst_r2r = 0i64;
    let mut worst_start = String::new();
    let mut worst_end = String::new();
    let mut worst_delay = 0i64;
    let mut endpoints = 0usize;

    for (ff, fx, fy) in &ffs {
        let Some(d_net) = nl.ff_d_net(ff) else {
            continue;
        };
        endpoints += 1;
        let mut visiting = HashSet::new();
        let (arr, startpoint) = arrival(
            &d_net,
            (*fx, *fy),
            &drivers,
            &fanin,
            &sites,
            lut_ps,
            ckq_ps,
            &route_delay,
            0,
            &mut visiting,
        )?;
        let slack = period_ps - setup_ps - arr;
        if slack < wns_ps {
            wns_ps = slack;
            worst_start = startpoint.clone();
            worst_end = ff.clone();
            worst_delay = arr;
        }
        if slack < 0 {
            tns_ps += slack;
        }
        if arr > worst_r2r {
            worst_r2r = arr;
        }
    }

    if endpoints == 0 {
        return Err("timing: no endpoints".to_string());
    }

    Ok(TimingReport {
        wns_ps,
        tns_ps,
        endpoints,
        r2r_ps: worst_r2r,
        iob_ps: 0,
        worst_startpoint: worst_start,
        worst_endpoint: worst_end,
        worst_delay_ps: worst_delay,
    })
}
