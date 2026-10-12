//! Orchestration: build the miter, solve it once, and on SAT extract a
//! concrete counterexample.
//!
//! Counterexample extraction re-evaluates both AIGs with the tested
//! [`Aig::eval_lit`][crate::Aig::eval_lit] under the SAT model's PI
//! assignment and reports the first output pair that disagrees, so the
//! counterexample never depends on trusting CNF variable bookkeeping. (The
//! brute-force evaluator caps at 64 PIs; wider models read output values
//! back from the SAT model instead.) A SAT model that re-evaluation cannot
//! reproduce is an internal bug and fails loud.

use crate::equiv::miter::Miter;
use crate::equiv::model::EquivModel;
use crate::equiv::solver::{SolverBackend, VarisatBackend};
use crate::equiv::{Counterexample, EquivError, EquivVerdict};
use crate::Aig;
use std::collections::HashMap;

/// Prove `golden` and `revised` combinationally equivalent.
///
/// Returns [`EquivVerdict::Equivalent`] on UNSAT, or a concrete
/// [`Counterexample`] on SAT. Hard-fails on PI/output mismatches and on
/// internal inconsistencies.
pub fn prove_equivalent(
    golden: &EquivModel,
    revised: &EquivModel,
) -> Result<EquivVerdict, EquivError> {
    let miter = Miter::build(golden, revised)?;
    if miter.out_pairs.is_empty() {
        // Vacuous: no observable outputs, nothing can differ.
        return Ok(EquivVerdict::Equivalent);
    }

    let mut backend = VarisatBackend::new(miter.encoder.num_vars());
    for clause in miter.encoder.clauses() {
        backend.add_clause(clause);
    }
    if !backend.solve()? {
        return Ok(EquivVerdict::Equivalent);
    }

    // SAT: read the full PI assignment out of the model.
    let mut inputs: Vec<(String, bool)> = Vec::with_capacity(miter.pi_order.len());
    let mut values: HashMap<&str, bool> = HashMap::with_capacity(miter.pi_order.len());
    for name in &miter.pi_order {
        let var = miter.pi_vars.get(name.as_str()).copied().ok_or_else(|| {
            EquivError::Solver(format!("internal error: no CNF variable for input '{name}'"))
        })?;
        let b = backend.model_value(var).ok_or_else(|| {
            EquivError::Solver(format!("SAT model has no value for input '{name}'"))
        })?;
        values.insert(name.as_str(), b);
        inputs.push((name.clone(), b));
    }

    // First output pair on which the two sides disagree.
    let wide = miter.pi_order.len() > 64;
    let mut differing: Option<(String, bool, bool)> = None;
    for pair in &miter.out_pairs {
        let (gv, rv) = if wide {
            (cnf_output_value(&backend, pair.golden_cnf)?, cnf_output_value(&backend, pair.revised_cnf)?)
        } else {
            (
                golden.aig.eval_lit(pair.golden, pi_bits(&golden.aig, &values)),
                revised.aig.eval_lit(pair.revised, pi_bits(&revised.aig, &values)),
            )
        };
        if gv != rv {
            differing = Some((pair.name.clone(), gv, rv));
            break;
        }
    }

    // A SAT model that re-evaluation cannot reproduce means the encoding
    // lied: fail loud, in every build profile.
    debug_assert!(
        differing.is_some(),
        "SAT model did not reproduce an output disagreement"
    );
    match differing {
        Some((differing_output, golden, revised)) => Ok(EquivVerdict::Counterexample(
            Counterexample {
                inputs,
                differing_output,
                golden,
                revised,
            },
        )),
        None => Err(EquivError::Solver(
            "SAT model did not reproduce an output disagreement (internal error)".into(),
        )),
    }
}

/// `eval_lit` input word for one AIG: bit i = value of `aig.pis[i]`.
fn pi_bits(aig: &Aig, values: &HashMap<&str, bool>) -> u64 {
    let mut bits = 0u64;
    for (i, name) in aig.pis.iter().enumerate() {
        if values.get(name.as_str()).copied().unwrap_or(false) {
            bits |= 1 << i;
        }
    }
    bits
}

/// Output value read back from the SAT model (>64-PI fallback path).
fn cnf_output_value(backend: &VarisatBackend, signed: i32) -> Result<bool, EquivError> {
    let b = backend.model_value(signed.unsigned_abs()).ok_or_else(|| {
        EquivError::Solver("SAT model has no value for an output literal".into())
    })?;
    Ok(if signed > 0 { b } else { !b })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aig_opt::rewrite_aig_multi;
    use crate::{set_aig_rewrite, synth_sv, Lit as SvLit};
    use helion_ir::{CellKind, Design, PortDir};

    /// 16-bit enable-gated counter: real RTL, 18 PIs (clk, en + 16 FF Qs),
    /// 32 outputs (16 count bits + 16 FF D pseudo-outputs).
    const COUNTER16: &str = r#"
module counter16(
    input  logic clk,
    input  logic en,
    output logic [15:0] count
);
    logic [15:0] cnt;
    always_ff @(posedge clk) begin
        if (en) cnt <= cnt + 16'd1;
    end
    assign count = cnt;
endmodule
"#;

    fn synth_counter16(rewrite: bool) -> Design {
        set_aig_rewrite(rewrite);
        let d = synth_sv(COUNTER16, "counter16.sv").expect("counter16 synth");
        set_aig_rewrite(true);
        d
    }

    #[test]
    fn prove_identical_cones_equivalent() {
        let mut a = Aig::with_pis(Vec::new());
        let x = a.pi_lit("x");
        let y = a.pi_lit("y");
        let o = a.and_lit(x, y.not());
        let golden = EquivModel::from_merged_cones(&[("o".to_string(), {
            let mut c = a.clone();
            c.output = o;
            c
        })]);
        let revised = golden.clone();
        assert_eq!(
            prove_equivalent(&golden, &revised).expect("prove"),
            EquivVerdict::Equivalent
        );
    }

    #[test]
    fn prove_rewrite_aig_multi_equivalent() {
        // Pre/post rewrite_aig_multi on real 16-bit next-state cones
        // (beyond the 12-PI exhaustive battery): the exact pair SAT fills.
        let design = synth_counter16(false);
        let golden = EquivModel::from_design(&design).expect("from_design");
        assert!(
            golden.inputs.len() > 12,
            "expected >12 PIs, got {}",
            golden.inputs.len()
        );
        let out_lits: Vec<SvLit> = golden.outputs.iter().map(|(_, l)| *l).collect();
        let (rw_aig, rw_outs) = rewrite_aig_multi(&golden.aig, &out_lits);
        let revised = EquivModel {
            inputs: golden.inputs.clone(),
            outputs: golden
                .outputs
                .iter()
                .zip(rw_outs)
                .map(|((n, _), l)| (n.clone(), l))
                .collect(),
            aig: rw_aig,
        };
        assert_eq!(
            prove_equivalent(&golden, &revised).expect("prove"),
            EquivVerdict::Equivalent
        );
    }

    #[test]
    fn prove_synth_rewrite_on_off_equivalent() {
        // End to end through the LUT bridge: rewrite disabled at synth vs
        // enabled. Both sides go LUT -> AIG -> CNF -> solver.
        let golden = EquivModel::from_design(&synth_counter16(false)).expect("golden");
        let revised = EquivModel::from_design(&synth_counter16(true)).expect("revised");
        // Sanity: both models are non-trivial (the rewrite may or may not
        // shrink this particular design; equivalence must hold either way).
        assert!(!golden.aig.ands.is_empty());
        assert!(!revised.aig.ands.is_empty());
        assert_eq!(
            prove_equivalent(&golden, &revised).expect("prove"),
            EquivVerdict::Equivalent
        );
    }

    #[test]
    fn prove_inverted_bit_counterexample() {
        // One output bit inverted: must report that bit, self-checked by
        // re-evaluating both AIGs under the reported assignment.
        let design = synth_counter16(true);
        let golden = EquivModel::from_design(&design).expect("from_design");
        let mut revised = golden.clone();
        let victim = 7usize;
        let victim_name = revised.outputs[victim].0.clone();
        revised.outputs[victim].1 = revised.outputs[victim].1.not();
        match prove_equivalent(&golden, &revised).expect("prove") {
            EquivVerdict::Equivalent => panic!("inverted bit must not prove equivalent"),
            EquivVerdict::Counterexample(ce) => {
                assert_eq!(ce.differing_output, victim_name);
                assert_ne!(ce.golden, ce.revised);
                // Self-check: re-run eval_lit under the reported assignment.
                let values: HashMap<&str, bool> =
                    ce.inputs.iter().map(|(n, b)| (n.as_str(), *b)).collect();
                let g_bits = pi_bits(&golden.aig, &values);
                let r_bits = pi_bits(&revised.aig, &values);
                let mut found = false;
                for ((name, gl), (_, rl)) in
                    golden.outputs.iter().zip(revised.outputs.iter())
                {
                    let gv = golden.aig.eval_lit(*gl, g_bits);
                    let rv = revised.aig.eval_lit(*rl, r_bits);
                    if name == &victim_name {
                        assert!(gv != rv, "victim bit must differ under the assignment");
                        assert_eq!(gv, ce.golden);
                        assert_eq!(rv, ce.revised);
                        found = true;
                    }
                }
                assert!(found, "victim output missing from model");
            }
        }
    }

    #[test]
    fn prove_determinism_identical_clauses() {
        // Two from_design builds of the same netlist -> identical clause vecs.
        let design = synth_counter16(true);
        let g = EquivModel::from_design(&design).expect("golden");
        let r = EquivModel::from_design(&design).expect("revised");
        let m1 = Miter::build(&g, &r).expect("miter1");
        let g2 = EquivModel::from_design(&design).expect("golden2");
        let r2 = EquivModel::from_design(&design).expect("revised2");
        let m2 = Miter::build(&g2, &r2).expect("miter2");
        assert_eq!(m1.encoder.clauses(), m2.encoder.clauses());
    }

    /// Hand-built combinational design: `y = a & b`, one IobOut.
    fn and2_design() -> Design {
        let mut d = Design::new("and2");
        d.add_port("a", PortDir::In);
        d.add_port("b", PortDir::In);
        d.add_port("y", PortDir::Out);
        // INIT for O = I0 & I1 (I0 = LSB): only addr 3 -> bit 3 set.
        d.add_cell("u_lut", CellKind::Lut6 { init: 0x8 });
        d.connect("a", "u_lut", "I0");
        d.connect("b", "u_lut", "I1");
        d.connect("w", "u_lut", "O");
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect("w", "u_iob", "I");
        d.connect("y", "u_iob", "PAD");
        d
    }

    #[test]
    fn prove_handbuilt_and2_equivalent() {
        let g = EquivModel::from_design(&and2_design()).expect("golden");
        let r = EquivModel::from_design(&and2_design()).expect("revised");
        assert_eq!(g.inputs, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(g.outputs.len(), 1);
        assert_eq!(g.outputs[0].0, "y");
        assert_eq!(
            prove_equivalent(&g, &r).expect("prove"),
            EquivVerdict::Equivalent
        );
    }

    #[test]
    fn prove_handbuilt_and2_vs_or_counterexample() {
        let g = EquivModel::from_design(&and2_design()).expect("golden");
        // Revised: y = a | b (INIT bit0..2 set: addr 1,2,3).
        let mut rd = and2_design();
        rd.cells
            .iter_mut()
            .find(|c| c.name == "u_lut")
            .expect("u_lut")
            .kind = CellKind::Lut6 { init: 0xE };
        let r = EquivModel::from_design(&rd).expect("revised");
        match prove_equivalent(&g, &r).expect("prove") {
            EquivVerdict::Equivalent => panic!("AND vs OR must differ"),
            EquivVerdict::Counterexample(ce) => {
                assert_eq!(ce.differing_output, "y");
                // AND vs OR differ exactly when one input is 1 and the other 0.
                let get = |n: &str| {
                    ce.inputs
                        .iter()
                        .find(|(m, _)| m == n)
                        .map(|(_, b)| *b)
                };
                match (get("a"), get("b")) {
                    (Some(a), Some(b)) => assert!(
                        a != b,
                        "AND vs OR counterexample needs a != b, got a={a} b={b}"
                    ),
                    _ => panic!("counterexample is missing a PI assignment"),
                }
                assert_eq!(ce.golden, false);
                assert_eq!(ce.revised, true);
            }
        }
    }

    #[test]
    fn prove_unsupported_blackbox_kinds() {
        for (kind, label) in [
            (CellKind::Bram18, "Bram18"),
            (CellKind::Mac27, "Mac27"),
            (CellKind::BlackBox { module: "ip".into() }, "BlackBox"),
            (CellKind::Ila { net: "w".into() }, "Ila"),
        ] {
            let mut d = and2_design();
            d.add_cell("u_bb", kind);
            let err = EquivModel::from_design(&d).unwrap_err();
            match err {
                EquivError::UnsupportedCell(msg) => {
                    assert!(
                        msg.contains("u_bb"),
                        "{label}: message should name the cell, got '{msg}'"
                    );
                }
                other => panic!("{label}: expected UnsupportedCell, got {other:?}"),
            }
        }
    }

    #[test]
    fn prove_error_display() {
        assert_eq!(
            EquivError::PiMismatch.to_string(),
            "PI sets differ between golden and revised models"
        );
        assert_eq!(
            EquivError::OutputMismatch.to_string(),
            "output names differ between golden and revised models"
        );
        assert!(
            EquivError::UnsupportedCell("x".into())
                .to_string()
                .contains('x')
        );
        assert!(
            EquivError::Solver("boom".into())
                .to_string()
                .contains("boom")
        );
    }
}
