use helion_ir::{CellKind, Design, PortDir, INC4_INIT};
use helion_lib::{
    check_equivalence, emit_cex_vcd, run_bmc, run_ic3_pdr, run_k_induction, BmcResult,
    InductionResult, LecResult, PdrResult, SafetyProperty,
};

#[test]
fn lec_proves_de_morgan_and_cascade_combinational_equivalence() {
    // Gold: single LUT6 computing `(a & b) | (c ^ d)`
    let mut gold = Design::new("gold_comb");
    for p in ["a", "b", "c", "d"] {
        gold.add_port(p, PortDir::In);
    }
    gold.add_port("y", PortDir::Out);

    let mut init_gold = 0u64;
    for addr in 0..64u64 {
        let a = addr & 1;
        let b = (addr >> 1) & 1;
        let c = (addr >> 2) & 1;
        let d = (addr >> 3) & 1;
        init_gold |= (((a & b) | (c ^ d)) & 1) << addr;
    }
    gold.add_cell("u_lut", CellKind::Lut6 { init: init_gold });
    gold.connect("a", "u_lut", "I0");
    gold.connect("b", "u_lut", "I1");
    gold.connect("c", "u_lut", "I2");
    gold.connect("d", "u_lut", "I3");
    gold.connect("y", "u_lut", "O");

    // Gate: 3-LUT De Morgan implementation:
    // n_nand = ~(a & b)
    // n_xnor = ~(c ^ d)
    // y = ~(n_nand & n_xnor)
    let mut gate = Design::new("gate_comb");
    for p in ["a", "b", "c", "d"] {
        gate.add_port(p, PortDir::In);
    }
    gate.add_port("y", PortDir::Out);

    gate.add_cell("u_nand", CellKind::Lut6 { init: 0x7777_7777_7777_7777 });
    gate.connect("a", "u_nand", "I0");
    gate.connect("b", "u_nand", "I1");
    gate.connect("n_nand", "u_nand", "O");

    gate.add_cell("u_xnor", CellKind::Lut6 { init: 0x9999_9999_9999_9999 });
    gate.connect("c", "u_xnor", "I0");
    gate.connect("d", "u_xnor", "I1");
    gate.connect("n_xnor", "u_xnor", "O");

    gate.add_cell("u_out", CellKind::Lut6 { init: 0x7777_7777_7777_7777 });
    gate.connect("n_nand", "u_out", "I0");
    gate.connect("n_xnor", "u_out", "I1");
    gate.connect("y", "u_out", "O");

    assert_eq!(check_equivalence(&gold, &gate), LecResult::Equivalent);

    // Mutate 1 bit of `u_out` INIT and verify LEC returns a concrete counterexample!
    if let CellKind::Lut6 { init } = &mut gate.cell_mut("u_out").unwrap().kind {
        *init ^= 1; // flip output when n_nand=0, n_xnor=0 (which occurs when a=1, b=1, c=1, d=0)
    }
    let res = check_equivalence(&gold, &gate);
    let cex = res.counterexample().expect("must detect 1-bit LUT mutation");
    let in_map = cex.inputs();
    assert!(in_map["a"] && in_map["b"], "counterexample must set a=1, b=1");
    assert_ne!(in_map["c"], in_map["d"], "counterexample must set c ^ d == 1");
    assert_eq!(cex.violated_signals, vec!["y".to_string()]);
}

#[test]
fn lec_proves_carry4_adder_equivalent_to_lut6_adder_and_muxf7() {
    // 4-bit incrementer implemented with Carry4 primitive vs 4 LUT6 cells (`INC4_INIT`).
    let mut d_lut = Design::new("inc4_lut");
    let mut d_carry = Design::new("inc4_carry");
    for d in [&mut d_lut, &mut d_carry] {
        for i in 0..4 {
            d.add_port(&format!("q{i}"), PortDir::In);
            d.add_port(&format!("sum{i}"), PortDir::Out);
        }
    }

    for i in 0..4 {
        d_lut.add_cell(&format!("lut{i}"), CellKind::Lut6 { init: INC4_INIT[i] });
        for pin in 0..=i {
            d_lut.connect(&format!("q{pin}"), &format!("lut{i}"), &format!("I{pin}"));
        }
        d_lut.connect(&format!("sum{i}"), &format!("lut{i}"), "O");
    }

    // Carry4 incrementer: `q + 1`:
    // bit 0: S0 = ~q0, DI0 = q0, CYINIT = 0 -> O0 = ~q0 ^ 0 = ~q0, CO0 = ~q0 ? 0 : q0 = q0!
    // bits 1..3: S_i = q_i, DI_i = 0 -> O_i = q_i ^ CO_{i-1}, CO_i = q_i ? CO_{i-1} : 0 = q_i & CO_{i-1}!
    d_carry.add_cell("inv0", CellKind::Lut6 { init: 0x5555_5555_5555_5555 });
    d_carry.connect("q0", "inv0", "I0");
    d_carry.connect("s0", "inv0", "O");

    d_carry.add_cell("u_carry", CellKind::Carry4);
    d_carry.connect("s0", "u_carry", "S0");
    d_carry.connect("q0", "u_carry", "DI0");
    for i in 1..4 {
        d_carry.connect(&format!("q{i}"), "u_carry", &format!("S{i}"));
    }
    for i in 0..4 {
        d_carry.connect(&format!("sum{i}"), "u_carry", &format!("O{i}"));
    }

    assert_eq!(check_equivalence(&d_lut, &d_carry), LecResult::Equivalent);

    // Also test MUXF7 BlackBox primitive vs equivalent LUT6 2:1 mux (`S ? I1 : I0`).
    let mut d_mux_bb = Design::new("mux_bb");
    let mut d_mux_lut = Design::new("mux_lut");
    for d in [&mut d_mux_bb, &mut d_mux_lut] {
        for p in ["d0", "d1", "sel"] {
            d.add_port(p, PortDir::In);
        }
        d.add_port("y", PortDir::Out);
    }
    d_mux_bb.add_cell(
        "u_muxf7",
        CellKind::BlackBox {
            module: helion_ir::intern("MUXF7"),
        },
    );
    d_mux_bb.connect("d0", "u_muxf7", "I0");
    d_mux_bb.connect("d1", "u_muxf7", "I1");
    d_mux_bb.connect("sel", "u_muxf7", "S");
    d_mux_bb.connect("y", "u_muxf7", "O");

    d_mux_lut.add_cell("u_lut", CellKind::Lut6 { init: 0xCACA_CACA_CACA_CACA });
    d_mux_lut.connect("d0", "u_lut", "I0");
    d_mux_lut.connect("d1", "u_lut", "I1");
    d_mux_lut.connect("sel", "u_lut", "I2");
    d_mux_lut.connect("y", "u_lut", "O");

    assert_eq!(check_equivalence(&d_mux_bb, &d_mux_lut), LecResult::Equivalent);
}

#[test]
fn sequential_lec_proves_retimed_and_replicated_pipelines_equivalent() {
    // Gold: `a, b -> lut_and -> ff_out -> y` (1 FF at output)
    let mut gold = Design::new("pipe_gold");
    for p in ["clk", "a", "b"] {
        gold.add_port(p, PortDir::In);
    }
    gold.add_port("y", PortDir::Out);

    gold.add_cell("lut_and", CellKind::Lut6 { init: 0x8888_8888_8888_8888 });
    gold.connect("a", "lut_and", "I0");
    gold.connect("b", "lut_and", "I1");
    gold.connect("d_and", "lut_and", "O");

    gold.add_cell("ff_out", CellKind::Hff);
    gold.connect("clk", "ff_out", "CLK");
    gold.connect("d_and", "ff_out", "D");
    gold.connect("y", "ff_out", "Q");

    // Gate: Backward-retimed version (`a -> ff_a`, `b -> ff_b`, `ff_a.Q & ff_b.Q -> lut_and -> y`, 2 FFs at inputs!)
    let mut gate = Design::new("pipe_retimed");
    for p in ["clk", "a", "b"] {
        gate.add_port(p, PortDir::In);
    }
    gate.add_port("y", PortDir::Out);

    gate.add_cell("ff_a", CellKind::Hff);
    gate.connect("clk", "ff_a", "CLK");
    gate.connect("a", "ff_a", "D");
    gate.connect("qa", "ff_a", "Q");

    gate.add_cell("ff_b", CellKind::Hff);
    gate.connect("clk", "ff_b", "CLK");
    gate.connect("b", "ff_b", "D");
    gate.connect("qb", "ff_b", "Q");

    gate.add_cell("lut_and", CellKind::Lut6 { init: 0x8888_8888_8888_8888 });
    gate.connect("qa", "lut_and", "I0");
    gate.connect("qb", "lut_and", "I1");
    gate.connect("y", "lut_and", "O");

    // Even though `gold` has 1 FF (`ff_out`) and `gate` has 2 FFs (`ff_a`, `ff_b`),
    // `check_equivalence` proves them 100% sequentially equivalent across all cycles via k-induction!
    assert_eq!(check_equivalence(&gold, &gate), LecResult::Equivalent);
}

#[test]
fn bmc_and_k_induction_prove_safe_invariant_and_catch_multi_cycle_violation() {
    // 3-bit modulo-6 counter (counts 0 -> 1 -> 2 -> 3 -> 4 -> 5 -> 0).
    // States 6 (`110`) and 7 (`111`) are unreachable!
    // Safety property: `!(q2 & q1)` (i.e. `bad = q2 & q1` is always 0).
    let mut d = Design::new("mod6_counter");
    d.add_port("clk", PortDir::In);
    d.add_port("bad", PortDir::Out);

    // Next-state truth tables over (I0=q0, I1=q1, I2=q2) for mod-6 sequence 0->1->2->3->4->5->0:
    // addr 0 (000) -> 001 (d0=1, d1=0, d2=0)
    // addr 1 (001) -> 010 (d0=0, d1=1, d2=0)
    // addr 2 (010) -> 011 (d0=1, d1=1, d2=0)
    // addr 3 (011) -> 100 (d0=0, d1=0, d2=1)
    // addr 4 (100) -> 101 (d0=1, d1=0, d2=1)
    // addr 5 (101) -> 000 (d0=0, d1=0, d2=0)
    // addr 6, 7    -> 000
    let init_d0 = 0b0001_0101u64;
    let init_d1 = 0b0000_0110u64;
    let init_d2 = 0b0001_1000u64;

    for (i, init) in [(0, init_d0), (1, init_d1), (2, init_d2)] {
        let lut = format!("lut_d{i}");
        let ff = format!("ff_q{i}");
        d.add_cell(&lut, CellKind::Lut6 { init });
        d.connect("q0", &lut, "I0");
        d.connect("q1", &lut, "I1");
        d.connect("q2", &lut, "I2");
        d.connect(&format!("d{i}"), &lut, "O");

        d.add_cell(&ff, CellKind::Hff);
        d.connect("clk", &ff, "CLK");
        d.connect(&format!("d{i}"), &ff, "D");
        d.connect(&format!("q{i}"), &ff, "Q");
    }

    // `bad = q2 & q1` (true only in states 6 and 7, which are unreachable)
    d.add_cell("lut_bad", CellKind::Lut6 { init: 0x8888_8888_8888_8888 });
    d.connect("q1", "lut_bad", "I0");
    d.connect("q2", "lut_bad", "I1");
    d.connect("bad", "lut_bad", "O");

    // `reach5 = q2 & ~q1 & q0` (state 5 = `101`, reached at cycle 5!)
    d.add_port("reach5", PortDir::Out);
    let mut init_reach5 = 0u64;
    for addr in 0..64u64 {
        if (addr & 0b111) == 0b101 {
            init_reach5 |= 1u64 << addr;
        }
    }
    d.add_cell("lut_r5", CellKind::Lut6 { init: init_reach5 });
    d.connect("q0", "lut_r5", "I0");
    d.connect("q1", "lut_r5", "I1");
    d.connect("q2", "lut_r5", "I2");
    d.connect("reach5", "lut_r5", "O");

    // 1. Bounded Model Checking & k-Induction on `!bad`:
    let bmc_ok = run_bmc(&d, "!bad", 10);
    assert_eq!(bmc_ok, BmcResult::BoundedSafe { bound: 10 });

    let ind_ok = run_k_induction(&d, "!bad", 6);
    assert!(
        ind_ok.is_proven(),
        "k-induction must prove `!bad` unboundedly safe, got {ind_ok:?}"
    );

    // 2. Bounded Model Checking on `!reach5` (violated at exact cycle depth = 5):
    let bmc_fail = run_bmc(&d, SafetyProperty::NetLow("reach5".into()), 8);
    match bmc_fail {
        BmcResult::Violated { depth, cex } => {
            assert_eq!(depth, 5, "state 5 (101) is reached on cycle 5");
            assert_eq!(cex.cycle_states.len(), 6);
        }
        other => panic!("expected violation at depth 5, got {other:?}"),
    }
}

#[test]
fn ic3_pdr_proves_safety_and_emits_counterexample_vcd_on_violation() {
    // 3-bit guarded shift/state machine with input `en`:
    // When `q2 & q1` is unreachable because `q1` and `q2` are mutually exclusive one-hot bits:
    // d0 = en & ~q0 & ~q1 & ~q2
    // d1 = q0 & ~q2
    // d2 = q1 & ~q0
    // Notice: from (0,0,0), reachable states are (0,0,0), (1,0,0), (0,1,0), (0,0,1), (1,0,1).
    // State `q1 & q2` is NEVER reachable, even though an arbitrary state (1, 1, 0) could transition to (0, 1, 1)!
    // Thus 0-step induction fails, requiring IC3/PDR frame blocking to strengthen the invariant (`~(q0 & q1)`)!
    let mut d = Design::new("pdr_onehot");
    d.add_port("clk", PortDir::In);
    d.add_port("en", PortDir::In);
    d.add_port("err", PortDir::Out);
    d.add_port("stage2", PortDir::Out);

    let mut init_d0 = 0u64;
    let mut init_d1 = 0u64;
    let mut init_d2 = 0u64;
    for addr in 0..64u64 {
        let q0 = addr & 1;
        let q1 = (addr >> 1) & 1;
        let q2 = (addr >> 2) & 1;
        let en = (addr >> 3) & 1;
        let d0 = en & (1 ^ q0) & (1 ^ q1) & (1 ^ q2);
        let d1 = q0 & (1 ^ q2);
        let d2 = q1 & (1 ^ q0);
        init_d0 |= (d0 & 1) << addr;
        init_d1 |= (d1 & 1) << addr;
        init_d2 |= (d2 & 1) << addr;
    }

    for (i, init) in [(0, init_d0), (1, init_d1), (2, init_d2)] {
        let lut = format!("lut_d{i}");
        let ff = format!("ff_q{i}");
        d.add_cell(&lut, CellKind::Lut6 { init });
        d.connect("q0", &lut, "I0");
        d.connect("q1", &lut, "I1");
        d.connect("q2", &lut, "I2");
        d.connect("en", &lut, "I3");
        d.connect(&format!("d{i}"), &lut, "O");

        d.add_cell(&ff, CellKind::Hff);
        d.connect("clk", &ff, "CLK");
        d.connect(&format!("d{i}"), &ff, "D");
        d.connect(&format!("q{i}"), &ff, "Q");
    }

    // `err = q1 & q2` (unreachable!)
    d.add_cell("lut_err", CellKind::Lut6 { init: 0x8888_8888_8888_8888 });
    d.connect("q1", "lut_err", "I0");
    d.connect("q2", "lut_err", "I1");
    d.connect("err", "lut_err", "O");

    // `stage2 = q2` (reachable in 3 cycles when `en = 1` at cycle 0!)
    d.add_cell("lut_s2", CellKind::Lut6 { init: 0xAAAA_AAAA_AAAA_AAAA });
    d.connect("q2", "lut_s2", "I0");
    d.connect("stage2", "lut_s2", "O");

    // 1. Prove `!err` unboundedly safe via IC3/PDR:
    let pdr_safe = run_ic3_pdr(&d, "!err", 12);
    assert!(
        pdr_safe.is_safe(),
        "IC3/PDR must converge on an inductive invariant proving `!err`, got {pdr_safe:?}"
    );

    // 2. Falsify `!stage2` via IC3/PDR and emit a valid multi-cycle `.vcd` waveform trace:
    let pdr_unsafe = run_ic3_pdr(&d, "!stage2", 12);
    let cex = match pdr_unsafe {
        PdrResult::Unsafe { cex } => cex,
        other => panic!("expected IC3/PDR to find counterexample for `!stage2`, got {other:?}"),
    };
    assert!(
        cex.cycle_inputs.len() >= 4,
        "reaching q2=1 from 000 requires at least 4 timeframes (t=0..3), got {}",
        cex.cycle_inputs.len()
    );
    assert_eq!(cex.cycle_inputs[0].get("en"), Some(&true));

    let vcd = emit_cex_vcd(&d, &cex);
    assert!(vcd.contains("$timescale 1ns $end"));
    assert!(vcd.contains("$scope module pdr_onehot $end"));
    assert!(vcd.contains("$var wire 1"));
    assert!(vcd.contains("$dumpvars"));
    assert!(vcd.contains("#0\n"));
    assert!(vcd.contains("#10\n"));
    assert!(vcd.contains("#20\n"));
    assert!(vcd.contains("#30\n"));
    assert_eq!(cex.to_vcd(&d), vcd);
}
