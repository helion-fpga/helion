# US-07: Random-design generator and the 100-design loop

- Track: UltraScale+, bitgen plus harness. This is the endgame of the
  bitgen track: 100 random designs, green at both tiers.
- Branch: fm/ultra-bitstream. Worktree:
  ~/workspace/helion-ultra-bitstream. The hourly driver cron
  bitgen-100-driver works this queue.
- Status: not started. Only 2 designs have ever been diffed.

## Goal

Build a seeded generator that produces random valid FASM designs covering
every block, and a harness loop that diffs them, until 100 designs are
green at tier 1 (and, when the license allows, tier 2).

## Done when

100/100 designs green at tier 1 (zero diff vs the public segbits DBs).
A design counts only when green at both tiers eventually (see US-08);
tier 1 100/100 is the milestone this sub-problem owns.

## What is already done

- The harness exists and is proven on 2 designs:
  ~/workspace/ultra-bitgen-crew/harness/ (diff.py, diff_engine.py,
  emitter.py, fasm.py, gen_golden.py, reference.py, roundtrip.py, tests/,
  golden/), with a snapshot committed in crates/helion-ultrascale/harness/.
  One command, `python3 diff.py`, from the harness directory. Harness
  suite: 38 tests OK. Baseline placement hashes: blinky xcku3p
  7bf85169..., counter xcku3p bbb40255... (full values in COORD.md).
- The greedy loop is locked in SPEC.md: the diff harness orders blocks by
  diff size descending, fix the largest first, one block per fix, one fix
  per commit, re-run the full diff after each fix, a fix that regresses a
  green block is reverted on the spot, a block goes green only on zero
  diff PLUS a critic re-run, stop only at zero diff everywhere or a named
  blocker with the exact missing data, never a silent skip.
- The diff report format is set: JSONL per block {block, diff_bits,
  total_bits, status green/red/blocked}, placement hash and seed recorded
  per run, ordered by effective size.
- Current green blocks on the 2 designs: LUT_INIT, FF_INIT, INT_PIP,
  FRAMING (blinky: 0/64, 0/2, 0/11, 0/3; counter: 0/256, 0/8, 0/17, 0/3).
  Blocked: IO_CONFIG (55 bits), BRAM_CONTENT, BRAM_PARITY, DSP48E2,
  URAM288, CLK, LUTRAM.
- Performance baseline: a full harness run takes about 9 minutes for 2
  designs; one bits run on a 16.5 MB bitstream took 111 s. The 100-design
  loop needs the speed work in US-12 or it will take days.

## What was tried and what failed

- Nothing tried. No generator exists; no random design has ever been
  diffed. The 2 goldens are hand-built.

## What to do

1. Build the seeded generator. It must produce random valid FASM designs
   covering: LUT INITs, FF INIT/SRVAL, BRAM contents/parity, INT PIPs
   from real DB edges, IO configs, and CLK/DSP/URAM as their encodings
   land (US-01, US-02, US-03, US-04 gate what the generator can cover).
   Seeds recorded, reproducible: the same seed must produce the same
   design.
2. Build the 100-design harness loop on top of the existing diff.py:
   generate, emit, diff, record per-design results with placement hash
   and seed, and order the failures by diff size for the greedy loop.
3. Work the failures greedily per SPEC.md: largest diff first, one fix
   per commit, full diff re-run after each fix, revert on regression.
   Each fix is its own sub-problem if it is a new block; small fixes go
   through the same loop with a critic re-run.
4. Report milestones through the driver: every 10-design tier-1
   milestone, and tier-1 100/100. When tier 1 hits 100/100 while the
   license is down, report once "100/100 at tier 1, tier 2 awaiting the
   Vivado license" and stay quiet until the license returns.
5. Do not start the loop until the harness speed (US-12) is good enough
   that 100 designs finish in a sane window, or run it in batches and
   record the runtime honestly.

## Files and commands

- Harness: ~/workspace/ultra-bitgen-crew/harness/ (diff.py and friends),
  snapshot in crates/helion-ultrascale/harness/.
- Contracts: docs/ultra-bitgen-crew/SPEC.md (greedy loop, definition of
  done), INTERFACES.md (diff report format, FASM format).
- Driver: cron bitgen-100-driver (hourly, goal
  virtex-ultrascale-backend-support); its run log is at
  ~/workspace/goals/virtex-ultrascale-backend-support/hidden_files/bitgen-100-driver/run-log.md.
- Baseline runs: ~/workspace/ultra-bitgen-crew/harness/runs/srval-complete/diff.jsonl.

## Dependencies

Depends on US-01, US-02, US-03, US-04, US-05 for coverage (the generator
can only cover blocks that exist). Depends on US-12 for speed. Tier 2
needs US-08.

## Rules

- No em dashes anywhere.
- Greedy loop only: largest diff first, one fix per commit, full re-run
  after each fix, revert on regression.
- A block goes green only on zero diff PLUS a critic re-run.
- Never pad the count with toy or duplicate designs. 100 means 100
  distinct random designs.
