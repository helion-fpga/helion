# N58 router-fix lane report (fm/ultra-bitgen-n58), Nov 24, 2026

Lane branch `fm/ultra-bitgen-n58` (impl commit `68f96af9` on stadin base
`be1a3178`; merge `6978a724`; report commit `61b7a29d`). Impl branch
`fm/ultra-bitgen-n58-impl` at `68f96af9` on origin.
Worktree `~/workspace/helion-ultra-bitgen-n58`, clean. Report
`~/workspace/ultra-bitgen-crew/N58_REPORT.md` (this file); copy committed
at `tools/n58/N58_REPORT.md` on the lane branch, byte-identical.

## 1. Headline: N58 is CLOSED, as redescribed by Critic Session 46

Chain16 and shift32 both pass verify-bit end to end on the fix with a
REAL din route, where chain16 failed cosim on the base (403 mismatches)
and din was never routed at all. The coordinator re-ran the acceptance
from the merged branch binary: chain16 twice (exit 0, WNS_PS=8570,
362 PIPs, `.bit` md5 `4cf99124` byte-identical both runs and identical
to the impl fix run and the stash restore run), shift32 twice (exit 0,
WNS_PS=8690, 289 PIPs, `.bit` md5 `a7a070dd` byte-identical both runs
and identical to the impl fix run and the stash restore run). All 8
regression designs pass verify-bit on the merged branch; blinky `.bit`
md5 `d82b70c8` unchanged.

N58 as corrected by Session 46 ruling (b) is a FLOW ROUTING bug, not an
extractor gap: (i) port-driven nets are dropped before routing, so din
never reaches any FF D pin; (ii) the router treats BYPASS pin-feed
wires as ordinary transit wires, so the H net drives pipe_0's EX pin.
The extractor reads the FASM correctly and was not touched; no file
under `tools/us09v/` changed in this lane.

## 2. The fix (3 files, +358/-4)

Fix commit `c4aab79f`, plus test-only commit `68f96af9` (W8 router
test). Diffstat tip vs base: `crates/helion-ultrascale/src/router.rs`
+51/-4, `crates/helion-cli/src/ultra.rs` +230/-0,
`crates/helion-ultrascale/src/router/tests.rs` +77/-0.

### 2a. BYPASS wires feeding a used site pin are terminal-only

`route_ultra` builds a `protected` set: every sink wire of every
routed net whose `wire_info` name starts with `BYPASS_`. `astar`
gains `blocked: &HashSet<WireId>` and `own_sinks: &[WireId]`; after
the stale-entry check, a leg never expands from a blocked wire
unless it is the leg's source or one of that net's own sink wires.
The leg's destination is always one of its own sinks, so it is never
blocked for its own net. The rule is a name-prefix rule, so it covers
the whole BYPASS family (W1-W14, E variants, every number), not a
per-wire list. A wire no net names as a sink stays usable as transit.
A net may still end at its own BYPASS sink wire and pass through it
on a later leg to another sink: the pin is meant to see this net. A
hard rule, not presence counting: on base, din was unrouted, so
BYPASS_W8 had presence 1 (H only) and overuse 0, and the bad route
was returned. Presence counting cannot see the bug.

### 2b. Port-driven nets are routable

`build_routable_nets` collects the top-level input ports
(`PortDir::In` / `Inout`) into `input_ports`. A net with no cell
driver that is port-driven no longer drops silently: it gets a
virtual driver from `port_entry()` and routes to all its sinks.
`port_entry()`: the first unused LUT letter (A-H) of the first sink
slice in sorted sink order, falling back to the next sink slice. The
entry pin is an OUT pin by construction, so the US-09 checker
resolves it as a primary driver and traces every sink back to the
port through the emitted PIPs, exactly as it resolves an IOB "I" pin.
The letter must have no LUT, so its O wire is driven by nothing but
the port. An FF in the same letter is no conflict: only a LUT drives
the O wire. No sink slice with a free LUT letter fails loud naming
the net. A port net whose sinks are all clock pins still drops out
(clock legs serve them). A `hang_diag` line names the entry on every
run, e.g. `hang_diag ultra route port net=din entry=SLICE_X4Y0.H6LUT.O6
reason=top-level input port; virtual driver at unused LUT O pin`.

Why an unused LUT O pin and not the sink pin itself: `IMUX_W3`
(F1's wire) has no outgoing PIPs in the tile JSON (it is a leaf), so
F1 cannot be a route source; and the extractor's `driver()` resolves
a primary only from an OUT-direction site pin, so an IN-pin map can
never make both din sinks resolve. The entry pin is the analogue of
the IOB "I" pin while IOB placement is blocked on B1.

Untouched guarantees: determinism from N48 (sorted fanout lists),
the 50-iteration budget, CLK2/E4/E5 leg machinery, FFMUX D6/BYP
rules. The counter test's BYPASS_W6 transit stays allowed (W6 is not
a sink wire there).

## 3. Baseline (W-BASELINE, 189-line report)

Worktree `~/workspace/helion-ultra-bitgen-n58-baseline`, detached at
`be1a3178`, clean. Build exit 0 (12m 02s). N58 reproduces verbatim on
chain16 with the stadin din map (din `SLICE_X4Y0.F1`): decode 530
FASM lines, parse, ports, place, route (347 PIPs, 48 nets, no
violations), extract (5 sites, 36 warnings) all pass; cosim FAIL,
800 comparisons, 403 mismatches, first mismatch cycle 19.
`extracted.v` byte-identical to the lane's; flow FASM
sorted-identical to the lane, coordinator, and critic FASMs. Din
unrouted, confirmed by grep on the base FASM: exactly 1 IMUX_W3 PIP
in the whole FASM, at INT_X1Y0; zero PIPs drive INT_X3Y0 IMUX_W3.
BYPASS transit confirmed: 31 BYPASS_W PIP lines, both named W8 PIPs
present. Baseline suites 460/0/1, 89/0/1, 100 OK.

Two baseline findings that changed the lane, both new:

1. **Shift32 PASSES verify-bit on the base (800/0, twice).** This
   contradicts the critic's expectation that shift32 would hit the
   same gap. Port map derived by tracing the extracted FF chain: head
   sr_0 at `SLICE_X10Y0` letter H with D `1'bx`, tail sr_31 at
   `SLICE_X1Y0.AQ`, so din `SLICE_X10Y0.HX`, dout `SLICE_X1Y0.AQ`.
   The pass is not vacuous (stimulus: 226 zeros, 174 ones over 400
   cycles). Why it passes: shift32 has no LUTs, so sr_0's D comes
   directly through the HX site pin and the port map supplies din by
   fiat; no second net collides on sr_0's BYP wire. Part (i) still
   holds at fabric level for shift32 (zero IMUX_W3 PIPs, no PIP sinks
   into BYPASS_W11, no INT_X9Y0 PIPs at all). Only chain16 exercises
   N58 part (ii) at verify-bit level. The impl lane answered this by
   using a discriminating map (section 4): din `SLICE_X10Y0.A_O`
   fails on base (737 mismatches) and passes on fix.
2. **The ruling's BYPASS transit list (W1, W4, W5, W8) is partial.**
   The base chain16 FASM carries 31 BYPASS_W PIP lines across W1,
   W3-W12. The fix's name-prefix rule covers the whole family (see
   section 2a), and two unit tests pin two different wires (W1/BX
   and W8/EX).

## 4. The din routes are REAL (coordinator, merged branch binary)

The acceptance maps name the entry pin, not the sink pin, so a
verify-bit pass cannot be port-map fiat: chain16 din
`SLICE_X4Y0.H_O`, shift32 din `SLICE_X10Y0.A_O`. The chains were
traced through the decoded FASM with the checker's own fabric model
(`trace-din.py` in `~/workspace/n58-logs/impl/`, BFS in tileconn
node space).

Chain16, entry `SLICE_X4Y0.H_O` (wire `LOGIC_OUTS_W28` at INT_X3Y0)
to pipe_0 D (`EX`), 8 PIPs:

```
INT_X3Y0.PIP.INT_NODE_SDQ_90_INT_OUT1.LOGIC_OUTS_W28
INT_X3Y0.PIP.WW1_W_BEG7.INT_NODE_SDQ_90_INT_OUT1
INT_X2Y0.PIP.INT_NODE_SDQ_47_INT_OUT0.WW1_W_END7
INT_X2Y0.PIP.EE1_E_BEG7.INT_NODE_SDQ_47_INT_OUT0
INT_X3Y0.PIP.INT_NODE_IMUX_59_INT_OUT1.EE1_E_END7
INT_X3Y0.PIP.BYPASS_W14.INT_NODE_IMUX_59_INT_OUT1
INT_X3Y0.PIP.INT_NODE_IMUX_49_INT_OUT1.BYPASS_W14
INT_X3Y0.PIP.BYPASS_W8.INT_NODE_IMUX_49_INT_OUT1
```

Same 8 PIPs plus 2 more reach pipe_1's LUT input (`F1`), 10 PIPs:

```
INT_X3Y0.PIP.INT_NODE_IMUX_37_INT_OUT1.BYPASS_W8
INT_X3Y0.PIP.IMUX_W3.INT_NODE_IMUX_37_INT_OUT1
```

The F1 leg passes through BYPASS_W8, which is din's own EX sink wire
(allowed by the own-sink exemption), and through BYPASS_W14, which
feeds the G_I pin. No routed net sinks at any `_I` pin (the flow
never connects `_I` pins; the extracted model reads no `_I` pin),
so W14 is transit-eligible under the scoped rule. Extracted model:
`s_SLICE_X4Y0__EQ <= s_SLICE_X4Y0__H_O` (pipe_0 D is the din entry
pin). Coordinator's own grep on `coord-c16.fasm` confirms the new
INT_X3Y0 IMUX_W3 PIP (2 IMUX_W3 lines on fix vs 1 on base).

Shift32, entry `SLICE_X10Y0.A_O` (wire `LOGIC_OUTS_E0` at INT_X6Y0)
to sr_0 D (`HX`), 7 PIPs:

```
INT_X6Y0.PIP.INT_NODE_SDQ_0_INT_OUT1.LOGIC_OUTS_E0
INT_X6Y0.PIP.WW4_E_BEG0.INT_NODE_SDQ_0_INT_OUT1
INT_X4Y0.PIP.INT_NODE_SDQ_33_INT_OUT0.WW4_E_END0
INT_X4Y0.PIP.EE4_E_BEG0.INT_NODE_SDQ_33_INT_OUT0
INT_X6Y0.PIP.INT_NODE_GLOBAL_0_INT_OUT1.EE4_E_END0
INT_X6Y0.PIP.INT_NODE_IMUX_16_INT_OUT0.INT_NODE_GLOBAL_0_INT_OUT1
INT_X6Y0.PIP.BYPASS_E11.INT_NODE_IMUX_16_INT_OUT0
```

Naming reconciliation with the baseline: `SLICE_X10Y0` sits in tile
`CLEL_R_X6Y0`, so its pins stitch to E-named wires (LOGIC_OUTS_E0,
BYPASS_E11), not W11 at a hypothetical INT_X9Y0. The baseline's "no
PIP into BYPASS_W11 / no INT_X9Y0 PIPs" is the same fabric fact
under west-tile naming. Extracted model on the fix:
`s_SLICE_X10Y0__HQ <= s_SLICE_X10Y0__A_O`. Coordinator's own grep on
`coord-s32.fasm` confirms both the LOGIC_OUTS_E0 and BYPASS_E11
PIPs. Base with the same A_O map FAILS (737 mismatches, 737 net
unknowns, first mismatch cycle 31; trace reports NO PATH from A_O
to HX), so the fix's shift32 pass is real routing, not fiat.

## 5. Acceptance (coordinator, merged branch binary)

Merged-branch build: `cargo build --release -j1 -p helion-cli
-p helion-ultrascale`, exit 0 (log `coord/build-release-rerun.log`;
the first attempt was killed by a runtime restart mid-compile, log
ends mid-compile with no binary produced, re-run in full);
`helion` 12,955,160 bytes, md5 `e24602a5bb471ed7fa471b387ba455b2`
(impl fix build 12,955,176 bytes, md5 `14cfe68ceb159a5108f7614e99e14ad3`;
16 bytes smaller, the known N54 worktree effect);
`helion-bitgen` 5,740,016 bytes, md5 `e7692787dbbd27fdc3bde27c351bbdad`
(impl 5,740,016 bytes, md5 `c288b369c1379853550ba892263309b0`; same
size, different md5, also N54). Script `coord/run-coord-flows.sh`,
summary `coord/coord-flow-summary.txt`. Flow env:
`HELION_TILECONN_DB_ROOT=$HOME/workspace/prjuray-db-ref/zynqusp`,
`HELION_TILECONN_PART=xczu3eg-sbva484-1-e`.

| design | run | exit | WNS_PS | nets | iters | route-to-pip PIPs | .bit md5 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| chain16 | fix r1 | 0 | 8570 | 28 | 7 | 362 | 4cf9912486bbd43ddd20ce82b6856965 |
| chain16 | fix r2 | 0 | 8570 | 28 | 7 | 362 | 4cf9912486bbd43ddd20ce82b6856965 |
| shift32 | fix r1 | 0 | 8690 | 32 | 2 | 289 | a7a070ddcaaec67a3215b99447524286 |
| shift32 | fix r2 | 0 | 8690 | 32 | 2 | 289 | a7a070ddcaaec67a3215b99447524286 |

Both runs byte-identical per design, and identical to the impl fix
runs and the stash restore runs (section 8). WNS unchanged from
base. Nets rise by 1 (the din net now routes); PIPs rise by 21
(chain16) and 9 (shift32): the din legs plus reroutes around
protected wires.

Verify-bit (branch-built decoder, oss-cad-suite iverilog 14.0,
absolute workdirs, `--top` on every run, 400 cycles; script
`coord/run-coord-verify.sh`, summary `coord/coord-verify-summary.txt`):

| design | run | map din | result |
| --- | --- | --- | --- |
| chain16 | fix r1 | `SLICE_X4Y0.H_O` | PASS, 800 comparisons, 0 mismatches, 0 RTL unknowns, 0 net unknowns |
| chain16 | fix r2 | `SLICE_X4Y0.H_O` | PASS, 800/0/0/0 |
| shift32 | fix r1 | `SLICE_X10Y0.A_O` | PASS, 800/0/0/0 |
| shift32 | fix r2 | `SLICE_X10Y0.A_O` | PASS, 800/0/0/0 |

Route stage on the fix bitstreams: chain16 368 PIPs, 48 nets, no
violations; shift32 303 PIPs, 48 nets, no violations. Extract: no
errors.

## 6. New tests, with fail-before proof

Five new tests, all FAIL on base and PASS with the fix, all passing
by name in the coordinator's own suite runs (section 7).

Router (`crates/helion-ultrascale/src/router/tests.rs`):

- `bypass_sink_wire_is_terminal_for_other_nets` (W1/BX): fixture
  driver `LOGIC_OUTS_W7` to sink `IMUX_W26` via `BYPASS_W1`
  (shortest 8 wires; shortest avoiding W1 is 10), victim net `C6LUT`
  O6 to `BQ`/`D` at `BYPASS_W1`, `max_iters=1`, through net routed
  first. Base: FAIL, `OveruseTimeout`, wire 28 shared by 2 nets.
  Fix: pass, 0 overuse, through route excludes W1, victim route
  contains W1.
- `bypass_sink_wire_is_terminal_for_other_nets_w8` (W8/EX):
  driver `LOGIC_OUTS_W20` to sink `IMUX_W3` via `BYPASS_W8`
  (shortest 5 wires; shortest avoiding W8 is 7), victim net `C6LUT`
  O6 to `EQ`/`D` at `BYPASS_W8`, `max_iters=1`. Base: FAIL,
  `OveruseTimeout`, wire 38 shared by 2 nets. Fix: pass, 0 overuse.

Route builder (`crates/helion-cli/src/ultra.rs`):

- `port_driven_net_is_routable_with_virtual_driver`: hand-built
  design, din port to an FF D (letter A) plus a LUT input (letter
  B), clk port to the FF CLK. Asserts din routes with driver bel
  `A6LUT` pin `O6` and 2 sinks, and clk is absent. Base: FAIL,
  `din must route`. Fix: pass.
- `port_driven_net_routes_on_int_tile`: the built din net routes on
  one INT tile from `LOGIC_OUTS_W0` to `BOUNCE_W_0_FT1` (FF D, AX)
  and `IMUX_W11` (LUT input, B1), 0 overuse. Base: FAIL, index out
  of bounds (nets empty). Fix: pass.
- `port_driven_net_fails_loud_when_no_unused_lut_letter`: all 8 LUT
  letters placed, so no O pin is free. Expects Err naming din. Base:
  FAIL (returns Ok, silently drops). Fix: pass.

Fail-before method: router tests by `git stash` of `router.rs` only
(W1) and by a one-token revert (`&protected` to `&HashSet::new()`)
for W8; ultra tests by reverting the port-net branch to base
`continue` with the fix saved as `fix-ultra.patch`, then restoring
from the patch. See nit N61 for a stash incident during the W8
fail-before.

## 7. Suites (coordinator, merged branch, serial, -j1)

| suite | result | log |
| --- | --- | --- |
| helion-ultrascale (debug) | exit 0, 462 passed / 0 failed / 1 ignored, 35 test-result lines | `coord/suite-ultrascale.log` |
| helion-cli (release) | exit 0, 92 passed / 0 failed / 1 ignored, 11 test-result lines | `coord/suite-cli-release.log` |
| US-09 checkers | exit 0, Ran 100 tests, OK, 116.004s, oss-cad-suite iverilog 14.0 | `coord/checkers-us09.log` |

Ultrascale is base 460/0/1 plus exactly the 2 new router tests
(462/0/1); the impl's own run showed 461/0/1 before the W8
test-only commit, and the lib re-run after it is 260/0, so the tip
total is 462/0/1 by construction. CLI is base 89/0/1 plus exactly
the 3 new ultra tests (92/0/1). All 5 new tests pass by name in
these logs. The two pre-existing E0133 warnings in the perf_profile
example (nit N52) are unchanged. System iverilog 12.0 was present
for the baseline checkers run; oss-cad-suite 14.0 was used for the
impl and coordinator checkers.

## 8. Regressions (coordinator, merged branch)

Flows script `coord/run-coord-flows.sh`; verify-bits script
`coord/run-coord-verify.sh` (branch-built decoder, oss-cad-suite
iverilog 14.0 on PATH), summary `coord/coord-verify-summary.txt`.

| design | flow (route-to-pip PIPs, WNS) | stadin (PIPs) | verify-bit |
| --- | --- | --- | --- |
| counter12 seed 0 | 752 PIPs, WNS 7370 | 804 PIPs | PASS, 400 cycles, 800/0 |
| counter8 seed 0 | 363 PIPs, WNS 8570 | 377 PIPs | PASS, 400 cycles, 800/0 |
| counter8 seed 1 | 342 PIPs, WNS 8570 | 339 PIPs | PASS, 400 cycles, 800/0 |
| counters2x4 | 307 PIPs, WNS 8450 | 320 PIPs | PASS, 400 cycles, 800/0 |
| counter5 | 82 PIPs, WNS 9650 | 76 PIPs | PASS, 400 cycles, 800/0 |
| counter | 58 PIPs, WNS 9650 | 58 PIPs | PASS, 400 cycles, 800/0 |
| blinky | 4 PIPs, WNS 9650 | 4 PIPs | PASS, 400 cycles, 800/0 |
| counter16 seed 1 | 1278 PIPs, WNS 5750 | 1244 PIPs | PASS, 40000 cycles, 80000/0 |

Blinky `.bit` md5 `d82b70c8e29ce72f28e08c94f3a16d16`, unchanged from
the stadin lane and the baseline. Every flow number matches the
impl worker's runs exactly (same PIPs, same WNS, same md5s).

PIP/iteration deltas vs stadin are legitimate consequences of the
fix, not regressions: no regression flow routes a port net (zero
`ultra route port net=` lines in any regression flow log; the
counter designs have no routable non-clock input port), so every
delta comes from the BYPASS protection alone. Forbidding transit
through BYPASS sink wires removes the sharing conflicts that drove
rip-up iterations (counter16 falls 37 to 15 iterations, counter12
20 to 18, counter8-s0 18 to 13), and the detours change PIP counts
in both directions (counter12 -52, counter8-s0 -14, counters2x4
-13, counter8-s1 +3, counter5 +6, counter16 +34; counter and
blinky unchanged). WNS is unchanged on every design, and all 8
verify-bits pass on the new routes.

## 9. Stash proof (coordinator)

fix.patch (`coord/fix.patch`, sha256
`e24927fd91597fa0cf27e977d37bff4a4b5c9a7457fbd029df09d6f9d0d3dea2`)
is byte-identical to the branch diff (cmp clean, 3 files, +358/-4).

Base side (baseline worktree binary, base `be1a3178`): chain16 flow
exit 0, md5 `e7cc453a`; shift32 flow exit 0, md5 `e7f0e75c`.
Chain16 verify-bit with the stadin F1 map: FAIL, 800 comparisons,
403 mismatches, first mismatch cycle 19 (log
`coord/stash-base-c16-f1map-verify.log`), the verbatim N58 symptom.
Chain16 verify-bit with the H_O map: FAIL at route (undriven sink
`SLICE_X4Y0.F1`) and extract (`SLICE_X4Y0.F1 (FLUT input) is
undriven`), the same gap at an earlier stage. Shift32 verify-bit
with the A_O map: FAIL, 800 comparisons, 737 mismatches, 737 net
unknowns, first mismatch cycle 31 (log
`coord/stash-base-s32-verify.log`).

Restore side (fix.patch applied to the baseline worktree, rebuilt):
fix.patch applies clean (`git apply --check`, no output); the
regenerated diff is byte-identical to fix.patch (cmp clean, same
sha256 `e24927fd`). Restore build exit 0. Chain16 flow exit 0, md5
`4cf99124`, verify-bit PASS 800/0/0/0; shift32 flow exit 0, md5
`a7a070dd`, verify-bit PASS 800/0/0/0. Both md5s byte-identical to
the impl fix runs and the coordinator merged-branch runs. The
baseline worktree was reverted to clean afterward (git status
empty).

## 10. Honest limits

- Proven on the measured design set: chain16 and shift32 (the two
  gated designs) plus the 8 counter/blinky regressions. Larger
  netlists with other top-level-input shapes are unmeasured.
- Port entry needs a free LUT letter in a sink slice; a design whose
  sink slices are all full fails loud by design. The general
  solution is IOB placement (blocked on B1), not a wider entry
  search.
- The entry consumes an unused LUT O pin (its O wire is occupied by
  the port). Placement is unaffected (the letter holds no LUT), but a
  design that needs that LUT letter for logic cannot also use it as
  a port entry; `port_entry` then falls back to the next sink slice.
- `_I` pin feeds (e.g. BYPASS_W14/G_I) stay transit-eligible: no
  routed net sinks there, and the checker models no logic from them.
  If a future flow connects `_I` pins, they become protected
  automatically, because protection is derived from routed sinks.
- The OOM kills on this shared 6 GiB cell (baseline: 4 checkers
  discover attempts; impl: 2 checkers attempts, 1 regression flow;
  all re-ran cleanly, no test failed) are box pressure, not test or
  flow failures; every killed run re-ran cleanly. Keep heavy runs
  serial. See nit N60.
- Stash incident (process, no code impact): during the W8
  fail-before the impl worker ran `git stash pop` after a `git stash
  push` that had saved nothing (the fix was already committed), and
  the pop applied a foreign stash entry (`stash@{0}: On
  wip/tcl-console: cross-probe: set aside sibling agent in-flight
  work (vcd power, constraints wizard)`). Git stash is shared across
  worktrees of one repo. The worker aborted the pop with `git reset
  --merge`, removed the one untracked file it had restored
  (`crates/helion-gui/src/constraints_wizard.rs`), verified the
  foreign stash entry is intact, verified the branch is clean, and
  verified the committed fix files are byte-identical to `c4aab79f`.
  The coordinator re-verified: the foreign stash entry is intact at
  `stash@{0}`, `constraints_wizard.rs` is absent from the impl
  worktree, the impl worktree is clean, and the branch diff is
  exactly the 3 fix files. No foreign files remain and no foreign
  commits were made. See nit N61.

## 11. New nits (numbered after N59)

- N60 (process): the US-09 checkers full-discover run is OOM-fragile
  on the shared 6 GiB cell: 2 kills in the impl lane, 4 in the
  baseline lane, all at 65-113 dots with no failure lines. The
  per-module fallback (baseline) and a verbose re-run with nothing
  else running (impl, coordinator) all land the same 100 OK.
  Consider making the per-module run the default on small cells.
- N61 (process): `git stash` is shared across worktrees of one repo;
  a stash pop in a lane worktree can apply a sibling lane's stash.
  Lane fail-before procedures should use a targeted file checkout
  (`git checkout -- <file>`) instead of stash once the fix is
  committed. No code impact; see section 10.
- N62 (tooling, non-blocking): the flow prints no per-net PIP list,
  so tracing a port net's PIPs required a throwaway script
  (`trace-din.py`, kept in `~/workspace/n58-logs/impl/`). A route
  debug dump naming each net's PIPs would make the next lane's grep
  evidence trivial.
- Carried, untouched by this lane: N51, N53, N55, N59, IOB blocked
  on B1, N52 (pre-existing E0133 warnings), N54 (binary bytes depend
  on the worktree, confirmed again: coordinator helion 16 bytes
  smaller than the impl build, same command, different worktree;
  helion-bitgen same size, different md5), N56 (pre-existing rustfmt
  deviations, untouched), N16 parse-side open, N9 block types 2+
  untested, N34 stale default decoder, N37, N38, INT_INTF tile model
  for DSP/BRAM routing, known-INIT Vivado datum, documented xczu3eg
  timing, remaining 54,907 non-band unexplained bits, flags A-CTRL
  and A-F7F8 still unresolved, need a CTO ruling. N58 CLOSED by this
  lane, pending Critic Session 47. No critic dispatched by this lane;
  the main agent dispatches Session 47.
