# US-02: BRAM golden design and INIT translation

- Track: UltraScale+, bitgen.
- Branch: fm/ultra-bitstream. Worktree:
  ~/workspace/helion-ultra-bitstream.
- Status: not started. BRAM has never been exercised by any golden.

## Goal

Prove the BRAM encoding layer: a design that uses block RAM must diff
to zero on BRAM_CONTENT and BRAM_PARITY.

## Done when

BRAM_CONTENT and BRAM_PARITY read green on a BRAM golden design, with no
other block regressed.

## What is already done

- Nothing for BRAM. The v1 goldens (blinky and counter) contain no BRAM,
  so BRAM_CONTENT and BRAM_PARITY have never been exercised. They are
  blocked, not red: the harness has nothing to compare.
- The interface contracts are set (INTERFACES.md): a BRAM tile is one
  RAMB36E2 OR two RAMB18E2 halves, never both (type-level BramTileUse);
  RAMB18 halves are named _L / _U, never _A / _B.
- The ZedBoard track has the same BRAM problem at a different layer
  (ZB-05: BRAM port config and loud INIT failure). Share the translation
  approach; do not invent two different ones.

## What was tried and what failed

- Nothing tried. No BRAM golden exists, no translation code exists.

## What to do

1. Build a BRAM golden design: a small design that uses block RAM with
   known contents, in the same style as the blinky and counter goldens
   (see crates/helion-ultrascale/harness/gen_golden.py and the golden/
   directory). Record its placement hash and seed like the others.
2. Write the whole-array to per-word INIT translation: the design's
   INIT[16383:0] / INITP[2047:0] whole-array values must be translated to
   the DB's per-word INIT_00..INIT_FF features. This translation is
   integration work; it does not exist yet.
3. Run the harness on the BRAM golden. Diff BRAM_CONTENT and BRAM_PARITY
   to zero. Any differing bit is a real bug in the translation or the
   encoding; fix it in one block per commit, full diff re-run after each
   fix, per the greedy loop in SPEC.md.
4. If the DB has no BRAM encoding for a feature the golden needs, record
   it as a named blocker with the exact missing data (which feature,
   which tile type), and fail loud on it. Do not invent bits.

## Files and commands

- Harness: crates/helion-ultrascale/harness/ (gen_golden.py, fasm.py,
  reference.py, diff.py, golden/).
- Contracts: docs/ultra-bitgen-crew/INTERFACES.md (BramTileUse,
  SegbitsDb, BitRef).
- Code: crates/helion-ultrascale/src/ (bitgen.rs, segbits.rs, bitloc.rs).
- Related: ZB-05 (ZedBoard BRAM).

## Dependencies

Depends on nothing for the golden and the translation. The hardware proof
of BRAM contents ultimately needs US-08 (tier 2). Related to ZB-05.

## Rules

- No em dashes anywhere.
- One block per fix, one fix per commit, full diff re-run after each fix.
- A block goes green only on zero diff PLUS a critic re-run.
- Never invent bits. Fail loud on missing encodings.
