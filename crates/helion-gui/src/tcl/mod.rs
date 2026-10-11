//! Real Tcl-subset interpreter + object model for the Helion IDE console.
//!
//! This is a genuine interpreter, not pattern matching: [`parse`] implements
//! Tcl word rules (`{}`, `""`, `[...]`, `$var`, backslashes, comments),
//! [`interp::TclInterp`] evaluates scripts with real variables, scopes,
//! procedures, loops, `expr`, lists and `catch`, and [`design`] implements
//! the Vivado-style object model (`get_cells`, `get_property`, `-filter`,
//! `-of_objects`) plus the flow commands against the live
//! [`helion_proj::Session`] engines.
//!
//! The persistent session state (variables, procs, `create_clock` clocks,
//! selection) lives in [`interp::TclInterp`], stored on [`crate::GpuiShell`]
//! so it survives across console commands. [`crate::tcl_eval`] is the entry
//! point used by the IDE console, the stdin console and the chrome buttons.
//!
//! Supported subset (honest list): `set`, `unset`, `puts`, `expr`, `if`,
//! `foreach`, `for`, `while`, `proc`, `return`, `break`, `continue`, `list`,
//! `llength`, `lindex`, `lrange`, `lappend`, `concat`, `join`, `split`,
//! `string` (length/range/index/match/tolower/toupper/trim/first/equal),
//! `info` (exists/vars/procs/commands/args/body), `catch`, `error`, `eval`,
//! `subst`, `source`, `format`, `incr`, `append`, `help`.
//!
//! Object model: `get_cells`, `get_nets`, `get_pins`, `get_ports`,
//! `get_clocks`, `get_timing_paths`, `get_property`, `set_property`,
//! `report_property`, `list_property`, `select_objects`,
//! `highlight_objects`, `get_selected_objects`, `current_design`.
//!
//! Flow: `synth_design`/`read_sv`, `opt_design`, `place_design`,
//! `route_design`, `write_bitstream`, `write_checkpoint`, `create_clock`,
//! `report_timing`, `report_utilization`, `mark_debug`, `eco`,
//! `open_hw_manager`, `program_hw`, `report_die`, `report_featuremap`,
//! `write_hnf`, `hds::synth`, `hds::impl`, `hds::flow`, `hds::tree`.
//!
//! Deliberately NOT supported: `namespace`, `upvar`/`uplevel`, `trace`,
//! `regexp`/`regsub`, `dict`, coroutines, `after`, `package require`,
//! `rename` of builtins. Unknown commands raise Tcl-style errors.

pub mod design;
pub mod interp;
pub mod parse;

pub use interp::TclInterp;

/// An object recorded by `select_objects` / `highlight_objects`.
#[derive(Clone, Debug, PartialEq)]
pub struct SelObj {
    /// Object class: `cell`, `net`, `pin`, `port`, `clock`, `path`.
    pub kind: String,
    /// Hierarchical object name.
    pub name: String,
    /// Set by `highlight_objects` (vs plain `select_objects`).
    pub highlighted: bool,
}
