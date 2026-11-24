# STA din gate lane

Lane branch `fm/ultra-bitgen-stadin` at `6d03d47c` on origin (code tip
`5b14e11a`: impl commit on N48 base `ac4d6f68`; merge `6d03d47c`;
report commit pending). Impl branch `fm/ultra-bitgen-stadin-impl` at
`5b14e11a` on origin. Worktree `~/workspace/helion-ultra-bitgen-stadin`,
clean. Report `~/workspace/ultra-bitgen-crew/STADIN_REPORT.md` (this
file; copy committed at `tools/stadin/STADIN_REPORT.md` on the lane branch,
byte-identical). Baseline report at
`~/workspace/stadin-logs/baseline/BASELINE_REPORT.md` (189 lines), impl
report at `~/workspace/stadin-logs/impl/IMPL_REPORT.md` (337 lines).
Logs under `~/workspace/stadin-logs/` (`baseline/`, `impl/`, `coord/`).
Coordinator: STA din gate lane coordinator (dispatched Nov 24, 2026,
about 07:02 EST, after Critic Session 45 closed N48 on the measured
design set and the grind watcher flagged no agent in flight). No critic
was dispatched by this lane; the main agent dispatches Session 46.

## 1. Headline: STA din gate is closed; verify-bit is blocked by a new extractor gap

Chain16 and shift32 both pass STA and flow end to end to bitgen on the
fix, where both stopped at STA with exit 1 on the base. The coordinator
re-ran the acceptance from the merged branch binary: chain16 twice
(exit 0, WNS_PS=8570, endpoints 16, 341 PIPs, `.bit` md5 identical
both runs and identical to the impl fix run and the stash restore run),
shift32 twice (exit 0, WNS_PS=8690, endpoints 32, 280 PIPs, `.bit` md5
identical both runs and identical to the impl fix run and the stash
restore run). All 8 regression designs pass verify-bit on the merged
branch; blinky `.bit` md5 `d82b70c8` unchanged. Suites on the merged
branch: ultrascale 460/0/1, CLI release 89/0/1, US-09 checkers 100 OK.
Stash proof holds both sides.

The honest limit, stated plainly: verify-bit is NOT achieved for
chain16 or shift32. Decode, parse, place, route, and extract all pass
on chain16 with a settled din map, but cosim fails on every dout pin
tried (16 of 16), because the extractor models the first-stage FF's D
as a LUT output instead of din. That is a new extractor gap, numbered
N58 below, out of this lane's scope. The lane closes the STA din gate
as named; it does not close end-to-end verify-bit for these two
designs. The critic must judge whether that is a pass of the lane as
dispatched (STA clearance was the stated minimum; end-to-end
verify-bit was the stated "ideally").

## 2. Baseline: the gate reproduces verbatim on both designs

W-BASELINE built the base (`cargo build --release -j1 -p helion-cli
-p helion-ultrascale`, exit 0, 23m 02s; a first full-workspace build
was stopped after about 46 min still compiling) and ran both fixtures
from `~/workspace/intcols-logs/impl/`:

- chain16: exit 1, `impl: ULTRA STA: timing: no driver for net din`
  (synth ok, 15 LUTs, 16 flops; pack ok, 2 slices including
  `ff=pipe_0_reg d_net=din`; place ok, violations=0).
- shift32: exit 1, `impl: ULTRA STA: timing: no driver for net din`
  (synth ok, 0 LUTs, 32 flops; pack ok, 4 slices including
  `ff=sr_0_reg d_net=din`; place ok, violations=0).

Neither design reaches route or bitstream emit; no `.bit` or `.fasm`
file was produced for either one. The error site, read only, is
`crates/helion-ultrascale/src/sta.rs:174`, the `ok_or_else` on
`drivers.get(net)` in `arrival()`. Baseline suites match the N48 base
exactly: ultrascale 456/0/1, CLI release 89/0/1, US-09 checkers 100 OK
(122.900s, system iverilog 12.0 present and used). One baseline suite
attempt was killed by a runtime restart mid-run and was re-run in full
from scratch.

## 3. Root cause

`sta.rs:174` (`arrival()`): a net with no entry in
`nl.net_drivers()` fails loud. In `crates/helion-cli/src/ultra.rs`
(`sta_on_placed`), `drivers` is built only from cell outputs (LUT `O`,
FF `Q`, carry `O`/`CO`). Top-level input ports have no cell driver,
so their nets are absent from `drivers`.

Why `din` and not `clk`: both are top-level input ports, but only
`din` reaches `arrival()`. STA traverses FF D-pin cones only; `clk`
feeds FF CLK pins, which are never traversed. `din` reaches
`arrival()` directly as an FF D net: chain16 has `pipe[0] <= din`
and shift32 has `sr[0] <= din`, so the first-stage FF's D net is the
port net itself, with no LUT in between.

Why the existing guard did not save it: `sta_on_placed` already drops
undriven nets from LUT/carry `fanin` lists (so a LUT input fed by a
port or a constant never reaches `arrival()`), but that pruning never
touches the FF D net passed to `arrival()` at the top of `sta_routed`.
The gate is exactly the unpruned D-net path.

The netlist does expose the ports: `helion_ir::Design.ports`
(`Vec<Port>`, `PortDir::In`/`Out`/`Inout`), and a port name is the
net it drives (the same convention `fraig.rs` uses for its leaves).
So the fix can identify input-port nets from the design itself; no
port is lost and no new data source is needed.

## 4. Fix

Three files, +229/-8 (impl commit `5b14e11a`).

### `crates/helion-ultrascale/src/sta.rs` (+25/-1)

- New `StaNetlist::input_ports()` method, default empty set: nets
  driven by top-level input ports. Documented as timing startpoints
  arriving at time 0 at the port.
- `arrival()` checks `input_ports` before the driver lookup and
  returns `(0, <port name>)` for a port net. The missing-driver error
  below it is unchanged, so an undriven net that is NOT an input port
  still fails loud.
- One-character tie change in the LUT fanin max: `>` to `>=`, so a
  cone fed only by input ports still names its startpoint instead of
  leaving it empty (a port arrives at 0, which ties the initial worst
  of 0). No existing test pins tie-breaking; all pre-existing STA
  tests still pass with identical numbers.

### `crates/helion-cli/src/ultra.rs` (+35/-7)

- `UltraStaNetlist` gains an `inputs: HashSet<String>` field and
  implements `input_ports()`.
- `sta_on_placed` builds the set from `design.ports` with dir `In`
  or `Inout`, intersected with real net names (so a port with no net
  never claims a name). `clk` needs no special case: it never reaches
  `arrival()`.
- The fanin retain now keeps input-port nets
  (`drivers.contains_key(n) || input_ports.contains(n)`) and drops
  only constants and other undriven nets. The stale comment claiming
  port nets "are not needed here" is removed.

### Regression tests (`crates/helion-ultrascale/tests/sta_sanity.rs`, +169, 4 new tests)

- `sta_input_port_feeding_ff_directly_is_startpoint`: single FF, D
  is `din`, port declared. Passes: endpoints 1, worst delay 0,
  startpoint `din`, WNS 9920.
- `sta_undriven_internal_net_still_fails_loud`: same netlist, port
  NOT declared. Fails loud with verbatim
  `timing: no driver for net din`.
- `sta_chain_head_with_port_input`: the chain16 head shape (ff0 D is
  `din`, ff1 D via a LUT on ff0 Q and `din`). Passes: endpoints 2,
  worst delay 810, startpoint `ff0`, WNS 9110.
- `sta_lut_fed_only_by_port_names_port_startpoint`: LUT fed only by
  `din`. Passes: worst delay 480, startpoint `din`, WNS 9440.

Fail-before proof (log `test-sta-nobranch.log`): with only the port
branch in `arrival()` temporarily removed, the three positive tests
fail with verbatim `timing: no driver for net din` and the negative
test still passes (8 passed, 3 failed). With the branch restored
(`test-sta-fix-rerun.log`): 11 passed, 0 failed.

### Model notes

- The preliminary timing DB is a ballpark (120 ps LUT + 180 ps INT
  hop, never signoff); an input arrival of 0 at the port is the model
  used, with no wire delay charged (the port has no placed site). The
  report stays labeled `estimate=1`.
- STA is not weakened: the check is intact, the error text is
  unchanged, and the negative test above pins the loud failure for a
  non-port undriven net.

## 5. Acceptance (coordinator, merged branch binary)

Merged-branch build: `cargo build --release -j1 -p helion-cli
-p helion-ultrascale`, exit 0, log `coord/build-release.log`;
`helion` 12,931,096 bytes (16 bytes smaller than the impl fix build's
12,931,112, the known N54 worktree effect). Script
`coord/run-coord-flows.sh`, summary `coord/coord-flow-summary.txt`.
Flow env: `HELION_TILECONN_DB_ROOT=$HOME/workspace/prjuray-db-ref/zynqusp`,
`HELION_TILECONN_PART=xczu3eg-sbva484-1-e`.

- chain16 run 1: exit 0, `hang_diag timing WNS_PS=8570 TNS_PS=0
  endpoints=16 r2r_ps=1350 iob_ps=0 ms=0 estimate=1`, 341 PIPs,
  bit md5 `e7cc453ad5e63298a374540be9bf2737`.
- chain16 run 2: exit 0, identical WNS, 341 PIPs, bit md5
  `e7cc453ad5e63298a374540be9bf2737` (byte-identical to run 1, to the
  impl fix run, and to the stash restore run).
- shift32 run 1: exit 0, `hang_diag timing WNS_PS=8690 TNS_PS=0
  endpoints=32 r2r_ps=1230 iob_ps=0 ms=0 estimate=1`, 280 PIPs,
  bit md5 `e7f0e75ceb4c57e6415d15abad23a9ad`.
- shift32 run 2: exit 0, identical WNS, 280 PIPs, bit md5
  `e7f0e75ceb4c57e6415d15abad23a9ad` (byte-identical to run 1, to the
  impl fix run, and to the stash restore run).

Both clear the STA stage that failed them and flow end to end to
bitgen with parse-back OK. Note on "a driver shown for din": the
flow prints only the `hang_diag timing` line, which carries no
per-net driver field, so `din` as a startpoint is shown at the
unit-test level (the test above asserts startpoint `din`, worst
delay 0) and in the flow by the STA pass itself with endpoints 16 /
32. No flow output names `din` as a driver; that is a reporting
limit of the hang_diag line, not a gap in the fix.

## 6. Verify-bit attempt for chain16 (not achieved; new gap N58)

W-IMPL attempted end-to-end verify-bit for chain16 with the
branch-built decoder (`US09V_DECODER_BIN=.../target/release/helion-bitgen`),
absolute `--workdir`, `--top chain16`, 400 cycles, fixture RTL
`chain16-rtl.v` (pipe init 0, mirroring the flow FF INIT.V0).

- Probe run (log `verify-chain16-probe.log`, map clk only, dout
  guessed AQ): decode 530 FASM lines exit 0, parse OK, ports OK,
  place OK, route FAIL (undriven sink `SLICE_X4Y0.F1` on net
  `{CLEM_X3Y0.CLE_CLE_M_SITE_0_F1, INT_X3Y0.IMUX_W3}`), extract
  FAIL (`SLICE_X4Y0.F1 (FLUT input) is undriven`). This names the
  din fabric entry: `SLICE_X4Y0.F1`.
- With din mapped (`chain16-port-map-din.json`: clk
  `SLICE_X1Y0.CLK1`, din `SLICE_X4Y0.F1`, dout `SLICE_X1Y0.AQ`;
  log `verify-chain16-din.log`): decode, parse, ports, place,
  route (347 PIPs, 48 nets, no violations), extract (5 sites, no
  errors) all OK; cosim FAILS, 800 comparisons, 403 mismatches,
  first mismatch cycle 19.
- Dout sweep (script `run-sweep-chain16-dout.sh`, summary
  `sweep-chain16-dout.txt`): all 16 Q pins (`SLICE_X1Y0` /
  `SLICE_X4Y0` x AQ..HQ) fail cosim. 13 pins give 349-432
  mismatches (wrong bit); `SLICE_X4Y0.FQ/GQ/HQ` give 800
  mismatches with net=x everywhere (no FF modeled at those pins).

Diagnosis from the extracted model
(`verify-c16-X1Y0-AQ/extracted.v`): the FASM is correct,
`SLICE_X4Y0.FFMUXE1.BYP`, so pipe_0's FF (letter E, repaired to
`SLICE_X0Y1`) should take D=din from the fabric. But the extractor
models `s_SLICE_X4Y0__EQ <= s_SLICE_X1Y0__H_O` (the H LUT output)
instead of din, and models `X4Y0.CQ` and `X4Y0.EQ` with the same D
(`X1Y0.H_O`), so the extracted pipe cannot be the RTL pipe and no
dout pin can pass. This is an extractor BYP-resolution gap (or a
router fanout issue for the din net to the BYP pin), not an STA
issue, and fixing it is a separate lane. Shift32 was not attempted:
same shape (`sr[0] <= din` via BYP), expected to hit the same gap,
unproven.

## 7. Suites (coordinator, merged branch, serial, -j1)

| suite | result | log |
| --- | --- | --- |
| helion-ultrascale (debug) | exit 0, 460 passed / 0 failed / 1 ignored, 35 test-result lines | `coord/suite-ultrascale.log` |
| helion-cli (release) | exit 0, 89 passed / 0 failed / 1 ignored, 11 test-result lines | `coord/suite-cli-release.log` |
| US-09 checkers | exit 0, Ran 100 tests, OK, 151.993s, oss-cad-suite iverilog 14.0 | `coord/checkers-us09.log` |

Ultrascale is base 456/0/1 plus exactly the 4 new STA tests
(460/0/1); all 4 pass by name. CLI is identical to base 89/0/1.
The checkers first attempt was killed by SIGTERM mid-run (the
suite script died after ultrascale exit 0 and cli-release exit 0,
checkers log only 94 bytes of dots; the same runtime-restart class
that hit both workers this lane) and was re-run alone with nothing
else running: 100 OK in 151.993s. The impl worker's own suite runs
from its branch-built binary agree exactly (460/0/1, 89/0/1, 100 OK
in 115.728s).

## 8. Regressions (coordinator, merged branch)

Flows script `coord/run-coord-flows.sh`; verify-bits script
`coord/run-coord-verify.sh` (branch-built decoder, oss-cad-suite
iverilog 14.0 on PATH), summary `coord/coord-verify-summary.txt`.

| design | flow | verify-bit |
| --- | --- | --- |
| counter12 seed 0 | exit 0, 804 PIPs | PASS, 400 cycles, 800/0 |
| counter8 seed 0 | exit 0, 377 PIPs | PASS, 400 cycles, 800/0 |
| counter8 seed 1 | exit 0, 339 PIPs | PASS, 400 cycles, 800/0 |
| counters2x4 | exit 0, 320 PIPs | PASS, 400 cycles, 800/0 |
| counter5 | exit 0, 76 PIPs | PASS, 400 cycles, 800/0 |
| counter | exit 0, 58 PIPs | PASS, 400 cycles, 800/0 |
| blinky | exit 0, 4 PIPs | PASS, 400 cycles, 800/0 |
| counter16 seed 1 | exit 0, WNS 5750, 1244 PIPs | PASS, 40000 cycles, 80000/0 |

Every flow number matches the N48 fix lane exactly (same PIP
counts). Blinky `.bit` md5 `d82b70c8e29ce72f28e08c94f3a16d16`,
unchanged. All 8 regression verify-bits pass. The impl worker's
own regression runs agree exactly (counter12-s0 5/5 at 20 iters /
804 PIPs, same verify-bit results).

Two coordinator script errors, recorded honestly: the blinky and
counter flows first exited 1 because the script pointed at
`intcols-logs/impl/blinky.sv` and `intcols-logs/impl/counter.sv`,
which do not exist (blinky.sv and counter.sv live in the worktree
`examples/`; counter12.sv lives in `critic42-runs/`). Re-run from
the correct paths, both exit 0. No flow result above comes from a
wrong-path run.

## 9. Stash proof (both sides)

`fix.patch` is the diff of the impl branch against base `ac4d6f68`
(340 lines, sha256
`2bebcd0efadc7ad3ccaf20cf0713db9bc590c6a346ada61e03ff8baa4a25ea77`).
The coordinator verified it is byte-identical to the branch diff
`ac4d6f68..5b14e11a` (cmp, no output; 3 files, +229/-8). Procedure
(`git stash push` the three fix files, rebuild release, run both
flows; `git stash pop`, rebuild release, run both flows):

- Base side (logs `flow-*-stashbase.log`, summary
  `stash-proof-fail.log`): chain16 exit 1, shift32 exit 1, both
  verbatim `impl: ULTRA STA: timing: no driver for net din`.
- Fix side (logs `flow-*-stashfix.log`, summary
  `stash-proof-pass.log`): chain16 exit 0 (WNS 8570, endpoints 16),
  shift32 exit 0 (WNS 8690, endpoints 32); `.bit` md5s identical
  to the first fix run and to the coordinator's merged-branch runs.
- The patch was generated from the restored (post-pop) tree;
  `git diff --stat` after the pop matches the patch exactly.

## 10. Files changed

| file | + | - |
| --- | --- | --- |
| `crates/helion-ultrascale/src/sta.rs` | 25 | 1 |
| `crates/helion-cli/src/ultra.rs` | 35 | 7 |
| `crates/helion-ultrascale/tests/sta_sanity.rs` | 169 | 0 |
| total (`git diff --numstat`) | 229 | 8 |

## 11. New nits

- N58 (blocking for chain16/shift32 verify-bit, not for the STA
  gate): the US-09 extractor mis-resolves a BYP-fed FF D when the
  driver is a top-level input port. Evidence: chain16 FASM carries
  `SLICE_X4Y0.FFMUXE1.BYP`, yet the extracted model drives pipe_0's
  FF D from `X1Y0.H_O` and gives two FFs the same D, so no dout pin
  can pass cosim (16 of 16 pins fail; 13 wrong-bit, 3 net=x). The
  din fabric entry is settled (`SLICE_X4Y0.F1`, from the probe run's
  undriven-sink error). Whether the fault is in extract.py's
  BYP resolution or in the router's din fanout to the BYP pin is
  unproven; the next lane must decide by inspection before fixing.
  Shift32 verify-bit is expected to hit the same gap, unproven.

## 12. Carried, not fixed here

- N58 (new, section 11): chain16/shift32 verify-bit blocked on the
  extractor BYP-resolution gap.
- N51 (stale doc sentence at ultra.rs 1230-1232; Session 44 ruled
  it NOT fixed). N53 (stale packer comment at ultra.rs 1638-1648).
  Both are doc-only, untouched by this lane.
- N57 (doc only: N48 report header still says "report commit
  pending" though ac4d6f68 exists). This lane's report header states
  its report commit explicitly; see section 13.
- N49 (system iverilog 12.0 present for the baseline checkers run,
  wiped by a service restart mid-lane; oss-cad-suite 14.0 used for
  the impl and coordinator checkers). N52 pre-existing E0133
  warnings (2 occurrences in the CLI suite log, unchanged).
- N50 class (see N54): the coordinator's merged-branch helion is
  16 bytes smaller than the impl fix build from the same command
  (12,931,096 vs 12,931,112). N54 (binary bytes depend on the
  worktree, not just the build command). N55 (N48 fix proven on
  the measured design set only; larger netlists unmeasured). N56
  (pre-existing rustfmt deviations; `ultra.rs` still 21, unchanged;
  `sta.rs` clean; `sta_sanity.rs` has only the pre-existing line-1
  import-order deviation, also present on base).
- STA din gate for top-level inputs (chain16, shift32) is CLOSED
  by this lane, subject to the critic's ruling on the verify-bit
  limit in section 1. IOB placement still blocked on B1. INT_INTF
  tile model for DSP/BRAM routing still open. Known-INIT Vivado
  datum still open. Documented xczu3eg timing still open.
  Remaining 54,907 non-band unexplained bits still open. N16
  parse-side open. N9 block types 2+ untested. N34 stale default
  decoder (every verify-bit run here used the branch-built decoder
  via the env override). N37. N38. Flags A-CTRL and A-F7F8 still
  unresolved, need a CTO ruling. No new nits from this lane beyond
  N58.

## 13. Evidence index

- `~/workspace/stadin-logs/baseline/BASELINE_REPORT.md`,
  `build-release-scoped.log`, `flow-chain16.log`,
  `flow-shift32.log`, `suite-ultrascale.log`,
  `suite-cli-release.log`, `checkers-us09.log`
  (plus `build-release.log` partial and
  `suite-ultrascale-partial.log`, both interrupted by a runtime
  restart and re-run in full).
- `~/workspace/stadin-logs/impl/IMPL_REPORT.md`, `fix.patch`
  (sha256 above), `flow-chain16-fixrun1.log`,
  `flow-shift32-fixrun1.log`, `flow-*-stashbase.log`,
  `flow-*-stashfix.log`, `stash-proof-fail.log`,
  `stash-proof-pass.log`, `test-sta-nobranch.log`,
  `test-sta-fix-rerun.log`, `suite-ultrascale.log`,
  `suite-cli-release.log`, `checkers-us09.log`,
  `fix-reg-summary.txt`, `verify-reg-summary.txt`,
  `verify-chain16-probe.log`, `verify-chain16-din.log`,
  `run-sweep-chain16-dout.sh`, `sweep-chain16-dout.txt`,
  `chain16-rtl.v`, `chain16-port-map-din.json`.
- `~/workspace/stadin-logs/coord/build-release.log`,
  `run-coord-flows.sh`, `coord-flow-summary.txt`,
  `flow-coord-*.log`, `coord-*.bit/.fasm`,
  `run-coord-verify.sh`, `coord-verify-summary.txt`,
  `verify-coord-*.log`, `run-coord-suites.sh`,
  `coord-suite-summary.txt`, `suite-ultrascale.log`,
  `suite-cli-release.log`, `checkers-us09.log`.
