# US-01 Mining Lane Report

Date: 2026-11-21. Lane branch: `fm/ultra-bitgen-us01-mine`, worktree
`~/workspace/helion-ultra-bitgen-us01-mine`, base `6f8a8ba2` (US-09,
Session 30 PASS). Head: `ee0f246e`, pushed to origin and verified
with `git ls-remote origin fm/ultra-bitgen-us01-mine`. No merge into
any other branch. No critic dispatched by this lane (that is the
coordinator's call). No Vivado used anywhere.

## Outcome, stated plainly

US-01 is NOT closed end to end: no design flows through `helion impl`
yet, and no bitstream was emitted or decoded by this lane. What is
closed is the US-01 data gap for CLE slices: the BEL-pin-to-INT-wire
map exists in the public DBs, loads into `helion-ultrascale` as a
`WireDb`, and routes blinky's q net in the harness. The remaining
blocker for an end-to-end blinky on xczu3eg is GAP-F5 (preliminary
STA, `timing: unsupported part xczu3eg`), which stops the flow
before ROUTE no matter what the router does.

The lane's premise changed mid-flight, and the change is the story:
Saksham chose bitstream mining because the standing evidence said no
public source maps BEL pins to INT wires. Characterization proved
that premise wrong for CLE slices. `tileconn.json` was sitting in
the public DB reference all along, missed because the prior check
stopped one directory level too shallow (`find -maxdepth 3`). The
lane therefore did not mine a map from bitstreams. It loaded the
existing map and used the bitstreams as validation data, exactly as
the characterization report recommended.

## Merges (one --no-ff merge commit each, in order)

| Order | Branch merged | Head | Merge commit | Conflicts |
|---|---|---|---|---|
| 1 | fm/ultra-bitgen-us01-char | 5b82c49f | f46295a8 | none |
| 2 | fm/ultra-bitgen-us01-validate | 902ad393 | 2865a051 | none |
| 3 | fm/ultra-bitgen-us01-wiredb | 708f30f5 | ee0f246e | none |

All three worker branches are on origin; all three heads are
reachable from the lane branch. No fix commits on the lane branch.

## Workstream 1: characterization (W-CHAR)

Report: `~/workspace/ultra-bitgen-crew/US01_CHAR_REPORT.md` (828
lines, 0 em dashes). Logs under `~/workspace/us01-char-logs/`.

- Decoded all 14 xczu3eg pairs with the US-09 decoder and
  reproduced the known totals exactly: 22,090,185 FASM lines,
  2,722,823 unexplained bits (13 files byte-identical to stored
  decodes; P-11 has 48 fewer lines, all default V0 CMT WIRE lines,
  no information loss).
- Found `tileconn.json` at
  `~/workspace/prjuray-db-ref/zynqusp/xczu3eg-sbva484-1-e/tileconn.json`:
  84,343,553 bytes, 81,640 entries, 658,732 wire pairs. The
  coordinator verified the file size and entry count independently,
  and re-counted the wire pairs with its own Python (658,732,
  exact). Entries are `{grid_deltas, tile_types, wire_pairs}` with
  0 `site_pins` keys. Site pins live in `site_types` files (41
  files); PIPs and wires live in `tile_types` files (158 files).
  The 7-series precedent is already in the repo
  (`crates/helion-xilinx7/src/db/load.rs`, load_tileconn, plus
  union-find fabric build).
- Verdicts: LUT inputs DETERMINED (P-11 26,569 needed pins
  accounted for, P-13 113,086; residuals 700 and 1,824, all
  VCC-tied per the VCC-default rule). FF outputs, FF D, FF control
  pins DETERMINED. CARRY8 DETERMINED with two open items (DI feed
  semantics; flag A-F7F8). DSP48E2 NOT MINEABLE from decoded FASM
  (0 DSP lines, no DSP segbits). BRAM PARTIAL (config decodes,
  60,307 lines; no BRAM site PIP ever appears in FASM;
  segbits_bram.db has 0 PIP lines). Ground truth: no pair fully
  verified; BNN-PYNQ P-13 to P-17 are the best pinning candidates.
- Recommendation accepted by the coordinator: do not mine the CLE
  pin-to-wire map from bitstreams; load tileconn (plus tile_types
  and site_types) in `helion-ultrascale` and treat the bitstreams
  as validation data.

## Workstream 2: validation (W-VALIDATE)

Report: `~/workspace/ultra-bitgen-crew/US01_VALIDATE_REPORT.md`
(286 lines, 0 em dashes). Logs under
`~/workspace/us01-validate-logs/`.

- Full 14-pair consistency: ZERO contradictions. 12,335,283
  decoded PIPs checked against the static DBs (tile_types plus
  tileconn); all agree on every check (wire exists, edge exists,
  BEL-pin stitch agrees), with 3,357,492 BEL-pin agreements. The
  coordinator re-added the per-pair PIP, C4, and needed-pin columns
  independently and all three sums match exactly (12,335,283 /
  3,357,492 / 1,587,046).
- Held-out tie-off PASS on P-40 (FINN: 122,746 real-driven, 2,036
  VCC-tied, 0 problems) and P-28 (PYNQ-ComputerVision: 48,430
  real-driven, 615 VCC-tied, 0 problems). The VCC-default rule
  generalizes to families it never saw during development.
- VCC-default rule re-check holds on all 14 pairs: 1,559,317
  real-driven, 27,729 VCC-tied, 0 misclassifications.
- DI-fed LUT semantics settled functionally: `CARRY8.DI{i}.DI{i}`
  means DI[i] is the same LUT O6 net as S[i]. The extractor fix is
  a one-line change in `extract.py` `_carry_di` (return `lut_o6[c]`
  instead of `1'bx`), with the VCC-default rule applied first,
  proved by direct probe of the real extractor.
- Coverage: the corpus wire-verifies 210 of 214 CLE pins (98.1
  percent) but 0 of 1,168 DSP/BRAM pins (never exercised); 210 of
  1,382 overall (15.2 percent). The map itself is complete in
  tileconn; the corpus only corroborates the CLE part.
- Checker corrections recorded, not hidden: bidirectional PIPs carry
  `.FWD`/`.REV` suffixes in FASM (18,890 lines); SLICE PIPs name
  sites, not tiles (78 lines in the first run). Both resolved in
  the checker; final numbers come from the corrected checker.

## Workstream 3: wire database (W-WIREDB)

Report: `~/workspace/ultra-bitgen-crew/US01_WIREDB_REPORT.md`
(458 lines, 0 em dashes). Logs under
`~/workspace/us01-wiredb-logs/`.

- Loader: `crates/helion-ultrascale/src/tileconn.rs`
  (`TileconnWireDb` implementing `WireDb`), plus
  `tests/tileconn_route.rs`, `lib.rs` re-exports, and
  `crates/helion-cli/src/ultra.rs` flow wiring (RouteDb enum,
  `load_route_db`). Data-file decision, with reasons: runtime load
  from a prjuray-db family root via `HELION_TILECONN_DB_ROOT`
  (optional `HELION_TILECONN_PART`), no vendoring of the 84 MB
  tileconn plus 15 MB tilegrid plus 18 MB tile_types. Without the
  env var the flow is byte-identical to US-09.
- Resolution examples: CLEL_R A1 to `IMUX_E10` delta (-1,0), A_O
  to `LOGIC_OUTS_E0`, AQ to `LOGIC_OUTS_E14`, AQ2 to
  `LOGIC_OUTS_E23`, AX to `BOUNCE_E_0_FT1`; real tilegrid INT tiles
  (`SLICE_X7Y0` partners `INT_X5Y0`, not the provisional
  `INT_X7Y0`).
- Blinky q net routes in the harness on the loaded map
  (`route_ultra` direct invocation, because `helion impl` stops at
  STA first): `LOGIC_OUTS_E0 -> INODE_E_1_FT1 -> BOUNCE_E_0_FT1`,
  translating to 2 PIPs that exist in `segbits_int.db` with real
  set bits and in decoded corpus FASM (P-11: 7 and 59, P-13: 19
  and 246, P-40: 29 and 357 occurrences). The golden blinky xczu3eg
  FASM contains only 1 of the 2 (the BOUNCE one); the extra INODE
  PIP is a real directional PIP, a real segbits feature, and occurs
  in decoded P-11 FASM, so it is a genuine routing PIP, not a graph
  artifact.
- Suites, verified by the coordinator re-counting the worker's
  logs: ultrascale 421 passed / 0 failed / 1 ignored both with and
  without the env var (baseline 412/0/1, delta exactly the 9 new
  tests: 6 unit plus 3 integration), CLI 66/0/1 exactly baseline,
  N16 stays ignored. One test bug found and fixed in-lane (fixture
  temp-dir race between parallel lib tests); no production code was
  touched for the fix.
- Flow runs, all exit 1, all loud: xczu3eg with and without the map
  stops at STA (`timing: unsupported part xczu3eg`); xcku3p without
  env stops at ROUTE with NoPath (unchanged US-09 behavior); xcku3p
  with env fails loud on the missing xcku3p part directory (no
  xcku3p tileconn exists in this DB root, which holds zynqusp
  parts only).

## Correction to the characterization report (found by W-WIREDB)

The char report's Q4 coverage table said DSP 432/432 and RAMBFIFO36
533/533 pins stitched in tileconn. Both are wrong, and the mechanism
is pinned: the char counts conflated plain INT tiles with INT_INTF
tiles via a `startswith("INT")` match. The loader's accounting:

| Site type | Pins | Any tileconn partner | Reaches INT_INTF fabric |
|---|---|---|---|
| SLICEL | 106 | 106 | 0 (plain INT, 104) |
| SLICEM | 108 | 108 | 0 (plain INT, 106) |
| DSP48E2 (per site) | 432 | 334 | 236 |
| RAMB181 | 203 | 203 | 131 |
| RAMBFIFO36 | 533 | 88 | 81 |
| RAMBFIFO18 | 242 | 242 | 166 |

RAMB181 203/203 and RAMBFIFO18 242/242 do reproduce. The 36-bit
site is a view over the two 18K halves: tileconn carries 203 CORE_1
wires, 242 CORE_2 wires, and only 88 CORE_3 wires. DSP/BRAM pins
stitch to INT_INTF tiles (the interface columns flanking DSP/BRAM
columns), never to plain INT tiles, so routing them needs an
INT_INTF tile model. The CLE counts hold (106/106 with the `_PIN`
strip applied). The CLE verdicts that matter for blinky routing
stand unchanged. This correction is recorded in the wiredb report
and here, not hidden; the char report itself is not rewritten
(the lane's reports are records of what each worker found).

## Merged-branch verification

Expected totals, computed before running from the three branch
counts: ultrascale N7 baseline 338 + decoder 53 + device 13 + flow 8
(US-09 total 412) + wiredb 9 new tests = 421 passed, 0 failed, 1
ignored; CLI 66/0/1 unchanged (the wiredb flow wiring adds no CLI
tests).

Results (all commands `cargo test --release -j1`, logs under
`~/workspace/us01-mine-logs/`):

- `helion-ultrascale` (log `test-ultrascale-merged.log`):
  421 passed, 0 failed, 1 ignored, across 31 test-result lines,
  exit 0. Exactly the expected total, zero deviation, zero
  regressions. The one ignored test is the N16 mixed-column test
  carried from the N7 lane.
- `helion-cli` (log `test-cli-merged.log`): 66 passed, 0 failed,
  1 ignored, exit 0. Exactly the baseline.

Incident: the first merged ultrascale run was killed mid-suite by a
runtime restart (log stopped at 00:45 UTC with 2 test-result lines,
no failures, no exit line; no cargo process survived). The run was
re-started from scratch on the same tree (target dir intact, never
cleaned) and completed to the 421/0/1 result above. The partial log
was overwritten by the re-run; the restart and the re-run are
recorded here, not hidden.

## Blockers (each with a named unblocker)

- GAP-F5, STA on xczu3eg (primary for end-to-end flow): `impl:
  ULTRA STA: timing: unsupported part xczu3eg` (exit 1). Until an
  xczu3eg STA DB exists, no design can flow end to end on xczu3eg
  no matter what the router does. Unblocker: a
  `preliminary_xczu3eg()` delay DB (follow-on GAP-F5 work). No
  timing numbers were invented by this lane.
- INT_INTF tile model for DSP/BRAM routing: DSP and BRAM pins
  stitch to INT_INTF tiles (472 DSP pins, 131 RAMB181, 81
  RAMBFIFO36, 166 RAMBFIFO18 counted, none edged in the graph,
  which is built from the plain INT tile JSON only). Unblocker: a
  follow-up lane adding `tile_type_INT_INTF_*.json` PIPs to the
  graph plus site-indexed DSP/BRAM registration. Does not block CLE
  routing.
- Multi-sink translate limitation, unverified with the map: the
  router emits a flattened union of per-sink paths, and the
  translator fails loud on those with
  `RouteToPipError::NotConnected`. Blinky's q net is single-sink,
  so this lane did not hit it. Counter would exercise it, but
  counter carries Carry4 cells (fail loud earlier: carry routing
  not implemented in the flow yet) and stops at STA first on
  xczu3eg. Unblocker: the router must emit trees; a multi-sink,
  carry-free design is the next routing proof once GAP-F5 clears.
- No xcku3p tileconn in the public DB root: `helion impl` on
  xcku3p with the env var fails loud on the missing part
  directory. Unblocker: a public xcku3p tileconn (none is known to
  exist in prjuray-db), or a derived edge table for xcku3p.
- FLAG A-F7F8 (for the critic/CTO, NOT resolved): F7/F8/F9 mapping
  is an unvalidated assumption. Unblocker: a Vivado build using
  F7/F8/F9, or UG574 figure confirmation.
- FLAG A-CTRL (for the critic/CTO, NOT resolved): the checker's
  per-half control-set rules differ from in-repo `legality.rs`
  per-slice limits. Carried from US-09.

## What this lane did NOT do

- Did not dispatch a critic (coordinator's call).
- Did not merge anything into any other branch, including
  fm/ultra-bitstream and master. The only push is
  fm/ultra-bitgen-us01-mine to origin.
- Did not invent xczu3eg timing numbers or add a
  preliminary_xczu3eg() DB.
- Did not emit or decode a bitstream from the flow, and does not
  claim US-01 closed end to end.
- Did not rewrite the characterization report's coverage table
  after the wiredb correction; the correction is recorded in the
  wiredb report and in this report.
- Did not resolve the DI-fed LUT extractor question in code: the
  one-line `_carry_di` fix is recommended by W-VALIDATE with a
  worked example, not applied by this lane (it lives in the
  checkers, not the loader).

## Logs

All under `~/workspace/` (never /tmp):

- `~/workspace/us01-char-logs/`: decode-all, per-pair decode logs,
  fresh decodes in `decoded/`, tie-off and trace outputs.
- `~/workspace/us01-validate-logs/`: tieoff-all, consistency-all,
  per-pair JSONs, coverage JSONs, DI probe log.
- `~/workspace/us01-wiredb-logs/`: build-cli, test-ultrascale-full,
  test-ultrascale-full2, test-ultrascale-full-env,
  test-ultrascale-lib2, test-cli-full, test-tileconn-route-real3,
  flow-blinky-xcku3p, flow-blinky-xcku3p-tileconn,
  flow-blinky-xczu3eg, flow-blinky-xczu3eg-tileconn.
- `~/workspace/us01-mine-logs/`: merged-branch verification
  (test-ultrascale-merged, test-cli-merged).
