//! Where does the time go on an Artix-35T-sized fabric?
use helion_xilinx7::build::PinConstraints;

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn db() -> std::path::PathBuf {
    std::env::var("HELION_XILINX_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../work/prjxray-db")
        })
}

#[test]
fn time_each_stage_on_a35t() {
    let mut design = helion_sv::synth_sv(
        &std::fs::read_to_string("/tmp/arty_blinky.sv").unwrap(),
        "arty.sv",
    )
    .unwrap();
    PinConstraints::parse(&std::fs::read_to_string("/tmp/arty_a7.xdc").unwrap()).apply(&mut design);

    let t = std::time::Instant::now();
    let mut device = helion_xilinx7::device::Device::load_part_from_db(&db(), "xc7a35t").unwrap();
    println!("device load: {:?}", t.elapsed());

    let t = std::time::Instant::now();
    let nm = helion_xilinx7::xdevice::build_node_map(&mut device);
    println!("node map: {:?}", t.elapsed());

    let t = std::time::Instant::now();
    let packed = helion_xilinx7::pack::pack(&design).unwrap();
    println!("pack: {:?} ({} luts, {} iobs)", t.elapsed(), packed.luts.len(), packed.iobs.len());

    let t = std::time::Instant::now();
    let mut xdev = helion_xilinx7::xdevice::XDevice::new(&device, nm);
    println!("xdevice: {:?}", t.elapsed());
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let t = std::time::Instant::now();
    let placed = helion_xilinx7::place::place(&packed, &xdev).unwrap();
    println!("PLACE: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let _routed = helion_xilinx7::route::route(&placed, &xdev).unwrap();
    println!("ROUTE: {:?}", t.elapsed());
}
