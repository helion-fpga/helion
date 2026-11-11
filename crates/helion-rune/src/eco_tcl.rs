//! Tcl verbs for the IP / HLS / board / hand-off surfaces (registered in
//! `tcl::COMMANDS`, dispatched from `tcl::eval_command`). Each verb maps to
//! the same model action the UI control runs.

use crate::state::AppState;
use crate::views::ip_model as ip;

/// Verbs this module owns.
pub const VERBS: &[&str] = &[
    "apply_bd_automation",
    "assign_bd_address",
    "validate_bd_design",
    "set_bd_cell_property",
    "select_bd_cell",
    "delete_bd_objs",
    "generate_bd",
    "package_ip",
    "csynth_design",
    "cosim_design",
    "open_hls",
    "add_hls_rtl",
];

/// One-line usage + description for `help <verb>`.
pub fn help(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "apply_bd_automation" => (
            "apply_bd_automation",
            "Helion-MM connection automation: clock/reset, interconnect, addresses, external pins",
        ),
        "assign_bd_address" => ("assign_bd_address", "map unassigned Helion-MM slaves (4 KiB apertures)"),
        "validate_bd_design" => ("validate_bd_design", "block-design DRC (errors and warnings)"),
        "set_bd_cell_property" => (
            "set_bd_cell_property cell CONFIG.NAME value",
            "set one IP-XACT parameter on a block-design cell (validated)",
        ),
        "select_bd_cell" => ("select_bd_cell cell", "select a block-design cell"),
        "delete_bd_objs" => ("delete_bd_objs ?cell?", "remove a cell (default: the selected one)"),
        "generate_bd" => (
            "generate_bd",
            "write the block design top + interconnect and add it to the project",
        ),
        "package_ip" => (
            "package_ip file top ?vendor library version?",
            "package a module into the project's user IP repository",
        ),
        "csynth_design" => ("csynth_design ?-clock ns?", "C synthesis of the HLS kernel (schedule, bind, emit RTL)"),
        "cosim_design" => (
            "cosim_design ?-vectors {a=1 b=2;a=3 b=4}?",
            "C/RTL co-simulation: the synthesized netlist in helion-sim vs the C model",
        ),
        "open_hls" => ("open_hls file.cpp", "open a C/C++ kernel in the HLS pane"),
        "add_hls_rtl" => ("add_hls_rtl", "write the generated SystemVerilog and add it to the project"),
        _ => return None,
    })
}

fn arg(args: &[String], i: usize) -> &str {
    args.get(i).map(|s| s.as_str()).unwrap_or("")
}

pub fn tcl_verb(s: &mut AppState, name: &str, args: &[String]) -> Result<String, String> {
    match name {
        "apply_bd_automation" => {
            ip::ensure_bd(s);
            ip::apply_automation(s)
        }
        "assign_bd_address" => ip::assign_addresses(s),
        "validate_bd_design" => ip::validate_text(s),
        "set_bd_cell_property" => {
            if args.len() < 3 {
                return Err("usage: set_bd_cell_property cell CONFIG.NAME value".into());
            }
            ip::set_cell_param(s, arg(args, 0), arg(args, 1), arg(args, 2))
        }
        "select_bd_cell" => ip::select_cell(s, arg(args, 0)),
        "delete_bd_objs" => ip::delete_bd_cell(s, arg(args, 0)),
        "generate_bd" => ip::generate_bd(s),
        "package_ip" => {
            if args.len() < 2 {
                return Err("usage: package_ip file top ?vendor library version?".into());
            }
            let model = helion_ipxact::package_user_ip(
                &helion_ipxact::PackageRequest {
                    source: std::path::PathBuf::from(arg(args, 0)),
                    top: arg(args, 1).to_string(),
                    vendor: args.get(2).cloned().unwrap_or_else(|| "user".into()),
                    library: args.get(3).cloned().unwrap_or_else(|| "ip".into()),
                    version: args.get(4).cloned().unwrap_or_else(|| "1.0".into()),
                },
                &ip::user_repo_dir(s),
            )?;
            ip::refresh_catalog(s);
            s.ip.selected = Some(model.core.name.clone());
            Ok(format!("package_ip {} bus={}", model.core.vlnv(), model.core.bus))
        }
        "csynth_design" | "cosim_design" | "open_hls" | "add_hls_rtl" => {
            crate::views::hls::tcl_verb(s, name, args)
        }
        _ => Err(format!("{name}: not an ecosystem verb")),
    }
}
