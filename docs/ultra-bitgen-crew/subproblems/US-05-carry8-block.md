# US-05: CARRY8 as its own harness block

- Track: UltraScale+, bitgen plus harness.
- Branch: fm/ultra-bitstream. Worktree:
  ~/workspace/helion-ultra-bitstream.
- Status: RULED, not implemented. CTO ruling Nov 8, 2026.

## Goal

CARRY8 bits must be tracked as their own block in the harness, never
folded into LUT_INIT or any other block, so a CARRY8 error can never hide
inside another block's green.

## Done when

CARRY8 exists as its own block in the INTERFACES.md registry and in
harness fasm.py classify, and the diff report shows a CARRY8 line (green
or red) for designs that use CARRY8. A design that uses CARRY8 and shows
no CARRY8 line is a failure of this sub-problem.

## What is already done

- The ruling exists (CTO, Nov 8, 2026): CARRY8 gets its own harness
  block, never folded into LUT_INIT. The registry implementation is still
  pending; the ruling was recorded for the next worker that touches the
  registry.
- The block registry lives in INTERFACES.md: LUT_INIT, FF_INIT,
  BRAM_CONTENT, BRAM_PARITY, INT_PIP, IO_CONFIG, CLK, DSP48E2, URAM288,
  FRAMING, plus LUTRAM. CARRY8 is not in it yet.
- The harness classifier is crates/helion-ultrascale/harness/fasm.py
  (classify). The diff report format is JSONL per block: {block,
  diff_bits, total_bits, status green/red/blocked}, ordered by effective
  size, with placement hash and seed recorded per run.
- The 7-series side has the same problem in a different form (ZB-04:
  CARRY4 inference does not exist yet), so there is no CARRY8 golden
  design to test against today. The block must still exist and report
  correctly on designs without CARRY8 (zero bits, green) and on designs
  with it.

## What was tried and what failed

- Nothing tried. The ruling was recorded and the implementation was
  deferred to the next registry-touching worker. That worker is this
  sub-problem.

## What to do

1. Add CARRY8 to the block registry in INTERFACES.md with its definition
   (which features belong to it, which tile types).
2. Add CARRY8 to harness fasm.py classify so CARRY8 features route to the
   CARRY8 block and nowhere else.
3. Add a harness test: a design with CARRY8 features produces a CARRY8
   line in the diff report; a design without them produces a zero-bit
   green CARRY8 line.
4. Run the full harness suite and the full diff on blinky and counter.
   Both must be byte-identical to runs/srval-complete (the current
   baseline) except for the added CARRY8 line. Any other change is a
   regression; revert it.
5. Record the change in COORD.md and get a critic re-run. A block goes
   green only on zero diff PLUS a critic re-run.

## Files and commands

- Contracts: docs/ultra-bitgen-crew/INTERFACES.md (block registry).
- Harness: crates/helion-ultrascale/harness/fasm.py (classify),
  diff.py, tests/.
- Baseline: ~/workspace/ultra-bitgen-crew/harness/runs/srval-complete/diff.jsonl.

## Dependencies

Depends on nothing. Blocks honest CARRY8 reporting in US-07 (the random
generator). Related to ZB-04 (CARRY4 inference).

## Rules

- No em dashes anywhere.
- CARRY8 bits must never be folded into LUT_INIT or any other block.
- A fix that regresses a green block is reverted on the spot.
