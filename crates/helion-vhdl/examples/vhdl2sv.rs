//! Print the SystemVerilog Helion translates a VHDL file to.
fn main() {
    let p = std::env::args().nth(1).expect("file.vhd");
    let src = std::fs::read_to_string(&p).unwrap();
    match helion_vhdl::vhdl_to_sv(&src) {
        Ok(sv) => print!("{sv}"),
        Err(e) => { eprintln!("error: {e}"); std::process::exit(1); }
    }
}
