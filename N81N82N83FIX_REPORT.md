# N81/N82/N83 fix report

Branch: `fm/ultra-bitgen-n81n82n83fix`
Worktree: `~/workspace/helion-ultra-bitgen-n81n82n83fix`
Base: `fm/ultra-bitgen-n79n80fix` at `6b9a841f`, verified before work:
clean tree, HEAD `6b9a841f`, `git ls-remote origin
fm/ultra-bitgen-n81n82n83fix` empty (not pushed). No merges, no
worktree or branch created by this lane (both pre-existed, verified
by the driver). No COORD.md or CRITIQUE.md edits. No Rust logic, no
N7/N8/N9/Q code, no decoder behavior, no N34/N37/N38 checks touched.
The N9 worktree and branch were not touched. The counter INT_X2Y0
defect was not touched (test vs router ruling belongs to the CTO).

All three nits are from Critic Session 56, section 12
(CRITIQUE.md lines 7376-7382), confirmed real by the critic's own
runs. One fix per commit:

- `78c7fdc1` n81fix: split CLI phase so flow tests run with tileconn env set
- `306b1a85` n82fix: correct per-bit rank frame count 3,798 to 3,863 in band_code doc
- `6ec7f183` n83fix: correct two N79N80 report claims against their evidence files

## N81: CLI phase split in run-suites.sh

The N38 fix makes the two flow tests panic when
`HELION_TILECONN_DB_ROOT` is unset, and `run-suites.sh` unset it
before the CLI suite (line 528), so the canonical run exited 1 by
design: the critic's tip run gave the CLI bin target 46 passed / 2
failed / 0 ignored, both failures the N38 panics. The fix splits
the CLI phase in `tools/ultra-bitgen/run-suites.sh` only:

- Bulk CLI suite runs env-unset, as before, with the two flow tests
  skipped (`--skip blinky_flow_emits_bitstream_on_xczu3eg --skip
  counter_flow_emits_bitstream_on_xczu3eg`), so the
  `*_stops_at_route_without_pin_wire_map` tests stay in the mode
  they assume. The skip flags are passed after `--`, so they apply
  to every test binary in the package.
- The two flow tests then run env-set, via `--bin helion` with the
  name filter `flow_emits_bitstream_on_xczu3eg`, which matches
  exactly those two tests. `--bin helion` matters: helion-cli is
  bin-only (mod ultra lives in `src/main.rs`, no lib.rs; bin name
  helion), so a `--lib` run would pass vacuously on zero tests.
- The flow phase logs to `suite-cli-flow.log` and reports a
  `cli-flow exit=...` summary line in the same format as the other
  phases. Summary format, log locations, prune behavior, ultrascale
  phase, and checkers phase are unchanged.

## N82: band_code.rs frame count

`crates/helion-ultrascale/src/band_code.rs` line 216 said per-bit
rank 40 to 44 on 3,798 frames; the measure3 histogram sums to 3,863
(147 + 46 + 19 + 40 + 3611), and line 228 already says 3,863. The
fix changes 3,798 to 3,863 on line 216 and nothing else. `grep
3,798` on the file now returns nothing.

## N83: N79N80 report corrections

Two claims in `N79N80FIX_REPORT.md` did not match its own evidence
files under `~/workspace/ultra-bitgen-crew/logs/n79n80fix/`.
Corrections only; conclusions stand. Both copies on this branch
(root and `tools/n79n80fix/`) are corrected and byte-identical
(`cmp` clean). The crew-dir copy is the reference the critic read
and was not edited.

1. Kill-after2 did not start from a fresh 4,000-orphan scratch
   tree. Its kill-result.txt line 3 says "orphan dirs before:
   3980", the leftover of kill-after (4,001 before minus 21
   deleted), so kill-after2 reused the kill-after tree. The report
   now says so plainly. The trap proof itself holds (tmp dir gone,
   exit 143 on both fixed runs); the kill table numbers (21 /
   3,980 and 30 / 3,950) were already correct and are unchanged.
2. The after timing list omitted the first after run at 262.38s
   (`after/timings.txt` line 5, "TIMING prune phase total:
   262.38s"). Including it, the four after runs are 262.38, 99.59,
   95.95, 60.98, median 97.77s against the control mean 162.0s,
   still about 40% faster (39.6%, about 64s saved, 1.7x), so the
   headline conclusion stands. The report now lists all four runs
   with median 97.77s; the directly-dependent sentences (R3 range
   61s to 262s, R6, the `after/` log entry) are updated to match.
   No other report content changed.

## Proof

Logs under `~/workspace/ultra-bitgen-crew/logs/n81n82n83fix/`.
Proof chain 1 (`proof.sh`, `proof-progress.log`): build the release
binaries, run the old CLI phase as control, run the new bulk and
flow phases, run the band_code tests, then run the canonical
runner. Proof chain 2 (`proof2.sh`, `proof2-progress.log`): build
the decoder binary, re-run the control cleanly, run the canonical
runner again.

### N81: CLI phase split

Control, old CLI phase, env unset, same command the unmodified
script runs (`cargo test --release -j1 -p helion-cli`,
`cli-control-unset2.log`): exit 101, bin target 46 passed / 2
failed / 0 ignored, both failures the N38 fail-loud panics
(blinky at ultra.rs:3132, counter at ultra.rs:3280, each naming
the missing HELION_TILECONN_DB_ROOT and the command to run with
it set). This matches the critic's tip canonical run exactly.

New bulk phase, env unset, flow tests skipped
(`cli-bulk-unset.log`): exit 0, totals 90 passed / 0 failed / 1
ignored across all CLI test binaries. Bin target: 46 passed / 0
failed / 0 ignored, 2 filtered out (the two flow tests skipped).
Both `*_stops_at_route_without_pin_wire_map` tests pass.

New flow phase, env set, `--bin helion
flow_emits_bitstream_on_xczu3eg` (`cli-flow-set.log`): exit 101,
1 passed / 1 failed / 0 ignored, 46 filtered out.
`blinky_flow_emits_bitstream_on_xczu3eg` ok.
`counter_flow_emits_bitstream_on_xczu3eg` FAILED with exactly the
pre-existing INT_X2Y0 assertion from CRITIQUE.md section 9
(panicked at ultra.rs:3365:17):

    PIP on an INT tile other than INT_X1Y0: INT_X2Y0.PIP.INT_NODE_SDQ_84_INT_OUT0.EE2_W_END6

The split changed the failure mode from the 2 N38 fail-loud
panics to the single INT_X2Y0 failure, as prescribed. The counter
flow itself completes (58 PIPs, bitstream 5,982,532 bytes, parses
back); only the INT_X1Y0-only assertion fails.

### N82: band_code tests

`cargo test -j2 -p helion-ultrascale band_code`
(`bandcode.log`): lib band_code tests 12 passed / 0 failed / 0
ignored. `grep 3,798` on `band_code.rs` returns nothing.

### N83: report corrections

Evidence quoted from the files: `kill-after2/kill-result.txt`
line 3 reads "orphan dirs before: 3980"; `after/timings.txt`
line 5 reads "TIMING prune phase total: 262.38s";
`after-target/timings.txt`, `after-target2/timings.txt`, and
`after-target3/timings.txt` read 99.59s, 95.95s, and 60.98s.
`cmp N79N80FIX_REPORT.md tools/n79n80fix/N79N80FIX_REPORT.md`
is clean (byte-identical). Median recomputed in python3:
sorted [60.98, 95.95, 99.59, 262.38], median 97.77s, 39.65%
faster than the 162.0s control mean, 64.23s saved, ratio 1.657.

### Canonical run

Canonical run `canonical2` (`~/workspace/ultra-bitgen-crew/logs/n81n82n83fix/canonical2/`, finished Nov 27 11:47 EST), run by the canonical runner on the final tip `6ec7f183`:

- ultrascale: exit 0, 481 passed, 0 failed, 0 ignored.
- cli-release: exit 0, 90 passed, 0 failed, 1 ignored.
- cli-flow: exit 101, 1 passed, 1 failed, 0 ignored.
- checkers (us09v): exit 0, 100 OK.

The single failure is `ultra::tests::counter_flow_emits_bitstream_on_xczu3eg`, which panicked at `crates/helion-cli/src/ultra.rs:3365:17` with the pre-existing INT_X2Y0 assertion:

    PIP on an INT tile other than INT_X1Y0: INT_X2Y0.PIP.INT_NODE_SDQ_84_INT_OUT0.EE2_W_END6

The counter flow itself completes (58 PIPs, bitstream 5,982,532 bytes, parses back); only the INT_X1Y0-only assertion fails. A CTO ruling on that defect is pending; it is not fixed by this lane.

Overall: SUITES FAILED, on that one pre-existing failure only. No ALL SUITES DONE is claimed.

## Honest N81 outcome

The canonical run still ends SUITES FAILED until the pre-existing
counter INT_X2Y0 defect is fixed. That is the critic's stated
expectation, not a failure of this lane. No ALL SUITES DONE is
claimed.

## Incidents

- The first proof-chain attempt used the wrong package name
  (`-p helion-bitgen`, which does not exist; the binary is
  `crates/helion-ultrascale/src/bin/helion-bitgen.rs`, package
  `helion-ultrascale`). Step 1 failed with exit 101 before
  building anything. The chain was fixed to `-p helion-ultrascale
  --bin helion-bitgen` and re-run.
- The first control run was killed by this worker mid-build
  (SIGTERM, exit 143) while stopping the chain to fix the package
  name. The control was re-run cleanly afterwards; only the
  clean re-run is cited as proof.
