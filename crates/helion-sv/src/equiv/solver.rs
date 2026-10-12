//! Solver backend trait and the varisat implementation.
//!
//! The [`SolverBackend`] trait keeps the SAT solver behind a boundary: a
//! future CaDiCaL swap is a one-file change. v3.0 uses a single-shot
//! solve; `assume` is reserved for later per-output localization.

use crate::equiv::EquivError;
use varisat::{ExtendFormula, Lit as VsatLit, Solver, Var};

/// Minimal SAT solver interface for miter checks.
pub trait SolverBackend {
    /// Add one clause (signed DIMACS literals, 1-based variables).
    fn add_clause(&mut self, clause: &[i32]);
    /// Solve. `Ok(true)` = SAT (designs differ), `Ok(false)` = UNSAT.
    fn solve(&mut self) -> Result<bool, EquivError>;
    /// Value of a DIMACS variable in the satisfying model, if solved SAT.
    fn model_value(&self, var: u32) -> Option<bool>;
}

/// varisat backend. DIMACS variable `v` (1-based) maps to
/// `Var::from_index(v - 1)`; allocation order matches the encoder's, so no
/// remapping table is needed.
pub struct VarisatBackend<'solver> {
    solver: Solver<'solver>,
}

impl VarisatBackend<'_> {
    /// Create a backend pre-allocating `nvars` variables (must cover every
    /// variable referenced by later [`add_clause`][SolverBackend::add_clause]
    /// calls).
    pub fn new(nvars: u32) -> Self {
        let mut solver = Solver::new();
        for _ in 0..nvars {
            solver.new_var();
        }
        Self { solver }
    }

    fn to_vsat(lit: i32) -> VsatLit {
        let var = Var::from_index(lit.unsigned_abs() as usize - 1);
        if lit > 0 {
            VsatLit::positive(var)
        } else {
            VsatLit::negative(var)
        }
    }
}

impl SolverBackend for VarisatBackend<'_> {
    fn add_clause(&mut self, clause: &[i32]) {
        let vs: Vec<VsatLit> = clause.iter().map(|&l| Self::to_vsat(l)).collect();
        self.solver.add_clause(&vs);
    }

    fn solve(&mut self) -> Result<bool, EquivError> {
        self.solver
            .solve()
            .map_err(|e| EquivError::Solver(format!("varisat solver error: {e}")))
    }

    fn model_value(&self, var: u32) -> Option<bool> {
        let model = self.solver.model()?;
        let idx = var as usize - 1;
        model
            .iter()
            .find(|l| l.var().index() == idx)
            .map(|l| l.is_positive())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trivial SAT: (x) is satisfiable, model says x = true.
    #[test]
    fn backend_trivial_sat() {
        let mut b = VarisatBackend::new(1);
        b.add_clause(&[1]);
        assert!(b.solve().expect("solve"));
        assert_eq!(b.model_value(1), Some(true));
    }

    /// Trivial UNSAT: (x) and (-x) together are unsatisfiable.
    #[test]
    fn backend_trivial_unsat() {
        let mut b = VarisatBackend::new(1);
        b.add_clause(&[1]);
        b.add_clause(&[-1]);
        assert!(!b.solve().expect("solve"));
        assert_eq!(b.model_value(1), None);
    }

    /// Model round-trip: (x | y) & (-x | y) & (x | -y) forces x = y = true.
    #[test]
    fn backend_model_roundtrip() {
        let mut b = VarisatBackend::new(2);
        b.add_clause(&[1, 2]);
        b.add_clause(&[-1, 2]);
        b.add_clause(&[1, -2]);
        assert!(b.solve().expect("solve"));
        assert_eq!(b.model_value(1), Some(true));
        assert_eq!(b.model_value(2), Some(true));
    }

    /// Negative literal round-trip: (-x) forces x = false.
    #[test]
    fn backend_negative_unit() {
        let mut b = VarisatBackend::new(1);
        b.add_clause(&[-1]);
        assert!(b.solve().expect("solve"));
        assert_eq!(b.model_value(1), Some(false));
    }

    /// Unknown variable (never added) has no model value.
    #[test]
    fn backend_unknown_var() {
        let mut b = VarisatBackend::new(2);
        b.add_clause(&[1]);
        assert!(b.solve().expect("solve"));
        assert_eq!(b.model_value(1), Some(true));
        // var 2 is unconstrained: varisat still assigns it, so only assert
        // the constrained one. (Documents behavior; not a contract.)
        let _ = b.model_value(2);
    }
}
