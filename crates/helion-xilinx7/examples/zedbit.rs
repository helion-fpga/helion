//! Build a 7-series bitstream from HDL with Helion's own flow: synthesis
//! (SystemVerilog or VHDL) -> pack -> place -> route -> FASM -> `.bit`.
//!
//! ```text
//! cargo run --release -p helion-xilinx7 --example zedbit -- \
//!     design.sv[,more.sv...] pins.xdc out.bit [top] [--board=NAME | --part=PART]
//!     [--front=auto|v2|zb] [--ignore-timing] [--vivado=PREFIX [--vivado-only]]
//! ```
//!
//! The part is the ZedBoard's (`xc7z020clg484-1`) unless `--board` names a
//! board (`zedboard`, `arty-a7-35`, `basys3`, `nexys-a7-100t`; its pins are in
//! `boards/<name>.xdc`) or `--part` names any part the Project X-Ray database
//! describes (see `helion_xilinx7::parts`).
//!
//! Front ends (`--front`, or `HELION_ZED_FRONT`):
//!
//! * `v2`: Helion's current SystemVerilog front end
//!   (`helion_sv::synth_sv_v2_files`). VHDL is translated to SystemVerilog
//!   first. Arithmetic is LUT logic; there is no carry-chain inference yet.
//!   Memories with registered reads become block RAM and instances of
//!   device primitives (`RAMB36E1`, `DSP48E1`, `MMCME2_ADV`, ...) pass
//!   through (`helion_sv::v2::set_hard`; `HELION_V2_HARD=0` turns it off).
//! * `zb`: the front end this flow first ran on the board with
//!   (`helion-zb-front`), which puts adders on `CARRY4` chains.
//! * `auto` (default): `v2`, and `zb` only for a design `v2` refuses; the
//!   switch is announced.
//!
//! Pins come from the XDC (`PACKAGE_PIN`, `IOSTANDARD`, including `-dict`
//! form and multi-port / wildcard `get_ports`). Every top-level port bit
//! must have a pin; a missing pin is an error, never a silent default.
//! The FASM is written next to the bitstream (`out.fasm`).
//!
//! `--vivado=PREFIX` also writes Helion's placed design for Vivado to time
//! (`PREFIX.v`, `.xdc`, `.tcl`, `_noroute.tcl`; see
//! `helion_xilinx7::vivado_export`); `--vivado-only` stops after that,
//! before routing.

//!
//! The flow itself is `helion_xilinx7::xdc_flow`, shared with the IDE.

use helion_xilinx7::xdc_flow;

fn main() {
    let all: Vec<String> = std::env::args().collect();
    let ignore_timing = all.iter().any(|a| a == "--ignore-timing");
    let front = all
        .iter()
        .find_map(|a| a.strip_prefix("--front=").map(str::to_string))
        .or_else(|| std::env::var("HELION_ZED_FRONT").ok())
        .unwrap_or_else(|| "auto".to_string());
    let opt = |k: &str| all.iter().find_map(|a| a.strip_prefix(k).map(str::to_string));
    let part = match (opt("--board="), opt("--part=")) {
        (Some(_), Some(_)) => {
            eprintln!("zedbit: give --board or --part, not both");
            std::process::exit(2);
        }
        (Some(b), None) => match helion_xilinx7::parts::board(&b) {
            Ok(b) => b.part.to_string(),
            Err(e) => {
                eprintln!("zedbit: {e}");
                std::process::exit(2);
            }
        },
        (None, Some(p)) => p,
        (None, None) => "xc7z020clg484-1".to_string(),
    };
    let vivado = opt("--vivado=").map(|p| (p, all.iter().any(|a| a == "--vivado-only")));
    let args: Vec<String> = all.into_iter().filter(|a| !a.starts_with("--")).collect();
    if args.len() < 4 {
        eprintln!("usage: zedbit <design.vhd|.sv[,more...]> <pins.xdc> <out.bit> [top] [--board=NAME | --part=PART] [--front=auto|v2|zb] [--ignore-timing]");
        std::process::exit(2);
    }
    let mut log = |l: &str| eprintln!("{l}");
    match xdc_flow::run(&args[1], &args[2], &args[3], args.get(4).map(|s| s.as_str()), ignore_timing, &front, &part, vivado, &mut log) {
        Ok(Some(b)) => {
            println!("zedbit: summary {}", b.summary);
            println!("{}", b.done);
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("zedbit: {e}");
            std::process::exit(1);
        }
    }
}
