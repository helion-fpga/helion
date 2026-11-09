//! Interactive simulation debugger: persistent event-sim session plus UG900
//! signal breakpoints.
//!
//! This is the first debugger slice. It ports the old interactive `sim_run` /
//! `sim_step` / `sim_restart` behavior and signal breakpoints, not the batch
//! VCD testbench runner (`views/wave.rs`) and not Memory/Locals/Forces (later
//! passes). The engine stays in UI state because stepping and breakpoints are
//! interactive; runs are bounded synchronous evaluations over real `Sim`.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el/scroll.rs`: `.scroll()` vertical viewport.
//! - `crates/rune/src/edit.rs`: `TextEdit::new`, its `text` field for reads.

use std::collections::HashMap;

use helion_sim::Sim;
use rune::{El, Tx, col, el, row, text, text_input};

use crate::live::live_design;
use crate::state::{AppState, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::sources::pane_header;

/// Default interactive run length (`sim_runtime_cycles`).
pub const DEFAULT_RUN_CYCLES: u32 = 16;
/// Default interactive timescale (`sim_timescale_ps`).
pub const DEFAULT_TIMESCALE_PS: u64 = 10_000;
/// Breakpoint row height (compact CAD density).
const ROW_H: f32 = 22.0;

/// One UG900 breakpoint row (`BreakpointRow`, signal breakpoints first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Breakpoint {
    pub id: usize,
    pub enabled: bool,
    pub signal: String,
    /// `change` or `==N`.
    pub condition: String,
    pub value: Option<u64>,
    pub hits: u32,
    /// `signal` now; line breakpoints arrive with the Source gutter pass.
    pub kind: String,
    /// RTL file for a line breakpoint.
    pub file: Option<String>,
    /// 1-based RTL line for a Source-window breakpoint.
    pub line: Option<usize>,
}

impl Breakpoint {
    pub fn enabled_cell(&self) -> &'static str {
        if self.enabled { "1" } else { "0" }
    }

    pub fn kind_cell(&self) -> &str {
        if self.kind.is_empty() {
            "signal"
        } else {
            self.kind.as_str()
        }
    }

    pub fn line_cell(&self) -> String {
        self.line
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".into())
    }

    pub fn signal_cell(&self) -> &str {
        if self.signal.is_empty() {
            "-"
        } else {
            self.signal.as_str()
        }
    }

    /// Clickable-grid dump row: UG900 Id/En/Signal/Cond/Hits/KIND/LINE.
    pub fn row_text(&self) -> String {
        format!(
            "ID={} ENABLED={} SIGNAL={} CONDITION={} HITS={} KIND={} LINE={}",
            self.id,
            self.enabled_cell(),
            self.signal_cell(),
            self.condition,
            self.hits,
            self.kind_cell(),
            self.line_cell()
        )
    }
}

/// Debugger-owned UI state: the persistent event engine, signal breakpoints,
/// spec/cycles editors, and breakpoint change tracking.
#[derive(Clone, Debug)]
pub struct DebugState {
    pub spec: rune::TextEdit,
    pub breakpoints: Vec<Breakpoint>,
    pub selected: Option<usize>,
    pub runtime_cycles: u32,
    pub timescale_ps: u64,
    next_id: usize,
    prev: HashMap<String, u64>,
    engine: Option<Sim>,
    design_key: Option<String>,
}

impl Default for DebugState {
    fn default() -> Self {
        Self {
            spec: rune::TextEdit::new(""),
            breakpoints: Vec::new(),
            selected: None,
            runtime_cycles: DEFAULT_RUN_CYCLES,
            timescale_ps: DEFAULT_TIMESCALE_PS,
            next_id: 1,
            prev: HashMap::new(),
            engine: None,
            design_key: None,
        }
    }
}

impl DebugState {
    pub fn engine(&self) -> Option<&Sim> {
        self.engine.as_ref()
    }

    /// Debugger status: engine progress when present, else honest session state.
    pub fn status(&self, s: &AppState) -> String {
        if let Some(sim) = self.engine.as_ref() {
            format!(
                "Event kernel · cycle {} · time {} ps · led={} · {} breakpoints",
                sim.cycle,
                sim.time,
                u8::from(sim.led),
                self.breakpoints.len()
            )
        } else if live_design(s).is_some() {
            format!(
                "Event kernel · idle · {} breakpoints",
                self.breakpoints.len()
            )
        } else {
            "No elaborated design — elaborate or run synthesis first".into()
        }
    }
}

/// Exact session identity: a changed netlist must not reuse a stale engine.
fn design_key(design: &helion_ir::Design) -> String {
    design.to_hnf()
}

/// Debugger output goes to both the Tcl console and the sim log, like batch
/// sim worker lines (`deliver_sim`).
fn debug_log(s: &mut AppState, text: &str, sev: MsgSev) {
    s.console.lines.push(ConsoleLine {
        text: text.to_string(),
        sev,
    });
    s.sim.log.push(ConsoleLine {
        text: text.to_string(),
        sev,
    });
}

/// Elaborate summary for a fresh engine (`prepare_sim` text).
fn elaborate_text(design: &helion_ir::Design, sim: &Sim) -> String {
    let name = helion_ir::resolve(design.name);
    let luts = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, helion_ir::CellKind::Lut6 { .. }))
        .count();
    let ffs = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, helion_ir::CellKind::Hff))
        .count();
    format!(
        "elaborated {name} cells={} luts={luts} ffs={ffs} kernel=event led={} time_ps={}",
        design.cells.len(),
        u8::from(sim.led),
        sim.time
    )
}

/// Ensure the existing event engine for stepping/breakpoints. Like old
/// `prepare_sim`, but persistent across steps (old `sim_run` gets a fresh
/// engine through [`fresh_engine`]).
pub fn ensure_engine(s: &mut AppState) -> Result<(), String> {
    let design = live_design(s)
        .cloned()
        .ok_or_else(|| "sim: no design (synth first)".to_string())?;
    let key = design_key(&design);
    if s.debug.engine.is_some() && s.debug.design_key.as_deref() == Some(key.as_str()) {
        return Ok(());
    }
    let sim = Sim::new(&design);
    let text = elaborate_text(&design, &sim);
    s.debug.engine = Some(sim);
    s.debug.design_key = Some(key);
    s.debug.prev.clear();
    debug_log(s, &text, MsgSev::Info);
    Ok(())
}

/// Fresh event engine for `sim_run` / `sim_restart` (old `prepare_sim`
/// recreates the engine on every run).
fn fresh_engine(s: &mut AppState, reset_hits: bool) -> Result<(), String> {
    let design = live_design(s)
        .cloned()
        .ok_or_else(|| "sim: no design (synth first)".to_string())?;
    let sim = Sim::new(&design);
    let text = elaborate_text(&design, &sim);
    s.debug.engine = Some(sim);
    s.debug.design_key = Some(design_key(&design));
    s.debug.prev.clear();
    if reset_hits {
        for b in &mut s.debug.breakpoints {
            b.hits = 0;
        }
    }
    debug_log(s, &text, MsgSev::Info);
    Ok(())
}

/// UG900 Breakpoints table dump (`breakpoints_text`).
pub fn breakpoints_text(s: &AppState) -> String {
    let n = s.debug.breakpoints.len();
    let mut out = format!("breakpoints n={n}");
    if n == 0 {
        out.push_str(" no breakpoints");
        return out;
    }
    for b in &s.debug.breakpoints {
        out.push('\n');
        out.push_str(&b.row_text());
    }
    out
}

/// Add `signal [value]` (`add_breakpoint`), validating against the live event
/// engine and selecting the new row.
pub fn add_breakpoint(s: &mut AppState, spec: &str) -> Result<String, String> {
    let toks: Vec<&str> = spec.split_whitespace().collect();
    if toks.is_empty() || toks.len() > 2 {
        return Err("add_bp: need <signal> [value]".into());
    }
    ensure_engine(s)?;
    let signal = toks[0].to_string();
    let value = if toks.len() == 2 {
        Some(
            toks[1]
                .parse::<u64>()
                .map_err(|_| "add_bp: value must be a number".to_string())?,
        )
    } else {
        None
    };
    let sim = s
        .debug
        .engine
        .as_ref()
        .ok_or_else(|| "sim: no design (synth first)".to_string())?;
    sim.signal_value(&signal)
        .ok_or_else(|| format!("add_bp: no signal {signal}"))?;
    let id = s.debug.next_id;
    s.debug.next_id += 1;
    let condition = value
        .map(|v| format!("=={v}"))
        .unwrap_or_else(|| "change".into());
    s.debug.breakpoints.push(Breakpoint {
        id,
        enabled: true,
        signal,
        condition,
        value,
        hits: 0,
        kind: "signal".into(),
        file: None,
        line: None,
    });
    s.debug.selected = Some(id);
    Ok(s.debug.breakpoints.last().expect("just pushed").row_text())
}

/// Resolve `id | index | name | bpN` (`resolve_breakpoint`).
fn resolve_breakpoint(s: &AppState, spec: &str) -> Result<usize, String> {
    let t = spec.trim();
    if t.is_empty() {
        if let Some(sel) = s.debug.selected {
            return s
                .debug
                .breakpoints
                .iter()
                .position(|b| b.id == sel)
                .ok_or_else(|| "select_breakpoint: no selection".to_string());
        }
        return Err("select_breakpoint: need id|index|name".into());
    }
    let lower = t.to_ascii_lowercase();
    let stripped = lower.strip_prefix("bp").unwrap_or(&lower);
    if let Ok(n) = stripped.parse::<usize>() {
        if let Some(i) = s.debug.breakpoints.iter().position(|b| b.id == n) {
            return Ok(i);
        }
        if n < s.debug.breakpoints.len() {
            return Ok(n);
        }
    }
    s.debug
        .breakpoints
        .iter()
        .position(|b| b.signal == t || b.id.to_string() == t)
        .ok_or_else(|| format!("select_breakpoint: no breakpoint {t}"))
}

/// Select a breakpoint row (`select_breakpoint`).
pub fn select_breakpoint(s: &mut AppState, spec: &str) -> Result<String, String> {
    let i = resolve_breakpoint(s, spec)?;
    let id = s.debug.breakpoints[i].id;
    s.debug.selected = Some(id);
    Ok(s.debug.breakpoints[i].row_text())
}

/// Enable/disable by selection or spec (`set_breakpoint_enabled`).
pub fn set_breakpoint_enabled(
    s: &mut AppState,
    spec: &str,
    enabled: bool,
) -> Result<String, String> {
    let i = resolve_breakpoint(s, spec).map_err(|_| {
        if spec.trim().is_empty() {
            "enable/disable: select a breakpoint first".to_string()
        } else {
            format!("enable/disable: no breakpoint {}", spec.trim())
        }
    })?;
    s.debug.breakpoints[i].enabled = enabled;
    Ok(s.debug.breakpoints[i].row_text())
}

/// Delete one row by selection or spec (`delete_breakpoint`).
pub fn delete_breakpoint(s: &mut AppState, spec: &str) -> Result<String, String> {
    let t = spec.trim();
    let idx = if t.is_empty() {
        s.debug
            .selected
            .and_then(|sel| s.debug.breakpoints.iter().position(|b| b.id == sel))
            .ok_or_else(|| "delete_bp: select a breakpoint first".to_string())?
    } else {
        let lower = t.to_ascii_lowercase();
        let stripped = lower.strip_prefix("bp").unwrap_or(&lower);
        if let Ok(n) = stripped.parse::<usize>() {
            if let Some(i) = s.debug.breakpoints.iter().position(|b| b.id == n) {
                i
            } else if n < s.debug.breakpoints.len() {
                n
            } else {
                return Err(format!("delete_bp: no breakpoint {t}"));
            }
        } else {
            s.debug
                .breakpoints
                .iter()
                .position(|b| b.signal == t || b.id.to_string() == t)
                .ok_or_else(|| format!("delete_bp: no breakpoint {t}"))?
        }
    };
    let removed = s.debug.breakpoints.remove(idx);
    if s.debug.selected == Some(removed.id) {
        s.debug.selected = None;
    }
    Ok(format!(
        "delete_bp id={} signal={}",
        removed.id, removed.signal
    ))
}

/// Run `cycles` fresh posedges and stop at the first enabled signal hit
/// (`sim_run`). Breakpoint hits persist across runs until `sim_restart`.
pub fn run_cycles(s: &mut AppState, cycles: u32) -> Result<String, String> {
    if let Err(e) = fresh_engine(s, false) {
        debug_log(s, &e, MsgSev::Error);
        return Err(e);
    }
    let ts = s.debug.timescale_ps.max(1);
    debug_log(
        s,
        &format!("sim_run start cycles={cycles} timescale_ps={ts}"),
        MsgSev::Info,
    );
    let (hit, ran, leds) = {
        let debug = &mut s.debug;
        let sim = debug
            .engine
            .as_mut()
            .ok_or_else(|| "sim: no design (synth first)".to_string())?;
        let mut hit: Option<String> = None;
        let mut ran = 0u32;
        let mut leds = String::new();
        for _ in 0..cycles {
            sim.step_posedge(ts);
            leds.push(if sim.led { '1' } else { '0' });
            ran += 1;
            let prev = debug.prev.clone();
            for b in debug.breakpoints.iter_mut().filter(|b| b.enabled) {
                let cur = sim
                    .signal_value(&b.signal)
                    .ok_or_else(|| format!("sim_run: no signal {}", b.signal))?;
                let tripped = match b.value {
                    Some(v) => cur == v,
                    None => prev.get(&b.signal).is_some_and(|p| *p != cur),
                };
                if tripped {
                    b.hits += 1;
                    hit = Some(format!(
                        "HIT id={} signal={} value={cur} CYCLE={ran} HITS={}",
                        b.id, b.signal, b.hits
                    ));
                    break;
                }
            }
            for b in &debug.breakpoints {
                if let Some(cur) = sim.signal_value(&b.signal) {
                    debug.prev.insert(b.signal.clone(), cur);
                }
            }
            if hit.is_some() {
                break;
            }
        }
        (hit, ran, leds)
    };
    if let Some(h) = hit {
        let time = s.debug.engine.as_ref().map(|sim| sim.time).unwrap_or(0);
        let line =
            format!("sim_run {h} cycles={ran}/{cycles} LED[{ran}]={leds} ENGINE_TIME_PS={time}");
        debug_log(s, &line, MsgSev::Warn);
        Ok(format!(
            "sim_run {h} cycles={ran}/{cycles} LED[{ran}]={leds}"
        ))
    } else {
        let time = s.debug.engine.as_ref().map(|sim| sim.time).unwrap_or(0);
        debug_log(
            s,
            &format!("sim_run cycles={cycles} LED[{cycles}]={leds} ENGINE_TIME_PS={time}"),
            MsgSev::Info,
        );
        Ok(format!("sim_run cycles={cycles} LED[{cycles}]={leds}"))
    }
}

/// Advance the persistent engine one posedge (`sim_step`). Breakpoints do
/// not stop a step, matching the old debugger.
pub fn step_once(s: &mut AppState) -> Result<String, String> {
    ensure_engine(s)?;
    let ts = s.debug.timescale_ps.max(1);
    let sim = s
        .debug
        .engine
        .as_mut()
        .ok_or_else(|| "sim: no design (synth first)".to_string())?;
    sim.step_posedge(ts);
    Ok(format!("sim_step t={}", sim.cycle * 2))
}

/// Drop and re-elaborate the engine, zero breakpoint hits (`sim_restart`).
/// The batch/sim log is preserved; restart only appends its own marker.
pub fn restart(s: &mut AppState) -> Result<String, String> {
    fresh_engine(s, true)?;
    debug_log(s, "sim_restart", MsgSev::Info);
    Ok("sim_restart".into())
}

fn ui_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text)
}

fn dim_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text_dim)
}

fn mono_cell(content: &str, t: &VivadoTheme) -> El {
    text(content)
        .font(VivadoTheme::FONT_MONO.mono())
        .color(t.text)
        .truncate()
}

/// One debugger toolbar button. Selection-gated buttons disable with a
/// reason instead of silently no-opping (old hover text).
fn debug_btn(key: &str, label: &str, t: &VivadoTheme, enabled: bool) -> El {
    let mut btn = el()
        .key(key.to_string())
        .role("button")
        .label(label.to_string())
        .focusable(enabled)
        .disabled(!enabled)
        .h(ROW_H)
        .px(8.0)
        .center()
        .hover(t.selection)
        .child(ui_label(label, t));
    if !enabled {
        btn = btn
            .description("Select a breakpoint below first")
            .layer_opacity(0.4);
    }
    btn
}

/// Seed the canonical debugger netlist (`examples/counter.sv`, with `led`)
/// for headless scenes and focused tests.
pub fn seed_debug_fixture(s: &mut AppState) -> Result<(), String> {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv");
    let design = helion_sv::synth_sv_path(&path)?;
    s.runs.source = Some(path);
    s.runs.live_design = Some(design);
    Ok(())
}

/// Submit the manual `signal [value]` editor through real `add_bp`.
/// Errors preserve the draft for correction; only success clears it.
fn submit_debug_spec(s: &mut AppState) {
    let spec = s.debug.spec.text.trim().to_string();
    let before = s.console.lines.len();
    if spec.is_empty() {
        crate::tcl::eval_command(s, "add_bp");
    } else {
        crate::tcl::eval_command(s, &format!("add_bp {spec}"));
    }
    let failed = s.console.lines[before..]
        .iter()
        .any(|l| l.sev == MsgSev::Error);
    if !failed {
        s.debug.spec.clear();
    }
}

/// Debugger toolbar: old Breakpoints recipes plus Step/Restart and a manual
/// signal spec. Every button evaluates its real Tcl verb.
fn debug_toolbar(s: &AppState, t: &VivadoTheme) -> El {
    let has_selection = s.debug.selected.is_some();
    row()
        .key("debug-toolbar")
        .role("toolbar")
        .w_full()
        .h(28.0)
        .items_center()
        .gap(4.0)
        .px(8.0)
        .fill(t.panel_bg)
        .child(
            debug_btn("debug-add-led-eq", "Add led == 1", t, true).on_press(|s: &mut AppState| {
                crate::tcl::eval_command(s, "add_bp led 1");
            }),
        )
        .child(
            debug_btn("debug-add-led-change", "Add led change", t, true).on_press(
                |s: &mut AppState| {
                    crate::tcl::eval_command(s, "add_bp led");
                },
            ),
        )
        .child(
            debug_btn("debug-run", "Run 16", t, true)
                .on_press(|s: &mut AppState| crate::tcl::eval_command(s, "sim_run")),
        )
        .child(
            debug_btn("debug-step", "Step", t, true)
                .on_press(|s: &mut AppState| crate::tcl::eval_command(s, "sim_step")),
        )
        .child(
            debug_btn("debug-restart", "Restart", t, true)
                .on_press(|s: &mut AppState| crate::tcl::eval_command(s, "sim_restart")),
        )
        .child(
            debug_btn("debug-disable", "Disable", t, has_selection).on_press(|s: &mut AppState| {
                crate::tcl::eval_command(s, "disable_bp");
            }),
        )
        .child(
            debug_btn("debug-enable", "Enable", t, has_selection).on_press(|s: &mut AppState| {
                crate::tcl::eval_command(s, "enable_bp");
            }),
        )
        .child(
            debug_btn("debug-delete", "Delete", t, has_selection).on_press(|s: &mut AppState| {
                crate::tcl::eval_command(s, "delete_bp");
            }),
        )
}

/// Manual breakpoint entry: `signal [value]`, validated by the engine.
fn debug_spec_row(s: &AppState, t: &VivadoTheme) -> El {
    row()
        .key("debug-spec-row")
        .w_full()
        .h(28.0)
        .items_center()
        .gap(6.0)
        .px(8.0)
        .child(
            text_input(&s.debug.spec, VivadoTheme::FONT_MONO.mono())
                .single_line()
                .key("debug-spec-input")
                .label("Add signal breakpoint")
                .w(220.0)
                .px(8.0)
                .py(3.0)
                .fill(t.chrome_bg)
                .border(1.0, t.border_light)
                .radius(4.0)
                .color(t.text)
                .placeholder("signal [value]", t.text_faint)
                .caret(t.text, t.selection)
                .on_edit(|s: &mut AppState| &mut s.debug.spec)
                .on_submit(|s: &mut AppState, _tx: &mut Tx| submit_debug_spec(s)),
        )
        .child(
            debug_btn("debug-add-spec", "Add", t, true)
                .on_press(|s: &mut AppState| submit_debug_spec(s)),
        )
}

/// One breakpoint table row; selecting evaluates `select_breakpoint`.
fn debug_row(s: &AppState, t: &VivadoTheme, b: &Breakpoint) -> El {
    let selected = s.debug.selected == Some(b.id);
    let id = b.id;
    row()
        .key(format!("debug-row-{id}"))
        .role("button")
        .label(b.row_text())
        .focus_selected(selected)
        .focusable(true)
        .w_full()
        .h(ROW_H)
        .items_center()
        .fill(if selected { t.selection } else { t.panel_bg })
        .hover(t.selection)
        .on_press(move |s: &mut AppState| {
            crate::tcl::eval_command(s, &format!("select_breakpoint {id}"));
        })
        .child(el().w(56.0).px(8.0).child(mono_cell(&id.to_string(), t)))
        .child(el().w(72.0).px(8.0).child(mono_cell(b.enabled_cell(), t)))
        .child(el().w(220.0).px(8.0).child(mono_cell(&b.signal, t)))
        .child(el().w(120.0).px(8.0).child(mono_cell(&b.condition, t)))
        .child(
            el().w(64.0)
                .px(8.0)
                .child(mono_cell(&b.hits.to_string(), t)),
        )
}

/// Breakpoints pane: old UG900 table over the persistent event engine.
pub fn debug_pane(s: &AppState, t: &VivadoTheme) -> El {
    let pane = col()
        .key("debug")
        .role("application")
        .label("Breakpoints")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.chrome_bg)
        .child(pane_header("Breakpoints - sim_1", "debug-pane", t))
        .child(debug_toolbar(s, t))
        .child(debug_spec_row(s, t))
        .child(
            row()
                .key("debug-status")
                .role("status")
                .w_full()
                .h(ROW_H)
                .items_center()
                .px(8.0)
                .child(dim_label(&s.debug.status(s), t)),
        );
    if s.debug.breakpoints.is_empty() {
        return pane.child(
            col()
                .key("debug-empty")
                .w_full()
                .gap(6.0)
                .px(8.0)
                .py(8.0)
                .child(dim_label("No breakpoints yet.", t))
                .child(row().w_full().items_center().child(
                    debug_btn("debug-empty-add", "Add led == 1", t, true).on_press(
                        |s: &mut AppState| {
                            crate::tcl::eval_command(s, "add_bp led 1");
                        },
                    ),
                )),
        );
    }
    let mut list = col()
        .key("debug-table")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        });
    list = list.child(
        row()
            .w_full()
            .h(ROW_H)
            .items_center()
            .fill(t.header_bg)
            .child(el().w(56.0).px(8.0).child(ui_label("Id", t)))
            .child(el().w(72.0).px(8.0).child(ui_label("Enabled", t)))
            .child(el().w(220.0).px(8.0).child(ui_label("Signal", t)))
            .child(el().w(120.0).px(8.0).child(ui_label("Condition", t)))
            .child(el().w(64.0).px(8.0).child(ui_label("Hits", t))),
    );
    for b in &s.debug.breakpoints {
        list = list.child(debug_row(s, t, b));
    }
    pane.child(list)
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

    fn debug_state() -> AppState {
        let mut s = init();
        s.center = crate::state::CenterPane::Debug;
        seed_debug_fixture(&mut s).expect("fixture must seed");
        s
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn console_text(rt: &Runtime<AppState>) -> Vec<String> {
        rt.state
            .console
            .lines
            .iter()
            .map(|l| l.text.clone())
            .collect()
    }

    #[test]
    fn debug_status_is_honest_before_and_after_elaboration() {
        let mut s = init();
        s.center = crate::state::CenterPane::Debug;
        assert_eq!(
            s.debug.status(&s),
            "No elaborated design — elaborate or run synthesis first"
        );
        seed_debug_fixture(&mut s).expect("fixture must seed");
        assert_eq!(s.debug.status(&s), "Event kernel · idle · 0 breakpoints");
    }

    #[test]
    fn debug_toolbar_runs_a_real_breakpoint_hit() {
        let mut rt = harness_with(debug_state());
        rt.frame(0.0);
        assert!(rt.find("debug-empty").is_some());
        tap(&mut rt, "debug-add-led-eq");
        assert_eq!(rt.state.debug.selected, Some(1));
        assert!(rt.find("debug-table").is_some());
        assert!(rt.find("debug-row-1").is_some());
        tap(&mut rt, "debug-run");
        let bp = &rt.state.debug.breakpoints[0];
        assert_eq!(bp.hits, 1, "a real engine hit must count");
        assert!(
            console_text(&rt)
                .iter()
                .any(|l| l.starts_with("sim_run HIT id=1 signal=led value=1")),
            "run must stop at the value: {console:?}",
            console = console_text(&rt)
        );
        assert_eq!(
            rt.state.debug.selected,
            Some(1),
            "runs must preserve row selection"
        );
    }

    #[test]
    fn debug_rows_gate_enable_disable_delete() {
        let mut rt = harness_with(debug_state());
        rt.frame(0.0);
        tap(&mut rt, "debug-add-led-change");
        tap(&mut rt, "debug-row-1");
        tap(&mut rt, "debug-disable");
        assert!(!rt.state.debug.breakpoints[0].enabled);
        tap(&mut rt, "debug-enable");
        assert!(rt.state.debug.breakpoints[0].enabled);
        tap(&mut rt, "debug-delete");
        assert!(rt.state.debug.breakpoints.is_empty());
        assert!(rt.find("debug-empty").is_some());
    }

    #[test]
    fn debug_spec_editor_adds_validated_signals() {
        let mut rt = harness_with(debug_state());
        rt.frame(0.0);
        rt.state.debug.spec = rune::TextEdit::new("led 0");
        rt.frame(1.0 / 60.0);
        tap(&mut rt, "debug-add-spec");
        assert_eq!(rt.state.debug.breakpoints.len(), 1);
        assert_eq!(rt.state.debug.breakpoints[0].condition, "==0");
        assert!(
            rt.state.debug.spec.text.is_empty(),
            "submit clears the spec"
        );
        rt.state.debug.spec = rune::TextEdit::new("bogus");
        rt.frame(1.0 / 60.0);
        tap(&mut rt, "debug-add-spec");
        assert_eq!(
            rt.state.debug.breakpoints.len(),
            1,
            "invalid specs add nothing"
        );
        assert_eq!(
            rt.state.debug.spec.text, "bogus",
            "errors preserve the draft"
        );
        assert!(
            console_text(&rt)
                .iter()
                .any(|l| l.contains("add_bp: no signal bogus")),
            "invalid specs must error: {console:?}",
            console = console_text(&rt)
        );
    }
}
