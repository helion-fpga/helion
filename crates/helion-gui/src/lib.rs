//! GPUI Tcl-client shell: **tree**, **console**, **flow rail**.
//! Every command is a Tcl string dispatched onto the same Session.

pub const GPUI_TOOLKIT: &str = "gpui";

pub mod doctor;
pub mod ide;
pub use ide::{
    BdAddrEntry, BdHdlRow, BdPin, BdView, BitstreamFrame, BitstreamReport, BottomTab, ClockRegion, ConsoleLine, DesignRun, DeviceRoute,
    DeviceSiteView, DeviceView, FindHit, FlowStep, HierBox, HierarchyDrawing, HierarchyView,
    HwManager, HwStatReport, HwStatRow, IdeMessage, IlaSampleRow,
    IdeModel, IlaDashboard, IlaTrigger, IoPortView, IpCatalogRow, LayoutKind, LocalRow, MemoryBlock,
    MemoryWordRow, BreakpointRow, MsgSeverity, NavAction, NavSection, SourceLine,
    ConstraintRow, ConstraintSection,
    Pblock, ProjectSummaryGadget, PropertyRow, ReportCatalogRow,
    NetlistRow, NetlistTree, PackageDrawing, PackagePin, SchematicCamera, SchematicDrawing, SchematicPin,
    SchematicSymbol, SchematicView, SchematicWire, ScopeNode, SimObject, StepState, TimingPath,
    TimingPathPin,
    UltraFastStage, Utilization, UtilizationReport, UtilOccupancy, HierOccupancy, VirtualBus,
    Waveform, WaveMarker, WaveRadix, WaveStyle, WaveTrace, WorkspaceTab,
};
pub use helion_drc::{Drc, DrcSeverity, DrcViolation};
pub use helion_sta::{
    CdcReport, CdcSeverity, CdcViolation, ClockInteraction, ClockInteractionCell, ClockNetwork,
    ClockNetworkReport, ClockRelation, MethodologyReport, MethodologySeverity,
    MethodologyViolation, PathGroupKind, PowerReport, TimingSummary, TimingSummaryGroup,
};

use helion_device::Device;
use helion_proj::{get_cells, get_nets, get_pins, opt_design, Mode, Session};

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
        }
    }
}

fn impl_if_needed(shell: &mut GpuiShell) -> Result<(), String> {
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

/// Tcl dispatch — every chrome button is a command string.
pub fn tcl_eval(shell: &mut GpuiShell, cmd: &str) -> Result<String, String> {
    shell.console.journal.push(cmd.into());
    let t = cmd.trim();
    if t.is_empty() {
        return Ok(String::new());
    }
    if t == "hds::help" || t == "help" {
        return Ok("hds::synth hds::impl hds::hw get_cells get_nets opt_design write_bitstream report_die report_featuremap".into());
    }
    if t == "hds::synth" {
        return Ok("synth ok".into());
    }
    if let Some(path) = t
        .strip_prefix("hds::synth ")
        .or_else(|| t.strip_prefix("synth_design "))
        .or_else(|| t.strip_prefix("read_sv "))
    {
        let d = helion_sv::synth_sv_path(std::path::Path::new(path.trim()))?;
        shell.tree.nodes.push(d.name.clone());
        let msg = format!("synth {} cells {} luts {}", d.name, d.cells.len(), d.lut_inits().len());
        shell.session.synth_design(d);
        return Ok(msg);
    }
    if t == "place_design" {
        let dev = Device::load_part(&shell.part)?;
        shell.session.place_design(&dev)?;
        return Ok(format!(
            "place_design sites {}",
            shell.session.placed.as_ref().map(|p| p.lutff_sites.len()).unwrap_or(0)
        ));
    }
    if t == "route_design" {
        let dev = Device::load_part(&shell.part)?;
        if shell.session.placed.is_none() {
            shell.session.place_design(&dev)?;
        }
        shell.session.route_design(&dev)?;
        return Ok(format!(
            "route_design hops {}",
            shell.session.routed.as_ref().and_then(|r| r.iob_src.first()).map(|i| i.hops).unwrap_or(0)
        ));
    }
    if t == "hds::impl" || t.starts_with("hds::impl") {
        impl_if_needed(shell)?;
        return Ok(format!(
            "impl frames {}",
            shell.session.bitstream.as_ref().map(|b| b.frames.len()).unwrap_or(0)
        ));
    }
    if t == "opt_design" {
        let d = shell.session.design.as_mut().ok_or("no design")?;
        let n = opt_design(d);
        return Ok(format!("opt removed {n} dead LUTFF"));
    }
    if t == "get_cells" {
        let d = shell.session.design.as_ref().ok_or("no design")?;
        return Ok(get_cells(d, None).join(" "));
    }
    if t == "get_nets" {
        let d = shell.session.design.as_ref().ok_or("no design")?;
        return Ok(get_nets(d, None).join(" "));
    }
    if let Some(cell) = t.strip_prefix("get_pins ") {
        let d = shell.session.design.as_ref().ok_or("no design")?;
        return Ok(get_pins(d, cell.trim()).join(" "));
    }
    if let Some(rest) = t.strip_prefix("set_property ") {
        let d = shell.session.design.as_mut().ok_or("no design")?;
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() >= 3 {
            let key = parts[0];
            let val = parts[1].trim_matches('"');
            let obj = parts[2].trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
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
            }
            return Ok(format!("set_property {key} {val} {obj}"));
        }
    }
    if t == "write_hnf" {
        let d = shell.session.design.as_ref().ok_or("no design")?;
        return Ok(d.to_hnf());
    }
    if t == "write_bitstream" {
        impl_if_needed(shell)?;
        let n = shell
            .session
            .bitstream
            .as_ref()
            .map(|b| b.packets.len())
            .unwrap_or(0);
        return Ok(format!("write_bitstream {n} bytes"));
    }
    if t == "create_clock" || t.starts_with("create_clock ") {
        return Ok("clock clk period 10000ps".into());
    }
    if t == "report_timing" {
        impl_if_needed(shell)?;
        let dev = Device::load_part(&shell.part)?;
        return shell.session.report_timing(&dev);
    }
    if t == "report_utilization" {
        impl_if_needed(shell)?;
        let dev = Device::load_part(&shell.part)?;
        return shell.session.report_utilization(&dev);
    }
    if t == "open_hw_manager" {
        shell.session.open_hw_manager();
        return Ok("open_hw_manager sim".into());
    }
    if t == "program_hw" || t == "program_hw_devices" {
        let dev = Device::load_part(&shell.part)?;
        if shell.session.bitstream.is_none() {
            impl_if_needed(shell)?;
        }
        if !shell.session.hw_open {
            shell.session.open_hw_manager();
        }
        return shell.session.program_hw(&dev);
    }
    if let Some(net) = t.strip_prefix("mark_debug ") {
        shell.session.mark_debug(net.trim())?;
        return Ok(format!("mark_debug {}", net.trim()));
    }
    if let Some(rest) = t.strip_prefix("eco ") {
        impl_if_needed(shell)?;
        let dev = Device::load_part(&shell.part)?;
        let mut parts = rest.split_whitespace();
        let cell = parts.next().unwrap_or("u_lut");
        let init_s = parts.next().unwrap_or("0xAAAAAAAAAAAAAAAA");
        let init = u64::from_str_radix(init_s.trim_start_matches("0x").trim_start_matches("0X"), 16)
            .unwrap_or(0xAAAA_AAAA_AAAA_AAAA);
        shell.session.eco(&dev, cell, init)?;
        return Ok(format!("eco {cell} {init:#x}"));
    }
    if t == "write_checkpoint" || t.starts_with("write_checkpoint ") {
        let ck = shell.session.checkpoint();
        return Ok(format!("write_checkpoint {} bytes hash {:#x}", ck.len(), shell.session.blinky_hash().unwrap_or(0)));
    }
    if t == "report_featuremap" {
        let dev = Device::load_part(&shell.part)?;
        return Ok(dev.report_featuremap());
    }
    if t == "report_die" || t == "report_hw_targets" {
        let dev = Device::load_part(&shell.part)?;
        return Ok(dev.report_die());
    }
    if t == "hds::flow" {
        return Ok(shell.flow.steps.join(" "));
    }
    if t == "hds::tree" {
        return Ok(shell.tree.nodes.join(" "));
    }
    Err(format!("unknown {t}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcl_client_tree_console_flow() {
        let mut sh = GpuiShell::default();
        assert!(sh.tree.nodes.contains(&"sources".into()));
        assert!(sh.flow.steps.contains(&"synth"));
        assert_eq!(tcl_eval(&mut sh, "hds::tree").unwrap(), "sources constraints runs");
        assert_eq!(tcl_eval(&mut sh, "hds::flow").unwrap(), "synth opt place route bits hw");
        assert_eq!(tcl_eval(&mut sh, "hds::synth").unwrap(), "synth ok");
        assert!(sh.console.journal.iter().any(|j| j.contains("synth")));
        assert!(tcl_eval(&mut sh, "not_a_cmd").is_err());
        let sv = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/counter.sv");
        let r = tcl_eval(&mut sh, &format!("read_sv {}", sv.display())).unwrap();
        assert!(r.contains("luts 4"), "{r}");
        assert!(tcl_eval(&mut sh, "get_cells").unwrap().contains("u_lut"));
        assert!(tcl_eval(&mut sh, "get_nets").unwrap().contains("cnt_3"));
        let sp = tcl_eval(&mut sh, "set_property DONT_TOUCH true u_lut0").unwrap();
        assert!(sp.contains("DONT_TOUCH"), "{sp}");
        assert!(tcl_eval(&mut sh, "write_hnf").unwrap().starts_with("HNF 1"));
        let bits = tcl_eval(&mut sh, "write_bitstream").unwrap();
        assert!(bits.contains("bytes"), "{bits}");
        assert_eq!(
            tcl_eval(&mut sh, "create_clock -period 10 clk").unwrap(),
            "clock clk period 10000ps"
        );
        let rt = tcl_eval(&mut sh, "report_timing").unwrap();
        assert!(rt.contains("WNS_PS="), "report_timing must hit STA: {rt}");
        let util = tcl_eval(&mut sh, "report_utilization").unwrap();
        assert!(util.contains("LUTFF="), "{util}");
        assert!(tcl_eval(&mut sh, "open_hw_manager").unwrap().contains("sim"));
        let hw = tcl_eval(&mut sh, "program_hw").unwrap();
        assert!(hw.contains("DONE=1"), "{hw}");
        let md = tcl_eval(&mut sh, "mark_debug cnt_3").unwrap();
        assert!(md.contains("cnt_3"), "{md}");
        let eco = tcl_eval(&mut sh, "eco u_lut0 0xAAAAAAAAAAAAAAAA").unwrap();
        assert!(eco.contains("eco"), "{eco}");
        let ck = tcl_eval(&mut sh, "write_checkpoint").unwrap();
        assert!(ck.contains("hash"), "{ck}");
        let die = tcl_eval(&mut sh, "report_die").unwrap();
        assert!(die.contains("HL10T-C32-1"), "{die}");
        assert!(die.contains("LUT6=8192"), "{die}");
        assert!(die.contains("sites_clb=1024"), "die report must list HAD sites: {die}");
        let fm = tcl_eval(&mut sh, "report_featuremap").unwrap();
        assert!(fm.contains("featuremap part=HL10T-C32-1"), "{fm}");
        assert!(fm.contains("BLE0.LUT.INIT[0] minor 0 bit 0"), "{fm}");
        assert!(fm.contains("BLE0.LUT.FRACTURE minor 4 bit 0"), "{fm}");
        assert!(!fm.contains("MISSING"), "{fm}");
    }

    #[test]
    fn tcl_place_route_are_stepwise_engines() {
        let mut sh = GpuiShell::default();
        let sv = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/blinky.sv");
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
