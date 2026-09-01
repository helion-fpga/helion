//! Headless IDE model — the Vivado-class application state.
//!
//! Sources/netlist tree, Tcl console, flow rail (Synthesis → Opt → Place → Route →
//! Bitstream) and the timing/utilization report panes. Everything here runs the *real*
//! [`Session`] engines: there is no canned output anywhere in this file, so the widget
//! layer (`helion-ide`) is a thin painter over this model and can be tested without a
//! display.

use crate::{tcl_eval, GpuiShell};
use helion_bd::{emit_sv, validate, BlockDesign};
use helion_debug::{insert_arm_capture, IlaCapture};
use helion_device::{Device, SiteKind};
use helion_drc::{check_placed, check_routed, Drc};
use helion_fabric::{Fabric, Stat};
use helion_ir::{CellKind, Design, PortDir};
use helion_ipxact::{pack_gpio, pack_uart, IpCore};
use helion_proj::{get_cells, get_nets, Mode, Session};
use helion_sim::Sim;
use helion_sta::{
    create_clock, load_xdc, report_timing_routed_xdc, Constraints, TimingResult,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

/// One stage of the implementation rail. Order is the dependency order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FlowStep {
    Synthesis,
    Opt,
    Place,
    Route,
    Bitstream,
}

impl FlowStep {
    pub const ALL: [FlowStep; 5] = [
        FlowStep::Synthesis,
        FlowStep::Opt,
        FlowStep::Place,
        FlowStep::Route,
        FlowStep::Bitstream,
    ];

    /// Rail label as painted in the GUI.
    pub fn label(self) -> &'static str {
        match self {
            FlowStep::Synthesis => "Synthesis",
            FlowStep::Opt => "Opt",
            FlowStep::Place => "Place",
            FlowStep::Route => "Route",
            FlowStep::Bitstream => "Bitstream",
        }
    }

    /// The Tcl command the step journals into the console.
    pub fn tcl(self) -> &'static str {
        match self {
            FlowStep::Synthesis => "synth_design",
            FlowStep::Opt => "opt_design",
            FlowStep::Place => "place_design",
            FlowStep::Route => "route_design",
            FlowStep::Bitstream => "write_bitstream",
        }
    }

    fn index(self) -> usize {
        match self {
            FlowStep::Synthesis => 0,
            FlowStep::Opt => 1,
            FlowStep::Place => 2,
            FlowStep::Route => 3,
            FlowStep::Bitstream => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Done,
    Failed,
}

#[derive(Clone, Debug)]
pub struct ConsoleLine {
    pub cmd: String,
    pub out: String,
    pub ok: bool,
}

/// Sources + elaborated netlist, exactly as the HNF design holds it.
#[derive(Clone, Debug, Default)]
pub struct NetlistTree {
    pub sources: Vec<String>,
    pub top: Option<String>,
    /// `(cell, primitive)` from the real design, e.g. `("u_lut0", "LUT6")`.
    pub cells: Vec<(String, String)>,
    pub nets: Vec<String>,
}

impl NetlistTree {
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty() && self.nets.is_empty()
    }

    pub fn has_cell(&self, name: &str) -> bool {
        self.cells.iter().any(|(c, _)| c == name)
    }

    pub fn kind_of(&self, name: &str) -> Option<&str> {
        self.cells
            .iter()
            .find(|(c, _)| c == name)
            .map(|(_, k)| k.as_str())
    }
}

fn primitive_of(kind: &CellKind) -> String {
    match kind {
        CellKind::Lut6 { .. } => "LUT6".into(),
        CellKind::Hff => "HFF".into(),
        CellKind::IobOut => "IOB_OUT".into(),
        CellKind::Mac27 => "MAC27".into(),
        CellKind::Ila { .. } => "ILA".into(),
        CellKind::Bram18 => "BRAM18".into(),
        CellKind::BlackBox { module } => format!("BLACKBOX:{module}"),
    }
}

/// Utilization pane, counted off the packed design against HAD capacity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Utilization {
    pub lutff: usize,
    pub lutff_cap: u32,
    pub iob: usize,
    pub iob_cap: usize,
    pub bram: usize,
    pub bram_cap: u32,
    pub dsp: usize,
    pub dsp_cap: u32,
}

impl Utilization {
    pub fn text(&self) -> String {
        format!(
            "LUTFF={}/{} IOB={}/{} BRAM={}/{} DSP={}/{}",
            self.lutff,
            self.lutff_cap,
            self.iob,
            self.iob_cap,
            self.bram,
            self.bram_cap,
            self.dsp,
            self.dsp_cap
        )
    }
}

/// UG893 Flow Navigator sections. Each click journals Tcl.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavSection {
    BoardDevice,
    ProjectManager,
    IpIntegrator,
    Simulation,
    RtlAnalysis,
    Synthesis,
    Implementation,
    TimingAnalysis,
    ProgramDebug,
}

impl NavSection {
    pub const ALL: [NavSection; 9] = [
        NavSection::BoardDevice,
        NavSection::ProjectManager,
        NavSection::IpIntegrator,
        NavSection::Simulation,
        NavSection::RtlAnalysis,
        NavSection::Synthesis,
        NavSection::Implementation,
        NavSection::TimingAnalysis,
        NavSection::ProgramDebug,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NavSection::BoardDevice => "I/O and Device Planning",
            NavSection::ProjectManager => "Project Manager",
            NavSection::IpIntegrator => "IP Integrator",
            NavSection::Simulation => "Simulation",
            NavSection::RtlAnalysis => "RTL Analysis",
            NavSection::Synthesis => "Synthesis",
            NavSection::Implementation => "Implementation",
            NavSection::TimingAnalysis => "Timing Analysis",
            NavSection::ProgramDebug => "Program and Debug",
        }
    }

    pub fn tcl(self) -> &'static str {
        match self {
            NavSection::BoardDevice => "board_device",
            NavSection::ProjectManager => "project_manager",
            NavSection::IpIntegrator => "ip_integrator",
            NavSection::Simulation => "simulation",
            NavSection::RtlAnalysis => "rtl_analysis",
            NavSection::Synthesis => "synthesis",
            NavSection::Implementation => "implementation",
            NavSection::TimingAnalysis => "timing_analysis",
            NavSection::ProgramDebug => "program_debug",
        }
    }

    /// UG949 / DH0001 system-level stage this navigator item hosts.
    pub fn ultrafast(self) -> UltraFastStage {
        match self {
            NavSection::BoardDevice => UltraFastStage::BoardDevice,
            NavSection::ProjectManager | NavSection::IpIntegrator | NavSection::RtlAnalysis => {
                UltraFastStage::DesignEntry
            }
            NavSection::Simulation => UltraFastStage::LogicSimulation,
            NavSection::Synthesis => UltraFastStage::Synthesis,
            NavSection::Implementation => UltraFastStage::Implementation,
            NavSection::TimingAnalysis => UltraFastStage::TimingAnalysis,
            NavSection::ProgramDebug => UltraFastStage::ProgramDebug,
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let t = s.trim().to_ascii_lowercase().replace(' ', "_");
        match t.as_str() {
            "board_device" | "io_planning" | "device_planning" => Ok(NavSection::BoardDevice),
            "project_manager" | "project" | "design_entry" => Ok(NavSection::ProjectManager),
            "ip_integrator" | "bd" => Ok(NavSection::IpIntegrator),
            "simulation" | "sim" | "logic_simulation" => Ok(NavSection::Simulation),
            "rtl_analysis" | "rtl" => Ok(NavSection::RtlAnalysis),
            "synthesis" | "synth" => Ok(NavSection::Synthesis),
            "implementation" | "impl" => Ok(NavSection::Implementation),
            "timing_analysis" | "design_closure" | "sta" => Ok(NavSection::TimingAnalysis),
            "program_debug" | "hw" | "program" | "bitstream" => Ok(NavSection::ProgramDebug),
            other => Err(format!("unknown nav section {other}")),
        }
    }
}

/// Named UG893 layouts. Default + Simulation are required; others are Helion equivalents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutKind {
    Default,
    Simulation,
}

impl LayoutKind {
    pub const ALL: [LayoutKind; 2] = [LayoutKind::Default, LayoutKind::Simulation];

    pub fn label(self) -> &'static str {
        match self {
            LayoutKind::Default => "Default",
            LayoutKind::Simulation => "Simulation",
        }
    }

    pub fn tcl(self) -> &'static str {
        match self {
            LayoutKind::Default => "default",
            LayoutKind::Simulation => "simulation",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "default" => Ok(LayoutKind::Default),
            "simulation" | "sim" => Ok(LayoutKind::Simulation),
            other => Err(format!("unknown layout {other}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsgSeverity {
    Info,
    Warning,
    Error,
}

impl MsgSeverity {
    pub fn tag(self) -> &'static str {
        match self {
            MsgSeverity::Info => "INFO",
            MsgSeverity::Warning => "WARNING",
            MsgSeverity::Error => "ERROR",
        }
    }
}

#[derive(Clone, Debug)]
pub struct IdeMessage {
    pub severity: MsgSeverity,
    pub id: String,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct DesignRun {
    pub name: String,
    pub step: String,
    pub status: String,
    pub wns_ps: Option<i64>,
    pub cells: Option<usize>,
    pub lutff: Option<usize>,
    pub part: String,
    pub top: Option<String>,
    pub bitstream_hash: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct SchematicNode {
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub struct SchematicEdge {
    pub src: String,
    pub dst: String,
    pub net: String,
}

#[derive(Clone, Debug)]
pub struct SchematicView {
    pub nodes: Vec<SchematicNode>,
    pub edges: Vec<SchematicEdge>,
    /// UG893 expand-cone root. `None` = show the full HNF schematic.
    pub cone_root: Option<String>,
    pub cone_depth: usize,
}

impl Default for SchematicView {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            cone_root: None,
            cone_depth: 1,
        }
    }
}

impl SchematicView {
    pub fn has_cell(&self, name: &str) -> bool {
        self.nodes.iter().any(|n| n.name == name)
    }

    fn cone_cell_names(&self) -> HashSet<String> {
        let Some(root) = &self.cone_root else {
            return self.nodes.iter().map(|n| n.name.clone()).collect();
        };
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        for e in &self.edges {
            adj.entry(e.src.clone()).or_default().push(e.dst.clone());
            adj.entry(e.dst.clone()).or_default().push(e.src.clone());
        }
        let mut seen = HashSet::new();
        let mut q = VecDeque::new();
        q.push_back((root.clone(), 0usize));
        seen.insert(root.clone());
        while let Some((cell, d)) = q.pop_front() {
            if d >= self.cone_depth {
                continue;
            }
            if let Some(nbrs) = adj.get(&cell) {
                for n in nbrs {
                    if seen.insert(n.clone()) {
                        q.push_back((n.clone(), d + 1));
                    }
                }
            }
        }
        seen
    }

    pub fn visible_nodes(&self) -> Vec<&SchematicNode> {
        let keep = self.cone_cell_names();
        self.nodes.iter().filter(|n| keep.contains(&n.name)).collect()
    }

    pub fn visible_edges(&self) -> Vec<&SchematicEdge> {
        let keep = self.cone_cell_names();
        self.edges
            .iter()
            .filter(|e| keep.contains(&e.src) && keep.contains(&e.dst))
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct DeviceSiteView {
    pub x: u32,
    pub y: u32,
    pub kind: SiteKind,
    pub occupant: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DeviceView {
    pub cols: u32,
    pub rows: u32,
    pub sites: Vec<DeviceSiteView>,
}

impl DeviceView {
    pub fn occupant_of(&self, cell: &str) -> Option<&DeviceSiteView> {
        self.sites.iter().find(|s| s.occupant.as_deref() == Some(cell))
    }
}

/// UG900 wave object style: Digital (0↔1 levels) or Analog (numeric series).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveStyle {
    Digital,
    Analog,
}

/// UG900 radix of the Value column. Binary vs Hexadecimal must format
/// the same engine samples differently without mutating them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveRadix {
    Binary,
    Hexadecimal,
}

/// One UG900 design wave object: Name, Value-at-cursor, sample series.
#[derive(Clone, Debug)]
pub struct WaveTrace {
    pub name: String,
    /// One sample per cycle. Width 1 = scalar net; width >1 = packed bus (LSB = bit 0).
    pub samples: Vec<u64>,
    pub width: u8,
    pub style: WaveStyle,
    pub radix: WaveRadix,
}

impl WaveTrace {
    pub fn scalar(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            samples: Vec::new(),
            width: 1,
            style: WaveStyle::Digital,
            radix: WaveRadix::Binary,
        }
    }

    pub fn bus(name: impl Into<String>, width: u8) -> Self {
        Self {
            name: name.into(),
            samples: Vec::new(),
            width: width.max(1),
            style: WaveStyle::Analog,
            radix: WaveRadix::Hexadecimal,
        }
    }

    /// 0/1 string of the LSB of each sample — the fabric LED bitstring for scalars.
    pub fn bit_string(&self) -> String {
        self.samples
            .iter()
            .map(|v| if v & 1 == 1 { '1' } else { '0' })
            .collect()
    }

    /// Analog Y from the same engine samples (scalar 0/1 or bus integer). Not a canned sine.
    pub fn analog_series(&self) -> Vec<f64> {
        self.samples.iter().map(|v| *v as f64).collect()
    }

    pub fn value_at(&self, idx: usize) -> String {
        let v = self.samples.get(idx).copied().unwrap_or(0);
        let w = self.width.max(1) as usize;
        match self.radix {
            WaveRadix::Binary => format!("{v:0w$b}"),
            WaveRadix::Hexadecimal => format!("0x{v:X}"),
        }
    }

    pub fn has_digital_transition(&self) -> bool {
        let bits: Vec<bool> = self.samples.iter().map(|v| v & 1 == 1).collect();
        bits.contains(&true) && bits.contains(&false)
    }
}

#[derive(Clone, Debug)]
pub struct Waveform {
    pub traces: Vec<WaveTrace>,
    pub cursor: usize,
    /// Picoseconds per sample (clock period). UG900 timescale ruler.
    pub timescale_ps: u64,
}

impl Default for Waveform {
    fn default() -> Self {
        Self {
            traces: Vec::new(),
            cursor: 0,
            timescale_ps: 10_000,
        }
    }
}

impl Waveform {
    pub fn bits_of(&self, name: &str) -> Option<String> {
        self.traces.iter().find(|t| t.name == name).map(|t| t.bit_string())
    }

    pub fn has_trace(&self, name: &str) -> bool {
        self.traces.iter().any(|t| t.name == name)
    }

    pub fn trace(&self, name: &str) -> Option<&WaveTrace> {
        self.traces.iter().find(|t| t.name == name)
    }

    pub fn trace_mut(&mut self, name: &str) -> Option<&mut WaveTrace> {
        self.traces.iter_mut().find(|t| t.name == name)
    }

    pub fn time_ps(&self, sample: usize) -> u64 {
        sample as u64 * self.timescale_ps.max(1)
    }

    pub fn set_cursor(&mut self, idx: usize) {
        let n = self
            .traces
            .iter()
            .map(|t| t.samples.len())
            .max()
            .unwrap_or(0);
        self.cursor = if n == 0 { 0 } else { idx.min(n - 1) };
    }

    pub fn sample_len(&self) -> usize {
        self.traces.iter().map(|t| t.samples.len()).max().unwrap_or(0)
    }
}

/// UG949 / DH0001 system-level stages Helion can host (not AMD silicon).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UltraFastStage {
    BoardDevice,
    DesignEntry,
    LogicSimulation,
    Synthesis,
    Implementation,
    TimingAnalysis,
    ProgramDebug,
}

impl UltraFastStage {
    pub const ALL: [UltraFastStage; 7] = [
        UltraFastStage::BoardDevice,
        UltraFastStage::DesignEntry,
        UltraFastStage::LogicSimulation,
        UltraFastStage::Synthesis,
        UltraFastStage::Implementation,
        UltraFastStage::TimingAnalysis,
        UltraFastStage::ProgramDebug,
    ];

    pub fn tcl(self) -> &'static str {
        match self {
            UltraFastStage::BoardDevice => "board_device",
            UltraFastStage::DesignEntry => "design_entry",
            UltraFastStage::LogicSimulation => "logic_simulation",
            UltraFastStage::Synthesis => "synthesis",
            UltraFastStage::Implementation => "implementation",
            UltraFastStage::TimingAnalysis => "timing_analysis",
            UltraFastStage::ProgramDebug => "program_debug",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScopeNode {
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub struct SimObject {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug)]
pub struct IoPortView {
    pub name: String,
    pub dir: String,
    pub site: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IlaTrigger {
    Immediate,
    Rising,
    Falling,
}

impl IlaTrigger {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "immediate" | "any" | "now" => Ok(IlaTrigger::Immediate),
            "rising" | "rise" | "posedge" => Ok(IlaTrigger::Rising),
            "falling" | "fall" | "negedge" => Ok(IlaTrigger::Falling),
            other => Err(format!("ila_trigger: unknown {other}")),
        }
    }

    pub fn tcl(self) -> &'static str {
        match self {
            IlaTrigger::Immediate => "immediate",
            IlaTrigger::Rising => "rising",
            IlaTrigger::Falling => "falling",
        }
    }

    /// Sample index of the trigger in a captured bit stream (fabric ILA).
    pub fn index(self, samples: &[bool]) -> Option<usize> {
        match self {
            IlaTrigger::Immediate => {
                if samples.is_empty() {
                    None
                } else {
                    Some(0)
                }
            }
            IlaTrigger::Rising => samples
                .windows(2)
                .position(|w| !w[0] && w[1])
                .map(|i| i + 1),
            IlaTrigger::Falling => samples
                .windows(2)
                .position(|w| w[0] && !w[1])
                .map(|i| i + 1),
        }
    }
}

/// UG900 Hardware Manager ILA dashboard: probe, window, trigger, capture on wave.
#[derive(Clone, Debug)]
pub struct IlaDashboard {
    pub net: String,
    pub window: usize,
    pub trigger: IlaTrigger,
    pub armed: bool,
    pub bits: String,
    pub trigger_at: Option<usize>,
}

impl Default for IlaDashboard {
    fn default() -> Self {
        Self {
            net: String::new(),
            window: 16,
            trigger: IlaTrigger::Rising,
            armed: false,
            bits: String::new(),
            trigger_at: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HwManager {
    pub open: bool,
    pub programmed: bool,
    pub target: String,
    pub stat: Option<Stat>,
}

impl Default for HwManager {
    fn default() -> Self {
        Self {
            open: false,
            programmed: false,
            target: "sim".into(),
            stat: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BdView {
    pub name: String,
    pub cores: Vec<String>,
    pub sv: String,
    pub ok: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceTab {
    Reports,
    Schematic,
    Device,
    Wave,
    Hardware,
    Ip,
    Constraints,
    Hierarchy,
    Find,
    Package,
    Runs,
}

/// UG893 Hierarchy — top module + instances + leaf primitives from HNF.
#[derive(Clone, Debug, Default)]
pub struct HierarchyView {
    pub top: Option<String>,
    /// `(name, kind)` in tree order: module, then instances, then leaf cells.
    pub nodes: Vec<(String, String)>,
}

impl HierarchyView {
    pub fn has(&self, name: &str) -> bool {
        self.top.as_deref() == Some(name) || self.nodes.iter().any(|(n, _)| n == name)
    }
}

/// UG893 Find Results — hits against the live HNF / HAD, not a placeholder list.
#[derive(Clone, Debug)]
pub struct FindHit {
    pub kind: String,
    pub name: String,
}

/// HAD IOB sites as package pins (Helion has no BGA; pins are IOB_XxYy).
#[derive(Clone, Debug)]
pub struct PackagePin {
    pub pin: String,
    pub x: u32,
    pub y: u32,
    pub port: Option<String>,
}

/// UG893 Package drawing: HAD IOB grid (x,y bounding box), not a restyled pin table.
#[derive(Clone, Debug, Default)]
pub struct PackageDrawing {
    pub part: String,
    pub x0: u32,
    pub y0: u32,
    pub cols: u32,
    pub rows: u32,
}

impl PackageDrawing {
    pub fn pin_at<'a>(&self, pins: &'a [PackagePin], x: u32, y: u32) -> Option<&'a PackagePin> {
        pins.iter().find(|p| p.x == x && p.y == y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BottomTab {
    Tcl,
    Messages,
    Log,
}

/// The whole application model. One `Session` is shared by the console and the rail,
/// and it is the same `Session` type the CLI drives, so both hit the same engines.
#[derive(Clone, Debug)]
pub struct IdeModel {
    pub shell: GpuiShell,
    pub tree: NetlistTree,
    pub console: Vec<ConsoleLine>,
    pub input: String,
    pub steps: [StepState; 5],
    pub timing: Option<TimingResult>,
    pub utilization: Option<Utilization>,
    pub status: String,
    pub clock_period_ps: u64,
    pub nav: NavSection,
    pub layout: LayoutKind,
    pub messages: Vec<IdeMessage>,
    pub log: Vec<String>,
    pub runs: Vec<DesignRun>,
    pub schematic: SchematicView,
    pub device: DeviceView,
    pub properties: Vec<(String, String)>,
    pub selected: Option<String>,
    pub scopes: Vec<ScopeNode>,
    pub objects: Vec<SimObject>,
    pub wave: Waveform,
    pub io_ports: Vec<IoPortView>,
    pub hw: HwManager,
    pub ila: IlaDashboard,
    pub ip_catalog: Vec<IpCore>,
    pub block_design: Option<BdView>,
    pub drc: Option<Drc>,
    /// UG893 Timing Constraints — SDC/XDC clocks that feed helion-sta.
    pub constraints: Constraints,
    pub hierarchy: HierarchyView,
    pub find_results: Vec<FindHit>,
    pub package_pins: Vec<PackagePin>,
    pub package: PackageDrawing,
    pub workspace: WorkspaceTab,
    pub bottom_tab: BottomTab,
    event_sim: Option<Sim>,
    fabric_sim: Option<Fabric>,
}

impl Default for IdeModel {
    fn default() -> Self {
        Self::new()
    }
}

impl IdeModel {
    pub fn new() -> Self {
        Self {
            shell: GpuiShell::default(),
            tree: NetlistTree::default(),
            console: Vec::new(),
            input: String::new(),
            steps: [StepState::Pending; 5],
            timing: None,
            utilization: None,
            status: "idle".into(),
            clock_period_ps: 10_000,
            nav: NavSection::ProjectManager,
            layout: LayoutKind::Default,
            messages: Vec::new(),
            log: Vec::new(),
            runs: vec![
                DesignRun {
                    name: "synth_1".into(),
                    step: "Synthesis".into(),
                    status: "Not started".into(),
                    wns_ps: None,
                    cells: None,
                    lutff: None,
                    part: "HL10T-C32-1".into(),
                    top: None,
                    bitstream_hash: None,
                },
                DesignRun {
                    name: "impl_1".into(),
                    step: "Implementation".into(),
                    status: "Not started".into(),
                    wns_ps: None,
                    cells: None,
                    lutff: None,
                    part: "HL10T-C32-1".into(),
                    top: None,
                    bitstream_hash: None,
                },
            ],
            schematic: SchematicView::default(),
            device: DeviceView::default(),
            properties: Vec::new(),
            selected: None,
            scopes: Vec::new(),
            objects: Vec::new(),
            wave: Waveform::default(),
            io_ports: Vec::new(),
            hw: HwManager::default(),
            ila: IlaDashboard::default(),
            ip_catalog: vec![pack_uart(), pack_gpio()],
            block_design: None,
            drc: None,
            constraints: Constraints::default(),
            hierarchy: HierarchyView::default(),
            find_results: Vec::new(),
            package_pins: Vec::new(),
            package: PackageDrawing::default(),
            workspace: WorkspaceTab::Reports,
            bottom_tab: BottomTab::Tcl,
            event_sim: None,
            fabric_sim: None,
        }
    }

    pub fn with_part(part: &str) -> Self {
        let mut m = Self::new();
        m.shell.part = part.to_string();
        m.shell.session.part = part.to_string();
        m
    }

    pub fn part(&self) -> &str {
        &self.shell.part
    }

    pub fn mode(&self) -> Mode {
        self.shell.mode
    }

    pub fn session(&self) -> &Session {
        &self.shell.session
    }

    pub fn device(&self) -> Result<Device, String> {
        Device::load_part(&self.shell.part)
    }

    pub fn design(&self) -> Option<&Design> {
        self.shell.session.design.as_ref()
    }

    /// State of one rail step.
    pub fn step_state(&self, step: FlowStep) -> StepState {
        self.steps[step.index()]
    }

    /// Real bitstream hash — `None` until `write_bitstream` actually ran.
    pub fn bitstream_hash(&self) -> Option<u32> {
        self.shell.session.blinky_hash()
    }

    pub fn bitstream_bytes(&self) -> Option<usize> {
        self.shell
            .session
            .bitstream
            .as_ref()
            .map(|b| b.packets.len())
    }

    pub fn bitstream_frames(&self) -> Option<usize> {
        self.shell.session.bitstream.as_ref().map(|b| b.frames.len())
    }

    pub fn wns_ps(&self) -> Option<i64> {
        self.timing.as_ref().map(|t| t.wns_ps)
    }

    /// Timing pane text. Empty until the design is routed.
    pub fn timing_text(&self) -> String {
        match &self.timing {
            None => "no routed design — run Route".into(),
            Some(t) => format!(
                "WNS_PS={} TNS_PS={} SETUP_PS={} HOLD_PS={} HOLD_SLACK_PS={} endpoints={} r2r_ps={} iob_ps={} route_ps={}",
                t.wns_ps,
                t.tns_ps,
                t.setup_ps,
                t.hold_ps,
                t.hold_slack_ps,
                t.endpoints,
                t.r2r_ps,
                t.iob_ps,
                t.route_ps
            ),
        }
    }

    /// Utilization pane text. Empty until the design is packed/placed.
    pub fn utilization_text(&self) -> String {
        match &self.utilization {
            None => "no placed design — run Place".into(),
            Some(u) => u.text(),
        }
    }

    /// Console entry point: a raw command string routed onto the real Session.
    pub fn exec(&mut self, cmd: &str) -> Result<String, String> {
        let t = cmd.trim();
        let r = if let Some(rest) = t.strip_prefix("nav ") {
            self.apply_nav(rest)
        } else if let Some(rest) = t.strip_prefix("layout ") {
            self.apply_layout(rest)
        } else if t == "sim_run" || t.starts_with("sim_run ") {
            let n = t
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(16);
            self.sim_run(n)
        } else if t == "sim_step" {
            self.sim_step()
        } else if t == "sim_restart" {
            self.sim_restart()
        } else if let Some(rest) = t.strip_prefix("ultrafast ") {
            self.open_ultrafast(rest)
        } else if let Some(name) = t.strip_prefix("add_wave ") {
            self.add_wave(name.trim())
        } else if let Some(rest) = t.strip_prefix("wave_radix ") {
            self.set_wave_radix(rest)
        } else if let Some(rest) = t.strip_prefix("wave_style ") {
            self.set_wave_style(rest)
        } else if let Some(id) = t.strip_prefix("select ") {
            self.select(id.trim());
            Ok(format!("select {}", id.trim()))
        } else if let Some(q) = t.strip_prefix("find ") {
            self.find(q.trim())
        } else if t == "hierarchy" {
            self.workspace = WorkspaceTab::Hierarchy;
            Ok(self.hierarchy_text())
        } else if t == "package" || t == "package_drawing" {
            self.workspace = WorkspaceTab::Package;
            Ok(self.package_drawing_text())
        } else if let Some(pin) = t.strip_prefix("select_package_pin ") {
            self.select_package_pin(pin.trim())
        } else if t == "design_runs" {
            self.workspace = WorkspaceTab::Runs;
            Ok(self.runs_text())
        } else if t == "launch_runs" || t.starts_with("launch_runs ") {
            let name = t.split_whitespace().nth(1).unwrap_or("impl_1");
            self.launch_runs(name)
        } else if t == "reset_runs" || t.starts_with("reset_runs ") || t == "reset_run" || t.starts_with("reset_run ") {
            let name = t.split_whitespace().nth(1).unwrap_or("impl_1");
            self.reset_runs(name)
        } else if t == "report_drc" {
            self.run_drc()
        } else if t == "create_clock" || t.starts_with("create_clock ") {
            self.apply_create_clock(t)
        } else if t.starts_with("set_input_delay")
            || t.starts_with("set_output_delay")
            || t.starts_with("set_false_path")
        {
            self.apply_sdc_exception(t)
        } else if let Some(path) = t
            .strip_prefix("read_xdc ")
            .or_else(|| t.strip_prefix("read_sdc "))
        {
            self.read_xdc_path(path.trim())
        } else if t == "create_bd" || t == "create_bd_design" || t == "ip_integrator" {
            self.create_block_design()
        } else if t == "ip_catalog" {
            self.refresh_ip_catalog();
            Ok(self
                .ip_catalog
                .iter()
                .map(|c| format!("{}:{}", c.name, c.bus))
                .collect::<Vec<_>>()
                .join(" "))
        } else if let Some(rest) = t.strip_prefix("ila_capture ") {
            let mut parts = rest.split_whitespace();
            let net = parts.next().unwrap_or("led");
            let n = parts
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(self.ila.window);
            self.capture_ila(net, n)
        } else if t == "ila_arm" || t.starts_with("ila_arm ") {
            self.ila_arm(t.strip_prefix("ila_arm").unwrap_or("").trim())
        } else if t == "ila_dashboard" {
            self.workspace = WorkspaceTab::Hardware;
            Ok(self.ila_dashboard_text())
        } else if let Some(rest) = t.strip_prefix("ila_trigger ") {
            self.set_ila_trigger(rest)
        } else if let Some(rest) = t.strip_prefix("ila_window ") {
            self.set_ila_window(rest)
        } else if t == "expand_cone" || t.starts_with("expand_cone ") {
            self.expand_cone(t.strip_prefix("expand_cone").unwrap_or("").trim())
        } else if t == "collapse_cone" {
            self.collapse_cone()
        } else if t == "schematic" {
            self.workspace = WorkspaceTab::Schematic;
            Ok(self.schematic_text())
        } else if t == "messages" {
            self.bottom_tab = BottomTab::Messages;
            Ok(self.messages_text())
        } else if t == "log" {
            self.bottom_tab = BottomTab::Log;
            Ok(self.log_text())
        } else if t == "report_timing" {
            // Same clocks as the Reports/Constraints panes — not Session's 10 ns default.
            self.report_timing_now()
        } else {
            tcl_eval(&mut self.shell, cmd)
        };
        self.journal(cmd, &r);
        self.sync_from_session();
        r
    }

    /// UG893 Messages pane: severity + Tcl id + engine text (not a restyled console).
    pub fn messages_text(&self) -> String {
        let n_err = self
            .messages
            .iter()
            .filter(|m| m.severity == MsgSeverity::Error)
            .count();
        let n_warn = self
            .messages
            .iter()
            .filter(|m| m.severity == MsgSeverity::Warning)
            .count();
        let n_info = self
            .messages
            .iter()
            .filter(|m| m.severity == MsgSeverity::Info)
            .count();
        let mut s = format!("messages errors={n_err} warnings={n_warn} info={n_info}");
        for m in &self.messages {
            s.push_str(&format!("\n{} [{}] {}", m.severity.tag(), m.id, m.text));
        }
        s
    }

    /// UG893 Log pane: Tcl transcript of every console and rail command.
    pub fn log_text(&self) -> String {
        if self.log.is_empty() {
            "log empty".into()
        } else {
            self.log.join("\n")
        }
    }

    fn journal(&mut self, cmd: &str, r: &Result<String, String>) {
        let (ok, out) = match r {
            Ok(s) => (true, s.as_str()),
            Err(e) => (false, e.as_str()),
        };
        let sev = if !ok {
            MsgSeverity::Error
        } else if out.contains("violations=") && !out.contains("violations=0") {
            MsgSeverity::Warning
        } else {
            MsgSeverity::Info
        };
        let id = cmd
            .split_whitespace()
            .next()
            .unwrap_or(cmd)
            .to_string();
        self.messages.push(IdeMessage {
            severity: sev,
            id,
            text: out.to_string(),
        });
        self.log.push(format!("helion% {cmd}\n{out}"));
        self.console.push(ConsoleLine {
            cmd: cmd.to_string(),
            out: out.to_string(),
            ok,
        });
        self.status = if ok {
            format!("{cmd}: ok")
        } else {
            format!("{cmd}: {out}")
        };
    }

    /// Submit whatever is in the console input box (what the widget calls on Enter).
    pub fn submit_input(&mut self) -> Option<Result<String, String>> {
        let cmd = self.input.trim().to_string();
        if cmd.is_empty() {
            return None;
        }
        self.input.clear();
        Some(self.exec(&cmd))
    }

    /// Add an RTL source and elaborate it (Vivado "Add Sources" + synth).
    pub fn open_source(&mut self, path: &Path) -> Result<String, String> {
        let p = path.to_string_lossy().into_owned();
        if !self.tree.sources.contains(&p) {
            self.tree.sources.push(p.clone());
        }
        self.run_step_from(FlowStep::Synthesis, Some(PathBuf::from(path)))
    }

    /// Run one rail step. Refuses to run out of order — Route before Place is an error,
    /// Bitstream before Route is an error — and every step calls the real engine.
    pub fn run_step(&mut self, step: FlowStep) -> Result<String, String> {
        self.run_step_from(step, None)
    }

    fn run_step_from(&mut self, step: FlowStep, source: Option<PathBuf>) -> Result<String, String> {
        let r = self.run_step_inner(step, source);
        self.steps[step.index()] = match &r {
            Ok(_) => StepState::Done,
            Err(_) => StepState::Failed,
        };
        self.journal(step.tcl(), &r);
        self.sync_from_session();
        r
    }

    fn run_step_inner(
        &mut self,
        step: FlowStep,
        source: Option<PathBuf>,
    ) -> Result<String, String> {
        let dev = self.device()?;
        match step {
            FlowStep::Synthesis => {
                let path = source
                    .or_else(|| self.tree.sources.last().map(PathBuf::from))
                    .ok_or("synth_design: add a source first")?;
                let d = helion_sv::synth_sv_path(&path)?;
                let msg = format!(
                    "synth_design {} cells={} luts={}",
                    d.name,
                    d.cells.len(),
                    d.lut_inits().len()
                );
                self.shell.session.synth_design(d);
                self.steps[FlowStep::Opt.index()] = StepState::Pending;
                self.steps[FlowStep::Place.index()] = StepState::Pending;
                self.steps[FlowStep::Route.index()] = StepState::Pending;
                self.steps[FlowStep::Bitstream.index()] = StepState::Pending;
                Ok(msg)
            }
            FlowStep::Opt => {
                let n = self.shell.session.opt_design_step()?;
                Ok(format!("opt_design removed={n}"))
            }
            FlowStep::Place => {
                if self.shell.session.design.is_none() {
                    return Err("place_design: run Synthesis first".into());
                }
                self.shell.session.place_design(&dev)?;
                self.steps[FlowStep::Route.index()] = StepState::Pending;
                self.steps[FlowStep::Bitstream.index()] = StepState::Pending;
                let p = self.shell.session.placed.as_ref().unwrap();
                Ok(format!(
                    "place_design lutff_sites={} iob_sites={} cost={:.3}",
                    p.lutff_sites.len(),
                    p.iob_sites.len(),
                    p.cost
                ))
            }
            FlowStep::Route => {
                if self.shell.session.placed.is_none() {
                    return Err("route_design: run Place first".into());
                }
                self.shell.session.route_design(&dev)?;
                self.steps[FlowStep::Bitstream.index()] = StepState::Pending;
                let r = self.shell.session.routed.as_ref().unwrap();
                Ok(format!(
                    "route_design pathfinder_iters={} overused={} hops={}",
                    r.pathfinder_iters,
                    r.overused,
                    r.iob_src.first().map(|i| i.hops).unwrap_or(0)
                ))
            }
            FlowStep::Bitstream => {
                if self.shell.session.routed.is_none() {
                    return Err("write_bitstream: run Route first".into());
                }
                let b = self.shell.session.write_bitstream(&dev)?;
                let (frames, bytes) = (b.frames.len(), b.packets.len());
                let hash = self.shell.session.blinky_hash().unwrap_or(0);
                Ok(format!(
                    "write_bitstream frames={frames} bytes={bytes} hash={hash:#010x}"
                ))
            }
        }
    }

    pub fn apply_nav(&mut self, name: &str) -> Result<String, String> {
        let sec = NavSection::parse(name)?;
        self.nav = sec;
        match sec {
            NavSection::Simulation => {
                self.layout = LayoutKind::Simulation;
                self.workspace = WorkspaceTab::Wave;
            }
            NavSection::ProgramDebug => {
                self.shell.session.open_hw_manager();
                self.hw.open = true;
                self.workspace = WorkspaceTab::Hardware;
            }
            NavSection::IpIntegrator => {
                self.refresh_ip_catalog();
                self.workspace = WorkspaceTab::Ip;
            }
            NavSection::RtlAnalysis => self.workspace = WorkspaceTab::Schematic,
            NavSection::BoardDevice | NavSection::Implementation => {
                self.workspace = WorkspaceTab::Device
            }
            NavSection::Synthesis | NavSection::TimingAnalysis => {
                self.workspace = WorkspaceTab::Reports
            }
            NavSection::ProjectManager => self.workspace = WorkspaceTab::Reports,
        }
        Ok(format!("nav {}", sec.tcl()))
    }

    pub fn apply_layout(&mut self, name: &str) -> Result<String, String> {
        let layout = LayoutKind::parse(name)?;
        self.layout = layout;
        self.workspace = match layout {
            LayoutKind::Simulation => WorkspaceTab::Wave,
            LayoutKind::Default => WorkspaceTab::Reports,
        };
        Ok(format!("layout {}", layout.tcl()))
    }

    pub fn set_nav(&mut self, sec: NavSection) -> Result<String, String> {
        self.exec(&format!("nav {}", sec.tcl()))
    }

    pub fn set_layout(&mut self, layout: LayoutKind) -> Result<String, String> {
        self.exec(&format!("layout {}", layout.tcl()))
    }

    pub fn open_ultrafast(&mut self, name: &str) -> Result<String, String> {
        let sec = NavSection::parse(name)?;
        self.apply_nav(sec.tcl())
    }

    /// Engine-backed snapshot of the pane an UltraFast stage opens. Empty chrome fails.
    pub fn ultrafast_pane_engine(&self, stage: UltraFastStage) -> Result<String, String> {
        match stage {
            UltraFastStage::BoardDevice => {
                if self.device.sites.is_empty() {
                    return Err("board/device: HAD sites missing".into());
                }
                let iob = self.io_ports.len();
                if self.package_pins.is_empty() {
                    return Err("board/device: HAD package pins missing".into());
                }
                if self.package.cols == 0 || self.package.rows == 0 {
                    return Err("board/device: HAD package drawing missing".into());
                }
                Ok(format!(
                    "board_device sites={} iob_ports={} pins={} cols={} rows={} part={}",
                    self.device.sites.len(),
                    iob,
                    self.package_pins.len(),
                    self.package.cols,
                    self.package.rows,
                    self.part()
                ))
            }
            UltraFastStage::DesignEntry => {
                if self.tree.sources.is_empty() && self.tree.cells.is_empty() {
                    return Err("design_entry: no sources".into());
                }
                Ok(format!(
                    "design_entry sources={} cells={}",
                    self.tree.sources.len(),
                    self.tree.cells.len()
                ))
            }
            UltraFastStage::LogicSimulation => {
                let n = self.wave.sample_len();
                if n == 0 {
                    return Err("logic_simulation: no wave samples".into());
                }
                Ok(format!(
                    "logic_simulation samples={n} traces={}",
                    self.wave.traces.len()
                ))
            }
            UltraFastStage::Synthesis => {
                let n = self.tree.cells.len();
                if n == 0 {
                    return Err("synthesis: no HNF cells".into());
                }
                Ok(format!("synthesis cells={n}"))
            }
            UltraFastStage::Implementation => {
                if self.device.occupant_of("u_lut0").is_none()
                    && self.device.sites.iter().all(|s| s.occupant.is_none())
                {
                    return Err("implementation: no placed occupants".into());
                }
                Ok(format!(
                    "implementation occupied={}",
                    self.device.sites.iter().filter(|s| s.occupant.is_some()).count()
                ))
            }
            UltraFastStage::TimingAnalysis => {
                let wns = self.wns_ps().ok_or("timing_analysis: no STA WNS_PS")?;
                let nclk = self.constraints.clocks.len();
                Ok(format!("timing_analysis WNS_PS={wns} clocks={nclk}"))
            }
            UltraFastStage::ProgramDebug => {
                if let Some(h) = self.bitstream_hash() {
                    return Ok(format!("program_debug hash={h:#010x}"));
                }
                if let Some(st) = &self.hw.stat {
                    return Ok(format!("program_debug DONE={}", st.done as u8));
                }
                Err("program_debug: no bitstream/STAT".into())
            }
        }
    }

    pub fn add_wave(&mut self, name: &str) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("add_wave: empty name".into());
        }
        if self.wave.has_trace(name) {
            return Ok(format!("add_wave {name} (already)"));
        }
        if name == "led" || self.objects.iter().any(|o| o.name == name) {
            self.wave.traces.push(WaveTrace::scalar(name));
            return Ok(format!("add_wave {name}"));
        }
        Err(format!("add_wave: no scoped object {name}"))
    }

    pub fn set_wave_radix(&mut self, spec: &str) -> Result<String, String> {
        let mut parts = spec.split_whitespace();
        let name = parts.next().ok_or("wave_radix: need <name> binary|hex")?;
        let r = parts.next().unwrap_or("binary");
        let radix = match r.to_ascii_lowercase().as_str() {
            "binary" | "bin" => WaveRadix::Binary,
            "hex" | "hexadecimal" => WaveRadix::Hexadecimal,
            other => return Err(format!("wave_radix: unknown {other}")),
        };
        let t = self
            .wave
            .trace_mut(name)
            .ok_or_else(|| format!("wave_radix: no trace {name}"))?;
        t.radix = radix;
        Ok(format!("wave_radix {name} {r}"))
    }

    pub fn set_wave_style(&mut self, spec: &str) -> Result<String, String> {
        let mut parts = spec.split_whitespace();
        let name = parts.next().ok_or("wave_style: need <name> digital|analog")?;
        let s = parts.next().unwrap_or("digital");
        let style = match s.to_ascii_lowercase().as_str() {
            "digital" => WaveStyle::Digital,
            "analog" => WaveStyle::Analog,
            other => return Err(format!("wave_style: unknown {other}")),
        };
        let t = self
            .wave
            .trace_mut(name)
            .ok_or_else(|| format!("wave_style: no trace {name}"))?;
        t.style = style;
        Ok(format!("wave_style {name} {s}"))
    }

    /// Cross-select: one identity shared by Netlist, Schematic, Device, Properties.
    pub fn select(&mut self, id: &str) {
        let id = id.trim();
        if id.is_empty() {
            self.selected = None;
            self.properties.clear();
            return;
        }
        self.selected = Some(id.to_string());
        self.refresh_properties();
    }

    pub fn selected_cell(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn netlist_has_selected(&self) -> bool {
        let Some(id) = self.selected.as_deref() else {
            return false;
        };
        self.tree.has_cell(id) || self.tree.nets.iter().any(|n| n == id)
    }

    pub fn schematic_has_selected(&self) -> bool {
        let Some(id) = self.selected.as_deref() else {
            return false;
        };
        self.schematic.has_cell(id)
    }

    pub fn hierarchy_has_selected(&self) -> bool {
        let Some(id) = self.selected.as_deref() else {
            return false;
        };
        self.hierarchy.has(id)
    }

    /// UG893 Hierarchy pane dump: top, instances, then leaf primitives.
    pub fn hierarchy_text(&self) -> String {
        match &self.hierarchy.top {
            None => "no hierarchy — synth first".into(),
            Some(top) => {
                let mut s = format!("top={top}");
                for (name, kind) in &self.hierarchy.nodes {
                    s.push_str(&format!(" {name}:{kind}"));
                }
                s
            }
        }
    }

    /// UG893 Design Runs pane: synth_1 / impl_1 status from the live Session.
    pub fn runs_text(&self) -> String {
        self.runs
            .iter()
            .map(|r| {
                let mut s = format!("{} {} {}", r.name, r.step, r.status);
                if let Some(top) = &r.top {
                    s.push_str(&format!(" top={top}"));
                }
                s.push_str(&format!(" part={}", r.part));
                if let Some(n) = r.cells {
                    s.push_str(&format!(" cells={n}"));
                }
                if let Some(n) = r.lutff {
                    s.push_str(&format!(" LUTFF={n}"));
                }
                if let Some(w) = r.wns_ps {
                    s.push_str(&format!(" WNS_PS={w}"));
                }
                if let Some(h) = r.bitstream_hash {
                    s.push_str(&format!(" hash={h:#010x}"));
                }
                s
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    /// UG893 schematic dump: full HNF or the expand-cone subset.
    pub fn schematic_text(&self) -> String {
        let nodes = self.schematic.visible_nodes();
        let edges = self.schematic.visible_edges();
        let root = self.schematic.cone_root.as_deref().unwrap_or("-");
        let mut s = format!(
            "schematic cone={root} depth={} cells={} edges={}",
            self.schematic.cone_depth,
            nodes.len(),
            edges.len()
        );
        for n in &nodes {
            s.push_str(&format!(" {}:{}", n.name, n.kind));
        }
        for e in &edges {
            s.push_str(&format!(" {}-{}-{}", e.src, e.net, e.dst));
        }
        s
    }

    /// Expand the schematic cone from a cell along HNF nets (UG893 Expand Cone).
    pub fn expand_cone(&mut self, spec: &str) -> Result<String, String> {
        let mut parts = spec.split_whitespace();
        let name = match parts.next() {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => self
                .selected
                .clone()
                .ok_or_else(|| "expand_cone: select a cell first".to_string())?,
        };
        if !self.schematic.has_cell(&name) {
            return Err(format!("expand_cone: no cell {name}"));
        }
        let depth = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
        self.schematic.cone_root = Some(name.clone());
        self.schematic.cone_depth = depth;
        self.select(&name);
        self.workspace = WorkspaceTab::Schematic;
        Ok(self.schematic_text())
    }

    pub fn collapse_cone(&mut self) -> Result<String, String> {
        self.schematic.cone_root = None;
        self.workspace = WorkspaceTab::Schematic;
        Ok(self.schematic_text())
    }

    /// UG893 Package drawing dump: HAD IOB bounding box + occupancy map.
    pub fn package_drawing_text(&self) -> String {
        let n = self.package_pins.len();
        let assigned = self
            .package_pins
            .iter()
            .filter(|p| p.port.is_some())
            .count();
        let mut s = format!(
            "package drawing part={} cols={} rows={} pins={} assigned={} x0={} y0={}",
            if self.package.part.is_empty() {
                self.part()
            } else {
                self.package.part.as_str()
            },
            self.package.cols,
            self.package.rows,
            n,
            assigned,
            self.package.x0,
            self.package.y0
        );
        if self.package.cols == 0 || self.package.rows == 0 {
            return s;
        }
        for dy in 0..self.package.rows {
            let y = self.package.y0 + dy;
            let mut row = format!(" row y={y}");
            let mut map = String::new();
            for dx in 0..self.package.cols {
                let x = self.package.x0 + dx;
                if let Some(p) = self.package.pin_at(&self.package_pins, x, y) {
                    row.push_str(&format!(
                        " {}={}",
                        p.pin,
                        p.port.as_deref().unwrap_or("-")
                    ));
                    map.push(match p.port.as_deref() {
                        Some("led") => 'L',
                        Some(_) => 'P',
                        None => '.',
                    });
                } else {
                    map.push(' ');
                }
            }
            s.push_str(&format!("{row} map={map}"));
        }
        s
    }

    /// Click a package pin: assigned pins cross-select the I/O port.
    pub fn select_package_pin(&mut self, pin: &str) -> Result<String, String> {
        let pin = pin.trim();
        let found = self
            .package_pins
            .iter()
            .find(|p| p.pin == pin)
            .cloned()
            .ok_or_else(|| format!("select_package_pin: no HAD pin {pin}"))?;
        self.workspace = WorkspaceTab::Package;
        match found.port {
            Some(port) => {
                self.select(&port);
                Ok(format!("package_pin {} port={port} X{}Y{}", found.pin, found.x, found.y))
            }
            None => {
                self.select(&found.pin);
                Ok(format!(
                    "package_pin {} unassigned X{}Y{}",
                    found.pin, found.x, found.y
                ))
            }
        }
    }

    pub fn package_has_selected(&self) -> bool {
        let Some(id) = self.selected.as_deref() else {
            return false;
        };
        self.package_pins
            .iter()
            .any(|p| p.pin == id || p.port.as_deref() == Some(id))
    }

    /// Launch a named run on the real engines (not a status lamp).
    pub fn launch_runs(&mut self, name: &str) -> Result<String, String> {
        self.workspace = WorkspaceTab::Runs;
        match name.trim().to_ascii_lowercase().as_str() {
            "synth_1" | "synth" | "synthesis" => {
                self.run_step(FlowStep::Synthesis)?;
            }
            "impl_1" | "impl" | "implementation" => {
                if self.shell.session.design.is_none() {
                    return Err("launch_runs impl_1: run synth_1 first".into());
                }
                for step in [
                    FlowStep::Opt,
                    FlowStep::Place,
                    FlowStep::Route,
                    FlowStep::Bitstream,
                ] {
                    if self.step_state(step) != StepState::Done {
                        self.run_step(step)?;
                    }
                }
            }
            other => return Err(format!("launch_runs: unknown run {other}")),
        }
        self.sync_from_session();
        Ok(self.runs_text())
    }

    /// UG893 `reset_run`: drop Session impl (and synth) artifacts, not a status lamp.
    pub fn reset_runs(&mut self, name: &str) -> Result<String, String> {
        self.workspace = WorkspaceTab::Runs;
        match name.trim().to_ascii_lowercase().as_str() {
            "impl_1" | "impl" | "implementation" => {
                self.shell.session.reset_impl();
                for step in [
                    FlowStep::Opt,
                    FlowStep::Place,
                    FlowStep::Route,
                    FlowStep::Bitstream,
                ] {
                    self.steps[step.index()] = StepState::Pending;
                }
            }
            "synth_1" | "synth" | "synthesis" => {
                self.shell.session.reset_synth();
                self.steps = [StepState::Pending; 5];
            }
            other => return Err(format!("reset_runs: unknown run {other}")),
        }
        self.timing = None;
        self.utilization = None;
        self.hw.programmed = false;
        self.hw.stat = None;
        self.drc = None;
        self.fabric_sim = None;
        self.event_sim = None;
        self.wave = Waveform::default();
        self.sync_from_session();
        Ok(self.runs_text())
    }

    /// UG893 Find: substring match on HNF cells/nets/ports and HAD pin names.
    pub fn find(&mut self, query: &str) -> Result<String, String> {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return Err("find: empty query".into());
        }
        let mut hits = Vec::new();
        if let Some(d) = self.shell.session.design.as_ref() {
            for c in &d.cells {
                if c.name.to_ascii_lowercase().contains(&q) {
                    hits.push(FindHit {
                        kind: "cell".into(),
                        name: c.name.clone(),
                    });
                }
            }
            for n in &d.nets {
                if n.name.to_ascii_lowercase().contains(&q) {
                    hits.push(FindHit {
                        kind: "net".into(),
                        name: n.name.clone(),
                    });
                }
            }
            for p in &d.ports {
                if p.name.to_ascii_lowercase().contains(&q) {
                    hits.push(FindHit {
                        kind: "port".into(),
                        name: p.name.clone(),
                    });
                }
            }
        }
        for pin in &self.package_pins {
            if pin.pin.to_ascii_lowercase().contains(&q) {
                hits.push(FindHit {
                    kind: "pin".into(),
                    name: pin.pin.clone(),
                });
            }
        }
        let n = hits.len();
        self.find_results = hits;
        self.workspace = WorkspaceTab::Find;
        if n == 0 {
            return Err(format!("find {query}: 0 hits"));
        }
        let cell = self
            .find_results
            .iter()
            .find(|h| h.kind == "cell")
            .map(|h| h.name.clone());
        if let Some(name) = cell {
            self.select(&name);
        }
        Ok(format!(
            "find {query} hits={n} {}",
            self.find_results
                .iter()
                .map(|h| format!("{}:{}", h.kind, h.name))
                .collect::<Vec<_>>()
                .join(" ")
        ))
    }

    pub fn device_has_selected(&self) -> bool {
        let Some(id) = self.selected.as_deref() else {
            return false;
        };
        self.device.occupant_of(id).is_some()
    }

    pub fn properties_name(&self) -> Option<&str> {
        self.properties
            .iter()
            .find(|(k, _)| k == "NAME")
            .map(|(_, v)| v.as_str())
    }

    pub fn refresh_ip_catalog(&mut self) {
        self.ip_catalog = vec![pack_uart(), pack_gpio()];
    }

    pub fn create_block_design(&mut self) -> Result<String, String> {
        self.refresh_ip_catalog();
        let bd = BlockDesign {
            name: "system".into(),
            cores: self.ip_catalog.clone(),
        };
        let v = validate(&bd);
        let sv = if v.ok {
            emit_sv(&bd)?
        } else {
            String::new()
        };
        let cores: Vec<String> = bd.cores.iter().map(|c| c.name.clone()).collect();
        let msg = if v.ok {
            format!("create_bd_design {} cores={}", bd.name, cores.join(","))
        } else {
            format!("create_bd_design failed {}", v.errors.join("; "))
        };
        self.block_design = Some(BdView {
            name: bd.name,
            cores,
            sv,
            ok: v.ok,
        });
        if v.ok {
            Ok(msg)
        } else {
            Err(msg)
        }
    }

    /// UG893 Timing Constraints pane text. Empty until create_clock / read_xdc.
    pub fn constraints_text(&self) -> String {
        if self.constraints.clocks.is_empty()
            && self.constraints.input_delay_ps.is_empty()
            && self.constraints.output_delay_ps.is_empty()
            && self.constraints.false_paths.is_empty()
            && self.constraints.package_pins.is_empty()
        {
            return "no timing constraints — create_clock / read_xdc".into();
        }
        let mut lines = Vec::new();
        for c in &self.constraints.clocks {
            lines.push(format!(
                "create_clock -name {} -period {:.3} [get_ports {}] PERIOD_PS={} generated={}",
                c.name,
                c.period_ps as f64 / 1000.0,
                c.source,
                c.period_ps,
                u8::from(c.generated)
            ));
        }
        for (port, ps) in &self.constraints.input_delay_ps {
            lines.push(format!("set_input_delay {port} DELAY_PS={ps}"));
        }
        for (port, ps) in &self.constraints.output_delay_ps {
            lines.push(format!("set_output_delay {port} DELAY_PS={ps}"));
        }
        for fp in &self.constraints.false_paths {
            lines.push(format!("set_false_path {fp}"));
        }
        for (port, pin) in &self.constraints.package_pins {
            lines.push(format!("set_property PACKAGE_PIN {pin} {port}"));
        }
        lines.join("\n")
    }

    fn merge_constraints(&mut self, extra: Constraints) {
        for c in extra.clocks {
            self.constraints.clocks.retain(|k| k.name != c.name);
            self.constraints.clocks.push(c);
        }
        self.constraints.input_delay_ps.extend(extra.input_delay_ps);
        self.constraints.output_delay_ps.extend(extra.output_delay_ps);
        for fp in extra.false_paths {
            if !self.constraints.false_paths.contains(&fp) {
                self.constraints.false_paths.push(fp);
            }
        }
        self.constraints.package_pins.extend(extra.package_pins);
        if let Some(c) = self
            .constraints
            .clocks
            .iter()
            .find(|c| c.source == "clk" || c.name == "clk")
            .or_else(|| self.constraints.clocks.first())
        {
            self.clock_period_ps = c.period_ps;
        }
        if let Some(d) = self.shell.session.design.as_mut() {
            let _ = self.constraints.apply(d);
        }
        self.workspace = WorkspaceTab::Constraints;
    }

    pub fn apply_create_clock(&mut self, cmd: &str) -> Result<String, String> {
        let extra = load_xdc(cmd)?;
        if extra.clocks.is_empty() {
            return Err("create_clock: missing -period".into());
        }
        let n = extra.clocks.len();
        let period = extra.clocks[0].period_ps;
        let name = extra.clocks[0].name.clone();
        self.merge_constraints(extra);
        Ok(format!("create_clock {name} PERIOD_PS={period} clocks={n}"))
    }

    /// UG893 Timing Constraints Apply: set_input_delay / set_output_delay / set_false_path
    /// land in the pane and feed helion-sta (WNS moves).
    pub fn apply_sdc_exception(&mut self, cmd: &str) -> Result<String, String> {
        let extra = load_xdc(cmd)?;
        if extra.input_delay_ps.is_empty()
            && extra.output_delay_ps.is_empty()
            && extra.false_paths.is_empty()
        {
            return Err(format!("{cmd}: missing delay or false path"));
        }
        let n_in = extra.input_delay_ps.len();
        let n_out = extra.output_delay_ps.len();
        let n_fp = extra.false_paths.len();
        let in_ps = extra.input_delay_ps.values().copied().max().unwrap_or(0);
        let out_ps = extra.output_delay_ps.values().copied().max().unwrap_or(0);
        self.merge_constraints(extra);
        Ok(format!(
            "apply_xdc input_delay={n_in} DELAY_PS={in_ps} output_delay={n_out} DELAY_PS={out_ps} false_path={n_fp}"
        ))
    }

    pub fn read_xdc_path(&mut self, path: &str) -> Result<String, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("read_xdc {path}: {e}"))?;
        let extra = load_xdc(&text)?;
        if extra.clocks.is_empty()
            && extra.input_delay_ps.is_empty()
            && extra.output_delay_ps.is_empty()
            && extra.false_paths.is_empty()
            && extra.package_pins.is_empty()
        {
            return Err(format!("read_xdc {path}: no timing constraints"));
        }
        let n = extra.clocks.len();
        let period = extra
            .clocks
            .first()
            .map(|c| c.period_ps)
            .unwrap_or(self.clock_period_ps);
        let n_in = extra.input_delay_ps.len();
        let n_out = extra.output_delay_ps.len();
        let n_fp = extra.false_paths.len();
        self.merge_constraints(extra);
        Ok(format!(
            "read_xdc clocks={n} PERIOD_PS={period} input_delay={n_in} output_delay={n_out} false_path={n_fp}"
        ))
    }

    fn clocks_for_sta(&self) -> Vec<helion_sta::Clock> {
        let mut clks = self.constraints.clocks.clone();
        if clks.is_empty() {
            create_clock(&mut clks, "clk", self.clock_period_ps, "clk");
        }
        clks
    }

    /// Console `report_timing` uses IdeModel constraint clocks + I/O delay/false path,
    /// the same vector `refresh_reports` feeds `report_timing_routed_xdc`. Pulls place/route
    /// if needed so `read_sv` then `report_timing` still hits STA (old Tcl path).
    pub fn report_timing_now(&mut self) -> Result<String, String> {
        if self.shell.session.design.is_none() {
            return Err("report_timing: no design".into());
        }
        if self.shell.session.routed.is_none() {
            let dev = self.device()?;
            if self.shell.session.placed.is_none() {
                self.shell.session.place_design(&dev)?;
            }
            self.shell.session.route_design(&dev)?;
        }
        let clks = self.clocks_for_sta();
        let d = self.shell.session.design.as_ref().unwrap();
        let r = self.shell.session.routed.as_ref().unwrap();
        let t = report_timing_routed_xdc(d, r, &clks, &self.constraints)?;
        Ok(format!(
            "report_timing {} WNS_PS={} TNS_PS={} SETUP_PS={} HOLD_PS={} HOLD_SLACK_PS={} endpoints={} r2r_ps={} iob_ps={} route_ps={}",
            d.name, t.wns_ps, t.tns_ps, t.setup_ps, t.hold_ps, t.hold_slack_ps, t.endpoints, t.r2r_ps, t.iob_ps, t.route_ps
        ))
    }

    pub fn run_drc(&mut self) -> Result<String, String> {
        let dev = self.device()?;
        let s = &self.shell.session;
        let drc = if let (Some(d), Some(r)) = (s.design.as_ref(), s.routed.as_ref()) {
            check_routed(d, r, &dev)
        } else if let (Some(d), Some(p)) = (s.design.as_ref(), s.placed.as_ref()) {
            check_placed(d, p, &dev)
        } else {
            return Err("report_drc: place or route first".into());
        };
        let n = drc.violations.len();
        let text = if drc.ok() {
            format!("report_drc violations=0 ok")
        } else {
            format!("report_drc violations={n} {}", drc.violations.join("; "))
        };
        self.drc = Some(drc);
        Ok(text)
    }

    pub fn sim_run(&mut self, cycles: u32) -> Result<String, String> {
        self.prepare_sim()?;
        self.wave.traces.clear();
        self.wave.cursor = 0;
        self.wave.timescale_ps = self.clock_period_ps;
        for _ in 0..cycles {
            self.sim_step_inner()?;
        }
        let n = self.wave.sample_len();
        if n > 0 {
            self.wave.set_cursor(n - 1);
        }
        let led = self
            .wave
            .bits_of("led")
            .unwrap_or_default();
        Ok(format!("sim_run cycles={cycles} LED[{cycles}]={led}"))
    }

    pub fn sim_step(&mut self) -> Result<String, String> {
        if self.event_sim.is_none() && self.fabric_sim.is_none() {
            self.prepare_sim()?;
        }
        self.sim_step_inner()?;
        let n = self
            .wave
            .traces
            .first()
            .map(|t| t.samples.len())
            .unwrap_or(0);
        Ok(format!("sim_step t={n}"))
    }

    pub fn sim_restart(&mut self) -> Result<String, String> {
        self.event_sim = None;
        self.fabric_sim = None;
        self.wave = Waveform::default();
        self.objects.clear();
        self.prepare_sim()?;
        Ok("sim_restart".into())
    }

    fn prepare_sim(&mut self) -> Result<(), String> {
        let d = self
            .shell
            .session
            .design
            .clone()
            .ok_or("sim: no design (synth first)")?;
        self.scopes = vec![ScopeNode {
            name: d.name.clone(),
            kind: "module".into(),
        }];
        if let (Some(bits), Some(r)) = (
            self.shell.session.bitstream.clone(),
            self.shell.session.routed.clone(),
        ) {
            let dev = self.device()?;
            let mut fab = Fabric::new(&dev);
            fab.program(&bits)?;
            fab.finish_startup();
            self.fabric_sim = Some(fab);
            let _ = r;
        } else {
            self.event_sim = Some(Sim::new(&d));
        }
        self.wave.timescale_ps = self.clock_period_ps;
        if self.wave.traces.is_empty() {
            self.wave.traces.push(WaveTrace::scalar("led"));
        }
        Ok(())
    }

    fn sim_step_inner(&mut self) -> Result<(), String> {
        let (led, bus, bus_w) = if let Some(fab) = self.fabric_sim.as_mut() {
            let iob = self
                .shell
                .session
                .routed
                .as_ref()
                .and_then(|r| r.iob_src.first())
                .ok_or("sim: no routed IOB")?;
            fab.step_user();
            let led = fab.led_at(iob.iob.0, iob.iob.1);
            let mut bus = 0u64;
            let mut w = 0u8;
            if let Some(pl) = self.shell.session.placed.as_ref() {
                w = pl.lutff_sites.len().min(8) as u8;
                for (i, (site, ble)) in pl.lutff_sites.iter().take(8).enumerate() {
                    if fab.ble_q(site.x, site.y, *ble as u32) {
                        bus |= 1 << i;
                    }
                }
            }
            (led, bus, w)
        } else if let Some(sim) = self.event_sim.as_mut() {
            sim.step_posedge(10);
            (sim.led, u64::from(sim.led), 1)
        } else {
            return Err("sim: not started".into());
        };
        Self::push_sample(&mut self.wave, "led", u64::from(led), 1, WaveStyle::Digital);
        if bus_w > 1 {
            Self::push_sample(&mut self.wave, "cnt", bus, bus_w, WaveStyle::Analog);
        }
        let n = self.wave.sample_len();
        if n > 0 {
            self.wave.set_cursor(n - 1);
        }
        let cur = self.wave.cursor;
        self.objects = vec![SimObject {
            name: "led".into(),
            value: self
                .wave
                .trace("led")
                .map(|t| t.value_at(cur))
                .unwrap_or_else(|| if led { "1".into() } else { "0".into() }),
        }];
        if let Some(t) = self.wave.trace("cnt") {
            self.objects.push(SimObject {
                name: "cnt".into(),
                value: t.value_at(cur),
            });
        }
        if let Some(d) = self.shell.session.design.as_ref() {
            for p in &d.ports {
                if p.name != "led" {
                    self.objects.push(SimObject {
                        name: p.name.clone(),
                        value: "-".into(),
                    });
                }
            }
        }
        Ok(())
    }

    fn push_sample(wave: &mut Waveform, name: &str, v: u64, width: u8, style: WaveStyle) {
        if let Some(t) = wave.traces.iter_mut().find(|t| t.name == name) {
            t.samples.push(v);
            t.width = width.max(1);
        } else {
            let mut t = if width > 1 {
                WaveTrace::bus(name, width)
            } else {
                WaveTrace::scalar(name)
            };
            t.style = style;
            t.samples.push(v);
            wave.traces.push(t);
        }
    }

    pub fn capture_ila(&mut self, net: &str, n: usize) -> Result<String, String> {
        let d = self
            .shell
            .session
            .design
            .clone()
            .ok_or("ila_capture: no design")?;
        let dev = self.device()?;
        let n = n.max(1);
        self.ila.armed = true;
        let cap: IlaCapture = insert_arm_capture(&dev, &d, net, n)?;
        let bits: String = cap
            .samples
            .iter()
            .map(|b| if *b { '1' } else { '0' })
            .collect();
        let tname = format!("ila:{}", cap.net);
        let samples: Vec<u64> = cap.samples.iter().map(|b| u64::from(*b)).collect();
        if let Some(t) = self.wave.traces.iter_mut().find(|t| t.name == tname) {
            t.samples = samples;
            t.width = 1;
        } else {
            let mut t = WaveTrace::scalar(tname);
            t.samples = samples;
            self.wave.traces.push(t);
        }
        self.ila.net = cap.net.clone();
        self.ila.window = cap.samples.len();
        self.ila.bits = bits.clone();
        self.ila.armed = false;
        self.apply_ila_trigger_to_wave();
        self.workspace = WorkspaceTab::Hardware;
        self.hw.open = true;
        let at = self
            .ila
            .trigger_at
            .map(|i| i.to_string())
            .unwrap_or_else(|| "-".into());
        Ok(format!(
            "ila_capture net={} samples={} bits={bits} trigger={} trigger_at={at}",
            cap.net,
            cap.samples.len(),
            self.ila.trigger.tcl()
        ))
    }

    pub fn ila_dashboard_text(&self) -> String {
        let at = self
            .ila
            .trigger_at
            .map(|i| i.to_string())
            .unwrap_or_else(|| "-".into());
        let captured = if self.ila.bits.is_empty() {
            0
        } else {
            self.ila.bits.len()
        };
        format!(
            "ila dashboard net={} window={} trigger={} armed={} captured={} trigger_at={} bits={}",
            if self.ila.net.is_empty() {
                "-"
            } else {
                self.ila.net.as_str()
            },
            self.ila.window,
            self.ila.trigger.tcl(),
            u8::from(self.ila.armed),
            captured,
            at,
            if self.ila.bits.is_empty() {
                "-"
            } else {
                self.ila.bits.as_str()
            }
        )
    }

    pub fn set_ila_trigger(&mut self, spec: &str) -> Result<String, String> {
        self.ila.trigger = IlaTrigger::parse(spec)?;
        self.apply_ila_trigger_to_wave();
        self.workspace = WorkspaceTab::Hardware;
        Ok(self.ila_dashboard_text())
    }

    pub fn set_ila_window(&mut self, spec: &str) -> Result<String, String> {
        let n: usize = spec
            .trim()
            .parse()
            .map_err(|_| format!("ila_window: not a count {spec}"))?;
        if n == 0 {
            return Err("ila_window: need >= 1".into());
        }
        self.ila.window = n;
        self.workspace = WorkspaceTab::Hardware;
        Ok(self.ila_dashboard_text())
    }

    pub fn ila_arm(&mut self, spec: &str) -> Result<String, String> {
        let mut parts = spec.split_whitespace();
        let net = parts
            .next()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if self.ila.net.is_empty() {
                    "led".into()
                } else {
                    self.ila.net.clone()
                }
            });
        let n = parts
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(self.ila.window);
        self.capture_ila(&net, n)
    }

    fn apply_ila_trigger_to_wave(&mut self) {
        let samples: Vec<bool> = self.ila.bits.chars().map(|c| c == '1').collect();
        self.ila.trigger_at = self.ila.trigger.index(&samples);
        if let Some(i) = self.ila.trigger_at {
            self.wave.set_cursor(i);
        }
    }

    /// Independent fabric LED string, same path as `helion run`.
    pub fn fabric_led_bits(&self, cycles: u32) -> Result<String, String> {
        let bits = self
            .shell
            .session
            .bitstream
            .as_ref()
            .ok_or("fabric LED: no bitstream")?;
        let r = self
            .shell
            .session
            .routed
            .as_ref()
            .ok_or("fabric LED: not routed")?;
        let iob = r.iob_src.first().ok_or("fabric LED: no IOB route")?;
        let dev = self.device()?;
        let mut fab = Fabric::new(&dev);
        fab.program(bits)?;
        fab.finish_startup();
        let mut wave = String::new();
        for _ in 0..cycles {
            fab.step_user();
            wave.push(if fab.led_at(iob.iob.0, iob.iob.1) {
                '1'
            } else {
                '0'
            });
        }
        Ok(wave)
    }

    /// Rebuild every pane off the current Session state. Called after each command.
    pub fn sync_from_session(&mut self) {
        self.refresh_tree();
        self.refresh_reports();
        self.refresh_steps();
        self.refresh_analysis();
        self.refresh_hw();
        self.refresh_runs();
        if self.selected.is_some() {
            self.refresh_properties();
        }
    }

    fn refresh_tree(&mut self) {
        let Some(d) = self.shell.session.design.clone() else {
            self.tree.top = None;
            self.tree.cells.clear();
            self.tree.nets.clear();
            return;
        };
        self.tree.top = Some(d.name.clone());
        self.tree.cells = d
            .cells
            .iter()
            .map(|c| (c.name.clone(), primitive_of(&c.kind)))
            .collect();
        self.tree.nets = get_nets(&d, None);
        debug_assert_eq!(self.tree.cells.len(), get_cells(&d, None).len());
    }

    fn refresh_reports(&mut self) {
        let s = &self.shell.session;
        self.utilization = match (s.placed.as_ref().map(|p| &p.packed)).or(s.packed.as_ref()) {
            None => None,
            Some(p) => match self.device() {
                Err(_) => None,
                Ok(dev) => Some(Utilization {
                    lutff: p.lutffs.len(),
                    lutff_cap: dev.lut6_count(),
                    iob: p.iobs.len(),
                    iob_cap: dev.iob_sites().count(),
                    bram: p.brams.len(),
                    bram_cap: dev.n_bram,
                    dsp: p.macs.len(),
                    dsp_cap: dev.n_dsp,
                }),
            },
        };
        let clks = self.clocks_for_sta();
        self.timing = match (
            self.shell.session.design.as_ref(),
            self.shell.session.routed.as_ref(),
        ) {
            (Some(d), Some(r)) => report_timing_routed_xdc(d, r, &clks, &self.constraints).ok(),
            _ => None,
        };
    }

    /// Steps the console may have completed behind the rail's back (`place_design`
    /// typed by hand still lights the Place lamp).
    fn refresh_steps(&mut self) {
        let s = &self.shell.session;
        let marks = [
            (FlowStep::Synthesis, s.design.is_some()),
            (FlowStep::Place, s.placed.is_some()),
            (FlowStep::Route, s.routed.is_some()),
            (FlowStep::Bitstream, s.bitstream.is_some()),
        ];
        for (step, done) in marks {
            let i = step.index();
            if done {
                if self.steps[i] == StepState::Pending {
                    self.steps[i] = StepState::Done;
                }
            } else if self.steps[i] == StepState::Done {
                self.steps[i] = StepState::Pending;
            }
        }
    }

    fn refresh_analysis(&mut self) {
        self.refresh_schematic();
        self.refresh_device();
        self.refresh_io_ports();
        self.refresh_hierarchy();
        self.refresh_package();
        if let Ok(dev) = self.device() {
            let s = &self.shell.session;
            self.drc = match (s.design.as_ref(), s.routed.as_ref(), s.placed.as_ref()) {
                (Some(d), Some(r), _) => Some(check_routed(d, r, &dev)),
                (Some(d), None, Some(p)) => Some(check_placed(d, p, &dev)),
                _ => None,
            };
        }
    }

    fn refresh_hierarchy(&mut self) {
        let Some(d) = self.shell.session.design.as_ref() else {
            self.hierarchy = HierarchyView::default();
            return;
        };
        let mut nodes = Vec::new();
        nodes.push((d.name.clone(), "module".into()));
        for inst in &d.instances {
            nodes.push((inst.name.clone(), format!("instance:{}", inst.module)));
        }
        for c in &d.cells {
            nodes.push((c.name.clone(), primitive_of(&c.kind)));
        }
        self.hierarchy = HierarchyView {
            top: Some(d.name.clone()),
            nodes,
        };
    }

    fn refresh_package(&mut self) {
        let Ok(dev) = self.device() else {
            self.package_pins.clear();
            self.package = PackageDrawing::default();
            return;
        };
        let assigned: Vec<(String, String)> = self
            .io_ports
            .iter()
            .filter_map(|p| p.site.as_ref().map(|s| (s.clone(), p.name.clone())))
            .collect();
        self.package_pins = dev
            .iob_sites()
            .map(|s| {
                let pin = format!("IOB_X{}Y{}", s.x, s.y);
                let port = assigned
                    .iter()
                    .find(|(site, _)| site == &pin)
                    .map(|(_, n)| n.clone());
                PackagePin {
                    pin,
                    x: s.x,
                    y: s.y,
                    port,
                }
            })
            .collect();
        let (x0, y0, cols, rows) = if let (Some(xmin), Some(xmax), Some(ymin), Some(ymax)) = (
            self.package_pins.iter().map(|p| p.x).min(),
            self.package_pins.iter().map(|p| p.x).max(),
            self.package_pins.iter().map(|p| p.y).min(),
            self.package_pins.iter().map(|p| p.y).max(),
        ) {
            (xmin, ymin, xmax - xmin + 1, ymax - ymin + 1)
        } else {
            (0, 0, 0, 0)
        };
        self.package = PackageDrawing {
            part: self.part().to_string(),
            x0,
            y0,
            cols,
            rows,
        };
    }

    fn refresh_schematic(&mut self) {
        let root = self.schematic.cone_root.take();
        let depth = self.schematic.cone_depth;
        let Some(d) = self.shell.session.design.as_ref() else {
            self.schematic = SchematicView::default();
            return;
        };
        let nodes = d
            .cells
            .iter()
            .map(|c| SchematicNode {
                name: c.name.clone(),
                kind: primitive_of(&c.kind),
            })
            .collect();
        let mut edges = Vec::new();
        for n in &d.nets {
            let cells: Vec<&str> = n.endpoints.iter().map(|e| e.cell.as_str()).collect();
            for i in 0..cells.len() {
                for j in (i + 1)..cells.len() {
                    if cells[i] != cells[j] {
                        edges.push(SchematicEdge {
                            src: cells[i].to_string(),
                            dst: cells[j].to_string(),
                            net: n.name.clone(),
                        });
                    }
                }
            }
        }
        self.schematic = SchematicView {
            nodes,
            edges,
            cone_root: root,
            cone_depth: depth,
        };
    }

    fn refresh_device(&mut self) {
        let Ok(dev) = self.device() else {
            self.device = DeviceView::default();
            return;
        };
        let mut occupants: Vec<((u32, u32), String)> = Vec::new();
        if let Some(pl) = self.shell.session.placed.as_ref() {
            for (i, (site, _ble)) in pl.lutff_sites.iter().enumerate() {
                if let Some(lf) = pl.packed.lutffs.get(i) {
                    occupants.push(((site.x, site.y), lf.lut_cell.clone()));
                }
            }
            for (i, site) in pl.iob_sites.iter().enumerate() {
                if let Some(iob) = pl.packed.iobs.get(i) {
                    occupants.push(((site.x, site.y), iob.cell.clone()));
                }
            }
        }
        let mut sites = Vec::new();
        for s in dev.clb_sites() {
            let occupant = occupants
                .iter()
                .find(|((x, y), _)| *x == s.x && *y == s.y)
                .map(|(_, n)| n.clone());
            sites.push(DeviceSiteView {
                x: s.x,
                y: s.y,
                kind: s.kind,
                occupant,
            });
        }
        for s in dev.iob_sites() {
            let occupant = occupants
                .iter()
                .find(|((x, y), _)| *x == s.x && *y == s.y)
                .map(|(_, n)| n.clone());
            sites.push(DeviceSiteView {
                x: s.x,
                y: s.y,
                kind: s.kind,
                occupant,
            });
        }
        self.device = DeviceView {
            cols: dev.interior_cols,
            rows: dev.interior_rows,
            sites,
        };
    }

    fn refresh_io_ports(&mut self) {
        let Some(d) = self.shell.session.design.as_ref() else {
            self.io_ports.clear();
            return;
        };
        let placed_iobs = self.shell.session.placed.as_ref().map(|p| {
            p.iob_sites
                .iter()
                .zip(p.packed.iobs.iter())
                .map(|(s, i)| (i.cell.clone(), format!("IOB_X{}Y{}", s.x, s.y)))
                .collect::<Vec<_>>()
        });
        self.io_ports = d
            .ports
            .iter()
            .map(|p| {
                let dir = match p.dir {
                    PortDir::In => "IN",
                    PortDir::Out => "OUT",
                    PortDir::Inout => "INOUT",
                };
                let site = placed_iobs.as_ref().and_then(|v| {
                    // Match output port to IOB cell by net name or first IOB for `led`.
                    v.iter().find(|(cell, _)| {
                        cell.contains(&p.name)
                            || (p.name == "led" && cell.contains("iob"))
                    })
                    .map(|(_, s)| s.clone())
                    .or_else(|| {
                        if p.dir == PortDir::Out {
                            v.first().map(|(_, s)| s.clone())
                        } else {
                            None
                        }
                    })
                });
                IoPortView {
                    name: p.name.clone(),
                    dir: dir.into(),
                    site,
                }
            })
            .collect();
    }

    fn refresh_properties(&mut self) {
        let Some(id) = self.selected.clone() else {
            self.properties.clear();
            return;
        };
        let mut props = vec![("NAME".into(), id.clone())];
        if let Some(d) = self.shell.session.design.as_ref() {
            if let Some(c) = d.cells.iter().find(|c| c.name == id) {
                props.push(("PRIMITIVE".into(), primitive_of(&c.kind)));
                match &c.kind {
                    CellKind::Lut6 { init } => {
                        props.push(("INIT".into(), format!("{init:#018x}")));
                    }
                    CellKind::Ila { net } => {
                        props.push(("ILA_NET".into(), net.clone()));
                    }
                    _ => {}
                }
                for (k, v) in &c.attrs.map {
                    props.push((k.clone(), v.clone()));
                }
            }
            if let Some(n) = d.nets.iter().find(|n| n.name == id) {
                props.push(("TYPE".into(), "net".into()));
                props.push(("ENDPOINTS".into(), n.endpoints.len().to_string()));
            }
            if let Some(p) = d.ports.iter().find(|p| p.name == id) {
                props.push(("TYPE".into(), "port".into()));
                props.push((
                    "DIR".into(),
                    match p.dir {
                        PortDir::In => "IN".into(),
                        PortDir::Out => "OUT".into(),
                        PortDir::Inout => "INOUT".into(),
                    },
                ));
            }
        }
        if let Some(site) = self.device.occupant_of(&id) {
            props.push((
                "LOC".into(),
                format!("X{}Y{} {:?}", site.x, site.y, site.kind),
            ));
        }
        if let Some(p) = self
            .package_pins
            .iter()
            .find(|p| p.pin == id || p.port.as_deref() == Some(id.as_str()))
        {
            props.push(("PACKAGE_PIN".into(), p.pin.clone()));
            props.push(("BANK_XY".into(), format!("X{}Y{}", p.x, p.y)));
            if p.pin == id {
                props.push(("TYPE".into(), "package_pin".into()));
                if let Some(port) = &p.port {
                    props.push(("PORT".into(), port.clone()));
                }
            }
        }
        self.properties = props;
    }

    fn refresh_hw(&mut self) {
        self.hw.open = self.shell.session.hw_open;
        self.hw.programmed = self.shell.session.programmed;
        if self.hw.programmed && self.hw.stat.is_none() {
            if let (Ok(dev), Some(bits)) = (
                self.device(),
                self.shell.session.bitstream.as_ref(),
            ) {
                if let Ok(st) = helion_hw::prog_sim(&dev, bits) {
                    self.hw.stat = Some(st);
                }
            }
        }
    }

    fn refresh_runs(&mut self) {
        let part = self.part().to_string();
        let top = self.tree.top.clone();
        let cells = self.shell.session.design.as_ref().map(|d| d.cells.len());
        let lutff = self.utilization.map(|u| u.lutff);
        let synth_done = self.shell.session.design.is_some();
        let impl_done = self.shell.session.bitstream.is_some();
        let placed = self.shell.session.placed.is_some();
        let wns = self.wns_ps();
        let hash = self.bitstream_hash();
        if let Some(r) = self.runs.iter_mut().find(|r| r.name == "synth_1") {
            r.status = if synth_done {
                "Complete".into()
            } else {
                "Not started".into()
            };
            r.wns_ps = None;
            r.cells = cells;
            r.lutff = lutff;
            r.part = part.clone();
            r.top = top.clone();
            r.bitstream_hash = None;
        }
        if let Some(r) = self.runs.iter_mut().find(|r| r.name == "impl_1") {
            r.status = if impl_done {
                "Complete".into()
            } else if placed {
                "Running".into()
            } else {
                "Not started".into()
            };
            r.wns_ps = wns;
            r.cells = cells;
            r.lutff = lutff;
            r.part = part;
            r.top = top;
            r.bitstream_hash = hash;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples")
            .join(name)
    }

    /// The console widget must be a pipe onto the real engines: `report_timing` has to
    /// come back with a *numeric* WNS that differs per design, not a canned string.
    #[test]
    fn console_routes_commands_through_the_real_session() {
        let mut ide = IdeModel::new();
        ide.input = format!("read_sv {}", example("counter.sv").display());
        let r = ide.submit_input().expect("input submitted").unwrap();
        assert!(r.contains("cells"), "{r}");
        assert!(ide.input.is_empty(), "submit must clear the input box");

        let out = ide.exec("report_timing").unwrap();
        let wns: i64 = out
            .split_whitespace()
            .find_map(|t| t.strip_prefix("WNS_PS="))
            .expect("report_timing must print WNS_PS=")
            .parse()
            .expect("WNS must be numeric, not a canned string");
        assert!(wns != 0, "WNS must come from STA, not a dummy zero");
        assert!(
            wns.abs() < 100_000,
            "WNS_PS={wns} is not a picosecond slack from the STA engine"
        );
        assert_eq!(ide.wns_ps(), Some(wns), "pane must agree with the console");
        let engine = ide.session().report_timing(&ide.device().unwrap()).unwrap();
        assert!(engine.contains(&format!("WNS_PS={wns}")), "console is the Session: {engine}");

        let util = ide.exec("report_utilization").unwrap();
        assert!(util.contains("LUTFF=4/8192"), "{util}");
        assert_eq!(ide.utilization.unwrap().lutff, 4);
        assert_eq!(ide.utilization.unwrap().lutff_cap, 8192);

        // A different design must produce a different real number.
        let mut blinky = IdeModel::new();
        blinky
            .exec(&format!("read_sv {}", example("blinky.sv").display()))
            .unwrap();
        blinky.exec("report_timing").unwrap();
        let bw = blinky.wns_ps().expect("blinky WNS from STA");
        assert_ne!(Some(bw), ide.wns_ps(), "WNS is per-design, not canned");

        let fm = ide.exec("report_featuremap").unwrap();
        assert!(fm.contains("featuremap part=HL10T-C32-1"), "{fm}");
        assert!(fm.contains("BLE0.LUT.INIT[0] minor 0 bit 0"), "{fm}");
        assert!(!fm.contains("MISSING"), "{fm}");

        let bad = ide.exec("no_such_command");
        assert!(bad.is_err());
        assert!(!ide.console.last().unwrap().ok, "errors are journaled too");
        assert!(
            ide.console.iter().any(|l| l.cmd == "report_timing"),
            "console keeps the Tcl journal"
        );
    }

    /// The rail is a state machine over the real Session, not a row of lamps.
    #[test]
    fn flow_rail_refuses_route_before_place_and_exposes_bitstream_hash() {
        let mut ide = IdeModel::new();
        assert!(
            ide.run_step(FlowStep::Synthesis).is_err(),
            "no source: synthesis must refuse"
        );
        ide.open_source(&example("counter.sv")).unwrap();
        assert_eq!(ide.step_state(FlowStep::Synthesis), StepState::Done);

        let e = ide.run_step(FlowStep::Route).unwrap_err();
        assert!(e.contains("Place first"), "{e}");
        assert_eq!(ide.step_state(FlowStep::Route), StepState::Failed);
        assert!(ide.session().routed.is_none(), "refusal must not route");

        let e = ide.run_step(FlowStep::Bitstream).unwrap_err();
        assert!(e.contains("Route first"), "{e}");
        assert!(ide.bitstream_hash().is_none(), "no bitstream, no hash");

        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        assert!(ide.session().routed.is_none(), "place must not route");
        assert_eq!(ide.utilization.unwrap().lutff, 4, "util pane after place");
        assert!(ide.timing.is_none(), "timing needs a routed design");

        ide.run_step(FlowStep::Route).unwrap();
        assert_eq!(ide.step_state(FlowStep::Route), StepState::Done);
        assert!(ide.session().bitstream.is_none(), "route must not bitgen");
        let wns = ide.wns_ps().expect("timing pane fed by the STA engine");
        let engine = ide.session().report_timing(&ide.device().unwrap()).unwrap();
        assert!(engine.contains(&format!("WNS_PS={wns}")), "{engine}");
        assert!(wns != 0, "WNS is a real STA number, not a placeholder");

        let out = ide.run_step(FlowStep::Bitstream).unwrap();
        let hash = ide.bitstream_hash().expect("hash after write_bitstream");
        assert!(out.contains(&format!("{hash:#010x}")), "{out}");
        assert!(ide.bitstream_frames().unwrap() > 0);

        // Same hash as an independent Session implemented the CLI way.
        let dev = ide.device().unwrap();
        let mut ref_sess = Session::new(Mode::NonProject);
        let d = helion_sv::synth_sv_path(&example("counter.sv")).unwrap();
        ref_sess.impl_design(d, &dev).unwrap();
        assert_eq!(
            ide.bitstream_hash(),
            ref_sess.blinky_hash(),
            "rail bitstream must equal the engine bitstream"
        );

        // Re-synthesis invalidates downstream state and the panes.
        ide.run_step(FlowStep::Synthesis).unwrap();
        assert_eq!(ide.step_state(FlowStep::Route), StepState::Pending);
        assert!(ide.bitstream_hash().is_none());
        assert!(ide.timing.is_none());
    }

    /// The tree is the elaborated HNF netlist, not a static placeholder.
    #[test]
    fn tree_reflects_real_hnf_cells_and_nets() {
        let mut ide = IdeModel::new();
        assert!(ide.tree.is_empty(), "empty until a design exists");
        ide.open_source(&example("counter.sv")).unwrap();
        let d = ide.design().cloned().expect("design after synth");

        assert_eq!(ide.tree.top.as_deref(), Some(d.name.as_str()));
        assert_eq!(ide.tree.cells.len(), d.cells.len());
        assert_eq!(ide.tree.nets, get_nets(&d, None));
        assert!(ide.tree.has_cell("u_lut0"), "{:?}", ide.tree.cells);
        assert_eq!(ide.tree.kind_of("u_lut0"), Some("LUT6"));
        assert!(ide.tree.nets.contains(&"cnt_3".to_string()));
        assert!(
            ide.tree.sources.iter().any(|s| s.ends_with("counter.sv")),
            "{:?}",
            ide.tree.sources
        );
        assert!(
            ide.tree.cells.iter().any(|(_, k)| k == "IOB_OUT"),
            "IOB primitive must show: {:?}",
            ide.tree.cells
        );

        // A netlist edit through the console shows up in the tree.
        let before = ide.tree.cells.len();
        ide.exec("mark_debug cnt_3").unwrap();
        assert!(
            ide.tree.cells.len() > before,
            "inserted ILA must appear in the tree"
        );
        assert!(ide.tree.cells.iter().any(|(_, k)| k == "ILA"));

        // Loading a different design replaces the tree.
        ide.open_source(&example("blinky.sv")).unwrap();
        assert!(!ide.tree.has_cell("u_lut0"));
        assert!(ide.tree.has_cell("u_lut"));
    }

    #[test]
    fn navigator_and_layouts_journal_tcl_onto_session() {
        let mut ide = IdeModel::new();
        assert_eq!(ide.nav, NavSection::ProjectManager);
        assert_eq!(ide.layout, LayoutKind::Default);
        ide.set_nav(NavSection::Simulation).unwrap();
        assert_eq!(ide.nav, NavSection::Simulation);
        assert_eq!(ide.layout, LayoutKind::Simulation);
        assert!(
            ide.console.iter().any(|l| l.cmd.contains("nav") && l.ok),
            "nav must journal Tcl: {:?}",
            ide.console
        );
        ide.set_layout(LayoutKind::Default).unwrap();
        assert_eq!(ide.layout, LayoutKind::Default);
        assert!(ide.console.iter().any(|l| l.cmd.contains("layout default")));

        ide.set_nav(NavSection::ProgramDebug).unwrap();
        assert!(ide.session().hw_open, "Program and Debug opens the sim cable");
        assert!(ide.hw.open);

        ide.set_nav(NavSection::IpIntegrator).unwrap();
        assert!(
            ide.ip_catalog.iter().any(|c| c.name == "h_uart" && c.bus == "Helion-MM"),
            "{:?}",
            ide.ip_catalog
        );
        assert!(
            ide.ip_catalog.iter().any(|c| c.name == "h_gpio"),
            "catalog is helion-ipxact, not a placeholder"
        );
    }

    #[test]
    fn analysis_views_share_selection_of_real_counter_cell() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        assert!(
            ide.schematic.has_cell("u_lut0"),
            "schematic is HNF, not empty: {:?}",
            ide.schematic.nodes.iter().map(|n| &n.name).collect::<Vec<_>>()
        );
        assert!(!ide.schematic.edges.is_empty(), "schematic nets are real HNF edges");
        ide.select("u_lut0");
        assert_eq!(ide.selected_cell(), Some("u_lut0"));
        assert!(ide.netlist_has_selected(), "netlist must see u_lut0");
        assert!(ide.schematic_has_selected(), "schematic must see u_lut0");
        assert_eq!(ide.properties_name(), Some("u_lut0"));
        assert!(ide.hierarchy.has("u_lut0"), "{:?}", ide.hierarchy.nodes);
        assert_eq!(ide.hierarchy.top.as_deref(), Some("counter"));
        assert!(ide.hierarchy_has_selected());
        assert!(
            ide.properties.iter().any(|(k, v)| k == "PRIMITIVE" && v == "LUT6"),
            "{:?}",
            ide.properties
        );
        let found = ide.exec("find u_lut0").unwrap();
        assert!(found.contains("cell:u_lut0"), "{found}");
        assert!(
            ide.find_results.iter().any(|h| h.kind == "cell" && h.name == "u_lut0"),
            "{:?}",
            ide.find_results
        );

        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        assert!(
            ide.device.occupant_of("u_lut0").is_some(),
            "placed u_lut0 must occupy a HAD site: {:?}",
            ide.device
                .sites
                .iter()
                .filter(|s| s.occupant.is_some())
                .map(|s| (s.x, s.y, s.occupant.clone()))
                .collect::<Vec<_>>()
        );
        assert!(ide.device_has_selected(), "device view shares the same selected cell");
        assert!(
            !ide.device.sites.is_empty(),
            "device sites come from HAD, not a dummy graph"
        );
        assert!(ide.io_ports.iter().any(|p| p.name == "led"));
        assert!(
            ide.io_ports.iter().any(|p| p.name == "led" && p.site.is_some()),
            "I/O Ports list a real IOB site after place: {:?}",
            ide.io_ports
        );
        assert!(
            !ide.package_pins.is_empty(),
            "package pins are HAD IOB sites"
        );
        assert!(
            ide.package_pins.iter().any(|p| p.port.as_deref() == Some("led")),
            "led must map to a package pin: {:?}",
            ide.package_pins.iter().filter(|p| p.port.is_some()).collect::<Vec<_>>()
        );

        let drc = ide.exec("report_drc").unwrap();
        assert!(drc.contains("violations=0") || drc.contains("ok"), "{drc}");
        assert!(ide.drc.as_ref().map(|d| d.ok()).unwrap_or(false), "DRC is helion-drc");
    }

    #[test]
    fn sim_wave_led_matches_fabric_sixteen_cycles() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        ide.run_step(FlowStep::Bitstream).unwrap();

        let gold = ide.fabric_led_bits(16).expect("independent fabric LED bits");
        assert_eq!(gold.len(), 16, "{gold}");
        // Must be the engine string, not a canned constant: a second design differs.
        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        blinky.run_step(FlowStep::Bitstream).unwrap();
        let gold_b = blinky.fabric_led_bits(16).unwrap();
        assert_ne!(gold, gold_b, "LED wave is per-design from fabric");

        let out = ide.sim_run(16).unwrap();
        assert!(out.contains("LED[16]="), "{out}");
        let wave = ide.wave.bits_of("led").expect("wave has led trace");
        assert_eq!(wave, gold, "UG900 wave samples the fabric LED net");
        assert!(ide.scopes.iter().any(|s| s.name == "counter"), "{:?}", ide.scopes);
        assert!(ide.objects.iter().any(|o| o.name == "led"), "{:?}", ide.objects);

        ide.sim_restart().unwrap();
        assert!(
            ide.wave.bits_of("led").map(|s| s.is_empty()).unwrap_or(true),
            "restart clears samples"
        );
        ide.sim_step().unwrap();
        ide.sim_step().unwrap();
        let two = ide.wave.bits_of("led").unwrap();
        assert_eq!(two, &gold[..2], "step is a real cycle, not a dummy");
    }

    #[test]
    fn wave_name_value_analog_radix_from_engine_samples() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        ide.run_step(FlowStep::Bitstream).unwrap();
        let gold = ide.fabric_led_bits(16).unwrap();
        ide.sim_run(16).unwrap();

        let led = ide.wave.trace("led").expect("led wave object");
        assert_eq!(led.name, "led");
        assert_eq!(led.samples.len(), 16);
        assert_eq!(led.analog_series().len(), 16);
        for (i, y) in led.analog_series().iter().enumerate() {
            let bit = gold.as_bytes()[i] == b'1';
            assert_eq!(*y, if bit { 1.0 } else { 0.0 }, "analog Y is the engine bit");
        }
        assert!(
            led.has_digital_transition(),
            "digital 0↔1 when gold LED has both: {gold}"
        );
        assert!(
            ide.wave.cursor < 16,
            "main cursor indexes a sample: {}",
            ide.wave.cursor
        );
        assert_eq!(ide.wave.time_ps(1), ide.clock_period_ps);
        assert_eq!(ide.wave.timescale_ps, 10_000);

        let before = led.samples.clone();
        ide.exec("wave_radix led binary").unwrap();
        let bin = ide.wave.trace("led").unwrap().value_at(ide.wave.cursor);
        ide.exec("wave_radix led hex").unwrap();
        let hex = ide.wave.trace("led").unwrap().value_at(ide.wave.cursor);
        assert_ne!(bin, hex, "Binary vs Hex format the same bits differently: {bin} {hex}");
        assert_eq!(
            ide.wave.trace("led").unwrap().samples,
            before,
            "radix must not mutate engine samples"
        );
        ide.exec("wave_style led analog").unwrap();
        assert_eq!(ide.wave.trace("led").unwrap().style, WaveStyle::Analog);
        ide.exec("wave_style led digital").unwrap();
        assert_eq!(ide.wave.bits_of("led").as_deref(), Some(gold.as_str()));

        assert!(ide.wave.has_trace("cnt"), "packed LUTFF bus from fabric Q");
        let cnt = ide.wave.trace("cnt").unwrap();
        assert!(cnt.width > 1, "cnt is a bus, not a scalar dump");
        assert_eq!(cnt.analog_series().len(), 16);
        let ymax = cnt.analog_series().iter().cloned().fold(0.0, f64::max);
        assert!(ymax > 1.0, "analog bus is the integer series, not a canned sine: {ymax}");

        ide.exec("add_wave led").unwrap();
        assert!(ide.scopes.iter().any(|s| s.name == "counter"));
        assert!(ide.objects.iter().any(|o| o.name == "led"));
    }

    #[test]
    fn ultrafast_stages_open_engine_backed_panes() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        ide.run_step(FlowStep::Bitstream).unwrap();
        ide.sim_run(16).unwrap();
        ide.exec("open_hw_manager").unwrap();
        ide.exec("program_hw").unwrap();

        for stage in UltraFastStage::ALL {
            let out = ide.open_ultrafast(stage.tcl()).unwrap();
            assert!(out.contains("nav"), "{out}");
            let pane = ide.ultrafast_pane_engine(stage).unwrap();
            match stage {
                UltraFastStage::BoardDevice => {
                    assert!(pane.contains("sites="), "{pane}");
                    assert!(pane.contains("pins="), "{pane}");
                    assert!(ide.device.sites.len() > 0);
                    assert!(ide.io_ports.iter().any(|p| p.site.is_some()), "{:?}", ide.io_ports);
                    assert!(!ide.package_pins.is_empty());
                }
                UltraFastStage::DesignEntry => {
                    assert!(pane.contains("cells="), "{pane}");
                    assert!(ide.tree.has_cell("u_lut0"));
                }
                UltraFastStage::LogicSimulation => {
                    assert!(pane.contains("samples="), "{pane}");
                    let gold = ide.fabric_led_bits(16).unwrap();
                    assert_eq!(ide.wave.bits_of("led").as_deref(), Some(gold.as_str()));
                }
                UltraFastStage::Synthesis => {
                    assert!(pane.contains("cells="), "{pane}");
                }
                UltraFastStage::Implementation => {
                    assert!(pane.contains("occupied="), "{pane}");
                    assert!(ide.device.occupant_of("u_lut0").is_some());
                }
                UltraFastStage::TimingAnalysis => {
                    assert!(pane.contains("WNS_PS="), "{pane}");
                    let wns: i64 = pane
                        .split_whitespace()
                        .find_map(|t| t.strip_prefix("WNS_PS="))
                        .unwrap()
                        .parse()
                        .unwrap();
                    assert_ne!(wns, 0);
                    assert_eq!(ide.wns_ps(), Some(wns));
                }
                UltraFastStage::ProgramDebug => {
                    assert!(
                        pane.contains("hash=") || pane.contains("DONE="),
                        "{pane}"
                    );
                    assert!(ide.hw.open);
                }
            }
        }
    }

    #[test]
    fn hw_ila_eco_bd_are_engine_backed() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        ide.run_step(FlowStep::Bitstream).unwrap();

        let hash0 = ide.bitstream_hash().expect("hash after bits");
        let mut ref_sess = Session::new(Mode::NonProject);
        let d = helion_sv::synth_sv_path(&example("counter.sv")).unwrap();
        let dev = ide.device().unwrap();
        ref_sess.impl_design(d, &dev).unwrap();
        assert_eq!(ide.bitstream_hash(), ref_sess.blinky_hash());

        let hw = ide.exec("open_hw_manager").unwrap();
        assert!(hw.contains("sim"), "{hw}");
        let prog = ide.exec("program_hw").unwrap();
        assert!(prog.contains("DONE=1"), "{prog}");
        assert!(ide.hw.programmed);
        assert_eq!(ide.hw.stat.as_ref().map(|s| s.done), Some(true));

        let eco = ide.exec("eco u_lut0 0xAAAAAAAAAAAAAAAA").unwrap();
        assert!(eco.contains("eco"), "{eco}");
        let hash1 = ide.bitstream_hash().expect("hash after eco");
        assert_ne!(hash1, hash0, "ECO LUT INIT must change the bitstream hash");

        let ila = ide.exec("ila_capture cnt_3 8").unwrap();
        assert!(ila.contains("ila_capture"), "{ila}");
        assert!(
            ide.wave.has_trace("ila:cnt_3"),
            "ILA capture lands on the waveform: {:?}",
            ide.wave.traces.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
        let ibits = ide.wave.bits_of("ila:cnt_3").unwrap();
        assert!(
            ibits.contains('0') && ibits.contains('1'),
            "ILA samples the marked net, not a constant: {ibits}"
        );
        assert_eq!(ide.ila.bits, ibits);
        assert_eq!(ide.ila.net, "cnt_3");

        let bd = ide.exec("create_bd").unwrap();
        assert!(bd.contains("system"), "{bd}");
        let view = ide.block_design.as_ref().expect("BD view");
        assert!(view.ok, "helion-bd validate");
        assert!(view.sv.contains("h_uart"), "{}", view.sv);
        assert!(view.sv.contains("h_gpio"), "{}", view.sv);
        assert!(
            ide.ip_catalog.iter().all(|c| c.bus != "AXI"),
            "Helion-MM/ST only"
        );
    }

    /// UG893 Timing Constraints is an IdeModel pane fed by helion-sta, not a stub string.
    #[test]
    fn timing_constraints_pane_drives_sta_wns() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let wns10 = ide.wns_ps().expect("STA WNS at default 10 ns");
        assert!(
            ide.constraints_text().contains("no timing constraints"),
            "{}",
            ide.constraints_text()
        );

        let sdc = example("counter.sdc");
        let out = ide.exec(&format!("read_xdc {}", sdc.display())).unwrap();
        assert!(out.contains("PERIOD_PS=10000"), "{out}");
        assert_eq!(ide.workspace, WorkspaceTab::Constraints);
        assert!(
            ide.constraints
                .clocks
                .iter()
                .any(|c| c.source == "clk" && c.period_ps == 10_000),
            "{:?}",
            ide.constraints.clocks
        );
        assert!(
            ide.constraints_text().contains("PERIOD_PS=10000"),
            "{}",
            ide.constraints_text()
        );
        assert_eq!(
            ide.wns_ps(),
            Some(wns10),
            "counter.sdc 10 ns must match the default STA period"
        );

        let out = ide
            .exec("create_clock -period 20.000 [get_ports clk]")
            .unwrap();
        assert!(out.contains("PERIOD_PS=20000"), "{out}");
        let wns20 = ide.wns_ps().expect("STA after 20 ns create_clock");
        assert_eq!(
            wns20,
            wns10 + 10_000,
            "WNS must move with create_clock period (STA), not a canned pane: {wns10} vs {wns20}"
        );
        assert_eq!(ide.clock_period_ps, 20_000);
        assert!(
            ide.constraints
                .clocks
                .iter()
                .any(|c| c.period_ps == 20_000 && !c.generated),
            "{:?}",
            ide.constraints.clocks
        );
        let pane = ide.ultrafast_pane_engine(UltraFastStage::TimingAnalysis).unwrap();
        assert!(pane.contains(&format!("WNS_PS={wns20}")), "{pane}");
        assert!(pane.contains("clocks=1"), "{pane}");
        let rt = ide.exec("report_timing").unwrap();
        let pane_wns = ide.wns_ps().expect("pane WNS after create_clock");
        assert_eq!(pane_wns, wns20);
        assert!(
            rt.contains(&format!("WNS_PS={pane_wns}")),
            "Tcl report_timing must use IdeModel clocks, not Session 10 ns: {rt} pane={pane_wns}"
        );
    }

    /// UG893 Timing Constraints Apply: set_input/output_delay and set_false_path
    /// are pane commands that move helion-sta WNS — not labels on a stub editor.
    #[test]
    fn timing_constraints_io_delay_false_path_apply_moves_sta() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let wns0 = ide.wns_ps().expect("STA WNS before I/O delay");
        let setup0 = ide.timing.as_ref().unwrap().setup_ps;
        assert_ne!(wns0, 0);
        assert!(
            ide.constraints.input_delay_ps.is_empty()
                && ide.constraints.output_delay_ps.is_empty()
                && ide.constraints.false_paths.is_empty(),
            "{:?}",
            ide.constraints
        );

        let out = ide
            .exec("set_output_delay -clock clk 2.0 [get_ports led]")
            .unwrap();
        assert!(out.contains("output_delay=1"), "{out}");
        assert!(out.contains("DELAY_PS=2000"), "{out}");
        assert_eq!(ide.workspace, WorkspaceTab::Constraints);
        assert_eq!(ide.constraints.output_delay_ps.get("led"), Some(&2000));
        assert!(
            ide.constraints_text()
                .contains("set_output_delay led DELAY_PS=2000"),
            "{}",
            ide.constraints_text()
        );
        let wns_od = ide.wns_ps().expect("STA after output delay");
        assert_eq!(
            wns_od,
            wns0 - 2000,
            "set_output_delay 2 ns must worsen WNS by 2000 ps: {wns0} vs {wns_od}"
        );
        assert_eq!(
            ide.timing.as_ref().unwrap().setup_ps,
            setup0 + 2000,
            "output delay adds to setup"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(
            rt.contains(&format!("WNS_PS={wns_od}")),
            "report_timing must honor applied I/O delay: {rt}"
        );

        let inn = ide
            .exec("set_input_delay -clock clk 1.5 [get_ports clk]")
            .unwrap();
        assert!(inn.contains("input_delay=1"), "{inn}");
        assert!(inn.contains("DELAY_PS=1500"), "{inn}");
        assert_eq!(ide.constraints.input_delay_ps.get("clk"), Some(&1500));
        assert!(
            ide.constraints_text()
                .contains("set_input_delay clk DELAY_PS=1500"),
            "{}",
            ide.constraints_text()
        );
        let wns_id = ide.wns_ps().expect("STA after input+output delay");
        assert_eq!(
            wns_id,
            wns0 - 3500,
            "input+output delay must stack: {wns0} vs {wns_id}"
        );

        let fp = ide
            .exec("set_false_path -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        assert!(fp.contains("false_path=1"), "{fp}");
        assert!(
            ide.constraints_text().contains("set_false_path"),
            "{}",
            ide.constraints_text()
        );
        let t = ide.timing.as_ref().expect("STA after false path");
        assert_eq!(t.iob_ps, 0, "false path must drop IOB from STA");
        assert_eq!(t.setup_ps, t.r2r_ps, "false path setup is r2r only");
        let wns_fp = t.wns_ps;
        assert_ne!(
            wns_fp, wns_id,
            "false path must move WNS off the I/O-delay result"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(rt.contains(&format!("WNS_PS={wns_fp}")), "{rt}");
        assert!(rt.contains("iob_ps=0"), "{rt}");

        // Per-design: blinky + the same 2 ns output delay is not a canned WNS.
        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let b0 = blinky.wns_ps().unwrap();
        blinky
            .exec("set_output_delay -clock clk 2.0 [get_ports led]")
            .unwrap();
        let b1 = blinky.wns_ps().unwrap();
        assert_eq!(b1, b0 - 2000);
        assert_ne!(b1, wns_od, "I/O-delay WNS is per-design STA, not canned");
    }

    /// UG893 Hierarchy is HNF instances, not a restyled netlist cell list.
    #[test]
    fn hierarchy_pane_shows_hnf_instances_from_hier_sv() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("hier.sv")).unwrap();
        let d = ide.design().expect("hier synth");
        assert_eq!(d.name, "hier");
        assert!(
            d.instances.iter().any(|i| i.name == "u0" && i.module == "tog"),
            "synth must keep the instance tree on HNF: {:?}",
            d.instances
        );
        assert_eq!(ide.hierarchy.top.as_deref(), Some("hier"));
        assert!(
            ide.hierarchy
                .nodes
                .iter()
                .any(|(n, k)| n == "u0" && k == "instance:tog"),
            "{:?}",
            ide.hierarchy.nodes
        );
        let text = ide.exec("hierarchy").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Hierarchy);
        assert!(text.contains("u0:instance:tog"), "{text}");
        assert!(text.contains("top=hier"), "{text}");

        ide.select("u0");
        assert!(ide.hierarchy_has_selected());
        assert_eq!(ide.properties_name(), Some("u0"));

        ide.open_source(&example("counter.sv")).unwrap();
        assert_eq!(ide.hierarchy.top.as_deref(), Some("counter"));
        assert!(
            ide.design()
                .map(|d| d.instances.is_empty())
                .unwrap_or(false),
            "flat counter has no child instances"
        );
        assert!(!ide.hierarchy.has("u0"));
        assert!(ide.hierarchy.has("u_lut0"));
        assert_ne!(
            ide.hierarchy.top.as_deref(),
            Some("hier"),
            "loading counter must replace the hierarchy pane"
        );
    }

    /// UG893 Design Runs is a pane over Session synth/impl, not a pair of lamps.
    #[test]
    fn design_runs_pane_tracks_engine_progress() {
        let mut ide = IdeModel::new();
        let dump = ide.exec("design_runs").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Runs);
        assert!(dump.contains("synth_1 Synthesis Not started"), "{dump}");
        assert!(dump.contains("impl_1 Implementation Not started"), "{dump}");

        ide.open_source(&example("counter.sv")).unwrap();
        let synth = ide.runs.iter().find(|r| r.name == "synth_1").unwrap();
        assert_eq!(synth.status, "Complete");
        let cells = ide.design().unwrap().cells.len();
        assert_eq!(synth.cells, Some(cells));
        assert!(cells > 0, "HNF cells after synth");
        assert_eq!(synth.top.as_deref(), Some("counter"));
        assert_eq!(synth.part, ide.part());
        let impl_r = ide.runs.iter().find(|r| r.name == "impl_1").unwrap();
        assert_eq!(impl_r.status, "Not started");
        assert!(impl_r.wns_ps.is_none());
        assert!(impl_r.bitstream_hash.is_none());

        let out = ide.exec("launch_runs impl_1").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Runs);
        assert!(out.contains("impl_1 Implementation Complete"), "{out}");
        let impl_r = ide.runs.iter().find(|r| r.name == "impl_1").unwrap();
        assert_eq!(impl_r.status, "Complete");
        assert_eq!(impl_r.lutff, Some(4), "counter packed LUTFF from the util engine");
        let wns = ide.wns_ps().expect("STA after launch_runs");
        assert_eq!(impl_r.wns_ps, Some(wns));
        assert_ne!(wns, 0);
        let hash = ide.bitstream_hash().expect("hash after launch_runs");
        assert_eq!(impl_r.bitstream_hash, Some(hash));
        assert!(out.contains(&format!("WNS_PS={wns}")), "{out}");
        assert!(out.contains(&format!("hash={hash:#010x}")), "{out}");

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.exec("launch_runs impl_1").unwrap();
        let b = blinky.runs.iter().find(|r| r.name == "impl_1").unwrap();
        assert_eq!(b.lutff, Some(1), "blinky LUTFF is not a canned 4");
        assert_ne!(b.wns_ps, Some(wns), "WNS is per-design STA");
        assert_ne!(b.bitstream_hash, Some(hash), "bitstream hash is per-design");
        assert_eq!(b.top.as_deref(), Some("blinky"));
    }

    /// UG893 reset_run drops Session artifacts; relaunch reproduces the engine hash.
    #[test]
    fn design_runs_reset_clears_session_not_just_status() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.exec("launch_runs impl_1").unwrap();
        let hash = ide.bitstream_hash().expect("hash after impl");
        let wns = ide.wns_ps().expect("STA after impl");
        let cells = ide.design().unwrap().cells.len();
        assert_ne!(hash, 0, "bitstream hash is from bitgen");
        assert_ne!(wns, 0, "WNS is from STA");
        assert_eq!(ide.step_state(FlowStep::Bitstream), StepState::Done);

        let out = ide.exec("reset_runs impl_1").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Runs);
        assert!(
            out.contains("impl_1 Implementation Not started"),
            "{out}"
        );
        assert!(ide.session().bitstream.is_none(), "reset_run must drop bitstream");
        assert!(ide.session().placed.is_none(), "reset_run must drop placement");
        assert!(ide.session().routed.is_none(), "reset_run must drop routing");
        assert!(
            ide.design().is_some(),
            "reset impl_1 keeps the synth netlist"
        );
        assert_eq!(ide.design().unwrap().cells.len(), cells);
        assert!(ide.bitstream_hash().is_none());
        assert!(ide.wns_ps().is_none(), "timing pane follows Session, not a lamp");
        assert_eq!(ide.step_state(FlowStep::Place), StepState::Pending);
        assert_eq!(ide.step_state(FlowStep::Route), StepState::Pending);
        assert_eq!(ide.step_state(FlowStep::Bitstream), StepState::Pending);
        assert_eq!(ide.step_state(FlowStep::Synthesis), StepState::Done);
        let impl_r = ide.runs.iter().find(|r| r.name == "impl_1").unwrap();
        assert_eq!(impl_r.status, "Not started");
        assert!(impl_r.wns_ps.is_none());
        assert!(impl_r.bitstream_hash.is_none());
        assert_eq!(
            ide.runs.iter().find(|r| r.name == "synth_1").unwrap().status,
            "Complete"
        );

        ide.exec("launch_runs impl_1").unwrap();
        assert_eq!(
            ide.bitstream_hash(),
            Some(hash),
            "relaunch must reproduce the engine bitstream, not a canned hash"
        );
        assert_eq!(ide.wns_ps(), Some(wns));

        let out = ide.exec("reset_runs synth_1").unwrap();
        assert!(out.contains("synth_1 Synthesis Not started"), "{out}");
        assert!(ide.design().is_none(), "reset synth_1 drops HNF");
        assert_eq!(ide.step_state(FlowStep::Synthesis), StepState::Pending);
        assert_eq!(
            ide.runs.iter().find(|r| r.name == "synth_1").unwrap().status,
            "Not started"
        );
        assert_eq!(
            ide.runs.iter().find(|r| r.name == "impl_1").unwrap().status,
            "Not started"
        );
        ide.exec("launch_runs synth_1").unwrap();
        assert_eq!(ide.design().unwrap().cells.len(), cells);
        assert_eq!(ide.tree.top.as_deref(), Some("counter"));

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.exec("launch_runs impl_1").unwrap();
        let bh = blinky.bitstream_hash().expect("blinky hash");
        assert_ne!(bh, hash, "hash is per-design, not canned");
        blinky.exec("reset_runs impl_1").unwrap();
        assert!(blinky.bitstream_hash().is_none());
        blinky.exec("launch_runs impl_1").unwrap();
        assert_eq!(blinky.bitstream_hash(), Some(bh));
        assert_eq!(
            blinky.runs.iter().find(|r| r.name == "impl_1").unwrap().lutff,
            Some(1),
            "blinky LUTFF after relaunch is the pack engine, not counter's 4"
        );
    }

    /// UG893 Package is a HAD IOB drawing (x/y grid + occupancy), not a pin-name table.
    #[test]
    fn package_drawing_is_had_iob_grid_not_a_pin_table() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        let dump = ide.exec("package").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Package);
        let dev = ide.device().unwrap();
        let n = dev.iob_sites().count();
        assert!(n > 1, "HAD must list real IOB sites, not a dummy pin");
        assert_eq!(ide.package_pins.len(), n, "drawing pins are HAD IOB sites");
        assert!(
            ide.package.cols > 0 && ide.package.rows > 0,
            "drawing has a bounding box: {:?}",
            ide.package
        );
        assert_eq!(
            ide.package.cols * ide.package.rows,
            n as u32,
            "drawing cells are the HAD IOB bounding box, not a 1-column list"
        );
        assert!(dump.contains(&format!("pins={n}")), "{dump}");
        assert!(dump.contains(&format!("cols={}", ide.package.cols)), "{dump}");
        assert!(dump.contains("map="), "drawing occupancy map: {dump}");
        assert!(
            dump.contains("assigned=0"),
            "unplaced design has empty occupancy: {dump}"
        );
        for p in &ide.package_pins {
            assert!(
                dev.iob_major(p.x, p.y).is_some(),
                "{} is not a HAD IOB site",
                p.pin
            );
            assert_eq!(p.pin, format!("IOB_X{}Y{}", p.x, p.y));
        }

        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        let dump = ide.exec("package").unwrap();
        assert!(
            ide.package_pins.iter().any(|p| p.port.as_deref() == Some("led")),
            "placed led must occupy a drawing cell: {dump}"
        );
        assert!(dump.contains("assigned="), "{dump}");
        assert!(!dump.contains("assigned=0"), "place assigns I/O: {dump}");
        let map = dump
            .split("map=")
            .nth(1)
            .expect("drawing occupancy map");
        assert!(
            map.contains('L'),
            "placed led must mark the HAD occupancy map (not the part name): {dump}"
        );
        let led = ide
            .package_pins
            .iter()
            .find(|p| p.port.as_deref() == Some("led"))
            .cloned()
            .expect("led pin");
        let site = ide
            .io_ports
            .iter()
            .find(|p| p.name == "led")
            .and_then(|p| p.site.clone());
        assert_eq!(site.as_deref(), Some(led.pin.as_str()), "drawing matches placed IOB");
        assert!(
            dump.contains(&format!("{}=led", led.pin)),
            "drawing names the HAD pin, not a canned BGA ball: {dump}"
        );

        let sel = ide.exec(&format!("select_package_pin {}", led.pin)).unwrap();
        assert!(sel.contains("port=led"), "{sel}");
        assert_eq!(ide.selected_cell(), Some("led"));
        assert!(ide.package_has_selected());
        assert_eq!(ide.properties_name(), Some("led"));
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "PACKAGE_PIN" && v == &led.pin),
            "{:?}",
            ide.properties
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.exec("package").unwrap();
        let bled = blinky
            .package_pins
            .iter()
            .find(|p| p.port.as_deref() == Some("led"))
            .expect("blinky led on drawing");
        assert!(dev.iob_major(bled.x, bled.y).is_some());
        assert_eq!(blinky.package_pins.len(), n, "same HAD, different design");
        assert_eq!(blinky.package.cols, ide.package.cols);
        // Occupancy is the placed IOB, not a hard-coded ball name.
        assert!(bled.pin.starts_with("IOB_X"), "{}", bled.pin);
        assert_eq!(
            blinky
                .io_ports
                .iter()
                .find(|p| p.name == "led")
                .and_then(|p| p.site.as_deref()),
            Some(bled.pin.as_str())
        );
    }

    /// UG893 Messages + Log are first-class journals of Session/rail engines.
    #[test]
    fn messages_and_log_journal_rail_and_sta_not_just_exec() {
        let mut ide = IdeModel::new();
        assert!(ide.messages.is_empty());
        assert!(ide.log.is_empty());
        assert_eq!(ide.exec("log").unwrap(), "log empty");

        let e = ide.run_step(FlowStep::Synthesis).unwrap_err();
        assert!(e.contains("source") || e.contains("synth"), "{e}");
        assert!(
            ide.messages.iter().any(|m| {
                m.severity == MsgSeverity::Error
                    && m.id == "synth_design"
                    && m.text == e
            }),
            "rail must journal Errors: {:?}",
            ide.messages
        );
        assert!(
            ide.log.iter().any(|l| l.contains("helion% synth_design")),
            "{:?}",
            ide.log
        );

        ide.open_source(&example("counter.sv")).unwrap();
        assert!(
            ide.messages.iter().any(|m| {
                m.severity == MsgSeverity::Info && m.text.contains("cells=")
            }),
            "synth cells must land in Messages: {:?}",
            ide.messages
        );

        let e = ide.run_step(FlowStep::Route).unwrap_err();
        assert!(e.contains("Place first"), "{e}");
        assert!(
            ide.messages.iter().any(|m| {
                m.severity == MsgSeverity::Error
                    && m.id == "route_design"
                    && m.text.contains("Place first")
            }),
            "{:?}",
            ide.messages
        );

        ide.exec("report_timing").unwrap();
        let wns = ide.wns_ps().expect("STA WNS");
        assert_ne!(wns, 0);
        let dump = ide.exec("messages").unwrap();
        assert_eq!(ide.bottom_tab, BottomTab::Messages);
        assert!(dump.contains("errors="), "{dump}");
        assert!(dump.contains("ERROR [route_design]"), "{dump}");
        assert!(
            dump.contains(&format!("WNS_PS={wns}")),
            "Messages must carry STA, not a stub: {dump}"
        );

        let log = ide.exec("log").unwrap();
        assert_eq!(ide.bottom_tab, BottomTab::Log);
        assert!(log.contains("helion% report_timing"), "{log}");
        assert!(log.contains(&format!("WNS_PS={wns}")), "{log}");

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.exec("report_timing").unwrap();
        let bw = blinky.wns_ps().expect("blinky STA");
        assert_ne!(bw, wns, "WNS is per-design");
        let bmsg = blinky.exec("messages").unwrap();
        assert!(bmsg.contains(&format!("WNS_PS={bw}")), "{bmsg}");
        assert!(
            !bmsg.contains(&format!("WNS_PS={wns}")),
            "Messages are per-session, not canned: {bmsg}"
        );
        assert!(
            blinky
                .messages
                .iter()
                .any(|m| m.id == "synth_design" && m.text.contains("blinky")),
            "{:?}",
            blinky.messages
        );
    }

    /// UG893 Expand Cone is a 1-hop HNF neighborhood, not a restyled full netlist.
    #[test]
    fn schematic_expand_cone_follows_hnf_nets() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        let n_all = ide.schematic.nodes.len();
        let e_all = ide.schematic.edges.len();
        assert!(n_all > 3, "counter HNF must have several primitives");
        assert!(!ide.schematic.edges.is_empty());

        let d = ide.design().cloned().expect("synth");
        let hop1 = |cell: &str| -> std::collections::HashSet<String> {
            let mut s = std::collections::HashSet::new();
            s.insert(cell.to_string());
            for n in &d.nets {
                let cells: Vec<&str> = n.endpoints.iter().map(|e| e.cell.as_str()).collect();
                if cells.contains(&cell) {
                    for c in cells {
                        s.insert(c.to_string());
                    }
                }
            }
            s
        };
        let lut_cone = hop1("u_lut0");
        assert!(
            lut_cone.len() < n_all,
            "1-hop from u_lut0 must be a proper subset of HNF ({}/{})",
            lut_cone.len(),
            n_all
        );
        assert!(lut_cone.contains("u_lut0"));

        ide.select("u_lut0");
        let out = ide.exec("expand_cone").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Schematic);
        assert_eq!(ide.schematic.cone_root.as_deref(), Some("u_lut0"));
        let vis: std::collections::HashSet<String> = ide
            .schematic
            .visible_nodes()
            .iter()
            .map(|n| n.name.clone())
            .collect();
        assert_eq!(vis, lut_cone, "cone must match HNF net endpoints: {out}");
        assert!(out.contains("cone=u_lut0"), "{out}");
        assert!(out.contains(&format!("cells={}", lut_cone.len())), "{out}");
        assert!(ide.schematic.visible_nodes().len() < n_all);
        assert!(ide.schematic_has_selected());
        assert_eq!(ide.properties_name(), Some("u_lut0"));

        let iob = ide
            .tree
            .cells
            .iter()
            .find(|(_, k)| k == "IOB_OUT")
            .map(|(n, _)| n.clone())
            .expect("IOB primitive");
        let iob_cone = hop1(&iob);
        let out2 = ide.exec(&format!("expand_cone {iob}")).unwrap();
        assert!(out2.contains(&format!("cone={iob}")), "{out2}");
        let vis2: std::collections::HashSet<String> = ide
            .schematic
            .visible_nodes()
            .iter()
            .map(|n| n.name.clone())
            .collect();
        assert_eq!(vis2, iob_cone);
        assert_ne!(
            vis2, lut_cone,
            "cone is per-cell HNF, not a canned schematic window"
        );

        let full = ide.exec("collapse_cone").unwrap();
        assert!(ide.schematic.cone_root.is_none());
        assert_eq!(ide.schematic.visible_nodes().len(), n_all);
        assert_eq!(ide.schematic.edges.len(), e_all);
        assert!(full.contains(&format!("cells={n_all}")), "{full}");
    }

    /// UG900 ILA dashboard: trigger/window from fabric samples on the wave, not a lamp.
    #[test]
    fn ila_dashboard_trigger_window_from_fabric_samples() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        ide.run_step(FlowStep::Bitstream).unwrap();

        let win = ide.exec("ila_window 8").unwrap();
        assert!(win.contains("window=8"), "{win}");
        assert_eq!(ide.workspace, WorkspaceTab::Hardware);
        ide.exec("ila_trigger rising").unwrap();

        let out = ide.exec("ila_arm cnt_3").unwrap();
        assert!(out.contains("net=cnt_3"), "{out}");
        assert!(out.contains("samples=8"), "{out}");
        assert_eq!(ide.ila.net, "cnt_3");
        assert_eq!(ide.ila.window, 8);
        assert_eq!(ide.ila.bits.len(), 8);
        assert!(
            ide.ila.bits.contains('0') && ide.ila.bits.contains('1'),
            "capture is fabric ILA, not constant: {}",
            ide.ila.bits
        );
        let rise = ide
            .ila
            .bits
            .as_bytes()
            .windows(2)
            .position(|w| w[0] == b'0' && w[1] == b'1')
            .map(|i| i + 1);
        assert_eq!(
            ide.ila.trigger_at, rise,
            "rising trigger_at is the first 0→1 in engine bits {}",
            ide.ila.bits
        );
        if let Some(i) = rise {
            assert_eq!(ide.wave.cursor, i, "wave cursor tracks ILA trigger");
        }
        let dump = ide.exec("ila_dashboard").unwrap();
        assert!(dump.contains("trigger=rising"), "{dump}");
        assert!(dump.contains(&format!("bits={}", ide.ila.bits)), "{dump}");
        match rise {
            Some(i) => assert!(dump.contains(&format!("trigger_at={i}")), "{dump}"),
            None => assert!(dump.contains("trigger_at=-"), "{dump}"),
        }

        let fall_at = ide
            .ila
            .bits
            .as_bytes()
            .windows(2)
            .position(|w| w[0] == b'1' && w[1] == b'0')
            .map(|i| i + 1);
        let fall = ide.exec("ila_trigger falling").unwrap();
        assert_eq!(ide.ila.trigger, IlaTrigger::Falling);
        assert_eq!(ide.ila.trigger_at, fall_at);
        assert!(fall.contains("trigger=falling"), "{fall}");
        if rise.is_some() && fall_at.is_some() {
            assert_ne!(
                rise, fall_at,
                "rising vs falling must pick different engine edges: {}",
                ide.ila.bits
            );
        }

        let bits8 = ide.ila.bits.clone();
        ide.exec("ila_window 16").unwrap();
        ide.exec("ila_arm cnt_3").unwrap();
        assert_eq!(ide.ila.bits.len(), 16, "window is the capture length");
        assert!(
            ide.ila.bits.starts_with(&bits8),
            "longer window continues the same fabric stream: {} vs {bits8}",
            ide.ila.bits
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        blinky.run_step(FlowStep::Bitstream).unwrap();
        blinky.exec("ila_window 8").unwrap();
        blinky.exec("ila_arm led").unwrap();
        assert_eq!(blinky.ila.net, "led");
        assert_ne!(blinky.ila.net, ide.ila.net, "probe net is the armed HNF net");
        assert!(
            blinky.wave.has_trace("ila:led"),
            "{:?}",
            blinky.wave.traces.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
        assert!(!blinky.wave.has_trace("ila:cnt_3"));
        assert_eq!(blinky.runs.iter().find(|r| r.name == "impl_1").unwrap().lutff, Some(1));
        assert_eq!(ide.runs.iter().find(|r| r.name == "impl_1").unwrap().lutff, Some(4));
    }
}
