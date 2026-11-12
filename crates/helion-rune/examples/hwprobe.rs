//! Read-only hardware probe through the IDE's own Tcl verbs: scan the real
//! native FTDI JTAG chain, list devices, read the target's IDCODE with a raw
//! IR/DR scan, then release the cable. Nothing is programmed.
//! `cargo run -p helion-rune --example hwprobe -- [ir_idcode_hex]` (default 09, Xilinx 7-series)
fn main() {
    let ir = std::env::args().nth(1).unwrap_or_else(|| "09".into());
    let mut s = helion_rune::init();
    let cmds = [
        "open_hw_target -cable native".to_string(),
        "get_hw_devices".to_string(),
        format!("scan_ir_hw_jtag 6 -tdi {ir}"),
        "scan_dr_hw_jtag 32 -tdi 0".to_string(),
        "close_hw_target".to_string(),
    ];
    for c in &cmds {
        let before = s.console.lines.len();
        helion_rune::tcl::eval_command(&mut s, c);
        for l in &s.console.lines[before..] {
            println!("{}", l.text);
        }
    }
}
