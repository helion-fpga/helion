//! Bottom console dock: Tcl Console / Messages / Log / Reports / Design Runs.
//!
//! New Rune `El` code written from scratch for the Vivado brief
//! (`/tmp/vivado-ref/rd-1b93965a*.png` Tcl Console dock,
//! `rd-29675bf9*.png` Log tab with its Synthesis/Implementation/Simulation
//! sub-tabs): the bottom dock with its tab strip, icon toolbar, monospace log
//! lines (`INFO: [Common 17-206]...`, `synth_design completed successfully`,
//! `X Infos, Y Warnings, ...` summaries), and a working Tcl input row. All
//! colors come from [`VivadoTheme`]; run state is the typed [`RunStatus`]
//! enum and severities are the typed [`MsgSev`] enum (no stringly status, no
//! hardcoded hex, no manual geometry, no unwraps on UI state).
//!
//! Data shapes mirror `helion-gui` read-only: [`ConsoleState`] (lines plus the
//! input edit) is the small console slice of [`AppState`], mirroring
//! `ide.rs` `ConsoleLine` + message severities.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el.rs`: `row()/col()/el()/text()/icon()`,
//!   `text_input(&TextEdit, style)`, `El::on_input_key`.
//! - `crates/rune/src/el/text.rs`: `.single_line()`, `.placeholder()`,
//!   `.caret()`, `.on_edit()`, `.on_submit()`, `.truncate()`.
//! - `crates/rune/src/edit.rs`: `TextEdit::new/replace_text/clear`.
//! - `crates/rune/src/edit/keys.rs`: `Key::Up/Down`, `KeyEvent`.
//! - `crates/rune/src/tok.rs`: `TextStyle::mono()` for the log face.
//! - `crates/rune-kit/src/glyphs.rs`: `SEARCH`, `COPY`, `CIRCLE_X`.

use rune::{El, Key, ScrollSpec, TextEdit, Tx, col, el, icon, row, text, text_input};
use rune_kit::glyphs;

use crate::state::{AppState, ConsoleLine, MsgSev, RunStatus};
use crate::theme::VivadoTheme;

/// Bottom dock height (the brief shows ~25% of the 900px window).
const CONSOLE_H: f32 = 216.0;
/// Tab strip height.
const TAB_H: f32 = 26.0;
/// Console toolbar height.
const TOOLBAR_H: f32 = 26.0;
/// Tcl input row height.
const INPUT_H: f32 = 26.0;
/// Log sub-tab strip height.
const SUBTAB_H: f32 = 24.0;
/// Mono log line height (compact CAD density).
const LINE_H: f32 = 18.0;

/// Bottom dock tabs, in Vivado order. Typed so selection is never a string.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConsoleTab {
    #[default]
    Tcl,
    Messages,
    Log,
    Reports,
    DesignRuns,
}

impl ConsoleTab {
    pub const ALL: [ConsoleTab; 5] = [
        ConsoleTab::Tcl,
        ConsoleTab::Messages,
        ConsoleTab::Log,
        ConsoleTab::Reports,
        ConsoleTab::DesignRuns,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ConsoleTab::Tcl => "Tcl Console",
            ConsoleTab::Messages => "Messages",
            ConsoleTab::Log => "Log",
            ConsoleTab::Reports => "Reports",
            ConsoleTab::DesignRuns => "Design Runs",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            ConsoleTab::Tcl => "console-tab-tcl",
            ConsoleTab::Messages => "console-tab-messages",
            ConsoleTab::Log => "console-tab-log",
            ConsoleTab::Reports => "console-tab-reports",
            ConsoleTab::DesignRuns => "console-tab-runs",
        }
    }
}

/// Log sub-tabs: which run's log the Log tab shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogRun {
    #[default]
    Synthesis,
    Implementation,
    Simulation,
}

impl LogRun {
    pub const ALL: [LogRun; 3] = [
        LogRun::Synthesis,
        LogRun::Implementation,
        LogRun::Simulation,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LogRun::Synthesis => "Synthesis",
            LogRun::Implementation => "Implementation",
            LogRun::Simulation => "Simulation",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            LogRun::Synthesis => "console-logrun-synth",
            LogRun::Implementation => "console-logrun-impl",
            LogRun::Simulation => "console-logrun-sim",
        }
    }
}

/// Severity ink for one console line.
fn sev_color(sev: MsgSev, t: &VivadoTheme) -> rune::Color {
    match sev {
        MsgSev::Info => t.text,
        MsgSev::Warn => t.warn,
        MsgSev::Error | MsgSev::Critical => t.fail,
    }
}

/// Counts of `(infos, warnings, criticals, errors)` folded from typed
/// severities, never parsed back out of strings.
pub fn message_counts(s: &AppState) -> (usize, usize, usize, usize) {
    let mut counts = (0, 0, 0, 0);
    for line in &s.console.lines {
        match line.sev {
            MsgSev::Info => counts.0 += 1,
            MsgSev::Warn => counts.1 += 1,
            MsgSev::Critical => counts.2 += 1,
            MsgSev::Error => counts.3 += 1,
        }
    }
    counts
}

/// Vivado-style `X Infos, Y Warnings, C Critical Warnings and K Errors`.
pub fn format_counts(infos: usize, warns: usize, crits: usize, errors: usize) -> String {
    format!("{infos} Infos, {warns} Warnings, {crits} Critical Warnings and {errors} Errors")
}

/// Submit the Tcl input row: evaluate the typed verb against the real Tcl
/// session ([`crate::tcl::eval_command`] echoes the verb, results, and
/// errors, and keeps history), then clear the edit. Empty input is a no-op.
pub fn submit_input(s: &mut AppState) {
    let verb = s.console.input.text.trim().to_string();
    if verb.is_empty() {
        return;
    }
    crate::tcl::eval_command(s, &verb);
    s.console.input.clear();
    s.console.input.set_caret(0);
}

/// Up/Down history recall for the Tcl input row: Up walks toward older
/// verbs (sticking at the oldest), Down walks back toward the live input
/// (clearing it past the newest). Empty history is a no-op. If the user
/// typed since recalling, the cursor is stale and recall restarts from
/// the newest verb.
pub fn recall_history(s: &mut AppState, older: bool) {
    let n = s.tcl.history.len();
    if n == 0 {
        return;
    }
    if let Some(i) = s.console.hist_pos {
        let stale = s.tcl.history.get(i).map(|h| h.as_str()) != Some(s.console.input.text.as_str());
        if stale {
            s.console.hist_pos = None;
        }
    }
    if older {
        let next = match s.console.hist_pos {
            None => n - 1,
            Some(i) => i.saturating_sub(1),
        };
        s.console.hist_pos = Some(next);
        let text = s.tcl.history[next].clone();
        s.console.input.replace_text(text);
    } else {
        match s.console.hist_pos {
            None => {}
            Some(i) if i + 1 < n => {
                s.console.hist_pos = Some(i + 1);
                let text = s.tcl.history[i + 1].clone();
                s.console.input.replace_text(text);
            }
            Some(_) => {
                s.console.hist_pos = None;
                s.console.input.clear();
            }
        }
    }
    let end = s.console.input.text.len();
    s.console.input.set_caret(end);
}

/// Echo a Tcl verb to the console without flipping run state.
fn echo(s: &mut AppState, tcl: &str) {
    s.console.lines.push(ConsoleLine {
        text: tcl.into(),
        sev: MsgSev::Info,
    });
}

/// Open a catalog report: Timing Summary navigates to Timing, the rest to
/// the Reports pane with the matching Utilization/Power/DRC tab selected.
/// Shared with the `open_report` Tcl command (hence `pub(crate)` + `&str`).
pub(crate) fn open_report(s: &mut AppState, i: usize, tcl: &str) {
    use crate::state::CenterPane;
    use crate::views::reports::ReportNav;
    match i {
        0 => s.navigate(CenterPane::Timing),
        1 => {
            s.navigate(CenterPane::Reports);
            s.reports_view.selected = ReportNav::Utilization;
        }
        2 => {
            s.navigate(CenterPane::Reports);
            s.reports_view.selected = ReportNav::Power;
        }
        3 => {
            s.navigate(CenterPane::Reports);
            s.reports_view.selected = ReportNav::Drc;
        }
        _ => {
            s.navigate(CenterPane::Reports);
            s.reports_view.selected = ReportNav::Utilization;
        }
    }
    echo(s, tcl);
}

/// Whether a run has started (typed state, never parsed).
fn run_started(status: RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Running | RunStatus::Complete | RunStatus::Failed
    )
}

/// One monospace log line at CAD density.
fn log_line(n: usize, content: &str, sev: MsgSev, t: &VivadoTheme) -> El {
    row()
        .key(format!("console-line-{n}"))
        .w_full()
        .h(LINE_H)
        .items_center()
        .px(8.0)
        .child(
            text(content.to_string())
                .key(format!("console-line-{n}-text"))
                .font(VivadoTheme::FONT_MONO.mono())
                .color(sev_color(sev, t))
                .grow(1.0)
                .shrink()
                .truncate(),
        )
}

/// Tab strip: Tcl Console / Messages / Log / Reports / Design Runs.
fn tab_strip(s: &AppState, t: &VivadoTheme) -> El {
    let mut strip = row()
        .key("console-tabs")
        .w_full()
        .h(TAB_H)
        .items_center()
        .gap(2.0)
        .px(4.0)
        .fill(t.header_bg)
        .border(1.0, t.border_light);
    for tab in ConsoleTab::ALL {
        let active = s.console.tab == tab;
        let style = if active {
            VivadoTheme::FONT_TITLE
        } else {
            VivadoTheme::FONT_UI
        };
        let bg = if active { t.panel_bg } else { t.header_bg };
        strip = strip.child(
            el().key(tab.key())
                .role("button")
                .label(tab.label())
                .focusable(true)
                .h(TAB_H - 4.0)
                .px(10.0)
                .center_y()
                .fill(bg)
                .hover(t.selection)
                .on_press(move |s: &mut AppState| s.console.tab = tab)
                .child(text(tab.label()).font(style).color(t.text)),
        );
    }
    strip = strip.child(
        el().key("console-collapse")
            .role("button")
            .label(if s.console.collapsed {
                "Expand console"
            } else {
                "Collapse console"
            })
            .focusable(true)
            .h(TAB_H - 4.0)
            .px(10.0)
            .center_y()
            .hover(t.selection)
            .on_press(|s: &mut AppState| s.console.collapsed = !s.console.collapsed)
            .child(
                text(if s.console.collapsed { "▴" } else { "▾" })
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_dim),
            ),
    );
    strip
}

/// Small toolbar icon button echoing a Tcl verb.
fn tool_btn(
    key: &'static str,
    tip: &'static str,
    tcl: &'static str,
    t: &VivadoTheme,
    glyph: El,
) -> El {
    el().key(key)
        .role("button")
        .label(tip)
        .focusable(true)
        .size(22.0, 22.0)
        .center()
        .hover(t.selection)
        .on_press(move |s: &mut AppState| echo(s, tcl))
        .child(glyph)
}

/// Console toolbar: search, copy, save, and a real Clear action.
fn console_toolbar(t: &VivadoTheme) -> El {
    row()
        .key("console-toolbar")
        .role("toolbar")
        .w_full()
        .h(TOOLBAR_H)
        .items_center()
        .gap(2.0)
        .px(6.0)
        .fill(t.panel_bg)
        .child(tool_btn(
            "console-search",
            "Find in console",
            "search_log",
            t,
            icon(&glyphs::SEARCH, 13.0).color(t.text_dim).a11y_hidden(),
        ))
        .child(tool_btn(
            "console-copy",
            "Copy console output",
            "copy_log",
            t,
            icon(&glyphs::COPY, 13.0).color(t.text_dim).a11y_hidden(),
        ))
        .child(
            el().key("console-clear")
                .role("button")
                .label("Clear console")
                .focusable(true)
                .size(22.0, 22.0)
                .center()
                .hover(t.selection)
                .on_press(|s: &mut AppState| s.console.lines.clear())
                .child(
                    icon(&glyphs::CIRCLE_X, 13.0)
                        .color(t.text_dim)
                        .a11y_hidden(),
                ),
        )
}

/// Tcl Console body: live mono lines plus the working input row.
fn tcl_body(s: &AppState, t: &VivadoTheme) -> El {
    let mut lines = col()
        .key("console-lines")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .py(4.0)
        .scroll(ScrollSpec { stick_bottom: true });
    if s.console.lines.is_empty() {
        lines = lines.child(
            row().w_full().h(LINE_H).items_center().px(8.0).child(
                text("(console is empty - type a Tcl command below)")
                    .font(VivadoTheme::FONT_MONO.mono())
                    .color(t.text_faint),
            ),
        );
    } else {
        for (n, line) in s.console.lines.iter().enumerate() {
            lines = lines.child(log_line(n, &line.text, line.sev, t));
        }
    }
    let input: &TextEdit = &s.console.input;
    col().w_full().grow(1.0).min_h(0.0).child(lines).child(
        row()
            .key("console-input-row")
            .w_full()
            .h(INPUT_H)
            .items_center()
            .gap(6.0)
            .px(8.0)
            .fill(t.panel_bg)
            .border(1.0, t.border_light)
            .child(
                text("tcl>")
                    .font(VivadoTheme::FONT_MONO.mono())
                    .color(t.text_dim)
                    .a11y_hidden(),
            )
            .child(
                text_input(input, VivadoTheme::FONT_MONO.mono())
                    .single_line()
                    .key("console-input")
                    .label("Tcl command input")
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .color(t.text)
                    .placeholder("Type a Tcl command, e.g. launch_runs synth_1", t.text_faint)
                    .caret(t.text, t.selection)
                    .on_edit(|s: &mut AppState| &mut s.console.input)
                    .on_submit(|s: &mut AppState, _tx: &mut Tx| submit_input(s))
                    .on_input_key(
                        |s: &mut AppState, key: &rune::KeyEvent, _tx: &mut Tx| match key.key {
                            Key::Up => {
                                recall_history(s, true);
                                true
                            }
                            Key::Down => {
                                recall_history(s, false);
                                true
                            }
                            _ => false,
                        },
                    ),
            ),
    )
}

/// Messages body: derived counts plus the warning/error lines.
fn messages_body(s: &AppState, t: &VivadoTheme) -> El {
    let (infos, warns, crits, errors) = message_counts(s);
    let mut body = col()
        .key("console-messages")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .py(4.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(
            row().w_full().h(LINE_H).items_center().px(8.0).child(
                text(format!(
                    "{} encountered.",
                    format_counts(infos, warns, crits, errors)
                ))
                .font(VivadoTheme::FONT_MONO.mono())
                .color(t.text)
                .key("console-msg-summary"),
            ),
        );
    let mut n = 0;
    for line in s.console.lines.iter().filter(|l| l.sev != MsgSev::Info) {
        body = body.child(log_line(n, &line.text, line.sev, t));
        n += 1;
    }
    if n == 0 {
        body = body.child(
            row().w_full().h(LINE_H).items_center().px(8.0).child(
                text("(No warnings or errors)")
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_faint),
            ),
        );
    }
    body
}

/// Prompt shown when a sim run has no log output yet. The link launches
/// the selected testbench on the real engine worker; without RTL the tab
/// names the blocker instead of offering a dead link.
fn sim_run_prompt(t: &VivadoTheme) -> El {
    col().key("console-log-prompt").w_full().padding(8.0).child(
        row()
            .w_full()
            .h(40.0)
            .items_center()
            .justify_center()
            .gap(4.0)
            .child(
                el().key("console-log-prompt-link")
                    .role("button")
                    .label("Run Simulation")
                    .focusable(true)
                    .h(20.0)
                    .px(2.0)
                    .center_y()
                    .hover(t.selection)
                    .on_press(|s: &mut AppState| {
                        if let Err(e) = crate::views::wave::launch_default_tb(s) {
                            s.console.lines.push(crate::state::ConsoleLine {
                                text: format!("ERROR: {e}"),
                                sev: crate::state::MsgSev::Error,
                            });
                            s.toasts.push(
                                crate::state::ToastKind::Error,
                                "Cannot start simulation",
                                &e,
                            );
                        }
                    })
                    .child(
                        text("Run Simulation")
                            .font(VivadoTheme::FONT_UI)
                            .color(t.link),
                    ),
            )
            .child(
                text("to see simulation log".to_string())
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_dim),
            ),
    )
}

/// Prompt shown when a run has no log output yet. The link starts the real
/// run.
fn run_prompt(
    key: &'static str,
    verb: &'static str,
    noun: &'static str,
    kind: crate::runs::RunKind,
    t: &VivadoTheme,
) -> El {
    col().key(key).w_full().padding(8.0).child(
        row()
            .w_full()
            .h(40.0)
            .items_center()
            .justify_center()
            .gap(4.0)
            .child(
                el().key(format!("{key}-link"))
                    .role("button")
                    .label(verb)
                    .focusable(true)
                    .h(20.0)
                    .px(2.0)
                    .center_y()
                    .hover(t.selection)
                    .on_press(move |s: &mut AppState| crate::runs::launch(s, kind))
                    .child(text(verb).font(VivadoTheme::FONT_UI).color(t.link)),
            )
            .child(
                text(format!("to see {noun} results"))
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_dim),
            ),
    )
}

/// Honest empty note for log tabs with no output (never canned log text).
fn empty_note(key: &'static str, note: &'static str, t: &VivadoTheme) -> El {
    row()
        .key(key)
        .w_full()
        .h(LINE_H)
        .items_center()
        .px(8.0)
        .child(text(note).font(VivadoTheme::FONT_UI).color(t.text_faint))
}

/// Log body: real buffered per-run lines plus the
/// Synthesis/Implementation/Simulation sub-tabs.
fn log_body(s: &AppState, t: &VivadoTheme) -> El {
    let run = s.console.log_run;
    let mut content = col()
        .key("console-log-lines")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .py(4.0)
        .scroll(ScrollSpec { stick_bottom: true });
    match run {
        LogRun::Simulation => {
            if s.sim.log.is_empty() {
                if s.sim.is_active() || run_started(s.flow.sim) {
                    content = content.child(empty_note(
                        "console-log-empty",
                        "(simulation running — streaming…)",
                        t,
                    ));
                } else if s.editor.lines.iter().all(|l| l.trim().is_empty())
                    && s.tcl.design.is_none()
                {
                    content = content.child(empty_note(
                        "console-log-empty",
                        "(no RTL source — open a source to simulate)",
                        t,
                    ));
                } else {
                    content = content.child(sim_run_prompt(t));
                }
            } else {
                for (n, line) in s.sim.log.iter().enumerate() {
                    content = content.child(log_line(n, &line.text, line.sev, t));
                }
            }
        }
        LogRun::Synthesis | LogRun::Implementation => {
            let (status, buf, kind) = match run {
                LogRun::Synthesis => (s.flow.synth, &s.runs.synth_log, crate::runs::RunKind::Synth),
                _ => (
                    s.flow.impl_run,
                    &s.runs.impl_log,
                    crate::runs::RunKind::Impl,
                ),
            };
            if buf.is_empty() {
                if run_started(status) {
                    content = content.child(empty_note(
                        "console-log-empty",
                        "(no log output captured for this run)",
                        t,
                    ));
                } else {
                    let (verb, noun) = match run {
                        LogRun::Synthesis => ("Run Synthesis", "synthesis log"),
                        _ => ("Run Implementation", "implementation log"),
                    };
                    content = content.child(run_prompt("console-log-prompt", verb, noun, kind, t));
                }
            } else {
                for (n, line) in buf.iter().enumerate() {
                    content = content.child(log_line(n, &line.text, line.sev, t));
                }
            }
        }
    }
    let mut subtabs = row()
        .key("console-logruns")
        .w_full()
        .h(SUBTAB_H)
        .items_center()
        .gap(2.0)
        .px(4.0)
        .fill(t.panel_bg)
        .border(1.0, t.border_light);
    for r in LogRun::ALL {
        let active = s.console.log_run == r;
        let style = if active {
            VivadoTheme::FONT_TITLE
        } else {
            VivadoTheme::FONT_UI
        };
        subtabs = subtabs.child(
            el().key(r.key())
                .role("button")
                .label(format!("{} log", r.label()))
                .focusable(true)
                .h(SUBTAB_H - 4.0)
                .px(10.0)
                .center_y()
                .fill(if active { t.selection } else { t.panel_bg })
                .hover(t.selection)
                .on_press(move |s: &mut AppState| s.console.log_run = r)
                .child(text(r.label()).font(style).color(t.text)),
        );
    }
    col()
        .key("console-log")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(content)
        .child(subtabs)
}

/// Report catalog entries: display name plus the Tcl verb opening them.
const REPORTS: &[(&str, &str)] = &[
    ("Timing Summary", "open_report timing"),
    ("Utilization", "open_report utilization"),
    ("Power", "open_report power"),
    ("DRC", "open_report drc"),
    ("Methodology", "open_report methodology"),
];

/// Reports body: catalog of report links navigating to Timing/Reports.
fn reports_body(t: &VivadoTheme) -> El {
    let mut body = col()
        .key("console-reports")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .py(4.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    for (i, (name, tcl)) in REPORTS.iter().enumerate() {
        body = body.child(
            row()
                .key(format!("console-report-{i}"))
                .w_full()
                .h(20.0)
                .items_center()
                .gap(8.0)
                .px(8.0)
                .child(
                    el().key(format!("console-report-link-{i}"))
                        .role("button")
                        .label(format!("Open {name} report"))
                        .focusable(true)
                        .h(20.0)
                        .px(2.0)
                        .center_y()
                        .hover(t.selection)
                        .on_press(move |s: &mut AppState| open_report(s, i, tcl))
                        .child(text(*name).font(VivadoTheme::FONT_UI).color(t.link)),
                )
                .child(
                    text("Report")
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text_faint),
                ),
        );
    }
    body
}

/// One Design Runs table row, derived from typed flow state, with a live
/// Run/Cancel action: Cancel while Running, Run otherwise. The action dims
/// when it cannot act at once (another run owns the worker); pressing it
/// still explains why via toast instead of failing silently.
fn run_row(
    key: &'static str,
    name: &'static str,
    status: RunStatus,
    part: &str,
    strategy: &str,
    kind: crate::runs::RunKind,
    active: Option<crate::runs::RunKind>,
    t: &VivadoTheme,
) -> El {
    let color = match status {
        RunStatus::Complete => t.pass,
        RunStatus::Failed => t.fail,
        RunStatus::Running => t.link,
        RunStatus::NotStarted => t.text,
    };
    let running = status == RunStatus::Running;
    let (action, ink) = if running {
        (
            "Cancel",
            if active == Some(kind) {
                t.link
            } else {
                t.text_faint
            },
        )
    } else if active.is_some() {
        ("Run", t.text_faint)
    } else {
        ("Run", t.link)
    };
    row()
        .key(key)
        .w_full()
        .h(20.0)
        .items_center()
        .gap(6.0)
        .px(8.0)
        .child(
            el().w(90.0)
                .child(text(name).font(VivadoTheme::FONT_UI).color(t.text)),
        )
        .child(
            el().w(100.0).child(
                text(status.label())
                    .font(VivadoTheme::FONT_UI)
                    .color(color)
                    .key(format!("{key}-status")),
            ),
        )
        .child(
            el().w(150.0)
                .child(text(part).font(VivadoTheme::FONT_UI).color(t.text_dim)),
        )
        .child(
            text(strategy.to_string())
                .font(VivadoTheme::FONT_UI)
                .color(t.link)
                .grow(1.0)
                .shrink()
                .truncate(),
        )
        .child(
            el().key(format!("{key}-action"))
                .role("button")
                .label(format!("{action} {name}"))
                .focusable(true)
                .w(64.0)
                .h(20.0)
                .center_y()
                .hover(t.selection)
                .on_press(move |s: &mut AppState| {
                    if running {
                        crate::runs::cancel_run(s);
                    } else {
                        crate::runs::launch(s, kind);
                    }
                })
                .child(text(action).font(VivadoTheme::FONT_UI).color(ink)),
        )
}

/// Design Runs body: run table with live typed statuses.
fn runs_body(s: &AppState, t: &VivadoTheme) -> El {
    let active = s.runs.active_kind();
    col()
        .key("console-runs")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .py(4.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(
            row()
                .w_full()
                .h(20.0)
                .items_center()
                .gap(6.0)
                .px(8.0)
                .child(
                    el().w(90.0)
                        .child(text("Name").font(VivadoTheme::FONT_UI).color(t.text_faint)),
                )
                .child(
                    el().w(100.0).child(
                        text("Status")
                            .font(VivadoTheme::FONT_UI)
                            .color(t.text_faint),
                    ),
                )
                .child(
                    el().w(150.0)
                        .child(text("Part").font(VivadoTheme::FONT_UI).color(t.text_faint)),
                )
                .child(
                    text("Strategy")
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text_faint)
                        .grow(1.0)
                        .shrink(),
                )
                .child(
                    el().w(64.0).child(
                        text("Action")
                            .font(VivadoTheme::FONT_UI)
                            .color(t.text_faint),
                    ),
                ),
        )
        .child(run_row(
            "console-run-synth",
            "synth_1",
            s.flow.synth,
            &s.project.part,
            &s.flow.synth_strategy,
            crate::runs::RunKind::Synth,
            active,
            t,
        ))
        .child(run_row(
            "console-run-impl",
            "impl_1",
            s.flow.impl_run,
            &s.project.part,
            &s.flow.impl_strategy,
            crate::runs::RunKind::Impl,
            active,
            t,
        ))
}

/// The bottom console dock wired into the shell.
pub fn console_pane(s: &AppState, t: &VivadoTheme) -> El {
    let mut pane = col()
        .key("console")
        .w_full()
        .h(if s.console.collapsed {
            TAB_H
        } else {
            CONSOLE_H
        })
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(tab_strip(s, t));
    if s.console.collapsed {
        return pane;
    }
    pane = pane.child(console_toolbar(t));
    pane = pane.child(match s.console.tab {
        ConsoleTab::Tcl => tcl_body(s, t),
        ConsoleTab::Messages => messages_body(s, t),
        ConsoleTab::Log => log_body(s, t),
        ConsoleTab::Reports => reports_body(t),
        ConsoleTab::DesignRuns => runs_body(s, t),
    });
    pane
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

    #[test]
    fn console_tree_exposes_tabs_toolbar_lines_and_input() {
        let mut rt = harness_with(init());
        rt.frame(0.0);
        for key in [
            "console",
            "console-tabs",
            "console-tab-tcl",
            "console-tab-messages",
            "console-tab-log",
            "console-tab-reports",
            "console-tab-runs",
            "console-toolbar",
            "console-lines",
            "console-input-row",
            "console-input",
        ] {
            assert!(rt.find(key).is_some(), "missing keyed region: {key}");
        }
    }

    #[test]
    fn each_tab_switches_its_body() {
        let mut rt = harness_with(init());
        rt.frame(0.0);
        tap(&mut rt, "console-tab-messages");
        assert_eq!(rt.state.console.tab, ConsoleTab::Messages);
        assert!(rt.find("console-messages").is_some());
        assert!(rt.find("console-msg-summary").is_some());
        tap(&mut rt, "console-tab-log");
        assert_eq!(rt.state.console.tab, ConsoleTab::Log);
        assert!(rt.find("console-log").is_some());
        assert!(rt.find("console-logruns").is_some());
        tap(&mut rt, "console-tab-reports");
        assert_eq!(rt.state.console.tab, ConsoleTab::Reports);
        assert!(rt.find("console-reports").is_some());
        assert!(rt.find("console-report-link-0").is_some());
        tap(&mut rt, "console-tab-runs");
        assert_eq!(rt.state.console.tab, ConsoleTab::DesignRuns);
        assert!(rt.find("console-runs").is_some());
        assert!(rt.find("console-run-synth").is_some());
        assert!(rt.find("console-run-impl").is_some());
    }

    #[test]
    fn log_run_subtabs_switch_the_run() {
        let mut s = init();
        s.console.tab = ConsoleTab::Log;
        s.flow.synth = RunStatus::Complete;
        s.flow.impl_run = RunStatus::Running;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "console-logrun-impl");
        assert_eq!(rt.state.console.log_run, LogRun::Implementation);
        tap(&mut rt, "console-logrun-sim");
        assert_eq!(rt.state.console.log_run, LogRun::Simulation);
    }

    #[test]
    fn clear_button_empties_the_console() {
        let mut rt = harness_with(init());
        rt.frame(0.0);
        assert!(!rt.state.console.lines.is_empty());
        tap(&mut rt, "console-clear");
        assert!(rt.state.console.lines.is_empty());
    }

    #[test]
    fn submit_input_appends_the_verb_and_clears() {
        let mut s = init();
        s.console.input = TextEdit::new("launch_runs synth_1");
        submit_input(&mut s);
        assert!(
            s.console
                .lines
                .iter()
                .any(|l| l.text == "launch_runs synth_1")
        );
        assert!(s.console.input.text.is_empty());
        let len = s.console.lines.len();
        s.console.input = TextEdit::new("   ");
        submit_input(&mut s);
        assert_eq!(s.console.lines.len(), len, "blank input submits nothing");
    }

    #[test]
    fn message_counts_derive_from_typed_severities() {
        let mut s = init();
        s.console.lines.push(ConsoleLine {
            text: "w".into(),
            sev: MsgSev::Warn,
        });
        s.console.lines.push(ConsoleLine {
            text: "c".into(),
            sev: MsgSev::Critical,
        });
        s.console.lines.push(ConsoleLine {
            text: "e".into(),
            sev: MsgSev::Error,
        });
        let (infos, warns, crits, errors) = message_counts(&s);
        assert_eq!((warns, crits, errors), (1, 1, 1));
        assert_eq!(
            format_counts(infos, warns, crits, errors),
            format!("{infos} Infos, 1 Warnings, 1 Critical Warnings and 1 Errors")
        );
    }

    #[test]
    fn design_runs_table_reflects_flow_state() {
        let mut s = init();
        s.console.tab = ConsoleTab::DesignRuns;
        s.flow.synth = RunStatus::Complete;
        s.flow.impl_run = RunStatus::Running;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("console-run-synth-status").is_some());
        assert!(rt.find("console-run-impl-status").is_some());
    }

    fn shown_text(rt: &Runtime<AppState>, key: &str) -> String {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .and_then(|e| e.text)
            .unwrap_or_default()
    }

    fn counter_source() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv")
    }

    fn pump_until_idle(s: &mut AppState) {
        let start = std::time::Instant::now();
        while s.runs.is_active() {
            crate::runs::poll_runs(s);
            if !s.runs.is_active() {
                break;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(60),
                "run timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        crate::runs::poll_runs(s);
    }

    #[test]
    fn runs_table_run_action_launches_a_real_run() {
        let mut s = init();
        s.console.tab = ConsoleTab::DesignRuns;
        s.runs.source = Some(counter_source());
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "console-run-synth-action");
        assert_eq!(rt.state.flow.synth, RunStatus::Running);
        assert!(rt.state.runs.is_active(), "action must arm the worker");
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "launch_runs synth_1"),
            "action must echo its Tcl verb"
        );
        crate::runs::cancel_run(&mut rt.state);
        pump_until_idle(&mut rt.state);
    }

    #[test]
    fn runs_table_cancel_action_without_worker_toasts_no_active_run() {
        let mut s = init();
        s.console.tab = ConsoleTab::DesignRuns;
        s.flow.synth = RunStatus::Running;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "console-run-synth-action");
        assert!(
            rt.state
                .toasts
                .items
                .iter()
                .any(|t| t.title == "No active run"),
            "cancel with no worker must say so"
        );
    }

    #[test]
    fn log_tab_renders_real_buffered_lines() {
        let mut s = init();
        s.console.tab = ConsoleTab::Log;
        crate::runs::deliver(
            &mut s,
            crate::runs::RunKind::Synth,
            crate::runs::RunEvent::Line {
                text: "synth_design cells=12".into(),
                sev: MsgSev::Info,
            },
        );
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert_eq!(
            shown_text(&rt, "console-line-0-text"),
            "synth_design cells=12"
        );
    }

    #[test]
    fn log_tab_with_no_output_shows_empty_note() {
        let mut s = init();
        s.console.tab = ConsoleTab::Log;
        s.flow.synth = RunStatus::Running;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("console-log-empty").is_some(),
            "empty log must show its note"
        );
    }

    #[test]
    fn submit_input_evaluates_tcl_and_echoes_the_result() {
        let mut s = init();
        s.console.input = TextEdit::new("expr {7 * 6}");
        submit_input(&mut s);
        assert!(
            s.console.lines.iter().any(|l| l.text == "expr {7 * 6}"),
            "verb must echo"
        );
        assert!(
            s.console.lines.iter().any(|l| l.text == "42"),
            "result must echo"
        );
        assert!(s.console.input.text.is_empty(), "input must clear");
    }

    #[test]
    fn submit_input_keeps_history_and_errors_on_unknown_verbs() {
        let mut s = init();
        s.console.input = TextEdit::new("bogus_cmd");
        submit_input(&mut s);
        assert_eq!(s.tcl.history, vec!["bogus_cmd".to_string()]);
        assert!(
            s.console.lines.iter().any(|l| l.sev == MsgSev::Error),
            "unknown verbs must error, not silently echo"
        );
    }

    #[test]
    fn submit_launch_runs_starts_a_real_run() {
        let mut s = init();
        s.runs.source = Some(counter_source());
        s.console.input = TextEdit::new("launch_runs synth_1");
        submit_input(&mut s);
        assert!(s.runs.is_active(), "launch_runs verb must arm the worker");
        assert_eq!(s.flow.synth, RunStatus::Running);
        crate::runs::cancel_run(&mut s);
        pump_until_idle(&mut s);
    }

    #[test]
    fn sim_log_tab_prompts_then_streams_the_engine_run() {
        let mut s = init();
        s.console.tab = ConsoleTab::Log;
        s.console.log_run = LogRun::Simulation;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("console-log-prompt-link").is_some(),
            "sim tab must offer its run"
        );
        tap(&mut rt, "console-log-prompt-link");
        assert!(rt.state.sim.is_active(), "prompt must arm the sim worker");
        assert_eq!(rt.state.flow.sim, RunStatus::Running);
        let start = std::time::Instant::now();
        while rt.state.sim.is_active() {
            crate::views::wave::poll_sim(&mut rt.state);
            assert!(
                start.elapsed() < std::time::Duration::from_secs(120),
                "sim timed out"
            );
            if rt.state.sim.is_active() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        crate::views::wave::poll_sim(&mut rt.state);
        // `frame` re-renders only on tracked (handler-driven) state changes,
        // so rebuild the harness around the drained state to assert pixels.
        let drained = rt.state.clone();
        assert!(!drained.sim.log.is_empty(), "engine must stream sim lines");
        let mut rt = harness_with(drained);
        rt.frame(0.0);
        assert!(
            rt.find("console-log-prompt").is_none(),
            "prompt must leave once lines stream"
        );
        assert_eq!(
            shown_text(&rt, "console-line-0-text"),
            rt.state.sim.log[0].text
        );
    }

    #[test]
    fn sim_log_tab_without_rtl_names_the_blocker() {
        let mut s = init();
        s.console.tab = ConsoleTab::Log;
        s.console.log_run = LogRun::Simulation;
        s.editor.lines.clear();
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("console-log-empty").is_some(),
            "no-RTL sim must name its blocker"
        );
        assert!(
            rt.find("console-log-prompt").is_none(),
            "no dead run link without RTL"
        );
    }

    #[test]
    fn history_recall_walks_oldest_to_live_input() {
        let mut s = init();
        crate::tcl::eval_command(&mut s, "set a 1");
        crate::tcl::eval_command(&mut s, "set b 2");
        assert_eq!(s.console.hist_pos, None, "submit resets the cursor");
        recall_history(&mut s, true);
        assert_eq!(s.console.input.text, "set b 2");
        recall_history(&mut s, true);
        assert_eq!(s.console.input.text, "set a 1");
        recall_history(&mut s, true);
        assert_eq!(s.console.input.text, "set a 1", "sticks at the oldest");
        recall_history(&mut s, false);
        assert_eq!(s.console.input.text, "set b 2");
        recall_history(&mut s, false);
        assert!(s.console.input.text.is_empty(), "past newest clears");
        assert_eq!(s.console.hist_pos, None);
    }

    #[test]
    fn history_recall_restarts_after_typing_and_ignores_empty() {
        let mut s = init();
        recall_history(&mut s, true);
        assert!(s.console.input.text.is_empty(), "empty history is a no-op");
        crate::tcl::eval_command(&mut s, "set a 1");
        crate::tcl::eval_command(&mut s, "set b 2");
        recall_history(&mut s, true);
        assert_eq!(s.console.input.text, "set b 2");
        s.console.input = TextEdit::new("set b 2; # typed");
        recall_history(&mut s, true);
        assert_eq!(s.console.input.text, "set b 2", "stale cursor restarts");
    }

    #[test]
    fn up_key_in_the_input_recalls_the_last_verb() {
        use rune::{KeyEvent, Mods};
        let mut s = init();
        crate::tcl::eval_command(&mut s, "expr {1 + 1}");
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "console-input");
        rt.key(KeyEvent {
            key: Key::Up,
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.console.input.text, "expr {1 + 1}");
        rt.key(KeyEvent {
            key: Key::Down,
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
        assert!(
            rt.state.console.input.text.is_empty(),
            "Down past newest clears"
        );
    }

    #[test]
    fn collapse_toggle_folds_the_dock_to_its_tab_strip() {
        let mut rt = harness_with(init());
        rt.frame(0.0);
        assert!(rt.find("console-input").is_some());
        tap(&mut rt, "console-collapse");
        assert!(rt.state.console.collapsed);
        assert!(
            rt.find("console-input").is_none(),
            "collapsed dock hides the transcript"
        );
        // Dock height animates: settle before measuring the rest height.
        for i in 0..40 {
            rt.frame(0.1 + i as f64 * (1.0 / 60.0));
        }
        let h = rt.presented("console").unwrap().0[3];
        assert!(h < 60.0, "collapsed dock is a strip, got {h}");
        tap(&mut rt, "console-collapse");
        assert!(!rt.state.console.collapsed);
        assert!(
            rt.find("console-input").is_some(),
            "toggle restores the transcript"
        );
    }

    #[test]
    fn wheel_over_transcript_scrolls_tall_logs() {
        let mut s = init();
        for i in 0..60 {
            s.console.lines.push(ConsoleLine {
                text: format!("line {i}"),
                sev: MsgSev::Info,
            });
        }
        let mut rt = harness_with(s);
        rt.frame(0.0);
        let vp = rt.presented("console-lines").unwrap().0;
        // stick_bottom pins the first frame to the live tail: line 0 sits
        // above the viewport and wheeling up must bring it back down.
        let y_before = rt.presented("console-line-0").unwrap().0[1];
        assert!(
            y_before < vp[1],
            "line 0 must start above the viewport (y={y_before}, top={})",
            vp[1]
        );
        let (wx, wy) = (vp[0] + vp[2] / 2.0, vp[1] + vp[3] / 2.0);
        for i in 0..10 {
            rt.wheel(wx, wy, 140.0);
            rt.frame(0.1 + i as f64 * (1.0 / 60.0));
        }
        let y_after = rt.presented("console-line-0").unwrap().0[1];
        assert!(
            y_after > y_before + 100.0,
            "wheel must move content down (before={y_before}, after={y_after})"
        );
    }
}
