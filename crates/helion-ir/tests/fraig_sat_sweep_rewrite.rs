use std::collections::HashMap;

use helion_ir::{
    default_pi_pattern, fraig_sat_sweep, intern, partition_candidate_classes, rewrite_aig_4cut,
    simulate_parallel_64, CellKind, Design, PortDir, Spur, FRAIG_VAR_INIT,
};

fn eval_comb_dag(d: &Design, inputs: &HashMap<Spur, bool>) -> HashMap<Spur, bool> {
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
        let res = eval_comb_dag(d, &env);
        for (oi, &osp) in out_spurs.iter().enumerate() {
            out[oi].push(res.get(&osp).copied().unwrap_or(false));
        }
    }
    out
}

#[test]
fn fraig_sat_sweep_merges_structurally_distinct_and_complemented_cones() {
    // Build a multi-level design with:
    // 1. Cone U: `(a & b) | (a & c)` (2-level: u_ab = a & b, u_ac = a & c, u_or = u_ab | u_ac)
    // 2. Cone V: `a & (b | c)` (2-level: v_bc = b | c, v_and = a & v_bc) -> functionally identical to U!
    // 3. Cone W: `~a | (~b & ~c)` (De Morgan complement of U: `W == ~U`!) feeding `y1 = W ^ d`
    let mut d = Design::new("fraig_equiv");
    let inputs = ["a", "b", "c", "d"];
    for &inp in &inputs {
        d.add_port(inp, PortDir::In);
    }
    d.add_port("y0", PortDir::Out);
    d.add_port("y1", PortDir::Out);

    let init_and2 = FRAIG_VAR_INIT[0] & FRAIG_VAR_INIT[1];
    let init_or2 = FRAIG_VAR_INIT[0] | FRAIG_VAR_INIT[1];
    let init_xor2 = FRAIG_VAR_INIT[0] ^ FRAIG_VAR_INIT[1];
    let init_nor2 = !(FRAIG_VAR_INIT[0] | FRAIG_VAR_INIT[1]);
    let init_nand2 = !(FRAIG_VAR_INIT[0] & FRAIG_VAR_INIT[1]);

    // Cone U:
    d.add_cell("u_ab", CellKind::Lut6 { init: init_and2 });
    d.connect("a", "u_ab", "I0");
    d.connect("b", "u_ab", "I1");
    d.connect("n_ab", "u_ab", "O");

    d.add_cell("u_ac", CellKind::Lut6 { init: init_and2 });
    d.connect("a", "u_ac", "I0");
    d.connect("c", "u_ac", "I1");
    d.connect("n_ac", "u_ac", "O");

    d.add_cell("u_or", CellKind::Lut6 { init: init_or2 });
    d.connect("n_ab", "u_or", "I0");
    d.connect("n_ac", "u_or", "I1");
    d.connect("net_u", "u_or", "O");

    // Cone V (structurally different factoring of `a & (b | c)`):
    d.add_cell("v_bc", CellKind::Lut6 { init: init_or2 });
    d.connect("b", "v_bc", "I0");
    d.connect("c", "v_bc", "I1");
    d.connect("n_bc", "v_bc", "O");

    d.add_cell("v_and", CellKind::Lut6 { init: init_and2 });
    d.connect("a", "v_and", "I0");
    d.connect("n_bc", "v_and", "I1");
    d.connect("net_v", "v_and", "O");

    // Cone W (`~(a & (b | c))` = `~a | (~b & ~c)`):
    d.add_cell("w_nor_bc", CellKind::Lut6 { init: init_nor2 });
    d.connect("b", "w_nor_bc", "I0");
    d.connect("c", "w_nor_bc", "I1");
    d.connect("n_nor_bc", "w_nor_bc", "O");

    d.add_cell("w_nand", CellKind::Lut6 { init: init_nand2 });
    d.connect("a", "w_nand", "I0");
    d.connect("n_bc", "w_nand", "I1");
    d.connect("net_w", "w_nand", "O");

    // Outputs: `y0 = net_u ^ net_v ^ d` (since net_u == net_v, net_u ^ net_v == 0!)
    d.add_cell("out0", CellKind::Lut6 { init: init_xor2 ^ FRAIG_VAR_INIT[2] });
    d.connect("net_u", "out0", "I0");
    d.connect("net_v", "out0", "I1");
    d.connect("d", "out0", "I2");
    d.connect("y0", "out0", "O");

    // `y1 = net_w ^ d`
    d.add_cell("out1", CellKind::Lut6 { init: init_xor2 });
    d.connect("net_w", "out1", "I0");
    d.connect("d", "out1", "I1");
    d.connect("y1", "out1", "O");

    // Check 64-bit parallel simulation candidate classes before sweeping:
    let mut pi_map = HashMap::new();
    for (idx, &inp) in inputs.iter().enumerate() {
        pi_map.insert(intern(inp), default_pi_pattern(idx, 42));
    }
    let sigs = simulate_parallel_64(&d, &pi_map);
    let classes = partition_candidate_classes(
        &[intern("net_u"), intern("net_v"), intern("net_w")],
        &sigs,
    );
    assert_eq!(
        classes.len(),
        1,
        "net_u, net_v, and complemented net_w must fall into one candidate class"
    );

    let before_tt = exhaustive_tt(&d, &inputs, &["y0", "y1"]);
    let luts_before = d.cells.len();

    let st = fraig_sat_sweep(&mut d);
    assert!(
        st.total_merged() >= 2,
        "expected both direct and inverted FRAIG merges, got {st:?}"
    );
    assert!(st.equiv_merged >= 1);
    assert!(st.inverted_merged >= 1);
    assert!(d.cells.len() < luts_before);
    assert_eq!(exhaustive_tt(&d, &inputs, &["y0", "y1"]), before_tt);
}

#[test]
fn fraig_sat_sweep_refines_near_equivalent_cones_via_counterexample() {
    // Two 7-input cones that are identical on 127 out of 128 input assignments,
    // differing ONLY when `x0 & x1 & x2 & x3 & x4 & x5 & x6 == 1`!
    // Initial 64-bit random simulation will likely see identical signatures (`0`),
    // and the SAT solver must find the 1-in-128 counterexample minterm, refine the
    // simulation table, and NOT merge them!
    let mut d = Design::new("fraig_cex_refine");
    let inputs = ["x0", "x1", "x2", "x3", "x4", "x5", "x6"];
    for &inp in &inputs {
        d.add_port(inp, PortDir::In);
    }
    d.add_port("y", PortDir::Out);

    // `and6 = x0 & x1 & x2 & x3 & x4 & x5` (`init = 1 << 63`)
    let init_and6 = 1u64 << 63;
    d.add_cell("u_and6", CellKind::Lut6 { init: init_and6 });
    for (i, &inp) in inputs[..6].iter().enumerate() {
        d.connect(inp, "u_and6", &format!("I{i}"));
    }
    d.connect("n_and6", "u_and6", "O");

    // `rare_one = n_and6 & x6` (only 1 on 1 of 128 inputs!)
    let init_and2 = FRAIG_VAR_INIT[0] & FRAIG_VAR_INIT[1];
    d.add_cell("u_rare", CellKind::Lut6 { init: init_and2 });
    d.connect("n_and6", "u_rare", "I0");
    d.connect("x6", "u_rare", "I1");
    d.connect("n_rare", "u_rare", "O");

    // `y = n_rare ^ x0`
    let init_xor2 = FRAIG_VAR_INIT[0] ^ FRAIG_VAR_INIT[1];
    d.add_cell("u_out", CellKind::Lut6 { init: init_xor2 });
    d.connect("n_rare", "u_out", "I0");
    d.connect("x0", "u_out", "I1");
    d.connect("y", "u_out", "O");

    let before_tt = exhaustive_tt(&d, &inputs, &["y"]);
    let st = d.fraig_sat_sweep();

    assert_eq!(st.consts_merged, 0, "n_rare is NOT constant 0 (it is 1 at minterm 127)");
    assert!(
        st.sat_counterexamples >= 1,
        "CDCL solver must find the counterexample minterm and refine signatures"
    );
    assert_eq!(exhaustive_tt(&d, &inputs, &["y"]), before_tt);
}

#[test]
fn rewrite_aig_4cut_collapses_4input_cascades_and_shares_canonical_4cuts() {
    let mut d = Design::new("rewrite_4cut");
    let inputs = ["a", "b", "c", "d", "e"];
    for &inp in &inputs {
        d.add_port(inp, PortDir::In);
    }
    d.add_port("y0", PortDir::Out);
    d.add_port("y1", PortDir::Out);

    let init_and2 = FRAIG_VAR_INIT[0] & FRAIG_VAR_INIT[1];
    let init_or2 = FRAIG_VAR_INIT[0] | FRAIG_VAR_INIT[1];
    let init_xor2 = FRAIG_VAR_INIT[0] ^ FRAIG_VAR_INIT[1];

    // Branch 0: 3-LUT cascade over 4 inputs `(a, b, c, d)` computing `((a & b) | c) ^ d`:
    d.add_cell("b0_l1", CellKind::Lut6 { init: init_and2 });
    d.connect("a", "b0_l1", "I0");
    d.connect("b", "b0_l1", "I1");
    d.connect("n_b0_1", "b0_l1", "O");

    d.add_cell("b0_l2", CellKind::Lut6 { init: init_or2 });
    d.connect("n_b0_1", "b0_l2", "I0");
    d.connect("c", "b0_l2", "I1");
    d.connect("n_b0_2", "b0_l2", "O");

    d.add_cell("b0_l3", CellKind::Lut6 { init: init_xor2 });
    d.connect("n_b0_2", "b0_l3", "I0");
    d.connect("d", "b0_l3", "I1");
    d.connect("n_b0_3", "b0_l3", "O");

    // Branch 1: another 3-LUT cascade over the same 4 inputs `(a, b, c, d)` computing the same function `d ^ (c | (b & a))`:
    d.add_cell("b1_l1", CellKind::Lut6 { init: init_and2 });
    d.connect("b", "b1_l1", "I0");
    d.connect("a", "b1_l1", "I1");
    d.connect("n_b1_1", "b1_l1", "O");

    d.add_cell("b1_l2", CellKind::Lut6 { init: init_or2 });
    d.connect("c", "b1_l2", "I0");
    d.connect("n_b1_1", "b1_l2", "I1");
    d.connect("n_b1_2", "b1_l2", "O");

    d.add_cell("b1_l3", CellKind::Lut6 { init: init_xor2 });
    d.connect("d", "b1_l3", "I0");
    d.connect("n_b1_2", "b1_l3", "I1");
    d.connect("n_b1_3", "b1_l3", "O");

    // Output LUTs using `n_b0_3` and `n_b1_3`:
    d.add_cell("out0", CellKind::Lut6 { init: init_and2 });
    d.connect("n_b0_3", "out0", "I0");
    d.connect("e", "out0", "I1");
    d.connect("y0", "out0", "O");

    d.add_cell("out1", CellKind::Lut6 { init: init_or2 });
    d.connect("n_b1_3", "out1", "I0");
    d.connect("e", "out1", "I1");
    d.connect("y1", "out1", "O");

    let before_tt = exhaustive_tt(&d, &inputs, &["y0", "y1"]);
    let st = rewrite_aig_4cut(&mut d);

    assert_eq!(st.luts_before, 8);
    assert_eq!(st.depth_before, 4);
    assert!(
        st.luts_after <= 3,
        "8 LUTs must rewrite and share down to <= 3 LUTs, got {}",
        st.luts_after
    );
    assert!(
        st.depth_after <= 2,
        "depth must drop from 4 to <= 2, got {}",
        st.depth_after
    );
    assert_eq!(exhaustive_tt(&d, &inputs, &["y0", "y1"]), before_tt);
}
