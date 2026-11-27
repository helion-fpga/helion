# N51/N53/N59/N63 doc/format fix lane

Date: 2026-11-27
Branch: `fm/ultra-bitgen-n51n53n59n63fix`
Base: `fm/ultra-bitgen-n65fix` at `bd3f1fe7454ea6f39426b11ec05d5ff6a36d87c4`
  (verified by `git rev-parse` in the n65fix worktree and `git ls-remote
  origin` before branching; the n65fix worktree was clean)
Worktree: `~/workspace/helion-ultra-bitgen-n51n53n59n63fix`
Status: all four nits fixed, one fix per commit, pushed to origin only.

Scope: doc comments and formatting only in
`crates/helion-cli/src/ultra.rs`, plus regeneration of one coord summary
by its own generating script. No logic touched: no N7/N8/N9/Q code, no
decoder behavior, no router/packer logic, no N34/N37/N38 checks. No
merges. No critic dispatched. N73 untouched (observation only). The N9
worktree and the `fm/ultra-bitgen-n9fix` branch untouched. No em dashes
in the report, the commit messages, or the touched code.

## Commits (4, one fix per commit)

- `394f418d` n51fix: correct clock_leg_forbidden_pips doc to the 3-PIP
  west-side set
- `313ebbc4` n53fix: packer comment no longer claims the flow cannot
  drive CLK2
- `6fefabb1` n63fix: rustfmt the 3 N58 deviations in ultra.rs
- `e2328ced` n59fix: regenerate coord-flow-summary.txt by re-running
  run-coord-flows.sh (empty commit: the regenerated file lives outside
  this repo, so the commit records the re-run)

Branch diff vs base: 1 file changed,
`crates/helion-cli/src/ultra.rs`, 26 insertions, 22 deletions.

## N51: stale forbidden-PIP count in the clock_leg_forbidden_pips doc

Control: the doc comment above `clock_leg_forbidden_pips` at
`ultra.rs:1258` said, verbatim, "this yields exactly one PIP for
CTRL_W5 ... and none for CTRL_W4". The unit test
`clock_leg_forbidden_pips_find_sdq80_and_vcc_conflicts` asserts the
computed west-side set is exactly 3 PIPs:

- `INT_X1Y0`, `INT_NODE_SDQ_80_INT_OUT1.LOGIC_OUTS_W22` (sets bit 21_35;
  every CTRL_W5 leg requires it clear)
- `INT_X1Y0`, `CTRL_W5.VCC_WIRE`
- `INT_X3Y0`, `CTRL_W4.VCC_WIRE`

Fix: rewrote the sentence so the comment states the 3-PIP set above.
Doc only, no behavior change.

Proof: `grep "this yields exactly one PIP for CTRL_W5"` on ultra.rs
returns no match; the new text names all three PIPs at lines 1258-1266;
the commit diff is 10 insertions, 6 deletions, comment only.

## N53: stale "cannot drive CLK2" packer comment

Control: the packer comment at `ultra.rs:1666` said "its slice's CLK2
driven, and this flow cannot drive CLK2: clock routing is phase 2, no
INT PIP chain connects the clk primary (CTRL_W4) to CLK2 (CTRL_W5),
and the GCLK spine is undriven in the checker model, so an EFGH FF
fails verify-bit on place even when everything else routes". Since the
CLK2 lane the flow does drive CLK2.

Fix: rewrote the comment to say EFGH letters are preferred last
because a CLK2 leg costs a second clock-leg slot on the tile, not
because CLK2 cannot be driven. Doc only, no behavior change.

Proof: `grep "this flow cannot drive CLK2"` on ultra.rs returns no
match; the new text states the second clock-leg slot reason; the
commit diff is 9 insertions, 11 deletions, comment only.

## N59: stale exit=1 rows in the coord flow summary, fixed by re-running the generator

Control: `~/workspace/stadin-logs/coord/coord-flow-summary.txt` (dated
Nov 24 14:56) carried stale `exit=1` rows for blinky and counter12 at
lines 5-6, though the re-run bits, flow logs (14:57-14:58), and verify
summary (all exit=0) postdate it and all pass. The summary file was
never hand-edited.

Prerequisites found missing and restored before the re-run:

1. The script's release binary was missing: the stadin worktree
   `~/workspace/helion-ultra-bitgen-stadin` (branch
   `fm/ultra-bitgen-stadin`, HEAD `6794274a`) had no `target/` at all.
   Rebuilt with `cargo build --release -p helion-cli`: 9m 15s, exit 0,
   binary at `target/release/helion`, build log at
   `~/workspace/stadin-logs/coord/build-release-n59rebuild.log`.
2. The `blinky.sv` and `counter12.sv` inputs were missing from
   `~/workspace/intcols-logs/impl/`. Both were restored from copies
   proven identical by trial runs of the rebuilt binary against the
   Nov 24 artifacts before any restore:
   - `blinky.sv` from the stadin worktree `examples/blinky.sv`
     (md5 `f23d06ceec548946d75a07d2d14abac6`): trial run produced a
     byte-identical `.bit` and `.fasm` to Nov 24 `coord-blinky.bit`
     and `coord-blinky.fasm`, synth 1 LUT, 1 flop.
   - `counter12.sv` from `~/workspace/critic42-runs/counter12.sv`
     (md5 `a4fc2d26e14f3d1da3cfda2c311f53a8`): trial run produced a
     byte-identical `.bit` and `.fasm` to Nov 24 `coord-c12-s0.bit`
     and `coord-c12-s0.fasm`, synth 21 LUTs, 12 flops, WNS_PS=7370
     in both.

Fix: ran the generating script `run-coord-flows.sh` with stdout
captured to `coord-flow-summary.txt`, exactly as the file was produced
before. The script exited 0.

Proof: the regenerated summary shows all 7 flows at exit=0:

- `coord-c16: exit=0 chain16 WNS_PS=8570 route-to-pip pips=341`
- `coord-c16-r2: exit=0 chain16 WNS_PS=8570 route-to-pip pips=341`
- `coord-s32: exit=0 shift32 WNS_PS=8690 route-to-pip pips=280`
- `coord-s32-r2: exit=0 shift32 WNS_PS=8690 route-to-pip pips=280`
- `coord-blinky: exit=0 blinky WNS_PS=9650 route-to-pip pips=4`
- `coord-c12-s0: exit=0 counter12 WNS_PS=7370 route-to-pip pips=804`
- `coord-c16-s1: exit=0 counter16 WNS_PS=5750 route-to-pip pips=1244`

All 36 regenerated artifacts (12 `.bit`, 12 `.fasm`, 12
`flow-coord-*.log`) are md5-identical to the pre-run snapshot taken
before the script ran. The pre-run summary is kept at
`coord-flow-summary.txt.before-n59`
(md5 `d485710be1c0dd0dc3262d34c0f924e3`).

## N63: 3 rustfmt deviations in new N58 code

Control: `~/.cargo/bin/rustfmt --edition 2021 --check
crates/helion-cli/src/ultra.rs` (rustfmt 1.8.0-stable) reported 24
diff hunks on the base, at lines 32, 42, 514, 564, 614, 1130, 1198,
1338, 1365, 1594, 1641, 1746, 1816, 1841, 2558, 2565, 2740, 2756,
2918, 2963, 3123, 3271, 3461, 3469. The 3 N58 spots were the hunks at
1841 (`port_entry` `let mut ordered` binding), 2558 (UltraSite
struct literal in `port_driven_net_routes_on_int_tile`), and 2565
(`route_ultra` expect call). After the N51/N53 doc edits the count
was still 24, with the 3 spots at 1843, 2560, 2567.

Fix: applied rustfmt's suggested formatting to exactly those 3 spots
(one-line `let mut ordered` binding, expanded UltraSite literal,
one-line `route_ultra(...).expect(...)`). No other spot touched;
rustfmt was never run over the whole file.

Proof: `rustfmt --edition 2021 --check` now reports 21 hunks, a drop
of exactly 3. The hunks at 1843, 2560, 2567 are gone. The remaining
21 are the pre-existing set with line numbers shifted only by the
edit deltas, and no new hunk appears anywhere. The commit diff is 7
insertions, 5 deletions, formatting only.

## Whole-lane proof: cargo test -p helion-cli

All runs are my own, in the lane worktree, debug profile unless noted.
`cargo test -p helion-cli` (no tileconn env set): 48 tests, 46 passed,
2 failed. The 2 failures are the N38 flow tests
(`blinky_flow_emits_bitstream_on_xczu3eg`,
`counter_flow_emits_bitstream_on_xczu3eg`), which fail loud by design
when `HELION_TILECONN_DB_ROOT` is unset.

`cargo test -p helion-cli` with `HELION_TILECONN_DB_ROOT` and
`HELION_TILECONN_PART=xczu3eg-sbva484-1-e` set: 48 tests, 45 passed,
3 failed:

- `counter_flow_emits_bitstream_on_xczu3eg`: pre-existing failure, see
  below.
- `blinky_stops_at_route_without_pin_wire_map` and
  `counter_stops_at_route_without_pin_wire_map`: these assert the flow
  stops at ROUTE when no pin-wire map exists, so they fail when the
  env is set and the map exists (`unwrap_err()` on an `Ok` value).
  They pass in the no-env run. This is their designed mode split,
  not a regression.

Behavior-unchanged check for the counter flow test: run alone with
`--test-threads=1` and the tileconn env set, it fails on this branch
with `PIP on an INT tile other than INT_X1Y0:
INT_X2Y0.PIP.INT_NODE_SDQ_84_INT_OUT0.EE2_W_END6` at ultra.rs:3365.
The same single test on the unmodified base `bd3f1fe7` (scratch
worktree, same env, same invocation) fails with the identical panic
at ultra.rs:3361; the 4-line shift is exactly the net line delta of
the N51/N53/N63 edits. The sibling `blinky_flow_emits_bitstream_on_xczu3eg`
passes on the base and passes on this branch. The branch diff is
comments and whitespace only, so the routed PIP set cannot differ.

## Pre-existing failure found during proof (out of scope for this lane)

`counter_flow_emits_bitstream_on_xczu3eg` fails on the base tip: the
router emits a PIP on `INT_X2Y0`
(`INT_X2Y0.PIP.INT_NODE_SDQ_84_INT_OUT0.EE2_W_END6`) while the test
asserts every PIP line starts with `INT_X1Y0.`. The flow itself
completes (bitgen ok, 58 PIPs). This needs a router or test owner;
fixing it would be a logic change, which this doc/format lane must
not make. Flagged here for the driver to route.

## Files and evidence

- Crew copy of this report:
  `~/workspace/ultra-bitgen-crew/N51N53N59N63FIX_REPORT.md`
- Branch copy (byte-identical, verified with `cmp` and sha256):
  `tools/n51n53n59n63fix/N51N53N59N63FIX_REPORT.md` on
  `fm/ultra-bitgen-n51n53n59n63fix`
- Regenerated summary:
  `~/workspace/stadin-logs/coord/coord-flow-summary.txt`
- Pre-run summary backup:
  `~/workspace/stadin-logs/coord/coord-flow-summary.txt.before-n59`
- Release rebuild log:
  `~/workspace/stadin-logs/coord/build-release-n59rebuild.log`
- Restored inputs: `~/workspace/intcols-logs/impl/blinky.sv`,
  `~/workspace/intcols-logs/impl/counter12.sv`
- Test logs (ephemeral, under /tmp): `n51-lane-cargo-test.log`
  (no-env full run), `n51-lane-cargo-test-env.log` (env full run),
  `n51-lane-counter-alone.log` (branch, alone),
  `n51-base-counter-alone.log` (base, alone),
  `n51-base-blinky-alone.log` (base, alone)
- rustfmt outputs (ephemeral, under /tmp): `rustfmt-before.txt`
  (24 hunks), `rustfmt-after-doc.txt` (24 hunks),
  `rustfmt-after-n63.txt` (21 hunks)
