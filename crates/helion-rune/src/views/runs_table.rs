//! Design Runs (console dock tab): the run table, run creation with
//! strategy presets and seed sweeps, the one-at-a-time run queue, `Make
//! active`, and the routed checkpoints the runs wrote.
//!
//! Columns follow the old `RunCol` (`helion-gui/src/ide.rs` 1365-1400):
//! Name, Status, Strategy, WNS, TNS, LUTs, FFs, BRAM, DSP, Runtime, all
//! sortable; metrics come from the run's last finished [`RunRecord`]
//! (real `helion-sta` timing and `helion-pack` utilization). Run creation
//! mirrors `helion-ide.rs` 4768-4890 (name + `helion_proj::RunStrategy`
//! preset + optional seed sweep); the toolbar mirrors 4594-4625 (Launch
//! selected / Launch all / Cancel queue / Make active); checkpoints mirror
//! 4970 using `Session::write_dcp` / `open_checkpoint`.
//!
//! Incremental runs (B:5160) place against the active run's checkpoint via
//! `Session::read_checkpoint_incremental` and list their `ReuseReport`.
//! The ECO table (B:5260) is not here: runs keep no persistent session to
//! edit, and the console `eco` verb already covers the one ECO the backend
//! supports.

use rune::{El, ScrollSpec, TextEdit, col, el, icon, row, text};
use rune_kit::glyphs;

use crate::runs::{RunKind, RunOutcome, RunRecord, RunSpec};
use crate::state::{AppState, ConsoleLine, MsgSev, RunStatus, ToastKind};
use crate::theme::VivadoTheme;
use crate::views::cad::{self, ColSpec, FONT_PIN};

/// Strategy presets offered by Create Run (`RunStrategy::preset` names).
pub const PRESETS: [&str; 9] = [
    "Default",
    "TimingExplore",
    "RuntimeOpt",
    "PhysOpt",
    "Performance_Explore",
    "Performance_NetDelay_high",
    "Area_Explore",
    "Flow_Quick",
    "Congestion_SpreadLogic_high",
];

/// Most runs one seed sweep may create.
const MAX_SWEEP: usize = 16;

/// Sortable Design Runs column (old `RunCol`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunCol {
    Name,
    Status,
    Strategy,
    Wns,
    Tns,
    Luts,
    Ffs,
    Bram,
    Dsp,
    Runtime,
}

impl RunCol {
    pub const ALL: [RunCol; 10] = [
        RunCol::Name,
        RunCol::Status,
        RunCol::Strategy,
        RunCol::Wns,
        RunCol::Tns,
        RunCol::Luts,
        RunCol::Ffs,
        RunCol::Bram,
        RunCol::Dsp,
        RunCol::Runtime,
    ];

    fn spec(self) -> ColSpec {
        match self {
            RunCol::Name => ColSpec::fixed("Name", 120.0),
            RunCol::Status => ColSpec::fixed("Status", 96.0),
            RunCol::Strategy => ColSpec::grow("Strategy", 1.0),
            RunCol::Wns => ColSpec::num("WNS (ns)", 70.0),
            RunCol::Tns => ColSpec::num("TNS (ns)", 70.0),
            RunCol::Luts => ColSpec::num("LUTs", 56.0),
            RunCol::Ffs => ColSpec::num("FFs", 56.0),
            RunCol::Bram => ColSpec::num("BRAM", 48.0),
            RunCol::Dsp => ColSpec::num("DSP", 44.0),
            RunCol::Runtime => ColSpec::num("Runtime", 64.0),
        }
    }

    fn slug(self) -> &'static str {
        match self {
            RunCol::Name => "name",
            RunCol::Status => "status",
            RunCol::Strategy => "strategy",
            RunCol::Wns => "wns",
            RunCol::Tns => "tns",
            RunCol::Luts => "luts",
            RunCol::Ffs => "ffs",
            RunCol::Bram => "bram",
            RunCol::Dsp => "dsp",
            RunCol::Runtime => "runtime",
        }
    }
}

/// Design Runs UI state (lives in [`crate::runs::RunsState::ui`]).
#[derive(Clone, Debug, Default)]
pub struct RunsUi {
    pub selected: Option<String>,
    /// Sort column and descending flag (`None` = creation order).
    pub sort: Option<(RunCol, bool)>,
    pub create_open: bool,
    pub name: TextEdit,
    pub seeds: TextEdit,
    /// Index into [`PRESETS`].
    pub preset: usize,
    pub preset_pop: bool,
    /// New runs place incrementally against the active run's checkpoint.
    pub incremental: bool,
}

/// The active run's routed checkpoint, when it is still on disk: the
/// reference an incremental run reuses.
pub fn active_checkpoint(s: &AppState) -> Option<(String, std::path::PathBuf)> {
    let name = s.runs.active_result.clone()?;
    checkpoints(s)
        .into_iter()
        .find(|(n, _, _)| *n == name)
        .map(|(n, p, _)| (n, p))
}

/// Make the runs `names` incremental against `reference`.
pub fn set_incremental(s: &mut AppState, names: &[String], reference: &std::path::Path) {
    for n in names {
        if let Some(d) = s.runs.defs.iter_mut().find(|d| &d.name == n) {
            d.incremental = Some(reference.to_path_buf());
        }
        echo(
            s,
            format!(
                "set_property INCREMENTAL_CHECKPOINT {{{}}} [get_runs {n}]",
                reference.display()
            ),
        );
    }
}

/// Row status, derived from the worker, the queue, and run history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowStatus {
    NotStarted,
    Queued,
    Running,
    Complete,
    Failed,
    Cancelled,
}

impl RowStatus {
    pub fn label(self) -> &'static str {
        match self {
            RowStatus::NotStarted => "Not started",
            RowStatus::Queued => "Queued",
            RowStatus::Running => "Running",
            RowStatus::Complete => "Complete",
            RowStatus::Failed => "Failed",
            RowStatus::Cancelled => "Cancelled",
        }
    }
}

/// One Design Runs row.
#[derive(Clone, Debug)]
pub struct RunRow {
    pub spec: RunSpec,
    pub status: RowStatus,
    pub strategy: String,
    /// Last completed record (metrics + checkpoint + artifacts).
    pub done: Option<RunRecord>,
}

fn echo(s: &mut AppState, text: String) {
    s.console.lines.push(ConsoleLine {
        text,
        sev: MsgSev::Info,
    });
}

/// Strategy cell text: preset plus a non-zero seed.
pub fn strategy_label(st: &helion_proj::RunStrategy) -> String {
    if st.placer_seed != 0 {
        format!("{} · seed {}", st.preset, st.placer_seed)
    } else {
        st.preset.clone()
    }
}

/// Every design run: `synth_1`, `impl_1`, then user-created runs.
pub fn run_rows(s: &AppState) -> Vec<RunRow> {
    let mut specs = vec![
        RunSpec::builtin(s, RunKind::Synth),
        RunSpec::builtin(s, RunKind::Impl),
    ];
    specs.extend(s.runs.defs.iter().cloned());
    specs
        .into_iter()
        .map(|spec| {
            let last = s.runs.last_record(&spec.name);
            let status = if s.runs.active_name() == Some(spec.name.as_str()) {
                RowStatus::Running
            } else if s.runs.queue.iter().any(|q| q.name == spec.name) {
                RowStatus::Queued
            } else if spec.is_builtin() {
                // Built-ins mirror the flow cards (the rail and summary
                // read the same statuses).
                match if spec.kind == RunKind::Synth {
                    s.flow.synth
                } else {
                    s.flow.impl_run
                } {
                    RunStatus::Running => RowStatus::Running,
                    RunStatus::Complete => RowStatus::Complete,
                    RunStatus::Failed => RowStatus::Failed,
                    RunStatus::NotStarted => match last.map(|r| r.outcome) {
                        Some(RunOutcome::Cancelled) => RowStatus::Cancelled,
                        _ => RowStatus::NotStarted,
                    },
                }
            } else {
                match last.map(|r| r.outcome) {
                    None => RowStatus::NotStarted,
                    Some(RunOutcome::Complete) => RowStatus::Complete,
                    Some(RunOutcome::Failed) => RowStatus::Failed,
                    Some(RunOutcome::Cancelled) => RowStatus::Cancelled,
                }
            };
            let strategy = match (spec.kind, spec.is_builtin()) {
                (RunKind::Synth, _) => s.flow.synth_strategy.clone(),
                (RunKind::Impl, true) => s.flow.impl_strategy.clone(),
                (RunKind::Impl, false) => {
                    let mut l = strategy_label(&spec.strategy);
                    if spec.incremental.is_some() {
                        l.push_str(" · incremental");
                    }
                    l
                }
            };
            let done = s
                .runs
                .history
                .iter()
                .rev()
                .find(|r| r.name == spec.name && r.outcome == RunOutcome::Complete)
                .cloned();
            RunRow {
                spec,
                status,
                strategy,
                done,
            }
        })
        .collect()
}

fn ns(ps: Option<i64>) -> String {
    ps.map(|p| format!("{:.3}", p as f64 / 1000.0))
        .unwrap_or_else(|| "–".into())
}

fn count(v: Option<u32>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "–".into())
}

fn runtime(ms: Option<u64>) -> String {
    match ms {
        None => "–".into(),
        Some(ms) if ms < 1000 => format!("{ms} ms"),
        Some(ms) if ms < 60_000 => format!("{:.1} s", ms as f64 / 1000.0),
        Some(ms) => format!("{}m {:02}s", ms / 60_000, (ms / 1000) % 60),
    }
}

fn sort_rows(rows: &mut [RunRow], col: RunCol, desc: bool) {
    let num = |r: &RunRow| -> Option<i64> {
        let d = r.done.as_ref()?;
        match col {
            RunCol::Wns => d.wns_ps,
            RunCol::Tns => d.tns_ps,
            RunCol::Luts => d.luts.map(i64::from),
            RunCol::Ffs => d.ffs.map(i64::from),
            RunCol::Bram => d.bram.map(i64::from),
            RunCol::Dsp => d.dsp.map(i64::from),
            RunCol::Runtime => Some(d.duration_ms as i64),
            _ => None,
        }
    };
    rows.sort_by(|a, b| {
        let ord = match col {
            RunCol::Name => a.spec.name.cmp(&b.spec.name),
            RunCol::Status => a.status.label().cmp(b.status.label()),
            RunCol::Strategy => a.strategy.cmp(&b.strategy),
            _ => match (num(a), num(b)) {
                // Runs without the metric always sink to the bottom.
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => return std::cmp::Ordering::Less,
                (None, Some(_)) => return std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            },
        };
        if desc { ord.reverse() } else { ord }
    });
}

/// Rows in display order (the active sort, else creation order).
pub fn sorted_rows(s: &AppState) -> Vec<RunRow> {
    let mut rows = run_rows(s);
    if let Some((col, desc)) = s.runs.ui.sort {
        sort_rows(&mut rows, col, desc);
    }
    rows
}

/// Header press: sort by `col`, toggling direction on a repeat press.
pub fn toggle_sort(s: &mut AppState, col: RunCol) {
    s.runs.ui.sort = match s.runs.ui.sort {
        Some((c, desc)) if c == col => Some((col, !desc)),
        _ => Some((col, false)),
    };
}

fn spec_named(s: &AppState, name: &str) -> Option<RunSpec> {
    run_rows(s)
        .into_iter()
        .find(|r| r.spec.name == name)
        .map(|r| r.spec)
}

/// Queue the run `name` (no-op with a reason when it is already running
/// or queued). Starts it at once when the worker is idle.
pub fn enqueue(s: &mut AppState, name: &str) -> Result<(), String> {
    let spec = spec_named(s, name).ok_or_else(|| format!("no run named {name}"))?;
    if s.runs.active_name() == Some(name) {
        return Err(format!("{name} is already running"));
    }
    if s.runs.queue.iter().any(|q| q.name == name) {
        return Err(format!("{name} is already queued"));
    }
    s.runs.queue.push_back(spec);
    crate::runs::pump_queue(s);
    Ok(())
}

/// Runs `Launch All` queues: everything not running, queued, or complete.
pub fn pending_runs(s: &AppState) -> Vec<String> {
    run_rows(s)
        .into_iter()
        .filter(|r| {
            matches!(
                r.status,
                RowStatus::NotStarted | RowStatus::Failed | RowStatus::Cancelled
            )
        })
        .map(|r| r.spec.name)
        .collect()
}

/// Validate a new run name.
pub fn name_blocker(s: &AppState, name: &str) -> Option<String> {
    let n = name.trim();
    if n.is_empty() {
        return Some("Name the run first".into());
    }
    if !n
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Some("Use letters, digits, _ or - in run names".into());
    }
    if run_rows(s).iter().any(|r| r.spec.name == n) {
        return Some(format!("A run named {n} already exists"));
    }
    None
}

/// Parse a seed list (`1, 2, 7`); empty = no sweep.
pub fn parse_seeds(text: &str) -> Result<Vec<u64>, String> {
    let mut out = Vec::new();
    for part in text.split([',', ' ']).filter(|p| !p.trim().is_empty()) {
        let v: u64 = part
            .trim()
            .parse()
            .map_err(|_| format!("Seed \"{}\" is not a whole number", part.trim()))?;
        if !out.contains(&v) {
            out.push(v);
        }
    }
    if out.len() > MAX_SWEEP {
        return Err(format!("A sweep holds at most {MAX_SWEEP} seeds"));
    }
    Ok(out)
}

/// Create implementation run(s): one run, or one run per seed
/// (`<name>_s<seed>`) with the placer seed knob set. Returns the names.
pub fn create_runs(
    s: &mut AppState,
    name: &str,
    preset: &str,
    seeds: &[u64],
) -> Result<Vec<String>, String> {
    let base = helion_proj::RunStrategy::preset(preset)?;
    let name = name.trim().to_string();
    let names: Vec<String> = if seeds.is_empty() {
        vec![name.clone()]
    } else {
        seeds.iter().map(|sd| format!("{name}_s{sd}")).collect()
    };
    for n in &names {
        if let Some(why) = name_blocker(s, n) {
            return Err(why);
        }
    }
    let mut out = Vec::new();
    for (k, n) in names.iter().enumerate() {
        let mut strategy = base.clone();
        if let Some(sd) = seeds.get(k) {
            strategy.placer_seed = *sd;
        }
        echo(
            s,
            format!("create_run {n} -flow impl -strategy {}", strategy.to_spec()),
        );
        s.runs.defs.push(RunSpec {
            name: n.clone(),
            kind: RunKind::Impl,
            strategy,
            incremental: None,
        });
        out.push(n.clone());
    }
    s.runs.ui.selected = out.first().cloned();
    Ok(out)
}

/// Next free `impl_N` name for the Create form.
pub fn next_run_name(s: &AppState) -> String {
    let rows = run_rows(s);
    (2..)
        .map(|i| format!("impl_{i}"))
        .find(|n| rows.iter().all(|r| &r.spec.name != n))
        .unwrap_or_else(|| "impl_x".into())
}

/// Re-apply a completed run's results to the report views.
pub fn make_active(s: &mut AppState, name: &str) -> Result<(), String> {
    let rec = run_rows(s)
        .into_iter()
        .find(|r| r.spec.name == name)
        .and_then(|r| r.done)
        .ok_or_else(|| format!("{name} has no completed results"))?;
    let art = rec
        .artifacts
        .ok_or_else(|| format!("{name} kept no results"))?;
    crate::runs::apply_artifacts(s, *art);
    s.runs.active_result = Some(name.to_string());
    echo(s, format!("current_run [get_runs {name}]"));
    s.toasts.push(
        ToastKind::Info,
        "Active run changed",
        &format!("Reports now show {name}"),
    );
    Ok(())
}

fn key_for(name: &str) -> String {
    match name {
        "synth_1" => "console-run-synth".into(),
        "impl_1" => "console-run-impl".into(),
        other => format!("console-run-x-{other}"),
    }
}

fn status_color(st: RowStatus, t: &VivadoTheme) -> rune::Color {
    match st {
        RowStatus::Complete => t.pass,
        RowStatus::Failed => t.fail,
        RowStatus::Running => t.link,
        RowStatus::Queued => t.warn,
        RowStatus::Cancelled => t.text_dim,
        RowStatus::NotStarted => t.text,
    }
}

fn tool_btn(
    key: &str,
    label: &str,
    t: &VivadoTheme,
    reason: Option<&str>,
    f: impl Fn(&mut AppState) + 'static,
) -> El {
    cad::btn(key.to_string(), label, t, reason, f)
}

fn toolbar(s: &AppState, t: &VivadoTheme) -> El {
    let sel = s.runs.ui.selected.clone();
    let rows = run_rows(s);
    let sel_row = sel
        .as_deref()
        .and_then(|n| rows.iter().find(|r| r.spec.name == n));
    let launch_why = match sel_row {
        None => Some("Select a run first".to_string()),
        Some(r) if r.status == RowStatus::Running => Some(format!("{} is running", r.spec.name)),
        Some(r) if r.status == RowStatus::Queued => {
            Some(format!("{} is already queued", r.spec.name))
        }
        _ => None,
    };
    let pending = pending_runs(s);
    let all_why = pending
        .is_empty()
        .then_some("Every run is complete, running, or queued");
    let queue_why = s.runs.queue.is_empty().then_some("The queue is empty");
    let active_why = match sel_row {
        None => Some("Select a completed run first".to_string()),
        Some(r) if r.done.as_ref().is_none_or(|d| d.artifacts.is_none()) => {
            Some(format!("{} has no completed results", r.spec.name))
        }
        Some(r) if s.runs.active_result.as_deref() == Some(r.spec.name.as_str()) => {
            Some(format!("{} is already the active run", r.spec.name))
        }
        _ => None,
    };
    let sel_l = sel.clone();
    let sel_m = sel.clone();
    let queued = s.runs.queue.len();
    cad::toolbar("runs-toolbar", t)
        .child(
            text(format!("Design Runs ({})", rows.len()))
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text),
        )
        .child(cad::tool_sep(t))
        .child(tool_btn(
            "runs-launch-selected",
            "Launch Selected",
            t,
            launch_why.as_deref(),
            move |s| {
                if let Some(n) = &sel_l {
                    if let Err(e) = enqueue(s, n) {
                        s.toasts.push(ToastKind::Error, "Cannot launch", &e);
                    }
                }
            },
        ))
        .child(
            tool_btn("runs-launch-all", "Launch All", t, all_why, |s| {
                for n in pending_runs(s) {
                    let _ = enqueue(s, &n);
                }
            })
            .help("Queue every run that has not completed; runs execute one at a time"),
        )
        .child(tool_btn(
            "runs-cancel-queue",
            "Cancel Queue",
            t,
            queue_why,
            |s| {
                let n = s.runs.queue.len();
                s.runs.queue.clear();
                echo(s, format!("cancel_run_queue ;# {n} queued run(s) dropped"));
            },
        ))
        .child(tool_btn(
            "runs-make-active",
            "Make Active",
            t,
            active_why.as_deref(),
            move |s| {
                if let Some(n) = &sel_m {
                    if let Err(e) = make_active(s, n) {
                        s.toasts.push(ToastKind::Error, "Cannot make active", &e);
                    }
                }
            },
        ))
        .child(el().grow(1.0))
        .child(
            text(if queued == 0 {
                String::new()
            } else {
                format!("{queued} queued")
            })
            .key("runs-queue-count")
            .font(VivadoTheme::FONT_UI)
            .color(t.warn),
        )
        .child(
            tool_btn(
                "runs-create-toggle",
                if s.runs.ui.create_open {
                    "Close Create Run"
                } else {
                    "+ Create Run"
                },
                t,
                None,
                |s| {
                    s.runs.ui.create_open = !s.runs.ui.create_open;
                    if s.runs.ui.create_open && s.runs.ui.name.text.trim().is_empty() {
                        s.runs.ui.name = TextEdit::new(&next_run_name(s));
                    }
                    s.runs.ui.preset_pop = false;
                },
            )
            .fill(if s.runs.ui.create_open {
                t.selection
            } else {
                t.panel_bg
            }),
        )
}

fn field(
    key: &'static str,
    edit: &TextEdit,
    placeholder: &'static str,
    w: f32,
    t: &VivadoTheme,
    on_edit: impl for<'a> Fn(&'a mut AppState) -> &'a mut TextEdit + 'static,
) -> El {
    row()
        .key(format!("{key}-box"))
        .w(w)
        .h(22.0)
        .px(6.0)
        .items_center()
        .radius(3.0)
        .fill(t.chrome_bg)
        .border(1.0, t.border_light)
        .child(
            rune::text_input(edit, VivadoTheme::FONT_UI)
                .single_line()
                .key(key)
                .label(placeholder)
                .grow(1.0)
                .min_w(0.0)
                .color(t.text)
                .placeholder(placeholder, t.text_faint)
                .caret(t.text, t.selection)
                .on_edit(on_edit),
        )
}

fn create_form(s: &AppState, t: &VivadoTheme) -> El {
    let name = s.runs.ui.name.text.clone();
    let seeds = parse_seeds(&s.runs.ui.seeds.text);
    let preset = PRESETS[s.runs.ui.preset.min(PRESETS.len() - 1)];
    let why = match &seeds {
        Err(e) => Some(e.clone()),
        Ok(list) if list.is_empty() => name_blocker(s, &name),
        Ok(list) => list
            .iter()
            .find_map(|sd| name_blocker(s, &format!("{}_s{sd}", name.trim())))
            .or_else(|| {
                name_blocker(s, &name).filter(|w| w.starts_with("Name") || w.starts_with("Use"))
            }),
    };
    let n_new = seeds.as_ref().map(|l| l.len().max(1)).unwrap_or(1);
    let create = move |s: &mut AppState, launch: bool| {
        let Ok(seeds) = parse_seeds(&s.runs.ui.seeds.text) else {
            return;
        };
        let name = s.runs.ui.name.text.clone();
        let preset = PRESETS[s.runs.ui.preset.min(PRESETS.len() - 1)];
        match create_runs(s, &name, preset, &seeds) {
            Ok(names) => {
                if s.runs.ui.incremental {
                    if let Some((_, reference)) = active_checkpoint(s) {
                        set_incremental(s, &names, &reference);
                    }
                }
                if launch {
                    for n in &names {
                        let _ = enqueue(s, n);
                    }
                }
                s.runs.ui.name = TextEdit::new(&next_run_name(s));
                s.runs.ui.seeds = TextEdit::new("");
            }
            Err(e) => s.toasts.push(ToastKind::Error, "Cannot create run", &e),
        }
    };
    let preset_trigger = el()
        .key("runs-create-preset")
        .role("button")
        .label(format!("Strategy preset, current {preset}"))
        .focusable(true)
        .w(220.0)
        .h(22.0)
        .px(8.0)
        .center_y()
        .radius(3.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .hover(t.selection)
        .on_press(|s: &mut AppState| s.runs.ui.preset_pop = !s.runs.ui.preset_pop)
        .child(
            row()
                .w_full()
                .items_center()
                .gap(4.0)
                .child(
                    cad::cut(preset, VivadoTheme::FONT_UI, t.text)
                        .grow(1.0)
                        .min_w(0.0),
                )
                .child(
                    icon(&glyphs::CHEVRON_DOWN, 10.0)
                        .color(t.text_dim)
                        .a11y_hidden(),
                ),
        );
    let label = |l: &str| {
        text(l.to_string())
            .font(VivadoTheme::FONT_UI)
            .color(t.text_dim)
    };
    let create_label = if n_new > 1 {
        format!("Create {n_new} Runs")
    } else {
        "Create".to_string()
    };
    let why_c = why.clone();
    row()
        .key("runs-create")
        .w_full()
        .h(32.0)
        .px(8.0)
        .gap(8.0)
        .items_center()
        .fill(t.header_bg)
        .child(label("Name"))
        .child(field(
            "runs-create-name",
            &s.runs.ui.name,
            "impl_2",
            140.0,
            t,
            |s| &mut s.runs.ui.name,
        ))
        .child(label("Strategy"))
        .child(preset_trigger)
        .child(label("Seed sweep"))
        .child(field(
            "runs-create-seeds",
            &s.runs.ui.seeds,
            "optional, e.g. 1, 2, 3",
            160.0,
            t,
            |s| &mut s.runs.ui.seeds,
        ))
        .child({
            let ck = active_checkpoint(s);
            let why = ck
                .is_none()
                .then_some("Make a run with a routed checkpoint active first");
            let on = s.runs.ui.incremental && ck.is_some();
            let label = match &ck {
                Some((n, _)) if on => format!("Incremental from {n}"),
                _ => "Incremental: off".to_string(),
            };
            tool_btn("runs-create-incr", &label, t, why, |s| {
                s.runs.ui.incremental = !s.runs.ui.incremental
            })
            .fill(if on { t.selection } else { t.panel_bg })
            .help("Place against the active run's checkpoint (read_checkpoint -incremental)")
        })
        .child(tool_btn(
            "runs-create-go",
            &create_label,
            t,
            why.as_deref(),
            move |s| create(s, false),
        ))
        .child(cad::primary_btn(
            "runs-create-launch",
            "Create & Launch",
            t,
            why_c.as_deref(),
            move |s| create(s, true),
        ))
        .child(el().grow(1.0))
        .child(
            text("Seed 0 = deterministic placement")
                .font(FONT_PIN)
                .color(t.text_faint),
        )
}

fn header(s: &AppState, t: &VivadoTheme) -> El {
    let mut r = row()
        .key("runs-head")
        .w_full()
        .h(cad::HEAD_H)
        .items_center()
        .gap(8.0)
        .px(8.0)
        .fill(t.header_bg);
    for col in RunCol::ALL {
        let spec = col.spec();
        let arrow = match s.runs.ui.sort {
            Some((c, desc)) if c == col => {
                if desc {
                    " ▼"
                } else {
                    " ▲"
                }
            }
            _ => "",
        };
        let title = format!("{}{arrow}", spec.title);
        let head = text(title.clone())
            .font(VivadoTheme::FONT_UI)
            .color(t.text_dim)
            .truncate();
        let head = if spec.right {
            head.w_full().align(rune::Align::End)
        } else {
            head
        };
        let cell = el()
            .key(format!("runs-sort-{}", col.slug()))
            .role("button")
            .label(format!("Sort by {}", spec.title))
            .focusable(true)
            .h_full()
            .center_y()
            .hover(t.selection)
            .on_press(move |s: &mut AppState| toggle_sort(s, col))
            .child(head);
        r = r.child(match spec.w {
            cad::ColW::Fixed(w) => cell.w(w),
            cad::ColW::Grow(g) => cell.grow(g).min_w(0.0),
        });
    }
    r.child(el().w(56.0).child(cad::dim("Action", t)))
}

fn table_row(s: &AppState, r: &RunRow, t: &VivadoTheme) -> El {
    let name = r.spec.name.clone();
    let key = key_for(&name);
    let selected = s.runs.ui.selected.as_deref() == Some(name.as_str());
    let active = s.runs.active_result.as_deref() == Some(name.as_str());
    let d = r.done.as_ref();
    let cells: Vec<El> = RunCol::ALL
        .iter()
        .map(|c| {
            let spec = c.spec();
            let content = match c {
                RunCol::Name => {
                    row()
                        .gap(4.0)
                        .items_center()
                        .child(cad::cut(name.clone(), VivadoTheme::FONT_UI, t.text).min_w(0.0))
                        .children(active.then(|| {
                            cad::badge("active", t.accent, t).key(format!("{key}-active"))
                        }))
                }
                RunCol::Status => text(r.status.label())
                    .key(format!("{key}-status"))
                    .font(VivadoTheme::FONT_UI)
                    .color(status_color(r.status, t)),
                RunCol::Strategy => cad::cut(r.strategy.clone(), VivadoTheme::FONT_UI, t.text_dim),
                RunCol::Wns => wns_cell(d.and_then(|d| d.wns_ps), t),
                RunCol::Tns => cad::mono(ns(d.and_then(|d| d.tns_ps)), t),
                RunCol::Luts => cad::mono(count(d.and_then(|d| d.luts)), t),
                RunCol::Ffs => cad::mono(count(d.and_then(|d| d.ffs)), t),
                RunCol::Bram => cad::mono(count(d.and_then(|d| d.bram)), t),
                RunCol::Dsp => cad::mono(count(d.and_then(|d| d.dsp)), t),
                RunCol::Runtime => cad::mono(runtime(d.map(|d| d.duration_ms)), t),
            };
            cad::cell(&spec, content)
        })
        .collect();
    let running = r.status == RowStatus::Running;
    let busy = s.runs.is_active();
    let (action, ink) = if running {
        ("Cancel", t.link)
    } else if r.status == RowStatus::Queued {
        ("Dequeue", t.link)
    } else if busy {
        ("Queue", t.link)
    } else {
        ("Run", t.link)
    };
    let act_name = name.clone();
    let is_builtin = r.spec.is_builtin();
    let kind = r.spec.kind;
    let sel_name = name.clone();
    cad::table_row(
        key.clone(),
        format!("Run {name}, {}", r.status.label()),
        selected,
        t,
    )
    .on_press(move |s: &mut AppState| s.runs.ui.selected = Some(sel_name.clone()))
    .children(cells)
    .child(
        el().key(format!("{key}-action"))
            .role("button")
            .label(format!("{action} {name}"))
            .focusable(true)
            .w(56.0)
            .h(cad::ROW_H)
            .center_y()
            .hover(t.selection)
            .on_press(move |s: &mut AppState| {
                if running || (is_builtin && run_flow_running(s, kind)) {
                    crate::runs::cancel_run(s);
                } else if s.runs.queue.iter().any(|q| q.name == act_name) {
                    s.runs.queue.retain(|q| q.name != act_name);
                    echo(s, format!("cancel_runs {act_name} ;# dequeued"));
                } else if is_builtin && !s.runs.is_active() {
                    // The flow verb path (echo + source staging).
                    crate::runs::launch(s, kind);
                } else if let Err(e) = enqueue(s, &act_name) {
                    s.toasts.push(ToastKind::Error, "Cannot launch", &e);
                }
            })
            .child(text(action).font(VivadoTheme::FONT_UI).color(ink)),
    )
}

/// A built-in row whose flow card says Running (fixtures/tests may set it
/// without a worker): its action is Cancel, which explains itself.
fn run_flow_running(s: &AppState, kind: RunKind) -> bool {
    match kind {
        RunKind::Synth => s.flow.synth == RunStatus::Running,
        RunKind::Impl => s.flow.impl_run == RunStatus::Running,
    }
}

fn wns_cell(ps: Option<i64>, t: &VivadoTheme) -> El {
    let color = match ps {
        Some(p) if p < 0 => t.fail,
        Some(_) => t.pass,
        None => t.text_dim,
    };
    text(ns(ps)).font(cad::mono_style()).color(color)
}

fn file_size(p: &std::path::Path) -> Option<u64> {
    std::fs::metadata(p).ok().map(|m| m.len())
}

fn size_label(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Routed checkpoints the impl runs wrote (newest per run, still on disk).
pub fn checkpoints(s: &AppState) -> Vec<(String, std::path::PathBuf, u64)> {
    let mut out: Vec<(String, std::path::PathBuf, u64)> = Vec::new();
    for r in s.runs.history.iter().rev() {
        if out.iter().any(|(n, _, _)| n == &r.name) {
            continue;
        }
        if let Some(p) = &r.checkpoint {
            if let Some(sz) = file_size(p) {
                out.push((r.name.clone(), p.clone(), sz));
            }
        }
    }
    out.reverse();
    out
}

fn checkpoints_section(s: &AppState, t: &VivadoTheme) -> El {
    let cols = [
        ColSpec::fixed("Run", 120.0),
        ColSpec::fixed("Stage", 96.0),
        ColSpec::num("Size", 70.0),
        ColSpec::grow("Path", 1.0),
    ];
    let list = checkpoints(s);
    let mut c = col()
        .key("runs-checkpoints")
        .w_full()
        .child(
            row()
                .w_full()
                .h(28.0)
                .px(8.0)
                .pt(8.0)
                .items_center()
                .child(cad::title("Checkpoints", t)),
        )
        .child(cad::header("runs-ckpt-head", &cols, t));
    if list.is_empty() {
        return c.child(cad::empty_state(
            "runs-ckpt-empty",
            "No checkpoints yet — each implementation run writes a routed checkpoint when it completes.",
            None,
            t,
        ));
    }
    for (k, (name, path, size)) in list.into_iter().enumerate() {
        let p = path.display().to_string();
        let open = p.clone();
        c = c.child(
            cad::table_row(
                format!("runs-ckpt-{k}"),
                format!("Checkpoint of {name}"),
                false,
                t,
            )
            .child(cad::cell(
                &cols[0],
                cad::cut(name.clone(), VivadoTheme::FONT_UI, t.text),
            ))
            .child(cad::cell(&cols[1], cad::dim("Routed", t)))
            .child(cad::cell(&cols[2], cad::mono(size_label(size), t)))
            .child(cad::cell(
                &cols[3],
                cad::cut(p, cad::mono_style(), t.text_dim),
            ))
            .child(
                cad::link(
                    format!("runs-ckpt-{k}-open"),
                    "Open",
                    t,
                    move |s: &mut AppState| {
                        crate::tcl::eval_command(s, &format!("open_checkpoint {{{open}}}"));
                    },
                )
                .help("Load into the Tcl session (open_checkpoint)"),
            ),
        );
    }
    c
}

/// Incremental reuse of runs that placed against a reference checkpoint
/// (`ReuseReport::text`); absent until one has.
fn reuse_section(s: &AppState, t: &VivadoTheme) -> Option<El> {
    let rows: Vec<&RunRecord> = s
        .runs
        .history
        .iter()
        .filter(|r| r.reuse.is_some())
        .collect();
    if rows.is_empty() {
        return None;
    }
    let cols = [ColSpec::fixed("Run", 120.0), ColSpec::grow("Reuse", 1.0)];
    let mut c = col()
        .key("runs-reuse")
        .w_full()
        .child(
            row()
                .w_full()
                .h(28.0)
                .px(8.0)
                .pt(8.0)
                .items_center()
                .child(cad::title("Incremental Reuse", t)),
        )
        .child(cad::header("runs-reuse-head", &cols, t));
    for (k, r) in rows.into_iter().enumerate() {
        let reuse = r.reuse.clone().unwrap_or_default();
        c = c.child(
            cad::table_row(
                format!("runs-reuse-{k}"),
                format!("Reuse of {}", r.name),
                false,
                t,
            )
            .child(cad::cell(
                &cols[0],
                cad::cut(r.name.clone(), VivadoTheme::FONT_UI, t.text),
            ))
            .child(cad::cell(
                &cols[1],
                cad::cut(reuse, cad::mono_style(), t.text_dim),
            )),
        );
    }
    Some(c)
}

/// Design Runs tab body.
pub fn runs_body(s: &AppState, t: &VivadoTheme) -> El {
    let mut table = col().key("runs-table").w_full().child(header(s, t));
    for r in sorted_rows(s) {
        table = table.child(table_row(s, &r, t));
    }
    let mut body = col()
        .key("console-runs")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(toolbar(s, t));
    if s.runs.ui.create_open {
        body = body.child(create_form(s, t));
    }
    body.child(
        col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(table)
            .child(checkpoints_section(s, t))
            .child(crate::views::ooc::ooc_section(s, t))
            .children(reuse_section(s, t)),
    )
}

/// Push the strategy preset popover (overlay layer from the root view).
pub fn push_runs_pops(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme) {
    if !s.runs.ui.preset_pop
        || !s.runs.ui.create_open
        || s.console.collapsed
        || s.console.tab != crate::views::console::ConsoleTab::DesignRuns
    {
        return;
    }
    let kit = t.kit();
    let mut opts = col().w_full().gap(0.0);
    for (i, p) in PRESETS.iter().enumerate() {
        let st = helion_proj::RunStrategy::preset(p).ok();
        let detail = st
            .map(|st| {
                format!(
                    "opt {} · route iters {}{}",
                    st.opt_level,
                    st.router_iters,
                    if st.phys_opt { " · phys-opt" } else { "" }
                )
            })
            .unwrap_or_default();
        let on = s.runs.ui.preset == i;
        opts = opts.child(
            row()
                .key(format!("runs-preset-{i}"))
                .role("button")
                .label(format!("Choose {p}"))
                .focusable(true)
                .w_full()
                .h(24.0)
                .px(8.0)
                .gap(8.0)
                .items_center()
                .fill(if on { t.selection } else { t.panel_bg })
                .hover(t.selection)
                .on_press(move |s: &mut AppState| {
                    s.runs.ui.preset = i;
                    s.runs.ui.preset_pop = false;
                })
                .child(
                    text(p.to_string())
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text)
                        .grow(1.0),
                )
                .child(text(detail).font(FONT_PIN).color(t.text_dim)),
        );
    }
    let anchor = "runs-create-preset";
    let content = col()
        .key("runs-preset-content")
        .role("dialog")
        .label("Strategy preset")
        .focus_return(anchor)
        .min_w(400.0)
        .child(
            col()
                .key("body")
                .padding(4.0)
                .shrink()
                .min_h(0.0)
                .child(rune_kit::popover::fit_scroll(opts)),
        );
    layer.push(rune_kit::scrim("runs-preset-scrim", |s: &mut AppState| {
        s.runs.ui.preset_pop = false
    }));
    layer.push(rune_kit::popover::anchored(
        &kit,
        rune_kit::popover::raised(&kit, content, kit.radius.md),
        anchor,
        rune_kit::Placement::TopStart,
        4.0,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn counter_source() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv")
    }

    fn pump_until_idle(s: &mut AppState) {
        let start = std::time::Instant::now();
        loop {
            crate::runs::poll_runs(s);
            crate::runs::pump_queue(s);
            if !s.runs.is_active() && s.runs.queue.is_empty() {
                break;
            }
            assert!(start.elapsed().as_secs() < 120, "runs timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "helion-rune-runs-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn rt(s: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        let mut rt = Runtime::new(s, crate::view, spec.size, env);
        rt.frame(1.0 / 60.0);
        rt
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn create_runs_validates_names_and_sweeps_seeds() {
        let mut s = init();
        assert_eq!(parse_seeds(" 1, 2 3,2").unwrap(), vec![1, 2, 3]);
        assert!(parse_seeds("1, x").is_err());
        assert_eq!(
            name_blocker(&s, "impl_1").as_deref(),
            Some("A run named impl_1 already exists")
        );
        assert!(name_blocker(&s, "bad name").is_some());
        assert!(name_blocker(&s, " ").is_some());
        let names = create_runs(&mut s, "sw", "PhysOpt", &[1, 5]).unwrap();
        assert_eq!(names, vec!["sw_s1", "sw_s5"]);
        let d = &s.runs.defs[1];
        assert_eq!(d.strategy.placer_seed, 5);
        assert!(d.strategy.phys_opt, "preset knobs carry over");
        assert_eq!(strategy_label(&d.strategy), "PhysOpt · seed 5");
        assert!(
            create_runs(&mut s, "sw", "PhysOpt", &[1]).is_err(),
            "sw_s1 exists"
        );
        assert!(create_runs(&mut s, "x", "NoSuchPreset", &[]).is_err());
        assert_eq!(next_run_name(&s), "impl_2");
        let rows = run_rows(&s);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[2].status, RowStatus::NotStarted);
    }

    #[test]
    fn sorting_puts_missing_metrics_last_and_toggles() {
        let mut s = init();
        create_runs(&mut s, "a", "Default", &[]).unwrap();
        create_runs(&mut s, "b", "Default", &[]).unwrap();
        for (name, wns) in [("a", -50i64), ("b", 900), ("impl_1", 10)] {
            s.runs.history.push(RunRecord {
                name: name.into(),
                kind: RunKind::Impl,
                wns_ps: Some(wns),
                ..Default::default()
            });
        }
        toggle_sort(&mut s, RunCol::Wns);
        let names: Vec<_> = sorted_rows(&s).into_iter().map(|r| r.spec.name).collect();
        assert_eq!(names, vec!["a", "impl_1", "b", "synth_1"]);
        toggle_sort(&mut s, RunCol::Wns);
        let names: Vec<_> = sorted_rows(&s).into_iter().map(|r| r.spec.name).collect();
        assert_eq!(names[0], "b");
        s.console.tab = crate::views::console::ConsoleTab::DesignRuns;
        let mut r = rt(s);
        tap(&mut r, "runs-sort-name");
        assert_eq!(r.state.runs.ui.sort, Some((RunCol::Name, false)));
    }

    #[test]
    fn queue_runs_one_at_a_time_and_cancel_queue_drops_waiters() {
        let mut s = init();
        s.runs.source = Some(counter_source());
        s.runs.checkpoint_root = Some(temp_root("queue"));
        create_runs(&mut s, "q", "RuntimeOpt", &[1, 2]).unwrap();
        enqueue(&mut s, "q_s1").unwrap();
        assert_eq!(
            s.runs.active_name(),
            Some("q_s1"),
            "idle worker starts at once"
        );
        enqueue(&mut s, "q_s2").unwrap();
        assert!(enqueue(&mut s, "q_s2").is_err(), "no double queue");
        let rows = run_rows(&s);
        let st = |n: &str| rows.iter().find(|r| r.spec.name == n).unwrap().status;
        assert_eq!(st("q_s1"), RowStatus::Running);
        assert_eq!(st("q_s2"), RowStatus::Queued);
        assert_eq!(
            s.flow.impl_run,
            RunStatus::NotStarted,
            "custom runs leave impl_1 alone"
        );
        s.console.tab = crate::views::console::ConsoleTab::DesignRuns;
        let mut r = rt(s);
        tap(&mut r, "runs-cancel-queue");
        assert!(r.state.runs.queue.is_empty());
        crate::runs::cancel_run(&mut r.state);
        pump_until_idle(&mut r.state);
        assert!(
            r.state.runs.last_record("q_s2").is_none(),
            "dropped run never ran"
        );
    }

    #[test]
    fn seeded_run_records_metrics_writes_a_checkpoint_and_opens_it() {
        let mut s = init();
        s.runs.source = Some(counter_source());
        let root = temp_root("ckpt");
        s.runs.checkpoint_root = Some(root.clone());
        crate::tcl::eval_command(&mut s, "create_run seeded -strategy RuntimeOpt -seed 3");
        assert_eq!(s.runs.defs[0].strategy.placer_seed, 3);
        crate::tcl::eval_command(&mut s, "launch_runs seeded");
        assert_eq!(s.runs.active_name(), Some("seeded"));
        pump_until_idle(&mut s);
        let rec = s.runs.last_record("seeded").unwrap().clone();
        assert_eq!(rec.outcome, RunOutcome::Complete, "{:?}", s.runs.impl_log);
        assert!(rec.strategy.contains("seed=3"), "{}", rec.strategy);
        assert!(rec.luts.is_some() && rec.ffs.is_some() && rec.bram.is_some());
        assert!(
            s.runs
                .impl_log
                .iter()
                .any(|l| l.text == "place_design -seed 3")
        );
        assert_eq!(s.runs.active_result.as_deref(), Some("seeded"));
        let ck = checkpoints(&s);
        assert_eq!(ck.len(), 1, "routed checkpoint on disk");
        assert!(ck[0].1.starts_with(&root));
        s.console.tab = crate::views::console::ConsoleTab::DesignRuns;
        s.layout.console_h = 320.0;
        let mut r = rt(s);
        tap(&mut r, "runs-ckpt-0-open");
        assert!(
            r.state
                .console
                .lines
                .iter()
                .any(|l| l.text.starts_with("open_checkpoint ") && l.text.contains("lutff_sites=")),
            "{:?}",
            r.state
                .console
                .lines
                .iter()
                .rev()
                .take(3)
                .collect::<Vec<_>>()
        );
        assert!(r.state.tcl.session.placed.is_some());
        // An incremental run reuses the active run's checkpoint.
        let mut s = r.state;
        assert_eq!(
            active_checkpoint(&s).map(|c| c.0).as_deref(),
            Some("seeded")
        );
        let names = create_runs(&mut s, "incr", "Default", &[]).unwrap();
        let reference = active_checkpoint(&s).unwrap().1;
        set_incremental(&mut s, &names, &reference);
        assert!(run_rows(&s)[3].strategy.ends_with("· incremental"));
        enqueue(&mut s, "incr").unwrap();
        pump_until_idle(&mut s);
        let rec = s.runs.last_record("incr").unwrap().clone();
        assert_eq!(rec.outcome, RunOutcome::Complete, "{:?}", s.runs.impl_log);
        assert!(
            rec.reuse
                .as_deref()
                .is_some_and(|r| r.starts_with("reuse cells="))
        );
        let r = rt(s);
        assert!(r.find("runs-reuse-0").is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn make_active_reapplies_a_runs_results() {
        let mut s = init();
        let mut art = crate::runs::RunArtifacts::default();
        let mut t = crate::state::TimingState::default();
        t.wns_ps = Some(-777);
        t.has_run = true;
        art.timing = Some(t);
        create_runs(&mut s, "old", "Default", &[]).unwrap();
        s.runs.history.push(RunRecord {
            name: "old".into(),
            kind: RunKind::Impl,
            artifacts: Some(Box::new(art)),
            ..Default::default()
        });
        s.runs.active_result = Some("impl_1".into());
        s.runs.ui.selected = Some("old".into());
        s.console.tab = crate::views::console::ConsoleTab::DesignRuns;
        let mut r = rt(s);
        tap(&mut r, "runs-make-active");
        assert_eq!(r.state.runs.active_result.as_deref(), Some("old"));
        assert_eq!(r.state.timing.wns_ps, Some(-777));
        assert!(r.find("console-run-x-old-active").is_some());
        // Already active: the button now says why it is disabled.
        let e = r
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("runs-make-active"))
            .unwrap();
        assert!(!e.focusable && e.help.as_deref().unwrap_or("").contains("already"));
    }

    #[test]
    fn create_form_creates_a_sweep_from_the_preset_popover() {
        let mut s = init();
        s.console.tab = crate::views::console::ConsoleTab::DesignRuns;
        let mut r = rt(s);
        tap(&mut r, "runs-create-toggle");
        assert_eq!(r.state.runs.ui.name.text, "impl_2");
        tap(&mut r, "runs-create-preset");
        assert!(r.find("runs-preset-content").is_some());
        tap(&mut r, "runs-preset-3");
        assert_eq!(PRESETS[r.state.runs.ui.preset], "PhysOpt");
        r.state.runs.ui.seeds = TextEdit::new("7,8");
        r.invalidate();
        r.frame(1.0 / 60.0);
        tap(&mut r, "runs-create-go");
        let names: Vec<_> = r.state.runs.defs.iter().map(|d| d.name.clone()).collect();
        assert_eq!(names, vec!["impl_2_s7", "impl_2_s8"]);
        assert!(r.find("console-run-x-impl_2_s8").is_some());
        assert!(
            r.state.runs.queue.is_empty(),
            "Create alone does not launch"
        );
    }
}
