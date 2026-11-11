//! Read-only hardware check for the native FTDI MPSSE JTAG engine.
//!
//! Opens the first MPSSE FTDI cable, scans the chain and reads each device's
//! IDCODE. Nothing is programmed or erased.
//!
//!     cargo run -p helion-hw --features usb-native --example native_scan [tck_hz] [channel]

#[cfg(feature = "usb-native")]
fn main() {
    use helion_hw::jtag_engine::JtagEngine;
    use helion_hw::usb_ftdi::open_usb_ftdi;

    let mut args = std::env::args().skip(1);
    let tck: u32 = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(6_000_000);
    let ch: char = args.next().and_then(|a| a.chars().next()).unwrap_or('A');

    let port = match open_usb_ftdi(None, ch) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("open: {e}");
            std::process::exit(1);
        }
    };
    println!("cable: {}  [pins: {}]", port.detail, port.pins.name);
    let mut eng = match JtagEngine::open(port, tck) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("engine: {e}");
            std::process::exit(1);
        }
    };
    let chain = eng.chain().to_vec();
    println!("chain: {} device(s) at {tck} Hz", chain.len());
    for (i, d) in chain.iter().enumerate() {
        match d.idcode {
            Some(id) => println!("  [{i}] idcode {id:#010x}  ir {}  {}", d.ir_len, d.name),
            None => println!("  [{i}] (bypass)  ir {}  {}", d.ir_len, d.name),
        }
    }
    // Re-read each IDCODE through IR (generic 1149.1 IDCODE is not the Helion
    // IR_IDCODE on foreign parts, so only the scan result is authoritative).
    let rescan = eng.scan_chain().map(|c| c.to_vec());
    match rescan {
        Ok(c) if c == chain => println!("rescan: stable"),
        Ok(c) => println!("rescan: DIFFERENT {c:?}"),
        Err(e) => println!("rescan: {e}"),
    }
    // Xilinx 7-series IDCODE instruction (0x09, IR 6) through the PL TAP,
    // with every other device in BYPASS.
    if let Some(pl) = chain.iter().position(|d| d.ir_len == 6) {
        let r = eng
            .select_target(pl)
            .and_then(|_| eng.shift_ir(0x09))
            .and_then(|cap| {
                // 7-series IR capture: bit5 DONE, bit4 INIT, bit3 ISC_ENABLED, bit2 ISC_DONE, 01.
                println!(
                    "PL IR capture {cap:#08b}: DONE={} INIT={} ISC_ENABLED={} ISC_DONE={}",
                    cap >> 5 & 1,
                    cap >> 4 & 1,
                    cap >> 3 & 1,
                    cap >> 2 & 1
                );
                eng.shift_dr_u32(0)
            });
        match r {
            Ok(id) => println!("PL IDCODE via IR 0x09: {id:#010x}"),
            Err(e) => println!("PL IDCODE via IR 0x09: {e}"),
        }
    }
    // The chain must read the same at every TCK we offer.
    for hz in [1_000_000u32, 15_000_000, 30_000_000] {
        let r = eng
            .set_tck_hz(hz)
            .and_then(|got| eng.scan_chain().map(|c| (got, c.to_vec())));
        match r {
            Ok((got, c)) => println!(
                "scan @ {got} Hz: {}",
                if c == chain {
                    "same chain".to_string()
                } else {
                    format!("DIFFERENT {c:?}")
                }
            ),
            Err(e) => println!("scan @ {hz} Hz: {e}"),
        }
    }
}

#[cfg(not(feature = "usb-native"))]
fn main() {
    eprintln!("build with --features usb-native");
}
