//! Small per-view state structs for the Helion Rune shell.
//!
//! Deliberately NOT a god-model: each view owns its slice (critique
//! do-not-repeat: `ide.rs:6965 IdeModel` in a 45k-line file). Data shapes
//! mirror the `helion-gui` domain types read-only:
//! - [`TimingState`] mirrors `helion_sta::TimingSummary` (`wns/tns/whs/ths`,
//!   failing endpoints) re-exported by `helion-gui/src/lib.rs`.
//! - [`ConsoleState`]/[`ConsoleLine`] mirror `ide.rs` `ConsoleLine` + message
//!   severities.
//! - [`VcdState`] mirrors `vcd.rs::VcdActivity` (timescale/duration/signals).
//! - Tcl verbs stay strings dispatched to the session; no Tcl paint code here.

/// Flow-run status. A real enum, never stringly (critique do-not-repeat).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunStatus {
    #[default]
    NotStarted,
    Running,
    Complete,
    Failed,
}

impl RunStatus {
    pub fn label(self) -> &'static str {
        match self {
            RunStatus::NotStarted => "Not started",
            RunStatus::Running => "Running",
            RunStatus::Complete => "Complete",
            RunStatus::Failed => "Failed",
        }
    }
}

/// Console / message severity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MsgSev {
    #[default]
    Info,
    Warn,
    Critical,
    Error,
}

/// Project Summary "Settings" card (Vivado brief: name/location/family/part).
#[derive(Clone, Debug, Default)]
pub struct ProjectState {
    pub name: String,
    pub location: String,
    pub family: String,
    pub part: String,
    /// `None` renders the "Not defined" link, never a sentinel string.
    pub top: Option<String>,
    pub target_lang: String,
    pub sim_lang: String,
}

/// Synthesis / Implementation / Simulation run cards.
#[derive(Clone, Debug, Default)]
pub struct FlowState {
    pub synth: RunStatus,
    pub synth_strategy: String,
    pub synth_reports: String,
    pub impl_run: RunStatus,
    pub impl_strategy: String,
    pub impl_reports: String,
    pub sim: RunStatus,
}

/// One Tcl console / log line.
#[derive(Clone, Debug)]
pub struct ConsoleLine {
    pub text: String,
    pub sev: MsgSev,
}

/// Bottom console pane: lines, the live input edit, and the dock tab state.
#[derive(Clone, Debug, Default)]
pub struct ConsoleState {
    pub lines: Vec<ConsoleLine>,
    pub input: rune::TextEdit,
    pub tab: crate::views::console::ConsoleTab,
    pub log_run: crate::views::console::LogRun,
    /// Up/Down recall cursor into [`crate::tcl::TclSession::history`]:
    /// `None` = live input, `Some(i)` = viewing `history[i]`. Any submit
    /// resets it to `None`.
    pub hist_pos: Option<usize>,
    /// The dock collapses to its tab strip so short windows give the
    /// center pane room; the toggle restores the transcript height.
    pub collapsed: bool,
    /// Text filter over the transcript / messages (toolbar search).
    pub filter: rune::TextEdit,
    pub filter_open: bool,
    /// Messages tab severity filter (`None` = every warning and error).
    pub msg_sev: Option<MsgSev>,
}

/// Demo constraints shown on `.xdc` tabs (seeded into
/// [`EditorState::xdc_lines`]; the Constraints wizard appends generated SDC).
pub fn default_xdc_lines() -> Vec<String> {
    [
        "# Clock: 100MHz on E3",
        "create_clock -period 10.000 -name clk [get_ports clk]",
        "set_property PACKAGE_PIN E3 [get_ports clk]",
        "set_property IOSTANDARD LVCMOS33 [get_ports clk]",
        "",
        "# LEDs and seven-segment display",
        "set_property PACKAGE_PIN H6 [get_ports {led[0]}]",
        "set_property IOSTANDARD LVCMOS33 [get_ports {led[0]}]",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Demo testbench shown on the `project1_tb.v` tab (seeded into
/// [`EditorState::tb_lines`]).
pub fn default_tb_lines() -> Vec<String> {
    [
        "`timescale 1ns / 1ps",
        "",
        "module project1_tb;",
        "    reg clk = 1'b0;",
        "    reg rst_n = 1'b0;",
        "    wire [15:0] led;",
        "    wire [7:0] seg;",
        "    wire [3:0] an;",
        "",
        "    project1_demo uut (",
        "        .clk(clk),",
        "        .led(led),",
        "        .seg(seg),",
        "        .an(an)",
        "    );",
        "",
        "    // 100MHz stimulus clock",
        "    always #5 clk = ~clk;",
        "",
        "    initial begin",
        "        #20 rst_n = 1'b1;",
        "        #200 $finish;",
        "    end",
        "endmodule",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Open editor tabs + cursor line (gutter/highlight in rune-05-editor).
#[derive(Clone, Debug, Default)]
pub struct EditorState {
    pub tabs: Vec<String>,
    pub active: usize,
    pub lines: Vec<String>,
    pub cursor_line: usize,
    /// Unsaved edits exist (toolbar/menu Save clears it).
    pub dirty: bool,
    /// Line-buffer snapshots for toolbar/menu undo/redo.
    pub undo_stack: Vec<Vec<String>>,
    pub redo_stack: Vec<Vec<String>>,
    /// Constraints buffer shown on `.xdc` tabs (Verilog lives in `lines`).
    pub xdc_lines: Vec<String>,
    /// Line-buffer snapshots for the XDC buffer.
    pub xdc_undo_stack: Vec<Vec<String>>,
    pub xdc_redo_stack: Vec<Vec<String>>,
    /// Testbench buffer shown on the `project1_tb.v` tab.
    pub tb_lines: Vec<String>,
    /// Line-buffer snapshots for the testbench buffer.
    pub tb_undo_stack: Vec<Vec<String>>,
    pub tb_redo_stack: Vec<Vec<String>>,
    /// Disk path per tab (Save writes the active buffer here; absent until
    /// the first save, which materializes a file under the temp dir).
    pub paths: std::collections::HashMap<String, std::path::PathBuf>,
    /// Find bar visibility (editor toolbar Find toggles it).
    pub find_open: bool,
    /// Find bar query (match stepping reads the active buffer).
    pub find_query: rune::TextEdit,
    /// Find bar replacement text (Replace / Replace All).
    pub replace_query: rune::TextEdit,
    /// Live text of the mirrored buffer: typing lands here, and tab
    /// switches/saves flush it back into the routed line buffer.
    pub edit: rune::TextEdit,
    /// Which tab `edit` mirrors (`None` = needs a load).
    pub edit_tab: Option<usize>,
    /// Which tab name currently owns each primary buffer (RTL, XDC/SDC,
    /// testbench). Other open files of the same kind are parked below, so
    /// two RTL tabs never share (and clobber) one buffer.
    pub owners: [Option<String>; 3],
    /// Lines + undo history of open tabs that don't own their buffer.
    pub parked: std::collections::HashMap<String, ParkedBuffer>,
}

/// A non-active file's buffer and undo history (see [`EditorState::owners`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParkedBuffer {
    pub lines: Vec<String>,
    pub undo: Vec<Vec<String>>,
    pub redo: Vec<Vec<String>>,
}

/// Testbench tabs route to the testbench buffer: simulation-fileset names
/// (`*_tb.v`, `tb_*.sv`, ...) that aren't constraints.
pub fn is_tb_name(name: &str) -> bool {
    crate::project::classify(name) == crate::project::FileSet::Simulation
}

impl EditorState {
    /// Snapshot the current buffer before a mutation; clears redo.
    pub fn push_undo(&mut self) {
        self.undo_stack.push(self.lines.clone());
        self.redo_stack.clear();
        self.dirty = true;
    }

    /// Restore the last snapshot; returns false when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.lines.clone());
            self.lines = prev;
            self.cursor_line = self.cursor_line.min(self.lines.len().saturating_sub(1));
            self.dirty = true;
            true
        } else {
            false
        }
    }

    /// Reapply an undone snapshot; returns false when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.lines.clone());
            self.lines = next;
            self.cursor_line = self.cursor_line.min(self.lines.len().saturating_sub(1));
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Snapshot the XDC buffer before a mutation; clears XDC redo.
    pub fn push_undo_xdc(&mut self) {
        self.xdc_undo_stack.push(self.xdc_lines.clone());
        self.xdc_redo_stack.clear();
        self.dirty = true;
    }

    /// Restore the last XDC snapshot; false when there is nothing to undo.
    pub fn undo_xdc(&mut self) -> bool {
        if let Some(prev) = self.xdc_undo_stack.pop() {
            self.xdc_redo_stack.push(self.xdc_lines.clone());
            self.xdc_lines = prev;
            self.cursor_line = self.cursor_line.min(self.xdc_lines.len().saturating_sub(1));
            self.dirty = true;
            true
        } else {
            false
        }
    }

    /// Reapply an undone XDC snapshot; false when there is nothing to redo.
    pub fn redo_xdc(&mut self) -> bool {
        if let Some(next) = self.xdc_redo_stack.pop() {
            self.xdc_undo_stack.push(self.xdc_lines.clone());
            self.xdc_lines = next;
            self.cursor_line = self.cursor_line.min(self.xdc_lines.len().saturating_sub(1));
            self.dirty = true;
            true
        } else {
            false
        }
    }

    /// Snapshot the testbench buffer before a mutation; clears TB redo.
    pub fn push_undo_tb(&mut self) {
        self.tb_undo_stack.push(self.tb_lines.clone());
        self.tb_redo_stack.clear();
        self.dirty = true;
    }

    /// Restore the last TB snapshot; false when there is nothing to undo.
    pub fn undo_tb(&mut self) -> bool {
        if let Some(prev) = self.tb_undo_stack.pop() {
            self.tb_redo_stack.push(self.tb_lines.clone());
            self.tb_lines = prev;
            self.cursor_line = self.cursor_line.min(self.tb_lines.len().saturating_sub(1));
            self.dirty = true;
            true
        } else {
            false
        }
    }

    /// Reapply an undone TB snapshot; false when there is nothing to redo.
    pub fn redo_tb(&mut self) -> bool {
        if let Some(next) = self.tb_redo_stack.pop() {
            self.tb_undo_stack.push(self.tb_lines.clone());
            self.tb_lines = next;
            self.cursor_line = self.cursor_line.min(self.tb_lines.len().saturating_sub(1));
            self.dirty = true;
            true
        } else {
            false
        }
    }

    /// Whether the active tab shows the testbench buffer (never indexes out
    /// of bounds). The `*_active` helpers route here first so menu/toolbar
    /// undo stay consistent with the displayed buffer without changing their
    /// call sites.
    pub fn active_is_tb(&self) -> bool {
        self.tabs
            .get(self.active)
            .is_some_and(|t| is_tb_name(t))
    }

    /// Snapshot the active buffer (`true` = XDC, `false` = Verilog; the TB
    /// tab routes to its own stacks regardless of the hint).
    pub fn push_undo_active(&mut self, is_xdc: bool) {
        if self.active_is_tb() {
            self.push_undo_tb();
        } else if is_xdc {
            self.push_undo_xdc();
        } else {
            self.push_undo();
        }
    }

    /// Undo on the active buffer; false when its stack is empty.
    pub fn undo_active(&mut self, is_xdc: bool) -> bool {
        if self.active_is_tb() {
            self.undo_tb()
        } else if is_xdc {
            self.undo_xdc()
        } else {
            self.undo()
        }
    }

    /// Redo on the active buffer; false when its stack is empty.
    pub fn redo_active(&mut self, is_xdc: bool) -> bool {
        if self.active_is_tb() {
            self.redo_tb()
        } else if is_xdc {
            self.redo_xdc()
        } else {
            self.redo()
        }
    }

    /// Whether the active buffer has something to undo.
    pub fn can_undo_active(&self, is_xdc: bool) -> bool {
        if self.active_is_tb() {
            !self.tb_undo_stack.is_empty()
        } else if is_xdc {
            !self.xdc_undo_stack.is_empty()
        } else {
            !self.undo_stack.is_empty()
        }
    }

    /// Whether the active buffer has something to redo.
    pub fn can_redo_active(&self, is_xdc: bool) -> bool {
        if self.active_is_tb() {
            !self.tb_redo_stack.is_empty()
        } else if is_xdc {
            !self.xdc_redo_stack.is_empty()
        } else {
            !self.redo_stack.is_empty()
        }
    }
}

/// One STA clock, mirroring `helion_sta::Clock` (name/period/source).
#[derive(Clone, Debug, Default)]
pub struct TimingClock {
    pub name: String,
    pub period_ps: u64,
    pub source: String,
    pub generated: bool,
    pub master: Option<String>,
}

/// Path-group class, mirroring `helion_sta::PathGroupKind`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimingGroupKind {
    #[default]
    Intra,
    Inter,
    Other,
}

impl TimingGroupKind {
    pub fn label(self) -> &'static str {
        match self {
            TimingGroupKind::Intra => "Intra-Clock",
            TimingGroupKind::Inter => "Inter-Clock",
            TimingGroupKind::Other => "Other",
        }
    }
}

/// One `report_timing_summary` group row, mirroring
/// `helion_sta::TimingSummaryGroup` (name/from/to/kind/slacks).
#[derive(Clone, Debug, Default)]
pub struct TimingGroupRow {
    pub name: String,
    pub from: String,
    pub to: String,
    pub kind: TimingGroupKind,
    pub wns_ps: Option<i64>,
    pub tns_ps: i64,
    /// Hold slacks of the group (`TimingSummaryGroup::whs_ps/ths_ps`).
    pub whs_ps: Option<i64>,
    pub ths_ps: i64,
    pub endpoints: usize,
}

/// STA path family: register-to-register or register-to-output, mirroring
/// the `group` field `extract_timing_paths` assigns (`ide.rs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimingPathGroup {
    #[default]
    Register,
    Output,
}

impl TimingPathGroup {
    pub fn label(self) -> &'static str {
        match self {
            TimingPathGroup::Register => "Intra-Clock",
            TimingPathGroup::Output => "Other Path Groups",
        }
    }

    /// Short family name for the paths table `Path group` column.
    pub fn short_label(self) -> &'static str {
        match self {
            TimingPathGroup::Register => "Register",
            TimingPathGroup::Output => "Output",
        }
    }
}

/// One UG903 pin-delay row, mirroring `ide.rs::TimingPathPin`
/// (cell/pin/delay-type/incr/path/net/fanout/location).
#[derive(Clone, Debug, Default)]
pub struct TimingPinRow {
    pub cell: String,
    pub pin: String,
    pub delay_type: String,
    pub incr_ps: i64,
    pub path_ps: i64,
    pub net: String,
    pub fanout: usize,
    pub location: String,
}

/// One STA endpoint path, mirroring `ide.rs::TimingPath`
/// (name/startpoint/endpoint/delay/slack/pins/group).
#[derive(Clone, Debug, Default)]
pub struct TimingPathRow {
    pub name: String,
    pub from: String,
    pub to: String,
    pub slack_ps: i64,
    pub delay_ps: i64,
    pub group: TimingPathGroup,
    pub pins: Vec<TimingPinRow>,
    /// STA clock launching the startpoint (`""` when no clock drives it).
    pub launch_clock: String,
    /// STA clock capturing the endpoint (`""` when no clock captures it).
    pub capture_clock: String,
}

impl TimingPathRow {
    /// Combinational + sequential cell arcs (not nets), mirroring
    /// `ide.rs::TimingPath::logic_ps`.
    pub fn logic_ps(&self) -> i64 {
        self.pins
            .iter()
            .filter(|p| matches!(p.delay_type.as_str(), "cell" | "setup" | "iob"))
            .map(|p| p.incr_ps)
            .sum()
    }

    /// Net / pin-load arcs, mirroring `ide.rs::TimingPath::net_ps`.
    pub fn net_ps(&self) -> i64 {
        self.pins
            .iter()
            .filter(|p| p.delay_type == "net")
            .map(|p| p.incr_ps)
            .sum()
    }

    /// Logic levels: combinational cells between the launch register and
    /// the endpoint (UG906 "Logic Levels"; the launch FF's own clock-to-Q
    /// arc is not a level).
    pub fn levels(&self) -> usize {
        let launch = self
            .pins
            .iter()
            .find(|p| p.delay_type == "clk")
            .map(|p| p.cell.as_str());
        let mut cells: Vec<&str> = Vec::new();
        for p in &self.pins {
            if p.delay_type == "cell"
                && Some(p.cell.as_str()) != launch
                && !cells.contains(&p.cell.as_str())
            {
                cells.push(p.cell.as_str());
            }
        }
        cells.len()
    }
}

/// One `report_clock_networks` tree, mirroring `helion_sta::ClockNetwork`
/// (loads/buffers/insertion).
#[derive(Clone, Debug, Default)]
pub struct ClockNetRow {
    pub name: String,
    pub period_ps: u64,
    /// Clock source object (`get_ports clk` target).
    pub source: String,
    pub net: String,
    pub loads: usize,
    pub buffers: usize,
    /// Net fanout (`ClockNetwork::fanout`).
    pub fanout: usize,
    pub insertion_ps: i64,
}

/// One `report_cdc` inter-clock row, mirroring `helion_sta::CdcViolation`.
#[derive(Clone, Debug, Default)]
pub struct CdcRow {
    pub from: String,
    pub to: String,
    pub severity: String,
    pub check: String,
    pub endpoints: usize,
    pub sync: bool,
    pub wns_ps: Option<i64>,
    /// UG949 relation of the pair (`ClockRelation::as_str`).
    pub relation: String,
}

/// UG949 clock-pair relation, mirroring `helion_sta::ClockRelation`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClockRel {
    #[default]
    Timed,
    TimedGenerated,
    TimedUnsafe,
    TimedDatapath,
    FalsePath,
    PartialFalsePath,
    Asynchronous,
    Exclusive,
    NoPaths,
}

/// Visual class of a clock-pair relation (the matrix legend).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockRelClass {
    /// Timed on a common clock: safe.
    Safe,
    /// Excepted (false path / asynchronous / exclusive): ignored by STA.
    Ignored,
    /// Partially excepted or datapath-only: review.
    Partial,
    /// Unrelated clocks timed without an exception: unsafe CDC.
    Unsafe,
    /// No timing paths between the pair.
    NoPaths,
}

impl ClockRel {
    pub fn label(self) -> &'static str {
        match self {
            ClockRel::Timed => "Timed",
            ClockRel::TimedGenerated => "Timed (generated)",
            ClockRel::TimedUnsafe => "Timed (unsafe)",
            ClockRel::TimedDatapath => "Timed (datapath)",
            ClockRel::FalsePath => "False Path",
            ClockRel::PartialFalsePath => "Partial False Path",
            ClockRel::Asynchronous => "Asynchronous",
            ClockRel::Exclusive => "Exclusive",
            ClockRel::NoPaths => "No Paths",
        }
    }

    pub fn class(self) -> ClockRelClass {
        match self {
            ClockRel::Timed | ClockRel::TimedGenerated => ClockRelClass::Safe,
            ClockRel::TimedUnsafe => ClockRelClass::Unsafe,
            ClockRel::TimedDatapath | ClockRel::PartialFalsePath => ClockRelClass::Partial,
            ClockRel::FalsePath | ClockRel::Asynchronous | ClockRel::Exclusive => {
                ClockRelClass::Ignored
            }
            ClockRel::NoPaths => ClockRelClass::NoPaths,
        }
    }
}

impl ClockRelClass {
    pub const ALL: [ClockRelClass; 5] = [
        ClockRelClass::Safe,
        ClockRelClass::Partial,
        ClockRelClass::Unsafe,
        ClockRelClass::Ignored,
        ClockRelClass::NoPaths,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ClockRelClass::Safe => "Timed (safe)",
            ClockRelClass::Ignored => "User ignored",
            ClockRelClass::Partial => "Partial / datapath",
            ClockRelClass::Unsafe => "Timed (unsafe)",
            ClockRelClass::NoPaths => "No paths",
        }
    }
}

/// One From x To cell of `report_clock_interaction`, mirroring
/// `helion_sta::ClockInteractionCell`.
#[derive(Clone, Debug, Default)]
pub struct ClockInteractionRow {
    pub from: String,
    pub to: String,
    pub relation: ClockRel,
    pub common_period_ps: u64,
    pub requirement_ps: i64,
    pub wns_ps: Option<i64>,
    pub path_count: usize,
}

/// One `report_methodology` check, mirroring
/// `helion_sta::MethodologyViolation`.
#[derive(Clone, Debug, Default)]
pub struct MethodRow {
    pub id: String,
    pub severity: String,
    pub category: String,
    pub objects: String,
    pub message: String,
}

/// Timing numbers in picoseconds, mirroring `helion_sta::TimingSummary`,
/// plus every other timing report result the backend can emit (clocks,
/// groups, paths with pin rows, clock networks, CDC, methodology, and the
/// raw `TimingResult` delay split). All vectors are empty until a real run
/// fills them; views render a disabled reason for empty sections.
#[derive(Clone, Debug, Default)]
pub struct TimingState {
    pub wns_ps: Option<i64>,
    pub tns_ps: i64,
    pub whs_ps: Option<i64>,
    pub ths_ps: i64,
    pub failing_endpoints: usize,
    /// Per-analysis failing endpoints, mirroring `TimingSummaryGroup`.
    pub failing_setup: usize,
    pub failing_hold: usize,
    /// Total endpoints per Setup/Hold column (`lab3-Fig9.png`).
    pub endpoints: usize,
    /// `None` until a run produces numbers (cards prompt to run first).
    pub has_run: bool,
    /// STA analysis clocks behind the summary (`TimingSummary.clocks`).
    pub clocks: Vec<TimingClock>,
    /// Intra/inter/other path groups (`TimingSummary.groups`).
    pub groups: Vec<TimingGroupRow>,
    /// Endpoint paths with UG903 pin rows (placed+routed runs only).
    pub paths: Vec<TimingPathRow>,
    /// Clock trees (`report_clock_networks`).
    pub clock_networks: Vec<ClockNetRow>,
    /// Inter-clock checks (`report_cdc`).
    pub cdc: Vec<CdcRow>,
    /// Constraint checks (`report_methodology`).
    pub methodology: Vec<MethodRow>,
    /// N x N clock-pair matrix (`report_clock_interaction`), row-major in
    /// clock order.
    pub clock_interaction: Vec<ClockInteractionRow>,
    /// Raw `TimingResult` delay split, picoseconds.
    pub r2r_ps: i64,
    pub iob_ps: i64,
    pub setup_ps: i64,
    pub hold_ps: i64,
    pub hold_slack_ps: i64,
    pub route_ps: i64,
    pub clk_net_ps: i64,
    /// Analyzed design name (`report_timing` header).
    pub design: String,
    /// Operating conditions behind the analysis (`set_operating_conditions`).
    pub voltage_mv: i64,
    pub temp_c: i64,
}

/// One hierarchy occupancy row, mirroring `ide.rs::HierOccupancy`
/// (name plus per-primitive counts).
#[derive(Clone, Debug, Default)]
pub struct HierRow {
    pub name: String,
    /// Child rows indent under the top module.
    pub child: bool,
    pub lut: u32,
    pub ff: u32,
    pub io: u32,
    pub bram: u32,
    pub dsp: u32,
}

/// Post-implementation resource usage (used/available per site class).
#[derive(Clone, Debug, Default)]
pub struct UtilState {
    pub luts_used: u32,
    pub luts_avail: u32,
    pub regs_used: u32,
    pub regs_avail: u32,
    pub bram_used: u32,
    pub bram_avail: u32,
    pub io_used: u32,
    pub io_avail: u32,
    pub dsp_used: u32,
    pub dsp_avail: u32,
    pub bufg_used: u32,
    pub bufg_avail: u32,
    /// Hierarchy occupancy from the elaborated design (empty until a run).
    pub hier: Vec<HierRow>,
    /// Design the occupancy was counted from.
    pub design: String,
}

/// Post-implementation on-chip power split, in milliwatts, plus the UG907
/// dynamic block rows in microwatts (mirroring `helion_sta::PowerReport`
/// `clocks/logic/signals/io/bram/dsp_uw`) and the Vivado thermal line.
#[derive(Clone, Debug, Default)]
pub struct PowerState {
    pub total_mw: u32,
    pub dynamic_mw: u32,
    pub static_mw: u32,
    /// Exact rails in microwatts (`PowerReport::total/dynamic/static_uw`);
    /// the `_mw` fields above are their whole-milliwatt summaries.
    pub total_uw: u32,
    pub dynamic_uw: u32,
    pub static_uw: u32,
    pub clocks_uw: u32,
    pub logic_uw: u32,
    pub signals_uw: u32,
    pub io_uw: u32,
    pub bram_uw: u32,
    pub dsp_uw: u32,
    /// Junction temperature in deci-°C (254 = 25.4 °C).
    pub junction_deci_c: u32,
    /// `report_power` header: part, PVT, and analysis frequency.
    pub part: String,
    pub voltage_mv: i64,
    pub temp_c: i64,
    pub f_mhz: i64,
    /// Estimate confidence derived from the run inputs (vectorless until a
    /// VCD annotates switching activity) plus its one-line reason.
    pub confidence: String,
    pub confidence_note: String,
}

/// DRC violation severity, mirroring `helion_drc::DrcSeverity`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DrcSev {
    Error,
    Warning,
    #[default]
    Advisory,
}

impl DrcSev {
    pub fn label(self) -> &'static str {
        match self {
            DrcSev::Error => "Error",
            DrcSev::Warning => "Warning",
            DrcSev::Advisory => "Advisory",
        }
    }
}

/// One DRC row, mirroring `helion_drc::DrcViolation` (`id`/severity/objects).
#[derive(Clone, Debug)]
pub struct DrcItem {
    pub id: String,
    pub severity: DrcSev,
    pub objects: String,
    pub message: String,
}

/// DRC report state: empty until a run reports violations.
#[derive(Clone, Debug, Default)]
pub struct DrcState {
    pub items: Vec<DrcItem>,
    /// Bitstream signoff gate (`check_bitstream_signoff`): `None` until an
    /// implementation run evaluates it.
    pub bitgen_allowed: Option<bool>,
    /// One-line `BitgenSignoffReport::summary_line` from the last run.
    pub signoff_summary: String,
    /// Unrouted nets and overused tiles behind the signoff verdict.
    pub unrouted: usize,
    pub overused: u32,
    /// True once a run evaluated DRC (even with zero violations).
    pub has_run: bool,
    /// The rows come from the pre-route deck (`check_placed`) on a placed,
    /// not yet routed design.
    pub pre_route: bool,
}

/// One configured frame of the bitstream, mirroring the old
/// `ide.rs::BitstreamFrame` (FAR-addressed 128-bit `helion_bits` frame).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FarRow {
    /// Encoded frame address ([`helion_device::Far::encode`]).
    pub far: u32,
    pub block: u8,
    pub die: u8,
    pub major: u16,
    pub minor: u8,
    pub word: u128,
}

impl FarRow {
    pub fn from_key(block: u8, major: u16, minor: u8, word: u128) -> Self {
        let far = helion_device::Far {
            block_type: block,
            die: 0,
            major,
            minor,
        };
        Self {
            far: far.encode(),
            block,
            die: far.die,
            major,
            minor,
            word,
        }
    }

    /// Block-type name (`helion_device::Far` block constants).
    pub fn block_name(&self) -> &'static str {
        match self.block {
            helion_device::Far::CLB_IO_CLK => "CLB_IO_CLK",
            helion_device::Far::DSP => "DSP",
            helion_device::Far::BRAM => "BRAM",
            helion_device::Far::IOB => "IOB",
            _ => "UNKNOWN",
        }
    }

    /// Set bits in the 128-bit frame word.
    pub fn ones(&self) -> u32 {
        self.word.count_ones()
    }

    pub fn far_hex(&self) -> String {
        format!("{:#010x}", self.far)
    }

    pub fn word_hex(&self) -> String {
        format!("{:#034x}", self.word)
    }
}

/// Bitstream result, mirroring the `write_bitstream` log line
/// (`ide.rs::run_step_inner`: frames/bytes/hash/configured) plus the real
/// `helion_bits::Bitstream` (frames + packets) behind it, so the Bitstream
/// FAR table and Program Device read the same artifact.
#[derive(Clone, Debug, Default)]
pub struct BitstreamState {
    pub idcode: u32,
    pub hash: u32,
    pub bytes: usize,
    pub frames: usize,
    pub configured: usize,
    /// True once a run wrote a bitstream.
    pub has_run: bool,
    /// Part the bitstream was generated for (`""` when unknown).
    pub part: String,
    /// Where the bitstream came from (`impl_1`, `Tcl session`).
    pub source: String,
    /// The real frames + packet stream (`None` until a run writes one).
    pub bits: Option<std::sync::Arc<helion_bits::Bitstream>>,
    /// Configured (non-zero) frames in FAR order, for the FAR table.
    pub rows: Vec<FarRow>,
    /// Selected FAR row (encoded FAR), if any.
    pub selected: Option<u32>,
}

impl BitstreamState {
    /// Fill every field from a real `helion_bits::Bitstream`.
    pub fn from_bits(bits: &helion_bits::Bitstream, hash: u32, part: &str, source: &str) -> Self {
        let rows: Vec<FarRow> = bits
            .frames
            .iter()
            .filter(|(_, w)| **w != 0)
            .map(|((block, major, minor), word)| FarRow::from_key(*block, *major, *minor, *word))
            .collect();
        Self {
            idcode: bits.idcode,
            hash,
            bytes: bits.packets.len(),
            frames: bits.frames.len(),
            configured: rows.len(),
            has_run: true,
            part: part.to_string(),
            source: source.to_string(),
            bits: Some(std::sync::Arc::new(bits.clone())),
            rows,
            selected: None,
        }
    }

    /// The selected FAR row, if it still exists.
    pub fn selected_row(&self) -> Option<&FarRow> {
        let far = self.selected?;
        self.rows.iter().find(|r| r.far == far)
    }
}

/// Waveform/VCD slice, mirroring `vcd.rs::VcdActivity` dimensions only.
#[derive(Clone, Debug, Default)]
pub struct VcdState {
    pub timescale_ps: u64,
    pub duration_ps: u64,
    pub signals: Vec<String>,
}

/// Toast severity. Typed so the status view picks ink without string tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToastKind {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

/// One notification in the bottom-right toast stack.
#[derive(Clone, Debug)]
pub struct ToastItem {
    /// Stable id so dismiss targets exactly one toast (no index juggling).
    pub id: u32,
    pub kind: ToastKind,
    pub title: String,
    pub detail: String,
    /// Poll-clock second the toast first showed (stamped by [`ToastState::tick`]).
    pub born: Option<f64>,
}

/// Toast stack state: append-only ids, dismiss-by-id, newest last.
#[derive(Clone, Debug, Default)]
pub struct ToastState {
    pub items: Vec<ToastItem>,
    next: u32,
}

/// Most toasts stacked at once (oldest drop first).
pub const TOAST_MAX: usize = 4;

impl ToastState {
    /// Show a notice. A repeat of a showing toast replaces it (fresh
    /// timer, no duplicate), and the stack keeps the newest [`TOAST_MAX`].
    pub fn push(&mut self, kind: ToastKind, title: &str, detail: &str) {
        self.items
            .retain(|t| !(t.kind == kind && t.title == title && t.detail == detail));
        let id = self.next;
        self.next += 1;
        self.items.push(ToastItem {
            id,
            kind,
            title: title.into(),
            detail: detail.into(),
            born: None,
        });
        if self.items.len() > TOAST_MAX {
            let drop = self.items.len() - TOAST_MAX;
            self.items.drain(..drop);
        }
    }

    /// Seconds a toast stays up: errors linger so they can be read.
    pub fn lifetime(kind: ToastKind) -> f64 {
        match kind {
            ToastKind::Error => 8.0,
            _ => 5.0,
        }
    }

    /// Stamp new toasts with the poll clock and drop expired ones. Returns
    /// (changed, seconds until the next expiry) so the poll can sleep
    /// instead of drawing every frame while idle.
    pub fn tick(&mut self, now: f64) -> (bool, Option<f64>) {
        let mut changed = false;
        for t in &mut self.items {
            if t.born.is_none() {
                t.born = Some(now);
            }
        }
        let before = self.items.len();
        self.items
            .retain(|t| now - t.born.unwrap_or(now) < Self::lifetime(t.kind));
        changed |= self.items.len() != before;
        let next = self
            .items
            .iter()
            .map(|t| Self::lifetime(t.kind) - (now - t.born.unwrap_or(now)))
            .fold(None, |acc: Option<f64>, d| Some(acc.map_or(d, |a| a.min(d))));
        (changed, next)
    }

    pub fn dismiss(&mut self, id: u32) {
        self.items.retain(|t| t.id != id);
    }
}

/// Root app state: composition of small view states + the hello toggle.
#[derive(Clone, Debug, Default)]
pub struct AppState {
    pub project: ProjectState,
    pub flow: FlowState,
    pub console: ConsoleState,
    pub editor: EditorState,
    pub timing: TimingState,
    pub util: UtilState,
    pub power: PowerState,
    pub vcd: VcdState,
    /// Hello-pattern switch (mirrors `examples/hello` `State::on`).
    pub hello_on: bool,
    /// Status-bar verb (`Ready`, `Synthesis Complete`, ...).
    pub status: StatusVerb,
    /// Menu/rail chrome UI state (open dropdown, collapsed sections).
    pub chrome: crate::views::chrome::ChromeState,
    /// Sources tree + properties card UI state (selection, tabs, Enabled).
    pub sources: crate::views::sources::SourcesState,
    /// Timing report-tree UI state (selected entry).
    pub timing_view: crate::views::timing::TimingView,
    /// Reports center-pane UI state (selected Utilization/Power/DRC entry).
    pub reports_view: crate::views::reports::ReportsView,
    /// Constraints center-pane UI state (selected constraint row).
    pub constraints_view: crate::views::constraints::ConstraintsView,
    /// DRC violations reported by the last run (empty = clean).
    pub drc: DrcState,
    /// Bitstream written by the last implementation run.
    pub bitstream: BitstreamState,
    /// Schematic canvas UI state (zoom, selection, hierarchy expand).
    pub schematic: crate::views::schematic::SchematicState,
    /// Device die canvas UI state (zoom, selection, layers, find).
    pub device: crate::views::device::DeviceState,
    /// Waveform viewer UI state (cursors, markers, scope, zoom, radix).
    pub wave: crate::views::wave::WaveState,
    /// Testbench runner state (discovery, runs, history, sim log).
    pub sim: crate::views::tb::SimState,
    /// Simulation Settings knobs + Compile/Elaborate results.
    pub sim_settings: crate::views::sim_settings::SimSettingsState,
    /// Logic Analyzer (simulation ILA): probes, trigger, capture, viewer.
    pub ila: crate::views::ila::IlaState,
    /// Interactive debugger UI state (persistent event engine, breakpoints).
    pub debug: crate::views::debug::DebugState,
    /// Hardware Manager UI state (cable, detect, program run, STAT).
    pub program: crate::views::program::ProgramState,
    /// IP Catalog + Block Design state (catalog, selection, BD, outputs).
    pub ip: crate::views::ip::IpState,
    /// Find Results state (query, scopes, selected hit).
    pub find: crate::views::find::FindState,
    /// Settings dialog + New Project / Constraints wizard UI state.
    pub dialogs: crate::views::dialogs::DialogState,
    /// Which center surface the shell shows (Summary dashboard or editor).
    pub center: CenterPane,
    /// Center-pane back/forward history for the toolbar nav buttons.
    pub nav_back: Vec<CenterPane>,
    pub nav_forward: Vec<CenterPane>,
    /// Bottom-right notification stack (kit `Toast` overlay).
    pub toasts: ToastState,
    /// Real engine runs: source override, per-run log buffers, live worker.
    pub runs: crate::runs::RunsState,
    /// Persistent Tcl session behind the `tcl>` input (vars, history, design).
    pub tcl: crate::tcl::TclSession,
    /// Theme choice: follow the OS (`cx.system_dark`) or a manual View-menu
    /// pin. Survives project reseed/empty (session chrome, not project data).
    pub theme: crate::theme::ThemeMode,
    /// Fuzzy command palette + shortcut cheatsheet overlay state.
    pub palette: crate::views::palette::PaletteState,
    /// Dock visibility/sizes and the active layout preset (session chrome).
    pub layout: crate::layout::LayoutState,
    /// Recently opened project/HDL paths, newest first (persisted).
    pub recent: Vec<String>,
    /// Files of the open project (Sources tree, compile order, palette).
    pub project_files: crate::project::ProjectFiles,
}

/// Center pane selector. Typed so scenes flip surfaces without strings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CenterPane {
    #[default]
    Summary,
    Editor,
    Timing,
    /// Vivado start page: vendor header + Quick Start/Tasks/Learning banners.
    Start,
    /// RTL schematic canvas: gates/wires, hierarchy, LOD, cross-probe.
    Schematic,
    /// Device die canvas: site tiles, placed cells, routed nets, find.
    Device,
    /// Simulation waveform: scopes/objects, traces, cursors, markers.
    Wave,
    /// Simulation testbenches: list plus run detail.
    Sim,
    /// Interactive debugger: breakpoints over the persistent event engine.
    Debug,
    /// Logic Analyzer: simulation ILA probes, trigger, capture viewer.
    Ila,
    /// Hardware Manager: open target, cable/target pickers, program + results.
    Program,
    /// Implementation reports: Utilization / Power / DRC detail bodies.
    Reports,
    /// IP Catalog + Block Design (IP Integrator canvas, address map, HDL).
    Ip,
    /// Bitstream: KPIs plus the FAR-addressed configuration frame table.
    Bitstream,
    /// Find Results over the live netlist (cells / ports / nets).
    Find,
    /// Timing constraints parsed from the project XDC: clocks, I/O delay,
    /// exceptions, I/O placement.
    Constraints,
}

/// Status-bar verb. Typed, with the exact Helion wording in one place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusVerb {
    #[default]
    Ready,
    SynthesisRunning,
    ImplementationRunning,
    SynthesisComplete,
    ImplementationComplete,
    Failed,
}

impl StatusVerb {
    pub fn label(self) -> &'static str {
        match self {
            StatusVerb::Ready => "Ready",
            StatusVerb::SynthesisRunning => "Synthesis Running",
            StatusVerb::ImplementationRunning => "Implementation Running",
            StatusVerb::SynthesisComplete => "Synthesis Complete",
            StatusVerb::ImplementationComplete => "Implementation Complete",
            StatusVerb::Failed => "Failed",
        }
    }
}

impl AppState {
    /// Vivado-like demo project matching the `rd-*.png` brief surfaces.
    pub fn demo() -> Self {
        let mut out = Self {
            project: ProjectState {
                name: "project_1".into(),
                location: "In memory (built-in demo)".into(),
                family: "Helion".into(),
                part: "HL10T-C32-1".into(),
                top: None,
                target_lang: "Verilog".into(),
                sim_lang: "Mixed".into(),
            },
            flow: FlowState {
                synth: RunStatus::NotStarted,
                synth_strategy: "Helion Synthesis Defaults".into(),
                synth_reports: "Helion Synthesis Default Reports".into(),
                impl_run: RunStatus::NotStarted,
                impl_strategy: "Default".into(),
                impl_reports: "Helion Implementation Default Reports".into(),
                sim: RunStatus::NotStarted,
            },
            console: ConsoleState {
                lines: vec![
                    ConsoleLine {
                        text: "INFO: [Common 17-17] Helion session started.".into(),
                        sev: MsgSev::Info,
                    },
                    ConsoleLine {
                        text: "INFO: Loaded the built-in demo project (in memory).".into(),
                        sev: MsgSev::Info,
                    },
                ],
                input: rune::TextEdit::new(""),
                tab: crate::views::console::ConsoleTab::Tcl,
                collapsed: false,
                filter: rune::TextEdit::new(""),
                filter_open: false,
                msg_sev: None,
                log_run: crate::views::console::LogRun::Synthesis,
                hist_pos: None,
            },
            editor: EditorState {
                tabs: vec!["project1_demo.v".into(), "project1.xdc".into()],
                active: 0,
                lines: vec![
                    "`timescale 1ns / 1ps".into(),
                    "".into(),
                    "module project1_demo(".into(),
                    "    input clk,".into(),
                    "    output [15:0] led,".into(),
                    "    output [7:0] seg,".into(),
                    "    output [3:0] an".into(),
                    ");".into(),
                    "".into(),
                    "reg [23:0] counter = 24'd0;".into(),
                    "reg divclk = 1'b0;".into(),
                    "reg [3:0] round_counter = 4'd0;".into(),
                    "reg [15:0] led_reg;".into(),
                    "reg [7:0] seg_reg;".into(),
                    "".into(),
                    "// Clock Divider: 100MHz -> 10Hz (100ms)".into(),
                    "always @(posedge clk)".into(),
                    "begin".into(),
                    "    if (counter == 24'd499999) begin".into(),
                    "        divclk <= ~divclk;".into(),
                    "        counter <= 24'd0;".into(),
                    "    end".into(),
                    "    else begin".into(),
                    "        divclk <= divclk;".into(),
                    "        counter <= counter + 1'd1;".into(),
                    "    end".into(),
                    "end".into(),
                    "".into(),
                    "always @(posedge divclk)".into(),
                ],
                cursor_line: 16,
                dirty: false,
                undo_stack: Vec::new(),
                redo_stack: Vec::new(),
                xdc_lines: default_xdc_lines(),
                xdc_undo_stack: Vec::new(),
                xdc_redo_stack: Vec::new(),
                tb_lines: default_tb_lines(),
                tb_undo_stack: Vec::new(),
                tb_redo_stack: Vec::new(),
                paths: std::collections::HashMap::new(),
                find_open: false,
                find_query: rune::TextEdit::new(""),
                replace_query: rune::TextEdit::new(""),
                edit: rune::TextEdit::new(""),
                edit_tab: None,
                owners: [None, None, None],
                parked: std::collections::HashMap::new(),
            },
            timing: TimingState::default(),
            util: UtilState::default(),
            power: PowerState::default(),
            bitstream: BitstreamState::default(),
            // No simulation has run: VCD dims stay empty until a real import.
            vcd: VcdState::default(),
            hello_on: false,
            status: StatusVerb::Ready,
            chrome: crate::views::chrome::ChromeState::default(),
            sources: crate::views::sources::SourcesState::default(),
            timing_view: crate::views::timing::TimingView::default(),
            reports_view: crate::views::reports::ReportsView::default(),
            constraints_view: crate::views::constraints::ConstraintsView::default(),
            drc: DrcState::default(),
            schematic: crate::views::schematic::SchematicState::default(),
            device: crate::views::device::DeviceState::default(),
            wave: crate::views::wave::WaveState::default(),
            sim: crate::views::tb::SimState::default(),
            sim_settings: crate::views::sim_settings::SimSettingsState::default(),
            debug: crate::views::debug::DebugState::default(),
            ila: crate::views::ila::IlaState::default(),
            program: crate::views::program::ProgramState::default(),
            ip: crate::views::ip::IpState::default(),
            find: crate::views::find::FindState::default(),
            dialogs: crate::views::dialogs::DialogState::default(),
            center: CenterPane::Summary,
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            toasts: ToastState::default(),
            runs: crate::runs::RunsState::default(),
            tcl: crate::tcl::TclSession::default(),
            theme: crate::theme::ThemeMode::default(),
            palette: crate::views::palette::PaletteState::default(),
            layout: crate::layout::LayoutState::default(),
            recent: Vec::new(),
            project_files: crate::project::ProjectFiles::default(),
        };
        out.project_files = crate::project::demo_files(&out);
        // Mirror the active demo buffer into the live edit field so the
        // first frame already types; caret parks on the cursor line.
        crate::views::editor::sync_edit(&mut out);
        out
    }

    /// Switch the center pane, recording history for toolbar back/forward.
    /// Navigating to the current pane is a no-op (no duplicate entries).
    pub fn navigate(&mut self, pane: CenterPane) {
        if self.center == pane {
            return;
        }
        self.nav_back.push(self.center);
        self.nav_forward.clear();
        self.center = pane;
    }

    /// Step back through center-pane history; false when there is none.
    pub fn go_back(&mut self) -> bool {
        if let Some(prev) = self.nav_back.pop() {
            self.nav_forward.push(self.center);
            self.center = prev;
            true
        } else {
            false
        }
    }

    /// Step forward through center-pane history; false when there is none.
    pub fn go_forward(&mut self) -> bool {
        if let Some(next) = self.nav_forward.pop() {
            self.nav_back.push(self.center);
            self.center = next;
            true
        } else {
            false
        }
    }

    /// Empty-project state for the start page: no open project name, no
    /// populated editor buffers, and session-startup console lines without
    /// the `open_project` verb. Sources/props panes key off the empty name
    /// and render their "no project" placeholder instead of the demo tree.
    /// Reseed session state for a freshly created project: apply the project
    /// identity, reset runs/status/reports/toasts/history. Editor buffers and
    /// console history stay (the demo sources tree is static until sources
    /// batch owns it; the console keeps the Tcl verb trail).
    pub fn reseed_for_new_project(
        &mut self,
        name: String,
        location: String,
        part: String,
        top: Option<String>,
    ) {
        self.project.name = name;
        self.project.location = location;
        self.project.part = part;
        self.project.top = top;
        self.flow.synth = RunStatus::NotStarted;
        self.flow.impl_run = RunStatus::NotStarted;
        self.flow.sim = RunStatus::NotStarted;
        self.status = StatusVerb::Ready;
        self.timing = TimingState::default();
        self.util = UtilState::default();
        self.power = PowerState::default();
        self.drc = DrcState::default();
        self.bitstream = BitstreamState::default();
        self.program.reset_results();
        self.ip = crate::views::ip::IpState::default();
        self.find.selected = None;
        self.toasts = ToastState::default();
        self.runs.reset();
        // A new project invalidates the sim log, the live VCD import, and
        // the VCD dims; wave view prefs (scope/zoom/radix) stay session-wide.
        self.sim.reset();
        // Settings knobs are session preferences; compile/elab results
        // belong to the old project's sources.
        self.sim_settings.modules = None;
        self.sim_settings.elab = None;
        self.sim_settings.editing = None;
        self.sim_settings.error = None;
        let (cycles, ts) = (self.debug.runtime_cycles, self.debug.timescale_ps);
        self.debug = crate::views::debug::DebugState::default();
        self.debug.runtime_cycles = cycles;
        self.debug.timescale_ps = ts;
        // The old design's probes and capture do not carry over.
        self.ila = crate::views::ila::IlaState::default();
        self.wave.live = None;
        self.wave.live_selected = None;
        self.wave.live_lanes.clear();
        self.wave.live_scope = None;
        self.wave.clamp_to_grid();
        self.vcd = VcdState::default();
        // A new project invalidates the elaborated design and its variables;
        // the command history is session-wide and stays.
        self.tcl.vars.clear();
        self.tcl.reset_design_state();
        self.nav_back.clear();
        self.nav_forward.clear();
    }

    pub fn empty_project(&mut self) {
        self.project.name.clear();
        self.project.top = None;
        self.editor.tabs.clear();
        self.editor.lines.clear();
        self.editor.cursor_line = 0;
        self.editor.dirty = false;
        self.editor.edit = rune::TextEdit::new("");
        self.editor.edit_tab = None;
        self.editor.undo_stack.clear();
        self.editor.redo_stack.clear();
        self.editor.xdc_lines.clear();
        self.editor.xdc_undo_stack.clear();
        self.editor.xdc_redo_stack.clear();
        self.editor.tb_lines.clear();
        self.editor.tb_undo_stack.clear();
        self.editor.tb_redo_stack.clear();
        self.editor.paths.clear();
        self.editor.owners = [None, None, None];
        self.project_files = crate::project::ProjectFiles::default();
        self.editor.parked.clear();
        self.editor.find_open = false;
        self.tcl.vars.clear();
        self.tcl.reset_design_state();
        self.console.lines = vec![ConsoleLine {
            text: "INFO: [Common 17-17] Helion session started.".into(),
            sev: MsgSev::Info,
        }];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_verbs_use_exact_helion_wording() {
        assert_eq!(StatusVerb::Ready.label(), "Ready");
        assert_eq!(StatusVerb::SynthesisRunning.label(), "Synthesis Running");
        assert_eq!(
            StatusVerb::ImplementationRunning.label(),
            "Implementation Running"
        );
        assert_eq!(StatusVerb::SynthesisComplete.label(), "Synthesis Complete");
        assert_eq!(
            StatusVerb::ImplementationComplete.label(),
            "Implementation Complete"
        );
        assert_eq!(StatusVerb::Failed.label(), "Failed");
    }

    #[test]
    fn run_status_labels_are_sentence_case() {
        assert_eq!(RunStatus::NotStarted.label(), "Not started");
        assert_eq!(RunStatus::Running.label(), "Running");
        assert_eq!(RunStatus::Complete.label(), "Complete");
        assert_eq!(RunStatus::Failed.label(), "Failed");
    }

    #[test]
    fn toast_dismiss_removes_only_the_target() {
        let mut t = ToastState::default();
        t.push(ToastKind::Success, "Synthesis complete", "0 Errors");
        t.push(ToastKind::Error, "Implementation failed", "2 Errors");
        t.dismiss(0);
        assert_eq!(t.items.len(), 1);
        assert_eq!(t.items[0].id, 1);
        assert_eq!(t.items[0].kind, ToastKind::Error);
    }

    #[test]
    fn demo_project_matches_brief() {
        let s = AppState::demo();
        assert_eq!(s.project.name, "project_1");
        assert_eq!(s.project.part, "HL10T-C32-1");
        assert_eq!(s.project.top, None);
        assert_eq!(s.flow.synth_strategy, "Helion Synthesis Defaults");
        assert_eq!(s.flow.impl_strategy, "Default");
        assert!(
            s.console
                .lines
                .first()
                .is_some_and(|l| l.text.contains("Helion session started")),
            "console must open on session startup, not exit"
        );
        assert!(!s.timing.has_run);
    }

    #[test]
    fn empty_project_clears_name_and_editor_buffers() {
        let mut s = AppState::demo();
        s.empty_project();
        assert!(s.project.name.is_empty());
        assert!(s.editor.tabs.is_empty());
        assert!(s.editor.lines.is_empty());
        assert_eq!(s.project.part, "HL10T-C32-1", "default part stays");
        assert!(
            !s.console
                .lines
                .iter()
                .any(|l| l.text.starts_with("open_project")),
            "empty project must not show an open_project verb"
        );
    }
}
