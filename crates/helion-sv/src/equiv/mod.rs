//! SAT-based combinational equivalence checking (v3.0 verification track).
//!
//! Two [`EquivModel`]s (golden and revised) are encoded to CNF with a shared
//! PI variable per input name, differenced output-by-output (miter), and
//! handed to a [`solver::SolverBackend`] (varisat). UNSAT = equivalent; SAT =
//! a concrete [`Counterexample`].
//!
//! Scope is combinational only: sequential elements are cut at the model
//! boundary (`Hff` Q = pseudo-PI, D = pseudo-output). Black boxes and other
//! unsupported cells fail loud via [`EquivError::UnsupportedCell`].

pub mod cnf;
pub mod lut_bridge;
pub mod miter;
pub mod model;
pub mod prove;
pub mod solver;

pub use miter::{Miter, OutPair};
pub use model::EquivModel;
pub use prove::prove_equivalent;
pub use solver::{SolverBackend, VarisatBackend};

use std::fmt;

/// Errors from equivalence checking. Every variant fails loud: there is no
/// silent pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EquivError {
    /// Golden and revised PI name sets differ.
    PiMismatch,
    /// Golden and revised output name sets differ.
    OutputMismatch,
    /// A cell, net, or structure the checker cannot model (black boxes,
    /// undriven nets, combinational loops, ...). The payload names it.
    UnsupportedCell(String),
    /// The SAT solver itself failed, or a SAT model was inconsistent.
    Solver(String),
}

impl fmt::Display for EquivError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EquivError::PiMismatch => {
                write!(f, "PI sets differ between golden and revised models")
            }
            EquivError::OutputMismatch => {
                write!(f, "output names differ between golden and revised models")
            }
            EquivError::UnsupportedCell(s) => {
                write!(f, "unsupported cell or net for equivalence checking: {s}")
            }
            EquivError::Solver(s) => write!(f, "SAT solver error: {s}"),
        }
    }
}

impl std::error::Error for EquivError {}

/// Verdict of [`prove_equivalent`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EquivVerdict {
    /// UNSAT: no input assignment distinguishes the two models.
    Equivalent,
    /// SAT: the inputs below distinguish the models at `differing_output`.
    Counterexample(Counterexample),
}

/// Concrete input assignment distinguishing golden from revised.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Counterexample {
    /// Full PI assignment (deterministic order) that distinguishes the models.
    pub inputs: Vec<(String, bool)>,
    /// Name of the first output on which the models disagree.
    pub differing_output: String,
    /// Golden value of that output under `inputs`.
    pub golden: bool,
    /// Revised value of that output under `inputs`.
    pub revised: bool,
}

impl fmt::Display for Counterexample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "counterexample: output '{}' differs", self.differing_output)?;
        writeln!(f, "  golden = {}, revised = {}", self.golden, self.revised)?;
        writeln!(f, "  inputs:")?;
        for (name, val) in &self.inputs {
            writeln!(f, "    {name} = {val}")?;
        }
        Ok(())
    }
}
