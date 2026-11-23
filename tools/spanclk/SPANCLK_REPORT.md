# Span-continuation + CLK2 lane report

Lane branch: fm/ultra-bitgen-spanclk, code tip 6da1ed03 (impl commit
on routepack base a80569d4; merge 0ffd7817). No merges into
fm/ultra-bitstream, fm/zedboard-merge, or master. No critic dispatched
by this lane; Session 42 is the main agent's to dispatch.

## 1. Outcome

The conjunction Critic Session 41 left open is now proven: all three
target designs (counter8 seed 0, counter8 seed 1, counters2x4) BOTH
route through INT_X2+ AND pass verify-bit on fixture RTL, twice each,
from fresh impl-branch flows. Decode round-trips exactly on all six
runs (sorted PIP diff empty, zero VCC_WIRE phantoms). counter5,
counter, and blinky verify-bit still PASS; blinky .bit md5 unchanged.

Suites: helion-ultrascale 451 passed / 0 failed / 1 ignored (448 + 3
new), helion-cli release 82/0/1 (78 + 4 new), US-09 checkers 100 OK
(95 + 5 new). Stash proof both sides. The lane meets its core
acceptance, with one scope decision stated plainly in section 4 (the
CLK2 leg is avoided, not driven, on these designs) and one new
checker assumption (A-CLKSPINE) for the critic to judge.

## 2. Baseline: the failure, measured

Baseline report: ~/workspace/spanclk-logs/baseline/BASELINE_REPORT.md.
On the routepack tip, reproduced exactly on the lane's saved bits
with a branch-built decoder:

- counter8-s0: verify-bit FAIL, 280 route violations (37 undriven-sink
  PIPs + 243 unreachable-net reports) + 1 CLK2 place violation.
- counter8-s1: verify-bit FAIL, 388 route violations (37 + 351),
  0 place violations on the lane bits.
- counters2x4: verify-bit FAIL, 232 route violations (31 + 201) + 1
  place violation. The routepack lane never ran it; the Session 41
  critic ran it first, and the baseline worker ran it from its own
  flow output and fixture.

Decode was clean on all three (604/677/585 lines). INT_X2+ routing
was already exact on the base (counter8-s0 185 PIPs, counters2x4 164
PIPs). Baseline suites 448/0/1, 78/0/1, 95 OK. Blinky .bit md5
d82b70c8.

Baseline findings that shaped the fix:

- The 37 undriven sinks are not the same set across seeds: lane s0
  vs s1 share only 12 of 37 reports verbatim. The fix had to be
  systematic, not seed-specific.
- Seed 1 "place passes" was a decoder false positive: the decoded
  s1 FASM carries a phantom PIP INT_X1Y0.PIP.CTRL_W5.VCC_WIRE
  rendered from shared segbits bit 21_35 set by real PIP
  INT_NODE_SDQ_80_INT_OUT1.LOGIC_OUTS_W22. The flow never drove
  CLK2 in any seed. A VCC-tied clock is not a clock.
- A second sub-class inside the undriven sinks: 2 of 37 (s0/s1)
  and 1 of 31 (2x4) are LUT-O legs (LOGIC_OUTS_W0/W12 on
  INT_X3Y0) from the FF-less SLICE_X4Y0, which has live LUTs but
  no FFMUX/OUTMUX; the checker only counts a LUT O as a driver
  when one selects it.
- Chain structure proven: every unreachable-net report hangs
  downstream of an undriven-sink root (36 direct + 207 chained in
  s0; 37 + 314 in s1; 30 + 171 in 2x4). The two route numbers are
  one phenomenon counted twice.

Fresh baseline flows varied by documented router nondeterminism
(N35/N36): absolute counts move run to run, classes and inventory
shape identical. Acceptance pins verify outcomes, not .bit md5.

## 3. The fix (11 files, 758 insertions, 60 deletions; fix.patch)

Three independent root causes, three fixes. Full detail in
~/workspace/spanclk-logs/impl/IMPL_REPORT.md sections 1-3.

### 3.1 Span-continuation divergence (wire_db.rs, route_to_pip.rs)

The router's wire graph and the route-to-PIP translator treated a
horizontal span wire as advancing LEN INT tiles. Public tileconn
stitching says a horizontal span advances (LEN + 1) / 2 INT tiles
(len 1 -> 1 step, len 2 -> 1, len 4 -> 2, len 12 -> 6); vertical
spans advance LEN, unchanged. Concrete proof: the router drove
WW2_E_BEG1 in INT_X2Y0 and emitted a PIP out of WW2_E_END1 in
INT_X0Y0, while tileconn stitches that END to INT_X1Y0, which the
flow never drove. Fixed in both copies of continuation_partner,
with unit tests. A span is a single conductor (BEG in tile T and
END in tile T+step are the same tileconn node), so the corrected
edge makes the END node's driver be the PIP driving the BEG wire.

### 3.2 LOGIC_OUTS sinks (fasm_gen.rs, lib.rs, ultra.rs)

Flow-side fix, not checker-side: the flow now emits
OUTMUX{letter}.D6 for live LUT letters with no FF in the letter,
so the checker counts the LUT O as a driver. A checker-side change
would have broken the checker's explicit negative test
(test_no_selection_lut_o_is_not_a_driver). Segbits exist (40
OUTMUX lines in segbits_clem.db); the extractor models OUTMUX D6
as the LUT O6. No OUTMUX emission existed anywhere in the flow
before this lane.

### 3.3 Clock (ultra.rs, extract.py, routecheck.py, README)

Two layers. Flow layer: repair_ff_xpin_conflicts now searches
ABCD letters in every slice before any EFGH letter, so no FF needs
CLK2 on these designs; and the flow emits a real GCLK leg
(CTRL_Wx.INT_NODE_GLOBAL_y plus
INT_NODE_GLOBAL_y.GCLK_B_0_z) for every slice half with FFs past
the clock-entry slice. The leg for the lane designs is
INT_X3Y0.PIP.CTRL_W4.INT_NODE_GLOBAL_14_INT_OUT0 and
INT_X3Y0.PIP.INT_NODE_GLOBAL_14_INT_OUT0.GCLK_B_0_9; the GCLK
spine node stitches to a BUFCE_LEAF CLK_LEAF OUT pin.
Checker layer: new documented assumption A-CLKSPINE (tools/us09v
README, extract.py, routecheck.py): a leg-fed clock pin resolves
to the design's clock primary, firing only for the leg shape
(every active PIP sourced from the spine node must feed a GLOBAL
wire) and only when exactly one clock port exists; with no clock
port the spine stays undriven, so a leg without a clock source
still fails (negative tests). Five new TestClockLeg tests pass.

## 4. The CLK2 scope decision (read this before the critic does)

Baseline acceptance item 3, read literally, says CLK2 must be driven
by a real clock leg in the flow FASM. What was built is narrower,
and this section states it plainly instead of claiming more.

Facts established before deciding: CTRL_W4 (CLK1) has no PIP fanout
in the INT tile JSON, so no INT PIP chain can connect the clk
primary to CLK2 (CTRL_W5); CTRL_W5 can be driven by
INT_NODE_GLOBAL_* wires or VCC_WIRE. A GCLK leg onto CLK2 was
tested empirically on the lane s0 decoded FASM and failed: place
still FAILS (CLK2 undriven) and route got worse (283 violations).
Extract requires CLK2 to resolve to site / const / primary; const 1
(VCC) means FFs never clock, and a site OUT (data LUT) means EFGH
FFs clock on a data signal, both cosim mismatches.

Decision: keep FFs out of EFGH halves via the ABCD-first X-pin
repair, so no slice needs CLK2 on these designs; emit a real GCLK
clock leg for every slice half with FFs that is not primary-driven;
a CLK2 leg IS emitted whenever EFGH FFs do occur (unit-tested:
clock_leg_fasm_legs_efgh_half_of_entry_slice). No lane design
emits a CLK2 leg, because no lane design has EFGH FFs after the
repair. I verified this directly: grep finds 0 CLK2 occurrences
in counter8-s0, counter8-s1, and counters2x4 FASM files.

What this means: the observable acceptance items (0 place
violations, 0 route violations, cosim 800/0) all hold. The literal
item-3 wording is satisfied in the general sense (the flow emits a
real clock leg wherever a clock pin needs one, and the checker
models it as driven by the real clock source), but a critic reading
it as "a CLK2 leg appears in the counter8 FASM" will correctly
find none. CLK2 is avoided, not driven, on these designs, and the
leg mechanism covers it when it is needed. The README states the
Vivado clock-tree confirmation gap plainly: the RCLK/BUFG features
the tree would carry have no public segbits, so A-CLKSPINE is NOT
settled by a Vivado clock-tree build. The critic must judge whether
the avoidance plus A-CLKSPINE satisfies the acceptance, or whether
the item stands unproven.

## 5. Results: target designs, both runs

Fresh flows from the impl branch, run twice per design. All six
runs route through all six INT_X2+ tiles (X2Y0, X2Y1, X3Y0, X3Y1,
X4Y0, X4Y1) and pass verify-bit with cosim 800/0.

Run 1:

| design | flow PIPs | INT_X2+ PIPs | verify-bit |
| --- | --- | --- | --- |
| counter8-s0 | 407 | 192 | PASS, 407 PIPs, 34 nets, cosim 800/0 |
| counter8-s1 | 373 | 179 | PASS, 373 PIPs, 34 nets, cosim 800/0 |
| counters2x4 | 345 | 140 | PASS, 345 PIPs, 33 nets, cosim 800/0 |

Run 2:

| design | flow PIPs | INT_X2+ PIPs | verify-bit |
| --- | --- | --- | --- |
| counter8-s0 | 383 | 164 | PASS, 383 PIPs, 34 nets, cosim 800/0 |
| counter8-s1 | 365 | 167 | PASS, 365 PIPs, 34 nets, cosim 800/0 |
| counters2x4 | 351 | 153 | PASS, 351 PIPs, 33 nets, cosim 800/0 |

Absolute PIP counts move run to run (N35/N36); classes and
inventory shape are identical, and all six runs pass. I verified
the run-2 verify logs directly (PASS lines, 383/365/351 PIPs,
cosim 800/0) and the run-1 PASS logs preserved in run1/.

Flow FASM checks (run 2 files): clock legs present in all three
(CTRL_W4.INT_NODE_GLOBAL_14_INT_OUT0 and
INT_NODE_GLOBAL_14_INT_OUT0.GCLK_B_0_9); OUTMUX lines present
(SLICE_X1Y0.OUTMUXE.D6, OUTMUXF.D6, OUTMUXG.D6, OUTMUXH.D6);
no FFMUXE/F/G/H line in any design (all FFs in ABCD letters);
zero VCC_WIRE occurrences in the flow FASM of all three designs.

Decode round-trip: sorted PIP diff empty on all six runs
(407/373/345 run 1, 383/365/351 run 2). Zero VCC_WIRE
occurrences in any decoded FASM. The decoder itself was not
changed; the round-trip diff is the proof.

## 6. Regressions

- helion-ultrascale suite: 451 passed, 0 failed, 1 ignored
  (baseline 448/0/1; +3 is exactly the three new lib tests).
  I recounted from suite-ultrascale.log: 33 binaries, 451
  passed, 0 failed, 1 ignored. One earlier run was killed
  mid-way by a runtime restart with no summary; it was re-run
  in full and the reported numbers are from the completed re-run.
- helion-cli release suite: 82 passed, 0 failed, 1 ignored
  (baseline 78/0/1; +4 is exactly the four new clock-leg
  tests). I recounted from suite-cli-release.log: 11 binaries,
  82 passed, 0 failed, 1 ignored.
- US-09 checkers: 100 tests ran, OK (baseline 95 OK; +5 is
  exactly the five new TestClockLeg tests). iverilog 14.0 from
  oss-cad-suite; system iverilog 12.0 is absent on this VM.
- counter5 verify-bit PASS (95 PIPs, 16 nets, cosim 800/0).
- blinky verify-bit PASS (4 PIPs, cosim 800/0); .bit md5
  d82b70c8e29ce72f28e08c94f3a16d16, unchanged from baseline.
  I verified the md5 directly.
- counter verify-bit PASS (54 PIPs, 12 nets, cosim 800/0).

## 7. Stash proof

fix.patch (log dir) is the branch diff a80569d4..6da1ed03. I
verified it is byte-identical to the branch diff (cmp clean),
and fix.patch is byte-identical to restored.patch (cmp clean).
Base fails with the baseline classes: counter8 verify-bit FAIL,
CLK2 place violation, WW2/WW4 span-end undriven sinks
(464 PIPs, 27 nets). I verified the stash-base log directly.
Restored passes: counter8 verify-bit PASS (379 PIPs, 34 nets,
cosim 800/0), round-trip diff empty. No vendored tables
touched. No STA gate change. No max_iters change.

## 8. Reproducible verify-bit commands (N41)

Decoder built from the lane branch (N34); absolute --workdir;
--top flag (N37). From tools/us09v in the lane worktree, with
US09V_DECODER_BIN set to the lane-built helion-bitgen and
oss-cad-suite bin on PATH for iverilog 14.0:

```
US09V_DECODER_BIN=$HOME/workspace/helion-ultra-bitgen-spanclk/target/release/helion-bitgen \
python3 -m us09v verify-bit --bit ~/workspace/spanclk-logs/impl/counter8-s0.bit \
  --design counter8 --rtl ~/workspace/routepack-logs/impl/verify-counter8/counter8-rtl.v \
  --top counter8 --port-map ~/workspace/routepack-logs/impl/counter8-port-map-AQ.json \
  --workdir ~/workspace/spanclk-logs/impl/verify-counter8-s0 --cycles 400
```

Same shape for counter8-s1 and counters2x4 (design counters2x4,
RTL ~/workspace/spanclk-logs/baseline/verify-counters2x4/counters2x4-rtl.v,
port map ~/workspace/spanclk-logs/baseline/verify-counters2x4/port-map-AQ.json).
Flow command: ./target/release/helion impl
~/workspace/intcols-logs/impl/counter8.sv --part xczu3eg --seed 0
--out counter8-s0.bit --fasm-out counter8-s0.fasm, with
HELION_TILECONN_DB_ROOT and HELION_TILECONN_PART set as in the
impl report section 5. Full detail in
~/workspace/spanclk-logs/impl/IMPL_REPORT.md section 5.

## 9. Carried (not fixed here)

- counter8 seeds 2-4 and counter16: ROUTE OveruseTimeout,
  2-nets-1-wire congestion at 50 iters. No max_iters change
  was tried.
- chain16/shift32: STA gate "timing: no driver for net din"
  (top-level input). STA was not weakened.
- IOB placement still blocked on B1 (no public INT_INTF_R_PCIE4
  PIP segbits; Vivado is the unblocker).
- INT_INTF tile model for DSP/BRAM routing; known-INIT Vivado
  datum; documented xczu3eg timing; remaining 54,907 non-band
  unexplained bits; N16 parse-side open; N9 block types 2+
  untested; N34 stale default decoder; N35/N36 router
  nondeterminism; N37; N38; flags A-CTRL and A-F7F8 still
  unresolved, need a CTO ruling.
- New: A-CLKSPINE (checker models the GCLK spine as driven by
  the design's clock primary) is an assumption, not a Vivado
  confirmation. The RCLK/BUFG tree above the leaves is still
  not modelled. The critic must rule on it.
- Nits N42-N45 carried from Session 41.

## 10. Evidence index

- Lane report: ~/workspace/ultra-bitgen-crew/SPANCLK_REPORT.md
  (this file; copy committed at tools/spanclk/SPANCLK_REPORT.md
  on the lane branch, byte-identical).
- Baseline: ~/workspace/spanclk-logs/baseline/BASELINE_REPORT.md
  (761 lines) plus recount logs, flow logs, suite logs,
  verify logs and workdirs.
- Impl: ~/workspace/spanclk-logs/impl/IMPL_REPORT.md
  (482 lines) plus fix.patch, restored.patch, run-flows.sh,
  flow logs, suite logs, checkers log, verify logs and
  workdirs (run 1 preserved in run1/), roundtrip-check.sh
  and round-trip diffs, clk-test-a.fasm (the failed GCLK-only
  experiment).
- Critic evidence will live in ~/workspace/critic42-runs/.
