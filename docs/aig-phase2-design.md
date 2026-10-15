# AIG Rewriting Phase 2 Design: NPN Classification

## Goal
Classify 4-input Boolean functions into NPN equivalence classes to enable
cut-based rewriting with precomputed optimal implementations.

## NPN Equivalence
Two functions f and g are NPN-equivalent if g can be obtained from f by:
- **N**: Negating any subset of inputs
- **P**: Permuting inputs
- **N**: Negating the output

There are 222 NPN classes for 4-input functions (vs 65,536 total functions).

## Algorithm

### Canonical Form
For a 4-input truth table (16 bits):
1. Generate all 2^4 * 4! * 2 = 768 NPN transformations
   - 16 input negation patterns
   - 24 input permutations  
   - 2 output negations
2. For each transformation, compute the transformed truth table
3. The canonical form is the lexicographically smallest truth table

### Implementation Steps
1. **Truth table representation**: u16 (16 bits for 4 inputs)
2. **Input negation**: For pattern p (4 bits), flip bits where input i is 1
   - For each minterm m (0-15), if ((m >> i) & 1) != ((p >> i) & 1), flip bit
3. **Permutation**: Reorder inputs according to perm[4]
   - New minterm m' has bit i = old bit perm[i]
4. **Output negation**: Flip all 16 bits

### Precomputation
For each of the 222 NPN classes:
- Store canonical representative
- Store optimal AIG implementation (fewest AND nodes)
- At rewrite time: classify cut function, look up optimal, replace if better

## Integration
1. During cut enumeration, for each 4-input cut:
   - Compute truth table
   - Classify into NPN class
   - Look up precomputed optimal implementation
   - If optimal uses fewer ANDs than current, replace

## Expected Impact
- SERV: 10-20 LUT reduction (from 323 toward 300)
- Based on ABC's `rewrite` command results on similar designs

## References
- ABC: `rewrite` command (Mishchenko et al.)
- "NPN Classification" - 222 classes for 4 inputs
