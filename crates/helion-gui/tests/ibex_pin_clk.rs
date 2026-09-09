use helion_gui::IdeModel;
use std::path::PathBuf;

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

#[test]
fn ibex_pin_wrap_user_sdc_closed_wns_or_named_miss() {
    let mut ide = IdeModel::new();
    ide.open_source(&example("ibex_pin_wrap.prj"))
        .expect("open ibex_pin_wrap.prj");
    let impl_r = ide.implement();
    eprintln!("ibex implement={impl_r:?}");
    impl_r.expect("implement ibex_pin_wrap");
    assert!(ide.user_sdc, "ibex.sdc must load (user clock)");

    let cells = ide.tree.cells.len();
    let util = ide.utilization_report();
    let lutff = util
        .occupancy
        .iter()
        .find(|r| r.resource == "LUTFF")
        .map(|r| r.used)
        .unwrap_or(0);
    let wns = ide.wns_ps();
    eprintln!("IBEX cells={cells} LUTFF={lutff} WNS_PS={wns:?}");

    // Honest: wrap heartbeat path should close WNS, or surface a named miss — never fake WNS on cells=0.
    assert!(cells > 0, "fabric cells (not empty / not fake WNS)");
    assert!(lutff > 8, "mid-core LUTFF, not counter clone: {lutff}");
    let wns = wns.expect("WNS present after implement with user SDC");
    assert!(wns > 0, "closed honest WNS on wrap heartbeat / user clock");

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

    let mut c = IdeModel::new();
    c.open_source(&example("counter.sv")).unwrap();
    c.implement().unwrap();
    assert_eq!(c.wns_ps(), Some(9640), "counter gold WNS");
}
