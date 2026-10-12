//! SAT-based combinational equivalence checking (v3.0 verification track).
//!
//! Implementation lands in this directory:
//! `model` (AIG cone model), `cnf` (Tseitin encoder), `lut_bridge`
//! (Lut6 -> AIG), `miter` (miter CNF construction), `solver`
//! (solver backend), `prove` (orchestration + counterexample extraction).
