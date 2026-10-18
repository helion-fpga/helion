use std::collections::{HashMap, HashSet};

use helion_ir::{
    intern, map_priority_cuts_lut6, npn4_apply, npn4_are_equivalent, npn4_canonical,
    npn4_reconstruct, CellKind, Design, PortDir, Spur, LUT6_VAR_INIT,
};

/// Evaluate the combinational LUT network of `d` for a primary input assignment.
fn eval_lut_dag(d: &Design, inputs: &HashMap<Spur, bool>) -> HashMap<Spur, bool> {
    let mut vals = inputs.clone();
    let i_pins = [
        intern("I0"),
        intern("I1"),
        intern("I2"),
        intern("I3"),
        intern("I4"),
        intern("I5"),
    ];
    let o_pin = intern("O");
    let max_rounds = d.cells.len() + 2;

    for _ in 0..max_rounds {
        let mut changed = false;
        for c in &d.cells {
            let CellKind::Lut6 { init } = c.kind else {
                continue;
            };
            let Some(out_net) = d.net_on(c.name, o_pin) else {
                continue;
            };
            let mut addr = 0u64;
            for (idx, &p_sp) in i_pins.iter().enumerate() {
                if let Some(in_net) = d.net_on(c.name, p_sp) {
                    if vals.get(&in_net).copied().unwrap_or(false) {
                        addr |= 1u64 << idx;
                    }
                }
            }
            let out_val = ((init >> addr) & 1) != 0;
            if vals.insert(out_net, out_val) != Some(out_val) {
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    vals
}

fn exhaustive_tt(d: &Design, inputs: &[&str], outputs: &[&str]) -> Vec<Vec<bool>> {
    let in_spurs: Vec<Spur> = inputs.iter().map(|s| intern(s)).collect();
    let out_spurs: Vec<Spur> = outputs.iter().map(|s| intern(s)).collect();
    let total = 1usize << inputs.len();
    let mut out = vec![Vec::with_capacity(total); outputs.len()];

    for mask in 0..total {
        let mut env = HashMap::with_capacity(inputs.len());
        for (bit, &sp) in in_spurs.iter().enumerate() {
            env.insert(sp, ((mask >> bit) & 1) != 0);
        }
        let res = eval_lut_dag(d, &env);
        for (oi, &osp) in out_spurs.iter().enumerate() {
            out[oi].push(res.get(&osp).copied().unwrap_or(false));
        }
    }
    out
}

#[test]
fn npn4_canonical_unifies_equivalent_functions_and_reconstructs_exact_truth_tables() {
    // 4-input variables as 16-bit truth tables:
    let x0 = 0xAAAAu16;
    let x1 = 0xCCCCu16;
    let x2 = 0xF0F0u16;
    let x3 = 0xFF00u16;

    // De Morgan & input-negated/permuted variants of 4-input AND (`x0 & x1 & x2 & x3`):
    let and4 = x0 & x1 & x2 & x3;
    let or4 = x0 | x1 | x2 | x3; // ~(~x0 & ~x1 & ~x2 & ~x3) -> NPN-equivalent to AND4!
    let nand4 = !and4;
    let skewed_and4 = (!x2) & x0 & (!x3) & x1;

    let c_and4 = npn4_canonical(and4);
    assert_eq!(c_and4.canonical, npn4_canonical(or4).canonical);
    assert_eq!(c_and4.canonical, npn4_canonical(nand4).canonical);
    assert_eq!(c_and4.canonical, npn4_canonical(skewed_and4).canonical);
    assert!(npn4_are_equivalent(and4, or4));
    assert!(npn4_are_equivalent(and4, skewed_and4));

    // Verify 4-input XOR (`x0 ^ x1 ^ x2 ^ x3`) is NPN-equivalent to XNOR4, but NOT to AND4:
    let xor4 = x0 ^ x1 ^ x2 ^ x3;
    let xnor4 = !xor4;
    assert!(npn4_are_equivalent(xor4, xnor4));
    assert!(!npn4_are_equivalent(and4, xor4));

    // Verify exact invertibility (`npn4_reconstruct`) and `npn4_apply` across 512 diverse 16-bit truth tables:
    let mut classes = HashSet::new();
    for i in 0..512u32 {
        let tt = (i.wrapping_mul(0x9E37) ^ (i << 7) ^ (i >> 2)) as u16;
        let tf = npn4_canonical(tt);
        assert_eq!(
            npn4_apply(tt, tf.perm, tf.in_neg, tf.out_neg),
            tf.canonical
        );
        assert_eq!(
            npn4_reconstruct(&tf),
            tt,
            "npn4_reconstruct must invert npn4_canonical for tt={tt:#06x}"
        );
        classes.insert(tf.canonical);
    }
    assert!(classes.len() > 40, "expected many distinct NPN4 classes");
}

#[test]
fn map_priority_cuts_lut6_collapses_deep_6input_tree_to_single_lut() {
    let mut d = Design::new("deep_tree6");
    let inputs = ["x0", "x1", "x2", "x3", "x4", "x5"];
    for &inp in &inputs {
        d.add_port(inp, PortDir::In);
    }
    d.add_port("y", PortDir::Out);

    // Build a 4-level tree of 5 two-input LUTs over 6 primary inputs:
    // g0 = x0 & x1 (depth 1)
    // g1 = x2 ^ x3 (depth 1)
    // g2 = g0 | g1 (depth 2)
    // g3 = g2 ^ x4 (depth 3)
    // y  = g3 & ~x5 (depth 4)
    let init_and2 = LUT6_VAR_INIT[0] & LUT6_VAR_INIT[1];
    let init_xor2 = LUT6_VAR_INIT[0] ^ LUT6_VAR_INIT[1];
    let init_or2 = LUT6_VAR_INIT[0] | LUT6_VAR_INIT[1];
    let init_and_not = LUT6_VAR_INIT[0] & !LUT6_VAR_INIT[1];

    d.add_cell("g0", CellKind::Lut6 { init: init_and2 });
    d.connect("x0", "g0", "I0");
    d.connect("x1", "g0", "I1");
    d.connect("n0", "g0", "O");

    d.add_cell("g1", CellKind::Lut6 { init: init_xor2 });
    d.connect("x2", "g1", "I0");
    d.connect("x3", "g1", "I1");
    d.connect("n1", "g1", "O");

    d.add_cell("g2", CellKind::Lut6 { init: init_or2 });
    d.connect("n0", "g2", "I0");
    d.connect("n1", "g2", "I1");
    d.connect("n2", "g2", "O");

    d.add_cell("g3", CellKind::Lut6 { init: init_xor2 });
    d.connect("n2", "g3", "I0");
    d.connect("x4", "g3", "I1");
    d.connect("n3", "g3", "O");

    d.add_cell("g_out", CellKind::Lut6 { init: init_and_not });
    d.connect("n3", "g_out", "I0");
    d.connect("x5", "g_out", "I1");
    d.connect("y", "g_out", "O");

    let before_tt = exhaustive_tt(&d, &inputs, &["y"]);
    let st = map_priority_cuts_lut6(&mut d);

    assert_eq!(st.luts_before, 5);
    assert_eq!(st.depth_before, 4);
    assert_eq!(st.luts_after, 1, "6-input cone must map to a single LUT6");
    assert_eq!(st.depth_after, 1, "combinational depth must drop from 4 to 1");
    assert_eq!(exhaustive_tt(&d, &inputs, &["y"]), before_tt);
}

#[test]
fn map_priority_cuts_lut6_minimizes_depth_and_recovers_shared_area_on_wide_dag() {
    // 8-input, 2-output DAG (`y0`, `y1`) sharing a 4-input subcone `s(a0..a3)`:
    // s0 = a0 ^ a1
    // s1 = a2 & a3
    // s  = s0 | s1  (shared node with fanout 2)
    // Branch 0: b0 = b_in0 & b_in1, c0 = s ^ b0, y0 = c0 | b_in2
    // Branch 1: b1 = d_in0 | d_in1, c1 = s & b1, y1 = c1 ^ b_in2
    let mut d = Design::new("wide_shared_dag");
    let inputs = ["a0", "a1", "a2", "a3", "b_in0", "b_in1", "b_in2", "d_in0", "d_in1"];
    for &inp in &inputs {
        d.add_port(inp, PortDir::In);
    }
    d.add_port("y0", PortDir::Out);
    d.add_port("y1", PortDir::Out);

    let init_and2 = LUT6_VAR_INIT[0] & LUT6_VAR_INIT[1];
    let init_or2 = LUT6_VAR_INIT[0] | LUT6_VAR_INIT[1];
    let init_xor2 = LUT6_VAR_INIT[0] ^ LUT6_VAR_INIT[1];

    d.add_cell("u_s0", CellKind::Lut6 { init: init_xor2 });
    d.connect("a0", "u_s0", "I0");
    d.connect("a1", "u_s0", "I1");
    d.connect("ns0", "u_s0", "O");

    d.add_cell("u_s1", CellKind::Lut6 { init: init_and2 });
    d.connect("a2", "u_s1", "I0");
    d.connect("a3", "u_s1", "I1");
    d.connect("ns1", "u_s1", "O");

    d.add_cell("u_s", CellKind::Lut6 { init: init_or2 });
    d.connect("ns0", "u_s", "I0");
    d.connect("ns1", "u_s", "I1");
    d.connect("ns", "u_s", "O");

    d.add_cell("u_b0", CellKind::Lut6 { init: init_and2 });
    d.connect("b_in0", "u_b0", "I0");
    d.connect("b_in1", "u_b0", "I1");
    d.connect("nb0", "u_b0", "O");

    d.add_cell("u_c0", CellKind::Lut6 { init: init_xor2 });
    d.connect("ns", "u_c0", "I0");
    d.connect("nb0", "u_c0", "I1");
    d.connect("nc0", "u_c0", "O");

    d.add_cell("u_y0", CellKind::Lut6 { init: init_or2 });
    d.connect("nc0", "u_y0", "I0");
    d.connect("b_in2", "u_y0", "I1");
    d.connect("y0", "u_y0", "O");

    d.add_cell("u_b1", CellKind::Lut6 { init: init_or2 });
    d.connect("d_in0", "u_b1", "I0");
    d.connect("d_in1", "u_b1", "I1");
    d.connect("nb1", "u_b1", "O");

    d.add_cell("u_c1", CellKind::Lut6 { init: init_and2 });
    d.connect("ns", "u_c1", "I0");
    d.connect("nb1", "u_c1", "I1");
    d.connect("nc1", "u_c1", "O");

    d.add_cell("u_y1", CellKind::Lut6 { init: init_xor2 });
    d.connect("nc1", "u_y1", "I0");
    d.connect("b_in2", "u_y1", "I1");
    d.connect("y1", "u_y1", "O");

    let before_tt = exhaustive_tt(&d, &inputs, &["y0", "y1"]);
    let st = d.map_priority_cuts_lut6();

    assert_eq!(st.luts_before, 9);
    assert_eq!(st.depth_before, 4);
    assert!(
        st.depth_after <= 2,
        "7-input cones must map to depth <= 2, got {}",
        st.depth_after
    );
    assert!(
        st.luts_after <= 4,
        "9 LUTs must compress to <= 4 LUTs (ideally 3 with shared subcone `ns`), got {}",
        st.luts_after
    );
    assert_eq!(exhaustive_tt(&d, &inputs, &["y0", "y1"]), before_tt);
}
