//! Synthesize SystemVerilog with Helion's front end and print the netlist
//! as HNF text. Used by `tools/svcheck` to compare the netlist with the RTL.
//!
//! ```text
//! cargo run --release -p helion-cli --example hnf -- design.sv > design.hnf
//! ```

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 2 {
        eprintln!("usage: hnf <design.sv>");
        std::process::exit(2);
    }
    match helion_sv::synth_sv_path(std::path::Path::new(&a[1])) {
        Ok(d) => print!("{}", d.to_hnf()),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
