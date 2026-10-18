//! Vivado UG835 / UG906 / UG949 reporting suite for [`Session`]:
//! `report_timing_summary`, `report_timing`, `report_utilization`,
//! `report_power`, `report_drc`, `report_cdc`, `report_clock_interaction`,
//! `report_clock_utilization`, `report_high_fanout_nets`, and
//! `report_design_analysis`.

use crate::Session;
use helion_device::Device;
use helion_ir::{CellKind, Design, intern, resolve};
use helion_place::Placed;
use helion_route::Routed;
use helion_sta::{
    Clock, Constraints, TimingResult, create_clock, report_cdc, report_clock_interaction,
    report_clock_networks, report_power, report_timing_placed, report_timing_routed_xdc,
    report_timing_summary, report_timing_xdc, timing_paths_routed,
};
use std::collections::{BTreeMap, HashSet};

fn effective_clocks(xdc: &Constraints) -> Vec<Clock> {
    let mut clks = xdc.clocks.clone();
    if clks.is_empty() {
        create_clock(&mut clks, "clk", 10_000, "clk");
    }
    clks
}

fn eval_session_timing(
    session: &Session,
    xdc: &Constraints,
) -> Result<(TimingResult, Vec<Clock>), String> {
    let design = session
        .design
        .as_ref()
        .ok_or("timing report: no synthesized design")?;
    let clks = effective_clocks(xdc);
    let tr = if let Some(routed) = &session.routed {
        report_timing_routed_xdc(design, routed, &clks, xdc)?
    } else if let Some(placed) = &session.placed {
        report_timing_placed(design, placed, &clks)?
    } else {
        report_timing_xdc(design, &clks, xdc)?
    };
    Ok((tr, clks))
}

/// UG835 / UG903 `report_timing_summary`: WNS, TNS, WHS, THS, clock summary, and path groups.
pub fn report_timing_summary_text(
    session: &Session,
    _dev: &Device,
    xdc: &Constraints,
) -> Result<String, String> {
    let design = session
        .design
        .as_ref()
        .ok_or("report_timing_summary: no design")?;
    let (tr, clks) = eval_session_timing(session, xdc)?;
    let summary = report_timing_summary(&clks, xdc, Some(&tr));
    let mut out = String::new();
    out.push_str(&format!(
        "report_timing_summary design={} WNS_PS={} TNS_PS={} WHS_PS={} THS_PS={} endpoints={}\n",
        resolve(design.name),
        tr.wns_ps,
        tr.tns_ps,
        tr.hold_slack_ps,
        if tr.hold_slack_ps < 0 {
            tr.hold_slack_ps
        } else {
            0
        },
        tr.endpoints
    ));
    out.push_str(&summary.text());
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// UG835 `report_timing` with optional `-max_paths`, `-delay_type`, `-from`, `-to`, `-through`.
#[allow(clippy::too_many_arguments)]
pub fn report_timing_detailed(
    session: &Session,
    dev: &Device,
    xdc: &Constraints,
    max_paths: usize,
    delay_type: &str,
    from: Option<&str>,
    to: Option<&str>,
    through: Option<&str>,
) -> Result<String, String> {
    let _ = dev;
    let design = session.design.as_ref().ok_or("report_timing: no design")?;
    let (tr, _clks) = eval_session_timing(session, xdc)?;
    let mut out = format!(
        "report_timing {} WNS_PS={} TNS_PS={} SETUP_PS={} HOLD_PS={} HOLD_SLACK_PS={} endpoints={} r2r_ps={} iob_ps={} route_ps={} delay_type={}",
        resolve(design.name),
        tr.wns_ps,
        tr.tns_ps,
        tr.setup_ps,
        tr.hold_ps,
        tr.hold_slack_ps,
        tr.endpoints,
        tr.r2r_ps,
        tr.iob_ps,
        tr.route_ps,
        delay_type
    );
    if let Some(routed) = &session.routed {
        let mut paths = timing_paths_routed(design, routed, &tr);
        if let Some(f) = from {
            paths.retain(|p| p.startpoint.cell.contains(f) || p.startpoint.bel_id.contains(f));
        }
        if let Some(t) = to {
            paths.retain(|p| p.endpoint.cell.contains(t) || p.endpoint.bel_id.contains(t));
        }
        if let Some(th) = through {
            paths.retain(|p| {
                p.cells.iter().any(|c| c.contains(th)) || p.nets.iter().any(|n| n.contains(th))
            });
        }
        for p in paths.into_iter().take(max_paths.max(1)) {
            out.push_str(&format!(
                "\n  path {} ({}) -> {} ({}) slack_ps={} delay_ps={} cells={} nets={}",
                p.startpoint.cell,
                p.startpoint.site_id,
                p.endpoint.cell,
                p.endpoint.site_id,
                p.slack_ps,
                p.delay_ps,
                p.cells.join(","),
                p.nets.join(",")
            ));
        }
    }
    Ok(out)
}

/// UG835 `report_utilization` (works post-synth, post-pack, post-place, and post-route).
pub fn report_utilization_detailed(
    session: &Session,
    dev: &Device,
    hierarchical: bool,
) -> Result<String, String> {
    let design = session
        .design
        .as_ref()
        .ok_or("report_utilization: no design")?;
    let (lutff, luts, ffs, iobs, brams, dsps) = if let Some(pl) = &session.placed {
        let p = &pl.packed;
        let ffs = p.lutffs.iter().filter(|l| !l.ff_cell.is_empty()).count();
        (
            p.lutffs.len(),
            p.lutffs.len(),
            ffs,
            p.iobs.len(),
            p.brams.len(),
            p.macs.len(),
        )
    } else if let Some(p) = &session.packed {
        let ffs = p.lutffs.iter().filter(|l| !l.ff_cell.is_empty()).count();
        (
            p.lutffs.len(),
            p.lutffs.len(),
            ffs,
            p.iobs.len(),
            p.brams.len(),
            p.macs.len(),
        )
    } else {
        let luts = design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count();
        let ffs = design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Hff))
            .count();
        let iobs = design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::IobOut))
            .count();
        let brams = design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Bram18))
            .count();
        let dsps = design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Mac27))
            .count();
        (luts.max(ffs), luts, ffs, iobs, brams, dsps)
    };
    let mut out = format!(
        "report_utilization LUTFF={}/{} LUT={}/{} FF={}/{} IOB={}/{} BRAM={}/{} DSP={}/{}",
        lutff,
        dev.lut6_count(),
        luts,
        dev.lut6_count(),
        ffs,
        dev.lut6_count(),
        iobs,
        dev.iob_sites().count(),
        brams,
        dev.n_bram,
        dsps,
        dev.n_dsp
    );
    if hierarchical {
        out.push_str(&format!(
            "\n  hierarchy top={} instances={} cells={} nets={}",
            resolve(design.name),
            design.instances.len(),
            design.cells.len(),
            design.nets.len()
        ));
    }
    Ok(out)
}

/// UG835 / UG907 `report_power`.
pub fn report_power_detailed(
    session: &Session,
    dev: &Device,
    xdc: &Constraints,
) -> Result<String, String> {
    if session.design.is_none() {
        return Err("report_power: no design".into());
    }
    let clks = effective_clocks(xdc);
    let pwr = report_power(
        dev,
        session.design.as_ref(),
        session.placed.as_ref(),
        &clks,
        &xdc.operating_conditions,
    );
    Ok(pwr.text())
}

/// DRC violation severity (`report_drc`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjDrcSeverity {
    Error,
    Warning,
    Advisory,
}

impl ProjDrcSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Warning",
            Self::Advisory => "Advisory",
        }
    }
}

/// One DRC violation row in [`ProjDrc`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjDrcViolation {
    pub id: String,
    pub severity: ProjDrcSeverity,
    pub objects: String,
    pub message: String,
}

/// Structured DRC report (`report_drc`).
#[derive(Clone, Debug, Default)]
pub struct ProjDrc {
    pub items: Vec<ProjDrcViolation>,
}

impl ProjDrc {
    pub fn ok(&self) -> bool {
        self.items.is_empty()
    }

    pub fn error_count(&self) -> usize {
        self.items
            .iter()
            .filter(|v| v.severity == ProjDrcSeverity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.items
            .iter()
            .filter(|v| v.severity == ProjDrcSeverity::Warning)
            .count()
    }

    fn push(
        &mut self,
        id: &str,
        severity: ProjDrcSeverity,
        objects: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.items.push(ProjDrcViolation {
            id: id.into(),
            severity,
            objects: objects.into(),
            message: message.into(),
        });
    }

    pub fn text(&self) -> String {
        if self.items.is_empty() {
            return "report_drc violations=0 errors=0 warnings=0 ok".into();
        }
        let mut lines = vec![format!(
            "report_drc violations={} errors={} warnings={}",
            self.items.len(),
            self.error_count(),
            self.warning_count()
        )];
        for v in &self.items {
            let obj = if v.objects.is_empty() {
                "-"
            } else {
                v.objects.as_str()
            };
            lines.push(format!(
                "{} {} {} {}",
                v.id,
                v.severity.as_str(),
                obj,
                v.message
            ));
        }
        lines.join("\n")
    }
}

fn parse_iob_xy(spec: &str) -> Option<(u32, u32)> {
    let t = spec.trim();
    let xoff = t.find('X')?;
    let rest = &t[xoff + 1..];
    let yoff = rest.find('Y')?;
    let x = rest[..yoff].parse().ok()?;
    let y = rest[yoff + 1..].parse().ok()?;
    Some((x, y))
}

fn check_placed_drc(design: &Design, placed: &Placed, dev: &Device, d: &mut ProjDrc) {
    if placed.lutff_sites.len() != placed.packed.lutffs.len() {
        d.push(
            "PLACE-1",
            ProjDrcSeverity::Error,
            "",
            format!(
                "placed {} LUTFF sites for {} clusters",
                placed.lutff_sites.len(),
                placed.packed.lutffs.len()
            ),
        );
    }
    if placed.packed.lutffs.len() as u32 > dev.lut6_count() {
        d.push(
            "PLACE-2",
            ProjDrcSeverity::Error,
            "",
            "LUT occupancy exceeds device",
        );
    }
    let mut used = HashSet::new();
    for (site, ble) in &placed.lutff_sites {
        let clb = format!("CLB_X{}Y{}", site.x, site.y);
        if *ble as u32 >= dev.n_ble {
            d.push(
                "PLACE-3",
                ProjDrcSeverity::Error,
                &clb,
                format!("BLE {ble} out of range at {clb}"),
            );
        }
        if !used.insert((site.x, site.y, *ble)) {
            d.push(
                "PLACE-4",
                ProjDrcSeverity::Error,
                &clb,
                format!("overused {clb} BLE{ble}"),
            );
        }
    }
    let has_clk = design.ports.iter().any(|p| p.name == intern("clk"))
        || design
            .nets
            .iter()
            .any(|n| n.endpoints.iter().any(|e| e.pin == intern("CLK")));
    let has_ff = placed.packed.lutffs.iter().any(|l| !l.ff_cell.is_empty());
    if has_ff && !has_clk {
        d.push(
            "CLK-1",
            ProjDrcSeverity::Error,
            "",
            "no clock on registered design",
        );
    }
    let mut by_bank: BTreeMap<u32, Vec<(String, String, u32)>> = BTreeMap::new();
    for p in &design.ports {
        let Some(std) = p.attrs.get("IOSTANDARD") else {
            continue;
        };
        let Some(vcco) = Device::iostandard_vcco_mv(std) else {
            d.push(
                "IOSTD-1",
                ProjDrcSeverity::Error,
                resolve(p.name),
                format!(
                    "IOSTANDARD {std} on {} is not a HAD I/O standard",
                    resolve(p.name)
                ),
            );
            continue;
        };
        let loc = p.attrs.get("LOC").and_then(parse_iob_xy).or_else(|| {
            placed
                .iob_sites
                .iter()
                .zip(placed.packed.iobs.iter())
                .find_map(|(s, i)| {
                    let hit = i.cell.contains(resolve(p.name))
                        || i.from_net == resolve(p.name)
                        || i.loc.as_deref() == Some(p.attrs.get("LOC").unwrap_or(""));
                    hit.then_some((s.x, s.y))
                })
        });
        if let Some((x, y)) = loc {
            if let Some(bank) = dev.iob_bank(x, y) {
                by_bank.entry(bank).or_default().push((
                    resolve(p.name).to_string(),
                    std.to_string(),
                    vcco,
                ));
            }
        }
    }
    for (bank, ports) in by_bank {
        let mut vccos: Vec<u32> = ports.iter().map(|p| p.2).collect();
        vccos.sort_unstable();
        vccos.dedup();
        if vccos.len() > 1 {
            let detail = ports
                .iter()
                .map(|(n, s, v)| format!("{n}={s}/{v}mV"))
                .collect::<Vec<_>>()
                .join(",");
            d.push(
                "IOSTD-2",
                ProjDrcSeverity::Error,
                format!("BANK{bank}"),
                format!("IOSTANDARD VCCO mix on BANK{bank}: {detail}"),
            );
        }
    }
}

fn check_routed_drc(design: &Design, routed: &Routed, dev: &Device, d: &mut ProjDrc) {
    check_placed_drc(design, &routed.placed, dev, d);
    if !routed.placed.packed.iobs.is_empty() && routed.iob_src.is_empty() {
        d.push("ROUTE-1", ProjDrcSeverity::Warning, "", "unrouted IOB");
    }
    if routed.overused > 0 {
        d.push(
            "ROUTE-2",
            ProjDrcSeverity::Warning,
            "",
            format!("PathFinder overused {} tiles", routed.overused),
        );
    }
    let driver_nets: HashSet<&str> = routed
        .placed
        .packed
        .lutffs
        .iter()
        .map(|l| l.q_net.as_str())
        .collect();
    for iob in &routed.placed.packed.iobs {
        if !driver_nets.contains(iob.from_net.as_str()) {
            d.push(
                "ROUTE-3",
                ProjDrcSeverity::Warning,
                &iob.cell,
                format!("IOB {} net {} has no LUT/FF driver", iob.cell, iob.from_net),
            );
        }
    }
}

/// Evaluate DRC on `session` (post-synth, post-place, or post-route).
pub fn evaluate_drc(
    session: &Session,
    dev: &Device,
    _xdc: &Constraints,
) -> Result<ProjDrc, String> {
    let design = session.design.as_ref().ok_or("report_drc: no design")?;
    let mut d = ProjDrc::default();
    if let Some(routed) = &session.routed {
        check_routed_drc(design, routed, dev, &mut d);
    } else if let Some(placed) = &session.placed {
        check_placed_drc(design, placed, dev, &mut d);
    } else {
        // Post-synth checks
        let has_clk = design.ports.iter().any(|p| p.name == intern("clk"))
            || design
                .nets
                .iter()
                .any(|n| n.endpoints.iter().any(|e| e.pin == intern("CLK")));
        let has_ff = design
            .cells
            .iter()
            .any(|c| matches!(c.kind, CellKind::Hff));
        if has_ff && !has_clk {
            d.push(
                "CLK-1",
                ProjDrcSeverity::Error,
                "",
                "no clock on registered design",
            );
        }
    }
    Ok(d)
}

/// UG835 / UG906 `report_drc`.
pub fn report_drc_detailed(
    session: &Session,
    dev: &Device,
    xdc: &Constraints,
) -> Result<String, String> {
    Ok(evaluate_drc(session, dev, xdc)?.text())
}

/// UG835 / UG906 `report_cdc`.
pub fn report_cdc_detailed(session: &Session, xdc: &Constraints) -> Result<String, String> {
    let design = session.design.as_ref().ok_or("report_cdc: no design")?;
    let (tr, clks) = eval_session_timing(session, xdc)?;
    let cdc = report_cdc(&clks, xdc, Some(&tr), Some(design));
    let mut out = format!(
        "report_cdc design={} clocks={} pair_rows={} structural_crossings={}\n",
        resolve(design.name),
        clks.len(),
        cdc.violations.len(),
        cdc.crossings.len()
    );
    out.push_str(&cdc.text());
    Ok(out)
}

/// UG835 / UG949 `report_clock_interaction`.
pub fn report_clock_interaction_detailed(
    session: &Session,
    xdc: &Constraints,
) -> Result<String, String> {
    let design = session
        .design
        .as_ref()
        .ok_or("report_clock_interaction: no design")?;
    let (tr, clks) = eval_session_timing(session, xdc)?;
    let ci = report_clock_interaction(&clks, xdc, Some(&tr));
    let mut out = format!(
        "report_clock_interaction design={} clocks={} pairs={}\n",
        resolve(design.name),
        ci.clocks.len(),
        ci.cells.len()
    );
    out.push_str(&ci.text());
    Ok(out)
}

/// Per-clock-domain utilization row for `report_clock_utilization`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockDomainUtil {
    pub clock_name: String,
    pub source_port: String,
    pub period_ps: u64,
    pub global_buffers: usize,
    pub ff_loads: usize,
    pub bram_loads: usize,
    pub dsp_loads: usize,
    pub clock_regions_spanned: usize,
    pub insertion_delay_ps: i64,
}

/// UG835 `report_clock_utilization` report structure.
#[derive(Clone, Debug, Default)]
pub struct ClockUtilizationReport {
    pub part: String,
    pub total_bufg_available: usize,
    pub total_bufg_used: usize,
    pub domains: Vec<ClockDomainUtil>,
}

impl ClockUtilizationReport {
    pub fn text(&self) -> String {
        let mut out = format!(
            "report_clock_utilization part={} BUFG={}/{} clocks={}\n",
            self.part,
            self.total_bufg_used,
            self.total_bufg_available,
            self.domains.len()
        );
        for d in &self.domains {
            let freq_mhz = if d.period_ps > 0 {
                1_000_000.0 / (d.period_ps as f64)
            } else {
                0.0
            };
            out.push_str(&format!(
                "  clock={} source={} period_ps={} freq_mhz={:.2} bufg={} ff_loads={} bram_loads={} dsp_loads={} regions={} insertion_ps={}\n",
                d.clock_name,
                d.source_port,
                d.period_ps,
                freq_mhz,
                d.global_buffers,
                d.ff_loads,
                d.bram_loads,
                d.dsp_loads,
                d.clock_regions_spanned,
                d.insertion_delay_ps
            ));
        }
        out
    }
}

/// Compute `report_clock_utilization` on `session`.
pub fn evaluate_clock_utilization(
    session: &Session,
    dev: &Device,
    xdc: &Constraints,
) -> Result<ClockUtilizationReport, String> {
    let design = session
        .design
        .as_ref()
        .ok_or("report_clock_utilization: no design")?;
    let clks = effective_clocks(xdc);
    let net_rep = report_clock_networks(&clks, Some(design), session.placed.as_ref());

    let brams = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Bram18))
        .count();
    let dsps = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Mac27))
        .count();
    let regions_spanned = if let Some(pl) = &session.placed {
        let mut quadrants = HashSet::new();
        for (site, _) in &pl.lutff_sites {
            quadrants.insert((site.x / 8, site.y / 8));
        }
        quadrants.len().max(1)
    } else {
        1
    };

    let mut domains = Vec::with_capacity(net_rep.clocks.len());
    let mut total_bufg_used = 0usize;
    for (idx, nw) in net_rep.clocks.iter().enumerate() {
        total_bufg_used += nw.n_buffers;
        domains.push(ClockDomainUtil {
            clock_name: nw.name.clone(),
            source_port: nw.source.clone(),
            period_ps: nw.period_ps,
            global_buffers: nw.n_buffers,
            ff_loads: nw.n_loads,
            bram_loads: if idx == 0 { brams } else { 0 },
            dsp_loads: if idx == 0 { dsps } else { 0 },
            clock_regions_spanned: if nw.n_loads > 0 { regions_spanned } else { 0 },
            insertion_delay_ps: nw.insertion_ps,
        });
    }

    let total_bufg_available = dev.clk_sites().count().max(16);
    Ok(ClockUtilizationReport {
        part: dev.part.clone(),
        total_bufg_available,
        total_bufg_used,
        domains,
    })
}

/// UG835 `report_clock_utilization`.
pub fn report_clock_utilization_detailed(
    session: &Session,
    dev: &Device,
    xdc: &Constraints,
) -> Result<String, String> {
    Ok(evaluate_clock_utilization(session, dev, xdc)?.text())
}

/// One net entry in `report_high_fanout_nets`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighFanoutNet {
    pub net_name: String,
    pub driver_cell: String,
    pub driver_pin: String,
    pub fanout: usize,
    pub lut_loads: usize,
    pub ff_loads: usize,
    pub iob_loads: usize,
    pub is_clock: bool,
}

/// Structured output of `report_high_fanout_nets`.
#[derive(Clone, Debug, Default)]
pub struct HighFanoutReport {
    pub nets: Vec<HighFanoutNet>,
}

impl HighFanoutReport {
    pub fn text(&self) -> String {
        let max_fo = self.nets.first().map(|n| n.fanout).unwrap_or(0);
        let mut out = format!(
            "report_high_fanout_nets count={} max_fanout={}\n",
            self.nets.len(),
            max_fo
        );
        out.push_str(
            "  Net Name                 Driver               Fanout  LUT   FF    IOB   Clock\n",
        );
        for n in &self.nets {
            let drv = if n.driver_cell.is_empty() {
                "<port>".to_string()
            } else {
                format!("{}/{}", n.driver_cell, n.driver_pin)
            };
            out.push_str(&format!(
                "  {:<24} {:<20} {:<7} {:<5} {:<5} {:<5} {}\n",
                n.net_name,
                drv,
                n.fanout,
                n.lut_loads,
                n.ff_loads,
                n.iob_loads,
                if n.is_clock { "yes" } else { "no" }
            ));
        }
        out
    }
}

/// Analyze high-fanout nets in `design`.
pub fn evaluate_high_fanout_nets(
    design: &Design,
    max_nets: usize,
    min_fanout: usize,
    include_clocks: bool,
) -> HighFanoutReport {
    let p_o = intern("O");
    let p_q = intern("Q");
    let p_clk = intern("CLK");

    let mut rows = Vec::new();
    for net in &design.nets {
        let nname = resolve(net.name).to_string();
        let mut driver_cell = String::new();
        let mut driver_pin = String::new();
        let mut lut_loads = 0usize;
        let mut ff_loads = 0usize;
        let mut iob_loads = 0usize;
        let mut is_clock = nname.eq_ignore_ascii_case("clk") || nname.eq_ignore_ascii_case("clock");

        for ep in &net.endpoints {
            let cname = resolve(ep.cell);
            let pname = resolve(ep.pin);
            if ep.pin == p_clk {
                is_clock = true;
            }
            if ep.pin == p_o || ep.pin == p_q {
                driver_cell = cname.to_string();
                driver_pin = pname.to_string();
                continue;
            }
            if let Some(cell) = design.cell(cname) {
                match cell.kind {
                    CellKind::Lut6 { .. } => lut_loads += 1,
                    CellKind::Hff => ff_loads += 1,
                    CellKind::IobOut => iob_loads += 1,
                    _ => {}
                }
            }
        }
        if !include_clocks && is_clock {
            continue;
        }
        let fanout = lut_loads + ff_loads + iob_loads;
        if fanout < min_fanout {
            continue;
        }
        rows.push(HighFanoutNet {
            net_name: nname,
            driver_cell,
            driver_pin,
            fanout,
            lut_loads,
            ff_loads,
            iob_loads,
            is_clock,
        });
    }

    rows.sort_by(|a, b| {
        b.fanout
            .cmp(&a.fanout)
            .then_with(|| a.net_name.cmp(&b.net_name))
    });
    rows.truncate(max_nets.max(1));
    HighFanoutReport { nets: rows }
}

/// UG835 `report_high_fanout_nets`.
pub fn report_high_fanout_nets_detailed(
    session: &Session,
    max_nets: usize,
    min_fanout: usize,
) -> Result<String, String> {
    let design = session
        .design
        .as_ref()
        .ok_or("report_high_fanout_nets: no design")?;
    let mut rep = evaluate_high_fanout_nets(design, max_nets, min_fanout, false);
    if rep.nets.is_empty() {
        rep = evaluate_high_fanout_nets(design, max_nets, min_fanout, true);
    }
    Ok(rep.text())
}

/// Structured UG906 `report_design_analysis` output.
#[derive(Clone, Debug)]
pub struct DesignAnalysisReport {
    pub design_name: String,
    pub part: String,
    pub stage: String,
    pub wns_ps: i64,
    pub tns_ps: i64,
    pub whs_ps: i64,
    pub logic_delay_ps: i64,
    pub route_delay_ps: i64,
    pub logic_levels: usize,
    pub total_cells: usize,
    pub luts: usize,
    pub ffs: usize,
    pub iobs: usize,
    pub brams: usize,
    pub dsps: usize,
    pub max_fanout: usize,
    pub avg_fanout_milli: u32,
    pub rent_exponent_milli: u32,
    pub bounding_box_area: u32,
    pub max_hops: u32,
    pub overused_tiles: u32,
}

impl DesignAnalysisReport {
    pub fn text(&self) -> String {
        let total_delay = (self.logic_delay_ps + self.route_delay_ps).max(1);
        let logic_pct = (self.logic_delay_ps * 100) / total_delay;
        let route_pct = 100 - logic_pct;
        let mut out = String::new();
        out.push_str(&format!(
            "report_design_analysis design={} part={} stage={} WNS_PS={} TNS_PS={} WHS_PS={}\n",
            self.design_name, self.part, self.stage, self.wns_ps, self.tns_ps, self.whs_ps
        ));
        out.push_str(&format!(
            "  Timing Characteristics: logic_levels={} logic_delay_ps={} ({logic_pct}%) route_delay_ps={} ({route_pct}%)\n",
            self.logic_levels, self.logic_delay_ps, self.route_delay_ps
        ));
        out.push_str(&format!(
            "  Complexity & Rent: cells={} LUT={} FF={} IOB={} BRAM={} DSP={} max_fanout={} avg_fanout={:.2} rent_exp={:.2}\n",
            self.total_cells,
            self.luts,
            self.ffs,
            self.iobs,
            self.brams,
            self.dsps,
            self.max_fanout,
            self.avg_fanout_milli as f64 / 1000.0,
            self.rent_exponent_milli as f64 / 1000.0
        ));
        out.push_str(&format!(
            "  Congestion & Placement: bbox_tiles={} max_hops={} overused_tiles={}\n",
            self.bounding_box_area, self.max_hops, self.overused_tiles
        ));
        out
    }
}

/// Evaluate UG906 `report_design_analysis` on `session`.
pub fn evaluate_design_analysis(
    session: &Session,
    dev: &Device,
    xdc: &Constraints,
) -> Result<DesignAnalysisReport, String> {
    let design = session
        .design
        .as_ref()
        .ok_or("report_design_analysis: no design")?;
    let (tr, _clks) = eval_session_timing(session, xdc)?;
    let stage = crate::stage_of(session)
        .map(|s| s.label().to_string())
        .unwrap_or_else(|| "post-synth".into());

    let luts = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    let ffs = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    let iobs = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::IobOut))
        .count();
    let brams = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Bram18))
        .count();
    let dsps = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Mac27))
        .count();

    let logic_levels = if let Some(routed) = &session.routed {
        timing_paths_routed(design, routed, &tr)
            .into_iter()
            .map(|p| p.cells.len())
            .max()
            .unwrap_or(1)
    } else {
        usize::from(luts > 0) + usize::from(ffs > 0)
    };

    let hf = evaluate_high_fanout_nets(design, 16, 0, true);
    let max_fanout = hf.nets.first().map(|n| n.fanout).unwrap_or(0);
    let total_sinks: usize = design
        .nets
        .iter()
        .map(|n| n.endpoints.len().saturating_sub(1))
        .sum();
    let avg_fanout_milli = if design.nets.is_empty() {
        0
    } else {
        ((total_sinks * 1000) / design.nets.len()) as u32
    };
    let rent_exponent_milli = (450 + (avg_fanout_milli / 10).min(300)).min(750);

    let bounding_box_area = if let Some(pl) = &session.placed {
        if pl.lutff_sites.is_empty() {
            0
        } else {
            let min_x = pl.lutff_sites.iter().map(|(s, _)| s.x).min().unwrap_or(0);
            let max_x = pl.lutff_sites.iter().map(|(s, _)| s.x).max().unwrap_or(0);
            let min_y = pl.lutff_sites.iter().map(|(s, _)| s.y).min().unwrap_or(0);
            let max_y = pl.lutff_sites.iter().map(|(s, _)| s.y).max().unwrap_or(0);
            (max_x - min_x + 1) * (max_y - min_y + 1)
        }
    } else {
        0
    };

    let max_hops = session
        .routed
        .as_ref()
        .and_then(|r| r.iob_src.iter().map(|i| i.hops).max())
        .unwrap_or(0);
    let overused_tiles = session.routed.as_ref().map(|r| r.overused).unwrap_or(0);

    Ok(DesignAnalysisReport {
        design_name: resolve(design.name).to_string(),
        part: dev.part.clone(),
        stage,
        wns_ps: tr.wns_ps,
        tns_ps: tr.tns_ps,
        whs_ps: tr.hold_slack_ps,
        logic_delay_ps: tr.r2r_ps + tr.iob_ps,
        route_delay_ps: tr.route_ps,
        logic_levels,
        total_cells: design.cells.len(),
        luts,
        ffs,
        iobs,
        brams,
        dsps,
        max_fanout,
        avg_fanout_milli,
        rent_exponent_milli,
        bounding_box_area,
        max_hops,
        overused_tiles,
    })
}

/// UG835 / UG906 `report_design_analysis`.
pub fn report_design_analysis_detailed(
    session: &Session,
    dev: &Device,
    xdc: &Constraints,
) -> Result<String, String> {
    Ok(evaluate_design_analysis(session, dev, xdc)?.text())
}
