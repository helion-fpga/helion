# N38 fix report: flow tests fail loud when the tileconn env is unset

Date: Fri 2026-11-27 (runs 02:30 to 03:13 EST)
Worker: N38 fix worker
Branch: `fm/ultra-bitgen-n38fix`, worktree `~/workspace/helion-ultra-bitgen-n38fix`
Base: `fm/ultra-bitgen-n37fix` at `f30745efcc5126a77443873977f982a488ed11f2`
Status: fix complete and proven for the N38 defect itself. One separate,
pre-existing base failure found during proof (counter flow test, see section 5).
No merges. No critic dispatch.

## 1. Ground truth and base verification

N38 per CRITIQUE.md line 5919 (Critic Session 36): the helion-cli suite total
(73/0/1) counts `counter_flow_emits_bitstream_on_xczu3eg` as passed when it
skips with the tileconn env unset, so the suite number alone is not evidence
the flow test executed. The lane did run it separately with the env set, so
this is reporting hygiene, not a lane defect. Non-blocking.

Base verified before branching:

- `git rev-parse fm/ultra-bitgen-n37fix` in `~/workspace/helion`:
  `f30745efcc5126a77443873977f982a488ed11f2`
- `git ls-remote origin fm/ultra-bitgen-n37fix`:
  `f30745efcc5126a77443873977f982a488ed11f2`
- Local and origin tips match, so the branch was created from the current tip.
- New branch `fm/ultra-bitgen-n38fix` created in worktree
  `~/workspace/helion-ultra-bitgen-n38fix` at `f30745ef`, clean status.
- First `git worktree add` attempt failed only because `fm/ultra-bitgen-n37fix`
  is already checked out in `~/workspace/helion-ultra-bitgen-n37fix`; retried
  with `git worktree add -b fm/ultra-bitgen-n38fix <path> f30745ef`, which
  succeeded. No execution-risk heuristic tripped.

Mechanism re-verified in the code before changing anything, on the base:

- `crates/helion-cli/src/ultra.rs:3268`, `fn counter_flow_emits_bitstream_on_xczu3eg`,
  has three early `return` skip paths (lines 3272, 3279, 3286) that print
  `counter_flow_emits_bitstream_on_xczu3eg: skipping, ...` and return, so the
  test passes without executing when `HELION_TILECONN_DB_ROOT` or
  `HELION_TILECONN_PART` is unset or the root dir is absent.
- Sibling `fn blinky_flow_emits_bitstream_on_xczu3eg` (line 3124) has the
  identical three skip paths (lines 3126-3142). Both were treated the same way.
- No other `skipping` test exists in `crates/helion-cli/`; the only other match
  is an unrelated multiseed doc comment at line 2264.

## 2. Control: base, env unset, full helion-cli suite

Command (run in the new worktree while it was still unmodified base code,
via `nohup setsid`, log `~/workspace/ultra-bitgen-crew/logs/n38fix-control-suite.log`):

```
env -u HELION_TILECONN_DB_ROOT -u HELION_TILECONN_PART /home/hatch/.cargo/bin/cargo test --release -j2 -p helion-cli
```

Result: 92 passed, 0 failed, 1 ignored across 11 test binaries, including:

```
test ultra::tests::blinky_flow_emits_bitstream_on_xczu3eg ... ok
test ultra::tests::counter_flow_emits_bitstream_on_xczu3eg ... ok
```

Both flow tests report `ok` while the tileconn env is unset, so both skipped
without executing and were still counted as passed. This reproduces N38.

Note on totals: the critic's 73/0/1 came from their run; this full
`-p helion-cli` control run totals 92 passed, 0 failed, 1 ignored (bin
`helion` target alone: 48 passed, 0 failed). The difference is run scope, not
a contradiction: in every scope the two flow tests count as passed while
skipping.

## 3. Fix

File changed: `crates/helion-cli/src/ultra.rs` only (35 insertions, 27 deletions).
Choice: fail loud naming the missing env, the smallest change that makes the
suite number honest evidence of whether the flow test executed. No skipped
accounting exists in the Rust test harness, and marking the tests `#[ignore]`
would have stopped them running by default even with the env set, so fail-loud
is the honest option.

In both `blinky_flow_emits_bitstream_on_xczu3eg` and
`counter_flow_emits_bitstream_on_xczu3eg`, each of the three skip paths now
panics instead of printing and returning:

- `HELION_TILECONN_DB_ROOT` unset: panic names the var, states the flow test
  must run, not skip (N38), and prints the exact command to run with the env set.
- `HELION_TILECONN_DB_ROOT` dir absent: panic names the var and the absent path.
- `HELION_TILECONN_PART` unset: panic names the var (zynqusp holds two part
  dirs) and prints the exact command to run.

Doc comments updated to match: blinky no longer says "it skips", it says it
fails loud (N38); counter notes the same env gate "fails loud when the env is
unset, N38".

Not changed: no bitstream assertion, no test expectation, no decoder, N7/N8/N9/Q
code, or `tools/us09v/us09v/verify.py` touched. No check weakened. The
`assert_eq!` on `HELION_TILECONN_PART`, the stable FASM lines, the PIP
assertions, the band checks, and the decoder round-trip are all byte-identical
to the base. `git status` shows only `crates/helion-cli/src/ultra.rs`
modified (plus this report under `tools/n38fix/`).

Note: `rustfmt --check` on the base file already reports diffs (system rustfmt
version differs from the project's pinned style), so the file was NOT
reformatted; the new code is hand-formatted to match the surrounding style.

## 4. Proof, env unset: skip is no longer counted as passed

Command (log `~/workspace/ultra-bitgen-crew/logs/n38fix-proof-unset.log`):

```
env -u HELION_TILECONN_DB_ROOT -u HELION_TILECONN_PART /home/hatch/.cargo/bin/cargo test --release -j2 -p helion-cli --bin helion flow_emits_bitstream_on_xczu3eg
```

Result:

```
test ultra::tests::blinky_flow_emits_bitstream_on_xczu3eg ... FAILED
test ultra::tests::counter_flow_emits_bitstream_on_xczu3eg ... FAILED
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 46 filtered out; finished in 0.00s
```

Panic text shown in the log, for example:

```
blinky_flow_emits_bitstream_on_xczu3eg: HELION_TILECONN_DB_ROOT is unset; the flow test must run, not skip (N38). Run with HELION_TILECONN_DB_ROOT=~/workspace/prjuray-db-ref/zynqusp HELION_TILECONN_PART=xczu3eg-sbva484-1-e cargo test --release -j1 -p helion-cli blinky_flow_emits_bitstream_on_xczu3eg
```

Full bin suite with env unset (log
`~/workspace/ultra-bitgen-crew/logs/n38fix-proof-unset-bin.log`):

```
env -u HELION_TILECONN_DB_ROOT -u HELION_TILECONN_PART /home/hatch/.cargo/bin/cargo test --release -j2 -p helion-cli --bin helion
test result: FAILED. 46 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 44.05s
```

Exit code 101. Control on the base was 48 passed, 0 failed for this binary;
after the fix the same binary is 46 passed, 2 failed, so the suite total no
longer counts either skipped flow test as passed.

## 5. Proof, env set: flow tests execute; blinky passes, counter hits a pre-existing base failure

Command (log `~/workspace/ultra-bitgen-crew/logs/n38fix-proof-set.log`):

```
env HELION_TILECONN_DB_ROOT=/home/hatch/workspace/prjuray-db-ref/zynqusp HELION_TILECONN_PART=xczu3eg-sbva484-1-e /home/hatch/.cargo/bin/cargo test --release -j2 -p helion-cli --bin helion flow_emits_bitstream_on_xczu3eg
```

Result:

```
test ultra::tests::blinky_flow_emits_bitstream_on_xczu3eg ... ok
test ultra::tests::counter_flow_emits_bitstream_on_xczu3eg ... FAILED
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 46 filtered out; finished in 10.91s
```

Blinky executes for real and passes. Counter executes for real (synth, pack,
place, route, bitgen all run: 5,982,532-byte bitstream, 58 PIPs) and then fails
at an assertion the N38 fix did not touch:

```
thread 'ultra::tests::counter_flow_emits_bitstream_on_xczu3eg' panicked at crates/helion-cli/src/ultra.rs:3361:17:
PIP on an INT tile other than INT_X1Y0: INT_X2Y0.PIP.INT_NODE_SDQ_84_INT_OUT0.EE2_W_END6
```

The test asserts every PIP is on INT_X1Y0, but the router routes one PIP via
INT_X2Y0. A retry of the counter test alone in the fixed worktree failed the
same way, same PIP, same 58 PIP count, so this is deterministic on this base,
not a flake in this run.

This failure is pre-existing on the base, not caused by the N38 change:

- The N38 diff touches only the env-gate branches, which are bypassed entirely
  when the env is set, plus doc comments. The failing assert at line 3361 and
  the router are untouched.
- History: the counter test was introduced in 28aa4c70 with the INT_X1Y0-only
  assertion; the routepack change 301d2225 ("route wider designs through
  INT_X2+") is an ancestor of the base `f30745ef` and postdates the test, so
  the router on this base can legitimately route via INT_X2Y0 while the test
  still forbids it.
- Direct base control: the counter test was run on the UNMODIFIED base in
  `~/workspace/helion-ultra-bitgen-n37fix` with the env set (log
  `~/workspace/ultra-bitgen-crew/logs/n38fix-base-counter-set.log`):

```
env HELION_TILECONN_DB_ROOT=/home/hatch/workspace/prjuray-db-ref/zynqusp HELION_TILECONN_PART=xczu3eg-sbva484-1-e /home/hatch/.cargo/bin/cargo test --release -j2 -p helion-cli --bin helion counter_flow_emits_bitstream_on_xczu3eg
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 47 filtered out; finished in 6.57s
thread 'ultra::tests::counter_flow_emits_bitstream_on_xczu3eg' panicked at crates/helion-cli/src/ultra.rs:3353:17:
PIP on an INT tile other than INT_X1Y0: INT_X2Y0.PIP.INT_NODE_SDQ_84_INT_OUT0.EE2_W_END6
```

Identical PIP, identical failure, on unmodified base code (line 3353 there
vs 3361 here is only the N38 lines added above it). The counter INT_X1Y0
assertion failure is therefore a separate, pre-existing base defect. It is
out of scope for N38 and was NOT fixed here; it needs its own lane (either the
test's INT_X1Y0-only assertion or the router's INT_X2Y0 use for this design,
a CTO call). N38 itself is fixed: the suite can no longer report the flow
tests as passed when they never ran, and with the env set both tests do run.

## 6. Scope and guardrails

- Touched: `crates/helion-cli/src/ultra.rs` (env gates + doc comments only)
  and this report.
- NOT touched: N7/N8/N9/Q code, decoder behavior, `tools/us09v/us09v/verify.py`
  (N34/N37 checks), the N9 worktree `~/workspace/helion-ultra-bitgen-n9fix`,
  the `fm/ultra-bitgen-n9fix` branch, any ZedBoard code.
- No merges into `fm/ultra-bitstream`, `fm/zedboard-merge`, or master. No
  critic dispatched.
- Build rules followed: all cargo runs `-j2` max via `nohup setsid`, logs under
  `~/workspace/ultra-bitgen-crew/logs/`, never `/tmp`; no target dir cleaned.
  (One early control attempt failed at launch because `cargo` is not on PATH
  in the `setsid` context; relaunched with the absolute path
  `/home/hatch/.cargo/bin/cargo`. No other deviation.)
- Warnings in the build logs are pre-existing in other crates (helion-ir,
  helion-route, helion-sv, helion-sta, and others); the helion-cli bin test
  target shows the same 2 warnings as on the base control run. No new warning
  introduced by this change.

## 7. Branch and report copies

- Commit on `fm/ultra-bitgen-n38fix`: `crates/helion-cli/src/ultra.rs`
  plus `tools/n38fix/N38FIX_REPORT.md`. Commit hash is in the driver's final
  report to the parent agent.
- Report copies: `~/workspace/ultra-bitgen-crew/N38FIX_REPORT.md` (this file)
  and `tools/n38fix/N38FIX_REPORT.md` in the worktree, verified byte-identical
  with `cmp` and `sha256sum` before pushing.
- Push: `git push origin fm/ultra-bitgen-n38fix` only. No merges.
