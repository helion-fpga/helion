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

    let mut c = IdeModel::new();
    c.open_source(&example("counter.sv")).unwrap();
    c.implement().unwrap();
    assert_eq!(c.wns_ps(), Some(9640), "counter gold WNS");
}
