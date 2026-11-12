//! Interactive simulation debugger: persistent event-sim session plus the UG900
//! Breakpoints, Memory, Locals, and Force Constants panes.
//!
//! This ports the old interactive `sim_run` / `sim_step` / `sim_restart`
//! behavior, signal breakpoints, Memory arrays, Locals probes, and
//! force/deposit constants — but not the batch VCD testbench runner
//! (`views/wave.rs`), source-line breakpoints (Source gutter pass), or the
//! fabric kernel (fabric-debugging pass). The engine stays in UI state because
//! stepping, breakpoints, and forces are interactive; runs are bounded
//! synchronous evaluations over real `Sim`.
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

/// Which UG900 debugger pane the Debug surface shows. The old IDE spreads
/// these over separate workspace tabs; here they are tabs over one shared
/// transport (Run/Step/Restart) and one persistent event engine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DebugTab {
    #[default]
    Breakpoints,
    Memory,
    Locals,
    Forces,
    Source,
}

impl DebugTab {
    pub const ALL: [DebugTab; 5] = [
        DebugTab::Source,
        DebugTab::Breakpoints,
        DebugTab::Memory,
        DebugTab::Locals,
        DebugTab::Forces,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DebugTab::Breakpoints => "Breakpoints",
            DebugTab::Memory => "Memory",
            DebugTab::Locals => "Locals",
            DebugTab::Forces => "Forces",
            DebugTab::Source => "Source",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            DebugTab::Breakpoints => "debug-tab-breakpoints",
            DebugTab::Memory => "debug-tab-memory",
            DebugTab::Locals => "debug-tab-locals",
            DebugTab::Forces => "debug-tab-forces",
            DebugTab::Source => "debug-tab-source",
        }
    }

    /// The Tcl verb that opens this tab (tab taps evaluate real verbs).
    pub fn verb(self) -> &'static str {
        match self {
            DebugTab::Breakpoints => "breakpoints",
            DebugTab::Memory => "memory",
            DebugTab::Locals => "locals",
            DebugTab::Forces => "forces",
            DebugTab::Source => "source_window",
        }
    }
}

/// UG900 Memory window: LUT INIT / packed sequential word from helion-sim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryBlock {
    pub name: String,
    /// `lut_init` / `seq`.
    pub kind: String,
    pub width: u8,
    /// Addressable words (LUT INIT is 64×1).
    pub words: Vec<u64>,
}

impl MemoryBlock {
    pub fn depth(&self) -> usize {
        self.words.len().max(1)
    }

    pub fn type_cell(&self) -> &str {
        if self.kind.is_empty() {
            "memory"
        } else {
            self.kind.as_str()
        }
    }

    pub fn packed_word(&self) -> u64 {
        if self.width <= 1 {
            self.words
                .iter()
                .enumerate()
                .fold(0u64, |acc, (i, b)| acc | ((*b & 1) << i))
        } else {
            self.words.first().copied().unwrap_or(0)
        }
    }

    pub fn data_cell(&self) -> String {
        format!("{:#x}", self.packed_word())
    }

    /// Clickable-grid dump row: UG900 Memory Name/Type/Addr/Data.
    pub fn row_text(&self) -> String {
        format!(
            "NAME={} TYPE={} ADDR=0 DATA={} WIDTH={} DEPTH={}",
            self.name,
            self.type_cell(),
            self.data_cell(),
            self.width,
            self.depth()
        )
    }
}

/// One address of a selected UG900 Memory array.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryWordRow {
    pub addr: usize,
    pub data: String,
}

impl MemoryWordRow {
    pub fn row_text(&self) -> String {
        format!("ADDR={} DATA={}", self.addr, self.data)
    }
}

/// UG900 Locals window: current-scope sequential probes from helion-sim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalRow {
    pub name: String,
    pub kind: String,
    pub value: String,
    pub scope: String,
}

impl LocalRow {
    pub fn type_cell(&self) -> &str {
        if self.kind.is_empty() {
            "reg"
        } else {
            self.kind.as_str()
        }
    }

    pub fn value_cell(&self) -> &str {
        if self.value.is_empty() {
            "-"
        } else {
            self.value.as_str()
        }
    }

    pub fn scope_cell(&self) -> &str {
        if self.scope.is_empty() {
            "-"
        } else {
            self.scope.as_str()
        }
    }

    /// Clickable-grid dump row: UG900 Locals Name/Type/Value/Scope.
    pub fn row_text(&self) -> String {
        format!(
            "NAME={} TYPE={} VALUE={} SCOPE={}",
            self.name,
            self.type_cell(),
            self.value_cell(),
            self.scope_cell()
        )
    }
}

/// One clickable row in the UG900 Force Constants pane (helion-sim
/// force/deposit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForceRow {
    pub name: String,
    /// `force` (hold) / `deposit` (one-shot poke).
    pub kind: String,
    pub value: u64,
    /// `bin` / `hex` radix column.
    pub radix: String,
    pub start_ps: u64,
    /// 0 = no cancel (hold until `remove_force`).
    pub cancel_ps: u64,
    /// `Active` / `Pending` / `Released`.
    pub status: String,
}

impl ForceRow {
    pub fn kind_cell(&self) -> &str {
        if self.kind.is_empty() {
            "force"
        } else {
            self.kind.as_str()
        }
    }

    pub fn radix_cell(&self) -> &str {
        if self.radix.is_empty() {
            "bin"
        } else {
            self.radix.as_str()
        }
    }

    pub fn status_cell(&self) -> &str {
        if self.status.is_empty() {
            "Pending"
        } else {
            self.status.as_str()
        }
    }

    pub fn value_cell(&self) -> String {
        match self.radix_cell() {
            "hex" => format!("{:x}", self.value),
            _ => {
                if self.value == 0 {
                    "0".into()
                } else if self.value == 1 {
                    "1".into()
                } else {
                    format!("{:b}", self.value)
                }
            }
        }
    }

    /// Clickable-grid dump row: UG900 Name/Kind/Value/Start/Cancel/Status.
    pub fn row_text(&self) -> String {
        format!(
            "NAME={} KIND={} VALUE={} RADIX={} START_PS={} CANCEL_PS={} STATUS={}",
            self.name,
            self.kind_cell(),
            self.value_cell(),
            self.radix_cell(),
            self.start_ps,
            self.cancel_ps,
            self.status_cell()
        )
    }
}

/// One classified RTL row of the UG900 Source window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLine {
    /// 1-based file line.
    pub line: usize,
    pub text: String,
    /// `seq` / `assign` / `decl` / `comment` / `blank` / `other`.
    pub kind: String,
    /// Helion-sim probe this line drives, if any.
    pub signal: Option<String>,
    /// Gutter may arm a breakpoint (always_ff body / assign).
    pub executable: bool,
}

impl SourceLine {
    pub fn type_cell(&self) -> &str {
        if self.kind.is_empty() {
            "other"
        } else {
            self.kind.as_str()
        }
    }

    pub fn signal_cell(&self) -> &str {
        self.signal
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("-")
    }

    pub fn exec_cell(&self) -> &'static str {
        if self.executable {
            "1"
        } else {
            "0"
        }
    }

    pub fn bp_cell(&self, armed: bool) -> &'static str {
        if armed {
            "1"
        } else {
            "0"
        }
    }

    pub fn pc_cell(&self, pc: Option<usize>) -> &'static str {
        if pc == Some(self.line) {
            ">"
        } else {
            "-"
        }
    }

    /// Clickable-grid dump row: UG900 Source Line/BP/PC/Kind/Text.
    pub fn row_text(&self, armed: bool, pc: Option<usize>) -> String {
        format!(
            "LINE={} BP={} PC={} KIND={} EXEC={} SIGNAL={} TEXT={}",
            self.line,
            self.bp_cell(armed),
            self.pc_cell(pc),
            self.type_cell(),
            self.exec_cell(),
            self.signal_cell(),
            self.text.trim()
        )
    }
}

fn source_basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string()
}

/// RTL extensions the Source window accepts (old `is_rtl_source`).
pub fn is_rtl_source(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(ext.as_str(), "sv" | "v" | "vhd" | "vhdl")
}

/// Assigned identifier on a sequential/combinational statement
/// (`divclk <= ~divclk;` → `divclk`).
fn lhs_ident(t: &str) -> Option<String> {
    let s = t.trim();
    let s = if let Some(rest) = s.strip_prefix("assign") {
        if rest.starts_with(|c: char| c.is_whitespace()) {
            rest.trim()
        } else {
            s
        }
    } else {
        s
    };
    let lhs = if let Some((l, _)) = s.split_once("<=") {
        l
    } else if let Some((l, r)) = s.split_once('=') {
        if r.starts_with('=') {
            return None;
        }
        l
    } else {
        return None;
    };
    let skip = [
        "always_ff",
        "always",
        "posedge",
        "negedge",
        "clk",
        "begin",
        "assign",
        "if",
        "then",
        "rising_edge",
        "falling_edge",
        "not",
        "process",
        "logic",
        "wire",
        "reg",
        "std_logic",
        "signal",
    ];
    lhs.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|tok| {
            !tok.is_empty()
                && !skip.iter().any(|s| tok.eq_ignore_ascii_case(s))
                && !tok.chars().next().is_some_and(|c| c.is_ascii_digit())
        })
        .last()
        .map(|s| s.to_string())
}

/// Port / net / module name on a declaration line.
fn decl_ident(t: &str) -> Option<String> {
    let s = t.trim().trim_end_matches([';', ',', '(', ')']);
    let skip = [
        "input",
        "output",
        "inout",
        "logic",
        "wire",
        "reg",
        "signed",
        "unsigned",
        "module",
        "endmodule",
        "entity",
        "architecture",
        "signal",
        "port",
        "std_logic",
        "std_logic_vector",
        "parameter",
        "localparam",
        "genvar",
        "integer",
    ];
    s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|tok| {
            !tok.is_empty()
                && !skip.iter().any(|k| tok.eq_ignore_ascii_case(k))
                && !tok.chars().next().is_some_and(|c| c.is_ascii_digit())
        })
        .last()
        .map(|s| s.to_string())
}

/// Classify RTL into UG900 Source rows. Sequential `always_ff`/`process`
/// bodies are the helion-sim posedge PC; `assign` maps to a live probe.
pub fn classify_rtl_lines(src: &str) -> Vec<SourceLine> {
    let mut lines = Vec::new();
    let mut in_seq = false;
    for (i, raw) in src.lines().enumerate() {
        let line = i + 1;
        let t = raw.trim();
        let t_l = t.to_ascii_lowercase();
        let mut kind = "other".to_string();
        let mut executable = false;
        let mut signal = None;
        let is_seq_header = t_l.contains("always_ff")
            || t_l.contains("always @")
            || t_l.contains("always@")
            || t_l.starts_with("process")
            || t_l.contains(" process(")
            || t_l.contains(" process ");
        let is_seq_end = t_l == "end"
            || t_l == "end;"
            || t_l.starts_with("end;")
            || t_l.contains("end process")
            || t_l.contains("endprocess")
            || (t_l.starts_with("end ")
                && !t_l.starts_with("end if")
                && !t_l.starts_with("end case")
                && !t_l.starts_with("endmodule"));
        if t.is_empty() {
            kind = "blank".into();
        } else if t.starts_with("//") || t_l.starts_with("--") {
            kind = "comment".into();
        } else if is_seq_header {
            kind = "seq".into();
            signal = lhs_ident(t);
            executable = signal.is_some();
            in_seq = !executable;
        } else if in_seq {
            kind = "seq".into();
            if is_seq_end {
                in_seq = false;
            } else if t_l.starts_with("begin")
                || t_l.starts_with("if ")
                || t_l.starts_with("if(")
                || t_l == "else"
                || t_l.starts_with("else ")
            {
                executable = false;
            } else {
                signal = lhs_ident(t);
                executable = signal.is_some();
            }
        } else if t_l.starts_with("assign ") {
            kind = "assign".into();
            signal = lhs_ident(t);
            executable = true;
        } else if t.contains("<=") && !t_l.starts_with("if") {
            kind = "assign".into();
            signal = lhs_ident(t);
            executable = signal.is_some();
        } else if t_l.starts_with("module")
            || t_l.starts_with("endmodule")
            || t_l.starts_with("input")
            || t_l.starts_with("output")
            || t_l.starts_with("inout")
            || t_l.starts_with("logic")
            || t_l.starts_with("wire")
            || t_l.starts_with("reg")
            || t_l.starts_with("entity")
            || t_l.starts_with("architecture")
            || t_l.starts_with("end entity")
            || t_l.starts_with("end architecture")
            || t_l.starts_with("signal ")
            || t_l.starts_with("port")
            || t == ");"
            || t == "("
        {
            kind = "decl".into();
            signal = decl_ident(t);
        }
        lines.push(SourceLine {
            line,
            text: raw.to_string(),
            kind,
            signal,
            executable,
        });
    }
    lines
}

/// Debugger-owned UI state: the persistent event engine, the four UG900
/// panes over it, and breakpoint change tracking.
#[derive(Clone, Debug)]
pub struct DebugState {
    pub tab: DebugTab,
    pub spec: rune::TextEdit,
    pub breakpoints: Vec<Breakpoint>,
    pub selected: Option<usize>,
    pub memories: Vec<MemoryBlock>,
    pub selected_memory: Option<String>,
    pub selected_memory_addr: Option<usize>,
    pub locals: Vec<LocalRow>,
    pub selected_local: Option<String>,
    /// Locals scope filter (design top by default; instance scopes arrive
    /// with the wave/scopes pass).
    pub scope: Option<String>,
    pub forces: Vec<ForceRow>,
    pub selected_force: Option<String>,
    /// Classified RTL rows of the UG900 Source window.
    pub source_lines: Vec<SourceLine>,
    pub selected_source_line: Option<usize>,
    /// Helion-sim posedge PC (first executable `seq` line).
    pub pc_line: Option<usize>,
    /// Display name of the classified file (gutter markers only paint when
    /// the viewed tab names this file).
    pub source_file: Option<String>,
    /// (file, line count) stamp: markers hide when the viewed buffer no
    /// longer matches the classification instead of painting stale rows.
    pub source_stamp: Option<(String, usize)>,
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
            tab: DebugTab::Breakpoints,
            spec: rune::TextEdit::new(""),
            breakpoints: Vec::new(),
            selected: None,
            memories: Vec::new(),
            selected_memory: None,
            selected_memory_addr: None,
            locals: Vec::new(),
            selected_local: None,
            scope: None,
            forces: Vec::new(),
            selected_force: None,
            source_lines: Vec::new(),
            selected_source_line: None,
            pc_line: None,
            source_file: None,
            source_stamp: None,
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
        refresh_sim_debug(s);
        return Ok(());
    }
    let sim = Sim::new(&design);
    let text = elaborate_text(&design, &sim);
    s.debug.engine = Some(sim);
    s.debug.design_key = Some(key);
    s.debug.prev.clear();
    rearm_active_forces(&mut s.debug);
    refresh_sim_debug(s);
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
    rearm_active_forces(&mut s.debug);
    refresh_sim_debug(s);
    debug_log(s, &text, MsgSev::Info);
    Ok(())
}

/// Live helion-sim time in picoseconds (`ENGINE_TIME_PS`).
fn engine_time_ps(debug: &DebugState) -> u64 {
    debug.engine.as_ref().map(|sim| sim.time).unwrap_or(0)
}

/// Re-apply held forces to a fresh engine (old `rearm_active_forces`):
/// continuous holds with no start delay, never deposits or released rows.
fn rearm_active_forces(debug: &mut DebugState) {
    let Some(sim) = debug.engine.as_mut() else {
        return;
    };
    for row in &debug.forces {
        if row.kind != "deposit" && row.status != "Released" && row.start_ps == 0 {
            let _ = sim.force(&row.name, row.value);
        }
    }
}

/// Apply start/cancel-scheduled forces at the current engine time (old
/// `apply_scheduled_forces`). Called after every posedge: pending holds go
/// Active at `start_ps`, deposits fire once then Release, and holds Release
/// (with an engine `release`) once `cancel_ps` passes.
fn apply_scheduled_forces(debug: &mut DebugState) {
    let time = engine_time_ps(debug);
    let mut actions: Vec<(String, u64, bool, bool)> = Vec::new();
    // (name, value, hold, release)
    for row in &mut debug.forces {
        if row.status == "Released" {
            continue;
        }
        if time < row.start_ps {
            row.status = "Pending".into();
            continue;
        }
        if row.kind == "deposit" {
            row.status = "Released".into();
            actions.push((row.name.clone(), row.value, false, false));
        } else {
            row.status = "Active".into();
            actions.push((row.name.clone(), row.value, true, false));
            if row.cancel_ps > 0 && time >= row.cancel_ps {
                row.status = "Released".into();
                actions.push((row.name.clone(), row.value, true, true));
            }
        }
    }
    let Some(sim) = debug.engine.as_mut() else {
        return;
    };
    for (name, value, hold, release) in actions {
        if release {
            let _ = sim.release(&name);
        } else if hold {
            let _ = sim.force(&name, value);
        } else {
            let _ = sim.deposit(&name, value);
        }
    }
}

/// Insert or replace one Memory block by name (old `upsert_memory`).
fn upsert_memory(blocks: &mut Vec<MemoryBlock>, b: MemoryBlock) {
    if let Some(existing) = blocks.iter_mut().find(|e| e.name == b.name) {
        *existing = b;
    } else {
        blocks.push(b);
    }
}

/// Collect UG900 Memory arrays: LUT INIT ROMs plus the packed sequential
/// word, with the design's own LUT INITs as fallback when the engine yields
/// nothing (old `collect_memories`, event-kernel half).
fn collect_memories(sim: Option<&Sim>, design: Option<&helion_ir::Design>) -> Vec<MemoryBlock> {
    let mut blocks: Vec<MemoryBlock> = Vec::new();
    if let Some(sim) = sim {
        for m in sim.memory_cells() {
            upsert_memory(
                &mut blocks,
                MemoryBlock {
                    name: m.name,
                    kind: m.kind,
                    width: m.width,
                    words: m.words,
                },
            );
        }
        let regs: Vec<_> = sim
            .locals()
            .into_iter()
            .filter(|l| l.kind == "reg")
            .collect();
        if !regs.is_empty() {
            let mut seq = 0u64;
            for (i, l) in regs.iter().take(64).enumerate() {
                if l.value == "1" {
                    seq |= 1 << i;
                }
            }
            upsert_memory(
                &mut blocks,
                MemoryBlock {
                    name: "seq".into(),
                    kind: "seq".into(),
                    width: regs.len().min(64) as u8,
                    words: vec![seq],
                },
            );
        }
    }
    if blocks.is_empty() {
        if let Some(d) = design {
            for c in &d.cells {
                if let helion_ir::CellKind::Lut6 { init } = c.kind {
                    upsert_memory(
                        &mut blocks,
                        MemoryBlock {
                            name: helion_ir::resolve(c.name).to_string(),
                            kind: "lut_init".into(),
                            width: 1,
                            words: (0..64).map(|a| (init >> a) & 1).collect(),
                        },
                    );
                }
            }
        }
    }
    blocks.sort_by(|a, b| a.name.cmp(&b.name));
    blocks
}

/// Scope names for Locals filtering: the design top plus its instances
/// (old `prepare_sim` scope seeding, without the wave Objects side).
fn debug_scopes(design: Option<&helion_ir::Design>) -> (String, Vec<String>) {
    let Some(d) = design else {
        return ("-".into(), Vec::new());
    };
    let top = helion_ir::resolve(d.name).to_string();
    let instances = d
        .instances
        .iter()
        .map(|i| helion_ir::resolve(i.name).to_string())
        .collect();
    (top, instances)
}

/// Whether a probe belongs to a scope (old `object_in_scope`): flat
/// designs show everything at top; hierarchical designs keep child
/// sequential probes in their instance scope.
fn object_in_scope(name: &str, scope: &str, top: &str, instances: &[String]) -> bool {
    if instances.is_empty() {
        return true;
    }
    if scope == top {
        !instances
            .iter()
            .any(|i| name == *i || name.starts_with(&format!("{i}_")))
            && !name.starts_with("u_ff")
            && !name.starts_with("u_lut")
    } else {
        let pfx = format!("{scope}_");
        if name == scope || name.starts_with(&pfx) {
            return true;
        }
        // Flattened child: sequential probes live in the instance scope.
        (name.starts_with("u_ff") || name.starts_with("u_lut"))
            && !instances.iter().any(|i| name.starts_with(&format!("{i}_")))
    }
}

/// Refresh the Memory/Locals panes from the live engine, default the scope
/// to the design top, and prune selections that no longer exist (old
/// `refresh_sim_debug`, event-kernel half).
pub fn refresh_sim_debug(s: &mut AppState) {
    let design = live_design(s).cloned();
    let (top, instances) = debug_scopes(design.as_ref());
    if s.debug.scope.is_none() && top != "-" {
        s.debug.scope = Some(top.clone());
    }
    s.debug.memories = collect_memories(s.debug.engine.as_ref(), design.as_ref());
    if let Some(sel) = s.debug.selected_memory.clone() {
        if !s.debug.memories.iter().any(|m| m.name == sel) {
            s.debug.selected_memory = None;
            s.debug.selected_memory_addr = None;
        }
    }
    let scope = s.debug.scope.clone().unwrap_or_else(|| top.clone());
    let mut locals = Vec::new();
    if let Some(sim) = s.debug.engine.as_ref() {
        for l in sim.locals() {
            if object_in_scope(&l.name, &scope, &top, &instances) {
                locals.push(LocalRow {
                    name: l.name,
                    kind: l.kind,
                    value: l.value,
                    scope: scope.clone(),
                });
            }
        }
    }
    s.debug.locals = locals;
    if let Some(sel) = s.debug.selected_local.clone() {
        if !s.debug.locals.iter().any(|l| l.name == sel) {
            s.debug.selected_local = None;
        }
    }
    if let Some(sel) = s.debug.selected_force.clone() {
        if !s.debug.forces.iter().any(|r| r.name == sel) {
            s.debug.selected_force = None;
        }
    }
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

/// UG900 Memory table dump (Name/Type/Addr/Data), not a hex blob.
pub fn memories_text(s: &AppState) -> String {
    let n = s.debug.memories.len();
    let mut out = format!("memory n={n}");
    if n == 0 {
        out.push_str(" no memories - run simulation");
        return out;
    }
    for m in &s.debug.memories {
        out.push('\n');
        out.push_str(&m.row_text());
    }
    out
}

/// Selected UG900 Memory contents (Addr/Data).
pub fn memory_word_rows(s: &AppState) -> Vec<MemoryWordRow> {
    let Some(name) = s.debug.selected_memory.as_deref() else {
        return Vec::new();
    };
    let Some(m) = s.debug.memories.iter().find(|b| b.name == name) else {
        return Vec::new();
    };
    m.words
        .iter()
        .enumerate()
        .map(|(addr, v)| MemoryWordRow {
            addr,
            data: if m.width <= 1 {
                if *v != 0 { "1" } else { "0" }.into()
            } else {
                format!("{v:#x}")
            },
        })
        .collect()
}

pub fn memory_words_text(s: &AppState) -> String {
    let name = s.debug.selected_memory.as_deref().unwrap_or("-");
    let rows = memory_word_rows(s);
    let mut out = format!("memory_words n={} name={name}", rows.len());
    if rows.is_empty() {
        out.push_str(" no words - select_memory");
        return out;
    }
    for r in &rows {
        out.push('\n');
        out.push_str(&r.row_text());
    }
    out
}

/// Open the Memory tab, elaborating the engine first (old `open_memory`).
pub fn open_memory(s: &mut AppState) -> Result<String, String> {
    ensure_engine(s)?;
    s.debug.tab = DebugTab::Memory;
    Ok(memories_text(s))
}

/// UG900: click a Memory row (name or index) — contents follow the selection.
pub fn select_memory(s: &mut AppState, spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("select_memory: empty".into());
    }
    if s.debug.memories.is_empty() {
        ensure_engine(s)?;
    }
    if s.debug.memories.is_empty() {
        return Err("select_memory: no memories".into());
    }
    let idx = if let Ok(i) = spec.parse::<usize>() {
        if i >= s.debug.memories.len() {
            return Err(format!("select_memory: no row {spec}"));
        }
        i
    } else {
        s.debug
            .memories
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(spec))
            .ok_or_else(|| format!("select_memory: no memory {spec}"))?
    };
    let m = s.debug.memories[idx].clone();
    s.debug.selected_memory = Some(m.name.clone());
    s.debug.selected_memory_addr = None;
    s.debug.tab = DebugTab::Memory;
    Ok(format!("{}\n{}", m.row_text(), memory_words_text(s)))
}

/// UG900: click a Memory address — the detail readout follows the word.
pub fn select_memory_word(s: &mut AppState, spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("select_memory_word: empty".into());
    }
    if s.debug.selected_memory.is_none() {
        return Err("select_memory_word: select a memory first".into());
    }
    let rows = memory_word_rows(s);
    if rows.is_empty() {
        return Err("select_memory_word: no words".into());
    }
    let addr = spec
        .parse::<usize>()
        .or_else(|_| spec.strip_prefix("0x").unwrap_or(spec).parse::<usize>())
        .map_err(|_| format!("select_memory_word: bad addr {spec}"))?;
    let row = rows
        .iter()
        .find(|r| r.addr == addr)
        .ok_or_else(|| format!("select_memory_word: no addr {spec}"))?;
    s.debug.selected_memory_addr = Some(row.addr);
    let name = s.debug.selected_memory.clone().unwrap_or_default();
    s.debug.tab = DebugTab::Memory;
    Ok(format!("memory_word NAME={name} {}", row.row_text()))
}

/// UG900 Locals table dump (Name/Type/Value/Scope).
pub fn locals_text(s: &AppState) -> String {
    let n = s.debug.locals.len();
    let scope = s.debug.scope.as_deref().unwrap_or("-");
    let mut out = format!("locals n={n} scope={scope}");
    if n == 0 {
        out.push_str(" no locals - run simulation");
        return out;
    }
    for l in &s.debug.locals {
        out.push('\n');
        out.push_str(&l.row_text());
    }
    out
}

/// Open the Locals tab, elaborating the engine first (old `open_locals`).
pub fn open_locals(s: &mut AppState) -> Result<String, String> {
    ensure_engine(s)?;
    s.debug.tab = DebugTab::Locals;
    Ok(locals_text(s))
}

/// UG900: click a Locals row (name or index) — the detail readout follows.
pub fn select_local(s: &mut AppState, spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("select_local: empty".into());
    }
    if s.debug.locals.is_empty() {
        ensure_engine(s)?;
    }
    if s.debug.locals.is_empty() {
        return Err("select_local: no locals".into());
    }
    let idx = if let Ok(i) = spec.parse::<usize>() {
        if i >= s.debug.locals.len() {
            return Err(format!("select_local: no row {spec}"));
        }
        i
    } else {
        s.debug
            .locals
            .iter()
            .position(|l| l.name.eq_ignore_ascii_case(spec))
            .ok_or_else(|| format!("select_local: no local {spec}"))?
    };
    let l = s.debug.locals[idx].clone();
    s.debug.selected_local = Some(l.name.clone());
    s.debug.tab = DebugTab::Locals;
    Ok(l.row_text())
}

/// Which kernel the debugger engine runs (`event`, else `idle`). The old
/// IDE also reports `fabric`; that arrives with the fabric-debugging pass.
fn sim_kernel(debug: &DebugState) -> &'static str {
    if debug.engine.is_some() {
        "event"
    } else {
        "idle"
    }
}

/// UG900 Force Constants table dump (Name/Kind/Value/Start/Cancel/Status).
pub fn forces_text(s: &AppState) -> String {
    let n = s.debug.forces.len();
    let kernel = sim_kernel(&s.debug);
    let mut out = format!("forces n={n} kernel={kernel} engine=helion-sim");
    if n == 0 {
        out.push_str(" no forces - add_force");
        return out;
    }
    for r in &s.debug.forces {
        out.push('\n');
        out.push_str(&r.row_text());
    }
    out
}

/// Open the Forces tab (old `open_forces`; needs no engine).
pub fn open_forces(s: &mut AppState) -> Result<String, String> {
    s.debug.tab = DebugTab::Forces;
    Ok(forces_text(s))
}

/// Parse a force schedule time: raw ps, or `ns`/`us` suffixed (old
/// `parse_sim_ps`, including its bare error words).
fn parse_sim_ps(raw: &str) -> Result<u64, String> {
    let s = raw.trim().trim_matches('"').to_ascii_lowercase();
    if s.is_empty() {
        return Err("missing value".into());
    }
    let (num, mul) = if let Some(n) = s.strip_suffix("ns") {
        (n.trim(), 1_000u64)
    } else if let Some(n) = s.strip_suffix("us") {
        (n.trim(), 1_000_000u64)
    } else if let Some(n) = s.strip_suffix("ps") {
        (n.trim(), 1u64)
    } else {
        (s.as_str(), 1u64)
    };
    let v: u64 = num.parse().map_err(|_| format!("not a time: {raw}"))?;
    Ok(v.saturating_mul(mul).max(1))
}

/// Parse a force value: booleans, Verilog bit literals, `0x`/`0b`
/// prefixes, else radix-dependent (old `parse_force_value`).
fn parse_force_value(raw: &str, radix: &str) -> Result<u64, String> {
    let s = raw.trim().trim_matches('"').trim_matches('\'');
    if s.is_empty() {
        return Err("add_force: missing value".into());
    }
    if s.eq_ignore_ascii_case("true") || s == "1'b1" {
        return Ok(1);
    }
    if s.eq_ignore_ascii_case("false") || s == "1'b0" {
        return Ok(0);
    }
    let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"));
    if let Some(h) = hex {
        return u64::from_str_radix(h, 16).map_err(|_| format!("add_force: bad hex {raw}"));
    }
    if let Some(b) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
        return u64::from_str_radix(b, 2).map_err(|_| format!("add_force: bad bin {raw}"));
    }
    match radix {
        "hex" => u64::from_str_radix(s, 16).map_err(|_| format!("add_force: bad hex {raw}")),
        "bin" => {
            if s.chars().all(|c| c == '0' || c == '1') {
                u64::from_str_radix(s, 2).map_err(|_| format!("add_force: bad bin {raw}"))
            } else {
                s.parse::<u64>()
                    .map_err(|_| format!("add_force: bad value {raw}"))
            }
        }
        _ => s
            .parse::<u64>()
            .map_err(|_| format!("add_force: bad value {raw}")),
    }
}

/// Parse `[-deposit] <signal> <value> [-radix bin|hex]
/// [-start_time ps] [-cancel_after ps]` (old `parse_force_spec`).
fn parse_force_spec(spec: &str) -> Result<(bool, String, u64, String, u64, u64), String> {
    let mut toks: Vec<&str> = spec.split_whitespace().collect();
    if toks.is_empty() {
        return Err("add_force: need <signal> <value>".into());
    }
    let mut deposit = false;
    if toks[0].eq_ignore_ascii_case("-deposit") {
        deposit = true;
        toks.remove(0);
    }
    if toks.len() < 2 {
        return Err("add_force: need <signal> <value>".into());
    }
    let name = toks[0].trim_matches('{').trim_matches('}').to_string();
    if name.is_empty() {
        return Err("add_force: need <signal> <value>".into());
    }
    let mut radix = "bin".to_string();
    let mut start_ps = 0u64;
    let mut cancel_ps = 0u64;
    let mut i = 2;
    while i < toks.len() {
        let t = toks[i];
        if t.eq_ignore_ascii_case("-radix") {
            let r = toks.get(i + 1).ok_or("add_force: -radix needs bin|hex")?;
            radix = r.to_ascii_lowercase();
            if radix != "bin" && radix != "hex" {
                return Err(format!("add_force: bad radix {r}"));
            }
            i += 2;
        } else if t.eq_ignore_ascii_case("-start_time") || t.eq_ignore_ascii_case("-start_ps") {
            let v = toks.get(i + 1).ok_or("add_force: -start_time needs ps")?;
            start_ps = parse_sim_ps(v)?;
            i += 2;
        } else if t.eq_ignore_ascii_case("-cancel_after") || t.eq_ignore_ascii_case("-cancel_ps") {
            let v = toks.get(i + 1).ok_or("add_force: -cancel_after needs ps")?;
            cancel_ps = parse_sim_ps(v)?;
            i += 2;
        } else {
            return Err(format!("add_force: unknown flag {t}"));
        }
    }
    let value = parse_force_value(toks[1].trim_matches('{').trim_matches('}'), &radix)?;
    Ok((deposit, name, value, radix, start_ps, cancel_ps))
}

/// One-shot poke (old `add_deposit`): `deposit` without `-deposit` gains it.
pub fn add_deposit(s: &mut AppState, spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("deposit: need <signal> <value>".into());
    }
    if spec.to_ascii_lowercase().starts_with("-deposit") {
        add_force(s, spec)
    } else {
        add_force(s, &format!("-deposit {spec}"))
    }
}

/// Add (or replace) a force/deposit row, validating the signal against the
/// live engine and driving it at once unless scheduled for later (old
/// `add_force`, event-kernel half).
pub fn add_force(s: &mut AppState, spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("add_force: need <signal> <value>".into());
    }
    let (deposit, name, value, radix, start_ps, cancel_ps) = parse_force_spec(spec)?;
    ensure_engine(s)?;
    let sim = s
        .debug
        .engine
        .as_ref()
        .ok_or_else(|| "sim: no design (synth first)".to_string())?;
    if sim.signal_value(&name).is_none() {
        return Err(format!("add_force: no signal {name}"));
    }
    let kind = if deposit { "deposit" } else { "force" };
    let time = engine_time_ps(&s.debug);
    let status = if time < start_ps {
        "Pending"
    } else if deposit {
        "Released"
    } else {
        "Active"
    };
    if let Some(existing) = s
        .debug
        .forces
        .iter_mut()
        .find(|r| r.name.eq_ignore_ascii_case(&name))
    {
        existing.kind = kind.into();
        existing.value = value;
        existing.radix = radix.clone();
        existing.start_ps = start_ps;
        existing.cancel_ps = cancel_ps;
        existing.status = status.into();
    } else {
        s.debug.forces.push(ForceRow {
            name: name.clone(),
            kind: kind.into(),
            value,
            radix: radix.clone(),
            start_ps,
            cancel_ps,
            status: status.into(),
        });
    }
    if time >= start_ps {
        if let Some(sim) = s.debug.engine.as_mut() {
            if deposit {
                let _ = sim.deposit(&name, value);
            } else {
                let _ = sim.force(&name, value);
            }
        }
    }
    s.debug.selected_force = Some(name.clone());
    s.debug.tab = DebugTab::Forces;
    let row = s
        .debug
        .forces
        .iter()
        .find(|r| r.name.eq_ignore_ascii_case(&name))
        .cloned()
        .expect("just inserted");
    Ok(row.row_text())
}

/// Drop a force row (selection, index, or name) and release its engine hold
/// (old `remove_force`).
pub fn remove_force(s: &mut AppState, spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        let name = s
            .debug
            .selected_force
            .clone()
            .ok_or_else(|| "remove_force: need <signal>".to_string())?;
        return remove_force(s, &name);
    }
    if s.debug.forces.is_empty() {
        return Err("remove_force: no forces".into());
    }
    let idx = if let Ok(i) = spec.parse::<usize>() {
        if i >= s.debug.forces.len() {
            return Err(format!("remove_force: no row {spec}"));
        }
        i
    } else {
        s.debug
            .forces
            .iter()
            .position(|r| r.name.eq_ignore_ascii_case(spec))
            .ok_or_else(|| format!("remove_force: no force {spec}"))?
    };
    let row = s.debug.forces.remove(idx);
    if let Some(sim) = s.debug.engine.as_mut() {
        let _ = sim.release(&row.name);
    }
    if s.debug.selected_force.as_deref() == Some(row.name.as_str()) {
        s.debug.selected_force = None;
    }
    s.debug.tab = DebugTab::Forces;
    Ok(format!("remove_force {}", row.name))
}

/// Select a force row (index or name, never empty) for the detail readout
/// (old `select_force`).
pub fn select_force(s: &mut AppState, spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("select_force: missing name".into());
    }
    if s.debug.forces.is_empty() {
        return Err("select_force: no forces".into());
    }
    let idx = if let Ok(i) = spec.parse::<usize>() {
        if i >= s.debug.forces.len() {
            return Err(format!("select_force: no row {spec}"));
        }
        i
    } else {
        s.debug
            .forces
            .iter()
            .position(|r| r.name.eq_ignore_ascii_case(spec))
            .ok_or_else(|| format!("select_force: no row {spec}"))?
    };
    let row = s.debug.forces[idx].clone();
    s.debug.selected_force = Some(row.name.clone());
    s.debug.tab = DebugTab::Forces;
    Ok(row.row_text())
}

/// Resolve the debugged RTL: the run's source file from disk when it is
/// RTL (old `rtl_source_path`), else the active editor RTL buffer. Returns
/// the display name plus text.
fn debug_source_input(s: &mut AppState) -> Option<(String, String)> {
    crate::views::editor::flush_edit(s);
    if let Some(path) = s.runs.source.as_ref() {
        let display = path.to_string_lossy().to_string();
        if is_rtl_source(&display) {
            if let Ok(text) = std::fs::read_to_string(path) {
                return Some((source_basename(&display), text));
            }
        }
    }
    let (name, text) = crate::views::editor::active_buffer_text(s)?;
    if !is_rtl_source(&name) {
        return None;
    }
    Some((name, text))
}

/// Ensure classified Source rows, reclassifying when the resolved file or
/// buffer changed (old `ensure_rtl_source` plus a buffer stamp so edits
/// re-sync instead of going stale until save).
pub fn ensure_source_lines(s: &mut AppState) -> Result<String, String> {
    let (name, text) =
        debug_source_input(s).ok_or_else(|| "source: add RTL first".to_string())?;
    let stamp = (name.clone(), text.lines().count());
    if s.debug.source_stamp.as_ref() != Some(&stamp) {
        let lines = classify_rtl_lines(&text);
        if lines.is_empty() {
            return Err("source: empty file".into());
        }
        let sel = s.debug.selected_source_line;
        let pc = s.debug.pc_line;
        s.debug.source_lines = lines;
        s.debug.source_file = Some(name.clone());
        s.debug.source_stamp = Some(stamp);
        s.debug.selected_source_line =
            sel.filter(|l| *l <= s.debug.source_lines.len());
        s.debug.pc_line = pc;
    }
    Ok(s.debug.source_file.clone().unwrap_or_default())
}

/// Whether an enabled line breakpoint arms a Source line.
pub fn source_bp_armed(s: &AppState, line: usize) -> bool {
    s.debug
        .breakpoints
        .iter()
        .any(|b| b.line == Some(line) && b.enabled)
}

/// UG900 Source window dump (Line/BP/PC/Kind/Text).
pub fn source_window_text(s: &AppState) -> String {
    let n = s.debug.source_lines.len();
    let file = s.debug.source_file.as_deref().unwrap_or("-");
    let pc = s
        .debug
        .pc_line
        .map(|n| n.to_string())
        .unwrap_or_else(|| "-".into());
    let mut out = format!("source n={n} file={file} pc={pc}");
    if n == 0 {
        out.push_str(" no source - add RTL");
        return out;
    }
    for l in &s.debug.source_lines {
        out.push('\n');
        out.push_str(&l.row_text(source_bp_armed(s, l.line), s.debug.pc_line));
    }
    out
}

/// Open the Source view over the debugged RTL (old `open_source_window`).
pub fn open_source_window(s: &mut AppState) -> Result<String, String> {
    ensure_source_lines(s)?;
    Ok(source_window_text(s))
}

/// Select a Source line by number, parking the editor cursor there when
/// the viewed tab is the classified file (old `select_source_line` plus
/// editor cross-probing).
pub fn select_source_line(s: &mut AppState, spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("select_source_line: empty".into());
    }
    ensure_source_lines(s)?;
    if s.debug.source_lines.is_empty() {
        return Err("select_source_line: no source".into());
    }
    let idx = if let Ok(i) = spec.parse::<usize>() {
        s.debug
            .source_lines
            .iter()
            .position(|l| l.line == i)
            .or_else(|| (i < s.debug.source_lines.len()).then_some(i))
            .ok_or_else(|| format!("select_source_line: no line {spec}"))?
    } else {
        return Err(format!("select_source_line: bad line {spec}"));
    };
    let row = s.debug.source_lines[idx].clone();
    s.debug.selected_source_line = Some(row.line);
    let viewing = s
        .editor
        .tabs
        .get(s.editor.active)
        .is_some_and(|t| Some(t.as_str()) == s.debug.source_file.as_deref());
    if viewing {
        crate::views::editor::move_cursor_to(s, row.line.saturating_sub(1));
    }
    let armed = source_bp_armed(s, row.line);
    Ok(row.row_text(armed, s.debug.pc_line))
}

/// UG900 Source gutter: arm/disarm a line breakpoint tied to helion-sim.
/// `toggle_bp_line` and `add_bp_line` both toggle (old behavior); seq
/// lines break on the PC, assign lines on probe change.
pub fn toggle_source_breakpoint(s: &mut AppState, spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("toggle_bp_line: empty".into());
    }
    ensure_source_lines(s)?;
    let line = spec
        .rsplit_once(':')
        .map(|(_, n)| n)
        .unwrap_or(spec)
        .parse::<usize>()
        .map_err(|_| format!("toggle_bp_line: bad line {spec}"))?;
    let row = s
        .debug
        .source_lines
        .iter()
        .find(|l| l.line == line)
        .cloned()
        .ok_or_else(|| format!("toggle_bp_line: no line {line}"))?;
    if !row.executable {
        return Err(format!("toggle_bp_line: line {line} not executable"));
    }
    if let Some(id) = s
        .debug
        .breakpoints
        .iter()
        .find(|b| b.line == Some(line) && b.kind_cell() == "line")
        .map(|b| b.id)
    {
        s.debug.breakpoints.retain(|b| b.id != id);
        if s.debug.selected == Some(id) {
            s.debug.selected = None;
        }
        return Ok(format!("toggle_bp_line {line} off"));
    }
    let _ = ensure_engine(s);
    let id = s.debug.next_id;
    s.debug.next_id += 1;
    let signal = row.signal.clone().unwrap_or_default();
    let condition = if row.kind == "seq" {
        "line".into()
    } else {
        "change".into()
    };
    let file = s.debug.source_file.clone();
    s.debug.breakpoints.push(Breakpoint {
        id,
        enabled: true,
        signal,
        condition,
        value: None,
        hits: 0,
        kind: "line".into(),
        file,
        line: Some(line),
    });
    s.debug.selected = Some(id);
    s.debug.selected_source_line = Some(line);
    s.debug.tab = DebugTab::Breakpoints;
    Ok(s
        .debug
        .breakpoints
        .last()
        .expect("just pushed")
        .row_text())
}

/// Debugger markers for the editor gutter (1-based lines).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GutterMarkers {
    /// Lines carrying an enabled line breakpoint.
    pub armed: Vec<usize>,
    /// Lines carrying a disabled line breakpoint (hollow marker).
    pub disabled: Vec<usize>,
    /// Executable lines (where a gutter tap may arm a breakpoint).
    pub executable: Vec<usize>,
    /// Current helion-sim PC line.
    pub pc: Option<usize>,
}

impl GutterMarkers {
    pub fn is_armed(&self, line: usize) -> bool {
        self.armed.contains(&line)
    }
    pub fn is_disabled(&self, line: usize) -> bool {
        self.disabled.contains(&line)
    }
    pub fn is_executable(&self, line: usize) -> bool {
        self.executable.contains(&line)
    }
}

/// Gutter markers for the viewed editor tab, or `None` when that tab is not
/// the classified Source file (or its line count drifted since
/// classification), so stale breakpoint/PC markers never paint.
pub fn gutter_markers(s: &AppState) -> Option<GutterMarkers> {
    let (file, lines) = s.debug.source_stamp.as_ref()?;
    let tab = s.editor.tabs.get(s.editor.active)?;
    if tab != file {
        return None;
    }
    let viewed = s.editor.edit.text.lines().count();
    if s.editor.edit_tab == Some(s.editor.active) && viewed != *lines {
        return None;
    }
    let mut m = GutterMarkers {
        pc: s.debug.pc_line,
        ..GutterMarkers::default()
    };
    for b in &s.debug.breakpoints {
        let Some(line) = b.line else { continue };
        if b.kind_cell() != "line" {
            continue;
        }
        if b.enabled {
            m.armed.push(line);
        } else {
            m.disabled.push(line);
        }
    }
    m.executable = s
        .debug
        .source_lines
        .iter()
        .filter(|l| l.executable)
        .map(|l| l.line)
        .collect();
    Some(m)
}

/// Gutter breakpoint tap on 0-based editor line `n`: classifies the viewed
/// RTL when needed, then toggles the line breakpoint through the same path
/// as the `toggle_bp_line` verb (errors land in the console, not silently).
pub fn gutter_toggle(s: &mut AppState, n: usize) {
    let line = n + 1;
    let res = toggle_source_breakpoint(s, &line.to_string());
    match res {
        Ok(text) => debug_log(s, &text, MsgSev::Info),
        Err(e) => debug_log(s, &e, MsgSev::Warn),
    }
}

/// Park the sim PC on the first executable `seq` line (old
/// `advance_source_pc`): the helion-sim posedge all sequential lines share.
pub fn advance_source_pc(s: &mut AppState) {
    if s.debug.source_lines.is_empty() {
        let keep = s.debug.selected_source_line;
        let _ = ensure_source_lines(s);
        s.debug.selected_source_line = keep;
    }
    s.debug.pc_line = s
        .debug
        .source_lines
        .iter()
        .find(|l| l.executable && l.kind == "seq")
        .map(|l| l.line);
}

/// Add `signal [value]` (`add_breakpoint`), validating against the live event
/// engine and selecting the new row.
pub fn add_breakpoint(s: &mut AppState, spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("add_bp: need <signal> [value]".into());
    }
    ensure_engine(s)?;
    let mut parts = spec.split_whitespace();
    let signal = parts.next().expect("checked non-empty").to_string();
    let rest = parts.collect::<Vec<_>>().join(" ");
    let rest = rest.trim().trim_start_matches("==").trim();
    let value = if rest.is_empty() {
        None
    } else {
        let t = rest.trim_start_matches("0x").trim_start_matches("0X");
        let v = if rest.starts_with("0x") || rest.starts_with("0X") {
            u64::from_str_radix(t, 16)
        } else {
            rest.parse::<u64>()
        }
        .map_err(|_| format!("add_bp: bad value {rest}"))?;
        Some(v)
    };
    let sim = s
        .debug
        .engine
        .as_ref()
        .ok_or_else(|| "sim: no design (synth first)".to_string())?;
    sim.signal_value(&signal)
        .ok_or_else(|| format!("add_bp: no signal {signal}"))?;
    if let Some(existing) = s
        .debug
        .breakpoints
        .iter_mut()
        .find(|b| b.signal.eq_ignore_ascii_case(&signal) && b.value == value)
    {
        existing.enabled = true;
        let id = existing.id;
        s.debug.selected = Some(id);
        s.debug.tab = DebugTab::Breakpoints;
        return Ok(existing.row_text());
    }
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
    s.debug.tab = DebugTab::Breakpoints;
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
    s.debug.tab = DebugTab::Breakpoints;
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
    s.debug.tab = DebugTab::Breakpoints;
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
    s.debug.tab = DebugTab::Breakpoints;
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
    // The PC parks on the first executable `seq` line for the whole run
    // (old `sim_step_inner` re-parks it every cycle; idempotent).
    advance_source_pc(s);
    let (hit, ran, leds) = {
        let debug = &mut s.debug;
        if debug.engine.is_none() {
            return Err("sim: no design (synth first)".to_string());
        }
        let mut hit: Option<String> = None;
        let mut ran = 0u32;
        let mut leds = String::new();
        for _ in 0..cycles {
            {
                let sim = debug.engine.as_mut().expect("checked above");
                sim.step_posedge(ts);
            }
            // Scheduled forces land after the edge, before breakpoints and
            // the LED sample are evaluated (old `sim_step_inner` order).
            apply_scheduled_forces(debug);
            ran += 1;
            {
                let sim = debug.engine.as_ref().expect("checked above");
                leds.push(if sim.led { '1' } else { '0' });
            }
            let prev = debug.prev.clone();
            // Enabled rows in table order (duplicate signals stay independent).
            let order: Vec<usize> = debug
                .breakpoints
                .iter()
                .enumerate()
                .filter(|(_, b)| b.enabled)
                .map(|(i, _)| i)
                .collect();
            let curs: Vec<Option<u64>> = {
                let sim = debug.engine.as_ref().expect("checked above");
                order
                    .iter()
                    .map(|&i| sim.signal_value(&debug.breakpoints[i].signal))
                    .collect()
            };
            let pc = debug.pc_line;
            for (k, &i) in order.iter().enumerate() {
                let is_line = debug.breakpoints[i].kind_cell() == "line"
                    && debug.breakpoints[i].line.is_some();
                let signal = debug.breakpoints[i].signal.clone();
                let tripped = if is_line {
                    // Line breakpoints fire on the PC, else on probe change.
                    let line = debug.breakpoints[i].line.unwrap_or(0);
                    if pc == Some(line) {
                        true
                    } else if signal.is_empty() {
                        false
                    } else if let Some(cur) = curs[k] {
                        prev.get(&signal).is_some_and(|p| *p != cur)
                    } else {
                        false
                    }
                } else if let Some(cur) = curs[k] {
                    // Missing signals skip (never error mid-run); value
                    // breakpoints are edge-triggered, like the old core.
                    let value = debug.breakpoints[i].value;
                    match value {
                        Some(v) => cur == v && prev.get(&signal) != Some(&cur),
                        None => prev.get(&signal).is_some_and(|p| *p != cur),
                    }
                } else {
                    continue;
                };
                if tripped {
                    let b = &mut debug.breakpoints[i];
                    b.hits += 1;
                    let cur = curs[k].unwrap_or(0);
                    hit = Some(format!(
                        "HIT id={} SIGNAL={} VALUE={cur} CONDITION={} HITS={} CYCLE={ran} LINE={} KIND={}",
                        b.id,
                        b.signal_cell(),
                        b.condition,
                        b.hits,
                        b.line_cell(),
                        b.kind_cell()
                    ));
                    break;
                }
            }
            // Refresh change-tracking from the same post-force samples.
            let posts: Vec<(String, u64)> = {
                let sim = debug.engine.as_ref().expect("checked above");
                debug
                    .breakpoints
                    .iter()
                    .filter_map(|b| sim.signal_value(&b.signal).map(|v| (b.signal.clone(), v)))
                    .collect()
            };
            for (k, v) in posts {
                debug.prev.insert(k, v);
            }
            if hit.is_some() {
                break;
            }
        }
        (hit, ran, leds)
    };
    refresh_sim_debug(s);
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
/// not stop a step, matching the old debugger; scheduled forces and the
/// Memory/Locals panes follow the new state.
pub fn step_once(s: &mut AppState) -> Result<String, String> {
    ensure_engine(s)?;
    let ts = s.debug.timescale_ps.max(1);
    {
        let sim = s
            .debug
            .engine
            .as_mut()
            .ok_or_else(|| "sim: no design (synth first)".to_string())?;
        sim.step_posedge(ts);
    }
    apply_scheduled_forces(&mut s.debug);
    refresh_sim_debug(s);
    advance_source_pc(s);
    let cycle = s.debug.engine.as_ref().map(|sim| sim.cycle).unwrap_or(0);
    Ok(format!("sim_step t={}", cycle * 2))
}

/// Drop and re-elaborate the engine, zero breakpoint hits, and clear the
/// Memory/Locals selections (old `sim_restart`; forces survive, rearmed).
/// The batch/sim log is preserved; restart only appends its own marker.
pub fn restart(s: &mut AppState) -> Result<String, String> {
    s.debug.memories.clear();
    s.debug.selected_memory = None;
    s.debug.selected_memory_addr = None;
    s.debug.locals.clear();
    s.debug.selected_local = None;
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

/// Debugger tab strip: the four UG900 panes. Taps evaluate the real
/// open verb (`memory` elaborates the engine; a bare state flip would show
/// an empty pane).
fn debug_tab_strip(s: &AppState, t: &VivadoTheme) -> El {
    let mut strip = row()
        .key("debug-tabs")
        .role("tablist")
        .w_full()
        .h(28.0)
        .items_center()
        .gap(2.0)
        .px(4.0)
        .fill(t.header_bg)
        .border(1.0, t.border_light);
    for tab in DebugTab::ALL {
        let active = s.debug.tab == tab;
        let style = if active {
            VivadoTheme::FONT_TITLE
        } else {
            VivadoTheme::FONT_UI
        };
        let bg = if active { t.panel_bg } else { t.header_bg };
        let verb = tab.verb().to_string();
        strip = strip.child(
            el().key(tab.key())
                .role("button")
                .label(tab.label())
                .focusable(true)
                .h(24.0)
                .px(10.0)
                .center_y()
                .fill(bg)
                .hover(t.selection)
                .on_press(move |s: &mut AppState| crate::tcl::eval_command(s, &verb))
                .child(text(tab.label()).font(style).color(t.text)),
        );
    }
    strip
}

/// Shared debugger transport: Run/Step/Restart stay mounted on every tab
/// (the old panes repeat these buttons per workspace tab).
fn debug_toolbar(s: &AppState, t: &VivadoTheme) -> El {
    let run_label = format!("Run {}", s.debug.runtime_cycles.max(1));
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
            debug_btn("debug-run", &run_label, t, true)
                .help("Run RUNTIME_CYCLES cycles (Simulation Settings)")
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
}

/// Breakpoints recipe row: the old Add/Enable/Disable/Delete buttons.
/// Every button evaluates its real Tcl verb.
fn debug_bp_bar(s: &AppState, t: &VivadoTheme) -> El {
    let has_selection = s.debug.selected.is_some();
    row()
        .key("debug-bp-bar")
        .w_full()
        .h(28.0)
        .items_center()
        .gap(4.0)
        .px(8.0)
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
        .child(el().w(64.0).px(8.0).child(mono_cell(b.kind_cell(), t)))
        .child(el().w(64.0).px(8.0).child(mono_cell(&b.line_cell(), t)))
        .child(el().w(220.0).px(8.0).child(mono_cell(b.signal_cell(), t)))
        .child(el().w(120.0).px(8.0).child(mono_cell(&b.condition, t)))
        .child(
            el().w(64.0)
                .px(8.0)
                .child(mono_cell(&b.hits.to_string(), t)),
        )
}

/// Breakpoints tab body: manual spec plus the UG900 table over the
/// persistent event engine.
fn debug_breakpoints_body(s: &AppState, t: &VivadoTheme) -> El {
    let body = col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(debug_bp_bar(s, t))
        .child(debug_spec_row(s, t));
    if s.debug.breakpoints.is_empty() {
        return body.child(
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
            .child(el().w(64.0).px(8.0).child(ui_label("Type", t)))
            .child(el().w(64.0).px(8.0).child(ui_label("Line", t)))
            .child(el().w(220.0).px(8.0).child(ui_label("Signal", t)))
            .child(el().w(120.0).px(8.0).child(ui_label("Condition", t)))
            .child(el().w(64.0).px(8.0).child(ui_label("Hits", t))),
    );
    for b in &s.debug.breakpoints {
        list = list.child(debug_row(s, t, b));
    }
    body.child(list)
}

/// One Memory block row; selecting evaluates `select_memory <index>`.
fn debug_memory_row(s: &AppState, t: &VivadoTheme, i: usize, m: &MemoryBlock) -> El {
    let selected = s.debug.selected_memory.as_deref() == Some(m.name.as_str());
    row()
        .key(format!("debug-memory-row-{i}"))
        .role("button")
        .label(m.row_text())
        .focus_selected(selected)
        .focusable(true)
        .w_full()
        .h(ROW_H)
        .items_center()
        .fill(if selected { t.selection } else { t.panel_bg })
        .hover(t.selection)
        .on_press(move |s: &mut AppState| {
            crate::tcl::eval_command(s, &format!("select_memory {i}"));
        })
        .child(el().w(220.0).px(8.0).child(mono_cell(&m.name, t)))
        .child(el().w(96.0).px(8.0).child(mono_cell(m.type_cell(), t)))
        .child(el().w(160.0).px(8.0).child(mono_cell(&m.data_cell(), t)))
        .child(
            el().w(48.0)
                .px(8.0)
                .child(mono_cell(&m.width.to_string(), t)),
        )
        .child(
            el().w(64.0)
                .px(8.0)
                .child(mono_cell(&m.depth().to_string(), t)),
        )
}

/// One Memory contents row; selecting evaluates `select_memory_word <addr>`.
fn debug_memory_word_row(s: &AppState, t: &VivadoTheme, w: &MemoryWordRow) -> El {
    let selected = s.debug.selected_memory_addr == Some(w.addr);
    let addr = w.addr;
    row()
        .key(format!("debug-memory-word-{addr}"))
        .role("button")
        .label(w.row_text())
        .focus_selected(selected)
        .focusable(true)
        .w_full()
        .h(ROW_H)
        .items_center()
        .fill(if selected { t.selection } else { t.panel_bg })
        .hover(t.selection)
        .on_press(move |s: &mut AppState| {
            crate::tcl::eval_command(s, &format!("select_memory_word {addr}"));
        })
        .child(
            el().w(120.0)
                .px(8.0)
                .child(mono_cell(&format!("{addr:#x}"), t)),
        )
        .child(el().w(160.0).px(8.0).child(mono_cell(&w.data, t)))
}

/// Memory tab body: block table plus the selected block's Contents.
fn debug_memory_body(s: &AppState, t: &VivadoTheme) -> El {
    if s.debug.memories.is_empty() {
        return col()
            .key("debug-memory-empty")
            .w_full()
            .gap(6.0)
            .px(8.0)
            .py(8.0)
            .child(dim_label("No memories until you run simulation.", t));
    }
    let mut blocks = col()
        .key("debug-memory-table")
        .w_full()
        .grow(1.0)
        .min_h(80.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        });
    blocks = blocks.child(
        row()
            .w_full()
            .h(ROW_H)
            .items_center()
            .fill(t.header_bg)
            .child(el().w(220.0).px(8.0).child(ui_label("Name", t)))
            .child(el().w(96.0).px(8.0).child(ui_label("Type", t)))
            .child(el().w(160.0).px(8.0).child(ui_label("Data", t)))
            .child(el().w(48.0).px(8.0).child(ui_label("W", t)))
            .child(el().w(64.0).px(8.0).child(ui_label("D", t))),
    );
    for (i, m) in s.debug.memories.iter().enumerate() {
        blocks = blocks.child(debug_memory_row(s, t, i, m));
    }
    let mut body = col().w_full().grow(1.0).min_h(0.0).child(blocks);
    if let Some(sel) = s.debug.selected_memory.as_deref() {
        if let Some(m) = s.debug.memories.iter().find(|b| b.name == sel) {
            body = body.child(
                row()
                    .key("debug-memory-detail")
                    .w_full()
                    .h(ROW_H)
                    .items_center()
                    .px(8.0)
                    .child(mono_cell(&m.row_text(), t)),
            );
        }
    }
    body = body.child(
        row()
            .w_full()
            .h(ROW_H)
            .items_center()
            .px(8.0)
            .child(ui_label("Contents", t)),
    );
    let words = memory_word_rows(s);
    if words.is_empty() {
        return body.child(
            col()
                .key("debug-memory-words-empty")
                .w_full()
                .px(8.0)
                .py(4.0)
                .child(dim_label("Select a memory to view address and data.", t)),
        );
    }
    let mut list = col()
        .key("debug-memory-words")
        .w_full()
        .grow(1.0)
        .min_h(80.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        });
    for w in &words {
        list = list.child(debug_memory_word_row(s, t, w));
    }
    if let (Some(name), Some(addr)) = (
        s.debug.selected_memory.as_deref(),
        s.debug.selected_memory_addr,
    ) {
        if let Some(w) = words.iter().find(|r| r.addr == addr) {
            list = list.child(
                row()
                    .key("debug-memory-word-detail")
                    .w_full()
                    .h(ROW_H)
                    .items_center()
                    .px(8.0)
                    .child(mono_cell(
                        &format!("memory_word NAME={name} {}", w.row_text()),
                        t,
                    )),
            );
        }
    }
    body.child(list)
}

/// One Locals row; selecting evaluates `select_local <index>`.
fn debug_local_row(s: &AppState, t: &VivadoTheme, i: usize, l: &LocalRow) -> El {
    let selected = s.debug.selected_local.as_deref() == Some(l.name.as_str());
    row()
        .key(format!("debug-locals-row-{i}"))
        .role("button")
        .label(l.row_text())
        .focus_selected(selected)
        .focusable(true)
        .w_full()
        .h(ROW_H)
        .items_center()
        .fill(if selected { t.selection } else { t.panel_bg })
        .hover(t.selection)
        .on_press(move |s: &mut AppState| {
            crate::tcl::eval_command(s, &format!("select_local {i}"));
        })
        .child(el().w(220.0).px(8.0).child(mono_cell(&l.name, t)))
        .child(el().w(96.0).px(8.0).child(mono_cell(l.type_cell(), t)))
        .child(el().w(120.0).px(8.0).child(mono_cell(l.value_cell(), t)))
        .child(el().w(160.0).px(8.0).child(mono_cell(l.scope_cell(), t)))
}

/// Locals tab body: current-scope sequential probes plus the detail readout.
fn debug_locals_body(s: &AppState, t: &VivadoTheme) -> El {
    if s.debug.locals.is_empty() {
        return col()
            .key("debug-locals-empty")
            .w_full()
            .gap(6.0)
            .px(8.0)
            .py(8.0)
            .child(dim_label("No locals until you run simulation.", t));
    }
    let mut list = col()
        .key("debug-locals-table")
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
            .child(el().w(220.0).px(8.0).child(ui_label("Name", t)))
            .child(el().w(96.0).px(8.0).child(ui_label("Type", t)))
            .child(el().w(120.0).px(8.0).child(ui_label("Value", t)))
            .child(el().w(160.0).px(8.0).child(ui_label("Scope", t))),
    );
    for (i, l) in s.debug.locals.iter().enumerate() {
        list = list.child(debug_local_row(s, t, i, l));
    }
    if let Some(sel) = s.debug.selected_local.as_deref() {
        if let Some(l) = s.debug.locals.iter().find(|r| r.name == sel) {
            list = list.child(
                row()
                    .key("debug-locals-detail")
                    .w_full()
                    .h(ROW_H)
                    .items_center()
                    .px(8.0)
                    .child(mono_cell(&l.row_text(), t)),
            );
        }
    }
    list
}

/// Forces recipe row: the old Force/Deposit/Run buttons plus a
/// selection-gated Remove. Every button evaluates its real Tcl verb.
fn debug_forces_bar(s: &AppState, t: &VivadoTheme) -> El {
    let has_selection = s.debug.selected_force.is_some();
    row()
        .key("debug-forces-bar")
        .w_full()
        .h(28.0)
        .items_center()
        .gap(4.0)
        .px(8.0)
        .child(
            debug_btn("debug-force-led", "Force led 1", t, true).on_press(|s: &mut AppState| {
                crate::tcl::eval_command(s, "add_force led 1");
            }),
        )
        .child(
            debug_btn("debug-deposit-led", "Deposit led 1", t, true).on_press(
                |s: &mut AppState| {
                    crate::tcl::eval_command(s, "deposit led 1");
                },
            ),
        )
        .child(
            debug_btn("debug-forces-run", "Run", t, true)
                .on_press(|s: &mut AppState| crate::tcl::eval_command(s, "sim_run")),
        )
        .child(
            debug_btn("debug-force-remove", "Remove", t, has_selection).on_press(
                |s: &mut AppState| {
                    crate::tcl::eval_command(s, "remove_force");
                },
            ),
        )
}

/// One Forces row; selecting evaluates `select_force <index>`.
fn debug_force_row(s: &AppState, t: &VivadoTheme, i: usize, r: &ForceRow) -> El {
    let selected = s.debug.selected_force.as_deref() == Some(r.name.as_str());
    row()
        .key(format!("debug-force-row-{i}"))
        .role("button")
        .label(r.row_text())
        .focus_selected(selected)
        .focusable(true)
        .w_full()
        .h(ROW_H)
        .items_center()
        .fill(if selected { t.selection } else { t.panel_bg })
        .hover(t.selection)
        .on_press(move |s: &mut AppState| {
            crate::tcl::eval_command(s, &format!("select_force {i}"));
        })
        .child(el().w(160.0).px(8.0).child(mono_cell(&r.name, t)))
        .child(el().w(80.0).px(8.0).child(mono_cell(r.kind_cell(), t)))
        .child(el().w(96.0).px(8.0).child(mono_cell(&r.value_cell(), t)))
        .child(el().w(64.0).px(8.0).child(mono_cell(r.radix_cell(), t)))
        .child(
            el().w(96.0)
                .px(8.0)
                .child(mono_cell(&r.start_ps.to_string(), t)),
        )
        .child(
            el().w(96.0)
                .px(8.0)
                .child(mono_cell(&r.cancel_ps.to_string(), t)),
        )
        .child(el().w(96.0).px(8.0).child(mono_cell(r.status_cell(), t)))
}

/// Forces tab body: recipes plus the UG900 Force Constants table.
fn debug_forces_body(s: &AppState, t: &VivadoTheme) -> El {
    let body = col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(debug_forces_bar(s, t));
    if s.debug.forces.is_empty() {
        return body.child(
            col()
                .key("debug-forces-empty")
                .w_full()
                .gap(6.0)
                .px(8.0)
                .py(8.0)
                .child(dim_label("No force constants yet.", t))
                .child(row().w_full().items_center().child(
                    debug_btn("debug-forces-empty-add", "Force led 1", t, true).on_press(
                        |s: &mut AppState| {
                            crate::tcl::eval_command(s, "add_force led 1");
                        },
                    ),
                )),
        );
    }
    let mut list = col()
        .key("debug-forces-table")
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
            .child(el().w(160.0).px(8.0).child(ui_label("Name", t)))
            .child(el().w(80.0).px(8.0).child(ui_label("Kind", t)))
            .child(el().w(96.0).px(8.0).child(ui_label("Value", t)))
            .child(el().w(64.0).px(8.0).child(ui_label("Radix", t)))
            .child(el().w(96.0).px(8.0).child(ui_label("Start_ps", t)))
            .child(el().w(96.0).px(8.0).child(ui_label("Cancel_ps", t)))
            .child(el().w(96.0).px(8.0).child(ui_label("Status", t))),
    );
    for (i, r) in s.debug.forces.iter().enumerate() {
        list = list.child(debug_force_row(s, t, i, r));
    }
    if let Some(sel) = s.debug.selected_force.as_deref() {
        if let Some(r) = s.debug.forces.iter().find(|row| row.name == sel) {
            list = list.child(
                row()
                    .key("debug-forces-detail")
                    .w_full()
                    .h(ROW_H)
                    .items_center()
                    .px(8.0)
                    .child(mono_cell(&r.row_text(), t)),
            );
        }
    }
    body.child(list)
}

/// One Source row: line number, a BP slot (tap toggles a line breakpoint
/// on executable lines), the PC arrow, kind, and the RTL text. Selecting
/// evaluates `select_source_line`.
fn debug_source_row(s: &AppState, t: &VivadoTheme, l: &SourceLine) -> El {
    let line = l.line;
    let selected = s.debug.selected_source_line == Some(line);
    let armed = source_bp_armed(s, line);
    let has_disabled = !armed
        && s.debug
            .breakpoints
            .iter()
            .any(|b| b.line == Some(line) && b.kind_cell() == "line");
    let pc = s.debug.pc_line == Some(line);
    let mut bp = el()
        .key(format!("debug-source-bp-{line}"))
        .w(28.0)
        .h(ROW_H)
        .center();
    if l.executable {
        bp = bp
            .role("button")
            .label(format!("Toggle breakpoint line {line}"))
            .help("Toggle line breakpoint")
            .focusable(true)
            .hover(t.selection)
            .on_press(move |s: &mut AppState| {
                crate::tcl::eval_command(s, &format!("toggle_bp_line {line}"));
            });
    }
    let mono = VivadoTheme::FONT_MONO.mono();
    if armed {
        bp = bp.child(text("●").font(mono).color(t.fail));
    } else if has_disabled {
        bp = bp.child(text("○").font(mono).color(t.fail));
    } else if l.executable {
        bp = bp.child(text("·").font(mono).color(t.text_faint));
    }
    let code_color = if l.executable { t.text } else { t.text_dim };
    row()
        .key(format!("debug-source-row-{line}"))
        .w_full()
        .h(ROW_H)
        .items_center()
        .fill(if selected {
            t.selection
        } else if pc {
            t.current_line
        } else {
            t.panel_bg
        })
        .child(bp)
        .child(
            row()
                .role("button")
                .label(l.row_text(armed, s.debug.pc_line))
                .focus_selected(selected)
                .focusable(true)
                .grow(1.0)
                .min_w(0.0)
                .h(ROW_H)
                .items_center()
                .hover(t.selection)
                .on_press(move |s: &mut AppState| {
                    crate::tcl::eval_command(s, &format!("select_source_line {line}"));
                })
                .child(
                    el().w(48.0).pr(8.0).child(
                        text(line.to_string())
                            .font(mono)
                            .color(if pc { t.pass } else { t.text_faint }),
                    ),
                )
                .child(
                    el().w(24.0)
                        .child(text(if pc { "▶" } else { "" }).font(mono).color(t.pass)),
                )
                .child(el().w(64.0).px(4.0).child(dim_label(l.type_cell(), t)))
                .child(
                    el().grow(1.0).min_w(0.0).px(4.0).child(
                        text(l.text.trim_end())
                            .font(mono)
                            .color(code_color)
                            .truncate(),
                    ),
                ),
        )
}

/// Source tab: the UG900 Source window over the debugged RTL with line
/// breakpoints and the helion-sim PC, plus a jump into the editor.
fn debug_source_body(s: &AppState, t: &VivadoTheme) -> El {
    if s.debug.source_lines.is_empty() {
        return col()
            .key("debug-source-empty")
            .w_full()
            .gap(6.0)
            .px(8.0)
            .py(8.0)
            .child(dim_label(
                "No RTL loaded for debugging. Open a Verilog/SystemVerilog/VHDL source, then reopen this tab.",
                t,
            ))
            .child(row().w_full().items_center().child(
                debug_btn("debug-source-load", "Load Source", t, true).on_press(
                    |s: &mut AppState| {
                        crate::tcl::eval_command(s, "source_window");
                    },
                ),
            ));
    }
    let file = s.debug.source_file.clone().unwrap_or_default();
    let exec = s.debug.source_lines.iter().filter(|l| l.executable).count();
    let line_bps = s
        .debug
        .breakpoints
        .iter()
        .filter(|b| b.kind_cell() == "line")
        .count();
    let pc = s
        .debug
        .pc_line
        .map(|n| format!("PC line {n}"))
        .unwrap_or_else(|| "PC not started".into());
    let summary = format!(
        "{file}  ·  {} lines  ·  {exec} executable  ·  {line_bps} line breakpoint{}  ·  {pc}",
        s.debug.source_lines.len(),
        if line_bps == 1 { "" } else { "s" }
    );
    let open_file = file.clone();
    let bar = row()
        .key("debug-source-bar")
        .w_full()
        .h(28.0)
        .items_center()
        .gap(4.0)
        .px(8.0)
        .child(
            el().grow(1.0)
                .min_w(0.0)
                .child(dim_label(&summary, t).truncate()),
        )
        .child(
            debug_btn("debug-source-open", "Open in Editor", t, true).on_press(
                move |s: &mut AppState| {
                    let line = s.debug.selected_source_line.or(s.debug.pc_line);
                    if let Some(i) = s.editor.tabs.iter().position(|t| *t == open_file) {
                        crate::views::editor::select_tab(s, i);
                        s.center = crate::state::CenterPane::Editor;
                        if let Some(line) = line {
                            crate::views::editor::move_cursor_to(s, line.saturating_sub(1));
                        }
                    } else if let Some(path) = s.runs.source.clone()
                        && source_basename(&path.to_string_lossy()) == open_file
                        && let Ok(text) = std::fs::read_to_string(&path)
                    {
                        crate::views::editor::open_file_tab(s, open_file.clone(), text, path);
                        s.center = crate::state::CenterPane::Editor;
                        if let Some(line) = line {
                            crate::views::editor::move_cursor_to(s, line.saturating_sub(1));
                        }
                    }
                },
            ),
        );
    let mut list = col()
        .key("debug-source-table")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .child(
            row()
                .w_full()
                .h(ROW_H)
                .items_center()
                .fill(t.header_bg)
                .child(el().w(28.0).center().child(ui_label("BP", t)))
                .child(el().w(48.0).pr(8.0).child(ui_label("Line", t)))
                .child(el().w(24.0).child(ui_label("PC", t)))
                .child(el().w(64.0).px(4.0).child(ui_label("Kind", t)))
                .child(el().grow(1.0).px(4.0).child(ui_label("Code", t))),
        );
    for l in &s.debug.source_lines {
        list = list.child(debug_source_row(s, t, l));
    }
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(bar)
        .child(list)
}

/// Debug surface: tab strip plus shared transport plus the active UG900
/// pane (Breakpoints, Memory, Locals, or Force Constants).
pub fn debug_pane(s: &AppState, t: &VivadoTheme) -> El {
    let body = match s.debug.tab {
        DebugTab::Breakpoints => debug_breakpoints_body(s, t),
        DebugTab::Memory => debug_memory_body(s, t),
        DebugTab::Locals => debug_locals_body(s, t),
        DebugTab::Forces => debug_forces_body(s, t),
        DebugTab::Source => debug_source_body(s, t),
    };
    col()
        .key("debug")
        .role("application")
        .label("Debug")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.chrome_bg)
        .child(pane_header("Debug - sim_1", "debug-pane", t))
        .child(debug_tab_strip(s, t))
        .child(debug_toolbar(s, t))
        .child(
            row()
                .key("debug-status")
                .role("status")
                .w_full()
                .h(ROW_H)
                .items_center()
                .px(8.0)
                .child(dim_label(&s.debug.status(s), t)),
        )
        .child(body)
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
                .any(|l| l.starts_with("sim_run HIT id=1 SIGNAL=led VALUE=1")),
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

    #[test]
    fn debug_tabs_switch_panes_through_real_verbs() {
        let mut rt = harness_with(debug_state());
        rt.frame(0.0);
        assert!(rt.find("debug-tabs").is_some());
        for (key, tab) in [
            ("debug-tab-memory", DebugTab::Memory),
            ("debug-tab-locals", DebugTab::Locals),
            ("debug-tab-forces", DebugTab::Forces),
            ("debug-tab-breakpoints", DebugTab::Breakpoints),
        ] {
            tap(&mut rt, key);
            assert_eq!(rt.state.debug.tab, tab, "tap {key} must switch tabs");
        }
        // Tab taps elaborate through the open verbs, not bare state flips.
        assert!(
            rt.state.debug.engine.is_some(),
            "memory/locals tabs must elaborate the engine"
        );
        assert!(
            console_text(&rt)
                .iter()
                .any(|l| l.starts_with("elaborated ")),
            "elaboration must echo: {console:?}",
            console = console_text(&rt)
        );
        // The shared transport stays mounted on every tab.
        tap(&mut rt, "debug-tab-forces");
        assert!(rt.find("debug-toolbar").is_some());
        assert!(rt.find("debug-run").is_some());
        assert!(rt.find("debug-step").is_some());
        assert!(rt.find("debug-restart").is_some());
        assert!(rt.find("debug-status").is_some());
    }

    #[test]
    fn memory_tab_lists_blocks_and_selects_words() {
        let mut rt = harness_with(debug_state());
        rt.frame(0.0);
        tap(&mut rt, "debug-tab-memory");
        assert!(!rt.state.debug.memories.is_empty(), "counter has LUTs");
        assert!(rt.find("debug-memory-table").is_some());
        assert!(rt.find("debug-memory-row-0").is_some());
        assert!(rt.find("debug-memory-words-empty").is_some());
        // "seq" sorts first with a single word; exercise a 64-word LUT ROM.
        let lut_idx = rt
            .state
            .debug
            .memories
            .iter()
            .position(|m| m.kind == "lut_init")
            .expect("counter LUT ROM");
        tap(&mut rt, &format!("debug-memory-row-{lut_idx}"));
        let sel = rt.state.debug.selected_memory.clone().expect("select");
        assert!(rt.find("debug-memory-words").is_some());
        assert!(rt.find("debug-memory-word-0").is_some());
        assert!(rt.find("debug-memory-detail").is_some());
        assert!(
            console_text(&rt)
                .iter()
                .any(|l| l.contains(&format!("NAME={sel} TYPE="))),
            "selection must dump block + words: {console:?}",
            console = console_text(&rt)
        );
        tap(&mut rt, "debug-memory-word-1");
        assert_eq!(rt.state.debug.selected_memory_addr, Some(1));
        assert!(rt.find("debug-memory-word-detail").is_some());
    }

    #[test]
    fn locals_tab_follows_live_register_state() {
        let mut rt = harness_with(debug_state());
        rt.frame(0.0);
        tap(&mut rt, "debug-tab-locals");
        assert!(rt.state.debug.locals.len() >= 2, "led plus registers");
        assert_eq!(rt.state.debug.locals[0].name, "led");
        assert_eq!(rt.state.debug.locals[0].value, "0");
        assert!(rt.find("debug-locals-table").is_some());
        assert!(rt.find("debug-locals-row-0").is_some());
        crate::tcl::eval_command(&mut rt.state, "sim_run 8");
        rt.frame(1.0 / 60.0);
        assert_eq!(
            rt.state.debug.locals[0].value, "1",
            "locals must follow the engine (cnt[3] after 8 cycles)"
        );
        tap(&mut rt, "debug-locals-row-0");
        assert_eq!(rt.state.debug.selected_local.as_deref(), Some("led"));
        assert!(rt.find("debug-locals-detail").is_some());
    }

    #[test]
    fn forces_hold_and_release_real_signals() {
        let mut rt = harness_with(debug_state());
        rt.frame(0.0);
        tap(&mut rt, "debug-tab-forces");
        assert!(rt.find("debug-forces-empty").is_some());
        tap(&mut rt, "debug-force-led");
        assert_eq!(rt.state.debug.forces.len(), 1);
        assert_eq!(rt.state.debug.forces[0].status, "Active");
        assert!(rt.find("debug-forces-table").is_some());
        assert!(rt.find("debug-force-row-0").is_some());
        tap(&mut rt, "debug-run");
        assert!(
            console_text(&rt)
                .iter()
                .any(|l| l.contains("LED[16]=1111111111111111")),
            "a held led=1 must flatten the waveform: {console:?}",
            console = console_text(&rt)
        );
        tap(&mut rt, "debug-force-remove");
        assert!(rt.state.debug.forces.is_empty());
        assert!(rt.state.debug.selected_force.is_none());
        assert!(rt.find("debug-forces-empty").is_some());
        tap(&mut rt, "debug-run");
        assert!(
            console_text(&rt)
                .iter()
                .any(|l| l.contains(&format!("LED[16]={}", helion_sim::COUNTER_OVERLAY_SIM_LED))),
            "release must restore the gold waveform: {console:?}",
            console = console_text(&rt)
        );
    }

    #[test]
    fn deposit_pokes_once_then_releases() {
        let mut rt = harness_with(debug_state());
        rt.frame(0.0);
        tap(&mut rt, "debug-tab-forces");
        tap(&mut rt, "debug-deposit-led");
        assert_eq!(rt.state.debug.forces.len(), 1);
        assert_eq!(rt.state.debug.forces[0].kind, "deposit");
        assert_eq!(rt.state.debug.forces[0].status, "Released");
        let led_now = rt
            .state
            .debug
            .engine
            .as_ref()
            .and_then(|sim| sim.signal_value("led"));
        assert_eq!(led_now, Some(1), "deposit drives at once");
        tap(&mut rt, "debug-step");
        let led_after = rt
            .state
            .debug
            .engine
            .as_ref()
            .and_then(|sim| sim.signal_value("led"));
        assert_eq!(
            led_after,
            Some(0),
            "one step recomputes past a one-shot deposit"
        );
    }

    #[test]
    fn scheduled_forces_flip_pending_active_released() {
        let mut s = debug_state();
        s.center = crate::state::CenterPane::Debug;
        crate::tcl::eval_command(&mut s, "add_force led 1 -start_time 5000");
        assert_eq!(s.debug.forces[0].status, "Pending");
        assert_eq!(
            s.debug
                .engine
                .as_ref()
                .and_then(|sim| sim.signal_value("led")),
            Some(0),
            "pending forces must not drive yet"
        );
        crate::tcl::eval_command(&mut s, "sim_run 2");
        assert_eq!(s.debug.forces[0].status, "Active");
        crate::tcl::eval_command(&mut s, "remove_force led");
        crate::tcl::eval_command(&mut s, "add_force led 0 -cancel_after 5000");
        assert_eq!(s.debug.forces[0].status, "Active");
        crate::tcl::eval_command(&mut s, "sim_run 2");
        assert_eq!(
            s.debug.forces[0].status, "Released",
            "cancel time must release the hold"
        );
    }

    #[test]
    fn restart_clears_debug_tables_but_keeps_forces() {
        let mut rt = harness_with(debug_state());
        rt.frame(0.0);
        crate::tcl::eval_command(&mut rt.state, "memory");
        crate::tcl::eval_command(&mut rt.state, "select_memory 0");
        crate::tcl::eval_command(&mut rt.state, "select_local 0");
        crate::tcl::eval_command(&mut rt.state, "add_force led 1");
        assert!(!rt.state.debug.memories.is_empty());
        assert!(rt.state.debug.selected_memory.is_some());
        assert!(rt.state.debug.selected_local.is_some());
        crate::tcl::eval_command(&mut rt.state, "sim_restart");
        assert!(rt.state.debug.selected_memory.is_none());
        assert!(rt.state.debug.selected_memory_addr.is_none());
        assert!(rt.state.debug.selected_local.is_none());
        assert_eq!(
            rt.state.debug.forces.len(),
            1,
            "forces survive restart, rearmed"
        );
        assert!(!rt.state.debug.memories.is_empty(), "tables rebuild");
        assert!(!rt.state.debug.locals.is_empty(), "tables rebuild");
    }
}
