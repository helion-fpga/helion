//! Synthesize SystemVerilog with Helion's front end and print the netlist
//! as HNF text. Used by `tools/svcheck` to compare the netlist with the RTL.
//!
//! ```text
//! cargo run --release -p helion-cli --example hnf -- [--legacy] design.sv [top] > design.hnf
//! ```

fn main() {
    let all: Vec<String> = std::env::args().collect();
    let legacy = all.iter().any(|a| a == "--legacy");
    let a: Vec<&String> = all.iter().skip(1).filter(|a| !a.starts_with("--")).collect();
    if a.is_empty() {
        eprintln!("usage: hnf [--legacy] <design.sv> [top]");
        std::process::exit(2);
    }
    let path = std::path::Path::new(a[0].as_str());
    let r = if legacy {
        helion_sv::synth_sv_path(path)
    } else {
        helion_sv::synth_sv_v2_path(path, a.get(1).map(|s| s.as_str())).map(|s| {
            for w in &s.warnings {
                eprintln!("warning: {w}");
            }
            eprintln!("synth: {} LUTs, {} flops, {} LUT levels", s.luts, s.ffs, s.depth);
            s.design
        })
    };
    match r {
        Ok(d) => print!("{}", d.to_hnf()),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
