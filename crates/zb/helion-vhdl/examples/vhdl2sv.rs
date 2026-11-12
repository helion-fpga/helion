//! Print the SystemVerilog Helion's VHDL front end produces.
//!
//! ```text
//! cargo run --release -p helion-vhdl --example vhdl2sv -- design.vhd [top]
//! ```
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = std::fs::read_to_string(&args[1]).expect("read");
    match helion_vhdl::v2::translate(&src, args.get(2).map(|s| s.as_str())) {
        Ok(sv) => print!("{sv}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
