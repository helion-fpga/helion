use super::*;
use crate::state::CenterPane;
use crate::{app_spec, init};
use rune::{Env, Runtime};

fn harness_with(state: AppState) -> Runtime<AppState> {
    let spec = app_spec();
    Runtime::new(
        state,
        crate::view,
        spec.size,
        Env {
            system_dark: false,
            scale: 2.0,
        },
    )
}

fn settle(rt: &mut Runtime<AppState>) {
    for i in 1..=20 {
        rt.frame(0.1 + i as f64 / 60.0);
    }
}

fn tap(rt: &mut Runtime<AppState>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn ip_state() -> AppState {
    let mut s = init();
    s.center = CenterPane::Ip;
    s
}

#[test]
fn catalog_comes_from_helion_ipxact_and_marks_generator_cores() {
    let s = ip_state();
    let rows = s.ip.rows();
    assert_eq!(rows.len(), helion_ipxact::catalog().len());
    let uart = rows.iter().find(|r| r.name == "h_uart").expect("h_uart");
    assert_eq!(uart.bus, "Helion-MM");
    assert_eq!(uart.status, "Available");
    let rv64 = rows.iter().find(|r| r.name == "h_rv64gc").expect("rv64");
    assert_eq!(rv64.status, "Generator", "no RTL package for generator cores");
}

#[test]
fn add_is_disabled_for_generator_cores_and_until_selection() {
    let mut s = ip_state();
    assert!(add_block_reason(&s).is_some());
    select_ip(&mut s, "h_rv64gc").unwrap();
    assert!(add_block_reason(&s).unwrap().contains("generator-only"));
    assert!(create_bd_cell(&mut s, "h_rv64gc").is_err());
    select_ip(&mut s, "h_uart").unwrap();
    assert_eq!(add_block_reason(&s), None);
}

#[test]
fn automation_wires_an_interconnect_that_the_hdl_and_canvas_share() {
    let mut s = ip_state();
    create_bd_cell(&mut s, "h_rv32_hb1").unwrap();
    create_bd_cell(&mut s, "h_uart").unwrap();
    create_bd_cell(&mut s, "h_gpio").unwrap();
    let bd = s.ip.bd.as_ref().unwrap();
    assert!(bd.emit.as_ref().unwrap().interconnect.is_none());
    assert!(!drawing(&s.ip).symbols.iter().any(|y| y.kind == SymKind::Interconnect));
    assert!(automation_block_reason(&s).is_none());
    let msg = apply_automation(&mut s).unwrap();
    assert!(msg.contains("interconnect=design_1_mm_interconnect"), "{msg}");
    let bd = s.ip.bd.as_ref().unwrap();
    assert!(bd.sv().contains("design_1_mm_interconnect mm_interconnect ("));
    let d = drawing(&s.ip);
    let hub = d.symbols.iter().find(|y| y.kind == SymKind::Interconnect).expect("hub drawn");
    assert_eq!(hub.name, "mm_interconnect");
    assert!(d.wires.iter().any(|w| w.src == "h_rv32_hb1_0" && w.dst == "mm_interconnect"));
    assert!(d.wires.iter().any(|w| w.src == "mm_interconnect" && w.dst == "h_gpio_0"));
    assert!(
        d.wires.iter().all(|w| w.pts.windows(2).all(|p| p[0][0] == p[1][0] || p[0][1] == p[1][1])),
        "orthogonal wires"
    );
    assert_eq!(automation_block_reason(&s).as_deref(), Some("Every cell is already connected."));
    let rows = hdl_rows(&s);
    assert!(rows.iter().any(|r| r.module == "design_1_mm_interconnect"));
}

#[test]
fn customize_validates_and_changes_emitted_hdl() {
    let mut s = ip_state();
    create_bd_cell(&mut s, "h_rv32_hb1").unwrap();
    create_bd_cell(&mut s, "h_uart").unwrap();
    apply_automation(&mut s).unwrap();
    open_customize(&mut s, "h_rv32_hb1_0").unwrap();
    assert_eq!(s.dialogs.open, Some(crate::views::dialogs::DialogId::IpCustomize));
    let d = s.ip.customize.as_mut().unwrap();
    let mul = d.params.iter_mut().find(|(n, _)| n == "ENABLE_MUL").unwrap();
    mul.1 = rune::TextEdit::new("5");
    assert!(customize_error(&s).unwrap().contains("0 or 1"));
    assert!(apply_customize(&mut s).is_err());
    let d = s.ip.customize.as_mut().unwrap();
    d.params.iter_mut().find(|(n, _)| n == "ENABLE_MUL").unwrap().1 = rune::TextEdit::new("1");
    d.name = rune::TextEdit::new("cpu");
    assert_eq!(customize_error(&s), None);
    let msg = apply_customize(&mut s).unwrap();
    assert!(msg.contains("CONFIG.ENABLE_MUL=1") && msg.contains("name=cpu"), "{msg}");
    let sv = s.ip.bd.as_ref().unwrap().sv().to_string();
    assert!(sv.contains(".ENABLE_MUL(1)") && sv.contains(") cpu ("), "{sv}");
    open_customize(&mut s, "h_uart_0").unwrap();
    s.ip.customize.as_mut().unwrap().base = rune::TextEdit::new("0x40000800");
    assert!(customize_error(&s).unwrap().contains("4 KiB"));
}

#[test]
fn generate_bd_writes_top_and_adds_project_sources() {
    let mut s = ip_state();
    s.project.name = format!("bdgen-{}", std::process::id());
    create_bd_cell(&mut s, "h_uart").unwrap();
    apply_automation(&mut s).unwrap();
    let before = s.project_files.files.len();
    let msg = generate_bd(&mut s).unwrap();
    let g = s.ip.generated.get(BD_NAME).expect("recorded");
    let text = std::fs::read_to_string(&g.path).unwrap();
    assert!(text.contains("module design_1_mm_interconnect"), "{msg}");
    assert!(s.project_files.files.len() >= before + 2, "top + h_uart.v added");
    let _ = std::fs::remove_dir_all(gen_dir(&s));
}

#[test]
fn generate_writes_full_ipxact_to_disk() {
    let mut s = ip_state();
    s.project.name = format!("iptest-{}", std::process::id());
    select_ip(&mut s, "h_gpio").unwrap();
    let out = generate_ip(&mut s, "").expect("generate");
    let g = s.ip.generated.get("h_gpio").expect("recorded");
    let xml = std::fs::read_to_string(&g.path).expect("written");
    assert_eq!(xml.len(), g.bytes);
    assert!(xml.contains("<ipxact:name>h_gpio</ipxact:name>"));
    assert!(xml.contains("<ipxact:name>gpio_out</ipxact:name>"), "model ports exported");
    assert!(out.contains("xml_bytes="));
    assert_eq!(s.ip.status("h_gpio"), "Generated");
    let _ = std::fs::remove_dir_all(output_dir(&s, "h_gpio"));
}

#[test]
fn bd_tabs_canvas_and_dialogs_mount() {
    let mut s = ip_state();
    create_bd_cell(&mut s, "h_uart").unwrap();
    create_bd_cell(&mut s, "h_timer").unwrap();
    let mut rt = harness_with(s);
    rt.frame(0.0);
    for key in ["ip-bd", "ip-bd-summary", "ip-bd-tabs", "ip-bd-canvas", "ip-bd-sym-h_uart_0"] {
        assert!(rt.find(key).is_some(), "missing key: {key}");
    }
    assert!(rt.find("ip-bd-sym-mm_interconnect").is_none(), "no hub before automation");
    tap(&mut rt, "ip-automation");
    settle(&mut rt);
    assert!(rt.find("ip-auto-dialog").is_some());
    assert!(rt.find("ip-auto-step-0").is_some());
    tap(&mut rt, "ip-auto-ok");
    settle(&mut rt);
    assert!(rt.find("ip-auto-dialog").is_none());
    assert!(rt.find("ip-bd-sym-mm_interconnect").is_some());
    tap(&mut rt, "ip-tab-address");
    settle(&mut rt);
    assert!(rt.find("ip-addr-row-1").is_some());
    tap(&mut rt, "ip-tab-hdl");
    settle(&mut rt);
    tap(&mut rt, "ip-hdl-row-0");
    assert_eq!(rt.state.ip.selected_cell.as_deref(), Some("h_uart_0"), "tab={:?} dlg={:?}", rt.state.ip.tab, rt.state.dialogs.open);
}

#[test]
fn tcl_verbs_drive_the_model() {
    let mut s = init();
    crate::tcl::eval_command(&mut s, "ip_catalog");
    assert_eq!(s.center, CenterPane::Ip);
    crate::tcl::eval_command(&mut s, "select_ip h_timer");
    assert_eq!(s.ip.selected.as_deref(), Some("h_timer"));
    crate::tcl::eval_command(&mut s, "create_bd_cell");
    assert!(s.ip.in_bd("h_timer"));
    crate::tcl::eval_command(&mut s, "create_bd_cell h_rv32_hb1");
    crate::tcl::eval_command(&mut s, "apply_bd_automation");
    let last = s.console.lines.last().unwrap().text.clone();
    assert!(last.contains("apply_bd_automation cells=2"), "{last}");
    crate::tcl::eval_command(&mut s, "set_bd_cell_property h_rv32_hb1_0 CONFIG.ENABLE_DIV 1");
    assert!(s.ip.bd.as_ref().unwrap().sv().contains(".ENABLE_DIV(1)"));
    crate::tcl::eval_command(&mut s, "validate_bd_design");
    assert!(s.console.lines.iter().any(|l| l.text.starts_with("validate_bd_design design_1")));
    crate::tcl::eval_command(&mut s, "bd_hdl");
    assert!(s.console.lines.iter().any(|l| l.text.contains("INSTANCE=h_timer_0")));
    crate::tcl::eval_command(&mut s, "bd_drawing");
    assert!(s.console.lines.iter().any(|l| l.text.starts_with("drawing symbols=")));
    crate::tcl::eval_command(&mut s, "select_ip nope_core");
    assert!(s.console.lines.last().unwrap().text.contains("no IP"));
}
