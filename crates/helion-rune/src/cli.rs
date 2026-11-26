//! Command-line modes of `helion-ide` that never open a window: version,
//! doctor, help, one-shot timing (`--headless`), Tcl batch (`--source`) and a
//! line-by-line Tcl console on stdin (`--stdin`). Every mode drives the same
//! engine and Tcl interpreter as the IDE window ([`crate::tcl::eval_batch`]).

use std::io::{BufRead, Write};
use std::path::Path;

pub const BIN: &str = "helion-ide";

pub fn usage() -> String {
    format!(
        "{BIN} {ver} — Helion FPGA IDE

  {BIN}                          open the IDE (Start page, recent projects)
  {BIN} <project.prj|file.sv>    open the IDE on a project or source
  {BIN} --headless <file>        synthesize, place, route and report timing; prints WNS_PS
  {BIN} --headless <top.sv> --part <7-series part>
                                 same on a lone source; constraints from <top>.xdc next to it
  {BIN} --source <script.tcl>    run a Tcl script in batch mode (exit 1 on the first error)
  {BIN} --stdin                  Tcl console on stdin, one command per line (`quit` ends)
  {BIN} --version                version line
  {BIN} --doctor                 target triple, bundled device data, self-check
  {BIN} --help                   this text

Headless rendering for docs/tests: --snap out.png [--scene name] [--tree].
The `helion` command-line tool covers scripted flows without the IDE.",
        ver = env!("CARGO_PKG_VERSION")
    )
}

/// Handle the windowless modes. `None` means "open the window" (with any
/// remaining arguments left for the window to interpret).
pub fn run(args: &[String]) -> Option<i32> {
    let first = args.first().map(String::as_str);
    Some(match first {
        Some("--version") | Some("-V") | Some("version") => {
            println!("{}", crate::doctor::version_line());
            0
        }
        Some("--doctor") | Some("doctor") => {
            print!("{}", crate::doctor::doctor_report());
            0
        }
        Some("--help") | Some("-h") | Some("help") => {
            println!("{}", usage());
            0
        }
        Some("--headless") => match args.get(1) {
            Some(path) => {
                let part = args.iter().position(|a| a == "--part").and_then(|i| args.get(i + 1));
                headless(Path::new(path), part.map(String::as_str))
            }
            None => stdin_console(),
        },
        Some("--stdin") => stdin_console(),
        Some("--source") | Some("-source") => match args.get(1) {
            Some(path) => source(Path::new(path)),
            None => {
                eprintln!("{BIN}: --source needs a Tcl script path");
                2
            }
        },
        _ => return None,
    })
}

fn state() -> crate::AppState {
    crate::init()
}

/// Load a source or project into a batch session.
fn open(s: &mut crate::AppState, path: &Path) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!("{}: no such file", path.display()));
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let quoted = format!("{{{}}}", path.display());
    match ext.as_str() {
        "prj" => {
            crate::pickers::open_path(s, path);
            crate::tcl::eval_batch(s, "synth_design")
        }
        "vhd" | "vhdl" => crate::tcl::eval_batch(s, &format!("read_vhdl {quoted}")),
        _ => crate::tcl::eval_batch(s, &format!("read_verilog {quoted}")),
    }
}

/// `--headless <file>`: the full flow plus timing, as one command.
fn headless(path: &Path, part: Option<&str>) -> i32 {
    if let Some(code) = headless_xilinx7(path, part) {
        return code;
    }
    let mut s = state();
    if let Err(e) = open(&mut s, path) {
        eprintln!("{BIN}: {e}");
        return 1;
    }
    match crate::tcl::eval_batch(&mut s, "report_timing") {
        Ok(out) => {
            println!("{out}");
            match crate::tcl::wns_ps(&mut s) {
                Ok(wns) => {
                    println!("WNS_PS={wns}");
                    0
                }
                Err(e) => {
                    eprintln!("{BIN}: no closed WNS: {e}");
                    1
                }
            }
        }
        Err(e) => {
            eprintln!("{BIN}: report_timing: {e}");
            1
        }
    }
}

/// A `.prj` on a 7-series part, or a lone HDL source with `--part` naming a
/// 7-series part (its `<stem>.xdc` beside it), runs the XDC flow and prints
/// the view contract's reports, device layers, heat maps and usage line.
/// `None` for any other source (the Tcl path handles it).
fn headless_xilinx7(path: &Path, part: Option<&str>) -> Option<i32> {
    let part = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("prj")) {
        helion_proj::load_prj(&std::fs::read_to_string(path).ok()?).ok()?.part
    } else {
        part?.to_string()
    };
    if !crate::runs_family::is_xilinx7(&part) {
        return None;
    }
    let out = std::env::temp_dir().join(format!("helion-ide-headless-{}", std::process::id()));
    let art = crate::runs_family::run_xdc_flow(path, &part, &out, &|l, _| eprintln!("{l}"));
    let _ = std::fs::remove_dir_all(&out);
    Some(match art.map(|a| a.view) {
        Ok(Some(v)) => {
            use crate::views::contract_views as cv;
            print!("{}", cv::render_all_reports(&v));
            print!("{}", cv::render_device_tab(&v));
            print!("{}", cv::summary_lines(&v));
            println!("{}", cv::usage_summary(&v));
            match &v.timing {
                Some(t) => {
                    println!("WNS_PS={}", t.wns_ps);
                    0
                }
                None => 1,
            }
        }
        Ok(None) => {
            eprintln!("{BIN}: the run produced no implementation view");
            1
        }
        Err(e) => {
            eprintln!("{BIN}: {e}");
            1
        }
    })
}

/// `--source <script.tcl>`: Vivado-style batch mode.
fn source(path: &Path) -> i32 {
    let script = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{BIN}: {}: {e}", path.display());
            return 1;
        }
    };
    let mut s = state();
    match crate::tcl::eval_batch(&mut s, &script) {
        Ok(out) => {
            if !out.is_empty() {
                println!("{out}");
            }
            0
        }
        Err(e) => {
            eprintln!("ERROR: {e}");
            1
        }
    }
}

/// `--stdin`: one Tcl command per line; `open <file>` loads a design,
/// `quit`/`exit` (or EOF) ends. Errors print as `ERROR:` and the console
/// keeps going, like the IDE's Tcl console.
fn stdin_console() -> i32 {
    let mut s = state();
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "quit" || t == "exit" {
            break;
        }
        let r = match t.strip_prefix("open ") {
            Some(p) => open(&mut s, Path::new(p.trim())),
            None => crate::tcl::eval_batch(&mut s, t),
        };
        let _ = match r {
            Ok(v) if v.is_empty() => Ok(()),
            Ok(v) => writeln!(out, "{v}"),
            Err(e) => writeln!(out, "ERROR: {e}"),
        };
        let _ = out.flush();
    }
    0
}
