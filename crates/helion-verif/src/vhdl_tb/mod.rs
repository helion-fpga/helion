//! Behavioral VHDL testbench simulation.
//!
//! `helion-vhdl` is a synthesis front end: it translates VHDL to
//! synthesizable SystemVerilog and drops `wait`, `report`, `assert` and
//! delays, so it cannot run a testbench. This module is a separate
//! simulation front end (lexer → parser → elaborator) over an event-driven
//! kernel that implements the IEEE 1076 simulation cycle: processes with
//! sensitivity lists and `wait on/until/for`, signals vs variables with
//! delta cycles, `after` delays (inertial and transport), resolved
//! `std_logic`, `report`/`assert … severity`, `std.env.stop`/`finish`,
//! enumerations, records, arrays, `numeric_std`, `textio` `write` /
//! `writeline`, and `math_real.uniform`.
//!
//! It lives in `helion-verif` next to the SystemVerilog testbench runner
//! because it produces the same [`TbOutcome`] (log, failures with
//! file:line, VCD, coverage, history) the IDE's Testbenches pane shows, and
//! shares its DUT lookup. `helion-sim` is a gate-level, cycle-stepped
//! simulator with no notion of processes, so the behavioral kernel cannot
//! live there; instead DUTs bridge into it:
//!
//! - a VHDL DUT the elaborator understands is simulated behaviorally in
//!   the same kernel (its internal signals appear in the VCD);
//! - an SV/Verilog DUT (or a VHDL DUT this front end cannot elaborate) is
//!   synthesized (`helion-sv` / `helion-vhdl`) and stepped in `helion-sim`
//!   on rising edges of its clock port, its outputs fed back as signal
//!   updates one delta later.

pub mod ast;
pub mod elab;
pub mod eval;
pub mod ir;
pub mod kernel;
pub mod lex;
pub mod ops;
pub mod parse;
pub mod value;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use crate::tb::{
    ClockGen, TbBranchCov, TbCoverage, TbDecl, TbError, TbFailure, TbLogLine, TbOutcome, TbProgram, TbRunConfig, TbSev,
    TbStreamEvent,
};
use ast::{Concurrent, DesignFile, Expr, Mode};

/// True for `.vhd` / `.vhdl`.
pub fn is_vhdl_path(p: &Path) -> bool {
    p.extension().is_some_and(|e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "vhd" | "vhdl"))
}

/// The testbench entity of a parsed file: no input ports, preferring the
/// one named like the file stem.
fn top_entity(df: &DesignFile, stem: &str) -> Option<String> {
    let portless: Vec<&ast::Entity> =
        df.entities.iter().filter(|e| e.ports.iter().all(|p| p.mode != Mode::In || p.default.is_some())).collect();
    portless
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(stem))
        .or_else(|| portless.last())
        .map(|e| e.name.clone())
}

fn stem_of(label: &str) -> String {
    Path::new(label).file_stem().and_then(|s| s.to_str()).unwrap_or("tb").to_ascii_lowercase()
}

/// Parse a VHDL testbench into the summary the IDE shows (DUT instance,
/// port map, clocks, signals). Statements are not kept (`initial` is empty);
/// [`run_vhdl_testbench`] elaborates the full file.
pub fn parse_vhdl_testbench(file: &str, src: &str) -> Result<TbProgram, TbError> {
    let df = parse::parse_design_file(file, src)?;
    let top = top_entity(&df, &stem_of(file)).ok_or_else(|| TbError::new(file, 0, "no testbench entity (an entity without input ports)"))?;
    let arch = df
        .archs
        .iter()
        .filter(|a| a.entity == top)
        .last()
        .ok_or_else(|| TbError::new(file, 0, format!("entity `{top}` has no architecture")))?;
    let mut prog = TbProgram {
        module: top.clone(),
        dut_module: String::new(),
        dut_instance: String::new(),
        port_map: Vec::new(),
        decls: Vec::new(),
        clocks: Vec::new(),
        initial: Vec::new(),
    };
    for d in &arch.decls {
        if let ast::Decl::Object { class: ast::ObjClass::Signal, names, ty, line, .. } = d {
            let width = match &ty.range {
                Some((Expr::Int(a), _, Expr::Int(b))) => ((a - b).unsigned_abs() + 1) as u32,
                Some(_) => 0,
                None => 1,
            };
            for n in names {
                prog.decls.push(TbDecl { name: n.clone(), width, init: None, line: *line });
            }
        }
    }
    // Time constants (`constant CLK_PERIOD : time := 10 ns;`) for clock
    // half-period detection.
    let mut times: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for d in &arch.decls {
        if let ast::Decl::Object { class: ast::ObjClass::Constant, names, init: Some(Expr::Time(fs)), .. } = d {
            for n in names {
                times.insert(n.clone(), *fs);
            }
        }
    }
    let time_of = |e: &Expr| match e {
        Expr::Time(fs) => Some(*fs),
        Expr::Name(n) => times.get(n).copied(),
        _ => None,
    };
    for c in &arch.stmts {
        match c {
            Concurrent::Instance { label, unit, ports, .. } if prog.dut_module.is_empty() => {
                prog.dut_module = unit.clone();
                prog.dut_instance = label.clone();
                for p in ports {
                    if let (Some(f), Expr::Name(a)) = (&p.formal, &p.actual) {
                        prog.port_map.push((f.clone(), a.clone()));
                    }
                }
            }
            // `clk <= not clk after T;`
            Concurrent::Assign { target: Expr::Name(t), arms, line } if arms.len() == 1 => {
                if let Some((Expr::Unary(ast::UnOp::Not, inner), Some(after))) = arms[0].0.elems.first() {
                    if matches!(&**inner, Expr::Name(n) if n == t) {
                        let half = match after {
                            Expr::Binary(ast::BinOp::Div, a, b) => match (time_of(a), &**b) {
                                (Some(fs), Expr::Int(d)) if *d != 0 => Some(fs / d),
                                _ => None,
                            },
                            other => time_of(other),
                        };
                        if let Some(h) = half {
                            prog.clocks.push(ClockGen { sig: t.clone(), half_period_units: (h / value::FS_PER_NS).max(1) as u64, line: *line });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(prog)
}

/// Load the testbench and the other VHDL files next to it (packages,
/// DUTs). Other testbench files are skipped; files this front end cannot
/// parse are left to the netlist bridge.
fn load_sources(tb: &Path, src: &str) -> Result<(Vec<elab::Source>, Vec<String>), TbError> {
    let label = tb.to_string_lossy().into_owned();
    let df = parse::parse_design_file(&label, src)?;
    let mut out = vec![elab::Source { label, file: df }];
    let mut notes = Vec::new();
    if let Some(dir) = tb.parent() {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && is_vhdl_path(p) && p != tb && !crate::tb::is_testbench_file(p)).collect())
            .unwrap_or_default();
        paths.sort();
        for p in paths {
            let l = p.to_string_lossy().into_owned();
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            match parse::parse_design_file(&l, &text) {
                Ok(f) => out.push(elab::Source { label: l, file: f }),
                Err(e) => notes.push(format!("{} not loaded for behavioral simulation ({e}); a DUT in it runs as a synthesized netlist", p.display())),
            }
        }
    }
    Ok((out, notes))
}

/// Run a VHDL testbench (see the module docs). Artifacts go where the SV
/// runner puts them: `<tb dir>/.helion/tb_runs/<stem>_<unix>/`.
pub fn run_vhdl_testbench(cfg: &TbRunConfig, tx: Option<&mpsc::Sender<TbStreamEvent>>) -> Result<TbOutcome, TbError> {
    let started = std::time::Instant::now();
    let tb_dir = cfg.tb_path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let file_label = cfg.tb_path.to_string_lossy().into_owned();
    let src = std::fs::read_to_string(&cfg.tb_path).map_err(|e| TbError::new(&file_label, 0, format!("cannot read: {e}")))?;
    let (sources, notes) = load_sources(&cfg.tb_path, &src)?;
    let top = top_entity(&sources[0].file, &stem_of(&file_label))
        .ok_or_else(|| TbError::new(&file_label, 0, "no testbench entity (an entity without input ports)"))?;
    let el = elab::Elaborator::new(&sources, Some(tb_dir.clone())).elaborate_top(&top)?;

    let mut log: Vec<TbLogLine> = Vec::new();
    let send = |ev: TbStreamEvent| {
        if let Some(tx) = tx {
            let _ = tx.send(ev);
        }
    };
    let push_log = |log: &mut Vec<TbLogLine>, time_ns: u64, sev: TbSev, text: String| {
        send(TbStreamEvent::Log { time_ns, sev, text: text.clone() });
        log.push(TbLogLine { time_ns, sev, text });
    };
    let (dut_unit, dut_label) = el.dut.as_ref().map(|(u, l, _)| (u.clone(), l.clone())).unwrap_or_default();
    let dut_desc = match el.bridges.first() {
        Some(b) => format!("{} (synthesized netlist, {})", b.unit, b.path.display()),
        None if !dut_unit.is_empty() => format!("{dut_unit} (behavioral VHDL)"),
        None => "none".into(),
    };
    push_log(&mut log, 0, TbSev::Info, format!("testbench {top}: DUT {dut_desc}"));
    for n in notes.iter().chain(el.log.iter()) {
        push_log(&mut log, 0, TbSev::Warning, n.clone());
    }

    let max_fs = (cfg.max_units as i64).saturating_mul(value::FS_PER_NS);
    let mut k = kernel::Kernel::new(&el, max_fs).map_err(|e| TbError::new(&file_label, 0, e))?;
    k.cancel = cfg.cancel.clone();
    // Failure snapshots: the DUT's port-mapped signals.
    if let Some((_, label, pm)) = &el.dut {
        for (formal, actual) in pm {
            if let Some(id) = el.signals.iter().position(|s| s.name == format!("{top}.{actual}")) {
                k.watch.push((format!("{label}.{formal}"), id));
            }
        }
        k.watch.sort();
    }
    let tx2 = tx.cloned();
    k.on_event = Some(Box::new(move |e: &kernel::KEvent| {
        let Some(tx) = &tx2 else { return };
        match e {
            kernel::KEvent::Log { time_fs, sev, text } => {
                let time_ns = (*time_fs / value::FS_PER_NS).max(0) as u64;
                let ev = if *sev == 4 {
                    TbStreamEvent::Progress { time_ns }
                } else {
                    TbStreamEvent::Log { time_ns, sev: sev_of(*sev), text: text.clone() }
                };
                let _ = tx.send(ev);
            }
            kernel::KEvent::Failure { time_fs, loc: _, message, signals } => {
                let _ = tx.send(TbStreamEvent::Failure {
                    file: String::new(),
                    line: 0,
                    message: message.clone(),
                    time_ns: (*time_fs / value::FS_PER_NS).max(0) as u64,
                    signals: signals.clone(),
                });
            }
        }
    }));
    let run = k.run();
    let mut failures: Vec<TbFailure> = Vec::new();
    let mut displays = 0u32;
    for e in &k.events {
        match e {
            kernel::KEvent::Log { time_fs, sev, text } if *sev != 4 => {
                displays += 1;
                log.push(TbLogLine { time_ns: (*time_fs / value::FS_PER_NS).max(0) as u64, sev: sev_of(*sev), text: text.clone() });
            }
            kernel::KEvent::Failure { time_fs, loc, message, signals } => failures.push(TbFailure {
                file: el.files.get(loc.file as usize).cloned().unwrap_or_else(|| file_label.clone()),
                line: loc.line as usize,
                message: message.clone(),
                time_ns: (*time_fs / value::FS_PER_NS).max(0) as u64,
                signals: signals.clone(),
                instance: dut_label.clone(),
            }),
            _ => {}
        }
    }
    let sim_time_ns = (k.now / value::FS_PER_NS).max(0) as u64;
    if let Err((loc, msg)) = run {
        if loc == ir::Loc::default() && msg.contains("timed out") {
            return Err(TbError::new(&file_label, 0, msg));
        }
        let file = el.files.get(loc.file as usize).cloned().unwrap_or_else(|| file_label.clone());
        let text = format!("{file}:{}: run-time error: {msg}", loc.line);
        send(TbStreamEvent::Log { time_ns: sim_time_ns, sev: TbSev::Error, text: text.clone() });
        log.push(TbLogLine { time_ns: sim_time_ns, sev: TbSev::Error, text });
        let signals = k.snapshot();
        failures.push(TbFailure { file, line: loc.line as usize, message: msg, time_ns: sim_time_ns, signals, instance: dut_label.clone() });
    }

    // Coverage: executable lines / branches of the testbench file.
    let mut lines = std::collections::BTreeMap::new();
    let mut branches = Vec::new();
    let mut seen_branch = std::collections::BTreeSet::new();
    for code in el.procs.iter().map(|p| &p.code).chain(el.funcs.iter().map(|f| &f.code)) {
        for op in &code.ops {
            let loc = match op {
                ir::Op::SigAssign { loc, .. }
                | ir::Op::VarAssign { loc, .. }
                | ir::Op::Wait { loc, .. }
                | ir::Op::Report { loc, .. }
                | ir::Op::Assert { loc, .. }
                | ir::Op::Case { loc, .. }
                | ir::Op::ForInit { loc, .. }
                | ir::Op::Builtin { loc, .. }
                | ir::Op::JumpIfNot(_, _, loc) => *loc,
                _ => continue,
            };
            if loc.file != 0 {
                continue;
            }
            lines.insert(loc.line as usize, k.line_hits.get(&(0, loc.line)).copied().unwrap_or(0));
            if matches!(op, ir::Op::JumpIfNot(..)) && seen_branch.insert(loc.line) {
                let (t, f) = k.branch_hits.get(&(0, loc.line)).copied().unwrap_or((0, 0));
                branches.push(TbBranchCov { line: loc.line as usize, kind: "if", true_hits: t, false_hits: f });
            }
        }
    }
    let dut_file = match k.bridges.first() {
        Some(b) => b.path.clone(),
        None => el
            .dut
            .as_ref()
            .and_then(|(u, _, _)| sources.iter().find(|s| s.file.entities.iter().any(|e| &e.name == u)))
            .map(|s| PathBuf::from(&s.label))
            .unwrap_or_default(),
    };
    let coverage = TbCoverage {
        tb_file: cfg.tb_path.clone(),
        dut_file,
        lines: lines.into_iter().collect(),
        branches,
        dut: k.bridges.first().map(|b| b.sim_coverage()).unwrap_or_default(),
        // Per-line DUT attribution comes from the SV testbench runner's
        // instrumented synthesis; the VHDL kernel does not collect it yet.
        dut_lines: Vec::new(),
        dut_branches: Vec::new(),
        dut_not_instrumented: Vec::new(),
        dut_line_note: Some(
            "DUT line coverage is collected for SystemVerilog testbenches only; \
             this VHDL testbench reports testbench line/branch and DUT toggle coverage"
                .into(),
        ),
    };

    // Artifacts.
    let stem = cfg.tb_path.file_stem().and_then(|s| s.to_str()).unwrap_or("tb").to_string();
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let run_dir = tb_dir.join(".helion").join("tb_runs").join(format!("{stem}_{unix}"));
    std::fs::create_dir_all(&run_dir).map_err(|e| TbError::new(&file_label, 0, format!("cannot create {}: {e}", run_dir.display())))?;
    let runtime_ms = started.elapsed().as_millis() as u64;
    let stopped_by_user = k.cancelled;
    let passed = failures.is_empty() && !stopped_by_user;
    let summary = if stopped_by_user {
        format!("{stem} STOPPED by user at {sim_time_ns} ns (not a pass/fail)")
    } else if passed {
        format!("{stem} PASSED: {displays} reports, sim_time={sim_time_ns} ns, wall={runtime_ms} ms")
    } else {
        format!("{stem} FAILED: {} error(s), {displays} reports, sim_time={sim_time_ns} ns", failures.len())
    };
    let sev = if passed { TbSev::Info } else if stopped_by_user { TbSev::Warning } else { TbSev::Error };
    send(TbStreamEvent::Log { time_ns: sim_time_ns, sev, text: summary.clone() });
    log.push(TbLogLine { time_ns: sim_time_ns, sev, text: summary.clone() });
    let log_path = run_dir.join("run.log");
    let text: String = log.iter().map(|l| format!("[{:>10} ns] {:<7} {}\n", l.time_ns, l.sev.tag(), l.text)).collect();
    std::fs::write(&log_path, text).map_err(|e| TbError::new(&file_label, 0, format!("cannot write log: {e}")))?;
    let vcd_path = if cfg.vcd {
        let p = run_dir.join("run.vcd");
        crate::tb::write_vcd(&p, &k.trace_names, &k.vcd).map_err(|e| TbError::new(&file_label, 0, format!("cannot write VCD: {e}")))?;
        Some(p)
    } else {
        None
    };
    Ok(TbOutcome {
        uvm: None,
        passed,
        stopped: stopped_by_user,
        sim_time_ns,
        failures,
        displays,
        vcd_path,
        log_path,
        log,
        summary,
        runtime_ms,
        dut_module: dut_unit,
        dut_instance: dut_label,
        coverage,
    })
}

fn sev_of(s: u32) -> TbSev {
    match s {
        0 => TbSev::Info,
        1 => TbSev::Warning,
        _ => TbSev::Error,
    }
}
