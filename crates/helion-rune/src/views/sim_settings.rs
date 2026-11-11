//! Simulation Settings: the UG900-style Property/Value knobs behind every
//! simulation, plus the Compile and Elaborate steps.
//!
//! The section mounts collapsible inside the Sim pane, beside the
//! testbenches ([`settings_section`]). Every row is real:
//!
//! - TIMESCALE_PS / RUNTIME_CYCLES / RUNTIME_PS edit
//!   [`crate::views::debug::DebugState`]'s `timescale_ps`/`runtime_cycles`,
//!   which drive the debugger's Run button and the free-running design run.
//! - SIM_TOP and PARAM feed `helion_sv::elaborate_sv` (Elaborate and the
//!   free-running design run); INCLUDE_PATH and DEFINE feed the sv-parser
//!   pass of Compile (`helion_sv::parse_sv_opts`).
//! - LOG_ALL_SIGNALS keeps every net in the free-run waveform (1) or only
//!   the design ports (0).
//! - TARGET_SIMULATOR and ENGINE_TIME_PS are read-only facts.
//!
//! Compile (`helion_sv::compile_sv`) and Elaborate (`elaborate_sv`, then a
//! `helion_sim::Sim` load) run on the shared simulation worker
//! ([`crate::views::tb::spawn_job`]) and report to the console and the sim
//! log; a successful Elaborate becomes the session design. Rows mirror the
//! old `sim_setting_rows` (`helion-gui/src/ide.rs`).

use std::collections::HashMap;

use rune::{El, ScrollSpec, Tx, col, el, row, text, text_input};
use rune_kit::glyphs;

use crate::state::{AppState, CenterPane, ConsoleLine, MsgSev, ToastKind};
use crate::theme::VivadoTheme;
use crate::views::tb::{RunTarget, SimEvent, spawn_job};

/// Row height (compact CAD density).
const ROW_H: f32 = 20.0;
/// Property-name column width.
const NAME_W: f32 = 128.0;

/// One settings row, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimKey {
    TargetSimulator,
    SimTop,
    IncludePath,
    Define,
    Param,
    TimescalePs,
    RuntimeCycles,
    RuntimePs,
    LogAllSignals,
    EngineTimePs,
}

impl SimKey {
    pub const ALL: [SimKey; 10] = [
        SimKey::TargetSimulator,
        SimKey::SimTop,
        SimKey::IncludePath,
        SimKey::Define,
        SimKey::Param,
        SimKey::TimescalePs,
        SimKey::RuntimeCycles,
        SimKey::RuntimePs,
        SimKey::LogAllSignals,
        SimKey::EngineTimePs,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SimKey::TargetSimulator => "TARGET_SIMULATOR",
            SimKey::SimTop => "SIM_TOP",
            SimKey::IncludePath => "INCLUDE_PATH",
            SimKey::Define => "DEFINE",
            SimKey::Param => "PARAM",
            SimKey::TimescalePs => "TIMESCALE_PS",
            SimKey::RuntimeCycles => "RUNTIME_CYCLES",
            SimKey::RuntimePs => "RUNTIME_PS",
            SimKey::LogAllSignals => "LOG_ALL_SIGNALS",
            SimKey::EngineTimePs => "ENGINE_TIME_PS",
        }
    }

    pub fn parse(name: &str) -> Option<SimKey> {
        let n = name.trim().to_ascii_uppercase();
        let n = if n == "TIMESCALE" {
            "TIMESCALE_PS".into()
        } else {
            n
        };
        SimKey::ALL.into_iter().find(|k| k.name() == n)
    }

    /// Read-only rows report engine facts and take no edits.
    pub fn editable(self) -> bool {
        !matches!(self, SimKey::TargetSimulator | SimKey::EngineTimePs)
    }

    /// Tooltip: what the knob drives.
    pub fn help(self) -> &'static str {
        match self {
            SimKey::TargetSimulator => "Read-only: Helion simulates on the helion-sim event kernel",
            SimKey::SimTop => "Top module to elaborate (empty = the file's top)",
            SimKey::IncludePath => "Comma-separated include directories for the Compile parse",
            SimKey::Define => "Comma-separated NAME[=VALUE] macros for the Compile parse",
            SimKey::Param => "Comma-separated NAME=VALUE top-level parameter overrides",
            SimKey::TimescalePs => "Clock period per cycle (ps; accepts 10ns, 2500ps)",
            SimKey::RuntimeCycles => "Cycles per Run (debugger and design runs)",
            SimKey::RuntimePs => "Run length in time; converts to RUNTIME_CYCLES",
            SimKey::LogAllSignals => "1 = every net in the design-run waveform, 0 = ports only",
            SimKey::EngineTimePs => "Read-only: the debugger engine's current time",
        }
    }

    fn slug(self) -> String {
        self.name().to_ascii_lowercase().replace('_', "-")
    }
}

/// Simulation-settings state (the debugger owns timescale/runtime).
#[derive(Clone, Debug)]
pub struct SimSettingsState {
    /// Section expanded in the Sim pane.
    pub open: bool,
    pub sim_top: Option<String>,
    pub include_path: String,
    pub define: String,
    pub param: String,
    pub log_all_signals: bool,
    /// Row being edited inline, with its edit buffer.
    pub editing: Option<SimKey>,
    pub edit: rune::TextEdit,
    /// Last rejected edit (shown under the rows until the next success).
    pub error: Option<String>,
    /// Modules found by the last Compile (`None` = not compiled).
    pub modules: Option<Vec<String>>,
    /// Report of the last Elaborate.
    pub elab: Option<helion_sv::SvElabReport>,
}

impl Default for SimSettingsState {
    fn default() -> Self {
        Self {
            open: true,
            sim_top: None,
            include_path: String::new(),
            define: String::new(),
            param: String::new(),
            log_all_signals: true,
            editing: None,
            edit: rune::TextEdit::new(""),
            error: None,
            modules: None,
            elab: None,
        }
    }
}

fn dash_or(v: &str) -> String {
    let t = v.trim();
    if t.is_empty() {
        "-".into()
    } else {
        t.to_string()
    }
}

/// Parse a time: `10ns`, `2us`, `2500ps`, or bare picoseconds.
pub fn parse_ps(raw: &str) -> Result<u64, String> {
    let v = raw.trim().trim_matches('"').to_ascii_lowercase();
    if v.is_empty() {
        return Err("missing value".into());
    }
    let (num, mul) = if let Some(n) = v.strip_suffix("ns") {
        (n.trim(), 1_000u64)
    } else if let Some(n) = v.strip_suffix("us") {
        (n.trim(), 1_000_000u64)
    } else if let Some(n) = v.strip_suffix("ms") {
        (n.trim(), 1_000_000_000u64)
    } else if let Some(n) = v.strip_suffix("ps") {
        (n.trim(), 1u64)
    } else {
        (v.as_str(), 1u64)
    };
    let n: u64 = num.parse().map_err(|_| format!("not a time: {raw}"))?;
    if n == 0 {
        return Err(format!("time must be > 0: {raw}"));
    }
    Ok(n.saturating_mul(mul))
}

fn parse_defines(text: &str) -> Vec<(String, String)> {
    text.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty() && *p != "-")
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
            None => (p.to_string(), String::new()),
        })
        .collect()
}

fn parse_params(text: &str) -> Result<HashMap<String, u128>, String> {
    let mut out = HashMap::new();
    for part in text
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty() && *p != "-")
    {
        let (k, v) = part
            .split_once('=')
            .ok_or_else(|| format!("PARAM: need NAME=VALUE, got {part}"))?;
        let name = k.trim();
        if name.is_empty() {
            return Err("PARAM: missing name".into());
        }
        let v = v.trim();
        let n = if let Some(h) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
            u128::from_str_radix(h, 16)
        } else {
            v.parse::<u128>()
        }
        .map_err(|_| format!("PARAM: not an integer: {v}"))?;
        out.insert(name.to_string(), n);
    }
    Ok(out)
}

/// PARAM overrides for elaboration.
pub fn params(s: &AppState) -> Result<HashMap<String, u128>, String> {
    parse_params(&s.sim_settings.param)
}

/// sv-parser options from INCLUDE_PATH / DEFINE.
pub fn compile_opts(s: &AppState) -> helion_sv::SvCompileOpts {
    helion_sv::SvCompileOpts {
        defines: parse_defines(&s.sim_settings.define),
        include_paths: s
            .sim_settings
            .include_path
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty() && p != "-")
            .collect(),
    }
}

/// Current value of `key`, as displayed.
pub fn value(s: &AppState, key: SimKey) -> String {
    let st = &s.sim_settings;
    match key {
        SimKey::TargetSimulator => "helion-sim".into(),
        SimKey::SimTop => match &st.sim_top {
            Some(t) => t.clone(),
            None => match &st.elab {
                Some(r) => format!("auto ({})", r.top),
                None => "auto".into(),
            },
        },
        SimKey::IncludePath => dash_or(&st.include_path),
        SimKey::Define => dash_or(&st.define),
        SimKey::Param => dash_or(&st.param),
        SimKey::TimescalePs => s.debug.timescale_ps.max(1).to_string(),
        SimKey::RuntimeCycles => s.debug.runtime_cycles.max(1).to_string(),
        SimKey::RuntimePs => {
            (u64::from(s.debug.runtime_cycles.max(1)) * s.debug.timescale_ps.max(1)).to_string()
        }
        SimKey::LogAllSignals => if st.log_all_signals { "1" } else { "0" }.into(),
        SimKey::EngineTimePs => s
            .debug
            .engine()
            .map(|e| e.time.to_string())
            .unwrap_or_else(|| "-".into()),
    }
}

/// Raw editable text of `key` (what the inline editor starts from).
fn edit_text(s: &AppState, key: SimKey) -> String {
    match key {
        SimKey::SimTop => s.sim_settings.sim_top.clone().unwrap_or_default(),
        SimKey::IncludePath => s.sim_settings.include_path.clone(),
        SimKey::Define => s.sim_settings.define.clone(),
        SimKey::Param => s.sim_settings.param.clone(),
        _ => value(s, key),
    }
}

/// Apply `val` to `key`. Validates before mutating; returns the Tcl echo.
pub fn set(s: &mut AppState, key: SimKey, val: &str) -> Result<String, String> {
    let v = val.trim().trim_matches('"');
    let name = key.name();
    match key {
        SimKey::TargetSimulator => {
            if v.eq_ignore_ascii_case("helion-sim") || v.eq_ignore_ascii_case("helion") {
                Ok(format!("set_property {name} helion-sim"))
            } else {
                Err(format!("{name}: only helion-sim is available"))
            }
        }
        SimKey::EngineTimePs => Err(format!("{name} is read-only")),
        SimKey::SimTop => {
            if v.is_empty() || v == "-" || v.eq_ignore_ascii_case("auto") {
                s.sim_settings.sim_top = None;
                return Ok(format!("set_property {name} auto"));
            }
            if let Some(mods) = &s.sim_settings.modules {
                if !mods.iter().any(|m| m == v) {
                    return Err(format!(
                        "{name}: no module {v} (compiled: {})",
                        if mods.is_empty() {
                            "-".into()
                        } else {
                            mods.join(", ")
                        }
                    ));
                }
            }
            s.sim_settings.sim_top = Some(v.to_string());
            Ok(format!("set_property {name} {v}"))
        }
        SimKey::IncludePath => {
            let low = v.to_ascii_lowercase();
            if low.contains("xvlog") || low.contains("xsim") {
                return Err(format!("{name}: helion-sv include paths only, not xvlog"));
            }
            s.sim_settings.include_path = if v == "-" {
                String::new()
            } else {
                v.to_string()
            };
            Ok(format!("set_property {name} {}", dash_or(v)))
        }
        SimKey::Define => {
            if parse_defines(v).iter().any(|(k, _)| k.is_empty()) {
                return Err(format!("{name}: need NAME[=VALUE], comma-separated"));
            }
            s.sim_settings.define = if v == "-" {
                String::new()
            } else {
                v.to_string()
            };
            Ok(format!("set_property {name} {}", dash_or(v)))
        }
        SimKey::Param => {
            parse_params(v)?;
            s.sim_settings.param = if v == "-" {
                String::new()
            } else {
                v.to_string()
            };
            Ok(format!("set_property {name} {}", dash_or(v)))
        }
        SimKey::TimescalePs => {
            let ps = parse_ps(v).map_err(|e| format!("{name}: {e}"))?;
            s.debug.timescale_ps = ps;
            Ok(format!("set_property {name} {ps}"))
        }
        SimKey::RuntimeCycles => {
            let n: u32 = v
                .parse()
                .map_err(|_| format!("{name}: not a cycle count: {val}"))?;
            if n == 0 {
                return Err(format!("{name} must be > 0"));
            }
            s.debug.runtime_cycles = n;
            Ok(format!("set_property {name} {n}"))
        }
        SimKey::RuntimePs => {
            let ps = parse_ps(v).map_err(|e| format!("{name}: {e}"))?;
            let ts = s.debug.timescale_ps.max(1);
            let n = ps.div_ceil(ts).clamp(1, u64::from(u32::MAX)) as u32;
            s.debug.runtime_cycles = n;
            Ok(format!(
                "set_property {name} {} RUNTIME_CYCLES={n}",
                u64::from(n) * ts
            ))
        }
        SimKey::LogAllSignals => {
            let on = match v.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                _ => return Err(format!("{name}: expected 0 or 1, got {val}")),
            };
            s.sim_settings.log_all_signals = on;
            Ok(format!("set_property {name} {}", u8::from(on)))
        }
    }
}

/// `report`-style dump of every row (Tcl `sim_settings`).
pub fn settings_text(s: &AppState) -> String {
    let mut out = format!(
        "simulation_settings target=helion-sim runtime_cycles={} timescale_ps={}",
        s.debug.runtime_cycles, s.debug.timescale_ps
    );
    for k in SimKey::ALL {
        out.push_str(&format!("\n{:<16} {}", k.name(), value(s, k)));
    }
    out
}

/// Tcl `set_timescale` / `set_runtime` / `sim_settings` / `compile_sim` /
/// `elaborate_sim`. The settings verbs open the Sim pane with the section
/// expanded so the change is visible.
pub fn exec_settings(s: &mut AppState, verb: &str, args: &[String]) -> Result<String, String> {
    let arg = args.join(" ");
    let show = |s: &mut AppState| {
        s.sim_settings.open = true;
        s.navigate(CenterPane::Sim);
    };
    match verb {
        "set_timescale" => {
            if arg.trim().is_empty() {
                return Err("set_timescale: need a time (e.g. 10ns)".into());
            }
            let r = set(s, SimKey::TimescalePs, &arg)?;
            show(s);
            Ok(r)
        }
        "set_runtime" => {
            let v = arg.trim();
            if v.is_empty() {
                return Err("set_runtime: need cycles or a time (e.g. 64 or 640ns)".into());
            }
            let key = if v.chars().any(|c| c.is_ascii_alphabetic()) {
                SimKey::RuntimePs
            } else {
                SimKey::RuntimeCycles
            };
            let r = set(s, key, v)?;
            show(s);
            Ok(r)
        }
        "sim_settings" => {
            show(s);
            let Some((key, rest)) = args.split_first() else {
                return Ok(settings_text(s));
            };
            let k = SimKey::parse(key).ok_or_else(|| format!("sim_settings: no setting {key}"))?;
            if rest.is_empty() {
                Ok(value(s, k))
            } else {
                set(s, k, &rest.join(" "))
            }
        }
        "compile_sim" => compile(s).map(|()| "compile_sim started".into()),
        "elaborate_sim" => elaborate(s).map(|()| "elaborate_sim started".into()),
        other => Err(format!("{other}: unknown simulation-settings verb")),
    }
}

/// Resolve the RTL source for Compile/Elaborate (run source or the staged
/// editor buffer), rejecting VHDL (helion-sv compiles SV/Verilog).
fn sv_source(s: &AppState, verb: &str) -> Result<(String, String), String> {
    let path = crate::runs::resolve_source(s).map_err(|e| e.replace("launch_runs", verb))?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(ext.as_str(), "vhd" | "vhdl") {
        return Err(format!(
            "{verb}: helion-sv compiles SystemVerilog/Verilog; {} is VHDL",
            path.display()
        ));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("{verb}: cannot read {}: {e}", path.display()))?;
    Ok((path.to_string_lossy().into_owned(), text))
}

/// Compile on the worker: helion-sv module parse, plus an sv-parser pass
/// with INCLUDE_PATH/DEFINE when either is set.
pub fn compile(s: &mut AppState) -> Result<(), String> {
    let (origin, src) = sv_source(s, "compile_sim")?;
    let opts = compile_opts(s);
    let short = short_name(&origin);
    spawn_job(
        s,
        RunTarget::Compile,
        format!("compile {short}"),
        None,
        move |tx, _| {
            let _ = tx.send(SimEvent::Line {
                text: format!("compile {origin} (helion-sv)"),
                sev: MsgSev::Info,
            });
            if !opts.defines.is_empty() || !opts.include_paths.is_empty() {
                let _ = tx.send(SimEvent::Line {
                    text: format!(
                        "compile: sv-parser with {} define(s), {} include path(s)",
                        opts.defines.len(),
                        opts.include_paths.len()
                    ),
                    sev: MsgSev::Info,
                });
                if let Err(e) = helion_sv::parse_sv_opts(&src, &origin, &opts) {
                    let _ = tx.send(SimEvent::Compiled(Err(format!("compile: {e}"))));
                    return;
                }
            }
            let r =
                helion_sv::compile_sv(&src, &origin, &opts).map_err(|e| format!("compile: {e}"));
            let _ = tx.send(SimEvent::Compiled(r));
        },
    )
}

/// Elaborate on the worker: helion-sv elaboration with SIM_TOP/PARAM, then
/// a `helion_sim::Sim` load proving the kernel accepts the netlist.
pub fn elaborate(s: &mut AppState) -> Result<(), String> {
    let (origin, src) = sv_source(s, "elaborate_sim")?;
    let opts = compile_opts(s);
    let params = params(s)?;
    let top = s.sim_settings.sim_top.clone();
    let short = short_name(&origin);
    spawn_job(
        s,
        RunTarget::Elaborate,
        format!("elaborate {short}"),
        None,
        move |tx, _| {
            let _ = tx.send(SimEvent::Line {
                text: format!(
                    "elaborate {origin} top={} (helion-sv)",
                    top.as_deref().unwrap_or("auto")
                ),
                sev: MsgSev::Info,
            });
            let r = helion_sv::elaborate_sv(&src, &origin, top.as_deref(), &params, &opts)
                .map_err(|e| format!("elaborate: {e}"));
            if let Ok((design, _)) = &r {
                let sim = helion_sim::Sim::new(design);
                let _ = tx.send(SimEvent::Line {
                    text: format!(
                        "helion-sim load: {} input(s), {} output(s)",
                        sim.input_ports().len(),
                        sim.output_ports().len()
                    ),
                    sev: MsgSev::Info,
                });
            }
            let _ = tx.send(SimEvent::Elaborated(r));
        },
    )
}

fn short_name(origin: &str) -> String {
    std::path::Path::new(origin)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| origin.to_string())
}

fn report_line(s: &mut AppState, text: String, sev: MsgSev) {
    s.console.lines.push(ConsoleLine {
        text: text.clone(),
        sev,
    });
    s.sim.log.push(ConsoleLine { text, sev });
}

/// Apply a finished Compile.
pub fn finish_compile(s: &mut AppState, r: Result<Vec<String>, String>) {
    match r {
        Ok(mods) => {
            let joined = if mods.is_empty() {
                "-".into()
            } else {
                mods.join(",")
            };
            report_line(
                s,
                format!("compile n={} MODULES={joined} engine=helion-sv", mods.len()),
                MsgSev::Info,
            );
            s.toasts.push(
                ToastKind::Success,
                "Compile complete",
                &format!("{} module(s): {joined}", mods.len()),
            );
            s.sim_settings.modules = Some(mods);
        }
        Err(e) => {
            report_line(s, format!("ERROR: {e}"), MsgSev::Error);
            s.toasts.push(ToastKind::Error, "Compile failed", &e);
        }
    }
}

/// Apply a finished Elaborate: keep the report and make the design the
/// session design (the ILA, design runs, and Tcl queries see it).
pub fn finish_elaborate(
    s: &mut AppState,
    r: Result<(helion_ir::Design, helion_sv::SvElabReport), String>,
) {
    match r {
        Ok((design, report)) => {
            let line = format!(
                "elaborate TOP={} CELLS={} LUTS={} FFS={} engine=helion-sv",
                report.top, report.cells, report.luts, report.ffs
            );
            report_line(s, line, MsgSev::Info);
            s.toasts.push(
                ToastKind::Success,
                "Elaboration complete",
                &format!("{}: {} cells, {} FFs", report.top, report.cells, report.ffs),
            );
            crate::tcl::store_design(s, design);
            s.sim_settings.elab = Some(report);
        }
        Err(e) => {
            report_line(s, format!("ERROR: {e}"), MsgSev::Error);
            s.toasts.push(ToastKind::Error, "Elaboration failed", &e);
        }
    }
}

/// Start (or switch) inline editing of `key`.
fn begin_edit(s: &mut AppState, key: SimKey) {
    if key == SimKey::LogAllSignals {
        let next = if s.sim_settings.log_all_signals {
            "0"
        } else {
            "1"
        };
        commit(s, key, next);
        return;
    }
    if !key.editable() {
        return;
    }
    s.sim_settings.editing = Some(key);
    s.sim_settings.edit = rune::TextEdit::new(edit_text(s, key));
}

/// Commit `val` to `key`, echoing the verb (errors stay inline).
fn commit(s: &mut AppState, key: SimKey, val: &str) {
    match set(s, key, val) {
        Ok(echo) => {
            s.sim_settings.error = None;
            s.sim_settings.editing = None;
            s.console.lines.push(ConsoleLine {
                text: echo,
                sev: MsgSev::Info,
            });
        }
        Err(e) => s.sim_settings.error = Some(e),
    }
}

fn submit_edit(s: &mut AppState, _tx: &mut Tx) {
    if let Some(key) = s.sim_settings.editing {
        let val = s.sim_settings.edit.text.clone();
        commit(s, key, &val);
    }
}

/// One Property/Value row: click to edit (LOG_ALL_SIGNALS toggles).
fn setting_row(s: &AppState, t: &VivadoTheme, key: SimKey) -> El {
    let editing = s.sim_settings.editing == Some(key);
    let val = value(s, key);
    let name_cell = el().w(NAME_W).child(
        text(key.name())
            .font(VivadoTheme::FONT_UI)
            .color(t.text_dim)
            .truncate()
            .help_when_truncated(key.name()),
    );
    let value_cell: El = if editing {
        text_input(&s.sim_settings.edit, VivadoTheme::FONT_MONO.mono())
            .single_line()
            .key(format!("sim-setting-input-{}", key.slug()))
            .label(format!("{} value", key.name()))
            .grow(1.0)
            .min_w(0.0)
            .h(18.0)
            .px(4.0)
            .fill(t.chrome_bg)
            .border(1.0, t.link)
            .radius(2.0)
            .color(t.text)
            .caret(t.text, t.selection)
            .on_edit(|s: &mut AppState| &mut s.sim_settings.edit)
            .on_submit(submit_edit)
    } else {
        el().grow(1.0).min_w(0.0).child(
            text(val.clone())
                .font(VivadoTheme::FONT_MONO)
                .color(if key.editable() { t.text } else { t.text_faint })
                .truncate()
                .help_when_truncated(val.clone()),
        )
    };
    let mut r = row()
        .key(format!("sim-setting-{}", key.slug()))
        .w_full()
        .h(ROW_H)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .fill(if editing { t.selection } else { t.panel_bg })
        .child(name_cell)
        .child(value_cell);
    if !editing {
        r = r
            .role("button")
            .label(format!("{} = {val}", key.name()))
            .help(key.help());
        if key.editable() {
            r = r
                .focusable(true)
                .hover(t.selection)
                .on_press(move |s: &mut AppState| begin_edit(s, key));
        } else {
            r = r.description(key.help());
        }
    }
    r
}

/// Collapsible Simulation Settings section (header + Property/Value rows).
pub fn settings_section(s: &AppState, t: &VivadoTheme) -> El {
    let open = s.sim_settings.open;
    let header = row()
        .key("sim-settings-head")
        .role("button")
        .label(if open {
            "Collapse Simulation Settings"
        } else {
            "Expand Simulation Settings"
        })
        .focusable(true)
        .w_full()
        .h(ROW_H)
        .px(6.0)
        .gap(4.0)
        .items_center()
        .fill(t.header_bg)
        .hover(t.selection)
        .on_press(|s: &mut AppState| s.sim_settings.open = !s.sim_settings.open)
        .child(
            rune::icon(
                if open {
                    &glyphs::TRIANGLE_DOWN
                } else {
                    &glyphs::TRIANGLE_RIGHT
                },
                9.0,
            )
            .color(t.text_dim)
            .a11y_hidden(),
        )
        .child(text("Settings").font(VivadoTheme::FONT_TITLE).color(t.text))
        .child(el().grow(1.0))
        .child(
            text(format!(
                "{} cyc · {} ps",
                s.debug.runtime_cycles, s.debug.timescale_ps
            ))
            .font(VivadoTheme::FONT_UI)
            .color(t.text_faint),
        );
    let mut body = col()
        .key("sim-settings")
        .w_full()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(header);
    if !open {
        return body;
    }
    let mut rows = col()
        .key("sim-settings-rows")
        .w_full()
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(
            row()
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .gap(6.0)
                .items_center()
                .child(
                    el().w(NAME_W).child(
                        text("Property")
                            .font(VivadoTheme::FONT_CAPS)
                            .color(t.text_faint),
                    ),
                )
                .child(
                    text("Value")
                        .font(VivadoTheme::FONT_CAPS)
                        .color(t.text_faint),
                ),
        );
    for key in SimKey::ALL {
        rows = rows.child(setting_row(s, t, key));
    }
    body = body.child(rows);
    let footer = match (
        &s.sim_settings.error,
        &s.sim_settings.elab,
        &s.sim_settings.modules,
    ) {
        (Some(e), _, _) => Some((e.clone(), t.fail)),
        (None, Some(r), _) => Some((
            format!("Elaborated {} · {} cells · {} FFs", r.top, r.cells, r.ffs),
            t.text_dim,
        )),
        (None, None, Some(m)) => Some((format!("Compiled {} module(s)", m.len()), t.text_dim)),
        _ if s.sim_settings.editing.is_some() => {
            Some(("Enter applies the new value".to_string(), t.text_faint))
        }
        _ => None,
    };
    if let Some((msg, ink)) = footer {
        body = body.child(
            row()
                .key("sim-settings-note")
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .items_center()
                .child(
                    text(msg.clone())
                        .font(VivadoTheme::FONT_UI)
                        .color(ink)
                        .truncate()
                        .help_when_truncated(msg),
                ),
        );
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init;

    fn pump(s: &mut AppState) {
        let start = std::time::Instant::now();
        while s.sim.is_active() {
            crate::views::tb::poll_sim(s);
            assert!(start.elapsed() < std::time::Duration::from_secs(120));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn times_parse_units_and_reject_zero() {
        assert_eq!(parse_ps("10ns"), Ok(10_000));
        assert_eq!(parse_ps("2us"), Ok(2_000_000));
        assert_eq!(parse_ps("2500"), Ok(2500));
        assert!(parse_ps("0ns").is_err());
        assert!(parse_ps("fast").is_err());
    }

    #[test]
    fn runtime_and_timescale_drive_the_debugger_knobs() {
        let mut s = init();
        exec_settings(&mut s, "set_timescale", &["5ns".into()]).unwrap();
        assert_eq!(s.debug.timescale_ps, 5000);
        exec_settings(&mut s, "set_runtime", &["64".into()]).unwrap();
        assert_eq!(s.debug.runtime_cycles, 64);
        let r = exec_settings(&mut s, "set_runtime", &["101ns".into()]).unwrap();
        assert_eq!(s.debug.runtime_cycles, 21, "101ns / 5ns rounds up: {r}");
        assert_eq!(
            s.center,
            CenterPane::Sim,
            "settings verbs open the Sim pane"
        );
        assert!(set(&mut s, SimKey::RuntimeCycles, "0").is_err());
        assert_eq!(s.debug.runtime_cycles, 21, "rejected edits never mutate");
    }

    #[test]
    fn rows_validate_and_dump_every_property() {
        let mut s = init();
        assert!(set(&mut s, SimKey::Param, "WIDTH").is_err());
        set(&mut s, SimKey::Param, "WIDTH=8, DEPTH=0x10").unwrap();
        assert_eq!(params(&s).unwrap().get("DEPTH"), Some(&16));
        assert!(set(&mut s, SimKey::IncludePath, "/opt/xvlog/inc").is_err());
        set(&mut s, SimKey::Define, "SIM, WIDTH=4").unwrap();
        assert_eq!(compile_opts(&s).defines.len(), 2);
        set(&mut s, SimKey::LogAllSignals, "0").unwrap();
        assert!(!s.sim_settings.log_all_signals);
        assert!(set(&mut s, SimKey::EngineTimePs, "5").is_err());
        let dump = settings_text(&s);
        for k in SimKey::ALL {
            assert!(dump.contains(k.name()), "dump lists {}", k.name());
        }
        let out = exec_settings(
            &mut s,
            "sim_settings",
            &["runtime_cycles".into(), "32".into()],
        );
        assert_eq!(out.unwrap(), "set_property RUNTIME_CYCLES 32");
    }

    #[test]
    fn compile_and_elaborate_run_the_real_engines() {
        let mut s = init();
        s.runs.source = Some(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv"),
        );
        exec_settings(&mut s, "compile_sim", &[]).unwrap();
        assert_eq!(s.sim.active_target(), Some(RunTarget::Compile));
        pump(&mut s);
        assert_eq!(
            s.sim_settings.modules.as_deref(),
            Some(&["counter".to_string()][..])
        );
        assert!(
            set(&mut s, SimKey::SimTop, "nope").is_err(),
            "SIM_TOP checks compiled modules"
        );
        set(&mut s, SimKey::SimTop, "counter").unwrap();
        exec_settings(&mut s, "elaborate_sim", &[]).unwrap();
        pump(&mut s);
        let rep = s.sim_settings.elab.as_ref().expect("elab report");
        assert_eq!(rep.top, "counter");
        assert!(rep.ffs >= 4, "4-bit counter has flops: {rep:?}");
        assert!(
            s.tcl.design.is_some(),
            "elaborate becomes the session design"
        );
        assert!(
            s.sim
                .log
                .iter()
                .any(|l| l.text.starts_with("helion-sim load:"))
        );
    }
}
