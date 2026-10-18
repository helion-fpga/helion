# AIG Rewriting Design (ABC-style)

## Goal
Close SERV QoR gap: 323 LUTs -> 217 LUTs (Yosys). Need 106 LUT reduction.

## Current State
- AIG exists in `crates/helion-sv/src/lib.rs` (`struct Aig`)
- `flowmap_lut6()` does naive LUT mapping without optimization
- No rewriting, refactoring, balancing, or FRAIG
- Peephole optimizer only does dead sweep, const fold, inverter absorb

## SERV Breakdown (from gap report)
- 55 LUTs: INIT=0xd8d8d8d8d8d8d8d8 (3-input muxes, likely from shared subexpressions)
- 38 LUTs: INIT=0xaaaaaaaaaaaaaaaa (wire/buffers, should be eliminated)
- 18 LUTs: INIT=0 (const-zero, survive because they drive FFs)

## ABC-style Rewriting Components

### 1. Cut Enumeration
For each AIG node, enumerate all k-cuts (k=6 for LUT6):
- A cut is a set of ≤k leaves that separates the node from PIs
- Use priority cuts: keep best N cuts per node by area-flow cost
- Complexity: O(n * C) where C is cuts per node

### 2. Cut Rewriting
For each cut, compute the Boolean function (truth table):
- Check against precomputed library of optimal implementations
- If better implementation exists (fewer LUTs / less depth), replace
- NPN classification to match functions up to input negation/permutation

### 3. Refactoring
Extract common subexpressions:
- For each node, find large cuts and try to express using existing nodes
- If a subfunction already exists elsewhere, reuse it (like LutShare but at AIG level)
- This addresses the 55 mux LUTs (shared subexpressions)

### 4. Balancing
Restructure AIG to minimize depth:
- For each node, try to rebalance the AND tree
- Reduces logic depth, helps timing

### 5. FRAIG (Functionally Reduced AIG)
- Use SAT to prove functional equivalence of nodes
- Merge equivalent nodes (stronger than structural hashing)
- Requires SAT solver integration

## Implementation Plan

### Phase 1: Cut Enumeration + Simple Rewriting (est. 2-3 days)
- Implement k-cut enumeration with priority cuts
- Truth table computation for cuts
- Simple rewriting: if cut function is buffer/inverter/const, eliminate
- Target: 38 wire LUTs + 18 const LUTs = ~30-40 LUTs saved

### Phase 2: Refactoring (est. 3-4 days)
- Implement subexpression extraction
- Hash-based matching of cut functions
- Target: 55 mux LUTs (shared subexpressions) = ~20-30 LUTs saved

### Phase 3: Balancing (est. 2 days)
- AND-tree rebalancing
- Helps timing, minor area impact

### Phase 4: FRAIG (est. 1-2 weeks)
- SAT solver integration (need to choose: kissat, cadical, or custom)
- Equivalence checking
- Node merging

## Safety Requirements
- All rewrites must preserve functional equivalence
- Need equivalence checking (SAT-based) to verify
- Test on SERV, PicoRV32, Ibex before/after
- Gold WNS_PS=9640 must not regress

## Alternative: Use Existing ABC
Instead of reimplementing, could:
1. Shell out to ABC binary (if available)
2. Use ABC's rewriting via FFI (complex)
3. Port specific ABC algorithms (what we're doing)

Recommendation: Implement Phase 1+2 ourselves (high impact, manageable scope).
Defer FRAIG until SAT solver is integrated for equivalence checking anyway.

## References
- ABC: https://github.com/berkeley-abc/abc
- "DAG-aware AIG rewriting" (Mishchenko et al.)
- "Priority cuts" for LUT mapping
