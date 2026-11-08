# ZB-06: DSP fail loud or implement

- Track: ZedBoard, backend. Investigator finding C4.
- Branch: none (no DSP work committed anywhere). Worktree:
  ~/workspace/helion-zed-bringup.
- Status: not started.

## Goal

A design using DSPs must either work, or the backend must refuse it with
a clear error. Today it silently programs a bitstream in which the DSP
does nothing.

## Done when

Either (a) a DSP design (Mac27 cells) works on the board, or (b) a design
using DSPs fails at build time with a clear error naming DSP as
unsupported. A bitstream that programs (done=1) with a silently dead DSP
is a failure of this sub-problem.

## What is already done

- Nothing except the finding. Investigator C4: emit.rs line 196 has
  `let _ = &placed.dsps;`, which explicitly discards DSP placements. If
  a design uses DSPs, they are placed on sites but no configuration is
  emitted. The comment says "future work" but there is no loud error.

## What was tried and what failed

- Nothing tried. The finding is untouched since Oct 22, 2026.

## What to do

1. First, make the failure loud. If `placed.dsps` is non-empty, return an
   error naming DSP as unsupported instead of discarding the placements.
   Add a unit test with a DSP design that asserts the error. This is the
   smallest, highest-value change and it should land first.
2. Then, decide the scope with the user before building the rest: full
   DSP48E1 configuration is a large job. Do not build it speculatively.
   The UltraScale+ track has the same problem (US-03: DSP48E2 encodings
   need Vivado fuzzing). If full DSP is approved, coordinate the approach
   with US-03.
3. If full DSP is approved, the encodings must come from the X-Ray DB and
   be validated against a Vivado bitstream for the same DSP design (see
   ZB-10). Guessed encodings are never emitted.

## Files and commands

- Code: crates/helion-xilinx7/src/emit.rs (line 196, `let _ = &placed.dsps;`).
- DB: ~/workspace/xilinx-dbsrc, DSP segbits for the zynq7 family.
- Related: US-03 (UltraScale+ DSP48E2 encodings).

## Dependencies

Depends on nothing. Blocks any corpus design that uses DSPs. Related to
US-03.

## Rules

- No em dashes anywhere.
- Fail loudly. Silent miscompiles are unacceptable.
- Do not build full DSP configuration without the user's scope decision.
- Guessed encodings are never emitted.
