//! GPUI Tcl-client shell: **tree**, **console**, **flow rail**.
//! Every command is a Tcl string dispatched onto the same Session.

pub const GPUI_TOOLKIT: &str = "gpui";

pub mod chrome;
pub use chrome::{Activity, Canvas, WorkspacePane, pane_for_workspace};
pub mod constraints_wizard;
pub use constraints_wizard::{
    WizardClock, WizardError, WizardException, WizardInput, WizardIoDelay, WizardStep, generate_sdc,
    parse_wns, validate, validate_sdc,
};
pub mod doctor;
pub mod ide;
pub mod ila;
pub mod power;
pub mod vcd;
pub mod vcd_wave;
pub mod learner_l1;pub mod learner_l2;
pub mod open_dialog;
pub mod physview;
pub mod qor;
pub mod surface;
pub mod tb;
pub mod tcl;
pub use tb::{
    TbFailure, TbFile, TbHistoryEntry, TbLogLine, TbRunConfig, TbRunRequest, TbSev, TbStatus,
    TbStreamEvent, discover_testbenches, is_testbench_file, run_testbench,
};
pub use tcl::{SelObj, TclInterp};
pub use learner_l1::{
    ConstraintProvenance, SessionStage, StageError, StageStatus,
};
pub use qor::{
    QorAction, QorInputs, QorReport, QorRule, QorSeverity, QorSuggestion, analyze_qor, qor_rules,
};
pub use learner_l2::{
    critical_path_highlight, highlight_set_from_path, packing_summary_from_placed, HighlightSet,
    HighlightSetExt,
};
pub use surface::{
    central_pane, ChromeDriver, ChromeState, EventClass, GeometryReport, JobHandle, JobKind,
    JobOutcome, UiTrace, apply_activity, apply_canvas, apply_more, geometry_at, queue_flow, spawn_job,
};
pub use open_dialog::{HDL_EXTENSIONS, dialog_backend, hdl_file_dialog, open_hdl_dialog, open_vcd_dialog};
pub use vcd::{SignalActivity, VcdActivity, parse_vcd};
pub use power::{
    CapTable, HierPower, LeakTable, NetPower, PowerConfig, PowerSeverity, PowerWarning,
    ResourcePower, VcdPowerReport, analyze_power,
};
pub use ide::{
    BdAddrEntry, BdHdlRow, BdPin, BdView, BitstreamFrame, BitstreamReport, BottomTab, ClockRegion, ConsoleLine, DesignRun, DeviceRoute,
    DeviceSiteView, DeviceView, FindHit, FlowStep, HierBox, HierarchyDrawing, HierarchyView,
    HwManager, HwStatReport, HwStatRow, IdeMessage,
    IdeModel, IoPortView, IpCatalogRow, LayoutKind, LocalRow, MemoryBlock,
    Ila, IlaStatus,
    MemoryWordRow, BreakpointRow, ForceRow, EditorMarker, MsgSeverity, NavAction, NavSection, SourceLine,
    ConstraintRow, ConstraintSection, MidRunUi,
    Pblock, ProjectSummaryGadget, PropertyRow, ProbeSelection, ReportCatalogRow,
    NetlistRow, NetlistTree, PackageDrawing, PackagePin, SchematicCamera, SchematicDrawing, SchematicPin,
    SchematicSymbol, SchematicView, SchematicWire, ScopeNode, SimLogRow, SimObject, EcoRow, IncrementalRow, StepState, TimingPath,
    schematic_kind_label, schematic_short_name,
    TimingPathPin, TimingPathSortCol,
    RunCol, best_run_index, sort_run_indices, sweep_wns_stats,
    UltraFastStage, Utilization, UtilizationReport, UtilOccupancy, HierOccupancy, VirtualBus,
    Waveform, WaveMarker, WaveCursorRow, WaveRadix, WaveStyle, WaveTrace, WorkspaceTab, WaveDensity,
};
pub use helion_drc::{Drc, DrcSeverity, DrcViolation};
pub use helion_sta::{
    CdcReport, CdcSeverity, CdcViolation, ClockInteraction, ClockInteractionCell, ClockNetwork,
    ClockNetworkReport, ClockRelation, MethodologyReport, MethodologySeverity,
    MethodologyViolation, PathGroupKind, PowerReport, TimingSummary, TimingSummaryGroup,
};


use helion_device::Device;
use helion_ir::Design;
use helion_proj::{
    Mode, Session, expand_ip_packages, load_prj, resolve_prj_path,
};
use std::collections::HashMap;
use std::path::Path;

/// Elaborate RTL by extension: `.prj` (multi-file + optional `top`), `.vhd`/`.vhdl`
/// through helion-vhdl, else SV.
pub fn synth_hdl_path(path: &Path) -> Result<Design, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "prj" => synth_prj_path(path),
        "vhd" | "vhdl" => helion_vhdl::synth_vhdl_path(path),
        _ => helion_sv::synth_sv_path(path),
    }
}

/// Like [`synth_hdl_path`], but also returns elaborated SVA checkers for
/// SystemVerilog sources. Non-SV sources yield empty checker lists.
pub fn synth_hdl_path_with_sva(
    path: &Path,
) -> Result<
    (
        Design,
        Vec<helion_sv::sva::SvaProperty>,
        Vec<helion_sv::sva::SvaImmProp>,
    ),
    String,
> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "vhd" | "vhdl" => Ok((helion_vhdl::synth_vhdl_path(path)?, Vec::new(), Vec::new())),
        "prj" => Ok((synth_prj_path(path)?, Vec::new(), Vec::new())),
        _ => {
            let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            let out =
                helion_sv::sva::synth_sv_with_sva(&src, &path.display().to_string())?;
            Ok((out.design, out.properties, out.immediates))
        }
    }
}

/// Multi-file Helion `.prj`: resolve `read_sv` sources and elaborate with optional `top`.
fn synth_prj_path(path: &Path) -> Result<Design, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut prj = load_prj(&text)?;
    let _ = expand_ip_packages(&mut prj, path)?;
    let src_paths: Vec<std::path::PathBuf> = prj
        .sources
        .iter()
        .map(|s| resolve_prj_path(path, s))
        .collect();
    for (src, resolved) in prj.sources.iter().zip(src_paths.iter()) {
        if !resolved.exists() {
            return Err(format!(
                "project source {src}: not found (tried {})",
                resolved.display()
            ));
        }
    }
    let rtl: Vec<std::path::PathBuf> = src_paths
        .into_iter()
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    let e = e.to_ascii_lowercase();
                    e == "sv" || e == "v"
                })
                .unwrap_or(false)
        })
        .collect();
    if rtl.is_empty() {
        return Err("project has no SystemVerilog sources".into());
    }
    if rtl.len() == 1 && prj.top.is_none() {
        return helion_sv::synth_sv_path(&rtl[0]);
    }
    let mut owned: Vec<(String, String)> = Vec::new();
    for p in &rtl {
        let src = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        owned.push((p.display().to_string(), src));
    }
    let refs: Vec<(&str, &str)> = owned
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let params: HashMap<String, u128> = HashMap::new();
    let opts = Default::default();
    let (d, _) = helion_sv::elaborate_sv_sources(&refs, prj.top.as_deref(), &params, &opts)?;
    Ok(d)
}

#[derive(Clone, Debug)]
pub struct Tree {
    pub nodes: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Console {
    pub journal: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct FlowRail {
    pub steps: Vec<&'static str>,
}

#[derive(Clone, Debug)]
pub struct GpuiShell {
    pub tree: Tree,
    pub console: Console,
    pub flow: FlowRail,
    pub mode: Mode,
    pub session: Session,
    pub part: String,
    /// Persistent Tcl session: variables, procs, clocks, selection.
    pub tcl_interp: TclInterp,
}

impl Default for GpuiShell {
    fn default() -> Self {
        Self {
            tree: Tree {
                nodes: vec!["sources".into(), "constraints".into(), "runs".into()],
            },
            console: Console { journal: vec![] },
            flow: FlowRail {
                steps: vec!["synth", "opt", "place", "route", "bits", "hw"],
            },
            mode: Mode::Project,
            session: Session::new(Mode::Project),
            part: "HL10T-C32-1".into(),
            tcl_interp: TclInterp::new(),
        }
    }
}

pub(crate) fn impl_if_needed(shell: &mut GpuiShell) -> Result<(), String> {
    if shell.session.bitstream.is_some() {
        return Ok(());
    }
    let d = shell
        .session
        .design
        .clone()
        .ok_or("no design (read_sv / synth_design first)")?;
    let dev = Device::load_part(&shell.part)?;
    shell.session.impl_design(d, &dev)
}

/// Tcl dispatch - every chrome button is a command string.
/// Tcl dispatch - every chrome button is a command string.
///
/// Evaluates `cmd` with the real Tcl-subset interpreter
/// ([`crate::tcl`]) against the shell's persistent session. `puts` output
/// is prepended to the returned value; errors are `ERROR: ...` style.
pub fn tcl_eval(shell: &mut GpuiShell, cmd: &str) -> Result<String, String> {
    shell.console.journal.push(cmd.into());
    // The interpreter lives on the shell, but `eval` needs `&mut shell`:
    // move it out, evaluate, move it back.
    let mut interp = std::mem::replace(&mut shell.tcl_interp, TclInterp::new());
    let r = interp.eval(cmd, shell);
    let puts_out = std::mem::take(&mut interp.output);
    shell.tcl_interp = interp;
    let combine = |v: String| {
        if puts_out.is_empty() {
            v
        } else if v.is_empty() {
            puts_out.clone()
        } else {
            format!("{puts_out}\n{v}")
        }
    };
    match r {
        Ok(v) => Ok(combine(v)),
        Err(e) => Err(combine(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcl_client_tree_console_flow() {
        let mut sh = GpuiShell::default();
        assert!(sh.tree.nodes.contains(&"sources".into()));
        assert!(sh.flow.steps.contains(&"synth"));
        assert_eq!(
            tcl_eval(&mut sh, "hds::tree").unwrap(),
            "sources constraints runs"
        );
        assert_eq!(
            tcl_eval(&mut sh, "hds::flow").unwrap(),
            "synth opt place route bits hw"
        );
        assert_eq!(tcl_eval(&mut sh, "hds::synth").unwrap(), "synth ok");
        assert!(sh.console.journal.iter().any(|j| j.contains("synth")));
        assert!(tcl_eval(&mut sh, "not_a_cmd").is_err());
        let sv =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv");
        let r = tcl_eval(&mut sh, &format!("read_sv {}", sv.display())).unwrap();
        assert!(r.contains("luts 4"), "{r}");
        assert!(tcl_eval(&mut sh, "get_cells").unwrap().contains("u_lut"));
        assert!(tcl_eval(&mut sh, "get_nets").unwrap().contains("cnt_3"));
        let sp = tcl_eval(&mut sh, "set_property DONT_TOUCH true u_luts0").unwrap();
        assert!(sp.contains("DONT_TOUCH"), "{sp}");
        assert!(tcl_eval(&mut sh, "write_hnf").unwrap().starts_with("HNF 1"));
        let bits = tcl_eval(&mut sh, "write_bitstream").unwrap();
        assert!(bits.contains("bytes"), "{bits}");
        assert_eq!(
            tcl_eval(&mut sh, "create_clock -period 10 clk").unwrap(),
            "clock clk period 10000ps"
        );
        let rt = tcl_eval(&mut sh, "report_timing").unwrap();
        assert!(
            rt.contains("WNS") && rt.contains("9640"),
            "report_timing must hit STA: {rt}"
        );
        let util = tcl_eval(&mut sh, "report_utilization").unwrap();
        assert!(util.contains("LUTFF"), "{util}");
        assert!(
            tcl_eval(&mut sh, "open_hw_manager")
                .unwrap()
                .contains("sim")
        );
        // USB=0: bare program_hw refuses board DONE; explicit sim for fabric.
        let board_err = tcl_eval(&mut sh, "program_hw").unwrap_err();
        assert!(
            board_err.contains("refused DONE")
                || board_err.contains("no USB")
                || board_err.contains("programmer"),
            "{board_err}"
        );
        assert!(!board_err.contains("soft-hold"), "{board_err}");
        let hw = tcl_eval(&mut sh, "program_hw cable=sim").unwrap();
        assert!(hw.contains("DONE=1"), "{hw}");
        assert!(
            hw.contains("not board DONE") || hw.contains("backend=sim"),
            "{hw}"
        );
        let md = tcl_eval(&mut sh, "mark_debug cnt_3").unwrap();
        assert!(md.contains("cnt_3"), "{md}");
        let eco = tcl_eval(&mut sh, "eco u_luts0 0xAAAAAAAAAAAAAAAA").unwrap();
        assert!(eco.contains("eco"), "{eco}");
        let ck = tcl_eval(&mut sh, "write_checkpoint").unwrap();
        assert!(ck.contains("hash"), "{ck}");
        let die = tcl_eval(&mut sh, "report_die").unwrap();
        assert!(die.contains("HL10T-C32-1"), "{die}");
        assert!(die.contains("LUT6=8192"), "{die}");
        assert!(
            die.contains("sites_clb=1024"),
            "die report must list HAD sites: {die}"
        );
        let fm = tcl_eval(&mut sh, "report_featuremap").unwrap();
        assert!(fm.contains("featuremap part=HL10T-C32-1"), "{fm}");
        assert!(fm.contains("BLE0.LUT.INIT[0] minor 0 bit 0"), "{fm}");
        assert!(fm.contains("BLE0.LUT.FRACTURE minor 4 bit 0"), "{fm}");
        assert!(!fm.contains("MISSING"), "{fm}");
    }

    #[test]
    fn tcl_place_route_are_stepwise_engines() {
        let mut sh = GpuiShell::default();
        let sv =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/blinky.sv");
        tcl_eval(&mut sh, &format!("synth_design {}", sv.display())).unwrap();
        let pl = tcl_eval(&mut sh, "place_design").unwrap();
        assert!(pl.contains("place_design"), "{pl}");
        assert!(sh.session.routed.is_none(), "place must not route");
        let rt = tcl_eval(&mut sh, "route_design").unwrap();
        assert!(rt.contains("route_design"), "{rt}");
        assert!(sh.session.bitstream.is_none(), "route must not bitgen");
        let bits = tcl_eval(&mut sh, "write_bitstream").unwrap();
        assert!(bits.contains("bytes"), "{bits}");
    }
}
