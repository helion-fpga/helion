# Counter ROUTE OveruseTimeout lane report

Lane branch: fm/ultra-bitgen-counter, code tip 69000bb0 (merge of W-IMPL 28aa4c70 on BYP-leg tip 90f62321). The lane tip is the report commit itself (this file's commit on the lane branch). No merges into fm/ultra-bitstream (still 07d0d7d), fm/zedboard-merge, or master. No critic dispatched by this lane; Session 38 is the main agent's to dispatch.

## 1. Outcome

The 4-bit counter now flows end to end on xczu3eg and verifies. The counter flow exits 0 in 5 of 5 lane runs plus the finisher verification run (6 of 6 total), route 6 nets with 0 overuse, 51 to 56 PIPs, and emits a 5,982,532-byte bitstream. US-09 verify-bit PASSES on fixture RTL: decode exit 0, route 55 PIPs / 10 nets with no violations, extract ok, cosim 400 comparisons, 0 mismatches. Raw RTL fails on cosim only (400 RTL unknowns, the known pre-existing uninitialized-register artifact). Flow blinky still PASSES on fixture RTL. Suites: helion-ultrascale 436 passed / 0 failed / 1 ignored, helion-cli 73/0/1, US-09 checkers 93 OK.

The lane's acceptance is met. The flow failed at ROUTE on the base in every run; the work was three combined faults in the router, the packer, and the route-to-PIP translator, not a single bug.

## 2. The failure (baseline, W-BASELINE, unmodified base 90f62321)

Design: `<worktree>/examples/counter.sv`, a 4-bit incrementer (module counter, clk in, led = cnt[3] out). Flow command in every run:

```
<worktree>/target/release/helion impl <worktree>/examples/counter.sv --part xczu3eg
```

With the tileconn map, env `HELION_TILECONN_DB_ROOT=~/workspace/prjuray-db-ref/zynqusp` and `HELION_TILECONN_PART=xczu3eg-sbva484-1-e`. Suites on the base, recounted from logs: helion-ultrascale 432/0/1; helion-cli 72/0/1 with the tileconn env unset; US-09 checkers 93 tests OK.

Synth, pack, place, legality, and STA pass in every run (4 LUTs, 4 flops, 1 slice, WNS_PS=9650, estimate=1). Every run fails at ROUTE with OveruseTimeout, exit 1, and emits no `.bit`.

| Run | Exit | Failing net | Wire | Tile | Shared by | Iters |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 1 | cnt_1 | 36 | (1,0) | 3 nets | 50 |
| 2 | 1 | cnt_1 | 36 | (1,0) | 3 nets | 50 |
| 3 | 1 | cnt_1 | 36 | (1,0) | 3 nets | 50 |
| 4 | 1 | cnt_0_d | 34 | (1,0) | 2 nets | 50 |
| 5 | 1 | cnt_1 | 36 | (1,0) | 3 nets | 50 |

Failure line verbatim (runs 1, 2, 3, 5):

```
impl: ULTRA ROUTE: route failed for net 'cnt_1' at wire 36 at tile (1,0) shared by 3 nets after 50 iters: OveruseTimeout
```

Run 4 (the variant):

```
impl: ULTRA ROUTE: route failed for net 'cnt_0_d' at wire 34 at tile (1,0) shared by 2 nets after 50 iters: OveruseTimeout
```

Acceptance pins the failure class, not the net: the failing net varies run to run, the tile (1,0) and the 50-iter limit do not.

Without the tileconn map the flow fails earlier and louder (log flow-counter-notileconn.log):

```
impl: ULTRA ROUTE: route failed for net 'cnt_3' at driver pin 1.AQ.Q has no wire: NoPath
```

Blinky control on the base passes (exit 0, 4 PIPs, .bit md5 d82b70c8e29ce72f28e08c94f3a16d16), so the base is good and the failure is counter-specific. Skip lines in every counter run: net=cnt_3 (IOB, no placed site), net=led (IOB), net=cnt_3_d (intra-slice LUT-O to same-letter FF-D, D6).

## 3. The three faults and the fixes (W-IMPL, commit 28aa4c70)

Commit 28aa4c70 "counter: fix ROUTE OveruseTimeout on xczu3eg (router own-trunk cost, X-pin pack repair, multi-sink PIP translator)", 4 files, +760/-35 (crates/helion-cli/src/ultra.rs +380, crates/helion-ultrascale/src/route_to_pip.rs +167, crates/helion-ultrascale/src/router.rs +100, crates/helion-ultrascale/src/router/tests.rs +148). Pushed to origin on fm/ultra-bitgen-counter-impl.

- F1 router (`router.rs`): A* charged a net presence cost for reusing its own trunk wires, so cnt_1's W13 leg would not share its W12 leg's trunk and stayed on BYPASS_W6. Fix: `astar` takes an own-use mask; cost is `1 + hist + (pres - own) * pres_fac`, so sharing a net's own trunk costs no presence penalty. Debug trace behind `HELION_ROUTE_DEBUG`.
- F2 packer (`ultra.rs`): two FFs in slice letter B shared the one BX pin wire (BYPASS_W1) with different BYP D nets, which is unconfigurable; the packer assigned FF BELs with no D-net knowledge. Fix: `repair_ff_xpin_conflicts` moves the second BYP FF to a free letter of the same slice, preferring its driver LUT's letter (the move then selects D6 and needs no fabric route at all). It runs after pack in `run_impl` and in the multiseed pack-once site. When no letter can take the FF, it fails loud naming the FF, its D net, and the slice. On the counter it moves cnt_2_reg from B to D (log line `xpin_repair ff=cnt_2_reg d_net=cnt_2_d slice=SLICE_X0Y0 letter B->D`), so cnt_2 takes D6 from the D LUT.
- F3 translator (`route_to_pip.rs`): multi-sink route unions failed translation at leg junctions (NotConnected), which blocked the counter once it routed. Fix: `translate_route_with` derives the PIP tree from the wire set (each wire's latest preceding driver), so flattened multi-sink unions translate; genuinely disconnected unions still fail loud with NotConnected.

Ablation table (flow logs in `~/workspace/counter-logs/impl/`):

| Config | Result |
| --- | --- |
| F1 only | OveruseTimeout on cnt_1, wire 36, tile (1,0), 3 nets, 50 iters (log flow-counter-f1only.log) |
| F2 only | OveruseTimeout on cnt_1, wire 36, tile (1,0), 3 nets, 50 iters (log flow-counter-f2only.log) |
| F1+F2 | Routes clean (6 nets, iters 4, overused 0) but fails at route-to-pip: `route in design 'cnt_0' jumps from wire 'IMUX_W8' to 'INT_NODE_IMUX_50_INT_OUT0' with no edge between them` (log flow-counter-f1f2.log) |
| F1+F2+F3 | Emits a bitstream: 56 PIPs, 5,982,532 bytes, parse-back 568,024 lines (log flow-counter-f1f2f3.log) |

No single fix suffices; all three are necessary. Each fix ships with a regression test: 2 new router tests (`counter_repaired_netlist_routes`, `counter_unrepaired_netlist_still_fails_loud`), 2 new translator tests (`multi_sink_union_translates_to_pip_tree`, `union_with_unreachable_wire_still_fails_loud`), and 1 new CLI flow test (`counter_flow_emits_bitstream_on_xczu3eg`, which pins the 22 stable FASM lines, the 0x04a42093 IDCODE, 14,952 FDRI frames, and the H-1 band check). A control test (`counter_multi_sink_net_shares_its_own_trunk`) passed on both base and fix and was dropped before commit; it discriminates nothing.

Results on the fix, recounted from logs: helion-ultrascale 436/0/1 (432 + 4 new); helion-cli 73/0/1 (72 + 1 new, tileconn env unset); US-09 checkers 93 tests OK (log test-checkers-rerun.log, after the iverilog reinstall; see section 6). Counter flow with the tileconn map exits 0 in 5 of 5 lane runs: 51 to 56 PIPs, iters 2 to 4, overused 0, 5,982,532 bytes in every run. Without the map it still fails loud with NoPath on cnt_3. Blinky control still exits 0 with md5 d82b70c8.

## 4. Validation (stash proof, both sides)

Fix stashed (new tests kept, old code): every discriminating test fails on the base.

- Router: `counter_repaired_netlist_routes` FAILED with `RouteError { net: "cnt_1", location: "wire 36 at tile (1,0) shared by 3 nets after 50 iters", kind: OveruseTimeout }` (log stash-proof/base-router-tests.log). The guard `counter_unrepaired_netlist_still_fails_loud` passes on base, as it must.
- Translator: `multi_sink_union_translates_to_pip_tree` FAILED with `NotConnected { src: "IMUX_W8", dst: "INT_NODE_IMUX_49_INT_OUT0", design: "counter" }` (log stash-proof/base-translator-tree.log).
- Translator negative: `union_with_unreachable_wire_still_fails_loud` FAILED on base with the wrong failure wire: left `NotConnected { src: "BOUNCE_E_0_FT1", dst: "GCLK_B_0_0", design: "blinky" }`, right `NotConnected { src: "LOGIC_OUTS_E0", dst: "GCLK_B_0_0", design: "blinky" }` (log stash-proof/base-translator-neg.log). The base translator reports the disconnect at the wrong wire; the fix reports it at the expected one.
- CLI flow: `counter_flow_emits_bitstream_on_xczu3eg` FAILED on base with the OveruseTimeout on cnt_1 (log stash-proof/base-cli-flow.log, 2.15s).
- Control: the redesigned testA passes on base (log stash-proof/base-testA-redesigned.log, 1 passed), as it does on the fix.

Fix restored: router 2/2 ok, translator 1/1 ok, translator negative 1/1 ok, CLI flow ok in 8.02s (logs test-router-restored.log, test-translator-restored.log, test-translator-neg-restored.log, test-cli-counter-flow-restored.log). The restored worktree diff matches fix.patch exactly in ultra.rs, route_to_pip.rs, and router.rs; only router/tests.rs differs, by the redesigned testA.

## 5. Finisher verify-bit (decoder from this branch, absolute workdir)

The finisher rebuilt the release from the committed tip (log build-release-finisher.log, 3.04s, no recompilation: the committed tree is exactly what was built) and re-ran the counter flow fresh (log flow-counter-finisher.log, exit 0, 6 nets, iters 3, overused 0, 55 PIPs, .bit md5 2f4deb3587912d67aecfd88b6e921b9d, 5,982,532 bytes). The FASM has 77 lines; all 22 stable lines are present (4 LUT INITs, 8 FF INIT/SRVAL, 4 FFMUX, 6 FF control). The .bit md5 differs from the lane runs (e8c8880e on run 5) because the router is nondeterministic across processes; acceptance pins verify outcomes, not md5 (nit N35).

Port map derivation for the counter, checked two ways. The A LUT INIT 0x6aaa6aaa6aaa6aaa decodes to cnt[3] ^ (cnt[2] & cnt[1] & cnt[0]), so the A LUT drives cnt_3_d; FFMUXA1.D6 means AFF takes D6 from the A LUT; the two D6-skipped D nets are cnt_2_d (DFF, per the repair log) and cnt_3_d, so AFF is cnt_3_reg and led is SLICE_X1Y0.AQ. Cross-check: the D LUT 0x7878 decodes to cnt[2] ^ (cnt[1] & cnt[0]) = cnt_2_d with DFF taking D6; the B LUT 0x5555 is ~cnt[0] = cnt_0_d and the C LUT 0x6666 is cnt[1] ^ cnt[0] = cnt_1_d, both routing through the fabric on BYP, matching the routed cnt_0_d and cnt_1_d nets. Port map: `{"clk": "SLICE_X1Y0.CLK1", "led": "SLICE_X1Y0.AQ"}`. Fixture RTL is the counter design with `cnt` init 0 (file counter-rtl.v in verify-counter/).

Verify-bit, fixture RTL (log verify-flow-counter-fixture.log, exit 0), decoder built from this branch and pointed at with US09V_DECODER_BIN (nit N34), absolute --workdir:

```
counter: PASS
  [ok] decode: 138 FASM lines, exit 0
  [ok] parse: 2 tiles, 138 features
  [ok] ports: 1 inputs, 1 outputs from port map
  [ok] place: 1 slices, 0 carry slices; no violations
  [ok] route: 55 PIPs, 10 nets; no violations
  [ok] extract: 1 sites modelled; no errors; 18 warnings
  [ok] cosim: 400 comparisons, 0 mismatches, 0 RTL unknowns, 0 net unknowns; no mismatches
```

Verify-bit, raw RTL (log verify-flow-counter-raw.log, exit 1), same .bit, raw design RTL examples/counter.sv instead of the fixture RTL:

```
counter: FAIL
  [ok] decode: 138 FASM lines, exit 0
  [ok] parse: 2 tiles, 138 features
  [ok] ports: 1 inputs, 1 outputs from port map
  [ok] place: 1 slices, 0 carry slices; no violations
  [ok] route: 55 PIPs, 10 nets; no violations
  [ok] extract: 1 sites modelled; no errors; 18 warnings
  [FAIL] cosim: 0 comparisons, 0 mismatches, 400 RTL unknowns, 0 net unknowns; no mismatches
```

Raw RTL leaves cnt uninitialized (no init 0), so every RTL comparison is unknown. Same pre-existing artifact as byp2 raw RTL (800 unknowns there). Not a lane defect; the fixture RTL PASS stands.

Flow blinky on the fix (log flow-blinky-verify.log, exit 0, 4 PIPs, .bit md5 d82b70c8e29ce72f28e08c94f3a16d16, byte-identical to the BYP-leg lane value). Verify-bit, fixture RTL (log verify-flow-blinky-fixture.log, exit 0): blinky PASS, decode 81 FASM lines exit 0, route 4 PIPs / 4 nets, extract ok, cosim 400/0. The finisher's first blinky verify attempt failed on place, route, and extract because it used the hand-fixture port map (SLICE_X10Y10) on the flow bitstream (SLICE_X1Y0); the rerun with the flow port map passes. That was a harness invocation error, not a lane defect, recorded here.

## 6. Incidents, all recorded

- iverilog was missing from the VM after a restart (US-09 checkers first run on the fix: 78 pass, 15 errors, all FileNotFoundError: iverilog; also recorded in the commit message). Icarus Verilog 12.0 reinstalled via apt; rerun 93 tests OK, exit 0. The finisher's own reinstall attempt hit the apt lock (a platform `apt-get update -qq` held it); the lane coordinator handled the reinstall after a nudge from the main agent.
- Worktree state discrepancy at 18:31 EST: the impl diff shrank from +789/-35 across 4 files to +393/-0 across 3 files, router.rs clean at HEAD while fix.patch still included it. Resolved by restoring from fix.patch; the restored diff matches fix.patch exactly in ultra.rs, route_to_pip.rs, and router.rs.
- One CLI flow restored log showed 0 tests run (all filtered) in one invocation; the CLI flow pass is not claimed from that log. It was re-run unfiltered: 1 passed, 8.02s.
- Nit N34 (Critic Session 36): verify.py defaults US09V_DECODER_BIN to a stale pre-N7 decoder binary that fails decode with exactly 41 band bits. Every verify-bit PASS in this lane used a decoder built from the lane branch via the env override. The default is still not fixed; it is a harness default defect, not a lane defect.
- Verify-bit needs an absolute --workdir; the relative form fails at cosim (cosim.py runs vvp with cwd=workdir on a relative path). Every PASS above used an absolute workdir.

## 7. Next blockers, in order

1. IOB placement (4 nets skipped on the counter per flow run: cnt_3 and led for no placed IOB site, plus cnt_3_d and cnt_2_d as D6 intra-slice skips).
2. NoPath without the tileconn map (cnt_3, driver pin 1.AQ.Q has no wire).
3. `set_int_columns` for INT_X2+ (unproven; the counter routes through INT_X1 only).
4. The missing known-INIT Vivado datum (P-02 blocked on the missing xcku3p DB).
5. Documented xczu3eg timing.
6. The remaining 54,907 non-band unexplained corpus bits.
7. Router nondeterminism (PIPs vary 51 to 56 run to run, .bit md5 varies; acceptance pins verify outcomes, not md5). Nit N35.

Flags A-CTRL and A-F7F8 still unresolved, need a CTO ruling; this lane touches neither and closes neither. N16 parse-side stays open. N9 block types 2+ untested. N34 (stale default decoder in verify.py) stays open.

## Evidence

- Lane report: ~/workspace/ultra-bitgen-crew/COUNTER_REPORT.md (this file; copy committed at tools/counter/COUNTER_REPORT.md on the lane branch).
- Logs: ~/workspace/counter-logs/baseline/ (BASELINE_REPORT.md and all flow/suite logs), ~/workspace/counter-logs/impl/ (ablation, stash-proof, suite, flow, and verify-bit logs).
- Branches: lane fm/ultra-bitgen-counter at 69000bb0 (code tip; lane tip is the report commit on top); impl fm/ultra-bitgen-counter-impl at 28aa4c70 (pushed); baseline fm/ultra-bitgen-counter-baseline at 90f62321 (no commits, local-only).
- Design: `<worktree>/examples/counter.sv` (also printed in full in the baseline report). Fixture RTL and port map: ~/workspace/counter-logs/impl/verify-counter/counter-rtl.v and counter-port-map.json.
