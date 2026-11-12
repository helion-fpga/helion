//! Assemble a FASM file into a 7-series `.bit` with Helion's own encoder.
//!
//! This is the encoder half of the golden round-trip check: decode a Vivado
//! bitstream to FASM (tools/x7v), assemble it here, and diff the result
//! against the Vivado original. Lines the encoder rejects are reported, not
//! silently dropped.
//!
//! ```text
//! cargo run --release -p helion-xilinx7 --example fasm2bit -- in.fasm out.bit [xc7z020clg484-1]
//! ```

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: fasm2bit <in.fasm> <out.bit> [part]");
        std::process::exit(2);
    }
    let part = args.get(3).map(String::as_str).unwrap_or("xc7z020clg484-1");
    let text = std::fs::read_to_string(&args[1]).expect("read fasm");
    let r = helion_xilinx7::bitgen::fasm_to_bit(&text, part, "fasm2bit").expect("fasm2bit");
    for e in &r.rejected {
        eprintln!("REJECT {e}");
    }
    std::fs::write(&args[2], &r.bit).expect("write out");
    println!(
        "applied={} rejected_lines={} by_tile_type={:?} bytes={}",
        r.applied,
        r.rejected.len(),
        r.rejected_by_tile_type,
        r.bit.len()
    );
}
