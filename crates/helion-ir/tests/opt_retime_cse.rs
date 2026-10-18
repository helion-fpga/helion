use std::collections::HashMap;

use helion_ir::{
    canonicalize_lut6, intern, resolve, CellKind, Design, MuxfKind, PortDir, Spur, WideTruthTable,
    BUF_INIT, MUXF_INIT,
};

/// Exhaustively evaluate `d` over all `1 << inputs.len()` boolean input assignments
/// and return the truth-table vector for each requested output net.
fn exhaustive_truth_tables(d: &Design, inputs: &[&str], outputs: &[&str]) -> Vec<Vec<bool>> {
    assert!(inputs.len() <= 12);
    let in_spurs: Vec<Spur> = inputs.iter().map(|s| intern(s)).collect();
    let out_spurs: Vec<Spur> = outputs.iter().map(|s| intern(s)).collect();
    let total = 1usize << inputs.len();
    let mut result = vec![Vec::with_capacity(total); outputs.len()];

    for mask in 0..total {
        let mut env = HashMap::with_capacity(inputs.len());
        for (bit, &sp) in in_spurs.iter().enumerate() {
            env.insert(sp, ((mask >> bit) & 1) != 0);
        }
        let eval = d.eval_comb(&env);
        for (oi, &out_sp) in out_spurs.iter().enumerate() {
            result[oi].push(eval.get(&out_sp).copied().unwrap_or(false));
        }
    }
    result
}

#[test]
fn canonicalize_lut6_permutes_inputs_and_prunes_dont_cares() {
    // Function: (a & b) ^ c, placed on pins I2=b, I4=a, I1=c, plus a don't-care net d on I0
    // and duplicate pin I5=a.
    let mut init = 0u64;
    for addr in 0..64u64 {
        let b = (addr >> 2) & 1;
        let a = (addr >> 4) & 1;
        let a_dup = (addr >> 5) & 1;
        let c = (addr >> 1) & 1;
        // Use (a & a_dup & b) ^ c; when I4 and I5 are both connected to `a`, this is (a & b) ^ c.
        let val = ((a & a_dup & b) ^ c) & 1;
        init |= val << addr;
    }

    let sa = intern("net_a");
    let sb = intern("net_b");
    let sc = intern("net_c");
    let sd = intern("net_dont_care");

    let (canon_init1, canon_ins1) = canonicalize_lut6(
        init,
        [Some(sd), Some(sc), Some(sb), None, Some(sa), Some(sa)],
    );

    // Same logical function `(a & b) ^ c` wired in a different pin order: I0=a, I1=b, I2=c.
    let mut init2 = 0u64;
    for addr in 0..64u64 {
        let a = addr & 1;
        let b = (addr >> 1) & 1;
        let c = (addr >> 2) & 1;
        let val = ((a & b) ^ c) & 1;
        init2 |= val << addr;
    }
    let (canon_init2, canon_ins2) =
        canonicalize_lut6(init2, [Some(sa), Some(sb), Some(sc), None, None, None]);

    assert_eq!(canon_ins1, canon_ins2, "canonical input Spur order must match");
    assert_eq!(canon_init1, canon_init2, "canonical 64-bit INIT must match");
    assert!(canon_ins1[3].is_none() && canon_ins1[4].is_none() && canon_ins1[5].is_none());
}

#[test]
fn opt_cse_merges_permuted_and_multilevel_cones_with_exhaustive_equivalence() {
    let mut d = Design::new("cse_cone");
    for p in ["a", "b", "c", "d"] {
        d.add_port(p, PortDir::In);
    }
    d.add_port("y0", PortDir::Out);
    d.add_port("y1", PortDir::Out);

    // Cone 0: sub0 computes (a & b) | c with (I0=a, I1=b, I2=c)
    let mut init_sub0 = 0u64;
    // Cone 1: sub1 computes the exact same function (a & b) | c, but wired with
    // permuted pins (I3=c, I1=a, I4=b) and a redundant don't-care input `d` on I0!
    let mut init_sub1 = 0u64;
    for addr in 0..64u64 {
        let a0 = addr & 1;
        let b0 = (addr >> 1) & 1;
        let c0 = (addr >> 2) & 1;
        init_sub0 |= (((a0 & b0) | c0) & 1) << addr;

        let a1 = (addr >> 1) & 1;
        let b1 = (addr >> 4) & 1;
        let c1 = (addr >> 3) & 1;
        init_sub1 |= (((a1 & b1) | c1) & 1) << addr;
    }

    d.add_cell("sub0", CellKind::Lut6 { init: init_sub0 });
    d.connect("a", "sub0", "I0");
    d.connect("b", "sub0", "I1");
    d.connect("c", "sub0", "I2");
    d.connect("n_sub0", "sub0", "O");

    d.add_cell("sub1", CellKind::Lut6 { init: init_sub1 });
    d.connect("d", "sub1", "I0"); // don't-care pin
    d.connect("a", "sub1", "I1");
    d.connect("c", "sub1", "I3");
    d.connect("b", "sub1", "I4");
    d.connect("n_sub1", "sub1", "O");

    // Second level: top0 computes `n_sub0 ^ d` (I0=n_sub0, I1=d),
    // top1 computes `d ^ n_sub1` (I2=d, I5=n_sub1).
    let mut init_top0 = 0u64;
    let mut init_top1 = 0u64;
    for addr in 0..64u64 {
        init_top0 |= (((addr & 1) ^ ((addr >> 1) & 1)) & 1) << addr;
        init_top1 |= ((((addr >> 2) & 1) ^ ((addr >> 5) & 1)) & 1) << addr;
    }

    d.add_cell("top0", CellKind::Lut6 { init: init_top0 });
    d.connect("n_sub0", "top0", "I0");
    d.connect("d", "top0", "I1");
    d.connect("w_y0", "top0", "O");

    d.add_cell("top1", CellKind::Lut6 { init: init_top1 });
    d.connect("d", "top1", "I2");
    d.connect("n_sub1", "top1", "I5");
    d.connect("w_y1", "top1", "O");

    d.add_cell("ob0", CellKind::IobOut);
    d.connect("w_y0", "ob0", "I");
    d.connect("y0", "ob0", "PAD");

    d.add_cell("ob1", CellKind::IobOut);
    d.connect("w_y1", "ob1", "I");
    d.connect("y1", "ob1", "PAD");

    let inputs = ["a", "b", "c", "d"];
    let outputs = ["y0", "y1"];
    let before_tt = exhaustive_truth_tables(&d, &inputs, &outputs);
    assert_eq!(d.lut_count(), 4);

    let merged = d.opt_cse();
    assert_eq!(
        merged, 2,
        "both sub1 and top1 must be eliminated by multi-level functional CSE"
    );
    assert_eq!(d.lut_count(), 2);

    let after_tt = exhaustive_truth_tables(&d, &inputs, &outputs);
    assert_eq!(before_tt, after_tt, "exhaustive truth table must match after opt_cse");
}

#[test]
fn opt_lut_merge_fuses_shallow_cascade_reducing_depth_and_lut_count() {
    let mut d = Design::new("cascade_merge");
    let inputs = ["x0", "x1", "x2", "x3", "x4", "x5"];
    for &inp in &inputs {
        d.add_port(inp, PortDir::In);
    }
    d.add_port("y", PortDir::Out);

    // 3-level LUT cascade over 6 distinct inputs:
    // lut_a (depth 1): (x0 & x1) ^ x2
    // lut_b (depth 2): (lut_a | x3) ^ x4
    // lut_c (depth 3): (lut_b & x5) ^ x0
    let mut init_a = 0u64;
    let mut init_b = 0u64;
    let mut init_c = 0u64;
    for addr in 0..64u64 {
        let b0 = addr & 1;
        let b1 = (addr >> 1) & 1;
        let b2 = (addr >> 2) & 1;
        init_a |= (((b0 & b1) ^ b2) & 1) << addr;
        init_b |= (((b0 | b1) ^ b2) & 1) << addr;
        init_c |= (((b0 & b1) ^ b2) & 1) << addr;
    }

    d.add_cell("lut_a", CellKind::Lut6 { init: init_a });
    d.connect("x0", "lut_a", "I0");
    d.connect("x1", "lut_a", "I1");
    d.connect("x2", "lut_a", "I2");
    d.connect("na", "lut_a", "O");

    d.add_cell("lut_b", CellKind::Lut6 { init: init_b });
    d.connect("na", "lut_b", "I0");
    d.connect("x3", "lut_b", "I1");
    d.connect("x4", "lut_b", "I2");
    d.connect("nb", "lut_b", "O");

    d.add_cell("lut_c", CellKind::Lut6 { init: init_c });
    d.connect("nb", "lut_c", "I0");
    d.connect("x5", "lut_c", "I1");
    d.connect("x0", "lut_c", "I2");
    d.connect("nc", "lut_c", "O");

    d.add_cell("ob", CellKind::IobOut);
    d.connect("nc", "ob", "I");
    d.connect("y", "ob", "PAD");

    let before_tt = exhaustive_truth_tables(&d, &inputs, &["y"]);
    assert_eq!(d.lut_count(), 3);
    assert_eq!(d.max_lut_depth(), 3);

    let fused = d.opt_lut_merge();
    assert_eq!(fused, 2, "lut_a and lut_b must fuse into lut_c");
    assert_eq!(d.lut_count(), 1, "3-LUT cascade over 6 inputs must collapse to 1 LUT6");
    assert_eq!(d.max_lut_depth(), 1, "logic depth must drop from 3 to 1");

    let after_tt = exhaustive_truth_tables(&d, &inputs, &["y"]);
    assert_eq!(before_tt, after_tt, "exhaustive 6-input truth table must match after opt_lut_merge");
}

#[test]
fn opt_lut_merge_rejects_when_union_inputs_exceed_six() {
    let mut d = Design::new("wide_no_merge");
    let inputs = ["x0", "x1", "x2", "x3", "x4", "x5", "x6"];
    for &inp in &inputs {
        d.add_port(inp, PortDir::In);
    }
    d.add_port("y", PortDir::Out);

    // 4-input XOR in lut_a (x0..x3) + 3 new inputs (x4..x6) in lut_b => 7 distinct inputs > 6.
    let mut xor4 = 0u64;
    for addr in 0..64u64 {
        let p = (addr & 0xF).count_ones() as u64 & 1;
        xor4 |= p << addr;
    }
    d.add_cell("lut_a", CellKind::Lut6 { init: xor4 });
    for i in 0..4 {
        d.connect(inputs[i], "lut_a", &format!("I{i}"));
    }
    d.connect("na", "lut_a", "O");

    d.add_cell("lut_b", CellKind::Lut6 { init: xor4 });
    d.connect("na", "lut_b", "I0");
    d.connect("x4", "lut_b", "I1");
    d.connect("x5", "lut_b", "I2");
    d.connect("x6", "lut_b", "I3");
    d.connect("y", "lut_b", "O");

    let before_tt = exhaustive_truth_tables(&d, &inputs, &["y"]);
    assert_eq!(d.opt_lut_merge(), 0, "7-input union cannot fit into a single LUT6");
    assert_eq!(d.lut_count(), 2);
    assert_eq!(exhaustive_truth_tables(&d, &inputs, &["y"]), before_tt);
}

#[test]
fn decompose_wide_cone_muxf_7_and_8_input_exhaustive_verification() {
    // 1. 7-input arbitrary non-degenerate boolean function verified across all 128 input vectors.
    let mut d7 = Design::new("wide7");
    let ins7 = ["i0", "i1", "i2", "i3", "i4", "i5", "i6"];
    for &p in &ins7 {
        d7.add_port(p, PortDir::In);
    }
    d7.add_port("y7", PortDir::Out);

    let mut init128: u128 = 0;
    for addr in 0..128usize {
        // Nonlinear 7-input function: parity(i0..i6) ^ ((i0 & i3 & i6) | (i1 & i5))
        let parity = (addr.count_ones() & 1) != 0;
        let term = (((addr & 1) != 0) && (((addr >> 3) & 1) != 0) && (((addr >> 6) & 1) != 0))
            || ((((addr >> 1) & 1) != 0) && (((addr >> 5) & 1) != 0));
        if parity ^ term {
            init128 |= 1u128 << addr;
        }
    }

    let decomp7 = d7
        .decompose_wide_cone_muxf("cone7", &ins7, "y7", init128)
        .expect("7-input decomposition");
    assert_eq!(decomp7.luts.len(), 2);
    assert_eq!(decomp7.muxf7s.len(), 1);
    assert!(decomp7.muxf8.is_none());
    let muxf7_cell = d7.cell(resolve(decomp7.muxf7s[0])).unwrap();
    assert!(muxf7_cell.is_muxf7());
    assert_eq!(muxf7_cell.muxf_kind(), Some(MuxfKind::Muxf7));
    assert!(matches!(muxf7_cell.kind, CellKind::Lut6 { init } if init == MUXF_INIT));

    let tt7 = exhaustive_truth_tables(&d7, &ins7, &["y7"]);
    for addr in 0..128usize {
        let expected = ((init128 >> addr) & 1) != 0;
        assert_eq!(tt7[0][addr], expected, "7-input mismatch at addr={addr:#09b}");
    }

    // 2. 8-input arbitrary function lowered into 4 LUT6 + 2 MUXF7 + 1 MUXF8, verified across all 256 vectors.
    let mut d8 = Design::new("wide8");
    let ins8 = ["i0", "i1", "i2", "i3", "i4", "i5", "i6", "i7"];
    for &p in &ins8 {
        d8.add_port(p, PortDir::In);
    }
    d8.add_port("y8", PortDir::Out);

    let mut cofactors = [0u64; 4];
    for addr in 0..256usize {
        let c_idx = addr >> 6;
        let bit_idx = addr & 63;
        let val = ((addr.wrapping_mul(0x9E37_79B9) >> 24) ^ addr ^ (addr >> 3)) & 1 != 0;
        if val {
            cofactors[c_idx] |= 1u64 << bit_idx;
        }
    }

    let decomp8 = d8
        .decompose_wide_cone_muxf("cone8", &ins8, "y8", WideTruthTable::Lut8(cofactors))
        .expect("8-input decomposition");
    assert_eq!(decomp8.luts.len(), 4);
    assert_eq!(decomp8.muxf7s.len(), 2);
    assert!(decomp8.muxf8.is_some());
    let muxf8_cell = d8.cell(resolve(decomp8.muxf8.unwrap())).unwrap();
    assert!(muxf8_cell.is_muxf8());
    assert_eq!(muxf8_cell.muxf_kind(), Some(MuxfKind::Muxf8));

    let tt8 = exhaustive_truth_tables(&d8, &ins8, &["y8"]);
    for addr in 0..256usize {
        let expected = ((cofactors[addr >> 6] >> (addr & 63)) & 1) != 0;
        assert_eq!(tt8[0][addr], expected, "8-input mismatch at addr={addr:#010b}");
    }
}

#[test]
fn phys_opt_retime_backward_reduces_critical_depth_and_preserves_simulation() {
    // Pipeline with unbalanced stages:
    // Stage 1 has depth 3 (lut1 -> lut2 -> lut3 -> ff_mid)
    // Stage 2 has depth 0 (ff_mid.Q -> obuf -> y)
    // Include an inverted truth table on lut3 (`(init & 1) == 1`) to test zero-init polarity preservation!
    let mut d = Design::new("retime_bwd");
    for p in ["clk", "a", "b", "c", "e"] {
        d.add_port(p, PortDir::In);
    }
    d.add_port("y", PortDir::Out);

    // lut1: a ^ b (init = 0x6666...)
    let init_xor2 = 0x6666_6666_6666_6666u64;
    // lut2: lut1 | c (init = 0xEEEE...)
    let init_or2 = 0xEEEE_EEEE_EEEE_EEEEu64;
    // lut3: ~(lut2 ^ e) (XNOR2: init = 0x9999..., so init & 1 == 1!)
    let init_xnor2 = 0x9999_9999_9999_9999u64;

    d.add_cell("lut1", CellKind::Lut6 { init: init_xor2 });
    d.connect("a", "lut1", "I0");
    d.connect("b", "lut1", "I1");
    d.connect("n1", "lut1", "O");

    d.add_cell("lut2", CellKind::Lut6 { init: init_or2 });
    d.connect("n1", "lut2", "I0");
    d.connect("c", "lut2", "I1");
    d.connect("n2", "lut2", "O");

    d.add_cell("lut3", CellKind::Lut6 { init: init_xnor2 });
    d.connect("n2", "lut3", "I0");
    d.connect("e", "lut3", "I1");
    d.connect("n3", "lut3", "O");

    d.add_cell("ff_mid", CellKind::Hff);
    d.connect("clk", "ff_mid", "CLK");
    d.connect("n3", "ff_mid", "D");
    d.connect("q_out", "ff_mid", "Q");

    d.add_cell("ob", CellKind::IobOut);
    d.connect("q_out", "ob", "I");
    d.connect("y", "ob", "PAD");

    assert_eq!(d.max_lut_depth(), 3);

    // Build a 32-cycle stimulus covering all 16 combinations of (a, b, c, e) twice.
    let mut vectors = Vec::with_capacity(32);
    for cyc in 0..32usize {
        let mut m = HashMap::new();
        m.insert(intern("a"), (cyc & 1) != 0);
        m.insert(intern("b"), ((cyc >> 1) & 1) != 0);
        m.insert(intern("c"), ((cyc >> 2) & 1) != 0);
        m.insert(intern("e"), ((cyc >> 3) & 1) != 0);
        vectors.push(m);
    }

    let trace_before = d.simulate_cycles(&vectors);
    let moves = d.phys_opt_retime();
    assert!(moves >= 1, "expected at least 1 backward retiming move");
    assert!(
        d.max_lut_depth() < 3,
        "critical LUT depth must decrease from 3, got {}",
        d.max_lut_depth()
    );

    let trace_after = d.simulate_cycles(&vectors);
    let y_spur = intern("y");
    for cyc in 0..vectors.len() {
        assert_eq!(
            trace_before[cyc].get(&y_spur),
            trace_after[cyc].get(&y_spur),
            "cycle {cyc} output `y` mismatch after backward retiming"
        );
    }
}

#[test]
fn phys_opt_retime_forward_reduces_depth_and_merges_input_ffs() {
    // Stage 1: two input FFs `ff_a` and `ff_b` (depth 0 before them)
    // Stage 2: `lut1` (NOR2, so init & 1 == 1!) -> `lut2` (XOR2) -> `lut3` (AND2) -> `obuf` (depth 3 after FFs)
    // Forward retiming pushes `ff_a` and `ff_b` across `lut1`, merging 2 FFs into 1 FF and reducing depth from 3 to 2!
    let mut d = Design::new("retime_fwd");
    for p in ["clk", "a", "b", "c", "e"] {
        d.add_port(p, PortDir::In);
    }
    d.add_port("y", PortDir::Out);

    d.add_cell("ff_a", CellKind::Hff);
    d.connect("clk", "ff_a", "CLK");
    d.connect("a", "ff_a", "D");
    d.connect("qa", "ff_a", "Q");

    d.add_cell("ff_b", CellKind::Hff);
    d.connect("clk", "ff_b", "CLK");
    d.connect("b", "ff_b", "D");
    d.connect("qb", "ff_b", "Q");

    // lut1: ~(qa | qb) (NOR2: init = 0x1111..., so init & 1 == 1!)
    d.add_cell("lut1", CellKind::Lut6 { init: 0x1111_1111_1111_1111 });
    d.connect("qa", "lut1", "I0");
    d.connect("qb", "lut1", "I1");
    d.connect("n1", "lut1", "O");

    // lut2: n1 ^ c
    d.add_cell("lut2", CellKind::Lut6 { init: 0x6666_6666_6666_6666 });
    d.connect("n1", "lut2", "I0");
    d.connect("c", "lut2", "I1");
    d.connect("n2", "lut2", "O");

    // lut3: n2 & e
    d.add_cell("lut3", CellKind::Lut6 { init: 0x8888_8888_8888_8888 });
    d.connect("n2", "lut3", "I0");
    d.connect("e", "lut3", "I1");
    d.connect("n3", "lut3", "O");

    d.add_cell("ob", CellKind::IobOut);
    d.connect("n3", "ob", "I");
    d.connect("y", "ob", "PAD");

    assert_eq!(d.max_lut_depth(), 3);
    assert_eq!(d.ff_count(), 2);

    let mut vectors = Vec::with_capacity(32);
    for cyc in 0..32usize {
        let mut m = HashMap::new();
        m.insert(intern("a"), ((cyc * 3) & 1) != 0);
        m.insert(intern("b"), (((cyc * 5) >> 1) & 1) != 0);
        m.insert(intern("c"), ((cyc >> 2) & 1) != 0);
        m.insert(intern("e"), ((cyc >> 1) & 1) != 0);
        vectors.push(m);
    }

    let trace_before = d.simulate_cycles(&vectors);
    let moves = d.phys_opt_retime();
    assert_eq!(moves, 1, "expected forward retiming across lut1");
    assert_eq!(d.max_lut_depth(), 2, "combinational depth after FF must drop from 3 to 2");
    assert_eq!(d.ff_count(), 1, "two input FFs must coalesce into 1 FF after lut1");

    let trace_after = d.simulate_cycles(&vectors);
    let y_spur = intern("y");
    for cyc in 0..vectors.len() {
        assert_eq!(
            trace_before[cyc].get(&y_spur),
            trace_after[cyc].get(&y_spur),
            "cycle {cyc} output `y` mismatch after forward retiming"
        );
    }
}

#[test]
fn phys_opt_fanout_split_replicates_high_fanout_register_and_preserves_behavior() {
    let mut d = Design::new("fanout_split");
    d.add_port("clk", PortDir::In);
    d.add_port("din", PortDir::In);

    d.add_cell("src_ff", CellKind::Hff);
    d.connect("clk", "src_ff", "CLK");
    d.connect("din", "src_ff", "D");
    d.connect("q_hot", "src_ff", "Q");

    // 10 fanout LUTs driven by `q_hot`.
    let mut out_names = Vec::new();
    for i in 0..10usize {
        let out_p = format!("y{i}");
        let lut_c = format!("sink_lut{i}");
        let init = if i % 2 == 0 {
            BUF_INIT
        } else {
            0x5555_5555_5555_5555
        };
        d.add_port(&out_p, PortDir::Out);
        d.add_cell(&lut_c, CellKind::Lut6 { init });
        d.connect("q_hot", &lut_c, "I0");
        d.connect(&out_p, &lut_c, "O");
        out_names.push(out_p);
    }

    let mut vectors = Vec::with_capacity(12);
    for cyc in 0..12usize {
        let mut m = HashMap::new();
        m.insert(intern("din"), (cyc % 3 == 1) || (cyc % 4 == 2));
        vectors.push(m);
    }
    let trace_before = d.simulate_cycles(&vectors);
    assert_eq!(d.ff_count(), 1);

    // Split with max_fanout = 4 -> 10 loads split into 4 + 4 + 2 (1 original + 2 replicas = 3 FFs).
    let replicas = d.phys_opt_fanout_split(4);
    assert_eq!(replicas, 2, "10 loads with max_fanout=4 requires 2 replica FFs");
    assert_eq!(d.ff_count(), 3);

    // Verify every FF Q net has at most 4 load endpoints (<= 5 total endpoints including Q driver).
    for net in &d.nets {
        let name = resolve(net.name);
        if name.starts_with("q_hot") {
            assert!(
                net.endpoints.len() <= 5,
                "net {name} has {} endpoints (expected <= 5)",
                net.endpoints.len()
            );
        }
    }

    let trace_after = d.simulate_cycles(&vectors);
    for cyc in 0..vectors.len() {
        for out_p in &out_names {
            let sp = intern(out_p);
            assert_eq!(
                trace_before[cyc].get(&sp),
                trace_after[cyc].get(&sp),
                "cycle {cyc} output {out_p} mismatch after fanout split"
            );
        }
    }
}

#[test]
fn opt_design_pipeline_coalesces_buffers_and_respects_dont_touch_guards() {
    let mut d = Design::new("full_opt");
    for p in ["a", "b", "c"] {
        d.add_port(p, PortDir::In);
    }
    d.add_port("y", PortDir::Out);

    // Buffer chain: a -> buf0 -> buf1 -> and_lut
    d.add_cell("buf0", CellKind::Lut6 { init: BUF_INIT });
    d.connect("a", "buf0", "I0");
    d.connect("na0", "buf0", "O");

    d.add_cell("buf1", CellKind::Lut6 { init: BUF_INIT });
    d.connect("na0", "buf1", "I0");
    d.connect("na1", "buf1", "O");

    // Guarded buffer that must NOT be removed:
    d.add_cell("buf_guarded", CellKind::Lut6 { init: BUF_INIT });
    d.connect("b", "buf_guarded", "I0");
    d.connect("nb_g", "buf_guarded", "O");
    d.dont_touch("buf_guarded").unwrap();

    d.add_cell("and_lut", CellKind::Lut6 { init: 0x8080_8080_8080_8080 });
    d.connect("na1", "and_lut", "I0");
    d.connect("nb_g", "and_lut", "I1");
    d.connect("c", "and_lut", "I2");
    d.connect("y", "and_lut", "O");

    let before_tt = exhaustive_truth_tables(&d, &["a", "b", "c"], &["y"]);
    assert_eq!(d.lut_count(), 4);

    let stats = d.opt_design();
    assert_eq!(stats.buffers_coalesced, 2, "buf0 and buf1 must be coalesced");
    assert!(d.cell("buf_guarded").is_some(), "DONT_TOUCH cell must remain intact");
    assert_eq!(d.lut_count(), 2);

    let after_tt = exhaustive_truth_tables(&d, &["a", "b", "c"], &["y"]);
    assert_eq!(before_tt, after_tt);
}
