//! Clock Utilization, Control Sets, SSN and IBIS report pages.
//!
//! Every number comes from the backend over the last run's design and
//! placement ([`crate::analysis::run_inputs`]):
//! - `helion_proj::evaluate_clock_utilization` (UG835 `report_clock_utilization`);
//! - `helion_proj::io_reports::report_control_sets_design` (helion-pack);
//! - `helion_proj::io_reports::report_ssn_design` (helion-device `analyze_ssn`);
//! - `helion_proj::io_reports::write_ibis_design` (helion-device IBIS writer).
//!
//! Engines run on a [`crate::bg`] worker; the page shows the last result
//! or the reason it could not run.

use std::path::PathBuf;

use rune::{El, col, el, row};

use super::ReportNav;
use crate::state::{AppState, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::cad;
use crate::views::report_kit::{self as kit, header_row, num_cell, plain_row, select_row, table};

const NUM_W: f32 = 72.0;
const NAME_W: f32 = 140.0;

/// SSN result plus the package and port mapping it used.
#[derive(Clone, Debug)]
pub struct SsnSnap {
    pub package: String,
    pub ports: Vec<helion_proj::io_reports::PlacedIoPort>,
    pub report: helion_device::SsnReport,
}

/// IBIS export result.
#[derive(Clone, Debug)]
pub struct IbisSnap {
    pub package: String,
    pub ports: Vec<helion_proj::io_reports::PlacedIoPort>,
    pub models: Vec<String>,
    pub path: PathBuf,
    pub bytes: usize,
}

/// State of the four I/O / clocking report pages.
#[derive(Clone, Debug, Default)]
pub struct IoReports {
    pub clock: Option<Result<helion_proj::ClockUtilizationReport, String>>,
    pub control: Option<Result<helion_pack::ControlSetReport, String>>,
    pub ssn: Option<Result<SsnSnap, String>>,
    pub ibis: Option<Result<IbisSnap, String>>,
    pub selected_ctrl: Option<usize>,
    pub selected_ssn: Option<usize>,
}

/// Owned engine inputs (cloned on the UI thread).
#[derive(Clone)]
pub struct IoInputs {
    pub design: helion_ir::Design,
    pub placed: Option<helion_place::Placed>,
    pub part: String,
    pub xdc: helion_sta::Constraints,
}

/// Inputs from the last run, or why the reports cannot run.
pub fn io_inputs(s: &AppState) -> Result<IoInputs, String> {
    let inp = crate::analysis::run_inputs(s)?;
    let lines = std::cell::RefCell::new(Vec::new());
    let (xdc, _) = crate::runs::load_ctx_constraints(&inp.ctx, &|t, sev| {
        lines.borrow_mut().push((t, sev))
    });
    Ok(IoInputs {
        design: inp.design,
        placed: inp.ctx.routed.map(|r| r.placed),
        part: inp.ctx.part,
        xdc,
    })
}

fn device(part: &str) -> Result<helion_device::Device, String> {
    crate::runs::resolve_device(part, &|_, _| {})
}

fn placed_or_err(inp: &IoInputs, verb: &str) -> Result<helion_place::Placed, String> {
    inp.placed
        .clone()
        .ok_or_else(|| format!("{verb}: needs a placed design (Run Implementation first)"))
}

pub fn compute_clock(inp: &IoInputs) -> Result<helion_proj::ClockUtilizationReport, String> {
    let dev = device(&inp.part)?;
    let mut session = helion_proj::Session::new(helion_proj::Mode::NonProject);
    session.synth_design(inp.design.clone());
    session.placed = inp.placed.clone();
    helion_proj::evaluate_clock_utilization(&session, &dev, &inp.xdc)
}

pub fn compute_control(inp: &IoInputs) -> Result<helion_pack::ControlSetReport, String> {
    let dev = device(&inp.part)?;
    Ok(helion_proj::io_reports::report_control_sets_design(
        &inp.design,
        inp.placed.as_ref(),
        &dev,
    ))
}

/// Outputs switch at the fastest constrained clock (100 MHz when none).
fn toggle_mhz(xdc: &helion_sta::Constraints) -> f64 {
    xdc.clocks
        .iter()
        .map(|c| c.period_ps)
        .filter(|p| *p > 0)
        .min()
        .map(|p| 1_000_000.0 / p as f64)
        .unwrap_or(100.0)
}

pub fn compute_ssn(inp: &IoInputs) -> Result<SsnSnap, String> {
    let dev = device(&inp.part)?;
    let placed = placed_or_err(inp, "report_ssn")?;
    let (kind, ports, report) =
        helion_proj::io_reports::report_ssn_design(&inp.design, &placed, &dev, toggle_mhz(&inp.xdc))?;
    Ok(SsnSnap {
        package: kind.name().to_string(),
        ports,
        report,
    })
}

pub fn compute_ibis(inp: &IoInputs, path: &std::path::Path) -> Result<IbisSnap, String> {
    let dev = device(&inp.part)?;
    let placed = placed_or_err(inp, "write_ibis")?;
    let file = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "design.ibs".into());
    let (kind, ports, text) =
        helion_proj::io_reports::write_ibis_design(&inp.design, &placed, &dev, &file)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("write_ibis {}: {e}", dir.display()))?;
    }
    std::fs::write(path, &text).map_err(|e| format!("write_ibis {}: {e}", path.display()))?;
    let models = text
        .lines()
        .filter_map(|l| l.strip_prefix("[Model]"))
        .map(|m| m.trim().to_string())
        .collect();
    Ok(IbisSnap {
        package: kind.name().to_string(),
        ports,
        models,
        path: path.to_path_buf(),
        bytes: text.len(),
    })
}

/// Default `.ibs` path: next to the open project, else the runs dir.
pub fn default_ibis_path(s: &AppState) -> PathBuf {
    let top = s
        .runs
        .live_design
        .as_ref()
        .map(|d| helion_ir::resolve(d.name).to_string())
        .unwrap_or_else(|| "design".into());
    crate::bg::output_dir(s).join(format!("{}.ibs", top.to_ascii_lowercase()))
}

fn log(s: &mut AppState, text: String, sev: MsgSev) {
    s.console.lines.push(ConsoleLine { text, sev });
}

fn log_result<T>(s: &mut AppState, r: &Result<T, String>, ok: impl Fn(&T) -> String) {
    match r {
        Ok(v) => log(s, ok(v), MsgSev::Info),
        Err(e) => log(s, format!("ERROR: {e}"), MsgSev::Error),
    }
}

/// Background key for a page's engine.
pub fn task_key(nav: ReportNav) -> &'static str {
    match nav {
        ReportNav::ClockUtil => "report_clock_utilization",
        ReportNav::ControlSets => "report_control_sets",
        ReportNav::Ssn => "report_ssn",
        ReportNav::Ibis => "write_ibis",
        _ => "",
    }
}

/// Start the engine behind `nav` on a worker. `Err` when inputs are
/// missing (the page shows the reason too).
pub fn run(s: &mut AppState, nav: ReportNav, ibis_path: Option<PathBuf>) -> Result<String, String> {
    let inp = match io_inputs(s) {
        Ok(i) => i,
        Err(e) => {
            store_err(s, nav, e.clone());
            return Err(e);
        }
    };
    let key = task_key(nav);
    let started = match nav {
        ReportNav::ClockUtil => crate::bg::spawn(s, key, move || compute_clock(&inp), |s, r| {
            log_result(s, &r, |c| c.text().lines().next().unwrap_or_default().to_string());
            s.reports_view.io.clock = Some(r);
        }),
        ReportNav::ControlSets => crate::bg::spawn(s, key, move || compute_control(&inp), |s, r| {
            log_result(s, &r, |c| c.text().lines().next().unwrap_or_default().to_string());
            s.reports_view.io.selected_ctrl = None;
            s.reports_view.io.control = Some(r);
        }),
        ReportNav::Ssn => crate::bg::spawn(s, key, move || compute_ssn(&inp), |s, r| {
            log_result(s, &r, |c| c.report.summary_text().lines().next().unwrap_or_default().to_string());
            s.reports_view.io.selected_ssn = None;
            s.reports_view.io.ssn = Some(r);
        }),
        ReportNav::Ibis => {
            let path = ibis_path.unwrap_or_else(|| default_ibis_path(s));
            crate::bg::spawn(s, key, move || compute_ibis(&inp, &path), |s, r| {
                log_result(s, &r, |c| {
                    format!(
                        "write_ibis {} package={} pins={} models={} bytes={}",
                        c.path.display(),
                        c.package,
                        c.ports.len(),
                        c.models.len(),
                        c.bytes
                    )
                });
                s.reports_view.io.ibis = Some(r);
            })
        }
        _ => return Err("not an I/O report".into()),
    };
    if started {
        Ok(format!("{key} running"))
    } else {
        Err(format!("{key} is already running"))
    }
}

fn store_err(s: &mut AppState, nav: ReportNav, e: String) {
    let io = &mut s.reports_view.io;
    match nav {
        ReportNav::ClockUtil => io.clock = Some(Err(e)),
        ReportNav::ControlSets => io.control = Some(Err(e)),
        ReportNav::Ssn => io.ssn = Some(Err(e)),
        ReportNav::Ibis => io.ibis = Some(Err(e)),
        _ => {}
    }
}

/// Opening a page that has never run starts its engine (cheap, real).
pub fn on_select(s: &mut AppState, nav: ReportNav) {
    let io = &s.reports_view.io;
    let fresh = match nav {
        ReportNav::ClockUtil => io.clock.is_none(),
        ReportNav::ControlSets => io.control.is_none(),
        ReportNav::Ssn => io.ssn.is_none(),
        _ => false,
    };
    if fresh {
        let _ = run(s, nav, None);
    }
}

/// Why the page's re-run action is disabled (`None` = enabled).
pub fn unavailable(s: &AppState, nav: ReportNav) -> Option<&'static str> {
    if crate::bg::busy(s, task_key(nav)) {
        return Some("Running…");
    }
    if s.runs.is_active() {
        return Some("A run is in progress");
    }
    if s.runs.live_design.is_none() || s.runs.analysis.is_none() {
        return Some("Run Synthesis or Implementation first");
    }
    let placed = s.runs.analysis.as_ref().is_some_and(|a| a.routed.is_some());
    if matches!(nav, ReportNav::Ssn | ReportNav::Ibis) && !placed {
        return Some("Needs placed I/O (Run Implementation first)");
    }
    None
}

fn kpis(cards: Vec<El>) -> El {
    let mut r = row().w_full().gap(8.0);
    for c in cards {
        r = r.child(c);
    }
    r
}

fn pending_or_error<T>(
    key: &'static str,
    v: &Option<Result<T, String>>,
    busy: bool,
    t: &VivadoTheme,
) -> Option<El> {
    match v {
        _ if busy && v.as_ref().is_none_or(|r| r.is_err()) => Some(kit::empty_state(
            key,
            "Running the analysis…",
            None,
            t,
        )),
        None => Some(kit::empty_state(key, "Not run yet — use the action above.", None, t)),
        Some(Err(e)) => Some(kit::empty_state(key, e, None, t)),
        Some(Ok(_)) => None,
    }
}

fn ns(ps: u64) -> String {
    format!("{:.3}", ps as f64 / 1000.0)
}

pub fn clock_body(s: &AppState, t: &VivadoTheme) -> El {
    let body = col().key("reports-clockutil").w_full().gap(10.0);
    let busy = crate::bg::busy(s, task_key(ReportNav::ClockUtil));
    if let Some(e) = pending_or_error("reports-clockutil-empty", &s.reports_view.io.clock, busy, t) {
        return body.child(e);
    }
    let Some(Ok(r)) = &s.reports_view.io.clock else {
        return body;
    };
    let loads: usize = r.domains.iter().map(|d| d.ff_loads + d.bram_loads + d.dsp_loads).sum();
    let head = header_row(t)
        .child(kit::header_cell("Clock", t).grow(1.0).min_w(0.0))
        .child(el().w(NAME_W).child(kit::header_cell("Source", t)))
        .child(kit::header_num("Period (ns)", NUM_W + 8.0, t))
        .child(kit::header_num("Freq (MHz)", NUM_W + 8.0, t))
        .child(kit::header_num("BUFG", 48.0, t))
        .child(kit::header_num("FF loads", NUM_W, t))
        .child(kit::header_num("BRAM", 48.0, t))
        .child(kit::header_num("DSP", 48.0, t))
        .child(kit::header_num("Regions", 56.0, t))
        .child(kit::header_num("Insertion (ns)", 96.0, t));
    let mut rows = col().w_full();
    for (i, d) in r.domains.iter().enumerate() {
        let mhz = if d.period_ps > 0 { 1_000_000.0 / d.period_ps as f64 } else { 0.0 };
        rows = rows.child(
            plain_row(format!("reports-clockutil-row-{i}"))
                .child(kit::text_cell(d.clock_name.clone(), 1.0, t.text))
                .child(kit::fixed_cell(d.source_port.clone(), NAME_W, t.text_dim))
                .child(num_cell(ns(d.period_ps), NUM_W + 8.0, t.text))
                .child(num_cell(format!("{mhz:.2}"), NUM_W + 8.0, t.text))
                .child(num_cell(d.global_buffers.to_string(), 48.0, t.text))
                .child(num_cell(d.ff_loads.to_string(), NUM_W, t.text))
                .child(num_cell(d.bram_loads.to_string(), 48.0, t.text))
                .child(num_cell(d.dsp_loads.to_string(), 48.0, t.text))
                .child(num_cell(d.clock_regions_spanned.to_string(), 56.0, t.text))
                .child(num_cell(format!("{:.3}", d.insertion_delay_ps as f64 / 1000.0), 96.0, t.text)),
        );
    }
    if r.domains.is_empty() {
        rows = rows.child(kit::empty_state("reports-clockutil-none", "The design has no clock nets.", None, t));
    }
    body.child(kpis(vec![
        cad::kpi("reports-clockutil-kpi-bufg", "Global buffers", format!("{} / {}", r.total_bufg_used, r.total_bufg_available), t.accent, "BUFG used / available", t),
        cad::kpi("reports-clockutil-kpi-clocks", "Clocks", r.domains.len().to_string(), t.text, format!("part {}", r.part), t),
        cad::kpi("reports-clockutil-kpi-loads", "Clock loads", loads.to_string(), t.text, "FF + BRAM + DSP pins", t),
    ]))
    .child(kit::section_title("Clock primitive utilization by domain", t))
    .child(table(head, rows, t))
}

pub fn control_body(s: &AppState, t: &VivadoTheme) -> El {
    let body = col().key("reports-ctrlsets").w_full().gap(10.0);
    let busy = crate::bg::busy(s, task_key(ReportNav::ControlSets));
    if let Some(e) = pending_or_error("reports-ctrlsets-empty", &s.reports_view.io.control, busy, t) {
        return body.child(e);
    }
    let Some(Ok(r)) = &s.reports_view.io.control else {
        return body;
    };
    let slices = if r.placed { "placed slices" } else { "estimated (not placed)" };
    let mut hist = col().w_full().gap(2.0);
    let max = r.histogram.iter().copied().max().unwrap_or(0).max(1);
    for (i, b) in helion_pack::CONTROL_SET_BUCKETS.iter().enumerate() {
        let n = r.histogram[i];
        hist = hist.child(
            plain_row(format!("reports-ctrlsets-hist-{i}"))
                .child(kit::fixed_cell(format!("Fanout {b}"), 96.0, t.text_dim))
                .child(kit::bar(n as f32 / max as f32, t.accent, t))
                .child(num_cell(n.to_string(), 48.0, t.text)),
        );
    }
    let head = header_row(t)
        .child(kit::header_cell("Clock Signal", t).grow(1.0).min_w(0.0))
        .child(el().w(NAME_W).child(kit::header_cell("Enable Signal", t)))
        .child(el().w(NAME_W).child(kit::header_cell("Set/Reset Signal", t)))
        .child(kit::header_num("Slice Loads", 84.0, t))
        .child(kit::header_num("Bel Loads", 72.0, t));
    let mut rows = col().w_full();
    let sel = s.reports_view.io.selected_ctrl;
    for (i, c) in r.rows.iter().enumerate() {
        let label = format!("{} / {}", c.clock, c.enable.as_deref().unwrap_or("-"));
        rows = rows.child(
            select_row(format!("reports-ctrlsets-row-{i}"), label, sel == Some(i), t, move |s: &mut AppState| {
                s.reports_view.io.selected_ctrl = Some(i);
            })
            .child(kit::text_cell(c.clock.clone(), 1.0, t.text))
            .child(kit::fixed_cell(c.enable.clone().unwrap_or_else(|| "-".into()), NAME_W, t.text_dim))
            .child(kit::fixed_cell(c.set_reset.clone().unwrap_or_else(|| "-".into()), NAME_W, t.text_dim))
            .child(num_cell(c.slice_loads.to_string(), 84.0, t.text))
            .child(num_cell(c.bel_loads.to_string(), 72.0, t.text)),
        );
    }
    let mut out = body
        .child(kpis(vec![
            cad::kpi("reports-ctrlsets-kpi-total", "Control sets", r.total().to_string(), t.accent, format!("{} flip-flops", r.total_ffs()), t),
            cad::kpi("reports-ctrlsets-kpi-ce", "With enable", r.with_enable().to_string(), t.text, "CE driven", t),
            cad::kpi("reports-ctrlsets-kpi-sr", "With set/reset", r.with_set_reset().to_string(), t.text, "SR driven", t),
            cad::kpi("reports-ctrlsets-kpi-unused", "Unused FF slots", r.unused_ff_slots.to_string(), t.text, slices, t),
        ]))
        .child(kit::section_title("Control sets by fanout", t))
        .child(hist)
        .child(kit::section_title("Control set details", t))
        .child(table(head, rows, t));
    if let Some(c) = sel.and_then(|i| r.rows.get(i)) {
        out = out.child(kit::detail_panel(
            "reports-ctrlsets-detail",
            format!("{} flip-flops on {}", c.bel_loads, c.clock),
            vec![("Cells", kit::wrap_mono(c.ff_cells.join("  "), t.text))],
            t,
        ));
    }
    out
}

pub fn ssn_body(s: &AppState, t: &VivadoTheme) -> El {
    let body = col().key("reports-ssn").w_full().gap(10.0);
    let busy = crate::bg::busy(s, task_key(ReportNav::Ssn));
    if let Some(e) = pending_or_error("reports-ssn-empty", &s.reports_view.io.ssn, busy, t) {
        return body.child(e);
    }
    let Some(Ok(snap)) = &s.reports_view.io.ssn else {
        return body;
    };
    let r = &snap.report;
    let defaulted = snap.ports.iter().filter(|p| p.default_std).count();
    let margin_color = |m: f64| if m < 0.0 { t.fail } else if m < 20.0 { t.warn } else { t.pass };
    let bank_head = header_row(t)
        .child(kit::header_cell("Bank", t).grow(1.0).min_w(0.0))
        .child(kit::header_num("VCCO (mV)", NUM_W, t))
        .child(kit::header_num("Switching", NUM_W, t))
        .child(kit::header_num("Worst SSN (mV)", 104.0, t))
        .child(kit::header_num("Margin", NUM_W, t))
        .child(kit::header_num("Status", 56.0, t));
    let mut banks = col().w_full();
    for (i, b) in r.banks.iter().enumerate() {
        banks = banks.child(
            plain_row(format!("reports-ssn-bank-{i}"))
                .child(kit::text_cell(format!("Bank {}", b.bank), 1.0, t.text))
                .child(num_cell(b.vcco_mv.to_string(), NUM_W, t.text))
                .child(num_cell(b.switching_outputs.to_string(), NUM_W, t.text))
                .child(num_cell(format!("{:.1}", b.worst_ssn_mv), 104.0, t.text))
                .child(num_cell(format!("{:.1}%", b.worst_margin_pct), NUM_W, margin_color(b.worst_margin_pct)))
                .child(num_cell(if b.passes { "PASS" } else { "FAIL" }, 56.0, if b.passes { t.pass } else { t.fail })),
        );
    }
    let pin_head = header_row(t)
        .child(kit::header_cell("Port", t).grow(1.0).min_w(0.0))
        .child(el().w(48.0).child(kit::header_cell("Ball", t)))
        .child(kit::header_num("Bank", 40.0, t))
        .child(el().w(88.0).child(kit::header_cell("I/O Std", t)))
        .child(kit::header_num("Drive", 48.0, t))
        .child(el().w(44.0).child(kit::header_cell("Slew", t)))
        .child(kit::header_num("SSN (mV)", NUM_W, t))
        .child(kit::header_num("Budget", NUM_W, t))
        .child(kit::header_num("Margin", NUM_W, t));
    let mut pins = col().w_full();
    let sel = s.reports_view.io.selected_ssn;
    for (i, p) in r.pins.iter().enumerate() {
        let std_color = if snap.ports.iter().any(|q| q.port == p.port && q.default_std) { t.text_faint } else { t.text_dim };
        pins = pins.child(
            select_row(format!("reports-ssn-pin-{i}"), p.port.clone(), sel == Some(i), t, move |s: &mut AppState| {
                s.reports_view.io.selected_ssn = Some(i);
            })
            .child(kit::text_cell(p.port.clone(), 1.0, t.text))
            .child(kit::fixed_cell(p.ball.clone(), 48.0, t.text_dim))
            .child(num_cell(p.bank.to_string(), 40.0, t.text))
            .child(kit::fixed_cell(p.iostandard.clone(), 88.0, std_color))
            .child(num_cell(format!("{}", p.drive_ma), 48.0, t.text))
            .child(kit::fixed_cell(p.slew.clone(), 44.0, t.text_dim))
            .child(num_cell(format!("{:.1}", p.induced_ssn_mv), NUM_W, t.text))
            .child(num_cell(format!("{:.0}", p.noise_budget_mv), NUM_W, t.text_dim))
            .child(num_cell(format!("{:.1}%", p.margin_pct), NUM_W, margin_color(p.margin_pct))),
        );
    }
    let mut out = body
        .child(kpis(vec![
            cad::kpi("reports-ssn-kpi-margin", "Worst margin", format!("{:.1}%", r.worst_margin_pct), margin_color(r.worst_margin_pct), "noise budget left", t),
            cad::kpi("reports-ssn-kpi-fail", "Failing pins", r.failing_pins.to_string(), if r.failing_pins > 0 { t.fail } else { t.pass }, format!("of {} analyzed", r.pins.len()), t),
            cad::kpi("reports-ssn-kpi-pkg", "Package", snap.package.clone(), t.text, if r.banks.len() == 1 { "1 bank".to_string() } else { format!("{} banks", r.banks.len()) }, t),
        ]))
        .child(cad::dim(
            format!(
                "Modeled Helion BGA package via placed IOB sites.{}",
                if defaulted > 0 { format!(" {defaulted} port(s) default to LVCMOS33 (dimmed).") } else { String::new() }
            ),
            t,
        ).truncate())
        .child(kit::section_title("Banks", t))
        .child(table(bank_head, banks, t))
        .child(kit::section_title("Pins", t))
        .child(table(pin_head, pins, t));
    if let Some(p) = sel.and_then(|i| r.pins.get(i)) {
        out = out.child(kit::detail_panel(
            "reports-ssn-detail",
            format!("{} on {}", p.port, p.ball),
            vec![
                ("Aggressors", kit::wrap_value(format!("{} simultaneously switching outputs couple into this pin", p.coupled_aggressors), t.text)),
                ("Fix", kit::wrap_value("Lower DRIVE, use SLEW SLOW, or move the port nearer a GND ball.", t.text_dim)),
            ],
            t,
        ));
    }
    out
}

pub fn ibis_body(s: &AppState, t: &VivadoTheme) -> El {
    let body = col().key("reports-ibis").w_full().gap(10.0);
    let busy = crate::bg::busy(s, task_key(ReportNav::Ibis));
    if let Some(e) = pending_or_error("reports-ibis-empty", &s.reports_view.io.ibis, busy, t) {
        return body.child(e);
    }
    let Some(Ok(snap)) = &s.reports_view.io.ibis else {
        return body;
    };
    let head = header_row(t)
        .child(kit::header_cell("Port", t).grow(1.0).min_w(0.0))
        .child(el().w(48.0).child(kit::header_cell("Ball", t)))
        .child(el().w(NAME_W).child(kit::header_cell("Model", t)))
        .child(el().w(88.0).child(kit::header_cell("I/O Std", t)))
        .child(kit::header_num("Drive", 48.0, t))
        .child(el().w(44.0).child(kit::header_cell("Slew", t)));
    let mut rows = col().w_full();
    for (i, p) in snap.ports.iter().enumerate() {
        let model = helion_device::IbisModel::for_pin(&helion_device::IbisPin {
            ball: p.ball.clone(),
            signal: p.port.clone(),
            iostandard: p.iostandard.clone(),
            drive_ma: p.drive_ma,
            fast: p.slew.as_deref() == Some("FAST"),
        })
        .map(|m| m.name)
        .unwrap_or_else(|e| e);
        rows = rows.child(
            plain_row(format!("reports-ibis-row-{i}"))
                .child(kit::text_cell(p.port.clone(), 1.0, t.text))
                .child(kit::fixed_cell(p.ball.clone(), 48.0, t.text_dim))
                .child(kit::fixed_cell(model, NAME_W, t.text))
                .child(kit::fixed_cell(p.iostandard.clone(), 88.0, if p.default_std { t.text_faint } else { t.text_dim }))
                .child(num_cell(p.drive_ma.map(|d| d.to_string()).unwrap_or_else(|| "default".into()), 48.0, t.text))
                .child(kit::fixed_cell(p.slew.clone().unwrap_or_else(|| "SLOW".into()), 44.0, t.text_dim)),
        );
    }
    let path = snap.path.display().to_string();
    body.child(kpis(vec![
        cad::kpi("reports-ibis-kpi-pins", "Signal pins", snap.ports.len().to_string(), t.accent, format!("package {}", snap.package), t),
        cad::kpi("reports-ibis-kpi-models", "Buffer models", snap.models.len().to_string(), t.text, snap.models.join(", "), t),
        cad::kpi("reports-ibis-kpi-size", "File size", format!("{:.1} KB", snap.bytes as f64 / 1024.0), t.text, "IBIS 5.0", t),
    ]))
    .child(cad::kv("Written to", kit::text_cell(path.clone(), 1.0, t.link).key("reports-ibis-path"), 80.0, t))
    .child(cad::dim("Behavioral buffers derived from DRIVE / SLEW / VCCO and the package model — not silicon-correlated.", t).truncate())
    .child(kit::section_title("Pins", t))
    .child(table(head, rows, t))
}

/// `(label, key, stage text)` for the action bar of an I/O page.
pub fn action_meta(nav: ReportNav) -> (&'static str, &'static str, &'static str) {
    match nav {
        ReportNav::ClockUtil => ("Report Clock Utilization", "reports-action-clockutil", "Clock primitives over the last run"),
        ReportNav::ControlSets => ("Report Control Sets", "reports-action-ctrlsets", "Flip-flop control sets over the last run"),
        ReportNav::Ssn => ("Report SSN", "reports-action-ssn", "Ground bounce over placed I/O"),
        _ => ("Write IBIS Model", "reports-action-ibis", "IBIS 5.0 export of placed I/O"),
    }
}

/// Tcl verbs: `report_clock_utilization`, `report_control_sets`,
/// `report_ssn`, `write_ibis ?path?`. Each navigates to its page, runs on
/// a worker and prints its summary when done.
pub fn tcl_verb(s: &mut AppState, name: &str, args: &[String]) -> Result<String, String> {
    let nav = match name {
        "report_clock_utilization" => ReportNav::ClockUtil,
        "report_control_sets" => ReportNav::ControlSets,
        "report_ssn" => ReportNav::Ssn,
        "write_ibis" => ReportNav::Ibis,
        _ => return Err(format!("{name}: not an I/O report verb")),
    };
    s.navigate(crate::state::CenterPane::Reports);
    s.reports_view.selected = nav;
    let path = args.iter().find(|a| !a.starts_with('-')).map(PathBuf::from);
    run(s, nav, path)
}

/// Scene fixture: implement a four-output counter in-process and run the
/// real engines over it (no constants), selecting `nav`.
pub fn scene_fill(s: &mut AppState, nav: ReportNav) {
    let Ok(dev) = helion_device::Device::load_part("HL10T-C32-1") else {
        return;
    };
    let mut d = helion_ir::Design::structural_counter();
    for i in 0..3 {
        let port = format!("cnt{i}");
        d.add_port(&port, helion_ir::PortDir::Out);
        d.add_cell(&format!("u_iob_cnt{i}"), helion_ir::CellKind::IobOut);
        d.connect(&format!("q{i}"), &format!("u_iob_cnt{i}"), "I");
        d.connect(&port, &format!("u_iob_cnt{i}"), "PAD");
    }
    let _ = d.set_port_attr("led", "IOSTANDARD", "LVCMOS18");
    let _ = d.set_port_attr("led", "DRIVE", "8");
    let _ = d.set_port_attr("cnt0", "IOSTANDARD", "LVCMOS33");
    let _ = d.set_port_attr("cnt0", "DRIVE", "24");
    let _ = d.set_port_attr("cnt0", "SLEW", "FAST");
    let mut session = helion_proj::Session::new(helion_proj::Mode::Project);
    if session.impl_design(d, &dev).is_err() {
        return;
    }
    let mut xdc = helion_sta::Constraints::default();
    helion_sta::create_clock(&mut xdc.clocks, "clk", 4_000, "clk");
    let inp = IoInputs {
        design: session.design.clone().unwrap_or_else(|| helion_ir::Design::new("x")),
        placed: session.placed.clone(),
        part: dev.part.clone(),
        xdc,
    };
    let io = &mut s.reports_view.io;
    io.clock = Some(compute_clock(&inp));
    io.control = Some(compute_control(&inp));
    io.selected_ctrl = Some(0);
    io.ssn = Some(compute_ssn(&inp));
    io.selected_ssn = None;
    let path = std::env::temp_dir().join("helion-rune-scenes").join("counter.ibs");
    io.ibis = Some(compute_ibis(&inp, &path));
    s.reports_view.selected = nav;
}

pub const VERBS: &[&str] = &["report_clock_utilization", "report_control_sets", "report_ssn", "write_ibis"];

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::runs::{RunKind, start_run};
    use std::time::Duration;

    pub(crate) fn implemented_state() -> AppState {
        let mut s = AppState::demo();
        let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv");
        start_run(&mut s, RunKind::Impl, src, "HL10T-C32-1".into()).unwrap();
        let start = std::time::Instant::now();
        while s.runs.is_active() && start.elapsed() < Duration::from_secs(180) {
            crate::runs::poll_runs(&mut s);
            std::thread::sleep(Duration::from_millis(5));
        }
        crate::runs::poll_runs(&mut s);
        s
    }

    #[test]
    fn io_verbs_compute_real_reports_off_thread() {
        let mut s = implemented_state();
        s.runs.checkpoint_root = Some(std::env::temp_dir().join("helion-rune-test-io"));
        for v in ["report_clock_utilization", "report_control_sets", "report_ssn"] {
            tcl_verb(&mut s, v, &[]).unwrap();
        }
        crate::bg::wait_all(&mut s);
        let io = &s.reports_view.io;
        let clock = io.clock.as_ref().unwrap().as_ref().unwrap();
        assert!(!clock.domains.is_empty());
        let ctrl = io.control.as_ref().unwrap().as_ref().unwrap();
        assert!(ctrl.placed && ctrl.total_ffs() > 0);
        let ssn = io.ssn.as_ref().unwrap().as_ref().unwrap();
        assert!(!ssn.report.pins.is_empty());
        let path = std::env::temp_dir().join("helion-rune-test-io").join("counter.ibs");
        tcl_verb(&mut s, "write_ibis", &[path.display().to_string()]).unwrap();
        crate::bg::wait_all(&mut s);
        let ibis = s.reports_view.io.ibis.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(ibis.path, path);
        assert!(std::fs::read_to_string(&path).unwrap().contains("[IBIS Ver]"));
        assert_eq!(s.reports_view.selected, ReportNav::Ibis);
    }

    #[test]
    fn io_reports_without_a_run_store_the_reason() {
        let mut s = AppState::demo();
        assert!(tcl_verb(&mut s, "report_ssn", &[]).is_err());
        assert!(matches!(s.reports_view.io.ssn, Some(Err(_))));
        assert!(unavailable(&s, ReportNav::Ssn).is_some());
    }
}
