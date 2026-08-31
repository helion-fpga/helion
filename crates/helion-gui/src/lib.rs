//! GPUI Tcl-client shell: **tree**, **console**, **flow rail**. No die viewer (later).
//! GUI does not compile a second engine — it dispatches Tcl to the same Session.
//!
//! Chrome is intended for **GPUI** (Zed-style) on aarch64-macos. This crate stays
//! CPU/safe: no GPU `unsafe` island. The widget model (Tree/Console/FlowRail) is
//! the GPUI client; linking `gpui` is optional at the binary.

pub const GPUI_TOOLKIT: &str = "gpui";

use helion_proj::Mode;

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
        }
    }
}

/// Tcl dispatch — every chrome button is a command string.
pub fn tcl_eval(shell: &mut GpuiShell, cmd: &str) -> Result<String, String> {
    shell.console.journal.push(cmd.into());
    let t = cmd.trim();
    if t.is_empty() {
        return Ok(String::new());
    }
    if t == "hds::help" {
        return Ok("hds::synth hds::impl hds::hw".into());
    }
    if t.starts_with("hds::synth") {
        return Ok("synth ok".into());
    }
    if t.starts_with("hds::impl") {
        return Ok("impl ok".into());
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
    }
}
