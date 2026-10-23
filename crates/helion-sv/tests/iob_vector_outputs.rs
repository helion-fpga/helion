//! A bit-selected output assignment must drive its pad from the RHS.
//!
//! `assign led[0] = cnt[26];` used to emit the IOB onto `led_0` -- the pad's
//! OWN net -- which nothing drives, so the pad was unconnected and the
//! assignment had no effect. examples/zedboard_blinky.sv produced eight IOBs
//! whose nets had exactly one endpoint each (the IOB itself).

fn pad_endpoint(d: &helion_ir::Design, iob: &str) -> String {
    d.nets
        .iter()
        .find(|n| n.endpoints.iter().any(|e| e.cell == iob && e.pin == "PAD"))
        .map(|n| n.name.clone())
        .unwrap_or_else(|| panic!("no PAD net for {iob}"))
}

fn input_net(d: &helion_ir::Design, iob: &str) -> String {
    d.nets
        .iter()
        .find(|n| n.endpoints.iter().any(|e| e.cell == iob && e.pin == "I"))
        .map(|n| n.name.clone())
        .unwrap_or_else(|| panic!("no I net for {iob}"))
}

#[test]
fn bit_selected_output_is_driven_by_its_rhs() {
    let d = helion_sv::synth_sv(
        r#"module v1(input clk, output [1:0] led);
             logic [2:0] cnt;
             always_ff @(posedge clk) cnt <= cnt + 3'b1;
             assign led[0] = cnt[2];
             assign led[1] = cnt[1];
           endmodule"#,
        "t.sv",
    )
    .expect("synth");

    let iobs: Vec<String> = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, helion_ir::CellKind::IobOut))
        .map(|c| c.name.clone())
        .collect();
    assert_eq!(iobs.len(), 2, "one IOB per output bit, got {iobs:?}");

    // Each pad's input net must be a real driver (has other endpoints too),
    // not the pad's own net.
    for iob in &iobs {
        let pad = pad_endpoint(&d, iob);
        let i = input_net(&d, iob);
        assert_ne!(i, pad, "{iob}: pad's I pin is shorted to its PAD net");
        let eps = d
            .nets
            .iter()
            .find(|n| n.name == i)
            .map(|n| n.endpoints.len())
            .unwrap_or(0);
        assert!(
            eps >= 2,
            "{iob}: driving net `{i}` has {eps} endpoint(s); it must be driven"
        );
    }
}

#[test]
fn every_output_bit_gets_its_own_pad() {
    let d = helion_sv::synth_sv(
        r#"module v2(input clk, output [3:0] q);
             logic [7:0] c;
             always_ff @(posedge clk) c <= c + 8'h1;
             assign q[0]=c[4]; assign q[1]=c[5];
             assign q[2]=c[6]; assign q[3]=c[7];
           endmodule"#,
        "t.sv",
    )
    .expect("synth");
    let n = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, helion_ir::CellKind::IobOut))
        .count();
    assert_eq!(n, 4, "each of the 4 output bits needs its own IOB");
    // No two pads may share one driving net.
    let mut drives: Vec<String> = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, helion_ir::CellKind::IobOut))
        .map(|c| input_net(&d, &c.name))
        .collect();
    drives.sort();
    let before = drives.len();
    drives.dedup();
    assert_eq!(before, drives.len(), "two pads share one driver: {drives:?}");
}

#[test]
fn scalar_initial_is_not_a_memory() {
    // `mem_inits` also records a scalar's power-up value. Treating every such
    // key as an array made ysyx_ibex.sv fail on `rst_sys_n`, a single wire.
    helion_sv::synth_sv(
        r#"module v3(output reg r);
             logic s;
             initial begin s = 1'b0; #8 s = 1'b1; end
             always @* r = s;
           endmodule"#,
        "t.sv",
    )
    .expect("a scalar with an initial value is not a memory");
}
