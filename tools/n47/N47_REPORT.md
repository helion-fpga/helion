# N47 east-side clock-leg lane report

Branch: `fm/ultra-bitgen-n47` (code tip `5384b148`, merge
`82286ae4`, report commit follows). Base: `fm/ultra-bitgen-clk2leg`
at `ae874d5f`, Critic Session 43 PASS (qualified). Worktree:
`~/workspace/helion-ultra-bitgen-n47`. Logs:
`~/workspace/n47-logs/` (`baseline/`, `impl/`, `coord/`).
No merges into `fm/ultra-bitstream`, `fm/zedboard-merge`, or
master.

## 0. Verdict in one paragraph

N47 is closed. Counter16 seed 1 now emits a real east-side clock
leg (`INT_X6Y0.PIP.CTRL_E4.INT_NODE_GLOBAL_14_INT_OUT0` plus
`INT_NODE_GLOBAL_14_INT_OUT0.GCLK_B_0_9`) and passes verify-bit
at 40000 cycles (80000 comparisons, 0 mismatches) on two fix
runs, on the restored build, and on the coordinator's own
merged-branch run. The fix is one file, 111 insertions, 29
deletions: `clock_leg_targets` accepts CTRL_E4/CTRL_E5
alongside CTRL_W4/W5, with the same 8 GLOBAL candidates and
the same GCLK_B_0_8..15 spine. Suites are green or better
(ultrascale 453/0/1, CLI release 89/0/1, US-09 checkers 100
OK). All 7 regressions pass verify-bit and blinky `.bit` md5
is unchanged. One honest limit: no CTRL_E5 leg was emitted in
any flow in this lane (SLICE_X10Y0 holds ABCD FFs only on seed
1), so the E5 path is unit-test proven, not flow proven (see
section 8).

## 1. Baseline: N47 reproduced, seed 0 not a passing control

Worker W-BASELINE, worktree
`~/workspace/helion-ultra-bitgen-n47-baseline`, detached at
`ae874d5f`. Report:
`~/workspace/n47-logs/baseline/BASELINE_REPORT.md` (197 lines).

Counter16 seed 1, design
`~/workspace/intcols-logs/impl/counter16.sv` (16 FFs), exits 1
with the verbatim line:

```
impl: ULTRA CLOCK LEG: slice SLICE_X10Y0 pin CLK1 resolved to CTRL_E4, expected CTRL_W4
```

Note the wording: there is no `ULTRA FASM:` prefix. That prefix
was correct on the older spanclk tip, where the failure fired
inside `clock_leg_fasm`. On this tip the clk2leg lane (commit
`d25fd12f`) added `clock_leg_targets`, called pre-route at
`crates/helion-cli/src/ultra.rs:758` with no prefix wrapper, so
the same check fires there first. The slice, pin, resolved
wire, and expected wire are identical, and the Session 43
critic quotes the same prefix-free line verbatim (CRITIQUE.md
line 6332). This is a failure-site move, not a different
failure. Flow facts: synth 32 LUTs, 16 flops; pack 4 slices;
place seed 1, legality 0 violations; FASM ff 32, ff_control
18, ffmux 16; timing WNS 5750 ps; wiredb 16 tiles, 23600
wires. Route never runs, so there is no PIP count and no FASM
line count. No `.bit` and no `.fasm` are produced. The log is
byte-identical to the Session 43 critic's run (md5
`7ab7e1d14442e8dc655e640b333a234d` both files).

Counter16 seed 0, expected to pass, did NOT reproduce as a
passing control: both of two baseline runs exit 1 with ROUTE
OveruseTimeout (nit N48), on a different net each time (run 1
net `cnt_3` at wire 9276, run 2 net `cnt_5` at wire 9212, both
at tile (6,0), shared by 2 nets, after 50 iters). No `.bit`, no
`.fasm` either time. The N46 avoidance is visibly active (4
forbidden PIPs printed before route). Same seed giving
different failing nets on back-to-back runs confirms router
nondeterminism (N35). Consequence: seed 0 cannot be assumed to
pass after the N47 fix until route is re-proven on it. The
coordinator re-proved the flow (not verify-bit) on the merged
branch; see section 6.

Baseline suites on the base (serial, -j1): ultrascale 453
passed / 0 failed / 1 ignored (33 test-result lines; first run
was SIGTERM-killed mid-way, numbers from the complete re-run);
CLI release 87/0/1 (11 lines); US-09 checkers 100 OK in
117.678s on oss-cad-suite iverilog 14.0 (system 12.0 absent,
N49). Release build exit 0, 19m 20s. No source file was
modified and nothing was committed by the baseline worker.

## 2. Root cause and the fix

Root cause (read in the baseline report, sections 4 and 7):
`clock_leg_fasm` / `clock_leg_targets` in
`crates/helion-cli/src/ultra.rs` compared the resolved wire
against the west-side name only (CTRL_W4 for CLK1, CTRL_W5
for CLK2) and failed loud on any other wire. The resolver
(`clock_int_tile` via `TileconnDb::int_tile_for_site`) is
correct: CLEL_R tiles stitch CLK1 to CTRL_E4 (grid delta
[-1,0], INT partner INT_X6Y0) and CLK2 to CTRL_E5, per the
public DB.

The fix (worker W-IMPL, commit `5384b148`, 1 file,
`crates/helion-cli/src/ultra.rs`, +111/-29): a new
`clock_ctrl_wires(pin)` table returns `["CTRL_W4",
"CTRL_E4"]` for CLK1 and `["CTRL_W5", "CTRL_E5"]` for CLK2.
`clock_leg_targets` now iterates `(half, pin)`, resolves the
wire through `clock_int_tile` as before, and accepts it when
it is one of the pin's two wires. The resolved wire becomes
the target's `ctrl`, so the leg is emitted on the wire the DB
actually stitches, with the same 8 GLOBAL candidates and the
same GCLK_B_0_8..15 spine wires as west-side legs. A resolver
returning any other wire still fails loud, naming the slice,
the pin, the resolved wire, and both accepted wires. Doc
comments were updated where they named only the west-side
wires.

Entangled spots, verified mechanically against the vendored
segbits DB (script in the impl report, section 2):

| CTRL wire | forbidden PIPs (disagree with all 8 candidates) |
| --- | --- |
| CTRL_W4 | `CTRL_W4.VCC_WIRE` |
| CTRL_W5 | `CTRL_W5.VCC_WIRE`, `INT_NODE_SDQ_80_INT_OUT1.LOGIC_OUTS_W22` |
| CTRL_E4 | `CTRL_E4.VCC_WIRE` |
| CTRL_E5 | `CTRL_E5.VCC_WIRE` |

Only two PIPs in the whole DB set bit 21_35 (both CTRL_W5),
and all 8 leg lines exist for CTRL_E4 and CTRL_E5. So no
east-side leg touches 21_35 and the N46 class cannot recur on
an east-side tile. `clock_leg_forbidden_pips` and
`clock_leg_candidate_compatible` already parameterize on the
CTRL wire and needed no change; `check_fasm_int_pip_bits`
and `RouteDb::forbid_pips` are wire-name agnostic. The live
flow confirms it: the fix run prints exactly
`INT_X6Y0.PIP.CTRL_E4.VCC_WIRE` as the east-side forbidden
PIP.

Checker side: no change needed. Grep of `tools/us09v/` finds
no west-side-only assumption: the A-CLKSPINE rule is written
generically as `CTRL_Wx <- INT_NODE_GLOBAL_y <- GCLK_B_0_z`
and the spine test is shape-based. The only literal CTRL_W4
mentions are west-side test fixtures. The end-to-end proof
that the checker accepts an east-side leg is the counter16
verify-bit PASS itself.

Tests (3 new or extended, all passing by name in the CLI
suite log): `clock_ctrl_wires_accept_east_side` (new),
`clock_leg_forbidden_pips_east_side_only_vcc` (new),
`clock_leg_candidates_exist_in_int_tile` (extended to all
four CTRL wires).

## 3. Acceptance: counter16 seed 1

Design: `~/workspace/intcols-logs/impl/counter16.sv` (16 FFs,
led = cnt[15]). Flow env: `HELION_TILECONN_DB_ROOT` pointing
at the prjuray zynqusp ref, `HELION_TILECONN_PART`
`xczu3eg-sbva484-1-e`, release binary.

Run 1 (impl logs `flow-counter16-s1.log`,
`counter16-s1.fasm`): flow exit 0; synth 32 LUTs, 16 flops;
pack 4 slices; place seed 1; route 47 nets, 39 iters, 0
overused; route-to-pip 1310 PIPs; FASM lut 32 lines, outmux
17 lines, clock_leg_lines 6; bitgen 5,982,532 bytes. Leg
lines in the FASM:

```
1426: INT_X3Y0.PIP.CTRL_W4.INT_NODE_GLOBAL_10_INT_OUT1
1427: INT_X3Y0.PIP.INT_NODE_GLOBAL_10_INT_OUT1.GCLK_B_0_15
1428: INT_X3Y0.PIP.CTRL_W5.INT_NODE_GLOBAL_8_INT_OUT0
1429: INT_X3Y0.PIP.INT_NODE_GLOBAL_8_INT_OUT0.GCLK_B_0_14
1430: INT_X6Y0.PIP.CTRL_E4.INT_NODE_GLOBAL_14_INT_OUT0
1431: INT_X6Y0.PIP.INT_NODE_GLOBAL_14_INT_OUT0.GCLK_B_0_9
```

The east-side leg is real: SLICE_X10Y0 (FFMUX A1, B1, B2, C1,
D1, ABCD half only) clocks through INT_X6Y0 CTRL_E4.
SLICE_X4Y0 holds A-H FFs, so INT_X3Y0 carries both a CTRL_W4
and a CTRL_W5 leg.

Port map: settled by a full 16-pin sweep at 40000 cycles, not
assumed. Every other Q pin fails at exactly cycle 2^k - 1 for
its bit k, and all 16 bits are accounted for exactly once.
Canonical map `counter16-port-map.json`:

```json
{"clk":"SLICE_X1Y0.CLK1","led":"SLICE_X4Y0.AQ"}
```

40000 cycles is required: led = cnt[15] first goes high at
cycle 32768, so short runs cannot distinguish the high bits;
this is the same trap the counter12 FQ map fell into.

Verify-bit results (canonical map, 40000 cycles,
branch-built decoder, absolute `--workdir`, `--top`,
fixture RTL `counter16-rtl.v` with cnt initialized to 0):

- Run 1: PASS, decode 1645 lines, route 1316 PIPs / 85 nets,
  cosim 80000 comparisons, 0 mismatches.
- Run 2 (repeat seed 1, 1257 PIPs vs 1310: documented router
  nondeterminism, N35/N36; same leg lines, FFMUX inventory
  identical): PASS, decode 1592 lines, route 1263 PIPs / 85
  nets, cosim 80000/0.
- Restored build (stash proof): PASS, decode 1521 lines,
  route 1192 PIPs / 85 nets, cosim 80000/0.

## 4. Suites (serial, never concurrent with checkers)

Impl branch numbers, recounted by the coordinator from the
logs:

| suite | result | log |
| --- | --- | --- |
| helion-ultrascale (debug) | 453 passed / 0 failed / 1 ignored, 33 lines | `suite-ultrascale.log` |
| helion-cli (release) | 89 passed / 0 failed / 1 ignored, 11 lines | `suite-cli-release.log` |
| US-09 checkers | Ran 100 tests, OK, 162.472s, iverilog 14.0 | `checkers-us09.log` |

CLI is 87 + 2 new tests; ultrascale is unchanged at 453
(this lane adds no ultrascale-crate tests). Environment
notes: a service restart killed the first CLI release suite
run mid-compile (re-run from scratch at -j1); system
iverilog 12.0 absent (N49); the N52 perf_profile.rs E0133
warnings appear in the CLI suite build log, pre-existing.

## 5. Stash proof (both sides)

`fix.patch` in `~/workspace/n47-logs/impl/`: 223 lines,
sha256
`b7f4f1154fd497e7618ea71cb673b5ddce65979b76ce00cbce9226f9c2586391`.
The fix was already committed when the stash step ran, so
the base side was produced the equivalent way: detached
checkout of `ae874d5f`, rebuild, test, then checkout back.
Base side: release rebuilt (4m 05s); counter16 seed 1 exit 1
with the verbatim N47 line, no `.bit` produced. Restore
side: regenerated diff byte-identical to `fix.patch` (`cmp`
clean, same sha256); release rebuilt (5m 21s); counter16
seed 1 flow exit 0 (1186 PIPs, same three leg lines);
verify-bit PASS at 40000 cycles, cosim 80000/0.

## 6. Coordinator verification (merged branch 82286ae4)

The coordinator verified the lane from the merged branch with
its own builds and flows (logs in
`~/workspace/n47-logs/coord/`):

- Release build exit 0; merge diff byte-identical to
  `fix.patch` (`cmp` clean). Helion 12,929,832 bytes,
  helion-bitgen 5,739,536 bytes.
- Suites, re-run serially at -j1: ultrascale 453/0/1 (33
  lines), CLI release 89/0/1 (11 lines), US-09 checkers 100
  OK in 115.629s on oss-cad-suite iverilog 14.0. Exact match
  to the impl numbers.
- Counter16 seed 1, own flow from the merged binary: exit 0,
  east-side leg present in the FASM
  (`INT_X6Y0.PIP.CTRL_E4.INT_NODE_GLOBAL_14_INT_OUT0`),
  forbidden PIP `INT_X6Y0.PIP.CTRL_E4.VCC_WIRE`,
  clock_leg_lines=6; verify-bit PASS, cosim 80000/0.
- Counter16 seed 0, own flow from the merged binary: exit 0,
  1407 PIPs, `.bit` emitted. The N48 congestion that blocked
  seed 0 twice on the base did not reproduce on this attempt
  (run-dependent class). No CTRL_E lines in the seed-0 FASM:
  seed 0's east slice holds no FFs, so no east leg is needed
  there. Verify-bit was NOT run for seed 0 (no seed-0 port
  map exists; the lane proves seed 1 end to end).
- Counter12 seed 0, own flow: exit 0, CLK2 leg present
  (`INT_X1Y0.PIP.CTRL_W5.INT_NODE_GLOBAL_10_INT_OUT1`);
  verify-bit PASS, cosim 800/0.
- Blinky, own flow: exit 0, `.bit` md5
  `d82b70c8e29ce72f28e08c94f3a16d16`, unchanged; verify-bit
  PASS, cosim 800/0.

## 7. Regressions (fix binary, 400 cycles, cosim 800/0 each)

| design | verify-bit |
| --- | --- |
| counter12 seed 0 | PASS (CLK2 leg present) |
| counter8 seed 0 | PASS |
| counter8 seed 1 | PASS |
| counters2x4 | PASS |
| counter5 | PASS |
| counter | PASS |
| blinky | PASS |

Blinky `.bit` md5 `d82b70c8e29ce72f28e08c94f3a16d16`,
unchanged from the clk2leg lane.

## 8. Carried and limitations

- No CTRL_E5 leg was emitted in any flow in this lane:
  SLICE_X10Y0 holds ABCD FFs only on seed 1, so only a CLK1
  (CTRL_E4) leg is needed there. The E5 code path is the same
  table lookup and the same candidate pick as E4, and it is
  covered by unit tests (candidates exist for E5 in the INT
  tile JSON; the E5 forbidden set is only its VCC_WIRE PIP),
  but no flow in this lane proves an E5 leg end to end. A
  design placing EFGH FFs on an east slice would be needed
  for that; none exists in the current corpus. The critic
  must judge whether the E4 end-to-end proof plus E5 unit
  coverage closes N47 as named (the resolver failure was on
  CTRL_E4).
- Counter16 seed 0 verify-bit was not run in this lane. The
  flow exits 0 on the merged branch (coordinator, 1407 PIPs);
  a port map for seed 0 would need its own 16-pin sweep.
- N48 ROUTE OveruseTimeout: hit on the base for counter16
  seed 0 (2 of 2 baseline runs); not hit on any fix-branch
  run in this lane; remains carried, run-dependent.
- N50 class: restore-build helion measures 12,929,912 bytes,
  identical to the first fix build; coordinator merged-branch
  build measures 12,929,832 bytes, an 80-byte delta from a
  byte-identical source tree (same class as N50, diagnostic
  only).
- N51 (stale doc comment in `clock_leg_forbidden_pips`): the
  doc now records the verified east-side set, so the stale
  west-only wording is fixed by this lane as a side effect.
- N52 (perf_profile.rs E0133 warnings): pre-existing,
  untouched by this lane.
- N47 is closed by this fix, in the E4 sense above. No new
  nits were found by the workers. Carried, unchanged: IOB
  placement (still blocked on B1); INT_INTF tile model for
  DSP/BRAM routing; known-INIT Vivado datum; documented
  xczu3eg timing; remaining 54,907 non-band unexplained bits;
  N16 parse-side open; N9 block types 2+ untested; N34 stale
  default decoder; N35/N36 router nondeterminism; N37; N38;
  flags A-CTRL and A-F7F8 still unresolved, need a CTO
  ruling.

## 9. Reproducible commands

Flow (from `~/workspace/helion-ultra-bitgen-n47`):

```
export PATH="$HOME/.cargo/bin:$PATH"
export HELION_TILECONN_DB_ROOT=$HOME/workspace/prjuray-db-ref/zynqusp
export HELION_TILECONN_PART=xczu3eg-sbva484-1-e
./target/release/helion impl ~/workspace/intcols-logs/impl/counter16.sv \
  --part xczu3eg --seed 1 \
  --out ~/workspace/n47-logs/coord/counter16-s1.bit \
  --fasm-out ~/workspace/n47-logs/coord/counter16-s1.fasm
```

Expected: exit 0, `clock_leg_lines=6`, leg line
`INT_X6Y0.PIP.CTRL_E4.INT_NODE_GLOBAL_14_INT_OUT0` in the
FASM.

Verify-bit (from `tools/us09v` in the lane worktree):

```
US09V_DECODER_BIN=$HOME/workspace/helion-ultra-bitgen-n47/target/release/helion-bitgen \
PATH=$HOME/workspace/oss-cad-suite/bin:$PATH \
python3 -m us09v verify-bit --bit ~/workspace/n47-logs/coord/counter16-s1.bit \
  --design counter16 --rtl ~/workspace/n47-logs/impl/counter16-rtl.v \
  --top counter16 --port-map ~/workspace/n47-logs/impl/counter16-port-map.json \
  --workdir ~/workspace/n47-logs/coord/verify-coord-s1 --cycles 40000
```

Expected: `counter16: PASS`, cosim 80000 comparisons,
0 mismatches. Note the 40000-cycle count: at 400 cycles every
high counter bit is still 0 and the map cannot be trusted.

## 10. Evidence index

- `~/workspace/n47-logs/baseline/BASELINE_REPORT.md` (197
  lines), `flow-counter16-s1.log` (verbatim N47),
  `flow-counter16-s0.log`, `flow-counter16-s0-r2.log` (N48),
  `suite-ultrascale-rerun.log`, `suite-cli-release.log`,
  `checkers-us09.log`
- `~/workspace/n47-logs/impl/IMPL_REPORT.md` (427 lines),
  `fix.patch`, `fix-restored.patch` (byte-identical),
  `build-release.log`, `build-release-base-stash.log`,
  `build-release-fix-restore.log`, `flow-counter16-s1.log`,
  `counter16-s1.bit/.fasm`, `flow-counter16-s1-r2.log`,
  `counter16-rtl.v`, `counter16-port-map.json` (canonical),
  `counter16-port-map-<slice>-<pin>.json` (pin sweep),
  `verify-c16-s1-X4Y0-AQ.log`, `verify-c16-s1-r2.log`,
  `verify-c16-s1-<slice>-<pin>.log` (16 runs),
  `stash-proof-base.log`, `flow-restore-s1.log`,
  `verify-restore-s1.log`, `suite-ultrascale.log`,
  `suite-cli-release.log`, `checkers-us09.log`,
  `flow-*.log` and `verify-*.log` for the 7 regressions
- `~/workspace/n47-logs/coord/build-release.log`,
  `suite-ultrascale.log`, `suite-cli-release.log`,
  `checkers-us09.log`, `flow-counter16-s1.log`,
  `counter16-s1.bit/.fasm`, `verify-coord-s1.log`,
  `flow-counter16-s0.log`, `counter16-s0.bit/.fasm`,
  `flow-counter12-s0.log`, `counter12-s0.bit/.fasm`,
  `verify-coord-c12.log`, `flow-blinky.log`, `blinky.bit/.fasm`,
  `verify-coord-blinky.log`
- Lane branch `fm/ultra-bitgen-n47`: impl commit
  `5384b148`, merge `82286ae4`, report commit follows.
  No critic dispatched by this lane; the main agent
  dispatches Session 44.
