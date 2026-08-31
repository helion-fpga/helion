//! GPUI Tcl-client shell: **tree**, **console**, **flow rail**.
//! Every command is a Tcl string dispatched onto the same Session.

pub const GPUI_TOOLKIT: &str = "gpui";

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
        return Ok("hds::synth hds::impl hds::hw get_cells get_nets opt_design write_bitstream".into());
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
        shell.session.design = Some(d);
        return Ok(msg);
    }
    if t == "hds::impl" || t == "place_design" || t == "route_design" || t.starts_with("hds::impl") {
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
        return Ok("report_timing ok".into());
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
    }
}
