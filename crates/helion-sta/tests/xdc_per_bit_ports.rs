//! The XDC -> netlist contract.
//!
//! `helion_sta::load_xdc` must preserve per-bit port names. It previously ran
//! every `get_ports {..}` argument through `tcl_name`, which splits on any
//! character that is not `[A-Za-z0-9_]` -- so `{led[0]}` and `{led[1]}` both
//! became `led`, and the map silently held one pin for the whole bus
//! (last-wins). Combined with `place`, which does not reject two IOBs on the
//! same site, that stacks every LED on one physical pin.

use std::collections::HashMap;

const XDC: &str = r#"
set_property PACKAGE_PIN T22 [get_ports {led[0]}]
set_property PACKAGE_PIN T21 [get_ports {led[1]}]
set_property PACKAGE_PIN U22 [get_ports {led[2]}]
set_property PACKAGE_PIN Y9  [get_ports clk]
"#;

#[test]
fn per_bit_ports_stay_distinct() {
    let c = helion_sta::load_xdc(XDC).expect("parse xdc");
    let got: HashMap<&str, &str> = c
        .package_pins
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    assert_eq!(
        got.get("led[0]").copied(),
        Some("T22"),
        "led[0] lost; whole map = {got:?}"
    );
    assert_eq!(got.get("led[1]").copied(), Some("T21"), "map = {got:?}");
    assert_eq!(got.get("led[2]").copied(), Some("U22"), "map = {got:?}");
    assert_eq!(got.get("clk").copied(), Some("Y9"), "map = {got:?}");
    assert_eq!(got.len(), 4, "expected 4 distinct ports, got {got:?}");
}

#[test]
fn whole_port_still_parses() {
    let c = helion_sta::load_xdc("set_property PACKAGE_PIN T22 [get_ports {led}]\n")
        .expect("parse xdc");
    assert_eq!(c.package_pins.get("led").map(String::as_str), Some("T22"));
}
