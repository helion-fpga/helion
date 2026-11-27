# N65 fix report: one shared fixture list for the coord flow and verify scripts

Date: Fri 2026-11-27 (work 08:27 to 08:55 EST)
Worker: N65 fix worker
Branch: `fm/ultra-bitgen-n65fix`, worktree `~/workspace/helion-ultra-bitgen-n65fix`
Base: `fm/ultra-bitgen-n38fix` at `ce098dfaebac40e95fb5bb4756faac6000ce6300`
Status: fix complete and proven. Both coord scripts now source one shared
fixture list, both re-run from that list with summaries byte-identical to
control and all 12 bitstreams md5-identical. Scratch demo proves the
failure mode is gone: one bad path in the shared list fails the same design
in both scripts. No merges. No critic dispatch.

## 1. Ground truth and base verification

N65 per CRITIQUE.md line 6795 (Critic Session 42), re-verified fresh by the
driver on the latest lane scripts: the coord flow script and the impl flow
script both carry the same fixture paths (critic42-runs/counter12.sv,
examples/blinky.sv, examples/counter.sv, intcols-logs/impl), i.e. the paths
were duplicated instead of shared, and the coord script first ran with
wrong paths (per the report). Process-only, carried. It recurred in later
critic sessions (47-55); Session 51 notes the coordinator verify script
once carried stale port-map paths copied from the n66 script, fixed, three
verify-bits re-run PASS. That is the exact failure mode this fix kills:
two scripts each carrying their own copy of the design set, agreeing only
by hand.

Mechanism re-verified on the latest lane scripts before changing anything,
in `~/workspace/n76n77n78fix-logs/coord/`:

- `coord-run-flows.sh` (44 lines) hardcodes fixture paths inline per
  `run_one` call: `FIX=$HOME/workspace/intcols-logs/impl` (line 10),
  `$HOME/workspace/critic42-runs/counter12.sv` (line 36),
  `$HOME/workspace/helion-n76n77n78fix-coord/examples/counter.sv`
  (line 41) and `examples/blinky.sv` (line 42), plus chain16/shift32/
  counter8/counters2x4/counter5/counter16 from `$FIX`.
- `coord-run-verify.sh` (31 lines) hardcodes the same designs' RTL and
  port-map paths inline per `vb` call, scattered across many different log
  dirs: stadin-logs, n58-logs, critic42-runs, clk2leg-logs, routepack-logs,
  spanclk-logs, counter-logs, emitter-xczu3eg-logs, n47-logs.
- No shared list existed anywhere. The two scripts each carried their own
  copy of the design set and had to agree by hand.

Base verified before branching:

- `git rev-parse fm/ultra-bitgen-n38fix` in `~/workspace/helion`:
  `ce098dfaebac40e95fb5bb4756faac6000ce6300`
- `git ls-remote origin fm/ultra-bitgen-n38fix`:
  `ce098dfaebac40e95fb5bb4756faac6000ce6300`
- Local and origin tips match, so the branch was created from the current tip.
- `git ls-remote origin fm/ultra-bitgen-n65fix` returned nothing: no such
  ref existed on origin.
- No `~/workspace/helion-ultra-bitgen-n65fix` worktree existed.
- New branch `fm/ultra-bitgen-n65fix` created in worktree
  `~/workspace/helion-ultra-bitgen-n65fix` at `ce098dfa`, clean status.
  No execution-risk heuristic tripped.

## 2. Control (recorded before changing anything)

The existing summaries and logs are the control. The scripts were NOT
re-run for the control. Copies in `~/workspace/ultra-bitgen-crew/logs/`:

- `n65fix-control-coord-flow-summary.txt` (control flow summary, Nov 26 19:18)
- `n65fix-control-coord-verify-summary.txt` (control verify summary, Nov 26 19:19)
- `n65fix-control-coord-run-flows.sh` (control flow script text)
- `n65fix-control-coord-run-verify.sh` (control verify script text)
- `n65fix-control-bit-md5s.txt` (full md5s of all 12 control .bit files)
- `n65fix-control-flow-exit-codes.txt` and
  `n65fix-control-verify-exit-codes.txt` (per-design exit codes)

Control result, 12 designs in both scripts:

- Flow: all 12 exit 0. md5s (short form from the summary):
  c16 4cf99124, c16-r2 4cf99124, s32 a7a070dd, s32-r2 a7a070dd,
  c12-s0 b18112d3, c8-s0 2902d6f0, c8-s1 01cb7f90, c2x4 730fcc93,
  c5 6fe2c80a, counter 4c99a51f, blinky d82b70c8, c16-s1 a45c6efd.
- Verify: all 12 exit 0, all 12 PASS, 800 cosim comparisons each except
  c16-s1 with 80000 comparisons (40000 cycles), 0 mismatches everywhere.
- All 24 fixture files referenced by the two scripts were checked present
  on disk before the rewrite (9 flow .sv sources, 12 verify RTL files,
  12 port-map JSON files, plus the relative blinky RTL
  `tests/fixtures/blinky/rtl.v` under tools/us09v). One check failed at
  first and shaped the proof: see section 5.

## 3. The shared list

New file `~/workspace/ultra-bitgen-crew/coord-fixtures.sh` (4986 bytes),
the single source of truth for the coord design set. Both scripts source
it and iterate over it; no fixture path is duplicated in either script.

Design:

- One record per design, in run order, pipe separated:
  `tag | flow .sv | impl seed | flow label | design | verify RTL .v |
  top | port map | cycles`
  Stored in the `COORD_FIXTURES` array. 12 records, tags in the same order
  as the original scripts: c16, c16-r2, s32, s32-r2, c12-s0, c8-s0, c8-s1,
  c2x4, c5, counter, blinky, c16-s1.
- The lane worktree path, fixture dirs, binary path, and log dir are
  variables the calling scripts set before sourcing. The list references
  only `COORD_WORKTREE`, `COORD_FIXDIR`, and `COORD_CRITIC42_DIR`, and
  fails loud with a named message if any is unset
  (`: "${COORD_WORKTREE:?coord-fixtures.sh: COORD_WORKTREE must be set by
  the calling script}"`, same for the other two). The list hardcodes no
  lane worktree path: a future lane reuses it by setting the same three
  variables to its own worktree, plus its own BIN and LOGDIR.
- Values preserved exactly, verified mechanically (section 5):
  every flow .sv, seed, label, design name, RTL path, top, port map, and
  cycle count matches the original inline call. Two values called out
  explicitly in the file header so no future editor "fixes" them: the
  blinky verify entry keeps its relative RTL path
  `tests/fixtures/blinky/rtl.v` (it resolves relative to tools/us09v,
  the verify script's working directory), and c16-s1 keeps 40000 cycles
  while every other design uses 400. The c16-r2 flow label stays
  `chain16-r2`, which differs from its design name `chain16`.
- The other RTL and port-map paths stay as `$HOME/workspace/...`
  references to the historical artifact dirs (stadin-logs, n58-logs,
  critic42-runs, clk2leg-logs, routepack-logs, spanclk-logs,
  counter-logs, emitter-xczu3eg-logs, n47-logs). Those dirs are shared
  history, not lane-specific, so they are not parameterized; only the
  lane worktree and the two lane fixture dirs are.

## 4. The rewritten scripts

Both scripts live in `~/workspace/n76n77n78fix-logs/coord/` (a log dir,
not the repo). Only the two scripts changed there.

`coord-run-flows.sh` (44 lines, 1920 bytes):

- Header gains the N65 note. `FIX` is replaced by the three lane
  variables `COORD_WORKTREE`, `COORD_FIXDIR`, `COORD_CRITIC42_DIR`,
  set next to the existing `BIN` and `LOGDIR`. One `source` line loads
  the shared list. The 12 `run_one` calls become one loop over
  `COORD_FIXTURES`, passing tag, flow .sv, seed, and label.
- `run_one` itself is byte-identical: same `helion impl` invocation,
  same `--part xczu3eg`, same log, .bit, and .fasm paths, same detail
  greps, same summary line format, same summary file, same final echo.
  Same env exports (`HELION_TILECONN_DB_ROOT`, `HELION_TILECONN_PART`).

`coord-run-verify.sh` (31 lines, 1733 bytes):

- Header gains the N65 note. The same three lane variables are set next
  to `LOGDIR`. One `source` line loads the shared list. The 12 `vb`
  calls become one loop over `COORD_FIXTURES`, passing tag, bit path
  (`$LOGDIR/coord-${tag}.bit`), design, RTL, top, port map, and cycles.
- `vb` itself is byte-identical: same `python3 -m us09v verify-bit`
  invocation, same `--top` passing (N34/N37 fail-loud rule untouched),
  same workdir, same summary line format, same summary file, same final
  echo. The N34 decoder env `US09V_DECODER_BIN` is unchanged.

Diff of each script against its control copy (in section 2) shows only
the header comment, the variable block, the `source` line, and the
call-list-to-loop change. No other line differs. Nothing else in the
coord log dir was touched except the proof outputs the scripts themselves
produce (summaries, per-design logs, .bit, .fasm). No Rust code, no
N7/N8/N9/Q code, no decoder behavior, and no N34/N37/N38 fail-loud check
was touched. The N9 worktree and the `fm/ultra-bitgen-n9fix` branch were
not touched. No other lane's log dir was touched.

## 5. Proof

### 5a. Static value check (before any re-run)

A script parsed the 12 `run_one` calls and the 12 `vb` calls out of the
control script copies, expanded `$FIX` and `$HOME`, and compared every
field against the resolved shared list
(`~/workspace/ultra-bitgen-crew/logs/n65fix-value-check.txt`,
`n65fix-resolved-fixtures.txt`). Result: all 12 flow tuples and all 12
verify tuples identical, tag order identical. This check runs on values
only and proves the rewrite preserves every path, seed, label, top,
port map, and cycle count.

### 5b. Rebuild required before the live re-run

The proof re-run needs the lane binaries, and they were gone: the coord
worktree `~/workspace/helion-n76n77n78fix-coord/target/` no longer
existed at 08:27 EST Nov 27 (the prune lists in the coord dir are dated
Nov 26 18:40-18:42; the control run at 19:17-19:19 used binaries that
have since been pruned). The scripts were not changed to point anywhere
else; the binaries were rebuilt from the same lane branch instead:

- `cargo build --release --bin helion --bin helion-bitgen` in
  `~/workspace/helion-n76n77n78fix-coord`, log
  `~/workspace/ultra-bitgen-crew/logs/n65fix-rebuild.log`.
- The first attempt was killed by a runtime restart 6m 44s in, before
  finishing, with no binaries produced. It was restarted with the same
  command (cargo resumes incrementally from the partial target dir; no
  other effect). It finished in 2m 36s, exit 0.
- Rebuilt `helion-bitgen` md5 is
  `1ef1a8761337c18e147fbb7b62149a6f`, byte-identical to the lane report's
  recorded `1ef1a876` for the binary used in the control run
  (5,738,240 bytes). Rebuilt `helion` is 13,095,296 bytes, md5
  `b52179c36a0b095e3df6dd0a658bd0ce`. The proof re-run is therefore
  apples-to-apples with the control run: same lane branch (9c4917de),
  same decoder bytes.

### 5c. Live re-run from the shared list

Flow script, run from anywhere, 08:45:27-08:46:46 EST, 1m 18.6s, exit 0.
Console: `~/workspace/ultra-bitgen-crew/logs/n65fix-proof-flows-console.log`.

- `diff` of the new `coord-flow-summary.txt` against the control copy:
  identical, byte for byte.
- `diff` of the full md5s of all 12 new `.bit` files against the control
  md5s: all 12 identical (including c16 matching c16-r2 at 4cf99124 and
  s32 matching s32-r2 at a7a070dd, as in control).
- All 12 exit 0, same tags in the same order, same PASS detail lines
  (same WNS_PS and pips counts per design).

Verify script, run from
`~/workspace/helion-n76n77n78fix-coord/tools/us09v` (required: the
blinky relative RTL path resolves from there), 08:46:59-08:48:07 EST,
1m 8.2s, exit 0. Console:
`~/workspace/ultra-bitgen-crew/logs/n65fix-proof-verify-console.log`.

- `diff` of the new `coord-verify-summary.txt` against the control copy:
  identical, byte for byte.
- All 12 exit 0, all 12 PASS, same comparison counts (800 each, 80000
  for c16-s1), 0 mismatches everywhere.

The proof summaries are the live files in
`~/workspace/n76n77n78fix-logs/coord/`; re-checked after the scratch
demo below, they are still the proof outputs, byte-identical to control.

## 6. Scratch failure-mode demo

Purpose: show the N65 failure mode is gone, i.e. one fixture change now
reaches both scripts from the one place, instead of each script carrying
its own copy that can drift.

Setup, in `~/workspace/ultra-bitgen-crew/logs/n65fix-scratch/` (scratch
copies only; the live scripts and live list were never edited for this):

- Copied the shared list and both scripts. The scratch scripts source
  the scratch list and write to the scratch dir, so no live log, summary,
  or .bit was touched.
- One change, and only one: the c5 record's flow .sv became
  `/nonexistent-dir/counter5.sv`. `diff` of the live list vs the scratch
  list shows exactly that one line. `grep -r nonexistent` over the
  scratch dir finds it in exactly one file, one line: the shared list.
  Neither scratch script contains the string.

Results:

- Scratch flow, 08:48:55-08:50:07 EST, 1m 12.1s. c5: `exit=1 counter5
  md5=?`. The other 11 designs exit 0 with the control md5s. The c5 log
  reads: `note: /nonexistent-dir/counter5.sv: No such file or directory
  (os error 2)` and `impl: No such file or directory (os error 2)`.
- Scratch verify, 08:50:12-08:51:17 EST, 1m 4.9s. c5: `exit=1
  counter5: FAIL`. The other 11 designs exit 0 PASS. The c5 log reads:
  `counter5: FAIL` and `[FAIL] decode: exit 1: helion-bitgen: cannot
  read bitstream .../n65fix-scratch/coord-c5.bit: No such file or
  directory (os error 2)`.
- So one bad path, entered once, in the shared list, failed the same
  design (c5) in both scripts, and only that design. Before this fix,
  the same mistake in one script could not have reached the other at
  all: that silent divergence is the defect N65 names. The scratch dir
  and its outputs were left in place for the critic to inspect; the live
  list still contains the real c5 path (verified: 0 occurrences of
  `nonexistent` in the live list).

Note on script exit codes: both scripts exit 0 overall even when one
design fails (they never had `set -e`; the per-design exit codes in the
summaries are the signal). That is pre-existing behavior, preserved
unchanged, not part of this fix.

## 7. Residual and out of scope

- Other lanes' older coord dirs still carry their own copies of fixture
  paths (e.g. earlier n-lane log dirs). They are out of scope for this
  fix: this lane's two scripts are the ones the driver named, and only
  they were rewritten. A future lane adopts the shared list by setting
  its own variables and sourcing it; migrating old lanes is separate work.
- The coord worktree's `target/` dir was pruned after the control run
  (section 5b). The rebuild reproduced the decoder byte-identically,
  so the proof stands, but any future proof re-run that needs the lane
  binaries must rebuild first if `target/` is pruned again.
- No critic was dispatched, per the task. The branch is ready for the
  critic to review the exact list text from `tools/n65fix/`.

## 8. Branch contents and push

Committed on `fm/ultra-bitgen-n65fix`, nothing else in the diff:

- `tools/n65fix/N65FIX_REPORT.md`: byte-identical copy of this report.
- `tools/n65fix/coord-fixtures.sh`: byte-identical copy of the shared
  list at `~/workspace/ultra-bitgen-crew/coord-fixtures.sh`.

Byte-identity is checked with sha256 before pushing (see the commit
record in the worker's final report). The live shared list stays at
`~/workspace/ultra-bitgen-crew/coord-fixtures.sh`, which is what the
rewritten coord scripts source. No merges.
