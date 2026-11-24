# N48 ROUTE OveruseTimeout congestion lane

Lane branch `fm/ultra-bitgen-n48` at `116c629c` on origin (code tip
`2a357801`: impl commit on N47 base `4720575d`; merge `116c629c`;
report commit pending). Impl branch `fm/ultra-bitgen-n48-impl` at
`2a357801` on origin. Worktree `~/workspace/helion-ultra-bitgen-n48`,
clean. Report `~/workspace/ultra-bitgen-crew/N48_REPORT.md` (this
file; copy committed at `tools/n48/N48_REPORT.md` on the lane branch,
byte-identical). Baseline report at
`~/workspace/n48-logs/baseline/BASELINE_REPORT.md` (382 lines), impl
report at `~/workspace/n48-logs/impl/IMPL_REPORT.md` (478 lines).
Logs under `~/workspace/n48-logs/` (`baseline/`, `impl/`, `coord/`).
Coordinator: N48 lane coordinator (dispatched Nov 24, 2026, about
02:12 EST, after Critic Session 44 closed N47 and the grind watcher
flagged no agent in flight). No critic was dispatched by this lane;
the main agent dispatches Session 45.

## 1. Headline: N48 is fixed on the measured design set

The ROUTE OveruseTimeout congestion class, which bounded run-level
success for every lane so far, no longer occurs on the three designs
that reproduce it. Counter12 seed 1 goes from 3 of 5 timeouts on the
base to 5 of 5 routes and 5 of 5 verify-bit passes on the fix, with
byte-identical routes every run (22 iters, 763 PIPs). Counter16 seed
0 goes from 3 of 5 timeouts to 5 of 5 routes and 5 of 5 verify-bit
passes at 40000 cycles (80000/0 each). Counter8 seed 2 routes 5 of 5
and passes verify-bit 5 of 5 with a swept port map. The coordinator
re-ran the acceptance from the merged branch binary: counter12 seed
1 twice (22 iters, 763 PIPs, .bit md5 byte-identical to the impl fix
run and the stash restore run), counter16 seed 0 twice (28 iters,
1446 PIPs), counter8 seed 2 twice (8 iters, 343 PIPs), all
verify-bit PASS. All 8 regression designs pass verify-bit on the
merged branch; blinky `.bit` md5 `d82b70c8` unchanged. Suites on the
merged branch: ultrascale 456/0/1, CLI release 89/0/1, US-09
checkers 100 OK. Stash proof holds both sides.

Two gaps the N47 lane and Session 44 left open are closed here as
side effects: counter8 seed 2 verify (its port map is settled by
sweep, led SLICE_X4Y0.AQ) and counter16 seed-0 verify-bit (its port
map is settled by sweep, led SLICE_X1Y0.AQ; 5 of 5 PASS at 40000
cycles). Session 44 ruling (d) is therefore satisfied: seed 0 is
now verify-bit proven, not flow-only.

## 2. The fix (5 files, +710/-9)

Commit `2a357801`, "ultra: fix N48 ROUTE OveruseTimeout congestion
class". Root cause, two mechanisms (IMPL_REPORT.md section 2):

1. Cross-process nondeterminism in fanout order (the main cause).
   `IntTileWireDb::from_json` built fanout lists by iterating a
   HashMap PIP table, so PIP edges landed in per-process random
   order and A* explored and tie-broke differently every run. Same
   netlist: 28-50 iters, different failing nets. This is also the
   root cause of the carried N35/N36 router nondeterminism.
2. Slow tail negotiation. Iteration 0 starts at 58-118 overused
   wires; the tail oscillates at 1-7 overused for dozens of
   iterations. Typical convergence needs 28-50 iterations, so the
   50-iteration budget sits inside the convergence distribution's
   upper tail: about 25-35 percent of runs time out. History grew
   by 1 per iteration, so it had to outgrow a net's detour length
   (tens of wires) before the net detoured. An offline graph model
   proved a detour exists for the stuck cases, so the class is NOT
   a capacity limit.

The fix:

- F1, deterministic fanout: `crates/helion-ultrascale/src/wire_db.rs`
  sorts every fanout list by wire id after construction
  (`list.sort_unstable()`). Result, 10 of 10 identical across
  processes: counter12 seed 0 routes in 34 iters, counter16 seed 0
  in 22 iters.
- C-f, history grows by the present-use count:
  `crates/helion-ultrascale/src/router.rs` uses
  `hist[i].saturating_add(c)` instead of `+1`, where `c` is the
  present-use count. Result, 10 of 10 identical: counter12 seed 0
  in 20 iters (F1: 34), counter16 seed 0 in 28 iters (F1: 22).
  Worst case across the two netlists improves 34 to 28; both pass
  with 22-30 iters headroom. `max_iters` stays 50 (no budget
  change).
- `crates/helion-cli/src/ultra.rs`, 11 lines: doc comment updated
  (router is deterministic now).
- Tests: `crates/helion-ultrascale/tests/n48_route.rs` (+176, two
  regression tests pinning the exact counter12-s0 and counter16-s0
  fixture netlists) and `crates/helion-ultrascale/tests/n48_fixtures.rs`
  (+464, the exact netlists from the flow debug logs).

Rejected by measurement, documented in the code: per-iteration net
re-draw (counter16 FAILED at 50 iters, 76 iters at a 150 budget)
and a terminal hard block (counter12 FAILED deterministically even
at 150 iters). Both fully reverted.

## 3. Baseline (W-BASELINE, 382-line report)

N48 reproduces on the N47 base: 7 of 20 fresh flows exit 1 with
ROUTE OveruseTimeout, 35 percent. Failure shape is stable in form
(always exactly 50 iters, always exactly 2 nets sharing 1 wire)
but run-dependent in content (6 distinct nets across 7 failures;
only `n0` repeats). Per-design timeout rates: counter12 seed 0, 1
of 5 (cnt_4, wire 4876, tile (3,0)); counter12 seed 1, 3 of 5
(n0, wire 1508, tile (1,0) twice, exact repeat of the known
instance; cnt_5, wire 4922, tile (3,0)); counter16 seed 0, 3 of 5
(three different nets, wires, tiles); counter8 seed 2, 0 of 5
timeouts, but all 5 flows fail verify-bit cosim identically (401
of 800 mismatches, led stuck at 1), a port-map mismatch, not a
route failure. Wires cluster tightly per tile: (1,0) gives
1508/1508/1513, (3,0) gives 4872/4876/4922, (5,0) gives 7872.
Most stable reproduction case: counter12 seed 1. Baseline suites
453/0/1, 89/0/1 (CLI), 100 OK checkers by per-module
rerun after the full discover run was OOM-killed. All 7 regression
designs pass verify-bit on base; blinky md5 unchanged. W-IMPL's own
20-flow repro matrix agrees (5 of 20 fail); combined with
W-BASELINE: 12 of 40 fail (30 percent).

## 4. Acceptance flows: 5 runs per design on the fix

Impl script `run-fix-accept.sh`, summary `fix-accept-summary.txt`.
Every run is byte-identical within a design: same iters, same PIP
count, same bitstream. Compare with the base PIP ranges (counter12
seed 1: 756-795 varying; counter16 seed 0: 1297-1430 varying;
counter8 seed 2: 341-393 varying).

| design | runs | iters | PIPs | base rate |
| --- | --- | --- | --- | --- |
| counter12 seed 1 | 5/5 exit 0 | 22 every run | 763 every run | 2/5 pass |
| counter16 seed 0 | 5/5 exit 0 | 28 every run | 1446 every run | 2/5 pass |
| counter8 seed 2 | 5/5 exit 0 | 8 every run | 343 every run | 5/5 route |

Fix: 15 of 15 pass, 0 timeouts, 0 variance.

Coordinator re-runs from the merged branch binary
(`~/workspace/n48-logs/coord/run-coord-flows.sh`,
`coord-flow-summary.txt`): counter12 seed 1 twice, 22 iters, 763
PIPs, .bit md5 `85d62a0f5aaec7372031b6f837959d74`, byte-identical to
the impl fix run and the stash restore run; counter16 seed 0 twice,
28 iters, 1446 PIPs, .bit md5 `12a513526a0396b61435c88ed1c5ea0a`;
counter8 seed 2 twice, 8 iters, 343 PIPs, .bit md5
`21b58a65cbf6b7834bc850d4ecd1606c`. Repeat runs within the
coordinator worktree are byte-identical (`cmp` clean on all three
designs).

## 5. Acceptance verify-bits: 5 runs per design on the fix

Port maps: counter12 seed 1 uses the canonical AQ map (clk
SLICE_X1Y0.CLK1, led SLICE_X1Y0.AQ). Counter16 seed 0 and counter8
seed 2 had no working map on the base; both were settled by pin
sweeps in this lane.

| design | cycles | runs | result |
| --- | --- | --- | --- |
| counter12 seed 1 | 400 | r1-r5 | 5/5 PASS, cosim 800/0 each |
| counter12 seed 1 | 2100 | r1 only | PASS, cosim 4200/0 (bit 11 flips at 2048, confirms the AQ pin holds cnt[11]) |
| counter16 seed 0 | 40000 | r1-r5 | 5/5 PASS, cosim 80000/0 each |
| counter8 seed 2 | 400 | r1-r5 | 5/5 PASS, cosim 800/0 each |

Counter16 seed 0 map (settled by sweep): the seed 1 map (led
SLICE_X4Y0.AQ) fails on seed 0 with 40001 of 80000 mismatches, led
net=1 from cycle 0. Settled map
(`counter16-s0-port-map.json`): clk SLICE_X1Y0.CLK1, led
SLICE_X1Y0.AQ. The X7Y0 candidate fails at cycle 1023 (= 2^10 - 1),
confirming the N47 bit-identification method. Counter8 seed 2 map
(settled by sweep): the s0/s1 AQ map (led SLICE_X1Y0.AQ) fails
exactly as W-BASELINE recorded (401 of 800, led stuck at 1); the
FFMUX inventory differs between the seed 2 and seed 0 FASMs. Settled
map (`counter8-s2-port-map-X4Y0-AQ.json`): clk SLICE_X1Y0.CLK1,
led SLICE_X4Y0.AQ.

Coordinator verify-bits from the merged branch binary
(`run-coord-verify.sh`, `coord-verify-summary.txt`): counter12 seed
1 PASS 800/0, counter16 seed 0 PASS 80000/0 at 40000 cycles,
counter8 seed 2 PASS 800/0, all with fixture RTL, --top, absolute
--workdir, branch-built decoder with US09V_DECODER_BIN override.

## 6. Regressions on the fix

Impl flows and verify-bits (`run-fix-regressions.sh`,
`run-verify-regressions.sh`); coordinator flows and verify-bits
from the merged branch binary (`run-coord-flows.sh`,
`run-coord-verify.sh`). Numbers agree exactly between impl and
coordinator.

| design | flow (coord) | verify-bit (coord) |
| --- | --- | --- |
| counter12 seed 0 | 5/5 exit 0, 20 iters, 804 PIPs (coord r1) | PASS, 400 cycles, 800/0 |
| counter8 seed 0 | exit 0, 18 iters, 377 PIPs | PASS, 400 cycles, 800/0 |
| counter8 seed 1 | exit 0, 9 iters, 339 PIPs | PASS, 400 cycles, 800/0 |
| counters2x4 | exit 0, 10 iters, 320 PIPs | PASS, 400 cycles, 800/0 |
| counter5 | exit 0, 5 iters, 76 PIPs | PASS, 400 cycles, 800/0 |
| counter | exit 0, 2 iters, 58 PIPs | PASS, 400 cycles, 800/0 |
| blinky | exit 0, 1 iter, 4 PIPs | PASS, 400 cycles, 800/0 |
| counter16 seed 1 | exit 0, 37 iters, 1244 PIPs | PASS, 40000 cycles, 80000/0 |

Blinky `.bit` md5 `d82b70c8e29ce72f28e08c94f3a16d16`, unchanged
from the N47 lane, the baseline, and the coordinator's own run. All
8 regression verify-bits pass. The coordinator's counter16 seed 1
FASM carries 1 CTRL_E4 line (the N47 east-side leg intact); the
counter12 seed 1 FASM carries 1 CTRL_W5 line (the CLK2 leg intact).

## 7. Suites (coordinator-recounted from the merged branch logs)

All three suites run serially at -j1 by the coordinator from the
merged branch binary; no cargo suite overlapped any checker run.

| suite | command | result | log |
| --- | --- | --- | --- |
| helion-ultrascale (debug) | `cargo test -j1 -p helion-ultrascale` | exit 0, 456 passed / 0 failed / 1 ignored, 35 test-result lines | `suite-ultrascale.log` |
| helion-cli (release) | `cargo test --release -j1 -p helion-cli` | exit 0, 89 passed / 0 failed / 1 ignored, 11 test-result lines | `suite-cli-release.log` |
| US-09 checkers | `python3 -m unittest discover -s tests` in `tools/us09v`, iverilog 14.0 on PATH | exit 0, Ran 100 tests in 117.609s, OK | `checkers-us09.log` |

Ultrascale is base 453/0/1 plus exactly the 3 new tests (the two
`n48_route` tests and `fanout_lists_are_sorted_deterministic`, all
present and passing by name in the coordinator log). CLI is
identical to base 89/0/1. Checkers ran as a single full discover
run here (the baseline worker's full run was OOM-killed after 113
tests while other heavy jobs ran; this run had no other heavy job
running). Impl numbers match exactly (456/0/1, 89/0/1, 100 OK in
133.557s). System iverilog 12.0 absent (N49); oss-cad-suite 14.0
used.

## 8. Stash proof (both sides)

`fix.patch` (`~/workspace/n48-logs/impl/fix.patch`, 780 lines, 5
files, sha256
`9080baf8fa6e9a23b82f753578f9915922a8c623c87013892dd97236fab11eb6`)
is `git diff 4720575d HEAD` from the impl worktree. The lane-branch
merge diff (`git diff 4720575d..116c629c`) is byte-identical to
`fix.patch` (coordinator-verified by `cmp`). Stash worktree:
`~/workspace/helion-ultra-bitgen-n48-stash`, detached HEAD at
`4720575d`.

Base side (stash binary built from unmodified `4720575d`): counter12
seed 1 r1 exit 0 (36 iters), r2 exit 1 with the verbatim line:

```
impl: ULTRA ROUTE: route failed for net 'cnt_5' at wire 4919 at tile (3,0) shared by 2 nets after 50 iters: OveruseTimeout
```

Restore side (`fix.patch` applied, rebuilt): counter12 seed 1 5 of
5 exit 0, 22 iters, 763 PIPs every run, matching the impl worktree
fix runs exactly. The restore r1 `.bit` md5
`85d62a0f5aaec7372031b6f837959d74` is byte-identical to the impl fix
r1 `.bit` md5, and to the coordinator's merged-branch r1 and r2
`.bit` md5s: the patch alone, not any other worktree state,
produces the fix behavior, and the fix is deterministic across
three worktrees and builds. One interruption to record plainly: a
runtime restart killed the first stash-fix rebuild after
`helion-bitgen` had rebuilt but before `helion` finished; the
partial state was verified from file timestamps and sizes, then the
same build command was re-run and completed exit 0. No evidence was
lost. One coordinator interruption to record plainly: the first
coordinator verify-bit run failed on every design (exit 1, "No
module named us09v") because the script ran from the wrong working
directory; the script was fixed to `cd` into `tools/us09v` first
and re-run cleanly, 11 of 11 PASS. The failed run produced no
verify results and is not counted anywhere.

## 9. Build sizes (N50/N54: same command only)

Coordinator merged-branch build, command `cargo build --release -j2
-p helion-cli -p helion-ultrascale`, log `build-release.log`, exit 0,
8m 17s: helion 12,926,664 bytes, md5
`4c8a5e29d1d9521d3069fc0c49414046`; helion-bitgen 5,737,424 bytes,
md5 `006a37b604c9cecf14e3cea9b2360cb9`. Impl fix build, same command:
helion 12,926,744 bytes (-80 vs coordinator), helion-bitgen
5,737,424 bytes (same size). The impl base build, same command:
helion 12,929,912 bytes, helion-bitgen 5,739,536 bytes. Within one
worktree the fix deltas are -3,168 / -2,112 bytes (impl worktree);
across worktrees the same command gives different absolute bytes
(see N54). Compare sizes only across identical build commands in
the same worktree.

## 10. Honest limits, stated plainly

- The fix is proven on the N48 design set only: counters up to 16
  bits, 1-47 nets, convergence 1-37 iters. No larger netlist
  (32-bit counters, DSP/BRAM designs) has been measured under the
  fix, so the convergence margin on bigger designs is unproven (see
  N55). The 50-iteration budget is unchanged, which bounds the
  risk, but a large design could still grind near the budget.
- The determinism claim is proven at the flow level on the measured
  designs (repeat runs byte-identical, including across worktrees
  for counter12 seed 1). It is not proven for designs outside the
  measured set.
- No CTRL_E5 leg was emitted in any flow in this lane, same as
  N47: no design places EFGH FFs on an east slice. E5 stays
  unit-test proven, not flow proven.
- The counter8 seed 2 verify gap is CLOSED by this lane (section
  5), but only through the swept port map: the flow itself did not
  change for seed 2, and the map is seed-specific. A future design
  with a new placement still needs its own sweep.

## 11. New nits (numbered after N53)

- N54 (N50 class): binary bytes depend on the worktree, not just
  the build command. The identical `-j2 -p helion-cli
  -p helion-ultrascale` command in two worktrees at the same commit
  gives helion sizes 12,929,912 and 12,929,928 bytes (base) and
  12,926,744 and 12,926,664 bytes (fix), and helion-bitgen the same
  size with different md5s. Size deltas are only meaningful within
  one worktree.
- N55: the fix is proven on the N48 design set only (counters up
  to 16 bits, 1-47 nets, convergence 1-37 iters). No larger netlist
  has been measured under the fix, so the convergence margin on
  bigger designs is unproven.
- N56: `rustfmt --check` on the touched files shows pre-existing
  deviations (ultra.rs 22 diffs, wire_db.rs:434,
  router/tests.rs). None are in this lane's edited regions, and the
  two new test files are fmt-clean, so the files were left
  unreformatted. A repo-wide fmt pass would churn unrelated lines;
  it needs its own lane if wanted.

## 12. Carried, not fixed here

- N51 (stale doc sentence at ultra.rs 1230-1232; Session 44 ruled
  it NOT fixed). N53 (stale packer comment at ultra.rs 1638-1648).
  Both are doc-only, same class, untouched by this lane.
- N49 (system iverilog 12.0 absent; oss-cad-suite 14.0 used).
  N52 pre-existing E0133 warnings (3 occurrences in the CLI suite
  log).
- N50 class (see N54). N35/N36 router nondeterminism: the root
  cause is removed by F1 for the measured designs, but the nits
  stay open until a critic rules on whether the fix closes them.
- STA din gate for top-level inputs (chain16, shift32) still open.
  IOB placement still blocked on B1. INT_INTF tile model for
  DSP/BRAM routing still open. Known-INIT Vivado datum still open.
  Documented xczu3eg timing still open. Remaining 54,907 non-band
  unexplained bits still open. N16 parse-side open. N9 block types
  2+ untested. N34 stale default decoder. N37. N38. Flags A-CTRL
  and A-F7F8 still unresolved, need a CTO ruling. No new nits from
  this lane beyond N54-N56.

## 13. Evidence index

- `~/workspace/n48-logs/baseline/BASELINE_REPORT.md`,
  `build-release.log`, `run-n48-flows.sh`,
  `run-n48-flows-resume.sh`, `flow-summary.txt`,
  `flow-*.log`, `*-r<n>.bit/.fasm`, `run-regression-flows.sh`,
  `run-verify.sh`, `verify-*.log`, `run-suites.sh`,
  `suite-ultrascale.log`, `suite-cli-release.log`,
  `checkers-us09.log` (partial, OOM-killed),
  `checkers-test_*.log`, `checkers-split-summary.txt`
- `~/workspace/n48-logs/impl/IMPL_REPORT.md`, `fix.patch`,
  `build-release-base.log`, `build-release-fix.log`,
  `run-baseline-repro.sh`, `base-repro-summary.txt`,
  `dbg-c12-s0-r*.log`, `dbg-c16-s0-r1.log`, `offline_graph.py`,
  `harness-*-dist.log`, `run-fix-accept.sh`,
  `fix-accept-summary.txt`, `fix-*.bit/.fasm`,
  `run-verify-accept.sh`, `run-verify-remaining.sh`,
  `verify-accept-summary.txt`, `verify-remaining-summary.txt`,
  `verify-fix-*.log`, `run-sweep-c16-s0.sh`,
  `sweep-c16-s0-summary.txt`, `counter16-s0-port-map.json`,
  `counter8-s2-port-map-X4Y0-AQ.json`, `run-fix-regressions.sh`,
  `fix-reg-summary.txt`, `run-verify-regressions.sh`,
  `verify-reg-summary.txt`, `suite-ultrascale-fix.log`,
  `suite-cli-fix.log`, `checkers-us09-fix.log`,
  `suite-n48route-fmt.log`, `build-release-stash-base.log`,
  `run-stash-base.sh`, `stash-base-summary.txt`,
  `stash-base-c12-s1-r<n>.log`, `build-release-stash-fix.log`,
  `build-release-stash-fix2.log`, `run-stash-fix.sh`,
  `stash-fix-summary.txt`, `stash-fix-c12-s1-r<n>.bit/.fasm`
- `~/workspace/n48-logs/coord/build-release.log`,
  `suite-ultrascale.log`, `suite-cli-release.log`,
  `checkers-us09.log`, `run-coord-flows.sh`,
  `coord-flow-summary.txt`, `flow-coord-*.log`,
  `coord-*.bit/.fasm`, `run-coord-verify.sh`,
  `coord-verify-summary.txt`, `verify-coord-*.log`,
  `verify-coord-*/`
