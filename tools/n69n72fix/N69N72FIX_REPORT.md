# N69/N70/N71/N72 fix lane report

Lane branch: fm/ultra-bitgen-n69n72fix, impl tip e1a6eecc (impl branch
fm/ultra-bitgen-n69n72fix-impl on n67n68fix base a687c2b4). No merges into
fm/ultra-bitstream, fm/zedboard-merge, or master. No critic dispatched by
this lane; Critic Session 53 is the main agent's to dispatch.

## 1. Outcome

All four nits from Critic Session 52 are fixed and verified by independent
runs at every level (workers, coordinator):

- N69 is CLOSED. The helion-cli ultra flow stops tests no longer fail on
  stale outputs: the fixed temp dir is cleaned at the start of the
  assert helper, so a prior successful run cannot poison a later run.
  Fail-before reproduced on the base tip (0/2, both panics at ultra.rs:2819)
  and pass-after on the lane tip (2/2, dirs re-poisoned before the run).
- N70 is CLOSED as a reclaim-on-demand step. run-suites.sh gains an opt-in
  --prune-target step that deletes older-hash artifacts per crate from
  target/release/deps and target/debug/deps, keeps the newest hash, and
  records before/after numbers. On this lane it freed 104,918,993 bytes
  (dup crates 20 to 0) and the post-prune rebuild plus the full suites
  prove the prune was safe. The limit is filed as N74: reclamation is
  transient across a full suite run, because cargo test rebuilds under
  the test profile.
- N71 is CLOSED. The N67N68 report section 6 now states the
  633327af..752ce8d5 diff is exactly 3 files, +63/-4, with no report copy
  in it; the 227-line report copy arrives in f9fb18e7, and 252 is the
  INTINTF report's line count. Both copies byte-identical.
- N72 is CLOSED. run-suites.sh defaults are generic: WORKTREE defaults to
  the git top-level of the repo containing the script, LOGDIR defaults to
  $HOME/workspace/<worktree-name>-logs. The coordinator ran the script
  with no positional args and the logs landed in the derived dir.

What this lane does NOT do: no production code changes, no flow number
moves, no DSP/BRAM end-to-end work (still blocked on placement,
config-bit encoding, and INT_INTF segbits, as Session 51 ruled).

## 2. The fix (3 files, +135/-7)

- `crates/helion-cli/src/ultra.rs` (5 lines, W-N69 commit 6a985c0b):
  `temp_out_paths` now does `remove_dir_all` on the fixed dir before
  `create_dir_all`, with a comment explaining why. No flow behavior,
  production code, or test expectation changed. The bug-class sweep
  found the xczu3eg flow tests at lines 3146 and 3282 already safe: they
  `remove_file` both outputs before `run_impl` and assert presence after
  success, not absence after failure.
- `tools/ultra-bitgen/run-suites.sh` (+131/-7, W-SCRIPT commit c8eb5087):
  generic defaults (N72) and the opt-in --prune-target hygiene step
  (N70). The prune keeps the newest-hash artifacts per crate and deletes
  older-hash files matching the strict cargo artifact pattern
  (lib<crate>-<16 hex> or <crate>-<16 hex>, plus an extension), plus
  matching .fingerprint/<crate>-<hash> dirs, only inside this worktree's
  target dir, never sources, never top-level binaries, never any other
  worktree's target dir. If CARGO_TARGET_DIR points outside the worktree,
  pruning is skipped with a clear message. A plain run without the flag
  never deletes anything. The DB-variables scoping is untouched: exported
  only for the ultrascale suite, unset before CLI and checkers.
- `tools/n67n68fix/N67N68FIX_REPORT.md` (6 lines changed, W-SCRIPT commit
  c8eb5087): section 6 wording corrected per N71. The crew copy
  ~/workspace/ultra-bitgen-crew/N67N68FIX_REPORT.md was overwritten with
  the fixed file; cmp exits 0.

## 3. N69: fail-before and pass-after

Worker W-N69, verified by the coordinator's own runs on separate worktrees:

- Fail-before, base tip a687c2b4 (worktree helion-n69n72fix-baseline,
  source touched to force a fresh test binary, DB variables unset,
  /tmp/helion-us09-flow-blinky and /tmp/helion-us09-flow-counter poisoned
  with dummy .xcku3p.bit and .fasm files): cargo test -j1 -p helion-cli
  stops_at_route_without_pin_wire_map, exit 101, 0 passed, 2 failed; both
  tests panicked at ultra.rs:2819 with "bitstream file exists despite the
  ROUTE failure". Log n69n72fix-logs/coord/n69-fail-before.log.
- Pass-after, merged branch (worktree helion-n69n72fix-impl, same poison,
  source touched to force a fresh test binary): cargo test --release -j1
  -p helion-cli stops_at_route_without_pin_wire_map, exit 0, 2 passed,
  0 failed; both /tmp dirs empty after the run, so the fix cleaned them.
  Log n69n72fix-logs/coord/n69-pass-after.log.
- The worker's own logs agree: fail-before 0/2 at ultra.rs:2819,
  pass-after 2/2, full CLI release suite 92/0/1.

## 4. N70: measurements and prune proof

Worker W-SCRIPT, verified by the coordinator's own end-to-end run:

- Grounding, read-only, on the intintf-model-impl release deps (1.2G):
  646 matched files, 132 crates, 74 crates with more than one hash,
  99 extra hash groups, 1,103,884,344 bytes in dup crates. Top offenders
  by hash count: syn 7 hashes / 21 files / 96.9M, memchr 5 / 15 / 14.7M,
  object 4 / 12 / 61.4M, helion 3 hashes, helion_ultrascale 3 hashes.
  (The N67N68 report's "libsyn 14 copies, libmemchr 10" were file counts;
  these are distinct hashes.)
- The worker's own worktree after a fresh release build, before prune:
  target/release 631M, deps 639,500,535 bytes, 396 files, 118 crates,
  20 dup crates, 22 extra hashes, 120 files / 161,453,370 bytes in dup
  crates. Prune inside its run 1: deps 639,500,535 to 534,581,416 bytes,
  freed 104,919,119, dup crates 20 to 0, 63 files deleted, 17 fingerprint
  dirs deleted. Post-prune rebuild (cargo build --release -j1 --bin helion
  --bin helion-bitgen) exit 0 in 3m33s; rebuilt binaries bit-identical to
  the pre-prune build.
- The coordinator's own end-to-end run (section 7) ran the prune inside
  run-suites.sh --prune-target: deps 639,498,147 to 534,579,154 bytes,
  freed 104,918,993, dup crates 20 to 0, 63 files deleted, 17 fingerprint
  dirs deleted, and the suites that followed passed at the same totals.
- A synthetic test of the exact shipped prune code on a fake tree deleted
  6 files and 3 fingerprint dirs, kept the newest hashes, and left
  single-hash crates, non-matching files, the top-level binary, and a
  fingerprint dir with no deps files untouched; a CARGO_TARGET_DIR outside
  the worktree was refused.
- After the worker's full run 1 the target regrew to 1.1G with 73 dup
  crates; see N74 in section 8. The prune is a reclaim-on-demand step,
  not a permanently small target dir.

## 5. N71 and N72: verification

- N71: `git diff 633327af..752ce8d5 --shortstat` is 3 files, +63/-4;
  `git show --stat f9fb18e7` is 1 file, 227 insertions; `wc -l` is 227 on
  both report copies and 252 on the INTINTF report. The fixed file stays
  227 lines; section 2's header was already correct and untouched.
- N72: the worker ran the script with no positional args (run 1, derived
  WORKTREE and LOGDIR), with explicit args (run 2, honored, derived dir
  untouched), and a harness over the shipped arg-parsing lines (relative
  and absolute no-arg invocations, --prune-target plus explicit args,
  explicit WT only with derived LOGDIR, outside-any-repo invocation fails
  loud exit 1). The coordinator's own end-to-end run used no positional
  args and the logs landed in ~/workspace/helion-n69n72fix-impl-logs/,
  the derived dir for its worktree.
- `bash -n tools/ultra-bitgen/run-suites.sh` exits 0. shellcheck is not
  installed on this VM, so no shellcheck run was possible.

## 6. Branch diff

Diff a687c2b4..e1a6eecc is exactly 3 files, +135/-7 (report copy separate):
crates/helion-cli/src/ultra.rs +5, tools/ultra-bitgen/run-suites.sh
+131/-7, tools/n67n68fix/N67N68FIX_REPORT.md 6 lines changed. No
production code changed anywhere, no other file touched. The coordinator
read the full diff.

## 7. Suites, flows, verify-bits (coordinator's own runs)

Release build from the merged branch (cargo build --release -j1 --bin
helion --bin helion-bitgen) exit 0 in about 11m47s: helion 13,095,296
bytes md5 621aa3b120081094b91f3a1326398871, helion-bitgen 5,738,240 bytes
md5 ace9f1295af191dc8cd4829c652b6377. Sizes match the N67N68 lane; md5s
differ across worktrees (N54 class, section 8).

Canonical script end to end as one invocation, exit 0, ALL SUITES DONE
(tools/ultra-bitgen/run-suites.sh --prune-target, no positional args,
US09V_DECODER_BIN set to the coordinator's lane-built helion-bitgen,
logs in ~/workspace/helion-n69n72fix-impl-logs/): ultrascale 475/0/1
(36 ok-lines, recount from the log), CLI release 92/0/1 (recount), checkers
Ran 100 tests in 122.317s, OK. The worker's two runs agree: run 1 (no args,
--prune-target) 475/0/1, 92/0/1, 100 OK in 117.490s; run 2 (explicit args,
plain) 475/0/1, 92/0/1, 100 OK in 126.771s.

12 flows exit 0 with numbers identical to the n67n68fix tip and Session 52
(chain16 8570/362/4cf99124 twice, shift32 8690/289/a7a070dd twice, counter12
7370/752/b18112d3, counter8 s0 8570/363/2902d6f0, s1 8570/342/01cb7f90,
counters2x4 8450/307/730fcc93, counter5 9650/82/6fe2c80a, counter
9650/58/4c99a51f, blinky 9650/4/d82b70c8, counter16 5750/1278/a45c6efd);
12 verify-bits PASS (800/0 each, counter16 80000/0). Blinky .bit is
5,982,532 bytes, md5 d82b70c8e29ce72f28e08c94f3a16d16, unchanged.

rustfmt --edition 2024 --check on ultra.rs: 23 deviations on the lane file
and 23 on the base file; the normalized outputs differ only by a +5 line
shift after the insertion point. Zero new deviations, none in the edited
region. (The file does not pass --check clean even at base; the 23 are
pre-existing.)

## 8. New nits

- N73 (observation, non-blocking): rebuilt binaries match the pre-prune
  build bit-for-bit, but their md5s differ from the N67N68 lane's at
  identical sizes (worker: helion 15e1719b vs 08f8b705, helion-bitgen
  7e756147 vs 3af9ba4f; coordinator: helion 621aa3b1, helion-bitgen
  ace9f129). Likely worktree-path strings embedded in the binaries;
  unproven, no impact: the decoder is built from this branch and the
  checkers pass 100 OK. N54 recurrence.
- N74 (finding, non-blocking): prune reclamation is transient across a
  full suite run. After the worker's run 1 the target regrew to 1.1G with
  73 dup crates, because cargo test --release rebuilds crates under the
  test profile, and the name-only grouping also deletes live
  multi-version artifacts (syn 1/2/3/0.15 are distinct versions, not
  stale rebuilds; all 4 hashes returned after the run). Cargo rebuilds
  what it needs, proven by identical totals and bit-identical rebuilt
  binaries, but a version-aware grouping would avoid rebuilding live
  versions. Prune is a reclaim-on-demand step, not a permanently small
  target dir.

## 9. Incidents

- A VM/runtime restart killed the worker's first post-prune rebuild
  attempt with SIGTERM after about 7 min. No results taken from it; the
  rebuild was re-run from fingerprints and finished exit 0. Same incident
  class as the N67N68 lane's killed script run.
- The checkers created untracked tools/us09v/tests/__pycache__/ and
  tools/us09v/us09v/__pycache__/ in both workers' worktrees and in the
  coordinator's impl worktree; all were removed before committing. No
  results were taken from tainted runs.
- Disk quota: 59G free at lane start; no quota error in any run.

## 10. Carried

N65, N51, N53, N52, N54, N55, N56, N59, N60, N61, N62, N63, IOB blocked
on B1, known-INIT datum, documented xczu3eg timing except the single BRAM
field, 54,907 non-band bits, N16, N9, N34, N37, N38, A-CTRL, A-F7F8.
Fabric delays still blocked on Vivado report_timing plus write_sdf; Tier 2
waits for Vivado. No critic dispatched by this lane; main agent dispatches
Critic Session 53.
