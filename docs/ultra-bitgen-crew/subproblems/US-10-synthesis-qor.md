# US-10: Synthesis quality (QoR vs Vivado)

- Track: QoR campaign, sprint 3. Owned by the QoR crew, not the bitgen
  crew, but it is on the goal's critical path: a perfect bitgen that
  faithfully encodes a bad netlist still produces a bad design.
- Branch: fm/sprint2-combined (1779474) is the base; worker branches
  fm/qor2-* in ~/workspace/helion-ice40. Crew dir: ~/workspace/qor2-crew/.
- Status: OPEN. Honest scoreboard: 23 RED, 13 green vs Yosys on 36
  comparable designs. Target is Vivado, not Yosys.

## Goal

A netlist as good as Vivado's, measured exactly against Vivado on the
same device and design (LUT count, timing, utilisation). Definition of
done, locked Oct 22, 2026: beat Yosys on ALL designs, ALL families, ALL
levels, per design. Median and mean are progress reports only.

## Done when

The QoR scoreboard is green against Vivado on every design, every family,
every level, per design. Until then, every number reported is a progress
report, not a completion.

## What is already done

- Sprint 1 (fm/qor-integrate, 837c629): 4 bugs fixed (absorb_lut_inverters
  fanout INIT flip; opt_merge_equivalent_ffs merging distinct FSM FFs;
  balance_associative_trees INIT remap aliasing; about 2,900 lines dead
  P and R code plus 2 lying stubs found).
- Sprint 2 (fm/sprint2-combined, 1779474, closed Oct 20, 2026): uart/spi
  MAC detector misfire fixed; axi_fifo silent miscompile fixed
  (merge_sequential_nbas override semantics, load-bearing); serv
  opt_ff_enable. Final honest numbers (n=31): master median +211.0
  percent; sprint1 median +39.1 percent; sprint2 median +39.3 percent,
  7 / 1 / 23.
- Sprint 3 work phase complete Oct 22, 2026, about 10:41 PM EDT. All 14
  agents done. Contracts: no design past 2x sprint-2 synth time (CPU
  time); simpleuart tripwire: below 300 LUTs without a Carry4 comparator
  plus miter = presumed miscompile FAIL.
- Comparator done (afb3fbe): hoist_procedural_comparators; correctness
  critic PASS (196,608 combos proven). simpleuart 2232 LUTs (debug raw
  LUT6; do NOT publish "3258 to 2232", units differ) vs Yosys 263, still
  RED. Bisection 3 complete: all 14 newly failed tests fail identically
  with the patch reverted; the comparator introduces zero new failures.
- AXIFIFO (fm/qor2-axififo, a18a791): split-brain BRAM bug fixed; numbers
  405 to 193, rd 203 to 90, wr to 83 (synth stage packed LUT4), still RED
  vs Yosys 63 / 33 / 34. Correctness critic verdict: FAIL with two NEW
  silent wrong-data defects in outside agent commit 14a52c6: (1) BRAM_DOUT
  pre-pass registers memory on total_bits >= 256 or force or has_init but
  inference skips on init_overflow, reads go to undriven nets; (2)
  find_mem_read_addr takes the first read address, no multiple distinct
  read check. BRAM changes must not integrate until repaired or reverted.
- SERV (replacement worker done): emit_mux no longer flattens nested
  ITEs; projection-based enable inference. 7 of 11 improved, ALL 11 still
  RED. immdec 149 to 46 (Yosys 38), ctrl 43 to 15, bufreg2 172 to 101.
- CEDFF done (work UNCOMMITTED): SB_DFFE lowering through full iCE40 pack
  / place / route; correction: no iCE40 clock-enable config bit exists,
  enable is purely the routed lutff_global / cen net. yosys_equiv plus sat
  -seq 6 SUCCESS, 33/33 tests green. ECP5 TRELLIS_FF CE NOT covered.
- Runtime done: full corpus CPU baseline 100/100, CPU total 3240.3 s,
  escalation line 4860.5 s. RUNTIME_LEDGER.md regenerated. Timing noise
  root cause is CPU contention, not the binary; the harness records CPU
  time (getrusage) in every CSV; the 2x runtime budget is gated on CPU
  time.
- picorv32 diagnosis (definitive): the frontend is optimal; the K=4
  mapper is broken (65 LUT6 to 161 LUT4, 2.5x). CTO put BRAM inference in
  scope per the locked definition of done (floors about +55 percent
  without it, 2500 vs Yosys 1613).
- Methodology (fixed): ice40-hx8k, synth stage only. Helion LUT6 to packed
  LUT4 via ~/workspace/bench/packed_lut4.py (0 to 1, 1 to 4 to 1, 5 to 3,
  6 to 7). Delta = (Helion - Yosys) / Yosys, negative good. Yosys 0.70 in
  oss-cad-suite. Harness: ~/workspace/qor-campaign/coord/harness/.

## What was tried and what failed

- Mapper is the blocker: the K=4 mapper is the sole blocker for 4 serv
  designs that beat Yosys pre-map (serv_csr 15 to 30, serv_ctrl 7 to 15,
  serv_immdec 37 to 46, serv_state 36 to 42 post-map). Correctness critic
  flag: fm/qor2-mapper commit 47eed1e contains a proven miscompile
  (6-input AND to const 0); the mapper worker verified a fix that sits
  UNCOMMITTED in the worktree. Branch repair (reset to 1779474, re-apply
  the verified fix, add 4 regression tests) is the highest-leverage
  pending CTO call.
- Residual gap (unassigned): simpleuart's remaining about 1960 LUT6s are
  ripple adder logic from procedural incrementers with no Carry4 adder
  inference. Needs an owner. This is the same problem as ZB-04 at a
  different scale.
- 25 helion-sv suite failures are pre-existing on base 1779474, awaiting
  CTO / critic disposition.
- 5 ownership incidents (open, needs CTO): commits appear on worker
  branches NOT authored by the assigned worker (all show author
  saksham-45), containing buggy code. (1) fm/qor2-mapper 47eed1e
  miscompile; (2) second unrostered harness worker; (3) fm/qor2-axififo
  14a52c6 with Defect 1; (4) fm/qor2-serv acfee41 with two proven bugs
  plus an ACTIVE REVERTER that overwrote the live worker's uncommitted
  edits at about 17:34 UTC Oct 22, 2026; (5) duplicate comparator
  sessions (PIDs 28749, 37660) both live on the same worktree, canonical
  session undeclared.
- CTO decision queue (Oct 22, 2026): (1) mapper branch repair, (2) BRAM
  scoping, (3) outside agent investigation, (4) axififo defect fix
  session, (5) integration sequencing, (6) release build capacity,
  (7) adder inference owner, (8) disposition of 25 pre-existing failures,
  (9) manifest gap follow-up. No release build was possible all day Oct
  22, 2026 (machine load), so no verified release numbers moved.

## What to do

1. First: the mapper branch repair. Reset fm/qor2-mapper to 1779474,
   re-apply the verified fix, add the 4 regression tests, and get a
   correctness critic PASS. This is the highest-leverage item.
2. Second: assign the adder inference owner and build Carry4 adder
   inference (coordinate with ZB-04; it is the same problem).
3. Third: repair or revert the axififo defects (BRAM_DOUT pre-pass and
   find_mem_read_addr), then integrate.
4. Fourth: disposition the 25 pre-existing helion-sv failures with the
   CTO / critic.
5. Throughout: never add a fix primarily to make failing tests pass
   without verifying the synthesis semantics (Oct 16, 2026 boundary).
   Suppressing cells to satisfy tests can hide a real bug.

## Files and commands

- Crew dir: ~/workspace/qor2-crew/ (SPEC.md with the locked definition
  of done, COORD.md, CRITIQUE.md).
- Harness: ~/workspace/qor-campaign/coord/harness/ (helion_qor.py,
  compare.py, gate.py, qor_lib.py, sprint3.py, --parity mode).
- Base: fm/sprint2-combined at 1779474. Master: ~/workspace/helion-ice40
  at 566246a.

## Dependencies

Depends on CTO decisions (mapper repair first). Related to ZB-04 (Carry4
inference) and US-09 (the synth side of the end-to-end flow).

## Rules

- No em dashes anywhere.
- Correctness before QoR claims. Industry-scale designs only.
- Never add a fix primarily to make failing tests pass without verifying
  the synthesis semantics.
- Every Helion claim ships with its command and output. Nothing is
  reported done or tested without a shown run. No "likely" or "probably"
  in status reports.
