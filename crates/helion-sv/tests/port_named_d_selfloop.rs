//! A design whose input port is named `d` used to synthesize a broken netlist.
//!
//! When the module's only register is called `q`, the single-register path
//! hardcoded the FF's data net as the literal `"d"`. If the design also had an
//! input port named `d`, that net and the port collided: the buffer LUT driving
//! the FF both read and drove `d` (`u_lut.I0 = d` and `u_lut.O = d`), so the LUT
//! was a self-loop and the port itself had no endpoint. `always_ff @(posedge
//! clk) q <= d;` was enough to trigger it, and it showed up as
//! `route: net `d` has 2 drivers (need exactly 1)`.

fn endpoints_of(d: &helion_ir::Design, net: &str) -> Vec<(String, String)> {
    d.nets
        .iter()
        .find(|n| n.name == net)
        .map(|n| {
            n.endpoints
                .iter()
                .map(|e| (e.cell.clone(), e.pin.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// No cell may both drive and read the same net.
fn assert_no_self_loop(d: &helion_ir::Design) {
    for n in &d.nets {
        let drivers: Vec<&str> = n
            .endpoints
            .iter()
            .filter(|e| e.pin == "O" || e.pin == "Q")
            .map(|e| e.cell.as_str())
            .collect();
        for e in &n.endpoints {
            if e.pin.starts_with('I') || e.pin == "D" {
                assert!(
                    !drivers.contains(&e.cell.as_str()),
                    "cell `{}` both drives and reads net `{}` (self-loop)",
                    e.cell,
                    n.name
                );
            }
        }
    }
}

#[test]
fn input_port_named_d_is_not_reused_as_the_ff_data_net() {
    let d = helion_sv::synth_sv(
        "module m(input logic clk, input logic d, output logic q);\n\
           always_ff @(posedge clk) q <= d;\n\
         endmodule",
        "t.sv",
    )
    .unwrap();
    assert_no_self_loop(&d);
    // The port must actually reach something, or nothing drives the FF.
    let eps = endpoints_of(&d, "d");
    assert!(!eps.is_empty(), "input port `d` has no endpoint: {eps:?}");
}

#[test]
fn input_port_named_din_is_unaffected() {
    let d = helion_sv::synth_sv(
        "module m(input logic clk, input logic din, output logic q);\n\
           always_ff @(posedge clk) q <= din;\n\
         endmodule",
        "t.sv",
    )
    .unwrap();
    assert_no_self_loop(&d);
}

#[test]
fn ports_named_d_and_q_do_not_collide() {
    // Both the register and the ports are named after the hardcoded literals.
    let d = helion_sv::synth_sv(
        "module m(input logic clk, input logic d, input logic r, output logic q);\n\
           always_ff @(posedge clk) q <= d;\n\
           always_ff @(posedge clk) r <= ~d;\n\
         endmodule",
        "t.sv",
    )
    .unwrap();
    assert_no_self_loop(&d);
    for net in ["d", "r"] {
        assert!(
            !endpoints_of(&d, net).is_empty(),
            "input port `{net}` has no endpoint"
        );
    }
}

#[test]
fn every_net_has_at_most_one_driver() {
    // The general form of the same defect: two drivers on one net.
    let d = helion_sv::synth_sv(
        "module m(input logic clk, input logic d, output logic q);\n\
           always_ff @(posedge clk) q <= d;\n\
         endmodule",
        "t.sv",
    )
    .unwrap();
    for n in &d.nets {
        let drivers = n
            .endpoints
            .iter()
            .filter(|e| e.pin == "O" || e.pin == "Q")
            .count();
        assert!(
            drivers <= 1,
            "net `{}` has {drivers} drivers, expected at most 1",
            n.name
        );
    }
}
