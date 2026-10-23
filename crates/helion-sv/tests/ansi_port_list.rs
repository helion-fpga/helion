//! ANSI port-list parsing: comma-continued names must inherit the
//! direction, packed range and signedness of the declaration they follow.
//!
//! Regression for a silent miscompilation: only the first name after
//! `input`/`output` was parsed, so `input [3:0] a, b` made `b` a 1-bit input
//! of the default direction. The 4-bit operand then collapsed into a single
//! net broadcast to every bit slice -- wrong hardware, no diagnostic.

fn net<'a>(name: &str, d: &'a helion_ir::Design) -> Option<&'a helion_ir::Net> {
    d.nets.iter().find(|n| n.name == name)
}

/// Number of per-bit nets `base_0 .. base_{n-1}`, i.e. the port's real width.
fn bit_width(base: &str, d: &helion_ir::Design) -> usize {
    let prefix = format!("{base}_");
    (0..)
        .take_while(|i| {
            d.nets
                .iter()
                .any(|n| n.name == format!("{prefix}{i}"))
        })
        .count()
}

#[test]
fn comma_continued_inputs_share_range() {
    let src = r#"
module t(input [3:0] a, b, output [3:0] y);
  assign y = a ^ b;
endmodule
"#;
    let d = helion_sv::synth_sv(src, "t.sv").expect("synth");

    let dirs: std::collections::HashMap<&str, _> =
        d.ports.iter().map(|p| (p.name.as_str(), p.dir)).collect();
    assert!(dirs.contains_key("b"), "port b missing entirely");
    assert_eq!(
        bit_width("b", &d),
        4,
        "`b` in `input [3:0] a, b` must be 4 bits; nets = {:?}",
        d.nets.iter().map(|n| &n.name).collect::<Vec<_>>()
    );
    assert_eq!(bit_width("a", &d), 4, "`a` must stay 4 bits");
}

#[test]
fn comma_continued_outputs_share_range() {
    let src = r#"
module t(input [3:0] a, output [3:0] y, z);
  assign y = a;
  assign z = ~a;
endmodule
"#;
    let d = helion_sv::synth_sv(src, "t.sv").expect("synth");
    let z = d.ports.iter().find(|p| p.name == "z").expect("port z");
    assert_eq!(
        format!("{:?}", z.dir),
        "Out",
        "`z` in `output [3:0] y, z` must be an output, not a default input"
    );
    assert_eq!(bit_width("z", &d), 4, "`z` must be 4 bits");
}

#[test]
fn comma_then_new_direction_still_works() {
    // The comma must not swallow the next declaration's direction keyword.
    let src = r#"
module t(input [3:0] a, output [3:0] y);
  assign y = a;
endmodule
"#;
    let d = helion_sv::synth_sv(src, "t.sv").expect("synth");
    let dirs: std::collections::HashMap<&str, _> =
        d.ports.iter().map(|p| (p.name.as_str(), format!("{:?}", p.dir))).collect();
    assert_eq!(dirs.get("a").map(String::as_str), Some("In"));
    assert_eq!(dirs.get("y").map(String::as_str), Some("Out"));
}

#[test]
fn whole_port_still_parses() {
    let src = r#"
module t(input clk, input rst, output q);
  reg q;
  always_ff @(posedge clk) q <= rst ? 1'b0 : ~q;
endmodule
"#;
    let d = helion_sv::synth_sv(src, "t.sv").expect("synth");
    assert_eq!(d.ports.len(), 3, "ports = {:?}", d.ports.iter().map(|p| &p.name).collect::<Vec<_>>());
    let _ = net("q", &d);
}
