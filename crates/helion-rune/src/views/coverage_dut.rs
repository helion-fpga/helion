//! DUT RTL line / branch coverage in the Coverage view and the editor gutter.
//!
//! Engine: `helion_verif::tb::TbCoverage::{dut_lines, dut_branches,
//! dut_not_instrumented, dut_line_note}`, filled by `run_testbench` from the
//! line probes `helion_sv::synth_sv_path_line_cov` builds into the simulated
//! netlist (`helion_sim::line_coverage` defines what a hit means). This
//! module only presents that data.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rune::{El, col, el};

use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::cad;
use crate::views::report_kit::{self as kit, header_row, num_cell, plain_row, select_row, table};
use helion_sim::RtlLineKind;
use helion_verif::tb::TbCoverage;

const MAX_ROWS: usize = 200;

/// How a gutter line number is colored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkState {
    Hit,
    /// Executed, but a branch on this line has an arm that was never taken.
    Partial,
    Missed,
}

/// Coverage mark of one editor gutter line plus its tooltip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GutterMark {
    pub state: MarkState,
    pub help: String,
}

/// The editor and the engine may spell one file differently (relative /
/// absolute / symlinked temp dir); compare on disk when the names match.
fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || (a.file_name() == b.file_name()
            && matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y))
}

fn times(n: u64) -> String {
    format!("{n}×")
}

/// DUT gutter mark for 1-based `line` of `path` (`None` for lines that carry
/// no instrumented statement, assign or branch).
pub fn dut_mark(c: &TbCoverage, path: &Path, line: usize) -> Option<GutterMark> {
    let hits = c
        .dut_lines
        .iter()
        .find(|l| l.line as usize == line && same_file(Path::new(&l.file), path))
        .map(|l| (l.kind, l.hits));
    let sites = c.dut_branches_at(path, line);
    let mut parts: Vec<String> = Vec::new();
    match hits {
        Some((RtlLineKind::Assign, 0)) => parts.push("Value never changed in the last testbench run".into()),
        Some((RtlLineKind::Assign, n)) => parts.push(format!("Value changed {} in the last testbench run", times(n))),
        Some((_, 0)) => parts.push("Not executed in the last testbench run".into()),
        Some((_, n)) => parts.push(format!("Executed {} in the last testbench run", times(n))),
        None => {}
    }
    let mut partial = false;
    for b in &sites {
        let arms: Vec<String> = b.arms.iter().map(|a| format!("{} {}", a.label, times(a.hits))).collect();
        partial |= b.arms.iter().any(|a| a.hits == 0);
        parts.push(format!("{}: {}", b.construct, arms.join(" · ")));
    }
    // A case item on its own line: report that arm.
    for b in c.dut_branches.iter().filter(|b| b.line as usize != line) {
        for a in b.arms.iter().filter(|a| a.line as usize == line) {
            if same_file(Path::new(&b.file), path) {
                parts.push(format!("{} arm {}: taken {}", b.construct, a.label, times(a.hits)));
                partial |= a.hits == 0;
            }
        }
    }
    if parts.is_empty() {
        return None;
    }
    let state = match hits {
        Some((_, 0)) => MarkState::Missed,
        _ if partial => MarkState::Partial,
        _ => MarkState::Hit,
    };
    Some(GutterMark { state, help: parts.join("\n") })
}

/// Distinct DUT files with line coverage, in report order.
fn dut_files(c: &TbCoverage) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for f in c.dut_lines.iter().map(|l| &l.file).chain(c.dut_branches.iter().map(|b| &b.file)) {
        if !v.contains(f) {
            v.push(f.clone());
        }
    }
    v
}

fn file_name(f: &str) -> String {
    Path::new(f).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| f.to_string())
}

fn source_lines(files: &[String]) -> HashMap<String, Vec<String>> {
    files
        .iter()
        .map(|f| {
            let lines = std::fs::read_to_string(f).map(|x| x.lines().map(str::to_string).collect()).unwrap_or_default();
            (f.clone(), lines)
        })
        .collect()
}

/// Open DUT `file` at `line` with the gutter overlay on.
fn open_at(s: &mut AppState, file: &Path, line: usize) {
    s.coverage.overlay = true;
    if let Err(e) = crate::views::tb::open_in_editor(s, file, line) {
        s.console.lines.push(crate::state::ConsoleLine { text: format!("ERROR: {e}"), sev: crate::state::MsgSev::Error });
    }
}

/// Open the DUT at its first never-executed line (else its first line).
pub fn open_dut(s: &mut AppState) {
    let Some((_, c)) = crate::views::coverage::latest(s) else { return };
    let (file, line) = c
        .dut_lines
        .iter()
        .find(|l| l.hits == 0)
        .or(c.dut_lines.first())
        .map(|l| (PathBuf::from(&l.file), l.line as usize))
        .unwrap_or_else(|| (c.dut_file.clone(), 1));
    open_at(s, &file, line);
}

/// Why DUT line coverage is missing, or `None` when it was collected.
pub fn unavailable(c: &TbCoverage) -> Option<String> {
    if !c.dut_lines.is_empty() || !c.dut_branches.is_empty() {
        return None;
    }
    Some(
        c.dut_line_note
            .clone()
            .unwrap_or_else(|| "The DUT has no instrumented RTL statements.".into()),
    )
}

/// KPI cards: DUT line and branch-arm coverage.
pub fn kpis(c: &TbCoverage, t: &VivadoTheme, pct_color: fn(f64, &VivadoTheme) -> rune::Color) -> [El; 2] {
    if let Some(why) = unavailable(c) {
        return [
            cad::kpi("cov-kpi-dutlines", "DUT lines", "n/a", t.text_faint, why.clone(), t).help(why.clone()),
            cad::kpi("cov-kpi-dutarms", "DUT branches", "n/a", t.text_faint, "no RTL branch data", t).help(why),
        ];
    }
    let arms = if c.dut_arms_total() == 0 {
        cad::kpi("cov-kpi-dutarms", "DUT branches", "n/a", t.text_faint, "no if / case in the DUT", t)
    } else {
        cad::kpi(
            "cov-kpi-dutarms",
            "DUT branches",
            format!("{:.1}%", c.dut_branch_pct()),
            pct_color(c.dut_branch_pct(), t),
            format!("{} / {} arms", c.dut_arms_hit(), c.dut_arms_total()),
            t,
        )
    };
    [
        cad::kpi(
            "cov-kpi-dutlines",
            "DUT lines",
            format!("{:.1}%", c.dut_line_pct()),
            pct_color(c.dut_line_pct(), t),
            format!("{} / {} lines", c.dut_lines_hit(), c.dut_lines.len()),
            t,
        ),
        arms,
    ]
}

/// The DUT row of the Coverage view's file table.
pub fn file_row(c: &TbCoverage, t: &VivadoTheme, pct_color: fn(f64, &VivadoTheme) -> rune::Color) -> El {
    let r = plain_row("cov-file-dut").child(kit::text_cell(c.dut_file.display().to_string(), 1.0, t.text));
    match unavailable(c) {
        Some(why) => r
            .child(kit::fixed_cell("Netlist toggle / FSM", 150.0, t.text_dim))
            .child(num_cell("n/a", 80.0, t.text_faint).help(why.clone()))
            .child(num_cell(format!("{:.1}", c.dut.toggle_bin_coverage_pct()), 64.0, pct_color(c.dut.toggle_bin_coverage_pct(), t)))
            .child(
                el().w(150.0).child(
                    kit::action_button("cov-open-dut", "Open with coverage", Some(&why), t, |_| {}),
                ),
            ),
        None => r
            .child(kit::fixed_cell("RTL line + branch", 150.0, t.text_dim))
            .child(num_cell(format!("{}/{}", c.dut_lines_hit(), c.dut_lines.len()), 80.0, t.text))
            .child(num_cell(format!("{:.1}", c.dut_line_pct()), 64.0, pct_color(c.dut_line_pct(), t)))
            .child(el().w(150.0).child(kit::link("cov-open-dut", "Open with coverage", t, open_dut))),
    }
}

/// "DUT Lines" tab: every instrumented DUT line, then the lines synthesis
/// could not instrument.
pub fn lines_tab(c: &TbCoverage, t: &VivadoTheme) -> El {
    if let Some(why) = unavailable(c) {
        return kit::empty_state("cov-dut-empty", &format!("No DUT line coverage: {why}"), None, t);
    }
    let files = dut_files(c);
    let src = source_lines(&files);
    let head = header_row(t)
        .child(el().w(140.0).child(kit::header_cell("File", t)))
        .child(kit::header_num("Line", 56.0, t))
        .child(el().w(64.0).child(kit::header_cell("Kind", t)))
        .child(kit::header_num("Hits", 72.0, t))
        .child(kit::header_cell("Source", t).grow(1.0).min_w(0.0));
    let mut body = col().w_full();
    for (i, l) in c.dut_lines.iter().take(MAX_ROWS).enumerate() {
        let line = l.line as usize;
        let path = PathBuf::from(&l.file);
        let text = src.get(&l.file).and_then(|v| v.get(line.saturating_sub(1))).map(|x| x.trim().to_string()).unwrap_or_default();
        let kind = match l.kind {
            RtlLineKind::Statement => "stmt",
            RtlLineKind::Assign => "assign",
        };
        body = body.child(
            select_row(format!("cov-dutline-{i}"), format!("DUT line {line}"), false, t, move |s: &mut AppState| {
                open_at(s, &path, line);
            })
            .child(kit::fixed_cell(file_name(&l.file), 140.0, t.text_dim).help(l.file.clone()))
            .child(num_cell(line.to_string(), 56.0, t.text_dim))
            .child(kit::fixed_cell(kind, 64.0, t.text_dim))
            .child(num_cell(l.hits.to_string(), 72.0, if l.hits == 0 { t.fail } else { t.pass }))
            .child(kit::text_cell(text, 1.0, t.text)),
        );
    }
    let mut out = col().w_full().gap(8.0).child(table(head, body, t).key("cov-dut-lines"));
    if !c.dut_not_instrumented.is_empty() {
        let head = header_row(t)
            .child(el().w(140.0).child(kit::header_cell("File", t)))
            .child(kit::header_num("Line", 56.0, t))
            .child(kit::header_cell("Not instrumented because", t).grow(1.0).min_w(0.0));
        let mut gaps = col().w_full();
        for (i, g) in c.dut_not_instrumented.iter().take(MAX_ROWS).enumerate() {
            gaps = gaps.child(
                plain_row(format!("cov-dutgap-{i}"))
                    .child(kit::fixed_cell(file_name(&g.file), 140.0, t.text_dim).help(g.file.clone()))
                    .child(num_cell(g.line.to_string(), 56.0, t.text_dim))
                    .child(kit::text_cell(g.reason.clone(), 1.0, t.text_dim)),
            );
        }
        out = out.child(table(head, gaps, t).key("cov-dut-gaps"));
    }
    out
}

/// "DUT Branches" tab: one row per `if` / `case` arm, grouped by branch.
pub fn branches_tab(c: &TbCoverage, t: &VivadoTheme) -> El {
    if let Some(why) = unavailable(c) {
        return kit::empty_state("cov-dut-empty", &format!("No DUT branch coverage: {why}"), None, t);
    }
    if c.dut_branches.is_empty() {
        return kit::empty_state("cov-dutbr-empty", "The DUT has no if / case branches.", None, t);
    }
    let head = header_row(t)
        .child(kit::header_cell("Branch", t).grow(1.0).min_w(0.0))
        .child(el().w(160.0).child(kit::header_cell("Arm", t)))
        .child(kit::header_num("Line", 56.0, t))
        .child(kit::header_num("Taken", 72.0, t))
        .child(el().w(72.0).child(kit::header_cell("Status", t)));
    let mut body = col().w_full();
    let mut n = 0usize;
    'outer: for (bi, b) in c.dut_branches.iter().enumerate() {
        let all = b.arms.iter().all(|a| a.hits > 0);
        for (ai, a) in b.arms.iter().enumerate() {
            if n >= MAX_ROWS {
                break 'outer;
            }
            n += 1;
            let line = a.line as usize;
            let path = PathBuf::from(&b.file);
            let branch = if ai == 0 {
                let mark = if all { "" } else { " · partial" };
                format!("{} at {}:{}{mark}", b.construct, file_name(&b.file), b.line)
            } else {
                String::new()
            };
            let (st, ink) = if a.hits > 0 { ("Taken", t.pass) } else { ("Missed", t.fail) };
            body = body.child(
                select_row(format!("cov-dutarm-{bi}-{ai}"), format!("{} arm {} at line {line}", b.construct, a.label), false, t, move |s: &mut AppState| {
                    open_at(s, &path, line);
                })
                .child(kit::text_cell(branch, 1.0, if all { t.text } else { t.warn }))
                .child(kit::fixed_cell(a.label.clone(), 160.0, t.text))
                .child(num_cell(line.to_string(), 56.0, t.text_dim))
                .child(num_cell(a.hits.to_string(), 72.0, t.text))
                .child(kit::fixed_cell(st, 72.0, ink)),
            );
        }
    }
    table(head, body, t).key("cov-dut-branches")
}

/// Scene / test fixture: a small DUT with `if` / `case` branches and a
/// testbench that exercises part of it, copied to a temp directory and run
/// for real (never writes into the repo).
pub fn seed_fixture(s: &mut AppState) -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("helion-rune-scenes").join(format!("coverage-dut-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let dut = dir.join("cov_dut.sv");
    std::fs::write(&dut, FIXTURE_DUT).ok()?;
    std::fs::write(dir.join("cov_dut_tb.sv"), FIXTURE_TB).ok()?;
    s.sim.dir_override = Some(dir);
    crate::views::tb::rescan(s);
    let i = s.sim.tbs.iter().position(|t| t.file.name == "cov_dut_tb")?;
    let cfg = helion_verif::tb::TbRunConfig::new(s.sim.tbs[i].file.path.clone());
    s.sim.tbs[i].last = Some(helion_verif::tb::run_testbench(&cfg, None));
    s.sim.selected = Some(i);
    Some(dut)
}

const FIXTURE_DUT: &str = "\
// Mode-selected counter (coverage fixture).
module cov_dut (
    input  logic clk,
    input  logic rst,
    input  logic en,
    input  logic m1,
    input  logic m0,
    output logic [3:0] q,
    output logic wrap
);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin
        if (rst)
            cnt <= 4'd0;
        else if (en)
            cnt <= cnt + 4'd1;
    end
    always_comb begin
        case ({m1, m0})
            2'd0: q = cnt;
            2'd1: q = ~cnt;
            default: q = 4'd0;
        endcase
    end
    assign wrap = cnt[3];
endmodule
";

const FIXTURE_TB: &str = "\
// Exercises reset, counting and mode 1; never mode 2/3 or a wrap.
module cov_dut_tb;
    logic clk = 0;
    logic rst;
    logic en;
    logic m1;
    logic m0;
    logic [3:0] q;
    logic wrap;

    cov_dut dut (.clk(clk), .rst(rst), .en(en), .m1(m1), .m0(m0), .q(q), .wrap(wrap));

    always #5 clk = ~clk;

    initial begin
        rst = 1; en = 0; m1 = 0; m0 = 0;
        @(posedge clk);
        rst = 0; en = 1;
        repeat (3) @(posedge clk);
        m0 = 1;
        #1;
        $display(\"q=%0d\", q);
        $finish;
    end
endmodule
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_run_reports_dut_lines_branches_and_gutter_marks() {
        let mut s = crate::init();
        let dut = seed_fixture(&mut s).expect("fixture");
        let (_, c) = crate::views::coverage::latest(&s).expect("run");
        assert!(unavailable(c).is_none(), "{:?}", c.dut_line_note);
        // Line numbers of FIXTURE_DUT.
        let mark = |l| dut_mark(c, &dut, l);
        assert_eq!(mark(14).map(|m| m.state), Some(MarkState::Hit), "reset assignment");
        assert_eq!(mark(16).map(|m| m.state), Some(MarkState::Hit), "increment");
        assert_eq!(mark(21).map(|m| m.state), Some(MarkState::Hit), "mode 1 arm statement");
        assert_eq!(mark(22).map(|m| m.state), Some(MarkState::Missed), "default never taken");
        assert_eq!(mark(25).map(|m| m.state), Some(MarkState::Missed), "wrap never changes");
        let case = mark(19).expect("case line");
        assert_eq!(case.state, MarkState::Partial);
        assert!(case.help.contains("case: 2'd0") && case.help.contains("default 0×"), "{}", case.help);
        assert!(mark(11).is_none(), "declaration carries no statement");
        // The view: KPIs, tables, gutter through the overlay.
        assert_eq!((c.dut_arms_hit(), c.dut_arms_total()), (5, 7));
        s.coverage.overlay = true;
        open_dut(&mut s);
        let g = crate::views::coverage::gutter_mark(&s, 22).expect("gutter on the DUT");
        assert_eq!(g.state, MarkState::Missed);
        s.coverage.overlay = false;
        assert!(crate::views::coverage::gutter_mark(&s, 22).is_none());
    }
}
