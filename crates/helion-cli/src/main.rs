use helion_bits::{bitgen, Bitstream};
use helion_device::Device;
use helion_fabric::Fabric;
use helion_hw::{prog_sim, Tap};
use helion_ir::Design;
use helion_pack::pack;
use helion_place::place;
use helion_route::route;

fn main() {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "--help".into());
    match cmd.as_str() {
        "--version" | "-V" | "version" => {
            println!("helion {}", env!("CARGO_PKG_VERSION"));
        }
        "doctor" => doctor(),
        "hw" => hw(args.collect()),
        "--help" | "-h" | "help" => {
            eprintln!(
                "helion {v}\n  helion --version\n  helion doctor\n  helion hw program --cable sim",
                v = env!("CARGO_PKG_VERSION")
            );
        }
        other => {
            eprintln!("unknown command {other}");
            std::process::exit(2);
        }
    }
}

fn hw(args: Vec<String>) {
    let mut it = args.into_iter();
    let sub = it.next().unwrap_or_default();
    let mut cable = String::new();
    while let Some(a) = it.next() {
        if a == "--cable" {
            cable = it.next().unwrap_or_default();
        }
    }
    if sub != "program" || cable != "sim" {
        eprintln!("usage: helion hw program --cable sim");
        std::process::exit(2);
    }
    let dev = Device::load_part("HL10T-C32-1").expect("HAD");
    let st = prog_sim(&dev, &Bitstream::empty(&dev)).expect("prog");
    println!(
        "hw sim STAT INIT={} DONE={} EOS={} GWE={} GSR={} GTS={} CRC_ERR={}",
        st.init as u8,
        st.done as u8,
        st.eos as u8,
        st.gwe as u8,
        st.gsr as u8,
        st.gts as u8,
        st.crc_err as u8
    );
}

fn doctor() {
    println!("helion doctor");
    let dev = Device::load_part("HL10T-C32-1").expect("HAD");
    println!(
        "  HAD {}: {}×{} CLB, {} LUT6, idcode {:#010x}",
        dev.part,
        dev.interior_cols,
        dev.interior_rows,
        dev.lut6_count(),
        dev.idcode
    );
    let loc = dev.locate("CLB_X2Y1.BLE0.LUT.INIT[0]").unwrap();
    let frac = dev.locate("CLB_X2Y1.BLE0.LUT.FRACTURE").unwrap();
    println!(
        "  FeatureMap INIT[0] minor {} bit {}; FRACTURE minor {} bit {}",
        loc.far.minor, loc.bit, frac.far.minor, frac.bit
    );
    let mut tap = Tap::new(&dev);
    let st = tap.program(&Bitstream::empty(&dev)).unwrap();
    println!(
        "  TAP empty STAT INIT={} DONE={} EOS={} GWE={} GSR={} GTS={} CRC_ERR={}",
        st.init as u8,
        st.done as u8,
        st.eos as u8,
        st.gwe as u8,
        st.gsr as u8,
        st.gts as u8,
        st.crc_err as u8
    );
    let packed = pack(&Design::structural_blinky(), &dev).unwrap();
    let placed = place(&packed, &dev).unwrap();
    let routed = route(&placed, &dev).unwrap();
    let _ = bitgen(&dev, &routed).unwrap();
    let _ = Fabric::new(&dev);
    println!("  blinky pack/place/route/bitgen: ok");
    println!("ok");
}
