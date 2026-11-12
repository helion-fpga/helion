//! Out-of-context (OOC) module runs inside Design Runs.
//!
//! Engine: `helion_proj::ooc` — `synth_module_ooc` synthesizes a module on
//! its own and caches the netlist as a content-addressed `.hck2`
//! checkpoint; `synth_top_with_ooc` black-boxes those modules in the top
//! synthesis and stitches the cached netlists back in. A module marked OOC
//! here gets its own `<module>_synth_1` run row; `synth_1` / `impl_1` then
//! reuse its checkpoint (cache hit) until its sources change.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use rune::{El, col, row};

use crate::state::{AppState, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::cad;
use crate::views::report_kit::{self as kit, header_row, num_cell, plain_row, table};

const SCAN: &str = "ooc_scan";
const RUN: &str = "ooc_run";

/// One OOC module run result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OocRow {
    pub module: String,
    pub cache_hit: bool,
    pub cells: usize,
    pub luts: usize,
    pub ffs: usize,
    pub checkpoint: PathBuf,
}

impl OocRow {
    fn from_run(r: &helion_proj::ooc::OocModuleRun) -> Self {
        Self {
            module: r.module.clone(),
            cache_hit: r.cache_hit,
            cells: r.cells,
            luts: r.luts,
            ffs: r.ffs,
            checkpoint: r.checkpoint.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct OocState {
    /// Modules defined by the run sources (from the last scan).
    pub modules: Vec<String>,
    /// Modules synthesized out of context.
    pub enabled: BTreeSet<String>,
    pub runs: BTreeMap<String, Result<OocRow, String>>,
    pub scan_error: Option<String>,
}

/// OOC checkpoint cache directory.
pub fn cache_dir(s: &AppState) -> PathBuf {
    crate::bg::output_dir(s).join("ooc_cache")
}

/// Worker body used by the run: synthesize/restore each OOC module, then
/// the top with the modules stitched in.
pub fn synth_with_ooc(
    source: &std::path::Path,
    part: &str,
    modules: &[String],
    cache: &std::path::Path,
    send: &impl Fn(String, MsgSev),
) -> Result<(helion_ir::Design, Vec<OocRow>), String> {
    let files = crate::runs::rtl_files(source)?;
    let dev = crate::runs::resolve_device(part, send)?;
    let mut runs = Vec::new();
    for m in modules {
        let r = helion_proj::ooc::synth_module_ooc(&files, m, cache, &dev)?;
        send(
            format!(
                "synth_design -mode out_of_context -top {m} cells={} {}",
                r.cells,
                if r.cache_hit { "(restored from cache)" } else { "(synthesized)" }
            ),
            MsgSev::Info,
        );
        runs.push(r);
    }
    let (design, log) = helion_proj::ooc::synth_top_with_ooc(&files, &runs)?;
    for l in log {
        send(l, MsgSev::Info);
    }
    Ok((design, runs.iter().map(OocRow::from_run).collect()))
}

fn log(s: &mut AppState, text: String, sev: MsgSev) {
    s.console.lines.push(ConsoleLine { text, sev });
}

/// List the modules of the run sources on a worker.
pub fn scan(s: &mut AppState) -> Result<String, String> {
    let files = crate::runs::resolve_source(s).and_then(|p| crate::runs::rtl_files(&p))?;
    crate::bg::spawn(
        s,
        SCAN,
        move || -> Result<Vec<String>, String> {
            let mut out = Vec::new();
            for f in &files {
                for m in helion_sv::list_sv_modules_path(f)? {
                    if !out.contains(&m) {
                        out.push(m);
                    }
                }
            }
            Ok(out)
        },
        |s, r| match r {
            Ok(mods) => {
                // The top (first module that no other module instantiates
                // is unknown here) stays selectable; a module that only
                // exists as top simply has no instance to stitch.
                s.ooc.enabled.retain(|m| mods.contains(m));
                s.ooc.modules = mods;
                s.ooc.scan_error = None;
            }
            Err(e) => s.ooc.scan_error = Some(e),
        },
    );
    Ok("ooc scan running".into())
}

/// Mark `module` OOC (or not).
pub fn set_ooc(s: &mut AppState, module: &str, on: bool) -> Result<String, String> {
    if !s.ooc.modules.is_empty() && !s.ooc.modules.iter().any(|m| m == module) {
        return Err(format!("set_ooc: no module {module} in the sources"));
    }
    if on {
        s.ooc.enabled.insert(module.to_string());
    } else {
        s.ooc.enabled.remove(module);
    }
    Ok(format!("set_property OOC {} [get_modules {module}]", if on { 1 } else { 0 }))
}

/// Run `<module>_synth_1` on a worker (cache-aware).
pub fn run_module(s: &mut AppState, module: &str) -> Result<String, String> {
    if crate::bg::busy(s, RUN) {
        return Err("an OOC run is in progress".into());
    }
    let files = crate::runs::resolve_source(s).and_then(|p| crate::runs::rtl_files(&p))?;
    let part = if s.project.part.is_empty() { "HL10T-C32-1".to_string() } else { s.project.part.clone() };
    let cache = cache_dir(s);
    let m = module.to_string();
    crate::bg::spawn(
        s,
        RUN,
        move || {
            let r = crate::runs::resolve_device(&part, &|_, _| {})
                .and_then(|dev| helion_proj::ooc::synth_module_ooc(&files, &m, &cache, &dev))
                .map(|r| OocRow::from_run(&r));
            (m, r)
        },
        |s, (m, r): (String, Result<OocRow, String>)| {
            match &r {
                Ok(row) => log(
                    s,
                    format!(
                        "{m}_synth_1 cells={} luts={} ffs={} {} {}",
                        row.cells,
                        row.luts,
                        row.ffs,
                        if row.cache_hit { "cache_hit" } else { "synthesized" },
                        row.checkpoint.display()
                    ),
                    MsgSev::Info,
                ),
                Err(e) => log(s, format!("ERROR: {m}_synth_1: {e}"), MsgSev::Error),
            }
            s.ooc.runs.insert(m, r);
        },
    );
    Ok(format!("{module}_synth_1 running"))
}

fn chip(key: String, label: &'static str, on: bool, t: &VivadoTheme, f: impl Fn(&mut AppState) + 'static) -> El {
    rune::el()
        .key(key)
        .role("checkbox")
        .label(label)
        .focusable(true)
        .h(18.0)
        .px(6.0)
        .center_y()
        .radius(3.0)
        .fill(if on { t.selection } else { t.panel_bg })
        .border(1.0, if on { t.accent } else { t.border })
        .hover(t.selection)
        .on_press(f)
        .child(rune::text(if on { "OOC" } else { "Global" }).font(VivadoTheme::FONT_UI).color(if on { t.text } else { t.text_dim }))
}

/// Design Runs section: modules, OOC flag, per-module run row.
pub fn ooc_section(s: &AppState, t: &VivadoTheme) -> El {
    let scanning = crate::bg::busy(s, SCAN);
    let src_missing = s.runs.source.is_none() && s.editor.tabs.is_empty();
    let scan_btn = kit::action_button(
        "runs-ooc-scan",
        if s.ooc.modules.is_empty() { "Find Modules" } else { "Rescan" },
        if scanning { Some("Scanning…") } else if src_missing { Some("Open a project or RTL source first") } else { None },
        t,
        |s: &mut AppState| {
            if let Err(e) = scan(s) {
                s.ooc.scan_error = Some(e);
            }
        },
    );
    let mut c = col().key("runs-ooc").w_full().child(
        row()
            .w_full()
            .h(28.0)
            .px(8.0)
            .pt(8.0)
            .items_center()
            .gap(8.0)
            .child(cad::title("Out-of-Context Modules", t))
            .child(rune::el().grow(1.0))
            .child(scan_btn),
    );
    if let Some(e) = &s.ooc.scan_error {
        return c.child(kit::empty_state("runs-ooc-error", e, None, t));
    }
    if s.ooc.modules.is_empty() {
        return c.child(kit::empty_state(
            "runs-ooc-empty",
            "Find the source modules to synthesize any of them out of context.",
            None,
            t,
        ));
    }
    let head = header_row(t)
        .child(rune::el().w(140.0).child(kit::header_cell("Module", t)))
        .child(rune::el().w(64.0).child(kit::header_cell("Mode", t)))
        .child(rune::el().w(210.0).child(kit::header_cell("Run", t)))
        .child(kit::header_num("Cells", 56.0, t))
        .child(kit::header_num("LUT", 48.0, t))
        .child(kit::header_num("FF", 48.0, t))
        .child(kit::header_cell("Checkpoint", t).grow(1.0).min_w(0.0))
        .child(rune::el().w(64.0));
    let mut body = col().w_full();
    let busy = crate::bg::busy(s, RUN) || s.runs.is_active();
    for (i, m) in s.ooc.modules.iter().enumerate() {
        let on = s.ooc.enabled.contains(m);
        let name = m.clone();
        let mut r = plain_row(format!("runs-ooc-row-{i}"))
            .child(kit::fixed_cell(m.clone(), 140.0, t.text))
            .child(rune::el().w(64.0).child(chip(format!("runs-ooc-toggle-{i}"), "Out of context", on, t, move |s: &mut AppState| {
                let next = !s.ooc.enabled.contains(&name);
                let _ = set_ooc(s, &name, next);
            })));
        match s.ooc.runs.get(m) {
            Some(Ok(row)) => {
                r = r
                    .child(kit::fixed_cell(format!("{m}_synth_1 · {}", if row.cache_hit { "cached" } else { "synthesized" }), 210.0, t.pass))
                    .child(num_cell(row.cells.to_string(), 56.0, t.text))
                    .child(num_cell(row.luts.to_string(), 48.0, t.text))
                    .child(num_cell(row.ffs.to_string(), 48.0, t.text))
                    .child(kit::text_cell(row.checkpoint.display().to_string(), 1.0, t.text_dim));
            }
            Some(Err(e)) => {
                r = r
                    .child(kit::fixed_cell(format!("{m}_synth_1 · failed"), 210.0, t.fail))
                    .child(kit::text_cell(e.clone(), 1.0, t.fail));
            }
            None => {
                r = r
                    .child(kit::fixed_cell(if on { "Not run" } else { "—" }, 210.0, t.text_faint))
                    .child(rune::el().grow(1.0));
            }
        }
        let mm = m.clone();
        let reason = if !on { Some("Mark the module OOC first") } else if busy { Some("A run is in progress") } else { None };
        r = r.child(rune::el().w(64.0).child(kit::action_button(format!("runs-ooc-run-{i}"), "Run", reason, t, move |s: &mut AppState| {
            if let Err(e) = run_module(s, &mm) {
                log(s, format!("ERROR: {e}"), MsgSev::Error);
            }
        })));
        body = body.child(r);
    }
    c = c.child(table(head, body, t));
    if !s.ooc.enabled.is_empty() {
        c = c.child(
            row().w_full().px(8.0).h(22.0).items_center().child(cad::dim(
                format!(
                    "synth_1 / impl_1 black-box {} and stitch the cached netlists.",
                    s.ooc.enabled.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
                t,
            ).truncate()),
        );
    }
    c
}

pub const VERBS: &[&str] = &["set_ooc", "synth_ooc", "report_ooc"];

pub fn help(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "set_ooc" => ("set_ooc module ?0|1?", "synthesize a module out of context (cached) in synth/impl runs"),
        "synth_ooc" => ("synth_ooc module", "run <module>_synth_1 out of context (restores the cache when unchanged)"),
        "report_ooc" => ("report_ooc", "list OOC modules and their cached runs"),
        _ => return None,
    })
}

pub fn tcl_verb(s: &mut AppState, name: &str, args: &[String]) -> Result<String, String> {
    match name {
        "set_ooc" => {
            let m = args.first().ok_or("set_ooc: need a module")?;
            let on = args.get(1).is_none_or(|v| v != "0" && !v.eq_ignore_ascii_case("false"));
            set_ooc(s, m, on)
        }
        "synth_ooc" => {
            let m = args.first().ok_or("synth_ooc: need a module")?.clone();
            s.ooc.enabled.insert(m.clone());
            run_module(s, &m)
        }
        "report_ooc" => Ok(s
            .ooc
            .enabled
            .iter()
            .map(|m| match s.ooc.runs.get(m) {
                Some(Ok(r)) => format!("{m} cells={} cache_hit={} {}", r.cells, r.cache_hit, r.checkpoint.display()),
                Some(Err(e)) => format!("{m} failed: {e}"),
                None => format!("{m} not run"),
            })
            .collect::<Vec<_>>()
            .join("\n")),
        _ => Err(format!("{name}: not an OOC verb")),
    }
}

/// Scene fixture: two modules, one OOC with a real cached run.
pub fn scene_fill(s: &mut AppState) {
    let dir = std::env::temp_dir().join("helion-rune-scenes").join("ooc");
    let _ = std::fs::create_dir_all(&dir);
    let top = dir.join("top.v");
    let _ = std::fs::write(&top, "module top(input clk, input [1:0] sel, output [3:0] led);\n  wire [3:0] q;\n  child u_c (.clk(clk), .sel(sel), .q(q));\n  assign led = q ^ 4'b0101;\nendmodule\nmodule child(input clk, input [1:0] sel, output reg [3:0] q);\n  always @(posedge clk) q <= q + {2'b0, sel};\nendmodule\n");
    s.ooc.modules = vec!["top".into(), "child".into()];
    s.ooc.enabled.insert("child".into());
    if let Ok(dev) = helion_device::Device::load_part("HL10T-C32-1") {
        let r = helion_proj::ooc::synth_module_ooc(&[top], "child", &dir.join("cache"), &dev).map(|r| OocRow::from_run(&r));
        s.ooc.runs.insert("child".into(), r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::{RunKind, start_run};

    #[test]
    fn ooc_module_is_cached_and_reused_by_the_top_run() {
        let dir = std::env::temp_dir().join(format!("helion-rune-ooc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let top = dir.join("top.v");
        std::fs::write(&top, "module top(input clk, input [1:0] sel, output [3:0] led);\n  wire [3:0] q;\n  child u_c (.clk(clk), .sel(sel), .q(q));\n  assign led = q ^ 4'b0101;\nendmodule\nmodule child(input clk, input [1:0] sel, output reg [3:0] q);\n  always @(posedge clk) q <= q + {2'b0, sel};\nendmodule\n").unwrap();
        let mut s = AppState::demo();
        s.runs.source = Some(top.clone());
        scan(&mut s).unwrap();
        crate::bg::wait_all(&mut s);
        assert_eq!(s.ooc.modules, vec!["top".to_string(), "child".to_string()]);
        tcl_verb(&mut s, "synth_ooc", &["child".into()]).unwrap();
        crate::bg::wait_all(&mut s);
        let first = s.ooc.runs["child"].clone().unwrap();
        assert!(!first.cache_hit && first.ffs == 4);
        assert!(first.checkpoint.starts_with(std::env::temp_dir()));
        start_run(&mut s, RunKind::Synth, top, "HL10T-C32-1".into()).unwrap();
        let t0 = std::time::Instant::now();
        while s.runs.is_active() && t0.elapsed() < std::time::Duration::from_secs(120) {
            crate::runs::poll_runs(&mut s);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        crate::runs::poll_runs(&mut s);
        assert!(s.runs.synth_log.iter().any(|l| l.text.contains("out_of_context -top child") && l.text.contains("restored from cache")),
            "{:?}", s.runs.synth_log.iter().map(|l| &l.text).collect::<Vec<_>>());
        assert!(s.runs.synth_log.iter().any(|l| l.text.starts_with("read_checkpoint -cell u_c")));
        assert!(s.ooc.runs["child"].as_ref().unwrap().cache_hit);
        let d = s.runs.live_design.as_ref().unwrap();
        assert!(d.cells.iter().any(|c| helion_ir::resolve(c.name).starts_with("u_c_")));
    }
}
