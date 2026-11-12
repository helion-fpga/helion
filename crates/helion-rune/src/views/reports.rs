//! Implementation reports: Utilization / Power / DRC detail bodies.
//!
//! New Rune `El` code written from scratch for the Vivado brief
//! (`/tmp/vivado-ref/lab3-Fig10.png` utilization hierarchy plus the
//! `Name | Used` table; `lab3-Fig11.png` left report tree beside wide
//! report tables): a center pane with a typed report-tree nav and, per
//! report, a summary card, a data table, and a histogram of `w_frac` bars.
//! All colors come from [`VivadoTheme`]; selection is the typed
//! [`ReportNav`] enum (no stringly status, no hardcoded hex, no manual
//! geometry, no unwraps on UI state).
//!
//! Data shapes mirror `helion-gui` read-only: occupancy rows mirror
//! `ide.rs::UtilOccupancy` (resource/used/available), the hierarchy table
//! mirrors `ide.rs::HierOccupancy` (`name/lut/ff/iob/bram/dsp`), power
//! block rows mirror `helion_sta::PowerReport` (`clocks/logic/signals/io/
//! bram/dsp_uw`), DRC rows mirror `helion_drc::DrcViolation`
//! (`id`/severity/objects/message), the signoff card mirrors
//! `helion_drc::BitgenSignoffReport`, the bitstream card mirrors the
//! `write_bitstream` log line, methodology rows mirror
//! `helion_sta::MethodologyViolation` via [`crate::state::TimingState`]
//! (`methodology`), and QoR rows mirror
//! `helion_proj::qor::QorAssessmentReport`. Every number comes from
//! [`AppState`] (`util`/`power`/`drc`/`bitstream`) as filled by real runs,
//! or from a real on-demand [`refresh_qor`] evaluation over the elaborated
//! design; empty sections render a disabled reason, never demo constants.
//!
//! QoR behavior mirrors `ide.rs` read-only: `qor_report` evaluates over the
//! session design, `refresh_qor` re-runs on demand, `select_qor` highlights
//! one suggestion, and the empty state reads like `qor_text`
//! ("no design - synth / report_qor_suggestions").
//!
//! Engine citations:
//! - `crates/helion-proj/src/qor.rs`: `report_qor_suggestions(&Session,
//!   &Device, &Constraints)`, `QorAssessmentReport::{text,rqa_score,...}`.
//! - `crates/helion-proj/src/lib.rs`: `Session::new(Mode::NonProject)`.
//! - `crates/helion-device/src/lib.rs`: `Device::load_part`.
//! - `crates/helion-sta/src/lib.rs`: `create_clock`, `Constraints::default`.
//! - `crates/helion-sv/src/lib.rs`: `synth_sv` (tests only, via fixtures).
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el.rs`: `row()/col()/el()/text()/icon()`, `ScrollSpec`.
//! - `crates/rune/src/el/scroll.rs`: `.scroll()` vertical viewport.
//! - `crates/rune/src/el/layout.rs`: `.w_frac()` histogram bars.
//! - `crates/rune/src/el/text.rs`: `.truncate()` for long labels.
//! - `crates/rune-kit/src/glyphs.rs`: `TRIANGLE_DOWN`, `CIRCLE_X`.

pub mod io;
pub mod vec_power;
mod panels;

use rune::{Align, El, ScrollSpec, col, el, icon, row, text};
use rune_kit::glyphs;

use crate::state::{AppState, ConsoleLine, DrcSev, MsgSev, RunStatus};
use crate::theme::VivadoTheme;
use crate::views::sources::pane_header;
use crate::views::summary::{format_util, format_watts, pct};

/// Report-tree width (the brief shows a narrow left nav beside wide tables).
const NAV_W: f32 = 220.0;
/// Report-tree row height.
const NAV_ROW_H: f32 = 24.0;
/// Summary `label: value` row height.
const ROW_H: f32 = 20.0;
/// Summary label width (fits "Total On-Chip Power:").
const LABEL_W: f32 = 136.0;
/// Table header band height.
const TABLE_HEADER_H: f32 = 22.0;
/// Table row height (compact CAD density).
const TABLE_ROW_H: f32 = 20.0;
/// Used/Available column widths (right-aligned counts).
const COUNT_W: f32 = 90.0;
/// Util/Share column width (right-aligned percents).
const PCT_W: f32 = 70.0;
/// Hierarchy metric column width (right-aligned counts).
const METRIC_W: f32 = 64.0;
/// DRC rule-id column width.
const RULE_W: f32 = 110.0;
/// DRC severity column width.
const SEV_W: f32 = 90.0;
/// Histogram row-label width.
const HIST_LABEL_W: f32 = 150.0;
/// Histogram bar height.
const BAR_H: f32 = 10.0;

/// Left report-tree entries. Typed so selection is never a string.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReportNav {
    #[default]
    Utilization,
    Power,
    Drc,
    Methodology,
    Qor,
    ClockUtil,
    ControlSets,
    Ssn,
    Ibis,
}

impl ReportNav {
    pub const ALL: [ReportNav; 9] = [
        ReportNav::Utilization,
        ReportNav::Power,
        ReportNav::Drc,
        ReportNav::Methodology,
        ReportNav::Qor,
        ReportNav::ClockUtil,
        ReportNav::ControlSets,
        ReportNav::Ssn,
        ReportNav::Ibis,
    ];

    /// Pages computed by [`io`] engines on a background worker.
    pub fn is_io(self) -> bool {
        matches!(
            self,
            ReportNav::ClockUtil | ReportNav::ControlSets | ReportNav::Ssn | ReportNav::Ibis
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            ReportNav::Utilization => "Utilization",
            ReportNav::Power => "Power",
            ReportNav::Drc => "DRC",
            ReportNav::Methodology => "Methodology",
            ReportNav::Qor => "QoR",
            ReportNav::ClockUtil => "Clock Utilization",
            ReportNav::ControlSets => "Control Sets",
            ReportNav::Ssn => "SSN",
            ReportNav::Ibis => "IBIS Model",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            ReportNav::Utilization => "reports-nav-utilization",
            ReportNav::Power => "reports-nav-power",
            ReportNav::Drc => "reports-nav-drc",
            ReportNav::Methodology => "reports-nav-methodology",
            ReportNav::Qor => "reports-nav-qor",
            ReportNav::ClockUtil => "reports-nav-clockutil",
            ReportNav::ControlSets => "reports-nav-ctrlsets",
            ReportNav::Ssn => "reports-nav-ssn",
            ReportNav::Ibis => "reports-nav-ibis",
        }
    }

    /// Pinned footer line naming the open report tab.
    fn footer(self) -> &'static str {
        match self {
            ReportNav::Utilization => "Utilization - utilization_1",
            ReportNav::Power => "Power - power_1",
            ReportNav::Drc => "DRC - drc_1",
            ReportNav::Methodology => "Methodology - methodology_1",
            ReportNav::Qor => "QoR Suggestions - qor_suggestions_1",
            ReportNav::ClockUtil => "Clock Utilization - clock_utilization_1",
            ReportNav::ControlSets => "Control Sets - control_sets_1",
            ReportNav::Ssn => "SSN - ssn_1",
            ReportNav::Ibis => "IBIS - write_ibis_1",
        }
    }
}

/// One owned QoR suggestion row, mirroring
/// `helion_proj::qor::QorSuggestion` (`id`/category/severity/`tcl_command`).
#[derive(Clone, Debug)]
pub struct QorSuggRow {
    pub id: String,
    pub category: String,
    pub severity: String,
    pub applicable_for: String,
    pub tcl_command: String,
    pub target: String,
    pub description: String,
    pub expected_gain_ps: i64,
}

/// Owned QoR assessment snapshot, mirroring
/// `helion_proj::qor::QorAssessmentReport` (RQA score, slacks, counts,
/// strategies, suggestions) plus the design/device it was assessed on.
#[derive(Clone, Debug, Default)]
pub struct QorSnapshot {
    pub rqa_score: u8,
    pub wns_ps: i64,
    pub tns_ps: i64,
    pub whs_ps: i64,
    pub endpoints: usize,
    pub lut_count: usize,
    pub ff_count: usize,
    pub iob_count: usize,
    pub lut_util_pct: u32,
    pub max_fanout: usize,
    pub high_fanout_net: String,
    pub overused_tiles: u32,
    pub current_strategy: String,
    pub recommended_strategy: String,
    pub device_part: String,
    pub design: String,
    pub suggestions: Vec<QorSuggRow>,
}

impl QorSnapshot {
    /// Suggestions at one engine severity (`Critical`/`Warning`/...
    /// as spelled by `report_qor_suggestions`).
    pub fn severity_count(&self, severity: &str) -> usize {
        self.suggestions
            .iter()
            .filter(|s| s.severity == severity)
            .count()
    }
}

/// UG906 RQA scale wording (5 = timing clean with margin, 1 = severe
/// violation, per `QorAssessmentReport::rqa_score`).
pub(crate) fn rqa_label(score: u8) -> &'static str {
    match score {
        5 => "Excellent",
        4 => "Good",
        3 => "Fair",
        2 => "Poor",
        1 => "Critical",
        _ => "Unknown",
    }
}

/// Reports center-pane UI state: which report-tree entry is shown, plus the
/// last on-demand QoR evaluation (or its failure reason) and the selected
/// suggestion id.
#[derive(Clone, Debug, Default)]
pub struct ReportsView {
    pub selected: ReportNav,
    pub qor: Option<QorSnapshot>,
    pub qor_error: Option<String>,
    pub selected_qor: Option<String>,
    /// Selected DRC row (index into [`crate::state::DrcState::items`]).
    pub selected_drc: Option<usize>,
    /// Selected methodology row (index into `TimingState::methodology`).
    pub selected_method: Option<usize>,
    /// Clock utilization / control sets / SSN / IBIS pages.
    pub io: io::IoReports,
    /// Vectored (VCD-driven) power under the Power report.
    pub vpower: vec_power::VecPower,
}

/// Fill the typed post-implementation fixture state the report scenes
/// share: completed runs, passing timing, utilization, and the power split
/// (block rows sum to the dynamic total), plus representative hierarchy,
/// signoff, and bitstream shaped like real run output. Leaves `center`
/// and `reports_view` to the caller.
pub fn demo_impl_complete(s: &mut AppState) {
    use crate::state::{BitstreamState, HierRow};
    s.flow.synth = RunStatus::Complete;
    s.flow.impl_run = RunStatus::Complete;
    s.status = crate::state::StatusVerb::ImplementationComplete;
    s.timing = crate::state::TimingState {
        wns_ps: Some(2061),
        tns_ps: 0,
        whs_ps: Some(154),
        ths_ps: 0,
        failing_endpoints: 0,
        failing_setup: 0,
        failing_hold: 0,
        endpoints: 112,
        has_run: true,
        design: "project1_demo".into(),
        ..crate::state::TimingState::default()
    };
    s.util = crate::state::UtilState {
        luts_used: 1523,
        luts_avail: 17600,
        regs_used: 892,
        regs_avail: 35200,
        bram_used: 2,
        bram_avail: 60,
        io_used: 24,
        io_avail: 100,
        dsp_used: 0,
        dsp_avail: 66,
        bufg_used: 1,
        bufg_avail: 32,
        hier: vec![
            HierRow {
                name: "project1_demo".into(),
                child: false,
                lut: 1523,
                ff: 892,
                io: 24,
                bram: 2,
                dsp: 0,
            },
            HierRow {
                name: "led_ctl0 (led_ctl)".into(),
                child: true,
                lut: 612,
                ff: 402,
                io: 8,
                bram: 0,
                dsp: 0,
            },
            HierRow {
                name: "divclk_gen".into(),
                child: true,
                lut: 88,
                ff: 96,
                io: 0,
                bram: 0,
                dsp: 0,
            },
            HierRow {
                name: "seg_dec".into(),
                child: true,
                lut: 214,
                ff: 96,
                io: 16,
                bram: 0,
                dsp: 0,
            },
        ],
        design: "project1_demo".into(),
        ..crate::state::UtilState::default()
    };
    s.power = crate::state::PowerState {
        total_mw: 142,
        dynamic_mw: 38,
        static_mw: 104,
        total_uw: 142_000,
        dynamic_uw: 38_000,
        static_uw: 104_000,
        clocks_uw: 12500,
        logic_uw: 8600,
        signals_uw: 9800,
        io_uw: 5200,
        bram_uw: 1900,
        dsp_uw: 0,
        junction_deci_c: 254,
        part: "HL10T-C32-1".into(),
        voltage_mv: 1000,
        temp_c: 25,
        f_mhz: 100,
        confidence: "Low".into(),
        confidence_note: "Vectorless estimate — no VCD switching activity".into(),
    };
    s.drc.bitgen_allowed = Some(true);
    s.drc.signoff_summary = "bitgen_signoff allowed=true errors=0 warnings=1".into();
    s.drc.has_run = true;
    s.bitstream = BitstreamState {
        idcode: 0x01EC_0001,
        hash: 0x9E37_42A1,
        bytes: 4096,
        frames: 32,
        configured: 28,
        has_run: true,
        ..BitstreamState::default()
    };
}

fn ui_label(content: String, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text)
}

fn dim_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text_dim)
}

/// Echo a Tcl verb to the console without flipping run state.
fn echo(s: &mut AppState, tcl: String) {
    s.console.lines.push(ConsoleLine {
        text: tcl,
        sev: MsgSev::Info,
    });
}

/// Select a report-tree entry. Selecting QoR re-evaluates the assessment
/// (mirroring the GUI `refresh_qor` on-demand behavior) so the body never
/// shows a stale design's numbers.
fn select_report(s: &mut AppState, nav: ReportNav) {
    s.reports_view.selected = nav;
    if nav == ReportNav::Qor {
        refresh_qor(s);
    }
    if nav.is_io() {
        io::on_select(s, nav);
    }
}

/// Fallback part when the project part has no Helion HAD model, mirroring
/// `runs.rs::resolve_device` (loud warning, never silent).
const FALLBACK_PART: &str = "HL10T-C32-1";

/// Implicit analysis clock when the run produced no clocks, mirroring
/// `runs.rs::IMPLICIT_CLOCK_PS`.
const IMPLICIT_CLOCK_PS: u64 = 10_000;

/// Real QoR assessment over the elaborated session design (the `tcl.design`
/// from `read_sv`/`synth_design`, else the `launch_runs` worker design):
/// device model, analysis clocks, then
/// `helion_proj::qor::report_qor_suggestions`. Returns the snapshot plus an
/// optional fallback-device note the caller echoes loudly.
fn compute_qor_report(s: &AppState) -> Result<(QorSnapshot, Option<String>), String> {
    let design = s
        .tcl
        .design
        .clone()
        .or_else(|| s.runs.live_design.clone())
        .ok_or_else(|| {
            "no design - synth / report_qor_suggestions (run Synthesis or read_sv first)"
                .to_string()
        })?;
    let design_name = helion_ir::resolve(design.name).to_string();
    let requested = if s.project.part.is_empty() {
        FALLBACK_PART
    } else {
        s.project.part.as_str()
    };
    let (dev, device_part, fallback_note) = match helion_device::Device::load_part(requested) {
        Ok(dev) => (dev, requested.to_string(), None),
        Err(first) if requested != FALLBACK_PART => {
            let dev = helion_device::Device::load_part(FALLBACK_PART).map_err(|e| {
                format!("no Helion device model (tried {requested}, {FALLBACK_PART}): {e}")
            })?;
            let note = format!(
                "part {requested} has no Helion HAD model ({first}); using {FALLBACK_PART}"
            );
            (dev, FALLBACK_PART.to_string(), Some(note))
        }
        Err(e) => return Err(format!("no Helion device model for part {requested}: {e}")),
    };
    let mut xdc = helion_sta::Constraints::default();
    for clock in &s.timing.clocks {
        helion_sta::create_clock(&mut xdc.clocks, &clock.name, clock.period_ps, &clock.source);
    }
    if xdc.clocks.is_empty() {
        helion_sta::create_clock(&mut xdc.clocks, "clk", IMPLICIT_CLOCK_PS, "clk");
    }
    let mut session = helion_proj::Session::new(helion_proj::Mode::NonProject);
    session.synth_design(design);
    let report = helion_proj::qor::report_qor_suggestions(&session, &dev, &xdc)?;
    Ok((
        QorSnapshot {
            rqa_score: report.rqa_score,
            wns_ps: report.wns_ps,
            tns_ps: report.tns_ps,
            whs_ps: report.whs_ps,
            endpoints: report.endpoints,
            lut_count: report.lut_count,
            ff_count: report.ff_count,
            iob_count: report.iob_count,
            lut_util_pct: report.lut_util_pct,
            max_fanout: report.max_fanout,
            high_fanout_net: report.high_fanout_net,
            overused_tiles: report.overused_tiles,
            current_strategy: report.current_strategy.label().into(),
            recommended_strategy: report.recommended_strategy.label().into(),
            device_part,
            design: design_name,
            suggestions: report
                .suggestions
                .iter()
                .map(|g| QorSuggRow {
                    id: g.id.clone(),
                    category: g.category.as_str().into(),
                    severity: g.severity.clone(),
                    applicable_for: g.applicable_for.clone(),
                    tcl_command: g.tcl_command.clone(),
                    target: g.target.clone(),
                    description: g.description.clone(),
                    expected_gain_ps: g.expected_gain_ps,
                })
                .collect(),
        },
        fallback_note,
    ))
}

/// Re-run the QoR assessment on demand (report-tree tap, Refresh link,
/// Summary QoR link): store the snapshot or the failure reason and echo the
/// console trail. A failed evaluation clears the previous snapshot so stale
/// numbers never stay mounted.
pub(crate) fn refresh_qor(s: &mut AppState) {
    match compute_qor_report(s) {
        Ok((snap, note)) => {
            if let Some(n) = note {
                s.console.lines.push(ConsoleLine {
                    text: format!("WARN: {n}"),
                    sev: MsgSev::Warn,
                });
            }
            let critical = snap.severity_count("Critical");
            let warning = snap.severity_count("Warning");
            echo(
                s,
                format!(
                    "report_qor_suggestions refreshed RQA_SCORE={}/5 suggestions={} critical={} warning={}",
                    snap.rqa_score,
                    snap.suggestions.len(),
                    critical,
                    warning,
                ),
            );
            s.reports_view.qor = Some(snap);
            s.reports_view.qor_error = None;
        }
        Err(e) => {
            s.reports_view.qor = None;
            s.reports_view.qor_error = Some(e.clone());
            s.console.lines.push(ConsoleLine {
                text: format!("ERROR: report_qor_suggestions failed: {e}"),
                sev: MsgSev::Error,
            });
        }
    }
}

/// Microwatts to Vivado-style `"12.500 mW"` text.
pub fn format_mw(uw: u32) -> String {
    format!("{:.3} mW", uw as f64 / 1000.0)
}

/// Deci-°C to Vivado-style `"25.4 °C"` text.
pub fn format_temp(deci_c: u32) -> String {
    format!("{:.1} °C", deci_c as f64 / 10.0)
}

/// One report-tree row, with a red alert on DRC while errors exist.
fn nav_row(nav: ReportNav, s: &AppState, t: &VivadoTheme) -> El {
    let selected = s.reports_view.selected == nav;
    let mut content = row().items_center().gap(4.0);
    content = content.child(ui_label(nav.label().into(), t).truncate());
    if nav == ReportNav::Drc && s.drc.items.iter().any(|v| v.severity == DrcSev::Error) {
        content = content.child(icon(&glyphs::CIRCLE_X, 12.0).color(t.fail).a11y_hidden());
    }
    if nav == ReportNav::Qor
        && s.reports_view
            .qor
            .as_ref()
            .is_some_and(|q| q.severity_count("Critical") > 0)
    {
        content = content.child(icon(&glyphs::CIRCLE_X, 12.0).color(t.fail).a11y_hidden());
    }
    let mut item = el()
        .key(nav.key())
        .role("button")
        .label(format!("Implementation report: {}", nav.label()))
        .focusable(true)
        .w_full()
        .h(NAV_ROW_H)
        .px(8.0)
        .center_y()
        .hover(t.selection)
        .on_press(move |s: &mut AppState| select_report(s, nav))
        .child(content);
    if selected {
        item = item.fill(t.selection);
    }
    item
}

/// One `label: value` summary row.
fn kv(label_text: &'static str, value: El, t: &VivadoTheme) -> El {
    row()
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(6.0)
        .child(el().w(LABEL_W).child(dim_label(label_text, t).truncate()))
        .child(value)
}

/// One histogram row: label, `w_frac` bar, and value text.
fn hist_row(
    key: String,
    label_text: impl Into<String>,
    frac: f32,
    value_text: String,
    bar_color: rune::Color,
    t: &VivadoTheme,
) -> El {
    row()
        .key(key)
        .w_full()
        .h(TABLE_ROW_H)
        .items_center()
        .gap(8.0)
        .child(
            el().w(HIST_LABEL_W)
                .child({
                    let l: String = label_text.into();
                    dim_label(&l, t).truncate().help_when_truncated(l.clone())
                }),
        )
        .child(
            el().grow(1.0)
                .min_w(0.0)
                .h(BAR_H)
                .fill(t.border_light)
                .child(el().w_frac(frac.clamp(0.0, 1.0)).h_full().fill(bar_color)),
        )
        .child(
            el().w(PCT_W).child(
                text(value_text)
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text)
                    .w_full()
                    .align(Align::End),
            ),
        )
}

/// Occupancy rows from the typed [`crate::state::UtilState`] (family
/// primitive names for iCE40/ECP5 parts).
fn occupancy(s: &AppState) -> Vec<(String, u32, u32)> {
    s.util.rows()
}

/// Utilization occupancy table: `Resource | Used | Available | Util (%)`.
fn util_table(s: &AppState, t: &VivadoTheme) -> El {
    let header_cell = |content: &'static str| -> El {
        text(content)
            .font(VivadoTheme::FONT_UI)
            .color(t.text_dim)
            .truncate()
    };
    let mut body = col().w_full();
    for (index, (name, used, avail)) in occupancy(s).into_iter().enumerate() {
        body = body.child(
            row()
                .key(format!("reports-util-row-{index}"))
                .w_full()
                .h(TABLE_ROW_H)
                .items_center()
                .gap(8.0)
                .px(8.0)
                .child(ui_label(name.clone(), t).grow(2.0).min_w(0.0).truncate().help_when_truncated(name.clone()))
                .child(
                    el().w(COUNT_W).child(
                        text(used.to_string())
                            .font(VivadoTheme::FONT_UI)
                            .color(t.text)
                            .w_full()
                            .align(Align::End),
                    ),
                )
                .child(
                    el().w(COUNT_W).child(
                        text(avail.to_string())
                            .font(VivadoTheme::FONT_UI)
                            .color(t.text)
                            .w_full()
                            .align(Align::End),
                    ),
                )
                .child(
                    el().w(PCT_W).child(
                        text(format!("{}%", pct(used, avail)))
                            .font(VivadoTheme::FONT_UI)
                            .color(t.text)
                            .w_full()
                            .align(Align::End),
                    ),
                ),
        );
    }
    col()
        .key("reports-util-table")
        .w_full()
        .gap(2.0)
        .child(
            text("Site Usage")
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text),
        )
        .child(
            col()
                .w_full()
                .fill(t.panel_bg)
                .border(1.0, t.border)
                .child(
                    row()
                        .w_full()
                        .h(TABLE_HEADER_H)
                        .items_center()
                        .gap(8.0)
                        .px(8.0)
                        .fill(t.header_bg)
                        .child(header_cell("Resource").grow(2.0).min_w(0.0))
                        .child(crate::views::report_kit::header_num("Used", COUNT_W, t))
                        .child(crate::views::report_kit::header_num(
                            "Available",
                            COUNT_W,
                            t,
                        ))
                        .child(crate::views::report_kit::header_num("Util (%)", PCT_W, t)),
                )
                .child(body),
        )
}

/// Utilization histogram: one bar per resource, fraction of available.
fn util_hist(s: &AppState, t: &VivadoTheme) -> El {
    let mut hist = col().key("reports-util-hist").w_full().gap(2.0).child(
        text("Utilization by Resource")
            .font(VivadoTheme::FONT_TITLE)
            .color(t.text),
    );
    for (index, (name, used, avail)) in occupancy(s).into_iter().enumerate() {
        let frac = if avail == 0 {
            0.0
        } else {
            used as f32 / avail as f32
        };
        hist = hist.child(hist_row(
            format!("reports-util-bar-{index}"),
            name,
            frac,
            format_util(used, avail),
            t.accent,
            t,
        ));
    }
    hist
}

/// Utilization hierarchy table: `Name | LUT | FF | IO | BRAM | DSP`
/// from the run's real occupancy. Empty until a run counts the design.
fn hier_table(s: &AppState, t: &VivadoTheme) -> El {
    if s.util.hier.is_empty() {
        return col()
            .key("reports-hier")
            .w_full()
            .gap(2.0)
            .child(
                text("Hierarchy")
                    .font(VivadoTheme::FONT_TITLE)
                    .color(t.text),
            )
            .child(
                row()
                    .key("reports-hier-empty")
                    .w_full()
                    .h(TABLE_ROW_H)
                    .items_center()
                    .px(8.0)
                    .child(dim_label(
                        "No hierarchy — run Synthesis to count the design.",
                        t,
                    )),
            );
    }
    let header_cell = |content: &'static str| -> El {
        text(content)
            .font(VivadoTheme::FONT_UI)
            .color(t.text_dim)
            .truncate()
    };
    let metric = |value: u32, t: &VivadoTheme| -> El {
        el().w(METRIC_W).child(
            text(value.to_string())
                .font(VivadoTheme::FONT_UI)
                .color(t.text)
                .w_full()
                .align(Align::End),
        )
    };
    let mut body = col().w_full();
    for (index, hier) in s.util.hier.iter().enumerate() {
        let mut name = row().items_center().gap(4.0);
        if hier.child {
            name = name.child(el().w(12.0));
        } else {
            name = name.child(
                icon(&glyphs::TRIANGLE_DOWN, 9.0)
                    .color(t.text_dim)
                    .a11y_hidden(),
            );
        }
        name = name.child(ui_label(hier.name.clone(), t).truncate());
        body = body.child(
            row()
                .key(format!("reports-hier-row-{index}"))
                .w_full()
                .h(TABLE_ROW_H)
                .items_center()
                .gap(8.0)
                .px(8.0)
                .child(el().grow(3.0).min_w(0.0).child(name))
                .child(metric(hier.lut, t))
                .child(metric(hier.ff, t))
                .child(metric(hier.io, t))
                .child(metric(hier.bram, t))
                .child(metric(hier.dsp, t)),
        );
    }
    col()
        .key("reports-hier")
        .w_full()
        .gap(2.0)
        .child(
            text("Hierarchy")
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text),
        )
        .child(
            col()
                .w_full()
                .fill(t.panel_bg)
                .border(1.0, t.border)
                .child(
                    row()
                        .w_full()
                        .h(TABLE_HEADER_H)
                        .items_center()
                        .gap(8.0)
                        .px(8.0)
                        .fill(t.header_bg)
                        .child(header_cell("Name").grow(3.0).min_w(0.0))
                        .child(crate::views::report_kit::header_num("LUT", METRIC_W, t))
                        .child(crate::views::report_kit::header_num("FF", METRIC_W, t))
                        .child(crate::views::report_kit::header_num("IO", METRIC_W, t))
                        .child(crate::views::report_kit::header_num("BRAM", METRIC_W, t))
                        .child(crate::views::report_kit::header_num("DSP", METRIC_W, t)),
                )
                .child(body),
        )
}

/// Utilization detail body: site table, histogram, hierarchy table.
fn util_body(s: &AppState, t: &VivadoTheme) -> El {
    col()
        .key("reports-util")
        .w_full()
        .gap(10.0)
        .child(
            text(format!("Utilization ({})", s.project.part))
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text)
                .key("reports-util-title"),
        )
        .child(panels::kpi_cards(&occupancy(s), t))
        .child(util_table(s, t))
        .child(util_hist(s, t))
        .child(hier_table(s, t))
}

/// Power summary card: `report_power` part/PVT header, on-chip split,
/// junction temperature, and the run-derived confidence line.
fn power_summary(s: &AppState, t: &VivadoTheme) -> El {
    let p = &s.power;
    let confidence = if p.confidence.is_empty() {
        "n/a".into()
    } else {
        p.confidence.clone()
    };
    let part = if p.part.is_empty() {
        "n/a".into()
    } else {
        p.part.clone()
    };
    let voltage = if p.voltage_mv == 0 {
        "n/a".into()
    } else {
        format!("{} mV", p.voltage_mv)
    };
    let temp = if p.temp_c == 0 {
        "n/a".into()
    } else {
        format!("{} °C", p.temp_c)
    };
    col()
        .key("reports-power-summary")
        .w_full()
        .gap(2.0)
        .child(text("Summary").font(VivadoTheme::FONT_TITLE).color(t.text))
        .child(
            col().w_full().fill(t.panel_bg).border(1.0, t.border).child(
                col()
                    .w_full()
                    .padding(8.0)
                    .gap(2.0)
                    .child(kv("Part:", ui_label(part, t).key("reports-power-part"), t))
                    .child(kv(
                        "Voltage:",
                        ui_label(voltage, t).key("reports-power-voltage"),
                        t,
                    ))
                    .child(kv(
                        "Temperature:",
                        ui_label(temp, t).key("reports-power-temp"),
                        t,
                    ))
                    .child(kv(
                        "Total On-Chip Power:",
                        ui_label(format_watts(p.total_mw), t).key("reports-power-total"),
                        t,
                    ))
                    .child(kv(
                        "Dynamic:",
                        ui_label(format_watts(p.dynamic_mw), t).key("reports-power-dynamic"),
                        t,
                    ))
                    .child(kv(
                        "Device Static:",
                        ui_label(format_watts(p.static_mw), t).key("reports-power-static"),
                        t,
                    ))
                    .child(kv(
                        "Junction Temp:",
                        ui_label(format_temp(p.junction_deci_c), t).key("reports-power-junction"),
                        t,
                    ))
                    .child(kv(
                        "Analysis Freq:",
                        ui_label(
                            if p.f_mhz > 0 {
                                format!("{} MHz", p.f_mhz)
                            } else {
                                "n/a".into()
                            },
                            t,
                        )
                        .key("reports-power-freq"),
                        t,
                    ))
                    .child(kv(
                        "Confidence:",
                        ui_label(confidence, t).key("reports-power-confidence"),
                        t,
                    ))
                    .child(dim_label(&p.confidence_note, t).key("reports-power-confidence-note")),
            ),
        )
}

/// Dynamic-vs-static split bar: two `w_frac` segments of the total.
fn power_split(s: &AppState, t: &VivadoTheme) -> El {
    let total = s.power.total_mw.max(1) as f32;
    col()
        .key("reports-power-split")
        .w_full()
        .gap(2.0)
        .child(
            text("On-Chip Power")
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text),
        )
        .child(
            row()
                .w_full()
                .h(14.0)
                .fill(t.border_light)
                .child(
                    el().key("reports-power-split-dynamic")
                        .w_frac(s.power.dynamic_mw as f32 / total)
                        .h_full()
                        .fill(t.accent),
                )
                .child(
                    el().key("reports-power-split-static")
                        .w_frac(s.power.static_mw as f32 / total)
                        .h_full()
                        .fill(t.text_dim),
                ),
        )
        .child(
            row().w_full().h(ROW_H).items_center().gap(8.0).child(
                dim_label(
                    &format!(
                        "Dynamic {} / Static {}",
                        format_watts(s.power.dynamic_mw),
                        format_watts(s.power.static_mw),
                    ),
                    t,
                )
                .key("reports-power-split-label"),
            ),
        )
}

/// Power detail body: summary, split bar, rails table, block power and
/// utilization.
fn power_body(s: &AppState, t: &VivadoTheme) -> El {
    let vectored = s.reports_view.vpower.show;
    let tabs = row()
        .key("reports-power-tabs")
        .w_full()
        .gap(1.0)
        .fill(t.border)
        .child(crate::views::cad::tab(
            "reports-power-tab-vectorless",
            "Vectorless (STA)",
            !vectored,
            t,
            |s: &mut AppState| s.reports_view.vpower.show = false,
        ))
        .child(crate::views::cad::tab(
            "reports-power-tab-vectored",
            "Simulation activity (VCD)",
            vectored,
            t,
            |s: &mut AppState| s.reports_view.vpower.show = true,
        ))
        .child(
            row()
                .grow(1.0)
                .min_w(0.0)
                .h(24.0)
                .px(8.0)
                .items_center()
                .fill(t.header_bg)
                .child(el().grow(1.0))
                .child(
                    text(format!("Power ({})", s.project.part))
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text)
                        .truncate()
                        .key("reports-power-title"),
                ),
        );
    let body = col()
        .key("reports-power")
        .w_full()
        .gap(10.0)
        .child(tabs);
    if vectored {
        return body.child(vec_power::section(s, t));
    }
    body.child(power_summary(s, t))
        .child(power_split(s, t))
        .child(panels::rails_table(s, t))
        .child(panels::blocks_table(s, t))
}

/// Counts of `(errors, warnings, advisories)` folded from typed severities.
fn drc_counts(s: &AppState) -> (usize, usize, usize) {
    let mut counts = (0, 0, 0);
    for item in &s.drc.items {
        match item.severity {
            DrcSev::Error => counts.0 += 1,
            DrcSev::Warning => counts.1 += 1,
            DrcSev::Advisory => counts.2 += 1,
        }
    }
    counts
}

/// Severity ink for one DRC row.
fn sev_color(sev: DrcSev, t: &VivadoTheme) -> rune::Color {
    match sev {
        DrcSev::Error => t.fail,
        DrcSev::Warning => t.warn,
        DrcSev::Advisory => t.text,
    }
}

/// DRC severity histogram: one bar per severity with its count.
fn drc_hist(s: &AppState, t: &VivadoTheme) -> El {
    let (errors, warns, advisories) = drc_counts(s);
    let max = errors.max(warns).max(advisories).max(1) as f32;
    let rows = [
        ("reports-drc-bar-errors", "Errors", errors, t.fail),
        ("reports-drc-bar-warnings", "Warnings", warns, t.warn),
        (
            "reports-drc-bar-advisories",
            "Advisories",
            advisories,
            t.text_dim,
        ),
    ];
    let mut hist = col().key("reports-drc-hist").w_full().gap(2.0).child(
        text("Violations by Severity")
            .font(VivadoTheme::FONT_TITLE)
            .color(t.text),
    );
    for (key, label_text, count, color) in rows {
        hist = hist.child(hist_row(
            key.into(),
            label_text,
            count as f32 / max,
            count.to_string(),
            color,
            t,
        ));
    }
    hist
}

/// DRC violations table: `Rule | Severity | Objects | Message`. Rows
/// select; the detail panel below shows the full message and fix hint.
fn drc_table(s: &AppState, t: &VivadoTheme) -> El {
    use crate::views::report_kit as kit;
    let head = kit::header_row(t)
        .child(el().w(RULE_W).child(kit::header_cell("Rule", t)))
        .child(el().w(SEV_W).child(kit::header_cell("Severity", t)))
        .child(kit::header_cell("Objects", t).grow(2.0).min_w(0.0))
        .child(kit::header_cell("Message", t).grow(3.0).min_w(0.0));
    let mut body = col().w_full();
    for (index, item) in s.drc.items.iter().enumerate() {
        body = body.child(
            kit::select_row(
                format!("reports-drc-row-{index}"),
                format!("DRC {} {}", item.id, item.severity.label()),
                s.reports_view.selected_drc == Some(index),
                t,
                move |s: &mut AppState| s.reports_view.selected_drc = Some(index),
            )
            .child(kit::fixed_cell(item.id.clone(), RULE_W, t.link))
            .child(kit::fixed_cell(
                item.severity.label(),
                SEV_W,
                sev_color(item.severity, t),
            ))
            .child(kit::text_cell(item.objects.clone(), 2.0, t.text))
            .child(kit::text_cell(item.message.clone(), 3.0, t.text)),
        );
    }
    col()
        .key("reports-drc-table")
        .w_full()
        .gap(2.0)
        .child(kit::section_title("Violations", t))
        .child(kit::table(head, body, t))
        .child(el().w_full().pt(6.0).child(panels::drc_detail(s, t)))
}

/// DRC detail body: verdict, severity histogram, violations table.
fn drc_body(s: &AppState, t: &VivadoTheme) -> El {
    let (errors, warns, advisories) = drc_counts(s);
    let total = s.drc.items.len();
    let plural = |n: usize, one: &str, many: &str| -> String {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    let verdict = if total == 0 {
        text("No DRC violations.")
            .font(VivadoTheme::FONT_TITLE)
            .color(t.pass)
            .key("reports-drc-verdict")
    } else {
        text(format!(
            "{} ({}, {}, {})",
            plural(total, "DRC violation", "DRC violations"),
            plural(errors, "Error", "Errors"),
            plural(warns, "Warning", "Warnings"),
            plural(advisories, "Advisory", "Advisories"),
        ))
        .font(VivadoTheme::FONT_TITLE)
        .color(if errors > 0 { t.fail } else { t.warn })
        .key("reports-drc-verdict")
    };
    let mut body = col()
        .key("reports-drc")
        .w_full()
        .gap(10.0)
        .child(
            text(format!("DRC ({})", s.project.part))
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text)
                .key("reports-drc-title"),
        )
        .child(verdict)
        .child(drc_hist(s, t));
    if !s.drc.items.is_empty() {
        body = body.child(drc_table(s, t));
    }
    body = body.child(signoff_card(s, t)).child(bitstream_card(s, t));
    body
}

/// Bitstream signoff card: the `gate_write_bitstream` verdict plus its
/// one-line summary. Unevaluated until an implementation run signs off.
fn signoff_card(s: &AppState, t: &VivadoTheme) -> El {
    let verdict = match s.drc.bitgen_allowed {
        Some(true) => text("Bitstream generation allowed.")
            .font(VivadoTheme::FONT_TITLE)
            .color(t.pass),
        Some(false) => text("Bitstream generation blocked by DRC errors.")
            .font(VivadoTheme::FONT_TITLE)
            .color(t.fail),
        None => text("Signoff not evaluated — run Implementation.")
            .font(VivadoTheme::FONT_TITLE)
            .color(t.text_dim),
    }
    .key("reports-drc-signoff-verdict");
    let mut card = col()
        .key("reports-drc-signoff")
        .w_full()
        .gap(2.0)
        .child(
            text("Bitstream Signoff")
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text),
        )
        .child(verdict);
    if !s.drc.signoff_summary.is_empty() {
        card = card
            .child(ui_label(s.drc.signoff_summary.clone(), t).key("reports-drc-signoff-summary"));
    }
    if s.drc.bitgen_allowed.is_some() {
        card = card.child(
            dim_label(
                &format!(
                    "Unrouted nets: {} · Overused tiles: {}",
                    s.drc.unrouted, s.drc.overused
                ),
                t,
            )
            .key("reports-drc-signoff-counts"),
        );
    }
    card
}

/// Bitstream card: the `write_bitstream` result (idcode/hash/bytes/frames
/// from the real packet stream). Absent until a run writes one.
fn bitstream_card(s: &AppState, t: &VivadoTheme) -> El {
    if !s.bitstream.has_run {
        return col()
            .key("reports-bitstream")
            .w_full()
            .gap(2.0)
            .child(
                text("Bitstream")
                    .font(VivadoTheme::FONT_TITLE)
                    .color(t.text),
            )
            .child(
                dim_label("No bitstream — implementation writes one after routing.", t)
                    .key("reports-bitstream-empty"),
            );
    }
    let b = &s.bitstream;
    col()
        .key("reports-bitstream")
        .w_full()
        .gap(2.0)
        .child(
            text("Bitstream")
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text),
        )
        .child(
            col().w_full().fill(t.panel_bg).border(1.0, t.border).child(
                col()
                    .w_full()
                    .padding(8.0)
                    .gap(2.0)
                    .child(kv(
                        "IDCODE:",
                        ui_label(format!("{:#010x}", b.idcode), t).key("reports-bitstream-idcode"),
                        t,
                    ))
                    .child(kv(
                        "Hash:",
                        ui_label(format!("{:#010x}", b.hash), t).key("reports-bitstream-hash"),
                        t,
                    ))
                    .child(kv(
                        "Bytes:",
                        ui_label(b.bytes.to_string(), t).key("reports-bitstream-bytes"),
                        t,
                    ))
                    .child(kv(
                        "Frames:",
                        ui_label(format!("{} ({} configured)", b.frames, b.configured), t)
                            .key("reports-bitstream-frames"),
                        t,
                    ))
                    .child(
                        el().key("reports-bitstream-program")
                            .role("button")
                            .label("Open Hardware Manager")
                            .focusable(true)
                            .h(ROW_H)
                            .px(2.0)
                            .center_y()
                            .hover(t.selection)
                            .on_press(|s: &mut AppState| {
                                s.navigate(crate::state::CenterPane::Program);
                                echo(s, "open_hw_manager".into());
                            })
                            .child(
                                text("Open Hardware Manager")
                                    .font(VivadoTheme::FONT_UI)
                                    .color(t.link),
                            ),
                    ),
            ),
        )
}

/// Methodology severity ink, mirroring the timing pane's check table
/// (`Error`/`CriticalWarning` red, `Warning` amber, else ink).
fn method_sev_color(severity: &str, t: &VivadoTheme) -> rune::Color {
    match severity {
        "Error" | "CriticalWarning" => t.fail,
        "Warning" => t.warn,
        _ => t.text,
    }
}

/// Methodology histogram: errors (`Error` + `CriticalWarning`), warnings,
/// and advisories from the real `report_methodology` rows.
fn method_hist(s: &AppState, t: &VivadoTheme) -> El {
    let mut errors = 0;
    let mut warns = 0;
    let mut advisories = 0;
    for v in &s.timing.methodology {
        match v.severity.as_str() {
            "Error" | "CriticalWarning" => errors += 1,
            "Warning" => warns += 1,
            _ => advisories += 1,
        }
    }
    let max = errors.max(warns).max(advisories).max(1) as f32;
    let rows = [
        ("reports-method-bar-errors", "Errors", errors, t.fail),
        ("reports-method-bar-warnings", "Warnings", warns, t.warn),
        (
            "reports-method-bar-advisories",
            "Advisories",
            advisories,
            t.text_dim,
        ),
    ];
    let mut hist = col().key("reports-method-hist").w_full().gap(2.0).child(
        text("Checks by Severity")
            .font(VivadoTheme::FONT_TITLE)
            .color(t.text),
    );
    for (key, label_text, count, color) in rows {
        hist = hist.child(hist_row(
            key.into(),
            label_text,
            count as f32 / max,
            count.to_string(),
            color,
            t,
        ));
    }
    hist
}

/// Methodology checks table: `Check | Severity | Category | Objects |
/// Message` from the real run rows. Rows select; the detail panel below
/// shows the full message, objects, and the catalog fix hint.
fn method_table(s: &AppState, t: &VivadoTheme) -> El {
    use crate::views::report_kit as kit;
    let head = kit::header_row(t)
        .child(el().w(RULE_W).child(kit::header_cell("Check", t)))
        .child(el().w(SEV_W).child(kit::header_cell("Severity", t)))
        .child(el().w(SEV_W).child(kit::header_cell("Category", t)))
        .child(kit::header_cell("Objects", t).grow(2.0).min_w(0.0))
        .child(kit::header_cell("Message", t).grow(3.0).min_w(0.0));
    let mut body = col().w_full();
    for (index, v) in s.timing.methodology.iter().enumerate() {
        body = body.child(
            kit::select_row(
                format!("reports-method-row-{index}"),
                format!("Methodology {} {}", v.id, v.severity),
                s.reports_view.selected_method == Some(index),
                t,
                move |s: &mut AppState| s.reports_view.selected_method = Some(index),
            )
            .child(kit::fixed_cell(v.id.clone(), RULE_W, t.text))
            .child(kit::fixed_cell(
                v.severity.clone(),
                SEV_W,
                method_sev_color(&v.severity, t),
            ))
            .child(kit::fixed_cell(v.category.clone(), SEV_W, t.text))
            .child(kit::text_cell(v.objects.clone(), 2.0, t.text))
            .child(kit::text_cell(v.message.clone(), 3.0, t.text)),
        );
    }
    col()
        .key("reports-method-table")
        .w_full()
        .gap(2.0)
        .child(kit::section_title("Checks", t))
        .child(kit::table(head, body, t))
        .child(el().w_full().pt(6.0).child(panels::method_detail(s, t)))
}

/// Methodology detail body: verdict, severity histogram, checks table.
/// Empty until a real run analyzes the constraints.
fn method_body(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col().key("reports-method").w_full().gap(10.0).child(
        text(format!("Methodology ({})", s.project.part))
            .font(VivadoTheme::FONT_TITLE)
            .color(t.text)
            .key("reports-method-title"),
    );
    if s.timing.methodology.is_empty() {
        let reason = if s.timing.has_run {
            "No methodology violations reported."
        } else {
            "No methodology results — run Implementation to analyze constraints."
        };
        return body.child(
            row()
                .key("reports-method-empty")
                .w_full()
                .h(TABLE_ROW_H)
                .items_center()
                .px(8.0)
                .child(dim_label(reason, t)),
        );
    }
    let mut errors = 0;
    let mut warns = 0;
    let mut advisories = 0;
    for v in &s.timing.methodology {
        match v.severity.as_str() {
            "Error" | "CriticalWarning" => errors += 1,
            "Warning" => warns += 1,
            _ => advisories += 1,
        }
    }
    let total = s.timing.methodology.len();
    let plural = |n: usize, one: &str, many: &str| -> String {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    body = body
        .child(
            text(format!(
                "{} ({}, {}, {})",
                plural(total, "methodology check", "methodology checks"),
                plural(errors, "Error", "Errors"),
                plural(warns, "Warning", "Warnings"),
                plural(advisories, "Advisory", "Advisories"),
            ))
            .font(VivadoTheme::FONT_TITLE)
            .color(if errors > 0 {
                t.fail
            } else if warns > 0 {
                t.warn
            } else {
                t.text
            })
            .key("reports-method-verdict"),
        )
        .child(method_hist(s, t))
        .child(method_table(s, t));
    body
}

/// RQA score ink: red at 1-2, amber at 3, green at 4-5.
pub(crate) fn rqa_color(score: u8, t: &VivadoTheme) -> rune::Color {
    match score {
        1 | 2 => t.fail,
        3 => t.warn,
        _ => t.pass,
    }
}

/// QoR suggestion severity ink, mirroring the DRC row inks.
fn qor_sev_color(severity: &str, t: &VivadoTheme) -> rune::Color {
    match severity {
        "Critical" => t.fail,
        "Warning" => t.warn,
        _ => t.text,
    }
}

/// QoR assessment summary card: RQA score, slacks, counts, fanout, and the
/// current versus recommended implementation strategy.
fn qor_summary(snap: &QorSnapshot, t: &VivadoTheme) -> El {
    let fanout = if snap.high_fanout_net.is_empty() {
        snap.max_fanout.to_string()
    } else {
        format!("{} ({})", snap.max_fanout, snap.high_fanout_net)
    };
    col()
        .key("reports-qor-summary")
        .w_full()
        .gap(2.0)
        .child(
            text("Assessment")
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text),
        )
        .child(
            col().w_full().fill(t.panel_bg).border(1.0, t.border).child(
                col()
                    .w_full()
                    .padding(8.0)
                    .gap(2.0)
                    .child(kv(
                        "RQA Score:",
                        text(format!(
                            "{}/5 ({})",
                            snap.rqa_score,
                            rqa_label(snap.rqa_score)
                        ))
                        .font(VivadoTheme::FONT_UI)
                        .color(rqa_color(snap.rqa_score, t))
                        .key("reports-qor-score"),
                        t,
                    ))
                    .child(kv(
                        "WNS / TNS / WHS:",
                        ui_label(
                            format!(
                                "{} / {} / {}",
                                crate::views::summary::format_ns(snap.wns_ps),
                                crate::views::summary::format_ns(snap.tns_ps),
                                crate::views::summary::format_ns(snap.whs_ps),
                            ),
                            t,
                        )
                        .key("reports-qor-slacks"),
                        t,
                    ))
                    .child(kv(
                        "Endpoints:",
                        ui_label(snap.endpoints.to_string(), t).key("reports-qor-endpoints"),
                        t,
                    ))
                    .child(kv(
                        "LUT / FF / IOB:",
                        ui_label(
                            format!(
                                "{} / {} / {} ({}% LUT)",
                                snap.lut_count, snap.ff_count, snap.iob_count, snap.lut_util_pct,
                            ),
                            t,
                        )
                        .key("reports-qor-counts"),
                        t,
                    ))
                    .child(kv(
                        "Max fanout:",
                        ui_label(fanout, t).key("reports-qor-fanout"),
                        t,
                    ))
                    .child(kv(
                        "Overused tiles:",
                        ui_label(snap.overused_tiles.to_string(), t).key("reports-qor-overused"),
                        t,
                    ))
                    .child(kv(
                        "Strategy:",
                        ui_label(
                            format!("{} → {}", snap.current_strategy, snap.recommended_strategy),
                            t,
                        )
                        .key("reports-qor-strategy"),
                        t,
                    ))
                    .child(kv(
                        "Design / Device:",
                        ui_label(format!("{} / {}", snap.design, snap.device_part), t)
                            .key("reports-qor-design")
                            .truncate(),
                        t,
                    )),
            ),
        )
}

/// QoR histogram: one bar per engine severity with its count.
fn qor_hist(snap: &QorSnapshot, t: &VivadoTheme) -> El {
    let rows = [
        (
            "reports-qor-bar-critical",
            "Critical",
            snap.severity_count("Critical"),
            t.fail,
        ),
        (
            "reports-qor-bar-warning",
            "Warning",
            snap.severity_count("Warning"),
            t.warn,
        ),
        (
            "reports-qor-bar-advisory",
            "Advisory",
            snap.severity_count("Advisory"),
            t.text_dim,
        ),
        (
            "reports-qor-bar-info",
            "Info",
            snap.severity_count("Info"),
            t.accent,
        ),
    ];
    let max = rows.iter().map(|(_, _, n, _)| *n).max().unwrap_or(0).max(1) as f32;
    let mut hist = col().key("reports-qor-hist").w_full().gap(2.0).child(
        text("Suggestions by Severity")
            .font(VivadoTheme::FONT_TITLE)
            .color(t.text),
    );
    for (key, label_text, count, color) in rows {
        hist = hist.child(hist_row(
            key.into(),
            label_text,
            count as f32 / max,
            count.to_string(),
            color,
            t,
        ));
    }
    hist
}

/// QoR suggestions table: `ID | Severity | Category | Target | Gain |
/// Description`. Rows select (GUI `select_qor`); the detail panel below
/// shows the full description and the suggestion's Tcl fix.
fn qor_table(s: &AppState, snap: &QorSnapshot, t: &VivadoTheme) -> El {
    use crate::views::report_kit as kit;
    let head = kit::header_row(t)
        .child(el().w(RULE_W).child(kit::header_cell("ID", t)))
        .child(el().w(SEV_W).child(kit::header_cell("Severity", t)))
        .child(el().w(SEV_W).child(kit::header_cell("Category", t)))
        .child(kit::header_cell("Target", t).grow(1.0).min_w(0.0))
        .child(kit::header_num("Gain", PCT_W, t))
        .child(kit::header_cell("Description", t).grow(3.0).min_w(0.0));
    let mut body = col().w_full();
    for (index, sugg) in snap.suggestions.iter().enumerate() {
        let id = sugg.id.clone();
        let selected = s.reports_view.selected_qor.as_deref() == Some(sugg.id.as_str());
        body = body.child(
            kit::select_row(
                format!("reports-qor-row-{index}"),
                format!("QoR suggestion {}", sugg.id),
                selected,
                t,
                move |s: &mut AppState| s.reports_view.selected_qor = Some(id.clone()),
            )
            .child(kit::fixed_cell(sugg.id.clone(), RULE_W, t.link))
            .child(kit::fixed_cell(
                sugg.severity.clone(),
                SEV_W,
                qor_sev_color(&sugg.severity, t),
            ))
            .child(kit::fixed_cell(sugg.category.clone(), SEV_W, t.text))
            .child(kit::text_cell(sugg.target.clone(), 1.0, t.text))
            .child(kit::num_cell(
                format!("+{}ps", sugg.expected_gain_ps),
                PCT_W,
                t.text,
            ))
            .child(kit::text_cell(sugg.description.clone(), 3.0, t.text)),
        );
    }
    col()
        .key("reports-qor-table")
        .w_full()
        .gap(2.0)
        .child(kit::section_title("Suggestions", t))
        .child(kit::table(head, body, t))
        .child(el().w_full().pt(6.0).child(panels::qor_detail(s, snap, t)))
}

/// QoR detail body: assessment summary, severity histogram, suggestions
/// table. Unevaluated until [`refresh_qor`] assesses the elaborated design;
/// failures render their reason, never stale numbers.
fn qor_body(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col().key("reports-qor").w_full().gap(10.0).child(
        text(format!("QoR Suggestions ({})", s.project.part))
            .font(VivadoTheme::FONT_TITLE)
            .color(t.text)
            .key("reports-qor-title"),
    );
    match (&s.reports_view.qor, &s.reports_view.qor_error) {
        (Some(snap), _) => {
            body = body.child(qor_summary(snap, t));
            if snap.suggestions.is_empty() {
                body = body.child(
                    text("No QoR suggestions — the design meets all checks.")
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.pass)
                        .key("reports-qor-clean"),
                );
            } else {
                body = body.child(qor_hist(snap, t)).child(qor_table(s, snap, t));
            }
        }
        (None, Some(reason)) => {
            body = body.child(
                row()
                    .key("reports-qor-empty")
                    .w_full()
                    .h(TABLE_ROW_H)
                    .items_center()
                    .px(8.0)
                    .child(dim_label(reason, t)),
            );
        }
        (None, None) => {
            body = body.child(
                row()
                    .key("reports-qor-empty")
                    .w_full()
                    .h(TABLE_ROW_H)
                    .items_center()
                    .px(8.0)
                    .child(dim_label(
                        "QoR not evaluated — open this report to assess the design.",
                        t,
                    )),
            );
        }
    }
    body
}

/// Right-hand report content for the selected tree entry: each report
/// opens as soon as its data exists (Methodology/QoR/Utilization/Power
/// after synthesis, DRC after placement), headed by its re-run action;
/// otherwise one sentence plus the run that produces it.
fn report_content(s: &AppState, t: &VivadoTheme) -> El {
    let selected = s.reports_view.selected;
    if !panels::report_ready(s, selected) {
        return panels::not_ready(s, selected, t);
    }
    col()
        .w_full()
        .gap(6.0)
        .padding(8.0)
        .child(panels::action_bar(s, selected, t))
        .child(match selected {
            ReportNav::Utilization => util_body(s, t),
            ReportNav::Power => power_body(s, t),
            ReportNav::Drc => drc_body(s, t),
            ReportNav::Methodology => method_body(s, t),
            ReportNav::Qor => qor_body(s, t),
            ReportNav::ClockUtil => io::clock_body(s, t),
            ReportNav::ControlSets => io::control_body(s, t),
            ReportNav::Ssn => io::ssn_body(s, t),
            ReportNav::Ibis => io::ibis_body(s, t),
        })
}

/// The center Reports pane wired into the shell.
pub fn reports_pane(s: &AppState, t: &VivadoTheme) -> El {
    let mut nav = col().key("reports-nav").w_full().gap(0.0).py(4.0);
    for nav_entry in ReportNav::ALL {
        nav = nav.child(nav_row(nav_entry, s, t));
    }
    col()
        .key("reports-pane")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Reports", "reports", t))
        .child(
            row()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(
                    col()
                        .key("reports-nav-scroll")
                        .w(NAV_W)
                        .min_h(0.0)
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .child(nav),
                )
                .child(el().w(1.0).grow(0.0).fill(t.border))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .min_h(0.0)
                        // Tables plus histograms exceed the pane height, so
                        // the report scrolls (framework viewport, no manual
                        // geometry) while the footer stays pinned.
                        .child(
                            col()
                                .key("reports-scroll")
                                .grow(1.0)
                                .min_h(0.0)
                                .scroll(ScrollSpec {
                                    stick_bottom: false,
                                })
                                .child(report_content(s, t)),
                        )
                        .child(el().w_full().px(8.0).py(2.0).child(
                            dim_label(s.reports_view.selected.footer(), t).key("reports-footer"),
                        )),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn harness_with(state: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(state, crate::view, spec.size, env)
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn reports_state() -> AppState {
        let mut s = init();
        s.center = crate::state::CenterPane::Reports;
        demo_impl_complete(&mut s);
        s
    }

    #[test]
    fn reports_tree_exposes_nav_table_hist_and_footer() {
        let mut rt = harness_with(reports_state());
        rt.frame(0.0);
        for key in [
            "reports-pane",
            "reports-nav",
            "reports-nav-utilization",
            "reports-nav-power",
            "reports-nav-drc",
            "reports-util",
            "reports-util-title",
            "reports-util-table",
            "reports-util-row-0",
            "reports-util-row-5",
            "reports-util-hist",
            "reports-util-bar-0",
            "reports-hier",
            "reports-hier-row-0",
            "reports-hier-row-3",
            "reports-footer",
        ] {
            assert!(rt.find(key).is_some(), "missing reports key: {key}");
        }
        for key in ["reports-power", "reports-drc", "reports-no-run"] {
            assert!(rt.find(key).is_none(), "{key} must wait for its nav entry");
        }
    }

    #[test]
    fn power_body_shows_summary_breakdown_and_split() {
        let mut s = reports_state();
        s.reports_view.selected = ReportNav::Power;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        for key in [
            "reports-power",
            "reports-power-title",
            "reports-power-summary",
            "reports-power-total",
            "reports-power-dynamic",
            "reports-power-static",
            "reports-power-junction",
            "reports-power-confidence",
            "reports-power-split",
            "reports-power-split-dynamic",
            "reports-power-split-static",
            "reports-power-split-label",
            "reports-power-table",
            "reports-power-row-0",
            "reports-power-row-8",
            "reports-power-blocks",
            "reports-power-block-0",
            "reports-power-block-5",
            "reports-action-power",
            "reports-footer",
        ] {
            assert!(rt.find(key).is_some(), "missing power key: {key}");
        }
        assert!(
            rt.find("reports-util-table").is_none(),
            "util body must leave"
        );
    }

    #[test]
    fn wheel_over_power_report_scrolls_blocks_into_view() {
        let mut s = reports_state();
        s.reports_view.selected = ReportNav::Power;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        let vp = rt.presented("reports-scroll").unwrap().0;
        let y_before = rt.presented("reports-power-blocks").unwrap().0[1];
        assert!(
            y_before > vp[1] + vp[3],
            "blocks must start below the fold (y={y_before}, fold={})",
            vp[1] + vp[3]
        );
        let (wx, wy) = (vp[0] + vp[2] / 2.0, vp[1] + vp[3] / 2.0);
        for i in 0..10 {
            rt.wheel(wx, wy, -140.0);
            rt.frame(0.1 + i as f64 * (1.0 / 60.0));
        }
        let y_after = rt.presented("reports-power-blocks").unwrap().0[1];
        assert!(
            y_after < y_before - 100.0,
            "wheel must move content up (before={y_before}, after={y_after})"
        );
    }

    #[test]
    fn drc_clean_verdict_gives_way_to_seeded_violations() {
        let mut rt = harness_with(reports_state());
        rt.frame(0.0);
        tap(&mut rt, "reports-nav-drc");
        assert_eq!(rt.state.reports_view.selected, ReportNav::Drc);
        assert!(
            rt.find("reports-drc-verdict").is_some(),
            "verdict must show"
        );
        assert!(rt.find("reports-drc-hist").is_some(), "histogram must show");
        assert!(
            rt.find("reports-drc-table").is_none(),
            "clean design has no table"
        );

        let mut s = reports_state();
        s.reports_view.selected = ReportNav::Drc;
        s.drc.items = vec![
            crate::state::DrcItem {
                id: "PDRC-190".into(),
                severity: DrcSev::Warning,
                objects: "clk_pin".into(),
                message: "High fanout net: 50 loads".into(),
            },
            crate::state::DrcItem {
                id: "REQP-144".into(),
                severity: DrcSev::Advisory,
                objects: "led_ctl0/led_o_reg[5]".into(),
                message: "Control set has no asynchronous reset".into(),
            },
        ];
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("reports-drc-table").is_some(), "table must mount");
        assert!(rt.find("reports-drc-row-0").is_some(), "rows must mount");
        assert!(rt.find("reports-drc-row-1").is_some(), "rows must mount");
        assert!(
            rt.find("reports-drc-hint").is_some(),
            "hint before selection"
        );
        tap(&mut rt, "reports-drc-row-0");
        assert_eq!(rt.state.reports_view.selected_drc, Some(0));
        assert!(
            rt.find("reports-drc-detail").is_some(),
            "detail panel must mount"
        );
        assert!(rt.find("reports-drc-hint").is_none());
    }

    #[test]
    fn nav_tap_switches_bodies() {
        let mut rt = harness_with(reports_state());
        rt.frame(0.0);
        tap(&mut rt, "reports-nav-power");
        assert_eq!(rt.state.reports_view.selected, ReportNav::Power);
        assert!(
            rt.find("reports-power-table").is_some(),
            "power body must show"
        );
        assert!(
            rt.find("reports-util-table").is_none(),
            "util body must leave"
        );
        tap(&mut rt, "reports-nav-utilization");
        assert_eq!(rt.state.reports_view.selected, ReportNav::Utilization);
        assert!(
            rt.find("reports-util-table").is_some(),
            "util body must return"
        );
    }

    #[test]
    fn fresh_project_prompts_for_the_run_each_report_needs() {
        let mut s = init();
        s.center = crate::state::CenterPane::Reports;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("reports-run-synth").is_some(),
            "utilization needs synthesis"
        );
        assert!(rt.find("reports-run-impl").is_none(), "one CTA only");
        for key in [
            "reports-util",
            "reports-power",
            "reports-drc-table",
            "reports-actions",
        ] {
            assert!(rt.find(key).is_none(), "result {key} must wait for a run");
        }
        tap(&mut rt, "reports-nav-drc");
        assert!(rt.find("reports-run-impl").is_some(), "DRC needs placement");
        assert!(rt.find("reports-run-synth").is_none(), "one CTA only");
        for nav in [
            "reports-nav-power",
            "reports-nav-methodology",
            "reports-nav-qor",
        ] {
            tap(&mut rt, nav);
            assert!(
                rt.find("reports-run-synth").is_some(),
                "{nav} needs synthesis"
            );
        }
    }

    #[test]
    fn methodology_and_util_open_after_synthesis_without_implementation() {
        let mut s = init();
        s.center = crate::state::CenterPane::Reports;
        s.flow.synth = RunStatus::Complete;
        s.timing.has_run = true;
        s.util.luts_used = 12;
        s.util.luts_avail = 8192;
        s.reports_view.selected = ReportNav::Methodology;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("reports-method").is_some(),
            "methodology shows post-synth"
        );
        assert!(rt.find("reports-stage").is_some());
        tap(&mut rt, "reports-nav-utilization");
        assert!(
            rt.find("reports-util-kpis").is_some(),
            "utilization shows post-synth"
        );
        tap(&mut rt, "reports-nav-drc");
        assert!(
            rt.find("reports-run-impl").is_some(),
            "DRC still needs placement"
        );
    }

    #[test]
    fn run_prompt_starts_implementation_and_echoes_tcl() {
        let mut s = init();
        s.center = crate::state::CenterPane::Reports;
        s.reports_view.selected = ReportNav::Drc;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "reports-run-impl");
        assert_eq!(rt.state.flow.impl_run, RunStatus::Running);
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "launch_runs impl_1"),
            "prompt echoes its Tcl verb"
        );
    }

    #[test]
    fn hier_table_renders_real_state_rows() {
        // deep-02: the hierarchy table must list UtilState.hier from the
        // run's real occupancy, never hardcoded demo modules.
        use crate::state::HierRow;
        let mut s = reports_state();
        s.util.hier = vec![
            HierRow {
                name: "counter".into(),
                child: false,
                lut: 9,
                ff: 8,
                io: 0,
                bram: 0,
                dsp: 0,
            },
            HierRow {
                name: "counter_led".into(),
                child: true,
                lut: 2,
                ff: 1,
                io: 0,
                bram: 0,
                dsp: 0,
            },
        ];
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("reports-hier").is_some(), "hierarchy must show");
        assert!(
            rt.find("reports-hier-row-0").is_some(),
            "real hier 0 must show"
        );
        assert!(
            rt.find("reports-hier-row-1").is_some(),
            "real hier 1 must show"
        );
        assert!(
            rt.find("reports-hier-row-2").is_none(),
            "no demo rows may appear"
        );
    }

    #[test]
    fn drc_page_shows_real_signoff_and_bitstream() {
        // deep-02: signoff gate + bitstream results from real runs must be
        // visible on the DRC page.
        let mut s = reports_state();
        s.reports_view.selected = ReportNav::Drc;
        s.drc.bitgen_allowed = Some(true);
        s.drc.signoff_summary = "bitgen_signoff allowed=true errors=0".into();
        s.drc.has_run = true;
        s.bitstream = crate::state::BitstreamState {
            idcode: 0x01EC,
            hash: 0x1234,
            bytes: 512,
            frames: 4,
            configured: 3,
            has_run: true,
            ..crate::state::BitstreamState::default()
        };
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("reports-drc-signoff").is_some(),
            "signoff card must show"
        );
        assert!(
            rt.find("reports-bitstream").is_some(),
            "bitstream card must show"
        );
    }

    #[test]
    fn formatters_match_vivado_text() {
        assert_eq!(format_mw(12500), "12.500 mW");
        assert_eq!(format_mw(0), "0.000 mW");
        assert_eq!(format_temp(254), "25.4 °C");
        assert_eq!(
            ReportNav::Utilization.footer(),
            "Utilization - utilization_1"
        );
        assert_eq!(ReportNav::Drc.footer(), "DRC - drc_1");
        assert_eq!(
            ReportNav::Methodology.footer(),
            "Methodology - methodology_1"
        );
        assert_eq!(
            ReportNav::Qor.footer(),
            "QoR Suggestions - qor_suggestions_1"
        );
        assert_eq!(rqa_label(5), "Excellent");
        assert_eq!(rqa_label(1), "Critical");
    }

    #[test]
    fn demo_blocks_sum_to_the_dynamic_total() {
        let mut s = init();
        demo_impl_complete(&mut s);
        let p = &s.power;
        let blocks: u32 = p.clocks_uw + p.signals_uw + p.logic_uw + p.io_uw + p.bram_uw + p.dsp_uw;
        assert_eq!(
            blocks,
            s.power.dynamic_mw * 1000,
            "blocks must sum to dynamic"
        );
        assert_eq!(
            s.power.total_mw,
            s.power.dynamic_mw + s.power.static_mw,
            "split must sum to total"
        );
    }

    #[test]
    fn no_run_prompt_launches_a_real_run() {
        let mut s = init();
        s.center = crate::state::CenterPane::Reports;
        s.reports_view.selected = ReportNav::Drc;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "reports-run-impl");
        assert!(rt.state.runs.is_active(), "prompt must arm a real worker");
        crate::runs::cancel_run(&mut rt.state);
        let start = std::time::Instant::now();
        while rt.state.runs.is_active() {
            crate::runs::poll_runs(&mut rt.state);
            if !rt.state.runs.is_active() {
                break;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(60),
                "run timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn running_run_replaces_prompt_with_status() {
        let mut s = init();
        s.center = crate::state::CenterPane::Reports;
        s.flow.impl_run = RunStatus::Running;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("reports-run-status").is_some(),
            "running notice must show"
        );
        assert!(
            rt.find("reports-run-impl").is_none(),
            "prompt link must hide while running"
        );
    }

    fn qor_design_state() -> AppState {
        let mut s = reports_state();
        s.tcl.design = Some(
            helion_sv::synth_sv(crate::live::FIXTURE_SRC, "fixture.sv")
                .expect("fixture must elaborate"),
        );
        s
    }

    #[test]
    fn qor_nav_computes_a_real_engine_report() {
        // deep-05: QoR comes from report_qor_suggestions on the real
        // elaborated design, never canned rows.
        let mut rt = harness_with(qor_design_state());
        rt.frame(0.0);
        tap(&mut rt, "reports-nav-qor");
        assert_eq!(rt.state.reports_view.selected, ReportNav::Qor);
        let snap = rt
            .state
            .reports_view
            .qor
            .as_ref()
            .expect("QoR snapshot must compute");
        assert!((1..=5).contains(&snap.rqa_score), "RQA score is 1..=5");
        assert!(!snap.device_part.is_empty(), "device part must be recorded");
        for key in [
            "reports-qor",
            "reports-qor-title",
            "reports-qor-summary",
            "reports-qor-score",
            "reports-qor-refresh",
            "reports-footer",
        ] {
            assert!(rt.find(key).is_some(), "missing QoR key: {key}");
        }
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text.starts_with("report_qor_suggestions ")),
            "QoR evaluation echoes its Tcl verb"
        );
    }

    #[test]
    fn qor_without_design_shows_a_disabled_reason() {
        // deep-05: no elaborated design means a visible reason, not an
        // empty table or demo rows.
        let mut rt = harness_with(reports_state());
        rt.frame(0.0);
        tap(&mut rt, "reports-nav-qor");
        assert!(rt.state.reports_view.qor.is_none(), "nothing to assess");
        assert!(
            rt.state.reports_view.qor_error.is_some(),
            "reason must be stored"
        );
        assert!(rt.find("reports-no-run").is_some(), "reason must show");
        assert!(rt.find("reports-run-synth").is_some(), "one real CTA");
        assert!(
            rt.find("reports-qor-table").is_none(),
            "no table without data"
        );
    }

    #[test]
    fn qor_table_renders_snapshot_rows_and_selection_shows_detail() {
        // deep-05: suggestion rows render the snapshot; selecting one shows
        // its description and executable Tcl fix (GUI select_qor behavior).
        let mut s = reports_state();
        s.reports_view.selected = ReportNav::Qor;
        s.reports_view.qor = Some(QorSnapshot {
            rqa_score: 2,
            wns_ps: -312,
            tns_ps: -1240,
            whs_ps: 96,
            endpoints: 112,
            lut_count: 40,
            ff_count: 32,
            iob_count: 10,
            lut_util_pct: 1,
            max_fanout: 9,
            high_fanout_net: "rst_n".into(),
            overused_tiles: 0,
            current_strategy: "Default".into(),
            recommended_strategy: "Performance_Explore".into(),
            device_part: "HL10T-C32-1".into(),
            design: "counter_top".into(),
            suggestions: vec![
                QorSuggRow {
                    id: "RQS_STRAT-1".into(),
                    category: "Strategy".into(),
                    severity: "Critical".into(),
                    applicable_for: "place_design,route_design".into(),
                    tcl_command: "set_property STRATEGY Performance_Explore [get_runs impl_1]"
                        .into(),
                    target: "impl_1".into(),
                    description: "Negative WNS; switch strategy".into(),
                    expected_gain_ps: 250,
                },
                QorSuggRow {
                    id: "RQS_FANOUT-1".into(),
                    category: "Congestion".into(),
                    severity: "Advisory".into(),
                    applicable_for: "synth_design".into(),
                    tcl_command: "set_property MAX_FANOUT 32 [get_nets rst_n]".into(),
                    target: "rst_n".into(),
                    description: "Replicate high-fanout driver".into(),
                    expected_gain_ps: 120,
                },
            ],
        });
        let mut rt = harness_with(s);
        rt.frame(0.0);
        for key in [
            "reports-qor-table",
            "reports-qor-hist",
            "reports-qor-row-0",
            "reports-qor-row-1",
            "reports-qor-hint",
        ] {
            assert!(rt.find(key).is_some(), "missing QoR key: {key}");
        }
        assert!(rt.find("reports-qor-clean").is_none(), "suggestions exist");
        tap(&mut rt, "reports-qor-row-0");
        assert_eq!(
            rt.state.reports_view.selected_qor.as_deref(),
            Some("RQS_STRAT-1")
        );
        assert!(
            rt.find("reports-qor-detail").is_some(),
            "detail shows the fix"
        );
    }

    #[test]
    fn methodology_body_lists_real_run_rows() {
        // deep-05: methodology renders TimingState.methodology from the
        // real report_methodology run; empty runs show a reason.
        let mut s = reports_state();
        s.timing.methodology = vec![
            crate::state::MethodRow {
                id: "TIMING-6".into(),
                severity: "Warning".into(),
                category: "TIMING".into(),
                objects: "clk".into(),
                message: "No clock defined on primary input".into(),
            },
            crate::state::MethodRow {
                id: "XDC-1".into(),
                severity: "Advisory".into(),
                category: "XDC".into(),
                objects: "led[*]".into(),
                message: "Unconstrained output port".into(),
            },
        ];
        s.reports_view.selected = ReportNav::Methodology;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        for key in [
            "reports-method",
            "reports-method-title",
            "reports-method-verdict",
            "reports-method-hist",
            "reports-method-table",
            "reports-method-row-0",
            "reports-method-row-1",
            "reports-method-hint",
        ] {
            assert!(rt.find(key).is_some(), "missing methodology key: {key}");
        }
        tap(&mut rt, "reports-method-row-0");
        assert_eq!(rt.state.reports_view.selected_method, Some(0));
        assert!(
            rt.find("reports-method-detail").is_some(),
            "selection opens the detail with the full message"
        );

        let mut s = reports_state();
        s.reports_view.selected = ReportNav::Methodology;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("reports-method-empty").is_some(),
            "reason must show"
        );
        assert!(
            rt.find("reports-method-table").is_none(),
            "no table without data"
        );
    }

    #[test]
    fn power_summary_shows_the_real_pvt_header() {
        // deep-05: the report_power part/PVT header fields must render,
        // not sit unread in PowerState.
        let mut s = reports_state();
        s.reports_view.selected = ReportNav::Power;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        for key in [
            "reports-power-part",
            "reports-power-voltage",
            "reports-power-temp",
        ] {
            assert!(rt.find(key).is_some(), "missing PVT key: {key}");
        }
    }

    #[test]
    fn bitstream_card_opens_hardware_manager_on_the_real_artifact() {
        // deep-05: the program flow starts from the real bitstream
        // artifact card, not a stray demo path.
        let mut s = reports_state();
        s.reports_view.selected = ReportNav::Drc;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "reports-bitstream-program");
        assert_eq!(rt.state.center, crate::state::CenterPane::Program);
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "open_hw_manager"),
            "navigation echoes its Tcl verb"
        );
    }
}
