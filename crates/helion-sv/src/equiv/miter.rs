//! Miter construction: XOR each golden/revised output pair, OR-reduce the
//! differences, and unit-clause the result. SAT => the designs differ;
//! UNSAT => they are combinationally equivalent.
//!
//! PI variables are shared **by name** across both AIGs (via the encoder's
//! `pi_vars` map): that sharing is what makes the miter a miter. Output
//! pairs are matched by name in golden output order. PI-set or output-name
//! mismatches hard-fail ([`EquivError::PiMismatch`] / [`OutputMismatch`]):
//! no renaming, no don't-cares.

use crate::equiv::cnf::{signed_lit, CnfEncoder};
use crate::equiv::model::EquivModel;
use crate::equiv::EquivError;
use crate::Lit;
use std::collections::{HashMap, HashSet};

/// One golden/revised output pair in the miter.
#[derive(Clone, Debug)]
pub struct OutPair {
    /// Output name (top port or `ff.<cell>.D` pseudo-output).
    pub name: String,
    /// Golden side literal (for counterexample re-evaluation).
    pub golden: Lit,
    /// Revised side literal (for counterexample re-evaluation).
    pub revised: Lit,
    /// Golden side signed CNF literal.
    pub golden_cnf: i32,
    /// Revised side signed CNF literal.
    pub revised_cnf: i32,
}

/// Built miter: CNF encoder holding both AIGs plus the difference circuit.
#[derive(Debug)]
pub struct Miter {
    pub encoder: CnfEncoder,
    /// Shared PI name -> DIMACS variable.
    pub pi_vars: HashMap<String, u32>,
    /// PI names in deterministic (golden input) order, for counterexamples.
    pub pi_order: Vec<String>,
    /// Output pairs in golden output order.
    pub out_pairs: Vec<OutPair>,
    /// DIMACS variable OR-reducing the per-output differences (0 when there
    /// are no outputs; `prove` short-circuits that vacuous case).
    pub miter_var: u32,
}

impl Miter {
    pub fn build(golden: &EquivModel, revised: &EquivModel) -> Result<Miter, EquivError> {
        // PI sets must match exactly (as sets; order is irrelevant since
        // variables are shared by name).
        let g_pis: HashSet<&str> = golden.inputs.iter().map(|s| s.as_str()).collect();
        let r_pis: HashSet<&str> = revised.inputs.iter().map(|s| s.as_str()).collect();
        if g_pis != r_pis {
            return Err(EquivError::PiMismatch);
        }

        // Output names must match exactly, matched by name.
        let g_names: HashSet<&str> =
            golden.outputs.iter().map(|(n, _)| n.as_str()).collect();
        let r_names: HashSet<&str> =
            revised.outputs.iter().map(|(n, _)| n.as_str()).collect();
        if g_names != r_names || g_names.len() != golden.outputs.len() {
            return Err(EquivError::OutputMismatch);
        }
        let r_lits: HashMap<&str, Lit> =
            revised.outputs.iter().map(|(n, l)| (n.as_str(), *l)).collect();

        let mut encoder = CnfEncoder::new();
        let mut pi_vars: HashMap<String, u32> = HashMap::new();
        let g_node_lits = encoder.encode_aig(&golden.aig, &mut pi_vars);
        let r_node_lits = encoder.encode_aig(&revised.aig, &mut pi_vars);

        let mut out_pairs = Vec::with_capacity(golden.outputs.len());
        let mut diffs = Vec::with_capacity(golden.outputs.len());
        for (name, gl) in &golden.outputs {
            let Some(&rl) = r_lits.get(name.as_str()) else {
                return Err(EquivError::OutputMismatch);
            };
            let gs = signed_lit(g_node_lits[gl.node as usize], gl.inv);
            let rs = signed_lit(r_node_lits[rl.node as usize], rl.inv);
            let w = encoder.encode_xor(gs, rs);
            diffs.push(w);
            out_pairs.push(OutPair {
                name: name.clone(),
                golden: *gl,
                revised: rl,
                golden_cnf: gs,
                revised_cnf: rs,
            });
        }

        let miter_var = if diffs.is_empty() {
            0
        } else {
            let m = encoder.encode_or(&diffs);
            encoder.assert_unit(m);
            m as u32
        };

        Ok(Miter {
            encoder,
            pi_vars,
            pi_order: golden.inputs.clone(),
            out_pairs,
            miter_var,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Aig, Lit as AigLit};

    fn two_pi_aig() -> (Aig, AigLit, AigLit) {
        let mut a = Aig::with_pis(Vec::new());
        let x = a.pi_lit("x");
        let y = a.pi_lit("y");
        (a, x, y)
    }

    #[test]
    fn miter_pi_mismatch_fails() {
        let (mut ga, gx, gy) = two_pi_aig();
        let go = ga.and_lit(gx, gy);
        let golden =
            EquivModel::from_merged_cones(&[("o".to_string(), ga_with_out(ga, go))]);
        let (mut ra, rx, _) = two_pi_aig();
        let rz = ra.pi_lit("z");
        let ro = ra.and_lit(rx, rz);
        let revised =
            EquivModel::from_merged_cones(&[("o".to_string(), ga_with_out(ra, ro))]);
        let err = Miter::build(&golden, &revised).unwrap_err();
        assert_eq!(err, EquivError::PiMismatch);
    }

    #[test]
    fn miter_output_mismatch_fails() {
        let (mut ga, gx, gy) = two_pi_aig();
        let go = ga.and_lit(gx, gy);
        let golden =
            EquivModel::from_merged_cones(&[("o".to_string(), ga_with_out(ga, go))]);
        let (mut ra, rx, ry) = two_pi_aig();
        let ro = ra.and_lit(rx, ry);
        let revised =
            EquivModel::from_merged_cones(&[("p".to_string(), ga_with_out(ra, ro))]);
        let err = Miter::build(&golden, &revised).unwrap_err();
        assert_eq!(err, EquivError::OutputMismatch);
    }

    #[test]
    fn miter_output_count_mismatch_fails() {
        let (mut ga, gx, gy) = two_pi_aig();
        let go = ga.and_lit(gx, gy);
        let golden =
            EquivModel::from_merged_cones(&[("o".to_string(), ga_with_out(ga, go))]);
        let (mut ra, rx, ry) = two_pi_aig();
        let ro = ra.and_lit(rx, ry);
        let revised = EquivModel::from_merged_cones(&[
            ("o".to_string(), ga_with_out(ra.clone(), ro)),
            ("o2".to_string(), ga_with_out(ra, rx)),
        ]);
        let err = Miter::build(&golden, &revised).unwrap_err();
        assert_eq!(err, EquivError::OutputMismatch);
    }

    #[test]
    fn miter_pairs_by_name_not_order() {
        // Golden [a, b], revised [b, a]: pairing is by name, so the miter is
        // still well-formed (and UNSAT for identical cones).
        let (mut ga, gx, gy) = two_pi_aig();
        let go1 = ga.and_lit(gx, gy);
        let go2 = ga.and_lit(gx.not(), gy);
        let golden = EquivModel::from_merged_cones(&[
            ("a".to_string(), ga_with_out(ga.clone(), go1)),
            ("b".to_string(), ga_with_out(ga, go2)),
        ]);
        let (mut ra, rx, ry) = two_pi_aig();
        let ro1 = ra.and_lit(rx, ry);
        let ro2 = ra.and_lit(rx.not(), ry);
        let revised = EquivModel::from_merged_cones(&[
            ("b".to_string(), ga_with_out(ra.clone(), ro2)),
            ("a".to_string(), ga_with_out(ra, ro1)),
        ]);
        let miter = Miter::build(&golden, &revised).expect("miter build");
        assert_eq!(miter.out_pairs.len(), 2);
        assert_eq!(miter.out_pairs[0].name, "a");
        assert_eq!(miter.out_pairs[1].name, "b");
    }

    /// Set an AIG's single output literal (helper: cones are built with
    /// `with_pis` + manual ANDs, so `output` is unused by the miter path;
    /// keep the field consistent anyway).
    fn ga_with_out(mut a: Aig, out: AigLit) -> Aig {
        a.output = out;
        a
    }
}
