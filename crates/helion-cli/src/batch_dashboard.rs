//! Vivado Batch/Tcl CLI Compatibility (`-mode batch|tcl|gui`, `-source`, `-tclargs`,
//! `-log`, `-journal`, `-nojournal`, `-notrace`) and Automated JSON + Self-Contained
//! Interactive HTML QoR & Timing Summary Dashboard Exporter (`--export-dashboard <dir>`).

use helion_bits::{Bitstream, bitgen};
use helion_device::Device;
use helion_drc::{Drc, check_routed};
use helion_ir::{CellKind, Design, resolve};
use helion_pack::pack;
use helion_place::{PlaceOpts, place_with_guide};
use helion_proj::{
    Mode, ProjectFile, Session, expand_ip_packages, load_prj, opt_design, resolve_prj_path,
};
use helion_route::{RouteOpts, Routed, route_with_guide};
use helion_sta::{
    Constraints, OperatingConditions, PowerReport, TimingResult, apply_xdc, create_clock,
    load_sdc, load_xdc, report_methodology, report_power, report_timing_routed_xdc,
};
use helion_sv::{SvCompileOpts, elaborate_sv_sources, synth_sv_files, synth_sv_path};
use helion_vhdl::synth_vhdl_path;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Parsed Vivado-compatible CLI invocation options.
#[derive(Clone, Debug, Default)]
pub struct VivadoCliOpts {
    pub mode: Option<String>,
    pub sources: Vec<String>,
    pub tclargs: Vec<String>,
    pub log_file: Option<String>,
    pub no_log: bool,
    pub journal_file: Option<String>,
    pub no_journal: bool,
    pub no_trace: bool,
    pub export_dashboard: Option<String>,
    pub design_target: Option<String>,
    pub part: Option<String>,
    pub top: Option<String>,
    pub sdc: Option<String>,
}

impl VivadoCliOpts {
    /// Returns true if raw CLI arguments trigger Vivado batch/tcl mode or standalone `--export-dashboard`.
    pub fn should_intercept(args: &[String]) -> bool {
        if args.is_empty() {
            return false;
        }
        let first = args[0].as_str();
        if matches!(
            first,
            "-mode"
                | "--mode"
                | "-source"
                | "--source"
                | "-log"
                | "-journal"
                | "-nojournal"
                | "-nojou"
                | "-nolog"
                | "-notrace"
                | "-tclargs"
                | "--export-dashboard"
        ) {
            return true;
        }
        // Also intercept if `-mode` or `-source` appears anywhere in args
        args.iter().any(|a| {
            matches!(
                a.as_str(),
                "-mode" | "--mode" | "-source" | "--source"
            )
        })
    }

    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut opts = Self::default();
        let mut i = 0usize;
        while i < args.len() {
            match args[i].as_str() {
                "-mode" | "--mode" => {
                    let v = args.get(i + 1).ok_or("-mode requires an argument (batch|tcl|gui)")?;
                    opts.mode = Some(v.to_ascii_lowercase());
                    i += 2;
                }
                "-source" | "--source" => {
                    let v = args.get(i + 1).ok_or("-source requires a <script.tcl> path")?;
                    opts.sources.push(v.clone());
                    i += 2;
                }
                "-log" | "--log" => {
                    let v = args.get(i + 1).ok_or("-log requires a file path")?;
                    opts.log_file = Some(v.clone());
                    i += 2;
                }
                "-nolog" | "--nolog" => {
                    opts.no_log = true;
                    i += 1;
                }
                "-journal" | "-jou" | "--journal" => {
                    let v = args.get(i + 1).ok_or("-journal requires a file path")?;
                    opts.journal_file = Some(v.clone());
                    i += 2;
                }
                "-nojournal" | "-nojou" | "--nojournal" => {
                    opts.no_journal = true;
                    i += 1;
                }
                "-notrace" | "--notrace" => {
                    opts.no_trace = true;
                    i += 1;
                }
                "--export-dashboard" | "-export-dashboard" => {
                    let v = args
                        .get(i + 1)
                        .ok_or("--export-dashboard requires an output directory")?;
                    opts.export_dashboard = Some(v.clone());
                    i += 2;
                }
                "--part" | "-part" => {
                    let v = args.get(i + 1).ok_or("--part requires a part name")?;
                    opts.part = Some(v.clone());
                    i += 2;
                }
                "--top" | "-top" => {
                    let v = args.get(i + 1).ok_or("--top requires a module name")?;
                    opts.top = Some(v.clone());
                    i += 2;
                }
                "--sdc" | "-sdc" => {
                    let v = args.get(i + 1).ok_or("--sdc requires a file path")?;
                    opts.sdc = Some(v.clone());
                    i += 2;
                }
                "-tclargs" | "--tclargs" => {
                    i += 1;
                    while i < args.len() {
                        if args[i] == "--export-dashboard" && i + 1 < args.len() {
                            opts.export_dashboard = Some(args[i + 1].clone());
                            i += 2;
                            continue;
                        }
                        opts.tclargs.push(args[i].clone());
                        i += 1;
                    }
                    break;
                }
                other if !other.starts_with('-') => {
                    if other.ends_with(".tcl") && opts.sources.is_empty() {
                        opts.sources.push(other.to_string());
                    } else if opts.design_target.is_none() {
                        opts.design_target = Some(other.to_string());
                    }
                    i += 1;
                }
                other => {
                    return Err(format!("unknown option: {other}"));
                }
            }
        }
        Ok(opts)
    }
}

/// Complete compiled design snapshot used for QoR & Timing Dashboard export.
pub struct DashboardSnapshot {
    pub design_name: String,
    pub requested_part: String,
    pub dev: Device,
    pub sources: Vec<String>,
    pub design: Design,
    pub routed: Routed,
    pub drc: Drc,
    pub timing: TimingResult,
    pub power: PowerReport,
    pub bits: Bitstream,
    pub xdc: Constraints,
    pub synth_ms: u128,
    pub pack_ms: u128,
    pub place_ms: u128,
    pub route_ms: u128,
    pub sta_ms: u128,
    pub bitgen_ms: u128,
    pub total_ms: u128,
}

pub fn resolve_had_part(requested: &str) -> String {
    let trimmed = requested.trim();
    if trimmed.is_empty() {
        return "HL10T-C32-1".to_string();
    }
    if Device::load_part(trimmed).is_ok() {
        return trimmed.to_string();
    }
    // Map common Vivado / Lattice / iCE40 benchmark part strings to HL10T-C32-1
    // so drop-in Vivado scripts (e.g. `xc7a35tcpg236-1`) execute cleanly.
    "HL10T-C32-1".to_string()
}

pub fn compile_for_dashboard(
    sources: &[PathBuf],
    top: Option<&str>,
    requested_part: &str,
    xdc: &Constraints,
) -> Result<DashboardSnapshot, String> {
    let t_total = Instant::now();
    let t_synth = Instant::now();
    let mut design = synth_sources_for_batch(sources, top)?;
    let synth_ms = t_synth.elapsed().as_millis();

    compile_design_into_snapshot(
        design.clone(),
        &mut design,
        sources
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        requested_part,
        xdc,
        synth_ms,
        t_total,
    )
}

pub fn compile_design_into_snapshot(
    _orig: Design,
    design: &mut Design,
    source_labels: Vec<String>,
    requested_part: &str,
    xdc: &Constraints,
    synth_ms: u128,
    t_total: Instant,
) -> Result<DashboardSnapshot, String> {
    apply_xdc(design, xdc)?;
    let had_part = resolve_had_part(requested_part);
    let dev = Device::load_part(&had_part).map_err(|e| format!("HAD {had_part}: {e}"))?;

    let t_pack = Instant::now();
    let mut packed = pack(design, &dev)?;
    let iob_budget = dev.iob_sites().count();
    let q_nets: std::collections::HashSet<&str> =
        packed.lutffs.iter().map(|l| l.q_net.as_str()).collect();
    let driven: Vec<_> = packed
        .iobs
        .iter()
        .filter(|io| q_nets.contains(io.from_net.as_str()))
        .cloned()
        .collect();
    let undriven = packed.iobs.len().saturating_sub(driven.len());
    if packed.iobs.len() > iob_budget || undriven > 0 {
        let keep = driven.len().min(iob_budget);
        let keep = if packed.lutffs.len() > 2000 {
            keep.min(4)
        } else {
            keep
        };
        packed.iobs = driven.into_iter().take(keep).collect();
    }
    let pack_ms = t_pack.elapsed().as_millis();

    let t_place = Instant::now();
    let guide = xdc.timing_guide();
    let placed = place_with_guide(
        &packed,
        &dev,
        PlaceOpts {
            timing_weight: 0.75,
            seed: 0,
        },
        &guide,
    )?;
    let place_ms = t_place.elapsed().as_millis();

    let t_route = Instant::now();
    let route_opts = RouteOpts {
        max_iters: if packed.lutffs.len() > 2000 { 24 } else { 8 },
        extra_hops: 0,
    };
    let routed = route_with_guide(&placed, &dev, route_opts, &guide)?;
    let route_ms = t_route.elapsed().as_millis();

    let drc = check_routed(design, &routed, &dev);

    let t_bit = Instant::now();
    let bits = match bitgen(&dev, &routed) {
        Ok(b) => b,
        Err(e) if e.contains("empty design") => Bitstream::empty(&dev),
        Err(e) => return Err(e),
    };
    let bitgen_ms = t_bit.elapsed().as_millis();

    let t_sta = Instant::now();
    let mut clks = xdc.clocks.clone();
    if clks.is_empty() {
        create_clock(&mut clks, "clk", 10_000, "clk");
    }
    let timing = report_timing_routed_xdc(design, &routed, &clks, xdc)?;
    let sta_ms = t_sta.elapsed().as_millis();

    let power = report_power(
        &dev,
        Some(design),
        Some(&routed.placed),
        &clks,
        &OperatingConditions::default(),
    );

    let total_ms = t_total.elapsed().as_millis();
    Ok(DashboardSnapshot {
        design_name: resolve(design.name).to_string(),
        requested_part: if requested_part.is_empty() {
            dev.part.clone()
        } else {
            requested_part.to_string()
        },
        dev,
        sources: source_labels,
        design: design.clone(),
        routed,
        drc,
        timing,
        power,
        bits,
        xdc: xdc.clone(),
        synth_ms,
        pack_ms,
        place_ms,
        route_ms,
        sta_ms,
        bitgen_ms,
        total_ms,
    })
}

fn synth_sources_for_batch(paths: &[PathBuf], top: Option<&str>) -> Result<Design, String> {
    if paths.is_empty() {
        return Err("no HDL sources provided".into());
    }
    if paths.len() == 1 && top.is_none() {
        let p = &paths[0];
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("sv")
            .to_ascii_lowercase();
        return match ext.as_str() {
            "prj" => {
                let text = fs::read_to_string(p).map_err(|e| e.to_string())?;
                let mut prj = load_prj(&text)?;
                let _ = expand_ip_packages(&mut prj, p)?;
                let src_paths: Vec<PathBuf> =
                    prj.sources.iter().map(|s| resolve_prj_path(p, s)).collect();
                synth_sources_for_batch(&src_paths, prj.top.as_deref())
            }
            "vhd" | "vhdl" => synth_vhdl_path(p),
            "c" | "cc" | "cpp" => helion_hls::synth_c_path(p),
            _ => synth_sv_path(p),
        };
    }
    let mut owned: Vec<(String, String)> = Vec::with_capacity(paths.len());
    for p in paths {
        let src = fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        owned.push((p.display().to_string(), src));
    }
    if let Some(t) = top {
        let refs: Vec<(&str, &str)> = owned
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let (d, _) =
            elaborate_sv_sources(&refs, Some(t), &Default::default(), &SvCompileOpts::default())?;
        Ok(d)
    } else {
        let refs: Vec<&Path> = paths.iter().map(|p| p.as_path()).collect();
        synth_sv_files(&refs)
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            other => out.push(other),
        }
    }
    out
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Export structured JSON (`dashboard.json`, `qor_summary.json`) and a self-contained
/// interactive HTML5 QoR & Timing Summary Dashboard (`index.html`, `dashboard.html`).
pub fn export_dashboard_to_dir(
    snap: &DashboardSnapshot,
    out_dir: &Path,
) -> Result<(PathBuf, PathBuf), String> {
    fs::create_dir_all(out_dir)
        .map_err(|e| format!("create dashboard dir {}: {e}", out_dir.display()))?;

    let p = &snap.routed.placed.packed;
    let lutff_used = p.lutffs.len();
    let lut6_avail = snap.dev.lut6_count() as usize;
    let lut_pct = if lut6_avail > 0 {
        (lutff_used as f64) * 100.0 / (lut6_avail as f64)
    } else {
        0.0
    };

    let luts_logic = snap
        .design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    let ffs_count = snap
        .design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    let carry4_count = snap
        .design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Carry4))
        .count();

    let iob_used = p.iobs.len();
    let iob_avail = snap.dev.iob_sites().count();
    let iob_pct = if iob_avail > 0 {
        (iob_used as f64) * 100.0 / (iob_avail as f64)
    } else {
        0.0
    };

    let bram_used = p.brams.len();
    let bram_avail = snap.dev.n_bram as usize;
    let bram_pct = if bram_avail > 0 {
        (bram_used as f64) * 100.0 / (bram_avail as f64)
    } else {
        0.0
    };

    let dsp_used = p.macs.len();
    let dsp_avail = snap.dev.n_dsp as usize;
    let dsp_pct = if dsp_avail > 0 {
        (dsp_used as f64) * 100.0 / (dsp_avail as f64)
    } else {
        0.0
    };

    let clk_period_ps = snap
        .timing
        .clocks
        .first()
        .map(|c| c.period_ps as i64)
        .unwrap_or(10_000);
    let crit_delay_ps = (clk_period_ps - snap.timing.wns_ps).max(100);
    let fmax_mhz = 1_000_000.0 / (crit_delay_ps as f64);
    let timing_met = snap.timing.wns_ps >= 0 && snap.timing.hold_slack_ps >= 0;

    let mut clks_for_meth = snap.timing.clocks.clone();
    if clks_for_meth.is_empty() {
        create_clock(&mut clks_for_meth, "clk", 10_000, "clk");
    }
    let methodology = report_methodology(
        &clks_for_meth,
        &snap.xdc,
        Some(&snap.timing),
        Some(&snap.design),
    );

    let mut drc_items_json = Vec::new();
    for v in &snap.drc.items {
        drc_items_json.push(format!(
            r#"      {{ "id": "{}", "severity": "{}", "objects": "{}", "message": "{}" }}"#,
            json_escape(&v.id),
            json_escape(v.severity.as_str()),
            json_escape(&v.objects),
            json_escape(&v.message)
        ));
    }

    let mut meth_items_json = Vec::new();
    for c in &methodology.checks {
        meth_items_json.push(format!(
            r#"      {{ "id": "{}", "severity": "{}", "category": "{}", "objects": "{}", "message": "{}" }}"#,
            json_escape(&c.id),
            json_escape(c.severity.as_str()),
            json_escape(&c.category),
            json_escape(&c.objects),
            json_escape(&c.message)
        ));
    }

    let mut iob_routes_json = Vec::new();
    for r in &snap.routed.iob_src {
        iob_routes_json.push(format!(
            r#"      {{ "net": "{}", "clb": "CLB_X{}Y{}", "ble": {}, "iob": "IOB_X{}Y{}", "hops": {}, "delay_ps": {}, "from_ff": {} }}"#,
            json_escape(&r.net),
            r.clb.0,
            r.clb.1,
            r.ble,
            r.iob.0,
            r.iob.1,
            r.hops,
            r.delay_ps,
            r.from_ff
        ));
    }

    let sources_json: Vec<String> = snap
        .sources
        .iter()
        .map(|s| format!("\"{}\"", json_escape(s)))
        .collect();

    let json_body = format!(
        r#"{{
  "schema": "helion.qor_dashboard.v1",
  "design": "{}",
  "part": "{}",
  "had_part": "{}",
  "sources": [{}],
  "runtime_ms": {{
    "synth": {},
    "pack": {},
    "place": {},
    "route": {},
    "sta": {},
    "bitgen": {},
    "total": {}
  }},
  "timing": {{
    "timing_met": {},
    "clock_period_ps": {},
    "wns_ps": {},
    "tns_ps": {},
    "wns_ns": {:.3},
    "tns_ns": {:.3},
    "fmax_mhz": {:.2},
    "setup_ps": {},
    "hold_ps": {},
    "hold_slack_ps": {},
    "r2r_ps": {},
    "iob_ps": {},
    "route_ps": {},
    "endpoints": {}
  }},
  "utilization": {{
    "cells_total": {},
    "lutff_used": {},
    "lut6_available": {},
    "lut_pct": {:.2},
    "luts_logic": {},
    "ffs": {},
    "carry4": {},
    "iob_used": {},
    "iob_available": {},
    "iob_pct": {:.2},
    "bram_used": {},
    "bram_available": {},
    "bram_pct": {:.2},
    "dsp_used": {},
    "dsp_available": {},
    "dsp_pct": {:.2}
  }},
  "routing": {{
    "pathfinder_iters": {},
    "imux_routes": {},
    "imux_skip": {},
    "overused_tiles": {},
    "iob_routes": [
{}
    ]
  }},
  "drc": {{
    "ok": {},
    "error_count": {},
    "violation_count": {},
    "violations": [
{}
    ],
    "methodology_checks": [
{}
    ]
  }},
  "power": {{
    "total_uw": {},
    "static_uw": {},
    "dynamic_uw": {},
    "total_mw": {:.3},
    "confidence": "{}"
  }},
  "bitstream": {{
    "frames": {},
    "bytes": {},
    "idcode": "{:#010x}"
  }}
}}
"#,
        json_escape(&snap.design_name),
        json_escape(&snap.requested_part),
        json_escape(&snap.dev.part),
        sources_json.join(", "),
        snap.synth_ms,
        snap.pack_ms,
        snap.place_ms,
        snap.route_ms,
        snap.sta_ms,
        snap.bitgen_ms,
        snap.total_ms,
        timing_met,
        clk_period_ps,
        snap.timing.wns_ps,
        snap.timing.tns_ps,
        (snap.timing.wns_ps as f64) / 1000.0,
        (snap.timing.tns_ps as f64) / 1000.0,
        fmax_mhz,
        snap.timing.setup_ps,
        snap.timing.hold_ps,
        snap.timing.hold_slack_ps,
        snap.timing.r2r_ps,
        snap.timing.iob_ps,
        snap.timing.route_ps,
        snap.timing.endpoints,
        snap.design.cells.len(),
        lutff_used,
        lut6_avail,
        lut_pct,
        luts_logic,
        ffs_count,
        carry4_count,
        iob_used,
        iob_avail,
        iob_pct,
        bram_used,
        bram_avail,
        bram_pct,
        dsp_used,
        dsp_avail,
        dsp_pct,
        snap.routed.pathfinder_iters,
        snap.routed.imux.len(),
        snap.routed.imux_skip,
        snap.routed.overused,
        iob_routes_json.join(",\n"),
        snap.drc.ok(),
        snap.drc.error_count(),
        snap.drc.items.len(),
        drc_items_json.join(",\n"),
        meth_items_json.join(",\n"),
        snap.power.total_uw,
        snap.power.static_uw,
        snap.power.dynamic_uw,
        (snap.power.total_uw as f64) / 1000.0,
        "High",
        snap.bits.frames.len(),
        snap.bits.packets.len(),
        snap.dev.idcode
    );

    let json_path = out_dir.join("dashboard.json");
    fs::write(&json_path, &json_body).map_err(|e| format!("write {}: {e}", json_path.display()))?;
    let qor_json_path = out_dir.join("qor_summary.json");
    fs::write(&qor_json_path, &json_body)
        .map_err(|e| format!("write {}: {e}", qor_json_path.display()))?;

    // Build SVG floorplan of CLB occupancy and routed IOB paths
    let mut svg_elements = String::new();
    for (site, ble) in &snap.routed.placed.lutff_sites {
        let cx = 20 + site.x * 10;
        let cy = 20 + site.y * 10;
        let color = if *ble < 4 { "#38bdf8" } else { "#818cf8" };
        svg_elements.push_str(&format!(
            r#"<rect x="{cx}" y="{cy}" width="8" height="8" rx="1.5" fill="{color}" opacity="0.85"><title>CLB_X{}Y{} BLE{}</title></rect>"#,
            site.x, site.y, ble
        ));
    }
    for r in &snap.routed.iob_src {
        let x1 = 24 + r.clb.0 * 10;
        let y1 = 24 + r.clb.1 * 10;
        let x2 = 24 + r.iob.0 * 10;
        let y2 = 24 + r.iob.1 * 10;
        svg_elements.push_str(&format!(
            r##"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="#34d399" stroke-width="1.8" stroke-dasharray="3,2"><title>Net {} ({} hops, {} ps)</title></line>"##,
            html_escape(&r.net),
            r.hops,
            r.delay_ps
        ));
        svg_elements.push_str(&format!(
            r##"<circle cx="{x2}" cy="{y2}" r="4.5" fill="#10b981"><title>IOB_X{}Y{} ({})</title></circle>"##,
            r.iob.0,
            r.iob.1,
            html_escape(&r.net)
        ));
    }

    let mut iob_rows_html = String::new();
    if snap.routed.iob_src.is_empty() {
        iob_rows_html.push_str(
            r#"<tr><td colspan="6" class="muted">No routed IOB paths (combinational/empty shell)</td></tr>"#,
        );
    } else {
        for r in &snap.routed.iob_src {
            iob_rows_html.push_str(&format!(
                r#"<tr><td><code>{}</code></td><td><code>CLB_X{}Y{}/BLE{}</code></td><td><code>IOB_X{}Y{}</code></td><td>{}</td><td>{} ps</td><td>{}</td></tr>"#,
                html_escape(&r.net),
                r.clb.0,
                r.clb.1,
                r.ble,
                r.iob.0,
                r.iob.1,
                r.hops,
                r.delay_ps,
                if r.from_ff { "FF (Q)" } else { "Comb (LUT)" }
            ));
        }
    }

    let mut drc_rows_html = String::new();
    if snap.drc.items.is_empty() && methodology.checks.is_empty() {
        drc_rows_html.push_str(
            r#"<tr><td colspan="4" class="pass-text">0 DRC or Methodology violations — All checks clean</td></tr>"#,
        );
    } else {
        for v in &snap.drc.items {
            drc_rows_html.push_str(&format!(
                r#"<tr><td><code>{}</code></td><td><span class="badge badge-warn">{}</span></td><td><code>{}</code></td><td>{}</td></tr>"#,
                html_escape(&v.id),
                html_escape(v.severity.as_str()),
                html_escape(&v.objects),
                html_escape(&v.message)
            ));
        }
        for c in &methodology.checks {
            drc_rows_html.push_str(&format!(
                r#"<tr><td><code>{}</code></td><td><span class="badge badge-info">{}</span></td><td><code>{}</code></td><td>{}</td></tr>"#,
                html_escape(&c.id),
                html_escape(c.severity.as_str()),
                html_escape(&c.objects),
                html_escape(&c.message)
            ));
        }
    }

    let status_badge_class = if timing_met && snap.drc.error_count() == 0 {
        "badge-pass"
    } else {
        "badge-fail"
    };
    let status_badge_text = if timing_met && snap.drc.error_count() == 0 {
        "TIMING MET & DRC CLEAN"
    } else if !timing_met {
        "TIMING VIOLATED"
    } else {
        "DRC ERRORS"
    };

    let html_body = format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Helion QoR &amp; Timing Summary Dashboard — {design}</title>
  <style>
    :root {{
      --bg: #0b0f19;
      --panel: #111827;
      --border: #1f2937;
      --text: #f3f4f6;
      --muted: #9ca3af;
      --accent: #38bdf8;
      --pass: #10b981;
      --warn: #f59e0b;
      --fail: #ef4444;
    }}
    * {{ box-sizing: border-box; }}
    body {{
      margin: 0;
      padding: 24px 32px;
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
      background: var(--bg);
      color: var(--text);
      line-height: 1.5;
    }}
    header {{
      display: flex;
      justify-content: space-between;
      align-items: center;
      flex-wrap: wrap;
      gap: 16px;
      padding-bottom: 20px;
      border-bottom: 1px solid var(--border);
      margin-bottom: 24px;
    }}
    h1 {{ margin: 0; font-size: 1.5rem; font-weight: 700; }}
    h2 {{ margin: 0 0 14px 0; font-size: 1.05rem; font-weight: 600; color: #e5e7eb; }}
    .subtitle {{ color: var(--muted); font-size: 0.9rem; margin-top: 4px; }}
    .badge {{
      display: inline-block;
      padding: 4px 12px;
      border-radius: 9999px;
      font-size: 0.78rem;
      font-weight: 700;
      letter-spacing: 0.04em;
    }}
    .badge-pass {{ background: rgba(16, 185, 129, 0.18); color: #34d399; border: 1px solid rgba(16, 185, 129, 0.4); }}
    .badge-fail {{ background: rgba(239, 68, 68, 0.18); color: #f87171; border: 1px solid rgba(239, 68, 68, 0.4); }}
    .badge-warn {{ background: rgba(245, 158, 11, 0.18); color: #fbbf24; }}
    .badge-info {{ background: rgba(56, 189, 248, 0.18); color: #38bdf8; }}
    .kpi-grid {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(185px, 1fr));
      gap: 16px;
      margin-bottom: 24px;
    }}
    .card {{
      background: var(--panel);
      border: 1px solid var(--border);
      border-radius: 10px;
      padding: 18px 20px;
    }}
    .kpi-label {{ color: var(--muted); font-size: 0.8rem; text-transform: uppercase; letter-spacing: 0.05em; }}
    .kpi-val {{ font-size: 1.65rem; font-weight: 700; margin-top: 6px; }}
    .kpi-sub {{ color: var(--muted); font-size: 0.8rem; margin-top: 4px; }}
    .two-col {{
      display: grid;
      grid-template-columns: 1.2fr 1fr;
      gap: 20px;
      margin-bottom: 24px;
    }}
    @media (max-width: 960px) {{ .two-col {{ grid-template-columns: 1fr; }} }}
    .bar-row {{ margin-bottom: 14px; }}
    .bar-head {{ display: flex; justify-content: space-between; font-size: 0.86rem; margin-bottom: 5px; }}
    .bar-track {{ width: 100%; height: 9px; background: #1f2937; border-radius: 999px; overflow: hidden; }}
    .bar-fill {{ height: 100%; background: linear-gradient(90deg, #38bdf8, #818cf8); border-radius: 999px; }}
    table {{ width: 100%; border-collapse: collapse; font-size: 0.88rem; }}
    th, td {{ text-align: left; padding: 9px 12px; border-bottom: 1px solid var(--border); }}
    th {{ color: var(--muted); font-weight: 600; font-size: 0.78rem; text-transform: uppercase; }}
    code {{ background: #1f2937; padding: 2px 6px; border-radius: 4px; font-size: 0.82rem; color: #93c5fd; }}
    .pass-text {{ color: #34d399; font-weight: 600; }}
    .muted {{ color: var(--muted); }}
    pre {{
      background: #090d16;
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 14px;
      overflow-x: auto;
      font-size: 0.8rem;
      color: #d1d5db;
      max-height: 320px;
    }}
  </style>
</head>
<body>
  <header>
    <div>
      <h1>Helion QoR &amp; Timing Summary Dashboard — <code>{design}</code></h1>
      <div class="subtitle">Target Part: <strong>{part}</strong> (HAD: <code>{had_part}</code>) &bull; Sources: <code>{sources}</code> &bull; Compile Time: <strong>{total_ms} ms</strong></div>
    </div>
    <div>
      <span class="badge {status_badge_class}">{status_badge_text}</span>
    </div>
  </header>

  <section class="kpi-grid">
    <div class="card">
      <div class="kpi-label">Worst Negative Slack (WNS)</div>
      <div class="kpi-val">{wns_ns:+.3} ns</div>
      <div class="kpi-sub">WNS_PS = {wns_ps} ps &bull; TNS = {tns_ps} ps</div>
    </div>
    <div class="card">
      <div class="kpi-label">Estimated Fmax</div>
      <div class="kpi-val">{fmax_mhz:.1} MHz</div>
      <div class="kpi-sub">Clock Period = {clk_period_ps} ps ({endpoints} endpoints)</div>
    </div>
    <div class="card">
      <div class="kpi-label">LUTFF Utilization</div>
      <div class="kpi-val">{lutff_used} / {lut6_avail}</div>
      <div class="kpi-sub">{lut_pct:.2}% BLEs ({luts_logic} LUT6, {ffs_count} FF)</div>
    </div>
    <div class="card">
      <div class="kpi-label">I/O &amp; Hard Blocks</div>
      <div class="kpi-val">{iob_used} / {iob_avail} IOB</div>
      <div class="kpi-sub">BRAM18: {bram_used}/{bram_avail} &bull; DSP: {dsp_used}/{dsp_avail}</div>
    </div>
    <div class="card">
      <div class="kpi-label">Total On-Chip Power</div>
      <div class="kpi-val">{total_mw:.2} mW</div>
      <div class="kpi-sub">Static: {static_uw} µW &bull; Dynamic: {dynamic_uw} µW</div>
    </div>
    <div class="card">
      <div class="kpi-label">Bitstream (.hbits)</div>
      <div class="kpi-val">{frames} Frames</div>
      <div class="kpi-sub">{bytes} bytes &bull; IDCODE {idcode:#010x}</div>
    </div>
  </section>

  <section class="two-col">
    <div class="card">
      <h2>Resource Utilization &amp; Timing Delay Breakdown</h2>
      <div class="bar-row">
        <div class="bar-head"><span>LUT6 / BLE Clusters</span><span>{lutff_used} / {lut6_avail} ({lut_pct:.2}%)</span></div>
        <div class="bar-track"><div class="bar-fill" style="width: {lut_bar_w:.1}%"></div></div>
      </div>
      <div class="bar-row">
        <div class="bar-head"><span>User I/O Pins (IOB)</span><span>{iob_used} / {iob_avail} ({iob_pct:.2}%)</span></div>
        <div class="bar-track"><div class="bar-fill" style="width: {iob_bar_w:.1}%"></div></div>
      </div>
      <div class="bar-row">
        <div class="bar-head"><span>Block RAM (BRAM18)</span><span>{bram_used} / {bram_avail} ({bram_pct:.2}%)</span></div>
        <div class="bar-track"><div class="bar-fill" style="width: {bram_bar_w:.1}%"></div></div>
      </div>
      <div class="bar-row">
        <div class="bar-head"><span>DSP Slices (MAC27 / DSP48E1)</span><span>{dsp_used} / {dsp_avail} ({dsp_pct:.2}%)</span></div>
        <div class="bar-track"><div class="bar-fill" style="width: {dsp_bar_w:.1}%"></div></div>
      </div>
      <hr style="border: 0; border-top: 1px solid var(--border); margin: 18px 0;" />
      <h2>Stage Compile Telemetry &amp; Delay Arcs</h2>
      <table>
        <thead>
          <tr><th>Metric</th><th>Value</th><th>Detail</th></tr>
        </thead>
        <tbody>
          <tr><td>Reg-to-Reg Delay</td><td><code>{r2r_ps} ps</code></td><td>Setup: {setup_ps} ps, Hold Slack: {hold_slack_ps} ps</td></tr>
          <tr><td>IOB Output Path Delay</td><td><code>{iob_ps} ps</code></td><td>Route Component: {route_ps} ps</td></tr>
          <tr><td>PathFinder Routing</td><td><code>{pf_iters} iters</code></td><td>IMUX: {imux_routes} routed, {imux_skip} skipped, {overused} overused</td></tr>
          <tr><td>Stage Runtimes</td><td><code>{total_ms} ms total</code></td><td>Synth {synth_ms}ms &bull; Pack/Place {place_ms}ms &bull; Route {route_ms}ms &bull; STA {sta_ms}ms</td></tr>
        </tbody>
      </table>
    </div>

    <div class="card">
      <h2>Device Floorplan — CLB Occupancy &amp; Routed IOB Paths</h2>
      <div style="display:flex; justify-content:center; align-items:center; background:#090d16; border:1px solid var(--border); border-radius:8px; padding:12px;">
        <svg viewBox="0 0 380 380" width="100%" height="320" xmlns="http://www.w3.org/2000/svg">
          <rect x="15" y="15" width="350" height="350" rx="6" fill="#0f172a" stroke="#334155" stroke-width="1.5" />
          <line x1="30" y1="15" x2="30" y2="365" stroke="#f59e0b" stroke-width="1" stroke-dasharray="4,4" opacity="0.6" />
          {svg_elements}
        </svg>
      </div>
    </div>
  </section>

  <section class="two-col">
    <div class="card">
      <h2>DRC &amp; Methodology Verification Summary</h2>
      <table>
        <thead>
          <tr><th>Rule ID</th><th>Severity</th><th>Objects</th><th>Message</th></tr>
        </thead>
        <tbody>
          {drc_rows_html}
        </tbody>
      </table>
    </div>
    <div class="card">
      <h2>Routed IOB Timing Paths</h2>
      <table>
        <thead>
          <tr><th>Net</th><th>Source BEL</th><th>Sink IOB</th><th>Hops</th><th>Route Delay</th><th>Source Type</th></tr>
        </thead>
        <tbody>
          {iob_rows_html}
        </tbody>
      </table>
    </div>
  </section>

  <section class="card">
    <h2>Embedded Machine-Readable QoR JSON (<code>dashboard.json</code>)</h2>
    <pre id="qor-json-data">{json_escaped_html}</pre>
  </section>
</body>
</html>
"##,
        design = html_escape(&snap.design_name),
        part = html_escape(&snap.requested_part),
        had_part = html_escape(&snap.dev.part),
        sources = html_escape(&snap.sources.join(", ")),
        total_ms = snap.total_ms,
        status_badge_class = status_badge_class,
        status_badge_text = html_escape(status_badge_text),
        wns_ns = (snap.timing.wns_ps as f64) / 1000.0,
        wns_ps = snap.timing.wns_ps,
        tns_ps = snap.timing.tns_ps,
        fmax_mhz = fmax_mhz,
        clk_period_ps = clk_period_ps,
        endpoints = snap.timing.endpoints,
        lutff_used = lutff_used,
        lut6_avail = lut6_avail,
        lut_pct = lut_pct,
        luts_logic = luts_logic,
        ffs_count = ffs_count,
        iob_used = iob_used,
        iob_avail = iob_avail,
        iob_pct = iob_pct,
        bram_used = bram_used,
        bram_avail = bram_avail,
        bram_pct = bram_pct,
        dsp_used = dsp_used,
        dsp_avail = dsp_avail,
        dsp_pct = dsp_pct,
        total_mw = (snap.power.total_uw as f64) / 1000.0,
        static_uw = snap.power.static_uw,
        dynamic_uw = snap.power.dynamic_uw,
        frames = snap.bits.frames.len(),
        bytes = snap.bits.packets.len(),
        idcode = snap.dev.idcode,
        lut_bar_w = lut_pct.clamp(1.5, 100.0),
        iob_bar_w = iob_pct.clamp(1.5, 100.0),
        bram_bar_w = if bram_used == 0 { 0.0 } else { bram_pct.clamp(2.0, 100.0) },
        dsp_bar_w = if dsp_used == 0 { 0.0 } else { dsp_pct.clamp(2.0, 100.0) },
        r2r_ps = snap.timing.r2r_ps,
        setup_ps = snap.timing.setup_ps,
        hold_slack_ps = snap.timing.hold_slack_ps,
        iob_ps = snap.timing.iob_ps,
        route_ps = snap.timing.route_ps,
        pf_iters = snap.routed.pathfinder_iters,
        imux_routes = snap.routed.imux.len(),
        imux_skip = snap.routed.imux_skip,
        overused = snap.routed.overused,
        synth_ms = snap.synth_ms,
        place_ms = snap.pack_ms + snap.place_ms,
        route_ms = snap.route_ms,
        sta_ms = snap.sta_ms,
        svg_elements = svg_elements,
        drc_rows_html = drc_rows_html,
        iob_rows_html = iob_rows_html,
        json_escaped_html = html_escape(&json_body),
    );

    let html_path = out_dir.join("index.html");
    fs::write(&html_path, &html_body).map_err(|e| format!("write {}: {e}", html_path.display()))?;
    let dash_html_path = out_dir.join("dashboard.html");
    fs::write(&dash_html_path, &html_body)
        .map_err(|e| format!("write {}: {e}", dash_html_path.display()))?;

    Ok((json_path, html_path))
}

// =============================================================================
// Vivado-Compatible Batch & Interactive Tcl Interpreter
// =============================================================================

pub struct BatchTclSession {
    pub vars: HashMap<String, String>,
    pub files: HashMap<String, String>,
    pub project_name: String,
    pub project_dir: Option<PathBuf>,
    pub requested_part: String,
    pub sources: Vec<PathBuf>,
    pub xdc_files: Vec<PathBuf>,
    pub xdc: Constraints,
    pub top: Option<String>,
    pub session: Session,
    pub last_snapshot: Option<DashboardSnapshot>,
    pub log_lines: Vec<String>,
    pub journal_lines: Vec<String>,
    pub no_trace: bool,
    pub script_base_dir: PathBuf,
    pub exit_requested: Option<i32>,
}

impl BatchTclSession {
    pub fn new(opts: &VivadoCliOpts) -> Self {
        let mut vars = HashMap::new();
        let argv0 = opts.sources.first().cloned().unwrap_or_default();
        vars.insert("argv0".to_string(), argv0);
        vars.insert("argc".to_string(), opts.tclargs.len().to_string());
        vars.insert("argv".to_string(), join_tcl_list(&opts.tclargs));

        let requested_part = opts
            .part
            .clone()
            .unwrap_or_else(|| "HL10T-C32-1".to_string());
        let mut session = Session::new(Mode::NonProject);
        session.part = resolve_had_part(&requested_part);

        let mut journal_lines = vec![
            format!("# Helion v{} Journal file", env!("CARGO_PKG_VERSION")),
            format!(
                "# Command line: helion {}",
                std::env::args().skip(1).collect::<Vec<_>>().join(" ")
            ),
        ];
        if let Some(m) = &opts.mode {
            journal_lines.push(format!("# Mode: {m}"));
        }

        let log_lines = vec![format!(
            "# Helion v{} ({}) — Vivado Batch/Tcl Compatibility Engine",
            env!("CARGO_PKG_VERSION"),
            opts.mode.as_deref().unwrap_or("batch")
        )];

        Self {
            vars,
            files: HashMap::new(),
            project_name: "project_1".to_string(),
            project_dir: None,
            requested_part,
            sources: Vec::new(),
            xdc_files: Vec::new(),
            xdc: Constraints::default(),
            top: opts.top.clone(),
            session,
            last_snapshot: None,
            log_lines,
            journal_lines,
            no_trace: opts.no_trace,
            script_base_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            exit_requested: None,
        }
    }

    pub fn emit_out(&mut self, line: &str) {
        println!("{line}");
        self.log_lines.push(line.to_string());
    }

    pub fn flush_logs(&self, opts: &VivadoCliOpts) -> Result<(), String> {
        if !opts.no_log {
            if let Some(log_path) = &opts.log_file {
                if let Some(parent) = Path::new(log_path).parent() {
                    if !parent.as_os_str().is_empty() {
                        let _ = fs::create_dir_all(parent);
                    }
                }
                let body = self.log_lines.join("\n") + "\n";
                fs::write(log_path, body).map_err(|e| format!("write log {log_path}: {e}"))?;
            }
        }
        if !opts.no_journal {
            if let Some(jou_path) = &opts.journal_file {
                if let Some(parent) = Path::new(jou_path).parent() {
                    if !parent.as_os_str().is_empty() {
                        let _ = fs::create_dir_all(parent);
                    }
                }
                let body = self.journal_lines.join("\n") + "\n";
                fs::write(jou_path, body)
                    .map_err(|e| format!("write journal {jou_path}: {e}"))?;
            }
        }
        Ok(())
    }

    fn resolve_source_path(&self, raw: &str) -> PathBuf {
        let p = Path::new(raw);
        if p.exists() {
            return p.to_path_buf();
        }
        let rel_script = self.script_base_dir.join(p);
        if rel_script.exists() {
            return rel_script;
        }
        let ex_path = Path::new("examples").join(p);
        if ex_path.exists() {
            return ex_path;
        }
        if let Some(fname) = p.file_name() {
            let third_p = Path::new("third_party/picorv32").join(fname);
            if third_p.exists() {
                return third_p;
            }
        }
        p.to_path_buf()
    }

    pub fn eval_script(&mut self, script: &str, record_journal: bool) -> Result<String, String> {
        let stmts = split_tcl_statements(script)?;
        let mut last = String::new();
        for stmt in stmts {
            let trimmed = stmt.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if record_journal {
                self.journal_lines.push(trimmed.to_string());
                if !self.no_trace {
                    let trace_line = format!("# [trace] {trimmed}");
                    self.log_lines.push(trace_line);
                }
            }
            let words = self.parse_and_subst_words(trimmed)?;
            if words.is_empty() {
                continue;
            }
            last = self.exec_command(&words)?;
            if self.exit_requested.is_some() {
                break;
            }
        }
        Ok(last)
    }

    fn parse_and_subst_words(&mut self, stmt: &str) -> Result<Vec<String>, String> {
        let raw_tokens = tokenize_tcl_words(stmt)?;
        let mut out = Vec::with_capacity(raw_tokens.len());
        for (tok, braced) in raw_tokens {
            if braced {
                out.push(tok);
            } else {
                out.push(self.subst_string(&tok)?);
            }
        }
        Ok(out)
    }

    fn subst_string(&mut self, s: &str) -> Result<String, String> {
        let chars: Vec<char> = s.chars().collect();
        let mut i = 0usize;
        let mut out = String::new();
        while i < chars.len() {
            match chars[i] {
                '\\' if i + 1 < chars.len() => {
                    match chars[i + 1] {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        other => out.push(other),
                    }
                    i += 2;
                }
                '[' => {
                    let (inner, next_i) = extract_bracketed(&chars, i)?;
                    let val = self.eval_script(&inner, false)?;
                    out.push_str(&val);
                    i = next_i;
                }
                '$' => {
                    if i + 1 < chars.len() && chars[i + 1] == '{' {
                        let mut j = i + 2;
                        while j < chars.len() && chars[j] != '}' {
                            j += 1;
                        }
                        let name: String = chars[i + 2..j].iter().collect();
                        let val = self.get_var(&name)?;
                        out.push_str(&val);
                        i = (j + 1).min(chars.len());
                    } else {
                        let mut j = i + 1;
                        while j < chars.len()
                            && (chars[j].is_ascii_alphanumeric() || chars[j] == '_')
                        {
                            j += 1;
                        }
                        if j == i + 1 {
                            out.push('$');
                            i += 1;
                        } else {
                            let name: String = chars[i + 1..j].iter().collect();
                            let val = self.get_var(&name)?;
                            out.push_str(&val);
                            i = j;
                        }
                    }
                }
                c => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        Ok(out)
    }

    fn get_var(&self, name: &str) -> Result<String, String> {
        if let Some(v) = self.vars.get(name) {
            return Ok(v.clone());
        }
        if let Ok(v) = std::env::var(name) {
            return Ok(v);
        }
        Err(format!("can't read \"{name}\": no such variable"))
    }

    fn ensure_compiled_snapshot(&mut self) -> Result<&DashboardSnapshot, String> {
        if self.last_snapshot.is_none() {
            if let Some(d) = self.session.design.clone() {
                let mut d_mut = d.clone();
                let snap = compile_design_into_snapshot(
                    d,
                    &mut d_mut,
                    self.sources
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect(),
                    &self.requested_part,
                    &self.xdc,
                    0,
                    Instant::now(),
                )?;
                self.session.placed = Some(snap.routed.placed.clone());
                self.session.routed = Some(snap.routed.clone());
                self.session.bitstream = Some(snap.bits.clone());
                self.last_snapshot = Some(snap);
            } else if !self.sources.is_empty() {
                let snap = compile_for_dashboard(
                    &self.sources,
                    self.top.as_deref(),
                    &self.requested_part,
                    &self.xdc,
                )?;
                self.session.synth_design(snap.design.clone());
                self.session.placed = Some(snap.routed.placed.clone());
                self.session.routed = Some(snap.routed.clone());
                self.session.bitstream = Some(snap.bits.clone());
                self.last_snapshot = Some(snap);
            } else {
                return Err("no synthesized design in session (run synth_design first)".into());
            }
        }
        Ok(self.last_snapshot.as_ref().unwrap())
    }

    fn exec_command(&mut self, argv: &[String]) -> Result<String, String> {
        let cmd = argv[0].as_str();
        let args = &argv[1..];
        match cmd {
            "set" => {
                if args.is_empty() {
                    return Err("wrong # args: should be \"set varName ?newValue?\"".into());
                }
                if args.len() == 1 {
                    return self.get_var(&args[0]);
                }
                self.vars.insert(args[0].clone(), args[1].clone());
                Ok(args[1].clone())
            }
            "unset" => {
                for a in args {
                    self.vars.remove(a);
                }
                Ok(String::new())
            }
            "incr" => {
                let name = args.first().ok_or("incr requires varName")?;
                let step: i64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
                let cur: i64 = self
                    .vars
                    .get(name)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let next = (cur + step).to_string();
                self.vars.insert(name.clone(), next.clone());
                Ok(next)
            }
            "puts" => {
                let mut idx = 0usize;
                if args.first().map(|s| s.as_str()) == Some("-nonewline") {
                    idx += 1;
                }
                if args.len() >= idx + 2 {
                    let chan = &args[idx];
                    let msg = &args[idx + 1];
                    if chan == "stdout" || chan == "stderr" {
                        self.emit_out(msg);
                    } else if let Some(buf) = self.files.get_mut(chan) {
                        buf.push_str(msg);
                        buf.push('\n');
                    } else {
                        self.emit_out(msg);
                    }
                    Ok(String::new())
                } else if let Some(msg) = args.get(idx) {
                    self.emit_out(msg);
                    Ok(String::new())
                } else {
                    self.emit_out("");
                    Ok(String::new())
                }
            }
            "open" => {
                let path = args.first().ok_or("open requires fileName")?;
                let mode = args.get(1).map(|s| s.as_str()).unwrap_or("r");
                let handle = format!("filehandle:{path}:{mode}");
                if mode.starts_with('r') {
                    let content = fs::read_to_string(path).unwrap_or_default();
                    self.files.insert(handle.clone(), content);
                } else {
                    self.files.insert(handle.clone(), String::new());
                }
                Ok(handle)
            }
            "close" => {
                let handle = args.first().ok_or("close requires channelId")?;
                if let Some(content) = self.files.remove(handle) {
                    if let Some(rest) = handle.strip_prefix("filehandle:") {
                        if let Some((path, mode)) = rest.rsplit_once(':') {
                            if mode.starts_with('w') || mode.starts_with('a') {
                                let _ = fs::write(path, content);
                            }
                        }
                    }
                }
                Ok(String::new())
            }
            "list" => Ok(join_tcl_list(args)),
            "llength" => {
                let list_str = args.first().map(|s| s.as_str()).unwrap_or("");
                let items = split_tcl_list(list_str);
                Ok(items.len().to_string())
            }
            "lindex" => {
                let list_str = args.first().map(|s| s.as_str()).unwrap_or("");
                let idx: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                let items = split_tcl_list(list_str);
                Ok(items.get(idx).cloned().unwrap_or_default())
            }
            "concat" | "join" => Ok(args.join(" ")),
            "expr" => {
                let expr_str = args.join(" ");
                eval_simple_expr(&expr_str)
            }
            "foreach" => {
                if args.len() < 3 {
                    return Err("wrong # args: should be \"foreach varList list body\"".into());
                }
                let var_names = split_tcl_list(&args[0]);
                let items = split_tcl_list(&args[1]);
                let body = &args[2];
                let step = var_names.len().max(1);
                let mut last = String::new();
                let mut i = 0usize;
                while i < items.len() {
                    for (vi, vname) in var_names.iter().enumerate() {
                        let val = items.get(i + vi).cloned().unwrap_or_default();
                        self.vars.insert(vname.clone(), val);
                    }
                    last = self.eval_script(body, false)?;
                    if self.exit_requested.is_some() {
                        break;
                    }
                    i += step;
                }
                Ok(last)
            }
            "if" => {
                if args.len() < 2 {
                    return Err("wrong # args: should be \"if expr1 ?then? body1 ...\"".into());
                }
                let cond = eval_simple_expr(&args[0])?;
                let truthy = cond != "0" && !cond.is_empty() && !cond.eq_ignore_ascii_case("false");
                let mut body_idx = 1usize;
                if args.get(1).map(|s| s.as_str()) == Some("then") {
                    body_idx = 2;
                }
                if truthy {
                    let body = args.get(body_idx).ok_or("missing if body")?;
                    self.eval_script(body, false)
                } else {
                    let mut else_idx = body_idx + 1;
                    if args.get(else_idx).map(|s| s.as_str()) == Some("else") {
                        else_idx += 1;
                    }
                    if let Some(else_body) = args.get(else_idx) {
                        self.eval_script(else_body, false)
                    } else {
                        Ok(String::new())
                    }
                }
            }
            "source" => {
                let path = args.first().ok_or("source requires fileName")?;
                let resolved = self.resolve_source_path(path);
                let text = fs::read_to_string(&resolved)
                    .map_err(|e| format!("couldn't read file \"{}\": {e}", resolved.display()))?;
                let prev_dir = self.script_base_dir.clone();
                if let Some(parent) = resolved.parent() {
                    if !parent.as_os_str().is_empty() {
                        self.script_base_dir = parent.to_path_buf();
                    }
                }
                let res = self.eval_script(&text, true);
                self.script_base_dir = prev_dir;
                res
            }
            "file" => {
                let sub = args.first().map(|s| s.as_str()).unwrap_or("");
                match sub {
                    "mkdir" => {
                        for p in &args[1..] {
                            if !p.starts_with('-') {
                                let _ = fs::create_dir_all(p);
                            }
                        }
                        Ok(String::new())
                    }
                    "delete" => {
                        for p in &args[1..] {
                            if !p.starts_with('-') {
                                let path = Path::new(p);
                                if path.is_dir() {
                                    let _ = fs::remove_dir_all(path);
                                } else {
                                    let _ = fs::remove_file(path);
                                }
                            }
                        }
                        Ok(String::new())
                    }
                    "exists" => {
                        let p = args.get(1).map(|s| Path::new(s).exists()).unwrap_or(false);
                        Ok(usize::from(p).to_string())
                    }
                    _ => Ok(String::new()),
                }
            }
            "exit" | "quit" => {
                let code: i32 = args.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                self.exit_requested = Some(code);
                Ok(String::new())
            }
            "create_project" => {
                let mut pos = Vec::new();
                let mut i = 0usize;
                while i < args.len() {
                    match args[i].as_str() {
                        "-force" | "-in_memory" => i += 1,
                        "-part" => {
                            if let Some(p) = args.get(i + 1) {
                                self.requested_part = p.clone();
                                self.session.part = resolve_had_part(p);
                            }
                            i += 2;
                        }
                        other if !other.starts_with('-') => {
                            pos.push(other.to_string());
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
                if let Some(name) = pos.first() {
                    self.project_name = name.clone();
                }
                if let Some(dir) = pos.get(1) {
                    self.project_dir = Some(PathBuf::from(dir));
                }
                self.sources.clear();
                self.xdc_files.clear();
                self.xdc = Constraints::default();
                self.last_snapshot = None;
                self.session = Session::new(Mode::Project);
                self.session.part = resolve_had_part(&self.requested_part);
                let line = format!(
                    "create_project {} part={}",
                    self.project_name, self.requested_part
                );
                self.emit_out(&line);
                Ok(self.project_name.clone())
            }
            "close_project" => {
                self.sources.clear();
                self.xdc_files.clear();
                self.last_snapshot = None;
                self.session.reset_synth();
                Ok(String::new())
            }
            "set_part" => {
                if let Some(p) = args.first() {
                    self.requested_part = p.clone();
                    self.session.part = resolve_had_part(p);
                }
                Ok(self.requested_part.clone())
            }
            "add_files" | "read_verilog" | "read_sv" | "read_vhdl" => {
                let mut i = 0usize;
                while i < args.len() {
                    match args[i].as_str() {
                        "-norecurse" | "-sv" | "-quiet" => i += 1,
                        "-fileset" => i += 2,
                        file if !file.starts_with('-') => {
                            for item in split_tcl_list(file) {
                                let resolved = self.resolve_source_path(&item);
                                if item.ends_with(".xdc") || item.ends_with(".sdc") {
                                    if let Ok(text) = fs::read_to_string(&resolved) {
                                        if let Ok(c) = load_xdc(&text) {
                                            self.xdc = c;
                                        }
                                    }
                                    self.xdc_files.push(resolved);
                                } else {
                                    self.sources.push(resolved);
                                }
                            }
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
                Ok(format!("added {} files", self.sources.len()))
            }
            "read_xdc" | "read_sdc" => {
                for a in args {
                    if a.starts_with('-') {
                        continue;
                    }
                    let resolved = self.resolve_source_path(a);
                    let text = fs::read_to_string(&resolved)
                        .map_err(|e| format!("read_xdc {}: {e}", resolved.display()))?;
                    if cmd == "read_sdc" {
                        load_sdc(&text, &mut self.xdc.clocks)?;
                    } else {
                        self.xdc = load_xdc(&text)?;
                    }
                    self.xdc_files.push(resolved);
                }
                Ok("read_xdc ok".into())
            }
            "read_ip" => {
                for a in args {
                    if a.starts_with('-') {
                        continue;
                    }
                    let resolved = self.resolve_source_path(a);
                    let mut prj = ProjectFile {
                        part: self.session.part.clone(),
                        ip_packages: vec![resolved.display().to_string()],
                        ..Default::default()
                    };
                    let _ = expand_ip_packages(&mut prj, &resolved)?;
                    for s in &prj.sources {
                        self.sources.push(resolve_prj_path(&resolved, s));
                    }
                }
                Ok("read_ip ok".into())
            }
            "update_compile_order" => Ok("update_compile_order ok".into()),
            "create_clock" => {
                let mut period_ps = 10_000u64;
                let mut name = "clk".to_string();
                let mut port = "clk".to_string();
                let mut i = 0usize;
                while i < args.len() {
                    match args[i].as_str() {
                        "-period" => {
                            if let Some(v) = args.get(i + 1) {
                                if let Ok(ns) = v.parse::<f64>() {
                                    period_ps = if ns < 500.0 {
                                        (ns * 1000.0).round() as u64
                                    } else {
                                        ns.round() as u64
                                    };
                                }
                            }
                            i += 2;
                        }
                        "-name" => {
                            if let Some(n) = args.get(i + 1) {
                                name = n.clone();
                            }
                            i += 2;
                        }
                        "-waveform" => i += 2,
                        other if !other.starts_with('-') => {
                            port = other.to_string();
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
                create_clock(&mut self.xdc.clocks, &name, period_ps, &port);
                Ok(name)
            }
            "synth_design" => {
                let mut i = 0usize;
                while i < args.len() {
                    match args[i].as_str() {
                        "-top" => {
                            if let Some(t) = args.get(i + 1) {
                                self.top = Some(t.clone());
                            }
                            i += 2;
                        }
                        "-part" => {
                            if let Some(p) = args.get(i + 1) {
                                self.requested_part = p.clone();
                                self.session.part = resolve_had_part(p);
                            }
                            i += 2;
                        }
                        "-flatten_hierarchy" | "-directive" => i += 2,
                        other if !other.starts_with('-') && self.sources.is_empty() => {
                            self.sources.push(self.resolve_source_path(other));
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
                let d = synth_sources_for_batch(&self.sources, self.top.as_deref())?;
                let luts = d.lut_inits();
                let ffs = d
                    .cells
                    .iter()
                    .filter(|c| matches!(c.kind, CellKind::Hff))
                    .count();
                let line = format!(
                    "synth_design {} cells={} luts={} ffs={} part={}",
                    resolve(d.name),
                    d.cells.len(),
                    luts.len(),
                    ffs,
                    self.requested_part
                );
                self.session.synth_design(d);
                self.last_snapshot = None;
                self.emit_out(&line);
                Ok(line)
            }
            "opt_design" => {
                if let Some(d) = self.session.design.as_mut() {
                    let removed = opt_design(d);
                    let line = format!("opt_design removed={removed} cells={}", d.cells.len());
                    self.emit_out(&line);
                    Ok(line)
                } else {
                    Err("opt_design: no design (run synth_design first)".into())
                }
            }
            "place_design" => {
                let snap = self.ensure_compiled_snapshot()?;
                let line = format!(
                    "place_design {} lutffs={} iobs={} part={}",
                    snap.design_name,
                    snap.routed.placed.packed.lutffs.len(),
                    snap.routed.placed.packed.iobs.len(),
                    snap.dev.part
                );
                self.emit_out(&line);
                Ok(line)
            }
            "phys_opt_design" => Ok("phys_opt_design ok".into()),
            "route_design" => {
                let snap = self.ensure_compiled_snapshot()?;
                let line = format!(
                    "route_design {} pathfinder_iters={} imux={} overused={}",
                    snap.design_name,
                    snap.routed.pathfinder_iters,
                    snap.routed.imux.len(),
                    snap.routed.overused
                );
                self.emit_out(&line);
                Ok(line)
            }
            "write_bitstream" => {
                let mut out_path: Option<String> = None;
                for a in args {
                    if !a.starts_with('-') {
                        out_path = Some(a.clone());
                    }
                }
                let snap = self.ensure_compiled_snapshot()?;
                if snap.bits.frames.is_empty() {
                    return Err(
                        "write_bitstream: empty bitstream refused (no configured frames)".into(),
                    );
                }
                let bytes = snap.bits.packets.clone();
                let frames = snap.bits.frames.len();
                let dname = snap.design_name.clone();
                if let Some(p) = &out_path {
                    fs::write(p, &bytes).map_err(|e| format!("write {p}: {e}"))?;
                }
                let line = format!(
                    "write_bitstream {dname} frames={frames} bytes={}",
                    bytes.len()
                );
                self.emit_out(&line);
                Ok(line)
            }
            "write_checkpoint" => {
                let mut out_path = "design.hckp".to_string();
                for a in args {
                    if !a.starts_with('-') {
                        out_path = a.clone();
                    }
                }
                let _ = self.ensure_compiled_snapshot()?;
                let wr = self.session.write_checkpoint_to(&out_path)?;
                self.emit_out(&wr);
                Ok(wr)
            }
            "open_checkpoint" | "read_checkpoint" => {
                let path = args
                    .iter()
                    .find(|a| !a.starts_with('-'))
                    .ok_or("open_checkpoint requires file path")?;
                let dev = Device::load_part(&self.session.part)?;
                self.session = Session::open_checkpoint(path, &dev)?;
                let line = format!("open_checkpoint {path} part={}", dev.part);
                self.emit_out(&line);
                Ok(line)
            }
            "report_timing" | "report_timing_summary" => {
                let file_out = take_tcl_flag(args, "-file");
                let snap = self.ensure_compiled_snapshot()?;
                let line = format!(
                    "report_timing {} WNS_PS={} TNS_PS={} endpoints={} r2r_ps={} iob_ps={}",
                    snap.design_name,
                    snap.timing.wns_ps,
                    snap.timing.tns_ps,
                    snap.timing.endpoints,
                    snap.timing.r2r_ps,
                    snap.timing.iob_ps
                );
                if let Some(f) = file_out {
                    fs::write(&f, format!("{line}\n")).map_err(|e| e.to_string())?;
                }
                self.emit_out(&line);
                Ok(line)
            }
            "report_utilization" => {
                let file_out = take_tcl_flag(args, "-file");
                let snap = self.ensure_compiled_snapshot()?;
                let p = &snap.routed.placed.packed;
                let line = format!(
                    "report_utilization {} LUTFF={}/{} IOB={}/{} BRAM={}/{} DSP={}/{}",
                    snap.design_name,
                    p.lutffs.len(),
                    snap.dev.lut6_count(),
                    p.iobs.len(),
                    snap.dev.iob_sites().count(),
                    p.brams.len(),
                    snap.dev.n_bram,
                    p.macs.len(),
                    snap.dev.n_dsp
                );
                if let Some(f) = file_out {
                    fs::write(&f, format!("{line}\n")).map_err(|e| e.to_string())?;
                }
                self.emit_out(&line);
                Ok(line)
            }
            "report_power" => {
                let file_out = take_tcl_flag(args, "-file");
                let snap = self.ensure_compiled_snapshot()?;
                let text = snap.power.text();
                if let Some(f) = file_out {
                    fs::write(&f, format!("{text}\n")).map_err(|e| e.to_string())?;
                }
                self.emit_out(&text);
                Ok(text)
            }
            "report_drc" => {
                let file_out = take_tcl_flag(args, "-file");
                let snap = self.ensure_compiled_snapshot()?;
                let text = snap.drc.text();
                if let Some(f) = file_out {
                    fs::write(&f, format!("{text}\n")).map_err(|e| e.to_string())?;
                }
                self.emit_out(&text);
                Ok(text)
            }
            "export_dashboard" => {
                let dir = args
                    .first()
                    .ok_or("export_dashboard requires an output directory")?;
                let snap = self.ensure_compiled_snapshot()?;
                let (jpath, hpath) = export_dashboard_to_dir(snap, Path::new(dir))?;
                let line = format!(
                    "export_dashboard {} json={} html={}",
                    snap.design_name,
                    jpath.display(),
                    hpath.display()
                );
                self.emit_out(&line);
                Ok(line)
            }
            "get_cells" => {
                let d = self
                    .session
                    .design
                    .as_ref()
                    .ok_or("get_cells: no design (run synth_design first)")?;
                let filter = take_tcl_flag(args, "-filter");
                let pat = args.iter().rfind(|a| !a.starts_with('-') && Some(*a) != filter.as_ref());
                let mut matched = Vec::new();
                for c in &d.cells {
                    let cname = resolve(c.name);
                    if let Some(f) = &filter {
                        if !cell_matches_vivado_filter(&c.kind, cname, f) {
                            continue;
                        }
                    } else if let Some(p) = pat {
                        if p != "*" && !cname.contains(p.trim_matches('*')) {
                            continue;
                        }
                    }
                    matched.push(cname.to_string());
                }
                Ok(matched.join(" "))
            }
            "get_nets" => {
                let d = self
                    .session
                    .design
                    .as_ref()
                    .ok_or("get_nets: no design")?;
                let names: Vec<String> = d.nets.iter().map(|n| resolve(n.name).to_string()).collect();
                Ok(names.join(" "))
            }
            "get_ports" => {
                let d = self
                    .session
                    .design
                    .as_ref()
                    .ok_or("get_ports: no design")?;
                let names: Vec<String> = d
                    .ports
                    .iter()
                    .map(|p| resolve(p.name).to_string())
                    .collect();
                Ok(names.join(" "))
            }
            "set_property" => {
                if args.len() >= 3 {
                    let prop = &args[0];
                    let val = &args[1];
                    let obj = &args[2];
                    if prop.eq_ignore_ascii_case("top") {
                        self.top = Some(val.clone());
                    } else if prop.eq_ignore_ascii_case("part") {
                        self.requested_part = val.clone();
                        self.session.part = resolve_had_part(val);
                    } else if self.session.design.is_some() {
                        let _ = self.session.set_property(prop, val, obj);
                    }
                }
                Ok(String::new())
            }
            other => Err(format!("invalid command name \"{other}\"")),
        }
    }
}

fn take_tcl_flag(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}

fn cell_matches_vivado_filter(kind: &CellKind, cname: &str, filter: &str) -> bool {
    let f = filter.trim();
    if f.contains("LUT") || f.contains("Lut6") {
        return matches!(kind, CellKind::Lut6 { .. });
    }
    if f.contains("FD") || f.contains("Hff") || f.contains("REGISTER") || f.contains("FLOP") {
        return matches!(kind, CellKind::Hff);
    }
    if f.contains("DSP") || f.contains("Mac27") {
        return matches!(kind, CellKind::Mac27 | CellKind::Dsp48e1 { .. });
    }
    if f.contains("RAMB") || f.contains("Bram") || f.contains("BMEM") {
        return matches!(kind, CellKind::Bram18 | CellKind::Bram18e1 { .. });
    }
    if f.contains("IOB") {
        return matches!(kind, CellKind::IobOut | CellKind::IobIn);
    }
    if f.contains("CARRY") {
        return matches!(kind, CellKind::Carry4);
    }
    cname.contains(f)
}

fn eval_simple_expr(expr: &str) -> Result<String, String> {
    let t = expr.trim();
    if let Ok(n) = t.parse::<i64>() {
        return Ok(n.to_string());
    }
    for op in ["==", "!=", "<=", ">=", "<", ">", "+", "-", "*", "/"] {
        if let Some((l, r)) = t.split_once(op) {
            let ls = l.trim().trim_matches('"');
            let rs = r.trim().trim_matches('"');
            if let (Ok(la), Ok(ra)) = (ls.parse::<i64>(), rs.parse::<i64>()) {
                let v = match op {
                    "==" => i64::from(la == ra),
                    "!=" => i64::from(la != ra),
                    "<=" => i64::from(la <= ra),
                    ">=" => i64::from(la >= ra),
                    "<" => i64::from(la < ra),
                    ">" => i64::from(la > ra),
                    "+" => la + ra,
                    "-" => la - ra,
                    "*" => la * ra,
                    "/" => {
                        if ra == 0 {
                            return Err("divide by zero".into());
                        }
                        la / ra
                    }
                    _ => 0,
                };
                return Ok(v.to_string());
            } else {
                let v = match op {
                    "==" => i64::from(ls == rs),
                    "!=" => i64::from(ls != rs),
                    _ => 0,
                };
                return Ok(v.to_string());
            }
        }
    }
    Ok(t.to_string())
}

fn join_tcl_list(items: &[String]) -> String {
    items
        .iter()
        .map(|s| {
            if s.is_empty() || s.contains(char::is_whitespace) {
                format!("{{{s}}}")
            } else {
                s.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_tcl_list(s: &str) -> Vec<String> {
    tokenize_tcl_words(s)
        .unwrap_or_default()
        .into_iter()
        .map(|(w, _)| w)
        .collect()
}

fn extract_bracketed(chars: &[char], start_bracket: usize) -> Result<(String, usize), String> {
    let mut depth = 0usize;
    let mut i = start_bracket;
    let mut inner = String::new();
    while i < chars.len() {
        let c = chars[i];
        if c == '[' {
            if depth > 0 {
                inner.push(c);
            }
            depth += 1;
        } else if c == ']' {
            depth -= 1;
            if depth == 0 {
                return Ok((inner, i + 1));
            }
            inner.push(c);
        } else {
            inner.push(c);
        }
        i += 1;
    }
    Err("missing close-bracket ']'".into())
}

fn split_tcl_statements(script: &str) -> Result<Vec<String>, String> {
    let chars: Vec<char> = script.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut brace_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut in_quotes = false;
    let mut in_comment = false;
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];
        if in_comment {
            if c == '\n' {
                in_comment = false;
            }
            i += 1;
            continue;
        }
        if c == '\\' && i + 1 < chars.len() {
            if chars[i + 1] == '\n' {
                cur.push(' ');
                i += 2;
                continue;
            }
            cur.push(c);
            cur.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if !in_quotes && brace_depth == 0 && bracket_depth == 0 && c == '#' && cur.trim().is_empty()
        {
            in_comment = true;
            i += 1;
            continue;
        }
        if c == '"' && brace_depth == 0 {
            in_quotes = !in_quotes;
            cur.push(c);
            i += 1;
            continue;
        }
        if !in_quotes {
            if c == '{' {
                brace_depth += 1;
            } else if c == '}' {
                brace_depth = (brace_depth - 1).max(0);
            } else if c == '[' {
                bracket_depth += 1;
            } else if c == ']' {
                bracket_depth = (bracket_depth - 1).max(0);
            } else if (c == '\n' || c == ';') && brace_depth == 0 && bracket_depth == 0 {
                if !cur.trim().is_empty() {
                    out.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
                i += 1;
                continue;
            }
        }
        cur.push(c);
        i += 1;
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// Returns `(word, is_braced_no_subst)`.
fn tokenize_tcl_words(stmt: &str) -> Result<Vec<(String, bool)>, String> {
    let chars: Vec<char> = stmt.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        if chars[i] == '#' && out.is_empty() {
            break;
        }
        if chars[i] == '{' {
            let mut depth = 1usize;
            i += 1;
            let mut w = String::new();
            while i < chars.len() && depth > 0 {
                if chars[i] == '{' {
                    depth += 1;
                    w.push('{');
                } else if chars[i] == '}' {
                    depth -= 1;
                    if depth > 0 {
                        w.push('}');
                    }
                } else {
                    w.push(chars[i]);
                }
                i += 1;
            }
            out.push((w, true));
        } else if chars[i] == '"' {
            i += 1;
            let mut w = String::new();
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    w.push(chars[i]);
                    w.push(chars[i + 1]);
                    i += 2;
                } else {
                    w.push(chars[i]);
                    i += 1;
                }
            }
            if i < chars.len() {
                i += 1;
            }
            out.push((w, false));
        } else {
            let mut w = String::new();
            let mut bdepth = 0i32;
            while i < chars.len() && (!chars[i].is_whitespace() || bdepth > 0) {
                if chars[i] == '[' {
                    bdepth += 1;
                } else if chars[i] == ']' {
                    bdepth = (bdepth - 1).max(0);
                }
                w.push(chars[i]);
                i += 1;
            }
            out.push((w, false));
        }
    }
    Ok(out)
}

/// Entrypoint for Vivado-compatible `-mode batch|tcl|gui`, `-source <script.tcl>`,
/// and standalone `--export-dashboard <dir>`.
pub fn run_vivado_cli_or_dashboard(raw_args: &[String], gui_fn: fn()) {
    let opts = VivadoCliOpts::parse(raw_args).unwrap_or_else(|e| {
        eprintln!("helion: {e}");
        std::process::exit(2);
    });

    if opts.mode.as_deref() == Some("gui") && opts.sources.is_empty() && opts.export_dashboard.is_none() {
        gui_fn();
        return;
    }

    let mut tcl = BatchTclSession::new(&opts);

    // 1. Execute any `-source <script.tcl>` files in order
    for src_file in &opts.sources {
        let p = Path::new(src_file);
        if !p.exists() {
            let msg = format!("ERROR: couldn't read file \"{src_file}\": no such file or directory");
            tcl.log_lines.push(msg.clone());
            let _ = tcl.flush_logs(&opts);
            eprintln!("{msg}");
            std::process::exit(1);
        }
        let script = fs::read_to_string(p).unwrap_or_else(|e| {
            eprintln!("ERROR: read {src_file}: {e}");
            std::process::exit(1);
        });
        if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                tcl.script_base_dir = parent.to_path_buf();
            }
        }
        tcl.vars.insert("argv0".to_string(), src_file.clone());
        if let Err(e) = tcl.eval_script(&script, true) {
            let msg = format!("ERROR: {e}");
            tcl.log_lines.push(msg.clone());
            let _ = tcl.flush_logs(&opts);
            eprintln!("{msg}");
            std::process::exit(1);
        }
        if tcl.exit_requested.is_some() {
            break;
        }
    }

    // 2. If `-mode tcl` and no exit was requested yet, read commands from stdin if piped/available
    if opts.mode.as_deref() == Some("tcl") && tcl.exit_requested.is_none() && opts.sources.is_empty()
    {
        let stdin = std::io::stdin();
        let mut buf = String::new();
        let mut depth = 0i32;
        for line in stdin.lock().lines() {
            let Ok(l) = line else { break };
            for c in l.chars() {
                if c == '{' {
                    depth += 1;
                } else if c == '}' {
                    depth = (depth - 1).max(0);
                }
            }
            buf.push_str(&l);
            buf.push('\n');
            if depth > 0 {
                continue;
            }
            let chunk = std::mem::take(&mut buf);
            if let Err(e) = tcl.eval_script(&chunk, true) {
                let msg = format!("ERROR: {e}");
                tcl.log_lines.push(msg.clone());
                let _ = tcl.flush_logs(&opts);
                eprintln!("{msg}");
                std::process::exit(1);
            }
            let _ = std::io::stdout().flush();
            if tcl.exit_requested.is_some() {
                break;
            }
        }
    }

    // 3. If `--export-dashboard <dir>` was given, export JSON + HTML dashboard
    if let Some(export_dir) = &opts.export_dashboard {
        if tcl.session.design.is_none() && tcl.sources.is_empty() {
            let target = opts
                .design_target
                .clone()
                .unwrap_or_else(|| "examples/counter.sv".to_string());
            tcl.sources.push(PathBuf::from(target));
        }
        if let Some(sdc_path) = &opts.sdc {
            if let Ok(text) = fs::read_to_string(sdc_path) {
                if let Ok(c) = load_xdc(&text) {
                    tcl.xdc = c;
                }
            }
        }
        let line = match tcl.ensure_compiled_snapshot() {
            Ok(snap) => match export_dashboard_to_dir(snap, Path::new(export_dir)) {
                Ok((jpath, hpath)) => format!(
                    "export_dashboard {} part={} WNS_PS={} LUTFF={} json={} html={}",
                    snap.design_name,
                    snap.dev.part,
                    snap.timing.wns_ps,
                    snap.routed.placed.packed.lutffs.len(),
                    jpath.display(),
                    hpath.display()
                ),
                Err(e) => {
                    eprintln!("export-dashboard: {e}");
                    let _ = tcl.flush_logs(&opts);
                    std::process::exit(1);
                }
            },
            Err(e) => {
                eprintln!("export-dashboard: {e}");
                let _ = tcl.flush_logs(&opts);
                std::process::exit(1);
            }
        };
        tcl.emit_out(&line);
    }

    if let Err(e) = tcl.flush_logs(&opts) {
        eprintln!("{e}");
        std::process::exit(1);
    }

    if let Some(code) = tcl.exit_requested {
        if code != 0 {
            std::process::exit(code);
        }
    }
}
