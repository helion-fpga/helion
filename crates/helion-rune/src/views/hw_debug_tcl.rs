//! Tcl verbs for the hardware debug cores (Vivado names): Set Up Debug,
//! hw ILA, VIO and JTAG-to-AXI. Each verb drives [`crate::views::hw_debug`].

use helion_debug::{AxiTxnKind, DebugCoreKind, create_hw_axi_txn};
use rune::TextEdit;

use crate::state::AppState;
use crate::views::hw_debug::{
    TrigMode, implement, marked_nets, parse_num, parse_words, program, refresh_vio, run_axi,
    run_ila, set_output, show_in_viewer, wait_idle,
};

/// Verbs this module owns (registered in `tcl::COMMANDS`).
pub const VERBS: &[&str] = &[
    "implement_debug_core",
    "write_debug_probes",
    "refresh_hw_device",
    "get_hw_ilas",
    "get_hw_vios",
    "get_hw_axis",
    "run_hw_ila",
    "wait_on_hw_ila",
    "upload_hw_ila_data",
    "display_hw_ila_data",
    "refresh_hw_vio",
    "create_hw_axi_txn",
    "run_hw_axi",
    "get_hw_axi_txns",
];

pub fn help(verb: &str) -> Option<(&'static str, &'static str)> {
    Some(match verb {
        "implement_debug_core" => (
            "implement_debug_core ?-probes {nets}? ?-depth n? ?-trigger_net n? ?-vio_in {nets}? ?-vio_out {nets}? ?-axi?",
            "insert ILA/VIO/JTAG-to-AXI cores and re-implement (writes .ltx + .hbits)",
        ),
        "write_debug_probes" => ("write_debug_probes file.ltx", "write the debug probes file"),
        "refresh_hw_device" => (
            "refresh_hw_device",
            "program the debug bitstream on the Simulation cable and list its cores",
        ),
        "get_hw_ilas" | "get_hw_vios" | "get_hw_axis" => {
            ("get_hw_ilas|get_hw_vios|get_hw_axis", "list debug cores on the hub")
        }
        "run_hw_ila" => (
            "run_hw_ila ?-trigger immediate|rising|falling|value? ?-value v? ?-mask m? ?-window n? ?-pre n?",
            "arm the hardware ILA (returns at once; wait_on_hw_ila blocks)",
        ),
        "wait_on_hw_ila" => ("wait_on_hw_ila ?-timeout s?", "wait for the hardware job to finish"),
        "upload_hw_ila_data" | "display_hw_ila_data" => (
            "upload_hw_ila_data|display_hw_ila_data",
            "show the last hardware window in the Logic Analyzer",
        ),
        "refresh_hw_vio" => ("refresh_hw_vio ?-cycles n?", "run clocks and sample VIO inputs"),
        "create_hw_axi_txn" => (
            "create_hw_axi_txn name addr -type read|write ?-len n? ?-data {words}?",
            "define a JTAG-to-AXI transaction",
        ),
        "run_hw_axi" => ("run_hw_axi name…", "execute transactions (logged in the AXI console)"),
        "get_hw_axi_txns" => ("get_hw_axi_txns", "list defined AXI transactions"),
        _ => return None,
    })
}

fn opt<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
}

fn list(v: &str) -> Vec<String> {
    v.split_whitespace()
        .map(|w| w.trim_matches(|c| c == '{' || c == '}').to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

fn cores_of(s: &AppState, kind: DebugCoreKind) -> String {
    s.program
        .debug
        .snap
        .as_ref()
        .map(|snap| {
            snap.cores
                .iter()
                .filter(|c| c.kind == kind)
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

pub fn tcl_verb(s: &mut AppState, verb: &str, args: &[String]) -> Result<String, String> {
    match verb {
        "implement_debug_core" => {
            let d = &mut s.program.debug;
            if let Some(p) = opt(args, "-probes") {
                d.ila_nets = list(p);
            }
            if let Some(p) = opt(args, "-vio_in") {
                d.vio_in = list(p);
            }
            if let Some(p) = opt(args, "-vio_out") {
                d.vio_out = list(p);
            }
            if let Some(n) = opt(args, "-depth") {
                d.depth = (parse_num(n)? as usize).clamp(1, 255);
            }
            if let Some(n) = opt(args, "-trigger_net") {
                d.trigger_net = Some(n.to_string());
            }
            if args.iter().any(|a| a == "-axi") {
                d.jtag_axi = true;
            }
            if s.program.debug.ila_nets.is_empty() {
                s.program.debug.ila_nets = marked_nets(s);
            }
            implement(s)
        }
        "write_debug_probes" => {
            let path = args
                .iter()
                .find(|a| !a.starts_with('-'))
                .ok_or("write_debug_probes: missing file")?;
            let b = s
                .program
                .debug
                .build
                .clone()
                .ok_or("write_debug_probes: no debug build (implement_debug_core first)")?;
            b.ltx().write_ltx(std::path::Path::new(path))?;
            Ok(format!("write_debug_probes {path}"))
        }
        "refresh_hw_device" => program(s),
        "get_hw_ilas" => Ok(cores_of(s, DebugCoreKind::Ila)),
        "get_hw_vios" => Ok(cores_of(s, DebugCoreKind::Vio)),
        "get_hw_axis" => Ok(cores_of(s, DebugCoreKind::JtagAxi)),
        "run_hw_ila" => {
            let d = &mut s.program.debug;
            if let Some(k) = opt(args, "-trigger") {
                d.trig = TrigMode::parse(k).ok_or_else(|| format!("run_hw_ila: -trigger {k}?"))?;
            }
            if let Some(v) = opt(args, "-value") {
                d.trig_value = TextEdit::new(v);
                d.trig = TrigMode::Value;
            }
            if let Some(v) = opt(args, "-mask") {
                d.trig_mask = TextEdit::new(v);
            }
            if let Some(v) = opt(args, "-window") {
                d.window = TextEdit::new(v);
            }
            if let Some(v) = opt(args, "-pre") {
                d.pre = TextEdit::new(v);
            }
            run_ila(s)
        }
        "wait_on_hw_ila" => {
            let secs = opt(args, "-timeout")
                .map(parse_num)
                .transpose()?
                .unwrap_or(60);
            if !wait_idle(s, std::time::Duration::from_secs(secs)) {
                return Err(format!("wait_on_hw_ila: still busy after {secs} s"));
            }
            match &s.program.debug.last_error {
                Some(e) => Err(e.clone()),
                None => Ok("wait_on_hw_ila: done".into()),
            }
        }
        "upload_hw_ila_data" | "display_hw_ila_data" => show_in_viewer(s),
        "refresh_hw_vio" => {
            if let Some(v) = opt(args, "-cycles") {
                s.program.debug.vio_cycles = TextEdit::new(v);
            }
            refresh_vio(s)
        }
        "create_hw_axi_txn" => {
            let pos: Vec<&String> = {
                let mut v = Vec::new();
                let mut i = 0;
                while i < args.len() {
                    if args[i].starts_with('-') {
                        i += 2;
                    } else {
                        v.push(&args[i]);
                        i += 1;
                    }
                }
                v
            };
            let name = pos.first().ok_or("create_hw_axi_txn: missing name")?;
            let addr = pos.get(1).ok_or("create_hw_axi_txn: missing address")?;
            let addr = u32::try_from(parse_num(addr)?)
                .map_err(|_| "create_hw_axi_txn: address must fit 32 bits".to_string())?;
            let kind = AxiTxnKind::parse(opt(args, "-type").unwrap_or("read"))?;
            let len = opt(args, "-len").map(parse_num).transpose()?.unwrap_or(1) as usize;
            let data = match opt(args, "-data") {
                Some(d) => parse_words(d)?,
                None => Vec::new(),
            };
            let txn = create_hw_axi_txn(name.as_str(), addr, kind, len, data)?;
            let d = &mut s.program.debug;
            d.axi_txns.retain(|t| t.name != txn.name);
            d.axi_txns.push(txn);
            Ok(name.to_string())
        }
        "run_hw_axi" => {
            let names: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
            if names.is_empty() {
                return Err("run_hw_axi: name the transaction(s)".into());
            }
            let mut out = Vec::new();
            for n in names {
                let txn = s
                    .program
                    .debug
                    .axi_txns
                    .iter()
                    .find(|t| &t.name == n)
                    .cloned()
                    .ok_or_else(|| format!("run_hw_axi: no transaction {n}"))?;
                run_axi(s, txn)?;
                wait_idle(s, std::time::Duration::from_secs(30));
                let row = s
                    .program
                    .debug
                    .axi_log
                    .first()
                    .ok_or("run_hw_axi: no result")?;
                match &row.resp {
                    Ok(r) => out.push(format!(
                        "{} {} {}",
                        row.name,
                        r.as_str(),
                        row.data
                            .iter()
                            .map(|w| format!("{w:08x}"))
                            .collect::<Vec<_>>()
                            .join(" ")
                    )),
                    Err(e) => return Err(format!("run_hw_axi {n}: {e}")),
                }
            }
            Ok(out.join("\n"))
        }
        "get_hw_axi_txns" => Ok(s
            .program
            .debug
            .axi_txns
            .iter()
            .map(|t| t.name.clone())
            .collect::<Vec<_>>()
            .join(" ")),
        other => Err(format!("{other}: not a hardware debug verb")),
    }
}

/// Tcl `set_property OUTPUT_VALUE v probe` (synchronous: waits for the commit).
pub fn tcl_output_value(s: &mut AppState, value: &str, probe: &str) -> Result<String, String> {
    let v = parse_num(value)?;
    let probe = probe
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split_whitespace()
        .last()
        .unwrap_or(probe)
        .trim_matches(|c| c == '{' || c == '}')
        .to_string();
    let msg = set_output(s, &probe, v)?;
    wait_idle(s, std::time::Duration::from_secs(30));
    match &s.program.debug.last_error {
        Some(e) => Err(e.clone()),
        None => Ok(msg),
    }
}


#[cfg(test)]
mod tests {
    use crate::state::{AppState, MsgSev};
    use crate::views::hw_debug::{FixtureStage, seed_debug_fixture};

    fn eval(s: &mut AppState, cmd: &str) -> String {
        let before = s.console.lines.len();
        crate::tcl::eval_command(s, cmd);
        let out: Vec<String> = s.console.lines[before..]
            .iter()
            .map(|l| l.text.clone())
            .collect();
        let errs: Vec<&String> = s.console.lines[before..]
            .iter()
            .filter(|l| l.sev == MsgSev::Error)
            .map(|l| &l.text)
            .collect();
        assert!(errs.is_empty(), "{cmd}: {errs:?}");
        out.join("\n")
    }

    #[test]
    fn vivado_verbs_drive_the_debug_target() {
        let mut s = AppState::demo();
        s.runs.checkpoint_root = Some(
            std::env::temp_dir().join(format!("helion-rune-hwtcl-{}", std::process::id())),
        );
        seed_debug_fixture(&mut s, FixtureStage::Picked).unwrap();
        let nets = s.program.debug.ila_nets.join(" ");
        eval(&mut s, &format!("implement_debug_core -probes {{{nets}}} -depth 16 -axi"));
        eval(&mut s, "wait_on_hw_ila");
        eval(&mut s, "refresh_hw_device");
        eval(&mut s, "wait_on_hw_ila");
        assert_eq!(eval(&mut s, "get_hw_ilas").lines().last(), Some("u_ila_0"));
        eval(&mut s, "run_hw_ila -trigger immediate -window 8");
        eval(&mut s, "wait_on_hw_ila");
        assert_eq!(s.program.debug.last_capture.as_ref().unwrap().words.len(), 8);
        let shown = eval(&mut s, "display_hw_ila_data");
        assert!(shown.contains("samples=8"), "{shown}");
        let out = s.program.debug.vio_edits.first().map(|(n, _)| n.clone());
        let probe = out.expect("VIO output probe");
        eval(&mut s, &format!("set_property OUTPUT_VALUE 1 {{{probe}}}"));
        assert!(s.program.debug.snap.as_ref().unwrap().vio.iter().any(|p| p.driving));
        let base = s.program.debug.snap.as_ref().unwrap().axi_regions[0].1;
        eval(
            &mut s,
            &format!("create_hw_axi_txn w {base:#x} -type write -len 2 -data {{0xA 0xB}}"),
        );
        eval(&mut s, &format!("create_hw_axi_txn r {base:#x} -type read -len 2"));
        let r = eval(&mut s, "run_hw_axi w r");
        assert!(r.contains("r OKAY 0000000a 0000000b"), "{r}");
        let ltx = std::env::temp_dir().join(format!("helion-rune-hwtcl-{}.ltx", std::process::id()));
        eval(&mut s, &format!("write_debug_probes {}", ltx.display()));
        assert!(ltx.is_file());
        let _ = std::fs::remove_file(ltx);
    }
}
