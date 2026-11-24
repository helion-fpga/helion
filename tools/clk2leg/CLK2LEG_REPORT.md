# CLK2 clock-leg lane report

Lane branch: fm/ultra-bitgen-clk2leg, code tip d25fd12f (impl commit
on spanclk base f7635c02; merge fd8cd492). No merges into
fm/ultra-bitstream, fm/zedboard-merge, or master. No critic dispatched
by this lane; Session 43 is the main agent's to dispatch.

## 1. Outcome

Acceptance item 3 is now proven. Counter12 (12 FFs, forcing EFGH
placement) emits a real CLK2 leg and passes verify-bit end to end on
the lane branch. The coordinator re-ran the acceptance from the merged
branch binary: flow exit 0, CLK2 leg present in FASM, conflicting PIP
absent, verify-bit PASS, cosim 800/0. The N46 bitgen conflict is gone
as a failure class: on the fix branch it never recurs; the only
remaining counter12 failures are the carried ROUTE OveruseTimeout
congestion class (nit N48).

Suites on the merged branch, run by the coordinator: helion-ultrascale
453 passed / 0 failed / 1 ignored (451 + 2 new), helion-cli release
87/0/1 (82 + 5 new), US-09 checkers 100 OK. Stash proof both sides.
Blinky .bit md5 unchanged.

## 2. Baseline: the failure, measured

Baseline report: ~/workspace/clk2leg-logs/baseline/BASELINE_REPORT.md
(597 lines). On the spanclk tip, reproduced exactly on fresh flows
with a branch-built decoder:

- counter12 seed 0, exit 1 at bitgen, verbatim: `tile INT_X1Y0
  feature PIP.CTRL_W5.INT_NODE_GLOBAL_10_INT_OUT1: bit 35 of FAR
  0x00000415 must read 0 but reads 1`. The FASM contains the CLK2 leg
  (line 951) and the conflicting PIP
  `INT_X1Y0.PIP.INT_NODE_SDQ_80_INT_OUT1.LOGIC_OUTS_W22` (line 645).
  No .bit is produced.
- counter12 seed 1, exit 1 at ROUTE OveruseTimeout, net `n0`, wire
  1508, tile (1,0), the carried N48 class.
- counter16 seed 1, exit 1 with the N47 resolver failure, verbatim:
  `ULTRA CLOCK LEG: slice SLICE_X10Y0 pin CLK1 resolved to CTRL_E4,
  expected CTRL_W4`.

Baseline suites 451/0/1, 82/0/1, 100 OK, matching Session 42 exactly.

The baseline's conflict table (report section 6) is the key
measurement: all 8 CTRL_W5 GLOBAL leg candidates require bit 21_35
clear, and the only routed PIP setting it is the SDQ_80 PIP (the
other setter DB-wide is CTRL_W5.VCC_WIRE, a decoded default, never
routed). Checked mechanically both directions, on the baseline FASM
and the critic's FASM: 21_35 is the sole contested clear bit for
every candidate. No GLOBAL leg can emit on INT_X1Y0 with the SDQ_80
OUT1 PIP routed. CTRL_E5 legs do not touch 21_35.

Fix-direction evidence from the baseline: the SDQ_80 net is SLICE_X1Y0
FQ feeding SLICE_X4Y0 D3 three columns east; five alternative PIPs
from the same source wire exist, none setting any leg clear bit, and
the lane-5 reroute (SDQ_78 OUT0 to EE2_W_BEG5) is structurally
available. Control case already in evidence: the critic's counter16
seed 0 routes the same wire via SDQ_80 OUT0 and the CLK2 leg emits
with a .bit produced.

## 3. The fix (3 files, 736 insertions, 52 deletions; fix.patch)

Full detail in ~/workspace/clk2leg-logs/impl/IMPL_REPORT.md
sections 1-2. Three mechanisms, all general (no hardcoded design,
seed, or single-PIP special case beyond what the DB itself proves),
plus a FASM-emit-time check. No invented segbits; the bitgen check
is not weakened.

### 3.1 Routing avoidance from the segbits DB (general form)

`clock_leg_forbidden_pips` in `crates/helion-cli/src/ultra.rs`
computes, from the vendored segbits DB, the PIPs that must not be
routed on a clock-leg tile: a PIP is forbidden when it disagrees (one
requires set a bit the other requires clear, in either direction)
with every candidate leg pair (the `CTRL.GLOBAL` PIP and the
`GLOBAL.GCLK` PIP) for that tile's CTRL wire. Leg tiles come from
`clock_leg_targets`, factored out of `clock_leg_fasm`: slices with
EFGH FFs after place, resolved through `wire_db.clock_int_tile`. A
missing leg entry in the DB fails loud instead of forbidding nothing.

For the counter12 leg tiles the computed set is exactly three PIPs:
`INT_X1Y0 SDQ_80/W22` (the N46 PIP, sets 21_35),
`INT_X1Y0 CTRL_W5.VCC_WIRE`, and `INT_X3Y0 CTRL_W4.VCC_WIRE`. The
forbidden edges are removed before routing by `RouteDb::forbid_pips`
(`wire_db.rs`, `tileconn.rs`); the translator derives parents via
`db.fanout`, so a removed edge can never be emitted. Each forbidden
PIP is printed as one `hang_diag ultra clock_leg_forbidden` line.

Measured alternative the router takes: the FQ net leaves
LOGIC_OUTS_W22 through `INT_NODE_SDQ_78_INT_OUT0` onto `EE2_W_BEG5`
(lane 5), the parallel lane the baseline predicted.

### 3.2 Segbits-compatible leg pick

`clock_leg_fasm` now takes the segbits DB and picks the first
candidate whose GLOBAL wire is unused AND whose two leg PIPs disagree
with no already-programmed PIP on the tile
(`clock_leg_candidate_compatible`). With no segbits it delegates to
the unchanged `pick_clock_leg`. Leg PIPs already emitted on the same
tile are tracked, so two legs on one tile must be compatible with
each other.

### 3.3 FASM-emit-time check

`check_fasm_int_pip_bits` runs after FASM assembly and before bitgen:
it fails loud naming the tile, both features, and the bit when two
INT PIP features on the same tile disagree. Unknown features and
non-PIP lines are skipped; the check is per-tile.

### 3.4 Tests (7 new)

- `wire_db::tests::remove_edge_drops_one_directional_pip_only`
- `tileconn::tests::forbid_pip_removes_one_edge_from_the_tile`
- `ultra::tests::clock_leg_targets_cover_efgh_half_of_entry_slice`
- `ultra::tests::clock_leg_forbidden_pips_find_sdq80_and_vcc_conflicts`
  (real vendored DB: the exact 3-entry set)
- `ultra::tests::clock_leg_forbidden_pips_missing_leg_fails_loud`
- `ultra::tests::clock_leg_pick_skips_segbits_conflicting_global`
- `ultra::tests::check_fasm_int_pip_bits_names_both_features`
  (N46 pair fails naming both features and bit 21_35)

## 4. Port-map correction: FQ is cnt_9, AQ is cnt_11

The first port map (led = `SLICE_X1Y0.FQ`) passed verify-bit at 400
cycles, but that pass was weak: at 400 cycles every high counter bit
is still 0, so the map cannot distinguish cnt_9, cnt_10, and cnt_11.
An 800-cycle run with the FQ map failed with 577 mismatches, first
at cycle 511, led stuck at 1. A pin sweep at 800 cycles on the
restore bitstream settled it: AQ PASS 1600/0 (cnt_11), BQ FAIL at
cycle 1 (cnt_1), CQ FAIL at cycle 7 (cnt_3), DQ FAIL at cycle 31
(cnt_5), EQ FAIL at cycle 127 (cnt_7), FQ FAIL at cycle 511 (cnt_9).
AQ was confirmed as cnt_11 by a 2100-cycle run: 4200 comparisons, 0
mismatches. The canonical map is
`counter12-port-map-AQ.json`; the FQ map is kept in the log dir only
as the superseded hypothesis. All counter12 verify-bit results in
this report use the AQ map. Placement is identical across all fix
and restore FASM files, so the AQ map applies to every run.

## 5. Results: counter12

Fix-branch runs (impl report section 4.1-4.2), AQ map, branch-built
decoder:

- Seed 0, 6 runs: 5 exit 0 with a real CLK2 leg and 0 bitgen
  failures, all 5 verify-bit PASS (cosim 800/0). The 6th fails at
  ROUTE OveruseTimeout (net cnt_3, wire 4919, tile (3,0)), the
  carried N48 class. Post-restore run passes at 400 cycles (800/0),
  800 cycles (1600/0), and 2100 cycles (4200/0); decode round-trip
  empty.
- Seed 1, 3 runs: r1 and r3 verify-bit PASS (cosim 800/0); r2 fails
  ROUTE OveruseTimeout (net `n0`, wire 1508, tile (1,0)), the exact
  critic failure class, carried as N48.
- Base for comparison, seed 0, 6 runs: 3 bitgen passes, 1 bitgen
  failure with the exact N46 error, 2 ROUTE OveruseTimeout failures.
  The base failure is run-dependent (N35/N36); the critic and the
  baseline each reproduced N46 on their first try.

Coordinator verification from the merged branch binary (logs in
~/workspace/clk2leg-logs/coord/):

- counter12 seed 0: flow exit 0, 832 PIPs, CLK2 leg present
  (`INT_X1Y0.PIP.CTRL_W5.INT_NODE_GLOBAL_10_INT_OUT1`), SDQ_80 OUT1
  absent, verify-bit PASS, cosim 800/0.
- counter8 seed 0: flow exit 0, verify-bit PASS, cosim 800/0.
- blinky: flow exit 0, verify-bit PASS, cosim 800/0, .bit md5
  d82b70c8e29ce72f28e08c94f3a16d16, unchanged.

## 6. Suites on the merged branch (coordinator, serial, -j1)

| suite | result | log |
| --- | --- | --- |
| helion-ultrascale (debug) | exit 0, 453 passed / 0 failed / 1 ignored, 33 test-result lines (451 + 2 new) | `coord/suite-ultrascale.log` |
| helion-cli (release) | exit 0, 87 passed / 0 failed / 1 ignored, 11 test-result lines (82 + 5 new) | `coord/suite-cli-release.log` |
| US-09 checkers | exit 0, Ran 100 tests, OK, 130.073s, full discover, iverilog 14.0 from oss-cad-suite (system 12.0 absent, nit N49) | `coord/checkers-us09.log` |

No regressions. One runtime restart killed the first merged-branch
release build (SIGTERM, no summary); it was re-run from scratch and
the numbers above are from the completed re-run. The 2 pre-existing
helion-cli warnings (unused `place_with`/`route_with` in main.rs)
are unchanged from base.

## 7. Regressions (fix branch, re-confirmed on the restored binary)

counter8 seed 0 PASS, counter8 seed 1 PASS, counters2x4 PASS,
counter5 PASS, counter PASS, blinky PASS, all cosim 800/0. Blinky
.bit md5 d82b70c8e29ce72f28e08c94f3a16d16, unchanged. The coordinator
re-confirmed counter8 seed 0 and blinky from the merged branch
binary (section 5).

## 8. Stash proof

fix.patch (log dir) is the branch diff f7635c02..d25fd12f: 919 lines,
sha256 e22d977b19007fb16749f716bc2329814971a986dda06ee08b51549beb634ca7.
The coordinator verified it is byte-identical to the branch diff
(cmp clean). Base side: counter12 seed 0 attempt 2 reproduced the
N46 failure verbatim, exit 1 (attempt 1 passed bitgen; the base
failure is run-dependent). Restore side: regenerated diff
byte-identical to fix.patch (same sha256); rebuilt; counter12 seed 0
verify-bit PASS at 400, 800, and 2100 cycles; all 6 regressions
re-confirmed on the restored binary.

## 9. Reproducible commands (N41)

Decoder built from the lane branch (N34); absolute --workdir;
--top flag (N37). From tools/us09v in the lane worktree, with
US09V_DECODER_BIN set to the lane-built helion-bitgen and
oss-cad-suite bin on PATH for iverilog 14.0:

```
US09V_DECODER_BIN=$HOME/workspace/helion-ultra-bitgen-clk2leg/target/release/helion-bitgen \
python3 -m us09v verify-bit --bit ~/workspace/clk2leg-logs/coord/coord-s0.bit \
  --design counter12 --rtl ~/workspace/critic42-runs/counter12-rtl.v \
  --top counter12 --port-map ~/workspace/clk2leg-logs/impl/counter12-port-map-AQ.json \
  --workdir ~/workspace/clk2leg-logs/coord/verify-coord-s0 --cycles 400
```

Expected: `counter12: PASS`, cosim 800 comparisons, 0 mismatches.
Flow command: ./target/release/helion impl
~/workspace/critic42-runs/counter12.sv --part xczu3eg --seed 0
--out coord-s0.bit --fasm-out coord-s0.fasm, with
HELION_TILECONN_DB_ROOT and HELION_TILECONN_PART set as in the
spanclk impl report section 5. Full detail in
~/workspace/clk2leg-logs/impl/IMPL_REPORT.md section 8.

## 10. Carried (not fixed here)

- N47: east-side clock-leg resolver. No counter12 run placed FFs on
  an east slice, so it was never triggered. The fix is fully
  specified by the baseline (section 7): accept CTRL_E4 (CLK1) and
  CTRL_E5 (CLK2) alongside CTRL_W4/W5, same 8 GLOBAL candidates,
  same GCLK_B_0_8..15 spine wires. East-side CTRL_E5 legs do not
  touch 21_35, so the N46 class cannot recur there. A future lane can
  implement it and prove it on counter16 seed 1, which currently
  fails loud with `ULTRA CLOCK LEG: slice SLICE_X10Y0 pin CLK1
  resolved to CTRL_E4, expected CTRL_W4`.
- ROUTE OveruseTimeout congestion (nit N48): counter12 seed 0 r5
  (net cnt_3) and seed 1 r2 (net `n0`) still fail at ROUTE on the fix
  branch. Same class and same nets as base; the fix changes which PIPs
  are available on leg tiles, not router capacity.
- Build-to-build binary size variance (nit N50 class): recorded in
  the impl report section 7, behavior verified, size delta not
  explained.
- Carried, unchanged: IOB placement (still blocked on B1); INT_INTF
  tile model for DSP/BRAM routing; known-INIT Vivado datum;
  documented xczu3eg timing; remaining 54,907 non-band unexplained
  bits; N16 parse-side open; N9 block types 2+ untested; N34 stale
  default decoder; N35/N36 router nondeterminism; N37; N38; flags
  A-CTRL and A-F7F8 still unresolved, need a CTO ruling. A-CLKSPINE
  is sound for the CLK1 legs it models; this lane proves a real CLK2
  leg can emit and pass verify-bit, which is what item 3 demanded.
- New nits from this lane: none. N46 is closed by the fix (no N46
  failure on any fix-branch run); N47 stays open with its fix
  specified.

## 11. Evidence index

- Lane report: ~/workspace/ultra-bitgen-crew/CLK2LEG_REPORT.md
  (this file; copy committed at tools/clk2leg/CLK2LEG_REPORT.md
  on the lane branch, byte-identical).
- Baseline: ~/workspace/clk2leg-logs/baseline/BASELINE_REPORT.md
  (597 lines) plus flow logs, FASM files, suite logs, checker logs.
- Impl: ~/workspace/clk2leg-logs/impl/IMPL_REPORT.md plus
  fix.patch, fix-restored.patch, build logs, suite logs, checkers
  log, flow logs and bitstreams, verify logs and workdirs,
  counter12-port-map-AQ.json (canonical) and
  counter12-port-map-FQ.json (superseded).
- Coordinator verification: ~/workspace/clk2leg-logs/coord/
  (build-release-rerun.log, suite-ultrascale.log,
  suite-cli-release.log, checkers-us09.log, flow-coord-s0.log,
  coord-s0.bit/.fasm, verify-coord-s0.log, flow and verify logs
  for counter8-s0 and blinky).
- Critic evidence will live in ~/workspace/critic43-runs/.
