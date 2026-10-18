use helion_ir::{CellKind, Design, PortDir};
use helion_lib::{
    cec_cutpoint_sweep, sec_prove_sequential_equiv, set_ff_init, SecStatus,
};

#[test]
fn cec_cutpoint_sweep_proves_internal_keypoints_including_complemented_nets() {
    // Golden:
    //   k1 = a & b
    //   k2 = c ^ d
    //   y0 = k1 | k2
    //   y1 = k1 ^ k2
    let mut gold = Design::new("gold_comb");
    for p in ["a", "b", "c", "d"] {
        gold.add_port(p, PortDir::In);
    }
    gold.add_port("y0", PortDir::Out);
    gold.add_port("y1", PortDir::Out);

    gold.add_cell("g_and", CellKind::Lut6 { init: 0x8888_8888_8888_8888 }); // I0 & I1
    gold.connect("a", "g_and", "I0");
    gold.connect("b", "g_and", "I1");
    gold.connect("k1", "g_and", "O");

    gold.add_cell("g_xor", CellKind::Lut6 { init: 0x6666_6666_6666_6666 }); // I0 ^ I1
    gold.connect("c", "g_xor", "I0");
    gold.connect("d", "g_xor", "I1");
    gold.connect("k2", "g_xor", "O");

    gold.add_cell("g_or", CellKind::Lut6 { init: 0xeeee_eeee_eeee_eeee }); // I0 | I1
    gold.connect("k1", "g_or", "I0");
    gold.connect("k2", "g_or", "I1");
    gold.connect("y0", "g_or", "O");

    gold.add_cell("g_y1", CellKind::Lut6 { init: 0x6666_6666_6666_6666 }); // I0 ^ I1
    gold.connect("k1", "g_y1", "I0");
    gold.connect("k2", "g_y1", "I1");
    gold.connect("y1", "g_y1", "O");

    // Revised (DeMorgan + XNOR internal keypoints):
    //   rk1_n = ~(a & b)   [inverted cutpoint of k1!]
    //   rk2_n = ~(c ^ d)   [inverted cutpoint of k2!]
    //   y0    = ~(rk1_n & rk2_n) == k1 | k2
    //   y1    = rk1_n ^ rk2_n    == k1 ^ k2
    let mut rev = Design::new("rev_comb");
    for p in ["a", "b", "c", "d"] {
        rev.add_port(p, PortDir::In);
    }
    rev.add_port("y0", PortDir::Out);
    rev.add_port("y1", PortDir::Out);

    rev.add_cell("r_nand", CellKind::Lut6 { init: 0x7777_7777_7777_7777 }); // ~(I0 & I1)
    rev.connect("a", "r_nand", "I0");
    rev.connect("b", "r_nand", "I1");
    rev.connect("rk1_n", "r_nand", "O");

    rev.add_cell("r_xnor", CellKind::Lut6 { init: 0x9999_9999_9999_9999 }); // ~(I0 ^ I1)
    rev.connect("c", "r_xnor", "I0");
    rev.connect("d", "r_xnor", "I1");
    rev.connect("rk2_n", "r_xnor", "O");

    rev.add_cell("r_y0", CellKind::Lut6 { init: 0x7777_7777_7777_7777 }); // ~(I0 & I1)
    rev.connect("rk1_n", "r_y0", "I0");
    rev.connect("rk2_n", "r_y0", "I1");
    rev.connect("y0", "r_y0", "O");

    rev.add_cell("r_y1", CellKind::Lut6 { init: 0x6666_6666_6666_6666 }); // I0 ^ I1
    rev.connect("rk1_n", "r_y1", "I0");
    rev.connect("rk2_n", "r_y1", "I1");
    rev.connect("y1", "r_y1", "O");

    let res = cec_cutpoint_sweep(&gold, &rev).expect("cec_cutpoint_sweep");
    assert!(res.equivalent, "DeMorgan revised design must be equivalent: {res:?}");
    assert!(res.failing_outputs.is_empty());
    assert!(
        res.proved_cutpoints
            .iter()
            .any(|cp| cp.golden_net == "k1" && cp.revised_net == "rk1_n" && cp.inverted),
        "expected complemented cutpoint k1 == ~rk1_n in {:?}",
        res.proved_cutpoints
    );
    assert!(
        res.proved_cutpoints
            .iter()
            .any(|cp| cp.golden_net == "k2" && cp.revised_net == "rk2_n" && cp.inverted),
        "expected complemented cutpoint k2 == ~rk2_n in {:?}",
        res.proved_cutpoints
    );

    // Introduce a bug on y1 in `rev` (change XOR to OR) and verify CEC pinpoints `y1` only!
    let mut rev_bug = rev.clone();
    for c in &mut rev_bug.cells {
        if c.name == helion_ir::intern("r_y1") {
            c.kind = CellKind::Lut6 {
                init: 0xeeee_eeee_eeee_eeee,
            };
        }
    }
    let res_bug = cec_cutpoint_sweep(&gold, &rev_bug).expect("cec_cutpoint_sweep buggy");
    assert!(!res_bug.equivalent);
    assert_eq!(res_bug.failing_outputs.len(), 1);
    assert_eq!(res_bug.failing_outputs[0].output_name, "y1");
    assert_ne!(
        res_bug.failing_outputs[0].golden_value,
        res_bug.failing_outputs[0].revised_value
    );
}

#[test]
fn sec_proves_negated_register_correspondence_and_retimed_pipeline() {
    // Test 1: Golden has FF `q` (init 0, D = q ^ a, y = q).
    // Revised has active-low FF `qn` (init 1, D = ~(qn ^ a), y = ~qn).
    let mut gold = Design::new("gold_tff");
    gold.add_port("clk", PortDir::In);
    gold.add_port("a", PortDir::In);
    gold.add_port("y", PortDir::Out);

    gold.add_cell("ff_q", CellKind::Hff);
    gold.connect("clk", "ff_q", "CLK");
    gold.connect("d", "ff_q", "D");
    gold.connect("q", "ff_q", "Q");
    set_ff_init(&mut gold, "ff_q", false);

    gold.add_cell("lut_d", CellKind::Lut6 { init: 0x6666_6666_6666_6666 }); // q ^ a
    gold.connect("q", "lut_d", "I0");
    gold.connect("a", "lut_d", "I1");
    gold.connect("d", "lut_d", "O");

    gold.add_cell("lut_y", CellKind::Lut6 { init: 0xaaaa_aaaa_aaaa_aaaa }); // y = q
    gold.connect("q", "lut_y", "I0");
    gold.connect("y", "lut_y", "O");

    let mut rev = Design::new("rev_tff_inv");
    rev.add_port("clk", PortDir::In);
    rev.add_port("a", PortDir::In);
    rev.add_port("y", PortDir::Out);

    rev.add_cell("ff_qn", CellKind::Hff);
    rev.connect("clk", "ff_qn", "CLK");
    rev.connect("dn", "ff_qn", "D");
    rev.connect("qn", "ff_qn", "Q");
    set_ff_init(&mut rev, "ff_qn", true);

    rev.add_cell("lut_dn", CellKind::Lut6 { init: 0x6666_6666_6666_6666 }); // qn ^ a == ~(~qn ^ a)
    rev.connect("qn", "lut_dn", "I0");
    rev.connect("a", "lut_dn", "I1");
    rev.connect("dn", "lut_dn", "O");

    rev.add_cell("lut_y", CellKind::Lut6 { init: 0x5555_5555_5555_5555 }); // y = ~qn
    rev.connect("qn", "lut_y", "I0");
    rev.connect("y", "lut_y", "O");

    let sec = sec_prove_sequential_equiv(&gold, &rev, 16).expect("sec negated reg");
    assert_eq!(sec.status, SecStatus::Equivalent);
    assert!(
        sec.register_correspondences
            .iter()
            .any(|rc| rc.golden_reg == "q" && rc.revised_reg == "qn" && rc.inverted),
        "expected q == ~qn in {:?}",
        sec.register_correspondences
    );

    // Test 2: Forward vs Backward Retiming across an AND gate!
    // Golden: inputs `a`, `b` -> AND gate -> 1 register `q_out` -> output `y`.
    // Revised: inputs `a`, `b` -> 2 registers `qa`, `qb` -> AND gate -> output `y`.
    let mut gold_ret = Design::new("gold_retimed");
    gold_ret.add_port("clk", PortDir::In);
    gold_ret.add_port("a", PortDir::In);
    gold_ret.add_port("b", PortDir::In);
    gold_ret.add_port("y", PortDir::Out);

    gold_ret.add_cell("lut_ab", CellKind::Lut6 { init: 0x8888_8888_8888_8888 });
    gold_ret.connect("a", "lut_ab", "I0");
    gold_ret.connect("b", "lut_ab", "I1");
    gold_ret.connect("ab", "lut_ab", "O");

    gold_ret.add_cell("ff_y", CellKind::Hff);
    gold_ret.connect("clk", "ff_y", "CLK");
    gold_ret.connect("ab", "ff_y", "D");
    gold_ret.connect("y", "ff_y", "Q");
    set_ff_init(&mut gold_ret, "ff_y", false);

    let mut rev_ret = Design::new("rev_retimed");
    rev_ret.add_port("clk", PortDir::In);
    rev_ret.add_port("a", PortDir::In);
    rev_ret.add_port("b", PortDir::In);
    rev_ret.add_port("y", PortDir::Out);

    rev_ret.add_cell("ff_a", CellKind::Hff);
    rev_ret.connect("clk", "ff_a", "CLK");
    rev_ret.connect("a", "ff_a", "D");
    rev_ret.connect("qa", "ff_a", "Q");
    set_ff_init(&mut rev_ret, "ff_a", false);

    rev_ret.add_cell("ff_b", CellKind::Hff);
    rev_ret.connect("clk", "ff_b", "CLK");
    rev_ret.connect("b", "ff_b", "D");
    rev_ret.connect("qb", "ff_b", "Q");
    set_ff_init(&mut rev_ret, "ff_b", false);

    rev_ret.add_cell("lut_y", CellKind::Lut6 { init: 0x8888_8888_8888_8888 });
    rev_ret.connect("qa", "lut_y", "I0");
    rev_ret.connect("qb", "lut_y", "I1");
    rev_ret.connect("y", "lut_y", "O");

    let sec_ret =
        sec_prove_sequential_equiv(&gold_ret, &rev_ret, 16).expect("sec retimed pipeline");
    assert_eq!(sec_ret.status, SecStatus::Equivalent);
}

#[test]
fn sec_proves_onehot_vs_binary_fsm_reencoding_and_catches_sequential_bug() {
    // 4-state cyclic FSM with enable input `en` and output `done` (high in state 3):
    // Golden: 4-bit one-hot encoding (s0=1, s1=0, s2=0, s3=0), `done = s3`.
    let mut onehot = Design::new("fsm_onehot");
    onehot.add_port("clk", PortDir::In);
    onehot.add_port("en", PortDir::In);
    onehot.add_port("done", PortDir::Out);

    let state_names = ["s0", "s1", "s2", "s3"];
    for (i, &sn) in state_names.iter().enumerate() {
        let prev = state_names[(i + 3) % 4];
        let ff_name = format!("ff_{sn}");
        let d_name = format!("d_{sn}");
        let lut_name = format!("lut_{sn}");
        // d_i = en ? prev : s_i  (I0 = en, I1 = prev, I2 = s_i -> init = 0xD8D8_D8D8_D8D8_D8D8)
        onehot.add_cell(
            &lut_name,
            CellKind::Lut6 {
                init: 0xd8d8_d8d8_d8d8_d8d8,
            },
        );
        onehot.connect("en", &lut_name, "I0");
        onehot.connect(prev, &lut_name, "I1");
        onehot.connect(sn, &lut_name, "I2");
        onehot.connect(&d_name, &lut_name, "O");

        onehot.add_cell(&ff_name, CellKind::Hff);
        onehot.connect("clk", &ff_name, "CLK");
        onehot.connect(&d_name, &ff_name, "D");
        onehot.connect(sn, &ff_name, "Q");
        set_ff_init(&mut onehot, &ff_name, i == 0);
    }
    onehot.add_cell(
        "lut_done",
        CellKind::Lut6 {
            init: 0xaaaa_aaaa_aaaa_aaaa,
        },
    );
    onehot.connect("s3", "lut_done", "I0");
    onehot.connect("done", "lut_done", "O");

    // Revised: 2-bit binary encoding (b0=0, b1=0):
    //   d_b0 = b0 ^ en
    //   d_b1 = b1 ^ (b0 & en)
    //   done = b0 & b1
    let mut binary = Design::new("fsm_binary");
    binary.add_port("clk", PortDir::In);
    binary.add_port("en", PortDir::In);
    binary.add_port("done", PortDir::Out);

    binary.add_cell("ff_b0", CellKind::Hff);
    binary.connect("clk", "ff_b0", "CLK");
    binary.connect("d_b0", "ff_b0", "D");
    binary.connect("b0", "ff_b0", "Q");
    set_ff_init(&mut binary, "ff_b0", false);

    binary.add_cell("ff_b1", CellKind::Hff);
    binary.connect("clk", "ff_b1", "CLK");
    binary.connect("d_b1", "ff_b1", "D");
    binary.connect("b1", "ff_b1", "Q");
    set_ff_init(&mut binary, "ff_b1", false);

    binary.add_cell(
        "lut_db0",
        CellKind::Lut6 {
            init: 0x6666_6666_6666_6666, // b0 ^ en
        },
    );
    binary.connect("b0", "lut_db0", "I0");
    binary.connect("en", "lut_db0", "I1");
    binary.connect("d_b0", "lut_db0", "O");

    binary.add_cell(
        "lut_db1",
        CellKind::Lut6 {
            init: 0x7878_7878_7878_7878, // I2 ^ (I0 & I1) -> b1 ^ (b0 & en)
        },
    );
    binary.connect("b0", "lut_db1", "I0");
    binary.connect("en", "lut_db1", "I1");
    binary.connect("b1", "lut_db1", "I2");
    binary.connect("d_b1", "lut_db1", "O");

    binary.add_cell(
        "lut_done",
        CellKind::Lut6 {
            init: 0x8888_8888_8888_8888, // b0 & b1
        },
    );
    binary.connect("b0", "lut_done", "I0");
    binary.connect("b1", "lut_done", "I1");
    binary.connect("done", "lut_done", "O");

    let sec = sec_prove_sequential_equiv(&onehot, &binary, 16)
        .expect("sec one-hot vs binary FSM re-encoding");
    assert_eq!(sec.status, SecStatus::Equivalent);
    assert_eq!(sec.reachable_product_states, 4);

    // Mutate `binary` so `done = b1` (fires at state 2 instead of state 3) -> must fail at cycle 2!
    let mut binary_bug = binary.clone();
    for c in &mut binary_bug.cells {
        if c.name == helion_ir::intern("lut_done") {
            c.kind = CellKind::Lut6 {
                init: 0xcccc_cccc_cccc_cccc, // I1 (b1)
            };
        }
    }
    let sec_bug = sec_prove_sequential_equiv(&onehot, &binary_bug, 16).expect("sec buggy fsm");
    match sec_bug.status {
        SecStatus::Counterexample {
            cycle,
            failing_output,
            trace,
        } => {
            assert_eq!(cycle, 2, "state 2 reached after 2 enabled transitions");
            assert_eq!(failing_output, "done");
            assert_eq!(trace.len(), 3);
        }
        other => panic!("expected Counterexample at cycle 2, got {other:?}"),
    }
}
