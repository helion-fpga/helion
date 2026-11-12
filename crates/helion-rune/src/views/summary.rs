//! Project Summary dashboard: Settings/Synth/Impl/DRC/Timing/Util/Power/QoR/Bitstream cards.
//!
//! New Rune `El` code written from scratch for the Vivado brief
//! (`/tmp/vivado-ref/rd-1b93965a*.png` main window): the `PROJECT MANAGER -
//! project_1` center pane with its "Project Summary" header and the card grid.
//! The Settings card spans the full width; Synthesis/Implementation,
//! DRC/Timing, Utilization/Power, and QoR/Bitstream sit in reflowing
//! half-width pairs (no fixed-size cards). All colors come from
//! [`VivadoTheme`]; run state is the typed [`RunStatus`] enum (no stringly
//! status, no hardcoded hex, no manual geometry).
//!
//! Data shapes mirror `helion-gui` read-only: [`TimingState`] mirrors
//! `helion_sta::TimingSummary` picosecond numbers; [`UtilState`]/[`PowerState`]
//! mirror post-implementation report dimensions (used/available resources,
//! on-chip power split); the QoR card mirrors the sibling
//! [`crate::views::reports::QorSnapshot`] from the real
//! `report_qor_suggestions` evaluation; the Bitstream card mirrors
//! `ide.rs::BitstreamReport` (idcode/hash/frames) plus the
//! `check_bitstream_signoff` gate, and its link hands the real artifact to
//! the Hardware Manager pane. Result cards follow the live run state: a
//! running flow shows "Running...", a failed run points at Messages, and
//! only an idle flow offers to start.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el.rs`: `row()/col()/el()/text()`, `ScrollSpec`.
//! - `crates/rune/src/el/scroll.rs`: `.scroll()` vertical viewport.
//! - `crates/rune/src/el/text.rs`: `.truncate_middle()` for long paths.
//! - `crates/rune/src/el/anchor.rs`: pane header reused from sibling views.

use rune::{El, ScrollSpec, col, el, row, text};

use crate::state::{AppState, ConsoleLine, MsgSev, RunStatus};
use crate::theme::VivadoTheme;
use crate::views::reports::{ReportNav, refresh_qor, rqa_color, rqa_label};
use crate::views::sources::pane_header;

/// Label column width for every `label: value` row (fits "Total On-Chip Power:").
const LABEL_W: f32 = 136.0;
/// Card header band height.
const CARD_HEADER_H: f32 = 22.0;
/// Settings/`label: value` row height (compact CAD density).
const ROW_H: f32 = 20.0;

/// Echo a Tcl verb to the console without flipping run state.
fn echo(s: &mut AppState, tcl: &'static str) {
    s.console.lines.push(ConsoleLine {
        text: tcl.into(),
        sev: MsgSev::Info,
    });
}

/// Start Implementation: launch the real implementation run (the run
/// echoes its own `launch_runs impl_1` verb and flips the typed status once
/// the worker starts).
fn start_impl(s: &mut AppState) {
    crate::runs::launch(s, crate::runs::RunKind::Impl);
}

fn ui_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text)
}

fn dim_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text_dim)
}

/// Blue Vivado hyperlink that also opens the Settings dialog on a page.
fn settings_link(
    key: String,
    label_text: String,
    tcl: &'static str,
    page: Option<crate::views::dialogs::SettingsPage>,
    t: &VivadoTheme,
) -> El {
    el().key(key)
        .role("button")
        .label(label_text.clone())
        .focusable(true)
        .h(ROW_H)
        .px(2.0)
        .center_y()
        .hover(t.selection)
        .on_press(move |s: &mut AppState| {
            if let Some(p) = page {
                s.dialogs.settings.page = p;
            }
            s.dialogs.open(crate::views::dialogs::DialogId::Settings);
            echo(s, tcl);
        })
        .child(text(label_text).font(VivadoTheme::FONT_UI).color(t.link))
}

/// "Run Implementation" link: arms the Implementation run like the rail does.
fn run_impl_link(key: &'static str, t: &VivadoTheme) -> El {
    el().key(key)
        .role("button")
        .label("Run Implementation")
        .focusable(true)
        .h(ROW_H)
        .px(2.0)
        .center_y()
        .hover(t.selection)
        .on_press(start_impl)
        .child(
            text("Run Implementation")
                .font(VivadoTheme::FONT_UI)
                .color(t.link),
        )
}

/// One `label: value` row.
fn kv(label_text: impl Into<String>, value: El, t: &VivadoTheme) -> El {
    let label_text: String = label_text.into();
    row()
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(6.0)
        .child(el().w(LABEL_W).child(dim_label(&label_text, t).truncate().help_when_truncated(label_text.clone())))
        .child(value)
}

/// Card frame: title band plus an optional header link, over a padded body.
fn card(
    key: &'static str,
    title: &'static str,
    header_extra: Option<El>,
    t: &VivadoTheme,
    body: El,
) -> El {
    let mut head = row()
        .w_full()
        .h(CARD_HEADER_H)
        .px(8.0)
        .items_center()
        .justify_between()
        .fill(t.header_bg)
        .child(text(title).font(VivadoTheme::FONT_TITLE).color(t.text));
    head = head.child(header_extra.unwrap_or_else(|| el().a11y_hidden()));
    col()
        .key(key)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(head)
        .child(body)
}

/// Two reflowing half-width cards side by side.
fn half_row(left: El, right: El) -> El {
    row()
        .w_full()
        .gap(8.0)
        .child(left.grow(1.0).min_w(0.0))
        .child(right.grow(1.0).min_w(0.0))
}

/// Live flow status behind the result-card prompts: either flow running
/// reads Running, a failed implementation reads Failed, otherwise the cards
/// offer to start the run.
fn prompt_status(s: &AppState) -> RunStatus {
    if s.flow.impl_run == RunStatus::Running || s.flow.synth == RunStatus::Running {
        RunStatus::Running
    } else if s.flow.impl_run == RunStatus::Failed {
        RunStatus::Failed
    } else {
        RunStatus::NotStarted
    }
}

/// Centered result placeholder honoring the live run state: "Running..."
/// while a flow is active, a Messages pointer on failure, and only on an
/// idle flow the "Run Implementation to see ... results" prompt.
fn run_prompt(key: &'static str, noun: &'static str, status: RunStatus, t: &VivadoTheme) -> El {
    let mut content = row()
        .w_full()
        .h(40.0)
        .items_center()
        .justify_center()
        .gap(4.0);
    match status {
        RunStatus::Running => {
            content = content.child(
                dim_label(&format!("{noun} results running..."), t).key(format!("{key}-status")),
            );
        }
        RunStatus::Failed => {
            content = content.child(
                text("Failed - see Messages")
                    .font(VivadoTheme::FONT_UI)
                    .color(t.fail)
                    .key(format!("{key}-status")),
            );
        }
        RunStatus::NotStarted | RunStatus::Complete => {
            content = content
                .child(run_impl_link(key, t))
                .child(dim_label(&format!("to see {noun} results"), t));
        }
    }
    col().w_full().padding(8.0).child(content)
}

/// Status value: green once complete, red on failure, ink otherwise.
fn status_value(key: &'static str, status: RunStatus, t: &VivadoTheme) -> El {
    let color = match status {
        RunStatus::Complete => t.pass,
        RunStatus::Failed => t.fail,
        _ => t.text,
    };
    text(status.label())
        .font(VivadoTheme::FONT_UI)
        .color(color)
        .key(key)
}

/// Messages row derived from the typed run state (never parsed back).
fn flow_messages(
    s: &AppState,
    kind: crate::runs::RunKind,
    key: &'static str,
    status: RunStatus,
    t: &VivadoTheme,
) -> El {
    if status == RunStatus::Complete {
        let (mut err, mut crit, mut warn) = (0usize, 0usize, 0usize);
        for l in s.runs.log_for(kind) {
            match l.sev {
                MsgSev::Error => err += 1,
                MsgSev::Critical => crit += 1,
                MsgSev::Warn => warn += 1,
                MsgSev::Info => {}
            }
        }
        if err + crit + warn > 0 {
            let n = |c: usize, one: &str| format!("{c} {one}{}", if c == 1 { "" } else { "s" });
            let mut parts = Vec::new();
            if err > 0 {
                parts.push(n(err, "error"));
            }
            if crit > 0 {
                parts.push(n(crit, "critical warning"));
            }
            if warn > 0 {
                parts.push(n(warn, "warning"));
            }
            return text(parts.join(", "))
                .font(VivadoTheme::FONT_UI)
                .color(if err + crit > 0 { t.fail } else { t.warn })
                .key(key);
        }
    }
    match status {
        RunStatus::Failed => text("Failed - see Messages")
            .font(VivadoTheme::FONT_UI)
            .color(t.fail)
            .key(key),
        RunStatus::Running => dim_label("Running...", t).key(key),
        RunStatus::NotStarted | RunStatus::Complete => {
            ui_label("No errors or warnings", t).key(key)
        }
    }
}

/// Paths under the user's home folder display as `~/…` (shorter, and keeps
/// the account name out of screenshots and screen shares).
pub fn home_relative(path: &str) -> String {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return path.to_string();
    };
    match std::path::Path::new(path).strip_prefix(&home) {
        Ok(rest) if !home.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.to_string(),
    }
}

/// Picoseconds to Vivado-style `"2.061 ns"` text.
pub fn format_ns(ps: i64) -> String {
    format!("{:.3} ns", ps as f64 / 1000.0)
}

/// Milliwatts to Vivado-style `"0.142 W"` text.
pub fn format_watts(mw: u32) -> String {
    format!("{:.3} W", mw as f64 / 1000.0)
}

/// Rounded whole-percent utilization (`0` when nothing is available).
pub fn pct(used: u32, avail: u32) -> u32 {
    if avail == 0 {
        0
    } else {
        (used * 100 + avail / 2) / avail
    }
}

/// Vivado-style `"1523 (9%)"` utilization cell.
pub fn format_util(used: u32, avail: u32) -> String {
    format!("{used} ({}%)", pct(used, avail))
}

/// Settings card: full-width project identity rows.
fn settings_card(s: &AppState, t: &VivadoTheme) -> El {
    let p = &s.project;
    use crate::views::dialogs::SettingsPage;
    let top: El = match &p.top {
        Some(name) => ui_label(name, t).key("summary-top"),
        None => settings_link(
            "summary-top".into(),
            "Not defined".into(),
            "set_top",
            Some(SettingsPage::Elaboration),
            t,
        ),
    };
    let body = col()
        .w_full()
        .padding(8.0)
        .gap(2.0)
        .child(kv(
            "Project name:",
            ui_label(&p.name, t).key("summary-name"),
            t,
        ))
        .child(kv(
            "Project location:",
            text(home_relative(&p.location))
                .font(VivadoTheme::FONT_UI)
                .color(t.text)
                .grow(1.0)
                .shrink()
                .truncate_middle()
                .key("summary-location"),
            t,
        ))
        .child(kv(
            "Product family:",
            ui_label(&p.family, t).key("summary-family"),
            t,
        ))
        .child(kv(
            "Project part:",
            settings_link(
                "summary-part".into(),
                p.part.clone(),
                "show_part",
                Some(SettingsPage::General),
                t,
            ),
            t,
        ))
        .child(kv("Top module name:", top, t))
        .child(kv(
            "Target language:",
            settings_link(
                "summary-target-lang".into(),
                p.target_lang.clone(),
                "show_settings",
                Some(SettingsPage::General),
                t,
            ),
            t,
        ))
        .child(kv(
            "Simulator language:",
            settings_link(
                "summary-sim-lang".into(),
                p.sim_lang.clone(),
                "show_sim_settings",
                Some(SettingsPage::Simulation),
                t,
            ),
            t,
        ));
    card(
        "summary-settings",
        "Settings",
        Some(settings_link(
            "summary-edit".into(),
            "Edit".into(),
            "show_settings",
            Some(SettingsPage::General),
            t,
        )),
        t,
        body,
    )
    .w_full()
}

/// Synthesis run card.
fn synth_card(s: &AppState, t: &VivadoTheme) -> El {
    use crate::views::dialogs::SettingsPage;
    let body = col()
        .w_full()
        .padding(8.0)
        .gap(2.0)
        .child(kv(
            "Status:",
            status_value("summary-synth-status", s.flow.synth, t),
            t,
        ))
        .child(kv(
            "Messages:",
            flow_messages(s, crate::runs::RunKind::Synth, "summary-synth-messages", s.flow.synth, t),
            t,
        ))
        .child(kv(
            "Part:",
            ui_label(&s.project.part, t).key("summary-synth-part"),
            t,
        ))
        .child(kv(
            "Strategy:",
            settings_link(
                "summary-synth-strategy".into(),
                s.flow.synth_strategy.clone(),
                "show_strategy synth",
                Some(SettingsPage::Synthesis),
                t,
            ),
            t,
        ))
        .child(kv(
            "Report Strategy:",
            settings_link(
                "summary-synth-reports".into(),
                s.flow.synth_reports.clone(),
                "show_report_strategy synth",
                Some(SettingsPage::Synthesis),
                t,
            ),
            t,
        ));
    card("summary-synth", "Synthesis", None, t, body)
}

/// Implementation run card (adds the Incremental compile row).
fn impl_card(s: &AppState, t: &VivadoTheme) -> El {
    use crate::views::dialogs::SettingsPage;
    let body = col()
        .w_full()
        .padding(8.0)
        .gap(2.0)
        .child(kv(
            "Status:",
            status_value("summary-impl-status", s.flow.impl_run, t),
            t,
        ))
        .child(kv(
            "Messages:",
            flow_messages(s, crate::runs::RunKind::Impl, "summary-impl-messages", s.flow.impl_run, t),
            t,
        ))
        .child(kv(
            "Part:",
            ui_label(&s.project.part, t).key("summary-impl-part"),
            t,
        ))
        .child(kv(
            "Strategy:",
            settings_link(
                "summary-impl-strategy".into(),
                s.flow.impl_strategy.clone(),
                "show_strategy impl",
                Some(SettingsPage::Implementation),
                t,
            ),
            t,
        ))
        .child(kv(
            "Report Strategy:",
            settings_link(
                "summary-impl-reports".into(),
                s.flow.impl_reports.clone(),
                "show_report_strategy impl",
                Some(SettingsPage::Implementation),
                t,
            ),
            t,
        ))
        .child(kv(
            "Incremental compile:",
            settings_link(
                "summary-impl-incr".into(),
                "None".into(),
                "show_incremental",
                Some(SettingsPage::Implementation),
                t,
            ),
            t,
        ));
    card("summary-impl", "Implementation", None, t, body)
}

/// DRC Violations card: prompt until Implementation completes, then the
/// typed violation count (`No violations` when the run reported none).
fn drc_card(s: &AppState, t: &VivadoTheme) -> El {
    let body = if s.flow.impl_run == RunStatus::Complete {
        let total = s.drc.items.len();
        let (label_text, color) = if total == 0 {
            ("No violations".to_string(), t.pass)
        } else if total == 1 {
            ("1 violation".to_string(), t.fail)
        } else {
            (format!("{total} violations"), t.fail)
        };
        col().w_full().padding(8.0).gap(2.0).child(kv(
            "Status:",
            text(label_text)
                .font(VivadoTheme::FONT_UI)
                .color(color)
                .key("summary-drc-status"),
            t,
        ))
    } else {
        run_prompt("summary-run-impl-drc", "DRC", prompt_status(s), t)
    };
    card("summary-drc", "DRC Violations", None, t, body)
}

/// One slack row: red when the slack is negative, ink otherwise.
fn slack_row(key: &'static str, label_text: &'static str, ps: Option<i64>, t: &VivadoTheme) -> El {
    let value = match ps {
        Some(v) => text(format_ns(v))
            .font(VivadoTheme::FONT_UI)
            .color(if v < 0 { t.fail } else { t.text })
            .key(key),
        None => dim_label("-", t).key(key),
    };
    kv(label_text, value, t)
}

/// Timing card: WNS/TNS/WHS/THS plus the bold verdict once STA data exists.
fn timing_card(s: &AppState, t: &VivadoTheme) -> El {
    let body = if s.timing.has_run {
        let met = s.timing.wns_ps.unwrap_or(0) >= 0
            && s.timing.whs_ps.unwrap_or(0) >= 0
            && s.timing.failing_endpoints == 0;
        col()
            .w_full()
            .padding(8.0)
            .gap(2.0)
            .child(slack_row("summary-wns", "WNS:", s.timing.wns_ps, t))
            .child(kv(
                "TNS:",
                text(format_ns(s.timing.tns_ps))
                    .font(VivadoTheme::FONT_UI)
                    .color(if s.timing.tns_ps < 0 { t.fail } else { t.text })
                    .key("summary-tns"),
                t,
            ))
            .child(slack_row("summary-whs", "WHS:", s.timing.whs_ps, t))
            .child(kv(
                "THS:",
                text(format_ns(s.timing.ths_ps))
                    .font(VivadoTheme::FONT_UI)
                    .color(if s.timing.ths_ps < 0 { t.fail } else { t.text })
                    .key("summary-ths"),
                t,
            ))
            .child(kv(
                "Failing endpoints:",
                ui_label(&s.timing.failing_endpoints.to_string(), t).key("summary-failing"),
                t,
            ))
            .child(
                text(if met {
                    "All user specified timing constraints are met."
                } else {
                    "Timing constraints are not met."
                })
                .font(VivadoTheme::FONT_TITLE)
                .color(if met { t.pass } else { t.fail })
                .key("summary-timing-verdict"),
            )
            .child(worst_endpoints(s, t))
    } else {
        run_prompt("summary-run-impl-timing", "timing", prompt_status(s), t)
    };
    card("summary-timing", "Timing", None, t, body)
}

/// Top-5 worst endpoints (`# | Endpoint | Slack`); a row opens that path
/// in the Timing summary detail.
fn worst_endpoints(s: &AppState, t: &VivadoTheme) -> El {
    use crate::views::report_kit as kit;
    let mut worst: Vec<(usize, &crate::state::TimingPathRow)> =
        s.timing.paths.iter().enumerate().collect();
    worst.sort_by_key(|(_, p)| p.slack_ps);
    if worst.is_empty() {
        return dim_label("Endpoint paths appear after placement.", t).key("summary-worst-empty");
    }
    let head = kit::header_row(t)
        .child(kit::header_num("#", 20.0, t))
        .child(kit::header_cell("Worst endpoints", t).grow(1.0).min_w(0.0))
        .child(kit::header_num("Slack (ns)", 72.0, t));
    let mut body = col().w_full();
    for (n, (idx, p)) in worst.into_iter().take(5).enumerate() {
        body = body.child(
            kit::select_row(
                format!("summary-worst-row-{n}"),
                format!("Open path to {} in Timing", p.to),
                false,
                t,
                move |s: &mut AppState| {
                    s.timing_view.selected = crate::views::timing::TimingNav::Summary;
                    s.timing_view.selected_path = Some(idx);
                    s.navigate(crate::state::CenterPane::Timing);
                },
            )
            .child(kit::num_cell((n + 1).to_string(), 20.0, t.text_faint))
            .child(kit::text_cell(p.to.clone(), 1.0, t.text))
            .child(kit::num_cell(
                kit::ns3(p.slack_ps),
                72.0,
                kit::slack_color(Some(p.slack_ps), t),
            )),
        );
    }
    el().key("summary-worst")
        .w_full()
        .pt(4.0)
        .child(kit::table(head, body, t))
}

/// Run duration as `350 ms`, `4.2 s`, or `1m 05s`.
pub fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{}m {:02}s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

/// Run History card: the newest five finished runs of this session
/// (`# | Run | Stage | Status | WNS | LUT+FF | Duration`).
fn history_card(s: &AppState, t: &VivadoTheme) -> El {
    use crate::runs::RunOutcome;
    use crate::views::report_kit as kit;
    let hist = &s.runs.history;
    let body = if hist.is_empty() {
        col().w_full().padding(8.0).child(
            dim_label("No runs have finished in this session.", t).key("summary-history-empty"),
        )
    } else {
        let head = kit::header_row(t)
            .child(kit::header_num("#", 24.0, t))
            .child(el().w(64.0).child(kit::header_cell("Run", t)))
            .child(kit::header_cell("Stage", t).grow(1.0).min_w(0.0))
            .child(el().w(80.0).child(kit::header_cell("Status", t)))
            .child(kit::header_num("WNS (ns)", 72.0, t))
            .child(kit::header_num("LUT+FF", 64.0, t))
            .child(kit::header_num("Duration", 72.0, t));
        let mut rows = col().w_full();
        for (n, r) in hist.iter().rev().take(5).enumerate() {
            let status_ink = match r.outcome {
                RunOutcome::Complete => t.pass,
                RunOutcome::Failed => t.fail,
                RunOutcome::Cancelled => t.warn,
            };
            rows = rows.child(
                kit::plain_row(format!("summary-history-row-{n}"))
                    .child(kit::num_cell(r.seq.to_string(), 24.0, t.text_faint))
                    .child(kit::fixed_cell(r.kind.run_name(), 64.0, t.text))
                    .child(kit::text_cell(r.kind.title(), 1.0, t.text_dim))
                    .child(kit::fixed_cell(r.outcome.label(), 80.0, status_ink))
                    .child(kit::num_cell(
                        kit::ns_opt(r.wns_ps),
                        72.0,
                        kit::slack_color(r.wns_ps, t),
                    ))
                    .child(kit::num_cell(
                        r.lut_ff
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "–".into()),
                        64.0,
                        t.text,
                    ))
                    .child(kit::num_cell(format_duration(r.duration_ms), 72.0, t.text)),
            );
        }
        col().w_full().padding(8.0).child(kit::table(head, rows, t))
    };
    card("summary-history", "Run History", None, t, body).w_full()
}

/// Utilization card: resource rows once Implementation completes.
fn util_card(s: &AppState, t: &VivadoTheme) -> El {
    let body = if s.flow.impl_run == RunStatus::Complete {
        let u = &s.util;
        col()
            .w_full()
            .padding(8.0)
            .gap(2.0)
            .child(kv(
                format!("{}:", u.rows()[0].0),
                ui_label(&format_util(u.luts_used, u.luts_avail), t).key("summary-luts"),
                t,
            ))
            .child(kv(
                format!("{}:", u.rows()[1].0),
                ui_label(&format_util(u.regs_used, u.regs_avail), t).key("summary-regs"),
                t,
            ))
            .child(kv(
                format!("{}:", u.rows()[2].0),
                ui_label(&format_util(u.bram_used, u.bram_avail), t).key("summary-bram"),
                t,
            ))
    } else {
        run_prompt("summary-run-impl-util", "utilization", prompt_status(s), t)
    };
    card("summary-util", "Utilization", None, t, body)
}

/// Power card: on-chip power split plus the run-derived estimate
/// confidence once Implementation completes.
fn power_card(s: &AppState, t: &VivadoTheme) -> El {
    let body = if s.flow.impl_run == RunStatus::Complete {
        let p = &s.power;
        let confidence = if p.confidence.is_empty() {
            "n/a"
        } else {
            p.confidence.as_str()
        };
        col()
            .w_full()
            .padding(8.0)
            .gap(2.0)
            .child(kv(
                "Total On-Chip Power:",
                ui_label(&format_watts(p.total_mw), t).key("summary-power-total"),
                t,
            ))
            .child(kv(
                "Dynamic:",
                ui_label(&format_watts(p.dynamic_mw), t).key("summary-power-dynamic"),
                t,
            ))
            .child(kv(
                "Device Static:",
                ui_label(&format_watts(p.static_mw), t).key("summary-power-static"),
                t,
            ))
            .child(kv(
                "Confidence:",
                ui_label(confidence, t).key("summary-power-confidence"),
                t,
            ))
    } else {
        run_prompt("summary-run-impl-power", "power", prompt_status(s), t)
    };
    card("summary-power", "Power", None, t, body)
}

/// "Open QoR Report" link: selects the QoR report, evaluates it against the
/// live design, and navigates there.
fn open_qor_link(t: &VivadoTheme) -> El {
    el().key("summary-qor-open")
        .role("button")
        .label("Open QoR Report")
        .focusable(true)
        .h(ROW_H)
        .px(2.0)
        .center_y()
        .hover(t.selection)
        .on_press(|s: &mut AppState| {
            s.reports_view.selected = ReportNav::Qor;
            refresh_qor(s);
            s.navigate(crate::state::CenterPane::Reports);
        })
        .child(
            text("Open QoR Report")
                .font(VivadoTheme::FONT_UI)
                .color(t.link),
        )
}

/// QoR card: RQA score, suggestion counts, and the top suggestion from the
/// last real `report_qor_suggestions` evaluation.
fn qor_card(s: &AppState, t: &VivadoTheme) -> El {
    let body = match &s.reports_view.qor {
        Some(q) => {
            let critical = q.severity_count("Critical");
            let count = if critical > 0 {
                format!("{} ({critical} Critical)", q.suggestions.len())
            } else {
                q.suggestions.len().to_string()
            };
            let top = q
                .suggestions
                .first()
                .map(|g| format!("{} ({})", g.id, g.severity))
                .unwrap_or_else(|| "None (clean)".into());
            col()
                .w_full()
                .padding(8.0)
                .gap(2.0)
                .child(kv(
                    "RQA Score:",
                    text(format!("{}/5 ({})", q.rqa_score, rqa_label(q.rqa_score)))
                        .font(VivadoTheme::FONT_UI)
                        .color(rqa_color(q.rqa_score, t))
                        .key("summary-qor-score"),
                    t,
                ))
                .child(kv(
                    "Suggestions:",
                    ui_label(&count, t).key("summary-qor-count"),
                    t,
                ))
                .child(kv("Top:", ui_label(&top, t).key("summary-qor-top"), t))
                .child(open_qor_link(t))
        }
        None => col().w_full().padding(8.0).gap(2.0).child(
            row()
                .w_full()
                .h(40.0)
                .items_center()
                .justify_center()
                .gap(4.0)
                .child(dim_label("Not evaluated.", t).key("summary-qor-empty"))
                .child(open_qor_link(t)),
        ),
    };
    card("summary-qor", "QoR", None, t, body)
}

/// Bitstream card: the real `write_bitstream` artifact identity plus the
/// signoff gate, with a link handing the artifact to Hardware Manager.
fn bitstream_card(s: &AppState, t: &VivadoTheme) -> El {
    let body = if s.bitstream.has_run {
        let b = &s.bitstream;
        let (signoff_text, signoff_color) = match s.drc.bitgen_allowed {
            Some(true) => ("Allowed".to_string(), t.pass),
            Some(false) => ("Blocked by DRC errors".to_string(), t.fail),
            None => ("Not evaluated".to_string(), t.text_dim),
        };
        col()
            .w_full()
            .padding(8.0)
            .gap(2.0)
            .child(kv(
                "IDCODE:",
                ui_label(&format!("{:#010x}", b.idcode), t).key("summary-bitstream-idcode"),
                t,
            ))
            .child(kv(
                "Hash:",
                ui_label(&format!("{:#010x}", b.hash), t).key("summary-bitstream-hash"),
                t,
            ))
            .child(kv(
                "Frames:",
                ui_label(&format!("{} ({} configured)", b.frames, b.configured), t)
                    .key("summary-bitstream-frames"),
                t,
            ))
            .child(kv(
                "Signoff:",
                text(signoff_text)
                    .font(VivadoTheme::FONT_UI)
                    .color(signoff_color)
                    .key("summary-bitstream-signoff"),
                t,
            ))
            .child(
                el().key("summary-bitstream-open")
                    .role("button")
                    .label("Open Hardware Manager")
                    .focusable(true)
                    .h(ROW_H)
                    .px(2.0)
                    .center_y()
                    .hover(t.selection)
                    .on_press(|s: &mut AppState| {
                        s.navigate(crate::state::CenterPane::Program);
                        echo(s, "open_hw_manager");
                    })
                    .child(
                        text("Open Hardware Manager")
                            .font(VivadoTheme::FONT_UI)
                            .color(t.link),
                    ),
            )
    } else {
        run_prompt(
            "summary-run-impl-bitstream",
            "bitstream",
            prompt_status(s),
            t,
        )
    };
    card("summary-bitstream", "Bitstream", None, t, body)
}

/// The center Project Summary pane wired into the shell.
pub fn summary_pane(s: &AppState, t: &VivadoTheme) -> El {
    col()
        .key("summary")
        .grow(1.0)
        .h_full()
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            el().key("summary-title")
                .w_full()
                .h(30.0)
                .px(10.0)
                .center_y()
                .fill(t.titlebar_bg)
                .child(
                    text(format!("PROJECT MANAGER - {}", s.project.name))
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text),
                ),
        )
        .child(pane_header("Project Summary", "summary-pane", t))
        // The full card grid is taller than the center pane once results
        // exist, so the cards scroll (framework viewport, no manual
        // geometry): without this the Timing failing-endpoints row clips at
        // the console fold and the Utilization/Power bodies paint unseen
        // beneath the console dock.
        .child(
            col()
                .key("summary-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(
                    col()
                        .w_full()
                        .padding(8.0)
                        .gap(8.0)
                        .child(settings_card(s, t))
                        .child(half_row(synth_card(s, t), impl_card(s, t)))
                        .child(half_row(drc_card(s, t), timing_card(s, t)))
                        .child(half_row(util_card(s, t), power_card(s, t)))
                        .child(half_row(qor_card(s, t), bitstream_card(s, t)))
                        .child(history_card(s, t)),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn harness() -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(init(), crate::view, spec.size, env)
    }

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

    /// Tall harness fitting the whole card grid without scrolling, so
    /// below-fold card links take taps like on a large monitor.
    fn harness_tall(state: AppState) -> Runtime<AppState> {
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(state, crate::view, (1440.0, 2400.0), env)
    }

    fn impl_complete_state() -> AppState {
        let mut s = init();
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
            ..crate::state::UtilState::default()
        };
        s.power = crate::state::PowerState {
            total_mw: 142,
            dynamic_mw: 38,
            static_mw: 104,
            clocks_uw: 12500,
            logic_uw: 8600,
            signals_uw: 9800,
            io_uw: 5200,
            bram_uw: 1900,
            dsp_uw: 0,
            junction_deci_c: 254,
            ..crate::state::PowerState::default()
        };
        s
    }

    #[test]
    fn summary_tree_exposes_every_card_and_link() {
        let mut rt = harness();
        rt.frame(0.0);
        for key in [
            "summary",
            "summary-title",
            "summary-scroll",
            "summary-settings",
            "summary-edit",
            "summary-name",
            "summary-part",
            "summary-top",
            "summary-target-lang",
            "summary-sim-lang",
            "summary-synth",
            "summary-synth-status",
            "summary-synth-strategy",
            "summary-synth-reports",
            "summary-impl",
            "summary-impl-status",
            "summary-impl-strategy",
            "summary-impl-reports",
            "summary-impl-incr",
            "summary-drc",
            "summary-timing",
            "summary-util",
            "summary-power",
        ] {
            assert!(rt.find(key).is_some(), "missing summary key: {key}");
        }
    }

    #[test]
    fn fresh_project_prompts_to_run_implementation() {
        let mut rt = harness();
        rt.frame(0.0);
        for key in [
            "summary-run-impl-drc",
            "summary-run-impl-timing",
            "summary-run-impl-util",
            "summary-run-impl-power",
        ] {
            assert!(rt.find(key).is_some(), "missing run prompt: {key}");
        }
        for key in [
            "summary-drc-status",
            "summary-wns",
            "summary-luts",
            "summary-power-total",
        ] {
            assert!(rt.find(key).is_none(), "result {key} must wait for a run");
        }
    }

    #[test]
    fn run_prompt_starts_implementation_and_echoes_tcl() {
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "summary-run-impl-drc");
        assert_eq!(rt.state.flow.impl_run, RunStatus::Running);
        assert!(
            rt.state.runs.is_active(),
            "the prompt must launch a real worker, not just flip the status"
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "launch_runs impl_1"),
            "prompt echoes its Tcl verb"
        );
        crate::runs::cancel_run(&mut rt.state);
        let start = std::time::Instant::now();
        while rt.state.runs.is_active() {
            crate::runs::poll_runs(&mut rt.state);
            assert!(
                start.elapsed() < std::time::Duration::from_secs(60),
                "run timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            rt.state.runs.history.len(),
            1,
            "the finished run is recorded in the history"
        );
    }

    #[test]
    fn history_card_lists_newest_runs_first() {
        use crate::runs::{RunKind, RunOutcome, RunRecord};
        let mut s = init();
        for seq in 1..=7 {
            s.runs.history.push(RunRecord {
                seq,
                kind: if seq % 2 == 0 {
                    RunKind::Impl
                } else {
                    RunKind::Synth
                },
                outcome: RunOutcome::Complete,
                wns_ps: Some(1200),
                lut_ff: Some(24),
                duration_ms: 1500,
                ..Default::default()
            });
        }
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("summary-history-row-4").is_some(),
            "five newest rows"
        );
        assert!(rt.find("summary-history-row-5").is_none(), "capped at five");
        assert_eq!(format_duration(350), "350 ms");
        assert_eq!(format_duration(4200), "4.2 s");
        assert_eq!(format_duration(65_000), "1m 05s");
    }

    #[test]
    fn settings_edit_echoes_without_flipping_runs() {
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "summary-edit");
        assert_eq!(rt.state.flow.synth, RunStatus::NotStarted);
        assert_eq!(rt.state.flow.impl_run, RunStatus::NotStarted);
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "show_settings")
        );
    }

    #[test]
    fn impl_complete_state_shows_results_not_prompts() {
        let mut rt = harness_with(impl_complete_state());
        rt.frame(0.0);
        for key in [
            "summary-drc-status",
            "summary-wns",
            "summary-tns",
            "summary-whs",
            "summary-ths",
            "summary-failing",
            "summary-timing-verdict",
            "summary-luts",
            "summary-regs",
            "summary-bram",
            "summary-power-total",
            "summary-power-dynamic",
            "summary-power-static",
        ] {
            assert!(rt.find(key).is_some(), "missing result key: {key}");
        }
        assert!(
            rt.find("summary-run-impl-timing").is_none(),
            "prompts leave once results exist"
        );
    }

    #[test]
    fn tall_card_grid_stays_mounted_behind_the_scroll_viewport() {
        let mut rt = harness_with(impl_complete_state());
        rt.frame(0.0);
        assert!(rt.find("summary-scroll").is_some(), "cards must scroll");
        // Below-fold rows stay in the tree (reachable by scrolling) instead
        // of painting beneath the console dock.
        for key in [
            "summary-failing",
            "summary-timing-verdict",
            "summary-luts",
            "summary-power-total",
            "console",
            "console-tabs",
        ] {
            assert!(rt.find(key).is_some(), "missing key behind the fold: {key}");
        }
    }

    #[test]
    fn wheel_over_cards_scrolls_below_fold_into_view() {
        let mut rt = harness_with(impl_complete_state());
        rt.frame(0.0);
        let y_before = rt.presented("summary-power-total").unwrap().0[1];
        let vp = rt.presented("summary-scroll").unwrap().0;
        assert!(
            y_before > vp[1] + vp[3],
            "power total must start below the fold (y={y_before}, fold={})",
            vp[1] + vp[3]
        );
        let (wx, wy) = (vp[0] + vp[2] / 2.0, vp[1] + vp[3] / 2.0);
        for i in 0..10 {
            rt.wheel(wx, wy, -140.0);
            rt.frame(0.1 + i as f64 * (1.0 / 60.0));
        }
        let y_after = rt.presented("summary-power-total").unwrap().0[1];
        assert!(
            y_after < y_before - 100.0,
            "wheel must move content up (before={y_before}, after={y_after})"
        );
    }

    #[test]
    fn formatters_match_vivado_text() {
        assert_eq!(format_ns(2061), "2.061 ns");
        assert_eq!(format_ns(-1545), "-1.545 ns");
        assert_eq!(format_ns(0), "0.000 ns");
        assert_eq!(format_watts(142), "0.142 W");
        assert_eq!(pct(1523, 17600), 9);
        assert_eq!(pct(1, 0), 0);
        assert_eq!(format_util(1523, 17600), "1523 (9%)");
    }

    #[test]
    fn running_impl_shows_status_not_a_second_prompt() {
        // deep-05: result cards reflect the live run state instead of
        // offering to start a second run while one is active.
        let mut s = init();
        s.flow.impl_run = RunStatus::Running;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        for key in [
            "summary-run-impl-drc",
            "summary-run-impl-timing",
            "summary-run-impl-util",
            "summary-run-impl-power",
        ] {
            assert!(
                rt.find(key).is_none(),
                "prompt {key} must hide while running"
            );
            assert!(
                rt.find(&format!("{key}-status")).is_some(),
                "live status for {key} must show"
            );
        }
    }

    #[test]
    fn failed_impl_points_at_messages() {
        // deep-05: a failed run shows the failure on the cards, not a
        // fresh-run prompt.
        let mut s = init();
        s.flow.impl_run = RunStatus::Failed;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("summary-run-impl-timing").is_none(),
            "prompt must hide"
        );
        assert!(
            rt.find("summary-run-impl-timing-status").is_some(),
            "failure must show"
        );
    }

    #[test]
    fn qor_and_bitstream_cards_follow_live_run_state() {
        // deep-05: the QoR and Bitstream cards stay unevaluated until a
        // real run produces them.
        let mut rt = harness();
        rt.frame(0.0);
        for key in ["summary-qor", "summary-qor-open", "summary-bitstream"] {
            assert!(rt.find(key).is_some(), "missing card key: {key}");
        }
        for key in ["summary-qor-score", "summary-bitstream-idcode"] {
            assert!(rt.find(key).is_none(), "result {key} must wait for a run");
        }

        let mut s = impl_complete_state();
        crate::views::reports::demo_impl_complete(&mut s);
        s.reports_view.qor = Some(crate::views::reports::QorSnapshot {
            rqa_score: 4,
            wns_ps: 2061,
            tns_ps: 0,
            whs_ps: 154,
            endpoints: 112,
            suggestions: vec![crate::views::reports::QorSuggRow {
                id: "RQS_FANOUT-1".into(),
                category: "Congestion".into(),
                severity: "Advisory".into(),
                applicable_for: "synth_design".into(),
                tcl_command: "set_property MAX_FANOUT 32 [get_nets rst_n]".into(),
                target: "rst_n".into(),
                description: "Replicate high-fanout driver".into(),
                expected_gain_ps: 120,
            }],
            ..crate::views::reports::QorSnapshot::default()
        });
        let mut rt = harness_with(s);
        rt.frame(0.0);
        for key in [
            "summary-qor-score",
            "summary-qor-count",
            "summary-qor-top",
            "summary-bitstream-idcode",
            "summary-bitstream-signoff",
            "summary-bitstream-open",
        ] {
            assert!(rt.find(key).is_some(), "missing result key: {key}");
        }
    }

    #[test]
    fn summary_qor_open_selects_and_evaluates_the_qor_report() {
        // deep-05: the QoR card link lands on the QoR report with a real
        // engine evaluation behind it.
        let mut s = init();
        s.tcl.design = Some(
            helion_sv::synth_sv(crate::live::FIXTURE_SRC, "fixture.sv")
                .expect("fixture must elaborate"),
        );
        let mut rt = harness_tall(s);
        rt.frame(0.0);
        tap(&mut rt, "summary-qor-open");
        assert_eq!(rt.state.center, crate::state::CenterPane::Reports);
        assert_eq!(
            rt.state.reports_view.selected,
            crate::views::reports::ReportNav::Qor
        );
        assert!(rt.state.reports_view.qor.is_some(), "QoR must evaluate");
    }

    #[test]
    fn summary_bitstream_open_navigates_to_hardware_manager() {
        // deep-05: the Bitstream card hands the real artifact to the
        // Hardware Manager pane.
        let mut s = impl_complete_state();
        crate::views::reports::demo_impl_complete(&mut s);
        let mut rt = harness_tall(s);
        rt.frame(0.0);
        tap(&mut rt, "summary-bitstream-open");
        assert_eq!(rt.state.center, crate::state::CenterPane::Program);
    }

    #[test]
    fn power_card_shows_confidence_from_the_run() {
        // deep-05: the run-derived power confidence renders on the card.
        let mut s = impl_complete_state();
        crate::views::reports::demo_impl_complete(&mut s);
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("summary-power-confidence").is_some(),
            "confidence must show"
        );
        let mut rt = harness();
        rt.frame(0.0);
        assert!(
            rt.find("summary-power-confidence").is_none(),
            "prompt has no confidence"
        );
    }
}
