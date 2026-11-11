//! Sim surface (`CenterPane::Sim`): the testbench runner.
//!
//! Layout: a toolbar (Rescan, Run all, Stop, VCD dump, Compile, Elaborate,
//! live status), then a left column with the discovered testbenches over
//! the collapsible Simulation Settings, and a right column with the
//! selected testbench's detail (facts, failures that jump the editor, the
//! run log) over the persisted run-history table. State and engine glue live
//! in [`crate::views::tb`]; every control calls into it for real work.

use std::path::Path;

use rune::tok::TextStyle;
use rune::{El, ScrollSpec, col, el, row, text};

use crate::state::{AppState, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::sources::pane_header;
use crate::views::tb::{self, RunTarget, TbItem, TbStatus};

/// Compact row height.
const ROW_H: f32 = 20.0;
/// Testbench list row height (name + badge).
const LIST_ROW_H: f32 = 24.0;
/// Toolbar height.
const TOOL_H: f32 = 26.0;
/// Left column width (testbenches + settings).
const LEFT_W: f32 = 300.0;
/// Run-history panel height.
const HISTORY_H: f32 = 148.0;
/// Detail label column width.
const LABEL_W: f32 = 88.0;
/// Log lines rendered in the detail panel (the rest stay in `run.log`).
const LOG_SHOWN: usize = 400;
/// Small caption type.
const FONT_PIN: TextStyle = TextStyle::new(10.0, 400.0);

fn echo(s: &mut AppState, line: String) {
    s.console.lines.push(ConsoleLine {
        text: line,
        sev: MsgSev::Info,
    });
}

fn report(s: &mut AppState, r: Result<String, String>, title: &str) {
    match r {
        Ok(line) => echo(s, line),
        Err(e) => {
            s.console.lines.push(ConsoleLine {
                text: format!("ERROR: {e}"),
                sev: MsgSev::Error,
            });
            s.toasts.push(crate::state::ToastKind::Error, title, &e);
        }
    }
}

fn ui_label(content: &str, t: &VivadoTheme) -> El {
    text(content.to_string())
        .font(VivadoTheme::FONT_UI)
        .color(t.text)
        .truncate()
        .help_when_truncated(content.to_string())
}

/// Toolbar button; disabled buttons carry the reason as description/help.
fn btn(key: &str, label: &str, t: &VivadoTheme, enabled: bool, why: &str) -> El {
    let b = row()
        .key(key.to_string())
        .role("button")
        .label(label.to_string())
        .h(20.0)
        .px(6.0)
        .items_center()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            text(label.to_string())
                .font(VivadoTheme::FONT_UI)
                .color(t.text),
        );
    if enabled {
        b.focusable(true).hover(t.selection)
    } else {
        b.disabled(true)
            .description(why.to_string())
            .help(why.to_string())
            .layer_opacity(0.45)
    }
}

/// Section title row (`Failures (2)`, `Log`, ...).
fn section_head(key: &str, title: &str, t: &VivadoTheme) -> El {
    row()
        .key(key.to_string())
        .w_full()
        .h(ROW_H)
        .px(8.0)
        .items_center()
        .fill(t.header_bg)
        .child(
            text(title.to_string())
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text),
        )
}

fn status_ink(status: TbStatus, t: &VivadoTheme) -> rune::Color {
    match status {
        TbStatus::Passed => t.pass,
        TbStatus::Failed => t.fail,
        TbStatus::Running => t.link,
        TbStatus::Stopped => t.warn,
        TbStatus::Idle => t.text_faint,
    }
}

fn busy_reason(s: &AppState) -> String {
    format!("{} is running — stop it first", s.sim.active_label())
}

/// Toolbar: discovery, batch runs, VCD dump, compile/elaborate, status.
fn sim_toolbar(s: &AppState, t: &VivadoTheme) -> El {
    let busy = s.sim.is_active();
    let has_tbs = !s.sim.tbs.is_empty();
    let vcd_label = if s.sim.vcd {
        "☑ VCD dump"
    } else {
        "☐ VCD dump"
    };
    let status = match s.sim.active_target() {
        Some(RunTarget::Testbench(_)) => format!(
            "Running {} · {} ns",
            s.sim.active_label(),
            s.sim.progress_ns
        ),
        Some(_) => format!("Running {}…", s.sim.active_label()),
        None => match &s.sim.scanned {
            Some(d) => format!("Testbenches in {}", d.display()),
            None => "No source directory".into(),
        },
    };
    row()
        .key("sim-toolbar")
        .w_full()
        .h(TOOL_H)
        .px(4.0)
        .gap(3.0)
        .items_center()
        .fill(t.header_bg)
        .child(
            btn("sim-discover", "↻ Rescan", t, true, "")
                .help("Discover *_tb.sv / tb_*.sv testbenches again")
                .on_press(|s: &mut AppState| {
                    let line = tb::rescan(s);
                    echo(s, line);
                }),
        )
        .child(
            btn(
                "sim-run-all",
                "▶ Run all",
                t,
                has_tbs && !busy,
                if busy {
                    "A simulation is running"
                } else {
                    "No testbenches discovered"
                },
            )
            .on_press(|s: &mut AppState| {
                let r = tb::run_all(s);
                report(s, r, "Cannot run testbenches");
            }),
        )
        .child(
            btn("sim-stop", "■ Stop", t, busy, "Nothing is running")
                .on_press(|s: &mut AppState| tb::cancel_sim(s)),
        )
        .child(
            btn("sim-vcd", vcd_label, t, true, "")
                .role("checkbox")
                .help("Write run.vcd from testbench runs (opens in the waveform viewer)")
                .on_press(|s: &mut AppState| {
                    s.sim.vcd = !s.sim.vcd;
                    let line = format!("tb_vcd {}", if s.sim.vcd { "on" } else { "off" });
                    echo(s, line);
                }),
        )
        .child(el().w(6.0))
        .child(
            btn("sim-compile", "Compile", t, !busy, &busy_reason(s))
                .help("Parse the RTL with helion-sv (INCLUDE_PATH/DEFINE apply)")
                .on_press(|s: &mut AppState| crate::tcl::eval_command(s, "compile_sim")),
        )
        .child(
            btn("sim-elaborate", "Elaborate", t, !busy, &busy_reason(s))
                .help("Elaborate SIM_TOP with PARAM and load it into helion-sim")
                .on_press(|s: &mut AppState| crate::tcl::eval_command(s, "elaborate_sim")),
        )
        .child(
            el().key("sim-status").grow(1.0).min_w(0.0).px(6.0).child(
                text(status.clone())
                    .font(VivadoTheme::FONT_UI)
                    .color(if busy { t.link } else { t.text_faint })
                    .truncate_middle()
                    .help_when_truncated(status),
            ),
        )
}

/// Empty list: one sentence and (with RTL available) the free-run action.
fn list_empty(s: &AppState, t: &VivadoTheme) -> El {
    let msg = match &s.sim.scanned {
        Some(d) => format!("No *_tb.sv or tb_*.sv files in {}.", d.display()),
        None => "Open a project or source file to discover its testbenches.".into(),
    };
    let ready = tb::can_launch(s);
    let why = ready.clone().err().unwrap_or_default();
    col()
        .key("sim-list-empty")
        .w_full()
        .padding(12.0)
        .gap(8.0)
        .child(
            text(msg)
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .wrap(),
        )
        .child(
            btn("sim-run-design", "▶ Run design", t, ready.is_ok(), &why)
                .help("Free-run the design for RUNTIME_CYCLES and show its waveform")
                .on_press(|s: &mut AppState| {
                    echo(s, "launch_simulation".into());
                    if let Err(e) = tb::launch_design(s) {
                        report(s, Err(e), "Cannot start simulation");
                    }
                }),
        )
}

/// Testbench list: discovered files with run-status badges.
fn tb_list(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col()
        .key("sim-list")
        .role("list")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(section_head(
            "sim-list-head",
            &format!("Testbenches ({})", s.sim.tbs.len()),
            t,
        ));
    if s.sim.tbs.is_empty() {
        return body.child(list_empty(s, t));
    }
    let mut rows = col().w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec {
        stick_bottom: false,
    });
    for (i, item) in s.sim.tbs.iter().enumerate() {
        let selected = s.sim.selected == Some(i);
        let status = item.status;
        let name = item.name().to_string();
        let badge = if item.info.is_err() && status == TbStatus::Idle {
            ("Parse error", t.fail)
        } else {
            (status.label(), status_ink(status, t))
        };
        rows = rows.child(
            row()
                .key(format!("sim-tb-{i}"))
                .role("option")
                .label(format!("{name} ({})", badge.0))
                .focus_selected(selected)
                .focusable(true)
                .w_full()
                .h(LIST_ROW_H)
                .px(8.0)
                .gap(8.0)
                .items_center()
                .fill(if selected { t.selection } else { t.panel_bg })
                .hover(t.selection)
                .on_press(move |s: &mut AppState| {
                    s.sim.selected = Some(i);
                    let n = s.sim.tbs[i].name().to_string();
                    echo(s, format!("tb_select {n}"));
                })
                .child(el().grow(1.0).min_w(0.0).child(ui_label(&name, t)))
                .child(
                    el().key(format!("sim-tb-status-{i}"))
                        .child(text(badge.0).font(FONT_PIN).color(badge.1)),
                ),
        );
    }
    body = body.child(rows);
    body
}

/// One label/value fact row (values truncate with a tooltip).
fn fact_row(key: &str, label: &str, value: &str, t: &VivadoTheme) -> El {
    row()
        .key(key.to_string())
        .w_full()
        .h(ROW_H)
        .px(8.0)
        .gap(8.0)
        .items_center()
        .child(
            el().w(LABEL_W)
                .child(text(label.to_string()).font(FONT_PIN).color(t.text_dim)),
        )
        .child(el().grow(1.0).min_w(0.0).child(ui_label(value, t)))
}

fn short(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Failures of the last run: each row opens the failing line.
fn failures_block(s: &AppState, i: usize, item: &TbItem, t: &VivadoTheme) -> El {
    let rows: Vec<(String, usize, String, Option<u64>)> = match &item.last {
        Some(Ok(out)) => out
            .failures
            .iter()
            .map(|f| (f.file.clone(), f.line, f.message.clone(), Some(f.time_ns)))
            .collect(),
        Some(Err(e)) => vec![(e.file.clone(), e.line, e.msg.clone(), None)],
        None => Vec::new(),
    };
    let mut block = col().key("sim-failures").w_full().child(section_head(
        "sim-failures-head",
        &format!("Failures ({})", rows.len()),
        t,
    ));
    if rows.is_empty() {
        let note = match (&item.last, item.status) {
            (_, TbStatus::Running) => "Running…",
            (Some(_), _) => "No failures in the last run.",
            (None, _) => "Not run yet in this session.",
        };
        return block.child(
            row()
                .key("sim-failures-none")
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .items_center()
                .child(text(note).font(VivadoTheme::FONT_UI).color(t.text_faint)),
        );
    }
    for (k, (file, line, msg, at)) in rows.into_iter().enumerate() {
        let loc = format!("{}:{line}", short(&file));
        let when = at.map(|ns| format!("@ {ns} ns")).unwrap_or_default();
        block = block.child(
            row()
                .key(format!("sim-failure-{k}"))
                .role("link")
                .label(format!("{loc} {msg}"))
                .help(format!("Open {file} at line {line}"))
                .focusable(true)
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .gap(8.0)
                .items_center()
                .hover(t.selection)
                .on_press(move |s: &mut AppState| {
                    let r = tb::jump_to_failure(s, i, k);
                    report(s, r, "Cannot open the failing line");
                })
                .child(
                    el().w(140.0).child(
                        text(loc.clone())
                            .font(VivadoTheme::FONT_MONO)
                            .color(t.link)
                            .truncate()
                            .help_when_truncated(loc),
                    ),
                )
                .child(
                    el().grow(1.0).min_w(0.0).child(
                        text(msg.clone())
                            .font(VivadoTheme::FONT_UI)
                            .color(t.fail)
                            .truncate()
                            .help_when_truncated(msg),
                    ),
                )
                .child(text(when).font(FONT_PIN).color(t.text_dim)),
        );
    }
    let _ = s;
    block
}

/// Log of the last (or running) run: `$display` output, failures, summary.
fn log_block(s: &AppState, i: usize, item: &TbItem, t: &VivadoTheme) -> El {
    let running = s.sim.active_tb() == Some(i);
    // (time, text, severity); streamed lines carry their time inline.
    let lines: Vec<(Option<u64>, String, MsgSev)> = if running {
        s.sim.log[s.sim.run_log_start.min(s.sim.log.len())..]
            .iter()
            .map(|l| (None, l.text.clone(), l.sev))
            .collect()
    } else {
        match &item.last {
            Some(Ok(out)) => out
                .log
                .iter()
                .map(|l| {
                    let sev = match l.sev {
                        helion_verif::tb::TbSev::Info => MsgSev::Info,
                        helion_verif::tb::TbSev::Warning => MsgSev::Warn,
                        helion_verif::tb::TbSev::Error => MsgSev::Error,
                    };
                    (Some(l.time_ns), l.text.clone(), sev)
                })
                .collect(),
            Some(Err(e)) => vec![(None, e.to_string(), MsgSev::Error)],
            None => Vec::new(),
        }
    };
    let title = match &item.last {
        Some(Ok(out)) if !running => format!(
            "Log ({} lines · {})",
            lines.len(),
            short(&out.log_path.to_string_lossy())
        ),
        _ => format!("Log ({} lines)", lines.len()),
    };
    let mut body = col()
        .key("sim-log")
        .w_full()
        .grow(1.0)
        .min_h(80.0)
        .fill(t.console_bg)
        .scroll(ScrollSpec { stick_bottom: true })
        .py(2.0);
    if lines.is_empty() {
        body = body.child(
            row().w_full().h(ROW_H).px(8.0).items_center().child(
                text("Run the testbench to see its $display output here.")
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_faint),
            ),
        );
    }
    let skip = lines.len().saturating_sub(LOG_SHOWN);
    for (n, (at, line, sev)) in lines.into_iter().enumerate().skip(skip) {
        let ink = match sev {
            MsgSev::Error | MsgSev::Critical => t.fail,
            MsgSev::Warn => t.warn,
            MsgSev::Info => t.text,
        };
        let mut r = row()
            .key(format!("sim-log-{n}"))
            .w_full()
            .h(18.0)
            .px(8.0)
            .gap(10.0)
            .items_center();
        if let Some(ns) = at {
            r = r.child(
                row().w(64.0).justify_end().child(
                    text(format!("{ns} ns"))
                        .font(VivadoTheme::FONT_MONO.mono())
                        .color(t.text_faint),
                ),
            );
        }
        body = body.child(
            r.child(
                text(line)
                    .font(VivadoTheme::FONT_MONO.mono())
                    .color(ink)
                    .grow(1.0)
                    .shrink()
                    .truncate(),
            ),
        );
    }
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(section_head("sim-log-head", &title, t))
        .child(body)
}

/// Selected-testbench detail: header actions, parsed facts, failures, log.
fn tb_detail(s: &AppState, t: &VivadoTheme) -> El {
    let Some(i) = s.sim.selected.filter(|&i| i < s.sim.tbs.len()) else {
        let msg = if s.sim.tbs.is_empty() {
            "Testbench details appear here once one is discovered."
        } else {
            "Select a testbench to see its details."
        };
        return el()
            .key("sim-detail-prompt")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .fill(t.panel_bg)
            .border(1.0, t.border)
            .center()
            .child(text(msg).font(VivadoTheme::FONT_UI).color(t.text_dim));
    };
    let item = &s.sim.tbs[i];
    let busy = s.sim.is_active();
    let vcd = item
        .last
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .and_then(|o| o.vcd_path.clone());
    let head = row()
        .key("sim-detail-head")
        .w_full()
        .h(28.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .fill(t.header_bg)
        .child(
            el().key("sim-detail-name").min_w(0.0).child(
                text(item.name().to_string())
                    .font(VivadoTheme::FONT_TITLE)
                    .color(t.text)
                    .truncate()
                    .help_when_truncated(item.name().to_string()),
            ),
        )
        .child(
            el().key("sim-detail-status").child(
                text(item.status.label())
                    .font(VivadoTheme::FONT_UI)
                    .color(status_ink(item.status, t)),
            ),
        )
        .child(el().grow(1.0))
        .child(
            btn("sim-detail-run", "▶ Run", t, !busy, &busy_reason(s)).on_press(
                move |s: &mut AppState| {
                    let name = s
                        .sim
                        .tbs
                        .get(i)
                        .map(|t| t.name().to_string())
                        .unwrap_or_default();
                    echo(s, format!("launch_simulation -simset sim_1 -tb {name}"));
                    if let Err(e) = tb::launch_testbench(s, i) {
                        report(s, Err(e), "Cannot start simulation");
                    }
                },
            ),
        )
        .child({
            let why = if s.sim.vcd {
                "Run the testbench first to produce a VCD"
            } else {
                "Turn on VCD dump and run the testbench"
            };
            btn(
                "sim-detail-open-wave",
                "Open in waveform viewer",
                t,
                vcd.is_some(),
                why,
            )
            .on_press(move |s: &mut AppState| {
                let Some(path) = vcd.clone() else { return };
                let name = s
                    .sim
                    .tbs
                    .get(i)
                    .map(|t| t.name().to_string())
                    .unwrap_or_default();
                let r = tb::open_vcd(s, &path, &format!("{name} (testbench VCD)"));
                report(s, r, "Cannot open the waveform");
            })
        });
    let mut facts = col()
        .key("sim-detail-facts")
        .w_full()
        .py(2.0)
        .child(fact_row(
            "sim-detail-path",
            "Path",
            &item.file.path.display().to_string(),
            t,
        ));
    match &item.info {
        Ok(info) => {
            facts = facts
                .child(fact_row(
                    "sim-detail-dut",
                    "DUT",
                    &format!("{} {}", info.dut_module, info.dut_instance),
                    t,
                ))
                .child(fact_row(
                    "sim-detail-ports",
                    "Ports",
                    &info
                        .ports
                        .iter()
                        .map(|(p, sig)| {
                            if p == sig {
                                p.clone()
                            } else {
                                format!(".{p}({sig})")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                    t,
                ))
                .child(fact_row(
                    "sim-detail-clocks",
                    "Clocks",
                    &if info.clocks.is_empty() {
                        "-".to_string()
                    } else {
                        info.clocks
                            .iter()
                            .map(|(c, half)| format!("{c} ({} ns period)", half * 2))
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                    t,
                ))
                .child(fact_row(
                    "sim-detail-decls",
                    "Signals",
                    &info
                        .decls
                        .iter()
                        .map(|(n, w)| {
                            if *w > 1 {
                                format!("{n}[{}:0]", w - 1)
                            } else {
                                n.clone()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                    t,
                ));
        }
        Err(e) => {
            facts = facts.child(
                row()
                    .key("sim-detail-parse-error")
                    .w_full()
                    .h(ROW_H)
                    .px(8.0)
                    .gap(8.0)
                    .items_center()
                    .child(
                        el().w(LABEL_W)
                            .child(text("Parse").font(FONT_PIN).color(t.text_dim)),
                    )
                    .child(
                        el().grow(1.0).min_w(0.0).child(
                            text(e.clone())
                                .font(VivadoTheme::FONT_UI)
                                .color(t.fail)
                                .truncate()
                                .help_when_truncated(e.clone()),
                        ),
                    ),
            );
        }
    }
    if let Some(Ok(out)) = &item.last {
        facts = facts.child(fact_row("sim-detail-summary", "Last run", &out.summary, t));
    }
    col()
        .key("sim-detail")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(head)
        .child(facts)
        .child(failures_block(s, i, item, t))
        .child(log_block(s, i, item, t))
}

/// Right-aligned monospace cell.
fn num_cell(value: String, w: f32, t: &VivadoTheme) -> El {
    row()
        .w(w)
        .justify_end()
        .child(text(value).font(VivadoTheme::FONT_MONO).color(t.text))
}

fn head_cell(label: &str, w: Option<f32>, right: bool, t: &VivadoTheme) -> El {
    let cell = row().child(
        text(label.to_string())
            .font(VivadoTheme::FONT_CAPS)
            .color(t.text_faint),
    );
    let cell = match w {
        Some(w) => cell.w(w),
        None => cell.grow(1.0).min_w(0.0),
    };
    if right { cell.justify_end() } else { cell }
}

fn fmt_ns(ns: u64) -> String {
    if ns >= 1_000_000 {
        format!("{:.2} ms", ns as f64 / 1e6)
    } else if ns >= 10_000 {
        format!("{:.1} us", ns as f64 / 1e3)
    } else {
        format!("{ns} ns")
    }
}

fn fmt_ms(ms: u64) -> String {
    if ms >= 10_000 {
        format!("{:.1} s", ms as f64 / 1e3)
    } else {
        format!("{ms} ms")
    }
}

/// Persisted run history: Testbench | Result | Sim time | Wall | Errors | VCD.
fn history_panel(s: &AppState, t: &VivadoTheme) -> El {
    let rows_newest = s.sim.history_newest_first();
    let mut table = col().w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec {
        stick_bottom: false,
    });
    if rows_newest.is_empty() {
        table = table.child(
            row()
                .key("sim-history-empty")
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .items_center()
                .child(
                    text("No runs recorded for this directory yet.")
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text_faint),
                ),
        );
    }
    for (k, h) in rows_newest.into_iter().enumerate() {
        let (result, ink) = if h.passed {
            ("Passed", t.pass)
        } else {
            ("Failed", t.fail)
        };
        let vcd = h.vcd_path.clone().filter(|p| p.is_file());
        let name = h.tb_name.clone();
        let vcd_cell = match vcd {
            Some(p) => row().w(56.0).justify_end().child(
                el().key(format!("sim-history-vcd-{k}"))
                    .role("link")
                    .label(format!("Open {} VCD", h.tb_name))
                    .help(p.display().to_string())
                    .focusable(true)
                    .px(4.0)
                    .hover(t.selection)
                    .on_press(move |s: &mut AppState| {
                        let r = tb::open_vcd(s, &p, &format!("{name} (history VCD)"));
                        report(s, r, "Cannot open the waveform");
                    })
                    .child(text("Open").font(VivadoTheme::FONT_UI).color(t.link)),
            ),
            None => row()
                .w(56.0)
                .justify_end()
                .child(text("—").font(VivadoTheme::FONT_UI).color(t.text_faint)),
        };
        table = table.child(
            row()
                .key(format!("sim-history-{k}"))
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .gap(10.0)
                .items_center()
                .hover(t.selection)
                .child(el().grow(1.0).min_w(0.0).child(ui_label(&h.tb_name, t)))
                .child(
                    el().w(56.0)
                        .child(text(result).font(VivadoTheme::FONT_UI).color(ink)),
                )
                .child(num_cell(fmt_ns(h.sim_time_ns), 76.0, t))
                .child(num_cell(fmt_ms(h.runtime_ms), 64.0, t))
                .child(num_cell(h.errors.to_string(), 48.0, t))
                .child(vcd_cell),
        );
    }
    col()
        .key("sim-history")
        .w_full()
        .h(HISTORY_H)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(section_head(
            "sim-history-head",
            &format!("Run history ({})", s.sim.history.len()),
            t,
        ))
        .child(
            row()
                .key("sim-history-cols")
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .gap(10.0)
                .items_center()
                .child(head_cell("Testbench", None, false, t))
                .child(head_cell("Result", Some(56.0), false, t))
                .child(head_cell("Sim time", Some(76.0), true, t))
                .child(head_cell("Wall", Some(64.0), true, t))
                .child(head_cell("Errors", Some(48.0), true, t))
                .child(head_cell("VCD", Some(56.0), true, t)),
        )
        .child(table)
}

/// Sim center surface.
pub fn sim_pane(s: &AppState, t: &VivadoTheme) -> El {
    col()
        .key("sim")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Simulation - sim_1", "sim-pane", t))
        .child(sim_toolbar(s, t))
        .child(
            row()
                .key("sim-body")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .gap(6.0)
                .padding(6.0)
                .child(
                    col()
                        .key("sim-left")
                        .w(LEFT_W)
                        .h_full()
                        .min_h(0.0)
                        .gap(6.0)
                        .child(tb_list(s, t))
                        .child(crate::views::sim_settings::settings_section(s, t)),
                )
                .child(
                    col()
                        .key("sim-right")
                        .grow(1.0)
                        .h_full()
                        .min_w(0.0)
                        .min_h(0.0)
                        .gap(6.0)
                        .child(tb_detail(s, t))
                        .child(history_panel(s, t)),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CenterPane;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn harness_with(state: AppState) -> Runtime<AppState> {
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(state, crate::view, app_spec().size, env)
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn empty_directory_shows_one_sentence_and_the_design_run() {
        let mut s = init();
        s.center = CenterPane::Sim;
        tb::rescan(&mut s);
        let mut rt = harness_with(s);
        rt.frame(0.0);
        for key in [
            "sim-toolbar",
            "sim-list-empty",
            "sim-run-design",
            "sim-detail-prompt",
            "sim-history-empty",
            "sim-settings",
        ] {
            assert!(rt.find(key).is_some(), "missing {key}");
        }
        assert!(rt.find("sim-tb-0").is_none(), "no fake testbenches");
    }

    #[test]
    fn run_results_show_failures_log_history_and_jump() {
        let mut s = init();
        s.center = CenterPane::Sim;
        tb::seed_run_fixture(&mut s).expect("fixture runs");
        s.sim.selected = Some(0);
        let mut rt = harness_with(s);
        rt.frame(0.0);
        for key in [
            "sim-detail",
            "sim-detail-name",
            "sim-detail-dut",
            "sim-detail-clocks",
            "sim-failure-0",
            "sim-log",
            "sim-history-0",
            "sim-history-1",
            "sim-history-vcd-0",
            "sim-detail-open-wave",
        ] {
            assert!(rt.find(key).is_some(), "missing {key}");
        }
        assert_eq!(rt.state.sim.status(0), TbStatus::Failed);
        tap(&mut rt, "sim-failure-0");
        assert_eq!(
            rt.state.center,
            CenterPane::Editor,
            "failure opens the line"
        );
    }

    #[test]
    fn open_in_waveform_viewer_loads_the_vcd() {
        let mut s = init();
        s.center = CenterPane::Sim;
        tb::seed_run_fixture(&mut s).expect("fixture runs");
        s.sim.selected = Some(1);
        s.wave.live = None;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "sim-detail-open-wave");
        assert_eq!(rt.state.center, CenterPane::Wave);
        assert!(rt.state.wave.has_live(), "VCD imports into the viewer");
        assert!(rt.find("wave-canvas").is_some());
    }

    #[test]
    fn vcd_toggle_and_settings_collapse_are_real_state() {
        let mut s = init();
        s.center = CenterPane::Sim;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.state.sim.vcd);
        tap(&mut rt, "sim-vcd");
        assert!(!rt.state.sim.vcd);
        assert!(rt.find("sim-setting-runtime-cycles").is_some());
        tap(&mut rt, "sim-settings-head");
        assert!(!rt.state.sim_settings.open);
        assert!(rt.find("sim-setting-runtime-cycles").is_none());
    }

    #[test]
    fn setting_rows_open_an_editor_and_toggle_log_all_signals() {
        let mut s = init();
        s.center = CenterPane::Sim;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "sim-setting-runtime-cycles");
        assert!(
            rt.find("sim-setting-input-runtime-cycles").is_some(),
            "editable rows open an inline editor"
        );
        assert!(rt.state.sim_settings.log_all_signals);
        tap(&mut rt, "sim-setting-log-all-signals");
        assert!(
            !rt.state.sim_settings.log_all_signals,
            "the flag row toggles"
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "set_property LOG_ALL_SIGNALS 0"),
            "the toggle echoes its verb"
        );
    }
}
