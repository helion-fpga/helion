# ZB-05: BRAM port configuration and loud INIT failure

- Track: ZedBoard, backend. Investigator findings C7 and C8.
- Branch: fm/zedboard-fix-bufg (no BRAM work committed anywhere yet).
  Worktree: ~/workspace/helion-zed-bringup.
- Status: not started.

## Goal

Block RAM must either work correctly on the board, or the backend must
refuse to emit it with a clear error. Silent wrong behaviour is not
acceptable.

## Done when

Either (a) a BRAM design programs and reads back correct data on the
board, or (b) a design using BRAM fails at build time with a clear error
naming the unsupported feature. A bitstream that programs (done=1) but
silently produces wrong data is a failure of this sub-problem.

## What is already done

- Nothing. Two findings document the current state:
- C8 (emit.rs:180-187): the backend emits only
  `TILE.RAMB18.SITE.IN_USE`. No port configuration (clocks, enables,
  address/data widths), no INIT data programming. The comment admits
  "future work".
- C7 (pack.rs:231-236): BRAM INIT hex strings are parsed with
  `u64::from_str_radix(t.trim(), 16).unwrap_or(0)`. On parse failure the
  value silently becomes 0. A malformed INIT attribute produces a zeroed
  BRAM with no error.

## What was tried and what failed

- Nothing tried. Both findings are untouched since the Oct 22, 2026
  investigation.

## What to do

1. First, make the failure loud. Change the INIT parse in pack.rs to
   return an error on parse failure instead of `unwrap_or(0)`. Add a
   unit test with a malformed INIT string that asserts the error. This
   is the smallest, highest-value change in this sub-problem and it
   should land first.
2. Then, decide the scope with the user before building the rest: full
   BRAM port configuration is a large job (RAMB36E2 as one block or two
   RAMB18E2 halves, port clocks, enables, widths, INIT data). Do not
   build it speculatively.
3. If full BRAM is approved, the design rule is: one RAMB36E2 OR two
   RAMB18E2 halves, never both (the same type-level rule the UltraScale+
   crew adopted for its BramTileUse type). RAMB18 halves are named _L /
   _U, never _A / _B.
4. The UltraScale+ track has the same BRAM problem at a different layer
   (US-02: whole-array INIT to per-word INIT translation). Share the
   translation approach; do not invent two different ones.
5. Verify with a BRAM design on the board (write a pattern, read it back)
   or, if the scope decision is fail-loud only, with the unit tests plus
   a build-time error on a BRAM design.

## Files and commands

- Code: crates/helion-xilinx7/src/pack.rs (lines 231-236, INIT parse),
  src/emit.rs (lines 180-187, BRAM emit).
- DB: ~/workspace/xilinx-dbsrc, BRAM segbits for the zynq7 family.
- Related: US-02 (UltraScale+ BRAM golden design and INIT translation).

## Dependencies

Depends on nothing. Blocks any corpus design that uses BRAM. Related to
US-02.

## Rules

- No em dashes anywhere.
- Fail loudly. Silent miscompiles are unacceptable; the investigator's
  recommendation 1 says exactly this.
- Do not build full BRAM port configuration without the user's scope
  decision.
