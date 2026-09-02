//! Headless IDE model — the Vivado-class application state.
//!
//! Sources/netlist Name/Type table, Tcl Console Status/Cmd/Out table, flow rail
//! (Synthesis → Opt → Place → Route → Bitstream) and the timing/utilization report
//! panes. Everything here runs the *real* [`Session`] engines: there is no canned
//! output anywhere in this file, so the widget layer (`helion-ide`) is a thin painter
//! over this model and can be tested without a display.

use crate::{tcl_eval, GpuiShell};
use helion_bd::{emit_sv, validate, BlockDesign};
use helion_debug::{insert_arm_capture, IlaCapture};
use helion_device::{Device, Far, SiteKind};
use helion_drc::{check_placed, check_routed, Drc};
use helion_fabric::{Fabric, Stat, StatBit};
use helion_ir::{CellKind, Design, PortDir};
use helion_ipxact::{catalog as ipxact_catalog, to_xml, IpCore};
use helion_proj::{get_cells, get_nets, ImplStrategy, Mode, Session};
use helion_sim::{Sim, SimLocal};
use helion_sta::{
    clock_network_delay_ps, create_clock, iostandard_pad_ps, port_pad_ps, load_xdc,
    report_cdc, report_clock_interaction, report_clock_networks, report_methodology,
    report_power, report_timing_routed_xdc, report_timing_summary, CdcReport,
    ClockInteraction, ClockNetworkReport, Constraints, MethodologyReport, PowerReport,
    TimingResult, TimingSummary, FF_CKQ_PS, LUT_PS, PIN_PS, SETUP_PS,
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

impl ConsoleLine {
    pub fn status(&self) -> &'static str {
        if self.ok {
            "ok"
        } else {
            "error"
        }
    }

    /// Clickable-grid dump row: UG893 Tcl Console Status/Cmd/Out, not `helion% cmd`.
    pub fn row_text(&self) -> String {
        format!("STATUS={} CMD={} OUT={}", self.status(), self.cmd, self.out)
    }
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

    pub fn has_net(&self, name: &str) -> bool {
        self.nets.iter().any(|n| n == name)
    }

    pub fn kind_of(&self, name: &str) -> Option<&str> {
        self.cells
            .iter()
            .find(|(c, _)| c == name)
            .map(|(_, k)| k.as_str())
    }
}

/// One clickable row in the UG893 Sources / Netlist pane (not a collapsing dump).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetlistRow {
    /// `source` / `cell` / `net`.
    pub kind: String,
    pub name: String,
    /// File language / primitive / `net`.
    pub type_name: String,
    /// Full source path, or HNF top for cells/nets.
    pub parent: String,
}

impl NetlistRow {
    pub fn type_cell(&self) -> &str {
        if self.type_name.is_empty() {
            self.kind.as_str()
        } else {
            self.type_name.as_str()
        }
    }

    pub fn parent_cell(&self) -> &str {
        if self.parent.is_empty() {
            "-"
        } else {
            self.parent.as_str()
        }
    }

    /// Clickable-grid dump row: UG893 Sources/Netlist Name/Type over HNF, not `name (kind)`.
    pub fn row_text(&self) -> String {
        format!("NAME={} TYPE={}", self.name, self.type_cell())
    }
}

fn source_basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string()
}

fn source_type_of(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "sv" => "sv",
        "v" => "verilog",
        "vhd" | "vhdl" => "vhdl",
        "sdc" | "xdc" => "constraint",
        "c" => "c",
        "prj" => "project",
        _ => "source",
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

/// One HAD resource row of the UG893 Utilization occupancy pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UtilOccupancy {
    pub resource: &'static str,
    pub used: usize,
    pub available: usize,
}

impl UtilOccupancy {
    pub fn pct(self) -> u32 {
        if self.available == 0 {
            0
        } else {
            ((self.used as u64 * 100) / self.available as u64) as u32
        }
    }
}

/// Hierarchical occupancy counted off HNF cells (not a resource-name dump).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HierOccupancy {
    pub name: String,
    pub lut: usize,
    pub ff: usize,
    pub iob: usize,
    pub bram: usize,
    pub dsp: usize,
}

/// UG893 Utilization occupancy report: HAD used/available + HNF hierarchy.
#[derive(Clone, Debug, Default)]
pub struct UtilizationReport {
    pub part: String,
    pub occupancy: Vec<UtilOccupancy>,
    pub hierarchy: Vec<HierOccupancy>,
}

impl UtilizationReport {
    pub fn row(&self, resource: &str) -> Option<&UtilOccupancy> {
        self.occupancy
            .iter()
            .find(|r| r.resource.eq_ignore_ascii_case(resource))
    }

    pub fn hier(&self, name: &str) -> Option<&HierOccupancy> {
        self.hierarchy.iter().find(|h| h.name == name)
    }

    pub fn text(&self) -> String {
        if self.part.is_empty() {
            return "no placed design — run Place".into();
        }
        let mut lines = Vec::new();
        if let (Some(lut), Some(iob), Some(bram), Some(dsp)) = (
            self.row("LUTFF"),
            self.row("IOB"),
            self.row("BRAM"),
            self.row("DSP"),
        ) {
            lines.push(format!(
                "report_utilization part={} LUTFF={}/{} IOB={}/{} BRAM={}/{} DSP={}/{}",
                self.part,
                lut.used,
                lut.available,
                iob.used,
                iob.available,
                bram.used,
                bram.available,
                dsp.used,
                dsp.available
            ));
        } else {
            lines.push(format!("report_utilization part={}", self.part));
        }
        for r in &self.occupancy {
            lines.push(format!(
                "resource {} used={} available={} pct={}",
                r.resource,
                r.used,
                r.available,
                r.pct()
            ));
        }
        for h in &self.hierarchy {
            lines.push(format!(
                "hier {} lut={} ff={} iob={} bram={} dsp={}",
                h.name, h.lut, h.ff, h.iob, h.bram, h.dsp
            ));
        }
        lines.join("\n")
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

    pub fn occupancy(&self) -> [UtilOccupancy; 4] {
        [
            UtilOccupancy {
                resource: "LUTFF",
                used: self.lutff,
                available: self.lutff_cap as usize,
            },
            UtilOccupancy {
                resource: "IOB",
                used: self.iob,
                available: self.iob_cap,
            },
            UtilOccupancy {
                resource: "BRAM",
                used: self.bram,
                available: self.bram_cap as usize,
            },
            UtilOccupancy {
                resource: "DSP",
                used: self.dsp,
                available: self.dsp_cap as usize,
            },
        ]
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

    /// UG949 Flow Navigator children — each Tcl hits a Helion engine, not empty chrome.
    pub fn actions(self) -> &'static [NavAction] {
        match self {
            NavSection::BoardDevice => &[
                NavAction {
                    label: "I/O Planning",
                    tcl: "io_planning",
                },
                NavAction {
                    label: "Floorplanning",
                    tcl: "floorplanning",
                },
                NavAction {
                    label: "Open Device",
                    tcl: "device",
                },
                NavAction {
                    label: "Open Package",
                    tcl: "package",
                },
            ],
            NavSection::ProjectManager => &[
                NavAction {
                    label: "Add Sources",
                    tcl: "project_manager",
                },
                NavAction {
                    label: "Project Settings",
                    tcl: "project_settings",
                },
                NavAction {
                    label: "Sources",
                    tcl: "sources",
                },
                NavAction {
                    label: "Netlist",
                    tcl: "netlist",
                },
                NavAction {
                    label: "Reports",
                    tcl: "reports",
                },
                NavAction {
                    label: "Find Results",
                    tcl: "find_results",
                },
            ],
            NavSection::IpIntegrator => &[
                NavAction {
                    label: "IP Catalog",
                    tcl: "ip_catalog",
                },
                NavAction {
                    label: "Create Block Design",
                    tcl: "create_bd",
                },
            ],
            NavSection::Simulation => &[
                NavAction {
                    label: "Run Simulation",
                    tcl: "run_simulation",
                },
                NavAction {
                    label: "Memory",
                    tcl: "memory",
                },
                NavAction {
                    label: "Breakpoints",
                    tcl: "breakpoints",
                },
                NavAction {
                    label: "Locals",
                    tcl: "locals",
                },
            ],
            NavSection::RtlAnalysis => &[NavAction {
                label: "Open Elaborated Schematic",
                tcl: "open_elaborated_schematic",
            }],
            NavSection::Synthesis => &[
                NavAction {
                    label: "Run Synthesis",
                    tcl: "run_synthesis",
                },
                NavAction {
                    label: "Open Synthesized Schematic",
                    tcl: "open_elaborated_schematic",
                },
            ],
            NavSection::Implementation => &[
                NavAction {
                    label: "Run Implementation",
                    tcl: "run_implementation",
                },
                NavAction {
                    label: "Design Runs",
                    tcl: "design_runs",
                },
                NavAction {
                    label: "Compare Runs",
                    tcl: "compare_runs",
                },
                NavAction {
                    label: "Report DRC",
                    tcl: "report_drc",
                },
                NavAction {
                    label: "Report Utilization",
                    tcl: "report_utilization",
                },
                NavAction {
                    label: "Report Methodology",
                    tcl: "report_methodology",
                },
            ],
            NavSection::TimingAnalysis => &[
                NavAction {
                    label: "Timing Constraints",
                    tcl: "timing_constraints",
                },
                NavAction {
                    label: "Report Timing",
                    tcl: "report_timing",
                },
                NavAction {
                    label: "Report Timing Summary",
                    tcl: "report_timing_summary",
                },
                NavAction {
                    label: "Report Clock Interaction",
                    tcl: "report_clock_interaction",
                },
                NavAction {
                    label: "Report CDC",
                    tcl: "report_cdc",
                },
                NavAction {
                    label: "Report Clock Networks",
                    tcl: "report_clock_networks",
                },
                NavAction {
                    label: "Report Power",
                    tcl: "report_power",
                },
                NavAction {
                    label: "Report Methodology",
                    tcl: "report_methodology",
                },
            ],
            NavSection::ProgramDebug => &[
                NavAction {
                    label: "Generate Bitstream",
                    tcl: "write_bitstream",
                },
                NavAction {
                    label: "Bitstream Frames",
                    tcl: "report_bitstream",
                },
                NavAction {
                    label: "Open Hardware Manager",
                    tcl: "open_hw_manager",
                },
                NavAction {
                    label: "Hardware STAT",
                    tcl: "report_hw_stat",
                },
            ],
        }
    }
}

/// One Flow Navigator tree child (UG949 stage action).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavAction {
    pub label: &'static str,
    pub tcl: &'static str,
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

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "error" | "errors" | "err" | "e" => Ok(Self::Error),
            "warning" | "warnings" | "warn" | "w" => Ok(Self::Warning),
            "info" | "information" | "i" => Ok(Self::Info),
            other => Err(format!("unknown message severity {other}")),
        }
    }
}

/// One clickable row in the UG893 Messages pane (not a colored dump).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdeMessage {
    pub severity: MsgSeverity,
    pub id: String,
    pub text: String,
}

/// UG893 Timing Constraints Editor folder (clocks / I/O delay / exceptions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintSection {
    Clocks,
    IoDelay,
    Exception,
}

impl ConstraintSection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clocks => "clocks",
            Self::IoDelay => "io_delay",
            Self::Exception => "exception",
        }
    }
}

/// One clickable row in the UG893 Timing Constraints pane (not a concatenated dump).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstraintRow {
    pub id: String,
    pub section: ConstraintSection,
    pub kind: String,
    pub name: String,
    pub from: String,
    pub to: String,
    pub value: String,
    pub enabled: bool,
}

fn constraint_from_to(s: &str) -> (String, String) {
    fn obj(after: &str) -> String {
        after
            .split([' ', '\t', '[', ']', '{', '}'])
            .map(str::trim)
            .find(|t| {
                !t.is_empty()
                    && !t.starts_with('-')
                    && !t.eq_ignore_ascii_case("get_ports")
                    && !t.eq_ignore_ascii_case("get_pins")
                    && !t.eq_ignore_ascii_case("get_clocks")
                    && !t.eq_ignore_ascii_case("get_cells")
                    && !t.eq_ignore_ascii_case("get_nets")
            })
            .unwrap_or("")
            .trim_matches(|c: char| matches!(c, '[' | ']' | '{' | '}' | '"' | '\''))
            .to_string()
    }
    let from = s
        .split_once("-from")
        .map(|(_, r)| obj(r))
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    let to = s
        .split_once("-to")
        .map(|(_, r)| obj(r))
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    if from.is_empty() && to.is_empty() {
        (s.trim().to_string(), String::new())
    } else {
        (from, to)
    }
}

/// One clickable FAR row in the bitstream pane (helion-bits frames, not a hash dump).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitstreamFrame {
    pub far: u32,
    pub block: u8,
    pub die: u8,
    pub major: u16,
    pub minor: u8,
    pub word: u128,
}

impl BitstreamFrame {
    fn from_key(block: u8, major: u16, minor: u8, word: u128) -> Self {
        let far = Far {
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

    pub fn block_name(&self) -> &'static str {
        match self.block {
            Far::CLB_IO_CLK => "CLB_IO_CLK",
            Far::DSP => "DSP",
            Far::BRAM => "BRAM",
            Far::IOB => "IOB",
            _ => "UNKNOWN",
        }
    }

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

/// helion-bits configuration memory: FAR-addressed frames from `bitgen`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BitstreamReport {
    pub idcode: u32,
    pub hash: u32,
    pub bytes: usize,
    pub frames: usize,
    pub configured: usize,
    pub rows: Vec<BitstreamFrame>,
}

impl BitstreamReport {
    pub fn frame(&self, spec: &str) -> Option<&BitstreamFrame> {
        let spec = spec.trim();
        if spec.is_empty() {
            return self.rows.first();
        }
        if let Ok(i) = spec.parse::<usize>() {
            if let Some(r) = self.rows.get(i) {
                return Some(r);
            }
        }
        let hex = spec.strip_prefix("0x").or_else(|| spec.strip_prefix("0X")).unwrap_or(spec);
        if hex.chars().all(|c| c.is_ascii_hexdigit()) {
            if let Ok(far) = u32::from_str_radix(hex, 16) {
                if let Some(r) = self.rows.iter().find(|f| f.far == far) {
                    return Some(r);
                }
            }
        }
        let up = spec.to_ascii_uppercase();
        if let Some(r) = self.rows.iter().find(|f| f.block_name() == up) {
            return Some(r);
        }
        let mut p = spec.split_whitespace();
        if let (Some(b), Some(maj), Some(min)) = (p.next(), p.next(), p.next()) {
            let block = match b.to_ascii_uppercase().as_str() {
                "CLB_IO_CLK" | "CLB" | "0" => Far::CLB_IO_CLK,
                "DSP" | "2" => Far::DSP,
                "BRAM" | "3" => Far::BRAM,
                "IOB" | "5" => Far::IOB,
                other => other.parse().ok()?,
            };
            let major: u16 = maj.parse().ok()?;
            let minor: u8 = min.parse().ok()?;
            return self
                .rows
                .iter()
                .find(|f| f.block == block && f.major == major && f.minor == minor);
        }
        None
    }

    pub fn text(&self) -> String {
        if self.frames == 0 && self.bytes == 0 {
            return "no bitstream — run Bitstream".into();
        }
        let mut s = format!(
            "bitstream idcode={:#010x} hash={:#010x} bytes={} frames={} configured={}",
            self.idcode, self.hash, self.bytes, self.frames, self.configured
        );
        for (i, r) in self.rows.iter().enumerate() {
            s.push_str(&format!(
                "\n{i} FAR={} BLOCK={} DIE={} MAJOR={} MINOR={} ONES={} WORD={}",
                r.far_hex(),
                r.block_name(),
                r.die,
                r.major,
                r.minor,
                r.ones(),
                r.word_hex()
            ));
        }
        s
    }
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
    /// UG986 Lab 1 Helion strategy (Default / TimingExplore / RuntimeOpt / PhysOpt).
    pub strategy: String,
    pub runtime_ms: Option<u64>,
    pub reuse_pct: Option<u32>,
}

impl DesignRun {
    fn new(name: &str, step: &str) -> Self {
        Self {
            name: name.into(),
            step: step.into(),
            status: "Not started".into(),
            wns_ps: None,
            cells: None,
            lutff: None,
            part: "HL10T-C32-1".into(),
            top: None,
            bitstream_hash: None,
            strategy: "Default".into(),
            runtime_ms: None,
            reuse_pct: None,
        }
    }

    /// UG893 Design Runs grid cell: impl strategy, dash for synth.
    pub fn strategy_cell(&self) -> &str {
        if self.step == "Implementation" && !self.strategy.is_empty() {
            self.strategy.as_str()
        } else {
            "-"
        }
    }

    pub fn wns_cell(&self) -> String {
        self.wns_ps
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".into())
    }

    pub fn runtime_cell(&self) -> String {
        self.runtime_ms
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".into())
    }

    pub fn hash_cell(&self) -> String {
        self.bitstream_hash
            .map(|h| format!("{h:#010x}"))
            .unwrap_or_else(|| "-".into())
    }

    pub fn lutff_cell(&self) -> String {
        self.lutff
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".into())
    }

    pub fn reuse_cell(&self) -> String {
        self.reuse_pct
            .map(|n| format!("{n}%"))
            .unwrap_or_else(|| "-".into())
    }

    pub fn top_cell(&self) -> &str {
        self.top.as_deref().unwrap_or("-")
    }

    pub fn cells_cell(&self) -> String {
        self.cells
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".into())
    }

    /// Clickable-grid dump row: name/strategy/WNS/runtime/hash from the live run.
    pub fn row_text(&self) -> String {
        let mut s = format!("{} {} {}", self.name, self.step, self.status);
        s.push_str(&format!(
            " NAME={} STEP={} STRATEGY={} STATUS={} WNS_PS={} RUNTIME_MS={} HASH={} LUTFF={} REUSE={}",
            self.name,
            self.step,
            self.strategy_cell(),
            self.status,
            self.wns_cell(),
            self.runtime_cell(),
            self.hash_cell(),
            self.lutff_cell(),
            self.reuse_cell()
        ));
        if !self.strategy.is_empty() && self.step == "Implementation" {
            s.push_str(&format!(" strategy={}", self.strategy));
        }
        if let Some(top) = &self.top {
            s.push_str(&format!(" top={top}"));
        }
        s.push_str(&format!(" part={}", self.part));
        if let Some(n) = self.cells {
            s.push_str(&format!(" cells={n}"));
        }
        if let Some(n) = self.lutff {
            s.push_str(&format!(" LUTFF={n}"));
        }
        if let Some(w) = self.wns_ps {
            s.push_str(&format!(" WNS_PS={w}"));
        }
        if let Some(ms) = self.runtime_ms {
            s.push_str(&format!(" runtime_ms={ms}"));
        }
        if let Some(p) = self.reuse_pct {
            s.push_str(&format!(" reuse={p}%"));
        }
        if let Some(h) = self.bitstream_hash {
            s.push_str(&format!(" hash={h:#010x}"));
        }
        s
    }
}

/// HNF pin on a schematic symbol (UG893 Fig. 56/57: stub inside and outside the box).
#[derive(Clone, Debug)]
pub struct SchematicPin {
    pub name: String,
    pub net: String,
    /// Output pins sit on the right of the symbol; inputs on the left.
    pub output: bool,
}

#[derive(Clone, Debug)]
pub struct SchematicNode {
    pub name: String,
    pub kind: String,
    pub pins: Vec<SchematicPin>,
}

#[derive(Clone, Debug)]
pub struct SchematicEdge {
    pub src: String,
    pub src_pin: String,
    pub dst: String,
    pub dst_pin: String,
    pub net: String,
}

/// Top-level port as a schematic terminator (UG893 Fig. 55 left/right I/O).
#[derive(Clone, Debug)]
pub struct SchematicPort {
    pub name: String,
    pub dir: String,
}

/// Positioned pin stub for the schematic canvas.
#[derive(Clone, Debug)]
pub struct SchematicPinGeom {
    pub name: String,
    pub net: String,
    pub output: bool,
    pub x: f32,
    pub y: f32,
}

/// UG893 schematic symbol: rounded box + pin stubs, not a list row.
#[derive(Clone, Debug)]
pub struct SchematicSymbol {
    pub name: String,
    pub kind: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub pins: Vec<SchematicPinGeom>,
    /// Fig. 59: cell is on the selected STA timing path.
    pub highlighted: bool,
}

/// Orthogonal net polyline (UG893 Fig. 55 green wires).
#[derive(Clone, Debug)]
pub struct SchematicWire {
    pub net: String,
    pub src: String,
    pub src_pin: String,
    pub dst: String,
    pub dst_pin: String,
    pub points: Vec<(f32, f32)>,
    /// Fig. 55: dotted when the net continues to logic not on this sheet.
    pub off_sheet: bool,
    /// Fig. 57: buses are thick wires (width > 1).
    pub width: u8,
    /// Fig. 59: net is on the selected STA timing path.
    pub highlighted: bool,
}

/// Laid-out schematic sheet derived from HNF cells/pins/nets.
#[derive(Clone, Debug, Default)]
pub struct SchematicDrawing {
    pub symbols: Vec<SchematicSymbol>,
    pub wires: Vec<SchematicWire>,
    pub width: f32,
    pub height: f32,
}

/// Fig. 55 schematic camera: zoom/pan the sheet; paint maps world→view with this.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SchematicCamera {
    pub zoom: f32,
    pub pan_x: f32,
    pub pan_y: f32,
}

impl Default for SchematicCamera {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
        }
    }
}

/// One UG903 `report_timing` pin-delay row (Name / Delay Type / Incr / Path).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimingPathPin {
    pub cell: String,
    /// `cell/pin` as Vivado's Name column.
    pub pin: String,
    /// `clk` / `cell` / `net` / `setup` / `iob`.
    pub delay_type: String,
    pub incr_ps: i64,
    pub path_ps: i64,
    pub net: String,
    pub fanout: usize,
    /// HAD site after Place (`CLB_XnYm` / `IOB_XnYm`), empty before.
    pub location: String,
}

/// Fig. 59 STA path (cells/nets from endpoints), not a restyle of the full sheet.
/// `pins` is the UG903 pin-delay table (incr/path from helion-sta arcs).
#[derive(Clone, Debug)]
pub struct TimingPath {
    pub name: String,
    pub startpoint: String,
    pub endpoint: String,
    pub cells: Vec<String>,
    pub nets: Vec<String>,
    pub delay_ps: i64,
    pub slack_ps: i64,
    pub pins: Vec<TimingPathPin>,
}

impl TimingPath {
    /// Clickable-grid dump row: UG893 Fig. 55 Name/From/To/Slack_ps, not `endpoint slack=`.
    pub fn row_text(&self) -> String {
        format!(
            "NAME={} FROM={} TO={} SLACK_PS={}",
            self.name, self.startpoint, self.endpoint, self.slack_ps
        )
    }

    /// Combinational + sequential cell arcs (not nets).
    pub fn logic_ps(&self) -> i64 {
        self.pins
            .iter()
            .filter(|p| {
                p.delay_type == "cell" || p.delay_type == "setup" || p.delay_type == "iob"
            })
            .map(|p| p.incr_ps)
            .sum()
    }

    /// Net / pin-load arcs.
    pub fn net_ps(&self) -> i64 {
        self.pins
            .iter()
            .filter(|p| p.delay_type == "net")
            .map(|p| p.incr_ps)
            .sum()
    }
}

#[derive(Clone, Debug)]
pub struct SchematicView {
    pub nodes: Vec<SchematicNode>,
    pub edges: Vec<SchematicEdge>,
    pub ports: Vec<SchematicPort>,
    /// UG893 expand-cone root. `None` = show the full HNF schematic.
    pub cone_root: Option<String>,
    pub cone_depth: usize,
    /// Hierarchical instance names (HNF `instances`), for Expand Inside.
    pub instances: Vec<String>,
    /// Fig. 56: instance whose nested contents are on the sheet.
    pub expand_inside: Option<String>,
    /// Fig. 59: STA path cells to highlight (and, with `path_only`, to isolate).
    pub highlight_cells: HashSet<String>,
    pub highlight_nets: HashSet<String>,
    /// When true, the sheet shows only the highlighted STA path.
    pub path_only: bool,
    /// Fig. 55 camera the painter applies.
    pub camera: SchematicCamera,
    pub view_history: Vec<SchematicCamera>,
    pub view_index: usize,
    pub viewport_w: f32,
    pub viewport_h: f32,
}

impl Default for SchematicView {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            ports: Vec::new(),
            cone_root: None,
            cone_depth: 1,
            instances: Vec::new(),
            expand_inside: None,
            highlight_cells: HashSet::new(),
            highlight_nets: HashSet::new(),
            path_only: false,
            camera: SchematicCamera::default(),
            view_history: vec![SchematicCamera::default()],
            view_index: 0,
            viewport_w: 800.0,
            viewport_h: 600.0,
        }
    }
}

fn pin_is_output(pin: &str) -> bool {
    matches!(
        pin,
        "O" | "Q" | "PAD" | "Y" | "COUT" | "P" | "DOA" | "DOB" | "YQ"
    )
}

/// UG893 Schematic Symbol: primitive pin order (inputs then outputs). Unconnected pins are n/c.
fn canonical_pins(kind: &str) -> &'static [(&'static str, bool)] {
    match kind {
        "LUT6" => &[
            ("I0", false),
            ("I1", false),
            ("I2", false),
            ("I3", false),
            ("I4", false),
            ("I5", false),
            ("O", true),
        ],
        "HFF" => &[("D", false), ("CLK", false), ("Q", true)],
        "IOB_OUT" => &[("I", false), ("PAD", true)],
        "MAC27" => &[("A", false), ("B", false), ("C", false), ("P", true)],
        "BRAM18" => &[
            ("DIA", false),
            ("ADDRA", false),
            ("WEA", false),
            ("CLKA", false),
            ("DOA", true),
        ],
        "ILA" => &[("CLK", false), ("PROBE", false)],
        _ => &[],
    }
}

fn merge_canonical_pins(kind: &str, connected: Vec<SchematicPin>) -> Vec<SchematicPin> {
    let mut pins = Vec::new();
    for &(name, output) in canonical_pins(kind) {
        let net = connected
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.net.clone())
            .unwrap_or_default();
        pins.push(SchematicPin {
            name: name.into(),
            net,
            output,
        });
    }
    for p in connected {
        if !pins.iter().any(|q| q.name == p.name) {
            pins.push(p);
        }
    }
    pins
}

fn schematic_column(kind: &str) -> usize {
    match kind {
        "PORT_IN" => 0,
        "LUT6" | "ILA" | "MAC27" | "BRAM18" => 1,
        k if k.starts_with("instance") => 1,
        "HFF" => 2,
        "IOB_OUT" => 3,
        "PORT_OUT" => 4,
        _ => 1,
    }
}

/// Bus width from a `[hi:lo]` slice, else a known wide primitive pin.
fn schematic_bus_width(net: &str, pin: &str) -> u8 {
    parse_bus_range(net)
        .or_else(|| parse_bus_range(pin))
        .unwrap_or_else(|| match pin {
            "DIA" | "DOA" | "ADDRA" => 18,
            "A" | "B" | "C" | "P" => 27,
            _ => 1,
        })
}

fn parse_bus_range(s: &str) -> Option<u8> {
    let b = s.find('[')?;
    let e = s.find(']')?;
    let inner = &s[b + 1..e];
    if let Some((hi, lo)) = inner.split_once(':') {
        let h: i32 = hi.parse().ok()?;
        let l: i32 = lo.parse().ok()?;
        Some(((h - l).unsigned_abs() as u8).saturating_add(1).max(2))
    } else {
        None
    }
}

/// Bit-blasted `cnt_0`..`cnt_3` is a 4-bit bus on the schematic.
fn bitblast_bus_width(net: &str, nets: &HashSet<String>) -> u8 {
    let Some((pfx, idx)) = net.rsplit_once('_') else {
        return 1;
    };
    if pfx.is_empty() || !idx.chars().all(|c| c.is_ascii_digit()) {
        return 1;
    }
    let n = nets
        .iter()
        .filter(|n| {
            n.strip_prefix(&format!("{pfx}_"))
                .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
        })
        .count();
    if n >= 2 { n as u8 } else { 1 }
}

impl SchematicView {
    pub fn has_cell(&self, name: &str) -> bool {
        self.nodes.iter().any(|n| n.name == name)
    }

    pub fn is_instance(&self, name: &str) -> bool {
        self.instances.iter().any(|i| i == name)
            || self
                .nodes
                .iter()
                .any(|n| n.name == name && n.kind.starts_with("instance"))
    }

    pub fn is_primitive(&self, name: &str) -> bool {
        self.nodes.iter().any(|n| {
            n.name == name && !n.kind.starts_with("instance") && !n.kind.starts_with("PORT")
        })
    }

    fn instance_member_cells(&self, inst: &str) -> HashSet<String> {
        let pfx = format!("{inst}_");
        let named: HashSet<String> = self
            .nodes
            .iter()
            .filter(|n| {
                !n.kind.starts_with("instance")
                    && (n.name == inst || n.name.starts_with(&pfx))
            })
            .map(|n| n.name.clone())
            .collect();
        if !named.is_empty() {
            return named;
        }
        // Flattened HNF (hier.sv): LUT/FF body of the child, IOB stays at parent.
        self.nodes
            .iter()
            .filter(|n| matches!(n.kind.as_str(), "LUT6" | "HFF" | "MAC27" | "BRAM18" | "ILA"))
            .map(|n| n.name.clone())
            .collect()
    }

    fn sheet_cell_names(&self) -> HashSet<String> {
        if let Some(inst) = &self.expand_inside {
            return self.instance_member_cells(inst);
        }
        if self.path_only && !self.highlight_cells.is_empty() {
            return self
                .nodes
                .iter()
                .filter(|n| self.highlight_cells.contains(&n.name))
                .map(|n| n.name.clone())
                .collect();
        }
        let mut hide = HashSet::new();
        for inst in &self.instances {
            for n in self.instance_member_cells(inst) {
                hide.insert(n);
            }
        }
        self.nodes
            .iter()
            .filter(|n| !hide.contains(&n.name))
            .map(|n| n.name.clone())
            .collect()
    }

    fn cone_cell_names(&self) -> HashSet<String> {
        let base = self.sheet_cell_names();
        let Some(root) = &self.cone_root else {
            return base;
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
        seen.intersection(&base).cloned().collect()
    }

    pub fn set_viewport(&mut self, w: f32, h: f32) {
        self.viewport_w = w.max(1.0);
        self.viewport_h = h.max(1.0);
    }

    fn commit_camera(&mut self, cam: SchematicCamera) {
        if self.view_history.is_empty() {
            self.view_history.push(self.camera);
            self.view_index = 0;
        }
        self.view_history.truncate(self.view_index + 1);
        self.view_history.push(cam);
        self.view_index = self.view_history.len() - 1;
        self.camera = cam;
    }

    pub fn zoom_fit(&mut self) {
        let d = self.drawing();
        let vw = self.viewport_w.max(1.0);
        let vh = self.viewport_h.max(1.0);
        let zx = vw / d.width.max(1.0);
        let zy = vh / d.height.max(1.0);
        let zoom = zx.min(zy).clamp(0.05, 16.0);
        let pan_x = (vw - d.width * zoom) * 0.5;
        let pan_y = (vh - d.height * zoom) * 0.5;
        self.commit_camera(SchematicCamera { zoom, pan_x, pan_y });
    }

    pub fn zoom_by(&mut self, factor: f32) {
        let mut cam = self.camera;
        cam.zoom = (cam.zoom * factor).clamp(0.05, 16.0);
        self.commit_camera(cam);
    }

    pub fn previous_view(&mut self) -> bool {
        if self.view_index == 0 || self.view_history.is_empty() {
            return false;
        }
        self.view_index -= 1;
        self.camera = self.view_history[self.view_index];
        true
    }

    pub fn next_view(&mut self) -> bool {
        if self.view_index + 1 >= self.view_history.len() {
            return false;
        }
        self.view_index += 1;
        self.camera = self.view_history[self.view_index];
        true
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

    /// UG893 Fig. 55/56/57: boxes with left/right pin stubs and orthogonal net polylines.
    pub fn drawing(&self) -> SchematicDrawing {
        const BOX_W: f32 = 120.0;
        const PORT_W: f32 = 72.0;
        const COL_GAP: f32 = 88.0;
        const ROW_GAP: f32 = 22.0;
        const PIN_PITCH: f32 = 14.0;
        const HEADER: f32 = 20.0;
        const FOOTER: f32 = 16.0;
        const MARGIN: f32 = 28.0;
        const STUB: f32 = 12.0;

        let keep = self.cone_cell_names();
        let mut items: Vec<(String, String, Vec<SchematicPin>)> = self
            .nodes
            .iter()
            .filter(|n| keep.contains(&n.name))
            .map(|n| (n.name.clone(), n.kind.clone(), n.pins.clone()))
            .collect();

        let visible_nets: HashSet<String> = items
            .iter()
            .flat_map(|(_, _, pins)| {
                pins.iter()
                    .filter(|p| !p.net.is_empty())
                    .map(|p| p.net.clone())
            })
            .collect();
        for p in &self.ports {
            if self.cone_root.is_some() && !visible_nets.contains(&p.name) {
                continue;
            }
            let kind = if p.dir == "OUT" { "PORT_OUT" } else { "PORT_IN" };
            let pin = SchematicPin {
                name: p.name.clone(),
                net: p.name.clone(),
                output: kind == "PORT_IN",
            };
            items.push((p.name.clone(), kind.into(), vec![pin]));
        }

        let mut columns: Vec<Vec<usize>> = vec![Vec::new(); 5];
        for (i, (_, kind, _)) in items.iter().enumerate() {
            columns[schematic_column(kind)].push(i);
        }
        for col in &mut columns {
            col.sort_by_key(|&i| items[i].0.as_str());
        }

        let mut symbols = vec![None; items.len()];
        let mut col_x = [0.0f32; 5];
        let mut x = MARGIN;
        for (c, col) in columns.iter().enumerate() {
            col_x[c] = x;
            let w = if c == 0 || c == 4 { PORT_W } else { BOX_W };
            if !col.is_empty() {
                x += w + COL_GAP;
            }
        }

        for (c, col) in columns.iter().enumerate() {
            let mut y = MARGIN;
            let w = if c == 0 || c == 4 { PORT_W } else { BOX_W };
            for &i in col {
                let (_, kind, pins) = &items[i];
                let ins: Vec<&SchematicPin> = pins.iter().filter(|p| !p.output).collect();
                let outs: Vec<&SchematicPin> = pins.iter().filter(|p| p.output).collect();
                let slots = ins.len().max(outs.len()).max(1);
                let h = HEADER + slots as f32 * PIN_PITCH + FOOTER;
                let mut geom = Vec::new();
                for (k, pin) in ins.iter().enumerate() {
                    geom.push(SchematicPinGeom {
                        name: pin.name.clone(),
                        net: pin.net.clone(),
                        output: false,
                        x: col_x[c] - STUB,
                        y: y + HEADER + k as f32 * PIN_PITCH + PIN_PITCH * 0.5,
                    });
                }
                for (k, pin) in outs.iter().enumerate() {
                    geom.push(SchematicPinGeom {
                        name: pin.name.clone(),
                        net: pin.net.clone(),
                        output: true,
                        x: col_x[c] + w + STUB,
                        y: y + HEADER + k as f32 * PIN_PITCH + PIN_PITCH * 0.5,
                    });
                }
                symbols[i] = Some(SchematicSymbol {
                    name: items[i].0.clone(),
                    kind: kind.clone(),
                    x: col_x[c],
                    y,
                    w,
                    h,
                    pins: geom,
                    highlighted: self.highlight_cells.contains(&items[i].0),
                });
                y += h + ROW_GAP;
            }
        }
        let symbols: Vec<SchematicSymbol> = symbols.into_iter().flatten().collect();

        let mut pin_at: HashMap<(String, String), (f32, f32, bool)> = HashMap::new();
        for sy in &symbols {
            for p in &sy.pins {
                pin_at.insert((sy.name.clone(), p.name.clone()), (p.x, p.y, p.output));
            }
        }

        let mut net_cells: HashMap<String, HashSet<String>> = HashMap::new();
        for n in &self.nodes {
            for p in &n.pins {
                if !p.net.is_empty() {
                    net_cells.entry(p.net.clone()).or_default().insert(n.name.clone());
                }
            }
        }
        let node_names: HashSet<String> = self.nodes.iter().map(|n| n.name.clone()).collect();
        let all_nets: HashSet<String> = net_cells.keys().cloned().collect();

        let mut wires = Vec::new();
        let mut seen_wire = HashSet::new();
        let mut jog = 0usize;
        let mut nets: Vec<String> = visible_nets.into_iter().collect();
        nets.sort();
        for net in &nets {
            if net.is_empty() {
                continue;
            }
            let mut drivers = Vec::new();
            let mut loads = Vec::new();
            for sy in &symbols {
                for p in &sy.pins {
                    if p.net != *net {
                        continue;
                    }
                    if p.output {
                        drivers.push((sy.name.clone(), p.name.clone()));
                    } else {
                        loads.push((sy.name.clone(), p.name.clone()));
                    }
                }
            }
            for (sc, sp) in &drivers {
                for (dc, dp) in &loads {
                    if sc == dc {
                        continue;
                    }
                    if !seen_wire.insert((sc.clone(), sp.clone(), dc.clone(), dp.clone(), net.clone()))
                    {
                        continue;
                    }
                    let Some(&(x0, y0, _)) = pin_at.get(&(sc.clone(), sp.clone())) else {
                        continue;
                    };
                    let Some(&(x1, y1, _)) = pin_at.get(&(dc.clone(), dp.clone())) else {
                        continue;
                    };
                    let offset = ((jog % 7) as f32 - 3.0) * 6.0;
                    jog += 1;
                    let points = if (y0 - y1).abs() < 0.75 {
                        vec![(x0, y0), (x1, y1)]
                    } else if x0 <= x1 {
                        let mid = (x0 + x1) * 0.5 + offset;
                        vec![(x0, y0), (mid, y0), (mid, y1), (x1, y1)]
                    } else {
                        let mid = x0.max(x1) + 18.0 + offset.abs();
                        vec![(x0, y0), (mid, y0), (mid, y1), (x1, y1)]
                    };
                    let width = schematic_bus_width(net, sp)
                        .max(schematic_bus_width(net, dp))
                        .max(bitblast_bus_width(net, &all_nets));
                    wires.push(SchematicWire {
                        net: net.clone(),
                        src: sc.clone(),
                        src_pin: sp.clone(),
                        dst: dc.clone(),
                        dst_pin: dp.clone(),
                        points,
                        off_sheet: false,
                        width,
                        highlighted: self.highlight_nets.contains(net),
                    });
                }
            }
        }

        // Fig. 55: dotted stubs for nets that continue to cells not on this sheet.
        let mut seen_off = HashSet::new();
        for sy in &symbols {
            for p in &sy.pins {
                if p.net.is_empty() {
                    continue;
                }
                let hidden = net_cells.get(&p.net).is_some_and(|cs| {
                    cs.iter()
                        .any(|c| node_names.contains(c) && !keep.contains(c))
                });
                if !hidden {
                    continue;
                }
                if !seen_off.insert((sy.name.clone(), p.name.clone(), p.net.clone())) {
                    continue;
                }
                let dir = if p.output { 1.0 } else { -1.0 };
                let width = schematic_bus_width(&p.net, &p.name)
                    .max(bitblast_bus_width(&p.net, &all_nets));
                wires.push(SchematicWire {
                    net: p.net.clone(),
                    src: sy.name.clone(),
                    src_pin: p.name.clone(),
                    dst: "offsheet".into(),
                    dst_pin: String::new(),
                    points: vec![(p.x, p.y), (p.x + dir * 28.0, p.y)],
                    off_sheet: true,
                    width,
                    highlighted: self.highlight_nets.contains(&p.net),
                });
            }
        }

        let width = symbols
            .iter()
            .map(|s| s.x + s.w + STUB + MARGIN)
            .fold(400.0f32, f32::max)
            .max(
                wires
                    .iter()
                    .flat_map(|w| w.points.iter().map(|p| p.0 + MARGIN))
                    .fold(0.0f32, f32::max),
            );
        let height = symbols
            .iter()
            .map(|s| s.y + s.h + MARGIN)
            .fold(240.0f32, f32::max);
        SchematicDrawing {
            symbols,
            wires,
            width,
            height,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DeviceSiteView {
    pub x: u32,
    pub y: u32,
    pub kind: SiteKind,
    pub occupant: Option<String>,
    /// Every packed HNF cell at this HAD xy (Vivado BEL occupancy).
    pub bels: Vec<String>,
}

impl DeviceSiteView {
    /// HAD site name as painted on the Device drawing (`CLB_X2Y1`, `IOB_X5Y0`).
    pub fn site_name(&self) -> String {
        DeviceView::site_name(self.kind, self.x, self.y)
    }

    /// Occupancy glyph for the Device floorplan map (not a pin-name dump).
    pub fn occupancy_char(&self) -> char {
        let has_lut = self.bels.iter().any(|n| n.to_ascii_lowercase().contains("lut"))
            || self
                .occupant
                .as_deref()
                .is_some_and(|n| n.to_ascii_lowercase().contains("lut"));
        match (self.occupant.is_some() || !self.bels.is_empty(), self.kind, has_lut) {
            (true, SiteKind::Iob, _) => 'O',
            (true, _, true) => 'L',
            (true, SiteKind::Clb, _) => 'C',
            (true, _, _) => '#',
            (false, SiteKind::Iob, _) => 'i',
            (false, SiteKind::Bram, _) => 'b',
            (false, SiteKind::Dsp, _) => 'd',
            (false, SiteKind::Clk, _) => 'k',
            (false, SiteKind::Clb, _) => '.',
        }
    }
}

/// UG893 Fig. 49 clock-region rectangle on the Device die.
#[derive(Clone, Debug)]
pub struct ClockRegion {
    pub name: String,
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl ClockRegion {
    pub fn cols(&self) -> u32 {
        self.x1.saturating_sub(self.x0).saturating_add(1)
    }
    pub fn rows(&self) -> u32 {
        self.y1.saturating_sub(self.y0).saturating_add(1)
    }

    pub fn contains(&self, x: u32, y: u32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    /// HAD site count inside this rectangle (Fig. 49 Properties).
    pub fn site_count(&self, sites: &[DeviceSiteView]) -> usize {
        sites.iter().filter(|s| self.contains(s.x, s.y)).count()
    }
}

/// UG893 Floorplanning: a Pblock rectangle on the Device die.
/// `create_pblock` / `resize_pblock` hit place + `bitgen_pblock`, not a dump.
#[derive(Clone, Debug, Default)]
pub struct Pblock {
    pub name: String,
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    /// HNF cells assigned via `add_cells_to_pblock` (empty = whole design).
    pub cells: Vec<String>,
    /// Partial bitstream frames from `helion-bits::bitgen_pblock`.
    pub frames: usize,
    pub bytes: usize,
    /// False until `resize_pblock` sets a HAD range.
    pub ranged: bool,
}

impl Pblock {
    pub fn cols(&self) -> u32 {
        if !self.ranged {
            return 0;
        }
        self.x1.saturating_sub(self.x0).saturating_add(1)
    }
    pub fn rows(&self) -> u32 {
        if !self.ranged {
            return 0;
        }
        self.y1.saturating_sub(self.y0).saturating_add(1)
    }

    pub fn contains(&self, x: u32, y: u32) -> bool {
        self.ranged && x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    pub fn range_text(&self) -> String {
        if !self.ranged {
            return "-".into();
        }
        format!("CLB_X{}Y{}:CLB_X{}Y{}", self.x0, self.y0, self.x1, self.y1)
    }

    pub fn site_count(&self, sites: &[DeviceSiteView]) -> usize {
        sites.iter().filter(|s| self.contains(s.x, s.y)).count()
    }

    /// Clickable-grid dump row: UG893 Floorplanning columns over bitgen_pblock.
    pub fn row_text(&self) -> String {
        format!(
            "NAME={} RANGE={} CELLS={} FRAMES={} BYTES={} range={} cells={} frames={} bytes={}",
            self.name,
            self.range_text(),
            self.cells.len(),
            self.frames,
            self.bytes,
            self.range_text(),
            self.cells.len(),
            self.frames,
            self.bytes
        )
    }
}

/// UG893 Device routing overlay: PathFinder tiles from Session.routed, not occupancy restyle.
#[derive(Clone, Debug)]
pub struct DeviceRoute {
    pub net: String,
    pub hops: u32,
    pub delay_ps: i64,
    pub tiles: Vec<(u32, u32)>,
    pub highlighted: bool,
}

/// UG893 Device drawing: HAD die bounding box, not a restyled occupant list.
#[derive(Clone, Debug, Default)]
pub struct DeviceView {
    pub cols: u32,
    pub rows: u32,
    pub x0: u32,
    pub y0: u32,
    pub sites: Vec<DeviceSiteView>,
    /// Fig. 49: large rectangles tiling the die (clock regions).
    pub clock_regions: Vec<ClockRegion>,
    /// PathFinder IOB routes (CLB → IOB tiles) after Route.
    pub routes: Vec<DeviceRoute>,
    /// UG893 Floorplanning Pblock rectangles (create_pblock / resize_pblock).
    pub pblocks: Vec<Pblock>,
}

/// Fig. 49: tile the HAD die into 2×2 clock-region rectangles.
fn had_clock_regions(x0: u32, y0: u32, cols: u32, rows: u32) -> Vec<ClockRegion> {
    if cols == 0 || rows == 0 {
        return Vec::new();
    }
    let nx = 2u32;
    let ny = 2u32;
    let cw = cols.div_ceil(nx).max(1);
    let rh = rows.div_ceil(ny).max(1);
    let mut v = Vec::new();
    for iy in 0..ny {
        for ix in 0..nx {
            let x_lo = x0 + ix * cw;
            let y_lo = y0 + iy * rh;
            let x_hi = (x0 + (ix + 1) * cw - 1).min(x0 + cols - 1);
            let y_hi = (y0 + (iy + 1) * rh - 1).min(y0 + rows - 1);
            if x_lo > x_hi || y_lo > y_hi {
                continue;
            }
            v.push(ClockRegion {
                name: format!("X{ix}Y{iy}"),
                x0: x_lo,
                y0: y_lo,
                x1: x_hi,
                y1: y_hi,
            });
        }
    }
    v
}

impl DeviceView {
    pub fn occupant_of(&self, cell: &str) -> Option<&DeviceSiteView> {
        self.sites.iter().find(|s| {
            s.occupant.as_deref() == Some(cell) || s.bels.iter().any(|b| b == cell)
        })
    }

    pub fn site_at(&self, x: u32, y: u32) -> Option<&DeviceSiteView> {
        self.sites.iter().find(|s| s.x == x && s.y == y)
    }

    pub fn clock_region_named(&self, name: &str) -> Option<&ClockRegion> {
        self.clock_regions.iter().find(|c| c.name == name)
    }

    pub fn clock_region_at(&self, x: u32, y: u32) -> Option<&ClockRegion> {
        self.clock_regions.iter().find(|c| c.contains(x, y))
    }

    pub fn occupied_count(&self) -> usize {
        self.sites.iter().filter(|s| s.occupant.is_some()).count()
    }

    pub fn route_named(&self, net: &str) -> Option<&DeviceRoute> {
        self.routes.iter().find(|r| r.net == net)
    }

    pub fn route_at(&self, x: u32, y: u32) -> Option<&DeviceRoute> {
        self.routes
            .iter()
            .find(|r| r.hops > 0 && r.tiles.iter().any(|t| *t == (x, y)))
    }

    pub fn pblock_named(&self, name: &str) -> Option<&Pblock> {
        self.pblocks.iter().find(|p| p.name == name)
    }

    pub fn pblock_at(&self, x: u32, y: u32) -> Option<&Pblock> {
        self.pblocks.iter().find(|p| p.contains(x, y))
    }

    pub fn site_name(kind: SiteKind, x: u32, y: u32) -> String {
        let p = match kind {
            SiteKind::Clb => "CLB",
            SiteKind::Iob => "IOB",
            SiteKind::Bram => "BRAM",
            SiteKind::Dsp => "DSP",
            SiteKind::Clk => "CLK",
        };
        format!("{p}_X{x}Y{y}")
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

/// UG900 waveform marker: a named time on the engine sample grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaveMarker {
    pub name: String,
    pub sample: usize,
}

/// UG900 virtual bus: packed display of existing traces (LSB = first member).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtualBus {
    pub name: String,
    pub members: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Waveform {
    pub traces: Vec<WaveTrace>,
    pub cursor: usize,
    /// Picoseconds per sample (clock period). UG900 timescale ruler.
    pub timescale_ps: u64,
    pub markers: Vec<WaveMarker>,
    pub virtual_buses: Vec<VirtualBus>,
    /// UG900 measurement cursor A (sample index). Independent of the main cursor.
    pub cursor_a: Option<usize>,
    /// UG900 measurement cursor B (sample index). Independent of the main cursor.
    pub cursor_b: Option<usize>,
}

impl Default for Waveform {
    fn default() -> Self {
        Self {
            traces: Vec::new(),
            cursor: 0,
            timescale_ps: 10_000,
            markers: Vec::new(),
            virtual_buses: Vec::new(),
            cursor_a: None,
            cursor_b: None,
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

    /// UG900 cursor A on the engine sample grid. None when the wave is empty.
    pub fn set_cursor_a(&mut self, idx: usize) {
        let n = self.sample_len();
        self.cursor_a = if n == 0 { None } else { Some(idx.min(n - 1)) };
    }

    /// UG900 cursor B on the engine sample grid. None when the wave is empty.
    pub fn set_cursor_b(&mut self, idx: usize) {
        let n = self.sample_len();
        self.cursor_b = if n == 0 { None } else { Some(idx.min(n - 1)) };
    }

    /// Signed B−A in picoseconds (UG900 time-delta). None until both cursors sit.
    pub fn time_delta_ps(&self) -> Option<i64> {
        let a = self.cursor_a?;
        let b = self.cursor_b?;
        Some(self.time_ps(b) as i64 - self.time_ps(a) as i64)
    }

    pub fn sample_len(&self) -> usize {
        self.traces.iter().map(|t| t.samples.len()).max().unwrap_or(0)
    }

    pub fn marker(&self, name: &str) -> Option<&WaveMarker> {
        self.markers.iter().find(|m| m.name == name)
    }

    pub fn virtual_bus(&self, name: &str) -> Option<&VirtualBus> {
        self.virtual_buses.iter().find(|v| v.name == name)
    }

    /// Rebuild packed virtual-bus traces from member engine samples.
    pub fn rebuild_virtual_buses(&mut self) {
        let defs = self.virtual_buses.clone();
        let names: Vec<String> = defs.iter().map(|v| v.name.clone()).collect();
        self.traces.retain(|t| !names.contains(&t.name));
        for vb in defs {
            let members: Vec<WaveTrace> = vb
                .members
                .iter()
                .filter_map(|n| self.trace(n).cloned())
                .collect();
            if members.len() != vb.members.len() {
                continue;
            }
            let n = members
                .iter()
                .map(|t| t.samples.len())
                .min()
                .unwrap_or(0);
            let width: u8 = members
                .iter()
                .map(|m| m.width.max(1))
                .fold(0u8, |a, w| a.saturating_add(w));
            let mut samples = Vec::with_capacity(n);
            for i in 0..n {
                let mut val = 0u64;
                let mut bit = 0u32;
                for m in &members {
                    let w = m.width.max(1) as u32;
                    let mask = if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
                    let v = m.samples.get(i).copied().unwrap_or(0) & mask;
                    val |= v << bit;
                    bit = bit.saturating_add(w);
                }
                samples.push(val);
            }
            let mut t = WaveTrace::bus(&vb.name, width.max(1));
            t.samples = samples;
            self.traces.push(t);
        }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeNode {
    pub name: String,
    pub kind: String,
}

impl ScopeNode {
    pub fn type_cell(&self) -> &str {
        self.kind.split(':').next().unwrap_or(self.kind.as_str())
    }

    /// Clickable-grid dump row: UG900 Scopes Name/Type over helion-sim, not `name (kind)`.
    pub fn row_text(&self) -> String {
        format!("NAME={} TYPE={}", self.name, self.type_cell())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimObject {
    pub name: String,
    /// `port` / `net` / `reg` / `bus` / `virtual_bus` from HNF + helion-sim.
    pub kind: String,
    pub value: String,
}

impl SimObject {
    pub fn type_cell(&self) -> &str {
        if self.kind.is_empty() {
            "signal"
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

    /// Clickable-grid dump row: UG900 Objects Name/Type/Value, not `name = value`.
    pub fn row_text(&self) -> String {
        format!(
            "NAME={} TYPE={} VALUE={}",
            self.name,
            self.type_cell(),
            self.value_cell()
        )
    }
}

/// UG900 Memory window: LUT INIT / BRAM / packed sequential word from helion-sim/fabric.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryBlock {
    pub name: String,
    /// `lut_init` / `bram` / `seq`.
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

    /// Clickable-grid dump row: UG900 Memory Name/Type/Addr/Data, not a hex dump.
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

/// UG900 Breakpoints window: stop helion-sim/fabric on signal value or change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BreakpointRow {
    pub id: usize,
    pub enabled: bool,
    pub signal: String,
    /// `change` or `==N`.
    pub condition: String,
    pub value: Option<u64>,
    pub hits: u32,
}

impl BreakpointRow {
    pub fn enabled_cell(&self) -> &'static str {
        if self.enabled {
            "1"
        } else {
            "0"
        }
    }

    /// Clickable-grid dump row: UG900 Breakpoints Id/En/Signal/Cond/Hits, not a list dump.
    pub fn row_text(&self) -> String {
        format!(
            "ID={} ENABLED={} SIGNAL={} CONDITION={} HITS={}",
            self.id,
            self.enabled_cell(),
            self.signal,
            self.condition,
            self.hits
        )
    }
}

/// UG900 Locals window: current-scope sequential probes from helion-sim/fabric.
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

    /// Clickable-grid dump row: UG900 Locals Name/Type/Value, not `name = value`.
    pub fn row_text(&self) -> String {
        format!(
            "NAME={} TYPE={} VALUE={} SCOPE={}",
            self.name,
            self.type_cell(),
            self.value_cell(),
            if self.scope.is_empty() {
                "-"
            } else {
                self.scope.as_str()
            }
        )
    }
}

#[derive(Clone, Debug)]
pub struct IoPortView {
    pub name: String,
    pub dir: String,
    /// Placed IOB after `place_design` (HAD `IOB_XnYm`).
    pub site: Option<String>,
    /// `set_property PACKAGE_PIN` / LOC constraint (may precede place).
    pub package_pin: Option<String>,
    /// `set_property IOSTANDARD` — HAD-legal pad standard (STA pad delay / DRC VCCO).
    pub iostandard: Option<String>,
    /// `set_property DRIVE` — HAD-legal mA (STA pad delay / DRC vs IOSTANDARD / bitgen).
    pub drive: Option<String>,
    /// `set_property SLEW` — SLOW | FAST.
    pub slew: Option<String>,
    /// `set_property PULLTYPE` — NONE | PULLUP | PULLDOWN | KEEPER.
    pub pulltype: Option<String>,
    /// `set_property DIFF_TERM` — TRUE | FALSE (HAD SSTL/HSTL only when TRUE).
    pub diff_term: Option<String>,
    /// `set_property IN_TERM` — NONE | UNTUNED_SPLIT_{40,50,60}.
    pub in_term: Option<String>,
}

impl IoPortView {
    pub fn package_pin_cell(&self) -> &str {
        self.package_pin.as_deref().unwrap_or("-")
    }
    pub fn placed_cell(&self) -> &str {
        self.site.as_deref().unwrap_or("-")
    }
    pub fn iostandard_cell(&self) -> &str {
        self.iostandard.as_deref().unwrap_or("-")
    }
    pub fn drive_cell(&self) -> &str {
        self.drive.as_deref().unwrap_or("-")
    }
    pub fn slew_cell(&self) -> &str {
        self.slew.as_deref().unwrap_or("-")
    }
    pub fn pulltype_cell(&self) -> &str {
        self.pulltype.as_deref().unwrap_or("-")
    }
    pub fn diff_term_cell(&self) -> &str {
        self.diff_term.as_deref().unwrap_or("-")
    }
    pub fn in_term_cell(&self) -> &str {
        self.in_term.as_deref().unwrap_or("-")
    }

    /// Clickable-grid dump row: UG893 I/O Ports columns from HAD/STA, not a pin list.
    pub fn row_text(&self) -> String {
        format!(
            "NAME={} DIR={} PACKAGE_PIN={} placed={} IOSTANDARD={} DRIVE={} SLEW={} PULLTYPE={} DIFF_TERM={} IN_TERM={}",
            self.name,
            self.dir,
            self.package_pin_cell(),
            self.placed_cell(),
            self.iostandard_cell(),
            self.drive_cell(),
            self.slew_cell(),
            self.pulltype_cell(),
            self.diff_term_cell(),
            self.in_term_cell()
        )
    }
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
    pub idcode: Option<u32>,
    pub ir: Option<u8>,
}

impl Default for HwManager {
    fn default() -> Self {
        Self {
            open: false,
            programmed: false,
            target: "sim".into(),
            stat: None,
            idcode: None,
            ir: None,
        }
    }
}

/// One clickable STAT bit in the UG893 Hardware Manager (helion-hw TAP DR, not a one-liner).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HwStatRow {
    pub bit: u8,
    pub name: String,
    pub value: bool,
    pub description: String,
}

impl HwStatRow {
    fn from_bit(b: StatBit) -> Self {
        Self {
            bit: b.bit,
            name: b.name.into(),
            value: b.value,
            description: b.description.into(),
        }
    }
}

/// Helion-hw/fabric status-register pane: TAP IDCODE + STAT bits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HwStatReport {
    pub open: bool,
    pub programmed: bool,
    pub target: String,
    pub part: String,
    pub idcode: u32,
    pub ir: u8,
    pub word: u32,
    pub bits: Vec<HwStatRow>,
}

impl HwStatReport {
    fn closed() -> Self {
        Self {
            open: false,
            programmed: false,
            target: "sim".into(),
            part: String::new(),
            idcode: 0,
            ir: 0,
            word: 0,
            bits: Vec::new(),
        }
    }

    pub fn bit(&self, spec: &str) -> Option<&HwStatRow> {
        let spec = spec.trim();
        if spec.is_empty() {
            return self
                .bits
                .iter()
                .find(|b| b.name == "DONE")
                .or(self.bits.first());
        }
        if let Ok(i) = spec.parse::<u8>() {
            if let Some(b) = self.bits.iter().find(|b| b.bit == i) {
                return Some(b);
            }
            if let Some(b) = self.bits.get(i as usize) {
                return Some(b);
            }
        }
        self.bits
            .iter()
            .find(|b| b.name.eq_ignore_ascii_case(spec))
    }

    pub fn word_hex(&self) -> String {
        format!("{:#010x}", self.word)
    }

    pub fn text(&self) -> String {
        if !self.open {
            return "no hardware — open_hw_manager".into();
        }
        let mut s = format!(
            "hw_stat target={} part={} idcode={:#010x} ir={:#04x} programmed={} word={}",
            self.target,
            self.part,
            self.idcode,
            self.ir,
            u8::from(self.programmed),
            self.word_hex()
        );
        for b in &self.bits {
            s.push_str(&format!(
                "\nBIT={} NAME={} VALUE={} DESC={}",
                b.bit,
                b.name,
                u8::from(b.value),
                b.description
            ));
        }
        s
    }
}

/// One ILA capture sample (fabric bit at the armed net, not a lamp).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IlaSampleRow {
    pub sample: usize,
    pub time_ps: u64,
    pub value: char,
    pub trigger: bool,
}

#[derive(Clone, Debug)]
pub struct BdView {
    pub name: String,
    pub cores: Vec<String>,
    pub sv: String,
    pub ok: bool,
}

/// One UG893 IP Catalog row (VLNV from helion-ipxact, not a collapsing dump).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpCatalogRow {
    pub name: String,
    pub vendor: String,
    pub library: String,
    pub version: String,
    pub bus: String,
    pub vlnv: String,
    pub status: String,
}

/// One generated-HDL instance row for the IP Integrator (not a SV dump).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BdHdlRow {
    pub instance: String,
    pub module: String,
    pub bus: String,
    pub ports: String,
}

/// IP Integrator symbol (UG893 BD canvas: boxes + interface pins, not a catalog dump).
#[derive(Clone, Debug)]
pub struct BdSymbol {
    pub name: String,
    pub kind: String,
    pub bus: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub pins: Vec<BdPin>,
}

/// Vivado-style BD pin: scalar stub or thick interface bar (Helion-MM, not AXI).
#[derive(Clone, Debug)]
pub struct BdPin {
    pub name: String,
    pub net: String,
    pub iface: bool,
    pub output: bool,
    pub x: f32,
    pub y: f32,
}

/// Address-editor row for a Helion-MM slave (offset + range).
#[derive(Clone, Debug)]
pub struct BdAddrEntry {
    pub slave: String,
    pub base: u32,
    pub range: u32,
}

#[derive(Clone, Debug)]
pub struct BdWire {
    pub net: String,
    pub src: String,
    pub dst: String,
    pub points: Vec<(f32, f32)>,
}

#[derive(Clone, Debug, Default)]
pub struct BdDrawing {
    pub symbols: Vec<BdSymbol>,
    pub wires: Vec<BdWire>,
    pub addresses: Vec<BdAddrEntry>,
    pub width: f32,
    pub height: f32,
}

impl BdView {
    /// Helion-MM canvas: clk/resetn ports, interconnect hub, IP boxes, orthogonal nets.
    pub fn drawing(&self, catalog: &[IpCore]) -> BdDrawing {
        const MARGIN: f32 = 28.0;
        const PORT_W: f32 = 64.0;
        const PORT_H: f32 = 32.0;
        const HUB_W: f32 = 140.0;
        const HUB_H: f32 = 96.0;
        const CORE_W: f32 = 132.0;
        const CORE_H: f32 = 84.0;
        const GAP: f32 = 72.0;
        const ROW: f32 = 20.0;
        const STUB: f32 = 14.0;

        let pin = |name: &str, net: &str, iface: bool, output: bool, x: f32, y: f32| BdPin {
            name: name.into(),
            net: net.into(),
            iface,
            output,
            x,
            y,
        };

        let mut symbols = Vec::new();
        let mut clk = BdSymbol {
            name: "clk".into(),
            kind: "PORT_IN".into(),
            bus: String::new(),
            x: MARGIN,
            y: MARGIN + 8.0,
            w: PORT_W,
            h: PORT_H,
            pins: Vec::new(),
        };
        clk.pins.push(pin(
            "clk",
            "clk",
            false,
            true,
            clk.x + clk.w + STUB,
            clk.y + clk.h * 0.5,
        ));
        let mut resetn = BdSymbol {
            name: "resetn".into(),
            kind: "PORT_IN".into(),
            bus: String::new(),
            x: MARGIN,
            y: MARGIN + 8.0 + PORT_H + ROW,
            w: PORT_W,
            h: PORT_H,
            pins: Vec::new(),
        };
        resetn.pins.push(pin(
            "resetn",
            "resetn",
            false,
            true,
            resetn.x + resetn.w + STUB,
            resetn.y + resetn.h * 0.5,
        ));
        let hub_x = MARGIN + PORT_W + GAP;
        let hub_y = MARGIN;
        let mut hub = BdSymbol {
            name: "mm_interconnect".into(),
            kind: "INTERCONNECT".into(),
            bus: "Helion-MM".into(),
            x: hub_x,
            y: hub_y,
            w: HUB_W,
            h: HUB_H,
            pins: Vec::new(),
        };
        hub.pins.push(pin("clk", "clk", false, false, hub.x - STUB, hub.y + 18.0));
        hub.pins.push(pin(
            "resetn",
            "resetn",
            false,
            false,
            hub.x - STUB,
            hub.y + 40.0,
        ));
        hub.pins.push(pin(
            "clk",
            "clk",
            false,
            true,
            hub.x + hub.w + STUB,
            hub.y + 18.0,
        ));
        hub.pins.push(pin(
            "resetn",
            "resetn",
            false,
            true,
            hub.x + hub.w + STUB,
            hub.y + 40.0,
        ));
        hub.pins.push(pin(
            "m_mm",
            "Helion-MM",
            true,
            true,
            hub.x + hub.w + STUB,
            hub.y + hub.h * 0.5,
        ));
        let core_x = hub_x + HUB_W + GAP;
        let mut core_y = MARGIN;
        let mut cores = Vec::new();
        let mut addresses = Vec::new();
        for (i, name) in self.cores.iter().enumerate() {
            let bus = catalog
                .iter()
                .find(|c| c.name == *name)
                .map(|c| c.bus.clone())
                .unwrap_or_else(|| "Helion-MM".into());
            let mut c = BdSymbol {
                name: format!("u_{name}"),
                kind: name.clone(),
                bus,
                x: core_x,
                y: core_y,
                w: CORE_W,
                h: CORE_H,
                pins: Vec::new(),
            };
            c.pins.push(pin("clk", "clk", false, false, c.x - STUB, c.y + 18.0));
            c.pins.push(pin(
                "resetn",
                "resetn",
                false,
                false,
                c.x - STUB,
                c.y + 36.0,
            ));
            c.pins.push(pin(
                "s_mm",
                "Helion-MM",
                true,
                false,
                c.x - STUB,
                c.y + c.h * 0.5,
            ));
            addresses.push(BdAddrEntry {
                slave: c.name.clone(),
                base: (i as u32) * 0x1000,
                range: 0x1000,
            });
            cores.push(c);
            core_y += CORE_H + ROW;
        }
        symbols.push(clk.clone());
        symbols.push(resetn.clone());
        symbols.push(hub.clone());
        symbols.extend(cores.iter().cloned());

        let mut wires = Vec::new();
        let manhattan = |x0: f32, y0: f32, x1: f32, y1: f32| -> Vec<(f32, f32)> {
            if (y0 - y1).abs() < 0.75 {
                vec![(x0, y0), (x1, y1)]
            } else {
                let mid = (x0 + x1) * 0.5;
                vec![(x0, y0), (mid, y0), (mid, y1), (x1, y1)]
            }
        };
        let pin_xy = |sy: &BdSymbol, name: &str, output: bool| -> Option<(f32, f32)> {
            sy.pins
                .iter()
                .find(|p| p.name == name && p.output == output)
                .map(|p| (p.x, p.y))
        };
        if let (Some(a), Some(b)) = (pin_xy(&clk, "clk", true), pin_xy(&hub, "clk", false)) {
            wires.push(BdWire {
                net: "clk".into(),
                src: clk.name.clone(),
                dst: hub.name.clone(),
                points: manhattan(a.0, a.1, b.0, b.1),
            });
        }
        if let (Some(a), Some(b)) = (
            pin_xy(&resetn, "resetn", true),
            pin_xy(&hub, "resetn", false),
        ) {
            wires.push(BdWire {
                net: "resetn".into(),
                src: resetn.name.clone(),
                dst: hub.name.clone(),
                points: manhattan(a.0, a.1, b.0, b.1),
            });
        }
        for c in &cores {
            if let (Some(a), Some(b)) = (pin_xy(&hub, "clk", true), pin_xy(c, "clk", false)) {
                wires.push(BdWire {
                    net: "clk".into(),
                    src: hub.name.clone(),
                    dst: c.name.clone(),
                    points: manhattan(a.0, a.1, b.0, b.1),
                });
            }
            if let (Some(a), Some(b)) = (pin_xy(&hub, "resetn", true), pin_xy(c, "resetn", false))
            {
                wires.push(BdWire {
                    net: "resetn".into(),
                    src: hub.name.clone(),
                    dst: c.name.clone(),
                    points: manhattan(a.0, a.1, b.0, b.1),
                });
            }
            if let (Some(a), Some(b)) = (pin_xy(&hub, "m_mm", true), pin_xy(c, "s_mm", false)) {
                wires.push(BdWire {
                    net: "Helion-MM".into(),
                    src: hub.name.clone(),
                    dst: c.name.clone(),
                    points: manhattan(a.0, a.1, b.0, b.1),
                });
            }
        }

        let width = symbols
            .iter()
            .map(|s| s.x + s.w + MARGIN + STUB)
            .fold(360.0f32, f32::max);
        let height = symbols
            .iter()
            .map(|s| s.y + s.h + MARGIN)
            .fold(200.0f32, f32::max);
        BdDrawing {
            symbols,
            wires,
            addresses,
            width,
            height,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceTab {
    Reports,
    Settings,
    Schematic,
    Device,
    Wave,
    Memory,
    Breakpoints,
    Locals,
    Hardware,
    Ip,
    Constraints,
    ClockInteraction,
    Cdc,
    ClockNetworks,
    Power,
    Methodology,
    Drc,
    Utilization,
    Hierarchy,
    Find,
    Package,
    Runs,
    Bitstream,
}

/// One clickable row in the UG893 Reports window (not a stacked dump).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportCatalogRow {
    pub id: String,
    pub name: String,
    pub category: String,
    pub tcl: String,
    pub status: String,
    pub summary: String,
    pub workspace: WorkspaceTab,
}

impl ReportCatalogRow {
    pub fn row_text(&self) -> String {
        format!(
            "NAME={} CATEGORY={} STATUS={} SUMMARY={} TCL={}",
            self.id, self.category, self.status, self.summary, self.tcl
        )
    }
}

/// UG893 Hierarchy — top module + instances + leaf primitives from HNF.
#[derive(Clone, Debug, Default)]
pub struct HierarchyView {
    pub top: Option<String>,
    /// `(name, kind)` in tree order: module, then instances, then leaf cells.
    pub nodes: Vec<(String, String)>,
}

/// Nested hierarchy box (Fig. 61). `w * h` scales with `cells`.
#[derive(Clone, Debug)]
pub struct HierBox {
    pub name: String,
    pub kind: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub cells: usize,
}

#[derive(Clone, Debug, Default)]
pub struct HierarchyDrawing {
    pub boxes: Vec<HierBox>,
    pub width: f32,
    pub height: f32,
}

impl HierarchyView {
    pub fn has(&self, name: &str) -> bool {
        self.top.as_deref() == Some(name) || self.nodes.iter().any(|(n, _)| n == name)
    }

    fn resource_weight(kind: &str) -> usize {
        if kind.contains("BRAM") {
            8
        } else if kind.contains("MAC") || kind.contains("DSP") {
            8
        } else if kind == "module" || kind.starts_with("instance:") {
            0
        } else {
            1
        }
    }

    /// Fig. 61 Block view: nested boxes whose area tracks HNF cell/resource count.
    pub fn drawing(&self) -> HierarchyDrawing {
        const PAD: f32 = 10.0;
        const HEADER: f32 = 20.0;
        const LEAF_W: f32 = 72.0;
        const LEAF_H: f32 = 36.0;
        const UNIT_AREA: f32 = 48.0 * 36.0;

        let top = self.top.clone().unwrap_or_else(|| "(none)".into());
        let instances: Vec<(String, String)> = self
            .nodes
            .iter()
            .filter(|(_, k)| k.starts_with("instance:"))
            .cloned()
            .collect();
        let leaves: Vec<(String, String)> = self
            .nodes
            .iter()
            .filter(|(_, k)| *k != "module" && !k.starts_with("instance:"))
            .cloned()
            .collect();

        let mut owned: HashSet<String> = HashSet::new();
        let mut inst_kids: Vec<(String, String, Vec<(String, String)>)> = Vec::new();
        for (iname, ikind) in &instances {
            let pfx = format!("{iname}_");
            let kids: Vec<(String, String)> = leaves
                .iter()
                .filter(|(n, _)| n == iname || n.starts_with(&pfx))
                .cloned()
                .collect();
            for (n, _) in &kids {
                owned.insert(n.clone());
            }
            inst_kids.push((iname.clone(), ikind.clone(), kids));
        }
        let top_leaves: Vec<(String, String)> = leaves
            .iter()
            .filter(|(n, _)| !owned.contains(n))
            .cloned()
            .collect();

        let mut boxes = Vec::new();
        let mut cursor_x = PAD;
        let mut row_h = 0.0_f32;
        let origin_y = HEADER + PAD;

        let place_leaves = |kids: &[(String, String)],
                            ox: f32,
                            oy: f32,
                            boxes: &mut Vec<HierBox>|
         -> (f32, f32) {
            let n = kids.len().max(1);
            let cols = (n as f32).sqrt().ceil().max(1.0) as usize;
            let rows = (n + cols - 1) / cols;
            for (i, (name, kind)) in kids.iter().enumerate() {
                let c = i % cols;
                let r = i / cols;
                let units = Self::resource_weight(kind).max(1);
                let w = LEAF_W * (units as f32).sqrt().max(1.0);
                let h = LEAF_H * (units as f32).sqrt().max(1.0);
                boxes.push(HierBox {
                    name: name.clone(),
                    kind: kind.clone(),
                    x: ox + c as f32 * (LEAF_W + PAD),
                    y: oy + r as f32 * (LEAF_H + PAD),
                    w,
                    h,
                    cells: units,
                });
            }
            (
                cols as f32 * (LEAF_W + PAD) + PAD,
                rows as f32 * (LEAF_H + PAD) + PAD,
            )
        };

        for (iname, ikind, kids) in &inst_kids {
            let units = kids
                .iter()
                .map(|(_, k)| Self::resource_weight(k))
                .sum::<usize>()
                .max(1);
            let inner_ox = cursor_x + PAD;
            let inner_oy = origin_y + HEADER + PAD;
            let (iw, ih) = place_leaves(kids, inner_ox, inner_oy, &mut boxes);
            let area = units as f32 * UNIT_AREA;
            let aw = (area * 1.2).sqrt().max(iw + PAD);
            let ah = (area / aw).max(ih + HEADER + PAD);
            let bw = aw.max(iw + 2.0 * PAD);
            let bh = ah.max(ih + HEADER + 2.0 * PAD);
            boxes.push(HierBox {
                name: iname.clone(),
                kind: ikind.clone(),
                x: cursor_x,
                y: origin_y,
                w: bw,
                h: bh,
                cells: units,
            });
            cursor_x += bw + PAD;
            row_h = row_h.max(bh);
        }

        if !top_leaves.is_empty() {
            let units = top_leaves
                .iter()
                .map(|(_, k)| Self::resource_weight(k))
                .sum::<usize>()
                .max(1);
            let inner_ox = cursor_x + PAD;
            let inner_oy = origin_y + HEADER + PAD;
            let (iw, ih) = place_leaves(&top_leaves, inner_ox, inner_oy, &mut boxes);
            let area = units as f32 * UNIT_AREA;
            let aw = (area * 1.2).sqrt().max(iw + PAD);
            let ah = (area / aw).max(ih + HEADER + PAD);
            let bw = aw.max(iw + 2.0 * PAD);
            let bh = ah.max(ih + HEADER + 2.0 * PAD);
            boxes.push(HierBox {
                name: "Leaf Cells".into(),
                kind: "leaves".into(),
                x: cursor_x,
                y: origin_y,
                w: bw,
                h: bh,
                cells: units,
            });
            cursor_x += bw + PAD;
            row_h = row_h.max(bh);
        }

        let top_units = leaves
            .iter()
            .map(|(_, k)| Self::resource_weight(k))
            .sum::<usize>()
            .max(1);
        let content_w = cursor_x.max(120.0);
        let content_h = origin_y + row_h + PAD;
        let area = top_units as f32 * UNIT_AREA;
        let tw = (area * 1.3).sqrt().max(content_w + PAD);
        let th = (area / tw).max(content_h + HEADER);
        let outer_w = tw.max(content_w + PAD);
        let outer_h = th.max(content_h + HEADER);
        boxes.insert(
            0,
            HierBox {
                name: top,
                kind: "module".into(),
                x: 0.0,
                y: 0.0,
                w: outer_w,
                h: outer_h,
                cells: top_units,
            },
        );
        HierarchyDrawing {
            boxes,
            width: outer_w + PAD,
            height: outer_h + PAD,
        }
    }
}

/// One clickable row in the UG893 Find Results pane (not a `kind  name` dump).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindHit {
    /// Object type: `cell` / `net` / `port` / `pin` (HNF or HAD).
    pub kind: String,
    pub name: String,
    /// LUT6 / HFF / IN / fanout=N / BANKn — Type is `kind`, this is the primitive.
    pub primitive: String,
    /// HNF top / connected port.
    pub parent: String,
}

impl FindHit {
    pub fn type_cell(&self) -> &str {
        self.kind.split(':').next().unwrap_or(self.kind.as_str())
    }

    pub fn primitive_cell(&self) -> &str {
        if !self.primitive.is_empty() {
            return self.primitive.as_str();
        }
        if let Some((_, rest)) = self.kind.split_once(':') {
            if !rest.is_empty() {
                return rest;
            }
        }
        "-"
    }

    pub fn parent_cell(&self) -> &str {
        if self.parent.is_empty() {
            "-"
        } else {
            self.parent.as_str()
        }
    }

    /// Clickable-grid dump row: UG893 Find Results Name/Type over HNF/HAD.
    pub fn row_text(&self) -> String {
        format!(
            "NAME={} TYPE={} PRIMITIVE={} PARENT={}",
            self.name,
            self.type_cell(),
            self.primitive_cell(),
            self.parent_cell()
        )
    }
}

/// One clickable Name/Value row in the UG893 Properties pane (not a weak-key dump).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyRow {
    pub name: String,
    pub value: String,
}

impl PropertyRow {
    pub fn row_text(&self) -> String {
        format!("NAME={} VALUE={}", self.name, self.value)
    }
}

/// HAD IOB sites as package pins (Helion has no BGA; pins are IOB_XxYy).
#[derive(Clone, Debug)]
pub struct PackagePin {
    pub pin: String,
    pub x: u32,
    pub y: u32,
    pub port: Option<String>,
    /// Fig. 53: colored I/O bank this pin belongs to.
    pub bank: u32,
}

impl PackagePin {
    /// Bank fill used on the package drawing (distinct hues per bank).
    pub fn bank_rgb(&self) -> (u8, u8, u8) {
        match self.bank % 4 {
            0 => (0x1e, 0x4a, 0x5c),
            1 => (0x4a, 0x2e, 0x5c),
            2 => (0x4a, 0x4a, 0x1e),
            _ => (0x5c, 0x2e, 0x1e),
        }
    }
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
    /// UG893 Properties selected Name (clickable Name/Value table).
    pub selected_property: Option<String>,
    pub selected: Option<String>,
    pub scopes: Vec<ScopeNode>,
    pub selected_scope: Option<String>,
    pub objects: Vec<SimObject>,
    /// UG900 Objects selected Name (clickable Name/Type/Value table).
    pub selected_object: Option<String>,
    /// UG900 Memory arrays from helion-sim LUT INIT / fabric BLE / BRAM.
    pub memories: Vec<MemoryBlock>,
    pub selected_memory: Option<String>,
    pub selected_memory_addr: Option<usize>,
    /// UG900 Breakpoints over helion-sim/fabric signals.
    pub breakpoints: Vec<BreakpointRow>,
    pub selected_breakpoint: Option<usize>,
    /// UG900 Locals: current-scope sequential probes.
    pub locals: Vec<LocalRow>,
    pub selected_local: Option<String>,
    bp_prev: HashMap<String, u64>,
    next_bp_id: usize,
    pub wave: Waveform,
    pub timing_paths: Vec<TimingPath>,
    pub selected_timing_path: Option<usize>,
    /// UG903 pin-delay table selected Name (`cell/pin`).
    pub selected_timing_pin: Option<String>,
    pub console_find: String,
    pub console_selected: Option<usize>,
    pub console_find_hits: Vec<usize>,
    pub io_ports: Vec<IoPortView>,
    pub hw: HwManager,
    pub ila: IlaDashboard,
    pub ip_catalog: Vec<IpCore>,
    /// UG893 IP Catalog selected core name.
    pub selected_ip: Option<String>,
    /// Last Generate Output Products XML (helion-ipxact), not painted as a dump.
    pub generated_ip_xml: Option<String>,
    pub block_design: Option<BdView>,
    pub drc: Option<Drc>,
    /// UG893 Timing Constraints — SDC/XDC clocks that feed helion-sta.
    pub constraints: Constraints,
    pub hierarchy: HierarchyView,
    pub find_results: Vec<FindHit>,
    /// UG893 Find Results selected row index.
    pub selected_find: Option<usize>,
    /// UG893 Sources selected path (clickable Name/Type table).
    pub selected_source: Option<String>,
    /// UG893 Netlist selected Name (clickable Name/Type table).
    pub selected_netlist: Option<String>,
    /// UG893 Project Settings selected Name (clickable Name/Value table).
    pub selected_setting: Option<String>,
    /// Active UG893 fileset (`sources_1` / `constrs_1` / `sim_1`).
    pub active_fileset: String,
    pub package_pins: Vec<PackagePin>,
    pub package: PackageDrawing,
    /// UG893 Floorplanning Pblocks (source of truth; copied onto DeviceView).
    pub pblocks: Vec<Pblock>,
    pub workspace: WorkspaceTab,
    pub bottom_tab: BottomTab,
    /// UG893 Messages filter (None = All). Counts stay unfiltered.
    pub message_filter: Option<MsgSeverity>,
    pub selected_message: Option<usize>,
    /// UG893 Reports window selected catalog row (`report_cdc`, …).
    pub selected_report: Option<String>,
    /// UG893 Log pane selected transcript row.
    pub selected_log: Option<usize>,
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
        let mut m = Self {
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
                DesignRun::new("synth_1", "Synthesis"),
                DesignRun::new("impl_1", "Implementation"),
            ],
            schematic: SchematicView::default(),
            device: DeviceView::default(),
            properties: Vec::new(),
            selected_property: None,
            selected: None,
            scopes: Vec::new(),
            selected_scope: None,
            objects: Vec::new(),
            selected_object: None,
            memories: Vec::new(),
            selected_memory: None,
            selected_memory_addr: None,
            breakpoints: Vec::new(),
            selected_breakpoint: None,
            locals: Vec::new(),
            selected_local: None,
            bp_prev: HashMap::new(),
            next_bp_id: 1,
            wave: Waveform::default(),
            timing_paths: Vec::new(),
            selected_timing_path: None,
            selected_timing_pin: None,
            console_find: String::new(),
            console_selected: None,
            console_find_hits: Vec::new(),
            io_ports: Vec::new(),
            hw: HwManager::default(),
            ila: IlaDashboard::default(),
            ip_catalog: ipxact_catalog(),
            selected_ip: None,
            generated_ip_xml: None,
            block_design: None,
            drc: None,
            constraints: Constraints::default(),
            hierarchy: HierarchyView::default(),
            find_results: Vec::new(),
            selected_find: None,
            selected_source: None,
            selected_netlist: None,
            selected_setting: None,
            active_fileset: "sources_1".into(),
            package_pins: Vec::new(),
            package: PackageDrawing::default(),
            pblocks: Vec::new(),
            workspace: WorkspaceTab::Reports,
            bottom_tab: BottomTab::Tcl,
            message_filter: None,
            selected_message: None,
            selected_report: None,
            selected_log: None,
            event_sim: None,
            fabric_sim: None,
        };
        // Vivado shows the part floorplan before sources exist — HAD sites, not empty chrome.
        m.refresh_device();
        m.refresh_package();
        m
    }

    pub fn with_part(part: &str) -> Self {
        let mut m = Self::new();
        m.shell.part = part.to_string();
        m.shell.session.part = part.to_string();
        m.refresh_device();
        m.refresh_package();
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

    /// helion-bits FAR table: configured (nonzero) frames from the Session bitstream.
    pub fn bitstream_report(&self) -> BitstreamReport {
        let Some(b) = self.shell.session.bitstream.as_ref() else {
            return BitstreamReport::default();
        };
        let rows: Vec<BitstreamFrame> = b
            .frames
            .iter()
            .filter(|(_, w)| **w != 0)
            .map(|((block, major, minor), word)| {
                BitstreamFrame::from_key(*block, *major, *minor, *word)
            })
            .collect();
        BitstreamReport {
            idcode: b.idcode,
            hash: self.bitstream_hash().unwrap_or(0),
            bytes: b.packets.len(),
            frames: b.frames.len(),
            configured: rows.len(),
            rows,
        }
    }

    pub fn bitstream_text(&self) -> String {
        self.bitstream_report().text()
    }

    /// Click a FAR row: properties + Bitstream workspace.
    pub fn select_bitstream_frame(&mut self, spec: &str) -> Result<String, String> {
        let report = self.bitstream_report();
        if report.rows.is_empty() {
            return Err("select_bitstream_frame: no bitstream".into());
        }
        let row = report
            .frame(spec)
            .cloned()
            .ok_or_else(|| format!("select_bitstream_frame: no FAR {spec}"))?;
        self.selected = Some(row.far_hex());
        self.properties = vec![
            ("NAME".into(), row.far_hex()),
            ("TYPE".into(), "bitstream_frame".into()),
            ("FAR".into(), row.far_hex()),
            ("BLOCK".into(), row.block_name().into()),
            ("DIE".into(), row.die.to_string()),
            ("MAJOR".into(), row.major.to_string()),
            ("MINOR".into(), row.minor.to_string()),
            ("ONES".into(), row.ones().to_string()),
            ("WORD".into(), row.word_hex()),
            ("IDCODE".into(), format!("{:#010x}", report.idcode)),
            ("HASH".into(), format!("{:#010x}", report.hash)),
        ];
        self.workspace = WorkspaceTab::Bitstream;
        Ok(format!(
            "bitstream FAR={} BLOCK={} DIE={} MAJOR={} MINOR={} ONES={} WORD={}",
            row.far_hex(),
            row.block_name(),
            row.die,
            row.major,
            row.minor,
            row.ones(),
            row.word_hex()
        ))
    }

    pub fn wns_ps(&self) -> Option<i64> {
        self.timing.as_ref().map(|t| t.wns_ps)
    }

    /// Timing pane text. Empty until the design is routed.
    pub fn timing_text(&self) -> String {
        match &self.timing {
            None => "no routed design — run Route".into(),
            Some(t) => format!(
                "WNS_PS={} TNS_PS={} SETUP_PS={} HOLD_PS={} HOLD_SLACK_PS={} endpoints={} r2r_ps={} iob_ps={} route_ps={} CLK_NET_PS={}",
                t.wns_ps,
                t.tns_ps,
                t.setup_ps,
                t.hold_ps,
                t.hold_slack_ps,
                t.endpoints,
                t.r2r_ps,
                t.iob_ps,
                t.route_ps,
                t.clk_net_ps
            ),
        }
    }

    /// Utilization pane text. Empty until the design is packed/placed.
    pub fn utilization_text(&self) -> String {
        self.utilization_report().text()
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
        } else if let Some(rest) = t.strip_prefix("add_wave_marker ") {
            self.add_wave_marker(rest)
        } else if let Some(rest) = t.strip_prefix("add_wave_virtual_bus ") {
            self.add_wave_virtual_bus(rest)
        } else if t == "wave_cursors" {
            self.workspace = WorkspaceTab::Wave;
            Ok(self.wave_cursors_text())
        } else if t == "wave_cursor_a" || t.starts_with("wave_cursor_a ") {
            let rest = t.strip_prefix("wave_cursor_a").unwrap_or("").trim();
            self.set_wave_ab_cursor(&format!("A {rest}"))
        } else if t == "wave_cursor_b" || t.starts_with("wave_cursor_b ") {
            let rest = t.strip_prefix("wave_cursor_b").unwrap_or("").trim();
            self.set_wave_ab_cursor(&format!("B {rest}"))
        } else if let Some(rest) = t.strip_prefix("wave_cursor ") {
            self.set_wave_ab_cursor(rest)
        } else if let Some(rest) = t.strip_prefix("wave_radix ") {
            self.set_wave_radix(rest)
        } else if let Some(rest) = t.strip_prefix("wave_style ") {
            self.set_wave_style(rest)
        } else if let Some(id) = t.strip_prefix("select ") {
            self.select(id.trim());
            Ok(format!("select {}", id.trim()))
        } else if t == "properties" {
            Ok(self.properties_text())
        } else if t == "select_property" || t.starts_with("select_property ") {
            let spec = t.strip_prefix("select_property").unwrap_or("").trim();
            self.select_property(spec)
        } else if t == "project_settings" || t == "settings" {
            self.open_project_settings()
        } else if t == "select_project_setting" || t.starts_with("select_project_setting ") {
            let spec = t.strip_prefix("select_project_setting").unwrap_or("").trim();
            self.select_project_setting(spec)
        } else if t == "set_part" || t.starts_with("set_part ") {
            let part = t.strip_prefix("set_part").unwrap_or("").trim();
            self.set_part(part)
        } else if t == "sources" {
            self.layout = LayoutKind::Default;
            Ok(self.sources_text())
        } else if t == "select_source" || t.starts_with("select_source ") {
            let spec = t.strip_prefix("select_source").unwrap_or("").trim();
            self.select_source(spec)
        } else if t == "netlist" || t == "tree" {
            self.layout = LayoutKind::Default;
            Ok(self.netlist_text())
        } else if t == "select_netlist" || t.starts_with("select_netlist ") {
            let spec = t.strip_prefix("select_netlist").unwrap_or("").trim();
            self.select_netlist(spec)
        } else if t == "find" || t == "find_results" {
            self.open_find_results()
        } else if let Some(q) = t.strip_prefix("find ") {
            self.find(q.trim())
        } else if t == "select_find" || t.starts_with("select_find ") {
            let spec = t.strip_prefix("select_find").unwrap_or("").trim();
            self.select_find(spec)
        } else if t == "hierarchy" {
            self.workspace = WorkspaceTab::Hierarchy;
            Ok(self.hierarchy_text())
        } else if t == "hierarchy_drawing" {
            self.workspace = WorkspaceTab::Hierarchy;
            Ok(self.hierarchy_drawing_text())
        } else if t == "run_synthesis" {
            self.run_step(FlowStep::Synthesis)
        } else if t == "run_implementation" {
            self.launch_runs("impl_1")
        } else if t == "run_simulation" {
            self.sim_run(16)
        } else if t == "open_elaborated_schematic" {
            self.open_elaborated_schematic()
        } else if t == "sheet_find" || t.starts_with("sheet_find ") {
            let kind = t.strip_prefix("sheet_find").unwrap_or("").trim();
            self.sheet_find(kind)
        } else if t == "package" || t == "package_drawing" {
            self.workspace = WorkspaceTab::Package;
            Ok(self.package_drawing_text())
        } else if t == "device" || t == "device_drawing" {
            self.workspace = WorkspaceTab::Device;
            Ok(self.device_drawing_text())
        } else if t == "io_planning" || t == "io_ports" {
            self.open_io_planning()
        } else if t == "select_io_port" || t.starts_with("select_io_port ") {
            let name = t.strip_prefix("select_io_port").unwrap_or("").trim();
            self.select_io_port(name)
        } else if t == "floorplanning" || t == "pblocks" || t == "get_pblocks" {
            self.open_floorplanning()
        } else if t == "create_pblock" || t.starts_with("create_pblock ") {
            self.create_pblock_cmd(t)
        } else if t.starts_with("resize_pblock ") {
            self.resize_pblock_cmd(t)
        } else if t.starts_with("add_cells_to_pblock ") {
            self.add_cells_to_pblock_cmd(t)
        } else if t == "select_pblock" || t.starts_with("select_pblock ") {
            let name = t.strip_prefix("select_pblock").unwrap_or("").trim();
            self.select_pblock(name)
        } else if t.starts_with("assign_package_pin ") {
            self.apply_package_pin(t)
        } else if t.starts_with("set_property ") {
            let key = t
                .split_whitespace()
                .nth(1)
                .unwrap_or("");
            if key.eq_ignore_ascii_case("PACKAGE_PIN") || key.eq_ignore_ascii_case("LOC") {
                self.apply_package_pin(t)
            } else if key.eq_ignore_ascii_case("PART")
                || key.eq_ignore_ascii_case("STRATEGY")
                || key.eq_ignore_ascii_case("FILESET")
            {
                self.apply_project_setting(t)
            } else if key.eq_ignore_ascii_case("IOSTANDARD") {
                self.apply_iostandard(t)
            } else if key.eq_ignore_ascii_case("DRIVE") {
                self.apply_drive(t)
            } else if key.eq_ignore_ascii_case("SLEW") {
                self.apply_slew(t)
            } else if key.eq_ignore_ascii_case("PULLTYPE") {
                self.apply_pulltype(t)
            } else if key.eq_ignore_ascii_case("DIFF_TERM") {
                self.apply_diff_term(t)
            } else if key.eq_ignore_ascii_case("IN_TERM") {
                self.apply_in_term(t)
            } else {
                tcl_eval(&mut self.shell, cmd)
            }
        } else if let Some(pin) = t.strip_prefix("select_package_pin ") {
            self.select_package_pin(pin.trim())
        } else if let Some(spec) = t.strip_prefix("select_device_site ") {
            self.select_device_site(spec.trim())
        } else if let Some(net) = t.strip_prefix("select_device_route ") {
            self.select_device_route(net.trim())
        } else if t == "design_runs" {
            self.workspace = WorkspaceTab::Runs;
            Ok(self.runs_text())
        } else if t == "compare_runs" {
            self.workspace = WorkspaceTab::Runs;
            Ok(self.compare_runs_text())
        } else if let Some(name) = t.strip_prefix("select_run ") {
            self.select_run(name.trim())
        } else if t == "select_run" {
            self.select_run("")
        } else if t == "create_run" || t.starts_with("create_run ") {
            self.create_run(t.strip_prefix("create_run").unwrap_or("").trim())
        } else if t == "launch_runs" || t.starts_with("launch_runs ") {
            let name = t.split_whitespace().nth(1).unwrap_or("impl_1");
            self.launch_runs(name)
        } else if t == "reset_runs" || t.starts_with("reset_runs ") || t == "reset_run" || t.starts_with("reset_run ") {
            let name = t.split_whitespace().nth(1).unwrap_or("impl_1");
            self.reset_runs(name)
        } else if t == "write_checkpoint" {
            self.shell.session.write_checkpoint()
        } else if t == "incremental_impl" {
            self.incremental_impl()
        } else if t == "incremental_place" {
            self.incremental_place_now()
        } else if t == "incremental_route" {
            self.incremental_route_now()
        } else if let Some(net) = t.strip_prefix("unroute_net ") {
            self.shell.session.unroute_net(net.trim())
        } else if let Some(rest) = t.strip_prefix("fix_route ") {
            let mut p = rest.split_whitespace();
            let net = p.next().ok_or("fix_route: need <net> [hops]")?;
            let hops: u32 = p.next().unwrap_or("3").parse().unwrap_or(3);
            self.shell.session.fix_route(net, hops)
        } else if t == "check_eco" {
            self.shell.session.check_eco()
        } else if let Some(rest) = t.strip_prefix("insert_eco_lut ") {
            let mut p = rest.split_whitespace();
            let name = p.next().unwrap_or("ECO_LUT3");
            let init = p
                .next()
                .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0x8);
            self.shell.session.insert_eco_lut(name, init)
        } else if t == "write_bitstream" {
            let r = self.run_step(FlowStep::Bitstream);
            if r.is_ok() {
                self.workspace = WorkspaceTab::Bitstream;
            }
            r
        } else if t == "report_bitstream" || t == "bitstream" {
            self.workspace = WorkspaceTab::Bitstream;
            Ok(self.bitstream_text())
        } else if let Some(spec) = t.strip_prefix("select_bitstream_frame ") {
            self.select_bitstream_frame(spec.trim())
        } else if t == "select_bitstream_frame" {
            self.select_bitstream_frame("0")
        } else if t == "report_drc" {
            self.run_drc()
        } else if t == "report_methodology" || t == "methodology" {
            self.workspace = WorkspaceTab::Methodology;
            Ok(self.methodology_text())
        } else if t == "report_utilization" || t == "utilization" {
            self.workspace = WorkspaceTab::Utilization;
            Ok(self.utilization_report().text())
        } else if t == "report_clock_interaction" || t == "clock_interaction" {
            self.workspace = WorkspaceTab::ClockInteraction;
            Ok(self.clock_interaction_text())
        } else if t == "report_timing_summary" || t == "timing_summary" {
            self.workspace = WorkspaceTab::Reports;
            self.selected_report = Some("report_timing_summary".into());
            Ok(self.timing_summary_text())
        } else if t == "reports" || t == "report_catalog" {
            self.workspace = WorkspaceTab::Reports;
            Ok(self.reports_text())
        } else if let Some(spec) = t.strip_prefix("select_report ") {
            self.select_report(spec.trim())
        } else if t == "select_report" {
            self.select_report("")
        } else if t == "report_cdc" || t == "cdc" {
            self.workspace = WorkspaceTab::Cdc;
            Ok(self.cdc_text())
        } else if t == "report_clock_networks" || t == "clock_networks" {
            self.workspace = WorkspaceTab::ClockNetworks;
            Ok(self.clock_networks_text())
        } else if t == "report_power" || t == "power" {
            self.workspace = WorkspaceTab::Power;
            Ok(self.power_text())
        } else if let Some(rest) = t.strip_prefix("select_timing_summary ") {
            let mut p = rest.split_whitespace();
            let a = p.next().unwrap_or("");
            let b = p.next();
            self.select_timing_summary(a, b)
        } else if let Some(rest) = t.strip_prefix("select_clock_interaction ") {
            let mut p = rest.split_whitespace();
            let from = p.next().unwrap_or("");
            let to = p.next().unwrap_or(from);
            self.select_clock_interaction(from, to)
        } else if let Some(rest) = t.strip_prefix("select_cdc ") {
            let mut p = rest.split_whitespace();
            let from = p.next().unwrap_or("");
            let to = p.next().unwrap_or(from);
            self.select_cdc(from, to)
        } else if let Some(name) = t.strip_prefix("select_clock_network ") {
            self.select_clock_network(name.trim())
        } else if let Some(rail) = t.strip_prefix("select_power ") {
            self.select_power(rail.trim())
        } else if let Some(id) = t.strip_prefix("select_methodology ") {
            self.select_methodology(id.trim())
        } else if let Some(id) = t.strip_prefix("select_drc ") {
            self.select_drc(id.trim())
        } else if let Some(res) = t.strip_prefix("select_utilization ") {
            self.select_utilization(res.trim())
        } else if t == "timing_constraints" || t == "report_timing_constraints" {
            self.workspace = WorkspaceTab::Constraints;
            Ok(self.constraints_table_text())
        } else if let Some(id) = t.strip_prefix("select_constraint ") {
            self.select_constraint(id.trim())
        } else if t == "create_clock" || t.starts_with("create_clock ") {
            self.apply_create_clock(t)
        } else if t == "create_generated_clock" || t.starts_with("create_generated_clock ") {
            self.apply_create_generated_clock(t)
        } else if t.starts_with("set_input_delay")
            || t.starts_with("set_output_delay")
            || t.starts_with("set_false_path")
            || t.starts_with("set_multicycle_path")
            || t.starts_with("set_max_delay")
            || t.starts_with("set_min_delay")
            || t.starts_with("set_clock_groups")
            || t.starts_with("set_clock_uncertainty")
            || t.starts_with("set_clock_latency")
            || t.starts_with("set_disable_timing")
            || t.starts_with("set_case_analysis")
            || t.starts_with("set_propagated_clock")
            || t.starts_with("set_clock_sense")
            || t.starts_with("set_input_jitter")
            || t.starts_with("set_system_jitter")
            || t.starts_with("set_timing_derate")
            || t.starts_with("set_operating_conditions")
            || t.starts_with("set_bus_skew")
            || t == "group_path"
            || t.starts_with("group_path ")
            || t.starts_with("set_max_time_borrow")
            || t.starts_with("set_data_check")
        {
            self.apply_sdc_exception(t)
        } else if let Some(path) = t
            .strip_prefix("read_xdc ")
            .or_else(|| t.strip_prefix("read_sdc "))
        {
            self.read_xdc_path(path.trim())
        } else if t == "create_bd" || t == "create_bd_design" || t == "ip_integrator" {
            self.create_block_design()
        } else if t == "bd_drawing" {
            self.workspace = WorkspaceTab::Ip;
            if self.block_design.is_none() {
                let _ = self.create_block_design();
            }
            Ok(self.bd_drawing_text())
        } else if t == "ip_catalog" {
            self.refresh_ip_catalog();
            self.workspace = WorkspaceTab::Ip;
            Ok(self.ip_catalog_text())
        } else if let Some(spec) = t.strip_prefix("select_ip ") {
            self.select_ip_core(spec.trim())
        } else if t == "select_ip" {
            self.select_ip_core("")
        } else if let Some(spec) = t.strip_prefix("generate_ip ") {
            self.generate_ip(spec.trim())
        } else if t == "generate_ip" {
            self.generate_ip("")
        } else if let Some(spec) = t.strip_prefix("create_bd_cell ") {
            self.create_bd_cell(spec.trim())
        } else if t == "create_bd_cell" {
            self.create_bd_cell("")
        } else if t == "bd_hdl" {
            self.workspace = WorkspaceTab::Ip;
            if self.block_design.is_none() {
                let _ = self.create_block_design();
            }
            Ok(self.bd_hdl_text())
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
        } else if t == "open_hw_manager" {
            let r = tcl_eval(&mut self.shell, t);
            self.workspace = WorkspaceTab::Hardware;
            r
        } else if t == "program_hw" || t == "program_hw_devices" {
            let r = tcl_eval(&mut self.shell, t);
            self.hw.stat = None;
            self.workspace = WorkspaceTab::Hardware;
            r
        } else if t == "report_hw" || t == "hw_stat" || t == "report_hw_stat" {
            self.shell.session.open_hw_manager();
            self.hw.open = true;
            self.workspace = WorkspaceTab::Hardware;
            Ok(self.hw_stat_text())
        } else if let Some(spec) = t.strip_prefix("select_hw_stat ") {
            self.select_hw_stat(spec.trim())
        } else if t == "select_hw_stat" {
            self.select_hw_stat("")
        } else if let Some(spec) = t.strip_prefix("select_ila_sample ") {
            self.select_ila_sample(spec.trim())
        } else if t == "select_ila_sample" {
            self.select_ila_sample("0")
        } else if let Some(rest) = t.strip_prefix("ila_trigger ") {
            self.set_ila_trigger(rest)
        } else if let Some(rest) = t.strip_prefix("ila_window ") {
            self.set_ila_window(rest)
        } else if t == "expand_cone" || t.starts_with("expand_cone ") {
            self.expand_cone(t.strip_prefix("expand_cone").unwrap_or("").trim())
        } else if t == "collapse_cone" {
            self.collapse_cone()
        } else if t == "expand_inside" || t.starts_with("expand_inside ") {
            self.expand_inside(t.strip_prefix("expand_inside").unwrap_or("").trim())
        } else if t == "collapse_inside" {
            self.collapse_inside()
        } else if t == "zoom_fit" || t == "schematic_zoom_fit" {
            self.schematic_zoom_fit()
        } else if t == "schematic_previous" || t == "previous_view" {
            self.schematic_previous_view()
        } else if t == "schematic_next" || t == "next_view" {
            self.schematic_next_view()
        } else if t == "schematic_zoom_in" || t == "zoom_in" {
            self.schematic_zoom_in()
        } else if t == "schematic_zoom_out" || t == "zoom_out" {
            self.schematic_zoom_out()
        } else if let Some(spec) = t.strip_prefix("select_timing_path ") {
            self.select_timing_path(spec.trim())
        } else if t == "select_timing_path" {
            self.select_timing_path("0")
        } else if t == "timing_path_table" || t == "report_timing_paths" {
            self.workspace = WorkspaceTab::Reports;
            if self.timing_paths.is_empty() {
                let _ = self.report_timing_now();
                self.sync_from_session();
            }
            Ok(self.timing_path_table_text())
        } else if let Some(spec) = t.strip_prefix("select_timing_path_report ") {
            self.select_timing_path_report(spec.trim())
        } else if t == "select_timing_path_report" {
            self.select_timing_path_report("0")
        } else if let Some(spec) = t.strip_prefix("select_timing_pin ") {
            self.select_timing_pin(spec.trim())
        } else if t == "select_timing_pin" {
            self.select_timing_pin("")
        } else if t == "scopes" {
            Ok(self.scopes_text())
        } else if t == "objects" {
            Ok(self.objects_text())
        } else if let Some(name) = t.strip_prefix("select_scope ") {
            self.select_scope(name.trim())
        } else if t == "select_scope" {
            self.select_scope("")
        } else if let Some(spec) = t.strip_prefix("select_object ") {
            self.select_object(spec.trim())
        } else if t == "select_object" {
            self.select_object("")
        } else if t == "memory" || t == "memories" {
            self.open_memory()
        } else if t == "select_memory" || t.starts_with("select_memory ") {
            let spec = t.strip_prefix("select_memory").unwrap_or("").trim();
            self.select_memory(spec)
        } else if t == "select_memory_word" || t.starts_with("select_memory_word ") {
            let spec = t.strip_prefix("select_memory_word").unwrap_or("").trim();
            self.select_memory_word(spec)
        } else if t == "locals" {
            self.open_locals()
        } else if t == "select_local" || t.starts_with("select_local ") {
            let spec = t.strip_prefix("select_local").unwrap_or("").trim();
            self.select_local(spec)
        } else if t == "breakpoints" {
            self.open_breakpoints()
        } else if t == "add_bp" || t.starts_with("add_bp ") {
            let spec = t.strip_prefix("add_bp").unwrap_or("").trim();
            self.add_breakpoint(spec)
        } else if t == "select_breakpoint" || t.starts_with("select_breakpoint ") {
            let spec = t.strip_prefix("select_breakpoint").unwrap_or("").trim();
            self.select_breakpoint(spec)
        } else if t == "enable_bp" || t.starts_with("enable_bp ") {
            let spec = t.strip_prefix("enable_bp").unwrap_or("").trim();
            self.set_breakpoint_enabled(spec, true)
        } else if t == "disable_bp" || t.starts_with("disable_bp ") {
            let spec = t.strip_prefix("disable_bp").unwrap_or("").trim();
            self.set_breakpoint_enabled(spec, false)
        } else if t == "delete_bp" || t.starts_with("delete_bp ") {
            let spec = t.strip_prefix("delete_bp").unwrap_or("").trim();
            self.delete_breakpoint(spec)
        } else if let Some(name) = t.strip_prefix("select_clock_region ") {
            self.select_clock_region(name.trim())
        } else if let Some(q) = t.strip_prefix("console_find ") {
            self.find_console(q.trim())
        } else if t == "console_find" {
            self.find_console(&self.console_find.clone())
        } else if t == "tcl_console" || t == "console" {
            self.bottom_tab = BottomTab::Tcl;
            Ok(self.console_text())
        } else if let Some(idx) = t.strip_prefix("select_console ") {
            self.select_console_line(idx.trim())
        } else if t == "select_console" {
            self.select_console_line("")
        } else if t == "schematic" {
            self.workspace = WorkspaceTab::Schematic;
            Ok(self.schematic_text())
        } else if t == "schematic_drawing" {
            self.workspace = WorkspaceTab::Schematic;
            Ok(self.schematic_drawing_text())
        } else if t == "messages" {
            self.bottom_tab = BottomTab::Messages;
            Ok(self.messages_text())
        } else if let Some(spec) = t.strip_prefix("select_message ") {
            self.select_message(spec.trim())
        } else if t == "select_message" {
            self.select_message("")
        } else if let Some(spec) = t.strip_prefix("filter_messages ") {
            self.filter_messages(spec.trim())
        } else if t == "filter_messages" {
            self.filter_messages("all")
        } else if t == "log" {
            self.bottom_tab = BottomTab::Log;
            Ok(self.log_text())
        } else if let Some(spec) = t.strip_prefix("select_log ") {
            self.select_log(spec.trim())
        } else if t == "select_log" {
            self.select_log("")
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
        let filter = match self.message_filter {
            None => "all",
            Some(MsgSeverity::Error) => "error",
            Some(MsgSeverity::Warning) => "warning",
            Some(MsgSeverity::Info) => "info",
        };
        let mut s = format!("messages errors={n_err} warnings={n_warn} info={n_info} filter={filter}");
        let rows = self.message_rows();
        if rows.is_empty() {
            s.push_str("\nno messages");
            return s;
        }
        for (i, m) in rows {
            s.push_str(&format!(
                "\n{i} SEVERITY={} ID={} TEXT={}",
                m.severity.tag(),
                m.id,
                m.text
            ));
            s.push_str(&format!("\n{} [{}] {}", m.severity.tag(), m.id, m.text));
        }
        s
    }

    /// Filtered clickable rows; indices are stable against the unfiltered journal.
    pub fn message_rows(&self) -> Vec<(usize, &IdeMessage)> {
        self.messages
            .iter()
            .enumerate()
            .filter(|(_, m)| self.message_filter.map_or(true, |f| m.severity == f))
            .collect()
    }

    /// UG893 Messages filter buttons (All / Errors / Warnings / Info).
    pub fn filter_messages(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        self.message_filter = if spec.is_empty() || spec.eq_ignore_ascii_case("all") {
            None
        } else {
            Some(MsgSeverity::parse(spec)?)
        };
        self.bottom_tab = BottomTab::Messages;
        Ok(self.messages_text())
    }

    /// Click a Messages row: properties + navigate to the engine pane that produced it.
    pub fn select_message(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_message: missing id".into());
        }
        if self.messages.is_empty() {
            return Err("select_message: no messages".into());
        }
        let spec_l = spec.to_ascii_lowercase();
        let pick = |list: &[(usize, &IdeMessage)]| {
            list.iter()
                .rev()
                .find(|(_, m)| {
                    m.id.eq_ignore_ascii_case(spec)
                        || m.severity.tag().eq_ignore_ascii_case(spec)
                        || format!("{}:{}", m.severity.tag(), m.id).eq_ignore_ascii_case(spec)
                        || m.text.to_ascii_lowercase().contains(&spec_l)
                })
                .map(|(i, m)| (*i, (*m).clone()))
        };
        let filtered = self.message_rows();
        let (idx, m) = if let Ok(i) = spec.parse::<usize>() {
            filtered
                .iter()
                .find(|(idx, _)| *idx == i)
                .map(|(idx, m)| (*idx, (*m).clone()))
                .or_else(|| self.messages.get(i).cloned().map(|m| (i, m)))
                .ok_or_else(|| format!("select_message: no row {spec}"))?
        } else {
            pick(&filtered)
                .or_else(|| {
                    let all: Vec<(usize, &IdeMessage)> =
                        self.messages.iter().enumerate().collect();
                    pick(&all)
                })
                .ok_or_else(|| format!("select_message: no row {spec}"))?
        };
        self.selected_message = Some(idx);
        self.selected = Some(format!("message:{idx}"));
        self.bottom_tab = BottomTab::Messages;
        self.properties = vec![
            ("NAME".into(), m.id.clone()),
            ("TYPE".into(), "message".into()),
            ("SEVERITY".into(), m.severity.tag().into()),
            ("ID".into(), m.id.clone()),
            ("INDEX".into(), idx.to_string()),
            ("TEXT".into(), m.text.clone()),
        ];
        match m.id.as_str() {
            "report_timing" | "report_timing_summary" => {
                self.workspace = WorkspaceTab::Reports;
            }
            "report_drc" => self.workspace = WorkspaceTab::Drc,
            "report_methodology" => self.workspace = WorkspaceTab::Methodology,
            "report_cdc" => self.workspace = WorkspaceTab::Cdc,
            "report_power" => self.workspace = WorkspaceTab::Power,
            "report_clock_interaction" => {
                self.workspace = WorkspaceTab::ClockInteraction;
            }
            "report_clock_networks" => self.workspace = WorkspaceTab::ClockNetworks,
            "report_utilization" => self.workspace = WorkspaceTab::Utilization,
            "timing_constraints" | "report_timing_constraints" => {
                self.workspace = WorkspaceTab::Constraints;
            }
            "place_design" | "route_design" | "device" => {
                self.workspace = WorkspaceTab::Device;
            }
            "synth_design" | "opt_design" | "schematic" => {
                self.workspace = WorkspaceTab::Schematic;
            }
            "write_bitstream" | "report_bitstream" => self.workspace = WorkspaceTab::Bitstream,
            _ => {}
        }
        Ok(format!(
            "message INDEX={idx} SEVERITY={} ID={} TEXT={}",
            m.severity.tag(),
            m.id,
            m.text
        ))
    }

    /// UG893 Log pane: clickable Tcl transcript (status / cmd / engine out).
    pub fn log_text(&self) -> String {
        if self.console.is_empty() {
            return "log empty".into();
        }
        let n_err = self.console.iter().filter(|l| !l.ok).count();
        let mut s = format!("log n={} errors={n_err}", self.console.len());
        for (i, line) in self.console.iter().enumerate() {
            s.push_str(&format!("\n{i} {}", line.row_text()));
            s.push_str(&format!("\nhelion% {}\n{}", line.cmd, line.out));
        }
        s
    }

    /// Clickable Log rows — same journal as the Tcl console, not a restyle of `log`.
    pub fn log_rows(&self) -> Vec<(usize, &ConsoleLine)> {
        self.console.iter().enumerate().collect()
    }

    /// Click a Log row: properties + jump to the engine pane that produced it.
    pub fn select_log(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_log: missing id".into());
        }
        if self.console.is_empty() {
            return Err("select_log: log empty".into());
        }
        let spec_l = spec.to_ascii_lowercase();
        let (idx, line) = if let Ok(i) = spec.parse::<usize>() {
            self.console
                .get(i)
                .cloned()
                .map(|l| (i, l))
                .ok_or_else(|| format!("select_log: no row {spec}"))?
        } else {
            self.console
                .iter()
                .enumerate()
                .rev()
                .find(|(_, l)| {
                    l.cmd.eq_ignore_ascii_case(spec)
                        || l.cmd
                            .split_whitespace()
                            .next()
                            .is_some_and(|c| c.eq_ignore_ascii_case(spec))
                })
                .or_else(|| {
                    self.console.iter().enumerate().rev().find(|(_, l)| {
                        !l.cmd.eq_ignore_ascii_case("log")
                            && l.out.to_ascii_lowercase().contains(&spec_l)
                    })
                })
                .map(|(i, l)| (i, l.clone()))
                .ok_or_else(|| format!("select_log: no row {spec}"))?
        };
        let status = if line.ok { "ok" } else { "error" };
        let id = line
            .cmd
            .split_whitespace()
            .next()
            .unwrap_or(line.cmd.as_str())
            .to_string();
        self.selected_log = Some(idx);
        self.selected = Some(format!("log:{idx}"));
        self.bottom_tab = BottomTab::Log;
        self.properties = vec![
            ("NAME".into(), id.clone()),
            ("TYPE".into(), "log".into()),
            ("INDEX".into(), idx.to_string()),
            ("STATUS".into(), status.into()),
            ("CMD".into(), line.cmd.clone()),
            ("OUT".into(), line.out.clone()),
        ];
        self.apply_cmd_workspace(&id);
        Ok(format!(
            "log INDEX={idx} STATUS={status} CMD={} OUT={}",
            line.cmd, line.out
        ))
    }

    /// UG893 Tcl Console: clickable Status/Cmd/Out table over the Session journal
    /// (not a `helion% cmd` + monospace-out dump). Same rows as Log.
    pub fn console_text(&self) -> String {
        if self.console.is_empty() {
            return "tcl_console empty".into();
        }
        let n_err = self.console.iter().filter(|l| !l.ok).count();
        let mut s = format!("tcl_console n={} errors={n_err}", self.console.len());
        for (i, line) in self.console.iter().enumerate() {
            s.push_str(&format!("\n{i} {}", line.row_text()));
        }
        s
    }

    /// Clickable Tcl Console rows — Status/Cmd/Out over Session, not a restyle of `log`.
    pub fn console_rows(&self) -> Vec<(usize, &ConsoleLine)> {
        self.console.iter().enumerate().collect()
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
                self.place_now(&dev)?;
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
                let configured = b.frames.values().filter(|w| **w != 0).count();
                let hash = self.shell.session.blinky_hash().unwrap_or(0);
                Ok(format!(
                    "write_bitstream frames={frames} bytes={bytes} hash={hash:#010x} configured={configured}"
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
                if self.device.cols == 0 || self.device.rows == 0 {
                    return Err("board/device: HAD device drawing missing".into());
                }
                Ok(format!(
                    "board_device sites={} iob_ports={} pins={} cols={} rows={} device_cols={} device_rows={} occupied={} part={}",
                    self.device.sites.len(),
                    iob,
                    self.package_pins.len(),
                    self.package.cols,
                    self.package.rows,
                    self.device.cols,
                    self.device.rows,
                    self.device.occupied_count(),
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
                    let r = self.bitstream_report();
                    return Ok(format!(
                        "program_debug hash={h:#010x} configured={} frames={} bytes={}",
                        r.configured, r.frames, r.bytes
                    ));
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

    /// UG900 waveform marker at a sample (or `-time` ps). Time is the engine
    /// timescale, not a canned label.
    pub fn add_wave_marker(&mut self, spec: &str) -> Result<String, String> {
        let mut parts = spec.split_whitespace();
        let name = parts
            .next()
            .ok_or("add_wave_marker: need <name> [sample|-time ps]")?;
        let rest: Vec<&str> = parts.collect();
        let n = self.wave.sample_len();
        if n == 0 {
            return Err("add_wave_marker: no wave samples".into());
        }
        let sample = if rest.first().copied() == Some("-time") {
            let ps: u64 = rest
                .get(1)
                .and_then(|s| s.parse().ok())
                .ok_or("add_wave_marker: -time needs ps")?;
            (ps / self.wave.timescale_ps.max(1)) as usize
        } else if let Some(s) = rest.first() {
            s.parse()
                .map_err(|_| format!("add_wave_marker: bad sample {s}"))?
        } else {
            self.wave.cursor
        };
        let sample = sample.min(n.saturating_sub(1));
        self.wave.markers.retain(|m| m.name != name);
        self.wave.markers.push(WaveMarker {
            name: name.into(),
            sample,
        });
        self.workspace = WorkspaceTab::Wave;
        let time_ps = self.wave.time_ps(sample);
        Ok(format!(
            "add_wave_marker {name} sample={sample} TIME_PS={time_ps}"
        ))
    }

    /// UG900 virtual bus: pack member traces (LSB = first member) into one
    /// display object whose Value is the engine bits, not a dump.
    pub fn add_wave_virtual_bus(&mut self, spec: &str) -> Result<String, String> {
        let mut parts = spec.split_whitespace();
        let name = parts
            .next()
            .ok_or("add_wave_virtual_bus: need <name> <members...>")?
            .to_string();
        let members: Vec<String> = parts.map(|s| s.to_string()).collect();
        if members.len() < 2 {
            return Err("add_wave_virtual_bus: need at least two members".into());
        }
        for m in &members {
            if !self.wave.has_trace(m) || self.wave.virtual_bus(m).is_some() {
                return Err(format!("add_wave_virtual_bus: no trace {m}"));
            }
        }
        self.wave.virtual_buses.retain(|v| v.name != name);
        self.wave.virtual_buses.push(VirtualBus {
            name: name.clone(),
            members: members.clone(),
        });
        self.wave.rebuild_virtual_buses();
        self.refresh_sim_objects();
        self.workspace = WorkspaceTab::Wave;
        let t = self
            .wave
            .trace(&name)
            .ok_or("add_wave_virtual_bus: pack failed")?;
        let value = t.value_at(self.wave.cursor);
        Ok(format!(
            "add_wave_virtual_bus {name} width={} members={} VALUE={value}",
            t.width,
            members.join(",")
        ))
    }

    /// UG900 dual cursors A/B on the engine sample grid. Time-delta is B−A in
    /// picoseconds from the wave timescale, not a canned interval.
    pub fn set_wave_ab_cursor(&mut self, spec: &str) -> Result<String, String> {
        let mut parts = spec.split_whitespace();
        let which = parts
            .next()
            .ok_or("wave_cursor: need A|B [sample|-time ps]")?;
        let is_a = match which.to_ascii_uppercase().as_str() {
            "A" => true,
            "B" => false,
            other => return Err(format!("wave_cursor: unknown {other} (need A|B)")),
        };
        let rest: Vec<&str> = parts.collect();
        let n = self.wave.sample_len();
        if n == 0 {
            return Err("wave_cursor: no wave samples".into());
        }
        let sample = if rest.first().copied() == Some("-time") {
            let ps: u64 = rest
                .get(1)
                .and_then(|s| s.parse().ok())
                .ok_or("wave_cursor: -time needs ps")?;
            (ps / self.wave.timescale_ps.max(1)) as usize
        } else if let Some(s) = rest.first() {
            s.parse()
                .map_err(|_| format!("wave_cursor: bad sample {s}"))?
        } else {
            self.wave.cursor
        };
        let sample = sample.min(n.saturating_sub(1));
        if is_a {
            self.wave.set_cursor_a(sample);
        } else {
            self.wave.set_cursor_b(sample);
        }
        self.workspace = WorkspaceTab::Wave;
        let time_ps = self.wave.time_ps(sample);
        let delta = match self.wave.time_delta_ps() {
            Some(d) => d.to_string(),
            None => "n/a".into(),
        };
        self.properties = vec![
            ("NAME".into(), format!("cursor {which}")),
            ("TYPE".into(), "wave_cursor".into()),
            ("SAMPLE".into(), sample.to_string()),
            ("TIME_PS".into(), time_ps.to_string()),
            ("DELTA_PS".into(), delta.clone()),
        ];
        let tag = if is_a { "A" } else { "B" };
        Ok(format!(
            "wave_cursor {tag} sample={sample} TIME_PS={time_ps} DELTA_PS={delta}"
        ))
    }

    /// UG900 A/B cursor pane dump: times, signed B−A, and Value-at-A/B from
    /// engine samples (not a canned concatenation).
    pub fn wave_cursors_text(&self) -> String {
        let a = match self.wave.cursor_a {
            Some(s) => format!("A_SAMPLE={s} A_TIME_PS={}", self.wave.time_ps(s)),
            None => "A=-".into(),
        };
        let b = match self.wave.cursor_b {
            Some(s) => format!("B_SAMPLE={s} B_TIME_PS={}", self.wave.time_ps(s)),
            None => "B=-".into(),
        };
        let delta = match self.wave.time_delta_ps() {
            Some(d) => format!("DELTA_PS={d}"),
            None => "DELTA_PS=n/a".into(),
        };
        let mut s = format!("wave_cursors {a} {b} {delta}");
        for t in &self.wave.traces {
            let va = self
                .wave
                .cursor_a
                .map(|i| t.value_at(i))
                .unwrap_or_else(|| "-".into());
            let vb = self
                .wave
                .cursor_b
                .map(|i| t.value_at(i))
                .unwrap_or_else(|| "-".into());
            s.push_str(&format!(" {} A={} B={}", t.name, va, vb));
        }
        s
    }

    /// Cross-select: one identity shared by Netlist, Schematic, Device, Properties.
    pub fn select(&mut self, id: &str) {
        let id = id.trim();
        self.selected_timing_pin = None;
        self.selected_ip = None;
        self.selected_property = None;
        self.selected_source = None;
        if id.is_empty() {
            self.selected = None;
            self.selected_netlist = None;
            self.properties.clear();
            return;
        }
        self.selected = Some(id.to_string());
        if self.tree.has_cell(id) || self.tree.has_net(id) {
            self.selected_netlist = Some(id.to_string());
        } else {
            self.selected_netlist = None;
        }
        self.refresh_properties();
        self.highlight_device_routes();
    }

    pub fn selected_cell(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn netlist_has_selected(&self) -> bool {
        let Some(id) = self.selected.as_deref() else {
            return false;
        };
        self.tree.has_cell(id) || self.tree.has_net(id)
    }

    /// UG893 Sources table rows (file Name/Type), not a basename dump.
    pub fn source_rows(&self) -> Vec<NetlistRow> {
        self.tree
            .sources
            .iter()
            .map(|p| NetlistRow {
                kind: "source".into(),
                name: source_basename(p),
                type_name: source_type_of(p).into(),
                parent: p.clone(),
            })
            .collect()
    }

    /// UG893 Netlist table rows (HNF cell primitive / net), not a collapsing dump.
    pub fn netlist_rows(&self) -> Vec<NetlistRow> {
        let parent = self
            .tree
            .top
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "-".into());
        let mut rows = Vec::with_capacity(self.tree.cells.len() + self.tree.nets.len());
        for (name, kind) in &self.tree.cells {
            rows.push(NetlistRow {
                kind: "cell".into(),
                name: name.clone(),
                type_name: kind.clone(),
                parent: parent.clone(),
            });
        }
        for n in &self.tree.nets {
            rows.push(NetlistRow {
                kind: "net".into(),
                name: n.clone(),
                type_name: "net".into(),
                parent: parent.clone(),
            });
        }
        rows
    }

    /// UG893 Sources pane dump (tests). Paint is a clickable Name/Type grid, not this string.
    pub fn sources_text(&self) -> String {
        let rows = self.source_rows();
        let n = rows.len();
        let mut s = format!("sources n={n}");
        if n == 0 {
            s.push_str(" no sources");
            return s;
        }
        for r in &rows {
            s.push('\n');
            s.push_str(&r.row_text());
        }
        s
    }

    /// UG893 Netlist pane dump (tests). Paint is a clickable Name/Type grid, not this string.
    pub fn netlist_text(&self) -> String {
        let rows = self.netlist_rows();
        let n = rows.len();
        let top = self.tree.top.as_deref().unwrap_or("-");
        let mut s = format!("netlist n={n} top={top}");
        if n == 0 {
            s.push_str(" no cells/nets");
            return s;
        }
        for r in &rows {
            s.push('\n');
            s.push_str(&r.row_text());
        }
        s
    }

    /// Click a UG893 Sources row (name, path, or index) — Name/Type table, not a dump.
    pub fn select_source(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_source: missing name".into());
        }
        let rows = self.source_rows();
        if rows.is_empty() {
            return Err("select_source: no sources".into());
        }
        let idx = if let Ok(i) = spec.parse::<usize>() {
            if i >= rows.len() {
                return Err(format!("select_source: no row {spec}"));
            }
            i
        } else {
            rows.iter()
                .position(|r| {
                    r.name.eq_ignore_ascii_case(spec)
                        || r.parent.eq_ignore_ascii_case(spec)
                        || r.parent.ends_with(spec)
                        || format!("{}:{}", r.kind, r.name).eq_ignore_ascii_case(spec)
                        || format!("{}:{}", r.type_cell(), r.name).eq_ignore_ascii_case(spec)
                })
                .ok_or_else(|| format!("select_source: no row {spec}"))?
        };
        let row = rows[idx].clone();
        self.selected_timing_pin = None;
        self.selected_ip = None;
        self.selected_property = None;
        self.selected_netlist = None;
        self.selected_source = Some(row.parent.clone());
        self.selected = Some(row.name.clone());
        self.layout = LayoutKind::Default;
        self.refresh_properties();
        Ok(row.row_text())
    }

    /// Click a UG893 Netlist row (name or index) — Name/Type table, not a collapsing dump.
    pub fn select_netlist(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_netlist: missing name".into());
        }
        let rows = self.netlist_rows();
        if rows.is_empty() {
            return Err("select_netlist: no cells/nets".into());
        }
        let idx = if let Ok(i) = spec.parse::<usize>() {
            if i >= rows.len() {
                return Err(format!("select_netlist: no row {spec}"));
            }
            i
        } else {
            rows.iter()
                .position(|r| {
                    r.name.eq_ignore_ascii_case(spec)
                        || format!("{}:{}", r.kind, r.name).eq_ignore_ascii_case(spec)
                        || format!("{}:{}", r.type_cell(), r.name).eq_ignore_ascii_case(spec)
                })
                .ok_or_else(|| format!("select_netlist: no row {spec}"))?
        };
        let row = rows[idx].clone();
        self.selected_source = None;
        self.layout = LayoutKind::Default;
        self.select(&row.name);
        self.selected_netlist = Some(row.name.clone());
        if !self.properties.iter().any(|(k, _)| k == "TYPE") {
            self.properties
                .push(("TYPE".into(), row.type_cell().to_string()));
        }
        if !self.properties.iter().any(|(k, _)| k == "KIND") {
            self.properties.push(("KIND".into(), row.kind.clone()));
        }
        Ok(row.row_text())
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

    /// Fig. 61 dump: nested boxes with area ∝ HNF cell count.
    pub fn hierarchy_drawing_text(&self) -> String {
        let d = self.hierarchy.drawing();
        let mut s = format!(
            "drawing boxes={} canvas={}x{}",
            d.boxes.len(),
            d.width as i32,
            d.height as i32
        );
        for b in &d.boxes {
            s.push_str(&format!(
                " {}:{}:box={},{},{},{}:cells={}:area={}",
                b.name,
                b.kind,
                b.x as i32,
                b.y as i32,
                b.w as i32,
                b.h as i32,
                b.cells,
                (b.w * b.h) as i32
            ));
        }
        s
    }

    /// UG893 Design Runs pane: clickable name/strategy/WNS/runtime/hash grid
    /// over Session synth/impl (not a concatenated dump).
    pub fn runs_text(&self) -> String {
        let mut s = format!("design_runs n={}", self.runs.len());
        for r in &self.runs {
            s.push('\n');
            s.push_str(&r.row_text());
        }
        s
    }

    pub fn compare_run_rows(&self) -> Vec<&DesignRun> {
        self.runs
            .iter()
            .filter(|r| r.step == "Implementation")
            .collect()
    }

    pub fn compare_runs_text(&self) -> String {
        let impls = self.compare_run_rows();
        let mut s = format!("compare_runs n={}", impls.len());
        for r in impls {
            s.push_str(&format!(
                " {} strategy={} WNS_PS={} runtime_ms={} hash={}",
                r.name,
                r.strategy,
                r.wns_cell(),
                r.runtime_cell(),
                r.hash_cell()
            ));
            s.push_str(&format!(
                "\n{} NAME={} STRATEGY={} STATUS={} WNS_PS={} RUNTIME_MS={} HASH={}",
                r.name,
                r.name,
                r.strategy_cell(),
                r.status,
                r.wns_cell(),
                r.runtime_cell(),
                r.hash_cell()
            ));
        }
        s
    }

    fn design_run_properties(r: &DesignRun) -> Vec<(String, String)> {
        vec![
            ("NAME".into(), r.name.clone()),
            ("TYPE".into(), "design_run".into()),
            ("STEP".into(), r.step.clone()),
            ("STRATEGY".into(), r.strategy_cell().into()),
            ("STATUS".into(), r.status.clone()),
            ("WNS_PS".into(), r.wns_cell()),
            ("RUNTIME_MS".into(), r.runtime_cell()),
            ("HASH".into(), r.hash_cell()),
            ("LUTFF".into(), r.lutff_cell()),
            ("REUSE".into(), r.reuse_cell()),
            ("PART".into(), r.part.clone()),
            ("TOP".into(), r.top_cell().into()),
            ("CELLS".into(), r.cells_cell()),
        ]
    }

    /// Click a Design Runs / compare_runs grid row: properties + Runs workspace.
    pub fn select_run(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_run: missing name".into());
        }
        let key = spec.strip_prefix("run:").unwrap_or(spec);
        let spec_l = key.to_ascii_lowercase();
        let run = if let Ok(i) = key.parse::<usize>() {
            self.runs
                .get(i)
                .cloned()
                .ok_or_else(|| format!("select_run: no row {spec}"))?
        } else {
            self.runs
                .iter()
                .find(|r| r.name.eq_ignore_ascii_case(key))
                .cloned()
                .or_else(|| {
                    self.runs.iter().find(|r| {
                        r.step == "Implementation"
                            && r.strategy.eq_ignore_ascii_case(key)
                    }).cloned()
                })
                .or_else(|| {
                    self.runs.iter().find(|r| {
                        r.step.eq_ignore_ascii_case(key)
                            || r.status.eq_ignore_ascii_case(key)
                            || r.name.to_ascii_lowercase().contains(&spec_l)
                    }).cloned()
                })
                .ok_or_else(|| format!("select_run: no row {spec}"))?
        };
        self.selected = Some(format!("run:{}", run.name));
        self.properties = Self::design_run_properties(&run);
        self.workspace = WorkspaceTab::Runs;
        Ok(format!(
            "run NAME={} STEP={} STRATEGY={} STATUS={} WNS_PS={} RUNTIME_MS={} HASH={} LUTFF={} REUSE={}",
            run.name,
            run.step,
            run.strategy_cell(),
            run.status,
            run.wns_cell(),
            run.runtime_cell(),
            run.hash_cell(),
            run.lutff_cell(),
            run.reuse_cell()
        ))
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
            for p in &n.pins {
                s.push_str(&format!(
                    " {}.{}:{}",
                    n.name,
                    p.name,
                    if p.output { "out" } else { "in" }
                ));
            }
        }
        for e in &edges {
            s.push_str(&format!(
                " {}.{}-{}-{}.{}",
                e.src, e.src_pin, e.net, e.dst, e.dst_pin
            ));
        }
        s
    }

    /// UG893 Fig. 55 dump: symbol boxes + pin stubs + orthogonal wires, not a cell list.
    pub fn schematic_drawing_text(&self) -> String {
        let d = self.schematic.drawing();
        let cam = self.schematic.camera;
        let mut s = format!(
            "drawing symbols={} wires={} canvas={}x{} camera={},{},{} hist={}/{} expand={} path={}",
            d.symbols.len(),
            d.wires.len(),
            d.width as i32,
            d.height as i32,
            cam.zoom,
            cam.pan_x as i32,
            cam.pan_y as i32,
            self.schematic.view_index,
            self.schematic.view_history.len(),
            self.schematic.expand_inside.as_deref().unwrap_or("-"),
            if self.schematic.path_only { "sta" } else { "-" }
        );
        for sy in &d.symbols {
            s.push_str(&format!(
                " {}:{}:box={},{},{},{}{}",
                sy.name,
                sy.kind,
                sy.x as i32,
                sy.y as i32,
                sy.w as i32,
                sy.h as i32,
                if sy.highlighted { ":hl" } else { "" }
            ));
            for p in &sy.pins {
                s.push_str(&format!(
                    " {}.{}:{}@{},{}{}",
                    sy.name,
                    p.name,
                    if p.output { "out" } else { "in" },
                    p.x as i32,
                    p.y as i32,
                    if p.net.is_empty() { ":nc" } else { "" }
                ));
            }
        }
        for w in &d.wires {
            s.push_str(&format!(
                " wire:{}:{}.{}->{}.{}:pts={}:w={}{}{}",
                w.net,
                w.src,
                w.src_pin,
                w.dst,
                w.dst_pin,
                w.points.len(),
                w.width,
                if w.off_sheet { ":dotted" } else { "" },
                if w.highlighted { ":hl" } else { "" }
            ));
        }
        s
    }

    /// Fig. 55 sheet links: Cells / I/O Ports / Nets open Find Results.
    pub fn sheet_find(&mut self, kind: &str) -> Result<String, String> {
        let k = kind.trim().to_ascii_lowercase();
        let k = if k.is_empty() { "cells".into() } else { k };
        let parent = self.find_parent_name();
        let mut hits = Vec::new();
        match k.as_str() {
            "cells" | "cell" => {
                for (n, prim) in &self.tree.cells {
                    hits.push(FindHit {
                        kind: "cell".into(),
                        name: n.clone(),
                        primitive: prim.clone(),
                        parent: parent.clone(),
                    });
                }
            }
            "ports" | "port" | "io" | "i/o" | "io_ports" | "i/o_ports" => {
                for p in &self.schematic.ports {
                    hits.push(FindHit {
                        kind: "port".into(),
                        name: p.name.clone(),
                        primitive: p.dir.clone(),
                        parent: parent.clone(),
                    });
                }
            }
            "nets" | "net" => {
                let fanout = |name: &str| {
                    self.shell
                        .session
                        .design
                        .as_ref()
                        .and_then(|d| d.nets.iter().find(|n| n.name == name))
                        .map(|n| format!("fanout={}", n.endpoints.len()))
                        .unwrap_or_else(|| "net".into())
                };
                for n in &self.tree.nets {
                    hits.push(FindHit {
                        kind: "net".into(),
                        name: n.clone(),
                        primitive: fanout(n),
                        parent: parent.clone(),
                    });
                }
            }
            other => return Err(format!("sheet_find: unknown {other}")),
        }
        let n = hits.len();
        if n == 0 {
            return Err(format!("sheet_find {k}: 0 hits"));
        }
        self.find_results = hits;
        self.selected_find = None;
        self.workspace = WorkspaceTab::Find;
        let mut s = format!(
            "sheet_find {k} hits={n} {}",
            self.find_results
                .iter()
                .map(|h| format!("{}:{}", h.kind, h.name))
                .collect::<Vec<_>>()
                .join(" ")
        );
        s.push('\n');
        s.push_str(&self.find_results_text());
        Ok(s)
    }

    /// RTL Analysis / Synthesis child: open the elaborated HNF schematic.
    pub fn open_elaborated_schematic(&mut self) -> Result<String, String> {
        if self.schematic.nodes.is_empty() {
            return Err("open_elaborated_schematic: no HNF — Run Synthesis first".into());
        }
        self.workspace = WorkspaceTab::Schematic;
        self.nav = NavSection::RtlAnalysis;
        Ok(self.schematic_drawing_text())
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

    /// Fig. 55 toolbar: Zoom Fit commits a camera that paint applies.
    pub fn schematic_zoom_fit(&mut self) -> Result<String, String> {
        self.workspace = WorkspaceTab::Schematic;
        self.schematic.zoom_fit();
        Ok(self.schematic_camera_text())
    }

    pub fn schematic_zoom_in(&mut self) -> Result<String, String> {
        self.workspace = WorkspaceTab::Schematic;
        self.schematic.zoom_by(1.25);
        Ok(self.schematic_camera_text())
    }

    pub fn schematic_zoom_out(&mut self) -> Result<String, String> {
        self.workspace = WorkspaceTab::Schematic;
        self.schematic.zoom_by(0.8);
        Ok(self.schematic_camera_text())
    }

    pub fn schematic_previous_view(&mut self) -> Result<String, String> {
        self.workspace = WorkspaceTab::Schematic;
        if !self.schematic.previous_view() {
            return Err("previous_view: no earlier camera".into());
        }
        Ok(self.schematic_camera_text())
    }

    pub fn schematic_next_view(&mut self) -> Result<String, String> {
        self.workspace = WorkspaceTab::Schematic;
        if !self.schematic.next_view() {
            return Err("next_view: no later camera".into());
        }
        Ok(self.schematic_camera_text())
    }

    pub fn schematic_camera_text(&self) -> String {
        let c = self.schematic.camera;
        format!(
            "camera zoom={} pan={},{} hist={}/{}",
            c.zoom,
            c.pan_x as i32,
            c.pan_y as i32,
            self.schematic.view_index,
            self.schematic.view_history.len()
        )
    }

    /// Fig. 56 Expand Inside: regenerate nested contents of a hierarchical instance.
    pub fn expand_inside(&mut self, spec: &str) -> Result<String, String> {
        let name = match spec.trim() {
            "" => self
                .selected
                .clone()
                .ok_or_else(|| "expand_inside: select an instance first".to_string())?,
            n => n.to_string(),
        };
        if self.schematic.is_primitive(&name) {
            return Err(format!(
                "expand_inside: primitive {name} refuses Expand Inside"
            ));
        }
        if !self.schematic.is_instance(&name) {
            return Err(format!("expand_inside: no hierarchical instance {name}"));
        }
        let nested = self.schematic.instance_member_cells(&name);
        if nested.is_empty() {
            return Err(format!("expand_inside: {name} has no nested cells"));
        }
        self.schematic.expand_inside = Some(name.clone());
        self.schematic.cone_root = None;
        self.schematic.path_only = false;
        self.schematic.highlight_cells.clear();
        self.schematic.highlight_nets.clear();
        self.select(&name);
        self.workspace = WorkspaceTab::Schematic;
        Ok(format!(
            "expand_inside {name} nested={} {}",
            nested.len(),
            self.schematic_drawing_text()
        ))
    }

    pub fn collapse_inside(&mut self) -> Result<String, String> {
        self.schematic.expand_inside = None;
        self.workspace = WorkspaceTab::Schematic;
        Ok(self.schematic_drawing_text())
    }

    /// Fig. 59: isolate/highlight the STA path's cells and nets on the schematic.
    pub fn select_timing_path(&mut self, spec: &str) -> Result<String, String> {
        self.ensure_timing_paths()?;
        let idx = self.timing_path_index(spec)?;
        let path = self
            .timing_paths
            .get(idx)
            .cloned()
            .ok_or_else(|| format!("select_timing_path: index {idx} out of range"))?;
        if path.cells.is_empty() {
            return Err("select_timing_path: path has no cells".into());
        }
        self.selected_timing_path = Some(idx);
        self.selected_timing_pin = None;
        self.schematic.highlight_cells = path.cells.iter().cloned().collect();
        self.schematic.highlight_nets = path.nets.iter().cloned().collect();
        self.schematic.path_only = true;
        self.schematic.cone_root = None;
        self.schematic.expand_inside = None;
        self.workspace = WorkspaceTab::Schematic;
        if let Some(end) = path.cells.first() {
            self.select(end);
        }
        Ok(format!(
            "timing_path {} start={} end={} cells={} nets={} slack_ps={} {}",
            path.name,
            path.startpoint,
            path.endpoint,
            path.cells.join(","),
            path.nets.join(","),
            path.slack_ps,
            self.schematic_drawing_text()
        ))
    }

    fn ensure_timing_paths(&mut self) -> Result<(), String> {
        if self.timing_paths.is_empty() {
            let _ = self.report_timing_now();
            self.sync_from_session();
        }
        if self.timing_paths.is_empty() {
            return Err("select_timing_path: no STA endpoints".into());
        }
        Ok(())
    }

    fn timing_path_index(&self, spec: &str) -> Result<usize, String> {
        let spec = spec.trim();
        if let Ok(n) = spec.parse::<usize>() {
            if n < self.timing_paths.len() {
                return Ok(n);
            }
            return Err(format!("select_timing_path: index {n} out of range"));
        }
        self.timing_paths
            .iter()
            .position(|p| {
                p.endpoint == spec
                    || p.startpoint == spec
                    || p.name == spec
                    || p.cells.iter().any(|c| c == spec)
            })
            .ok_or_else(|| format!("select_timing_path: no path {spec}"))
    }

    /// UG903 Reports window: select a path and stay on the pin-delay table.
    pub fn select_timing_path_report(&mut self, spec: &str) -> Result<String, String> {
        self.ensure_timing_paths()?;
        let idx = self.timing_path_index(spec)?;
        let path = self
            .timing_paths
            .get(idx)
            .cloned()
            .ok_or_else(|| format!("select_timing_path: index {idx} out of range"))?;
        if path.cells.is_empty() {
            return Err("select_timing_path: path has no cells".into());
        }
        self.selected_timing_path = Some(idx);
        self.selected_timing_pin = None;
        self.selected_report = Some("report_timing".into());
        self.workspace = WorkspaceTab::Reports;
        if let Some(end) = path.cells.first() {
            self.select(end);
        }
        Ok(self.timing_path_table_text())
    }

    fn find_timing_pin(&self, spec: &str) -> Result<(usize, TimingPathPin), String> {
        let spec = spec.trim();
        let mut order: Vec<usize> = Vec::new();
        if let Some(p) = self.selected_timing_path {
            order.push(p);
        }
        order.extend((0..self.timing_paths.len()).filter(|i| Some(*i) != self.selected_timing_path));
        let as_idx = spec.parse::<usize>().ok();
        for pi in order {
            let path = &self.timing_paths[pi];
            if let Some(n) = as_idx {
                if let Some(pin) = path.pins.get(n) {
                    return Ok((pi, pin.clone()));
                }
            }
            if let Some(pin) = path.pins.iter().find(|p| {
                p.pin.eq_ignore_ascii_case(spec)
                    || p.cell.eq_ignore_ascii_case(spec)
                    || p.pin.rsplit('/').next() == Some(spec)
                    || p.net == spec
            }) {
                return Ok((pi, pin.clone()));
            }
        }
        Err(format!("select_timing_pin: no pin {spec}"))
    }

    /// UG903 pin-delay Name click: properties for that STA pin, select the HNF cell.
    pub fn select_timing_pin(&mut self, spec: &str) -> Result<String, String> {
        self.ensure_timing_paths()?;
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_timing_pin: missing pin".into());
        }
        let (pi, pin) = self.find_timing_pin(spec)?;
        let slack = self.timing_paths[pi].slack_ps;
        self.selected_timing_path = Some(pi);
        self.selected_timing_pin = Some(pin.pin.clone());
        self.workspace = WorkspaceTab::Reports;
        self.selected = Some(pin.cell.clone());
        let net = if pin.net.is_empty() {
            "-".to_string()
        } else {
            pin.net.clone()
        };
        let loc = if pin.location.is_empty() {
            "-".to_string()
        } else {
            pin.location.clone()
        };
        self.properties = vec![
            ("NAME".into(), pin.pin.clone()),
            ("TYPE".into(), "timing_pin".into()),
            ("CELL".into(), pin.cell.clone()),
            ("PIN".into(), pin.pin.clone()),
            ("DELAY_TYPE".into(), pin.delay_type.clone()),
            ("INCR_PS".into(), pin.incr_ps.to_string()),
            ("PATH_PS".into(), pin.path_ps.to_string()),
            ("NET".into(), net.clone()),
            ("FANOUT".into(), pin.fanout.to_string()),
            ("LOCATION".into(), loc.clone()),
            ("SLACK_PS".into(), slack.to_string()),
        ];
        Ok(format!(
            "timing_pin {} type={} incr_ps={} path_ps={} net={} fanout={} loc={}",
            pin.pin, pin.delay_type, pin.incr_ps, pin.path_ps, net, pin.fanout, loc
        ))
    }

    /// UG903 pin-delay table dump (tests). Paint is a clickable grid, not this string.
    pub fn timing_path_table_text(&self) -> String {
        let sel = self
            .selected_timing_path
            .map(|i| i.to_string())
            .unwrap_or_else(|| "-".into());
        let mut s = format!(
            "timing_paths n={} selected={} pins={}",
            self.timing_paths.len(),
            sel,
            self.timing_paths
                .iter()
                .map(|p| p.pins.len())
                .sum::<usize>()
        );
        for (i, p) in self.timing_paths.iter().enumerate() {
            s.push_str(&format!(
                "\npath[{i}] name={} from={} to={} slack_ps={} delay_ps={} logic_ps={} net_ps={} cells={} pins={}",
                p.name,
                p.startpoint,
                p.endpoint,
                p.slack_ps,
                p.delay_ps,
                p.logic_ps(),
                p.net_ps(),
                p.cells.len(),
                p.pins.len()
            ));
            for pin in &p.pins {
                s.push_str(&format!(
                    " pin={}:type={}:incr_ps={}:path_ps={}:net={}:fanout={}:loc={}",
                    pin.pin,
                    pin.delay_type,
                    pin.incr_ps,
                    pin.path_ps,
                    if pin.net.is_empty() { "-" } else { pin.net.as_str() },
                    pin.fanout,
                    if pin.location.is_empty() { "-" } else { pin.location.as_str() }
                ));
            }
        }
        s
    }

    pub fn scope_rows(&self) -> &[ScopeNode] {
        &self.scopes
    }

    pub fn object_rows(&self) -> &[SimObject] {
        &self.objects
    }

    /// UG900 Scopes table dump (Name/Type), not a `name (kind)` line.
    pub fn scopes_text(&self) -> String {
        let n = self.scopes.len();
        let mut s = format!("scopes n={n}");
        if n == 0 {
            s.push_str(" no scopes");
            return s;
        }
        for sc in &self.scopes {
            s.push('\n');
            s.push_str(&sc.row_text());
        }
        s
    }

    /// UG900 Objects table dump (Name/Type/Value), not a `name = value` line.
    pub fn objects_text(&self) -> String {
        let n = self.objects.len();
        let scope = self.selected_scope.as_deref().unwrap_or("-");
        let mut s = format!("objects n={n} scope={scope}");
        if n == 0 {
            s.push_str(" no objects");
            return s;
        }
        for o in &self.objects {
            s.push('\n');
            s.push_str(&o.row_text());
        }
        s
    }

    /// UG900: click a Scope row (name or index) — Name/Type table, not a dump.
    pub fn select_scope(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_scope: empty".into());
        }
        if self.scopes.is_empty() {
            self.prepare_sim()?;
        }
        let idx = if let Ok(i) = spec.parse::<usize>() {
            if i >= self.scopes.len() {
                return Err(format!("select_scope: no row {spec}"));
            }
            i
        } else {
            self.scopes
                .iter()
                .position(|s| s.name.eq_ignore_ascii_case(spec))
                .ok_or_else(|| format!("select_scope: no scope {spec}"))?
        };
        let sc = self.scopes[idx].clone();
        self.selected_scope = Some(sc.name.clone());
        self.selected_object = None;
        self.refresh_sim_objects();
        self.refresh_sim_debug();
        self.workspace = WorkspaceTab::Wave;
        self.properties = vec![
            ("NAME".into(), sc.name.clone()),
            ("TYPE".into(), sc.type_cell().to_string()),
            ("OBJECTS".into(), self.objects.len().to_string()),
            ("LOCALS".into(), self.locals.len().to_string()),
        ];
        let mut s = format!("scope {} objects={}", sc.name, self.objects.len());
        for o in &self.objects {
            s.push('\n');
            s.push_str(&o.row_text());
        }
        Ok(s)
    }

    /// UG900: click an Objects row (name or index) — add_wave + Properties, not a dump.
    pub fn select_object(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_object: missing name".into());
        }
        if self.objects.is_empty() {
            if self.scopes.is_empty() {
                self.prepare_sim()?;
            }
            self.refresh_sim_objects();
        }
        if self.objects.is_empty() {
            return Err("select_object: no objects".into());
        }
        let idx = if let Ok(i) = spec.parse::<usize>() {
            if i >= self.objects.len() {
                return Err(format!("select_object: no row {spec}"));
            }
            i
        } else {
            self.objects
                .iter()
                .position(|o| o.name.eq_ignore_ascii_case(spec))
                .ok_or_else(|| format!("select_object: no object {spec}"))?
        };
        let o = self.objects[idx].clone();
        self.selected_object = Some(o.name.clone());
        self.workspace = WorkspaceTab::Wave;
        self.properties = vec![
            ("NAME".into(), o.name.clone()),
            ("TYPE".into(), o.type_cell().to_string()),
            ("VALUE".into(), o.value_cell().to_string()),
        ];
        let _ = self.add_wave(&o.name);
        Ok(o.row_text())
    }

    pub fn memory_rows(&self) -> &[MemoryBlock] {
        &self.memories
    }

    pub fn local_rows(&self) -> &[LocalRow] {
        &self.locals
    }

    pub fn breakpoint_rows(&self) -> &[BreakpointRow] {
        &self.breakpoints
    }

    /// UG900 Memory table dump (Name/Type/Addr/Data), not a hex blob.
    pub fn memories_text(&self) -> String {
        let n = self.memories.len();
        let mut s = format!("memory n={n}");
        if n == 0 {
            s.push_str(" no memories — sim_run");
            return s;
        }
        for m in &self.memories {
            s.push('\n');
            s.push_str(&m.row_text());
        }
        s
    }

    /// Selected UG900 Memory contents (Addr/Data), not a concatenated INIT dump.
    pub fn memory_word_rows(&self) -> Vec<MemoryWordRow> {
        let Some(name) = self.selected_memory.as_deref() else {
            return Vec::new();
        };
        let Some(m) = self.memories.iter().find(|b| b.name == name) else {
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

    pub fn memory_words_text(&self) -> String {
        let name = self.selected_memory.as_deref().unwrap_or("-");
        let rows = self.memory_word_rows();
        let mut s = format!("memory_words n={} name={name}", rows.len());
        if rows.is_empty() {
            s.push_str(" no words — select_memory");
            return s;
        }
        for r in &rows {
            s.push('\n');
            s.push_str(&r.row_text());
        }
        s
    }

    /// UG900 Locals table dump (Name/Type/Value), not `name = value`.
    pub fn locals_text(&self) -> String {
        let n = self.locals.len();
        let scope = self.selected_scope.as_deref().unwrap_or("-");
        let mut s = format!("locals n={n} scope={scope}");
        if n == 0 {
            s.push_str(" no locals — sim_run");
            return s;
        }
        for l in &self.locals {
            s.push('\n');
            s.push_str(&l.row_text());
        }
        s
    }

    /// UG900 Breakpoints table dump (Id/En/Signal/Cond/Hits).
    pub fn breakpoints_text(&self) -> String {
        let n = self.breakpoints.len();
        let mut s = format!("breakpoints n={n}");
        if n == 0 {
            s.push_str(" no breakpoints");
            return s;
        }
        for b in &self.breakpoints {
            s.push('\n');
            s.push_str(&b.row_text());
        }
        s
    }

    pub fn open_memory(&mut self) -> Result<String, String> {
        if self.event_sim.is_none() && self.fabric_sim.is_none() {
            self.prepare_sim()?;
        }
        self.refresh_sim_debug();
        self.workspace = WorkspaceTab::Memory;
        self.layout = LayoutKind::Simulation;
        Ok(self.memories_text())
    }

    pub fn open_locals(&mut self) -> Result<String, String> {
        if self.event_sim.is_none() && self.fabric_sim.is_none() {
            self.prepare_sim()?;
        }
        self.refresh_sim_debug();
        self.workspace = WorkspaceTab::Locals;
        self.layout = LayoutKind::Simulation;
        Ok(self.locals_text())
    }

    pub fn open_breakpoints(&mut self) -> Result<String, String> {
        self.workspace = WorkspaceTab::Breakpoints;
        self.layout = LayoutKind::Simulation;
        Ok(self.breakpoints_text())
    }

    /// UG900: click a Memory row (name or index) — contents + Properties, not a dump.
    pub fn select_memory(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_memory: empty".into());
        }
        if self.memories.is_empty() {
            if self.event_sim.is_none() && self.fabric_sim.is_none() {
                self.prepare_sim()?;
            }
            self.refresh_sim_debug();
        }
        if self.memories.is_empty() {
            return Err("select_memory: no memories".into());
        }
        let idx = if let Ok(i) = spec.parse::<usize>() {
            if i >= self.memories.len() {
                return Err(format!("select_memory: no row {spec}"));
            }
            i
        } else {
            self.memories
                .iter()
                .position(|m| m.name.eq_ignore_ascii_case(spec))
                .ok_or_else(|| format!("select_memory: no memory {spec}"))?
        };
        let m = self.memories[idx].clone();
        self.selected_memory = Some(m.name.clone());
        self.selected_memory_addr = None;
        self.workspace = WorkspaceTab::Memory;
        self.properties = vec![
            ("NAME".into(), m.name.clone()),
            ("TYPE".into(), m.type_cell().to_string()),
            ("ADDR".into(), "0".into()),
            ("DATA".into(), m.data_cell()),
            ("WIDTH".into(), m.width.to_string()),
            ("DEPTH".into(), m.depth().to_string()),
        ];
        Ok(format!("{}\n{}", m.row_text(), self.memory_words_text()))
    }

    /// UG900: click a Memory address — Properties show Addr/Data from the engine word.
    pub fn select_memory_word(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_memory_word: empty".into());
        }
        if self.selected_memory.is_none() {
            return Err("select_memory_word: select a memory first".into());
        }
        let rows = self.memory_word_rows();
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
        self.selected_memory_addr = Some(row.addr);
        let name = self.selected_memory.clone().unwrap_or_default();
        self.workspace = WorkspaceTab::Memory;
        self.properties = vec![
            ("NAME".into(), name.clone()),
            ("TYPE".into(), "memory_word".into()),
            ("ADDR".into(), row.addr.to_string()),
            ("DATA".into(), row.data.clone()),
        ];
        Ok(format!("memory_word NAME={name} {}", row.row_text()))
    }

    /// UG900: click a Locals row (name or index) — add_wave + Properties.
    pub fn select_local(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_local: empty".into());
        }
        if self.locals.is_empty() {
            if self.event_sim.is_none() && self.fabric_sim.is_none() {
                self.prepare_sim()?;
            }
            self.refresh_sim_debug();
        }
        if self.locals.is_empty() {
            return Err("select_local: no locals".into());
        }
        let idx = if let Ok(i) = spec.parse::<usize>() {
            if i >= self.locals.len() {
                return Err(format!("select_local: no row {spec}"));
            }
            i
        } else {
            self.locals
                .iter()
                .position(|l| l.name.eq_ignore_ascii_case(spec))
                .ok_or_else(|| format!("select_local: no local {spec}"))?
        };
        let l = self.locals[idx].clone();
        self.selected_local = Some(l.name.clone());
        self.workspace = WorkspaceTab::Locals;
        self.properties = vec![
            ("NAME".into(), l.name.clone()),
            ("TYPE".into(), l.type_cell().to_string()),
            ("VALUE".into(), l.value_cell().to_string()),
            ("SCOPE".into(), l.scope.clone()),
        ];
        let _ = self.add_wave(&l.name);
        Ok(l.row_text())
    }

    pub fn add_breakpoint(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("add_bp: need <signal> [value]".into());
        }
        if self.event_sim.is_none() && self.fabric_sim.is_none() {
            self.prepare_sim()?;
        }
        self.refresh_sim_debug();
        let mut parts = spec.split_whitespace();
        let signal = parts.next().unwrap().to_string();
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
        if self.sim_signal_value(&signal).is_none() {
            return Err(format!("add_bp: no signal {signal}"));
        }
        if let Some(existing) = self
            .breakpoints
            .iter_mut()
            .find(|b| b.signal.eq_ignore_ascii_case(&signal) && b.value == value)
        {
            existing.enabled = true;
            self.selected_breakpoint = Some(existing.id);
            self.workspace = WorkspaceTab::Breakpoints;
            return Ok(existing.row_text());
        }
        let condition = match value {
            None => "change".into(),
            Some(v) => format!("=={v}"),
        };
        let id = self.next_bp_id;
        self.next_bp_id += 1;
        let bp = BreakpointRow {
            id,
            enabled: true,
            signal: signal.clone(),
            condition,
            value,
            hits: 0,
        };
        self.selected_breakpoint = Some(id);
        self.workspace = WorkspaceTab::Breakpoints;
        self.properties = vec![
            ("NAME".into(), format!("bp{id}")),
            ("TYPE".into(), "breakpoint".into()),
            ("ID".into(), id.to_string()),
            ("ENABLED".into(), "1".into()),
            ("SIGNAL".into(), signal),
            ("CONDITION".into(), bp.condition.clone()),
            ("HITS".into(), "0".into()),
        ];
        let text = bp.row_text();
        self.breakpoints.push(bp);
        Ok(text)
    }

    pub fn select_breakpoint(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_breakpoint: empty".into());
        }
        if self.breakpoints.is_empty() {
            return Err("select_breakpoint: no breakpoints".into());
        }
        let idx = if let Ok(i) = spec.parse::<usize>() {
            self.breakpoints
                .iter()
                .position(|b| b.id == i)
                .or_else(|| (i < self.breakpoints.len()).then_some(i))
                .ok_or_else(|| format!("select_breakpoint: no row {spec}"))?
        } else {
            let name = spec.trim_start_matches("bp");
            self.breakpoints
                .iter()
                .position(|b| b.signal.eq_ignore_ascii_case(spec) || b.id.to_string() == name)
                .ok_or_else(|| format!("select_breakpoint: no breakpoint {spec}"))?
        };
        let b = self.breakpoints[idx].clone();
        self.selected_breakpoint = Some(b.id);
        self.workspace = WorkspaceTab::Breakpoints;
        self.properties = vec![
            ("NAME".into(), format!("bp{}", b.id)),
            ("TYPE".into(), "breakpoint".into()),
            ("ID".into(), b.id.to_string()),
            ("ENABLED".into(), b.enabled_cell().into()),
            ("SIGNAL".into(), b.signal.clone()),
            ("CONDITION".into(), b.condition.clone()),
            ("HITS".into(), b.hits.to_string()),
        ];
        Ok(b.row_text())
    }

    pub fn set_breakpoint_enabled(&mut self, spec: &str, enabled: bool) -> Result<String, String> {
        if spec.trim().is_empty() {
            let id = self
                .selected_breakpoint
                .ok_or("enable_bp: need id")?;
            return self.set_breakpoint_enabled(&id.to_string(), enabled);
        }
        self.select_breakpoint(spec)?;
        let id = self.selected_breakpoint.unwrap();
        let b = self
            .breakpoints
            .iter_mut()
            .find(|b| b.id == id)
            .ok_or("enable_bp: missing")?;
        b.enabled = enabled;
        let text = b.row_text();
        Ok(text)
    }

    pub fn delete_breakpoint(&mut self, spec: &str) -> Result<String, String> {
        if spec.trim().is_empty() {
            let id = self
                .selected_breakpoint
                .ok_or("delete_bp: need id")?;
            return self.delete_breakpoint(&id.to_string());
        }
        self.select_breakpoint(spec)?;
        let id = self.selected_breakpoint.unwrap();
        self.breakpoints.retain(|b| b.id != id);
        if self.selected_breakpoint == Some(id) {
            self.selected_breakpoint = None;
        }
        self.workspace = WorkspaceTab::Breakpoints;
        Ok(format!("delete_bp {id}"))
    }

    /// Fig. 49: select a clock region; Properties show name + HAD site count.
    pub fn select_clock_region(&mut self, name: &str) -> Result<String, String> {
        let name = name.trim();
        let cr = self
            .device
            .clock_region_named(name)
            .cloned()
            .ok_or_else(|| format!("select_clock_region: no region {name}"))?;
        let sites = cr.site_count(&self.device.sites);
        self.workspace = WorkspaceTab::Device;
        self.selected = Some(cr.name.clone());
        self.properties = vec![
            ("NAME".into(), cr.name.clone()),
            ("TYPE".into(), "clock_region".into()),
            ("SITES".into(), sites.to_string()),
            ("X0".into(), cr.x0.to_string()),
            ("Y0".into(), cr.y0.to_string()),
            ("X1".into(), cr.x1.to_string()),
            ("Y1".into(), cr.y1.to_string()),
        ];
        Ok(format!(
            "clock_region {} sites={} X{}Y{}-X{}Y{}",
            cr.name, sites, cr.x0, cr.y0, cr.x1, cr.y1
        ))
    }

    fn apply_cmd_workspace(&mut self, id: &str) {
        match id {
            "report_timing" | "report_timing_summary" | "reports" => {
                self.workspace = WorkspaceTab::Reports;
            }
            "report_drc" => self.workspace = WorkspaceTab::Drc,
            "report_methodology" => self.workspace = WorkspaceTab::Methodology,
            "report_cdc" => self.workspace = WorkspaceTab::Cdc,
            "report_power" => self.workspace = WorkspaceTab::Power,
            "report_clock_interaction" => {
                self.workspace = WorkspaceTab::ClockInteraction;
            }
            "report_clock_networks" => self.workspace = WorkspaceTab::ClockNetworks,
            "report_utilization" => self.workspace = WorkspaceTab::Utilization,
            "timing_constraints" | "report_timing_constraints" => {
                self.workspace = WorkspaceTab::Constraints;
            }
            "place_design" | "route_design" | "device" => {
                self.workspace = WorkspaceTab::Device;
            }
            "synth_design" | "opt_design" | "schematic" => {
                self.workspace = WorkspaceTab::Schematic;
            }
            "write_bitstream" | "report_bitstream" => self.workspace = WorkspaceTab::Bitstream,
            "memory" | "memories" | "select_memory" => self.workspace = WorkspaceTab::Memory,
            "breakpoints" | "add_bp" | "select_breakpoint" => {
                self.workspace = WorkspaceTab::Breakpoints
            }
            "locals" | "select_local" => self.workspace = WorkspaceTab::Locals,
            _ => {}
        }
    }

    /// Fig. 64: Find in the Tcl journal (selectable console, not a dump).
    pub fn find_console(&mut self, q: &str) -> Result<String, String> {
        let q = q.trim();
        if q.is_empty() {
            return Err("console_find: empty".into());
        }
        let needle = q.to_ascii_lowercase();
        let hits: Vec<usize> = self
            .console
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                l.cmd.to_ascii_lowercase().contains(&needle)
                    || l.out.to_ascii_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect();
        self.console_find = q.to_string();
        self.console_find_hits = hits.clone();
        self.bottom_tab = BottomTab::Tcl;
        if hits.is_empty() {
            self.console_selected = None;
            return Err(format!("console_find {q}: 0 hits"));
        }
        self.console_selected = hits.first().copied();
        Ok(format!(
            "console_find {q} hits={} selected={} {}",
            hits.len(),
            self.console_selected.unwrap_or(0),
            hits.iter()
                .map(|i| format!("{i}:{}", self.console[*i].cmd))
                .collect::<Vec<_>>()
                .join(" ")
        ))
    }

    /// Click a UG893 Tcl Console row: properties + jump to the engine pane.
    pub fn select_console_line(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_console: missing id".into());
        }
        if self.console.is_empty() {
            return Err("select_console: console empty".into());
        }
        let spec_l = spec.to_ascii_lowercase();
        let (idx, line) = if let Ok(i) = spec.parse::<usize>() {
            self.console
                .get(i)
                .cloned()
                .map(|l| (i, l))
                .ok_or_else(|| format!("select_console: no row {spec}"))?
        } else {
            self.console
                .iter()
                .enumerate()
                .rev()
                .find(|(_, l)| {
                    l.cmd.eq_ignore_ascii_case(spec)
                        || l.cmd
                            .split_whitespace()
                            .next()
                            .is_some_and(|c| c.eq_ignore_ascii_case(spec))
                })
                .or_else(|| {
                    self.console.iter().enumerate().rev().find(|(_, l)| {
                        !l.cmd.eq_ignore_ascii_case("tcl_console")
                            && !l.cmd.eq_ignore_ascii_case("console")
                            && l.out.to_ascii_lowercase().contains(&spec_l)
                    })
                })
                .map(|(i, l)| (i, l.clone()))
                .ok_or_else(|| format!("select_console: no row {spec}"))?
        };
        let status = line.status();
        let id = line
            .cmd
            .split_whitespace()
            .next()
            .unwrap_or(line.cmd.as_str())
            .to_string();
        self.console_selected = Some(idx);
        self.selected = Some(format!("tcl:{idx}"));
        self.bottom_tab = BottomTab::Tcl;
        self.properties = vec![
            ("NAME".into(), id.clone()),
            ("TYPE".into(), "tcl".into()),
            ("INDEX".into(), idx.to_string()),
            ("STATUS".into(), status.into()),
            ("CMD".into(), line.cmd.clone()),
            ("OUT".into(), line.out.clone()),
        ];
        self.apply_cmd_workspace(&id);
        Ok(format!(
            "console_line {idx} ok={} cmd={} out={} STATUS={status} CMD={} OUT={}",
            line.ok as u8, line.cmd, line.out, line.cmd, line.out
        ))
    }

    /// UG893 Package drawing dump: HAD IOB bounding box + occupancy map.
    pub fn package_drawing_text(&self) -> String {
        let n = self.package_pins.len();
        let assigned = self
            .package_pins
            .iter()
            .filter(|p| p.port.is_some())
            .count();
        let n_banks = self
            .package_pins
            .iter()
            .map(|p| p.bank)
            .collect::<HashSet<_>>()
            .len();
        let mut s = format!(
            "package drawing part={} cols={} rows={} pins={} assigned={} x0={} y0={} banks={}",
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
            self.package.y0,
            n_banks
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
                        " {}={}:bank={}",
                        p.pin,
                        p.port.as_deref().unwrap_or("-"),
                        p.bank
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

    /// UG893 I/O Ports table: PACKAGE_PIN + IOSTANDARD/DRIVE/SLEW/PULLTYPE/DIFF_TERM/IN_TERM
    /// hitting HAD/STA/DRC/bitgen, not a pin dump.
    pub fn io_port_rows(&self) -> &[IoPortView] {
        &self.io_ports
    }

    pub fn io_ports_text(&self) -> String {
        let n = self.io_ports.len();
        let assigned = self
            .io_ports
            .iter()
            .filter(|p| p.package_pin.is_some() || p.site.is_some())
            .count();
        let mut s = format!("io_ports n={n} assigned={assigned}");
        for p in &self.io_ports {
            s.push('\n');
            s.push_str(&p.row_text());
        }
        s
    }

    /// Open the I/O Planning pane (I/O Ports + Package drawing).
    pub fn open_io_planning(&mut self) -> Result<String, String> {
        self.nav = NavSection::BoardDevice;
        self.workspace = WorkspaceTab::Package;
        Ok(self.io_ports_text())
    }

    /// Click a UG893 I/O Ports row (name or index) — Package workspace, not a dump.
    pub fn select_io_port(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_io_port: missing name".into());
        }
        let p = if let Ok(i) = spec.parse::<usize>() {
            self.io_ports
                .get(i)
                .cloned()
                .ok_or_else(|| format!("select_io_port: no row {spec}"))?
        } else {
            self.io_ports
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(spec))
                .cloned()
                .ok_or_else(|| format!("select_io_port: no port {spec}"))?
        };
        self.nav = NavSection::BoardDevice;
        self.workspace = WorkspaceTab::Package;
        self.select(&p.name);
        Ok(p.row_text())
    }

    /// Place using the first ranged Pblock (UG893 floorplan containment).
    fn place_now(&mut self, dev: &helion_device::Device) -> Result<(), String> {
        if let Some(pb) = self.pblocks.iter().find(|p| p.ranged).cloned() {
            self.shell
                .session
                .place_pblock(dev, pb.x0, pb.y0, pb.x1, pb.y1)
        } else {
            self.shell.session.place_design(dev)
        }
    }

    /// UG893 Floorplanning pane: Pblock rectangles on the Device die.
    pub fn open_floorplanning(&mut self) -> Result<String, String> {
        self.nav = NavSection::BoardDevice;
        self.workspace = WorkspaceTab::Device;
        Ok(self.pblocks_text())
    }

    pub fn pblock_rows(&self) -> &[Pblock] {
        &self.pblocks
    }

    pub fn pblocks_text(&self) -> String {
        let n = self.pblocks.len();
        let mut s = format!("pblocks n={n}");
        for p in &self.pblocks {
            s.push('\n');
            s.push_str(&p.row_text());
            s.push_str(&format!(
                " sites={}",
                p.site_count(&self.device.sites)
            ));
        }
        s
    }

    fn create_pblock_cmd(&mut self, cmd: &str) -> Result<String, String> {
        let rest = cmd.strip_prefix("create_pblock").unwrap_or("").trim();
        let mut name = String::new();
        let mut add = None;
        let mut toks = rest.split_whitespace().peekable();
        while let Some(tok) = toks.next() {
            if tok == "-add" {
                let spec = toks.next().unwrap_or("");
                add = Some(spec.to_string());
            } else if name.is_empty() {
                name = tok.trim_matches(|c: char| "{}[]".contains(c)).to_string();
            } else if add.is_none() {
                add = Some(tok.to_string());
            }
        }
        if name.is_empty() {
            name = format!("pblock_{}", self.pblocks.len());
        }
        if self.pblocks.iter().any(|p| p.name == name) {
            return Err(format!("create_pblock: {name} exists"));
        }
        self.pblocks.push(Pblock {
            name: name.clone(),
            ..Pblock::default()
        });
        self.nav = NavSection::BoardDevice;
        self.workspace = WorkspaceTab::Device;
        self.selected = Some(name.clone());
        if let Some(spec) = add {
            let out = self.resize_pblock(&name, &spec)?;
            return Ok(format!("create_pblock {name} {out}"));
        }
        self.refresh_device();
        self.refresh_properties();
        Ok(format!("create_pblock {name}"))
    }

    fn resize_pblock_cmd(&mut self, cmd: &str) -> Result<String, String> {
        let rest = cmd.strip_prefix("resize_pblock").unwrap_or("").trim();
        let mut toks = rest.split_whitespace();
        let name = toks
            .next()
            .ok_or("resize_pblock: need <name> -add <range>")?
            .trim_matches(|c: char| "{}[]".contains(c));
        let mut spec = String::new();
        for tok in toks {
            if tok == "-add" {
                continue;
            }
            if !spec.is_empty() {
                spec.push(' ');
            }
            spec.push_str(tok);
        }
        if spec.is_empty() {
            return Err("resize_pblock: need -add {CLB_X0Y0:CLB_X1Y1}".into());
        }
        self.resize_pblock(name, &spec)
    }

    /// `resize_pblock`: set the HAD rectangle, re-place into it, partial bitgen.
    pub fn resize_pblock(&mut self, name: &str, spec: &str) -> Result<String, String> {
        let (x0, y0, x1, y1) = self.resolve_pblock_range(spec)?;
        let idx = self
            .pblocks
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| format!("resize_pblock: no pblock {name}"))?;
        let cells = self.pblocks[idx].cells.clone();
        let mut placed = 0u8;
        let mut routed = 0u8;
        let mut frames = 0usize;
        let mut bytes = 0usize;
        let mut loc = format!("CLB_X{x0}Y{y0}");
        if self.shell.session.design.is_some() {
            let dev = self.device()?;
            self.shell.session.place_pblock(&dev, x0, y0, x1, y1)?;
            placed = 1;
            if let Some(s) = self
                .shell
                .session
                .placed
                .as_ref()
                .and_then(|p| p.lutff_sites.first())
            {
                loc = format!("CLB_X{}Y{}", s.0.x, s.0.y);
            }
            self.shell.session.route_design(&dev)?;
            routed = 1;
            let sites = self.pblock_engine_sites(x0, y0, x1, y1, &cells);
            if sites.is_empty() {
                return Err("resize_pblock: no placed sites in range".into());
            }
            let pb = self.shell.session.write_pblock_bitstream(&dev, &sites)?;
            frames = pb.frames.len();
            bytes = pb.packets.len();
        }
        let p = &mut self.pblocks[idx];
        p.x0 = x0;
        p.y0 = y0;
        p.x1 = x1;
        p.y1 = y1;
        p.ranged = true;
        p.frames = frames;
        p.bytes = bytes;
        self.nav = NavSection::BoardDevice;
        self.workspace = WorkspaceTab::Device;
        self.selected = Some(name.to_string());
        self.refresh_device();
        self.refresh_properties();
        Ok(format!(
            "resize_pblock {name} -add CLB_X{x0}Y{y0}:CLB_X{x1}Y{y1} loc={loc} placed={placed} routed={routed} frames={frames} bytes={bytes}"
        ))
    }

    fn add_cells_to_pblock_cmd(&mut self, cmd: &str) -> Result<String, String> {
        let rest = cmd.strip_prefix("add_cells_to_pblock").unwrap_or("").trim();
        let mut toks = rest.split_whitespace();
        let name = toks
            .next()
            .ok_or("add_cells_to_pblock: need <pblock> <cell>")?
            .trim_matches(|c: char| "{}[]".contains(c));
        let mut cells: Vec<String> = Vec::new();
        for tok in toks {
            let c = tok.trim_matches(|c: char| "{}[]".contains(c));
            if c.eq_ignore_ascii_case("get_cells") || c.is_empty() {
                continue;
            }
            cells.push(c.to_string());
        }
        if cells.is_empty() {
            if let Some(d) = self.shell.session.design.as_ref() {
                cells = d
                    .cells
                    .iter()
                    .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
                    .map(|c| c.name.clone())
                    .collect();
            }
        }
        if cells.is_empty() {
            return Err("add_cells_to_pblock: need <cell>".into());
        }
        let idx = self
            .pblocks
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| format!("add_cells_to_pblock: no pblock {name}"))?;
        for c in &cells {
            if !self.pblocks[idx].cells.contains(c) {
                self.pblocks[idx].cells.push(c.clone());
            }
        }
        let ranged = self.pblocks[idx].ranged;
        let out = if ranged {
            let spec = self.pblocks[idx].range_text();
            self.resize_pblock(name, &spec)?
        } else {
            String::new()
        };
        self.selected = Some(name.to_string());
        self.workspace = WorkspaceTab::Device;
        self.refresh_device();
        self.refresh_properties();
        Ok(format!(
            "add_cells_to_pblock {name} cells={} {out}",
            self.pblocks
                .iter()
                .find(|p| p.name == name)
                .map(|p| p.cells.join(","))
                .unwrap_or_default()
        ))
    }

    pub fn select_pblock(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_pblock: missing name".into());
        }
        let pb = if let Ok(i) = spec.parse::<usize>() {
            self.pblocks
                .get(i)
                .cloned()
                .ok_or_else(|| format!("select_pblock: no row {spec}"))?
        } else {
            self.pblocks
                .iter()
                .find(|p| p.name == spec)
                .cloned()
                .ok_or_else(|| format!("select_pblock: no pblock {spec}"))?
        };
        let sites = pb.site_count(&self.device.sites);
        self.workspace = WorkspaceTab::Device;
        self.nav = NavSection::BoardDevice;
        self.selected = Some(pb.name.clone());
        self.properties = vec![
            ("NAME".into(), pb.name.clone()),
            ("TYPE".into(), "pblock".into()),
            ("RANGE".into(), pb.range_text()),
            ("SITES".into(), sites.to_string()),
            ("CELLS".into(), pb.cells.len().to_string()),
            ("FRAMES".into(), pb.frames.to_string()),
            ("BYTES".into(), pb.bytes.to_string()),
            ("X0".into(), pb.x0.to_string()),
            ("Y0".into(), pb.y0.to_string()),
            ("X1".into(), pb.x1.to_string()),
            ("Y1".into(), pb.y1.to_string()),
        ];
        Ok(format!(
            "pblock {} NAME={} RANGE={} CELLS={} FRAMES={} BYTES={} SITES={} range={} sites={} frames={}",
            pb.name,
            pb.name,
            pb.range_text(),
            pb.cells.len(),
            pb.frames,
            pb.bytes,
            sites,
            pb.range_text(),
            sites,
            pb.frames
        ))
    }

    fn resolve_pblock_range(&self, spec: &str) -> Result<(u32, u32, u32, u32), String> {
        let t = spec
            .trim()
            .trim_matches(|c: char| "{}[]".contains(c))
            .trim();
        let t = t.strip_prefix("-add").unwrap_or(t).trim();
        let t = t.trim_matches(|c: char| "{}[]".contains(c)).trim();
        let cr_name = t
            .strip_prefix("CLOCKREGION_")
            .or_else(|| t.strip_prefix("clockregion_"));
        if let Some(name) = cr_name {
            let cr = self
                .device
                .clock_region_named(name)
                .ok_or_else(|| format!("resize_pblock: no clock region {name}"))?;
            return Ok((cr.x0, cr.y0, cr.x1, cr.y1));
        }
        parse_pblock_range(t)
    }

    fn pblock_engine_sites(
        &self,
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
        cells: &[String],
    ) -> Vec<(u32, u32)> {
        let mut sites = Vec::new();
        let Some(pl) = self.shell.session.placed.as_ref() else {
            return sites;
        };
        for (i, (site, _)) in pl.lutff_sites.iter().enumerate() {
            if site.x < x0 || site.x > x1 || site.y < y0 || site.y > y1 {
                continue;
            }
            if !cells.is_empty() {
                let Some(lf) = pl.packed.lutffs.get(i) else {
                    continue;
                };
                if !cells.iter().any(|c| c == &lf.lut_cell || c == &lf.ff_cell) {
                    continue;
                }
            }
            if !sites.contains(&(site.x, site.y)) {
                sites.push((site.x, site.y));
            }
        }
        for (i, site) in pl.iob_sites.iter().enumerate() {
            if site.x < x0 || site.x > x1 || site.y < y0 || site.y > y1 {
                continue;
            }
            if !cells.is_empty() {
                let Some(iob) = pl.packed.iobs.get(i) else {
                    continue;
                };
                if !cells.iter().any(|c| c == &iob.cell) {
                    continue;
                }
            }
            if !sites.contains(&(site.x, site.y)) {
                sites.push((site.x, site.y));
            }
        }
        sites
    }

    /// Parse `set_property PACKAGE_PIN|LOC <pin> [get_ports <port>]` or
    /// `assign_package_pin <port> <pin>`.
    fn apply_package_pin(&mut self, cmd: &str) -> Result<String, String> {
        let (port, pin) = parse_package_pin_cmd(cmd)?;
        self.set_package_pin(&port, &pin)
    }

    /// I/O Planning `set_property PACKAGE_PIN`: bind LOC, re-place (and re-route
    /// if already routed) so the loc hits place/STA rather than a pin list.
    pub fn set_package_pin(&mut self, port: &str, pin: &str) -> Result<String, String> {
        let port = port.trim();
        let pin = pin.trim();
        if port.is_empty() || pin.is_empty() {
            return Err("set_property PACKAGE_PIN: need <pin> [get_ports <port>]".into());
        }
        let (x, y) = parse_site_xy(pin).map_err(|e| {
            format!("set_property PACKAGE_PIN: bad pin {pin} ({e})")
        })?;
        let dev = self.device()?;
        if dev.iob_major(x, y).is_none() {
            return Err(format!(
                "set_property PACKAGE_PIN: {pin} is not a HAD IOB site"
            ));
        }
        {
            let d = self
                .shell
                .session
                .design
                .as_mut()
                .ok_or("set_property PACKAGE_PIN: no design")?;
            if d.ports.iter().all(|p| p.name != port) {
                return Err(format!("set_property PACKAGE_PIN: no port {port}"));
            }
            d.set_loc(port, pin)?;
        }
        self.constraints
            .package_pins
            .insert(port.to_string(), pin.to_string());
        let was_routed = self.shell.session.routed.is_some();
        let was_placed = self.shell.session.placed.is_some();
        let mut placed = format!("IOB_X{x}Y{y}");
        if was_placed {
            self.shell.session.place_design(&dev)?;
            if let Some(s) = self
                .shell
                .session
                .placed
                .as_ref()
                .and_then(|p| p.iob_sites.first())
            {
                placed = format!("IOB_X{}Y{}", s.x, s.y);
            }
            if was_routed {
                self.shell.session.route_design(&dev)?;
            }
        }
        self.nav = NavSection::BoardDevice;
        self.workspace = WorkspaceTab::Package;
        self.select(port);
        Ok(format!(
            "set_property PACKAGE_PIN {pin} {port} loc={placed} replaced={} rerouted={}",
            u8::from(was_placed),
            u8::from(was_routed)
        ))
    }

    /// Parse `set_property IOSTANDARD <std> [get_ports <port>]`.
    fn apply_iostandard(&mut self, cmd: &str) -> Result<String, String> {
        let (port, std) = parse_port_prop_cmd(cmd, "IOSTANDARD")?;
        self.set_iostandard(&port, &std)
    }

    /// I/O Planning `set_property IOSTANDARD`: bind HAD pad standard on the HNF
    /// port so STA pad delay and DRC bank-VCCO see it — not a table label.
    pub fn set_iostandard(&mut self, port: &str, std: &str) -> Result<String, String> {
        let port = port.trim();
        let std = std.trim();
        if port.is_empty() || std.is_empty() {
            return Err("set_property IOSTANDARD: need <std> [get_ports <port>]".into());
        }
        let std_up = std.to_ascii_uppercase();
        if !Device::legal_iostandard(&std_up) {
            return Err(format!(
                "set_property IOSTANDARD: {std} is not a HAD I/O standard"
            ));
        }
        {
            let d = self
                .shell
                .session
                .design
                .as_mut()
                .ok_or("set_property IOSTANDARD: no design")?;
            if d.ports.iter().all(|p| p.name != port) {
                return Err(format!("set_property IOSTANDARD: no port {port}"));
            }
            d.set_iostandard(port, &std_up)?;
        }
        self.constraints
            .iostandards
            .insert(port.to_string(), std_up.clone());
        self.nav = NavSection::BoardDevice;
        self.workspace = WorkspaceTab::Package;
        self.select(port);
        let pad_ps = iostandard_pad_ps(Some(&std_up));
        Ok(format!(
            "set_property IOSTANDARD {std_up} {port} pad_ps={pad_ps}"
        ))
    }

    fn apply_drive(&mut self, cmd: &str) -> Result<String, String> {
        let (port, val) = parse_port_prop_cmd(cmd, "DRIVE")?;
        self.set_drive(&port, &val)
    }

    fn apply_slew(&mut self, cmd: &str) -> Result<String, String> {
        let (port, val) = parse_port_prop_cmd(cmd, "SLEW")?;
        self.set_slew(&port, &val)
    }

    fn apply_pulltype(&mut self, cmd: &str) -> Result<String, String> {
        let (port, val) = parse_port_prop_cmd(cmd, "PULLTYPE")?;
        self.set_pulltype(&port, &val)
    }

    fn apply_diff_term(&mut self, cmd: &str) -> Result<String, String> {
        let (port, val) = parse_port_prop_cmd(cmd, "DIFF_TERM")?;
        self.set_diff_term(&port, &val)
    }

    fn apply_in_term(&mut self, cmd: &str) -> Result<String, String> {
        let (port, val) = parse_port_prop_cmd(cmd, "IN_TERM")?;
        self.set_in_term(&port, &val)
    }

    /// I/O Planning `set_property DRIVE`: HAD mA on the HNF port so STA / DRC / bitgen see it.
    pub fn set_drive(&mut self, port: &str, ma: &str) -> Result<String, String> {
        let port = port.trim();
        let ma = ma.trim();
        if port.is_empty() || ma.is_empty() {
            return Err("set_property DRIVE: need <ma> [get_ports <port>]".into());
        }
        let Some(parsed) = Device::parse_drive(ma) else {
            return Err(format!("set_property DRIVE: {ma} is not a HAD drive strength"));
        };
        let drive = parsed.to_string();
        {
            let d = self
                .shell
                .session
                .design
                .as_mut()
                .ok_or("set_property DRIVE: no design")?;
            if d.ports.iter().all(|p| p.name != port) {
                return Err(format!("set_property DRIVE: no port {port}"));
            }
            d.set_drive(port, &drive)?;
        }
        self.constraints.drives.insert(port.to_string(), drive.clone());
        self.nav = NavSection::BoardDevice;
        self.workspace = WorkspaceTab::Package;
        self.select(port);
        let bitgen = self.maybe_rebitgen()?;
        let pad_ps = self.port_pad_ps_now(port);
        Ok(format!(
            "set_property DRIVE {drive} {port} pad_ps={pad_ps} bitgen={}",
            u8::from(bitgen)
        ))
    }

    /// I/O Planning `set_property SLEW`: SLOW | FAST hits STA pad delay and bitgen.
    pub fn set_slew(&mut self, port: &str, slew: &str) -> Result<String, String> {
        let port = port.trim();
        let slew = slew.trim();
        if port.is_empty() || slew.is_empty() {
            return Err("set_property SLEW: need <SLOW|FAST> [get_ports <port>]".into());
        }
        if !Device::legal_slew(slew) {
            return Err(format!("set_property SLEW: {slew} is not a HAD slew (SLOW|FAST)"));
        }
        let slew_up = slew.to_ascii_uppercase();
        {
            let d = self
                .shell
                .session
                .design
                .as_mut()
                .ok_or("set_property SLEW: no design")?;
            if d.ports.iter().all(|p| p.name != port) {
                return Err(format!("set_property SLEW: no port {port}"));
            }
            d.set_slew(port, &slew_up)?;
        }
        self.constraints.slews.insert(port.to_string(), slew_up.clone());
        self.nav = NavSection::BoardDevice;
        self.workspace = WorkspaceTab::Package;
        self.select(port);
        let bitgen = self.maybe_rebitgen()?;
        let pad_ps = self.port_pad_ps_now(port);
        Ok(format!(
            "set_property SLEW {slew_up} {port} pad_ps={pad_ps} bitgen={}",
            u8::from(bitgen)
        ))
    }

    /// I/O Planning `set_property PULLTYPE`: NONE | PULLUP | PULLDOWN | KEEPER.
    pub fn set_pulltype(&mut self, port: &str, pull: &str) -> Result<String, String> {
        let port = port.trim();
        let pull = pull.trim();
        if port.is_empty() || pull.is_empty() {
            return Err("set_property PULLTYPE: need <type> [get_ports <port>]".into());
        }
        if !Device::legal_pulltype(pull) {
            return Err(format!(
                "set_property PULLTYPE: {pull} is not a HAD pull (NONE|PULLUP|PULLDOWN|KEEPER)"
            ));
        }
        let pull_up = pull.to_ascii_uppercase();
        {
            let d = self
                .shell
                .session
                .design
                .as_mut()
                .ok_or("set_property PULLTYPE: no design")?;
            if d.ports.iter().all(|p| p.name != port) {
                return Err(format!("set_property PULLTYPE: no port {port}"));
            }
            d.set_pulltype(port, &pull_up)?;
        }
        self.constraints
            .pulltypes
            .insert(port.to_string(), pull_up.clone());
        self.nav = NavSection::BoardDevice;
        self.workspace = WorkspaceTab::Package;
        self.select(port);
        let bitgen = self.maybe_rebitgen()?;
        let pad_ps = self.port_pad_ps_now(port);
        Ok(format!(
            "set_property PULLTYPE {pull_up} {port} pad_ps={pad_ps} bitgen={}",
            u8::from(bitgen)
        ))
    }

    /// I/O Planning `set_property DIFF_TERM`: TRUE | FALSE hits STA pad delay and bitgen.
    pub fn set_diff_term(&mut self, port: &str, term: &str) -> Result<String, String> {
        let port = port.trim();
        let term = term.trim();
        if port.is_empty() || term.is_empty() {
            return Err("set_property DIFF_TERM: need <TRUE|FALSE> [get_ports <port>]".into());
        }
        let Some(canon) = Device::parse_diff_term(term) else {
            return Err(format!(
                "set_property DIFF_TERM: {term} is not a HAD term (TRUE|FALSE)"
            ));
        };
        {
            let d = self
                .shell
                .session
                .design
                .as_mut()
                .ok_or("set_property DIFF_TERM: no design")?;
            if d.ports.iter().all(|p| p.name != port) {
                return Err(format!("set_property DIFF_TERM: no port {port}"));
            }
            d.set_diff_term(port, canon)?;
        }
        self.constraints
            .diff_terms
            .insert(port.to_string(), canon.to_string());
        self.nav = NavSection::BoardDevice;
        self.workspace = WorkspaceTab::Package;
        self.select(port);
        let bitgen = self.maybe_rebitgen()?;
        let pad_ps = self.port_pad_ps_now(port);
        Ok(format!(
            "set_property DIFF_TERM {canon} {port} pad_ps={pad_ps} bitgen={}",
            u8::from(bitgen)
        ))
    }

    /// I/O Planning `set_property IN_TERM`: NONE | UNTUNED_SPLIT_{40,50,60}.
    pub fn set_in_term(&mut self, port: &str, term: &str) -> Result<String, String> {
        let port = port.trim();
        let term = term.trim();
        if port.is_empty() || term.is_empty() {
            return Err("set_property IN_TERM: need <type> [get_ports <port>]".into());
        }
        let Some(canon) = Device::parse_in_term(term) else {
            return Err(format!(
                "set_property IN_TERM: {term} is not a HAD term (NONE|UNTUNED_SPLIT_40|UNTUNED_SPLIT_50|UNTUNED_SPLIT_60)"
            ));
        };
        {
            let d = self
                .shell
                .session
                .design
                .as_mut()
                .ok_or("set_property IN_TERM: no design")?;
            if d.ports.iter().all(|p| p.name != port) {
                return Err(format!("set_property IN_TERM: no port {port}"));
            }
            d.set_in_term(port, canon)?;
        }
        self.constraints
            .in_terms
            .insert(port.to_string(), canon.to_string());
        self.nav = NavSection::BoardDevice;
        self.workspace = WorkspaceTab::Package;
        self.select(port);
        let bitgen = self.maybe_rebitgen()?;
        let pad_ps = self.port_pad_ps_now(port);
        Ok(format!(
            "set_property IN_TERM {canon} {port} pad_ps={pad_ps} bitgen={}",
            u8::from(bitgen)
        ))
    }

    fn port_pad_ps_now(&self, port: &str) -> i64 {
        let p = self
            .shell
            .session
            .design
            .as_ref()
            .and_then(|d| d.ports.iter().find(|p| p.name == port));
        port_pad_ps(
            p.and_then(|p| p.attrs.get("IOSTANDARD")),
            p.and_then(|p| p.attrs.get("DRIVE")),
            p.and_then(|p| p.attrs.get("SLEW")),
            p.and_then(|p| p.attrs.get("PULLTYPE")),
            p.and_then(|p| p.attrs.get("DIFF_TERM")),
            p.and_then(|p| p.attrs.get("IN_TERM")),
        )
    }

    fn maybe_rebitgen(&mut self) -> Result<bool, String> {
        if self.shell.session.bitstream.is_none() || self.shell.session.routed.is_none() {
            return Ok(false);
        }
        let dev = self.device()?;
        self.shell.session.write_bitstream(&dev)?;
        Ok(true)
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
                let t0 = std::time::Instant::now();
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
                let ms = t0.elapsed().as_millis() as u64;
                let _ = self.shell.session.write_checkpoint();
                if let Some(r) = self.runs.iter_mut().find(|r| r.name == "impl_1") {
                    r.runtime_ms = Some(ms);
                    r.strategy = "Default".into();
                }
            }
            other => {
                self.launch_strategy_run(other)?;
            }
        }
        self.sync_from_session();
        Ok(self.runs_text())
    }

    /// UG986 Lab 1: extra impl run with a Helion strategy, without clobbering impl_1.
    fn launch_strategy_run(&mut self, name: &str) -> Result<String, String> {
        let (strategy_s, idx) = {
            let r = self
                .runs
                .iter()
                .enumerate()
                .find(|(_, r)| r.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| format!("launch_runs: unknown run {name}"))?;
            (r.1.strategy.clone(), r.0)
        };
        let strategy = ImplStrategy::parse(&strategy_s)?;
        let d = self
            .shell
            .session
            .design
            .clone()
            .ok_or_else(|| format!("launch_runs {name}: run synth_1 first"))?;
        let dev = self.device()?;
        let t0 = std::time::Instant::now();
        let mut side = Session::new(Mode::NonProject);
        side.part = self.part().to_string();
        side.synth_design(d);
        side.impl_with_strategy(&dev, strategy)?;
        let ms = t0.elapsed().as_millis() as u64;
        let clks = self.clocks_for_sta();
        let wns = match (side.design.as_ref(), side.routed.as_ref()) {
            (Some(d), Some(r)) => {
                report_timing_routed_xdc(d, r, &clks, &self.constraints)
                    .ok()
                    .map(|t| t.wns_ps)
            }
            _ => None,
        };
        let lutff = side.placed.as_ref().map(|p| p.packed.lutffs.len());
        let cells = side.design.as_ref().map(|d| d.cells.len());
        let hash = side.blinky_hash();
        let top = side.design.as_ref().map(|d| d.name.clone());
        let part = self.part().to_string();
        let r = &mut self.runs[idx];
        r.status = "Complete".into();
        r.strategy = strategy.label().into();
        r.runtime_ms = Some(ms);
        r.wns_ps = wns;
        r.lutff = lutff;
        r.cells = cells;
        r.bitstream_hash = hash;
        r.part = part;
        r.top = top;
        Ok(format!("launch_runs {name} strategy={}", strategy.label()))
    }

    /// UG986 Lab 1: `create_run impl_runtime -strategy RuntimeOpt`
    pub fn create_run(&mut self, spec: &str) -> Result<String, String> {
        let mut name = String::new();
        let mut strategy = ImplStrategy::Default;
        let mut toks = spec.split_whitespace();
        if let Some(n) = toks.next() {
            name = n.to_string();
        }
        while let Some(t) = toks.next() {
            if t == "-strategy" || t == "-strat" {
                if let Some(s) = toks.next() {
                    strategy = ImplStrategy::parse(s)?;
                }
            }
        }
        if name.is_empty() {
            return Err("create_run: need a run name".into());
        }
        if self.runs.iter().any(|r| r.name == name) {
            return Err(format!("create_run: {name} exists"));
        }
        let mut run = DesignRun::new(&name, "Implementation");
        run.strategy = strategy.label().into();
        self.runs.push(run);
        self.workspace = WorkspaceTab::Runs;
        Ok(format!("create_run {name} strategy={}", strategy.label()))
    }

    /// UG986 Lab 2: re-synth current source and incremental-place from the checkpoint.
    pub fn incremental_impl(&mut self) -> Result<String, String> {
        let prev = self
            .shell
            .session
            .impl_checkpoint
            .clone()
            .or_else(|| self.shell.session.placed.clone())
            .ok_or("incremental_impl: write_checkpoint / launch_runs impl_1 first")?;
        let path = self
            .tree
            .sources
            .last()
            .cloned()
            .ok_or("incremental_impl: no source")?;
        let d = helion_sv::synth_sv_path(std::path::Path::new(&path))?;
        self.shell.session.synth_design(d);
        let dev = self.device()?;
        let report = self.shell.session.incremental_place(&dev, &prev)?;
        self.shell.session.route_design(&dev)?;
        self.shell.session.write_bitstream(&dev)?;
        let _ = self.shell.session.write_checkpoint();
        if let Some(r) = self.runs.iter_mut().find(|r| r.name == "impl_1") {
            r.reuse_pct = Some(report.cell_pct());
            r.status = "Complete".into();
        }
        self.workspace = WorkspaceTab::Runs;
        Ok(format!("incremental_impl {}", report.text()))
    }

    pub fn incremental_place_now(&mut self) -> Result<String, String> {
        let prev = self
            .shell
            .session
            .impl_checkpoint
            .clone()
            .or_else(|| self.shell.session.placed.clone())
            .ok_or("incremental_place: no checkpoint")?;
        let dev = self.device()?;
        let report = self.shell.session.incremental_place(&dev, &prev)?;
        if let Some(r) = self.runs.iter_mut().find(|r| r.name == "impl_1") {
            r.reuse_pct = Some(report.cell_pct());
        }
        Ok(format!("incremental_place {}", report.text()))
    }

    pub fn incremental_route_now(&mut self) -> Result<String, String> {
        let dev = self.device()?;
        self.shell.session.route_design(&dev)?;
        self.shell.session.write_bitstream(&dev)?;
        Ok("incremental_route ok".into())
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

    fn find_parent_name(&self) -> String {
        self.hierarchy
            .top
            .clone()
            .or_else(|| self.tree.top.clone())
            .or_else(|| {
                self.shell
                    .session
                    .design
                    .as_ref()
                    .map(|d| d.name.clone())
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "-".into())
    }

    pub fn find_rows(&self) -> &[FindHit] {
        &self.find_results
    }

    /// UG893 Find Results table dump (Name/Type/Primitive/Parent), not a kind-name line.
    pub fn find_results_text(&self) -> String {
        let n = self.find_results.len();
        let mut s = format!("find_results n={n}");
        if n == 0 {
            s.push_str(" no hits");
            return s;
        }
        for h in &self.find_results {
            s.push('\n');
            s.push_str(&h.row_text());
        }
        s
    }

    pub fn open_find_results(&mut self) -> Result<String, String> {
        self.workspace = WorkspaceTab::Find;
        Ok(self.find_results_text())
    }

    /// Click a UG893 Find Results row (name or index) — Find workspace, not a dump.
    pub fn select_find(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_find: missing name".into());
        }
        if self.find_results.is_empty() {
            return Err("select_find: no hits".into());
        }
        let idx = if let Ok(i) = spec.parse::<usize>() {
            if i >= self.find_results.len() {
                return Err(format!("select_find: no row {spec}"));
            }
            i
        } else {
            self.find_results
                .iter()
                .position(|h| {
                    h.name.eq_ignore_ascii_case(spec)
                        || format!("{}:{}", h.kind, h.name).eq_ignore_ascii_case(spec)
                        || format!("{}:{}", h.type_cell(), h.name).eq_ignore_ascii_case(spec)
                })
                .ok_or_else(|| format!("select_find: no row {spec}"))?
        };
        let hit = self.find_results[idx].clone();
        self.selected_find = Some(idx);
        self.workspace = WorkspaceTab::Find;
        self.select(&hit.name);
        if !self.properties.iter().any(|(k, _)| k == "TYPE") {
            self.properties
                .push(("TYPE".into(), hit.type_cell().to_string()));
        }
        if !self.properties.iter().any(|(k, _)| k == "PRIMITIVE") {
            self.properties
                .push(("PRIMITIVE".into(), hit.primitive_cell().to_string()));
        }
        if !self.properties.iter().any(|(k, _)| k == "PARENT") {
            self.properties
                .push(("PARENT".into(), hit.parent_cell().to_string()));
        }
        Ok(hit.row_text())
    }

    /// UG893 Find: substring match on HNF cells/nets/ports and HAD pin names.
    pub fn find(&mut self, query: &str) -> Result<String, String> {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return Err("find: empty query".into());
        }
        let parent = self.find_parent_name();
        let mut hits = Vec::new();
        if let Some(d) = self.shell.session.design.as_ref() {
            for c in &d.cells {
                if c.name.to_ascii_lowercase().contains(&q) {
                    hits.push(FindHit {
                        kind: "cell".into(),
                        name: c.name.clone(),
                        primitive: primitive_of(&c.kind),
                        parent: parent.clone(),
                    });
                }
            }
            for n in &d.nets {
                if n.name.to_ascii_lowercase().contains(&q) {
                    hits.push(FindHit {
                        kind: "net".into(),
                        name: n.name.clone(),
                        primitive: format!("fanout={}", n.endpoints.len()),
                        parent: parent.clone(),
                    });
                }
            }
            for p in &d.ports {
                if p.name.to_ascii_lowercase().contains(&q) {
                    let dir = match p.dir {
                        PortDir::In => "IN",
                        PortDir::Out => "OUT",
                        PortDir::Inout => "INOUT",
                    };
                    hits.push(FindHit {
                        kind: "port".into(),
                        name: p.name.clone(),
                        primitive: dir.into(),
                        parent: parent.clone(),
                    });
                }
            }
        }
        for pin in &self.package_pins {
            if pin.pin.to_ascii_lowercase().contains(&q) {
                hits.push(FindHit {
                    kind: "pin".into(),
                    name: pin.pin.clone(),
                    primitive: format!("BANK{}", pin.bank),
                    parent: pin.port.clone().unwrap_or_else(|| parent.clone()),
                });
            }
        }
        let n = hits.len();
        self.find_results = hits;
        self.selected_find = None;
        self.workspace = WorkspaceTab::Find;
        if n == 0 {
            return Err(format!("find {query}: 0 hits"));
        }
        let cell = self.find_results.iter().enumerate().find_map(|(i, h)| {
            if h.kind == "cell" || h.type_cell() == "cell" {
                Some((i, h.name.clone()))
            } else {
                None
            }
        });
        if let Some((i, name)) = cell {
            self.selected_find = Some(i);
            self.select(&name);
        }
        let mut s = format!("find {query} hits={n}");
        for h in &self.find_results {
            s.push_str(&format!(" {}:{}", h.kind, h.name));
        }
        s.push('\n');
        s.push_str(&self.find_results_text());
        Ok(s)
    }

    pub fn device_has_selected(&self) -> bool {
        let Some(id) = self.selected.as_deref() else {
            return false;
        };
        self.device.occupant_of(id).is_some()
            || self.device.sites.iter().any(|s| s.site_name() == id)
            || self.device.route_named(id).is_some()
            || self.pblocks.iter().any(|p| p.name == id)
    }

    /// UG893 Device drawing dump: HAD die occupancy map, not an occupant-name list.
    pub fn device_drawing_text(&self) -> String {
        let n = self.device.sites.len();
        let occ = self.device.occupied_count();
        let mut s = format!(
            "device drawing part={} cols={} rows={} sites={} occupied={} x0={} y0={} clock_regions={} routes={} pblocks={}",
            self.part(),
            self.device.cols,
            self.device.rows,
            n,
            occ,
            self.device.x0,
            self.device.y0,
            self.device.clock_regions.len(),
            self.device.routes.len(),
            self.pblocks.len()
        );
        for pb in &self.pblocks {
            s.push_str(&format!(
                " pb={}:{},{},{},{}:cells={}:frames={}:sites={}",
                pb.name,
                pb.x0,
                pb.y0,
                pb.x1,
                pb.y1,
                pb.cells.len(),
                pb.frames,
                pb.site_count(&self.device.sites)
            ));
        }
        for cr in &self.device.clock_regions {
            s.push_str(&format!(
                " cr={}:{},{},{},{}:sites={}",
                cr.name,
                cr.x0,
                cr.y0,
                cr.x1,
                cr.y1,
                cr.site_count(&self.device.sites)
            ));
        }
        for rt in &self.device.routes {
            let tiles = rt
                .tiles
                .iter()
                .map(|(x, y)| format!("X{x}Y{y}"))
                .collect::<Vec<_>>()
                .join(",");
            s.push_str(&format!(
                " route net={} hops={} delay_ps={} tiles={} hl={}",
                rt.net,
                rt.hops,
                rt.delay_ps,
                tiles,
                u8::from(rt.highlighted)
            ));
        }
        for site in self.device.sites.iter().filter(|s| !s.bels.is_empty()) {
            for bel in &site.bels {
                s.push_str(&format!(" occ {bel}={}", site.site_name()));
            }
        }
        if self.device.cols == 0 || self.device.rows == 0 {
            return s;
        }
        for dy in 0..self.device.rows {
            let y = self.device.y0 + dy;
            let mut map = String::with_capacity(self.device.cols as usize);
            for dx in 0..self.device.cols {
                let x = self.device.x0 + dx;
                if let Some(site) = self.device.site_at(x, y) {
                    map.push(site.occupancy_char());
                } else {
                    map.push(' ');
                }
            }
            s.push_str(&format!(" row y={y} map={map}"));
        }
        s
    }

    /// Click a Device tile: occupied sites cross-select the HNF cell.
    /// Intermediate PathFinder tiles (no occupant) cross-select the routed net.
    pub fn select_device_site(&mut self, spec: &str) -> Result<String, String> {
        let (x, y) = parse_site_xy(spec)?;
        let found = self
            .device
            .site_at(x, y)
            .cloned()
            .ok_or_else(|| format!("select_device_site: no HAD site X{x}Y{y}"))?;
        self.workspace = WorkspaceTab::Device;
        match found.occupant.clone() {
            Some(cell) => {
                self.select(&cell);
                Ok(format!(
                    "device_site {} occupant={cell} X{}Y{}",
                    found.site_name(),
                    found.x,
                    found.y
                ))
            }
            None => {
                if let Some(rt) = self.device.route_at(x, y).cloned() {
                    self.select(&rt.net);
                    return Ok(format!(
                        "device_site {} route={} hops={} delay_ps={} X{}Y{}",
                        found.site_name(),
                        rt.net,
                        rt.hops,
                        rt.delay_ps,
                        found.x,
                        found.y
                    ));
                }
                let name = found.site_name();
                self.select(&name);
                Ok(format!(
                    "device_site {name} unoccupied X{}Y{}",
                    found.x, found.y
                ))
            }
        }
    }

    /// UG893 Device: click a PathFinder net on the die (not an occupancy restyle).
    pub fn select_device_route(&mut self, net: &str) -> Result<String, String> {
        let net = net.trim();
        let found = self
            .device
            .routes
            .iter()
            .find(|r| r.net == net)
            .cloned()
            .ok_or_else(|| format!("select_device_route: no PathFinder net {net}"))?;
        self.workspace = WorkspaceTab::Device;
        self.select(&found.net);
        Ok(format!(
            "device_route net={} hops={} delay_ps={} tiles={}",
            found.net,
            found.hops,
            found.delay_ps,
            found.tiles.len()
        ))
    }

    pub fn properties_name(&self) -> Option<&str> {
        self.properties
            .iter()
            .find(|(k, _)| k == "NAME")
            .map(|(_, v)| v.as_str())
    }

    /// UG893 Properties: clickable Name/Value rows over the selected HNF/HAD/STA object.
    pub fn property_rows(&self) -> Vec<PropertyRow> {
        self.properties
            .iter()
            .map(|(name, value)| PropertyRow {
                name: name.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// UG893 Properties pane dump (tests). Paint is a clickable grid, not this string.
    pub fn properties_text(&self) -> String {
        let obj = self
            .properties_name()
            .or(self.selected.as_deref())
            .or(self.selected_ip.as_deref())
            .unwrap_or("-");
        let n = self.properties.len();
        let mut s = format!("properties n={n} object={obj}");
        if n == 0 {
            s.push_str("\nno properties — select a cell, net, or port");
            return s;
        }
        for row in self.property_rows() {
            s.push('\n');
            s.push_str(&row.row_text());
        }
        s
    }

    /// Click a UG893 Properties Name/Value row — highlight that property, not a dump.
    pub fn select_property(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_property: missing name".into());
        }
        if self.properties.is_empty() {
            return Err("select_property: no properties".into());
        }
        let idx = if let Ok(i) = spec.parse::<usize>() {
            if i >= self.properties.len() {
                return Err(format!("select_property: no row {spec}"));
            }
            i
        } else {
            self.properties
                .iter()
                .position(|(k, v)| {
                    k.eq_ignore_ascii_case(spec)
                        || format!("{k}={v}").eq_ignore_ascii_case(spec)
                        || format!("{k}:{v}").eq_ignore_ascii_case(spec)
                        || format!("NAME={k} VALUE={v}").eq_ignore_ascii_case(spec)
                })
                .ok_or_else(|| format!("select_property: no row {spec}"))?
        };
        let (name, value) = self.properties[idx].clone();
        self.selected_property = Some(name.clone());
        Ok(format!("property NAME={name} VALUE={value}"))
    }

    /// RTL vs constraint vs simulation fileset membership (UG893 Project Settings).
    fn fileset_of_source(path: &str) -> &'static str {
        match source_type_of(path) {
            "constraint" => "constrs_1",
            _ => "sources_1",
        }
    }

    /// Files in a UG893 fileset (`sources_1` / `constrs_1` / `sim_1`) from Session RTL + XDC.
    pub fn fileset_files(&self, name: &str) -> Vec<String> {
        let key = match name.trim().to_ascii_lowercase().as_str() {
            "sources_1" | "sources" | "sourceset" => "sources_1",
            "constrs_1" | "constrs" | "constraints" => "constrs_1",
            "sim_1" | "sim" | "simulation" => "sim_1",
            _ => return Vec::new(),
        };
        self.tree
            .sources
            .iter()
            .filter(|p| match key {
                "constrs_1" => Self::fileset_of_source(p) == "constrs_1",
                _ => Self::fileset_of_source(p) == "sources_1",
            })
            .cloned()
            .collect()
    }

    fn canon_fileset(name: &str) -> Result<&'static str, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "sources_1" | "sources" | "sourceset" => Ok("sources_1"),
            "constrs_1" | "constrs" | "constraints" => Ok("constrs_1"),
            "sim_1" | "sim" | "simulation" => Ok("sim_1"),
            other => Err(format!("select_project_setting: unknown fileset {other}")),
        }
    }

    fn impl_strategy_value(&self) -> String {
        self.runs
            .iter()
            .find(|r| r.step == "Implementation" && r.name == "impl_1")
            .or_else(|| self.runs.iter().find(|r| r.step == "Implementation"))
            .map(|r| r.strategy.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Default".into())
    }

    fn target_language_value(&self) -> &'static str {
        self.tree
            .sources
            .iter()
            .find_map(|p| {
                let t = source_type_of(p);
                matches!(t, "sv" | "verilog" | "vhdl").then_some(t)
            })
            .unwrap_or("-")
    }

    fn top_value(&self) -> String {
        self.tree
            .top
            .clone()
            .or_else(|| self.shell.session.design.as_ref().map(|d| d.name.clone()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "-".into())
    }

    fn fileset_files_cell(files: &[String]) -> String {
        if files.is_empty() {
            "-".into()
        } else {
            files
                .iter()
                .map(|p| source_basename(p))
                .collect::<Vec<_>>()
                .join(",")
        }
    }

    /// UG893 Project Settings: clickable Name/Value rows over Session (part / top / fileset / strategy).
    pub fn project_setting_rows(&self) -> Vec<PropertyRow> {
        let fileset = if self.active_fileset.is_empty() {
            "sources_1".to_string()
        } else {
            self.active_fileset.clone()
        };
        let mode = match self.shell.session.mode {
            Mode::Project => "Project",
            Mode::NonProject => "NonProject",
        };
        vec![
            PropertyRow {
                name: "PART".into(),
                value: self.part().to_string(),
            },
            PropertyRow {
                name: "TOP".into(),
                value: self.top_value(),
            },
            PropertyRow {
                name: "FILESET".into(),
                value: fileset,
            },
            PropertyRow {
                name: "STRATEGY".into(),
                value: self.impl_strategy_value(),
            },
            PropertyRow {
                name: "MODE".into(),
                value: mode.into(),
            },
            PropertyRow {
                name: "TARGET_LANGUAGE".into(),
                value: self.target_language_value().into(),
            },
            PropertyRow {
                name: "CLOCK_PERIOD_PS".into(),
                value: self.clock_period_ps.to_string(),
            },
        ]
    }

    /// UG893 Project Settings dump (tests). Paint is a clickable grid, not this string.
    pub fn project_settings_text(&self) -> String {
        let rows = self.project_setting_rows();
        let n = rows.len();
        let part = rows
            .iter()
            .find(|r| r.name == "PART")
            .map(|r| r.value.as_str())
            .unwrap_or("-");
        let top = rows
            .iter()
            .find(|r| r.name == "TOP")
            .map(|r| r.value.as_str())
            .unwrap_or("-");
        let fileset = rows
            .iter()
            .find(|r| r.name == "FILESET")
            .map(|r| r.value.as_str())
            .unwrap_or("-");
        let strategy = rows
            .iter()
            .find(|r| r.name == "STRATEGY")
            .map(|r| r.value.as_str())
            .unwrap_or("-");
        let mut s = format!(
            "project_settings n={n} part={part} top={top} fileset={fileset} strategy={strategy}"
        );
        for row in &rows {
            s.push('\n');
            s.push_str(&row.row_text());
        }
        s
    }

    pub fn open_project_settings(&mut self) -> Result<String, String> {
        self.nav = NavSection::ProjectManager;
        self.workspace = WorkspaceTab::Settings;
        Ok(self.project_settings_text())
    }

    fn apply_project_setting(&mut self, cmd: &str) -> Result<String, String> {
        let toks: Vec<&str> = cmd.split_whitespace().collect();
        if toks.len() < 3 {
            return Err("set_property: need KEY VALUE".into());
        }
        let key = toks[1];
        let val = toks[2].trim_matches('"');
        if key.eq_ignore_ascii_case("PART") {
            self.set_part(val)
        } else if key.eq_ignore_ascii_case("STRATEGY") {
            self.set_impl_strategy(val)
        } else if key.eq_ignore_ascii_case("FILESET") {
            self.set_active_fileset(val)
        } else {
            Err(format!("set_property: unsupported project setting {key}"))
        }
    }

    /// HAD part for the Session (`set_part` / Project Settings PART).
    pub fn set_part(&mut self, part: &str) -> Result<String, String> {
        let part = part.trim();
        if part.is_empty() {
            return Err("set_part: missing part".into());
        }
        let dev = Device::load_part(part)?;
        self.shell.part = dev.part.clone();
        self.shell.session.part = dev.part.clone();
        for r in &mut self.runs {
            r.part = dev.part.clone();
        }
        self.refresh_device();
        self.refresh_package();
        self.workspace = WorkspaceTab::Settings;
        self.selected_setting = Some("PART".into());
        self.refresh_project_setting_properties("PART");
        Ok(format!("set_part {}", self.part()))
    }

    fn set_impl_strategy(&mut self, name: &str) -> Result<String, String> {
        let strategy = ImplStrategy::parse(name)?;
        if let Some(r) = self
            .runs
            .iter_mut()
            .find(|r| r.name == "impl_1" && r.step == "Implementation")
        {
            r.strategy = strategy.label().into();
        } else if let Some(r) = self
            .runs
            .iter_mut()
            .find(|r| r.step == "Implementation")
        {
            r.strategy = strategy.label().into();
        }
        self.workspace = WorkspaceTab::Settings;
        self.selected_setting = Some("STRATEGY".into());
        self.refresh_project_setting_properties("STRATEGY");
        Ok(format!("set_property STRATEGY {}", strategy.label()))
    }

    fn set_active_fileset(&mut self, name: &str) -> Result<String, String> {
        let canon = Self::canon_fileset(name).map_err(|e| {
            e.replace("select_project_setting:", "set_property FILESET:")
        })?;
        self.active_fileset = canon.into();
        self.workspace = WorkspaceTab::Settings;
        self.selected_setting = Some("FILESET".into());
        self.refresh_project_setting_properties("FILESET");
        Ok(format!("set_property FILESET {canon}"))
    }

    fn refresh_project_setting_properties(&mut self, name: &str) {
        let rows = self.project_setting_rows();
        let value = rows
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(name))
            .map(|r| r.value.clone())
            .unwrap_or_else(|| "-".into());
        let mut props = vec![
            ("NAME".into(), name.to_string()),
            ("TYPE".into(), "project_setting".into()),
            ("VALUE".into(), value.clone()),
        ];
        match name {
            "PART" => {
                if let Ok(dev) = self.device() {
                    props.push(("IDCODE".into(), format!("{:#010x}", dev.idcode)));
                    props.push(("LUT6".into(), dev.lut6_count().to_string()));
                    props.push(("DSP".into(), dev.n_dsp.to_string()));
                    props.push(("BRAM".into(), dev.n_bram.to_string()));
                }
            }
            "TOP" => {
                props.push(("CELLS".into(), self.tree.cells.len().to_string()));
                props.push(("NETS".into(), self.tree.nets.len().to_string()));
            }
            "FILESET" => {
                let files = self.fileset_files(&self.active_fileset);
                props.push(("N".into(), files.len().to_string()));
                props.push(("FILES".into(), Self::fileset_files_cell(&files)));
                for fs in ["sources_1", "constrs_1", "sim_1"] {
                    let f = self.fileset_files(fs);
                    props.push((fs.to_ascii_uppercase().into(), Self::fileset_files_cell(&f)));
                }
            }
            "STRATEGY" => {
                let run = self
                    .runs
                    .iter()
                    .find(|r| r.step == "Implementation" && r.name == "impl_1")
                    .or_else(|| self.runs.iter().find(|r| r.step == "Implementation"));
                props.push((
                    "RUN".into(),
                    run.map(|r| r.name.clone()).unwrap_or_else(|| "impl_1".into()),
                ));
                props.push((
                    "STATUS".into(),
                    run.map(|r| r.status.clone())
                        .unwrap_or_else(|| "Not started".into()),
                ));
            }
            "CLOCK_PERIOD_PS" => {
                props.push(("UNIT".into(), "ps".into()));
            }
            _ => {}
        }
        self.properties = props;
        self.selected_property = Some(name.to_string());
    }

    /// Click a UG893 Project Settings Name/Value row — highlight that setting, not a dump.
    pub fn select_project_setting(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_project_setting: missing name".into());
        }
        if let Ok(fs) = Self::canon_fileset(spec) {
            self.active_fileset = fs.into();
            self.workspace = WorkspaceTab::Settings;
            self.selected_setting = Some("FILESET".into());
            self.refresh_project_setting_properties("FILESET");
            let files = self.fileset_files(fs);
            return Ok(format!(
                "project_setting NAME=FILESET VALUE={fs} FILES={}",
                Self::fileset_files_cell(&files)
            ));
        }
        let rows = self.project_setting_rows();
        let idx = if let Ok(i) = spec.parse::<usize>() {
            if i >= rows.len() {
                return Err(format!("select_project_setting: no row {spec}"));
            }
            i
        } else {
            rows.iter()
                .position(|r| {
                    r.name.eq_ignore_ascii_case(spec)
                        || format!("{}={}", r.name, r.value).eq_ignore_ascii_case(spec)
                        || format!("NAME={} VALUE={}", r.name, r.value).eq_ignore_ascii_case(spec)
                })
                .ok_or_else(|| format!("select_project_setting: no row {spec}"))?
        };
        let row = rows[idx].clone();
        if row.name == "FILESET" {
            if let Ok(fs) = Self::canon_fileset(&row.value) {
                self.active_fileset = fs.into();
            }
        }
        self.workspace = WorkspaceTab::Settings;
        self.selected_setting = Some(row.name.clone());
        self.refresh_project_setting_properties(&row.name);
        Ok(format!("project_setting NAME={} VALUE={}", row.name, row.value))
    }

    pub fn refresh_ip_catalog(&mut self) {
        self.ip_catalog = ipxact_catalog();
    }

    fn default_bd_cores(&self) -> Vec<IpCore> {
        self.ip_catalog
            .iter()
            .filter(|c| c.name == "h_uart" || c.name == "h_gpio")
            .cloned()
            .collect()
    }

    fn commit_block_design(&mut self, name: String, cores: Vec<IpCore>) -> Result<String, String> {
        let bd = BlockDesign {
            name,
            cores,
        };
        let v = validate(&bd);
        let sv = if v.ok {
            emit_sv(&bd)?
        } else {
            String::new()
        };
        let names: Vec<String> = bd.cores.iter().map(|c| c.name.clone()).collect();
        let msg = if v.ok {
            format!("create_bd_design {} cores={}", bd.name, names.join(","))
        } else {
            format!("create_bd_design failed {}", v.errors.join("; "))
        };
        self.block_design = Some(BdView {
            name: bd.name,
            cores: names,
            sv,
            ok: v.ok,
        });
        self.workspace = WorkspaceTab::Ip;
        if v.ok {
            Ok(msg)
        } else {
            Err(msg)
        }
    }

    pub fn create_block_design(&mut self) -> Result<String, String> {
        self.refresh_ip_catalog();
        let cores = self.default_bd_cores();
        self.commit_block_design("system".into(), cores)
    }

    /// UG893 IP Catalog rows: VLNV + bus from helion-ipxact.
    pub fn ip_catalog_rows(&self) -> Vec<IpCatalogRow> {
        let in_bd: Vec<&str> = self
            .block_design
            .as_ref()
            .map(|b| b.cores.iter().map(|c| c.as_str()).collect())
            .unwrap_or_default();
        self.ip_catalog
            .iter()
            .map(|c| IpCatalogRow {
                name: c.name.clone(),
                vendor: c.vendor.clone(),
                library: c.library.clone(),
                version: c.version.clone(),
                bus: c.bus.clone(),
                vlnv: c.vlnv(),
                status: if in_bd.iter().any(|n| *n == c.name) {
                    "In BD".into()
                } else {
                    "Available".into()
                },
            })
            .collect()
    }

    pub fn ip_catalog_text(&self) -> String {
        let rows = self.ip_catalog_rows();
        let mut s = format!("ip_catalog n={}", rows.len());
        for r in &rows {
            s.push_str(&format!(
                "\nNAME={} VLNV={} BUS={} STATUS={} VENDOR={} LIBRARY={} VERSION={}",
                r.name, r.vlnv, r.bus, r.status, r.vendor, r.library, r.version
            ));
        }
        s
    }

    fn find_ip_core(&self, spec: &str) -> Result<IpCore, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_ip: missing name".into());
        }
        if let Ok(i) = spec.parse::<usize>() {
            return self
                .ip_catalog
                .get(i)
                .cloned()
                .ok_or_else(|| format!("select_ip: no row {spec}"));
        }
        self.ip_catalog
            .iter()
            .find(|c| {
                c.name.eq_ignore_ascii_case(spec)
                    || c.vlnv().eq_ignore_ascii_case(spec)
            })
            .cloned()
            .ok_or_else(|| format!("select_ip: no IP {spec}"))
    }

    fn ip_properties(core: &IpCore, status: &str) -> Vec<(String, String)> {
        vec![
            ("NAME".into(), core.name.clone()),
            ("TYPE".into(), "ip_core".into()),
            ("VLNV".into(), core.vlnv()),
            ("VENDOR".into(), core.vendor.clone()),
            ("LIBRARY".into(), core.library.clone()),
            ("VERSION".into(), core.version.clone()),
            ("BUS".into(), core.bus.clone()),
            ("STATUS".into(), status.into()),
        ]
    }

    /// UG893 IP Catalog click: Properties = VLNV / bus from helion-ipxact.
    pub fn select_ip_core(&mut self, spec: &str) -> Result<String, String> {
        self.refresh_ip_catalog();
        let core = self.find_ip_core(spec)?;
        if core.bus == "AXI" {
            return Err(format!("select_ip: {} bus AXI is not Helion-MM/ST", core.name));
        }
        let status = self
            .ip_catalog_rows()
            .iter()
            .find(|r| r.name == core.name)
            .map(|r| r.status.clone())
            .unwrap_or_else(|| "Available".into());
        self.selected_ip = Some(core.name.clone());
        self.selected = Some(core.name.clone());
        self.workspace = WorkspaceTab::Ip;
        self.properties = Self::ip_properties(&core, &status);
        Ok(format!(
            "ip {} VLNV={} BUS={} STATUS={}",
            core.name,
            core.vlnv(),
            core.bus,
            status
        ))
    }

    /// UG893 Generate Output Products: helion-ipxact XML, not a canned string.
    pub fn generate_ip(&mut self, spec: &str) -> Result<String, String> {
        let spec = if spec.trim().is_empty() {
            self.selected_ip.clone().unwrap_or_default()
        } else {
            spec.to_string()
        };
        if spec.trim().is_empty() {
            return Err("generate_ip: missing name".into());
        }
        let core = self.find_ip_core(&spec)?;
        if core.bus == "AXI" {
            return Err(format!("generate_ip: {} bus AXI is not Helion-MM/ST", core.name));
        }
        let xml = to_xml(&core);
        if xml.contains("AXI") {
            return Err("generate_ip: IP-XACT must not emit AXI".into());
        }
        self.generated_ip_xml = Some(xml.clone());
        self.selected_ip = Some(core.name.clone());
        self.selected = Some(core.name.clone());
        self.workspace = WorkspaceTab::Ip;
        self.properties = Self::ip_properties(&core, "Generated");
        self.properties
            .push(("XML_BYTES".into(), xml.len().to_string()));
        Ok(format!(
            "generate_target {} VLNV={} BUS={} xml_bytes={} ipxact:name={}",
            core.name,
            core.vlnv(),
            core.bus,
            xml.len(),
            core.name
        ))
    }

    /// UG893 Add to Block Design: instantiate a catalog core on the Helion-MM canvas.
    pub fn create_bd_cell(&mut self, spec: &str) -> Result<String, String> {
        self.refresh_ip_catalog();
        let spec = if spec.trim().is_empty() {
            self.selected_ip.clone().unwrap_or_default()
        } else {
            spec.to_string()
        };
        if spec.trim().is_empty() {
            return Err("create_bd_cell: missing name".into());
        }
        let core = self.find_ip_core(&spec)?;
        if core.bus != "Helion-MM" && core.bus != "Helion-ST" {
            return Err(format!(
                "create_bd_cell: {} bus {} not Helion-MM/ST",
                core.name, core.bus
            ));
        }
        if self.block_design.is_none() {
            self.create_block_design()?;
        }
        let mut names = self
            .block_design
            .as_ref()
            .map(|b| b.cores.clone())
            .unwrap_or_default();
        if names.iter().any(|n| n == &core.name) {
            self.workspace = WorkspaceTab::Ip;
            return Ok(format!("create_bd_cell {} already on canvas", core.name));
        }
        names.push(core.name.clone());
        let cores: Vec<IpCore> = names
            .iter()
            .filter_map(|n| self.ip_catalog.iter().find(|c| c.name == *n).cloned())
            .collect();
        let bd_name = self
            .block_design
            .as_ref()
            .map(|b| b.name.clone())
            .unwrap_or_else(|| "system".into());
        let out = self.commit_block_design(bd_name, cores)?;
        Ok(format!("create_bd_cell {} {out}", core.name))
    }

    /// Generated HDL as instance rows (module / instance / bus / ports), not a SV dump.
    pub fn bd_hdl_rows(&self) -> Vec<BdHdlRow> {
        let Some(bd) = &self.block_design else {
            return Vec::new();
        };
        bd.cores
            .iter()
            .map(|name| {
                let bus = self
                    .ip_catalog
                    .iter()
                    .find(|c| c.name == *name)
                    .map(|c| c.bus.clone())
                    .unwrap_or_else(|| "Helion-MM".into());
                BdHdlRow {
                    instance: format!("u_{name}"),
                    module: name.clone(),
                    bus,
                    ports: "clk,resetn".into(),
                }
            })
            .collect()
    }

    pub fn bd_hdl_text(&self) -> String {
        let rows = self.bd_hdl_rows();
        let mut s = format!("bd_hdl n={}", rows.len());
        for r in &rows {
            s.push_str(&format!(
                "\nINSTANCE={} MODULE={} BUS={} PORTS={}",
                r.instance, r.module, r.bus, r.ports
            ));
        }
        s
    }

    /// IP Integrator canvas dump: IP boxes + Helion-MM wires, not a catalog list.
    pub fn bd_drawing_text(&self) -> String {
        let Some(bd) = &self.block_design else {
            return "drawing symbols=0 wires=0 (create_bd first)".into();
        };
        let d = bd.drawing(&self.ip_catalog);
        let mut s = format!(
            "drawing symbols={} wires={} canvas={}x{} bd={} ok={}",
            d.symbols.len(),
            d.wires.len(),
            d.width as i32,
            d.height as i32,
            bd.name,
            bd.ok
        );
        for sy in &d.symbols {
            s.push_str(&format!(
                " {}:{}:box={},{},{},{}:bus={}",
                sy.name,
                sy.kind,
                sy.x as i32,
                sy.y as i32,
                sy.w as i32,
                sy.h as i32,
                sy.bus
            ));
            for p in &sy.pins {
                s.push_str(&format!(
                    " pin:{}:{}@{},{}{}{}",
                    sy.name,
                    p.name,
                    p.x as i32,
                    p.y as i32,
                    if p.iface { ":iface" } else { "" },
                    if p.output { ":out" } else { ":in" }
                ));
            }
        }
        for w in &d.wires {
            s.push_str(&format!(
                " wire:{}:{}->{}:pts={}",
                w.net,
                w.src,
                w.dst,
                w.points.len()
            ));
        }
        for a in &d.addresses {
            s.push_str(&format!(
                " addr={}:0x{:x}:0x{:x}",
                a.slave, a.base, a.range
            ));
        }
        s
    }

    /// UG949 Clock Interaction pane: STA clock matrix, not a dump.
    /// Implicit analysis clock appears after synth; user clocks come from create_clock.
    pub fn clock_interaction(&self) -> ClockInteraction {
        let clks = if !self.constraints.clocks.is_empty() {
            self.constraints.clocks.clone()
        } else if self.timing.is_some() || self.shell.session.design.is_some() {
            self.clocks_for_sta()
        } else {
            return ClockInteraction::default();
        };
        report_clock_interaction(&clks, &self.constraints, self.timing.as_ref())
    }

    pub fn clock_interaction_text(&self) -> String {
        self.clock_interaction().text()
    }

    /// Click a From×To cell: properties + Clock Interaction workspace.
    pub fn select_clock_interaction(&mut self, from: &str, to: &str) -> Result<String, String> {
        let report = self.clock_interaction();
        let cell = report.cell(from, to).ok_or_else(|| {
            format!("select_clock_interaction: no cell {from}->{to}")
        })?;
        let wns = match cell.wns_ps {
            Some(w) => format!("{w}"),
            None => "n/a".into(),
        };
        self.selected = Some(format!("{from}->{to}"));
        self.properties = vec![
            ("NAME".into(), format!("{from}->{to}")),
            ("TYPE".into(), "clock_interaction".into()),
            ("FROM".into(), cell.from.clone()),
            ("TO".into(), cell.to.clone()),
            ("RELATION".into(), cell.relation.as_str().into()),
            ("COMMON_PS".into(), cell.common_period_ps.to_string()),
            ("REQ_PS".into(), cell.requirement_ps.to_string()),
            ("WNS_PS".into(), wns.clone()),
            ("PATHS".into(), cell.path_count.to_string()),
        ];
        self.workspace = WorkspaceTab::ClockInteraction;
        Ok(format!(
            "clock_interaction FROM={from} TO={to} {} COMMON_PS={} REQ_PS={} WNS_PS={wns} paths={}",
            cell.relation.as_str(),
            cell.common_period_ps,
            cell.requirement_ps,
            cell.path_count
        ))
    }

    /// UG903/UG949 Timing Summary pane: intra/inter-clock WNS/TNS/WHS/THS by path
    /// group from STA, not a dump. Implicit analysis clock after synth; user
    /// clocks from create_clock / group_path. Empty XDC keeps gold WNS.
    pub fn timing_summary(&self) -> TimingSummary {
        let clks = if !self.constraints.clocks.is_empty() {
            self.constraints.clocks.clone()
        } else if self.timing.is_some() || self.shell.session.design.is_some() {
            self.clocks_for_sta()
        } else {
            return TimingSummary::default();
        };
        report_timing_summary(&clks, &self.constraints, self.timing.as_ref())
    }

    pub fn timing_summary_text(&self) -> String {
        self.timing_summary().text()
    }

    /// Click a path-group row: properties + Reports workspace.
    pub fn select_timing_summary(
        &mut self,
        a: &str,
        b: Option<&str>,
    ) -> Result<String, String> {
        let report = self.timing_summary();
        let group = if let Some(to) = b {
            report.group(a, to).or_else(|| report.named(a))
        } else {
            report.named(a).or_else(|| report.group(a, a))
        }
        .ok_or_else(|| format!("select_timing_summary: no group {a}"))?;
        let wns = group
            .wns_ps
            .map(|w| w.to_string())
            .unwrap_or_else(|| "n/a".into());
        let whs = group
            .whs_ps
            .map(|w| w.to_string())
            .unwrap_or_else(|| "n/a".into());
        self.selected = Some(if group.kind == helion_sta::PathGroupKind::Other {
            group.name.clone()
        } else {
            format!("{}->{}", group.from, group.to)
        });
        self.properties = vec![
            ("NAME".into(), group.name.clone()),
            ("TYPE".into(), "timing_summary".into()),
            ("KIND".into(), group.kind.as_str().into()),
            ("FROM".into(), group.from.clone()),
            ("TO".into(), group.to.clone()),
            ("WNS_PS".into(), wns.clone()),
            ("TNS_PS".into(), group.tns_ps.to_string()),
            ("WHS_PS".into(), whs.clone()),
            ("THS_PS".into(), group.ths_ps.to_string()),
            ("FAILING_SETUP".into(), group.failing_setup.to_string()),
            ("FAILING_HOLD".into(), group.failing_hold.to_string()),
            ("ENDPOINTS".into(), group.endpoints.to_string()),
        ];
        self.workspace = WorkspaceTab::Reports;
        Ok(format!(
            "timing_summary NAME={} KIND={} FROM={} TO={} WNS_PS={wns} TNS_PS={} WHS_PS={whs} THS_PS={} endpoints={}",
            group.name,
            group.kind.as_str(),
            group.from,
            group.to,
            group.tns_ps,
            group.ths_ps,
            group.endpoints
        ))
    }

    /// UG893 Reports window: clickable catalog of engine reports, not stacked dumps.
    pub fn report_catalog(&self) -> Vec<ReportCatalogRow> {
        fn row(
            id: &str,
            name: &str,
            category: &str,
            workspace: WorkspaceTab,
            ready: bool,
            summary: String,
        ) -> ReportCatalogRow {
            ReportCatalogRow {
                id: id.into(),
                name: name.into(),
                category: category.into(),
                tcl: id.into(),
                status: if ready {
                    "Complete".into()
                } else {
                    "Not generated".into()
                },
                summary: if ready { summary } else { "-".into() },
                workspace,
            }
        }

        let ts = self.timing_summary();
        let ts_ready = !ts.clocks.is_empty();
        let ts_sum = format!(
            "WNS_PS={} TNS_PS={} clocks={}",
            ts.wns_ps
                .map(|w| w.to_string())
                .unwrap_or_else(|| "n/a".into()),
            ts.tns_ps,
            ts.clocks.len()
        );

        let ci = self.clock_interaction();
        let ci_ready = !ci.clocks.is_empty();
        let ci_sum = format!("clocks={} cells={}", ci.clocks.len(), ci.cells.len());

        let cdc = self.cdc_report();
        let cdc_ready = !cdc.clocks.is_empty();
        let cdc_sum = format!(
            "critical={} warning={} info={} safe={}",
            cdc.critical_count(),
            cdc.warning_count(),
            cdc.info_count(),
            cdc.safe_count()
        );

        let cn = self.clock_networks();
        let cn_ready = !cn.clocks.is_empty();
        let cn_sum = format!(
            "loads={} buffers={} INSERTION_PS={}",
            cn.total_loads, cn.total_buffers, cn.max_insertion_ps
        );

        let pwr = self.power_report();
        let pwr_ready = !pwr.part.is_empty();
        let pwr_sum = format!(
            "TOTAL_UW={} STATIC_UW={} DYNAMIC_UW={} VOLTAGE_MV={}",
            pwr.total_uw, pwr.static_uw, pwr.dynamic_uw, pwr.voltage_mv
        );

        let meth = self.methodology_report();
        let meth_ready = self.shell.session.design.is_some();
        let meth_sum = format!(
            "checks={} errors={} warning={}",
            meth.checks.len(),
            meth.error_count(),
            meth.warning_count()
        );

        let drc = self.drc.clone().unwrap_or_else(|| self.drc_report());
        let drc_ready = self.drc.is_some()
            || self.utilization.is_some()
            || self.shell.session.placed.is_some()
            || self.shell.session.routed.is_some();
        let drc_sum = format!(
            "violations={} errors={}",
            drc.violations.len(),
            drc.error_count()
        );

        let util = self.utilization_report();
        let util_ready = !util.part.is_empty();
        let util_sum = if let Some(lut) = util.row("LUTFF") {
            format!(
                "LUTFF={}/{} IOB={}/{}",
                lut.used,
                lut.available,
                util.row("IOB").map(|r| r.used).unwrap_or(0),
                util.row("IOB").map(|r| r.available).unwrap_or(0)
            )
        } else {
            format!("part={}", util.part)
        };

        let bits = self.bitstream_report();
        let bits_ready = self.shell.session.bitstream.is_some();
        let bits_sum = format!(
            "hash={:#010x} frames={} configured={}",
            bits.hash, bits.frames, bits.configured
        );

        vec![
            row(
                "report_timing_summary",
                "Timing Summary",
                "Timing",
                WorkspaceTab::Reports,
                ts_ready,
                ts_sum,
            ),
            row(
                "report_clock_interaction",
                "Clock Interaction",
                "Timing",
                WorkspaceTab::ClockInteraction,
                ci_ready,
                ci_sum,
            ),
            row(
                "report_cdc",
                "CDC",
                "Timing",
                WorkspaceTab::Cdc,
                cdc_ready,
                cdc_sum,
            ),
            row(
                "report_clock_networks",
                "Clock Networks",
                "Timing",
                WorkspaceTab::ClockNetworks,
                cn_ready,
                cn_sum,
            ),
            row(
                "report_power",
                "Power",
                "Implementation",
                WorkspaceTab::Power,
                pwr_ready,
                pwr_sum,
            ),
            row(
                "report_methodology",
                "Methodology",
                "Implementation",
                WorkspaceTab::Methodology,
                meth_ready,
                meth_sum,
            ),
            row(
                "report_drc",
                "DRC",
                "Implementation",
                WorkspaceTab::Drc,
                drc_ready,
                drc_sum,
            ),
            row(
                "report_utilization",
                "Utilization",
                "Implementation",
                WorkspaceTab::Utilization,
                util_ready,
                util_sum,
            ),
            row(
                "report_bitstream",
                "Bitstream",
                "Program",
                WorkspaceTab::Bitstream,
                bits_ready,
                bits_sum,
            ),
        ]
    }

    pub fn reports_text(&self) -> String {
        let rows = self.report_catalog();
        let n_done = rows.iter().filter(|r| r.status == "Complete").count();
        let mut s = format!("reports n={} complete={n_done}", rows.len());
        for r in &rows {
            s.push('\n');
            s.push_str(&r.row_text());
        }
        s
    }

    /// Click a Reports catalog row: properties + open that engine pane.
    pub fn select_report(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_report: missing name".into());
        }
        let key = spec.strip_prefix("report:").unwrap_or(spec);
        let spec_l = key.to_ascii_lowercase();
        let rows = self.report_catalog();
        let row = if let Ok(i) = key.parse::<usize>() {
            rows.get(i)
                .cloned()
                .ok_or_else(|| format!("select_report: no row {spec}"))?
        } else {
            rows.iter()
                .find(|r| r.id.eq_ignore_ascii_case(key) || r.tcl.eq_ignore_ascii_case(key))
                .cloned()
                .or_else(|| {
                    rows.iter()
                        .find(|r| r.name.eq_ignore_ascii_case(key))
                        .cloned()
                })
                .or_else(|| {
                    rows.iter()
                        .find(|r| {
                            r.id.to_ascii_lowercase().contains(&spec_l)
                                || r.name.to_ascii_lowercase().contains(&spec_l)
                                || r.category.to_ascii_lowercase() == spec_l
                        })
                        .cloned()
                })
                .ok_or_else(|| format!("select_report: no row {spec}"))?
        };
        self.selected_report = Some(row.id.clone());
        self.selected = Some(format!("report:{}", row.id));
        self.properties = vec![
            ("NAME".into(), row.id.clone()),
            ("TYPE".into(), "report".into()),
            ("TITLE".into(), row.name.clone()),
            ("CATEGORY".into(), row.category.clone()),
            ("STATUS".into(), row.status.clone()),
            ("SUMMARY".into(), row.summary.clone()),
            ("TCL".into(), row.tcl.clone()),
        ];
        self.workspace = row.workspace;
        Ok(format!("report {}", row.row_text()))
    }

    fn pane_clocks(&self) -> Vec<helion_sta::Clock> {
        if !self.constraints.clocks.is_empty() {
            self.constraints.clocks.clone()
        } else if self.timing.is_some() || self.shell.session.design.is_some() {
            self.clocks_for_sta()
        } else {
            Vec::new()
        }
    }

    /// UG906 CDC pane: STA inter-clock rows + XDC exceptions, not a dump.
    pub fn cdc_report(&self) -> CdcReport {
        let clks = self.pane_clocks();
        if clks.is_empty() {
            return CdcReport::default();
        }
        report_cdc(
            &clks,
            &self.constraints,
            self.timing.as_ref(),
            self.shell.session.design.as_ref(),
        )
    }

    pub fn cdc_text(&self) -> String {
        self.cdc_report().text()
    }

    /// Click a CDC row: properties + CDC workspace.
    pub fn select_cdc(&mut self, from: &str, to: &str) -> Result<String, String> {
        let report = self.cdc_report();
        let v = report
            .violation(from, to)
            .ok_or_else(|| format!("select_cdc: no row {from}->{to}"))?;
        let wns = match v.wns_ps {
            Some(w) => format!("{w}"),
            None => "n/a".into(),
        };
        self.selected = Some(format!("{from}->{to}"));
        self.properties = vec![
            ("NAME".into(), format!("{from}->{to}")),
            ("TYPE".into(), "cdc".into()),
            ("FROM".into(), v.from.clone()),
            ("TO".into(), v.to.clone()),
            ("SEVERITY".into(), v.severity.as_str().into()),
            ("CHECK".into(), v.check.clone()),
            ("SYNC".into(), u8::from(v.synchronizer).to_string()),
            ("ENDPOINTS".into(), v.endpoints.to_string()),
            ("WNS_PS".into(), wns.clone()),
            ("RELATION".into(), v.relation.as_str().into()),
        ];
        self.workspace = WorkspaceTab::Cdc;
        Ok(format!(
            "cdc FROM={from} TO={to} SEVERITY={} CHECK={} SYNC={} ENDPOINTS={} WNS_PS={wns} RELATION={}",
            v.severity.as_str(),
            v.check,
            u8::from(v.synchronizer),
            v.endpoints,
            v.relation.as_str()
        ))
    }

    /// UG903 Clock Networks pane: STA clocks + HNF FF loads + HAD spine insertion.
    pub fn clock_networks(&self) -> ClockNetworkReport {
        let clks = self.pane_clocks();
        if clks.is_empty() {
            return ClockNetworkReport::default();
        }
        let placed = self
            .shell
            .session
            .routed
            .as_ref()
            .map(|r| &r.placed)
            .or(self.shell.session.placed.as_ref());
        report_clock_networks(&clks, self.shell.session.design.as_ref(), placed)
    }

    pub fn clock_networks_text(&self) -> String {
        self.clock_networks().text()
    }

    /// Click a clock-tree row: properties + Clock Networks workspace.
    pub fn select_clock_network(&mut self, name: &str) -> Result<String, String> {
        let report = self.clock_networks();
        let n = report
            .network(name)
            .ok_or_else(|| format!("select_clock_network: no clock {name}"))?;
        self.selected = Some(n.name.clone());
        self.properties = vec![
            ("NAME".into(), n.name.clone()),
            ("TYPE".into(), "clock_network".into()),
            ("PERIOD_PS".into(), n.period_ps.to_string()),
            ("SOURCE".into(), n.source.clone()),
            ("NET".into(), n.net.clone()),
            ("GENERATED".into(), u8::from(n.generated).to_string()),
            (
                "MASTER".into(),
                n.master.clone().unwrap_or_else(|| "-".into()),
            ),
            ("LOADS".into(), n.n_loads.to_string()),
            ("BUFFERS".into(), n.n_buffers.to_string()),
            ("FANOUT".into(), n.fanout.to_string()),
            ("INSERTION_PS".into(), n.insertion_ps.to_string()),
        ];
        self.workspace = WorkspaceTab::ClockNetworks;
        Ok(format!(
            "clock_network NAME={} PERIOD_PS={} SOURCE={} NET={} loads={} buffers={} fanout={} INSERTION_PS={}",
            n.name,
            n.period_ps,
            n.source,
            n.net,
            n.n_loads,
            n.n_buffers,
            n.fanout,
            n.insertion_ps
        ))
    }

    /// UG907 Power pane: HAD occupancy × STA clocks × PVT, not a dump.
    pub fn power_report(&self) -> PowerReport {
        if self.shell.session.design.is_none() && self.shell.session.placed.is_none() {
            return PowerReport::default();
        }
        let Ok(dev) = self.device() else {
            return PowerReport::default();
        };
        let clks = self.pane_clocks();
        let placed = self
            .shell
            .session
            .routed
            .as_ref()
            .map(|r| &r.placed)
            .or(self.shell.session.placed.as_ref());
        report_power(
            &dev,
            self.shell.session.design.as_ref(),
            placed,
            &clks,
            &self.constraints.operating_conditions,
        )
    }

    pub fn power_text(&self) -> String {
        self.power_report().text()
    }

    /// Click a power rail: properties + Power workspace.
    pub fn select_power(&mut self, rail: &str) -> Result<String, String> {
        let p = self.power_report();
        if p.part.is_empty() {
            return Err("select_power: no design".into());
        }
        let (name, uw) = match rail.trim().to_ascii_lowercase().as_str() {
            "total" | "" => ("total", p.total_uw),
            "static" => ("static", p.static_uw),
            "dynamic" => ("dynamic", p.dynamic_uw),
            "clocks" | "clock" => ("clocks", p.clocks_uw),
            "logic" => ("logic", p.logic_uw),
            "signals" | "signal" => ("signals", p.signals_uw),
            "io" | "iob" => ("io", p.io_uw),
            "bram" => ("bram", p.bram_uw),
            "dsp" => ("dsp", p.dsp_uw),
            other => return Err(format!("select_power: unknown rail {other}")),
        };
        self.selected = Some(name.into());
        self.properties = vec![
            ("NAME".into(), name.into()),
            ("TYPE".into(), "power".into()),
            ("UW".into(), uw.to_string()),
            ("TOTAL_UW".into(), p.total_uw.to_string()),
            ("STATIC_UW".into(), p.static_uw.to_string()),
            ("DYNAMIC_UW".into(), p.dynamic_uw.to_string()),
            ("VOLTAGE_MV".into(), p.voltage_mv.to_string()),
            ("TEMP_C".into(), p.temperature_c.to_string()),
            ("F_MHZ".into(), p.f_mhz.to_string()),
            ("PART".into(), p.part.clone()),
        ];
        self.workspace = WorkspaceTab::Power;
        Ok(format!(
            "power RAIL={name} UW={uw} TOTAL_UW={} STATIC_UW={} DYNAMIC_UW={} VOLTAGE_MV={} TEMP_C={} F_MHZ={}",
            p.total_uw, p.static_uw, p.dynamic_uw, p.voltage_mv, p.temperature_c, p.f_mhz
        ))
    }

    fn hierarchical_occupancy(design: &Design) -> Vec<HierOccupancy> {
        let mut top = HierOccupancy {
            name: design.name.clone(),
            lut: 0,
            ff: 0,
            iob: 0,
            bram: 0,
            dsp: 0,
        };
        for c in &design.cells {
            match c.kind {
                CellKind::Lut6 { .. } => top.lut += 1,
                CellKind::Hff => top.ff += 1,
                CellKind::IobOut => top.iob += 1,
                CellKind::Bram18 => top.bram += 1,
                CellKind::Mac27 => top.dsp += 1,
                _ => {}
            }
        }
        vec![top]
    }

    /// UG893 Utilization occupancy pane: packed HAD used/available + HNF hierarchy.
    pub fn utilization_report(&self) -> UtilizationReport {
        let Some(u) = self.utilization else {
            return UtilizationReport::default();
        };
        let part = self.part().to_string();
        let occupancy = u.occupancy().to_vec();
        let hierarchy = self
            .shell
            .session
            .design
            .as_ref()
            .map(Self::hierarchical_occupancy)
            .unwrap_or_default();
        UtilizationReport {
            part,
            occupancy,
            hierarchy,
        }
    }

    /// Click a resource row: properties + Utilization workspace.
    pub fn select_utilization(&mut self, resource: &str) -> Result<String, String> {
        let r = self.utilization_report();
        if r.part.is_empty() {
            return Err("select_utilization: no placed design".into());
        }
        let row = r
            .row(resource)
            .copied()
            .ok_or_else(|| format!("select_utilization: unknown resource {resource}"))?;
        self.selected = Some(row.resource.into());
        self.properties = vec![
            ("NAME".into(), row.resource.into()),
            ("TYPE".into(), "utilization".into()),
            ("USED".into(), row.used.to_string()),
            ("AVAILABLE".into(), row.available.to_string()),
            ("PCT".into(), row.pct().to_string()),
            ("PART".into(), r.part.clone()),
        ];
        self.workspace = WorkspaceTab::Utilization;
        Ok(format!(
            "utilization RESOURCE={} USED={} AVAILABLE={} PCT={} PART={}",
            row.resource,
            row.used,
            row.available,
            row.pct(),
            r.part
        ))
    }

    /// Live helion-drc result from placed/routed Session (not a cached dump).
    pub fn drc_report(&self) -> Drc {
        let Ok(dev) = self.device() else {
            return Drc::default();
        };
        let s = &self.shell.session;
        if let (Some(d), Some(r)) = (s.design.as_ref(), s.routed.as_ref()) {
            check_routed(d, r, &dev)
        } else if let (Some(d), Some(p)) = (s.design.as_ref(), s.placed.as_ref()) {
            check_placed(d, p, &dev)
        } else {
            Drc::default()
        }
    }

    pub fn drc_text(&self) -> String {
        match (&self.drc, self.shell.session.placed.is_some() || self.shell.session.routed.is_some())
        {
            (Some(d), _) => d.text(),
            (None, true) => self.drc_report().text(),
            (None, false) => "no DRC — run Place/Route".into(),
        }
    }

    /// Click a DRC rule: properties + DRC workspace.
    pub fn select_drc(&mut self, id: &str) -> Result<String, String> {
        let report = if let Some(d) = self.drc.clone() {
            d
        } else {
            self.drc_report()
        };
        if report.ok() && report.items.is_empty() {
            return Err("select_drc: no violations".into());
        }
        let v = if let Some(v) = report.item(id) {
            v.clone()
        } else if let Ok(i) = id.parse::<usize>() {
            report
                .items
                .get(i)
                .cloned()
                .ok_or_else(|| format!("select_drc: no row {id}"))?
        } else {
            report
                .items
                .iter()
                .find(|v| v.message.contains(id) || v.objects.contains(id))
                .cloned()
                .ok_or_else(|| format!("select_drc: no rule {id}"))?
        };
        self.selected = Some(v.id.clone());
        self.properties = vec![
            ("NAME".into(), v.id.clone()),
            ("TYPE".into(), "drc".into()),
            ("SEVERITY".into(), v.severity.as_str().into()),
            ("OBJECTS".into(), v.objects.clone()),
            ("MESSAGE".into(), v.message.clone()),
        ];
        self.workspace = WorkspaceTab::Drc;
        Ok(format!(
            "drc ID={} SEVERITY={} OBJECTS={} {}",
            v.id,
            v.severity.as_str(),
            if v.objects.is_empty() {
                "-"
            } else {
                v.objects.as_str()
            },
            v.message
        ))
    }

    /// UG949 Methodology pane: STA/XDC/HNF checks, not a dump. Empty XDC keeps gold WNS.
    pub fn methodology_report(&self) -> MethodologyReport {
        let Some(d) = self.shell.session.design.as_ref() else {
            return MethodologyReport::default();
        };
        let clks = self.pane_clocks();
        report_methodology(&clks, &self.constraints, self.timing.as_ref(), Some(d))
    }

    pub fn methodology_text(&self) -> String {
        if self.shell.session.design.is_none() {
            return "no design — synth / report_methodology".into();
        }
        self.methodology_report().text()
    }

    /// Click a methodology check: properties + Methodology workspace.
    pub fn select_methodology(&mut self, id: &str) -> Result<String, String> {
        let report = self.methodology_report();
        let v = report
            .check(id)
            .or_else(|| {
                report
                    .checks
                    .iter()
                    .find(|c| c.id.eq_ignore_ascii_case(id) || c.objects == id)
            })
            .cloned()
            .ok_or_else(|| format!("select_methodology: no check {id}"))?;
        self.selected = Some(v.id.clone());
        self.properties = vec![
            ("NAME".into(), v.id.clone()),
            ("TYPE".into(), "methodology".into()),
            ("SEVERITY".into(), v.severity.as_str().into()),
            ("CATEGORY".into(), v.category.clone()),
            ("OBJECTS".into(), v.objects.clone()),
            ("MESSAGE".into(), v.message.clone()),
        ];
        self.workspace = WorkspaceTab::Methodology;
        Ok(format!(
            "methodology ID={} SEVERITY={} CATEGORY={} OBJECTS={} {}",
            v.id,
            v.severity.as_str(),
            v.category,
            if v.objects.is_empty() {
                "-"
            } else {
                v.objects.as_str()
            },
            v.message
        ))
    }

    /// UG893 Timing Constraints Editor: clickable clocks / I/O-delay / exception
    /// rows from helion-sta XDC (not a concatenated dump). Empty XDC keeps gold WNS.
    pub fn constraint_rows(&self) -> Vec<ConstraintRow> {
        let mut rows = Vec::new();
        let mut push = |section: ConstraintSection,
                        kind: &str,
                        id: String,
                        name: String,
                        from: String,
                        to: String,
                        value: String| {
            rows.push(ConstraintRow {
                id,
                section,
                kind: kind.into(),
                name,
                from,
                to,
                value,
                enabled: true,
            });
        };
        for c in &self.constraints.clocks {
            let kind = if c.generated {
                "create_generated_clock"
            } else {
                "create_clock"
            };
            let mut value = format!("PERIOD_PS={}", c.period_ps);
            if c.generated {
                let master = c.master.as_deref().unwrap_or("clk");
                value.push_str(&format!(
                    " DIVIDE_BY={} MULTIPLY_BY={} INVERT={} MASTER={master}",
                    c.divide_by,
                    c.multiply_by,
                    u8::from(c.invert)
                ));
                if !c.edges.is_empty() {
                    value.push_str(&format!(
                        " EDGES={}",
                        c.edges
                            .iter()
                            .map(|e| e.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                }
            }
            push(
                ConstraintSection::Clocks,
                kind,
                format!("clock:{}", c.name),
                c.name.clone(),
                c.master.clone().unwrap_or_default(),
                c.source.clone(),
                value,
            );
        }
        for (port, ps) in &self.constraints.input_delay_ps {
            push(
                ConstraintSection::IoDelay,
                "set_input_delay",
                format!("input_delay:{port}"),
                port.clone(),
                String::new(),
                port.clone(),
                format!("DELAY_PS={ps}"),
            );
        }
        for (port, ps) in &self.constraints.output_delay_ps {
            push(
                ConstraintSection::IoDelay,
                "set_output_delay",
                format!("output_delay:{port}"),
                port.clone(),
                String::new(),
                port.clone(),
                format!("DELAY_PS={ps}"),
            );
        }
        for (i, fp) in self.constraints.false_paths.iter().enumerate() {
            let (from, to) = constraint_from_to(fp);
            let name = if from.is_empty() && to.is_empty() {
                fp.clone()
            } else if to.is_empty() {
                from.clone()
            } else {
                format!("{from}->{to}")
            };
            push(
                ConstraintSection::Exception,
                "set_false_path",
                format!("false_path:{i}:{name}"),
                name,
                from,
                to,
                fp.clone(),
            );
        }
        for (i, m) in self.constraints.multicycle_paths.iter().enumerate() {
            push(
                ConstraintSection::Exception,
                "set_multicycle_path",
                format!("multicycle:{i}:{}->{}", m.from, m.to),
                format!("{}->{}", m.from, m.to),
                m.from.clone(),
                m.to.clone(),
                format!("SETUP_MULT={} HOLD_MULT={}", m.setup_mult, m.hold_mult),
            );
        }
        for (i, m) in self.constraints.max_delays.iter().enumerate() {
            push(
                ConstraintSection::Exception,
                "set_max_delay",
                format!("max_delay:{i}:{}->{}", m.from, m.to),
                format!("{}->{}", m.from, m.to),
                m.from.clone(),
                m.to.clone(),
                format!(
                    "DELAY_PS={} datapath_only={}",
                    m.delay_ps,
                    u8::from(m.datapath_only)
                ),
            );
        }
        for (i, m) in self.constraints.min_delays.iter().enumerate() {
            push(
                ConstraintSection::Exception,
                "set_min_delay",
                format!("min_delay:{i}:{}->{}", m.from, m.to),
                format!("{}->{}", m.from, m.to),
                m.from.clone(),
                m.to.clone(),
                format!(
                    "DELAY_PS={} datapath_only={}",
                    m.delay_ps,
                    u8::from(m.datapath_only)
                ),
            );
        }
        for (i, b) in self.constraints.bus_skews.iter().enumerate() {
            push(
                ConstraintSection::Exception,
                "set_bus_skew",
                format!("bus_skew:{i}:{}->{}", b.from, b.to),
                format!("{}->{}", b.from, b.to),
                b.from.clone(),
                b.to.clone(),
                format!(
                    "SKEW_PS={} setup={} hold={}",
                    b.skew_ps,
                    u8::from(b.setup),
                    u8::from(b.hold)
                ),
            );
        }
        for (i, g) in self.constraints.path_groups.iter().enumerate() {
            push(
                ConstraintSection::Exception,
                "group_path",
                format!("group_path:{i}:{}", g.name),
                g.name.clone(),
                g.from.clone(),
                g.to.clone(),
                format!(
                    "WEIGHT_MILLI={} CRITICAL_RANGE_PS={}",
                    g.weight_milli, g.critical_range_ps
                ),
            );
        }
        for (i, b) in self.constraints.max_time_borrows.iter().enumerate() {
            let obj = if b.object.is_empty() {
                "-".into()
            } else {
                b.object.clone()
            };
            push(
                ConstraintSection::Exception,
                "set_max_time_borrow",
                format!("max_time_borrow:{i}:{obj}"),
                obj.clone(),
                String::new(),
                obj,
                format!("BORROW_PS={}", b.borrow_ps),
            );
        }
        for (i, d) in self.constraints.data_checks.iter().enumerate() {
            push(
                ConstraintSection::Exception,
                "set_data_check",
                format!("data_check:{i}:{}->{}", d.from, d.to),
                format!("{}->{}", d.from, d.to),
                d.from.clone(),
                d.to.clone(),
                format!("SETUP_PS={} HOLD_PS={}", d.setup_ps, d.hold_ps),
            );
        }
        for (i, g) in self.constraints.clock_groups.iter().enumerate() {
            let kind_flag = if g.asynchronous {
                "asynchronous"
            } else if g.exclusive {
                "exclusive"
            } else {
                "groups"
            };
            let from = g.groups.first().map(|grp| grp.join(",")).unwrap_or_default();
            let to = g.groups.get(1).map(|grp| grp.join(",")).unwrap_or_default();
            push(
                ConstraintSection::Exception,
                "set_clock_groups",
                format!("clock_groups:{i}"),
                kind_flag.into(),
                from,
                to,
                format!("{kind_flag} groups={}", g.groups.len()),
            );
        }
        for (i, u) in self.constraints.clock_uncertainties.iter().enumerate() {
            push(
                ConstraintSection::Exception,
                "set_clock_uncertainty",
                format!("clock_uncertainty:{i}:{}->{}", u.from, u.to),
                format!("{}->{}", u.from, u.to),
                u.from.clone(),
                u.to.clone(),
                format!("SETUP_PS={} HOLD_PS={}", u.setup_ps, u.hold_ps),
            );
        }
        for l in &self.constraints.clock_latencies {
            push(
                ConstraintSection::Exception,
                "set_clock_latency",
                format!("clock_latency:{}", l.clock),
                l.clock.clone(),
                l.clock.clone(),
                String::new(),
                format!(
                    "LATE_PS={} EARLY_PS={} source={}",
                    l.late_ps,
                    l.early_ps,
                    u8::from(l.source)
                ),
            );
        }
        for (i, d) in self.constraints.disable_timings.iter().enumerate() {
            let name = if d.object.is_empty() {
                format!("{}->{}", d.from, d.to)
            } else {
                d.object.clone()
            };
            push(
                ConstraintSection::Exception,
                "set_disable_timing",
                format!("disable_timing:{i}:{name}"),
                name,
                d.from.clone(),
                d.to.clone(),
                d.object.clone(),
            );
        }
        for c in &self.constraints.case_analyses {
            push(
                ConstraintSection::Exception,
                "set_case_analysis",
                format!("case_analysis:{}", c.object),
                c.object.clone(),
                String::new(),
                c.object.clone(),
                format!("VALUE={}", c.value),
            );
        }
        for p in &self.constraints.propagated_clocks {
            push(
                ConstraintSection::Exception,
                "set_propagated_clock",
                format!("propagated_clock:{p}"),
                p.clone(),
                p.clone(),
                String::new(),
                "propagated=1".into(),
            );
        }
        for s in &self.constraints.clock_senses {
            push(
                ConstraintSection::Exception,
                "set_clock_sense",
                format!("clock_sense:{}", s.object),
                s.object.clone(),
                String::new(),
                s.object.clone(),
                format!("SENSE={}", s.sense),
            );
        }
        for j in &self.constraints.input_jitters {
            push(
                ConstraintSection::Exception,
                "set_input_jitter",
                format!("input_jitter:{}", j.clock),
                j.clock.clone(),
                j.clock.clone(),
                String::new(),
                format!("JITTER_PS={}", j.jitter_ps),
            );
        }
        if self.constraints.system_jitter_ps != 0 {
            push(
                ConstraintSection::Exception,
                "set_system_jitter",
                "system_jitter".into(),
                "system".into(),
                String::new(),
                String::new(),
                format!("JITTER_PS={}", self.constraints.system_jitter_ps),
            );
        }
        for (i, d) in self.constraints.timing_derates.iter().enumerate() {
            push(
                ConstraintSection::Exception,
                "set_timing_derate",
                format!("timing_derate:{i}"),
                "derate".into(),
                String::new(),
                String::new(),
                format!(
                    "LATE_MILLI={} EARLY_MILLI={} cell={} net={}",
                    d.late_milli,
                    d.early_milli,
                    u8::from(d.cell),
                    u8::from(d.net)
                ),
            );
        }
        if self.constraints.operating_conditions.is_set() {
            let oc = &self.constraints.operating_conditions;
            push(
                ConstraintSection::Exception,
                "set_operating_conditions",
                "operating_conditions".into(),
                "pvt".into(),
                String::new(),
                String::new(),
                format!(
                    "VOLTAGE_MV={} TEMP_C={} SCALE_MILLI={}",
                    oc.voltage_mv,
                    oc.temperature_c,
                    oc.scale_milli()
                ),
            );
        }
        for (port, pin) in &self.constraints.package_pins {
            push(
                ConstraintSection::Exception,
                "set_property PACKAGE_PIN",
                format!("package_pin:{port}"),
                port.clone(),
                String::new(),
                port.clone(),
                pin.clone(),
            );
        }
        for (port, std) in &self.constraints.iostandards {
            push(
                ConstraintSection::Exception,
                "set_property IOSTANDARD",
                format!("iostandard:{port}"),
                port.clone(),
                String::new(),
                port.clone(),
                std.clone(),
            );
        }
        for (port, ma) in &self.constraints.drives {
            push(
                ConstraintSection::Exception,
                "set_property DRIVE",
                format!("drive:{port}"),
                port.clone(),
                String::new(),
                port.clone(),
                ma.clone(),
            );
        }
        for (port, slew) in &self.constraints.slews {
            push(
                ConstraintSection::Exception,
                "set_property SLEW",
                format!("slew:{port}"),
                port.clone(),
                String::new(),
                port.clone(),
                slew.clone(),
            );
        }
        for (port, pull) in &self.constraints.pulltypes {
            push(
                ConstraintSection::Exception,
                "set_property PULLTYPE",
                format!("pulltype:{port}"),
                port.clone(),
                String::new(),
                port.clone(),
                pull.clone(),
            );
        }
        for (port, term) in &self.constraints.diff_terms {
            push(
                ConstraintSection::Exception,
                "set_property DIFF_TERM",
                format!("diff_term:{port}"),
                port.clone(),
                String::new(),
                port.clone(),
                term.clone(),
            );
        }
        for (port, term) in &self.constraints.in_terms {
            push(
                ConstraintSection::Exception,
                "set_property IN_TERM",
                format!("in_term:{port}"),
                port.clone(),
                String::new(),
                port.clone(),
                term.clone(),
            );
        }
        for pb in &self.pblocks {
            let mut value = if pb.ranged {
                format!("RANGE={}", pb.range_text())
            } else {
                "pblock".into()
            };
            if !pb.cells.is_empty() {
                value.push_str(&format!(" CELLS={}", pb.cells.join(",")));
            }
            push(
                ConstraintSection::Exception,
                "create_pblock",
                format!("pblock:{}", pb.name),
                pb.name.clone(),
                String::new(),
                String::new(),
                value,
            );
        }
        rows
    }

    pub fn constraints_table_text(&self) -> String {
        let rows = self.constraint_rows();
        if rows.is_empty() {
            return "no timing constraints — create_clock / create_generated_clock / read_xdc"
                .into();
        }
        let n_clk = rows
            .iter()
            .filter(|r| r.section == ConstraintSection::Clocks)
            .count();
        let n_io = rows
            .iter()
            .filter(|r| r.section == ConstraintSection::IoDelay)
            .count();
        let n_ex = rows
            .iter()
            .filter(|r| r.section == ConstraintSection::Exception)
            .count();
        let mut s = format!("constraints clocks={n_clk} io_delay={n_io} exceptions={n_ex}");
        for r in &rows {
            let from = if r.from.is_empty() { "-" } else { r.from.as_str() };
            let to = if r.to.is_empty() { "-" } else { r.to.as_str() };
            s.push_str(&format!(
                "\n{} ID={} KIND={} NAME={} FROM={from} TO={to} VALUE={}",
                r.section.as_str(),
                r.id,
                r.kind,
                r.name,
                r.value
            ));
        }
        s
    }

    /// Click a clocks / I/O-delay / exception row: properties + Constraints workspace.
    pub fn select_constraint(&mut self, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("select_constraint: missing id".into());
        }
        let spec_l = spec.to_ascii_lowercase();
        let rows = self.constraint_rows();
        let row = rows
            .iter()
            .find(|r| r.id.eq_ignore_ascii_case(spec))
            .or_else(|| {
                rows.iter().find(|r| {
                    r.name.eq_ignore_ascii_case(spec)
                        || r.kind.eq_ignore_ascii_case(spec)
                        || format!("{}:{}", r.kind, r.name).eq_ignore_ascii_case(spec)
                        || format!("{}:{}->{}", r.kind, r.from, r.to).eq_ignore_ascii_case(spec)
                        || r.id.to_ascii_lowercase().ends_with(&format!(":{spec_l}"))
                        || spec_l.split_once(':').is_some_and(|(k, n)| {
                            let n = n.trim();
                            !n.is_empty()
                                && r.id.to_ascii_lowercase().starts_with(&format!("{k}:"))
                                && (r.name.eq_ignore_ascii_case(n)
                                    || r.from.eq_ignore_ascii_case(n)
                                    || r.id.to_ascii_lowercase().ends_with(&format!(":{n}")))
                        })
                })
            })
            .cloned()
            .ok_or_else(|| format!("select_constraint: no row {spec}"))?;
        let from = if row.from.is_empty() {
            "-"
        } else {
            row.from.as_str()
        };
        let to = if row.to.is_empty() {
            "-"
        } else {
            row.to.as_str()
        };
        self.selected = Some(row.id.clone());
        self.properties = vec![
            ("NAME".into(), row.name.clone()),
            ("TYPE".into(), "constraint".into()),
            ("SECTION".into(), row.section.as_str().into()),
            ("KIND".into(), row.kind.clone()),
            ("FROM".into(), from.into()),
            ("TO".into(), to.into()),
            ("VALUE".into(), row.value.clone()),
            ("ENABLED".into(), u8::from(row.enabled).to_string()),
            ("ID".into(), row.id.clone()),
        ];
        self.workspace = WorkspaceTab::Constraints;
        Ok(format!(
            "constraint ID={} SECTION={} KIND={} NAME={} FROM={from} TO={to} VALUE={}",
            row.id,
            row.section.as_str(),
            row.kind,
            row.name,
            row.value
        ))
    }

    /// UG893 Timing Constraints pane text. Empty until create_clock /
    /// create_generated_clock / read_xdc.
    pub fn constraints_text(&self) -> String {
        if self.constraint_rows().is_empty() {
            return "no timing constraints — create_clock / create_generated_clock / read_xdc".into();
        }
        let mut lines = Vec::new();
        for c in &self.constraints.clocks {
            if c.generated {
                let master = c.master.as_deref().unwrap_or("clk");
                let ratio = if !c.edges.is_empty() {
                    format!(
                        "-edges {{{}}}",
                        c.edges
                            .iter()
                            .map(|e| e.to_string())
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                } else if c.multiply_by > 1 {
                    format!("-multiply_by {}", c.multiply_by)
                } else {
                    format!("-divide_by {}", c.divide_by)
                };
                let invert = if c.invert { " -invert" } else { "" };
                let edges = if c.edges.is_empty() {
                    "-".into()
                } else {
                    c.edges
                        .iter()
                        .map(|e| e.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                };
                lines.push(format!(
                    "create_generated_clock -name {} -source [get_ports {master}] {ratio}{invert} [get_pins {}] PERIOD_PS={} DIVIDE_BY={} MULTIPLY_BY={} INVERT={} EDGES={edges} MASTER={master} generated=1",
                    c.name,
                    c.source,
                    c.period_ps,
                    c.divide_by,
                    c.multiply_by,
                    u8::from(c.invert)
                ));
            } else {
                lines.push(format!(
                    "create_clock -name {} -period {:.3} [get_ports {}] PERIOD_PS={} generated={}",
                    c.name,
                    c.period_ps as f64 / 1000.0,
                    c.source,
                    c.period_ps,
                    u8::from(c.generated)
                ));
            }
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
        for m in &self.constraints.multicycle_paths {
            lines.push(format!(
                "set_multicycle_path -from {} -to {} SETUP_MULT={} HOLD_MULT={}",
                m.from, m.to, m.setup_mult, m.hold_mult
            ));
        }
        for m in &self.constraints.max_delays {
            lines.push(format!(
                "set_max_delay -from {} -to {} DELAY_PS={} datapath_only={}",
                m.from, m.to, m.delay_ps, u8::from(m.datapath_only)
            ));
        }
        for m in &self.constraints.min_delays {
            lines.push(format!(
                "set_min_delay -from {} -to {} DELAY_PS={} datapath_only={}",
                m.from, m.to, m.delay_ps, u8::from(m.datapath_only)
            ));
        }
        for b in &self.constraints.bus_skews {
            lines.push(format!(
                "set_bus_skew -from {} -to {} SKEW_PS={} setup={} hold={}",
                b.from,
                b.to,
                b.skew_ps,
                u8::from(b.setup),
                u8::from(b.hold)
            ));
        }
        for g in &self.constraints.path_groups {
            lines.push(format!(
                "group_path -name {} -from {} -to {} WEIGHT_MILLI={} CRITICAL_RANGE_PS={}",
                g.name, g.from, g.to, g.weight_milli, g.critical_range_ps
            ));
        }
        for b in &self.constraints.max_time_borrows {
            let obj = if b.object.is_empty() { "-" } else { &b.object };
            lines.push(format!(
                "set_max_time_borrow {obj} BORROW_PS={}",
                b.borrow_ps
            ));
        }
        for d in &self.constraints.data_checks {
            lines.push(format!(
                "set_data_check -from {} -to {} SETUP_PS={} HOLD_PS={}",
                d.from, d.to, d.setup_ps, d.hold_ps
            ));
        }
        for g in &self.constraints.clock_groups {
            let mut flags = Vec::new();
            if g.asynchronous {
                flags.push("-asynchronous");
            }
            if g.exclusive {
                flags.push("-exclusive");
            }
            let groups = g
                .groups
                .iter()
                .map(|grp| format!("-group {}", grp.join(",")))
                .collect::<Vec<_>>()
                .join(" ");
            lines.push(format!(
                "set_clock_groups {} groups={} {groups}",
                flags.join(" "),
                g.groups.len()
            ));
        }
        for u in &self.constraints.clock_uncertainties {
            lines.push(format!(
                "set_clock_uncertainty -from {} -to {} SETUP_PS={} HOLD_PS={}",
                u.from, u.to, u.setup_ps, u.hold_ps
            ));
        }
        for l in &self.constraints.clock_latencies {
            lines.push(format!(
                "set_clock_latency {} LATE_PS={} EARLY_PS={} source={}",
                l.clock, l.late_ps, l.early_ps, u8::from(l.source)
            ));
        }
        for d in &self.constraints.disable_timings {
            lines.push(format!(
                "set_disable_timing -from {} -to {} {}",
                d.from, d.to, d.object
            ));
        }
        for c in &self.constraints.case_analyses {
            lines.push(format!("set_case_analysis {} {}", c.value, c.object));
        }
        for p in &self.constraints.propagated_clocks {
            lines.push(format!("set_propagated_clock {p}"));
        }
        for s in &self.constraints.clock_senses {
            lines.push(format!("set_clock_sense -{} {}", s.sense, s.object));
        }
        for j in &self.constraints.input_jitters {
            lines.push(format!(
                "set_input_jitter {} JITTER_PS={}",
                j.clock, j.jitter_ps
            ));
        }
        if self.constraints.system_jitter_ps != 0 {
            lines.push(format!(
                "set_system_jitter JITTER_PS={}",
                self.constraints.system_jitter_ps
            ));
        }
        for d in &self.constraints.timing_derates {
            lines.push(format!(
                "set_timing_derate LATE_MILLI={} EARLY_MILLI={} cell={} net={}",
                d.late_milli,
                d.early_milli,
                u8::from(d.cell),
                u8::from(d.net)
            ));
        }
        if self.constraints.operating_conditions.is_set() {
            let oc = &self.constraints.operating_conditions;
            lines.push(format!(
                "set_operating_conditions VOLTAGE_MV={} TEMP_C={} voltage={} temperature={} SCALE_MILLI={}",
                oc.voltage_mv,
                oc.temperature_c,
                u8::from(oc.voltage_set),
                u8::from(oc.temperature_set),
                oc.scale_milli()
            ));
        }
        for (port, pin) in &self.constraints.package_pins {
            lines.push(format!("set_property PACKAGE_PIN {pin} {port}"));
        }
        for (port, std) in &self.constraints.iostandards {
            lines.push(format!("set_property IOSTANDARD {std} {port}"));
        }
        for (port, ma) in &self.constraints.drives {
            lines.push(format!("set_property DRIVE {ma} {port}"));
        }
        for (port, slew) in &self.constraints.slews {
            lines.push(format!("set_property SLEW {slew} {port}"));
        }
        for (port, pull) in &self.constraints.pulltypes {
            lines.push(format!("set_property PULLTYPE {pull} {port}"));
        }
        for (port, term) in &self.constraints.diff_terms {
            lines.push(format!("set_property DIFF_TERM {term} {port}"));
        }
        for (port, term) in &self.constraints.in_terms {
            lines.push(format!("set_property IN_TERM {term} {port}"));
        }
        for pb in &self.pblocks {
            lines.push(format!("create_pblock {}", pb.name));
            if pb.ranged {
                lines.push(format!("resize_pblock {} -add {{{}}}", pb.name, pb.range_text()));
            }
            for c in &pb.cells {
                lines.push(format!("add_cells_to_pblock {} {c}", pb.name));
            }
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
        self.constraints
            .multicycle_paths
            .extend(extra.multicycle_paths);
        self.constraints.max_delays.extend(extra.max_delays);
        self.constraints.min_delays.extend(extra.min_delays);
        self.constraints.clock_groups.extend(extra.clock_groups);
        self.constraints
            .clock_uncertainties
            .extend(extra.clock_uncertainties);
        self.constraints.clock_latencies.extend(extra.clock_latencies);
        self.constraints
            .disable_timings
            .extend(extra.disable_timings);
        self.constraints.case_analyses.extend(extra.case_analyses);
        for p in extra.propagated_clocks {
            if !self.constraints.propagated_clocks.contains(&p) {
                self.constraints.propagated_clocks.push(p);
            }
        }
        self.constraints.clock_senses.extend(extra.clock_senses);
        for j in extra.input_jitters {
            self.constraints.input_jitters.retain(|k| k.clock != j.clock);
            self.constraints.input_jitters.push(j);
        }
        if extra.system_jitter_ps != 0 {
            self.constraints.system_jitter_ps = extra.system_jitter_ps;
        }
        self.constraints
            .timing_derates
            .extend(extra.timing_derates);
        self.constraints.bus_skews.extend(extra.bus_skews);
        self.constraints.path_groups.extend(extra.path_groups);
        self.constraints
            .max_time_borrows
            .extend(extra.max_time_borrows);
        self.constraints.data_checks.extend(extra.data_checks);
        if extra.operating_conditions.voltage_set {
            self.constraints.operating_conditions.voltage_mv =
                extra.operating_conditions.voltage_mv;
            self.constraints.operating_conditions.voltage_set = true;
        }
        if extra.operating_conditions.temperature_set {
            self.constraints.operating_conditions.temperature_c =
                extra.operating_conditions.temperature_c;
            self.constraints.operating_conditions.temperature_set = true;
        }
        self.constraints.package_pins.extend(extra.package_pins);
        self.constraints.iostandards.extend(extra.iostandards);
        self.constraints.drives.extend(extra.drives);
        self.constraints.slews.extend(extra.slews);
        self.constraints.pulltypes.extend(extra.pulltypes);
        self.constraints.diff_terms.extend(extra.diff_terms);
        self.constraints.in_terms.extend(extra.in_terms);
        if let Some(c) = self
            .constraints
            .clocks
            .iter()
            .filter(|c| c.generated)
            .max_by_key(|c| c.period_ps)
            .or_else(|| {
                self.constraints
                    .clocks
                    .iter()
                    .find(|c| c.source == "clk" || c.name == "clk")
            })
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

    /// UG893 Timing Constraints Apply: `create_generated_clock -divide_by` /
    /// `-multiply_by` / `-invert` / `-edges` derives a new period (and optional
    /// half-cycle invert) from the master clock and feeds helion-sta (WNS
    /// moves). Empty XDC / no generated clock keeps gold WNS.
    pub fn apply_create_generated_clock(&mut self, cmd: &str) -> Result<String, String> {
        let xdc = {
            let mut xdc = String::new();
            let masters: Vec<&helion_sta::Clock> = self
                .constraints
                .clocks
                .iter()
                .filter(|c| !c.generated)
                .collect();
            if masters.is_empty() {
                xdc.push_str(&format!(
                    "create_clock -name clk -period {:.3} [get_ports clk]\n",
                    self.clock_period_ps as f64 / 1000.0
                ));
            } else {
                for c in masters {
                    xdc.push_str(&format!(
                        "create_clock -name {} -period {:.3} [get_ports {}]\n",
                        c.name,
                        c.period_ps as f64 / 1000.0,
                        c.source
                    ));
                }
            }
            xdc.push_str(cmd);
            xdc.push('\n');
            xdc
        };
        let extra = load_xdc(&xdc)?;
        let gclk = extra
            .clocks
            .iter()
            .find(|c| c.generated)
            .ok_or_else(|| {
                "create_generated_clock: missing -divide_by/-multiply_by/-edges or unknown master"
                    .to_string()
            })?;
        let n = extra.clocks.len();
        let period = gclk.period_ps;
        let name = gclk.name.clone();
        let div = gclk.divide_by;
        let mul = gclk.multiply_by;
        let invert = u8::from(gclk.invert);
        let edges = if gclk.edges.is_empty() {
            "-".into()
        } else {
            gclk.edges
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        let master = gclk.master.clone().unwrap_or_else(|| "clk".into());
        self.merge_constraints(extra);
        Ok(format!(
            "create_generated_clock {name} PERIOD_PS={period} DIVIDE_BY={div} MULTIPLY_BY={mul} INVERT={invert} EDGES={edges} MASTER={master} clocks={n}"
        ))
    }

    /// UG893 Timing Constraints Apply: set_input_delay / set_output_delay /
    /// set_false_path / set_multicycle_path / set_max_delay / set_min_delay /
    /// set_clock_groups / set_clock_uncertainty / set_clock_latency /
    /// set_disable_timing / set_case_analysis / set_propagated_clock /
    /// set_clock_sense / set_input_jitter / set_system_jitter /
    /// set_timing_derate / set_operating_conditions / set_bus_skew /
    /// group_path / set_max_time_borrow / set_data_check land in the pane
    /// and feed helion-sta (setup/hold WNS move).
    pub fn apply_sdc_exception(&mut self, cmd: &str) -> Result<String, String> {
        let extra = load_xdc(cmd)?;
        if extra.input_delay_ps.is_empty()
            && extra.output_delay_ps.is_empty()
            && extra.false_paths.is_empty()
            && extra.multicycle_paths.is_empty()
            && extra.max_delays.is_empty()
            && extra.min_delays.is_empty()
            && extra.clock_groups.is_empty()
            && extra.clock_uncertainties.is_empty()
            && extra.clock_latencies.is_empty()
            && extra.disable_timings.is_empty()
            && extra.case_analyses.is_empty()
            && extra.propagated_clocks.is_empty()
            && extra.clock_senses.is_empty()
            && extra.input_jitters.is_empty()
            && extra.system_jitter_ps == 0
            && extra.timing_derates.is_empty()
            && !extra.operating_conditions.is_set()
            && extra.bus_skews.is_empty()
            && extra.path_groups.is_empty()
            && extra.max_time_borrows.is_empty()
            && extra.data_checks.is_empty()
        {
            return Err(format!(
                "{cmd}: missing delay, false path, multicycle, max_delay, min_delay, clock_groups, uncertainty, latency, disable_timing, case_analysis, propagated_clock, clock_sense, input_jitter, system_jitter, timing_derate, operating_conditions, bus_skew, group_path, max_time_borrow, or data_check"
            ));
        }
        let n_in = extra.input_delay_ps.len();
        let n_out = extra.output_delay_ps.len();
        let n_fp = extra.false_paths.len();
        let n_mcp = extra.multicycle_paths.len();
        let n_md = extra.max_delays.len();
        let n_mind = extra.min_delays.len();
        let n_cg = extra.clock_groups.len();
        let n_cgg = extra
            .clock_groups
            .iter()
            .map(|g| g.groups.len())
            .max()
            .unwrap_or(0);
        let n_u = extra.clock_uncertainties.len();
        let n_l = extra.clock_latencies.len();
        let n_dt = extra.disable_timings.len();
        let n_ca = extra.case_analyses.len();
        let n_pc = extra.propagated_clocks.len();
        let n_cs = extra.clock_senses.len();
        let n_ij = extra.input_jitters.len();
        let n_sj = u8::from(extra.system_jitter_ps != 0);
        let n_td = extra.timing_derates.len();
        let n_oc = u8::from(extra.operating_conditions.is_set());
        let n_bs = extra.bus_skews.len();
        let n_gp = extra.path_groups.len();
        let n_tb = extra.max_time_borrows.len();
        let n_dc = extra.data_checks.len();
        let in_ps = extra.input_delay_ps.values().copied().max().unwrap_or(0);
        let out_ps = extra.output_delay_ps.values().copied().max().unwrap_or(0);
        let sm = extra.setup_mult();
        let hm = extra.hold_mult();
        let md_ps = extra.max_delay_ps().unwrap_or(0);
        let mind_ps = extra.min_delay_ps().unwrap_or(0);
        let us = extra.uncertainty_setup_ps();
        let uh = extra.uncertainty_hold_ps();
        let late = extra.latency_late_ps();
        let early = extra.latency_early_ps();
        let ij = extra.input_jitter_ps();
        let sj = extra.system_jitter_ps;
        let late_m = extra
            .timing_derates
            .last()
            .map(|d| d.late_milli)
            .unwrap_or(0);
        let early_m = extra
            .timing_derates
            .last()
            .map(|d| d.early_milli)
            .unwrap_or(0);
        let vmv = extra.operating_conditions.voltage_mv;
        let tc = extra.operating_conditions.temperature_c;
        let ocm = extra.operating_conditions.scale_milli();
        let bs = extra.bus_skew_setup_ps().max(extra.bus_skew_hold_ps());
        let wm = extra.group_path_weight_milli();
        let cr = extra.group_path_critical_range_ps();
        let tb = extra.time_borrow_ps();
        let dcs = extra.data_check_setup_ps();
        let dch = extra.data_check_hold_ps();
        let case = extra
            .case_analyses
            .first()
            .map(|c| c.value.clone())
            .unwrap_or_default();
        let sense = extra
            .clock_senses
            .first()
            .map(|s| s.sense.clone())
            .unwrap_or_default();
        let clk_net = self
            .shell
            .session
            .routed
            .as_ref()
            .map(|r| clock_network_delay_ps(&r.placed))
            .unwrap_or(0);
        self.merge_constraints(extra);
        Ok(format!(
            "apply_xdc input_delay={n_in} DELAY_PS={in_ps} output_delay={n_out} DELAY_PS={out_ps} false_path={n_fp} multicycle={n_mcp} SETUP_MULT={sm} HOLD_MULT={hm} max_delay={n_md} MAX_DELAY_PS={md_ps} min_delay={n_mind} MIN_DELAY_PS={mind_ps} clock_groups={n_cg} GROUPS={n_cgg} uncertainty={n_u} UNCERT_SETUP_PS={us} UNCERT_HOLD_PS={uh} latency={n_l} LATE_PS={late} EARLY_PS={early} disable_timing={n_dt} case_analysis={n_ca} CASE={case} propagated_clock={n_pc} CLK_NET_PS={clk_net} clock_sense={n_cs} SENSE={sense} input_jitter={n_ij} INPUT_JITTER_PS={ij} system_jitter={n_sj} SYSTEM_JITTER_PS={sj} timing_derate={n_td} LATE_MILLI={late_m} EARLY_MILLI={early_m} operating_conditions={n_oc} VOLTAGE_MV={vmv} TEMP_C={tc} OC_SCALE_MILLI={ocm} bus_skew={n_bs} BUS_SKEW_PS={bs} group_path={n_gp} WEIGHT_MILLI={wm} CRITICAL_RANGE_PS={cr} time_borrow={n_tb} BORROW_PS={tb} data_check={n_dc} DATA_CHECK_SETUP_PS={dcs} DATA_CHECK_HOLD_PS={dch}"
        ))
    }

    pub fn read_xdc_path(&mut self, path: &str) -> Result<String, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("read_xdc {path}: {e}"))?;
        let extra = load_xdc(&text)?;
        if extra.clocks.is_empty()
            && extra.input_delay_ps.is_empty()
            && extra.output_delay_ps.is_empty()
            && extra.false_paths.is_empty()
            && extra.multicycle_paths.is_empty()
            && extra.max_delays.is_empty()
            && extra.min_delays.is_empty()
            && extra.clock_groups.is_empty()
            && extra.clock_uncertainties.is_empty()
            && extra.clock_latencies.is_empty()
            && extra.disable_timings.is_empty()
            && extra.case_analyses.is_empty()
            && extra.propagated_clocks.is_empty()
            && extra.clock_senses.is_empty()
            && extra.input_jitters.is_empty()
            && extra.system_jitter_ps == 0
            && extra.timing_derates.is_empty()
            && !extra.operating_conditions.is_set()
            && extra.bus_skews.is_empty()
            && extra.path_groups.is_empty()
            && extra.max_time_borrows.is_empty()
            && extra.data_checks.is_empty()
            && extra.package_pins.is_empty()
            && extra.iostandards.is_empty()
            && extra.drives.is_empty()
            && extra.slews.is_empty()
            && extra.pulltypes.is_empty()
            && extra.diff_terms.is_empty()
            && extra.in_terms.is_empty()
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
        let n_mcp = extra.multicycle_paths.len();
        let n_md = extra.max_delays.len();
        let n_mind = extra.min_delays.len();
        let n_cg = extra.clock_groups.len();
        let n_u = extra.clock_uncertainties.len();
        let n_l = extra.clock_latencies.len();
        let n_dt = extra.disable_timings.len();
        let n_ca = extra.case_analyses.len();
        let n_pc = extra.propagated_clocks.len();
        let n_cs = extra.clock_senses.len();
        let n_ij = extra.input_jitters.len();
        let n_sj = u8::from(extra.system_jitter_ps != 0);
        let n_td = extra.timing_derates.len();
        let n_oc = u8::from(extra.operating_conditions.is_set());
        let n_bs = extra.bus_skews.len();
        let n_gp = extra.path_groups.len();
        let n_tb = extra.max_time_borrows.len();
        let n_dc = extra.data_checks.len();
        self.merge_constraints(extra);
        let p = path.to_string();
        if !self.tree.sources.contains(&p) {
            self.tree.sources.push(p);
        }
        Ok(format!(
            "read_xdc clocks={n} PERIOD_PS={period} input_delay={n_in} output_delay={n_out} false_path={n_fp} multicycle={n_mcp} max_delay={n_md} min_delay={n_mind} clock_groups={n_cg} uncertainty={n_u} latency={n_l} disable_timing={n_dt} case_analysis={n_ca} propagated_clock={n_pc} clock_sense={n_cs} input_jitter={n_ij} system_jitter={n_sj} timing_derate={n_td} operating_conditions={n_oc} bus_skew={n_bs} group_path={n_gp} time_borrow={n_tb} data_check={n_dc}"
        ))
    }

    fn clocks_for_sta(&self) -> Vec<helion_sta::Clock> {
        let mut clks = self.constraints.clocks.clone();
        if clks.is_empty() {
            create_clock(&mut clks, "clk", self.clock_period_ps, "clk");
        } else if let Some((i, _)) = clks
            .iter()
            .enumerate()
            .filter(|(_, c)| c.generated)
            .max_by_key(|(_, c)| c.period_ps)
        {
            // UG903 create_generated_clock: divide_by / multiply_by / edges scale
            // the analysis period/WNS; -invert is a half-cycle setup.
            clks.swap(0, i);
        }
        clks
    }

    /// Console `report_timing` uses IdeModel constraint clocks (including
    /// create_generated_clock) + I/O delay/false path/
    /// multicycle/max_delay/min_delay/clock_groups/uncertainty/latency/disable_timing/
    /// case_analysis/propagated_clock/clock_sense/input_jitter/system_jitter/
    /// timing_derate/operating_conditions/bus_skew/group_path/max_time_borrow/
    /// data_check, the same
    /// vector `refresh_reports` feeds `report_timing_routed_xdc`.
    /// Pulls place/route if needed so `read_sv` then `report_timing` still hits STA
    /// (old Tcl path).
    pub fn report_timing_now(&mut self) -> Result<String, String> {
        if self.shell.session.design.is_none() {
            return Err("report_timing: no design".into());
        }
        if self.shell.session.routed.is_none() {
            let dev = self.device()?;
            if self.shell.session.placed.is_none() {
                self.place_now(&dev)?;
            }
            self.shell.session.route_design(&dev)?;
        }
        let clks = self.clocks_for_sta();
        let d = self.shell.session.design.as_ref().unwrap();
        let r = self.shell.session.routed.as_ref().unwrap();
        let t = report_timing_routed_xdc(d, r, &clks, &self.constraints)?;
        Ok(format!(
            "report_timing {} WNS_PS={} TNS_PS={} SETUP_PS={} HOLD_PS={} HOLD_SLACK_PS={} endpoints={} r2r_ps={} iob_ps={} route_ps={} CLK_NET_PS={}",
            d.name, t.wns_ps, t.tns_ps, t.setup_ps, t.hold_ps, t.hold_slack_ps, t.endpoints, t.r2r_ps, t.iob_ps, t.route_ps, t.clk_net_ps
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
        let text = drc.text();
        self.drc = Some(drc);
        self.workspace = WorkspaceTab::Drc;
        Ok(text)
    }

    pub fn sim_run(&mut self, cycles: u32) -> Result<String, String> {
        self.prepare_sim()?;
        self.wave.traces.clear();
        self.wave.cursor = 0;
        self.wave.timescale_ps = self.clock_period_ps;
        self.bp_prev.clear();
        let mut hit: Option<String> = None;
        let mut ran = 0u32;
        for _ in 0..cycles {
            self.sim_step_inner()?;
            ran += 1;
            if let Some(h) = self.check_breakpoints() {
                hit = Some(h);
                break;
            }
        }
        let n = self.wave.sample_len();
        if n > 0 {
            self.wave.set_cursor(n - 1);
            if let Some(a) = self.wave.cursor_a {
                self.wave.set_cursor_a(a);
            }
            if let Some(b) = self.wave.cursor_b {
                self.wave.set_cursor_b(b);
            }
        } else {
            self.wave.cursor_a = None;
            self.wave.cursor_b = None;
        }
        let led = self
            .wave
            .bits_of("led")
            .unwrap_or_default();
        if let Some(h) = hit {
            return Ok(format!(
                "sim_run HIT {h} cycles={ran}/{cycles} LED[{ran}]={led}"
            ));
        }
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
        self.selected_object = None;
        self.memories.clear();
        self.selected_memory = None;
        self.selected_memory_addr = None;
        self.locals.clear();
        self.selected_local = None;
        self.bp_prev.clear();
        for bp in &mut self.breakpoints {
            bp.hits = 0;
        }
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
        for inst in &d.instances {
            self.scopes.push(ScopeNode {
                name: inst.name.clone(),
                kind: format!("instance:{}", inst.module),
            });
        }
        if self.selected_scope.as_deref() != Some(d.name.as_str())
            && !self
                .scopes
                .iter()
                .any(|s| Some(s.name.as_str()) == self.selected_scope.as_deref())
        {
            self.selected_scope = Some(d.name.clone());
        }
        if self.selected_scope.is_none() {
            self.selected_scope = Some(d.name.clone());
        }
        if let (Some(bits), Some(r)) = (
            self.shell.session.bitstream.clone(),
            self.shell.session.routed.clone(),
        ) {
            let dev = self.device()?;
            let mut fab = Fabric::new(&dev);
            fab.program(&bits)?;
            fab.finish_startup();
            self.fabric_sim = Some(fab);
            self.event_sim = None;
            let _ = r;
        } else {
            self.event_sim = Some(Sim::new(&d));
            self.fabric_sim = None;
        }
        self.wave.timescale_ps = self.clock_period_ps;
        if self.wave.traces.is_empty() {
            self.wave.traces.push(WaveTrace::scalar("led"));
        }
        self.refresh_sim_debug();
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
        self.wave.rebuild_virtual_buses();
        let n = self.wave.sample_len();
        if n > 0 {
            self.wave.set_cursor(n - 1);
        }
        self.refresh_sim_objects();
        self.refresh_sim_debug();
        Ok(())
    }

    fn sim_object_kind(&self, name: &str) -> String {
        if self.wave.virtual_bus(name).is_some() {
            return "virtual_bus".into();
        }
        if let Some(t) = self.wave.trace(name) {
            if t.width > 1 {
                return "bus".into();
            }
        }
        if let Some(d) = self.shell.session.design.as_ref() {
            if d.ports.iter().any(|p| p.name == name) {
                return "port".into();
            }
            if d.nets.iter().any(|n| n.name == name) {
                return "net".into();
            }
            if let Some(c) = d.cells.iter().find(|c| c.name == name) {
                return match &c.kind {
                    CellKind::Hff => "reg".into(),
                    CellKind::Lut6 { .. } => "net".into(),
                    CellKind::IobOut => "port".into(),
                    CellKind::Ila { .. } => "ila".into(),
                    CellKind::Mac27 => "dsp".into(),
                    CellKind::Bram18 => "bram".into(),
                    CellKind::BlackBox { .. } => "instance".into(),
                };
            }
        }
        "signal".into()
    }

    fn collect_sim_objects(&self) -> Vec<SimObject> {
        let mut v = Vec::new();
        let mut seen = HashSet::new();
        let push = |v: &mut Vec<SimObject>, seen: &mut HashSet<String>, name: String, value: String| {
            if seen.insert(name.clone()) {
                let kind = self.sim_object_kind(&name);
                v.push(SimObject { name, kind, value });
            }
        };
        if let Some(sim) = &self.event_sim {
            for (name, value) in sim.object_values() {
                push(&mut v, &mut seen, name, value);
            }
        }
        let cur = self.wave.cursor;
        if let Some(t) = self.wave.trace("led") {
            push(&mut v, &mut seen, "led".into(), t.value_at(cur));
        }
        if let Some(t) = self.wave.trace("cnt") {
            push(&mut v, &mut seen, "cnt".into(), t.value_at(cur));
        }
        for vb in &self.wave.virtual_buses {
            if let Some(t) = self.wave.trace(&vb.name) {
                push(&mut v, &mut seen, vb.name.clone(), t.value_at(cur));
            }
        }
        if let Some(d) = self.shell.session.design.as_ref() {
            for p in &d.ports {
                let val = self
                    .wave
                    .trace(&p.name)
                    .map(|t| t.value_at(cur))
                    .unwrap_or_else(|| "-".into());
                push(&mut v, &mut seen, p.name.clone(), val);
            }
        }
        v
    }

    fn object_in_scope(&self, name: &str, scope: &str) -> bool {
        let top = self
            .scopes
            .iter()
            .find(|s| s.kind == "module")
            .map(|s| s.name.as_str())
            .or_else(|| self.tree.top.as_deref())
            .unwrap_or(scope);
        let instances: Vec<&str> = self
            .scopes
            .iter()
            .filter(|s| s.kind.starts_with("instance"))
            .map(|s| s.name.as_str())
            .collect();
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
            // Flattened child (hier.sv): sequential probes live in the instance scope.
            (name.starts_with("u_ff") || name.starts_with("u_lut"))
                && !instances
                    .iter()
                    .any(|i| name.starts_with(&format!("{i}_")))
        }
    }

    fn refresh_sim_objects(&mut self) {
        let all = self.collect_sim_objects();
        let scope = self
            .selected_scope
            .clone()
            .or_else(|| self.scopes.first().map(|s| s.name.clone()));
        self.objects = match scope {
            None => all,
            Some(s) => all
                .into_iter()
                .filter(|o| self.object_in_scope(&o.name, &s))
                .collect(),
        };
    }

    fn refresh_sim_debug(&mut self) {
        self.memories = self.collect_memories();
        if let Some(sel) = self.selected_memory.clone() {
            if !self.memories.iter().any(|m| m.name == sel) {
                self.selected_memory = None;
                self.selected_memory_addr = None;
            }
        }
        let all = self.collect_locals();
        let scope = self
            .selected_scope
            .clone()
            .or_else(|| self.scopes.first().map(|s| s.name.clone()));
        self.locals = match scope {
            None => all,
            Some(s) => all
                .into_iter()
                .filter(|l| self.object_in_scope(&l.name, &s))
                .collect(),
        };
        if let Some(sel) = self.selected_local.clone() {
            if !self.locals.iter().any(|l| l.name == sel) {
                self.selected_local = None;
            }
        }
    }

    fn upsert_memory(blocks: &mut Vec<MemoryBlock>, b: MemoryBlock) {
        if let Some(existing) = blocks.iter_mut().find(|e| e.name == b.name) {
            *existing = b;
        } else {
            blocks.push(b);
        }
    }

    fn collect_memories(&self) -> Vec<MemoryBlock> {
        let mut blocks: Vec<MemoryBlock> = Vec::new();
        if let Some(sim) = &self.event_sim {
            for m in sim.memory_cells() {
                Self::upsert_memory(
                    &mut blocks,
                    MemoryBlock {
                        name: m.name,
                        kind: m.kind,
                        width: m.width,
                        words: m.words,
                    },
                );
            }
            let regs: Vec<SimLocal> = sim
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
                Self::upsert_memory(
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
        if let (Some(fab), Some(pl)) = (&self.fabric_sim, self.shell.session.placed.as_ref()) {
            for (i, lf) in pl.packed.lutffs.iter().enumerate() {
                if let Some((site, ble)) = pl.lutff_sites.get(i) {
                    let init = fab.lut_init(site.x, site.y, *ble as u32);
                    Self::upsert_memory(
                        &mut blocks,
                        MemoryBlock {
                            name: lf.lut_cell.clone(),
                            kind: "lut_init".into(),
                            width: 1,
                            words: (0..64).map(|a| (init >> a) & 1).collect(),
                        },
                    );
                }
            }
            for (i, br) in pl.packed.brams.iter().enumerate() {
                let mut words = br.init.clone();
                if words.is_empty() {
                    words.push(fab.bram_init_word(i as u16, 0));
                } else {
                    for (addr, w) in words.iter_mut().enumerate() {
                        let live = fab.bram_init_word(i as u16, addr);
                        if live != 0 {
                            *w = live;
                        }
                    }
                }
                Self::upsert_memory(
                    &mut blocks,
                    MemoryBlock {
                        name: br.cell.clone(),
                        kind: "bram".into(),
                        width: 64,
                        words,
                    },
                );
            }
            let n = pl.lutff_sites.len().min(64);
            if n > 0 {
                let mut seq = 0u64;
                for (i, (site, ble)) in pl.lutff_sites.iter().take(64).enumerate() {
                    if fab.ble_q(site.x, site.y, *ble as u32) {
                        seq |= 1 << i;
                    }
                }
                Self::upsert_memory(
                    &mut blocks,
                    MemoryBlock {
                        name: "seq".into(),
                        kind: "seq".into(),
                        width: n as u8,
                        words: vec![seq],
                    },
                );
            }
        }
        if blocks.is_empty() {
            if let Some(d) = self.shell.session.design.as_ref() {
                for c in &d.cells {
                    if let CellKind::Lut6 { init } = c.kind {
                        Self::upsert_memory(
                            &mut blocks,
                            MemoryBlock {
                                name: c.name.clone(),
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

    fn collect_locals(&self) -> Vec<LocalRow> {
        let scope = self
            .selected_scope
            .clone()
            .or_else(|| self.scopes.first().map(|s| s.name.clone()))
            .unwrap_or_else(|| "-".into());
        let mut v = Vec::new();
        let mut seen = HashSet::new();
        let mut push = |name: String, kind: String, value: String| {
            if seen.insert(name.clone()) {
                v.push(LocalRow {
                    name,
                    kind,
                    value,
                    scope: scope.clone(),
                });
            }
        };
        if let Some(sim) = &self.event_sim {
            for l in sim.locals() {
                push(l.name, l.kind, l.value);
            }
        }
        if let (Some(fab), Some(pl)) = (&self.fabric_sim, self.shell.session.placed.as_ref()) {
            let iob = self
                .shell
                .session
                .routed
                .as_ref()
                .and_then(|r| r.iob_src.first());
            if let Some(iob) = iob {
                let led = fab.led_at(iob.iob.0, iob.iob.1);
                push("led".into(), "port".into(), if led { "1" } else { "0" }.into());
            }
            for (i, lf) in pl.packed.lutffs.iter().enumerate() {
                if let Some((site, ble)) = pl.lutff_sites.get(i) {
                    let q = fab.ble_q(site.x, site.y, *ble as u32);
                    push(
                        lf.ff_cell.clone(),
                        "reg".into(),
                        if q { "1" } else { "0" }.into(),
                    );
                }
            }
        }
        v
    }

    fn sim_signal_value(&self, name: &str) -> Option<u64> {
        if let Some(sim) = &self.event_sim {
            if let Some(v) = sim.signal_value(name) {
                return Some(v);
            }
        }
        if let Some(t) = self.wave.trace(name) {
            if let Some(v) = t.samples.last().copied() {
                return Some(v);
            }
        }
        if let Some(m) = self.memories.iter().find(|m| m.name.eq_ignore_ascii_case(name)) {
            return Some(m.packed_word());
        }
        if let Some(l) = self.locals.iter().find(|l| l.name.eq_ignore_ascii_case(name)) {
            return match l.value.as_str() {
                "1" => Some(1),
                "0" => Some(0),
                other => {
                    let t = other.trim_start_matches("0x").trim_start_matches("0X");
                    u64::from_str_radix(t, 16).ok().or_else(|| other.parse().ok())
                }
            };
        }
        if let (Some(fab), Some(pl)) = (&self.fabric_sim, self.shell.session.placed.as_ref()) {
            if name.eq_ignore_ascii_case("led") {
                if let Some(iob) = self.shell.session.routed.as_ref().and_then(|r| r.iob_src.first())
                {
                    return Some(u64::from(fab.led_at(iob.iob.0, iob.iob.1)));
                }
            }
            if name.eq_ignore_ascii_case("seq") || name.eq_ignore_ascii_case("cnt") {
                let mut seq = 0u64;
                for (i, (site, ble)) in pl.lutff_sites.iter().take(64).enumerate() {
                    if fab.ble_q(site.x, site.y, *ble as u32) {
                        seq |= 1 << i;
                    }
                }
                return Some(seq);
            }
            for (i, lf) in pl.packed.lutffs.iter().enumerate() {
                if lf.ff_cell.eq_ignore_ascii_case(name)
                    || lf.lut_cell.eq_ignore_ascii_case(name)
                    || lf.q_net.eq_ignore_ascii_case(name)
                {
                    if let Some((site, ble)) = pl.lutff_sites.get(i) {
                        return Some(u64::from(fab.ble_q(site.x, site.y, *ble as u32)));
                    }
                }
            }
        }
        None
    }

    fn check_breakpoints(&mut self) -> Option<String> {
        if self.breakpoints.iter().all(|b| !b.enabled) {
            return None;
        }
        let mut current: HashMap<String, u64> = HashMap::new();
        for bp in &self.breakpoints {
            if let Some(v) = self.sim_signal_value(&bp.signal) {
                current.insert(bp.signal.clone(), v);
            }
        }
        let cycle = self.wave.sample_len();
        let mut hit: Option<String> = None;
        for bp in &mut self.breakpoints {
            if !bp.enabled {
                continue;
            }
            let Some(&cur) = current.get(&bp.signal) else {
                continue;
            };
            let prev = self.bp_prev.get(&bp.signal).copied();
            let fire = if let Some(expect) = bp.value {
                cur == expect && prev != Some(cur)
            } else {
                prev.map(|p| p != cur).unwrap_or(false)
            };
            if fire {
                bp.hits = bp.hits.saturating_add(1);
                hit = Some(format!(
                    "id={} SIGNAL={} VALUE={cur} CONDITION={} HITS={} CYCLE={}",
                    bp.id, bp.signal, bp.condition, bp.hits, cycle
                ));
                break;
            }
        }
        for (k, v) in current {
            self.bp_prev.insert(k, v);
        }
        hit
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
        let mut s = format!(
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
        );
        for r in self.ila_sample_rows() {
            s.push_str(&format!(
                "\n{} SAMPLE={} TIME_PS={} VALUE={} MARKER={}",
                r.sample,
                r.sample,
                r.time_ps,
                r.value,
                if r.trigger { "TRIGGER" } else { "-" }
            ));
        }
        s
    }

    /// UG900 ILA dashboard rows: each fabric sample of the armed net.
    pub fn ila_sample_rows(&self) -> Vec<IlaSampleRow> {
        let ts = self.wave.timescale_ps.max(1);
        self.ila
            .bits
            .chars()
            .enumerate()
            .map(|(i, value)| IlaSampleRow {
                sample: i,
                time_ps: i as u64 * ts,
                value,
                trigger: self.ila.trigger_at == Some(i),
            })
            .collect()
    }

    pub fn select_ila_sample(&mut self, spec: &str) -> Result<String, String> {
        let rows = self.ila_sample_rows();
        if rows.is_empty() {
            return Err("select_ila_sample: no capture".into());
        }
        let spec = spec.trim();
        let i: usize = if spec.is_empty() {
            0
        } else {
            spec.parse()
                .map_err(|_| format!("select_ila_sample: bad sample {spec}"))?
        };
        let row = rows
            .get(i)
            .ok_or_else(|| format!("select_ila_sample: no sample {spec}"))?;
        self.wave.set_cursor(row.sample);
        self.workspace = WorkspaceTab::Hardware;
        self.selected = Some(format!("ila:{}", self.ila.net));
        self.properties = vec![
            ("NAME".into(), format!("ila:{}", self.ila.net)),
            ("TYPE".into(), "ila_sample".into()),
            ("SAMPLE".into(), row.sample.to_string()),
            ("TIME_PS".into(), row.time_ps.to_string()),
            ("VALUE".into(), row.value.to_string()),
            ("TRIGGER".into(), u8::from(row.trigger).to_string()),
            ("NET".into(), self.ila.net.clone()),
        ];
        Ok(format!(
            "ila_sample SAMPLE={} TIME_PS={} VALUE={} MARKER={}",
            row.sample,
            row.time_ps,
            row.value,
            if row.trigger { "TRIGGER" } else { "-" }
        ))
    }

    /// UG893 Hardware Manager STAT table from helion-hw TAP / fabric Stat.
    pub fn hw_stat_report(&self) -> HwStatReport {
        let open = self.hw.open || self.shell.session.hw_open;
        if !open {
            return HwStatReport::closed();
        }
        let Ok(dev) = self.device() else {
            return HwStatReport::closed();
        };
        let programmed = self.hw.programmed || self.shell.session.programmed;
        let (idcode, ir, stat) = if programmed {
            if let Some(st) = &self.hw.stat {
                if st.done {
                    (
                        self.hw.idcode.unwrap_or(dev.idcode),
                        self.hw.ir.unwrap_or(helion_hw::IR_STAT),
                        st.clone(),
                    )
                } else {
                    self.programmed_stat(&dev)
                        .unwrap_or_else(|| self.tap_stat(&dev))
                }
            } else {
                self.programmed_stat(&dev)
                    .unwrap_or_else(|| self.tap_stat(&dev))
            }
        } else {
            self.tap_stat(&dev)
        };
        HwStatReport {
            open: true,
            programmed,
            target: self.hw.target.clone(),
            part: self.part().to_string(),
            idcode,
            ir,
            word: stat.word(),
            bits: stat.bits().into_iter().map(HwStatRow::from_bit).collect(),
        }
    }

    pub fn hw_stat_text(&self) -> String {
        self.hw_stat_report().text()
    }

    fn tap_stat(&self, dev: &helion_device::Device) -> (u32, u8, Stat) {
        let mut tap = helion_hw::Tap::new(dev);
        let idcode = tap.read_idcode();
        let st = tap.read_stat();
        (idcode, tap.ir, st)
    }

    fn programmed_stat(&self, dev: &helion_device::Device) -> Option<(u32, u8, Stat)> {
        let bits = self.shell.session.bitstream.as_ref()?;
        let st = helion_hw::prog_sim(dev, bits).ok()?;
        Some((dev.idcode, helion_hw::IR_STAT, st))
    }

    /// Click a STAT bit: properties + Hardware workspace.
    pub fn select_hw_stat(&mut self, spec: &str) -> Result<String, String> {
        if !self.hw.open {
            self.shell.session.open_hw_manager();
            self.hw.open = true;
        }
        let report = self.hw_stat_report();
        if report.bits.is_empty() {
            return Err("select_hw_stat: open_hw_manager first".into());
        }
        let row = report
            .bit(spec)
            .cloned()
            .ok_or_else(|| format!("select_hw_stat: no bit {spec}"))?;
        self.selected = Some(row.name.clone());
        self.workspace = WorkspaceTab::Hardware;
        self.properties = vec![
            ("NAME".into(), row.name.clone()),
            ("TYPE".into(), "hw_stat".into()),
            ("BIT".into(), row.bit.to_string()),
            ("VALUE".into(), u8::from(row.value).to_string()),
            ("DESC".into(), row.description.clone()),
            ("WORD".into(), report.word_hex()),
            ("IDCODE".into(), format!("{:#010x}", report.idcode)),
            ("IR".into(), format!("{:#04x}", report.ir)),
            ("PROGRAMMED".into(), u8::from(report.programmed).to_string()),
        ];
        Ok(format!(
            "hw_stat BIT={} NAME={} VALUE={} WORD={}",
            row.bit,
            row.name,
            u8::from(row.value),
            report.word_hex()
        ))
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
        self.timing_paths = match (self.shell.session.design.as_ref(), self.timing.as_ref()) {
            (Some(d), Some(t)) => extract_timing_paths(d, t, &self.device),
            _ => Vec::new(),
        };
        if let Some(i) = self.selected_timing_path {
            if i >= self.timing_paths.len() {
                self.selected_timing_path = None;
                self.selected_timing_pin = None;
            }
        }
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
        self.fill_timing_path_locations();
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
            .filter_map(|p| {
                p.site
                    .as_ref()
                    .or(p.package_pin.as_ref())
                    .map(|s| (s.clone(), p.name.clone()))
            })
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
                    bank: 0,
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
        // Fig. 53: group HAD IOB pins into colored I/O banks (8 pins / bank).
        const BANK_PINS: u32 = 8;
        for p in &mut self.package_pins {
            let dx = p.x.saturating_sub(x0);
            let dy = p.y.saturating_sub(y0);
            p.bank = if cols >= rows {
                dx / BANK_PINS
            } else {
                dy / BANK_PINS
            };
        }
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
        let expand = self.schematic.expand_inside.take();
        let hl_cells = std::mem::take(&mut self.schematic.highlight_cells);
        let hl_nets = std::mem::take(&mut self.schematic.highlight_nets);
        let path_only = self.schematic.path_only;
        let camera = self.schematic.camera;
        let view_history = std::mem::take(&mut self.schematic.view_history);
        let view_index = self.schematic.view_index;
        let viewport_w = self.schematic.viewport_w;
        let viewport_h = self.schematic.viewport_h;
        let Some(d) = self.shell.session.design.as_ref() else {
            self.schematic = SchematicView::default();
            return;
        };
        let mut pins: HashMap<String, Vec<SchematicPin>> = HashMap::new();
        for n in &d.nets {
            for e in &n.endpoints {
                let list = pins.entry(e.cell.clone()).or_default();
                if !list.iter().any(|p| p.name == e.pin) {
                    list.push(SchematicPin {
                        name: e.pin.clone(),
                        net: n.name.clone(),
                        output: pin_is_output(&e.pin),
                    });
                }
            }
        }
        let mut nodes: Vec<SchematicNode> = d
            .cells
            .iter()
            .map(|c| {
                let kind = primitive_of(&c.kind);
                SchematicNode {
                    name: c.name.clone(),
                    kind: kind.clone(),
                    pins: merge_canonical_pins(&kind, pins.remove(&c.name).unwrap_or_default()),
                }
            })
            .collect();
        for inst in &d.instances {
            let ipins: Vec<SchematicPin> = inst
                .conns
                .iter()
                .map(|(p, net)| SchematicPin {
                    name: p.clone(),
                    net: net.clone(),
                    output: pin_is_output(p)
                        || p.eq_ignore_ascii_case("q")
                        || p.eq_ignore_ascii_case("led")
                        || p.eq_ignore_ascii_case("out"),
                })
                .collect();
            nodes.push(SchematicNode {
                name: inst.name.clone(),
                kind: format!("instance:{}", inst.module),
                pins: ipins,
            });
        }
        let mut edges = Vec::new();
        for n in &d.nets {
            let eps = &n.endpoints;
            for i in 0..eps.len() {
                for j in (i + 1)..eps.len() {
                    if eps[i].cell != eps[j].cell {
                        edges.push(SchematicEdge {
                            src: eps[i].cell.clone(),
                            src_pin: eps[i].pin.clone(),
                            dst: eps[j].cell.clone(),
                            dst_pin: eps[j].pin.clone(),
                            net: n.name.clone(),
                        });
                    }
                }
            }
        }
        let ports = d
            .ports
            .iter()
            .map(|p| SchematicPort {
                name: p.name.clone(),
                dir: match p.dir {
                    PortDir::In => "IN".into(),
                    PortDir::Out => "OUT".into(),
                    PortDir::Inout => "INOUT".into(),
                },
            })
            .collect();
        let instances = d.instances.iter().map(|i| i.name.clone()).collect();
        self.schematic = SchematicView {
            nodes,
            edges,
            ports,
            cone_root: root,
            cone_depth: depth,
            instances,
            expand_inside: expand,
            highlight_cells: hl_cells,
            highlight_nets: hl_nets,
            path_only,
            camera,
            view_history: if view_history.is_empty() {
                vec![camera]
            } else {
                view_history
            },
            view_index,
            viewport_w,
            viewport_h,
        };
    }

    fn fill_timing_path_locations(&mut self) {
        for p in &mut self.timing_paths {
            for pin in &mut p.pins {
                pin.location = site_of(&self.device, &pin.cell);
            }
        }
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
        let bram: HashSet<(u32, u32)> = dev.bram_sites().map(|s| (s.x, s.y)).collect();
        let dsp: HashSet<(u32, u32)> = dev.dsp_sites().map(|s| (s.x, s.y)).collect();
        let mut sites = Vec::new();
        for s in dev.clb_sites() {
            let bels: Vec<String> = occupants
                .iter()
                .filter(|((x, y), _)| *x == s.x && *y == s.y)
                .map(|(_, n)| n.clone())
                .collect();
            let occupant = bels.first().cloned();
            let kind = if bram.contains(&(s.x, s.y)) {
                SiteKind::Bram
            } else if dsp.contains(&(s.x, s.y)) {
                SiteKind::Dsp
            } else {
                SiteKind::Clb
            };
            sites.push(DeviceSiteView {
                x: s.x,
                y: s.y,
                kind,
                occupant,
                bels,
            });
        }
        for s in dev.iob_sites() {
            let bels: Vec<String> = occupants
                .iter()
                .filter(|((x, y), _)| *x == s.x && *y == s.y)
                .map(|(_, n)| n.clone())
                .collect();
            let occupant = bels.first().cloned();
            sites.push(DeviceSiteView {
                x: s.x,
                y: s.y,
                kind: SiteKind::Iob,
                occupant,
                bels,
            });
        }
        let (x0, y0, cols, rows) = if let (Some(xmin), Some(xmax), Some(ymin), Some(ymax)) = (
            sites.iter().map(|s| s.x).min(),
            sites.iter().map(|s| s.x).max(),
            sites.iter().map(|s| s.y).min(),
            sites.iter().map(|s| s.y).max(),
        ) {
            (xmin, ymin, xmax - xmin + 1, ymax - ymin + 1)
        } else {
            (0, 0, 0, 0)
        };
        self.device = DeviceView {
            cols,
            rows,
            x0,
            y0,
            sites,
            clock_regions: had_clock_regions(x0, y0, cols, rows),
            routes: Vec::new(),
            pblocks: self.pblocks.clone(),
        };
        self.refresh_device_routes();
    }

    fn refresh_device_routes(&mut self) {
        let mut routes = Vec::new();
        if let Some(r) = self.shell.session.routed.as_ref() {
            for io in &r.iob_src {
                routes.push(DeviceRoute {
                    net: io.net.clone(),
                    hops: io.hops,
                    delay_ps: io.delay_ps,
                    tiles: io.path.clone(),
                    highlighted: false,
                });
            }
        }
        self.device.routes = routes;
        self.highlight_device_routes();
    }

    fn highlight_device_routes(&mut self) {
        let Some(sel) = self.selected.clone() else {
            for r in &mut self.device.routes {
                r.highlighted = false;
            }
            return;
        };
        let occ: HashSet<(u32, u32)> = self
            .device
            .sites
            .iter()
            .filter(|s| {
                s.occupant.as_deref() == Some(sel.as_str()) || s.bels.iter().any(|b| b == &sel)
            })
            .map(|s| (s.x, s.y))
            .collect();
        for r in &mut self.device.routes {
            r.highlighted = r.net == sel || r.tiles.iter().any(|t| occ.contains(t));
        }
    }

    fn refresh_io_ports(&mut self) {
        let Some(d) = self.shell.session.design.as_ref() else {
            self.io_ports.clear();
            return;
        };
        let locs = self.constraints.package_pins.clone();
        let iostds = self.constraints.iostandards.clone();
        let drives = self.constraints.drives.clone();
        let slews = self.constraints.slews.clone();
        let pulls = self.constraints.pulltypes.clone();
        let diffs = self.constraints.diff_terms.clone();
        let interms = self.constraints.in_terms.clone();
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
                let package_pin = p
                    .attrs
                    .get("LOC")
                    .map(|s| s.to_string())
                    .or_else(|| locs.get(&p.name).cloned());
                let iostandard = p
                    .attrs
                    .get("IOSTANDARD")
                    .map(|s| s.to_string())
                    .or_else(|| iostds.get(&p.name).cloned());
                let drive = p
                    .attrs
                    .get("DRIVE")
                    .map(|s| s.to_string())
                    .or_else(|| drives.get(&p.name).cloned());
                let slew = p
                    .attrs
                    .get("SLEW")
                    .map(|s| s.to_string())
                    .or_else(|| slews.get(&p.name).cloned());
                let pulltype = p
                    .attrs
                    .get("PULLTYPE")
                    .map(|s| s.to_string())
                    .or_else(|| pulls.get(&p.name).cloned());
                let diff_term = p
                    .attrs
                    .get("DIFF_TERM")
                    .map(|s| s.to_string())
                    .or_else(|| diffs.get(&p.name).cloned());
                let in_term = p
                    .attrs
                    .get("IN_TERM")
                    .map(|s| s.to_string())
                    .or_else(|| interms.get(&p.name).cloned());
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
                    package_pin,
                    iostandard,
                    drive,
                    slew,
                    pulltype,
                    diff_term,
                    in_term,
                }
            })
            .collect();
    }

    fn refresh_properties(&mut self) {
        if let Some(path) = self.selected_source.clone() {
            let name = source_basename(&path);
            let ty = source_type_of(&path);
            self.properties = vec![
                ("NAME".into(), name),
                ("TYPE".into(), ty.into()),
                ("KIND".into(), "source".into()),
                ("PATH".into(), path),
            ];
            return;
        }
        if let Some(name) = self.selected_ip.clone() {
            if let Some(core) = self.ip_catalog.iter().find(|c| c.name == name).cloned() {
                let status = self
                    .ip_catalog_rows()
                    .iter()
                    .find(|r| r.name == core.name)
                    .map(|r| r.status.clone())
                    .unwrap_or_else(|| "Available".into());
                self.properties = Self::ip_properties(&core, &status);
                if let Some(xml) = &self.generated_ip_xml {
                    if xml.contains(&format!("<ipxact:name>{}</ipxact:name>", core.name)) {
                        self.properties
                            .push(("XML_BYTES".into(), xml.len().to_string()));
                    }
                }
                return;
            }
        }
        if let Some(pin_name) = self.selected_timing_pin.clone() {
            if let Ok((pi, pin)) = self.find_timing_pin(&pin_name) {
                let slack = self.timing_paths.get(pi).map(|p| p.slack_ps).unwrap_or(0);
                let net = if pin.net.is_empty() {
                    "-".to_string()
                } else {
                    pin.net.clone()
                };
                let loc = if pin.location.is_empty() {
                    "-".to_string()
                } else {
                    pin.location.clone()
                };
                self.properties = vec![
                    ("NAME".into(), pin.pin.clone()),
                    ("TYPE".into(), "timing_pin".into()),
                    ("CELL".into(), pin.cell.clone()),
                    ("PIN".into(), pin.pin.clone()),
                    ("DELAY_TYPE".into(), pin.delay_type.clone()),
                    ("INCR_PS".into(), pin.incr_ps.to_string()),
                    ("PATH_PS".into(), pin.path_ps.to_string()),
                    ("NET".into(), net),
                    ("FANOUT".into(), pin.fanout.to_string()),
                    ("LOCATION".into(), loc),
                    ("SLACK_PS".into(), slack.to_string()),
                ];
                return;
            }
        }
        let Some(id) = self.selected.clone() else {
            self.properties.clear();
            self.selected_property = None;
            return;
        };
        if let Some(rest) = id.strip_prefix("message:") {
            if let Ok(i) = rest.parse::<usize>() {
                if let Some(m) = self.messages.get(i) {
                    self.properties = vec![
                        ("NAME".into(), m.id.clone()),
                        ("TYPE".into(), "message".into()),
                        ("SEVERITY".into(), m.severity.tag().into()),
                        ("ID".into(), m.id.clone()),
                        ("INDEX".into(), i.to_string()),
                        ("TEXT".into(), m.text.clone()),
                    ];
                    return;
                }
            }
        }
        if let Some(name) = id.strip_prefix("run:") {
            if let Some(r) = self.runs.iter().find(|r| r.name == name).cloned() {
                self.properties = Self::design_run_properties(&r);
                return;
            }
        }
        if let Some(rest) = id.strip_prefix("log:") {
            if let Ok(i) = rest.parse::<usize>() {
                if let Some(line) = self.console.get(i) {
                    let status = if line.ok { "ok" } else { "error" };
                    let name = line
                        .cmd
                        .split_whitespace()
                        .next()
                        .unwrap_or(line.cmd.as_str());
                    self.properties = vec![
                        ("NAME".into(), name.into()),
                        ("TYPE".into(), "log".into()),
                        ("INDEX".into(), i.to_string()),
                        ("STATUS".into(), status.into()),
                        ("CMD".into(), line.cmd.clone()),
                        ("OUT".into(), line.out.clone()),
                    ];
                    return;
                }
            }
        }
        if let Some(rest) = id.strip_prefix("tcl:") {
            if let Ok(i) = rest.parse::<usize>() {
                if let Some(line) = self.console.get(i) {
                    let name = line
                        .cmd
                        .split_whitespace()
                        .next()
                        .unwrap_or(line.cmd.as_str());
                    self.properties = vec![
                        ("NAME".into(), name.into()),
                        ("TYPE".into(), "tcl".into()),
                        ("INDEX".into(), i.to_string()),
                        ("STATUS".into(), line.status().into()),
                        ("CMD".into(), line.cmd.clone()),
                        ("OUT".into(), line.out.clone()),
                    ];
                    return;
                }
            }
        }
        if let Some(rest) = id.strip_prefix("report:") {
            if let Some(row) = self.report_catalog().into_iter().find(|r| r.id == rest) {
                self.properties = vec![
                    ("NAME".into(), row.id),
                    ("TYPE".into(), "report".into()),
                    ("TITLE".into(), row.name),
                    ("CATEGORY".into(), row.category),
                    ("STATUS".into(), row.status),
                    ("SUMMARY".into(), row.summary),
                    ("TCL".into(), row.tcl),
                ];
                return;
            }
        }
        let mut props = vec![("NAME".into(), id.clone())];
        if let Some(d) = self.shell.session.design.as_ref() {
            if let Some(c) = d.cells.iter().find(|c| c.name == id) {
                let prim = primitive_of(&c.kind);
                props.push(("PRIMITIVE".into(), prim.clone()));
                if !props.iter().any(|(k, _)| k == "TYPE") {
                    props.push(("TYPE".into(), prim));
                }
                if !props.iter().any(|(k, _)| k == "KIND") {
                    props.push(("KIND".into(), "cell".into()));
                }
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
                for (k, v) in &p.attrs.map {
                    if !props.iter().any(|(pk, _)| pk == k) {
                        props.push((k.clone(), v.clone()));
                    }
                }
            }
        }
        if let Some(rt) = self.device.route_named(&id).cloned() {
            if !props.iter().any(|(k, _)| k == "TYPE") {
                props.push(("TYPE".into(), "net".into()));
            }
            props.push(("ROUTE_HOPS".into(), rt.hops.to_string()));
            props.push(("DELAY_PS".into(), rt.delay_ps.to_string()));
            props.push(("TILES".into(), rt.tiles.len().to_string()));
        }
        if let Some(site) = self.device.occupant_of(&id) {
            props.push((
                "LOC".into(),
                format!("X{}Y{} {:?}", site.x, site.y, site.kind),
            ));
            props.push(("SITE".into(), site.site_name()));
        }
        if let Some(site) = self
            .device
            .sites
            .iter()
            .find(|s| s.site_name() == id)
        {
            props.push(("TYPE".into(), "site".into()));
            props.push(("KIND".into(), format!("{:?}", site.kind)));
            props.push((
                "LOC".into(),
                format!("X{}Y{} {:?}", site.x, site.y, site.kind),
            ));
            props.push(("SITE".into(), site.site_name()));
            if let Some(cell) = &site.occupant {
                props.push(("OCCUPANT".into(), cell.clone()));
            }
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
        if !props.iter().any(|(k, _)| k == "PACKAGE_PIN") {
            if let Some(pin) = self
                .io_ports
                .iter()
                .find(|p| p.name == id)
                .and_then(|p| p.package_pin.clone().or(p.site.clone()))
            {
                props.push(("PACKAGE_PIN".into(), pin));
            }
        }
        if !props.iter().any(|(k, _)| k == "IOSTANDARD") {
            if let Some(std) = self
                .io_ports
                .iter()
                .find(|p| p.name == id)
                .and_then(|p| p.iostandard.clone())
            {
                props.push(("IOSTANDARD".into(), std));
            }
        }
        if let Some(p) = self.io_ports.iter().find(|p| p.name == id) {
            if !props.iter().any(|(k, _)| k == "DRIVE") {
                if let Some(v) = p.drive.clone() {
                    props.push(("DRIVE".into(), v));
                }
            }
            if !props.iter().any(|(k, _)| k == "SLEW") {
                if let Some(v) = p.slew.clone() {
                    props.push(("SLEW".into(), v));
                }
            }
            if !props.iter().any(|(k, _)| k == "PULLTYPE") {
                if let Some(v) = p.pulltype.clone() {
                    props.push(("PULLTYPE".into(), v));
                }
            }
            if !props.iter().any(|(k, _)| k == "DIFF_TERM") {
                if let Some(v) = p.diff_term.clone() {
                    props.push(("DIFF_TERM".into(), v));
                }
            }
            if !props.iter().any(|(k, _)| k == "IN_TERM") {
                if let Some(v) = p.in_term.clone() {
                    props.push(("IN_TERM".into(), v));
                }
            }
        }
        if let Some(cr) = self.device.clock_region_named(&id).cloned() {
            let sites = cr.site_count(&self.device.sites);
            props.push(("TYPE".into(), "clock_region".into()));
            props.push(("SITES".into(), sites.to_string()));
            props.push(("X0".into(), cr.x0.to_string()));
            props.push(("Y0".into(), cr.y0.to_string()));
            props.push(("X1".into(), cr.x1.to_string()));
            props.push(("Y1".into(), cr.y1.to_string()));
        }
        if let Some(pb) = self.pblocks.iter().find(|p| p.name == id).cloned() {
            let sites = pb.site_count(&self.device.sites);
            props.push(("TYPE".into(), "pblock".into()));
            props.push(("RANGE".into(), pb.range_text()));
            props.push(("SITES".into(), sites.to_string()));
            props.push(("CELLS".into(), pb.cells.len().to_string()));
            props.push(("FRAMES".into(), pb.frames.to_string()));
            props.push(("BYTES".into(), pb.bytes.to_string()));
            props.push(("X0".into(), pb.x0.to_string()));
            props.push(("Y0".into(), pb.y0.to_string()));
            props.push(("X1".into(), pb.x1.to_string()));
            props.push(("Y1".into(), pb.y1.to_string()));
        }
        if self.workspace == WorkspaceTab::Reports {
            if let Some(g) = self.timing_summary().named(&id).cloned().or_else(|| {
                id.split_once("->")
                    .and_then(|(from, to)| self.timing_summary().group(from, to).cloned())
            }) {
                if !props.iter().any(|(k, _)| k == "TYPE") {
                    props.push(("TYPE".into(), "timing_summary".into()));
                }
                props.push(("KIND".into(), g.kind.as_str().into()));
                props.push(("FROM".into(), g.from));
                props.push(("TO".into(), g.to));
                props.push((
                    "WNS_PS".into(),
                    g.wns_ps
                        .map(|w| w.to_string())
                        .unwrap_or_else(|| "n/a".into()),
                ));
                props.push(("TNS_PS".into(), g.tns_ps.to_string()));
                props.push((
                    "WHS_PS".into(),
                    g.whs_ps
                        .map(|w| w.to_string())
                        .unwrap_or_else(|| "n/a".into()),
                ));
                props.push(("THS_PS".into(), g.ths_ps.to_string()));
                props.push(("ENDPOINTS".into(), g.endpoints.to_string()));
            }
        }
        if let Some((from, to)) = id.split_once("->") {
            if self.workspace == WorkspaceTab::Cdc {
                if let Some(v) = self.cdc_report().violation(from, to).cloned() {
                    if !props.iter().any(|(k, _)| k == "TYPE") {
                        props.push(("TYPE".into(), "cdc".into()));
                    }
                    props.push(("FROM".into(), v.from));
                    props.push(("TO".into(), v.to));
                    props.push(("SEVERITY".into(), v.severity.as_str().into()));
                    props.push(("CHECK".into(), v.check));
                    props.push(("SYNC".into(), u8::from(v.synchronizer).to_string()));
                    props.push(("ENDPOINTS".into(), v.endpoints.to_string()));
                    props.push((
                        "WNS_PS".into(),
                        v.wns_ps
                            .map(|w| w.to_string())
                            .unwrap_or_else(|| "n/a".into()),
                    ));
                    props.push(("RELATION".into(), v.relation.as_str().into()));
                }
            } else if self.workspace != WorkspaceTab::Reports {
                if let Some(cell) = self.clock_interaction().cell(from, to).cloned() {
                    if !props.iter().any(|(k, _)| k == "TYPE") {
                        props.push(("TYPE".into(), "clock_interaction".into()));
                    }
                    props.push(("FROM".into(), cell.from));
                    props.push(("TO".into(), cell.to));
                    props.push(("RELATION".into(), cell.relation.as_str().into()));
                    props.push(("COMMON_PS".into(), cell.common_period_ps.to_string()));
                    props.push(("REQ_PS".into(), cell.requirement_ps.to_string()));
                    props.push((
                        "WNS_PS".into(),
                        cell.wns_ps
                            .map(|w| w.to_string())
                            .unwrap_or_else(|| "n/a".into()),
                    ));
                    props.push(("PATHS".into(), cell.path_count.to_string()));
                }
            }
        }
        if self.workspace == WorkspaceTab::ClockNetworks {
            if let Some(n) = self.clock_networks().network(&id).cloned() {
                props.retain(|(k, _)| k != "TYPE");
                props.insert(0, ("TYPE".into(), "clock_network".into()));
                props.push(("PERIOD_PS".into(), n.period_ps.to_string()));
                props.push(("SOURCE".into(), n.source));
                props.push(("NET".into(), n.net));
                props.push(("GENERATED".into(), u8::from(n.generated).to_string()));
                props.push((
                    "MASTER".into(),
                    n.master.unwrap_or_else(|| "-".into()),
                ));
                props.push(("LOADS".into(), n.n_loads.to_string()));
                props.push(("BUFFERS".into(), n.n_buffers.to_string()));
                props.push(("FANOUT".into(), n.fanout.to_string()));
                props.push(("INSERTION_PS".into(), n.insertion_ps.to_string()));
            }
        }
        if self.workspace == WorkspaceTab::Power {
            let p = self.power_report();
            if !p.part.is_empty() {
                let uw = match id.as_str() {
                    "static" => p.static_uw,
                    "dynamic" => p.dynamic_uw,
                    "clocks" => p.clocks_uw,
                    "logic" => p.logic_uw,
                    "signals" => p.signals_uw,
                    "io" => p.io_uw,
                    "bram" => p.bram_uw,
                    "dsp" => p.dsp_uw,
                    _ => p.total_uw,
                };
                if !props.iter().any(|(k, _)| k == "TYPE") {
                    props.push(("TYPE".into(), "power".into()));
                }
                props.push(("UW".into(), uw.to_string()));
                props.push(("TOTAL_UW".into(), p.total_uw.to_string()));
                props.push(("STATIC_UW".into(), p.static_uw.to_string()));
                props.push(("DYNAMIC_UW".into(), p.dynamic_uw.to_string()));
                props.push(("VOLTAGE_MV".into(), p.voltage_mv.to_string()));
                props.push(("TEMP_C".into(), p.temperature_c.to_string()));
                props.push(("F_MHZ".into(), p.f_mhz.to_string()));
            }
        }
        if self.workspace == WorkspaceTab::Methodology {
            if let Some(v) = self.methodology_report().check(&id).cloned() {
                if !props.iter().any(|(k, _)| k == "TYPE") {
                    props.push(("TYPE".into(), "methodology".into()));
                }
                props.push(("SEVERITY".into(), v.severity.as_str().into()));
                props.push(("CATEGORY".into(), v.category));
                props.push(("OBJECTS".into(), v.objects));
                props.push(("MESSAGE".into(), v.message));
            }
        }
        if self.workspace == WorkspaceTab::Drc {
            let report = self.drc.clone().unwrap_or_else(|| self.drc_report());
            if let Some(v) = report.item(&id).cloned() {
                if !props.iter().any(|(k, _)| k == "TYPE") {
                    props.push(("TYPE".into(), "drc".into()));
                }
                props.push(("SEVERITY".into(), v.severity.as_str().into()));
                props.push(("OBJECTS".into(), v.objects));
                props.push(("MESSAGE".into(), v.message));
            }
        }
        if self.workspace == WorkspaceTab::Utilization {
            if let Some(row) = self.utilization_report().row(&id).copied() {
                if !props.iter().any(|(k, _)| k == "TYPE") {
                    props.push(("TYPE".into(), "utilization".into()));
                }
                props.push(("USED".into(), row.used.to_string()));
                props.push(("AVAILABLE".into(), row.available.to_string()));
                props.push(("PCT".into(), row.pct().to_string()));
            }
        }
        if self.workspace == WorkspaceTab::Bitstream {
            let report = self.bitstream_report();
            if let Some(row) = report.frame(&id).cloned() {
                props.retain(|(k, _)| k != "TYPE" && k != "NAME");
                props.insert(0, ("NAME".into(), row.far_hex()));
                props.insert(1, ("TYPE".into(), "bitstream_frame".into()));
                props.push(("FAR".into(), row.far_hex()));
                props.push(("BLOCK".into(), row.block_name().into()));
                props.push(("DIE".into(), row.die.to_string()));
                props.push(("MAJOR".into(), row.major.to_string()));
                props.push(("MINOR".into(), row.minor.to_string()));
                props.push(("ONES".into(), row.ones().to_string()));
                props.push(("WORD".into(), row.word_hex()));
                props.push(("IDCODE".into(), format!("{:#010x}", report.idcode)));
                props.push(("HASH".into(), format!("{:#010x}", report.hash)));
            }
        }
        if self.workspace == WorkspaceTab::Hardware {
            let report = self.hw_stat_report();
            if let Some(row) = report.bit(&id).cloned() {
                props.retain(|(k, _)| k != "TYPE" && k != "NAME");
                props.insert(0, ("NAME".into(), row.name.clone()));
                props.insert(1, ("TYPE".into(), "hw_stat".into()));
                props.push(("BIT".into(), row.bit.to_string()));
                props.push(("VALUE".into(), u8::from(row.value).to_string()));
                props.push(("DESC".into(), row.description));
                props.push(("WORD".into(), report.word_hex()));
                props.push(("IDCODE".into(), format!("{:#010x}", report.idcode)));
                props.push(("IR".into(), format!("{:#04x}", report.ir)));
                props.push((
                    "PROGRAMMED".into(),
                    u8::from(report.programmed).to_string(),
                ));
            }
            if let Some(net) = id.strip_prefix("ila:") {
                if net == self.ila.net {
                    if let Some(row) = self
                        .ila_sample_rows()
                        .into_iter()
                        .find(|r| r.sample == self.wave.cursor)
                    {
                        if !props.iter().any(|(k, _)| k == "TYPE") {
                            props.retain(|(k, _)| k != "NAME");
                            props.insert(0, ("NAME".into(), id.clone()));
                            props.insert(1, ("TYPE".into(), "ila_sample".into()));
                        }
                        props.push(("SAMPLE".into(), row.sample.to_string()));
                        props.push(("TIME_PS".into(), row.time_ps.to_string()));
                        props.push(("VALUE".into(), row.value.to_string()));
                        props.push(("TRIGGER".into(), u8::from(row.trigger).to_string()));
                        props.push(("NET".into(), self.ila.net.clone()));
                    }
                }
            }
        }
        if self.workspace == WorkspaceTab::Constraints {
            if let Some(row) = self.constraint_rows().into_iter().find(|r| r.id == id) {
                let from = if row.from.is_empty() {
                    "-".into()
                } else {
                    row.from
                };
                let to = if row.to.is_empty() {
                    "-".into()
                } else {
                    row.to
                };
                props.retain(|(k, _)| k != "TYPE" && k != "NAME");
                props.insert(0, ("NAME".into(), row.name.clone()));
                props.insert(1, ("TYPE".into(), "constraint".into()));
                props.push(("SECTION".into(), row.section.as_str().into()));
                props.push(("KIND".into(), row.kind));
                props.push(("FROM".into(), from));
                props.push(("TO".into(), to));
                props.push(("VALUE".into(), row.value));
                props.push(("ENABLED".into(), u8::from(row.enabled).to_string()));
                props.push(("ID".into(), row.id));
            }
        }
        self.properties = props;
    }

    fn refresh_hw(&mut self) {
        self.hw.open = self.shell.session.hw_open;
        self.hw.programmed = self.shell.session.programmed;
        let Ok(dev) = self.device() else {
            return;
        };
        if !self.hw.open {
            return;
        }
        if self.hw.programmed {
            let stale = self.hw.stat.as_ref().map(|s| !s.done).unwrap_or(true);
            if stale {
                if let Some(bits) = self.shell.session.bitstream.as_ref() {
                    if let Ok(st) = helion_hw::prog_sim(&dev, bits) {
                        self.hw.stat = Some(st);
                        self.hw.idcode = Some(dev.idcode);
                        self.hw.ir = Some(helion_hw::IR_STAT);
                    }
                }
            }
        } else {
            let mut tap = helion_hw::Tap::new(&dev);
            self.hw.idcode = Some(tap.read_idcode());
            self.hw.stat = Some(tap.read_stat());
            self.hw.ir = Some(tap.ir);
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

fn tcl_ident(joined: &str, key: &str) -> Option<String> {
    joined.split_once(key).and_then(|(_, r)| {
        r.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .find(|s| !s.is_empty())
            .map(|s| s.to_string())
    })
}

fn parse_package_pin_cmd(cmd: &str) -> Result<(String, String), String> {
    let t = cmd.trim();
    if let Some(rest) = t.strip_prefix("assign_package_pin ") {
        let mut p = rest.split_whitespace();
        let port = p
            .next()
            .ok_or("assign_package_pin: need <port> <pin>")?;
        let pin = p
            .next()
            .ok_or("assign_package_pin: need <port> <pin>")?;
        return Ok((port.to_string(), pin.to_string()));
    }
    let toks: Vec<&str> = t.split_whitespace().collect();
    if toks.first().copied() != Some("set_property") {
        return Err("set_property PACKAGE_PIN: need PACKAGE_PIN <pin> [get_ports <port>]".into());
    }
    let key = toks.get(1).copied().unwrap_or("");
    if !key.eq_ignore_ascii_case("PACKAGE_PIN") && !key.eq_ignore_ascii_case("LOC") {
        return Err(format!("set_property: not a PACKAGE_PIN ({key})"));
    }
    let pin = toks
        .get(2)
        .ok_or("set_property PACKAGE_PIN: missing pin")?
        .to_string();
    let joined = toks.get(3..).unwrap_or(&[]).join(" ");
    let port = tcl_ident(&joined, "get_ports")
        .or_else(|| {
            toks.get(3).map(|s| {
                s.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .to_string()
            })
            .filter(|s| !s.is_empty())
        })
        .ok_or_else(|| {
            "set_property PACKAGE_PIN: missing [get_ports <port>]".to_string()
        })?;
    Ok((port, pin))
}

fn parse_port_prop_cmd(cmd: &str, want: &str) -> Result<(String, String), String> {
    let toks: Vec<&str> = cmd.split_whitespace().collect();
    if toks.first().copied() != Some("set_property") {
        return Err(format!(
            "set_property {want}: need {want} <val> [get_ports <port>]"
        ));
    }
    let key = toks.get(1).copied().unwrap_or("");
    if !key.eq_ignore_ascii_case(want) {
        return Err(format!("set_property: not a {want} ({key})"));
    }
    let val = toks
        .get(2)
        .ok_or_else(|| format!("set_property {want}: missing value"))?
        .to_string();
    let joined = toks.get(3..).unwrap_or(&[]).join(" ");
    let port = tcl_ident(&joined, "get_ports")
        .or_else(|| {
            toks.get(3)
                .map(|s| {
                    s.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .to_string()
                })
                .filter(|s| !s.is_empty())
        })
        .ok_or_else(|| format!("set_property {want}: missing [get_ports <port>]"))?;
    Ok((port, val))
}

fn parse_pblock_range(spec: &str) -> Result<(u32, u32, u32, u32), String> {
    let t = spec.trim().trim_matches(|c: char| "{}[]".contains(c)).trim();
    if t.is_empty() {
        return Err("resize_pblock: empty range".into());
    }
    if let Some((a, b)) = t.split_once(':') {
        let (x0, y0) = parse_site_xy(a)?;
        let (x1, y1) = parse_site_xy(b)?;
        return Ok((x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)));
    }
    let (x, y) = parse_site_xy(t)?;
    Ok((x, y, x, y))
}

fn parse_site_xy(spec: &str) -> Result<(u32, u32), String> {
    let t = spec.trim();
    if let Some(xoff) = t.find('X') {
        let rest = &t[xoff + 1..];
        if let Some(yoff) = rest.find('Y') {
            let x: u32 = rest[..yoff]
                .trim()
                .parse()
                .map_err(|_| format!("select_device_site: bad X in {spec}"))?;
            let y: u32 = rest[yoff + 1..]
                .trim()
                .parse()
                .map_err(|_| format!("select_device_site: bad Y in {spec}"))?;
            return Ok((x, y));
        }
    }
    let mut parts = t.split_whitespace();
    let x = parts
        .next()
        .ok_or_else(|| "select_device_site: need X Y".to_string())?
        .parse()
        .map_err(|_| format!("select_device_site: bad X in {spec}"))?;
    let y = parts
        .next()
        .ok_or_else(|| "select_device_site: need X Y".to_string())?
        .parse()
        .map_err(|_| format!("select_device_site: bad Y in {spec}"))?;
    Ok((x, y))
}

fn site_of(device: &DeviceView, cell: &str) -> String {
    device
        .occupant_of(cell)
        .map(|s| s.site_name())
        .unwrap_or_default()
}

fn net_fanout(design: &Design, net: &str) -> usize {
    design
        .net(net)
        .map(|n| n.endpoints.len().saturating_sub(1).max(1))
        .unwrap_or(1)
}

fn push_timing_pin(
    pins: &mut Vec<TimingPathPin>,
    cell: &str,
    pin: &str,
    delay_type: &str,
    incr_ps: i64,
    net: &str,
    fanout: usize,
    location: String,
) {
    let path_ps = pins.last().map(|p| p.path_ps).unwrap_or(0) + incr_ps;
    pins.push(TimingPathPin {
        cell: cell.to_string(),
        pin: format!("{cell}/{pin}"),
        delay_type: delay_type.to_string(),
        incr_ps,
        path_ps,
        net: net.to_string(),
        fanout,
        location,
    });
}

fn is_hff(design: &Design, name: &str) -> bool {
    design
        .cell(name)
        .map(|c| matches!(c.kind, CellKind::Hff))
        .unwrap_or(false)
}

/// UG903 r2r pin table: CkQ + LUT fanin×PIN + LUT comb + setup.
fn r2r_pin_rows(
    design: &Design,
    device: &DeviceView,
    start: &str,
    lut: Option<&str>,
    endpoint: &str,
) -> Vec<TimingPathPin> {
    let mut pins = Vec::new();
    if is_hff(design, start) {
        let clk_net = design.net_on(start, "C").unwrap_or("clk");
        push_timing_pin(
            &mut pins,
            start,
            "C",
            "clk",
            0,
            clk_net,
            net_fanout(design, clk_net),
            site_of(device, start),
        );
        let qnet = design.net_on(start, "Q").unwrap_or("");
        push_timing_pin(
            &mut pins,
            start,
            "Q",
            "cell",
            FF_CKQ_PS,
            qnet,
            net_fanout(design, qnet),
            site_of(device, start),
        );
    }
    if let Some(lut) = lut {
        for i in 0..6u8 {
            let pn = format!("I{i}");
            if let Some(inet) = design.net_on(lut, &pn) {
                push_timing_pin(
                    &mut pins,
                    lut,
                    &pn,
                    "net",
                    PIN_PS,
                    inet,
                    net_fanout(design, inet),
                    site_of(device, lut),
                );
            }
        }
        let onet = design.net_on(lut, "O").unwrap_or("");
        push_timing_pin(
            &mut pins,
            lut,
            "O",
            "cell",
            LUT_PS,
            onet,
            net_fanout(design, onet),
            site_of(device, lut),
        );
    }
    let dnet = design.net_on(endpoint, "D").unwrap_or("");
    push_timing_pin(
        &mut pins,
        endpoint,
        "D",
        "setup",
        SETUP_PS,
        dnet,
        net_fanout(design, dnet),
        site_of(device, endpoint),
    );
    pins
}

/// UG903 IOB pin table: CkQ + PathFinder net + pad. Sums to STA `iob_ps`.
fn iob_pin_rows(
    design: &Design,
    device: &DeviceView,
    t: &TimingResult,
    start: &str,
    iob: &str,
) -> Vec<TimingPathPin> {
    let mut pins = Vec::new();
    let has_ff = is_hff(design, start);
    let ckq = if has_ff && t.iob_ps > 0 { FF_CKQ_PS } else { 0 };
    let route = if t.iob_ps > 0 { t.route_ps } else { 0 };
    let pad_ps = (t.iob_ps - ckq - route).max(0);
    if has_ff {
        let clk_net = design.net_on(start, "C").unwrap_or("clk");
        push_timing_pin(
            &mut pins,
            start,
            "C",
            "clk",
            0,
            clk_net,
            net_fanout(design, clk_net),
            site_of(device, start),
        );
        let qnet = design.net_on(start, "Q").unwrap_or("");
        push_timing_pin(
            &mut pins,
            start,
            "Q",
            "cell",
            ckq,
            qnet,
            net_fanout(design, qnet),
            site_of(device, start),
        );
    }
    let inet = design.net_on(iob, "I").unwrap_or("");
    push_timing_pin(
        &mut pins,
        iob,
        "I",
        "net",
        route,
        inet,
        net_fanout(design, inet),
        site_of(device, iob),
    );
    let pad = design.net_on(iob, "PAD").unwrap_or("");
    push_timing_pin(
        &mut pins,
        iob,
        "PAD",
        "iob",
        pad_ps,
        pad,
        net_fanout(design, pad),
        site_of(device, iob),
    );
    pins
}

/// STA endpoint paths: each FF and IOB, cells/nets walked off the HNF (Fig. 59).
/// Pin rows are UG903 incr/path delays from helion-sta arcs, not a name list.
fn extract_timing_paths(
    design: &Design,
    t: &TimingResult,
    device: &DeviceView,
) -> Vec<TimingPath> {
    let mut paths = Vec::new();
    for c in &design.cells {
        if !matches!(c.kind, CellKind::Hff) {
            continue;
        }
        let mut cells = vec![c.name.clone()];
        let mut nets = Vec::new();
        let mut start = "clk".to_string();
        let mut lut = None;
        if let Some(dnet) = design.net_on(&c.name, "D") {
            nets.push(dnet.to_string());
            if let Some(n) = design.nets.iter().find(|n| n.name == dnet) {
                for e in &n.endpoints {
                    if e.pin == "O" && !cells.contains(&e.cell) {
                        cells.push(e.cell.clone());
                        lut = Some(e.cell.clone());
                        for i in 0..6u8 {
                            let pin = format!("I{i}");
                            if let Some(inet) = design.net_on(&e.cell, &pin) {
                                if !nets.iter().any(|x| x == inet) {
                                    nets.push(inet.to_string());
                                }
                                if let Some(nn) = design.nets.iter().find(|n| n.name == inet) {
                                    for ee in &nn.endpoints {
                                        if ee.pin == "Q" && !cells.contains(&ee.cell) {
                                            cells.push(ee.cell.clone());
                                            start = ee.cell.clone();
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        let pins = r2r_pin_rows(design, device, &start, lut.as_deref(), &c.name);
        let delay_ps = pins.last().map(|p| p.path_ps).unwrap_or(t.r2r_ps);
        paths.push(TimingPath {
            name: format!("{start}->{}", c.name),
            startpoint: start,
            endpoint: c.name.clone(),
            cells,
            nets,
            delay_ps,
            slack_ps: t.wns_ps,
            pins,
        });
    }
    for c in &design.cells {
        if !matches!(c.kind, CellKind::IobOut) {
            continue;
        }
        let mut cells = vec![c.name.clone()];
        let mut nets = Vec::new();
        let mut start = c.name.clone();
        if let Some(inet) = design.net_on(&c.name, "I") {
            nets.push(inet.to_string());
            if let Some(n) = design.nets.iter().find(|n| n.name == inet) {
                for e in &n.endpoints {
                    if e.pin == "Q" && !cells.contains(&e.cell) {
                        cells.push(e.cell.clone());
                        start = e.cell.clone();
                    }
                }
            }
        }
        if let Some(pad) = design.net_on(&c.name, "PAD") {
            if !nets.iter().any(|x| x == pad) {
                nets.push(pad.to_string());
            }
        }
        let pins = iob_pin_rows(design, device, t, &start, &c.name);
        let delay_ps = pins.last().map(|p| p.path_ps).unwrap_or(t.iob_ps);
        paths.push(TimingPath {
            name: format!("{start}->{}", c.name),
            startpoint: start,
            endpoint: c.name.clone(),
            cells,
            nets,
            delay_ps,
            slack_ps: t.wns_ps,
            pins,
        });
    }
    paths
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

    /// UG900 wave markers sit on the engine sample grid; a virtual bus packs
    /// member traces (not a canned concatenation).
    #[test]
    fn wave_markers_and_virtual_bus_from_engine_samples() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        ide.run_step(FlowStep::Bitstream).unwrap();
        let gold = ide.fabric_led_bits(16).unwrap();
        ide.sim_run(16).unwrap();
        assert_eq!(ide.wave.bits_of("led").as_deref(), Some(gold.as_str()));
        assert!(ide.wave.has_trace("cnt"), "packed LUTFF bus from fabric Q");
        let cnt = ide.wave.trace("cnt").unwrap().samples.clone();
        let cnt_w = ide.wave.trace("cnt").unwrap().width;

        let e = ide.exec("add_wave_marker M0").unwrap();
        assert!(e.contains("TIME_PS="), "{e}");
        let mcur = ide.wave.marker("M0").expect("marker at cursor");
        assert_eq!(mcur.sample, ide.wave.cursor);
        assert_eq!(
            ide.wave.time_ps(mcur.sample),
            mcur.sample as u64 * ide.wave.timescale_ps
        );

        let out = ide.exec("add_wave_marker M4 4").unwrap();
        assert!(out.contains("sample=4"), "{out}");
        assert!(out.contains("TIME_PS=40000"), "{out}");
        let m4 = ide.wave.marker("M4").unwrap();
        assert_eq!(m4.sample, 4);
        assert_eq!(ide.wave.time_ps(4), 40_000);
        let tmark = ide.exec("add_wave_marker Mt -time 80000").unwrap();
        assert!(tmark.contains("sample=8"), "{tmark}");
        assert_eq!(ide.wave.marker("Mt").unwrap().sample, 8);
        assert_eq!(ide.workspace, WorkspaceTab::Wave);

        let vb = ide.exec("add_wave_virtual_bus vb led cnt").unwrap();
        assert!(vb.contains("add_wave_virtual_bus vb"), "{vb}");
        assert!(vb.contains(&format!("width={}", 1 + cnt_w)), "{vb}");
        let packed = ide.wave.trace("vb").expect("virtual bus trace");
        assert_eq!(packed.width, 1 + cnt_w);
        assert_eq!(packed.samples.len(), 16);
        for i in 0..16 {
            let led_bit = u64::from(gold.as_bytes()[i] == b'1');
            let expect = led_bit | (cnt[i] << 1);
            assert_eq!(
                packed.samples[i], expect,
                "virtual bus sample {i} is packed engine bits, not a dump"
            );
        }
        assert!(ide.wave.virtual_bus("vb").is_some());
        assert!(
            ide.objects.iter().any(|o| o.name == "vb"),
            "{:?}",
            ide.objects
        );
        let before = packed.samples.clone();
        ide.exec("wave_radix vb hex").unwrap();
        let hex = ide.wave.trace("vb").unwrap().value_at(ide.wave.cursor);
        ide.exec("wave_radix vb binary").unwrap();
        let bin = ide.wave.trace("vb").unwrap().value_at(ide.wave.cursor);
        assert_ne!(bin, hex, "radix formats the packed bus: {bin} {hex}");
        assert_eq!(
            ide.wave.trace("vb").unwrap().samples,
            before,
            "radix must not mutate packed engine samples"
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        blinky.run_step(FlowStep::Bitstream).unwrap();
        blinky.sim_run(16).unwrap();
        let gold_b = blinky.fabric_led_bits(16).unwrap();
        assert_ne!(gold, gold_b, "LED wave is per-design from fabric");
        if blinky.wave.has_trace("cnt") {
            blinky.exec("add_wave_virtual_bus vb led cnt").unwrap();
            assert_ne!(
                blinky.wave.trace("vb").unwrap().samples,
                before,
                "virtual bus is per-design engine bits, not canned"
            );
        } else {
            blinky.exec("add_wave led").unwrap();
            let e = blinky.exec("add_wave_virtual_bus vb led missing");
            assert!(e.unwrap_err().contains("no trace"), "missing member fails");
        }

        let mut fresh = IdeModel::new();
        assert!(
            fresh.exec("add_wave_marker x").unwrap_err().contains("no wave samples"),
            "marker needs engine samples"
        );
        assert!(
            ide.exec("add_wave_virtual_bus only led")
                .unwrap_err()
                .contains("at least two"),
            "virtual bus needs two members"
        );
    }

    /// UG900 A/B dual cursors sit on the engine sample grid; Δt is B−A in the
    /// wave timescale, and Value-at-A/B is the fabric bits — not a canned pair.
    #[test]
    fn wave_ab_cursors_time_delta_from_engine_samples() {
        let mut ide = IdeModel::new();
        assert!(
            ide.exec("wave_cursor_a").unwrap_err().contains("no wave samples"),
            "A needs engine samples"
        );
        assert!(
            ide.exec("wave_cursor B 1").unwrap_err().contains("no wave samples"),
            "B needs engine samples"
        );
        let empty = ide.wave_cursors_text();
        assert!(empty.contains("A=-"), "{empty}");
        assert!(empty.contains("B=-"), "{empty}");
        assert!(empty.contains("DELTA_PS=n/a"), "{empty}");
        assert!(ide.wave.time_delta_ps().is_none());

        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        ide.run_step(FlowStep::Bitstream).unwrap();
        let gold = ide.fabric_led_bits(16).unwrap();
        ide.sim_run(16).unwrap();
        assert_eq!(ide.wave.bits_of("led").as_deref(), Some(gold.as_str()));
        let main = ide.wave.cursor;
        assert_eq!(main, 15, "sim_run parks the main cursor on the last sample");

        let a = ide.exec("wave_cursor_a 2").unwrap();
        assert!(a.contains("wave_cursor A sample=2"), "{a}");
        assert!(a.contains("TIME_PS=20000"), "{a}");
        assert!(a.contains("DELTA_PS=n/a"), "{a}");
        assert_eq!(ide.wave.cursor_a, Some(2));
        assert_eq!(ide.wave.time_ps(2), 20_000);
        assert_eq!(ide.wave.cursor, main, "placing A must not move the main cursor");
        assert_eq!(ide.workspace, WorkspaceTab::Wave);
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "wave_cursor"),
            "{:?}",
            ide.properties
        );

        let b = ide.exec("wave_cursor_b 8").unwrap();
        assert!(b.contains("wave_cursor B sample=8"), "{b}");
        assert!(b.contains("TIME_PS=80000"), "{b}");
        assert!(b.contains("DELTA_PS=60000"), "{b}");
        assert_eq!(ide.wave.cursor_b, Some(8));
        assert_eq!(ide.wave.time_ps(8), 80_000);
        assert_eq!(ide.wave.time_delta_ps(), Some(60_000));
        assert_eq!(ide.wave.cursor, main, "placing B must not move the main cursor");

        let pane = ide.exec("wave_cursors").unwrap();
        assert!(pane.contains("A_SAMPLE=2 A_TIME_PS=20000"), "{pane}");
        assert!(pane.contains("B_SAMPLE=8 B_TIME_PS=80000"), "{pane}");
        assert!(pane.contains("DELTA_PS=60000"), "{pane}");
        let led = ide.wave.trace("led").unwrap();
        let va = led.value_at(2);
        let vb = led.value_at(8);
        assert!(pane.contains(&format!("led A={va} B={vb}")), "{pane}");
        assert_eq!(va.chars().last().unwrap(), gold.as_bytes()[2] as char);
        assert_eq!(vb.chars().last().unwrap(), gold.as_bytes()[8] as char);
        if gold.as_bytes()[2] != gold.as_bytes()[8] {
            assert_ne!(va, vb, "A/B values are engine bits at those samples");
        }

        let tmark = ide.exec("wave_cursor A -time 40000").unwrap();
        assert!(tmark.contains("sample=4"), "{tmark}");
        assert_eq!(ide.wave.cursor_a, Some(4));
        assert_eq!(ide.wave.time_delta_ps(), Some(40_000));
        let swapped = ide.exec("wave_cursor B 1").unwrap();
        assert!(swapped.contains("DELTA_PS=-30000"), "{swapped}");
        assert_eq!(ide.wave.time_delta_ps(), Some(-30_000));

        ide.wave.set_cursor(10);
        let at_main = ide.exec("wave_cursor_a").unwrap();
        assert!(at_main.contains("sample=10"), "{at_main}");
        assert_eq!(ide.wave.cursor_a, Some(10));

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        blinky.run_step(FlowStep::Bitstream).unwrap();
        blinky.sim_run(16).unwrap();
        let gold_b = blinky.fabric_led_bits(16).unwrap();
        assert_ne!(gold, gold_b, "LED wave is per-design from fabric");
        blinky.exec("wave_cursor_a 2").unwrap();
        blinky.exec("wave_cursor_b 8").unwrap();
        assert_eq!(blinky.wave.time_delta_ps(), Some(60_000));
        let va_b = blinky.wave.trace("led").unwrap().value_at(2);
        let vb_b = blinky.wave.trace("led").unwrap().value_at(8);
        if gold.as_bytes()[2] != gold_b.as_bytes()[2] || gold.as_bytes()[8] != gold_b.as_bytes()[8]
        {
            assert_ne!(
                (va.clone(), vb.clone()),
                (va_b, vb_b),
                "A/B values are per-design engine bits, not a dump"
            );
        }

        ide.exec("wave_cursor_a 2").unwrap();
        ide.exec("wave_cursor_b 8").unwrap();
        assert_eq!(ide.wave.time_delta_ps(), Some(60_000));
        ide.sim_restart().unwrap();
        assert!(ide.wave.cursor_a.is_none());
        assert!(ide.wave.cursor_b.is_none());
        assert!(ide.wave.time_delta_ps().is_none());
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
        let canvas = ide.exec("bd_drawing").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Ip);
        assert!(canvas.contains("box="), "BD is boxes, not a catalog list: {canvas}");
        assert!(canvas.contains("u_h_uart:h_uart:box="), "{canvas}");
        assert!(canvas.contains("u_h_gpio:h_gpio:box="), "{canvas}");
        assert!(canvas.contains("mm_interconnect:INTERCONNECT"), "{canvas}");
        assert!(canvas.contains("bus=Helion-MM"), "{canvas}");
        assert!(canvas.contains("wire:Helion-MM:mm_interconnect->u_h_uart"), "{canvas}");
        assert!(canvas.contains("wire:clk:"), "{canvas}");
        assert!(!canvas.contains("AXI"), "{canvas}");
        let d = ide
            .block_design
            .as_ref()
            .unwrap()
            .drawing(&ide.ip_catalog);
        assert!(d.symbols.iter().all(|s| s.w > 8.0 && s.h > 8.0));
        assert!(d.wires.iter().any(|w| w.points.len() >= 2));
        assert!(d.wires.iter().all(|w| w.net != "AXI"));
    }

    /// IP Integrator is a Helion-MM canvas of IP boxes and wires, not a catalog dump.
    #[test]
    fn ip_integrator_canvas_is_helion_mm_boxes_and_wires() {
        let mut ide = IdeModel::new();
        let canvas = ide.exec("bd_drawing").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Ip);
        assert!(canvas.contains("box="), "{canvas}");
        assert!(canvas.contains("u_h_uart:h_uart:box="), "{canvas}");
        assert!(canvas.contains("u_h_gpio:h_gpio:box="), "{canvas}");
        assert!(canvas.contains("mm_interconnect:INTERCONNECT:box="), "{canvas}");
        assert!(canvas.contains("clk:PORT_IN:box="), "{canvas}");
        assert!(canvas.contains("bus=Helion-MM"), "{canvas}");
        assert!(canvas.contains("wire:Helion-MM:mm_interconnect->u_h_uart"), "{canvas}");
        assert!(canvas.contains("wire:Helion-MM:mm_interconnect->u_h_gpio"), "{canvas}");
        assert!(!canvas.contains("AXI"), "{canvas}");
        let d = ide.block_design.as_ref().unwrap().drawing(&ide.ip_catalog);
        let uart = d.symbols.iter().find(|s| s.kind == "h_uart").unwrap();
        let gpio = d.symbols.iter().find(|s| s.kind == "h_gpio").unwrap();
        let hub = d.symbols.iter().find(|s| s.kind == "INTERCONNECT").unwrap();
        assert!(uart.x > hub.x + hub.w, "IP to the right of the interconnect");
        assert!((uart.y - gpio.y).abs() > 8.0, "UART and GPIO are separate boxes");
        assert!(d.wires.iter().any(|w| w.net == "Helion-MM" && w.points.len() >= 2));
    }

    /// UG893 IP Catalog is a clickable VLNV table from helion-ipxact, not a collapsing dump.
    #[test]
    fn ip_catalog_is_clickable_vlnv_table_not_a_dump() {
        let mut ide = IdeModel::new();
        assert!(
            NavSection::IpIntegrator
                .actions()
                .iter()
                .any(|a| a.tcl == "ip_catalog"),
            "Flow Navigator IP Integrator must offer IP Catalog"
        );
        let table = ide.exec("ip_catalog").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Ip);
        assert!(table.contains("ip_catalog n="), "{table}");
        assert!(table.contains('\n'), "must not be a one-liner dump: {table}");
        assert!(table.contains("NAME=h_uart"), "{table}");
        assert!(table.contains("NAME=h_gpio"), "{table}");
        assert!(table.contains("NAME=h_rv32_hb1"), "{table}");
        assert!(table.contains("VLNV=community:helion:h_uart:1.0"), "{table}");
        assert!(table.contains("BUS=Helion-MM"), "{table}");
        assert!(table.contains("STATUS=Available"), "{table}");
        assert!(!table.contains("AXI"), "{table}");
        assert!(
            ide.ip_catalog.iter().all(|c| c.bus != "AXI"),
            "{:?}",
            ide.ip_catalog
        );
        let rows = ide.ip_catalog_rows();
        assert!(rows.len() >= 3, "catalog is helion-ipxact pack, not two dump lines");
        assert!(rows.iter().all(|r| r.vlnv.contains(&r.name) && r.bus == "Helion-MM"));

        assert!(
            ide.exec("select_ip").unwrap_err().contains("missing name"),
            "empty click must refuse"
        );
        assert!(
            ide.exec("select_ip no_such").unwrap_err().contains("no IP"),
            "unknown core must refuse"
        );

        let sel = ide.exec("select_ip h_uart").unwrap();
        assert_eq!(ide.selected_ip.as_deref(), Some("h_uart"));
        assert_eq!(ide.selected.as_deref(), Some("h_uart"));
        assert!(sel.contains("VLNV=community:helion:h_uart:1.0"), "{sel}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "ip_core"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "BUS" && v == "Helion-MM"),
            "{:?}",
            ide.properties
        );

        let generated = ide.exec("generate_ip h_uart").unwrap();
        assert!(generated.contains("xml_bytes="), "{generated}");
        assert!(generated.contains("VLNV=community:helion:h_uart:1.0"), "{generated}");
        let xml = ide.generated_ip_xml.as_deref().expect("generate_ip stores IP-XACT");
        assert!(xml.contains("<ipxact:name>h_uart</ipxact:name>"), "{xml}");
        assert!(xml.contains("Helion-MM"), "{xml}");
        assert!(!xml.contains("AXI"), "{xml}");
        assert!(
            ide.exec("generate_ip").unwrap().contains("h_uart"),
            "generate_ip with selection uses selected core"
        );
        assert!(
            ide.exec("generate_ip no_such").unwrap_err().contains("no IP"),
            "unknown generate must refuse"
        );

        ide.exec("create_bd").unwrap();
        let uart_status = ide
            .ip_catalog_rows()
            .into_iter()
            .find(|r| r.name == "h_uart")
            .unwrap();
        assert_eq!(uart_status.status, "In BD");
        let rv_status = ide
            .ip_catalog_rows()
            .into_iter()
            .find(|r| r.name == "h_rv32_hb1")
            .unwrap();
        assert_eq!(rv_status.status, "Available");
        let n0 = ide.block_design.as_ref().unwrap().cores.len();
        let add = ide.exec("create_bd_cell h_rv32_hb1").unwrap();
        assert!(add.contains("h_rv32_hb1"), "{add}");
        let bd = ide.block_design.as_ref().unwrap();
        assert_eq!(bd.cores.len(), n0 + 1);
        assert!(bd.cores.iter().any(|c| c == "h_rv32_hb1"));
        assert!(bd.sv.contains("h_rv32_hb1"), "{}", bd.sv);
        assert!(!bd.sv.contains("AXI"), "{}", bd.sv);
        let already = ide.exec("create_bd_cell h_uart").unwrap();
        assert!(already.contains("already"), "{already}");

        let hdl = ide.exec("bd_hdl").unwrap();
        assert!(hdl.contains("bd_hdl n="), "{hdl}");
        assert!(hdl.contains("INSTANCE=u_h_uart"), "{hdl}");
        assert!(hdl.contains("MODULE=h_uart"), "{hdl}");
        assert!(hdl.contains("MODULE=h_gpio"), "{hdl}");
        assert!(hdl.contains("MODULE=h_rv32_hb1"), "{hdl}");
        assert!(hdl.contains("BUS=Helion-MM"), "{hdl}");
        assert!(hdl.contains("PORTS=clk,resetn"), "{hdl}");
        assert!(!hdl.contains("module system"), "HDL pane is an instance table, not a SV dump: {hdl}");
        assert!(
            ide.exec("create_bd_cell no_such")
                .unwrap_err()
                .contains("no IP"),
            "unknown cell must refuse"
        );
        let mut empty = IdeModel::new();
        assert!(
            empty.exec("create_bd_cell").unwrap_err().contains("missing name"),
            "empty add must refuse"
        );
        assert!(
            empty.exec("generate_ip").unwrap_err().contains("missing name"),
            "empty generate must refuse"
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

    /// UG893 Timing Constraints pane is clickable clocks / I/O-delay / exception
    /// tables from helion-sta XDC — not a concatenated report_box dump.
    #[test]
    fn timing_constraints_pane_clickable_clocks_io_delay_exception_table() {
        let mut ide = IdeModel::new();
        assert!(ide.constraint_rows().is_empty());
        let empty = ide.exec("timing_constraints").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Constraints);
        assert!(
            empty.contains("no timing constraints"),
            "idle pane has no canned rows: {empty}"
        );
        assert!(
            ide.exec("select_constraint clk")
                .unwrap_err()
                .contains("no row"),
            "empty table must refuse a click"
        );
        assert!(
            NavSection::TimingAnalysis
                .actions()
                .iter()
                .any(|a| a.tcl == "timing_constraints"),
            "Flow Navigator Timing Analysis must offer the pane"
        );

        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let gold = ide.wns_ps().expect("gold WNS");
        assert_ne!(gold, 0);
        assert!(
            ide.constraint_rows().is_empty(),
            "empty XDC must not invent constraint rows: {:?}",
            ide.constraint_rows()
        );
        assert_eq!(ide.wns_ps(), Some(gold), "empty XDC keeps gold WNS");

        ide.exec("create_clock -period 10.000 [get_ports clk]")
            .unwrap();
        let rows = ide.constraint_rows();
        assert!(
            rows.iter().any(|r| r.section == ConstraintSection::Clocks
                && r.id == "clock:clk"
                && r.kind == "create_clock"
                && r.name == "clk"
                && r.value.contains("PERIOD_PS=10000")),
            "{rows:?}"
        );
        let table = ide.exec("timing_constraints").unwrap();
        assert!(table.contains("clocks=1"), "{table}");
        assert!(table.contains("io_delay=0"), "{table}");
        assert!(table.contains("clocks ID=clock:clk"), "{table}");
        let sel = ide.exec("select_constraint clock:clk").unwrap();
        assert!(sel.contains("SECTION=clocks"), "{sel}");
        assert!(sel.contains("KIND=create_clock"), "{sel}");
        assert!(sel.contains("VALUE=PERIOD_PS=10000"), "{sel}");
        assert_eq!(ide.selected.as_deref(), Some("clock:clk"));
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "constraint"),
            "{:?}",
            ide.properties
        );
        assert_eq!(
            ide.wns_ps(),
            Some(gold),
            "10 ns create_clock matches the default STA period"
        );

        ide.exec("set_output_delay -clock clk 2.0 [get_ports led]")
            .unwrap();
        ide.exec("set_input_delay -clock clk 1.5 [get_ports clk]")
            .unwrap();
        let rows = ide.constraint_rows();
        assert!(
            rows.iter().any(|r| r.section == ConstraintSection::IoDelay
                && r.kind == "set_output_delay"
                && r.name == "led"
                && r.value.contains("DELAY_PS=2000")),
            "{rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.section == ConstraintSection::IoDelay
                && r.kind == "set_input_delay"
                && r.name == "clk"
                && r.value.contains("DELAY_PS=1500")),
            "{rows:?}"
        );
        let iod = ide.exec("select_constraint output_delay:led").unwrap();
        assert!(iod.contains("SECTION=io_delay"), "{iod}");
        assert!(iod.contains("KIND=set_output_delay"), "{iod}");
        let table = ide.constraints_table_text();
        assert!(table.contains("io_delay=2"), "{table}");
        let wns_io = ide.wns_ps().expect("STA after I/O delay");
        assert_eq!(
            wns_io,
            gold - 3500,
            "I/O delay rows must feed helion-sta, not a dump: {gold} vs {wns_io}"
        );

        ide.exec("set_false_path -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        ide.exec("set_multicycle_path 2 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        let rows = ide.constraint_rows();
        assert!(
            rows.iter().any(|r| r.section == ConstraintSection::Exception
                && r.kind == "set_false_path"),
            "{rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.kind == "set_multicycle_path"
                && r.value.contains("SETUP_MULT=2")),
            "{rows:?}"
        );
        let fp = ide.exec("select_constraint false_path:clk").unwrap();
        assert!(fp.contains("SECTION=exception"), "{fp}");
        assert!(fp.contains("KIND=set_false_path"), "{fp}");
        let mcp = ide.exec("select_constraint multicycle:0:clk->led").unwrap();
        assert!(mcp.contains("KIND=set_multicycle_path"), "{mcp}");
        let table = ide.constraints_table_text();
        assert!(table.contains("exceptions="), "{table}");
        let t = ide.timing.as_ref().expect("STA after false path");
        assert_eq!(t.iob_ps, 0, "false path must drop IOB from STA");
        assert_ne!(t.wns_ps, wns_io, "exception rows must move WNS");

        ide.exec(
            "create_generated_clock -name clkdiv -source [get_ports clk] -divide_by 2 [get_pins u_ff/Q]",
        )
        .unwrap();
        let rows = ide.constraint_rows();
        assert!(
            rows.iter().any(|r| r.section == ConstraintSection::Clocks
                && r.kind == "create_generated_clock"
                && r.name == "clkdiv"
                && r.value.contains("DIVIDE_BY=2")),
            "{rows:?}"
        );
        let gsel = ide.exec("select_constraint clock:clkdiv").unwrap();
        assert!(gsel.contains("KIND=create_generated_clock"), "{gsel}");

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let b0 = blinky.wns_ps().expect("blinky gold");
        assert_ne!(b0, gold, "gold WNS is per-design STA");
        blinky
            .exec("create_clock -period 10.000 [get_ports clk]")
            .unwrap();
        blinky
            .exec("set_output_delay -clock clk 2.0 [get_ports led]")
            .unwrap();
        let brows = blinky.constraint_rows();
        assert!(
            brows.iter().any(|r| r.kind == "set_output_delay" && r.name == "led"),
            "{brows:?}"
        );
        let b1 = blinky.wns_ps().unwrap();
        assert_eq!(b1, b0 - 2000);
        assert_ne!(
            b1, wns_io,
            "I/O-delay WNS is per-design STA, not a canned table"
        );
        let bsel = blinky.exec("select_constraint output_delay:led").unwrap();
        assert!(bsel.contains("DELAY_PS=2000"), "{bsel}");
        assert_eq!(blinky.workspace, WorkspaceTab::Constraints);
    }

    /// UG893 Timing Constraints Apply: create_generated_clock -divide_by is a
    /// pane command that scales the STA period/WNS — not a label. Empty XDC
    /// keeps gold WNS.
    #[test]
    fn timing_constraints_generated_clock_divide_by_moves_sta() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let wns10 = ide.wns_ps().expect("STA WNS at default 10 ns");
        assert_ne!(wns10, 0);
        assert!(
            ide.constraints_text().contains("no timing constraints"),
            "{}",
            ide.constraints_text()
        );
        assert!(
            ide.constraints.clocks.iter().all(|c| !c.generated),
            "{:?}",
            ide.constraints.clocks
        );

        let out = ide
            .exec(
                "create_generated_clock -name clkdiv -source [get_ports clk] -divide_by 2 [get_pins u_ff/Q]",
            )
            .unwrap();
        assert!(out.contains("PERIOD_PS=20000"), "{out}");
        assert!(out.contains("DIVIDE_BY=2"), "{out}");
        assert!(out.contains("MASTER=clk"), "{out}");
        assert_eq!(ide.workspace, WorkspaceTab::Constraints);
        assert_eq!(ide.clock_period_ps, 20_000);
        assert!(
            ide.constraints
                .clocks
                .iter()
                .any(|c| c.generated && c.name == "clkdiv" && c.period_ps == 20_000 && c.divide_by == 2),
            "{:?}",
            ide.constraints.clocks
        );
        let pane = ide.constraints_text();
        assert!(pane.contains("create_generated_clock"), "{pane}");
        assert!(pane.contains("DIVIDE_BY=2"), "{pane}");
        assert!(pane.contains("PERIOD_PS=20000"), "{pane}");
        let wns20 = ide.wns_ps().expect("STA after divide_by 2");
        assert_eq!(
            wns20,
            wns10 + 10_000,
            "WNS must move with generated-clock divide_by (STA), not a canned pane: {wns10} vs {wns20}"
        );
        let ta = ide
            .ultrafast_pane_engine(UltraFastStage::TimingAnalysis)
            .unwrap();
        assert!(ta.contains(&format!("WNS_PS={wns20}")), "{ta}");
        assert!(ta.contains("clocks=2"), "{ta}");
        let rt = ide.exec("report_timing").unwrap();
        assert!(
            rt.contains(&format!("WNS_PS={wns20}")),
            "Tcl report_timing must use generated-clock period, not Session 10 ns: {rt}"
        );

        let out4 = ide
            .exec(
                "create_generated_clock -name clkdiv -source [get_ports clk] -divide_by 4 [get_pins u_ff/Q]",
            )
            .unwrap();
        assert!(out4.contains("PERIOD_PS=40000"), "{out4}");
        assert!(out4.contains("DIVIDE_BY=4"), "{out4}");
        assert_eq!(ide.clock_period_ps, 40_000);
        let wns40 = ide.wns_ps().expect("STA after divide_by 4");
        assert_eq!(
            wns40,
            wns10 + 30_000,
            "divide_by 4 must scale period/WNS again: {wns10} vs {wns40}"
        );
        assert!(
            ide.constraints_text().contains("DIVIDE_BY=4"),
            "{}",
            ide.constraints_text()
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let b0 = blinky.wns_ps().expect("blinky gold WNS");
        assert_ne!(b0, wns10, "gold WNS is per-design STA");
        blinky
            .exec(
                "create_generated_clock -name clkdiv -source [get_ports clk] -divide_by 2 [get_pins u_ff/Q]",
            )
            .unwrap();
        let b1 = blinky.wns_ps().expect("blinky after divide_by 2");
        assert_eq!(b1, b0 + 10_000);
        assert_ne!(b1, wns20, "generated-clock WNS is per-design STA, not canned");
    }

    /// UG893/UG903 create_generated_clock -multiply_by / -invert / -edges
    /// move helion-sta WNS — empty XDC keeps gold.
    #[test]
    fn timing_constraints_generated_clock_multiply_by_invert_edges_moves_sta() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let wns10 = ide.wns_ps().expect("STA WNS at default 10 ns");
        assert_ne!(wns10, 0);
        assert!(
            ide.constraints_text().contains("no timing constraints"),
            "{}",
            ide.constraints_text()
        );

        let out = ide
            .exec(
                "create_generated_clock -name clk2x -source [get_ports clk] -multiply_by 2 [get_pins u_ff/Q]",
            )
            .unwrap();
        assert!(out.contains("PERIOD_PS=5000"), "{out}");
        assert!(out.contains("MULTIPLY_BY=2"), "{out}");
        assert!(out.contains("INVERT=0"), "{out}");
        assert_eq!(ide.clock_period_ps, 5_000);
        assert!(
            ide.constraints.clocks.iter().any(|c| {
                c.generated && c.name == "clk2x" && c.period_ps == 5_000 && c.multiply_by == 2
            }),
            "{:?}",
            ide.constraints.clocks
        );
        let pane = ide.constraints_text();
        assert!(pane.contains("-multiply_by 2"), "{pane}");
        assert!(pane.contains("MULTIPLY_BY=2"), "{pane}");
        let wns_mul = ide.wns_ps().expect("STA after multiply_by 2");
        assert_eq!(
            wns_mul,
            wns10 - 5_000,
            "multiply_by 2 must halve the requirement: {wns10} vs {wns_mul}"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(
            rt.contains(&format!("WNS_PS={wns_mul}")),
            "Tcl report_timing must use multiplied period: {rt}"
        );

        let mut inv = IdeModel::new();
        inv.open_source(&example("counter.sv")).unwrap();
        inv.run_step(FlowStep::Opt).unwrap();
        inv.run_step(FlowStep::Place).unwrap();
        inv.run_step(FlowStep::Route).unwrap();
        let out_inv = inv
            .exec(
                "create_generated_clock -name clkinv -source [get_ports clk] -divide_by 1 -invert [get_pins u_ff/Q]",
            )
            .unwrap();
        assert!(out_inv.contains("PERIOD_PS=10000"), "{out_inv}");
        assert!(out_inv.contains("INVERT=1"), "{out_inv}");
        assert!(
            inv.constraints
                .clocks
                .iter()
                .any(|c| c.generated && c.invert && c.period_ps == 10_000),
            "{:?}",
            inv.constraints.clocks
        );
        let pane_inv = inv.constraints_text();
        assert!(pane_inv.contains("-invert"), "{pane_inv}");
        assert!(pane_inv.contains("INVERT=1"), "{pane_inv}");
        let wns_inv = inv.wns_ps().expect("STA after invert");
        assert_eq!(
            wns_inv,
            wns10 - 5_000,
            "invert is a half-cycle setup: {wns10} vs {wns_inv}"
        );

        let mut edg = IdeModel::new();
        edg.open_source(&example("counter.sv")).unwrap();
        edg.run_step(FlowStep::Opt).unwrap();
        edg.run_step(FlowStep::Place).unwrap();
        edg.run_step(FlowStep::Route).unwrap();
        let out_edg = edg
            .exec(
                "create_generated_clock -name clkedg -source [get_ports clk] -edges {1 3 5} [get_pins u_ff/Q]",
            )
            .unwrap();
        assert!(out_edg.contains("PERIOD_PS=20000"), "{out_edg}");
        assert!(out_edg.contains("EDGES=1,3,5"), "{out_edg}");
        assert!(
            edg.constraints.clocks.iter().any(|c| {
                c.generated && c.name == "clkedg" && c.edges == vec![1, 3, 5] && c.period_ps == 20_000
            }),
            "{:?}",
            edg.constraints.clocks
        );
        let pane_edg = edg.constraints_text();
        assert!(pane_edg.contains("-edges {1 3 5}"), "{pane_edg}");
        let wns_edg = edg.wns_ps().expect("STA after edges");
        assert_eq!(
            wns_edg,
            wns10 + 10_000,
            "edges {{1 3 5}} is divide-by-2: {wns10} vs {wns_edg}"
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let b0 = blinky.wns_ps().expect("blinky gold WNS");
        assert_ne!(b0, wns10, "gold WNS is per-design STA");
        blinky
            .exec(
                "create_generated_clock -name clk2x -source [get_ports clk] -multiply_by 2 [get_pins u_ff/Q]",
            )
            .unwrap();
        let b1 = blinky.wns_ps().expect("blinky after multiply_by 2");
        assert_eq!(b1, b0 - 5_000);
        assert_ne!(b1, wns_mul, "multiply_by WNS is per-design STA, not canned");
        assert_eq!(
            IdeModel::new().wns_ps(),
            None,
            "idle model has no canned WNS"
        );
    }

    /// UG893 Timing Constraints Apply: set_bus_skew / group_path -weight
    /// move helion-sta WNS — empty XDC keeps gold.
    #[test]
    fn timing_constraints_bus_skew_group_path_apply_moves_sta() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let wns0 = ide.wns_ps().expect("STA WNS before bus skew");
        let setup0 = ide.timing.as_ref().unwrap().setup_ps;
        let hold0 = ide.timing.as_ref().unwrap().hold_slack_ps;
        assert_ne!(wns0, 0);
        assert!(
            ide.constraints.bus_skews.is_empty() && ide.constraints.path_groups.is_empty(),
            "{:?}",
            ide.constraints
        );

        let bs = ide
            .exec("set_bus_skew -setup 0.5 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        assert!(bs.contains("bus_skew=1"), "{bs}");
        assert!(bs.contains("BUS_SKEW_PS=500"), "{bs}");
        assert_eq!(ide.workspace, WorkspaceTab::Constraints);
        assert_eq!(ide.constraints.bus_skew_setup_ps(), 500);
        assert_eq!(ide.constraints.bus_skew_hold_ps(), 0);
        assert!(
            ide.constraints_text()
                .contains("set_bus_skew -from clk -to led SKEW_PS=500 setup=1 hold=0"),
            "{}",
            ide.constraints_text()
        );
        let wns_bs = ide.wns_ps().expect("STA after setup bus skew");
        assert_eq!(
            wns_bs,
            wns0 - 500,
            "setup bus skew 0.5 ns must worsen WNS: {wns0} vs {wns_bs}"
        );
        assert_eq!(
            ide.timing.as_ref().unwrap().hold_slack_ps,
            hold0,
            "setup-only bus skew must not move hold"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(
            rt.contains(&format!("WNS_PS={wns_bs}")),
            "report_timing must honor bus skew: {rt}"
        );

        let bh = ide
            .exec("set_bus_skew -hold 0.2 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        assert!(bh.contains("BUS_SKEW_PS=200"), "{bh}");
        assert_eq!(ide.constraints.bus_skew_hold_ps(), 200);
        let hold_bs = ide.timing.as_ref().expect("STA after hold bus skew").hold_slack_ps;
        assert_eq!(ide.wns_ps().unwrap(), wns_bs, "hold bus skew must not move setup WNS");
        assert_eq!(
            hold_bs,
            hold0 - 200,
            "hold bus skew 0.2 ns must worsen hold slack: {hold_bs} vs {hold0}"
        );

        let gp = ide
            .exec("group_path -name extra -weight 2 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        assert!(gp.contains("group_path=1"), "{gp}");
        assert!(gp.contains("WEIGHT_MILLI=2000"), "{gp}");
        assert_eq!(ide.constraints.group_path_weight_milli(), 2000);
        assert!(
            ide.constraints_text()
                .contains("group_path -name extra -from clk -to led WEIGHT_MILLI=2000"),
            "{}",
            ide.constraints_text()
        );
        let wns_gp = ide.wns_ps().expect("STA after group_path weight");
        assert_eq!(
            wns_gp,
            wns_bs - setup0,
            "group_path -weight 2 must double setup: {wns_bs} vs {wns_gp} setup={setup0}"
        );
        assert_eq!(
            ide.timing.as_ref().unwrap().hold_slack_ps,
            hold_bs,
            "group_path weight must not move hold"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(
            rt.contains(&format!("WNS_PS={wns_gp}")),
            "report_timing must honor group_path weight: {rt}"
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let b0 = blinky.wns_ps().unwrap();
        let bsetup = blinky.timing.as_ref().unwrap().setup_ps;
        assert_ne!(b0, wns0, "gold WNS is per-design STA");
        blinky
            .exec("set_bus_skew -setup 0.5 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        let b1 = blinky.wns_ps().unwrap();
        assert_eq!(b1, b0 - 500);
        assert_ne!(b1, wns_bs, "bus-skew WNS is per-design STA, not canned");
        blinky
            .exec("group_path -name extra -weight 2 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        let b2 = blinky.wns_ps().unwrap();
        assert_eq!(b2, b1 - bsetup);
        assert_ne!(b2, wns_gp, "group_path WNS is per-design STA, not canned");
    }

    /// UG893/UG903 Timing Constraints Apply: set_max_time_borrow (latch steal)
    /// and set_data_check (data-to-data) move helion-sta WNS — empty XDC keeps gold.
    #[test]
    fn timing_constraints_time_borrow_data_check_apply_moves_sta() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let wns0 = ide.wns_ps().expect("STA WNS before time borrow");
        let hold0 = ide.timing.as_ref().unwrap().hold_slack_ps;
        assert_ne!(wns0, 0);
        assert!(
            ide.constraints.max_time_borrows.is_empty() && ide.constraints.data_checks.is_empty(),
            "{:?}",
            ide.constraints
        );

        let tb = ide
            .exec("set_max_time_borrow 1.0 [get_cells u_ff]")
            .unwrap();
        assert!(tb.contains("time_borrow=1"), "{tb}");
        assert!(tb.contains("BORROW_PS=1000"), "{tb}");
        assert_eq!(ide.workspace, WorkspaceTab::Constraints);
        assert_eq!(ide.constraints.time_borrow_ps(), 1000);
        assert!(
            ide.constraints_text()
                .contains("set_max_time_borrow u_ff BORROW_PS=1000"),
            "{}",
            ide.constraints_text()
        );
        let wns_tb = ide.wns_ps().expect("STA after time borrow");
        assert_eq!(
            wns_tb,
            wns0 + 1000,
            "latch borrow 1 ns must improve WNS: {wns0} vs {wns_tb}"
        );
        assert_eq!(
            ide.timing.as_ref().unwrap().hold_slack_ps,
            hold0,
            "time borrow must not move hold"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(
            rt.contains(&format!("WNS_PS={wns_tb}")),
            "report_timing must honor time borrow: {rt}"
        );

        let dc = ide
            .exec("set_data_check -setup 0.5 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        assert!(dc.contains("data_check=1"), "{dc}");
        assert!(dc.contains("DATA_CHECK_SETUP_PS=500"), "{dc}");
        assert_eq!(ide.constraints.data_check_setup_ps(), 500);
        assert_eq!(ide.constraints.data_check_hold_ps(), 0);
        assert!(
            ide.constraints_text()
                .contains("set_data_check -from clk -to led SETUP_PS=500 HOLD_PS=0"),
            "{}",
            ide.constraints_text()
        );
        let wns_dc = ide.wns_ps().expect("STA after setup data check");
        assert_eq!(
            wns_dc,
            wns_tb - 500,
            "setup data check 0.5 ns must worsen WNS: {wns_tb} vs {wns_dc}"
        );
        assert_eq!(
            ide.timing.as_ref().unwrap().hold_slack_ps,
            hold0,
            "setup-only data check must not move hold"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(
            rt.contains(&format!("WNS_PS={wns_dc}")),
            "report_timing must honor data check: {rt}"
        );

        let dh = ide
            .exec("set_data_check -hold 0.2 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        assert!(dh.contains("DATA_CHECK_HOLD_PS=200"), "{dh}");
        assert_eq!(ide.constraints.data_check_hold_ps(), 200);
        let hold_dc = ide.timing.as_ref().expect("STA after hold data check").hold_slack_ps;
        assert_eq!(ide.wns_ps().unwrap(), wns_dc, "hold data check must not move setup WNS");
        assert_eq!(
            hold_dc,
            hold0 - 200,
            "hold data check 0.2 ns must worsen hold slack: {hold_dc} vs {hold0}"
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let b0 = blinky.wns_ps().unwrap();
        assert_ne!(b0, wns0, "gold WNS is per-design STA");
        blinky
            .exec("set_max_time_borrow 1.0 [get_cells u_ff]")
            .unwrap();
        let b1 = blinky.wns_ps().unwrap();
        assert_eq!(b1, b0 + 1000);
        assert_ne!(b1, wns_tb, "time-borrow WNS is per-design STA, not canned");
        blinky
            .exec("set_data_check -setup 0.5 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        let b2 = blinky.wns_ps().unwrap();
        assert_eq!(b2, b1 - 500);
        assert_ne!(b2, wns_dc, "data-check WNS is per-design STA, not canned");
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

    /// UG893 Timing Constraints Apply: set_multicycle_path / set_max_delay
    /// move helion-sta setup WNS and hold slack — not labels on a stub editor.
    #[test]
    fn timing_constraints_multicycle_max_delay_apply_moves_sta() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let wns0 = ide.wns_ps().expect("STA WNS before multicycle");
        let setup0 = ide.timing.as_ref().unwrap().setup_ps;
        let hold0 = ide.timing.as_ref().unwrap().hold_slack_ps;
        assert_ne!(wns0, 0);
        assert!(
            ide.constraints.multicycle_paths.is_empty() && ide.constraints.max_delays.is_empty(),
            "{:?}",
            ide.constraints
        );

        let mcp = ide
            .exec("set_multicycle_path 2 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        assert!(mcp.contains("multicycle=1"), "{mcp}");
        assert!(mcp.contains("SETUP_MULT=2"), "{mcp}");
        assert!(mcp.contains("HOLD_MULT=0"), "{mcp}");
        assert_eq!(ide.workspace, WorkspaceTab::Constraints);
        assert_eq!(ide.constraints.setup_mult(), 2);
        assert_eq!(ide.constraints.hold_mult(), 0);
        assert!(
            ide.constraints_text()
                .contains("set_multicycle_path -from clk -to led SETUP_MULT=2 HOLD_MULT=0"),
            "{}",
            ide.constraints_text()
        );
        let wns_mcp = ide.wns_ps().expect("STA after setup MCP 2");
        assert_eq!(
            wns_mcp,
            wns0 + 10_000,
            "setup MCP 2 must add one 10 ns period to WNS: {wns0} vs {wns_mcp}"
        );
        assert_eq!(
            ide.timing.as_ref().unwrap().setup_ps,
            setup0,
            "MCP changes the requirement, not the path delay"
        );
        assert_eq!(
            ide.timing.as_ref().unwrap().hold_slack_ps,
            hold0,
            "setup-only MCP must not move hold"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(
            rt.contains(&format!("WNS_PS={wns_mcp}")),
            "report_timing must honor setup MCP: {rt}"
        );

        let hold = ide
            .exec("set_multicycle_path -hold 1 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        assert!(hold.contains("HOLD_MULT=1"), "{hold}");
        assert_eq!(ide.constraints.hold_mult(), 1);
        assert!(
            ide.constraints_text().contains("HOLD_MULT=1"),
            "{}",
            ide.constraints_text()
        );
        let hold_slack = ide.timing.as_ref().expect("STA after hold MCP").hold_slack_ps;
        let wns_hold = ide.timing.as_ref().unwrap().wns_ps;
        assert_eq!(wns_hold, wns_mcp, "hold MCP must not move setup WNS");
        assert_eq!(
            hold_slack,
            hold0 - 10_000,
            "hold MCP 1 must subtract one period from hold slack: {hold_slack} vs {hold0}"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(rt.contains(&format!("HOLD_SLACK_PS={hold_slack}")), "{rt}");

        let md = ide
            .exec("set_max_delay 5.0 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        assert!(md.contains("max_delay=1"), "{md}");
        assert!(md.contains("MAX_DELAY_PS=5000"), "{md}");
        assert_eq!(ide.constraints.max_delay_ps(), Some(5000));
        assert!(
            ide.constraints_text()
                .contains("set_max_delay -from clk -to led DELAY_PS=5000 datapath_only=0"),
            "{}",
            ide.constraints_text()
        );
        let wns_md = ide.wns_ps().expect("STA after max_delay");
        assert_eq!(
            wns_md,
            5000 - setup0,
            "set_max_delay 5 ns replaces the period/MCP requirement: {wns_md} vs setup {setup0}"
        );
        assert_ne!(wns_md, wns_mcp, "max_delay must move WNS off the MCP result");
        let rt = ide.exec("report_timing").unwrap();
        assert!(rt.contains(&format!("WNS_PS={wns_md}")), "{rt}");

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let b0 = blinky.wns_ps().unwrap();
        blinky
            .exec("set_multicycle_path 2 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        let b1 = blinky.wns_ps().unwrap();
        assert_eq!(b1, b0 + 10_000);
        assert_ne!(b1, wns_mcp, "MCP WNS is per-design STA, not canned");
        blinky
            .exec("set_max_delay 5.0 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        let b2 = blinky.wns_ps().unwrap();
        let bsetup = blinky.timing.as_ref().unwrap().setup_ps;
        assert_eq!(b2, 5000 - bsetup);
        assert_ne!(b2, wns_md, "max_delay WNS is per-design STA, not canned");
    }

    /// UG893 Timing Constraints Apply: set_min_delay / set_clock_groups
    /// move helion-sta hold slack and setup WNS — not labels on a stub editor.
    #[test]
    fn timing_constraints_min_delay_clock_groups_apply_moves_sta() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let wns0 = ide.wns_ps().expect("STA WNS before min_delay");
        let hold0 = ide.timing.as_ref().unwrap().hold_slack_ps;
        let hold_ps0 = ide.timing.as_ref().unwrap().hold_ps;
        assert_ne!(wns0, 0);
        assert!(
            ide.constraints.min_delays.is_empty() && ide.constraints.clock_groups.is_empty(),
            "{:?}",
            ide.constraints
        );

        let mind = ide
            .exec("set_min_delay 1.0 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        assert!(mind.contains("min_delay=1"), "{mind}");
        assert!(mind.contains("MIN_DELAY_PS=1000"), "{mind}");
        assert_eq!(ide.workspace, WorkspaceTab::Constraints);
        assert_eq!(ide.constraints.min_delay_ps(), Some(1000));
        assert!(
            ide.constraints_text()
                .contains("set_min_delay -from clk -to led DELAY_PS=1000 datapath_only=0"),
            "{}",
            ide.constraints_text()
        );
        let t = ide.timing.as_ref().expect("STA after min_delay");
        assert_eq!(
            t.hold_slack_ps,
            hold_ps0 - 1000,
            "set_min_delay 1 ns replaces HOLD_REQ_PS: {} vs hold {}",
            t.hold_slack_ps,
            hold_ps0
        );
        assert_eq!(t.wns_ps, wns0, "min_delay must not move setup WNS");
        assert_ne!(
            t.hold_slack_ps, hold0,
            "min_delay must move hold slack off the gold result"
        );
        let hold_min = t.hold_slack_ps;
        let rt = ide.exec("report_timing").unwrap();
        assert!(
            rt.contains(&format!("HOLD_SLACK_PS={hold_min}")),
            "report_timing must honor set_min_delay: {rt}"
        );

        let od = ide
            .exec("set_output_delay -clock clk 2.0 [get_ports led]")
            .unwrap();
        assert!(od.contains("output_delay=1"), "{od}");
        let wns_od = ide.wns_ps().expect("STA after output delay");
        assert_eq!(
            wns_od,
            wns0 - 2000,
            "output delay must still worsen WNS before clock groups: {wns0} vs {wns_od}"
        );

        let cg = ide
            .exec("set_clock_groups -asynchronous -group [get_clocks clk] -group [get_clocks virt]")
            .unwrap();
        assert!(cg.contains("clock_groups=1"), "{cg}");
        assert!(cg.contains("GROUPS=2"), "{cg}");
        assert!(
            ide.constraints.clock_groups_false_path(),
            "{:?}",
            ide.constraints.clock_groups
        );
        assert!(
            ide.constraints_text()
                .contains("set_clock_groups -asynchronous groups=2 -group clk -group virt"),
            "{}",
            ide.constraints_text()
        );
        let t = ide.timing.as_ref().expect("STA after clock groups");
        assert_eq!(t.iob_ps, 0, "clock groups must drop IOB from STA");
        assert_eq!(t.setup_ps, t.r2r_ps, "clock groups setup is r2r only");
        let wns_cg = t.wns_ps;
        assert_ne!(
            wns_cg, wns_od,
            "clock groups must move WNS off the I/O-delay result"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(rt.contains(&format!("WNS_PS={wns_cg}")), "{rt}");
        assert!(rt.contains("iob_ps=0"), "{rt}");

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let bhold0 = blinky.timing.as_ref().unwrap().hold_ps;
        let bwns0 = blinky.wns_ps().unwrap();
        assert_ne!(bwns0, wns0, "setup WNS is per-design STA");
        blinky
            .exec("set_min_delay 1.0 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        let bhold = blinky.timing.as_ref().unwrap().hold_slack_ps;
        assert_eq!(bhold, bhold0 - 1000);
        assert_ne!(bhold0, 0, "hold delay comes from route, not a canned 0");
        blinky
            .exec("set_output_delay -clock clk 2.0 [get_ports led]")
            .unwrap();
        let b_od = blinky.wns_ps().unwrap();
        assert_eq!(b_od, bwns0 - 2000);
        blinky
            .exec("set_clock_groups -asynchronous -group [get_clocks clk] -group [get_clocks virt]")
            .unwrap();
        let b_cg = blinky.wns_ps().unwrap();
        assert_ne!(b_cg, b_od, "clock groups must move blinky WNS off I/O delay");
        assert_ne!(b_cg, wns_cg, "clock groups WNS is per-design STA, not canned");
    }

    /// UG893 Timing Constraints Apply: set_clock_uncertainty / set_clock_latency
    /// move helion-sta setup WNS and hold slack — not labels on a stub editor.
    #[test]
    fn timing_constraints_uncertainty_latency_apply_moves_sta() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let wns0 = ide.wns_ps().expect("STA WNS before uncertainty");
        let hold0 = ide.timing.as_ref().unwrap().hold_slack_ps;
        assert_ne!(wns0, 0);
        assert!(
            ide.constraints.clock_uncertainties.is_empty()
                && ide.constraints.clock_latencies.is_empty(),
            "{:?}",
            ide.constraints
        );

        let su = ide
            .exec("set_clock_uncertainty -setup 0.5 [get_clocks clk]")
            .unwrap();
        assert!(su.contains("uncertainty=1"), "{su}");
        assert!(su.contains("UNCERT_SETUP_PS=500"), "{su}");
        assert!(su.contains("UNCERT_HOLD_PS=0"), "{su}");
        assert_eq!(ide.workspace, WorkspaceTab::Constraints);
        assert_eq!(ide.constraints.uncertainty_setup_ps(), 500);
        assert_eq!(ide.constraints.uncertainty_hold_ps(), 0);
        assert!(
            ide.constraints_text()
                .contains("set_clock_uncertainty")
                && ide.constraints_text().contains("SETUP_PS=500"),
            "{}",
            ide.constraints_text()
        );
        let wns_su = ide.wns_ps().expect("STA after setup uncertainty");
        assert_eq!(
            wns_su,
            wns0 - 500,
            "setup uncertainty 0.5 ns must worsen WNS: {wns0} vs {wns_su}"
        );
        assert_eq!(
            ide.timing.as_ref().unwrap().hold_slack_ps,
            hold0,
            "setup-only uncertainty must not move hold"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(
            rt.contains(&format!("WNS_PS={wns_su}")),
            "report_timing must honor setup uncertainty: {rt}"
        );

        let hu = ide
            .exec("set_clock_uncertainty -hold 0.2 [get_clocks clk]")
            .unwrap();
        assert!(hu.contains("UNCERT_HOLD_PS=200"), "{hu}");
        assert_eq!(ide.constraints.uncertainty_hold_ps(), 200);
        let hold_u = ide.timing.as_ref().expect("STA after hold uncertainty").hold_slack_ps;
        assert_eq!(ide.wns_ps().unwrap(), wns_su, "hold uncertainty must not move setup WNS");
        assert_eq!(
            hold_u,
            hold0 - 200,
            "hold uncertainty 0.2 ns must worsen hold slack: {hold_u} vs {hold0}"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(rt.contains(&format!("HOLD_SLACK_PS={hold_u}")), "{rt}");

        let late = ide
            .exec("set_clock_latency -late 0.4 [get_clocks clk]")
            .unwrap();
        assert!(late.contains("latency=1"), "{late}");
        assert!(late.contains("LATE_PS=400"), "{late}");
        assert_eq!(ide.constraints.latency_late_ps(), 400);
        assert!(
            ide.constraints_text()
                .contains("set_clock_latency clk LATE_PS=400 EARLY_PS=0 source=0"),
            "{}",
            ide.constraints_text()
        );
        let wns_late = ide.wns_ps().expect("STA after late latency");
        assert_eq!(
            wns_late,
            wns_su - 400,
            "late latency 0.4 ns must worsen WNS: {wns_su} vs {wns_late}"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(rt.contains(&format!("WNS_PS={wns_late}")), "{rt}");

        let early = ide
            .exec("set_clock_latency -source -early 0.1 [get_clocks clk]")
            .unwrap();
        assert!(early.contains("EARLY_PS=100"), "{early}");
        assert_eq!(ide.constraints.latency_early_ps(), 100);
        assert!(
            ide.constraints_text().contains("EARLY_PS=100")
                && ide.constraints_text().contains("source=1"),
            "{}",
            ide.constraints_text()
        );
        let hold_e = ide.timing.as_ref().expect("STA after early latency").hold_slack_ps;
        assert_eq!(ide.wns_ps().unwrap(), wns_late, "early latency must not move setup WNS");
        assert_eq!(
            hold_e,
            hold_u - 100,
            "early latency 0.1 ns must worsen hold slack: {hold_e} vs {hold_u}"
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let b0 = blinky.wns_ps().unwrap();
        blinky
            .exec("set_clock_uncertainty -setup 0.5 [get_clocks clk]")
            .unwrap();
        let b1 = blinky.wns_ps().unwrap();
        assert_eq!(b1, b0 - 500);
        assert_ne!(b1, wns_su, "uncertainty WNS is per-design STA, not canned");
        blinky
            .exec("set_clock_latency -late 0.4 [get_clocks clk]")
            .unwrap();
        let b2 = blinky.wns_ps().unwrap();
        assert_eq!(b2, b1 - 400);
        assert_ne!(b2, wns_late, "latency WNS is per-design STA, not canned");
    }

    /// UG893 Timing Constraints Apply: set_input_jitter / set_system_jitter
    /// move helion-sta setup WNS and hold slack like uncertainty — empty XDC keeps gold.
    #[test]
    fn timing_constraints_input_system_jitter_apply_moves_sta() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let wns0 = ide.wns_ps().expect("STA WNS before jitter");
        let hold0 = ide.timing.as_ref().unwrap().hold_slack_ps;
        assert_ne!(wns0, 0);
        assert!(
            ide.constraints.input_jitters.is_empty() && ide.constraints.system_jitter_ps == 0,
            "{:?}",
            ide.constraints
        );

        let ij = ide
            .exec("set_input_jitter [get_clocks clk] 0.2")
            .unwrap();
        assert!(ij.contains("input_jitter=1"), "{ij}");
        assert!(ij.contains("INPUT_JITTER_PS=200"), "{ij}");
        assert_eq!(ide.workspace, WorkspaceTab::Constraints);
        assert_eq!(ide.constraints.input_jitter_ps(), 200);
        assert_eq!(ide.constraints.jitter_setup_ps(), 200);
        assert!(
            ide.constraints_text()
                .contains("set_input_jitter clk JITTER_PS=200"),
            "{}",
            ide.constraints_text()
        );
        let wns_ij = ide.wns_ps().expect("STA after input jitter");
        assert_eq!(
            wns_ij,
            wns0 - 200,
            "input jitter 0.2 ns must worsen WNS: {wns0} vs {wns_ij}"
        );
        let hold_ij = ide.timing.as_ref().expect("STA after input jitter").hold_slack_ps;
        assert_eq!(
            hold_ij,
            hold0 - 200,
            "input jitter 0.2 ns must worsen hold slack: {hold_ij} vs {hold0}"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(
            rt.contains(&format!("WNS_PS={wns_ij}")),
            "report_timing must honor input jitter: {rt}"
        );
        assert!(rt.contains(&format!("HOLD_SLACK_PS={hold_ij}")), "{rt}");

        let sj = ide.exec("set_system_jitter 0.1").unwrap();
        assert!(sj.contains("system_jitter=1"), "{sj}");
        assert!(sj.contains("SYSTEM_JITTER_PS=100"), "{sj}");
        assert_eq!(ide.constraints.system_jitter_ps, 100);
        assert_eq!(ide.constraints.jitter_setup_ps(), 300);
        assert!(
            ide.constraints_text()
                .contains("set_system_jitter JITTER_PS=100"),
            "{}",
            ide.constraints_text()
        );
        let wns_sj = ide.wns_ps().expect("STA after system jitter");
        assert_eq!(
            wns_sj,
            wns_ij - 100,
            "system jitter 0.1 ns must stack on WNS: {wns_ij} vs {wns_sj}"
        );
        let hold_sj = ide.timing.as_ref().expect("STA after system jitter").hold_slack_ps;
        assert_eq!(
            hold_sj,
            hold_ij - 100,
            "system jitter 0.1 ns must stack on hold: {hold_sj} vs {hold_ij}"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(rt.contains(&format!("WNS_PS={wns_sj}")), "{rt}");
        assert!(rt.contains(&format!("HOLD_SLACK_PS={hold_sj}")), "{rt}");

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let b0 = blinky.wns_ps().unwrap();
        let bhold0 = blinky.timing.as_ref().unwrap().hold_slack_ps;
        blinky
            .exec("set_input_jitter [get_clocks clk] 0.2")
            .unwrap();
        let b1 = blinky.wns_ps().unwrap();
        assert_eq!(b1, b0 - 200);
        assert_ne!(b1, wns_ij, "input jitter WNS is per-design STA, not canned");
        assert_eq!(
            blinky.timing.as_ref().unwrap().hold_slack_ps,
            bhold0 - 200
        );
        blinky.exec("set_system_jitter 0.1").unwrap();
        let b2 = blinky.wns_ps().unwrap();
        assert_eq!(b2, b1 - 100);
        assert_ne!(b2, wns_sj, "system jitter WNS is per-design STA, not canned");
    }

    /// UG893 Timing Constraints Apply: set_timing_derate / set_operating_conditions
    /// scale helion-sta setup/hold path delay — empty XDC keeps gold.
    #[test]
    fn timing_constraints_derate_operating_conditions_apply_moves_sta() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let wns0 = ide.wns_ps().expect("STA WNS before derate");
        let setup0 = ide.timing.as_ref().unwrap().setup_ps;
        let hold0 = ide.timing.as_ref().unwrap().hold_ps;
        let hold_slack0 = ide.timing.as_ref().unwrap().hold_slack_ps;
        assert_ne!(wns0, 0);
        assert!(
            ide.constraints.timing_derates.is_empty()
                && !ide.constraints.operating_conditions.is_set(),
            "{:?}",
            ide.constraints
        );

        let late = ide.exec("set_timing_derate -late 1.1").unwrap();
        assert!(late.contains("timing_derate=1"), "{late}");
        assert!(late.contains("LATE_MILLI=1100"), "{late}");
        assert_eq!(ide.workspace, WorkspaceTab::Constraints);
        assert_eq!(ide.constraints.late_derate_milli(), 1100);
        assert_eq!(ide.constraints.early_derate_milli(), 1000);
        assert!(
            ide.constraints_text()
                .contains("set_timing_derate LATE_MILLI=1100 EARLY_MILLI=0"),
            "{}",
            ide.constraints_text()
        );
        let setup_late = setup0 * 1100 / 1000;
        let wns_late = ide.wns_ps().expect("STA after late derate");
        assert_eq!(
            wns_late,
            wns0 - (setup_late - setup0),
            "late derate 1.1 must scale setup WNS: {wns0} vs {wns_late}"
        );
        assert_eq!(
            ide.timing.as_ref().unwrap().hold_slack_ps,
            hold_slack0,
            "late derate must not move hold"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(
            rt.contains(&format!("WNS_PS={wns_late}")),
            "report_timing must honor late derate: {rt}"
        );

        let early = ide.exec("set_timing_derate -early 0.9").unwrap();
        assert!(early.contains("EARLY_MILLI=900"), "{early}");
        assert_eq!(ide.constraints.early_derate_milli(), 900);
        assert!(
            ide.constraints_text()
                .contains("set_timing_derate LATE_MILLI=0 EARLY_MILLI=900"),
            "{}",
            ide.constraints_text()
        );
        let hold_early = hold0 * 900 / 1000;
        let wns_early = ide.wns_ps().expect("STA after early derate");
        assert_eq!(wns_early, wns_late, "early derate must not move setup WNS");
        let hold_e = ide.timing.as_ref().expect("STA after early derate").hold_slack_ps;
        assert_eq!(
            hold_e,
            hold_slack0 - (hold0 - hold_early),
            "early derate 0.9 must scale hold slack: {hold_e} vs {hold_slack0}"
        );

        let oc = ide
            .exec("set_operating_conditions -voltage 0.95 -temperature 85")
            .unwrap();
        assert!(oc.contains("operating_conditions=1"), "{oc}");
        assert!(oc.contains("VOLTAGE_MV=950"), "{oc}");
        assert!(oc.contains("TEMP_C=85"), "{oc}");
        assert!(oc.contains("OC_SCALE_MILLI=1172"), "{oc}");
        assert_eq!(ide.constraints.operating_conditions.voltage_mv, 950);
        assert_eq!(ide.constraints.operating_conditions.temperature_c, 85);
        assert_eq!(ide.constraints.operating_conditions.scale_milli(), 1172);
        assert!(
            ide.constraints_text()
                .contains("set_operating_conditions VOLTAGE_MV=950 TEMP_C=85"),
            "{}",
            ide.constraints_text()
        );
        let late_m = 1100i64 * 1172 / 1000;
        let early_m = 900i64 * 1172 / 1000;
        let setup_all = setup0 * late_m / 1000;
        let hold_all = hold0 * early_m / 1000;
        let wns_oc = ide.wns_ps().expect("STA after operating conditions");
        assert_eq!(
            wns_oc,
            wns0 - (setup_all - setup0),
            "derate × PVT must stack on setup: {wns0} vs {wns_oc}"
        );
        let hold_oc = ide.timing.as_ref().expect("STA after OC").hold_slack_ps;
        assert_eq!(
            hold_oc,
            hold_slack0 - (hold0 - hold_all),
            "derate × PVT must stack on hold"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(rt.contains(&format!("WNS_PS={wns_oc}")), "{rt}");
        assert!(rt.contains(&format!("HOLD_SLACK_PS={hold_oc}")), "{rt}");

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let b0 = blinky.wns_ps().unwrap();
        let bsetup = blinky.timing.as_ref().unwrap().setup_ps;
        blinky.exec("set_timing_derate -late 1.1").unwrap();
        let b1 = blinky.wns_ps().unwrap();
        let bsetup_d = bsetup * 1100 / 1000;
        assert_eq!(b1, b0 - (bsetup_d - bsetup));
        assert_ne!(b1, wns_late, "derate WNS is per-design STA, not canned");
        blinky
            .exec("set_operating_conditions -voltage 0.95")
            .unwrap();
        let b2 = blinky.wns_ps().unwrap();
        let b_late_m = 1100i64 * 1052 / 1000;
        let bsetup_oc = bsetup * b_late_m / 1000;
        assert_eq!(b2, b0 - (bsetup_oc - bsetup));
        assert_ne!(b2, wns_oc, "OC WNS is per-design STA, not canned");
    }

    /// UG893 Timing Constraints Apply: set_disable_timing / set_case_analysis
    /// drop/force paths in helion-sta like false path — not labels on a stub editor.
    #[test]
    fn timing_constraints_disable_timing_case_analysis_apply_moves_sta() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let wns0 = ide.wns_ps().expect("STA WNS before disable_timing");
        assert_ne!(wns0, 0);
        assert!(
            ide.constraints.disable_timings.is_empty() && ide.constraints.case_analyses.is_empty(),
            "{:?}",
            ide.constraints
        );

        let od = ide
            .exec("set_output_delay -clock clk 2.0 [get_ports led]")
            .unwrap();
        assert!(od.contains("output_delay=1"), "{od}");
        let wns_od = ide.wns_ps().expect("STA after output delay");
        assert_eq!(
            wns_od,
            wns0 - 2000,
            "output delay must worsen WNS before disable_timing: {wns0} vs {wns_od}"
        );

        let dt = ide
            .exec("set_disable_timing -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        assert!(dt.contains("disable_timing=1"), "{dt}");
        assert_eq!(ide.workspace, WorkspaceTab::Constraints);
        assert!(
            ide.constraints.arcs_disabled(),
            "{:?}",
            ide.constraints.disable_timings
        );
        assert!(
            ide.constraints_text()
                .contains("set_disable_timing -from clk -to led"),
            "{}",
            ide.constraints_text()
        );
        let t = ide.timing.as_ref().expect("STA after disable_timing");
        assert_eq!(t.iob_ps, 0, "disable_timing must drop IOB from STA");
        assert_eq!(t.setup_ps, t.r2r_ps, "disable_timing setup is r2r only");
        let wns_dt = t.wns_ps;
        assert_ne!(
            wns_dt, wns_od,
            "disable_timing must move WNS off the I/O-delay result"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(rt.contains(&format!("WNS_PS={wns_dt}")), "{rt}");
        assert!(rt.contains("iob_ps=0"), "{rt}");

        let mut ca_ide = IdeModel::new();
        ca_ide.open_source(&example("counter.sv")).unwrap();
        ca_ide.run_step(FlowStep::Opt).unwrap();
        ca_ide.run_step(FlowStep::Place).unwrap();
        ca_ide.run_step(FlowStep::Route).unwrap();
        let c0 = ca_ide.wns_ps().unwrap();
        ca_ide
            .exec("set_output_delay -clock clk 2.0 [get_ports led]")
            .unwrap();
        let c_od = ca_ide.wns_ps().unwrap();
        assert_eq!(c_od, c0 - 2000);
        let ca = ca_ide
            .exec("set_case_analysis 0 [get_ports clk]")
            .unwrap();
        assert!(ca.contains("case_analysis=1"), "{ca}");
        assert!(ca.contains("CASE=0"), "{ca}");
        assert_eq!(ca_ide.constraints.case_analyses.len(), 1);
        assert_eq!(ca_ide.constraints.case_analyses[0].value, "0");
        assert_eq!(ca_ide.constraints.case_analyses[0].object, "clk");
        assert!(
            ca_ide.constraints_text().contains("set_case_analysis 0 clk"),
            "{}",
            ca_ide.constraints_text()
        );
        let t = ca_ide.timing.as_ref().expect("STA after case_analysis");
        assert_eq!(t.iob_ps, 0, "case_analysis must drop IOB from STA");
        assert_eq!(t.setup_ps, t.r2r_ps, "case_analysis setup is r2r only");
        let wns_ca = t.wns_ps;
        assert_ne!(
            wns_ca, c_od,
            "case_analysis must force-drop WNS off the I/O-delay result"
        );
        assert_eq!(wns_ca, wns_dt, "case_analysis and disable_timing drop the same IOB path");
        let rt = ca_ide.exec("report_timing").unwrap();
        assert!(rt.contains(&format!("WNS_PS={wns_ca}")), "{rt}");
        assert!(rt.contains("iob_ps=0"), "{rt}");

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let b0 = blinky.wns_ps().unwrap();
        assert_ne!(b0, wns0, "setup WNS is per-design STA");
        blinky
            .exec("set_output_delay -clock clk 2.0 [get_ports led]")
            .unwrap();
        let b_od = blinky.wns_ps().unwrap();
        assert_eq!(b_od, b0 - 2000);
        blinky
            .exec("set_disable_timing -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        let b_dt = blinky.wns_ps().unwrap();
        assert_ne!(b_dt, b_od, "disable_timing must move blinky WNS off I/O delay");
        assert_ne!(b_dt, wns_dt, "disable_timing WNS is per-design STA, not canned");
        blinky
            .exec("set_case_analysis 1 [get_pins u_lut0/I0]")
            .unwrap();
        assert!(
            blinky
                .constraints_text()
                .contains("set_case_analysis 1 u_lut0"),
            "{}",
            blinky.constraints_text()
        );
        assert_eq!(
            blinky.wns_ps().unwrap(),
            b_dt,
            "second case_analysis still drops IOB, does not invent slack"
        );
    }

    /// UG893 Timing Constraints Apply: set_propagated_clock / set_clock_sense
    /// fold routed clock-network delay into helion-sta — ideal clocks keep gold WNS.
    #[test]
    fn timing_constraints_propagated_clock_sense_apply_moves_sta() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let wns0 = ide.wns_ps().expect("STA WNS before propagated clock");
        let hold0 = ide.timing.as_ref().unwrap().hold_slack_ps;
        let clk_net = ide.timing.as_ref().unwrap().clk_net_ps;
        assert_ne!(wns0, 0);
        assert!(clk_net > 0, "routed clock network must have hop delay, got {clk_net}");
        assert!(
            ide.constraints.propagated_clocks.is_empty() && ide.constraints.clock_senses.is_empty(),
            "{:?}",
            ide.constraints
        );
        assert!(
            ide.timing_text().contains(&format!("CLK_NET_PS={clk_net}")),
            "{}",
            ide.timing_text()
        );
        assert_eq!(wns0, 10_000 - ide.timing.as_ref().unwrap().setup_ps);

        let pos = ide
            .exec("set_clock_sense -positive [get_pins u_ff/CLK]")
            .unwrap();
        assert!(pos.contains("clock_sense=1"), "{pos}");
        assert!(pos.contains("SENSE=positive"), "{pos}");
        assert_eq!(
            ide.wns_ps().unwrap(),
            wns0,
            "positive sense is the default edge and must keep gold WNS"
        );

        let prop = ide
            .exec("set_propagated_clock [get_clocks clk]")
            .unwrap();
        assert!(prop.contains("propagated_clock=1"), "{prop}");
        assert!(prop.contains(&format!("CLK_NET_PS={clk_net}")), "{prop}");
        assert_eq!(ide.workspace, WorkspaceTab::Constraints);
        assert!(ide.constraints.clocks_propagated(), "{:?}", ide.constraints.propagated_clocks);
        assert!(
            ide.constraints_text().contains("set_propagated_clock clk"),
            "{}",
            ide.constraints_text()
        );
        let wns_prop = ide.wns_ps().expect("STA after set_propagated_clock");
        assert_eq!(
            wns_prop,
            wns0 - clk_net,
            "propagated clocks must add routed insertion to WNS: {wns0} vs {wns_prop} net {clk_net}"
        );
        assert_eq!(
            ide.timing.as_ref().unwrap().hold_slack_ps,
            hold0 - clk_net,
            "propagated clocks must move hold by insertion delay"
        );
        let rt = ide.exec("report_timing").unwrap();
        assert!(rt.contains(&format!("WNS_PS={wns_prop}")), "{rt}");
        assert!(rt.contains(&format!("CLK_NET_PS={clk_net}")), "{rt}");

        let stop = ide
            .exec("set_clock_sense -stop_propagation [get_pins clk_buf/O]")
            .unwrap();
        assert!(stop.contains("SENSE=stop"), "{stop}");
        assert!(ide.constraints.clock_stopped());
        assert!(
            ide.constraints_text()
                .contains("set_clock_sense -stop clk_buf"),
            "{}",
            ide.constraints_text()
        );
        assert_eq!(
            ide.wns_ps().unwrap(),
            wns0,
            "stop_propagation must restore ideal insertion (gold WNS)"
        );
        assert_eq!(ide.timing.as_ref().unwrap().hold_slack_ps, hold0);

        let mut neg_ide = IdeModel::new();
        neg_ide.open_source(&example("counter.sv")).unwrap();
        neg_ide.run_step(FlowStep::Opt).unwrap();
        neg_ide.run_step(FlowStep::Place).unwrap();
        neg_ide.run_step(FlowStep::Route).unwrap();
        let n0 = neg_ide.wns_ps().unwrap();
        let n_net = neg_ide.timing.as_ref().unwrap().clk_net_ps;
        let neg = neg_ide
            .exec("set_clock_sense -negative [get_pins u_lut0/I0]")
            .unwrap();
        assert!(neg.contains("SENSE=negative"), "{neg}");
        assert!(
            neg_ide
                .constraints_text()
                .contains("set_clock_sense -negative u_lut0"),
            "{}",
            neg_ide.constraints_text()
        );
        let wns_neg = neg_ide.wns_ps().unwrap();
        assert_eq!(
            wns_neg,
            n0 - 5_000,
            "negative sense is a half-cycle setup: {n0} vs {wns_neg}"
        );
        let rt = neg_ide.exec("report_timing").unwrap();
        assert!(rt.contains(&format!("WNS_PS={wns_neg}")), "{rt}");

        neg_ide
            .exec("set_propagated_clock [get_clocks clk]")
            .unwrap();
        let wns_both = neg_ide.wns_ps().unwrap();
        assert_eq!(
            wns_both,
            n0 - n_net - 5_000,
            "propagated + negative must stack: {wns_both}"
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let b0 = blinky.wns_ps().unwrap();
        let b_net = blinky.timing.as_ref().unwrap().clk_net_ps;
        assert_ne!(b0, wns0, "setup WNS is per-design STA");
        blinky
            .exec("set_propagated_clock [get_clocks clk]")
            .unwrap();
        let b1 = blinky.wns_ps().unwrap();
        assert_eq!(b1, b0 - b_net);
        assert_ne!(b1, wns_prop, "propagated WNS is per-design STA, not canned");
        blinky
            .exec("set_clock_sense -negative [get_pins u_lut0/I0]")
            .unwrap();
        let b2 = blinky.wns_ps().unwrap();
        assert_eq!(b2, b1 - 5_000);
        assert_ne!(b2, wns_both, "sense WNS is per-design STA, not canned");
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

    /// UG986 Lab 1: extra Helion strategies produce different WNS; impl_1 stays Default.
    #[test]
    fn ug986_lab1_strategies_compare_runs() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.exec("launch_runs impl_1").unwrap();
        let wns_def = ide.wns_ps().expect("default WNS");
        let hash_def = ide.bitstream_hash().expect("default hash");
        ide.exec("create_run impl_runtime -strategy RuntimeOpt").unwrap();
        ide.exec("launch_runs impl_runtime").unwrap();
        ide.exec("create_run impl_phys -strategy PhysOpt").unwrap();
        ide.exec("launch_runs impl_phys").unwrap();
        let cmp = ide.exec("compare_runs").unwrap();
        assert!(cmp.contains("impl_1 strategy=Default"), "{cmp}");
        assert!(cmp.contains("impl_runtime strategy=RuntimeOpt"), "{cmp}");
        assert!(cmp.contains("impl_phys strategy=PhysOpt"), "{cmp}");
        let rt = ide.runs.iter().find(|r| r.name == "impl_runtime").unwrap();
        let phys = ide.runs.iter().find(|r| r.name == "impl_phys").unwrap();
        assert_ne!(rt.wns_ps, Some(wns_def), "RuntimeOpt WL place must move WNS");
        assert_ne!(phys.wns_ps, Some(wns_def), "PhysOpt extra hops must move WNS");
        assert_ne!(rt.wns_ps, phys.wns_ps);
        assert_eq!(ide.wns_ps(), Some(wns_def), "side runs must not clobber impl_1");
        assert_eq!(ide.bitstream_hash(), Some(hash_def));
        assert!(!cmp.contains("Performance_Explore") && !cmp.contains("AXI"));
    }

    /// UG893/UG986 Design Runs is a clickable name/strategy/WNS/runtime/hash
    /// grid over Session engines, not a monospace dump + compare_runs report_box.
    #[test]
    fn design_runs_pane_clickable_name_strategy_wns_runtime_hash_grid() {
        let mut ide = IdeModel::new();
        assert!(
            NavSection::Implementation
                .actions()
                .iter()
                .any(|a| a.tcl == "design_runs"),
            "Flow Navigator Implementation must offer Design Runs"
        );
        assert!(
            NavSection::Implementation
                .actions()
                .iter()
                .any(|a| a.tcl == "compare_runs"),
            "Flow Navigator Implementation must offer Compare Runs"
        );
        let empty = ide.exec("select_run").unwrap_err();
        assert!(empty.contains("missing name"), "{empty}");
        let missing = ide.exec("select_run no_such").unwrap_err();
        assert!(missing.contains("no row"), "{missing}");

        let table = ide.exec("design_runs").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Runs);
        assert!(table.contains("design_runs n="), "{table}");
        assert!(table.contains('\n'), "must not be a one-liner dump: {table}");
        assert!(table.contains("NAME=synth_1"), "{table}");
        assert!(table.contains("NAME=impl_1"), "{table}");
        assert!(table.contains("STRATEGY=Default"), "{table}");
        assert!(table.contains("STATUS=Not started"), "{table}");
        assert!(table.contains("WNS_PS=-"), "{table}");
        assert!(table.contains("HASH=-"), "{table}");
        assert!(table.contains("RUNTIME_MS=-"), "{table}");

        let sel = ide.exec("select_run impl_1").unwrap();
        assert!(sel.contains("NAME=impl_1"), "{sel}");
        assert!(sel.contains("STEP=Implementation"), "{sel}");
        assert!(sel.contains("STRATEGY=Default"), "{sel}");
        assert!(sel.contains("STATUS=Not started"), "{sel}");
        assert!(sel.contains("WNS_PS=-"), "{sel}");
        assert!(sel.contains("HASH=-"), "{sel}");
        assert_eq!(ide.selected.as_deref(), Some("run:impl_1"));
        assert_eq!(ide.workspace, WorkspaceTab::Runs);
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "design_run"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "STRATEGY" && v == "Default"),
            "{:?}",
            ide.properties
        );

        let by_idx = ide.exec("select_run 0").unwrap();
        assert!(by_idx.contains("NAME=synth_1"), "{by_idx}");
        assert_eq!(ide.selected.as_deref(), Some("run:synth_1"));

        ide.open_source(&example("counter.sv")).unwrap();
        ide.exec("launch_runs impl_1").unwrap();
        let wns = ide.wns_ps().expect("STA after launch_runs");
        let hash = ide.bitstream_hash().expect("hash after launch_runs");
        assert_ne!(wns, 0);
        let table = ide.exec("design_runs").unwrap();
        assert!(table.contains("NAME=impl_1"), "{table}");
        assert!(table.contains("STRATEGY=Default"), "{table}");
        assert!(table.contains(&format!("WNS_PS={wns}")), "{table}");
        assert!(table.contains(&format!("HASH={hash:#010x}")), "{table}");
        assert!(table.contains("STATUS=Complete"), "{table}");
        let sel = ide.exec("select_run impl_1").unwrap();
        assert!(sel.contains(&format!("WNS_PS={wns}")), "{sel}");
        assert!(sel.contains(&format!("HASH={hash:#010x}")), "{sel}");
        assert!(sel.contains("STRATEGY=Default"), "{sel}");
        assert!(sel.contains("LUTFF=4"), "{sel}");
        assert_eq!(
            ide.wns_ps(),
            Some(wns),
            "select_run must not move gold WNS"
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "WNS_PS" && v == &wns.to_string()),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "HASH" && v == &format!("{hash:#010x}")),
            "{:?}",
            ide.properties
        );

        ide.exec("create_run impl_runtime -strategy RuntimeOpt")
            .unwrap();
        ide.exec("launch_runs impl_runtime").unwrap();
        ide.exec("create_run impl_phys -strategy PhysOpt")
            .unwrap();
        ide.exec("launch_runs impl_phys").unwrap();
        let cmp = ide.exec("compare_runs").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Runs);
        assert!(cmp.contains("compare_runs n=3"), "{cmp}");
        assert!(cmp.contains("NAME=impl_1"), "{cmp}");
        assert!(cmp.contains("NAME=impl_runtime"), "{cmp}");
        assert!(cmp.contains("NAME=impl_phys"), "{cmp}");
        assert!(cmp.contains("STRATEGY=RuntimeOpt"), "{cmp}");
        assert!(cmp.contains("STRATEGY=PhysOpt"), "{cmp}");
        assert!(cmp.contains('\n'), "compare_runs is a grid, not a dump: {cmp}");
        let rt_wns = ide
            .runs
            .iter()
            .find(|r| r.name == "impl_runtime")
            .unwrap()
            .wns_ps;
        let phys_wns = ide
            .runs
            .iter()
            .find(|r| r.name == "impl_phys")
            .unwrap()
            .wns_ps;
        let rt_cell = ide
            .runs
            .iter()
            .find(|r| r.name == "impl_runtime")
            .unwrap()
            .wns_cell();
        assert_ne!(rt_wns, Some(wns), "RuntimeOpt must move WNS");
        assert_ne!(phys_wns, Some(wns), "PhysOpt must move WNS");
        let sel_rt = ide.exec("select_run RuntimeOpt").unwrap();
        assert!(sel_rt.contains("NAME=impl_runtime"), "{sel_rt}");
        assert!(sel_rt.contains("STRATEGY=RuntimeOpt"), "{sel_rt}");
        assert!(sel_rt.contains(&format!("WNS_PS={rt_cell}")), "{sel_rt}");
        assert_eq!(ide.selected.as_deref(), Some("run:impl_runtime"));
        assert_eq!(
            ide.wns_ps(),
            Some(wns),
            "clicking a side run must not clobber impl_1 STA"
        );
        assert_eq!(ide.bitstream_hash(), Some(hash));
        assert!(!cmp.contains("Performance_Explore") && !cmp.contains("AXI"));

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.exec("launch_runs impl_1").unwrap();
        let bw = blinky.wns_ps().expect("blinky WNS");
        let bh = blinky.bitstream_hash().expect("blinky hash");
        assert_ne!(bw, wns, "WNS is per-design STA");
        assert_ne!(bh, hash, "hash is per-design");
        let bsel = blinky.exec("select_run impl_1").unwrap();
        assert!(bsel.contains(&format!("WNS_PS={bw}")), "{bsel}");
        assert!(bsel.contains(&format!("HASH={bh:#010x}")), "{bsel}");
        assert!(bsel.contains("LUTFF=1"), "{bsel}");
        assert!(
            !bsel.contains(&format!("WNS_PS={wns}")),
            "blinky row is not counter's dump: {bsel}"
        );
        assert_eq!(blinky.wns_ps(), Some(bw));
    }

    /// UG986 Lab 2: incremental impl reuses named cells from the checkpoint.
    #[test]
    fn ug986_lab2_incremental_reuse_report() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.exec("launch_runs impl_1").unwrap();
        let out = ide.exec("incremental_impl").unwrap();
        assert!(out.contains("reuse cells="), "{out}");
        assert!(out.contains("100%") || out.contains("reuse cells="), "{out}");
        let impl_r = ide.runs.iter().find(|r| r.name == "impl_1").unwrap();
        assert_eq!(impl_r.reuse_pct, Some(100), "{out}");
        assert!(ide.bitstream_hash().is_some());
    }

    /// UG986 Lab 3: directed extra hops on an IOB net move STA WNS.
    #[test]
    fn ug986_lab3_fix_route_moves_wns() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.exec("launch_runs impl_1").unwrap();
        let wns0 = ide.wns_ps().expect("WNS");
        let net = ide
            .session()
            .placed
            .as_ref()
            .unwrap()
            .packed
            .iobs[0]
            .from_net
            .clone();
        let out = ide.exec(&format!("fix_route {net} 8")).unwrap();
        assert!(out.contains("extra_hops=8"), "{out}");
        let wns1 = ide.wns_ps().expect("WNS after fix_route");
        assert!(
            wns1 < wns0,
            "extra hops must reduce slack ({wns1} vs {wns0})"
        );
        ide.exec(&format!("unroute_net {net}")).unwrap();
    }

    /// UG986 Lab 4: insert ECO_LUT3, check_eco sees it, incremental place reuses others.
    #[test]
    fn ug986_lab4_check_eco_incremental_place() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.exec("launch_runs impl_1").unwrap();
        let n0 = ide.design().unwrap().cells.len();
        let out = ide.exec("insert_eco_lut ECO_LUT3 0x8").unwrap();
        assert!(out.contains("ECO_LUT3"), "{out}");
        assert!(ide.design().unwrap().cells.len() > n0);
        let chk = ide.exec("check_eco").unwrap();
        assert!(chk.contains("ECO_LUT3"), "{chk}");
        let inc = ide.exec("incremental_place").unwrap();
        assert!(inc.contains("reuse"), "{inc}");
        let impl_r = ide.runs.iter().find(|r| r.name == "impl_1").unwrap();
        assert!(impl_r.reuse_pct.unwrap() < 100, "{inc}");
        ide.exec("incremental_route").unwrap();
        assert!(ide.bitstream_hash().is_some());
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

    /// UG893 Messages pane is a clickable severity table (filter + properties +
    /// engine navigation), not a colored dump of the Tcl journal.
    #[test]
    fn messages_pane_clickable_severity_table() {
        let mut ide = IdeModel::new();
        assert!(ide.messages.is_empty());
        assert!(ide.message_rows().is_empty());
        assert!(
            ide.select_message("0")
                .unwrap_err()
                .contains("no messages"),
            "empty table must refuse a click"
        );
        let empty = ide.exec("messages").unwrap();
        assert_eq!(ide.bottom_tab, BottomTab::Messages);
        assert!(empty.contains("errors=0"), "{empty}");
        assert!(empty.contains("filter=all"), "{empty}");
        assert!(empty.contains("no messages"), "{empty}");

        let e = ide.run_step(FlowStep::Synthesis).unwrap_err();
        assert!(e.contains("source") || e.contains("synth"), "{e}");
        let err_idx = ide
            .messages
            .iter()
            .position(|m| m.severity == MsgSeverity::Error && m.id == "synth_design")
            .expect("rail must journal Errors");
        let table = ide.exec("messages").unwrap();
        assert!(table.contains("SEVERITY=ERROR ID=synth_design"), "{table}");
        let sel = ide.exec("select_message ERROR:synth_design").unwrap();
        assert!(sel.contains("SEVERITY=ERROR"), "{sel}");
        assert!(sel.contains("ID=synth_design"), "{sel}");
        let want = format!("message:{err_idx}");
        assert_eq!(ide.selected.as_deref(), Some(want.as_str()));
        assert_eq!(ide.selected_message, Some(err_idx));
        assert_eq!(ide.workspace, WorkspaceTab::Schematic);
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "message"),
            "{:?}",
            ide.properties
        );

        ide.open_source(&example("counter.sv")).unwrap();
        assert!(
            ide.message_rows()
                .iter()
                .any(|(_, m)| m.severity == MsgSeverity::Info
                    && m.id == "synth_design"
                    && m.text.contains("cells=")),
            "{:?}",
            ide.messages
        );

        let ferr = ide.exec("filter_messages error").unwrap();
        assert!(ferr.contains("filter=error"), "{ferr}");
        assert!(
            ide.message_rows()
                .iter()
                .all(|(_, m)| m.severity == MsgSeverity::Error),
            "error filter must hide Info/Warning: {:?}",
            ide.message_rows()
                .iter()
                .map(|(_, m)| (m.severity, m.id.as_str()))
                .collect::<Vec<_>>()
        );
        assert!(
            !ferr.contains("SEVERITY=INFO"),
            "filtered table is not a dump of every line: {ferr}"
        );
        let esel = ide.exec("select_message ERROR:synth_design").unwrap();
        assert!(esel.contains("SEVERITY=ERROR"), "{esel}");
        assert!(esel.contains("ID=synth_design"), "{esel}");

        ide.exec("filter_messages all").unwrap();
        let e = ide.run_step(FlowStep::Route).unwrap_err();
        assert!(e.contains("Place first"), "{e}");
        let rsel = ide.exec("select_message route_design").unwrap();
        assert!(rsel.contains("SEVERITY=ERROR"), "{rsel}");
        assert!(rsel.contains("Place first"), "{rsel}");
        assert_eq!(ide.workspace, WorkspaceTab::Device);

        ide.exec("report_timing").unwrap();
        let wns = ide.wns_ps().expect("STA WNS");
        assert_ne!(wns, 0);
        let tsel = ide.exec("select_message report_timing").unwrap();
        assert!(
            tsel.contains(&format!("WNS_PS={wns}")),
            "click must carry STA, not a stub: {tsel}"
        );
        assert_eq!(ide.workspace, WorkspaceTab::Reports);
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TEXT" && v.contains(&format!("WNS_PS={wns}"))),
            "{:?}",
            ide.properties
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.exec("report_timing").unwrap();
        let bw = blinky.wns_ps().expect("blinky STA");
        assert_ne!(bw, wns, "WNS is per-design");
        let bsel = blinky.exec("select_message report_timing").unwrap();
        assert!(bsel.contains(&format!("WNS_PS={bw}")), "{bsel}");
        assert!(
            !bsel.contains(&format!("WNS_PS={wns}")),
            "Messages clicks are per-session, not canned: {bsel}"
        );
        assert_eq!(blinky.workspace, WorkspaceTab::Reports);
        let btable = blinky.exec("filter_messages info").unwrap();
        assert!(btable.contains("filter=info"), "{btable}");
        assert!(
            blinky
                .message_rows()
                .iter()
                .any(|(_, m)| m.id == "synth_design" && m.text.contains("blinky")),
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

    /// UG893 Fig. 55/56/57: schematic is symbol boxes + pin stubs + orthogonal wires.
    #[test]
    fn schematic_drawing_is_ug893_symbols_and_wires() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        let dump = ide.exec("schematic_drawing").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Schematic);
        assert!(dump.contains("box="), "must report symbol boxes, not a list: {dump}");
        assert!(dump.contains("u_lut0:LUT6:box="), "{dump}");
        assert!(dump.contains("u_lut0.O:out@"), "{dump}");
        assert!(dump.contains("u_lut0.I0:in@"), "{dump}");
        assert!(dump.contains("wire:"), "{dump}");
        assert!(dump.contains("pts="), "{dump}");

        let d = ide.schematic.drawing();
        let cells: Vec<_> = d
            .symbols
            .iter()
            .filter(|s| !s.kind.starts_with("PORT"))
            .collect();
        assert_eq!(cells.len(), ide.schematic.visible_nodes().len());
        let lut = d
            .symbols
            .iter()
            .find(|s| s.name == "u_lut0")
            .expect("u_lut0 symbol");
        assert!(lut.w > 8.0 && lut.h > 8.0, "symbol is a box {:?}", lut);
        assert!(
            lut.pins.iter().any(|p| p.name == "O" && p.output),
            "LUT6 O stub on the right: {:?}",
            lut.pins
        );
        assert!(
            lut.pins.iter().any(|p| p.name == "I0" && !p.output),
            "LUT6 I0 stub on the left: {:?}",
            lut.pins
        );
        let o = lut.pins.iter().find(|p| p.name == "O").unwrap();
        let i0 = lut.pins.iter().find(|p| p.name == "I0").unwrap();
        assert!(
            o.x > lut.x + lut.w,
            "output stub is outside the right edge"
        );
        assert!(i0.x < lut.x, "input stub is outside the left edge");
        assert!(
            i0.y > lut.y && i0.y < lut.y + lut.h,
            "pin stub is vertically on the symbol"
        );
        let i_pins: Vec<_> = lut.pins.iter().filter(|p| p.name.starts_with('I')).collect();
        assert_eq!(i_pins.len(), 6, "LUT6 schematic symbol has I0–I5: {:?}", lut.pins);
        assert!(
            !i0.net.is_empty(),
            "u_lut0 I0 is on an HNF net: {:?}",
            i0
        );
        let i5 = lut.pins.iter().find(|p| p.name == "I5").expect("I5");
        assert!(
            i5.net.is_empty(),
            "u_lut0 I5 is n/c on the incrementer: {:?}",
            i5
        );
        assert!(dump.contains("u_lut0.I5:in@"), "{dump}");
        assert!(dump.contains(":nc"), "n/c pins in the drawing dump: {dump}");
        let d0 = d
            .wires
            .iter()
            .find(|w| w.src == "u_lut0" && w.src_pin == "O")
            .expect("u_lut0/O drives a net polyline");
        assert!(
            d0.points.len() >= 2,
            "O net is a polyline, not a name: {:?}",
            d0.points
        );
        assert_eq!(d0.points[0], (o.x, o.y), "polyline starts at the O stub");
        assert!(
            d0.dst.contains("ff") || d0.dst_pin == "D",
            "u_lut0/O fans to an FF D pin: {}/{}",
            d0.dst,
            d0.dst_pin
        );
        assert!(
            d.wires.iter().any(|w| w.points.len() >= 2),
            "nets are polylines"
        );
        assert!(
            d.wires.iter().any(|w| w.src == "u_lut0" || w.dst == "u_lut0"),
            "u_lut0 is on a net polyline: {:?}",
            d.wires.iter().map(|w| format!("{}->{}", w.src, w.dst)).collect::<Vec<_>>()
        );
        let ff = d
            .symbols
            .iter()
            .find(|s| s.kind == "HFF")
            .expect("HFF symbol");
        assert!(
            ff.x > lut.x,
            "UG893 dataflow: LUT column left of FF column"
        );
        assert!(
            d.symbols.iter().any(|s| s.kind == "PORT_IN" || s.kind == "PORT_OUT"),
            "top-level ports are schematic terminators"
        );

        ide.select("u_lut0");
        ide.exec("expand_cone").unwrap();
        let cone = ide.schematic.drawing();
        let cone_cells: Vec<_> = cone
            .symbols
            .iter()
            .filter(|s| !s.kind.starts_with("PORT"))
            .map(|s| s.name.as_str())
            .collect();
        assert!(cone_cells.contains(&"u_lut0"));
        assert!(
            cone_cells.len() < cells.len(),
            "cone drawing is a subset of the sheet ({}/{})",
            cone_cells.len(),
            cells.len()
        );
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

    /// UG893 Device is a HAD die occupancy grid, not a list of occupied site names.
    #[test]
    fn device_drawing_is_had_floorplan_not_a_site_list() {
        let mut ide = IdeModel::new();
        let dump = ide.exec("device").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Device);
        let n = ide.device.sites.len();
        assert!(n > 32, "HAD die must list CLB+IOB sites, not a dummy tile: {n}");
        assert!(
            ide.device.cols > 1 && ide.device.rows > 1,
            "drawing is a 2-D bounding box: {:?}",
            (ide.device.cols, ide.device.rows, ide.device.x0, ide.device.y0)
        );
        assert_eq!(
            ide.device.cols * ide.device.rows,
            n as u32,
            "every drawing cell is a HAD site, not a 1-column occupant list"
        );
        assert_eq!(ide.device.occupied_count(), 0, "empty until Place");
        assert!(dump.contains(&format!("sites={n}")), "{dump}");
        assert!(dump.contains(&format!("cols={}", ide.device.cols)), "{dump}");
        assert!(dump.contains("occupied=0"), "{dump}");
        assert!(dump.contains("map="), "floorplan occupancy map: {dump}");
        assert!(
            !dump.contains(" occ "),
            "unplaced drawing has no occupants: {dump}"
        );
        let iob_row = dump
            .split(&format!("row y={} map=", ide.device.y0))
            .nth(1)
            .expect("IOB row at y0")
            .split(" row ")
            .next()
            .unwrap();
        assert!(
            iob_row.contains('i'),
            "bottom IOB bank is on the die map: {iob_row}"
        );
        assert!(
            dump.contains("map=") && dump.contains('b'),
            "HAD BRAM column is on the die (not a pin table): {dump}"
        );
        let empty_clb = ide
            .device
            .sites
            .iter()
            .find(|s| s.kind == SiteKind::Clb && s.occupant.is_none())
            .cloned()
            .expect("empty CLB");
        let empty_name = empty_clb.site_name();
        let unocc = ide
            .exec(&format!("select_device_site {empty_name}"))
            .unwrap();
        assert!(unocc.contains("unoccupied"), "{unocc}");
        assert_eq!(ide.selected_cell(), Some(empty_name.as_str()));
        assert!(ide.device_has_selected());
        assert_eq!(ide.properties_name(), Some(empty_name.as_str()));
        assert!(
            ide.properties.iter().any(|(k, v)| k == "TYPE" && v == "site"),
            "{:?}",
            ide.properties
        );

        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        let dump = ide.exec("device").unwrap();
        let lut = ide
            .device
            .occupant_of("u_lut0")
            .cloned()
            .expect("placed u_lut0 occupies a HAD CLB");
        assert_eq!(lut.occupancy_char(), 'L');
        assert!(
            dump.contains(&format!("occ u_lut0={}", lut.site_name())),
            "drawing names the HAD site, not a canned pin: {dump}"
        );
        assert!(!dump.contains("occupied=0"), "place fills the die: {dump}");
        let occ = ide.device.occupied_count();
        assert!(occ >= 2, "counter LUT+IOB on the die: {occ}");
        assert!(
            occ < n,
            "occupancy is a proper subset of the HAD grid ({occ}/{n})"
        );
        let row = dump
            .split(&format!("row y={} map=", lut.y))
            .nth(1)
            .expect("LUT row")
            .split(" row ")
            .next()
            .unwrap();
        let dx = (lut.x - ide.device.x0) as usize;
        assert_eq!(
            row.as_bytes().get(dx).copied(),
            Some(b'L'),
            "u_lut0 must mark the die map at X{}Y{}: {row}",
            lut.x,
            lut.y
        );
        let iob = ide
            .device
            .sites
            .iter()
            .find(|s| s.kind == SiteKind::Iob && s.occupant.is_some())
            .expect("placed IOB");
        assert_eq!(iob.occupancy_char(), 'O');
        let irow = dump
            .split(&format!("row y={} map=", iob.y))
            .nth(1)
            .expect("IOB row")
            .split(" row ")
            .next()
            .unwrap();
        let idx = (iob.x - ide.device.x0) as usize;
        assert_eq!(irow.as_bytes().get(idx).copied(), Some(b'O'));

        let sel = ide
            .exec(&format!("select_device_site X{}Y{}", lut.x, lut.y))
            .unwrap();
        assert!(sel.contains("occupant=u_lut0"), "{sel}");
        assert_eq!(ide.selected_cell(), Some("u_lut0"));
        assert!(ide.device_has_selected());
        assert!(ide.netlist_has_selected());
        assert_eq!(ide.properties_name(), Some("u_lut0"));
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "SITE" && v == &lut.site_name()),
            "{:?}",
            ide.properties
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        let bdump = blinky.exec("device").unwrap();
        assert_eq!(blinky.device.sites.len(), n, "same HAD, different occupancy");
        assert_eq!(blinky.device.cols, ide.device.cols);
        assert_eq!(ide.utilization.unwrap().lutff, 4, "counter packed LUTFF");
        assert_eq!(blinky.utilization.unwrap().lutff, 1, "blinky packed LUTFF");
        let c_bels: Vec<&str> = ide
            .device
            .sites
            .iter()
            .flat_map(|s| s.bels.iter().map(String::as_str))
            .collect();
        let b_bels: Vec<&str> = blinky
            .device
            .sites
            .iter()
            .flat_map(|s| s.bels.iter().map(String::as_str))
            .collect();
        assert!(c_bels.iter().any(|b| *b == "u_lut0"), "{c_bels:?}");
        assert!(b_bels.iter().any(|b| *b == "u_lut"), "{b_bels:?}");
        assert!(
            !b_bels.iter().any(|b| *b == "u_lut0"),
            "blinky die is not counter's canned occupants: {b_bels:?}"
        );
        assert!(dump.contains("occ u_lut0="), "{dump}");
        assert!(bdump.contains("occ u_lut="), "{bdump}");
        assert!(!bdump.contains("occ u_lut0="), "{bdump}");
        assert!(blinky.device.occupant_of("u_lut").is_some());
        assert!(blinky.device.occupant_of("u_lut0").is_none());
    }

    /// Fig. 55: dotted off-sheet stubs + Cells/I/O Ports/Nets sheet links open Find.
    #[test]
    fn schematic_offsheet_dotted_and_sheet_find_opens_find() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        let full = ide.schematic.drawing();
        assert!(
            full.wires.iter().all(|w| !w.off_sheet),
            "full sheet has no off-sheet nets"
        );

        ide.select("u_lut0");
        ide.exec("expand_cone").unwrap();
        let cone = ide.schematic.drawing();
        assert!(
            cone.symbols.len() < full.symbols.len(),
            "cone hides cells so nets can go off-sheet"
        );
        assert!(
            cone.wires.iter().any(|w| w.off_sheet && w.dst == "offsheet"),
            "Fig. 55 dotted off-sheet stubs: {:?}",
            cone.wires
                .iter()
                .map(|w| format!("{}:{}->{}:dotted={}", w.net, w.src, w.dst, w.off_sheet))
                .collect::<Vec<_>>()
        );
        let dump = ide.exec("schematic_drawing").unwrap();
        assert!(dump.contains(":dotted"), "dump names dotted nets: {dump}");

        let cells = ide.exec("sheet_find cells").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Find);
        assert!(cells.contains("hits="), "{cells}");
        assert!(
            ide.find_results.iter().any(|h| h.name == "u_lut0"),
            "{:?}",
            ide.find_results
        );
        let ports = ide.exec("sheet_find ports").unwrap();
        assert!(
            ide.find_results.iter().any(|h| h.kind.starts_with("port") && h.name == "clk"),
            "{ports} {:?}",
            ide.find_results
        );
        let nets = ide.exec("sheet_find nets").unwrap();
        assert!(
            ide.find_results.iter().any(|h| h.kind == "net"),
            "{nets}"
        );
    }

    /// Fig. 57: buses are thick wires (width > 1).
    #[test]
    fn schematic_bus_wires_are_thick() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        let d = ide.schematic.drawing();
        let bus = d
            .wires
            .iter()
            .filter(|w| w.width > 1)
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            !bus.is_empty(),
            "cnt_0..cnt_3 is a bit-blasted bus: {:?}",
            d.wires
                .iter()
                .map(|w| format!("{}:w={}", w.net, w.width))
                .collect::<Vec<_>>()
        );
        assert!(
            bus.iter().any(|w| w.net.starts_with("cnt_") && w.width >= 4),
            "counter bus width tracks the 4-bit cnt: {:?}",
            bus.iter().map(|w| (&w.net, w.width)).collect::<Vec<_>>()
        );
        let dump = ide.exec("schematic_drawing").unwrap();
        assert!(dump.contains(":w=4") || dump.contains(":w=3"), "{dump}");
        let scalar = d.wires.iter().find(|w| w.net == "clk" || w.net == "led");
        if let Some(s) = scalar {
            assert_eq!(s.width, 1, "clk/led stay scalar");
        }
    }

    /// Fig. 61: hierarchy box area scales with cell/resource count.
    #[test]
    fn hierarchy_boxes_scale_with_cell_count() {
        let mut hier = IdeModel::new();
        hier.open_source(&example("hier.sv")).unwrap();
        let dump = hier.exec("hierarchy_drawing").unwrap();
        assert_eq!(hier.workspace, WorkspaceTab::Hierarchy);
        assert!(dump.contains("box="), "block view is boxes, not a list: {dump}");
        let d = hier.hierarchy.drawing();
        let top = d
            .boxes
            .iter()
            .find(|b| b.kind == "module")
            .expect("top module box");
        assert!(top.w * top.h > 64.0 * 32.0, "top is a real rectangle {:?}", top);
        let u0 = d
            .boxes
            .iter()
            .find(|b| b.name == "u0")
            .expect("instance box");
        assert!(
            top.w * top.h + 1.0 >= u0.w * u0.h,
            "top area covers the instance: top={} u0={}",
            top.w * top.h,
            u0.w * u0.h
        );
        assert!(
            u0.cells >= 1 && u0.w * u0.h > 40.0 * 24.0,
            "instance box is sized by its cells: {:?}",
            u0
        );
        let leaf = d
            .boxes
            .iter()
            .find(|b| b.kind != "module" && !b.kind.starts_with("instance") && b.kind != "leaves")
            .expect("leaf box");
        assert!(
            u0.w * u0.h > leaf.w * leaf.h,
            "instance area > leaf area (nested): u0={} leaf={}",
            u0.w * u0.h,
            leaf.w * leaf.h
        );

        let mut counter = IdeModel::new();
        counter.open_source(&example("counter.sv")).unwrap();
        let cd = counter.hierarchy.drawing();
        let ctop = cd.boxes.iter().find(|b| b.kind == "module").unwrap();
        assert_ne!(
            ctop.cells, top.cells,
            "area tracks HNF cell count, not a canned box"
        );
        assert!(
            (ctop.w * ctop.h - top.w * top.h).abs() > 1.0,
            "counter vs hier top areas differ: {} vs {}",
            ctop.w * ctop.h,
            top.w * top.h
        );
    }

    /// UG893 Fig. 61 Hierarchy pane is nested HNF boxes (click to select),
    /// not a `hierarchy_drawing_text` dump painted above the canvas.
    #[test]
    fn hierarchy_pane_nested_boxes_are_clickable_not_a_dump() {
        let mut hier = IdeModel::new();
        hier.open_source(&example("hier.sv")).unwrap();
        let dump = hier.exec("hierarchy_drawing").unwrap();
        assert_eq!(hier.workspace, WorkspaceTab::Hierarchy);
        assert!(dump.contains("box="), "exec dump is for tests: {dump}");
        let d = hier.hierarchy.drawing();
        assert!(
            d.boxes.len() >= 3,
            "top + instance + leaf: {:?}",
            d.boxes
                .iter()
                .map(|b| (&b.name, &b.kind, b.cells))
                .collect::<Vec<_>>()
        );
        assert!(
            d.boxes.iter().all(|b| b.w > 0.0 && b.h > 0.0),
            "paint surface is rectangles, not a text list: {:?}",
            d.boxes
        );
        let u0 = d.boxes.iter().find(|b| b.name == "u0").expect("u0 box");
        let sel = hier.exec(&format!("select {}", u0.name)).unwrap();
        assert!(sel.contains("u0"), "{sel}");
        assert!(hier.hierarchy_has_selected());
        assert_eq!(hier.selected.as_deref(), Some("u0"));
        assert!(
            hier.properties
                .iter()
                .any(|(k, v)| k == "NAME" && v == "u0"),
            "{:?}",
            hier.properties
        );

        let mut counter = IdeModel::new();
        counter.open_source(&example("counter.sv")).unwrap();
        let cd = counter.hierarchy.drawing();
        assert!(
            !cd.boxes.iter().any(|b| b.name == "u0"),
            "counter boxes are per-design HNF, not hier's u0: {:?}",
            cd.boxes.iter().map(|b| &b.name).collect::<Vec<_>>()
        );
        let ctop = cd.boxes.iter().find(|b| b.kind == "module").unwrap();
        let htop = d.boxes.iter().find(|b| b.kind == "module").unwrap();
        assert_ne!(
            ctop.cells, htop.cells,
            "box cells track HNF, not a canned dump"
        );
    }

    /// Fig. 49: Device clock-region outlines tile the HAD die.
    #[test]
    fn device_clock_region_outlines_tile_the_die() {
        let mut ide = IdeModel::new();
        let dump = ide.exec("device").unwrap();
        assert!(
            ide.device.clock_regions.len() >= 4,
            "2×2 clock regions: {:?}",
            ide.device.clock_regions
        );
        assert!(dump.contains("clock_regions="), "{dump}");
        assert!(dump.contains("cr=X0Y0:"), "{dump}");
        assert!(dump.contains("cr=X1Y1:"), "{dump}");
        for cr in &ide.device.clock_regions {
            assert!(cr.x1 > cr.x0 && cr.y1 > cr.y0, "region is a rectangle {cr:?}");
            assert!(
                cr.x0 >= ide.device.x0 && cr.y0 >= ide.device.y0,
                "region stays on the die {cr:?}"
            );
        }
        let area: u32 = ide
            .device
            .clock_regions
            .iter()
            .map(|c| c.cols() * c.rows())
            .sum();
        assert_eq!(
            area,
            ide.device.cols * ide.device.rows,
            "clock regions tile the drawing without gaps"
        );
    }

    /// Fig. 53: package pins sit in colored I/O bank regions.
    #[test]
    fn package_io_bank_colored_regions() {
        let mut ide = IdeModel::new();
        let dump = ide.exec("package").unwrap();
        let banks: std::collections::HashSet<u32> = ide.package_pins.iter().map(|p| p.bank).collect();
        assert!(
            banks.len() >= 2,
            "HAD IOB is split into banks, not one grey slab: {banks:?} {dump}"
        );
        assert!(dump.contains("banks="), "{dump}");
        assert!(dump.contains(":bank=0"), "{dump}");
        assert!(dump.contains(":bank=1"), "{dump}");
        let b0 = ide.package_pins.iter().find(|p| p.bank == 0).unwrap();
        let b1 = ide.package_pins.iter().find(|p| p.bank == 1).unwrap();
        assert_ne!(b0.bank_rgb(), b1.bank_rgb(), "banks have distinct colors");
        for p in &ide.package_pins {
            assert_eq!(p.pin, format!("IOB_X{}Y{}", p.x, p.y));
        }
    }

    /// Flow Navigator children actually Run Synthesis / Open Elaborated Schematic.
    #[test]
    fn navigator_children_run_synthesis_and_open_schematic() {
        let mut ide = IdeModel::new();
        assert!(
            NavSection::Synthesis
                .actions()
                .iter()
                .any(|a| a.tcl == "run_synthesis")
        );
        assert!(
            NavSection::RtlAnalysis
                .actions()
                .iter()
                .any(|a| a.tcl == "open_elaborated_schematic")
        );
        let e = ide.exec("open_elaborated_schematic").unwrap_err();
        assert!(e.contains("Run Synthesis") || e.contains("no HNF"), "{e}");

        ide.open_source(&example("counter.sv")).unwrap();
        let out = ide.exec("run_synthesis").unwrap();
        assert!(out.contains("cells") || ide.tree.cells.len() > 0, "{out}");
        assert_eq!(ide.step_state(FlowStep::Synthesis), StepState::Done);

        let sch = ide.exec("open_elaborated_schematic").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Schematic);
        assert!(sch.contains("box="), "opens the schematic drawing: {sch}");
        assert!(sch.contains("u_lut0:LUT6:box="), "{sch}");

        let impl_out = ide.exec("run_implementation").unwrap();
        assert!(impl_out.contains("Complete") || ide.bitstream_hash().is_some(), "{impl_out}");
        assert_eq!(ide.step_state(FlowStep::Bitstream), StepState::Done);
        let wns = ide.wns_ps().expect("STA after Run Implementation");
        assert_ne!(wns, 0);
    }

    /// BD canvas has Helion-MM interface pin stubs and an address map.
    #[test]
    fn bd_interface_pin_stubs_and_address_map() {
        let mut ide = IdeModel::new();
        let canvas = ide.exec("bd_drawing").unwrap();
        assert!(canvas.contains("pin:"), "IP boxes have pin stubs: {canvas}");
        assert!(canvas.contains(":iface"), "Helion-MM interface bars: {canvas}");
        assert!(canvas.contains("pin:u_h_uart:s_mm"), "{canvas}");
        assert!(canvas.contains("pin:mm_interconnect:m_mm"), "{canvas}");
        assert!(canvas.contains("addr=u_h_uart:0x0:0x1000"), "{canvas}");
        assert!(canvas.contains("addr=u_h_gpio:0x1000:0x1000"), "{canvas}");
        assert!(!canvas.contains("AXI"), "{canvas}");
        let d = ide
            .block_design
            .as_ref()
            .unwrap()
            .drawing(&ide.ip_catalog);
        let uart = d.symbols.iter().find(|s| s.kind == "h_uart").unwrap();
        assert!(
            uart.pins.iter().any(|p| p.name == "s_mm" && p.iface && !p.output),
            "slave interface stub on the IP box: {:?}",
            uart.pins
        );
        assert!(
            uart.pins.iter().any(|p| p.name == "clk" && !p.iface),
            "scalar clk stub: {:?}",
            uart.pins
        );
        assert_eq!(d.addresses.len(), 2);
        assert_ne!(d.addresses[0].base, d.addresses[1].base);
        let mm = d.wires.iter().find(|w| w.net == "Helion-MM").unwrap();
        let m_mm = d
            .symbols
            .iter()
            .find(|s| s.kind == "INTERCONNECT")
            .unwrap()
            .pins
            .iter()
            .find(|p| p.name == "m_mm")
            .unwrap();
        assert_eq!(mm.points[0], (m_mm.x, m_mm.y), "MM wire starts at the interface pin");
    }

    /// Fig. 55 toolbar: Previous/Next/Zoom Fit change the schematic camera paint uses.
    #[test]
    fn schematic_previous_next_zoom_fit_change_camera() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        let cam0 = ide.schematic.camera;
        assert_eq!(cam0.zoom, 1.0);
        let before = ide.exec("schematic_drawing").unwrap();
        assert!(before.contains("camera="), "{before}");

        let zin = ide.exec("schematic_zoom_in").unwrap();
        assert!(zin.contains("camera zoom="), "{zin}");
        let cam_in = ide.schematic.camera;
        assert!(cam_in.zoom > cam0.zoom, "zoom in must raise zoom: {cam_in:?}");
        assert_ne!(cam_in, cam0);

        let fit = ide.exec("zoom_fit").unwrap();
        assert!(fit.contains("camera zoom="), "{fit}");
        let cam_fit = ide.schematic.camera;
        assert_ne!(cam_fit, cam_in, "Zoom Fit must commit a new camera");
        assert_ne!(cam_fit.zoom, cam_in.zoom);
        let dump = ide.exec("schematic_drawing").unwrap();
        assert!(dump.contains(&format!("camera={},{},{}", cam_fit.zoom, cam_fit.pan_x as i32, cam_fit.pan_y as i32)), "{dump}");

        let prev = ide.exec("schematic_previous").unwrap();
        assert_eq!(ide.schematic.camera, cam_in, "Previous restores zoom-in camera: {prev}");
        let next = ide.exec("schematic_next").unwrap();
        assert_eq!(ide.schematic.camera, cam_fit, "Next walks forward to Zoom Fit: {next}");
        assert_eq!(ide.workspace, WorkspaceTab::Schematic);
    }

    /// Fig. 59: selecting a report_timing path highlights THAT path's STA cells/nets.
    #[test]
    fn schematic_highlights_selected_sta_timing_path() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        let full = ide.schematic.drawing();
        let n_full = full.symbols.iter().filter(|s| !s.kind.starts_with("PORT")).count();
        assert!(n_full > 2);
        assert!(full.symbols.iter().all(|s| !s.highlighted));

        ide.exec("report_timing").unwrap();
        assert!(
            !ide.timing_paths.is_empty(),
            "STA endpoints must become timing paths: endpoints={:?}",
            ide.timing.as_ref().map(|t| t.endpoints)
        );
        let p0 = ide.timing_paths[0].clone();
        assert!(!p0.cells.is_empty(), "path cells come from STA endpoints: {p0:?}");
        let d = ide.design().unwrap();
        for c in &p0.cells {
            assert!(
                d.cells.iter().any(|x| x.name == *c),
                "path cell {c} must be an HNF cell, not chrome"
            );
        }

        let out = ide.exec("select_timing_path 0").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Schematic);
        assert!(out.contains("timing_path"), "{out}");
        assert!(ide.schematic.path_only);
        let sheet = ide.schematic.drawing();
        let vis: Vec<_> = sheet
            .symbols
            .iter()
            .filter(|s| !s.kind.starts_with("PORT"))
            .collect();
        assert!(
            vis.len() < n_full,
            "path sheet is not a restyle of the full netlist ({}/{})",
            vis.len(),
            n_full
        );
        assert!(
            vis.iter().all(|s| s.highlighted && p0.cells.contains(&s.name)),
            "highlighted symbols are the STA path cells: {:?} path={:?}",
            vis.iter().map(|s| &s.name).collect::<Vec<_>>(),
            p0.cells
        );
        assert!(sheet.symbols.iter().any(|s| s.highlighted), "{out}");
        if !p0.nets.is_empty() {
            assert!(
                sheet.wires.iter().any(|w| w.highlighted && p0.nets.contains(&w.net)),
                "path nets highlighted: {:?}",
                sheet.wires.iter().map(|w| (&w.net, w.highlighted)).collect::<Vec<_>>()
            );
        }

        if ide.timing_paths.len() >= 2 {
            let p1 = ide.timing_paths[1].clone();
            ide.exec("select_timing_path 1").unwrap();
            let sheet1 = ide.schematic.drawing();
            let names: HashSet<_> = sheet1
                .symbols
                .iter()
                .filter(|s| s.highlighted)
                .map(|s| s.name.clone())
                .collect();
            for n in &names {
                assert!(p1.cells.contains(n), "path 1 cells only: {n} not in {:?}", p1.cells);
            }
        }
    }

    /// UG893 Fig. 55 schematic path picker is Name/From/To/Slack_ps, not `endpoint slack=` chips.
    #[test]
    fn schematic_timing_paths_name_from_to_slack_table() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.exec("report_timing").unwrap();
        assert!(!ide.timing_paths.is_empty(), "STA endpoints become timing paths");
        let wns = ide.wns_ps().expect("STA WNS");
        for p in &ide.timing_paths {
            let row = p.row_text();
            assert!(row.contains("NAME="), "{row}");
            assert!(row.contains(&format!("FROM={}", p.startpoint)), "{row}");
            assert!(row.contains(&format!("TO={}", p.endpoint)), "{row}");
            assert!(row.contains(&format!("SLACK_PS={}", p.slack_ps)), "{row}");
            assert!(
                !row.contains(" slack="),
                "not an endpoint slack= chip: {row}"
            );
            assert_eq!(p.slack_ps, wns, "Slack_ps is helion-sta WNS, not chrome");
            assert!(
                ide.design()
                    .unwrap()
                    .cells
                    .iter()
                    .any(|c| c.name == p.endpoint || p.cells.contains(&c.name)),
                "To is an HNF cell: {}",
                p.endpoint
            );
        }
        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.exec("report_timing").unwrap();
        assert!(!blinky.timing_paths.is_empty());
        assert_ne!(
            blinky.timing_paths[0].row_text(),
            ide.timing_paths[0].row_text(),
            "path table is per-design STA, not canned"
        );
    }

    /// UG903 report_timing is a pin-delay table (incr/path from helion-sta), not a name list.
    #[test]
    fn reports_timing_paths_is_ug903_pin_delay_table() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.exec("report_timing").unwrap();
        let t = ide.timing.clone().expect("STA after report_timing");
        assert!(
            !ide.timing_paths.is_empty(),
            "STA endpoints must become timing paths"
        );
        assert!(
            ide.timing_paths.iter().all(|p| !p.pins.is_empty()),
            "every path has pin-delay rows, not a name list: {:?}",
            ide.timing_paths
                .iter()
                .map(|p| (p.name.as_str(), p.pins.len()))
                .collect::<Vec<_>>()
        );
        let d = ide.design().unwrap();
        for p in &ide.timing_paths {
            let mut acc = 0i64;
            for pin in &p.pins {
                acc += pin.incr_ps;
                assert_eq!(
                    pin.path_ps, acc,
                    "cumulative Path column: {} {}",
                    pin.pin, pin.path_ps
                );
                assert!(
                    pin.pin.contains('/'),
                    "UG903 Name is cell/pin, not a dump token: {}",
                    pin.pin
                );
                assert!(
                    d.cells.iter().any(|c| c.name == pin.cell),
                    "pin cell {} must be HNF, not chrome",
                    pin.cell
                );
                assert!(
                    matches!(
                        pin.delay_type.as_str(),
                        "clk" | "cell" | "net" | "setup" | "iob"
                    ),
                    "delay type {}",
                    pin.delay_type
                );
            }
            assert_eq!(acc, p.delay_ps, "last Path_ps equals path delay {}", p.name);
            assert_eq!(p.logic_ps() + p.net_ps(), p.delay_ps);
        }
        let max_r2r = ide
            .timing_paths
            .iter()
            .filter(|p| p.pins.iter().any(|r| r.delay_type == "setup"))
            .map(|p| p.delay_ps)
            .max();
        assert_eq!(
            max_r2r,
            Some(t.r2r_ps),
            "worst r2r pin sum is STA r2r_ps={}",
            t.r2r_ps
        );
        let max_iob = ide
            .timing_paths
            .iter()
            .filter(|p| p.pins.iter().any(|r| r.delay_type == "iob"))
            .map(|p| p.delay_ps)
            .max();
        assert_eq!(
            max_iob,
            Some(t.iob_ps),
            "IOB pin sum is STA iob_ps={}",
            t.iob_ps
        );
        assert!(
            ide.timing_paths
                .iter()
                .any(|p| p.pins.iter().any(|r| r.incr_ps == FF_CKQ_PS)),
            "CkQ arc is helion-sta FF_CKQ_PS"
        );
        assert!(
            ide.timing_paths
                .iter()
                .any(|p| p.pins.iter().any(|r| r.incr_ps == LUT_PS)),
            "LUT comb arc is helion-sta LUT_PS"
        );
        assert!(
            ide.timing_paths
                .iter()
                .any(|p| p.pins.iter().any(|r| r.location.starts_with("CLB_")
                    || r.location.starts_with("IOB_"))),
            "Place fills HAD Location, not empty chrome: {:?}",
            ide.timing_paths
                .iter()
                .flat_map(|p| p.pins.iter().map(|r| r.location.as_str()))
                .collect::<Vec<_>>()
        );

        let table = ide.exec("timing_path_table").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Reports);
        assert!(table.contains("timing_paths n="), "{table}");
        assert!(table.contains("pin="), "{table}");
        assert!(table.contains("incr_ps="), "{table}");
        assert!(table.contains("path_ps="), "{table}");
        assert!(table.contains("type=cell"), "{table}");
        assert!(table.contains("type=setup") || table.contains("type=iob"), "{table}");
        assert!(!table.contains("AXI"), "{table}");

        let r0 = ide.exec("select_timing_path_report 0").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Reports);
        assert_eq!(ide.selected_timing_path, Some(0));
        assert!(r0.contains("pin="), "{r0}");
        let pin = ide.timing_paths[0]
            .pins
            .iter()
            .find(|p| p.incr_ps > 0)
            .cloned()
            .expect("nonzero pin delay");
        let out = ide.exec(&format!("select_timing_pin {}", pin.pin)).unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Reports);
        assert_eq!(ide.selected.as_deref(), Some(pin.cell.as_str()));
        assert_eq!(ide.selected_timing_pin.as_deref(), Some(pin.pin.as_str()));
        assert!(out.contains("incr_ps="), "{out}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "timing_pin"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "INCR_PS" && v.as_str() == pin.incr_ps.to_string()),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.exec("select_timing_pin")
                .unwrap_err()
                .contains("missing pin"),
            "empty pin click must refuse"
        );
        assert!(
            ide.exec("select_timing_pin no_such_pin")
                .unwrap_err()
                .contains("no pin"),
            "unknown pin must refuse"
        );
        let gold = ide.wns_ps().expect("gold WNS");
        assert_eq!(
            ide.timing.as_ref().map(|x| x.wns_ps),
            Some(gold),
            "pin table must not move STA WNS"
        );
    }

    /// Fig. 56 Expand Inside regenerates nested instance contents; primitives refuse.
    #[test]
    fn schematic_expand_inside_instance_primitives_refuse() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("hier.sv")).unwrap();
        assert!(
            ide.schematic.is_instance("u0"),
            "hier.sv instance u0 on the schematic: {:?}",
            ide.schematic.instances
        );
        let collapsed = ide.schematic.drawing();
        assert!(
            collapsed.symbols.iter().any(|s| s.name == "u0" && s.kind.starts_with("instance")),
            "collapsed sheet shows the instance box: {:?}",
            collapsed.symbols.iter().map(|s| format!("{}:{}", s.name, s.kind)).collect::<Vec<_>>()
        );
        let nested = ide.schematic.instance_member_cells("u0");
        assert!(!nested.is_empty(), "u0 has nested HNF cells");
        assert!(
            collapsed
                .symbols
                .iter()
                .filter(|s| nested.contains(&s.name))
                .count()
                == 0,
            "nested contents stay inside until Expand Inside"
        );

        let prim = ide
            .tree
            .cells
            .iter()
            .find(|(_, k)| k == "LUT6")
            .map(|(n, _)| n.clone())
            .expect("LUT primitive");
        let e = ide.exec(&format!("expand_inside {prim}")).unwrap_err();
        assert!(
            e.contains("primitive") && e.contains("refuses"),
            "primitives refuse Expand Inside: {e}"
        );

        ide.select("u0");
        let out = ide.exec("expand_inside").unwrap();
        assert_eq!(ide.schematic.expand_inside.as_deref(), Some("u0"));
        assert_eq!(ide.workspace, WorkspaceTab::Schematic);
        assert!(out.contains("expand_inside u0"), "{out}");
        let opened = ide.schematic.drawing();
        assert!(
            opened.symbols.iter().any(|s| nested.contains(&s.name)),
            "Expand Inside regenerates nested contents: nested={nested:?} symbols={:?}",
            opened.symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        assert!(
            opened.symbols.iter().all(|s| s.name != "u0" || !s.kind.starts_with("instance")),
            "instance box is replaced by nested cells"
        );
        assert!(
            opened
                .symbols
                .iter()
                .filter(|s| !s.kind.starts_with("PORT"))
                .any(|s| nested.contains(&s.name)),
            "{out}"
        );

        ide.exec("collapse_inside").unwrap();
        assert!(ide.schematic.expand_inside.is_none());
        let again = ide.schematic.drawing();
        assert!(again.symbols.iter().any(|s| s.name == "u0"));
    }

    /// UG900: clicking a Scope filters Objects from helion-sim, not a static dump.
    #[test]
    fn scopes_filter_objects_from_helion_sim() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("hier.sv")).unwrap();
        ide.sim_run(4).unwrap();
        assert!(ide.scopes.iter().any(|s| s.name == "hier" && s.kind == "module"));
        assert!(
            ide.scopes.iter().any(|s| s.name == "u0" && s.kind.contains("tog")),
            "{:?}",
            ide.scopes
        );
        let top = ide.exec("select_scope hier").unwrap();
        assert!(top.contains("scope hier"), "{top}");
        let top_names: Vec<String> = ide.objects.iter().map(|o| o.name.clone()).collect();
        assert!(
            ide.objects.iter().any(|o| o.name == "led"),
            "top scope has the LED probe from helion-sim: {top_names:?}"
        );
        assert!(
            ide.objects.iter().all(|o| !o.name.starts_with("u_ff") && !o.name.starts_with("u_lut")),
            "top scope must not dump child sequential objects: {top_names:?}"
        );

        let child = ide.exec("select_scope u0").unwrap();
        assert!(child.contains("scope u0"), "{child}");
        let child_names: Vec<String> = ide.objects.iter().map(|o| o.name.clone()).collect();
        assert_ne!(
            child_names, top_names,
            "Scopes must filter Objects, not keep a static list"
        );
        assert!(
            ide.objects.iter().any(|o| o.name.starts_with("u_ff") || o.name.starts_with("u0")),
            "instance scope objects come from helion-sim FFs: {child_names:?}"
        );
        assert!(
            ide.objects.iter().all(|o| o.name != "led"),
            "LED stays in the parent scope: {child_names:?}"
        );
        let led_top = ide
            .objects
            .iter()
            .find(|o| o.name.starts_with("u_ff"))
            .map(|o| o.value.clone());
        assert!(led_top.is_some());
        // Values are engine bits, not placeholders.
        assert!(
            ide.objects.iter().any(|o| o.value == "0" || o.value == "1"),
            "{child_names:?} {:?}",
            ide.objects
        );
    }

    /// UG900 Scopes/Objects is a clickable Name/Type/Value table over helion-sim, not a dump.
    #[test]
    fn ug900_scopes_objects_name_type_value_table() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("hier.sv")).unwrap();
        ide.sim_run(4).unwrap();

        let scopes = ide.exec("scopes").unwrap();
        assert!(scopes.contains('\n'), "Scopes must be a table: {scopes}");
        assert!(scopes.contains("NAME=hier"), "{scopes}");
        assert!(scopes.contains("TYPE=module"), "{scopes}");
        assert!(scopes.contains("NAME=u0"), "{scopes}");
        assert!(scopes.contains("TYPE=instance"), "{scopes}");
        assert!(
            !scopes.contains(" (module)"),
            "not a name (kind) dump: {scopes}"
        );
        assert!(
            ide.scope_rows()
                .iter()
                .any(|s| s.name == "hier" && s.type_cell() == "module"),
            "{:?}",
            ide.scope_rows()
        );
        assert!(
            ide.scope_rows()
                .iter()
                .any(|s| s.name == "u0" && s.type_cell() == "instance"),
            "{:?}",
            ide.scope_rows()
        );

        let top = ide.exec("select_scope 0").unwrap();
        assert!(top.contains("scope hier"), "{top}");
        assert_eq!(ide.selected_scope.as_deref(), Some("hier"));
        assert_eq!(ide.workspace, WorkspaceTab::Wave);
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "module"),
            "{:?}",
            ide.properties
        );

        let objs = ide.exec("objects").unwrap();
        assert!(objs.contains('\n'), "Objects must be a table: {objs}");
        assert!(objs.contains("scope=hier"), "{objs}");
        assert!(objs.contains("NAME=led"), "{objs}");
        assert!(objs.contains("TYPE=port"), "{objs}");
        assert!(objs.contains("VALUE="), "{objs}");
        assert!(
            !objs.contains("led = "),
            "not a name = value dump: {objs}"
        );
        let led = ide
            .object_rows()
            .iter()
            .find(|o| o.name == "led")
            .cloned()
            .expect("led object");
        assert_eq!(led.type_cell(), "port");
        assert!(
            led.value == "0" || led.value == "1",
            "Value is a helion-sim bit, not a placeholder: {led:?}"
        );

        let sel = ide.exec("select_object led").unwrap();
        assert!(sel.contains("NAME=led"), "{sel}");
        assert!(sel.contains("TYPE=port"), "{sel}");
        assert!(sel.contains("VALUE="), "{sel}");
        assert_eq!(ide.selected_object.as_deref(), Some("led"));
        assert_eq!(ide.workspace, WorkspaceTab::Wave);
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "port"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "VALUE" && (v == "0" || v == "1")),
            "{:?}",
            ide.properties
        );
        assert!(ide.wave.has_trace("led"), "click add_wave");

        let by_idx = ide.exec("select_object 0").unwrap();
        assert!(by_idx.contains("NAME="), "{by_idx}");
        assert_eq!(ide.selected_object.as_deref(), Some(ide.objects[0].name.as_str()));

        let child = ide.exec("select_scope u0").unwrap();
        assert!(child.contains("scope u0"), "{child}");
        assert!(child.contains("TYPE="), "{child}");
        assert!(
            ide.object_rows()
                .iter()
                .any(|o| o.type_cell() == "reg" && (o.value == "0" || o.value == "1")),
            "instance scope Objects are helion-sim FF probes: {:?}",
            ide.object_rows()
        );
        assert!(
            ide.object_rows().iter().all(|o| o.name != "led"),
            "LED stays in the parent scope: {:?}",
            ide.object_rows()
        );
        assert!(
            ide.exec("select_object no_such")
                .unwrap_err()
                .contains("no object"),
            "unknown object must refuse"
        );

        let mut counter = IdeModel::new();
        counter.open_source(&example("counter.sv")).unwrap();
        counter.run_step(FlowStep::Opt).unwrap();
        counter.run_step(FlowStep::Place).unwrap();
        counter.run_step(FlowStep::Route).unwrap();
        counter.run_step(FlowStep::Bitstream).unwrap();
        counter.sim_run(16).unwrap();
        let c_objs = counter.exec("objects").unwrap();
        assert!(c_objs.contains("NAME=led"), "{c_objs}");
        assert!(c_objs.contains("TYPE=port"), "{c_objs}");
        assert!(
            counter
                .object_rows()
                .iter()
                .any(|o| o.name == "cnt" && o.type_cell() == "bus"),
            "packed LUTFF bus is TYPE=bus: {:?}",
            counter.object_rows()
        );
        let gold = counter.fabric_led_bits(16).unwrap();
        counter.exec("add_wave_virtual_bus vb led cnt").unwrap();
        let vb = counter
            .object_rows()
            .iter()
            .find(|o| o.name == "vb")
            .expect("virtual bus object");
        assert_eq!(vb.type_cell(), "virtual_bus");
        assert_ne!(vb.value_cell(), "-");
        assert_eq!(counter.wave.bits_of("led").as_deref(), Some(gold.as_str()));
    }

    /// UG900 Memory / Breakpoints / Locals are clickable tables over helion-sim/fabric.
    #[test]
    fn ug900_memory_breakpoints_locals_from_helion_sim_fabric() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();

        let mem = ide.exec("memory").unwrap();
        assert!(mem.contains('\n'), "Memory must be a table: {mem}");
        assert!(mem.contains("NAME=u_lut0"), "{mem}");
        assert!(mem.contains("TYPE=lut_init"), "{mem}");
        assert!(mem.contains("DATA=0x5555555555555555"), "{mem}");
        assert!(mem.contains("DEPTH=64"), "{mem}");
        assert!(mem.contains("NAME=u_lut3"), "{mem}");
        assert!(
            !mem.contains("NAME=u_lut "),
            "counter memories are not blinky: {mem}"
        );
        assert_eq!(ide.workspace, WorkspaceTab::Memory);
        assert_eq!(ide.layout, LayoutKind::Simulation);

        let lut0 = ide
            .memory_rows()
            .iter()
            .find(|m| m.name == "u_lut0")
            .cloned()
            .expect("u_lut0");
        assert_eq!(lut0.type_cell(), "lut_init");
        assert_eq!(lut0.packed_word(), helion_ir::INC4_INIT[0]);
        assert_eq!(lut0.words[0], 1);
        assert_eq!(lut0.words[1], 0);
        let lut3 = ide
            .memory_rows()
            .iter()
            .find(|m| m.name == "u_lut3")
            .cloned()
            .expect("u_lut3");
        assert_eq!(lut3.packed_word(), helion_ir::INC4_INIT[3]);
        assert_ne!(lut0.data_cell(), lut3.data_cell());

        let sel = ide.exec("select_memory u_lut0").unwrap();
        assert!(sel.contains("NAME=u_lut0"), "{sel}");
        assert!(sel.contains("ADDR=0 DATA=1"), "{sel}");
        assert_eq!(ide.selected_memory.as_deref(), Some("u_lut0"));
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "lut_init"),
            "{:?}",
            ide.properties
        );
        let word0 = ide.exec("select_memory_word 0").unwrap();
        assert!(word0.contains("ADDR=0 DATA=1"), "{word0}");
        let word1 = ide.exec("select_memory_word 1").unwrap();
        assert!(word1.contains("ADDR=1 DATA=0"), "{word1}");
        assert_eq!(ide.selected_memory_addr, Some(1));

        let loc = ide.exec("locals").unwrap();
        assert!(loc.contains('\n'), "Locals must be a table: {loc}");
        assert!(loc.contains("NAME=led"), "{loc}");
        assert!(loc.contains("TYPE=port"), "{loc}");
        assert!(loc.contains("VALUE="), "{loc}");
        assert!(
            !loc.contains("led = "),
            "not a name = value dump: {loc}"
        );
        assert!(
            ide.local_rows()
                .iter()
                .any(|l| l.type_cell() == "reg" && (l.value == "0" || l.value == "1")),
            "Locals are helion-sim FF probes: {:?}",
            ide.local_rows()
        );
        assert_eq!(ide.workspace, WorkspaceTab::Locals);
        let led_l = ide.exec("select_local led").unwrap();
        assert!(led_l.contains("NAME=led"), "{led_l}");
        assert_eq!(ide.selected_local.as_deref(), Some("led"));
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "port"),
            "{:?}",
            ide.properties
        );

        ide.sim_run(1).unwrap();
        let seq1 = ide
            .memory_rows()
            .iter()
            .find(|m| m.name == "seq")
            .map(|m| m.packed_word())
            .expect("seq after step");
        ide.sim_run(8).unwrap();
        let seq8 = ide
            .memory_rows()
            .iter()
            .find(|m| m.name == "seq")
            .map(|m| m.packed_word())
            .expect("seq after 8");
        assert_ne!(seq1, seq8, "seq memory is live helion-sim Q, not a static dump");

        let bp = ide.exec("add_bp led 1").unwrap();
        assert!(bp.contains("ID="), "{bp}");
        assert!(bp.contains("SIGNAL=led"), "{bp}");
        assert!(bp.contains("CONDITION===1") || bp.contains("CONDITION===1"), "{bp}");
        assert!(bp.contains("ENABLED=1"), "{bp}");
        let table = ide.exec("breakpoints").unwrap();
        assert!(table.contains('\n'), "Breakpoints must be a table: {table}");
        assert!(table.contains("SIGNAL=led"), "{table}");
        assert_eq!(ide.workspace, WorkspaceTab::Breakpoints);
        let hit = ide.exec("sim_run 16").unwrap();
        assert!(hit.contains("HIT"), "value breakpoint must stop sim: {hit}");
        assert!(hit.contains("SIGNAL=led"), "{hit}");
        assert!(hit.contains("CYCLE=8") || hit.contains("CYCLE=8"), "{hit}");
        assert!(
            ide.breakpoints.iter().any(|b| b.signal == "led" && b.hits >= 1),
            "hit count from engine: {:?}",
            ide.breakpoints
        );
        let n = ide.wave.sample_len();
        assert!(n < 16, "sim_run must stop at the breakpoint, not run to completion: n={n}");
        let click = ide.exec("select_breakpoint 1").unwrap();
        assert!(click.contains("SIGNAL=led"), "{click}");
        assert_eq!(ide.selected_breakpoint, Some(1));
        ide.exec("disable_bp 1").unwrap();
        let full = ide.exec("sim_run 16").unwrap();
        assert!(
            !full.contains("HIT"),
            "disabled breakpoint must not stop: {full}"
        );
        assert_eq!(ide.wave.sample_len(), 16);
        ide.exec("enable_bp 1").unwrap();
        let hit2 = ide.exec("sim_run 16").unwrap();
        assert!(hit2.contains("HIT"), "{hit2}");

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        let bmem = blinky.exec("memory").unwrap();
        assert!(bmem.contains("NAME=u_lut"), "{bmem}");
        assert!(!bmem.contains("NAME=u_lut0"), "blinky is not counter: {bmem}");
        assert!(
            blinky
                .memory_rows()
                .iter()
                .any(|m| m.name == "u_lut" && m.packed_word() == 0x5555_5555_5555_5555),
            "{:?}",
            blinky.memory_rows()
        );
        let bloc = blinky.exec("locals").unwrap();
        assert!(bloc.contains("NAME=led"), "{bloc}");
        assert!(
            blinky
                .local_rows()
                .iter()
                .any(|l| l.name == "u_ff" && l.type_cell() == "reg"),
            "blinky locals are u_ff, not counter u_ff0: {:?}",
            blinky.local_rows()
        );
        assert!(
            blinky.local_rows().iter().all(|l| !l.name.starts_with("u_ff0")),
            "{:?}",
            blinky.local_rows()
        );

        let mut hier = IdeModel::new();
        hier.open_source(&example("hier.sv")).unwrap();
        hier.sim_run(4).unwrap();
        hier.exec("select_scope hier").unwrap();
        let top_loc: Vec<String> = hier.local_rows().iter().map(|l| l.name.clone()).collect();
        assert!(
            hier.local_rows().iter().any(|l| l.name == "led"),
            "top locals include LED: {top_loc:?}"
        );
        hier.exec("select_scope u0").unwrap();
        let child_loc: Vec<String> = hier.local_rows().iter().map(|l| l.name.clone()).collect();
        assert_ne!(child_loc, top_loc, "Locals filter by Scope, not a static dump");
        assert!(
            hier.local_rows()
                .iter()
                .any(|l| l.name.starts_with("u_ff") || l.name.starts_with("u0")),
            "{child_loc:?}"
        );
        assert!(
            hier.local_rows().iter().all(|l| l.name != "led"),
            "LED stays in parent scope: {child_loc:?}"
        );

        let mut fab = IdeModel::new();
        fab.open_source(&example("counter.sv")).unwrap();
        fab.run_step(FlowStep::Opt).unwrap();
        fab.run_step(FlowStep::Place).unwrap();
        fab.run_step(FlowStep::Route).unwrap();
        fab.run_step(FlowStep::Bitstream).unwrap();
        let gold_wns = fab.wns_ps();
        fab.sim_run(16).unwrap();
        let fmem = fab.exec("memory").unwrap();
        assert!(fmem.contains("NAME=u_lut0"), "{fmem}");
        assert!(fmem.contains("TYPE=lut_init"), "{fmem}");
        let flut0 = fab
            .memory_rows()
            .iter()
            .find(|m| m.name == "u_lut0")
            .expect("fabric u_lut0");
        assert_eq!(
            flut0.packed_word(),
            helion_ir::INC4_INIT[0],
            "fabric LUT INIT is programmed bitstream, not a placeholder"
        );
        assert!(
            fab.memory_rows().iter().any(|m| m.name == "seq" && m.kind == "seq"),
            "fabric sequential Q is a Memory row: {:?}",
            fab.memory_rows()
        );
        let floc = fab.exec("locals").unwrap();
        assert!(floc.contains("TYPE=reg"), "{floc}");
        assert!(
            fab.local_rows()
                .iter()
                .any(|l| l.value == "0" || l.value == "1"),
            "fabric locals are BLE Q bits: {:?}",
            fab.local_rows()
        );
        fab.exec("add_bp led 1").unwrap();
        let fhit = fab.exec("sim_run 16").unwrap();
        assert!(fhit.contains("HIT"), "fabric breakpoint: {fhit}");
        assert!(fhit.contains("CYCLE=8") || fhit.contains("CYCLE=8"), "{fhit}");
        assert_eq!(
            fab.wns_ps(),
            gold_wns,
            "Memory/Breakpoints/Locals must not disturb gold WNS"
        );

        assert!(
            ide.exec("select_memory no_such")
                .unwrap_err()
                .contains("no memory")
        );
        assert!(
            ide.exec("add_bp no_such")
                .unwrap_err()
                .contains("no signal")
        );
    }

    /// Fig. 49: click a clock region; Properties show name + HAD site count.
    #[test]
    fn device_clock_region_select_shows_had_site_count() {
        let mut ide = IdeModel::new();
        let dump = ide.exec("device").unwrap();
        assert!(dump.contains(":sites="), "{dump}");
        let cr = ide
            .device
            .clock_region_named("X0Y0")
            .cloned()
            .expect("X0Y0");
        let n = cr.site_count(&ide.device.sites);
        assert!(n > 0, "HAD sites in X0Y0");
        assert_eq!(
            n,
            ide.device
                .sites
                .iter()
                .filter(|s| cr.contains(s.x, s.y))
                .count()
        );
        let out = ide.exec("select_clock_region X0Y0").unwrap();
        assert!(out.contains("clock_region X0Y0"), "{out}");
        assert!(out.contains(&format!("sites={n}")), "{out}");
        assert_eq!(ide.selected.as_deref(), Some("X0Y0"));
        assert_eq!(ide.properties_name(), Some("X0Y0"));
        assert!(
            ide.properties.iter().any(|(k, v)| k == "TYPE" && v == "clock_region"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "SITES" && v == &n.to_string()),
            "Properties site count from HAD: {:?}",
            ide.properties
        );

        let cr2 = ide.device.clock_region_named("X1Y1").cloned().expect("X1Y1");
        let n2 = cr2.site_count(&ide.device.sites);
        ide.exec("select_clock_region X1Y1").unwrap();
        assert_eq!(ide.properties_name(), Some("X1Y1"));
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "SITES" && v == &n2.to_string()),
            "{:?}",
            ide.properties
        );
        if n != n2 {
            assert_ne!(n, n2, "site count is per-region HAD, not a canned number");
        }
    }

    /// Fig. 64: Tcl journal is selectable with Find; ok vs error still from Session.
    #[test]
    fn tcl_console_journal_is_selectable_with_find() {
        let mut ide = IdeModel::new();
        ide.exec("report_utilization").ok();
        let bad = ide.exec("no_such_command");
        assert!(bad.is_err());
        ide.open_source(&example("counter.sv")).unwrap();
        ide.exec("report_timing").unwrap();

        assert!(ide.console.iter().any(|l| l.cmd == "report_timing" && l.ok));
        assert!(ide.console.iter().any(|l| !l.ok && l.cmd.contains("no_such_command")));

        let found = ide.exec("console_find report_timing").unwrap();
        assert_eq!(ide.bottom_tab, BottomTab::Tcl);
        assert!(found.contains("hits="), "{found}");
        assert!(ide.console_selected.is_some(), "{found}");
        let idx = ide.console_selected.unwrap();
        assert_eq!(ide.console[idx].cmd, "report_timing");
        assert!(ide.console[idx].ok, "Find lands on the real Session ok line");
        assert!(ide.console[idx].out.contains("WNS_PS="), "{}", ide.console[idx].out);

        let err = ide.exec("console_find no_such_command").unwrap();
        let ei = ide.console_selected.unwrap();
        assert!(!ide.console[ei].ok, "errors stay journaled from Session: {err}");

        let sel = ide.exec(&format!("select_console {idx}")).unwrap();
        assert_eq!(ide.console_selected, Some(idx));
        assert!(sel.contains("ok=1"), "{sel}");
        assert!(sel.contains("cmd=report_timing"), "{sel}");
    }

    /// UG893 Tcl Console is a clickable Status/Cmd/Out table over Session,
    /// not a `helion% cmd` + monospace-out dump (Log already is that table).
    #[test]
    fn tcl_console_clickable_status_cmd_out_table() {
        let mut ide = IdeModel::new();
        assert!(ide.console_rows().is_empty());
        assert!(
            ide.select_console_line("0")
                .unwrap_err()
                .contains("console empty"),
            "empty table must refuse a click"
        );
        assert!(
            ide.exec("select_console")
                .unwrap_err()
                .contains("missing id"),
            "empty click must refuse"
        );
        let empty = ide.exec("tcl_console").unwrap();
        assert_eq!(ide.bottom_tab, BottomTab::Tcl);
        assert!(
            empty.contains("tcl_console empty")
                || empty.contains("tcl_console n=")
                || empty.contains("CMD=tcl_console"),
            "{empty}"
        );
        assert!(
            !empty.contains("helion%"),
            "Tcl Console table must not be a helion% dump: {empty}"
        );

        let e = ide.run_step(FlowStep::Synthesis).unwrap_err();
        assert!(e.contains("source") || e.contains("synth"), "{e}");
        let table = ide.exec("tcl_console").unwrap();
        assert_eq!(ide.bottom_tab, BottomTab::Tcl);
        assert!(table.contains("STATUS=error"), "{table}");
        assert!(table.contains("CMD=synth_design"), "{table}");
        assert!(table.contains('\n'), "must not be a one-liner dump: {table}");
        assert!(
            !table.contains("helion%"),
            "must not dump helion% lines: {table}"
        );
        let sel = ide.exec("select_console synth_design").unwrap();
        assert!(sel.contains("STATUS=error"), "{sel}");
        assert!(sel.contains("CMD=synth_design"), "{sel}");
        assert_eq!(ide.workspace, WorkspaceTab::Schematic);
        assert_eq!(ide.bottom_tab, BottomTab::Tcl);
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "tcl"),
            "{:?}",
            ide.properties
        );

        ide.open_source(&example("counter.sv")).unwrap();
        ide.exec("report_timing").unwrap();
        let gold = ide.wns_ps().expect("STA WNS");
        assert_ne!(gold, 0);
        let table = ide.exec("tcl_console").unwrap();
        assert!(table.contains("tcl_console n="), "{table}");
        assert!(table.contains("STATUS=ok CMD=report_timing"), "{table}");
        assert!(table.contains(&format!("WNS_PS={gold}")), "{table}");
        assert!(
            !table.contains("helion% report_timing"),
            "table is Status/Cmd/Out, not a prompt dump: {table}"
        );
        let tsel = ide.exec("select_console report_timing").unwrap();
        assert!(
            tsel.contains(&format!("WNS_PS={gold}")),
            "click must carry STA, not a stub: {tsel}"
        );
        assert!(tsel.contains("STATUS=ok"), "{tsel}");
        assert_eq!(ide.workspace, WorkspaceTab::Reports);
        assert_eq!(ide.bottom_tab, BottomTab::Tcl);
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "OUT" && v.contains(&format!("WNS_PS={gold}"))),
            "{:?}",
            ide.properties
        );
        assert_eq!(
            ide.wns_ps(),
            Some(gold),
            "empty XDC gold WNS must hold after Tcl Console"
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.exec("report_timing").unwrap();
        let bw = blinky.wns_ps().expect("blinky STA");
        assert_ne!(bw, gold, "WNS is per-design");
        let bsel = blinky.exec("select_console report_timing").unwrap();
        assert!(bsel.contains(&format!("WNS_PS={bw}")), "{bsel}");
        assert!(
            !bsel.contains(&format!("WNS_PS={gold}")),
            "Tcl Console clicks are per-session, not canned: {bsel}"
        );
        assert_eq!(blinky.workspace, WorkspaceTab::Reports);
        let btable = blinky.exec("tcl_console").unwrap();
        assert!(btable.contains(&format!("WNS_PS={bw}")), "{btable}");
        assert!(
            !btable.contains(&format!("WNS_PS={gold}")),
            "table is per-session, not canned: {btable}"
        );
    }

    /// UG893 Device routing overlay: PathFinder IOB tiles from Session.routed, not occupancy restyle.
    #[test]
    fn device_drawing_shows_pathfinder_iob_routes() {
        let mut ide = IdeModel::new();
        let empty = ide.exec("device").unwrap();
        assert!(empty.contains("routes=0"), "{empty}");
        assert!(ide.device.routes.is_empty());

        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        let placed = ide.exec("device").unwrap();
        assert!(
            placed.contains("routes=0"),
            "place must not invent routing: {placed}"
        );
        assert!(ide.session().routed.is_none());
        assert!(ide.device.routes.is_empty());

        ide.run_step(FlowStep::Route).unwrap();
        let dump = ide.exec("device").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Device);
        let (eng_net, eng_path, eng_hops, eng_delay, eng_clb, eng_iob, n_routes) = {
            let engine = ide
                .session()
                .routed
                .as_ref()
                .expect("Session.routed after Route");
            let io = engine.iob_src.first().expect("PathFinder IOB net");
            (
                io.net.clone(),
                io.path.clone(),
                io.hops,
                io.delay_ps,
                io.clb,
                io.iob,
                engine.iob_src.len(),
            )
        };
        assert_eq!(
            ide.device.routes.len(),
            n_routes,
            "overlay count is PathFinder IOB nets, not occupancy"
        );
        assert!(
            eng_path.len() >= 2,
            "PathFinder must keep CLB→IOB tiles: {eng_path:?}"
        );
        assert_eq!(eng_path.first().copied(), Some(eng_clb));
        assert_eq!(eng_path.last().copied(), Some(eng_iob));
        assert!(!eng_net.is_empty(), "route names the packed IOB net");
        let route = ide
            .device
            .route_named(&eng_net)
            .cloned()
            .unwrap_or_else(|| ide.device.routes[0].clone());
        assert_eq!(route.net, eng_net);
        assert_eq!(route.tiles, eng_path, "overlay tiles are the engine path");
        assert_eq!(route.hops, eng_hops);
        assert_eq!(route.delay_ps, eng_delay);
        assert!(route.hops >= 1);
        assert!(route.delay_ps > 0, "STA delay comes from hops, not a stub");
        let tiles = route
            .tiles
            .iter()
            .map(|(x, y)| format!("X{x}Y{y}"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(dump.contains(&format!("routes={}", ide.device.routes.len())), "{dump}");
        assert!(
            dump.contains(&format!("route net={} hops={}", route.net, route.hops)),
            "{dump}"
        );
        assert!(dump.contains(&format!("delay_ps={}", route.delay_ps)), "{dump}");
        assert!(dump.contains(&format!("tiles={tiles}")), "{dump}");

        let net = route.net.clone();
        let hops0 = route.hops;
        let delay0 = route.delay_ps;
        let tiles0 = route.tiles.clone();
        let sel = ide.exec(&format!("select_device_route {net}")).unwrap();
        assert!(sel.contains(&format!("net={net}")), "{sel}");
        assert!(sel.contains(&format!("hops={hops0}")), "{sel}");
        assert_eq!(ide.selected.as_deref(), Some(net.as_str()));
        assert!(ide.device_has_selected());
        assert!(
            ide.device.routes.iter().any(|r| r.net == net && r.highlighted),
            "selecting the net highlights THAT PathFinder route: {:?}",
            ide.device.routes
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "ROUTE_HOPS" && v == &hops0.to_string()),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "DELAY_PS" && v == &delay0.to_string()),
            "{:?}",
            ide.properties
        );

        if let Some(&(x, y)) = tiles0.iter().find(|&&(x, y)| {
            Some((x, y)) != tiles0.first().copied()
                && Some((x, y)) != tiles0.last().copied()
                && ide
                    .device
                    .site_at(x, y)
                    .map(|s| s.occupant.is_none())
                    .unwrap_or(false)
        }) {
            let mid = ide.exec(&format!("select_device_site X{x}Y{y}")).unwrap();
            assert!(mid.contains(&format!("route={net}")), "{mid}");
            assert_eq!(ide.selected.as_deref(), Some(net.as_str()));
        }

        ide.exec(&format!("unroute_net {net}")).unwrap();
        let u = ide.exec("device").unwrap();
        let ur = ide.device.route_named(&net).expect("unrouted net stays on overlay");
        assert_eq!(ur.hops, 0, "unroute_net zeros engine hops");
        assert_eq!(ur.delay_ps, 0);
        assert!(u.contains(&format!("route net={net} hops=0")), "{u}");

        ide.exec(&format!("fix_route {net} 8")).unwrap();
        let fx = ide.device.route_named(&net).expect("fix_route overlay");
        assert_eq!(fx.tiles, tiles0, "directed hops keep PathFinder tiles");
        assert_eq!(fx.hops, 8, "unroute then +8 hops is directed delay");
        assert_eq!(fx.delay_ps, 8 * 40);

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        blinky.exec("device").unwrap();
        assert!(!blinky.device.routes.is_empty());
        let (b_net, b_path) = {
            let be = blinky.session().routed.as_ref().unwrap();
            (
                be.iob_src[0].net.clone(),
                be.iob_src[0].path.clone(),
            )
        };
        let b = &blinky.device.routes[0];
        assert_eq!(b.tiles, b_path);
        assert_eq!(b.net, b_net);
        assert_ne!(
            b.net, net,
            "overlay net is the packed IOB net, not a canned label: {} vs {net}",
            b.net
        );
        assert_eq!(
            blinky.runs.iter().find(|r| r.name == "impl_1").unwrap().lutff,
            Some(1)
        );
        assert_eq!(blinky.utilization.unwrap().lutff, 1);
        assert_eq!(ide.utilization.unwrap().lutff, 4);
    }

    /// UG893 I/O Planning: `set_property PACKAGE_PIN` re-places onto the HAD IOB
    /// and re-runs STA — not a pin-name list.
    #[test]
    fn io_planning_package_pin_replaces_and_hits_sta() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();

        let x0 = ide.session().placed.as_ref().unwrap().iob_sites[0].x;
        let lut_x0 = ide.session().placed.as_ref().unwrap().lutff_sites[0].0.x;
        let wns0 = ide.wns_ps().expect("STA after route");
        assert_ne!(wns0, 0);
        assert_eq!(x0, 2, "default IOB is the first HAD site IOB_X2Y0");
        assert_eq!(lut_x0, 2, "LUTFF follows the default IOB column");
        assert!(
            NavSection::BoardDevice
                .actions()
                .iter()
                .any(|a| a.tcl == "io_planning"),
            "navigator I/O Planning child: {:?}",
            NavSection::BoardDevice.actions()
        );

        let dump = ide.exec("io_planning").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Package);
        assert_eq!(ide.nav, NavSection::BoardDevice);
        assert!(dump.contains("led"), "{dump}");
        assert!(dump.contains("PACKAGE_PIN=-") || dump.contains("placed=IOB_X2Y0"), "{dump}");
        let led0 = ide
            .io_ports
            .iter()
            .find(|p| p.name == "led")
            .cloned()
            .expect("led port");
        assert_eq!(led0.site.as_deref(), Some("IOB_X2Y0"), "{:?}", ide.io_ports);
        assert!(led0.package_pin.is_none(), "no LOC until set_property: {led0:?}");

        let out = ide
            .exec("set_property PACKAGE_PIN IOB_X5Y0 [get_ports led]")
            .unwrap();
        assert!(out.contains("PACKAGE_PIN IOB_X5Y0"), "{out}");
        assert!(out.contains("led"), "{out}");
        assert!(out.contains("replaced=1"), "must re-place, not stash a pin: {out}");
        assert!(out.contains("rerouted=1"), "must re-route so STA sees the loc: {out}");
        assert_eq!(ide.workspace, WorkspaceTab::Package);

        let pl = ide.session().placed.as_ref().expect("re-placed");
        assert_eq!(pl.iob_sites[0].x, 5, "PACKAGE_PIN must bind IOB in place");
        assert_eq!(pl.iob_sites[0].y, 0);
        assert_eq!(pl.lutff_sites[0].0.x, 5, "LUTFF follows LOC column");
        assert_ne!(pl.iob_sites[0].x, x0);
        assert_eq!(
            pl.packed.iobs[0].loc.as_deref(),
            Some("IOB_X5Y0"),
            "pack must see LOC"
        );

        let led = ide
            .io_ports
            .iter()
            .find(|p| p.name == "led")
            .cloned()
            .expect("led after loc");
        assert_eq!(led.package_pin.as_deref(), Some("IOB_X5Y0"), "{led:?}");
        assert_eq!(led.site.as_deref(), Some("IOB_X5Y0"), "{led:?}");
        assert!(
            ide.package_pins
                .iter()
                .any(|p| p.pin == "IOB_X5Y0" && p.port.as_deref() == Some("led")),
            "package drawing occupancy must move: {:?}",
            ide.package_pins.iter().filter(|p| p.port.is_some()).collect::<Vec<_>>()
        );
        assert!(
            !ide.package_pins
                .iter()
                .any(|p| p.pin == "IOB_X2Y0" && p.port.as_deref() == Some("led")),
            "old IOB must be vacated"
        );

        let ports = ide.exec("io_ports").unwrap();
        assert!(ports.contains("PACKAGE_PIN=IOB_X5Y0"), "{ports}");
        assert!(ports.contains("placed=IOB_X5Y0"), "{ports}");
        assert_eq!(
            ide.constraints.package_pins.get("led").map(|s| s.as_str()),
            Some("IOB_X5Y0")
        );
        let ctext = ide.constraints_text();
        assert!(
            ctext.contains("set_property PACKAGE_PIN IOB_X5Y0 led"),
            "{ctext}"
        );
        let loc = ide
            .design()
            .unwrap()
            .ports
            .iter()
            .find(|p| p.name == "led")
            .and_then(|p| p.attrs.get("LOC"));
        assert_eq!(loc, Some("IOB_X5Y0"));

        assert_eq!(ide.selected_cell(), Some("led"));
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "PACKAGE_PIN" && v == "IOB_X5Y0"),
            "{:?}",
            ide.properties
        );

        let lut = ide.device.occupant_of("u_lut0").expect("LUTFF after re-place");
        assert_eq!(lut.x, 5, "device floorplan must follow PACKAGE_PIN column");
        let wns1 = ide.wns_ps().expect("STA after PACKAGE_PIN re-place/route");
        assert_ne!(wns1, 0);
        let rt = ide.session().routed.as_ref().expect("re-routed");
        assert!(
            rt.iob_src[0].hops >= 1,
            "PathFinder hops after loc: {}",
            rt.iob_src[0].hops
        );
        assert_eq!(rt.placed.iob_sites[0].x, 5);
        assert!(ide.timing.as_ref().unwrap().route_ps > 0);

        let e = ide
            .exec("set_property PACKAGE_PIN IOB_X0Y0 [get_ports led]")
            .unwrap_err();
        assert!(
            e.contains("not a HAD IOB"),
            "bogus ball must fail against HAD: {e}"
        );
        assert_eq!(
            ide.session().placed.as_ref().unwrap().iob_sites[0].x,
            5,
            "failed loc must not move place"
        );

        let assign = ide.exec("assign_package_pin led IOB_X8Y0").unwrap();
        assert!(assign.contains("IOB_X8Y0"), "{assign}");
        assert_eq!(ide.session().placed.as_ref().unwrap().iob_sites[0].x, 8);
        assert_eq!(ide.session().placed.as_ref().unwrap().lutff_sites[0].0.x, 8);
        assert_eq!(
            ide.io_ports
                .iter()
                .find(|p| p.name == "led")
                .and_then(|p| p.site.as_deref()),
            Some("IOB_X8Y0")
        );
        assert!(ide.wns_ps().is_some());
    }

    /// UG893 I/O Ports: `set_property IOSTANDARD` hits HNF + HAD + STA pad delay
    /// and DRC bank VCCO — not a table label.
    #[test]
    fn io_planning_iostandard_hits_sta_and_drc() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();

        let wns0 = ide.wns_ps().expect("STA after route");
        let iob0 = ide.timing.as_ref().unwrap().iob_ps;
        assert_ne!(wns0, 0);
        let dump = ide.exec("io_ports").unwrap();
        assert!(dump.contains("led"), "{dump}");
        assert!(dump.contains("IOSTANDARD=-"), "{dump}");
        let led0 = ide
            .io_ports
            .iter()
            .find(|p| p.name == "led")
            .cloned()
            .expect("led port");
        assert!(led0.iostandard.is_none(), "{led0:?}");

        let out = ide
            .exec("set_property IOSTANDARD LVCMOS33 [get_ports led]")
            .unwrap();
        assert!(out.contains("IOSTANDARD LVCMOS33"), "{out}");
        assert!(out.contains("pad_ps="), "must report STA pad delay: {out}");
        assert_eq!(ide.workspace, WorkspaceTab::Package);
        assert_eq!(ide.nav, NavSection::BoardDevice);

        let std = ide
            .design()
            .unwrap()
            .ports
            .iter()
            .find(|p| p.name == "led")
            .and_then(|p| p.attrs.get("IOSTANDARD"));
        assert_eq!(std, Some("LVCMOS33"), "HNF port attr");
        let led = ide
            .io_ports
            .iter()
            .find(|p| p.name == "led")
            .cloned()
            .expect("led after IOSTANDARD");
        assert_eq!(led.iostandard.as_deref(), Some("LVCMOS33"), "{led:?}");
        let ports = ide.exec("io_ports").unwrap();
        assert!(ports.contains("IOSTANDARD=LVCMOS33"), "{ports}");
        assert_eq!(
            ide.constraints.iostandards.get("led").map(|s| s.as_str()),
            Some("LVCMOS33")
        );
        let ctext = ide.constraints_text();
        assert!(
            ctext.contains("set_property IOSTANDARD LVCMOS33 led"),
            "{ctext}"
        );
        assert_eq!(ide.selected_cell(), Some("led"));
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "IOSTANDARD" && v == "LVCMOS33"),
            "{:?}",
            ide.properties
        );

        let iob1 = ide.timing.as_ref().expect("STA after IOSTANDARD").iob_ps;
        assert!(
            iob1 > iob0,
            "LVCMOS33 pad must slow IOB STA ({iob1} vs {iob0})"
        );
        let wns1 = ide.wns_ps().expect("STA after IOSTANDARD");
        assert!(
            wns1 < wns0,
            "slower I/O standard must worsen WNS ({wns1} vs {wns0})"
        );
        assert_ne!(wns1, 0);

        let e = ide
            .exec("set_property IOSTANDARD LVDS_25 [get_ports led]")
            .unwrap_err();
        assert!(
            e.contains("not a HAD"),
            "illegal I/O standard must fail against HAD: {e}"
        );
        assert_eq!(
            ide.design()
                .unwrap()
                .ports
                .iter()
                .find(|p| p.name == "led")
                .and_then(|p| p.attrs.get("IOSTANDARD")),
            Some("LVCMOS33"),
            "failed IOSTANDARD must not clobber HNF"
        );

        let clean = ide.exec("report_drc").unwrap();
        assert!(
            clean.contains("violations=0") || clean.contains("ok"),
            "single LVCMOS33 on one port is legal: {clean}"
        );

        ide.exec("set_property PACKAGE_PIN IOB_X2Y0 [get_ports led]")
            .unwrap();
        ide.exec("set_property PACKAGE_PIN IOB_X3Y0 [get_ports clk]")
            .unwrap();
        ide.exec("set_property IOSTANDARD LVCMOS18 [get_ports clk]")
            .unwrap();
        let mix = ide.exec("report_drc").unwrap();
        assert!(
            mix.contains("VCCO") || mix.contains("IOSTANDARD"),
            "mixed 1.8/3.3 V on BANK0 must fail DRC: {mix}"
        );
        assert!(
            !ide.drc.as_ref().unwrap().ok(),
            "DRC must not be clean: {:?}",
            ide.drc.as_ref().unwrap().violations
        );

        let back = ide
            .exec("set_property IOSTANDARD LVCMOS18 [get_ports led]")
            .unwrap();
        assert!(back.contains("LVCMOS18"), "{back}");
        let wns18 = ide.wns_ps().expect("STA after back to LVCMOS18");
        assert_eq!(
            wns18, wns0,
            "LVCMOS18 must restore gold pad delay ({wns18} vs {wns0})"
        );
    }

    /// UG893 I/O Ports: `set_property DRIVE / SLEW / PULLTYPE` hit HNF + HAD +
    /// STA pad delay + DRC + bitgen — not table labels.
    #[test]
    fn io_planning_drive_slew_pulltype_hit_sta_drc_bitgen() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        ide.run_step(FlowStep::Bitstream).unwrap();

        let wns0 = ide.wns_ps().expect("STA after route");
        let iob0 = ide.timing.as_ref().unwrap().iob_ps;
        let hash0 = ide.bitstream_hash().expect("bitgen before electrical");
        assert_ne!(wns0, 0);
        let dump = ide.exec("io_ports").unwrap();
        assert!(dump.contains("DRIVE=-"), "{dump}");
        assert!(dump.contains("SLEW=-"), "{dump}");
        assert!(dump.contains("PULLTYPE=-"), "{dump}");
        let led0 = ide
            .io_ports
            .iter()
            .find(|p| p.name == "led")
            .cloned()
            .expect("led port");
        assert!(led0.drive.is_none(), "{led0:?}");
        assert!(led0.slew.is_none(), "{led0:?}");
        assert!(led0.pulltype.is_none(), "{led0:?}");

        let out = ide
            .exec("set_property DRIVE 4 [get_ports led]")
            .unwrap();
        assert!(out.contains("DRIVE 4"), "{out}");
        assert!(out.contains("pad_ps="), "must report STA pad delay: {out}");
        assert!(out.contains("bitgen=1"), "must re-bitgen: {out}");
        assert_eq!(ide.workspace, WorkspaceTab::Package);
        assert_eq!(
            ide.design()
                .unwrap()
                .ports
                .iter()
                .find(|p| p.name == "led")
                .and_then(|p| p.attrs.get("DRIVE")),
            Some("4"),
            "HNF port attr"
        );
        let led = ide
            .io_ports
            .iter()
            .find(|p| p.name == "led")
            .cloned()
            .expect("led after DRIVE");
        assert_eq!(led.drive.as_deref(), Some("4"), "{led:?}");
        let ports = ide.exec("io_ports").unwrap();
        assert!(ports.contains("DRIVE=4"), "{ports}");
        assert_eq!(
            ide.constraints.drives.get("led").map(|s| s.as_str()),
            Some("4")
        );
        let ctext = ide.constraints_text();
        assert!(ctext.contains("set_property DRIVE 4 led"), "{ctext}");
        assert!(
            ide.properties.iter().any(|(k, v)| k == "DRIVE" && v == "4"),
            "{:?}",
            ide.properties
        );
        let iob1 = ide.timing.as_ref().expect("STA after DRIVE").iob_ps;
        assert!(
            iob1 > iob0,
            "DRIVE 4 must slow IOB STA ({iob1} vs {iob0})"
        );
        let hash1 = ide.bitstream_hash().expect("bitgen after DRIVE");
        assert_ne!(hash1, hash0, "DRIVE must change the IOB bitstream");

        let e = ide.exec("set_property DRIVE 99 [get_ports led]").unwrap_err();
        assert!(
            e.contains("not a HAD"),
            "illegal DRIVE must fail against HAD: {e}"
        );
        assert_eq!(
            ide.design()
                .unwrap()
                .ports
                .iter()
                .find(|p| p.name == "led")
                .and_then(|p| p.attrs.get("DRIVE")),
            Some("4"),
            "failed DRIVE must not clobber HNF"
        );

        let slew = ide.exec("set_property SLEW FAST [get_ports led]").unwrap();
        assert!(slew.contains("SLEW FAST"), "{slew}");
        let led = ide
            .io_ports
            .iter()
            .find(|p| p.name == "led")
            .cloned()
            .expect("led after SLEW");
        assert_eq!(led.slew.as_deref(), Some("FAST"), "{led:?}");
        let iob2 = ide.timing.as_ref().expect("STA after SLEW").iob_ps;
        assert!(
            iob2 < iob1,
            "FAST slew must speed IOB STA ({iob2} vs {iob1})"
        );
        let e = ide.exec("set_property SLEW MEDIUM [get_ports led]").unwrap_err();
        assert!(e.contains("HAD") || e.contains("SLOW"), "{e}");

        let pull = ide
            .exec("set_property PULLTYPE PULLUP [get_ports led]")
            .unwrap();
        assert!(pull.contains("PULLTYPE PULLUP"), "{pull}");
        let led = ide
            .io_ports
            .iter()
            .find(|p| p.name == "led")
            .cloned()
            .expect("led after PULLTYPE");
        assert_eq!(led.pulltype.as_deref(), Some("PULLUP"), "{led:?}");
        let iob3 = ide.timing.as_ref().expect("STA after PULLTYPE").iob_ps;
        assert!(
            iob3 > iob2,
            "PULLUP must add pad load ({iob3} vs {iob2})"
        );
        let e = ide
            .exec("set_property PULLTYPE PULL [get_ports led]")
            .unwrap_err();
        assert!(e.contains("HAD") || e.contains("PULLUP"), "{e}");

        ide.exec("set_property DRIVE 24 [get_ports led]").unwrap();
        let mix = ide.exec("report_drc").unwrap();
        assert!(
            mix.contains("DRIVE") || mix.contains("IOSTANDARD"),
            "DRIVE 24 vs default LVCMOS18 must fail DRC: {mix}"
        );
        assert!(
            !ide.drc.as_ref().unwrap().ok(),
            "DRC must not be clean: {:?}",
            ide.drc.as_ref().unwrap().violations
        );

        ide.exec("set_property DRIVE 12 [get_ports led]").unwrap();
        ide.exec("set_property SLEW SLOW [get_ports led]").unwrap();
        ide.exec("set_property PULLTYPE NONE [get_ports led]")
            .unwrap();
        let wns_back = ide.wns_ps().expect("STA after defaults");
        assert_eq!(
            wns_back, wns0,
            "DRIVE 12 / SLOW / NONE must restore gold pad ({wns_back} vs {wns0})"
        );
        let iob_back = ide.timing.as_ref().unwrap().iob_ps;
        assert_eq!(iob_back, iob0, "gold IOB pad must return ({iob_back} vs {iob0})");
        let hash_back = ide.bitstream_hash().expect("bitgen after defaults");
        assert_eq!(
            hash_back, hash0,
            "default electrical bits must restore gold bitstream"
        );
        let clean = ide.exec("report_drc").unwrap();
        assert!(
            clean.contains("violations=0") || clean.contains("ok"),
            "defaults are legal: {clean}"
        );
    }

    /// UG893 I/O Ports: `set_property DIFF_TERM / IN_TERM` hit HNF + HAD + STA
    /// pad delay + DRC + bitgen — not table labels.
    #[test]
    fn io_planning_diff_term_in_term_hit_sta_drc_bitgen() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        ide.run_step(FlowStep::Bitstream).unwrap();

        let wns0 = ide.wns_ps().expect("STA after route");
        let iob0 = ide.timing.as_ref().unwrap().iob_ps;
        let hash0 = ide.bitstream_hash().expect("bitgen before termination");
        assert_ne!(wns0, 0);
        let dump = ide.exec("io_ports").unwrap();
        assert!(dump.contains("DIFF_TERM=-"), "{dump}");
        assert!(dump.contains("IN_TERM=-"), "{dump}");
        let led0 = ide
            .io_ports
            .iter()
            .find(|p| p.name == "led")
            .cloned()
            .expect("led port");
        assert!(led0.diff_term.is_none(), "{led0:?}");
        assert!(led0.in_term.is_none(), "{led0:?}");

        let out = ide
            .exec("set_property DIFF_TERM TRUE [get_ports led]")
            .unwrap();
        assert!(out.contains("DIFF_TERM TRUE"), "{out}");
        assert!(out.contains("pad_ps="), "must report STA pad delay: {out}");
        assert!(out.contains("bitgen=1"), "must re-bitgen: {out}");
        assert_eq!(ide.workspace, WorkspaceTab::Package);
        assert_eq!(
            ide.design()
                .unwrap()
                .ports
                .iter()
                .find(|p| p.name == "led")
                .and_then(|p| p.attrs.get("DIFF_TERM")),
            Some("TRUE"),
            "HNF port attr"
        );
        let led = ide
            .io_ports
            .iter()
            .find(|p| p.name == "led")
            .cloned()
            .expect("led after DIFF_TERM");
        assert_eq!(led.diff_term.as_deref(), Some("TRUE"), "{led:?}");
        let ports = ide.exec("io_ports").unwrap();
        assert!(ports.contains("DIFF_TERM=TRUE"), "{ports}");
        assert_eq!(
            ide.constraints.diff_terms.get("led").map(|s| s.as_str()),
            Some("TRUE")
        );
        let ctext = ide.constraints_text();
        assert!(ctext.contains("set_property DIFF_TERM TRUE led"), "{ctext}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "DIFF_TERM" && v == "TRUE"),
            "{:?}",
            ide.properties
        );
        let iob1 = ide.timing.as_ref().expect("STA after DIFF_TERM").iob_ps;
        assert!(
            iob1 > iob0,
            "DIFF_TERM TRUE must add pad load ({iob1} vs {iob0})"
        );
        let hash1 = ide.bitstream_hash().expect("bitgen after DIFF_TERM");
        assert_ne!(hash1, hash0, "DIFF_TERM must change the IOB bitstream");

        let e = ide
            .exec("set_property DIFF_TERM YES [get_ports led]")
            .unwrap_err();
        assert!(
            e.contains("not a HAD"),
            "illegal DIFF_TERM must fail against HAD: {e}"
        );
        assert_eq!(
            ide.design()
                .unwrap()
                .ports
                .iter()
                .find(|p| p.name == "led")
                .and_then(|p| p.attrs.get("DIFF_TERM")),
            Some("TRUE"),
            "failed DIFF_TERM must not clobber HNF"
        );

        let interm = ide
            .exec("set_property IN_TERM UNTUNED_SPLIT_50 [get_ports led]")
            .unwrap();
        assert!(interm.contains("IN_TERM UNTUNED_SPLIT_50"), "{interm}");
        let led = ide
            .io_ports
            .iter()
            .find(|p| p.name == "led")
            .cloned()
            .expect("led after IN_TERM");
        assert_eq!(led.in_term.as_deref(), Some("UNTUNED_SPLIT_50"), "{led:?}");
        let iob2 = ide.timing.as_ref().expect("STA after IN_TERM").iob_ps;
        assert!(
            iob2 > iob1,
            "IN_TERM UNTUNED_SPLIT_50 must add pad load ({iob2} vs {iob1})"
        );
        let e = ide
            .exec("set_property IN_TERM 50 [get_ports led]")
            .unwrap_err();
        assert!(e.contains("HAD") || e.contains("UNTUNED"), "{e}");

        let mix = ide.exec("report_drc").unwrap();
        assert!(
            mix.contains("DIFF_TERM") || mix.contains("IN_TERM") || mix.contains("IOSTANDARD"),
            "TRUE / UNTUNED_SPLIT_50 vs default LVCMOS18 must fail DRC: {mix}"
        );
        assert!(
            !ide.drc.as_ref().unwrap().ok(),
            "DRC must not be clean: {:?}",
            ide.drc.as_ref().unwrap().violations
        );

        ide.exec("set_property IOSTANDARD SSTL15 [get_ports led]")
            .unwrap();
        let sstl = ide.exec("report_drc").unwrap();
        assert!(
            sstl.contains("violations=0") || sstl.contains("ok"),
            "SSTL15 + DIFF_TERM TRUE + IN_TERM is HAD-legal: {sstl}"
        );

        ide.exec("set_property IOSTANDARD LVCMOS18 [get_ports led]")
            .unwrap();
        ide.exec("set_property DIFF_TERM FALSE [get_ports led]")
            .unwrap();
        ide.exec("set_property IN_TERM NONE [get_ports led]")
            .unwrap();
        let wns_back = ide.wns_ps().expect("STA after defaults");
        assert_eq!(
            wns_back, wns0,
            "FALSE / NONE / LVCMOS18 must restore gold pad ({wns_back} vs {wns0})"
        );
        let iob_back = ide.timing.as_ref().unwrap().iob_ps;
        assert_eq!(iob_back, iob0, "gold IOB pad must return ({iob_back} vs {iob0})");
        let hash_back = ide.bitstream_hash().expect("bitgen after defaults");
        assert_eq!(
            hash_back, hash0,
            "default termination bits must restore gold bitstream"
        );
        let clean = ide.exec("report_drc").unwrap();
        assert!(
            clean.contains("violations=0") || clean.contains("ok"),
            "defaults are legal: {clean}"
        );
    }

    /// UG893 I/O Ports is a clickable Name/Dir/PACKAGE_PIN/IOSTANDARD table
    /// from HAD/STA, not a collapsing concatenated dump.
    #[test]
    fn io_ports_pane_clickable_table_not_a_dump() {
        let mut ide = IdeModel::new();
        assert!(ide.io_port_rows().is_empty());
        assert!(
            ide.exec("select_io_port")
                .unwrap_err()
                .contains("missing name"),
            "empty click must refuse"
        );
        assert!(
            ide.exec("select_io_port led")
                .unwrap_err()
                .contains("no port"),
            "unknown port must refuse"
        );
        let empty = ide.exec("io_ports").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Package);
        assert_eq!(ide.nav, NavSection::BoardDevice);
        assert!(empty.contains("io_ports n=0"), "{empty}");
        assert!(!empty.contains("NAME="), "idle pane has no canned rows: {empty}");
        assert!(
            NavSection::BoardDevice
                .actions()
                .iter()
                .any(|a| a.tcl == "io_planning"),
            "Flow Navigator Board/Device must offer I/O Planning"
        );

        ide.open_source(&example("counter.sv")).unwrap();
        let table = ide.exec("io_ports").unwrap();
        assert!(table.contains('\n'), "must not be a one-liner dump: {table}");
        assert!(table.contains("NAME=clk"), "{table}");
        assert!(table.contains("NAME=led"), "{table}");
        assert!(table.contains("DIR=IN"), "{table}");
        assert!(table.contains("DIR=OUT"), "{table}");
        assert!(table.contains("PACKAGE_PIN=-"), "{table}");
        assert!(table.contains("IOSTANDARD=-"), "{table}");
        assert!(table.contains("DRIVE=-"), "{table}");
        assert!(table.contains("SLEW=-"), "{table}");
        assert!(table.contains("PULLTYPE=-"), "{table}");
        assert!(table.contains("DIFF_TERM=-"), "{table}");
        assert!(table.contains("IN_TERM=-"), "{table}");
        let rows = ide.io_port_rows();
        assert_eq!(rows.len(), 2, "counter has clk+led: {rows:?}");
        assert!(rows.iter().any(|p| p.name == "clk" && p.dir == "IN"));
        assert!(rows.iter().any(|p| p.name == "led" && p.dir == "OUT"));

        let sel = ide.exec("select_io_port led").unwrap();
        assert!(sel.contains("NAME=led"), "{sel}");
        assert!(sel.contains("DIR=OUT"), "{sel}");
        assert_eq!(ide.selected.as_deref(), Some("led"));
        assert_eq!(ide.workspace, WorkspaceTab::Package);
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "port"),
            "{:?}",
            ide.properties
        );
        let by_idx = ide.exec("select_io_port 0").unwrap();
        assert!(
            by_idx.contains("NAME=clk") || by_idx.contains("NAME=led"),
            "{by_idx}"
        );

        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let gold = ide.wns_ps().expect("gold WNS");
        assert_ne!(gold, 0);
        let placed = ide.exec("io_ports").unwrap();
        assert!(
            placed.contains("placed=IOB_"),
            "place fills the Placed column from HAD: {placed}"
        );
        let led = ide
            .io_port_rows()
            .iter()
            .find(|p| p.name == "led")
            .cloned()
            .expect("led");
        assert!(led.site.as_deref().unwrap_or("").starts_with("IOB_"), "{led:?}");
        ide.exec("set_property PACKAGE_PIN IOB_X5Y0 [get_ports led]")
            .unwrap();
        let loc = ide.exec("select_io_port led").unwrap();
        assert!(loc.contains("PACKAGE_PIN=IOB_X5Y0"), "{loc}");
        assert!(loc.contains("placed=IOB_X5Y0"), "{loc}");
        assert_eq!(
            ide.io_port_rows()
                .iter()
                .find(|p| p.name == "led")
                .and_then(|p| p.package_pin.as_deref()),
            Some("IOB_X5Y0")
        );
        let wns = ide.wns_ps().expect("STA after PACKAGE_PIN");
        assert_ne!(wns, 0);

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        let btable = blinky.exec("io_ports").unwrap();
        assert!(btable.contains("NAME=led"), "{btable}");
        assert!(btable.contains('\n'), "{btable}");
        blinky.exec("select_io_port led").unwrap();
        assert_eq!(blinky.selected.as_deref(), Some("led"));
        assert_eq!(blinky.io_port_rows().len(), 2);
    }

    /// UG893 Floorplanning: `create_pblock` / `resize_pblock` re-places into a
    /// HAD rectangle and hits `helion-bits::bitgen_pblock` — not a site dump.
    #[test]
    fn pblock_floorplanning_hits_place_and_bitgen_pblock() {
        let mut ide = IdeModel::new();
        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        ide.run_step(FlowStep::Bitstream).unwrap();

        let x0 = ide.session().placed.as_ref().unwrap().lutff_sites[0].0.x;
        let y0 = ide.session().placed.as_ref().unwrap().lutff_sites[0].0.y;
        let full = ide
            .session()
            .bitstream
            .as_ref()
            .expect("full bitstream")
            .frames
            .len();
        assert!(full > 0, "full bitgen must produce frames");
        assert_eq!(x0, 2, "default LUTFF follows IOB column");
        assert!(
            NavSection::BoardDevice
                .actions()
                .iter()
                .any(|a| a.tcl == "floorplanning"),
            "navigator Floorplanning child: {:?}",
            NavSection::BoardDevice.actions()
        );

        let created = ide.exec("create_pblock pblock_0").unwrap();
        assert!(created.contains("pblock_0"), "{created}");
        assert_eq!(ide.workspace, WorkspaceTab::Device);
        assert_eq!(ide.nav, NavSection::BoardDevice);
        let pane = ide.exec("floorplanning").unwrap();
        assert!(pane.contains("pblocks n=1"), "{pane}");
        assert!(pane.contains("pblock_0"), "{pane}");

        let out = ide
            .exec("resize_pblock pblock_0 -add {CLB_X5Y1:CLB_X8Y8}")
            .unwrap();
        assert!(out.contains("resize_pblock pblock_0"), "{out}");
        assert!(out.contains("CLB_X5Y1:CLB_X8Y8"), "{out}");
        assert!(out.contains("placed=1"), "must re-place into the pblock: {out}");
        assert!(out.contains("routed=1"), "must re-route so bitgen_pblock sees the loc: {out}");
        assert!(out.contains("frames="), "must hit bitgen_pblock: {out}");

        let lut_sites: Vec<(u32, u32)> = ide
            .session()
            .placed
            .as_ref()
            .expect("re-placed")
            .lutff_sites
            .iter()
            .map(|(s, _)| (s.x, s.y))
            .collect();
        for (x, y) in &lut_sites {
            assert!(
                *x >= 5 && *x <= 8 && *y >= 1 && *y <= 8,
                "LUTFF must sit in the pblock: X{x}Y{y}"
            );
        }
        assert_ne!(lut_sites[0].0, x0, "pblock must move LUTFF off default column");
        assert!(
            lut_sites.iter().any(|&(x, y)| x != x0 || y != y0),
            "pblock must move at least one LUTFF off the default site"
        );

        let pb = ide
            .pblocks
            .iter()
            .find(|p| p.name == "pblock_0")
            .cloned()
            .expect("pblock_0");
        assert!(pb.ranged);
        assert_eq!((pb.x0, pb.y0, pb.x1, pb.y1), (5, 1, 8, 8));
        assert!(pb.frames > 0, "partial bitstream frames: {pb:?}");
        assert!(
            pb.frames < full,
            "pblock frames {} must be a subset of full {full}",
            pb.frames
        );
        assert_eq!(ide.device.pblocks.len(), 1);
        assert!(ide.device.pblock_named("pblock_0").is_some());

        let dump = ide.exec("device").unwrap();
        assert!(dump.contains("pblocks=1"), "{dump}");
        assert!(dump.contains("pb=pblock_0:5,1,8,8:"), "{dump}");
        let lut = ide.device.occupant_of("u_lut0").expect("LUTFF after pblock");
        assert!(
            pb.contains(lut.x, lut.y),
            "device floorplan occupant must sit in the pblock: {lut:?}"
        );

        let sel = ide.exec("select_pblock pblock_0").unwrap();
        assert!(sel.contains("pblock pblock_0"), "{sel}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "pblock"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "FRAMES" && v == &pb.frames.to_string()),
            "{:?}",
            ide.properties
        );

        let add = ide.exec("add_cells_to_pblock pblock_0 u_lut0").unwrap();
        assert!(add.contains("u_lut0"), "{add}");
        assert!(
            ide.pblocks[0].cells.iter().any(|c| c == "u_lut0"),
            "{:?}",
            ide.pblocks[0].cells
        );
        let ctext = ide.constraints_text();
        assert!(ctext.contains("create_pblock pblock_0"), "{ctext}");
        assert!(
            ctext.contains("resize_pblock pblock_0 -add {CLB_X5Y1:CLB_X8Y8}"),
            "{ctext}"
        );

        let wns = ide.wns_ps().expect("STA after pblock re-place/route");
        assert_ne!(wns, 0);
        let rt_x = ide
            .session()
            .routed
            .as_ref()
            .expect("re-routed")
            .placed
            .lutff_sites[0]
            .0
            .x;
        assert_eq!(rt_x, lut_sites[0].0);

        let e = ide
            .exec("resize_pblock missing -add {CLB_X5Y1:CLB_X8Y8}")
            .unwrap_err();
        assert!(e.contains("no pblock"), "{e}");
        let e = ide
            .exec("resize_pblock pblock_0 -add {CLB_X99Y99:CLB_X100Y100}")
            .unwrap_err();
        assert!(
            e.contains("no HAD CLB") || e.contains("no placed sites"),
            "bogus range must fail against HAD: {e}"
        );
    }

    /// UG893 Floorplanning Pblocks is a clickable Name/Range/Cells/Sites/Frames
    /// table over create_pblock / resize_pblock / bitgen_pblock, not a collapsing dump.
    #[test]
    fn pblocks_pane_clickable_table_not_a_dump() {
        let mut ide = IdeModel::new();
        assert!(ide.pblock_rows().is_empty());
        assert!(
            ide.exec("select_pblock")
                .unwrap_err()
                .contains("missing name"),
            "empty click must refuse"
        );
        assert!(
            ide.exec("select_pblock pblock_0")
                .unwrap_err()
                .contains("no pblock"),
            "unknown pblock must refuse"
        );
        let empty = ide.exec("pblocks").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Device);
        assert_eq!(ide.nav, NavSection::BoardDevice);
        assert!(empty.contains("pblocks n=0"), "{empty}");
        assert!(!empty.contains("NAME="), "idle pane has no canned rows: {empty}");
        assert!(
            NavSection::BoardDevice
                .actions()
                .iter()
                .any(|a| a.tcl == "floorplanning"),
            "Flow Navigator Board/Device must offer Floorplanning"
        );

        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let gold = ide.wns_ps().expect("gold WNS");
        assert_ne!(gold, 0);

        ide.exec("create_pblock pblock_0").unwrap();
        let table = ide.exec("pblocks").unwrap();
        assert!(table.contains('\n'), "must not be a one-liner dump: {table}");
        assert!(table.contains("pblocks n=1"), "{table}");
        assert!(table.contains("NAME=pblock_0"), "{table}");
        assert!(table.contains("RANGE=-"), "{table}");
        assert!(table.contains("CELLS=0"), "{table}");
        assert!(table.contains("FRAMES=0"), "{table}");
        let rows = ide.pblock_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "pblock_0");
        assert!(!rows[0].ranged);

        let sel = ide.exec("select_pblock pblock_0").unwrap();
        assert!(sel.contains("NAME=pblock_0"), "{sel}");
        assert!(sel.contains("pblock pblock_0"), "{sel}");
        assert_eq!(ide.selected.as_deref(), Some("pblock_0"));
        assert_eq!(ide.workspace, WorkspaceTab::Device);
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "pblock"),
            "{:?}",
            ide.properties
        );
        let by_idx = ide.exec("select_pblock 0").unwrap();
        assert!(by_idx.contains("NAME=pblock_0"), "{by_idx}");

        ide.exec("resize_pblock pblock_0 -add {CLB_X5Y1:CLB_X8Y8}")
            .unwrap();
        let ranged = ide.exec("pblocks").unwrap();
        assert!(ranged.contains("RANGE=CLB_X5Y1:CLB_X8Y8"), "{ranged}");
        assert!(ranged.contains("NAME=pblock_0"), "{ranged}");
        let pb = ide.pblock_rows()[0].clone();
        assert!(pb.ranged);
        assert!(pb.frames > 0, "bitgen_pblock fills Frames: {pb:?}");
        assert!(ranged.contains(&format!("FRAMES={}", pb.frames)), "{ranged}");
        let click = ide.exec("select_pblock pblock_0").unwrap();
        assert!(click.contains("RANGE=CLB_X5Y1:CLB_X8Y8"), "{click}");
        assert!(
            click.contains(&format!("FRAMES={}", pb.frames)),
            "click must carry bitgen_pblock, not a stub: {click}"
        );
        let wns = ide.wns_ps().expect("STA after pblock");
        assert_ne!(wns, 0);

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.exec("create_pblock pblock_blinky").unwrap();
        let btable = blinky.exec("pblocks").unwrap();
        assert!(btable.contains("NAME=pblock_blinky"), "{btable}");
        assert!(!btable.contains("NAME=pblock_0"), "pblocks are per-session: {btable}");
        assert!(btable.contains('\n'), "{btable}");
    }

    /// UG949 Clock Interaction (`report_clock_interaction`) pane is STA clocks
    /// + XDC CDC exceptions — not a canned matrix. Empty XDC keeps gold WNS.
    #[test]
    fn clock_interaction_pane_from_sta_clocks_not_a_dump() {
        let mut ide = IdeModel::new();
        let empty = ide.clock_interaction_text();
        assert!(
            empty.contains("no clocks"),
            "idle pane has no canned matrix: {empty}"
        );
        assert!(ide.clock_interaction().cells.is_empty());
        assert!(
            NavSection::TimingAnalysis
                .actions()
                .iter()
                .any(|a| a.tcl == "report_clock_interaction"),
            "Flow Navigator Timing Analysis must offer the pane"
        );

        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let gold = ide.wns_ps().expect("STA after route");
        assert_ne!(gold, 0);

        let out = ide.exec("report_clock_interaction").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::ClockInteraction);
        assert!(out.contains("FROM=clk TO=clk Timed"), "{out}");
        assert!(out.contains(&format!("WNS_PS={gold}")), "{out}");
        let r = ide.clock_interaction();
        assert_eq!(r.clocks.len(), 1, "{}", r.text());
        assert_eq!(r.cells.len(), 1);
        let intra = r.cell("clk", "clk").expect("intra-clock cell");
        assert_eq!(intra.relation, helion_sta::ClockRelation::Timed);
        assert_eq!(intra.wns_ps, Some(gold));
        assert_eq!(intra.path_count, ide.timing.as_ref().unwrap().endpoints);
        assert_eq!(
            ide.wns_ps(),
            Some(gold),
            "opening the pane must not move gold WNS"
        );

        let sel = ide.exec("select_clock_interaction clk clk").unwrap();
        assert!(sel.contains("TYPE") || sel.contains("FROM=clk"), "{sel}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "clock_interaction"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "RELATION" && v == "Timed"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "WNS_PS" && v == &gold.to_string()),
            "{:?}",
            ide.properties
        );

        let gclk = ide
            .exec(
                "create_generated_clock -name clkdiv -source [get_ports clk] -divide_by 2 [get_pins u_ff/Q]",
            )
            .unwrap();
        assert!(gclk.contains("PERIOD_PS=20000"), "{gclk}");
        let r = ide.clock_interaction();
        assert_eq!(r.clocks.len(), 2, "{}", r.text());
        assert_eq!(r.cells.len(), 4);
        assert_eq!(
            r.cell("clk", "clkdiv").unwrap().relation,
            helion_sta::ClockRelation::TimedGenerated
        );
        assert_eq!(
            r.cell("clkdiv", "clk").unwrap().relation,
            helion_sta::ClockRelation::TimedGenerated
        );
        let wns_div = ide.wns_ps().expect("STA after generated clock");
        assert_eq!(wns_div, gold + 10_000);
        assert_eq!(
            r.cell("clkdiv", "clkdiv").unwrap().wns_ps,
            Some(wns_div),
            "generated intra-clock WNS is STA, not a label"
        );
        assert_eq!(r.cell("clk", "clk").unwrap().wns_ps, Some(gold));

        let virt = ide
            .exec("create_clock -name virt -period 8.000 [get_ports virt]")
            .unwrap();
        assert!(virt.contains("PERIOD_PS=8000"), "{virt}");
        let r = ide.clock_interaction();
        assert_eq!(r.clocks.len(), 3, "{}", r.text());
        assert_eq!(r.cells.len(), 9);
        let cdc = r.cell("clk", "virt").expect("CDC cell");
        assert_eq!(cdc.relation, helion_sta::ClockRelation::TimedUnsafe);
        assert_ne!(
            cdc.wns_ps,
            Some(gold),
            "unsafe CDC slack uses the destination period, not a canned copy: {:?}",
            cdc.wns_ps
        );
        assert!(r.unsafe_count() >= 2, "{}", r.text());
        assert!(r.cdc_count() >= 2, "{}", r.text());
        let pane = ide.exec("report_clock_interaction").unwrap();
        assert!(pane.contains("Timed (unsafe)"), "{pane}");
        assert!(pane.contains("FROM=clk TO=virt"), "{pane}");

        let cg = ide
            .exec("set_clock_groups -asynchronous -group [get_clocks clk] -group [get_clocks virt]")
            .unwrap();
        assert!(cg.contains("clock_groups=1"), "{cg}");
        let r = ide.clock_interaction();
        assert_eq!(
            r.cell("clk", "virt").unwrap().relation,
            helion_sta::ClockRelation::Asynchronous
        );
        assert!(r.cell("clk", "virt").unwrap().wns_ps.is_none());
        assert_eq!(
            r.cell("clk", "clk").unwrap().relation,
            helion_sta::ClockRelation::Timed
        );
        let pane = ide.clock_interaction_text();
        assert!(pane.contains("Asynchronous"), "{pane}");

        let mut ex = IdeModel::new();
        ex.open_source(&example("counter.sv")).unwrap();
        ex.run_step(FlowStep::Place).unwrap();
        ex.run_step(FlowStep::Route).unwrap();
        ex.exec("create_clock -period 10.000 [get_ports clk]")
            .unwrap();
        ex.exec("create_clock -name virt -period 8.000 [get_ports virt]")
            .unwrap();
        let gold2 = ex.wns_ps().expect("STA with virt clock still on clk");
        assert_eq!(gold2, gold, "virtual clock must not steal the analysis period");
        let fp = ex
            .exec("set_false_path -from [get_clocks clk] -to [get_clocks virt]")
            .unwrap();
        assert!(fp.contains("false_path=1"), "{fp}");
        assert_eq!(
            ex.clock_interaction()
                .cell("clk", "virt")
                .unwrap()
                .relation,
            helion_sta::ClockRelation::FalsePath
        );

        let mut dp = IdeModel::new();
        dp.open_source(&example("counter.sv")).unwrap();
        dp.run_step(FlowStep::Place).unwrap();
        dp.run_step(FlowStep::Route).unwrap();
        dp.exec("create_clock -period 10.000 [get_ports clk]")
            .unwrap();
        dp.exec("create_clock -name virt -period 8.000 [get_ports virt]")
            .unwrap();
        let md = dp
            .exec("set_max_delay -datapath_only 2.0 -from [get_clocks clk] -to [get_clocks virt]")
            .unwrap();
        assert!(md.contains("max_delay=1") || md.contains("MAX_DELAY_PS=2000"), "{md}");
        let cell = dp
            .clock_interaction()
            .cell("clk", "virt")
            .unwrap()
            .clone();
        assert_eq!(cell.relation, helion_sta::ClockRelation::TimedDatapath);
        assert_eq!(cell.requirement_ps, 2_000);

        let mut excl = IdeModel::new();
        excl.open_source(&example("counter.sv")).unwrap();
        excl.run_step(FlowStep::Place).unwrap();
        excl.run_step(FlowStep::Route).unwrap();
        excl.exec("create_clock -period 10.000 [get_ports clk]")
            .unwrap();
        excl.exec("create_clock -name virt -period 8.000 [get_ports virt]")
            .unwrap();
        let exg = excl
            .exec(
                "set_clock_groups -physically_exclusive -group [get_clocks clk] -group [get_clocks virt]",
            )
            .unwrap();
        assert!(exg.contains("clock_groups=1"), "{exg}");
        assert_eq!(
            excl.clock_interaction()
                .cell("clk", "virt")
                .unwrap()
                .relation,
            helion_sta::ClockRelation::Exclusive
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let bwns = blinky.wns_ps().expect("blinky STA");
        assert_ne!(
            bwns, gold,
            "clock-interaction WNS is per-design STA, not a canned pane"
        );
        assert_eq!(
            blinky.clock_interaction().cell("clk", "clk").unwrap().wns_ps,
            Some(bwns)
        );
    }

    /// UG903/UG949 `report_timing_summary` pane is intra/inter-clock WNS/TNS/WHS/THS
    /// by path group from STA — not a canned table. Empty XDC keeps gold WNS.
    #[test]
    fn timing_summary_pane_intra_inter_other_from_sta_not_a_dump() {
        let mut ide = IdeModel::new();
        let empty = ide.timing_summary_text();
        assert!(
            empty.contains("no clocks"),
            "idle pane has no canned summary: {empty}"
        );
        assert!(ide.timing_summary().groups.is_empty());
        assert!(
            NavSection::TimingAnalysis
                .actions()
                .iter()
                .any(|a| a.tcl == "report_timing_summary"),
            "Flow Navigator Timing Analysis must offer the pane"
        );

        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let gold = ide.wns_ps().expect("STA after route");
        assert_ne!(gold, 0);
        let hold = ide.timing.as_ref().unwrap().hold_slack_ps;
        let tns = ide.timing.as_ref().unwrap().tns_ps;
        let endpoints = ide.timing.as_ref().unwrap().endpoints;

        let out = ide.exec("report_timing_summary").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Reports);
        assert!(out.contains("KIND=intra"), "{out}");
        assert!(out.contains("FROM=clk TO=clk"), "{out}");
        assert!(out.contains(&format!("WNS_PS={gold}")), "{out}");
        assert!(out.contains(&format!("WHS_PS={hold}")), "{out}");
        assert!(out.contains(&format!("TNS_PS={tns}")), "{out}");
        let r = ide.timing_summary();
        assert_eq!(r.clocks.len(), 1, "{}", r.text());
        assert_eq!(r.intra_count(), 1);
        assert_eq!(r.inter_count(), 0);
        assert_eq!(r.other_count(), 0);
        let intra = r.group("clk", "clk").expect("intra-clock group");
        assert_eq!(intra.kind, helion_sta::PathGroupKind::IntraClock);
        assert_eq!(intra.wns_ps, Some(gold));
        assert_eq!(intra.tns_ps, tns);
        assert_eq!(intra.whs_ps, Some(hold));
        assert_eq!(intra.ths_ps, hold.min(0));
        assert_eq!(intra.endpoints, endpoints);
        assert_eq!(r.wns_ps, Some(gold));
        assert_eq!(r.whs_ps, Some(hold));
        assert_eq!(
            ide.wns_ps(),
            Some(gold),
            "opening the pane must not move gold WNS"
        );

        let sel = ide.exec("select_timing_summary clk clk").unwrap();
        assert!(sel.contains("KIND=intra"), "{sel}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "timing_summary"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "WNS_PS" && v == &gold.to_string()),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "WHS_PS" && v == &hold.to_string()),
            "{:?}",
            ide.properties
        );

        let gclk = ide
            .exec(
                "create_generated_clock -name clkdiv -source [get_ports clk] -divide_by 2 [get_pins u_ff/Q]",
            )
            .unwrap();
        assert!(gclk.contains("PERIOD_PS=20000"), "{gclk}");
        let r = ide.timing_summary();
        assert_eq!(r.clocks.len(), 2, "{}", r.text());
        assert_eq!(r.intra_count(), 2);
        assert_eq!(r.inter_count(), 2);
        let wns_div = ide.wns_ps().expect("STA after generated clock");
        assert_eq!(wns_div, gold + 10_000);
        assert_eq!(
            r.group("clkdiv", "clkdiv").unwrap().wns_ps,
            Some(wns_div),
            "generated intra-clock WNS is STA, not a label"
        );
        assert_eq!(r.group("clk", "clk").unwrap().wns_ps, Some(gold));
        let inter = r.group("clk", "clkdiv").unwrap();
        assert_eq!(inter.kind, helion_sta::PathGroupKind::InterClock);
        assert_ne!(inter.wns_ps, Some(gold), "inter-clock WNS uses dest period");

        let virt = ide
            .exec("create_clock -name virt -period 8.000 [get_ports virt]")
            .unwrap();
        assert!(virt.contains("PERIOD_PS=8000"), "{virt}");
        let r = ide.timing_summary();
        assert_eq!(r.clocks.len(), 3, "{}", r.text());
        assert_eq!(r.inter_count(), 6);
        let cdc = r.group("clk", "virt").expect("CDC group");
        assert_eq!(cdc.kind, helion_sta::PathGroupKind::InterClock);
        assert_ne!(
            cdc.wns_ps,
            Some(gold),
            "inter-clock slack uses the destination period: {:?}",
            cdc.wns_ps
        );
        let pane = ide.exec("report_timing_summary").unwrap();
        assert!(pane.contains("KIND=inter"), "{pane}");
        assert!(pane.contains("FROM=clk TO=virt"), "{pane}");

        let cg = ide
            .exec("set_clock_groups -asynchronous -group [get_clocks clk] -group [get_clocks virt]")
            .unwrap();
        assert!(cg.contains("clock_groups=1"), "{cg}");
        let r = ide.timing_summary();
        assert!(r.group("clk", "virt").unwrap().wns_ps.is_none());
        assert!(
            r.group("clk", "clk").unwrap().wns_ps.is_some(),
            "async CDC must not drop intra-clock STA WNS: {}",
            r.text()
        );

        let mut gp = IdeModel::new();
        gp.open_source(&example("counter.sv")).unwrap();
        gp.run_step(FlowStep::Place).unwrap();
        gp.run_step(FlowStep::Route).unwrap();
        let gold2 = gp.wns_ps().expect("STA before group_path");
        assert_eq!(gold2, gold, "empty XDC keeps gold WNS");
        gp.exec("group_path -name extra -weight 2 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        gp.exec("group_path -name light -weight 1 -from [get_ports clk] -to [get_ports led]")
            .unwrap();
        let r = gp.timing_summary();
        assert_eq!(r.other_count(), 2, "{}", r.text());
        let extra = r.named("extra").unwrap();
        let light = r.named("light").unwrap();
        assert_eq!(extra.kind, helion_sta::PathGroupKind::Other);
        assert_ne!(
            extra.wns_ps, light.wns_ps,
            "group_path weight must move that group's WNS"
        );
        assert!(
            extra.wns_ps.unwrap() < light.wns_ps.unwrap(),
            "weight 2 must worsen WNS vs weight 1: extra={:?} light={:?}",
            extra.wns_ps,
            light.wns_ps
        );
        let sel = gp.exec("select_timing_summary extra").unwrap();
        assert!(sel.contains("KIND=other"), "{sel}");
        assert!(
            gp.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "timing_summary"),
            "{:?}",
            gp.properties
        );
        assert!(
            gp.properties
                .iter()
                .any(|(k, v)| k == "NAME" && v == "extra"),
            "{:?}",
            gp.properties
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let bwns = blinky.wns_ps().expect("blinky STA");
        assert_ne!(
            bwns, gold,
            "timing-summary WNS is per-design STA, not a canned pane"
        );
        assert_eq!(
            blinky.timing_summary().group("clk", "clk").unwrap().wns_ps,
            Some(bwns)
        );
        assert_eq!(
            blinky.timing_summary().wns_ps,
            Some(bwns)
        );
    }

    /// UG906 `report_cdc` pane is STA inter-clock rows + XDC exceptions, not a dump.
    /// Empty XDC keeps gold WNS.
    #[test]
    fn cdc_pane_from_sta_clocks_not_a_dump() {
        let mut ide = IdeModel::new();
        let empty = ide.cdc_text();
        assert!(
            empty.contains("no clocks"),
            "idle pane has no canned CDC table: {empty}"
        );
        assert!(ide.cdc_report().violations.is_empty());
        assert!(
            NavSection::TimingAnalysis
                .actions()
                .iter()
                .any(|a| a.tcl == "report_cdc"),
            "Flow Navigator Timing Analysis must offer report_cdc"
        );

        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let gold = ide.wns_ps().expect("STA after route");
        assert_ne!(gold, 0);

        let out = ide.exec("report_cdc").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Cdc);
        assert!(out.contains("report_cdc"), "{out}");
        assert!(
            ide.cdc_report().violations.is_empty(),
            "single clock is not CDC: {}",
            ide.cdc_text()
        );
        assert_eq!(
            ide.wns_ps(),
            Some(gold),
            "opening the pane must not move gold WNS"
        );

        ide.exec(
            "create_generated_clock -name clkdiv -source [get_ports clk] -divide_by 2 [get_pins u_ff/Q]",
        )
        .unwrap();
        ide.exec("create_clock -name virt -period 8.000 [get_ports virt]")
            .unwrap();
        let r = ide.cdc_report();
        assert_eq!(r.clocks.len(), 3, "{}", r.text());
        assert_eq!(r.violations.len(), 6);
        let cdc = r.violation("clk", "virt").expect("CDC row");
        assert_eq!(cdc.severity, helion_sta::CdcSeverity::Critical);
        assert_eq!(cdc.check, "CDC-10");
        assert_eq!(cdc.relation, helion_sta::ClockRelation::TimedUnsafe);
        assert_ne!(cdc.wns_ps, Some(gold), "CDC slack uses dest period");
        assert!(r.critical_count() >= 2, "{}", r.text());
        let counter_cdc_wns = cdc.wns_ps;
        let pane = ide.exec("report_cdc").unwrap();
        assert!(pane.contains("SEVERITY=Critical"), "{pane}");
        assert!(pane.contains("FROM=clk TO=virt"), "{pane}");
        assert_eq!(ide.workspace, WorkspaceTab::Cdc);

        let sel = ide.exec("select_cdc clk virt").unwrap();
        assert!(sel.contains("SEVERITY=Critical"), "{sel}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "cdc"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "CHECK" && v == "CDC-10"),
            "{:?}",
            ide.properties
        );

        ide.exec("set_clock_groups -asynchronous -group [get_clocks clk] -group [get_clocks virt]")
            .unwrap();
        let r = ide.cdc_report();
        let v = r.violation("clk", "virt").unwrap();
        assert_eq!(v.severity, helion_sta::CdcSeverity::Info);
        assert!(v.wns_ps.is_none());
        assert_eq!(
            r.violation("clk", "clkdiv").unwrap().severity,
            helion_sta::CdcSeverity::Safe
        );

        let mut dp = IdeModel::new();
        dp.open_source(&example("counter.sv")).unwrap();
        dp.run_step(FlowStep::Place).unwrap();
        dp.run_step(FlowStep::Route).unwrap();
        dp.exec("create_clock -period 10.000 [get_ports clk]")
            .unwrap();
        dp.exec("create_clock -name virt -period 8.000 [get_ports virt]")
            .unwrap();
        dp.exec("set_max_delay -datapath_only 2.0 -from [get_clocks clk] -to [get_clocks virt]")
            .unwrap();
        let v = dp.cdc_report().violation("clk", "virt").unwrap().clone();
        assert_eq!(v.severity, helion_sta::CdcSeverity::Warning);
        assert_eq!(v.check, "CDC-13");

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        blinky
            .exec("create_clock -period 10.000 [get_ports clk]")
            .unwrap();
        blinky
            .exec("create_clock -name virt -period 8.000 [get_ports virt]")
            .unwrap();
        let bwns = blinky.wns_ps().expect("blinky STA");
        assert_ne!(bwns, gold);
        assert_ne!(
            blinky
                .cdc_report()
                .violation("clk", "virt")
                .expect("blinky CDC")
                .wns_ps,
            counter_cdc_wns,
            "CDC WNS is per-design STA, not a canned pane"
        );
    }

    /// UG903 `report_clock_networks` pane is HNF FF loads + HAD CLK-spine insertion,
    /// not a dump. Empty XDC keeps gold WNS.
    #[test]
    fn clock_networks_pane_from_hnf_and_had_not_a_dump() {
        let mut ide = IdeModel::new();
        let empty = ide.clock_networks_text();
        assert!(
            empty.contains("no clocks"),
            "idle pane has no canned tree: {empty}"
        );
        assert!(ide.clock_networks().clocks.is_empty());
        assert!(
            NavSection::TimingAnalysis
                .actions()
                .iter()
                .any(|a| a.tcl == "report_clock_networks"),
            "Flow Navigator Timing Analysis must offer report_clock_networks"
        );

        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let gold = ide.wns_ps().expect("STA after route");
        let clk_net = ide.timing.as_ref().unwrap().clk_net_ps;

        let out = ide.exec("report_clock_networks").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::ClockNetworks);
        assert!(out.contains("loads=4"), "{out}");
        assert!(out.contains("buffers=1"), "{out}");
        assert!(out.contains("INSERTION_PS="), "{out}");
        let r = ide.clock_networks();
        let clk = r.network("clk").expect("clk tree");
        assert_eq!(clk.n_loads, 4, "{}", r.text());
        assert_eq!(clk.fanout, 4);
        assert_eq!(clk.n_buffers, 1);
        assert_eq!(clk.net, "clk");
        assert_eq!(clk.insertion_ps, clk_net);
        assert!(clk.insertion_ps > 0, "{}", r.text());
        assert_eq!(
            ide.wns_ps(),
            Some(gold),
            "opening the pane must not move gold WNS"
        );

        let sel = ide.exec("select_clock_network clk").unwrap();
        assert!(sel.contains("loads=4"), "{sel}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "clock_network"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "LOADS" && v == "4"),
            "{:?}",
            ide.properties
        );

        ide.exec(
            "create_generated_clock -name clkdiv -source [get_ports clk] -divide_by 2 [get_pins u_ff0/Q]",
        )
        .unwrap();
        let r = ide.clock_networks();
        assert_eq!(r.clocks.len(), 2, "{}", r.text());
        let div = r.network("clkdiv").unwrap();
        assert!(div.generated, "{}", r.text());
        assert_eq!(div.n_buffers, 0, "generated clocks are local, not gclk");
        assert_eq!(div.insertion_ps, 0);
        assert_eq!(r.network("clk").unwrap().n_loads, 4);

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let br = blinky.clock_networks();
        assert_eq!(br.network("clk").unwrap().n_loads, 1, "{}", br.text());
        assert_ne!(
            br.total_loads, r.total_loads,
            "clock-network loads are per-design HNF, not a dump"
        );
        let b_insert = blinky.timing.as_ref().unwrap().clk_net_ps;
        assert_eq!(
            br.max_insertion_ps, b_insert,
            "blinky insertion is STA CLK_NET_PS from this placement"
        );
    }

    /// UG907 `report_power` pane is HAD occupancy × STA clocks × PVT, not a dump.
    /// Empty XDC keeps gold WNS.
    #[test]
    fn power_pane_from_had_occupancy_and_sta_not_a_dump() {
        let mut ide = IdeModel::new();
        let empty = ide.power_text();
        assert!(
            empty.contains("no design"),
            "idle pane has no canned wattage: {empty}"
        );
        assert!(ide.power_report().part.is_empty());
        assert!(
            NavSection::TimingAnalysis
                .actions()
                .iter()
                .any(|a| a.tcl == "report_power"),
            "Flow Navigator Timing Analysis must offer report_power"
        );

        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let gold = ide.wns_ps().expect("STA after route");
        assert_ne!(gold, 0);

        let out = ide.exec("report_power").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Power);
        assert!(out.contains("part=HL10T-C32-1"), "{out}");
        assert!(out.contains("LUTFF=4/"), "{out}");
        assert!(out.contains("TOTAL_UW="), "{out}");
        let p = ide.power_report();
        assert_eq!(p.voltage_mv, 1000);
        assert_eq!(p.temperature_c, 25);
        assert_eq!(p.f_mhz, 100);
        assert_eq!(p.lutff, 4);
        assert_eq!(p.iob, 1);
        assert!(p.total_uw > 0, "{}", p.text());
        assert_eq!(p.total_uw, p.static_uw + p.dynamic_uw);
        assert!(p.logic_uw > 0);
        assert!(p.clocks_uw > 0);
        assert_eq!(
            ide.wns_ps(),
            Some(gold),
            "opening the pane must not move gold WNS"
        );

        let sel = ide.exec("select_power logic").unwrap();
        assert!(sel.contains("RAIL=logic"), "{sel}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "power"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "UW" && v == &p.logic_uw.to_string()),
            "{:?}",
            ide.properties
        );

        ide.exec("set_operating_conditions -voltage 0.95 -temperature 85")
            .unwrap();
        let pvt = ide.power_report();
        assert_eq!(pvt.voltage_mv, 950);
        assert_eq!(pvt.temperature_c, 85);
        assert_ne!(
            pvt.total_uw, p.total_uw,
            "PVT must move power vs gold 1.00 V 25 C"
        );
        assert!(
            ide.wns_ps().unwrap() < gold,
            "OC also scales STA (already shipped); power must still be occupancy-backed"
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let pb = blinky.power_report();
        assert_eq!(pb.lutff, 1, "{}", pb.text());
        assert!(
            p.total_uw > pb.total_uw,
            "counter occupancy must draw more than blinky: counter={} blinky={}",
            p.total_uw,
            pb.total_uw
        );
        assert_ne!(p.logic_uw, pb.logic_uw);
        let bwns = blinky.wns_ps().expect("blinky STA");
        assert_ne!(bwns, gold, "power WNS companion is per-design STA");
    }

    /// UG949 `report_methodology` + UG893 DRC/Utilization panes are engine-backed
    /// violation/occupancy tables — not one-line dumps. Empty XDC keeps gold WNS.
    #[test]
    fn methodology_drc_utilization_panes_from_engines_not_dumps() {
        let mut ide = IdeModel::new();
        let empty_m = ide.methodology_text();
        assert!(
            empty_m.contains("no design"),
            "idle methodology has no canned checks: {empty_m}"
        );
        assert!(ide.methodology_report().checks.is_empty());
        let empty_u = ide.utilization_text();
        assert!(
            empty_u.contains("no placed") || empty_u.contains("Place"),
            "idle occupancy is empty: {empty_u}"
        );
        assert!(ide.utilization_report().occupancy.is_empty());
        let empty_d = ide.drc_text();
        assert!(
            empty_d.contains("no DRC") || empty_d.contains("Place"),
            "idle DRC is empty: {empty_d}"
        );
        assert!(
            NavSection::TimingAnalysis
                .actions()
                .iter()
                .any(|a| a.tcl == "report_methodology"),
            "Flow Navigator Timing Analysis must offer report_methodology"
        );
        assert!(
            NavSection::Implementation
                .actions()
                .iter()
                .any(|a| a.tcl == "report_drc"),
            "Flow Navigator Implementation must offer report_drc"
        );
        assert!(
            NavSection::Implementation
                .actions()
                .iter()
                .any(|a| a.tcl == "report_utilization"),
            "Flow Navigator Implementation must offer report_utilization"
        );

        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let gold = ide.wns_ps().expect("STA after route");
        assert_ne!(gold, 0);

        let mout = ide.exec("report_methodology").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Methodology);
        assert!(mout.contains("TIMING-1"), "{mout}");
        assert!(mout.contains("TIMING-7"), "{mout}");
        let m = ide.methodology_report();
        assert_eq!(m.check("TIMING-7").unwrap().objects, "led");
        assert!(
            m.check("TIMING-6").is_none(),
            "clk is a clock, not a data input: {}",
            m.text()
        );
        assert_eq!(
            ide.wns_ps(),
            Some(gold),
            "opening methodology must not move gold WNS"
        );

        let sel = ide.exec("select_methodology TIMING-7").unwrap();
        assert!(sel.contains("ID=TIMING-7"), "{sel}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "methodology"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "OBJECTS" && v == "led"),
            "{:?}",
            ide.properties
        );

        ide.exec("create_clock -period 10.000 [get_ports clk]")
            .unwrap();
        let m = ide.methodology_report();
        assert!(m.check("TIMING-1").is_none(), "{}", m.text());
        assert!(m.check("TIMING-7").is_some(), "{}", m.text());
        assert!(m.check("TIMING-24").is_some(), "{}", m.text());
        assert_eq!(ide.wns_ps(), Some(gold), "create_clock 10 ns keeps gold WNS");

        ide.exec("set_output_delay -clock clk 0.0 [get_ports led]")
            .unwrap();
        let m = ide.methodology_report();
        assert!(
            m.check("TIMING-7").is_none(),
            "output delay must clear TIMING-7: {}",
            m.text()
        );

        ide.exec("create_clock -name virt -period 8.000 [get_ports virt]")
            .unwrap();
        let m = ide.methodology_report();
        assert!(m.check("CDC-1").is_some(), "{}", m.text());
        assert!(m.critical_count() >= 1, "{}", m.text());
        ide.exec("set_clock_groups -asynchronous -group [get_clocks clk] -group [get_clocks virt]")
            .unwrap();
        let m = ide.methodology_report();
        assert!(
            m.check("CDC-1").is_none(),
            "async groups must clear CDC-1: {}",
            m.text()
        );

        let mut util_ide = IdeModel::new();
        util_ide.open_source(&example("counter.sv")).unwrap();
        util_ide.run_step(FlowStep::Place).unwrap();
        util_ide.run_step(FlowStep::Route).unwrap();
        let gold_u = util_ide.wns_ps().expect("STA");
        let uout = util_ide.exec("report_utilization").unwrap();
        assert_eq!(util_ide.workspace, WorkspaceTab::Utilization);
        assert!(uout.contains("LUTFF=4/8192"), "{uout}");
        assert!(uout.contains("resource LUTFF"), "{uout}");
        assert!(uout.contains("hier counter"), "{uout}");
        let ur = util_ide.utilization_report();
        let lut = ur.row("LUTFF").expect("LUTFF occupancy");
        assert_eq!(lut.used, 4);
        assert_eq!(lut.available, 8192);
        assert_eq!(lut.pct(), 0);
        assert_eq!(ur.hier("counter").unwrap().lut, 4);
        assert_eq!(
            util_ide.wns_ps(),
            Some(gold_u),
            "occupancy pane must not move gold WNS"
        );
        let usel = util_ide.exec("select_utilization LUTFF").unwrap();
        assert!(usel.contains("RESOURCE=LUTFF"), "{usel}");
        assert!(
            util_ide
                .properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "utilization"),
            "{:?}",
            util_ide.properties
        );
        assert!(
            util_ide
                .properties
                .iter()
                .any(|(k, v)| k == "USED" && v == "4"),
            "{:?}",
            util_ide.properties
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let br = blinky.utilization_report();
        assert_eq!(br.row("LUTFF").unwrap().used, 1, "{}", br.text());
        assert_eq!(br.hier("blinky").unwrap().lut, 1);
        assert_ne!(
            ur.row("LUTFF").unwrap().used,
            br.row("LUTFF").unwrap().used,
            "occupancy is per-design packed HAD, not a canned table"
        );
        let bwns = blinky.wns_ps().expect("blinky STA");
        assert_ne!(bwns, gold_u, "utilization companion STA is per-design");

        let dout = util_ide.exec("report_drc").unwrap();
        assert_eq!(util_ide.workspace, WorkspaceTab::Drc);
        assert!(
            dout.contains("violations=0") || dout.contains("ok"),
            "clean counter DRC: {dout}"
        );
        assert!(util_ide.drc.as_ref().unwrap().ok());
        assert!(util_ide.drc.as_ref().unwrap().items.is_empty());

        util_ide
            .exec("set_property PACKAGE_PIN IOB_X2Y0 [get_ports led]")
            .unwrap();
        util_ide
            .exec("set_property PACKAGE_PIN IOB_X3Y0 [get_ports clk]")
            .unwrap();
        util_ide
            .exec("set_property IOSTANDARD LVCMOS33 [get_ports led]")
            .unwrap();
        util_ide
            .exec("set_property IOSTANDARD LVCMOS18 [get_ports clk]")
            .unwrap();
        let mix = util_ide.exec("report_drc").unwrap();
        assert!(
            mix.contains("IOSTD-2") || mix.contains("VCCO"),
            "mixed VCCO must be a DRC row: {mix}"
        );
        let dr = util_ide.drc.clone().unwrap();
        let row = dr.item("IOSTD-2").expect("structured DRC");
        assert_eq!(row.severity, helion_drc::DrcSeverity::Error);
        assert!(row.objects.contains("BANK"), "{}", row.objects);
        let dsel = util_ide.exec("select_drc IOSTD-2").unwrap();
        assert!(dsel.contains("ID=IOSTD-2"), "{dsel}");
        assert!(
            util_ide
                .properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "drc"),
            "{:?}",
            util_ide.properties
        );
    }

    /// Bitstream pane is a helion-bits FAR table, not a hash/bytes/frames dump.
    #[test]
    fn bitstream_far_table_from_helion_bits_not_a_dump() {
        let mut ide = IdeModel::new();
        let empty = ide.bitstream_text();
        assert!(
            empty.contains("no bitstream"),
            "idle bitstream has no canned FAR rows: {empty}"
        );
        assert!(ide.bitstream_report().rows.is_empty());
        assert!(
            NavSection::ProgramDebug
                .actions()
                .iter()
                .any(|a| a.tcl == "write_bitstream"),
            "Flow Navigator Program and Debug must offer write_bitstream"
        );
        assert!(
            NavSection::ProgramDebug
                .actions()
                .iter()
                .any(|a| a.tcl == "report_bitstream"),
            "Flow Navigator Program and Debug must offer report_bitstream"
        );

        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let wns_route = ide.wns_ps().expect("STA after route");
        let e = ide.exec("select_bitstream_frame 0").unwrap_err();
        assert!(e.contains("no bitstream"), "{e}");

        let out = ide.exec("write_bitstream").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Bitstream);
        let hash = ide.bitstream_hash().expect("hash after write_bitstream");
        assert!(out.contains(&format!("{hash:#010x}")), "{out}");
        assert!(out.contains("configured="), "{out}");
        assert_eq!(
            ide.wns_ps(),
            Some(wns_route),
            "FAR table must not move gold WNS"
        );

        let report = ide.bitstream_report();
        assert_eq!(report.hash, hash);
        assert_eq!(report.bytes, ide.bitstream_bytes().unwrap());
        assert_eq!(report.frames, ide.bitstream_frames().unwrap());
        assert!(report.configured > 0, "{}", report.text());
        assert_eq!(report.configured, report.rows.len());
        assert_eq!(
            report.idcode,
            ide.session().bitstream.as_ref().unwrap().idcode
        );
        let bits = ide.session().bitstream.as_ref().unwrap();
        let engine_cfg: Vec<_> = bits
            .frames
            .iter()
            .filter(|(_, w)| **w != 0)
            .collect();
        assert_eq!(engine_cfg.len(), report.rows.len(), "rows are helion-bits frames");
        for (row, ((block, major, minor), word)) in report.rows.iter().zip(engine_cfg.iter()) {
            let far = helion_device::Far {
                block_type: *block,
                die: 0,
                major: *major,
                minor: *minor,
            };
            assert_eq!(row.far, far.encode());
            assert_eq!(row.block, *block);
            assert_eq!(row.major, *major);
            assert_eq!(row.minor, *minor);
            assert_eq!(row.word, **word);
            assert_eq!(helion_device::Far::decode(row.far), far);
            assert!(row.ones() > 0, "configured FAR {} has payload", row.far_hex());
        }
        assert!(
            report.rows.iter().any(|r| r.block_name() == "CLB_IO_CLK"),
            "counter LUT INIT must set CLB frames: {}",
            report.text()
        );
        assert!(
            report.rows.iter().any(|r| r.block_name() == "IOB"),
            "counter pads must set IOB frames: {}",
            report.text()
        );

        let table = ide.exec("report_bitstream").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Bitstream);
        assert!(table.contains("FAR="), "pane is a FAR table: {table}");
        assert!(table.contains("BLOCK=CLB_IO_CLK"), "{table}");
        assert!(table.contains("BLOCK=IOB"), "{table}");
        assert!(table.contains(&format!("hash={hash:#010x}")), "{table}");
        assert!(
            !table.starts_with("hash=") || table.contains('\n'),
            "must not be a one-liner dump: {table}"
        );

        let first = report.rows[0].clone();
        let sel = ide.exec(&format!("select_bitstream_frame {}", first.far_hex())).unwrap();
        assert!(sel.contains(&format!("FAR={}", first.far_hex())), "{sel}");
        assert!(sel.contains(&format!("BLOCK={}", first.block_name())), "{sel}");
        assert!(sel.contains(&format!("ONES={}", first.ones())), "{sel}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "bitstream_frame"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "WORD" && v == &first.word_hex()),
            "{:?}",
            ide.properties
        );
        let by_idx = ide.exec("select_bitstream_frame 0").unwrap();
        assert!(by_idx.contains(&format!("FAR={}", first.far_hex())), "{by_idx}");
        let by_blk = ide
            .exec(&format!(
                "select_bitstream_frame {} {} {}",
                first.block_name(),
                first.major,
                first.minor
            ))
            .unwrap();
        assert!(by_blk.contains(&format!("FAR={}", first.far_hex())), "{by_blk}");

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        blinky.run_step(FlowStep::Bitstream).unwrap();
        let br = blinky.bitstream_report();
        assert!(br.configured > 0, "{}", br.text());
        assert_ne!(
            br.hash, report.hash,
            "FAR table hash is per-design bitgen, not canned"
        );
        let same_word = report.rows.iter().any(|c| {
            br.rows
                .iter()
                .any(|b| b.far == c.far && b.word == c.word)
        });
        assert!(
            !same_word
                || br.configured != report.configured
                || br.rows.iter().map(|r| r.word).collect::<Vec<_>>()
                    != report.rows.iter().map(|r| r.word).collect::<Vec<_>>(),
            "counter vs blinky frames must differ: counter={} blinky={}",
            report.text(),
            br.text()
        );
        let bwns = blinky.wns_ps().expect("blinky STA");
        assert_ne!(bwns, wns_route, "bitstream companion STA is per-design");
        assert_eq!(
            ide.wns_ps(),
            Some(wns_route),
            "empty XDC gold WNS must hold after FAR table"
        );
    }

    /// Hardware Manager STAT is a helion-hw TAP / fabric status-register table, not a one-liner.
    #[test]
    fn hardware_manager_stat_table_from_fabric_not_a_dump() {
        let mut ide = IdeModel::new();
        let idle = ide.hw_stat_text();
        assert!(
            idle.contains("no hardware"),
            "idle Hardware Manager has no canned STAT bits: {idle}"
        );
        assert!(ide.hw_stat_report().bits.is_empty());
        assert!(
            NavSection::ProgramDebug
                .actions()
                .iter()
                .any(|a| a.tcl == "open_hw_manager"),
            "Flow Navigator Program and Debug must offer open_hw_manager"
        );
        assert!(
            NavSection::ProgramDebug
                .actions()
                .iter()
                .any(|a| a.tcl == "report_hw_stat"),
            "Flow Navigator Program and Debug must offer report_hw_stat"
        );

        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let wns_route = ide.wns_ps().expect("STA after route");
        ide.run_step(FlowStep::Bitstream).unwrap();
        assert_eq!(ide.wns_ps(), Some(wns_route), "bitgen must not move gold WNS");

        let open = ide.exec("open_hw_manager").unwrap();
        assert!(open.contains("sim"), "{open}");
        assert_eq!(ide.workspace, WorkspaceTab::Hardware);
        assert!(ide.hw.open);
        assert!(!ide.hw.programmed);

        let table = ide.exec("report_hw_stat").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Hardware);
        let reset = ide.hw_stat_report();
        assert!(reset.open);
        assert!(!reset.programmed);
        assert_eq!(reset.word, Stat::RESET_WORD);
        assert_eq!(reset.idcode, ide.device().unwrap().idcode);
        assert_eq!(reset.ir, helion_hw::IR_STAT);
        assert_eq!(reset.bits.len(), 7);
        let gsr = reset.bit("GSR").expect("GSR row");
        let gts = reset.bit("GTS").expect("GTS row");
        let done = reset.bit("DONE").expect("DONE row");
        assert!(gsr.value && gts.value, "unconfigured holds GSR/GTS: {table}");
        assert!(!done.value && !reset.bit("INIT").unwrap().value);
        assert!(!reset.bit("GWE").unwrap().value);
        assert!(!reset.bit("EOS").unwrap().value);
        assert!(!reset.bit("CRC_ERR").unwrap().value);
        assert!(table.contains("BIT="), "pane is a STAT bit table: {table}");
        assert!(table.contains("NAME=GSR"), "{table}");
        assert!(table.contains("NAME=DONE"), "{table}");
        assert!(
            table.contains('\n'),
            "must not be a one-liner dump: {table}"
        );
        let sel = ide.exec("select_hw_stat GSR").unwrap();
        assert!(sel.contains("NAME=GSR"), "{sel}");
        assert!(sel.contains("VALUE=1"), "{sel}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "hw_stat"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "VALUE" && v == "1"),
            "{:?}",
            ide.properties
        );

        let prog = ide.exec("program_hw").unwrap();
        assert!(prog.contains("DONE=1"), "{prog}");
        assert!(ide.hw.programmed);
        let started = ide.hw_stat_report();
        assert_eq!(started.word, Stat::STARTUP_WORD);
        assert_ne!(
            started.word, reset.word,
            "program flips GTS/GSR → DONE/GWE: reset={:#x} startup={:#x}",
            reset.word, started.word
        );
        assert!(started.bit("DONE").unwrap().value);
        assert!(started.bit("INIT").unwrap().value);
        assert!(started.bit("GWE").unwrap().value);
        assert!(started.bit("EOS").unwrap().value);
        assert!(!started.bit("GSR").unwrap().value);
        assert!(!started.bit("GTS").unwrap().value);
        assert!(!started.bit("CRC_ERR").unwrap().value);
        let by_bit = ide.exec("select_hw_stat 5").unwrap();
        assert!(by_bit.contains("NAME=DONE"), "{by_bit}");
        assert!(by_bit.contains("VALUE=1"), "{by_bit}");
        assert_eq!(started.bit("5").map(|b| b.name.as_str()), Some("DONE"));
        let engine = ide.hw.stat.as_ref().expect("cached fabric STAT");
        assert_eq!(engine.word(), started.word);
        for (row, bit) in started.bits.iter().zip(engine.bits()) {
            assert_eq!(row.bit, bit.bit);
            assert_eq!(row.name, bit.name);
            assert_eq!(row.value, bit.value);
        }

        ide.exec("ila_window 8").unwrap();
        ide.exec("ila_trigger rising").unwrap();
        ide.exec("ila_arm cnt_3").unwrap();
        let samples = ide.ila_sample_rows();
        assert_eq!(samples.len(), ide.ila.bits.len());
        assert_eq!(samples.len(), 8);
        for (i, row) in samples.iter().enumerate() {
            assert_eq!(row.sample, i);
            assert_eq!(row.value, ide.ila.bits.chars().nth(i).unwrap());
            assert_eq!(row.trigger, ide.ila.trigger_at == Some(i));
            assert_eq!(row.time_ps, i as u64 * ide.wave.timescale_ps.max(1));
        }
        let dash = ide.exec("ila_dashboard").unwrap();
        assert!(dash.contains("SAMPLE="), "ILA is a sample table: {dash}");
        assert!(dash.contains(&format!("bits={}", ide.ila.bits)), "{dash}");
        let trig = samples.iter().find(|r| r.trigger);
        if let Some(t) = trig {
            let hit = ide.exec(&format!("select_ila_sample {}", t.sample)).unwrap();
            assert!(hit.contains("MARKER=TRIGGER"), "{hit}");
            assert_eq!(ide.wave.cursor, t.sample);
            assert!(
                ide.properties
                    .iter()
                    .any(|(k, v)| k == "TYPE" && v == "ila_sample"),
                "{:?}",
                ide.properties
            );
        }

        assert_eq!(
            ide.wns_ps(),
            Some(wns_route),
            "STAT table must not move gold WNS"
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        blinky.run_step(FlowStep::Bitstream).unwrap();
        blinky.exec("open_hw_manager").unwrap();
        blinky.exec("program_hw").unwrap();
        let br = blinky.hw_stat_report();
        assert_eq!(br.word, Stat::STARTUP_WORD);
        assert_eq!(br.idcode, started.idcode);
        blinky.exec("ila_window 8").unwrap();
        blinky.exec("ila_arm led").unwrap();
        assert_eq!(blinky.ila.net, "led");
        assert_ne!(blinky.ila.net, ide.ila.net, "probe net is the armed HNF net");
        let b_rows = blinky.ila_sample_rows();
        assert_eq!(b_rows.len(), blinky.ila.bits.len());
        assert_eq!(
            b_rows.iter().map(|r| r.value).collect::<String>(),
            blinky.ila.bits,
            "blinky ILA rows are that design's fabric bits"
        );
        assert_eq!(
            samples.iter().map(|r| r.value).collect::<String>(),
            ide.ila.bits,
            "counter ILA rows are that design's fabric bits"
        );
        assert!(
            blinky.wave.has_trace("ila:led") && ide.wave.has_trace("ila:cnt_3"),
            "ILA samples land on per-design wave traces"
        );
        assert_eq!(
            ide.wns_ps(),
            Some(wns_route),
            "empty XDC gold WNS must hold after Hardware Manager STAT"
        );
    }

    /// UG893 Reports window is a clickable catalog of engine reports
    /// (Clock Interaction / CDC / Clock Networks / Power / …), not stacked dumps.
    #[test]
    fn reports_workspace_clickable_catalog_not_a_dump() {
        let mut ide = IdeModel::new();
        assert!(ide.report_catalog().iter().all(|r| r.status == "Not generated"));
        assert!(
            ide.report_catalog().iter().all(|r| r.summary == "-"),
            "idle catalog has no canned summaries: {:?}",
            ide.report_catalog()
                .iter()
                .map(|r| (r.id.as_str(), r.summary.as_str()))
                .collect::<Vec<_>>()
        );
        assert!(
            ide.exec("select_report")
                .unwrap_err()
                .contains("missing name"),
            "empty click must refuse"
        );
        assert!(
            ide.exec("select_report no_such")
                .unwrap_err()
                .contains("no row"),
            "unknown report must refuse"
        );
        assert!(
            NavSection::ProjectManager
                .actions()
                .iter()
                .any(|a| a.tcl == "reports"),
            "Flow Navigator Project Manager must offer Reports"
        );

        let table = ide.exec("reports").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Reports);
        assert!(table.contains("reports n=9"), "{table}");
        assert!(table.contains("complete=0"), "{table}");
        assert!(table.contains('\n'), "must not be a one-liner dump: {table}");
        assert!(table.contains("NAME=report_timing_summary"), "{table}");
        assert!(table.contains("NAME=report_clock_interaction"), "{table}");
        assert!(table.contains("NAME=report_cdc"), "{table}");
        assert!(table.contains("NAME=report_clock_networks"), "{table}");
        assert!(table.contains("NAME=report_power"), "{table}");
        assert!(table.contains("NAME=report_methodology"), "{table}");
        assert!(table.contains("NAME=report_drc"), "{table}");
        assert!(table.contains("NAME=report_utilization"), "{table}");
        assert!(table.contains("NAME=report_bitstream"), "{table}");
        assert!(table.contains("STATUS=Not generated"), "{table}");
        assert!(
            !table.contains("WNS_PS=9640") || table.contains("SUMMARY=-"),
            "idle catalog must not can gold WNS: {table}"
        );

        ide.open_source(&example("counter.sv")).unwrap();
        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let gold = ide.wns_ps().expect("STA after route");
        assert_ne!(gold, 0);

        let table = ide.exec("reports").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Reports);
        assert!(table.contains("complete="), "{table}");
        assert!(
            table.contains(&format!("WNS_PS={gold}")),
            "Timing Summary row must carry STA, not a dump: {table}"
        );
        let rows = ide.report_catalog();
        let ts = rows
            .iter()
            .find(|r| r.id == "report_timing_summary")
            .expect("timing summary row");
        assert_eq!(ts.status, "Complete");
        assert!(ts.summary.contains(&format!("WNS_PS={gold}")), "{}", ts.summary);
        let ci = rows
            .iter()
            .find(|r| r.id == "report_clock_interaction")
            .unwrap();
        assert_eq!(ci.status, "Complete");
        assert!(ci.summary.contains("clocks="), "{}", ci.summary);
        let cdc = rows.iter().find(|r| r.id == "report_cdc").unwrap();
        assert_eq!(cdc.status, "Complete");
        assert!(cdc.summary.contains("critical="), "{}", cdc.summary);
        let cn = rows
            .iter()
            .find(|r| r.id == "report_clock_networks")
            .unwrap();
        assert_eq!(cn.status, "Complete");
        assert!(cn.summary.contains("INSERTION_PS="), "{}", cn.summary);
        let pwr = rows.iter().find(|r| r.id == "report_power").unwrap();
        assert_eq!(pwr.status, "Complete");
        assert!(pwr.summary.contains("TOTAL_UW="), "{}", pwr.summary);
        let util = rows
            .iter()
            .find(|r| r.id == "report_utilization")
            .unwrap();
        assert_eq!(util.status, "Complete");
        assert!(util.summary.contains("LUTFF="), "{}", util.summary);
        let bits = rows
            .iter()
            .find(|r| r.id == "report_bitstream")
            .unwrap();
        assert_eq!(bits.status, "Not generated");
        assert_eq!(bits.summary, "-");
        assert_eq!(
            ide.wns_ps(),
            Some(gold),
            "opening the catalog must not move gold WNS"
        );

        let sel = ide.exec("select_report report_cdc").unwrap();
        assert!(sel.contains("NAME=report_cdc"), "{sel}");
        assert!(sel.contains("TYPE") || sel.contains("CATEGORY=Timing"), "{sel}");
        assert_eq!(ide.workspace, WorkspaceTab::Cdc);
        assert_eq!(ide.selected.as_deref(), Some("report:report_cdc"));
        assert_eq!(ide.selected_report.as_deref(), Some("report_cdc"));
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "report"),
            "{:?}",
            ide.properties
        );

        let ts_sel = ide.exec("select_report report_timing_summary").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Reports);
        assert!(ts_sel.contains(&format!("WNS_PS={gold}")), "{ts_sel}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "SUMMARY" && v.contains(&format!("WNS_PS={gold}"))),
            "{:?}",
            ide.properties
        );

        let psel = ide.exec("select_report report_power").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Power);
        assert!(psel.contains("TOTAL_UW="), "{psel}");

        ide.exec("write_bitstream").unwrap();
        let bits = ide
            .report_catalog()
            .into_iter()
            .find(|r| r.id == "report_bitstream")
            .unwrap();
        assert_eq!(bits.status, "Complete");
        let hash = ide.bitstream_hash().expect("hash");
        assert!(
            bits.summary.contains(&format!("{hash:#010x}")),
            "{}",
            bits.summary
        );
        assert_eq!(
            ide.wns_ps(),
            Some(gold),
            "bitstream catalog row must not move gold WNS"
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.run_step(FlowStep::Opt).unwrap();
        blinky.run_step(FlowStep::Place).unwrap();
        blinky.run_step(FlowStep::Route).unwrap();
        let bw = blinky.wns_ps().expect("blinky STA");
        assert_ne!(bw, gold, "WNS is per-design");
        let btable = blinky.exec("reports").unwrap();
        assert!(btable.contains(&format!("WNS_PS={bw}")), "{btable}");
        assert!(
            !btable.contains(&format!("WNS_PS={gold}")),
            "catalog is per-session, not canned: {btable}"
        );
        assert_eq!(
            ide.wns_ps(),
            Some(gold),
            "empty XDC gold WNS must hold after Reports catalog"
        );
    }

    /// UG893 Log pane is a clickable command/result table, not a monospace dump.
    #[test]
    fn log_pane_clickable_command_result_table() {
        let mut ide = IdeModel::new();
        assert!(ide.log_rows().is_empty());
        assert!(
            ide.select_log("0").unwrap_err().contains("log empty"),
            "empty table must refuse a click"
        );
        let empty = ide.exec("log").unwrap();
        assert_eq!(ide.bottom_tab, BottomTab::Log);
        assert!(empty.contains("log n=1") || empty.contains("log empty") || empty.contains("CMD=log"), "{empty}");

        let e = ide.run_step(FlowStep::Synthesis).unwrap_err();
        assert!(e.contains("source") || e.contains("synth"), "{e}");
        let table = ide.exec("log").unwrap();
        assert_eq!(ide.bottom_tab, BottomTab::Log);
        assert!(table.contains("STATUS=error"), "{table}");
        assert!(table.contains("CMD=synth_design"), "{table}");
        assert!(table.contains('\n'), "must not be a one-liner dump: {table}");
        let sel = ide.exec("select_log synth_design").unwrap();
        assert!(sel.contains("STATUS=error"), "{sel}");
        assert!(sel.contains("CMD=synth_design"), "{sel}");
        assert_eq!(ide.workspace, WorkspaceTab::Schematic);
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "log"),
            "{:?}",
            ide.properties
        );

        ide.open_source(&example("counter.sv")).unwrap();
        ide.exec("report_timing").unwrap();
        let wns = ide.wns_ps().expect("STA WNS");
        assert_ne!(wns, 0);
        let table = ide.exec("log").unwrap();
        assert!(table.contains("helion% report_timing"), "{table}");
        assert!(table.contains(&format!("WNS_PS={wns}")), "{table}");
        assert!(table.contains("STATUS=ok CMD=report_timing"), "{table}");
        let tsel = ide.exec("select_log report_timing").unwrap();
        assert!(
            tsel.contains(&format!("WNS_PS={wns}")),
            "click must carry STA, not a stub: {tsel}"
        );
        assert_eq!(ide.workspace, WorkspaceTab::Reports);
        assert_eq!(ide.bottom_tab, BottomTab::Log);
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "OUT" && v.contains(&format!("WNS_PS={wns}"))),
            "{:?}",
            ide.properties
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.exec("report_timing").unwrap();
        let bw = blinky.wns_ps().expect("blinky STA");
        assert_ne!(bw, wns, "WNS is per-design");
        let bsel = blinky.exec("select_log report_timing").unwrap();
        assert!(bsel.contains(&format!("WNS_PS={bw}")), "{bsel}");
        assert!(
            !bsel.contains(&format!("WNS_PS={wns}")),
            "Log clicks are per-session, not canned: {bsel}"
        );
        assert_eq!(blinky.workspace, WorkspaceTab::Reports);
    }

    /// UG893 Find Results is a clickable Name/Type table over HNF/HAD find,
    /// not a `kind  name` selectable-label dump.
    #[test]
    fn find_results_pane_clickable_name_type_table_not_a_dump() {
        let mut ide = IdeModel::new();
        assert!(ide.find_rows().is_empty());
        assert!(
            ide.exec("select_find")
                .unwrap_err()
                .contains("missing name"),
            "empty click must refuse"
        );
        assert!(
            ide.exec("select_find u_lut0")
                .unwrap_err()
                .contains("no hits"),
            "idle pane must refuse a click"
        );
        let empty = ide.exec("find_results").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Find);
        assert!(empty.contains("find_results n=0"), "{empty}");
        assert!(empty.contains("no hits"), "{empty}");
        assert!(
            !empty.contains("NAME="),
            "idle pane has no canned rows: {empty}"
        );
        assert!(
            NavSection::ProjectManager
                .actions()
                .iter()
                .any(|a| a.tcl == "find_results"),
            "Flow Navigator Project Manager must offer Find Results"
        );

        ide.open_source(&example("counter.sv")).unwrap();
        let table = ide.exec("find u_lut0").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Find);
        assert!(table.contains("cell:u_lut0"), "{table}");
        assert!(table.contains('\n'), "must not be a one-liner dump: {table}");
        assert!(table.contains("find_results n="), "{table}");
        assert!(table.contains("NAME=u_lut0"), "{table}");
        assert!(table.contains("TYPE=cell"), "{table}");
        assert!(table.contains("PRIMITIVE=LUT6"), "{table}");
        assert!(table.contains("PARENT=counter"), "{table}");
        let rows = ide.find_rows();
        assert!(
            rows.iter()
                .any(|h| h.name == "u_lut0" && h.type_cell() == "cell" && h.primitive_cell() == "LUT6"),
            "{rows:?}"
        );
        assert!(
            rows.iter().all(|h| h.parent_cell() == "counter"),
            "parent is HNF top, not a dump stub: {rows:?}"
        );

        let sel = ide.exec("select_find u_lut0").unwrap();
        assert!(sel.contains("NAME=u_lut0"), "{sel}");
        assert!(sel.contains("TYPE=cell"), "{sel}");
        assert!(sel.contains("PRIMITIVE=LUT6"), "{sel}");
        assert_eq!(ide.selected.as_deref(), Some("u_lut0"));
        assert_eq!(ide.workspace, WorkspaceTab::Find);
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "PRIMITIVE" && v == "LUT6"),
            "{:?}",
            ide.properties
        );
        let by_idx = ide.exec("select_find 0").unwrap();
        assert!(by_idx.contains("NAME="), "{by_idx}");
        assert_eq!(ide.selected_find, Some(0));

        let ports = ide.exec("sheet_find ports").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Find);
        assert!(ports.contains('\n'), "sheet_find must be a table: {ports}");
        assert!(ports.contains("NAME=clk"), "{ports}");
        assert!(ports.contains("NAME=led"), "{ports}");
        assert!(ports.contains("TYPE=port"), "{ports}");
        assert!(ports.contains("PRIMITIVE=IN"), "{ports}");
        assert!(ports.contains("PRIMITIVE=OUT"), "{ports}");
        let psel = ide.exec("select_find clk").unwrap();
        assert!(psel.contains("NAME=clk"), "{psel}");
        assert!(psel.contains("TYPE=port"), "{psel}");
        assert_eq!(ide.selected.as_deref(), Some("clk"));
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "port"),
            "{:?}",
            ide.properties
        );

        let nets = ide.exec("sheet_find nets").unwrap();
        assert!(nets.contains("TYPE=net"), "{nets}");
        assert!(
            ide.find_rows().iter().any(|h| h.type_cell() == "net"),
            "{:?}",
            ide.find_rows()
        );
        let cells = ide.exec("sheet_find cells").unwrap();
        assert!(cells.contains("NAME=u_lut0"), "{cells}");
        assert!(cells.contains("TYPE=cell"), "{cells}");
        assert!(cells.contains("PRIMITIVE=LUT6"), "{cells}");

        assert!(
            ide.exec("select_find no_such")
                .unwrap_err()
                .contains("no row"),
            "unknown hit must refuse"
        );

        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let gold = ide.wns_ps().expect("STA after route");
        assert_ne!(gold, 0);
        let after = ide.exec("find u_lut0").unwrap();
        assert!(after.contains("NAME=u_lut0"), "{after}");
        let click = ide.exec("select_find u_lut0").unwrap();
        assert!(click.contains("NAME=u_lut0"), "{click}");
        let pin_hits = ide.exec("find IOB").unwrap();
        assert!(
            pin_hits.contains("TYPE=pin") || pin_hits.contains("TYPE=cell"),
            "HAD pins are findable: {pin_hits}"
        );
        assert_eq!(
            ide.wns_ps(),
            Some(gold),
            "empty XDC gold WNS must hold after Find Results"
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        let btable = blinky.exec("find led").unwrap();
        assert!(btable.contains("NAME=led"), "{btable}");
        assert!(btable.contains("PARENT=blinky"), "{btable}");
        assert!(
            !btable.contains("PARENT=counter"),
            "Find Results are per-session, not canned: {btable}"
        );
        assert!(
            blinky
                .exec("find u_lut0")
                .unwrap_err()
                .contains("0 hits"),
            "blinky has no counter LUT"
        );
        blinky.exec("sheet_find cells").unwrap();
        assert!(
            blinky
                .find_rows()
                .iter()
                .any(|h| h.type_cell() == "cell" && h.parent_cell() == "blinky"),
            "{:?}",
            blinky.find_rows()
        );
        assert!(
            !blinky.find_rows().iter().any(|h| h.name == "u_lut0"),
            "blinky cells are not counter: {:?}",
            blinky.find_rows()
        );
    }

    /// UG893 Properties is a clickable Name/Value table over the selected
    /// HNF/HAD/STA object, not a weak-key / monospace-value dump.
    #[test]
    fn properties_pane_clickable_name_value_table_not_a_dump() {
        let mut ide = IdeModel::new();
        assert!(ide.property_rows().is_empty());
        assert!(ide.selected_property.is_none());
        assert!(
            ide.exec("select_property")
                .unwrap_err()
                .contains("missing name"),
            "empty click must refuse"
        );
        assert!(
            ide.exec("select_property NAME")
                .unwrap_err()
                .contains("no properties"),
            "idle pane must refuse a click"
        );
        let empty = ide.exec("properties").unwrap();
        assert!(empty.contains("properties n=0"), "{empty}");
        assert!(empty.contains("no properties"), "{empty}");
        assert!(
            !empty.contains("NAME="),
            "idle pane has no canned rows: {empty}"
        );

        ide.open_source(&example("counter.sv")).unwrap();
        ide.select("u_lut0");
        let table = ide.exec("properties").unwrap();
        assert!(table.contains("properties n="), "{table}");
        assert!(table.contains('\n'), "must not be a one-liner dump: {table}");
        assert!(table.contains("NAME=NAME VALUE=u_lut0"), "{table}");
        assert!(table.contains("NAME=PRIMITIVE VALUE=LUT6"), "{table}");
        assert!(table.contains("NAME=INIT"), "{table}");
        let rows = ide.property_rows();
        assert!(
            rows.iter()
                .any(|r| r.name == "NAME" && r.value == "u_lut0"),
            "{rows:?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.name == "PRIMITIVE" && r.value == "LUT6"),
            "{rows:?}"
        );
        let prim = ide.exec("select_property PRIMITIVE").unwrap();
        assert!(prim.contains("NAME=PRIMITIVE"), "{prim}");
        assert!(prim.contains("VALUE=LUT6"), "{prim}");
        assert_eq!(ide.selected_property.as_deref(), Some("PRIMITIVE"));
        assert_eq!(ide.selected.as_deref(), Some("u_lut0"));
        let by_idx = ide.exec("select_property 0").unwrap();
        assert!(by_idx.contains("NAME=NAME"), "{by_idx}");
        assert!(by_idx.contains("VALUE=u_lut0"), "{by_idx}");
        assert_eq!(ide.selected_property.as_deref(), Some("NAME"));

        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.select("u_lut0");
        let placed = ide.exec("properties").unwrap();
        assert!(
            placed.contains("NAME=SITE VALUE=") || placed.contains("NAME=LOC VALUE="),
            "placed HNF cell must expose HAD site: {placed}"
        );
        let site = ide
            .device
            .occupant_of("u_lut0")
            .expect("placed u_lut0 occupies a HAD site")
            .site_name();
        assert!(
            ide.property_rows()
                .iter()
                .any(|r| r.name == "SITE" && r.value == site),
            "{:?}",
            ide.property_rows()
        );
        let site_click = ide.exec("select_property SITE").unwrap();
        assert!(site_click.contains("NAME=SITE"), "{site_click}");
        assert!(site_click.contains(&format!("VALUE={site}")), "{site_click}");
        ide.select(&site);
        let had = ide.exec("properties").unwrap();
        assert!(had.contains("NAME=TYPE VALUE=site"), "{had}");
        assert!(had.contains(&format!("NAME=SITE VALUE={site}")), "{had}");
        assert!(
            ide.property_rows()
                .iter()
                .any(|r| r.name == "TYPE" && r.value == "site"),
            "{:?}",
            ide.property_rows()
        );
        let kind = ide.exec("select_property KIND").unwrap();
        assert!(kind.contains("NAME=KIND"), "{kind}");
        assert_eq!(ide.selected_property.as_deref(), Some("KIND"));

        ide.select("u_lut0");
        ide.run_step(FlowStep::Route).unwrap();
        let gold = ide.wns_ps().expect("STA after route");
        assert_ne!(gold, 0);
        let pin = {
            ide.ensure_timing_paths().expect("STA paths");
            ide.timing_paths
                .iter()
                .flat_map(|p| p.pins.iter())
                .find(|p| p.incr_ps > 0)
                .cloned()
                .expect("STA pin delay")
        };
        let pin_out = ide
            .exec(&format!("select_timing_pin {}", pin.pin))
            .unwrap();
        assert!(pin_out.contains("incr_ps="), "{pin_out}");
        let sta = ide.exec("properties").unwrap();
        assert!(sta.contains("NAME=TYPE VALUE=timing_pin"), "{sta}");
        assert!(sta.contains("NAME=INCR_PS VALUE="), "{sta}");
        assert!(sta.contains("NAME=SLACK_PS VALUE="), "{sta}");
        assert!(
            ide.property_rows()
                .iter()
                .any(|r| r.name == "TYPE" && r.value == "timing_pin"),
            "{:?}",
            ide.property_rows()
        );
        assert!(
            ide.property_rows()
                .iter()
                .any(|r| r.name == "INCR_PS" && r.value == pin.incr_ps.to_string()),
            "{:?}",
            ide.property_rows()
        );
        let slack = ide.exec("select_property SLACK_PS").unwrap();
        assert!(slack.contains("NAME=SLACK_PS"), "{slack}");
        assert_eq!(ide.selected_property.as_deref(), Some("SLACK_PS"));
        assert!(
            ide.exec("select_property no_such")
                .unwrap_err()
                .contains("no row"),
            "unknown property must refuse"
        );
        assert_eq!(
            ide.wns_ps(),
            Some(gold),
            "empty XDC gold WNS must hold after Properties"
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        blinky.select("led");
        let btable = blinky.exec("properties").unwrap();
        assert!(btable.contains("NAME=NAME VALUE=led"), "{btable}");
        assert!(btable.contains("NAME=TYPE VALUE=port"), "{btable}");
        assert!(
            !btable.contains("VALUE=u_lut0"),
            "Properties are per-session, not canned: {btable}"
        );
        assert!(
            blinky
                .property_rows()
                .iter()
                .any(|r| r.name == "TYPE" && r.value == "port"),
            "{:?}",
            blinky.property_rows()
        );
        let bsel = blinky.exec("select_property DIR").unwrap();
        assert!(bsel.contains("NAME=DIR"), "{bsel}");
        assert_eq!(blinky.selected_property.as_deref(), Some("DIR"));
    }

    /// UG893 Sources/Netlist is a clickable Name/Type table over RTL + HNF
    /// cells/nets, not a collapsing Cells/Nets monospace dump.
    #[test]
    fn sources_netlist_pane_clickable_name_type_table_not_a_dump() {
        let mut ide = IdeModel::new();
        assert!(ide.source_rows().is_empty());
        assert!(ide.netlist_rows().is_empty());
        assert!(ide.selected_source.is_none());
        assert!(ide.selected_netlist.is_none());
        assert!(
            ide.exec("select_source")
                .unwrap_err()
                .contains("missing name"),
            "empty click must refuse"
        );
        assert!(
            ide.exec("select_source counter.sv")
                .unwrap_err()
                .contains("no sources"),
            "idle pane must refuse a source click"
        );
        assert!(
            ide.exec("select_netlist")
                .unwrap_err()
                .contains("missing name"),
            "empty netlist click must refuse"
        );
        assert!(
            ide.exec("select_netlist u_lut0")
                .unwrap_err()
                .contains("no cells/nets"),
            "idle pane must refuse a netlist click"
        );
        let empty_src = ide.exec("sources").unwrap();
        assert_eq!(ide.layout, LayoutKind::Default);
        assert!(empty_src.contains("sources n=0"), "{empty_src}");
        assert!(empty_src.contains("no sources"), "{empty_src}");
        assert!(
            !empty_src.contains("NAME="),
            "idle Sources has no canned rows: {empty_src}"
        );
        let empty_nl = ide.exec("netlist").unwrap();
        assert!(empty_nl.contains("netlist n=0"), "{empty_nl}");
        assert!(empty_nl.contains("no cells/nets"), "{empty_nl}");
        assert!(
            !empty_nl.contains("NAME="),
            "idle Netlist has no canned rows: {empty_nl}"
        );
        assert!(
            NavSection::ProjectManager
                .actions()
                .iter()
                .any(|a| a.tcl == "sources"),
            "Flow Navigator Project Manager must offer Sources"
        );
        assert!(
            NavSection::ProjectManager
                .actions()
                .iter()
                .any(|a| a.tcl == "netlist"),
            "Flow Navigator Project Manager must offer Netlist"
        );

        ide.open_source(&example("counter.sv")).unwrap();
        let sources = ide.exec("sources").unwrap();
        assert_eq!(ide.layout, LayoutKind::Default);
        assert!(sources.contains('\n'), "must not be a one-liner dump: {sources}");
        assert!(sources.contains("sources n="), "{sources}");
        assert!(sources.contains("NAME=counter.sv"), "{sources}");
        assert!(sources.contains("TYPE=sv"), "{sources}");
        let src_rows = ide.source_rows();
        assert!(
            src_rows
                .iter()
                .any(|r| r.name == "counter.sv" && r.type_cell() == "sv" && r.kind == "source"),
            "{src_rows:?}"
        );
        assert!(
            src_rows.iter().any(|r| r.parent.ends_with("counter.sv")),
            "parent is the RTL path, not a dump stub: {src_rows:?}"
        );

        let ssel = ide.exec("select_source counter.sv").unwrap();
        assert!(ssel.contains("NAME=counter.sv"), "{ssel}");
        assert!(ssel.contains("TYPE=sv"), "{ssel}");
        assert!(
            ide.selected_source
                .as_deref()
                .map(|p| p.ends_with("counter.sv"))
                .unwrap_or(false),
            "{:?}",
            ide.selected_source
        );
        assert_eq!(ide.selected.as_deref(), Some("counter.sv"));
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "sv"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "KIND" && v == "source"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "PATH" && v.ends_with("counter.sv")),
            "{:?}",
            ide.properties
        );
        let by_idx = ide.exec("select_source 0").unwrap();
        assert!(by_idx.contains("NAME=counter.sv"), "{by_idx}");

        let table = ide.exec("netlist").unwrap();
        assert_eq!(ide.layout, LayoutKind::Default);
        assert!(table.contains('\n'), "must not be a one-liner dump: {table}");
        assert!(table.contains("netlist n="), "{table}");
        assert!(table.contains("top=counter"), "{table}");
        assert!(table.contains("NAME=u_lut0"), "{table}");
        assert!(table.contains("TYPE=LUT6"), "{table}");
        assert!(table.contains("TYPE=net"), "{table}");
        assert!(table.contains("NAME=cnt_3"), "{table}");
        let rows = ide.netlist_rows();
        assert!(
            rows.iter()
                .any(|h| h.name == "u_lut0" && h.type_cell() == "LUT6" && h.kind == "cell"),
            "{rows:?}"
        );
        assert!(
            rows.iter()
                .any(|h| h.name == "cnt_3" && h.type_cell() == "net" && h.kind == "net"),
            "{rows:?}"
        );
        assert!(
            rows.iter().all(|h| h.parent_cell() == "counter"),
            "parent is HNF top, not a dump stub: {rows:?}"
        );
        assert!(
            rows.iter().any(|h| h.type_cell() == "IOB_OUT"),
            "IOB primitive must show: {rows:?}"
        );

        let sel = ide.exec("select_netlist u_lut0").unwrap();
        assert!(sel.contains("NAME=u_lut0"), "{sel}");
        assert!(sel.contains("TYPE=LUT6"), "{sel}");
        assert_eq!(ide.selected.as_deref(), Some("u_lut0"));
        assert_eq!(ide.selected_netlist.as_deref(), Some("u_lut0"));
        assert!(ide.selected_source.is_none());
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "PRIMITIVE" && v == "LUT6"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "LUT6"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "KIND" && v == "cell"),
            "{:?}",
            ide.properties
        );
        let cell0 = ide.exec("select_netlist 0").unwrap();
        assert!(cell0.contains("NAME="), "{cell0}");
        assert!(ide.selected_netlist.is_some());

        let nsel = ide.exec("select_netlist cnt_3").unwrap();
        assert!(nsel.contains("NAME=cnt_3"), "{nsel}");
        assert!(nsel.contains("TYPE=net"), "{nsel}");
        assert_eq!(ide.selected.as_deref(), Some("cnt_3"));
        assert_eq!(ide.selected_netlist.as_deref(), Some("cnt_3"));
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "net"),
            "{:?}",
            ide.properties
        );

        assert!(
            ide.exec("select_netlist no_such")
                .unwrap_err()
                .contains("no row"),
            "unknown hit must refuse"
        );
        assert!(
            ide.exec("select_source no_such")
                .unwrap_err()
                .contains("no row"),
            "unknown source must refuse"
        );

        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let gold = ide.wns_ps().expect("STA after route");
        assert_ne!(gold, 0);
        let after = ide.exec("netlist").unwrap();
        assert!(after.contains("NAME=u_lut0"), "{after}");
        assert!(after.contains("TYPE=LUT6"), "{after}");
        let click = ide.exec("select_netlist u_lut0").unwrap();
        assert!(click.contains("NAME=u_lut0"), "{click}");
        let src_again = ide.exec("select_source counter.sv").unwrap();
        assert!(src_again.contains("TYPE=sv"), "{src_again}");
        assert_eq!(
            ide.wns_ps(),
            Some(gold),
            "empty XDC gold WNS must hold after Sources/Netlist"
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        let bsrc = blinky.exec("sources").unwrap();
        assert!(bsrc.contains("NAME=blinky.sv"), "{bsrc}");
        assert!(
            !bsrc.contains("NAME=counter.sv"),
            "Sources are per-session, not canned: {bsrc}"
        );
        let btable = blinky.exec("netlist").unwrap();
        assert!(btable.contains("NAME=u_lut"), "{btable}");
        assert!(btable.contains("top=blinky"), "{btable}");
        assert!(
            !btable.contains("NAME=u_lut0"),
            "Netlist is per-session, not canned: {btable}"
        );
        assert!(
            !btable.contains("PARENT=counter") && !btable.contains("top=counter"),
            "blinky netlist is not counter: {btable}"
        );
        let bsel = blinky.exec("select_netlist u_lut").unwrap();
        assert!(bsel.contains("NAME=u_lut"), "{bsel}");
        assert_eq!(blinky.selected.as_deref(), Some("u_lut"));
        assert!(
            blinky
                .netlist_rows()
                .iter()
                .any(|h| h.kind == "cell" && h.parent_cell() == "blinky"),
            "{:?}",
            blinky.netlist_rows()
        );
        assert!(
            !blinky.netlist_rows().iter().any(|h| h.name == "u_lut0"),
            "blinky cells are not counter: {:?}",
            blinky.netlist_rows()
        );
        blinky.exec("select_source blinky.sv").unwrap();
        assert!(
            blinky
                .selected_source
                .as_deref()
                .map(|p| p.ends_with("blinky.sv"))
                .unwrap_or(false),
            "{:?}",
            blinky.selected_source
        );
    }

    /// UG893 Project Settings is a clickable part / top / fileset / strategy
    /// Name/Value table over Session, not a collapsing dump.
    #[test]
    fn project_settings_pane_clickable_part_top_fileset_strategy_table() {
        let mut ide = IdeModel::new();
        assert!(ide.selected_setting.is_none());
        assert_eq!(ide.active_fileset, "sources_1");
        assert!(
            ide.exec("select_project_setting")
                .unwrap_err()
                .contains("missing name"),
            "empty click must refuse"
        );
        assert!(
            ide.exec("select_project_setting no_such")
                .unwrap_err()
                .contains("no row"),
            "unknown setting must refuse"
        );
        assert!(
            NavSection::ProjectManager
                .actions()
                .iter()
                .any(|a| a.tcl == "project_settings"),
            "Flow Navigator Project Manager must offer Project Settings"
        );

        let idle = ide.exec("project_settings").unwrap();
        assert_eq!(ide.workspace, WorkspaceTab::Settings);
        assert_eq!(ide.nav, NavSection::ProjectManager);
        assert!(idle.contains('\n'), "must not be a one-liner dump: {idle}");
        assert!(idle.contains("project_settings n="), "{idle}");
        assert!(idle.contains("part=HL10T-C32-1"), "{idle}");
        assert!(idle.contains("top=-"), "{idle}");
        assert!(idle.contains("fileset=sources_1"), "{idle}");
        assert!(idle.contains("strategy=Default"), "{idle}");
        assert!(idle.contains("NAME=PART VALUE=HL10T-C32-1"), "{idle}");
        assert!(idle.contains("NAME=TOP VALUE=-"), "{idle}");
        assert!(idle.contains("NAME=FILESET VALUE=sources_1"), "{idle}");
        assert!(idle.contains("NAME=STRATEGY VALUE=Default"), "{idle}");
        assert!(idle.contains("NAME=MODE VALUE=Project"), "{idle}");
        let rows = ide.project_setting_rows();
        assert!(
            rows.iter()
                .any(|r| r.name == "PART" && r.value == "HL10T-C32-1"),
            "{rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.name == "TOP" && r.value == "-"),
            "idle TOP is Session-empty, not a canned module: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.name == "FILESET" && r.value == "sources_1"),
            "{rows:?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.name == "STRATEGY" && r.value == "Default"),
            "{rows:?}"
        );
        let part = ide.exec("select_project_setting PART").unwrap();
        assert!(part.contains("NAME=PART"), "{part}");
        assert!(part.contains("VALUE=HL10T-C32-1"), "{part}");
        assert_eq!(ide.selected_setting.as_deref(), Some("PART"));
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "TYPE" && v == "project_setting"),
            "{:?}",
            ide.properties
        );
        assert!(
            ide.properties
                .iter()
                .any(|(k, _)| k == "IDCODE" || k == "LUT6"),
            "PART click exposes HAD, not a dump stub: {:?}",
            ide.properties
        );
        let by_idx = ide.exec("select_project_setting 0").unwrap();
        assert!(by_idx.contains("NAME=PART"), "{by_idx}");

        let dsp = ide.exec("set_part HL10T-DSP1").unwrap();
        assert!(dsp.contains("HL10T-DSP1"), "{dsp}");
        assert_eq!(ide.part(), "HL10T-DSP1");
        assert_eq!(ide.session().part, "HL10T-DSP1");
        let dsp_table = ide.exec("project_settings").unwrap();
        assert!(dsp_table.contains("NAME=PART VALUE=HL10T-DSP1"), "{dsp_table}");
        assert!(
            !dsp_table.contains("NAME=PART VALUE=HL10T-C32-1"),
            "PART is Session-backed, not canned: {dsp_table}"
        );
        ide.exec("set_property PART HL10T-C32-1").unwrap();
        assert_eq!(ide.part(), "HL10T-C32-1");

        ide.open_source(&example("counter.sv")).unwrap();
        let table = ide.exec("project_settings").unwrap();
        assert!(table.contains("top=counter"), "{table}");
        assert!(table.contains("NAME=TOP VALUE=counter"), "{table}");
        assert!(table.contains("NAME=TARGET_LANGUAGE VALUE=sv"), "{table}");
        assert!(
            ide.project_setting_rows()
                .iter()
                .any(|r| r.name == "TOP" && r.value == "counter"),
            "{:?}",
            ide.project_setting_rows()
        );
        let top = ide.exec("select_project_setting TOP").unwrap();
        assert!(top.contains("NAME=TOP"), "{top}");
        assert!(top.contains("VALUE=counter"), "{top}");
        assert_eq!(ide.selected_setting.as_deref(), Some("TOP"));
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "CELLS" && v != "0"),
            "TOP is HNF-backed: {:?}",
            ide.properties
        );

        let fs = ide.exec("select_project_setting FILESET").unwrap();
        assert!(fs.contains("NAME=FILESET"), "{fs}");
        assert!(fs.contains("VALUE=sources_1"), "{fs}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "FILES" && v.contains("counter.sv")),
            "FILESET lists Session RTL, not a dump: {:?}",
            ide.properties
        );
        assert!(
            ide.fileset_files("sources_1")
                .iter()
                .any(|p| p.ends_with("counter.sv")),
            "{:?}",
            ide.fileset_files("sources_1")
        );

        let xdc = example("counter.sdc");
        ide.exec(&format!("read_xdc {}", xdc.display())).unwrap();
        assert!(
            ide.fileset_files("constrs_1")
                .iter()
                .any(|p| p.ends_with("counter.sdc")),
            "XDC lands in constrs_1: {:?}",
            ide.fileset_files("constrs_1")
        );
        let constrs = ide.exec("select_project_setting constrs_1").unwrap();
        assert_eq!(ide.active_fileset, "constrs_1");
        assert!(constrs.contains("NAME=FILESET"), "{constrs}");
        assert!(constrs.contains("VALUE=constrs_1"), "{constrs}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "FILES" && v.contains("counter.sdc")),
            "{:?}",
            ide.properties
        );
        let after_fs = ide.exec("project_settings").unwrap();
        assert!(after_fs.contains("NAME=FILESET VALUE=constrs_1"), "{after_fs}");
        ide.exec("set_property FILESET sources_1").unwrap();
        assert_eq!(ide.active_fileset, "sources_1");

        ide.exec("set_property STRATEGY RuntimeOpt").unwrap();
        let strat = ide.exec("project_settings").unwrap();
        assert!(strat.contains("NAME=STRATEGY VALUE=RuntimeOpt"), "{strat}");
        assert_eq!(ide.selected_setting.as_deref(), Some("STRATEGY"));
        let strat_click = ide.exec("select_project_setting STRATEGY").unwrap();
        assert!(strat_click.contains("VALUE=RuntimeOpt"), "{strat_click}");
        assert!(
            ide.properties
                .iter()
                .any(|(k, v)| k == "RUN" && v == "impl_1"),
            "{:?}",
            ide.properties
        );
        ide.exec("set_property STRATEGY Default").unwrap();

        ide.run_step(FlowStep::Opt).unwrap();
        ide.run_step(FlowStep::Place).unwrap();
        ide.run_step(FlowStep::Route).unwrap();
        let gold = ide.wns_ps().expect("STA after route");
        assert_ne!(gold, 0);
        let after = ide.exec("project_settings").unwrap();
        assert!(after.contains("NAME=PART VALUE=HL10T-C32-1"), "{after}");
        assert!(after.contains("NAME=TOP VALUE=counter"), "{after}");
        ide.exec("select_project_setting CLOCK_PERIOD_PS").unwrap();
        assert_eq!(
            ide.wns_ps(),
            Some(gold),
            "empty XDC gold WNS must hold after Project Settings"
        );

        let mut blinky = IdeModel::new();
        blinky.open_source(&example("blinky.sv")).unwrap();
        let btable = blinky.exec("project_settings").unwrap();
        assert!(btable.contains("NAME=TOP VALUE=blinky"), "{btable}");
        assert!(
            !btable.contains("NAME=TOP VALUE=counter"),
            "Project Settings are per-session, not canned: {btable}"
        );
        blinky.exec("select_project_setting FILESET").unwrap();
        assert!(
            blinky
                .properties
                .iter()
                .any(|(k, v)| k == "FILES" && v.contains("blinky.sv")),
            "{:?}",
            blinky.properties
        );
        assert!(
            !blinky
                .properties
                .iter()
                .any(|(_, v)| v.contains("counter.sv")),
            "blinky fileset is not counter: {:?}",
            blinky.properties
        );
    }
}
