# N74/N75 Fix Lane Report

Branch: `fm/ultra-bitgen-n74n75fix`, merge tip `b4d9b639` (base `d7b481de`, the n69n72fix tip).
The report copy is committed on the branch; the final push tip is recorded in
COORD.md. Date: 2026-11-26. Coordinator: lane agent (no project code written by the coordinator).
Workers: W-PRUNE (agent `c22cdf0b`), W-VERIFY (agent `17797b4e`).

## 1. Summary

Both nits from Critic Session 53 were worked on one branch, off the n69n72fix tip.

- N74 (prune deletes live multi-version artifacts): FIXED by W-PRUNE. The prune
  grouping in `tools/ultra-bitgen/run-suites.sh` is now keyed on
  (crate, version, features, unit-kind, profile) instead of crate name only.
  Synthetic multi-version test passes, real-run proof shows the named
  multi-version and multi-feature artifacts survive, and the full suites pass
  at the same totals afterward.
- N75 (saved `coord-run-verify.sh` has a stale c5 port-map path): NOT REPRODUCED.
  W-VERIFY proved byte for byte that the saved script already holds the correct
  path; the stale path appears only in the critic's own hand-built log and in
  the CRITIQUE.md filing. The lane committed a parameterized, reviewable copy
  of the script at `tools/n74n75fix/coord-run-verify.sh` as durable evidence and
  a reusable script. No in-place edit was made, because the file already
  contains the correct path.

Coordinator acceptance from the merged lane branch: release build exit 0;
`run-suites.sh --prune-target` end to end exit 0, ALL SUITES DONE
(ultrascale 475/0/1, CLI release 92/0/1, checkers 100 OK); 12 flows exit 0
with numbers identical to the n69n72fix tip; 12 verify-bits PASS; blinky .bit
md5 `d82b70c8e29ce72f28e08c94f3a16d16` unchanged.

New nits filed by this lane: N76 (orphan fingerprint dirs accumulate) and N77
(debug same-profile dependency-universe duplicates). A third residual, the
release bin-vs-test universe pairs found by the coordinator's post-suite
measurement, is filed as N78; see section 6.

## 2. Branch and diff

Base: `fm/ultra-bitgen-n69n72fix` at `d7b481de` on origin.
Lane branch: `fm/ultra-bitgen-n74n75fix`.
Impl branch: `fm/ultra-bitgen-n74n75fix-impl` (worker merges only).
No merges into `fm/ultra-bitstream`, `fm/zedboard-merge`, or master.

Diff vs base `d7b481de` (before the report commit):

```
 tools/n74n75fix/coord-run-verify.sh |  45 ++++++++
 tools/ultra-bitgen/run-suites.sh    | 197 ++++++++++++++++++++++++++++++------
 2 files changed, 213 insertions(+), 29 deletions(-)
```

The run-suites.sh change is 168 insertions and 29 deletions (the 197 is the
per-file changed-line count). No production code changed; this lane is
script-only. Worktrees left clean (`__pycache__` dirs removed before
committing).

Commits on the lane branch:

```
95936c74 N75: commit corrected N69/N72 coordinator verify script
08f0b554 n74n75fix: merge W-VERIFY (N75 evidence: committed verify script copy)
45271f25 N74: version/feature/kind/profile-aware prune grouping
5bfef082 n74n75fix: merge W-PRUNE (N74 version-aware prune grouping)
b4d9b639 n74n75fix: merge fm/ultra-bitgen-n74n75fix-impl into fm/ultra-bitgen-n74n75fix
```

## 3. N74: version-aware prune grouping (W-PRUNE, CLOSED as fixed)

Worker report: `~/workspace/n74n75fix-logs/prune/N74-REPORT.md` (288 lines).
Branch: `fm/ultra-bitgen-n74n75fix-prune`, commit `45271f25`, 1 file changed.

### What changed

The prune group key is now (crate, version, features, unit-kind, profile).
Keep the newest-mtime hash per group; delete older same-group hashes plus
their fingerprint dirs (found by hash glob). Version comes from dep-info `.d`
files (registry paths carry it); features, kind, and profile come from the
fingerprint unit JSON. Unknowns group together (old N70 behavior).

### Correction to the nit as filed

`memchr` is NOT multi-version: Cargo.lock lists exactly one version, 2.8.3;
its three hashes are three live feature sets. `syn` really is multi-version
(0.15.44, 1.0.109, 2.0.119, 3.0.4). The fix covers both dimensions, plus kind
and profile.

### Two N70 defects fixed in the same function

- `.d` filenames misparsed as if carrying a lib prefix (`libc-<hash>.d` read
  as crate "c", a phantom dup; true old count 72, not 73).
- Fingerprint deletion missed dashed crates (now by hash glob).

### Proof

Synthetic test `N74-PRUNE-TEST`: PASS, dup groups 5 to 0, 14 files, 4
fingerprint dirs deleted. The test was extended to a multi-version case: two
versions of one crate both survive, a stale same-version hash is deleted.

Real run, cycle 1 (4-field key, no profile): standalone prune freed
87,031,952 bytes; 9 of 14 release hashes and all 3 debug hashes rebuilt on
the next full run; all 9 release pairs differ in profile hash, so profile
was added to the key.

Real run, cycles 2 and 3 (5-field key): standalone prune freed 0 release and
27,230,376 debug; full `run-suites.sh --prune-target` exit 0 (ultrascale
475/0/1, CLI 92/0/1, checkers 100 OK in 119.659s); post-suite release deps
unchanged at 649,417,840 bytes, 0 hashes rebuilt, 0 dup groups; debug 3
hashes rebuilt deterministically, deps back to exactly 2,505,935,134.

Reference target: 50 dup groups under the new counting (vs 73 old, 57 with
the 4-field key). `bash -n` passes on the edited script; shellcheck is not
installed on this box.

## 4. N75: stale c5 port-map path (W-VERIFY, NOT REPRODUCED / misfiled)

Worker grounding note: `~/workspace/n74n75fix-logs/verify/n75-grounding-note.txt`.
Branch: `fm/ultra-bitgen-n74n75fix-verify`, commit `95936c74`, 1 file, +45.

### What the bytes on disk show

- Line 27 of the saved script `~/workspace/n69n72fix-logs/coord/coord-run-verify.sh`
  holds the CORRECT path: `~/workspace/routepack-logs/impl/counter5-port-map-AQ.json`
  (47 bytes, exists). The string `verify-counter5/` occurs exactly once on
  line 27, in the RTL path (`counter5-rtl.v`), not in the port-map path.
- `grep -c "verify-counter5/counter5-port-map"` on the saved script = 0.
- `grep -c "impl/counter5-port-map"` on the saved script = 1.
- stat: the saved script was created 2026-11-26 05:07:26 UTC and has never
  been modified since (mtime = birth = 05:07:26 UTC, size 3106 bytes). The
  stale-path version would be 15 bytes longer (3121). md5
  `11ffca8643a7a9f12c51b9f3ca1f9717`, unchanged across this whole lane
  (checked at start, mid-lane, and end).
- The stale path string appears nowhere in the coordinator logs or scripts;
  in the searched trees it appears only in the critic's own hand-built stale
  log (`~/workspace/critic53-runs/c53-verify-c5-stale.log`) and once in
  CRITIQUE.md (the N75 filing itself). The critic's own `c53-run-verify.sh`
  line 27 also holds the CORRECT path.
- The coordinator's own run is consistent with the correct path:
  `verify-c5.log` (06:13 UTC) shows counter5 PASS, 800 comparisons, 0
  mismatches, and the summary shows 12/12 PASS. A script holding the stale
  path could not have produced that c5 PASS.

Likely cause: a misread of line 27, where the RTL path on the same line
contains `verify-counter5/` and the eye carries it over to the port-map
path. The coordinator made the same misread at lane start and verified the
correction byte for byte afterward.

One irreproducible observation, disclosed honestly: the very first read in
this lane (sed + grep on the saved script) appeared to show the stale path
on line 27. Every byte-level re-read since (cat -A, python repr, grep -c,
muse.read, two re-runs of the exact first commands) shows the correct path,
and the mtime/size arithmetic rules out any edit between the reads. That
first read cannot be reproduced and is contradicted by six independent
checks plus the file metadata; it is recorded here rather than silently
dropped.

### What the lane did anyway

- Committed a reviewable copy at `tools/n74n75fix/coord-run-verify.sh` on
  the lane branch, with the c5 port map at the correct path, a header stating
  the N75 correction, and `US09V_DECODER_BIN` plus `LOGDIR` parameterized
  (decoder defaults to this checkout's `target/release/helion-bitgen` via
  the script's location; LOGDIR defaults to
  `~/workspace/n74n75fix-logs/verify`). All twelve vb lines are identical
  to the saved script; the diff vs the saved script shows only the header,
  the decoder parameterization, and the LOGDIR line.
- No in-place edit was made to the saved script, because the file already
  contains the correct path byte for byte. Editing it to "the same one-line
  change" would be a no-op; the md5 before and after this lane is identical
  (`11ffca8643a7a9f12c51b9f3ca1f9717`).

### Proof

- Fail-before on a hand-built stale-path invocation: exit 1,
  `FileNotFoundError` for
  `~/workspace/routepack-logs/impl/verify-counter5/counter5-port-map-AQ.json`
  (log `n75-fail-before-c5-stale.log`). This reproduces the critic's
  stale-path failure as an invocation; it does not implicate the saved
  script.
- Pass-after with a branch-built decoder (5,738,208 bytes, built in 52.56s):
  12/12 verify-bits PASS, c5 800/0, counter16 80000/0; path check: all 24
  RTL + port-map paths exist (log `n75-pass-after-console.log`,
  `n75-path-check.log`).

## 5. Coordinator acceptance (own runs from the merged lane branch b4d9b639)

All runs from the lane worktree `~/workspace/helion-n74n75fix-coord`, decoder
built from the current branch, `US09V_DECODER_BIN` set on every verify-bit
and checkers run (N34 trap), absolute `--workdir` and `--top` on verify-bit
(N37). Logs: `~/workspace/n74n75fix-logs/coord/`.

### Release build: exit 0

`cargo build --release -j1`, finished in 11m 46s
(log `coord/build-release.log`):

- `helion` 13,095,296 bytes, md5 `d3f4985a7d86f949de0df7ce94f879f6`
- `helion-bitgen` 5,738,240 bytes, md5 `a1ed3100bbc32989f2f81a8f3535f359`

### Full suites: exit 0, ALL SUITES DONE

`tools/ultra-bitgen/run-suites.sh --prune-target`, one invocation
(log `coord/suites-console.log`, derived logs
`~/workspace/helion-n74n75fix-coord-logs/`):

- ultrascale 475 passed, 0 failed, 1 ignored (36 ok-lines, recount)
- CLI release 92 passed, 0 failed, 1 ignored (11 ok-lines)
- checkers: `Ran 100 tests in 117.908s`, OK
- Prune was a no-op on the fresh build: release deps 639,499,341 to
  639,499,341 bytes, freed 0, dup groups 0 to 0; debug had no deps dir.

### 12 flows: exit 0, numbers identical to the n69n72fix tip

(log `coord/flows-console.log`, summary `coord/coord-flow-summary.txt`):

| flow | WNS_PS | pips | md5 |
| --- | --- | --- | --- |
| chain16 | 8570 | 362 | 4cf99124 |
| chain16-r2 | 8570 | 362 | 4cf99124 |
| shift32 | 8690 | 289 | a7a070dd |
| shift32-r2 | 8690 | 289 | a7a070dd |
| counter12 | 7370 | 752 | b18112d3 |
| counter8 s0 | 8570 | 363 | 2902d6f0 |
| counter8 s1 | 8570 | 342 | 01cb7f90 |
| counters2x4 | 8450 | 307 | 730fcc93 |
| counter5 | 9650 | 82 | 6fe2c80a |
| counter | 9650 | 58 | 4c99a51f |
| blinky | 9650 | 4 | d82b70c8 |
| counter16 | 5750 | 1278 | a45c6efd |

### 12 verify-bits: PASS

(log `coord/verify-console.log`, summary `coord/coord-verify-summary.txt`):
800 comparisons, 0 mismatches each; counter16 80000 comparisons, 0
mismatches. Blinky `.bit` 5,982,532 bytes, md5
`d82b70c8e29ce72f28e08c94f3a16d16`, unchanged.

The committed `tools/n74n75fix/coord-run-verify.sh` was also run end to end
from the merged branch (LOGDIR `coord-committed`, decoder coord-built):
exit 0, 12/12 PASS including c5.

`bash -n` passes on both edited scripts. No Rust changed, so no rustfmt
check applies.

## 6. Post-suite release regrowth: N78 (residual, filed by the coordinator)

After the coordinator's full suite run, a read-only measurement with the
shipped prune functions showed the coord worktree `target/release/deps` at
1,137,824,848 bytes with 54 dup groups and 213 (crate, hash) pairs, versus
639,499,341 bytes and 0 dup groups at suite start. The worker's steady-state
proof showed 649,417,840 bytes, 0 dup groups, 140 pairs, and its worktree
target is still 649,417,840 bytes (398 files in deps).

The 54 groups are all count 2, mostly lib kind with identical
(crate, version, features, kind, profile), e.g. varisat, thiserror,
sv_parser_*, serde, regex, rayon, parking_lot_core; profiles
1692222358841669167 (most), 16706159572256498396 (proc macros),
4294650570988402071 (regex). These look like dev-dependency crates from
the CLI test build.

### Forensics

For serde, the two hashes have identical fingerprint JSON (features
[default, derive, serde_derive, std], profile 1692222358841669167); they
differ only in the dependency-fingerprint closure. mtimes: the older hash
was built during the bin build (11:23 UTC), the newer during the CLI test
build (12:03 UTC). This is the N77 mechanism (live alternate-universe
builds, differing only in dependency fingerprints) in release, not just
debug: the bin universe and the test universe build the same crate twice
under the same 5-field key.

### Decisive experiment (prune, rebuild, count reappearances)

Prune 1 (release, shipped functions): freed 401,321,602 bytes, dup groups
54 to 0, 157 files deleted, 54 fingerprint dirs deleted. A partial rebuild
(`cargo build --release`, killed by a VM restart about 4 min in) restored
239,480,556 bytes, and serde's deleted hash `2f4b495e` was observed back
in deps with the rebuild log showing `Compiling serde v1.0.229`.

Prune 2 (release, after the partial rebuild): 190 pairs, 30 dup groups;
freed 180,769,588 bytes, 30 to 0, 87 files, 30 fingerprint dirs deleted.
Rebuild (`cargo build --release --bin helion --bin helion-bitgen` plus
`cargo test --release -p helion-cli --no-run`, both exit 0; logs
`rebuild-bins.log`, `rebuild-tests.log`): deps back to exactly
1,137,824,848 bytes, 213 pairs, 54 dup groups, and all 30 of the 30
deleted hashes reappeared, 0 stayed gone. The reappeared hashes include
serde `2e6ae397f26a7e1e`, regex `0ef85111fe9b17e0`, rayon
`ebd847e200df6723`, and serde_derive `db4c960ebd9c04e1`.

### Ruling

The worker's "release regrowth eliminated" claim holds in steady state
(target already contains suite artifacts before the prune), but NOT on a
fresh worktree's first suite run: the CLI test build creates bin-vs-test
universe pairs that the 5-field key cannot separate, the prune deletes one
member of each pair, and the next build rebuilds it. The deletion is the
bug, and the key is the cause: no static key available from the fingerprint
JSON separates the two universes. Filed as N78; the fix, if any, needs a
key that distinguishes the test universe from the bin universe (candidate:
the dependency-fingerprint closure itself), which this lane did not
attempt. This does not reopen N74: the named N74 cases (syn versions,
memchr feature sets) stay fixed, and the suites pass at the same totals.

## 7. New nits filed

- N76: orphan fingerprint dirs accumulate; a hash-only sweep is unsafe,
  mostly build-script units whose outputs live in `target/release/build/`.
- N77: debug same-profile dependency-universe duplicates are not separable
  by any static key; the prune cycles, about 27MB recompilation per cycle,
  zero lasting reduction.
- N78: release bin-vs-test universe pairs (section 6) are not separable by
  the 5-field key; on a fresh worktree the prune deletes one member of
  each pair and the next build rebuilds it. Steady-state release regrowth
  stays eliminated (worker cycle 3).

## 8. Logs and evidence

- Lane logs: `~/workspace/n74n75fix-logs/` (`prune/`, `verify/`, `coord/`,
  `coord-committed/`).
- W-PRUNE report: `n74n75fix-logs/prune/N74-REPORT.md`.
- W-VERIFY grounding note: `n74n75fix-logs/verify/n75-grounding-note.txt`.
- Coordinator acceptance: `n74n75fix-logs/coord/` (`build-release.log`,
  `suites-console.log`, `flows-console.log`, `verify-console.log`,
  `coord-flow-summary.txt`, `coord-verify-summary.txt`); derived suite
  logs `~/workspace/helion-n74n75fix-coord-logs/`.
- Regrowth experiment: `n74n75fix-logs/coord/regrowth/`
  (`grouped-before.txt`, `prune-release.delete`, `suite-summary.txt`,
  `rebuild-bins.log`, `rebuild-tests.log`).
- Incidents: two VM restarts killed runs mid-suite during the lane
  (W-PRUNE baseline attempt 1, proof-2 attempt 1; each re-run from scratch);
  a third restart killed the coordinator's regrowth rebuild mid-flight
  (rebuilt from scratch, logs under `coord/regrowth/`, never `/tmp`).
  Disk: 53G free at lane start.
