//! Class-based and UVM testbenches on the `helion-svtb` engine.
//!
//! [`crate::tb::run_testbench`] routes a testbench here when its source
//! uses classes, packages, interfaces, macros, `fork`, queues or UVM
//! ([`helion_svtb::wants_engine`]). The result is the same
//! [`TbOutcome`] the stimulus-subset runner produces (streamed log lines,
//! clickable failures with file:line, `run.log`, optional VCD, testbench
//! line/branch coverage of the top file, DUT netlist + RTL line coverage),
//! plus a [`UvmSummary`] when a UVM test ran.
//!
//! DUT modules (instantiated but not defined in the testbench sources) are
//! looked up in the testbench directory, then its parent directory, and
//! synthesized with [`crate::tb::synth_dut_for_coverage`].

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use crate::tb::{
    TbBranchCov, TbCoverage, TbError, TbFailure, TbLogLine, TbOutcome, TbRunConfig, TbSev, TbStreamEvent,
    find_dut_source, synth_dut_for_coverage, write_vcd,
};

/// UVM end-of-test counts (from the report server summary).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UvmSummary {
    /// The test that ran (`Running test X...`).
    pub test: Option<String>,
    pub info: u32,
    pub warning: u32,
    pub error: u32,
    pub fatal: u32,
}

/// One parsed UVM report line:
/// `UVM_ERROR file(line) @ time: path [ID] message`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UvmMsg {
    /// `UVM_INFO` / `UVM_WARNING` / `UVM_ERROR` / `UVM_FATAL`.
    pub severity: String,
    pub file: Option<String>,
    pub line: usize,
    pub time: u64,
    /// Component path (`uvm_test_top.env.scb`) or `reporter`.
    pub path: String,
    pub id: String,
    pub message: String,
}

/// Parse a UVM report line produced by the Helion UVM library.
pub fn parse_uvm_message(text: &str) -> Option<UvmMsg> {
    let (sev, rest) = text.split_once(' ')?;
    if !matches!(sev, "UVM_INFO" | "UVM_WARNING" | "UVM_ERROR" | "UVM_FATAL") {
        return None;
    }
    let at = rest.find("@ ")?;
    let loc = rest[..at].trim();
    let (file, line) = if loc.is_empty() {
        (None, 0)
    } else {
        let open = loc.rfind('(')?;
        let l: usize = loc[open + 1..loc.len().checked_sub(1)?].parse().ok()?;
        (Some(loc[..open].to_string()), l)
    };
    let after = &rest[at + 2..];
    let (time, after) = after.split_once(": ")?;
    let time: u64 = time.trim().parse().ok()?;
    let lb = after.find(" [")?;
    let path = after[..lb].trim().to_string();
    let rb = after[lb + 2..].find("] ")? + lb + 2;
    let id = after[lb + 2..rb].to_string();
    let message = after[rb + 2..].to_string();
    Some(UvmMsg { severity: sev.to_string(), file, line, time, path, id, message })
}

/// Facts about a class-based testbench for the detail panel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClassTbInfo {
    pub uses_uvm: bool,
    /// UVM tests found in the sources (classes deriving from `uvm_test`).
    pub uvm_tests: Vec<String>,
    /// `(DUT module, instance)` pairs.
    pub duts: Vec<(String, String)>,
    /// Classes declared in the testbench sources (UVM library excluded).
    pub classes: usize,
    /// Test named by `run_test("…")` in the top file (used without
    /// `+UVM_TESTNAME`).
    pub default_test: Option<String>,
}

/// Does this testbench source need the class-based engine?
pub fn is_class_testbench(src: &str) -> bool {
    helion_svtb::wants_engine(src)
}

/// Parse a class-based testbench (no simulation).
pub fn inspect_class_testbench(path: &Path) -> Result<ClassTbInfo, TbError> {
    let prog = helion_svtb::compile(&[path.to_path_buf()], &Default::default()).map_err(sv_err)?;
    let mut classes = 0;
    let count = |items: &[helion_svtb::ast::PkgItem]| items.iter().filter(|i| matches!(i, helion_svtb::ast::PkgItem::Class(_))).count();
    for p in &prog.unit.packages {
        if p.name != "uvm_pkg" {
            classes += count(&p.items);
        }
    }
    classes += count(&prog.unit.unit);
    let src = std::fs::read_to_string(path).unwrap_or_default();
    let default_test = src.split("run_test(").nth(1).and_then(|r| {
        let r = r.trim_start().strip_prefix('"')?;
        r.split('"').next().filter(|n| !n.is_empty()).map(str::to_string)
    });
    Ok(ClassTbInfo {
        uses_uvm: prog.uses_uvm,
        uvm_tests: helion_svtb::uvm_tests(&prog),
        duts: helion_svtb::dut_modules(&prog),
        classes,
        default_test,
    })
}

fn sv_err(e: helion_svtb::SvError) -> TbError {
    TbError::new(&e.file, e.line, e.msg)
}

struct Provider {
    dirs: Vec<PathBuf>,
    notes: Vec<(PathBuf, Option<String>)>,
}

impl helion_svtb::DutProvider for Provider {
    fn synth(&mut self, module: &str) -> Result<(helion_ir::Design, PathBuf), String> {
        let mut last = String::new();
        for d in &self.dirs {
            match find_dut_source(d, module) {
                Ok(p) => {
                    let (design, note) = synth_dut_for_coverage(&p).map_err(|e| format!("synthesis of {} failed: {e}", p.display()))?;
                    self.notes.push((p.clone(), note));
                    return Ok((design, p));
                }
                Err(e) => last = e.msg,
            }
        }
        Err(last)
    }
}

fn tb_sev(s: helion_svtb::Sev) -> TbSev {
    match s {
        helion_svtb::Sev::Info => TbSev::Info,
        helion_svtb::Sev::Warning => TbSev::Warning,
        _ => TbSev::Error,
    }
}

fn uvm_summary(log: &[TbLogLine]) -> Option<UvmSummary> {
    let mut s = UvmSummary::default();
    let mut seen = false;
    for l in log {
        let t = l.text.as_str();
        if let Some(rest) = t.split("Running test ").nth(1) {
            if t.starts_with("UVM_INFO") {
                s.test = Some(rest.trim_end_matches("...").to_string());
            }
        }
        for (k, slot) in [("UVM_INFO : ", 0), ("UVM_WARNING : ", 1), ("UVM_ERROR : ", 2), ("UVM_FATAL : ", 3)] {
            if let Some(n) = t.strip_prefix(k).and_then(|n| n.trim().parse::<u32>().ok()) {
                seen = true;
                match slot {
                    0 => s.info = n,
                    1 => s.warning = n,
                    2 => s.error = n,
                    _ => s.fatal = n,
                }
            }
        }
    }
    if !seen {
        // A fatal before the summary, or no summary: count report lines.
        for l in log {
            if let Some(m) = parse_uvm_message(&l.text) {
                seen = true;
                match m.severity.as_str() {
                    "UVM_INFO" => s.info += 1,
                    "UVM_WARNING" => s.warning += 1,
                    "UVM_ERROR" => s.error += 1,
                    _ => s.fatal += 1,
                }
            }
        }
    }
    (seen || s.test.is_some()).then_some(s)
}

/// Run a class-based / UVM testbench end to end.
pub fn run_class_testbench(cfg: &TbRunConfig, tx: Option<&mpsc::Sender<TbStreamEvent>>) -> Result<TbOutcome, TbError> {
    let started = std::time::Instant::now();
    let tb_dir = cfg.tb_path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let prog = helion_svtb::compile(&[cfg.tb_path.clone()], &Default::default()).map_err(sv_err)?;
    let mut dirs = vec![tb_dir.clone()];
    if let Some(p) = tb_dir.parent() {
        if !p.as_os_str().is_empty() {
            dirs.push(p.to_path_buf());
        }
    }
    let mut provider = Provider { dirs, notes: Vec::new() };
    let run_cfg = helion_svtb::RunConfig {
        seed: cfg.seed,
        max_ps: cfg.max_units.saturating_mul(crate::tb::TB_PS_PER_UNIT),
        plusargs: cfg.plusargs.clone(),
        vcd: cfg.vcd,
        cancel: cfg.cancel.clone(),
        top: None,
        ..Default::default()
    };
    let mut sink = |e: helion_svtb::Event| {
        let Some(tx) = tx else { return };
        let _ = match e {
            helion_svtb::Event::Log { time_ps, sev, text } => tx.send(TbStreamEvent::Log { time_ns: time_ps / 1000, sev: tb_sev(sev), text }),
            helion_svtb::Event::Failure { file, line, msg, time_ps } => {
                tx.send(TbStreamEvent::Failure { file, line, message: msg, time_ns: time_ps / 1000, signals: vec![] })
            }
            helion_svtb::Event::Progress { time_ps } => tx.send(TbStreamEvent::Progress { time_ns: time_ps / 1000 }),
        };
    };
    let res = helion_svtb::run(&prog, &mut provider, run_cfg, &mut sink).map_err(sv_err)?;

    let stem = cfg.tb_path.file_stem().and_then(|s| s.to_str()).unwrap_or("tb").to_string();
    let sim_time_ns = res.time_ps / 1000;
    let mut log: Vec<TbLogLine> = res.log.iter().map(|l| TbLogLine { time_ns: l.time_ps / 1000, sev: tb_sev(l.sev), text: l.text.clone() }).collect();
    let (dut_module, dut_instance, dut_file) = res
        .duts
        .first()
        .map(|d| (d.module.clone(), d.instance.clone(), d.src.clone()))
        .unwrap_or_default();
    let failures: Vec<TbFailure> = res
        .failures
        .iter()
        .map(|f| TbFailure { file: f.file.clone(), line: f.line, message: f.msg.clone(), time_ns: f.time_ps / 1000, signals: vec![], instance: dut_instance.clone() })
        .collect();

    // Coverage: statement / branch lines of the top testbench file.
    let top = res.top_file;
    let mut lines = Vec::new();
    let mut branches = Vec::new();
    for ((f, l), kind) in helion_svtb::cov::statement_lines(&prog.unit) {
        if f != top || l == 0 {
            continue;
        }
        lines.push((l as usize, res.line_hits.get(&(f, l)).copied().unwrap_or(0)));
        if let Some(kind) = kind {
            let (t, fl) = res.branches.get(&(f, l)).map(|b| (b.0, b.1)).unwrap_or((0, 0));
            branches.push(TbBranchCov { line: l as usize, kind, true_hits: t, false_hits: fl });
        }
    }
    let mut coverage = TbCoverage { tb_file: cfg.tb_path.clone(), dut_file: dut_file.clone(), lines, branches, ..Default::default() };
    if let Some(d) = res.duts.first() {
        coverage.dut = d.cov.clone();
        let note = provider.notes.iter().find(|(p, _)| *p == d.src).and_then(|(_, n)| n.clone());
        coverage.set_dut_line_coverage(d.lines.clone(), note);
    }

    // Artifacts: <tb_dir>/.helion/tb_runs/<stem>_<unix>/{run.log,run.vcd}
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let run_dir = tb_dir.join(".helion").join("tb_runs").join(format!("{stem}_{unix}"));
    let label = cfg.tb_path.to_string_lossy().into_owned();
    std::fs::create_dir_all(&run_dir).map_err(|e| TbError::new(&label, 0, format!("cannot create {}: {e}", run_dir.display())))?;
    let vcd_path = if cfg.vcd {
        let p = run_dir.join("run.vcd");
        let samples: Vec<(u64, usize, u64, bool)> = res.samples.iter().map(|(t, i, v)| (*t, *i, *v, false)).collect();
        write_vcd(&p, &res.trace, &samples).map_err(|e| TbError::new(&label, 0, format!("cannot write VCD: {e}")))?;
        Some(p)
    } else {
        None
    };
    let uvm = if prog.uses_uvm { uvm_summary(&log) } else { None };
    let runtime_ms = started.elapsed().as_millis() as u64;
    let stopped = res.cancelled;
    let passed = failures.is_empty() && !stopped;
    let summary = if stopped {
        format!("{stem} STOPPED by user at {sim_time_ns} ns (not a pass/fail)")
    } else {
        let uvm_part = uvm
            .as_ref()
            .map(|u| format!(" [{}: {} error(s), {} fatal, {} warning(s)]", u.test.as_deref().unwrap_or("UVM"), u.error, u.fatal, u.warning))
            .unwrap_or_default();
        if passed {
            format!("{stem} PASSED{uvm_part}: {} displays, sim_time={sim_time_ns} ns, wall={runtime_ms} ms", res.displays)
        } else {
            format!("{stem} FAILED{uvm_part}: {} error(s), sim_time={sim_time_ns} ns", failures.len())
        }
    };
    let summary_line = TbLogLine {
        time_ns: sim_time_ns,
        sev: if passed { TbSev::Info } else if stopped { TbSev::Warning } else { TbSev::Error },
        text: summary.clone(),
    };
    if let Some(tx) = tx {
        let _ = tx.send(TbStreamEvent::Log { time_ns: summary_line.time_ns, sev: summary_line.sev, text: summary_line.text.clone() });
    }
    log.push(summary_line);
    let log_path = run_dir.join("run.log");
    let mut text = String::new();
    for l in &log {
        text.push_str(&format!("[{:>10} ns] {:<7} {}\n", l.time_ns, l.sev.tag(), l.text));
    }
    std::fs::write(&log_path, text).map_err(|e| TbError::new(&label, 0, format!("cannot write log: {e}")))?;
    Ok(TbOutcome {
        passed,
        stopped,
        sim_time_ns,
        failures,
        displays: res.displays,
        vcd_path,
        log_path,
        log,
        summary,
        runtime_ms,
        dut_module,
        dut_instance,
        coverage,
        uvm,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn uvm_message_lines_parse() {
        let m = parse_uvm_message("UVM_ERROR /a/b/pkg.sv(42) @ 55: uvm_test_top.env.scb [SCB] cycle 5: led=1").unwrap();
        assert_eq!(m.severity, "UVM_ERROR");
        assert_eq!(m.file.as_deref(), Some("/a/b/pkg.sv"));
        assert_eq!((m.line, m.time), (42, 55));
        assert_eq!(m.path, "uvm_test_top.env.scb");
        assert_eq!(m.id, "SCB");
        assert_eq!(m.message, "cycle 5: led=1");
        let r = parse_uvm_message("UVM_INFO @ 0: reporter [RNTST] Running test t...").unwrap();
        assert_eq!((r.file, r.path.as_str(), r.id.as_str()), (None, "reporter", "RNTST"));
        assert!(parse_uvm_message("plain display").is_none());
    }

    #[test]
    fn shipped_uvm_counter_example_passes_through_run_testbench() {
        let tb = repo().join("examples/uvm_counter/uvm_counter_tb.sv");
        // Run on a copy so artifacts stay in the temp dir.
        let d = std::env::temp_dir().join(format!("helion_classtb_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let ex = d.join("uvm_counter");
        std::fs::create_dir_all(&ex).unwrap();
        for f in ["counter_if.sv", "counter_pkg.sv", "uvm_counter_tb.sv"] {
            std::fs::copy(tb.parent().unwrap().join(f), ex.join(f)).unwrap();
        }
        std::fs::copy(repo().join("examples/counter.sv"), d.join("counter.sv")).unwrap();
        let mut cfg = TbRunConfig::new(ex.join("uvm_counter_tb.sv"));
        cfg.vcd = true;
        let (stx, srx) = mpsc::channel();
        let out = crate::tb::run_testbench(&cfg, Some(&stx)).unwrap();
        drop(stx);
        assert!(out.passed, "{}\n{:?}", out.summary, out.failures);
        let u = out.uvm.clone().expect("uvm summary");
        assert_eq!(u.test.as_deref(), Some("counter_test"));
        assert_eq!((u.error, u.fatal), (0, 0));
        assert!(u.info > 0);
        assert!(out.summary.contains("PASSED [counter_test: 0 error(s)"), "{}", out.summary);
        assert_eq!((out.dut_module.as_str(), out.dut_instance.as_str()), ("counter", "dut"));
        assert!(out.coverage.dut_file.ends_with("counter.sv"));
        assert!(!out.coverage.lines.is_empty() && out.coverage.lines_hit() > 0);
        assert!(out.coverage.dut.toggle_bin_coverage_pct() > 0.0);
        assert!(out.vcd_path.as_ref().is_some_and(|p| std::fs::read_to_string(p).unwrap().contains("$enddefinitions")));
        let streamed: Vec<TbStreamEvent> = srx.try_iter().collect();
        assert!(streamed.iter().any(|e| matches!(e, TbStreamEvent::Log { text, .. } if text.contains("[RNTST]"))));
        // A different test by plusarg; and a deliberately broken DUT fails.
        cfg.plusargs = vec!["+UVM_TESTNAME=counter_long_test".into()];
        cfg.vcd = false;
        let long = crate::tb::run_testbench(&cfg, None).unwrap();
        assert!(long.passed && long.uvm.unwrap().test.as_deref() == Some("counter_long_test"));
        std::fs::write(d.join("counter.sv"), "module counter(input logic clk, output logic led);\n logic [3:0] c;\n always_ff @(posedge clk) c <= c + 1;\n assign led = c[2];\nendmodule\n").unwrap();
        cfg.plusargs.clear();
        let bad = crate::tb::run_testbench(&cfg, None).unwrap();
        assert!(!bad.passed);
        assert!(bad.uvm.as_ref().unwrap().error > 0, "{:?}", bad.uvm);
        assert!(bad.failures[0].file.ends_with("counter_pkg.sv") && bad.failures[0].line > 0);
        let info = inspect_class_testbench(&cfg.tb_path).unwrap();
        assert_eq!(info.uvm_tests, ["counter_long_test", "counter_test"]);
        assert_eq!(info.default_test.as_deref(), Some("counter_test"));
        assert_eq!(info.duts, [("counter".to_string(), "dut".to_string())]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
