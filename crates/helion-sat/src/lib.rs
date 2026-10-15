//! SAT solver for Helion equivalence checking.
//!
//! Provides a simple DPLL-based SAT solver for combinational equivalence
//! checking. For production use, this will be replaced with kissat via FFI.
//!
//! API:
//! - `Cnf`: Conjunctive Normal Form formula
//! - `Solver`: DPLL solver with unit propagation
//! - `solve(&cnf) -> SatResult`

/// A literal: variable index (1-based) with polarity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Lit {
    /// Variable index (1-based).
    pub var: u32,
    /// True if negated.
    pub neg: bool,
}

impl Lit {
    pub fn new(var: u32, neg: bool) -> Self {
        assert!(var >= 1, "variable index must be 1-based");
        Self { var, neg }
    }

    pub fn pos(var: u32) -> Self {
        Self::new(var, false)
    }

    pub fn neg(var: u32) -> Self {
        Self::new(var, true)
    }
}

/// A clause: disjunction of literals.
pub type Clause = Vec<Lit>;

/// CNF formula: conjunction of clauses.
#[derive(Clone, Debug, Default)]
pub struct Cnf {
    /// Number of variables (max var index).
    pub n_vars: u32,
    /// Clauses.
    pub clauses: Vec<Clause>,
}

impl Cnf {
    pub fn new(n_vars: u32) -> Self {
        Self {
            n_vars,
            clauses: Vec::new(),
        }
    }

    pub fn add_clause(&mut self, clause: Clause) {
        for lit in &clause {
            assert!(lit.var >= 1 && lit.var <= self.n_vars);
        }
        self.clauses.push(clause);
    }
}

/// Result of SAT solving.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SatResult {
    /// Satisfiable, with model (var index -> value).
    Sat(Vec<bool>),
    /// Unsatisfiable.
    Unsat,
}

/// Simple DPLL SAT solver with unit propagation.
///
/// Not optimized for large instances; for production equivalence checking
/// on full designs, use kissat via FFI (future work).
pub struct Solver {
    // Assignment: None = unassigned, Some(true/false) = assigned.
    assign: Vec<Option<bool>>,
}

impl Solver {
    pub fn new(n_vars: u32) -> Self {
        Self {
            assign: vec![None; (n_vars + 1) as usize],
        }
    }

    /// Solve the CNF formula. Returns Sat(model) or Unsat.
    pub fn solve(&mut self, cnf: &Cnf) -> SatResult {
        // Reset assignment
        for a in self.assign.iter_mut() {
            *a = None;
        }
        if self.dpll(cnf, 0) {
            // Build model
            let mut model = vec![false; (cnf.n_vars + 1) as usize];
            for (i, a) in self.assign.iter().enumerate() {
                if let Some(v) = a {
                    if i < model.len() {
                        model[i] = *v;
                    }
                }
            }
            SatResult::Sat(model)
        } else {
            SatResult::Unsat
        }
    }

    fn dpll(&mut self, cnf: &Cnf, _depth: usize) -> bool {
        // Unit propagation
        loop {
            let mut unit: Option<Lit> = None;
            for clause in &cnf.clauses {
                match self.clause_status(clause) {
                    ClauseStatus::Satisfied => continue,
                    ClauseStatus::Unsatisfied => return false,
                    ClauseStatus::Unit(lit) => {
                        unit = Some(lit);
                        break;
                    }
                    ClauseStatus::Unresolved => continue,
                }
            }
            if let Some(lit) = unit {
                self.assign[lit.var as usize] = Some(!lit.neg);
            } else {
                break;
            }
        }

        // Find unassigned variable
        let var = (1..=cnf.n_vars).find(|&v| self.assign[v as usize].is_none());
        match var {
            None => true, // All assigned, no conflict -> SAT
            Some(v) => {
                // Try true
                self.assign[v as usize] = Some(true);
                if self.dpll(cnf, _depth + 1) {
                    return true;
                }
                // Try false
                self.assign[v as usize] = Some(false);
                if self.dpll(cnf, _depth + 1) {
                    return true;
                }
                // Backtrack
                self.assign[v as usize] = None;
                false
            }
        }
    }

    fn clause_status(&self, clause: &Clause) -> ClauseStatus {
        let mut unassigned = None;
        let mut unassigned_count = 0;
        for lit in clause {
            match self.assign[lit.var as usize] {
                Some(val) => {
                    let lit_true = if lit.neg { !val } else { val };
                    if lit_true {
                        return ClauseStatus::Satisfied;
                    }
                }
                None => {
                    unassigned_count += 1;
                    unassigned = Some(*lit);
                }
            }
        }
        if unassigned_count == 0 {
            ClauseStatus::Unsatisfied
        } else if unassigned_count == 1 {
            ClauseStatus::Unit(unassigned.unwrap())
        } else {
            ClauseStatus::Unresolved
        }
    }
}

enum ClauseStatus {
    Satisfied,
    Unsatisfied,
    Unit(Lit),
    Unresolved,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sat_simple() {
        // (x1 OR x2) AND (NOT x1 OR x2) -> SAT (x1=false, x2=true)
        let mut cnf = Cnf::new(2);
        cnf.add_clause(vec![Lit::pos(1), Lit::pos(2)]);
        cnf.add_clause(vec![Lit::neg(1), Lit::pos(2)]);
        let mut solver = Solver::new(2);
        match solver.solve(&cnf) {
            SatResult::Sat(model) => {
                assert_eq!(model[2], true);
            }
            SatResult::Unsat => panic!("expected SAT"),
        }
    }

    #[test]
    fn unsat_simple() {
        // (x1) AND (NOT x1) -> UNSAT
        let mut cnf = Cnf::new(1);
        cnf.add_clause(vec![Lit::pos(1)]);
        cnf.add_clause(vec![Lit::neg(1)]);
        let mut solver = Solver::new(1);
        assert_eq!(solver.solve(&cnf), SatResult::Unsat);
    }
}
