//! Coverage view: code coverage of the last testbench run.
//!
//! Engine: `helion_verif::tb::TbCoverage`, collected by `run_testbench`:
//! testbench statement (line) and `if`/`assert` branch coverage from the
//! interpreter, plus the DUT's netlist-level toggle, branch (mux select /
//! flip-flop enable) and FSM coverage from the `helion-sim` kernel. The
//! editor gutter colors executed / missed testbench lines when the overlay
//! is on ([`line_hits`]). DUT *line* coverage is not offered: the DUT is
//! simulated as a synthesized netlist without source-line attribution.

use rune::{El, col, el, row};

use crate::state::{AppState, CenterPane};
use crate::theme::VivadoTheme;
use crate::views::cad;
use crate::views::report_kit::{self as kit, header_row, num_cell, plain_row, select_row, table};
use crate::views::sources::pane_header;
use helion_verif::tb::TbCoverage;

const MAX_ROWS: usize = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CovTab {
    #[default]
    Lines,
    Branches,
    Toggles,
    Fsm,
}

#[derive(Clone, Debug, Default)]
pub struct CoverageView {
    pub tab: CovTab,
    /// Color executed / missed lines in the editor gutter.
    pub overlay: bool,
}

/// `(testbench name, coverage)` of the selected testbench's last run, else
/// the first testbench that has one.
pub fn latest(s: &AppState) -> Option<(&str, &TbCoverage)> {
    let pick = |i: usize| {
        let t = s.sim.tbs.get(i)?;
        let out = t.last.as_ref()?.as_ref().ok()?;
        Some((t.file.name.as_str(), &out.coverage))
    };
    s.sim.selected.and_then(pick).or_else(|| (0..s.sim.tbs.len()).find_map(pick))
}

/// Execution count of 1-based `line` in the active editor file when the
/// overlay is on and that file is the covered testbench (`None` otherwise
/// or for non-executable lines).
pub fn line_hits(s: &AppState, line: usize) -> Option<u64> {
    if !s.coverage.overlay {
        return None;
    }
    let (_, c) = latest(s)?;
    let tab = s.editor.tabs.get(s.editor.active)?;
    let path = s.editor.paths.get(tab)?;
    if path != &c.tb_file {
        return None;
    }
    c.lines.iter().find(|(l, _)| *l == line).map(|(_, n)| *n)
}

fn pct_color(p: f64, t: &VivadoTheme) -> rune::Color {
    if p >= 90.0 {
        t.pass
    } else if p >= 60.0 {
        t.warn
    } else {
        t.fail
    }
}

fn open_tb(s: &mut AppState) {
    let Some((_, c)) = latest(s) else { return };
    let first_missed = c.lines.iter().find(|(_, n)| *n == 0).or(c.lines.first()).map(|(l, _)| *l).unwrap_or(1);
    let path = c.tb_file.clone();
    s.coverage.overlay = true;
    if let Err(e) = crate::views::tb::open_in_editor(s, &path, first_missed) {
        s.console.lines.push(crate::state::ConsoleLine { text: format!("ERROR: {e}"), sev: crate::state::MsgSev::Error });
    }
}

fn kpis(c: &TbCoverage, t: &VivadoTheme) -> El {
    let d = &c.dut;
    row()
        .w_full()
        .gap(8.0)
        .child(cad::kpi("cov-kpi-lines", "TB statements", format!("{:.1}%", c.line_pct()), pct_color(c.line_pct(), t), format!("{} / {} lines", c.lines_hit(), c.lines.len()), t))
        .child(cad::kpi("cov-kpi-branches", "TB branches", format!("{:.1}%", c.branch_pct()), pct_color(c.branch_pct(), t), format!("{} / {} arms", c.branch_arms_hit(), 2 * c.branches.len()), t))
        .child(cad::kpi("cov-kpi-toggle", "DUT toggle", format!("{:.1}%", d.toggle_bin_coverage_pct()), pct_color(d.toggle_bin_coverage_pct(), t), format!("{} / {} nets fully toggled", d.fully_toggled_nets(), d.total_nets()), t))
        .child(if d.branches.is_empty() {
            cad::kpi("cov-kpi-dutbranch", "DUT mux / enable", "n/a", t.text_faint, "no mux selects or enables", t)
        } else {
            cad::kpi("cov-kpi-dutbranch", "DUT mux / enable", format!("{:.1}%", d.branch_coverage_pct()), pct_color(d.branch_coverage_pct(), t), format!("{} conditions", d.branches.len()), t)
        })
        .child(if d.fsms.is_empty() {
            cad::kpi("cov-kpi-fsm", "DUT FSM states", "n/a", t.text_faint, "no state registers", t)
        } else {
            cad::kpi("cov-kpi-fsm", "DUT FSM states", format!("{:.1}%", d.fsm_state_coverage_pct()), pct_color(d.fsm_state_coverage_pct(), t), format!("{} state register(s)", d.fsms.len()), t)
        })
}

fn files(c: &TbCoverage, t: &VivadoTheme) -> El {
    let head = header_row(t)
        .child(kit::header_cell("File", t).grow(1.0).min_w(0.0))
        .child(el().w(150.0).child(kit::header_cell("Coverage kind", t)))
        .child(kit::header_num("Lines", 80.0, t))
        .child(kit::header_num("%", 64.0, t))
        .child(el().w(150.0));
    let tb = plain_row("cov-file-tb")
        .child(kit::text_cell(c.tb_file.display().to_string(), 1.0, t.text))
        .child(kit::fixed_cell("Statement + branch", 150.0, t.text_dim))
        .child(num_cell(format!("{}/{}", c.lines_hit(), c.lines.len()), 80.0, t.text))
        .child(num_cell(format!("{:.1}", c.line_pct()), 64.0, pct_color(c.line_pct(), t)))
        .child(el().w(150.0).child(kit::link("cov-open-tb", "Open with coverage", t, open_tb)));
    let dut = plain_row("cov-file-dut")
        .child(kit::text_cell(c.dut_file.display().to_string(), 1.0, t.text))
        .child(kit::fixed_cell("Netlist toggle / FSM", 150.0, t.text_dim))
        .child(num_cell("n/a", 80.0, t.text_faint).help("The DUT simulates as a synthesized netlist; cells carry no source lines."))
        .child(num_cell(format!("{:.1}", c.dut.toggle_bin_coverage_pct()), 64.0, pct_color(c.dut.toggle_bin_coverage_pct(), t)))
        .child(el().w(150.0));
    table(head, col().w_full().child(tb).child(dut), t).key("cov-files")
}

fn tab_body(s: &AppState, c: &TbCoverage, t: &VivadoTheme) -> El {
    match s.coverage.tab {
        CovTab::Lines => {
            let src: Vec<String> = std::fs::read_to_string(&c.tb_file).map(|x| x.lines().map(str::to_string).collect()).unwrap_or_default();
            let head = header_row(t)
                .child(kit::header_num("Line", 56.0, t))
                .child(kit::header_num("Hits", 72.0, t))
                .child(kit::header_cell("Source", t).grow(1.0).min_w(0.0));
            let mut body = col().w_full();
            for (i, (l, n)) in c.lines.iter().take(MAX_ROWS).enumerate() {
                let line = *l;
                let path = c.tb_file.clone();
                let text = src.get(line.saturating_sub(1)).map(|x| x.trim().to_string()).unwrap_or_default();
                body = body.child(
                    select_row(format!("cov-line-{i}"), format!("Line {line}"), false, t, move |s: &mut AppState| {
                        s.coverage.overlay = true;
                        let _ = crate::views::tb::open_in_editor(s, &path, line);
                    })
                    .child(num_cell(line.to_string(), 56.0, t.text_dim))
                    .child(num_cell(n.to_string(), 72.0, if *n == 0 { t.fail } else { t.pass }))
                    .child(kit::text_cell(text, 1.0, t.text)),
                );
            }
            table(head, body, t)
        }
        CovTab::Branches => {
            let head = header_row(t)
                .child(kit::header_cell("Branch", t).grow(1.0).min_w(0.0))
                .child(kit::header_num("True", 72.0, t))
                .child(kit::header_num("False", 72.0, t))
                .child(el().w(72.0).child(kit::header_cell("Status", t)));
            let mut body = col().w_full();
            let status = |a: u64, b: u64, t: &VivadoTheme| match (a > 0, b > 0) {
                (true, true) => ("Both", t.pass),
                (false, false) => ("None", t.fail),
                _ => ("Partial", t.warn),
            };
            for (i, b) in c.branches.iter().enumerate() {
                let (st, col_) = status(b.true_hits, b.false_hits, t);
                body = body.child(
                    plain_row(format!("cov-branch-{i}"))
                        .child(kit::text_cell(format!("{} at line {} (testbench)", b.kind, b.line), 1.0, t.text))
                        .child(num_cell(b.true_hits.to_string(), 72.0, t.text))
                        .child(num_cell(b.false_hits.to_string(), 72.0, t.text))
                        .child(kit::fixed_cell(st, 72.0, col_)),
                );
            }
            for (i, b) in c.dut.branches.iter().take(MAX_ROWS).enumerate() {
                let (st, col_) = status(b.true_hits, b.false_hits, t);
                body = body.child(
                    plain_row(format!("cov-dutbranch-{i}"))
                        .child(kit::text_cell(format!("{} ({}) in DUT", b.name, b.cond_net), 1.0, t.text))
                        .child(num_cell(b.true_hits.to_string(), 72.0, t.text))
                        .child(num_cell(b.false_hits.to_string(), 72.0, t.text))
                        .child(kit::fixed_cell(st, 72.0, col_)),
                );
            }
            table(head, body, t)
        }
        CovTab::Toggles => {
            let mut nets: Vec<_> = c.dut.net_toggles.iter().collect();
            nets.sort_by_key(|n| (n.bins_hit(), n.net.clone()));
            let head = header_row(t)
                .child(kit::header_cell("Net", t).grow(1.0).min_w(0.0))
                .child(kit::header_num("0→1", 72.0, t))
                .child(kit::header_num("1→0", 72.0, t))
                .child(el().w(72.0).child(kit::header_cell("Status", t)));
            let mut body = col().w_full();
            for (i, n) in nets.iter().take(MAX_ROWS).enumerate() {
                let (st, col_) = if n.is_fully_toggled() {
                    ("Full", t.pass)
                } else if n.is_partially_toggled() {
                    ("Partial", t.warn)
                } else {
                    ("None", t.fail)
                };
                body = body.child(
                    plain_row(format!("cov-toggle-{i}"))
                        .child(kit::text_cell(n.net.clone(), 1.0, t.text))
                        .child(num_cell(n.transitions_0_to_1.to_string(), 72.0, t.text))
                        .child(num_cell(n.transitions_1_to_0.to_string(), 72.0, t.text))
                        .child(kit::fixed_cell(st, 72.0, col_)),
                );
            }
            table(head, body, t)
        }
        CovTab::Fsm => {
            let head = header_row(t)
                .child(kit::header_cell("State register", t).grow(1.0).min_w(0.0))
                .child(kit::header_num("States", 96.0, t))
                .child(kit::header_num("Transitions", 96.0, t))
                .child(kit::header_num("%", 64.0, t));
            let mut body = col().w_full();
            if c.dut.fsms.is_empty() {
                body = body.child(kit::empty_state("cov-fsm-empty", "No multi-bit state registers were found in the DUT.", None, t));
            }
            for (i, f) in c.dut.fsms.iter().enumerate() {
                body = body.child(
                    plain_row(format!("cov-fsm-{i}"))
                        .child(kit::text_cell(f.name.clone(), 1.0, t.text))
                        .child(num_cell(format!("{}/{}", f.visited_states.len(), f.expected_states), 96.0, t.text))
                        .child(num_cell(f.transitions.len().to_string(), 96.0, t.text))
                        .child(num_cell(format!("{:.1}", f.state_coverage_pct()), 64.0, pct_color(f.state_coverage_pct(), t))),
                );
            }
            table(head, body, t)
        }
    }
}

pub fn coverage_pane(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col().w_full().gap(10.0).padding(8.0);
    match latest(s) {
        None => {
            body = body.child(kit::empty_state(
                "cov-empty",
                "No coverage yet — run a testbench (Simulation) to collect it.",
                Some(kit::link("cov-open-sim", "Open Simulation", t, |s: &mut AppState| s.navigate(CenterPane::Sim))),
                t,
            ));
        }
        Some((name, c)) => {
            let mut tabs = row().key("cov-tabs").w_full().gap(1.0).fill(t.border);
            for (k, label, tab) in [
                ("cov-tab-lines", "Statements", CovTab::Lines),
                ("cov-tab-branches", "Branches", CovTab::Branches),
                ("cov-tab-toggles", "Toggles", CovTab::Toggles),
                ("cov-tab-fsm", "FSM", CovTab::Fsm),
            ] {
                tabs = tabs.child(cad::tab(k, label, s.coverage.tab == tab, t, move |s: &mut AppState| s.coverage.tab = tab));
            }
            tabs = tabs.child(el().grow(1.0).h(24.0).fill(t.header_bg));
            let overlay = s.coverage.overlay;
            body = body
                .child(
                    row().w_full().h(28.0).items_center().gap(8.0)
                        .child(cad::cut(format!("{name} · {}", c.summary_line()), VivadoTheme::FONT_UI, t.text_dim).grow(1.0).min_w(0.0).key("cov-summary"))
                        .child(kit::action_button("cov-overlay", if overlay { "Hide Editor Highlights" } else { "Show in Editor" }, None, t, move |s: &mut AppState| {
                            s.coverage.overlay = !overlay;
                        })),
                )
                .child(kpis(c, t))
                .child(files(c, t))
                .child(tabs)
                .child(tab_body(s, c, t));
        }
    }
    col()
        .key("cov-pane")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Code Coverage", "cov", t))
        .child(col().key("cov-scroll").grow(1.0).min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false }).child(body))
}

pub const VERBS: &[&str] = &["report_coverage"];

pub fn help(name: &str) -> Option<(&'static str, &'static str)> {
    (name == "report_coverage").then_some(("report_coverage", "code coverage of the last testbench run (Coverage view)"))
}

pub fn tcl_verb(s: &mut AppState, _name: &str, _args: &[String]) -> Result<String, String> {
    s.navigate(CenterPane::Coverage);
    latest(s)
        .map(|(n, c)| format!("{n}: {}", c.summary_line()))
        .ok_or_else(|| "report_coverage: no testbench run yet (tb_run first)".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_follows_the_last_run_and_drives_the_gutter_overlay() {
        let mut s = crate::init();
        let ex = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let dir = std::env::temp_dir().join(format!("helion-rune-cov-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["counter.sv", "counter_tb.sv"] {
            std::fs::copy(ex.join(f), dir.join(f)).unwrap();
        }
        s.sim.dir_override = Some(dir.clone());
        crate::views::tb::exec_tb(&mut s, "tb_rescan", &[]).unwrap();
        assert!(tcl_verb(&mut s, "report_coverage", &[]).is_err());
        let i = s.sim.tbs.iter().position(|t| t.file.name == "counter_tb").expect("counter_tb");
        let path = s.sim.tbs[i].file.path.clone();
        let outcome = helion_verif::tb::run_testbench(&helion_verif::tb::TbRunConfig::new(path.clone()), None).unwrap();
        s.sim.tbs[i].last = Some(Ok(outcome));
        s.sim.selected = Some(i);
        let v = tcl_verb(&mut s, "report_coverage", &[]).unwrap();
        assert!(v.contains("tb_lines="), "{v}");
        assert_eq!(s.center, CenterPane::Coverage);
        open_tb(&mut s);
        assert!(s.coverage.overlay);
        let (_, c) = latest(&s).unwrap();
        let (line, hits) = c.lines[0];
        assert_eq!(line_hits(&s, line), Some(hits));
    }
}
