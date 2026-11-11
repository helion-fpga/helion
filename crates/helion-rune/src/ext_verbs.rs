//! Tcl verbs added for the implementation / analysis / simulation parity
//! surfaces. [`crate::tcl`] routes every name in [`VERBS`] here and asks
//! [`help`] for their usage lines.

use crate::state::AppState;

/// Every verb this module owns.
pub fn is_verb(name: &str) -> bool {
    crate::views::reports::io::VERBS.contains(&name)
        || name == "report_power_vcd"
        || crate::views::eco::VERBS.contains(&name)
}

/// Run one verb (`args` without the verb name).
pub fn tcl_verb(s: &mut AppState, name: &str, args: &[String]) -> Result<String, String> {
    if crate::views::reports::io::VERBS.contains(&name) {
        return crate::views::reports::io::tcl_verb(s, name, args);
    }
    if crate::views::eco::VERBS.contains(&name) {
        return crate::views::eco::tcl_verb(s, name, args);
    }
    if name == "report_power_vcd" {
        return crate::views::reports::vec_power::tcl_verb(s, args);
    }
    Err(format!("{name}: unknown command"))
}

/// `(usage, description)` for `help <verb>`.
pub fn help(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "report_clock_utilization" => (
            "report_clock_utilization",
            "clock buffers, loads and regions per clock domain (Reports > Clock Utilization)",
        ),
        "report_control_sets" => (
            "report_control_sets",
            "unique clock/enable/reset sets, fanout histogram, slice loads",
        ),
        "report_ssn" => (
            "report_ssn",
            "simultaneous switching noise margins over the placed I/O",
        ),
        "write_ibis" => ("write_ibis ?file.ibs?", "write an IBIS 5.0 model of the placed I/O"),
        "report_power_vcd" => (
            "report_power_vcd ?file.vcd?",
            "vectored power from simulation activity (default: the last testbench VCD)",
        ),
        n if crate::views::eco::VERBS.contains(&n) => return crate::views::eco::help(n),
        _ => return None,
    })
}
