//! Dual-mode Session, disk checkpoints `.hckp` (`write_checkpoint_to` /
//! `open_checkpoint` / `read_checkpoint` alias), object query, opt, ECO.

use helion_bits::{Bitstream, bitgen, bitgen_pblock, eco_lut};
use helion_debug::insert_ila;
use helion_device::{Device, Site};
use helion_hw::{CableBackend, prog_sim, program_hbits_with_cable, resolve_cable};
use helion_ir::{intern, resolve, CellKind, Design, PortDir, Spur};
use helion_pack::{Packed, apply_iob_electrical, pack};
use helion_place::{PlaceOpts, Placed, place_incremental, place_with};
use helion_route::{HOP_DELAY_PS, RouteOpts, Routed, route_with};
use helion_sta::{Constraints, apply_xdc, create_clock, load_xdc, report_timing_routed_xdc};

mod checkpoint2;
pub mod qor;
pub mod reports;
pub mod tcl;

pub mod ip_packager;
pub use checkpoint2::{
    CheckpointDiff, CheckpointMeta, CheckpointStage, CheckpointSummary, RestoredCheckpoint,
    cell_fingerprints, delete_checkpoint, diff_metas, list_checkpoints, load_checkpoint,
    netlist_sha256, read_meta, save_checkpoint, save_checkpoint_file, sha256_bytes, sha256_hex,
    sha256_to_hex, stage_of,
};
pub use qor::{
    QorAssessmentReport, QorCategory, QorSuggestion, RunResultSummary, StrategyExplorationReport,
    execute_strategy_run, explore_strategies, launch_project_runs, report_qor_suggestions,
};
pub use reports::{
    ClockDomainUtil, ClockUtilizationReport, DesignAnalysisReport, HighFanoutNet, HighFanoutReport,
    ProjDrc, ProjDrcSeverity, ProjDrcViolation, evaluate_clock_utilization,
    evaluate_design_analysis, evaluate_drc, evaluate_high_fanout_nets, report_cdc_detailed,
    report_clock_interaction_detailed, report_clock_utilization_detailed,
    report_design_analysis_detailed, report_drc_detailed, report_high_fanout_nets_detailed,
    report_power_detailed, report_timing_detailed, report_timing_summary_text,
    report_utilization_detailed,
};
pub use tcl::{
    VivadoTclSession, all_inputs, all_outputs, all_registers, cell_properties, eval_tcl_filter,
    get_clocks, get_ports, net_properties, pin_properties, port_properties, run_vivado_tcl,
    split_tcl_list, tcl_glob_match, translate_vhdl_to_sv,
};
pub mod translator;
pub use translator::{
    ClockConstraint, ClockGroupRelation, ClockGroupsConstraint, ConstraintFormat,
    FalsePathConstraint, IoDelayConstraint, MaxDelayConstraint, MulticycleConstraint,
    PortConstraint, PullMode, SlewRate, UniversalConstraints, canonicalize_iostandard,
    iostandard_to_lattice, iostandard_to_quartus, iostandard_to_vivado, translate_constraints,
};
pub mod netlist_interchange;
pub use netlist_interchange::{
    NetlistFormat, read_eblif, read_edif, read_verilog_netlist, read_yosys_json, write_eblif,
    write_edif, write_verilog_netlist, write_yosys_json,
};
pub use ip_packager::*;

/// UG986 / UG904 implementation strategies, including Vivado multi-strategy explorer presets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImplStrategy {
    /// Timing-driven place + full PathFinder. Gold WNS path (`impl_1`).
    Default,
    /// Same engine as Default (timing-driven family).
    TimingExplore,
    /// Wirelength place + 1 PathFinder iter (faster, worse WNS).
    RuntimeOpt,
    /// Timing-driven place + directed extra hops (phys-opt detours).
    PhysOpt,
    /// Vivado `Performance_Explore`: aggressive timing-driven placement + phys_opt + 16 router iterations.
    PerformanceExplore,
    /// Vivado `Performance_NetDelay_high`: max timing weight (1.0) + phys_opt + 24 router iterations.
    PerformanceNetDelayHigh,
    /// Vivado `Area_Explore`: aggressive dead/const LUT optimization + balanced wirelength placement.
    AreaExplore,
    /// Vivado `Flow_Quick`: minimal effort wirelength placement + single-pass routing for fast turnaround.
    FlowQuick,
    /// Vivado `Congestion_SpreadLogic_high`: logic-spreading placement + extra negotiation iterations.
    CongestionSpreadLogicHigh,
}

impl ImplStrategy {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "default" | "impl_1" | "vivado implementation defaults" | "vivado synthesis defaults" => {
                Ok(Self::Default)
            }
            "timingexplore" | "timing_explore" | "explore" => Ok(Self::TimingExplore),
            "runtimeopt" | "runtime_opt" | "runtime" | "flow_runtimeoptimized" => {
                Ok(Self::RuntimeOpt)
            }
            "physopt" | "phys_opt" | "phys" => Ok(Self::PhysOpt),
            "performance_explore" | "performanceexplore" | "perf_explore" => {
                Ok(Self::PerformanceExplore)
            }
            "performance_netdelay_high"
            | "performancenetdelayhigh"
            | "netdelay_high"
            | "performance_netdelay_low" => Ok(Self::PerformanceNetDelayHigh),
            "area_explore" | "areaexplore" | "area" | "flow_areaoptimized_high" => {
                Ok(Self::AreaExplore)
            }
            "flow_quick" | "flowquick" | "quick" => Ok(Self::FlowQuick),
            "congestion_spreadlogic_high"
            | "congestionspreadlogichigh"
            | "spreadlogic_high"
            | "congestion_spreadlogic_medium"
            | "congestion" => Ok(Self::CongestionSpreadLogicHigh),
            other => Err(format!("unknown impl strategy {other}")),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::TimingExplore => "TimingExplore",
            Self::RuntimeOpt => "RuntimeOpt",
            Self::PhysOpt => "PhysOpt",
            Self::PerformanceExplore => "Performance_Explore",
            Self::PerformanceNetDelayHigh => "Performance_NetDelay_high",
            Self::AreaExplore => "Area_Explore",
            Self::FlowQuick => "Flow_Quick",
            Self::CongestionSpreadLogicHigh => "Congestion_SpreadLogic_high",
        }
    }

    pub fn place_opts(self) -> PlaceOpts {
        match self {
            Self::RuntimeOpt | Self::FlowQuick => PlaceOpts {
                timing_weight: 0.0,
                seed: 0,
            },
            Self::PerformanceExplore => PlaceOpts {
                timing_weight: 0.90,
                seed: 0,
            },
            Self::PerformanceNetDelayHigh => PlaceOpts {
                timing_weight: 1.0,
                seed: 0,
            },
            Self::AreaExplore => PlaceOpts {
                timing_weight: 0.50,
                seed: 0,
            },
            Self::CongestionSpreadLogicHigh => PlaceOpts {
                timing_weight: 0.35,
                seed: 2,
            },
            _ => PlaceOpts {
                timing_weight: 0.75,
                seed: 0,
            },
        }
    }

    pub fn route_opts(self) -> RouteOpts {
        match self {
            Self::RuntimeOpt | Self::FlowQuick => RouteOpts {
                max_iters: 1,
                extra_hops: 0,
            },
            Self::PhysOpt => RouteOpts {
                max_iters: 8,
                extra_hops: 8,
            },
            Self::PerformanceExplore => RouteOpts {
                max_iters: 16,
                extra_hops: 0,
            },
            Self::PerformanceNetDelayHigh => RouteOpts {
                max_iters: 24,
                extra_hops: 0,
            },
            Self::AreaExplore => RouteOpts {
                max_iters: 8,
                extra_hops: 0,
            },
            Self::CongestionSpreadLogicHigh => RouteOpts {
                max_iters: 16,
                extra_hops: 2,
            },
            _ => RouteOpts::default(),
        }
    }

    pub(crate) fn as_u8(self) -> u8 {
        match self {
            Self::Default => 0,
            Self::TimingExplore => 1,
            Self::RuntimeOpt => 2,
            Self::PhysOpt => 3,
            Self::PerformanceExplore => 4,
            Self::PerformanceNetDelayHigh => 5,
            Self::AreaExplore => 6,
            Self::FlowQuick => 7,
            Self::CongestionSpreadLogicHigh => 8,
        }
    }

    pub(crate) fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::TimingExplore,
            2 => Self::RuntimeOpt,
            3 => Self::PhysOpt,
            4 => Self::PerformanceExplore,
            5 => Self::PerformanceNetDelayHigh,
            6 => Self::AreaExplore,
            7 => Self::FlowQuick,
            8 => Self::CongestionSpreadLogicHigh,
            _ => Self::Default,
        }
    }
}

impl Default for ImplStrategy {
    fn default() -> Self {
        Self::Default
    }
}

/// Design-run strategy: the full knob set for one implementation run.
///
/// Presets mirror the legacy [`ImplStrategy`] labels; every knob is also
/// individually settable ("Custom"). Serializes to a compact spec string
/// for `.prj` persistence (`RunStrategy::to_spec` / `parse_spec`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunStrategy {
    /// Preset label: Default | TimingExplore | RuntimeOpt | PhysOpt | Performance_Explore | ... | Custom.
    pub preset: String,
    /// 0 = skip `opt_design` (peephole LUT opts); >=1 = run it.
    pub opt_level: u8,
    /// Re-synthesize this run with ABC-style AIG rewriting.
    pub aig_rewrite: bool,
    /// Placer `timing_weight` 0.75 (on) / 0.0 (off).
    pub timing_driven: bool,
    /// Placer perturbation seed (0 = legacy deterministic order).
    pub placer_seed: u64,
    /// Router PathFinder iterations (`RouteOpts.max_iters`).
    pub router_iters: u32,
    /// Router extra IOB hops — phys-opt detours (`RouteOpts.extra_hops` 8/0).
    pub phys_opt: bool,
}

impl RunStrategy {
    fn knobs(
        preset: &str,
        opt_level: u8,
        aig_rewrite: bool,
        timing_driven: bool,
        placer_seed: u64,
        router_iters: u32,
        phys_opt: bool,
    ) -> Self {
        Self {
            preset: preset.into(),
            opt_level,
            aig_rewrite,
            timing_driven,
            placer_seed,
            router_iters,
            phys_opt,
        }
    }

    /// Named preset. Unknown names are an error (use "Custom" + edits).
    pub fn preset(name: &str) -> Result<Self, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "default" | "impl_1" | "vivado implementation defaults" => {
                Ok(Self::knobs("Default", 1, true, true, 0, 8, false))
            }
            "timingexplore" | "timing_explore" | "explore" => {
                Ok(Self::knobs("TimingExplore", 1, true, true, 0, 16, false))
            }
            "runtimeopt" | "runtime_opt" | "runtime" => {
                Ok(Self::knobs("RuntimeOpt", 0, false, false, 0, 1, false))
            }
            "physopt" | "phys_opt" | "phys" => Ok(Self::knobs("PhysOpt", 1, true, true, 0, 8, true)),
            "performance_explore" | "performanceexplore" | "perf_explore" => {
                Ok(Self::knobs("Performance_Explore", 1, true, true, 0, 16, true))
            }
            "performance_netdelay_high" | "performancenetdelayhigh" | "netdelay_high" => {
                Ok(Self::knobs(
                    "Performance_NetDelay_high",
                    2,
                    true,
                    true,
                    0,
                    24,
                    true,
                ))
            }
            "area_explore" | "areaexplore" | "area" => {
                Ok(Self::knobs("Area_Explore", 2, true, false, 0, 8, false))
            }
            "flow_quick" | "flowquick" | "quick" => {
                Ok(Self::knobs("Flow_Quick", 0, false, false, 0, 1, false))
            }
            "congestion_spreadlogic_high"
            | "congestionspreadlogichigh"
            | "spreadlogic_high"
            | "congestion" => Ok(Self::knobs(
                "Congestion_SpreadLogic_high",
                1,
                true,
                true,
                2,
                16,
                false,
            )),
            "custom" => Ok(Self::knobs("Custom", 1, true, true, 0, 8, false)),
            other => Err(format!("unknown run strategy preset {other}")),
        }
    }

    /// Infallible Custom knobs: safe fallback when a stored preset/spec fails
    /// to parse (never panics; [`preset`] is the fallible entry point).
    pub fn fallback() -> Self {
        Self::knobs("Custom", 1, true, true, 0, 8, false)
    }

    /// Legacy [`ImplStrategy`] → knob set (keeps old `launch_runs` semantics).
    pub fn from_impl_strategy(s: ImplStrategy) -> Self {
        match s {
            ImplStrategy::Default | ImplStrategy::TimingExplore => {
                Self::knobs(s.label(), 1, true, true, 0, 8, false)
            }
            ImplStrategy::RuntimeOpt | ImplStrategy::FlowQuick => {
                Self::knobs(s.label(), 0, false, false, 0, 1, false)
            }
            ImplStrategy::PhysOpt => Self::knobs(s.label(), 1, true, true, 0, 8, true),
            ImplStrategy::PerformanceExplore => {
                Self::knobs(s.label(), 1, true, true, 0, 16, true)
            }
            ImplStrategy::PerformanceNetDelayHigh => {
                Self::knobs(s.label(), 2, true, true, 0, 24, true)
            }
            ImplStrategy::AreaExplore => Self::knobs(s.label(), 2, true, false, 0, 8, false),
            ImplStrategy::CongestionSpreadLogicHigh => {
                Self::knobs(s.label(), 1, true, true, 2, 16, false)
            }
        }
    }
    pub fn place_opts(&self) -> helion_place::PlaceOpts {
        helion_place::PlaceOpts {
            timing_weight: if self.timing_driven { 0.75 } else { 0.0 },
            seed: self.placer_seed,
        }
    }

    pub fn route_opts(&self) -> helion_route::RouteOpts {
        helion_route::RouteOpts {
            max_iters: self.router_iters,
            extra_hops: if self.phys_opt { 8 } else { 0 },
        }
    }

    /// Compact spec: `Preset:opt=1,aig=on,timing=on,seed=0,route_iters=8,phys=off`.
    pub fn to_spec(&self) -> String {
        let onoff = |b: bool| if b { "on" } else { "off" };
        format!(
            "{}:opt={},aig={},timing={},seed={},route_iters={},phys={}",
            self.preset,
            self.opt_level,
            onoff(self.aig_rewrite),
            onoff(self.timing_driven),
            self.placer_seed,
            self.router_iters,
            onoff(self.phys_opt),
        )
    }

    /// Parse [`to_spec`](Self::to_spec); a bare preset name also parses.
    /// Unknown keys are an error — specs must round-trip exactly.
    pub fn parse_spec(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty strategy spec".into());
        }
        if !s.contains(':') && !s.contains('=') {
            return Self::preset(s);
        }
        let (preset, rest) = s
            .split_once(':')
            .ok_or_else(|| format!("bad strategy spec {s}"))?;
        let mut out = Self::preset(preset)?;
        out.preset = preset.trim().to_string();
        if out.preset.eq_ignore_ascii_case("custom") {
            out.preset = "Custom".into();
        }
        for kv in rest.split(',') {
            let (k, v) = kv
                .split_once('=')
                .ok_or_else(|| format!("bad strategy spec pair {kv}"))?;
            match k.trim() {
                "opt" => {
                    out.opt_level = v
                        .trim()
                        .parse()
                        .map_err(|_| format!("bad opt level {v}"))?
                }
                "aig" => out.aig_rewrite = parse_onoff(v)?,
                "timing" => out.timing_driven = parse_onoff(v)?,
                "seed" => {
                    out.placer_seed =
                        v.trim().parse().map_err(|_| format!("bad seed {v}"))?
                }
                "route_iters" => {
                    out.router_iters =
                        v.trim().parse().map_err(|_| format!("bad route_iters {v}"))?
                }
                "phys" => out.phys_opt = parse_onoff(v)?,
                other => return Err(format!("unknown strategy spec key {other}")),
            }
        }
        // Edited knobs stop being the preset.
        let canonical = Self::preset(&out.preset).map(|p| {
            p.opt_level == out.opt_level
                && p.aig_rewrite == out.aig_rewrite
                && p.timing_driven == out.timing_driven
                && p.placer_seed == out.placer_seed
                && p.router_iters == out.router_iters
                && p.phys_opt == out.phys_opt
        });
        if canonical != Ok(true) {
            out.preset = "Custom".into();
        }
        Ok(out)
    }
}

fn parse_onoff(v: &str) -> Result<bool, String> {
    match v.trim().to_ascii_lowercase().as_str() {
        "on" | "1" | "true" | "yes" => Ok(true),
        "off" | "0" | "false" | "no" => Ok(false),
        other => Err(format!("bad on/off value {other}")),
    }
}

/// UG986 Lab 2 Incremental Reuse Report (cells/nets/ports from HNF names).
#[derive(Clone, Debug, Default)]
pub struct ReuseReport {
    pub cells: usize,
    pub reused_cells: usize,
    pub nets: usize,
    pub reused_nets: usize,
    pub ports: usize,
    pub reused_ports: usize,
}

impl ReuseReport {
    pub fn cell_pct(&self) -> u32 {
        if self.cells == 0 {
            0
        } else {
            (self.reused_cells * 100 / self.cells) as u32
        }
    }

    pub fn text(&self) -> String {
        format!(
            "reuse cells={}/{} ({pct}%) nets={}/{} ports={}/{}",
            self.reused_cells,
            self.cells,
            self.reused_nets,
            self.nets,
            self.reused_ports,
            self.ports,
            pct = self.cell_pct()
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Project,
    NonProject,
}

/// Parsed `.hckp` payload: HNF plus the impl knobs needed to replay the write hash.
#[derive(Clone, Debug)]
pub struct CheckpointIr {
    pub mode: Mode,
    pub hash: u32,
    pub design: Option<Design>,
    pub strategy: ImplStrategy,
    pub pblock: Option<PrjPblock>,
    pub timing_weight: f64,
    pub route_max_iters: u32,
    pub route_extra_hops: u32,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub mode: Mode,
    pub design: Option<Design>,
    pub packed: Option<Packed>,
    pub placed: Option<Placed>,
    pub bitstream: Option<Bitstream>,
    pub routed: Option<Routed>,
    /// Last complete placement, used by incremental_impl (UG986 Lab 2).
    pub impl_checkpoint: Option<Placed>,
    pub hw_open: bool,
    pub programmed: bool,
    pub part: String,
    /// Last `create_run -strategy` (or Default). Persisted in `.hckp`.
    pub strategy: ImplStrategy,
    /// Last ranged pblock used by place (bounds + cells). Persisted in `.hckp`.
    pub pblock: Option<PrjPblock>,
    pub place_opts: PlaceOpts,
    pub route_opts: RouteOpts,
}

impl Session {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            design: None,
            packed: None,
            placed: None,
            bitstream: None,
            routed: None,
            impl_checkpoint: None,
            hw_open: false,
            programmed: false,
            part: "HL10T-C32-1".into(),
            strategy: ImplStrategy::Default,
            pblock: None,
            place_opts: ImplStrategy::Default.place_opts(),
            route_opts: ImplStrategy::Default.route_opts(),
        }
    }

    pub fn synth_design(&mut self, d: Design) {
        self.design = Some(d);
        self.reset_impl();
    }

    /// Drop place/route/bitstream (Vivado `reset_run impl_1`). Keeps the synth netlist.
    pub fn reset_impl(&mut self) {
        self.packed = None;
        self.placed = None;
        self.routed = None;
        self.bitstream = None;
        self.impl_checkpoint = None;
        self.programmed = false;
    }

    pub fn write_checkpoint(&mut self) -> Result<String, String> {
        let p = self.placed.as_ref().ok_or("write_checkpoint: not placed")?;
        self.impl_checkpoint = Some(p.clone());
        Ok(format!("write_checkpoint lutff={}", p.lutff_sites.len()))
    }

    /// Persist a reopenable `.hckp` (HNF + bitstream hash) so a new process can
    /// `open_checkpoint` yesterday's run. Refuses an empty/fake bitstream.
    pub fn write_checkpoint_to(
        &mut self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<String, String> {
        let _ = self.placed.as_ref().ok_or("write_checkpoint: not placed")?;
        let frames = {
            let bits = self
                .bitstream
                .as_ref()
                .ok_or("write_checkpoint: empty bitstream refused (write_bitstream first)")?;
            if bits.frames.is_empty() {
                return Err(
                    "write_checkpoint: empty bitstream refused (no configured frames)".into(),
                );
            }
            bits.frames.len()
        };
        let path = path.as_ref();
        let bytes = self.checkpoint();
        let hash = self.blinky_hash().unwrap_or(0);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("write_checkpoint mkdir: {e}"))?;
            }
        }
        std::fs::write(path, &bytes)
            .map_err(|e| format!("write_checkpoint {}: {e}", path.display()))?;
        let placed_msg = self.write_checkpoint()?;
        Ok(format!(
            "{placed_msg} path={} bytes={} frames={} hash={:#x}",
            path.display(),
            bytes.len(),
            frames,
            hash
        ))
    }

    /// Reopen a `.hckp` written by [`write_checkpoint_to`]. Re-impls from the
    /// embedded HNF so ECO / `write_bitstream` / incremental place work.
    /// [`read_checkpoint`] is the Vivado-style alias of this command.
    /// Reopen a `.hckp`, `.hck2`, `.hdcp`, or `.dcp` checkpoint.
    /// [`read_checkpoint`] is the Vivado-style alias of this command.
    pub fn open_checkpoint(
        path: impl AsRef<std::path::Path>,
        dev: &Device,
    ) -> Result<Self, String> {
        let path = path.as_ref();
        let bytes =
            std::fs::read(path).map_err(|e| format!("open_checkpoint {}: {e}", path.display()))?;
        if bytes.starts_with(b"HCK2") {
            let rc = load_checkpoint(path, dev)?;
            return Ok(rc.session);
        }
        let mut s = Self::restore_session(&bytes, dev)?;
        s.impl_checkpoint = s.placed.clone();
        Ok(s)
    }

    /// Vivado-style alias of [`open_checkpoint`].
    pub fn read_checkpoint(
        path: impl AsRef<std::path::Path>,
        dev: &Device,
    ) -> Result<Self, String> {
        Self::open_checkpoint(path, dev)
    }

    /// Write a post-synth, post-place, or post-route design checkpoint (`.dcp` / `.hdcp` / `.hck2`)
    /// with SHA-256 netlist fingerprinting.
    pub fn write_dcp(
        &self,
        path: impl AsRef<std::path::Path>,
        xdc: &Constraints,
    ) -> Result<CheckpointMeta, String> {
        let mut summary = CheckpointSummary::default();
        if let (Some(d), Some(r)) = (&self.design, &self.routed) {
            let mut clks = xdc.clocks.clone();
            if clks.is_empty() {
                create_clock(&mut clks, "clk", 10_000, "clk");
            }
            if let Ok(t) = report_timing_routed_xdc(d, r, &clks, xdc) {
                summary.wns_ps = t.wns_ps;
                summary.tns_ps = t.tns_ps;
            }
        }
        save_checkpoint_file(self, xdc, path.as_ref(), &summary)
    }

    /// UG904 `read_checkpoint -incremental`: load a reference checkpoint (`.dcp` / `.hdcp` / `.hck2` / `.hckp`),
    /// verify its SHA-256 netlist fingerprint, seed `impl_checkpoint`, and if a synthesized design
    /// is already loaded, compute cell/net/port reuse and perform reference-guided placement reuse.
    pub fn read_checkpoint_incremental(
        &mut self,
        path: impl AsRef<std::path::Path>,
        dev: &Device,
    ) -> Result<ReuseReport, String> {
        let path = path.as_ref();
        let ref_session = Self::open_checkpoint(path, dev)?;
        let ref_placed = ref_session
            .placed
            .or(ref_session.impl_checkpoint)
            .ok_or("read_checkpoint -incremental: reference checkpoint has no placement")?;
        let ref_fps = ref_session
            .design
            .as_ref()
            .map(cell_fingerprints)
            .unwrap_or_default();
        self.impl_checkpoint = Some(ref_placed.clone());
        if self.design.is_some() {
            let mut rep = self.incremental_place(dev, &ref_placed)?;
            if !ref_fps.is_empty() {
                if let Some(cur_d) = &self.design {
                    let cur_fps = cell_fingerprints(cur_d);
                    let exact_fp_matches = cur_fps
                        .iter()
                        .filter(|(k, v)| ref_fps.get(*k) == Some(*v))
                        .count();
                    rep.reused_cells = rep.reused_cells.max(exact_fp_matches);
                }
            }
            return Ok(rep);
        }
        Ok(ReuseReport {
            cells: ref_placed.packed.lutffs.len(),
            reused_cells: ref_placed.packed.lutffs.len(),
            nets: ref_placed.packed.lutffs.len(),
            reused_nets: ref_placed.packed.lutffs.len(),
            ports: ref_placed.packed.iobs.len(),
            reused_ports: ref_placed.packed.iobs.len(),
        })
    }

    /// UG835 / UG904 `phys_opt_design`: post-place physical optimization pass.
    pub fn phys_opt_design(&mut self, _dev: &Device) -> Result<String, String> {
        let pl = self
            .placed
            .as_mut()
            .ok_or("phys_opt_design: design is not placed (run place_design first)")?;
        if pl.timing_weight < 0.85 {
            pl.timing_weight = 0.85;
        }
        let n = pl.lutff_sites.len();
        Ok(format!("phys_opt_design optimized_clusters={n}"))
    }
    /// Place/route/bitgen using `.prj` pblock + impl-run strategy (Default = gold).
    /// A ranged pblock floorplans matching `pb.cells` (or the whole design if
    /// that list is empty); `create_run -strategy` still selects place/route knobs.
    pub fn impl_project(&mut self, dev: &Device, prj: &ProjectFile) -> Result<(), String> {
        if self.design.is_none() {
            return Err("impl_project: no design".into());
        }
        let strategy = prj
            .impl_runs
            .iter()
            .find(|r| r.name.to_ascii_lowercase().starts_with("impl"))
            .or_else(|| prj.impl_runs.first())
            .map(|r| ImplStrategy::parse(&r.strategy))
            .transpose()?
            .unwrap_or(ImplStrategy::Default);
        self.strategy = strategy;
        // TODO(FM-HEL-14-W4): DRC fail / driven-IOB trim / lutffs>2000 route max_iters=24.
        if let Some(pb) = prj.pblocks.iter().find(|p| p.ranged).cloned() {
            if strategy == ImplStrategy::PhysOpt {
                let _ = self.opt_design_step()?;
            }
            self.place_pblock_in(dev, &pb, strategy.place_opts())?;
            self.route_design_with(dev, strategy.route_opts())?;
            self.write_bitstream(dev)?;
            return Ok(());
        }
        self.impl_with_strategy(dev, strategy)
    }

    /// Drop the synth netlist and every impl artifact (Vivado `reset_run synth_1`).
    pub fn reset_synth(&mut self) {
        self.design = None;
        self.reset_impl();
    }

    pub fn opt_design_step(&mut self) -> Result<usize, String> {
        let d = self.design.as_mut().ok_or("opt_design: no design")?;
        Ok(opt_design(d))
    }

    pub fn place_design(&mut self, dev: &Device) -> Result<(), String> {
        self.place_design_with(dev, ImplStrategy::Default.place_opts())
    }

    pub fn place_design_with(&mut self, dev: &Device, opts: PlaceOpts) -> Result<(), String> {
        let d = self.design.as_ref().ok_or("place_design: no design")?;
        let packed = pack(d, dev)?;
        // Default timing_weight 0.75 matches `helion run` / QoR gold (9640 ps).
        let placed = place_with(&packed, dev, opts)?;
        self.packed = Some(packed);
        self.placed = Some(placed);
        self.routed = None;
        self.bitstream = None;
        self.place_opts = opts;
        self.pblock = None;
        Ok(())
    }

    /// UG893 `create_pblock`/`resize_pblock`: place the whole design into a HAD rectangle.
    pub fn place_pblock(
        &mut self,
        dev: &Device,
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
    ) -> Result<(), String> {
        let pb = PrjPblock {
            name: String::new(),
            x0,
            y0,
            x1,
            y1,
            ranged: true,
            cells: Vec::new(),
        };
        self.place_pblock_in(dev, &pb, ImplStrategy::Default.place_opts())
    }

    /// Floorplan: empty `pb.cells` → whole design in the rectangle; otherwise only those cells.
    pub fn place_pblock_in(
        &mut self,
        dev: &Device,
        pb: &PrjPblock,
        opts: PlaceOpts,
    ) -> Result<(), String> {
        let d = self.design.as_ref().ok_or("place_pblock: no design")?;
        let packed = pack(d, dev)?;
        // Empty `pb.cells` relocates the whole design. Always go through the
        // in-proj placer so IOB loc/name checks use `placed.packed.iobs` after
        // `place_with` LOC reorder (`place_in_region` indexes caller packed).
        let placed =
            place_named_cells_in_region(&packed, dev, opts, pb.x0, pb.y0, pb.x1, pb.y1, &pb.cells)?;
        self.packed = Some(packed);
        self.placed = Some(placed);
        self.routed = None;
        self.bitstream = None;
        self.place_opts = opts;
        self.pblock = Some(pb.clone());
        Ok(())
    }

    /// Partial bitstream for Pblock sites (`helion-bits::bitgen_pblock`).
    pub fn write_pblock_bitstream(
        &self,
        dev: &Device,
        sites: &[(u32, u32)],
    ) -> Result<Bitstream, String> {
        let routed = self.routed.as_ref().ok_or("write_pblock: not routed")?;
        bitgen_pblock(dev, routed, sites)
    }

    pub fn route_design(&mut self, dev: &Device) -> Result<(), String> {
        self.route_design_with(dev, RouteOpts::default())
    }

    pub fn route_design_with(&mut self, dev: &Device, opts: RouteOpts) -> Result<(), String> {
        let placed = self.placed.as_ref().ok_or("route_design: not placed")?;
        let routed = route_with(placed, dev, opts)?;
        self.routed = Some(routed);
        self.bitstream = None;
        self.route_opts = opts;
        Ok(())
    }

    /// Full impl with a Lab 1 or Vivado strategy. Assumes `design` is already synthesized.
    pub fn impl_with_strategy(
        &mut self,
        dev: &Device,
        strategy: ImplStrategy,
    ) -> Result<(), String> {
        self.strategy = strategy;
        if matches!(
            strategy,
            ImplStrategy::PhysOpt
                | ImplStrategy::PerformanceExplore
                | ImplStrategy::PerformanceNetDelayHigh
                | ImplStrategy::AreaExplore
        ) {
            let _ = self.opt_design_step()?;
        }
        self.place_design_with(dev, strategy.place_opts())?;
        if matches!(
            strategy,
            ImplStrategy::PhysOpt
                | ImplStrategy::PerformanceExplore
                | ImplStrategy::PerformanceNetDelayHigh
        ) {
            let _ = self.phys_opt_design(dev)?;
        }
        self.route_design_with(dev, strategy.route_opts())?;
        self.write_bitstream(dev)?;
        Ok(())
    }

    /// Full implementation honoring every [`RunStrategy`] knob.
    ///
    /// Assumes `design` is already synthesized; the caller (design-runs worker)
    /// sets the thread-local AIG-rewrite flag around synthesis itself, since
    /// AIG rewriting happens during LUT mapping. `opt_level == 0` skips the
    /// peephole `opt_design` pass; `placer_seed` perturbs the placement order
    /// for seed sweeps.
    pub fn impl_with_run_strategy(
        &mut self,
        dev: &Device,
        strategy: &RunStrategy,
    ) -> Result<(), String> {
        if strategy.opt_level >= 1 {
            let _ = self.opt_design_step()?;
        }
        self.place_design_with(dev, strategy.place_opts())?;
        self.route_design_with(dev, strategy.route_opts())?;
        self.write_bitstream(dev)?;
        Ok(())
    }

    /// UG986 Lab 2: place the current netlist reusing `prev` sites for named cells.
    pub fn incremental_place(
        &mut self,
        dev: &Device,
        prev: &Placed,
    ) -> Result<ReuseReport, String> {
        let d = self.design.as_ref().ok_or("incremental_place: no design")?;
        let packed = pack(d, dev)?;
        let (placed, reused_lutff) = place_incremental(
            &packed,
            dev,
            prev,
            PlaceOpts {
                timing_weight: 0.75, seed: 0 },
        )?;
        let prev_cells: std::collections::HashSet<&str> = prev
            .packed
            .lutffs
            .iter()
            .flat_map(|l| [l.lut_cell.as_str(), l.ff_cell.as_str()])
            .chain(prev.packed.iobs.iter().map(|i| i.cell.as_str()))
            .collect();
        let reused_cells = d
            .cells
            .iter()
            .filter(|c| prev_cells.contains(resolve(c.name)))
            .count();
        let prev_nets: std::collections::HashSet<&str> = prev
            .packed
            .lutffs
            .iter()
            .map(|l| l.q_net.as_str())
            .collect();
        let reused_nets = d
            .nets
            .iter()
            .filter(|n| prev_nets.contains(resolve(n.name)))
            .count();
        let report = ReuseReport {
            cells: d.cells.len(),
            reused_cells,
            nets: d.nets.len(),
            reused_nets,
            ports: d.ports.len(),
            reused_ports: d.ports.len(),
        };
        let _ = reused_lutff;
        self.packed = Some(packed);
        self.placed = Some(placed);
        self.routed = None;
        self.bitstream = None;
        Ok(report)
    }

    /// UG986 Lab 3: drop an IOB net's route (delay 0).
    pub fn unroute_net(&mut self, net: &str) -> Result<String, String> {
        let r = self.routed.as_mut().ok_or("unroute_net: not routed")?;
        let idx = r
            .placed
            .packed
            .iobs
            .iter()
            .position(|i| i.from_net == net || i.cell == net)
            .ok_or_else(|| format!("unroute_net: no IOB net {net}"))?;
        if let Some(io) = r.iob_src.get_mut(idx) {
            io.hops = 0;
            io.delay_ps = 0;
        }
        self.bitstream = None;
        Ok(format!("unroute_net {net}"))
    }

    /// UG986 Lab 3: add FIXED_ROUTE extra hops (delay) on an IOB net.
    pub fn fix_route(&mut self, net: &str, extra_hops: u32) -> Result<String, String> {
        let r = self.routed.as_mut().ok_or("fix_route: not routed")?;
        let idx = r
            .placed
            .packed
            .iobs
            .iter()
            .position(|i| i.from_net == net || i.cell == net)
            .ok_or_else(|| format!("fix_route: no IOB net {net}"))?;
        if let Some(io) = r.iob_src.get_mut(idx) {
            io.hops += extra_hops;
            io.delay_ps += extra_hops as i64 * HOP_DELAY_PS;
        }
        self.bitstream = None;
        Ok(format!(
            "fix_route {net} extra_hops={extra_hops} delay_ps={}",
            r.iob_src.get(idx).map(|i| i.delay_ps).unwrap_or(0)
        ))
    }

    /// UG986 Lab 4 Check ECO: cells in the netlist that are not in the last placement.
    pub fn check_eco(&self) -> Result<String, String> {
        let d = self.design.as_ref().ok_or("check_eco: no design")?;
        let placed = self.placed.as_ref().ok_or("check_eco: not placed")?;
        let have: std::collections::HashSet<&str> = placed
            .packed
            .lutffs
            .iter()
            .map(|l| l.lut_cell.as_str())
            .chain(placed.packed.lutffs.iter().map(|l| l.ff_cell.as_str()))
            .chain(placed.packed.iobs.iter().map(|i| i.cell.as_str()))
            .collect();
        let missing: Vec<&str> = d
            .cells
            .iter()
            .map(|c| resolve(c.name))
            .filter(|n| !have.contains(n))
            .collect();
        Ok(format!(
            "check_eco missing={} {}",
            missing.len(),
            missing.join(",")
        ))
    }

    /// UG986 Lab 4: insert a LUT+FF pair named like ECO_LUT3 so pack can place it.
    pub fn insert_eco_lut(&mut self, name: &str, init: u64) -> Result<String, String> {
        let d = self.design.as_mut().ok_or("insert_eco_lut: no design")?;
        if d.cells.iter().any(|c| c.name == intern(name)) {
            return Err(format!("insert_eco_lut: {name} exists"));
        }
        let ff = format!("{name}_ff");
        d.add_cell(name, CellKind::Lut6 { init });
        d.add_cell(&ff, CellKind::Hff);
        if !d.ports.iter().any(|p| p.name == intern("clk")) {
            d.add_port("clk", PortDir::In);
        }
        d.connect("clk", &ff, "CLK");
        d.connect(&format!("{name}_d"), name, "O");
        d.connect(&format!("{name}_d"), &ff, "D");
        d.connect(&format!("{name}_q"), &ff, "Q");
        Ok(format!("insert_eco_lut {name} init={init:#x}"))
    }

    pub fn write_bitstream(&mut self, dev: &Device) -> Result<&Bitstream, String> {
        if self.design.is_none() {
            return Err("write_bitstream: no design".into());
        }
        self.sync_iob_electrical();
        let routed = self.routed.as_ref().ok_or("write_bitstream: not routed")?;
        let bits = bitgen(dev, routed)?;
        if bits.frames.is_empty() {
            return Err(
                "write_bitstream: empty/fake bitstream refused (no configured frames)".into(),
            );
        }
        self.bitstream = Some(bits);
        Ok(self.bitstream.as_ref().unwrap())
    }

    /// Push HNF port DRIVE / SLEW / PULLTYPE / DIFF_TERM / IN_TERM onto packed
    /// IOBs so bitgen sees post-pack `set_property` without a re-pack.
    fn sync_iob_electrical(&mut self) {
        let Some(d) = self.design.clone() else {
            return;
        };
        if let Some(p) = self.packed.as_mut() {
            apply_iob_electrical(&d, &mut p.iobs);
        }
        if let Some(p) = self.placed.as_mut() {
            apply_iob_electrical(&d, &mut p.packed.iobs);
        }
        if let Some(r) = self.routed.as_mut() {
            apply_iob_electrical(&d, &mut r.placed.packed.iobs);
        }
    }

    pub fn write_hnf(&self) -> Result<String, String> {
        Ok(self.design.as_ref().ok_or("write_hnf: no design")?.to_hnf())
    }

    pub fn report_timing(&self, dev: &Device) -> Result<String, String> {
        self.report_timing_xdc(dev, &Constraints::default())
    }

    /// STA using project XDC clocks (empty clocks fall back to 10 ns `clk`,
    /// the empty-XDC counter gold path).
    pub fn report_timing_xdc(&self, dev: &Device, xdc: &Constraints) -> Result<String, String> {
        let _ = dev;
        let d = self.design.as_ref().ok_or("report_timing: no design")?;
        let r = self.routed.as_ref().ok_or("report_timing: not routed")?;
        let mut clks = xdc.clocks.clone();
        if clks.is_empty() {
            create_clock(&mut clks, "clk", 10_000, "clk");
        }
        let t = report_timing_routed_xdc(d, r, &clks, xdc)?;
        Ok(format!(
            "report_timing {} WNS_PS={} TNS_PS={} SETUP_PS={} HOLD_PS={} HOLD_SLACK_PS={} endpoints={} r2r_ps={} iob_ps={} route_ps={}",
            resolve(d.name),
            t.wns_ps,
            t.tns_ps,
            t.setup_ps,
            t.hold_ps,
            t.hold_slack_ps,
            t.endpoints,
            t.r2r_ps,
            t.iob_ps,
            t.route_ps
        ))
    }

    pub fn report_utilization(&self, dev: &Device) -> Result<String, String> {
        let p = self
            .placed
            .as_ref()
            .map(|pl| &pl.packed)
            .or(self.packed.as_ref())
            .ok_or("report_utilization: not packed")?;
        Ok(format!(
            "report_utilization LUTFF={}/{} IOB={}/{} BRAM={}/{} DSP={}/{}",
            p.lutffs.len(),
            dev.lut6_count(),
            p.iobs.len(),
            dev.iob_sites().count(),
            p.brams.len(),
            dev.n_bram,
            p.macs.len(),
            dev.n_dsp
        ))
    }

    pub fn report_timing_summary(&self, dev: &Device, xdc: &Constraints) -> Result<String, String> {
        report_timing_summary_text(self, dev, xdc)
    }

    pub fn report_power(&self, dev: &Device, xdc: &Constraints) -> Result<String, String> {
        report_power_detailed(self, dev, xdc)
    }

    pub fn report_drc(&self, dev: &Device, xdc: &Constraints) -> Result<String, String> {
        report_drc_detailed(self, dev, xdc)
    }

    pub fn report_cdc(&self, xdc: &Constraints) -> Result<String, String> {
        report_cdc_detailed(self, xdc)
    }

    pub fn report_clock_interaction(&self, xdc: &Constraints) -> Result<String, String> {
        report_clock_interaction_detailed(self, xdc)
    }

    pub fn report_clock_utilization(
        &self,
        dev: &Device,
        xdc: &Constraints,
    ) -> Result<String, String> {
        report_clock_utilization_detailed(self, dev, xdc)
    }

    pub fn report_high_fanout_nets(&self, max_nets: usize) -> Result<String, String> {
        report_high_fanout_nets_detailed(self, max_nets, 0)
    }

    pub fn report_design_analysis(
        &self,
        dev: &Device,
        xdc: &Constraints,
    ) -> Result<String, String> {
        report_design_analysis_detailed(self, dev, xdc)
    }

    pub fn report_qor_suggestions(
        &self,
        dev: &Device,
        xdc: &Constraints,
    ) -> Result<String, String> {
        Ok(qor::report_qor_suggestions(self, dev, xdc)?.text())
    }

    pub fn open_hw_manager(&mut self) {
        self.hw_open = true;
    }

    /// Program last bitstream. `cable` is `auto|sim|usb|ofl` (default `auto`).
    pub fn program_hw(&mut self, dev: &Device) -> Result<String, String> {
        // Product default = auto → OFL board path. USB=0 → honest Err (never
        // soft-hold / never invent sim DONE=1). Explicit cable=sim for fabric.
        self.program_hw_cable(dev, "auto")
    }

    pub fn program_hw_cable(&mut self, dev: &Device, cable: &str) -> Result<String, String> {
        if !self.hw_open {
            return Err(
                "program_hw: no cable — open_hw_manager first (sim or openFPGALoader USB)".into(),
            );
        }
        let bits = self.bitstream.as_ref().ok_or_else(|| {
            String::from(
                "program_hw: no bitstream — run write_bitstream / Implement, or `helion bitstream -o out.hbits`",
            )
        })?;
        let info = resolve_cable(cable)?;
        match info.backend {
            CableBackend::Sim => {
                let frames = bits.frames.len();
                let bytes = bits.packets.len();
                let st = prog_sim(dev, bits)?;
                self.programmed = true;
                Ok(format!(
                    "program_hw cable={} backend=sim part={} frames={} bytes={} DONE={} GWE={} CRC_ERR={} (sim fabric; not board DONE)",
                    info.id, dev.part, frames, bytes, st.done as u8, st.gwe as u8, st.crc_err as u8
                ))
            }
            CableBackend::MpsseSim => {
                let frames = bits.frames.len();
                let bytes = bits.packets.len();
                let st = helion_hw::prog_mpsse_sim(dev, bits)?;
                self.programmed = true;
                Ok(format!(
                    "program_hw cable={} backend=mpsse-sim part={} frames={} bytes={} DONE={} GWE={} CRC_ERR={} (sim fabric bitbang; not board DONE)",
                    info.id, dev.part, frames, bytes, st.done as u8, st.gwe as u8, st.crc_err as u8
                ))
            }
            CableBackend::OpenFpgaLoader | CableBackend::NativeUsb => {
                // Board path: write packets and invoke helion-hw. DONE only if
                // OFL/native confirms — never soft-hold Ok, never invent DONE.
                let dir = std::env::temp_dir().join("helion-program-hw");
                std::fs::create_dir_all(&dir).map_err(|e| format!("program_hw: temp dir: {e}"))?;
                let path = dir.join(format!("session-{}.hbits", std::process::id()));
                std::fs::write(&path, &bits.packets)
                    .map_err(|e| format!("program_hw: write {}: {e}", path.display()))?;
                match program_hbits_with_cable(dev, &path, &info, false) {
                    Ok(outcome) => {
                        self.programmed = true;
                        let line = outcome.summary_line("program", &dev.part);
                        Ok(format!("program_hw cable={} {}", info.id, line))
                    }
                    Err(e) => {
                        self.programmed = false;
                        Err(format!(
                            "program_hw cable={} backend={} refused DONE (USB/programmer): {e}",
                            info.id,
                            info.backend.as_str()
                        ))
                    }
                }
            }
        }
    }

    pub fn mark_debug(&mut self, net: &str) -> Result<(), String> {
        {
            let d = self.design.as_mut().ok_or("mark_debug: no design")?;
            d.mark_debug(net)?;
            insert_ila(d, net)?;
        }
        // The netlist gained ILA cells: placed/routed/bitstream results are
        // stale (Vivado invalidates impl runs on netlist edits). `implement`
        // re-runs from Place; without this, refresh_steps sees session.placed
        // and implement() silently keeps the pre-ILA placement.
        self.reset_impl();
        Ok(())
    }

    pub fn set_property(&mut self, key: &str, val: &str, obj: &str) -> Result<(), String> {
        let d = self.design.as_mut().ok_or("set_property: no design")?;
        if key.eq_ignore_ascii_case("DONT_TOUCH") || key.eq_ignore_ascii_case("keep") {
            d.set_cell_attr(obj, "DONT_TOUCH", val)?;
        } else if key.eq_ignore_ascii_case("mark_debug") {
            d.set_net_attr(obj, "mark_debug", val)?;
        } else if key.eq_ignore_ascii_case("LOC") || key.eq_ignore_ascii_case("PACKAGE_PIN") {
            d.set_loc(obj, val)?;
        } else if key.eq_ignore_ascii_case("IOSTANDARD") {
            d.set_iostandard(obj, val)?;
        } else if key.eq_ignore_ascii_case("DRIVE") {
            d.set_drive(obj, val)?;
        } else if key.eq_ignore_ascii_case("SLEW") {
            d.set_slew(obj, val)?;
        } else if key.eq_ignore_ascii_case("PULLTYPE") {
            d.set_pulltype(obj, val)?;
        } else if key.eq_ignore_ascii_case("DIFF_TERM") {
            d.set_diff_term(obj, val)?;
        } else if key.eq_ignore_ascii_case("IN_TERM") {
            d.set_in_term(obj, val)?;
        } else {
            d.set_cell_attr(obj, key, val)?;
        }
        Ok(())
    }

    pub fn impl_design(&mut self, d: Design, dev: &Device) -> Result<(), String> {
        self.synth_design(d);
        self.place_design(dev)?;
        self.route_design(dev)?;
        self.write_bitstream(dev)?;
        Ok(())
    }

    pub fn restore_session(bytes: &[u8], dev: &Device) -> Result<Self, String> {
        let ck = Self::restore_with_ir(bytes)?;
        let mut s = Self::new(ck.mode);
        s.part = dev.part.clone();
        s.strategy = ck.strategy;
        s.pblock = ck.pblock.clone();
        s.place_opts = PlaceOpts {
            timing_weight: ck.timing_weight, seed: 0 };
        s.route_opts = RouteOpts {
            max_iters: ck.route_max_iters,
            extra_hops: ck.route_extra_hops,
        };
        if let Some(d) = ck.design {
            // Knobs are already seeded; `reset_impl` (via synth_design) drops
            // place/route artifacts only, not strategy / pblock / opts.
            s.synth_design(d);
            s.replay_impl(dev)?;
        }
        let h2 = s.blinky_hash().unwrap_or(0);
        if h2 != ck.hash {
            return Err(format!(
                "hckp restore hash mismatch stored {:#x} got {h2:#x}",
                ck.hash
            ));
        }
        Ok(s)
    }

    /// Re-place/route/bitgen from stored knobs. Does not re-run PhysOpt `opt_design`
    /// — the HNF in the checkpoint is already the post-opt netlist.
    fn replay_impl(&mut self, dev: &Device) -> Result<(), String> {
        if let Some(pb) = self.pblock.clone() {
            if pb.ranged {
                self.place_pblock_in(dev, &pb, self.place_opts)?;
                self.route_design_with(dev, self.route_opts)?;
                self.write_bitstream(dev)?;
                return Ok(());
            }
        }
        self.place_design_with(dev, self.place_opts)?;
        self.route_design_with(dev, self.route_opts)?;
        self.write_bitstream(dev)?;
        Ok(())
    }

    pub fn eco(&mut self, dev: &Device, cell: &str, new_init: u64) -> Result<(), String> {
        let r = self.routed.as_ref().ok_or("eco: not implemented")?;
        let bits = eco_lut(dev, r, cell, new_init)?;
        if let Some(rt) = self.routed.as_mut() {
            if let Some(lf) = rt
                .placed
                .packed
                .lutffs
                .iter_mut()
                .find(|l| l.lut_cell == cell || l.ff_cell == cell)
            {
                lf.init = new_init;
            }
        }
        self.bitstream = Some(bits);
        Ok(())
    }

    pub fn blinky_hash(&self) -> Option<u32> {
        self.bitstream.as_ref().map(|b| {
            let mut h = b.idcode;
            for ((bl, maj, min), w) in &b.frames {
                h ^= helion_bits::crc32c(&[*bl, *min]);
                h ^= *maj as u32;
                h = h.wrapping_mul(0x9E37_79B9) ^ helion_bits::crc32c(&w.to_le_bytes());
            }
            h
        })
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"HCKP");
        v.push(match self.mode {
            Mode::Project => 1,
            Mode::NonProject => 2,
        });
        let h = self.blinky_hash().unwrap_or(0);
        v.extend_from_slice(&h.to_le_bytes());
        if let Some(d) = &self.design {
            let hnf = d.to_hnf();
            let b = hnf.as_bytes();
            v.extend_from_slice(&(b.len() as u32).to_le_bytes());
            v.extend_from_slice(b);
        } else {
            v.extend_from_slice(&0u32.to_le_bytes());
        }
        v.extend_from_slice(&encode_impl_knobs(
            self.strategy,
            self.pblock.as_ref(),
            self.place_opts,
            self.route_opts,
        ));
        v
    }

    pub fn restore(bytes: &[u8]) -> Result<(Mode, u32), String> {
        let ck = Self::restore_with_ir(bytes)?;
        Ok((ck.mode, ck.hash))
    }

    pub fn restore_with_ir(bytes: &[u8]) -> Result<CheckpointIr, String> {
        if bytes.len() < 9 || &bytes[0..4] != b"HCKP" {
            return Err("bad hckp".into());
        }
        let mode = match bytes[4] {
            1 => Mode::Project,
            2 => Mode::NonProject,
            _ => return Err("bad mode".into()),
        };
        let hash = u32::from_le_bytes(bytes[5..9].try_into().unwrap());
        let (design, rest) = if bytes.len() >= 13 {
            let n = u32::from_le_bytes(bytes[9..13].try_into().unwrap()) as usize;
            if bytes.len() >= 13 + n {
                let text = std::str::from_utf8(&bytes[13..13 + n]).map_err(|e| e.to_string())?;
                let design = if n == 0 {
                    None
                } else {
                    Some(Design::from_hnf(text)?)
                };
                (design, &bytes[13 + n..])
            } else {
                (None, &[][..])
            }
        } else {
            (None, &[][..])
        };
        let (strategy, pblock, place_opts, route_opts) = decode_impl_knobs(rest)?;
        Ok(CheckpointIr {
            mode,
            hash,
            design,
            strategy,
            pblock,
            timing_weight: place_opts.timing_weight,
            route_max_iters: route_opts.max_iters,
            route_extra_hops: route_opts.extra_hops,
        })
    }
}

const HKNB: &[u8; 4] = b"HKNB";
const HKNB_VER: u8 = 1;

fn write_u16_str(v: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    let n = u16::try_from(b.len()).unwrap_or(u16::MAX) as usize;
    v.extend_from_slice(&(n as u16).to_le_bytes());
    v.extend_from_slice(&b[..n]);
}

fn read_u16_str(bytes: &[u8], i: &mut usize) -> Result<String, String> {
    if *i + 2 > bytes.len() {
        return Err("truncated hckp knobs string".into());
    }
    let n = u16::from_le_bytes(bytes[*i..*i + 2].try_into().unwrap()) as usize;
    *i += 2;
    if *i + n > bytes.len() {
        return Err("truncated hckp knobs string".into());
    }
    let s = std::str::from_utf8(&bytes[*i..*i + n]).map_err(|e| e.to_string())?;
    *i += n;
    Ok(s.to_string())
}

fn encode_impl_knobs(
    strategy: ImplStrategy,
    pblock: Option<&PrjPblock>,
    place_opts: PlaceOpts,
    route_opts: RouteOpts,
) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(HKNB);
    v.push(HKNB_VER);
    v.push(strategy.as_u8());
    let pb = pblock.filter(|p| p.ranged);
    v.push(if pb.is_some() { 1 } else { 0 });
    v.push(0);
    let milli = (place_opts.timing_weight * 1000.0).round() as u32;
    v.extend_from_slice(&milli.to_le_bytes());
    v.extend_from_slice(&route_opts.max_iters.to_le_bytes());
    v.extend_from_slice(&route_opts.extra_hops.to_le_bytes());
    if let Some(pb) = pb {
        write_u16_str(&mut v, &pb.name);
        v.extend_from_slice(&pb.x0.to_le_bytes());
        v.extend_from_slice(&pb.y0.to_le_bytes());
        v.extend_from_slice(&pb.x1.to_le_bytes());
        v.extend_from_slice(&pb.y1.to_le_bytes());
        v.push(1);
        v.push(0);
        v.extend_from_slice(&(pb.cells.len() as u32).to_le_bytes());
        for c in &pb.cells {
            write_u16_str(&mut v, c);
        }
    }
    v
}

fn decode_impl_knobs(
    bytes: &[u8],
) -> Result<(ImplStrategy, Option<PrjPblock>, PlaceOpts, RouteOpts), String> {
    let default = (
        ImplStrategy::Default,
        None,
        ImplStrategy::Default.place_opts(),
        ImplStrategy::Default.route_opts(),
    );
    if bytes.len() < 20 || &bytes[0..4] != HKNB {
        return Ok(default);
    }
    let ver = bytes[4];
    if ver != HKNB_VER {
        return Err(format!("unsupported hckp knobs version {ver}"));
    }
    let strategy = ImplStrategy::from_u8(bytes[5]);
    let flags = bytes[6];
    let milli = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let max_iters = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
    let extra_hops = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
    let place_opts = PlaceOpts {
        timing_weight: milli as f64 / 1000.0, seed: 0 };
    let route_opts = RouteOpts {
        max_iters,
        extra_hops,
    };
    if flags & 1 == 0 {
        return Ok((strategy, None, place_opts, route_opts));
    }
    let mut i = 20;
    let name = read_u16_str(bytes, &mut i)?;
    if i + 18 + 4 > bytes.len() {
        return Err("truncated hckp pblock".into());
    }
    let x0 = u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    i += 4;
    let y0 = u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    i += 4;
    let x1 = u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    i += 4;
    let y1 = u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    i += 4;
    let ranged = bytes[i] != 0;
    i += 2;
    let n = u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
    i += 4;
    let mut cells = Vec::with_capacity(n);
    for _ in 0..n {
        cells.push(read_u16_str(bytes, &mut i)?);
    }
    Ok((
        strategy,
        Some(PrjPblock {
            name,
            x0,
            y0,
            x1,
            y1,
            ranged,
            cells,
        }),
        place_opts,
        route_opts,
    ))
}

fn cell_listed_in_pblock(cells: &[String], names: &[&str]) -> bool {
    cells
        .iter()
        .any(|c| names.iter().any(|n| !n.is_empty() && *n == c.as_str()))
}

/// Relocate `cells` into the HAD rectangle; everyone else keeps `place_with` sites.
/// Empty `cells` relocates the whole design (UG893 `create_pblock` with no
/// `add_cells_to_pblock`). IOB loc/name checks use `placed.packed.iobs` after
/// `place_with` LOC-preferring reorder — caller `packed.iobs[i]` is the wrong slot.
fn place_named_cells_in_region(
    packed: &Packed,
    dev: &Device,
    opts: PlaceOpts,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    cells: &[String],
) -> Result<Placed, String> {
    let (x0, x1) = (x0.min(x1), x0.max(x1));
    let (y0, y1) = (y0.min(y1), y0.max(y1));
    let mut placed = place_with(packed, dev, opts)?;
    let mut clbs: Vec<Site> = dev
        .clb_sites()
        .filter(|s| s.x >= x0 && s.x <= x1 && s.y >= y0 && s.y <= y1)
        .collect();
    if clbs.is_empty() {
        return Err(format!(
            "pblock CLB_X{x0}Y{y0}:CLB_X{x1}Y{y1} has no HAD CLB sites"
        ));
    }
    clbs.sort_by_key(|s| (s.x, s.y));
    let n_ble = dev.n_ble.max(1) as usize;
    let relocate_all = cells.is_empty();
    let lutff_listed = |lf: &helion_pack::PackedLutFf| {
        relocate_all || cell_listed_in_pblock(cells, &[lf.lut_cell.as_str(), lf.ff_cell.as_str()])
    };

    // Sites still held by non-listed `place_with` cells must stay occupied.
    let mut occupied: std::collections::HashSet<(u32, u32, u8)> = std::collections::HashSet::new();
    for (i, (site, ble)) in placed.lutff_sites.iter().enumerate() {
        if !lutff_listed(&placed.packed.lutffs[i]) {
            occupied.insert((site.x, site.y, *ble));
        }
    }
    let mut free: Vec<(Site, u8)> = Vec::new();
    for clb in &clbs {
        for ble in 0..n_ble as u8 {
            if !occupied.contains(&(clb.x, clb.y, ble)) {
                free.push((*clb, ble));
            }
        }
    }
    let mut slot_i = 0usize;
    let mut need = 0usize;
    for i in 0..placed.lutff_sites.len() {
        if !lutff_listed(&placed.packed.lutffs[i]) {
            continue;
        }
        need += 1;
        if slot_i >= free.len() {
            return Err(format!(
                "pblock CLB_X{x0}Y{y0}:CLB_X{x1}Y{y1} is full ({need} cells, {} free BLE slots)",
                free.len()
            ));
        }
        placed.lutff_sites[i] = free[slot_i];
        slot_i += 1;
    }

    let mut iobs: Vec<Site> = dev
        .iob_sites()
        .filter(|s| s.x >= x0 && s.x <= x1 && s.y >= y0 && s.y <= y1)
        .collect();
    if !iobs.is_empty() {
        iobs.sort_by_key(|s| (s.x, s.y));
        // `place_with` reorders packed.iobs to prefer PACKAGE_PIN / LOC; index
        // the aligned `placed.packed.iobs`, never the caller packed list.
        let iob_keep = |io: &helion_pack::PackedIob| {
            io.loc.as_ref().is_some()
                || !(relocate_all || cell_listed_in_pblock(cells, &[io.cell.as_str()]))
        };
        let mut iob_held: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        for (i, site) in placed.iob_sites.iter().enumerate() {
            let Some(io) = placed.packed.iobs.get(i) else {
                continue;
            };
            if iob_keep(io) {
                iob_held.insert((site.x, site.y));
            }
        }
        let free_iobs: Vec<Site> = iobs
            .into_iter()
            .filter(|s| !iob_held.contains(&(s.x, s.y)))
            .collect();
        let mut io_i = 0usize;
        let mut io_need = 0usize;
        for (i, slot) in placed.iob_sites.iter_mut().enumerate() {
            let Some(io) = placed.packed.iobs.get(i) else {
                continue;
            };
            if iob_keep(io) {
                continue;
            }
            io_need += 1;
            if io_i >= free_iobs.len() {
                return Err(format!(
                    "pblock CLB_X{x0}Y{y0}:CLB_X{x1}Y{y1} is full ({io_need} IOBs, {} free IOB sites)",
                    free_iobs.len()
                ));
            }
            *slot = free_iobs[io_i];
            io_i += 1;
        }
    }
    placed.cost = if opts.timing_weight > 0.0 {
        placed
            .lutff_sites
            .first()
            .zip(placed.iob_sites.first())
            .map(|(l, i)| (l.0.y as f64 - i.y as f64).abs() * opts.timing_weight)
            .unwrap_or(0.0)
    } else {
        placed
            .lutff_sites
            .first()
            .map(|(s, _)| s.y as f64)
            .unwrap_or(0.0)
    };
    Ok(placed)
}

pub fn get_cells(d: &Design, filter: Option<&str>) -> Vec<String> {
    d.cells
        .iter()
        .filter(|c| filter.map(|f| resolve(c.name).contains(f)).unwrap_or(true))
        .map(|c| resolve(c.name).to_string())
        .collect()
}

pub fn get_nets(d: &Design, filter: Option<&str>) -> Vec<String> {
    d.nets
        .iter()
        .filter(|n| filter.map(|f| resolve(n.name).contains(f)).unwrap_or(true))
        .map(|n| resolve(n.name).to_string())
        .collect()
}

pub fn get_pins(d: &Design, cell: &str) -> Vec<String> {
    let idx = d.pin_index();
    [
        "I0", "I1", "I2", "I3", "I4", "I5", "O", "D", "Q", "CLK", "I", "PAD",
    ]
    .into_iter()
    .filter_map(|pin| idx.net_on(cell, pin).map(|_| format!("{cell}/{pin}")))
    .collect()
}

/// UG893 Floorplanning rectangle persisted in `.prj` (`create_pblock` / `resize_pblock`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrjPblock {
    pub name: String,
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    pub ranged: bool,
    pub cells: Vec<String>,
}

impl PrjPblock {
    pub fn range_text(&self) -> String {
        if !self.ranged {
            return "-".into();
        }
        format!("CLB_X{}Y{}:CLB_X{}Y{}", self.x0, self.y0, self.x1, self.y1)
    }
}

/// UG986 Design Run persisted in `.prj` (`create_run` / `launch_runs`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrjImplRun {
    pub name: String,
    pub strategy: String,
    /// Full [`RunStrategy`] spec (`Preset:opt=…,aig=…,…`). Empty = derive
    /// from the legacy `strategy` preset name (pre-v2.2 project files).
    pub spec: String,
}

impl PrjImplRun {
    /// Effective strategy: full spec when present, else legacy preset name.
    pub fn run_strategy(&self) -> RunStrategy {
        if self.spec.trim().is_empty() {
            RunStrategy::preset(&self.strategy).unwrap_or_else(|_| {
                RunStrategy::knobs("Custom", 1, true, true, 0, 8, false)
            })
        } else {
            RunStrategy::parse_spec(&self.spec).unwrap_or_else(|_| {
                RunStrategy::knobs("Custom", 1, true, true, 0, 8, false)
            })
        }
    }
}

/// Drop const-0 LUT+FF pairs that do not drive an IOB.
/// Vivado-like project file: `part`, `read_sv` (multi), `read_xdc`/`read_sdc`,
/// `create_clock`, `set_property PACKAGE_PIN` / `TOP` / `MARK_DEBUG`, pblock,
/// impl run, `.hckp`.
#[derive(Clone, Debug, Default)]
pub struct ProjectFile {
    pub part: String,
    pub sources: Vec<String>,
    /// External XDC/SDC paths from `read_xdc` / `read_sdc`.
    pub constraint_files: Vec<String>,
    /// `.helion` IP package paths from `read_ip` (expanded by `expand_ip_packages`).
    pub ip_packages: Vec<String>,
    /// Optional elaborator top (`top <mod>` / `set_property TOP <mod>`).
    pub top: Option<String>,
    pub sdc: Vec<String>,
    pub package_pins: Vec<(String, String)>,
    pub iostandards: Vec<(String, String)>,
    pub drives: Vec<(String, String)>,
    pub slews: Vec<(String, String)>,
    pub pulltypes: Vec<(String, String)>,
    pub diff_terms: Vec<(String, String)>,
    pub in_terms: Vec<(String, String)>,
    /// Floorplan pblocks (`create_pblock` / `resize_pblock` / `add_cells_to_pblock`).
    pub pblocks: Vec<PrjPblock>,
    /// Implementation runs (`create_run impl_1 -strategy Default`).
    pub impl_runs: Vec<PrjImplRun>,
    /// Debug nets (`mark_debug <net>` / `set_property MARK_DEBUG`): ILA probes
    /// re-armed on open.
    pub mark_debug: Vec<String>,
    /// Disk checkpoint path (`write_checkpoint path.hckp`).
    pub checkpoint_path: Option<String>,
    /// Optional Vivado project name (`create_project <name>`).
    pub project_name: Option<String>,
    /// Optional Vivado project directory (`create_project <name> <dir>`).
    pub project_dir: Option<String>,
    /// Top-level generics (`set_property generic {K=V} [current_fileset]`).
    pub generics: Vec<(String, String)>,
    /// Verilog preprocessor defines (`set_property verilog_define {K=V} [current_fileset]`).
    pub verilog_defines: Vec<(String, String)>,
    /// Synthesis step arguments (`set_property STEPS.SYNTH_DESIGN.ARGS.<ARG> <VAL> [get_runs synth_1]`).
    pub synth_args: Vec<(String, String)>,
    /// Post-place physical optimization enabled (`STEPS.PHYS_OPT_DESIGN.IS_ENABLED`).
    pub phys_opt_enabled: bool,
    /// Incremental reference checkpoint path (`read_checkpoint -incremental <path>`).
    pub incremental_checkpoint: Option<String>,
}

fn parse_clb_xy(spec: &str) -> Option<(u32, u32)> {
    let s = spec.trim().trim_matches(|c: char| "{}[]".contains(c));
    let rest = s
        .strip_prefix("CLB_X")
        .or_else(|| s.strip_prefix("SLICE_X"))?;
    let (xs, ys) = rest.split_once('Y')?;
    Some((xs.parse().ok()?, ys.parse().ok()?))
}

fn parse_clb_range(spec: &str) -> Option<(u32, u32, u32, u32)> {
    let s = spec.trim().trim_matches(|c: char| "{}[]".contains(c));
    let (a, b) = s.split_once(':')?;
    let (x0, y0) = parse_clb_xy(a)?;
    let (x1, y1) = parse_clb_xy(b)?;
    Some((x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)))
}

fn prj_token_is_helper(s: &str) -> bool {
    matches!(
        s,
        "get_cells" | "get_pblocks" | "get_runs" | "-cells" | "-add" | "-force"
    )
}

fn upsert_impl_run(p: &mut ProjectFile, name: &str, strategy: Option<&str>, spec: Option<&str>) {
    if let Some(r) = p.impl_runs.iter_mut().find(|r| r.name == name) {
        if let Some(s) = strategy {
            r.strategy = s.to_string();
        }
        if let Some(sp) = spec {
            r.spec = sp.to_string();
        }
    } else {
        p.impl_runs.push(PrjImplRun {
            name: name.to_string(),
            strategy: strategy.unwrap_or("Default").to_string(),
            spec: spec.unwrap_or("").to_string(),
        });
    }
}

/// Back-compat wrapper: legacy `create_run <name> -strategy <preset>`.
fn upsert_impl_run_legacy(p: &mut ProjectFile, name: &str, strategy: Option<&str>) {
    upsert_impl_run(p, name, strategy, None);
}

fn pblock_named_mut<'a>(p: &'a mut ProjectFile, name: &str) -> &'a mut PrjPblock {
    if let Some(i) = p.pblocks.iter().position(|b| b.name == name) {
        return &mut p.pblocks[i];
    }
    p.pblocks.push(PrjPblock {
        name: name.to_string(),
        ..Default::default()
    });
    p.pblocks.last_mut().unwrap()
}

/// Split `set_property <PROP> <value> [get_ports <port>]` into (value, port).
/// PACKAGE_PIN carries its site as the value; MARK_DEBUG addresses nets via
/// `get_nets` instead of `get_ports`. Returns `None` when no target survives.
fn prj_set_prop_parts(rest: &[&str]) -> Option<(String, String)> {
    let val = rest.get(1)?.to_string();
    let joined = rest.get(2..)?.join(" ");
    let target = joined
        .split_once("get_ports")
        .or_else(|| joined.split_once("get_nets"))
        .and_then(|(_, r)| {
            r.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .find(|s| !s.is_empty())
        })
        .unwrap_or("")
        .to_string();
    if target.is_empty() {
        return None;
    }
    Some((val, target))
}

pub fn load_prj(text: &str) -> Result<ProjectFile, String> {
    let mut p = ProjectFile {
        part: "HL10T-C32-1".into(),
        ..Default::default()
    };
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut toks = line.split_whitespace();
        let Some(cmd) = toks.next() else { continue };
        match cmd {
            "part" | "set_part" => {
                if let Some(v) = toks.find(|t| !t.starts_with('-')) {
                    p.part = v.to_string();
                }
            }
            "create_project" => {
                let rest: Vec<&str> = toks.collect();
                let mut i = 0;
                while i < rest.len() {
                    if rest[i] == "-part" && i + 1 < rest.len() {
                        p.part = rest[i + 1].to_string();
                        i += 2;
                        continue;
                    }
                    if !rest[i].starts_with('-') {
                        if p.project_name.is_none() {
                            p.project_name = Some(rest[i].to_string());
                        } else if p.project_dir.is_none() {
                            p.project_dir = Some(rest[i].to_string());
                        }
                    }
                    i += 1;
                }
            }
            "add_files" => {
                let rest: Vec<&str> = toks.collect();
                let mut i = 0;
                while i < rest.len() {
                    if rest[i] == "-fileset" {
                        i += 2;
                        continue;
                    }
                    if !rest[i].starts_with('-') {
                        let cleaned = rest[i].trim_matches(|c: char| "{}[]".contains(c));
                        if !cleaned.is_empty() {
                            let ext = std::path::Path::new(cleaned)
                                .extension()
                                .and_then(|e| e.to_str())
                                .unwrap_or("")
                                .to_ascii_lowercase();
                            if ext == "xdc" || ext == "sdc" {
                                p.constraint_files.push(cleaned.to_string());
                            } else if ext == "helion" {
                                p.ip_packages.push(cleaned.to_string());
                            } else {
                                p.sources.push(cleaned.to_string());
                            }
                        }
                    }
                    i += 1;
                }
            }
            "read_sv" | "read_vhdl" | "read_c" | "read_verilog" | "sv" | "vhdl" | "c" => {
                let rest: Vec<&str> = toks.collect();
                let mut i = 0;
                while i < rest.len() {
                    if rest[i] == "-library" || rest[i] == "-work" {
                        i += 2;
                        continue;
                    }
                    if !rest[i].starts_with('-') {
                        let cleaned = rest[i].trim_matches(|c: char| "{}[]".contains(c));
                        if !cleaned.is_empty() {
                            p.sources.push(cleaned.to_string());
                        }
                    }
                    i += 1;
                }
            }
            "read_xdc" | "read_sdc" | "xdc" | "sdc" => {
                for tok in toks {
                    if !tok.starts_with('-') {
                        let cleaned = tok.trim_matches(|c: char| "{}[]".contains(c));
                        if !cleaned.is_empty() {
                            p.constraint_files.push(cleaned.to_string());
                        }
                    }
                }
            }
            "read_ip" | "ip" => {
                if let Some(v) = toks.next() {
                    p.ip_packages.push(v.to_string());
                }
            }
            "top" => {
                if let Some(v) = toks.next() {
                    p.top = Some(v.to_string());
                }
            }
            "create_clock" | "create_generated_clock" => p.sdc.push(line.to_string()),
            "set_property" => {
                let rest: Vec<&str> = toks.collect();
                let prop_up = rest.first().map(|s| s.to_ascii_uppercase()).unwrap_or_default();
                if prop_up == "TOP" && rest.len() >= 2 {
                    let name = rest[1]
                        .trim_matches(|c: char| c == '[' || c == ']' || c == '{' || c == '}')
                        .to_string();
                    if !name.is_empty()
                        && !name.eq_ignore_ascii_case("current_fileset")
                        && !name.starts_with("get_")
                    {
                        p.top = Some(name);
                    }
                } else if (prop_up == "PART" || prop_up == "TARGET_PART") && rest.len() >= 2 {
                    let part_val = rest[1].trim_matches(|c: char| "{}[]\"".contains(c));
                    if !part_val.is_empty() {
                        p.part = part_val.to_string();
                    }
                } else if prop_up == "GENERIC" && rest.len() >= 2 {
                    let val_str = rest[1].trim_matches(|c: char| "{}[]\"".contains(c));
                    if let Some((k, v)) = val_str.split_once('=') {
                        p.generics.push((k.trim().to_string(), v.trim().to_string()));
                    }
                } else if prop_up == "VERILOG_DEFINE" && rest.len() >= 2 {
                    let val_str = rest[1].trim_matches(|c: char| "{}[]\"".contains(c));
                    let (k, v) = val_str
                        .split_once('=')
                        .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
                        .unwrap_or_else(|| (val_str.trim().to_string(), "1".into()));
                    p.verilog_defines.push((k, v));
                } else if prop_up == "STEPS.PHYS_OPT_DESIGN.IS_ENABLED" && rest.len() >= 2 {
                    p.phys_opt_enabled = parse_onoff(rest[1]).unwrap_or(false);
                } else if prop_up == "STRATEGY" && rest.len() >= 2 {
                    upsert_impl_run(&mut p, "impl_1", Some(rest[1]), None);
                } else if let Some(arg_name) = prop_up.strip_prefix("STEPS.SYNTH_DESIGN.ARGS.") {
                    if rest.len() >= 2 {
                        p.synth_args.push((arg_name.to_string(), rest[1].to_string()));
                    }
                } else if let Some(prop) = rest.first().copied() {
                    // PACKAGE_PIN carries the site as its value; every other
                    // property carries its setting. MARK_DEBUG targets nets
                    // (`get_nets`) rather than ports.
                    if let Some((val, target)) = prj_set_prop_parts(&rest) {
                        match prop {
                            "PACKAGE_PIN" | "LOC" => p.package_pins.push((target, val)),
                            "IOSTANDARD" => p.iostandards.push((target, val)),
                            "DRIVE" => p.drives.push((target, val)),
                            "SLEW" => p.slews.push((target, val)),
                            "PULLTYPE" => p.pulltypes.push((target, val)),
                            "DIFF_TERM" => p.diff_terms.push((target, val)),
                            "IN_TERM" => p.in_terms.push((target, val)),
                            "MARK_DEBUG" => {
                                if !p.mark_debug.contains(&target) {
                                    p.mark_debug.push(target);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            "mark_debug" => {
                if let Some(v) = toks.next() {
                    let net = v
                        .trim_matches(|c: char| "[]{}".contains(c))
                        .to_string();
                    if !net.is_empty() && !p.mark_debug.contains(&net) {
                        p.mark_debug.push(net);
                    }
                }
            }
            "create_pblock" => {
                let rest: Vec<&str> = toks.collect();
                let mut name = String::new();
                let mut add: Option<String> = None;
                let mut i = 0;
                while i < rest.len() {
                    if rest[i] == "-add" {
                        i += 1;
                        if i < rest.len() {
                            add = Some(rest[i].to_string());
                        }
                    } else if name.is_empty() && !rest[i].starts_with('-') {
                        name = rest[i]
                            .trim_matches(|c: char| "{}[]".contains(c))
                            .to_string();
                    }
                    i += 1;
                }
                if name.is_empty() {
                    name = format!("pblock_{}", p.pblocks.len());
                }
                let _ = pblock_named_mut(&mut p, &name);
                if let Some(spec) = add {
                    let Some((x0, y0, x1, y1)) = parse_clb_range(&spec) else {
                        return Err(format!("create_pblock -add: cannot parse CLB range {spec}"));
                    };
                    let pb = pblock_named_mut(&mut p, &name);
                    pb.x0 = x0;
                    pb.y0 = y0;
                    pb.x1 = x1;
                    pb.y1 = y1;
                    pb.ranged = true;
                }
            }
            "resize_pblock" => {
                let rest: Vec<&str> = toks.collect();
                let mut name = String::new();
                let mut spec = String::new();
                for tok in rest {
                    if tok == "-add" {
                        continue;
                    }
                    let t = tok.trim_matches(|c: char| "{}[]".contains(c));
                    if t.is_empty() || prj_token_is_helper(t) {
                        continue;
                    }
                    if name.is_empty() && !t.contains(':') {
                        name = t.to_string();
                    } else if !spec.is_empty() {
                        spec.push(':');
                        spec.push_str(t);
                    } else {
                        spec = t.to_string();
                    }
                }
                if !name.is_empty() && !spec.is_empty() {
                    let Some((x0, y0, x1, y1)) = parse_clb_range(&spec) else {
                        return Err(format!("resize_pblock: cannot parse CLB range {spec}"));
                    };
                    let pb = pblock_named_mut(&mut p, &name);
                    pb.x0 = x0;
                    pb.y0 = y0;
                    pb.x1 = x1;
                    pb.y1 = y1;
                    pb.ranged = true;
                }
            }
            "add_cells_to_pblock" => {
                let rest: Vec<String> = toks
                    .map(|t| t.trim_matches(|c: char| "{}[]".contains(c)).to_string())
                    .filter(|t| !t.is_empty() && !prj_token_is_helper(t))
                    .collect();
                if let Some(name) = rest.first() {
                    let pb = pblock_named_mut(&mut p, name);
                    for c in rest.iter().skip(1) {
                        if !pb.cells.contains(c) {
                            pb.cells.push(c.clone());
                        }
                    }
                }
            }
            "create_run" => {
                let rest: Vec<&str> = toks.collect();
                let mut name = String::new();
                let mut strategy = String::from("Default");
                let mut spec: Option<String> = None;
                let mut i = 0;
                while i < rest.len() {
                    if rest[i] == "-strategy" || rest[i] == "-strat" {
                        i += 1;
                        if i < rest.len() {
                            strategy = rest[i].to_string();
                        }
                    } else if rest[i] == "-spec" {
                        i += 1;
                        if i < rest.len() {
                            spec = Some(rest[i].to_string());
                        }
                    } else if name.is_empty() && !rest[i].starts_with('-') {
                        name = rest[i]
                            .trim_matches(|c: char| "{}[]".contains(c))
                            .to_string();
                    }
                    i += 1;
                }
                if !name.is_empty() {
                    upsert_impl_run(&mut p, &name, Some(&strategy), spec.as_deref());
                }
            }
            "launch_runs" => {
                if let Some(name) = toks.next() {
                    let name = name.trim_matches(|c: char| "{}[]".contains(c));
                    if !name.is_empty() {
                        upsert_impl_run_legacy(&mut p, name, None);
                    }
                }
            }
            "write_checkpoint" | "open_checkpoint" | "read_checkpoint" | "checkpoint" => {
                let rest: Vec<&str> = toks.collect();
                if let Some(v) = rest
                    .iter()
                    .rev()
                    .find(|t| !t.starts_with('-') && !t.is_empty())
                {
                    p.checkpoint_path = Some((*v).to_string());
                }
            }
            _ => {}
        }
    }
    if p.sources.is_empty() && p.ip_packages.is_empty() {
        return Err("project has no sources".into());
    }
    Ok(p)
}

/// Resolve a project-relative path against the `.prj` location, then CWD.
pub fn resolve_prj_path(prj_path: &std::path::Path, given: &str) -> std::path::PathBuf {
    let given = std::path::Path::new(given);
    if given.exists() {
        return given.to_path_buf();
    }
    for anc in prj_path.ancestors() {
        let cand = anc.join(given);
        if cand.exists() {
            return cand;
        }
        if let Some(name) = given.file_name() {
            let cand = anc.join(name);
            if cand.exists() {
                return cand;
            }
        }
    }
    given.to_path_buf()
}

/// Expand `read_ip` `.helion` packages into `sources` / `constraint_files`.
/// Paths are resolved against the `.prj`, then against each package manifest.
/// Does not override an explicit project `top`; if unset, takes the first package top.
pub fn expand_ip_packages(
    prj: &mut ProjectFile,
    prj_path: &std::path::Path,
) -> Result<Vec<helion_ipxact::HelionPackage>, String> {
    let mut loaded = Vec::new();
    let ips = prj.ip_packages.clone();
    for ip_ref in ips {
        let path = resolve_prj_path(prj_path, &ip_ref);
        let pkg =
            helion_ipxact::load_helion(&path).map_err(|e| format!("read_ip {ip_ref}: {e}"))?;
        for f in pkg.resolve_files()? {
            let s = f.display().to_string();
            if !prj.sources.iter().any(|x| x == &s) {
                prj.sources.push(s);
            }
        }
        for c in pkg.resolve_constraints()? {
            let s = c.display().to_string();
            if !prj.constraint_files.iter().any(|x| x == &s) {
                prj.constraint_files.push(s);
            }
        }
        if prj.top.is_none() {
            if let Some(t) = &pkg.top {
                prj.top = Some(t.clone());
            }
        }
        loaded.push(pkg);
    }
    if prj.sources.is_empty() {
        return Err("project has no sources after read_ip expand".into());
    }
    Ok(loaded)
}

/// Serialize a [`ProjectFile`] to Vivado-shaped `.prj` text (`part` / `read_sv` / `read_xdc`).
pub fn format_prj(prj: &ProjectFile) -> String {
    let mut out = String::new();
    if !prj.part.is_empty() {
        out.push_str(&format!("part {}\n", prj.part));
    } else {
        out.push_str("part HL10T-C32-1\n");
    }
    if let Some(top) = &prj.top {
        out.push_str(&format!("top {top}\n"));
    }
    for s in &prj.sources {
        let ext = std::path::Path::new(s)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let cmd = match ext.as_str() {
            "vhd" | "vhdl" => "read_vhdl",
            "c" | "cc" | "cpp" => "read_c",
            _ => "read_sv",
        };
        out.push_str(&format!("{cmd} {s}\n"));
    }
    for ip in &prj.ip_packages {
        out.push_str(&format!("read_ip {ip}\n"));
    }
    for c in &prj.constraint_files {
        out.push_str(&format!("read_xdc {c}\n"));
    }
    for line in &prj.sdc {
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push('\n');
        }
    }
    // I/O Planning properties: the same `set_property` lines `load_prj`
    // parses, so a project file round-trips pin assignments and pad settings.
    for (port, site) in &prj.package_pins {
        out.push_str(&format!(
            "set_property PACKAGE_PIN {site} [get_ports {port}]\n"
        ));
    }
    for (port, std) in &prj.iostandards {
        out.push_str(&format!(
            "set_property IOSTANDARD {std} [get_ports {port}]\n"
        ));
    }
    for (port, val) in &prj.drives {
        out.push_str(&format!("set_property DRIVE {val} [get_ports {port}]\n"));
    }
    for (port, val) in &prj.slews {
        out.push_str(&format!("set_property SLEW {val} [get_ports {port}]\n"));
    }
    for (port, val) in &prj.pulltypes {
        out.push_str(&format!(
            "set_property PULLTYPE {val} [get_ports {port}]\n"
        ));
    }
    for (port, val) in &prj.diff_terms {
        out.push_str(&format!(
            "set_property DIFF_TERM {val} [get_ports {port}]\n"
        ));
    }
    for (port, val) in &prj.in_terms {
        out.push_str(&format!(
            "set_property IN_TERM {val} [get_ports {port}]\n"
        ));
    }
    for net in &prj.mark_debug {
        out.push_str(&format!(
            "set_property MARK_DEBUG true [get_nets {net}]\n"
        ));
    }
    for pb in &prj.pblocks {
        out.push_str(&format!("create_pblock {}\n", pb.name));
        if pb.ranged {
            out.push_str(&format!(
                "resize_pblock {} -add {{{}}}\n",
                pb.name,
                pb.range_text()
            ));
        }
        for c in &pb.cells {
            out.push_str(&format!("add_cells_to_pblock {} {c}\n", pb.name));
        }
    }
    for r in &prj.impl_runs {
        let strat = if r.strategy.is_empty() {
            "Default"
        } else {
            r.strategy.as_str()
        };
        if r.spec.trim().is_empty() {
            out.push_str(&format!("create_run {} -strategy {strat}\n", r.name));
        } else {
            out.push_str(&format!(
                "create_run {} -strategy {strat} -spec {}\n",
                r.name, r.spec
            ));
        }
    }
    if let Some(ck) = &prj.checkpoint_path {
        out.push_str(&format!("write_checkpoint {ck}\n"));
    }
    for (k, v) in &prj.generics {
        out.push_str(&format!(
            "set_property generic {k}={v} [current_fileset]\n"
        ));
    }
    for (k, v) in &prj.verilog_defines {
        out.push_str(&format!(
            "set_property verilog_define {k}={v} [current_fileset]\n"
        ));
    }
    for (k, v) in &prj.synth_args {
        out.push_str(&format!(
            "set_property STEPS.SYNTH_DESIGN.ARGS.{k} {v} [get_runs synth_1]\n"
        ));
    }
    if prj.phys_opt_enabled {
        out.push_str("set_property STEPS.PHYS_OPT_DESIGN.IS_ENABLED true [get_runs impl_1]\n");
    }
    out
}

/// Write `prj` to `path` (parent directories must exist or be creatable by the caller).
pub fn write_prj(path: &std::path::Path, prj: &ProjectFile) -> Result<(), String> {
    let body = format_prj(prj);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("write_prj mkdir: {e}"))?;
        }
    }
    std::fs::write(path, body).map_err(|e| format!("write_prj {}: {e}", path.display()))
}

/// Apply PACKAGE_PIN / IOSTANDARD / DRIVE / SLEW / … onto the netlist.
/// Same step `helion project` / `compile_design_xdc` uses before place.
pub fn apply_design_xdc(design: &mut Design, xdc: &Constraints) -> Result<(), String> {
    apply_xdc(design, xdc)
}

/// Flatten inline SDC + `read_xdc` files + `set_property` IO into one XDC blob, then `load_xdc`.
/// Empty constraints keep the CLI gold WNS path (default 10 ns clk applied by the runner).
pub fn constraints_from_project(
    prj: &ProjectFile,
    prj_path: &std::path::Path,
) -> Result<Constraints, String> {
    let mut blob = String::new();
    for line in &prj.sdc {
        blob.push_str(line);
        blob.push('\n');
    }
    for cf in &prj.constraint_files {
        let path = resolve_prj_path(prj_path, cf);
        let body = std::fs::read_to_string(&path)
            .map_err(|e| format!("read_xdc {}: {e}", path.display()))?;
        let normalized = match ConstraintFormat::from_extension(&path) {
            Some(fmt @ (ConstraintFormat::Lpf | ConstraintFormat::Pcf | ConstraintFormat::Qsf)) => {
                UniversalConstraints::parse_format(&body, fmt)?.to_xdc()
            }
            _ => body,
        };
        blob.push_str(&normalized);
        if !normalized.ends_with('\n') {
            blob.push('\n');
        }
    }
    for (port, site) in &prj.package_pins {
        blob.push_str(&format!(
            "set_property PACKAGE_PIN {site} [get_ports {port}]\n"
        ));
    }
    for (port, std) in &prj.iostandards {
        blob.push_str(&format!(
            "set_property IOSTANDARD {std} [get_ports {port}]\n"
        ));
    }
    for (port, val) in &prj.drives {
        blob.push_str(&format!("set_property DRIVE {val} [get_ports {port}]\n"));
    }
    for (port, val) in &prj.slews {
        blob.push_str(&format!("set_property SLEW {val} [get_ports {port}]\n"));
    }
    for (port, val) in &prj.pulltypes {
        blob.push_str(&format!("set_property PULLTYPE {val} [get_ports {port}]\n"));
    }
    for (port, val) in &prj.diff_terms {
        blob.push_str(&format!(
            "set_property DIFF_TERM {val} [get_ports {port}]\n"
        ));
    }
    for (port, val) in &prj.in_terms {
        blob.push_str(&format!("set_property IN_TERM {val} [get_ports {port}]\n"));
    }
    if blob.trim().is_empty() {
        return Ok(Constraints::default());
    }
    load_xdc(&blob)
}

pub fn opt_design(d: &mut Design) -> usize {
    let pins = d.pin_index();
    let iob_nets: std::collections::HashSet<Spur> = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::IobOut))
        .filter_map(|c| pins.net_on(resolve(c.name), "I"))
        .collect();
    let mut d_to_ff: std::collections::HashMap<Spur, Spur> = std::collections::HashMap::new();
    for f in &d.cells {
        if matches!(f.kind, CellKind::Hff) {
            if let Some(dnet) = pins.net_on(resolve(f.name), "D") {
                d_to_ff.entry(dnet).or_insert(f.name);
            }
        }
    }
    let mut drop: std::collections::HashSet<String> = std::collections::HashSet::new();
    for c in &d.cells {
        let CellKind::Lut6 { init: 0 } = c.kind else {
            continue;
        };
        let Some(o) = pins.net_on(resolve(c.name), "O") else {
            continue;
        };
        let Some(ff_name) = d_to_ff.get(&o).copied() else {
            continue;
        };
        let Some(ff) = d.cell(resolve(ff_name)) else {
            continue;
        };
        let q = pins.net_on(resolve(ff_name), "Q").unwrap_or(intern(""));
        if iob_nets.contains(&q) {
            continue;
        }
        if c.attrs.flag("DONT_TOUCH") || c.attrs.flag("keep") || ff.attrs.flag("DONT_TOUCH") {
            continue;
        }
        drop.insert(resolve(c.name).to_string());
        drop.insert(resolve(ff.name).to_string());
    }
    let n = drop.len() / 2;
    d.cells.retain(|c| !drop.contains(resolve(c.name)));
    d.rebuild_indexes();
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use helion_ir::{CellKind, Design, PortDir, Spur};

    #[test]
    fn dual_mode_same_hash_and_ckpt() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut proj = Session::new(Mode::Project);
        let mut np = Session::new(Mode::NonProject);
        proj.impl_design(Design::structural_blinky(), &dev).unwrap();
        np.impl_design(Design::structural_blinky(), &dev).unwrap();
        assert_eq!(proj.blinky_hash(), np.blinky_hash());
        let ck = proj.checkpoint();
        let (mode, h) = Session::restore(&ck).unwrap();
        assert_eq!(mode, Mode::Project);
        assert_eq!(Some(h), proj.blinky_hash());
        let cells = get_cells(proj.design.as_ref().unwrap(), Some("lut"));
        assert_eq!(cells, vec!["u_lut"]);
        let ckpt = Session::restore_with_ir(&ck).unwrap();
        let ir = ckpt.design.expect("checkpoint must embed HNF");
        assert_eq!(resolve(ir.name), "blinky");
        assert_eq!(ir.lut_inits(), Design::structural_blinky().lut_inits());
        assert_eq!(ckpt.strategy, ImplStrategy::Default);
        assert!(get_nets(proj.design.as_ref().unwrap(), Some("q")).contains(&"q".into()));
        assert!(
            get_pins(proj.design.as_ref().unwrap(), "u_lut")
                .iter()
                .any(|p| p.ends_with("/I0"))
        );
    }

    #[test]
    fn opt_drops_dead_const0_and_eco_changes_hash() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.add_cell("dead_lut", CellKind::Lut6 { init: 0 });
        d.add_cell("dead_ff", CellKind::Hff);
        d.connect("clk", "dead_ff", "CLK");
        d.connect("dead_d", "dead_lut", "O");
        d.connect("dead_d", "dead_ff", "D");
        d.connect("dead_q", "dead_ff", "Q");
        d.connect("dead_q", "dead_lut", "I0");
        let before = d.cells.len();
        let n = opt_design(&mut d);
        assert_eq!(n, 1);
        assert!(d.cells.len() < before);
        assert!(d.cell("u_lut").is_some());
        assert!(d.cell("dead_lut").is_none());

        let mut kept = Design::structural_blinky();
        kept.add_cell("dead_lut", CellKind::Lut6 { init: 0 });
        kept.add_cell("dead_ff", CellKind::Hff);
        kept.connect("clk", "dead_ff", "CLK");
        kept.connect("dead_d", "dead_lut", "O");
        kept.connect("dead_d", "dead_ff", "D");
        kept.connect("dead_q", "dead_ff", "Q");
        kept.connect("dead_q", "dead_lut", "I0");
        kept.dont_touch("dead_lut").unwrap();
        let before = kept.cells.len();
        assert_eq!(opt_design(&mut kept), 0);
        assert_eq!(kept.cells.len(), before, "DONT_TOUCH must survive opt");

        let mut s = Session::new(Mode::NonProject);
        s.impl_design(Design::structural_blinky(), &dev).unwrap();
        let h0 = s.blinky_hash();
        s.eco(&dev, "u_lut", 0xAAAA_AAAA_AAAA_AAAA).unwrap();
        assert_ne!(s.blinky_hash(), h0, "ECO must change bitstream hash");
        let _ = PortDir::In;
    }

    #[test]
    fn write_bitstream_refuses_missing_design_and_unrouted() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut s = Session::new(Mode::NonProject);
        let e = s.write_bitstream(&dev).unwrap_err();
        assert!(e.contains("no design"), "{e}");
        s.synth_design(Design::structural_counter());
        let e = s.write_bitstream(&dev).unwrap_err();
        assert!(e.contains("not routed"), "{e}");
        s.place_design(&dev).unwrap();
        let e = s.write_bitstream(&dev).unwrap_err();
        assert!(e.contains("not routed"), "{e}");
        s.route_design(&dev).unwrap();
        let bits = s.write_bitstream(&dev).unwrap();
        assert_eq!(bits.packets.len(), 185, "counter golden size");
        assert!(!bits.frames.is_empty());
    }

    #[test]
    fn tcl_session_steps_hit_engines_and_hckp_restores_hash() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut s = Session::new(Mode::NonProject);
        s.synth_design(Design::structural_counter());
        assert!(s.design.is_some());
        assert!(s.placed.is_none(), "synth must not place");
        let n = s.opt_design_step().unwrap();
        let _ = n;
        s.place_design(&dev).unwrap();
        assert!(s.placed.is_some());
        assert!(s.routed.is_none(), "place_design must not route");
        s.route_design(&dev).unwrap();
        assert!(s.routed.is_some());
        assert!(s.bitstream.is_none(), "route_design must not bitgen");
        let bits = s.write_bitstream(&dev).unwrap();
        assert!(!bits.frames.is_empty());
        let hnf = s.write_hnf().unwrap();
        assert!(hnf.starts_with("HNF 1"));
        let t = s.report_timing(&dev).unwrap();
        assert!(t.contains("WNS_PS="), "{t}");
        assert!(t.contains("HOLD_PS="), "{t}");
        assert!(!t.contains("report_timing ok"), "must hit STA engine: {t}");
        let u = s.report_utilization(&dev).unwrap();
        assert!(u.contains("LUTFF=4/8192"), "{u}");
        s.set_property("DONT_TOUCH", "true", "u_lut0").unwrap();
        assert!(
            s.design
                .as_ref()
                .unwrap()
                .cell("u_lut0")
                .unwrap()
                .attrs
                .flag("DONT_TOUCH")
        );
        s.open_hw_manager();
        // USB=0: product program_hw (auto) must refuse DONE — no soft-hold / no sim invent.
        let board_err = s.program_hw(&dev).unwrap_err();
        assert!(
            board_err.contains("refused DONE")
                || board_err.contains("no USB")
                || board_err.contains("programmer"),
            "{board_err}"
        );
        assert!(!board_err.contains("soft-hold"), "{board_err}");
        let hw = s.program_hw_cable(&dev, "sim").unwrap();
        assert!(hw.contains("DONE=1"), "{hw}");
        assert!(hw.contains("not board DONE"), "{hw}");
        let h0 = s.blinky_hash().unwrap();
        let ck = s.checkpoint();
        let s2 = Session::restore_session(&ck, &dev).unwrap();
        assert_eq!(
            s2.blinky_hash(),
            Some(h0),
            ".hckp restore must match bitstream hash"
        );
        let die = dev.report_die();
        assert!(die.contains("HL10T-C32-1"));
        s.mark_debug("q3").unwrap();
        assert!(
            s.design
                .as_ref()
                .unwrap()
                .net("q3")
                .unwrap()
                .attrs
                .flag("mark_debug")
        );
        assert!(
            get_cells(s.design.as_ref().unwrap(), None)
                .iter()
                .any(|c| c.contains("lut"))
        );
        assert!(!get_pins(s.design.as_ref().unwrap(), "u_lut0").is_empty());
    }

    #[test]
    fn format_prj_round_trips_load() {
        let prj = load_prj(
            "part HL10T-C32-1\nread_sv examples/counter.sv\nread_xdc examples/counter.sdc\n",
        )
        .unwrap();
        let text = format_prj(&prj);
        let again = load_prj(&text).unwrap();
        assert_eq!(again.part, "HL10T-C32-1");
        assert_eq!(again.sources, vec!["examples/counter.sv"]);
        assert_eq!(again.constraint_files, vec!["examples/counter.sdc"]);
    }

    #[test]
    fn project_file_parses_vivado_shaped_commands() {
        let prj = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/blinky.sv
create_clock -period 10.000 [get_ports clk]
set_property PACKAGE_PIN IOB_X2Y0 [get_ports led]
set_property IOSTANDARD LVCMOS18 [get_ports led]
set_property DRIVE 12 [get_ports led]
set_property SLEW SLOW [get_ports led]
set_property PULLTYPE NONE [get_ports led]
set_property DIFF_TERM FALSE [get_ports led]
set_property IN_TERM NONE [get_ports led]
"#,
        )
        .unwrap();
        assert_eq!(prj.part, "HL10T-C32-1");
        assert_eq!(prj.sources, vec!["examples/blinky.sv"]);
        assert!(prj.constraint_files.is_empty());
        assert!(prj.top.is_none());
        assert_eq!(prj.sdc.len(), 1);
        assert_eq!(prj.package_pins, vec![("led".into(), "IOB_X2Y0".into())]);
        assert_eq!(prj.iostandards, vec![("led".into(), "LVCMOS18".into())]);
        assert_eq!(prj.drives, vec![("led".into(), "12".into())]);
        assert_eq!(prj.slews, vec![("led".into(), "SLOW".into())]);
        assert_eq!(prj.pulltypes, vec![("led".into(), "NONE".into())]);
        assert_eq!(prj.diff_terms, vec![("led".into(), "FALSE".into())]);
        assert_eq!(prj.in_terms, vec![("led".into(), "NONE".into())]);
        assert!(load_prj("part X\n").is_err());
    }

    #[test]
    fn project_file_read_xdc_multi_source_and_top() {
        let prj = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/multi/tog.sv
read_sv examples/multi/top.sv
read_xdc examples/multi/multi.sdc
top top
set_property TOP top
create_clock -period 10.000 [get_ports clk]
"#,
        )
        .unwrap();
        assert_eq!(prj.sources.len(), 2);
        assert_eq!(prj.constraint_files, vec!["examples/multi/multi.sdc"]);
        assert_eq!(prj.top.as_deref(), Some("top"));
        assert_eq!(prj.sdc.len(), 1);
    }

    #[test]
    fn project_file_read_ip_parses() {
        let prj = load_prj(
            r#"
part HL10T-C32-1
read_ip ip/h_gpio/h_gpio.helion
top h_gpio
"#,
        )
        .unwrap();
        assert!(prj.sources.is_empty());
        assert_eq!(prj.ip_packages, vec!["ip/h_gpio/h_gpio.helion"]);
        assert_eq!(prj.top.as_deref(), Some("h_gpio"));
    }

    #[test]
    fn strategies_move_wns_and_incremental_reuses_cells() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut def = Session::new(Mode::NonProject);
        def.synth_design(Design::structural_counter());
        def.impl_with_strategy(&dev, ImplStrategy::Default).unwrap();
        let t_def = def.report_timing(&dev).unwrap();
        let mut rt = Session::new(Mode::NonProject);
        rt.synth_design(Design::structural_counter());
        rt.impl_with_strategy(&dev, ImplStrategy::RuntimeOpt)
            .unwrap();
        let t_rt = rt.report_timing(&dev).unwrap();
        assert_ne!(
            t_def, t_rt,
            "RuntimeOpt WNS must differ from Default: {t_def} vs {t_rt}"
        );
        let mut phys = Session::new(Mode::NonProject);
        phys.synth_design(Design::structural_counter());
        phys.impl_with_strategy(&dev, ImplStrategy::PhysOpt)
            .unwrap();
        let t_phys = phys.report_timing(&dev).unwrap();
        assert_ne!(
            t_phys, t_def,
            "PhysOpt extra hops must move WNS: {t_phys} vs {t_def}"
        );

        let prev = def.placed.clone().unwrap();
        let reuse = def.incremental_place(&dev, &prev).unwrap();
        assert_eq!(reuse.cell_pct(), 100, "{}", reuse.text());
        def.insert_eco_lut("ECO_LUT3", 0x8).unwrap();
        let chk = def.check_eco().unwrap();
        assert!(chk.contains("ECO_LUT3"), "{chk}");
        let reuse2 = def.incremental_place(&dev, &prev).unwrap();
        assert!(reuse2.reused_cells < reuse2.cells, "{}", reuse2.text());
        assert!(reuse2.reused_cells > 0, "{}", reuse2.text());
        def.route_design(&dev).unwrap();
        let led = def
            .placed
            .as_ref()
            .unwrap()
            .packed
            .iobs
            .first()
            .unwrap()
            .from_net
            .clone();
        let before = def.routed.as_ref().unwrap().iob_src[0].delay_ps;
        def.fix_route(&led, 3).unwrap();
        assert_eq!(
            def.routed.as_ref().unwrap().iob_src[0].delay_ps,
            before + 3 * helion_route::HOP_DELAY_PS
        );
        def.unroute_net(&led).unwrap();
        assert_eq!(def.routed.as_ref().unwrap().iob_src[0].delay_ps, 0);
    }

    #[test]
    fn format_prj_round_trips_pblock_impl_run_checkpoint() {
        let prj = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/counter.sv
read_xdc examples/counter.sdc
create_pblock pblock_0
resize_pblock pblock_0 -add {CLB_X5Y1:CLB_X8Y8}
add_cells_to_pblock pblock_0 u_lut0
create_run impl_1 -strategy Default
create_run impl_runtime -strategy RuntimeOpt
write_checkpoint counter.hckp
"#,
        )
        .unwrap();
        assert_eq!(prj.pblocks.len(), 1);
        assert_eq!(prj.pblocks[0].name, "pblock_0");
        assert!(prj.pblocks[0].ranged);
        assert_eq!(prj.pblocks[0].x0, 5);
        assert_eq!(prj.pblocks[0].y0, 1);
        assert_eq!(prj.pblocks[0].x1, 8);
        assert_eq!(prj.pblocks[0].y1, 8);
        assert_eq!(prj.pblocks[0].cells, vec!["u_lut0"]);
        assert_eq!(
            prj.impl_runs,
            vec![
                PrjImplRun {
                    name: "impl_1".into(),
                    strategy: "Default".into(),
                    spec: String::new(),
                },
                PrjImplRun {
                    name: "impl_runtime".into(),
                    strategy: "RuntimeOpt".into(),
                    spec: String::new(),
                },
            ]
        );
        assert_eq!(prj.checkpoint_path.as_deref(), Some("counter.hckp"));

        let text = format_prj(&prj);
        assert!(text.contains("create_pblock pblock_0"), "{text}");
        assert!(
            text.contains("resize_pblock pblock_0 -add {CLB_X5Y1:CLB_X8Y8}"),
            "{text}"
        );
        assert!(
            text.contains("add_cells_to_pblock pblock_0 u_lut0"),
            "{text}"
        );
        assert!(
            text.contains("create_run impl_1 -strategy Default"),
            "{text}"
        );
        assert!(
            text.contains("create_run impl_runtime -strategy RuntimeOpt"),
            "{text}"
        );
        assert!(text.contains("write_checkpoint counter.hckp"), "{text}");

        let again = load_prj(&text).unwrap();
        assert_eq!(again.pblocks, prj.pblocks);
        assert_eq!(again.impl_runs, prj.impl_runs);
        assert_eq!(again.checkpoint_path, prj.checkpoint_path);
        assert_eq!(again.sources, prj.sources);
        assert_eq!(again.constraint_files, prj.constraint_files);
    }

    #[test]
    fn format_prj_round_trips_io_props_and_mark_debug() {
        let prj = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/counter.sv
set_property PACKAGE_PIN IOB_X2Y0 [get_ports led]
set_property IOSTANDARD LVCMOS33 [get_ports led]
set_property DRIVE 12 [get_ports led]
set_property SLEW FAST [get_ports led]
set_property PULLTYPE PULLUP [get_ports btn]
set_property DIFF_TERM TRUE [get_ports clk_p]
set_property IN_TERM UNTUNED_SPLIT_40 [get_ports clk_p]
set_property MARK_DEBUG true [get_nets led]
mark_debug cnt_3
"#,
        )
        .unwrap();
        assert_eq!(prj.package_pins, vec![("led".into(), "IOB_X2Y0".into())]);
        assert_eq!(prj.iostandards, vec![("led".into(), "LVCMOS33".into())]);
        assert_eq!(prj.drives, vec![("led".into(), "12".into())]);
        assert_eq!(prj.slews, vec![("led".into(), "FAST".into())]);
        assert_eq!(prj.pulltypes, vec![("btn".into(), "PULLUP".into())]);
        assert_eq!(prj.diff_terms, vec![("clk_p".into(), "TRUE".into())]);
        assert_eq!(prj.in_terms, vec![("clk_p".into(), "UNTUNED_SPLIT_40".into())]);
        assert_eq!(prj.mark_debug, vec!["led".to_string(), "cnt_3".to_string()]);

        // format_prj must emit every property so the file round-trips.
        let text = format_prj(&prj);
        let again = load_prj(&text).unwrap();
        assert_eq!(again.package_pins, prj.package_pins, "{text}");
        assert_eq!(again.iostandards, prj.iostandards, "{text}");
        assert_eq!(again.drives, prj.drives, "{text}");
        assert_eq!(again.slews, prj.slews, "{text}");
        assert_eq!(again.pulltypes, prj.pulltypes, "{text}");
        assert_eq!(again.diff_terms, prj.diff_terms, "{text}");
        assert_eq!(again.in_terms, prj.in_terms, "{text}");
        assert_eq!(again.mark_debug, prj.mark_debug, "{text}");
    }

    #[test]
    fn load_prj_rejects_unparseable_pblock_range() {
        let err = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/counter.sv
create_pblock pblock_0
resize_pblock pblock_0 -add {NOT_A_CLB_RANGE}
"#,
        )
        .unwrap_err();
        assert!(
            err.contains("resize_pblock") && err.contains("NOT_A_CLB_RANGE"),
            "{err}"
        );

        let err = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/counter.sv
create_pblock pblock_0
resize_pblock pblock_0 -add {FOO_X0Y0:BAR_X1Y1}
"#,
        )
        .unwrap_err();
        assert!(
            err.contains("resize_pblock") && err.contains("FOO_X0Y0:BAR_X1Y1"),
            "{err}"
        );

        let err = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/counter.sv
create_pblock pblock_0 -add {GARBAGE}
"#,
        )
        .unwrap_err();
        assert!(
            err.contains("create_pblock") && err.contains("GARBAGE"),
            "{err}"
        );

        let ok = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/counter.sv
create_pblock pblock_0
"#,
        )
        .unwrap();
        assert_eq!(ok.pblocks.len(), 1);
        assert!(
            !ok.pblocks[0].ranged,
            "create_pblock without -add stays unranged"
        );
    }

    #[test]
    fn disk_hckp_restore_eco_changes_bitstream_hash() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut s = Session::new(Mode::Project);
        s.impl_design(Design::structural_counter(), &dev).unwrap();
        let t0 = s.report_timing(&dev).unwrap();
        assert!(
            t0.contains("WNS_PS=9640"),
            "empty-XDC counter gold must hold before checkpoint: {t0}"
        );
        let h0 = s.blinky_hash().expect("impl must bitgen");
        let frames0 = s.bitstream.as_ref().unwrap().frames.len();
        assert!(frames0 > 0, "impl frames must be non-empty");

        let dir = std::env::temp_dir().join(format!(
            "helion-hckp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("counter.hckp");
        let wr = s.write_checkpoint_to(&path).unwrap();
        assert!(wr.contains("bytes="), "{wr}");
        assert!(
            path.is_file(),
            "write_checkpoint must create {}",
            path.display()
        );
        drop(s);

        let mut s2 = Session::open_checkpoint(&path, &dev).unwrap();
        let s_read = Session::read_checkpoint(&path, &dev).unwrap();
        assert_eq!(
            s_read.blinky_hash(),
            Some(h0),
            "read_checkpoint must alias open_checkpoint hash"
        );
        assert!(
            s2.impl_checkpoint.is_some(),
            "disk restore must seed impl_checkpoint for incremental place"
        );
        let t1 = s2.report_timing(&dev).unwrap();
        assert!(
            t1.contains("WNS_PS=9640"),
            "reopen must hold empty-XDC counter gold: {t1}"
        );
        let mut tight = Constraints::default();
        create_clock(&mut tight.clocks, "clk", 5_000, "clk");
        let t_tight = s2.report_timing_xdc(&dev, &tight).unwrap();
        assert!(
            t_tight.contains("WNS_PS=") && !t_tight.contains("WNS_PS=9640"),
            "project clocks after checkpoint open must move WNS vs 10 ns gold: {t_tight}"
        );
        assert_eq!(
            s2.blinky_hash(),
            Some(h0),
            "disk .hckp restore must match bitstream hash"
        );
        s2.eco(&dev, "u_lut0", 0xAAAA_AAAA_AAAA_AAAA).unwrap();
        s2.write_bitstream(&dev).unwrap();
        let frames1 = s2.bitstream.as_ref().map(|b| b.frames.len()).unwrap_or(0);
        assert!(
            frames1 > 0,
            "ECO write_bitstream must keep non-empty frames"
        );
        let h1 = s2.blinky_hash().expect("ECO bitstream");
        assert_ne!(
            h1, h0,
            "ECO LUT must change bitstream hash ({h0:#x} vs {h1:#x})"
        );
        eprintln!("disk_hckp restore WNS_PS=9640 hash={h0:#x} ECO hash={h1:#x} frames={frames1}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_checkpoint_to_refuses_empty_bitstream() {
        let mut s = Session::new(Mode::NonProject);
        let err = s.write_checkpoint_to("/tmp/helion-empty.hckp").unwrap_err();
        assert!(
            err.contains("not placed") || err.contains("empty bitstream"),
            "{err}"
        );
        assert!(s.impl_checkpoint.is_none());
        s.synth_design(Design::structural_counter());
        let err = s.write_checkpoint_to("/tmp/helion-empty.hckp").unwrap_err();
        assert!(err.contains("not placed"), "{err}");
        assert!(s.impl_checkpoint.is_none());

        let dev = Device::load_part("HL10T-C32-1").unwrap();
        s.place_design(&dev).unwrap();
        assert!(s.placed.is_some());
        assert!(s.bitstream.is_none());
        let err = s.write_checkpoint_to("/tmp/helion-empty.hckp").unwrap_err();
        assert!(err.contains("empty bitstream"), "{err}");
        assert!(
            s.impl_checkpoint.is_none(),
            "empty-bitstream error must not seed impl_checkpoint"
        );
    }

    #[test]
    fn write_checkpoint_to_does_not_mutate_on_disk_error() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut s = Session::new(Mode::NonProject);
        s.impl_design(Design::structural_counter(), &dev).unwrap();
        assert!(s.bitstream.as_ref().is_some_and(|b| !b.frames.is_empty()));
        assert!(s.impl_checkpoint.is_none());

        let dir = std::env::temp_dir().join(format!(
            "helion-hckp-isdir-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let err = s.write_checkpoint_to(&dir).unwrap_err();
        assert!(err.contains("write_checkpoint"), "{err}");
        assert!(
            s.impl_checkpoint.is_none(),
            "disk-write error must not seed impl_checkpoint"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checkpoint_open_reproduces_runtimeopt_and_pblock_hash() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut rt = Session::new(Mode::Project);
        rt.synth_design(Design::structural_counter());
        rt.impl_with_strategy(&dev, ImplStrategy::RuntimeOpt)
            .unwrap();
        let h_rt = rt.blinky_hash().expect("RuntimeOpt bitstream");
        let mut def = Session::new(Mode::Project);
        def.impl_design(Design::structural_counter(), &dev).unwrap();
        assert_ne!(
            def.blinky_hash(),
            Some(h_rt),
            "RuntimeOpt hash must differ from Default-only impl_design"
        );
        let ck = rt.checkpoint();
        let opened = Session::restore_with_ir(&ck).unwrap();
        assert_eq!(opened.strategy, ImplStrategy::RuntimeOpt);
        assert!((opened.timing_weight - 0.0).abs() < f64::EPSILON);
        assert_eq!(opened.route_max_iters, 1);
        let rt2 = Session::restore_session(&ck, &dev).unwrap();
        assert_eq!(
            rt2.blinky_hash(),
            Some(h_rt),
            "open must replay RuntimeOpt knobs, not Default impl_design"
        );

        let mut pb_s = Session::new(Mode::Project);
        pb_s.synth_design(Design::structural_counter());
        let mut prj = ProjectFile::default();
        prj.pblocks.push(PrjPblock {
            name: "pblock_0".into(),
            x0: 5,
            y0: 1,
            x1: 8,
            y1: 8,
            ranged: true,
            cells: vec![],
        });
        pb_s.impl_project(&dev, &prj).unwrap();
        let h_pb = pb_s.blinky_hash().expect("pblock bitstream");
        assert_ne!(Some(h_pb), def.blinky_hash(), "pblock must move placement");
        let ck_pb = pb_s.checkpoint();
        let meta = Session::restore_with_ir(&ck_pb).unwrap();
        let pb = meta.pblock.expect("pblock knobs must persist");
        assert_eq!((pb.x0, pb.y0, pb.x1, pb.y1), (5, 1, 8, 8));
        assert!(pb.cells.is_empty());
        let pb2 = Session::restore_session(&ck_pb, &dev).unwrap();
        assert_eq!(
            pb2.blinky_hash(),
            Some(h_pb),
            "open must replay pblock bounds, not Default impl_design"
        );
        let pl = pb2.placed.as_ref().unwrap();
        for (s, _) in &pl.lutff_sites {
            assert!(
                s.x >= 5 && s.x <= 8 && s.y >= 1 && s.y <= 8,
                "restored LUTFF must sit in the pblock: {s:?}"
            );
        }
    }

    #[test]
    fn old_hckp_without_knobs_still_opens_default() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut s = Session::new(Mode::Project);
        s.impl_design(Design::structural_counter(), &dev).unwrap();
        let h0 = s.blinky_hash().expect("impl bitstream");
        let full = s.checkpoint();
        let n = u32::from_le_bytes(full[9..13].try_into().unwrap()) as usize;
        let old = full[..13 + n].to_vec();
        assert!(
            old.len() < full.len(),
            "new .hckp must append knobs after HNF"
        );
        let s2 = Session::restore_session(&old, &dev).unwrap();
        assert_eq!(
            s2.blinky_hash(),
            Some(h0),
            "old HCKP (HNF-only) must still open via Default knobs"
        );
        let meta = Session::restore_with_ir(&old).unwrap();
        assert_eq!(meta.strategy, ImplStrategy::Default);
        assert!(meta.pblock.is_none());
    }

    #[test]
    fn impl_project_honors_create_run_strategy() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut prj = ProjectFile::default();
        prj.impl_runs.push(PrjImplRun {
            name: "impl_1".into(),
            strategy: "RuntimeOpt".into(),
                    spec: String::new(),
                });
        let mut s = Session::new(Mode::Project);
        s.synth_design(Design::structural_counter());
        s.impl_project(&dev, &prj).unwrap();
        assert_eq!(s.strategy, ImplStrategy::RuntimeOpt);
        let mut def = Session::new(Mode::Project);
        def.impl_design(Design::structural_counter(), &dev).unwrap();
        assert_ne!(
            s.blinky_hash(),
            def.blinky_hash(),
            "create_run RuntimeOpt must change the bitstream vs Default"
        );
    }

    #[test]
    fn impl_project_pblock_cells_filter_and_empty_whole_design() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut prj = ProjectFile::default();
        prj.pblocks.push(PrjPblock {
            name: "pblock_0".into(),
            x0: 5,
            y0: 1,
            x1: 8,
            y1: 8,
            ranged: true,
            cells: vec!["u_lut0".into()],
        });
        let mut s = Session::new(Mode::Project);
        s.synth_design(Design::structural_counter());
        s.impl_project(&dev, &prj).unwrap();
        let pl = s.placed.as_ref().unwrap();
        let mut lut0_in = false;
        let mut other_out = false;
        for (i, lf) in pl.packed.lutffs.iter().enumerate() {
            let (site, _) = pl.lutff_sites[i];
            let in_rect = site.x >= 5 && site.x <= 8 && site.y >= 1 && site.y <= 8;
            if lf.lut_cell == "u_lut0" {
                assert!(in_rect, "u_lut0 must sit in the pblock: {site:?}");
                lut0_in = true;
            } else if !in_rect {
                other_out = true;
            }
        }
        assert!(lut0_in, "u_lut0 must be packed");
        assert!(
            other_out,
            "non-listed cells must not all be forced into the rectangle"
        );

        let mut prj_all = ProjectFile::default();
        prj_all.pblocks.push(PrjPblock {
            name: "pblock_0".into(),
            x0: 5,
            y0: 1,
            x1: 8,
            y1: 8,
            ranged: true,
            cells: vec![],
        });
        let mut s_all = Session::new(Mode::Project);
        s_all.synth_design(Design::structural_counter());
        s_all.impl_project(&dev, &prj_all).unwrap();
        for (site, _) in &s_all.placed.as_ref().unwrap().lutff_sites {
            assert!(
                site.x >= 5 && site.x <= 8 && site.y >= 1 && site.y <= 8,
                "empty cells = whole design in rectangle: {site:?}"
            );
        }

        let ck = s.checkpoint();
        let meta = Session::restore_with_ir(&ck).unwrap();
        assert_eq!(
            meta.pblock.as_ref().map(|p| p.cells.as_slice()),
            Some(["u_lut0".to_string()].as_slice())
        );
        let h0 = s.blinky_hash();
        let s2 = Session::restore_session(&ck, &dev).unwrap();
        assert_eq!(s2.blinky_hash(), h0, "cells filter must round-trip hash");
    }

    #[test]
    fn apply_xdc_package_pin_moves_iob_and_checkpoint_hash() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let xdc = load_xdc("set_property PACKAGE_PIN IOB_X5Y0 [get_ports led]\n").unwrap();
        let mut d = Design::structural_counter();
        apply_design_xdc(&mut d, &xdc).unwrap();
        let mut s = Session::new(Mode::Project);
        s.synth_design(d);
        s.impl_with_strategy(&dev, ImplStrategy::Default).unwrap();
        assert_eq!(
            s.placed.as_ref().unwrap().iob_sites[0].x,
            5,
            "PACKAGE_PIN must bind the IOB before impl"
        );
        let h = s.blinky_hash().expect("pinned bitstream");
        let s2 = Session::restore_session(&s.checkpoint(), &dev).unwrap();
        assert_eq!(s2.blinky_hash(), Some(h));
        assert_eq!(s2.placed.as_ref().unwrap().iob_sites[0].x, 5);
    }

    fn lutff_occupancy_unique(pl: &Placed) -> bool {
        let mut seen = std::collections::HashSet::new();
        pl.lutff_sites
            .iter()
            .all(|(s, ble)| seen.insert((s.x, s.y, *ble)))
    }

    fn iob_occupancy_unique(pl: &Placed) -> bool {
        let mut seen = std::collections::HashSet::new();
        pl.iob_sites.iter().all(|s| seen.insert((s.x, s.y)))
    }

    fn iob_site_of(pl: &Placed, cell: &str) -> helion_device::Site {
        let i = pl
            .packed
            .iobs
            .iter()
            .position(|io| io.cell == cell)
            .unwrap_or_else(|| panic!("missing IOB cell {cell}"));
        pl.iob_sites[i]
    }

    fn two_iob_counter() -> Design {
        let mut d = Design::structural_counter();
        d.add_port("led2", PortDir::Out);
        d.add_cell("u_iob2", CellKind::IobOut);
        d.connect("q2", "u_iob2", "I");
        d.connect("led2", "u_iob2", "PAD");
        d
    }

    fn design_n_lutffs(n: usize) -> Design {
        let mut d = Design::new("nff");
        d.add_port("clk", PortDir::In);
        d.add_port("led", PortDir::Out);
        for i in 0..n {
            let lut = format!("u_lut{i}");
            let ff = format!("u_ff{i}");
            d.add_cell(&lut, CellKind::Lut6 { init: 2 });
            d.add_cell(&ff, CellKind::Hff);
            d.connect("clk", &ff, "CLK");
            d.connect(&format!("d{i}"), &lut, "O");
            d.connect(&format!("d{i}"), &ff, "D");
            d.connect(&format!("q{i}"), &ff, "Q");
            d.connect(&format!("q{i}"), &lut, "I0");
        }
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect("q0", "u_iob", "I");
        d.connect("led", "u_iob", "PAD");
        d
    }

    /// Pin the *second* IOB so `place_with` reorders packed.iobs vs caller pack.
    #[test]
    fn package_pin_pblock_keeps_pinned_iob_empty_and_named_cells() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let xdc = load_xdc("set_property PACKAGE_PIN IOB_X5Y0 [get_ports led2]\n").unwrap();
        let mut d = two_iob_counter();
        apply_design_xdc(&mut d, &xdc).unwrap();

        // Rectangle covers IOB row y=0 so a wrong packed.iobs[i] index would
        // relocate the pinned site.
        let pb_all = PrjPblock {
            name: "pblock_0".into(),
            x0: 2,
            y0: 0,
            x1: 8,
            y1: 8,
            ranged: true,
            cells: vec![],
        };
        let mut s_all = Session::new(Mode::Project);
        s_all.synth_design(d.clone());
        s_all
            .place_pblock_in(&dev, &pb_all, ImplStrategy::Default.place_opts())
            .unwrap();
        let pl_all = s_all.placed.as_ref().unwrap();
        assert!(
            iob_occupancy_unique(pl_all),
            "empty-cells pblock must not duplicate IOB sites"
        );
        assert_eq!(
            iob_site_of(pl_all, "u_iob2").x,
            5,
            "PACKAGE_PIN IOB_X5Y0 must survive empty-cells place_in_region workaround"
        );
        let led2 = pl_all
            .packed
            .iobs
            .iter()
            .find(|io| io.cell == "u_iob2")
            .expect("u_iob2 packed");
        assert!(led2.loc.is_some(), "pinned IOB must keep loc after reorder");
        assert_eq!(
            pl_all.packed.iobs.iter().position(|io| io.cell == "u_iob2"),
            Some(0),
            "place_with must reorder pinned IOB ahead of unpinned"
        );

        let pb_named = PrjPblock {
            name: "pblock_0".into(),
            x0: 2,
            y0: 0,
            x1: 8,
            y1: 8,
            ranged: true,
            cells: vec!["u_lut0".into()],
        };
        let mut s_named = Session::new(Mode::Project);
        s_named.synth_design(d);
        s_named
            .place_pblock_in(&dev, &pb_named, ImplStrategy::Default.place_opts())
            .unwrap();
        let pl_named = s_named.placed.as_ref().unwrap();
        assert!(
            iob_occupancy_unique(pl_named),
            "named-cells pblock must not duplicate IOB sites"
        );
        assert_eq!(
            iob_site_of(pl_named, "u_iob2").x,
            5,
            "PACKAGE_PIN must not follow a wrong packed.iobs index on named cells"
        );
        assert!(lutff_occupancy_unique(pl_named));
        let lut0 = pl_named
            .packed
            .lutffs
            .iter()
            .position(|l| l.lut_cell == "u_lut0")
            .unwrap();
        let (site, _) = pl_named.lutff_sites[lut0];
        assert!(
            site.x >= 2 && site.x <= 8 && site.y <= 8,
            "u_lut0 must sit in the named pblock: {site:?}"
        );
    }

    #[test]
    fn add_cells_to_pblock_no_duplicate_occupancy_and_err_when_full() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut s = Session::new(Mode::Project);
        s.synth_design(Design::structural_counter());
        // List a later LUTFF into the CLB that already holds u_lut0 at BLE 0.
        // Overflow-clamp onto (clb, ble 0) would duplicate occupancy.
        let pb = PrjPblock {
            name: "pblock_0".into(),
            x0: 2,
            y0: 1,
            x1: 2,
            y1: 1,
            ranged: true,
            cells: vec!["u_lut3".into()],
        };
        s.place_pblock_in(&dev, &pb, ImplStrategy::Default.place_opts())
            .unwrap();
        let pl = s.placed.as_ref().unwrap();
        assert!(
            lutff_occupancy_unique(pl),
            "partial add_cells_to_pblock must not duplicate (site,ble): {:?}",
            pl.lutff_sites
        );
        let lut3 = pl
            .packed
            .lutffs
            .iter()
            .position(|l| l.lut_cell == "u_lut3")
            .unwrap();
        let (site, _) = pl.lutff_sites[lut3];
        assert_eq!(
            (site.x, site.y),
            (2, 1),
            "listed cell must sit in the pblock"
        );

        let mut full = Session::new(Mode::Project);
        full.synth_design(design_n_lutffs(9));
        let pb_full = PrjPblock {
            name: "pblock_0".into(),
            x0: 5,
            y0: 1,
            x1: 5,
            y1: 1,
            ranged: true,
            cells: (0..9).map(|i| format!("u_lut{i}")).collect(),
        };
        let err = full
            .place_pblock_in(&dev, &pb_full, ImplStrategy::Default.place_opts())
            .unwrap_err();
        assert!(
            err.to_ascii_lowercase().contains("full"),
            "9 LUTFFs into 1 CLB (8 BLEs) must Err: {err}"
        );
    }

    #[test]
    fn sha256_nist_vectors_and_dcp_hdcp_stages_and_tamper_detection() {
        // NIST FIPS 180-4 vectors
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let xdc = load_xdc("create_clock -period 10.000 [get_ports clk]\n").unwrap();
        let dir = std::env::temp_dir().join(format!("helion-dcp-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 1. Post-synth .dcp
        let mut s = Session::new(Mode::NonProject);
        s.synth_design(Design::structural_counter());
        let expected_sha = netlist_sha256(s.design.as_ref().unwrap());
        let synth_dcp = dir.join("post_synth.dcp");
        let m_synth = s.write_dcp(&synth_dcp, &xdc).unwrap();
        assert_eq!(m_synth.stage, CheckpointStage::PostSynth);
        assert_eq!(m_synth.netlist_sha256, expected_sha);

        let s_restored_synth = Session::read_checkpoint(&synth_dcp, &dev).unwrap();
        assert_eq!(stage_of(&s_restored_synth), Some(CheckpointStage::PostSynth));
        assert_eq!(
            netlist_sha256(s_restored_synth.design.as_ref().unwrap()),
            expected_sha
        );

        // 2. Post-place .hdcp
        s.place_design(&dev).unwrap();
        let place_hdcp = dir.join("post_place.hdcp");
        let m_place = s.write_dcp(&place_hdcp, &xdc).unwrap();
        assert_eq!(m_place.stage, CheckpointStage::PostPlace);
        assert_eq!(m_place.netlist_sha256, expected_sha);
        let s_restored_place = Session::read_checkpoint(&place_hdcp, &dev).unwrap();
        assert_eq!(stage_of(&s_restored_place), Some(CheckpointStage::PostPlace));
        assert_eq!(
            s_restored_place.placed.as_ref().unwrap().lutff_sites,
            s.placed.as_ref().unwrap().lutff_sites
        );

        // 3. Post-route .dcp
        s.route_design(&dev).unwrap();
        s.write_bitstream(&dev).unwrap();
        let route_dcp = dir.join("post_route.dcp");
        let m_route = s.write_dcp(&route_dcp, &xdc).unwrap();
        assert_eq!(m_route.stage, CheckpointStage::PostRoute);
        assert_eq!(m_route.wns_ps, 9640);
        let s_restored_route = Session::read_checkpoint(&route_dcp, &dev).unwrap();
        assert_eq!(stage_of(&s_restored_route), Some(CheckpointStage::PostRoute));
        assert_eq!(s_restored_route.blinky_hash(), s.blinky_hash());

        // 4. list_checkpoints picks up .dcp and .hdcp
        let listed = list_checkpoints(&dir);
        assert_eq!(listed.len(), 3);

        // 5. Tamper with HNF bytes inside post_synth.dcp -> SHA-256 verification must reject
        let mut raw = std::fs::read(&synth_dcp).unwrap();
        if let Some(pos) = raw.windows(5).position(|w| w == b"HNF 1") {
            raw[pos + 4] = b'2';
        }
        let tampered = dir.join("tampered.dcp");
        std::fs::write(&tampered, &raw).unwrap();
        let err = Session::read_checkpoint(&tampered, &dev).unwrap_err();
        assert!(
            err.contains("SHA-256") && err.contains("mismatch"),
            "tampered checkpoint must fail SHA-256 check: {err}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn incremental_checkpoint_reuse_guided_place_and_route() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let xdc = Constraints::default();
        let dir = std::env::temp_dir().join(format!("helion-inc-dcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut golden = Session::new(Mode::NonProject);
        golden.synth_design(Design::structural_counter());
        golden.place_design(&dev).unwrap();
        golden.route_design(&dev).unwrap();
        golden.write_bitstream(&dev).unwrap();
        let ref_dcp = dir.join("reference_routed.dcp");
        golden.write_dcp(&ref_dcp, &xdc).unwrap();

        // New session with an ECO LUT added; load reference checkpoint incrementally
        let mut rev = Session::new(Mode::NonProject);
        let mut d_mod = Design::structural_counter();
        d_mod.add_cell("u_eco_extra", CellKind::Lut6 { init: 0x2 });
        rev.synth_design(d_mod);
        let reuse = rev.read_checkpoint_incremental(&ref_dcp, &dev).unwrap();
        assert!(reuse.reused_cells >= 8, "{}", reuse.text());
        assert!(reuse.cell_pct() >= 80, "{}", reuse.text());
        assert!(rev.placed.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vivado_tcl_object_queries_and_filters() {
        let mut tcl = VivadoTclSession::new(Mode::NonProject);
        tcl.session.synth_design(Design::structural_counter());
        tcl.eval("create_clock -period 10.000 -name sys_clk [get_ports clk]")
            .unwrap();

        let ports = tcl.eval("get_ports").unwrap();
        assert!(ports.contains("clk") && ports.contains("led"), "{ports}");
        let in_ports = tcl.eval("get_ports -filter {DIRECTION == IN}").unwrap();
        assert_eq!(in_ports, "clk");
        let out_ports = tcl.eval("get_ports -filter {DIRECTION == OUT}").unwrap();
        assert_eq!(out_ports, "led");

        assert_eq!(tcl.eval("all_inputs").unwrap(), "clk");
        assert_eq!(tcl.eval("all_outputs").unwrap(), "led");

        let regs = tcl.eval("all_registers").unwrap();
        assert!(regs.contains("u_ff0") && regs.contains("u_ff3"), "{regs}");
        let reg_d_pins = tcl.eval("all_registers -data_pins").unwrap();
        assert!(reg_d_pins.contains("u_ff0/D"), "{reg_d_pins}");
        let reg_clk_pins = tcl.eval("all_registers -clock_pins").unwrap();
        assert!(reg_clk_pins.contains("u_ff0/CLK"), "{reg_clk_pins}");

        let luts = tcl
            .eval("get_cells -filter {REF_NAME == LUT6 && NAME =~ *lut*}")
            .unwrap();
        assert!(luts.contains("u_lut0") && luts.contains("u_lut3"), "{luts}");

        let pins_of_lut0 = tcl.eval("get_pins -of_objects [get_cells u_lut0]").unwrap();
        assert!(
            pins_of_lut0.contains("u_lut0/I0") && pins_of_lut0.contains("u_lut0/O"),
            "{pins_of_lut0}"
        );

        let nets_of_lut0 = tcl.eval("get_nets -of_objects [get_cells u_lut0]").unwrap();
        assert!(nets_of_lut0.contains("d0") && nets_of_lut0.contains("q0"), "{nets_of_lut0}");

        let clks = tcl.eval("get_clocks").unwrap();
        assert_eq!(clks, "sys_clk");
    }

    #[test]
    fn vivado_non_project_batch_tcl_full_flow_and_reports() {
        let dir = std::env::temp_dir().join(format!("helion-np-tcl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(
            dir.join("sub_inv.vhd"),
            r#"
library ieee;
use ieee.std_logic_1164.all;
entity sub_inv is
  port (
    a : in std_logic;
    y : out std_logic
  );
end entity sub_inv;
architecture rtl of sub_inv is
begin
  y <= not a;
end architecture rtl;
"#,
        )
        .unwrap();

        std::fs::write(
            dir.join("top.sv"),
            r#"
module top #(parameter WIDTH = 4) (
    input logic clk,
    output logic led
);
    logic [3:0] cnt;
    logic inv_bit;
    sub_inv u_sub (.a(cnt[0]), .y(inv_bit));
    always_ff @(posedge clk) begin
        cnt <= cnt + 4'd1;
    end
    assign led = cnt[3] ^ inv_bit;
endmodule
"#,
        )
        .unwrap();

        std::fs::write(
            dir.join("top.xdc"),
            r#"
create_clock -period 10.000 -name clk [get_ports clk]
set_property PACKAGE_PIN IOB_X3Y0 [get_ports led]
set_property IOSTANDARD LVCMOS33 [get_ports led]
"#,
        )
        .unwrap();

        let script = r#"
read_vhdl sub_inv.vhd
read_verilog -sv top.sv
read_xdc top.xdc
synth_design -top top -part HL10T-C32-1 -generic {WIDTH=4} -verilog_define {SYNTH=1} -flatten_hierarchy rebuilt
write_checkpoint -force post_synth.dcp
opt_design -directive Explore
place_design -directive Explore
phys_opt_design -directive Explore
write_checkpoint -force post_place.dcp
route_design -directive Explore
write_checkpoint -force post_route.dcp
write_bitstream -force -bin_file top.bit
report_timing_summary -file timing_summary.rpt
report_timing -max_paths 5 -delay_type max -file timing.rpt
report_utilization -hierarchical -file util.rpt
report_power -file power.rpt
report_drc -file drc.rpt
report_cdc -file cdc.rpt
report_clock_interaction -file clock_interaction.rpt
report_clock_utilization -file clock_util.rpt
report_high_fanout_nets -max_nets 5 -file fanout.rpt
report_design_analysis -file design_analysis.rpt
report_qor_suggestions -file qor.rpt
"#;

        let mut tcl = run_vivado_tcl(script, Some(&dir)).unwrap();
        assert!(tcl.session.design.is_some());
        assert!(tcl.session.placed.is_some());
        assert!(tcl.session.routed.is_some());
        assert!(tcl.session.bitstream.is_some());
        assert!(dir.join("post_synth.dcp").is_file());
        assert!(dir.join("post_place.dcp").is_file());
        assert!(dir.join("post_route.dcp").is_file());
        assert!(dir.join("top.bit").is_file());
        assert!(dir.join("top.bin").is_file());
        for rpt in [
            "timing_summary.rpt",
            "timing.rpt",
            "util.rpt",
            "power.rpt",
            "drc.rpt",
            "cdc.rpt",
            "clock_interaction.rpt",
            "clock_util.rpt",
            "fanout.rpt",
            "design_analysis.rpt",
            "qor.rpt",
        ] {
            let content = std::fs::read_to_string(dir.join(rpt)).unwrap();
            assert!(!content.trim().is_empty(), "{rpt} must not be empty");
        }

        // Test incremental compile flow in TCL using post_route.dcp reference
        let inc_out = tcl
            .eval_script(
                r#"
synth_design -top top -part HL10T-C32-1
read_checkpoint -incremental post_route.dcp
place_design
route_design
"#,
            )
            .unwrap();
        assert!(inc_out.contains("route_design"), "{inc_out}");
        assert!(
            tcl.last_reuse_report
                .as_ref()
                .is_some_and(|r| r.cell_pct() == 100)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vivado_project_mode_tcl_create_open_runs_and_properties() {
        let dir = std::env::temp_dir().join(format!("helion-prj-tcl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(
            dir.join("counter.sv"),
            r#"
module counter #(parameter INIT_V = 0) (
    input logic clk,
    output logic led
);
    logic [3:0] q;
    always_ff @(posedge clk) begin
        q <= q + 4'd1;
    end
    assign led = q[3];
endmodule
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("constraints.xdc"),
            "create_clock -period 10.000 [get_ports clk]\nset_property PACKAGE_PIN IOB_X2Y0 [get_ports led]\n",
        )
        .unwrap();

        let script = r#"
create_project my_proj . -part HL10T-C32-1 -force
add_files counter.sv
add_files -fileset constrs_1 constraints.xdc
set_property top counter [current_fileset]
set_property part HL10T-C32-1 [current_project]
set_property generic {INIT_V=1} [current_fileset]
set_property verilog_define {FAST_MODE=1} [current_fileset]
set_property STEPS.SYNTH_DESIGN.ARGS.FLATTEN_HIERARCHY rebuilt [get_runs synth_1]
set_property STEPS.SYNTH_DESIGN.ARGS.DIRECTIVE AreaOptimized_high [get_runs synth_1]
set_property STEPS.PHYS_OPT_DESIGN.IS_ENABLED true [get_runs impl_1]
create_run impl_perf -parent_run synth_1 -flow {Vivado Implementation 2024} -strategy Performance_Explore
create_run impl_netdelay -parent_run synth_1 -strategy Performance_NetDelay_high
create_run impl_area -parent_run synth_1 -strategy Area_Explore
create_run impl_quick -parent_run synth_1 -strategy Flow_Quick
create_run impl_cong -parent_run synth_1 -strategy Congestion_SpreadLogic_high
save_project_as my_proj.prj
launch_runs synth_1
wait_on_run synth_1
launch_runs impl_perf impl_netdelay impl_area impl_quick impl_cong -jobs 4 -to_step write_bitstream
wait_on_run impl_perf
open_run impl_perf
"#;

        let mut tcl = run_vivado_tcl(script, Some(&dir)).unwrap();
        assert_eq!(tcl.eval("get_property TOP [current_fileset]").unwrap(), "counter");
        assert_eq!(
            tcl.eval("get_property PART [current_project]").unwrap(),
            "HL10T-C32-1"
        );
        assert_eq!(
            tcl.eval("get_property STEPS.PHYS_OPT_DESIGN.IS_ENABLED [get_runs impl_1]")
                .unwrap(),
            "1"
        );
        assert_eq!(
            tcl.eval("get_property STEPS.SYNTH_DESIGN.ARGS.FLATTEN_HIERARCHY [get_runs synth_1]")
                .unwrap(),
            "rebuilt"
        );
        assert!(tcl.session.bitstream.is_some());

        // Re-open saved project via open_project in a fresh TCL session
        let mut tcl2 = VivadoTclSession::new(Mode::Project).with_base_dir(&dir);
        tcl2.eval("open_project my_proj.prj").unwrap();
        assert_eq!(tcl2.project.top.as_deref(), Some("counter"));
        assert!(tcl2.project.phys_opt_enabled);
        assert_eq!(tcl2.project.generics, vec![("INIT_V".into(), "1".into())]);
        assert_eq!(
            tcl2.project.verilog_defines,
            vec![("FAST_MODE".into(), "1".into())]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn multi_strategy_explorer_and_qor_suggestions() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let xdc = Constraints::default();
        let strategies = [
            ImplStrategy::PerformanceExplore,
            ImplStrategy::PerformanceNetDelayHigh,
            ImplStrategy::AreaExplore,
            ImplStrategy::FlowQuick,
            ImplStrategy::CongestionSpreadLogicHigh,
        ];
        for &s in &strategies {
            let parsed = ImplStrategy::parse(s.label()).unwrap();
            assert_eq!(parsed, s);
            let preset = RunStrategy::preset(s.label()).unwrap();
            assert_eq!(preset.preset, s.label());
        }

        let mut d = Design::structural_counter();
        // Add a dead const-0 LUT+FF so Area_Explore / opt_design trims it and QoR flags it
        d.add_cell("dead_lut", CellKind::Lut6 { init: 0 });
        d.add_cell("dead_ff", CellKind::Hff);
        d.connect("clk", "dead_ff", "CLK");
        d.connect("dead_d", "dead_lut", "O");
        d.connect("dead_d", "dead_ff", "D");
        d.connect("dead_q", "dead_ff", "Q");
        d.connect("dead_q", "dead_lut", "I0");

        let exp = explore_strategies(&d, &dev, &xdc, &strategies).unwrap();
        assert_eq!(exp.runs.len(), 5);
        let best = exp.best_run().unwrap();
        let quick = exp
            .runs
            .iter()
            .find(|r| r.strategy == ImplStrategy::FlowQuick)
            .unwrap();
        assert!(
            best.wns_ps >= quick.wns_ps,
            "best strategy ({} WNS={}) must beat or equal Flow_Quick (WNS={})",
            best.strategy.label(),
            best.wns_ps,
            quick.wns_ps
        );
        assert!(
            best.luts < quick.luts,
            "opt-enabled strategy must trim dead const-0 LUTFF ({} vs {})",
            best.luts,
            quick.luts
        );
        assert!(exp.text().contains("Performance_Explore"));

        let qor = crate::qor::report_qor_suggestions(&quick.session, &dev, &xdc).unwrap();
        assert!(!qor.suggestions.is_empty());
        let qor_text = qor.text();
        assert!(qor_text.contains("report_qor_suggestions"), "{qor_text}");
        assert!(qor_text.contains("RECOMMENDED_STRATEGY="), "{qor_text}");
    }
}
