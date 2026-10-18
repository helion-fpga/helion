use helion_ir::{CellKind, Design, PortDir};
use helion_lib::sby::{
    add_formal_assert, add_formal_assume, add_formal_cover, add_formal_fair, emit_aiger_ascii,
    emit_btor2, emit_smtlib2_qfbv, parse_aiger_ascii, parse_btor2, parse_sby_config, run_sby_task,
    set_ff_init, ProofMethod, SbyEngine, SbyMode, SbyStatus,
};

#[test]
fn sby_config_parser_handles_tasks_options_engines_script_and_files() {
    let sby_src = r#"
# SymbiYosys formal verification configuration
[tasks]
bmc_task bmc
prove_task prove
cover_task cover

[options]
bmc: mode bmc
bmc: depth 12
prove: mode prove
prove: depth 16
cover: mode cover
cover: depth 10
multiclock on
wait on

[engines]
bmc: smtbmc --nopresat z3
prove: btor btormc
cover: aiger avy

[script]
read -formal fifo.sv
prep -top fifo_ctrl

[files]
rtl/fifo.sv
formal/fifo_props.sv rtl/fifo_props.sv

[file defs.vh]
`define FIFO_DEPTH 8
"#;

    let cfg = parse_sby_config(sby_src).expect("parse_sby_config");
    assert_eq!(cfg.tasks.len(), 3);

    let t_bmc = cfg.task("bmc_task").expect("bmc_task");
    assert_eq!(t_bmc.options.mode, SbyMode::Bmc);
    assert_eq!(t_bmc.options.depth, 12);
    assert!(t_bmc.options.multiclock);
    assert!(t_bmc.options.wait);
    assert_eq!(t_bmc.top_module.as_deref(), Some("fifo_ctrl"));
    assert_eq!(
        t_bmc.engines,
        vec![SbyEngine::SmtBmc {
            solver: Some("z3".into()),
            args: vec!["--nopresat".into()],
        }]
    );

    let t_prove = cfg.task("prove_task").expect("prove_task");
    assert_eq!(t_prove.options.mode, SbyMode::Prove);
    assert_eq!(t_prove.options.depth, 16);
    assert_eq!(
        t_prove.engines,
        vec![SbyEngine::Btor {
            solver: Some("btormc".into()),
            args: Vec::new(),
        }]
    );

    let t_cover = cfg.task("cover_task").expect("cover_task");
    assert_eq!(t_cover.options.mode, SbyMode::Cover);
    assert_eq!(t_cover.options.depth, 10);
    assert_eq!(
        t_cover.engines,
        vec![SbyEngine::Aiger {
            solver: Some("avy".into()),
            args: Vec::new(),
        }]
    );

    assert_eq!(cfg.files.len(), 2);
    assert_eq!(
        cfg.embedded_files.get("defs.vh").map(|s| s.as_str()),
        Some("`define FIFO_DEPTH 8")
    );
}

/// Build a 2-bit saturating / guarded counter with LUT6, Carry4, HFF, Assume, Assert, Cover, Fair.
fn build_guarded_counter_design() -> Design {
    let mut d = Design::new("guarded_cnt");
    d.add_port("clk", PortDir::In);
    d.add_port("inc", PortDir::In);
    d.add_port(" forbid_wrap", PortDir::In);
    d.add_port("overflow_o", PortDir::Out);

    // Two state bits q0, q1 (initialized to 0)
    d.add_cell("ff0", CellKind::Hff);
    d.connect("clk", "ff0", "CLK");
    d.connect("d0", "ff0", "D");
    d.connect("q0", "ff0", "Q");
    set_ff_init(&mut d, "ff0", false);

    d.add_cell("ff1", CellKind::Hff);
    d.connect("clk", "ff1", "CLK");
    d.connect("d1", "ff1", "D");
    d.connect("q1", "ff1", "Q");
    set_ff_init(&mut d, "ff1", false);

    // Use a Carry4 cell to compute {q1, q0} + inc
    // Stage 0: S0 = q0 ^ inc, DI0 = q0 (so CO0 = q0 & inc)
    // Stage 1: S1 = q1, DI1 = 0 -> O1 = q1 ^ CO0
    d.add_cell(
        "lut_s0",
        CellKind::Lut6 {
            init: 0x6666_6666_6666_6666, // I0 ^ I1
        },
    );
    d.connect("q0", "lut_s0", "I0");
    d.connect("inc", "lut_s0", "I1");
    d.connect("s0", "lut_s0", "O");

    d.add_cell("u_carry", CellKind::Carry4);
    d.connect("s0", "u_carry", "S0");
    d.connect("inc", "u_carry", "DI0");
    d.connect("d0", "u_carry", "O0");
    d.connect("q1", "u_carry", "S1");
    d.connect("d1", "u_carry", "O1");

    // at_max = q0 & q1 (state == 2'b11)
    d.add_cell(
        "lut_max",
        CellKind::Lut6 {
            init: 0x8888_8888_8888_8888, // I0 & I1
        },
    );
    d.connect("q0", "lut_max", "I0");
    d.connect("q1", "lut_max", "I1");
    d.connect("at_max", "lut_max", "O");

    // safe_cond = ~at_max (assert we never reach 2'b11)
    d.add_cell(
        "lut_safe",
        CellKind::Lut6 {
            init: 0x5555_5555_5555_5555, // ~I0
        },
    );
    d.connect("at_max", "lut_safe", "I0");
    d.connect("not_max", "lut_safe", "O");

    // Assume: when q1 == 1, inc must be 0 (`~(q1 & inc)`)
    d.add_cell(
        "lut_assume",
        CellKind::Lut6 {
            init: 0x7777_7777_7777_7777, // ~(I0 & I1)
        },
    );
    d.connect("q1", "lut_assume", "I0");
    d.connect("inc", "lut_assume", "I1");
    d.connect("no_inc_when_q1", "lut_assume", "O");

    d.add_cell("u_iob", CellKind::IobOut);
    d.connect("at_max", "u_iob", "I");
    d.connect("overflow_o", "u_iob", "PAD");

    add_formal_assert(&mut d, "never_reach_3", "not_max");
    add_formal_cover(&mut d, "reach_state_2", "q1");
    add_formal_fair(&mut d, "inc_fair", "inc");
    d
}

#[test]
fn btor2_aiger_and_smtlib2_emitters_produce_valid_models() {
    let mut d = build_guarded_counter_design();
    add_formal_assume(&mut d, "stop_at_2", "no_inc_when_q1");

    // 1. BTOR2 emission & round-trip parse
    let btor2 = emit_btor2(&d).expect("emit_btor2");
    assert!(btor2.contains("1 sort bitvec 1"), "{btor2}");
    assert!(btor2.contains(" state 1 q0"), "{btor2}");
    assert!(btor2.contains(" state 1 q1"), "{btor2}");
    assert!(btor2.contains(" init 1 "), "{btor2}");
    assert!(btor2.contains(" next 1 "), "{btor2}");
    assert!(btor2.contains(" constraint "), "{btor2}");
    assert!(btor2.contains(" bad "), "{btor2}");
    assert!(btor2.contains(" fair "), "{btor2}");
    let parsed_btor = parse_btor2(&btor2).expect("parse_btor2");
    assert!(parsed_btor.iter().any(|l| l.op == "sort"));
    assert_eq!(parsed_btor.iter().filter(|l| l.op == "state").count(), 2);
    assert_eq!(parsed_btor.iter().filter(|l| l.op == "bad").count(), 1);
    assert_eq!(
        parsed_btor.iter().filter(|l| l.op == "constraint").count(),
        1
    );
    assert_eq!(parsed_btor.iter().filter(|l| l.op == "fair").count(), 1);

    // 2. AIGER 1.9 ASCII (.aag) emission & round-trip parse
    let aag = emit_aiger_ascii(&d).expect("emit_aiger_ascii");
    assert!(aag.starts_with("aag "), "{aag}");
    let parsed_aag = parse_aiger_ascii(&aag).expect("parse_aiger_ascii");
    assert_eq!(parsed_aag.num_latches, 2);
    assert_eq!(parsed_aag.num_bads, 1);
    assert_eq!(parsed_aag.num_constraints, 1);
    assert!(parsed_aag.num_ands >= 2);
    for &(lhs, r0, r1) in &parsed_aag.ands {
        assert_eq!(lhs % 2, 0, "AIGER AND lhs must be even literal");
        assert!(lhs > r0 && lhs > r1, "AIGER AND gates must be topologically ordered");
    }

    // 3. SMT-LIB v2 QF_BV emission
    let smt2 = emit_smtlib2_qfbv(&d, 4).expect("emit_smtlib2_qfbv");
    assert!(smt2.contains("(set-logic QF_BV)"), "{smt2}");
    assert!(smt2.contains("(_ BitVec 1)"), "{smt2}");
    assert!(smt2.contains("(check-sat)"), "{smt2}");
}

#[test]
fn run_sby_task_bmc_finds_counterexample_without_assume_and_passes_with_assume() {
    let sby_src = r#"
[tasks]
bmc_check
prove_check
cover_check

[options]
bmc_check: mode bmc
bmc_check: depth 6
prove_check: mode prove
prove_check: depth 6
cover_check: mode cover
cover_check: depth 6

[engines]
smtbmc z3

[script]
prep -top guarded_cnt

[files]
guarded_cnt.sv
"#;
    let cfg = parse_sby_config(sby_src).expect("parse sby");

    // Without the `assume(no_inc_when_q1)` constraint, the counter reaches 3 (`q1=1, q0=1`) in 3 cycles!
    let d_unconstrained = build_guarded_counter_design();
    let res_fail =
        run_sby_task(&cfg, Some("bmc_check"), &d_unconstrained).expect("run bmc unconstrained");
    assert_eq!(res_fail.status, SbyStatus::Fail);
    assert_eq!(res_fail.failed_assertions, vec!["never_reach_3"]);
    let cex = res_fail.counterexample.expect("counterexample trace");
    assert_eq!(cex.property, "never_reach_3");
    assert_eq!(
        cex.steps.len(),
        4,
        "0 -> 1 -> 2 -> 3 takes 3 transitions (4 states: cycle 0..=3)"
    );
    assert_eq!(cex.steps[0].states.get("q0"), Some(&false));
    assert_eq!(cex.steps[0].states.get("q1"), Some(&false));
    assert_eq!(cex.steps[3].states.get("q0"), Some(&true));
    assert_eq!(cex.steps[3].states.get("q1"), Some(&true));

    // With the `assume(no_inc_when_q1)` constraint active, state 3 is unreachable!
    let mut d_constrained = build_guarded_counter_design();
    add_formal_assume(&mut d_constrained, "stop_at_2", "no_inc_when_q1");

    let res_bmc_pass =
        run_sby_task(&cfg, Some("bmc_check"), &d_constrained).expect("run bmc constrained");
    assert_eq!(res_bmc_pass.status, SbyStatus::Pass);

    // Unbounded proof (`prove` mode) proves `never_reach_3` holds for all time!
    let res_prove =
        run_sby_task(&cfg, Some("prove_check"), &d_constrained).expect("run prove constrained");
    assert_eq!(res_prove.status, SbyStatus::Pass);
    assert!(matches!(
        res_prove.method,
        Some(ProofMethod::KInduction { .. } | ProofMethod::Pdr { .. })
    ));

    // `cover` mode generates a concrete witness trace reaching `q1 == 1` (state 2) in 2 transitions!
    let res_cover =
        run_sby_task(&cfg, Some("cover_check"), &d_constrained).expect("run cover constrained");
    assert_eq!(res_cover.status, SbyStatus::Pass);
    assert_eq!(res_cover.cover_traces.len(), 1);
    let cov_trace = &res_cover.cover_traces[0];
    assert_eq!(cov_trace.property, "reach_state_2");
    assert_eq!(cov_trace.steps.last().unwrap().states.get("q1"), Some(&true));
}

#[test]
fn run_sby_task_prove_pdr_handles_non_1_inductive_onehot_fsm() {
    // 3-state one-hot ring (s0=1, s1=0, s2=0):
    // Next state: s0' = s2, s1' = s0, s2' = s1.
    // Property: `~(s0 & s1)` (never both s0 and s1 high simultaneously).
    // Note: `~(s0 & s1)` is NOT 1-inductive from an arbitrary state `(s0=0, s1=1, s2=1)`
    // because in 1 step that unreachable state transitions to `(s0=1, s1=1, s2=0)`!
    // It requires k-induction (k >= 2) or PDR reachable-frame invariant strengthening!
    let mut d = Design::new("onehot_ring");
    d.add_port("clk", PortDir::In);

    d.add_cell("ff0", CellKind::Hff);
    d.connect("clk", "ff0", "CLK");
    d.connect("s2", "ff0", "D");
    d.connect("s0", "ff0", "Q");
    set_ff_init(&mut d, "ff0", true);

    d.add_cell("ff1", CellKind::Hff);
    d.connect("clk", "ff1", "CLK");
    d.connect("s0", "ff1", "D");
    d.connect("s1", "ff1", "Q");
    set_ff_init(&mut d, "ff1", false);

    d.add_cell("ff2", CellKind::Hff);
    d.connect("clk", "ff2", "CLK");
    d.connect("s1", "ff2", "D");
    d.connect("s2", "ff2", "Q");
    set_ff_init(&mut d, "ff2", false);

    d.add_cell(
        "lut_mutex",
        CellKind::Lut6 {
            init: 0x7777_7777_7777_7777, // ~(I0 & I1)
        },
    );
    d.connect("s0", "lut_mutex", "I0");
    d.connect("s1", "lut_mutex", "I1");
    d.connect("mutex_ok", "lut_mutex", "O");
    add_formal_assert(&mut d, "onehot_mutex", "mutex_ok");

    let cfg = parse_sby_config(
        r#"
[options]
mode prove
depth 10
[engines]
btor btormc
[script]
prep -top onehot_ring
[files]
onehot_ring.sv
"#,
    )
    .unwrap();

    let res = run_sby_task(&cfg, None, &d).expect("prove onehot_ring");
    assert_eq!(res.status, SbyStatus::Pass);
    assert!(
        match res.method {
            Some(ProofMethod::KInduction { k }) => k >= 2,
            Some(ProofMethod::Pdr { .. }) => true,
            _ => false,
        },
        "expected k-induction (k>=2) or PDR proof, got {:?}",
        res.method
    );
}
