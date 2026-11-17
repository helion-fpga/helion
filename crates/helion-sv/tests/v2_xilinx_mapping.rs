//! What the v2 front end builds for the 7-series flow (`set_carry_arith`):
//! carry chains, flop clock enables, and Ibex's depth. These pin the
//! structure; `tools/svcheck` (e.g. `tests/mul_cmp_chain.sv`) checks the
//! function.

use helion_ir::{resolve, CellKind, Design};

fn synth(src: &str, top: &str) -> Design {
    helion_sv::v2::dag::set_carry_arith(true);
    helion_sv::synth_sv_v2(src, "t.sv", Some(top)).expect("synth").design
}

fn count(d: &Design, kind: fn(&CellKind) -> bool) -> usize {
    d.cells.iter().filter(|c| kind(&c.kind)).count()
}

fn carries(d: &Design) -> usize {
    count(d, |k| matches!(k, CellKind::Carry4))
}

#[test]
fn wide_add_is_one_carry_chain() {
    let d = synth("module t(input logic [31:0] a, b, output logic [31:0] y); assign y = a + b; endmodule", "t");
    assert_eq!(carries(&d), 8, "32 bits = 8 CARRY4");
}

#[test]
fn constant_side_index_math_needs_no_chain() {
    // `31 - i` on a 5-bit index widened to 32 bits is `!i`: no carry chain.
    let d = synth(
        "module t(input logic [4:0] i, input logic [31:0] v, output logic y); assign y = v[31 - i]; endmodule",
        "t",
    );
    assert_eq!(carries(&d), 0);
}

#[test]
fn constant_high_bits_stop_the_chain() {
    // An 8-bit add widened to 32 bits: one chain for the varying bits plus the carry out.
    let d = synth(
        "module t(input logic [7:0] a, b, output logic [31:0] y); assign y = 32'(a) + 32'(b); endmodule",
        "t",
    );
    assert!(carries(&d) <= 3, "{} CARRY4 for a 9-bit result", carries(&d));
}

#[test]
fn shared_enable_drives_ce_not_a_d_mux() {
    let d = synth(
        "module t(input logic clk, en, input logic [7:0] d, output logic [7:0] q);
           always_ff @(posedge clk) if (en) q <= d;
         endmodule",
        "t",
    );
    let pins = d.pin_index();
    let ffs: Vec<&str> = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).map(|c| resolve(c.name)).collect();
    assert_eq!(ffs.len(), 8);
    for f in ffs {
        assert_eq!(pins.net_on(f, "CE").map(resolve), Some("en"), "{f} CE");
        assert!(pins.net_on(f, "D").map(resolve).is_some_and(|n| n.starts_with("d_")), "{f} D straight from the input");
    }
    assert_eq!(count(&d, |k| matches!(k, CellKind::Lut6 { .. })), 0, "no mux LUTs");
}

#[test]
fn rare_enable_stays_in_d() {
    // Two flops on an enable: not worth a slice control set.
    let d = synth(
        "module t(input logic clk, en, input logic [1:0] d, output logic [1:0] q);
           always_ff @(posedge clk) if (en) q <= d;
         endmodule",
        "t",
    );
    let pins = d.pin_index();
    assert!(d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).all(|c| pins.net_on(resolve(c.name), "CE").is_none()));
}

/// G2 guard: Ibex keeps its carry chains and its depth.
#[test]
fn ibex_depth_and_carry_chains() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let files = ["examples/ysyx_ibex.sv", "examples/ibex_obs_wrap.sv"];
    let sources: Vec<(String, String)> =
        files.iter().map(|f| (f.to_string(), std::fs::read_to_string(root.join(f)).expect(f))).collect();
    helion_sv::v2::dag::set_carry_arith(true);
    let s = helion_sv::v2::synth(&sources, Some("ibex_obs_wrap"), &[("VERILATOR".into(), "1".into())], vec![]).expect("ibex");
    assert!(carries(&s.design) > 0, "no carry chains");
    assert!(s.depth <= 20, "Ibex {} LUT levels (was 20)", s.depth);
    assert!(s.luts <= 7000, "Ibex {} LUTs (was 6,671)", s.luts);
}
