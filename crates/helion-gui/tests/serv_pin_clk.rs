use helion_gui::IdeModel;
use std::path::PathBuf;

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

#[test]
fn serv_pin_wrap_clk_package_pin_and_ila() {
    let mut ide = IdeModel::new();
    ide.open_source(&example("serv.prj"))
        .expect("open serv.prj");
    ide.implement().expect("implement serv");
    let wns = ide.wns_ps().expect("WNS");
    eprintln!("SERV WNS_PS={wns}");
    assert!(wns > 0, "closed WNS");

    let clk = ide
        .io_port_rows()
        .iter()
        .find(|p| p.name == "clk")
        .cloned()
        .expect("clk port");
    assert_eq!(clk.package_pin.as_deref(), Some("IOB_X3Y0"), "{clk:?}");
    assert_eq!(
        clk.site.as_deref(),
        Some("IOB_X3Y0"),
        "clk must place on PACKAGE_PIN: {clk:?}"
    );
    assert!(
        ide.device
            .sites
            .iter()
            .any(|s| s.occupant.as_deref() == Some("clk")),
        "Device must show clk occupant"
    );

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

    ide.exec("sim_run 16").unwrap();
    ide.exec(&format!("mark_debug {probe}")).unwrap();
    ide.exec("add_probe").unwrap();
    let prog = ide.exec("program_hw cable=sim").unwrap();
    assert!(
        prog.contains("DONE=1") || prog.contains("backend=sim"),
        "{prog}"
    );
    ide.exec("ila depth 16").unwrap();
    ide.exec("ila trigger rising").unwrap();
    let arm = ide.exec("ila arm").unwrap();
    eprintln!("ila_arm={arm}");
    let cap = ide.ila.capture.as_ref().expect("unified ILA capture");
    let bits: String = cap.samples.iter().map(|s| if s[0] == 1 { '1' } else { '0' }).collect();
    eprintln!("ila_bits={bits}");
    assert!(
        bits.contains('0') && bits.contains('1'),
        "filled ILA bits: {bits}"
    );

    let mut c = IdeModel::new();
    c.open_source(&example("counter.sv")).unwrap();
    c.implement().unwrap();
    assert_eq!(c.wns_ps(), Some(9640), "counter gold WNS");
}
