use helion_gui::IdeModel;
use std::path::PathBuf;

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

#[test]
fn pico_pin_wrap_user_sdc_closed_wns() {
    let mut ide = IdeModel::new();
    ide.open_source(&example("pico.prj")).expect("open pico.prj");
    ide.implement().expect("implement pico");
    let wns = ide.wns_ps().expect("WNS");
    let util = ide.utilization_report();
    let lutff = util
        .occupancy
        .iter()
        .find(|r| r.resource == "LUTFF")
        .map(|r| r.used)
        .unwrap_or(0);
    let cells = ide.tree.cells.len();
    eprintln!("PICO cells={cells} LUTFF={lutff} WNS_PS={wns}");
    assert!(cells > 0, "fabric cells (not empty)");
    assert!(lutff > 8, "mid-core LUTFF, not counter clone: {lutff}");
    assert!(wns > 0, "closed honest WNS on user clock");

    let clk = ide
        .io_port_rows()
        .iter()
        .find(|p| p.name == "clk")
        .cloned()
        .expect("clk port");
    assert_eq!(clk.package_pin.as_deref(), Some("IOB_X3Y0"), "{clk:?}");

    let led = ide
        .io_port_rows()
        .iter()
        .find(|p| p.name == "led")
        .cloned()
        .expect("led");
    assert_eq!(led.package_pin.as_deref(), Some("IOB_X2Y0"), "{led:?}");

    let probe = ide.default_ila_probe();
    eprintln!("default_ila_probe={probe}");
    assert!(!probe.is_empty(), "mark_debug hb should yield a probe");
    assert_eq!(probe, "hb_0", "ILA probe is bit-blasted hb_0");

    // add_wave must accept the ILA/RTL Q-net name (not only sim FF cell u_ff0).
    let aw = ide.exec("add_wave hb_0").expect("add_wave hb_0 after implement");
    assert!(aw.contains("add_wave hb_0"), "{aw}");
    assert!(ide.wave.has_trace("hb_0"), "wave has hb_0");
    let bus_miss = ide.exec("add_wave hb").expect_err("bus root must diagnose bit-blast");
    assert!(
        bus_miss.contains("bit-blast") && bus_miss.contains("hb_0"),
        "{bus_miss}"
    );

    // Pattern after SERV: Simulate + Mark/Probe + Program sim + ILA filled bits.
    ide.exec("sim_run 16").unwrap();
    assert!(
        ide.objects.iter().any(|o| o.name == "hb_0"),
        "Objects must list hb_0 after sim: {:?}",
        ide.objects.iter().map(|o| o.name.clone()).collect::<Vec<_>>()
    );
    let bits = ide.wave.bits_of("hb_0").expect("hb_0 samples after sim_run");
    eprintln!("hb_0 wave bits={bits}");
    assert!(
        bits.contains('0') && bits.contains('1'),
        "hb_0 must toggle on Simulate: {bits}"
    );

    ide.exec(&format!("mark_debug {probe}")).unwrap();
    ide.exec("add_probe").unwrap();
    let prog = ide.exec("program_hw").unwrap();
    assert!(
        prog.contains("DONE=1") || prog.contains("backend=sim") || prog.contains("soft-hold"),
        "{prog}"
    );
    ide.exec("ila_window 16").unwrap();
    ide.exec("ila_trigger rising").unwrap();
    let arm = ide.exec("ila_arm").unwrap();
    eprintln!("ila_arm={arm}");
    eprintln!("ila_bits={}", ide.ila.bits);
    assert!(
        ide.ila.bits.contains('0') && ide.ila.bits.contains('1'),
        "filled ILA bits: {}",
        ide.ila.bits
    );

    let mut c = IdeModel::new();
    c.open_source(&example("counter.sv")).unwrap();
    c.implement().unwrap();
    assert_eq!(c.wns_ps(), Some(9640), "counter gold WNS");
}

#[test]
fn pico_add_wave_hb0_after_mark_debug_implement() {
    let mut ide = IdeModel::new();
    ide.open_source(&example("pico.prj")).expect("open");
    ide.implement().expect("implement");
    assert_eq!(ide.default_ila_probe(), "hb_0");
    ide.exec("mark_debug hb_0").unwrap();
    let aw = ide.exec("add_wave hb_0").unwrap();
    assert!(aw.contains("add_wave hb_0"), "{aw}");
    ide.exec("sim_run 8").unwrap();
    assert!(ide.wave.has_trace("hb_0"));
    let bits = ide.wave.bits_of("hb_0").unwrap();
    assert!(
        bits.contains('0') && bits.contains('1'),
        "add_wave hb_0 visible on Simulate: {bits}"
    );
}
