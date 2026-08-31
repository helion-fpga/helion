//! Headless IDE model — the Vivado-class application state.
//!
//! Sources/netlist tree, Tcl console, flow rail (Synthesis → Opt → Place → Route →
//! Bitstream) and the timing/utilization report panes. Everything here runs the *real*
//! [`Session`] engines: there is no canned output anywhere in this file, so the widget
//! layer (`helion-ide`) is a thin painter over this model and can be tested without a
//! display.

use crate::{tcl_eval, GpuiShell};
use helion_device::Device;
use helion_ir::{CellKind, Design};
use helion_proj::{get_cells, get_nets, Mode, Session};
use helion_sta::{create_clock, report_timing_routed, TimingResult};
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
        let r = tcl_eval(&mut self.shell, cmd);
        let line = match &r {
            Ok(out) => ConsoleLine {
                cmd: cmd.to_string(),
                out: out.clone(),
                ok: true,
            },
            Err(e) => ConsoleLine {
                cmd: cmd.to_string(),
                out: e.clone(),
                ok: false,
            },
        };
        self.status = if line.ok {
            format!("{cmd}: ok")
        } else {
            format!("{cmd}: {}", line.out)
        };
        self.console.push(line);
        self.sync_from_session();
        r
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
        let line = match &r {
            Ok(out) => ConsoleLine {
                cmd: step.tcl().into(),
                out: out.clone(),
                ok: true,
            },
            Err(e) => ConsoleLine {
                cmd: step.tcl().into(),
                out: e.clone(),
                ok: false,
            },
        };
        self.status = format!("{} {}", step.label(), if line.ok { "ok" } else { "failed" });
        self.console.push(line);
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

    /// Rebuild every pane off the current Session state. Called after each command.
    pub fn sync_from_session(&mut self) {
        self.refresh_tree();
        self.refresh_reports();
        self.refresh_steps();
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
        let s = &self.shell.session;
        self.timing = match (s.design.as_ref(), s.routed.as_ref()) {
            (Some(d), Some(r)) => {
                let mut clks = Vec::new();
                create_clock(&mut clks, "clk", self.clock_period_ps, "clk");
                report_timing_routed(d, r, &clks).ok()
            }
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
}
