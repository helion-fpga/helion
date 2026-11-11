# AIG Rewriting Phase 1 Results

Date: 2026-10-14 23:45 EDT

## Status: LIVE AND VALIDATED

Phase 1 (trivial cut rewriting) is integrated into `Aig::from_expr`
and runs automatically on every AIG construction.

## Implementation
- `Aig::enumerate_cuts(k)`: k-cut enumeration with priority cuts
- `Aig::cut_truth_table()`: truth table computation for cuts
- `Aig::fanout_map()`: fanout tracking for rewriting
- `Aig::replace_node()`: node replacement with inversion handling
- `Aig::rewrite_trivial_cuts()`: eliminates const/buffer/inverter nodes
- Integrated into `Aig::from_expr()` (runs on every AIG)

## Validation
- 226/226 helion-sv tests pass
- 13/13 AIG-specific tests pass
- `rewrite_random_miter`: verifies equivalence after rewriting (SAT-style miter)
- No regressions in any test suite

## SERV QoR Impact
- Before: 323 LUTs
- After: 323 LUTs
- Delta: 0 (no regression, no improvement)

## Analysis
Phase 1 handles constant, buffer, and inverter nodes. These cases are
already covered by the existing peephole optimizer, so no LUT reduction
is expected. The value of Phase 1 is:
1. Infrastructure validated (cut enumeration, fanout, replacement)
2. Correctness proven (miter test)
3. Foundation for Phase 2+ (NPN rewriting, refactoring, resubstitution)

## Next: Phase 2
- NPN classification of 4-input cuts
- Precomputed optimal rewrites for NPN classes
- Cut-based refactoring (larger cuts, area flow)
- Expected: 10-20 LUT reduction on SERV
