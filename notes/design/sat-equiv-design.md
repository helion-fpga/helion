# SAT-Based Equivalence Checking Design

## Goal
Verify that optimizations (AIG rewriting, LUT sharing, etc.) preserve
functional equivalence. Required before aggressive optimizations can be
enabled by default.

## Approach

### Miter Construction
Given two designs (original and optimized):
1. Create miter: feed same inputs to both, XOR the outputs
2. If miter is UNSAT (XOR never 1), designs are equivalent
3. If SAT, the satisfying assignment is a counterexample

### SAT Solver Options
1. **kissat**: Modern, fast, simple API. Good choice.
2. **cadical**: Also good, slightly more complex API.
3. **MiniSat**: Older, but well-understood.
4. **Custom DPLL**: For learning, not production.

Recommendation: Start with kissat via FFI or CLI.

### Integration Points
1. **AIG rewriting**: After each rewrite, verify equivalence
   - Build miter from original AIG and rewritten AIG
   - SAT check (with timeout)
   
2. **Peephole optimizer**: After each round, verify
   - Currently trusts the transformations
   - SAT check would catch bugs like the round-1 inverter bug

3. **LUT mapping**: Verify mapped netlist vs original RTL
   - More expensive (larger miter)
   - Run on release builds, not every commit

### Implementation Plan

#### Phase 1: AIG to CNF (1-2 days)
- Tseitin transformation: AIG -> CNF
- Each AND gate becomes 3 clauses
- Handle inverted edges

#### Phase 2: SAT Solver Integration (2-3 days)
- Choose solver (kissat recommended)
- FFI bindings or CLI wrapper
- Timeout handling

#### Phase 3: Miter + Check (1-2 days)
- Build miter from two AIGs
- Run SAT solver
- Parse result (SAT/UNSAT/timeout)

#### Phase 4: Integration (2-3 days)
- Hook into AIG rewriting
- Hook into peephole optimizer
- Add `helion equiv` CLI command

### API Sketch
```rust
pub fn check_equiv(aig1: &Aig, aig2: &Aig) -> EquivResult {
    let miter = Miter::new(aig1, aig2);
    let cnf = miter.to_cnf();
    let solver = SatSolver::new();
    match solver.solve(&cnf, timeout) {
        SatResult::Unsat => EquivResult::Equivalent,
        SatResult::Sat(assignment) => EquivResult::NotEquivalent(assignment),
        SatResult::Timeout => EquivResult::Unknown,
    }
}
```

### Testing
- Positive: Verify known-equivalent transforms
- Negative: Verify detected inequivalence (mutate a LUT)
- Performance: SERV-sized designs should check in <10s

## References
- ABC `cec` command (combinational equivalence checking)
- "SAT-based equivalence checking" literature
- kissat: http://fmv.jku.at/kissat/
