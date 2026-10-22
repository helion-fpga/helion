//! End-to-end smoke test: trivial counter netlist through the full 7-series
//! flow for `xc7z020` (pack, place, route, FASM emit).
//!
//! Requires the X-Ray zynq7 DB at `~/workspace/xilinx-dbsrc` (or
//! `HELION_XILINX_DB`). LOC pins are real clg484 PL pins (banks 35).

use helion_xilinx7::flow::run_for_part;

#[test]
fn z020_flow_smoke() {
    let mut netlist = helion_ir::Design::structural_counter();
    for port in &mut netlist.ports {
        match port.name.as_str() {
            "clk" => port.attrs.set("LOC", "A16"),
            "led" => port.attrs.set("LOC", "A17"),
            _ => {}
        }
    }

    let bs = run_for_part(&netlist, "xc7z020").expect("xc7z020 flow run");
    assert!(!bs.fasm.is_empty(), "empty FASM");
    let text = bs.fasm.join("\n");
    assert!(
        text.contains(".ALUT.INIT[63:0]"),
        "missing LUT INIT:\n{text}"
    );
    assert!(
        text.contains(".AFF.ZINI") || text.contains("FF.ZINI"),
        "missing FF:\n{text}"
    );
    // Routed nets must reference real z020 tiles.
    assert!(
        text.lines().any(|l| l.starts_with("INT_L_X") || l.starts_with("INT_R_X")),
        "no INT tile FASM lines"
    );
}
