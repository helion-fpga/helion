# Router/packer lane report

Lane branch: fm/ultra-bitgen-routepack, code tip 301d2225 (impl commit
on intcols base 1a095675). No merges into fm/ultra-bitstream,
fm/zedboard-merge, or master. No critic dispatched by this lane;
Session 41 is the main agent's to dispatch.

## 1. Outcome

A design wider than the 4-bit counter now routes through INT_X2+ and
verifies end to end. counter5 passes verify-bit on fixture RTL (decode,
route, cosim 800 comparisons, 0 mismatches) on two independent runs.
counter8 and counters2x4 flow through INT_X2Y0..INT_X4Y1 with PIPs
landing at table FARs. counter8 does NOT pass verify-bit (route checker
failures, section 6). counter16 still times out in ROUTE;
chain16/shift32 still stop at the STA din gate. Those are named gaps,
not hidden failures. This lane meets its core acceptance (one wider
design verifying end to end) with honest gaps carried.

Suites: helion-ultrascale 448 passed / 0 failed / 1 ignored (441 + 7
new), helion-cli release 78/0/1 (76 + 2 new), US-09 checkers 95 OK.
Stash proof both sides. Blinky .bit md5 unchanged.

## 2. Baseline: the failure, measured

Baseline report: ~/workspace/routepack-logs/baseline/BASELINE_REPORT.md.
On the base tip, every flow log showed `wiredb tiles=1 wires=1475`
and no FASM used any INT tile other than INT_X1Y0. counter5 (5 seeds)
and counter8 (5 seeds): ROUTE OveruseTimeout, tile (1,0), 50 iters.
counter5 seed 0 matched Session 40 verbatim (net cnt_0, wire 463,
shared by 2); counter8 seed 0 matched the intcols lane (wire 36,
shared by 4). counter16/chain16: PACK fail, X-pin repair, no free
compatible letter. shift32: PACK fail, no slice with compatible
control set. counters2x4: ROUTE OveruseTimeout. tiny2x2: exit 0, one
slice, all INT_X1Y0. Baseline suites 441/0/1, 76/0/1, 95 OK.

Root causes pinned to code: placer `place_ultra` (place.rs:225-262)
took the first free site in (Y,X) order with no spreading; wire DB
`TileconnWireDb::from_db_and_sites` (tileconn.rs:757-906)
instantiated only INT tiles touching placed slices, and both placed
slices flanked the same tile INT_X1Y0, so the graph had zero
continuation edges. The NoPath wire gap was REFUTED: pin 1.AQ.Q
resolves through tile_type_CLEM.json and tileconn.json to
LOGIC_OUTS_W14, and route-debug shows the counter routing through it.

## 3. The fix (8 files, 797 insertions, 126 deletions)

- Wire DB (`tileconn.rs`, expansion at 897-935): after collecting seed
  INT tiles from placed pins, instantiate every INT tile in the 3x3
  block around each seed tile that exists in the part tilegrid. Wire
  counts: blinky/counter5 6 tiles / 8850 wires; counter8 10 tiles /
  14750 wires (baseline: 1 tile / 1475 wires). A row-only experiment
  (dx -2..=2, dy=0) was tried first and failed; the 3x3 block is the
  smallest graph with continuation edges in both axes.
- Placer (`place.rs`, spread at 210-262): a candidate site must now be
  Manhattan distance at least 2 from every already placed slice, else
  fall back to the first free site. Verified against tilegrid/tileconn:
  SLICE_X1Y0 flanks INT_X1Y0, SLICE_X3Y0 flanks INT_X2Y0, SLICE_X4Y0
  flanks INT_X3Y0. counter8 places SLICE_X1Y0 + SLICE_X4Y0.
- Packer (`pack.rs`, FF loop around line 289): when no existing slice
  can take an FF, create a fresh Logic slice instead of erroring.
  shift32 (0 LUTs, 32 FFs) now packs into 4 slices.
- Packer (`ultra.rs`, `repair_ff_xpin_conflicts` around lines
  1115-1230): the target search now covers all slices with a
  control-set check for cross-slice moves, and appends a fresh empty
  slice when no letter anywhere can take the FF. counter16 now PACKs
  (4 slices); chain16 PACKs (2 slices).
- Router (`router.rs`, pres loop lines 240-244, 404-428): every wire a
  net uses now counts toward `pres`, terminals included. Proven by
  route-debug: cnt_1_d sank at BYPASS_W1 through an IMUX_35 PIP while
  cnt_3 passed through the same wire through an IMUX_43 PIP, and the
  old code counted pres=1, overused=0 because it excluded src/sinks.
  Two different PIPs drove one mux input.
- Bitgen (`bitgen_device.rs` `int_tile_bit_shift` line 171;
  `bitgen.rs` emit shift 1871-1895, decode shift and row-start lift
  2486-2560, fast path 1010-1050): INT tiles off Y0 need a bit shift,
  not a FAR shift. The tilegrid gives every INT tile in a 60-row
  clock region a shared column base FAR with `offset` stepping 3 per Y
  and jumping 6 at Y30; the BitFiltrator LutLoc table steps +48 bits
  per Y and +96 at Y30. One tilegrid offset unit is 16 bits, each tile
  occupies a 48-bit segbits window starting at 16 * offset(y), where
  offset(y) = 3*(y%60) + 6 when (y%60) >= 30. The segbits DB lines
  carry the Y0 mapping only. The shift is exact: the highest window
  (Y59, offset 183) ends at bit 47 + 16*183 = 2975, the last bit of
  the 93-word frame. The old row-start skip in `parse_bitstream` is
  lifted for plain INT tiles (kept for INT_INTF/HDIO); a DB-derived
  fast path skips tiles whose window holds no set bit. The first
  hardcoded 48-bit window broke `round_trip_returns_identical_fasm`
  (stub DB uses INT bit 100); the DB-derived bound fixed it.

New tests: 9, all passing. The sink-wire test passes on base too
(pinning test; its fail-on-base proof is the counter5 flow itself).
The shift unit test cannot run on base (calls the new helper); its
proof is the targeted stash in section 7.

## 4. Flow results (release CLI, intcols designs)

| design | exit | PIPs | INT tiles in FASM |
|---|---|---|---|
| counter5 s0-s4 | 0 x5 | 101, 102, 83, 89, 82 | X0Y0, X0Y1, X1Y0, X1Y1 |
| counter8 s0, s1 | 0, 0 | 436, 508 | X0Y0..X4Y1 |
| counter8 s2-s4 | 1 x3 | none | ROUTE OveruseTimeout |
| counter16 | 1 | none | ROUTE OveruseTimeout, net cnt_9 wire 8839 tile (5,0) shared by 2 |
| chain16 | 1 | none | STA: no driver for net din |
| shift32 | 1 | none | STA: no driver for net din |
| counters2x4 | 0 | 418 | X0Y0..X4Y1 |
| tiny2x2 | 0 | 54 | INT_X1Y0 only |
| blinky | 0 | 4 | INT_X1Y0 only, md5 d82b70c8 |
| counter | 0 | 56 (53 on re-run) | INT_X1Y0 only |

INT_X2+ FAR evidence, coordinator-verified from counter8-s0.fasm:
185 PIPs on INT_X2Y0..X4Y1 (16 X2Y0, 14 X2Y1, 77 X3Y0, 28 X3Y1, 22
X4Y0, 28 X4Y1). Example: `INT_X2Y0.PIP.INT_NODE_SDQ_9_INT_OUT1.
EE2_E_END2` lists set bit 45_12, landing at table base 0x700 + frame
45 = FAR 0x72d. Decode of counter8-s0 returns all 436 PIPs with the
identical tile distribution. The intcols flip test (INT_X2Y0 PIP at
FAR 0x711) still passes.

Router nondeterminism (N35/N36): counter5-s0 gave 101 PIPs in
experiment 5 and 95 PIPs on the post-restore re-run from a
byte-identical source tree. Acceptance pins verify outcomes, not .bit
md5; both runs pass verify-bit.

## 5. Verify-bit (decoder built from this branch, N34)

Full reproducible commands in the impl report section 9 (nit N41
closed for this lane). Coordinator-verified from the logs:

- counter5 seed 0: PASS twice. First run: decode 186 FASM lines exit
  0, route 101 PIPs / 15 nets, cosim 800 comparisons, 0 mismatches.
  Post-restore re-run: decode 180 lines, route 95 PIPs / 15 nets,
  cosim 800 comparisons, 0 mismatches. The led pin was determined by
  trial: only AQ passes; AQ2/BQ/CQ/DQ fail cosim with 193-200
  mismatches. Port map: `{"clk": "SLICE_X1Y0.CLK1", "led":
  "SLICE_X1Y0.AQ"}`.
- blinky: PASS, decode 81 lines, route 4 PIPs / 4 nets, cosim 800/0.
  .bit md5 unchanged.
- counter: PASS, decode 136 lines, route 53 PIPs / 10 nets, cosim
  800/0.
- counter8 seed 0 and seed 1: FAIL, route checker (section 6).
  Decode itself is clean (exit 0, 604 lines, all 436 PIPs for seed 0).

Cosim uses fixture RTL with `reg [N:0] cnt = 0` (the lane designs
leave cnt uninitialized, so plain RTL cosim reports 400 RTL unknowns),
exactly as the counter lane did.

## 6. What still fails and why (named next gaps)

- counter8 verify-bit: 280 violations (seed 0), 388 (seed 1).
  Classes: undriven-sink PIPs whose source is a span-wire end
  (WW2/EE2/EE4 _E_END/_W_END), and CLK2 undriven on seed 0
  (EFGH-half FFs present, no PIP drives CLK2; the flow routes the
  clock to CLK1 only). Next gap: router span-continuation fidelity
  plus clock-net sink modelling for CLK2. Coordinator note: the 280
  and 388 counts are the worker's numbers from the impl report
  section 6; no separate counter8 verify log was saved in the impl
  logs, so the coordinator did not independently recount them. The
  FAIL itself is corroborated by the decode files and the documented
  example PIPs; the PASS claims in section 5 are all verified from
  logs.
- counter8 seeds 2-4 and counter16: ROUTE OveruseTimeout, 2 nets
  sharing 1 wire after 50 iters, now in a multi-tile graph. Raising
  max_iters was NOT tried; it is a router-hardening follow-up.
- chain16/shift32: STA `timing: no driver for net din` (top-level
  input with no driver entry). STA was not weakened. Fix options: treat
  IOB-driven input nets as port arrivals, or document as the next gap.
- No max_iters change, no router cost-model change, no STA change, no
  CLK2 routing change. No vendored table touched; the intcols lane's 3
  tests still pass.

## 7. Suites and stash proofs (coordinator-verified)

Suites on the impl branch, serially, -j1, recounted by the coordinator
from the logs (0 FAILED lines in either log):

- helion-ultrascale: 448 passed / 0 failed / 1 ignored
  (suite-ultrascale.log). Baseline 441 + 7 new = 448 exact.
- helion-cli release: 78 passed / 0 failed / 1 ignored
  (suite-cli-release.log). Baseline 76 + 2 new = 78 exact. Stale
  /tmp/helion-us09-flow-{blinky,counter} dirs deleted before the run.
- US-09 checkers: 95 OK in 108.268s (checkers-us09.log, oss-cad-suite
  iverilog 14.0; system iverilog 12.0 is gone from this VM).

Stash proof, coordinator-verified: the branch diff is byte-identical
to fix.patch (8 files, 797+/126-). Full stash: counter5-s0 and
counter8-s0 fail on base with the baseline ROUTE OveruseTimeout
verbatim (cnt_0, wire 463 tile (1,0) shared by 2; cnt_0, wire 36 tile
(1,0) shared by 5, the sharing count varying run to run). Restore
byte-identical to fix.patch, verified twice. Targeted bitgen-only
stash (bitgen.rs + bitgen_device.rs only): ROUTE succeeds (6 iters),
BITGEN fails with exactly the predicted collision (INT_X1Y1 SDQ_56,
bit 10 of FAR 0x41a must read 0 but reads 1), isolating the section
2.6 fix. New tests on base: 7 of 8 fail (wire_db, placer spread,
both packer tests, both xpin tests, bitgen emit test); sink-wire
passes on base as documented.

## 8. Incidents

- System iverilog 12.0 is gone from this VM (/usr/bin/iverilog does
  not exist). Checkers ran on oss-cad-suite iverilog 14.0
  (~/workspace/oss-cad-suite/bin/iverilog on PATH); the baseline
  proved all 95 pass with it. No apt install attempted.
- The first checkers attempt in the baseline was SIGTERM-killed at
  ~94s with no cause established; reruns completed cleanly.
- An intermediate ultrascale run failed exactly one test
  (round_trip_returns_identical_fasm, stub-DB bit 100); the DB-derived
  max-bit fix resolved it and the full suite was re-run green.

## 9. Known gaps and carried items

- Next gaps from this lane: router span-continuation fidelity and
  CLK2 clock-net sink modelling (counter8 verify-bit); 2-nets-1-wire
  ROUTE congestion at 50 iters (counter8 s2-s4, counter16);
  STA din gate for top-level inputs (chain16, shift32).
- Carried, unchanged: IOB placement (still blocked on B1, needs a
  Vivado-built INT_INTF-routed design); INT_INTF tile model for
  DSP/BRAM routing; known-INIT Vivado datum; documented xczu3eg
  timing; remaining 54,907 non-band unexplained bits; N16 parse-side
  open; N9 block types 2+ untested; N34 stale default decoder;
  N35/N36 router nondeterminism; N37; N38; flags A-CTRL and A-F7F8
  still unresolved, need a CTO ruling.
- N41 (crew report lacking reproducible verify-bit commands) is
  closed for this lane: full commands are in the impl report
  section 9.

## 10. Files changed (impl branch, 8 files)

- `crates/helion-ultrascale/src/tileconn.rs`, `place.rs`,
  `pack.rs`, `router.rs`, `router/tests.rs`, `bitgen.rs`,
  `bitgen_device.rs`; `crates/helion-cli/src/ultra.rs`. No other
  files changed. No vendored DB file edited.
